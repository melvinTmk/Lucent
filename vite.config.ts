import { defineConfig } from "vite";
import { resolve } from "node:path";
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

export default defineConfig(() => ({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    rollupOptions: {
      input: {
        dock: resolve(import.meta.dirname, "dock.html"),
        thumbs: resolve(import.meta.dirname, "thumbs.html"),
        settings: resolve(import.meta.dirname, "settings.html"),
        search: resolve(import.meta.dirname, "search.html"),
        switcher: resolve(import.meta.dirname, "switcher.html"),
        overview: resolve(import.meta.dirname, "overview.html"),
      },
    },
  },
}));
