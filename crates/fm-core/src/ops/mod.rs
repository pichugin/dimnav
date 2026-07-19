//! File-operation pipeline (SPEC §5.4a / §5.4b / §5.6).
//!
//! Owns copy / move as cancellable operations that never block the UI thread and
//! always return a structured [`OpOutcome`]. To keep `fm-core` platform-agnostic
//! (no Tauri, no OS-native prompt code), every side-effecting *interaction* — user
//! decisions, progress, cancellation, privilege elevation — is delegated to an
//! [`OpObserver`] the caller implements. The transfer logic itself is pure and
//! unit-testable with a mock observer.
//!
//! Responsibilities (SPEC §5.4a):
//!
//! - **Collision detection** with FAR dialog shapes: single-file offers
//!   Skip/Overwrite/Cancel; multi-file adds Skip All / Overwrite All. Never
//!   silently overwrites. Sticky Skip-All / Overwrite-All modes are honoured.
//! - **Recursion guard**: normal directory recursion (copying a folder's full
//!   contents) is expected; copying a folder *into itself or a descendant* is
//!   detected and surfaced as a Skip-that-folder / Cancel prompt.
//! - **Progress & cancel**: count-based progress (one unit per file/dir); the
//!   observer's cancel flag is polled between items.
//! - **Errors** route through [`ErrorResolution`] (Retry / Skip / Skip All /
//!   Cancel / Elevate). Elevation is delegated to the observer, which runs the
//!   single item through the OS-native auth prompt — the core never sees a
//!   password.
//! - **Move** uses `fs::rename` (a whole-subtree move in one syscall), falling
//!   back to copy-then-remove across filesystem boundaries (`EXDEV`).
//!
//! Built so a custom operation can register against this same seam — the
//! [`crate::plugin::Operation`] extension point (§6a). Delete/Trash (next slice)
//! reuses this observer/engine.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::types::{ErrorResolution, OpKind, OpOutcome, OpRequest, OpStatus, Resolution};

/// The seam between the pure engine and the platform/UI. `src-tauri` implements
/// this over channels + Tauri events; unit tests implement it with a mock.
///
/// The `on_collision` / `on_error` calls **block** until the user answers (the
/// engine is synchronous and runs on a background thread on the Tauri side).
pub trait OpObserver {
    /// A unit finished (or was skipped): `done` of `total` items, `current` is the
    /// path just processed. Count-based, not byte-based, this slice.
    fn progress(&self, done: u64, total: u64, current: &str);

    /// A destination already exists. `multiple` enables the `*_all` choices in the
    /// dialog. Blocks for the user's decision.
    fn on_collision(&self, path: &str, multiple: bool) -> Resolution;

    /// An item failed. `offer_elevate` gates the OS-native Elevate choice (set when
    /// the failure was a permission error). Blocks for the user's decision.
    fn on_error(&self, path: &str, reason: &str, offer_elevate: bool) -> ErrorResolution;

    /// Re-run a single item with elevated privileges via the OS-native auth prompt.
    /// The app never handles the password. `Ok(())` means the item is now done.
    fn elevate_item(&self, kind: OpKind, src: &str, dest: &str) -> Result<(), String>;

    /// Cooperative cancellation, polled between items.
    fn is_cancelled(&self) -> bool;
}

/// Control-flow signal threaded through the recursive walk.
enum Flow {
    /// Keep going with the next sibling / source.
    Continue,
    /// Abort the whole operation (user cancelled).
    Cancel,
}

/// Outcome of driving the error dialog for one failed action.
enum ErrFlow {
    /// Re-run the action (Retry).
    Retry,
    /// Move on — the failure was recorded (Skip / Skip All / Elevate).
    Handled,
    /// Abort the whole operation.
    Cancel,
}

/// Run a copy or move described by `req`, reporting to `obs`. Synchronous and
/// blocking by design — the caller runs it off the UI thread. The returned
/// [`OpOutcome`] carries an empty `op_id`; the caller stamps the real id before
/// emitting the completion event.
pub fn run_transfer(req: &OpRequest, obs: &dyn OpObserver) -> OpOutcome {
    let dest = PathBuf::from(&req.dest);

    // Target resolution (FAR semantics):
    // - dest is an existing directory  → each source goes *into* it.
    // - dest is not an existing dir and there is exactly one source → dest is the
    //   full target path, i.e. a copy/move-with-rename (SPEC §5.4a). Its parent is
    //   created if missing.
    // - otherwise (several sources, dest absent) → dest is a new directory to
    //   create and drop the sources into.
    let into_dir = dest.is_dir() || req.sources.len() > 1;
    if into_dir {
        let _ = fs::create_dir_all(&dest);
    } else if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Pre-walk to size progress and decide single-vs-multiple dialog shape.
    let total: u64 = req.sources.iter().map(|s| count_items(Path::new(s))).sum();
    let multiple = total > 1;

    let mut engine = Engine {
        kind: req.kind,
        obs,
        total,
        done: 0,
        multiple,
        sticky: None,
        err_skip_all: false,
        copied: 0,
        skipped: 0,
        failed: 0,
        cancelled: false,
    };

    for src in &req.sources {
        if engine.obs.is_cancelled() {
            engine.cancelled = true;
            break;
        }
        let src = PathBuf::from(src);
        let Some(name) = src.file_name() else {
            continue;
        };
        // Into an existing/created dir we append the source's own name; otherwise
        // the (single) source adopts the dest path verbatim (rename).
        let target = if into_dir {
            dest.join(name)
        } else {
            dest.clone()
        };

        // Self-into-descendant guard: copying/moving a folder into itself or one
        // of its own children would recurse forever. Surface it as a skip/cancel
        // prompt (no elevation — it is not a permission problem).
        if is_self_or_descendant(&src, &target) {
            match engine.obs.on_error(
                &target.to_string_lossy(),
                "Cannot copy a folder into itself or its own subfolder",
                false,
            ) {
                ErrorResolution::Cancel => {
                    engine.cancelled = true;
                    break;
                }
                // Any non-cancel answer skips just this source.
                _ => {
                    engine.failed += 1;
                    continue;
                }
            }
        }

        if let Flow::Cancel = engine.transfer(&src, &target) {
            break;
        }
    }

    engine.finish()
}

/// Mutable state carried through one `run_transfer`.
struct Engine<'a> {
    kind: OpKind,
    obs: &'a dyn OpObserver,
    total: u64,
    done: u64,
    multiple: bool,
    /// Sticky collision mode once the user picks a `*_all` answer.
    sticky: Option<Resolution>,
    /// Sticky "Skip All" from the *error* dialog.
    err_skip_all: bool,
    copied: u64,
    skipped: u64,
    failed: u64,
    cancelled: bool,
}

impl Engine<'_> {
    /// Transfer one entry (`src`) to `target`, recursing into directories.
    fn transfer(&mut self, src: &Path, target: &Path) -> Flow {
        if self.obs.is_cancelled() {
            self.cancelled = true;
            return Flow::Cancel;
        }

        let meta = match fs::symlink_metadata(src) {
            Ok(m) => m,
            Err(e) => return self.on_item_error(src, target, &e),
        };
        // A symlink is treated as a single leaf (we recreate the link, never
        // follow it), so only *real* directories recurse.
        let is_dir = meta.file_type().is_dir();

        // Move fast path: a single rename relocates the whole subtree. Only when
        // the target does not yet exist (otherwise we need per-item collision
        // handling) and the move stays on one filesystem.
        if self.kind == OpKind::Move && !target.exists() {
            let subtree = count_items(src);
            match fs::rename(src, target) {
                Ok(()) => {
                    self.copied += subtree;
                    self.done += subtree;
                    self.obs
                        .progress(self.done, self.total, &target.to_string_lossy());
                    return Flow::Continue;
                }
                // Cross-device: fall through to copy-then-remove.
                Err(e) if is_cross_device(&e) => {}
                Err(e) => return self.on_item_error(src, target, &e),
            }
        }

        if is_dir {
            self.transfer_dir(src, target)
        } else {
            self.transfer_leaf(src, target, meta.file_type().is_symlink())
        }
    }

    /// Copy/merge a directory and recurse into its children.
    fn transfer_dir(&mut self, src: &Path, target: &Path) -> Flow {
        // Make sure `target` is a directory. If a *file* sits there, that is a
        // real collision the user must resolve.
        match fs::symlink_metadata(target) {
            Ok(m) if m.file_type().is_dir() => {} // merge into existing dir
            Ok(_) => match self.resolve_collision(target) {
                Resolution::Skip => {
                    self.skip_subtree(src, target);
                    return Flow::Continue;
                }
                Resolution::Cancel => {
                    self.cancelled = true;
                    return Flow::Cancel;
                }
                Resolution::Overwrite | Resolution::SkipAll | Resolution::OverwriteAll => {
                    // (SkipAll already handled inside resolve_collision when sticky.)
                    if let Flow::Cancel = self.ensure_removed(target) {
                        return Flow::Cancel;
                    }
                    if let Flow::Cancel = self.mkdir(src, target) {
                        return Flow::Cancel;
                    }
                }
            },
            Err(_) => {
                if let Flow::Cancel = self.mkdir(src, target) {
                    return Flow::Cancel;
                }
            }
        }

        // The directory node itself counts as one processed unit.
        self.done += 1;
        self.obs
            .progress(self.done, self.total, &target.to_string_lossy());

        let read = match fs::read_dir(src) {
            Ok(r) => r,
            Err(e) => return self.on_item_error(src, target, &e),
        };
        for child in read.flatten() {
            let child_src = child.path();
            let child_target = target.join(child.file_name());
            if let Flow::Cancel = self.transfer(&child_src, &child_target) {
                return Flow::Cancel;
            }
        }

        // For a move, drop the (now-emptied) source directory. If children were
        // skipped it is not empty and removal fails harmlessly.
        if self.kind == OpKind::Move {
            let _ = fs::remove_dir(src);
        }
        Flow::Continue
    }

    /// Copy/move a single file or symlink (one unit).
    fn transfer_leaf(&mut self, src: &Path, target: &Path, is_symlink: bool) -> Flow {
        if target.exists() || is_broken_symlink_at(target) {
            match self.resolve_collision(target) {
                Resolution::Skip => {
                    self.skipped += 1;
                    self.done += 1;
                    self.obs
                        .progress(self.done, self.total, &target.to_string_lossy());
                    return Flow::Continue;
                }
                Resolution::Cancel => {
                    self.cancelled = true;
                    return Flow::Cancel;
                }
                Resolution::Overwrite | Resolution::OverwriteAll | Resolution::SkipAll => {
                    // SkipAll only reaches here as a non-sticky first answer for a
                    // single item; treat identically to Skip.
                    if matches!(self.sticky, Some(Resolution::SkipAll)) {
                        self.skipped += 1;
                        self.done += 1;
                        self.obs
                            .progress(self.done, self.total, &target.to_string_lossy());
                        return Flow::Continue;
                    }
                    if let Flow::Cancel = self.ensure_removed(target) {
                        return Flow::Cancel;
                    }
                }
            }
        }

        // Perform the copy (recreating a symlink verbatim), retrying / elevating /
        // skipping per the error dialog.
        let flow = self.perform(src, target, || copy_leaf(src, target, is_symlink));
        if let Flow::Cancel = flow {
            return Flow::Cancel;
        }

        // For a cross-device move the source file lingers after the copy; remove it.
        if self.kind == OpKind::Move {
            let _ = fs::remove_file(src);
        }
        Flow::Continue
    }

    /// Create `target` as a directory, with error handling.
    fn mkdir(&mut self, src: &Path, target: &Path) -> Flow {
        self.perform(src, target, || match fs::create_dir(target) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            other => other,
        })
    }

    /// Remove whatever currently sits at `target` (file or dir tree) before an
    /// overwrite, with error handling.
    fn ensure_removed(&mut self, target: &Path) -> Flow {
        let t = target.to_path_buf();
        self.perform(target, target, move || match fs::symlink_metadata(&t) {
            Ok(m) if m.file_type().is_dir() => fs::remove_dir_all(&t),
            Ok(_) => fs::remove_file(&t),
            Err(_) => Ok(()),
        })
    }

    /// Run one fallible filesystem action, driving the error dialog on failure:
    /// Retry re-runs it, Skip/SkipAll record a failure and move on, Cancel aborts,
    /// Elevate re-runs the item through the OS-native prompt. On success bumps the
    /// progress counters.
    fn perform(
        &mut self,
        src: &Path,
        target: &Path,
        mut action: impl FnMut() -> io::Result<()>,
    ) -> Flow {
        loop {
            match action() {
                Ok(()) => {
                    self.copied += 1;
                    self.done += 1;
                    self.obs
                        .progress(self.done, self.total, &target.to_string_lossy());
                    return Flow::Continue;
                }
                Err(e) => match self.resolve_error(src, target, &e) {
                    ErrFlow::Retry => continue,
                    ErrFlow::Handled => return Flow::Continue,
                    ErrFlow::Cancel => return Flow::Cancel,
                },
            }
        }
    }

    /// Drive the error dialog for a failed action. Records counters for the
    /// non-retry resolutions and reports whether the caller should retry, move on,
    /// or abort.
    fn resolve_error(&mut self, src: &Path, target: &Path, err: &io::Error) -> ErrFlow {
        if self.err_skip_all {
            self.record_failure(target);
            return ErrFlow::Handled;
        }
        let offer_elevate = err.kind() == io::ErrorKind::PermissionDenied;
        match self
            .obs
            .on_error(&target.to_string_lossy(), &err.to_string(), offer_elevate)
        {
            ErrorResolution::Retry => ErrFlow::Retry,
            ErrorResolution::Skip => {
                self.record_failure(target);
                ErrFlow::Handled
            }
            ErrorResolution::SkipAll => {
                self.err_skip_all = true;
                self.record_failure(target);
                ErrFlow::Handled
            }
            ErrorResolution::Cancel => {
                self.cancelled = true;
                ErrFlow::Cancel
            }
            ErrorResolution::Elevate => {
                match self.obs.elevate_item(
                    self.kind,
                    &src.to_string_lossy(),
                    &target.to_string_lossy(),
                ) {
                    Ok(()) => {
                        self.copied += 1;
                        self.done += 1;
                        self.obs
                            .progress(self.done, self.total, &target.to_string_lossy());
                    }
                    // Elevation itself failed: treat as a skipped failure.
                    Err(_) => self.record_failure(target),
                }
                ErrFlow::Handled
            }
        }
    }

    /// Record one failed unit and advance progress past it.
    fn record_failure(&mut self, target: &Path) {
        self.failed += 1;
        self.done += 1;
        self.obs
            .progress(self.done, self.total, &target.to_string_lossy());
    }

    /// Resolve a name collision, honouring any sticky `*_all` mode. Returns a
    /// concrete Skip / Overwrite / Cancel decision for this item.
    fn resolve_collision(&mut self, target: &Path) -> Resolution {
        match self.sticky {
            Some(Resolution::SkipAll) => return Resolution::Skip,
            Some(Resolution::OverwriteAll) => return Resolution::Overwrite,
            _ => {}
        }
        let answer = self
            .obs
            .on_collision(&target.to_string_lossy(), self.multiple);
        match answer {
            Resolution::SkipAll => {
                self.sticky = Some(Resolution::SkipAll);
                Resolution::Skip
            }
            Resolution::OverwriteAll => {
                self.sticky = Some(Resolution::OverwriteAll);
                Resolution::Overwrite
            }
            other => other,
        }
    }

    /// Account a skipped directory subtree (collision Skip on a directory) as
    /// processed so progress still reaches `total`.
    fn skip_subtree(&mut self, src: &Path, target: &Path) {
        let n = count_items(src);
        self.skipped += n;
        self.done += n;
        self.obs
            .progress(self.done, self.total, &target.to_string_lossy());
    }

    /// Error while stat-ing / reading a source item (not a retryable copy). Retry
    /// on such an unactionable failure is treated as a skip so progress still
    /// completes.
    fn on_item_error(&mut self, src: &Path, target: &Path, err: &io::Error) -> Flow {
        match self.resolve_error(src, target, err) {
            ErrFlow::Cancel => Flow::Cancel,
            ErrFlow::Retry => {
                self.record_failure(target);
                Flow::Continue
            }
            ErrFlow::Handled => Flow::Continue,
        }
    }

    /// Build the terminal outcome from the counters.
    fn finish(self) -> OpOutcome {
        let verb = match self.kind {
            OpKind::Copy => "Copied",
            OpKind::Move => "Moved",
        };
        let (status, summary) = if self.cancelled {
            (
                OpStatus::Cancelled,
                format!(
                    "Cancelled — {} {} of {} item(s)",
                    verb.to_lowercase(),
                    self.done,
                    self.total
                ),
            )
        } else if self.failed > 0 && self.copied == 0 {
            (
                OpStatus::Failed,
                format!("Failed — {} error(s)", self.failed),
            )
        } else if self.failed > 0 {
            (
                OpStatus::Partial,
                format!(
                    "{} {}, {} failed, {} skipped",
                    verb, self.copied, self.failed, self.skipped
                ),
            )
        } else {
            (
                OpStatus::Success,
                format!(
                    "{} {} item(s){}",
                    verb,
                    self.copied,
                    if self.skipped > 0 {
                        format!(", {} skipped", self.skipped)
                    } else {
                        String::new()
                    }
                ),
            )
        };
        OpOutcome {
            op_id: String::new(),
            status,
            summary,
        }
    }
}

/// Recreate a file or symlink at `target`. Symlinks are copied verbatim (the link
/// itself, not the pointed-at contents); regular files use `fs::copy`.
fn copy_leaf(src: &Path, target: &Path, is_symlink: bool) -> io::Result<()> {
    if is_symlink {
        return copy_symlink(src, target);
    }
    fs::copy(src, target).map(|_| ())
}

#[cfg(unix)]
fn copy_symlink(src: &Path, target: &Path) -> io::Result<()> {
    let link = fs::read_link(src)?;
    std::os::unix::fs::symlink(link, target)
}

#[cfg(not(unix))]
fn copy_symlink(src: &Path, target: &Path) -> io::Result<()> {
    // Non-unix fallback: copy the resolved contents.
    fs::copy(src, target).map(|_| ())
}

/// Count items in a subtree — each file/symlink is 1, a directory is 1 + its
/// children. Used to size count-based progress and to advance it when a whole
/// subtree is moved by a single rename.
fn count_items(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_dir() => {
            let mut n = 1;
            if let Ok(read) = fs::read_dir(path) {
                for child in read.flatten() {
                    n += count_items(&child.path());
                }
            }
            n
        }
        _ => 1,
    }
}

/// Whether `target` is the source itself or lives inside it — the case that would
/// recurse forever. Compared lexically on normalized paths.
fn is_self_or_descendant(src: &Path, target: &Path) -> bool {
    let src = lexical_normalize(src);
    let target = lexical_normalize(target);
    target == src || target.starts_with(&src)
}

/// A cross-filesystem `rename` failure (`EXDEV`), which means we must fall back to
/// copy-then-remove.
fn is_cross_device(err: &io::Error) -> bool {
    // 18 == EXDEV on Linux and macOS.
    err.raw_os_error() == Some(18)
}

/// Is there a symlink at `path` whose target is missing? `Path::exists` follows
/// links and returns false for a broken one, but it still occupies the name, so a
/// copy there is a genuine collision.
fn is_broken_symlink_at(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Resolve `input` against `base` and normalize `.`/`..` **lexically** (no
/// filesystem access, so it works for not-yet-existing destinations). Absolute
/// inputs ignore `base`. This backs the editable F5/F6 destination prompt (§5.4a).
pub fn resolve_dest(base: &Path, input: &str) -> PathBuf {
    let joined = {
        let p = Path::new(input);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            base.join(p)
        }
    };
    lexical_normalize(&joined)
}

/// Collapse `.` and `..` components without touching the filesystem.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Scripted observer: returns queued collision/error answers in order and
    /// records progress calls. Cancels once `cancel_after` progress ticks elapse.
    struct MockObs {
        collisions: RefCell<Vec<Resolution>>,
        errors: RefCell<Vec<ErrorResolution>>,
        progress_calls: AtomicUsize,
        cancel_after: Option<usize>,
    }

    impl MockObs {
        fn new() -> Self {
            Self {
                collisions: RefCell::new(Vec::new()),
                errors: RefCell::new(Vec::new()),
                progress_calls: AtomicUsize::new(0),
                cancel_after: None,
            }
        }
        fn collisions(self, answers: Vec<Resolution>) -> Self {
            *self.collisions.borrow_mut() = answers;
            self
        }
        fn errors(self, answers: Vec<ErrorResolution>) -> Self {
            *self.errors.borrow_mut() = answers;
            self
        }
        fn cancel_after(mut self, n: usize) -> Self {
            self.cancel_after = Some(n);
            self
        }
    }

    impl OpObserver for MockObs {
        fn progress(&self, _done: u64, _total: u64, _current: &str) {
            self.progress_calls.fetch_add(1, Ordering::SeqCst);
        }
        fn on_collision(&self, _path: &str, _multiple: bool) -> Resolution {
            self.collisions
                .borrow_mut()
                .pop()
                .unwrap_or(Resolution::Cancel)
        }
        fn on_error(&self, _path: &str, _reason: &str, _offer: bool) -> ErrorResolution {
            self.errors
                .borrow_mut()
                .pop()
                .unwrap_or(ErrorResolution::Cancel)
        }
        fn elevate_item(&self, _k: OpKind, _s: &str, _d: &str) -> Result<(), String> {
            Ok(())
        }
        fn is_cancelled(&self) -> bool {
            match self.cancel_after {
                Some(n) => self.progress_calls.load(Ordering::SeqCst) >= n,
                None => false,
            }
        }
    }

    fn tmp() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fm_core_ops_{nanos}_{:p}", &nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn copies_a_nested_tree() {
        let root = tmp();
        let src = root.join("src");
        write(&src.join("a.txt"), "a");
        write(&src.join("sub/b.txt"), "b");
        let dest = root.join("dest");
        fs::create_dir_all(&dest).unwrap();

        let obs = MockObs::new();
        let req = OpRequest {
            kind: OpKind::Copy,
            sources: vec![src.to_string_lossy().into_owned()],
            dest: dest.to_string_lossy().into_owned(),
        };
        let out = run_transfer(&req, &obs);

        assert_eq!(out.status, OpStatus::Success);
        assert!(dest.join("src/a.txt").exists());
        assert!(dest.join("src/sub/b.txt").exists());
        assert!(src.join("a.txt").exists()); // copy leaves the source
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn move_removes_sources() {
        let root = tmp();
        let src = root.join("src");
        write(&src.join("a.txt"), "a");
        let dest = root.join("dest");
        fs::create_dir_all(&dest).unwrap();

        let obs = MockObs::new();
        let req = OpRequest {
            kind: OpKind::Move,
            sources: vec![src.to_string_lossy().into_owned()],
            dest: dest.to_string_lossy().into_owned(),
        };
        let out = run_transfer(&req, &obs);

        assert_eq!(out.status, OpStatus::Success);
        assert!(dest.join("src/a.txt").exists());
        assert!(!src.exists()); // move removes the source
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn collision_skip_leaves_target_untouched() {
        let root = tmp();
        let src = root.join("src");
        write(&src.join("a.txt"), "new");
        let dest = root.join("dest");
        write(&dest.join("src/a.txt"), "old");

        let obs = MockObs::new().collisions(vec![Resolution::Skip]);
        let req = OpRequest {
            kind: OpKind::Copy,
            sources: vec![src.to_string_lossy().into_owned()],
            dest: dest.to_string_lossy().into_owned(),
        };
        let out = run_transfer(&req, &obs);

        assert_eq!(out.status, OpStatus::Success);
        assert_eq!(fs::read_to_string(dest.join("src/a.txt")).unwrap(), "old");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn collision_overwrite_replaces_target() {
        let root = tmp();
        let src = root.join("src");
        write(&src.join("a.txt"), "new");
        let dest = root.join("dest");
        write(&dest.join("src/a.txt"), "old");

        let obs = MockObs::new().collisions(vec![Resolution::Overwrite]);
        let req = OpRequest {
            kind: OpKind::Copy,
            sources: vec![src.to_string_lossy().into_owned()],
            dest: dest.to_string_lossy().into_owned(),
        };
        run_transfer(&req, &obs);

        assert_eq!(fs::read_to_string(dest.join("src/a.txt")).unwrap(), "new");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn skip_all_is_sticky() {
        let root = tmp();
        let src = root.join("src");
        write(&src.join("a.txt"), "new");
        write(&src.join("b.txt"), "new");
        let dest = root.join("dest");
        write(&dest.join("src/a.txt"), "old");
        write(&dest.join("src/b.txt"), "old");

        // One SkipAll answer must cover both colliding files.
        let obs = MockObs::new().collisions(vec![Resolution::SkipAll]);
        let req = OpRequest {
            kind: OpKind::Copy,
            sources: vec![src.to_string_lossy().into_owned()],
            dest: dest.to_string_lossy().into_owned(),
        };
        run_transfer(&req, &obs);

        assert_eq!(fs::read_to_string(dest.join("src/a.txt")).unwrap(), "old");
        assert_eq!(fs::read_to_string(dest.join("src/b.txt")).unwrap(), "old");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn self_into_descendant_is_guarded() {
        let root = tmp();
        let src = root.join("folder");
        write(&src.join("a.txt"), "a");
        // dest == src's parent-of-target so target = src/folder lands inside src.
        let obs = MockObs::new().errors(vec![ErrorResolution::Skip]);
        let req = OpRequest {
            kind: OpKind::Copy,
            sources: vec![src.to_string_lossy().into_owned()],
            dest: src.to_string_lossy().into_owned(), // copy folder into itself
        };
        let out = run_transfer(&req, &obs);

        // Guarded and skipped: no runaway nesting created.
        assert!(!src.join("folder").exists());
        assert_eq!(out.status, OpStatus::Failed);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn single_source_to_nonexistent_dest_renames() {
        let root = tmp();
        write(&root.join("foo.txt"), "hi");

        // Move foo.txt to a new name in the same dir (dest is not an existing dir).
        let obs = MockObs::new();
        let req = OpRequest {
            kind: OpKind::Move,
            sources: vec![root.join("foo.txt").to_string_lossy().into_owned()],
            dest: root.join("bar.txt").to_string_lossy().into_owned(),
        };
        let out = run_transfer(&req, &obs);

        assert_eq!(out.status, OpStatus::Success);
        assert!(!root.join("foo.txt").exists());
        assert_eq!(fs::read_to_string(root.join("bar.txt")).unwrap(), "hi");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn multiple_sources_to_nonexistent_dest_creates_dir() {
        let root = tmp();
        write(&root.join("a.txt"), "a");
        write(&root.join("b.txt"), "b");
        let dest = root.join("newdir");

        let obs = MockObs::new();
        let req = OpRequest {
            kind: OpKind::Copy,
            sources: vec![
                root.join("a.txt").to_string_lossy().into_owned(),
                root.join("b.txt").to_string_lossy().into_owned(),
            ],
            dest: dest.to_string_lossy().into_owned(),
        };
        let out = run_transfer(&req, &obs);

        assert_eq!(out.status, OpStatus::Success);
        assert!(dest.join("a.txt").exists());
        assert!(dest.join("b.txt").exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cancel_mid_op_yields_cancelled() {
        let root = tmp();
        let src = root.join("src");
        for i in 0..10 {
            write(&src.join(format!("f{i}.txt")), "x");
        }
        let dest = root.join("dest");
        fs::create_dir_all(&dest).unwrap();

        let obs = MockObs::new().cancel_after(3);
        let req = OpRequest {
            kind: OpKind::Copy,
            sources: vec![src.to_string_lossy().into_owned()],
            dest: dest.to_string_lossy().into_owned(),
        };
        let out = run_transfer(&req, &obs);

        assert_eq!(out.status, OpStatus::Cancelled);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_dest_handles_relative_absolute_and_dotdot() {
        let base = Path::new("/home/user/dir");
        assert_eq!(
            resolve_dest(base, "sub"),
            PathBuf::from("/home/user/dir/sub")
        );
        assert_eq!(
            resolve_dest(base, "../other"),
            PathBuf::from("/home/user/other")
        );
        assert_eq!(
            resolve_dest(base, "/etc/hosts"),
            PathBuf::from("/etc/hosts")
        );
        assert_eq!(
            resolve_dest(base, "./x/./y"),
            PathBuf::from("/home/user/dir/x/y")
        );
    }
}
