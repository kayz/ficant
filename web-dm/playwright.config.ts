import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./platform-shell/e2e",
  testMatch: "platform-shell.spec.ts",
  fullyParallel: false,
  retries: 0,
  reporter: "line",
  use: {
    baseURL: "http://127.0.0.1:4173",
    trace: "retain-on-failure",
    ...devices["Desktop Chrome"],
  },
  webServer: {
    command: "corepack pnpm@10.12.4 --filter @ficant/platform-shell dev --host 127.0.0.1 --port 4173",
    port: 4173,
    reuseExistingServer: false,
  },
});
