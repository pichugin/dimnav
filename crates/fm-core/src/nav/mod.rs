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
//! Everything is clamped to `[0, len-1]`. Scroll is derived (`cursor / page`), not
//! stored. This is pure, I/O-free, and unit-tested below.

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
        return;
    }
    let last = len - 1;
    let cur = state.cursor_index.min(last);

    // Effective geometry. A "column" of `rows` entries; a page of `cols*rows`.
    let rows = if state.geometry.rows_per_column == 0 {
        len
    } else {
        state.geometry.rows_per_column as usize
    };
    let cols = (state.geometry.columns as usize).max(1);
    let page = rows.saturating_mul(cols).max(1);

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
    state.cursor_index = next;
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
    state.selection.clear();
}

/// Move the cursor to an explicit index, clamped to `[0, len-1]` (0 when empty).
/// Backs mouse-click focus: the frontend reports the clicked entry's global index
/// and the core owns the resulting cursor position, same as [`move_cursor`].
pub fn set_cursor(state: &mut PanelState, index: usize) {
    let len = state.entries.len();
    state.cursor_index = if len == 0 { 0 } else { index.min(len - 1) };
}

/// Move the cursor onto the entry with the given name, if present. Used for the
/// "auto-position onto the folder just exited" rule when going to a parent (§5.2).
pub fn position_on(state: &mut PanelState, name: &str) {
    if let Some(i) = state.entries.iter().position(|e| e.name == name) {
        state.cursor_index = i;
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
    fn right_paginates_landing_same_row_leftmost_column() {
        // page = 6. From rightmost column (col1) row1 = index 4, Right -> next page
        // leftmost column same row = index 7.
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
}
