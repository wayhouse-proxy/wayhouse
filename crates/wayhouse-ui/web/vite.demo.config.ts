import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The interactive demo: `npm run build:demo` writes dist-demo/ (see src/demo/). Separate from vite.config.ts,
// so the real build never includes the mock backend.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  base: "./",
  build: { outDir: "dist-demo", emptyOutDir: true, rollupOptions: { input: "demo.html", output: { inlineDynamicImports: true } } },
});
