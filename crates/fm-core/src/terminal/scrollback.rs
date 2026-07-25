//! The terminal's scrollback buffer (SPEC §5.7).
//!
//! A byte-capped ring of whole lines. Output from every command ever run in the
//! session accumulates here — the pane is a window onto it, not its owner — so
//! the user can execute something with the terminal collapsed and read the
//! output later.
//!
//! ## Why lines, capped by bytes
//!
//! The cap the user configures is a byte budget (1 MiB by default), because that
//! is what "how much output do I keep" actually means. But eviction happens in
//! whole **lines**: dropping a partial line would leave mojibake at the top of
//! the pane and, worse, would force the frontend to do byte arithmetic on a
//! string it indexes in UTF-16 units. So [`Scrollback::append`] reports what
//! changed as a line delta ([`TerminalChunk`]) and the frontend's mirror applies
//! it mechanically, with no policy of its own.
//!
//! **The delta is append-then-drop**, matching the order the buffer itself uses:
//! `mirror.concat(chunk.lines).slice(chunk.dropped)`. A big append can evict
//! lines it just added — 3000 lines into a 2 KB buffer keeps only the last few
//! hundred — so `dropped` may exceed everything the mirror held beforehand, and
//! dropping first would silently leak the surplus.

use std::collections::VecDeque;

use crate::types::{TerminalBuffer, TerminalChunk};

/// Bytes charged per stored line on top of its own length — the newline that
/// separated it in the stream. Counting it keeps the cap honest for output made
/// of many short lines.
const LINE_OVERHEAD: usize = 1;

/// Never shrink below this many lines, however small the configured cap or
/// however long the lines: a buffer that cannot hold one screenful is useless.
const MIN_LINES: usize = 2;

/// A capped, line-oriented output buffer.
#[derive(Debug)]
pub struct Scrollback {
    lines: VecDeque<String>,
    /// The trailing line that has not been terminated by `\n` yet. Output
    /// arrives in arbitrary chunks, so a line is routinely split across appends.
    pending: String,
    /// Sum of `line.len() + LINE_OVERHEAD` over `lines`, maintained
    /// incrementally so appending never walks the buffer.
    bytes: usize,
    limit: usize,
}

impl Default for Scrollback {
    fn default() -> Self {
        Self::new(1 << 20)
    }
}

impl Scrollback {
    pub fn new(limit: u64) -> Self {
        Self {
            lines: VecDeque::new(),
            pending: String::new(),
            bytes: 0,
            limit: limit as usize,
        }
    }

    /// Append raw output bytes, returning the delta the frontend applies.
    ///
    /// Invalid UTF-8 is replaced rather than rejected: a file manager runs
    /// arbitrary programs, and one stray byte must not poison the pane. `\r` is
    /// stripped at line ends so CRLF output does not render a stray glyph — but
    /// mid-line carriage returns are left alone, since interpreting them is
    /// terminal-emulation, which this deliberately is not.
    pub fn append(&mut self, bytes: &[u8]) -> TerminalChunk {
        self.append_str(&String::from_utf8_lossy(bytes))
    }

    /// [`append`](Self::append) for text that is already valid UTF-8 — the path
    /// the core's own echo lines take.
    pub fn append_str(&mut self, text: &str) -> TerminalChunk {
        let mut added: Vec<String> = Vec::new();
        for ch in text.chars() {
            if ch == '\n' {
                let line = std::mem::take(&mut self.pending);
                added.push(line.clone());
                self.push_line(line);
            } else {
                self.pending.push(ch);
            }
        }
        let dropped = self.trim();
        TerminalChunk {
            lines: added,
            pending: self.pending.clone(),
            dropped,
        }
    }

    /// Append one complete line (used for the core's echo of the command being
    /// run and its exit footer). Any partial line in flight is flushed first, so
    /// a program that ended without a newline does not get its last line
    /// swallowed into ours.
    pub fn append_line(&mut self, line: &str) -> TerminalChunk {
        let mut text = String::new();
        if !self.pending.is_empty() {
            text.push('\n');
        }
        text.push_str(line);
        text.push('\n');
        self.append_str(&text)
    }

    fn push_line(&mut self, line: String) {
        self.bytes += line.len() + LINE_OVERHEAD;
        self.lines.push_back(line);
    }

    /// Evict whole lines from the front until the buffer fits its cap. Returns
    /// how many were dropped.
    fn trim(&mut self) -> u32 {
        let mut dropped = 0u32;
        while self.bytes > self.limit && self.lines.len() > MIN_LINES {
            if let Some(line) = self.lines.pop_front() {
                self.bytes -= line.len() + LINE_OVERHEAD;
                dropped += 1;
            }
        }
        dropped
    }

    /// Re-cap the buffer (the control in the corner of the expanded pane). The
    /// frontend re-pulls the whole buffer afterwards, so no delta is returned.
    pub fn set_limit(&mut self, limit: u64) {
        self.limit = limit as usize;
        self.trim();
    }

    pub fn limit(&self) -> u64 {
        self.limit as u64
    }

    /// Drop everything (`clear` / Ctrl+L).
    ///
    /// Reported as an ordinary delta that evicts every line, so the frontend's
    /// one mirror rule — `slice(dropped).concat(lines)` — handles a clear with no
    /// special case of its own.
    pub fn clear(&mut self) -> TerminalChunk {
        let dropped = self.lines.len() as u32;
        self.lines.clear();
        self.pending.clear();
        self.bytes = 0;
        TerminalChunk {
            lines: Vec::new(),
            pending: String::new(),
            dropped,
        }
    }

    /// The whole buffer, for the frontend's initial sync and re-sync.
    pub fn snapshot(&self) -> TerminalBuffer {
        TerminalBuffer {
            lines: self.lines.iter().cloned().collect(),
            pending: self.pending.clone(),
        }
    }

    #[cfg(test)]
    fn text(&self) -> String {
        let mut out = self
            .lines
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        if !self.pending.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&self.pending);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frontend's mirror, implemented exactly as `App.svelte` does it.
    /// Tests assert against this rather than restating the rule inline, so a
    /// change to the contract has to be made in one place and shows up here.
    #[derive(Default)]
    struct Mirror {
        lines: Vec<String>,
        pending: String,
    }

    impl Mirror {
        fn apply(&mut self, chunk: &TerminalChunk) {
            self.lines.extend(chunk.lines.iter().cloned());
            self.lines.drain(0..(chunk.dropped as usize).min(self.lines.len()));
            self.pending = chunk.pending.clone();
        }

        fn assert_matches(&self, sb: &Scrollback) {
            let snap = sb.snapshot();
            assert_eq!(self.lines, snap.lines, "mirror drifted from the buffer");
            assert_eq!(self.pending, snap.pending, "pending line drifted");
        }
    }

    #[test]
    fn splits_appends_into_lines_and_keeps_the_partial_tail() {
        let mut sb = Scrollback::new(1 << 20);

        let chunk = sb.append_str("one\ntwo\nthr");
        assert_eq!(chunk.lines, vec!["one", "two"]);
        assert_eq!(chunk.pending, "thr");
        assert_eq!(chunk.dropped, 0);

        // The partial line is completed by the *next* append, not re-emitted.
        let chunk = sb.append_str("ee\n");
        assert_eq!(chunk.lines, vec!["three"]);
        assert_eq!(chunk.pending, "");
        assert_eq!(sb.text(), "one\ntwo\nthree");
    }

    #[test]
    fn evicting_reports_the_exact_line_count_the_mirror_must_drop() {
        // Cap fits roughly three "aaaa" lines (4 + 1 overhead each).
        let mut sb = Scrollback::new(15);
        let mut mirror = Mirror::default();

        mirror.apply(&sb.append_str("aaaa\nbbbb\ncccc\n"));
        assert_eq!(sb.text(), "aaaa\nbbbb\ncccc");
        mirror.assert_matches(&sb);

        let chunk = sb.append_str("dddd\n");
        assert_eq!(chunk.lines, vec!["dddd"]);
        assert_eq!(chunk.dropped, 1);
        assert_eq!(sb.text(), "bbbb\ncccc\ndddd");
        mirror.apply(&chunk);
        mirror.assert_matches(&sb);
    }

    /// The delta is **append-then-drop**, and this is the case that proves it
    /// matters: one append that evicts more lines than the mirror held before it.
    /// Dropping first clamps at zero and leaks the surplus, so the mirror ends up
    /// permanently longer than the buffer — which is exactly what a burst of
    /// program output into a small scrollback does.
    #[test]
    fn one_append_may_evict_more_lines_than_the_mirror_previously_held() {
        let mut sb = Scrollback::new(15);
        let mut mirror = Mirror::default();

        mirror.apply(&sb.append_str("aaaa\nbbbb\n"));
        mirror.assert_matches(&sb);

        // Five lines land at once into a buffer that holds three; the eviction
        // reaches past the two the mirror had and into the new arrivals.
        let chunk = sb.append_str("cccc\ndddd\neeee\nffff\ngggg\n");
        assert!(
            chunk.dropped as usize > 2,
            "expected the eviction to exceed the mirror's prior length, got {}",
            chunk.dropped
        );
        mirror.apply(&chunk);
        mirror.assert_matches(&sb);
        assert_eq!(sb.text(), "eeee\nffff\ngggg");
    }

    /// The end-to-end shape of the live bug: `seq 1 3000` into a 2 KB scrollback,
    /// then a clear. Output arrives in whole pipe reads — ~1000 lines at a time,
    /// far more than the buffer holds — so every burst evicts into its own new
    /// lines. If the mirror drifts even once, the clear's `dropped` no longer
    /// covers it and stale output survives on screen.
    #[test]
    fn a_burst_bigger_than_the_buffer_then_a_clear_leaves_the_mirror_empty() {
        const BURST: u32 = 1000;
        let mut sb = Scrollback::new(2048);
        let mut mirror = Mirror::default();

        for start in (1..=3000).step_by(BURST as usize) {
            let burst: String = (start..start + BURST).map(|n| format!("{n}\n")).collect();
            let held_before = mirror.lines.len();
            let chunk = sb.append_str(&burst);
            // The point of the test: the eviction reaches past everything the
            // mirror held and into the lines this very append added.
            assert!(
                chunk.dropped as usize > held_before,
                "expected the eviction to overlap the new lines: dropped={} held={held_before}",
                chunk.dropped
            );
            mirror.apply(&chunk);
            mirror.assert_matches(&sb);
        }
        assert!(sb.snapshot().lines.len() < 3000, "the cap should have evicted");

        mirror.apply(&sb.clear());
        mirror.assert_matches(&sb);
        assert!(mirror.lines.is_empty(), "clear must empty the mirror too");
    }

    #[test]
    fn a_single_line_larger_than_the_cap_is_still_kept() {
        // MIN_LINES is the floor: we never evict our way down to nothing.
        let mut sb = Scrollback::new(8);
        sb.append_str(&format!("{}\n", "x".repeat(500)));
        sb.append_str("short\n");
        assert_eq!(sb.snapshot().lines.len(), MIN_LINES);
        assert!(sb.snapshot().lines.last().unwrap() == "short");
    }

    #[test]
    fn lowering_the_cap_trims_immediately() {
        let mut sb = Scrollback::new(1 << 20);
        for i in 0..100 {
            sb.append_str(&format!("line {i}\n"));
        }
        assert_eq!(sb.snapshot().lines.len(), 100);
        sb.set_limit(30);
        let after = sb.snapshot().lines;
        assert!(after.len() < 100, "expected a trim, kept {}", after.len());
        // Trimming drops the oldest, so the newest line must survive.
        assert_eq!(after.last().unwrap(), "line 99");
    }

    #[test]
    fn invalid_utf8_is_replaced_rather_than_dropping_the_line() {
        let mut sb = Scrollback::new(1 << 20);
        let chunk = sb.append(b"ok \xff\xfe bytes\n");
        assert_eq!(chunk.lines.len(), 1);
        assert!(chunk.lines[0].starts_with("ok "));
        assert!(chunk.lines[0].ends_with(" bytes"));
    }

    #[test]
    fn append_line_flushes_a_partial_line_first() {
        let mut sb = Scrollback::new(1 << 20);
        sb.append_str("no newline yet");
        sb.append_line("[exit 1]");
        assert_eq!(sb.text(), "no newline yet\n[exit 1]");
        assert_eq!(sb.snapshot().pending, "");
    }

    #[test]
    fn clear_empties_everything_and_reports_it_as_an_ordinary_delta() {
        let mut sb = Scrollback::new(1 << 20);
        sb.append_str("a\nb\npartial");
        let mut mirror = Mirror::default();
        mirror.lines = vec!["a".to_string(), "b".into()];
        mirror.pending = "partial".to_string();

        let chunk = sb.clear();
        assert_eq!(sb.snapshot().lines.len(), 0);
        assert_eq!(sb.snapshot().pending, "");

        // Applying the delta to a mirror must empty it too, with no special case.
        assert_eq!(chunk.dropped, 2);
        mirror.apply(&chunk);
        mirror.assert_matches(&sb);

        // And the byte accounting resets, so the next append is not over-trimmed.
        let chunk = sb.append_str("fresh\n");
        assert_eq!(chunk.dropped, 0);
    }
}
