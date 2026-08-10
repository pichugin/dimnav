# Changelog

All notable changes to dimnav are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Ctrl+=** shows the active panel's folder on the other panel, without moving
  the keyboard off the panel you pressed it from. It is inert while the terminal,
  viewer, editor or a dialog has focus.

### Fixed

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
