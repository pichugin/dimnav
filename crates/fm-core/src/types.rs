//! The typed IPC contract surface — every DTO that crosses the Rust ↔ frontend
//! boundary lives here, in one place.
//!
//! Each type is `serde`-serializable and `specta`-exportable, so the TypeScript
//! types the frontend consumes are **generated** from these definitions rather
//! than hand-maintained (see `src-tauri` binding generation). Paths cross the
//! boundary as `String` — simplest to render and unambiguous in TS.
//!
//! These are the Phase-1 *shapes*; field-level behaviour lands with feature work.

use serde::{Deserialize, Serialize};
use specta::Type;

// ---------------------------------------------------------------------------
// Panels & entries
// ---------------------------------------------------------------------------

/// Which panel an intent targets. The two-panel model is the app's core (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum PanelId {
    #[default]
    Left,
    Right,
}

/// What kind of filesystem object an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Special,
}

/// Readability marker, so a single unreadable entry renders as a marker rather
/// than failing the whole listing (§5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum EntryMarker {
    Ok,
    Denied,
    Broken,
}

/// One row in a directory listing.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    /// Modification time, Unix seconds.
    pub modified: i64,
    /// POSIX permission bits (e.g. `0o755`).
    pub permissions: u32,
    /// Target path when `kind == Symlink`; `None` otherwise.
    pub symlink_target: Option<String>,
    pub is_executable: bool,
    pub marker: EntryMarker,
}

/// Per-panel view mode. `Columns(n)` covers the brief multi-column modes
/// (2 by default); `Detailed` is the single-column-with-metadata mode (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", content = "columns", rename_all = "lowercase")]
pub enum ViewMode {
    Columns(u8),
    Detailed,
}

impl Default for ViewMode {
    fn default() -> Self {
        // 2-column brief is the default (§5.2).
        ViewMode::Columns(2)
    }
}

/// Sort order. Folders-first-by-name is the default (§5.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SortMode {
    #[default]
    NameFoldersFirst,
    TypeName,
    Size,
    Date,
}

/// Layout geometry reported by the frontend on resize / view-mode change. The
/// cursor state machine needs it to compute the column/page traversal (§5.2) —
/// the frontend owns pixels, the core owns the resulting cursor index.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Type)]
pub struct PanelGeometry {
    pub columns: u16,
    pub rows_per_column: u16,
}

/// The full state of one panel — the unit the frontend renders.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PanelState {
    pub path: String,
    pub entries: Vec<Entry>,
    /// Index into `entries` of the entry under the cursor.
    pub cursor_index: usize,
    /// Index of the first visible entry — the top of the leftmost column. The
    /// panel scrolls as a sliding window, one entry at a time, rather than
    /// flipping whole pages (§5.2). Transient: never persisted.
    pub top_index: usize,
    /// Indices of selected entries (§5.3). Persists across cursor movement.
    pub selection: Vec<usize>,
    pub view_mode: ViewMode,
    pub sort_mode: SortMode,
    pub show_hidden: bool,
    pub geometry: PanelGeometry,
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            path: String::new(),
            entries: Vec::new(),
            cursor_index: 0,
            top_index: 0,
            selection: Vec::new(),
            view_mode: ViewMode::default(),
            sort_mode: SortMode::default(),
            // Hidden files are shown by default (§5.8).
            show_hidden: true,
            geometry: PanelGeometry::default(),
        }
    }
}

/// Result of reading a directory. Unreadable entries appear in `entries` carrying
/// a non-`Ok` [`EntryMarker`] rather than aborting the listing (§5.6).
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct DirListing {
    pub path: String,
    pub entries: Vec<Entry>,
}

/// A full snapshot of navigation state — both panels plus which is active. Every
/// navigation command returns this so the frontend replaces its whole render
/// state in one step (panels are small; cloning is cheap).
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct AppSnapshot {
    pub left: PanelState,
    pub right: PanelState,
    pub active: PanelId,
    /// Global "Move to Trash" default for the delete dialog, OFF by default
    /// (§5.4a). The frontend renders the checkbox from this.
    pub trash_default: bool,
}

// ---------------------------------------------------------------------------
// Navigation & selection intents
// ---------------------------------------------------------------------------

/// Cursor motions — the intents the frontend forwards; the core turns them into
/// a new cursor index via the single traversal state machine (§5.2).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
}

/// Target for `navigate`.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum NavTarget {
    /// Enter the directory currently under the cursor.
    Into,
    /// Go to the parent, auto-positioning on the folder just exited (§5.2).
    Parent,
    /// Jump to an explicit path (absolute or relative).
    Path(String),
}

// ---------------------------------------------------------------------------
// File operations
// ---------------------------------------------------------------------------

/// Copy vs move — the two panel-to-panel transfer operations (§5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum OpKind {
    Copy,
    Move,
}

/// A copy/move request. `dest` is the editable destination path from the F5/F6
/// prompt — it accepts `..`, relative, and absolute paths (§5.4a).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OpRequest {
    pub kind: OpKind,
    pub sources: Vec<String>,
    pub dest: String,
}

/// A delete request. `to_trash` mirrors the persisted "Move to Trash" checkbox,
/// which is OFF by default (§5.4a).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DeleteRequest {
    pub paths: Vec<String>,
    pub to_trash: bool,
}

/// Name-collision resolution. FAR shapes: a single-file dialog omits the `*_all`
/// variants; a multi-file dialog offers all of them (§5.4a).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Skip,
    SkipAll,
    Overwrite,
    OverwriteAll,
    Cancel,
}

/// Error-dialog resolution. Adds `Retry` and `Elevate`; elevation routes through
/// the OS-native auth prompt on the Tauri side — the app never handles passwords
/// (§5.6).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ErrorResolution {
    Retry,
    Skip,
    SkipAll,
    Cancel,
    Elevate,
}

/// How to open a file with an external tool: system default, or the configured
/// viewer (F3) / editor (F4) for its type (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum OpenAction {
    Open,
    View,
    Edit,
}

/// Terminal status of an operation (§5.4a / §5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum OpStatus {
    #[default]
    Pending,
    Success,
    Partial,
    Failed,
    Cancelled,
}

/// Structured result of an operation — the frontend renders this, never an
/// opaque thrown error (§5.6).
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct OpOutcome {
    pub op_id: String,
    pub status: OpStatus,
    pub summary: String,
}

/// Progress event payload for a running operation (§5.4a).
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct OpProgress {
    pub op_id: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub count_done: u64,
    pub count_total: u64,
    /// The file currently being processed.
    pub current: String,
}

/// Basic file info shown on demand (§5.4).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
    pub modified: i64,
    pub permissions: u32,
    pub kind: EntryKind,
}

// ---------------------------------------------------------------------------
// Event payloads (core → frontend)
// ---------------------------------------------------------------------------

/// Payload for the collision-prompt event. `multiple` enables the `*_all`
/// resolution choices in the dialog (§5.4a).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CollisionPrompt {
    pub op_id: String,
    pub path: String,
    pub multiple: bool,
}

/// Payload for the failure event — the frontend renders this as a red-background
/// dialog. `offer_elevate` gates the OS-native elevation choice (§5.4b / §5.6).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OpErrorInfo {
    pub op_id: String,
    pub path: String,
    pub reason: String,
    pub offer_elevate: bool,
}

/// Payload for a panel-state change pushed by the core, e.g. the directory
/// changed underneath us and was refreshed (§5.6).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PanelChanged {
    pub panel: PanelId,
    pub state: PanelState,
}

/// One line of output from a running executable (§5.5 / §5.7). Phase 1 renders
/// these into a simple output modal; Phase 2 routes the same stream into the
/// embedded terminal (the `plugin::ExecutionSink` seam) without changing callers.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ExecOutput {
    pub line: String,
}

/// A run started by Enter-on-executable has finished (§5.5). `code` is the
/// process exit code, or `-1` when it was killed / had no code.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ExecDone {
    pub code: i32,
    pub summary: String,
}

// ---------------------------------------------------------------------------
// Config (persisted, TOML)
// ---------------------------------------------------------------------------

/// One keybinding: an action id (e.g. `"cursor.down"`) and the key chords bound
/// to it. Sourced from core config so the webview never hardcodes keys (§6/§7).
/// Remapping, persistence, and conflict detection are a later slice.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct KeyBinding {
    pub action: String,
    pub keys: Vec<String>,
}

/// Persisted per-panel preferences, restored on the next launch (§5.8 / §7).
///
/// Field order matters: TOML requires plain values before tables, and `view_mode`
/// serializes as a table (`{kind, columns}`), so it comes last. `serde(default)`
/// lets a hand-edited, partial config still load (§7 — zero-config must work).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct PanelPrefs {
    /// Directory the panel opens to; `None` means a sensible default (e.g. home).
    pub start_dir: Option<String>,
    pub sort_mode: SortMode,
    pub show_hidden: bool,
    pub view_mode: ViewMode,
}

impl Default for PanelPrefs {
    fn default() -> Self {
        Self {
            start_dir: None,
            sort_mode: SortMode::default(),
            show_hidden: true, // hidden files shown by default (§5.8)
            view_mode: ViewMode::default(),
        }
    }
}

/// One file-type → external-application mapping (§5.5 / §7). Associates a set of
/// extensions with the app to launch for each action; `None` for an action means
/// "fall back to the system default (`open`)". Matched case-insensitively on the
/// entry's extension, no leading dot (e.g. `"md"`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct FileAssociation {
    /// Lower-case extensions this mapping claims, e.g. `["md", "markdown"]`.
    pub extensions: Vec<String>,
    /// App for the default Open action (Enter / double-click).
    pub open: Option<String>,
    /// App for View (F3, read-only); falls back to `open` then system default.
    pub view: Option<String>,
    /// App for Edit (F4, read-write); falls back to `open` then system default.
    pub edit: Option<String>,
}

/// Root config document (serialized to TOML). Ships with working defaults — the
/// app is fully usable with zero configuration (§7).
///
/// Field order matters: TOML rejects a plain value emitted after a table, so the
/// scalars come first and the panel tables / association array-of-tables last.
/// `serde(default)` means a partial or hand-edited file still loads.
///
/// Keybindings are deliberately omitted so we don't freeze an unreviewed schema;
/// they join here with the remapping slice.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct Config {
    /// Global "Move to Trash" default — OFF by default, persisted (§5.4a).
    pub trash_default: bool,
    /// Id of the active theme.
    pub theme: String,
    pub left_panel: PanelPrefs,
    pub right_panel: PanelPrefs,
    /// File-type → external-application map (§5.5). Empty by default, so every
    /// file opens with the system default until the user edits the TOML.
    pub associations: Vec<FileAssociation>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            trash_default: false,
            theme: "classic".to_string(),
            left_panel: PanelPrefs::default(),
            right_panel: PanelPrefs::default(),
            associations: Vec::new(),
        }
    }
}
