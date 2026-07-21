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
  Motion,
  NavTarget,
  OpenAction,
  OpKind,
  PanelId,
  Resolution,
  SortMode,
  ViewMode,
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
  EntryKind,
  EntryMarker,
  ViewMode,
  SortMode,
  Motion,
  NavTarget,
  PanelId,
  KeyBinding,
  OpKind,
  OpenAction,
  Resolution,
  ErrorResolution,
  OpProgress,
  OpOutcome,
  CollisionPrompt,
  OpErrorInfo,
  ExecOutput,
  ExecDone,
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
  refresh: (panel: PanelId) => unwrap(commands.refresh(panel)),
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
 * panel's cursor with an external tool — Open uses the system default (and runs
 * executables, streaming output via {@link events.execOutputEvent}); View/Edit
 * route to the configured tool. `cancelExec` kills a running executable.
 */
export const open = {
  openEntry: (panel: PanelId, action: OpenAction) =>
    unwrap(commands.openEntry(panel, action)),
  cancelExec: () => unwrap(commands.cancelExec()),
};
