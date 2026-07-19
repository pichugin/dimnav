//! Operation runtime — the Tauri-side host for `fm-core`'s transfer engine.
//!
//! `fm-core::ops::run_transfer` is pure and synchronous; it delegates every
//! side-effecting interaction to an [`OpObserver`]. This module supplies that
//! observer ([`TauriObserver`]) and the [`OpRegistry`] that lets command handlers
//! answer collision/error prompts and cancel a running op — the pieces that must
//! live on the Tauri side because they touch events, threads, and the macOS
//! native auth prompt (SPEC §3 / §5.6).
//!
//! Lifecycle: `start_transfer` (in `commands`) registers an op, spawns a
//! background thread running `run_transfer`, and returns the `op_id`. The engine
//! emits progress events and, on a collision/error, emits a prompt event then
//! **blocks** on a channel until a `resolve_*` command feeds the answer back in.
//! Cancellation flips a shared `AtomicBool` the engine polls between items.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use fm_core::ops::OpObserver;
use fm_core::types::{
    CollisionPrompt, ErrorResolution, OpErrorInfo, OpKind, OpProgress, Resolution,
};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::events::{OpCollisionEvent, OpErrorEvent, OpProgressEvent};

/// A user's answer to a blocking prompt, fed back from a `resolve_*` command.
pub enum UserInput {
    Collision(Resolution),
    Error(ErrorResolution),
}

/// Handles to a running operation held by the registry.
struct OpControl {
    /// Delivers prompt answers to the engine thread's observer.
    tx: Sender<UserInput>,
    /// Cooperative cancel flag polled by the engine.
    cancel: Arc<AtomicBool>,
}

/// Tauri-managed table of in-flight operations, keyed by `op_id`.
#[derive(Default)]
pub struct OpRegistry {
    ops: Mutex<HashMap<String, OpControl>>,
    next: AtomicU64,
}

impl OpRegistry {
    /// Allocate an `op_id` and record its control handles.
    pub fn register(&self, tx: Sender<UserInput>, cancel: Arc<AtomicBool>) -> String {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let op_id = format!("op-{id}");
        self.ops
            .lock()
            .expect("op registry poisoned")
            .insert(op_id.clone(), OpControl { tx, cancel });
        op_id
    }

    /// Deliver a prompt answer to a waiting op. Errors if the op has already
    /// finished (its receiver is gone).
    pub fn send(&self, op_id: &str, input: UserInput) -> Result<(), String> {
        let ops = self.ops.lock().expect("op registry poisoned");
        match ops.get(op_id) {
            Some(c) => {
                c.tx.send(input)
                    .map_err(|_| "operation is no longer running".to_string())
            }
            None => Err("unknown operation id".to_string()),
        }
    }

    /// Request cancellation of a running op (no-op if unknown/finished).
    pub fn cancel(&self, op_id: &str) {
        if let Some(c) = self.ops.lock().expect("op registry poisoned").get(op_id) {
            c.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Drop an op's handles once its thread has finished.
    pub fn remove(&self, op_id: &str) {
        self.ops.lock().expect("op registry poisoned").remove(op_id);
    }
}

/// The `fm-core` observer implemented over Tauri events + a channel + a cancel
/// flag. Runs on the op's background thread.
pub struct TauriObserver {
    pub app: AppHandle,
    pub op_id: String,
    pub rx: Receiver<UserInput>,
    pub cancel: Arc<AtomicBool>,
}

impl OpObserver for TauriObserver {
    fn progress(&self, done: u64, total: u64, current: &str) {
        let _ = OpProgressEvent(OpProgress {
            op_id: self.op_id.clone(),
            bytes_done: 0,
            bytes_total: 0,
            count_done: done,
            count_total: total,
            current: current.to_string(),
        })
        .emit(&self.app);
    }

    fn on_collision(&self, path: &str, multiple: bool) -> Resolution {
        let _ = OpCollisionEvent(CollisionPrompt {
            op_id: self.op_id.clone(),
            path: path.to_string(),
            multiple,
        })
        .emit(&self.app);
        // Block until a resolve_collision command answers. A closed channel or a
        // mismatched answer defaults to Cancel — the safe choice.
        match self.rx.recv() {
            Ok(UserInput::Collision(r)) => r,
            _ => Resolution::Cancel,
        }
    }

    fn on_error(&self, path: &str, reason: &str, offer_elevate: bool) -> ErrorResolution {
        let _ = OpErrorEvent(OpErrorInfo {
            op_id: self.op_id.clone(),
            path: path.to_string(),
            reason: reason.to_string(),
            offer_elevate,
        })
        .emit(&self.app);
        match self.rx.recv() {
            Ok(UserInput::Error(r)) => r,
            _ => ErrorResolution::Cancel,
        }
    }

    fn elevate_item(&self, kind: OpKind, src: &str, dest: &str) -> Result<(), String> {
        elevate(kind, src, dest)
    }

    fn trash_item(&self, path: &str) -> Result<(), String> {
        trash::delete(path).map_err(|e| format!("could not move to Trash: {e}"))
    }

    fn elevate_delete(&self, path: &str) -> Result<(), String> {
        elevate_delete(path)
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// Convenience: fetch the managed registry off an `AppHandle` (used from the op
/// thread to clean up on completion).
pub fn registry(app: &AppHandle) -> tauri::State<'_, OpRegistry> {
    app.state::<OpRegistry>()
}

// --- Privilege elevation (OS-native prompt) --------------------------------

/// Re-run a single copy/move item with administrator privileges. On macOS this
/// goes through `osascript`'s `with administrator privileges`, which shows the
/// **OS-native** auth dialog — the app never sees the password (SPEC §5.6).
#[cfg(target_os = "macos")]
fn elevate(kind: OpKind, src: &str, dest: &str) -> Result<(), String> {
    use std::process::Command;

    // Build a shell command with single-quoted paths, then embed that whole
    // command as an AppleScript double-quoted string literal.
    let shell_cmd = match kind {
        OpKind::Copy => format!("/bin/cp -R {} {}", sh_quote(src), sh_quote(dest)),
        OpKind::Move => format!("/bin/mv {} {}", sh_quote(src), sh_quote(dest)),
    };
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        applescript_escape(&shell_cmd)
    );

    let status = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .status()
        .map_err(|e| format!("failed to launch osascript: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err("elevation was cancelled or failed".to_string())
    }
}

#[cfg(not(target_os = "macos"))]
fn elevate(_kind: OpKind, _src: &str, _dest: &str) -> Result<(), String> {
    Err("privilege elevation is not supported on this platform".to_string())
}

/// Re-delete a single path with administrator privileges (the delete counterpart
/// of [`elevate`]). On macOS this runs `rm -rf <path>` through `osascript`'s
/// `with administrator privileges`, showing the OS-native auth dialog — the app
/// never handles the password (SPEC §5.6).
#[cfg(target_os = "macos")]
fn elevate_delete(path: &str) -> Result<(), String> {
    use std::process::Command;

    let shell_cmd = format!("/bin/rm -rf {}", sh_quote(path));
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        applescript_escape(&shell_cmd)
    );

    let status = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .status()
        .map_err(|e| format!("failed to launch osascript: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err("elevation was cancelled or failed".to_string())
    }
}

#[cfg(not(target_os = "macos"))]
fn elevate_delete(_path: &str) -> Result<(), String> {
    Err("privilege elevation is not supported on this platform".to_string())
}

/// POSIX single-quote a string so the shell treats it as one literal argument.
#[cfg(target_os = "macos")]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Escape a string for embedding inside an AppleScript double-quoted literal.
#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
