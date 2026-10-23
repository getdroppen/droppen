// Builds the two-host move run's pages (`harness.tsx`) into one script,
// `dist/harness.js`: `pnpm exec vite build --config e2e/move/vite.config.ts`
// from `web/`.
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const here = fileURLToPath(new URL(".", import.meta.url));

export default defineConfig({
  root: here,
  plugins: [react()],
  resolve: { alias: { "@": fileURLToPath(new URL("../../src", import.meta.url)) } },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    minify: false,
    rollupOptions: {
      input: `${here}harness.tsx`,
      output: { format: "es", entryFileNames: "harness.js", inlineDynamicImports: true },
    },
  },
});
