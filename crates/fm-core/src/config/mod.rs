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

/// The default keymap for the navigation slice, sourced from the core so the
/// webview never hardcodes keys (SPEC §6). `keys` are frontend `KeyboardEvent.key`
/// values. Remapping, persistence, and conflict detection are a later slice; the
/// broader action set (F-keys, selection, etc.) joins as those slices land.
pub fn default_keymap() -> Vec<KeyBinding> {
    let bind = |action: &str, keys: &[&str]| KeyBinding {
        action: action.to_string(),
        keys: keys.iter().map(|k| k.to_string()).collect(),
    };
    vec![
        bind("cursor.up", &["ArrowUp"]),
        bind("cursor.down", &["ArrowDown"]),
        bind("cursor.left", &["ArrowLeft"]),
        bind("cursor.right", &["ArrowRight"]),
        bind("cursor.page_up", &["PageUp"]),
        bind("cursor.page_down", &["PageDown"]),
        bind("cursor.home", &["Home"]),
        bind("cursor.end", &["End"]),
        bind("panel.switch", &["Tab"]),
        bind("nav.enter", &["Enter"]),
        bind("nav.parent", &["Backspace"]),
    ]
}
