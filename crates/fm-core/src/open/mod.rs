//! Open / View / Edit routing (SPEC §5.5).
//!
//! Pure decision logic: given the entry under the cursor, the requested
//! [`OpenAction`], the panel's directory, and the [`Config`] file-type→app map,
//! decide **what** the adapter should do — launch an external app (system default
//! or a configured one) or, for Enter on an executable, run it. This module does
//! **no I/O and spawns no process**; the `src-tauri` adapter performs the OS
//! side-effect from the returned [`OpenPlan`] (the same "core decides, adapter
//! acts" split used for privilege elevation).
//!
//! File associations are the [`plugin::FileTypeHandler`](crate::plugin) extension
//! point in its simplest, config-driven form. Until the config-persistence slice
//! lands, the map is empty by default, so every file falls back to the system
//! default — a working zero-config baseline (§7).

use std::path::Path;

use crate::types::{Config, Entry, EntryKind, OpenAction};

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
}

/// Decide how to fulfil `action` for `entry` living in `cwd`, consulting the
/// config file-type→app map.
///
/// Returns `None` when there is nothing to open — the parent entry `..` or a
/// directory (navigation, not opening, handles those; the frontend never asks us
/// to open a plain directory). `Open` on an executable yields [`OpenPlan::Execute`];
/// every other case yields [`OpenPlan::Launch`] with the configured app (if any)
/// or the system default.
pub fn plan_open(entry: &Entry, action: OpenAction, cwd: &str, config: &Config) -> Option<OpenPlan> {
    if entry.name == ".." || entry.kind == EntryKind::Dir {
        return None;
    }

    let path = Path::new(cwd).join(&entry.name).to_string_lossy().into_owned();

    // Enter (Open) on an executable runs it; View/Edit always launch an app.
    if action == OpenAction::Open && entry.is_executable {
        return Some(OpenPlan::Execute {
            path,
            cwd: cwd.to_string(),
        });
    }

    Some(OpenPlan::Launch {
        path,
        app: resolve_app(&entry.name, action, config),
    })
}

/// The configured app for `name`'s extension and `action`, or `None` (system
/// default). View/Edit fall back to the association's `open` app before giving up.
fn resolve_app(name: &str, action: OpenAction, config: &Config) -> Option<String> {
    let ext = extension(name)?;
    let assoc = config
        .associations
        .iter()
        .find(|a| a.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)))?;
    match action {
        OpenAction::Open => assoc.open.clone(),
        OpenAction::View => assoc.view.clone().or_else(|| assoc.open.clone()),
        OpenAction::Edit => assoc.edit.clone().or_else(|| assoc.open.clone()),
    }
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
    use crate::types::{EntryMarker, FileAssociation};

    fn entry(name: &str, kind: EntryKind, executable: bool) -> Entry {
        Entry {
            name: name.to_string(),
            kind,
            size: 0,
            modified: 0,
            permissions: 0,
            symlink_target: None,
            is_executable: executable,
            marker: EntryMarker::Ok,
        }
    }

    fn config_with(assoc: FileAssociation) -> Config {
        Config {
            associations: vec![assoc],
            ..Config::default()
        }
    }

    #[test]
    fn dotdot_and_dirs_are_not_openable() {
        let cfg = Config::default();
        assert_eq!(plan_open(&entry("..", EntryKind::Dir, false), OpenAction::Open, "/x", &cfg), None);
        assert_eq!(plan_open(&entry("sub", EntryKind::Dir, false), OpenAction::Open, "/x", &cfg), None);
    }

    #[test]
    fn plain_file_with_no_association_uses_system_default() {
        let cfg = Config::default();
        let plan = plan_open(&entry("notes.txt", EntryKind::File, false), OpenAction::Open, "/home", &cfg);
        assert_eq!(
            plan,
            Some(OpenPlan::Launch { path: "/home/notes.txt".to_string(), app: None })
        );
    }

    #[test]
    fn executable_enter_runs_it_but_view_still_launches() {
        let cfg = Config::default();
        let exe = entry("run.sh", EntryKind::File, true);
        assert_eq!(
            plan_open(&exe, OpenAction::Open, "/home", &cfg),
            Some(OpenPlan::Execute { path: "/home/run.sh".to_string(), cwd: "/home".to_string() })
        );
        // F3/F4 on an executable open it in an editor rather than running it.
        assert_eq!(
            plan_open(&exe, OpenAction::View, "/home", &cfg),
            Some(OpenPlan::Launch { path: "/home/run.sh".to_string(), app: None })
        );
    }

    #[test]
    fn association_maps_apps_per_action_with_fallback() {
        let cfg = config_with(FileAssociation {
            extensions: vec!["md".into(), "markdown".into()],
            open: Some("Typora".into()),
            view: None,               // View falls back to `open`
            edit: Some("Code".into()),
            ..Default::default()
        });
        let md = entry("README.md", EntryKind::File, false);
        let app = |a| match plan_open(&md, a, "/w", &cfg) {
            Some(OpenPlan::Launch { app, .. }) => app,
            other => panic!("expected Launch, got {other:?}"),
        };
        assert_eq!(app(OpenAction::Open), Some("Typora".into()));
        assert_eq!(app(OpenAction::View), Some("Typora".into())); // fell back to open
        assert_eq!(app(OpenAction::Edit), Some("Code".into()));
        // Case-insensitive extension match.
        let md_upper = entry("READ.MARKDOWN", EntryKind::File, false);
        assert!(matches!(
            plan_open(&md_upper, OpenAction::Edit, "/w", &cfg),
            Some(OpenPlan::Launch { app: Some(ref a), .. }) if a == "Code"
        ));
    }

    #[test]
    fn dotfile_without_extension_has_no_association() {
        assert_eq!(extension(".bashrc"), None);
        assert_eq!(extension("archive.tar.gz"), Some("gz".to_string()));
        assert_eq!(extension("NOEXT"), None);
    }
}
