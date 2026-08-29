# Changelog

All notable changes to dimnav are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Ctrl+=** shows the active panel's folder on the other panel, without moving
  the keyboard off the panel you pressed it from. It is inert while the terminal,
  viewer, editor or a dialog has focus.
- **Shift+PageUp / Shift+PageDown / Shift+Home / Shift+End** extend the
  range-toggle gesture below to whole pages and to the ends of the listing.

### Changed

- **The editor saves with ⌘S instead of F2.** F2 was the Far/MC inheritance, but
  ⌘S is the key macOS users actually reach for; on Windows and Linux the same
  binding resolves to Ctrl+S. F2 no longer saves — it keeps its viewer meaning
  (word wrap).
- **The F-key bars are now generated from the live keymap**, the way the F1 help
  screen already was. All three bars — panels, viewer, editor — name actions, and
  the chord printed beside each one comes from the core, so a rebind moves the bar
  on its own and an unbound action drops off it. On macOS every existing label is
  unchanged.

- **Shift+Arrow now toggles the whole range the cursor sweeps**, instead of
  marking a single entry. Shift+Right used to jump a column and mark only the one
  file it started on, silently skipping the seven it flew over, and it could never
  unmark anything. Now every entry swept over is flipped, as if Space had been
  pressed on each — so sweeping back over a marked run clears it. The entry the
  cursor lands on is left untouched, so repeated presses paint one continuous run
  and the cursor always rests on the next entry not yet touched. When the motion
  is clamped and the cursor cannot move, the entry under it is flipped in place,
  which is how the last file gets marked.

### Fixed

- **A broken line in `config.toml` no longer costs the whole file.** TOML
  deserialization fails the entire document on one wrong value, so a hand-edited
  `trash_default = yes` used to take the panel directories, the file associations
  and the Trash flag down with it — silently, because loading deliberately cannot
  report. Loading now falls back to a salvage pass that keeps every top-level key
  that still parses and drops only the one that does not, so a typo costs the
  setting it is in. An absent or wholly unparsable file still yields defaults.

- Closing the Esc terminal curtain now hands the keyboard back to wherever it was
  before the curtain went up. Pressing Esc twice from a panel used to bring the
  panels back while the command line silently kept the keys, so arrows and Enter
  went to the prompt until you pressed Cmd+T. Leaving the curtain with Cmd+Shift+T
  behaves the same way; starting from the prompt still leaves you at the prompt.

## [0.1.0] - 2026-08-01

First public release.

### Added

- Two-panel navigation with a single cursor-index state machine: arrow keys step
  through columns, then paginate, and `..` returns to the parent with the cursor
  landing on the folder just left.
- Per-panel view modes (1–3 columns plus a detailed single column), sort order,
  and hidden-file toggle, each persisted across restarts.
- Asynchronous, cancellable copy, move, rename, create-directory and delete, with
  progress reporting, an editable destination prompt, and Far Manager-style
  collision handling.
- Delete with a persisted "Move to Trash" checkbox, off by default.
- Privilege elevation through the macOS-native authorization prompt; the app
  never handles the password.
- Embedded terminal with command history, sharing the panel's directory.
- Embedded viewer and editor (F3/F4) with text, hex and image modes, search,
  goto, wrapping, encoding detection, and atomic save with conflict detection.
- Quick search within a panel.
- F1 help: an About topic and a shortcut list generated from the live keymap.
- Signed in-app updates, surfaced in Help → About.

[Unreleased]: https://github.com/pichugin/dimnav/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/pichugin/dimnav/releases/tag/v0.1.0
