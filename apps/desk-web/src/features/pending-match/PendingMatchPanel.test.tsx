import { create } from '@bufbuild/protobuf';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  AccountBlotterSnapshotSchema,
  AccountChangeSchema,
  BaseEvidenceSchema,
  BaselineDeltaSchema,
  BlotterEventEnvelopeSchema,
  CoverageStatus,
  EarmarkRecordSchema,
  EarmarkState,
  FeedHealthState,
  FeedStatusViewSchema,
  FundingGapSummarySchema,
  InstrumentPositionViewSchema,
  OpeningPositionBaseSchema,
  PendingMatchReason,
  PendingMatchRecordSchema,
  PendingMatchResolutionAction,
  PendingMatchState,
  PollEventsBatchSchema,
  ReconcileAllocationViewSchema,
  RiskAndCoverageSummarySchema,
  SeqRangeSchema,
  SettlementBucket,
  SnapshotFallbackRequiredSchema,
  TradeSide,
  type AccountBlotterSnapshot,
  type BaseEvidence,
  type BlotterEventEnvelope,
  type EarmarkRecord,
  type FeedStatusView,
  type FundingGapSummary,
  type PendingMatchRecord,
  type ReconcileAllocationView,
} from '../../generated/ficant/blotter/v1/model_pb';
import {
  PollEventsResultSchema,
  ResolvePendingMatchResultSchema,
  type PollEventsRequest,
  type PollEventsResult,
  type ResolvePendingMatchRequest,
  type ResolvePendingMatchResult,
} from '../../generated/ficant/blotter/v1/service_pb';
import { UlidSchema } from '../../generated/ficant/core/v1/common_pb';
import { ErrorCode, ErrorDetailSchema } from '../../generated/ficant/core/v1/error_pb';
import { AnalysisUnitsSchema, BondTaxBasis } from '../../generated/ficant/rates/v1/analytics_pb';
import {
  createDecimalValue,
  createMarketTimeFromIso,
  createSha256FromHex,
  createUnitRef,
} from '../../codec';
import { createBlotterStore, type BlotterStoreApi } from '../../store/blotterStore';
import { BlotterClientError, WriteIntentManager, type DeskBlotterRpcClient } from '../../transport';
import {
  PendingMatchPanel,
  UNKNOWN_SERVER_CASH_IMPACT_LABEL,
  buildResolvePendingMatchRequest,
} from './index';

const ULID_CCY = '01JNB0G1UNITCURRAMT0000001';
const ULID_PRICE = '01JNB0G1UNITPRICE000000002';
const ULID_RATE = '01JNB0G1UNITRATE0000000003';
const ULID_DV01 = '01JNB0G1UNITDV010000000004';
const ULID_DV01P100 = '01JNB0G1UNITDV01P100000005';
const ULID_DIM = '01JNB0G1UNITDIMLESS0000006';
const ULID_CNT = '01JNB0G1UNITCNTRCNT0000007';

const PM_ID_1 = '01JNB0G1PM0000000000000001';
const PM_ID_2 = '01JNB0G1PM0000000000000002';
const PM_ID_WRONG = '01JNB0G1PM0000000000000099';
const EM_ID_FULL = '01JNB0G1EM0000000000000011';
const EM_ID_SMALL = '01JNB0G1EM0000000000000012';
const EM_ID_WRONG_SIDE = '01JNB0G1EM0000000000000013';
const EM_ID_WRONG_DATE = '01JNB0G1EM0000000000000014';
const EM_ID_HOLD = '01JNB0G1EM0000000000000015';
const ALLOC_ID_1 = '01JNB0G1ALLOC0000000000001';
const ALLOC_ID_2 = '01JNB0G1ALLOC0000000000002';
const ALLOC_ID_OTHER = '01JNB0G1ALLOC0000000000099';
const EVIDENCE_HASH_HEX = 'a'.repeat(64);

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

function dec(value: string) {
  return createDecimalValue(value, createUnitRef(ULID_CCY, 1n));
}

function makeEvidence(overrides: Partial<{
  readonly sourceId: string;
  readonly externalTradeId: string;
  readonly instrumentCode: string;
  readonly side: TradeSide;
  readonly settlementBucket: SettlementBucket;
  readonly originalRevision: bigint;
  readonly oldNotional: string;
  readonly oldSignedCash: string;
  readonly evidenceRef: string;
}> = {}): BaseEvidence {
  return create(BaseEvidenceSchema, {
    sourceId: overrides.sourceId ?? 'feed-1',
    externalTradeId: overrides.externalTradeId ?? 'ext-base-100',
    originalFeedSeq: 5n,
    originalExecutedAt: createMarketTimeFromIso('2026-06-15T01:30:00Z', 'Asia/Shanghai', '2026-06-15'),
    oldNotional: dec(overrides.oldNotional ?? '10000000'),
    oldSignedCash: dec(overrides.oldSignedCash ?? '-10050000.25'),
    settlementBucket: overrides.settlementBucket ?? SettlementBucket.T_PLUS_0,
    evidenceRef: overrides.evidenceRef ?? 'ev-ref-base-100',
    evidenceHash: createSha256FromHex(EVIDENCE_HASH_HEX),
    instrumentCode: overrides.instrumentCode ?? 'BOND-10Y',
    side: overrides.side ?? TradeSide.BUY,
    originalRevision: overrides.originalRevision ?? 1n,
  });
}

function makeEarmark(overrides: Partial<{
  readonly id: string;
  readonly revision: bigint;
  readonly accountId: string;
  readonly instrumentCode: string;
  readonly side: TradeSide;
  readonly settlementBucket: SettlementBucket;
  readonly settlementDate: string;
  readonly notional: string;
  readonly unreconciledNotional: string;
  readonly state: EarmarkState;
}> = {}): EarmarkRecord {
  return create(EarmarkRecordSchema, {
    earmarkId: create(UlidSchema, { value: overrides.id ?? EM_ID_FULL }),
    earmarkRevision: overrides.revision ?? 3n,
    accountId: overrides.accountId ?? 'acct-A',
    instrumentCode: overrides.instrumentCode ?? 'BOND-10Y',
    side: overrides.side ?? TradeSide.BUY,
    settlementBucket: overrides.settlementBucket ?? SettlementBucket.T_PLUS_0,
    settlementDate: overrides.settlementDate ?? '2026-06-15',
    notional: dec(overrides.notional ?? '10000000'),
    unreconciledNotional: dec(overrides.unreconciledNotional ?? '10000000'),
    state: overrides.state ?? EarmarkState.COMMITTED,
  });
}

function makePendingMatch(overrides: Partial<{
  readonly id: string;
  readonly revision: bigint;
  readonly accountId: string;
  readonly instrumentCode: string;
  readonly side: TradeSide;
  readonly settlementBucket: SettlementBucket;
  readonly settlementDate: string;
  readonly sourceId: string;
  readonly externalTradeId: string;
  readonly sourceRevision: bigint;
  readonly originalNotional: string;
  readonly unresolvedNotional: string;
  readonly unresolvedSignedCash: string | null;
  readonly state: PendingMatchState;
  readonly reason: PendingMatchReason;
  readonly candidateEarmarkIds: readonly string[];
  readonly evidence: BaseEvidence | undefined;
}> = {}): PendingMatchRecord {
  const cashOpt =
    overrides.unresolvedSignedCash === null
      ? undefined
      : dec(overrides.unresolvedSignedCash ?? '-10123456.78');
  return create(PendingMatchRecordSchema, {
    pendingMatchId: create(UlidSchema, { value: overrides.id ?? PM_ID_1 }),
    pendingRevision: overrides.revision ?? 4n,
    accountId: overrides.accountId ?? 'acct-A',
    instrumentCode: overrides.instrumentCode ?? 'BOND-10Y',
    side: overrides.side ?? TradeSide.BUY,
    settlementBucket: overrides.settlementBucket ?? SettlementBucket.T_PLUS_0,
    settlementDate: overrides.settlementDate ?? '2026-06-15',
    sourceId: overrides.sourceId ?? 'feed-1',
    externalTradeId: overrides.externalTradeId ?? 'ext-trade-1',
    sourceRevision: overrides.sourceRevision ?? 1n,
    originalNotional: dec(overrides.originalNotional ?? '10000000'),
    unresolvedNotional: dec(overrides.unresolvedNotional ?? '10000000'),
    ...(cashOpt !== undefined ? { unresolvedSignedCash: cashOpt } : {}),
    state: overrides.state ?? PendingMatchState.OPEN,
    reason: overrides.reason ?? PendingMatchReason.NOTIONAL_MISMATCH_OR_SPLIT,
    candidateEarmarkIds: (overrides.candidateEarmarkIds ?? []).map((value) =>
      create(UlidSchema, { value }),
    ),
    ...(overrides.evidence !== undefined ? { evidence: overrides.evidence } : {}),
  });
}

function makeFeedStatus(overrides: Partial<{
  readonly sourceId: string;
  readonly missingRanges: readonly [bigint, bigint][];
  readonly healthState: FeedHealthState;
}> = {}): FeedStatusView {
  const ranges = overrides.missingRanges ?? [];
  return create(FeedStatusViewSchema, {
    sourceId: overrides.sourceId ?? 'feed-1',
    feedStatusRevision: 1n,
    healthState:
      overrides.healthState ??
      (ranges.length > 0 ? FeedHealthState.GAP_DETECTED : FeedHealthState.HEALTHY),
    pausedByGap: ranges.length > 0,
    missingFeedRanges: ranges.map(([first, last]) =>
      create(SeqRangeSchema, { first, last }),
    ),
  });
}

function makeFundingSummary(overrides: Partial<{
  readonly projectedCashT0: string;
  readonly projectedCashT1: string;
  readonly pendingMatchCashT0: string;
  readonly pendingMatchCashT1: string;
}> = {}): FundingGapSummary {
  return create(FundingGapSummarySchema, {
    projectedCashT0: dec(overrides.projectedCashT0 ?? '50000000'),
    projectedCashT1: dec(overrides.projectedCashT1 ?? '20000000'),
    pendingMatchCashT0: dec(overrides.pendingMatchCashT0 ?? '-10123456.78'),
    pendingMatchCashT1: dec(overrides.pendingMatchCashT1 ?? '0'),
    fundingIsCompleteExact: false,
  });
}

function makeSnapshot(options: {
  readonly accountId?: string;
  readonly seq?: bigint;
  readonly positionVersion?: bigint;
  readonly pendingMatches?: readonly PendingMatchRecord[];
  readonly activeEarmarks?: readonly EarmarkRecord[];
  readonly feedStatuses?: readonly FeedStatusView[];
  readonly fundingSummary?: FundingGapSummary;
} = {}): AccountBlotterSnapshot {
  const accountId = options.accountId ?? 'acct-A';
  return create(AccountBlotterSnapshotSchema, {
    snapshotSeq: options.seq ?? 20n,
    accountId,
    accountVersion: 1n,
    openingBase: create(OpeningPositionBaseSchema, {
      accountId,
      units: makeUnits(),
    }),
    riskSummary: create(RiskAndCoverageSummarySchema, {
      coveredInstrumentCount: 1,
      uncoveredInstrumentCount: 0,
      coveredTaxBasis: BondTaxBasis.PRE_TAX,
      wholeAccountHedgeAllowed: true,
    }),
    fundingSummary: options.fundingSummary ?? makeFundingSummary(),
    feedStatuses: [...(options.feedStatuses ?? [makeFeedStatus()])],
    positions: [
      create(InstrumentPositionViewSchema, {
        accountId,
        instrumentCode: 'BOND-10Y',
        positionVersion: options.positionVersion ?? 7n,
        coverageStatus: CoverageStatus.COVERED_PRE_TAX,
        taxBasis: BondTaxBasis.PRE_TAX,
      }),
    ],
    activeEarmarks: [...(options.activeEarmarks ?? [])],
    openPendingMatches: [...(options.pendingMatches ?? [])],
  });
}

function makeAllocationView(overrides: Partial<{
  readonly allocationId: string;
  readonly pendingMatchId: string;
  readonly earmarkId: string | null;
  readonly allocatedNotional: string;
  readonly allocatedFillCash: string;
  readonly releasedEarmarkCash: string | null;
  readonly actorId: string;
  readonly baselineDelta: ReturnType<typeof create<typeof BaselineDeltaSchema>>;
}> = {}): ReconcileAllocationView {
  return create(ReconcileAllocationViewSchema, {
    allocationId: create(UlidSchema, { value: overrides.allocationId ?? ALLOC_ID_1 }),
    sourceId: 'feed-1',
    externalTradeId: 'ext-trade-1',
    sourceRevision: 1n,
    pendingMatchId: create(UlidSchema, { value: overrides.pendingMatchId ?? PM_ID_1 }),
    ...(overrides.earmarkId !== null && overrides.earmarkId !== undefined
      ? { earmarkId: create(UlidSchema, { value: overrides.earmarkId }) }
      : {}),
    allocatedNotional: dec(overrides.allocatedNotional ?? '3000000'),
    allocatedFillCash: dec(overrides.allocatedFillCash ?? '-3037037.04'),
    ...(overrides.releasedEarmarkCash !== null && overrides.releasedEarmarkCash !== undefined
      ? { releasedEarmarkCash: dec(overrides.releasedEarmarkCash) }
      : {}),
    ...(overrides.baselineDelta !== undefined ? { baselineDelta: overrides.baselineDelta } : {}),
    actorId: overrides.actorId ?? 'trader-1',
  });
}

function setupStoreWithSnapshot(
  snapshot: AccountBlotterSnapshot,
  extraSnapshots: readonly AccountBlotterSnapshot[] = [],
): {
  readonly store: BlotterStoreApi;
  readonly sessionGeneration: number;
  readonly commitEnvelope: (envelope: BlotterEventEnvelope) => void;
} {
  const store = createBlotterStore();
  const sessionGeneration = store.getState().beginSession({
    headerMode: 'authorization',
    actorLabel: 'trader-1',
  });
  const all = [snapshot, ...extraSnapshots];
  store.getState().setRequestedAccounts(
    all.map((s) => s.accountId),
    sessionGeneration,
  );
  const fence = store
    .getState()
    .activateReader(sessionGeneration, 'stream', 'stream_live');
  if (!fence) {
    throw new Error('Failed to activate test reader');
  }

  const snapMap = new Map<string, AccountBlotterSnapshot>();
  for (const s of all) {
    snapMap.set(s.accountId, s);
  }
  const outcome = store.getState().commitBaselineSnapshots({
    sessionGeneration: fence.sessionGeneration,
    transportEpoch: fence.transportEpoch,
    readerId: fence.readerId,
    snapshots: snapMap,
    cursorSeq: snapshot.snapshotSeq,
    headSeq: snapshot.snapshotSeq,
    nextConnectionState: 'stream_live',
  });
  if (!outcome.committed) {
    throw new Error(`Failed to commit baseline snapshot: ${outcome.reason}`);
  }

  return {
    store,
    sessionGeneration,
    commitEnvelope(envelope: BlotterEventEnvelope) {
      const res = store.getState().commitStreamEnvelope({
        sessionGeneration: fence.sessionGeneration,
        transportEpoch: fence.transportEpoch,
        readerId: fence.readerId,
        envelope,
      });
      if (!res.committed) {
        throw new Error(`Failed to commit envelope seq=${envelope.seq.toString()}: ${res.reason}`);
      }
    },
  };
}

function makeEnvelope(
  seq: bigint,
  changes: Array<{
    readonly accountId: string;
    readonly snapshot: AccountBlotterSnapshot;
    readonly allocations?: readonly ReconcileAllocationView[];
  }>,
): BlotterEventEnvelope {
  return create(BlotterEventEnvelopeSchema, {
    seq,
    eventType: 'RESOLVE_PENDING_MATCH',
    changes: changes.map((c) =>
      create(AccountChangeSchema, {
        accountId: c.accountId,
        replaceSnapshot: c.snapshot,
        changedAllocations: [...(c.allocations ?? [])],
      }),
    ),
  });
}

function makeMockClient(overrides: {
  readonly resolvePendingMatch?: (
    req: ResolvePendingMatchRequest,
  ) => Promise<ResolvePendingMatchResult>;
  readonly pollEvents?: (req: PollEventsRequest) => Promise<PollEventsResult>;
} = {}): {
  readonly client: Pick<DeskBlotterRpcClient, 'resolvePendingMatch' | 'pollEvents'>;
  readonly resolveMock: ReturnType<typeof vi.fn>;
  readonly pollMock: ReturnType<typeof vi.fn>;
} {
  const resolveMock = vi.fn(
    overrides.resolvePendingMatch ??
      (async (req: ResolvePendingMatchRequest) =>
        create(ResolvePendingMatchResultSchema, {
          committedSeq: 21n,
          replayed: false,
          pendingMatch: makePendingMatch({
            id: req.pendingMatchId?.value ?? PM_ID_1,
            revision: req.expectedPendingRevision + 1n,
            unresolvedNotional: '0',
            state: PendingMatchState.RESOLVED_MATCHED,
          }),
        })),
  );
  const pollMock = vi.fn(
    overrides.pollEvents ??
      (async (req: PollEventsRequest) =>
        create(PollEventsResultSchema, {
          outcome: {
            case: 'batch',
            value: create(PollEventsBatchSchema, {
              events: [],
              scannedThroughSeq: req.afterSeq + 1n,
              currentHeadSeq: req.afterSeq + 1n,
              hasMore: false,
            }),
          },
        })),
  );

  return {
    client: {
      resolvePendingMatch: resolveMock as DeskBlotterRpcClient['resolvePendingMatch'],
      pollEvents: pollMock as DeskBlotterRpcClient['pollEvents'],
    },
    resolveMock,
    pollMock,
  };
}

afterEach(() => {
  cleanup();
});

describe('PendingMatchPanel — 5-action parity, CAS, four-key, mutual exclusion & overlap rules', () => {
  it('enforces exact wire payload fields and CAS bindings for MATCH_FULL, filtering out wrong-four-key/HOLD earmarks', async () => {
    const user = userEvent.setup();
    const pm = makePendingMatch({
      id: PM_ID_1,
      revision: 4n,
      unresolvedNotional: '10000000',
      unresolvedSignedCash: '-10123456.78',
    });
    const fullEm = makeEarmark({
      id: EM_ID_FULL,
      revision: 9n,
      unreconciledNotional: '15000000',
    });
    const wrongSide = makeEarmark({
      id: EM_ID_WRONG_SIDE,
      side: TradeSide.SELL,
      unreconciledNotional: '15000000',
    });
    const wrongDate = makeEarmark({
      id: EM_ID_WRONG_DATE,
      settlementDate: '2026-06-16',
      unreconciledNotional: '15000000',
    });
    const holdEm = makeEarmark({
      id: EM_ID_HOLD,
      state: EarmarkState.HOLD,
      unreconciledNotional: '15000000',
    });

    const snap = makeSnapshot({
      seq: 20n,
      positionVersion: 7n,
      pendingMatches: [pm],
      activeEarmarks: [fullEm, wrongSide, wrongDate, holdEm],
    });
    const { store } = setupStoreWithSnapshot(snap);
    const { client, resolveMock } = makeMockClient();

    render(<PendingMatchPanel store={store} client={client} />);

    // Only matching four-key COMMITTED earmark is selectable.
    const select = screen.getByLabelText(/目标 COMMITTED 占仓/);
    expect(within(select).getAllByRole('option')).toHaveLength(1);
    expect(within(select).getByRole('option', { name: new RegExp(EM_ID_FULL) })).toBeTruthy();

    // Pre-submit preview shows deterministic notional and unknown server cash impact.
    expect(screen.getByTestId('preview-offset-notional').textContent).toBe('10,000,000');
    expect(screen.getByTestId('preview-pending-remaining').textContent).toBe('0');
    expect(screen.getByTestId('preview-earmark-remaining').textContent).toBe('5,000,000');
    expect(screen.getByTestId('preview-known-pending-cash').textContent).toBe('-10,123,456.78');
    expect(screen.getByTestId('preview-cash-impact').textContent).toBe(
      UNKNOWN_SERVER_CASH_IMPACT_LABEL,
    );

    await user.click(screen.getByTestId('submit-resolve-pending-match'));

    await waitFor(() => expect(resolveMock).toHaveBeenCalledTimes(1));
    const sentReq = resolveMock.mock.calls[0]?.[0] as ResolvePendingMatchRequest;
    expect(sentReq.action).toBe(PendingMatchResolutionAction.MATCH_FULL);
    expect(sentReq.accountId).toBe('acct-A');
    expect(sentReq.pendingMatchId?.value).toBe(PM_ID_1);
    expect(sentReq.expectedPendingRevision).toBe(4n);
    expect(sentReq.expectedPositionVersion).toBe(7n);
    expect(sentReq.targetEarmarkId?.value).toBe(EM_ID_FULL);
    expect(sentReq.expectedEarmarkRevision).toBe(9n);
    expect(sentReq.idempotencyKey.length).toBeGreaterThan(0);
    // Forbidden fields MUST be omitted (undefined) on the wire!
    expect(sentReq.offsetNotional).toBeUndefined();
    expect(sentReq.baselineDelta).toBeUndefined();
  });

  it('validates SPLIT_OFFSET residual bounds, omits baseline_delta, and NEVER fabricates post-split cash client-side', async () => {
    const user = userEvent.setup();
    const pm = makePendingMatch({
      id: PM_ID_1,
      revision: 4n,
      unresolvedNotional: '10000000',
      unresolvedSignedCash: '-10123456.78',
    });
    const smallEm = makeEarmark({
      id: EM_ID_SMALL,
      revision: 5n,
      unreconciledNotional: '6000000',
    });
    const snap = makeSnapshot({
      seq: 20n,
      positionVersion: 8n,
      pendingMatches: [pm],
      activeEarmarks: [smallEm],
    });
    const { store } = setupStoreWithSnapshot(snap);
    const { client, resolveMock } = makeMockClient();

    render(<PendingMatchPanel store={store} client={client} />);

    // MATCH_FULL must be disabled because smallEm remaining (6M) < pm unresolved (10M).
    const matchFullRadio = screen.getByLabelText(/全额匹配 \(MATCH_FULL\)/) as HTMLInputElement;
    expect(matchFullRadio.disabled).toBe(true);

    // SPLIT_OFFSET is enabled and auto-selected.
    const splitRadio = screen.getByLabelText(/拆分冲抵 \(SPLIT_OFFSET\)/) as HTMLInputElement;
    expect(splitRadio.disabled).toBe(false);
    expect(splitRadio.checked).toBe(true);

    const offsetInput = screen.getByLabelText(/拆分冲抵面额/);

    // Exceeds target earmark remaining (7M > 6M) -> blocked client-side with 0 RPC calls.
    await user.type(offsetInput, '7000000');
    await user.click(screen.getByTestId('submit-resolve-pending-match'));
    expect(screen.getByTestId('pending-match-validation-errors').textContent).toContain(
      '超过目标占仓剩余未核销面额',
    );
    expect(resolveMock).not.toHaveBeenCalled();

    // Valid split offset: 3,000,000 -> deterministic notional residuals + explicit unknown cash label.
    await user.clear(offsetInput);
    await user.type(offsetInput, '3000000');

    expect(screen.getByTestId('preview-offset-notional').textContent).toBe('3,000,000');
    expect(screen.getByTestId('preview-pending-remaining').textContent).toBe('7,000,000');
    expect(screen.getByTestId('preview-earmark-remaining').textContent).toBe('3,000,000');
    expect(screen.getByTestId('preview-known-pending-cash').textContent).toBe('-10,123,456.78');
    const previewEl = screen.getByTestId('pre-submit-preview');
    expect(screen.getByTestId('preview-cash-impact').textContent).toBe(
      UNKNOWN_SERVER_CASH_IMPACT_LABEL,
    );
    // Must NEVER fabricate proportional cash (-10123456.78 * 0.3 = -3037037.034).
    expect(previewEl.textContent).not.toContain('3,037,037');
    expect(previewEl.textContent).not.toContain('3037037');

    await user.click(screen.getByTestId('submit-resolve-pending-match'));
    await waitFor(() => expect(resolveMock).toHaveBeenCalledTimes(1));

    const sentReq = resolveMock.mock.calls[0]?.[0] as ResolvePendingMatchRequest;
    expect(sentReq.action).toBe(PendingMatchResolutionAction.SPLIT_OFFSET);
    expect(sentReq.expectedPendingRevision).toBe(4n);
    expect(sentReq.expectedPositionVersion).toBe(8n);
    expect(sentReq.targetEarmarkId?.value).toBe(EM_ID_SMALL);
    expect(sentReq.expectedEarmarkRevision).toBe(5n);
    expect(sentReq.offsetNotional?.coefficient).toBe('3000000');
    expect(sentReq.offsetNotional?.scale).toBe(0);
    expect(sentReq.offsetNotional?.unit?.unitId?.value).toBe(ULID_CCY);
    expect(sentReq.baselineDelta).toBeUndefined();
  });

  it('enforces ACCEPT_ORPHAN and RELEASE_REJECT forbidden fields and mandatory reason on RELEASE_REJECT', async () => {
    const user = userEvent.setup();
    const pm = makePendingMatch({
      id: PM_ID_1,
      revision: 2n,
      reason: PendingMatchReason.ORPHAN_NO_COMMITMENT,
      unresolvedNotional: '5000000',
      unresolvedSignedCash: null,
    });
    const snap = makeSnapshot({
      seq: 20n,
      positionVersion: 3n,
      pendingMatches: [pm],
      activeEarmarks: [],
    });
    const { store } = setupStoreWithSnapshot(snap);
    const { client, resolveMock } = makeMockClient();

    render(<PendingMatchPanel store={store} client={client} />);

    // Known pending cash missing -> shows "未知".
    expect(screen.getByTestId('preview-known-pending-cash').textContent).toBe('未知');

    // Submit ACCEPT_ORPHAN -> omits targetEarmarkId, expectedEarmarkRevision, offsetNotional, baselineDelta.
    const orphanRadio = screen.getByLabelText(/接受孤儿成交 \(ACCEPT_ORPHAN\)/);
    await user.click(orphanRadio);
    await user.click(screen.getByTestId('submit-resolve-pending-match'));
    await waitFor(() => expect(resolveMock).toHaveBeenCalledTimes(1));

    const orphanReq = resolveMock.mock.calls[0]?.[0] as ResolvePendingMatchRequest;
    expect(orphanReq.action).toBe(PendingMatchResolutionAction.ACCEPT_ORPHAN);
    expect(orphanReq.targetEarmarkId).toBeUndefined();
    expect(orphanReq.expectedEarmarkRevision).toBeUndefined();
    expect(orphanReq.offsetNotional).toBeUndefined();
    expect(orphanReq.baselineDelta).toBeUndefined();

    // Switch to RELEASE_REJECT -> blank reason MUST be rejected with 0 extra RPC calls.
    await user.click(screen.getByLabelText(/释放\/拒绝 \(RELEASE_REJECT\)/));
    expect(screen.getByTestId('preview-cash-impact').textContent).toContain('无分摊');
    await user.click(screen.getByTestId('submit-resolve-pending-match'));
    expect(screen.getByTestId('pending-match-validation-errors').textContent).toContain(
      '必须填写非空原因',
    );
    expect(resolveMock).toHaveBeenCalledTimes(1);

    // Fill reason and submit RELEASE_REJECT -> succeeds with forbidden fields omitted and zero-allocation UI state.
    await user.type(screen.getByLabelText(/释放\/拒绝原因/), '重复报送，人工拒绝');
    resolveMock.mockResolvedValueOnce(
      create(ResolvePendingMatchResultSchema, {
        committedSeq: 22n,
        replayed: false,
        pendingMatch: makePendingMatch({
          id: PM_ID_1,
          revision: 3n,
          unresolvedNotional: '0',
          state: PendingMatchState.RELEASED,
        }),
      }),
    );
    await user.click(screen.getByTestId('submit-resolve-pending-match'));
    await waitFor(() => expect(resolveMock).toHaveBeenCalledTimes(2));

    const releaseReq = resolveMock.mock.calls[1]?.[0] as ResolvePendingMatchRequest;
    expect(releaseReq.action).toBe(PendingMatchResolutionAction.RELEASE_REJECT);
    expect(releaseReq.reason).toBe('重复报送，人工拒绝');
    expect(releaseReq.targetEarmarkId).toBeUndefined();
    expect(releaseReq.expectedEarmarkRevision).toBeUndefined();
    expect(releaseReq.offsetNotional).toBeUndefined();
    expect(releaseReq.baselineDelta).toBeUndefined();

    const releaseCard = screen.getByTestId(`submission-22-${PM_ID_1}`);
    expect(releaseCard.getAttribute('data-attribution-state')).toBe('no_allocations');
    expect(within(releaseCard).getByTestId(`submission-22-${PM_ID_1}-status`).textContent).toBe(
      '本动作无分摊',
    );
  });

  it('allows legal APPLY_BASELINE_DELTA even while an active sourceGap exists, disabling first 4 actions and preserving exact BaseEvidence', async () => {
    const user = userEvent.setup();
    const originalEv = makeEvidence({
      sourceId: 'feed-1',
      externalTradeId: 'ext-base-100',
      instrumentCode: 'BOND-10Y',
      side: TradeSide.BUY,
      settlementBucket: SettlementBucket.T_PLUS_0,
      oldNotional: '10000000',
      oldSignedCash: '-10050000.25',
    });
    const pm = makePendingMatch({
      id: PM_ID_1,
      revision: 1n,
      sourceId: 'feed-1',
      externalTradeId: 'ext-base-100',
      sourceRevision: 2n,
      settlementBucket: SettlementBucket.T_PLUS_0,
      reason: PendingMatchReason.BASELINE_REVISION_MANUAL,
      unresolvedNotional: '0',
      unresolvedSignedCash: '10050000.25',
      evidence: originalEv,
    });
    const gapFeed = makeFeedStatus({
      sourceId: 'feed-1',
      missingRanges: [[10n, 12n]],
    });
    const snap = makeSnapshot({
      seq: 20n,
      positionVersion: 11n,
      pendingMatches: [pm],
      activeEarmarks: [makeEarmark({ id: EM_ID_FULL })],
      feedStatuses: [gapFeed],
    });
    const { store } = setupStoreWithSnapshot(snap);
    const { client, resolveMock } = makeMockClient();

    render(<PendingMatchPanel store={store} client={client} />);

    // Source gap banner explicitly explains overlap rule.
    expect(screen.getByTestId('source-gap-banner').textContent).toContain(
      'APPLY_BASELINE_DELTA 仍可合法提交',
    );

    // First 4 actions MUST be disabled while APPLY_BASELINE_DELTA is enabled!
    expect((screen.getByLabelText(/全额匹配 \(MATCH_FULL\)/) as HTMLInputElement).disabled).toBe(
      true,
    );
    expect((screen.getByLabelText(/拆分冲抵 \(SPLIT_OFFSET\)/) as HTMLInputElement).disabled).toBe(
      true,
    );
    expect(
      (screen.getByLabelText(/接受孤儿成交 \(ACCEPT_ORPHAN\)/) as HTMLInputElement).disabled,
    ).toBe(true);
    expect((screen.getByLabelText(/释放\/拒绝 \(RELEASE_REJECT\)/) as HTMLInputElement).disabled).toBe(
      true,
    );
    const baselineRadio = screen.getByLabelText(
      /应用基线差额 \(APPLY_BASELINE_DELTA\)/,
    ) as HTMLInputElement;
    expect(baselineRadio.disabled).toBe(false);
    expect(baselineRadio.checked).toBe(true);

    // Fill negative delta (e.g., baseline cancel reversal) and non-empty reason.
    await user.type(screen.getByLabelText(/基线面额差额/), '-10000000');
    await user.type(screen.getByLabelText(/基线现金差额/), '10050000.25');
    await user.type(screen.getByLabelText(/基线修订原因/), '盘前基准单撤销差额确认');

    await user.click(screen.getByTestId('submit-resolve-pending-match'));
    await waitFor(() => expect(resolveMock).toHaveBeenCalledTimes(1));

    const sentReq = resolveMock.mock.calls[0]?.[0] as ResolvePendingMatchRequest;
    expect(sentReq.action).toBe(PendingMatchResolutionAction.APPLY_BASELINE_DELTA);
    expect(sentReq.expectedPendingRevision).toBe(1n);
    expect(sentReq.expectedPositionVersion).toBe(11n);
    // Forbidden target/offset fields MUST be omitted!
    expect(sentReq.targetEarmarkId).toBeUndefined();
    expect(sentReq.expectedEarmarkRevision).toBeUndefined();
    expect(sentReq.offsetNotional).toBeUndefined();
    // Exact original BaseEvidence preserved verbatim.
    expect(sentReq.baselineDelta?.settlementBucket).toBe(SettlementBucket.T_PLUS_0);
    expect(sentReq.baselineDelta?.deltaNotional?.coefficient).toBe('-10000000');
    expect(sentReq.baselineDelta?.deltaCash?.coefficient).toBe('1005000025');
    expect(sentReq.baselineDelta?.deltaCash?.scale).toBe(2);
    expect(sentReq.baselineDelta?.reason).toBe('盘前基准单撤销差额确认');
    expect(sentReq.baselineDelta?.evidence).toEqual(originalEv);
  });

  it('blocks APPLY_BASELINE_DELTA on cross-bucket BaseEvidence anomaly and blocks first 4 actions on same-key REVISION_HOLD', () => {
    const crossBucketEv = makeEvidence({
      settlementBucket: SettlementBucket.T_PLUS_1,
    });
    const pmCrossBucket = makePendingMatch({
      id: PM_ID_1,
      settlementBucket: SettlementBucket.T_PLUS_0,
      reason: PendingMatchReason.BASELINE_REVISION_MANUAL,
      evidence: crossBucketEv,
    });
    const snapCross = makeSnapshot({ pendingMatches: [pmCrossBucket] });
    const builtCross = buildResolvePendingMatchRequest({
      snapshot: snapCross,
      pendingMatch: pmCrossBucket,
      writesPaused: false,
      form: {
        action: PendingMatchResolutionAction.APPLY_BASELINE_DELTA,
        targetEarmarkId: '',
        offsetNotionalInput: '',
        reasonInput: '',
        baselineDeltaNotionalInput: '-100',
        baselineDeltaCashInput: '100',
        baselineDeltaReasonInput: 'cross bucket attempt',
      },
    });
    expect(builtCross.ok).toBe(false);
    if (!builtCross.ok) {
      expect(builtCross.issues[0]?.message).toContain('跨券/跨方向/跨结算桶');
    }

    // Same four-key active REVISION_HOLD blocks another OPEN pending match on the same four-key.
    const pmHold = makePendingMatch({
      id: PM_ID_2,
      reason: PendingMatchReason.REVISION_HOLD,
      unresolvedNotional: '0',
      unresolvedSignedCash: null,
    });
    const pmNormal = makePendingMatch({
      id: PM_ID_1,
      reason: PendingMatchReason.EXISTING_PENDING_ON_KEY,
      unresolvedNotional: '5000000',
    });
    const snapHold = makeSnapshot({
      pendingMatches: [pmNormal, pmHold],
      activeEarmarks: [makeEarmark({ id: EM_ID_FULL })],
    });
    const builtBlocked = buildResolvePendingMatchRequest({
      snapshot: snapHold,
      pendingMatch: pmNormal,
      writesPaused: false,
      form: {
        action: PendingMatchResolutionAction.ACCEPT_ORPHAN,
        targetEarmarkId: '',
        offsetNotionalInput: '',
        reasonInput: '',
        baselineDeltaNotionalInput: '',
        baselineDeltaCashInput: '',
        baselineDeltaReasonInput: '',
      },
    });
    expect(builtBlocked.ok).toBe(false);
    if (!builtBlocked.ok) {
      expect(builtBlocked.issues[0]?.message).toContain('REVISION_HOLD');
    }
  });

  it('reuses exact sticky idempotency_key across transport retry and rotates key when parameters change', async () => {
    const user = userEvent.setup();
    const pm = makePendingMatch({
      id: PM_ID_1,
      revision: 4n,
      unresolvedNotional: '10000000',
    });
    const snap = makeSnapshot({
      seq: 20n,
      positionVersion: 7n,
      pendingMatches: [pm],
      activeEarmarks: [makeEarmark({ id: EM_ID_FULL })],
    });
    const { store } = setupStoreWithSnapshot(snap);
    const writeIntents = new WriteIntentManager();
    const { client, resolveMock } = makeMockClient();

    resolveMock.mockRejectedValueOnce(
      new BlotterClientError({
        kind: 'transport',
        message: 'network temporarily unavailable',
        retryable: true,
      }),
    );

    render(<PendingMatchPanel store={store} client={client} writeIntents={writeIntents} />);

    await user.click(screen.getByLabelText(/接受孤儿成交 \(ACCEPT_ORPHAN\)/));
    await user.type(screen.getByLabelText(/核销原因/), 'orphan-1');
    await user.click(screen.getByTestId('submit-resolve-pending-match'));

    await waitFor(() =>
      expect(screen.getByTestId('pending-match-submit-error').textContent).toContain('已保留原幂等键'),
    );
    const firstKey = (resolveMock.mock.calls[0]?.[0] as ResolvePendingMatchRequest).idempotencyKey;

    // Retry with identical payload -> MUST reuse EXACT same idempotencyKey.
    resolveMock.mockRejectedValueOnce(
      new BlotterClientError({
        kind: 'transport',
        message: 'still flaky',
        retryable: true,
      }),
    );
    await user.click(screen.getByTestId('submit-resolve-pending-match'));
    await waitFor(() => expect(resolveMock).toHaveBeenCalledTimes(2));
    const retryKey = (resolveMock.mock.calls[1]?.[0] as ResolvePendingMatchRequest).idempotencyKey;
    expect(retryKey).toBe(firstKey);

    // Modify reason parameter -> MUST allocate a fresh idempotencyKey.
    await user.type(screen.getByLabelText(/核销原因/), '-edited');
    await user.click(screen.getByTestId('submit-resolve-pending-match'));
    await waitFor(() => expect(resolveMock).toHaveBeenCalledTimes(3));
    const editedKey = (resolveMock.mock.calls[2]?.[0] as ResolvePendingMatchRequest).idempotencyKey;
    expect(editedKey).not.toBe(firstKey);
  });
});

describe('PendingMatchPanel — Post-submit transaction-attributed cash breakdown (Decision #3 Scheme A)', () => {
  it('handles Order B (RPC receipt arrives BEFORE stream event) without advancing cursorSeq, then correlates exact allocations on stream arrival', async () => {
    const user = userEvent.setup();
    const pm = makePendingMatch({
      id: PM_ID_1,
      revision: 4n,
      unresolvedNotional: '10000000',
      unresolvedSignedCash: '-10123456.78',
    });
    const em = makeEarmark({ id: EM_ID_FULL, revision: 3n, unreconciledNotional: '10000000' });
    const snap20 = makeSnapshot({ seq: 20n, pendingMatches: [pm], activeEarmarks: [em] });
    const { store, commitEnvelope } = setupStoreWithSnapshot(snap20);
    const { client, pollMock } = makeMockClient({
      resolvePendingMatch: async () =>
        create(ResolvePendingMatchResultSchema, {
          committedSeq: 21n,
          replayed: false,
          pendingMatch: makePendingMatch({
            id: PM_ID_1,
            revision: 5n,
            unresolvedNotional: '7000000',
          }),
        }),
    });

    render(<PendingMatchPanel store={store} client={client} />);

    await user.click(screen.getByLabelText(/拆分冲抵 \(SPLIT_OFFSET\)/));
    await user.type(screen.getByLabelText(/拆分冲抵面额/), '3000000');
    await user.click(screen.getByTestId('submit-resolve-pending-match'));

    // Receipt arrived BEFORE stream event -> pending_correlation, cursorSeq STILL 20n!
    const card = await screen.findByTestId(`submission-21-${PM_ID_1}`);
    expect(card.getAttribute('data-attribution-state')).toBe('pending_correlation');
    expect(within(card).getByTestId(`submission-21-${PM_ID_1}-status`).textContent).toBe(
      '等待本次事务明细',
    );
    expect(store.getState().cursorSeq).toBe(20n);
    expect(pollMock).not.toHaveBeenCalled();

    // Stream delivers seq=21n with exact server-computed allocation cash.
    const alloc = makeAllocationView({
      allocationId: ALLOC_ID_1,
      pendingMatchId: PM_ID_1,
      earmarkId: EM_ID_FULL,
      allocatedNotional: '3000000',
      allocatedFillCash: '-3037037.04',
      releasedEarmarkCash: '-3036900.00',
    });
    const snap21 = makeSnapshot({
      seq: 21n,
      pendingMatches: [makePendingMatch({ id: PM_ID_1, revision: 5n, unresolvedNotional: '7000000' })],
      activeEarmarks: [makeEarmark({ id: EM_ID_FULL, revision: 4n, unreconciledNotional: '7000000' })],
    });
    commitEnvelope(makeEnvelope(21n, [{ accountId: 'acct-A', snapshot: snap21, allocations: [alloc] }]));

    await waitFor(() => {
      expect(screen.getByTestId(`submission-21-${PM_ID_1}`).getAttribute('data-attribution-state')).toBe(
        'allocated',
      );
    });
    expect(screen.getByTestId(`allocation-fill-cash-${ALLOC_ID_1}`).textContent).toContain(
      '-3,037,037.04',
    );
    expect(screen.getByTestId(`allocation-released-cash-${ALLOC_ID_1}`).textContent).toContain(
      '-3,036,900',
    );
    expect(pollMock).not.toHaveBeenCalled();
  });

  it('handles Order A (stream event arrives BEFORE RPC receipt returns) and idempotent replay (replayed=true) with zero duplicate rows', async () => {
    const user = userEvent.setup();
    const pm = makePendingMatch({ id: PM_ID_1, revision: 4n, unresolvedNotional: '10000000' });
    const em = makeEarmark({ id: EM_ID_FULL, revision: 3n, unreconciledNotional: '10000000' });
    const snap20 = makeSnapshot({ seq: 20n, pendingMatches: [pm], activeEarmarks: [em] });
    const { store, commitEnvelope } = setupStoreWithSnapshot(snap20);

    let resolveRpc!: (res: ResolvePendingMatchResult) => void;
    const rpcPromise = new Promise<ResolvePendingMatchResult>((res) => {
      resolveRpc = res;
    });
    const { client, pollMock } = makeMockClient({
      resolvePendingMatch: () => rpcPromise,
    });

    render(<PendingMatchPanel store={store} client={client} />);

    await user.click(screen.getByTestId('submit-resolve-pending-match'));

    // Stream event seq=21 arrives BEFORE RPC promise resolves!
    const alloc = makeAllocationView({
      allocationId: ALLOC_ID_1,
      pendingMatchId: PM_ID_1,
      earmarkId: EM_ID_FULL,
      allocatedNotional: '10000000',
      allocatedFillCash: '-10123456.78',
      releasedEarmarkCash: '-10120000',
    });
    const snap21 = makeSnapshot({ seq: 21n, pendingMatches: [], activeEarmarks: [] });
    commitEnvelope(makeEnvelope(21n, [{ accountId: 'acct-A', snapshot: snap21, allocations: [alloc] }]));

    // Now RPC returns with replayed=true for the same committedSeq=21n.
    resolveRpc(
      create(ResolvePendingMatchResultSchema, {
        committedSeq: 21n,
        replayed: true,
        pendingMatch: makePendingMatch({
          id: PM_ID_1,
          revision: 5n,
          unresolvedNotional: '0',
          state: PendingMatchState.RESOLVED_MATCHED,
        }),
      }),
    );

    const card = await screen.findByTestId(`submission-21-${PM_ID_1}`);
    expect(card.getAttribute('data-attribution-state')).toBe('allocated');
    expect(within(card).getByTestId(`submission-21-${PM_ID_1}-replayed-badge`).textContent).toContain(
      'replayed=true',
    );
    expect(within(card).getAllByTestId(/^allocation-row-/)).toHaveLength(1);
    expect(screen.getByTestId(`allocation-fill-cash-${ALLOC_ID_1}`).textContent).toContain(
      '-10,123,456.78',
    );
    expect(pollMock).not.toHaveBeenCalled();
  });

  it('strictly isolates subsequent unrelated account balance mutations and mismatched pending_match_id / account_id allocations', async () => {
    const user = userEvent.setup();
    const pm = makePendingMatch({ id: PM_ID_1, revision: 4n, unresolvedNotional: '10000000' });
    const em = makeEarmark({ id: EM_ID_FULL, revision: 3n, unreconciledNotional: '10000000' });
    const snapA20 = makeSnapshot({
      accountId: 'acct-A',
      seq: 20n,
      pendingMatches: [pm],
      activeEarmarks: [em],
      fundingSummary: makeFundingSummary({ projectedCashT0: '50000000' }),
    });
    const snapB20 = makeSnapshot({
      accountId: 'acct-B',
      seq: 20n,
      pendingMatches: [],
      activeEarmarks: [],
    });
    const { store, commitEnvelope } = setupStoreWithSnapshot(snapA20, [snapB20]);
    const { client } = makeMockClient({
      resolvePendingMatch: async () =>
        create(ResolvePendingMatchResultSchema, {
          committedSeq: 21n,
          replayed: false,
          pendingMatch: makePendingMatch({
            id: PM_ID_1,
            revision: 5n,
            unresolvedNotional: '0',
            state: PendingMatchState.RESOLVED_MATCHED,
          }),
        }),
    });

    render(<PendingMatchPanel store={store} client={client} />);
    await user.click(screen.getByTestId('submit-resolve-pending-match'));
    await screen.findByTestId(`submission-21-${PM_ID_1}`);

    // Envelope seq=21 contains:
    //  - Exact match for (acct-A, seq=21, PM_ID_1): -10,123,456.78
    //  - Wrong pending_match_id on acct-A at seq=21 (PM_ID_WRONG): -77,777,777
    //  - Wrong account (acct-B) at seq=21 with same PM_ID_1: -88,888,888
    const exactAlloc = makeAllocationView({
      allocationId: ALLOC_ID_1,
      pendingMatchId: PM_ID_1,
      allocatedNotional: '10000000',
      allocatedFillCash: '-10123456.78',
    });
    const wrongPmAlloc = makeAllocationView({
      allocationId: ALLOC_ID_2,
      pendingMatchId: PM_ID_WRONG,
      allocatedNotional: '77000000',
      allocatedFillCash: '-77777777',
    });
    const wrongAcctAlloc = makeAllocationView({
      allocationId: ALLOC_ID_OTHER,
      pendingMatchId: PM_ID_1,
      allocatedNotional: '88000000',
      allocatedFillCash: '-88888888',
    });

    commitEnvelope(
      makeEnvelope(21n, [
        {
          accountId: 'acct-A',
          snapshot: makeSnapshot({
            accountId: 'acct-A',
            seq: 21n,
            fundingSummary: makeFundingSummary({ projectedCashT0: '39876543.22' }),
          }),
          allocations: [exactAlloc, wrongPmAlloc],
        },
        {
          accountId: 'acct-B',
          snapshot: makeSnapshot({ accountId: 'acct-B', seq: 21n }),
          allocations: [wrongAcctAlloc],
        },
      ]),
    );

    // Subsequent unrelated account balance mutation at seq=22 (-50,000,000 cash swing + another allocation).
    commitEnvelope(
      makeEnvelope(22n, [
        {
          accountId: 'acct-A',
          snapshot: makeSnapshot({
            accountId: 'acct-A',
            seq: 22n,
            fundingSummary: makeFundingSummary({ projectedCashT0: '-10123456.78' }),
          }),
          allocations: [
            makeAllocationView({
              allocationId: '01JNB0G1ALLOC0000000000022',
              pendingMatchId: PM_ID_1,
              allocatedNotional: '50000000',
              allocatedFillCash: '-50000000',
            }),
          ],
        },
      ]),
    );

    const card = await screen.findByTestId(`submission-21-${PM_ID_1}`);
    await waitFor(() => expect(card.getAttribute('data-attribution-state')).toBe('allocated'));

    // Strictly ONLY the exact (seq=21, acct-A, PM_ID_1) allocation appears!
    expect(within(card).getAllByTestId(/^allocation-row-/)).toHaveLength(1);
    expect(card.textContent).toContain('-10,123,456.78');
    expect(card.textContent).not.toContain('77,777,777');
    expect(card.textContent).not.toContain('88,888,888');
    expect(card.textContent).not.toContain('-50,000,000');

    // Separate account funding snapshot reference displays its own seq=22.
    const fundingRef = screen.getByTestId('account-funding-snapshot-ref');
    expect(fundingRef.getAttribute('data-snapshot-seq')).toBe('22');
    expect(fundingRef.textContent).toContain('快照序号 seq=22');
  });

  it('distinguishes unmatched wrong pending_match_id on complete history from pruned history without leaking mismatched allocations', async () => {
    const user = userEvent.setup();
    const pm = makePendingMatch({ id: PM_ID_1, revision: 4n, unresolvedNotional: '5000000' });
    const snap25 = makeSnapshot({ seq: 25n, pendingMatches: [pm] });
    const { store } = setupStoreWithSnapshot(snap25);

    // Replay returns older committedSeq=21 (<= cursorSeq 25), auto-triggering read-only PollEvents(afterSeq=20).
    // Server returns intact history at seq=21, but its allocation belongs to PM_ID_WRONG!
    const { client, pollMock } = makeMockClient({
      resolvePendingMatch: async () =>
        create(ResolvePendingMatchResultSchema, {
          committedSeq: 21n,
          replayed: true,
        }),
      pollEvents: async () =>
        create(PollEventsResultSchema, {
          outcome: {
            case: 'batch',
            value: create(PollEventsBatchSchema, {
              events: [
                makeEnvelope(21n, [
                  {
                    accountId: 'acct-A',
                    snapshot: makeSnapshot({ accountId: 'acct-A', seq: 21n }),
                    allocations: [
                      makeAllocationView({
                        allocationId: ALLOC_ID_2,
                        pendingMatchId: PM_ID_WRONG,
                        allocatedFillCash: '-9999999',
                      }),
                    ],
                  },
                ]),
              ],
              scannedThroughSeq: 21n,
              currentHeadSeq: 25n,
              hasMore: false,
            }),
          },
        }),
    });

    render(<PendingMatchPanel store={store} client={client} />);
    await user.click(screen.getByLabelText(/接受孤儿成交 \(ACCEPT_ORPHAN\)/));
    await user.click(screen.getByTestId('submit-resolve-pending-match'));

    const card = await screen.findByTestId(`submission-21-${PM_ID_1}`);
    await waitFor(() =>
      expect(card.getAttribute('data-attribution-state')).toBe('unmatched_empty_history'),
    );
    expect(pollMock).toHaveBeenCalledWith(
      { afterSeq: 20n, maxEvents: 1, accountId: 'acct-A' },
      expect.anything(),
    );
    // Mismatched cash NEVER leaked, NOT falsely labeled pruned, and cursorSeq UNTOUCHED!
    expect(card.textContent).not.toContain('9,999,999');
    expect(card.textContent).not.toContain('历史分摊已裁剪');
    expect(store.getState().cursorSeq).toBe(25n);
  });

  it('distinguishes transient disconnect (retryable) from truly pruned history (pruned_history_unrecoverable), with ZERO workspace cursorSeq mutation', async () => {
    const user = userEvent.setup();
    const pm1 = makePendingMatch({ id: PM_ID_1, revision: 4n, unresolvedNotional: '5000000' });
    const pm2 = makePendingMatch({ id: PM_ID_2, revision: 2n, unresolvedNotional: '3000000' });
    // Workspace cursorSeq is already 30n; replayed/missed writes at seq=12n and seq=15n trigger read-only lookup.
    const snap30 = makeSnapshot({ seq: 30n, pendingMatches: [pm1, pm2] });
    const { store } = setupStoreWithSnapshot(snap30);

    const { client, resolveMock, pollMock } = makeMockClient();

    // Case 1: PM_ID_1 at committedSeq=15n hits transient disconnect first, then succeeds on manual retry!
    resolveMock.mockResolvedValueOnce(
      create(ResolvePendingMatchResultSchema, {
        committedSeq: 15n,
        replayed: true,
      }),
    );
    pollMock.mockRejectedValueOnce(
      new BlotterClientError({
        kind: 'transport',
        message: 'gateway 503 temporary disconnect',
        retryable: true,
      }),
    );

    render(<PendingMatchPanel store={store} client={client} />);
    await user.click(screen.getByLabelText(/接受孤儿成交 \(ACCEPT_ORPHAN\)/));
    await user.click(screen.getByTestId('submit-resolve-pending-match'));

    const card15 = await screen.findByTestId(`submission-15-${PM_ID_1}`);
    await waitFor(() =>
      expect(card15.getAttribute('data-attribution-state')).toBe('transient_disconnected'),
    );
    expect(card15.textContent).toContain('暂时断线');
    expect(card15.textContent).not.toContain('历史分摊已裁剪，明细不可恢复');
    expect(store.getState().cursorSeq).toBe(30n);

    // Retry read-only lookup -> recovers exact allocation via PollEvents(afterSeq=14) WITHOUT touching cursorSeq!
    const historicalAlloc = makeAllocationView({
      allocationId: ALLOC_ID_1,
      pendingMatchId: PM_ID_1,
      allocatedNotional: '5000000',
      allocatedFillCash: '-5025000.50',
    });
    pollMock.mockResolvedValueOnce(
      create(PollEventsResultSchema, {
        outcome: {
          case: 'batch',
          value: create(PollEventsBatchSchema, {
            events: [
              makeEnvelope(15n, [
                {
                  accountId: 'acct-A',
                  snapshot: makeSnapshot({ accountId: 'acct-A', seq: 15n }),
                  allocations: [historicalAlloc],
                },
              ]),
            ],
            scannedThroughSeq: 15n,
            currentHeadSeq: 30n,
            hasMore: false,
          }),
        },
      }),
    );

    await user.click(within(card15).getByRole('button', { name: '重试只读查询' }));
    await waitFor(() => expect(card15.getAttribute('data-attribution-state')).toBe('allocated'));
    expect(card15.textContent).toContain('-5,025,000.5');
    expect(store.getState().cursorSeq).toBe(30n);

    // Case 2: PM_ID_2 at committedSeq=12n hits genuine SnapshotFallbackRequired (truly pruned)!
    await user.click(screen.getByTestId(`pending-match-item-${PM_ID_2}`));
    resolveMock.mockResolvedValueOnce(
      create(ResolvePendingMatchResultSchema, {
        committedSeq: 12n,
        replayed: true,
      }),
    );
    pollMock.mockResolvedValueOnce(
      create(PollEventsResultSchema, {
        outcome: {
          case: 'snapshotFallback',
          value: create(SnapshotFallbackRequiredSchema, {
            requestedAfterSeq: 11n,
            oldestRetainedSeq: 28n,
            currentHeadSeq: 30n,
            reason: 'event log pruned past retention window',
          }),
        },
      }),
    );

    await user.click(screen.getByLabelText(/接受孤儿成交 \(ACCEPT_ORPHAN\)/));
    await user.click(screen.getByTestId('submit-resolve-pending-match'));

    const card12 = await screen.findByTestId(`submission-12-${PM_ID_2}`);
    await waitFor(() =>
      expect(card12.getAttribute('data-attribution-state')).toBe('pruned_history_unrecoverable'),
    );
    expect(within(card12).getByTestId(`submission-12-${PM_ID_2}-status`).textContent).toBe(
      '历史分摊已裁剪，明细不可恢复',
    );
    expect(card12.textContent).toContain('oldest_retained_seq=28');
    // CRITICAL INVARIANT: read-only point-in-time lookup NEVER modified workspace cursorSeq!
    expect(store.getState().cursorSeq).toBe(30n);
  });

  it('clears session-scoped submission attributions immediately on session switch / logout', async () => {
    const user = userEvent.setup();
    const pm = makePendingMatch({ id: PM_ID_1, revision: 4n, unresolvedNotional: '5000000' });
    const snap = makeSnapshot({ seq: 20n, pendingMatches: [pm] });
    const { store } = setupStoreWithSnapshot(snap);
    const { client } = makeMockClient({
      resolvePendingMatch: async () =>
        create(ResolvePendingMatchResultSchema, {
          committedSeq: 21n,
          replayed: false,
        }),
    });

    render(<PendingMatchPanel store={store} client={client} />);
    await user.click(screen.getByLabelText(/接受孤儿成交 \(ACCEPT_ORPHAN\)/));
    await user.click(screen.getByTestId('submit-resolve-pending-match'));
    await screen.findByTestId(`submission-21-${PM_ID_1}`);

    // Switch session (increments sessionGeneration & clears snapshots) -> old submissions immediately vanish!
    store.getState().beginSession({ headerMode: 'authorization', actorLabel: 'trader-2' });
    await waitFor(() => {
      expect(screen.queryByTestId(`submission-21-${PM_ID_1}`)).toBeNull();
    });
  });

  it('surfaces business ErrorDetail cleanly without retaining a stale write intent', async () => {
    const user = userEvent.setup();
    const pm = makePendingMatch({ id: PM_ID_1, revision: 4n, unresolvedNotional: '5000000' });
    const snap = makeSnapshot({ seq: 20n, pendingMatches: [pm] });
    const { store } = setupStoreWithSnapshot(snap);
    const writeIntents = new WriteIntentManager();
    const { client, resolveMock } = makeMockClient({
      resolvePendingMatch: async () => {
        throw BlotterClientError.fromErrorDetail(
          create(ErrorDetailSchema, {
            code: ErrorCode.CONCURRENCY_CONFLICT,
            message: 'stale expected_pending_revision',
            retryable: false,
          }),
        );
      },
    });

    render(<PendingMatchPanel store={store} client={client} writeIntents={writeIntents} />);
    await user.click(screen.getByLabelText(/接受孤儿成交 \(ACCEPT_ORPHAN\)/));
    await user.click(screen.getByTestId('submit-resolve-pending-match'));

    await waitFor(() =>
      expect(screen.getByTestId('pending-match-submit-error').textContent).toContain(
        'stale expected_pending_revision',
      ),
    );
    expect(resolveMock).toHaveBeenCalledTimes(1);
    expect(writeIntents.size()).toBe(0);
  });

  it('renders baseline_delta details on post-submit APPLY_BASELINE_DELTA allocation and blocks missing BaseEvidence', async () => {
    const user = userEvent.setup();
    const originalEv = makeEvidence({
      oldNotional: '10000000',
      oldSignedCash: '-10050000.25',
    });
    const pmValid = makePendingMatch({
      id: PM_ID_1,
      revision: 1n,
      reason: PendingMatchReason.BASELINE_REVISION_MANUAL,
      unresolvedNotional: '0',
      unresolvedSignedCash: '10050000.25',
      evidence: originalEv,
    });
    const pmNoEvidence = makePendingMatch({
      id: PM_ID_2,
      revision: 1n,
      reason: PendingMatchReason.BASELINE_REVISION_MANUAL,
      unresolvedNotional: '0',
      unresolvedSignedCash: null,
      evidence: undefined,
    });
    const snap20 = makeSnapshot({
      seq: 20n,
      pendingMatches: [pmValid, pmNoEvidence],
    });
    const { store, commitEnvelope } = setupStoreWithSnapshot(snap20);
    const { client } = makeMockClient({
      resolvePendingMatch: async () =>
        create(ResolvePendingMatchResultSchema, {
          committedSeq: 21n,
          replayed: false,
          pendingMatch: makePendingMatch({
            id: PM_ID_1,
            revision: 2n,
            unresolvedNotional: '0',
            state: PendingMatchState.RESOLVED_BASELINE_DELTA,
          }),
        }),
    });

    render(<PendingMatchPanel store={store} client={client} />);

    // First verify pmNoEvidence disables APPLY_BASELINE_DELTA with clear diagnostic.
    await user.click(screen.getByTestId(`pending-match-item-${PM_ID_2}`));
    const baselineRadioNoEv = screen.getByLabelText(
      /应用基线差额 \(APPLY_BASELINE_DELTA\)/,
    ) as HTMLInputElement;
    expect(baselineRadioNoEv.disabled).toBe(true);
    expect(
      screen.getByTestId(
        `action-disabled-reason-${String(PendingMatchResolutionAction.APPLY_BASELINE_DELTA)}`,
      ).textContent,
    ).toContain('BaseEvidence');

    // Switch back to pmValid and submit APPLY_BASELINE_DELTA.
    await user.click(screen.getByTestId(`pending-match-item-${PM_ID_1}`));
    await user.type(screen.getByLabelText(/基线面额差额/), '-10000000');
    await user.type(screen.getByLabelText(/基线现金差额/), '10050000.25');
    await user.type(screen.getByLabelText(/基线修订原因/), '基线差额核销');
    await user.click(screen.getByTestId('submit-resolve-pending-match'));

    const card = await screen.findByTestId(`submission-21-${PM_ID_1}`);
    const baselineDelta = create(BaselineDeltaSchema, {
      deltaNotional: dec('-10000000'),
      deltaCash: dec('10050000.25'),
      settlementBucket: SettlementBucket.T_PLUS_0,
      evidence: originalEv,
      reason: '基线差额核销',
    });
    const alloc = makeAllocationView({
      allocationId: ALLOC_ID_1,
      pendingMatchId: PM_ID_1,
      allocatedNotional: '-10000000',
      allocatedFillCash: '10050000.25',
      baselineDelta,
    });
    commitEnvelope(
      makeEnvelope(21n, [
        {
          accountId: 'acct-A',
          snapshot: makeSnapshot({ seq: 21n, pendingMatches: [pmNoEvidence] }),
          allocations: [alloc],
        },
      ]),
    );

    await waitFor(() => expect(card.getAttribute('data-attribution-state')).toBe('allocated'));
    const deltaEl = screen.getByTestId(`allocation-baseline-delta-${ALLOC_ID_1}`);
    expect(deltaEl.textContent).toContain('-10,000,000');
    expect(deltaEl.textContent).toContain('10,050,000.25');
    expect(deltaEl.textContent).toContain('基线差额核销');
  });

  it('disables all 5 actions for normal pending match during active sourceGap and during recoveryPausedWrites', () => {
    const pmNormal = makePendingMatch({
      id: PM_ID_1,
      reason: PendingMatchReason.NOTIONAL_MISMATCH_OR_SPLIT,
      unresolvedNotional: '5000000',
    });
    const snapGap = makeSnapshot({
      seq: 20n,
      pendingMatches: [pmNormal],
      activeEarmarks: [makeEarmark({ id: EM_ID_FULL })],
      feedStatuses: [makeFeedStatus({ sourceId: 'feed-1', missingRanges: [[3n, 5n]] })],
    });
    const { store, sessionGeneration } = setupStoreWithSnapshot(snapGap);
    const { client } = makeMockClient();

    render(<PendingMatchPanel store={store} client={client} />);
    expect((screen.getByTestId('submit-resolve-pending-match') as HTMLButtonElement).disabled).toBe(
      true,
    );
    for (const label of [
      /全额匹配 \(MATCH_FULL\)/,
      /拆分冲抵 \(SPLIT_OFFSET\)/,
      /接受孤儿成交 \(ACCEPT_ORPHAN\)/,
      /释放\/拒绝 \(RELEASE_REJECT\)/,
      /应用基线差额 \(APPLY_BASELINE_DELTA\)/,
    ]) {
      expect((screen.getByLabelText(label) as HTMLInputElement).disabled).toBe(true);
    }

    // Even a valid BASELINE_REVISION_MANUAL is paused when workspace is in recoveryPausedWrites!
    cleanup();
    const pmBaseline = makePendingMatch({
      id: PM_ID_1,
      reason: PendingMatchReason.BASELINE_REVISION_MANUAL,
      unresolvedNotional: '0',
      evidence: makeEvidence(),
    });
    const snapBaseline = makeSnapshot({ seq: 20n, pendingMatches: [pmBaseline] });
    const { store: storeRec, sessionGeneration: genRec } = setupStoreWithSnapshot(snapBaseline);
    storeRec.getState().activateReader(genRec, 'recovery', 'recovering', { pauseWrites: true });
    expect(sessionGeneration).toBeGreaterThan(0);

    render(<PendingMatchPanel store={storeRec} client={client} />);
    expect(
      (screen.getByLabelText(/应用基线差额 \(APPLY_BASELINE_DELTA\)/) as HTMLInputElement).disabled,
    ).toBe(true);
    expect((screen.getByTestId('submit-resolve-pending-match') as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it('rejects zero, negative, non-numeric, and over-pending SPLIT_OFFSET inputs with zero RPC calls', () => {
    const pm = makePendingMatch({ id: PM_ID_1, unresolvedNotional: '4000000' });
    const em = makeEarmark({ id: EM_ID_FULL, unreconciledNotional: '10000000' });
    const snap = makeSnapshot({ pendingMatches: [pm], activeEarmarks: [em] });

    for (const badOffset of ['', '0', '-1000', 'abc', '5000000']) {
      const res = buildResolvePendingMatchRequest({
        snapshot: snap,
        pendingMatch: pm,
        writesPaused: false,
        form: {
          action: PendingMatchResolutionAction.SPLIT_OFFSET,
          targetEarmarkId: EM_ID_FULL,
          offsetNotionalInput: badOffset,
          reasonInput: '',
          baselineDeltaNotionalInput: '',
          baselineDeltaCashInput: '',
          baselineDeltaReasonInput: '',
        },
      });
      expect(res.ok).toBe(false);
    }
  });
});
