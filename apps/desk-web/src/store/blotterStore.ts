import { createStore, type StoreApi } from 'zustand/vanilla';
import type {
  AccountBlotterSnapshot,
  BlotterEventEnvelope,
  EarmarkRecord,
  PollEventsBatch,
  PreviewHedgeScope,
  PrincipalRole,
  StreamHeartbeat,
} from '../generated/ficant/blotter/v1/model_pb';
import type { PreviewCoveredHedgeResult } from '../generated/ficant/blotter/v1/service_pb';
import type { NlpEarmarkProposal } from '../generated/ficant/blotter/v1/spi_pb';
import type { CredentialHeaderMode } from '../transport/auth';
import type { BlotterClientError } from '../transport/error';
import {
  mergeDeduplicatedAllocations,
  stageAndValidateEnvelope,
  stageAndValidatePollBatch,
  validateAccountSnapshot,
  validateStreamHeartbeat,
  type IndexedAllocation,
} from './envelopeValidator';

export type ConnectionHealthState =
  | 'unauthenticated'
  | 'idle_no_accounts'
  | 'bootstrapping'
  | 'stream_live'
  | 'poll_degraded'
  | 'recovering'
  | 'error';

export interface WriteReceiptRecord {
  readonly id: string;
  readonly operation: string;
  readonly accountId: string;
  readonly committedSeq: bigint;
  readonly replayed: boolean;
  readonly status: 'succeeded' | 'confirm_hold_expired' | 'failed';
  readonly earmarkId?: string | undefined;
  readonly pendingMatchId?: string | undefined;
  readonly draftId?: string | undefined;
  readonly earmark?: EarmarkRecord | undefined;
  readonly error?: BlotterClientError | undefined;
  readonly recordedAtMs: number;
}

export interface ActiveDraftConfirmationContext {
  readonly draftId: string;
  readonly accountId: string;
  readonly confirmationTokenBytes: Uint8Array;
  readonly previewAtSeq: bigint;
}

export interface HedgePreviewState {
  readonly sessionGeneration: number;
  readonly accountId: string;
  readonly scope: PreviewHedgeScope;
  readonly productCode: string;
  readonly contractCode: string;
  readonly expectedSnapshotSeq: bigint;
  readonly result: PreviewCoveredHedgeResult | null;
  readonly error: BlotterClientError | null;
}

export type CommitOutcome =
  | { readonly committed: true; readonly cursorSeq: bigint; readonly noOp?: boolean }
  | {
      readonly committed: false;
      readonly reason:
        | 'stale_session'
        | 'stale_transport_epoch'
        | 'stale_reader'
        | 'stale_cursor'
        | 'non_increasing_seq'
        | 'validation_error';
      readonly error?: BlotterClientError;
    };

export interface ReaderActivationResult {
  readonly sessionGeneration: number;
  readonly transportEpoch: number;
  readonly readerId: string;
}

export interface SessionInitMetadata {
  readonly headerMode: CredentialHeaderMode;
  readonly roleHint?: PrincipalRole | null | undefined;
  readonly actorLabel?: string | null | undefined;
}

export interface BlotterStoreData {
  readonly sessionGeneration: number;
  readonly authenticated: boolean;
  readonly headerMode: CredentialHeaderMode | null;
  readonly roleHint: PrincipalRole | null;
  readonly actorLabel: string | null;

  readonly transportEpoch: number;
  readonly activeReaderId: string | null;
  readonly connectionState: ConnectionHealthState;
  readonly recoveryReason: string | null;
  readonly recoveryPausedWrites: boolean;
  readonly lastError: BlotterClientError | null;

  readonly requestedAccountIds: readonly string[];
  readonly selectedAccountId: string | null;
  readonly accountLoadErrors: Readonly<Record<string, BlotterClientError>>;

  readonly cursorSeq: bigint;
  readonly knownHeadSeq: bigint;
  readonly snapshots: Readonly<Record<string, AccountBlotterSnapshot>>;
  readonly allocationsByAccount: Readonly<Record<string, readonly IndexedAllocation[]>>;
  readonly businessCommitVersion: number;

  readonly writeReceipts: readonly WriteReceiptRecord[];
  readonly latestReceiptCommittedSeq: bigint;

  readonly draftInputText: string;
  readonly parsedProposal: NlpEarmarkProposal | null;
  readonly activeDraftConfirmation: ActiveDraftConfirmationContext | null;
  readonly hedgePreview: HedgePreviewState | null;
}

export interface BlotterStoreActions {
  /**
   * Authenticates or switches credentials.
   * Synchronously increments `sessionGeneration` and `transportEpoch`, invalidates any active
   * reader, and wipes all accounts, allocations, drafts, confirmation tokens, receipts, and previews.
   */
  beginSession(meta: SessionInitMetadata): number;

  /**
   * Logs out synchronously: increments `sessionGeneration`, clears active reader, and wipes
   * all session-scoped business and UI state.
   */
  logout(): number;

  /**
   * Handles server-reported UNAUTHENTICATED.
   * Only takes effect if `forSessionGeneration === state.sessionGeneration`; stale errors from
   * a superseded session are dropped without affecting the current session.
   */
  handleUnauthenticated(forSessionGeneration: number, error?: BlotterClientError): boolean;

  /**
   * Sets explicit account IDs submitted by the trader/operator (deduplicated, trimmed).
   * Empty array keeps the workspace in `idle_no_accounts`.
   */
  setRequestedAccounts(accountIds: readonly string[], forSessionGeneration: number): readonly string[];

  setSelectedAccountId(accountId: string | null): void;

  setAccountLoadError(
    forSessionGeneration: number,
    accountId: string,
    error: BlotterClientError | null,
  ): void;

  /**
   * Activates a new transport reader (stream, poll, bootstrap, or recovery), synchronously
   * incrementing `transportEpoch` so any late frames or fallbacks from prior readers are fenced out.
   */
  activateReader(
    forSessionGeneration: number,
    readerKind: 'bootstrap' | 'stream' | 'poll' | 'recovery',
    nextConnectionState: ConnectionHealthState,
    options?: { readonly recoveryReason?: string | null; readonly pauseWrites?: boolean },
  ): ReaderActivationResult | null;

  /**
   * Reports a connection/transport error for the current `(sessionGeneration, transportEpoch, readerId)`.
   * Stale errors are ignored without mutating store state.
   */
  reportTransportError(params: {
    readonly sessionGeneration: number;
    readonly transportEpoch: number;
    readonly readerId: string;
    readonly error: BlotterClientError;
    readonly nextConnectionState?: ConnectionHealthState;
  }): boolean;

  /**
   * Single synchronous atomic committer: publishes baseline snapshots (cold-start or
   * completed two-phase multi-account recovery barrier). No `await` inside.
   */
  commitBaselineSnapshots(params: {
    readonly sessionGeneration: number;
    readonly transportEpoch: number;
    readonly readerId: string;
    readonly snapshots: ReadonlyMap<string, AccountBlotterSnapshot>;
    readonly allocations?: ReadonlyMap<string, readonly IndexedAllocation[]>;
    readonly cursorSeq: bigint;
    readonly headSeq: bigint;
    readonly nextConnectionState?: ConnectionHealthState;
    readonly mergeExistingAllocations?: boolean;
  }): CommitOutcome;

  /**
   * Single synchronous atomic committer: validates and applies one `SubscribeEvents` envelope.
   * No `await` inside. On any fencing or validation failure, leaves state/cursor/listeners untouched.
   */
  commitStreamEnvelope(params: {
    readonly sessionGeneration: number;
    readonly transportEpoch: number;
    readonly readerId: string;
    readonly envelope: BlotterEventEnvelope;
  }): CommitOutcome;

  /**
   * Single synchronous atomic committer: validates and applies a `StreamHeartbeat`.
   * Rejects `scanned_through_seq < cursorSeq` or `scanned_through_seq > head_seq`.
   */
  commitStreamHeartbeat(params: {
    readonly sessionGeneration: number;
    readonly transportEpoch: number;
    readonly readerId: string;
    readonly heartbeat: StreamHeartbeat;
  }): CommitOutcome;

  /**
   * Single synchronous atomic committer: validates and applies a `PollEventsBatch` with strict
   * start-cursor CAS (`requestAfterSeq === state.cursorSeq`).
   * On any stale fence, cursor mismatch, or corrupt envelope (including corrupt 2nd event),
   * performs full atomic rollback with zero state/cursor/subscriber notification.
   */
  commitPollBatch(params: {
    readonly sessionGeneration: number;
    readonly transportEpoch: number;
    readonly readerId: string;
    readonly requestAfterSeq: bigint;
    readonly batch: PollEventsBatch;
  }): CommitOutcome;

  /**
   * Records a write receipt and known `committed_seq` WITHOUT advancing workspace `cursorSeq`.
   * Drops late receipts from superseded sessions.
   */
  recordWriteReceipt(forSessionGeneration: number, receipt: WriteReceiptRecord): boolean;

  setDraftState(
    forSessionGeneration: number,
    patch: {
      readonly draftInputText?: string;
      readonly parsedProposal?: NlpEarmarkProposal | null;
      readonly activeDraftConfirmation?: ActiveDraftConfirmationContext | null;
    },
  ): boolean;

  setHedgePreview(forSessionGeneration: number, preview: HedgePreviewState | null): boolean;
}

export type BlotterStoreState = BlotterStoreData & BlotterStoreActions;

function createCleanSessionData(
  sessionGeneration: number,
  transportEpoch: number,
  auth: {
    readonly authenticated: boolean;
    readonly headerMode: CredentialHeaderMode | null;
    readonly roleHint: PrincipalRole | null;
    readonly actorLabel: string | null;
    readonly connectionState: ConnectionHealthState;
    readonly lastError: BlotterClientError | null;
  },
): BlotterStoreData {
  return {
    sessionGeneration,
    authenticated: auth.authenticated,
    headerMode: auth.headerMode,
    roleHint: auth.roleHint,
    actorLabel: auth.actorLabel,

    transportEpoch,
    activeReaderId: null,
    connectionState: auth.connectionState,
    recoveryReason: null,
    recoveryPausedWrites: false,
    lastError: auth.lastError,

    requestedAccountIds: [],
    selectedAccountId: null,
    accountLoadErrors: {},

    cursorSeq: 0n,
    knownHeadSeq: 0n,
    snapshots: {},
    allocationsByAccount: {},
    businessCommitVersion: 0,

    writeReceipts: [],
    latestReceiptCommittedSeq: 0n,

    draftInputText: '',
    parsedProposal: null,
    activeDraftConfirmation: null,
    hedgePreview: null,
  };
}

function checkReaderFence(
  state: BlotterStoreData,
  sessionGeneration: number,
  transportEpoch: number,
  readerId: string,
): CommitOutcome | null {
  if (state.sessionGeneration !== sessionGeneration) {
    return { committed: false, reason: 'stale_session' };
  }
  if (state.transportEpoch !== transportEpoch) {
    return { committed: false, reason: 'stale_transport_epoch' };
  }
  if (state.activeReaderId !== readerId) {
    return { committed: false, reason: 'stale_reader' };
  }
  return null;
}

function deduplicateAccountIds(rawIds: readonly string[]): readonly string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of rawIds) {
    const trimmed = raw.trim();
    if (trimmed.length > 0 && !seen.has(trimmed)) {
      seen.add(trimmed);
      out.push(trimmed);
    }
  }
  return out;
}

export type BlotterStoreApi = StoreApi<BlotterStoreState>;

export function createBlotterStore(): BlotterStoreApi {
  return createStore<BlotterStoreState>((set, get) => ({
    ...createCleanSessionData(0, 0, {
      authenticated: false,
      headerMode: null,
      roleHint: null,
      actorLabel: null,
      connectionState: 'unauthenticated',
      lastError: null,
    }),

    beginSession(meta) {
      const prev = get();
      const nextGen = prev.sessionGeneration + 1;
      const nextEpoch = prev.transportEpoch + 1;
      set(
        createCleanSessionData(nextGen, nextEpoch, {
          authenticated: true,
          headerMode: meta.headerMode,
          roleHint: meta.roleHint ?? null,
          actorLabel: meta.actorLabel ?? null,
          connectionState: 'idle_no_accounts',
          lastError: null,
        }),
      );
      return nextGen;
    },

    logout() {
      const prev = get();
      const nextGen = prev.sessionGeneration + 1;
      const nextEpoch = prev.transportEpoch + 1;
      set(
        createCleanSessionData(nextGen, nextEpoch, {
          authenticated: false,
          headerMode: null,
          roleHint: null,
          actorLabel: null,
          connectionState: 'unauthenticated',
          lastError: null,
        }),
      );
      return nextGen;
    },

    handleUnauthenticated(forSessionGeneration, error) {
      const prev = get();
      if (prev.sessionGeneration !== forSessionGeneration) {
        return false;
      }
      const nextGen = prev.sessionGeneration + 1;
      const nextEpoch = prev.transportEpoch + 1;
      set(
        createCleanSessionData(nextGen, nextEpoch, {
          authenticated: false,
          headerMode: null,
          roleHint: null,
          actorLabel: null,
          connectionState: 'unauthenticated',
          lastError: error ?? null,
        }),
      );
      return true;
    },

    setRequestedAccounts(accountIds, forSessionGeneration) {
      const prev = get();
      if (prev.sessionGeneration !== forSessionGeneration) {
        return prev.requestedAccountIds;
      }
      const normalized = deduplicateAccountIds(accountIds);
      const nextSelected =
        prev.selectedAccountId !== null && normalized.includes(prev.selectedAccountId)
          ? prev.selectedAccountId
          : (normalized[0] ?? null);

      const nextErrors: Record<string, BlotterClientError> = {};
      for (const id of normalized) {
        const existingErr = prev.accountLoadErrors[id];
        if (existingErr) {
          nextErrors[id] = existingErr;
        }
      }

      set({
        requestedAccountIds: normalized,
        selectedAccountId: nextSelected,
        accountLoadErrors: nextErrors,
        ...(normalized.length === 0
          ? {
              connectionState: prev.authenticated ? 'idle_no_accounts' : 'unauthenticated',
              activeReaderId: null,
              transportEpoch: prev.transportEpoch + 1,
            }
          : {}),
      });
      return normalized;
    },

    setSelectedAccountId(accountId) {
      const prev = get();
      if (prev.selectedAccountId === accountId) {
        return;
      }
      set({ selectedAccountId: accountId });
    },

    setAccountLoadError(forSessionGeneration, accountId, error) {
      const prev = get();
      if (prev.sessionGeneration !== forSessionGeneration) {
        return;
      }
      const nextErrors: Record<string, BlotterClientError> = { ...prev.accountLoadErrors };
      if (error === null) {
        delete nextErrors[accountId];
      } else {
        nextErrors[accountId] = error;
      }
      set({ accountLoadErrors: nextErrors });
    },

    activateReader(forSessionGeneration, readerKind, nextConnectionState, options) {
      const prev = get();
      if (prev.sessionGeneration !== forSessionGeneration) {
        return null;
      }
      const nextEpoch = prev.transportEpoch + 1;
      const readerId = `${readerKind}-${nextEpoch}`;
      set({
        transportEpoch: nextEpoch,
        activeReaderId: readerId,
        connectionState: nextConnectionState,
        recoveryReason:
          options?.recoveryReason !== undefined
            ? options.recoveryReason
            : nextConnectionState === 'recovering'
              ? prev.recoveryReason
              : null,
        recoveryPausedWrites:
          options?.pauseWrites !== undefined
            ? options.pauseWrites
            : nextConnectionState === 'recovering',
      });
      return {
        sessionGeneration: prev.sessionGeneration,
        transportEpoch: nextEpoch,
        readerId,
      };
    },

    reportTransportError(params) {
      const prev = get();
      const fenceFailure = checkReaderFence(
        prev,
        params.sessionGeneration,
        params.transportEpoch,
        params.readerId,
      );
      if (fenceFailure !== null) {
        return false;
      }
      set({
        lastError: params.error,
        ...(params.nextConnectionState !== undefined
          ? { connectionState: params.nextConnectionState }
          : {}),
      });
      return true;
    },

    commitBaselineSnapshots(params) {
      const prev = get();
      const fenceFailure = checkReaderFence(
        prev,
        params.sessionGeneration,
        params.transportEpoch,
        params.readerId,
      );
      if (fenceFailure !== null) {
        return fenceFailure;
      }

      if (params.cursorSeq > params.headSeq) {
        return { committed: false, reason: 'validation_error' };
      }

      // Validate ALL baseline snapshots before any mutation.
      const nextSnapshots: Record<string, AccountBlotterSnapshot> = {};
      try {
        for (const [acctId, snap] of params.snapshots.entries()) {
          validateAccountSnapshot(snap, 'BaselineCommit', acctId);
          nextSnapshots[acctId] = snap;
        }
      } catch (err) {
        return {
          committed: false,
          reason: 'validation_error',
          ...(err instanceof Error ? { error: err as BlotterClientError } : {}),
        };
      }

      const nextAllocations: Record<string, readonly IndexedAllocation[]> =
        params.mergeExistingAllocations ? { ...prev.allocationsByAccount } : {};
      if (params.allocations) {
        for (const [acctId, incoming] of params.allocations.entries()) {
          const existing = nextAllocations[acctId] ?? [];
          nextAllocations[acctId] = mergeDeduplicatedAllocations(existing, incoming);
        }
      }

      const acctKeys = Object.keys(nextSnapshots);
      const nextSelected =
        prev.selectedAccountId !== null && acctKeys.includes(prev.selectedAccountId)
          ? prev.selectedAccountId
          : (acctKeys[0] ?? prev.selectedAccountId);

      const nextHead = params.headSeq > prev.knownHeadSeq ? params.headSeq : prev.knownHeadSeq;

      set({
        snapshots: nextSnapshots,
        allocationsByAccount: nextAllocations,
        cursorSeq: params.cursorSeq,
        knownHeadSeq: nextHead,
        selectedAccountId: nextSelected,
        accountLoadErrors: {},
        recoveryPausedWrites: false,
        recoveryReason: null,
        lastError: null,
        businessCommitVersion: prev.businessCommitVersion + 1,
        ...(params.nextConnectionState !== undefined
          ? { connectionState: params.nextConnectionState }
          : {}),
      });
      return { committed: true, cursorSeq: params.cursorSeq };
    },

    commitStreamEnvelope(params) {
      const prev = get();
      const fenceFailure = checkReaderFence(
        prev,
        params.sessionGeneration,
        params.transportEpoch,
        params.readerId,
      );
      if (fenceFailure !== null) {
        return fenceFailure;
      }

      if (params.envelope.seq <= prev.cursorSeq) {
        return { committed: false, reason: 'non_increasing_seq' };
      }

      let staged;
      try {
        staged = stageAndValidateEnvelope(params.envelope, 'subscribe');
      } catch (err) {
        return {
          committed: false,
          reason: 'validation_error',
          ...(err instanceof Error ? { error: err as BlotterClientError } : {}),
        };
      }

      const nextSnapshots: Record<string, AccountBlotterSnapshot> = { ...prev.snapshots };
      const nextAllocations: Record<string, readonly IndexedAllocation[]> = {
        ...prev.allocationsByAccount,
      };

      for (const [acctId, snap] of staged.snapshots.entries()) {
        nextSnapshots[acctId] = snap;
      }
      for (const [acctId, allocs] of staged.allocations.entries()) {
        if (allocs.length > 0) {
          const existing = nextAllocations[acctId] ?? [];
          nextAllocations[acctId] = mergeDeduplicatedAllocations(existing, allocs);
        }
      }

      const nextHead = staged.seq > prev.knownHeadSeq ? staged.seq : prev.knownHeadSeq;
      const nextSelected =
        prev.selectedAccountId ?? Object.keys(nextSnapshots)[0] ?? null;

      set({
        snapshots: nextSnapshots,
        allocationsByAccount: nextAllocations,
        cursorSeq: staged.seq,
        knownHeadSeq: nextHead,
        selectedAccountId: nextSelected,
        connectionState: 'stream_live',
        lastError: null,
        businessCommitVersion: prev.businessCommitVersion + 1,
      });
      return { committed: true, cursorSeq: staged.seq };
    },

    commitStreamHeartbeat(params) {
      const prev = get();
      const fenceFailure = checkReaderFence(
        prev,
        params.sessionGeneration,
        params.transportEpoch,
        params.readerId,
      );
      if (fenceFailure !== null) {
        return fenceFailure;
      }

      let validated;
      try {
        validated = validateStreamHeartbeat(params.heartbeat, prev.cursorSeq);
      } catch (err) {
        return {
          committed: false,
          reason: 'validation_error',
          ...(err instanceof Error ? { error: err as BlotterClientError } : {}),
        };
      }

      const nextHead =
        validated.headSeq > prev.knownHeadSeq ? validated.headSeq : prev.knownHeadSeq;
      const cursorChanged = validated.nextCursorSeq !== prev.cursorSeq;
      const headChanged = nextHead !== prev.knownHeadSeq;
      const stateChanged = prev.connectionState !== 'stream_live';

      if (!cursorChanged && !headChanged && !stateChanged) {
        return { committed: true, cursorSeq: prev.cursorSeq, noOp: true };
      }

      set({
        cursorSeq: validated.nextCursorSeq,
        knownHeadSeq: nextHead,
        connectionState: 'stream_live',
        ...(cursorChanged ? { businessCommitVersion: prev.businessCommitVersion + 1 } : {}),
      });
      return { committed: true, cursorSeq: validated.nextCursorSeq };
    },

    commitPollBatch(params) {
      const prev = get();
      const fenceFailure = checkReaderFence(
        prev,
        params.sessionGeneration,
        params.transportEpoch,
        params.readerId,
      );
      if (fenceFailure !== null) {
        return fenceFailure;
      }

      // Synchronous start-cursor CAS check: r === store.cursorSeq.
      if (params.requestAfterSeq !== prev.cursorSeq) {
        return { committed: false, reason: 'stale_cursor' };
      }

      let stagedBatch;
      try {
        stagedBatch = stageAndValidatePollBatch(params.batch, params.requestAfterSeq, 'poll');
      } catch (err) {
        return {
          committed: false,
          reason: 'validation_error',
          ...(err instanceof Error ? { error: err as BlotterClientError } : {}),
        };
      }

      const hasEvents = stagedBatch.envelopes.length > 0;
      const cursorAdvanced = stagedBatch.nextCursorSeq > prev.cursorSeq;
      const nextHead =
        stagedBatch.headSeq > prev.knownHeadSeq ? stagedBatch.headSeq : prev.knownHeadSeq;
      const headChanged = nextHead !== prev.knownHeadSeq;

      if (!hasEvents && !cursorAdvanced && !headChanged) {
        return { committed: true, cursorSeq: prev.cursorSeq, noOp: true };
      }

      const nextSnapshots: Record<string, AccountBlotterSnapshot> = { ...prev.snapshots };
      const nextAllocations: Record<string, readonly IndexedAllocation[]> = {
        ...prev.allocationsByAccount,
      };

      for (const staged of stagedBatch.envelopes) {
        for (const [acctId, snap] of staged.snapshots.entries()) {
          nextSnapshots[acctId] = snap;
        }
        for (const [acctId, allocs] of staged.allocations.entries()) {
          if (allocs.length > 0) {
            const existing = nextAllocations[acctId] ?? [];
            nextAllocations[acctId] = mergeDeduplicatedAllocations(existing, allocs);
          }
        }
      }

      const nextSelected =
        prev.selectedAccountId ?? Object.keys(nextSnapshots)[0] ?? null;

      set({
        snapshots: nextSnapshots,
        allocationsByAccount: nextAllocations,
        cursorSeq: stagedBatch.nextCursorSeq,
        knownHeadSeq: nextHead,
        selectedAccountId: nextSelected,
        lastError: null,
        businessCommitVersion: prev.businessCommitVersion + 1,
      });
      return { committed: true, cursorSeq: stagedBatch.nextCursorSeq };
    },

    recordWriteReceipt(forSessionGeneration, receipt) {
      const prev = get();
      if (prev.sessionGeneration !== forSessionGeneration) {
        return false;
      }
      const nextLatest =
        receipt.committedSeq > prev.latestReceiptCommittedSeq
          ? receipt.committedSeq
          : prev.latestReceiptCommittedSeq;
      // CRITICAL INVARIANT: Write receipts NEVER advance `cursorSeq` or mutate `snapshots`!
      set({
        writeReceipts: [...prev.writeReceipts, receipt],
        latestReceiptCommittedSeq: nextLatest,
      });
      return true;
    },

    setDraftState(forSessionGeneration, patch) {
      const prev = get();
      if (prev.sessionGeneration !== forSessionGeneration) {
        return false;
      }
      set({
        ...(patch.draftInputText !== undefined ? { draftInputText: patch.draftInputText } : {}),
        ...(patch.parsedProposal !== undefined ? { parsedProposal: patch.parsedProposal } : {}),
        ...(patch.activeDraftConfirmation !== undefined
          ? { activeDraftConfirmation: patch.activeDraftConfirmation }
          : {}),
      });
      return true;
    },

    setHedgePreview(forSessionGeneration, preview) {
      const prev = get();
      if (prev.sessionGeneration !== forSessionGeneration) {
        return false;
      }
      if (preview !== null && preview.sessionGeneration !== forSessionGeneration) {
        return false;
      }
      set({ hedgePreview: preview });
      return true;
    },
  }));
}
