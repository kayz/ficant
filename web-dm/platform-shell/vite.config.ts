import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  base: process.env.FICANT_UI_BASE_PATH ?? "/",
  plugins: [react()],
  build: {
    sourcemap: true,
  },
  test: {
    environment: "jsdom",
    setupFiles: "./tests/setup.ts",
    include: ["tests/**/*.test.{ts,tsx}"],
    css: true,
    restoreMocks: true,
  },
});
