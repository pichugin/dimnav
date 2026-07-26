<script lang="ts">
  // The embedded editor's rendering surface (F4, SPEC §5.5).
  //
  // A plain text widget, deliberately: the core owns the *document* — encoding,
  // line endings, permissions, and the file's on-disk identity — while this owns
  // only the buffer being typed into, which is what buys native undo, selection,
  // and IME for free. Nothing here decides anything about the file; F2 hands the
  // text back to the core, which does the rest.
  import type { EditDoc } from "./ipc";

  let {
    doc,
    text = $bindable(),
    dirty = false,
    message = "",
  }: {
    doc: EditDoc;
    text: string;
    dirty?: boolean;
    /** Transient status text, e.g. the result of the last save. */
    message?: string;
  } = $props();

  let area: HTMLTextAreaElement | null = $state(null);
  let caret = $state({ line: 1, col: 1 });

  // Caret position for the status line, derived from the buffer up to the
  // selection start — cheap enough at editor-sized documents.
  function updateCaret() {
    if (!area) return;
    const upto = area.value.slice(0, area.selectionStart);
    const nl = upto.lastIndexOf("\n");
    caret = { line: upto.split("\n").length, col: upto.length - nl };
  }

  // Open at the top of the file, the way an editor should — a focused textarea
  // otherwise puts the caret after the last character.
  function focus(node: HTMLTextAreaElement) {
    node.focus();
    node.setSelectionRange(0, 0);
    node.scrollTop = 0;
  }

  /**
   * Take the DOM focus back after an overlay borrowed it (F1 help), leaving the
   * caret and scroll position exactly where the user left them — unlike the
   * mount-time `focus` action above, which deliberately jumps to the top.
   */
  export function refocus() {
    area?.focus();
  }

  const encodingLabel: Record<string, string> = {
    utf8: "UTF-8",
    utf8_bom: "UTF-8 BOM",
    utf16_le: "UTF-16LE",
    utf16_be: "UTF-16BE",
    latin1: "Latin-1",
  };
  const eolLabel: Record<string, string> = { lf: "LF", crlf: "CRLF", cr: "CR" };

  const fkeys: [string, string][] = [
    ["F2", "Save"],
    ["F6", "View"],
    ["Esc", "Close"],
  ];
</script>

<div class="editor">
  <header class="bar">
    <span class="name" title={doc.path}>{dirty ? "*" : ""}{doc.name}</span>
    <span class="tags">
      <span class="tag">{encodingLabel[doc.encoding] ?? doc.encoding}</span>
      <span class="tag">{eolLabel[doc.eol] ?? doc.eol}</span>
      {#if doc.read_only}<span class="tag ro">Read-only</span>{/if}
    </span>
  </header>

  <textarea
    bind:this={area}
    bind:value={text}
    use:focus
    readonly={doc.read_only}
    spellcheck="false"
    autocapitalize="off"
    onkeyup={updateCaret}
    onclick={updateCaret}
    onselect={updateCaret}
  ></textarea>

  <footer class="bar status">
    {#if message}
      <span class="message">{message}</span>
    {:else}
      <span>Line {caret.line}, Col {caret.col}{dirty ? " · modified" : ""}</span>
    {/if}
    <span class="fkeys">
      {#each fkeys as [key, label]}
        <span class="fkey"><b>{key}</b>{label}</span>
      {/each}
    </span>
  </footer>
</div>

<style>
  .editor {
    position: fixed;
    inset: 0;
    /* Above the panels, but below the operation dialogs (.overlay, z-index 10)
       so a save conflict or unsaved-changes prompt is visible over it. */
    z-index: 5;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    color: var(--fg);
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
  textarea {
    flex: 1;
    width: 100%;
    padding: 0 10px;
    border: none;
    outline: none;
    resize: none;
    background: var(--bg);
    color: var(--fg);
    font: inherit;
    line-height: 18px;
    tab-size: 4;
    white-space: pre;
    overflow: auto;
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
