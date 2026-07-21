//! Navigation & selection — **the single cursor-index state machine** (SPEC §5.2).
//!
//! The whole Left/Right/Up/Down/Page/Home/End traversal is one function over one
//! flat `cursor_index`, because the frontend renders each page **column-major**:
//! a page holds `columns * rows_per_column` entries and cell `(col, row)` sits at
//! flat offset `col*rows_per_column + row`. Under that layout:
//!
//! - Down = `+1`, Up = `-1` (walks down a column, then rolls to the next column).
//! - Right = `+rows_per_column`, Left = `-rows_per_column` (one column over at the
//!   same row; at the rightmost column this rolls into the next page's leftmost
//!   column at the same row; clamping past the ends lands on the last file / `..`).
//! - PageDown/Up = `± (columns*rows_per_column)`; Home = 0; End = last.
//!
//! Everything is clamped to `[0, len-1]`.
//!
//! The **viewport is a sliding window**, not a page grid: `top_index` is stored
//! alongside the cursor and holds the first visible entry. Stepping off an edge
//! scrolls by the motion's own step — one entry for Up/Down, one column for
//! Left/Right, a full page for PageUp/Down — so the listing shifts smoothly the
//! way the orthodox managers do, instead of flipping a screenful at a time. The
//! window is kept inside `[0, len-page]` so no blank space shows below a long
//! listing. This is pure, I/O-free, and unit-tested below.

use std::path::Path;

use crate::types::{DirListing, Motion, PanelState};

/// Apply a cursor [`Motion`], updating `cursor_index` per the traversal above.
/// Uses `state.geometry`; when geometry is unset (`rows_per_column == 0`) the
/// listing is treated as a single column spanning every entry, so Down/Up still
/// work and Left/Right/Page jump to the ends.
pub fn move_cursor(state: &mut PanelState, motion: Motion) {
    let len = state.entries.len();
    if len == 0 {
        state.cursor_index = 0;
        state.top_index = 0;
        return;
    }
    let last = len - 1;
    let cur = state.cursor_index.min(last);
    let (rows, page) = geometry_of(state);

    let next = match motion {
        Motion::Up => cur.saturating_sub(1),
        Motion::Down => (cur + 1).min(last),
        Motion::Left => cur.saturating_sub(rows),
        Motion::Right => (cur + rows).min(last),
        Motion::PageUp => cur.saturating_sub(page),
        Motion::PageDown => (cur + page).min(last),
        Motion::Home => 0,
        Motion::End => last,
    };

    // Scroll by the motion's own step when the cursor leaves the window, so the
    // listing shifts by one entry / one column / one page rather than jumping to
    // a page boundary. Up/Down fall through to `top` and let `visible_top` do the
    // minimal one-entry adjustment.
    let max_top = len.saturating_sub(page);
    let top = state.top_index.min(max_top);
    let desired = match motion {
        Motion::Home => 0,
        Motion::End => max_top,
        Motion::PageDown => (top + page).min(max_top),
        Motion::PageUp => top.saturating_sub(page),
        Motion::Right if next >= top + page => (top + rows).min(max_top),
        Motion::Left if next < top => top.saturating_sub(rows),
        _ => top,
    };

    state.cursor_index = next;
    state.top_index = visible_top(desired, next, page, max_top);
}

/// Effective `(rows_per_column, page_size)` for a panel. Geometry unset
/// (`rows_per_column == 0`) means "one column spanning the whole listing", which
/// keeps Down/Up working before the frontend has reported a viewport.
fn geometry_of(state: &PanelState) -> (usize, usize) {
    let len = state.entries.len().max(1);
    let rows = if state.geometry.rows_per_column == 0 {
        len
    } else {
        state.geometry.rows_per_column as usize
    };
    let cols = (state.geometry.columns as usize).max(1);
    (rows, rows.saturating_mul(cols).max(1))
}

/// The smallest adjustment of `top` that keeps `cursor` inside the window and
/// the window inside the listing.
fn visible_top(top: usize, cursor: usize, page: usize, max_top: usize) -> usize {
    let lower = (cursor + 1).saturating_sub(page);
    top.clamp(lower, cursor).min(max_top)
}

/// Scroll the window as little as needed to bring the cursor back into view.
/// Every path that moves the cursor without a [`Motion`] — a click, the parent
/// auto-position, a re-listing — ends here, as does a geometry change.
pub fn clamp_scroll(state: &mut PanelState) {
    let len = state.entries.len();
    if len == 0 {
        state.top_index = 0;
        return;
    }
    let (_, page) = geometry_of(state);
    let cursor = state.cursor_index.min(len - 1);
    state.top_index = visible_top(state.top_index, cursor, page, len.saturating_sub(page));
}

/// Record the panel's rendered layout (SPEC §5.2) and re-clamp the scroll: a
/// resize or a view-mode change must never leave the cursor off-screen.
pub fn set_geometry(state: &mut PanelState, columns: u16, rows: u16) {
    state.geometry.columns = columns;
    state.geometry.rows_per_column = rows;
    clamp_scroll(state);
}

/// Replace a panel's listing: assign path/entries, re-apply the panel's sort,
/// reset the cursor to the top (`..`) and clear selection. Callers that need a
/// specific cursor (e.g. the parent auto-position rule) follow with
/// [`position_on`].
pub fn set_listing(state: &mut PanelState, listing: DirListing) {
    state.path = listing.path;
    state.entries = listing.entries;
    crate::fs::sort_entries(&mut state.entries, state.sort_mode);
    state.cursor_index = 0;
    state.top_index = 0;
    state.selection.clear();
}

/// Replace a panel's listing **for the same directory**, keeping the user's place:
/// the cursor stays on the entry it was on and the selection survives, both matched
/// by name rather than index (indices shift when the sort mode or the directory
/// contents change). Names that have disappeared are simply dropped.
///
/// This is what a refresh, a sort/hidden-file change, and a post-operation re-read
/// should use; [`set_listing`] (cursor to top, selection cleared) is for actually
/// changing directory.
pub fn set_listing_preserving(state: &mut PanelState, listing: DirListing) {
    let focused = state
        .entries
        .get(state.cursor_index)
        .map(|e| e.name.clone());
    let selected: Vec<String> = state
        .selection
        .iter()
        .filter_map(|&i| state.entries.get(i))
        .map(|e| e.name.clone())
        .collect();

    set_listing(state, listing);

    state.selection = state
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| is_selectable(e) && selected.contains(&e.name))
        .map(|(i, _)| i)
        .collect();
    if let Some(name) = focused {
        position_on(state, &name);
    }
}

/// Re-apply the panel's current sort mode to the entries already loaded, keeping
/// the cursor and selection on the same entries (§5.8). No I/O — switching sort
/// order never needs to re-read the directory.
pub fn resort(state: &mut PanelState) {
    // The entries are cloned rather than moved out: `set_listing_preserving` reads
    // the *current* cursor/selection names off the panel, so it must still see the
    // old listing when it is called.
    let listing = DirListing {
        path: state.path.clone(),
        entries: state.entries.clone(),
    };
    set_listing_preserving(state, listing);
}

/// Move the cursor to an explicit index, clamped to `[0, len-1]` (0 when empty).
/// Backs mouse-click focus: the frontend reports the clicked entry's global index
/// and the core owns the resulting cursor position, same as [`move_cursor`].
pub fn set_cursor(state: &mut PanelState, index: usize) {
    let len = state.entries.len();
    state.cursor_index = if len == 0 { 0 } else { index.min(len - 1) };
    clamp_scroll(state);
}

/// Move the cursor onto the entry with the given name, if present. Used for the
/// "auto-position onto the folder just exited" rule when going to a parent (§5.2).
pub fn position_on(state: &mut PanelState, name: &str) {
    if let Some(i) = state.entries.iter().position(|e| e.name == name) {
        state.cursor_index = i;
        clamp_scroll(state);
    }
}

/// The parent directory of `path`, or `None` at a filesystem root.
pub fn parent_of(path: &str) -> Option<String> {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
}

/// The final path component (the folder's own name), or `None` at a root.
pub fn child_name(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// Whether an entry can be selected. The synthetic `..` never is (§5.3).
fn is_selectable(entry: &crate::types::Entry) -> bool {
    entry.name != ".."
}

/// Toggle selection of the entry under the cursor, then advance the cursor to the
/// next entry (classic Norton Commander "Space to mark and step down"). Selection
/// is a separate layer from the cursor and persists as the cursor moves (§5.3).
/// On the last entry the cursor stays put (`Down` clamps). `..` is never
/// selectable, so toggling it is a no-op — and, having nothing to mark, does not
/// move the cursor either.
pub fn toggle_selection(state: &mut PanelState) {
    let idx = state.cursor_index;
    match state.entries.get(idx) {
        Some(e) if is_selectable(e) => {}
        _ => return,
    }
    if let Some(pos) = state.selection.iter().position(|&i| i == idx) {
        state.selection.remove(pos);
    } else {
        state.selection.push(idx);
    }
    move_cursor(state, Motion::Down);
}

/// Add the entry under the cursor to the selection (if selectable and not already
/// selected), then move the cursor. Backs the Shift+Arrow "select while moving"
/// gesture (§5.3) — additive only; deselection is Space or `deselect_all`.
pub fn select_and_move(state: &mut PanelState, motion: Motion) {
    let idx = state.cursor_index;
    if let Some(e) = state.entries.get(idx) {
        if is_selectable(e) && !state.selection.contains(&idx) {
            state.selection.push(idx);
        }
    }
    move_cursor(state, motion);
}

/// Select every selectable entry in the panel — all files/folders except `..`
/// (§5.3, the `*` action).
pub fn select_all(state: &mut PanelState) {
    state.selection = state
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| is_selectable(e))
        .map(|(i, _)| i)
        .collect();
}

/// Clear the selection (§5.3, the `-` action).
pub fn deselect_all(state: &mut PanelState) {
    state.selection.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Entry, EntryKind, EntryMarker, PanelGeometry, PanelState};

    fn ent(name: &str, kind: EntryKind) -> Entry {
        Entry {
            name: name.to_string(),
            kind,
            size: 0,
            modified: 0,
            permissions: 0,
            symlink_target: None,
            is_executable: false,
            marker: EntryMarker::Ok,
        }
    }

    /// Panel of `n` file entries named "0".."n-1", with the given geometry.
    fn panel(n: usize, columns: u16, rows: u16) -> PanelState {
        let entries = (0..n)
            .map(|i| ent(&i.to_string(), EntryKind::File))
            .collect();
        PanelState {
            entries,
            cursor_index: 0,
            geometry: PanelGeometry {
                columns,
                rows_per_column: rows,
            },
            ..Default::default()
        }
    }

    fn mv(p: &mut PanelState, m: Motion) -> usize {
        move_cursor(p, m);
        p.cursor_index
    }

    #[test]
    fn down_walks_column_then_wraps_to_next_column() {
        // 2 columns x 3 rows, 7 entries. Column-major: col0 = 0,1,2 ; col1 = 3,4,5.
        let mut p = panel(7, 2, 3);
        p.cursor_index = 2; // bottom of column 0
        assert_eq!(mv(&mut p, Motion::Down), 3); // top of column 1
    }

    #[test]
    fn right_moves_one_column_same_row() {
        let mut p = panel(7, 2, 3);
        p.cursor_index = 1; // col0, row1
        assert_eq!(mv(&mut p, Motion::Right), 4); // col1, row1
    }

    #[test]
    fn right_from_rightmost_column_steps_one_column_further() {
        // page = 6. From rightmost column (col1) row1 = index 4, Right -> index 7,
        // which the scrolled window renders in the rightmost column, same row.
        let mut p = panel(13, 2, 3);
        p.cursor_index = 4;
        assert_eq!(mv(&mut p, Motion::Right), 7);
    }

    #[test]
    fn right_past_end_lands_on_last_file() {
        let mut p = panel(7, 2, 3);
        p.cursor_index = 5; // last full-column cell; +rows(3) = 8 > last(6)
        assert_eq!(mv(&mut p, Motion::Right), 6);
    }

    #[test]
    fn left_from_leftmost_paginates_up_same_row() {
        let mut p = panel(13, 2, 3);
        p.cursor_index = 7; // page1, col0, row1
        assert_eq!(mv(&mut p, Motion::Left), 4); // page0, col1, row1
    }

    #[test]
    fn left_past_start_lands_on_first_entry() {
        let mut p = panel(7, 2, 3);
        p.cursor_index = 1;
        assert_eq!(mv(&mut p, Motion::Left), 0); // '..' would be entry 0
    }

    #[test]
    fn up_and_down_clamp_at_ends() {
        let mut p = panel(7, 2, 3);
        p.cursor_index = 0;
        assert_eq!(mv(&mut p, Motion::Up), 0);
        p.cursor_index = 6;
        assert_eq!(mv(&mut p, Motion::Down), 6);
    }

    #[test]
    fn page_and_home_end() {
        let mut p = panel(20, 2, 3); // page = 6
        p.cursor_index = 1;
        assert_eq!(mv(&mut p, Motion::PageDown), 7);
        assert_eq!(mv(&mut p, Motion::PageUp), 1);
        assert_eq!(mv(&mut p, Motion::End), 19);
        assert_eq!(mv(&mut p, Motion::Home), 0);
    }

    #[test]
    fn without_geometry_down_still_works_and_right_jumps_to_end() {
        let mut p = panel(7, 0, 0); // geometry unset
        p.cursor_index = 0;
        assert_eq!(mv(&mut p, Motion::Down), 1);
        assert_eq!(mv(&mut p, Motion::Right), 6); // spans whole list -> last
    }

    // --- Sliding-window scrolling (§5.2) --------------------------------------
    // All of these use a 2x3 grid over 13 entries: page = 6, max_top = 7.

    fn scroll(p: &mut PanelState, m: Motion) -> (usize, usize) {
        move_cursor(p, m);
        (p.cursor_index, p.top_index)
    }

    #[test]
    fn down_at_the_bottom_scrolls_one_entry() {
        let mut p = panel(13, 2, 3);
        p.cursor_index = 5; // bottom of the rightmost column, window 0..=5
        // The window slides by exactly one: entry 3 moves from the top of the
        // right column to the bottom of the left one, entry 0 scrolls off.
        assert_eq!(scroll(&mut p, Motion::Down), (6, 1));
        assert_eq!(scroll(&mut p, Motion::Down), (7, 2));
    }

    #[test]
    fn up_at_the_top_scrolls_one_entry_and_otherwise_stays_put() {
        let mut p = panel(13, 2, 3);
        p.cursor_index = 6;
        p.top_index = 1; // window 1..=6
        // Still inside the window — nothing scrolls.
        assert_eq!(scroll(&mut p, Motion::Up), (5, 1));
        p.cursor_index = 1;
        assert_eq!(scroll(&mut p, Motion::Up), (0, 0)); // off the top edge
    }

    #[test]
    fn right_at_the_edge_scrolls_one_column_left_mirrors_it() {
        let mut p = panel(13, 2, 3);
        p.cursor_index = 5; // rightmost column, window 0..=5
        assert_eq!(scroll(&mut p, Motion::Right), (8, 3)); // window 3..=8
        // Back the same way: index 5 is still visible, so nothing scrolls yet.
        assert_eq!(scroll(&mut p, Motion::Left), (5, 3));
        assert_eq!(scroll(&mut p, Motion::Left), (2, 0)); // off the left edge
    }

    #[test]
    fn page_motions_scroll_a_full_page_keeping_the_cursor_in_place() {
        let mut p = panel(13, 2, 3);
        p.cursor_index = 1;
        assert_eq!(scroll(&mut p, Motion::PageDown), (7, 6)); // same cell, next page
        assert_eq!(scroll(&mut p, Motion::PageUp), (1, 0));
    }

    #[test]
    fn home_and_end_jump_to_the_first_and_last_window() {
        let mut p = panel(13, 2, 3);
        assert_eq!(scroll(&mut p, Motion::End), (12, 7)); // len - page
        assert_eq!(scroll(&mut p, Motion::Home), (0, 0));
    }

    #[test]
    fn a_listing_shorter_than_a_page_never_scrolls() {
        let mut p = panel(4, 2, 3); // page = 6 > len
        for m in [Motion::Down, Motion::Right, Motion::PageDown, Motion::End] {
            move_cursor(&mut p, m);
            assert_eq!(p.top_index, 0, "{m:?} should not scroll a short listing");
        }
    }

    #[test]
    fn set_cursor_and_set_geometry_pull_the_window_onto_the_cursor() {
        let mut p = panel(13, 2, 3);
        set_cursor(&mut p, 12); // a click far below the window
        assert_eq!(p.top_index, 7);
        // Shrinking the panel to 2 rows (page = 4) leaves the cursor off-screen
        // until the geometry change re-clamps the window.
        set_geometry(&mut p, 2, 2);
        assert_eq!(p.top_index, 9);
        assert_eq!(p.cursor_index, 12);
    }

    #[test]
    fn set_listing_resets_the_window_and_position_on_scrolls_to_the_entry() {
        let mut p = panel(13, 2, 3);
        p.cursor_index = 12;
        p.top_index = 7;
        let mut entries: Vec<Entry> = vec![ent("..", EntryKind::Dir)];
        entries.extend((0..12).map(|i| ent(&i.to_string(), EntryKind::File)));
        set_listing(
            &mut p,
            DirListing {
                path: "/dir".into(),
                entries,
            },
        );
        assert_eq!((p.cursor_index, p.top_index), (0, 0));
        // The "auto-position onto the folder just exited" rule must bring the
        // window along when that folder sits below the first screen.
        // (names sort lexicographically, so "9" is the last of "0".."11")
        position_on(&mut p, "9");
        assert_eq!(p.cursor_index, 12);
        assert_eq!(p.top_index, 7);
    }

    #[test]
    fn set_listing_resets_cursor_and_position_on_finds_entry() {
        let mut p = panel(1, 2, 3);
        p.cursor_index = 5; // stale
        let listing = DirListing {
            path: "/tmp/parent".into(),
            entries: vec![
                ent("..", EntryKind::Dir),
                ent("child", EntryKind::Dir),
                ent("zeta.txt", EntryKind::File),
            ],
        };
        set_listing(&mut p, listing);
        assert_eq!(p.cursor_index, 0); // reset to '..'
        assert_eq!(p.path, "/tmp/parent");
        // Auto-position onto the folder just exited.
        position_on(&mut p, "child");
        assert_eq!(p.entries[p.cursor_index].name, "child");
    }

    #[test]
    fn parent_and_child_helpers() {
        assert_eq!(parent_of("/a/b/c").as_deref(), Some("/a/b"));
        assert_eq!(child_name("/a/b/c").as_deref(), Some("c"));
        assert_eq!(parent_of("/"), None);
    }

    /// Panel whose entry 0 is `..`, followed by `n` files.
    fn panel_with_dotdot(n: usize) -> PanelState {
        let mut entries = vec![ent("..", EntryKind::Dir)];
        entries.extend((0..n).map(|i| ent(&i.to_string(), EntryKind::File)));
        PanelState {
            entries,
            ..Default::default()
        }
    }

    #[test]
    fn toggle_selection_adds_then_removes_and_skips_dotdot() {
        let mut p = panel_with_dotdot(3); // indices 0(..),1,2,3
        // Cursor on '..' -> no-op, and the cursor does not advance.
        p.cursor_index = 0;
        toggle_selection(&mut p);
        assert!(p.selection.is_empty());
        assert_eq!(p.cursor_index, 0);
        // Cursor on a real file -> select and step down (§5.3, NC behavior).
        p.cursor_index = 2;
        toggle_selection(&mut p);
        assert_eq!(p.selection, vec![2]);
        assert_eq!(p.cursor_index, 3);
        // Step back onto it and toggle off; the cursor advances again.
        p.cursor_index = 2;
        toggle_selection(&mut p);
        assert!(p.selection.is_empty());
        assert_eq!(p.cursor_index, 3);
    }

    #[test]
    fn toggle_selection_on_last_entry_keeps_cursor() {
        let mut p = panel_with_dotdot(3); // indices 0(..),1,2,3; last = 3
        p.cursor_index = 3;
        toggle_selection(&mut p);
        assert_eq!(p.selection, vec![3]); // still marked
        assert_eq!(p.cursor_index, 3); // Down clamps at the last entry
    }

    #[test]
    fn set_cursor_clamps_in_range_past_end_and_empty() {
        let mut p = panel_with_dotdot(3); // len 4, last = 3
        set_cursor(&mut p, 2);
        assert_eq!(p.cursor_index, 2);
        set_cursor(&mut p, 99);
        assert_eq!(p.cursor_index, 3); // clamped to last
        let mut empty = panel(0, 2, 3);
        set_cursor(&mut empty, 5);
        assert_eq!(empty.cursor_index, 0); // empty -> 0
    }

    #[test]
    fn select_all_excludes_dotdot_and_deselect_clears() {
        let mut p = panel_with_dotdot(3); // indices 0(..),1,2,3
        select_all(&mut p);
        assert_eq!(p.selection, vec![1, 2, 3]);
        deselect_all(&mut p);
        assert!(p.selection.is_empty());
    }

    #[test]
    fn select_and_move_adds_current_then_advances_idempotently() {
        let mut p = panel_with_dotdot(3);
        p.geometry = PanelGeometry {
            columns: 1,
            rows_per_column: 10,
        };
        p.cursor_index = 1;
        select_and_move(&mut p, Motion::Down);
        assert_eq!(p.selection, vec![1]);
        assert_eq!(p.cursor_index, 2);
        // Re-selecting an already-selected index does not duplicate it.
        p.cursor_index = 1;
        select_and_move(&mut p, Motion::Down);
        assert_eq!(p.selection, vec![1]);
    }

    #[test]
    fn selection_survives_cursor_movement() {
        let mut p = panel_with_dotdot(5);
        p.cursor_index = 2;
        toggle_selection(&mut p);
        move_cursor(&mut p, Motion::Down);
        move_cursor(&mut p, Motion::Down);
        assert_eq!(p.selection, vec![2]); // unchanged by navigation
    }

    /// A panel of named files in a fixed directory, sorted by name (the default).
    fn named_panel(names: &[&str]) -> PanelState {
        let mut p = PanelState {
            path: "/dir".to_string(),
            ..Default::default()
        };
        set_listing(
            &mut p,
            DirListing {
                path: "/dir".to_string(),
                entries: names
                    .iter()
                    .map(|n| ent(n, EntryKind::File))
                    .collect(),
            },
        );
        p
    }

    #[test]
    fn resort_keeps_cursor_and_selection_on_the_same_entries() {
        let mut p = named_panel(&["a.txt", "b.txt", "c.txt"]);
        // Give them distinct sizes so Size order is the reverse of name order.
        for (i, e) in p.entries.iter_mut().enumerate() {
            e.size = (i + 1) as u64;
        }
        p.cursor_index = 0; // a.txt (smallest)
        p.selection = vec![0, 2]; // a.txt, c.txt

        p.sort_mode = crate::types::SortMode::Size;
        resort(&mut p);

        // Size sorts largest first: c, b, a.
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["c.txt", "b.txt", "a.txt"]);
        assert_eq!(p.entries[p.cursor_index].name, "a.txt"); // cursor followed
        let mut sel: Vec<&str> = p.selection.iter().map(|&i| p.entries[i].name.as_str()).collect();
        sel.sort_unstable();
        assert_eq!(sel, ["a.txt", "c.txt"]); // selection followed
    }

    #[test]
    fn preserving_relist_drops_names_that_vanished() {
        let mut p = named_panel(&["a.txt", "b.txt", "c.txt"]);
        p.cursor_index = 2; // c.txt
        p.selection = vec![0, 1, 2];

        // b.txt and c.txt are gone; d.txt appeared.
        set_listing_preserving(
            &mut p,
            DirListing {
                path: "/dir".to_string(),
                entries: vec![
                    ent("a.txt", EntryKind::File),
                    ent("d.txt", EntryKind::File),
                ],
            },
        );

        // Only the survivor stays selected; the cursor falls back to the top since
        // the entry it was on no longer exists.
        assert_eq!(p.selection, vec![0]);
        assert_eq!(p.entries[p.selection[0]].name, "a.txt");
        assert_eq!(p.cursor_index, 0);
    }

    #[test]
    fn preserving_relist_never_selects_dotdot() {
        let mut p = panel_with_dotdot(3);
        p.selection = vec![1];
        let same = DirListing {
            path: p.path.clone(),
            entries: p.entries.clone(),
        };
        set_listing_preserving(&mut p, same);
        assert_eq!(p.selection, vec![1]); // the real entry, never `..` at index 0
    }
}
