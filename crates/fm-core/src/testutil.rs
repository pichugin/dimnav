//! Test-only helpers shared by the crate's unit tests.
//!
//! Compiled under `#[cfg(test)]` only, so nothing here reaches a release build.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Hands out a distinct suffix per call, for the lifetime of the process.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A scratch directory under the system temp dir that no other test will touch.
///
/// The obvious implementation — timestamp the name — is what this replaces, and
/// it was subtly wrong: `SystemTime::now()` does not resolve to nanoseconds on
/// macOS, so two tests that run in parallel routinely read the *same* value and
/// derive the *same* path. Three `config::tests` migrate cases shared a root
/// that way and stomped each other's fixtures at random, which is a miserable
/// failure to diagnose from a CI log.
///
/// A process-wide counter cannot collide however coarse the clock is, and the
/// pid separates concurrent `cargo test` processes. The directory is emptied
/// before it is handed back so a run that was killed part-way through — or a
/// recycled pid — can never leak stale fixtures into a later test.
pub fn unique_dir(prefix: &str) -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("{prefix}_{}_{n}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
