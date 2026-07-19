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

use fm_core::state::AppState;
use fm_core::types::{
    AppSnapshot, Config, DeleteRequest, DirListing, EntryKind, ErrorResolution, KeyBinding, Motion,
    NavTarget, OpKind, OpRequest, PanelId, Resolution,
};
use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::events::OpCompleteEvent;
use crate::ops_runtime::{registry, OpRegistry, TauriObserver, UserInput};

/// Tauri-managed shared navigation state.
pub type SharedState = Mutex<AppState>;

fn lock_err<T>(_: PoisonError<T>) -> String {
    "navigation state lock was poisoned".to_string()
}

/// Default starting directory for a fresh session.
fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}

// --- Liveness / config ------------------------------------------------------

/// Liveness check used to verify the IPC pipeline round-trips.
#[tauri::command]
#[specta::specta]
pub fn ping() -> String {
    "pong".to_string()
}

/// Return the current configuration (Phase 1: defaults).
#[tauri::command]
#[specta::specta]
pub fn get_config() -> Config {
    fm_core::config::load()
}

/// The active keymap (action id → key chords), sourced from core config so the
/// webview never hardcodes keys (SPEC §6).
#[tauri::command]
#[specta::specta]
pub fn get_keymap() -> Vec<KeyBinding> {
    fm_core::config::default_keymap()
}

/// List a directory into a structured [`DirListing`] (utility; panels use the
/// stateful commands below).
#[tauri::command]
#[specta::specta]
pub fn list_dir(path: String, show_hidden: bool) -> DirListing {
    fm_core::fs::list_dir(&path, show_hidden)
}

// --- Stateful navigation ----------------------------------------------------

/// Populate both panels with their starting directory (home). Call once on boot.
#[tauri::command]
#[specta::specta]
pub async fn init(state: State<'_, SharedState>) -> Result<AppSnapshot, String> {
    let home = home_dir();
    let (show_left, show_right) = {
        let s = state.lock().map_err(lock_err)?;
        (s.left.show_hidden, s.right.show_hidden)
    };

    let (hl, hr) = (home.clone(), home);
    let left = tauri::async_runtime::spawn_blocking(move || fm_core::fs::list_dir(&hl, show_left))
        .await
        .map_err(|e| e.to_string())?;
    let right =
        tauri::async_runtime::spawn_blocking(move || fm_core::fs::list_dir(&hr, show_right))
            .await
            .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::set_listing(&mut s.left, left);
    fm_core::nav::set_listing(&mut s.right, right);
    Ok(s.snapshot())
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
    let p = s.panel_mut(panel);
    p.geometry.columns = columns;
    p.geometry.rows_per_column = rows;
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
    Ok(s.snapshot())
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
    Ok(s.snapshot())
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
    Ok(s.snapshot())
}

/// Additively select the entry under the cursor, then move (Shift+Arrow).
#[tauri::command]
#[specta::specta]
pub fn select_and_move(
    state: State<'_, SharedState>,
    panel: PanelId,
    motion: Motion,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::select_and_move(s.panel_mut(panel), motion);
    Ok(s.snapshot())
}

/// Select all selectable entries in the panel (`*`).
#[tauri::command]
#[specta::specta]
pub fn select_all(state: State<'_, SharedState>, panel: PanelId) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::select_all(s.panel_mut(panel));
    Ok(s.snapshot())
}

/// Clear the panel's selection (`-`).
#[tauri::command]
#[specta::specta]
pub fn deselect_all(state: State<'_, SharedState>, panel: PanelId) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    fm_core::nav::deselect_all(s.panel_mut(panel));
    Ok(s.snapshot())
}

/// Navigate a panel: into the entry under the cursor, up to the parent (with the
/// auto-position-on-exited-folder rule, §5.2), or to an explicit path.
#[tauri::command]
#[specta::specta]
pub async fn navigate(
    state: State<'_, SharedState>,
    panel: PanelId,
    target: NavTarget,
) -> Result<AppSnapshot, String> {
    // Capture what we need from state, then drop the lock before any I/O.
    let (cur_path, show_hidden, cursor_entry) = {
        let s = state.lock().map_err(lock_err)?;
        let p = s.panel(panel);
        let entry = p
            .entries
            .get(p.cursor_index)
            .map(|e| (e.name.clone(), e.kind));
        (p.path.clone(), p.show_hidden, entry)
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
        let s = state.lock().map_err(lock_err)?;
        return Ok(s.snapshot());
    };

    let listing =
        tauri::async_runtime::spawn_blocking(move || fm_core::fs::list_dir(&path, show_hidden))
            .await
            .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    let p = s.panel_mut(panel);
    fm_core::nav::set_listing(p, listing);
    if let Some(name) = exited_child {
        fm_core::nav::position_on(p, &name);
    }
    Ok(s.snapshot())
}

/// Re-read a panel's current directory, keeping the cursor on the same entry name
/// when it still exists (graceful refresh, SPEC §5.6).
#[tauri::command]
#[specta::specta]
pub async fn refresh(state: State<'_, SharedState>, panel: PanelId) -> Result<AppSnapshot, String> {
    let (path, show_hidden, focused) = {
        let s = state.lock().map_err(lock_err)?;
        let p = s.panel(panel);
        let focused = p.entries.get(p.cursor_index).map(|e| e.name.clone());
        (p.path.clone(), p.show_hidden, focused)
    };

    let listing =
        tauri::async_runtime::spawn_blocking(move || fm_core::fs::list_dir(&path, show_hidden))
            .await
            .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    let p = s.panel_mut(panel);
    fm_core::nav::set_listing(p, listing);
    if let Some(name) = focused {
        fm_core::nav::position_on(p, &name);
    }
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
    let (path, show_hidden) = {
        let s = state.lock().map_err(lock_err)?;
        let p = s.panel(panel);
        (p.path.clone(), p.show_hidden)
    };

    fm_core::fs::make_dir(Path::new(&path), &name)?;

    // The new entry to focus is the first path component of `name`.
    let focus = Path::new(&name)
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .map(str::to_string);

    let listing =
        tauri::async_runtime::spawn_blocking(move || fm_core::fs::list_dir(&path, show_hidden))
            .await
            .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    let p = s.panel_mut(panel);
    fm_core::nav::set_listing(p, listing);
    if let Some(name) = focus {
        fm_core::nav::position_on(p, &name);
    }
    Ok(s.snapshot())
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
    let (path, show_hidden, old) = {
        let s = state.lock().map_err(lock_err)?;
        let p = s.panel(panel);
        let old = p
            .entries
            .get(p.cursor_index)
            .map(|e| e.name.clone())
            .ok_or_else(|| "nothing to rename".to_string())?;
        (p.path.clone(), p.show_hidden, old)
    };

    fm_core::fs::rename_entry(Path::new(&path), &old, &new_name)?;
    let focus = new_name.trim().to_string();

    let listing =
        tauri::async_runtime::spawn_blocking(move || fm_core::fs::list_dir(&path, show_hidden))
            .await
            .map_err(|e| e.to_string())?;

    let mut s = state.lock().map_err(lock_err)?;
    let p = s.panel_mut(panel);
    fm_core::nav::set_listing(p, listing);
    fm_core::nav::position_on(p, &focus);
    Ok(s.snapshot())
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

        let sources: Vec<String> = indices
            .into_iter()
            .filter_map(|i| p.entries.get(i))
            .filter(|e| e.name != "..")
            .map(|e| base.join(&e.name).to_string_lossy().into_owned())
            .collect();

        let dest_path = fm_core::ops::resolve_dest(&base, &dest);
        OpRequest {
            kind,
            sources,
            dest: dest_path.to_string_lossy().into_owned(),
        }
    };

    if req.sources.is_empty() {
        return Err("nothing to transfer".to_string());
    }

    // A copy/move destination is always a folder to place sources into. Renaming in
    // place is a separate, explicit action (Shift+F6), so a single-source transfer
    // to a path that does not exist must NOT silently rename — surface an error
    // instead (per product decision; the engine still supports rename for internal
    // reuse). Multiple sources to a missing path stay allowed: that means "create a
    // new folder for them".
    if req.sources.len() == 1 && !Path::new(&req.dest).exists() {
        return Err(format!("destination folder does not exist: {}", req.dest));
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
/// default (§5.4a). In-memory this slice — persistence lands with the config slice.
#[tauri::command]
#[specta::specta]
pub fn set_trash_default(
    state: State<'_, SharedState>,
    value: bool,
) -> Result<AppSnapshot, String> {
    let mut s = state.lock().map_err(lock_err)?;
    s.trash_default = value;
    Ok(s.snapshot())
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

        let paths: Vec<String> = indices
            .into_iter()
            .filter_map(|i| p.entries.get(i))
            .filter(|e| e.name != "..")
            .map(|e| base.join(&e.name).to_string_lossy().into_owned())
            .collect();

        DeleteRequest {
            paths,
            to_trash: s.trash_default,
        }
    };

    if req.paths.is_empty() {
        return Err("nothing to delete".to_string());
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
