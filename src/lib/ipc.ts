// Thin façade over the generated Tauri bindings.
//
// The rest of the frontend imports IPC from here, never from `bindings.ts`
// directly, so the generated file can be regenerated freely and a future
// renderer swap only re-points this one module. All logic still lives in the
// Rust core — this is pure marshalling (SPEC §3).

import { commands } from "./bindings";
import type { AppSnapshot, Motion, NavTarget, PanelId } from "./bindings";

export { commands } from "./bindings";
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
  setActivePanel: (panel: PanelId) => unwrap(commands.setActivePanel(panel)),
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
};
