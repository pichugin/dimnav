// Thin façade over the generated Tauri bindings.
//
// The rest of the frontend imports IPC from here, never from `bindings.ts`
// directly, so the generated file can be regenerated freely and a future
// renderer swap only re-points this one module. All logic still lives in the
// Rust core — this is pure marshalling (SPEC §3).

import { commands } from "./bindings";
import type {
  AppSnapshot,
  ErrorResolution,
  GotoTarget,
  HistoryDir,
  Motion,
  NavTarget,
  OpenAction,
  OpKind,
  PanelId,
  Resolution,
  SearchDirection,
  SortMode,
  ViewMode,
  ViewMotion,
} from "./bindings";

export { commands, events } from "./bindings";
export type {
  AppSnapshot,
  PanelState,
  PanelGeometry,
  PanelPrefs,
  Config,
  DirListing,
  Entry,
  EntryCategory,
  EntryKind,
  EntryMarker,
  ViewMode,
  SortMode,
  Motion,
  NavTarget,
  PanelId,
  QuickSearch,
  KeyBinding,
  OpKind,
  OpenAction,
  Resolution,
  ErrorResolution,
  OpProgress,
  OpOutcome,
  CollisionPrompt,
  OpErrorInfo,
  // Embedded terminal (§5.7)
  TerminalState,
  TerminalStatus,
  TerminalSize,
  TerminalChunk,
  TerminalBuffer,
  HistoryDir,
  // Embedded viewer / editor (§5.5)
  OpenOutcome,
  ViewPage,
  ViewMotion,
  ViewerMode,
  GotoTarget,
  SearchDirection,
  EditDoc,
  SaveOutcome,
  TextEncoding,
  Eol,
  // Help (F1) (§6)
  AppInfo,
  HelpBook,
  HelpTopicView,
  HelpBody,
  AboutBody,
  HelpLine,
  HelpLink,
  ShortcutsBody,
  ShortcutSection,
  ShortcutGroup,
  ShortcutItem,
  // In-app updates
  UpdateInfo,
} from "./bindings";

/** Result envelope produced by tauri-specta for `Result`-returning commands. */
type Envelope<T> = { status: "ok"; data: T } | { status: "error"; error: string };

/** Unwrap a command envelope, throwing on the error arm. */
async function unwrap<T>(p: Promise<Envelope<T>>): Promise<T> {
  const r = await p;
  if (r.status === "error") throw new Error(r.error);
  return r.data;
}

/**
 * Navigation API — every call returns the full {@link AppSnapshot} the frontend
 * renders. The core owns all cursor/selection state; these just forward intents.
 */
export const nav = {
  init: () => unwrap(commands.init()),
  setViewport: (panel: PanelId, columns: number, rows: number) =>
    unwrap(commands.setViewport(panel, columns, rows)),
  moveCursor: (panel: PanelId, motion: Motion) =>
    unwrap(commands.moveCursor(panel, motion)),
  setCursor: (panel: PanelId, index: number) =>
    unwrap(commands.setCursor(panel, index)),
  setActivePanel: (panel: PanelId) => unwrap(commands.setActivePanel(panel)),
  // Per-panel view state (§5.8) — the core persists each change to config.toml.
  setViewMode: (panel: PanelId, mode: ViewMode) =>
    unwrap(commands.setViewMode(panel, mode)),
  setSortMode: (panel: PanelId, mode: SortMode) =>
    unwrap(commands.setSortMode(panel, mode)),
  setShowHidden: (panel: PanelId, value: boolean) =>
    unwrap(commands.setShowHidden(panel, value)),
  navigate: (panel: PanelId, target: NavTarget) =>
    unwrap(commands.navigate(panel, target)),
  /** Ctrl+= — push the active panel's folder onto the other panel (§5.1). */
  equalizePanels: () => unwrap(commands.equalizePanels()),
  refresh: (panel: PanelId) => unwrap(commands.refresh(panel)),
  // Quick search (§5.9). The core owns the query and decides whether a typed
  // character is accepted or rejected — `search.miss_rev` on the returned panel
  // increments on a reject, which is what the beep watches.
  searchStart: (panel: PanelId) => unwrap(commands.searchStart(panel)),
  searchPush: (panel: PanelId, text: string) => unwrap(commands.searchPush(panel, text)),
  searchBackspace: (panel: PanelId) => unwrap(commands.searchBackspace(panel)),
  searchClose: (panel: PanelId) => unwrap(commands.searchClose(panel)),
  getKeymap: () => commands.getKeymap(),
  // Selection (§5.3)
  toggleSelection: (panel: PanelId) => unwrap(commands.toggleSelection(panel)),
  selectAndMove: (panel: PanelId, motion: Motion) =>
    unwrap(commands.selectAndMove(panel, motion)),
  selectAll: (panel: PanelId) => unwrap(commands.selectAll(panel)),
  deselectAll: (panel: PanelId) => unwrap(commands.deselectAll(panel)),
  // Create directory (F7) / Rename (Shift+F6) (§5.4)
  createDir: (panel: PanelId, name: string) => unwrap(commands.createDir(panel, name)),
  rename: (panel: PanelId, newName: string) => unwrap(commands.rename(panel, newName)),
  // Recursively compute folder sizes (F3 on a folder); results are cached and
  // surfaced onto entries' `computed_size`.
  calculateDirSize: (paths: string[]) => unwrap(commands.calculateDirSize(paths)),
  setTrashDefault: (value: boolean) => unwrap(commands.setTrashDefault(value)),
};

/**
 * File-operation API (§5.4a). `startTransfer` kicks off a copy/move on the active
 * panel's selection and returns an `op_id`; progress, collisions, errors, and
 * completion arrive as events (see {@link events}). The `resolve*` calls answer a
 * blocking prompt for a given op.
 */
export const ops = {
  startTransfer: (kind: OpKind, dest: string) =>
    unwrap(commands.startTransfer(kind, dest)),
  startDelete: () => unwrap(commands.startDelete()),
  resolveCollision: (opId: string, resolution: Resolution) =>
    unwrap(commands.resolveCollision(opId, resolution)),
  resolveError: (opId: string, resolution: ErrorResolution) =>
    unwrap(commands.resolveError(opId, resolution)),
  cancelOp: (opId: string) => unwrap(commands.cancelOp(opId)),
};

/**
 * Open / View / Edit API (§5.5). `openEntry` opens the entry under the given
 * panel's cursor with an external tool — Open uses the system default and runs
 * executables, streaming their output into the terminal buffer; View/Edit route
 * to the configured tool or the embedded surfaces.
 */
export const open = {
  openEntry: (panel: PanelId, action: OpenAction) =>
    unwrap(commands.openEntry(panel, action)),
};

/**
 * Embedded terminal API (§5.7). The core owns the prompt text, the history, the
 * scrollback and its eviction, the run-status machine, and the built-ins — every
 * call here just forwards an intent and gets the whole {@link AppSnapshot} back.
 *
 * Output does not come through these calls: it arrives as
 * {@link events.terminalChunkEvent} line deltas, and `buffer` re-syncs the
 * frontend's mirror from scratch.
 */
export const terminal = {
  /** Cmd+T — move the keyboard to the prompt, or back to the active panel. */
  toggleFocus: () => unwrap(commands.terminalToggleFocus()),
  /** Cmd+Shift+T — expand to the bottom half of the window, or collapse. */
  toggleHalf: () => unwrap(commands.terminalToggleHalf()),
  /** Esc — the panels-aside curtain over the full-height terminal (§6). */
  toggleCurtain: () => unwrap(commands.terminalToggleCurtain()),
  /**
   * Mirror what the user is typing. Fire-and-forget by design: re-rendering the
   * input from the response would fight the caret (see `input_rev`).
   */
  setInput: (text: string) => unwrap(commands.terminalSetInput(text)),
  run: () => unwrap(commands.terminalRun()),
  /** Ctrl+C — interrupt a running command, else clear the prompt. */
  interruptOrClear: () => unwrap(commands.terminalInterruptOrClear()),
  history: (dir: HistoryDir) => unwrap(commands.terminalHistory(dir)),
  /** Ctrl+Enter — append the panel's focused name, shell-quoted (§5.7). */
  insertName: (panel: PanelId) => unwrap(commands.terminalInsertName(panel)),
  setScrollback: (bytes: number) => unwrap(commands.terminalSetScrollback(bytes)),
  clearBuffer: () => unwrap(commands.terminalClearBuffer()),
  /** The whole scrollback — the frontend's initial sync and re-sync. */
  buffer: () => unwrap(commands.terminalBuffer()),
};

/**
 * Embedded viewer API (F3, §5.5). Every call returns the freshly rendered
 * {@link ViewPage} — the core holds the open file handle, tracks the byte
 * position, and does all wrapping, tab expansion, and hex formatting, so the
 * frontend only ever paints the rows it is given.
 */
export const viewer = {
  setViewport: (id: string, rows: number, cols: number) =>
    unwrap(commands.viewSetViewport(id, rows, cols)),
  scroll: (id: string, motion: ViewMotion) => unwrap(commands.viewScroll(id, motion)),
  toggleHex: (id: string) => unwrap(commands.viewToggleHex(id)),
  setWrap: (id: string, wrap: boolean) => unwrap(commands.viewSetWrap(id, wrap)),
  /** `null` means "not found" — a normal outcome, not an error. */
  search: (id: string, needle: string, direction: SearchDirection) =>
    unwrap(commands.viewSearch(id, needle, direction)),
  goto: (id: string, target: GotoTarget) => unwrap(commands.viewGoto(id, target)),
  /** F6 — hand the same file to the editor; the core supplies the path. */
  toEdit: (id: string) => unwrap(commands.viewToEdit(id)),
  close: (id: string) => unwrap(commands.viewClose(id)),
};

/**
 * Embedded editor API (F4, §5.5). The core owns the document — encoding, line
 * endings, permissions, and the file's on-disk identity — while the text widget
 * here owns the buffer and hands it back whole on save.
 */
export const editor = {
  save: (id: string, text: string, force = false) =>
    unwrap(commands.editSave(id, text, force)),
  /** F6 — drop back to the viewer on the same file. */
  toView: (id: string) => unwrap(commands.editToView(id)),
  close: (id: string) => unwrap(commands.editClose(id)),
};

/**
 * Help API (F1, §6). The core assembles every topic — including the shortcut
 * list, which it derives from the live keymap — and applies the search filter,
 * so this is one call and the renderer holds no help content of its own.
 */
export const help = {
  /** `query` filters the shortcut list; `""` returns everything. */
  book: (query: string) => commands.getHelp(query),
  /**
   * Hand one of the About topic's links to the OS browser. Never navigate the
   * webview to it — that would replace the app with a web page and there is no
   * way back.
   */
  openLink: (url: string) => unwrap(commands.openLink(url)),
};

/**
 * In-app updates. The release feed is checked once at startup and the result is
 * surfaced in Help → About; there is no nagging dialog.
 */
export const updates = {
  /**
   * `null` means up to date *or* undeterminable — the backend folds the two
   * together on purpose, so an offline launch is silent rather than an error.
   */
  check: () => unwrap(commands.checkUpdate()),
  /** Downloads, verifies, installs, and relaunches. Does not return on success. */
  install: () => unwrap(commands.installUpdate()),
};
