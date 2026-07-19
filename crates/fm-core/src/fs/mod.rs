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

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::types::{DirListing, Entry, EntryKind, EntryMarker, SortMode};

/// List a directory into a structured result. `show_hidden` controls whether
/// dotfiles are included (§5.8). The first entry is always `..` unless `path` is
/// a filesystem root (§5.2). Unreadable children become marker entries rather
/// than aborting the listing (§5.6).
///
/// Entries are returned sorted per [`SortMode::NameFoldersFirst`] (the default,
/// §5.8) with `..` pinned to the top.
pub fn list_dir(path: &str, show_hidden: bool) -> DirListing {
    let p = Path::new(path);
    let mut entries: Vec<Entry> = Vec::new();

    // `..` first for every non-root directory (§5.2).
    if p.parent().is_some() {
        entries.push(dotdot_entry());
    }

    let mut children: Vec<Entry> = Vec::new();
    if let Ok(read) = fs::read_dir(p) {
        for dirent in read.flatten() {
            let name = dirent.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            children.push(entry_from_path(&dirent.path(), name));
        }
    }

    sort_entries(&mut children, SortMode::NameFoldersFirst);
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
        permissions: 0,
        symlink_target: None,
        is_executable: false,
        marker: EntryMarker::Ok,
    }
}

/// Build an [`Entry`] from a path, never failing — read errors become markers.
fn entry_from_path(path: &Path, name: String) -> Entry {
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

    Entry {
        name,
        kind,
        size: meta.len(),
        modified: mtime_secs(meta),
        permissions: mode_bits(meta),
        symlink_target,
        is_executable: is_executable(meta),
        marker,
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
        permissions: 0,
        symlink_target: None,
        is_executable: false,
        marker: EntryMarker::Denied,
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

/// Sort a listing in place. `..` (and any parent entry) stays pinned first;
/// everything else orders per `mode`. The default `NameFoldersFirst` groups
/// directories before files, then case-insensitive by name (§5.8).
pub fn sort_entries(entries: &mut [Entry], mode: SortMode) {
    entries.sort_by(|a, b| {
        // Pin `..` to the very top regardless of mode.
        let a_dotdot = a.name == "..";
        let b_dotdot = b.name == "..";
        if a_dotdot != b_dotdot {
            return b_dotdot.cmp(&a_dotdot); // dotdot (true) sorts first
        }

        match mode {
            SortMode::NameFoldersFirst => folders_first(a, b).then(by_name(a, b)),
            SortMode::TypeName => folders_first(a, b).then(by_name(a, b)),
            SortMode::Size => a.size.cmp(&b.size).then(by_name(a, b)),
            SortMode::Date => a.modified.cmp(&b.modified).then(by_name(a, b)),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Create a unique temp directory with a known layout, returning its path.
    fn make_fixture() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fm_core_fs_test_{nanos}"));
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
        let shown = list_dir(dir.to_str().unwrap(), true);
        let names: Vec<&str> = shown.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "alpha", "Beta", ".hidden", "gamma.txt"]);
        assert_eq!(shown.entries[0].kind, EntryKind::Dir); // ..
        assert_eq!(shown.entries[1].kind, EntryKind::Dir); // alpha

        // Hidden filtered out.
        let hidden = list_dir(dir.to_str().unwrap(), false);
        let names: Vec<&str> = hidden.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "alpha", "Beta", "gamma.txt"]);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unreadable_dir_yields_only_dotdot_not_a_panic() {
        // A path that does not exist: read_dir fails, but we still get a listing
        // with `..` and no crash (§5.6).
        let listing = list_dir("/definitely/not/a/real/path/xyzzy", true);
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, [".."]);
    }
}
