import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      // Vite watches the project root, including gitignored files. Keep Loom
      // checkouts, local tools/state, and Rust build output out of its watcher.
      ignored: ["**/.worktrees/**", "**/.local/**", "**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "safari15",
    minify: "esbuild",
    sourcemap: false,
  },
});
