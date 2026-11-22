import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Two pages: the main hub window and the floating dictation bar.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: "es2022",
    outDir: "dist",
    rollupOptions: {
      input: {
        main: new URL("./index.html", import.meta.url).pathname,
        bar: new URL("./bar.html", import.meta.url).pathname,
      },
    },
  },
  test: {
    globals: true,
    environment: "jsdom",
  },
});
