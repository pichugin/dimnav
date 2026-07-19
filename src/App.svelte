<script lang="ts">
  import { onMount, tick } from "svelte";
  import { nav } from "./lib/ipc";
  import type {
    AppSnapshot,
    Entry,
    KeyBinding,
    Motion,
    PanelId,
    PanelState,
    ViewMode,
  } from "./lib/ipc";

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
  });
  let status = $state("starting…");

  // key (KeyboardEvent.key) -> action id, built from the core-provided keymap.
  let keymapByKey: Record<string, string> = {};
  const listingEls: Record<PanelId, HTMLElement | null> = { left: null, right: null };

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

  function label(e: Entry): string {
    if (e.marker === "denied") return `⚠ ${e.name}`;
    if (e.kind === "dir") return e.name === ".." ? ".." : `${e.name}/`;
    if (e.kind === "symlink") return `${e.name}@${e.marker === "broken" ? " ⨯" : ""}`;
    return e.name;
  }

  function buildKeymap(bindings: KeyBinding[]): Record<string, string> {
    const map: Record<string, string> = {};
    for (const b of bindings) for (const k of b.keys) map[k] = b.action;
    return map;
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

  async function onKeydown(e: KeyboardEvent) {
    const action = keymapByKey[e.key];
    if (!action) return;
    e.preventDefault();
    const active = snapshot.active;
    try {
      if (action.startsWith("cursor.")) {
        snapshot = await nav.moveCursor(active, action.slice("cursor.".length) as Motion);
      } else if (action === "panel.switch") {
        snapshot = await nav.setActivePanel(active === "left" ? "right" : "left");
      } else if (action === "nav.enter") {
        snapshot = await nav.navigate(active, { kind: "into" });
      } else if (action === "nav.parent") {
        snapshot = await nav.navigate(active, { kind: "parent" });
      }
    } catch (err) {
      status = `error: ${String(err)}`;
    }
  }

  onMount(() => {
    let ro: ResizeObserver | undefined;
    (async () => {
      try {
        keymapByKey = buildKeymap(await nav.getKeymap());
        snapshot = await nav.init();
        await tick();
        await measureAll();
        ro = new ResizeObserver(() => void measureAll());
        if (listingEls.left) ro.observe(listingEls.left);
        if (listingEls.right) ro.observe(listingEls.right);
        status = "ready";
      } catch (err) {
        status = `failed to start: ${String(err)}`;
      }
    })();

    window.addEventListener("keydown", onKeydown);
    return () => {
      ro?.disconnect();
      window.removeEventListener("keydown", onKeydown);
    };
  });

  // Full name of the entry under the active panel's cursor, for the status bar.
  const focused = $derived.by(() => {
    const p = snapshot[snapshot.active];
    return p.entries[p.cursor_index]?.name ?? "";
  });
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
          <span class="view-ctl">{p.view_mode.kind === "columns" ? `${p.view_mode.columns}-col` : "detail"}</span>
        </header>

        <div class="listing" bind:this={listingEls[side]} style={gridStyle(L.cols, L.rows)}>
          {#each L.pageEntries as entry, i}
            {@const gi = L.pageStart + i}
            <div
              class="row kind-{entry.kind}"
              class:cursor={gi === p.cursor_index}
              class:inactive={snapshot.active !== side}
              class:selected={p.selection.includes(gi)}
              class:denied={entry.marker === "denied"}
            >
              {label(entry)}
            </div>
          {/each}
        </div>

        <footer class="panel-foot">
          <span>{p.selection.length} selected</span>
          <span>{p.entries.length ? p.cursor_index + 1 : 0} / {p.entries.length}</span>
        </footer>
      </section>
    {/each}
  </div>

  <!-- Phase 2 seam: the terminal command line lives here; Esc will draw the
       panels aside as a curtain over it (SPEC §5.7 / §6). -->
  <nav class="fkeys" aria-label="function keys">
    {#each [["F3", "View"], ["F4", "Edit"], ["F5", "Copy"], ["F6", "Move"], ["F7", "MkDir"], ["F8", "Delete"], ["Tab", "Switch"], ["Enter", "Open"], ["⌫", "Up"]] as [key, name]}
      <span class="fkey"><b>{key}</b> {name}</span>
    {/each}
  </nav>

  <div class="statusbar">
    <span class="focused">{focused}</span>
    <span class="state">{status}</span>
  </div>
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

  .listing {
    flex: 1;
    display: grid;
    padding: 2px 0;
    overflow: hidden;
    min-height: 0;
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
    color: var(--fg);
    font-weight: 600;
  }
  .row.kind-symlink {
    font-style: italic;
  }
  .row.denied {
    color: #d86b6b;
  }
  .row.selected {
    color: var(--accent);
  }
  .row.cursor {
    background: var(--accent);
    color: var(--accent-fg);
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
</style>
