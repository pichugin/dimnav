//! Typed events — the push (core → frontend) half of the IPC adapter.
//!
//! The event *payloads* live in `fm-core::types` (the single contract surface).
//! `tauri_specta::Event` can only be derived here (it is a Tauri-side concern),
//! so each event is a transparent newtype over its core payload: the generated
//! TypeScript payload type equals the inner DTO, no wrapper object.
//!
//! These correspond to the operation lifecycle and panel/config change signals
//! in the plan (Part C). Emitting them is feature work; defining them now makes
//! the event contract concrete in the generated bindings.

use fm_core::types::{
    CollisionPrompt, OpErrorInfo, OpOutcome, OpProgress, PanelChanged, TerminalChunk, TerminalState,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

/// Progress tick for a running operation.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(transparent)]
#[specta(transparent)]
pub struct OpProgressEvent(pub OpProgress);

/// A name collision needs a user decision (FAR dialog shapes).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(transparent)]
#[specta(transparent)]
pub struct OpCollisionEvent(pub CollisionPrompt);

/// An operation failed — rendered as a red-background dialog.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(transparent)]
#[specta(transparent)]
pub struct OpErrorEvent(pub OpErrorInfo);

/// An operation finished (success / partial / failed).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(transparent)]
#[specta(transparent)]
pub struct OpCompleteEvent(pub OpOutcome);

/// A panel's state changed out of band (e.g. directory changed underneath us).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(transparent)]
#[specta(transparent)]
pub struct PanelChangedEvent(pub PanelChanged);

/// Config was reloaded (hot reload — nice-to-have). No payload.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct ConfigChangedEvent;

/// Fresh output from the running command (§5.7). Carries a *line delta* — the
/// lines completed, the new partial tail, and how many old lines the core
/// evicted — so the frontend's mirror stays exact without knowing the eviction
/// policy.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(transparent)]
#[specta(transparent)]
pub struct TerminalChunkEvent(pub TerminalChunk);

/// The command line's state changed from the backend rather than from a command
/// the frontend issued — i.e. a run finished, so the indicator must change
/// colour (§5.7).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(transparent)]
#[specta(transparent)]
pub struct TerminalStateEvent(pub TerminalState);
