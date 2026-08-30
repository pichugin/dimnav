//! Tauri adapter — the thin shell around `fm-core`.
//!
//! This crate is the ONLY one that knows about Tauri. It maps commands/events to
//! `fm-core` and owns Tauri-specific concerns — the window, the Tauri plugins, the
//! long-running op/terminal/watch runtimes, and the macOS-native elevation prompt
//! (`ops_runtime`). No business logic lives here (SPEC §3 / §10).

mod commands;
mod events;
mod ops_runtime;
mod terminal_runtime;
mod watch_runtime;

use tauri::Manager;
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
            commands::get_palette,
            commands::get_keymap,
            commands::get_help,
            commands::get_settings,
            commands::set_setting,
            commands::reset_setting,
            commands::open_link,
            commands::open_privacy_settings,
            commands::check_update,
            commands::install_update,
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
            commands::equalize_panels,
            commands::refresh,
            // Quick search (§5.9).
            commands::search_start,
            commands::search_push,
            commands::search_backspace,
            commands::search_close,
            commands::create_dir,
            commands::rename,
            commands::calculate_dir_size,
            commands::set_trash_default,
            commands::start_transfer,
            commands::start_delete,
            commands::resolve_collision,
            commands::resolve_error,
            commands::cancel_op,
            commands::open_entry,
            // Embedded terminal (§5.7).
            commands::terminal_toggle_focus,
            commands::terminal_toggle_half,
            commands::terminal_toggle_curtain,
            commands::terminal_set_input,
            commands::terminal_run,
            commands::terminal_interrupt_or_clear,
            commands::terminal_history,
            commands::terminal_insert_name,
            commands::terminal_set_scrollback,
            commands::terminal_clear_buffer,
            commands::terminal_buffer,
            // Embedded viewer / editor (§5.5).
            commands::view_set_viewport,
            commands::view_scroll,
            commands::view_toggle_hex,
            commands::view_set_wrap,
            commands::view_search,
            commands::view_goto,
            commands::view_to_edit,
            commands::view_close,
            commands::edit_save,
            commands::edit_to_view,
            commands::edit_close,
        ])
        .events(collect_events![
            events::OpProgressEvent,
            events::OpCollisionEvent,
            events::OpErrorEvent,
            events::OpCompleteEvent,
            events::PanelChangedEvent,
            events::ConfigChangedEvent,
            events::TerminalChunkEvent,
            events::TerminalStateEvent,
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Shared navigation state lives in fm-core; the adapter just wraps it in
        // a Mutex and hands it to Tauri as managed state.
        .manage(commands::SharedState::default())
        // In-flight file operations (copy/move), so command handlers can answer
        // prompts and cancel a running op.
        .manage(ops_runtime::OpRegistry::default())
        // The single running command — typed at the prompt or started by Enter
        // on an executable — so Ctrl+C can interrupt it (§5.7).
        .manage(terminal_runtime::TerminalRuntime::default())
        // Open viewer sessions and editor documents. Both registries are
        // `fm-core` types; the adapter owns nothing but the lock (§5.5).
        .manage(commands::ViewState::default())
        .manage(commands::EditState::default())
        // Watches the directories both panels have open so outside changes show
        // up without the user asking (§5.6).
        .manage(watch_runtime::WatchRuntime::default())
        .invoke_handler(builder.invoke_handler())
        // Regaining focus re-checks both panels. Cheap, and it is what covers
        // everything a watcher structurally cannot see: volumes FSEvents does not
        // report on, dropped events, and time spent suspended (§5.6).
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(true) = event {
                use fm_core::plugin::FsObserver;
                let refresh_on_focus = window
                    .state::<commands::SharedState>()
                    .lock()
                    .map(|s| s.config.watch.enabled && s.config.watch.refresh_on_focus)
                    .unwrap_or(false);
                if refresh_on_focus {
                    window.state::<watch_runtime::WatchRuntime>().poke();
                }
            }
        })
        .setup(move |app| {
            builder.mount_events(app);
            app.state::<watch_runtime::WatchRuntime>()
                .start(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
