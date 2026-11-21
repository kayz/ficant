import '@testing-library/jest-dom/vitest';
import { create } from '@bufbuild/protobuf';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { TRUSTED_RULEPACK_ANCHOR } from 'virtual:ficant-trusted-rulepack';
import {
  AccountBlotterSnapshotSchema,
  AccountChangeSchema,
  BlotterEventEnvelopeSchema,
  CoverageStatus,
  FeedHealthState,
  FeedStatusViewSchema,
  FundingGapSummarySchema,
  InstrumentPositionViewSchema,
  OpeningPositionBaseSchema,
  PrincipalRole,
  RiskAndCoverageSummarySchema,
  type AccountBlotterSnapshot,
} from '../generated/ficant/blotter/v1/model_pb';
import type { SubscribeEventsResponse } from '../generated/ficant/blotter/v1/service_pb';
import { UlidSchema } from '../generated/ficant/core/v1/common_pb';
import { AnalysisUnitsSchema, BondTaxBasis } from '../generated/ficant/rates/v1/analytics_pb';
import { createDecimalValue, createUnitRef } from '../codec';
import {
  InMemoryCredentialStore,
  WriteIntentManager,
  type DeskBlotterRpcClient,
} from '../transport';
import { BlotterSupervisor, createBlotterStore } from '../store';
import type { EChartsInitFactory, EChartsInstanceLike } from '../features/exposure-chart';
import { DeskApp } from '../app';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const ULID_CCY = '01JNB0G1UNITCURRAMT0000001';
const ULID_PRICE = '01JNB0G1UNITPRICE000000002';
const ULID_RATE = '01JNB0G1UNITRATE0000000003';
const ULID_DV01 = '01JNB0G1UNITDV010000000004';
const ULID_DV01P100 = '01JNB0G1UNITDV01P100000005';
const ULID_DIM = '01JNB0G1UNITDIMLESS0000006';
const ULID_CNT = '01JNB0G1UNITCNTRCNT0000007';

function makeUnits() {
  return create(AnalysisUnitsSchema, {
    currencyAmount: createUnitRef(ULID_CCY, 1n),
    pricePer100: createUnitRef(ULID_PRICE, 1n),
    rate: createUnitRef(ULID_RATE, 1n),
    dv01: createUnitRef(ULID_DV01, 1n),
    dv01Per100: createUnitRef(ULID_DV01P100, 1n),
    dimensionless: createUnitRef(ULID_DIM, 1n),
    contractCount: createUnitRef(ULID_CNT, 1n),
  });
}

function makeSnapshot(accountId: string, seq: bigint, dv01Text = '12500'): AccountBlotterSnapshot {
  const units = makeUnits();
  return create(AccountBlotterSnapshotSchema, {
    snapshotSeq: seq,
    accountId,
    accountVersion: 3n,
    openingBase: create(OpeningPositionBaseSchema, {
      accountId,
      baseId: create(UlidSchema, { value: '01JNB0G1BASEID000000000001' }),
      tradingDate: '2026-11-21',
      sourceId: 'feed-src-1',
      units,
    }),
    riskSummary: create(RiskAndCoverageSummarySchema, {
      coveredInstrumentCount: 1,
      uncoveredInstrumentCount: 0,
      coveredTaxBasis: BondTaxBasis.PRE_TAX,
      coveredNetPreTaxDv01: createDecimalValue(dv01Text, units.dv01!),
      coveredGrossNotional: createDecimalValue('50000000', units.currencyAmount!),
      wholeAccountHedgeAllowed: true,
    }),
    fundingSummary: create(FundingGapSummarySchema, {
      openingCashT0: createDecimalValue('100000000', units.currencyAmount!),
      netSettlementFlowT0: createDecimalValue('0', units.currencyAmount!),
      projectedCashT0: createDecimalValue('100000000', units.currencyAmount!),
      fundingGapT0: createDecimalValue('0', units.currencyAmount!),
      openingNetCashDeltaT1: createDecimalValue('0', units.currencyAmount!),
      netSettlementFlowT1: createDecimalValue('0', units.currencyAmount!),
      projectedCashT1: createDecimalValue('100000000', units.currencyAmount!),
      fundingGapT1: createDecimalValue('0', units.currencyAmount!),
      pendingMatchCashT0: createDecimalValue('0', units.currencyAmount!),
      pendingMatchCashT1: createDecimalValue('0', units.currencyAmount!),
      cleanPriceEstimatedFundingCount: 0,
      unpricedFundingEarmarkCount: 0,
      pendingMatchCount: 0,
      pendingUnknownCashCount: 0,
      feedIncomplete: false,
      fundingIsCompleteExact: true,
    }),
    feedStatuses: [
      create(FeedStatusViewSchema, {
        sourceId: 'feed-src-1',
        feedStatusRevision: 1n,
        healthState: FeedHealthState.HEALTHY,
        earmarksPaused: false,
        pausedByGap: false,
        pausedByStaleness: false,
        pausedByOperator: false,
        missingFeedRanges: [],
      }),
    ],
    positions: [
      create(InstrumentPositionViewSchema, {
        accountId,
        instrumentCode: '240011.IB',
        positionVersion: 2n,
        economicNetNotional: createDecimalValue('50000000', units.currencyAmount!),
        coverageStatus: CoverageStatus.COVERED_PRE_TAX,
        taxBasis: BondTaxBasis.PRE_TAX,
        preTaxDv01: createDecimalValue(dv01Text, units.dv01!),
      }),
    ],
    allowedHedgeProductCodes: ['T', 'TF', 'TL', 'TS'],
  });
}

function createHangingStream(): AsyncIterable<SubscribeEventsResponse> {
  return {
    [Symbol.asyncIterator](): AsyncIterator<SubscribeEventsResponse> {
      return {
        next: () => new Promise<IteratorResult<SubscribeEventsResponse>>(() => undefined),
      };
    },
  };
}

function createStubClient(): DeskBlotterRpcClient {
  const unexpected = (name: string) => () => {
    throw new Error(`Unexpected RPC call in app-shell test: ${name}`);
  };
  return {
    getAccountSnapshot: vi.fn(unexpected('getAccountSnapshot')),
    pollEvents: vi.fn(unexpected('pollEvents')),
    subscribeEvents: vi.fn(() => createHangingStream()),
    previewCoveredHedge: vi.fn(unexpected('previewCoveredHedge')),
    importOpeningBase: vi.fn(unexpected('importOpeningBase')),
    ingestMarketQuotes: vi.fn(unexpected('ingestMarketQuotes')),
    createHold: vi.fn(unexpected('createHold')),
    extendHold: vi.fn(unexpected('extendHold')),
    confirmHold: vi.fn(unexpected('confirmHold')),
    releaseHold: vi.fn(unexpected('releaseHold')),
    createCommitted: vi.fn(unexpected('createCommitted')),
    cancelCommitted: vi.fn(unexpected('cancelCommitted')),
    submitDraftEarmark: vi.fn(unexpected('submitDraftEarmark')),
    confirmDraftEarmark: vi.fn(unexpected('confirmDraftEarmark')),
    ingestTradeFeed: vi.fn(unexpected('ingestTradeFeed')),
    resolvePendingMatch: vi.fn(unexpected('resolvePendingMatch')),
    sweepLifecycle: vi.fn(unexpected('sweepLifecycle')),
    setFeedControl: vi.fn(unexpected('setFeedControl')),
    fetchOpeningPositionBase: vi.fn(unexpected('fetchOpeningPositionBase')),
    parseNlpIntent: vi.fn(unexpected('parseNlpIntent')),
  };
}

function createMockEChartsFactory(): EChartsInitFactory {
  return () => {
    let disposed = false;
    const instance: EChartsInstanceLike = {
      setOption: vi.fn(),
      resize: vi.fn(),
      dispose: vi.fn(() => {
        disposed = true;
      }),
      isDisposed: () => disposed,
    };
    return instance;
  };
}

function createHarness() {
  const store = createBlotterStore();
  const credentialStore = new InMemoryCredentialStore();
  const writeIntents = new WriteIntentManager();
  const client = createStubClient();
  const supervisor = new BlotterSupervisor({
    store,
    client,
    credentialStore,
    writeIntents,
    location: { protocol: 'http:', hostname: '127.0.0.1', port: '4173', origin: 'http://127.0.0.1:4173' },
  });
  return { store, credentialStore, writeIntents, client, supervisor };
}

function setWindowViewport(width: number, height: number): void {
  Object.defineProperty(window, 'innerWidth', {
    configurable: true,
    writable: true,
    value: width,
  });
  Object.defineProperty(window, 'innerHeight', {
    configurable: true,
    writable: true,
    value: height,
  });
}

describe('DeskApp formal shell integration', () => {
  it('mounts cleanly in empty unauthenticated state with RulePack anchor, credential entry, and account controls (zero RPCs)', async () => {
    const user = userEvent.setup();
    const { store, client, supervisor, writeIntents } = createHarness();

    render(
      <DeskApp
        store={store}
        client={client}
        supervisor={supervisor}
        writeIntents={writeIntents}
        initialViewportWidth={1440}
        echartsFactory={createMockEChartsFactory()}
        location={{ protocol: 'http:', hostname: '127.0.0.1', port: '4173', origin: 'http://127.0.0.1:4173' }}
      />,
    );

    const shell = screen.getByTestId('desk-app-shell');
    expect(shell).toBeVisible();
    expect(shell).toHaveAttribute('data-layout', 'wide');
    expect(screen.getByRole('heading', { level: 1 })).toBeVisible();

    const rulepackBadge = screen.getByTestId('trusted-rulepack-id');
    expect(rulepackBadge).toHaveTextContent(TRUSTED_RULEPACK_ANCHOR.manifest.rule_pack_id);
    expect(rulepackBadge).toHaveTextContent(`v${TRUSTED_RULEPACK_ANCHOR.manifest.version}`);

    // Initial unauthenticated mount exposes credential form and pre-auth account staging input.
    expect(screen.getByTestId('auth-form')).toBeVisible();
    const tokenInput = document.getElementById('session-token-input') as HTMLInputElement | null;
    expect(tokenInput).not.toBeNull();
    expect(tokenInput).toBeVisible();
    expect(screen.getByTestId('cold-start-account-ids-input')).toBeVisible();

    // No snapshot seq attributes before any account snapshot is loaded.
    expect(screen.getByTestId('funding-risk-top-bar')).not.toHaveAttribute('data-snapshot-seq');
    expect(screen.getByTestId('positions-grid-panel')).not.toHaveAttribute('data-snapshot-seq');
    expect(screen.getByTestId('exposure-chart-panel')).not.toHaveAttribute('data-snapshot-seq');

    // Stage an account hint and establish an in-memory session (zero RPCs).
    await user.type(screen.getByTestId('cold-start-account-ids-input'), 'acct-001');
    await user.type(tokenInput!, 'valid-inmemory-token');
    await user.click(screen.getByTestId('login-submit-btn'));

    expect(screen.getByTestId('accounts-form')).toBeVisible();
    expect(document.getElementById('explicit-account-ids-input')).toBeVisible();
    expect(screen.getByTestId('fill-account-hints-btn')).toHaveTextContent('acct-001');
    expect(screen.queryByTestId('cold-start-account-staging')).toBeNull();
    expect(client.getAccountSnapshot).not.toHaveBeenCalled();
    expect(client.subscribeEvents).not.toHaveBeenCalled();
    expect(client.pollEvents).not.toHaveBeenCalled();
  });

  it('switches cleanly between 480x900 narrow layout and 1440x900 wide layout on viewport resize', () => {
    const { store, client, supervisor, writeIntents } = createHarness();
    setWindowViewport(480, 900);

    render(
      <DeskApp
        store={store}
        client={client}
        supervisor={supervisor}
        writeIntents={writeIntents}
        initialViewportWidth={480}
        echartsFactory={createMockEChartsFactory()}
      />,
    );

    const shell = screen.getByTestId('desk-app-shell');
    expect(shell).toHaveAttribute('data-layout', 'narrow');
    expect(shell).toHaveAttribute('data-viewport-width', '480');
    expect(screen.getByTestId('narrow-panel-nav')).toBeVisible();

    // All three snapshot-synced panels remain mounted in narrow layout.
    expect(screen.getByTestId('funding-risk-top-bar')).toBeInTheDocument();
    expect(screen.getByTestId('positions-grid-panel')).toBeInTheDocument();
    expect(screen.getByTestId('exposure-chart-panel')).toBeInTheDocument();

    // Resize viewport to 1440x900 wide desk layout.
    act(() => {
      setWindowViewport(1440, 900);
      window.dispatchEvent(new Event('resize'));
    });

    expect(shell).toHaveAttribute('data-layout', 'wide');
    expect(shell).toHaveAttribute('data-viewport-width', '1440');
    expect(screen.queryByTestId('narrow-panel-nav')).toBeNull();

    // Resize back to 480x900 narrow layout.
    act(() => {
      setWindowViewport(480, 900);
      window.dispatchEvent(new Event('resize'));
    });

    expect(shell).toHaveAttribute('data-layout', 'narrow');
    expect(screen.getByTestId('narrow-panel-nav')).toBeVisible();
  });

  it('opens and focuses DraftDrawer on Ctrl+V outside inputs, restores trigger focus on Escape, and never hijacks input paste or calls navigator.clipboard.readText', () => {
    const { store, client, supervisor, writeIntents } = createHarness();

    const readTextSpy = vi.fn(async () => 'should-never-be-read');
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { readText: readTextSpy },
    });

    render(
      <DeskApp
        store={store}
        client={client}
        supervisor={supervisor}
        writeIntents={writeIntents}
        initialViewportWidth={1440}
        echartsFactory={createMockEChartsFactory()}
      />,
    );

    const toggleBtn = screen.getByTestId('draft-drawer-toggle-btn');
    expect(screen.queryByTestId('draft-drawer-panel')).toBeNull();

    // 1. Focus inside an <input> and press Ctrl+V: must NOT open DraftDrawer or hijack paste.
    const tokenInput = document.getElementById('session-token-input') as HTMLInputElement;
    tokenInput.focus();
    const inputPasteEvent = new KeyboardEvent('keydown', {
      key: 'v',
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    tokenInput.dispatchEvent(inputPasteEvent);
    expect(inputPasteEvent.defaultPrevented).toBe(false);
    expect(screen.queryByTestId('draft-drawer-panel')).toBeNull();

    // 2. Focus non-editable element (toggle button) and press Ctrl+V: opens drawer & focuses textarea.
    toggleBtn.focus();
    fireEvent.keyDown(window, { key: 'v', ctrlKey: true });

    expect(screen.getByTestId('draft-drawer-panel')).toBeVisible();
    const textarea = screen.getByTestId('draft-paste-textarea');
    expect(document.activeElement).toBe(textarea);

    // 3. Pressing Ctrl+V while already focused in draft textarea must NOT be prevented.
    const textareaPasteKey = new KeyboardEvent('keydown', {
      key: 'v',
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    textarea.dispatchEvent(textareaPasteKey);
    expect(textareaPasteKey.defaultPrevented).toBe(false);

    // 4. Pressing Escape closes drawer and restores focus to the toggle button.
    fireEvent.keyDown(textarea, { key: 'Escape' });
    expect(screen.queryByTestId('draft-drawer-panel')).toBeNull();
    expect(document.activeElement).toBe(toggleBtn);

    // 5. Silent clipboard read API was NEVER invoked.
    expect(readTextSpy).not.toHaveBeenCalled();
  });

  it('synchronizes data-snapshot-seq across TopBar, PositionsGrid, and ExposureChart on publish, advance, account switch, and clears on logout', async () => {
    const user = userEvent.setup();
    const { store, client, supervisor, writeIntents } = createHarness();

    // Establish authenticated session and publish two accounts via the store's single committer.
    const gen = supervisor.login({
      token: 'trader-token-01',
      headerMode: 'authorization',
      roleHint: PrincipalRole.TRADER,
    });
    store.getState().setRequestedAccounts(['acct-A', 'acct-B'], gen);
    const fence = store.getState().activateReader(gen, 'stream', 'stream_live');
    expect(fence).not.toBeNull();

    const snapA101 = makeSnapshot('acct-A', 101n, '12500');
    const snapB102 = makeSnapshot('acct-B', 102n, '24800');
    const baselineCommit = store.getState().commitBaselineSnapshots({
      sessionGeneration: fence!.sessionGeneration,
      transportEpoch: fence!.transportEpoch,
      readerId: fence!.readerId,
      snapshots: new Map([
        ['acct-A', snapA101],
        ['acct-B', snapB102],
      ]),
      cursorSeq: 102n,
      headSeq: 102n,
      nextConnectionState: 'stream_live',
    });
    expect(baselineCommit.committed).toBe(true);
    store.getState().setSelectedAccountId('acct-A');

    render(
      <DeskApp
        store={store}
        client={client}
        supervisor={supervisor}
        writeIntents={writeIntents}
        initialViewportWidth={1440}
        echartsFactory={createMockEChartsFactory()}
      />,
    );

    const topBar = screen.getByTestId('funding-risk-top-bar');
    const grid = screen.getByTestId('positions-grid-panel');
    const chart = screen.getByTestId('exposure-chart-panel');

    // Initial selected account acct-A -> all three show seq 101.
    expect(topBar).toHaveAttribute('data-snapshot-seq', '101');
    expect(grid).toHaveAttribute('data-snapshot-seq', '101');
    expect(chart).toHaveAttribute('data-snapshot-seq', '101');

    // Advance controlled snapshot for acct-A via stream envelope to seq 105.
    act(() => {
      const env105 = create(BlotterEventEnvelopeSchema, {
        seq: 105n,
        changes: [
          create(AccountChangeSchema, {
            accountId: 'acct-A',
            replaceSnapshot: makeSnapshot('acct-A', 105n, '18900'),
          }),
        ],
      });
      const res = store.getState().commitStreamEnvelope({
        sessionGeneration: fence!.sessionGeneration,
        transportEpoch: fence!.transportEpoch,
        readerId: fence!.readerId,
        envelope: env105,
      });
      expect(res.committed).toBe(true);
    });

    expect(screen.getByTestId('funding-risk-top-bar')).toHaveAttribute('data-snapshot-seq', '105');
    expect(screen.getByTestId('positions-grid-panel')).toHaveAttribute('data-snapshot-seq', '105');
    expect(screen.getByTestId('exposure-chart-panel')).toHaveAttribute('data-snapshot-seq', '105');

    // Switch selected account to acct-B via SessionPanel switcher -> all three show seq 102.
    await user.click(screen.getByTestId('account-tab-acct-B'));
    expect(screen.getByTestId('funding-risk-top-bar')).toHaveAttribute('data-snapshot-seq', '102');
    expect(screen.getByTestId('positions-grid-panel')).toHaveAttribute('data-snapshot-seq', '102');
    expect(screen.getByTestId('exposure-chart-panel')).toHaveAttribute('data-snapshot-seq', '102');

    // Logout -> clears snapshots and removes data-snapshot-seq across all three surfaces.
    await user.click(screen.getByTestId('logout-btn'));
    expect(screen.getByTestId('funding-risk-top-bar')).not.toHaveAttribute('data-snapshot-seq');
    expect(screen.getByTestId('positions-grid-panel')).not.toHaveAttribute('data-snapshot-seq');
    expect(screen.getByTestId('exposure-chart-panel')).not.toHaveAttribute('data-snapshot-seq');
    expect(screen.getByTestId('auth-form')).toBeVisible();
  });
});
