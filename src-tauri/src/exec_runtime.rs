//! Execution runtime — the Tauri-side host for running an executable that the
//! user launched with Enter (SPEC §5.5 / §5.7).
//!
//! `fm-core` decides *that* an entry should run ([`fm_core::open::OpenPlan::Execute`]);
//! this module performs the OS side-effect, because spawning processes, threads,
//! and emitting events are Tauri-side concerns (SPEC §3). It captures the child's
//! stdout/stderr line-by-line into [`ExecOutputEvent`]s and reports completion via
//! [`ExecDoneEvent`] — the "simple output modal" Phase-1 sink.
//!
//! This is the **Phase-2 terminal seam**: output flows through
//! [`fm_core::plugin::ExecutionSink`], so Phase 2 swaps [`TauriExecSink`] for the
//! PTY-backed terminal without changing the caller. Only one run happens at a time
//! (mirrors the single-op UI); the running child is held in [`ExecState`] so a
//! `cancel_exec` command can kill it.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use fm_core::plugin::ExecutionSink;
use fm_core::types::{ExecDone, ExecOutput};
use tauri::AppHandle;
use tauri_specta::Event;

use crate::events::{ExecDoneEvent, ExecOutputEvent};

/// Tauri-managed handle to the single running child, so `cancel_exec` can kill it.
#[derive(Default)]
pub struct ExecState(Arc<Mutex<Option<Child>>>);

/// The [`ExecutionSink`] implemented over Tauri events: each write becomes one
/// output line in the frontend modal. Phase 2 replaces this with the terminal.
struct TauriExecSink {
    app: AppHandle,
}

impl ExecutionSink for TauriExecSink {
    fn write(&mut self, bytes: &[u8]) {
        let line = String::from_utf8_lossy(bytes)
            .trim_end_matches('\r')
            .to_string();
        let _ = ExecOutputEvent(ExecOutput { line }).emit(&self.app);
    }
}

/// Spawn `path` with `cwd` as its working directory, streaming its output to the
/// frontend and reaping it on a background thread. Returns immediately (never
/// blocks the UI); a spawn failure (e.g. permission denied) is returned so the
/// caller can surface it. Emits [`ExecDoneEvent`] when the process exits.
pub fn spawn_exec(app: AppHandle, exec_state: &ExecState, path: String, cwd: String) -> Result<(), String> {
    let mut child = Command::new(&path)
        .current_dir(&cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {path}: {e}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    *exec_state.0.lock().expect("exec state poisoned") = Some(child);

    let child_slot = exec_state.0.clone();
    std::thread::spawn(move || {
        // One reader thread per pipe, each feeding its own sink; interleaving of
        // stdout/stderr is best-effort, which is fine for a simple output pane.
        let mut readers: Vec<JoinHandle<()>> = Vec::new();
        if let Some(out) = stdout {
            readers.push(spawn_reader(app.clone(), out));
        }
        if let Some(err) = stderr {
            readers.push(spawn_reader(app.clone(), err));
        }
        for r in readers {
            let _ = r.join();
        }

        // Pipes closed ⇒ the process is done or was killed; reap it for the code.
        let code = match child_slot.lock().expect("exec state poisoned").take() {
            Some(mut child) => child.wait().ok().and_then(|s| s.code()).unwrap_or(-1),
            None => -1,
        };
        let summary = format!("finished (exit {code})");
        let _ = ExecDoneEvent(ExecDone { code, summary }).emit(&app);
    });

    Ok(())
}

/// Read `r` line-by-line, forwarding each line to a [`TauriExecSink`]. Bytes are
/// split on `\n` (not `lines()`) so non-UTF-8 output degrades gracefully rather
/// than truncating the stream.
fn spawn_reader<R: Read + Send + 'static>(app: AppHandle, r: R) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut sink = TauriExecSink { app };
        let mut reader = BufReader::new(r);
        let mut buf: Vec<u8> = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if buf.last() == Some(&b'\n') {
                        buf.pop();
                    }
                    sink.write(&buf);
                }
                Err(_) => break,
            }
        }
    })
}

/// Kill the running child, if any (no-op when nothing is running). The orchestrator
/// thread reaps it and emits [`ExecDoneEvent`].
pub fn cancel(exec_state: &ExecState) {
    if let Some(child) = exec_state
        .0
        .lock()
        .expect("exec state poisoned")
        .as_mut()
    {
        let _ = child.kill();
    }
}
