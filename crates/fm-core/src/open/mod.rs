//! Open / View / Edit routing (SPEC §5.5).
//!
//! Decision logic: given the entry under the cursor, the requested
//! [`OpenAction`], the panel's directory, and the [`Config`] file-type map,
//! decide **what** the adapter should do — open the embedded viewer/editor,
//! launch an external app, or (for Enter on an executable) run it. This module
//! performs no process spawning and no side-effects; the `src-tauri` adapter acts
//! on the returned [`OpenPlan`] (the same "core decides, adapter acts" split used
//! for privilege elevation).
//!
//! ## The resolution chain
//!
//! Modelled on FAR's per-type View/Edit commands, with DOS Navigator's automatic
//! type detection underneath:
//!
//! 1. A [`FileAssociation`](crate::types::FileAssociation) for the extension wins
//!    — `"internal"`, `"system"`, or an application name. View and Edit fall back
//!    to the association's `open` value before giving up.
//! 2. With no association, F3/F4 use the **embedded** viewer/editor, choosing the
//!    representation from the file's sniffed [`MediaKind`]: text as text, binary
//!    as hex, images as pictures. F4 on something it cannot edit (a binary, an
//!    image, a file too big to hold in memory) degrades gracefully rather than
//!    refusing.
//! 3. Enter is unchanged: executables run, everything else goes to the system
//!    default, so double-click behaviour still matches the OS.
//!
//! File associations are the [`plugin::FileTypeHandler`](crate::plugin) extension
//! point in its simplest, config-driven form; the embedded text/hex/image
//! handlers are the first consumers of that extension point.

use std::path::Path;

use crate::types::{Config, Entry, EntryKind, FileProbe, MediaKind, OpenAction};

/// What the adapter should do to fulfil an open request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenPlan {
    /// Launch an external application on `path`. `app == None` means the system
    /// default ("Open"); `Some(app)` names a configured viewer/editor.
    Launch { path: String, app: Option<String> },
    /// The entry is executable and the user pressed Enter → run it, with `cwd` as
    /// the working directory. Phase 1 routes this to a simple captured-output
    /// modal; Phase 2 routes it to the embedded terminal (§5.7).
    Execute { path: String, cwd: String },
    /// Open the file inside the app, in the embedded viewer or editor.
    Embedded { path: String, mode: EmbeddedMode },
}

/// Which embedded surface a file opens in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedMode {
    /// The viewer, showing decoded text.
    Text,
    /// The viewer, showing a hex dump — binaries, and F4 on a binary.
    Hex,
    /// The viewer, showing the picture itself.
    Image,
    /// The editor.
    Edit,
}

/// What an association value asks for. Parsed from the config string so the
/// TOML stays human-readable (`view = "internal"`) without a bespoke schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handler {
    /// The embedded viewer/editor.
    Internal,
    /// The OS default application for the type.
    System,
    /// A specific application by name.
    App(String),
}

impl Handler {
    fn parse(value: &str) -> Handler {
        match value.trim().to_lowercase().as_str() {
            "internal" | "builtin" | "embedded" => Handler::Internal,
            "system" | "default" => Handler::System,
            _ => Handler::App(value.to_string()),
        }
    }
}

/// Decide how to fulfil `action` for `entry` living in `cwd`.
///
/// `probe` is the file's sniffed type, which the caller supplies for View/Edit
/// (see [`crate::view::probe`]); `None` means "unknown", and routing then falls
/// back to the system default rather than guessing at a representation.
///
/// Returns `None` when there is nothing to open — the parent entry `..` or a
/// directory (navigation, not opening, handles those).
pub fn route(
    entry: &Entry,
    action: OpenAction,
    cwd: &str,
    config: &Config,
    probe: Option<&FileProbe>,
) -> Option<OpenPlan> {
    if entry.name == ".." || entry.kind == EntryKind::Dir {
        return None;
    }

    let path = Path::new(cwd).join(&entry.name).to_string_lossy().into_owned();

    // Enter on an executable runs it, whatever the associations say.
    if action == OpenAction::Open && entry.is_executable {
        return Some(OpenPlan::Execute {
            path,
            cwd: cwd.to_string(),
        });
    }

    let handler = resolve_handler(&entry.name, action, config);
    let plan = match handler {
        Some(Handler::App(app)) => OpenPlan::Launch {
            path,
            app: Some(app),
        },
        Some(Handler::System) => OpenPlan::Launch { path, app: None },
        Some(Handler::Internal) => embedded(path, action, config, probe),
        // No association: F3/F4 default to the embedded surfaces, Enter keeps
        // handing off to the OS so double-click behaviour is unsurprising.
        None => match action {
            OpenAction::Open => OpenPlan::Launch { path, app: None },
            _ => embedded(path, action, config, probe),
        },
    };
    Some(plan)
}

/// [`route`] for callers that just want the answer: sniffs the file when the
/// action needs a type (F3/F4) and returns the plan together with the probe, so
/// the viewer/editor can be handed the type without sniffing the file twice.
///
/// A file that cannot be sniffed is not an error here — routing simply falls
/// back to the system default, and the real failure surfaces when the OS tries
/// to open it (§5.6).
pub fn plan_open(
    entry: &Entry,
    action: OpenAction,
    cwd: &str,
    config: &Config,
) -> (Option<OpenPlan>, Option<FileProbe>) {
    // Enter on an executable runs it without caring what is inside; every other
    // case may end up embedded, so sniff. The probe reads only a few KiB.
    let probe = if action == OpenAction::Open && entry.is_executable {
        None
    } else {
        crate::view::probe::probe(&Path::new(cwd).join(&entry.name)).ok()
    };
    (route(entry, action, cwd, config, probe.as_ref()), probe)
}

/// Which embedded surface `action` should use for a file of the probed type,
/// degrading to an external launch when the embedded one cannot serve it.
fn embedded(
    path: String,
    action: OpenAction,
    config: &Config,
    probe: Option<&FileProbe>,
) -> OpenPlan {
    // Nothing known about the file (it could not be read): let the OS try.
    let Some(probe) = probe else {
        return OpenPlan::Launch { path, app: None };
    };
    let mode = match (action, probe.media) {
        (OpenAction::Edit, MediaKind::Text) if probe.size <= config.edit_max_bytes => {
            EmbeddedMode::Edit
        }
        // A binary is editable only as a read-only hex dump, and an image or an
        // oversized file is better served by a real external tool.
        (OpenAction::Edit, MediaKind::Binary) => EmbeddedMode::Hex,
        (OpenAction::Edit, _) => return OpenPlan::Launch { path, app: None },
        (_, MediaKind::Text) => EmbeddedMode::Text,
        (_, MediaKind::Binary) => EmbeddedMode::Hex,
        (_, MediaKind::Image) => EmbeddedMode::Image,
    };
    OpenPlan::Embedded { path, mode }
}

/// The handler configured for `name`'s extension and `action`, or `None` when no
/// association claims it. View/Edit fall back to the association's `open` value
/// before giving up, so a one-line association covers all three actions.
fn resolve_handler(name: &str, action: OpenAction, config: &Config) -> Option<Handler> {
    let ext = extension(name)?;
    let assoc = config
        .associations
        .iter()
        .find(|a| a.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)))?;
    let value = match action {
        OpenAction::Open => assoc.open.clone(),
        OpenAction::View => assoc.view.clone().or_else(|| assoc.open.clone()),
        OpenAction::Edit => assoc.edit.clone().or_else(|| assoc.open.clone()),
    };
    value.map(|v| Handler::parse(&v))
}

/// Lower-cased extension (no dot), or `None`. A leading-dot name with no other
/// dot (e.g. `.bashrc`) has no extension — matches the frontend's colour rule.
fn extension(name: &str) -> Option<String> {
    let dot = name.rfind('.')?;
    if dot == 0 {
        return None;
    }
    Some(name[dot + 1..].to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Eol, EntryMarker, FileAssociation, TextEncoding};

    fn entry(name: &str, kind: EntryKind, executable: bool) -> Entry {
        Entry {
            name: name.to_string(),
            kind,
            size: 0,
            modified: 0,
            created: 0,
            permissions: 0,
            uid: 0,
            gid: 0,
            owner: None,
            group: None,
            nlink: 0,
            symlink_target: None,
            is_executable: executable,
            marker: EntryMarker::Ok,
            computed_size: None,
        }
    }

    fn config_with(assoc: FileAssociation) -> Config {
        Config {
            associations: vec![assoc],
            ..Config::default()
        }
    }

    fn probed(media: MediaKind, size: u64) -> FileProbe {
        FileProbe {
            size,
            media,
            encoding: TextEncoding::Utf8,
            eol: Eol::Lf,
        }
    }

    /// Route with no type knowledge — what Enter does, and what View/Edit fall
    /// back to when a file cannot be probed.
    fn plan(entry: &Entry, action: OpenAction, cfg: &Config) -> Option<OpenPlan> {
        route(entry, action, "/home", cfg, None)
    }

    /// Route a file of a known type, as F3/F4 do.
    fn plan_probed(
        entry: &Entry,
        action: OpenAction,
        cfg: &Config,
        media: MediaKind,
        size: u64,
    ) -> Option<OpenPlan> {
        route(entry, action, "/home", cfg, Some(&probed(media, size)))
    }

    fn embedded_mode(plan: Option<OpenPlan>) -> EmbeddedMode {
        match plan {
            Some(OpenPlan::Embedded { mode, .. }) => mode,
            other => panic!("expected an embedded plan, got {other:?}"),
        }
    }

    #[test]
    fn dotdot_and_dirs_are_not_openable() {
        let cfg = Config::default();
        assert_eq!(plan(&entry("..", EntryKind::Dir, false), OpenAction::Open, &cfg), None);
        assert_eq!(plan(&entry("sub", EntryKind::Dir, false), OpenAction::Open, &cfg), None);
    }

    #[test]
    fn enter_still_hands_plain_files_to_the_system_default() {
        let cfg = Config::default();
        assert_eq!(
            plan(&entry("notes.txt", EntryKind::File, false), OpenAction::Open, &cfg),
            Some(OpenPlan::Launch { path: "/home/notes.txt".to_string(), app: None })
        );
    }

    #[test]
    fn executable_enter_runs_it_but_f3_views_it() {
        let cfg = Config::default();
        let exe = entry("run.sh", EntryKind::File, true);
        assert_eq!(
            plan(&exe, OpenAction::Open, &cfg),
            Some(OpenPlan::Execute { path: "/home/run.sh".to_string(), cwd: "/home".to_string() })
        );
        // F3 on a script shows the script, rather than running it.
        assert_eq!(
            embedded_mode(plan_probed(&exe, OpenAction::View, &cfg, MediaKind::Text, 40)),
            EmbeddedMode::Text
        );
    }

    #[test]
    fn f3_picks_its_representation_from_the_files_type() {
        let cfg = Config::default();
        let f = entry("thing", EntryKind::File, false);
        let view = |media| embedded_mode(plan_probed(&f, OpenAction::View, &cfg, media, 100));
        assert_eq!(view(MediaKind::Text), EmbeddedMode::Text);
        assert_eq!(view(MediaKind::Binary), EmbeddedMode::Hex);
        assert_eq!(view(MediaKind::Image), EmbeddedMode::Image);
    }

    #[test]
    fn f4_edits_text_and_degrades_for_everything_else() {
        let cfg = Config::default();
        let f = entry("thing", EntryKind::File, false);
        assert_eq!(
            embedded_mode(plan_probed(&f, OpenAction::Edit, &cfg, MediaKind::Text, 100)),
            EmbeddedMode::Edit
        );
        // A binary is editable only as a read-only dump.
        assert_eq!(
            embedded_mode(plan_probed(&f, OpenAction::Edit, &cfg, MediaKind::Binary, 100)),
            EmbeddedMode::Hex
        );
        // An image, and a file too big to hold in memory, go to a real tool.
        assert!(matches!(
            plan_probed(&f, OpenAction::Edit, &cfg, MediaKind::Image, 100),
            Some(OpenPlan::Launch { app: None, .. })
        ));
        assert!(matches!(
            plan_probed(&f, OpenAction::Edit, &cfg, MediaKind::Text, cfg.edit_max_bytes + 1),
            Some(OpenPlan::Launch { app: None, .. })
        ));
    }

    #[test]
    fn an_unprobeable_file_falls_back_to_the_system_rather_than_guessing() {
        let cfg = Config::default();
        let f = entry("mystery.dat", EntryKind::File, false);
        assert!(matches!(
            plan(&f, OpenAction::View, &cfg),
            Some(OpenPlan::Launch { app: None, .. })
        ));
    }

    #[test]
    fn association_maps_handlers_per_action_with_fallback() {
        let cfg = config_with(FileAssociation {
            extensions: vec!["md".into(), "markdown".into()],
            open: Some("Typora".into()),
            view: None, // View falls back to `open`
            edit: Some("Code".into()),
        });
        let md = entry("README.md", EntryKind::File, false);
        let app = |a| match plan_probed(&md, a, &cfg, MediaKind::Text, 100) {
            Some(OpenPlan::Launch { app, .. }) => app,
            other => panic!("expected Launch, got {other:?}"),
        };
        assert_eq!(app(OpenAction::Open), Some("Typora".into()));
        assert_eq!(app(OpenAction::View), Some("Typora".into())); // fell back to open
        assert_eq!(app(OpenAction::Edit), Some("Code".into()));
        // Case-insensitive extension match.
        let md_upper = entry("READ.MARKDOWN", EntryKind::File, false);
        assert!(matches!(
            plan_probed(&md_upper, OpenAction::Edit, &cfg, MediaKind::Text, 100),
            Some(OpenPlan::Launch { app: Some(ref a), .. }) if a == "Code"
        ));
    }

    #[test]
    fn the_system_and_internal_keywords_override_the_defaults() {
        let cfg = config_with(FileAssociation {
            extensions: vec!["log".into()],
            open: Some("internal".into()), // Enter opens our viewer for logs
            view: Some("system".into()),   // but F3 hands off to Console.app
            edit: None,
        });
        let log = entry("system.log", EntryKind::File, false);
        assert_eq!(
            embedded_mode(plan_probed(&log, OpenAction::Open, &cfg, MediaKind::Text, 100)),
            EmbeddedMode::Text
        );
        assert!(matches!(
            plan_probed(&log, OpenAction::View, &cfg, MediaKind::Text, 100),
            Some(OpenPlan::Launch { app: None, .. })
        ));
        // Edit has no value of its own and inherits `open` — the internal editor.
        assert_eq!(
            embedded_mode(plan_probed(&log, OpenAction::Edit, &cfg, MediaKind::Text, 100)),
            EmbeddedMode::Edit
        );
    }

    #[test]
    fn handler_keywords_are_recognised_case_insensitively() {
        assert_eq!(Handler::parse("Internal"), Handler::Internal);
        assert_eq!(Handler::parse(" system "), Handler::System);
        assert_eq!(Handler::parse("Visual Studio Code"), Handler::App("Visual Studio Code".into()));
    }

    #[test]
    fn dotfile_without_extension_has_no_association() {
        assert_eq!(extension(".bashrc"), None);
        assert_eq!(extension("archive.tar.gz"), Some("gz".to_string()));
        assert_eq!(extension("NOEXT"), None);
    }
}
