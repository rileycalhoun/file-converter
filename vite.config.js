import { defineConfig } from "vite";

const isolationHeaders = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

export default defineConfig({
  clearScreen: false,
  server: {
    strictPort: true,
    headers: isolationHeaders,
  },
  preview: {
    headers: isolationHeaders,
  },
});
