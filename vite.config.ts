import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and serves the built files from ui/dist.
export default defineConfig({
  root: "ui",
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**", "**/target/**"] } },
  build: { outDir: "dist", emptyOutDir: true, target: "es2022" },
});
