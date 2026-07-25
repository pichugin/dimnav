//! Terminal runtime — the Tauri-side host that actually runs commands (SPEC §5.7).
//!
//! `fm-core` decides *what* to run ([`fm_core::terminal::RunPlan`]) and owns the
//! scrollback, the history, and the run-status machine; this module performs the
//! OS side-effects, because spawning processes, threads, signals, and events are
//! Tauri-side concerns (SPEC §3). It replaces the Phase-1 `exec_runtime`, whose
//! output went to a throwaway modal.
//!
//! Both ways of starting a program — typing a name at the prompt and pressing
//! Enter on an executable in a panel (§5.5) — funnel through [`spawn`], so their
//! output lands in the same buffer and looks identical.
//!
//! ## Pipes, not a PTY
//!
//! Commands run as a child of the user's login shell with stdout and stderr on
//! **separate pipes**. That is what makes the status indicator work as specified:
//! `wait()` yields an exit code, and two pipes are what let the core tell "wrote
//! to stderr" (red) from "clean" (green). A PTY merges the two by definition, so
//! it could only ever colour the dot by exit code. The trade is that programs see
//! no TTY — no colours, no interactivity — which SPEC §8 scopes out.
//!
//! Output flows through [`fm_core::plugin::ExecutionSink`], so a future
//! PTY-backed runtime replaces this file rather than the core state machine.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use fm_core::plugin::ExecutionSink;
use fm_core::types::Stream;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::commands::SharedState;
use crate::events::{TerminalChunkEvent, TerminalStateEvent};

/// Read buffer size. A read that fills it means more output is already queued.
const READ_CHUNK: usize = 8 * 1024;
/// Upper bound on how much output is batched into one event during a torrent.
const FLUSH_BYTES: usize = 64 * 1024;
/// How long an interrupted command has to exit on SIGINT before SIGKILL.
const KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// The single running child, plus the id of the run it belongs to.
struct RunningChild {
    child: Child,
    run_id: u64,
}

/// Tauri-managed handle to the running command. One at a time, mirroring the
/// single-operation UI.
#[derive(Default)]
pub struct TerminalRuntime {
    current: Arc<Mutex<Option<RunningChild>>>,
    /// Monotonic run counter, so a delayed SIGKILL can never land on a command
    /// the user started afterwards.
    next_run_id: AtomicU64,
}

/// The [`ExecutionSink`] implemented over the core's scrollback: each write is
/// appended by `fm-core` (which owns the eviction policy) and the resulting line
/// delta is pushed to the frontend.
struct TerminalSink {
    app: AppHandle,
    stream: Stream,
}

impl ExecutionSink for TerminalSink {
    fn write(&mut self, bytes: &[u8]) {
        let state = self.app.state::<SharedState>();
        let chunks = {
            let Ok(mut s) = state.lock() else { return };
            s.terminal.append(bytes, self.stream);
            s.terminal.drain_chunks()
        };
        for chunk in chunks {
            let _ = TerminalChunkEvent(chunk).emit(&self.app);
        }
    }
}

/// Run `command` through the user's shell with `cwd` as its working directory.
///
/// Returns immediately — reading and reaping happen on background threads, so the
/// UI thread is never blocked (§5.4a). A spawn failure is returned so the caller
/// can record it in the buffer and turn the indicator red.
pub fn spawn(
    app: AppHandle,
    runtime: &TerminalRuntime,
    shell: Option<String>,
    command: String,
    cwd: String,
) -> Result<(), String> {
    let shell = shell
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/sh".to_string());

    let mut cmd = Command::new(&shell);
    // `-l` sources the user's profile, so PATH matches what they get in a real
    // terminal — otherwise a GUI-launched app runs with a surprisingly bare one.
    cmd.arg("-l")
        .arg("-c")
        .arg(&command)
        .current_dir(&cwd)
        // No TTY means no interactive input is possible, so hand programs an
        // immediate EOF: `sudo` and friends then fail fast with a readable
        // message instead of hanging forever on a prompt nobody can answer.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Put the shell and everything it spawns in one process group, so an
    // interrupt reaches the grandchildren too — killing only the shell would
    // leave the actual program running.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not run {command}: {e}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let run_id = runtime.next_run_id.fetch_add(1, Ordering::SeqCst);
    *runtime.current.lock().expect("terminal runtime poisoned") =
        Some(RunningChild { child, run_id });

    let slot = runtime.current.clone();
    std::thread::spawn(move || {
        // One reader thread per pipe. Interleaving between them is best-effort;
        // what matters is that they stay distinguishable, which is what decides
        // the indicator's colour.
        let mut readers: Vec<JoinHandle<()>> = Vec::new();
        if let Some(out) = stdout {
            readers.push(spawn_reader(app.clone(), out, Stream::Stdout));
        }
        if let Some(err) = stderr {
            readers.push(spawn_reader(app.clone(), err, Stream::Stderr));
        }
        for r in readers {
            let _ = r.join();
        }

        // Both pipes closed ⇒ the process is done or was killed; reap the code.
        let code = match slot.lock().expect("terminal runtime poisoned").take() {
            Some(mut running) => running
                .child
                .wait()
                .ok()
                .and_then(|s| s.code())
                .unwrap_or(-1),
            None => -1,
        };

        let state = app.state::<SharedState>();
        let (chunks, term) = {
            let Ok(mut s) = state.lock() else { return };
            s.terminal.finish(code);
            let chunks = s.terminal.drain_chunks();
            let cwd = s.terminal_cwd();
            (chunks, s.terminal.state(&cwd))
        };
        for chunk in chunks {
            let _ = TerminalChunkEvent(chunk).emit(&app);
        }
        let _ = TerminalStateEvent(term).emit(&app);
    });

    Ok(())
}

/// Read `r` to EOF, forwarding output to the core's scrollback.
///
/// Batching rule: a read that fills the buffer means the producer has more
/// queued, so keep accumulating rather than emitting an event per 8 KiB — a
/// `find /` would otherwise fire thousands of events a second. A short read means
/// the producer has drained, so flush at once and keep the pane responsive.
fn spawn_reader<R: Read + Send + 'static>(
    app: AppHandle,
    mut r: R,
    stream: Stream,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut sink = TerminalSink { app, stream };
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; READ_CHUNK];
        loop {
            match r.read(&mut chunk) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if n < READ_CHUNK || buf.len() >= FLUSH_BYTES {
                        sink.write(&buf);
                        buf.clear();
                    }
                }
                Err(_) => break,
            }
        }
        if !buf.is_empty() {
            sink.write(&buf);
        }
    })
}

/// Ctrl+C on a running command: SIGINT the whole process group, then SIGKILL
/// anything still alive after a grace period. The reader threads see EOF, the
/// orchestrator reaps the exit code, and the indicator turns red.
///
/// Returns whether there was anything to interrupt — the caller clears the prompt
/// instead when there was not (§5.7).
pub fn interrupt(runtime: &TerminalRuntime) -> bool {
    // Read what we need and release the lock before signalling, so the reaping
    // thread is never blocked behind us.
    let Some((pid, run_id)) = ({
        let Ok(guard) = runtime.current.lock() else {
            return false;
        };
        guard.as_ref().map(|r| (r.child.id(), r.run_id))
    }) else {
        return false;
    };

    #[cfg(unix)]
    {
        signal_group(pid, libc::SIGINT);
        // A program that ignores SIGINT must not strand the terminal. The run-id
        // check keeps this delayed kill from landing on a command the user
        // started in the meantime.
        let slot = runtime.current.clone();
        std::thread::spawn(move || {
            std::thread::sleep(KILL_GRACE);
            let same_run = slot
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|r| r.run_id == run_id))
                .unwrap_or(false);
            if same_run {
                signal_group(pid, libc::SIGKILL);
            }
        });
    }
    // Windows has no process group to signal this way; killing the child is the
    // best approximation until Phase 4 addresses it properly.
    #[cfg(not(unix))]
    {
        let _ = (pid, run_id);
        if let Ok(mut guard) = runtime.current.lock() {
            if let Some(running) = guard.as_mut() {
                let _ = running.child.kill();
            }
        }
    }
    true
}

/// Signal the process group led by `pid` (see `process_group(0)` in [`spawn`]).
#[cfg(unix)]
fn signal_group(pid: u32, sig: libc::c_int) {
    // Safety: `killpg` is a plain libc call with no memory contract. An unknown
    // pgid just returns ESRCH, which is the ordinary race where the command
    // exited between our reading the pid and signalling it.
    unsafe {
        libc::killpg(pid as libc::pid_t, sig);
    }
}
