//! The F2 settings book (SPEC §7).
//!
//! Everything the app persists is edited from here. Before this module the only
//! way to change a setting was to hand-edit `config.toml`, which is the
//! configuration story §7 asks for but not a discoverable one — nothing in the
//! app said which settings existed.
//!
//! The whole book is built core-side and handed over as data, exactly as
//! [`crate::help`] builds the F1 book: the renderer picks a page, paints each
//! field by its [`FieldControl`], and decides nothing about which settings exist,
//! what they are called, or what they may be set to (CLAUDE.md — the frontend is
//! a thin, swappable layer).
//!
//! Pages are written against [`crate::plugin::SettingsPage`] rather than being
//! special-cased, so a plugin-contributed page in Phase 5 is a push into
//! [`book`]'s registry and nothing more (SPEC §6a).
//!
//! ## One address per setting
//!
//! A field's `id` is its dotted path into [`Config`] — `"appearance"`,
//! `"watch.debounce_ms"`. The same string addresses it in [`read`], [`apply`] and
//! [`reset`], so there is exactly one place that knows how a setting is spelled
//! and no separate identifier scheme for the frontend to keep in step.
//!
//! [`reset`] is [`apply`] with the default looked up, rather than a second
//! assignment per field: one write path means a validation rule cannot apply to
//! setting a value and not to resetting it.

use crate::plugin::SettingsPage;
use crate::theme;
use crate::types::{
    Appearance, AppearanceMode, ChoiceOption, Config, FieldControl, FieldValue, SettingField,
    SettingsBody, SettingsBook, SettingsPageView, ThemeBody,
};

/// Everything a page needs to render itself.
pub struct SettingsCtx<'a> {
    /// The config currently in force, not the file on disk.
    pub config: &'a Config,
    /// Which appearance the OS is in — the one fact this crate structurally
    /// cannot read, supplied by the adapter exactly as `AppInfo` is for the help
    /// book (SPEC §3).
    pub os: Appearance,
}

// ---------------------------------------------------------------------------
// Field addressing: read / apply / reset
// ---------------------------------------------------------------------------

/// The current value of the field at `id`, or `None` if nothing lives there.
///
/// The one place that maps a dotted path onto a [`Config`] field for reading.
pub fn read(config: &Config, id: &str) -> Option<FieldValue> {
    Some(match id {
        "theme" => FieldValue::Str(config.theme.clone()),
        "appearance" => FieldValue::Str(appearance_mode_id(config.appearance).to_string()),
        _ => return None,
    })
}

/// What [`Config::default`] holds for the field at `id`.
///
/// Derived from the real default config rather than from a second table of
/// literals, so a changed default cannot leave the "reset" affordance restoring
/// a value the app no longer ships with.
pub fn default_of(id: &str) -> Option<FieldValue> {
    read(&Config::default(), id)
}

/// Write `value` to the field at `id`, validating it first.
///
/// The single write path for every setting. Persisting is the caller's job — the
/// core owns the decision, the adapter owns the file.
pub fn apply(config: &mut Config, id: &str, value: &FieldValue) -> Result<(), String> {
    match id {
        "theme" => {
            let wanted = as_str(id, value)?;
            // `resolve` falls back for an unknown id rather than failing, which
            // is right for painting and wrong for storing: writing an id that
            // silently resolves to something else would leave the picker showing
            // one theme and the app painting another.
            if !theme::exists(wanted) {
                return Err(format!("no theme named {wanted:?}"));
            }
            config.theme = wanted.to_string();
        }
        "appearance" => {
            let wanted = as_str(id, value)?;
            config.appearance = parse_appearance_mode(wanted)
                .ok_or_else(|| format!("{wanted:?} is not one of system, light, dark"))?;
        }
        _ => return Err(format!("no setting named {id:?}")),
    }
    Ok(())
}

/// Restore the field at `id` to its default.
pub fn reset(config: &mut Config, id: &str) -> Result<(), String> {
    let value = default_of(id).ok_or_else(|| format!("no setting named {id:?}"))?;
    apply(config, id, &value)
}

/// Unwrap a [`FieldValue::Str`], naming the field when it is the wrong shape —
/// the frontend sends what the control told it to, so a mismatch is a bug worth
/// reading rather than a user error.
fn as_str<'a>(id: &str, value: &'a FieldValue) -> Result<&'a str, String> {
    match value {
        FieldValue::Str(s) => Ok(s),
        other => Err(format!("{id:?} takes a string, got {other:?}")),
    }
}

/// The wire spelling of an [`AppearanceMode`], matching its serde representation
/// so the value a control sends back is the value the TOML file stores.
fn appearance_mode_id(mode: AppearanceMode) -> &'static str {
    match mode {
        AppearanceMode::System => "system",
        AppearanceMode::Light => "light",
        AppearanceMode::Dark => "dark",
    }
}

fn parse_appearance_mode(id: &str) -> Option<AppearanceMode> {
    Some(match id {
        "system" => AppearanceMode::System,
        "light" => AppearanceMode::Light,
        "dark" => AppearanceMode::Dark,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Field construction
// ---------------------------------------------------------------------------

/// Build one renderable field.
///
/// `None` when `id` addresses nothing, so a mistyped id drops the row instead of
/// painting a control wired to a setting that does not exist — the same posture
/// the About topic takes with a link the adapter left unset.
/// [`tests::every_field_in_the_book_is_addressable`] is what keeps that from
/// happening quietly.
fn field(
    config: &Config,
    id: &str,
    label: &str,
    description: &str,
    control: FieldControl,
) -> Option<SettingField> {
    let value = read(config, id)?;
    let default = default_of(id)?;
    Some(SettingField {
        id: id.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        control,
        is_default: value == default,
        value,
        default,
    })
}

/// Shorthand for one option of a [`FieldControl::Choice`] whose value is a
/// string — which is every enum-valued setting, since they travel as their serde
/// spelling.
fn option(value: &str, label: &str, description: &str) -> ChoiceOption {
    ChoiceOption {
        value: FieldValue::Str(value.to_string()),
        label: label.to_string(),
        description: description.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

/// Theme choice and the light/dark preference (§4).
struct AppearancePage;

impl SettingsPage for AppearancePage {
    fn id(&self) -> &str {
        "appearance"
    }

    fn title(&self) -> &str {
        "Appearance"
    }

    fn body(&self, ctx: &SettingsCtx<'_>) -> SettingsBody {
        SettingsBody::Theme(ThemeBody {
            themes: theme::available(ctx.config, ctx.os),
            // The id actually in force, not the configured one: a config
            // pointing at a deleted theme should mark what is really painted.
            current: theme::resolve(ctx.config, ctx.os).id,
            fields: [field(
                ctx.config,
                "appearance",
                "Light / dark",
                "Follow the system setting, or pin one. A theme that defines only a single \
                 variant pins itself regardless, since following the OS into the other would \
                 paint half a palette.",
                FieldControl::Choice {
                    options: vec![
                        option("system", "System", "Follow the operating system."),
                        option("light", "Light", ""),
                        option("dark", "Dark", ""),
                    ],
                },
            )]
            .into_iter()
            .flatten()
            .collect(),
        })
    }
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

/// Build the whole settings book. Adding a page — in-tree or, later, from a
/// plugin — means extending this registry and teaching [`read`] and [`apply`]
/// the field ids it declares; nothing else in the stack changes.
pub fn book(config: &Config, os: Appearance) -> SettingsBook {
    let ctx = SettingsCtx { config, os };
    let pages: Vec<&dyn SettingsPage> = vec![&AppearancePage];
    SettingsBook {
        pages: pages
            .into_iter()
            .map(|p| SettingsPageView {
                id: p.id().to_string(),
                title: p.title().to_string(),
                body: p.body(&ctx),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SettingsBody, ThemeSource};

    fn book_of(config: &Config) -> SettingsBook {
        book(config, Appearance::Dark)
    }

    /// Every field the book paints must round-trip through the same dotted id it
    /// is rendered with. Without this, `field()`'s `None` arm would drop a row
    /// silently and the setting would simply be missing from the UI.
    #[test]
    fn every_field_in_the_book_is_addressable() {
        let config = Config::default();
        let book = book_of(&config);
        let mut seen = 0;
        for page in &book.pages {
            let fields = match &page.body {
                SettingsBody::Fields(f) => f.groups.iter().flat_map(|g| &g.fields).collect::<Vec<_>>(),
                SettingsBody::Theme(t) => t.fields.iter().collect(),
            };
            for f in fields {
                seen += 1;
                assert!(read(&config, &f.id).is_some(), "{} is painted but not readable", f.id);
                assert!(default_of(&f.id).is_some(), "{} has no default", f.id);
                let mut probe = config.clone();
                assert!(reset(&mut probe, &f.id).is_ok(), "{} cannot be reset", f.id);
            }
        }
        assert!(seen > 0, "the book painted no fields at all");
    }

    #[test]
    fn the_appearance_page_lists_every_bundled_theme() {
        let book = book_of(&Config::default());
        let page = &book.pages[0];
        assert_eq!(page.id, "appearance");
        assert_eq!(page.title, "Appearance");
        let SettingsBody::Theme(body) = &page.body else {
            panic!("expected the theme body, got {:?}", page.body);
        };
        for provider in theme::registry() {
            let found = body.themes.iter().find(|t| t.id == provider.id());
            let found = found.unwrap_or_else(|| panic!("{} is bundled but not offered", provider.id()));
            assert_eq!(found.name, provider.title());
            assert_eq!(found.source, ThemeSource::Bundled);
        }
    }

    /// The picker marks what is *painted*, not what is configured — otherwise a
    /// config naming a deleted theme would highlight a row that is not there.
    #[test]
    fn the_current_theme_is_the_resolved_one_not_the_configured_one() {
        let config = Config { theme: "no-such-theme".to_string(), ..Config::default() };
        let book = book_of(&config);
        let SettingsBody::Theme(body) = &book.pages[0].body else { unreachable!() };
        assert_eq!(body.current, theme::DEFAULT_THEME);
        assert!(body.themes.iter().any(|t| t.id == body.current));
    }

    /// Every offered theme carries a swatch, or the picker paints empty boxes.
    #[test]
    fn every_offered_theme_previews_itself() {
        let book = book_of(&Config::default());
        let SettingsBody::Theme(body) = &book.pages[0].body else { unreachable!() };
        for t in &body.themes {
            assert_eq!(
                t.swatches.len(),
                theme::PREVIEW_TOKENS.len(),
                "{} previews {} of {} tokens",
                t.id,
                t.swatches.len(),
                theme::PREVIEW_TOKENS.len(),
            );
            assert!(t.swatches.iter().all(|v| !v.value.is_empty()));
        }
    }

    /// A theme that defines only one variant reports the pin, so the light/dark
    /// control can explain why it is being ignored instead of looking broken.
    #[test]
    fn a_pinned_theme_says_so() {
        let book = book_of(&Config::default());
        let SettingsBody::Theme(body) = &book.pages[0].body else { unreachable!() };
        let find = |id: &str| body.themes.iter().find(|t| t.id == id).expect(id);
        assert_eq!(find("dark-minimal").pinned, Some(Appearance::Dark));
        assert_eq!(find("light-minimal").pinned, Some(Appearance::Light));
        // Classic carries both variants, so it pins nothing and follows the
        // preference.
        assert_eq!(find("classic").pinned, None);
    }

    #[test]
    fn applying_a_theme_and_an_appearance_round_trips() {
        let mut config = Config::default();
        apply(&mut config, "theme", &FieldValue::Str("dark-minimal".into())).unwrap();
        assert_eq!(config.theme, "dark-minimal");
        assert_eq!(read(&config, "theme"), Some(FieldValue::Str("dark-minimal".into())));

        for mode in ["system", "light", "dark"] {
            apply(&mut config, "appearance", &FieldValue::Str(mode.into())).unwrap();
            assert_eq!(read(&config, "appearance"), Some(FieldValue::Str(mode.into())));
        }
    }

    /// The picker must not be able to store an id that `resolve` would silently
    /// substitute — that is the difference between painting and persisting.
    #[test]
    fn an_unknown_theme_is_rejected_rather_than_stored() {
        let mut config = Config::default();
        let before = config.theme.clone();
        assert!(apply(&mut config, "theme", &FieldValue::Str("no-such-theme".into())).is_err());
        assert_eq!(config.theme, before);
    }

    #[test]
    fn a_bad_appearance_and_a_bad_shape_are_both_refused() {
        let mut config = Config::default();
        assert!(apply(&mut config, "appearance", &FieldValue::Str("sepia".into())).is_err());
        assert!(apply(&mut config, "appearance", &FieldValue::Bool(true)).is_err());
        assert!(apply(&mut config, "nope.nope", &FieldValue::Bool(true)).is_err());
        assert_eq!(config.appearance, AppearanceMode::System);
    }

    #[test]
    fn reset_restores_the_shipped_default() {
        let mut config = Config::default();
        apply(&mut config, "theme", &FieldValue::Str("light-minimal".into())).unwrap();
        apply(&mut config, "appearance", &FieldValue::Str("dark".into())).unwrap();

        reset(&mut config, "theme").unwrap();
        reset(&mut config, "appearance").unwrap();

        assert_eq!(config.theme, Config::default().theme);
        assert_eq!(config.appearance, Config::default().appearance);
        assert!(reset(&mut config, "nope").is_err());
    }

    /// `is_default` is precomputed for the renderer, so it has to be right.
    #[test]
    fn is_default_tracks_the_value() {
        let mut config = Config::default();
        let field_of = |c: &Config| {
            let book = book_of(c);
            let SettingsBody::Theme(b) = &book.pages[0].body else { unreachable!() };
            b.fields.iter().find(|f| f.id == "appearance").expect("appearance").clone()
        };
        assert!(field_of(&config).is_default);
        apply(&mut config, "appearance", &FieldValue::Str("dark".into())).unwrap();
        let f = field_of(&config);
        assert!(!f.is_default);
        assert_eq!(f.value, FieldValue::Str("dark".into()));
        assert_eq!(f.default, FieldValue::Str("system".into()));
    }
}
