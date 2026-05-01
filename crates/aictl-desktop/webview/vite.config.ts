import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// Tauri 2 dev workflow: the desktop binary launches Vite via
// `beforeDevCommand` and points its webview at `devUrl`. The bundler
// must therefore listen on a stable port (5173 here) and emit assets
// the macOS WebKit can load (`base: "./"` keeps relative URLs).
export default defineConfig({
  plugins: [solid()],
  base: "./",
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "safari16",
    sourcemap: true,
    minify: "esbuild",
  },
});
