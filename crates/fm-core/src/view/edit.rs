//! The embedded editor's documents (F4, SPEC §5.5).
//!
//! The core owns the **document** — path, encoding, line-ending style,
//! permission bits, and the file's on-disk identity at the moment it was opened.
//! The frontend owns only the editable text buffer and hands it back whole on
//! save, which is what lets the webview use a plain text widget (with its native
//! undo, selection, and IME) while every decision that can damage a file stays
//! on this side of the IPC boundary.
//!
//! Saving is atomic (temp file in the same directory, then rename) and preserves
//! what was found: the original encoding, the original line endings, and the
//! original permission bits. If the file changed underneath us since it was
//! opened, the save reports a [`SaveOutcome::Conflict`] instead of clobbering
//! it (§5.4b / §5.6).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::codec;
use crate::types::{EditDoc, Eol, FileProbe, SaveOutcome, TextEncoding};

/// Open editor documents, keyed by id.
#[derive(Default)]
pub struct Docs {
    next_id: u64,
    map: HashMap<String, Doc>,
}

struct Doc {
    path: PathBuf,
    encoding: TextEncoding,
    eol: Eol,
    read_only: bool,
    /// Permission bits to restore onto the replacement file.
    permissions: std::fs::Permissions,
    /// The file's identity when it was opened, so a change underneath us is
    /// caught before it is overwritten.
    mtime: Option<std::time::SystemTime>,
    len: u64,
}

impl Docs {
    /// Load `path` into a document. Fails when the file is larger than
    /// `max_bytes` — the editor holds the whole text in memory, unlike the
    /// viewer, which pages — so routing sends oversized files to the external
    /// editor before ever getting here.
    pub fn open(
        &mut self,
        path: &str,
        probe: FileProbe,
        max_bytes: u64,
    ) -> Result<EditDoc, String> {
        let path = PathBuf::from(path);
        if probe.size > max_bytes {
            return Err(format!(
                "{} is {} — too large for the built-in editor",
                path.display(),
                human_size(probe.size)
            ));
        }
        let meta = std::fs::metadata(&path)
            .map_err(|e| format!("could not stat {}: {e}", path.display()))?;
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let text = normalize_eol(&codec::decode(
            codec::strip_bom(&bytes, probe.encoding),
            probe.encoding,
        ));

        self.next_id += 1;
        let id = format!("e{}", self.next_id);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let read_only = meta.permissions().readonly();

        self.map.insert(
            id.clone(),
            Doc {
                path: path.clone(),
                encoding: probe.encoding,
                eol: probe.eol,
                read_only,
                permissions: meta.permissions(),
                mtime: meta.modified().ok(),
                len: meta.len(),
            },
        );

        Ok(EditDoc {
            id,
            path: path.to_string_lossy().into_owned(),
            name,
            text,
            encoding: probe.encoding,
            eol: probe.eol,
            read_only,
        })
    }

    /// Write `text` back to the document. `force` answers a previous
    /// [`SaveOutcome::Conflict`] with "overwrite anyway".
    pub fn save(&mut self, id: &str, text: &str, force: bool) -> SaveOutcome {
        let Some(doc) = self.map.get_mut(id) else {
            return SaveOutcome::Failed(format!("editor session {id} is no longer open"));
        };
        if doc.read_only {
            return SaveOutcome::ReadOnly;
        }
        if !force {
            if let Some(reason) = doc.changed_on_disk() {
                return SaveOutcome::Conflict(reason);
            }
        }
        let restored = restore_eol(text, doc.eol);
        let Some(bytes) = codec::encode(&restored, doc.encoding) else {
            return SaveOutcome::Failed(format!(
                "the text contains characters that {} cannot represent",
                encoding_label(doc.encoding)
            ));
        };
        match write_atomically(&doc.path, &bytes, doc.permissions.clone()) {
            Ok(()) => {
                // Re-baseline, so a second save doesn't report our own write as
                // someone else's change.
                if let Ok(meta) = std::fs::metadata(&doc.path) {
                    doc.mtime = meta.modified().ok();
                    doc.len = meta.len();
                }
                SaveOutcome::Saved
            }
            Err(e) => SaveOutcome::Failed(e),
        }
    }

    /// Path of an open document, so F6 (edit → view) can hand the viewer the
    /// file without the frontend ever holding authority over it.
    pub fn path_of(&self, id: &str) -> Option<String> {
        self.map
            .get(id)
            .map(|d| d.path.to_string_lossy().into_owned())
    }

    pub fn close(&mut self, id: &str) {
        self.map.remove(id);
    }
}

impl Doc {
    /// A human-readable reason when the file no longer looks the way it did at
    /// open time; `None` when it is unchanged.
    fn changed_on_disk(&self) -> Option<String> {
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            // Vanished: saving would recreate it, which the user should confirm.
            Err(_) => return Some("the file no longer exists on disk".to_string()),
        };
        if meta.len() != self.len || meta.modified().ok() != self.mtime {
            return Some("the file changed on disk after it was opened".to_string());
        }
        None
    }
}

/// Write `bytes` to `path` without ever leaving a truncated file behind: write a
/// sibling temp file, restore the permission bits, then rename over the target.
/// The same idiom the config writer uses.
fn write_atomically(
    path: &Path,
    bytes: &[u8],
    permissions: std::fs::Permissions,
) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".to_string());
    // Same directory, so the rename is atomic and never crosses a filesystem.
    let tmp = dir.join(format!(".{file_name}.fm-tmp"));

    std::fs::write(&tmp, bytes).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    // A failure to restore the mode is not worth losing the edit over.
    let _ = std::fs::set_permissions(&tmp, permissions);
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not replace {}: {e}", path.display())
    })
}

/// Normalize every line-ending style to `\n` for editing. The original style is
/// remembered on the document and put back on save, so opening a CRLF file and
/// changing one word doesn't rewrite every line.
fn normalize_eol(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn restore_eol(text: &str, eol: Eol) -> String {
    match eol {
        Eol::Lf => text.to_string(),
        Eol::Crlf => text.replace('\n', "\r\n"),
        Eol::Cr => text.replace('\n', "\r"),
    }
}

fn encoding_label(encoding: TextEncoding) -> &'static str {
    match encoding {
        TextEncoding::Utf8 | TextEncoding::Utf8Bom => "UTF-8",
        TextEncoding::Utf16Le => "UTF-16LE",
        TextEncoding::Utf16Be => "UTF-16BE",
        TextEncoding::Latin1 => "Latin-1",
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["bytes", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::probe;

    struct Temp(PathBuf);
    impl Temp {
        fn new(tag: &str, bytes: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "fm_edit_{tag}_{}_{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::write(&path, bytes).unwrap();
            Temp(path)
        }
        fn as_str(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
        fn bytes(&self) -> Vec<u8> {
            std::fs::read(&self.0).unwrap()
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            std::fs::remove_file(&self.0).ok();
        }
    }

    fn open(docs: &mut Docs, tmp: &Temp) -> EditDoc {
        let p = probe::probe(&tmp.0).unwrap();
        docs.open(&tmp.as_str(), p, 16 << 20).unwrap()
    }

    #[test]
    fn round_trips_a_plain_utf8_file() {
        let tmp = Temp::new("plain", b"one\ntwo\n");
        let mut docs = Docs::default();
        let doc = open(&mut docs, &tmp);
        assert_eq!(doc.text, "one\ntwo\n");
        assert!(!doc.read_only);

        assert!(matches!(docs.save(&doc.id, "one\nTWO\n", false), SaveOutcome::Saved));
        assert_eq!(tmp.bytes(), b"one\nTWO\n");
    }

    #[test]
    fn crlf_files_are_edited_as_lf_and_written_back_as_crlf() {
        let tmp = Temp::new("crlf", b"alpha\r\nbeta\r\n");
        let mut docs = Docs::default();
        let doc = open(&mut docs, &tmp);
        assert_eq!(doc.eol, Eol::Crlf);
        assert_eq!(doc.text, "alpha\nbeta\n");

        docs.save(&doc.id, "alpha\nbeta\ngamma\n", false);
        assert_eq!(tmp.bytes(), b"alpha\r\nbeta\r\ngamma\r\n");
    }

    #[test]
    fn a_bom_survives_a_save() {
        let mut bytes = codec::UTF8_BOM.to_vec();
        bytes.extend_from_slice("héllo\n".as_bytes());
        let tmp = Temp::new("bom", &bytes);
        let mut docs = Docs::default();
        let doc = open(&mut docs, &tmp);
        assert_eq!(doc.encoding, TextEncoding::Utf8Bom);
        assert_eq!(doc.text, "héllo\n");

        docs.save(&doc.id, "héllo world\n", false);
        let written = tmp.bytes();
        assert!(written.starts_with(codec::UTF8_BOM));
        assert_eq!(&written[3..], "héllo world\n".as_bytes());
    }

    #[test]
    fn latin1_stays_latin1_and_refuses_text_it_cannot_hold() {
        let tmp = Temp::new("latin1", b"caf\xE9\n");
        let mut docs = Docs::default();
        let doc = open(&mut docs, &tmp);
        assert_eq!(doc.encoding, TextEncoding::Latin1);
        assert_eq!(doc.text, "café\n");

        docs.save(&doc.id, "café crème\n", false);
        assert_eq!(tmp.bytes(), b"caf\xE9 cr\xE8me\n");

        // An em dash has no Latin-1 form: refuse rather than mangle the file.
        let outcome = docs.save(&doc.id, "café — crème\n", false);
        assert!(matches!(outcome, SaveOutcome::Failed(ref m) if m.contains("Latin-1")));
        assert_eq!(tmp.bytes(), b"caf\xE9 cr\xE8me\n"); // untouched
    }

    #[test]
    fn a_file_changed_underneath_us_conflicts_until_forced() {
        let tmp = Temp::new("conflict", b"original\n");
        let mut docs = Docs::default();
        let doc = open(&mut docs, &tmp);

        std::fs::write(&tmp.0, b"someone else's edit\n").unwrap();
        let outcome = docs.save(&doc.id, "mine\n", false);
        assert!(matches!(outcome, SaveOutcome::Conflict(_)));
        assert_eq!(tmp.bytes(), b"someone else's edit\n"); // not clobbered

        assert!(matches!(docs.save(&doc.id, "mine\n", true), SaveOutcome::Saved));
        assert_eq!(tmp.bytes(), b"mine\n");
        // Our own write must not look like someone else's on the next save.
        assert!(matches!(docs.save(&doc.id, "mine again\n", false), SaveOutcome::Saved));
    }

    #[test]
    fn a_read_only_file_opens_but_will_not_save() {
        let tmp = Temp::new("ro", b"look but do not touch\n");
        let mut perms = std::fs::metadata(&tmp.0).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&tmp.0, perms).unwrap();

        let mut docs = Docs::default();
        let doc = open(&mut docs, &tmp);
        assert!(doc.read_only);
        assert!(matches!(docs.save(&doc.id, "changed\n", false), SaveOutcome::ReadOnly));
        assert_eq!(tmp.bytes(), b"look but do not touch\n");

        // Restore write permission so Temp's drop can remove the file. Set the
        // mode explicitly rather than via set_readonly(false), which on unix
        // means world-writable.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp.0, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn saving_preserves_the_permission_bits() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = Temp::new("perms", b"#!/bin/sh\necho hi\n");
        std::fs::set_permissions(&tmp.0, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut docs = Docs::default();
        let doc = open(&mut docs, &tmp);
        docs.save(&doc.id, "#!/bin/sh\necho bye\n", false);

        let mode = std::fs::metadata(&tmp.0).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "an edited script must stay executable");
    }

    #[test]
    fn oversized_files_are_refused_rather_than_loaded() {
        let tmp = Temp::new("big", &vec![b'x'; 4096]);
        let mut docs = Docs::default();
        let p = probe::probe(&tmp.0).unwrap();
        let err = docs.open(&tmp.as_str(), p, 1024).unwrap_err();
        assert!(err.contains("too large"));
    }

    #[test]
    fn a_closed_document_fails_cleanly() {
        let tmp = Temp::new("closed", b"x\n");
        let mut docs = Docs::default();
        let doc = open(&mut docs, &tmp);
        assert!(docs.path_of(&doc.id).is_some());
        docs.close(&doc.id);
        assert!(docs.path_of(&doc.id).is_none());
        assert!(matches!(docs.save(&doc.id, "x\n", false), SaveOutcome::Failed(_)));
    }
}
