//! Themes: colour values, resolved core-side (SPEC §4/§7).
//!
//! CLAUDE.md's rule is that theme values are config-driven and never hardcoded.
//! Before this module the app had a `theme` id in [`Config`] that nothing read
//! and a stylesheet full of literal hex, which is the opposite arrangement.
//!
//! Two types, deliberately kept apart:
//!
//! - [`ThemeDoc`] is the **file format** — sparse, everything optional, with a
//!   `base` so a user theme can be four lines over a bundled one. It never
//!   crosses IPC.
//! - [`Palette`] is the **resolved result** — every value present, the light/dark
//!   choice already made. That is what the renderer gets, so it has no fallback
//!   decision of its own to make and stays swappable (SPEC §3).
//!
//! The bundled themes are `include_str!`ed TOML parsed through the same
//! [`ThemeDoc`] deserializer a user's file goes through, rather than Rust
//! constants. That costs one runtime parse and buys the guarantee that the merge
//! path is exercised by the default configuration — a Rust-const table would fork
//! the two, leaving the merge tested only by files that CI never sees.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::plugin::ThemeProvider;
use crate::types::{
    Appearance, AppearanceMode, Config, EntryCategory, Palette, ThemeSource, ThemeSummary,
    ThemeVar,
};

/// The id used when the configured one names nothing.
pub const DEFAULT_THEME: &str = "classic";

/// Every CSS custom property a complete palette defines.
///
/// The single source of truth for "what is a token". A bundled theme missing one
/// fails [`tests::every_bundled_theme_defines_every_token`] rather than rendering
/// a transparent row somewhere far from the cause.
pub const TOKENS: &[&str] = &[
    "accent",
    "accent-dim",
    "accent-fg",
    "bg",
    "bg-alt",
    "border",
    "fg",
    "fg-dim",
    "file-archive",
    "file-code",
    "file-data",
    "file-dir",
    "file-doc",
    "file-exec",
    "file-hidden",
    "file-image",
    "file-media",
    "file-selected",
    "file-symlink",
    "term-err",
    "term-idle",
    "term-ok",
    "term-run",
];

/// The handful of tokens a picker swatch is drawn from.
///
/// A preview has to say what a theme *feels* like in a few square millimetres,
/// so it is the page colours plus the accent plus two listing colours far enough
/// apart to show the palette's range — not a fair sample of all 23, which at
/// swatch size would read as noise.
pub const PREVIEW_TOKENS: &[&str] = &["bg", "fg", "accent", "file-dir", "file-exec"];

/// The token that colours a listing row of the given category (§4).
///
/// Pairs with [`crate::filetype::classify`], which decides the category. Keeping
/// the mapping here rather than in the renderer means a new category is a
/// compile error in this file — the match is exhaustive — instead of a row that
/// silently loses its colour.
pub fn token_for(category: EntryCategory) -> Option<&'static str> {
    Some(match category {
        EntryCategory::Dir => "file-dir",
        EntryCategory::Symlink => "file-symlink",
        EntryCategory::Hidden => "file-hidden",
        EntryCategory::Doc => "file-doc",
        EntryCategory::Data => "file-data",
        EntryCategory::Code => "file-code",
        EntryCategory::Archive => "file-archive",
        EntryCategory::Image => "file-image",
        EntryCategory::Media => "file-media",
        EntryCategory::Exec => "file-exec",
        // Nothing claimed it: the row keeps the default foreground rather than
        // being forced into a bucket.
        EntryCategory::Plain => return None,
    })
}

/// A theme as it is written down — bundled or hand-authored.
///
/// Sparse by construction: a user theme naming a `base` and three colours is
/// valid and complete. Unknown keys inside `[dark]` / `[light]` are kept rather
/// than rejected, so a theme written for a later version still loads here.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThemeDoc {
    pub name: String,
    /// Id of the theme this one starts from. Empty means "start from nothing"
    /// for a bundled theme, and the resolver supplies the default underneath a
    /// user theme that names no base.
    pub base: String,
    /// Pins this theme to one appearance regardless of the OS or the user's
    /// [`AppearanceMode`]. Set it when the theme defines only one variant —
    /// following the OS into the other would render half a palette.
    pub appearance: Option<Appearance>,
    pub dark: BTreeMap<String, String>,
    pub light: BTreeMap<String, String>,
}

impl ThemeDoc {
    /// The variant map for one appearance.
    fn variant(&self, appearance: Appearance) -> &BTreeMap<String, String> {
        match appearance {
            Appearance::Dark => &self.dark,
            Appearance::Light => &self.light,
        }
    }

    /// Which appearances this document actually defines colours for.
    fn defines(&self, appearance: Appearance) -> bool {
        !self.variant(appearance).is_empty()
    }

    /// Lay `self` over `base`, key by key. `self` wins wherever both speak.
    fn over(self, base: ThemeDoc) -> ThemeDoc {
        let merge = |mut under: BTreeMap<String, String>, over: BTreeMap<String, String>| {
            under.extend(over);
            under
        };
        ThemeDoc {
            name: if self.name.is_empty() { base.name } else { self.name },
            base: String::new(),
            appearance: self.appearance.or(base.appearance),
            dark: merge(base.dark, self.dark),
            light: merge(base.light, self.light),
        }
    }
}

macro_rules! bundled {
    ($ty:ident, $id:literal, $file:literal) => {
        /// A bundled theme, written against the [`ThemeProvider`] extension point
        /// rather than special-cased (SPEC §6a).
        pub struct $ty;

        impl ThemeProvider for $ty {
            fn id(&self) -> &str {
                $id
            }

            fn title(&self) -> &str {
                &self.document().name
            }

            fn document(&self) -> &ThemeDoc {
                static DOC: OnceLock<ThemeDoc> = OnceLock::new();
                DOC.get_or_init(|| {
                    toml::from_str(include_str!($file)).unwrap_or_else(|e| {
                        // A bundled theme is compiled in, so this is a build-time
                        // mistake that only shows up at runtime. Fail loudly here
                        // rather than silently shipping a themeless app.
                        panic!("bundled theme {} is malformed: {e}", $id)
                    })
                })
            }
        }
    };
}

bundled!(ClassicTheme, "classic", "themes/classic.toml");
bundled!(DarkMinimalTheme, "dark-minimal", "themes/dark-minimal.toml");
bundled!(LightMinimalTheme, "light-minimal", "themes/light-minimal.toml");

/// Every theme the app ships with.
///
/// The seam a plugin-contributed theme pushes into once Phase 5 lands — the same
/// shape [`crate::help::topics`] uses for help topics.
pub fn registry() -> Vec<&'static dyn ThemeProvider> {
    vec![&ClassicTheme, &DarkMinimalTheme, &LightMinimalTheme]
}

/// Look up a bundled theme by id.
fn bundled(id: &str) -> Option<&'static dyn ThemeProvider> {
    registry().into_iter().find(|p| p.id() == id)
}

/// Where a user's own theme files live: `themes/` beside `config.toml` (§7).
pub fn theme_dir() -> Option<PathBuf> {
    crate::config::config_path().and_then(|p| p.parent().map(|d| d.join("themes")))
}

/// Read a user theme by id. `None` for any reason at all — absent, unreadable, or
/// malformed — because the caller's answer is the same in every case: fall back.
fn user_doc(id: &str) -> Option<ThemeDoc> {
    let path = theme_dir()?.join(format!("{id}.toml"));
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

/// The document for `id`, with its `base` merged in underneath.
///
/// A base chain is followed exactly one level and then cut. One level is what
/// "start from dark-minimal and change the accent" needs; an arbitrary chain
/// would need cycle detection to be safe, and buys nothing a single merge does
/// not already give.
fn document(id: &str) -> Option<ThemeDoc> {
    let doc = match bundled(id) {
        Some(p) => p.document().clone(),
        None => user_doc(id)?,
    };
    if doc.base.is_empty() {
        return Some(doc);
    }
    let base = match bundled(&doc.base) {
        Some(p) => p.document().clone(),
        // A base naming nothing is not fatal: the theme's own colours still
        // apply, over the default underneath.
        None => bundled(DEFAULT_THEME)?.document().clone(),
    };
    Some(doc.over(base))
}

/// Whether `id` names a theme that can actually be applied — bundled or a
/// parsable `themes/<id>.toml`.
///
/// [`resolve`] falls back for an unknown id rather than failing, which is right
/// for painting but wrong for *storing*: a picker must not write an id that
/// silently resolves to something else.
pub fn exists(id: &str) -> bool {
    document(id).is_some()
}

/// Resolve the configured theme into a paintable [`Palette`].
///
/// `os` is the appearance the operating system is currently in — a platform fact
/// this crate has no way to read, supplied by the adapter exactly as `AppInfo` is
/// for the help book (SPEC §3).
///
/// Never fails. An id that names nothing, a theme whose file went missing, and a
/// variant a theme does not define all fall back rather than leaving the app
/// unpainted; a config the user typed by hand must not be able to stop the app
/// from starting (§7).
pub fn resolve(config: &Config, os: Appearance) -> Palette {
    let wanted = wanted_appearance(config, os);

    let (id, doc) = match document(&config.theme) {
        Some(doc) => (config.theme.clone(), doc),
        None => (
            DEFAULT_THEME.to_string(),
            bundled(DEFAULT_THEME)
                .expect("the default theme is bundled")
                .document()
                .clone(),
        ),
    };

    let appearance = pick_appearance(&doc, wanted);

    let mut vars: Vec<ThemeVar> = doc
        .variant(appearance)
        .iter()
        .map(|(name, value)| ThemeVar {
            name: name.clone(),
            value: value.clone(),
        })
        .collect();
    vars.sort_by(|a, b| a.name.cmp(&b.name));

    Palette {
        id,
        name: doc.name,
        appearance,
        vars,
    }
}

/// Which variant of `doc` to paint, given the appearance the user asked for.
///
/// A theme that pins an appearance wins over the preference: it defines only that
/// variant, so honouring `system` would paint half a palette. Otherwise take what
/// was wanted, unless the theme does not define it.
///
/// Shared by [`resolve`] and [`available`] so a picker swatch cannot show one
/// variant while applying the theme paints the other.
fn pick_appearance(doc: &ThemeDoc, wanted: Appearance) -> Appearance {
    match doc.appearance {
        Some(pinned) => pinned,
        None if doc.defines(wanted) => wanted,
        None if doc.defines(other(wanted)) => other(wanted),
        None => wanted,
    }
}

/// The appearance the user's preference asks for, before any theme has a say.
fn wanted_appearance(config: &Config, os: Appearance) -> Appearance {
    match config.appearance {
        AppearanceMode::System => os,
        AppearanceMode::Light => Appearance::Light,
        AppearanceMode::Dark => Appearance::Dark,
    }
}

/// Every theme the picker can offer: the bundled ones, then whatever
/// `themes/*.toml` holds (§7).
///
/// Each carries a small preview swatch resolved the same way [`resolve`] would
/// resolve it, so what the picker shows is what applying it paints.
///
/// A user file that is missing, unreadable or malformed is **skipped**, not
/// reported: the picker's job is to list what can be applied, and a theme that
/// cannot be parsed cannot be applied. It is the same posture [`user_doc`] takes,
/// and it means a half-written theme file cannot stop the page from rendering.
///
/// A user file whose stem collides with a bundled id is skipped too, because
/// [`document`] resolves bundled first — listing it would offer something the
/// picker could not actually select.
pub fn available(config: &Config, os: Appearance) -> Vec<ThemeSummary> {
    let wanted = wanted_appearance(config, os);
    let mut out = Vec::new();

    for provider in registry() {
        if let Some(summary) = summarize(provider.id(), ThemeSource::Bundled, wanted) {
            out.push(summary);
        }
    }

    let mut user_ids: Vec<String> = theme_dir()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().into_owned()))
        .filter(|id| bundled(id).is_none())
        .collect();
    // `read_dir` order is filesystem order, which is arbitrary and would let the
    // picker reshuffle itself between launches.
    user_ids.sort();
    user_ids.dedup();

    for id in user_ids {
        if let Some(summary) = summarize(&id, ThemeSource::User, wanted) {
            out.push(summary);
        }
    }

    out
}

/// One picker row, or `None` if the theme will not resolve.
fn summarize(id: &str, source: ThemeSource, wanted: Appearance) -> Option<ThemeSummary> {
    let doc = document(id)?;
    let appearance = pick_appearance(&doc, wanted);
    let variant = doc.variant(appearance);
    Some(ThemeSummary {
        id: id.to_string(),
        // A theme file with no `name` is still perfectly usable, so fall back to
        // its id rather than offering a nameless row.
        name: if doc.name.is_empty() { id.to_string() } else { doc.name.clone() },
        source,
        pinned: doc.appearance,
        swatches: PREVIEW_TOKENS
            .iter()
            .filter_map(|t| {
                variant.get(*t).map(|v| ThemeVar {
                    name: (*t).to_string(),
                    value: v.clone(),
                })
            })
            .collect(),
    })
}

/// The appearance that is not this one.
fn other(appearance: Appearance) -> Appearance {
    match appearance {
        Appearance::Dark => Appearance::Light,
        Appearance::Light => Appearance::Dark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every category a listing row can carry. The exhaustive match in
    /// [`token_for`] is what forces a new variant to be handled at all; this list
    /// is what forces the token it names to actually exist.
    const CATEGORIES: &[EntryCategory] = &[
        EntryCategory::Dir,
        EntryCategory::Symlink,
        EntryCategory::Hidden,
        EntryCategory::Doc,
        EntryCategory::Data,
        EntryCategory::Code,
        EntryCategory::Archive,
        EntryCategory::Image,
        EntryCategory::Media,
        EntryCategory::Exec,
        EntryCategory::Plain,
    ];

    fn config(theme: &str, appearance: AppearanceMode) -> Config {
        Config {
            theme: theme.to_string(),
            appearance,
            ..Config::default()
        }
    }

    fn value<'a>(palette: &'a Palette, name: &str) -> Option<&'a str> {
        palette
            .vars
            .iter()
            .find(|v| v.name == name)
            .map(|v| v.value.as_str())
    }

    /// The bundled themes are parsed at runtime rather than compiled as Rust
    /// tables, which is what lets them share the user-theme code path — so a typo
    /// in one is a runtime panic. This is the test that turns it back into a
    /// build failure.
    #[test]
    fn every_bundled_theme_parses() {
        for provider in registry() {
            let doc = provider.document();
            assert!(!doc.name.is_empty(), "{} has no name", provider.id());
            assert!(
                doc.defines(Appearance::Dark) || doc.defines(Appearance::Light),
                "{} defines no colours at all",
                provider.id(),
            );
        }
    }

    /// A missing token renders as an unset custom property — an invisible row, or
    /// a transparent background, far from the theme file that caused it.
    #[test]
    fn every_bundled_theme_defines_every_token() {
        for provider in registry() {
            let doc = provider.document();
            for appearance in [Appearance::Dark, Appearance::Light] {
                if !doc.defines(appearance) {
                    continue; // A pinned theme need only define its own variant.
                }
                let variant = doc.variant(appearance);
                for token in TOKENS {
                    assert!(
                        variant.contains_key(*token),
                        "{} is missing {token:?} in its {appearance:?} variant",
                        provider.id(),
                    );
                }
            }
        }
    }

    /// A theme that parses but leaves a category uncoloured would paint that row
    /// in the default foreground, silently losing the §4 colour coding for it.
    #[test]
    fn the_palette_carries_a_colour_for_every_entry_category() {
        for provider in registry() {
            let palette = resolve(&config(provider.id(), AppearanceMode::System), Appearance::Dark);
            for category in CATEGORIES {
                let Some(token) = token_for(*category) else {
                    continue; // `Plain` deliberately has no colour of its own.
                };
                assert!(
                    value(&palette, token).is_some(),
                    "{} has no {token:?} for {category:?}",
                    provider.id(),
                );
            }
        }
    }

    /// Every token the mapping names must be one the themes are checked for,
    /// otherwise the two lists could drift and both tests above would still pass.
    #[test]
    fn every_category_token_is_a_known_token() {
        for category in CATEGORIES {
            if let Some(token) = token_for(*category) {
                assert!(TOKENS.contains(&token), "{token:?} is not in TOKENS");
            }
        }
    }

    /// A hand-typed theme id must not be able to leave the app unpainted (§7).
    #[test]
    fn an_unknown_theme_id_falls_back_to_the_default() {
        let palette = resolve(&config("no-such-theme", AppearanceMode::System), Appearance::Dark);

        assert_eq!(palette.id, DEFAULT_THEME);
        assert!(!palette.vars.is_empty());
        assert_eq!(value(&palette, "bg"), Some("#1e1e22"));
    }

    /// `system` is the default, and it must reproduce what the stylesheet's
    /// `prefers-color-scheme` block used to do on its own.
    #[test]
    fn system_follows_the_os_on_a_two_variant_theme() {
        let dark = resolve(&config("classic", AppearanceMode::System), Appearance::Dark);
        let light = resolve(&config("classic", AppearanceMode::System), Appearance::Light);

        assert_eq!(dark.appearance, Appearance::Dark);
        assert_eq!(value(&dark, "bg"), Some("#1e1e22"));
        assert_eq!(light.appearance, Appearance::Light);
        assert_eq!(value(&light, "bg"), Some("#fbfbfa"));
    }

    /// Pinning overrides the OS — that is the whole point of the preference.
    #[test]
    fn an_explicit_mode_overrides_the_os() {
        let palette = resolve(&config("classic", AppearanceMode::Light), Appearance::Dark);

        assert_eq!(palette.appearance, Appearance::Light);
        assert_eq!(value(&palette, "bg"), Some("#fbfbfa"));
    }

    /// A theme that defines one variant must not be dragged into the other by the
    /// OS or by the preference: half its colours would simply be absent.
    #[test]
    fn a_single_variant_theme_ignores_both_the_os_and_the_mode() {
        for mode in [AppearanceMode::System, AppearanceMode::Light, AppearanceMode::Dark] {
            for os in [Appearance::Light, Appearance::Dark] {
                let palette = resolve(&config("dark-minimal", mode), os);
                assert_eq!(
                    palette.appearance,
                    Appearance::Dark,
                    "dark-minimal drifted with mode {mode:?} on a {os:?} system",
                );
                assert_eq!(value(&palette, "bg"), Some("#17171a"));
            }
        }
    }

    /// The merge a user theme relies on: name and colours fall through from the
    /// base, and the child wins wherever both speak.
    #[test]
    fn a_theme_merges_over_its_base() {
        let base = ClassicTheme.document().clone();
        let mut child = ThemeDoc {
            name: "Mine".to_string(),
            ..ThemeDoc::default()
        };
        child.dark.insert("accent".to_string(), "#c07ad8".to_string());

        let merged = child.over(base);

        assert_eq!(merged.name, "Mine");
        // Overridden.
        assert_eq!(merged.dark.get("accent").map(String::as_str), Some("#c07ad8"));
        // Inherited, including the whole other variant.
        assert_eq!(merged.dark.get("bg").map(String::as_str), Some("#1e1e22"));
        assert_eq!(merged.light.get("bg").map(String::as_str), Some("#fbfbfa"));
    }

    /// A base that names nothing must not cost the child its own colours.
    #[test]
    fn a_nameless_child_keeps_the_base_name() {
        let merged = ThemeDoc::default().over(ClassicTheme.document().clone());
        assert_eq!(merged.name, "Classic Commander");
    }

    /// Two providers answering to one id would make which theme loads depend on
    /// registry order.
    #[test]
    fn every_provider_id_is_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for provider in registry() {
            assert!(
                seen.insert(provider.id().to_string()),
                "{} is registered twice",
                provider.id(),
            );
        }
    }

    /// The payload is asserted on and diffed; an unstable order would make both
    /// noisy for no reason.
    #[test]
    fn vars_come_back_sorted() {
        let palette = resolve(&config("classic", AppearanceMode::System), Appearance::Dark);
        let mut sorted = palette.vars.clone();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(palette.vars, sorted);
    }
}
