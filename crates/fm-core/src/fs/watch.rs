//! Directory identity, and what to do when a panel's directory stops being there
//! (SPEC §5.6).
//!
//! [`crate::fs::list_dir`] cannot answer this question: it is infallible, so a
//! deleted directory and an empty one both come back as a listing containing just
//! `..`. Telling them apart needs an explicit probe.
//!
//! The trick is that a directory has an **identity** — its `(dev, ino)` — that is
//! independent of its path. A panel holds a descriptor on its open directory, and
//! that descriptor keeps referring to the same directory no matter how the path
//! changes, so a *renamed* folder can be followed instead of abandoned. Climbing
//! to the nearest surviving parent, the obvious reflex, is the right answer only
//! when the directory is genuinely gone; doing it on a rename would throw away
//! the cursor, the selection, and the scroll position in response to something
//! the user would describe as "nothing happened".
//!
//! ## Measured platform behaviour (macOS/APFS)
//!
//! These are not assumptions — each one was probed before this module was
//! written, and two of them contradict the obvious guess:
//!
//! - `fcntl(F_GETPATH)` resolves the descriptor's path *at call time*, so it
//!   follows a rename of the directory itself **and of any ancestor, at any
//!   depth**. It returns the user-facing `/Users/...` path; it is
//!   `F_GETPATH_NOFIRMLINK` that yields `/System/Volumes/Data/Users/...`, so no
//!   firmlink normalization is needed here.
//! - `F_GETPATH` **cannot establish liveness**. After the directory is removed it
//!   keeps returning the now-stale path instead of failing. If something new is
//!   then created at that path, `stat` on it succeeds and it *is* a directory —
//!   so "does the path still exist?" reports a healthy directory that is not
//!   ours. Only `fstat` on the descriptor catches that.
//! - `st_nlink == 0` is not the deletion signal: `fstat` itself fails, with
//!   `ENOENT` for a removed directory and `EBADF` once the volume is unmounted.
//!   That errno split is what distinguishes those two cases, which is why this
//!   module needs no `/Volumes` path heuristic.
//!
//! Kept behind a small platform seam: the macOS arm ships now, and Phase 4 adds
//! `/proc/self/fd` (Linux) and `GetFinalPathNameByHandle` (Windows) without the
//! decision table below changing at all.

use std::path::{Path, PathBuf};

use crate::types::{OnLost, PanelNotice, PanelNoticeKind, WatchPrefs};

// ---------------------------------------------------------------------------
// Observations
// ---------------------------------------------------------------------------

/// What `fstat` on the panel's directory descriptor reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// The directory is still there, whatever its path now is.
    Alive { dev: u64, ino: u64 },
    /// `ENOENT` — the directory was removed.
    Gone,
    /// `EBADF` — the volume went away underneath it.
    VolumeGone,
    /// Some other error. Deliberately distinct from `Gone`: an unexpected errno
    /// must never be enough to move the user out of their directory.
    Unknown,
}

/// The raw facts a probe collects, separated from [`decide`] so the decision
/// table is unit-testable without touching a filesystem.
#[derive(Debug, Clone)]
pub struct Observation {
    pub liveness: Liveness,
    /// `F_GETPATH`, when it succeeded. Only meaningful if `liveness` is `Alive`.
    pub current_path: Option<String>,
    /// Identity of whatever now sits at the path the panel believes it is at, if
    /// anything does. This is what separates "replaced" from "deleted".
    pub at_panel_path: Option<(u64, u64)>,
    /// Whether the directory could actually be read (TCC / permissions).
    pub readable: bool,
}

/// What happened to a directory a panel had open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirFate {
    /// Same directory, same path. The overwhelmingly common case.
    Alive,
    /// Same directory, new path — renamed, or moved, or an ancestor was.
    Moved(String),
    /// Same directory, and it is now in the Trash.
    Trashed,
    /// A *different* directory now occupies the path.
    Replaced,
    /// The directory was removed and nothing took its place.
    Deleted,
    /// The volume was ejected.
    Unmounted,
    /// Still there, but no longer readable.
    Unreadable,
}

/// Classify a probe. Pure — every filesystem fact arrives via [`Observation`].
pub fn decide(obs: &Observation, panel_path: &str, home: &Path, follow_moves: bool) -> DirFate {
    match obs.liveness {
        Liveness::VolumeGone => DirFate::Unmounted,

        // Our directory is gone. Whether the user should be moved depends on
        // what, if anything, took over the path they were looking at.
        Liveness::Gone => match obs.at_panel_path {
            Some(_) => DirFate::Replaced,
            None => DirFate::Deleted,
        },

        // An errno we did not anticipate. Stay put and let readability decide —
        // guessing "deleted" here would yank the panel on a transient error.
        Liveness::Unknown => {
            if obs.readable {
                DirFate::Alive
            } else {
                DirFate::Unreadable
            }
        }

        Liveness::Alive { dev, ino } => {
            let current = obs.current_path.as_deref().unwrap_or(panel_path);

            // Trashing is a deletion the user asked for. Following the folder
            // into ~/.Trash would read as the panel teleporting, and the Trash
            // renames on collision so the landing name would be mangled too.
            if is_in_trash(Path::new(current), home) {
                return DirFate::Trashed;
            }

            // "Did we move?" is an identity question, not a string one. On macOS
            // /var, /tmp and /etc are symlinks into /private, so F_GETPATH's
            // resolved path routinely differs from the path the user actually
            // navigated — comparing text would report a move on every probe for
            // any panel under those roots. Asking whether the panel's path still
            // leads to *this* directory is both correct and cheaper.
            if obs.at_panel_path != Some((dev, ino)) {
                return if follow_moves {
                    DirFate::Moved(current.to_string())
                } else {
                    DirFate::Deleted
                };
            }

            if obs.readable {
                DirFate::Alive
            } else {
                DirFate::Unreadable
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Recovery
// ---------------------------------------------------------------------------

/// What the panel should do about a [`DirFate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirRecovery {
    /// Re-read the same path, keeping cursor, selection and scroll.
    Reload,
    /// Re-read the same path from scratch — it is a different directory now.
    ReloadFresh(PanelNoticeSpec),
    /// The directory lives somewhere else now; go there, keep the user's place.
    Follow {
        path: String,
        notice: PanelNoticeSpec,
    },
    /// Leave the directory behind and land somewhere that still exists.
    Fallback {
        path: String,
        /// Name of the directory that vanished, so the cursor can land where it
        /// used to sort rather than at the top of the listing.
        cursor_hint: Option<String>,
        notice: PanelNoticeSpec,
    },
    /// Do not move the panel. Explain instead.
    Hold(PanelNoticeSpec),
}

/// A notice before it is rendered — kind plus the path it concerns. Kept separate
/// from [`PanelNotice`] so [`recover`] stays pure and free of message formatting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelNoticeSpec {
    pub kind: PanelNoticeKind,
    pub path: String,
}

impl PanelNoticeSpec {
    fn new(kind: PanelNoticeKind, path: &str) -> Self {
        Self {
            kind,
            path: path.to_string(),
        }
    }

    /// Compose the user-facing sentence. The core owns the wording for the same
    /// reason it owns [`crate::types::OpErrorInfo::reason`]: it is the side that
    /// knows the old path and the volume, and the frontend must stay a renderer.
    pub fn into_notice(self) -> PanelNotice {
        // Every one of these leads with what happened and puts the path last.
        // Paths are long and the notice is one line, so a path-first sentence
        // ellipsizes away exactly the part that carries the meaning.
        let message = match self.kind {
            PanelNoticeKind::Moved => format!("Followed this folder from {}", self.path),
            PanelNoticeKind::Replaced => {
                format!("Replaced by a different folder — was {}", self.path)
            }
            PanelNoticeKind::Deleted => format!("Folder deleted — was {}", self.path),
            PanelNoticeKind::Trashed => format!("Folder moved to the Trash — was {}", self.path),
            PanelNoticeKind::Unmounted => {
                format!("Volume ejected — was {}", self.path)
            }
            PanelNoticeKind::Denied => format!("Permission denied reading {}", self.path),
        };
        PanelNotice {
            kind: self.kind,
            message,
            path: self.path,
        }
    }
}

/// Map a fate onto an action. Pure: `resolve_ancestor` supplies the only
/// filesystem-dependent input, so the policy can be tested with a stub.
pub fn recover(
    fate: DirFate,
    panel_path: &str,
    prefs: &WatchPrefs,
    home: &str,
    resolve_ancestor: impl Fn(&str) -> Option<String>,
) -> DirRecovery {
    // Where a panel lands when its directory is gone for good.
    let land = |kind: PanelNoticeKind| -> DirRecovery {
        let target = match prefs.on_lost {
            OnLost::Home => Some(home.to_string()),
            OnLost::NearestAncestor => resolve_ancestor(panel_path),
        }
        // An ancestor walk can come up empty (the whole volume went away, or
        // nothing up the chain is readable); home is the last resort either way.
        .unwrap_or_else(|| home.to_string());

        DirRecovery::Fallback {
            path: target,
            cursor_hint: child_name(panel_path),
            notice: PanelNoticeSpec::new(kind, panel_path),
        }
    };

    match fate {
        DirFate::Alive => DirRecovery::Reload,
        DirFate::Moved(to) => DirRecovery::Follow {
            path: to,
            notice: PanelNoticeSpec::new(PanelNoticeKind::Moved, panel_path),
        },
        DirFate::Replaced => {
            DirRecovery::ReloadFresh(PanelNoticeSpec::new(PanelNoticeKind::Replaced, panel_path))
        }
        DirFate::Trashed => land(PanelNoticeKind::Trashed),
        DirFate::Deleted => land(PanelNoticeKind::Deleted),
        // An ejected volume has no ancestor worth walking to: /Volumes still
        // exists but landing there is not useful, so this always goes home.
        DirFate::Unmounted => DirRecovery::Fallback {
            path: home.to_string(),
            cursor_hint: None,
            notice: PanelNoticeSpec::new(PanelNoticeKind::Unmounted, panel_path),
        },
        // Deliberately does not move: navigating away would hide the actual
        // problem, and the user may just need to grant access and retry (§5.6).
        DirFate::Unreadable => {
            DirRecovery::Hold(PanelNoticeSpec::new(PanelNoticeKind::Denied, panel_path))
        }
    }
}

/// The final path component — the vanished directory's own name.
fn child_name(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// Is this path inside a Trash directory?
///
/// Covers both macOS spellings: the per-user `~/.Trash`, and the per-volume
/// `/Volumes/<name>/.Trashes/<uid>` used for external disks.
pub fn is_in_trash(path: &Path, home: &Path) -> bool {
    if path.starts_with(home.join(".Trash")) {
        return true;
    }
    path.components()
        .any(|c| c.as_os_str() == ".Trashes" || c.as_os_str() == ".Trash")
}

/// Closest ancestor of `path` that exists and can actually be listed.
///
/// Readability matters as much as existence: landing the panel on a directory it
/// cannot read would just move the error somewhere less obvious.
pub fn nearest_readable_ancestor(path: &str) -> Option<String> {
    let mut cur = Path::new(path).parent();
    while let Some(p) = cur {
        if std::fs::read_dir(p).is_ok() {
            return Some(p.to_string_lossy().into_owned());
        }
        cur = p.parent();
    }
    None
}

// ---------------------------------------------------------------------------
// The platform seam
// ---------------------------------------------------------------------------

/// A live reference to a panel's open directory, independent of its path.
///
/// Dropping it releases the descriptor.
#[derive(Debug)]
pub struct DirHandle {
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
    dev: u64,
    ino: u64,
    /// The path this handle was opened at, for `Moved` comparisons.
    opened_at: PathBuf,
}

impl DirHandle {
    /// Identity of the directory this handle refers to.
    pub fn identity(&self) -> (u64, u64) {
        (self.dev, self.ino)
    }

    /// The path the directory was at when the handle was opened.
    pub fn opened_at(&self) -> &Path {
        &self.opened_at
    }
}

#[cfg(unix)]
mod imp {
    use super::{DirHandle, Liveness, Observation};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::path::Path;

    // macOS: O_EVTONLY means "reference this for events only". Measured: it is
    // what lets the user still eject the volume — a plain O_RDONLY descriptor on
    // a panel's directory makes `hdiutil detach` fail with "Resource busy".
    #[cfg(target_os = "macos")]
    const OPEN_FLAGS: libc::c_int = libc::O_EVTONLY | libc::O_DIRECTORY;
    // Other unixes have no O_EVTONLY. O_PATH would be the Linux analogue and is
    // the right thing to reach for in Phase 4; O_RDONLY keeps this compiling and
    // correct in the meantime.
    #[cfg(all(unix, not(target_os = "macos")))]
    const OPEN_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY;

    impl DirHandle {
        /// Open a directory for identity tracking.
        pub fn open(path: &Path) -> io::Result<Self> {
            let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;

            // SAFETY: `c` is a valid NUL-terminated C string for the duration of
            // the call, and the returned descriptor is immediately handed to
            // OwnedFd so it is closed exactly once.
            let raw = unsafe { libc::open(c.as_ptr(), OPEN_FLAGS) };
            if raw < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = unsafe { OwnedFd::from_raw_fd(raw) };

            let st = fstat(fd.as_raw_fd())?;
            Ok(DirHandle {
                fd,
                dev: st.st_dev as u64,
                ino: st.st_ino as u64,
                opened_at: path.to_path_buf(),
            })
        }

        /// Collect the facts [`super::decide`] needs.
        pub fn observe(&self, panel_path: &str) -> Observation {
            let liveness = match fstat(self.fd.as_raw_fd()) {
                // nlink 0 is checked as well as the errno: a filesystem that
                // reports an unlinked directory instead of failing outright
                // still has to be read as "gone".
                // The widths of st_dev/st_ino differ across the unixes (st_ino is
                // already u64 on macOS, st_dev is a signed int), so one of these
                // casts is a no-op on any given target.
                #[allow(clippy::unnecessary_cast)]
                Ok(st) if st.st_nlink > 0 => Liveness::Alive {
                    dev: st.st_dev as u64,
                    ino: st.st_ino as u64,
                },
                Ok(_) => Liveness::Gone,
                Err(e) => match e.raw_os_error() {
                    Some(libc::ENOENT) => Liveness::Gone,
                    Some(libc::EBADF) => Liveness::VolumeGone,
                    _ => Liveness::Unknown,
                },
            };

            // Only ask where the directory is if it is still there — after a
            // deletion F_GETPATH keeps handing back the stale path.
            let current_path = match liveness {
                Liveness::Alive { .. } => current_path(self.fd.as_raw_fd()),
                _ => None,
            };

            let at_panel_path = std::fs::metadata(panel_path).ok().map(|m| {
                use std::os::unix::fs::MetadataExt;
                (m.dev(), m.ino())
            });

            Observation {
                liveness,
                current_path,
                at_panel_path,
                readable: std::fs::read_dir(panel_path).is_ok(),
            }
        }
    }

    fn fstat(fd: std::os::fd::RawFd) -> io::Result<libc::stat> {
        // SAFETY: `fd` is owned by the caller's DirHandle and open for the
        // duration of the call; `st` is fully initialized by a successful fstat.
        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        let rc = unsafe { libc::fstat(fd, st.as_mut_ptr()) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { st.assume_init() })
    }

    /// The descriptor's path *right now* — this is what follows renames.
    #[cfg(target_os = "macos")]
    fn current_path(fd: std::os::fd::RawFd) -> Option<String> {
        // Plain F_GETPATH, deliberately not F_GETPATH_NOFIRMLINK: the former
        // returns the user-facing /Users/... path, the latter the firmlinked
        // /System/Volumes/Data/... one nobody wants to see in a path bar.
        let mut buf = vec![0u8; libc::PATH_MAX as usize];
        // SAFETY: buf is PATH_MAX bytes, which is the size F_GETPATH requires.
        let rc = unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        buf.truncate(end);
        String::from_utf8(buf).ok()
    }

    /// Phase 4: Linux resolves this through `/proc/self/fd/<n>`, which appends
    /// " (deleted)" for unlinked paths.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn current_path(fd: std::os::fd::RawFd) -> Option<String> {
        std::fs::read_link(format!("/proc/self/fd/{fd}"))
            .ok()
            .and_then(|p| p.to_str().map(str::to_string))
            .filter(|p| !p.ends_with(" (deleted)"))
    }
}

#[cfg(not(unix))]
mod imp {
    use super::{DirHandle, Liveness, Observation};
    use std::io;
    use std::path::Path;

    impl DirHandle {
        /// Phase 4: Windows opens the directory with `FILE_FLAG_BACKUP_SEMANTICS`
        /// and resolves it with `GetFinalPathNameByHandle`. Note it must also
        /// pass `FILE_SHARE_DELETE`, or holding the handle blocks the user from
        /// deleting the directory — the same trap `O_EVTONLY` avoids on macOS.
        pub fn open(_path: &Path) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "directory watching is not implemented on this platform yet",
            ))
        }

        pub fn observe(&self, panel_path: &str) -> Observation {
            Observation {
                liveness: Liveness::Unknown,
                current_path: None,
                at_panel_path: None,
                readable: std::fs::read_dir(panel_path).is_ok(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/Users/tester")
    }

    /// The directory is still reachable at the path the panel believes in.
    fn alive_at(path: &str) -> Observation {
        Observation {
            liveness: Liveness::Alive { dev: 1, ino: 42 },
            current_path: Some(path.to_string()),
            at_panel_path: Some((1, 42)),
            readable: true,
        }
    }

    /// The directory is alive at `now`, but the panel's old path no longer leads
    /// to it — nothing is there any more.
    fn moved_to(now: &str) -> Observation {
        Observation {
            liveness: Liveness::Alive { dev: 1, ino: 42 },
            current_path: Some(now.to_string()),
            at_panel_path: None,
            readable: true,
        }
    }

    // --- decide -------------------------------------------------------------

    #[test]
    fn unchanged_directory_is_alive() {
        let obs = alive_at("/Users/tester/Projects");
        assert_eq!(
            decide(&obs, "/Users/tester/Projects", &home(), true),
            DirFate::Alive
        );
    }

    #[test]
    fn a_symlinked_ancestor_is_not_mistaken_for_a_move() {
        // Regression: /var is a symlink to /private/var, so F_GETPATH reports the
        // resolved path while the panel holds the path the user navigated. That
        // textual difference must not read as a rename, or every panel under
        // /tmp, /var or /etc would jump on its first probe.
        let obs = Observation {
            liveness: Liveness::Alive { dev: 1, ino: 42 },
            current_path: Some("/private/var/folders/abc/target".into()),
            at_panel_path: Some((1, 42)), // the panel's path still resolves to us
            readable: true,
        };
        assert_eq!(
            decide(&obs, "/var/folders/abc/target", &home(), true),
            DirFate::Alive
        );
    }

    #[test]
    fn renamed_directory_is_followed() {
        let obs = moved_to("/Users/tester/ProjectsNew");
        assert_eq!(
            decide(&obs, "/Users/tester/Projects", &home(), true),
            DirFate::Moved("/Users/tester/ProjectsNew".into())
        );
    }

    #[test]
    fn renamed_ancestor_is_followed_too() {
        // The panel is deep inside; only a grandparent changed name. F_GETPATH
        // resolves the whole chain, so this is indistinguishable from any other
        // move — which is exactly the point.
        let obs = moved_to("/Users/tester/WorkNew/foo/src");
        assert_eq!(
            decide(&obs, "/Users/tester/Work/foo/src", &home(), true),
            DirFate::Moved("/Users/tester/WorkNew/foo/src".into())
        );
    }

    #[test]
    fn a_move_that_leaves_something_behind_still_follows_our_directory() {
        // Our folder moved away and an unrelated one now sits at the old path.
        // We follow the identity we were tracking, not the name.
        let obs = Observation {
            at_panel_path: Some((1, 777)),
            ..moved_to("/Users/tester/ProjectsNew")
        };
        assert_eq!(
            decide(&obs, "/Users/tester/Projects", &home(), true),
            DirFate::Moved("/Users/tester/ProjectsNew".into())
        );
    }

    #[test]
    fn move_becomes_deletion_when_following_is_disabled() {
        let obs = moved_to("/Users/tester/ProjectsNew");
        assert_eq!(
            decide(&obs, "/Users/tester/Projects", &home(), false),
            DirFate::Deleted
        );
    }

    #[test]
    fn trashing_is_not_followed() {
        let obs = moved_to("/Users/tester/.Trash/Projects");
        assert_eq!(
            decide(&obs, "/Users/tester/Projects", &home(), true),
            DirFate::Trashed
        );
    }

    #[test]
    fn trashing_to_a_volume_trash_is_not_followed() {
        let obs = moved_to("/Volumes/Backup/.Trashes/501/Projects");
        assert_eq!(
            decide(&obs, "/Volumes/Backup/Projects", &home(), true),
            DirFate::Trashed
        );
    }

    #[test]
    fn deleted_directory_with_nothing_at_the_path() {
        let obs = Observation {
            liveness: Liveness::Gone,
            current_path: None,
            at_panel_path: None,
            readable: false,
        };
        assert_eq!(
            decide(&obs, "/Users/tester/Projects", &home(), true),
            DirFate::Deleted
        );
    }

    #[test]
    fn a_new_directory_at_the_same_path_is_replaced_not_alive() {
        // The regression this module exists to prevent: F_GETPATH still reports
        // the old path and stat() on it succeeds, so a path-only check would
        // call this healthy. fstat is what catches the impostor.
        let obs = Observation {
            liveness: Liveness::Gone,
            current_path: Some("/Users/tester/Projects".into()),
            at_panel_path: Some((1, 999)), // different inode
            readable: true,
        };
        assert_eq!(
            decide(&obs, "/Users/tester/Projects", &home(), true),
            DirFate::Replaced
        );
    }

    #[test]
    fn unmounted_volume_is_its_own_case() {
        let obs = Observation {
            liveness: Liveness::VolumeGone,
            current_path: None,
            at_panel_path: None,
            readable: false,
        };
        assert_eq!(
            decide(&obs, "/Volumes/Stick/photos", &home(), true),
            DirFate::Unmounted
        );
    }

    #[test]
    fn present_but_unreadable_is_not_a_deletion() {
        let obs = Observation {
            readable: false,
            ..alive_at("/Users/tester/Documents")
        };
        assert_eq!(
            decide(&obs, "/Users/tester/Documents", &home(), true),
            DirFate::Unreadable
        );
    }

    #[test]
    fn unexpected_errno_never_moves_the_panel() {
        let obs = Observation {
            liveness: Liveness::Unknown,
            current_path: None,
            at_panel_path: None,
            readable: true,
        };
        assert_eq!(
            decide(&obs, "/Users/tester/Projects", &home(), true),
            DirFate::Alive
        );
    }

    // --- recover ------------------------------------------------------------

    fn prefs() -> WatchPrefs {
        WatchPrefs::default()
    }

    #[test]
    fn a_move_follows_and_reports_where_it_came_from() {
        let r = recover(
            DirFate::Moved("/Users/tester/New".into()),
            "/Users/tester/Old",
            &prefs(),
            "/Users/tester",
            |_| None,
        );
        match r {
            DirRecovery::Follow { path, notice } => {
                assert_eq!(path, "/Users/tester/New");
                assert_eq!(notice.kind, PanelNoticeKind::Moved);
                assert_eq!(notice.path, "/Users/tester/Old");
            }
            other => panic!("expected Follow, got {other:?}"),
        }
    }

    #[test]
    fn a_deletion_lands_on_the_ancestor_with_a_cursor_hint() {
        let r = recover(
            DirFate::Deleted,
            "/Users/tester/Projects/foo",
            &prefs(),
            "/Users/tester",
            |_| Some("/Users/tester/Projects".to_string()),
        );
        match r {
            DirRecovery::Fallback {
                path,
                cursor_hint,
                notice,
            } => {
                assert_eq!(path, "/Users/tester/Projects");
                // So the cursor lands where "foo" used to sort, not at the top.
                assert_eq!(cursor_hint.as_deref(), Some("foo"));
                assert_eq!(notice.kind, PanelNoticeKind::Deleted);
            }
            other => panic!("expected Fallback, got {other:?}"),
        }
    }

    #[test]
    fn a_deletion_falls_back_to_home_when_no_ancestor_is_readable() {
        let r = recover(
            DirFate::Deleted,
            "/Volumes/Gone/deep/dir",
            &prefs(),
            "/Users/tester",
            |_| None,
        );
        match r {
            DirRecovery::Fallback { path, .. } => assert_eq!(path, "/Users/tester"),
            other => panic!("expected Fallback, got {other:?}"),
        }
    }

    #[test]
    fn on_lost_home_skips_the_ancestor_walk() {
        let p = WatchPrefs {
            on_lost: OnLost::Home,
            ..prefs()
        };
        let r = recover(
            DirFate::Deleted,
            "/Users/tester/Projects/foo",
            &p,
            "/Users/tester",
            |_| panic!("ancestor walk must not run when on_lost is Home"),
        );
        match r {
            DirRecovery::Fallback { path, .. } => assert_eq!(path, "/Users/tester"),
            other => panic!("expected Fallback, got {other:?}"),
        }
    }

    #[test]
    fn an_unreadable_directory_holds_position() {
        let r = recover(
            DirFate::Unreadable,
            "/Users/tester/Documents",
            &prefs(),
            "/Users/tester",
            |_| Some("/Users/tester".to_string()),
        );
        match r {
            DirRecovery::Hold(n) => assert_eq!(n.kind, PanelNoticeKind::Denied),
            other => panic!("expected Hold, got {other:?}"),
        }
    }

    #[test]
    fn an_ejected_volume_goes_home_not_to_slash_volumes() {
        let r = recover(
            DirFate::Unmounted,
            "/Volumes/Stick/photos",
            &prefs(),
            "/Users/tester",
            |_| Some("/Volumes".to_string()),
        );
        match r {
            DirRecovery::Fallback { path, notice, .. } => {
                assert_eq!(path, "/Users/tester");
                assert_eq!(notice.kind, PanelNoticeKind::Unmounted);
            }
            other => panic!("expected Fallback, got {other:?}"),
        }
    }

    #[test]
    fn notices_name_the_directory_they_are_about() {
        let n = PanelNoticeSpec::new(PanelNoticeKind::Trashed, "/Users/tester/Old").into_notice();
        assert!(n.message.contains("/Users/tester/Old"), "{}", n.message);
        assert!(n.message.contains("Trash"), "{}", n.message);
    }

    // --- helpers ------------------------------------------------------------

    #[test]
    fn trash_detection() {
        let h = home();
        assert!(is_in_trash(Path::new("/Users/tester/.Trash/x"), &h));
        assert!(is_in_trash(Path::new("/Volumes/D/.Trashes/501/x"), &h));
        assert!(!is_in_trash(Path::new("/Users/tester/Trash/x"), &h));
        assert!(!is_in_trash(Path::new("/Users/tester/Projects"), &h));
    }

    #[test]
    fn nearest_readable_ancestor_walks_up_past_missing_directories() {
        let root = crate::testutil::unique_dir("fm_core_watch_anc");
        let deep = root.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();

        let missing = deep.join("gone/deeper");
        let found = nearest_readable_ancestor(&missing.to_string_lossy()).unwrap();
        assert_eq!(found, deep.to_string_lossy());

        std::fs::remove_dir_all(&root).ok();
    }

    // --- the real syscalls --------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn handle_follows_a_rename_and_reports_deletion() {
        let root = crate::testutil::unique_dir("fm_core_watch_fd");
        let dir = root.join("target");
        std::fs::create_dir_all(&dir).unwrap();

        let h = DirHandle::open(&dir).unwrap();
        let panel_path = dir.to_string_lossy().to_string();

        // Untouched.
        let obs = h.observe(&panel_path);
        assert!(matches!(obs.liveness, Liveness::Alive { .. }));
        assert_eq!(
            decide(&obs, &panel_path, &home(), true),
            DirFate::Alive,
            "an untouched directory must not look like an event"
        );

        // Renamed: the descriptor should resolve to the new path. Compared
        // against the canonical form, because F_GETPATH reports the real path —
        // the temp dir lives under /var, which is a symlink to /private/var.
        let renamed = root.join("target_renamed");
        std::fs::rename(&dir, &renamed).unwrap();
        let canonical = std::fs::canonicalize(&renamed).unwrap();
        let obs = h.observe(&panel_path);
        assert_eq!(
            decide(&obs, &panel_path, &home(), true),
            DirFate::Moved(canonical.to_string_lossy().into_owned())
        );

        // Removed: fstat fails, and nothing occupies the old path.
        std::fs::remove_dir(&renamed).unwrap();
        let obs = h.observe(&panel_path);
        assert_eq!(decide(&obs, &panel_path, &home(), true), DirFate::Deleted);

        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn handle_distinguishes_a_replacement_from_the_original() {
        let root = crate::testutil::unique_dir("fm_core_watch_repl");
        let dir = root.join("target");
        std::fs::create_dir_all(&dir).unwrap();

        let h = DirHandle::open(&dir).unwrap();
        let panel_path = dir.to_string_lossy().to_string();
        let original = h.identity();

        std::fs::remove_dir(&dir).unwrap();
        std::fs::create_dir(&dir).unwrap(); // a different directory, same path

        let obs = h.observe(&panel_path);
        assert_ne!(
            obs.at_panel_path,
            Some(original),
            "the impostor must not share the original's inode"
        );
        assert_eq!(decide(&obs, &panel_path, &home(), true), DirFate::Replaced);

        std::fs::remove_dir_all(&root).ok();
    }
}
