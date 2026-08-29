//! Quick search — jump the cursor to a file by typing its name prefix (§5.9).
//!
//! A submodule of [`nav`](super) because this is cursor positioning: every hit
//! lands in [`super::set_cursor`], so the sliding-window invariant of §5.2 holds
//! here for free and a match below the fold scrolls into view like any other
//! cursor move.
//!
//! Two rules shape the whole module:
//!
//! - **The core owns the query.** A character that matches nothing is *rejected*
//!   rather than appended, and the caller is told via
//!   [`QuickSearch::miss_rev`] so it can beep. The string therefore always
//!   describes a real entry, and the user can never strand themselves in a dead
//!   query that only Backspace can escape.
//! - **Matching is prefix, not fuzzy**, and case-insensitive: typing `re` finds
//!   `README.md`. `..` is never a candidate — it is the one entry the user can
//!   already reach with Home.
//!
//! Opening and closing are explicit (Cmd+F, then Esc or Enter). Everything else
//! the user might press ends the search, but that is not decided here: it falls
//! out of [`crate::state::AppState::snapshot_after_input`], which every
//! user-initiated command returns through.

use crate::types::{Entry, PanelState, QuickSearch};

/// Open a fresh search box on the panel. The cursor does not move — the user has
/// only declared an intent to search, not yet said what for.
///
/// Re-opening over an already-open box starts over rather than resuming, which
/// is what a second Cmd+F reads as.
pub fn open(state: &mut PanelState) {
    state.search = Some(QuickSearch::default());
}

/// Close the box, leaving the cursor wherever the search put it (§5.9). Both
/// exits — Esc and Enter — land here; neither restores the pre-search cursor,
/// because the point of the search was to move it.
pub fn close(state: &mut PanelState) {
    state.search = None;
}

/// Append typed text to the query and move the cursor to the first match.
///
/// On a miss nothing is appended and the cursor stays put; only `miss_rev` moves,
/// which is the frontend's cue to beep. A no-op when no box is open.
pub fn push(state: &mut PanelState, text: &str) {
    let Some(query) = state.search.as_ref().map(|s| s.query.clone()) else {
        return;
    };
    let candidate = query + text;

    match find_match(&state.entries, &candidate) {
        Some(index) => {
            if let Some(search) = state.search.as_mut() {
                search.query = candidate;
            }
            super::set_cursor(state, index);
        }
        None => {
            if let Some(search) = state.search.as_mut() {
                // Saturating rather than wrapping only to be boring: a user
                // cannot press a key four billion times, and 0 is the "no miss
                // yet" value the frontend compares against.
                search.miss_rev = search.miss_rev.saturating_add(1);
            }
        }
    }
}

/// Drop the last character of the query and let the cursor follow it back to the
/// shorter prefix's first match.
///
/// Emptying the query leaves the box open and the cursor alone: the box is opened
/// and closed deliberately (§5.9), so backspacing past the start closing it would
/// be a surprise. A no-op when no box is open.
pub fn backspace(state: &mut PanelState) {
    let Some(mut query) = state.search.as_ref().map(|s| s.query.clone()) else {
        return;
    };
    // `String::pop` removes the last *char*, so a multi-byte name is safe here.
    if query.pop().is_none() {
        return;
    }

    let target = find_match(&state.entries, &query);
    if let Some(search) = state.search.as_mut() {
        search.query = query;
    }
    if let Some(index) = target {
        super::set_cursor(state, index);
    }
}

/// First entry whose name starts with `needle`, compared case-insensitively.
///
/// `None` for an empty needle — an empty query matches everything, and moving the
/// cursor to the first file the moment the user backspaces to nothing would be a
/// jump they did not ask for.
fn find_match(entries: &[Entry], needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let needle = needle.to_lowercase();
    entries
        .iter()
        .position(|e| super::is_selectable(e) && e.name.to_lowercase().starts_with(&needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EntryCategory, EntryKind, EntryMarker, PanelGeometry};

    fn ent(name: &str) -> Entry {
        Entry {
            name: name.to_string(),
            kind: EntryKind::File,
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
            category: EntryCategory::Plain,
            marker: EntryMarker::Ok,
            computed_size: None,
        }
    }

    /// A panel over `names`, searching, with a one-column viewport `rows` tall.
    fn searching(names: &[&str], rows: u16) -> PanelState {
        let mut state = PanelState {
            entries: names.iter().map(|n| ent(n)).collect(),
            geometry: PanelGeometry {
                columns: 1,
                rows_per_column: rows,
            },
            ..Default::default()
        };
        open(&mut state);
        state
    }

    fn type_in(state: &mut PanelState, text: &str) {
        for c in text.chars() {
            push(state, &c.to_string());
        }
    }

    fn query_of(state: &PanelState) -> &str {
        &state.search.as_ref().expect("box is open").query
    }

    fn misses(state: &PanelState) -> u32 {
        state.search.as_ref().expect("box is open").miss_rev
    }

    #[test]
    fn opening_does_not_move_the_cursor() {
        let mut state = searching(&["..", "alpha", "beta"], 10);
        state.cursor_index = 2;
        open(&mut state);
        assert_eq!(state.cursor_index, 2);
        assert_eq!(query_of(&state), "");
    }

    #[test]
    fn typing_a_prefix_jumps_to_the_first_match() {
        let mut state = searching(&["..", "alpha", "beta", "berry"], 10);
        type_in(&mut state, "be");
        assert_eq!(query_of(&state), "be");
        assert_eq!(state.entries[state.cursor_index].name, "beta");
    }

    /// Prefix, not substring: `erry` is inside `berry` but starts nothing.
    #[test]
    fn matching_is_a_prefix_not_a_substring() {
        let mut state = searching(&["..", "berry"], 10);
        push(&mut state, "e");
        assert_eq!(query_of(&state), "");
        assert_eq!(misses(&state), 1);
    }

    #[test]
    fn matching_ignores_case_in_both_directions() {
        let mut state = searching(&["..", "README.md", "src"], 10);
        type_in(&mut state, "re");
        assert_eq!(state.entries[state.cursor_index].name, "README.md");

        let mut state = searching(&["..", "readme.md"], 10);
        type_in(&mut state, "RE");
        assert_eq!(state.entries[state.cursor_index].name, "readme.md");
    }

    /// `..` is never a candidate — Home already goes there (§5.3's rule that it
    /// is not a real entry).
    #[test]
    fn parent_entry_is_never_matched() {
        let mut state = searching(&["..", "alpha"], 10);
        state.cursor_index = 1;
        push(&mut state, ".");
        assert_eq!(misses(&state), 1);
        assert_eq!(state.cursor_index, 1, "cursor must not jump to ..");
    }

    #[test]
    fn a_miss_rejects_the_character_and_leaves_the_cursor() {
        let mut state = searching(&["..", "alpha", "beta"], 10);
        type_in(&mut state, "be");
        let at_match = state.cursor_index;

        push(&mut state, "z");
        assert_eq!(query_of(&state), "be", "the rejected char must not land");
        assert_eq!(state.cursor_index, at_match);
        assert_eq!(misses(&state), 1);

        // Consecutive misses each have to be reported, or the second beep is lost.
        push(&mut state, "q");
        assert_eq!(misses(&state), 2);

        // A hit after a miss still works — the query was never poisoned.
        push(&mut state, "t");
        assert_eq!(query_of(&state), "bet");
    }

    #[test]
    fn backspace_walks_the_cursor_back() {
        let mut state = searching(&["..", "alpha", "beta", "berry"], 10);
        type_in(&mut state, "bet");
        assert_eq!(state.entries[state.cursor_index].name, "beta");

        backspace(&mut state);
        assert_eq!(query_of(&state), "be");
        assert_eq!(state.entries[state.cursor_index].name, "beta");
    }

    /// Emptying the query keeps the box open and the cursor where it is; only an
    /// explicit Esc/Enter closes it (§5.9).
    #[test]
    fn backspacing_to_empty_keeps_the_box_open() {
        let mut state = searching(&["..", "alpha", "beta"], 10);
        type_in(&mut state, "b");
        let at_match = state.cursor_index;

        backspace(&mut state);
        assert_eq!(query_of(&state), "");
        assert_eq!(state.cursor_index, at_match, "an empty query moves nothing");

        backspace(&mut state);
        assert!(state.search.is_some(), "backspace must not close the box");
    }

    #[test]
    fn multibyte_names_survive_backspace() {
        let mut state = searching(&["..", "日本語.txt", "naïve"], 10);
        type_in(&mut state, "日本");
        assert_eq!(state.entries[state.cursor_index].name, "日本語.txt");
        backspace(&mut state);
        assert_eq!(query_of(&state), "日");

        let mut state = searching(&["..", "naïve"], 10);
        type_in(&mut state, "naï");
        assert_eq!(state.entries[state.cursor_index].name, "naïve");
    }

    /// The reason this lives under `nav`: a hit goes through `set_cursor`, so the
    /// window scrolls to bring an off-screen match into view (§5.2).
    #[test]
    fn a_match_below_the_fold_scrolls_into_view() {
        let mut names: Vec<String> = vec!["..".to_string()];
        names.extend((0..40).map(|i| format!("file{i:02}")));
        names.push("zebra".to_string());
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();

        let mut state = searching(&refs, 10);
        assert_eq!(state.top_index, 0);

        type_in(&mut state, "z");
        assert_eq!(state.entries[state.cursor_index].name, "zebra");
        assert!(
            state.cursor_index >= state.top_index
                && state.cursor_index < state.top_index + 10,
            "cursor {} outside window [{}, {})",
            state.cursor_index,
            state.top_index,
            state.top_index + 10,
        );
    }

    #[test]
    fn closing_keeps_the_cursor_on_the_match() {
        let mut state = searching(&["..", "alpha", "beta"], 10);
        type_in(&mut state, "be");
        let at_match = state.cursor_index;

        close(&mut state);
        assert!(state.search.is_none());
        assert_eq!(state.cursor_index, at_match);
    }

    /// Every entry point is a no-op with no box open, so a stray call from the
    /// frontend cannot move the cursor behind the user's back.
    #[test]
    fn the_operations_are_inert_when_no_box_is_open() {
        let mut state = searching(&["..", "alpha", "beta"], 10);
        close(&mut state);
        state.cursor_index = 2;

        push(&mut state, "a");
        backspace(&mut state);
        assert!(state.search.is_none());
        assert_eq!(state.cursor_index, 2);
    }

    #[test]
    fn reopening_starts_a_fresh_query() {
        let mut state = searching(&["..", "alpha", "beta"], 10);
        type_in(&mut state, "bez");
        assert_eq!(misses(&state), 1);

        open(&mut state);
        assert_eq!(query_of(&state), "");
        assert_eq!(misses(&state), 0);
    }
}
