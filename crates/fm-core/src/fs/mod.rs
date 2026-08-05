//! Filesystem engine.
//!
//! Responsibilities (SPEC §5.4 / §5.6): read directories into structured
//! [`DirListing`]s where each entry carries metadata (size, mtime, permissions),
//! symlink target, and executable detection; degrade unreadable entries to an
//! [`EntryMarker`] rather than failing the whole listing; and (later) provide
//! async, cancellable copy/move/delete primitives. Nothing here panics on a bad
//! entry.
//!
//! Kept platform-agnostic: unix-only metadata sits behind `#[cfg(unix)]` with
//! fallbacks so Phase 4 (Windows/Linux) only has to add arms.

pub mod watch;

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::types::{DirListing, Entry, EntryKind, EntryMarker, SortMode};

/// Resolves and memoizes uid/gid → name lookups for the span of a single
/// listing. Most directories are owned by one or two users, so a tiny cache
/// removes almost all of the (comparatively expensive) passwd/group lookups.
#[derive(Default)]
struct OwnerResolver {
    users: HashMap<u32, Option<String>>,
    groups: HashMap<u32, Option<String>>,
}

impl OwnerResolver {
    #[cfg(unix)]
    fn user(&mut self, uid: u32) -> Option<String> {
        self.users
            .entry(uid)
            .or_insert_with(|| {
                uzers::get_user_by_uid(uid).map(|u| u.name().to_string_lossy().into_owned())
            })
            .clone()
    }

    #[cfg(unix)]
    fn group(&mut self, gid: u32) -> Option<String> {
        self.groups
            .entry(gid)
            .or_insert_with(|| {
                uzers::get_group_by_gid(gid).map(|g| g.name().to_string_lossy().into_owned())
            })
            .clone()
    }

    #[cfg(not(unix))]
    fn user(&mut self, _uid: u32) -> Option<String> {
        None
    }

    #[cfg(not(unix))]
    fn group(&mut self, _gid: u32) -> Option<String> {
        None
    }
}

/// List a directory into a structured result. `show_hidden` controls whether
/// dotfiles are included (§5.8). The first entry is always `..` unless `path` is
/// a filesystem root (§5.2). Unreadable children become marker entries rather
/// than aborting the listing (§5.6).
///
/// Entries are returned sorted per `sort` (§5.8) with `..` pinned to the top.
pub fn list_dir(path: &str, show_hidden: bool, sort: SortMode) -> DirListing {
    let p = Path::new(path);
    let mut entries: Vec<Entry> = Vec::new();

    // `..` first for every non-root directory (§5.2).
    if p.parent().is_some() {
        entries.push(dotdot_entry());
    }

    let mut children: Vec<Entry> = Vec::new();
    let mut resolver = OwnerResolver::default();
    if let Ok(read) = fs::read_dir(p) {
        for dirent in read.flatten() {
            let name = dirent.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            children.push(entry_from_path(&dirent.path(), name, &mut resolver));
        }
    }

    sort_entries(&mut children, sort);
    entries.extend(children);

    DirListing {
        path: path.to_string(),
        entries,
    }
}

/// The synthetic parent-directory entry.
fn dotdot_entry() -> Entry {
    Entry {
        name: "..".to_string(),
        kind: EntryKind::Dir,
        size: 0,
        modified: 0,
        created: 0,
        permissions: 0,
        uid: 0,
        gid: 0,
        owner: None,
        group: None,
        nlink: 0,
        symlink_target: None,
        is_executable: false,
        marker: EntryMarker::Ok,
        computed_size: None,
    }
}

/// Build an [`Entry`] from a path, never failing — read errors become markers.
fn entry_from_path(path: &Path, name: String, resolver: &mut OwnerResolver) -> Entry {
    // `symlink_metadata` does not follow links, so we can detect symlinks and
    // read the link's own metadata for move/delete semantics (§5.4a).
    let link_meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => return unreadable_entry(name, &e),
    };

    let is_symlink = link_meta.file_type().is_symlink();
    let symlink_target = if is_symlink {
        fs::read_link(path)
            .ok()
            .map(|t| t.to_string_lossy().into_owned())
    } else {
        None
    };

    // For a symlink, does the target resolve? A broken link is marked (§5.6).
    let mut marker = EntryMarker::Ok;
    let target_meta = if is_symlink {
        match fs::metadata(path) {
            Ok(m) => Some(m),
            Err(_) => {
                marker = EntryMarker::Broken;
                None
            }
        }
    } else {
        None
    };

    // Kind reflects the link itself when it's a symlink; otherwise the object.
    let kind = if is_symlink {
        EntryKind::Symlink
    } else {
        kind_of(&link_meta.file_type())
    };

    // Metadata used for size/mtime/perms: the target's when a link resolves,
    // else the link's own.
    let meta = target_meta.as_ref().unwrap_or(&link_meta);

    let uid = uid_of(meta);
    let gid = gid_of(meta);

    Entry {
        name,
        kind,
        size: meta.len(),
        modified: mtime_secs(meta),
        created: ctime_secs(meta),
        permissions: mode_bits(meta),
        uid,
        gid,
        owner: resolver.user(uid),
        group: resolver.group(gid),
        nlink: nlink_of(meta),
        symlink_target,
        is_executable: is_executable(meta),
        marker,
        computed_size: None,
    }
}

/// Map a std file type to our [`EntryKind`] (symlinks handled by the caller).
fn kind_of(ft: &std::fs::FileType) -> EntryKind {
    if ft.is_dir() {
        EntryKind::Dir
    } else if ft.is_file() {
        EntryKind::File
    } else {
        EntryKind::Special
    }
}

/// An entry whose metadata could not be read — surfaced as a Denied marker so a
/// single bad child never fails the whole listing (§5.6).
fn unreadable_entry(name: String, _err: &std::io::Error) -> Entry {
    Entry {
        name,
        kind: EntryKind::Special,
        size: 0,
        modified: 0,
        created: 0,
        permissions: 0,
        uid: 0,
        gid: 0,
        owner: None,
        group: None,
        nlink: 0,
        symlink_target: None,
        is_executable: false,
        marker: EntryMarker::Denied,
        computed_size: None,
    }
}

/// Modification time as Unix seconds, or 0 if unavailable.
fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Creation/birth time as Unix seconds, or 0 if the platform does not report it.
fn ctime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn uid_of(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.uid()
}

#[cfg(not(unix))]
fn uid_of(_meta: &std::fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn gid_of(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.gid()
}

#[cfg(not(unix))]
fn gid_of(_meta: &std::fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn nlink_of(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() as u32
}

#[cfg(not(unix))]
fn nlink_of(_meta: &std::fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn mode_bits(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode()
}

#[cfg(not(unix))]
fn mode_bits(_meta: &std::fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.is_file() && (meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Sort a listing in place. `..` (and any parent entry) stays pinned first, and
/// directories are grouped ahead of files in **every** mode — FAR behaviour, and
/// what makes a two-panel listing readable (§5.8). Within those groups:
///
/// - `NameFoldersFirst` (default): case-insensitive by name.
/// - `TypeName`: by extension, then by name.
/// - `Size`: largest first (directories carry no meaningful size, so they fall
///   back to name).
/// - `Date`: newest first.
///
/// Ties always break on name, so every mode is a total, stable-looking order.
pub fn sort_entries(entries: &mut [Entry], mode: SortMode) {
    entries.sort_by(|a, b| {
        // Pin `..` to the very top regardless of mode.
        let a_dotdot = a.name == "..";
        let b_dotdot = b.name == "..";
        if a_dotdot != b_dotdot {
            return b_dotdot.cmp(&a_dotdot); // dotdot (true) sorts first
        }

        let grouped = folders_first(a, b);
        let both_dirs = a.kind == EntryKind::Dir && b.kind == EntryKind::Dir;

        let within = match mode {
            SortMode::NameFoldersFirst => by_name(a, b),
            SortMode::TypeName => by_ext(a, b).then(by_name(a, b)),
            // Directories have no meaningful size; order them by name instead.
            SortMode::Size if both_dirs => by_name(a, b),
            SortMode::Size => b.size.cmp(&a.size).then(by_name(a, b)),
            SortMode::Date => b.modified.cmp(&a.modified).then(by_name(a, b)),
        };
        grouped.then(within)
    });
}

/// Directories (and dir-like symlinks) before files.
fn folders_first(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    let rank = |e: &Entry| if e.kind == EntryKind::Dir { 0 } else { 1 };
    rank(a).cmp(&rank(b))
}

/// Case-insensitive name comparison.
fn by_name(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    a.name.to_lowercase().cmp(&b.name.to_lowercase())
}

/// Case-insensitive extension comparison; extension-less entries sort first. A
/// leading dot does not start an extension (`.gitignore` has none), matching how
/// the listing colours already classify names.
fn by_ext(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    ext_of(&a.name).cmp(&ext_of(&b.name))
}

fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// Create a directory named `name` inside `parent` (F7, §5.4). `name` may be a
/// nested relative path (`a/b/c`), created in full. Returns a human-readable error
/// on failure. Idempotent: creating an existing directory succeeds.
pub fn make_dir(parent: &Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("directory name cannot be empty".to_string());
    }
    let target = parent.join(name);
    fs::create_dir_all(&target).map_err(|e| format!("could not create directory: {e}"))
}

/// Rename the entry `old` to `new` in place inside `dir` (Shift+F6, §5.4). This is a
/// pure in-place rename: `new` must be a bare filename (no path separator), so it
/// can never silently turn into a move. Refuses to rename `..`, and **errors if a
/// file named `new` already exists** rather than overwriting it (§5.4a — never
/// silently overwrite).
pub fn rename_entry(dir: &Path, old: &str, new: &str) -> Result<(), String> {
    if old == ".." {
        return Err("cannot rename the parent directory".to_string());
    }
    let new = new.trim();
    if new.is_empty() {
        return Err("name cannot be empty".to_string());
    }
    if new == "." || new == ".." {
        return Err("invalid name".to_string());
    }
    if new.chars().any(std::path::is_separator) {
        return Err("name cannot contain a path separator".to_string());
    }
    if new == old {
        return Ok(()); // no-op rename
    }

    let target = dir.join(new);
    // `symlink_metadata` (not `exists`) so a broken symlink occupying the name still
    // counts as a collision.
    if fs::symlink_metadata(&target).is_ok() {
        return Err(format!("a file named \"{new}\" already exists"));
    }
    fs::rename(dir.join(old), &target).map_err(|e| format!("could not rename: {e}"))
}

/// Belt-and-suspenders cap on directory nesting for [`dir_size`], guarding
/// against pathological trees. The real cycle guard is that we never follow
/// symlinks (see below); this only bounds absurdly deep real hierarchies.
const MAX_DIR_DEPTH: usize = 4096;

/// Recursively compute the on-disk size of a directory subtree, returning
/// `(total_bytes, dir_mtime_secs)` where `dir_mtime_secs` is the mtime of the
/// top directory (used by the cache to detect direct-child changes).
///
/// Guard rails against infinite recursion (§ hardening):
/// - Uses `symlink_metadata` and **descends only into real directories** —
///   symlinks are never followed, so symlink cycles are impossible (the same
///   technique `ops::count_items` relies on).
/// - Iterative (explicit stack), so deep trees can't overflow the call stack.
/// - Bounded by [`MAX_DIR_DEPTH`].
/// - Deduplicates hardlinks by `(dev, ino)` on unix, so each physical inode is
///   counted once (accurate, `du`-like) — this also doubles as an extra cycle
///   guard for exotic layouts (bind mounts, etc.).
/// - Unreadable subdirectories are skipped rather than aborting the walk.
pub fn dir_size(path: &Path) -> (u64, i64) {
    let root_mtime = fs::symlink_metadata(path).map(|m| mtime_secs(&m)).unwrap_or(0);

    let mut total: u64 = 0;
    #[cfg(unix)]
    let mut seen: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    // Stack of (dir_path, depth). We start from the root's children.
    let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(path.to_path_buf(), 0)];

    while let Some((dir, depth)) = stack.pop() {
        if depth >= MAX_DIR_DEPTH {
            continue;
        }
        let read = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue, // unreadable subdir — skip, don't abort
        };
        for child in read.flatten() {
            let cpath = child.path();
            // `symlink_metadata`: a symlink reports as a symlink (never a dir),
            // so it is counted as a leaf and never descended into.
            let meta = match fs::symlink_metadata(&cpath) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let ft = meta.file_type();
            // Symlinks are pointers, not content: never followed (this is the
            // cycle guard) and their own bytes are not counted.
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push((cpath, depth + 1));
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                // Count each physical inode once (hardlink dedup).
                if meta.nlink() > 1 && !seen.insert((meta.dev(), meta.ino())) {
                    continue;
                }
            }
            total += meta.len();
        }
    }

    (total, root_mtime)
}

/// The mtime (Unix seconds) of `path`, or `None` if it cannot be stat'd (e.g. it
/// was removed). Used by the size cache to detect direct-child changes.
pub fn dir_mtime(path: &Path) -> Option<i64> {
    fs::symlink_metadata(path).ok().map(|m| mtime_secs(&m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Create a unique temp directory with a known layout, returning its path.
    fn make_fixture() -> std::path::PathBuf {
        let dir = crate::testutil::unique_dir("fm_core_fs_test");
        fs::create_dir_all(dir.join("Beta")).unwrap();
        fs::create_dir_all(dir.join("alpha")).unwrap();
        let mut f = fs::File::create(dir.join("gamma.txt")).unwrap();
        writeln!(f, "hi").unwrap();
        fs::File::create(dir.join(".hidden")).unwrap();
        dir
    }

    #[test]
    fn lists_dotdot_first_folders_first_hidden_respected() {
        let dir = make_fixture();

        // Hidden shown: `..`, then folders (alpha, Beta case-insensitive), then file.
        let shown = list_dir(dir.to_str().unwrap(), true, SortMode::NameFoldersFirst);
        let names: Vec<&str> = shown.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "alpha", "Beta", ".hidden", "gamma.txt"]);
        assert_eq!(shown.entries[0].kind, EntryKind::Dir); // ..
        assert_eq!(shown.entries[1].kind, EntryKind::Dir); // alpha

        // Hidden filtered out.
        let hidden = list_dir(dir.to_str().unwrap(), false, SortMode::NameFoldersFirst);
        let names: Vec<&str> = hidden.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "alpha", "Beta", "gamma.txt"]);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unreadable_dir_yields_only_dotdot_not_a_panic() {
        // A path that does not exist: read_dir fails, but we still get a listing
        // with `..` and no crash (§5.6).
        let listing = list_dir(
            "/definitely/not/a/real/path/xyzzy",
            true,
            SortMode::NameFoldersFirst,
        );
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, [".."]);
    }

    /// A bare entry for the pure sorting tests.
    fn ent(name: &str, kind: EntryKind, size: u64, modified: i64) -> Entry {
        Entry {
            name: name.to_string(),
            kind,
            size,
            modified,
            created: 0,
            permissions: 0,
            uid: 0,
            gid: 0,
            owner: None,
            group: None,
            nlink: 0,
            symlink_target: None,
            is_executable: false,
            marker: EntryMarker::Ok,
            computed_size: None,
        }
    }

    #[test]
    fn every_sort_mode_pins_dotdot_and_groups_folders_first() {
        let sample = || {
            vec![
                ent("m.txt", EntryKind::File, 50, 300),
                ent("..", EntryKind::Dir, 0, 0),
                ent("a.zip", EntryKind::File, 10, 100),
                ent("zeta", EntryKind::Dir, 0, 900),
                ent("b.txt", EntryKind::File, 500, 200),
                ent("alpha", EntryKind::Dir, 0, 50),
            ]
        };
        let names = |v: &[Entry]| -> Vec<String> { v.iter().map(|e| e.name.clone()).collect() };

        let mut v = sample();
        sort_entries(&mut v, SortMode::NameFoldersFirst);
        assert_eq!(names(&v), ["..", "alpha", "zeta", "a.zip", "b.txt", "m.txt"]);

        // Extension first (txt before zip), then name within an extension.
        let mut v = sample();
        sort_entries(&mut v, SortMode::TypeName);
        assert_eq!(names(&v), ["..", "alpha", "zeta", "b.txt", "m.txt", "a.zip"]);

        // Largest first; dirs have no size so they stay in name order.
        let mut v = sample();
        sort_entries(&mut v, SortMode::Size);
        assert_eq!(names(&v), ["..", "alpha", "zeta", "b.txt", "m.txt", "a.zip"]);

        // Newest first, folders still grouped ahead of files.
        let mut v = sample();
        sort_entries(&mut v, SortMode::Date);
        assert_eq!(names(&v), ["..", "zeta", "alpha", "m.txt", "b.txt", "a.zip"]);
    }

    #[test]
    fn make_dir_creates_nested_and_rejects_empty() {
        let dir = make_fixture();
        assert!(make_dir(&dir, "new/inner").is_ok());
        assert!(dir.join("new/inner").is_dir());
        assert!(make_dir(&dir, "   ").is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rename_entry_moves_name_and_guards_collisions() {
        let dir = make_fixture(); // has gamma.txt, alpha/, Beta/

        // Happy path.
        assert!(rename_entry(&dir, "gamma.txt", "delta.txt").is_ok());
        assert!(!dir.join("gamma.txt").exists());
        assert!(dir.join("delta.txt").exists());

        // Collision: alpha already exists.
        let err = rename_entry(&dir, "Beta", "alpha").unwrap_err();
        assert!(err.contains("already exists"));
        assert!(dir.join("Beta").is_dir()); // untouched

        // Refuse `..`, separators, and empty names.
        assert!(rename_entry(&dir, "..", "x").is_err());
        assert!(rename_entry(&dir, "delta.txt", "a/b").is_err());
        assert!(rename_entry(&dir, "delta.txt", "  ").is_err());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dir_size_sums_recursively_and_survives_symlink_loops() {
        let root = crate::testutil::unique_dir("fm_core_dirsize");
        fs::create_dir_all(root.join("sub/deep")).unwrap();

        // Known byte sizes at several depths.
        fs::write(root.join("a.bin"), vec![0u8; 1000]).unwrap();
        fs::write(root.join("sub/b.bin"), vec![0u8; 200]).unwrap();
        fs::write(root.join("sub/deep/c.bin"), vec![0u8; 30]).unwrap();

        // A symlink pointing back at the root would recurse forever if followed.
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&root, root.join("loop")).unwrap();
        }

        let (total, mtime) = dir_size(&root);
        assert_eq!(total, 1230, "should sum all files once, not follow the loop");
        assert!(mtime > 0);

        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn dir_size_dedups_hardlinks() {
        let root = crate::testutil::unique_dir("fm_core_dirsize_hl");
        fs::write(root.join("orig.bin"), vec![0u8; 500]).unwrap();
        fs::hard_link(root.join("orig.bin"), root.join("link.bin")).unwrap();

        let (total, _) = dir_size(&root);
        assert_eq!(total, 500, "hardlinked inode counted once");

        fs::remove_dir_all(&root).ok();
    }
}
