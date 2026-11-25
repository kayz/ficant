#!/usr/bin/env node
import { existsSync, writeFileSync } from 'node:fs';
import { chromium } from '@playwright/test';

const SCALE_12_FACTOR = 1_000_000_000_000n;
const PRINCIPAL_ROLE_TRADER = '1';
const PENDING_MATCH_ACTION_ACCEPT_ORPHAN = '3';

function parseArgs(argv) {
  const opts = {
    baseUrl: '',
    token: '',
    tokenEnv: '',
    accountId: '',
    draftId: '',
    expectedBondCode: 'BOND_10Y',
    expectedPreTaxDv01: '',
    expectedDv01Unit: '',
    orphanInstrumentCode: 'BOND_5Y',
    reportOut: '',
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--') {
      continue;
    }
    const next = () => {
      i += 1;
      if (i >= argv.length || argv[i] === undefined) {
        throw new Error(`Missing value for argument ${arg}`);
      }
      return argv[i];
    };
    switch (arg) {
      case '--base-url':
        opts.baseUrl = next();
        break;
      case '--token':
        opts.token = next();
        break;
      case '--token-env':
        opts.tokenEnv = next();
        break;
      case '--account-id':
        opts.accountId = next();
        break;
      case '--draft-id':
        opts.draftId = next();
        break;
      case '--expected-bond-code':
        opts.expectedBondCode = next();
        break;
      case '--expected-pre-tax-dv01':
        opts.expectedPreTaxDv01 = next();
        break;
      case '--expected-dv01-unit':
        opts.expectedDv01Unit = next();
        break;
      case '--orphan-instrument-code':
        opts.orphanInstrumentCode = next();
        break;
      case '--report-out':
        opts.reportOut = next();
        break;
      default:
        throw new Error(`Unsupported argument: ${arg}`);
    }
  }

  if (!opts.baseUrl) {
    throw new Error('--base-url is required');
  }
  if (!opts.token && opts.tokenEnv) {
    opts.token = process.env[opts.tokenEnv] ?? '';
  }
  if (!opts.token) {
    throw new Error('Either --token or --token-env pointing to a non-empty env var is required');
  }
  if (!opts.accountId) {
    throw new Error('--account-id is required');
  }
  if (!opts.draftId) {
    throw new Error('--draft-id is required');
  }
  return opts;
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

function parseExactScaledDecimal12(rawText, label) {
  const cleaned = rawText.replace(/,/g, '').trim();
  const match = /^(-)?([0-9]+)(?:\.([0-9]+))?$/.exec(cleaned);
  if (!match) {
    throw new Error(`Invalid decimal string for ${label}: ${JSON.stringify(rawText)}`);
  }
  const neg = match[1] === '-';
  const intPart = match[2] ?? '0';
  const fracPart = match[3] ?? '';
  if (fracPart.length > 12) {
    throw new Error(`Decimal scale > 12 for ${label}: ${JSON.stringify(rawText)}`);
  }
  const paddedFrac = fracPart.padEnd(12, '0');
  const scaledAbs = BigInt(intPart) * SCALE_12_FACTOR + BigInt(paddedFrac);
  return neg ? -scaledAbs : scaledAbs;
}

async function waitForCondition(fn, timeoutMs, description) {
  const deadline = Date.now() + timeoutMs;
  let lastDetail = '';
  while (Date.now() < deadline) {
    const res = await fn();
    if (res.ok) {
      return res.value;
    }
    lastDetail = res.detail ?? '';
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`Timed out after ${timeoutMs}ms waiting for ${description}${lastDetail ? ` (last: ${lastDetail})` : ''}`);
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const executablePath = resolveInstalledBrowserExecutable();

  let browser = null;
  const consoleErrors = [];
  const pageErrors = [];
  const failedRequests = [];
  const httpErrors = [];
  const corsHeaderViolations = [];
  let totalResponses = 0;
  let grpcWebResponses = 0;

  try {
    browser = await chromium.launch({
      executablePath,
      headless: true,
    });
    const context = await browser.newContext({
      viewport: { width: 1440, height: 900 },
    });
    const page = await context.newPage();

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
      totalResponses += 1;
      const url = res.url();
      const headers = res.headers();
      const contentType = headers['content-type'] ?? '';
      if (contentType.includes('application/grpc-web') || url.includes('/ficant.')) {
        grpcWebResponses += 1;
      }
      if (res.status() >= 400) {
        httpErrors.push(`${res.status()} ${url}`);
      }
      for (const headerName of Object.keys(headers)) {
        if (headerName.toLowerCase().startsWith('access-control-allow-')) {
          corsHeaderViolations.push(`${url}: ${headerName}=${headers[headerName]}`);
        }
      }
    });

    await page.goto(opts.baseUrl, { waitUntil: 'networkidle', timeout: 25000 });

    const shell = page.getByTestId('desk-app-shell');
    await shell.waitFor({ state: 'visible', timeout: 10000 });
    const rulepackBadge = page.getByTestId('trusted-rulepack-id');
    await rulepackBadge.waitFor({ state: 'visible', timeout: 5000 });
    const rulepackBadgeText = ((await rulepackBadge.textContent()) ?? '').trim();

    // 1. Log in as TRADER directly against single-binary same-origin gRPC-Web endpoint.
    await page.getByTestId('auth-form').waitFor({ state: 'visible', timeout: 10000 });
    await page.locator('input[name="credential-header-mode"][value="authorization"]').check();
    await page.locator('#session-token-input').fill(opts.token);
    await page.locator('#session-role-hint-select').selectOption(PRINCIPAL_ROLE_TRADER);
    await page.getByTestId('login-submit-btn').click();
    await page.getByTestId('accounts-form').waitFor({ state: 'visible', timeout: 10000 });

    // 2. Load target account and verify live same-origin stream.
    await page.locator('#explicit-account-ids-input').fill(opts.accountId);
    await page.getByTestId('submit-accounts-btn').click();
    await page.getByTestId(`account-tab-${opts.accountId}`).waitFor({ state: 'visible', timeout: 15000 });

    await waitForCondition(
      async () => {
        const attr = await page.getByTestId('connection-badge').getAttribute('data-raw-connection-state');
        return { ok: attr === 'stream_live', value: attr, detail: String(attr) };
      },
      15000,
      'connection-badge data-raw-connection-state === stream_live',
    );

    // 3. Verify BOND_10Y coverage status (COVERED_PRE_TAX), tax basis (PRE_TAX), and non-zero pre_tax_dv01.
    const bondRow = page.getByTestId(`position-row-${opts.expectedBondCode}`);
    await bondRow.waitFor({ state: 'visible', timeout: 10000 });

    // Switch PositionsGrid coverage filter to 'covered' (which filters strictly for CoverageStatus.COVERED_PRE_TAX).
    await page.getByTestId('positions-coverage-filter').selectOption('covered');
    await bondRow.waitFor({ state: 'visible', timeout: 5000 });
    const bondRowText = ((await bondRow.textContent()) ?? '').trim();
    await page.getByTestId('positions-coverage-filter').selectOption('all');

    if (bondRowText.includes('UNCOVERED') || bondRowText.includes('未覆盖')) {
      throw new Error(`Expected ${opts.expectedBondCode} to be COVERED_PRE_TAX, but row rendered: ${bondRowText}`);
    }
    const dv01Match = /PRE_TAX\s*·\s*DV01:\s*(-?[0-9,]+(?:\.[0-9]+)?)/.exec(bondRowText);
    if (!dv01Match || !dv01Match[1]) {
      throw new Error(`Expected PRE_TAX · DV01 numeric rendering in ${opts.expectedBondCode} row, got: ${bondRowText}`);
    }
    const renderedPositionDv01Text = dv01Match[1];
    const renderedPositionDv01Scaled = parseExactScaledDecimal12(
      renderedPositionDv01Text,
      `${opts.expectedBondCode} rendered pre_tax_dv01`,
    );
    if (renderedPositionDv01Scaled === 0n) {
      throw new Error(`Expected non-zero rendered pre_tax_dv01 for ${opts.expectedBondCode}, got ${renderedPositionDv01Text}`);
    }
    if (opts.expectedPreTaxDv01) {
      const expectedScaled = parseExactScaledDecimal12(
        opts.expectedPreTaxDv01,
        'expected server pre_tax_dv01',
      );
      if (renderedPositionDv01Scaled !== expectedScaled) {
        throw new Error(
          `Rendered ${opts.expectedBondCode} pre_tax_dv01 (${renderedPositionDv01Text}) does not match server snapshot (${opts.expectedPreTaxDv01})`,
        );
      }
    }

    // 4. Open Draft Drawer, select exact Sidecar draft_id, verify covered non-zero unit-bearing preview, and confirm.
    await page.getByTestId('draft-drawer-toggle-btn').click();
    const drawer = page.getByTestId('draft-drawer-panel');
    await drawer.waitFor({ state: 'visible', timeout: 10000 });
    await page.getByTestId('snapshot-pending-drafts').waitFor({ state: 'visible', timeout: 10000 });

    const draftSelectBtn = page.getByTestId(`select-pending-draft-${opts.draftId}`);
    await draftSelectBtn.waitFor({ state: 'visible', timeout: 10000 });
    await draftSelectBtn.click();

    await page.getByTestId('draft-confirmation-section').waitFor({ state: 'visible', timeout: 10000 });
    const activeDraftIdText = ((await page.getByTestId('active-draft-id').textContent()) ?? '').trim();
    if (activeDraftIdText !== opts.draftId) {
      throw new Error(`Active draft ID mismatch: expected ${opts.draftId}, got ${activeDraftIdText}`);
    }
    if ((await page.getByTestId('draft-lineage-invalid').count()) > 0) {
      throw new Error('Draft regulatory RuleLineage rendered invalid');
    }

    await page.getByTestId('preview-covered-nonzero').waitFor({ state: 'visible', timeout: 10000 });
    const previewDv01Text = ((await page.getByTestId('preview-dv01-value').textContent()) ?? '').trim();
    const previewDv01UnitText = ((await page.getByTestId('preview-dv01-unit').textContent()) ?? '').trim();
    const previewDv01Scaled = parseExactScaledDecimal12(previewDv01Text, 'draft preview pre_tax_dv01');
    if (previewDv01Scaled === 0n) {
      throw new Error(`Expected non-zero draft preview pre_tax_dv01, got ${previewDv01Text}`);
    }
    if (opts.expectedDv01Unit && !previewDv01UnitText.includes(opts.expectedDv01Unit)) {
      throw new Error(
        `Draft preview DV01 unit (${previewDv01UnitText}) does not include expected unit ULID (${opts.expectedDv01Unit})`,
      );
    }
    if ((await page.getByTestId('acknowledge-uncovered-checkbox').count()) !== 0) {
      throw new Error('Unexpected uncovered acknowledgement checkbox rendered for covered BOND_10Y draft');
    }

    const confirmBtn = page.getByTestId('draft-confirm-button');
    await waitForCondition(
      async () => {
        const disabled = await confirmBtn.isDisabled();
        return { ok: !disabled, value: !disabled, detail: `disabled=${String(disabled)}` };
      },
      5000,
      'draft-confirm-button enabled',
    );
    await confirmBtn.click();

    const confirmReceipt = page.getByTestId('draft-confirm-receipt');
    await confirmReceipt.waitFor({ state: 'visible', timeout: 15000 });
    const confirmReceiptText = ((await confirmReceipt.textContent()) ?? '').trim();
    await waitForCondition(
      async () => {
        const stage = await drawer.getAttribute('data-draft-stage');
        return { ok: stage === 'confirmed', value: stage, detail: String(stage) };
      },
      5000,
      'draft-drawer-panel data-draft-stage === confirmed',
    );
    await page.getByTestId('draft-drawer-close').click();
    await drawer.waitFor({ state: 'hidden', timeout: 5000 });

    // 5. Execute ACCEPT_ORPHAN on BOND_5Y's open pending match from PendingMatchPanel.
    const pmPanel = page.getByTestId('pending-match-panel');
    await pmPanel.waitFor({ state: 'visible', timeout: 10000 });

    const orphanButton = pmPanel
      .locator('button[data-testid^="pending-match-item-"]')
      .filter({ hasText: opts.orphanInstrumentCode })
      .first();
    await orphanButton.waitFor({ state: 'visible', timeout: 10000 });
    const orphanTestId = (await orphanButton.getAttribute('data-testid')) ?? '';
    const resolvedPendingMatchId = orphanTestId.replace(/^pending-match-item-/, '');
    await orphanButton.click();

    const acceptOrphanRadio = page.locator(`#pm-action-${PENDING_MATCH_ACTION_ACCEPT_ORPHAN}`);
    await acceptOrphanRadio.waitFor({ state: 'visible', timeout: 5000 });
    await acceptOrphanRadio.check();
    await page.locator('#pm-reason').fill('bp4-single-binary-ui-accept-orphan');
    await page.getByTestId('submit-resolve-pending-match').click();

    await page.getByTestId('pending-match-empty').waitFor({ state: 'visible', timeout: 15000 });
    const allocatedCard = page
      .locator('[data-testid="post-submit-attribution-section"] article[data-attribution-state="allocated"]')
      .first();
    await allocatedCard.waitFor({ state: 'visible', timeout: 15000 });

    if (consoleErrors.length > 0) {
      throw new Error(`Console errors detected:\n${consoleErrors.join('\n')}`);
    }
    if (pageErrors.length > 0) {
      throw new Error(`Page errors detected:\n${pageErrors.join('\n')}`);
    }
    if (failedRequests.length > 0) {
      throw new Error(`Failed network requests detected:\n${failedRequests.join('\n')}`);
    }
    if (httpErrors.length > 0) {
      throw new Error(`HTTP >= 400 responses detected:\n${httpErrors.join('\n')}`);
    }
    if (corsHeaderViolations.length > 0) {
      throw new Error(`Forbidden Access-Control-Allow-* headers detected:\n${corsHeaderViolations.join('\n')}`);
    }
    if (grpcWebResponses === 0) {
      throw new Error('Expected same-origin gRPC-Web responses, observed 0');
    }

    const report = {
      status: 'pass',
      base_url: opts.baseUrl,
      browser_executable: executablePath,
      rulepack_badge: rulepackBadgeText,
      account_id: opts.accountId,
      verified_bond_code: opts.expectedBondCode,
      rendered_position_pre_tax_dv01: renderedPositionDv01Text,
      confirmed_draft_id: activeDraftIdText,
      draft_preview_dv01: previewDv01Text,
      draft_preview_dv01_unit: previewDv01UnitText,
      draft_confirm_receipt: confirmReceiptText,
      resolved_orphan_instrument: opts.orphanInstrumentCode,
      resolved_pending_match_id: resolvedPendingMatchId,
      total_responses: totalResponses,
      grpc_web_responses: grpcWebResponses,
      console_errors: 0,
      page_errors: 0,
      failed_requests: 0,
      http_errors: 0,
      cors_header_violations: 0,
    };

    const jsonText = JSON.stringify(report, null, 2);
    if (opts.reportOut) {
      writeFileSync(opts.reportOut, `${jsonText}\n`, 'utf8');
    }
    console.log(jsonText);
  } finally {
    if (browser) {
      try {
        await browser.close();
      } catch {
        // Ignore browser close error on cleanup.
      }
    }
  }
}

main().catch((err) => {
  console.error(`[verify-single-binary-ui] ERROR: ${err instanceof Error ? err.message : String(err)}`);
  process.exit(1);
});
