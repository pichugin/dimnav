//! File-operation pipeline (SPEC §5.4a / §5.4b / §5.6).
//!
//! Owns copy / move / delete as async, cancellable operations that never block
//! the UI thread and always return a structured [`OpOutcome`]. Responsibilities:
//!
//! - **Collision detection** with FAR dialog shapes: single-file offers
//!   Cancel/Skip/Overwrite; multi-file adds Skip All / Overwrite All. Never
//!   silently overwrites.
//! - **Recursion guard**: normal directory recursion (copying a folder's full
//!   contents) is expected; copying a folder *into itself or a descendant* is
//!   detected and surfaced as a Skip-that-folder / Cancel dialog.
//! - **Trash vs delete**: honours the persisted "Move to Trash" flag; default is
//!   a real delete.
//! - **Progress & cancel** for large/many-file operations.
//! - A **resolution protocol** ([`Resolution`] / [`ErrorResolution`]) driven by
//!   dialogs on the frontend.
//!
//! Built so a custom operation can register against this same pipeline — the
//! [`crate::plugin::Operation`] extension point (§6a).
//!
//! Phase 1: signatures and doc contracts only.

use crate::types::{DeleteRequest, OpOutcome, OpRequest};

/// Begin a copy or move operation. Real execution is async with progress/collision
/// events and returns an `op_id` to correlate them; this stub returns a pending
/// outcome.
pub fn run(request: &OpRequest) -> OpOutcome {
    let _ = request;
    OpOutcome::default()
}

/// Begin a delete operation (trash or real delete per `request.to_trash`).
///
/// Phase 1 stub: returns a pending outcome.
pub fn delete(request: &DeleteRequest) -> OpOutcome {
    let _ = request;
    OpOutcome::default()
}
