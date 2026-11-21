import { create } from '@bufbuild/protobuf';
import { TimestampSchema } from '@bufbuild/protobuf/wkt';
import { describe, expect, it, vi } from 'vitest';
import { TRUSTED_RULEPACK_ANCHOR } from 'virtual:ficant-trusted-rulepack';
import {
  AccountBlotterSnapshotSchema,
  AccountChangeSchema,
  BlotterEventEnvelopeSchema,
  CoverageStatus,
  InstrumentPositionViewSchema,
  OpeningPositionBaseSchema,
  PollEventsBatchSchema,
  PreviewHedgeScope,
  ReconcileAllocationViewSchema,
  RiskAndCoverageSummarySchema,
  SnapshotFallbackRequiredSchema,
  StreamHeartbeatSchema,
  type AccountBlotterSnapshot,
  type BlotterEventEnvelope,
  type ReconcileAllocationView,
} from '../generated/ficant/blotter/v1/model_pb';
import {
  GetAccountSnapshotResultSchema,
  PollEventsResultSchema,
  PreviewCoveredHedgeResultSchema,
  SubscribeEventsResponseSchema,
  type GetAccountSnapshotResult,
  type PollEventsResult,
  type PreviewCoveredHedgeResult,
  type SubscribeEventsResponse,
} from '../generated/ficant/blotter/v1/service_pb';
import { MarketTimeSchema, UlidSchema } from '../generated/ficant/core/v1/common_pb';
import { ErrorCode, ErrorDetailSchema } from '../generated/ficant/core/v1/error_pb';
import {
  AnalysisUnitsSchema,
  BondTaxBasis,
  FuturesHedgeMeasuresSchema,
} from '../generated/ficant/rates/v1/analytics_pb';
import {
  PINNED_RULEPACK_PAYLOAD_BYTES,
  PrimitiveValidationError,
  RulePackVerificationError,
  createDecimalValue,
  createMarketTimeFromIso,
  createUnitRef,
  sha256PureHex,
} from '../codec';
import {
  BlotterClientError,
  InMemoryCredentialStore,
  type DeskBlotterRpcClient,
} from '../transport';
import {
  BlotterSupervisor,
  createBlotterStore,
  executePreviewCoveredHedge,
  fetchHedgeSnapshot,
  lookupCommittedAllocationsReadOnly,
  selectAccountAllocations,
  selectAllowedHedgeProductCodes,
  selectIsWholeAccountHedgeBlocked,
  selectIsWritesPaused,
  type ClockAdapter,
} from './index';

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

function makeValidSnapshot(
  accountId: string,
  seq: bigint,
  options: {
    readonly accountVersion?: bigint;
    readonly uncoveredCount?: number;
    readonly wholeAccountHedgeAllowed?: boolean;
    readonly coveredTaxBasis?: BondTaxBasis;
    readonly positionTaxBasis?: BondTaxBasis;
    readonly omitRiskSummary?: boolean;
    readonly allowedProducts?: readonly string[];
  } = {},
): AccountBlotterSnapshot {
  const units = makeUnits();
  const uncoveredCount = options.uncoveredCount ?? 0;
  return create(AccountBlotterSnapshotSchema, {
    snapshotSeq: seq,
    accountId,
    accountVersion: options.accountVersion ?? 1n,
    openingBase: create(OpeningPositionBaseSchema, {
      accountId,
      units,
    }),
    riskSummary: options.omitRiskSummary
      ? undefined
      : create(RiskAndCoverageSummarySchema, {
          coveredInstrumentCount: 1,
          uncoveredInstrumentCount: uncoveredCount,
          coveredTaxBasis: options.coveredTaxBasis ?? BondTaxBasis.PRE_TAX,
          wholeAccountHedgeAllowed: options.wholeAccountHedgeAllowed ?? uncoveredCount === 0,
        }),
    positions: [
      create(InstrumentPositionViewSchema, {
        accountId,
        instrumentCode: '240011.IB',
        positionVersion: 1n,
        coverageStatus: CoverageStatus.COVERED_PRE_TAX,
        taxBasis: options.positionTaxBasis ?? BondTaxBasis.PRE_TAX,
      }),
    ],
    allowedHedgeProductCodes: [...(options.allowedProducts ?? ['PROD1', 'PROD2'])],
  });
}

function makeAllocation(
  allocationId: string,
  pendingMatchId = '01JNB0G1PENDINGID000000009',
): ReconcileAllocationView {
  return create(ReconcileAllocationViewSchema, {
    allocationId: create(UlidSchema, { value: allocationId }),
    pendingMatchId: create(UlidSchema, { value: pendingMatchId }),
    sourceId: 'src-1',
    externalTradeId: 'ext-1',
    sourceRevision: 1n,
    actorId: 'trader-1',
  });
}

function makeEnvelope(
  seq: bigint,
  changes: Array<{
    readonly accountId: string;
    readonly snapshot?: AccountBlotterSnapshot | undefined;
    readonly allocations?: readonly ReconcileAllocationView[];
  }>,
): BlotterEventEnvelope {
  return create(BlotterEventEnvelopeSchema, {
    seq,
    eventType: 'TEST_EVENT',
    changes: changes.map((c) =>
      create(AccountChangeSchema, {
        accountId: c.accountId,
        replaceSnapshot: c.snapshot,
        changedAllocations: [...(c.allocations ?? [])],
      }),
    ),
  });
}

interface Deferred<T> {
  readonly promise: Promise<T>;
  readonly resolve: (value: T) => void;
  readonly reject: (err: unknown) => void;
}

function createDeferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (err: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

class ControllableStream {
  readonly #queue: Array<
    { readonly kind: 'item'; readonly value: SubscribeEventsResponse } | { readonly kind: 'end' } | { readonly kind: 'error'; readonly error: unknown }
  > = [];
  #waiter: Deferred<IteratorResult<SubscribeEventsResponse>> | null = null;
  #aborted = false;

  get aborted(): boolean {
    return this.#aborted;
  }

  attachSignal(signal?: AbortSignal): void {
    if (!signal) return;
    if (signal.aborted) {
      this.#aborted = true;
      this.fail(BlotterClientError.aborted());
      return;
    }
    signal.addEventListener(
      'abort',
      () => {
        this.#aborted = true;
        this.fail(BlotterClientError.aborted());
      },
      { once: true },
    );
  }

  push(frame: SubscribeEventsResponse): void {
    if (this.#waiter) {
      const w = this.#waiter;
      this.#waiter = null;
      w.resolve({ done: false, value: frame });
    } else {
      this.#queue.push({ kind: 'item', value: frame });
    }
  }

  end(): void {
    if (this.#waiter) {
      const w = this.#waiter;
      this.#waiter = null;
      w.resolve({ done: true, value: undefined });
    } else {
      this.#queue.push({ kind: 'end' });
    }
  }

  fail(error: unknown): void {
    if (this.#waiter) {
      const w = this.#waiter;
      this.#waiter = null;
      w.reject(error);
    } else {
      this.#queue.push({ kind: 'error', error });
    }
  }

  [Symbol.asyncIterator](): AsyncIterator<SubscribeEventsResponse> {
    return {
      next: () => {
        const nextItem = this.#queue.shift();
        if (nextItem) {
          if (nextItem.kind === 'item') {
            return Promise.resolve({ done: false, value: nextItem.value });
          }
          if (nextItem.kind === 'end') {
            return Promise.resolve({ done: true, value: undefined });
          }
          return Promise.reject(nextItem.error);
        }
        this.#waiter = createDeferred<IteratorResult<SubscribeEventsResponse>>();
        return this.#waiter.promise;
      },
    };
  }
}

class FakeClock implements ClockAdapter {
  #now = 1_000_000;
  #nextId = 1;
  readonly #timers = new Map<number, { readonly dueAt: number; readonly cb: () => void }>();

  now = (): number => this.#now;

  setTimeout = (
    callback: () => void,
    ms: number,
  ): ReturnType<typeof globalThis.setTimeout> => {
    const id = this.#nextId++;
    this.#timers.set(id, { dueAt: this.#now + ms, cb: callback });
    return id as unknown as ReturnType<typeof globalThis.setTimeout>;
  };

  clearTimeout = (handle: ReturnType<typeof globalThis.setTimeout>): void => {
    this.#timers.delete(handle as unknown as number);
  };

  advance(ms: number): void {
    this.#now += ms;
    const ready: Array<{ id: number; dueAt: number; cb: () => void }> = [];
    for (const [id, t] of this.#timers.entries()) {
      if (t.dueAt <= this.#now) {
        ready.push({ id, dueAt: t.dueAt, cb: t.cb });
      }
    }
    ready.sort((a, b) => a.dueAt - b.dueAt);
    for (const item of ready) {
      if (this.#timers.delete(item.id)) {
        item.cb();
      }
    }
  }
}

function createMockClient(
  overrides: Partial<
    Pick<
      DeskBlotterRpcClient,
      'getAccountSnapshot' | 'pollEvents' | 'subscribeEvents' | 'previewCoveredHedge'
    >
  >,
): DeskBlotterRpcClient {
  const unimpl = (): never => {
    throw new Error('Unexpected RPC call in test');
  };
  return {
    getAccountSnapshot: overrides.getAccountSnapshot ?? unimpl,
    pollEvents: overrides.pollEvents ?? unimpl,
    subscribeEvents: overrides.subscribeEvents ?? unimpl,
    previewCoveredHedge: overrides.previewCoveredHedge ?? unimpl,
    importOpeningBase: unimpl,
    ingestMarketQuotes: unimpl,
    createHold: unimpl,
    extendHold: unimpl,
    confirmHold: unimpl,
    releaseHold: unimpl,
    createCommitted: unimpl,
    cancelCommitted: unimpl,
    submitDraftEarmark: unimpl,
    confirmDraftEarmark: unimpl,
    ingestTradeFeed: unimpl,
    resolvePendingMatch: unimpl,
    sweepLifecycle: unimpl,
    setFeedControl: unimpl,
    fetchOpeningPositionBase: unimpl,
    parseNlpIntent: unimpl,
  };
}

async function flushMicrotasks(ticks = 6): Promise<void> {
  for (let i = 0; i < ticks; i += 1) {
    await Promise.resolve();
  }
}

describe('Cold-start empty accounts, baseline-first bootstrap, and pruned-journal idle account', () => {
  it('issues zero event RPCs on empty accounts, fetches baseline first, and opens SubscribeEvents at min(snapshot_seq)', async () => {
    const store = createBlotterStore();
    const credStore = new InMemoryCredentialStore();
    const callOrder: string[] = [];
    const streams: ControllableStream[] = [];
    let subscribedAfterSeq: bigint | null = null;

    const getAccountSnapshot = vi.fn(async (req: { accountId?: string }) => {
      callOrder.push(`snap:${req.accountId ?? ''}`);
      return create(GetAccountSnapshotResultSchema, {
        snapshot: makeValidSnapshot(req.accountId ?? '', 500n),
      });
    });
    const pollEvents = vi.fn(async () => create(PollEventsResultSchema, {}));
    const subscribeEvents = vi.fn((req: { afterSeq?: bigint }, opts?: { signal?: AbortSignal }) => {
      callOrder.push(`sub:${(req.afterSeq ?? 0n).toString()}`);
      subscribedAfterSeq = req.afterSeq ?? 0n;
      const s = new ControllableStream();
      s.attachSignal(opts?.signal);
      streams.push(s);
      return s;
    });

    const supervisor = new BlotterSupervisor({
      store,
      client: createMockClient({ getAccountSnapshot, pollEvents, subscribeEvents }),
      credentialStore: credStore,
      location: { protocol: 'http:', hostname: '127.0.0.1' },
    });

    supervisor.login({ token: 'tok-idle', headerMode: 'authorization' });
    expect(store.getState().connectionState).toBe('idle_no_accounts');
    expect(getAccountSnapshot).not.toHaveBeenCalled();
    expect(subscribeEvents).not.toHaveBeenCalled();
    expect(pollEvents).not.toHaveBeenCalled();

    // Empty account list submission also issues 0 RPCs
    await supervisor.submitAccounts(['   ', '']);
    expect(getAccountSnapshot).not.toHaveBeenCalled();
    expect(subscribeEvents).not.toHaveBeenCalled();
    expect(pollEvents).not.toHaveBeenCalled();

    // Single idle authorized account on a pruned journal (snapshot_seq=500)
    const res = await supervisor.submitAccounts(['acct-idle', 'acct-idle']);
    expect(res.ok).toBe(true);
    expect(getAccountSnapshot).toHaveBeenCalledTimes(1);
    expect(subscribeEvents).toHaveBeenCalledTimes(1);
    expect(callOrder).toEqual(['snap:acct-idle', 'sub:500']);
    expect(subscribedAfterSeq).toBe(500n);
    expect(store.getState().snapshots['acct-idle']?.snapshotSeq).toBe(500n);
    expect(store.getState().cursorSeq).toBe(500n);
    expect(pollEvents).not.toHaveBeenCalled();

    supervisor.dispose();
  });
});

describe('Session generation switching, logout, and late response/UNAUTHENTICATED dropping', () => {
  it('cancels active streams, wipes all session state/intents, and drops late responses and stale UNAUTHENTICATED', async () => {
    const store = createBlotterStore();
    const credStore = new InMemoryCredentialStore();
    const deferredSnapB = createDeferred<GetAccountSnapshotResult>();
    const streams: ControllableStream[] = [];

    let snapCall = 0;
    const getAccountSnapshot = vi.fn(async (req: { accountId?: string }) => {
      snapCall += 1;
      if (snapCall === 1) {
        return create(GetAccountSnapshotResultSchema, {
          snapshot: makeValidSnapshot(req.accountId ?? 'acct-a', 10n),
        });
      }
      if (req.accountId === 'acct-b-slow') {
        return deferredSnapB.promise;
      }
      return create(GetAccountSnapshotResultSchema, {
        snapshot: makeValidSnapshot(req.accountId ?? 'acct-a', 20n),
      });
    });

    const subscribeEvents = vi.fn((_req: { afterSeq?: bigint }, opts?: { signal?: AbortSignal }) => {
      const s = new ControllableStream();
      s.attachSignal(opts?.signal);
      streams.push(s);
      return s;
    });

    const supervisor = new BlotterSupervisor({
      store,
      client: createMockClient({ getAccountSnapshot, subscribeEvents }),
      credentialStore: credStore,
      location: { protocol: 'http:', hostname: '127.0.0.1' },
    });

    // Session 1
    const gen1 = supervisor.login({ token: 'tok-session-1', headerMode: 'authorization' });
    await supervisor.submitAccounts(['acct-a']);
    store.getState().setDraftState(gen1, {
      draftInputText: '买入 240011',
      activeDraftConfirmation: {
        draftId: '01JNB0G1DRAFTID00000000010',
        accountId: 'acct-a',
        confirmationTokenBytes: new Uint8Array(32).fill(9),
        previewAtSeq: 10n,
      },
    });
    store.getState().recordWriteReceipt(gen1, {
      id: 'rcpt-1',
      operation: 'CreateHold',
      accountId: 'acct-a',
      committedSeq: 15n,
      replayed: false,
      status: 'succeeded',
      recordedAtMs: 1000,
    });
    supervisor.writeIntents.acquire({
      operation: 'CreateHold',
      canonicalPayload: { notional: '100' },
      sessionGeneration: gen1,
    });

    // Start a slow submit in Session 1 for acct-b-slow
    const slowPromise = supervisor.submitAccounts(['acct-b-slow']);
    const firstStream = streams[0];

    // Switch credential to Session 2 BEFORE slowPromise finishes
    const gen2 = supervisor.login({ token: 'tok-session-2', headerMode: 'x-ficant-credential' });
    expect(gen2).toBe(gen1 + 1);
    expect(firstStream?.aborted).toBe(true);
    expect(supervisor.writeIntents.size()).toBe(0);
    expect(store.getState().snapshots).toEqual({});
    expect(store.getState().writeReceipts).toEqual([]);
    expect(store.getState().draftInputText).toBe('');
    expect(store.getState().activeDraftConfirmation).toBeNull();
    expect(store.getState().hedgePreview).toBeNull();
    expect(store.getState().cursorSeq).toBe(0n);

    // Late success from Session 1 resolves AFTER switch -> must NOT pollute Session 2!
    deferredSnapB.resolve(
      create(GetAccountSnapshotResultSchema, {
        snapshot: makeValidSnapshot('acct-b-slow', 99n),
      }),
    );
    await slowPromise;
    expect(store.getState().snapshots).toEqual({});
    expect(store.getState().cursorSeq).toBe(0n);

    // Late UNAUTHENTICATED error from Session 1 must NOT log out Session 2!
    const unauthErr = BlotterClientError.fromErrorDetail(
      create(ErrorDetailSchema, { code: ErrorCode.UNAUTHENTICATED, message: 'revoked' }),
    );
    expect(store.getState().handleUnauthenticated(gen1, unauthErr)).toBe(false);
    expect(store.getState().authenticated).toBe(true);
    expect(store.getState().sessionGeneration).toBe(gen2);

    // Active UNAUTHENTICATED for Session 2 logs out and wipes immediately
    expect(store.getState().handleUnauthenticated(gen2, unauthErr)).toBe(true);
    expect(store.getState().authenticated).toBe(false);
    expect(store.getState().connectionState).toBe('unauthenticated');
    expect(store.getState().sessionGeneration).toBe(gen2 + 1);

    supervisor.dispose();
  });
});

describe('Single synchronous atomic committer: cursor CAS, out-of-order polls, write receipts, empty batches & RBAC gaps', () => {
  it('rejects late poll 11 after stream advanced 10->12, rejects out-of-order twin polls, rejects out-of-range heartbeats/polls, allows empty batch & RBAC gaps, and never advances cursor on write receipts', () => {
    const store = createBlotterStore();
    const gen = store.getState().beginSession({ headerMode: 'authorization' });
    const bootFence = store.getState().activateReader(gen, 'bootstrap', 'bootstrapping')!;
    store.getState().commitBaselineSnapshots({
      ...bootFence,
      snapshots: new Map([['acct-a', makeValidSnapshot('acct-a', 10n)]]),
      cursorSeq: 10n,
      headSeq: 10n,
      nextConnectionState: 'stream_live',
    });

    // Write receipt with committed_seq=25 MUST NOT advance cursorSeq (remains 10n)
    expect(
      store.getState().recordWriteReceipt(gen, {
        id: 'rcpt-25',
        operation: 'CreateHold',
        accountId: 'acct-a',
        committedSeq: 25n,
        replayed: false,
        status: 'succeeded',
        recordedAtMs: 1234,
      }),
    ).toBe(true);
    expect(store.getState().cursorSeq).toBe(10n);
    expect(store.getState().latestReceiptCommittedSeq).toBe(25n);

    // Switch to stream reader, simulate poll started at r=10 before stream advanced to 12
    const streamFence = store.getState().activateReader(gen, 'stream', 'stream_live')!;
    const streamCommit = store.getState().commitStreamEnvelope({
      ...streamFence,
      envelope: makeEnvelope(12n, [
        { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', 12n, { accountVersion: 2n }) },
      ]),
    });
    expect(streamCommit).toEqual({ committed: true, cursorSeq: 12n });
    expect(store.getState().cursorSeq).toBe(12n);

    // Late poll from r=10 arriving with scanned_through_seq=11 -> rejected with 0 listener bumps!
    const pollFence = store.getState().activateReader(gen, 'poll', 'poll_degraded')!;
    const listener = vi.fn();
    const unsub = store.subscribe(listener);

    const latePollOutcome = store.getState().commitPollBatch({
      ...pollFence,
      requestAfterSeq: 10n, // stale: store.cursorSeq is 12n!
      batch: create(PollEventsBatchSchema, {
        events: [
          makeEnvelope(11n, [
            {
              accountId: 'acct-a',
              snapshot: makeValidSnapshot('acct-a', 11n, { accountVersion: 99n }),
            },
          ]),
        ],
        scannedThroughSeq: 11n,
        currentHeadSeq: 12n,
        hasMore: false,
      }),
    });
    expect(latePollOutcome).toEqual({ committed: false, reason: 'stale_cursor' });
    expect(listener).not.toHaveBeenCalled();
    expect(store.getState().cursorSeq).toBe(12n);
    expect(store.getState().snapshots['acct-a']?.accountVersion).toBe(2n);

    // Twin polls both starting at r=12: poll B commits 12->14 (RBAC seq gap 13 skipped), then late poll A (12->13) is rejected!
    const pollB = store.getState().commitPollBatch({
      ...pollFence,
      requestAfterSeq: 12n,
      batch: create(PollEventsBatchSchema, {
        events: [
          makeEnvelope(14n, [
            {
              accountId: 'acct-a',
              snapshot: makeValidSnapshot('acct-a', 14n, { accountVersion: 3n }),
            },
          ]),
        ],
        scannedThroughSeq: 14n,
        currentHeadSeq: 14n,
        hasMore: false,
      }),
    });
    expect(pollB).toEqual({ committed: true, cursorSeq: 14n });
    expect(store.getState().cursorSeq).toBe(14n);

    listener.mockClear();
    const pollAOutOfOrder = store.getState().commitPollBatch({
      ...pollFence,
      requestAfterSeq: 12n,
      batch: create(PollEventsBatchSchema, {
        events: [
          makeEnvelope(13n, [
            { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', 13n) },
          ]),
        ],
        scannedThroughSeq: 13n,
        currentHeadSeq: 14n,
        hasMore: false,
      }),
    });
    expect(pollAOutOfOrder).toEqual({ committed: false, reason: 'stale_cursor' });
    expect(listener).not.toHaveBeenCalled();

    // Legal empty poll batch advances cursor 14 -> 18
    const emptyAdvance = store.getState().commitPollBatch({
      ...pollFence,
      requestAfterSeq: 14n,
      batch: create(PollEventsBatchSchema, {
        events: [],
        scannedThroughSeq: 18n,
        currentHeadSeq: 20n,
        hasMore: false,
      }),
    });
    expect(emptyAdvance).toEqual({ committed: true, cursorSeq: 18n });
    expect(store.getState().cursorSeq).toBe(18n);

    // Out-of-range poll batch (scanned_through_seq > current_head_seq) rejected with 0 listener bumps
    listener.mockClear();
    const badPollWatermark = store.getState().commitPollBatch({
      ...pollFence,
      requestAfterSeq: 18n,
      batch: create(PollEventsBatchSchema, {
        events: [],
        scannedThroughSeq: 25n,
        currentHeadSeq: 20n,
        hasMore: false,
      }),
    });
    expect(badPollWatermark.committed).toBe(false);
    expect(listener).not.toHaveBeenCalled();

    // Out-of-range heartbeat (scanned_through_seq > head_seq) rejected with 0 listener bumps
    const liveFence = store.getState().activateReader(gen, 'stream', 'stream_live')!;
    listener.mockClear();
    const badHeartbeat = store.getState().commitStreamHeartbeat({
      ...liveFence,
      heartbeat: create(StreamHeartbeatSchema, {
        headSeq: 20n,
        scannedThroughSeq: 21n,
      }),
    });
    expect(badHeartbeat.committed).toBe(false);
    expect(listener).not.toHaveBeenCalled();

    // Old reader / stale transportEpoch rejected
    const staleReaderCommit = store.getState().commitStreamEnvelope({
      ...streamFence, // older transportEpoch!
      envelope: makeEnvelope(19n, [
        { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', 19n) },
      ]),
    });
    expect(staleReaderCommit).toEqual({ committed: false, reason: 'stale_transport_epoch' });
    expect(listener).not.toHaveBeenCalled();

    unsub();
  });
});

describe('Parameterized "A valid, B corrupt" across SubscribeEvents, PollEvents, and Recovery + corrupt 2nd Poll event', () => {
  type CorruptionCase = {
    readonly name: string;
    readonly buildEnvelope: (seq: bigint) => BlotterEventEnvelope;
  };

  const corruptionCases: readonly CorruptionCase[] = [
    {
      name: 'envelope.seq === 0',
      buildEnvelope: () =>
        makeEnvelope(0n, [
          { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', 0n) },
        ]),
    },
    {
      name: 'B empty account_id',
      buildEnvelope: (seq) =>
        makeEnvelope(seq, [
          { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', seq, { accountVersion: 9n }) },
          { accountId: '', snapshot: makeValidSnapshot('acct-b', seq) },
        ]),
    },
    {
      name: 'duplicate account_id (acct-a twice)',
      buildEnvelope: (seq) =>
        makeEnvelope(seq, [
          { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', seq, { accountVersion: 9n }) },
          { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', seq, { accountVersion: 10n }) },
        ]),
    },
    {
      name: 'B missing replace_snapshot',
      buildEnvelope: (seq) =>
        makeEnvelope(seq, [
          { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', seq, { accountVersion: 9n }) },
          { accountId: 'acct-b', snapshot: undefined },
        ]),
    },
    {
      name: 'B inner account_id !== outer account_id',
      buildEnvelope: (seq) =>
        makeEnvelope(seq, [
          { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', seq, { accountVersion: 9n }) },
          { accountId: 'acct-b', snapshot: makeValidSnapshot('acct-wrong', seq) },
        ]),
    },
    {
      name: 'B snapshot_seq !== envelope.seq',
      buildEnvelope: (seq) =>
        makeEnvelope(seq, [
          { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', seq, { accountVersion: 9n }) },
          { accountId: 'acct-b', snapshot: makeValidSnapshot('acct-b', seq - 1n) },
        ]),
    },
    {
      name: 'B missing risk_summary',
      buildEnvelope: (seq) =>
        makeEnvelope(seq, [
          { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', seq, { accountVersion: 9n }) },
          { accountId: 'acct-b', snapshot: makeValidSnapshot('acct-b', seq, { omitRiskSummary: true }) },
        ]),
    },
    {
      name: 'B covered_tax_basis !== PRE_TAX',
      buildEnvelope: (seq) =>
        makeEnvelope(seq, [
          { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', seq, { accountVersion: 9n }) },
          {
            accountId: 'acct-b',
            snapshot: makeValidSnapshot('acct-b', seq, { coveredTaxBasis: BondTaxBasis.TAX_ADJUSTED }),
          },
        ]),
    },
    {
      name: 'B uncovered_instrument_count > 0 && whole_account_hedge_allowed === true',
      buildEnvelope: (seq) =>
        makeEnvelope(seq, [
          { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', seq, { accountVersion: 9n }) },
          {
            accountId: 'acct-b',
            snapshot: makeValidSnapshot('acct-b', seq, {
              uncoveredCount: 1,
              wholeAccountHedgeAllowed: true,
            }),
          },
        ]),
    },
    {
      name: 'B position tax_basis !== PRE_TAX',
      buildEnvelope: (seq) =>
        makeEnvelope(seq, [
          { accountId: 'acct-a', snapshot: makeValidSnapshot('acct-a', seq, { accountVersion: 9n }) },
          {
            accountId: 'acct-b',
            snapshot: makeValidSnapshot('acct-b', seq, {
              positionTaxBasis: BondTaxBasis.TAX_ADJUSTED,
            }),
          },
        ]),
    },
  ];

  function createSeededStore() {
    const store = createBlotterStore();
    const gen = store.getState().beginSession({ headerMode: 'authorization' });
    const boot = store.getState().activateReader(gen, 'bootstrap', 'bootstrapping')!;
    const initAlloc = makeAllocation('01JNB0G1ALLOCINIT000000001');
    store.getState().commitBaselineSnapshots({
      ...boot,
      snapshots: new Map([
        ['acct-a', makeValidSnapshot('acct-a', 100n, { accountVersion: 1n })],
        ['acct-b', makeValidSnapshot('acct-b', 100n, { accountVersion: 1n })],
      ]),
      allocations: new Map([
        [
          'acct-a',
          [{ seq: 100n, allocationId: '01JNB0G1ALLOCINIT000000001', accountId: 'acct-a', view: initAlloc }],
        ],
      ]),
      cursorSeq: 100n,
      headSeq: 100n,
      nextConnectionState: 'stream_live',
    });
    return { store, gen };
  }

  for (const testCase of corruptionCases) {
    it(`SubscribeEvents atomic rollback on [${testCase.name}]`, () => {
      const { store, gen } = createSeededStore();
      const fence = store.getState().activateReader(gen, 'stream', 'stream_live')!;
      const beforeCommitVer = store.getState().businessCommitVersion;
      const listener = vi.fn();
      const unsub = store.subscribe(listener);

      const outcome = store.getState().commitStreamEnvelope({
        ...fence,
        envelope: testCase.buildEnvelope(101n),
      });
      expect(outcome.committed).toBe(false);
      expect(listener).not.toHaveBeenCalled();
      expect(store.getState().cursorSeq).toBe(100n);
      expect(store.getState().businessCommitVersion).toBe(beforeCommitVer);
      expect(store.getState().snapshots['acct-a']?.accountVersion).toBe(1n);
      expect(store.getState().snapshots['acct-b']?.accountVersion).toBe(1n);
      expect(selectAccountAllocations(store.getState(), 'acct-a')).toHaveLength(1);
      unsub();
    });

    it(`PollEvents atomic rollback on [${testCase.name}]`, () => {
      const { store, gen } = createSeededStore();
      const fence = store.getState().activateReader(gen, 'poll', 'poll_degraded')!;
      const beforeCommitVer = store.getState().businessCommitVersion;
      const listener = vi.fn();
      const unsub = store.subscribe(listener);

      const outcome = store.getState().commitPollBatch({
        ...fence,
        requestAfterSeq: 100n,
        batch: create(PollEventsBatchSchema, {
          events: [testCase.buildEnvelope(101n)],
          scannedThroughSeq: 101n,
          currentHeadSeq: 101n,
          hasMore: false,
        }),
      });
      expect(outcome.committed).toBe(false);
      expect(listener).not.toHaveBeenCalled();
      expect(store.getState().cursorSeq).toBe(100n);
      expect(store.getState().businessCommitVersion).toBe(beforeCommitVer);
      expect(store.getState().snapshots['acct-a']?.accountVersion).toBe(1n);
      expect(store.getState().snapshots['acct-b']?.accountVersion).toBe(1n);
      expect(selectAccountAllocations(store.getState(), 'acct-a')).toHaveLength(1);
      unsub();
    });

    it(`Recovery replay atomic rollback on [${testCase.name}] even when B baseline already covers seq 101`, async () => {
      const { store, gen } = createSeededStore();
      const credStore = new InMemoryCredentialStore();
      const beforeCommitVer = store.getState().businessCommitVersion;

      // Baseline fetch yields A@100 and B@102 -> replay at seq 101 would normally skip B,
      // yet B MUST STILL BE VALIDATED so corrupt B aborts the entire recovery transaction!
      const getAccountSnapshot = vi.fn(async (req: { accountId?: string }) =>
        create(GetAccountSnapshotResultSchema, {
          snapshot: makeValidSnapshot(
            req.accountId ?? '',
            req.accountId === 'acct-a' ? 100n : 102n,
            { accountVersion: 5n },
          ),
        }),
      );
      const pollEvents = vi.fn(async () =>
        create(PollEventsResultSchema, {
          outcome: {
            case: 'batch',
            value: create(PollEventsBatchSchema, {
              events: [testCase.buildEnvelope(101n)],
              scannedThroughSeq: 102n,
              currentHeadSeq: 102n,
              hasMore: false,
            }),
          },
        }),
      );

      const supervisor = new BlotterSupervisor({
        store,
        client: createMockClient({ getAccountSnapshot, pollEvents }),
        credentialStore: credStore,
      });

      const ok = await supervisor.runRecoveryBarrier(gen);
      expect(ok).toBe(false);
      // Original A@100, B@100, allocations, cursor, and businessCommitVersion are 100% preserved!
      expect(store.getState().cursorSeq).toBe(100n);
      expect(store.getState().businessCommitVersion).toBe(beforeCommitVer);
      expect(store.getState().snapshots['acct-a']?.accountVersion).toBe(1n);
      expect(store.getState().snapshots['acct-b']?.accountVersion).toBe(1n);
      expect(selectAccountAllocations(store.getState(), 'acct-a')).toHaveLength(1);

      supervisor.dispose();
    });
  }

  it('PollEvents rolls back ENTIRE batch when 1st event is valid and 2nd event is corrupt (0 state/cursor/listener bump)', () => {
    const { store, gen } = createSeededStore();
    const fence = store.getState().activateReader(gen, 'poll', 'poll_degraded')!;
    const beforeCommitVer = store.getState().businessCommitVersion;
    const listener = vi.fn();
    const unsub = store.subscribe(listener);

    const validFirst = makeEnvelope(101n, [
      {
        accountId: 'acct-a',
        snapshot: makeValidSnapshot('acct-a', 101n, { accountVersion: 2n }),
        allocations: [makeAllocation('01JNB0G1ALLOC1010000000002')],
      },
    ]);
    const corruptSecond = makeEnvelope(102n, [
      {
        accountId: 'acct-b',
        snapshot: makeValidSnapshot('acct-b', 102n, { positionTaxBasis: BondTaxBasis.UNSPECIFIED }),
      },
    ]);

    const outcome = store.getState().commitPollBatch({
      ...fence,
      requestAfterSeq: 100n,
      batch: create(PollEventsBatchSchema, {
        events: [validFirst, corruptSecond],
        scannedThroughSeq: 102n,
        currentHeadSeq: 102n,
        hasMore: false,
      }),
    });

    expect(outcome.committed).toBe(false);
    expect(listener).not.toHaveBeenCalled();
    expect(store.getState().cursorSeq).toBe(100n);
    expect(store.getState().businessCommitVersion).toBe(beforeCommitVer);
    expect(store.getState().snapshots['acct-a']?.accountVersion).toBe(1n);
    expect(selectAccountAllocations(store.getState(), 'acct-a')).toHaveLength(1);
    unsub();
  });
});

describe('Two-phase multi-account recovery barrier (A@100 -> A@101 -> B@102, re-pruning, new account) and 35s watchdog', () => {
  it('replays A@100 -> A@101 -> B@102 skipping covered B@101, deduplicates allocations, admits new account C, keeps view & pauses writes during recovery, and recovers from re-pruning', async () => {
    const store = createBlotterStore();
    const credStore = new InMemoryCredentialStore();
    const clock = new FakeClock();
    const streams: ControllableStream[] = [];

    const gen = store.getState().beginSession({ headerMode: 'authorization' });
    const boot = store.getState().activateReader(gen, 'bootstrap', 'bootstrapping')!;
    store.getState().commitBaselineSnapshots({
      ...boot,
      snapshots: new Map([
        ['acct-a', makeValidSnapshot('acct-a', 90n, { accountVersion: 1n })],
        ['acct-b', makeValidSnapshot('acct-b', 90n, { accountVersion: 1n })],
      ]),
      cursorSeq: 90n,
      headSeq: 90n,
      nextConnectionState: 'stream_live',
    });

    let recoveryAttempt = 0;
    const pollGate = createDeferred<PollEventsResult>();

    const getAccountSnapshot = vi.fn(async (req: { accountId?: string }) => {
      if (req.accountId === 'acct-a') {
        recoveryAttempt += 1;
        return create(GetAccountSnapshotResultSchema, {
          snapshot: makeValidSnapshot('acct-a', 100n, { accountVersion: 10n }),
        });
      }
      return create(GetAccountSnapshotResultSchema, {
        snapshot: makeValidSnapshot('acct-b', 102n, { accountVersion: 20n }),
      });
    });

    let pollCount = 0;
    const pollEvents = vi.fn(async (req: { afterSeq?: bigint }) => {
      pollCount += 1;
      if (pollCount === 1) {
        // Attempt 1 hits re-pruning DURING recovery!
        return create(PollEventsResultSchema, {
          outcome: {
            case: 'snapshotFallback',
            value: create(SnapshotFallbackRequiredSchema, {
              requestedAfterSeq: 100n,
              oldestRetainedSeq: 101n,
              currentHeadSeq: 102n,
              reason: 're-pruned mid-recovery',
            }),
          },
        });
      }
      if (pollCount === 2) {
        return pollGate.promise;
      }
      const afterSeq = req.afterSeq ?? 102n;
      return create(PollEventsResultSchema, {
        outcome: {
          case: 'batch',
          value: create(PollEventsBatchSchema, {
            events: [],
            scannedThroughSeq: afterSeq,
            currentHeadSeq: afterSeq,
            hasMore: false,
          }),
        },
      });
    });

    const subscribeEvents = vi.fn((_req: { afterSeq?: bigint }, opts?: { signal?: AbortSignal }) => {
      const s = new ControllableStream();
      s.attachSignal(opts?.signal);
      streams.push(s);
      return s;
    });

    const supervisor = new BlotterSupervisor({
      store,
      client: createMockClient({ getAccountSnapshot, pollEvents, subscribeEvents }),
      credentialStore: credStore,
      clock,
      recoveryBackoffBaseMs: 100,
    });

    const recoveryPromise = supervisor.runRecoveryBarrier(gen);
    await flushMicrotasks();

    // During recovery: last complete view (90n) stays on screen and writes are paused!
    expect(store.getState().connectionState).toBe('recovering');
    expect(selectIsWritesPaused(store.getState())).toBe(true);
    expect(store.getState().cursorSeq).toBe(90n);
    expect(store.getState().snapshots['acct-a']?.snapshotSeq).toBe(90n);

    // Advance clock past 100ms backoff so attempt 2 starts
    clock.advance(150);
    await flushMicrotasks();
    expect(recoveryAttempt).toBe(2);

    // Deliver replay batch containing:
    // - seq 101: A@101 (v11) + B@101 (v99, MUST BE SKIPPED because B baseline is 102!) + duplicate allocation IDs
    // - seq 102: newly discovered authorized account C@102 (v1)
    const dupAlloc = makeAllocation('01JNB0G1ALLOCDUP0000000001');
    pollGate.resolve(
      create(PollEventsResultSchema, {
        outcome: {
          case: 'batch',
          value: create(PollEventsBatchSchema, {
            events: [
              makeEnvelope(101n, [
                {
                  accountId: 'acct-a',
                  snapshot: makeValidSnapshot('acct-a', 101n, { accountVersion: 11n }),
                  allocations: [dupAlloc, dupAlloc],
                },
                {
                  accountId: 'acct-b',
                  snapshot: makeValidSnapshot('acct-b', 101n, { accountVersion: 99n }),
                },
              ]),
              makeEnvelope(102n, [
                {
                  accountId: 'acct-c',
                  snapshot: makeValidSnapshot('acct-c', 102n, { accountVersion: 1n }),
                },
              ]),
            ],
            scannedThroughSeq: 102n,
            currentHeadSeq: 102n,
            hasMore: false,
          }),
        },
      }),
    );

    const recovered = await recoveryPromise;
    expect(recovered).toBe(true);
    expect(store.getState().connectionState).toBe('stream_live');
    expect(selectIsWritesPaused(store.getState())).toBe(false);
    expect(store.getState().cursorSeq).toBe(102n);
    // A@101 retained, B@102 (v20, NOT overwritten by B@101 v99) retained, C@102 admitted!
    expect(store.getState().snapshots['acct-a']?.snapshotSeq).toBe(101n);
    expect(store.getState().snapshots['acct-a']?.accountVersion).toBe(11n);
    expect(store.getState().snapshots['acct-b']?.snapshotSeq).toBe(102n);
    expect(store.getState().snapshots['acct-b']?.accountVersion).toBe(20n);
    expect(store.getState().snapshots['acct-c']?.snapshotSeq).toBe(102n);
    // Allocations deduplicated by (seq, allocation_id)
    expect(selectAccountAllocations(store.getState(), 'acct-a')).toHaveLength(1);
    expect(subscribeEvents).toHaveBeenCalledTimes(1);

    // Controllable 35s watchdog silence degrades stream -> poll_degraded!
    clock.advance(35_000);
    await flushMicrotasks();
    expect(store.getState().connectionState).toBe('poll_degraded');
    expect(streams[0]?.aborted).toBe(true);

    supervisor.dispose();
  });
});

describe('Non-publishing fetchHedgeSnapshot, executePreviewCoveredHedge (0 RPCs on bad RulePack), and read-only allocation lookup', () => {
  const units = makeUnits();
  const valuationAt = createMarketTimeFromIso('2026-06-15T02:00:00Z', 'Asia/Shanghai', '2026-06-15');

  function makeSyntheticRulePackOpts() {
    const payload = new Uint8Array(PINNED_RULEPACK_PAYLOAD_BYTES).fill(0x33);
    const payloadSha = sha256PureHex(payload);
    const manifest = { ...TRUSTED_RULEPACK_ANCHOR.manifest, payload_sha256: payloadSha };
    const manifestBytes = new TextEncoder().encode(`${JSON.stringify(manifest, null, 2)}\n`);
    const manifestSha = sha256PureHex(manifestBytes);
    return {
      expectedManifestSha256: manifestSha,
      expectedPayloadSha256: payloadSha,
      anchor: {
        ...TRUSTED_RULEPACK_ANCHOR,
        manifestSha256: manifestSha,
        payloadSha256: payloadSha,
        manifest,
      },
      payloadBytes: payload,
      subtleOverride: null as null,
    };
  }

  function makeHedgeResult(seq: bigint): PreviewCoveredHedgeResult {
    return create(PreviewCoveredHedgeResultSchema, {
      evaluatedAtSeq: seq,
      scope: PreviewHedgeScope.COVERED_SUBSET_ONLY,
      productCode: 'PROD1',
      contractCode: 'PROD1-2609',
      zeroNetExposure: false,
      coveredTaxBasis: BondTaxBasis.PRE_TAX,
      coveredInstrumentCount: 2,
      uncoveredInstrumentCount: 1,
      coveredNetPreTaxDv01: createDecimalValue('5000', units.dv01!),
      excludedUncoveredGrossNotional: createDecimalValue('10000000', units.currencyAmount!),
      measures: create(FuturesHedgeMeasuresSchema, {
        futuresContractDv01: createDecimalValue('800', units.dv01!),
        rawContracts: createDecimalValue('6.25', units.contractCount!),
        recommendedContracts: 6n,
        residualDv01: createDecimalValue('200', units.dv01!),
        hedgeEffectiveness: createDecimalValue('0.96', units.dimensionless!),
        contractNotional: createDecimalValue('1000000', units.currencyAmount!),
      }),
    });
  }

  it('blocks hedge RPCs (0 RPCs) on invalid/expired RulePack, retries conflict once via private fetchHedgeSnapshot with 0 store mutation, and drops stale session results', async () => {
    const store = createBlotterStore();
    const gen = store.getState().beginSession({ headerMode: 'authorization' });
    const boot = store.getState().activateReader(gen, 'bootstrap', 'bootstrapping')!;
    const acctSnap = makeValidSnapshot('acct-a', 50n, {
      uncoveredCount: 1,
      wholeAccountHedgeAllowed: false,
      allowedProducts: ['PROD1', 'PROD2'],
    });
    store.getState().commitBaselineSnapshots({
      ...boot,
      snapshots: new Map([['acct-a', acctSnap]]),
      cursorSeq: 50n,
      headSeq: 50n,
    });

    expect(selectAllowedHedgeProductCodes(store.getState(), 'acct-a')).toEqual(['PROD1', 'PROD2']);
    expect(selectIsWholeAccountHedgeBlocked(store.getState(), 'acct-a')).toBe(true);

    const storeListener = vi.fn();
    const unsub = store.subscribe(storeListener);

    const getAccountSnapshot = vi.fn(async () =>
      create(GetAccountSnapshotResultSchema, {
        snapshot: makeValidSnapshot('acct-a', 55n, {
          uncoveredCount: 1,
          wholeAccountHedgeAllowed: false,
          allowedProducts: ['PROD1', 'PROD2'],
        }),
      }),
    );
    let hedgeCalls = 0;
    const previewCoveredHedge = vi.fn(async (req: { expectedSnapshotSeq?: bigint }) => {
      hedgeCalls += 1;
      if (hedgeCalls === 1) {
        throw BlotterClientError.fromErrorDetail(
          create(ErrorDetailSchema, {
            code: ErrorCode.CONCURRENCY_CONFLICT,
            message: 'global head advanced',
            retryable: true,
          }),
        );
      }
      return makeHedgeResult(req.expectedSnapshotSeq ?? 55n);
    });

    const client = createMockClient({ getAccountSnapshot, previewCoveredHedge });

    // 1. Expired RulePack (valuationAt === effective_to) fails BEFORE any RPC (0 RPCs!)
    await expect(
      executePreviewCoveredHedge(client, {
        sessionGeneration: gen,
        getCurrentSessionGeneration: () => store.getState().sessionGeneration,
        snapshot: acctSnap,
        scope: PreviewHedgeScope.COVERED_SUBSET_ONLY,
        productCode: 'PROD1',
        contractCode: 'PROD1-2609',
        ctdDv01Per100: createDecimalValue('0.085', units.dv01Per100!),
        conversionFactor: createDecimalValue('1.01', units.dimensionless!),
        valuationAt: createMarketTimeFromIso(
          TRUSTED_RULEPACK_ANCHOR.manifest.effective_to.instant,
          'Asia/Shanghai',
          TRUSTED_RULEPACK_ANCHOR.manifest.effective_to.local_trading_date,
        ),
      }),
    ).rejects.toThrow(RulePackVerificationError);
    expect(getAccountSnapshot).not.toHaveBeenCalled();
    expect(previewCoveredHedge).not.toHaveBeenCalled();

    // 1b. Malformed MarketTime valuationAt (local_trading_date mismatch & invalid timezone) fails with 0 RPCs (M1-F04)
    const malformedDateMt = create(MarketTimeSchema, {
      instant: create(TimestampSchema, {
        seconds: BigInt(Math.floor(new Date('2026-06-15T02:00:00Z').getTime() / 1000)),
        nanos: 0,
      }),
      marketTimezone: 'Asia/Shanghai',
      localTradingDate: '2026-06-14',
    });
    const invalidTzMt = create(MarketTimeSchema, {
      instant: create(TimestampSchema, {
        seconds: BigInt(Math.floor(new Date('2026-06-15T02:00:00Z').getTime() / 1000)),
        nanos: 0,
      }),
      marketTimezone: 'Invalid/Timezone',
      localTradingDate: '2026-06-15',
    });
    for (const badValuationAt of [malformedDateMt, invalidTzMt]) {
      await expect(
        executePreviewCoveredHedge(client, {
          sessionGeneration: gen,
          getCurrentSessionGeneration: () => store.getState().sessionGeneration,
          snapshot: acctSnap,
          scope: PreviewHedgeScope.COVERED_SUBSET_ONLY,
          productCode: 'PROD1',
          contractCode: 'PROD1-2609',
          ctdDv01Per100: createDecimalValue('0.085', units.dv01Per100!),
          conversionFactor: createDecimalValue('1.01', units.dimensionless!),
          valuationAt: badValuationAt,
          rulePackOptions: makeSyntheticRulePackOpts(),
        }),
      ).rejects.toThrow(PrimitiveValidationError);
    }
    expect(getAccountSnapshot).not.toHaveBeenCalled();
    expect(previewCoveredHedge).not.toHaveBeenCalled();

    // 2. Valid RulePack + CONCURRENCY_CONFLICT -> refreshes private fetchHedgeSnapshot once (55n)
    //    and succeeds with ZERO mutation to workspace store or cursorSeq (remains 50n)!
    const outcome = await executePreviewCoveredHedge(client, {
      sessionGeneration: gen,
      getCurrentSessionGeneration: () => store.getState().sessionGeneration,
      snapshot: acctSnap,
      scope: PreviewHedgeScope.COVERED_SUBSET_ONLY,
      productCode: 'PROD1',
      contractCode: 'PROD1-2609',
      ctdDv01Per100: createDecimalValue('0.085', units.dv01Per100!),
      conversionFactor: createDecimalValue('1.01', units.dimensionless!),
      valuationAt,
      rulePackOptions: makeSyntheticRulePackOpts(),
    });
    expect(outcome.status).toBe('ok');
    if (outcome.status === 'ok') {
      expect(outcome.retriedConflict).toBe(true);
      expect(outcome.usedSnapshotSeq).toBe(55n);
      expect(outcome.result.measures?.recommendedContracts).toBe(6n);
    }
    expect(getAccountSnapshot).toHaveBeenCalledTimes(1);
    expect(previewCoveredHedge).toHaveBeenCalledTimes(2);
    expect(store.getState().cursorSeq).toBe(50n);
    expect(store.getState().snapshots['acct-a']?.snapshotSeq).toBe(50n);
    expect(storeListener).not.toHaveBeenCalled();

    // 3. Direct non-publishing fetchHedgeSnapshot never mutates workspace store
    const privateSnap = await fetchHedgeSnapshot(client, { accountId: 'acct-a' });
    expect(privateSnap.snapshotSeq).toBe(55n);
    expect(store.getState().cursorSeq).toBe(50n);
    expect(storeListener).not.toHaveBeenCalled();

    // 4. Stale session returns status='stale' without throwing
    const staleOutcome = await executePreviewCoveredHedge(client, {
      sessionGeneration: gen - 1,
      getCurrentSessionGeneration: () => store.getState().sessionGeneration,
      snapshot: acctSnap,
      scope: PreviewHedgeScope.COVERED_SUBSET_ONLY,
      productCode: 'PROD1',
      contractCode: 'PROD1-2609',
      ctdDv01Per100: createDecimalValue('0.085', units.dv01Per100!),
      conversionFactor: createDecimalValue('1.01', units.dimensionless!),
      valuationAt,
      rulePackOptions: makeSyntheticRulePackOpts(),
    });
    expect(staleOutcome).toEqual({ status: 'stale' });
    unsub();
  });

  it('lookupCommittedAllocationsReadOnly distinguishes found, pruned_unrecoverable, and transient_error with 0 workspace cursor mutation', async () => {
    const store = createBlotterStore();
    const gen = store.getState().beginSession({ headerMode: 'authorization' });
    const boot = store.getState().activateReader(gen, 'bootstrap', 'bootstrapping')!;
    store.getState().commitBaselineSnapshots({
      ...boot,
      snapshots: new Map([['acct-a', makeValidSnapshot('acct-a', 50n)]]),
      cursorSeq: 50n,
      headSeq: 50n,
    });
    const listener = vi.fn();
    const unsub = store.subscribe(listener);

    const alloc = makeAllocation('01JNB0G1ALLOC6000000000001', '01JNB0G1PM6000000000000001');
    const pollFound = vi.fn(async () =>
      create(PollEventsResultSchema, {
        outcome: {
          case: 'batch',
          value: create(PollEventsBatchSchema, {
            events: [
              makeEnvelope(60n, [
                {
                  accountId: 'acct-a',
                  snapshot: makeValidSnapshot('acct-a', 60n),
                  allocations: [alloc],
                },
              ]),
            ],
            scannedThroughSeq: 60n,
            currentHeadSeq: 60n,
            hasMore: false,
          }),
        },
      }),
    );

    const found = await lookupCommittedAllocationsReadOnly(
      createMockClient({ pollEvents: pollFound }),
      {
        sessionGeneration: gen,
        getCurrentSessionGeneration: () => store.getState().sessionGeneration,
        accountId: 'acct-a',
        committedSeq: 60n,
        pendingMatchId: '01JNB0G1PM6000000000000001',
      },
    );
    expect(found.status).toBe('found');
    if (found.status === 'found') {
      expect(found.allocations).toHaveLength(1);
    }
    expect(store.getState().cursorSeq).toBe(50n);
    expect(listener).not.toHaveBeenCalled();

    const pollPruned = vi.fn(async () =>
      create(PollEventsResultSchema, {
        outcome: {
          case: 'snapshotFallback',
          value: create(SnapshotFallbackRequiredSchema, {
            requestedAfterSeq: 59n,
            oldestRetainedSeq: 70n,
            currentHeadSeq: 75n,
            reason: 'journal pruned',
          }),
        },
      }),
    );
    const pruned = await lookupCommittedAllocationsReadOnly(
      createMockClient({ pollEvents: pollPruned }),
      {
        sessionGeneration: gen,
        getCurrentSessionGeneration: () => store.getState().sessionGeneration,
        accountId: 'acct-a',
        committedSeq: 60n,
      },
    );
    expect(pruned.status).toBe('pruned_unrecoverable');
    expect(store.getState().cursorSeq).toBe(50n);
    expect(listener).not.toHaveBeenCalled();

    const pollTransient = vi.fn(async () => {
      throw new Error('network reset');
    });
    const transient = await lookupCommittedAllocationsReadOnly(
      createMockClient({ pollEvents: pollTransient }),
      {
        sessionGeneration: gen,
        getCurrentSessionGeneration: () => store.getState().sessionGeneration,
        accountId: 'acct-a',
        committedSeq: 60n,
      },
    );
    expect(transient.status).toBe('transient_error');
    expect(store.getState().cursorSeq).toBe(50n);
    expect(listener).not.toHaveBeenCalled();

    unsub();
  });
});

describe('Supervisor fail-closed protocol violations (M1-F01)', () => {
  async function setupLiveSingleAccount(options?: {
    readonly pollEvents?: DeskBlotterRpcClient['pollEvents'];
  }) {
    const store = createBlotterStore();
    const credStore = new InMemoryCredentialStore();
    const clock = new FakeClock();
    const streams: ControllableStream[] = [];

    const getAccountSnapshot = vi.fn(async (req: { accountId?: string }) =>
      create(GetAccountSnapshotResultSchema, {
        snapshot: makeValidSnapshot(req.accountId ?? 'acct-a', 100n, { accountVersion: 1n }),
      }),
    );
    const pollEvents = vi.fn(
      options?.pollEvents ??
        (async () =>
          create(PollEventsResultSchema, {
            outcome: {
              case: 'batch',
              value: create(PollEventsBatchSchema, {
                events: [],
                scannedThroughSeq: 100n,
                currentHeadSeq: 100n,
                hasMore: false,
              }),
            },
          })),
    );
    const subscribeEvents = vi.fn((_req: { afterSeq?: bigint }, opts?: { signal?: AbortSignal }) => {
      const s = new ControllableStream();
      s.attachSignal(opts?.signal);
      streams.push(s);
      return s;
    });

    const supervisor = new BlotterSupervisor({
      store,
      client: createMockClient({ getAccountSnapshot, pollEvents, subscribeEvents }),
      credentialStore: credStore,
      clock,
      location: { protocol: 'http:', hostname: '127.0.0.1' },
    });

    supervisor.login({ token: 'tok-f01', headerMode: 'authorization' });
    await supervisor.submitAccounts(['acct-a']);
    expect(store.getState().connectionState).toBe('stream_live');
    expect(streams).toHaveLength(1);

    return { store, clock, streams, supervisor, pollEvents, subscribeEvents };
  }

  it('fails closed on corrupt stream envelope, aborts stream, and never enters poll/reconnect after 35s+', async () => {
    const { store, clock, streams, supervisor, pollEvents, subscribeEvents } =
      await setupLiveSingleAccount();

    // Push corrupt envelope (A valid, B missing risk_summary)
    streams[0]?.push(
      create(SubscribeEventsResponseSchema, {
        payload: {
          case: 'event',
          value: makeEnvelope(101n, [
            {
              accountId: 'acct-a',
              snapshot: makeValidSnapshot('acct-a', 101n, { accountVersion: 9n }),
            },
            {
              accountId: 'acct-b',
              snapshot: makeValidSnapshot('acct-b', 101n, { omitRiskSummary: true }),
            },
          ]),
        },
      }),
    );
    await flushMicrotasks();

    expect(store.getState().connectionState).toBe('error');
    expect(store.getState().lastError?.kind).toBe('protocol');
    expect(store.getState().cursorSeq).toBe(100n);
    expect(store.getState().snapshots['acct-a']?.accountVersion).toBe(1n);
    expect(streams[0]?.aborted).toBe(true);

    // Advance past 35s watchdog and 10s reconnect -> must stay in 'error' with 0 polls / 0 reconnects
    clock.advance(40_000);
    await flushMicrotasks();
    expect(store.getState().connectionState).toBe('error');
    expect(pollEvents).not.toHaveBeenCalled();
    expect(subscribeEvents).toHaveBeenCalledTimes(1);

    supervisor.dispose();
  });

  it('fails closed on duplicate and regressing stream seq (non_increasing_seq), aborts stream, and stays in error', async () => {
    for (const badSeq of [100n, 99n]) {
      const { store, clock, streams, supervisor, pollEvents, subscribeEvents } =
        await setupLiveSingleAccount();

      streams[0]?.push(
        create(SubscribeEventsResponseSchema, {
          payload: {
            case: 'event',
            value: makeEnvelope(badSeq, [
              {
                accountId: 'acct-a',
                snapshot: makeValidSnapshot('acct-a', badSeq, { accountVersion: 2n }),
              },
            ]),
          },
        }),
      );
      await flushMicrotasks();

      expect(store.getState().connectionState).toBe('error');
      expect(store.getState().lastError?.kind).toBe('protocol');
      expect(store.getState().cursorSeq).toBe(100n);
      expect(streams[0]?.aborted).toBe(true);

      clock.advance(40_000);
      await flushMicrotasks();
      expect(store.getState().connectionState).toBe('error');
      expect(pollEvents).not.toHaveBeenCalled();
      expect(subscribeEvents).toHaveBeenCalledTimes(1);

      supervisor.dispose();
    }
  });

  it('fails closed on out-of-range heartbeat and protocol error thrown by SubscribeEvents stream', async () => {
    // 1. Out-of-range heartbeat (scanned_through_seq > head_seq)
    const hbCase = await setupLiveSingleAccount();
    hbCase.streams[0]?.push(
      create(SubscribeEventsResponseSchema, {
        payload: {
          case: 'heartbeat',
          value: create(StreamHeartbeatSchema, {
            headSeq: 100n,
            scannedThroughSeq: 105n,
          }),
        },
      }),
    );
    await flushMicrotasks();

    expect(hbCase.store.getState().connectionState).toBe('error');
    expect(hbCase.store.getState().lastError?.kind).toBe('protocol');
    expect(hbCase.streams[0]?.aborted).toBe(true);
    hbCase.clock.advance(40_000);
    await flushMicrotasks();
    expect(hbCase.store.getState().connectionState).toBe('error');
    expect(hbCase.pollEvents).not.toHaveBeenCalled();
    expect(hbCase.subscribeEvents).toHaveBeenCalledTimes(1);
    hbCase.supervisor.dispose();

    // 2. Protocol error thrown from SubscribeEvents iterator (e.g., empty payload oneof)
    const protoErrCase = await setupLiveSingleAccount();
    protoErrCase.streams[0]?.fail(
      BlotterClientError.protocol('SubscribeEvents frame has empty payload oneof.'),
    );
    await flushMicrotasks();

    expect(protoErrCase.store.getState().connectionState).toBe('error');
    expect(protoErrCase.store.getState().lastError?.kind).toBe('protocol');
    expect(protoErrCase.streams[0]?.aborted).toBe(true);
    protoErrCase.clock.advance(40_000);
    await flushMicrotasks();
    expect(protoErrCase.store.getState().connectionState).toBe('error');
    expect(protoErrCase.pollEvents).not.toHaveBeenCalled();
    expect(protoErrCase.subscribeEvents).toHaveBeenCalledTimes(1);
    protoErrCase.supervisor.dispose();
  });

  it('fails closed on corrupt PollEvents batch in poll_degraded, cancels poll/reconnect timers, and stays in error', async () => {
    const { store, clock, streams, supervisor, pollEvents, subscribeEvents } =
      await setupLiveSingleAccount({
        pollEvents: async () =>
          create(PollEventsResultSchema, {
            outcome: {
              case: 'batch',
              value: create(PollEventsBatchSchema, {
                events: [
                  makeEnvelope(101n, [
                    {
                      accountId: 'acct-a',
                      snapshot: makeValidSnapshot('acct-a', 101n, { accountVersion: 2n }),
                    },
                    {
                      accountId: 'acct-b',
                      snapshot: makeValidSnapshot('acct-b', 101n, {
                        coveredTaxBasis: BondTaxBasis.TAX_ADJUSTED,
                      }),
                    },
                  ]),
                ],
                scannedThroughSeq: 101n,
                currentHeadSeq: 101n,
                hasMore: false,
              }),
            },
          }),
      });

    // Trigger clean stream close -> enters poll_degraded -> runs first poll cycle -> gets corrupt batch
    streams[0]?.end();
    await flushMicrotasks();

    expect(pollEvents).toHaveBeenCalledTimes(1);
    expect(store.getState().connectionState).toBe('error');
    expect(store.getState().lastError?.kind).toBe('protocol');
    expect(store.getState().cursorSeq).toBe(100n);
    expect(store.getState().snapshots['acct-a']?.accountVersion).toBe(1n);

    // Advance past poll (2s), reconnect (10s), and watchdog (35s) intervals -> zero additional polls or reconnects!
    clock.advance(40_000);
    await flushMicrotasks();
    expect(store.getState().connectionState).toBe('error');
    expect(pollEvents).toHaveBeenCalledTimes(1);
    expect(subscribeEvents).toHaveBeenCalledTimes(1);

    supervisor.dispose();
  });
});

describe('Multi-account cold-start initialBarrier drop recovery (M1-F02)', () => {
  for (const dropMode of ['clean_close', 'transient_error', 'watchdog_timeout'] as const) {
    it(`recovers both acct-active and acct-idle via runRecoveryBarrier on initialBarrier ${dropMode} before targetMaxSeq`, async () => {
      const store = createBlotterStore();
      const credStore = new InMemoryCredentialStore();
      const clock = new FakeClock();
      const streams: ControllableStream[] = [];
      const pollAfterSeqs: bigint[] = [];
      const subAfterSeqs: bigint[] = [];

      let snapshotRound = 0;
      const getAccountSnapshot = vi.fn(async (req: { accountId?: string }) => {
        if (req.accountId === 'acct-active') {
          snapshotRound += 1;
          // Round 1 (cold-start): acct-active@100, acct-idle@102 (diff seqs -> initialBarrier)
          // Round 2 (recovery barrier): acct-active@102, acct-idle@103
          return create(GetAccountSnapshotResultSchema, {
            snapshot: makeValidSnapshot(
              'acct-active',
              snapshotRound === 1 ? 100n : 102n,
              { accountVersion: snapshotRound === 1 ? 1n : 2n },
            ),
          });
        }
        return create(GetAccountSnapshotResultSchema, {
          snapshot: makeValidSnapshot(
            'acct-idle',
            snapshotRound === 1 ? 102n : 103n,
            { accountVersion: 1n },
          ),
        });
      });

      const pollEvents = vi.fn(async (req: { afterSeq?: bigint }) => {
        pollAfterSeqs.push(req.afterSeq ?? 0n);
        // Recovery replay from minSeq=102n -> maxSeq=103n (acct-idle has zero journal events!)
        return create(PollEventsResultSchema, {
          outcome: {
            case: 'batch',
            value: create(PollEventsBatchSchema, {
              events: [
                makeEnvelope(103n, [
                  {
                    accountId: 'acct-active',
                    snapshot: makeValidSnapshot('acct-active', 103n, { accountVersion: 3n }),
                  },
                ]),
              ],
              scannedThroughSeq: 103n,
              currentHeadSeq: 103n,
              hasMore: false,
            }),
          },
        });
      });

      const subscribeEvents = vi.fn(
        (req: { afterSeq?: bigint }, opts?: { signal?: AbortSignal }) => {
          subAfterSeqs.push(req.afterSeq ?? 0n);
          const s = new ControllableStream();
          s.attachSignal(opts?.signal);
          streams.push(s);
          return s;
        },
      );

      const supervisor = new BlotterSupervisor({
        store,
        client: createMockClient({ getAccountSnapshot, pollEvents, subscribeEvents }),
        credentialStore: credStore,
        clock,
        location: { protocol: 'http:', hostname: '127.0.0.1' },
      });

      supervisor.login({ token: 'tok-f02', headerMode: 'authorization' });
      const res = await supervisor.submitAccounts(['acct-active', 'acct-idle']);
      expect(res.ok).toBe(true);
      // Initial SubscribeEvents opened at min(100, 102) = 100n in bootstrapping barrier mode
      expect(store.getState().connectionState).toBe('bootstrapping');
      expect(subAfterSeqs).toEqual([100n]);
      expect(store.getState().snapshots).toEqual({});

      // Drop initialBarrier stream BEFORE reaching 102n
      if (dropMode === 'clean_close') {
        streams[0]?.end();
      } else if (dropMode === 'transient_error') {
        streams[0]?.fail(new Error('transient stream disconnect'));
      } else {
        clock.advance(35_000);
      }
      await flushMicrotasks(16);

      // Must NOT have polled from after_seq=0n; must have run recovery barrier from minSeq=102n
      expect(pollAfterSeqs).toEqual([102n]);
      expect(store.getState().connectionState).toBe('stream_live');
      expect(store.getState().cursorSeq).toBe(103n);
      // BOTH acct-active AND idle acct-idle are present and consistent!
      expect(store.getState().snapshots['acct-active']?.snapshotSeq).toBe(103n);
      expect(store.getState().snapshots['acct-active']?.accountVersion).toBe(3n);
      expect(store.getState().snapshots['acct-idle']?.snapshotSeq).toBe(103n);
      expect(store.getState().snapshots['acct-idle']?.accountVersion).toBe(1n);
      expect(subAfterSeqs).toEqual([100n, 103n]);

      supervisor.dispose();
    });
  }
});
