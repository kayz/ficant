#!/usr/bin/env node
import { spawn, spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { createServer } from 'node:net';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const DESK_WEB_ROOT = resolve(dirname(SCRIPT_PATH), '..');
const DIST_INDEX = join(DESK_WEB_ROOT, 'dist', 'index.html');
const VITE_BIN = join(DESK_WEB_ROOT, 'node_modules', 'vite', 'bin', 'vite.js');

const VALID_MODES = new Set(['scaffold', 'desk']);

function parseMode(argv) {
  const cleaned = argv.filter((arg) => arg !== '--');
  if (cleaned.length !== 1 || !cleaned[0] || !VALID_MODES.has(cleaned[0])) {
    throw new Error(
      `Usage: node scripts/smoke-mount.mjs [--] <scaffold|desk>. Received: ${JSON.stringify(argv)}`,
    );
  }
  return cleaned[0];
}

function getFreeLoopbackPort() {
  return new Promise((resolvePort, rejectPort) => {
    const server = createServer();
    server.unref();
    server.on('error', rejectPort);
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address();
      if (!addr || typeof addr === 'string') {
        server.close(() => rejectPort(new Error('Failed to obtain ephemeral loopback port.')));
        return;
      }
      const { port } = addr;
      server.close((closeErr) => {
        if (closeErr) {
          rejectPort(closeErr);
        } else {
          resolvePort(port);
        }
      });
    });
  });
}

function resolveInstalledBrowserExecutable() {
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
    // Fall through to system Chrome/Edge discovery without downloading.
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

async function waitForHttpReady(url, proc, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (proc.exitCode !== null) {
      throw new Error(
        `vite preview exited prematurely with code ${String(proc.exitCode)} before becoming ready.`,
      );
    }
    try {
      const response = await fetch(url);
      if (response.ok) {
        return;
      }
    } catch {
      // Server still binding.
    }
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`Timed out after ${timeoutMs}ms waiting for vite preview at ${url}.`);
}

async function terminateProcess(proc) {
  if (!proc || proc.exitCode !== null) {
    return;
  }
  const exited = new Promise((resolveExit) => {
    proc.once('exit', () => resolveExit());
  });

  if (process.platform === 'win32' && proc.pid) {
    spawnSync('taskkill', ['/PID', String(proc.pid), '/T', '/F'], {
      stdio: 'ignore',
      windowsHide: true,
    });
  } else {
    proc.kill('SIGTERM');
  }

  await Promise.race([
    exited,
    new Promise((resolveTimeout) => setTimeout(resolveTimeout, 3000)),
  ]);
  if (proc.exitCode === null) {
    proc.kill('SIGKILL');
  }
}

async function main() {
  const mode = parseMode(process.argv.slice(2));

  if (!existsSync(DIST_INDEX)) {
    throw new Error(
      `Missing built bundle at ${DIST_INDEX}. Run "pnpm --dir apps/desk-web run build" before smoke:mount.`,
    );
  }
  if (!existsSync(VITE_BIN)) {
    throw new Error(`Missing local vite CLI at ${VITE_BIN}.`);
  }

  const executablePath = resolveInstalledBrowserExecutable();
  const port = await getFreeLoopbackPort();
  const baseUrl = `http://127.0.0.1:${port}/`;

  let previewLogs = '';
  const previewProc = spawn(
    process.execPath,
    [VITE_BIN, 'preview', '--host', '127.0.0.1', '--port', String(port), '--strictPort'],
    {
      cwd: DESK_WEB_ROOT,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    },
  );
  previewProc.stdout.on('data', (chunk) => {
    previewLogs += chunk.toString('utf8');
  });
  previewProc.stderr.on('data', (chunk) => {
    previewLogs += chunk.toString('utf8');
  });

  let browser = null;
  try {
    await waitForHttpReady(baseUrl, previewProc, 15000);

    // Verify emitted trusted RulePack binary asset is served cleanly by preview.
    const rulepackRes = await fetch(`${baseUrl}rulepacks/cgb-futures-v2.bin`);
    if (!rulepackRes.ok) {
      throw new Error(
        `Expected /rulepacks/cgb-futures-v2.bin to return 200, got ${rulepackRes.status}.`,
      );
    }
    const rulepackBuf = new Uint8Array(await rulepackRes.arrayBuffer());
    if (rulepackBuf.byteLength !== 159) {
      throw new Error(
        `Expected /rulepacks/cgb-futures-v2.bin to be 159 bytes, got ${rulepackBuf.byteLength}.`,
      );
    }

    browser = await chromium.launch({
      executablePath,
      headless: true,
    });
    const context = await browser.newContext({
      viewport: { width: 1280, height: 800 },
    });
    const page = await context.newPage();

    const consoleErrors = [];
    const pageErrors = [];
    const failedRequests = [];
    const httpErrors = [];

    page.on('console', (msg) => {
      if (msg.type() === 'error') {
        consoleErrors.push(msg.text());
      }
    });
    page.on('pageerror', (err) => {
      pageErrors.push(err.message || String(err));
    });
    page.on('requestfailed', (req) => {
      failedRequests.push(`${req.method()} ${req.url()} (${req.failure()?.errorText || 'failed'})`);
    });
    page.on('response', (res) => {
      if (res.status() >= 400) {
        httpErrors.push(`${res.status()} ${res.url()}`);
      }
    });

    await page.goto(baseUrl, { waitUntil: 'networkidle', timeout: 20000 });

    if (mode === 'scaffold') {
      const root = page.locator('#root');
      await root.waitFor({ state: 'visible', timeout: 10000 });
      const heading = root.locator('h1').first();
      await heading.waitFor({ state: 'visible', timeout: 5000 });
      const headingText = (await heading.textContent())?.trim() || '';
      if (headingText.length === 0) {
        throw new Error('Mounted heading text is empty.');
      }
      const rulepackBadge = page.locator('[data-testid="trusted-rulepack-id"]');
      await rulepackBadge.waitFor({ state: 'visible', timeout: 5000 });
      const rulepackText = (await rulepackBadge.textContent())?.trim() || '';
      if (rulepackText.length === 0) {
        throw new Error('Trusted RulePack badge text is empty.');
      }
    } else if (mode === 'desk') {
      const shell = page.locator('[data-testid="desk-app-shell"]');
      await shell.waitFor({ state: 'visible', timeout: 10000 });

      const heading = shell.locator('h1').first();
      await heading.waitFor({ state: 'visible', timeout: 5000 });
      const headingText = (await heading.textContent())?.trim() || '';
      if (headingText.length === 0) {
        throw new Error('DeskApp heading text is empty.');
      }

      const rulepackBadge = shell.locator('[data-testid="trusted-rulepack-id"]');
      await rulepackBadge.waitFor({ state: 'visible', timeout: 5000 });

      // Assert credential entry form & input are visible in fresh unauthenticated session.
      const authForm = shell.locator('[data-testid="auth-form"]');
      await authForm.waitFor({ state: 'visible', timeout: 5000 });
      const tokenInput = shell.locator('#session-token-input');
      await tokenInput.waitFor({ state: 'visible', timeout: 5000 });
      const loginBtn = shell.locator('[data-testid="login-submit-btn"]');
      await loginBtn.waitFor({ state: 'visible', timeout: 5000 });

      // Assert pre-auth account ID staging control is visible immediately on initial empty mount.
      const preAuthAccountInput = shell.locator('[data-testid="cold-start-account-ids-input"]');
      await preAuthAccountInput.waitFor({ state: 'visible', timeout: 5000 });

      // Establish pure in-memory session (zero RPCs) to verify formal explicit account ID input
      // renders cleanly without waiting for any backend snapshot or data-snapshot-seq.
      await tokenInput.fill('smoke-inmemory-token');
      await loginBtn.click();

      const accountsForm = shell.locator('[data-testid="accounts-form"]');
      await accountsForm.waitFor({ state: 'visible', timeout: 5000 });
      const explicitAccountInput = shell.locator('#explicit-account-ids-input');
      await explicitAccountInput.waitFor({ state: 'visible', timeout: 5000 });

      const connBadge = shell.locator('[data-testid="connection-badge"]');
      const rawConnState = await connBadge.getAttribute('data-raw-connection-state');
      if (rawConnState !== 'idle_no_accounts') {
        throw new Error(
          `Expected connection state to remain "idle_no_accounts" without backend, got ${String(rawConnState)}.`,
        );
      }
    }

    if (consoleErrors.length > 0) {
      throw new Error(`Console errors detected:\n${consoleErrors.join('\n')}`);
    }
    if (pageErrors.length > 0) {
      throw new Error(`Page errors detected:\n${pageErrors.join('\n')}`);
    }
    if (failedRequests.length > 0) {
      throw new Error(`Failed asset requests detected:\n${failedRequests.join('\n')}`);
    }
    if (httpErrors.length > 0) {
      throw new Error(`HTTP >= 400 responses detected:\n${httpErrors.join('\n')}`);
    }

    console.log(
      `[smoke-mount] PASS (${mode}): mounted ${baseUrl} via ${executablePath}; 0 console/page/network errors.`,
    );
  } catch (err) {
    if (previewLogs.trim().length > 0) {
      console.error(`[smoke-mount] vite preview logs:\n${previewLogs.trim()}`);
    }
    throw err;
  } finally {
    if (browser) {
      try {
        await browser.close();
      } catch {
        // Ignore close error during cleanup.
      }
    }
    await terminateProcess(previewProc);
  }
}

main().catch((err) => {
  console.error(`[smoke-mount] ERROR: ${err instanceof Error ? err.message : String(err)}`);
  process.exit(1);
});
