//! Decoding and encoding for the handful of encodings the viewer/editor detect.
//!
//! Deliberately hand-rolled rather than pulled from a dependency: the set is
//! small (UTF-8 ±BOM, UTF-16 both ways, Latin-1), every member is a few lines,
//! and `fm-core` stays dependency-light — which matters for a crate whose one
//! structural rule is that it must not accumulate platform baggage.
//!
//! Decoding is **lossy by design**: the viewer must render *something* for every
//! file rather than fail. The editor only ever saves text it decoded losslessly
//! (see [`super::edit`]), so a lossy render can never be written back over a
//! user's file.

use crate::types::TextEncoding;

/// UTF-8 byte-order mark, written back on save for [`TextEncoding::Utf8Bom`].
pub const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Decode `bytes` as `encoding`. `bytes` may be a window cut out of a larger
/// file, so a trailing partial character is dropped rather than rendered as a
/// replacement glyph — the next window starts with it.
pub fn decode(bytes: &[u8], encoding: TextEncoding) -> String {
    match encoding {
        TextEncoding::Utf8 | TextEncoding::Utf8Bom => {
            let end = valid_utf8_end(bytes);
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        }
        TextEncoding::Latin1 => bytes.iter().map(|b| *b as char).collect(),
        TextEncoding::Utf16Le => decode_utf16(bytes, u16::from_le_bytes),
        TextEncoding::Utf16Be => decode_utf16(bytes, u16::from_be_bytes),
    }
}

/// Encode `text` as `encoding`, including the BOM where the encoding carries
/// one. Returns `None` when the text contains characters the encoding cannot
/// represent — the editor refuses the save rather than silently mangling it.
pub fn encode(text: &str, encoding: TextEncoding) -> Option<Vec<u8>> {
    match encoding {
        TextEncoding::Utf8 => Some(text.as_bytes().to_vec()),
        TextEncoding::Utf8Bom => {
            let mut out = UTF8_BOM.to_vec();
            out.extend_from_slice(text.as_bytes());
            Some(out)
        }
        TextEncoding::Latin1 => text
            .chars()
            .map(|c| (c as u32 <= 0xFF).then_some(c as u8))
            .collect(),
        TextEncoding::Utf16Le => Some(encode_utf16(text, u16::to_le_bytes, [0xFF, 0xFE])),
        TextEncoding::Utf16Be => Some(encode_utf16(text, u16::to_be_bytes, [0xFE, 0xFF])),
    }
}

/// Strip the encoding's byte-order mark from the head of a whole-file buffer.
pub fn strip_bom(bytes: &[u8], encoding: TextEncoding) -> &[u8] {
    let bom = match encoding {
        TextEncoding::Utf8Bom => 3,
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => 2,
        _ => 0,
    };
    if bytes.len() >= bom {
        &bytes[bom..]
    } else {
        bytes
    }
}

/// Bytes per character unit — 2 for UTF-16, 1 otherwise. Used to keep window
/// reads aligned so a UTF-16 file never decodes half-shifted.
pub fn unit(encoding: TextEncoding) -> u64 {
    match encoding {
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => 2,
        _ => 1,
    }
}

/// Where the last complete UTF-8 sequence ends. A window boundary that lands
/// mid-character would otherwise render as `U+FFFD`.
fn valid_utf8_end(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        // Genuinely malformed: keep everything and let the lossy decode mark it.
        Err(_) => bytes.len(),
    }
}

fn decode_utf16(bytes: &[u8], to_u16: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| to_u16([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

fn encode_utf16(text: &str, from_u16: fn(u16) -> [u8; 2], bom: [u8; 2]) -> Vec<u8> {
    let mut out = bom.to_vec();
    for unit in text.encode_utf16() {
        out.extend_from_slice(&from_u16(unit));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_round_trips_and_keeps_its_bom() {
        let text = "héllo — wörld\n";
        assert_eq!(decode(text.as_bytes(), TextEncoding::Utf8), text);

        let bytes = encode(text, TextEncoding::Utf8Bom).unwrap();
        assert!(bytes.starts_with(UTF8_BOM));
        assert_eq!(
            decode(strip_bom(&bytes, TextEncoding::Utf8Bom), TextEncoding::Utf8Bom),
            text
        );
    }

    #[test]
    fn a_window_cut_mid_character_drops_the_partial_tail() {
        let full = "aé".as_bytes(); // 'é' is two bytes
        let cut = &full[..full.len() - 1];
        assert_eq!(decode(cut, TextEncoding::Utf8), "a");
    }

    #[test]
    fn latin1_maps_bytes_to_chars_and_refuses_what_it_cannot_hold() {
        assert_eq!(decode(b"caf\xE9", TextEncoding::Latin1), "café");
        assert_eq!(encode("café", TextEncoding::Latin1).unwrap(), b"caf\xE9");
        // Em dash has no Latin-1 representation, so the save is refused.
        assert_eq!(encode("a — b", TextEncoding::Latin1), None);
    }

    #[test]
    fn utf16_round_trips_in_both_byte_orders() {
        for enc in [TextEncoding::Utf16Le, TextEncoding::Utf16Be] {
            let bytes = encode("hi ω\n", enc).unwrap();
            assert_eq!(decode(strip_bom(&bytes, enc), enc), "hi ω\n");
            assert_eq!(unit(enc), 2);
        }
    }

    #[test]
    fn an_odd_trailing_byte_is_ignored_rather_than_shifting_the_decode() {
        let bytes = b"h\0i\0!"; // 5 bytes: two whole UTF-16LE units plus a stray
        assert_eq!(decode(bytes, TextEncoding::Utf16Le), "hi");
    }
}
