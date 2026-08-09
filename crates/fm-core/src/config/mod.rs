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
use std::sync::Once;

use crate::types::{Config, KeyBinding};

/// Directory name under the OS config root.
const APP_DIR: &str = "dimnav";

/// What [`APP_DIR`] was called before the app was named dimnav. See [`migrate`].
const LEGACY_APP_DIR: &str = "file-manager";

const FILE_NAME: &str = "config.toml";

/// Absolute path of the config file — `~/Library/Application Support/dimnav/
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
    ensure_migrated();
    config_path().map(|p| load_from(&p)).unwrap_or_default()
}

/// Move a pre-rename config directory to its new name, once per process.
///
/// The app shipped as "File Manager" before it was named dimnav, so anyone who
/// ran it then has settings, panel state, and terminal history under the old
/// directory name. Everything in there is path-derived from [`config_path`], so
/// renaming the directory carries the whole lot across in one move.
///
/// Call this from every entry point that reads persisted state, not just
/// [`load`] — terminal history builds its path from [`config_path`] directly and
/// may well be read first. The [`Once`] makes the extra calls free and keeps the
/// rename from racing itself if two surfaces load concurrently.
pub fn ensure_migrated() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if let Some(root) = dirs::config_dir() {
            migrate(&root.join(LEGACY_APP_DIR), &root.join(APP_DIR));
        }
    });
}

/// The testable half of [`ensure_migrated`].
///
/// Only ever renames into a name that does not exist yet, so a live config can
/// never be clobbered and a half-finished move cannot compound. Errors are
/// swallowed deliberately: losing preferences is annoying, but refusing to start
/// because a rename failed would be worse (§7).
fn migrate(legacy: &Path, current: &Path) {
    if legacy.is_dir() && !current.exists() {
        let _ = std::fs::rename(legacy, current);
    }
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
/// Bindings are scoped by **context** — `"panels"`, `"viewer"`, `"editor"`,
/// `"terminal"`, `"help"` — because the same F-key means different things
/// depending on which surface owns the keyboard (F4 edits the file under the
/// cursor from the panels, and toggles hex inside the viewer). The frontend
/// consults the context of whichever surface is on top.
///
/// Every action id here must also appear in [`crate::actions::catalog`], which
/// carries the human-readable text the F1 help screen renders. A test enforces
/// that in both directions.
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
    let terminal = |action: &str, keys: &[&str]| KeyBinding {
        context: "terminal".to_string(),
        action: action.to_string(),
        keys: keys.iter().map(|k| k.to_string()).collect(),
    };
    let help = |action: &str, keys: &[&str]| KeyBinding {
        context: "help".to_string(),
        action: action.to_string(),
        keys: keys.iter().map(|k| k.to_string()).collect(),
    };
    let search = |action: &str, keys: &[&str]| KeyBinding {
        context: "search".to_string(),
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
        // Push the active panel's folder onto the passive one. Ctrl-based for the
        // same reason as the view state below, and `=` reads as "make them
        // equal". A printable key carries no `Shift` part, and `=` is already the
        // unshifted character on the key, so the chord is plain `Ctrl+=`.
        bind("panel.equalize", &["Ctrl+="]),
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
        // Ctrl+R, not Cmd+R, for two reasons: it follows the rule above (Cmd is
        // reserved for chords that mirror a real macOS system shortcut, which
        // refresh is not), and Cmd+R is webview reload in a dev build, which
        // would throw away the app's whole state.
        bind("panel.refresh", &["Ctrl+r"]),
        bind("panel.toggle_hidden", &["Ctrl+h"]),
        bind("panel.cycle_sort", &["Ctrl+s"]),
        bind("panel.view_1", &["Ctrl+1"]),
        bind("panel.view_2", &["Ctrl+2"]),
        bind("panel.view_3", &["Ctrl+3"]),
        bind("panel.view_detailed", &["Ctrl+4"]),
        // --- Quick search (§5.9) ---------------------------------------------
        // Cmd+F opens a box in the active panel's corner; from there plain
        // characters are the query, so nothing above has to give up a key. It is
        // entered deliberately rather than by typing into the panel, which is
        // what leaves Space / `*` / `-` above meaning exactly what they meant.
        bind("search.start", &["Meta+f"]),
        // --- Inside the quick-search box (§5.9) -------------------------------
        // Both exits close the box and leave the cursor on the match. `Enter` is
        // bound HERE so it cannot reach `nav.enter` — closing the box and opening
        // the folder under it on one press would make the search unusable for
        // finding a folder you did not want to enter. Opening it takes a second
        // Enter, exactly as the curtain takes a second Escape.
        search("search.close", &["Escape", "Enter"]),
        search("search.backspace", &["Backspace"]),
        // --- Terminal, reachable from the panels (§5.7) ----------------------
        // A chord normally carries no explicit `Shift` for a printable key, since
        // the shift is baked into the character (`*` is reported as `*`). That
        // does not survive a second modifier: macOS reports the *unshifted*
        // character while Command is held, so Cmd+Shift+T would be
        // indistinguishable from Cmd+T. With any non-shift modifier present the
        // frontend therefore spells `Shift` out and lower-cases the letter —
        // hence `Meta+Shift+t` rather than `Meta+T`.
        bind("terminal.focus", &["Meta+t"]),
        bind("terminal.toggle_half", &["Meta+Shift+t"]),
        // The Esc curtain (§6): panels aside, full terminal, and back.
        bind("terminal.curtain", &["Escape"]),
        // Append the name under the cursor to the command line without leaving
        // the panel, so repeated presses build a multi-file command (§5.7).
        bind("terminal.insert_name", &["Ctrl+Enter"]),
        // --- Terminal prompt (§5.7) -------------------------------------------
        // Anything not bound here is text the user is typing and falls through
        // to the input, exactly as unbound keys do in the editor.
        terminal("terminal.run", &["Enter"]),
        terminal("terminal.blur", &["Meta+t"]),
        terminal("terminal.toggle_half", &["Meta+Shift+t"]),
        terminal("terminal.curtain", &["Escape"]),
        // Interrupts a running command; with nothing running it clears the
        // prompt — what a shell does, and what the user asked for.
        terminal("terminal.interrupt", &["Ctrl+c"]),
        terminal("terminal.history_prev", &["ArrowUp"]),
        terminal("terminal.history_next", &["ArrowDown"]),
        terminal("terminal.scroll_up", &["PageUp"]),
        terminal("terminal.scroll_down", &["PageDown"]),
        terminal("terminal.clear_buffer", &["Ctrl+l"]),
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
        // --- Help (§6) -------------------------------------------------------
        // F1 is bound in every context, so help is always one key away no matter
        // which surface owns the keyboard.
        bind("help.open", &["F1"]),
        viewer("help.open", &["F1"]),
        editor("help.open", &["F1"]),
        terminal("help.open", &["F1"]),
        // The help popup's own context. `Escape` MUST be declared here: it is
        // bound in every other context (terminal.curtain, viewer.close,
        // editor.close), so without this entry closing help would fall through
        // and yank the terminal curtain instead.
        help("help.close", &["Escape", "F1", "F10"]),
        // Tab cycles the topic rail in both directions, wrapping at either end.
        help("help.next_topic", &["Tab"]),
        help("help.prev_topic", &["Shift+Tab"]),
        // Left/Right and Home/End are deliberately left unbound so they keep
        // moving the caret in the search field.
        help("help.scroll_up", &["ArrowUp"]),
        help("help.scroll_down", &["ArrowDown"]),
        help("help.page_up", &["PageUp"]),
        help("help.page_down", &["PageDown"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_dir;
    use crate::types::{FileAssociation, SortMode, ViewMode};

    fn temp_path() -> PathBuf {
        unique_dir("fm_core_cfg").join(FILE_NAME)
    }

    /// A fresh, empty directory to hang legacy/current config dirs off.
    fn temp_root() -> PathBuf {
        unique_dir("fm_core_mig")
    }

    #[test]
    fn migrate_renames_a_legacy_dir_and_keeps_its_contents() {
        let root = temp_root();
        let (legacy, current) = (root.join(LEGACY_APP_DIR), root.join(APP_DIR));
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(FILE_NAME), "trash_default = true\n").unwrap();
        std::fs::write(legacy.join("history"), "ls -la\n").unwrap();

        migrate(&legacy, &current);

        assert!(!legacy.exists(), "legacy dir should be gone after the move");
        // Everything path-derived from config_path rides along, history included.
        assert!(current.join(FILE_NAME).is_file());
        assert_eq!(
            std::fs::read_to_string(current.join("history")).unwrap(),
            "ls -la\n"
        );
    }

    #[test]
    fn migrate_never_clobbers_an_existing_config() {
        let root = temp_root();
        let (legacy, current) = (root.join(LEGACY_APP_DIR), root.join(APP_DIR));
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(FILE_NAME), "trash_default = true\n").unwrap();
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join(FILE_NAME), "trash_default = false\n").unwrap();

        migrate(&legacy, &current);

        assert_eq!(
            std::fs::read_to_string(current.join(FILE_NAME)).unwrap(),
            "trash_default = false\n",
            "the live config must win"
        );
        assert!(legacy.exists(), "legacy dir is left alone, not deleted");
    }

    #[test]
    fn migrate_is_a_noop_without_a_legacy_dir() {
        let root = temp_root();
        let (legacy, current) = (root.join(LEGACY_APP_DIR), root.join(APP_DIR));

        migrate(&legacy, &current);

        assert!(!current.exists(), "must not conjure an empty config dir");
    }

    #[test]
    fn round_trips_through_toml() {
        let path = temp_path();
        let mut cfg = Config {
            trash_default: true,
            ..Config::default()
        };
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
