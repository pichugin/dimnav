//! Tauri command handlers — the thin request/response half of the IPC adapter.
//!
//! Each handler is marshalling only: it locks the shared [`AppState`], calls into
//! `fm-core`, and returns a serializable result. No business logic lives here
//! (SPEC §3). Navigation commands return a full [`AppSnapshot`] so the frontend
//! can replace its whole render state in one step.
//!
//! Filesystem reads run on a blocking thread pool and never hold the state lock
//! across an `.await`, so the UI thread is never blocked (SPEC §5.4a).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};

use fm_core::open::{EmbeddedMode, OpenPlan};
use fm_core::state::AppState;
use fm_core::types::{
    AppInfo, AppSnapshot, Config, DeleteRequest, DirListing, EditDoc, EntryKind, ErrorResolution,
    GotoTarget, HelpBook, HistoryDir, KeyBinding, MediaKind, Motion, NavTarget, OpKind, OpenAction,
    OpenOutcome, OpRequest, PanelId, Resolution, SaveOutcome, SearchDirection, SortMode,
    TerminalBuffer, ViewMode, ViewMotion, ViewPage, ViewerMode,
};
use fm_core::view::{edit::Docs, Sessions};
use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::events::OpCompleteEvent;
use crate::ops_runtime::{registry, OpRegistry, TauriObserver, UserInput};
use crate::terminal_runtime::{self, TerminalRuntime};
use crate::watch_runtime::WatchRuntime;
use fm_core::plugin::FsObserver;

/// Tauri-managed shared navigation state.
pub type SharedState = Mutex<AppState>;

/// Tauri-managed embedded-viewer sessions and editor documents. Both registries
/// live in `fm-core`; the adapter only owns the lock around them.
pub type ViewState = Mutex<Sessions>;
pub type EditState = Mutex<Docs>;

fn view_lock<T>(_: PoisonError<T>) -> String {
    "viewer state lock was poisoned".to_string()
}

fn lock_err<T>(_: PoisonError<T>) -> String {
    "navigation state lock was poisoned".to_string()
}

/// Default starting directory for a fresh session, and the landing spot when a
/// panel's directory goes away for good (§5.6).
pub(crate) fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}

/// The directory a panel should open at: its persisted `start_dir` when that
/// still exists, otherwise home (§7 — a stale config must never strand a panel).
fn start_dir(prefs: &fm_core::types::PanelPrefs) -> String {
    match prefs.start_dir.as_deref() {
        Some(d) if Path::new(d).is_dir() => d.to_string(),
        _ => home_dir(),
    }
}

/// Check that entries resolved from a panel's in-memory listing still describe
/// what is on disk, before a destructive operation acts on them.
///
/// A panel's listing can lag the filesystem. That gap is much smaller now that
/// directories are watched, but it is never zero — and an operation built from a
/// stale name would otherwise fail somewhere deep inside the pipeline with a
/// message that does not say which entry was wrong.
///
/// This is deliberately *not* a fix for the general time-of-check/time-of-use
/// race, which no check of this shape can close: a path can always be swapped
/// between here and the operation. It catches the case that actually occurs — a
/// listing that has not caught up yet — and fails early and legibly.
fn verify_current(targets: &[(String, EntryKind)]) -> Result<(), String> {
    for (path, kind) in targets {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return Err(format!(
                "{path} is no longer there — the listing was out of date. Refresh and try again."
            ));
        };
        let actual = if meta.file_type().is_symlink() {
            EntryKind::Symlink
        } else if meta.is_dir() {
            EntryKind::Dir
        } else if meta.is_file() {
            EntryKind::File
        } else {
            EntryKind::Special
        };
        if actual != *kind {
            return Err(format!(
                "{path} is no longer a {kind:?} — the listing was out of date. Refresh and try again."
            ));
        }
    }
    Ok(())
}

/// Persist the current preferences off the caller's thread. Panel state is folded
/// into the config first so there is exactly one path from live state to disk
/// (§5.8). Write failures are non-fatal by design — see `fm_core::config::save`.
fn persist(state: &mut fm_core::state::AppState) {
    state.sync_prefs_from_panels();
    let config = state.config.clone();
    tauri::async_runtime::spawn_blocking(move || fm_core::config::save(&config));
}

// --- Liveness / config ------------------------------------------------------

/// Liveness check used to verify the IPC pipeline round-trips.
#[tauri::command]
#[specta::specta]
pub fn ping() -> String {
    "pong".to_string()
}

/// Return the configuration currently in effect (loaded from TOML at `init`, §7).
#[tauri::command]
#[specta::specta]
pub fn get_config(state: State<'_, SharedState>) -> Result<Config, String> {
    let s = state.lock().map_err(lock_err)?;
    Ok(s.config.clone())
}

/// The active keymap (action id → key chords), sourced from core config so the
/// webview never hardcodes keys (SPEC §6).
#[tauri::command]
#[specta::specta]
pub fn get_keymap() -> Vec<KeyBinding> {
    fm_core::config::default_keymap()
}

/// The F1 help book, filtered by `query` (§6). Both the content and the search
/// matching are the core's: this only supplies the packaging metadata, which is
/// the one thing `fm-core` cannot know about itself.
///
/// `package_info()` resolves to the bundle's `productName` from `tauri.conf.json`
/// and its `version` — which, since `tauri.conf.json` omits `version`, the
/// bundler takes from `Cargo.toml`, making the workspace manifest the single
/// source of truth. The remaining fields come straight from that same manifest,
/// so there is no hand-maintained copy of any of this to drift.
#[tauri::command]
#[specta::specta]
pub fn get_help(app: AppHandle, query: String) -> HelpBook {
    let pkg = app.package_info();
    let info = AppInfo {
        name: pkg.name.clone(),
        version: pkg.version.to_string(),
        description: env!("CARGO_PKG_DESCRIPTION").to_string(),
        license: env!("CARGO_PKG_LICENSE").to_string(),
        homepage: env!("CARGO_PKG_HOMEPAGE").to_string(),
        repository: env!("CARGO_PKG_REPOSITORY").to_string(),
        // Cargo has no sponsorship field, so this is the one string with nowhere
        // else to live. Keep it beside the rest of the identity rather than
        // hiding it in the core, which owns no packaging facts at all.
        sponsor: SPONSOR_URL.to_string(),
    };
    // Same keymap source as `get_keymap`, so help can never disagree with what
    // the keyboard actually does.
    fm_core::help::book(&info, &fm_core::config::default_keymap(), &query)
}

/// Where the About topic's "Support this project" row points.
const SPONSOR_URL: &str = "https://github.com/sponsors/pichugin";

/// A newer release than the one running, as advertised by the update feed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct UpdateInfo {
    pub version: String,
    /// Release notes from the feed. Often empty; the renderer must cope.
    pub notes: String,
}

/// Ask the update feed whether a newer release exists (§ release process).
///
/// `Ok(None)` means "up to date" *and* "could not tell" — the two are
/// deliberately not distinguished. This runs unprompted at startup, and a laptop
/// that is offline, behind a captive portal, or simply ahead of a not-yet-created
/// release must not produce an error the user has to dismiss. Real failures are
/// still worth seeing while developing, so they are logged.
///
/// Updates are verified against the public key in `tauri.conf.json` before
/// anything is written to disk; an unsigned or mis-signed payload fails here
/// rather than being installed.
#[tauri::command]
#[specta::specta]
pub async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => {
            eprintln!("updater unavailable: {e}");
            return Ok(None);
        }
    };
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(UpdateInfo {
            version: update.version.clone(),
            notes: update.body.clone().unwrap_or_default(),
        })),
        Ok(None) => Ok(None),
        Err(e) => {
            eprintln!("update check failed: {e}");
            Ok(None)
        }
    }
}

/// Download, verify, install the pending update, then relaunch into it.
///
/// Unlike [`check_update`] this is user-initiated, so failures are surfaced: the
/// user pressed a button and is owed an answer. Does not return on success — the
/// process is replaced by the new build.
#[tauri::command]
#[specta::specta]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;

    let update = app
        .updater()
        .map_err(|e| format!("updater unavailable: {e}"))?
        .check()
        .await
        .map_err(|e| format!("could not reach the update service: {e}"))?
        .ok_or_else(|| "no update is available".to_string())?;

    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("could not install the update: {e}"))?;

    app.restart();
}

/// Open one of the About topic's links in the user's browser (§6).
///
/// Restricted to `http`/`https` on purpose. The webview only ever passes URLs the
/// core just handed it, but this command is reachable from any frontend code, and
/// the opener plugin will happily launch `file://` paths or custom schemes —
/// which is a local-file-execution primitive, not a browser link. Narrowing the
/// scheme here keeps that door shut regardless of what the renderer does.
#[tauri::command]
#[specta::specta]
pub fn open_link(app: AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) links can be opened".to_string());
    }
    tauri_plugin_opener::OpenerExt::opener(&app)
        .open_url(url, None::<&str>)
        .map_err(|e| format!("could not open link: {e}"))
}

/// List a directory into a structured [`DirListing`] (utility; panels use the
/// stateful commands below).
#[tauri::command]
#[specta::specta]
pub fn list_dir(path: String, show_hidden: bool, sort: SortMode) -> DirListing {
    fm_core::fs::list_dir(&path, show_hidden, sort)
}

// --- Stateful navigation ----------------------------------------------------

/// Load the persisted configuration and populate both panels from it — each panel
/// reopens its last directory with its own view/sort/hidden state (§5.8 / §7),
/// falling back to home when a remembered directory is gone. Call once on boot.
#[tauri::command]
#[specta::specta]
pub async fn init(
    state: State<'_, SharedState>,
    watch: State<'_, WatchRuntime>,
) -> Result<AppSnapshot, String> {
    let config = tauri::async_runtime::spawn_blocking(fm_core::config::load)
        .await
        .map_err(|e| e.to_string())?;

    let (dir_left, dir_right) = (start_dir(&config.left_panel), start_dir(&config.right_panel));
    let (pl, pr) = (config.left_panel.clone(), config.right_panel.clone());

    // Command history is a separate file beside config.toml, so it is read the
    // same way: off-thread, and never fatal if it is missing (§5.7 / §7).
    let history = tauri::async_runtime::spawn_blocking(fm_core::terminal::history::load)
        .await
        .map_err(|e| e.to_string())?;

    {
        let mut s = state.lock().map_err(lock_err)?;
        s.apply_config(config);
        s.terminal.set_history(history);
    }

    let left = tauri::async_runtime::spawn_blocking(move || {
        fm_core::fs::list_dir(&dir_left, pl.show_hidden, pl.sort_mode)
    })
    .await
    .map_err(|e| e.to_string())?;
    let right = tauri::async_runtime::spawn_blocking(move || {
        fm_core::fs::list_dir(&dir_right, pr.show_hidden, pr.sort_mode)
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::set_listing(&mut s.left, left);
    fm_core::nav::set_listing(&mut s.right, right);
    let snapshot = s.snapshot();
    drop(s);

    // Start watching both panels' directories so outside changes show up without
    // the user asking (§5.6).
    watch.observe(PanelId::Left, Path::new(&snapshot.left.path));
    watch.observe(PanelId::Right, Path::new(&snapshot.right.path));
    Ok(snapshot)
}

// --- Per-panel view state (§5.8) --------------------------------------------

/// Set a panel's view mode — 1/2/3-column brief or the detailed single-column
/// mode. Pure layout: no directory re-read is needed, only the cursor geometry
/// the frontend re-reports afterwards. Persisted per panel.
#[tauri::command]
#[specta::specta]
pub fn set_view_mode(
    state: State<'_, SharedState>,
    panel: PanelId,
    mode: ViewMode,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    s.panel_mut(panel).view_mode = mode;
    persist(&mut s);
    Ok(s.snapshot_after_input())
}

/// Set a panel's sort mode (§5.8). Re-sorts the entries already loaded — no I/O —
/// keeping the cursor and selection on the same entries. Persisted per panel.
#[tauri::command]
#[specta::specta]
pub fn set_sort_mode(
    state: State<'_, SharedState>,
    panel: PanelId,
    mode: SortMode,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    let p = s.panel_mut(panel);
    p.sort_mode = mode;
    fm_core::nav::resort(p);
    persist(&mut s);
    Ok(s.snapshot_after_input())
}

/// Show or hide dotfiles in a panel (§5.8 — shown by default). Needs a re-read,
/// which runs off the state lock; the cursor and selection survive by name.
/// Persisted per panel.
#[tauri::command]
#[specta::specta]
pub async fn set_show_hidden(
    state: State<'_, SharedState>,
    panel: PanelId,
    value: bool,
) -> Result<AppSnapshot, String> {
    let (path, sort) = {
        let mut s = state.lock().map_err(lock_err)?;
        let p = s.panel_mut(panel);
        p.show_hidden = value;
        (p.path.clone(), p.sort_mode)
    };

    let listing =
        tauri::async_runtime::spawn_blocking(move || fm_core::fs::list_dir(&path, value, sort))
            .await
            .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::set_listing_preserving(s.panel_mut(panel), listing);
    persist(&mut s);
    Ok(s.snapshot_after_input())
}

/// Report a panel's rendered layout so the cursor state machine can compute the
/// column/page traversal (SPEC §5.2).
#[tauri::command]
#[specta::specta]
pub fn set_viewport(
    state: State<'_, SharedState>,
    panel: PanelId,
    columns: u16,
    rows: u16,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::set_geometry(s.panel_mut(panel), columns, rows);
    Ok(s.snapshot())
}

/// Apply a cursor motion (SPEC §5.2). Pure and instant — runs synchronously.
#[tauri::command]
#[specta::specta]
pub fn move_cursor(
    state: State<'_, SharedState>,
    panel: PanelId,
    motion: Motion,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::move_cursor(s.panel_mut(panel), motion);
    Ok(s.snapshot_after_input())
}

/// Move a panel's cursor to an explicit entry index (SPEC §5.2). Backs mouse-click
/// focus: the frontend reports the clicked entry's global index; the core clamps
/// and owns the resulting cursor position. Pure and instant — runs synchronously.
#[tauri::command]
#[specta::specta]
pub fn set_cursor(
    state: State<'_, SharedState>,
    panel: PanelId,
    index: usize,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::set_cursor(s.panel_mut(panel), index);
    Ok(s.snapshot_after_input())
}

/// Set the active (focused) panel (SPEC §5.1).
#[tauri::command]
#[specta::specta]
pub fn set_active_panel(
    state: State<'_, SharedState>,
    panel: PanelId,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    s.set_active(panel);
    // Reaching for a panel — by Tab or by clicking it — is a request for the
    // keyboard to be there, so the prompt gives it up (§5.7). Ignored while the
    // Esc curtain is drawn, since there is no visible panel to hand it to.
    s.terminal.set_focused(false);
    Ok(s.snapshot_after_input())
}

// --- Quick search (§5.9) ----------------------------------------------------
//
// Four intents, no policy: the core owns the query, the matching, and the
// accept-or-reject decision, and the frontend renders whatever comes back
// (CLAUDE.md). All four return through `snapshot_after_search_input` rather than
// `snapshot_after_input`, since the latter is precisely what *ends* a search —
// these are the one kind of input that must not.

/// Cmd+F — open a fresh search box on the panel. The cursor does not move.
#[tauri::command]
#[specta::specta]
pub fn search_start(state: State<'_, SharedState>, panel: PanelId) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::search::open(s.panel_mut(panel));
    Ok(s.snapshot_after_search_input())
}

/// Append typed text to the query and jump the cursor to the first match. A
/// character matching nothing is rejected and bumps `search.miss_rev` instead,
/// which is the frontend's cue to beep.
///
/// Takes a `String` rather than a `char`: it marshals cleanly through specta, and
/// a paste into the box later needs no second command.
#[tauri::command]
#[specta::specta]
pub fn search_push(
    state: State<'_, SharedState>,
    panel: PanelId,
    text: String,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::search::push(s.panel_mut(panel), &text);
    Ok(s.snapshot_after_search_input())
}

/// Backspace — drop the last character and let the cursor follow it back.
#[tauri::command]
#[specta::specta]
pub fn search_backspace(
    state: State<'_, SharedState>,
    panel: PanelId,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::search::backspace(s.panel_mut(panel));
    Ok(s.snapshot_after_search_input())
}

/// Esc, Enter, or the box's ✕ — close it, leaving the cursor on the match.
#[tauri::command]
#[specta::specta]
pub fn search_close(state: State<'_, SharedState>, panel: PanelId) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::search::close(s.panel_mut(panel));
    Ok(s.snapshot_after_search_input())
}

// --- Selection (§5.3) -------------------------------------------------------

/// Toggle selection of the entry under the cursor (Space). `..` is a no-op.
#[tauri::command]
#[specta::specta]
pub fn toggle_selection(
    state: State<'_, SharedState>,
    panel: PanelId,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::toggle_selection(s.panel_mut(panel));
    Ok(s.snapshot_after_input())
}

/// Move the cursor, flipping the selection of every entry it sweeps over
/// (Shift+Arrow / PageUp / PageDown / Home / End). The entry the cursor leaves is
/// flipped, the one it lands on is not, so repeated presses paint a continuous run.
#[tauri::command]
#[specta::specta]
pub fn select_and_move(
    state: State<'_, SharedState>,
    panel: PanelId,
    motion: Motion,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::toggle_range_and_move(s.panel_mut(panel), motion);
    Ok(s.snapshot_after_input())
}

/// Select all selectable entries in the panel (`*`).
#[tauri::command]
#[specta::specta]
pub fn select_all(state: State<'_, SharedState>, panel: PanelId) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::select_all(s.panel_mut(panel));
    Ok(s.snapshot_after_input())
}

/// Clear the panel's selection (`-`).
#[tauri::command]
#[specta::specta]
pub fn deselect_all(state: State<'_, SharedState>, panel: PanelId) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::deselect_all(s.panel_mut(panel));
    Ok(s.snapshot_after_input())
}

/// Navigate a panel: into the entry under the cursor, up to the parent (with the
/// auto-position-on-exited-folder rule, §5.2), or to an explicit path.
#[tauri::command]
#[specta::specta]
pub async fn navigate(
    state: State<'_, SharedState>,
    watch: State<'_, WatchRuntime>,
    panel: PanelId,
    target: NavTarget,
) -> Result<AppSnapshot, String> {
    // Capture what we need from state, then drop the lock before any I/O.
    let (cur_path, show_hidden, sort, cursor_entry) = {
        let s = state.lock().map_err(lock_err)?;
        let p = s.panel(panel);
        let entry = p
            .entries
            .get(p.cursor_index)
            .map(|e| (e.name.clone(), e.kind));
        (p.path.clone(), p.show_hidden, p.sort_mode, entry)
    };

    // Resolve the destination path and, for a parent hop, the child to land on.
    let (target_path, exited_child): (Option<String>, Option<String>) = match target {
        NavTarget::Path(path) => (Some(path), None),
        NavTarget::Parent => (
            fm_core::nav::parent_of(&cur_path),
            fm_core::nav::child_name(&cur_path),
        ),
        NavTarget::Into => match cursor_entry {
            // Enter on `..` behaves like Parent.
            Some((name, _)) if name == ".." => (
                fm_core::nav::parent_of(&cur_path),
                fm_core::nav::child_name(&cur_path),
            ),
            Some((name, kind)) => {
                let joined = Path::new(&cur_path).join(&name);
                // Descend into directories; follow symlinks that resolve to a
                // directory (SPEC §5.4a "follow for navigation"). Files are a
                // no-op this slice (opening files is a later slice).
                let navigable =
                    kind == EntryKind::Dir || (kind == EntryKind::Symlink && joined.is_dir());
                if navigable {
                    (Some(joined.to_string_lossy().into_owned()), None)
                } else {
                    (None, None)
                }
            }
            None => (None, None),
        },
    };

    let Some(path) = target_path else {
        // Nothing to do (file under cursor, or parent at a root): return as-is.
        let mut s = state.lock().map_err(lock_err)?;
        return Ok(s.snapshot_after_input());
    };

    let listing = tauri::async_runtime::spawn_blocking(move || {
        fm_core::fs::list_dir(&path, show_hidden, sort)
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    let p = s.panel_mut(panel);
    fm_core::nav::set_listing(p, listing);
    if let Some(name) = exited_child {
        fm_core::nav::position_on(p, &name);
    }
    // A deliberate move clears any notice the watcher left behind (§5.6).
    s.panel_mut(panel).notice = None;
    // Remember the new directory so the next launch reopens here (§7).
    persist(&mut s);
    let snapshot = s.snapshot_after_input();
    let now = PathBuf::from(&s.panel(panel).path);
    drop(s);

    // Point the watcher at wherever this panel just landed.
    watch.observe(panel, &now);
    Ok(snapshot)
}

/// Show the active panel's folder on the other panel too (Ctrl+=).
///
/// A push, not a Tab: only the passive panel moves, so the keyboard stays where
/// the user left it. Delegating to [`navigate`] rather than listing here is what
/// keeps the watcher re-armed, the new directory persisted, and the cursor rules
/// identical to every other directory change — the same reason `terminal_run`'s
/// `cd` arm delegates.
#[tauri::command]
#[specta::specta]
pub async fn equalize_panels(
    state: State<'_, SharedState>,
    watch: State<'_, WatchRuntime>,
) -> Result<AppSnapshot, String> {
    let (other, path, already_there) = {
        let s = state.lock().map_err(lock_err)?;
        let other = s.active.other();
        let path = s.panel(s.active).path.clone();
        let already_there = path == s.panel(other).path;
        (other, path, already_there)
    };

    // Nothing to mirror: an empty path means the panel has not been populated
    // yet, and a panel already showing the folder must not have its cursor
    // knocked back to `..` by a second press.
    if path.is_empty() || already_there {
        let mut s = state.lock().map_err(lock_err)?;
        return Ok(s.snapshot_after_input());
    }

    navigate(state, watch, other, NavTarget::Path(path)).await
}

/// Re-read a panel's current directory, keeping the cursor on the same entry name
/// when it still exists (graceful refresh, SPEC §5.6).
#[tauri::command]
#[specta::specta]
pub async fn refresh(state: State<'_, SharedState>, panel: PanelId) -> Result<AppSnapshot, String> {
    let (path, show_hidden, sort) = {
        let s = state.lock().map_err(lock_err)?;
        let p = s.panel(panel);
        (p.path.clone(), p.show_hidden, p.sort_mode)
    };

    let listing = tauri::async_runtime::spawn_blocking(move || {
        fm_core::fs::list_dir(&path, show_hidden, sort)
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    // Cursor *and* selection follow the entries by name across the re-read (§5.6).
    fm_core::nav::set_listing_preserving(s.panel_mut(panel), listing);
    // Drop any cached folder sizes under this directory whose contents changed
    // underneath us (external edits) — a user-initiated safety net (§ caching).
    let base = PathBuf::from(&s.panel(panel).path);
    s.revalidate_sizes_under(&base);
    Ok(s.snapshot())
}

// --- Create directory (F7) / Rename (Shift+F6) (§5.4) -----------------------

/// Create a directory named `name` inside `panel`'s current directory (F7), then
/// re-read the listing and position the cursor on the new folder. `name` may be a
/// nested relative path (`a/b`), in which case the cursor lands on the first
/// component.
#[tauri::command]
#[specta::specta]
pub async fn create_dir(
    state: State<'_, SharedState>,
    panel: PanelId,
    name: String,
) -> Result<AppSnapshot, String> {
    let (path, show_hidden, sort) = {
        let s = state.lock().map_err(lock_err)?;
        let p = s.panel(panel);
        (p.path.clone(), p.show_hidden, p.sort_mode)
    };

    fm_core::fs::make_dir(Path::new(&path), &name)?;

    // The new entry to focus is the first path component of `name`.
    let focus = Path::new(&name)
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .map(str::to_string);

    let listing = tauri::async_runtime::spawn_blocking(move || {
        fm_core::fs::list_dir(&path, show_hidden, sort)
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    // Adding a folder changes the parent's total, so invalidate it and its
    // ancestors (§ caching).
    let parent = PathBuf::from(&s.panel(panel).path);
    s.invalidate_size_cache(&parent);
    let p = s.panel_mut(panel);
    fm_core::nav::set_listing(p, listing);
    if let Some(name) = focus {
        fm_core::nav::position_on(p, &name);
    }
    Ok(s.snapshot_after_input())
}

/// Rename the entry under `panel`'s cursor to `new_name`, in place (Shift+F6).
/// Errors if the cursor is on `..`, `new_name` is empty / contains a separator, or
/// a file of that name already exists (never silently overwrites, §5.4a). On
/// success, re-reads the listing and keeps the cursor on the renamed entry.
#[tauri::command]
#[specta::specta]
pub async fn rename(
    state: State<'_, SharedState>,
    panel: PanelId,
    new_name: String,
) -> Result<AppSnapshot, String> {
    let (path, show_hidden, sort, old) = {
        let s = state.lock().map_err(lock_err)?;
        let p = s.panel(panel);
        let old = p
            .entries
            .get(p.cursor_index)
            .map(|e| e.name.clone())
            .ok_or_else(|| "nothing to rename".to_string())?;
        (p.path.clone(), p.show_hidden, p.sort_mode, old)
    };

    fm_core::fs::rename_entry(Path::new(&path), &old, &new_name)?;
    let focus = new_name.trim().to_string();

    let listing = tauri::async_runtime::spawn_blocking(move || {
        fm_core::fs::list_dir(&path, show_hidden, sort)
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    // A rename can change what a cached folder covers; invalidate the old and new
    // paths and their ancestors (§ caching).
    let base = PathBuf::from(&s.panel(panel).path);
    s.invalidate_size_cache(&base.join(&old));
    s.invalidate_size_cache(&base.join(new_name.trim()));
    let p = s.panel_mut(panel);
    fm_core::nav::set_listing(p, listing);
    fm_core::nav::position_on(p, &focus);
    Ok(s.snapshot_after_input())
}

// --- Recursive folder size (F3 on a folder) ---------------------------------

/// Recursively compute the size of each folder in `paths` and cache the results,
/// then return a fresh snapshot with the computed sizes surfaced onto the
/// matching dir entries (`computed_size`). This always recomputes the requested
/// paths — pressing F3 again is the explicit "recalculate" gesture.
///
/// The walk runs on the blocking thread pool and never holds the state lock, so
/// large trees never block the UI (§5.4a). Infinite recursion is prevented by
/// [`fm_core::fs::dir_size`] (symlinks are never followed).
#[tauri::command]
#[specta::specta]
pub async fn calculate_dir_size(
    state: State<'_, SharedState>,
    paths: Vec<String>,
) -> Result<AppSnapshot, String> {
    let results = tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|p| {
                let (size, mtime) = fm_core::fs::dir_size(Path::new(&p));
                (PathBuf::from(p), size, mtime)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    for (path, size, mtime) in results {
        s.set_size(path, size, mtime);
    }
    Ok(s.snapshot_after_input())
}

// --- File operations: copy / move (§5.4a) -----------------------------------

/// Begin a copy (F5) or move (F6) of the active panel's selection — or, when
/// nothing is selected, the entry under the cursor (never `..`). `dest` is the
/// editable destination from the F5/F6 prompt; it accepts `..`, relative, and
/// absolute paths, resolved against the active panel's directory (§5.4a).
///
/// Registers the op, spawns the transfer on a background thread (never blocking
/// the UI), and returns the `op_id` the frontend uses to correlate the progress /
/// collision / error / complete events and to cancel.
#[tauri::command]
#[specta::specta]
pub fn start_transfer(
    app: AppHandle,
    state: State<'_, SharedState>,
    registry_state: State<'_, OpRegistry>,
    kind: OpKind,
    dest: String,
) -> Result<String, String> {
    // Build the request from the active panel while holding the lock, then drop it
    // before any I/O.
    let req = {
        let s = state.lock().map_err(lock_err)?;
        let p = s.panel(s.active);
        let base = PathBuf::from(&p.path);

        // Selection drives the op; empty selection falls back to the cursor entry.
        let indices: Vec<usize> = if p.selection.is_empty() {
            vec![p.cursor_index]
        } else {
            let mut v = p.selection.clone();
            v.sort_unstable();
            v
        };

        let targets: Vec<(String, EntryKind)> = indices
            .into_iter()
            .filter_map(|i| p.entries.get(i))
            .filter(|e| e.name != "..")
            .map(|e| (base.join(&e.name).to_string_lossy().into_owned(), e.kind))
            .collect();

        let dest_path = fm_core::ops::resolve_dest(&base, &dest);
        (
            targets,
            OpRequest {
                kind,
                sources: Vec::new(),
                dest: dest_path.to_string_lossy().into_owned(),
            },
        )
    };
    let (targets, mut req) = req;

    if targets.is_empty() {
        return Err("nothing to transfer".to_string());
    }
    // Fail early and legibly if the listing these came from is out of date.
    verify_current(&targets)?;
    req.sources = targets.into_iter().map(|(p, _)| p).collect();

    // A copy/move destination is always a folder to place sources into. Renaming in
    // place is a separate, explicit action (Shift+F6), so a single-source transfer
    // to a path that does not exist must NOT silently rename — surface an error
    // instead (per product decision; the engine still supports rename for internal
    // reuse). Multiple sources to a missing path stay allowed: that means "create a
    // new folder for them".
    if req.sources.len() == 1 && !Path::new(&req.dest).exists() {
        return Err(format!("destination folder does not exist: {}", req.dest));
    }

    // The transfer changes both the destination's total and (for a move) each
    // source's parent total, so drop their cached folder sizes (§ caching).
    {
        let mut s = state.lock().map_err(lock_err)?;
        s.invalidate_size_cache(Path::new(&req.dest));
        for src in &req.sources {
            s.invalidate_size_cache(Path::new(src));
        }
    }

    let (tx, rx) = std::sync::mpsc::channel::<UserInput>();
    let cancel = Arc::new(AtomicBool::new(false));
    let op_id = registry_state.register(tx, cancel.clone());

    let app = app.clone();
    let thread_id = op_id.clone();
    std::thread::spawn(move || {
        let observer = TauriObserver {
            app: app.clone(),
            op_id: thread_id.clone(),
            rx,
            cancel,
        };
        let mut outcome = fm_core::ops::run_transfer(&req, &observer);
        outcome.op_id = thread_id.clone();
        let _ = OpCompleteEvent(outcome).emit(&app);
        // Retire the op so late resolve_* calls fail cleanly.
        registry(&app).remove(&thread_id);
    });

    Ok(op_id)
}

/// Set the global "Move to Trash" default (the delete-dialog checkbox), OFF by
/// default and persisted across sessions (§5.4a).
#[tauri::command]
#[specta::specta]
pub fn set_trash_default(
    state: State<'_, SharedState>,
    value: bool,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    s.config.trash_default = value;
    persist(&mut s);
    Ok(s.snapshot_after_input())
}

/// Begin a delete (F8) of the active panel's selection — or, when nothing is
/// selected, the entry under the cursor (never `..`). Reads the persisted
/// "Move to Trash" flag from state; when set, items go to the OS trash, otherwise
/// they are permanently removed (§5.4a). Mirrors [`start_transfer`]: registers the
/// op, spawns the delete on a background thread (never blocking the UI), and
/// returns the `op_id` the frontend uses to correlate progress / error / complete
/// events and to cancel.
#[tauri::command]
#[specta::specta]
pub fn start_delete(
    app: AppHandle,
    state: State<'_, SharedState>,
    registry_state: State<'_, OpRegistry>,
) -> Result<String, String> {
    let req = {
        let s = state.lock().map_err(lock_err)?;
        let p = s.panel(s.active);
        let base = PathBuf::from(&p.path);

        // Selection drives the op; empty selection falls back to the cursor entry.
        let indices: Vec<usize> = if p.selection.is_empty() {
            vec![p.cursor_index]
        } else {
            let mut v = p.selection.clone();
            v.sort_unstable();
            v
        };

        let targets: Vec<(String, EntryKind)> = indices
            .into_iter()
            .filter_map(|i| p.entries.get(i))
            .filter(|e| e.name != "..")
            .map(|e| (base.join(&e.name).to_string_lossy().into_owned(), e.kind))
            .collect();

        (
            targets,
            DeleteRequest {
                paths: Vec::new(),
                to_trash: s.trash_default(),
            },
        )
    };
    let (targets, mut req) = req;

    if targets.is_empty() {
        return Err("nothing to delete".to_string());
    }
    // Deleting is irreversible without the Trash, so refuse outright rather than
    // acting on a name the listing no longer describes.
    verify_current(&targets)?;
    req.paths = targets.into_iter().map(|(p, _)| p).collect();

    // Deleting changes each parent's total, so drop cached folder sizes for the
    // targets and their ancestors (§ caching).
    {
        let mut s = state.lock().map_err(lock_err)?;
        for path in &req.paths {
            s.invalidate_size_cache(Path::new(path));
        }
    }

    let (tx, rx) = std::sync::mpsc::channel::<UserInput>();
    let cancel = Arc::new(AtomicBool::new(false));
    let op_id = registry_state.register(tx, cancel.clone());

    let app = app.clone();
    let thread_id = op_id.clone();
    std::thread::spawn(move || {
        let observer = TauriObserver {
            app: app.clone(),
            op_id: thread_id.clone(),
            rx,
            cancel,
        };
        let mut outcome = fm_core::ops::run_delete(&req, &observer);
        outcome.op_id = thread_id.clone();
        let _ = OpCompleteEvent(outcome).emit(&app);
        registry(&app).remove(&thread_id);
    });

    Ok(op_id)
}

/// Answer a collision prompt for a running op (§5.4a).
#[tauri::command]
#[specta::specta]
pub fn resolve_collision(
    registry_state: State<'_, OpRegistry>,
    op_id: String,
    resolution: Resolution,
) -> Result<(), String> {
    registry_state.send(&op_id, UserInput::Collision(resolution))
}

/// Answer an error prompt for a running op — Retry / Skip / Skip All / Cancel /
/// Elevate (§5.6).
#[tauri::command]
#[specta::specta]
pub fn resolve_error(
    registry_state: State<'_, OpRegistry>,
    op_id: String,
    resolution: ErrorResolution,
) -> Result<(), String> {
    registry_state.send(&op_id, UserInput::Error(resolution))
}

/// Request cancellation of a running op; the engine stops between items (§5.4a).
#[tauri::command]
#[specta::specta]
pub fn cancel_op(registry_state: State<'_, OpRegistry>, op_id: String) -> Result<(), String> {
    registry_state.cancel(&op_id);
    Ok(())
}

// --- Open / View / Edit (§5.5) ----------------------------------------------

/// Open the entry under `panel`'s cursor. `Open` (Enter) uses the system default
/// and *runs* executables; `View` (F3) / `Edit` (F4) route to the embedded viewer
/// or editor, or to a configured external tool (§5.5).
///
/// The core (`fm_core::open::plan_open`) sniffs the file and makes the whole
/// decision; this handler only performs the side-effect it asks for and returns
/// whatever the frontend needs to render (SPEC §3).
#[tauri::command]
#[specta::specta]
pub fn open_entry(
    app: AppHandle,
    state: State<'_, SharedState>,
    runtime: State<'_, TerminalRuntime>,
    view_state: State<'_, ViewState>,
    edit_state: State<'_, EditState>,
    panel: PanelId,
    action: OpenAction,
) -> Result<OpenOutcome, String> {
    let (entry, cwd, config) = {
        let mut s = state.lock().map_err(lock_err)?;
        s.terminal.touch();
        let p = s.panel(panel);
        (
            p.entries.get(p.cursor_index).cloned(),
            p.path.clone(),
            s.config.clone(),
        )
    };
    let Some(entry) = entry else {
        return Ok(OpenOutcome::Nothing); // empty panel — nothing under the cursor
    };

    let (plan, probe) = fm_core::open::plan_open(&entry, action, &cwd, &config);
    let Some(plan) = plan else {
        // `..` or a directory — navigation handles those, not opening.
        return Ok(OpenOutcome::Nothing);
    };

    match plan {
        OpenPlan::Launch { path, app: with_app } => {
            launch(&path, with_app.as_deref())?;
            Ok(OpenOutcome::Launched)
        }
        OpenPlan::Execute { path, cwd } => {
            // Enter-on-executable and a typed command are the same thing from
            // here on: both echo a prompt line and stream into the terminal
            // buffer, so the pane reads the same either way (§5.5 / §5.7).
            let command = fm_core::terminal::quote(&path);
            {
                let mut s = state.lock().map_err(lock_err)?;
                s.terminal.begin_external(command.clone(), &cwd);
            }
            emit_terminal_chunks(&app, &state)?;
            emit_terminal_state(&app, &state)?;
            if let Err(reason) = terminal_runtime::spawn(
                app.clone(),
                &runtime,
                config.terminal.shell.clone(),
                command,
                cwd,
            ) {
                record_spawn_failure(&app, &state, &reason)?;
            }
            Ok(OpenOutcome::Executing)
        }
        OpenPlan::Embedded { path, mode } => {
            // Routing only reaches an embedded plan by way of a successful
            // probe, so the type is known here.
            let probe = probe.ok_or_else(|| format!("could not read {path}"))?;
            match mode {
                EmbeddedMode::Edit => {
                    let mut docs = edit_state.lock().map_err(view_lock)?;
                    Ok(OpenOutcome::Editor(docs.open(
                        &path,
                        probe,
                        config.edit_max_bytes,
                    )?))
                }
                EmbeddedMode::Text | EmbeddedMode::Hex | EmbeddedMode::Image => {
                    let viewer_mode = match mode {
                        EmbeddedMode::Hex => ViewerMode::Hex,
                        EmbeddedMode::Image => ViewerMode::Image,
                        _ => ViewerMode::Text,
                    };
                    let mut sessions = view_state.lock().map_err(view_lock)?;
                    let id = sessions.open(&path, probe, viewer_mode, &config.viewer)?;
                    Ok(OpenOutcome::Viewer(sessions.page(&id)?))
                }
            }
        }
    }
}

// --- Embedded viewer (§5.5) --------------------------------------------------

/// Report the viewer's visible geometry in rows and characters, the same way the
/// panels report theirs: the frontend owns pixels, the core owns what they mean.
#[tauri::command]
#[specta::specta]
pub fn view_set_viewport(
    view_state: State<'_, ViewState>,
    id: String,
    rows: u16,
    cols: u16,
) -> Result<ViewPage, String> {
    view_state
        .lock()
        .map_err(view_lock)?
        .set_viewport(&id, rows, cols)
}

#[tauri::command]
#[specta::specta]
pub fn view_scroll(
    view_state: State<'_, ViewState>,
    id: String,
    motion: ViewMotion,
) -> Result<ViewPage, String> {
    view_state.lock().map_err(view_lock)?.scroll(&id, motion)
}

/// Toggle between text and hex (F4), keeping the byte position.
#[tauri::command]
#[specta::specta]
pub fn view_toggle_hex(view_state: State<'_, ViewState>, id: String) -> Result<ViewPage, String> {
    view_state.lock().map_err(view_lock)?.toggle_mode(&id)
}

/// Toggle word wrap (F2) and persist it, the way the panels persist their view
/// state (§5.8).
#[tauri::command]
#[specta::specta]
pub fn view_set_wrap(
    state: State<'_, SharedState>,
    view_state: State<'_, ViewState>,
    id: String,
    wrap: bool,
) -> Result<ViewPage, String> {
    {
        let mut s = state.lock().map_err(lock_err)?;
        s.config.viewer.wrap = wrap;
        fm_core::config::save(&s.config);
    }
    view_state.lock().map_err(view_lock)?.set_wrap(&id, wrap)
}

/// Search from just past the current position (F7 / Shift+F7). `Ok(None)` means
/// "not found" — a normal outcome the frontend reports in the status line,
/// rather than an error dialog.
#[tauri::command]
#[specta::specta]
pub fn view_search(
    view_state: State<'_, ViewState>,
    id: String,
    needle: String,
    direction: SearchDirection,
) -> Result<Option<ViewPage>, String> {
    view_state
        .lock()
        .map_err(view_lock)?
        .search(&id, &needle, direction)
}

/// Jump to a line, byte offset, or percentage through the file (F5).
#[tauri::command]
#[specta::specta]
pub fn view_goto(
    view_state: State<'_, ViewState>,
    id: String,
    target: GotoTarget,
) -> Result<ViewPage, String> {
    view_state.lock().map_err(view_lock)?.goto(&id, target)
}

/// Hand the file open in the viewer to the editor (F6). The path comes from the
/// session rather than the frontend, so the webview never names the file it
/// wants opened for writing.
#[tauri::command]
#[specta::specta]
pub fn view_to_edit(
    state: State<'_, SharedState>,
    view_state: State<'_, ViewState>,
    edit_state: State<'_, EditState>,
    id: String,
) -> Result<EditDoc, String> {
    let path = view_state
        .lock()
        .map_err(view_lock)?
        .path_of(&id)
        .ok_or_else(|| format!("viewer session {id} is no longer open"))?;
    let max_bytes = state.lock().map_err(lock_err)?.config.edit_max_bytes;
    let probe = fm_core::view::probe::probe(Path::new(&path))?;
    // The editor is a text editor: decoding a binary into it and saving would
    // corrupt the file, so F6 refuses rather than offering a lossy buffer.
    if probe.media != MediaKind::Text {
        return Err("only text files can be edited".to_string());
    }
    let doc = edit_state
        .lock()
        .map_err(view_lock)?
        .open(&path, probe, max_bytes)?;
    view_state.lock().map_err(view_lock)?.close(&id);
    Ok(doc)
}

#[tauri::command]
#[specta::specta]
pub fn view_close(view_state: State<'_, ViewState>, id: String) -> Result<(), String> {
    view_state.lock().map_err(view_lock)?.close(&id);
    Ok(())
}

// --- Embedded editor (§5.5) --------------------------------------------------

/// Write the buffer back (`editor.save`). `force` answers a previous `Conflict`
/// outcome with "overwrite anyway". The outcome is structured, never a thrown
/// error (§5.6).
#[tauri::command]
#[specta::specta]
pub fn edit_save(
    edit_state: State<'_, EditState>,
    id: String,
    text: String,
    force: bool,
) -> Result<SaveOutcome, String> {
    Ok(edit_state.lock().map_err(view_lock)?.save(&id, &text, force))
}

/// Drop back from the editor to the viewer on the same file (F6).
#[tauri::command]
#[specta::specta]
pub fn edit_to_view(
    state: State<'_, SharedState>,
    view_state: State<'_, ViewState>,
    edit_state: State<'_, EditState>,
    id: String,
) -> Result<ViewPage, String> {
    let path = edit_state
        .lock()
        .map_err(view_lock)?
        .path_of(&id)
        .ok_or_else(|| format!("editor session {id} is no longer open"))?;
    let viewer_prefs = state.lock().map_err(lock_err)?.config.viewer;
    let probe = fm_core::view::probe::probe(Path::new(&path))?;
    let mut sessions = view_state.lock().map_err(view_lock)?;
    let view_id = sessions.open(&path, probe, ViewerMode::Text, &viewer_prefs)?;
    let page = sessions.page(&view_id)?;
    edit_state.lock().map_err(view_lock)?.close(&id);
    Ok(page)
}

#[tauri::command]
#[specta::specta]
pub fn edit_close(edit_state: State<'_, EditState>, id: String) -> Result<(), String> {
    edit_state.lock().map_err(view_lock)?.close(&id);
    Ok(())
}

// --- Embedded terminal (§5.7) ------------------------------------------------
//
// The core owns the prompt text, the history, the scrollback, the run-status
// machine, and the built-ins; these handlers are the usual marshalling plus the
// one side-effect the core cannot perform — spawning a process.

/// Push whatever buffer deltas the core has queued — the echoed prompt line, an
/// exit footer, a `clear`. Commands return an [`AppSnapshot`] for the prompt row,
/// but scrollback travels as events, so this is the other half of every terminal
/// handler that can touch the buffer.
fn emit_terminal_chunks(app: &AppHandle, state: &State<'_, SharedState>) -> Result<(), String> {
    let chunks = {
        let mut s = state.lock().map_err(lock_err)?;
        s.terminal.drain_chunks()
    };
    for chunk in chunks {
        let _ = crate::events::TerminalChunkEvent(chunk).emit(app);
    }
    Ok(())
}

/// Push a freshly rendered terminal state. Used when the backend changes it
/// outside a command's return value.
fn emit_terminal_state(app: &AppHandle, state: &State<'_, SharedState>) -> Result<(), String> {
    let term = {
        let s = state.lock().map_err(lock_err)?;
        s.terminal.state(&s.terminal_cwd())
    };
    let _ = crate::events::TerminalStateEvent(term).emit(app);
    Ok(())
}

/// Record a command that could not even start (bad path, permission denied) in
/// the buffer and turn the indicator red — a spawn failure is a normal, visible
/// outcome, not an exception (§5.6).
fn record_spawn_failure(
    app: &AppHandle,
    state: &State<'_, SharedState>,
    reason: &str,
) -> Result<(), String> {
    {
        let mut s = state.lock().map_err(lock_err)?;
        s.terminal.fail(reason);
    }
    emit_terminal_chunks(app, state)?;
    emit_terminal_state(app, state)
}

/// Cmd+T: move the keyboard to the prompt, or hand it back to the panel that is
/// still marked active (§5.7).
#[tauri::command]
#[specta::specta]
pub fn terminal_toggle_focus(state: State<'_, SharedState>) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    s.terminal.toggle_focus();
    Ok(s.snapshot_after_input())
}

/// Cmd+Shift+T: expand the pane to the bottom half of the window, or collapse it
/// back to the bare command line (§5.7).
#[tauri::command]
#[specta::specta]
pub fn terminal_toggle_half(state: State<'_, SharedState>) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    s.terminal.toggle_half();
    persist(&mut s);
    Ok(s.snapshot_after_input())
}

/// Esc: draw the panels aside to reveal the full terminal, and back (§6).
#[tauri::command]
#[specta::specta]
pub fn terminal_toggle_curtain(state: State<'_, SharedState>) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    s.terminal.toggle_curtain();
    Ok(s.snapshot_after_input())
}

/// Mirror the text being typed at the prompt. Returns nothing: this is an echo
/// of what the frontend already shows, and re-rendering from it would fight the
/// caret (see `TerminalState::input_rev`).
#[tauri::command]
#[specta::specta]
pub fn terminal_set_input(state: State<'_, SharedState>, text: String) -> Result<(), String> {
    let mut s = state.lock().map_err(lock_err)?;
    s.terminal.touch();
    s.terminal.set_input(text);
    Ok(())
}

/// Enter at the prompt. The core decides what the line means; a `cd` becomes a
/// panel navigation, `clear` empties the buffer, and anything else is spawned.
#[tauri::command]
#[specta::specta]
pub async fn terminal_run(
    app: AppHandle,
    state: State<'_, SharedState>,
    runtime: State<'_, TerminalRuntime>,
    watch: State<'_, WatchRuntime>,
) -> Result<AppSnapshot, String> {
    let (plan, shell, history) = {
        let mut s = state.lock().map_err(lock_err)?;
        s.terminal.touch();
        let cwd = s.terminal_cwd();
        let plan = s.terminal.prepare(&cwd);
        let shell = s.config.terminal.shell.clone();
        let history = s.terminal.history().clone();
        (plan, shell, history)
    };

    // Persist the command list off-thread, fire-and-forget, exactly as
    // preferences are written — a failed history write must never break the
    // command that triggered it.
    if !matches!(plan, fm_core::terminal::RunPlan::Nothing) {
        tauri::async_runtime::spawn_blocking(move || {
            fm_core::terminal::history::save(&history)
        });
    }
    // `prepare` echoed the prompt line (and, for `clear`, emptied the buffer);
    // push that before doing anything slow, so the pane reflects the command the
    // instant Enter is pressed.
    emit_terminal_chunks(&app, &state)?;

    match plan {
        fm_core::terminal::RunPlan::Nothing | fm_core::terminal::RunPlan::Cleared => {}
        fm_core::terminal::RunPlan::ChangeDir(path) => {
            // `cd` moves the active panel, which is what keeps the prompt in the
            // folder the user is looking at (§5.7).
            let panel = state.lock().map_err(lock_err)?.active;
            return navigate(state, watch, panel, NavTarget::Path(path)).await;
        }
        fm_core::terminal::RunPlan::Spawn { command, cwd } => {
            if let Err(reason) = terminal_runtime::spawn(app.clone(), &runtime, shell, command, cwd)
            {
                record_spawn_failure(&app, &state, &reason)?;
            }
        }
    }

    let mut s = state.lock().map_err(lock_err)?;
    Ok(s.snapshot_after_input())
}

/// Ctrl+C: interrupt the running command, or — with nothing running — clear the
/// prompt (§5.7).
#[tauri::command]
#[specta::specta]
pub fn terminal_interrupt_or_clear(
    state: State<'_, SharedState>,
    runtime: State<'_, TerminalRuntime>,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    if s.terminal.is_running() {
        // Drop the lock first: the reaping thread needs it to record the exit.
        drop(s);
        terminal_runtime::interrupt(&runtime);
        let mut s = state.lock().map_err(lock_err)?;
        return Ok(s.snapshot_after_input());
    }
    s.terminal.clear_input();
    Ok(s.snapshot_after_input())
}

/// Up / Down at the prompt: recall a previous command (§5.7).
#[tauri::command]
#[specta::specta]
pub fn terminal_history(
    state: State<'_, SharedState>,
    dir: HistoryDir,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    s.terminal.recall(dir);
    Ok(s.snapshot_after_input())
}

/// Ctrl+Enter: append the name under `panel`'s cursor to the command line,
/// shell-quoted, without the panel losing focus (§5.7).
#[tauri::command]
#[specta::specta]
pub fn terminal_insert_name(
    state: State<'_, SharedState>,
    panel: PanelId,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    let p = s.panel(panel);
    let name = p
        .entries
        .get(p.cursor_index)
        .map(|e| e.name.clone())
        .filter(|n| n != "..");
    if let Some(name) = name {
        s.terminal.insert_name(&name);
    }
    Ok(s.snapshot_after_input())
}

/// Re-cap the scrollback from the control in the corner of the expanded pane,
/// and persist the choice (§5.7 / §7).
#[tauri::command]
#[specta::specta]
pub fn terminal_set_scrollback(
    state: State<'_, SharedState>,
    bytes: u64,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    s.terminal.set_scrollback_limit(bytes);
    persist(&mut s);
    Ok(s.snapshot_after_input())
}

/// Empty the scrollback (Ctrl+L, or the button in the corner).
#[tauri::command]
#[specta::specta]
pub fn terminal_clear_buffer(
    app: AppHandle,
    state: State<'_, SharedState>,
) -> Result<AppSnapshot, String> {
    let snapshot = {
        let mut s = state.lock().map_err(lock_err)?;
        s.terminal.clear_buffer();
        s.snapshot_after_input()
    };
    emit_terminal_chunks(&app, &state)?;
    Ok(snapshot)
}

/// The whole scrollback, for the frontend's initial sync and any re-sync (on
/// expand, or after the cap changes).
#[tauri::command]
#[specta::specta]
pub fn terminal_buffer(state: State<'_, SharedState>) -> Result<TerminalBuffer, String> {
    let s = state.lock().map_err(lock_err)?;
    Ok(s.terminal.buffer())
}

/// Launch `path` in an external application. `app == None` → the system default
/// ("Open"); `Some(app)` names a specific application. macOS-only for now; Phase 4
/// adds Windows/Linux arms. The app never handles the file itself — it hands off
/// to the OS launcher (§5.5).
#[cfg(target_os = "macos")]
fn launch(path: &str, app: Option<&str>) -> Result<(), String> {
    use std::process::Command;
    let mut cmd = Command::new("/usr/bin/open");
    if let Some(app) = app {
        cmd.arg("-a").arg(app);
    }
    cmd.arg(path);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to launch `open`: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        match app {
            Some(app) => Err(format!("could not open with \"{app}\"")),
            None => Err(format!("could not open {path}")),
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn launch(_path: &str, _app: Option<&str>) -> Result<(), String> {
    Err("opening files is not supported on this platform yet".to_string())
}
