<script lang="ts">
  // The embedded terminal's rendering surface (SPEC §5.7).
  //
  // Pure presentation. The core owns the prompt text, the history, the
  // scrollback and its eviction, the run-status machine, and the built-ins; this
  // component paints the lines it is handed, forwards keystrokes, and reports
  // nothing back that it decided on its own.
  //
  // The one deliberate bend of "no logic in the frontend" — the same one the
  // editor makes with its <textarea> — is that the <input> element holds the
  // text while the user types. The core is still authoritative: it is told every
  // keystroke, and it re-seeds the element whenever *it* rewrites the text,
  // signalled by `term.input_rev` changing. That revision check is what keeps a
  // snapshot arriving mid-keystroke from clobbering the caret.
  //
  // Keys are deliberately not handled here: App's window-level keydown listener
  // already sees them and owns the keymap, so binding them again on the input
  // would run every command twice.
  import { tick } from "svelte";
  import type { TerminalState } from "./ipc";

  let {
    term,
    lines,
    pending,
    onInput,
    onFocus,
    onScrollback,
    onClear,
  }: {
    term: TerminalState;
    /** Completed scrollback lines, mirrored from the core's buffer. */
    lines: string[];
    /** The still-incomplete trailing line, if a program stopped mid-line. */
    pending: string;
    onInput: (text: string) => void;
    onFocus: () => void;
    onScrollback: (bytes: number) => void;
    onClear: () => void;
  } = $props();

  let inputEl: HTMLInputElement | null = $state(null);
  let outputEl: HTMLElement | null = $state(null);

  const expanded = $derived(term.size !== "collapsed");

  // Scrollback sizes offered by the control in the corner of the expanded pane.
  const SCROLLBACK: { bytes: number; label: string }[] = [
    { bytes: 256 * 1024, label: "256 KB" },
    { bytes: 1024 * 1024, label: "1 MB" },
    { bytes: 4 * 1024 * 1024, label: "4 MB" },
    { bytes: 16 * 1024 * 1024, label: "16 MB" },
    { bytes: 64 * 1024 * 1024, label: "64 MB" },
  ];

  const STATUS_TITLE: Record<TerminalState["status"], string> = {
    idle: "Nothing running",
    running: "Running…",
    ok: "Last command finished cleanly",
    error: "Last command failed or wrote to stderr",
  };

  // --- Focus ---------------------------------------------------------------
  // The core decides who owns the keyboard; the DOM just follows it, so Cmd+T
  // and a click on the prompt end up in exactly the same state.
  $effect(() => {
    if (!inputEl) return;
    if (term.focused) {
      if (document.activeElement !== inputEl) inputEl.focus();
    } else if (document.activeElement === inputEl) {
      inputEl.blur();
    }
  });

  // Re-seed the element only when the core rewrote the text (history recall,
  // Ctrl+Enter insertion, clear, run) — never from an echo of our own typing.
  let appliedRev = -1;
  $effect(() => {
    const rev = term.input_rev;
    if (!inputEl || rev === appliedRev) return;
    appliedRev = rev;
    inputEl.value = term.input;
    // Insertion appends, so put the caret at the end where typing continues.
    inputEl.setSelectionRange(inputEl.value.length, inputEl.value.length);
  });

  // --- Output pane ---------------------------------------------------------
  // Stick to the bottom while the user is reading live output, but leave them
  // alone the moment they scroll up to look at something — standard terminal
  // behaviour, and the only reason this component tracks any state of its own.
  let stickToBottom = $state(true);

  function onScroll() {
    if (!outputEl) return;
    const slack = outputEl.scrollHeight - outputEl.scrollTop - outputEl.clientHeight;
    stickToBottom = slack < 8;
  }

  // One re-render per frame at most: a torrent of output must not thrash layout.
  let scrollQueued = false;
  $effect(() => {
    // Touch the reactive inputs so this runs whenever output or size changes.
    void lines.length;
    void pending;
    void term.size;
    if (!outputEl || !stickToBottom || scrollQueued) return;
    scrollQueued = true;
    requestAnimationFrame(async () => {
      scrollQueued = false;
      await tick();
      if (outputEl && stickToBottom) outputEl.scrollTop = outputEl.scrollHeight;
    });
  });

  /** Page the output pane with PgUp/PgDn while the prompt has focus. */
  export function scrollByPage(direction: -1 | 1) {
    if (!outputEl) return;
    outputEl.scrollBy({ top: direction * outputEl.clientHeight * 0.9 });
    onScroll();
  }

  /** Jump back to the live end of the output. */
  export function scrollToEnd() {
    stickToBottom = true;
    if (outputEl) outputEl.scrollTop = outputEl.scrollHeight;
  }

  /**
   * Take the DOM focus back after an overlay borrowed it (F1 help). The core
   * still has us focused, so the `$effect` above will not fire on its own —
   * without this the prompt looks focused but swallows nothing.
   */
  export function focusInput() {
    inputEl?.focus();
  }
</script>

<section class="terminal" class:expanded class:full={term.size === "full"}>
  {#if expanded}
    <div
      class="output"
      bind:this={outputEl}
      onscroll={onScroll}
      role="log"
      aria-label="terminal output"
    >
      <pre class="output-text">{lines.join("\n")}{pending ? (lines.length ? "\n" : "") + pending : ""}</pre>
    </div>
    <!-- The scrollback control lives in the corner of the expanded pane, out of
         the way of the output it governs. -->
    <div class="output-foot">
      <span class="cwd" title={term.cwd}>{term.cwd}</span>
      <span class="foot-ctls">
        {#if !stickToBottom}
          <button class="ctl" onclick={scrollToEnd} title="Jump to the live end">↓ end</button>
        {/if}
        <label class="ctl-label">
          scrollback
          <select
            class="ctl"
            title="How much output to keep"
            value={String(term.scrollback_bytes)}
            onchange={(e) => onScrollback(Number(e.currentTarget.value))}
          >
            {#each SCROLLBACK as s}<option value={String(s.bytes)}>{s.label}</option>{/each}
            <!-- A hand-edited config value that is not one of the presets still
                 has to be selectable, or the control would silently change it. -->
            {#if !SCROLLBACK.some((s) => s.bytes === term.scrollback_bytes)}
              <option value={String(term.scrollback_bytes)}>
                {Math.round(term.scrollback_bytes / 1024)} KB
              </option>
            {/if}
          </select>
        </label>
        <button class="ctl" onclick={onClear} title="Clear the buffer — Ctrl+L">clear</button>
      </span>
    </div>
  {/if}

  <div class="prompt" class:focused={term.focused}>
    <span class="sigil" aria-hidden="true">&gt;</span>
    <input
      class="prompt-input"
      type="text"
      spellcheck="false"
      autocomplete="off"
      autocapitalize="off"
      aria-label="command line"
      bind:this={inputEl}
      oninput={(e) => onInput(e.currentTarget.value)}
      onmousedown={() => { if (!term.focused) onFocus(); }}
    />
    <!-- The run indicator: yellow flashing while a command runs, then green or
         red until the user touches anything, then grey (§5.7). -->
    <span
      class="dot"
      data-status={term.status}
      title={term.running ?? STATUS_TITLE[term.status]}
    ></span>
  </div>
</section>

<style>
  .terminal {
    display: flex;
    flex-direction: column;
    flex: none;
    min-height: 0;
    background: var(--bg);
  }
  /* Half the window, taken from the bottom. The panels above are flex:1, so they
     shrink to fit — the terminal pushes them up rather than covering them. */
  .terminal.expanded {
    height: 50vh;
  }
  /* The Esc curtain: the panels are gone, so take everything that is left. */
  .terminal.full {
    height: auto;
    flex: 1;
  }

  .output {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: auto;
    padding: 4px 8px;
    border-top: 1px solid var(--border);
    background: var(--bg);
  }
  .output-text {
    margin: 0;
    font-family: inherit;
    font-size: 12px;
    line-height: 1.45;
    white-space: pre;
    color: var(--fg);
  }

  .output-foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    padding: 2px 8px;
    font-size: 11px;
    background: var(--bg-alt);
    border-top: 1px solid var(--border);
    color: var(--fg-dim);
  }
  /* Truncates on the right, like the panel headers' paths — a `direction: rtl`
     trick would keep the deepest part visible but relocates the leading `/` to
     the end of the string, which reads as a different path. */
  .cwd {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .foot-ctls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  .ctl-label {
    display: flex;
    align-items: center;
    gap: 3px;
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

  /* The command line. Its top border lights up in the active-panel accent when
     it owns the keyboard, matching how `.panel.active` marks itself. */
  .prompt {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 8px;
    background: var(--bg);
    border-top: 2px solid var(--border);
  }
  .prompt.focused {
    border-top-color: var(--accent);
  }
  .sigil {
    flex: none;
    color: var(--fg-dim);
    user-select: none;
  }
  .prompt.focused .sigil {
    color: var(--accent);
  }
  .prompt-input {
    flex: 1;
    min-width: 0;
    padding: 0;
    font-family: inherit;
    font-size: 13px;
    color: var(--fg);
    background: transparent;
    border: none;
    outline: none;
  }

  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--term-idle);
    opacity: 0.25;
  }
  .dot[data-status="running"] {
    background: var(--term-run);
    opacity: 1;
    animation: blink 0.9s steps(1, end) infinite;
  }
  .dot[data-status="ok"] {
    background: var(--term-ok);
    opacity: 1;
  }
  .dot[data-status="error"] {
    background: var(--term-err);
    opacity: 1;
  }
  @keyframes blink {
    50% {
      opacity: 0.15;
    }
  }
  /* The flash is a status signal, not decoration — honour a reduced-motion
     preference by holding it steady instead (§4: nothing distracting). */
  @media (prefers-reduced-motion: reduce) {
    .dot[data-status="running"] {
      animation: none;
    }
  }
</style>
