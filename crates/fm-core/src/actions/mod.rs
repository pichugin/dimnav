//! The action catalog — what every bindable action *means* (SPEC §6/§6a).
//!
//! [`crate::config::default_keymap`] says which keys reach which action id; this
//! module says what that action id does in words a user can read. The two are
//! deliberately separate: a shortcut *schema* swaps the keymap, while the set of
//! things the app can do stays put, so the F1 help screen can render "the
//! currently applied shortcuts" for any schema without a second table to
//! maintain.
//!
//! Descriptions live in the core rather than the webview for the usual reason
//! (CLAUDE.md): the frontend is a thin, swappable renderer, and a future Iced
//! frontend must get the same help text without reimplementing it.
//!
//! This is also the precursor to the [`crate::plugin::Command`] registry (§6a —
//! "register commands, bindable to keys, appearing in a command palette"), whose
//! `id()` / `title()` shape it mirrors. When that registry lands, this table
//! becomes its in-tree contributor rather than a parallel structure.
//!
//! Declaration order is display order, in both dimensions: categories appear in
//! the order they are listed here, and actions in the order they appear within
//! their category.

/// One group of related actions. The `title` is rendered as a sub-heading inside
/// whichever keyboard context the group's actions turned out to be bound in, so
/// it reads as a heading ("Cursor motion"), not as a sentence.
#[derive(Debug, Clone, Copy)]
pub struct ActionCategory {
    /// Stable id, e.g. `"cursor"`. Not shown to the user, but searchable.
    pub id: &'static str,
    pub title: &'static str,
    pub actions: &'static [ActionInfo],
}

/// One bindable action. `id` is the same string a [`crate::types::KeyBinding`]
/// carries in its `action` field — that equality is what
/// [`tests::catalog_covers_the_default_keymap`] enforces.
#[derive(Debug, Clone, Copy)]
pub struct ActionInfo {
    pub id: &'static str,
    /// Short imperative label, e.g. `"Move cursor down"`.
    pub title: &'static str,
    /// One extra clause of context, or `""` when the title already says it all.
    pub description: &'static str,
}

/// Shorthand so the table below stays readable.
const fn a(id: &'static str, title: &'static str, description: &'static str) -> ActionInfo {
    ActionInfo { id, title, description }
}

const CURSOR: &[ActionInfo] = &[
    a("cursor.up", "Move cursor up", "Steps up one entry, wrapping to the previous column at the top."),
    a("cursor.down", "Move cursor down", "Steps down one entry, wrapping to the next column at the bottom."),
    a("cursor.left", "Move cursor left", "Steps a column left, then paginates up to the first entry (§5.2)."),
    a("cursor.right", "Move cursor right", "Steps a column right, then paginates down to the last file (§5.2)."),
    a("cursor.page_up", "Page up", ""),
    a("cursor.page_down", "Page down", ""),
    a("cursor.home", "Go to first entry", "Jumps to `..` at the top of the listing."),
    a("cursor.end", "Go to last entry", ""),
];

const NAVIGATION: &[ActionInfo] = &[
    a("panel.switch", "Switch panel", "Moves the keyboard between the left and right panel."),
    a("nav.enter", "Open entry", "Enters a folder, or opens a file with its configured handler. On `..` it goes to the parent and puts the cursor on the folder just left."),
    a("nav.parent", "Go to parent folder", "Same as Enter on `..` — the cursor lands on the folder just left."),
    a("panel.equalize", "Show this folder on the other panel", "Opens the active panel's folder on the opposite panel too. The keyboard stays where it is."),
];

/// The eight Shift+motion sweeps differ only in direction, so they share one
/// description rather than eight near-identical ones (§5.3).
const SWEEP: &str = "Flips the selection of every entry swept over, but not the one it lands on.";

const SELECTION: &[ActionInfo] = &[
    a("selection.toggle", "Toggle selection", "Selects or deselects the entry under the cursor and steps down."),
    a("selection.all", "Select all", ""),
    a("selection.none", "Deselect all", ""),
    a("select.up", "Toggle selection up", SWEEP),
    a("select.down", "Toggle selection down", SWEEP),
    a("select.left", "Toggle selection left", SWEEP),
    a("select.right", "Toggle selection right", SWEEP),
    a("select.page_up", "Toggle selection a page up", SWEEP),
    a("select.page_down", "Toggle selection a page down", SWEEP),
    a("select.home", "Toggle selection to the first entry", SWEEP),
    a("select.end", "Toggle selection to the last entry", SWEEP),
];

const FILE_OPS: &[ActionInfo] = &[
    a("op.copy", "Copy", "Opens the editable destination prompt; accepts `..`, relative and absolute paths (§5.4a)."),
    a("op.move", "Move", "Opens the same editable destination prompt."),
    a("op.rename", "Rename", "Renames in place, with the basename preselected."),
    a("op.mkdir", "Create directory", ""),
    a("op.delete", "Delete", "Confirms first, with the persisted \"Move to Trash\" checkbox (§5.4a)."),
];

const OPEN: &[ActionInfo] = &[
    a("open.view", "View file", "Read-only. On a folder it calculates the recursive size instead (§5.4)."),
    a("open.edit", "Edit file", "Opens the configured editor — embedded or external (§5.5)."),
];

const PANEL_VIEW: &[ActionInfo] = &[
    a("panel.refresh", "Refresh listing", "Re-reads the directory, keeping the cursor and selection. Directories are watched and refresh themselves, so this is for volumes that cannot be watched (§5.6)."),
    a("panel.toggle_hidden", "Toggle hidden files", "Per panel, persisted across restarts (§5.8)."),
    a("panel.cycle_sort", "Cycle sort mode", "Folders-first-by-name, type+name, size, date."),
    a("panel.view_1", "One-column view", ""),
    a("panel.view_2", "Two-column view", "The default (§5.8)."),
    a("panel.view_3", "Three-column view", ""),
    a("panel.view_detailed", "Detailed view", "Single column with size, date and attributes."),
];

const QUICK_SEARCH: &[ActionInfo] = &[
    a("search.start", "Quick search", "Opens a search box in the active panel's corner; what you type jumps the cursor to the first name that starts with it."),
];

/// Split from [`QUICK_SEARCH`] the way `TERMINAL_PROMPT` is split from
/// `TERMINAL`: these are bound in the box's own context, so they head their own
/// group rather than repeating the section title above them.
const QUICK_SEARCH_BOX: &[ActionInfo] = &[
    a("search.close", "Close the search box", "Leaves the cursor on the match. Enter does not open the entry — press it again for that."),
    a("search.backspace", "Erase a character", "Steps the query back one character, and the cursor with it."),
];

const TERMINAL: &[ActionInfo] = &[
    a("terminal.focus", "Focus command line", "Hands the keyboard to the prompt below the panels."),
    a("terminal.blur", "Leave command line", "Hands the keyboard back to the active panel; partial input survives."),
    a("terminal.toggle_half", "Toggle output pane", "Expands the terminal to the bottom half of the window; the panels shrink rather than being covered."),
    a("terminal.curtain", "Toggle terminal curtain", "Slides the panels aside for a full-height terminal, and back (§6)."),
    a("terminal.insert_name", "Insert name at prompt", "Appends the name under the cursor, shell-quoted, without leaving the panel (§5.7)."),
];

const TERMINAL_PROMPT: &[ActionInfo] = &[
    a("terminal.run", "Run command", ""),
    a("terminal.interrupt", "Interrupt or clear", "Signals the running command; with nothing running it clears the prompt."),
    a("terminal.history_prev", "Previous command", ""),
    a("terminal.history_next", "Next command", ""),
    a("terminal.scroll_up", "Scroll output up", ""),
    a("terminal.scroll_down", "Scroll output down", ""),
    a("terminal.clear_buffer", "Clear output", "Empties the scrollback buffer, like `clear`."),
];

const VIEWER_MOTION: &[ActionInfo] = &[
    a("viewer.line_up", "Line up", ""),
    a("viewer.line_down", "Line down", ""),
    a("viewer.col_left", "Column left", "Only with wrapping off."),
    a("viewer.col_right", "Column right", "Only with wrapping off."),
    a("viewer.page_up", "Page up", ""),
    a("viewer.page_down", "Page down", ""),
    a("viewer.home", "Go to start of file", ""),
    a("viewer.end", "Go to end of file", ""),
];

const VIEWER_CMD: &[ActionInfo] = &[
    a("viewer.close", "Close viewer", ""),
    a("viewer.toggle_wrap", "Toggle line wrap", "Persisted across sessions (§7)."),
    a("viewer.toggle_hex", "Toggle hex mode", ""),
    a("viewer.goto", "Go to position", "Accepts a line number, a `0x` offset, or a percentage."),
    a("viewer.to_edit", "Switch to editor", "Reopens the same file read-write at the same position."),
    a("viewer.search", "Search", ""),
    a("viewer.search_next", "Search again", "Repeats the last search without re-prompting."),
];

const EDITOR_CMD: &[ActionInfo] = &[
    a("editor.save", "Save", "Atomic write that preserves the file's encoding, line endings and permissions."),
    a("editor.to_view", "Switch to viewer", "Reopens the same file read-only."),
    a("editor.close", "Close editor", "Prompts when the buffer has unsaved changes."),
];

const HELP: &[ActionInfo] = &[
    a("help.open", "Open help", "Available from every surface."),
    a("help.close", "Close help", ""),
    a("help.next_topic", "Next topic", "Cycles round to the first topic past the last."),
    a("help.prev_topic", "Previous topic", "Cycles round to the last topic before the first."),
    a("help.scroll_up", "Scroll up", ""),
    a("help.scroll_down", "Scroll down", ""),
    a("help.page_up", "Page up", ""),
    a("help.page_down", "Page down", ""),
];

const CATALOG: &[ActionCategory] = &[
    ActionCategory { id: "cursor", title: "Cursor motion", actions: CURSOR },
    ActionCategory { id: "navigation", title: "Directory navigation", actions: NAVIGATION },
    ActionCategory { id: "selection", title: "Selection", actions: SELECTION },
    ActionCategory { id: "file_ops", title: "File operations", actions: FILE_OPS },
    ActionCategory { id: "open", title: "View & edit", actions: OPEN },
    ActionCategory { id: "panel_view", title: "Layout & sorting", actions: PANEL_VIEW },
    ActionCategory { id: "quick_search", title: "Quick search", actions: QUICK_SEARCH },
    ActionCategory { id: "quick_search_box", title: "In the search box", actions: QUICK_SEARCH_BOX },
    ActionCategory { id: "terminal", title: "Terminal", actions: TERMINAL },
    ActionCategory { id: "terminal_prompt", title: "At the prompt", actions: TERMINAL_PROMPT },
    ActionCategory { id: "viewer_motion", title: "Scrolling", actions: VIEWER_MOTION },
    ActionCategory { id: "viewer_cmd", title: "Commands", actions: VIEWER_CMD },
    ActionCategory { id: "editor_cmd", title: "Commands", actions: EDITOR_CMD },
    ActionCategory { id: "help", title: "Help", actions: HELP },
];

/// Every action the app knows about, grouped and in display order.
pub fn catalog() -> &'static [ActionCategory] {
    CATALOG
}

/// Look up one action by its id, with the category it belongs to. `None` for an
/// unknown id — callers decide whether that is a bug or just an action with no
/// help text yet.
pub fn find(action_id: &str) -> Option<(&'static ActionCategory, &'static ActionInfo)> {
    CATALOG.iter().find_map(|cat| {
        cat.actions
            .iter()
            .find(|info| info.id == action_id)
            .map(|info| (cat, info))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_keymap;
    use std::collections::BTreeSet;

    /// The anti-drift guard. The F1 help screen renders the keymap through this
    /// catalog, so an action bound without help text would silently vanish from
    /// help, and a catalog entry for an action nobody binds is dead text. Both
    /// directions must hold — if this fails, add the missing entry rather than
    /// relaxing the assertion.
    #[test]
    fn catalog_covers_the_default_keymap() {
        let bound: BTreeSet<String> = default_keymap().into_iter().map(|b| b.action).collect();
        let described: BTreeSet<String> = CATALOG
            .iter()
            .flat_map(|c| c.actions.iter().map(|a| a.id.to_string()))
            .collect();

        let undescribed: Vec<_> = bound.difference(&described).collect();
        assert!(undescribed.is_empty(), "bound but missing from the catalog: {undescribed:?}");

        let unbound: Vec<_> = described.difference(&bound).collect();
        assert!(unbound.is_empty(), "in the catalog but bound to no key: {unbound:?}");
    }

    /// Ids must be unique, or `find` would silently shadow one with another.
    #[test]
    fn action_ids_are_unique() {
        let mut seen = BTreeSet::new();
        for cat in CATALOG {
            for info in cat.actions {
                assert!(seen.insert(info.id), "duplicate action id: {}", info.id);
            }
        }
    }

    #[test]
    fn find_returns_the_owning_category() {
        let (cat, info) = find("op.copy").expect("op.copy is catalogued");
        assert_eq!(cat.id, "file_ops");
        assert_eq!(info.title, "Copy");
        assert!(find("nope.nothing").is_none());
    }
}
