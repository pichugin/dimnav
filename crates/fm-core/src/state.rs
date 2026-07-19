//! Application navigation state — the two panels and which one is active.
//!
//! This is core navigation state (SPEC §3/§10), so it lives here in the
//! platform-agnostic core, not in the Tauri adapter. The adapter merely wraps it
//! in a `Mutex` and exposes it as Tauri-managed state.

use crate::types::{AppSnapshot, PanelId, PanelState};

/// Holds both panels and the active-panel selector.
#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub left: PanelState,
    pub right: PanelState,
    pub active: PanelId,
    /// Global "Move to Trash" default for the delete dialog — OFF by default
    /// (§5.4a). In-memory this slice; TOML persistence lands with the config slice.
    pub trash_default: bool,
}

impl AppState {
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
            trash_default: self.trash_default,
        }
    }
}
