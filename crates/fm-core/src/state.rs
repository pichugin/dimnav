//! Application navigation state — the two panels and which one is active.
//!
//! This is core navigation state (SPEC §3/§10), so it lives here in the
//! platform-agnostic core, not in the Tauri adapter. The adapter merely wraps it
//! in a `Mutex` and exposes it as Tauri-managed state.

use crate::types::{AppSnapshot, Config, PanelId, PanelPrefs, PanelState};

/// Holds both panels, the active-panel selector, and the loaded configuration.
///
/// The config is part of application state rather than re-read per call: it is
/// the single source of truth for the persisted preferences (§5.8 / §7), and
/// [`sync_prefs_from_panels`](AppState::sync_prefs_from_panels) is the one place
/// live panel state flows back into it before a save.
#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub left: PanelState,
    pub right: PanelState,
    pub active: PanelId,
    pub config: Config,
}

impl AppState {
    /// Apply loaded preferences to both panels (view/sort/hidden). The starting
    /// directories are resolved by the caller, which owns the "does it still
    /// exist?" filesystem question.
    pub fn apply_config(&mut self, config: Config) {
        apply_prefs(&mut self.left, &config.left_panel);
        apply_prefs(&mut self.right, &config.right_panel);
        self.config = config;
    }

    /// Copy the live per-panel view state back into the config, so a following
    /// `config::save` persists exactly what the user sees (§5.8).
    pub fn sync_prefs_from_panels(&mut self) {
        capture_prefs(&self.left, &mut self.config.left_panel);
        capture_prefs(&self.right, &mut self.config.right_panel);
    }

    /// The persisted "Move to Trash" default for the delete dialog — OFF by
    /// default (§5.4a). Lives in the config so it survives restarts.
    pub fn trash_default(&self) -> bool {
        self.config.trash_default
    }
    /// Immutable access to a panel by id.
    pub fn panel(&self, id: PanelId) -> &PanelState {
        match id {
            PanelId::Left => &self.left,
            PanelId::Right => &self.right,
        }
    }

    /// Mutable access to a panel by id.
    pub fn panel_mut(&mut self, id: PanelId) -> &mut PanelState {
        match id {
            PanelId::Left => &mut self.left,
            PanelId::Right => &mut self.right,
        }
    }

    /// Set the active panel (§5.1).
    pub fn set_active(&mut self, id: PanelId) {
        self.active = id;
    }

    /// A serializable snapshot of the whole navigation state for the frontend.
    pub fn snapshot(&self) -> AppSnapshot {
        AppSnapshot {
            left: self.left.clone(),
            right: self.right.clone(),
            active: self.active,
            trash_default: self.config.trash_default,
        }
    }
}

fn apply_prefs(panel: &mut PanelState, prefs: &PanelPrefs) {
    panel.view_mode = prefs.view_mode;
    panel.sort_mode = prefs.sort_mode;
    panel.show_hidden = prefs.show_hidden;
}

fn capture_prefs(panel: &PanelState, prefs: &mut PanelPrefs) {
    prefs.view_mode = panel.view_mode;
    prefs.sort_mode = panel.sort_mode;
    prefs.show_hidden = panel.show_hidden;
    // Remember where the panel was left, so the next launch reopens it (§7).
    if !panel.path.is_empty() {
        prefs.start_dir = Some(panel.path.clone());
    }
}
