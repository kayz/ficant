import { create } from '@bufbuild/protobuf';
import { expect, test, type Page } from '@playwright/test';
import { join } from 'node:path';
import {
  InstrumentCatalogEntrySchema,
  OpeningInstrumentBalanceSchema,
  PendingMatchResolutionAction,
  PrincipalRole,
  RateCoverageEligibility,
  SettlementBucket,
  TradeFeedRecordAction,
  TradeSide,
  type InstrumentCatalogEntry,
  type OpeningInstrumentBalance,
} from '../src/generated/ficant/blotter/v1/model_pb';
import { CalendarRequirement } from '../src/generated/ficant/rates/v1/analytics_pb';
import { startPreviewWithProxy, type PreviewWithProxyHandle } from './controllable-proxy';
import {
  BAD_TOKEN,
  NLP_TEMPLATES_CSV_PATH,
  REFERENCE_UNITS,
  TRADER_A_ONLY_TOKEN,
  TRADER_TOKEN,
  buildRegulatoryRulePackBinding,
  createAdminClients,
  createIsolatedTempDir,
  makeBaseEvidence,
  makeDec,
  makeOpeningBase,
  makeQuoteTermsClean,
  makeQuoteTick,
  makeStandardBond,
  makeTradeFill,
  removeIsolatedTempDir,
  rollbackHoldExpiryInSqlite,
  startFicantServer,
  type FicantServerHandle,
} from './server-harness';

const CROCKFORD32 = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';

function crockfordSuffix(index: number): string {
  let n = index;
  let out = '';
  for (let i = 0; i < 4; i += 1) {
    out = CROCKFORD32[n % 32] + out;
    n = Math.floor(n / 32);
  }
  return out;
}

async function loginInUi(
  page: Page,
  opts: {
    readonly token: string;
    readonly headerMode?: 'authorization' | 'x-ficant-credential';
    readonly roleHint?: PrincipalRole;
  },
): Promise<void> {
  const authForm = page.getByTestId('auth-form');
  if (!(await authForm.isVisible())) {
    await page.getByTestId('toggle-switch-auth-btn').click();
    await expect(authForm).toBeVisible();
  }
  const mode = opts.headerMode ?? 'authorization';
  await page.locator(`input[name="credential-header-mode"][value="${mode}"]`).check();
  await page.locator('#session-token-input').fill(opts.token);
  await page
    .locator('#session-role-hint-select')
    .selectOption(String(opts.roleHint ?? PrincipalRole.TRADER));
  await page.getByTestId('login-submit-btn').click();
  await expect(page.getByTestId('accounts-form')).toBeVisible();
}

async function submitAccountsInUi(page: Page, accountsCsv: string): Promise<void> {
  await page.locator('#explicit-account-ids-input').fill(accountsCsv);
  await page.getByTestId('submit-accounts-btn').click();
}

async function dispatchExplicitPaste(page: Page, text: string): Promise<void> {
  const textarea = page.getByTestId('draft-paste-textarea');
  await expect(textarea).toBeVisible();
  await textarea.evaluate((el, payload) => {
    const dt = new DataTransfer();
    dt.setData('text/plain', payload);
    const ev = new ClipboardEvent('paste', {
      clipboardData: dt,
      bubbles: true,
      cancelable: true,
    });
    el.dispatchEvent(ev);
  }, text);
}

function attachPageErrorCollector(page: Page): string[] {
  const errors: string[] = [];
  page.on('pageerror', (err) => {
    const msg = err.stack || err.message || String(err);
    console.error('[E2E pageerror]', msg);
    errors.push(err.message || String(err));
  });
  return errors;
}

test.describe.serial('Desk Blotter Real-Server E2E Suites', () => {
  // ==========================================================================
  // Suite 1: Cold Start, Auth/RBAC, Dynamic Policy B/A, Real PreviewCoveredHedge
  //          TS 2,000,000 Notional, Late-B Clear, Active Stream Cut, Retention-2
  //          Dual-Read-Block Recovery & Idle Authorized Account Cold Start
  // ==========================================================================
  test('Suite 1: Auth/RBAC, Policy B/A, TS 2,000,000, Late-B Clear, Stream Cut & Retention-2 Recovery', async ({
    page,
  }) => {
    const pageErrors = attachPageErrorCollector(page);
    const servers: FicantServerHandle[] = [];
    let preview: PreviewWithProxyHandle | null = null;

    try {
      // --- Part 1A: Embedded Default Policy B (no FICANT_BLOTTER_POLICY_PATH) ---
      const srvPolicyB = await startFicantServer({
        mode: 'ephemeral',
        policy: 'embedded_default',
        accounts: ['acct-A', 'acct-B', 'acct-idle'],
        sources: ['FEED-A', 'FEED-B', 'FEED-IDLE', 'primary-quote-feed'],
        traderAOnlyAccounts: ['acct-A'],
        watchdogIntervalMs: 0,
        nlpCsvPath: NLP_TEMPLATES_CSV_PATH,
      });
      servers.push(srvPolicyB);

      const adminB = createAdminClients(srvPolicyB.origin);
      await adminB.operator.importOpeningBase({
        idempotencyKey: 's1-imp-a',
        openingBase: makeOpeningBase({
          accountId: 'acct-A',
          baseSuffix: 'S10A',
          sourceId: 'FEED-A',
        }),
        expectedAccountVersion: 0n,
      });
      await adminB.operator.importOpeningBase({
        idempotencyKey: 's1-imp-b',
        openingBase: makeOpeningBase({
          accountId: 'acct-B',
          baseSuffix: 'S10B',
          sourceId: 'FEED-B',
        }),
        expectedAccountVersion: 0n,
      });
      await adminB.operator.importOpeningBase({
        idempotencyKey: 's1-imp-idle',
        openingBase: makeOpeningBase({
          accountId: 'acct-idle',
          baseSuffix: 'S10C',
          sourceId: 'FEED-IDLE',
        }),
        expectedAccountVersion: 0n,
      });
      await adminB.trader.ingestMarketQuotes({
        idempotencyKey: 's1-q-1',
        accountId: 'acct-A',
        sourceId: 'primary-quote-feed',
        ticks: [
          makeQuoteTick({
            sourceId: 'primary-quote-feed',
            quoteSeq: 1n,
            instrumentCode: 'BOND_10Y',
            ytm: '0.025',
          }),
        ],
      });

      preview = await startPreviewWithProxy(srvPolicyB.origin);
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.goto(preview.baseUrl, { waitUntil: 'networkidle' });

      // Unauthenticated initial state: zero event RPCs
      await expect(page.getByTestId('desk-app-shell')).toBeVisible();
      await expect(page.getByTestId('auth-form')).toBeVisible();
      expect(preview.proxy.getRecordedRequests().length).toBe(0);

      // Invalid token via x-ficant-credential -> UNAUTHENTICATED + zero storage/URL leaks
      await loginInUi(page, { token: BAD_TOKEN, headerMode: 'x-ficant-credential' });
      await submitAccountsInUi(page, 'acct-A');
      await expect(page.getByTestId('session-error-banner')).toContainText('UNAUTHENTICATED');

      const storageLeakCheck = await page.evaluate((tok) => {
        const ls = JSON.stringify(window.localStorage);
        const ss = JSON.stringify(window.sessionStorage);
        const ck = document.cookie;
        const href = window.location.href;
        return ls.includes(tok) || ss.includes(tok) || ck.includes(tok) || href.includes(tok);
      }, BAD_TOKEN);
      expect(storageLeakCheck).toBe(false);

      // Valid trader via x-ficant-credential: empty account list rejected with 0 event RPCs
      preview.proxy.clearRecordedRequests();
      await loginInUi(page, { token: TRADER_TOKEN, headerMode: 'x-ficant-credential' });
      await submitAccountsInUi(page, '   ');
      await expect(page.getByTestId('accounts-form-error')).toBeVisible();
      expect(preview.proxy.getRecordedRequests().length).toBe(0);

      // Load acct-A with x-ficant-credential header and verify header exclusivity on wire
      await submitAccountsInUi(page, 'acct-A');
      await expect(page.getByTestId('account-tab-acct-A')).toBeVisible();
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'stream_live',
      );
      const customHdrReqs = preview.proxy.getRecordedRequests();
      expect(customHdrReqs.length).toBeGreaterThan(0);
      for (const req of customHdrReqs) {
        expect(req.hasCustomCredential).toBe(true);
        expect(req.hasAuthorization).toBe(false);
      }

      // RBAC FORBIDDEN: trader-a-only requests acct-A, acct-B -> acct-B FORBIDDEN -> remove & load
      await loginInUi(page, { token: TRADER_A_ONLY_TOKEN, headerMode: 'authorization' });
      await submitAccountsInUi(page, 'acct-A, acct-B');
      await expect(page.getByTestId('account-error-acct-B')).toContainText('FORBIDDEN');
      await page.getByTestId('remove-failed-account-acct-B').click();
      await expect(page.getByTestId('account-tab-acct-A')).toBeVisible();
      await expect(page.getByTestId('account-tab-acct-B')).toHaveCount(0);

      // Dynamic Policy B: 4 products ["T", "TF", "TL", "TS"] & real TS contract_notional = 2,000,000
      const productOptionsB = await page
        .getByTestId('hedge-product-select')
        .locator('option')
        .allTextContents();
      expect(productOptionsB).toEqual(['T', 'TF', 'TL', 'TS']);

      await page.getByTestId('hedge-product-select').selectOption('TS');
      await page.getByTestId('hedge-contract-input').fill('TS2609');
      await page.getByTestId('hedge-ctd-dv01-input').fill('0.02');
      await page.getByTestId('hedge-cf-input').fill('0.985');
      await page.getByTestId('hedge-preview-submit-btn').click();

      await expect(page.getByTestId('hedge-preview-result')).toBeVisible();
      await expect(page.getByTestId('hedge-contract-notional')).toContainText('2,000,000');
      await expect(page.getByTestId('hedge-result-metadata')).toContainText('product=TS');
      await expect(page.getByTestId('hedge-result-metadata')).toContainText('tax_basis=PRE_TAX');

      // Missing regulatory_rule_pack on embedded Policy B -> draft submit rejected, no ack checkbox
      await page.getByTestId('draft-drawer-toggle-btn').click();
      await expect(page.getByTestId('draft-drawer-panel')).toBeVisible();
      await dispatchExplicitPaste(page, '无监管包测试买入BOND_10Y面额一千万');
      await expect(page.getByTestId('draft-proposal-review')).toBeVisible();
      await page.getByTestId('draft-submit-button').click();
      await expect(
        page.locator('[data-testid="draft-regulatory-error"], [data-testid="draft-submit-error"]'),
      ).toBeVisible();
      await expect(page.getByTestId('acknowledge-uncovered-checkbox')).toHaveCount(0);
      await page.getByTestId('draft-drawer-close').click();

      // Late-B isolation on session switch: delay acct-B snapshot, switch to trader-a-only, release B
      await loginInUi(page, { token: TRADER_TOKEN, headerMode: 'authorization' });
      const lateBGate = preview.proxy.delaySnapshotForAccount('acct-B');
      await submitAccountsInUi(page, 'acct-A, acct-B');
      await lateBGate.arrived;

      // Switch session while acct-B is still in-flight!
      await loginInUi(page, { token: TRADER_A_ONLY_TOKEN, headerMode: 'authorization' });
      await submitAccountsInUi(page, 'acct-A');
      await expect(page.getByTestId('account-tab-acct-A')).toBeVisible();
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'stream_live',
      );

      // Now release late B response and assert acct-B NEVER leaks into new session
      lateBGate.release();
      await page.waitForTimeout(300);
      await expect(page.getByTestId('account-tab-acct-B')).toHaveCount(0);
      await expect(page.getByTestId('session-error-banner')).toHaveCount(0);

      // --- Part 1B: Policy A Override (["T", "TF", "TL"], zero-net DV01, empty covered, uncovered alerts) ---
      const srvPolicyA = await startFicantServer({
        mode: 'ephemeral',
        policy: {
          profile: 'A',
          allowedHedgeProductCodes: ['T', 'TF', 'TL'],
          defaultQuoteSourceId: 'primary-quote-feed',
        },
        accounts: ['acct-zero-dv01', 'acct-empty-cov', 'acct-zero-net-uncov'],
        sources: ['FEED-Z1', 'FEED-Z2', 'FEED-Z3', 'primary-quote-feed'],
        watchdogIntervalMs: 0,
      });
      servers.push(srvPolicyA);

      const adminA = createAdminClients(srvPolicyA.origin);
      const u = REFERENCE_UNITS;

      // 1) acct-zero-dv01: two identical covered bonds (+10M and -10M) => covered_instrument_count=2, net DV01=0!
      await adminA.operator.importOpeningBase({
        idempotencyKey: 'pa-imp-zero-dv01',
        openingBase: makeOpeningBase({
          accountId: 'acct-zero-dv01',
          baseSuffix: 'PA01',
          sourceId: 'FEED-Z1',
          balances: [
            create(OpeningInstrumentBalanceSchema, {
              instrumentCode: 'B_LONG',
              openingEconomicNotional: makeDec('10000000', u.currencyAmount!),
              openingAvailableNotional: makeDec('10000000', u.currencyAmount!),
              openingNetT0Notional: makeDec('0', u.currencyAmount!),
              openingNetT1Notional: makeDec('0', u.currencyAmount!),
            }),
            create(OpeningInstrumentBalanceSchema, {
              instrumentCode: 'B_SHORT',
              openingEconomicNotional: makeDec('-10000000', u.currencyAmount!),
              openingAvailableNotional: makeDec('0', u.currencyAmount!),
              openingNetT0Notional: makeDec('0', u.currencyAmount!),
              openingNetT1Notional: makeDec('0', u.currencyAmount!),
            }),
          ],
          instruments: [
            create(InstrumentCatalogEntrySchema, {
              instrumentCode: 'B_LONG',
              bond: makeStandardBond('PA11'),
              eligibilityClass: RateCoverageEligibility.SOVEREIGN_OR_POLICY_FIXED,
              calendarRequirement: CalendarRequirement.UNSPECIFIED,
            }),
            create(InstrumentCatalogEntrySchema, {
              instrumentCode: 'B_SHORT',
              bond: makeStandardBond('PA12'),
              eligibilityClass: RateCoverageEligibility.SOVEREIGN_OR_POLICY_FIXED,
              calendarRequirement: CalendarRequirement.UNSPECIFIED,
            }),
          ],
        }),
        expectedAccountVersion: 0n,
      });
      await adminA.trader.ingestMarketQuotes({
        idempotencyKey: 'pa-q-1',
        accountId: 'acct-zero-dv01',
        sourceId: 'primary-quote-feed',
        ticks: [
          makeQuoteTick({
            sourceId: 'primary-quote-feed',
            quoteSeq: 1n,
            instrumentCode: 'B_LONG',
            ytm: '0.025',
          }),
          makeQuoteTick({
            sourceId: 'primary-quote-feed',
            quoteSeq: 1n,
            instrumentCode: 'B_SHORT',
            ytm: '0.025',
          }),
        ],
      });

      // 2) acct-empty-cov: pure short uncovered bond (-10M) -> covered_instrument_count == 0, uncovered net < 0
      await adminA.operator.importOpeningBase({
        idempotencyKey: 'pa-imp-empty-cov',
        openingBase: makeOpeningBase({
          accountId: 'acct-empty-cov',
          baseSuffix: 'PA02',
          sourceId: 'FEED-Z2',
          balances: [
            create(OpeningInstrumentBalanceSchema, {
              instrumentCode: 'UNCOV_SHORT',
              openingEconomicNotional: makeDec('-10000000', u.currencyAmount!),
              openingAvailableNotional: makeDec('0', u.currencyAmount!),
              openingNetT0Notional: makeDec('0', u.currencyAmount!),
              openingNetT1Notional: makeDec('0', u.currencyAmount!),
            }),
          ],
          instruments: [
            create(InstrumentCatalogEntrySchema, {
              instrumentCode: 'UNCOV_SHORT',
              bond: makeStandardBond('PA20'),
              eligibilityClass: RateCoverageEligibility.UNSPECIFIED,
              calendarRequirement: CalendarRequirement.UNSPECIFIED,
            }),
          ],
        }),
        expectedAccountVersion: 0n,
      });

      // 3) acct-zero-net-uncov: two uncovered bonds (+10M and -10M => net == 0, gross == 20M, count == 2)
      await adminA.operator.importOpeningBase({
        idempotencyKey: 'pa-imp-zero-net-uncov',
        openingBase: makeOpeningBase({
          accountId: 'acct-zero-net-uncov',
          baseSuffix: 'PA03',
          sourceId: 'FEED-Z3',
          balances: [
            create(OpeningInstrumentBalanceSchema, {
              instrumentCode: 'UNCOV_L',
              openingEconomicNotional: makeDec('10000000', u.currencyAmount!),
              openingAvailableNotional: makeDec('10000000', u.currencyAmount!),
              openingNetT0Notional: makeDec('0', u.currencyAmount!),
              openingNetT1Notional: makeDec('0', u.currencyAmount!),
            }),
            create(OpeningInstrumentBalanceSchema, {
              instrumentCode: 'UNCOV_S',
              openingEconomicNotional: makeDec('-10000000', u.currencyAmount!),
              openingAvailableNotional: makeDec('0', u.currencyAmount!),
              openingNetT0Notional: makeDec('0', u.currencyAmount!),
              openingNetT1Notional: makeDec('0', u.currencyAmount!),
            }),
          ],
          instruments: [
            create(InstrumentCatalogEntrySchema, {
              instrumentCode: 'UNCOV_L',
              bond: makeStandardBond('PA31'),
              eligibilityClass: RateCoverageEligibility.UNSPECIFIED,
              calendarRequirement: CalendarRequirement.UNSPECIFIED,
            }),
            create(InstrumentCatalogEntrySchema, {
              instrumentCode: 'UNCOV_S',
              bond: makeStandardBond('PA32'),
              eligibilityClass: RateCoverageEligibility.UNSPECIFIED,
              calendarRequirement: CalendarRequirement.UNSPECIFIED,
            }),
          ],
        }),
        expectedAccountVersion: 0n,
      });

      preview.setUpstreamOrigin(srvPolicyA.origin);
      preview.proxy.resetFaults();
      await loginInUi(page, { token: TRADER_TOKEN, headerMode: 'authorization' });
      await submitAccountsInUi(page, 'acct-zero-dv01, acct-empty-cov, acct-zero-net-uncov');
      await expect(page.getByTestId('account-tab-acct-zero-dv01')).toBeVisible();

      // Dynamic Policy A shrinks product selector to 3 products (no TS)
      const productOptionsA = await page
        .getByTestId('hedge-product-select')
        .locator('option')
        .allTextContents();
      expect(productOptionsA).toEqual(['T', 'TF', 'TL']);

      // Covered zero net DV01 succeeds with 0 contracts
      await page.getByTestId('hedge-product-select').selectOption('T');
      await page.getByTestId('hedge-contract-input').fill('T2609');
      await page.getByTestId('hedge-ctd-dv01-input').fill('0.075');
      await page.getByTestId('hedge-cf-input').fill('0.985');
      await page.getByTestId('hedge-preview-submit-btn').click();
      await expect(page.getByTestId('hedge-preview-result')).toHaveAttribute(
        'data-zero-net-exposure',
        'true',
      );
      await expect(page.getByTestId('hedge-recommended-contracts')).toHaveText('0');

      // Pure short uncovered account: warning active, WHOLE_ACCOUNT disabled, empty covered book rejected
      await page.getByTestId('account-tab-acct-empty-cov').click();
      await expect(page.getByTestId('uncovered-risk-warning')).toHaveAttribute(
        'data-pure-short',
        'true',
      );
      await expect(page.getByTestId('uncovered-risk-warning')).toHaveAttribute(
        'data-whole-account-blocked',
        'true',
      );
      await expect(page.getByTestId('scope-whole-account')).toBeDisabled();
      await page.getByTestId('hedge-contract-input').fill('T2609');
      await page.getByTestId('hedge-ctd-dv01-input').fill('0.075');
      await page.getByTestId('hedge-cf-input').fill('0.985');
      await page.getByTestId('hedge-preview-submit-btn').click();
      await expect(page.getByTestId('hedge-preview-error')).toBeVisible();

      // Zero-net non-zero-gross uncovered account: warning active, WHOLE_ACCOUNT disabled
      await page.getByTestId('account-tab-acct-zero-net-uncov').click();
      await expect(page.getByTestId('uncovered-risk-warning')).toHaveAttribute(
        'data-net-zero-gross-nonzero',
        'true',
      );
      await expect(page.getByTestId('uncovered-risk-warning')).toHaveAttribute(
        'data-whole-account-blocked',
        'true',
      );
      await expect(page.getByTestId('scope-whole-account')).toBeDisabled();

      // --- Part 1C: Retention Window = 2, Active Stream Cut -> Poll -> Dual-Read Block -> Recovery
      //              + Idle Authorized Account Cold Start on Pruned Journal ---
      const srvRet2 = await startFicantServer({
        mode: 'ephemeral',
        policy: 'embedded_default',
        accounts: ['acct-A', 'acct-idle'],
        sources: ['FEED-A', 'FEED-IDLE', 'primary-quote-feed'],
        retentionWindow: 2,
        watchdogIntervalMs: 0,
      });
      servers.push(srvRet2);

      const adminR2 = createAdminClients(srvRet2.origin);
      await adminR2.operator.importOpeningBase({
        idempotencyKey: 'r2-imp-a',
        openingBase: makeOpeningBase({
          accountId: 'acct-A',
          baseSuffix: 'R20A',
          sourceId: 'FEED-A',
        }),
        expectedAccountVersion: 0n,
      });
      await adminR2.operator.importOpeningBase({
        idempotencyKey: 'r2-imp-idle',
        openingBase: makeOpeningBase({
          accountId: 'acct-idle',
          baseSuffix: 'R20B',
          sourceId: 'FEED-IDLE',
        }),
        expectedAccountVersion: 0n,
      });

      preview.setUpstreamOrigin(srvRet2.origin);
      preview.proxy.resetFaults();
      await loginInUi(page, { token: TRADER_TOKEN, headerMode: 'authorization' });
      await submitAccountsInUi(page, 'acct-A, acct-idle');
      await expect(page.getByTestId('account-tab-acct-A')).toBeVisible();
      await expect(page.getByTestId('account-tab-acct-idle')).toBeVisible();
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'stream_live',
      );
      await preview.proxy.waitForActiveStream(1);

      // 1) Active stream cut -> degraded PollEvents catches up new event
      preview.proxy.blockSubscribeEvents(true);
      const cutCount = preview.proxy.destroyActiveStreams();
      expect(cutCount).toBeGreaterThanOrEqual(1);
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'poll_degraded',
      );

      const quoteRes1 = await adminR2.trader.ingestMarketQuotes({
        idempotencyKey: 'r2-q-1',
        accountId: 'acct-A',
        sourceId: 'primary-quote-feed',
        ticks: [
          makeQuoteTick({
            sourceId: 'primary-quote-feed',
            quoteSeq: 1n,
            instrumentCode: 'BOND_10Y',
            ytm: '0.025',
          }),
        ],
      });
      await expect(page.getByTestId('funding-risk-top-bar')).toHaveAttribute(
        'data-snapshot-seq',
        quoteRes1.committedSeq.toString(),
      );

      // 2) Dual-read block (SubscribeEvents + PollEvents BOTH blocked) while journal prunes past retention=2
      preview.proxy.blockPollEvents(true);
      let latestSeq = quoteRes1.committedSeq;
      for (let i = 2; i <= 5; i += 1) {
        const res = await adminR2.trader.ingestMarketQuotes({
          idempotencyKey: `r2-q-${i}`,
          accountId: 'acct-A',
          sourceId: 'primary-quote-feed',
          ticks: [
            makeQuoteTick({
              sourceId: 'primary-quote-feed',
              quoteSeq: BigInt(i),
              instrumentCode: 'BOND_10Y',
              ytm: `0.02${i}`,
            }),
          ],
        });
        latestSeq = res.committedSeq;
      }

      // Restore proxy -> PollEvents hits real SnapshotFallbackRequired -> recovery barrier -> stream_live!
      preview.proxy.resetFaults();
      await expect(page.getByTestId('funding-risk-top-bar')).toHaveAttribute(
        'data-snapshot-seq',
        latestSeq.toString(),
      );
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'stream_live',
      );
      // Both active acct-A AND idle acct-idle preserved across recovery barrier
      await expect(page.getByTestId('account-tab-acct-A')).toBeVisible();
      await expect(page.getByTestId('account-tab-acct-idle')).toBeVisible();

      // 3) Brand-new cold session for idle authorized account on pruned journal:
      //    Must fetch GetAccountSnapshot FIRST, then SubscribeEvents(after_seq == snapshot_seq) cleanly!
      await page.getByTestId('logout-btn').click();
      await expect(page.getByTestId('auth-form')).toBeVisible();
      preview.proxy.clearRecordedRequests();

      await loginInUi(page, { token: TRADER_TOKEN, headerMode: 'authorization' });
      await submitAccountsInUi(page, 'acct-idle');
      await expect(page.getByTestId('account-tab-acct-idle')).toBeVisible();
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'stream_live',
      );
      await preview.proxy.waitForActiveStream(1);

      const coldReqs = preview.proxy.getRecordedRequests();
      const firstSnapIdx = coldReqs.findIndex(
        (r) =>
          r.path.endsWith('/GetAccountSnapshot') && r.decodedAccountId === 'acct-idle',
      );
      const firstSubIdx = coldReqs.findIndex((r) => r.path.endsWith('/SubscribeEvents'));
      expect(firstSnapIdx).toBeGreaterThanOrEqual(0);
      expect(firstSubIdx).toBeGreaterThan(firstSnapIdx);
      expect(coldReqs[firstSubIdx]?.decodedAfterSeq).toBe(latestSeq);

      expect(pageErrors).toEqual([]);
    } finally {
      if (preview) {
        await preview.stop();
      }
      for (const srv of servers) {
        await srv.cleanup();
      }
    }
  });

  // ==========================================================================
  // Suite 2: Virtual Grid 200+ Rows Scroll-to-End, Dual Viewport (1440x900 &
  //          480x900) No Page Overflow + Live Stream 3-Surface Seq Sync,
  //          Dual-Source HOLD Lazy Expiry (COMMITTED vs EXPIRED + Replay)
  // ==========================================================================
  test('Suite 2: Virtual Grid 200+ Rows Dual Viewport, 3-Surface Seq Sync & Dual-Source HOLD Lazy Expiry Replay', async ({
    page,
  }) => {
    const pageErrors = attachPageErrorCollector(page);
    const sharedTempDir = createIsolatedTempDir('ficant-e2e-wal-');
    const dbPath = join(sharedTempDir, 'desk-blotter-suite2.sqlite');
    let server: FicantServerHandle | null = null;
    let preview: PreviewWithProxyHandle | null = null;

    const regPack = buildRegulatoryRulePackBinding();
    const walPolicy = {
      profile: 'B',
      allowedHedgeProductCodes: ['T', 'TF', 'TL', 'TS'],
      defaultQuoteSourceId: 'primary-quote-feed',
      limitCeilings: { MAX_INST_FACE: '500000000' },
      regulatoryRulePack: regPack,
    };

    try {
      server = await startFicantServer({
        tempDir: sharedTempDir,
        mode: { kind: 'persistent_wal', dbPath },
        policy: walPolicy,
        accounts: ['acct-A', 'acct-B'],
        sources: ['FEED-A', 'FEED-B', 'primary-quote-feed'],
        watchdogIntervalMs: 0,
      });

      const admin = createAdminClients(server.origin);
      const u = REFERENCE_UNITS;

      // Seed acct-A with 205 instruments (BOND_001 .. BOND_205) on FEED-A
      const balancesA: OpeningInstrumentBalance[] = [];
      const instrumentsA: InstrumentCatalogEntry[] = [];
      for (let i = 1; i <= 205; i += 1) {
        const code = `BOND_${String(i).padStart(3, '0')}`;
        balancesA.push(
          create(OpeningInstrumentBalanceSchema, {
            instrumentCode: code,
            openingEconomicNotional: makeDec('10000000', u.currencyAmount!),
            openingAvailableNotional: makeDec('10000000', u.currencyAmount!),
            openingNetT0Notional: makeDec('0', u.currencyAmount!),
            openingNetT1Notional: makeDec('0', u.currencyAmount!),
          }),
        );
        instrumentsA.push(
          create(InstrumentCatalogEntrySchema, {
            instrumentCode: code,
            bond: makeStandardBond(crockfordSuffix(1000 + i)),
            eligibilityClass: RateCoverageEligibility.SOVEREIGN_OR_POLICY_FIXED,
            calendarRequirement: CalendarRequirement.UNSPECIFIED,
          }),
        );
      }

      await admin.operator.importOpeningBase({
        idempotencyKey: 's2-imp-a',
        openingBase: makeOpeningBase({
          accountId: 'acct-A',
          baseSuffix: 'S20A',
          sourceId: 'FEED-A',
          balances: balancesA,
          instruments: instrumentsA,
        }),
        expectedAccountVersion: 0n,
      });

      // Seed acct-B on an independent healthy source FEED-B
      await admin.operator.importOpeningBase({
        idempotencyKey: 's2-imp-b',
        openingBase: makeOpeningBase({
          accountId: 'acct-B',
          baseSuffix: 'S20B',
          sourceId: 'FEED-B',
          balances: [
            create(OpeningInstrumentBalanceSchema, {
              instrumentCode: 'BOND_001',
              openingEconomicNotional: makeDec('20000000', u.currencyAmount!),
              openingAvailableNotional: makeDec('20000000', u.currencyAmount!),
              openingNetT0Notional: makeDec('0', u.currencyAmount!),
              openingNetT1Notional: makeDec('0', u.currencyAmount!),
            }),
          ],
          instruments: [
            create(InstrumentCatalogEntrySchema, {
              instrumentCode: 'BOND_001',
              bond: makeStandardBond('S2B1'),
              eligibilityClass: RateCoverageEligibility.SOVEREIGN_OR_POLICY_FIXED,
              calendarRequirement: CalendarRequirement.UNSPECIFIED,
            }),
          ],
        }),
        expectedAccountVersion: 0n,
      });

      await admin.trader.ingestMarketQuotes({
        idempotencyKey: 's2-q-init',
        accountId: 'acct-A',
        sourceId: 'primary-quote-feed',
        ticks: [
          makeQuoteTick({
            sourceId: 'primary-quote-feed',
            quoteSeq: 1n,
            instrumentCode: 'BOND_001',
            ytm: '0.025',
          }),
          makeQuoteTick({
            sourceId: 'primary-quote-feed',
            quoteSeq: 1n,
            instrumentCode: 'BOND_205',
            ytm: '0.026',
          }),
        ],
      });

      preview = await startPreviewWithProxy(server.origin);
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.goto(preview.baseUrl, { waitUntil: 'networkidle' });

      await loginInUi(page, { token: TRADER_TOKEN, headerMode: 'authorization' });
      await submitAccountsInUi(page, 'acct-A, acct-B');
      await expect(page.getByTestId('account-tab-acct-A')).toBeVisible();
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'stream_live',
      );
      await preview.proxy.waitForActiveStream(1);

      // --- Part 2A: Dual Viewport (1440x900 & 480x900) Virtual Scroll-to-End + 3-Surface Seq Sync ---
      let quoteSeqCounter = 2n;
      for (const vp of [
        { width: 1440, height: 900, expectedLayout: 'wide', holdNote: 'vp-wide-last-row' },
        { width: 480, height: 900, expectedLayout: 'narrow', holdNote: 'vp-narrow-last-row' },
      ]) {
        await page.setViewportSize({ width: vp.width, height: vp.height });
        await expect(page.getByTestId('desk-app-shell')).toHaveAttribute(
          'data-layout',
          vp.expectedLayout,
        );
        await expect(page.getByTestId('positions-row-counts')).toContainText('总计: 205');

        // Assert zero horizontal page overflow
        const noOverflow = await page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        );
        expect(noOverflow).toBe(true);

        // Scroll virtual grid to the very end (row 205) and verify virtualization window
        const scroller = page.getByTestId('positions-grid-scroll');
        await scroller.evaluate((el) => {
          el.scrollTop = el.scrollHeight;
        });
        const lastRow = page.getByTestId('position-row-BOND_205');
        await expect(lastRow).toBeVisible();
        await expect(lastRow).toHaveAttribute('data-row-index', '204');

        // Click Column 1 inside lastRow (0..140px stays cleanly inside 480px viewport!)
        await lastRow.locator('strong').first().click();
        await expect(page.getByTestId('selected-target-context')).toContainText('BOND_205');

        // Operate last row: create a 1800s HOLD on BOND_205
        await page.getByTestId('mode-btn-create_hold').click();
        await page.getByTestId('input-notional').fill('1000000');
        await page.getByTestId('input-ttl-seconds').fill('1800');
        await page.getByTestId('input-note').fill(vp.holdNote);
        await page.getByTestId('hold-form-submit-btn').click();
        await expect(page.getByTestId('hold-form-feedback')).toContainText('CreateHold 已提交');

        // Trigger external live stream push and assert 3-surface data-snapshot-seq sync!
        const pushRes = await admin.trader.ingestMarketQuotes({
          idempotencyKey: `s2-live-push-${quoteSeqCounter.toString()}`,
          accountId: 'acct-A',
          sourceId: 'primary-quote-feed',
          ticks: [
            makeQuoteTick({
              sourceId: 'primary-quote-feed',
              quoteSeq: quoteSeqCounter,
              instrumentCode: 'BOND_205',
              ytm: `0.02${quoteSeqCounter.toString()}`,
            }),
          ],
        });
        quoteSeqCounter += 1n;
        const targetSeq = pushRes.committedSeq.toString();

        await expect(page.getByTestId('funding-risk-top-bar')).toHaveAttribute(
          'data-snapshot-seq',
          targetSeq,
        );
        await expect(page.getByTestId('positions-grid-panel')).toHaveAttribute(
          'data-snapshot-seq',
          targetSeq,
        );
        await expect(page.getByTestId('exposure-chart-panel')).toHaveAttribute(
          'data-snapshot-seq',
          targetSeq,
        );
      }

      // --- Part 2B: Dual-Source HOLD Lazy Expiry (COMMITTED vs EXPIRED + reconfirmation_error & Replay) ---
      await page.setViewportSize({ width: 1440, height: 900 });

      // Create one 1800s HOLD on acct-A (healthy FEED-A) and one 1800s HOLD on acct-B (healthy FEED-B)
      const snapA0 = (await admin.trader.getAccountSnapshot({ accountId: 'acct-A' })).snapshot!;
      const posA001 = snapA0.positions.find((p) => p.instrumentCode === 'BOND_001')!;
      const holdARes = await admin.trader.createHold({
        idempotencyKey: 's2-lazy-hold-a',
        accountId: 'acct-A',
        expectedPositionVersion: posA001.positionVersion,
        instrumentCode: 'BOND_001',
        side: TradeSide.BUY,
        settlementBucket: SettlementBucket.T_PLUS_0,
        notional: makeDec('1000000', u.currencyAmount!),
        ttlSeconds: 1800,
        note: 'lazy-expiry-healthy-src',
      });
      const holdAId = holdARes.earmark!.earmarkId!.value;

      const snapB0 = (await admin.trader.getAccountSnapshot({ accountId: 'acct-B' })).snapshot!;
      const posB001 = snapB0.positions.find((p) => p.instrumentCode === 'BOND_001')!;
      const holdBRes = await admin.trader.createHold({
        idempotencyKey: 's2-lazy-hold-b',
        accountId: 'acct-B',
        expectedPositionVersion: posB001.positionVersion,
        instrumentCode: 'BOND_001',
        side: TradeSide.BUY,
        settlementBucket: SettlementBucket.T_PLUS_0,
        notional: makeDec('1000000', u.currencyAmount!),
        ttlSeconds: 1800,
        note: 'lazy-expiry-paused-src',
      });
      const holdBId = holdBRes.earmark!.earmarkId!.value;

      // Pause ONLY FEED-B (FEED-A stays healthy!)
      const feedBStatus = snapB0.feedStatuses.find((f) => f.sourceId === 'FEED-B')!;
      await admin.operator.setFeedControl({
        idempotencyKey: 's2-pause-feed-b',
        sourceId: 'FEED-B',
        expectedFeedStatusRevision: feedBStatus.feedStatusRevision,
        pausedByOperator: true,
        forceClearGapPause: false,
        reason: 'pause-only-feed-b-for-lazy-expiry',
      });

      // Cleanly stop ficant-server, roll back BOTH HOLDs via node:sqlite preserving local_trading_date
      await server.stop();
      const rolledBack = rollbackHoldExpiryInSqlite(dbPath, [holdAId, holdBId], 120);
      expect(rolledBack.length).toBe(2);

      // Restart ficant-server on the EXACT SAME WAL DB and re-point proxy
      server = await startFicantServer({
        tempDir: sharedTempDir,
        mode: { kind: 'persistent_wal', dbPath },
        policy: walPolicy,
        accounts: ['acct-A', 'acct-B'],
        sources: ['FEED-A', 'FEED-B', 'primary-quote-feed'],
        watchdogIntervalMs: 0,
      });
      preview.setUpstreamOrigin(server.origin);
      const adminAfter = createAdminClients(server.origin);

      // Re-login in browser to observe fresh latest revisions from restarted WAL server
      await loginInUi(page, { token: TRADER_TOKEN, headerMode: 'authorization' });
      await submitAccountsInUi(page, 'acct-A, acct-B');
      await expect(page.getByTestId('account-tab-acct-A')).toBeVisible();
      await expect(page.getByTestId('account-tab-acct-B')).toBeVisible();
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'stream_live',
      );

      // Branch 1: Confirm expired HOLD on healthy FEED-A -> COMMITTED!
      await page.getByTestId('positions-search-input').fill('BOND_001');
      await page.getByTestId(`earmark-badge-${holdAId}`).click();
      await expect(page.getByTestId('hold-form-confirm_hold')).toBeVisible();
      await page.getByTestId('input-quote-mode').selectOption('settlement_only');
      await page.getByTestId('input-settlement-amount').fill('1000000');
      await page.getByTestId('hold-form-submit-btn').click();
      const committedReceipt = page
        .locator(
          '[data-testid="receipts-list"] > li[data-receipt-operation="ConfirmHold"][data-receipt-status="succeeded"]',
        )
        .first();
      await expect(committedReceipt).toBeVisible();
      await expect(committedReceipt).toContainText('COMMITTED');

      // Branch 2: Confirm expired HOLD on paused FEED-B -> committed EXPIRED + structured reconfirmation_error!
      await page.getByTestId('account-tab-acct-B').click();
      await page.getByTestId('positions-search-input').fill('BOND_001');
      await page.getByTestId(`earmark-badge-${holdBId}`).click();
      await expect(page.getByTestId('hold-form-confirm_hold')).toBeVisible();
      await page.getByTestId('input-quote-mode').selectOption('settlement_only');
      await page.getByTestId('input-settlement-amount').fill('1000000');
      await page.getByTestId('hold-form-submit-btn').click();

      const expiredReceipt = page
        .locator('[data-testid="receipts-list"] > li[data-receipt-status="confirm_hold_expired"]')
        .first();
      await expect(expiredReceipt).toBeVisible();
      await expect(expiredReceipt).toHaveAttribute('data-receipt-replayed', 'false');
      const firstCommittedSeq = await expiredReceipt.getAttribute('data-receipt-committed-seq');
      expect(firstCommittedSeq).not.toBeNull();
      expect(BigInt(firstCommittedSeq!)).toBeGreaterThan(0n);
      await expect(expiredReceipt).toContainText('EXPIRED');
      await expect(expiredReceipt).toContainText('VALIDATION_FAILED');
      await expect(expiredReceipt).toContainText('trade feed is paused');

      // Hold B is removed from active_earmarks, yet receipt persists in ReceiptsDrawer!
      await expect(page.getByTestId(`earmark-badge-${holdBId}`)).toHaveCount(0);

      // Record server head_seq BEFORE replay
      const headBeforeReplay = (
        await adminAfter.trader.getAccountSnapshot({ accountId: 'acct-B' })
      ).snapshot!.snapshotSeq;

      // Replay exact original failed ConfirmHold request (same idempotency_key + exact payload)
      await expiredReceipt.locator('button[data-testid^="receipt-replay-btn-"]').click();

      const replayedReceipt = page
        .locator(
          '[data-testid="receipts-list"] > li[data-receipt-status="confirm_hold_expired"][data-receipt-replayed="true"]',
        )
        .first();
      await expect(replayedReceipt).toBeVisible();
      await expect(replayedReceipt).toHaveAttribute(
        'data-receipt-committed-seq',
        firstCommittedSeq!,
      );

      // Verify server global head_seq did NOT grow on idempotent replay!
      const headAfterReplay = (
        await adminAfter.trader.getAccountSnapshot({ accountId: 'acct-B' })
      ).snapshot!.snapshotSeq;
      expect(headAfterReplay).toBe(headBeforeReplay);

      expect(pageErrors).toEqual([]);
    } finally {
      if (preview) {
        await preview.stop();
      }
      if (server) {
        await server.stop();
      }
      removeIsolatedTempDir(sharedTempDir);
    }
  });

  // ==========================================================================
  // Suite 3: ResolvePendingMatch 5-Action Matrix, BASELINE_REVISION_MANUAL +
  //          Active sourceGap, & 3-State Transaction-Scoped Cash Attribution
  // ==========================================================================
  test('Suite 3: ResolvePendingMatch 5-Action Matrix, BASELINE_REVISION_MANUAL Under Source Gap & 3-State Cash Attribution', async ({
    page,
  }) => {
    const pageErrors = attachPageErrorCollector(page);
    let server: FicantServerHandle | null = null;
    let preview: PreviewWithProxyHandle | null = null;

    try {
      server = await startFicantServer({
        mode: 'ephemeral',
        policy: 'embedded_default',
        accounts: ['acct-A'],
        sources: ['FEED-A', 'primary-quote-feed'],
        retentionWindow: 2,
        watchdogIntervalMs: 0,
      });

      const admin = createAdminClients(server.origin);
      const u = REFERENCE_UNITS;

      const baseEv = makeBaseEvidence({
        sourceId: 'FEED-A',
        externalTradeId: 'BASE-50M',
        originalFeedSeq: 5n,
        oldNotional: '50000000',
        oldSignedCash: '50000000',
      });

      await admin.operator.importOpeningBase({
        idempotencyKey: 's3-imp-a',
        openingBase: makeOpeningBase({
          accountId: 'acct-A',
          baseSuffix: 'S30A',
          sourceId: 'FEED-A',
          startAfterSeq: 5n,
          baselineEvidence: [baseEv],
        }),
        expectedAccountVersion: 0n,
      });

      // Seed COMMITTED SELL 60M + partial fill 40M -> creates PENDING_MATCH for SPLIT_OFFSET & MATCH_FULL
      await admin.trader.createCommitted({
        idempotencyKey: 's3-comm-60',
        accountId: 'acct-A',
        expectedPositionVersion: 1n,
        instrumentCode: 'BOND_10Y',
        side: TradeSide.SELL,
        settlementBucket: SettlementBucket.T_PLUS_0,
        notional: makeDec('60000000', u.currencyAmount!),
        quoteTerms: makeQuoteTermsClean('100.00', '60000000'),
        note: 'sell-60m',
      });
      await admin.feed.ingestTradeFeed({
        idempotencyKey: 's3-fill-40',
        sourceId: 'FEED-A',
        fills: [
          makeTradeFill({
            sourceId: 'FEED-A',
            tradeId: 'F-SPLIT-40',
            feedSeq: 6n,
            accountId: 'acct-A',
            instrumentCode: 'BOND_10Y',
            side: TradeSide.SELL,
            notional: '40000000',
            quoteTerms: makeQuoteTermsClean('100.00', '40000000'),
          }),
        ],
      });

      preview = await startPreviewWithProxy(server.origin);
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.goto(preview.baseUrl, { waitUntil: 'networkidle' });

      await loginInUi(page, { token: TRADER_TOKEN, headerMode: 'authorization' });
      await submitAccountsInUi(page, 'acct-A');
      await expect(page.getByTestId('account-tab-acct-A')).toBeVisible();
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'stream_live',
      );
      await preview.proxy.waitForActiveStream(1);

      // --- Action 1: SPLIT_OFFSET (25M of 40M) — pre-submit unknown cash label -> post-submit State 1 (allocated) ---
      await page
        .locator(`#pm-action-${String(PendingMatchResolutionAction.SPLIT_OFFSET)}`)
        .check();
      await page.locator('#pm-offset-notional').fill('25000000');
      await expect(page.getByTestId('preview-cash-impact')).toHaveAttribute(
        'data-cash-impact-kind',
        'unknown_server_reconcile',
      );
      await expect(page.getByTestId('preview-cash-impact')).toContainText('未知');
      await page.getByTestId('submit-resolve-pending-match').click();

      const splitCard = page
        .locator(
          '[data-testid="post-submit-attribution-section"] article[data-attribution-state="allocated"]',
        )
        .first();
      await expect(splitCard).toBeVisible();
      await expect(splitCard).toContainText('allocated_fill_cash');
      await expect(splitCard).toContainText('released_earmark_cash');

      // --- Action 2: MATCH_FULL (remaining 15M against remaining 35M earmark) -> State 1 (allocated) ---
      await page
        .locator(`#pm-action-${String(PendingMatchResolutionAction.MATCH_FULL)}`)
        .check();
      await page.getByTestId('submit-resolve-pending-match').click();
      await expect(page.getByTestId('pending-match-empty')).toBeVisible();
      await expect(
        page.locator(
          '[data-testid="post-submit-attribution-section"] article[data-attribution-state="allocated"]',
        ),
      ).toHaveCount(2);

      // --- Action 3: RELEASE_REJECT -> State 2 (no_allocations), unaffected by later cash change ---
      const snapBeforeRej = (await admin.trader.getAccountSnapshot({ accountId: 'acct-A' }))
        .snapshot!;
      await admin.feed.ingestTradeFeed({
        idempotencyKey: 's3-fill-rej',
        sourceId: 'FEED-A',
        fills: [
          makeTradeFill({
            sourceId: 'FEED-A',
            tradeId: 'F-REJ-10',
            feedSeq: 7n,
            accountId: 'acct-A',
            instrumentCode: 'BOND_10Y',
            side: TradeSide.SELL,
            notional: '10000000',
            quoteTerms: makeQuoteTermsClean('100.06', '10006000'),
          }),
        ],
      });
      await expect(
        page.locator('button[data-testid^="pending-match-item-"]').first(),
      ).toBeVisible();
      await page
        .locator(`#pm-action-${String(PendingMatchResolutionAction.RELEASE_REJECT)}`)
        .check();
      await page.locator('#pm-reason').fill('price-out-of-tolerance-reject');
      await page.getByTestId('submit-resolve-pending-match').click();

      const rejCard = page
        .locator(
          '[data-testid="post-submit-attribution-section"] article[data-attribution-state="no_allocations"]',
        )
        .first();
      await expect(rejCard).toBeVisible();
      await expect(rejCard).toContainText('本动作无分摊');

      // Push an unrelated balance-changing committed trade and confirm RELEASE_REJECT card stays no_allocations
      const posNow = (await admin.trader.getAccountSnapshot({ accountId: 'acct-A' })).snapshot!
        .positions[0]!;
      await admin.trader.createCommitted({
        idempotencyKey: 's3-unrelated-comm',
        accountId: 'acct-A',
        expectedPositionVersion: posNow.positionVersion,
        instrumentCode: 'BOND_10Y',
        side: TradeSide.BUY,
        settlementBucket: SettlementBucket.T_PLUS_0,
        notional: makeDec('5000000', u.currencyAmount!),
        quoteTerms: makeQuoteTermsClean('100.00', '5000000'),
        note: 'unrelated-cash-movement',
      });
      expect(snapBeforeRej.accountId).toBe('acct-A');
      await expect(rejCard).toHaveAttribute('data-attribution-state', 'no_allocations');

      // --- Action 4: ACCEPT_ORPHAN + Retention-2 Pruned History -> State 3 (pruned_history_unrecoverable) ---
      await admin.feed.ingestTradeFeed({
        idempotencyKey: 's3-fill-orph',
        sourceId: 'FEED-A',
        fills: [
          makeTradeFill({
            sourceId: 'FEED-A',
            tradeId: 'F-ORPH-5',
            feedSeq: 8n,
            accountId: 'acct-A',
            instrumentCode: 'BOND_10Y',
            side: TradeSide.BUY,
            settlementDate: '2026-06-16',
            notional: '5000000',
            quoteTerms: makeQuoteTermsClean('100.00', '5000000'),
          }),
        ],
      });
      await expect(
        page.locator('button[data-testid^="pending-match-item-"]').first(),
      ).toBeVisible();

      // Block BOTH event read paths & cut stream so browser misses ACCEPT_ORPHAN's event before pruning
      preview.proxy.blockSubscribeEvents(true);
      preview.proxy.blockPollEvents(true);
      preview.proxy.destroyActiveStreams();
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'poll_degraded',
      );

      await page
        .locator(`#pm-action-${String(PendingMatchResolutionAction.ACCEPT_ORPHAN)}`)
        .check();
      await page.locator('#pm-reason').fill('accept-orphan-for-pruned-check');
      await page.getByTestId('submit-resolve-pending-match').click();

      const prunedCard = page
        .locator('[data-testid="post-submit-attribution-section"] article')
        .first();
      await expect(prunedCard).toHaveAttribute('data-attribution-state', 'pending_correlation');

      // Advance journal past retentionWindow=2 while both read paths remain blocked
      for (let q = 1; q <= 4; q += 1) {
        await admin.trader.ingestMarketQuotes({
          idempotencyKey: `s3-prune-q-${q}`,
          accountId: 'acct-A',
          sourceId: 'primary-quote-feed',
          ticks: [
            makeQuoteTick({
              sourceId: 'primary-quote-feed',
              quoteSeq: BigInt(q),
              instrumentCode: 'BOND_10Y',
              ytm: `0.02${q}`,
            }),
          ],
        });
      }

      // Unblock proxy -> recovery advances cursorSeq past committed_seq -> read-only PollEvents confirms pruned!
      preview.proxy.resetFaults();
      await expect(prunedCard).toHaveAttribute(
        'data-attribution-state',
        'pruned_history_unrecoverable',
      );
      await expect(prunedCard).toContainText('历史分摊已裁剪，明细不可恢复');
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'stream_live',
      );

      // --- Action 5: BASELINE_REVISION_MANUAL + Active sourceGap -> APPLY_BASELINE_DELTA allowed ---
      // 1) Ingest AMEND rev=2 on BASE-50M (feed_seq=9) -> creates BASELINE_REVISION_MANUAL pending match
      await admin.feed.ingestTradeFeed({
        idempotencyKey: 's3-base-amend',
        sourceId: 'FEED-A',
        fills: [
          makeTradeFill({
            sourceId: 'FEED-A',
            tradeId: 'BASE-50M',
            rev: 2n,
            feedSeq: 9n,
            accountId: 'acct-A',
            instrumentCode: 'BOND_10Y',
            side: TradeSide.SELL,
            notional: '40000000',
            quoteTerms: makeQuoteTermsClean('100.00', '40000000'),
            action: TradeFeedRecordAction.AMEND,
          }),
        ],
      });
      // 2) Skip feed_seq=10 and ingest feed_seq=11 -> triggers active sourceGap [10..10] on FEED-A!
      await admin.feed.ingestTradeFeed({
        idempotencyKey: 's3-gap-seq11',
        sourceId: 'FEED-A',
        fills: [
          makeTradeFill({
            sourceId: 'FEED-A',
            tradeId: 'F-GAP-11',
            feedSeq: 11n,
            accountId: 'acct-A',
            instrumentCode: 'BOND_10Y',
            side: TradeSide.BUY,
            notional: '1000000',
            quoteTerms: makeQuoteTermsClean('100.00', '1000000'),
          }),
        ],
      });

      await expect(page.getByTestId('source-gap-banner')).toBeVisible();
      await expect(page.getByTestId('source-gap-banner')).toContainText('[10..10]');

      // Actions 1..4 MUST be disabled while APPLY_BASELINE_DELTA (5) stays enabled!
      for (const blockedAction of [
        PendingMatchResolutionAction.MATCH_FULL,
        PendingMatchResolutionAction.SPLIT_OFFSET,
        PendingMatchResolutionAction.ACCEPT_ORPHAN,
        PendingMatchResolutionAction.RELEASE_REJECT,
      ]) {
        await expect(page.locator(`#pm-action-${String(blockedAction)}`)).toBeDisabled();
      }
      const baselineActionRadio = page.locator(
        `#pm-action-${String(PendingMatchResolutionAction.APPLY_BASELINE_DELTA)}`,
      );
      await expect(baselineActionRadio).toBeEnabled();
      await baselineActionRadio.check();

      await expect(page.getByTestId('base-evidence-summary')).toContainText('BASE-50M');
      await page.locator('#pm-baseline-delta-notional').fill('-10000000');
      await page.locator('#pm-baseline-delta-cash').fill('-10000000');
      await page.locator('#pm-baseline-delta-reason').fill('baseline-amend-during-active-gap');
      await page.getByTestId('submit-resolve-pending-match').click();

      const deltaCard = page
        .locator(
          '[data-testid="post-submit-attribution-section"] article[data-attribution-state="allocated"]',
        )
        .first();
      await expect(deltaCard).toBeVisible();
      await expect(deltaCard).toContainText('baseline_delta');
      await expect(deltaCard).toContainText('baseline-amend-during-active-gap');

      expect(pageErrors).toEqual([]);
    } finally {
      if (preview) {
        await preview.stop();
      }
      if (server) {
        await server.cleanup();
      }
    }
  });

  // ==========================================================================
  // Suite 4: Explicit Ctrl+V Paste Draft Drawer, UTF-8 Chinese Anchors,
  //          Regulatory Limits, 3-Stage Snapshots, CONCURRENCY_CONFLICT
  //          Re-Confirm & Unconfirmed Hold Zero-Leak
  // ==========================================================================
  test('Suite 4: Ctrl+V Paste Drawer, UTF-8 Chinese Anchors, Regulatory Limits, 3-Stage Snapshots & CONCURRENCY_CONFLICT Re-Confirm', async ({
    page,
  }) => {
    const pageErrors = attachPageErrorCollector(page);
    let server: FicantServerHandle | null = null;
    let preview: PreviewWithProxyHandle | null = null;

    try {
      const regPack = buildRegulatoryRulePackBinding();
      server = await startFicantServer({
        mode: 'ephemeral',
        policy: {
          profile: 'B',
          allowedHedgeProductCodes: ['T', 'TF', 'TL', 'TS'],
          defaultQuoteSourceId: 'primary-quote-feed',
          limitCeilings: { MAX_INST_FACE: '500000000' },
          regulatoryRulePack: regPack,
        },
        accounts: ['acct-A'],
        sources: ['FEED-A', 'primary-quote-feed'],
        watchdogIntervalMs: 0,
        nlpCsvPath: NLP_TEMPLATES_CSV_PATH,
      });

      const admin = createAdminClients(server.origin);
      const u = REFERENCE_UNITS;

      await admin.operator.importOpeningBase({
        idempotencyKey: 's4-imp-a',
        openingBase: makeOpeningBase({
          accountId: 'acct-A',
          baseSuffix: 'S40A',
          sourceId: 'FEED-A',
          balances: [
            create(OpeningInstrumentBalanceSchema, {
              instrumentCode: 'BOND_10Y',
              openingEconomicNotional: makeDec('100000000', u.currencyAmount!),
              openingAvailableNotional: makeDec('100000000', u.currencyAmount!),
              openingNetT0Notional: makeDec('0', u.currencyAmount!),
              openingNetT1Notional: makeDec('0', u.currencyAmount!),
            }),
            create(OpeningInstrumentBalanceSchema, {
              instrumentCode: 'UNCOV_BOND',
              openingEconomicNotional: makeDec('10000000', u.currencyAmount!),
              openingAvailableNotional: makeDec('10000000', u.currencyAmount!),
              openingNetT0Notional: makeDec('0', u.currencyAmount!),
              openingNetT1Notional: makeDec('0', u.currencyAmount!),
            }),
          ],
          instruments: [
            create(InstrumentCatalogEntrySchema, {
              instrumentCode: 'BOND_10Y',
              bond: makeStandardBond('S4B1'),
              eligibilityClass: RateCoverageEligibility.SOVEREIGN_OR_POLICY_FIXED,
              calendarRequirement: CalendarRequirement.UNSPECIFIED,
            }),
            create(InstrumentCatalogEntrySchema, {
              instrumentCode: 'UNCOV_BOND',
              bond: makeStandardBond('S4B2'),
              eligibilityClass: RateCoverageEligibility.UNSPECIFIED,
              calendarRequirement: CalendarRequirement.UNSPECIFIED,
            }),
          ],
        }),
        expectedAccountVersion: 0n,
      });

      await admin.trader.ingestMarketQuotes({
        idempotencyKey: 's4-q-1',
        accountId: 'acct-A',
        sourceId: 'primary-quote-feed',
        ticks: [
          makeQuoteTick({
            sourceId: 'primary-quote-feed',
            quoteSeq: 1n,
            instrumentCode: 'BOND_10Y',
            ytm: '0.025',
          }),
        ],
      });

      preview = await startPreviewWithProxy(server.origin);
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.goto(preview.baseUrl, { waitUntil: 'networkidle' });

      await loginInUi(page, { token: TRADER_TOKEN, headerMode: 'authorization' });
      await submitAccountsInUi(page, 'acct-A');
      await expect(page.getByTestId('account-tab-acct-A')).toBeVisible();
      await expect(page.getByTestId('connection-badge')).toHaveAttribute(
        'data-raw-connection-state',
        'stream_live',
      );
      await preview.proxy.waitForActiveStream(1);

      const initialSeq = await page
        .getByTestId('funding-risk-top-bar')
        .getAttribute('data-snapshot-seq');
      expect(initialSeq).not.toBeNull();

      // 1) Press Ctrl+V outside any editable input -> opens & focuses DraftDrawerPanel textarea
      await page.locator('h1').first().click();
      await page.keyboard.press('Control+v');
      await expect(page.getByTestId('draft-drawer-panel')).toBeVisible();
      await expect(page.getByTestId('draft-paste-textarea')).toBeFocused();

      // 2) Stage 1: Explicit Chinese paste -> UTF-8 anchor highlight + dimensionless confidence + ZERO seq/earmark change
      await dispatchExplicitPaste(page, '买入十年国债BOND_10Y面额一千万净价100.25');
      await expect(page.getByTestId('draft-input-source')).toContainText('显式粘贴');
      await expect(page.getByTestId('draft-proposal-review')).toBeVisible();

      const anchorMark = page.getByTestId('draft-anchor-highlight').first();
      await expect(anchorMark).toHaveText('面额一千万');
      await expect(anchorMark).toHaveAttribute('data-anchor-fields', 'notional');
      await expect(page.getByTestId('proposal-confidence')).toContainText('0.95');
      await expect(page.getByTestId('proposal-confidence')).toContainText(
        u.dimensionless!.unitId!.value,
      );

      // Verify Stage 1 boundary: snapshot_seq unchanged & active_earmarks == 0
      await expect(page.getByTestId('stage-snapshot-seq')).toContainText(initialSeq!);
      await expect(page.getByTestId('stage-active-earmarks-count')).toContainText('0');

      // 3) Regulatory Limit Ceiling Breach (600M > 500M MAX_INST_FACE) -> blocked with zero earmarks!
      await page.getByTestId('draft-paste-textarea').fill('');
      await dispatchExplicitPaste(page, '大额买入十年国债BOND_10Y面额六亿超限测试');
      await expect(page.getByTestId('draft-anchor-highlight').first()).toHaveText('面额六亿');
      await page.getByTestId('draft-submit-button').click();
      await expect(page.getByTestId('draft-regulatory-error')).toBeVisible();
      await expect(page.getByTestId('stage-active-earmarks-count')).toContainText('0');

      // 4) Stage 2: Uncovered Bond with single positive settlement_amount -> Unconfirmed Zero-Leak
      await page.getByTestId('draft-paste-textarea').fill('');
      await dispatchExplicitPaste(page, '买入未覆盖信用债UNCOV_BOND面额五百万仅结算金额');
      await expect(page.getByTestId('draft-anchor-highlight').first()).toHaveText('未覆盖信用债');
      await page.getByTestId('draft-submit-button').click();

      await expect(page.getByTestId('draft-confirmation-section')).toBeVisible();
      await expect(page.getByTestId('draft-drawer-panel')).toHaveAttribute(
        'data-draft-stage',
        'draft_pending',
      );
      await expect(page.getByTestId('draft-rule-lineage')).toContainText(
        '01ARZ3NDEKTSV4RRFFQ69G5F99',
      );
      await expect(page.getByTestId('preview-uncovered-valid')).toBeVisible();
      await expect(page.getByTestId('preview-uncovered-reason')).toBeVisible();
      await expect(page.getByTestId('preview-uncovered-notional')).toBeVisible();
      await expect(page.getByTestId('preview-at-seq')).toBeVisible();

      // Unconfirmed draft MUST leave active_earmarks at 0 and pending_drafts at 1!
      await expect(page.getByTestId('stage-active-earmarks-count')).toContainText('0');
      await expect(page.getByTestId('stage-pending-drafts-count')).toContainText('1');

      // Confirm disabled until manual uncovered risk acknowledgement checkbox is checked
      await expect(page.getByTestId('draft-confirm-button')).toBeDisabled();
      await page.getByTestId('acknowledge-uncovered-checkbox').check();
      await expect(page.getByTestId('draft-confirm-button')).toBeEnabled();

      // 5) Trigger real server CONCURRENCY_CONFLICT by mutating UNCOV_BOND position version & releasing
      const snapMid = (await admin.trader.getAccountSnapshot({ accountId: 'acct-A' })).snapshot!;
      const uncovPos = snapMid.positions.find((p) => p.instrumentCode === 'UNCOV_BOND')!;
      const tempHold = await admin.trader.createHold({
        idempotencyKey: 's4-conflict-hold',
        accountId: 'acct-A',
        expectedPositionVersion: uncovPos.positionVersion,
        instrumentCode: 'UNCOV_BOND',
        side: TradeSide.BUY,
        settlementBucket: SettlementBucket.T_PLUS_0,
        notional: makeDec('1000000', u.currencyAmount!),
        ttlSeconds: 1800,
        note: 'bump-version-for-conflict',
      });
      await admin.trader.releaseHold({
        idempotencyKey: 's4-conflict-release',
        accountId: 'acct-A',
        earmarkId: tempHold.earmark!.earmarkId!,
        expectedEarmarkRevision: tempHold.earmark!.earmarkRevision,
        reason: 'release-after-version-bump',
      });

      // Wait for live stream to deliver released state (active_earmarks back to 0)
      await expect(page.getByTestId('stage-active-earmarks-count')).toContainText('0');

      // Click confirm on stale draft -> real server returns CONCURRENCY_CONFLICT!
      await page.getByTestId('draft-confirm-button').click();
      await expect(page.getByTestId('draft-conflict-notice')).toBeVisible();
      await expect(page.getByTestId('preview-invalidated-badge')).toBeVisible();
      await expect(page.getByTestId('acknowledge-uncovered-checkbox')).toHaveCount(0);
      await expect(page.getByTestId('draft-confirm-button')).toBeDisabled();
      await expect(page.getByTestId('stage-active-earmarks-count')).toContainText('0');

      // 6) Resubmit fresh draft -> manually re-check checkbox -> Stage 3 Confirm succeeds!
      await page.getByTestId('draft-resubmit-button').click();
      await expect(page.getByTestId('draft-conflict-notice')).toHaveCount(0);
      await expect(page.getByTestId('preview-invalidated-badge')).toHaveCount(0);
      await expect(page.getByTestId('acknowledge-uncovered-checkbox')).not.toBeChecked();
      await expect(page.getByTestId('draft-confirm-button')).toBeDisabled();

      await page.getByTestId('acknowledge-uncovered-checkbox').check();
      await expect(page.getByTestId('draft-confirm-button')).toBeEnabled();
      await page.getByTestId('draft-confirm-button').click();

      await expect(page.getByTestId('draft-confirm-receipt')).toBeVisible();
      await expect(page.getByTestId('draft-drawer-panel')).toHaveAttribute(
        'data-draft-stage',
        'confirmed',
      );
      await expect(page.getByTestId('stage-active-earmarks-count')).toContainText('1');

      expect(pageErrors).toEqual([]);
    } finally {
      if (preview) {
        await preview.stop();
      }
      if (server) {
        await server.cleanup();
      }
    }
  });
});
