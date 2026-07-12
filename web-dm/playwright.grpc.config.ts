import { defineConfig, devices } from "@playwright/test";

const bearerToken = process.env.FICANT_GRPC_WEB_BEARER_TOKEN;

export default defineConfig({
  testDir: "./platform-shell/e2e",
  testMatch: "platform-shell.grpc.spec.ts",
  fullyParallel: false,
  retries: 0,
  reporter: "line",
  use: {
    baseURL: "http://127.0.0.1:4174",
    extraHTTPHeaders: bearerToken ? { Authorization: `Bearer ${bearerToken}` } : undefined,
    trace: "retain-on-failure",
    ...devices["Desktop Chrome"],
  },
  webServer: {
    command: "corepack pnpm@10.12.4 --filter @ficant/platform-shell dev --host 127.0.0.1 --port 4174",
    port: 4174,
    reuseExistingServer: false,
    env: {
      VITE_FICANT_GRPC_WEB_BASE_URL: process.env.FICANT_GRPC_WEB_BASE_URL ?? "http://127.0.0.1:50051",
    },
  },
});
