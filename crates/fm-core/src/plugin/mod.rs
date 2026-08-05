//! Extension points (SPEC §6a).
//!
//! The app is modular from day one: even in-tree features (terminal, viewer/
//! editor, archive support) are meant to be written against these same traits, so
//! the extension system is dogfooded rather than bolted on later. Phase 1 defines
//! the traits only — there is **no plugin loader and no WASM host yet** (those
//! arrive in Phase 5 as a versioned, capability-gated public API).
//!
//! Keeping these as a stable, documented internal contract now is what lets
//! third-party plugins later avoid breaking on every release.

use std::path::Path;

use crate::types::{Entry, PanelId};

/// A bindable command that can appear in a command palette (§6a).
pub trait Command {
    /// Stable identifier, e.g. `"panel.toggle_hidden"`.
    fn id(&self) -> &str;
    /// Human-readable title for the command palette.
    fn title(&self) -> &str;
    // fn run(&self, ctx: &mut CommandCtx) -> Result<(), OpError>;  // (later)
}

/// A topic in the F1 help book — the "contribute a UI surface" extension point
/// (§6a), in the one form that costs the frontend nothing: a topic hands back
/// structured content, and the renderer decides how to paint it. That keeps the
/// swappable-frontend rule intact, since a plugin can never ship markup.
///
/// The in-tree About and Shortcuts topics in [`crate::help`] are written against
/// this trait rather than being special-cased, so the loader in Phase 5 has
/// nothing new to teach them.
pub trait HelpTopic {
    /// Stable identifier, e.g. `"shortcuts"`.
    fn id(&self) -> &str;
    /// Label for the topic rail.
    fn title(&self) -> &str;
    /// Render this topic against the current app state and search query.
    fn body(&self, ctx: &crate::help::HelpCtx<'_>) -> crate::types::HelpBody;
}

/// Custom view / edit / open / preview behaviour for a file type — e.g. an image
/// previewer, a hex viewer, or an archive-as-virtual-directory provider (§6a).
pub trait FileTypeHandler {
    /// Whether this handler claims the given entry.
    fn handles(&self, entry: &Entry) -> bool;
}

/// Contributes an extra panel column / metadata cell, e.g. git status or a
/// checksum (§6a).
pub trait ColumnProvider {
    fn id(&self) -> &str;
    /// Column header label.
    fn header(&self) -> &str;
    /// Cell text for a given entry.
    fn cell(&self, entry: &Entry) -> String;
}

/// A custom operation registered against the file-operation pipeline, e.g.
/// "compress selection" (§6a). Integrates with [`crate::ops`].
pub trait Operation {
    fn id(&self) -> &str;
}

/// Contributes a theme (colors, fonts, transparency) (§6a / §7).
pub trait ThemeProvider {
    fn id(&self) -> &str;
}

/// Watches the directories the panels have open and reports when one changes
/// underneath the app (§5.6 / §6a).
///
/// The live-refresh feature is written against this rather than calling a
/// watcher crate directly, so the change *source* is replaceable: an
/// archive-as-virtual-directory provider or a remote filesystem can implement it
/// without the panels learning anything new. It is also the seam that keeps the
/// platform-specific half (FSEvents / inotify / ReadDirectoryChangesW) out of the
/// core, which owns only the decision of what a change *means*
/// ([`crate::fs::watch`]).
pub trait FsObserver {
    /// Begin observing `path` on behalf of `panel`, replacing whatever that panel
    /// was previously pointed at. Called on every directory change, so it must be
    /// cheap and must not fail loudly — watching is a convenience, and a panel
    /// that cannot be watched simply falls back to manual refresh.
    fn observe(&self, panel: PanelId, path: &Path);

    /// Stop observing on `panel`'s behalf.
    fn release(&self, panel: PanelId);

    /// Check every observed directory right now, without waiting for the next
    /// event or poll. Backs the window-focus refresh, which is what covers the
    /// cases a watcher structurally cannot see.
    fn poke(&self);
}

/// **Phase-2 seam.** Where an executable's stdout/stderr is routed when the user
/// runs it (§5.5 / §5.7). Phase 1 wires this to a simple modal output sink;
/// Phase 2 swaps in the PTY-backed terminal without changing callers, so
/// Enter-to-execute and the Esc terminal curtain slot in cleanly.
pub trait ExecutionSink {
    /// Append output bytes from the running process.
    fn write(&mut self, bytes: &[u8]);
}
