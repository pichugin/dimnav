//! Tauri command handlers — the thin request/response half of the IPC adapter.
//!
//! Each handler is marshalling only: it locks the shared [`AppState`], calls into
//! `fm-core`, and returns a serializable result. No business logic lives here
//! (SPEC §3). Navigation commands return a full [`AppSnapshot`] so the frontend
//! can replace its whole render state in one step.
//!
//! Filesystem reads run on a blocking thread pool and never hold the state lock
//! across an `.await`, so the UI thread is never blocked (SPEC §5.4a).

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use fm_core::state::AppState;
use fm_core::types::{
    AppSnapshot, Config, DirListing, EntryKind, KeyBinding, Motion, NavTarget, PanelId,
};
use tauri::State;

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
