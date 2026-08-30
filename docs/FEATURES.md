# Feature Checklist

Running record of what this file manager can do and what is still planned.
`docs/SPEC.md` says what the app *should* be; this file says where it *is*.

**[Outstanding](#outstanding) is the short answer to "what is left"** — phases, platform
stubs, unimplemented extension points and known defects, in one place. The
[Planned](#planned) list itemises the remaining features; the [Implemented](#implemented)
sections record what shipped and why.

> **Backfill pending.** This file was started during the terminal slice, so only
> the terminal is filled in below. Everything shipped before it — navigation,
> selection, copy/move/delete, sorting, the embedded viewer/editor — is listed
> under **[Backfill needed](#backfill-needed)** and gets itemised in a follow-up
> pass.

Section references are to `docs/SPEC.md`.

---

## Outstanding

**Read this first when asking "what is left?"** — it is the consolidated answer, so the
question does not have to be re-derived from the source each time. The
[Planned](#planned) list further down itemises the remaining *features*; this section
adds the phases, the platform stubs, the unimplemented extension points, and the known
defects, with pointers.

### Phases (SPEC §8)

| Phase | State |
|---|---|
| 1 — MVP (macOS) | **Shipped** v0.1.0 |
| 2 — Embedded terminal | **Shipped** v0.1.0 — pipe-based, not PTY (see the terminal limits above) |
| 3 — Embedded editor/viewer | **Shipped** v0.1.0 |
| 4 — Cross-platform (Windows/Linux) | **Not started** — see the stub table below |
| 5 — Public plugin system | **Not started** — traits only, no loader and no host |
| Future / backlog | Archive browsing, bookmarks, bulk rename, tree search, git column |

SPEC §2 puts Windows/Linux at "Phase 2+" while §8 puts them at Phase 4. **§8 is the
numbering this file follows.**

### Phase 4 — every platform stub

Each is a deliberate `#[cfg]` fallback rather than a bug. The ones that return *wrong
data silently* are marked ⚠, because those are the ones that will not announce
themselves when the port begins.

| Location | Behaviour off macOS / off unix |
|---|---|
| `crates/fm-core/src/fs/watch.rs:468` | `DirHandle::open` → `Err(Unsupported)`; watching degrades to nothing. Windows must pass `FILE_SHARE_DELETE`, or holding the handle blocks deleting the watched directory |
| `crates/fm-core/src/fs/watch.rs:350` | `O_PATH` is the Linux analogue to reach for; `O_RDONLY` is the placeholder |
| `crates/fm-core/src/ops/mod.rs:811` | ⚠ `copy_symlink` copies the **resolved contents** instead of the link |
| `crates/fm-core/src/fs/mod.rs:52`, `:253`–`:300` | Owner/group → `None`; uid/gid/nlink/mode → `0`; ⚠ `is_executable` → `false`, which would break the colour and execute classifier outright |
| `src-tauri/src/commands.rs:1562` | `launch` → `Err`; opening files unsupported |
| `src-tauri/src/commands.rs` `open_privacy_settings` | → `Err`. Windows has no equivalent and the Linux answer is desktop-specific; the core simply never offers the remedy off macOS |
| `crates/fm-core/src/fs/mod.rs` `is_policy_denial` | → `false`, so every denial classifies as `Denied` rather than `Restricted`. Correct until a platform gains its own policy layer |
| `src-tauri/src/ops_runtime.rs:196`, `:228` | `elevate`, `elevate_delete` → `Err` |
| `src-tauri/src/terminal_runtime.rs:247` | No process group to signal; killing the child is the approximation |
| `src-tauri/src/watch_runtime.rs:614` | ⚠ `is_local_volume` → unconditionally `true` (Linux wants `statfs.f_type`, Windows `GetDriveType`) |
| `.github/workflows/release.yml:26` | Linux and Windows matrix entries commented out |

### Phase 5 — extension points with no implementor

`crates/fm-core/src/plugin/mod.rs` declares eight traits. Implemented in-tree:
`HelpTopic` (About, Shortcuts), `FsObserver` (the watcher), `ExecutionSink` (the
terminal). **Not implemented: `Command` — whose `run()` is commented out at
`plugin/mod.rs:22` — `FileTypeHandler`, `ColumnProvider`, `Operation`, and
`ThemeProvider`, which has only `id()` and would need widening to be usable.** SPEC §6a's
"dogfood the extension system" therefore holds for three of eight points today.

### Known defects

- **`ConfigChangedEvent` is declared** (`src-tauri/src/events.rs:51`) **and emitted by
  nothing.**
- **`OpProgress.bytes_done` / `bytes_total` are always `0`** (`ops_runtime.rs:100`), cross
  the wire, and are read by nothing — the progress bar is count-based. Either populate
  them or drop them from the contract.

### Test coverage

~238 inline Rust unit tests; **no test files, no frontend test runner, no e2e.**
Untested: `src-tauri/src/commands.rs` (1565 lines, all 67 handlers); `ops_runtime.rs`,
which holds `sh_quote` and `applescript_escape` — the strings fed to `osascript … with
administrator privileges`, and so the highest-risk untested code in the tree;
`terminal_runtime.rs`; and `fm-core/src/state.rs`, whose `snapshot_after_input` is the
documented "no call site can forget" chokepoint. `watch_runtime.rs` has three tests
across 730 lines. There is no cross-platform CI, so every `#[cfg]` arm above is never
compiled.

### Deliberate non-goals

Recorded so they are not mistaken for gaps: no undo in v1; the Mac App Store is out of
scope (its sandbox would break the terminal and the `osascript` elevation); Intel Macs
are unsupported and the floor is macOS 13; there is deliberately no `cargo fmt` gate; and
full-screen TUI hosting is scoped out by SPEC §8.

---

## Implemented

### Panel navigation (§5.1)

The rest of two-panel navigation is still under
[Backfill needed](#backfill-needed); only what has shipped since is itemised here.

- [x] **Ctrl+=** shows the **active panel's folder on the other panel**, leaving
      the keyboard where it is — a push, not a Tab. The passive panel lands on
      `..` and keeps its own sort, view and hidden-file settings, since only the
      directory is mirrored
- [x] Pressing it again is a no-op rather than a re-read, so a second press
      cannot knock the other panel's cursor back to the top
- [x] It does nothing at all while the terminal prompt, the viewer, the editor or
      a dialog owns the keyboard — the binding lives only in the `panels`
      keyboard context, so no surface has to opt out of it by hand (§6)
- [x] The mirrored panel's new directory persists like any other move, so it
      reopens there on the next launch (§7)

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
- [x] **About** (the default topic): the app icon, name, version, license and
      description, taken from the Tauri bundle metadata (`productName`) and the
      crate manifest — no hand-maintained version string. `tauri.conf.json` omits
      `version` entirely so the bundler falls back to `Cargo.toml`, making the
      workspace manifest the single source
- [x] About shows the **squircle-masked app icon**, not the wordmark lockup: a
      self-contained rounded tile sits correctly on both themes, where a
      full-bleed dark image would be a slab on the light background
- [x] About links out to the website, the source repository, and sponsorship.
      Links the adapter left unset are omitted rather than rendered dead. Opening
      one hands the URL to the OS browser via a command restricted to `http(s)`,
      so the webview is never navigated away from the app
- [x] When a newer release exists, About shows it with an **Install and restart**
      action. The check runs once at startup and never blocks the panels;
      offline, rate-limited, and not-yet-published feeds are all silent
- [x] **Shortcuts**: every binding currently in force, sectioned by keyboard
      context (Panels / Viewer / Editor / Terminal / Help) and grouped by what
      the action does, each with a title and a short explanation
- [x] Key chords are rendered the way the F-key bar renders them (`⌘⇧T`, `↑`,
      `Space`), not as the internal chord strings — one formatter,
      `fm-core::keys::display_chord`, feeds both
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

### Keyboard hints (§6)

- [x] The editor saves with **⌘S** — the platform's own chord, not FAR's F2. On
      Windows and Linux the same binding is **Ctrl+S**, chosen at compile time in
      `config::default_keymap`; nothing in the frontend knows which platform it is
      on. F2 keeps its viewer meaning (word wrap) and no longer saves
- [x] The **F-key bars are generated from the live keymap**, like the F1 help
      screen (§6 — "help must be generated, never hand-written"). All three of
      them — panels, viewer, editor — name *actions*, and the chord printed
      beside each one is `KeyBinding::labels`, rendered core-side by
      `fm-core::keys::display_chord`
- [x] An action bound to nothing drops off its bar rather than advertising a key
      that does nothing
- [x] The editor and the viewer consult the keymap before letting a Cmd/Ctrl
      combo through, so a modified chord can be bound there; combos that are
      *not* bound still reach the OS and the text area untouched (⌘Q, ⌘A, ⌘C/V/Z)

### Embedded terminal (§5.7, §8 Phase 2)

- [x] Command line permanently below the panels, under the focused-entry status
      bar and above the F-key bar
- [x] `>` at the far left; the top border lights up in the active-panel accent
      when the prompt owns the keyboard
- [x] **Cmd+T** focuses the prompt; Cmd+T again hands the keyboard back to the
      panel that is still active
- [x] **Clicking a panel hands the keyboard back** — including the panel that is
      *already* active, which is the whole point: a panel can be the active one
      and still not hold the keyboard, because the prompt has it. The click is
      always reported to the core rather than being skipped when only `active`
      looks unchanged, so `set_active_panel` stays the one place that decides
      what reaching for a panel means
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
      `~/Library/Application Support/dimnav/history`, beside `config.toml`
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

### Access, permissions & error handling (§5.6)

- [x] **A directory the app cannot read says so**, instead of painting as an empty
      folder. `list_dir` used to drop the `read_dir` error on the floor, so a
      TCC-protected `~/Desktop`, a `0700` folder belonging to someone else and a
      directory that had just been deleted were all indistinguishable from an
      empty one — and from each other
- [x] **`EPERM` is told apart from `EACCES`**, which `io::ErrorKind::PermissionDenied`
      flattens together. On macOS the first is TCC and is fixed by a privacy
      grant; the second is the permission bits and is fixed by a mode or owner
      change. Offering the wrong remedy sends the user somewhere that cannot help
- [x] Where the fix is a TCC grant, a button opens **Full Disk Access** — macOS
      will not re-prompt once the user has refused, so the deep link is the only
      remaining route. Full Disk Access rather than the per-category Files and
      Folders pane, which lists only apps that have already triggered a prompt and
      so is a dead end for one that was denied or never asked
- [x] The remedy goes through its **own command with no URL parameter**.
      `open_link` is hard-restricted to `http(s)` because the opener plugin will
      launch `file://` and custom schemes — a local-file-execution primitive — and
      widening it for this would have undone that
- [x] The retry chord is read from the **live keymap**, like the F-key bars and the
      F1 book, so a rebind moves it (§6)
- [x] `..` is still listed, so a panel that lands in an unreadable folder is never
      a dead end the keyboard cannot leave
- [x] **The watcher can finally report it.** "Became unreadable" was in the §5.6
      fate table and unreachable in practice: the watcher recognises a directory by
      holding an `O_EVTONLY` descriptor on it, and an unreadable directory is
      exactly one it cannot open — so the fate was forced to `Alive` and the
      `Denied` notice never fired. The state now comes from the listing, which
      needs no descriptor
- [x] The listing digest hashes the access state, so a directory flipping to
      unreadable is not dropped as "nothing visibly changed" — its entry list is
      empty either way
- [x] The block **replaces** the panel notice rather than doubling it: both would
      be saying the directory cannot be read
- [x] **A directory the panel cannot read is not remembered as its start
      directory**, so the next launch does not reopen in a dead end (§7). Guarded
      in `capture_prefs`, the one place live panel state flows back into the config
- [x] Painted in the palette's error colour, not as a red slab: the saturated
      red-background dialog stays reserved for operations that actually failed
      (§5.4b)

Known limits:

- Where the fix is **elevation** rather than a grant, nothing is offered. Copy,
  move and delete escalate through the OS authorization prompt (§5.4a), but
  listing a directory as root would mean shelling out and parsing output
- Per-*child* denial still discards the errno: an unreadable row is
  `EntryMarker::Denied` whatever refused it
- `dir_size` skips subdirectories it cannot read, so a recursive folder size
  computed across a denied subtree is silently short

---

### Live directory watching (§5.6)

- [x] Both panels' open directories are **watched**, so changes made by Finder, a
      terminal or a build tool appear without the user asking. The panel's own
      directory and its parent are watched; two panels in the same place cost one
      watch
- [x] Events are **coalesced, never one re-listing per event**: a change marks the
      panel dirty and it is re-read after a quiet period, with a hard upper bound
      so a long extraction still streams updates instead of showing nothing until
      it finishes. A `git checkout` firing thousands of events costs at most one
      re-read per cap interval
- [x] A re-read that produces an identical listing is **dropped before it becomes
      IPC traffic**, so an event storm that changes nothing visible pushes nothing
- [x] Cursor and selection follow **names**, and the **scroll position is
      preserved** — a background change never moves what the user is looking at
- [x] The cursor falls to the **nearest surviving neighbour** when the entry it
      was on is deleted, rather than jumping to the top of the listing
- [x] **The open directory itself changing** is handled by tracking the
      directory's identity rather than its path, with a distinct response per
      cause — follows a rename or move (including of any *ancestor*), stays put
      when access is revoked, falls back to the nearest readable ancestor on a
      real deletion, goes home on an eject, and never follows into the Trash. See
      the table in SPEC §5.6
- [x] Each of those attaches a short **non-modal notice** to the panel; the
      red-background dialog stays reserved for operations that actually failed
- [x] **Ctrl+R** refreshes on demand (SPEC §6), and regaining window focus
      re-checks both panels — covering what a watcher structurally cannot see
- [x] Network volumes fall back to **polling**, since FSEvents creates the stream
      and then never delivers on SMB/NFS mounts
- [x] All of it is config-driven under `[watch]`, including a master switch
- [x] Destructive operations **re-check each path** against the filesystem before
      acting, so an operation built from a listing that has not caught up fails
      early and names the entry instead of failing obscurely later

Known limits:

- The identity/follow half is the only platform-specific part (`O_EVTONLY` +
  `fcntl(F_GETPATH)` on macOS). `notify` itself is portable, so Phase 4 needs
  roughly forty lines per OS behind the existing seam — with the caveat that
  Windows must pass `FILE_SHARE_DELETE` or the handle would block deleting the
  watched directory
- A renamed *ancestor* is caught by the identity poll (a couple of syscalls on a
  timer), not by an event, so it is noticed within that interval rather than
  instantly — no event fires inside either watched directory when a grandparent
  is renamed
- FSEvents is inherently recursive, so a panel sitting on a huge tree receives and
  discards subtree events. The cost per discarded event is a path comparison

---

### Theming (§4)

- [x] **Colour values are config-driven, not hardcoded.** `Config.theme` had been
      written and persisted since the config slice while nothing read it, and every
      colour lived as a literal in `app.css` — the exact inverse of the CLAUDE.md
      rule. `fm_core::theme::resolve` now turns the configured id into a `Palette`
      of ready-to-paint CSS custom properties
- [x] Three bundled themes: **Classic Commander** (the palette dimnav has always
      drawn, and still the default), **Dark Minimal** and **Light Minimal** — fewer
      hues, lower chroma, one accent
- [x] A theme carries **up to two variants**, dark and light. One that defines both
      follows the OS, which is what the stylesheet's `prefers-color-scheme` block
      used to do on its own; one that defines a single variant **pins** it, because
      following the OS into the other would paint half a palette
- [x] `appearance = "system" | "light" | "dark"` in `config.toml` overrides the OS
      for a two-variant theme. A pinned theme still wins, for the same reason
- [x] **User themes** live in `themes/<id>.toml` beside `config.toml`. A `base = ` line
      merges over a bundled theme, so a personal theme is a name, a base and the
      three colours actually being changed
- [x] Nothing about a hand-typed theme can stop the app painting: an unknown id, a
      missing file, a malformed one, and a variant the theme does not define all
      fall back rather than failing (§7)
- [x] **Bundled themes go through the `ThemeProvider` extension point** (§6a) rather
      than being special-cased, joining `HelpTopic`, `FsObserver` and
      `ExecutionSink`. They are parsed from embedded TOML by the same deserializer a
      user's file uses, so the merge path is exercised by the default configuration
      instead of only by files CI never sees
- [x] The core owns the light/dark decision, not a CSS media query — it is a
      three-way choice (`system`/`light`/`dark`), and a future Iced frontend has no
      `prefers-color-scheme` to consult. `app.css` keeps its values purely as the
      **pre-IPC bootstrap** so the first frame is not unstyled; inline properties on
      `:root` outrank them
- [x] A test maps every `EntryCategory` to a token and asserts every bundled theme
      defines it, so a new category or a dropped colour fails in CI rather than
      rendering an invisible row

---

### Settings (§7)

- [x] **F2 opens a settings popup from every surface** — panels, viewer, editor,
      the focused terminal prompt, and from the F1 help screen. Shaped like the
      help book: pages listed vertically on the left, **Tab** / **Shift+Tab** to
      cycle them, **Esc** to close, restoring keyboard focus to whatever owned it
- [x] The two large popups **swap rather than stack** — F2 in help opens
      settings, F1 in settings opens help, and neither is ever underneath the
      other. Neither opens over a file-operation dialog: those are questions
      awaiting an answer
- [x] Operated by the keyboard, not only the mouse: **↑ / ↓** walk the rows,
      **Enter** or **Space** changes the setting under the cursor, **← / →**
      step a multiple choice. A focused text or number field keeps those five
      keys and the popup keeps everything else, so Esc still closes and Tab still
      changes page from inside one
- [x] **Changes apply and persist immediately** — the contract every existing
      preference already had (`set_view_mode`, `set_sort_mode`,
      `set_trash_default` all write `config.toml` on the spot). No OK button and
      nothing to confirm; a row moved off its default grows a reset control
      instead
- [x] **Appearance page**: a theme **picker** listing every bundled theme plus
      whatever `themes/*.toml` holds, each with a preview swatch resolved through
      the same code path that would paint it — so the preview cannot disagree
      with the result. Picking one repaints the window with no restart
- [x] A theme that **pins** an appearance says so on its row, so the light/dark
      control beside it reads as overridden rather than broken
- [x] The picker marks the theme **actually in force**, not the configured id: a
      config naming a deleted theme highlights what is really painted (§4)
- [x] An id that names nothing is **refused rather than stored**. `resolve` falls
      back for an unknown id, which is right for painting and wrong for
      persisting — writing one would leave the picker showing one theme and the
      app painting another
- [x] A user theme that is missing, unreadable or malformed is **skipped from the
      list**, not reported: the picker offers what can be applied, and a
      half-written file cannot stop the page rendering (§7)
- [x] **Core-authored**, like the help book: `fm_core::settings` owns which pages
      exist, which settings are on them, their labels, option lists, defaults and
      validation, and the renderer paints each field by its control kind. There
      is no list of settings in the frontend
- [x] A field's id is its **dotted path into `Config`** (`"appearance"`), and the
      same string addresses it for reading, writing and resetting — so there is
      no second identifier scheme to keep in step. Reset is `apply` with the
      default looked up, not a second assignment per field, so a validation rule
      cannot cover one and miss the other
- [x] Pages are built against a **`SettingsPage` extension point** (§6a) beside
      `HelpTopic`, taking the implemented extension points to four. A test asserts
      every field the book paints round-trips through its own id, so a mistyped
      one fails CI rather than dropping a row silently
- [x] **F2 displaced the viewer's word wrap**, which moved to **⌃W**. Both bars
      followed on their own — they are generated from the live keymap — and the
      F1 shortcut list gained a Settings section the same way

### Configuration (§7)

- [x] **A broken line in `config.toml` costs that line, not the file.** `toml::from_str`
      fails the *whole* document on a single wrong value, so a hand-edited
      `trash_default = yes` used to discard panel directories, file associations and the
      Trash flag along with the flag actually mistyped — silently, since loading cannot
      report. Loading now falls back to a salvage pass that re-reads the document as a
      raw table and keeps every top-level key that still deserializes, dropping only
      those that do not
- [x] The file is meant to be hand-edited, so this is the granularity a user can act on:
      the setting they got wrong reverts to its default and everything else survives
- [x] Load still **never fails** — an absent, unreadable or wholly unparsable file yields
      defaults, and the app starts (§7: zero configuration is a working configuration)

---

### Listing colours (§4)

- [x] Entries are **coloured by category**: folders, symlinks, hidden (dotfiles
      and dotfolders), documents, data, source code, archives, images, media, and
      executables. Anything unclaimed keeps the default foreground rather than
      being forced into a bucket
- [x] **A known extension outranks the exec bit.** `mode & 0o111` is a poor
      signal on macOS — files copied off an SMB/exFAT share, unpacked by an
      archiver that does not restore modes, or lifted out of a DMG all arrive
      `0755`/`0750` whatever they contain. Checking the bit first painted whole
      folders of PDFs a uniform green, so a name that positively identifies a
      document, dataset, archive, image or media file now wins over it
- [x] Source nobody executes is covered by the same rule: a `+x` `.html`, `.css`
      or `.rs` stays code-coloured, because the bit says nothing true about a
      file that only ever gets rendered or compiled
- [x] The bit still decides what it is good for: files whose name claims nothing
      (`build`, `myapp.x86_64`) and **interpreted scripts, where executable beats
      source** — a `0755` `deploy.sh` is green, the same file without the bit is
      blue, which is what NC, FAR and `ls` all do
- [x] **The same rule decides whether Enter runs a file.** `filetype::is_runnable`
      is "the bit won and nothing about the name objected" — literally
      `is_executable && classify(e) == Exec`, so the colour and the Enter
      behaviour cannot disagree. Enter on a `0750` PDF now opens Preview instead
      of trying to execute it; previously it executed
- [x] Hidden outranks folder, so a dotfolder reads as hidden rather than as a
      folder; unreadable and broken-symlink rows keep their own marker styling and
      are never overpainted by a type colour
- [x] **Core-authored**: `fm_core::filetype` owns the one extension table and
      stamps every entry with its category during `list_dir`. The frontend holds
      only a category → CSS-class map, so the panel and the execute-vs-launch
      decision cannot drift apart (§3). Colour *values* are CSS custom properties
      in `app.css`, so a theme remaps them without touching the classifier

**Known limit.** The extension table is compiled in, not config-driven. `Config`
carries a `theme` id that nothing reads yet — there is no palette loader for a
user table to hang off, and building one is its own slice (§7).

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

### Settings UI (F2)

The framework and the Appearance page have **shipped** — see
[Settings (§7)](#settings-7) under Implemented. What is left:

Everything the app persists was configurable **only** by hand-editing
`~/Library/Application Support/dimnav/config.toml` today. That is the
configuration story §7 asks for, but it is not a discoverable one: nothing in the
app said which settings existed. One of them still is not config-driven at all —
`Config` carries no shortcuts, so `get_keymap` returns the compiled-in
`default_keymap()`, which is the exact inverse of the CLAUDE.md rule that
shortcuts and theme values come from config.

**F2** opens a large popup shaped like the F1 help book: a vertical page rail,
one page per configurable area. The core authors the whole settings model as data
— pages, field labels, option lists, defaults, validation — and the renderer
paints controls by kind, the arrangement that already keeps the help book free of
business logic (§3). Pages are written against a new `SettingsPage` extension
point beside `HelpTopic` (§6a). Changes apply and persist immediately, the way
every existing `set_*` command already behaves; Esc closes, and each row resets
to its default on its own.

- [ ] **Shortcuts page** (§6) — makes shortcuts config-driven for the first time.
      `Config` gains a `[[shortcuts]]` **override** list, not a keymap dump, so a
      later release can still add bindings without a stale copy shadowing them;
      `config::keymap` merges the overrides under the defaults, and `get_keymap`
      and the F1 help screen both read that instead of `default_keymap()`.
      Remapping by pressing the chord, per-row reset, and **conflict detection**
      within a context. *Supersedes the previous "keybinding remapping and
      conflict detection" item*
- [ ] **The remaining pages** — Panels (start directory, sort, hidden files, view
      mode, per panel), Files (Trash default, the editor size cap, the
      `[[associations]]` table), Viewer & Editor, Terminal (scrollback, shell),
      Watching (all eight `[watch]` values). Guarded by a two-way test in the
      spirit of `catalog_covers_the_default_keymap`: every path in a serialized
      `Config::default()` must be claimed by some page, so a new config field
      cannot ship without a row in the UI to reach it
- [ ] **Custom theme editor** — duplicate a bundled theme into a user theme, then
      override any of the 23 palette tokens, with live preview and per-token
      revert to the base. Only the keys actually changed are written, so a
      UI-authored `themes/<id>.toml` stays as small as a hand-written one. The
      values are not all hex — `accent-dim` is an `rgba(…)` — so each row needs a
      swatch *and* a text field, or alpha is silently destroyed

### Elsewhere (from SPEC §8)

- [ ] **Window transparency** (§4) — the palette landed; opacity needs
      `macosPrivateApi` + a transparent window before a translucent `--bg` means
      anything
- [ ] Configurable **fonts** and row height (§4) — the theme file is the place for
      them; nothing reads a font token yet
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

## Distribution

The app ships as **dimnav**, macOS-only for now, signed with a Developer ID
certificate and notarized so it installs without Gatekeeper warnings. The Mac App
Store is deliberately out of scope: its mandatory sandbox would break the
built-in terminal and the `osascript` privilege elevation, and would force
security-scoped bookmarks on every panel path.

- [x] Named **dimnav**, bundle identifier `com.dimnav.desktop` (reverse DNS of
      `dimnav.com`), with a one-time migration that renames the old
      `file-manager` config directory so existing settings and terminal history
      carry across
- [x] App icon generated from the emblem by `scripts/make-icon.py` — superellipse
      mask on the Big Sur 824-in-1024 grid. `npm run icon -- --preview` writes a
      contact sheet down to 16px, which is where icon detail dies
- [x] `bundle.macOS` block with `minimumSystemVersion`, category, copyright, and
      an `Entitlements.plist` that is deliberately empty and deliberately **not**
      sandboxed
- [x] `Info.plist` carrying the TCC usage strings for Desktop, Documents,
      Downloads, removable and network volumes (§3) — without them the permission
      prompt is blank, and on recent macOS may be denied outright
- [x] Version single-sourced to the workspace `Cargo.toml`; `npm run bump <ver>`
      drags npm's manifests along
- [x] MIT `LICENSE`, `README`, `CONTRIBUTING`, and pinned toolchains
      (`rust-toolchain.toml`, `.nvmrc`) for reproducible CI builds
- [x] CI on every PR: clippy under `-D warnings`, the full test suite, the Svelte
      typecheck, and a guard that fails if `bindings.ts` is stale
- [x] Tag-triggered release workflow producing a signed, notarized Apple Silicon
      `.dmg` as a **draft** release. Linux and Windows jobs are present but
      commented out until those platforms are actually implemented
- [x] Signed auto-updates against the GitHub release feed
- [x] Static site in `site/`, deployed to GitHub Pages, resolving the current
      download from the releases API with a no-JS fallback
- [x] Apple Developer Program enrolment and the signing secrets (manual, external) —
      procedure and the secret inventory are in `docs/RELEASE.md`
- [ ] A screenshot for the README and the site

---

## Backfill needed

Shipped and working, but not yet itemised here — a follow-up task expands each of
these into checkboxes:

- Two-panel navigation and the single cursor-index state machine (§5.2)
- Per-panel column/view modes including detailed mode, persisted (§5.2, §5.8)
- Multi-selection model (§5.3)
- Copy / move with the editable destination prompt and FAR collision dialogs (§5.4a)
- Delete with the persisted "Move to Trash" checkbox (§5.4a)
- Create directory, rename, recursive folder-size calculation (§5.4)
- Sorting modes and the hidden-files toggle, persisted per panel (§5.8)
- Open / F3 View / F4 Edit routing, external and embedded (§5.5)
- Embedded viewer and editor: text/hex/image, search, goto, wrap, encodings,
  atomic save with conflict detection (§5.5)
- Red-background failure dialogs and macOS-native privilege elevation (§5.4b, §5.6)
- TOML config persistence (§7)
