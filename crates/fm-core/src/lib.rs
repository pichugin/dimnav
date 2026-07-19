//! # fm-core
//!
//! The platform-agnostic core of the two-panel file manager. **All** application
//! logic lives here: the filesystem engine, the navigation/selection state
//! machine, the file-operation pipeline, config, and the plugin extension-point
//! traits.
//!
//! ## The one non-negotiable rule
//!
//! This crate MUST NOT depend on `tauri` or on any webview concern. The frontend
//! is a thin, swappable rendering layer (SPEC §3); keeping every bit of logic and
//! state on this side of a typed IPC boundary is what makes a future swap to
//! Iced/egui possible without a rewrite. `cargo tree -p fm-core` must never show
//! `tauri`.
//!
//! ## Module map (SPEC §6a layering)
//!
//! - [`fs`]     — filesystem engine: structured listings, metadata, async ops.
//! - [`nav`]    — the single cursor-index state machine + selection model (§5.2/§5.3).
//! - [`ops`]    — copy/move/delete pipeline: collisions, recursion guard, results.
//! - [`config`] — TOML config load/save; per-panel persistence; trash flag.
//! - [`plugin`] — extension-point traits (Phase 1: definitions only, no loader).
//! - [`state`]  — the two-panel app navigation state container.
//! - [`types`]  — the serde + specta DTOs that form the IPC contract surface.

pub mod config;
pub mod fs;
pub mod nav;
pub mod ops;
pub mod plugin;
pub mod state;
pub mod types;
