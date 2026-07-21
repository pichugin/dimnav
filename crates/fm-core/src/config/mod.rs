//! Configuration system (SPEC §7).
//!
//! Loads and saves a human-readable TOML document from the OS config directory
//! (`~/Library/Application Support/<AppName>/` on macOS), with working defaults so
//! the app is fully usable with zero configuration. Owns keybindings, theme /
//! transparency, the file-type→application map, the persisted per-panel state
//! (view/sort/hidden, §5.8), and the global Trash-checkbox flag (§5.4a).
//!
//! Path resolution lives here rather than in the Tauri adapter: `dirs` is a tiny,
//! platform-agnostic crate, so the whole config system stays in the core and the
//! adapter stays pure marshalling (CLAUDE.md).

use std::path::{Path, PathBuf};

use crate::types::{Config, KeyBinding};

/// Directory name under the OS config root.
const APP_DIR: &str = "file-manager";
const FILE_NAME: &str = "config.toml";

/// Absolute path of the config file — `~/Library/Application Support/file-manager/
/// config.toml` on macOS (§7), the platform equivalent elsewhere. `None` only if
/// the OS config directory cannot be determined, in which case the app runs on
/// defaults and simply does not persist.
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_DIR).join(FILE_NAME))
}

/// Load config from disk, falling back to [`Config::default`] when the file is
/// absent, unreadable, or unparsable. Never fails: a broken config must not stop
/// the app from starting (§7 — zero configuration is a working configuration).
pub fn load() -> Config {
    config_path().map(|p| load_from(&p)).unwrap_or_default()
}

/// Persist config to disk as TOML. Errors are swallowed by design — a failed
/// preference write must never break the file operation that triggered it.
pub fn save(config: &Config) {
    if let Some(path) = config_path() {
        let _ = save_to(&path, config);
    }
}

/// [`load`] against an explicit path (the testable half).
pub fn load_from(path: &Path) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

/// [`save`] against an explicit path (the testable half). Writes to a sibling
/// temp file and renames, so an interrupted write can never leave a truncated
/// config behind.
pub fn save_to(path: &Path, config: &Config) -> Result<(), String> {
    let text = toml::to_string_pretty(config).map_err(|e| format!("could not encode config: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("could not create config dir: {e}"))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("could not write config: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("could not replace config: {e}"))
}

/// The default keymap, sourced from the core so the webview never hardcodes keys
/// (SPEC §6). `keys` are **chord strings** the frontend builds from a
/// `KeyboardEvent`: modifiers in the fixed order `Ctrl+Meta+Alt+Shift+<key>`,
/// where `<key>` is `KeyboardEvent.key`. `Shift` is only present for named keys
/// (e.g. `Shift+ArrowDown`); for printable keys the shift is already baked into
/// the character (`*` is `Shift+8` but reports as `"*"`). Remapping, persistence,
/// and conflict detection are a later slice.
///
/// Bindings are scoped by **context** — `"panels"`, `"viewer"`, `"editor"` —
/// because the same F-key means different things depending on which surface owns
/// the keyboard (F4 edits the file under the cursor from the panels, and toggles
/// hex inside the viewer). The frontend consults the context of whichever
/// surface is on top.
pub fn default_keymap() -> Vec<KeyBinding> {
    let bind = |action: &str, keys: &[&str]| KeyBinding {
        context: "panels".to_string(),
        action: action.to_string(),
        keys: keys.iter().map(|k| k.to_string()).collect(),
    };
    let viewer = |action: &str, keys: &[&str]| KeyBinding {
        context: "viewer".to_string(),
        action: action.to_string(),
        keys: keys.iter().map(|k| k.to_string()).collect(),
    };
    let editor = |action: &str, keys: &[&str]| KeyBinding {
        context: "editor".to_string(),
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
        // Open / View / Edit external tools (§5.5). Enter (nav.enter) opens files
        // via the system default; F3/F4 route to the configured viewer/editor.
        bind("open.view", &["F3"]),
        bind("open.edit", &["F4"]),
        // Per-panel view state (§5.8). Ctrl-based so nothing collides with macOS
        // system shortcuts (which use Cmd) or the bindings above. Printable keys
        // carry no `Shift` in a chord, and `KeyboardEvent.key` reports them
        // lower-case, hence `Ctrl+h` rather than `Ctrl+H`.
        bind("panel.toggle_hidden", &["Ctrl+h"]),
        bind("panel.cycle_sort", &["Ctrl+s"]),
        bind("panel.view_1", &["Ctrl+1"]),
        bind("panel.view_2", &["Ctrl+2"]),
        bind("panel.view_3", &["Ctrl+3"]),
        bind("panel.view_detailed", &["Ctrl+4"]),
        // --- Embedded viewer (§5.5) -----------------------------------------
        // FAR's assignments, with one deliberate choice: F4 toggles hex (as it
        // does in FAR's viewer) and F6 does the view↔edit swap, so F4 never
        // means two things at once.
        viewer("viewer.close", &["Escape", "F10"]),
        viewer("viewer.toggle_wrap", &["F2"]),
        viewer("viewer.toggle_hex", &["F4"]),
        viewer("viewer.goto", &["F5"]),
        viewer("viewer.to_edit", &["F6"]),
        viewer("viewer.search", &["F7"]),
        viewer("viewer.search_next", &["Shift+F7"]),
        viewer("viewer.line_up", &["ArrowUp"]),
        viewer("viewer.line_down", &["ArrowDown"]),
        viewer("viewer.col_left", &["ArrowLeft"]),
        viewer("viewer.col_right", &["ArrowRight"]),
        viewer("viewer.page_up", &["PageUp"]),
        viewer("viewer.page_down", &["PageDown"]),
        viewer("viewer.home", &["Home"]),
        viewer("viewer.end", &["End"]),
        // --- Embedded editor (§5.5) -----------------------------------------
        // Everything else in the editor is text entry, so only the commands are
        // bound; Esc prompts when the buffer is dirty.
        editor("editor.save", &["F2"]),
        editor("editor.to_view", &["F6"]),
        editor("editor.close", &["Escape"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileAssociation, SortMode, ViewMode};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("fm_core_cfg_{nanos}"))
            .join(FILE_NAME)
    }

    #[test]
    fn round_trips_through_toml() {
        let path = temp_path();
        let mut cfg = Config::default();
        cfg.trash_default = true;
        cfg.left_panel.view_mode = ViewMode::Detailed;
        cfg.left_panel.sort_mode = SortMode::Size;
        cfg.left_panel.show_hidden = false;
        cfg.right_panel.start_dir = Some("/tmp".to_string());
        cfg.associations.push(FileAssociation {
            extensions: vec!["md".to_string()],
            open: None,
            view: None,
            edit: Some("Visual Studio Code".to_string()),
        });

        save_to(&path, &cfg).unwrap();
        let back = load_from(&path);

        assert!(back.trash_default);
        assert_eq!(back.left_panel.view_mode, ViewMode::Detailed);
        assert_eq!(back.left_panel.sort_mode, SortMode::Size);
        assert!(!back.left_panel.show_hidden);
        assert_eq!(back.right_panel.start_dir.as_deref(), Some("/tmp"));
        assert_eq!(back.associations.len(), 1);
        assert_eq!(back.associations[0].edit.as_deref(), Some("Visual Studio Code"));

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn missing_or_broken_file_yields_defaults() {
        let defaults = Config::default();
        // Absent file.
        let absent = load_from(Path::new("/definitely/not/a/real/path/config.toml"));
        assert_eq!(absent.theme, defaults.theme);
        assert!(!absent.trash_default);

        // Unparsable file.
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "this is not = = toml").unwrap();
        let broken = load_from(&path);
        assert_eq!(broken.theme, defaults.theme);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn partial_file_fills_in_defaults() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "trash_default = true\n").unwrap();

        let cfg = load_from(&path);
        assert!(cfg.trash_default);
        // Everything else falls back — hidden files shown, 2-column brief (§5.8).
        assert!(cfg.left_panel.show_hidden);
        assert_eq!(cfg.right_panel.view_mode, ViewMode::default());

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
