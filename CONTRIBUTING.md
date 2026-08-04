# Contributing to dimnav

Thanks for taking an interest. Bug reports, ideas, and pull requests are all
welcome.

## Before you start

**Read `docs/SPEC.md`.** dimnav has a lot of behaviour that looks like a quirk
and is actually specified — the cursor's column-then-page traversal, the parent
directory landing on the folder you just left, the exact wording and button order
of the collision dialog. If you change one of those without reading why it is
that way, the change will be rejected for reasons that will feel arbitrary. They
are not; they are in the spec.

`docs/FEATURES.md` tracks what is built versus planned. If you are picking
something up, it is worth checking there first.

## The one architectural rule

**All application logic lives in the Rust core (`crates/fm-core`). The frontend
renders and forwards input, and does nothing else.**

This is what keeps the UI swappable — the whole point of the boundary is that the
Svelte layer could be replaced with Iced or egui without rewriting anything that
matters. Concretely:

- Filesystem work, navigation and selection state, file operations, config, and
  help content are all core concerns. `crates/fm-core` must never depend on
  Tauri.
- `src-tauri` is the adapter: command handlers that marshal between the core and
  the webview. Business logic does not belong here either.
- `src` is the renderer. If you find yourself deciding *what* something means
  rather than *how it looks*, that decision belongs in the core.
- Keybindings and theme values come from config. Never hardcode them.

If a change seems to need logic in the frontend, that is worth raising in an
issue before writing it — usually it means the IPC contract needs a new field.

## Development

```bash
npm install
npm run tauri dev
```

Before opening a pull request:

```bash
cargo test --workspace     # must pass
cargo clippy --workspace --all-targets -- -D warnings
npm run check              # Svelte/TypeScript typecheck
```

CI runs exactly these, plus a guard that fails if `src/lib/bindings.ts` is stale. There is
no `cargo fmt` gate — the codebase is hand-formatted in places where rustfmt would hurt
readability.

### Changing the IPC contract

DTOs are defined once in Rust and exported to TypeScript. After editing anything
in `crates/fm-core/src/types.rs` or a command signature:

```bash
npm run bindings
```

`src/lib/bindings.ts` is generated — **never edit it by hand.** It also
regenerates automatically on every debug launch, so a stale copy usually means
you have not run the app since your change.

### Adding a keybinding

Two places, and a test enforces they agree:

1. `crates/fm-core/src/config/mod.rs` — `default_keymap()`, the chord itself.
2. `crates/fm-core/src/actions/mod.rs` — the catalog entry with the
   human-readable title and description that F1 renders.

Miss either one and `catalog_covers_the_default_keymap` will tell you.

### Icons

The app icon is generated from the emblem artwork:

```bash
npm run icon -- --preview          # writes the masters plus a contact sheet
npm run tauri icon src-tauri/icon-master.png
```

Check the 32px and 16px cells on the contact sheet — that is where icon detail
disappears.

## Pull requests

- Keep them focused; one concern per PR.
- Match the surrounding code's style, including its comment density. The codebase
  explains *why*, not *what*.
- Add tests for logic in the core. It is pure and fast to test, which is much of
  the reason the boundary exists.
- If behaviour changes, update `docs/FEATURES.md` in the same PR.

`main` is protected: it cannot be deleted or force-pushed, and a change lands through a
pull request whose `check` run is green. Zero approvals are required, because a sole
maintainer cannot review their own work into existence — the gate is CI, not a second
opinion.

The maintainer holds an admin bypass and uses it for release commits and documentation.
That is deliberate, not an oversight: `docs/RELEASE.md` pushes the version commit to
`main` directly, and routing it through a squash merge would rewrite the SHA out from
under the tag that has to follow it. Changes to behaviour go through a pull request like
everyone else's.

## Releasing

`docs/RELEASE.md` — the tag-triggered pipeline, the signing secrets, and what to verify
before publishing a draft.

## Reporting bugs

Include your macOS version, the dimnav version (Help → About, or F1), and what
you did. For anything involving file operations, the exact paths matter more than
you would expect — permissions, symlinks, and network volumes are where the
interesting failures live.
