import type {
  AccountBlotterSnapshot,
  BlotterEventEnvelope,
  InstrumentPositionView,
  PollEventsBatch,
  ReconcileAllocationView,
  RiskAndCoverageSummary,
  StreamHeartbeat,
} from '../generated/ficant/blotter/v1/model_pb';
import { BondTaxBasis } from '../generated/ficant/rates/v1/analytics_pb';
import { isNonEmptyTrimmed } from '../codec/primitives';
import { BlotterClientError } from '../transport/error';

export type EnvelopeIngestionSource = 'subscribe' | 'poll' | 'recovery';

export interface IndexedAllocation {
  readonly seq: bigint;
  readonly allocationId: string;
  readonly accountId: string;
  readonly view: ReconcileAllocationView;
}

export interface StagedEnvelope {
  readonly seq: bigint;
  readonly eventType: string;
  readonly snapshots: ReadonlyMap<string, AccountBlotterSnapshot>;
  readonly allocations: ReadonlyMap<string, readonly IndexedAllocation[]>;
}

export interface StagedPollBatch {
  readonly envelopes: readonly StagedEnvelope[];
  readonly nextCursorSeq: bigint;
  readonly headSeq: bigint;
  readonly hasMore: boolean;
}

export function validatePositionView(
  position: InstrumentPositionView,
  context: string,
): void {
  if (position.taxBasis !== BondTaxBasis.PRE_TAX) {
    throw BlotterClientError.protocol(
      `${context} failed position tax_basis validation: expected BOND_TAX_BASIS_PRE_TAX, got ${String(position.taxBasis)} for instrument "${position.instrumentCode}"`,
    );
  }
}

export function validateRiskSummary(
  summary: RiskAndCoverageSummary,
  context: string,
): void {
  if (summary.coveredTaxBasis !== BondTaxBasis.PRE_TAX) {
    throw BlotterClientError.protocol(
      `${context} failed covered_tax_basis validation: expected BOND_TAX_BASIS_PRE_TAX, got ${String(summary.coveredTaxBasis)}`,
    );
  }
  if (summary.uncoveredInstrumentCount > 0 && summary.wholeAccountHedgeAllowed) {
    throw BlotterClientError.protocol(
      `${context} failed whole_account_hedge_allowed validation: whole_account_hedge_allowed=true with uncovered_instrument_count=${summary.uncoveredInstrumentCount}`,
    );
  }
}

export function validateAccountSnapshot(
  snapshot: AccountBlotterSnapshot | undefined,
  context: string,
  expectedAccountId?: string,
): AccountBlotterSnapshot {
  if (!snapshot) {
    throw BlotterClientError.protocol(`${context} missing AccountBlotterSnapshot`);
  }
  if (!isNonEmptyTrimmed(snapshot.accountId)) {
    throw BlotterClientError.protocol(`${context} snapshot missing non-empty trimmed account_id`);
  }
  if (expectedAccountId !== undefined && snapshot.accountId !== expectedAccountId) {
    throw BlotterClientError.protocol(
      `${context} snapshot account_id mismatch: expected "${expectedAccountId}", got "${snapshot.accountId}"`,
    );
  }
  if (!snapshot.riskSummary) {
    throw BlotterClientError.protocol(
      `${context} snapshot for account "${snapshot.accountId}" missing risk_summary`,
    );
  }
  validateRiskSummary(snapshot.riskSummary, context);
  for (const position of snapshot.positions) {
    validatePositionView(position, context);
  }
  return snapshot;
}

function toIndexedAllocation(
  seq: bigint,
  accountId: string,
  view: ReconcileAllocationView,
  index: number,
): IndexedAllocation {
  const allocationId = view.allocationId?.value ?? `anon:${seq.toString()}:${index}`;
  return {
    seq,
    allocationId,
    accountId,
    view,
  };
}

/**
 * Deduplicates indexed allocations by `(seq, allocationId)` while preserving deterministic order.
 */
export function mergeDeduplicatedAllocations(
  existing: readonly IndexedAllocation[],
  incoming: readonly IndexedAllocation[],
): readonly IndexedAllocation[] {
  if (incoming.length === 0) {
    return existing;
  }
  const seen = new Set<string>();
  const merged: IndexedAllocation[] = [];
  for (const item of existing) {
    const key = `${item.seq.toString()}::${item.allocationId}`;
    if (!seen.has(key)) {
      seen.add(key);
      merged.push(item);
    }
  }
  for (const item of incoming) {
    const key = `${item.seq.toString()}::${item.allocationId}`;
    if (!seen.has(key)) {
      seen.add(key);
      merged.push(item);
    }
  }
  return merged;
}

/**
 * Unified whole-transaction envelope validator shared identically across
 * `SubscribeEvents`, `PollEvents`, and two-phase multi-account recovery replay.
 *
 * Parity with `python/sdk/src/ficant_sdk/blotter.py::_stage_envelope`.
 */
export function stageAndValidateEnvelope(
  envelope: BlotterEventEnvelope,
  source: EnvelopeIngestionSource,
): StagedEnvelope {
  const context = `Envelope[${source}]`;
  if (envelope.seq === 0n) {
    throw BlotterClientError.protocol(`${context} event envelope has invalid seq=0`);
  }

  const snapshots = new Map<string, AccountBlotterSnapshot>();
  const allocations = new Map<string, readonly IndexedAllocation[]>();

  for (const change of envelope.changes) {
    if (!isNonEmptyTrimmed(change.accountId)) {
      throw BlotterClientError.protocol(`${context} AccountChange missing non-empty account_id`);
    }
    if (snapshots.has(change.accountId)) {
      throw BlotterClientError.protocol(
        `${context} duplicate AccountChange for account_id="${change.accountId}" in seq=${envelope.seq.toString()}`,
      );
    }
    if (!change.replaceSnapshot) {
      throw BlotterClientError.protocol(
        `${context} AccountChange missing replace_snapshot for account_id="${change.accountId}"`,
      );
    }
    const snap = change.replaceSnapshot;
    if (snap.accountId !== change.accountId) {
      throw BlotterClientError.protocol(
        `${context} AccountChange account_id mismatch: change="${change.accountId}", snapshot="${snap.accountId}"`,
      );
    }
    if (snap.snapshotSeq !== envelope.seq) {
      throw BlotterClientError.protocol(
        `${context} AccountChange snapshot_seq mismatch: envelope.seq=${envelope.seq.toString()}, snapshot_seq=${snap.snapshotSeq.toString()}`,
      );
    }

    validateAccountSnapshot(snap, context, change.accountId);
    snapshots.set(change.accountId, snap);

    const indexed = change.changedAllocations.map((alloc, idx) =>
      toIndexedAllocation(envelope.seq, change.accountId, alloc, idx),
    );
    allocations.set(change.accountId, mergeDeduplicatedAllocations([], indexed));
  }

  return {
    seq: envelope.seq,
    eventType: envelope.eventType,
    snapshots,
    allocations,
  };
}

/**
 * Validates an entire `PollEventsBatch` atomically BEFORE any state or cursor mutation.
 * Enforces `startCursorSeq < event.seq <= scanned_through_seq <= current_head_seq`.
 */
export function stageAndValidatePollBatch(
  batch: PollEventsBatch,
  startCursorSeq: bigint,
  source: 'poll' | 'recovery',
): StagedPollBatch {
  const context = `PollBatch[${source}]`;
  if (batch.scannedThroughSeq < startCursorSeq) {
    throw BlotterClientError.protocol(
      `${context} seq regression: scanned_through_seq=${batch.scannedThroughSeq.toString()} < start_cursor=${startCursorSeq.toString()}`,
    );
  }
  if (batch.scannedThroughSeq > batch.currentHeadSeq) {
    throw BlotterClientError.protocol(
      `${context} out-of-range watermark: scanned_through_seq=${batch.scannedThroughSeq.toString()} > current_head_seq=${batch.currentHeadSeq.toString()}`,
    );
  }

  const envelopes: StagedEnvelope[] = [];
  let runningSeq = startCursorSeq;

  for (const envelope of batch.events) {
    if (envelope.seq <= runningSeq) {
      throw BlotterClientError.protocol(
        `${context} non-increasing event seq: got ${envelope.seq.toString()}, previous ${runningSeq.toString()}`,
      );
    }
    if (envelope.seq > batch.scannedThroughSeq) {
      throw BlotterClientError.protocol(
        `${context} event seq=${envelope.seq.toString()} exceeds scanned_through_seq=${batch.scannedThroughSeq.toString()}`,
      );
    }
    if (envelope.seq > batch.currentHeadSeq) {
      throw BlotterClientError.protocol(
        `${context} event seq=${envelope.seq.toString()} exceeds current_head_seq=${batch.currentHeadSeq.toString()}`,
      );
    }
    const staged = stageAndValidateEnvelope(envelope, source);
    envelopes.push(staged);
    runningSeq = envelope.seq;
  }

  const nextCursorSeq =
    batch.scannedThroughSeq > runningSeq ? batch.scannedThroughSeq : runningSeq;

  return {
    envelopes,
    nextCursorSeq,
    headSeq: batch.currentHeadSeq,
    hasMore: batch.hasMore,
  };
}

/**
 * Validates a `StreamHeartbeat` frame against the current workspace cursor.
 * Enforces `currentCursorSeq <= scanned_through_seq <= head_seq`.
 */
export function validateStreamHeartbeat(
  heartbeat: StreamHeartbeat,
  currentCursorSeq: bigint,
): { readonly nextCursorSeq: bigint; readonly headSeq: bigint } {
  if (heartbeat.scannedThroughSeq < currentCursorSeq) {
    throw BlotterClientError.protocol(
      `StreamHeartbeat seq regression: scanned_through_seq=${heartbeat.scannedThroughSeq.toString()} < cursor=${currentCursorSeq.toString()}`,
    );
  }
  if (heartbeat.scannedThroughSeq > heartbeat.headSeq) {
    throw BlotterClientError.protocol(
      `StreamHeartbeat out-of-range watermark: scanned_through_seq=${heartbeat.scannedThroughSeq.toString()} > head_seq=${heartbeat.headSeq.toString()}`,
    );
  }
  return {
    nextCursorSeq: heartbeat.scannedThroughSeq,
    headSeq: heartbeat.headSeq,
  };
}
