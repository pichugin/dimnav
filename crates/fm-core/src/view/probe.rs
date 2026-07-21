//! File-type sniffing for the embedded viewer/editor (§5.5).
//!
//! DOS Navigator's "smart viewer" decides what a file *is* before deciding how
//! to show it; this module is that decision. It reads only the first
//! [`SAMPLE`] bytes — opening a multi-gigabyte log must stay instant — and
//! classifies the file as text (with an encoding and a line-ending style),
//! binary, or an image.
//!
//! The heuristics are the conventional ones: BOM first, then image magic
//! numbers, then git's "a NUL byte means binary" rule, then UTF-8 validity, and
//! finally a printable-ratio test that catches legacy single-byte text.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::types::{Eol, FileProbe, MediaKind, TextEncoding};

/// How much of the file's head to sniff. Big enough to be representative, small
/// enough to be a single cheap read.
const SAMPLE: usize = 8 * 1024;

/// Fraction of the sample that must be printable for a BOM-less, NUL-free,
/// non-UTF-8 file to still count as (legacy single-byte) text.
const PRINTABLE_RATIO: f32 = 0.95;

/// Sniff `path` and decide how the viewer/editor should treat it.
///
/// An empty file is text (an empty buffer is a perfectly good thing to edit).
/// Errors are only I/O errors — an unreadable file surfaces as a structured
/// failure rather than a wrong classification (§5.6).
pub fn probe(path: &Path) -> Result<FileProbe, String> {
    let mut file = File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    let size = file
        .metadata()
        .map_err(|e| format!("could not stat {}: {e}", path.display()))?
        .len();

    let mut sample = vec![0u8; SAMPLE.min(size.max(1) as usize)];
    let read = file
        .read(&mut sample)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    sample.truncate(read);

    Ok(classify(&sample, size, extension(path).as_deref()))
}

/// The pure half of [`probe`]: classify a head sample. Split out so the
/// heuristics are unit-testable without touching the filesystem.
pub fn classify(sample: &[u8], size: u64, ext: Option<&str>) -> FileProbe {
    let media_and_encoding = detect(sample, ext);
    let (media, encoding) = media_and_encoding;
    FileProbe {
        size,
        media,
        encoding,
        eol: detect_eol(sample, encoding),
    }
}

fn detect(sample: &[u8], ext: Option<&str>) -> (MediaKind, TextEncoding) {
    // 1. A BOM is definitive — it names the encoding outright.
    if let Some(enc) = bom_encoding(sample) {
        return (MediaKind::Text, enc);
    }
    // 2. Images: magic numbers first, extension only as a backstop for formats
    //    whose containers we don't sniff (HEIC, TIFF, ICO) and for SVG, which is
    //    text that we still want to render as a picture.
    if is_image_magic(sample) || image_extension(ext) {
        return (MediaKind::Image, TextEncoding::Utf8);
    }
    // An empty file is an editable empty text buffer, not a binary blob.
    if sample.is_empty() {
        return (MediaKind::Text, TextEncoding::Utf8);
    }
    // 3. git's heuristic: a NUL in the head means binary.
    if sample.contains(&0) {
        return (MediaKind::Binary, TextEncoding::Utf8);
    }
    // 4. Valid UTF-8, allowing for a multi-byte sequence chopped by the sample
    //    boundary.
    if is_utf8_ignoring_truncation(sample) {
        return (MediaKind::Text, TextEncoding::Utf8);
    }
    // 5. Mostly-printable bytes — legacy single-byte text (CP-1252 / Latin-1).
    let printable = sample.iter().filter(|b| is_printable(**b)).count();
    if printable as f32 >= sample.len() as f32 * PRINTABLE_RATIO {
        (MediaKind::Text, TextEncoding::Latin1)
    } else {
        (MediaKind::Binary, TextEncoding::Utf8)
    }
}

/// The encoding named by a byte-order mark, if the sample starts with one.
/// UTF-16 is checked before UTF-8 because neither prefix contains the other, and
/// UTF-8-with-BOM is kept distinct from plain UTF-8 so the editor can write the
/// mark back.
fn bom_encoding(sample: &[u8]) -> Option<TextEncoding> {
    match sample {
        [0xEF, 0xBB, 0xBF, ..] => Some(TextEncoding::Utf8Bom),
        [0xFF, 0xFE, ..] => Some(TextEncoding::Utf16Le),
        [0xFE, 0xFF, ..] => Some(TextEncoding::Utf16Be),
        _ => None,
    }
}

fn is_image_magic(sample: &[u8]) -> bool {
    sample.starts_with(b"\x89PNG\r\n\x1a\n")
        || sample.starts_with(b"\xFF\xD8\xFF")            // JPEG
        || sample.starts_with(b"GIF87a")
        || sample.starts_with(b"GIF89a")
        || sample.starts_with(b"BM")                       // BMP
        || (sample.starts_with(b"RIFF") && sample.len() >= 12 && &sample[8..12] == b"WEBP")
        || sample.starts_with(b"II*\0")                    // TIFF, little-endian
        || sample.starts_with(b"MM\0*")                    // TIFF, big-endian
}

fn image_extension(ext: Option<&str>) -> bool {
    matches!(
        ext,
        Some("heic" | "heif" | "ico" | "svg" | "avif" | "tif" | "tiff")
    )
}

/// Whether `sample` is valid UTF-8, tolerating a trailing multi-byte sequence
/// that the 8 KiB cut ran through. Without this, a large UTF-8 file would be
/// misread as Latin-1 roughly one time in four.
fn is_utf8_ignoring_truncation(sample: &[u8]) -> bool {
    match std::str::from_utf8(sample) {
        Ok(_) => true,
        Err(e) => {
            // `error_len() == None` means "unexpected end of input" — i.e. the
            // sequence was cut off, not malformed — and everything before the
            // cut was valid.
            e.error_len().is_none() && e.valid_up_to() + 4 > sample.len()
        }
    }
}

/// Printable for the ratio test: graphic characters, common whitespace, and the
/// high range where accented Latin-1 letters live.
fn is_printable(b: u8) -> bool {
    matches!(b, 0x09 | 0x0A | 0x0D | 0x20..=0x7E) || b >= 0xA0
}

/// The file's line-ending style, from the first break in the sample. UTF-16 is
/// checked on every other byte so an ASCII-range `\n` is still found.
fn detect_eol(sample: &[u8], encoding: TextEncoding) -> Eol {
    let step = match encoding {
        TextEncoding::Utf16Le => 2,
        TextEncoding::Utf16Be => 2,
        _ => 1,
    };
    let start = match encoding {
        TextEncoding::Utf16Be => 1, // low byte of each UTF-16BE unit
        _ => 0,
    };
    let mut i = start;
    while i < sample.len() {
        match sample[i] {
            b'\n' => return Eol::Lf,
            b'\r' => {
                return if sample.get(i + step) == Some(&b'\n') {
                    Eol::Crlf
                } else {
                    // A lone CR is classic-Mac EOL; only conclude that at a real
                    // break, not at a truncated sample boundary.
                    Eol::Cr
                }
            }
            _ => {}
        }
        i += step;
    }
    Eol::Lf
}

/// Lower-cased extension (no dot), or `None`.
pub fn extension(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(sample: &[u8]) -> (MediaKind, TextEncoding) {
        let p = classify(sample, sample.len() as u64, None);
        (p.media, p.encoding)
    }

    #[test]
    fn plain_ascii_and_utf8_are_text() {
        assert_eq!(kind(b"hello, world\n"), (MediaKind::Text, TextEncoding::Utf8));
        assert_eq!(
            kind("привет — ok\n".as_bytes()),
            (MediaKind::Text, TextEncoding::Utf8)
        );
    }

    #[test]
    fn empty_file_is_an_editable_text_buffer() {
        assert_eq!(kind(b""), (MediaKind::Text, TextEncoding::Utf8));
    }

    #[test]
    fn boms_name_the_encoding() {
        assert_eq!(kind(b"\xEF\xBB\xBFhi"), (MediaKind::Text, TextEncoding::Utf8Bom));
        assert_eq!(kind(b"\xFF\xFEh\0i\0"), (MediaKind::Text, TextEncoding::Utf16Le));
        assert_eq!(kind(b"\xFE\xFF\0h\0i"), (MediaKind::Text, TextEncoding::Utf16Be));
    }

    #[test]
    fn nul_byte_means_binary() {
        assert_eq!(kind(b"MZ\x90\x00\x03\x00rest"), (MediaKind::Binary, TextEncoding::Utf8));
    }

    #[test]
    fn latin1_text_without_nuls_is_still_text() {
        // 0xE9 is `é` in Latin-1 and invalid UTF-8 on its own.
        assert_eq!(kind(b"caf\xE9 au lait\n"), (MediaKind::Text, TextEncoding::Latin1));
    }

    #[test]
    fn high_entropy_without_nuls_is_binary() {
        // Bytes in 0x80..0x9F are neither valid UTF-8 nor printable Latin-1.
        let noise: Vec<u8> = (0..200u16).map(|i| 0x80 + (i % 0x20) as u8).collect();
        assert_eq!(kind(&noise).0, MediaKind::Binary);
    }

    #[test]
    fn images_come_from_magic_or_extension() {
        assert_eq!(kind(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").0, MediaKind::Image);
        assert_eq!(kind(b"\xFF\xD8\xFF\xE0\x00\x10JFIF").0, MediaKind::Image);
        assert_eq!(kind(b"GIF89a...").0, MediaKind::Image);
        assert_eq!(kind(b"RIFF\x24\0\0\0WEBPVP8 ").0, MediaKind::Image);
        // SVG is text on disk but a picture to the user.
        let svg = classify(b"<svg xmlns=\"...\"/>", 18, Some("svg"));
        assert_eq!(svg.media, MediaKind::Image);
    }

    #[test]
    fn truncated_utf8_at_the_sample_edge_is_still_utf8() {
        // "é" is 0xC3 0xA9; cut it in half at the end of the sample.
        let mut s = b"a fine file ".to_vec();
        s.push(0xC3);
        assert_eq!(kind(&s), (MediaKind::Text, TextEncoding::Utf8));
        // A stray 0xC3 in the *middle* is not a truncation, so it falls through
        // to the printable-ratio test and reads as legacy text.
        let mut s = vec![0xC3];
        s.extend_from_slice(b" a fine file");
        assert_eq!(kind(&s), (MediaKind::Text, TextEncoding::Latin1));
    }

    #[test]
    fn eol_style_is_detected_and_defaults_to_lf() {
        assert_eq!(classify(b"a\r\nb\r\n", 6, None).eol, Eol::Crlf);
        assert_eq!(classify(b"a\nb\n", 4, None).eol, Eol::Lf);
        assert_eq!(classify(b"a\rb\r", 4, None).eol, Eol::Cr);
        assert_eq!(classify(b"no breaks here", 14, None).eol, Eol::Lf);
        // UTF-16LE: the `\n` unit is 0x0A 0x00, found by stepping two bytes.
        assert_eq!(classify(b"\xFF\xFEa\0\r\0\n\0", 8, None).eol, Eol::Crlf);
    }

    #[test]
    fn probe_reads_a_real_file() {
        let path = std::env::temp_dir().join(format!("fm_probe_{}", std::process::id()));
        std::fs::write(&path, b"line one\nline two\n").unwrap();
        let p = probe(&path).unwrap();
        assert_eq!(p.media, MediaKind::Text);
        assert_eq!(p.size, 18);
        assert_eq!(p.eol, Eol::Lf);
        std::fs::remove_file(&path).ok();

        assert!(probe(Path::new("/definitely/not/here")).is_err());
    }
}
