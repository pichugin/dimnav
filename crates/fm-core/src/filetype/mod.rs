//! What a listing row *is* — the single source of truth for entry classification.
//!
//! Two callers, one rule: the panel colours a row by [`classify`], and
//! [`crate::open`] decides execute-vs-launch by [`is_runnable`]. Keeping both on
//! one table is the point of this module — they used to disagree, and the
//! disagreement was the bug described below.
//!
//! ## Why a known extension outranks the exec bit
//!
//! `mode & 0o111` is a poor signal on macOS. Files copied off an SMB/exFAT
//! share, unpacked by an archiver that does not restore modes, or lifted out of
//! a DMG all arrive `0755`/`0750` whatever they contain — so a directory of PDFs
//! reads as a directory of programs. The old rule checked that bit first and
//! painted every such folder a uniform green, and, far worse, made Enter try to
//! *execute* a spreadsheet.
//!
//! So a name that positively identifies a document, dataset, archive, image or
//! media file wins over the bit — and so does source nobody executes, which is
//! why a `+x` `.html` stays a web page rather than turning green.
//!
//! The bit still decides everything it is actually good for: files whose name
//! claims nothing (`build`, `myapp.x86_64`) and interpreted scripts, where
//! `Exec` deliberately outranks `Code` — a `0755` `deploy.sh` is green in Norton
//! Commander, FAR and `ls`, and it is green here.

use crate::types::{Entry, EntryCategory, EntryKind, EntryMarker};

/// Extensions whose name settles the matter, exec bit or not.
///
/// Ordered most-specific first only for readability; the sets are disjoint, so
/// lookup order among them does not matter.
const DOC_EXTS: &[&str] = &[
    "md", "txt", "rtf", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp",
    "pages", "numbers", "key", "epub",
];

const DATA_EXTS: &[&str] = &[
    "xml", "json", "yaml", "yml", "toml", "csv", "tsv", "ini", "cfg", "conf", "plist", "sqlite",
    "db", "log",
];

const ARCHIVE_EXTS: &[&str] = &[
    "zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "jar", "war", "dmg", "pkg", "iso",
];

const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "svg", "webp", "heic", "tiff", "ico", "icns",
];

const MEDIA_EXTS: &[&str] = &[
    "mp4", "mov", "mkv", "avi", "webm", "mp3", "wav", "flac", "aac", "ogg", "m4a", "m4v",
];

/// Source that is compiled, rendered or queried — never handed to an interpreter
/// and run. The exec bit tells you nothing about these, so it does not get to
/// recolour them: a `+x` `.html` is a web page, not a program.
const SOURCE_EXTS: &[&str] = &[
    "c", "h", "cc", "cxx", "cpp", "hpp", "hh", "m", "mm", "rs", "ts", "tsx", "jsx", "java", "go",
    "swift", "kt", "cs", "scala", "dart", "ex", "exs", "zig", "html", "htm", "css", "scss", "sass",
    "less", "sql", "vue", "svelte",
];

/// Interpreted scripts — the one kind of named file the exec bit really does
/// describe. With the bit they are `Exec` (green, and Enter runs them); without
/// it they are ordinary source. Both colour as [`EntryCategory::Code`] otherwise.
const SCRIPT_EXTS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "ksh", "command", "py", "rb", "pl", "php", "lua", "r", "tcl",
    "awk", "ps1", "bat", "cmd", "js", "mjs", "cjs",
];

/// The categories a name claims outright — the ones that beat the exec bit.
fn by_extension(ext: &str) -> Option<EntryCategory> {
    if DOC_EXTS.contains(&ext) {
        Some(EntryCategory::Doc)
    } else if DATA_EXTS.contains(&ext) {
        Some(EntryCategory::Data)
    } else if ARCHIVE_EXTS.contains(&ext) {
        Some(EntryCategory::Archive)
    } else if IMAGE_EXTS.contains(&ext) {
        Some(EntryCategory::Image)
    } else if MEDIA_EXTS.contains(&ext) {
        Some(EntryCategory::Media)
    } else {
        None
    }
}

/// Lower-cased extension (no dot), or `None` when the name has none. A leading
/// dot does not start an extension — `.bashrc` is a hidden file, not a `bashrc`
/// file. The canonical implementation: `fs::ext_of` (sorting) and
/// `open::resolve_handler` (associations) both defer to it, so the three cannot
/// drift apart.
pub fn extension(name: &str) -> Option<String> {
    let dot = name.rfind('.')?;
    if dot == 0 {
        return None;
    }
    Some(name[dot + 1..].to_lowercase())
}

/// Which colour class `entry` falls into (§4). First match wins.
pub fn classify(entry: &Entry) -> EntryCategory {
    // Unreadable and broken rows carry their own styling, which must not be
    // overpainted by a type colour.
    if matches!(entry.marker, EntryMarker::Denied | EntryMarker::Broken) {
        return EntryCategory::Plain;
    }
    // Hidden outranks folder, so a dotfolder reads as hidden.
    if entry.name != ".." && entry.name.starts_with('.') {
        return EntryCategory::Hidden;
    }
    match entry.kind {
        EntryKind::Dir => return EntryCategory::Dir,
        EntryKind::Symlink => return EntryCategory::Symlink,
        _ => {}
    }
    let ext = extension(&entry.name);
    // A name that identifies a document beats the exec bit (see module docs).
    if let Some(cat) = ext.as_deref().and_then(by_extension) {
        return cat;
    }
    // So does a name that identifies source nobody executes.
    if ext.as_deref().is_some_and(|e| SOURCE_EXTS.contains(&e)) {
        return EntryCategory::Code;
    }
    // What is left is what the bit is actually good for: scripts, and names that
    // claim nothing at all (`build`, `myapp.x86_64`).
    if entry.is_executable {
        return EntryCategory::Exec;
    }
    if ext.as_deref().is_some_and(|e| SCRIPT_EXTS.contains(&e)) {
        return EntryCategory::Code;
    }
    EntryCategory::Plain
}

/// May Enter run this file, as opposed to handing it to the OS?
///
/// The exec bit, minus the files whose name says they are data. This is the
/// check `open::route` wants; `Entry::is_executable` is the raw bit and answers
/// a different question.
pub fn is_runnable(entry: &Entry) -> bool {
    // `Exec` *is* the answer: it is the one category [`classify`] reaches only by
    // letting the exec bit win, i.e. after nothing about the name objected. One
    // rule, so the colour and the Enter behaviour can never disagree.
    entry.is_executable && classify(entry) == EntryCategory::Exec
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: EntryKind, is_executable: bool) -> Entry {
        Entry {
            name: name.to_string(),
            kind,
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
            is_executable,
            category: EntryCategory::Plain,
            marker: EntryMarker::Ok,
            computed_size: None,
        }
    }

    fn file(name: &str, is_executable: bool) -> Entry {
        entry(name, EntryKind::File, is_executable)
    }

    /// The regression test for the reported bug: a folder full of `0750` PDFs
    /// must not read as a folder full of programs.
    #[test]
    fn exec_bit_does_not_beat_a_document_extension() {
        assert_eq!(classify(&file("report.pdf", true)), EntryCategory::Doc);
        assert_eq!(classify(&file("Q3 Results.xlsx", true)), EntryCategory::Doc);
        assert_eq!(classify(&file("swagger.json", true)), EntryCategory::Data);
        assert_eq!(classify(&file("shipment.zip", true)), EntryCategory::Archive);
        assert_eq!(classify(&file("logo.PNG", true)), EntryCategory::Image);
        assert_eq!(classify(&file("clip.mp4", true)), EntryCategory::Media);
    }

    /// The bit describes a script, so it is allowed to win there — and only there.
    #[test]
    fn exec_bit_beats_a_script_extension_but_not_other_source() {
        assert_eq!(classify(&file("deploy.sh", true)), EntryCategory::Exec);
        assert_eq!(classify(&file("deploy.sh", false)), EntryCategory::Code);
        assert_eq!(classify(&file("cli.js", true)), EntryCategory::Exec);

        // You never execute these, so a stray bit must not recolour them.
        assert_eq!(classify(&file("bank_host.html", true)), EntryCategory::Code);
        assert_eq!(classify(&file("main.rs", true)), EntryCategory::Code);
        assert_eq!(classify(&file("styles.css", true)), EntryCategory::Code);
        assert_eq!(classify(&file("main.rs", false)), EntryCategory::Code);
    }

    #[test]
    fn extensionless_executable_is_exec() {
        assert_eq!(classify(&file("build", true)), EntryCategory::Exec);
        assert_eq!(classify(&file("myapp.x86_64", true)), EntryCategory::Exec);
        assert_eq!(classify(&file("README", false)), EntryCategory::Plain);
    }

    #[test]
    fn hidden_beats_everything() {
        assert_eq!(classify(&file(".gitignore", false)), EntryCategory::Hidden);
        // Even when the name looks like a document and the bit is set.
        assert_eq!(classify(&file(".notes.pdf", true)), EntryCategory::Hidden);
        let dotdir = entry(".config", EntryKind::Dir, false);
        assert_eq!(classify(&dotdir), EntryCategory::Hidden);
    }

    #[test]
    fn dotdot_is_dir_and_symlinks_keep_their_own_colour() {
        assert_eq!(
            classify(&entry("..", EntryKind::Dir, false)),
            EntryCategory::Dir
        );
        // The link's own colour wins over what its name suggests.
        assert_eq!(
            classify(&entry("latest.pdf", EntryKind::Symlink, false)),
            EntryCategory::Symlink
        );
    }

    #[test]
    fn denied_and_broken_are_plain() {
        let mut denied = file("secret.pdf", false);
        denied.marker = EntryMarker::Denied;
        assert_eq!(classify(&denied), EntryCategory::Plain);

        let mut broken = entry("dangling", EntryKind::Symlink, false);
        broken.marker = EntryMarker::Broken;
        assert_eq!(classify(&broken), EntryCategory::Plain);
    }

    #[test]
    fn is_runnable_ignores_the_bit_on_documents() {
        assert!(!is_runnable(&file("report.pdf", true)));
        assert!(!is_runnable(&file("swagger.json", true)));
        assert!(!is_runnable(&file("bank_host.html", true)));
        assert!(!is_runnable(&file("main.rs", true)));
        assert!(is_runnable(&file("deploy.sh", true)));
        assert!(is_runnable(&file("build", true)));
        // No bit, nothing runs.
        assert!(!is_runnable(&file("deploy.sh", false)));
        assert!(!is_runnable(&file("build", false)));
    }

    #[test]
    fn extension_ignores_a_leading_dot_and_lowercases() {
        assert_eq!(extension("a.PDF").as_deref(), Some("pdf"));
        assert_eq!(extension("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension(".bashrc"), None);
        assert_eq!(extension("README"), None);
        assert_eq!(extension("trailing."), Some(String::new()));
    }
}
