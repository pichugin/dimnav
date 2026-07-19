//! Regenerates `src/lib/bindings.ts` without launching the app.
//!
//! Handy for bootstrapping the frontend before the first `cargo tauri dev`, and
//! for CI checks that the committed bindings match the Rust contract.
//! Run: `cargo run -p file-manager --bin export_bindings`.

fn main() {
    #[cfg(debug_assertions)]
    {
        file_manager_lib::export_bindings();
        println!("Exported TypeScript bindings to src/lib/bindings.ts");
    }
    #[cfg(not(debug_assertions))]
    {
        eprintln!("export_bindings is only available in debug builds");
    }
}
