//! Configuration system (SPEC §7).
//!
//! Loads and saves a human-readable TOML document from the OS config directory
//! (`~/Library/Application Support/<AppName>/` on macOS), with working defaults so
//! the app is fully usable with zero configuration. Owns keybindings, theme /
//! transparency, the file-type→application map, the persisted per-panel state
//! (view/sort/hidden, §5.8), and the global Trash-checkbox flag (§5.4a).
//!
//! Phase 1: signatures and doc contracts only — the [`Config`] shape ships now;
//! TOML (de)serialization and the config-dir resolution land with feature work.

use crate::types::{Config, KeyBinding};

/// Load config from disk, falling back to [`Config::default`] when absent or
/// unreadable. Phase 1 stub: always returns defaults.
pub fn load() -> Config {
    Config::default()
}

/// Persist config to disk as TOML. Phase 1 stub: no-op.
pub fn save(config: &Config) {
    let _ = config;
}

/// The default keymap, sourced from the core so the webview never hardcodes keys
/// (SPEC §6). `keys` are **chord strings** the frontend builds from a
/// `KeyboardEvent`: modifiers in the fixed order `Ctrl+Meta+Alt+Shift+<key>`,
/// where `<key>` is `KeyboardEvent.key`. `Shift` is only present for named keys
/// (e.g. `Shift+ArrowDown`); for printable keys the shift is already baked into
/// the character (`*` is `Shift+8` but reports as `"*"`). Remapping, persistence,
/// and conflict detection are a later slice.
pub fn default_keymap() -> Vec<KeyBinding> {
    let bind = |action: &str, keys: &[&str]| KeyBinding {
        action: action.to_string(),
        keys: keys.iter().map(|k| k.to_string()).collect(),
    };
    vec![
        // Cursor motion (§5.2).
        bind("cursor.up", &["ArrowUp"]),
        bind("cursor.down", &["ArrowDown"]),
        bind("cursor.left", &["ArrowLeft"]),
        bind("cursor.right", &["ArrowRight"]),
        bind("cursor.page_up", &["PageUp"]),
        bind("cursor.page_down", &["PageDown"]),
        bind("cursor.home", &["Home"]),
        bind("cursor.end", &["End"]),
        // Panels & directory navigation.
        bind("panel.switch", &["Tab"]),
        bind("nav.enter", &["Enter"]),
        bind("nav.parent", &["Backspace"]),
        // Selection (§5.3).
        bind("selection.toggle", &[" "]),
        bind("selection.all", &["*"]),
        bind("selection.none", &["-"]),
        bind("select.up", &["Shift+ArrowUp"]),
        bind("select.down", &["Shift+ArrowDown"]),
        bind("select.left", &["Shift+ArrowLeft"]),
        bind("select.right", &["Shift+ArrowRight"]),
        // File operations (§5.4).
        bind("op.copy", &["F5"]),
        bind("op.move", &["F6"]),
        bind("op.rename", &["Shift+F6"]),
        bind("op.mkdir", &["F7"]),
        // Forward-delete key ("Delete"), distinct from Backspace (nav.parent).
        bind("op.delete", &["F8", "Delete"]),
    ]
}
