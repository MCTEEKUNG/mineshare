import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(({ mode }) => ({
  plugins: [react(), {
    name: "mineshare-design-preview",
    apply: "serve",
    transformIndexHtml() {
      if (mode !== "design-preview") return;
      return [{ tag: "script", children: readFileSync(new URL("./dev/preview.js", import.meta.url), "utf8"), injectTo: "head-prepend" }];
    },
  }],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "es2022",
    minify: "esbuild",
  },
}));
