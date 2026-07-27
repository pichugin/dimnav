import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri sets TAURI_DEV_HOST when developing against a device on the network.
const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/ — tuned for use as a Tauri frontend.
export default defineConfig({
  plugins: [svelte()],
  // Keep Vite from clobbering the Rust compiler output in the terminal.
  clearScreen: false,
  server: {
    // Must match `devUrl` in src-tauri/tauri.conf.json.
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: {
      // Rust is watched by the Tauri CLI, not Vite.
      ignored: ["**/src-tauri/**"],
    },
  },
  // Produce a browser bundle Tauri can load; `frontendDist` points at `dist`.
  build: {
    target: "esnext",
    // Off: this only governs `vite build`, whose output ships inside the .app,
    // so maps here mean shipping readable frontend source and a bigger download
    // for something no end user can act on. The dev server is unaffected — it
    // generates its own sourcemaps through the transform pipeline.
    sourcemap: false,
  },
});
