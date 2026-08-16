import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    // The Rust target directory can contain tens of thousands of files. Vite
    // does not need to watch it, and scanning it can leave the dev server
    // unresponsive on Windows before the WebView receives index.html.
    watch: {
      ignored: ["**/src-tauri/target/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
});
