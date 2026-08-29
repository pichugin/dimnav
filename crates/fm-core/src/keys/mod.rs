//! How a chord is spelled for a human (§6).
//!
//! [`crate::config::default_keymap`] stores chords in the internal form the
//! frontend builds from a `KeyboardEvent` — `"Meta+Shift+t"`, `"ArrowDown"`,
//! `" "`. This module turns those into what the user actually reads: `⌘⇧T`, `↓`,
//! `Space`.
//!
//! It lives on its own rather than inside [`crate::help`] because two very
//! different surfaces need it — the F1 help book and the F-key hint bars — and
//! the keymap itself carries the rendered labels (`KeyBinding::labels`), so
//! `config` depends on this too. It is also the single place that knows a
//! platform writes `⌘S` and another writes `Ctrl+S`, which is what keeps that
//! decision out of the renderer entirely (CLAUDE.md).

/// How one modifier part of a chord is written, separator included. macOS spells
/// modifiers as glyphs run together (`⌘⇧T`); everywhere else they keep their
/// names and the `+` separators (`Ctrl+Shift+T`).
fn modifier_symbol(part: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        match part {
            "Ctrl" => "⌃",
            "Meta" => "⌘",
            "Alt" => "⌥",
            "Shift" => "⇧",
            other => other,
        }
        .to_string()
    }
    #[cfg(not(target_os = "macos"))]
    {
        match part {
            "Meta" => "Super+".to_string(),
            other => format!("{other}+"),
        }
    }
}

/// The printable name of the non-modifier key in a chord.
fn key_label(key: &str) -> String {
    match key {
        " " => "Space".to_string(),
        "ArrowUp" => "↑".to_string(),
        "ArrowDown" => "↓".to_string(),
        "ArrowLeft" => "←".to_string(),
        "ArrowRight" => "→".to_string(),
        "PageUp" => "PgUp".to_string(),
        "PageDown" => "PgDn".to_string(),
        "Backspace" => "⌫".to_string(),
        "Escape" => "Esc".to_string(),
        "Delete" => "Del".to_string(),
        // Printable keys are reported lower-case when a modifier is held
        // (config::default_keymap), but read as shortcuts upper-case.
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.to_string(),
    }
}

/// Turn an internal chord string (`"Meta+Shift+t"`, `"ArrowDown"`, `" "`) into
/// what the user sees (`"⌘⇧T"`, `"↓"`, `"Space"`).
///
/// Modifier order is the chord's own — `Ctrl+Meta+Alt+Shift` — rather than the
/// macOS HIG order, so the output matches the F-key hint bar, which is painted
/// from these same labels.
pub fn display_chord(chord: &str) -> String {
    let mut parts = chord.split('+').peekable();
    let mut out = String::new();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            out.push_str(&key_label(part));
        } else {
            out.push_str(&modifier_symbol(part));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn chords_render_the_way_the_fkey_bar_does() {
        assert_eq!(display_chord("Meta+Shift+t"), "⌘⇧T");
        assert_eq!(display_chord("Meta+t"), "⌘T");
        assert_eq!(display_chord("Meta+s"), "⌘S");
        assert_eq!(display_chord("Ctrl+h"), "⌃H");
        assert_eq!(display_chord("Shift+F6"), "⇧F6");
        assert_eq!(display_chord("Ctrl+Enter"), "⌃Enter");
    }

    /// The non-macOS branch of `modifier_symbol`. It is what a Windows or Linux
    /// user will read off the F-key bar — `Ctrl+S` for save — so it is worth
    /// pinning even though CI only runs the macOS side.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn chords_spell_their_modifiers_out_off_macos() {
        assert_eq!(display_chord("Ctrl+s"), "Ctrl+S");
        assert_eq!(display_chord("Ctrl+Shift+t"), "Ctrl+Shift+T");
        assert_eq!(display_chord("Meta+f"), "Super+F");
        assert_eq!(display_chord("Shift+F6"), "Shift+F6");
    }

    #[test]
    fn named_keys_render_as_symbols() {
        assert_eq!(display_chord("ArrowDown"), "↓");
        assert_eq!(display_chord("PageUp"), "PgUp");
        assert_eq!(display_chord("Backspace"), "⌫");
        assert_eq!(display_chord("Escape"), "Esc");
        assert_eq!(display_chord(" "), "Space");
        assert_eq!(display_chord("*"), "*");
        assert_eq!(display_chord("F5"), "F5");
    }
}
