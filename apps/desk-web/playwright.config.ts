import { existsSync } from 'node:fs';
import { chromium, defineConfig } from '@playwright/test';

export function resolveInstalledChromiumExecutable(): string {
  const explicit = process.env['PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH'];
  if (explicit && explicit.trim().length > 0) {
    if (!existsSync(explicit)) {
      throw new Error(
        `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH does not exist: ${explicit}`,
      );
    }
    return explicit;
  }

  try {
    const managed = chromium.executablePath();
    if (managed && existsSync(managed)) {
      return managed;
    }
  } catch {
    // Fall through to locally installed Chrome/Edge discovery without downloading.
  }

  const candidates =
    process.platform === 'win32'
      ? [
          'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
          'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
          'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
          'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
        ]
      : process.platform === 'darwin'
        ? [
            '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
            '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
          ]
        : [
            '/usr/bin/google-chrome',
            '/usr/bin/chromium',
            '/usr/bin/chromium-browser',
            '/usr/bin/microsoft-edge',
          ];

  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      return candidate;
    }
  }

  throw new Error(
    'No installed Chromium/Chrome/Edge browser found (checked PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH, Playwright cache, and system Chrome/Edge). Refusing to auto-download.',
  );
}

const executablePath = resolveInstalledChromiumExecutable();

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.spec.ts',
  outputDir: 'coverage/playwright-results',
  workers: 1,
  fullyParallel: false,
  retries: 0,
  timeout: 120_000,
  expect: {
    timeout: 15_000,
  },
  reporter: [['list']],
  use: {
    headless: true,
    launchOptions: {
      executablePath,
    },
    trace: 'off',
    video: 'off',
    screenshot: 'only-on-failure',
  },
});
