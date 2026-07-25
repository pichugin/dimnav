//! The embedded terminal (SPEC §5.7 / §8 Phase 2).
//!
//! A command line that lives permanently under the panels, expandable to show
//! accumulated output. Everything about it that is a *decision* lives here — the
//! input text, history recall, the scrollback and its eviction, the run-status
//! machine behind the indicator dot, built-in commands, shell quoting, and the
//! three pane sizes. The `src-tauri` adapter only spawns the child process and
//! pumps its bytes back in through [`append`](Terminal::append); the frontend
//! renders [`TerminalState`] and forwards keys (CLAUDE.md: all logic in the core).
//!
//! ## Execution model
//!
//! Each command runs as its own child process with **piped** stdout and stderr,
//! not through a PTY. That is what makes the status indicator work as specified:
//! a child gives an exit code on `wait()`, and separate pipes are what let
//! [`finish`](Terminal::finish) distinguish "wrote to stderr" (red) from "clean"
//! (green) — a PTY merges both into one stream by definition. The cost is no
//! TTY: interactive programs and colours do not work, which SPEC §8 explicitly
//! scopes out ("not a full terminal-emulator clone").
//!
//! Because output flows through [`crate::plugin::ExecutionSink`] and this module
//! owns the buffer, swapping in a PTY-backed runtime later replaces one adapter
//! file rather than this state machine.
//!
//! ## Built-ins
//!
//! `cd` and `clear` never reach a shell. `cd` is meaningless in a one-shot child
//! (the process exits and takes its working directory with it), so the core
//! resolves it into a panel navigation — which also keeps the promise that the
//! prompt is always in the active panel's folder, MC-style. `clear` empties the
//! scrollback the app owns, which no child process could do.

pub mod history;
pub mod scrollback;

use crate::types::{
    HistoryDir, Stream, TerminalBuffer, TerminalChunk, TerminalPrefs, TerminalSize, TerminalState,
    TerminalStatus,
};

use history::History;
use scrollback::Scrollback;

/// What [`Terminal::prepare`] decided a submitted command line means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPlan {
    /// Blank input — Enter on an empty prompt does nothing.
    Nothing,
    /// Navigate the active panel to this (already normalized, absolute) path.
    /// The adapter performs the directory read; the core only decided.
    ChangeDir(String),
    /// The scrollback was cleared; there is nothing else to do.
    Cleared,
    /// Run `command` through the shell, with `cwd` as its working directory.
    Spawn { command: String, cwd: String },
}

/// The command currently executing.
#[derive(Debug, Clone)]
struct RunningCommand {
    command: String,
    /// Whether anything has arrived on stderr — the red-vs-green decision.
    had_stderr: bool,
}

/// The command line's state machine.
#[derive(Debug)]
pub struct Terminal {
    input: String,
    input_rev: u32,
    history: History,
    scrollback: Scrollback,
    status: TerminalStatus,
    size: TerminalSize,
    /// The size Esc returns to when the curtain is drawn back.
    restore_size: TerminalSize,
    focused: bool,
    running: Option<RunningCommand>,
    /// Buffer deltas produced since the adapter last drained them.
    ///
    /// Output is not the only thing that writes to the scrollback — so does the
    /// echoed prompt line, the exit footer, a spawn failure, and `clear`. Queuing
    /// them all here gives the adapter a single rule ("drain and emit") instead of
    /// a return value to remember on each individual method.
    pending_chunks: Vec<TerminalChunk>,
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new(&TerminalPrefs::default())
    }
}

impl Terminal {
    pub fn new(prefs: &TerminalPrefs) -> Self {
        Self {
            input: String::new(),
            input_rev: 0,
            history: History::default(),
            scrollback: Scrollback::new(prefs.scrollback_bytes),
            status: TerminalStatus::Idle,
            // `Full` is never restored on launch — the curtain is a transient
            // look, not a startup state.
            size: match prefs.size {
                TerminalSize::Full => TerminalSize::Half,
                other => other,
            },
            restore_size: TerminalSize::Collapsed,
            focused: false,
            running: None,
            pending_chunks: Vec::new(),
        }
    }

    /// Take the buffer deltas produced since the last call, for the adapter to
    /// push to the frontend. Draining is the only way they leave the core, so a
    /// missed drain shows up as missing output rather than a corrupt mirror.
    pub fn drain_chunks(&mut self) -> Vec<TerminalChunk> {
        std::mem::take(&mut self.pending_chunks)
    }

    /// Adopt persisted preferences after they are loaded at boot.
    pub fn apply_prefs(&mut self, prefs: &TerminalPrefs) {
        self.scrollback.set_limit(prefs.scrollback_bytes);
        self.size = match prefs.size {
            TerminalSize::Full => TerminalSize::Half,
            other => other,
        };
    }

    /// Fold live terminal state back into the preferences for persistence, the
    /// mirror of [`apply_prefs`](Self::apply_prefs).
    pub fn capture_prefs(&self, prefs: &mut TerminalPrefs) {
        prefs.scrollback_bytes = self.scrollback.limit();
        // A curtain that happened to be open at quit time restores as Half, so
        // the next launch always shows the panels.
        prefs.size = match self.size {
            TerminalSize::Full => TerminalSize::Half,
            other => other,
        };
    }

    /// Install the history loaded from disk at boot.
    pub fn set_history(&mut self, history: History) {
        self.history = history;
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    // --- Input ------------------------------------------------------------

    /// Mirror the text the user is typing.
    ///
    /// Deliberately does **not** bump `input_rev`: this is an echo of what the
    /// frontend already displays, and re-seeding the input element from it would
    /// fight the caret. Only core-originated rewrites bump the revision.
    pub fn set_input(&mut self, text: String) {
        if self.input != text {
            self.history.reset();
        }
        self.input = text;
    }

    /// Replace the prompt text from the core side, signalling the frontend to
    /// re-seed its input element.
    fn rewrite_input(&mut self, text: String) {
        self.input = text;
        self.input_rev = self.input_rev.wrapping_add(1);
    }

    /// Ctrl+C with nothing running: empty the prompt (§5.7).
    pub fn clear_input(&mut self) {
        self.history.reset();
        self.rewrite_input(String::new());
    }

    /// Recall a previous command (Up / Down). A no-op at either end, so the
    /// prompt is never blanked by an over-eager keypress.
    pub fn recall(&mut self, dir: HistoryDir) {
        let recalled = match dir {
            HistoryDir::Prev => self.history.prev(&self.input),
            HistoryDir::Next => self.history.next(),
        };
        if let Some(text) = recalled {
            self.rewrite_input(text);
        }
    }

    /// Ctrl+Enter: append the name under the panel cursor to the command line,
    /// space-separated and shell-quoted, without the panel losing focus (§5.7).
    /// Pressed repeatedly across files, it builds up a multi-file command.
    pub fn insert_name(&mut self, name: &str) {
        let mut text = std::mem::take(&mut self.input);
        if !text.is_empty() && !text.ends_with(' ') {
            text.push(' ');
        }
        text.push_str(&quote(name));
        text.push(' ');
        self.rewrite_input(text);
    }

    // --- Running ----------------------------------------------------------

    /// Decide what the submitted command line means, and record the submission:
    /// echo the prompt line into the scrollback, push history, clear the input,
    /// and (for a real spawn) go [`TerminalStatus::Running`].
    ///
    /// `cwd` is the active panel's directory — the promise that the prompt is
    /// always in the folder of the panel that had focus (§5.7).
    pub fn prepare(&mut self, cwd: &str) -> RunPlan {
        let command = self.input.trim().to_string();
        if command.is_empty() {
            return RunPlan::Nothing;
        }

        let echo = self.scrollback.append_line(&format!("{cwd}> {command}"));
        self.pending_chunks.push(echo);
        self.history.push(command.clone());
        self.rewrite_input(String::new());

        match builtin(&command) {
            Some(Builtin::Cd(arg)) => {
                let target = crate::ops::resolve_dest(std::path::Path::new(cwd), &expand_tilde(&arg));
                RunPlan::ChangeDir(target.to_string_lossy().into_owned())
            }
            Some(Builtin::Clear) => {
                let cleared = self.scrollback.clear();
                self.pending_chunks.push(cleared);
                RunPlan::Cleared
            }
            None => {
                self.status = TerminalStatus::Running;
                self.running = Some(RunningCommand {
                    command: command.clone(),
                    had_stderr: false,
                });
                RunPlan::Spawn {
                    command,
                    cwd: cwd.to_string(),
                }
            }
        }
    }

    /// Start a run the user launched by pressing Enter on an executable in a
    /// panel (§5.5) rather than by typing. Echoed into the scrollback exactly
    /// like a typed command, so both look the same in the output pane.
    pub fn begin_external(&mut self, command: String, cwd: &str) {
        let echo = self.scrollback.append_line(&format!("{cwd}> {command}"));
        self.pending_chunks.push(echo);
        self.status = TerminalStatus::Running;
        self.running = Some(RunningCommand {
            command,
            had_stderr: false,
        });
    }

    /// Feed output from the running child into the scrollback.
    pub fn append(&mut self, bytes: &[u8], stream: Stream) {
        if stream == Stream::Stderr && !bytes.is_empty() {
            if let Some(running) = self.running.as_mut() {
                running.had_stderr = true;
            }
        }
        let chunk = self.scrollback.append(bytes);
        self.pending_chunks.push(chunk);
    }

    /// The child exited. Green when it exited 0 **and** wrote nothing to stderr;
    /// red otherwise (§5.7). A non-zero or killed run also gets a footer line, so
    /// the reason is visible in the pane and not only in the dot's colour.
    pub fn finish(&mut self, code: i32) {
        let had_stderr = self.running.take().is_some_and(|r| r.had_stderr);
        self.status = if code == 0 && !had_stderr {
            TerminalStatus::Ok
        } else {
            TerminalStatus::Error
        };
        if code != 0 {
            let footer = self.scrollback.append_line(&format!("[exit {code}]"));
            self.pending_chunks.push(footer);
        }
    }

    /// Record that a run could not even start (bad path, permission denied).
    pub fn fail(&mut self, reason: &str) {
        self.running = None;
        self.status = TerminalStatus::Error;
        let chunk = self.scrollback.append_line(reason);
        self.pending_chunks.push(chunk);
    }

    /// The grey-decay rule: once the user touches any control, last run's green
    /// or red fades to a barely-visible grey, so a stale verdict is never
    /// mistaken for a fresh one (§5.7). A run in progress is left alone.
    pub fn touch(&mut self) {
        if matches!(self.status, TerminalStatus::Ok | TerminalStatus::Error) {
            self.status = TerminalStatus::Idle;
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    // --- Pane size & focus -------------------------------------------------

    /// Cmd+Shift+T: flip between the bare command line and the bottom half of
    /// the window. From the Esc curtain it collapses back, since a second size
    /// key should always make the panels visible again.
    pub fn toggle_half(&mut self) {
        self.size = match self.size {
            TerminalSize::Half | TerminalSize::Full => TerminalSize::Collapsed,
            TerminalSize::Collapsed => TerminalSize::Half,
        };
        self.restore_size = TerminalSize::Collapsed;
    }

    /// Esc: draw the panels aside to reveal the full terminal, and back again to
    /// whatever size was showing before (§6).
    pub fn toggle_curtain(&mut self) {
        if self.size == TerminalSize::Full {
            self.size = self.restore_size;
        } else {
            self.restore_size = self.size;
            self.size = TerminalSize::Full;
            // The curtain exists to work in the terminal, so it takes the keys.
            self.focused = true;
        }
    }

    /// Cmd+T: move the keyboard to the prompt, or hand it back to the panel that
    /// is still marked active. With the panels hidden the prompt has to keep the
    /// keyboard — there is nothing else to give it to.
    pub fn toggle_focus(&mut self) {
        if self.focused && self.size == TerminalSize::Full {
            return;
        }
        self.focused = !self.focused;
    }

    /// Force focus state (e.g. clicking a panel moves the keyboard back).
    pub fn set_focused(&mut self, focused: bool) {
        if !focused && self.size == TerminalSize::Full {
            return;
        }
        self.focused = focused;
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    pub fn size(&self) -> TerminalSize {
        self.size
    }

    // --- Scrollback --------------------------------------------------------

    /// Re-cap the buffer from the control in the corner of the expanded pane.
    pub fn set_scrollback_limit(&mut self, bytes: u64) {
        self.scrollback.set_limit(bytes);
    }

    pub fn clear_buffer(&mut self) {
        let cleared = self.scrollback.clear();
        self.pending_chunks.push(cleared);
    }

    pub fn buffer(&self) -> TerminalBuffer {
        self.scrollback.snapshot()
    }

    /// The renderable state. `cwd` is supplied by the caller because it belongs
    /// to the active panel, not to the terminal — the terminal follows it.
    pub fn state(&self, cwd: &str) -> TerminalState {
        TerminalState {
            input: self.input.clone(),
            input_rev: self.input_rev,
            cwd: cwd.to_string(),
            size: self.size,
            focused: self.focused,
            status: self.status,
            running: self.running.as_ref().map(|r| r.command.clone()),
            scrollback_bytes: self.scrollback.limit(),
        }
    }
}

/// The built-in commands that never reach a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Builtin {
    /// `cd [path]`; a bare `cd` means home, as in every shell.
    Cd(String),
    Clear,
}

fn builtin(command: &str) -> Option<Builtin> {
    let (head, rest) = match command.split_once(char::is_whitespace) {
        Some((h, r)) => (h, r.trim()),
        None => (command, ""),
    };
    match head {
        // Only a bare `cd` is a built-in: `cd x && make` is a compound command
        // and belongs to the shell, which the `&&` here signals.
        "cd" if !rest.contains("&&") && !rest.contains(';') && !rest.contains('|') => {
            Some(Builtin::Cd(if rest.is_empty() { "~".to_string() } else { rest.to_string() }))
        }
        "clear" | "cls" if rest.is_empty() => Some(Builtin::Clear),
        _ => None,
    }
}

/// Expand a leading `~` to the home directory. Only the leading form, and only
/// for the current user — `~other` is a shell feature we do not reimplement.
fn expand_tilde(path: &str) -> String {
    let home = || std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    if path == "~" {
        return home();
    }
    match path.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home()),
        None => path.to_string(),
    }
}

/// Quote a filename so a shell receives it as a single argument (§5.7).
///
/// POSIX single-quoting: everything inside `'…'` is literal, and an embedded
/// quote is spliced in as `'\''`. Names already made only of safe characters are
/// left bare, so the common case stays readable.
pub fn quote(name: &str) -> String {
    const SAFE: &str = "._-+=/@:,";
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || SAFE.contains(c))
    {
        return name.to_string();
    }
    format!("'{}'", name.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal() -> Terminal {
        Terminal::default()
    }

    fn lines(t: &Terminal) -> Vec<String> {
        t.buffer().lines
    }

    // --- Quoting (§5.7) ---------------------------------------------------

    #[test]
    fn quoting_leaves_safe_names_alone_and_wraps_the_rest() {
        assert_eq!(quote("README.md"), "README.md");
        assert_eq!(quote("/usr/local/bin/tool"), "/usr/local/bin/tool");
        assert_eq!(quote("My Notes.txt"), "'My Notes.txt'");
        assert_eq!(quote("it's here.txt"), r"'it'\''s here.txt'");
        assert_eq!(quote("$(rm -rf /)"), "'$(rm -rf /)'");
        assert_eq!(quote("naïve.txt"), "'naïve.txt'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn ctrl_enter_builds_up_a_multi_file_command_line() {
        let mut t = terminal();
        t.set_input("rm ".to_string());
        t.insert_name("My Notes.txt");
        t.insert_name("plain.md");
        assert_eq!(t.state("/").input, "rm 'My Notes.txt' plain.md ");
        // Each core rewrite tells the frontend to re-seed its input element.
        assert_eq!(t.state("/").input_rev, 2);
    }

    // --- input_rev contract ------------------------------------------------

    #[test]
    fn typing_does_not_bump_the_revision_but_core_rewrites_do() {
        let mut t = terminal();
        t.set_input("ec".into());
        t.set_input("echo hi".into());
        assert_eq!(t.state("/").input_rev, 0, "typing must not re-seed the input");

        t.clear_input();
        assert_eq!(t.state("/").input_rev, 1);
        assert_eq!(t.state("/").input, "");
    }

    // --- prepare / built-ins ----------------------------------------------

    #[test]
    fn a_blank_prompt_does_nothing() {
        let mut t = terminal();
        t.set_input("   ".into());
        assert_eq!(t.prepare("/home"), RunPlan::Nothing);
        assert!(lines(&t).is_empty(), "nothing should be echoed");
    }

    #[test]
    fn running_echoes_the_prompt_clears_the_input_and_goes_running() {
        let mut t = terminal();
        t.set_input("make build".into());
        assert_eq!(
            t.prepare("/home/dima"),
            RunPlan::Spawn {
                command: "make build".into(),
                cwd: "/home/dima".into()
            }
        );
        assert_eq!(lines(&t), ["/home/dima> make build"]);
        assert_eq!(t.state("/").input, "");
        assert_eq!(t.state("/").status, TerminalStatus::Running);
        assert_eq!(t.state("/").running.as_deref(), Some("make build"));
        assert_eq!(t.history().entries(), ["make build"]);
    }

    #[test]
    fn cd_becomes_a_panel_navigation_rather_than_a_child_process() {
        let mut t = terminal();
        t.set_input("cd sub".into());
        assert_eq!(t.prepare("/home/dima"), RunPlan::ChangeDir("/home/dima/sub".into()));
        // A built-in never enters the Running state — nothing was spawned.
        assert_eq!(t.state("/").status, TerminalStatus::Idle);

        t.set_input("cd ..".into());
        assert_eq!(t.prepare("/home/dima"), RunPlan::ChangeDir("/home".into()));

        t.set_input("cd /etc".into());
        assert_eq!(t.prepare("/home/dima"), RunPlan::ChangeDir("/etc".into()));
    }

    #[test]
    fn bare_cd_and_tilde_go_home() {
        std::env::set_var("HOME", "/home/tester");
        let mut t = terminal();
        t.set_input("cd".into());
        assert_eq!(t.prepare("/tmp"), RunPlan::ChangeDir("/home/tester".into()));

        t.set_input("cd ~/work".into());
        assert_eq!(t.prepare("/tmp"), RunPlan::ChangeDir("/home/tester/work".into()));
    }

    #[test]
    fn a_compound_cd_belongs_to_the_shell_not_the_builtin() {
        let mut t = terminal();
        t.set_input("cd /tmp && ls".into());
        assert!(
            matches!(t.prepare("/home"), RunPlan::Spawn { .. }),
            "a compound command must reach the shell intact"
        );
    }

    #[test]
    fn clear_empties_the_buffer_the_app_owns() {
        let mut t = terminal();
        t.begin_external("./noisy.sh".into(), "/home");
        t.append(b"lots of output\n", Stream::Stdout);
        assert!(lines(&t).len() >= 2);

        t.set_input("clear".into());
        assert_eq!(t.prepare("/home"), RunPlan::Cleared);
        assert!(lines(&t).is_empty());
    }

    // --- Status machine (§5.7) --------------------------------------------

    #[test]
    fn a_clean_run_is_green() {
        let mut t = terminal();
        t.set_input("echo hi".into());
        t.prepare("/home");
        t.append(b"hi\n", Stream::Stdout);
        t.finish(0);
        assert_eq!(t.state("/").status, TerminalStatus::Ok);
        assert!(!t.is_running());
        assert_eq!(
            lines(&t),
            ["/home> echo hi", "hi"],
            "a clean exit needs no footer"
        );
    }

    /// Every write to the buffer — the echoed prompt, program output, the exit
    /// footer — has to reach the frontend, or the pane silently diverges from
    /// what the core holds.
    #[test]
    fn every_buffer_change_is_queued_for_the_frontend() {
        let mut t = terminal();
        t.set_input("false".into());
        t.prepare("/home");
        t.append(b"oops\n", Stream::Stderr);
        t.finish(1);

        let chunks = t.drain_chunks();
        // Append then drop — the contract on `TerminalChunk`.
        let mut mirror: Vec<String> = Vec::new();
        for c in &chunks {
            mirror.extend(c.lines.clone());
            mirror.drain(0..(c.dropped as usize).min(mirror.len()));
        }
        assert_eq!(mirror, lines(&t), "the mirror must equal the core's buffer");
        assert_eq!(mirror, ["/home> false", "oops", "[exit 1]"]);

        // Draining is exhaustive — a second drain yields nothing to re-apply.
        assert!(t.drain_chunks().is_empty());
    }

    #[test]
    fn clearing_the_buffer_reaches_the_frontend_as_a_delta() {
        let mut t = terminal();
        t.begin_external("./run.sh".into(), "/home");
        t.append(b"a\nb\n", Stream::Stdout);
        t.drain_chunks();

        t.clear_buffer();
        let chunks = t.drain_chunks();
        assert_eq!(chunks.len(), 1);
        // Three lines existed (the echo plus two of output); all are evicted.
        assert_eq!(chunks[0].dropped, 3);
        assert!(chunks[0].lines.is_empty());
        assert!(lines(&t).is_empty());
    }

    #[test]
    fn stderr_output_makes_it_red_even_on_a_zero_exit() {
        let mut t = terminal();
        t.set_input("noisy".into());
        t.prepare("/home");
        t.append(b"warning: something\n", Stream::Stderr);
        t.finish(0);
        assert_eq!(t.state("/").status, TerminalStatus::Error);
    }

    #[test]
    fn a_nonzero_exit_is_red_and_leaves_a_footer() {
        let mut t = terminal();
        t.set_input("false".into());
        t.prepare("/home");
        t.finish(1);
        assert_eq!(t.state("/").status, TerminalStatus::Error);
        assert_eq!(lines(&t).last().unwrap(), "[exit 1]");
    }

    #[test]
    fn touching_a_control_decays_a_verdict_to_grey_but_never_interrupts_running() {
        let mut t = terminal();
        t.set_input("echo hi".into());
        t.prepare("/home");
        assert_eq!(t.state("/").status, TerminalStatus::Running);

        t.touch();
        assert_eq!(
            t.state("/").status,
            TerminalStatus::Running,
            "a run in progress must keep flashing"
        );

        t.finish(0);
        assert_eq!(t.state("/").status, TerminalStatus::Ok);
        t.touch();
        assert_eq!(t.state("/").status, TerminalStatus::Idle);
        t.touch();
        assert_eq!(t.state("/").status, TerminalStatus::Idle);
    }

    #[test]
    fn a_run_that_cannot_start_is_red() {
        let mut t = terminal();
        t.begin_external("/bin/nope".into(), "/home");
        t.fail("could not run /bin/nope: No such file or directory");
        assert_eq!(t.state("/").status, TerminalStatus::Error);
        assert!(!t.is_running());
        assert!(lines(&t).last().unwrap().contains("could not run"));
    }

    // --- Pane size & focus -------------------------------------------------

    #[test]
    fn cmd_shift_t_flips_between_the_bare_line_and_half_the_window() {
        let mut t = terminal();
        assert_eq!(t.size(), TerminalSize::Collapsed);
        t.toggle_half();
        assert_eq!(t.size(), TerminalSize::Half);
        t.toggle_half();
        assert_eq!(t.size(), TerminalSize::Collapsed);
    }

    #[test]
    fn the_esc_curtain_restores_whatever_size_was_showing() {
        let mut t = terminal();
        // From collapsed.
        t.toggle_curtain();
        assert_eq!(t.size(), TerminalSize::Full);
        assert!(t.focused(), "the curtain exists to work in the terminal");
        t.toggle_curtain();
        assert_eq!(t.size(), TerminalSize::Collapsed);

        // From half — Esc must come back to half, not collapse the pane.
        t.toggle_half();
        t.toggle_curtain();
        assert_eq!(t.size(), TerminalSize::Full);
        t.toggle_curtain();
        assert_eq!(t.size(), TerminalSize::Half);
    }

    #[test]
    fn the_prompt_keeps_the_keyboard_while_the_panels_are_hidden() {
        let mut t = terminal();
        t.toggle_curtain();
        t.toggle_focus();
        assert!(t.focused(), "there is no panel to hand the keyboard back to");
        t.set_focused(false);
        assert!(t.focused());
    }

    #[test]
    fn cmd_t_hands_the_keyboard_back_and_forth() {
        let mut t = terminal();
        assert!(!t.focused());
        t.toggle_focus();
        assert!(t.focused());
        t.toggle_focus();
        assert!(!t.focused());
    }

    #[test]
    fn partial_input_survives_losing_focus() {
        let mut t = terminal();
        t.set_input("half-typed comm".into());
        t.toggle_focus();
        t.set_focused(false);
        t.set_focused(true);
        assert_eq!(t.state("/").input, "half-typed comm");
        assert_eq!(t.state("/").input_rev, 0, "and the caret is never disturbed");
    }

    // --- Preferences -------------------------------------------------------

    #[test]
    fn preferences_round_trip_but_never_restore_the_curtain() {
        let mut t = Terminal::new(&TerminalPrefs {
            scrollback_bytes: 4 << 20,
            shell: None,
            size: TerminalSize::Half,
        });
        assert_eq!(t.size(), TerminalSize::Half);
        assert_eq!(t.state("/").scrollback_bytes, 4 << 20);

        t.toggle_curtain();
        let mut prefs = TerminalPrefs::default();
        t.capture_prefs(&mut prefs);
        assert_eq!(prefs.size, TerminalSize::Half, "a launch must show the panels");
        assert_eq!(prefs.scrollback_bytes, 4 << 20);

        // And a config that somehow holds Full opens as Half.
        let fresh = Terminal::new(&TerminalPrefs {
            size: TerminalSize::Full,
            ..TerminalPrefs::default()
        });
        assert_eq!(fresh.size(), TerminalSize::Half);
    }

    #[test]
    fn history_recall_drives_the_prompt() {
        let mut t = terminal();
        for cmd in ["ls", "make", "git status"] {
            t.set_input(cmd.into());
            t.prepare("/home");
        }
        t.recall(HistoryDir::Prev);
        assert_eq!(t.state("/").input, "git status");
        t.recall(HistoryDir::Prev);
        assert_eq!(t.state("/").input, "make");
        t.recall(HistoryDir::Next);
        assert_eq!(t.state("/").input, "git status");
        // Past the newest, back to the (empty) draft.
        t.recall(HistoryDir::Next);
        assert_eq!(t.state("/").input, "");
    }
}
