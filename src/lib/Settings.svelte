<script lang="ts">
  // The F2 settings popup's rendering surface (SPEC §7).
  //
  // Pure presentation, the same contract Terminal/Viewer/Editor keep. The core
  // authors the whole book — which pages exist, which settings are on them,
  // their labels, option lists, defaults and validation — and this paints each
  // field by its `control.kind` and reports the value back. There is no list of
  // settings in this file, and there must never be one: a new setting is a row
  // in `fm_core::settings`, not an edit here (CLAUDE.md).
  //
  // Keys are deliberately not handled here. App's window-level keydown listener
  // owns the keymap and already sees them, so binding them again would run every
  // action twice — the same arrangement the terminal input uses.
  import type {
    FieldValue,
    SettingField,
    SettingsBook,
    ThemeSummary,
  } from "./ipc";

  let {
    book,
    page = $bindable(),
    contentEl = $bindable(),
    onSet,
    onReset,
  }: {
    book: SettingsBook;
    /** Index into `book.pages` — App owns it so Tab can cycle the rail. */
    page: number;
    /** The scrollport, bound out so App's scroll actions can reach it. */
    contentEl: HTMLElement | null;
    onSet: (id: string, value: FieldValue) => void;
    onReset: (id: string) => void;
  } = $props();

  const current = $derived(book.pages[page]);

  /**
   * The page flattened to the rows the cursor walks — theme choices first on the
   * Appearance page, then every field, in the order the core laid them out.
   *
   * Derived rather than stored: the core decides what is on a page, and this is
   * only the order a cursor visits them in. Which row *has* the cursor is view
   * state for the same reason the help popup's scroll offset is (App.svelte) —
   * it is presentation, not a decision about what the settings are.
   */
  type Row = { theme: string } | { field: SettingField };

  const rows = $derived.by<Row[]>(() => {
    const body = current?.body;
    if (body?.kind === "theme") {
      return [
        ...body.value.themes.map((t) => ({ theme: t.id })),
        ...body.value.fields.map((f) => ({ field: f })),
      ];
    }
    if (body?.kind === "fields") {
      return body.value.groups.flatMap((g) => g.fields.map((f) => ({ field: f })));
    }
    return [];
  });

  let cursor = $state(0);

  // A new page starts at the top; its rows are a different list entirely, so
  // carrying an index across would land somewhere arbitrary.
  $effect(() => {
    void page;
    cursor = 0;
  });

  /** Keep the cursor on a row that still exists after a re-render. */
  const at = $derived(rows.length ? Math.min(cursor, rows.length - 1) : 0);

  function reveal() {
    contentEl?.querySelector(`[data-row="${at}"]`)?.scrollIntoView({ block: "nearest" });
  }

  /** The options of a choice field, or `null` for anything else. */
  function choicesOf(f: SettingField) {
    return f.control.kind === "choice" ? f.control.options : null;
  }

  // --- Commands App's key handler calls (bind:this), the same arrangement
  // Editor.refocus and Terminal.focusInput use. Keys are never bound here.

  export function move(delta: number) {
    if (!rows.length) return;
    cursor = (at + delta + rows.length) % rows.length;
    // Wait for the class to land before asking the browser to scroll to it.
    queueMicrotask(reveal);
  }

  /** Enter / Space on the row under the cursor. */
  export function activate() {
    const row = rows[at];
    if (!row) return;
    if ("theme" in row) {
      onSet("theme", { kind: "str", value: row.theme });
      return;
    }
    const f = row.field;
    if (f.control.kind === "toggle") {
      onSet(f.id, { kind: "bool", value: !(f.value.kind === "bool" && f.value.value) });
    } else if (f.control.kind === "choice") {
      step(f, 1);
    } else {
      // Nothing to cycle: hand the caret to the field so it can be typed into.
      contentEl?.querySelector<HTMLInputElement>(`[data-row="${at}"] input`)?.focus();
    }
  }

  /** Left / Right on the row under the cursor. */
  export function adjust(delta: number) {
    const row = rows[at];
    if (!row || "theme" in row) return;
    const f = row.field;
    if (f.control.kind === "toggle") {
      onSet(f.id, { kind: "bool", value: delta > 0 });
    } else if (f.control.kind === "choice") {
      step(f, delta);
    } else if (f.control.kind === "number" && f.value.kind === "int") {
      const c = f.control;
      const next = Math.min(c.max, Math.max(c.min, f.value.value + delta * c.step));
      onSet(f.id, { kind: "int", value: next });
    }
  }

  export function scrollBy(amount: number) {
    contentEl?.scrollBy({ top: amount });
  }

  /** Move a choice field `delta` options along, wrapping at either end. */
  function step(f: SettingField, delta: number) {
    const options = choicesOf(f);
    if (!options?.length) return;
    const i = options.findIndex((o) => same(o.value, f.value));
    const next = ((i < 0 ? 0 : i) + delta + options.length) % options.length;
    onSet(f.id, options[next].value);
  }

  /** Index of a field row, so the markup can mark and address it. */
  function rowOf(f: SettingField): number {
    return rows.findIndex((r) => "field" in r && r.field.id === f.id);
  }

  function themeRowOf(id: string): number {
    return rows.findIndex((r) => "theme" in r && r.theme === id);
  }

  /** The plain text of a value, for the controls that edit one as a string. */
  function text(v: FieldValue): string {
    return v.kind === "str" ? v.value : String(v.value);
  }

  /** Whether two values are the same — used to mark the selected choice. */
  function same(a: FieldValue, b: FieldValue): boolean {
    return a.kind === b.kind && a.value === b.value;
  }

  /**
   * The swatch strip for a theme row. Painted from the colours the core
   * resolved for that theme, so the preview is what applying it would paint.
   */
  function swatchStyle(t: ThemeSummary, name: string): string {
    const v = t.swatches.find((s) => s.name === name);
    return v ? `background: ${v.value};` : "";
  }

  /** The one place a pinned theme's consequence is spelled out. */
  function pinnedNote(t: ThemeSummary): string {
    return t.pinned ? `Always ${t.pinned}` : "";
  }
</script>

<div class="overlay settings-overlay" role="presentation">
  <div class="settings">
    <header class="bar">
      <span class="name">Settings</span>
      <span class="tags"><span class="tag">{current?.title ?? ""}</span></span>
    </header>

    <div class="settings-body">
      <nav class="pages" aria-label="Settings pages">
        {#each book.pages as p, i (p.id)}
          <button class="page" class:active={i === page} onclick={() => (page = i)}>
            {p.title}
          </button>
        {/each}
      </nav>

      <section class="page-content" bind:this={contentEl}>
        {#if current?.body.kind === "theme"}
          {@const body = current.body.value}
          <h2 class="section">Theme</h2>
          <ul class="themes">
            {#each body.themes as t (t.id)}
              {@const row = themeRowOf(t.id)}
              <li>
                <button
                  class="theme"
                  class:active={t.id === body.current}
                  class:cursor={row === at}
                  data-row={row}
                  onclick={() => {
                    cursor = row;
                    onSet("theme", { kind: "str", value: t.id });
                  }}
                >
                  <span class="swatch" aria-hidden="true">
                    {#each ["bg", "fg", "accent", "file-dir", "file-exec"] as tok (tok)}
                      <span class="chip" style={swatchStyle(t, tok)}></span>
                    {/each}
                  </span>
                  <span class="what">
                    <span class="title">{t.name}</span>
                    <span class="desc">
                      {t.source === "user" ? "Your theme" : "Bundled"}{pinnedNote(t)
                        ? ` · ${pinnedNote(t)}`
                        : ""}
                    </span>
                  </span>
                  <span class="theme-id">{t.id}</span>
                </button>
              </li>
            {/each}
          </ul>
          {@render fields(body.fields)}
        {:else if current?.body.kind === "fields"}
          {@const body = current.body.value}
          {#each body.groups as group (group.id)}
            {#if group.title}<h2 class="section">{group.title}</h2>{/if}
            {@render fields(group.fields)}
          {/each}
        {/if}
      </section>
    </div>

    <footer class="bar status">
      <span class="hint">
        ↑↓ move · Enter change · ←→ choose · Tab page · changes save as you make them · Esc
        close
      </span>
    </footer>
  </div>
</div>

{#snippet fields(list: SettingField[])}
  <ul class="fields">
    {#each list as f (f.id)}
      {@const row = rowOf(f)}
      <li class:cursor={row === at} data-row={row}>
        <span class="what">
          <span class="title">{f.label}</span>
          {#if f.description}<span class="desc">{f.description}</span>{/if}
        </span>

        <span class="control">
          {#if f.control.kind === "toggle"}
            <button
              class="ctl"
              class:on={f.value.kind === "bool" && f.value.value}
              aria-pressed={f.value.kind === "bool" && f.value.value}
              onclick={() =>
                onSet(f.id, { kind: "bool", value: !(f.value.kind === "bool" && f.value.value) })}
            >
              {f.value.kind === "bool" && f.value.value ? "On" : "Off"}
            </button>
          {:else if f.control.kind === "choice"}
            <span class="choices">
              {#each f.control.options as o (text(o.value))}
                <button
                  class="ctl"
                  class:on={same(o.value, f.value)}
                  title={o.description}
                  onclick={() => onSet(f.id, o.value)}
                >
                  {o.label}
                </button>
              {/each}
            </span>
          {:else if f.control.kind === "number"}
            <input
              class="num"
              type="number"
              min={f.control.min}
              max={f.control.max}
              step={f.control.step}
              value={f.value.kind === "int" ? f.value.value : 0}
              onchange={(e) =>
                onSet(f.id, { kind: "int", value: Number(e.currentTarget.value) })}
            />
            {#if f.control.unit}<span class="unit">{f.control.unit}</span>{/if}
          {:else}
            <input
              class="str"
              type="text"
              spellcheck="false"
              autocomplete="off"
              autocapitalize="off"
              placeholder={f.control.kind === "text" ? f.control.placeholder : ""}
              value={text(f.value)}
              onchange={(e) => onSet(f.id, { kind: "str", value: e.currentTarget.value })}
            />
          {/if}

          <!-- Shown only when there is something to undo, so the column stays
               quiet on a freshly installed config. -->
          <button
            class="reset"
            class:hidden={f.is_default}
            title="Reset to {text(f.default)}"
            onclick={() => onReset(f.id)}>↺</button
          >
        </span>
      </li>
    {/each}
  </ul>
{/snippet}

<style>
  /* Settings is reachable from every surface, so like help it outranks the
     viewer/editor (z-index 5) and the op dialogs (10). It never coexists with
     help — the two swap — so they share a level. */
  .overlay.settings-overlay {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.45);
    z-index: 20;
  }

  /* Same proportions as the help popup: a panel over the app, not a
     replacement for it. */
  .settings {
    display: flex;
    flex-direction: column;
    width: 88vw;
    height: 86vh;
    max-width: 1100px;
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, monospace;
    font-size: 13px;
    background: var(--bg-alt);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
    overflow: hidden;
  }
  /* Same chrome as help and the viewer/editor surfaces. */
  .bar {
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
  .bar.status {
    border-bottom: none;
    border-top: 1px solid var(--border);
    color: var(--fg-dim);
    font-size: 11px;
  }
  .tags {
    display: flex;
    gap: 6px;
    flex: none;
  }
  .tag {
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--fg-dim);
    font-size: 11px;
  }

  .settings-body {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  /* Page rail. Tab cycles it; clicking is the mouse equivalent. */
  .pages {
    display: flex;
    flex-direction: column;
    flex: none;
    width: 160px;
    padding: 8px 0;
    border-right: 1px solid var(--border);
    background: var(--bg);
    overflow-y: auto;
  }
  .page {
    padding: 5px 12px;
    text-align: left;
    font: inherit;
    color: var(--fg-dim);
    background: none;
    border: none;
    border-left: 2px solid transparent;
    cursor: pointer;
  }
  .page:hover {
    color: var(--fg);
  }
  .page.active {
    color: var(--fg);
    border-left-color: var(--accent);
    background: var(--bg-alt);
  }

  .page-content {
    flex: 1;
    min-width: 0;
    padding: 14px 18px;
    overflow-y: auto;
    /* Not `smooth`: held arrow keys would queue animations and lag behind. */
  }

  .section {
    margin: 18px 0 8px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--fg-dim);
  }
  .section:first-child {
    margin-top: 0;
  }

  /* --- Theme picker --- */
  .themes,
  .fields {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .theme {
    display: grid;
    grid-template-columns: 92px 1fr max-content;
    gap: 12px;
    align-items: center;
    width: 100%;
    padding: 6px 8px;
    font: inherit;
    text-align: left;
    color: var(--fg);
    background: none;
    border: 1px solid transparent;
    border-radius: 4px;
    cursor: pointer;
  }
  .theme:hover {
    background: var(--bg);
  }
  .theme.active {
    border-color: var(--accent);
    background: var(--bg);
  }
  .swatch {
    display: flex;
    height: 20px;
    overflow: hidden;
    border: 1px solid var(--border);
    border-radius: 3px;
  }
  .chip {
    flex: 1;
  }
  .theme-id {
    color: var(--fg-dim);
    font-size: 11px;
  }

  /* The keyboard cursor. Deliberately distinct from `.theme.active`, which says
     which theme is *applied* — the two are different facts and are often on
     different rows. */
  .theme.cursor,
  .fields li.cursor {
    background: var(--bg);
    box-shadow: inset 2px 0 0 var(--accent);
  }

  /* --- Field rows --- */
  .fields li {
    display: grid;
    grid-template-columns: 1fr max-content;
    gap: 12px;
    align-items: start;
    padding: 6px 8px;
    border-radius: 4px;
  }
  .what {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .what .title {
    color: var(--fg);
  }
  .what .desc {
    color: var(--fg-dim);
    font-size: 11px;
  }
  .control {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .choices {
    display: flex;
    gap: 4px;
  }
  /* Matches the panel header controls, so a toggle reads the same wherever it
     appears in the app. */
  .ctl {
    padding: 2px 8px;
    font: inherit;
    font-size: 11px;
    color: var(--fg-dim);
    background: none;
    border: 1px solid var(--border);
    border-radius: 3px;
    cursor: pointer;
  }
  .ctl:hover {
    color: var(--fg);
  }
  .ctl.on {
    color: var(--accent-fg);
    background: var(--accent);
    border-color: var(--accent);
  }
  .num,
  .str {
    padding: 3px 6px;
    font-family: inherit;
    font-size: 12px;
    color: var(--fg);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 3px;
  }
  .num {
    width: 96px;
  }
  .str {
    width: 220px;
  }
  .unit {
    color: var(--fg-dim);
    font-size: 11px;
  }
  .reset {
    padding: 0 5px;
    font: inherit;
    color: var(--fg-dim);
    background: none;
    border: 1px solid transparent;
    border-radius: 3px;
    cursor: pointer;
  }
  .reset:hover {
    color: var(--fg);
    border-color: var(--border);
  }
  /* Kept in the layout rather than removed, so the rows either side of a
     modified one do not shift when it is reset. */
  .reset.hidden {
    visibility: hidden;
    cursor: default;
  }
</style>
