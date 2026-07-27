<script lang="ts">
  import { onMount, tick } from "svelte";
  import { nav, ops, open, viewer, editor, terminal, help, updates, events } from "./lib/ipc";
  import type {
    AppSnapshot,
    EditDoc,
    Entry,
    ErrorResolution,
    GotoTarget,
    HelpBook,
    HistoryDir,
    KeyBinding,
    Motion,
    OpKind,
    OpenOutcome,
    PanelId,
    PanelState,
    Resolution,
    SearchDirection,
    SortMode,
    TerminalState,
    UpdateInfo,
    ViewMode,
    ViewMotion,
    ViewPage,
  } from "./lib/ipc";
  import Viewer from "./lib/Viewer.svelte";
  import Editor from "./lib/Editor.svelte";
  import Terminal from "./lib/Terminal.svelte";
  import type { UnlistenFn } from "@tauri-apps/api/event";
  // Vite rewrites this to a hashed bundle URL. Generated from the same emblem as
  // the app icon by `scripts/make-icon.py` — regenerate both together.
  import logoUrl from "./assets/dimnav-icon.png";

  // Row height in px — single source of truth shared by the grid layout and the
  // viewport measurement, so the core's rows_per_column matches what's rendered.
  const ROW_H = 22;
  const SIDES: PanelId[] = ["left", "right"];

  function emptyPanel(): PanelState {
    return {
      path: "",
      entries: [],
      cursor_index: 0,
      top_index: 0,
      selection: [],
      view_mode: { kind: "columns", columns: 2 },
      sort_mode: "name_folders_first",
      show_hidden: true,
      geometry: { columns: 0, rows_per_column: 0 },
      search: null,
    };
  }

  function emptyTerminal(): TerminalState {
    return {
      input: "",
      input_rev: 0,
      cwd: "",
      size: "collapsed",
      focused: false,
      status: "idle",
      running: null,
      scrollback_bytes: 1024 * 1024,
    };
  }

  let snapshot = $state<AppSnapshot>({
    left: emptyPanel(),
    right: emptyPanel(),
    active: "left",
    trash_default: false,
    terminal: emptyTerminal(),
  });
  let status = $state("starting…");
  // True while Shift is held, so the F-key bar can show its shifted labels
  // (F6 → Rename). Reset on blur so a released-outside Shift never sticks.
  let shiftHeld = $state(false);

  // --- File-operation dialogs (§5.4a) ---------------------------------------
  // The editable F5/F6 destination prompt, shown before an op starts.
  type DestPrompt = { op: OpKind; value: string };
  let destPrompt = $state<DestPrompt | null>(null);
  // The running op's progress (driven by op-progress events).
  type ActiveOp = { id: string; done: number; total: number; current: string };
  let activeOp = $state<ActiveOp | null>(null);
  // Id of the most recently completed op. A fast op can finish (and clear
  // activeOp) before startTransfer's promise even resolves; this guards against a
  // late progress/init event resurrecting a phantom modal for a done op.
  let finishedOpId: string | null = null;
  // A blocking prompt overlaying the progress modal, awaiting the user's answer.
  type Prompt =
    | { kind: "collision"; opId: string; path: string; multiple: boolean }
    | { kind: "error"; opId: string; path: string; reason: string; offerElevate: boolean };
  let prompt = $state<Prompt | null>(null);
  // The delete confirmation dialog (F8), carrying the "Move to Trash" checkbox.
  type DeleteConfirm = { label: string };
  let deleteConfirm = $state<DeleteConfirm | null>(null);
  // A single-field text prompt reused for Create Directory (F7) and Rename
  // (Shift+F6). `error` holds an inline message (e.g. name collision) so the
  // prompt can stay open for the user to retype.
  type TextPrompt = {
    kind: "mkdir" | "rename" | "search" | "goto";
    value: string;
    error?: string;
  };
  let textPrompt = $state<TextPrompt | null>(null);
  // --- Embedded terminal (§5.7) ---------------------------------------------
  // A mirror of the core's scrollback, never a second copy of the policy: the
  // core sends line deltas and says how many old lines it evicted, so keeping
  // this in step is mechanical. `terminalRef` exposes the pane's scrolling for
  // the PgUp/PgDn bindings.
  let termLines = $state<string[]>([]);
  let termPending = $state("");
  let terminalRef = $state<Terminal | null>(null);

  // --- Embedded viewer / editor (§5.5) --------------------------------------
  // Only one of these is ever open, and it covers the panels like FAR's viewer.
  let viewerPage = $state<ViewPage | null>(null);
  let editorDoc = $state<EditDoc | null>(null);
  // The buffer being typed into; `saved` is the last text the core wrote, so
  // "modified" is a comparison rather than a flag that can drift.
  let editorText = $state("");
  let editorSaved = $state("");
  const editorDirty = $derived(editorText !== editorSaved);
  // Transient status text for whichever overlay is open (search misses, save
  // results), cleared on the next action.
  let overlayMessage = $state("");
  // The last search, so Shift+F7 can repeat it without re-prompting.
  let lastSearch = $state("");
  // A save that hit a conflict, awaiting the user's Overwrite/Cancel answer.
  let saveConflict = $state<string | null>(null);
  // Esc on a modified buffer, awaiting Save / Discard / Cancel.
  let unsavedPrompt = $state(false);
  let editorRef = $state<Editor | null>(null);

  // --- Help (F1, §6) --------------------------------------------------------
  // The whole book comes from the core already filtered — topics, the shortcut
  // list derived from the live keymap, and the search matching. Nothing here
  // decides what help says, only which topic is showing and where it is scrolled.
  let helpOpen = $state(false);
  let helpTopic = $state(0);
  let helpQuery = $state("");
  let helpBook = $state<HelpBook | null>(null);
  let helpContentEl = $state<HTMLElement | null>(null);

  // --- Updates --------------------------------------------------------------
  // Checked once at startup and shown only inside About. Deliberately not a
  // dialog: an update is never urgent enough to interrupt what you were doing,
  // and this app is used by the keystroke.
  let pendingUpdate = $state<UpdateInfo | null>(null);
  let updateInstalling = $state(false);
  let updateError = $state("");

  // Every modal that owns the keyboard until it is answered. F1 defers to these:
  // they are questions, and stacking help on top of one strands the answer.
  const modalUp = $derived(
    !!(destPrompt || textPrompt || activeOp || prompt || deleteConfirm || saveConflict || unsavedPrompt),
  );

  // Which keymap context owns the keyboard right now (§6). Help wins outright —
  // it is reachable from every surface, so while it is up nothing underneath it
  // should see a key. Otherwise the embedded surfaces win over the terminal: the
  // viewer/editor cover the whole window, so while one is open it is the only
  // thing the keyboard can sensibly reach.
  const keyContext = $derived(
    helpOpen
      ? "help"
      : editorDoc
        ? "editor"
        : viewerPage
          ? "viewer"
          : snapshot.terminal.focused
            ? "terminal"
            : "panels",
  );

  // context -> chord string -> action id, built from the core-provided keymap.
  let keymaps: Record<string, Record<string, string>> = {};
  const listingEls: Record<PanelId, HTMLElement | null> = { left: null, right: null };

  // Canonical chord for a KeyboardEvent: modifiers in the fixed order
  // Ctrl+Meta+Alt+Shift, then the key. Must match the format the core keymap
  // produces (config::default_keymap).
  //
  // Shift is deliberately *not* a part for a plain printable key, because the
  // shift is already baked into the character there — `*` reports as "*", not as
  // Shift+8. But that shortcut breaks the moment another modifier joins in: on
  // macOS, WebKit reports the **unshifted** character while Command is held, so
  // Cmd+Shift+T arrives as `key === "t"` and is indistinguishable from Cmd+T
  // unless Shift is spelled out. Hence: with any non-shift modifier present,
  // Shift becomes an explicit part and the letter is lower-cased, so the chord
  // is the same whichever case the engine happens to report.
  function chord(e: KeyboardEvent): string {
    const parts: string[] = [];
    if (e.ctrlKey) parts.push("Ctrl");
    if (e.metaKey) parts.push("Meta");
    if (e.altKey) parts.push("Alt");
    const modified = e.ctrlKey || e.metaKey || e.altKey;
    if (e.shiftKey && (e.key.length > 1 || modified)) parts.push("Shift");
    parts.push(modified && e.key.length === 1 ? e.key.toLowerCase() : e.key);
    return parts.join("+");
  }

  function colsOf(vm: ViewMode): number {
    return vm.kind === "columns" ? vm.columns : 1;
  }

  // Column-major layout for a panel: which slice of entries is visible and how
  // to shape the grid. The window origin is the core's `top_index` — the panel
  // scrolls one entry at a time, it does not flip pages (SPEC §5.2) — clamped
  // here only against a snapshot that is briefly out of step with the geometry.
  function layout(p: PanelState) {
    const rows = p.geometry.rows_per_column;
    const cols = rows > 0 ? colsOf(p.view_mode) : 1;
    const effRows = rows > 0 ? rows : Math.max(1, p.entries.length);
    const pageSize = Math.max(1, cols * effRows);
    const pageStart = Math.min(p.top_index, Math.max(0, p.entries.length - pageSize));
    const pageEntries = p.entries.slice(pageStart, pageStart + pageSize);
    return { cols, rows: effRows, pageStart, pageEntries };
  }

  function gridStyle(cols: number, rows: number): string {
    return (
      `grid-template-columns: repeat(${cols}, minmax(0, 1fr));` +
      `grid-template-rows: repeat(${rows}, ${ROW_H}px);` +
      `grid-auto-flow: column;`
    );
  }

  // The listing color class for an entry (SPEC §4 theming). Pure presentation —
  // first match wins, with hidden taking precedence over folder/type so the
  // requested "hidden files/folders are grey" holds even for dotfolders. Values
  // live in CSS custom properties (app.css) so a theme can remap them.
  // Source-code extensions share the classic selection-blue (--file-code).
  const CODE_EXTS = new Set([
    "c", "h", "cc", "cxx", "cpp", "hpp", "hh", "rs", "ts", "tsx", "js", "jsx",
    "mjs", "cjs", "java", "go", "py", "rb", "swift", "kt", "cs", "php", "sh",
  ]);

  function entryClass(e: Entry): string {
    if (e.marker === "denied" || e.marker === "broken") return ""; // own styling
    if (e.name !== ".." && e.name.startsWith(".")) return "c-hidden";
    if (e.kind === "dir") return "c-dir";
    if (e.kind === "symlink") return "c-symlink";
    if (e.is_executable) return "c-exec";
    const dot = e.name.lastIndexOf(".");
    const ext = dot > 0 ? e.name.slice(dot + 1).toLowerCase() : "";
    if (ext === "md" || ext === "txt") return "c-doc";
    if (ext === "xml" || ext === "json") return "c-data";
    if (CODE_EXTS.has(ext)) return "c-code";
    return "";
  }

  function label(e: Entry): string {
    if (e.marker === "denied") return `⚠ ${e.name}`;
    if (e.kind === "dir") return e.name === ".." ? ".." : `${e.name}/`;
    if (e.kind === "symlink") return `${e.name}@${e.marker === "broken" ? " ⨯" : ""}`;
    return e.name;
  }

  function humanSize(bytes: number): string {
    const u = ["B", "K", "M", "G", "T"];
    let n = bytes;
    let i = 0;
    while (n >= 1024 && i < u.length - 1) {
      n /= 1024;
      i++;
    }
    return `${i === 0 ? n : n.toFixed(1)}${u[i]}`;
  }

  function selectedSize(p: PanelState): number {
    // A calculated folder (F3) contributes its recursive size; everything else
    // its own size. `computed_size` is only set on folders that have been walked.
    return p.selection.reduce((sum, i) => {
      const e = p.entries[i];
      return sum + (e?.computed_size ?? e?.size ?? 0);
    }, 0);
  }

  // Byte-exact size for the status bar and selection total. Precision matters
  // (§ user request) so we group digits rather than collapse to K/M/G.
  function bytesFmt(bytes: number): string {
    return `${bytes.toLocaleString("en-US")} bytes`;
  }

  function buildKeymap(bindings: KeyBinding[]): Record<string, Record<string, string>> {
    const map: Record<string, Record<string, string>> = {};
    for (const b of bindings) {
      const ctx = (map[b.context] ??= {});
      for (const k of b.keys) ctx[k] = b.action;
    }
    return map;
  }

  const PROMPT_TITLES: Record<TextPrompt["kind"], string> = {
    mkdir: "Create directory:",
    rename: "Rename to:",
    search: "Search for:",
    goto: "Go to line, 0x offset, or percent:",
  };
  const PROMPT_ACTIONS: Record<TextPrompt["kind"], string> = {
    mkdir: "Create",
    rename: "Rename",
    search: "Search",
    goto: "Go",
  };

  // Svelte action: focus (and select) a text input as soon as it mounts.
  function autofocus(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  function pct(op: ActiveOp): number {
    return op.total > 0 ? Math.round((op.done / op.total) * 100) : 0;
  }

  async function measure(side: PanelId) {
    const el = listingEls[side];
    if (!el) return;
    const rows = Math.max(1, Math.floor(el.clientHeight / ROW_H));
    snapshot = await nav.setViewport(side, colsOf(snapshot[side].view_mode), rows);
  }

  async function measureAll() {
    await measure("left");
    await measure("right");
  }

  // --- Per-panel view state (§5.8) ------------------------------------------
  // The core owns and persists these; the UI only forwards the intent and, for a
  // view-mode change, re-reports the new geometry so the cursor math matches what
  // is actually rendered.

  // Order of the sort dropdown, and the cycle order for the keyboard shortcut.
  const SORTS: { id: SortMode; label: string }[] = [
    { id: "name_folders_first", label: "Name" },
    { id: "type_name", label: "Type" },
    { id: "size", label: "Size" },
    { id: "date", label: "Date" },
  ];

  const VIEWS: { id: string; label: string; mode: ViewMode }[] = [
    { id: "1", label: "1 col", mode: { kind: "columns", columns: 1 } },
    { id: "2", label: "2 col", mode: { kind: "columns", columns: 2 } },
    { id: "3", label: "3 col", mode: { kind: "columns", columns: 3 } },
    { id: "detailed", label: "Detail", mode: { kind: "detailed" } },
  ];

  function viewId(vm: ViewMode): string {
    return vm.kind === "detailed" ? "detailed" : String(vm.columns);
  }

  async function setView(side: PanelId, mode: ViewMode) {
    try {
      snapshot = await nav.setViewMode(side, mode);
      await tick();
      await measure(side); // column count changed — re-report the geometry
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  async function setSort(side: PanelId, mode: SortMode) {
    try {
      snapshot = await nav.setSortMode(side, mode);
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  async function cycleSort(side: PanelId) {
    const i = SORTS.findIndex((s) => s.id === snapshot[side].sort_mode);
    await setSort(side, SORTS[(i + 1) % SORTS.length].id);
  }

  async function setHidden(side: PanelId, value: boolean) {
    try {
      snapshot = await nav.setShowHidden(side, value);
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  // --- Detailed-mode formatting (pure presentation) -------------------------

  function fmtDate(unixSeconds: number): string {
    if (!unixSeconds) return "";
    const d = new Date(unixSeconds * 1000);
    const p = (n: number) => String(n).padStart(2, "0");
    return `${p(d.getDate())}.${p(d.getMonth() + 1)}.${p(d.getFullYear() % 100)} ${p(d.getHours())}:${p(d.getMinutes())}`;
  }

  function fmtPerms(mode: number): string {
    const bits = "rwxrwxrwx";
    let out = "";
    for (let i = 0; i < 9; i++) {
      out += mode & (1 << (8 - i)) ? bits[i] : "-";
    }
    return out;
  }

  // Full-precision timestamp for the status bar: YYYY-MM-DD HH:MM:SS.
  function fmtDateTime(unixSeconds: number): string {
    if (!unixSeconds) return "—";
    const d = new Date(unixSeconds * 1000);
    const p = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
  }

  // Leading type char for the attribute string (drwxr-xr-x).
  function typeChar(kind: Entry["kind"]): string {
    switch (kind) {
      case "dir":
        return "d";
      case "symlink":
        return "l";
      case "special":
        return "s";
      default:
        return "-";
    }
  }

  // The full attribute + owner + size + dates string for the focused entry,
  // rendered in the bottom status bar. `..` gets no metadata line.
  function focusedInfo(p: PanelState): string {
    const e = p.entries[p.cursor_index];
    if (!e || e.name === "..") return "";
    const attrs = typeChar(e.kind) + fmtPerms(e.permissions);
    const owner = `${e.owner ?? e.uid}:${e.group ?? e.gid}`;
    const size =
      e.kind === "dir"
        ? e.computed_size != null
          ? bytesFmt(e.computed_size)
          : "---"
        : bytesFmt(e.size);
    const link = e.symlink_target ? ` → ${e.symlink_target}` : "";
    return `${attrs}  ${owner}  ${size}  mod ${fmtDateTime(e.modified)}  crt ${fmtDateTime(e.created)}${link}`;
  }

  // Mouse: a click anywhere in a panel — its padding, its header, a row — is a
  // request for the keyboard to be in that panel. Always reported, never guarded
  // on `snapshot.active`: the panel may already be active and still not hold the
  // keyboard, because the terminal prompt has it (§5.7).
  async function activatePanel(side: PanelId) {
    try {
      snapshot = await nav.setActivePanel(side);
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  // Mouse: single-click focuses the clicked entry (and activates its panel). The
  // core owns the cursor index; we just report the clicked global index.
  //
  // The activation is unconditional even when the panel is already active: it is
  // also what takes the keyboard back from the terminal prompt (§5.7), and the
  // core is the one that decides that. Skipping the call when only the *panel*
  // looks unchanged would be the frontend quietly overruling it.
  async function focusEntry(side: PanelId, index: number) {
    try {
      snapshot = await nav.setActivePanel(side);
      snapshot = await nav.setCursor(side, index);
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  // Mouse: double-click behaves like Enter — focus the entry, then navigate into
  // it (dir / symlink / `..`) or open a file with the system default (§5.5).
  async function openEntry(side: PanelId, index: number) {
    try {
      snapshot = await nav.setActivePanel(side);
      snapshot = await nav.setCursor(side, index);
      await activateFocused(side);
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  // --- Quick search (§5.9) --------------------------------------------------
  // The core owns the query, the matching and the accept/reject decision; these
  // forward the keystroke and render what comes back. The only thing decided
  // here is the feedback for a rejected character, which is presentation —
  // same category as the red background on a failure dialog.

  // Reused across beeps: creating an AudioContext per keystroke leaks them, and
  // browsers cap how many a page may hold. Created lazily on the first miss, by
  // which point a keydown has certainly happened, so it is never blocked by the
  // autoplay gesture requirement.
  let audioCtx: AudioContext | null = null;

  // The reject beep. Deliberately short and quiet: it fires on a mistyped
  // character, so it has to read as a nudge rather than an alarm.
  function beep() {
    try {
      audioCtx ??= new AudioContext();
      if (audioCtx.state === "suspended") void audioCtx.resume();
      const osc = audioCtx.createOscillator();
      const gain = audioCtx.createGain();
      const t = audioCtx.currentTime;
      osc.type = "sine";
      osc.frequency.value = 660;
      // Ramp down rather than stopping flat, which would click.
      gain.gain.setValueAtTime(0.06, t);
      gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.08);
      osc.connect(gain).connect(audioCtx.destination);
      osc.start(t);
      osc.stop(t + 0.09);
    } catch {
      // No audio device, or the context was refused. The box still flashes red,
      // so the miss is not silent in the sense that matters.
    }
  }

  async function searchStart(side: PanelId) {
    try {
      snapshot = await nav.searchStart(side);
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  // A rejected character comes back as a bumped `miss_rev` rather than an error,
  // so the beep is driven by comparing the counter across the call.
  async function searchPush(side: PanelId, text: string) {
    const before = snapshot[side].search?.miss_rev ?? 0;
    try {
      snapshot = await nav.searchPush(side, text);
      if ((snapshot[side].search?.miss_rev ?? 0) !== before) beep();
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  async function searchBackspace(side: PanelId) {
    try {
      snapshot = await nav.searchBackspace(side);
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  async function searchClose(side: PanelId) {
    try {
      snapshot = await nav.searchClose(side);
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  function joinPath(base: string, name: string): string {
    return base.endsWith("/") ? base + name : `${base}/${name}`;
  }

  // F3: on a file, open the viewer. On a folder, recursively calculate its size
  // (and every currently selected folder) instead — the result shows in the
  // status bar and folds into the selection total. `..` is ignored.
  async function viewOrCalcSize(side: PanelId) {
    const p = snapshot[side];
    const focused = p.entries[p.cursor_index];
    if (focused && focused.kind === "dir" && focused.name !== "..") {
      const paths = new Set<string>([joinPath(p.path, focused.name)]);
      for (const i of p.selection) {
        const e = p.entries[i];
        if (e && e.kind === "dir" && e.name !== "..") paths.add(joinPath(p.path, e.name));
      }
      const prev = status;
      status = "Calculating…";
      try {
        snapshot = await nav.calculateDirSize([...paths]);
        status = prev;
      } catch (err) {
        status = `error: ${errMessage(err)}`;
      }
      return;
    }
    receiveOpen(await open.openEntry(side, "view"));
  }

  // Enter / double-click behavior on the focused entry: directories, symlinks,
  // and `..` navigate; any other entry opens with the system default. The core
  // no-ops `navigate` on files and `open_entry` on dirs, so this split is clean.
  async function activateFocused(side: PanelId) {
    const p = snapshot[side];
    const entry = p.entries[p.cursor_index];
    if (!entry || entry.name === ".." || entry.kind === "dir" || entry.kind === "symlink") {
      snapshot = await nav.navigate(side, { kind: "into" });
    } else {
      await open.openEntry(side, "open");
    }
  }

  // Open the F5/F6 destination prompt, pre-filled with the *inactive* panel's
  // directory — the natural other-panel target (§5.4a).
  function openDestPrompt(op: OpKind) {
    const other = snapshot.active === "left" ? "right" : "left";
    const p = snapshot[snapshot.active];
    const hasSources = p.selection.length > 0 || (p.entries[p.cursor_index]?.name ?? "..") !== "..";
    if (!hasSources) return; // nothing to act on (e.g. cursor on `..`)
    destPrompt = { op: op, value: snapshot[other].path };
  }

  async function confirmDest() {
    if (!destPrompt) return;
    const { op, value } = destPrompt;
    destPrompt = null;
    try {
      const opId = await ops.startTransfer(op, value);
      // If the op already completed while we were awaiting (fast single-item
      // ops emit + finish before this resolves), do NOT open a phantom modal.
      if (opId !== finishedOpId) {
        activeOp = { id: opId, done: 0, total: 0, current: "" };
      }
    } catch (err) {
      status = `error: ${String(err)}`;
    }
  }

  function cancelDest() {
    destPrompt = null;
  }

  function errMessage(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  // --- Delete (F8) ----------------------------------------------------------
  // Open the delete confirmation, describing what the op will act on (the active
  // panel's selection, else the entry under the cursor; never `..`).
  function openDelete() {
    const p = snapshot[snapshot.active];
    const selCount = p.selection.length;
    const cursorName = p.entries[p.cursor_index]?.name ?? "..";
    if (selCount === 0 && cursorName === "..") return; // nothing to delete
    const label =
      selCount > 0 ? `${selCount} selected item${selCount > 1 ? "s" : ""}` : cursorName;
    deleteConfirm = { label };
  }

  async function confirmDelete() {
    deleteConfirm = null;
    try {
      const opId = await ops.startDelete();
      // Same fast-op guard as confirmDest: a quick delete can finish before this
      // resolves, so don't resurrect a phantom progress modal.
      if (opId !== finishedOpId) {
        activeOp = { id: opId, done: 0, total: 0, current: "" };
      }
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  // Toggle the persisted "Move to Trash" default (core is the source of truth).
  async function toggleTrash() {
    try {
      snapshot = await nav.setTrashDefault(!snapshot.trash_default);
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  // --- Create directory (F7) / Rename (Shift+F6) ----------------------------
  function openMkdir() {
    textPrompt = { kind: "mkdir", value: "" };
  }

  function openRename() {
    const p = snapshot[snapshot.active];
    const name = p.entries[p.cursor_index]?.name ?? "..";
    if (name === "..") return; // `..` cannot be renamed
    textPrompt = { kind: "rename", value: name };
  }

  async function confirmText() {
    if (!textPrompt) return;
    const { kind, value } = textPrompt;
    // Viewer prompts act on the open session rather than the panels.
    if (kind === "search") {
      textPrompt = null;
      await runSearch(value.trim(), "forward");
      return;
    }
    if (kind === "goto") {
      const target = parseGoto(value);
      if (!target) {
        textPrompt = { kind, value, error: "Enter a line, 0x offset, or percentage" };
        return;
      }
      textPrompt = null;
      if (viewerPage) await viewerDo(() => viewer.goto(viewerPage!.id, target));
      return;
    }
    try {
      snapshot =
        kind === "mkdir"
          ? await nav.createDir(snapshot.active, value)
          : await nav.rename(snapshot.active, value);
      textPrompt = null;
    } catch (err) {
      // Keep the prompt open with an inline error so the user can retype.
      textPrompt = { kind, value, error: errMessage(err) };
    }
  }

  function cancelText() {
    textPrompt = null;
  }

  // Focus a rename input and preselect the base name (extension excluded), so a
  // quick retype changes the name but keeps the extension.
  function renameFocus(node: HTMLInputElement) {
    node.focus();
    const dot = node.value.lastIndexOf(".");
    if (dot > 0) node.setSelectionRange(0, dot);
    else node.select();
  }

  async function answerCollision(resolution: Resolution) {
    if (prompt?.kind !== "collision") return;
    const opId = prompt.opId;
    prompt = null;
    try {
      await ops.resolveCollision(opId, resolution);
    } catch (err) {
      status = `error: ${String(err)}`;
    }
  }

  async function answerError(resolution: ErrorResolution) {
    if (prompt?.kind !== "error") return;
    const opId = prompt.opId;
    prompt = null;
    try {
      await ops.resolveError(opId, resolution);
    } catch (err) {
      status = `error: ${String(err)}`;
    }
  }

  async function cancelActiveOp() {
    if (!activeOp) return;
    try {
      await ops.cancelOp(activeOp.id);
    } catch (err) {
      status = `error: ${String(err)}`;
    }
  }

  // --- Embedded terminal (§5.7) ---------------------------------------------
  // Every one of these is a core call that returns the whole snapshot; nothing
  // about the command line is decided here.

  async function termDo(fn: () => Promise<AppSnapshot>) {
    try {
      snapshot = await fn();
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  // Re-pull the whole buffer. Cheap and always correct — used on mount and
  // whenever the pane opens, since output accumulates while it is collapsed.
  async function syncTerminalBuffer() {
    try {
      const buf = await terminal.buffer();
      termLines = buf.lines;
      termPending = buf.pending;
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  async function toggleTerminalSize() {
    const wasCollapsed = snapshot.terminal.size === "collapsed";
    await termDo(() => terminal.toggleHalf());
    // Opening the pane reveals output produced while it was shut.
    if (wasCollapsed) await syncTerminalBuffer();
    await tick();
    await measureAll(); // the panels just changed height
  }

  async function toggleTerminalCurtain() {
    const wasCollapsed = snapshot.terminal.size === "collapsed";
    await termDo(() => terminal.toggleCurtain());
    if (wasCollapsed) await syncTerminalBuffer();
    await tick();
    await measureAll();
  }

  // The terminal actions reachable from the panels (§5.7): Cmd+T, Cmd+Shift+T,
  // the Esc curtain, and Ctrl+Enter's filename insertion — which deliberately
  // leaves focus where it is, so the user can keep moving the cursor and pressing
  // it to build up a multi-file command.
  async function runPanelTerminalAction(action: string) {
    switch (action) {
      case "terminal.focus":
        await termDo(() => terminal.toggleFocus());
        break;
      case "terminal.toggle_half":
        await toggleTerminalSize();
        break;
      case "terminal.curtain":
        await toggleTerminalCurtain();
        break;
      case "terminal.insert_name":
        await termDo(() => terminal.insertName(snapshot.active));
        break;
    }
  }

  // Keys while the prompt owns the keyboard. Returns true when the key was a
  // bound command; anything else is text the user is typing and must reach the
  // input untouched.
  function handleTerminalKey(action: string): boolean {
    switch (action) {
      case "terminal.run":
        void termDo(() => terminal.run());
        return true;
      case "terminal.blur":
        void termDo(() => terminal.toggleFocus());
        return true;
      case "terminal.toggle_half":
        void toggleTerminalSize();
        return true;
      case "terminal.curtain":
        void toggleTerminalCurtain();
        return true;
      case "terminal.interrupt":
        void termDo(() => terminal.interruptOrClear());
        return true;
      case "terminal.history_prev":
      case "terminal.history_next":
        void termDo(() =>
          terminal.history(
            (action === "terminal.history_prev" ? "prev" : "next") as HistoryDir,
          ),
        );
        return true;
      case "terminal.scroll_up":
        terminalRef?.scrollByPage(-1);
        return true;
      case "terminal.scroll_down":
        terminalRef?.scrollByPage(1);
        return true;
      case "terminal.clear_buffer":
        void termDo(() => terminal.clearBuffer());
        return true;
    }
    return false;
  }

  // --- Embedded viewer / editor (§5.5) --------------------------------------

  // Route what `open_entry` decided into the right surface. External launches
  // and executable runs need nothing here — the core already did them.
  function receiveOpen(outcome: OpenOutcome) {
    overlayMessage = "";
    if (outcome.kind === "viewer") {
      viewerPage = outcome.value;
      editorDoc = null;
    } else if (outcome.kind === "editor") {
      openEditor(outcome.value);
    }
  }

  function openEditor(doc: EditDoc) {
    editorDoc = doc;
    editorText = doc.text;
    editorSaved = doc.text;
    viewerPage = null;
    saveConflict = null;
  }

  // Every viewer command returns the freshly rendered page, so the whole
  // frontend job is to hand the result back to the renderer.
  async function viewerDo(fn: () => Promise<ViewPage>) {
    try {
      overlayMessage = "";
      viewerPage = await fn();
    } catch (err) {
      overlayMessage = errMessage(err);
    }
  }

  async function runSearch(needle: string, direction: SearchDirection) {
    if (!viewerPage || !needle) return;
    lastSearch = needle;
    try {
      const page = await viewer.search(viewerPage.id, needle, direction);
      // `null` is "not found" — a status-line message, not an error dialog.
      if (page) {
        viewerPage = page;
        overlayMessage = "";
      } else {
        overlayMessage = `"${needle}" not found`;
      }
    } catch (err) {
      overlayMessage = errMessage(err);
    }
  }

  // Parse the Goto (F5) input the way FAR does: a bare number is a line, a `%`
  // suffix a percentage, and a `0x`/`$` prefix a byte offset.
  function parseGoto(input: string): GotoTarget | null {
    const text = input.trim();
    if (!text) return null;
    if (text.endsWith("%")) {
      const pct = Number(text.slice(0, -1));
      return Number.isFinite(pct) ? { kind: "percent", value: Math.max(0, Math.min(100, pct)) } : null;
    }
    if (/^(0x|\$)/i.test(text)) {
      const offset = Number.parseInt(text.replace(/^\$/, "0x"), 16);
      return Number.isFinite(offset) ? { kind: "offset", value: offset } : null;
    }
    const line = Number(text);
    return Number.isFinite(line) && line > 0 ? { kind: "line", value: Math.floor(line) } : null;
  }

  async function closeViewer() {
    const page = viewerPage;
    viewerPage = null;
    overlayMessage = "";
    if (page) await viewer.close(page.id).catch(() => {});

  }

  // F6 in the viewer: the core hands the same file to the editor, so the
  // frontend never names a file it wants opened for writing.
  async function viewerToEdit() {
    if (!viewerPage) return;
    try {
      openEditor(await viewer.toEdit(viewerPage.id));
    } catch (err) {
      overlayMessage = errMessage(err);
    }
  }

  async function saveEditor(force = false) {
    if (!editorDoc) return;
    try {
      const outcome = await editor.save(editorDoc.id, editorText, force);
      if (outcome.kind === "saved") {
        editorSaved = editorText;
        saveConflict = null;
        overlayMessage = "Saved";
        await refreshBoth();
      } else if (outcome.kind === "conflict") {
        saveConflict = outcome.value;
      } else if (outcome.kind === "read_only") {
        overlayMessage = "This file is read-only";
      } else {
        overlayMessage = outcome.value;
      }
    } catch (err) {
      overlayMessage = errMessage(err);
    }
  }

  // Esc in the editor. A dirty buffer asks first — this is the one place the
  // app can lose the user's typing.
  async function closeEditor(discard = false) {
    if (!editorDoc) return;
    if (editorDirty && !discard) {
      deleteConfirm = null;
      unsavedPrompt = true;
      return;
    }
    const id = editorDoc.id;
    editorDoc = null;
    unsavedPrompt = false;
    saveConflict = null;
    overlayMessage = "";
    await editor.close(id).catch(() => {});
    await refreshBoth();
  }

  async function editorToView() {
    if (!editorDoc) return;
    if (editorDirty) {
      overlayMessage = "Save (F2) before switching to the viewer";
      return;
    }
    try {
      const id = editorDoc.id;
      editorDoc = null;
      viewerPage = await editor.toView(id);
    } catch (err) {
      overlayMessage = errMessage(err);
    }
  }

  // Keys while the viewer is open. Everything it does is a core call that
  // returns a new page; nothing about the file is decided here.
  function handleViewerKey(action: string): boolean {
    if (!viewerPage) return false;
    const id = viewerPage.id;
    const motions: Record<string, ViewMotion> = {
      "viewer.line_up": "line_up",
      "viewer.line_down": "line_down",
      "viewer.page_up": "page_up",
      "viewer.page_down": "page_down",
      "viewer.home": "home",
      "viewer.end": "end",
      "viewer.col_left": "col_left",
      "viewer.col_right": "col_right",
    };
    const motion = motions[action];
    if (motion) {
      void viewerDo(() => viewer.scroll(id, motion));
      return true;
    }
    switch (action) {
      case "viewer.close":
        void closeViewer();
        return true;
      case "viewer.toggle_hex":
        void viewerDo(() => viewer.toggleHex(id));
        return true;
      case "viewer.toggle_wrap":
        void viewerDo(() => viewer.setWrap(id, !viewerPage!.wrap));
        return true;
      case "viewer.to_edit":
        void viewerToEdit();
        return true;
      case "viewer.goto":
        textPrompt = { kind: "goto", value: "" };
        return true;
      case "viewer.search":
        textPrompt = { kind: "search", value: lastSearch };
        return true;
      case "viewer.search_next":
        void runSearch(lastSearch, "forward");
        return true;
    }
    return false;
  }

  function handleEditorKey(action: string): boolean {
    switch (action) {
      case "editor.save":
        void saveEditor();
        return true;
      case "editor.to_view":
        void editorToView();
        return true;
      case "editor.close":
        void closeEditor();
        return true;
    }
    return false;
  }

  // --- Help (F1, §6) --------------------------------------------------------

  /**
   * Fetch the book for the current query. The core does the filtering; this only
   * drops responses that arrived out of order, so fast typing can't leave the
   * list showing the results of a prefix the user has already moved past.
   */
  async function loadHelp() {
    const q = helpQuery;
    try {
      const book = await help.book(q);
      if (helpOpen && helpQuery === q) helpBook = book;
    } catch (err) {
      status = `error: ${String(err)}`;
    }
  }

  async function openHelp() {
    // Help is the one surface reached without a snapshot-returning command, so
    // it is also the one place an open quick-search box has to be closed by
    // hand — everything else gets it from `snapshot_after_input` (§5.9). No race:
    // nothing else is in flight for this keypress.
    const side = snapshot.active;
    if (snapshot[side].search) await searchClose(side);
    helpOpen = true;
    helpTopic = 0;
    helpQuery = "";
    await loadHelp();
  }

  function closeHelp() {
    helpOpen = false;
    helpQuery = "";
    // Hand the DOM focus back to whoever had it. The core's idea of who owns the
    // keyboard never changed while help was up, so nothing else will do this —
    // skip it and the editor or the prompt goes quietly dead.
    tick().then(() => {
      if (editorDoc) editorRef?.refocus();
      else if (snapshot.terminal.focused) terminalRef?.focusInput();
    });
  }

  /** Scroll the topic pane. The list is short, so this stays view-side state. */
  function scrollHelp(amount: number) {
    helpContentEl?.scrollBy({ top: amount });
  }

  /**
   * Open an About link in the OS browser. A failure here is not worth a dialog —
   * the user pressed a link, not started an operation — so it degrades to a
   * console warning and the help screen stays exactly as it was.
   */
  async function openAboutLink(url: string) {
    try {
      await help.openLink(url);
    } catch (e) {
      console.warn(`could not open ${url}:`, e);
    }
  }

  /**
   * Install the pending update and relaunch. The button is replaced by a status
   * line for the duration: the download can take a while, and a second press
   * would start a second download.
   */
  async function installUpdate() {
    if (updateInstalling) return;
    updateInstalling = true;
    updateError = "";
    try {
      await updates.install();
      // Not reached — the backend relaunches the app on success.
    } catch (e) {
      updateError = String(e);
      updateInstalling = false;
    }
  }

  function handleHelpKey(action: string): boolean {
    const count = helpBook?.topics.length ?? 0;
    switch (action) {
      case "help.close":
        closeHelp();
        return true;
      case "help.next_topic":
        if (count) helpTopic = (helpTopic + 1) % count;
        return true;
      case "help.prev_topic":
        if (count) helpTopic = (helpTopic - 1 + count) % count;
        return true;
      case "help.scroll_up":
        scrollHelp(-ROW_H * 3);
        return true;
      case "help.scroll_down":
        scrollHelp(ROW_H * 3);
        return true;
      case "help.page_up":
        scrollHelp(-(helpContentEl?.clientHeight ?? 0) * 0.9);
        return true;
      case "help.page_down":
        scrollHelp((helpContentEl?.clientHeight ?? 0) * 0.9);
        return true;
    }
    return false;
  }

  // Keys for the two dialogs the editor can raise: a save conflict and an
  // unsaved-changes confirmation.
  function handleOverlayDialogKey(e: KeyboardEvent): boolean {
    if (saveConflict) {
      switch (e.key) {
        case "o": case "O": saveConflict = null; void saveEditor(true); return true;
        case "Escape": case "c": case "C": saveConflict = null; return true;
      }
      return false;
    }
    if (unsavedPrompt) {
      switch (e.key) {
        case "s": case "S": unsavedPrompt = false; void saveEditor().then(() => closeEditor(true)); return true;
        case "d": case "D": void closeEditor(true); return true;
        case "Escape": case "c": case "C": unsavedPrompt = false; return true;
      }
      return false;
    }
    return false;
  }

  // Keyboard handling while a (non-text) op modal is open. Returns true only when
  // the key maps to a modal action, so the caller can preventDefault just those.
  function handleModalKey(e: KeyboardEvent): boolean {
    if (prompt?.kind === "collision") {
      switch (e.key) {
        case "s": case "S": void answerCollision("skip"); return true;
        case "a": case "A": if (prompt.multiple) { void answerCollision("skip_all"); return true; } return false;
        case "o": case "O": void answerCollision("overwrite"); return true;
        case "l": case "L": if (prompt.multiple) { void answerCollision("overwrite_all"); return true; } return false;
        case "Escape": void answerCollision("cancel"); return true;
      }
      return false;
    }
    if (prompt?.kind === "error") {
      switch (e.key) {
        case "r": case "R": void answerError("retry"); return true;
        case "s": case "S": void answerError("skip"); return true;
        case "a": case "A": void answerError("skip_all"); return true;
        case "e": case "E": if (prompt.offerElevate) { void answerError("elevate"); return true; } return false;
        case "Escape": void answerError("cancel"); return true;
      }
      return false;
    }
    if (deleteConfirm) {
      switch (e.key) {
        case "Enter": case "d": case "D": void confirmDelete(); return true;
        case "t": case "T": void toggleTrash(); return true;
        case "Escape": deleteConfirm = null; return true;
      }
      return false;
    }
    if (activeOp) {
      if (e.key === "Escape") { void cancelActiveOp(); return true; }
      return false;
    }
    return false;
  }

  // While the quick-search box is open it takes the plain characters, Backspace,
  // and its two exits; everything else falls through to the normal panel
  // dispatch (§5.9). Returns true when the key was consumed.
  //
  // The fall-through arm deliberately issues NO cancel call of its own: the core
  // ends the search inside `snapshot_after_input`, which whatever command runs
  // next returns through. A cancel fired from here would be a second IPC call
  // racing that command's snapshot, and the stale one could land last.
  function searchKeydown(e: KeyboardEvent): boolean {
    if (keyContext !== "panels") return false;
    const side = snapshot.active;
    if (!snapshot[side].search) return false;

    // preventDefault has to happen synchronously — after an await the event has
    // already been dispatched and preventing it does nothing (same reason the
    // panels dispatch below calls it before its first await).
    const bound = keymaps.search?.[chord(e)];
    if (bound === "search.close") {
      e.preventDefault();
      void searchClose(side);
      return true;
    }
    if (bound === "search.backspace") {
      e.preventDefault();
      void searchBackspace(side);
      return true;
    }
    // A printable character is query text. Shift is allowed through because it
    // is already baked into the character; Ctrl/Meta/Alt combos are commands and
    // must reach the dispatch below.
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      void searchPush(side, e.key);
      return true;
    }
    return false;
  }

  async function onKeydown(e: KeyboardEvent) {
    shiftHeld = e.shiftKey;
    // Help owns the keyboard outright while it is up — it can be opened over any
    // surface, so nothing underneath may act on a key. Unbound keys fall through
    // untouched to the focused search field, exactly as they do at the terminal
    // prompt; that is what lets the user simply type to filter.
    if (helpOpen) {
      const bound = keymaps.help?.[chord(e)];
      if (bound && handleHelpKey(bound)) e.preventDefault();
      return;
    }
    // The quick-search box, when open, owns plain typing (§5.9). Ahead of F1 so
    // that Backspace and Escape reach the box, but only ever consuming keys it
    // has a use for — F1 itself is not one, so help still opens over a search.
    if (searchKeydown(e)) return;
    // F1 from any surface (§6). Handled here rather than in each surface's own
    // handler so viewer/editor/terminal need to know nothing about help. The
    // modals are the deliberate exception: they are questions awaiting an answer.
    if (!modalUp && keymaps[keyContext]?.[chord(e)] === "help.open") {
      e.preventDefault();
      void openHelp();
      return;
    }
    // Text prompts (destination, mkdir, rename, search, goto) own the keyboard
    // via their focused <input>; let them be.
    if (destPrompt || textPrompt) return;
    // The overlay dialogs the viewer/editor can raise answer for themselves.
    if (saveConflict || unsavedPrompt) {
      if (e.metaKey || e.ctrlKey) return;
      if (handleOverlayDialogKey(e)) e.preventDefault();
      return;
    }
    // The terminal prompt owns the keyboard while it is focused (§5.7). Unlike
    // the viewer/editor branch below, it must NOT bail out on Cmd/Ctrl before
    // consulting the keymap — Cmd+T and Ctrl+C are its two most important
    // bindings. Unbound combos still fall through, so Cmd+A/Cmd+V keep working
    // in the input, and unbound plain keys are simply the user typing.
    if (keyContext === "terminal") {
      const bound = keymaps.terminal?.[chord(e)];
      if (bound && handleTerminalKey(bound)) e.preventDefault();
      return;
    }
    // The embedded surfaces own the keyboard while they are open (§6). In the
    // editor, anything not bound to a command is text the user is typing, so it
    // must fall through to the <textarea> untouched.
    if (keyContext !== "panels") {
      if (e.metaKey || e.ctrlKey) return;
      const bound = keymaps[keyContext]?.[chord(e)];
      if (!bound) {
        // The viewer has no text entry, so unbound keys do nothing there.
        if (keyContext === "viewer") e.preventDefault();
        return;
      }
      const handled =
        keyContext === "viewer" ? handleViewerKey(bound) : handleEditorKey(bound);
      if (handled) e.preventDefault();
      return;
    }
    // Other op modals consume their own keys and otherwise block navigation —
    // but must let OS/browser shortcuts (Cmd/Ctrl combos, e.g. Cmd+Q) through.
    // The terminal keys are the exception: Cmd+T must reach the prompt even with
    // a dialog up, or a modal could strand the keyboard.
    if (activeOp || prompt || deleteConfirm) {
      if (e.metaKey || e.ctrlKey) {
        const bound = keymaps.panels?.[chord(e)];
        if (bound?.startsWith("terminal.")) {
          e.preventDefault();
          void runPanelTerminalAction(bound);
        }
        return;
      }
      if (handleModalKey(e)) e.preventDefault();
      return;
    }

    const action = keymaps.panels?.[chord(e)];
    if (!action) return;
    e.preventDefault();
    const active = snapshot.active;
    try {
      if (action === "op.copy") {
        openDestPrompt("copy");
      } else if (action === "op.move") {
        openDestPrompt("move");
      } else if (action === "op.delete") {
        openDelete();
      } else if (action === "op.mkdir") {
        openMkdir();
      } else if (action === "op.rename") {
        openRename();
      } else if (action === "open.view") {
        await viewOrCalcSize(active);
      } else if (action === "open.edit") {
        receiveOpen(await open.openEntry(active, "edit"));
      } else if (action.startsWith("cursor.")) {
        snapshot = await nav.moveCursor(active, action.slice("cursor.".length) as Motion);
      } else if (action.startsWith("select.")) {
        snapshot = await nav.selectAndMove(active, action.slice("select.".length) as Motion);
      } else if (action === "selection.toggle") {
        snapshot = await nav.toggleSelection(active);
      } else if (action === "selection.all") {
        snapshot = await nav.selectAll(active);
      } else if (action === "selection.none") {
        snapshot = await nav.deselectAll(active);
      } else if (action === "panel.switch") {
        snapshot = await nav.setActivePanel(active === "left" ? "right" : "left");
      } else if (action === "nav.enter") {
        await activateFocused(active);
      } else if (action === "nav.parent") {
        snapshot = await nav.navigate(active, { kind: "parent" });
      } else if (action === "search.start") {
        await searchStart(active);
      } else if (action === "panel.toggle_hidden") {
        await setHidden(active, !snapshot[active].show_hidden);
      } else if (action === "panel.cycle_sort") {
        await cycleSort(active);
      } else if (action.startsWith("panel.view_")) {
        const which = action.slice("panel.view_".length);
        await setView(active, which === "detailed" ? { kind: "detailed" } : { kind: "columns", columns: Number(which) });
      } else if (action.startsWith("terminal.")) {
        await runPanelTerminalAction(action);
      }
    } catch (err) {
      status = `error: ${String(err)}`;
    }
  }

  // Re-read both panels after an op so their listings reflect the changes.
  async function refreshBoth() {
    try {
      snapshot = await nav.refresh("left");
      snapshot = await nav.refresh("right");
    } catch (err) {
      status = `error: ${String(err)}`;
    }
  }

  onMount(() => {
    let ro: ResizeObserver | undefined;
    const unlisten: UnlistenFn[] = [];
    (async () => {
      try {
        keymaps = buildKeymap(await nav.getKeymap());
        snapshot = await nav.init();

        // Fire-and-forget: the panels must not wait on a network round trip to
        // paint. The backend already swallows offline and not-yet-published
        // feeds, so there is nothing here worth reporting.
        void updates
          .check()
          .then((info) => (pendingUpdate = info))
          .catch(() => {});

        await tick();
        await measureAll();
        ro = new ResizeObserver(() => void measureAll());
        if (listingEls.left) ro.observe(listingEls.left);
        if (listingEls.right) ro.observe(listingEls.right);

        // Operation lifecycle events (§5.4a). One op runs at a time in this UI.
        unlisten.push(
          await events.opProgressEvent.listen((e) => {
            const p = e.payload;
            // Ignore a progress tick that arrives after the op already completed
            // (event delivery to the webview is not strictly ordered).
            if (p.op_id === finishedOpId) return;
            activeOp = {
              id: p.op_id,
              done: p.count_done,
              total: p.count_total,
              current: p.current,
            };
          }),
        );
        unlisten.push(
          await events.opCollisionEvent.listen((e) => {
            const p = e.payload;
            prompt = { kind: "collision", opId: p.op_id, path: p.path, multiple: p.multiple };
          }),
        );
        unlisten.push(
          await events.opErrorEvent.listen((e) => {
            const p = e.payload;
            prompt = { kind: "error", opId: p.op_id, path: p.path, reason: p.reason, offerElevate: p.offer_elevate };
          }),
        );
        unlisten.push(
          await events.opCompleteEvent.listen((e) => {
            const o = e.payload;
            finishedOpId = o.op_id;
            if (activeOp && activeOp.id !== o.op_id) return;
            activeOp = null;
            prompt = null;
            status = o.summary;
            void refreshBoth();
          }),
        );

        // Terminal scrollback (§5.7). The payload is a line delta: apply the
        // core's eviction count, then append. This mirror holds no policy of its
        // own, which is what keeps it from drifting from the core's buffer.
        unlisten.push(
          await events.terminalChunkEvent.listen((e) => {
            const c = e.payload;
            // Append THEN drop — the order is the contract (see TerminalChunk).
            // The core trims after appending, so one burst into a small buffer
            // can evict lines it just added; dropping first would clamp at zero
            // and leave the mirror permanently longer than the core's buffer.
            const next = c.lines.length ? [...termLines, ...c.lines] : termLines;
            termLines = c.dropped > 0 ? next.slice(c.dropped) : next;
            termPending = c.pending;
          }),
        );
        // The backend changing the terminal outside a command we issued — i.e. a
        // run finished, so the indicator must change colour.
        unlisten.push(
          await events.terminalStateEvent.listen((e) => {
            snapshot = { ...snapshot, terminal: e.payload };
          }),
        );
        await syncTerminalBuffer();

        status = "ready";
      } catch (err) {
        status = `failed to start: ${String(err)}`;
      }
    })();

    const onKeyup = (e: KeyboardEvent) => (shiftHeld = e.shiftKey);
    const onBlur = () => (shiftHeld = false);
    window.addEventListener("keydown", onKeydown);
    window.addEventListener("keyup", onKeyup);
    window.addEventListener("blur", onBlur);
    return () => {
      ro?.disconnect();
      window.removeEventListener("keydown", onKeydown);
      window.removeEventListener("keyup", onKeyup);
      window.removeEventListener("blur", onBlur);
      for (const u of unlisten) u();
    };
  });

  // Full name of the entry under the active panel's cursor, for the status bar.
  const focused = $derived.by(() => {
    const p = snapshot[snapshot.active];
    return p.entries[p.cursor_index]?.name ?? "";
  });

  // Rich metadata line for the focused entry (attributes, owner:group, exact
  // byte size, modified/created datetimes), shown beside the name.
  const focusedMeta = $derived.by(() => focusedInfo(snapshot[snapshot.active]));

  // Function-key hints. While Shift is held, F6 advertises its shifted action
  // (Rename) instead of Move, so the bar reflects what the next keystroke does.
  const fkeys = $derived<[string, string][]>([
    ["F1", "Help"],
    ["F3", "View"],
    ["F4", "Edit"],
    ["F5", "Copy"],
    shiftHeld ? ["⇧F6", "Rename"] : ["F6", "Move"],
    ["F7", "MkDir"],
    ["F8", "Delete"],
    ["Space", "Select"],
    ["*", "All"],
    ["Tab", "Switch"],
    ["Enter", "Open"],
    ["⌫", "Up"],
    // The terminal is invisible until you know it is there (§5.7).
    ["⌘T", "Term"],
    [snapshot.terminal.size === "collapsed" ? "⌘⇧T" : "⌘⇧T", snapshot.terminal.size === "collapsed" ? "Output" : "Hide"],
  ]);
</script>

<main class="app">
  <!-- The Esc curtain hides the panels entirely, revealing the full-height
       terminal beneath them (SPEC §6). At every other size the terminal takes
       its share below and the panels shrink to fit — pushed up, never covered. -->
  <div class="panels" class:hidden={snapshot.terminal.size === "full"}>
    {#each SIDES as side}
      {@const p = snapshot[side]}
      {@const L = layout(p)}
      <section
        class="panel"
        class:active={snapshot.active === side}
        class:dimmed={snapshot.terminal.focused}
        onclick={() => void activatePanel(side)}
        role="presentation"
      >
        <header class="panel-head">
          <span class="path" title={p.path}>{p.path || "…"}</span>
          <!-- Per-panel view controls (§5.8). Clicks are stopped so they never
               reach the panel's activate handler. -->
          <span class="panel-ctls" role="presentation" onclick={(e) => e.stopPropagation()}>
            <button
              class="ctl toggle"
              class:on={p.show_hidden}
              title="Hidden files ({p.show_hidden ? "shown" : "hidden"}) — Ctrl+H"
              aria-pressed={p.show_hidden}
              onclick={() => void setHidden(side, !p.show_hidden)}
            >.*</button>
            <select
              class="ctl"
              title="Sort order — Ctrl+S cycles"
              value={p.sort_mode}
              onchange={(e) => void setSort(side, e.currentTarget.value as SortMode)}
            >
              {#each SORTS as s}<option value={s.id}>{s.label}</option>{/each}
            </select>
            <select
              class="ctl"
              title="View mode — Ctrl+1/2/3/4"
              value={viewId(p.view_mode)}
              onchange={(e) => void setView(side, VIEWS.find((v) => v.id === e.currentTarget.value)!.mode)}
            >
              {#each VIEWS as v}<option value={v.id}>{v.label}</option>{/each}
            </select>
          </span>
        </header>

        {#if p.view_mode.kind === "detailed"}
          <div class="listing detailed" bind:this={listingEls[side]}>
            {#each L.pageEntries as entry, i}
              {@const gi = L.pageStart + i}
              <div
                class="row detail-row kind-{entry.kind} {entryClass(entry)}"
                class:cursor={gi === p.cursor_index}
                class:inactive={snapshot.active !== side}
                class:selected={p.selection.includes(gi)}
                class:denied={entry.marker === "denied"}
                role="presentation"
                onclick={(e) => { e.stopPropagation(); void focusEntry(side, gi); }}
                ondblclick={(e) => { e.stopPropagation(); void openEntry(side, gi); }}
              >
                <span class="d-name">{label(entry)}</span>
                <span class="d-size">{entry.kind === "dir" ? "<DIR>" : humanSize(entry.size)}</span>
                <span class="d-date">{fmtDate(entry.modified)}</span>
                <span class="d-perms">{entry.name === ".." ? "" : fmtPerms(entry.permissions)}</span>
              </div>
            {/each}
          </div>
        {:else}
          <div class="listing" bind:this={listingEls[side]} style={gridStyle(L.cols, L.rows)}>
            {#each L.pageEntries as entry, i}
              {@const gi = L.pageStart + i}
              <div
                class="row kind-{entry.kind} {entryClass(entry)}"
                class:cursor={gi === p.cursor_index}
                class:inactive={snapshot.active !== side}
                class:selected={p.selection.includes(gi)}
                class:denied={entry.marker === "denied"}
                role="presentation"
                onclick={(e) => { e.stopPropagation(); void focusEntry(side, gi); }}
                ondblclick={(e) => { e.stopPropagation(); void openEntry(side, gi); }}
              >
                {label(entry)}
              </div>
            {/each}
          </div>
        {/if}

        <footer class="panel-foot">
          <span>
            {p.selection.length} selected{p.selection.length ? ` · ${bytesFmt(selectedSize(p))}` : ""}
          </span>
          <span>{p.entries.length ? p.cursor_index + 1 : 0} / {p.entries.length}</span>
        </footer>

        <!-- Quick search (§5.9), in the panel's top-right corner over the view
             controls. The query is core-authored text, not an <input>: the core
             rejects a character that matches nothing, and a focused field would
             paint it before the core could withdraw it. The `{#key}` remounts the
             box on each miss so the flash animation retriggers. -->
        {#if p.search}
          {#key p.search.miss_rev}
            <div
              class="quick-search"
              class:miss={p.search.miss_rev > 0}
              role="presentation"
              onclick={(e) => e.stopPropagation()}
            >
              <span class="qs-label">/</span>
              <span class="qs-text">{p.search.query}</span>
              <button
                class="ctl qs-close"
                title="Cancel search — Esc"
                onclick={() => void searchClose(side)}>✕</button>
            </div>
          {/key}
        {/if}
      </section>
    {/each}
  </div>

  <!-- The focused entry, directly beneath the panel it describes. The curtain
       takes the panels away, so this goes with them — it would otherwise be left
       at the top of the window narrating a cursor nobody can see. -->
  {#if snapshot.terminal.size !== "full"}
    <div class="statusbar">
      <span class="focused-group">
        <span class="focused">{focused}</span>
        {#if focusedMeta}
          <span class="focused-meta">{focusedMeta}</span>
        {/if}
      </span>
      <span class="state">{status}</span>
    </div>
  {/if}

  <!-- The command line, beneath the panels and the entry they point at (§5.7).
       Expanded, its output pane grows upward from here and the panels give up
       the room. -->
  <Terminal
    bind:this={terminalRef}
    term={snapshot.terminal}
    lines={termLines}
    pending={termPending}
    onInput={(text) => void terminal.setInput(text).catch(() => {})}
    onFocus={() => void termDo(() => terminal.toggleFocus())}
    onScrollback={(bytes) => void termDo(() => terminal.setScrollback(bytes))}
    onClear={() => void termDo(() => terminal.clearBuffer())}
  />

  <nav class="fkeys" aria-label="function keys">
    {#each fkeys as [key, name]}
      <span class="fkey"><b>{key}</b> {name}</span>
    {/each}
  </nav>

  <!-- File-operation dialogs (§5.4a). Rendered top-most; only one shows at a
       time, in priority order: destination prompt → collision/error → progress. -->
  {#if destPrompt}
    <div class="overlay" role="presentation">
      <div class="dialog">
        <h2>{destPrompt.op === "copy" ? "Copy" : "Move"} to:</h2>
        <input
          class="dest-input"
          type="text"
          bind:value={destPrompt.value}
          use:autofocus
          onkeydown={(e) => {
            if (e.key === "Enter") { e.preventDefault(); void confirmDest(); }
            else if (e.key === "Escape") { e.preventDefault(); cancelDest(); }
          }}
        />
        <div class="buttons">
          <button onclick={() => void confirmDest()}>{destPrompt.op === "copy" ? "Copy" : "Move"}</button>
          <button onclick={cancelDest}>Cancel</button>
        </div>
      </div>
    </div>
  {:else if textPrompt}
    <div class="overlay" role="presentation">
      <div class="dialog">
        <h2>{PROMPT_TITLES[textPrompt.kind]}</h2>
        {#if textPrompt.kind === "rename"}
          <input
            class="dest-input"
            type="text"
            bind:value={textPrompt.value}
            use:renameFocus
            onkeydown={(e) => {
              if (e.key === "Enter") { e.preventDefault(); void confirmText(); }
              else if (e.key === "Escape") { e.preventDefault(); cancelText(); }
            }}
          />
        {:else}
          <input
            class="dest-input"
            type="text"
            bind:value={textPrompt.value}
            use:autofocus
            onkeydown={(e) => {
              if (e.key === "Enter") { e.preventDefault(); void confirmText(); }
              else if (e.key === "Escape") { e.preventDefault(); cancelText(); }
            }}
          />
        {/if}
        {#if textPrompt.error}
          <p class="inline-error">{textPrompt.error}</p>
        {/if}
        <div class="buttons">
          <button onclick={() => void confirmText()}>{PROMPT_ACTIONS[textPrompt.kind]}</button>
          <button onclick={cancelText}>Cancel</button>
        </div>
      </div>
    </div>
  {:else if prompt?.kind === "error"}
    <div class="overlay" role="presentation">
      <div class="dialog error">
        <h2>Operation failed</h2>
        <p class="path" title={prompt.path}>{prompt.path}</p>
        <p class="reason">{prompt.reason}</p>
        <div class="buttons">
          <button onclick={() => void answerError("retry")}><u>R</u>etry</button>
          <button onclick={() => void answerError("skip")}><u>S</u>kip</button>
          <button onclick={() => void answerError("skip_all")}>Skip <u>A</u>ll</button>
          {#if prompt.offerElevate}
            <button onclick={() => void answerError("elevate")}><u>E</u>levate</button>
          {/if}
          <button onclick={() => void answerError("cancel")}>Cancel</button>
        </div>
      </div>
    </div>
  {:else if prompt?.kind === "collision"}
    <div class="overlay" role="presentation">
      <div class="dialog">
        <h2>Destination already exists</h2>
        <p class="path" title={prompt.path}>{prompt.path}</p>
        <div class="buttons">
          <button onclick={() => void answerCollision("skip")}><u>S</u>kip</button>
          {#if prompt.multiple}
            <button onclick={() => void answerCollision("skip_all")}>Skip <u>A</u>ll</button>
          {/if}
          <button onclick={() => void answerCollision("overwrite")}><u>O</u>verwrite</button>
          {#if prompt.multiple}
            <button onclick={() => void answerCollision("overwrite_all")}>Overwrite A<u>l</u>l</button>
          {/if}
          <button onclick={() => void answerCollision("cancel")}>Cancel</button>
        </div>
      </div>
    </div>
  {:else if deleteConfirm}
    <div class="overlay" role="presentation">
      <div class="dialog">
        <h2>Delete?</h2>
        <p class="path" title={deleteConfirm.label}>{deleteConfirm.label}</p>
        <label class="trash-toggle">
          <input
            type="checkbox"
            checked={snapshot.trash_default}
            onchange={() => void toggleTrash()}
          />
          Move to <u>T</u>rash
        </label>
        <div class="buttons">
          <button onclick={() => void confirmDelete()}><u>D</u>elete</button>
          <button onclick={() => (deleteConfirm = null)}>Cancel</button>
        </div>
      </div>
    </div>
  {:else if activeOp}
    <div class="overlay" role="presentation">
      <div class="dialog">
        <h2>Working…</h2>
        <p class="path" title={activeOp.current}>{activeOp.current || "…"}</p>
        <div class="progress-track">
          <div class="progress-fill" style="width: {pct(activeOp)}%"></div>
        </div>
        <p class="count">{activeOp.done} / {activeOp.total} ({pct(activeOp)}%)</p>
        <div class="buttons">
          <button onclick={() => void cancelActiveOp()}>Cancel</button>
        </div>
      </div>
    </div>
  {/if}

  <!-- The embedded viewer / editor (§5.5). Full-window over the panels, the way
       FAR's do — only one is ever open. Both are pure renderers: every row and
       every document fact comes from the core. -->
  {#if editorDoc}
    <Editor
      bind:this={editorRef}
      doc={editorDoc}
      bind:text={editorText}
      dirty={editorDirty}
      message={overlayMessage}
    />
  {:else if viewerPage}
    <Viewer
      page={viewerPage}
      message={overlayMessage}
      onGeometry={(rows, cols) => void viewerDo(() => viewer.setViewport(viewerPage!.id, rows, cols))}
    />
  {/if}

  <!-- Editor dialogs, above the overlay. A save conflict is a failure state, so
       it uses the red dialog (§5.4b). -->
  {#if saveConflict}
    <div class="overlay" role="presentation">
      <div class="dialog error">
        <h2>Cannot save</h2>
        <p class="path" title={editorDoc?.path}>{editorDoc?.name}</p>
        <p class="reason">{saveConflict}</p>
        <div class="buttons">
          <button onclick={() => { saveConflict = null; void saveEditor(true); }}><u>O</u>verwrite</button>
          <button onclick={() => (saveConflict = null)}><u>C</u>ancel</button>
        </div>
      </div>
    </div>
  {:else if unsavedPrompt}
    <div class="overlay" role="presentation">
      <div class="dialog">
        <h2>Unsaved changes</h2>
        <p class="path" title={editorDoc?.path}>{editorDoc?.name}</p>
        <div class="buttons">
          <button onclick={() => { unsavedPrompt = false; void saveEditor().then(() => closeEditor(true)); }}><u>S</u>ave</button>
          <button onclick={() => void closeEditor(true)}><u>D</u>iscard</button>
          <button onclick={() => (unsavedPrompt = false)}><u>C</u>ancel</button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Help (F1, §6). Reachable from every surface, so it sits above them all.
       Every string below comes from the core's help book — this block chooses
       layout and nothing else. -->
  {#if helpOpen && helpBook}
    {@const topic = helpBook.topics[helpTopic]}
    <div class="overlay help-overlay" role="presentation">
      <div class="help">
        <header class="bar">
          <span class="name">Help</span>
          <span class="tags"><span class="tag">{topic?.title ?? ""}</span></span>
        </header>

        <div class="help-body">
          <nav class="topics" aria-label="Help topics">
            {#each helpBook.topics as t, i (t.id)}
              <button class="topic" class:active={i === helpTopic} onclick={() => (helpTopic = i)}>
                {t.title}
              </button>
            {/each}
          </nav>

          <section class="topic-content" bind:this={helpContentEl}>
            {#if topic?.body.kind === "about"}
              {@const about = topic.body.value}
              <!-- The icon is the squircle-masked emblem the app bundle ships,
                   not the full wordmark lockup: a self-contained rounded tile
                   sits correctly on both the dark and light themes, where a
                   full-bleed dark image would be a slab on near-white. -->
              <img class="about-logo" src={logoUrl} alt="" width="112" height="112" />
              <h1 class="about-name">{about.app.name}</h1>
              <p class="about-desc">{about.app.description}</p>
              <dl class="about-lines">
                {#each about.lines as line (line.label)}
                  <dt>{line.label}</dt>
                  <dd>{line.value}</dd>
                {/each}
              </dl>
              {#if pendingUpdate}
                <div class="about-update">
                  <span>Version {pendingUpdate.version} is available.</span>
                  {#if updateInstalling}
                    <span class="about-update-status">Downloading…</span>
                  {:else}
                    <button class="about-link" onclick={installUpdate}>
                      Install and restart
                    </button>
                  {/if}
                  {#if updateError}
                    <span class="about-update-error">{updateError}</span>
                  {/if}
                </div>
              {/if}

              {#if about.links.length}
                <div class="about-links">
                  {#each about.links as link (link.url)}
                    <button class="about-link" onclick={() => openAboutLink(link.url)}>
                      {link.label}
                    </button>
                  {/each}
                </div>
              {/if}
            {:else if topic?.body.kind === "shortcuts"}
              {@const sc = topic.body.value}
              <div class="search">
                <!-- The same input hygiene the terminal prompt uses: without it
                     WebKit floats an autofill dropdown over the first rows. -->
                <input
                  type="text"
                  placeholder="Filter shortcuts…"
                  spellcheck="false"
                  autocomplete="off"
                  autocapitalize="off"
                  bind:value={helpQuery}
                  oninput={() => void loadHelp()}
                  use:autofocus
                />
                <span class="match-count">{sc.match_count} of {sc.total_count}</span>
              </div>

              {#each sc.sections as section (section.context)}
                <h2 class="section">{section.title}</h2>
                {#each section.groups as group (group.category)}
                  <h3 class="group">{group.title}</h3>
                  <ul class="shortcuts">
                    {#each group.items as item (item.action)}
                      <li>
                        <span class="keys">
                          {#each item.keys as k (k)}<b>{k}</b>{/each}
                        </span>
                        <span class="what">
                          <span class="title">{item.title}</span>
                          {#if item.description}
                            <span class="desc">{item.description}</span>
                          {/if}
                        </span>
                        <span class="action-id">{item.action}</span>
                      </li>
                    {/each}
                  </ul>
                {/each}
              {:else}
                <p class="no-matches">No shortcuts match “{sc.query}”.</p>
              {/each}
            {/if}
          </section>
        </div>

        <footer class="bar status">
          <span class="hint">Tab / ⇧Tab topic · ↑↓ PgUp/PgDn scroll · Esc close</span>
        </footer>
      </div>
    </div>
  {/if}
</main>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, monospace;
    font-size: 13px;
    color: var(--fg);
    background: var(--bg);
    --row-h: 22px;
  }

  .panels {
    flex: 1;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1px;
    min-height: 0;
    background: var(--border);
  }
  /* The Esc curtain (§6): the panels are drawn aside and the terminal, which is
     flex:1 at that size, takes the whole window. */
  .panels.hidden {
    display: none;
  }

  .panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    /* Grid and flex items default to min-width:auto, which lets a long
       directory path force the panel — and the whole window — wider than the
       viewport. Both this and `.path` need an explicit 0 for the path's
       ellipsis to actually get a chance to apply. */
    min-width: 0;
    background: var(--bg);
    border-top: 2px solid transparent;
    cursor: default;
    /* Anchors the quick-search box (§5.9). It cannot live inside `.listing`,
       which is overflow:hidden and would clip it. */
    position: relative;
  }
  .panel.active {
    border-top-color: var(--accent);
  }
  /* While the prompt owns the keyboard its top border carries the accent, so the
     active panel dims its own — exactly one surface should read as focused. The
     panel stays *active* (it still decides the terminal's cwd and where
     Ctrl+Enter takes names from); it just isn't where the keys are going. */
  .panel.active.dimmed {
    border-top-color: color-mix(in srgb, var(--accent) 35%, transparent);
  }

  .panel-head,
  .panel-foot {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 8px;
    background: var(--bg-alt);
    color: var(--fg-dim);
  }
  .panel-head {
    border-bottom: 1px solid var(--border);
  }
  .panel-foot {
    border-top: 1px solid var(--border);
  }
  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Per-panel view controls (§5.8). Deliberately plain — terminal-flavoured
     restraint (§4) — and sized so the header row keeps its height. */
  .panel-ctls {
    display: flex;
    gap: 4px;
    flex: none;
  }
  .ctl {
    font: inherit;
    font-size: 11px;
    color: var(--fg-dim);
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 3px;
    cursor: pointer;
  }
  .ctl:hover {
    color: var(--fg);
  }
  .ctl.toggle.on {
    color: var(--accent);
    border-color: var(--accent);
  }

  /* Quick search (§5.9) — the panel's top-right corner, sitting over the view
     controls while it is open. Transient, so covering them costs nothing, and
     the corner keeps it clear of the listing the user is reading. */
  .quick-search {
    position: absolute;
    top: 3px;
    right: 4px;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 140px;
    max-width: calc(100% - 16px);
    padding: 1px 3px 1px 5px;
    font-size: 11px;
    color: var(--fg);
    background: var(--bg-alt);
    border: 1px solid var(--accent);
    border-radius: 3px;
  }
  .qs-label {
    color: var(--accent);
    flex: none;
  }
  .qs-text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* A rendered caret: the query is core-authored text, not a focused field, so
     there is no real one to show. */
  .qs-text::after {
    content: "▌";
    color: var(--accent);
    animation: qs-caret 1s step-end infinite;
  }
  @keyframes qs-caret {
    50% {
      opacity: 0;
    }
  }
  .qs-close {
    flex: none;
    border-color: transparent;
  }
  /* A rejected character. Pairs with the beep, and carries the whole message on
     its own for anyone with interface sounds off. */
  .quick-search.miss {
    animation: qs-flash 180ms ease-out;
  }
  @keyframes qs-flash {
    from {
      background: #6e1f1f;
      border-color: #d86b6b;
      color: #ffe;
    }
  }
  @media (prefers-color-scheme: light) {
    @keyframes qs-flash {
      from {
        background: #f5c6c6;
        border-color: #b34a4a;
        color: #3a0f0f;
      }
    }
  }

  .listing {
    flex: 1;
    display: grid;
    padding: 2px 0;
    overflow: hidden;
    min-height: 0;
  }
  /* Detailed mode: one column, metadata alongside each name (§5.2). */
  .listing.detailed {
    display: block;
  }
  .detail-row {
    display: grid;
    grid-template-columns: 1fr 6ch 14ch 9ch;
    gap: 8px;
  }
  .detail-row > span {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .d-size,
  .d-date,
  .d-perms {
    color: var(--fg-dim);
    text-align: right;
  }
  .d-perms {
    font-variant-numeric: tabular-nums;
  }

  .row {
    height: var(--row-h);
    line-height: var(--row-h);
    padding: 0 8px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .row.kind-dir {
    font-weight: 600;
  }
  .row.kind-symlink {
    font-style: italic;
  }
  /* Per-entry colors (§4). Listed before selected/cursor so those override at
     equal specificity, keeping the active-row highlight readable. */
  .row.c-dir {
    color: var(--file-dir);
  }
  .row.c-symlink {
    color: var(--file-symlink);
  }
  .row.c-hidden {
    color: var(--file-hidden);
  }
  .row.c-doc {
    color: var(--file-doc);
  }
  .row.c-exec {
    color: var(--file-exec);
  }
  .row.c-data {
    color: var(--file-data);
  }
  .row.c-code {
    color: var(--file-code);
  }
  .row.denied {
    color: #d86b6b;
  }
  .row.selected {
    color: var(--file-selected);
  }
  .row.cursor {
    background: var(--accent);
    color: var(--accent-fg);
  }
  /* On a highlighted detail row the metadata must follow the row's colour, or the
     dim grey becomes unreadable against the cursor fill. */
  .detail-row.cursor > span,
  .detail-row.selected > span {
    color: inherit;
  }
  /* Cursor in the inactive panel: outlined rather than filled. */
  .row.cursor.inactive {
    background: transparent;
    color: var(--fg);
    box-shadow: inset 0 0 0 1px var(--border);
  }

  /* The bottom rows, in the order they stack. Each draws only a `border-top` —
     a row is separated by the one below it, and the last is closed by the
     window edge. `.statusbar` keeps `--bg` even though `.prompt` below it is
     also `--bg`: that seam carries the prompt's 2px accent rule, so it is the
     one that can afford to skip the banding. */
  .statusbar {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    padding: 3px 8px;
    background: var(--bg);
    border-top: 1px solid var(--border);
    color: var(--fg-dim);
  }
  .focused-group {
    display: flex;
    align-items: baseline;
    gap: 12px;
    min-width: 0;
    overflow: hidden;
  }
  .focused {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex-shrink: 0;
    max-width: 40%;
  }
  .focused-meta {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, monospace;
    color: var(--fg);
  }
  .state {
    flex-shrink: 0;
    white-space: nowrap;
  }

  .fkeys {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
    padding: 4px 8px;
    background: var(--bg-alt);
    border-top: 1px solid var(--border);
    color: var(--fg-dim);
  }
  .fkey b {
    color: var(--accent);
  }

  /* --- File-operation dialogs (§5.4a) --- */
  .overlay {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.45);
    z-index: 10;
  }
  /* Help is reachable from every surface, so it outranks the viewer/editor
     (z-index 5) and the op dialogs (10) alike. */
  .overlay.help-overlay {
    z-index: 20;
  }
  .dialog {
    min-width: 360px;
    max-width: 70vw;
    padding: 16px;
    background: var(--bg-alt);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
  }
  /* Failures use a red-background dialog (SPEC §5.6). */
  .dialog.error {
    background: #6e1f1f;
    border-color: #d86b6b;
    color: #ffe;
  }
  .dialog h2 {
    margin: 0 0 10px;
    font-size: 13px;
    font-weight: 600;
  }
  .dialog .path {
    margin: 0 0 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-dim);
  }
  .dialog.error .path,
  .dialog.error .reason {
    color: #ffd;
  }
  .dialog .reason {
    margin: 0 0 12px;
  }
  .dest-input {
    width: 100%;
    box-sizing: border-box;
    padding: 6px 8px;
    margin-bottom: 12px;
    font-family: inherit;
    font-size: 13px;
    color: var(--fg);
    background: var(--bg);
    border: 1px solid var(--accent);
    border-radius: 4px;
  }
  .dest-input:focus {
    outline: none;
  }
  .inline-error {
    margin: 0 0 12px;
    color: #d86b6b;
  }
  .trash-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 12px;
    cursor: pointer;
    user-select: none;
  }
  .buttons {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .buttons button {
    padding: 4px 12px;
    font-family: inherit;
    font-size: 13px;
    color: var(--fg);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    cursor: pointer;
  }
  .buttons button:hover {
    border-color: var(--accent);
  }
  .dialog.error .buttons button {
    color: #ffe;
    background: rgba(0, 0, 0, 0.25);
    border-color: #d86b6b;
  }
  .buttons u {
    text-underline-offset: 2px;
  }
  .progress-track {
    height: 8px;
    margin-bottom: 8px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    overflow: hidden;
  }
  .progress-fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.1s linear;
  }
  .count {
    margin: 0 0 12px;
    color: var(--fg-dim);
  }

  /* --- Help (F1, §6) ---
     Large enough to read a long shortcut list without paging, but still a panel
     over the app rather than a replacement for it. */
  .help {
    display: flex;
    flex-direction: column;
    width: 88vw;
    height: 86vh;
    max-width: 1100px;
    background: var(--bg-alt);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
    overflow: hidden;
  }
  /* Same chrome as the viewer/editor surfaces. */
  .help .bar {
    display: flex;
    gap: 12px;
    align-items: center;
    justify-content: space-between;
    padding: 4px 10px;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
    overflow: hidden;
    flex: none;
  }
  .help .bar.status {
    border-bottom: none;
    border-top: 1px solid var(--border);
    color: var(--fg-dim);
    font-size: 11px;
  }
  .help .tags {
    display: flex;
    gap: 6px;
    flex: none;
  }
  .help .tag {
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--fg-dim);
    font-size: 11px;
  }

  .help-body {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  /* Topic rail. Tab cycles it; clicking is the mouse equivalent. */
  .topics {
    display: flex;
    flex-direction: column;
    flex: none;
    width: 160px;
    padding: 8px 0;
    border-right: 1px solid var(--border);
    background: var(--bg);
    overflow-y: auto;
  }
  .topic {
    padding: 5px 12px;
    text-align: left;
    font: inherit;
    color: var(--fg-dim);
    background: none;
    border: none;
    border-left: 2px solid transparent;
    cursor: pointer;
  }
  .topic:hover {
    color: var(--fg);
  }
  .topic.active {
    color: var(--fg);
    border-left-color: var(--accent);
    background: var(--bg-alt);
  }

  .topic-content {
    flex: 1;
    min-width: 0;
    padding: 14px 18px;
    overflow-y: auto;
    /* Not `smooth`: held arrow keys would queue animations and lag behind. */
  }

  /* About */
  .about-logo {
    display: block;
    width: 112px;
    height: 112px;
    margin: 0 0 12px;
    /* The source already carries the macOS squircle in its alpha channel, so no
       border-radius here — rounding it again would clip the corners twice. */
    image-rendering: -webkit-optimize-contrast;
  }
  .about-name {
    margin: 0 0 4px;
    font-size: 18px;
    font-weight: 600;
  }
  .about-desc {
    margin: 0 0 16px;
    color: var(--fg-dim);
  }
  .about-lines {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 4px 16px;
    margin: 0;
  }
  .about-lines dt {
    color: var(--fg-dim);
  }
  .about-lines dd {
    margin: 0;
  }
  .about-update {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 10px;
    margin-top: 20px;
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-left: 3px solid var(--accent);
    border-radius: 4px;
    background: var(--bg);
  }
  .about-update-status {
    color: var(--fg-dim);
  }
  .about-update-error {
    flex-basis: 100%;
    color: var(--term-err);
  }
  .about-links {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 16px;
    margin-top: 20px;
  }
  /* A <button>, not an <a>: these never navigate the webview, they hand the URL
     to the OS browser. Styled as a link so it still reads as one. */
  .about-link {
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    color: var(--accent);
    text-decoration: underline;
    cursor: pointer;
  }
  .about-link:hover {
    text-decoration: none;
  }
  .about-link:focus-visible {
    outline: 1px solid var(--accent);
    outline-offset: 2px;
  }

  /* Shortcuts */
  .search {
    display: flex;
    gap: 10px;
    align-items: center;
    position: sticky;
    /* The negative margins cancel the pane's padding so the field spans the full
       width and starts flush with the scrollport, where `top: 0` then pins it —
       otherwise rows would scroll visibly past its edges. */
    top: 0;
    margin: -14px -18px 12px;
    padding: 14px 18px 10px;
    background: var(--bg-alt);
    border-bottom: 1px solid var(--border);
  }
  .search input {
    flex: 1;
    min-width: 0;
    padding: 4px 8px;
    font: inherit;
    color: var(--fg);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
  }
  .search input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .match-count {
    flex: none;
    color: var(--fg-dim);
    font-size: 11px;
  }

  .section {
    margin: 18px 0 6px;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--accent);
  }
  .section:first-of-type {
    margin-top: 0;
  }
  .group {
    margin: 12px 0 4px;
    font-size: 12px;
    font-weight: 600;
    color: var(--fg-dim);
  }
  .shortcuts {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .shortcuts li {
    display: grid;
    grid-template-columns: 120px 1fr max-content;
    gap: 12px;
    align-items: baseline;
    padding: 2px 0;
  }
  .shortcuts li:hover {
    background: var(--bg);
  }
  .keys {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  /* Same weight the viewer's F-key legend uses. */
  .keys b {
    font-weight: 600;
    color: var(--fg);
  }
  .what {
    min-width: 0;
  }
  .what .title {
    color: var(--fg);
  }
  .what .desc {
    color: var(--fg-dim);
  }
  /* The action id is what a remapped config.toml will key off, so it is here —
     but quietly, since most readers only want the key and the label. */
  .action-id {
    color: var(--fg-dim);
    font-size: 11px;
    opacity: 0.55;
  }
  .no-matches {
    margin: 0;
    color: var(--fg-dim);
  }

</style>
