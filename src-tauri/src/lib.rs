//! Tauri adapter — the thin shell around `fm-core`.
//!
//! This crate is the ONLY one that knows about Tauri. It maps commands/events to
//! `fm-core` and owns Tauri-specific concerns (window, plugins, and — later — the
//! macOS-native elevation prompt). No business logic lives here (SPEC §3 / §10).

mod commands;
mod events;
mod exec_runtime;
mod ops_runtime;

use tauri_specta::{collect_commands, collect_events, Builder};

/// Construct the `tauri-specta` builder with the registered command and event
/// surface. Shared by [`run`] and the `export_bindings` binary so both stay in
/// sync with a single source of truth.
pub fn make_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::ping,
            commands::list_dir,
            commands::get_config,
            commands::get_keymap,
            commands::init,
            commands::set_viewport,
            commands::move_cursor,
            commands::set_cursor,
            commands::set_active_panel,
            commands::set_view_mode,
            commands::set_sort_mode,
            commands::set_show_hidden,
            commands::toggle_selection,
            commands::select_and_move,
            commands::select_all,
            commands::deselect_all,
            commands::navigate,
            commands::refresh,
            commands::create_dir,
            commands::rename,
            commands::set_trash_default,
            commands::start_transfer,
            commands::start_delete,
            commands::resolve_collision,
            commands::resolve_error,
            commands::cancel_op,
            commands::open_entry,
            commands::cancel_exec,
        ])
        .events(collect_events![
            events::OpProgressEvent,
            events::OpCollisionEvent,
            events::OpErrorEvent,
            events::OpCompleteEvent,
            events::PanelChangedEvent,
            events::ConfigChangedEvent,
            events::ExecOutputEvent,
            events::ExecDoneEvent,
        ])
}

/// Regenerate the TypeScript IPC bindings the frontend imports. Path is anchored
/// to this crate's manifest dir so it is correct regardless of the caller's CWD.
#[cfg(debug_assertions)]
pub fn export_bindings() {
    use specta_typescript::Typescript;
    // Map Rust's 64-bit ints (file sizes, mtimes, indices) to TS `number`.
    // Specta forbids this by default to prevent precision loss, but our values
    // stay well under JS's 2^53 safe-integer ceiling in practice (2^53 bytes ≈
    // 9 PB), so casting keeps the frontend types simple and correct here.
    make_builder()
        .dangerously_cast_bigints_to_number()
        .export(
            Typescript::default(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts"),
        )
        .expect("failed to export typescript bindings");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = make_builder();

    // In dev, keep the generated bindings fresh on every launch so the typed
    // contract can never drift from the Rust definitions.
    #[cfg(debug_assertions)]
    export_bindings();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // Shared navigation state lives in fm-core; the adapter just wraps it in
        // a Mutex and hands it to Tauri as managed state.
        .manage(commands::SharedState::default())
        // In-flight file operations (copy/move), so command handlers can answer
        // prompts and cancel a running op.
        .manage(ops_runtime::OpRegistry::default())
        // The single running executable (Enter-on-executable), so `cancel_exec`
        // can kill it (§5.5).
        .manage(exec_runtime::ExecState::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
