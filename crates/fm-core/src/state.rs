//! Application navigation state — the two panels and which one is active.
//!
//! This is core navigation state (SPEC §3/§10), so it lives here in the
//! platform-agnostic core, not in the Tauri adapter. The adapter merely wraps it
//! in a `Mutex` and exposes it as Tauri-managed state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::terminal::Terminal;
use crate::types::{AppSnapshot, Config, PanelId, PanelPrefs, PanelState};

/// A cached recursive folder size, keyed by absolute path in
/// [`AppState::size_cache`]. `dir_mtime` is the top directory's mtime at
/// calculation time, so a later refresh can detect direct-child changes.
#[derive(Debug, Clone, Copy)]
pub struct CachedSize {
    pub size: u64,
    pub dir_mtime: i64,
}

/// Holds both panels, the active-panel selector, and the loaded configuration.
///
/// The config is part of application state rather than re-read per call: it is
/// the single source of truth for the persisted preferences (§5.8 / §7), and
/// [`sync_prefs_from_panels`](AppState::sync_prefs_from_panels) is the one place
/// live panel state flows back into it before a save.
#[derive(Debug, Default)]
pub struct AppState {
    pub left: PanelState,
    pub right: PanelState,
    pub active: PanelId,
    pub config: Config,
    /// The command line (§5.7). It lives beside the panels rather than in its own
    /// lock because it reads the active panel's directory on every run and every
    /// snapshot — one lock keeps that consistent.
    pub terminal: Terminal,
    /// Recursively computed folder sizes keyed by absolute path (F3). Surfaced
    /// into each dir entry's `computed_size` by [`snapshot`](AppState::snapshot).
    pub size_cache: HashMap<PathBuf, CachedSize>,
}

impl AppState {
    /// Apply loaded preferences to both panels (view/sort/hidden) and the
    /// terminal. The starting directories are resolved by the caller, which owns
    /// the "does it still exist?" filesystem question.
    pub fn apply_config(&mut self, config: Config) {
        apply_prefs(&mut self.left, &config.left_panel);
        apply_prefs(&mut self.right, &config.right_panel);
        self.terminal.apply_prefs(&config.terminal);
        self.config = config;
    }

    /// Copy the live per-panel view state back into the config, so a following
    /// `config::save` persists exactly what the user sees (§5.8).
    pub fn sync_prefs_from_panels(&mut self) {
        capture_prefs(&self.left, &mut self.config.left_panel);
        capture_prefs(&self.right, &mut self.config.right_panel);
        self.terminal.capture_prefs(&mut self.config.terminal);
    }

    /// The directory the terminal runs in — the active panel's, which is what
    /// makes the prompt "already in the folder you were browsing" (§5.7).
    pub fn terminal_cwd(&self) -> String {
        self.panel(self.active).path.clone()
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
    ///
    /// Pure/no-IO: each panel is cloned and its directory entries have
    /// `computed_size` filled from [`size_cache`](AppState::size_cache) by simple
    /// path lookups — never a filesystem walk.
    pub fn snapshot(&self) -> AppSnapshot {
        AppSnapshot {
            left: self.panel_snapshot(PanelId::Left),
            right: self.panel_snapshot(PanelId::Right),
            active: self.active,
            trash_default: self.config.trash_default,
            terminal: self.terminal.state(&self.terminal_cwd()),
        }
    }

    /// [`snapshot`](Self::snapshot) for a command the **user** initiated.
    ///
    /// Deliberately a separate entry point: the terminal's run indicator fades
    /// its green/red verdict to grey the moment the user touches any control
    /// (§5.7), so "was this a user action or a background refresh?" has to be
    /// answerable. `set_viewport` (fired by a resize observer) and `refresh`
    /// (fired when an operation completes) use the plain
    /// [`snapshot`](Self::snapshot) precisely because they are neither.
    ///
    /// It is also where an open quick-search box dies (§5.9). Every user-initiated
    /// command returns through here, so switching panel, reaching for the
    /// terminal, navigating, sorting and every file operation end the search
    /// without a single one of them having to remember to — which is the point,
    /// since the one that forgot would leave a box floating over a panel it no
    /// longer describes.
    pub fn snapshot_after_input(&mut self) -> AppSnapshot {
        self.left.search = None;
        self.right.search = None;
        self.snapshot_after_search_input()
    }

    /// [`snapshot_after_input`](Self::snapshot_after_input) minus the quick-search
    /// reset — for the quick-search commands themselves, the one kind of input
    /// that must not end the search it is driving (§5.9).
    pub fn snapshot_after_search_input(&mut self) -> AppSnapshot {
        self.terminal.touch();
        self.snapshot()
    }

    /// Clone a panel and surface any cached recursive folder sizes onto its dir
    /// entries. `..` and non-directories are left untouched.
    ///
    /// Public because the watcher pushes a *single* panel when a directory
    /// changes underneath it (§5.6); [`snapshot`](Self::snapshot) would clone
    /// both panels' entry vectors to deliver one panel's news.
    pub fn panel_snapshot(&self, id: PanelId) -> PanelState {
        let mut panel = self.panel(id).clone();
        if self.size_cache.is_empty() || panel.path.is_empty() {
            return panel;
        }
        let base = Path::new(&panel.path);
        for entry in &mut panel.entries {
            if entry.kind != crate::types::EntryKind::Dir || entry.name == ".." {
                continue;
            }
            if let Some(cached) = self.size_cache.get(&base.join(&entry.name)) {
                entry.computed_size = Some(cached.size);
            }
        }
        panel
    }

    /// Record a freshly computed recursive folder size (F3).
    pub fn set_size(&mut self, path: PathBuf, size: u64, dir_mtime: i64) {
        self.size_cache.insert(path, CachedSize { size, dir_mtime });
    }

    /// Drop cache entries affected by a change at `path`: the path itself, its
    /// ancestors (whose totals changed), and its descendants (if `path` moved or
    /// was deleted).
    pub fn invalidate_size_cache(&mut self, path: &Path) {
        self.size_cache
            .retain(|k, _| !(k.starts_with(path) || path.starts_with(k)));
    }

    /// Opportunistically drop cached sizes under `base` whose directory mtime has
    /// changed (a direct child added/removed/renamed) or that no longer exist.
    /// Called on refresh — catches external changes the app didn't make. Entries
    /// outside `base` are left untouched. Note: a change to a *deep* descendant
    /// that doesn't alter any ancestor's mtime is not detected here and requires
    /// an explicit F3 recompute.
    pub fn revalidate_sizes_under(&mut self, base: &Path) {
        self.size_cache.retain(|k, v| {
            if !k.starts_with(base) {
                return true;
            }
            crate::fs::dir_mtime(k) == Some(v.dir_mtime)
        });
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
    // Remember where the panel was left, so the next launch reopens it (§7) —
    // unless the panel cannot read it. Persisting a directory that renders as a
    // permission error would reopen the app in a dead end (§5.6).
    if !panel.path.is_empty() && panel.access.is_none() {
        prefs.start_dir = Some(panel.path.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DirAccessError, DirAccessKind};

    fn denied() -> DirAccessError {
        DirAccessError {
            kind: DirAccessKind::Denied,
            message: "nope".to_string(),
            remedies: Vec::new(),
        }
    }

    #[test]
    fn a_readable_directory_is_remembered_for_the_next_launch() {
        let panel = PanelState {
            path: "/tmp/somewhere".to_string(),
            ..PanelState::default()
        };
        let mut prefs = PanelPrefs::default();
        capture_prefs(&panel, &mut prefs);
        assert_eq!(prefs.start_dir.as_deref(), Some("/tmp/somewhere"));
    }

    #[test]
    fn a_directory_the_panel_cannot_read_is_not_remembered() {
        // Otherwise the next launch reopens in the dead end (§5.6 / §7). The
        // previously remembered directory is left in place rather than cleared:
        // it is still the last place the panel could actually show.
        let panel = PanelState {
            path: "/tmp/locked".to_string(),
            access: Some(denied()),
            ..PanelState::default()
        };
        let mut prefs = PanelPrefs {
            start_dir: Some("/tmp/readable".to_string()),
            ..PanelPrefs::default()
        };
        capture_prefs(&panel, &mut prefs);
        assert_eq!(prefs.start_dir.as_deref(), Some("/tmp/readable"));
    }
}
