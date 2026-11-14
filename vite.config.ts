import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  root: "ui",
  plugins: [react()],
  clearScreen: false,
  build: { outDir: "../dist", emptyOutDir: true },
  server: {
    watch: {
      ignored: [
        "**/src-tauri/**",
        "**/src/**",
        "**/tests/**",
        "**/artifacts/**",
      ],
    },
  },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});
