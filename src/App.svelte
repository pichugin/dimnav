<script lang="ts">
  import { onMount, tick } from "svelte";
  import { nav, ops, open, events } from "./lib/ipc";
  import type {
    AppSnapshot,
    Entry,
    ErrorResolution,
    KeyBinding,
    Motion,
    OpKind,
    PanelId,
    PanelState,
    Resolution,
    SortMode,
    ViewMode,
  } from "./lib/ipc";
  import type { UnlistenFn } from "@tauri-apps/api/event";

  // Row height in px — single source of truth shared by the grid layout and the
  // viewport measurement, so the core's rows_per_column matches what's rendered.
  const ROW_H = 22;
  const SIDES: PanelId[] = ["left", "right"];

  function emptyPanel(): PanelState {
    return {
      path: "",
      entries: [],
      cursor_index: 0,
      selection: [],
      view_mode: { kind: "columns", columns: 2 },
      sort_mode: "name_folders_first",
      show_hidden: true,
      geometry: { columns: 0, rows_per_column: 0 },
    };
  }

  let snapshot = $state<AppSnapshot>({
    left: emptyPanel(),
    right: emptyPanel(),
    active: "left",
    trash_default: false,
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
  type TextPrompt = { kind: "mkdir" | "rename"; value: string; error?: string };
  let textPrompt = $state<TextPrompt | null>(null);
  // The output modal for a running executable (Enter-on-executable, §5.5). Opens
  // lazily when the first exec event arrives; the Phase-2 terminal replaces it.
  type ExecView = { lines: string[]; done: boolean; code: number };
  let execView = $state<ExecView | null>(null);

  // chord string -> action id, built from the core-provided keymap.
  let keymapByChord: Record<string, string> = {};
  const listingEls: Record<PanelId, HTMLElement | null> = { left: null, right: null };

  // Canonical chord for a KeyboardEvent: modifiers in a fixed order, then the
  // key. Shift is only added for named keys (length > 1) — for printable keys the
  // shift is already baked into the character (e.g. `*` reports as "*"). Must
  // match the format produced by the core keymap (config::default_keymap).
  function chord(e: KeyboardEvent): string {
    const parts: string[] = [];
    if (e.ctrlKey) parts.push("Ctrl");
    if (e.metaKey) parts.push("Meta");
    if (e.altKey) parts.push("Alt");
    if (e.shiftKey && e.key.length > 1) parts.push("Shift");
    parts.push(e.key);
    return parts.join("+");
  }

  function colsOf(vm: ViewMode): number {
    return vm.kind === "columns" ? vm.columns : 1;
  }

  // Column-major page layout for a panel: which slice of entries is visible and
  // how to shape the grid. Mirrors the core's cursor math (SPEC §5.2).
  function layout(p: PanelState) {
    const rows = p.geometry.rows_per_column;
    const cols = rows > 0 ? colsOf(p.view_mode) : 1;
    const effRows = rows > 0 ? rows : Math.max(1, p.entries.length);
    const pageSize = Math.max(1, cols * effRows);
    const pageStart = Math.floor(p.cursor_index / pageSize) * pageSize;
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
    return p.selection.reduce((sum, i) => sum + (p.entries[i]?.size ?? 0), 0);
  }

  function buildKeymap(bindings: KeyBinding[]): Record<string, string> {
    const map: Record<string, string> = {};
    for (const b of bindings) for (const k of b.keys) map[k] = b.action;
    return map;
  }

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

  // Mouse: single-click focuses the clicked entry (and activates its panel). The
  // core owns the cursor index; we just report the clicked global index.
  async function focusEntry(side: PanelId, index: number) {
    try {
      if (snapshot.active !== side) snapshot = await nav.setActivePanel(side);
      snapshot = await nav.setCursor(side, index);
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
  }

  // Mouse: double-click behaves like Enter — focus the entry, then navigate into
  // it (dir / symlink / `..`) or open a file with the system default (§5.5).
  async function openEntry(side: PanelId, index: number) {
    try {
      if (snapshot.active !== side) snapshot = await nav.setActivePanel(side);
      snapshot = await nav.setCursor(side, index);
      await activateFocused(side);
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
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

  // Kill the executable running in the output modal (§5.5). The backend reaps it
  // and emits the done event, which flips the modal to its finished state.
  async function cancelExec() {
    try {
      await open.cancelExec();
    } catch (err) {
      status = `error: ${errMessage(err)}`;
    }
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
    if (execView) {
      // Esc cancels a running executable; once finished it closes the modal.
      if (e.key === "Escape") {
        if (execView.done) execView = null;
        else void cancelExec();
        return true;
      }
      return false;
    }
    return false;
  }

  async function onKeydown(e: KeyboardEvent) {
    shiftHeld = e.shiftKey;
    // Text prompts (destination, mkdir, rename) own the keyboard via their focused
    // <input>; let them be.
    if (destPrompt || textPrompt) return;
    // Other op modals consume their own keys and otherwise block navigation —
    // but must let OS/browser shortcuts (Cmd/Ctrl combos, e.g. Cmd+Q) through.
    if (activeOp || prompt || deleteConfirm || execView) {
      if (e.metaKey || e.ctrlKey) return;
      if (handleModalKey(e)) e.preventDefault();
      return;
    }

    const action = keymapByChord[chord(e)];
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
        await open.openEntry(active, "view");
      } else if (action === "open.edit") {
        await open.openEntry(active, "edit");
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
      } else if (action === "panel.toggle_hidden") {
        await setHidden(active, !snapshot[active].show_hidden);
      } else if (action === "panel.cycle_sort") {
        await cycleSort(active);
      } else if (action.startsWith("panel.view_")) {
        const which = action.slice("panel.view_".length);
        await setView(active, which === "detailed" ? { kind: "detailed" } : { kind: "columns", columns: Number(which) });
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
        keymapByChord = buildKeymap(await nav.getKeymap());
        snapshot = await nav.init();
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

        // Executable run output (Enter-on-executable, §5.5). The modal opens
        // lazily on the first event so a plain file-open (no events) never shows
        // one. Reassign (not mutate) so Svelte's reactivity fires.
        unlisten.push(
          await events.execOutputEvent.listen((e) => {
            const line = e.payload.line;
            execView = execView
              ? { ...execView, lines: [...execView.lines, line] }
              : { lines: [line], done: false, code: 0 };
          }),
        );
        unlisten.push(
          await events.execDoneEvent.listen((e) => {
            const { code } = e.payload;
            execView = execView
              ? { ...execView, done: true, code }
              : { lines: [], done: true, code };
          }),
        );

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

  // Function-key hints. While Shift is held, F6 advertises its shifted action
  // (Rename) instead of Move, so the bar reflects what the next keystroke does.
  const fkeys = $derived<[string, string][]>([
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
  ]);
</script>

<main class="app">
  <div class="panels">
    {#each SIDES as side}
      {@const p = snapshot[side]}
      {@const L = layout(p)}
      <section
        class="panel"
        class:active={snapshot.active === side}
        onclick={() => (snapshot.active !== side ? nav.setActivePanel(side).then((s) => (snapshot = s)) : null)}
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
            {p.selection.length} selected{p.selection.length ? ` · ${humanSize(selectedSize(p))}` : ""}
          </span>
          <span>{p.entries.length ? p.cursor_index + 1 : 0} / {p.entries.length}</span>
        </footer>
      </section>
    {/each}
  </div>

  <!-- Phase 2 seam: the terminal command line lives here; Esc will draw the
       panels aside as a curtain over it (SPEC §5.7 / §6). -->
  <nav class="fkeys" aria-label="function keys">
    {#each fkeys as [key, name]}
      <span class="fkey"><b>{key}</b> {name}</span>
    {/each}
  </nav>

  <div class="statusbar">
    <span class="focused">{focused}</span>
    <span class="state">{status}</span>
  </div>

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
        <h2>{textPrompt.kind === "mkdir" ? "Create directory:" : "Rename to:"}</h2>
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
          <button onclick={() => void confirmText()}>{textPrompt.kind === "mkdir" ? "Create" : "Rename"}</button>
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

  <!-- Executable output modal (§5.5). Independent of the op chain above; the
       Phase-2 terminal (Esc curtain) replaces this pane. -->
  {#if execView}
    <div class="overlay" role="presentation">
      <div class="dialog exec">
        <h2>{execView.done ? `Finished — exit ${execView.code}` : "Running…"}</h2>
        <pre class="exec-output">{execView.lines.join("\n") || "…"}</pre>
        <div class="buttons">
          {#if execView.done}
            <button onclick={() => (execView = null)}>Close</button>
          {:else}
            <button onclick={() => void cancelExec()}>Cancel</button>
          {/if}
        </div>
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

  .panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg);
    border-top: 2px solid transparent;
    cursor: default;
  }
  .panel.active {
    border-top-color: var(--accent);
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

  .statusbar {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    padding: 3px 8px;
    background: var(--bg);
    border-top: 1px solid var(--border);
    color: var(--fg-dim);
  }
  .focused {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  /* Executable output modal (§5.5). Wider, with a scrollable monospace pane. */
  .dialog.exec {
    min-width: 520px;
    max-width: 80vw;
  }
  .exec-output {
    max-height: 50vh;
    margin: 0 0 12px;
    padding: 8px;
    overflow: auto;
    font-family: inherit;
    font-size: 12px;
    line-height: 1.4;
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--fg);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
  }
</style>
