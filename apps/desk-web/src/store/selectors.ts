import type {
  AccountBlotterSnapshot,
  FeedStatusView,
  FundingGapSummary,
} from '../generated/ficant/blotter/v1/model_pb';
import type { BlotterStoreData, ConnectionHealthState } from './blotterStore';
import type { IndexedAllocation } from './envelopeValidator';

export interface FundingCompletenessFactors {
  readonly fundingIsCompleteExact: boolean;
  readonly cleanPriceEstimatedFundingCount: number;
  readonly unpricedFundingEarmarkCount: number;
  readonly pendingMatchCount: number;
  readonly pendingUnknownCashCount: number;
  readonly feedIncomplete: boolean;
}

export function selectAccountSnapshot(
  state: BlotterStoreData,
  accountId: string,
): AccountBlotterSnapshot | undefined {
  return state.snapshots[accountId];
}

export function selectSelectedSnapshot(
  state: BlotterStoreData,
): AccountBlotterSnapshot | undefined {
  if (state.selectedAccountId === null) {
    return undefined;
  }
  return state.snapshots[state.selectedAccountId];
}

export function selectLoadedAccountIds(state: BlotterStoreData): readonly string[] {
  return Object.keys(state.snapshots);
}

export function selectAccountAllocations(
  state: BlotterStoreData,
  accountId: string,
): readonly IndexedAllocation[] {
  return state.allocationsByAccount[accountId] ?? [];
}

export function selectAllocationsForTransaction(
  state: BlotterStoreData,
  accountId: string,
  committedSeq: bigint,
  pendingMatchId?: string,
): readonly IndexedAllocation[] {
  const all = state.allocationsByAccount[accountId] ?? [];
  return all.filter(
    (item) =>
      item.seq === committedSeq &&
      (pendingMatchId === undefined || item.view.pendingMatchId?.value === pendingMatchId),
  );
}

/**
 * Dynamically reads `snapshot.allowed_hedge_product_codes` (zero hardcoded product lists).
 */
export function selectAllowedHedgeProductCodes(
  state: BlotterStoreData,
  accountId: string,
): readonly string[] {
  return state.snapshots[accountId]?.allowedHedgeProductCodes ?? [];
}

/**
 * Strictly driven by `uncovered_instrument_count > 0` (even when net uncovered notional == 0 or < 0).
 */
export function selectIsWholeAccountHedgeBlocked(
  state: BlotterStoreData,
  accountId: string,
): boolean {
  const summary = state.snapshots[accountId]?.riskSummary;
  if (!summary) {
    return true;
  }
  return summary.uncoveredInstrumentCount > 0;
}

export function selectIsWritesPaused(state: BlotterStoreData): boolean {
  return !state.authenticated || state.recoveryPausedWrites;
}

export function selectConnectionHealth(state: BlotterStoreData): {
  readonly connectionState: ConnectionHealthState;
  readonly recoveryPausedWrites: boolean;
  readonly recoveryReason: string | null;
  readonly cursorSeq: bigint;
  readonly knownHeadSeq: bigint;
} {
  return {
    connectionState: state.connectionState,
    recoveryPausedWrites: state.recoveryPausedWrites,
    recoveryReason: state.recoveryReason,
    cursorSeq: state.cursorSeq,
    knownHeadSeq: state.knownHeadSeq,
  };
}

export function selectFeedStatuses(
  state: BlotterStoreData,
  accountId: string,
): readonly FeedStatusView[] {
  return state.snapshots[accountId]?.feedStatuses ?? [];
}

export function selectFundingCompletenessFactors(
  funding: FundingGapSummary | undefined,
): FundingCompletenessFactors | null {
  if (!funding) {
    return null;
  }
  return {
    fundingIsCompleteExact: funding.fundingIsCompleteExact,
    cleanPriceEstimatedFundingCount: funding.cleanPriceEstimatedFundingCount,
    unpricedFundingEarmarkCount: funding.unpricedFundingEarmarkCount,
    pendingMatchCount: funding.pendingMatchCount,
    pendingUnknownCashCount: funding.pendingUnknownCashCount,
    feedIncomplete: funding.feedIncomplete,
  };
}
