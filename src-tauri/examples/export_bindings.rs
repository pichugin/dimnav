//! Regenerates `src/lib/bindings.ts` without launching the app.
//!
//! Handy for bootstrapping the frontend before the first `cargo tauri dev`, and
//! for CI checks that the committed bindings match the Rust contract.
//! Run: `cargo run -p file-manager --example export_bindings` (or `npm run
//! bindings`).
//!
//! An **example** rather than a second `[[bin]]` on purpose: Tauri's bundler
//! copies every binary target in the package into the `.app`, but
//! `--target universal-apple-darwin` only lipos the main binary. A second bin
//! therefore fails the bundle step with "does not exist" — and only on the
//! universal target, which is exactly what release CI builds and what a plain
//! local `tauri build` does not.

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
