import '@testing-library/jest-dom/vitest';
import { create } from '@bufbuild/protobuf';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  AccountBlotterSnapshotSchema,
  AccountChangeSchema,
  BlotterEventEnvelopeSchema,
  BucketBalanceViewSchema,
  CalendarResolutionMode,
  CoverageStatus,
  EarmarkRecordSchema,
  EarmarkState,
  FeedHealthState,
  FeedStatusViewSchema,
  FundingPrecision,
  InstrumentPositionViewSchema,
  OpeningInstrumentBalanceSchema,
  OpeningPositionBaseSchema,
  PrincipalRole,
  QuoteTermsSchema,
  RiskAndCoverageSummarySchema,
  SettlementBucket,
  TradeSide,
  UncoveredReason,
  type AccountBlotterSnapshot,
  type EarmarkRecord,
  type InstrumentPositionView,
} from '../../generated/ficant/blotter/v1/model_pb';
import {
  CancelCommittedResultSchema,
  ConfirmHoldResultSchema,
  CreateCommittedResultSchema,
  CreateHoldResultSchema,
  ExtendHoldResultSchema,
  ReleaseHoldResultSchema,
} from '../../generated/ficant/blotter/v1/service_pb';
import { UlidSchema } from '../../generated/ficant/core/v1/common_pb';
import {
  ErrorCode,
  ErrorDetailSchema,
  FieldViolationSchema,
} from '../../generated/ficant/core/v1/error_pb';
import { AnalysisUnitsSchema, BondTaxBasis } from '../../generated/ficant/rates/v1/analytics_pb';
import {
  createDecimalValue,
  createMarketTimeFromIso,
  createUnitRef,
} from '../../codec';
import { createBlotterStore, type BlotterStoreApi } from '../../store';
import { BlotterClientError, WriteIntentManager } from '../../transport';
import { PositionsGridPanel, type PositionsGridRpcClient } from './index';

afterEach(() => {
  cleanup();
});

const ULID_CCY = '01JNB0G1UNITCURRAMT0000001';
const ULID_PRICE = '01JNB0G1UNITPRICE000000002';
const ULID_RATE = '01JNB0G1UNITRATE0000000003';
const ULID_DV01 = '01JNB0G1UNITDV010000000004';
const ULID_DV01P100 = '01JNB0G1UNITDV01P100000005';
const ULID_DIM = '01JNB0G1UNITDIMLESS0000006';
const ULID_CNT = '01JNB0G1UNITCNTRCNT0000007';

const ULID_HOLD_1 = '01JNB0G1H01D00000000000001';
const ULID_HOLD_EXPIRED = '01JNB0G1H01D00000000000002';
const ULID_COMMITTED_1 = '01JNB0G1C0MM00000000000003';

const FIXED_NOW_ISO = '2026-11-22T02:00:00Z';
const FIXED_NOW_MS = Date.parse(FIXED_NOW_ISO);

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

function makePosition(
  accountId: string,
  index: number,
  options: {
    readonly code?: string;
    readonly version?: bigint;
    readonly uncovered?: boolean;
    readonly uncoveredReason?: UncoveredReason;
    readonly uncoveredDetail?: string;
    readonly sellAmbiguityLockedT0?: boolean;
    readonly sellAmbiguityLockedT1?: boolean;
    readonly quoteSourceId?: string;
    readonly quoteSeq?: bigint;
  } = {},
): InstrumentPositionView {
  const units = makeUnits();
  const ccy = units.currencyAmount!;
  const priceUnit = units.pricePer100!;
  const rateUnit = units.rate!;
  const dv01Unit = units.dv01!;
  const code = options.code ?? `INST-${index.toString().padStart(4, '0')}`;
  const isUncovered = options.uncovered ?? false;

  return create(InstrumentPositionViewSchema, {
    accountId,
    instrumentCode: code,
    positionVersion: options.version ?? BigInt(index + 1),
    economicNetNotional: createDecimalValue(String((index + 1) * 100000), ccy),
    availableT0Notional: createDecimalValue(String((index + 1) * 60000), ccy),
    availableT1Notional: createDecimalValue(String((index + 1) * 40000), ccy),
    bucketT0: create(BucketBalanceViewSchema, {
      settlementBucket: SettlementBucket.T_PLUS_0,
      settlementDate: '2026-11-22',
      availableNotional: createDecimalValue(String((index + 1) * 60000), ccy),
      holdBuyNotional: createDecimalValue('200000', ccy),
      holdSellNotional: createDecimalValue('100000', ccy),
      committedBuyUnreconciledNotional: createDecimalValue('300000', ccy),
      committedSellUnreconciledNotional: createDecimalValue('50000', ccy),
      reconciledBuyNotional: createDecimalValue('400000', ccy),
      reconciledSellNotional: createDecimalValue('150000', ccy),
      pendingMatchBuyUnresolvedNotional: createDecimalValue('10000', ccy),
      pendingMatchSellUnresolvedNotional: createDecimalValue('0', ccy),
      sellAmbiguityLocked: options.sellAmbiguityLockedT0 ?? false,
    }),
    bucketT1: create(BucketBalanceViewSchema, {
      settlementBucket: SettlementBucket.T_PLUS_1,
      settlementDate: '2026-11-23',
      availableNotional: createDecimalValue(String((index + 1) * 40000), ccy),
      holdBuyNotional: createDecimalValue('50000', ccy),
      holdSellNotional: createDecimalValue('0', ccy),
      committedBuyUnreconciledNotional: createDecimalValue('100000', ccy),
      committedSellUnreconciledNotional: createDecimalValue('0', ccy),
      reconciledBuyNotional: createDecimalValue('200000', ccy),
      reconciledSellNotional: createDecimalValue('0', ccy),
      pendingMatchBuyUnresolvedNotional: createDecimalValue('0', ccy),
      pendingMatchSellUnresolvedNotional: createDecimalValue('25000', ccy),
      sellAmbiguityLocked: options.sellAmbiguityLockedT1 ?? false,
    }),
    coverageStatus: isUncovered ? CoverageStatus.UNCOVERED : CoverageStatus.COVERED_PRE_TAX,
    taxBasis: BondTaxBasis.PRE_TAX,
    ...(isUncovered
      ? {
          uncoveredReason: options.uncoveredReason ?? UncoveredReason.STALE_QUOTE,
          uncoveredDetail: options.uncoveredDetail ?? 'quote age exceeded policy threshold',
        }
      : {
          preTaxDv01: createDecimalValue('1234.56', dv01Unit),
          valuationCleanPricePer100: createDecimalValue('101.25', priceUnit),
          valuationYieldToMaturity: createDecimalValue('0.0235', rateUnit),
        }),
    calendarResolution: CalendarResolutionMode.EXACT,
    valuationSettlementDate: '2026-11-22',
    activeQuoteSourceId: options.quoteSourceId ?? 'quote-feed-a',
    activeQuoteSeq: options.quoteSeq ?? 42n,
  });
}

function makeEarmark(params: {
  readonly id: string;
  readonly accountId: string;
  readonly instrumentCode: string;
  readonly state: EarmarkState;
  readonly revision?: bigint;
  readonly expiresIso?: string;
  readonly side?: TradeSide;
  readonly bucket?: SettlementBucket;
  readonly overdueAlert?: boolean;
  readonly preCloseAlert?: boolean;
  readonly reopenedByReversal?: boolean;
  readonly settlementAmount?: string;
}): EarmarkRecord {
  const units = makeUnits();
  const ccy = units.currencyAmount!;
  return create(EarmarkRecordSchema, {
    earmarkId: create(UlidSchema, { value: params.id }),
    earmarkRevision: params.revision ?? 3n,
    accountId: params.accountId,
    instrumentCode: params.instrumentCode,
    side: params.side ?? TradeSide.BUY,
    settlementBucket: params.bucket ?? SettlementBucket.T_PLUS_0,
    settlementDate: '2026-11-22',
    notional: createDecimalValue('500000', ccy),
    unreconciledNotional: createDecimalValue('500000', ccy),
    ...(params.settlementAmount !== undefined
      ? {
          quoteTerms: create(QuoteTermsSchema, {
            settlementAmount: createDecimalValue(params.settlementAmount, ccy),
          }),
        }
      : {}),
    fundingPrecision: FundingPrecision.EXACT_SETTLEMENT_AMOUNT,
    state: params.state,
    createdBy: 'trader-1',
    createdAt: createMarketTimeFromIso('2026-11-22T01:30:00Z', 'UTC'),
    ...(params.expiresIso !== undefined
      ? { expiresAt: createMarketTimeFromIso(params.expiresIso, 'UTC') }
      : {}),
    overdueAlert: params.overdueAlert ?? false,
    preCloseAlert: params.preCloseAlert ?? false,
    reopenedByReversal: params.reopenedByReversal ?? false,
    createdSeq: 10n,
    updatedSeq: 12n,
  });
}

function makeSnapshot(params: {
  readonly accountId: string;
  readonly seq: bigint;
  readonly accountVersion?: bigint;
  readonly positions: readonly InstrumentPositionView[];
  readonly activeEarmarks?: readonly EarmarkRecord[];
  readonly feedPaused?: boolean;
}): AccountBlotterSnapshot {
  const units = makeUnits();
  const ccy = units.currencyAmount!;
  const coveredCount = params.positions.filter(
    (p) => p.coverageStatus === CoverageStatus.COVERED_PRE_TAX,
  ).length;
  const uncoveredCount = params.positions.filter(
    (p) => p.coverageStatus === CoverageStatus.UNCOVERED,
  ).length;

  return create(AccountBlotterSnapshotSchema, {
    snapshotSeq: params.seq,
    accountId: params.accountId,
    accountVersion: params.accountVersion ?? 5n,
    openingBase: create(OpeningPositionBaseSchema, {
      accountId: params.accountId,
      units,
      balances: params.positions.slice(0, 5).map((p) =>
        create(OpeningInstrumentBalanceSchema, {
          instrumentCode: p.instrumentCode,
          openingEconomicNotional: createDecimalValue('1000000', ccy),
        }),
      ),
    }),
    riskSummary: create(RiskAndCoverageSummarySchema, {
      coveredInstrumentCount: coveredCount,
      uncoveredInstrumentCount: uncoveredCount,
      coveredTaxBasis: BondTaxBasis.PRE_TAX,
      wholeAccountHedgeAllowed: uncoveredCount === 0,
    }),
    feedStatuses: [
      create(FeedStatusViewSchema, {
        sourceId: 'trade-feed-1',
        feedStatusRevision: 1n,
        healthState: params.feedPaused
          ? FeedHealthState.PAUSED_BY_OPERATOR
          : FeedHealthState.HEALTHY,
        earmarksPaused: params.feedPaused ?? false,
      }),
    ],
    positions: [...params.positions],
    activeEarmarks: [...(params.activeEarmarks ?? [])],
  });
}

function createSeededStore(
  snapshot: AccountBlotterSnapshot,
  roleHint: PrincipalRole | null = PrincipalRole.TRADER,
): { readonly store: BlotterStoreApi; readonly sessionGen: number } {
  const store = createBlotterStore();
  const sessionGen = store.getState().beginSession({
    headerMode: 'authorization',
    roleHint,
    actorLabel: 'trader-1',
  });
  store.getState().setRequestedAccounts([snapshot.accountId], sessionGen);
  const fence = store.getState().activateReader(sessionGen, 'bootstrap', 'bootstrapping')!;
  const commit = store.getState().commitBaselineSnapshots({
    ...fence,
    snapshots: new Map([[snapshot.accountId, snapshot]]),
    cursorSeq: snapshot.snapshotSeq,
    headSeq: snapshot.snapshotSeq,
    nextConnectionState: 'stream_live',
  });
  expect(commit.committed).toBe(true);
  return { store, sessionGen };
}

function createMockClient(
  overrides: Partial<PositionsGridRpcClient> = {},
): PositionsGridRpcClient {
  const unimpl = (): never => {
    throw new Error('Unexpected RPC call');
  };
  return {
    createHold: overrides.createHold ?? unimpl,
    extendHold: overrides.extendHold ?? unimpl,
    confirmHold: overrides.confirmHold ?? unimpl,
    releaseHold: overrides.releaseHold ?? unimpl,
    createCommitted: overrides.createCommitted ?? unimpl,
    cancelCommitted: overrides.cancelCommitted ?? unimpl,
  };
}

describe('PositionsGridPanel — 1,200-row virtualization, bucket strip, coverage & lifecycle display', () => {
  it('mounts only a small visible window for 1,200 rows, exposes data-snapshot-seq, scrolls to row #1190, displays bucket/risk/quote/lifecycle details, and creates a HOLD on row #1190', async () => {
    const user = userEvent.setup();
    const accountId = 'acct-1200';
    const positions: InstrumentPositionView[] = [];
    for (let i = 0; i < 1200; i += 1) {
      positions.push(
        makePosition(accountId, i, {
          uncovered: i === 1191,
          uncoveredReason: i === 1191 ? UncoveredReason.STALE_QUOTE : UncoveredReason.UNSPECIFIED,
          uncoveredDetail: i === 1191 ? 'quote stale > 600s' : '',
          sellAmbiguityLockedT0: i === 1190,
          quoteSourceId: i === 1190 ? 'quote-src-tail' : 'quote-src-main',
          quoteSeq: BigInt(1000 + i),
        }),
      );
    }

    const tailHold = makeEarmark({
      id: ULID_HOLD_1,
      accountId,
      instrumentCode: 'INST-1190',
      state: EarmarkState.HOLD,
      revision: 4n,
      expiresIso: '2026-11-22T02:30:00Z',
      overdueAlert: true,
      preCloseAlert: true,
    });

    const snapshot = makeSnapshot({
      accountId,
      seq: 100n,
      accountVersion: 12n,
      positions,
      activeEarmarks: [tailHold],
    });
    const { store } = createSeededStore(snapshot);

    const createHold = vi.fn<PositionsGridRpcClient['createHold']>(async () =>
      create(CreateHoldResultSchema, {
        committedSeq: 101n,
        replayed: false,
        earmark: tailHold,
      }),
    );

    render(
      <PositionsGridPanel
        store={store}
        client={createMockClient({ createHold })}
        nowMs={() => FIXED_NOW_MS}
        viewportHeightPx={420}
        rowHeightPx={84}
        overscan={3}
      />,
    );

    const panel = screen.getByTestId('positions-grid-panel');
    expect(panel).toHaveAttribute('data-snapshot-seq', '100');

    // Windowed virtualization check: only a small slice of 1,200 rows is mounted in DOM
    const virtualBody = screen.getByTestId('positions-grid-virtual-body');
    const initialMountedRows = within(virtualBody).getAllByRole('row');
    expect(initialMountedRows.length).toBeGreaterThan(0);
    expect(initialMountedRows.length).toBeLessThan(30);
    expect(screen.getByTestId('position-row-INST-0000')).toBeInTheDocument();
    expect(screen.queryByTestId('position-row-INST-1190')).not.toBeInTheDocument();

    // Scroll down to row #1190
    const scrollContainer = screen.getByTestId('positions-grid-scroll');
    Object.defineProperty(scrollContainer, 'scrollTop', {
      configurable: true,
      writable: true,
      value: 1190 * 84,
    });
    fireEvent.scroll(scrollContainer);

    await waitFor(() => {
      expect(screen.getByTestId('position-row-INST-1190')).toBeInTheDocument();
    });
    expect(screen.queryByTestId('position-row-INST-0000')).not.toBeInTheDocument();

    // Verify row #1190 (PRE_TAX covered + sell_ambiguity_locked + quote source + active HOLD badge)
    const row1190 = screen.getByTestId('position-row-INST-1190');
    expect(row1190).toHaveTextContent('INST-1190');
    expect(row1190).toHaveTextContent('PRE_TAX');
    expect(row1190).toHaveTextContent('1,234.56');
    expect(row1190).toHaveTextContent('quote-src-tail');
    expect(row1190).toHaveTextContent('#2190');
    expect(row1190).toHaveTextContent('sell_ambiguity_locked');
    expect(row1190).toHaveTextContent('rev 4');
    expect(row1190).toHaveTextContent('逾期告警');
    expect(row1190).toHaveTextContent('临近收市');

    // Verify row #1191 displays explicit UNCOVERED reason and STALE_QUOTE indicator
    const row1191 = screen.getByTestId('position-row-INST-1191');
    expect(row1191).toHaveTextContent('未覆盖 (UNCOVERED)');
    expect(row1191).toHaveTextContent('STALE_QUOTE');
    expect(row1191).toHaveTextContent('quote stale > 600s');

    // Select row #1190 and verify BucketBalanceStrip updates to INST-1190
    await user.click(row1190);
    const bucketStrip = screen.getByTestId('bucket-balance-strip');
    expect(bucketStrip).toHaveTextContent('INST-1190');
    expect(screen.getByTestId('bucket-balance-t0-sell-lock')).toHaveTextContent(
      'sell_ambiguity_locked',
    );
    expect(screen.getByTestId('bucket-balance-t0')).toHaveTextContent('200,000');
    expect(screen.getByTestId('bucket-balance-t1')).toHaveTextContent('50,000');

    // Submit CreateHold on scrolled row #1190 (omitting quote cleanly)
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    await waitFor(() => {
      expect(createHold).toHaveBeenCalledTimes(1);
    });
    const sentReq = createHold.mock.calls[0]?.[0];
    expect(sentReq?.instrumentCode).toBe('INST-1190');
    expect(sentReq?.expectedPositionVersion).toBe(1191n);
    expect(sentReq?.ttlSeconds).toBe(1800);
    expect(sentReq?.quoteTerms).toBeUndefined();
  });
});

describe('CreateHold & ExtendHold — strict TTL [1800, 2700], quote validation, CAS & sticky idempotency', () => {
  it('validates CreateHold TTL boundaries 1799/1800/2700/2701, positive settlement_amount single-input, rejects empty/0/negative/NaN/malformed quotes, and preserves sticky idempotency_key on transport retry', async () => {
    const user = userEvent.setup();
    const accountId = 'acct-hold';
    const pos = makePosition(accountId, 0, { code: 'BOND-A', version: 7n });
    const snapshot = makeSnapshot({ accountId, seq: 50n, positions: [pos] });
    const { store } = createSeededStore(snapshot);
    const intents = new WriteIntentManager();

    let callCount = 0;
    const createHold = vi.fn<PositionsGridRpcClient['createHold']>(async (req) => {
      callCount += 1;
      return create(CreateHoldResultSchema, {
        committedSeq: BigInt(50 + callCount),
        replayed: false,
        earmark: makeEarmark({
          id: ULID_HOLD_1,
          accountId,
          instrumentCode: req.instrumentCode ?? 'BOND-A',
          state: EarmarkState.HOLD,
          revision: 1n,
        }),
      });
    });

    render(
      <PositionsGridPanel
        store={store}
        client={createMockClient({ createHold })}
        writeIntents={intents}
        nowMs={() => FIXED_NOW_MS}
      />,
    );

    const ttlInput = screen.getByTestId('input-ttl-seconds');
    const submitBtn = screen.getByTestId('hold-form-submit-btn');

    // 1) TTL 1799 -> REJECTED client-side (0 RPCs)
    await user.clear(ttlInput);
    await user.type(ttlInput, '1799');
    await user.click(submitBtn);
    expect(screen.getByTestId('hold-form-validation-errors')).toHaveTextContent('ttl_seconds');
    expect(createHold).toHaveBeenCalledTimes(0);

    // 2) TTL 1800 -> PASSED (RPC #1, quoteTerms omitted cleanly for HOLD)
    await user.clear(ttlInput);
    await user.type(ttlInput, '1800');
    await user.click(submitBtn);
    await waitFor(() => expect(createHold).toHaveBeenCalledTimes(1));
    expect(createHold.mock.calls[0]?.[0]?.ttlSeconds).toBe(1800);
    expect(createHold.mock.calls[0]?.[0]?.quoteTerms).toBeUndefined();
    expect(createHold.mock.calls[0]?.[0]?.expectedPositionVersion).toBe(7n);

    // 3) TTL 2700 -> PASSED (RPC #2)
    await user.clear(ttlInput);
    await user.type(ttlInput, '2700');
    await user.click(submitBtn);
    await waitFor(() => expect(createHold).toHaveBeenCalledTimes(2));
    expect(createHold.mock.calls[1]?.[0]?.ttlSeconds).toBe(2700);

    // 4) TTL 2701 -> REJECTED client-side (still 2 RPCs)
    await user.clear(ttlInput);
    await user.type(ttlInput, '2701');
    await user.click(submitBtn);
    expect(screen.getByTestId('hold-form-validation-errors')).toHaveTextContent('ttl_seconds');
    expect(createHold).toHaveBeenCalledTimes(2);

    // 5) Quote mode: positive settlement_amount single-input validation
    await user.clear(ttlInput);
    await user.type(ttlInput, '1800');
    await user.selectOptions(screen.getByTestId('input-quote-mode'), 'settlement_only');
    const settlementInput = screen.getByTestId('input-settlement-amount');

    for (const invalidValue of ['', '0', '-1000', 'NaN', '12.3.4', '+500']) {
      await user.clear(settlementInput);
      if (invalidValue.length > 0) {
        await user.type(settlementInput, invalidValue);
      }
      await user.click(submitBtn);
      expect(screen.getByTestId('hold-form-validation-errors')).toBeInTheDocument();
      expect(createHold).toHaveBeenCalledTimes(2);
    }

    // Valid positive settlement_amount alone -> PASSED (RPC #3)
    await user.clear(settlementInput);
    await user.type(settlementInput, '1012345.67');
    await user.click(submitBtn);
    await waitFor(() => expect(createHold).toHaveBeenCalledTimes(3));
    const quoteSent = createHold.mock.calls[2]?.[0]?.quoteTerms;
    expect(quoteSent?.priceOrYield?.case).toBeUndefined();
    expect(quoteSent?.settlementAmount?.coefficient).toBe('101234567');
    expect(quoteSent?.settlementAmount?.scale).toBe(2);

    // 6) Transport error preserves sticky idempotency_key on retry; editing payload creates a new key
    createHold.mockRejectedValueOnce(
      new BlotterClientError({
        kind: 'transport',
        message: 'network timeout',
        retryable: true,
      }),
    );
    await user.clear(settlementInput);
    await user.type(settlementInput, '2025000');
    await user.click(submitBtn);
    await waitFor(() => expect(createHold).toHaveBeenCalledTimes(4));
    const failedKey = createHold.mock.calls[3]?.[0]?.idempotencyKey;
    expect(failedKey).toBeTruthy();

    // Retry unchanged -> EXACT same idempotencyKey
    await user.click(submitBtn);
    await waitFor(() => expect(createHold).toHaveBeenCalledTimes(5));
    expect(createHold.mock.calls[4]?.[0]?.idempotencyKey).toBe(failedKey);

    // Change notional -> NEW idempotencyKey
    const notionalInput = screen.getByTestId('input-notional');
    await user.clear(notionalInput);
    await user.type(notionalInput, '2000000');
    await user.click(submitBtn);
    await waitFor(() => expect(createHold).toHaveBeenCalledTimes(6));
    expect(createHold.mock.calls[5]?.[0]?.idempotencyKey).not.toBe(failedKey);
  });

  it('validates ExtendHold TTL boundaries 1799/1800/2700/2701 with CAS expected_earmark_revision and omits quote inputs', async () => {
    const user = userEvent.setup();
    const accountId = 'acct-extend';
    const pos = makePosition(accountId, 0, { code: 'BOND-B', version: 4n });
    const hold = makeEarmark({
      id: ULID_HOLD_1,
      accountId,
      instrumentCode: 'BOND-B',
      state: EarmarkState.HOLD,
      revision: 9n,
      expiresIso: '2026-11-22T02:20:00Z',
    });
    const snapshot = makeSnapshot({
      accountId,
      seq: 60n,
      positions: [pos],
      activeEarmarks: [hold],
    });
    const { store } = createSeededStore(snapshot);

    const extendHold = vi.fn<PositionsGridRpcClient['extendHold']>(async (req) =>
      create(ExtendHoldResultSchema, {
        committedSeq: 61n,
        replayed: false,
        earmark: makeEarmark({
          id: req.earmarkId?.value ?? ULID_HOLD_1,
          accountId,
          instrumentCode: 'BOND-B',
          state: EarmarkState.HOLD,
          revision: 10n,
        }),
      }),
    );

    render(
      <PositionsGridPanel
        store={store}
        client={createMockClient({ extendHold })}
        nowMs={() => FIXED_NOW_MS}
      />,
    );

    await user.click(screen.getByTestId(`earmark-badge-${ULID_HOLD_1}`));
    await user.click(screen.getByTestId('mode-btn-extend_hold'));

    // Quote inputs must be omitted cleanly for ExtendHold
    expect(screen.queryByTestId('quote-terms-fieldset')).not.toBeInTheDocument();

    const ttlInput = screen.getByTestId('input-ttl-seconds');
    const submitBtn = screen.getByTestId('hold-form-submit-btn');

    // 1799 -> rejected
    await user.clear(ttlInput);
    await user.type(ttlInput, '1799');
    await user.click(submitBtn);
    expect(screen.getByTestId('hold-form-validation-errors')).toHaveTextContent('ttl_seconds');
    expect(extendHold).toHaveBeenCalledTimes(0);

    // 1800 -> passed
    await user.clear(ttlInput);
    await user.type(ttlInput, '1800');
    await user.click(submitBtn);
    await waitFor(() => expect(extendHold).toHaveBeenCalledTimes(1));
    expect(extendHold.mock.calls[0]?.[0]?.ttlSeconds).toBe(1800);
    expect(extendHold.mock.calls[0]?.[0]?.expectedEarmarkRevision).toBe(9n);
    expect(extendHold.mock.calls[0]?.[0]?.earmarkId?.value).toBe(ULID_HOLD_1);

    // 2700 -> passed
    await user.clear(ttlInput);
    await user.type(ttlInput, '2700');
    await user.click(submitBtn);
    await waitFor(() => expect(extendHold).toHaveBeenCalledTimes(2));
    expect(extendHold.mock.calls[1]?.[0]?.ttlSeconds).toBe(2700);

    // 2701 -> rejected
    await user.clear(ttlInput);
    await user.type(ttlInput, '2701');
    await user.click(submitBtn);
    expect(screen.getByTestId('hold-form-validation-errors')).toHaveTextContent('ttl_seconds');
    expect(extendHold).toHaveBeenCalledTimes(2);
  });
});

describe('ReleaseHold & CancelCommitted — mandatory reason, CAS & quote omission', () => {
  it('enforces mandatory non-empty reason and CAS on ReleaseHold and CancelCommitted while omitting quote inputs', async () => {
    const user = userEvent.setup();
    const accountId = 'acct-release';
    const pos = makePosition(accountId, 0, { code: 'BOND-C', version: 3n });
    const hold = makeEarmark({
      id: ULID_HOLD_1,
      accountId,
      instrumentCode: 'BOND-C',
      state: EarmarkState.HOLD,
      revision: 5n,
      expiresIso: '2026-11-22T02:30:00Z',
    });
    const committed = makeEarmark({
      id: ULID_COMMITTED_1,
      accountId,
      instrumentCode: 'BOND-C',
      state: EarmarkState.COMMITTED,
      revision: 8n,
    });
    const snapshot = makeSnapshot({
      accountId,
      seq: 70n,
      positions: [pos],
      activeEarmarks: [hold, committed],
    });
    const { store } = createSeededStore(snapshot);

    const releaseHold = vi.fn<PositionsGridRpcClient['releaseHold']>(async () =>
      create(ReleaseHoldResultSchema, {
        committedSeq: 71n,
        replayed: false,
        earmark: makeEarmark({
          id: ULID_HOLD_1,
          accountId,
          instrumentCode: 'BOND-C',
          state: EarmarkState.RELEASED,
          revision: 6n,
        }),
      }),
    );
    const cancelCommitted = vi.fn<PositionsGridRpcClient['cancelCommitted']>(async () =>
      create(CancelCommittedResultSchema, {
        committedSeq: 72n,
        replayed: false,
        earmark: makeEarmark({
          id: ULID_COMMITTED_1,
          accountId,
          instrumentCode: 'BOND-C',
          state: EarmarkState.CANCELLED,
          revision: 9n,
        }),
      }),
    );

    render(
      <PositionsGridPanel
        store={store}
        client={createMockClient({ releaseHold, cancelCommitted })}
        nowMs={() => FIXED_NOW_MS}
      />,
    );

    // ReleaseHold
    await user.click(screen.getByTestId(`earmark-badge-${ULID_HOLD_1}`));
    await user.click(screen.getByTestId('mode-btn-release_hold'));
    expect(screen.queryByTestId('quote-terms-fieldset')).not.toBeInTheDocument();

    const reasonInput = screen.getByTestId('input-reason');
    await user.type(reasonInput, '   ');
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    expect(screen.getByTestId('hold-form-validation-errors')).toHaveTextContent('reason');
    expect(releaseHold).toHaveBeenCalledTimes(0);

    await user.clear(reasonInput);
    await user.type(reasonInput, '客户撤销询价');
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    await waitFor(() => expect(releaseHold).toHaveBeenCalledTimes(1));
    expect(releaseHold.mock.calls[0]?.[0]?.expectedEarmarkRevision).toBe(5n);
    expect(releaseHold.mock.calls[0]?.[0]?.reason).toBe('客户撤销询价');

    // CancelCommitted
    await user.click(screen.getByTestId(`earmark-badge-${ULID_COMMITTED_1}`));
    expect(screen.queryByTestId('quote-terms-fieldset')).not.toBeInTheDocument();
    const cancelReasonInput = screen.getByTestId('input-reason');
    await user.clear(cancelReasonInput);
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    expect(screen.getByTestId('hold-form-validation-errors')).toHaveTextContent('reason');
    expect(cancelCommitted).toHaveBeenCalledTimes(0);

    await user.type(cancelReasonInput, '对手方取消成交');
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    await waitFor(() => expect(cancelCommitted).toHaveBeenCalledTimes(1));
    expect(cancelCommitted.mock.calls[0]?.[0]?.expectedEarmarkRevision).toBe(8n);
    expect(cancelCommitted.mock.calls[0]?.[0]?.reason).toBe('对手方取消成交');
  });
});

describe('ConfirmHold — normal, lazy-expiry COMMITTED, committed EXPIRED + reconfirmation_error, persistent receipts after row removal, idempotent replay & CONCURRENCY_CONFLICT', () => {
  it('supports normal confirmation and expired-at-lazy-eval turning COMMITTED cleanly', async () => {
    const user = userEvent.setup();
    const accountId = 'acct-confirm-ok';
    const pos = makePosition(accountId, 0, { code: 'BOND-D', version: 11n });
    const activeHold = makeEarmark({
      id: ULID_HOLD_1,
      accountId,
      instrumentCode: 'BOND-D',
      state: EarmarkState.HOLD,
      revision: 2n,
      expiresIso: '2026-11-22T02:25:00Z',
    });
    const expiredHold = makeEarmark({
      id: ULID_HOLD_EXPIRED,
      accountId,
      instrumentCode: 'BOND-D',
      state: EarmarkState.HOLD,
      revision: 4n,
      // Past relative to FIXED_NOW_ISO (02:00:00Z)
      expiresIso: '2026-11-22T01:55:00Z',
      settlementAmount: '505000',
    });
    const snapshot = makeSnapshot({
      accountId,
      seq: 80n,
      positions: [pos],
      activeEarmarks: [activeHold, expiredHold],
    });
    const { store } = createSeededStore(snapshot);

    let seq = 80n;
    const confirmHold = vi.fn<PositionsGridRpcClient['confirmHold']>(async (req) => {
      seq += 1n;
      return create(ConfirmHoldResultSchema, {
        committedSeq: seq,
        replayed: false,
        earmark: makeEarmark({
          id: req.earmarkId?.value ?? ULID_HOLD_1,
          accountId,
          instrumentCode: 'BOND-D',
          state: EarmarkState.COMMITTED,
          revision: (req.expectedEarmarkRevision ?? 1n) + 1n,
        }),
      });
    });

    render(
      <PositionsGridPanel
        store={store}
        client={createMockClient({ confirmHold })}
        nowMs={() => FIXED_NOW_MS}
      />,
    );

    // 1) Normal in-term ConfirmHold
    await user.click(screen.getByTestId(`earmark-badge-${ULID_HOLD_1}`));
    await user.type(screen.getByTestId('input-settlement-amount'), '501000');
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    await waitFor(() => expect(confirmHold).toHaveBeenCalledTimes(1));
    expect(confirmHold.mock.calls[0]?.[0]?.expectedEarmarkRevision).toBe(2n);
    expect(confirmHold.mock.calls[0]?.[0]?.expectedPositionVersion).toBe(11n);
    expect(screen.getByTestId('hold-receipts-drawer')).toHaveTextContent('committed_seq: 81');
    expect(screen.getByTestId('hold-receipts-drawer')).toHaveTextContent('COMMITTED');

    // 2) Expired HOLD at lazy-eval turning COMMITTED cleanly
    const expiredBadge = screen.getByTestId(`earmark-badge-${ULID_HOLD_EXPIRED}`);
    expect(expiredBadge).toHaveTextContent('已到期');
    await user.click(expiredBadge);
    // Pre-populated with settlementAmount 505000
    expect(screen.getByTestId('input-settlement-amount')).toHaveValue('505000');
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    await waitFor(() => expect(confirmHold).toHaveBeenCalledTimes(2));
    expect(screen.getByTestId('hold-receipts-drawer')).toHaveTextContent('committed_seq: 82');
    expect(screen.getByTestId('hold-form-feedback')).toHaveTextContent('ConfirmHold 成功转为 COMMITTED');
  });

  it('records committed EXPIRED + structured reconfirmation_error, keeps receipt visible after earmark leaves active_earmarks, replays idempotently with replayed=true & ZERO new seq, and wipes receipts on session switch', async () => {
    const user = userEvent.setup();
    const accountId = 'acct-confirm-exp';
    const pos = makePosition(accountId, 0, { code: 'BOND-E', version: 20n });
    const expiredHold = makeEarmark({
      id: ULID_HOLD_EXPIRED,
      accountId,
      instrumentCode: 'BOND-E',
      state: EarmarkState.HOLD,
      revision: 6n,
      expiresIso: '2026-11-22T01:50:00Z',
      settlementAmount: '502000',
    });
    const snapshot = makeSnapshot({
      accountId,
      seq: 200n,
      positions: [pos],
      activeEarmarks: [expiredHold],
      feedPaused: true,
    });
    const { store, sessionGen } = createSeededStore(snapshot);

    // Simulated backend state tracking headSeq to prove replay creates ZERO new seq
    let serverHeadSeq = 200n;
    const seenKeys = new Map<string, bigint>();

    const confirmHold = vi.fn<PositionsGridRpcClient['confirmHold']>(async (req) => {
      const key = req.idempotencyKey ?? '';
      const existingSeq = seenKeys.get(key);
      const isReplay = existingSeq !== undefined;
      const committedSeq = isReplay ? existingSeq : (serverHeadSeq += 1n);
      if (!isReplay) {
        seenKeys.set(key, committedSeq);
      }
      const result = create(ConfirmHoldResultSchema, {
        committedSeq,
        replayed: isReplay,
        earmark: makeEarmark({
          id: ULID_HOLD_EXPIRED,
          accountId,
          instrumentCode: 'BOND-E',
          state: EarmarkState.EXPIRED,
          revision: 7n,
        }),
        reconfirmationError: create(ErrorDetailSchema, {
          code: ErrorCode.VALIDATION_FAILED,
          message: 'feed source trade-feed-1 is paused',
          traceId: 'trace-exp-201',
          resourceRef: 'feed/trade-feed-1',
          retryable: false,
          fieldViolations: [
            create(FieldViolationSchema, {
              field: 'source_id',
              description: 'earmarks paused by operator',
            }),
          ],
        }),
      });
      // Match real `createDeskBlotterClient().confirmHold` throwing `BlotterClientError.fromConfirmHoldExpired`
      throw BlotterClientError.fromConfirmHoldExpired(result);
    });

    const onWriteCommitted = vi.fn();

    render(
      <PositionsGridPanel
        store={store}
        client={createMockClient({ confirmHold })}
        onWriteCommitted={onWriteCommitted}
        nowMs={() => FIXED_NOW_MS}
      />,
    );

    await user.click(screen.getByTestId(`earmark-badge-${ULID_HOLD_EXPIRED}`));
    await user.click(screen.getByTestId('hold-form-submit-btn'));

    await waitFor(() => expect(confirmHold).toHaveBeenCalledTimes(1));
    expect(serverHeadSeq).toBe(201n);
    expect(onWriteCommitted).toHaveBeenCalledWith(201n);

    const firstIdempotencyKey = confirmHold.mock.calls[0]?.[0]?.idempotencyKey;
    expect(firstIdempotencyKey).toBeTruthy();

    // Receipt drawer displays committed_seq=201, EXPIRED, replayed=false, and structured ErrorDetail
    const drawer = screen.getByTestId('hold-receipts-drawer');
    expect(drawer).toHaveTextContent('committed_seq: 201');
    expect(drawer).toHaveTextContent('replayed=false');
    expect(drawer).toHaveTextContent('EXPIRED');
    expect(drawer).toHaveTextContent('VALIDATION_FAILED');
    expect(drawer).toHaveTextContent('feed source trade-feed-1 is paused');
    expect(drawer).toHaveTextContent('trace-exp-201');
    expect(drawer).toHaveTextContent('feed/trade-feed-1');
    expect(drawer).toHaveTextContent('source_id: earmarks paused by operator');

    // Now simulate server stream delivering seq=201 snapshot where the expired HOLD is REMOVED from active_earmarks!
    const streamFence = store.getState().activateReader(sessionGen, 'stream', 'stream_live')!;
    const updatedSnap = makeSnapshot({
      accountId,
      seq: 201n,
      positions: [makePosition(accountId, 0, { code: 'BOND-E', version: 21n })],
      activeEarmarks: [],
      feedPaused: true,
    });
    store.getState().commitStreamEnvelope({
      ...streamFence,
      envelope: create(BlotterEventEnvelopeSchema, {
        seq: 201n,
        eventType: 'CONFIRM_HOLD_EXPIRED',
        changes: [
          create(AccountChangeSchema, {
            accountId,
            replaceSnapshot: updatedSnap,
          }),
        ],
      }),
    });

    await waitFor(() => {
      expect(screen.getByTestId('positions-grid-panel')).toHaveAttribute('data-snapshot-seq', '201');
    });
    // The active earmark row/badge is gone from the table...
    expect(screen.queryByTestId(`earmark-badge-${ULID_HOLD_EXPIRED}`)).not.toBeInTheDocument();
    // ...YET the persistent receipt stays visible in the receipts drawer!
    expect(screen.getByTestId('hold-receipts-drawer')).toHaveTextContent('committed_seq: 201');
    expect(screen.getByTestId('hold-receipts-drawer')).toHaveTextContent('trace-exp-201');

    // Trigger idempotent replay from the persistent receipt drawer
    const replayButtons = within(screen.getByTestId('hold-receipts-drawer')).getAllByRole(
      'button',
      { name: /重放/ },
    );
    expect(replayButtons.length).toBeGreaterThan(0);
    await user.click(replayButtons[0]!);

    await waitFor(() => expect(confirmHold).toHaveBeenCalledTimes(2));
    // Replay MUST send exact same idempotencyKey and create ZERO new seq!
    expect(confirmHold.mock.calls[1]?.[0]?.idempotencyKey).toBe(firstIdempotencyKey);
    expect(serverHeadSeq).toBe(201n);
    expect(screen.getByTestId('hold-receipts-drawer')).toHaveTextContent('replayed=true');
    expect(screen.getByTestId('hold-receipts-drawer')).toHaveTextContent('committed_seq: 201');

    // Session switch synchronously wipes receipts scoped to the prior sessionGeneration
    store.getState().beginSession({
      headerMode: 'x-ficant-credential',
      roleHint: PrincipalRole.TRADER,
    });
    await waitFor(() => {
      expect(screen.getByTestId('positions-grid-empty')).toBeInTheDocument();
    });
    expect(store.getState().writeReceipts).toHaveLength(0);
  });

  it('handles direct ConfirmHoldResult with reconfirmationError and CONCURRENCY_CONFLICT re-sync + retry', async () => {
    const user = userEvent.setup();
    const accountId = 'acct-conflict';
    const pos = makePosition(accountId, 0, { code: 'BOND-F', version: 30n });
    const hold = makeEarmark({
      id: ULID_HOLD_1,
      accountId,
      instrumentCode: 'BOND-F',
      state: EarmarkState.HOLD,
      revision: 3n,
      expiresIso: '2026-11-22T02:20:00Z',
      settlementAmount: '600000',
    });
    const snapshot = makeSnapshot({
      accountId,
      seq: 300n,
      positions: [pos],
      activeEarmarks: [hold],
    });
    const { store, sessionGen } = createSeededStore(snapshot);
    const onResyncRequested = vi.fn();

    const confirmHold = vi
      .fn<PositionsGridRpcClient['confirmHold']>()
      .mockRejectedValueOnce(
        BlotterClientError.fromErrorDetail(
          create(ErrorDetailSchema, {
            code: ErrorCode.CONCURRENCY_CONFLICT,
            message: 'stale expected_position_version',
            traceId: 'tr-cas-1',
          }),
        ),
      )
      .mockResolvedValueOnce(
        create(ConfirmHoldResultSchema, {
          committedSeq: 302n,
          replayed: false,
          earmark: makeEarmark({
            id: ULID_HOLD_1,
            accountId,
            instrumentCode: 'BOND-F',
            state: EarmarkState.EXPIRED,
            revision: 5n,
          }),
          reconfirmationError: create(ErrorDetailSchema, {
            code: ErrorCode.VALIDATION_FAILED,
            message: 'insufficient capacity on reconfirmation',
            traceId: 'tr-direct-302',
          }),
        }),
      );

    render(
      <PositionsGridPanel
        store={store}
        client={createMockClient({ confirmHold })}
        onResyncRequested={onResyncRequested}
        nowMs={() => FIXED_NOW_MS}
      />,
    );

    await user.click(screen.getByTestId(`earmark-badge-${ULID_HOLD_1}`));
    await user.click(screen.getByTestId('hold-form-submit-btn'));

    await waitFor(() => expect(confirmHold).toHaveBeenCalledTimes(1));
    const conflictKey = confirmHold.mock.calls[0]?.[0]?.idempotencyKey;
    expect(screen.getByTestId('hold-form-submit-error')).toHaveTextContent('CONCURRENCY_CONFLICT');

    // Click Re-sync button
    await user.click(screen.getByTestId('conflict-resync-btn'));
    expect(onResyncRequested).toHaveBeenCalledWith(accountId);

    // Simulate updated snapshot arriving with bumped versions
    const streamFence = store.getState().activateReader(sessionGen, 'stream', 'stream_live')!;
    store.getState().commitStreamEnvelope({
      ...streamFence,
      envelope: create(BlotterEventEnvelopeSchema, {
        seq: 301n,
        eventType: 'RESYNC',
        changes: [
          create(AccountChangeSchema, {
            accountId,
            replaceSnapshot: makeSnapshot({
              accountId,
              seq: 301n,
              positions: [makePosition(accountId, 0, { code: 'BOND-F', version: 31n })],
              activeEarmarks: [
                makeEarmark({
                  id: ULID_HOLD_1,
                  accountId,
                  instrumentCode: 'BOND-F',
                  state: EarmarkState.HOLD,
                  revision: 4n,
                  expiresIso: '2026-11-22T02:20:00Z',
                  settlementAmount: '600000',
                }),
              ],
            }),
          }),
        ],
      }),
    });

    await waitFor(() => {
      expect(screen.getByTestId('positions-grid-panel')).toHaveAttribute('data-snapshot-seq', '301');
    });

    // Retry now uses bumped versions and a fresh idempotencyKey, and decodes direct ConfirmHoldResult.reconfirmationError
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    await waitFor(() => expect(confirmHold).toHaveBeenCalledTimes(2));
    expect(confirmHold.mock.calls[1]?.[0]?.expectedPositionVersion).toBe(31n);
    expect(confirmHold.mock.calls[1]?.[0]?.expectedEarmarkRevision).toBe(4n);
    expect(confirmHold.mock.calls[1]?.[0]?.idempotencyKey).not.toBe(conflictKey);
    expect(screen.getByTestId('hold-receipts-drawer')).toHaveTextContent('committed_seq: 302');
    expect(screen.getByTestId('hold-receipts-drawer')).toHaveTextContent(
      'insufficient capacity on reconfirmation',
    );
  });

  it('supports CreateCommitted with mandatory positive quote and rejects empty quote', async () => {
    const user = userEvent.setup();
    const accountId = 'acct-committed';
    const pos = makePosition(accountId, 0, { code: 'BOND-G', version: 2n });
    const snapshot = makeSnapshot({ accountId, seq: 90n, positions: [pos] });
    const { store } = createSeededStore(snapshot);

    const createCommitted = vi.fn<PositionsGridRpcClient['createCommitted']>(async () =>
      create(CreateCommittedResultSchema, {
        committedSeq: 91n,
        replayed: false,
        earmark: makeEarmark({
          id: ULID_COMMITTED_1,
          accountId,
          instrumentCode: 'BOND-G',
          state: EarmarkState.COMMITTED,
          revision: 1n,
        }),
      }),
    );

    render(
      <PositionsGridPanel
        store={store}
        client={createMockClient({ createCommitted })}
        nowMs={() => FIXED_NOW_MS}
      />,
    );

    await user.click(screen.getByTestId('mode-btn-create_committed'));
    // Empty settlement amount rejected
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    expect(screen.getByTestId('hold-form-validation-errors')).toBeInTheDocument();
    expect(createCommitted).toHaveBeenCalledTimes(0);

    // Valid yield = 0 (non-negative) accepted
    await user.selectOptions(screen.getByTestId('input-quote-mode'), 'yield');
    await user.type(screen.getByTestId('input-yield'), '0');
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    await waitFor(() => expect(createCommitted).toHaveBeenCalledTimes(1));
    expect(createCommitted.mock.calls[0]?.[0]?.quoteTerms?.priceOrYield?.case).toBe(
      'yieldToMaturity',
    );
  });

  it('rejects non-positive clean price and negative yield, disables writes during recovery pause, and isolates receipts per accountId', async () => {
    const user = userEvent.setup();
    const acct1 = 'acct-iso-1';
    const acct2 = 'acct-iso-2';
    const snap1 = makeSnapshot({
      accountId: acct1,
      seq: 400n,
      positions: [makePosition(acct1, 0, { code: 'ISO-1', version: 1n })],
    });
    const snap2 = makeSnapshot({
      accountId: acct2,
      seq: 400n,
      positions: [makePosition(acct2, 0, { code: 'ISO-2', version: 1n, uncovered: true })],
    });

    const store = createBlotterStore();
    const gen = store.getState().beginSession({
      headerMode: 'authorization',
      roleHint: PrincipalRole.TRADER,
    });
    store.getState().setRequestedAccounts([acct1, acct2], gen);
    const fence = store.getState().activateReader(gen, 'bootstrap', 'bootstrapping')!;
    store.getState().commitBaselineSnapshots({
      ...fence,
      snapshots: new Map([
        [acct1, snap1],
        [acct2, snap2],
      ]),
      cursorSeq: 400n,
      headSeq: 400n,
      nextConnectionState: 'stream_live',
    });

    const createHold = vi.fn<PositionsGridRpcClient['createHold']>(async () =>
      create(CreateHoldResultSchema, {
        committedSeq: 401n,
        replayed: false,
        earmark: makeEarmark({
          id: ULID_HOLD_1,
          accountId: acct1,
          instrumentCode: 'ISO-1',
          state: EarmarkState.HOLD,
          revision: 1n,
        }),
      }),
    );

    render(
      <PositionsGridPanel
        store={store}
        client={createMockClient({ createHold })}
        nowMs={() => FIXED_NOW_MS}
      />,
    );

    // Clean price 0 / negative rejected
    await user.selectOptions(screen.getByTestId('input-quote-mode'), 'clean_price');
    const cleanInput = screen.getByTestId('input-clean-price');
    await user.type(cleanInput, '0');
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    expect(screen.getByTestId('hold-form-validation-errors')).toBeInTheDocument();
    expect(createHold).toHaveBeenCalledTimes(0);

    // Yield negative rejected
    await user.selectOptions(screen.getByTestId('input-quote-mode'), 'yield');
    const yieldInput = screen.getByTestId('input-yield');
    await user.type(yieldInput, '-0.01');
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    expect(screen.getByTestId('hold-form-validation-errors')).toBeInTheDocument();
    expect(createHold).toHaveBeenCalledTimes(0);

    // Switch back to omit and submit valid CreateHold on acct1
    await user.selectOptions(screen.getByTestId('input-quote-mode'), 'omit');
    await user.click(screen.getByTestId('hold-form-submit-btn'));
    await waitFor(() => expect(createHold).toHaveBeenCalledTimes(1));
    expect(screen.getByTestId('hold-receipts-drawer')).toHaveTextContent('committed_seq: 401');

    // Switch selected account to acct2 -> acct1 receipt is scoped out of acct2's drawer
    store.getState().setSelectedAccountId(acct2);
    await waitFor(() => {
      expect(screen.getByTestId('hold-receipts-drawer')).toHaveAttribute('data-account-id', acct2);
    });
    expect(screen.getByTestId('receipts-empty')).toBeInTheDocument();

    // Activate recovery pause -> submit button disabled and banner visible
    store.getState().activateReader(gen, 'recovery', 'recovering', {
      recoveryReason: 'journal pruned',
      pauseWrites: true,
    });
    await waitFor(() => {
      expect(screen.getByTestId('writes-paused-banner')).toHaveTextContent('journal pruned');
    });
    expect(screen.getByTestId('hold-form-submit-btn')).toBeDisabled();
  });

  it('filters positions by search text, coverage status, and active earmarks while keeping receipts visible', async () => {
    const user = userEvent.setup();
    const accountId = 'acct-filter';
    const posCovered = makePosition(accountId, 0, { code: 'COV-01' });
    const posUncovered = makePosition(accountId, 1, {
      code: 'UNCOV-02',
      uncovered: true,
      uncoveredReason: UncoveredReason.MISSING_MARKET_QUOTE,
    });
    const hold = makeEarmark({
      id: ULID_HOLD_1,
      accountId,
      instrumentCode: 'COV-01',
      state: EarmarkState.HOLD,
      revision: 1n,
      expiresIso: '2026-11-22T02:30:00Z',
    });
    const snapshot = makeSnapshot({
      accountId,
      seq: 500n,
      positions: [posCovered, posUncovered],
      activeEarmarks: [hold],
    });
    const { store } = createSeededStore(snapshot);

    render(
      <PositionsGridPanel
        store={store}
        client={createMockClient()}
        nowMs={() => FIXED_NOW_MS}
      />,
    );

    expect(screen.getByTestId('position-row-COV-01')).toBeInTheDocument();
    expect(screen.getByTestId('position-row-UNCOV-02')).toBeInTheDocument();

    await user.selectOptions(screen.getByTestId('positions-coverage-filter'), 'uncovered');
    expect(screen.queryByTestId('position-row-COV-01')).not.toBeInTheDocument();
    expect(screen.getByTestId('position-row-UNCOV-02')).toBeInTheDocument();

    await user.selectOptions(screen.getByTestId('positions-coverage-filter'), 'all');
    await user.click(screen.getByTestId('positions-active-earmarks-filter'));
    expect(screen.getByTestId('position-row-COV-01')).toBeInTheDocument();
    expect(screen.queryByTestId('position-row-UNCOV-02')).not.toBeInTheDocument();

    await user.click(screen.getByTestId('positions-active-earmarks-filter'));
    await user.type(screen.getByTestId('positions-search-input'), 'UNCOV');
    expect(screen.queryByTestId('position-row-COV-01')).not.toBeInTheDocument();
    expect(screen.getByTestId('position-row-UNCOV-02')).toBeInTheDocument();
  });
});
