# Two-Panel File Manager — Project Specification

**Codename:** dimnav (shipped name; bundle id `com.dimnav.desktop`)
**Author:** Dima
**Purpose:** Specification document for Claude Code to implement, maintain, and iteratively improve a modern, cross-platform, keyboard-driven two-panel file manager, inspired by Norton Commander / Midnight Commander / DOS Navigator.

---

## 1. Vision

A fast, keyboard-first, two-panel file manager for power users, built with a graphical interface that *feels* like a clean terminal application but has the visual flexibility of a modern GUI (transparency, theming, custom fonts).

The two-panel layout is the **first-class, non-negotiable core interaction model** — not a mode or a plugin, but the defining feature of the app. Everything else (terminal, editor, search) is built around it.

Primary inspiration: Norton Commander, Midnight Commander, DOS Navigator, Total Commander.

---

## 2. Target Platforms

- **Phase 1 (MVP): macOS only.**
- **Phase 4:** Windows and Linux, once the core is stable on macOS. (§8 is the authoritative phase numbering; this section formerly said "Phase 2+".)
- Architecture must remain cross-platform-friendly from day one (avoid macOS-only APIs in core logic) even though we ship macOS first.

---

## 3. Technology Stack

| Layer | Choice | Notes |
|---|---|---|
| Core logic / backend | **Rust** | File system operations, performance-critical logic, config management |
| Application shell | **Tauri** | Cross-platform native window, packaging, OS integration |
| Frontend UI | **Svelte** (decided at scaffolding) | Rendered inside Tauri's webview; kept thin per the swappable-frontend rule |
| IPC | Tauri commands/events | Frontend ↔ Rust backend communication |

### Why Rust + Tauri
- Rust gives near-native performance for file system traversal, copy/move operations, and large-directory handling.
- Tauri provides a modern UI layer with far less overhead than Electron, while still allowing rich theming, transparency, and animation if desired.
- The developer (Dima) is experienced with web technologies and has basic Rust knowledge — this stack plays to those strengths while keeping the performance-critical path in Rust.

### Webview keyboard responsiveness — assessment
A concern was raised about whether a webview-based UI can deliver the snappy keyboard feel this app demands. Assessment:

- **Latency is not expected to be a problem for this class of app.** On macOS, Tauri renders through WKWebView; keydown handling is on the order of a single frame (~16ms), which is imperceptible for list navigation. Webview input lag matters for games and high-refresh creative tools — not for a keyboard-driven file list. Tauri is an industry-standard choice for this category today.
- **The genuine downside of webview is cross-platform *visual consistency*, not speed.** Each OS ships a different webview engine — WebKit (macOS), WebView2/Chromium (Windows), WebKitGTK (Linux) — so CSS rendering, font smoothing, and edge-case behavior differ per platform. This is the opposite of what an app aiming for a consistent look everywhere wants, and will require per-platform testing and CSS tweaking in Phase 4.
- **Truly pixel-consistent alternatives are the GPU-canvas Rust toolkits** — Iced and egui (and the newer GPUI) — which render directly to the GPU with no web engine, giving identical output on every platform and smaller binaries. This is how apps like Zed and GPU terminals achieve uniform rendering. The cost is writing the UI in Rust with a less mature styling story and no HTML/CSS.

### Decision & the swappable-frontend constraint
- **Phase 1 stays on Tauri.** Dima's web-tech fluency is the decisive factor; it is the fastest path to a working, iterable app, and keyboard latency is a non-issue.
- **Critical architectural rule:** the frontend must be treated as a **thin, replaceable rendering layer**. ALL application logic — file operations, navigation state, selection model, config, plugin host — lives in the Rust backend behind a clean, typed IPC boundary. The webview only renders state and forwards input events. If per-platform rendering inconsistency becomes painful at Phase 4, the UI can be swapped to Iced/egui **without rewriting the core**. Do not let business logic leak into the frontend.

### Trade-offs to keep in mind
- Rust ↔ JS boundary needs a clean, well-typed IPC contract to avoid this becoming a maintenance burden as features grow. This boundary is doubly important given the swappable-frontend rule above.
- macOS TCC (Transparency, Consent & Control): the app will need explicit user permission to read protected folders (Desktop, Documents, Downloads, external volumes). Access-denied must be a handled, first-class state — see Section 5.6.

---

## 4. Visual & UX Direction

- **Look:** Modern but terminal-flavored. Default theme should look simple, clean, lightweight — not flashy or busy.
- **Transparency:** Background transparency supported, configurable.
- **Themes:** Fully configurable color schemes and fonts; ship with a small set of sensible default themes (e.g., "Classic Commander," "Dark Minimal," "Light Minimal").
- **No unnecessary animation/motion.** Static, calm, functional. Subtle transitions are fine; nothing distracting.
- **Window chrome:** Modern native window (title bar, resizing, etc.) — the app lives in a normal window, it doesn't try to *be* a terminal session, it just borrows the terminal's visual restraint.

---

## 5. Core Interaction Model — Two-Panel Navigation

### 5.1 Panels
- Two independent panels, side by side by default (left/right).
- Each panel independently browses its own directory.
- One panel is always "active" (has focus); the other is inactive but still visible with its own cursor position retained.
- Tab (or configurable key) switches active panel.

### 5.2 Multi-column layout within a panel
- Each panel's file list is displayed in **two columns** by default. (See Design Note below — column count may become width-adaptive.)
- Cursor (down arrow) moves down the first column; upon reaching the bottom, it continues from the top of the second column. Up arrow is the inverse.
- **The viewport is a sliding window, never a page flip** (orthodox behavior — Norton Commander, DOS Navigator, FAR). The visible entries are a contiguous window over the listing; a motion that would leave the window scrolls it by **that motion's own step**, no more:
  - Down at the bottom of the **rightmost** column scrolls by **one entry**: the next file appears at the bottom right, the entry that was at the top of the rightmost column moves to the bottom of the column left of it, and the top entry of the leftmost column scrolls off. Up at the top of the leftmost column mirrors it.
  - Right at the rightmost column scrolls by **one column**; the cursor keeps its on-screen row and stays in the rightmost column. Left at the leftmost column mirrors it.
  - Page Up / Page Down scroll by a **full page**; the cursor keeps its on-screen position.
  - The window is kept within the listing, so no blank space is shown below a listing longer than one page.
- **Left/Right arrow semantics — explicit rule to avoid ambiguity:**
  - Right moves the cursor one column to the right at the current row — one column's worth of entries further into the listing, scrolling as above once the cursor is already in the rightmost column.
  - Once the **end of the listing** is reached and there are no further columns, a further Right moves the cursor to the **last file** in the listing.
  - Left mirrors this exactly in reverse; once the start of the listing is reached, a further Left eventually lands on the **first entry** (which is always `..`, see below).
  - Net effect: Left/Right is a single continuous linear traversal across the whole listing (column by column), Up/Down traverse it entry by entry. Implement as **one cursor-index state machine** with a stored window origin, not two independent handlers.
- **The first entry is always `..`** (parent directory), shown at the top of every non-root listing.
  - Pressing **Enter** on `..` navigates to the parent folder.
  - On arrival in the parent folder, the cursor **auto-positions onto the folder that was just exited** (the child directory the user came from), not the top of the list. This must be preserved so users can step in and out of folders fluidly.
- **Copy/move to the other panel do NOT use arrow keys** — they are bound to F5/F6 (Section 6). Left/Right arrows are purely intra-panel cursor movement and never move files.
- **Page Up / Page Down**: scroll by a full page of entries (see the sliding-window rule above).
- **Home / End**: jump to the very first (`..`) / very last file in the directory listing.

**View mode — column count is user-selectable, not fixed at two.** The number of columns is a per-panel setting, plus a dedicated **detailed mode** (single column with metadata — size, date, permissions — shown alongside each name).
- Modes: 1-column, 2-column (default), 3+ columns (brief/name-only), and **detailed** (single-column with metadata).
- Selectable **per panel** via a control at the top of that panel (mouse for now; keyboard shortcut strongly desired if a non-conflicting key is available).
- The Left/Right traversal above applies to the multi-column brief modes; detailed/1-column mode uses straightforward vertical Up/Down + PgUp/PgDn, over the same sliding window.
- The selected mode is **persisted per panel in user preferences** and restored on restart.

### 5.3 Selection model
- At any time, exactly one file has the **cursor** (like a mouse hover position).
- Selection is a separate, persistent state layered on top of cursor position — analogous to Ctrl+clicking multiple files with a mouse.
- **Space**: toggles selection of the file currently under the cursor, and is expected to be used repeatedly while moving the cursor to build up a multi-selection (select file → move cursor → select another file → ...).
- **Shift + motion** (Arrow, PageUp/PageDown, Home/End): moves the cursor and **flips the selection of every entry the cursor sweeps over**, as if Space had been pressed on each — so a sweep over an already-selected run clears it, exactly the way Space on a selected file unselects it. This matters because Left/Right jump a whole column: marking only the entry under the cursor would skip everything the cursor flew past.
  - The range is **half-open**: the entry the cursor *leaves* is flipped, the entry it *lands on* is not. Repeated presses therefore paint one continuous run with nothing flipped twice, and the cursor always rests on the next entry not yet touched.
  - When the motion is clamped and the cursor cannot move (Shift+Down on the last entry, Shift+Left on `..`), there is no range to sweep, so it degenerates to flipping the entry under the cursor — the same thing Space does at the last entry. Without this the last file would be unreachable, since Right past the end lands *on* it and the half-open range would exclude it.
  - Each entry flips **independently**; a mixed range comes out inverted, not painted to a single uniform state.
  - There is **no anchor**, so reversing direction does not undo the previous sweep (FarManager behaves the same way). Deselect all (`-`) is the way out.
  - `..` is never selectable, so it is never flipped.
- **Select All (e.g., `*` key)**: selects all files/folders in the active panel.
- Selection **persists** as the cursor moves through columns/pages — it does not reset on navigation.
- Operations (copy, move, delete) act on the current selection; if no explicit selection exists, they act on the single file under the cursor.

### 5.4 Core file operations (MVP)
- Navigate into directories (Enter), go up a level (Backspace or configurable).
- Create directory.
- Delete file(s)/directory(ies) (with confirmation).
- Copy file(s)/directory(ies) from active panel to the *other* panel's current directory (classic Commander behavior — this is the whole point of the two-panel model).
- Move file(s)/directory(ies) similarly.
- Rename file/directory.
- Refresh directory listing.
- Show basic file info (size, modified date, permissions).

### 5.4a File operation semantics (edge cases — MVP must define these)
Reference behavior: **model these on FarManager first; where FarManager guidance is unavailable, fall back to Midnight Commander.** These are the cases that make or break a file manager.

- **Destination prompt on copy/move:** F5 (copy) and F6 (move) open a dialog showing the **destination path**, pre-filled with the other panel's current directory but **manually editable**. The user can type a relative path — e.g., entering `..` copies the selection into the parent folder; an absolute path is also accepted. Confirming runs the operation to that path.
- **Name collisions:**
  - **Single file:** dialog offers **Cancel / Skip / Overwrite** (no "…All" options shown).
  - **Multiple files:** dialog offers **Cancel / Skip / Skip All / Overwrite / Overwrite All**.
  - Never silently overwrite. (FarManager semantics preferred; MC as fallback.)
- **Directory recursion — clarification:** copying/moving a folder **descends into and copies its full contents** — this normal directory recursion is required and expected. What is forbidden is copying/moving a folder **into itself or into one of its own descendants**; detect this and handle per the next point. ("No recursive copying" is understood as "no infinite self-recursion," not "don't copy folder contents.")
- **Selection includes the destination/parent folder itself:** if one of the selected items is the folder into which the operation would place things (i.e., copying a folder into a child of itself, or the folder is its own target), pop a dialog identifying that specific folder and let the user **Skip that folder** or **Cancel** the whole process. The rest of the selection proceeds if Skip is chosen.
- **Copying into a child folder** is otherwise perfectly fine and supported.
- **Insufficient permissions:** show a failure dialog offering to **elevate privileges** (Skip / Cancel also available, matching the collision dialog's shape). Privilege escalation must go through the **OS's native authorization mechanism** (macOS Authorization Services / native admin prompt) so the password is entered into the operating system's own dialog — the app never collects or handles the password itself. See Section 5.6.
- **Large / many files:** operations run on the Rust side with a **progress indicator and a Cancel option**; UI stays responsive (async, never blocks the event loop).
- **Symlinks:** treated as by most managers / MC — shown with a distinct indicator, with the ability to **edit the link target** (as in MC). Default: operate on the link itself (not its target) for move/delete; follow for navigation.
- **Deletion — Trash is a checkbox, OFF by default:** the delete flow includes a **"Move to Trash" checkbox, unchecked by default** (so default delete is a real delete). The checkbox state **persists across sessions**. No undo in v1. Delete still confirms before acting.

### 5.4b Failure dialogs
- Any operation failure surfaces a **dialog with a red background** (clear, unmistakable error styling), stating what failed and why, with appropriate Skip / Skip All / Cancel / Retry options consistent with the operation in progress.

### 5.5 Opening files
- **MVP:** Opening a file delegates to an external application:
  - System default application (equivalent to macOS "Open").
  - OR a user-configured external editor/viewer mapped by file extension/type (e.g., `.java` → VS Code, `.md` → Typora, `.txt` → configured default).
- **View / Edit keys (carried from the classic Commander convention):**
  - **F3 = View** the file under the cursor (read-only). MVP: opens the configured external viewer for that type, or system default.
  - **F4 = Edit** the file under the cursor (read-write). MVP: opens the configured external editor for that type.
  - Post-MVP these route to the embedded viewer/editor (Section 8, Phase 3) when configured to do so.
- **Executing files (see also Section 5.7 Terminal integration):** pressing **Enter** on an executable file (shell script or binary) **runs it**, with output directed into the terminal (Phase 2). In Phase 1, before the terminal exists, running an executable should surface its output in a simple modal/output pane or be deferred — to be decided at implementation; the *file-type routing* (is-this-executable detection) should still be built in Phase 1.
- **Post-MVP:** Built-in lightweight embedded text viewer/editor (see Section 8).

### 5.6 Access, permissions & error handling (first-class states)
- **Access denied** (macOS TCC-protected folders, permission bits, locked files): show a clear, non-fatal **red-background** message; never crash or hang. Where the fix is elevation, offer to escalate via the **OS-native authorization prompt** (macOS Authorization Services) — the app never collects the password itself — with Skip / Cancel available. Where the fix is a TCC grant (Desktop/Documents/Downloads/external volumes), trigger the OS permission prompt.
- **Unreadable / broken entries** (dangling symlinks, special files, devices): render with a distinct marker rather than failing the whole listing.
- **Directory changed underneath us** (files added/removed by another process): refresh gracefully; keep cursor on a sensible entry.
  - Both panels' directories are **watched**, so outside changes appear without the user asking. Events are coalesced, never mapped one-to-one to re-listings: a change marks the panel dirty, and it is re-read after a short quiet period or a hard upper bound, whichever comes first (so a long extraction still streams updates). A re-read whose result is identical is dropped rather than pushed.
  - The cursor and the selection follow **names**, not indices, and the scroll position is preserved — a background change must never move what the user is looking at.
  - `Ctrl+R` refreshes on demand, and regaining window focus re-checks both panels. These cover what watching structurally cannot: network volumes (FSEvents does not report on them; they fall back to polling), dropped events, and time spent suspended.
- **The open directory itself changes.** A directory has an identity independent of its path, so the panel tracks the directory rather than the string. What the panel does depends on which of these happened:

  | What happened | What the panel does |
  |---|---|
  | Renamed or moved (including any **ancestor** being renamed) | **Follows it.** Path updates; cursor, selection and scroll survive. The folder still exists with the same contents, so from the user's point of view nothing happened — climbing to the parent would discard their place in response to a non-event. |
  | Moved to the Trash | Treated as a deletion. Deliberately **not** followed: trashing means "get rid of it", not "browse it in the Trash". |
  | Deleted | Falls back to the nearest existing **readable** ancestor, with the cursor placed where the deleted folder used to sort. |
  | Replaced (a different directory now at the same path) | Stays at the path and reloads from scratch — cursor to top, selection dropped. |
  | Volume ejected | Goes home. Landing in `/Volumes` would be useless. |
  | Became unreadable (TCC / permissions) | **Stays put** and says so, offering a retry. Navigating away would hide the actual problem. |

  Each of these attaches a short, **non-modal** notice to the panel. This is not the red failure dialog: a background process renaming a folder is not an operation failure.
- Every file operation returns a structured result the frontend can render (success / partial / failed-with-reason), rather than throwing opaque errors.
- **Operating on a stale listing:** before a destructive operation runs, each resolved path is re-checked against the filesystem and the operation is refused with a clear message if the listing no longer describes it. This is not a fix for the general time-of-check/time-of-use race, which no check of this shape can close; it catches the case that actually occurs.

### 5.7 Terminal integration with navigation (Phase 2 behaviors, specified now)
- **Enter on an executable** runs it; stdout/stderr stream into the terminal.
- **Ctrl+Enter** inserts the **filename under the cursor** into the terminal command line **without moving focus away from the panel**. This lets the user move the cursor across files and press Ctrl+Enter repeatedly to quickly concatenate multiple filenames onto the current terminal command line. (Behavior: append the name, space-separated, respecting shell-quoting for names with spaces.)

### 5.8 Sorting, hidden files & per-panel persistence
- **Default sort:** by name, with **folders first, then files.**
- **Additional sort modes:** by filetype+name, by size, by date, and other standard modes. Selected via a **small control (button + dropdown) at the top of each panel.** A keyboard shortcut for cycling sort is welcome **only if a non-conflicting key exists** (must not clash with app or macOS system shortcuts).
- **Hidden files: shown by default.** A simple toggle turns them off. The toggle state **persists across restarts.**
- **Persistence (per panel):** sort mode, view/column mode (Section 5.2), and hidden-file visibility are all stored in user preferences and restored on the next launch, independently for each panel.

### 5.9 Quick search
Jump the cursor to a file by typing the beginning of its name — the orthodox managers' quick search, not a filter and not fuzzy matching.

- **Explicitly opened with Cmd+F**, never by typing into a panel. This is what leaves every existing panel binding (Space, `*`, `-`) meaning exactly what it meant, and it means *any* character — spaces and punctuation included — is query text once the box is open.
- A small **search box appears in the top-right corner of the active panel**, showing what has been typed so far and a **✕ button** that cancels it.
- Each typed character extends the query, and the cursor moves to the **first entry whose name starts with it**, case-insensitively. A match below the fold scrolls into view under the normal sliding-window rule (§5.2). `..` is never a match.
- **A character that would match nothing is rejected**: it is not appended, the cursor does not move, and the app **beeps** and flashes the box. The query therefore always describes a real entry — the user can never be stranded in a dead query.
- **Backspace** steps the query back one character and the cursor with it. Emptying the query leaves the box open; only an explicit close shuts it.
- **Esc or Enter closes the box**, leaving the cursor on the match. Neither key does its usual job on that press: **Enter must not open the entry** under the cursor (opening a folder takes a second Enter) and **Esc must not draw the terminal curtain** (that takes a second Esc). Finding a folder you did not want to enter is the common case, so closing and opening on one keystroke would make the search unusable.
- **Any other input cancels the search** — switching panel, reaching for the terminal, navigating, sorting, or starting a file operation — and then does its normal job. The cursor stays where the search left it.
- The query is **transient**: never persisted, and it does not survive a change of directory. A refresh of the *same* directory leaves it alone, so an operation completing does not yank the box away mid-word.

---

## 6. Keyboard Shortcuts

All shortcuts must be **fully configurable**, with a sensible, documented default set inspired by Norton Commander / Midnight Commander conventions. Below is the proposed default mapping for MVP — to be finalized during implementation and stored in an editable config file.

| Action | Default Key |
|---|---|
| Help (About + the live shortcut list) | F1 |
| Switch active panel | Tab |
| Show the active panel's folder on the other panel (focus stays put) | Ctrl+= |
| Move cursor up/down | ↑ / ↓ |
| Jump between columns | ← / → |
| Page up/down | PgUp / PgDn |
| Jump to first/last file | Home / End |
| Toggle selection on current file | Space |
| Toggle selection across a range, and move | Shift + Arrow / PgUp / PgDn / Home / End |
| Select all | `*` |
| Deselect all | `-` (or configurable) |
| Enter directory / open file / run executable | Enter |
| Go to parent directory | Backspace |
| View file (read-only) | F3 |
| Edit file | F4 |
| Copy | F5 |
| Move/Rename | F6 |
| Create directory | F7 |
| Delete (Trash checkbox in dialog, off by default) | F8 / Delete |
| Insert filename into terminal (no focus change) | Ctrl+Enter |
| Quick search — jump to a name (§5.9) | Cmd+F |
| Close quick search (does not open the entry) | Esc / Enter *(while the box is open)* |
| Erase a character of the quick search | Backspace *(while the box is open)* |
| Toggle terminal / panels view | Esc *(Phase 2 — see below)* |
| Cycle sort order | Ctrl+S |
| Toggle hidden files | Ctrl+H |
| Cycle view/column mode | Ctrl+1 … Ctrl+4 *(1–3 columns, detailed)* |
| Refresh | Ctrl+R |
| Quit | Cmd+Q (macOS convention) |

Configuration should support remapping any action to any key or key combination, and detecting conflicts.

**Help must be generated, never hand-written.** The F1 screen lists the shortcuts actually in force by joining the live keymap against a catalog of action descriptions, so a remapped key or an alternate schema is reflected without editing a second list. Every bindable action therefore needs a human-readable title, and a test enforces that the keymap and the catalog stay in step.

**Esc / terminal toggle (Phase 2):** modeled on Midnight Commander. The terminal is always present as a command line at the bottom edge. Pressing **Esc** hides the file panels and reveals the full terminal output (as if the panels were a curtain drawn over an always-running terminal); pressing **Esc** again restores the panels. This is the primary way to inspect command output. See Section 8, Phase 2.

---

## 6a. Modular Architecture & Plugin System

The app must be **modular and extensible from the start** — even features shipped in-core (terminal, editor, archive browsing) should be structured as if they were plugins, so the extension system is dogfooded rather than bolted on later.

### Architectural layering
- **Core (Rust):** file-system engine, navigation/selection state, config, plugin host, IPC surface. Knows nothing about specific UI.
- **Frontend (webview, swappable):** renders state, forwards input. See Section 3's swappable-frontend rule.
- **Modules/Plugins:** self-contained units that hook into defined extension points. In-tree features (terminal, viewer/editor, archive support) should be built against these same extension points where practical.

### Extension points (the plugin API surface)
A plugin should be able to, at minimum:
- Register **commands** (bindable to keys, appearing in a command palette).
- Register **file-type handlers** (custom view/edit/open/preview behavior, e.g., an image previewer, a hex viewer, an archive-as-virtual-directory provider).
- Add **panel columns / metadata providers** (e.g., git status, checksums).
- Add **custom operations** to the file-operation pipeline (e.g., "compress selection," "upload to X").
- Contribute **themes**.
- Optionally contribute **UI surfaces** (a panel, a status-bar item) via the frontend, subject to the swappable-frontend constraint.

### Plugin model — decisions to make during Phase design
- **Isolation & language:** options include native Rust dynamic plugins (fast, but ABI/safety concerns and must be recompiled per platform), **WASM plugins** (sandboxed, portable, safer — increasingly the norm for this; aligns well with a Rust core), or a scripting layer (e.g., Lua/JS) for lightweight extensions. **Recommendation: design the plugin host around WASM** for sandboxing and cross-platform portability, with a possible scripting bridge for simple cases. Confirm at the Phase where plugins land.
- **Security:** plugins must run with least privilege; file-system and shell access should be permissioned and visible to the user. Never let a plugin silently perform destructive or network operations without a declared capability.
- **Stability:** define the plugin API as a versioned, documented contract early, even if only used internally at first, so third-party plugins later don't break on every release.

### Phasing
- **Phase 1:** establish the internal module boundaries and a stable core↔UI IPC contract. No third-party plugin loading yet, but structure the code so features are modules behind clean interfaces.
- **Later phase (see roadmap):** expose the public, versioned plugin API and a loader; migrate/confirm in-tree features (terminal, editor, archives) as consumers of that API.

---

## 7. Configuration System

- Config stored in a human-readable file format (likely TOML or JSON) in the standard OS config directory (`~/Library/Application Support/<AppName>/` on macOS).
- Configurable areas:
  - **Keybindings** — override any default shortcut.
  - **Themes** — colors, fonts, transparency level; select from bundled themes or define a custom one.
  - **File type → application mapping** — associate extensions/MIME types with specific external editors/viewers, with support for multiple mappings (e.g., default open vs. explicit "view" vs. explicit "edit" actions, once that mode is introduced).
  - **Panel behavior & persisted UI state** — default starting directories; per-panel column/view mode, sort mode, and hidden-file visibility (all persisted across restarts); global Trash-checkbox state.
- Config should have sane, working defaults out of the box — the app must be fully usable with zero configuration.

---

## 8. Post-MVP Roadmap (Phased)

### Phase 1 — MVP (macOS)
- Two-panel navigation; **selectable column count per panel** (2-column default) plus a **detailed single-column mode**; full keyboard nav and `..` behavior per Section 5.
- Core file operations: copy, move, delete, rename, create directory — with the **destination-path prompt** (F5/F6, editable, accepts `..`/relative/absolute) and **FAR-style collision dialogs**.
- Delete with a **"Move to Trash" checkbox (off by default), state persisted**; no undo.
- Multi-selection model (space / shift+arrow / select all).
- **Sort control** (folders-first-by-name default; type+name / size / date modes) via a per-panel dropdown; **hidden files shown by default** with a persisted toggle.
- **Per-panel persistence** of column/view mode, sort mode, and hidden-file visibility.
- Opening files via system default or configured external application (by file type); **F3 View / F4 Edit** routed to external tools.
- **Red-background failure dialogs**; access-denied handled via OS-native elevation/permission prompts.
- Configurable keybindings and basic theming (including transparency).
- Config file support (read on startup; hot-reload optional, nice-to-have).

### Phase 2 — Embedded Terminal
- Spawn and interact with a single shell process from within the app (one session is sufficient — no multi-session/tab management required).
- Not a full terminal-emulator clone. It should feel natural — like an always-available shell — without needing to perfectly host complex full-screen TUI apps. Implementation likely via a PTY-backed shell (`portable-pty`) plus a lightweight renderer; whether to embed a fuller emulator crate is a spike at the start of Phase 2.
- **Primary interaction model (Midnight Commander style):**
  - A terminal **command line lives permanently at the bottom** of the window, beneath the panels. The user can type and run commands there at any time.
  - Pressing **Esc** hides the file panels and shows the full terminal output — conceptually the panels are a "curtain" over an always-running terminal. Pressing **Esc** again brings the panels back. This is the main way to view command output.
- **Executing files:** pressing **Enter** on an executable (script or binary) runs it in this shell; output goes to the terminal.
- **Ctrl+Enter** appends the filename under the cursor to the current command line **without shifting focus from the panel**, enabling rapid multi-file command building (space-separated, shell-safe quoting). See Section 5.7.
- **Optional alternate layouts (nice-to-have, not required):** terminal docked at the bottom at partial height alongside visible panels, or a vertical split within one side (file view on top, terminal below). The Esc curtain model is the required baseline; these are extras.
- Working directory of the terminal should track the **active panel's** current directory (with a clear rule/hotkey to sync or decouple).

### Phase 3 — Embedded Editor/Viewer
- Lightweight built-in text viewer and editor.
- Two explicit modes: **View** (read-only, **F3**) and **Edit** (read-write, **F4**).
- Still respects the external-editor file-type mapping from Phase 1 — F3/F4 route to the embedded viewer/editor only when the user configures them to; otherwise they continue to open external tools. Embedded editor is one selectable option, or a fallback default.

### Phase 4 — Cross-Platform Expansion
- Windows and Linux builds.
- Audit for any macOS-specific assumptions introduced in earlier phases (file dialogs, path handling, keybinding conventions like Cmd vs Ctrl).
- Re-evaluate webview visual consistency across platforms; if painful, this is the point to consider swapping the frontend to a GPU-canvas toolkit (Iced/egui) per Section 3 — enabled by the swappable-frontend rule.

### Phase 5 — Public Plugin System
- Expose the versioned, documented plugin API (Section 6a) and a loader.
- Ship with the WASM (or chosen) plugin host, capability-based permissions, and at least one reference plugin (e.g., git-status column or archive provider).
- Confirm in-tree features consume the same extension points.

### Future / Backlog (not yet scoped in detail)
- Archive browsing (zip/tar) as virtual directories (candidate first-party plugin).
- Bookmarks / quick-jump favorite directories.
- Bulk rename tool.
- File search across directory tree.
- Git-aware status column.

---

## 9. Open Questions for Future Discussion
- ~~Exact frontend framework — decide at scaffolding.~~ **Resolved: Svelte** (§3). Kept thin per the swappable-frontend rule.
- Terminal implementation depth: PTY-backed shell vs. a fuller emulator crate. **Partly settled:** Phase 2 shipped a *pipe-based* child of the login shell, which is what makes the stderr-based red/green run indicator possible. The PTY upgrade — and the trade it forces — is tracked under `docs/FEATURES.md` → Planned → Terminal.
- Plugin runtime: WASM (recommended) vs. native dynamic vs. scripting bridge — confirm before Phase 5.
- ~~Which specific keys are free for sort / hidden-files / view-mode toggles.~~ **Resolved: Ctrl+S, Ctrl+H, Ctrl+1–4** (§6), all bound in `config::default_keymap`.
- ~~Whether the terminal's working directory auto-syncs to the active panel.~~ **Resolved: it auto-syncs**, and the prompt follows the active panel.

---

## 10. Guidance for Claude Code

- Treat Section 5 (Core Interaction Model) as the non-negotiable heart of the app — protect the responsiveness and correctness of two-panel keyboard navigation above all else. Implement the Left/Right/Up/Down traversal (Section 5.2) as **one cursor-index state machine**, not ad-hoc handlers.
- **Keep ALL logic in the Rust core behind a clean, typed IPC contract.** The webview is a thin, swappable rendering layer (Section 3). No business logic in the frontend. This is what makes a future Iced/egui swap possible.
- **Structure features as modules against defined extension points from day one** (Section 6a), even before the public plugin API ships — the terminal, viewer/editor, and archive support should look like plugins internally.
- Build Phase 1 completely and solidly before Phase 2+ scope creep. But do NOT design Phase 1 in a way that blocks the specified Phase 2 behaviors (Esc terminal curtain, Enter-to-execute, Ctrl+Enter filename insertion) — leave the seams.
- File operations are **async, cancellable, and return structured results**; never block the UI thread (Section 5.4a).
- **Trash-first deletion** and **collision prompts** are required product behavior, not optional polish.
- Keep the Rust backend platform-agnostic where reasonable even though macOS ships first, to ease Phase 4. Handle macOS TCC permission states explicitly.
- All keybindings and theme values are config-driven, never hardcoded — core to the product vision.
