# Feature Checklist

Running record of what this file manager can do and what is still planned.
`docs/SPEC.md` says what the app *should* be; this file says where it *is*.

> **Backfill pending.** This file was started during the terminal slice, so only
> the terminal is filled in below. Everything shipped before it — navigation,
> selection, copy/move/delete, sorting, the embedded viewer/editor — is listed
> under **[Backfill needed](#backfill-needed)** and gets itemised in a follow-up
> pass.

Section references are to `docs/SPEC.md`.

---

## Implemented

### Quick search (§5.9)

- [x] **Cmd+F** opens a search box in the **top-right corner of the active
      panel**, with a **✕** button to cancel. Opened deliberately, never by
      typing into a panel — which is what leaves Space / `*` / `-` meaning what
      they always meant, and makes *every* character query text once it is open
- [x] Each character extends the query and moves the cursor to the **first name
      that starts with it**, case-insensitively. Prefix, not fuzzy, not a filter
- [x] A match below the fold scrolls into view under the §5.2 sliding-window
      rule — quick search goes through the same `set_cursor` as a mouse click
- [x] `..` is never matched
- [x] **A character that matches nothing is rejected** — not appended, cursor
      unmoved — and the app **beeps** and flashes the box red. The query always
      describes a real entry, so there is no dead-query state to back out of
- [x] **Backspace** steps the query back and the cursor with it; emptying the
      query leaves the box open
- [x] **Esc or Enter** closes the box with the cursor on the match, and
      **neither does its usual job on that press**: Enter does not open the entry
      (a second Enter does) and Esc does not draw the terminal curtain (a second
      Esc does)
- [x] Any other input cancels the search and then does its normal job —
      switching panel, Cmd+T, navigating, sorting, F1, or any file operation.
      Centralised in `AppState::snapshot_after_input`, which every user-initiated
      command returns through, so no call site can forget
- [x] Transient: not persisted, and dropped on a change of directory. A refresh
      of the *same* directory keeps it, so a completing operation cannot yank the
      box away mid-word
- [x] The query is **core-authored** — `fm-core::nav::search` owns the matching
      and the accept/reject decision, and the box renders text rather than
      hosting an `<input>`, so a rejected character is never painted at all

### Help popup (§6)

- [x] **F1** opens a large popup over the app, from every surface — panels,
      viewer, editor, and the focused terminal prompt
- [x] Topics listed vertically on the left; **Tab** / **Shift+Tab** cycle them,
      wrapping round at either end. Clicking a topic works too
- [x] **About** (the default topic): app name, version and description, taken
      from the Tauri bundle metadata (`productName` / `version`) and the crate
      manifest — no hand-maintained version string
- [x] **Shortcuts**: every binding currently in force, sectioned by keyboard
      context (Panels / Viewer / Editor / Terminal / Help) and grouped by what
      the action does, each with a title and a short explanation
- [x] Key chords are rendered the way the F-key bar renders them (`⌘⇧T`, `↑`,
      `Space`), not as the internal chord strings
- [x] Quick-search field filters the list across **all** of it — context,
      category, key chord, action id, title and description. Multiple terms
      narrow (AND), and symbols are findable by name (`cmd` finds `⌘`)
- [x] `↑` / `↓` / PgUp / PgDn scroll the topic; `←` / `→` and Home/End are left
      alone so they still edit the search text
- [x] **Esc** (or F1 again, or F10) closes, restoring keyboard focus to whatever
      owned it — including the editor buffer and the terminal prompt
- [x] The shortcut list is **generated** from the live keymap joined against a
      core action catalog (`fm-core::actions`), so it cannot drift from the
      keyboard; a test fails if an action is bound without help text
- [x] Topics are built against a `HelpTopic` extension point (§6a), so a future
      plugin-contributed topic is a registry entry rather than a special case
- [x] Deliberately does **not** open over a file-operation dialog — those are
      questions awaiting an answer

### Embedded terminal (§5.7, §8 Phase 2)

- [x] Command line permanently below the panels, under the focused-entry status
      bar and above the F-key bar
- [x] `>` at the far left; the top border lights up in the active-panel accent
      when the prompt owns the keyboard
- [x] **Cmd+T** focuses the prompt; Cmd+T again hands the keyboard back to the
      panel that is still active
- [x] **Cmd+Shift+T** expands the pane to the bottom half of the window and shows
      accumulated output; the panels shrink to fit rather than being covered
- [x] Expanded pane stays open when focus returns to the panels — browse, run,
      and watch output at the same time
- [x] Commands run in the active panel's directory, which the prompt follows
      automatically
- [x] Enter on an executable in a panel runs it into the same buffer (no modal)
- [x] Typing a program's name at the prompt runs it
- [x] Run indicator at the far right: yellow flashing while running, green on a
      clean exit, red on a non-zero exit **or** any stderr output, grey once the
      user touches any control again
- [x] Partial input survives losing focus
- [x] **Ctrl+C** interrupts a running command (SIGINT to the whole process group,
      SIGKILL after a grace period); with nothing running it clears the prompt
- [x] Scrollable output pane that accumulates everything run in the session,
      sticky to the bottom unless the user scrolls up
- [x] Scrollback capped at 1 MB by default, adjustable from the corner of the
      expanded pane, persisted
- [x] **Esc curtain** — panels aside, full-height terminal, Esc again to restore
      (§6)
- [x] **Ctrl+Enter** appends the name under the cursor to the command line,
      shell-quoted, without leaving the panel (§5.7)
- [x] Command history: Up/Down recall, persisted to
      `~/Library/Application Support/file-manager/history`
- [x] `cd` built-in navigates the active panel (MC-style), `clear` / Ctrl+L
      empties the buffer
- [x] Pane size and scrollback cap persisted in `config.toml` (§7)

**Known limits of the current execution model.** Commands run as a child of the
user's login shell with piped stdout/stderr, not through a PTY. This is what
makes the red/green indicator work as specified — a PTY merges the two streams,
so it could only colour by exit code. The consequences:

- No colours (programs see no TTY and turn them off)
- No interactive programs — `sudo`, `ssh`, `vim`, `less`, `python` REPL. stdin is
  closed deliberately so they fail fast rather than hang
- No full-screen TUI apps (`top`, `htop`), which §8 explicitly scopes out
- Shell state does not carry between commands (`export FOO=1` then `echo $FOO`);
  each command is a fresh shell
- stdout and stderr can interleave out of order. This is the standard pipe
  buffering asymmetry, not a threading bug: stdout is block-buffered when it is
  not a TTY, so it arrives in bursts, while stderr is unbuffered and arrives
  immediately. A PTY would fix it, at the cost of the stderr-based indicator

---

## Planned

### Terminal

- [ ] **Persistent PTY shell** — one long-lived interactive shell for the session:
      real interactivity (`vim`, `sudo`, `ssh`), colours, and shell state that
      carries between commands. Accepted trade: the run indicator loses its
      stderr-based red/green and degrades to exit-code-only, since a PTY merges
      stdout and stderr. Also needs an ANSI parser, echo suppression, and
      shell-integration hooks (OSC 133/7) to know when a command starts, ends,
      and where it is
- [ ] ANSI colour rendering in the output pane
- [ ] Virtualised scrollback rendering (the pane currently paints the whole
      buffer as one text node)
- [ ] Drag handle to resize the pane instead of the fixed half-height
- [ ] Tab completion of paths and command names at the prompt
- [ ] Multiple terminal sessions / tabs
- [ ] Windows and Linux shell handling (§8 Phase 4)

### Elsewhere (from SPEC §8)

- [ ] Theming and transparency, config-driven (§4)
- [ ] Keybinding remapping and conflict detection (§6) — including the terminal
      bindings added in this slice. `get_keymap` and the F1 help screen both read
      `default_keymap()` today, so help already renders whatever the keymap says;
      remapping only has to change that one source for help to follow
- [ ] Selectable shortcut schemas (§6) — the F1 About topic already reports which
      schema is applied, currently always "Default"
- [ ] More help topics (§6) — the `HelpTopic` extension point is in place; only
      About and Shortcuts ship so far
- [ ] Symlink target editing (§5.4a)
- [ ] Config hot-reload (§7, nice-to-have)
- [ ] Public plugin API and loader (§6a, Phase 5)
- [ ] Archive browsing as virtual directories
- [ ] Bookmarks / quick-jump directories
- [ ] Bulk rename
- [ ] File search across a directory tree
- [ ] Git-aware status column

---

## Backfill needed

Shipped and working, but not yet itemised here — a follow-up task expands each of
these into checkboxes:

- Two-panel navigation and the single cursor-index state machine (§5.2)
- Per-panel column/view modes including detailed mode, persisted (§5.2, §5.8)
- Multi-selection model (§5.3)
- Copy / move with the editable destination prompt and FAR collision dialogs (§5.4a)
- Delete with the persisted "Move to Trash" checkbox (§5.4a)
- Create directory, rename, refresh, recursive folder-size calculation (§5.4)
- Sorting modes and the hidden-files toggle, persisted per panel (§5.8)
- Open / F3 View / F4 Edit routing, external and embedded (§5.5)
- Embedded viewer and editor: text/hex/image, search, goto, wrap, encodings,
  atomic save with conflict detection (§5.5)
- Red-background failure dialogs and macOS-native privilege elevation (§5.4b, §5.6)
- TOML config persistence (§7)
