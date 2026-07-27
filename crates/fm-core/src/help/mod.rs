//! The F1 help book (§6).
//!
//! Assembles the topics shown in the help popup — currently **About** and
//! **Shortcuts** — from the live keymap and the [`crate::actions`] catalog. The
//! whole book is built core-side and handed over as data: the renderer picks a
//! topic and paints it, and makes no decision about what goes in it (CLAUDE.md —
//! the frontend is a thin, swappable layer).
//!
//! That includes the search filter. Deciding which rows match `"⌘ term"` is a
//! matching rule, not a paint instruction, so it lives here where it can be unit
//! tested and where a future Iced frontend inherits it for free.
//!
//! Topics are written against [`crate::plugin::HelpTopic`] rather than being
//! special-cased, so a plugin-contributed topic in Phase 5 is a push into
//! [`book`]'s registry and nothing more (SPEC §6a).

use crate::actions;
use crate::plugin::HelpTopic;
use crate::types::{
    AboutBody, AppInfo, HelpBody, HelpBook, HelpLine, HelpLink, HelpTopicView, KeyBinding,
    ShortcutGroup, ShortcutItem, ShortcutSection, ShortcutsBody,
};

/// Everything a topic needs to render itself.
pub struct HelpCtx<'a> {
    pub app: &'a AppInfo,
    /// The keymap actually in force, not the defaults — so when remapping ships,
    /// help follows it without changing.
    pub keymap: &'a [KeyBinding],
    /// The user's raw search text; empty means "show everything".
    pub query: &'a str,
}

impl HelpCtx<'_> {
    /// The query split into lower-cased terms. Every term must match for a row to
    /// survive, which is what makes typing more words narrow the list.
    fn terms(&self) -> Vec<String> {
        self.query
            .split_whitespace()
            .map(|t| t.to_lowercase())
            .collect()
    }
}

/// Keyboard contexts in the order they are presented, with their headings. A
/// context absent from here would simply not be shown, so this list is also the
/// allow-list.
const SECTIONS: &[(&str, &str)] = &[
    ("panels", "Panels"),
    ("search", "Quick search"),
    ("viewer", "Viewer"),
    ("editor", "Editor"),
    ("terminal", "Terminal"),
    ("help", "Help"),
];

// ---------------------------------------------------------------------------
// Chord rendering
// ---------------------------------------------------------------------------

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
/// macOS HIG order, so the output matches the F-key hint bar the app already
/// shows (`⌘⇧T`).
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

/// Extra words one part of a chord should be findable by, so `"cmd"` finds `⌘`
/// and `"down"` finds `↓`. Without this the search field could only match what
/// is literally painted, which is unusable for anything spelled as a symbol.
fn part_aliases(part: &str) -> &'static str {
    match part {
        "Meta" => "meta cmd command",
        "Ctrl" => "ctrl control",
        "Alt" => "alt option",
        "Shift" => "shift",
        "ArrowUp" => "arrow up",
        "ArrowDown" => "arrow down",
        "ArrowLeft" => "arrow left",
        "ArrowRight" => "arrow right",
        "PageUp" => "page up pgup",
        "PageDown" => "page down pgdn",
        "Backspace" => "backspace",
        "Escape" => "escape esc",
        "Delete" => "delete del",
        "Enter" => "enter return",
        " " => "space",
        _ => "",
    }
}

// ---------------------------------------------------------------------------
// Topics
// ---------------------------------------------------------------------------

/// Name, version, and what this thing is.
struct AboutTopic;

impl HelpTopic for AboutTopic {
    fn id(&self) -> &str {
        "about"
    }

    fn title(&self) -> &str {
        "About"
    }

    fn body(&self, ctx: &HelpCtx<'_>) -> HelpBody {
        let line = |label: &str, value: &str| HelpLine {
            label: label.to_string(),
            value: value.to_string(),
        };
        // Skip links the adapter left unset, so a build without packaging
        // metadata shows a shorter About rather than rows that go nowhere.
        let link = |label: &str, url: &str| {
            (!url.is_empty()).then(|| HelpLink {
                label: label.to_string(),
                url: url.to_string(),
            })
        };
        HelpBody::About(AboutBody {
            app: ctx.app.clone(),
            lines: vec![
                line("Version", &ctx.app.version),
                line("License", &ctx.app.license),
                line("Shortcut schema", "Default"),
            ],
            links: [
                link("Website", &ctx.app.homepage),
                link("Source code", &ctx.app.repository),
                link("Support this project", &ctx.app.sponsor),
            ]
            .into_iter()
            .flatten()
            .collect(),
        })
    }
}

/// Every shortcut currently in force, grouped by keyboard context and then by
/// what the action does, filtered by the search query.
struct ShortcutsTopic;

impl HelpTopic for ShortcutsTopic {
    fn id(&self) -> &str {
        "shortcuts"
    }

    fn title(&self) -> &str {
        "Shortcuts"
    }

    fn body(&self, ctx: &HelpCtx<'_>) -> HelpBody {
        let terms = ctx.terms();
        let mut total = 0u32;
        let mut matched = 0u32;
        let mut sections = Vec::new();

        for (context, section_title) in SECTIONS {
            let mut groups = Vec::new();

            for category in actions::catalog() {
                let mut items = Vec::new();

                for info in category.actions {
                    // All chords bound to this action in this context. Usually
                    // one binding, but `op.delete` is F8 *and* Delete.
                    let chords: Vec<&str> = ctx
                        .keymap
                        .iter()
                        .filter(|b| b.context == *context && b.action == info.id)
                        .flat_map(|b| b.keys.iter().map(String::as_str))
                        .collect();
                    if chords.is_empty() {
                        continue;
                    }
                    total += 1;

                    let keys: Vec<String> = chords.iter().map(|c| display_chord(c)).collect();
                    if !matches(&terms, section_title, category.title, info, &chords, &keys) {
                        continue;
                    }
                    matched += 1;
                    items.push(ShortcutItem {
                        action: info.id.to_string(),
                        keys,
                        title: info.title.to_string(),
                        description: info.description.to_string(),
                    });
                }

                if !items.is_empty() {
                    groups.push(ShortcutGroup {
                        category: category.id.to_string(),
                        title: category.title.to_string(),
                        items,
                    });
                }
            }

            if !groups.is_empty() {
                sections.push(ShortcutSection {
                    context: context.to_string(),
                    title: section_title.to_string(),
                    groups,
                });
            }
        }

        HelpBody::Shortcuts(ShortcutsBody {
            query: ctx.query.to_string(),
            match_count: matched,
            total_count: total,
            sections,
        })
    }
}

/// Whether one shortcut row survives the filter. Every term must appear
/// somewhere in the row — its context, its category, its keys (as typed *and* as
/// painted, plus aliases), its action id, its title, or its description. That is
/// the "search in all of it" the filter field promises.
///
/// What is deliberately **not** searchable: internal category ids. They are
/// never shown, and matching them is pure noise — `"cmd"` would drag in every
/// row of the `viewer_cmd` and `editor_cmd` groups.
fn matches(
    terms: &[String],
    section_title: &str,
    category_title: &str,
    info: &actions::ActionInfo,
    chords: &[&str],
    keys: &[String],
) -> bool {
    if terms.is_empty() {
        return true;
    }
    let mut hay = String::new();
    for piece in [section_title, category_title, info.id, info.title, info.description] {
        hay.push_str(piece);
        hay.push(' ');
    }
    for chord in chords {
        hay.push_str(chord);
        hay.push(' ');
        for part in chord.split('+') {
            hay.push_str(part_aliases(part));
            hay.push(' ');
        }
    }
    for key in keys {
        hay.push_str(key);
        hay.push(' ');
    }
    let hay = hay.to_lowercase();
    terms.iter().all(|t| hay.contains(t))
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

/// Build the whole help book. Adding a topic — in-tree or, later, from a plugin
/// — means extending this registry; nothing else in the stack changes.
pub fn book(app: &AppInfo, keymap: &[KeyBinding], query: &str) -> HelpBook {
    let ctx = HelpCtx { app, keymap, query };
    let topics: Vec<&dyn HelpTopic> = vec![&AboutTopic, &ShortcutsTopic];
    HelpBook {
        topics: topics
            .into_iter()
            .map(|t| HelpTopicView {
                id: t.id().to_string(),
                title: t.title().to_string(),
                body: t.body(&ctx),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_keymap;

    fn app() -> AppInfo {
        AppInfo {
            name: "dimnav".to_string(),
            version: "0.1.0".to_string(),
            description: "Keyboard-first two-panel file manager".to_string(),
            license: "MIT".to_string(),
            homepage: "https://dimnav.com".to_string(),
            repository: "https://github.com/pichugin/dimnav".to_string(),
            sponsor: "https://github.com/sponsors/pichugin".to_string(),
        }
    }

    fn about(app: &AppInfo) -> AboutBody {
        match &book(app, &default_keymap(), "").topics[0].body {
            HelpBody::About(a) => a.clone(),
            other => panic!("expected About, got {other:?}"),
        }
    }

    fn shortcuts(query: &str) -> ShortcutsBody {
        let book = book(&app(), &default_keymap(), query);
        match &book.topics[1].body {
            HelpBody::Shortcuts(s) => s.clone(),
            other => panic!("expected the shortcuts topic, got {other:?}"),
        }
    }

    /// Flatten to (action, keys) pairs for assertions.
    fn rows(body: &ShortcutsBody) -> Vec<(String, Vec<String>)> {
        body.sections
            .iter()
            .flat_map(|s| s.groups.iter())
            .flat_map(|g| g.items.iter())
            .map(|i| (i.action.clone(), i.keys.clone()))
            .collect()
    }

    #[test]
    fn book_has_about_first_then_shortcuts() {
        let book = book(&app(), &default_keymap(), "");
        assert_eq!(book.topics.len(), 2);
        assert_eq!(book.topics[0].id, "about");
        assert_eq!(book.topics[0].title, "About");
        assert_eq!(book.topics[1].id, "shortcuts");
        match &book.topics[0].body {
            HelpBody::About(a) => {
                assert_eq!(a.app.name, "dimnav");
                assert_eq!(a.app.version, "0.1.0");
            }
            other => panic!("expected About, got {other:?}"),
        }
    }

    #[test]
    fn about_lists_every_link_the_adapter_supplied() {
        let body = about(&app());
        assert_eq!(
            body.links
                .iter()
                .map(|l| (l.label.as_str(), l.url.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("Website", "https://dimnav.com"),
                ("Source code", "https://github.com/pichugin/dimnav"),
                ("Support this project", "https://github.com/sponsors/pichugin"),
            ]
        );
    }

    /// A build without packaging metadata must show a shorter About, never a row
    /// whose activation goes nowhere.
    #[test]
    fn about_omits_links_the_adapter_left_unset() {
        let mut info = app();
        info.homepage = String::new();
        info.sponsor = String::new();

        let body = about(&info);

        assert_eq!(
            body.links.iter().map(|l| l.label.as_str()).collect::<Vec<_>>(),
            vec!["Source code"]
        );
        assert!(body.links.iter().all(|l| !l.url.is_empty()));
    }

    #[test]
    fn about_reports_the_license() {
        let body = about(&app());
        let license = body.lines.iter().find(|l| l.label == "License");
        assert_eq!(license.map(|l| l.value.as_str()), Some("MIT"));
    }

    /// Every binding in the keymap has to reach the help screen — a shortcut the
    /// user cannot find is the whole bug this feature exists to fix.
    #[test]
    fn every_binding_is_listed() {
        let body = shortcuts("");
        let listed: std::collections::BTreeSet<(String, String)> = body
            .sections
            .iter()
            .flat_map(|s| {
                s.groups
                    .iter()
                    .flat_map(move |g| g.items.iter().map(move |i| (s.context.clone(), i.action.clone())))
            })
            .collect();
        for b in default_keymap() {
            assert!(
                listed.contains(&(b.context.clone(), b.action.clone())),
                "{}/{} is bound but missing from help",
                b.context,
                b.action,
            );
        }
        assert_eq!(body.match_count, body.total_count);
        assert_eq!(body.total_count as usize, listed.len());
    }

    #[test]
    fn sections_are_ordered_and_named() {
        let body = shortcuts("");
        let titles: Vec<&str> = body.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Panels", "Quick search", "Viewer", "Editor", "Terminal", "Help"]
        );
    }

    #[test]
    fn filters_on_the_title() {
        let body = shortcuts("copy");
        let rows = rows(&body);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "op.copy");
        assert_eq!(rows[0].1, vec!["F5"]);
        assert_eq!(body.match_count, 1);
        assert!(body.total_count > 1, "total is the unfiltered count");
    }

    /// The symbol a shortcut is painted with is untypeable, so the alias table is
    /// the only way to find `⌘T` by searching. Both spellings must land on
    /// exactly the Meta-bound rows.
    #[test]
    fn filters_on_the_key_symbol_and_its_aliases() {
        // Panels-context rows come first, in category order — quick search is
        // declared before the terminal group, so ⌘F leads.
        let expected = vec!["search.start", "terminal.focus", "terminal.toggle_half", "terminal.blur", "terminal.toggle_half"];
        for query in ["⌘", "cmd"] {
            let found: Vec<String> = rows(&shortcuts(query)).into_iter().map(|(a, _)| a).collect();
            assert_eq!(found, expected, "{query:?} did not match the ⌘ rows exactly");
        }
        // "command" is also the key's name, but it collides with the English
        // word — it appears in the "Commands" group headings and in several
        // terminal descriptions. It must still be a superset of the ⌘ rows.
        let loose: Vec<String> = rows(&shortcuts("command")).into_iter().map(|(a, _)| a).collect();
        for action in &expected {
            assert!(loose.contains(&action.to_string()), "'command' lost {action}");
        }
    }

    #[test]
    fn filters_on_the_category_title() {
        let rows = rows(&shortcuts("scrolling"));
        assert!(!rows.is_empty());
        assert!(
            rows.iter().all(|(a, _)| a.starts_with("viewer.")),
            "the Scrolling group is the viewer's: {rows:?}",
        );
    }

    #[test]
    fn filters_on_the_description_alone() {
        // "trash" appears in no title and no key — only in op.delete's description.
        let rows = rows(&shortcuts("trash"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "op.delete");
    }

    /// An action bound in two contexts is listed under both, so a query that
    /// matches it returns one row per context rather than collapsing them.
    #[test]
    fn an_action_bound_in_two_contexts_appears_in_both() {
        let body = shortcuts("terminal.toggle_half");
        let contexts: Vec<&str> = body
            .sections
            .iter()
            .filter(|s| s.groups.iter().any(|g| !g.items.is_empty()))
            .map(|s| s.context.as_str())
            .collect();
        assert_eq!(contexts, vec!["panels", "terminal"]);
    }

    /// Multiple terms narrow rather than widen.
    #[test]
    fn every_term_must_match() {
        assert!(!rows(&shortcuts("page")).is_empty());
        assert!(rows(&shortcuts("page viewer")).iter().all(|(a, _)| a.starts_with("viewer.")));
        assert!(rows(&shortcuts("page zzzz")).is_empty());
    }

    /// A query that matches nothing must collapse the sections away entirely,
    /// not leave a page of empty headings.
    #[test]
    fn a_miss_yields_no_sections() {
        let body = shortcuts("nothingmatchesthis");
        assert!(body.sections.is_empty());
        assert_eq!(body.match_count, 0);
        assert!(body.total_count > 0);
        assert_eq!(body.query, "nothingmatchesthis");
    }

    #[test]
    fn multi_chord_actions_show_every_chord() {
        let body = shortcuts("op.delete");
        let rows = rows(&body);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, vec!["F8", "Del"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn chords_render_the_way_the_fkey_bar_does() {
        assert_eq!(display_chord("Meta+Shift+t"), "⌘⇧T");
        assert_eq!(display_chord("Meta+t"), "⌘T");
        assert_eq!(display_chord("Ctrl+h"), "⌃H");
        assert_eq!(display_chord("Shift+F6"), "⇧F6");
        assert_eq!(display_chord("Ctrl+Enter"), "⌃Enter");
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
