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
//! - [`actions`] — the catalog of bindable actions and what each one means (§6).
//! - [`fs`]     — filesystem engine: structured listings, metadata, async ops.
//! - [`filetype`] — one table deciding an entry's colour class and whether it runs (§4).
//! - [`nav`]    — the single cursor-index state machine + selection model (§5.2/§5.3).
//! - [`ops`]    — copy/move/delete pipeline: collisions, recursion guard, results.
//! - [`open`]   — Open/View/Edit routing: file-type→app decision, execute vs launch (§5.5).
//! - [`config`] — TOML config load/save; per-panel persistence; trash flag.
//! - [`help`]     — the F1 help book: About + the live shortcut list (§6).
//! - [`plugin`]   — extension-point traits (Phase 1: definitions only, no loader).
//! - [`view`]     — the embedded viewer/editor: type probe, paged sessions, docs (§5.5).
//! - [`terminal`] — the embedded command line: scrollback, history, run status (§5.7).
//! - [`state`]    — the two-panel app navigation state container.
//! - [`types`]    — the serde + specta DTOs that form the IPC contract surface.

pub mod actions;
pub mod config;
pub mod filetype;
pub mod fs;
pub mod help;
pub mod nav;
pub mod ops;
pub mod open;
pub mod plugin;
pub mod state;
pub mod terminal;
pub mod types;
pub mod view;

#[cfg(test)]
mod testutil;
