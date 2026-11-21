import {
  PendingMatchResolutionAction,
  type EarmarkRecord,
  type FundingGapSummary,
  type PendingMatchRecord,
  type ReconcileAllocationView,
  type SnapshotFallbackRequired,
} from '../../generated/ficant/blotter/v1/model_pb';
import type { BlotterStoreData } from '../../store/blotterStore';
import type { BlotterClientError } from '../../transport/error';

export type LookupAllocationsStatus =
  | 'idle'
  | 'loading'
  | 'found'
  | 'no_allocations'
  | 'pruned_unrecoverable'
  | 'transient_error';

export interface PendingMatchSubmissionRecord {
  readonly receiptId: string;
  readonly sessionGeneration: number;
  readonly accountId: string;
  readonly pendingMatchId: string;
  readonly instrumentCode: string;
  readonly action: PendingMatchResolutionAction;
  readonly committedSeq: bigint;
  readonly replayed: boolean;
  readonly submittedAtSnapshotSeq: bigint;
  readonly pendingMatchResult?: PendingMatchRecord | undefined;
  readonly updatedEarmark?: EarmarkRecord | undefined;
  readonly fundingSummaryAtResult?: FundingGapSummary | undefined;
  readonly lookupStatus: LookupAllocationsStatus;
  readonly lookedUpAllocations: readonly ReconcileAllocationView[];
  readonly lookupFallback?: SnapshotFallbackRequired | undefined;
  readonly lookupError?: BlotterClientError | undefined;
}

export type AttributionStateKind =
  | 'allocated'
  | 'no_allocations'
  | 'pending_correlation'
  | 'transient_disconnected'
  | 'pruned_history_unrecoverable'
  | 'unmatched_empty_history'
  | 'stale_session';

export interface ResolvedAttributionView {
  readonly kind: AttributionStateKind;
  readonly allocations: readonly ReconcileAllocationView[];
  readonly source: 'store_stream' | 'readonly_lookup' | 'none';
  readonly isLoadingLookup: boolean;
  readonly hasMismatchedPendingIdAtSeq: boolean;
  readonly fallback?: SnapshotFallbackRequired | undefined;
  readonly error?: BlotterClientError | undefined;
}

/**
 * Only `RELEASE_REJECT` legitimately produces zero `ReconcileAllocationRecord`s per `reconcile.rs:2036-2040`.
 * All other 4 actions (`MATCH_FULL`, `SPLIT_OFFSET`, `ACCEPT_ORPHAN`, `APPLY_BASELINE_DELTA`)
 * produce a transaction-scoped allocation on the committed event envelope.
 */
export function actionExpectsAllocations(action: PendingMatchResolutionAction): boolean {
  return action !== PendingMatchResolutionAction.RELEASE_REJECT;
}

export function dedupeReconcileAllocations(
  views: readonly ReconcileAllocationView[],
): readonly ReconcileAllocationView[] {
  const seen = new Set<string>();
  const out: ReconcileAllocationView[] = [];
  for (let i = 0; i < views.length; i += 1) {
    const view = views[i];
    if (!view) {
      continue;
    }
    const id = view.allocationId?.value;
    const key = id && id.length > 0 ? id : `idx:${i}:${view.sourceId}:${view.externalTradeId}`;
    if (!seen.has(key)) {
      seen.add(key);
      out.push(view);
    }
  }
  return out;
}

/**
 * Returns ONLY the allocations strictly matching `(committedSeq, accountId, pendingMatchId)`
 * from the workspace store's account-keyed allocation index.
 *
 * Guarantees:
 * - Allocations belonging to a different `accountId` are NEVER returned.
 * - Allocations belonging to a different `seq` (including subsequent unrelated mutations) are NEVER returned.
 * - Allocations at the same `(committedSeq, accountId)` with a different `pendingMatchId` are NEVER returned.
 */
export function selectExactTransactionAllocations(
  storeState: Pick<BlotterStoreData, 'allocationsByAccount'>,
  accountId: string,
  committedSeq: bigint,
  pendingMatchId: string,
): readonly ReconcileAllocationView[] {
  const acctAllocs = storeState.allocationsByAccount[accountId] ?? [];
  const matching: ReconcileAllocationView[] = [];
  for (const item of acctAllocs) {
    if (
      item.accountId === accountId &&
      item.seq === committedSeq &&
      item.view.pendingMatchId?.value === pendingMatchId
    ) {
      matching.push(item.view);
    }
  }
  return dedupeReconcileAllocations(matching);
}

/**
 * Detects whether the local store contains allocations at `(committedSeq, accountId)` that carry
 * a DIFFERENT `pending_match_id` (diagnostic only — those allocations are NEVER attributed to this submission).
 */
export function hasMismatchedPendingIdAllocationsAtSeq(
  storeState: Pick<BlotterStoreData, 'allocationsByAccount'>,
  accountId: string,
  committedSeq: bigint,
  expectedPendingMatchId: string,
): boolean {
  const acctAllocs = storeState.allocationsByAccount[accountId] ?? [];
  return acctAllocs.some(
    (item) =>
      item.accountId === accountId &&
      item.seq === committedSeq &&
      item.view.pendingMatchId?.value !== expectedPendingMatchId,
  );
}

/**
 * Resolves the exact post-submit cash attribution state for a submitted `ResolvePendingMatch`
 * transaction, strictly respecting session boundaries, both stream/receipt arrival orders,
 * idempotent replay, zero-allocation actions, transient disconnects, and truly pruned history.
 */
export function resolveSubmissionAttribution(
  submission: PendingMatchSubmissionRecord,
  storeState: Pick<
    BlotterStoreData,
    'sessionGeneration' | 'allocationsByAccount' | 'cursorSeq' | 'connectionState'
  >,
): ResolvedAttributionView {
  if (submission.sessionGeneration !== storeState.sessionGeneration) {
    return {
      kind: 'stale_session',
      allocations: [],
      source: 'none',
      isLoadingLookup: false,
      hasMismatchedPendingIdAtSeq: false,
    };
  }

  const fromStore = selectExactTransactionAllocations(
    storeState,
    submission.accountId,
    submission.committedSeq,
    submission.pendingMatchId,
  );
  const fromLookup = dedupeReconcileAllocations(
    submission.lookedUpAllocations.filter(
      (view) => view.pendingMatchId?.value === submission.pendingMatchId,
    ),
  );
  const merged = dedupeReconcileAllocations([...fromStore, ...fromLookup]);
  const hasMismatched = hasMismatchedPendingIdAllocationsAtSeq(
    storeState,
    submission.accountId,
    submission.committedSeq,
    submission.pendingMatchId,
  );

  // 1. Exact transaction-scoped allocations found (covers both arrival orders & replay).
  if (merged.length > 0) {
    return {
      kind: 'allocated',
      allocations: merged,
      source: fromStore.length > 0 ? 'store_stream' : 'readonly_lookup',
      isLoadingLookup: false,
      hasMismatchedPendingIdAtSeq: hasMismatched,
    };
  }

  // 2. Legitimate zero-allocation action (RELEASE_REJECT).
  if (!actionExpectsAllocations(submission.action)) {
    return {
      kind: 'no_allocations',
      allocations: [],
      source: 'none',
      isLoadingLookup: false,
      hasMismatchedPendingIdAtSeq: hasMismatched,
    };
  }

  // 3. Truly pruned / unrecoverable journal history confirmed by SnapshotFallbackRequired.
  if (submission.lookupStatus === 'pruned_unrecoverable') {
    return {
      kind: 'pruned_history_unrecoverable',
      allocations: [],
      source: 'none',
      isLoadingLookup: false,
      hasMismatchedPendingIdAtSeq: hasMismatched,
      ...(submission.lookupFallback !== undefined
        ? { fallback: submission.lookupFallback }
        : {}),
    };
  }

  // 4. Transient network/transport error during read-only lookup (NOT pruned!).
  if (submission.lookupStatus === 'transient_error') {
    return {
      kind: 'transient_disconnected',
      allocations: [],
      source: 'none',
      isLoadingLookup: false,
      hasMismatchedPendingIdAtSeq: hasMismatched,
      ...(submission.lookupError !== undefined ? { error: submission.lookupError } : {}),
    };
  }

  // 5. Read-only lookup completed against intact journal history with zero matching allocations
  //    (e.g., wrong pending_match_id, wrong account_id, or empty changed_allocations).
  if (submission.lookupStatus === 'no_allocations') {
    return {
      kind: 'unmatched_empty_history',
      allocations: [],
      source: 'none',
      isLoadingLookup: false,
      hasMismatchedPendingIdAtSeq: hasMismatched,
    };
  }

  // 6. Waiting for matching transaction event (receipt arrived before stream/poll, or lookup loading).
  return {
    kind: 'pending_correlation',
    allocations: [],
    source: 'none',
    isLoadingLookup: submission.lookupStatus === 'loading',
    hasMismatchedPendingIdAtSeq: hasMismatched,
  };
}

/**
 * Determines whether the panel should automatically trigger a read-only point-in-time lookup
 * (`lookupCommittedAllocationsReadOnly`, ZERO workspace cursor write) because the workspace
 * cursor has already reached or passed `committedSeq` while the local store has no matching
 * allocation for `(committedSeq, accountId, pendingMatchId)`.
 */
export function shouldAutoLookupAllocations(
  submission: PendingMatchSubmissionRecord,
  storeState: Pick<BlotterStoreData, 'sessionGeneration' | 'allocationsByAccount' | 'cursorSeq'>,
): boolean {
  if (submission.sessionGeneration !== storeState.sessionGeneration) {
    return false;
  }
  if (!actionExpectsAllocations(submission.action)) {
    return false;
  }
  if (submission.committedSeq < 1n) {
    return false;
  }
  if (submission.lookupStatus !== 'idle') {
    return false;
  }
  if (storeState.cursorSeq < submission.committedSeq) {
    return false;
  }
  const existing = selectExactTransactionAllocations(
    storeState,
    submission.accountId,
    submission.committedSeq,
    submission.pendingMatchId,
  );
  return existing.length === 0;
}
