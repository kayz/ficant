import { create } from '@bufbuild/protobuf';
import type {
  AccountBlotterSnapshot,
  BlotterEventEnvelope,
  ReconcileAllocationView,
  SnapshotFallbackRequired,
} from '../generated/ficant/blotter/v1/model_pb';
import { PreviewHedgeScope } from '../generated/ficant/blotter/v1/model_pb';
import {
  PreviewCoveredHedgeRequestSchema,
  type PreviewCoveredHedgeResult,
} from '../generated/ficant/blotter/v1/service_pb';
import type { DecimalValue, MarketTime } from '../generated/ficant/core/v1/common_pb';
import { validateCanonicalDecimalValue } from '../codec/decimal';
import { isNonEmptyTrimmed, maxUint64, minUint64, validateMarketTime } from '../codec/primitives';
import {
  loadTrustedDeliveryRulePack,
  type LoadTrustedRulePackOptions,
} from '../codec/rulepack';
import {
  ClientValidationError,
  validatePreviewCoveredHedgeResult,
} from '../codec/validators';
import type {
  InMemoryCredentialInput,
  InMemoryCredentialStore,
  LocationLike,
} from '../transport/auth';
import type { DeskBlotterRpcClient } from '../transport/client';
import { BlotterClientError } from '../transport/error';
import { WriteIntentManager } from '../transport/idempotency';
import type { BlotterStoreApi, ReaderActivationResult } from './blotterStore';
import {
  mergeDeduplicatedAllocations,
  stageAndValidateEnvelope,
  stageAndValidatePollBatch,
  validateAccountSnapshot,
  validateStreamHeartbeat,
  type IndexedAllocation,
} from './envelopeValidator';

export interface ClockAdapter {
  readonly now: () => number;
  readonly setTimeout: (callback: () => void, ms: number) => ReturnType<typeof globalThis.setTimeout>;
  readonly clearTimeout: (handle: ReturnType<typeof globalThis.setTimeout>) => void;
}

export const defaultClockAdapter: ClockAdapter = {
  now: () => Date.now(),
  setTimeout: (cb, ms) => globalThis.setTimeout(cb, ms),
  clearTimeout: (handle) => globalThis.clearTimeout(handle),
};

export const DEFAULT_WATCHDOG_TIMEOUT_MS = 35_000;
export const DEFAULT_POLL_INTERVAL_MS = 2_000;
export const DEFAULT_RECONNECT_INTERVAL_MS = 10_000;
export const DEFAULT_POLL_MAX_EVENTS = 100;
export const DEFAULT_MAX_RECOVERY_ATTEMPTS = 4;
export const DEFAULT_RECOVERY_BACKOFF_BASE_MS = 250;
export const DEFAULT_RECOVERY_BACKOFF_MAX_MS = 2_000;

export interface BlotterSupervisorOptions {
  readonly store: BlotterStoreApi;
  readonly client: DeskBlotterRpcClient;
  readonly credentialStore: InMemoryCredentialStore;
  readonly writeIntents?: WriteIntentManager | undefined;
  readonly clock?: ClockAdapter | undefined;
  readonly location?: LocationLike | undefined;
  readonly watchdogTimeoutMs?: number | undefined;
  readonly pollIntervalMs?: number | undefined;
  readonly reconnectIntervalMs?: number | undefined;
  readonly pollMaxEvents?: number | undefined;
  readonly maxRecoveryAttempts?: number | undefined;
  readonly recoveryBackoffBaseMs?: number | undefined;
  readonly recoveryBackoffMaxMs?: number | undefined;
}

function sleepWithAbort(
  ms: number,
  clock: ClockAdapter,
  signal: AbortSignal,
): Promise<boolean> {
  if (signal.aborted) {
    return Promise.resolve(false);
  }
  if (ms <= 0) {
    return Promise.resolve(true);
  }
  return new Promise<boolean>((resolve) => {
    const onAbort = (): void => {
      clock.clearTimeout(timer);
      resolve(false);
    };
    const timer = clock.setTimeout(() => {
      signal.removeEventListener('abort', onAbort);
      resolve(true);
    }, ms);
    signal.addEventListener('abort', onAbort, { once: true });
  });
}

function dedupeIds(ids: readonly string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of ids) {
    const trimmed = raw.trim();
    if (trimmed.length > 0 && !seen.has(trimmed)) {
      seen.add(trimmed);
      out.push(trimmed);
    }
  }
  return out;
}

function mergeEnvelopeIntoRecoveryStaging(
  stagedSnapshots: Map<string, AccountBlotterSnapshot>,
  stagedAllocations: Map<string, readonly IndexedAllocation[]>,
  baselineSeqs: ReadonlyMap<string, bigint>,
  envelope: BlotterEventEnvelope,
  source: 'subscribe' | 'recovery',
): bigint {
  // Always validate EVERY AccountChange in the envelope (including accounts whose baseline
  // already covers envelope.seq) BEFORE applying any partial update.
  const validated = stageAndValidateEnvelope(envelope, source);

  for (const [acctId, snap] of validated.snapshots.entries()) {
    const baseSeq = baselineSeqs.get(acctId);
    // Skip per-account covered replacements (env.seq <= s[a]), while still admitting
    // newly discovered authorized accounts (baseSeq === undefined) and newer updates.
    if (baseSeq === undefined || validated.seq > baseSeq) {
      stagedSnapshots.set(acctId, snap);
    }
  }
  for (const [acctId, allocs] of validated.allocations.entries()) {
    if (allocs.length > 0) {
      const existing = stagedAllocations.get(acctId) ?? [];
      stagedAllocations.set(acctId, mergeDeduplicatedAllocations(existing, allocs));
    }
  }
  return validated.seq;
}

export class BlotterSupervisor {
  readonly #store: BlotterStoreApi;
  readonly #client: DeskBlotterRpcClient;
  readonly #credentialStore: InMemoryCredentialStore;
  readonly #writeIntents: WriteIntentManager;
  readonly #clock: ClockAdapter;
  readonly #location: LocationLike | undefined;
  readonly #watchdogTimeoutMs: number;
  readonly #pollIntervalMs: number;
  readonly #reconnectIntervalMs: number;
  readonly #pollMaxEvents: number;
  readonly #maxRecoveryAttempts: number;
  readonly #recoveryBackoffBaseMs: number;
  readonly #recoveryBackoffMaxMs: number;

  #activeAbort: AbortController | null = null;
  #pollTimer: ReturnType<typeof globalThis.setTimeout> | null = null;
  #reconnectTimer: ReturnType<typeof globalThis.setTimeout> | null = null;
  #watchdogTimer: ReturnType<typeof globalThis.setTimeout> | null = null;

  constructor(options: BlotterSupervisorOptions) {
    this.#store = options.store;
    this.#client = options.client;
    this.#credentialStore = options.credentialStore;
    this.#writeIntents = options.writeIntents ?? new WriteIntentManager();
    this.#clock = options.clock ?? defaultClockAdapter;
    this.#location = options.location;
    this.#watchdogTimeoutMs = options.watchdogTimeoutMs ?? DEFAULT_WATCHDOG_TIMEOUT_MS;
    this.#pollIntervalMs = options.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;
    this.#reconnectIntervalMs = options.reconnectIntervalMs ?? DEFAULT_RECONNECT_INTERVAL_MS;
    this.#pollMaxEvents = options.pollMaxEvents ?? DEFAULT_POLL_MAX_EVENTS;
    this.#maxRecoveryAttempts = options.maxRecoveryAttempts ?? DEFAULT_MAX_RECOVERY_ATTEMPTS;
    this.#recoveryBackoffBaseMs =
      options.recoveryBackoffBaseMs ?? DEFAULT_RECOVERY_BACKOFF_BASE_MS;
    this.#recoveryBackoffMaxMs =
      options.recoveryBackoffMaxMs ?? DEFAULT_RECOVERY_BACKOFF_MAX_MS;
  }

  get writeIntents(): WriteIntentManager {
    return this.#writeIntents;
  }

  #cancelAllActivity(): void {
    if (this.#activeAbort !== null) {
      this.#activeAbort.abort();
      this.#activeAbort = null;
    }
    this.#clearTimers();
  }

  #clearTimers(): void {
    if (this.#pollTimer !== null) {
      this.#clock.clearTimeout(this.#pollTimer);
      this.#pollTimer = null;
    }
    if (this.#reconnectTimer !== null) {
      this.#clock.clearTimeout(this.#reconnectTimer);
      this.#reconnectTimer = null;
    }
    if (this.#watchdogTimer !== null) {
      this.#clock.clearTimeout(this.#watchdogTimer);
      this.#watchdogTimer = null;
    }
  }

  #isFenceCurrent(fence: ReaderActivationResult): boolean {
    const s = this.#store.getState();
    return (
      s.sessionGeneration === fence.sessionGeneration &&
      s.transportEpoch === fence.transportEpoch &&
      s.activeReaderId === fence.readerId
    );
  }

  #handlePossibleAuthFailure(sessionGeneration: number, error: BlotterClientError): boolean {
    if (!error.isUnauthenticated()) {
      return false;
    }
    if (this.#store.getState().sessionGeneration !== sessionGeneration) {
      return true;
    }
    this.#cancelAllActivity();
    this.#credentialStore.clear();
    this.#writeIntents.clearAll();
    this.#store.getState().handleUnauthenticated(sessionGeneration, error);
    return true;
  }

  /**
   * Authenticates or switches credentials.
   * Synchronously cancels in-flight streams/polls, clears pending write intents, wipes store state,
   * increments `sessionGeneration`, and stays idle (0 event RPCs) until explicit accounts are submitted.
   */
  login(input: InMemoryCredentialInput): number {
    this.#cancelAllActivity();
    this.#writeIntents.clearAll();
    const meta = this.#credentialStore.setCredential(input, this.#location);
    return this.#store.getState().beginSession({
      headerMode: meta.headerMode,
      roleHint: meta.roleHint,
      actorLabel: meta.actorLabel,
    });
  }

  logout(): number {
    this.#cancelAllActivity();
    this.#credentialStore.clear();
    this.#writeIntents.clearAll();
    return this.#store.getState().logout();
  }

  dispose(): void {
    this.#cancelAllActivity();
  }

  /**
   * Cold-start explicit account submission.
   * - Empty account list -> cancels readers, stays in `idle_no_accounts`, issues 0 RPCs.
   * - Non-empty list -> fetches `GetAccountSnapshot` baseline for each account FIRST,
   *   then opens `SubscribeEvents(after_seq = min(snapshot_seq))`.
   */
  async submitAccounts(accountIds: readonly string[]): Promise<{
    readonly ok: boolean;
    readonly failedAccounts: Readonly<Record<string, BlotterClientError>>;
  }> {
    const sessionGen = this.#store.getState().sessionGeneration;
    if (!this.#store.getState().authenticated) {
      return { ok: false, failedAccounts: {} };
    }

    this.#cancelAllActivity();
    const normalized = this.#store.getState().setRequestedAccounts(accountIds, sessionGen);
    if (normalized.length === 0) {
      return { ok: true, failedAccounts: {} };
    }

    const fence = this.#store
      .getState()
      .activateReader(sessionGen, 'bootstrap', 'bootstrapping');
    if (!fence) {
      return { ok: false, failedAccounts: {} };
    }

    const abortController = new AbortController();
    this.#activeAbort = abortController;

    const baselines = new Map<string, AccountBlotterSnapshot>();
    const baselineSeqs = new Map<string, bigint>();
    const failedAccounts: Record<string, BlotterClientError> = {};

    for (const accountId of normalized) {
      if (!this.#isFenceCurrent(fence) || abortController.signal.aborted) {
        return { ok: false, failedAccounts };
      }
      try {
        const res = await this.#client.getAccountSnapshot(
          { accountId },
          { signal: abortController.signal },
        );
        if (!this.#isFenceCurrent(fence)) {
          return { ok: false, failedAccounts };
        }
        const snap = validateAccountSnapshot(res.snapshot, 'GetAccountSnapshot', accountId);
        baselines.set(accountId, snap);
        baselineSeqs.set(accountId, snap.snapshotSeq);
        this.#store.getState().setAccountLoadError(sessionGen, accountId, null);
      } catch (err) {
        const clientErr = BlotterClientError.fromUnknown(
          err,
          this.#credentialStore.getRedactedSecrets(),
        );
        if (this.#handlePossibleAuthFailure(sessionGen, clientErr)) {
          return { ok: false, failedAccounts };
        }
        if (!this.#isFenceCurrent(fence)) {
          return { ok: false, failedAccounts };
        }
        failedAccounts[accountId] = clientErr;
        this.#store.getState().setAccountLoadError(sessionGen, accountId, clientErr);
      }
    }

    if (Object.keys(failedAccounts).length > 0) {
      if (this.#isFenceCurrent(fence)) {
        const firstErr = Object.values(failedAccounts)[0];
        if (firstErr) {
          this.#store.getState().reportTransportError({
            sessionGeneration: fence.sessionGeneration,
            transportEpoch: fence.transportEpoch,
            readerId: fence.readerId,
            error: firstErr,
            nextConnectionState: 'error',
          });
        }
      }
      return { ok: false, failedAccounts };
    }

    const seqList = Array.from(baselineSeqs.values());
    const minSeq = minUint64(seqList);
    const maxSeq = maxUint64(seqList);

    if (minSeq === maxSeq) {
      const outcome = this.#store.getState().commitBaselineSnapshots({
        sessionGeneration: fence.sessionGeneration,
        transportEpoch: fence.transportEpoch,
        readerId: fence.readerId,
        snapshots: baselines,
        cursorSeq: minSeq,
        headSeq: maxSeq,
        nextConnectionState: 'stream_live',
        mergeExistingAllocations: false,
      });
      if (!outcome.committed) {
        return { ok: false, failedAccounts };
      }
      this.#startLiveStream(sessionGen, minSeq);
      return { ok: true, failedAccounts: {} };
    }

    // Multi-account differing baseline seqs (e.g. A@100, B@102): open first SubscribeEvents at
    // after_seq = min(snapshot_seq) in staging barrier mode until >= max(snapshot_seq).
    this.#startLiveStream(sessionGen, minSeq, {
      stagedSnapshots: baselines,
      stagedAllocations: new Map(),
      baselineSeqs,
      targetMaxSeq: maxSeq,
      mergeExistingAllocations: false,
    });
    return { ok: true, failedAccounts: {} };
  }

  #startLiveStream(
    sessionGeneration: number,
    afterSeq: bigint,
    initialBarrier?: {
      readonly stagedSnapshots: Map<string, AccountBlotterSnapshot>;
      readonly stagedAllocations: Map<string, readonly IndexedAllocation[]>;
      readonly baselineSeqs: ReadonlyMap<string, bigint>;
      readonly targetMaxSeq: bigint;
      readonly mergeExistingAllocations: boolean;
    },
  ): void {
    this.#cancelAllActivity();
    const fence = this.#store
      .getState()
      .activateReader(
        sessionGeneration,
        'stream',
        initialBarrier ? 'bootstrapping' : 'stream_live',
      );
    if (!fence) {
      return;
    }

    const abortController = new AbortController();
    this.#activeAbort = abortController;
    let barrier = initialBarrier;
    let barrierCursor = afterSeq;

    const resetWatchdog = (): void => {
      if (this.#watchdogTimer !== null) {
        this.#clock.clearTimeout(this.#watchdogTimer);
      }
      this.#watchdogTimer = this.#clock.setTimeout(() => {
        if (!this.#isFenceCurrent(fence)) {
          return;
        }
        abortController.abort();
        if (barrier !== undefined) {
          void this.runRecoveryBarrier(sessionGeneration);
          return;
        }
        this.#enterDegradedPoll(sessionGeneration);
      }, this.#watchdogTimeoutMs);
    };

    resetWatchdog();

    void (async () => {
      try {
        const stream = this.#client.subscribeEvents(
          { afterSeq },
          { signal: abortController.signal },
        );

        for await (const frame of stream) {
          if (!this.#isFenceCurrent(fence) || abortController.signal.aborted) {
            return;
          }
          resetWatchdog();

          if (frame.payload.case === 'snapshotFallback') {
            this.#clearTimers();
            void this.runRecoveryBarrier(sessionGeneration, frame.payload.value);
            return;
          }

          if (frame.payload.case === 'event') {
            const env = frame.payload.value;
            if (barrier) {
              if (env.seq <= barrierCursor) {
                throw BlotterClientError.protocol(
                  `Bootstrap stream non-increasing seq=${env.seq.toString()} <= ${barrierCursor.toString()}`,
                );
              }
              const nextSeq = mergeEnvelopeIntoRecoveryStaging(
                barrier.stagedSnapshots,
                barrier.stagedAllocations,
                barrier.baselineSeqs,
                env,
                'subscribe',
              );
              barrierCursor = nextSeq;
              if (barrierCursor >= barrier.targetMaxSeq) {
                const commit = this.#store.getState().commitBaselineSnapshots({
                  sessionGeneration: fence.sessionGeneration,
                  transportEpoch: fence.transportEpoch,
                  readerId: fence.readerId,
                  snapshots: barrier.stagedSnapshots,
                  allocations: barrier.stagedAllocations,
                  cursorSeq: barrierCursor,
                  headSeq: barrierCursor,
                  nextConnectionState: 'stream_live',
                  mergeExistingAllocations: barrier.mergeExistingAllocations,
                });
                if (!commit.committed) {
                  if (commit.reason === 'validation_error') {
                    this.#store.getState().reportTransportError({
                      sessionGeneration: fence.sessionGeneration,
                      transportEpoch: fence.transportEpoch,
                      readerId: fence.readerId,
                      error:
                        commit.error ??
                        BlotterClientError.protocol('Bootstrap baseline commit validation failed'),
                      nextConnectionState: 'error',
                    });
                    this.#cancelAllActivity();
                  }
                  return;
                }
                barrier = undefined;
              }
            } else {
              const commit = this.#store.getState().commitStreamEnvelope({
                sessionGeneration: fence.sessionGeneration,
                transportEpoch: fence.transportEpoch,
                readerId: fence.readerId,
                envelope: env,
              });
              if (
                !commit.committed &&
                (commit.reason === 'validation_error' || commit.reason === 'non_increasing_seq')
              ) {
                const defaultMsg =
                  commit.reason === 'non_increasing_seq'
                    ? `SubscribeEvents non-increasing seq=${env.seq.toString()} <= cursor=${this.#store.getState().cursorSeq.toString()}`
                    : 'SubscribeEvents envelope validation failed';
                this.#store.getState().reportTransportError({
                  sessionGeneration: fence.sessionGeneration,
                  transportEpoch: fence.transportEpoch,
                  readerId: fence.readerId,
                  error: commit.error ?? BlotterClientError.protocol(defaultMsg),
                  nextConnectionState: 'error',
                });
                this.#cancelAllActivity();
                return;
              }
            }
          } else if (frame.payload.case === 'heartbeat') {
            const hb = frame.payload.value;
            if (barrier) {
              const validated = validateStreamHeartbeat(hb, barrierCursor);
              barrierCursor = validated.nextCursorSeq;
              if (barrierCursor >= barrier.targetMaxSeq) {
                const commit = this.#store.getState().commitBaselineSnapshots({
                  sessionGeneration: fence.sessionGeneration,
                  transportEpoch: fence.transportEpoch,
                  readerId: fence.readerId,
                  snapshots: barrier.stagedSnapshots,
                  allocations: barrier.stagedAllocations,
                  cursorSeq: barrierCursor,
                  headSeq: validated.headSeq,
                  nextConnectionState: 'stream_live',
                  mergeExistingAllocations: barrier.mergeExistingAllocations,
                });
                if (!commit.committed) {
                  if (commit.reason === 'validation_error') {
                    this.#store.getState().reportTransportError({
                      sessionGeneration: fence.sessionGeneration,
                      transportEpoch: fence.transportEpoch,
                      readerId: fence.readerId,
                      error:
                        commit.error ??
                        BlotterClientError.protocol('Bootstrap baseline commit validation failed'),
                      nextConnectionState: 'error',
                    });
                    this.#cancelAllActivity();
                  }
                  return;
                }
                barrier = undefined;
              }
            } else {
              const commit = this.#store.getState().commitStreamHeartbeat({
                sessionGeneration: fence.sessionGeneration,
                transportEpoch: fence.transportEpoch,
                readerId: fence.readerId,
                heartbeat: hb,
              });
              if (
                !commit.committed &&
                (commit.reason === 'validation_error' || commit.reason === 'non_increasing_seq')
              ) {
                this.#store.getState().reportTransportError({
                  sessionGeneration: fence.sessionGeneration,
                  transportEpoch: fence.transportEpoch,
                  readerId: fence.readerId,
                  error:
                    commit.error ??
                    BlotterClientError.protocol('SubscribeEvents heartbeat validation failed'),
                  nextConnectionState: 'error',
                });
                this.#cancelAllActivity();
                return;
              }
            }
          } else {
            this.#store.getState().reportTransportError({
              sessionGeneration: fence.sessionGeneration,
              transportEpoch: fence.transportEpoch,
              readerId: fence.readerId,
              error: BlotterClientError.protocol('SubscribeEvents frame has unexpected payload.'),
              nextConnectionState: 'error',
            });
            this.#cancelAllActivity();
            return;
          }
        }

        // Stream closed cleanly without abort: recover if barrier incomplete, otherwise degrade to poll.
        if (this.#isFenceCurrent(fence) && !abortController.signal.aborted) {
          if (barrier !== undefined) {
            void this.runRecoveryBarrier(sessionGeneration);
          } else {
            this.#enterDegradedPoll(sessionGeneration);
          }
        }
      } catch (err) {
        const clientErr = BlotterClientError.fromUnknown(
          err,
          this.#credentialStore.getRedactedSecrets(),
        );
        if (this.#handlePossibleAuthFailure(sessionGeneration, clientErr)) {
          return;
        }
        if (!this.#isFenceCurrent(fence) || clientErr.isAborted()) {
          return;
        }
        if (clientErr.kind === 'protocol') {
          this.#store.getState().reportTransportError({
            sessionGeneration: fence.sessionGeneration,
            transportEpoch: fence.transportEpoch,
            readerId: fence.readerId,
            error: clientErr,
            nextConnectionState: 'error',
          });
          this.#cancelAllActivity();
          return;
        }
        this.#store.getState().reportTransportError({
          sessionGeneration: fence.sessionGeneration,
          transportEpoch: fence.transportEpoch,
          readerId: fence.readerId,
          error: clientErr,
        });
        if (barrier !== undefined) {
          void this.runRecoveryBarrier(sessionGeneration);
          return;
        }
        this.#enterDegradedPoll(sessionGeneration);
      }
    })();
  }

  #enterDegradedPoll(sessionGeneration: number): void {
    this.#cancelAllActivity();
    const fence = this.#store
      .getState()
      .activateReader(sessionGeneration, 'poll', 'poll_degraded');
    if (!fence) {
      return;
    }

    const abortController = new AbortController();
    this.#activeAbort = abortController;

    const scheduleReconnect = (): void => {
      if (this.#reconnectTimer !== null) {
        this.#clock.clearTimeout(this.#reconnectTimer);
      }
      this.#reconnectTimer = this.#clock.setTimeout(() => {
        if (!this.#isFenceCurrent(fence)) {
          return;
        }
        this.#startLiveStream(sessionGeneration, this.#store.getState().cursorSeq);
      }, this.#reconnectIntervalMs);
    };

    const scheduleNextPoll = (): void => {
      if (this.#pollTimer !== null) {
        this.#clock.clearTimeout(this.#pollTimer);
      }
      this.#pollTimer = this.#clock.setTimeout(() => {
        void runPollCycle();
      }, this.#pollIntervalMs);
    };

    const runPollCycle = async (): Promise<void> => {
      if (!this.#isFenceCurrent(fence) || abortController.signal.aborted) {
        return;
      }
      const reqAfterSeq = this.#store.getState().cursorSeq;
      try {
        const res = await this.#client.pollEvents(
          { afterSeq: reqAfterSeq, maxEvents: this.#pollMaxEvents },
          { signal: abortController.signal },
        );
        if (!this.#isFenceCurrent(fence) || abortController.signal.aborted) {
          return;
        }
        if (res.outcome.case === 'snapshotFallback') {
          this.#clearTimers();
          void this.runRecoveryBarrier(sessionGeneration, res.outcome.value);
          return;
        }
        if (res.outcome.case === 'batch') {
          const commit = this.#store.getState().commitPollBatch({
            sessionGeneration: fence.sessionGeneration,
            transportEpoch: fence.transportEpoch,
            readerId: fence.readerId,
            requestAfterSeq: reqAfterSeq,
            batch: res.outcome.value,
          });
          if (
            !commit.committed &&
            (commit.reason === 'validation_error' || commit.reason === 'non_increasing_seq')
          ) {
            this.#store.getState().reportTransportError({
              sessionGeneration: fence.sessionGeneration,
              transportEpoch: fence.transportEpoch,
              readerId: fence.readerId,
              error:
                commit.error ?? BlotterClientError.protocol('PollEvents batch validation failed'),
              nextConnectionState: 'error',
            });
            this.#cancelAllActivity();
            return;
          }
          if (commit.committed && res.outcome.value.hasMore) {
            void runPollCycle();
            return;
          }
        } else if (res.outcome.case === undefined) {
          this.#store.getState().reportTransportError({
            sessionGeneration: fence.sessionGeneration,
            transportEpoch: fence.transportEpoch,
            readerId: fence.readerId,
            error: BlotterClientError.protocol('PollEvents returned empty outcome.'),
            nextConnectionState: 'error',
          });
          this.#cancelAllActivity();
          return;
        }
        scheduleNextPoll();
      } catch (err) {
        const clientErr = BlotterClientError.fromUnknown(
          err,
          this.#credentialStore.getRedactedSecrets(),
        );
        if (this.#handlePossibleAuthFailure(sessionGeneration, clientErr)) {
          return;
        }
        if (!this.#isFenceCurrent(fence) || clientErr.isAborted()) {
          return;
        }
        if (clientErr.kind === 'protocol') {
          this.#store.getState().reportTransportError({
            sessionGeneration: fence.sessionGeneration,
            transportEpoch: fence.transportEpoch,
            readerId: fence.readerId,
            error: clientErr,
            nextConnectionState: 'error',
          });
          this.#cancelAllActivity();
          return;
        }
        this.#store.getState().reportTransportError({
          sessionGeneration: fence.sessionGeneration,
          transportEpoch: fence.transportEpoch,
          readerId: fence.readerId,
          error: clientErr,
        });
        scheduleNextPoll();
      }
    };

    scheduleReconnect();
    void runPollCycle();
  }

  /**
   * Executes one explicit `PollEvents` call under the current active poll reader (or activates one
   * if in degraded poll) to catch up after a write receipt reports `committed_seq > cursorSeq`.
   */
  triggerCatchUp(): void {
    const s = this.#store.getState();
    if (!s.authenticated || s.connectionState !== 'poll_degraded') {
      return;
    }
    this.#enterDegradedPoll(s.sessionGeneration);
  }

  /**
   * Two-phase multi-account recovery barrier:
   * - Keeps the last complete workspace view on screen and pauses version-dependent writes.
   * - Phase 1: Fetches `GetAccountSnapshot` baseline `s[a]` for every tracked account.
   * - Phase 2: Replays `PollEvents` from `min(s[a])` to `>= max(s[a])`, validating EVERY
   *   envelope/change (even for covered accounts), skipping stale per-account snapshot replacements
   *   (`env.seq <= s[a]`), admitting newly discovered authorized accounts, and deduplicating
   *   `changed_allocations` by `(seq, allocation_id)`.
   * - If re-pruned during recovery (`SnapshotFallbackRequired`), discards all staged recovery
   *   state, backs off with a bounded retry count, and restarts from Phase 1.
   * - Publishes atomically in ONE commit before resuming `SubscribeEvents`.
   */
  async runRecoveryBarrier(
    sessionGeneration: number,
    fallback?: SnapshotFallbackRequired,
  ): Promise<boolean> {
    if (this.#store.getState().sessionGeneration !== sessionGeneration) {
      return false;
    }

    this.#cancelAllActivity();
    const fence = this.#store.getState().activateReader(
      sessionGeneration,
      'recovery',
      'recovering',
      {
        recoveryReason: fallback?.reason ?? 'snapshot fallback recovery',
        pauseWrites: true,
      },
    );
    if (!fence) {
      return false;
    }

    const abortController = new AbortController();
    this.#activeAbort = abortController;

    const stateAtStart = this.#store.getState();
    const accountsToRecover = dedupeIds([
      ...stateAtStart.requestedAccountIds,
      ...Object.keys(stateAtStart.snapshots),
    ]);

    if (accountsToRecover.length === 0) {
      return false;
    }

    for (let attempt = 1; attempt <= this.#maxRecoveryAttempts; attempt += 1) {
      if (!this.#isFenceCurrent(fence) || abortController.signal.aborted) {
        return false;
      }

      const stagedSnapshots = new Map<string, AccountBlotterSnapshot>();
      const stagedAllocations = new Map<string, readonly IndexedAllocation[]>();
      const baselineSeqs = new Map<string, bigint>();
      let repruned = false;

      try {
        // Phase 1: Fetch baseline snapshots for all accounts.
        for (const accountId of accountsToRecover) {
          if (!this.#isFenceCurrent(fence) || abortController.signal.aborted) {
            return false;
          }
          const res = await this.#client.getAccountSnapshot(
            { accountId },
            { signal: abortController.signal },
          );
          if (!this.#isFenceCurrent(fence) || abortController.signal.aborted) {
            return false;
          }
          const snap = validateAccountSnapshot(res.snapshot, 'RecoverySnapshot', accountId);
          stagedSnapshots.set(accountId, snap);
          baselineSeqs.set(accountId, snap.snapshotSeq);
        }

        const seqValues = Array.from(baselineSeqs.values());
        const minSeq = minUint64(seqValues);
        const maxSeq = maxUint64(seqValues);
        let replayCursor = minSeq;
        let observedHead = maxSeq;

        // Phase 2: Replay from min(s[a]) to at least max(s[a]).
        while (replayCursor < maxSeq) {
          if (!this.#isFenceCurrent(fence) || abortController.signal.aborted) {
            return false;
          }
          const pollRes = await this.#client.pollEvents(
            { afterSeq: replayCursor, maxEvents: this.#pollMaxEvents },
            { signal: abortController.signal },
          );
          if (!this.#isFenceCurrent(fence) || abortController.signal.aborted) {
            return false;
          }

          if (pollRes.outcome.case === 'snapshotFallback') {
            // Re-pruned during recovery: discard all staged state and retry with bounded backoff.
            repruned = true;
            break;
          }

          if (pollRes.outcome.case !== 'batch') {
            throw BlotterClientError.protocol('Recovery PollEvents returned empty outcome.');
          }

          const batch = pollRes.outcome.value;
          // Validate the ENTIRE batch atomically (including every change in every envelope,
          // even for accounts whose baseline already covers env.seq!).
          const stagedBatch = stageAndValidatePollBatch(batch, replayCursor, 'recovery');

          for (const stagedEnv of stagedBatch.envelopes) {
            for (const [acctId, snap] of stagedEnv.snapshots.entries()) {
              const baseSeq = baselineSeqs.get(acctId);
              if (baseSeq === undefined || stagedEnv.seq > baseSeq) {
                stagedSnapshots.set(acctId, snap);
              }
            }
            for (const [acctId, allocs] of stagedEnv.allocations.entries()) {
              if (allocs.length > 0) {
                const existing = stagedAllocations.get(acctId) ?? [];
                stagedAllocations.set(acctId, mergeDeduplicatedAllocations(existing, allocs));
              }
            }
          }

          if (stagedBatch.headSeq > observedHead) {
            observedHead = stagedBatch.headSeq;
          }
          if (stagedBatch.nextCursorSeq <= replayCursor && !stagedBatch.hasMore) {
            throw BlotterClientError.protocol(
              `Recovery PollEvents stalled at seq=${replayCursor.toString()} before reaching target max_seq=${maxSeq.toString()}`,
            );
          }
          replayCursor = stagedBatch.nextCursorSeq;
        }

        if (repruned) {
          if (attempt < this.#maxRecoveryAttempts) {
            const delay = Math.min(
              this.#recoveryBackoffBaseMs * 2 ** (attempt - 1),
              this.#recoveryBackoffMaxMs,
            );
            const continued = await sleepWithAbort(delay, this.#clock, abortController.signal);
            if (!continued) {
              return false;
            }
            continue;
          }
          this.#store.getState().reportTransportError({
            sessionGeneration: fence.sessionGeneration,
            transportEpoch: fence.transportEpoch,
            readerId: fence.readerId,
            error: BlotterClientError.protocol(
              'Recovery re-pruned repeatedly; manual retry required.',
            ),
            nextConnectionState: 'error',
          });
          return false;
        }

        // Atomic publish of consistent multi-account cut.
        const commit = this.#store.getState().commitBaselineSnapshots({
          sessionGeneration: fence.sessionGeneration,
          transportEpoch: fence.transportEpoch,
          readerId: fence.readerId,
          snapshots: stagedSnapshots,
          allocations: stagedAllocations,
          cursorSeq: replayCursor,
          headSeq: observedHead,
          nextConnectionState: 'stream_live',
          mergeExistingAllocations: true,
        });
        if (!commit.committed) {
          return false;
        }

        this.#startLiveStream(sessionGeneration, replayCursor);
        return true;
      } catch (err) {
        const clientErr = BlotterClientError.fromUnknown(
          err,
          this.#credentialStore.getRedactedSecrets(),
        );
        if (this.#handlePossibleAuthFailure(sessionGeneration, clientErr)) {
          return false;
        }
        if (!this.#isFenceCurrent(fence) || clientErr.isAborted()) {
          return false;
        }
        // Protocol corruption during recovery MUST abort immediately without mutating last complete view!
        if (clientErr.kind === 'protocol') {
          this.#store.getState().reportTransportError({
            sessionGeneration: fence.sessionGeneration,
            transportEpoch: fence.transportEpoch,
            readerId: fence.readerId,
            error: clientErr,
            nextConnectionState: 'error',
          });
          this.#cancelAllActivity();
          return false;
        }
        if (attempt < this.#maxRecoveryAttempts) {
          const delay = Math.min(
            this.#recoveryBackoffBaseMs * 2 ** (attempt - 1),
            this.#recoveryBackoffMaxMs,
          );
          const continued = await sleepWithAbort(delay, this.#clock, abortController.signal);
          if (!continued) {
            return false;
          }
          continue;
        }
        this.#store.getState().reportTransportError({
          sessionGeneration: fence.sessionGeneration,
          transportEpoch: fence.transportEpoch,
          readerId: fence.readerId,
          error: clientErr,
          nextConnectionState: 'error',
        });
        return false;
      }
    }

    return false;
  }

  /**
   * Manual retry entry point exposed when recovery exhausts bounded retries.
   */
  retryRecovery(): Promise<boolean> {
    const s = this.#store.getState();
    return this.runRecoveryBarrier(s.sessionGeneration);
  }
}

/**
 * Non-publishing private snapshot reader for hedge preview.
 * Has ZERO reference to the workspace store and can NEVER mutate workspace snapshots or `cursorSeq`.
 */
export async function fetchHedgeSnapshot(
  client: Pick<DeskBlotterRpcClient, 'getAccountSnapshot'>,
  params: {
    readonly accountId: string;
    readonly signal?: AbortSignal | undefined;
  },
): Promise<AccountBlotterSnapshot> {
  const res = await client.getAccountSnapshot(
    { accountId: params.accountId },
    { signal: params.signal },
  );
  return validateAccountSnapshot(res.snapshot, 'HedgePrivateSnapshot', params.accountId);
}

export interface ExecutePreviewCoveredHedgeParams {
  readonly sessionGeneration: number;
  readonly getCurrentSessionGeneration: () => number;
  readonly isRequestCurrent?: (() => boolean) | undefined;
  readonly snapshot: AccountBlotterSnapshot;
  readonly scope: PreviewHedgeScope;
  readonly productCode: string;
  readonly contractCode: string;
  readonly ctdDv01Per100: DecimalValue;
  readonly conversionFactor: DecimalValue;
  readonly valuationAt: MarketTime;
  readonly expectedSnapshotSeq?: bigint | undefined;
  readonly rulePackOptions?: LoadTrustedRulePackOptions | undefined;
  readonly signal?: AbortSignal | undefined;
  readonly maxConflictRetries?: number | undefined;
}

export type PreviewCoveredHedgeOutcome =
  | {
      readonly status: 'ok';
      readonly result: PreviewCoveredHedgeResult;
      readonly usedSnapshotSeq: bigint;
      readonly retriedConflict: boolean;
    }
  | {
      readonly status: 'stale';
    };

/**
 * Executes `PreviewCoveredHedge` in a non-publishing private context:
 * 1. Verifies the trusted delivery RulePack BEFORE any RPC (`0 RPCs` on tampered/expired/missing anchor).
 * 2. Validates request inputs against account `AnalysisUnits` and `allowed_hedge_product_codes`.
 * 3. Binds `(sessionGeneration, accountId, scope, productCode, contractCode, expectedSnapshotSeq)`.
 * 4. On global `head_seq` conflict (`CONCURRENCY_CONFLICT`/`VERSION_CONFLICT`), refreshes ONLY the
 *    private snapshot via `fetchHedgeSnapshot` at most once (0 workspace store/cursor mutation).
 * 5. Validates the response via `validatePreviewCoveredHedgeResult` (matching `blotter.py:1059-1170`).
 * 6. Discards late/superseded responses if `sessionGeneration` or `isRequestCurrent()` changed.
 */
export async function executePreviewCoveredHedge(
  client: Pick<DeskBlotterRpcClient, 'getAccountSnapshot' | 'previewCoveredHedge'>,
  params: ExecutePreviewCoveredHedgeParams,
): Promise<PreviewCoveredHedgeOutcome> {
  const stillValid = (): boolean =>
    params.getCurrentSessionGeneration() === params.sessionGeneration &&
    (params.isRequestCurrent ? params.isRequestCurrent() : true);

  if (!stillValid()) {
    return { status: 'stale' };
  }

  // Step 0: Validate MarketTime invariants client-side BEFORE RulePack fetch or any RPC.
  validateMarketTime(params.valuationAt, 'valuation_at');

  // Step 1: Verify trusted delivery RulePack BEFORE issuing ANY RPC call.
  const verifiedPack = await loadTrustedDeliveryRulePack({
    valuationAt: params.valuationAt,
    ...params.rulePackOptions,
  });

  if (!stillValid()) {
    return { status: 'stale' };
  }

  const maxConflictRetries = params.maxConflictRetries ?? 1;
  let activeSnapshot = params.snapshot;
  let activeExpectedSeq = params.expectedSnapshotSeq ?? activeSnapshot.snapshotSeq;
  let retriedConflict = false;

  for (let attempt = 0; attempt <= maxConflictRetries; attempt += 1) {
    if (!stillValid()) {
      return { status: 'stale' };
    }

    const units = activeSnapshot.openingBase?.units;
    if (!isNonEmptyTrimmed(params.productCode)) {
      throw new ClientValidationError([{ field: 'product_code', message: 'must not be empty' }]);
    }
    if (!isNonEmptyTrimmed(params.contractCode)) {
      throw new ClientValidationError([{ field: 'contract_code', message: 'must not be empty' }]);
    }
    if (
      activeSnapshot.allowedHedgeProductCodes.length > 0 &&
      !activeSnapshot.allowedHedgeProductCodes.includes(params.productCode)
    ) {
      throw new ClientValidationError([
        {
          field: 'product_code',
          message: `product "${params.productCode}" is not in snapshot.allowed_hedge_product_codes`,
        },
      ]);
    }
    if (
      params.scope === PreviewHedgeScope.WHOLE_ACCOUNT &&
      (activeSnapshot.riskSummary?.uncoveredInstrumentCount ?? 0) > 0
    ) {
      throw new ClientValidationError([
        {
          field: 'scope',
          message: 'WHOLE_ACCOUNT is blocked while uncovered_instrument_count > 0',
        },
      ]);
    }

    validateCanonicalDecimalValue(params.ctdDv01Per100, {
      field: 'ctd_dv01_per_100',
      sign: 'positive',
      requireUnit: true,
      ...(units?.dv01Per100 !== undefined ? { expectedUnit: units.dv01Per100 } : {}),
    });
    validateCanonicalDecimalValue(params.conversionFactor, {
      field: 'conversion_factor',
      sign: 'positive',
      requireUnit: true,
      ...(units?.dimensionless !== undefined ? { expectedUnit: units.dimensionless } : {}),
    });

    const request = create(PreviewCoveredHedgeRequestSchema, {
      accountId: activeSnapshot.accountId,
      scope: params.scope,
      productCode: params.productCode,
      contractCode: params.contractCode,
      ctdDv01Per100: params.ctdDv01Per100,
      conversionFactor: params.conversionFactor,
      deliveryRulePack: verifiedPack.rulePack,
      valuationAt: params.valuationAt,
      expectedSnapshotSeq: activeExpectedSeq,
    });

    try {
      const result = await client.previewCoveredHedge(request, { signal: params.signal });
      if (!stillValid()) {
        return { status: 'stale' };
      }
      validatePreviewCoveredHedgeResult(request, result, units?.currencyAmount);
      return {
        status: 'ok',
        result,
        usedSnapshotSeq: activeExpectedSeq,
        retriedConflict,
      };
    } catch (err) {
      const clientErr = BlotterClientError.fromUnknown(err);
      if (clientErr.isConcurrencyConflict() && attempt < maxConflictRetries) {
        retriedConflict = true;
        activeSnapshot = await fetchHedgeSnapshot(client, {
          accountId: activeSnapshot.accountId,
          signal: params.signal,
        });
        if (!stillValid()) {
          return { status: 'stale' };
        }
        activeExpectedSeq = activeSnapshot.snapshotSeq;
        continue;
      }
      throw err;
    }
  }

  return { status: 'stale' };
}

export type HistoricalAllocationLookupResult =
  | {
      readonly status: 'found';
      readonly allocations: readonly ReconcileAllocationView[];
    }
  | {
      readonly status: 'no_allocations';
    }
  | {
      readonly status: 'pruned_unrecoverable';
      readonly fallback: SnapshotFallbackRequired;
    }
  | {
      readonly status: 'transient_error';
      readonly error: BlotterClientError;
    }
  | {
      readonly status: 'stale';
    };

/**
 * Account-scoped read-only historical allocation lookup (`PollEvents(after_seq = committedSeq - 1)`).
 * Strictly isolated from workspace store cursor (ZERO workspace cursor/snapshot write access).
 */
export async function lookupCommittedAllocationsReadOnly(
  client: Pick<DeskBlotterRpcClient, 'pollEvents'>,
  params: {
    readonly sessionGeneration: number;
    readonly getCurrentSessionGeneration: () => number;
    readonly accountId: string;
    readonly committedSeq: bigint;
    readonly pendingMatchId?: string | undefined;
    readonly signal?: AbortSignal | undefined;
  },
): Promise<HistoricalAllocationLookupResult> {
  if (params.committedSeq < 1n) {
    return { status: 'no_allocations' };
  }
  if (params.getCurrentSessionGeneration() !== params.sessionGeneration) {
    return { status: 'stale' };
  }

  const afterSeq = params.committedSeq - 1n;
  try {
    const res = await client.pollEvents(
      {
        afterSeq,
        maxEvents: 1,
        accountId: params.accountId,
      },
      { signal: params.signal },
    );
    if (params.getCurrentSessionGeneration() !== params.sessionGeneration) {
      return { status: 'stale' };
    }
    if (res.outcome.case === 'snapshotFallback') {
      return {
        status: 'pruned_unrecoverable',
        fallback: res.outcome.value,
      };
    }
    if (res.outcome.case !== 'batch') {
      return {
        status: 'transient_error',
        error: BlotterClientError.protocol('Read-only PollEvents returned empty outcome.'),
      };
    }

    const stagedBatch = stageAndValidatePollBatch(res.outcome.value, afterSeq, 'poll');
    const matchingEnv = stagedBatch.envelopes.find((env) => env.seq === params.committedSeq);
    if (!matchingEnv) {
      return { status: 'no_allocations' };
    }
    const acctAllocs = matchingEnv.allocations.get(params.accountId) ?? [];
    const filtered =
      params.pendingMatchId !== undefined
        ? acctAllocs.filter((a) => a.view.pendingMatchId?.value === params.pendingMatchId)
        : acctAllocs;

    if (filtered.length === 0) {
      return { status: 'no_allocations' };
    }
    return {
      status: 'found',
      allocations: filtered.map((a) => a.view),
    };
  } catch (err) {
    if (params.getCurrentSessionGeneration() !== params.sessionGeneration) {
      return { status: 'stale' };
    }
    return {
      status: 'transient_error',
      error: BlotterClientError.fromUnknown(err),
    };
  }
}
