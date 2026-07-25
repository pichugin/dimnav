# CLAUDE.md

## What this is
A fast, keyboard-first, **two-panel file manager** (Norton Commander / FarManager / Midnight Commander lineage), built with **Tauri + Rust**. macOS first; Windows/Linux later.

**Full specification: `./docs/SPEC.md` — read it before implementing anything.** This file holds only the durable rules that must survive long sessions.

**Feature checklist: `./docs/FEATURES.md`** — what is built vs. planned. Update it as part of any slice that ships or plans a feature.

## Non-negotiable architecture rules
- **All logic lives in the Rust core** (file ops, navigation/selection state, config, plugin host) behind a **clean, typed IPC contract**.
- **The frontend is a thin, swappable rendering layer.** No business logic in the webview. This is what keeps a future swap to Iced/egui possible — do not violate it.
- **Structure every feature as a module against defined extension points** (see SPEC §6a), even before the public plugin API ships. Terminal, editor, and archive support should look like plugins internally.
- File operations are **async, cancellable, and return structured results**. Never block the UI thread.
- All keybindings and theme values are **config-driven, never hardcoded**.

## Core behaviors that are easy to get wrong (get these right)
- **Navigation is one cursor-index state machine** (SPEC §5.2): Right steps columns then paginates down then lands on the last file; Left mirrors it upward to the first entry. The first entry is always `..`; Enter on `..` goes to parent and **auto-positions the cursor on the folder just exited**.
- **Column count is per-panel and selectable** (2-col default) plus a detailed single-column mode. Per-panel view/sort/hidden-file state is **persisted across restarts**.
- **Collision dialogs follow FarManager first, Midnight Commander as fallback.** Single file: Cancel/Skip/Overwrite. Multiple: adds Skip All/Overwrite All. F5/F6 show an **editable destination-path prompt** (accepts `..`, relative, absolute).
- **Deletion has a "Move to Trash" checkbox, OFF by default**, state persisted. No undo in v1.
- **Failures show a red-background dialog.** Privilege escalation goes through the **OS-native auth prompt** — the app never collects passwords itself.
- Hidden files **shown by default**; folders-first-by-name is the default sort.

## How to work
- **Build Phase 1 (MVP) only, in slices** — do not pull Phase 2+ scope forward. But leave the seams for the specified Phase 2 behaviors (Esc terminal curtain, Enter-to-execute, Ctrl+Enter filename insertion).
- Before writing feature code: scaffold the Tauri+Rust structure, then **propose the module boundaries and the IPC contract for review**.
- Keep the Rust backend platform-agnostic where reasonable; handle macOS TCC permission states explicitly.
- When the spec leaves a decision open (e.g. frontend framework), propose an option — but never at the cost of the hard rules above.
