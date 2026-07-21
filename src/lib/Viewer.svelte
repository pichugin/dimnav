<script lang="ts">
  // The embedded viewer's rendering surface (F3, SPEC §5.5).
  //
  // Pure presentation: every row string, the gutter, the wrapping, and the hex
  // layout arrive already formatted from the core. The only thing computed here
  // is how many rows and characters fit on screen, which is reported back the
  // same way the panels report their geometry — the frontend owns pixels, the
  // core owns what they mean.
  import { onMount } from "svelte";
  import { convertFileSrc } from "@tauri-apps/api/core";
  import type { ViewPage } from "./ipc";

  let {
    page,
    message = "",
    onGeometry,
  }: {
    page: ViewPage;
    /** Transient status text, e.g. a failed search. */
    message?: string;
    onGeometry: (rows: number, cols: number) => void;
  } = $props();

  // Must match the CSS below: row height and the fixed gutter width.
  const ROW_H = 18;
  const GUTTER_CH = 9;

  let contentEl: HTMLElement | null = $state(null);
  let rulerEl: HTMLElement | null = $state(null);

  // Report the visible geometry. The character width is measured from a real
  // rendered string rather than assumed, so it holds for any themed font.
  function measure() {
    if (!contentEl || !rulerEl || page.mode === "image") return;
    const charW = rulerEl.getBoundingClientRect().width / 100;
    if (charW <= 0) return;
    const rows = Math.max(1, Math.floor(contentEl.clientHeight / ROW_H));
    const cols = Math.max(8, Math.floor(contentEl.clientWidth / charW) - GUTTER_CH);
    onGeometry(rows, cols);
  }

  onMount(() => {
    measure();
    const ro = new ResizeObserver(() => measure());
    if (contentEl) ro.observe(contentEl);
    return () => ro.disconnect();
  });

  const encodingLabel: Record<string, string> = {
    utf8: "UTF-8",
    utf8_bom: "UTF-8 BOM",
    utf16_le: "UTF-16LE",
    utf16_be: "UTF-16BE",
    latin1: "Latin-1",
  };

  function bytes(n: number): string {
    if (n < 1024) return `${n} B`;
    const units = ["KiB", "MiB", "GiB", "TiB"];
    let v = n / 1024;
    let u = 0;
    while (v >= 1024 && u < units.length - 1) { v /= 1024; u += 1; }
    return `${v.toFixed(1)} ${units[u]}`;
  }

  const fkeys: [string, string][] = [
    ["F2", "Wrap"],
    ["F4", "Hex"],
    ["F5", "Goto"],
    ["F6", "Edit"],
    ["F7", "Search"],
    ["Esc", "Close"],
  ];
</script>

<div class="viewer">
  <header class="bar">
    <span class="name" title={page.path}>{page.name}</span>
    <span class="tags">
      <span class="tag">{page.mode === "hex" ? "Hex" : page.mode === "image" ? "Image" : "Text"}</span>
      {#if page.mode !== "image"}
        <span class="tag">{encodingLabel[page.encoding] ?? page.encoding}</span>
      {/if}
      {#if page.wrap}<span class="tag">Wrap</span>{/if}
      {#if !page.writable}<span class="tag ro">RO</span>{/if}
    </span>
  </header>

  <div class="content" bind:this={contentEl}>
    <!-- Off-screen ruler: 100 characters of the real font, for measuring. -->
    <span class="ruler" bind:this={rulerEl} aria-hidden="true"
      >0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000</span
    >
    {#if page.mode === "image"}
      <div class="image-wrap">
        <img src={convertFileSrc(page.path)} alt={page.name} />
      </div>
    {:else}
      {#each page.rows as row, i}
        <div class="vrow">
          <span class="gutter">{page.gutter[i] ?? ""}</span><span class="text">{row}</span>
        </div>
      {/each}
    {/if}
  </div>

  <footer class="bar status">
    {#if message}
      <span class="message">{message}</span>
    {:else if page.mode === "image"}
      <span>{bytes(page.total_bytes)}</span>
    {:else}
      <span>
        {#if page.top_line !== null}Line {page.top_line} · {/if}
        Offset {page.top_offset} of {page.total_bytes} ({page.percent}%)
        {#if page.col_offset > 0} · Col {page.col_offset + 1}{/if}
      </span>
    {/if}
    <span class="fkeys">
      {#each fkeys as [key, label]}
        <span class="fkey"><b>{key}</b>{label}</span>
      {/each}
    </span>
  </footer>
</div>

<style>
  .viewer {
    position: fixed;
    inset: 0;
    /* Above the panels, but below the operation dialogs (.overlay, z-index 10)
       so a save conflict or unsaved-changes prompt is visible over it. */
    z-index: 5;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    color: var(--fg);
    font-family: inherit;
    font-size: 13px;
  }
  .bar {
    display: flex;
    gap: 12px;
    align-items: center;
    justify-content: space-between;
    padding: 4px 10px;
    background: var(--bg-alt);
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
    overflow: hidden;
  }
  .status {
    border-bottom: none;
    border-top: 1px solid var(--border);
    color: var(--fg-dim);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
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
  .tag.ro {
    color: var(--file-doc);
    border-color: var(--file-doc);
  }
  .content {
    position: relative;
    flex: 1;
    overflow: hidden;
    padding: 0 4px;
  }
  /* Measured, never seen. */
  .ruler {
    position: absolute;
    visibility: hidden;
    white-space: pre;
    pointer-events: none;
  }
  .vrow {
    height: 18px;
    line-height: 18px;
    white-space: pre;
    overflow: hidden;
  }
  .gutter {
    display: inline-block;
    width: 9ch;
    padding-right: 1ch;
    text-align: right;
    color: var(--fg-dim);
    user-select: none;
  }
  .image-wrap {
    display: flex;
    height: 100%;
    align-items: center;
    justify-content: center;
    padding: 8px;
  }
  .image-wrap img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .message {
    color: var(--file-doc);
  }
  .fkeys {
    display: flex;
    gap: 10px;
    flex: none;
  }
  .fkey b {
    margin-right: 4px;
    font-weight: 600;
    color: var(--fg);
  }
</style>
