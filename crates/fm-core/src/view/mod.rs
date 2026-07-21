//! The embedded viewer (F3) and editor (F4) — SPEC §5.5, brought forward from
//! the Phase-3 roadmap because an internal viewer/editor is what makes a
//! Commander feel like a Commander.
//!
//! Modelled on **FAR Manager**, with DOS Navigator's "smart viewer" type
//! detection ([`probe`]): F3 opens *our* viewer whatever the file is — text, hex,
//! or image — instead of leaving the app, and a config association can still
//! send any file type to the system or a named application (§7).
//!
//! ## Why a session rather than a buffer
//!
//! The viewer holds an open file handle and serves only the visible window, so a
//! multi-gigabyte log opens instantly and `End` is immediate. Position is a
//! **byte offset** throughout, and the status percentage is byte-based, so
//! nothing here ever needs a huge file's total line count. Line *numbers* are a
//! best-effort convenience: the index grows forward only as far as the user has
//! actually scrolled, and after a jump past the indexed region the gutter simply
//! goes blank rather than paying for a full scan (FAR behaves the same way).
//!
//! All formatting — wrapping, tab expansion, hex layout — happens here, so the
//! frontend renders one shape ([`ViewPage`]) for every mode and stays the thin
//! layer the architecture requires.

pub mod codec;
pub mod edit;
pub mod probe;

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use crate::types::{
    FileProbe, GotoTarget, SearchDirection, TextEncoding, ViewMotion, ViewPage, ViewerMode,
    ViewerPrefs,
};

/// Window size for the file reads that back rendering, searching, and indexing.
const CHUNK: usize = 64 * 1024;
/// Largest read a single render will grow to when one logical line is longer
/// than [`CHUNK`]. Past this the line is simply clipped — no file is worth
/// stalling the UI over.
const MAX_RENDER_READ: usize = 4 << 20;
/// How far ahead the line index will chase a downward jump. Beyond this the
/// gutter goes blank instead of scanning gigabytes for line numbers.
const INDEX_CHASE_LIMIT: u64 = 4 << 20;
/// Horizontal scroll step, in characters, when wrapping is off.
const COL_STEP: u64 = 8;

/// Open viewer sessions, keyed by id. Owned by the core (it is nothing but
/// `std::fs`), wrapped in Tauri managed state by the adapter.
#[derive(Default)]
pub struct Sessions {
    next_id: u64,
    map: HashMap<String, Session>,
}

struct Session {
    path: PathBuf,
    name: String,
    file: File,
    probe: FileProbe,
    writable: bool,
    mode: ViewerMode,
    wrap: bool,
    tab_width: usize,
    hex_bytes_per_row: u64,
    rows: usize,
    cols: usize,
    top_offset: u64,
    col_offset: u64,
    /// Byte offsets of logical line starts from the start of the file, grown
    /// forward only. `line_starts[i]` is the start of line `i + 1`.
    line_starts: Vec<u64>,
    /// End of the contiguous region [`Self::line_starts`] describes.
    indexed_to: u64,
    /// Offset the next PageDown lands on; computed while rendering, so a page
    /// step always advances by exactly what was shown.
    next_page: u64,
}

impl Sessions {
    /// Open `path` in the viewer. `mode` comes from routing — text, hex, or
    /// image — and the caller has already probed the file.
    pub fn open(
        &mut self,
        path: &str,
        probe: FileProbe,
        mode: ViewerMode,
        prefs: &ViewerPrefs,
    ) -> Result<String, String> {
        let path = PathBuf::from(path);
        let file =
            File::open(&path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
        let writable = !file
            .metadata()
            .map(|m| m.permissions().readonly())
            .unwrap_or(true);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        self.next_id += 1;
        let id = format!("v{}", self.next_id);
        self.map.insert(
            id.clone(),
            Session {
                path,
                name,
                file,
                probe,
                writable,
                mode,
                wrap: prefs.wrap,
                tab_width: prefs.tab_width.max(1) as usize,
                hex_bytes_per_row: prefs.hex_bytes_per_row.max(1) as u64,
                rows: 24,
                cols: 80,
                top_offset: 0,
                col_offset: 0,
                line_starts: vec![0],
                indexed_to: 0,
                next_page: 0,
            },
        );
        Ok(id)
    }

    /// Render the current window.
    pub fn page(&mut self, id: &str) -> Result<ViewPage, String> {
        let owned = id.to_string();
        self.get(id)?.render(&owned)
    }

    /// Report the visible geometry, in rows and characters. Same contract as the
    /// panels' `set_viewport`: the frontend owns pixels, the core owns what that
    /// means for the content.
    pub fn set_viewport(&mut self, id: &str, rows: u16, cols: u16) -> Result<ViewPage, String> {
        let owned = id.to_string();
        let s = self.get(id)?;
        s.rows = rows.max(1) as usize;
        s.cols = cols.max(8) as usize;
        s.render(&owned)
    }

    pub fn scroll(&mut self, id: &str, motion: ViewMotion) -> Result<ViewPage, String> {
        let owned = id.to_string();
        let s = self.get(id)?;
        s.apply(motion)?;
        s.render(&owned)
    }

    /// Switch between text and hex, keeping the byte position (F4). Image
    /// sessions ignore this — there is nothing to toggle.
    pub fn toggle_mode(&mut self, id: &str) -> Result<ViewPage, String> {
        let owned = id.to_string();
        let s = self.get(id)?;
        s.mode = match s.mode {
            ViewerMode::Text => ViewerMode::Hex,
            ViewerMode::Hex => ViewerMode::Text,
            ViewerMode::Image => ViewerMode::Image,
        };
        s.col_offset = 0;
        let top = s.top_offset;
        s.seek_to(top)?;
        s.render(&owned)
    }

    pub fn set_wrap(&mut self, id: &str, wrap: bool) -> Result<ViewPage, String> {
        let owned = id.to_string();
        let s = self.get(id)?;
        s.wrap = wrap;
        s.col_offset = 0;
        s.render(&owned)
    }

    /// Search from just past the current position. Returns `Ok(None)` when the
    /// needle is not found — "not found" is a normal outcome, not an error.
    pub fn search(
        &mut self,
        id: &str,
        needle: &str,
        direction: SearchDirection,
    ) -> Result<Option<ViewPage>, String> {
        let owned = id.to_string();
        let s = self.get(id)?;
        match s.find(needle, direction)? {
            Some(hit) => {
                s.seek_to(hit)?;
                Ok(Some(s.render(&owned)?))
            }
            None => Ok(None),
        }
    }

    pub fn goto(&mut self, id: &str, target: GotoTarget) -> Result<ViewPage, String> {
        let owned = id.to_string();
        let s = self.get(id)?;
        let offset = match target {
            GotoTarget::Offset(o) => o.min(s.probe.size),
            GotoTarget::Percent(p) => s.probe.size * u64::from(p.min(100)) / 100,
            GotoTarget::Line(n) => s.line_offset(n.max(1))?,
        };
        s.seek_to(offset)?;
        s.render(&owned)
    }

    /// Path of an open session, so the adapter can hand F6 (view → edit) the
    /// file without the frontend ever holding authority over it.
    pub fn path_of(&self, id: &str) -> Option<String> {
        self.map
            .get(id)
            .map(|s| s.path.to_string_lossy().into_owned())
    }

    pub fn close(&mut self, id: &str) {
        self.map.remove(id);
    }

    fn get(&mut self, id: &str) -> Result<&mut Session, String> {
        self.map
            .get_mut(id)
            .ok_or_else(|| format!("viewer session {id} is no longer open"))
    }
}

impl Session {
    // --- rendering ---------------------------------------------------------

    fn render(&mut self, id: &str) -> Result<ViewPage, String> {
        let (gutter, rows) = match self.mode {
            ViewerMode::Text => self.render_text()?,
            ViewerMode::Hex => self.render_hex()?,
            // The frontend renders the image straight from the path; there are
            // no rows, but the page still carries the status-line facts.
            ViewerMode::Image => (Vec::new(), Vec::new()),
        };
        let percent = if self.probe.size == 0 {
            100
        } else {
            (self.top_offset.min(self.probe.size) * 100 / self.probe.size) as u8
        };
        Ok(ViewPage {
            id: id.to_string(),
            path: self.path.to_string_lossy().into_owned(),
            name: self.name.clone(),
            mode: self.mode,
            wrap: self.wrap,
            encoding: self.probe.encoding,
            gutter,
            rows,
            top_offset: self.top_offset,
            total_bytes: self.probe.size,
            percent,
            top_line: match self.mode {
                ViewerMode::Text => self.line_number_at(self.top_offset),
                _ => None,
            },
            col_offset: self.col_offset,
            writable: self.writable,
        })
    }

    /// Text mode: logical lines, tab-expanded, then either wrapped at `cols` or
    /// sliced horizontally at `col_offset`.
    fn render_text(&mut self) -> Result<(Vec<String>, Vec<String>), String> {
        let enc = self.probe.encoding;
        let mut gutter: Vec<String> = Vec::new();
        let mut rows: Vec<String> = Vec::new();
        let mut line_no = self.line_number_at(self.top_offset);
        let mut cursor = self.top_offset;
        // The offset the last *fully* rendered line ended at — where PageDown
        // should land.
        let mut consumed = self.top_offset;
        let mut need = CHUNK.max(self.rows * (self.cols + self.col_offset as usize + 1) * 4);

        while rows.len() < self.rows && cursor < self.probe.size {
            let buf = self.read_at(cursor, need)?;
            if buf.is_empty() {
                break;
            }
            let at_eof = cursor + buf.len() as u64 >= self.probe.size;
            let spans = line_spans(&buf, enc, at_eof);
            if spans.is_empty() {
                // A single logical line longer than the read window. Grow the
                // read once or twice, then give up and clip it.
                if need < MAX_RENDER_READ {
                    need = (need * 8).min(MAX_RENDER_READ);
                    continue;
                }
                let text = codec::decode(&buf, enc);
                self.push_line(&mut rows, &mut gutter, &text, line_no);
                cursor += buf.len() as u64;
                consumed = cursor;
                line_no = None; // the clip broke line accounting
                continue;
            }

            let buf_start = cursor;
            for span in spans {
                if rows.len() >= self.rows {
                    break;
                }
                let text = codec::decode(&buf[span.start..span.start + span.content], enc);
                self.push_line(&mut rows, &mut gutter, &text, line_no);
                line_no = line_no.map(|n| n + 1);
                cursor = buf_start + (span.start + span.total) as u64;
                // Only count the line as consumed if all of its display rows
                // fit; otherwise the next page must resume at this line.
                if rows.len() <= self.rows {
                    consumed = cursor;
                }
            }
        }

        // Guard against a wrapped line taller than the window, which would
        // otherwise make PageDown a no-op: always advance by at least one line.
        self.next_page = if consumed > self.top_offset {
            consumed
        } else {
            self.next_line_start(self.top_offset)?
        };
        Ok((gutter, rows))
    }

    /// Append one logical line's display rows, respecting wrap, tab width, the
    /// horizontal offset, and the remaining room in the window.
    fn push_line(
        &self,
        rows: &mut Vec<String>,
        gutter: &mut Vec<String>,
        text: &str,
        line_no: Option<u64>,
    ) {
        let label = line_no.map(|n| n.to_string()).unwrap_or_default();
        let expanded = expand_tabs(text, self.tab_width);
        if !self.wrap {
            let row: String = expanded
                .chars()
                .skip(self.col_offset as usize)
                .take(self.cols)
                .collect();
            rows.push(row);
            gutter.push(label);
            return;
        }
        let chars: Vec<char> = expanded.chars().collect();
        if chars.is_empty() {
            rows.push(String::new());
            gutter.push(label);
            return;
        }
        for (i, chunk) in chars.chunks(self.cols).enumerate() {
            if rows.len() >= self.rows {
                return;
            }
            rows.push(chunk.iter().collect());
            // Only the first display row of a wrapped line is numbered.
            gutter.push(if i == 0 { label.clone() } else { String::new() });
        }
    }

    /// Hex mode: `hex_bytes_per_row` bytes per row, formatted here so the
    /// frontend renders hex and text through the same code path.
    fn render_hex(&mut self) -> Result<(Vec<String>, Vec<String>), String> {
        let bpr = self.hex_bytes_per_row as usize;
        let buf = self.read_at(self.top_offset, bpr * self.rows)?;
        let mut gutter = Vec::new();
        let mut rows = Vec::new();
        for (i, chunk) in buf.chunks(bpr).enumerate() {
            let offset = self.top_offset + (i * bpr) as u64;
            gutter.push(format!("{offset:08X}"));
            let mut hex = String::with_capacity(bpr * 3);
            for (j, b) in chunk.iter().enumerate() {
                if j > 0 {
                    hex.push(' ');
                }
                // A gap at the halfway mark, the way every hex dump does it.
                if j > 0 && j % 8 == 0 {
                    hex.push(' ');
                }
                hex.push_str(&format!("{b:02X}"));
            }
            let ascii: String = chunk
                .iter()
                .map(|b| if (0x20..0x7F).contains(b) { *b as char } else { '.' })
                .collect();
            let width = bpr * 3 + bpr / 8;
            rows.push(format!("{hex:<width$} │ {ascii}"));
        }
        self.next_page = (self.top_offset + (bpr * self.rows) as u64).min(self.probe.size);
        Ok((gutter, rows))
    }

    // --- motion ------------------------------------------------------------

    fn apply(&mut self, motion: ViewMotion) -> Result<(), String> {
        if self.mode == ViewerMode::Hex {
            return self.apply_hex(motion);
        }
        match motion {
            ViewMotion::LineDown => {
                let next = self.next_line_start(self.top_offset)?;
                if next < self.probe.size {
                    self.set_top(next);
                }
            }
            ViewMotion::LineUp => {
                let prev = self.prev_line_start(self.top_offset)?;
                self.set_top(prev);
            }
            ViewMotion::PageDown => {
                if self.next_page < self.probe.size {
                    self.set_top(self.next_page);
                }
            }
            ViewMotion::PageUp => {
                let target = self.back_up_rows(self.top_offset, self.rows)?;
                self.set_top(target);
            }
            ViewMotion::Home => {
                self.set_top(0);
                self.col_offset = 0;
            }
            ViewMotion::End => {
                let target = self.back_up_rows(self.probe.size, self.rows)?;
                self.set_top(target);
            }
            ViewMotion::ColLeft => self.col_offset = self.col_offset.saturating_sub(COL_STEP),
            ViewMotion::ColRight => {
                if !self.wrap {
                    self.col_offset += COL_STEP;
                }
            }
        }
        Ok(())
    }

    /// Hex motion is pure arithmetic over row-aligned offsets.
    fn apply_hex(&mut self, motion: ViewMotion) -> Result<(), String> {
        let bpr = self.hex_bytes_per_row;
        let page = bpr * self.rows as u64;
        let last_page_top = self.probe.size.saturating_sub(1) / bpr * bpr;
        let top = match motion {
            ViewMotion::LineDown => (self.top_offset + bpr).min(last_page_top),
            ViewMotion::LineUp => self.top_offset.saturating_sub(bpr),
            ViewMotion::PageDown => (self.top_offset + page).min(last_page_top),
            ViewMotion::PageUp => self.top_offset.saturating_sub(page),
            ViewMotion::Home => 0,
            ViewMotion::End => last_page_top.saturating_sub(page.saturating_sub(bpr)),
            ViewMotion::ColLeft | ViewMotion::ColRight => self.top_offset,
        };
        self.top_offset = top / bpr * bpr;
        Ok(())
    }

    /// Move the window to an arbitrary offset, snapping to whatever the current
    /// mode considers a row boundary.
    fn seek_to(&mut self, offset: u64) -> Result<(), String> {
        let offset = offset.min(self.probe.size);
        let snapped = match self.mode {
            ViewerMode::Hex => offset / self.hex_bytes_per_row * self.hex_bytes_per_row,
            ViewerMode::Text => self.line_start_containing(offset)?,
            ViewerMode::Image => 0,
        };
        self.set_top(snapped);
        Ok(())
    }

    /// Set the window top, extending the line index when the jump is short
    /// enough to be worth chasing (so ordinary scrolling keeps line numbers).
    fn set_top(&mut self, offset: u64) {
        self.top_offset = offset;
        if offset > self.indexed_to && offset - self.indexed_to <= INDEX_CHASE_LIMIT {
            let _ = self.index_forward(offset);
        }
    }

    /// Walk back from `from` until `want` display rows' worth of lines have been
    /// passed, and return that line's start — used by PageUp and End.
    fn back_up_rows(&mut self, from: u64, want: usize) -> Result<u64, String> {
        let mut offset = from;
        let mut counted = 0usize;
        while counted < want && offset > 0 {
            let prev = self.prev_line_start(offset)?;
            counted += self.display_rows_between(prev, offset)?;
            offset = prev;
        }
        Ok(offset)
    }

    /// How many display rows the line starting at `start` occupies. One, unless
    /// wrapping is on and the line is longer than the window is wide.
    fn display_rows_between(&mut self, start: u64, end: u64) -> Result<usize, String> {
        if !self.wrap {
            return Ok(1);
        }
        let len = (end - start).min(MAX_RENDER_READ as u64) as usize;
        let buf = self.read_at(start, len)?;
        let text = expand_tabs(&codec::decode(&buf, self.probe.encoding), self.tab_width);
        let chars = text.trim_end_matches(['\n', '\r']).chars().count();
        Ok(chars.div_ceil(self.cols).max(1))
    }

    // --- line geometry -----------------------------------------------------

    /// Offset of the line following the one starting at `offset`, or the file
    /// size when there is none.
    fn next_line_start(&mut self, offset: u64) -> Result<u64, String> {
        let enc = self.probe.encoding;
        let unit = codec::unit(enc);
        let buf = self.read_at(offset, CHUNK)?;
        if buf.is_empty() {
            return Ok(self.probe.size);
        }
        let at_eof = offset + buf.len() as u64 >= self.probe.size;
        match line_spans(&buf, enc, at_eof).first() {
            Some(span) => Ok(offset + (span.start + span.total) as u64),
            // A line longer than the window: step to the window edge, keeping
            // the offset unit-aligned so a UTF-16 file never decodes shifted.
            None => Ok((offset + buf.len() as u64) / unit * unit),
        }
    }

    /// Offset of the line before the one starting at `offset`.
    fn prev_line_start(&mut self, offset: u64) -> Result<u64, String> {
        if offset == 0 {
            return Ok(0);
        }
        let enc = self.probe.encoding;
        let unit = codec::unit(enc) as usize;
        let win = (CHUNK as u64).min(offset);
        let start = offset - win;
        let buf = self.read_at(start, win as usize)?;

        let mut i = buf.len().saturating_sub(unit);
        // Step over the terminator that ended the previous line (CR, LF, CRLF).
        if unit_value(&buf, i, enc) == Some(b'\n' as u16) {
            i = i.saturating_sub(unit);
        }
        if unit_value(&buf, i, enc) == Some(b'\r' as u16) && i >= unit {
            i -= unit;
        } else if i == 0 && unit_value(&buf, 0, enc) == Some(b'\r' as u16) {
            return Ok(start);
        }
        loop {
            match unit_value(&buf, i, enc) {
                Some(v) if v == b'\n' as u16 || v == b'\r' as u16 => {
                    return Ok(start + (i + unit) as u64)
                }
                _ => {}
            }
            if i < unit {
                break;
            }
            i -= unit;
        }
        // No break inside the window: either the file starts here, or the line
        // is longer than the window and we settle for its edge.
        Ok(start)
    }

    /// Start of the line that *contains* `offset` — where a search hit or a
    /// Goto-by-offset lands the window.
    fn line_start_containing(&mut self, offset: u64) -> Result<u64, String> {
        if offset == 0 {
            return Ok(0);
        }
        let enc = self.probe.encoding;
        let unit = codec::unit(enc) as usize;
        let win = (CHUNK as u64).min(offset);
        let start = offset - win;
        let buf = self.read_at(start, win as usize)?;
        let mut i = buf.len().saturating_sub(unit);
        loop {
            match unit_value(&buf, i, enc) {
                Some(v) if v == b'\n' as u16 || v == b'\r' as u16 => {
                    return Ok(start + (i + unit) as u64)
                }
                _ => {}
            }
            if i < unit {
                break;
            }
            i -= unit;
        }
        Ok(start)
    }

    /// 1-based line number at `offset`, when the index reaches that far and the
    /// offset really is a line start. `None` means "unknown" — the gutter goes
    /// blank rather than the viewer paying for a full-file scan.
    fn line_number_at(&self, offset: u64) -> Option<u64> {
        if offset > self.indexed_to && offset != 0 {
            return None;
        }
        self.line_starts
            .binary_search(&offset)
            .ok()
            .map(|i| i as u64 + 1)
    }

    /// Offset of 1-based line `n`, indexing forward as far as needed. This is
    /// the one place an unbounded scan is allowed: the user explicitly asked to
    /// go to a line, so the cost is theirs to spend.
    fn line_offset(&mut self, n: u64) -> Result<u64, String> {
        while (self.line_starts.len() as u64) < n && self.indexed_to < self.probe.size {
            self.index_chunk()?;
        }
        let idx = (n as usize - 1).min(self.line_starts.len() - 1);
        Ok(self.line_starts[idx])
    }

    /// Extend the line index to cover at least `until`.
    fn index_forward(&mut self, until: u64) -> Result<(), String> {
        while self.indexed_to < until && self.indexed_to < self.probe.size {
            self.index_chunk()?;
        }
        Ok(())
    }

    /// Scan one chunk forward, recording the line starts it contains.
    fn index_chunk(&mut self) -> Result<(), String> {
        let from = self.indexed_to;
        let buf = self.read_at(from, CHUNK)?;
        if buf.is_empty() {
            self.indexed_to = self.probe.size;
            return Ok(());
        }
        let enc = self.probe.encoding;
        let at_eof = from + buf.len() as u64 >= self.probe.size;
        let spans = line_spans(&buf, enc, at_eof);
        let mut end = from;
        for span in &spans {
            let next = from + (span.start + span.total) as u64;
            if next > *self.line_starts.last().unwrap_or(&0) && next < self.probe.size {
                self.line_starts.push(next);
            }
            end = next;
        }
        // No complete line in the whole chunk — a very long line; skip past it
        // without recording anything.
        self.indexed_to = if spans.is_empty() {
            from + buf.len() as u64
        } else {
            end
        };
        Ok(())
    }

    // --- search ------------------------------------------------------------

    /// Find `needle` from just past the current position, wrapping neither end.
    /// The needle is encoded into the file's own encoding and matched on raw
    /// bytes, which works uniformly for UTF-8, Latin-1, and UTF-16.
    fn find(&mut self, needle: &str, direction: SearchDirection) -> Result<Option<u64>, String> {
        if needle.is_empty() {
            return Ok(None);
        }
        let enc = self.probe.encoding;
        let Some(encoded) = codec::encode(needle, enc) else {
            // The needle cannot exist in this encoding, so it cannot be found.
            return Ok(None);
        };
        let pat = ascii_lower(codec::strip_bom(&encoded, enc));
        if pat.is_empty() || pat.len() as u64 > self.probe.size {
            return Ok(None);
        }
        let overlap = pat.len() - 1;

        match direction {
            SearchDirection::Forward => {
                let mut at = self.top_offset + codec::unit(enc);
                while at < self.probe.size {
                    let buf = self.read_at(at, CHUNK)?;
                    if buf.is_empty() {
                        break;
                    }
                    let hay = ascii_lower(&buf);
                    if let Some(pos) = find_bytes(&hay, &pat) {
                        return Ok(Some(at + pos as u64));
                    }
                    if at + buf.len() as u64 >= self.probe.size {
                        break;
                    }
                    // Overlap so a match straddling the window boundary is found.
                    at += (buf.len() - overlap) as u64;
                }
            }
            SearchDirection::Backward => {
                let mut end = self.top_offset;
                while end > 0 {
                    let win = (CHUNK as u64).min(end + overlap as u64);
                    let start = (end + overlap as u64).saturating_sub(win);
                    let buf = self.read_at(start, win as usize)?;
                    let hay = ascii_lower(&buf);
                    if let Some(pos) = rfind_bytes(&hay, &pat) {
                        let hit = start + pos as u64;
                        if hit < self.top_offset {
                            return Ok(Some(hit));
                        }
                    }
                    if start == 0 {
                        break;
                    }
                    end = start;
                }
            }
        }
        Ok(None)
    }

    // --- io ----------------------------------------------------------------

    fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        if offset >= self.probe.size || len == 0 {
            return Ok(Vec::new());
        }
        let len = len.min((self.probe.size - offset) as usize);
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| format!("could not seek {}: {e}", self.name))?;
        let mut buf = vec![0u8; len];
        let mut got = 0;
        while got < len {
            match self.file.read(&mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) => return Err(format!("could not read {}: {e}", self.name)),
            }
        }
        buf.truncate(got);
        Ok(buf)
    }
}

/// One logical line inside a read buffer: where it starts, how many bytes of
/// content it has, and how many bytes it occupies including its terminator.
struct LineSpan {
    start: usize,
    content: usize,
    total: usize,
}

/// Split a buffer into logical lines. Only *terminated* lines are returned
/// unless `at_eof`, so a line cut by the window boundary is never mistaken for a
/// short line — the caller re-reads instead.
fn line_spans(buf: &[u8], enc: TextEncoding, at_eof: bool) -> Vec<LineSpan> {
    let unit = codec::unit(enc) as usize;
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i + unit <= buf.len() {
        let v = unit_value(buf, i, enc).unwrap_or(0);
        if v == b'\n' as u16 || v == b'\r' as u16 {
            let mut total = i + unit - start;
            if v == b'\r' as u16 && unit_value(buf, i + unit, enc) == Some(b'\n' as u16) {
                total += unit;
            }
            spans.push(LineSpan {
                start,
                content: i - start,
                total,
            });
            start += total;
            i = start;
            continue;
        }
        i += unit;
    }
    // A trailing line with no terminator is only a line if the file ends here.
    if at_eof && start < buf.len() {
        spans.push(LineSpan {
            start,
            content: buf.len() - start,
            total: buf.len() - start,
        });
    }
    spans
}

/// The character unit at byte index `i`, as a `u16` so UTF-16 and single-byte
/// encodings compare the same way. `None` past the end of the buffer.
fn unit_value(buf: &[u8], i: usize, enc: TextEncoding) -> Option<u16> {
    match enc {
        TextEncoding::Utf16Le => Some(u16::from_le_bytes([*buf.get(i)?, *buf.get(i + 1)?])),
        TextEncoding::Utf16Be => Some(u16::from_be_bytes([*buf.get(i)?, *buf.get(i + 1)?])),
        _ => buf.get(i).map(|b| u16::from(*b)),
    }
}

/// Expand tabs to the next tab stop, so columns line up the way they do in the
/// user's editor.
fn expand_tabs(text: &str, width: usize) -> String {
    if !text.contains('\t') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + width);
    let mut col = 0;
    for c in text.chars() {
        if c == '\t' {
            let pad = width - (col % width);
            out.extend(std::iter::repeat_n(' ', pad));
            col += pad;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

/// ASCII-fold a buffer so search is case-insensitive without decoding it.
fn ascii_lower(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(|b| b.to_ascii_lowercase()).collect()
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MediaKind;
    use std::io::Write;

    /// A scratch file that cleans itself up.
    struct Temp(PathBuf);
    impl Temp {
        fn new(tag: &str, bytes: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "fm_view_{tag}_{}_{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let mut f = File::create(&path).unwrap();
            f.write_all(bytes).unwrap();
            Temp(path)
        }
        fn as_str(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            std::fs::remove_file(&self.0).ok();
        }
    }

    fn open(tmp: &Temp, mode: ViewerMode, rows: u16, cols: u16) -> (Sessions, String) {
        let mut s = Sessions::default();
        let p = probe::probe(&tmp.0).unwrap();
        let id = s.open(&tmp.as_str(), p, mode, &ViewerPrefs::default()).unwrap();
        s.set_viewport(&id, rows, cols).unwrap();
        (s, id)
    }

    fn numbered(lines: usize) -> Vec<u8> {
        (1..=lines)
            .map(|i| format!("line {i}\n"))
            .collect::<String>()
            .into_bytes()
    }

    #[test]
    fn text_paging_walks_the_file_and_comes_back() {
        let tmp = Temp::new("page", &numbered(100));
        let (mut s, id) = open(&tmp, ViewerMode::Text, 10, 40);

        let page = s.page(&id).unwrap();
        assert_eq!(page.rows.len(), 10);
        assert_eq!(page.rows[0], "line 1");
        assert_eq!(page.gutter[0], "1");
        assert_eq!(page.top_line, Some(1));
        assert_eq!(page.percent, 0);

        let page = s.scroll(&id, ViewMotion::PageDown).unwrap();
        assert_eq!(page.rows[0], "line 11");
        assert_eq!(page.top_line, Some(11));

        let page = s.scroll(&id, ViewMotion::LineDown).unwrap();
        assert_eq!(page.rows[0], "line 12");
        let page = s.scroll(&id, ViewMotion::LineUp).unwrap();
        assert_eq!(page.rows[0], "line 11");

        let page = s.scroll(&id, ViewMotion::PageUp).unwrap();
        assert_eq!(page.rows[0], "line 1");
        assert_eq!(page.top_offset, 0);
    }

    #[test]
    fn end_shows_the_last_screenful_and_home_returns() {
        let tmp = Temp::new("end", &numbered(100));
        let (mut s, id) = open(&tmp, ViewerMode::Text, 10, 40);

        let page = s.scroll(&id, ViewMotion::End).unwrap();
        assert_eq!(page.rows.len(), 10);
        assert_eq!(page.rows.last().unwrap(), "line 100");
        assert_eq!(page.rows[0], "line 91");

        let page = s.scroll(&id, ViewMotion::Home).unwrap();
        assert_eq!(page.rows[0], "line 1");
    }

    #[test]
    fn paging_down_past_the_end_stops_rather_than_running_off() {
        let tmp = Temp::new("short", b"a\nb\nc\n");
        let (mut s, id) = open(&tmp, ViewerMode::Text, 10, 40);
        for _ in 0..5 {
            s.scroll(&id, ViewMotion::PageDown).unwrap();
        }
        let page = s.page(&id).unwrap();
        assert_eq!(page.rows, vec!["a", "b", "c"]);
        assert_eq!(page.top_offset, 0);
    }

    #[test]
    fn wrap_splits_long_lines_and_numbers_only_the_first_row() {
        let tmp = Temp::new("wrap", b"0123456789abcdefghij\nshort\n");
        let (mut s, id) = open(&tmp, ViewerMode::Text, 10, 10);
        let page = s.set_wrap(&id, true).unwrap();
        assert_eq!(page.rows[0], "0123456789");
        assert_eq!(page.rows[1], "abcdefghij");
        assert_eq!(page.rows[2], "short");
        assert_eq!(page.gutter[..3], ["1", "", "2"]);

        // Unwrapped, the same line is clipped to the window width instead.
        let page = s.set_wrap(&id, false).unwrap();
        assert_eq!(page.rows[0], "0123456789");
        assert_eq!(page.rows[1], "short");
    }

    #[test]
    fn horizontal_scrolling_slices_the_line() {
        let tmp = Temp::new("hscroll", b"0123456789abcdefghij\n");
        let (mut s, id) = open(&tmp, ViewerMode::Text, 4, 10);
        let page = s.scroll(&id, ViewMotion::ColRight).unwrap();
        assert_eq!(page.col_offset, COL_STEP);
        assert_eq!(page.rows[0], "89abcdefgh");
        let page = s.scroll(&id, ViewMotion::ColLeft).unwrap();
        assert_eq!(page.rows[0], "0123456789");
    }

    #[test]
    fn crlf_and_tabs_render_without_artefacts() {
        let tmp = Temp::new("crlf", b"a\tb\r\nsecond\r\n");
        let (mut s, id) = open(&tmp, ViewerMode::Text, 4, 40);
        let page = s.page(&id).unwrap();
        assert_eq!(page.rows[0], "a   b"); // tab expands to the next stop of 4
        assert_eq!(page.rows[1], "second");
    }

    #[test]
    fn a_file_without_a_trailing_newline_still_shows_its_last_line() {
        let tmp = Temp::new("noeol", b"one\ntwo");
        let (mut s, id) = open(&tmp, ViewerMode::Text, 4, 40);
        assert_eq!(s.page(&id).unwrap().rows, vec!["one", "two"]);
    }

    #[test]
    fn an_empty_file_renders_an_empty_page_at_100_percent() {
        let tmp = Temp::new("empty", b"");
        let (mut s, id) = open(&tmp, ViewerMode::Text, 4, 40);
        let page = s.page(&id).unwrap();
        assert!(page.rows.is_empty());
        assert_eq!(page.percent, 100);
        // Motions on an empty file must not panic or move anywhere.
        for m in [ViewMotion::End, ViewMotion::PageDown, ViewMotion::LineDown] {
            assert_eq!(s.scroll(&id, m).unwrap().top_offset, 0);
        }
    }

    #[test]
    fn hex_mode_formats_rows_and_pages_by_offset() {
        let bytes: Vec<u8> = (0..=255u8).collect();
        let tmp = Temp::new("hex", &bytes);
        let (mut s, id) = open(&tmp, ViewerMode::Hex, 4, 80);
        let page = s.page(&id).unwrap();
        assert_eq!(page.gutter[0], "00000000");
        assert!(page.rows[0].starts_with("00 01 02 03 04 05 06 07  08"));
        assert!(page.rows[0].ends_with("│ ................"));
        assert_eq!(page.rows.len(), 4);

        let page = s.scroll(&id, ViewMotion::PageDown).unwrap();
        assert_eq!(page.top_offset, 64);
        assert_eq!(page.gutter[0], "00000040");

        // The printable middle of the byte range shows as ASCII.
        let page = s.goto(&id, GotoTarget::Offset(0x41)).unwrap();
        assert_eq!(page.top_offset, 0x40); // snapped to the row boundary
        assert!(page.rows[0].ends_with("│ @ABCDEFGHIJKLMNO"));

        let page = s.scroll(&id, ViewMotion::End).unwrap();
        assert_eq!(page.rows.len(), 4);
        assert_eq!(page.top_offset, 256 - 64);
    }

    #[test]
    fn toggling_modes_keeps_the_position() {
        let tmp = Temp::new("toggle", &numbered(100));
        let (mut s, id) = open(&tmp, ViewerMode::Text, 10, 40);
        s.scroll(&id, ViewMotion::PageDown).unwrap();
        let text_top = s.page(&id).unwrap().top_offset;

        let hex = s.toggle_mode(&id).unwrap();
        assert_eq!(hex.mode, ViewerMode::Hex);
        assert!(hex.top_offset <= text_top && text_top - hex.top_offset < 16);

        // Coming back, the row-aligned hex offset lands inside the previous
        // line, and text mode snaps to that line's start — the small backwards
        // drift FAR has too, and far better than resuming mid-line.
        let back = s.toggle_mode(&id).unwrap();
        assert_eq!(back.mode, ViewerMode::Text);
        assert_eq!(back.rows[0], "line 10");
    }

    #[test]
    fn search_finds_forward_and_backward_and_reports_misses() {
        let tmp = Temp::new("search", &numbered(500));
        let (mut s, id) = open(&tmp, ViewerMode::Text, 10, 40);

        // Case-insensitive, and the window lands on the hit's line.
        let page = s.search(&id, "LINE 400", SearchDirection::Forward).unwrap();
        assert_eq!(page.unwrap().rows[0], "line 400");

        let page = s.search(&id, "line 100", SearchDirection::Backward).unwrap();
        assert_eq!(page.unwrap().rows[0], "line 100");

        assert!(s.search(&id, "nothing here", SearchDirection::Forward).unwrap().is_none());
        assert!(s.search(&id, "", SearchDirection::Forward).unwrap().is_none());
    }

    #[test]
    fn search_finds_a_match_straddling_a_read_window() {
        // Put the needle right across the 64 KiB boundary.
        let mut bytes = vec![b'.'; CHUNK - 4];
        bytes.extend_from_slice(b"\nneedle\n");
        bytes.extend(std::iter::repeat_n(b'.', 100));
        let tmp = Temp::new("straddle", &bytes);
        let (mut s, id) = open(&tmp, ViewerMode::Text, 10, 40);
        let page = s.search(&id, "needle", SearchDirection::Forward).unwrap();
        assert_eq!(page.unwrap().rows[0], "needle");
    }

    #[test]
    fn goto_reaches_lines_percentages_and_offsets() {
        let tmp = Temp::new("goto", &numbered(1000));
        let (mut s, id) = open(&tmp, ViewerMode::Text, 10, 40);

        let page = s.goto(&id, GotoTarget::Line(700)).unwrap();
        assert_eq!(page.rows[0], "line 700");
        assert_eq!(page.top_line, Some(700));

        let page = s.goto(&id, GotoTarget::Percent(0)).unwrap();
        assert_eq!(page.rows[0], "line 1");

        let page = s.goto(&id, GotoTarget::Percent(100)).unwrap();
        assert_eq!(page.percent, 100);

        // Past the end clamps to the last line rather than failing.
        let page = s.goto(&id, GotoTarget::Line(99_999)).unwrap();
        assert_eq!(page.rows[0], "line 1000");
    }

    #[test]
    fn a_line_longer_than_the_read_window_is_clipped_not_hung() {
        // One logical line bigger than the largest read the renderer will grow
        // to — a minified bundle, say — followed by ordinary lines.
        let mut bytes = vec![b'x'; MAX_RENDER_READ + 1000];
        bytes.push(b'\n');
        bytes.extend_from_slice(&numbered(50));
        let tmp = Temp::new("longline", &bytes);
        let (mut s, id) = open(&tmp, ViewerMode::Text, 4, 20);

        let page = s.page(&id).unwrap();
        assert_eq!(page.rows[0].chars().count(), 20, "the giant line is clipped");

        // Paging must still make progress through it and reach what follows.
        let mut top = page.top_offset;
        for _ in 0..8 {
            let page = s.scroll(&id, ViewMotion::PageDown).unwrap();
            assert!(page.top_offset > top, "PageDown must always advance");
            top = page.top_offset;
        }
        let page = s.scroll(&id, ViewMotion::End).unwrap();
        assert_eq!(page.rows.last().unwrap(), "line 50");
    }

    #[test]
    fn utf16_files_render_and_page_correctly() {
        let mut bytes = codec::encode("alpha\nbeta\ngamma\n", TextEncoding::Utf16Le).unwrap();
        bytes.truncate(bytes.len()); // includes the BOM
        let tmp = Temp::new("utf16", &bytes);
        let (mut s, id) = open(&tmp, ViewerMode::Text, 4, 40);
        let page = s.page(&id).unwrap();
        assert_eq!(page.encoding, TextEncoding::Utf16Le);
        // The BOM decodes as a zero-width mark at the head of the first line.
        assert!(page.rows[0].ends_with("alpha"));
        assert_eq!(page.rows[1], "beta");
        assert_eq!(page.rows[2], "gamma");
    }

    #[test]
    fn latin1_files_decode_with_their_accents_intact() {
        let tmp = Temp::new("latin1", b"caf\xE9 cr\xE8me\nsecond\n");
        let (mut s, id) = open(&tmp, ViewerMode::Text, 4, 40);
        let page = s.page(&id).unwrap();
        assert_eq!(page.encoding, TextEncoding::Latin1);
        assert_eq!(page.rows[0], "café crème");
    }

    #[test]
    fn a_binary_file_probes_as_binary_and_views_as_hex() {
        let tmp = Temp::new("bin", b"\x7FELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
        let p = probe::probe(&tmp.0).unwrap();
        assert_eq!(p.media, MediaKind::Binary);
        let (mut s, id) = open(&tmp, ViewerMode::Hex, 4, 80);
        assert!(s.page(&id).unwrap().rows[0].starts_with("7F 45 4C 46"));
    }

    #[test]
    fn closing_a_session_makes_further_calls_fail_cleanly() {
        let tmp = Temp::new("close", b"hi\n");
        let (mut s, id) = open(&tmp, ViewerMode::Text, 4, 40);
        assert!(s.path_of(&id).is_some());
        s.close(&id);
        assert!(s.path_of(&id).is_none());
        assert!(s.page(&id).is_err());
    }
}
