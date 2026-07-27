//! Command history for the terminal prompt (SPEC §5.7).
//!
//! Up/Down recall previously run commands, and the list survives restarts — it
//! lives next to `config.toml` in the OS config directory as a plain
//! newline-delimited file, so it is readable and editable like every other piece
//! of this app's configuration (§7).
//!
//! The recall cursor follows shell convention: Up walks backwards through the
//! list, Down walks forward, and stepping past the newest entry restores the
//! line the user was typing before they started recalling — that draft is never
//! lost to an accidental Up.

use std::path::{Path, PathBuf};

const FILE_NAME: &str = "history";

/// How many commands to remember. Bounded so the file cannot grow without limit
/// across a long-lived install.
const MAX_ENTRIES: usize = 500;

/// Recallable command list plus the cursor Up/Down walks.
///
/// `Clone` is cheap and deliberate: capped at [`MAX_ENTRIES`] short strings, it
/// lets the adapter copy the list out from under the state lock and write it to
/// disk on a background thread.
#[derive(Debug, Default, Clone)]
pub struct History {
    /// Oldest first; the most recent command is last.
    entries: Vec<String>,
    /// How many steps back from the end the cursor sits. `0` means "not
    /// recalling — showing the draft".
    offset: usize,
    /// What the user had typed before recall started, restored on stepping back
    /// past the newest entry.
    draft: String,
}

impl History {
    pub fn from_entries(entries: Vec<String>) -> Self {
        let mut h = History::default();
        for e in entries {
            h.push(e);
        }
        h
    }

    /// Record a command that was just run and reset the recall cursor.
    ///
    /// Consecutive duplicates are collapsed (re-running the same command should
    /// not make Up press twice), and the list is capped at [`MAX_ENTRIES`].
    /// Blank input is not history.
    pub fn push(&mut self, command: String) {
        self.reset();
        if command.trim().is_empty() {
            return;
        }
        if self.entries.last() == Some(&command) {
            return;
        }
        self.entries.push(command);
        if self.entries.len() > MAX_ENTRIES {
            let excess = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(0..excess);
        }
    }

    /// Step backwards (Up). Returns the recalled line, or `None` at the oldest
    /// entry — the caller then leaves the prompt untouched.
    ///
    /// `current` is what is in the prompt right now; on the first Up it becomes
    /// the saved draft.
    pub fn prev(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() || self.offset >= self.entries.len() {
            return None;
        }
        if self.offset == 0 {
            self.draft = current.to_string();
        }
        self.offset += 1;
        Some(self.entries[self.entries.len() - self.offset].clone())
    }

    /// Step forwards (Down). Past the newest entry this returns the saved draft
    /// once, then `None`.
    // Named for its pair with `prev`, which is what a shell history reads like.
    // Clippy suggests implementing Iterator instead, but this is a cursor the
    // user drives in both directions, not a one-way sequence — and it is stateful
    // in a way Iterator's contract does not describe.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<String> {
        match self.offset {
            0 => None,
            1 => {
                self.offset = 0;
                Some(std::mem::take(&mut self.draft))
            }
            _ => {
                self.offset -= 1;
                Some(self.entries[self.entries.len() - self.offset].clone())
            }
        }
    }

    /// Abandon recall (typing, or running a command) without changing the list.
    pub fn reset(&mut self) {
        self.offset = 0;
        self.draft.clear();
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }
}

/// Absolute path of the history file, beside `config.toml` (§7). `None` only
/// when the OS config directory cannot be determined — history then works for
/// the session and simply is not persisted.
pub fn history_path() -> Option<PathBuf> {
    crate::config::config_path().map(|p| p.with_file_name(FILE_NAME))
}

/// Load the persisted history, or an empty list. Never fails: an unreadable
/// history file must not stop the app from starting, exactly like the config.
pub fn load() -> History {
    // History lives inside the config directory, so it has to be read *after*
    // any pending rename of that directory — and nothing guarantees the config
    // is loaded first. Idempotent, so calling it here costs nothing.
    crate::config::ensure_migrated();
    history_path()
        .map(|p| load_from(&p))
        .unwrap_or_default()
}

/// Persist the history. Errors are swallowed by design — a failed history write
/// must never break the command that triggered it.
pub fn save(history: &History) {
    if let Some(path) = history_path() {
        let _ = save_to(&path, history);
    }
}

/// [`load`] against an explicit path (the testable half).
pub fn load_from(path: &Path) -> History {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    History::from_entries(
        text.lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// [`save`] against an explicit path (the testable half). Same temp-file-plus-
/// rename dance as the config, so an interrupted write cannot truncate history.
pub fn save_to(path: &Path, history: &History) -> Result<(), String> {
    let mut text = history.entries().join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("could not create config dir: {e}"))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("could not write history: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("could not replace history: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        crate::testutil::unique_dir("fm_core_hist").join(FILE_NAME)
    }

    fn history(cmds: &[&str]) -> History {
        History::from_entries(cmds.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn up_walks_backwards_and_stops_at_the_oldest() {
        let mut h = history(&["one", "two", "three"]);
        assert_eq!(h.prev("").as_deref(), Some("three"));
        assert_eq!(h.prev("").as_deref(), Some("two"));
        assert_eq!(h.prev("").as_deref(), Some("one"));
        // Nothing older — the prompt keeps showing "one".
        assert_eq!(h.prev(""), None);
    }

    #[test]
    fn down_returns_the_draft_the_user_was_typing() {
        let mut h = history(&["one", "two"]);
        assert_eq!(h.prev("half-typed").as_deref(), Some("two"));
        assert_eq!(h.prev("ignored").as_deref(), Some("one"));
        assert_eq!(h.next().as_deref(), Some("two"));
        assert_eq!(h.next().as_deref(), Some("half-typed"));
        assert_eq!(h.next(), None);
    }

    #[test]
    fn down_alone_does_nothing() {
        let mut h = history(&["one"]);
        assert_eq!(h.next(), None);
    }

    #[test]
    fn consecutive_duplicates_and_blanks_are_not_recorded() {
        let mut h = History::default();
        h.push("ls".into());
        h.push("ls".into());
        h.push("   ".into());
        h.push("pwd".into());
        h.push("ls".into()); // not consecutive — kept
        assert_eq!(h.entries(), ["ls", "pwd", "ls"]);
    }

    #[test]
    fn the_list_is_capped() {
        let mut h = History::default();
        for i in 0..MAX_ENTRIES + 50 {
            h.push(format!("cmd {i}"));
        }
        assert_eq!(h.entries().len(), MAX_ENTRIES);
        // The oldest are the ones dropped.
        assert_eq!(h.entries()[0], format!("cmd {}", 50));
    }

    #[test]
    fn running_a_command_cancels_recall() {
        let mut h = history(&["one", "two"]);
        h.prev("draft");
        h.push("three".into());
        // The cursor is back at the bottom, so Up starts from the newest again.
        assert_eq!(h.prev("").as_deref(), Some("three"));
    }

    #[test]
    fn round_trips_through_the_history_file() {
        let path = temp_path();
        let h = history(&["make build", "echo 'hi there'", "cd /tmp"]);
        save_to(&path, &h).unwrap();

        let back = load_from(&path);
        assert_eq!(back.entries(), ["make build", "echo 'hi there'", "cd /tmp"]);

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_missing_history_file_is_an_empty_history() {
        let h = load_from(Path::new("/definitely/not/a/real/path/history"));
        assert!(h.entries().is_empty());
    }
}
