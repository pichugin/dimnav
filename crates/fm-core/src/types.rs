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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum PanelId {
    #[default]
    Left,
    Right,
}

impl PanelId {
    /// The other side. With exactly two panels this is a total function, not a
    /// lookup that can fail.
    pub fn other(self) -> Self {
        match self {
            PanelId::Left => PanelId::Right,
            PanelId::Right => PanelId::Left,
        }
    }
}

/// What kind of filesystem object an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Special,
}

/// Readability marker, so a single unreadable entry renders as a marker rather
/// than failing the whole listing (§5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum EntryMarker {
    Ok,
    Denied,
    Broken,
}

/// What a listing row *is*, for colouring and for the execute-vs-launch decision
/// (§4). Derived from the name, kind and permission bits by
/// [`crate::filetype::classify`] — never set by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum EntryCategory {
    Dir,
    Symlink,
    /// A dotfile or dotfolder. Outranks `Dir`, so a hidden folder reads as
    /// hidden rather than as a folder.
    Hidden,
    Doc,
    Data,
    Code,
    Archive,
    Image,
    Media,
    /// Carries the exec bit *and* nothing about its name says otherwise.
    Exec,
    /// Nothing claimed it — renders in the default foreground colour.
    Plain,
}

/// One row in a directory listing.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    /// Modification time, Unix seconds.
    pub modified: i64,
    /// Creation/birth time, Unix seconds; `0` when the platform/filesystem does
    /// not report it.
    pub created: i64,
    /// POSIX permission bits (e.g. `0o755`).
    pub permissions: u32,
    /// Numeric owner uid / group gid (0 on non-unix).
    pub uid: u32,
    pub gid: u32,
    /// Resolved owner / group names; `None` when unresolved (frontend then shows
    /// the numeric id).
    pub owner: Option<String>,
    pub group: Option<String>,
    /// Hardlink count (0 on non-unix).
    pub nlink: u32,
    /// Target path when `kind == Symlink`; `None` otherwise.
    pub symlink_target: Option<String>,
    /// The raw POSIX fact: any of the three x bits is set. Note this is *not* the
    /// same question as "may we run it" — see [`crate::filetype::is_runnable`].
    pub is_executable: bool,
    /// Which colour class this row falls into (§4).
    pub category: EntryCategory,
    pub marker: EntryMarker,
    /// Recursively computed folder size in bytes, when it has been calculated
    /// (F3) and is present in the size cache; `None` otherwise. Always `None`
    /// for non-directories.
    pub computed_size: Option<u64>,
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

/// An open quick-search box on a panel (§5.9).
///
/// The core is the sole author of `query`: a character that matches nothing is
/// rejected rather than appended, so the string always describes a real entry.
/// That is why the frontend renders it as text rather than hosting an `<input>` —
/// a focused field would show the rejected character before the core could
/// withdraw it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct QuickSearch {
    /// What the user has typed so far — only characters that matched.
    pub query: String,
    /// Bumped on every rejected character, so the frontend can beep and flash.
    /// A counter rather than a flag: two misses in a row must fire twice, and a
    /// flag that is already `true` would look unchanged.
    pub miss_rev: u32,
}

/// Why a panel is showing an out-of-band notice (§5.6). Drives styling only —
/// the sentence itself comes from [`PanelNotice::message`], because composing it
/// needs the old path / volume name and that is core knowledge, not the
/// frontend's (same split as [`OpErrorInfo::reason`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum PanelNoticeKind {
    /// The directory was renamed or moved and the panel followed it.
    Moved,
    /// Something else now occupies the path; the listing was reloaded fresh.
    Replaced,
    /// The directory was deleted; the panel fell back to an ancestor.
    Deleted,
    /// The directory was moved to the Trash; the panel fell back to an ancestor.
    Trashed,
    /// The volume went away; the panel fell back to home.
    Unmounted,
    /// The directory still exists but became unreadable. The panel deliberately
    /// stays put — navigating away would hide the actual problem (§5.6).
    Denied,
}

/// A one-line, non-modal explanation of something that happened to a panel's
/// directory without the user asking (§5.6). Rendered in the panel footer, not as
/// a dialog: a background process renaming a folder is not a failure, and the
/// red-background dialog is reserved for operations that failed.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PanelNotice {
    pub kind: PanelNoticeKind,
    /// Ready-to-render sentence, composed by the core.
    pub message: String,
    /// What the notice is about — the directory's previous path, or the volume.
    pub path: String,
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
    /// The open quick-search box, or `None` when there is none (§5.9). Transient
    /// like `top_index`: never persisted, and any other input ends it.
    pub search: Option<QuickSearch>,
    /// Set when the directory changed underneath the panel and the core had to
    /// react (§5.6). Transient: cleared by the next deliberate navigation.
    pub notice: Option<PanelNotice>,
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
            search: None,
            notice: None,
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
    /// The command line's state (§5.7). Rides along on every snapshot so the
    /// terminal row re-renders in lockstep with the panels — in particular its
    /// `cwd`, which follows the active panel.
    pub terminal: TerminalState,
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

// ---------------------------------------------------------------------------
// Embedded terminal (§5.7 / §8 Phase 2)
// ---------------------------------------------------------------------------

/// What the run-status indicator at the right edge of the command line shows.
///
/// `Ok`/`Error` are the *last* run's verdict and decay back to `Idle` as soon as
/// the user touches any control again, so a stale green dot never masquerades as
/// a fresh result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum TerminalStatus {
    /// Nothing has run since the user last did something — grey, barely visible.
    #[default]
    Idle,
    /// A command is running — yellow, flashing.
    Running,
    /// Last run exited 0 and wrote nothing to stderr — green.
    Ok,
    /// Last run exited non-zero or wrote to stderr — red.
    Error,
}

/// How much room the terminal occupies. Three sizes, two toggles: Cmd+Shift+T
/// flips `Collapsed` ↔ `Half`, and Esc draws the panels aside as a curtain over
/// the `Full` terminal (§6) and back again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum TerminalSize {
    /// Just the command line, directly under the panel footers.
    #[default]
    Collapsed,
    /// Bottom half of the window: output pane above, command line pinned below.
    Half,
    /// Panels hidden entirely — the Esc curtain.
    Full,
}

/// Direction for command-history recall (Up / Down at the prompt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum HistoryDir {
    /// Older — what Up does.
    Prev,
    /// Newer, ending back at the line the user was typing.
    Next,
}

/// Which pipe a chunk of output came from. Only the core cares: a command that
/// wrote *anything* to stderr finishes `Error` even when it exits 0 (§5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// The command line's full state — everything the prompt row renders.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct TerminalState {
    /// The text in the prompt. Survives losing focus: it is core state, not a
    /// property of the focused widget (§5.7).
    pub input: String,
    /// Bumped **only** when the core itself rewrites `input` (history recall,
    /// Ctrl+Enter insertion, clear, run). The frontend re-seeds its `<input>`
    /// element on a change and otherwise leaves the user's typing alone, so a
    /// snapshot arriving mid-keystroke can never clobber it.
    pub input_rev: u32,
    /// Working directory the next command runs in — the active panel's directory
    /// (§8 Phase 2: the terminal tracks the active panel).
    pub cwd: String,
    pub size: TerminalSize,
    /// Whether the prompt owns the keyboard (Cmd+T). Drives the accent border.
    pub focused: bool,
    pub status: TerminalStatus,
    /// The command line currently executing, if any.
    pub running: Option<String>,
    /// Scrollback cap in bytes, configurable from the expanded pane (1 MiB
    /// default, §5.7).
    pub scrollback_bytes: u64,
}

/// An incremental scrollback update.
///
/// The core owns the buffer and its eviction policy; this is the delta that
/// keeps the frontend's mirror exact. Lines rather than bytes, deliberately — a
/// byte count would be unusable in a frontend that indexes strings in UTF-16
/// units.
///
/// **Apply it in this order — append, then drop:**
///
/// ```text
/// lines   = lines.concat(chunk.lines).slice(chunk.dropped)
/// pending = chunk.pending
/// ```
///
/// The order is load-bearing, not stylistic. The core appends and *then* trims,
/// so a single large append can evict lines it just added — feed a 3000-line
/// burst into a 2 KB buffer and `dropped` exceeds everything the mirror held
/// beforehand. Dropping first would clamp at zero and leak the surplus, leaving
/// the mirror permanently longer than the buffer it mirrors.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct TerminalChunk {
    /// Lines completed by this append (no trailing newline).
    pub lines: Vec<String>,
    /// The still-incomplete trailing line; replaces whatever partial the
    /// frontend was holding.
    pub pending: String,
    /// Leading lines evicted to stay under the byte cap, counted **after** this
    /// append — so it can exceed the mirror's previous length.
    pub dropped: u32,
}

/// The whole scrollback, pulled on expand / on mount / after a cap change.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct TerminalBuffer {
    pub lines: Vec<String>,
    pub pending: String,
}

// ---------------------------------------------------------------------------
// Embedded viewer & editor (§5.5 / §8 Phase 3)
// ---------------------------------------------------------------------------

/// What kind of content a file holds, decided by sniffing its first bytes
/// (DOS Navigator's "smart viewer" type detection). Drives which embedded mode
/// F3/F4 open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Text,
    Binary,
    Image,
}

/// Character encoding of a text file. Detected from a BOM or by sniffing;
/// remembered so the editor writes back what it read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TextEncoding {
    #[default]
    Utf8,
    /// UTF-8 with a byte-order mark, which must be preserved on save.
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Latin1,
}

/// Line-ending style, detected from the file's first line break and re-applied
/// on save so editing never rewrites every line of a CRLF file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum Eol {
    #[default]
    Lf,
    Crlf,
    Cr,
}

/// Result of sniffing a file's leading bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FileProbe {
    pub size: u64,
    pub media: MediaKind,
    pub encoding: TextEncoding,
    pub eol: Eol,
}

/// Which representation the viewer is currently showing. Text and Hex toggle
/// (F4); Image is fixed for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum ViewerMode {
    Text,
    Hex,
    Image,
}

/// One rendered screenful of the viewer. Text and hex modes emit the same shape
/// — a gutter column plus row strings — so the frontend renders one way and all
/// formatting stays in the core.
///
/// Position is tracked as a **byte offset**, and `percent` is byte-based, so the
/// viewer never needs a file's total line count and opens a multi-gigabyte log
/// instantly (FAR behaviour).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ViewPage {
    pub id: String,
    pub path: String,
    pub name: String,
    pub mode: ViewerMode,
    pub wrap: bool,
    pub encoding: TextEncoding,
    /// Line numbers (text) or byte offsets (hex), one per row.
    pub gutter: Vec<String>,
    pub rows: Vec<String>,
    pub top_offset: u64,
    pub total_bytes: u64,
    /// Position through the file, 0–100, from `top_offset`.
    pub percent: u8,
    /// 1-based logical line at the top of the window, when the index reaches
    /// that far; `None` in hex mode.
    pub top_line: Option<u64>,
    /// Horizontal scroll, in characters, when wrapping is off.
    pub col_offset: u64,
    /// Whether the file could be written — drives the status line's read-only
    /// marker and whether F6 (view → edit) can offer an editable buffer.
    pub writable: bool,
}

/// Viewer scroll intents — one offset-advancing state machine, mirroring the
/// panel cursor's design (§10).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ViewMotion {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Home,
    End,
    ColLeft,
    ColRight,
}

/// Target for the viewer's Goto (F5): a 1-based line, an absolute byte offset,
/// or a percentage through the file.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum GotoTarget {
    Line(u64),
    Offset(u64),
    Percent(u8),
}

/// Search direction for F7 / Shift+F7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum SearchDirection {
    Forward,
    Backward,
}

/// An open editor document. The core owns the document (path, encoding, line
/// endings, permissions, on-disk identity); the frontend owns the editable text
/// buffer and hands it back whole on save.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct EditDoc {
    pub id: String,
    pub path: String,
    pub name: String,
    /// Full text, decoded and normalized to `\n` line endings.
    pub text: String,
    pub encoding: TextEncoding,
    pub eol: Eol,
    /// True when the file is not writable — F2 reports this rather than failing
    /// at write time.
    pub read_only: bool,
}

/// Structured result of an editor save (§5.6 — never an opaque error).
/// `Conflict` means the file changed on disk since it was opened; the frontend
/// offers Overwrite (retry with `force`) or Cancel in the red dialog (§5.4b).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SaveOutcome {
    Saved,
    Conflict(String),
    ReadOnly,
    Failed(String),
}

/// What `open_entry` actually did — the frontend renders the embedded arms and
/// ignores the external ones (§5.5).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum OpenOutcome {
    /// Handed off to an external application; nothing to render.
    Launched,
    /// An executable was started; output arrives via the exec events.
    Executing,
    /// Nothing to open (`..`, a directory, or an empty panel).
    Nothing,
    Viewer(ViewPage),
    Editor(EditDoc),
}

// ---------------------------------------------------------------------------
// Config (persisted, TOML)
// ---------------------------------------------------------------------------

/// One keybinding: an action id (e.g. `"cursor.down"`) and the key chords bound
/// to it. Sourced from core config so the webview never hardcodes keys (§6/§7).
/// Remapping, persistence, and conflict detection are a later slice.
///
/// `context` scopes the binding to the surface that owns the keyboard —
/// `"panels"`, `"viewer"`, `"editor"`, `"terminal"`, `"help"` — because the same
/// F-key means different things in each (F4 edits a file from the panels,
/// toggles hex in the viewer). The frontend groups the keymap by context and
/// consults the one for whichever surface is on top.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct KeyBinding {
    pub context: String,
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

/// One file-type → handler mapping (§5.5 / §7). Associates a set of extensions
/// with what should happen for each action. Matched case-insensitively on the
/// entry's extension, no leading dot (e.g. `"md"`).
///
/// Each value is a handler string, parsed by [`crate::open::Handler`]:
/// `"internal"` (the embedded viewer/editor), `"system"` (the OS default app),
/// or an application name (`"Visual Studio Code"`). `None` means "no opinion" —
/// View/Edit then fall back to `open`, and finally to the built-in default for
/// the file's detected type.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct FileAssociation {
    /// Lower-case extensions this mapping claims, e.g. `["md", "markdown"]`.
    pub extensions: Vec<String>,
    /// Handler for the default Open action (Enter / double-click).
    pub open: Option<String>,
    /// Handler for View (F3, read-only); falls back to `open`.
    pub view: Option<String>,
    /// Handler for Edit (F4, read-write); falls back to `open`.
    pub edit: Option<String>,
}

/// Embedded-viewer preferences (§7). `wrap` is toggled with F2 in the viewer and
/// persisted, matching how the panels persist their view state (§5.8).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct ViewerPrefs {
    pub wrap: bool,
    pub tab_width: u8,
    pub hex_bytes_per_row: u8,
}

impl Default for ViewerPrefs {
    fn default() -> Self {
        Self {
            wrap: false,
            tab_width: 4,
            hex_bytes_per_row: 16,
        }
    }
}

/// Embedded-terminal preferences (§7). The scrollback cap is adjustable from the
/// corner of the expanded pane, and the pane's size is remembered across
/// restarts the way the panels remember their view state (§5.8).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct TerminalPrefs {
    /// Scrollback cap in bytes — 1 MiB by default (§5.7).
    pub scrollback_bytes: u64,
    /// Shell used to run commands. `None` means `$SHELL`, falling back to
    /// `/bin/sh`.
    pub shell: Option<String>,
    /// The pane size to restore on launch. `Full` is deliberately not persisted
    /// — the Esc curtain is a transient look, not a startup state.
    pub size: TerminalSize,
}

impl Default for TerminalPrefs {
    fn default() -> Self {
        Self {
            scrollback_bytes: 1 << 20, // 1 MiB
            shell: None,
            size: TerminalSize::Collapsed,
        }
    }
}

/// Where a panel goes when its directory is gone for good and there is nothing to
/// follow (§5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "kebab-case")]
pub enum OnLost {
    /// Walk up to the closest ancestor that still exists and is readable.
    #[default]
    NearestAncestor,
    /// Go straight to the home directory.
    Home,
}

/// Directory-watching preferences (§5.6 / §7).
///
/// Every value here is a rate or a policy the user may want to change, so none of
/// it is hardcoded (CLAUDE.md: keybindings and theme values are config-driven —
/// the same rule applies to anything tunable).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct WatchPrefs {
    /// Master switch. Off means the panels only refresh on Ctrl+R and after the
    /// app's own operations, exactly as they did before watching existed.
    pub enabled: bool,
    /// Quiet period before a dirty directory is re-listed. Filesystem events
    /// arrive in storms; this is what stops one re-list per event.
    pub debounce_ms: u64,
    /// Upper bound on how long `debounce_ms` may keep deferring. Without it a
    /// sustained write (an archive extracting into the visible directory) would
    /// never go quiet and the panel would show nothing until it finished.
    pub max_delay_ms: u64,
    /// How often to re-check that the panel's directory is still where it was.
    /// Two syscalls per panel; this is what catches a renamed *ancestor*, which
    /// produces no event in either watched directory.
    pub identity_poll_ms: u64,
    /// Follow a renamed or moved directory to its new path. Off falls back to
    /// treating a move like a deletion.
    pub follow_moves: bool,
    /// Where to land when the directory is gone and cannot be followed.
    pub on_lost: OnLost,
    /// Poll interval for volumes FSEvents does not cover (SMB/NFS). `0` disables
    /// watching there entirely, leaving Ctrl+R and the focus refresh.
    pub poll_non_local_ms: u64,
    /// Re-check both panels when the window regains focus. Cheap, and it covers
    /// everything the watcher structurally cannot (dropped events, suspension).
    pub refresh_on_focus: bool,
}

impl Default for WatchPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            debounce_ms: 200,
            max_delay_ms: 1000,
            identity_poll_ms: 2000,
            follow_moves: true,
            on_lost: OnLost::default(),
            poll_non_local_ms: 5000,
            refresh_on_focus: true,
        }
    }
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
    /// Largest file the embedded editor will load. Above this, F4 hands off to
    /// the external editor — the editor holds the whole document in memory,
    /// unlike the viewer, which pages.
    pub edit_max_bytes: u64,
    pub left_panel: PanelPrefs,
    pub right_panel: PanelPrefs,
    pub viewer: ViewerPrefs,
    pub terminal: TerminalPrefs,
    pub watch: WatchPrefs,
    /// File-type → external-application map (§5.5). Empty by default, so every
    /// file opens with the system default until the user edits the TOML.
    ///
    /// Stays **last**: it serializes as an array-of-tables, and TOML rejects any
    /// plain value or table emitted after one.
    pub associations: Vec<FileAssociation>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            trash_default: false,
            theme: "classic".to_string(),
            edit_max_bytes: 16 << 20, // 16 MiB
            left_panel: PanelPrefs::default(),
            right_panel: PanelPrefs::default(),
            viewer: ViewerPrefs::default(),
            terminal: TerminalPrefs::default(),
            watch: WatchPrefs::default(),
            associations: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Help (F1) — see `crate::help`
// ---------------------------------------------------------------------------

/// Who this app is, for the About topic. Filled in by the platform adapter from
/// its packaging metadata (Cargo / the Tauri bundle config), because the core
/// has no packaging of its own to read.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct AppInfo {
    /// Display name, e.g. `"dimnav"` — the product name, not the crate.
    pub name: String,
    pub version: String,
    pub description: String,
    /// SPDX identifier, e.g. `"MIT"`.
    pub license: String,
    /// Project website. Empty string when unset — the About topic omits the row
    /// rather than rendering a dead link.
    pub homepage: String,
    /// Source repository. Empty string when unset.
    pub repository: String,
    /// Where to support the project financially. Empty string when unset.
    pub sponsor: String,
}

/// The whole help book: every topic, already rendered and filtered. The frontend
/// picks one to show and renders it — it makes no decisions about content.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct HelpBook {
    pub topics: Vec<HelpTopicView>,
}

/// One topic, as it appears in the left-hand rail plus the body it renders.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct HelpTopicView {
    /// Stable id, e.g. `"about"`. The frontend never switches on it, but it makes
    /// the payload self-describing and gives a plugin topic a handle.
    pub id: String,
    /// Label for the topic rail.
    pub title: String,
    pub body: HelpBody,
}

/// The content of a topic. Tagged the same way as [`OpenOutcome`] so the
/// generated TypeScript is a discriminated union the renderer can switch on.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum HelpBody {
    About(AboutBody),
    Shortcuts(ShortcutsBody),
}

/// The About topic: who the app is, plus a few label/value facts and the
/// project's outbound links.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AboutBody {
    pub app: AppInfo,
    pub lines: Vec<HelpLine>,
    /// Rendered as activatable rows, separately from [`Self::lines`], so the
    /// renderer never has to guess which values happen to be URLs. Already
    /// filtered — an unset link is absent here, not empty.
    pub links: Vec<HelpLink>,
}

/// One label/value row.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct HelpLine {
    pub label: String,
    pub value: String,
}

/// One outbound link. Opened through the OS browser by the adapter, never
/// navigated to inside the webview.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct HelpLink {
    pub label: String,
    pub url: String,
}

/// The Shortcuts topic: the live keymap, grouped and filtered.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ShortcutsBody {
    /// Echo of the query these results were produced for, so the renderer can
    /// tell a stale response from a current one.
    pub query: String,
    /// How many shortcuts survived the filter, and how many exist in total —
    /// the "12 of 68" counter.
    pub match_count: u32,
    pub total_count: u32,
    /// One section per keyboard context that has at least one match.
    pub sections: Vec<ShortcutSection>,
}

/// All shortcuts for one keyboard context (`"panels"`, `"viewer"`, …). The same
/// key can mean different things in different contexts, which is exactly why the
/// list is sectioned this way rather than flattened.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ShortcutSection {
    /// The raw context id from the keymap.
    pub context: String,
    /// Display heading, e.g. `"Panels"`.
    pub title: String,
    pub groups: Vec<ShortcutGroup>,
}

/// A run of related shortcuts within a section, e.g. "Cursor motion".
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ShortcutGroup {
    pub category: String,
    pub title: String,
    pub items: Vec<ShortcutItem>,
}

/// One shortcut row.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ShortcutItem {
    /// The action id, e.g. `"op.copy"` — shown small, and searchable.
    pub action: String,
    /// **Display** chords (`"⌘⇧T"`, `"↑"`, `"Space"`), not the raw chord strings.
    /// The renderer never has to know the internal chord format.
    pub keys: Vec<String>,
    pub title: String,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `other` is an involution — which is what lets callers use it as "the
    /// passive panel" without also tracking which one they started from.
    #[test]
    fn the_other_panel_is_the_opposite_one() {
        assert_eq!(PanelId::Left.other(), PanelId::Right);
        assert_eq!(PanelId::Right.other(), PanelId::Left);
        assert_eq!(PanelId::Left.other().other(), PanelId::Left);
    }
}
