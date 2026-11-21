import { create } from '@bufbuild/protobuf';
import {
  BaselineDeltaSchema,
  EarmarkState,
  PendingMatchReason,
  PendingMatchResolutionAction,
  PendingMatchState,
  SettlementBucket,
  TradeSide,
  type AccountBlotterSnapshot,
  type BaseEvidence,
  type EarmarkRecord,
  type FeedStatusView,
  type InstrumentPositionView,
  type PendingMatchRecord,
} from '../../generated/ficant/blotter/v1/model_pb';
import {
  ResolvePendingMatchRequestSchema,
  type ResolvePendingMatchRequest,
} from '../../generated/ficant/blotter/v1/service_pb';
import type { DecimalValue, UnitRef } from '../../generated/ficant/core/v1/common_pb';
import type { AnalysisUnits } from '../../generated/ficant/rates/v1/analytics_pb';
import {
  MAX_DECIMAL_SCALE,
  createDecimalValue,
  formatDecimalValue,
  parseDecimalString,
  validateWireDecimalValue,
} from '../../codec/decimal';
import {
  isNonEmptyTrimmed,
  validateMarketTime,
  validateSha256,
} from '../../codec/primitives';
import {
  validateResolvePendingMatchRequest,
  type ValidationIssue,
} from '../../codec/validators';

export const UNKNOWN_SERVER_CASH_IMPACT_LABEL = '未知（等待服务端成交序号撮合确认）';
export const NO_ALLOCATION_CASH_IMPACT_LABEL = '无分摊（释放/拒绝不产生现金分摊）';

export const ALL_RESOLUTION_ACTIONS: readonly PendingMatchResolutionAction[] = [
  PendingMatchResolutionAction.MATCH_FULL,
  PendingMatchResolutionAction.SPLIT_OFFSET,
  PendingMatchResolutionAction.ACCEPT_ORPHAN,
  PendingMatchResolutionAction.RELEASE_REJECT,
  PendingMatchResolutionAction.APPLY_BASELINE_DELTA,
];

export interface ActionAvailability {
  readonly action: PendingMatchResolutionAction;
  readonly allowed: boolean;
  readonly disabledReason: string | null;
}

export interface PendingMatchGateContext {
  readonly position: InstrumentPositionView | undefined;
  readonly sourceFeed: FeedStatusView | undefined;
  readonly hasActiveSourceGap: boolean;
  readonly isSelfRevisionHold: boolean;
  readonly isBaselineRevisionManual: boolean;
  readonly hasSameKeyRevisionHold: boolean;
  readonly eligibleCommittedEarmarks: readonly EarmarkRecord[];
  readonly fullMatchEarmarks: readonly EarmarkRecord[];
  readonly baselineEvidenceIssue: string | null;
}

export interface PreSubmitPreview {
  readonly currentUnresolvedNotionalText: string;
  readonly currentKnownPendingCashText: string;
  readonly projectedOffsetNotionalText: string | null;
  readonly projectedPendingRemainingNotionalText: string | null;
  readonly projectedEarmarkRemainingNotionalText: string | null;
  readonly projectedBaselineDeltaNotionalText: string | null;
  readonly projectedBaselineDeltaCashText: string | null;
  readonly cashImpactKind: 'unknown_server_reconcile' | 'no_allocation' | 'baseline_delta_input';
  readonly cashImpactText: string;
}

export interface ResolvePendingMatchFormInput {
  readonly action: PendingMatchResolutionAction;
  readonly targetEarmarkId: string;
  readonly offsetNotionalInput: string;
  readonly reasonInput: string;
  readonly baselineDeltaNotionalInput: string;
  readonly baselineDeltaCashInput: string;
  readonly baselineDeltaReasonInput: string;
  readonly baselineDeltaBucket?: SettlementBucket | undefined;
}

export type BuildResolveRequestResult =
  | {
      readonly ok: true;
      readonly request: ResolvePendingMatchRequest;
      readonly canonicalPayload: Record<string, unknown>;
    }
  | {
      readonly ok: false;
      readonly issues: readonly ValidationIssue[];
    };

export function formatTradeSideLabel(side: TradeSide): string {
  switch (side) {
    case TradeSide.BUY:
      return '买入 (BUY)';
    case TradeSide.SELL:
      return '卖出 (SELL)';
    default:
      return '未知方向';
  }
}

export function formatSettlementBucketLabel(bucket: SettlementBucket): string {
  switch (bucket) {
    case SettlementBucket.T_PLUS_0:
      return 'T+0';
    case SettlementBucket.T_PLUS_1:
      return 'T+1';
    default:
      return '未指定桶';
  }
}

export function formatPendingMatchReasonLabel(reason: PendingMatchReason): string {
  switch (reason) {
    case PendingMatchReason.AMBIGUOUS_CANDIDATES:
      return '多候选歧义 (AMBIGUOUS_CANDIDATES)';
    case PendingMatchReason.NOTIONAL_MISMATCH_OR_SPLIT:
      return '面额不等/拆单 (NOTIONAL_MISMATCH_OR_SPLIT)';
    case PendingMatchReason.PRICE_OUT_OF_TOLERANCE:
      return '价格超容差 (PRICE_OUT_OF_TOLERANCE)';
    case PendingMatchReason.ORPHAN_NO_COMMITMENT:
      return '无对应占仓 (ORPHAN_NO_COMMITMENT)';
    case PendingMatchReason.BURST_REPEAT:
      return '短时重复成交 (BURST_REPEAT)';
    case PendingMatchReason.EXISTING_PENDING_ON_KEY:
      return '同键已有待核销 (EXISTING_PENDING_ON_KEY)';
    case PendingMatchReason.AMENDMENT_MISMATCH:
      return '改单不匹配 (AMENDMENT_MISMATCH)';
    case PendingMatchReason.BASELINE_REVISION_MANUAL:
      return '基线手工修订 (BASELINE_REVISION_MANUAL)';
    case PendingMatchReason.INCOMPARABLE_BASIS:
      return '报价口径不可比 (INCOMPARABLE_BASIS)';
    case PendingMatchReason.REVISION_HOLD:
      return '修订暂挂 (REVISION_HOLD)';
    case PendingMatchReason.GAP_HOLD:
      return '源缺段暂挂 (GAP_HOLD)';
    default:
      return '未知原因';
  }
}

export function formatPendingMatchStateLabel(state: PendingMatchState): string {
  switch (state) {
    case PendingMatchState.OPEN:
      return '待核销 (OPEN)';
    case PendingMatchState.RESOLVED_MATCHED:
      return '已核销匹配 (RESOLVED_MATCHED)';
    case PendingMatchState.RESOLVED_ORPHAN_ACCEPTED:
      return '已按孤儿成交入账 (RESOLVED_ORPHAN_ACCEPTED)';
    case PendingMatchState.RELEASED:
      return '已释放/拒绝 (RELEASED)';
    case PendingMatchState.REVERSED_BY_FEED:
      return '已被成交源冲销 (REVERSED_BY_FEED)';
    case PendingMatchState.RESOLVED_BASELINE_DELTA:
      return '已确认基线差额 (RESOLVED_BASELINE_DELTA)';
    default:
      return '未知状态';
  }
}

export function formatResolutionActionLabel(action: PendingMatchResolutionAction): string {
  switch (action) {
    case PendingMatchResolutionAction.MATCH_FULL:
      return '全额匹配 (MATCH_FULL)';
    case PendingMatchResolutionAction.SPLIT_OFFSET:
      return '拆分冲抵 (SPLIT_OFFSET)';
    case PendingMatchResolutionAction.ACCEPT_ORPHAN:
      return '接受孤儿成交 (ACCEPT_ORPHAN)';
    case PendingMatchResolutionAction.RELEASE_REJECT:
      return '释放/拒绝 (RELEASE_REJECT)';
    case PendingMatchResolutionAction.APPLY_BASELINE_DELTA:
      return '应用基线差额 (APPLY_BASELINE_DELTA)';
    default:
      return '未指定动作';
  }
}

/**
 * Formats a `10^12`-scaled `bigint` into an exact canonical decimal string without ever using IEEE-754 `Number`.
 */
export function formatScaledI128(scaled: bigint, grouping = true): string {
  const negative = scaled < 0n;
  const absVal = negative ? -scaled : scaled;
  const padded = absVal.toString().padStart(MAX_DECIMAL_SCALE + 1, '0');
  const splitAt = padded.length - MAX_DECIMAL_SCALE;
  const rawInt = padded.slice(0, splitAt);
  let rawFrac = padded.slice(splitAt);
  while (rawFrac.length > 0 && rawFrac.endsWith('0')) {
    rawFrac = rawFrac.slice(0, -1);
  }
  const intText = grouping ? rawInt.replace(/\B(?=(\d{3})+(?!\d))/g, ',') : rawInt;
  const sign = negative && absVal !== 0n ? '-' : '';
  return rawFrac.length > 0 ? `${sign}${intText}.${rawFrac}` : `${sign}${intText}`;
}

export function tryReadScaledI128(value: DecimalValue | undefined): bigint | null {
  if (value === undefined) {
    return null;
  }
  try {
    return validateWireDecimalValue(value, { requireUnit: false }).scaledI128;
  } catch {
    return null;
  }
}

export function formatOptionalSignedDecimal(
  value: DecimalValue | undefined,
  missingText = '未知',
): string {
  if (value === undefined) {
    return missingText;
  }
  try {
    return formatDecimalValue(value, { grouping: true, missingText });
  } catch {
    return missingText;
  }
}

/**
 * Four-key `K = (account_id, instrument_code, side, settlement_date)` equality check matching `reconcile.rs`.
 */
export function matchesFourKey(
  lhs: {
    readonly accountId: string;
    readonly instrumentCode: string;
    readonly side: TradeSide;
    readonly settlementDate: string;
  },
  rhs: {
    readonly accountId: string;
    readonly instrumentCode: string;
    readonly side: TradeSide;
    readonly settlementDate: string;
  },
): boolean {
  return (
    lhs.accountId === rhs.accountId &&
    lhs.instrumentCode === rhs.instrumentCode &&
    lhs.side === rhs.side &&
    lhs.settlementDate === rhs.settlementDate
  );
}

/**
 * Returns active `COMMITTED` earmarks matching four-key `K` with strictly positive `unreconciled_notional`.
 * Candidate earmarks referenced on the pending match are sorted first while keeping deterministic order.
 */
export function getEligibleCommittedEarmarks(
  snapshot: AccountBlotterSnapshot,
  pm: PendingMatchRecord,
): readonly EarmarkRecord[] {
  const candidateSet = new Set(pm.candidateEarmarkIds.map((u) => u.value));
  const matched = snapshot.activeEarmarks.filter((earmark) => {
    if (earmark.state !== EarmarkState.COMMITTED) {
      return false;
    }
    if (!matchesFourKey(pm, earmark)) {
      return false;
    }
    const rem = tryReadScaledI128(earmark.unreconciledNotional);
    return rem !== null && rem > 0n;
  });

  return [...matched].sort((a, b) => {
    const aCand = candidateSet.has(a.earmarkId?.value ?? '') ? 0 : 1;
    const bCand = candidateSet.has(b.earmarkId?.value ?? '') ? 0 : 1;
    if (aCand !== bCand) {
      return aCand - bCand;
    }
    const aId = a.earmarkId?.value ?? '';
    const bId = b.earmarkId?.value ?? '';
    return aId < bId ? -1 : aId > bId ? 1 : 0;
  });
}

/**
 * Validates that `pm.evidence` is present, structurally complete, and matches `pm`'s instrument,
 * side, and settlement bucket (mirroring `reconcile.rs:2044-2069`).
 */
export function validatePendingBaselineEvidence(pm: PendingMatchRecord): string | null {
  const ev: BaseEvidence | undefined = pm.evidence;
  if (!ev) {
    return '缺少已验证的原始基线证据 BaseEvidence，无法提交 APPLY_BASELINE_DELTA';
  }
  if (!isNonEmptyTrimmed(ev.sourceId)) {
    return '基线证据缺少有效的 source_id';
  }
  if (!isNonEmptyTrimmed(ev.externalTradeId)) {
    return '基线证据缺少有效的 external_trade_id';
  }
  if (!isNonEmptyTrimmed(ev.evidenceRef)) {
    return '基线证据缺少有效的 evidence_ref';
  }
  if (!isNonEmptyTrimmed(ev.instrumentCode)) {
    return '基线证据缺少有效的 instrument_code';
  }
  if (ev.originalRevision < 1n) {
    return '基线证据 original_revision 必须 >= 1';
  }
  if (
    ev.settlementBucket !== SettlementBucket.T_PLUS_0 &&
    ev.settlementBucket !== SettlementBucket.T_PLUS_1
  ) {
    return '基线证据 settlement_bucket 无效';
  }
  try {
    validateSha256(ev.evidenceHash, 'evidence_hash');
  } catch {
    return '基线证据 evidence_hash 必须为 32 字节 SHA-256';
  }
  try {
    validateMarketTime(ev.originalExecutedAt, 'original_executed_at');
  } catch {
    return '基线证据 original_executed_at 无效';
  }
  if (
    ev.instrumentCode !== pm.instrumentCode ||
    ev.side !== pm.side ||
    ev.settlementBucket !== pm.settlementBucket
  ) {
    return '跨券/跨方向/跨结算桶基线修订不能通过单桶 APPLY_BASELINE_DELTA 确认';
  }
  return null;
}

export function computePendingMatchGateContext(
  snapshot: AccountBlotterSnapshot,
  pm: PendingMatchRecord,
): PendingMatchGateContext {
  const position = snapshot.positions.find((p) => p.instrumentCode === pm.instrumentCode);
  const sourceFeed = snapshot.feedStatuses.find((f) => f.sourceId === pm.sourceId);
  const hasActiveSourceGap = sourceFeed !== undefined && sourceFeed.missingFeedRanges.length > 0;
  const isSelfRevisionHold = pm.reason === PendingMatchReason.REVISION_HOLD;
  const isBaselineRevisionManual = pm.reason === PendingMatchReason.BASELINE_REVISION_MANUAL;

  const hasSameKeyRevisionHold = snapshot.openPendingMatches.some(
    (other) =>
      other.state === PendingMatchState.OPEN &&
      other.reason === PendingMatchReason.REVISION_HOLD &&
      matchesFourKey(other, pm),
  );

  const eligibleCommittedEarmarks = getEligibleCommittedEarmarks(snapshot, pm);
  const pmUnresolved = tryReadScaledI128(pm.unresolvedNotional);
  const fullMatchEarmarks =
    pmUnresolved !== null && pmUnresolved > 0n
      ? eligibleCommittedEarmarks.filter((e) => {
          const rem = tryReadScaledI128(e.unreconciledNotional);
          return rem !== null && rem >= pmUnresolved;
        })
      : [];

  const baselineEvidenceIssue = isBaselineRevisionManual
    ? validatePendingBaselineEvidence(pm)
    : null;

  return {
    position,
    sourceFeed,
    hasActiveSourceGap,
    isSelfRevisionHold,
    isBaselineRevisionManual,
    hasSameKeyRevisionHold,
    eligibleCommittedEarmarks,
    fullMatchEarmarks,
    baselineEvidenceIssue,
  };
}

/**
 * Evaluates action availability strictly matching `crates/ficant-domain/src/blotter/reconcile.rs`:
 * - First 4 actions (`MATCH_FULL`, `SPLIT_OFFSET`, `ACCEPT_ORPHAN`, `RELEASE_REJECT`) are blocked
 *   on non-OPEN state, self `REVISION_HOLD`, `BASELINE_REVISION_MANUAL`, same-four-key active
 *   `REVISION_HOLD`, or active source `missing_feed_ranges`.
 * - `APPLY_BASELINE_DELTA` is ONLY valid for `BASELINE_REVISION_MANUAL`, and remains legally
 *   ALLOWED even while an active `sourceGap` exists whenever original `BaseEvidence` is valid
 *   for the same bucket and key!
 */
export function evaluateActionAvailability(
  action: PendingMatchResolutionAction,
  pm: PendingMatchRecord,
  gate: PendingMatchGateContext,
  writesPaused: boolean,
): ActionAvailability {
  if (writesPaused) {
    return {
      action,
      allowed: false,
      disabledReason: '工作区恢复中或未认证，依赖版本的核销写入已暂停',
    };
  }
  if (pm.state !== PendingMatchState.OPEN) {
    return {
      action,
      allowed: false,
      disabledReason: '仅 OPEN 状态的待核销记录允许人工核销',
    };
  }
  if (!gate.position) {
    return {
      action,
      allowed: false,
      disabledReason: `快照中缺少券种 ${pm.instrumentCode} 的持仓版本`,
    };
  }

  if (action === PendingMatchResolutionAction.APPLY_BASELINE_DELTA) {
    if (!gate.isBaselineRevisionManual) {
      return {
        action,
        allowed: false,
        disabledReason: 'APPLY_BASELINE_DELTA 仅适用于 BASELINE_REVISION_MANUAL 记录',
      };
    }
    if (gate.baselineEvidenceIssue !== null) {
      return {
        action,
        allowed: false,
        disabledReason: gate.baselineEvidenceIssue,
      };
    }
    return {
      action,
      allowed: true,
      disabledReason: null,
    };
  }

  // First four actions enforce `forbid_hold_or_baseline`.
  if (gate.isSelfRevisionHold) {
    return {
      action,
      allowed: false,
      disabledReason: '当前记录处于 REVISION_HOLD 修订暂挂，禁止此核销动作',
    };
  }
  if (gate.isBaselineRevisionManual) {
    return {
      action,
      allowed: false,
      disabledReason: 'BASELINE_REVISION_MANUAL 禁止前四项核销动作，请使用 APPLY_BASELINE_DELTA',
    };
  }
  if (gate.hasSameKeyRevisionHold) {
    return {
      action,
      allowed: false,
      disabledReason: '同四键存在活动 REVISION_HOLD 修订暂挂，暂停人工核销',
    };
  }
  if (gate.hasActiveSourceGap) {
    return {
      action,
      allowed: false,
      disabledReason: `所属成交源 ${pm.sourceId} 存在活动缺段 (missing_feed_ranges)，暂停此核销动作`,
    };
  }

  const pmUnresolved = tryReadScaledI128(pm.unresolvedNotional);

  if (action === PendingMatchResolutionAction.MATCH_FULL) {
    if (pmUnresolved === null || pmUnresolved <= 0n) {
      return {
        action,
        allowed: false,
        disabledReason: '待核销未决面额必须为正数方可全额匹配',
      };
    }
    if (gate.fullMatchEarmarks.length === 0) {
      return {
        action,
        allowed: false,
        disabledReason: '无同四键且剩余面额足以覆盖全部待核销额的 COMMITTED 占仓',
      };
    }
  } else if (action === PendingMatchResolutionAction.SPLIT_OFFSET) {
    if (pmUnresolved === null || pmUnresolved <= 0n) {
      return {
        action,
        allowed: false,
        disabledReason: '待核销未决面额必须为正数方可拆分冲抵',
      };
    }
    if (gate.eligibleCommittedEarmarks.length === 0) {
      return {
        action,
        allowed: false,
        disabledReason: '无同四键且剩余面额为正的 COMMITTED 占仓可供拆分冲抵',
      };
    }
  }

  return {
    action,
    allowed: true,
    disabledReason: null,
  };
}

export function evaluateAllActionsAvailability(
  pm: PendingMatchRecord,
  gate: PendingMatchGateContext,
  writesPaused: boolean,
): Readonly<Record<PendingMatchResolutionAction, ActionAvailability>> {
  const result: Record<PendingMatchResolutionAction, ActionAvailability> = {
    [PendingMatchResolutionAction.UNSPECIFIED]: {
      action: PendingMatchResolutionAction.UNSPECIFIED,
      allowed: false,
      disabledReason: '未指定核销动作',
    },
    [PendingMatchResolutionAction.MATCH_FULL]: evaluateActionAvailability(
      PendingMatchResolutionAction.MATCH_FULL,
      pm,
      gate,
      writesPaused,
    ),
    [PendingMatchResolutionAction.SPLIT_OFFSET]: evaluateActionAvailability(
      PendingMatchResolutionAction.SPLIT_OFFSET,
      pm,
      gate,
      writesPaused,
    ),
    [PendingMatchResolutionAction.ACCEPT_ORPHAN]: evaluateActionAvailability(
      PendingMatchResolutionAction.ACCEPT_ORPHAN,
      pm,
      gate,
      writesPaused,
    ),
    [PendingMatchResolutionAction.RELEASE_REJECT]: evaluateActionAvailability(
      PendingMatchResolutionAction.RELEASE_REJECT,
      pm,
      gate,
      writesPaused,
    ),
    [PendingMatchResolutionAction.APPLY_BASELINE_DELTA]: evaluateActionAvailability(
      PendingMatchResolutionAction.APPLY_BASELINE_DELTA,
      pm,
      gate,
      writesPaused,
    ),
  };
  return result;
}

export function pickDefaultAction(
  pm: PendingMatchRecord,
  availability: Readonly<Record<PendingMatchResolutionAction, ActionAvailability>>,
): PendingMatchResolutionAction {
  if (pm.reason === PendingMatchReason.BASELINE_REVISION_MANUAL) {
    return PendingMatchResolutionAction.APPLY_BASELINE_DELTA;
  }
  for (const action of ALL_RESOLUTION_ACTIONS) {
    if (availability[action].allowed) {
      return action;
    }
  }
  return PendingMatchResolutionAction.MATCH_FULL;
}

/**
 * Computes ONLY deterministic pre-submit notional math and known pending cash.
 * NEVER fabricates post-split bucket cash or server AllocationPool slices client-side!
 */
export function computePreSubmitPreview(
  pm: PendingMatchRecord,
  form: ResolvePendingMatchFormInput,
  selectedEarmark: EarmarkRecord | undefined,
): PreSubmitPreview {
  const pmUnresolved = tryReadScaledI128(pm.unresolvedNotional);
  const currentUnresolvedNotionalText =
    pmUnresolved !== null ? formatScaledI128(pmUnresolved, true) : '未知';
  const currentKnownPendingCashText = formatOptionalSignedDecimal(
    pm.unresolvedSignedCash,
    '未知',
  );

  switch (form.action) {
    case PendingMatchResolutionAction.MATCH_FULL: {
      const earmarkRem = tryReadScaledI128(selectedEarmark?.unreconciledNotional);
      const validCapacity =
        pmUnresolved !== null &&
        pmUnresolved > 0n &&
        earmarkRem !== null &&
        earmarkRem >= pmUnresolved;
      return {
        currentUnresolvedNotionalText,
        currentKnownPendingCashText,
        projectedOffsetNotionalText:
          pmUnresolved !== null ? formatScaledI128(pmUnresolved, true) : null,
        projectedPendingRemainingNotionalText: pmUnresolved !== null ? '0' : null,
        projectedEarmarkRemainingNotionalText: validCapacity
          ? formatScaledI128(earmarkRem - pmUnresolved, true)
          : null,
        projectedBaselineDeltaNotionalText: null,
        projectedBaselineDeltaCashText: null,
        cashImpactKind: 'unknown_server_reconcile',
        cashImpactText: UNKNOWN_SERVER_CASH_IMPACT_LABEL,
      };
    }

    case PendingMatchResolutionAction.SPLIT_OFFSET: {
      const earmarkRem = tryReadScaledI128(selectedEarmark?.unreconciledNotional);
      let parsedOffset: bigint | null = null;
      const trimmedOffset = form.offsetNotionalInput.trim();
      if (trimmedOffset.length > 0) {
        try {
          const parts = parseDecimalString(trimmedOffset, 'offset_notional');
          if (parts.scaledI128 > 0n) {
            parsedOffset = parts.scaledI128;
          }
        } catch {
          parsedOffset = null;
        }
      }

      const projectedPendingRemainingNotionalText =
        parsedOffset !== null && pmUnresolved !== null && parsedOffset <= pmUnresolved
          ? formatScaledI128(pmUnresolved - parsedOffset, true)
          : null;
      const projectedEarmarkRemainingNotionalText =
        parsedOffset !== null && earmarkRem !== null && parsedOffset <= earmarkRem
          ? formatScaledI128(earmarkRem - parsedOffset, true)
          : null;

      return {
        currentUnresolvedNotionalText,
        currentKnownPendingCashText,
        projectedOffsetNotionalText:
          parsedOffset !== null ? formatScaledI128(parsedOffset, true) : null,
        projectedPendingRemainingNotionalText,
        projectedEarmarkRemainingNotionalText,
        projectedBaselineDeltaNotionalText: null,
        projectedBaselineDeltaCashText: null,
        cashImpactKind: 'unknown_server_reconcile',
        cashImpactText: UNKNOWN_SERVER_CASH_IMPACT_LABEL,
      };
    }

    case PendingMatchResolutionAction.ACCEPT_ORPHAN: {
      return {
        currentUnresolvedNotionalText,
        currentKnownPendingCashText,
        projectedOffsetNotionalText:
          pmUnresolved !== null ? formatScaledI128(pmUnresolved, true) : null,
        projectedPendingRemainingNotionalText: pmUnresolved !== null ? '0' : null,
        projectedEarmarkRemainingNotionalText: null,
        projectedBaselineDeltaNotionalText: null,
        projectedBaselineDeltaCashText: null,
        cashImpactKind: 'unknown_server_reconcile',
        cashImpactText: UNKNOWN_SERVER_CASH_IMPACT_LABEL,
      };
    }

    case PendingMatchResolutionAction.RELEASE_REJECT: {
      return {
        currentUnresolvedNotionalText,
        currentKnownPendingCashText,
        projectedOffsetNotionalText: '0',
        projectedPendingRemainingNotionalText: '0',
        projectedEarmarkRemainingNotionalText: null,
        projectedBaselineDeltaNotionalText: null,
        projectedBaselineDeltaCashText: null,
        cashImpactKind: 'no_allocation',
        cashImpactText: NO_ALLOCATION_CASH_IMPACT_LABEL,
      };
    }

    case PendingMatchResolutionAction.APPLY_BASELINE_DELTA: {
      let deltaNotionalText: string | null = null;
      const trimmedN = form.baselineDeltaNotionalInput.trim();
      if (trimmedN.length > 0) {
        try {
          deltaNotionalText = formatScaledI128(
            parseDecimalString(trimmedN, 'baseline_delta.delta_notional').scaledI128,
            true,
          );
        } catch {
          deltaNotionalText = null;
        }
      }

      let deltaCashText: string | null = null;
      const trimmedC = form.baselineDeltaCashInput.trim();
      if (trimmedC.length > 0) {
        try {
          deltaCashText = formatScaledI128(
            parseDecimalString(trimmedC, 'baseline_delta.delta_cash').scaledI128,
            true,
          );
        } catch {
          deltaCashText = null;
        }
      }

      return {
        currentUnresolvedNotionalText,
        currentKnownPendingCashText,
        projectedOffsetNotionalText: null,
        projectedPendingRemainingNotionalText: '0',
        projectedEarmarkRemainingNotionalText: null,
        projectedBaselineDeltaNotionalText: deltaNotionalText,
        projectedBaselineDeltaCashText: deltaCashText,
        cashImpactKind:
          deltaCashText !== null ? 'baseline_delta_input' : 'unknown_server_reconcile',
        cashImpactText:
          deltaCashText !== null
            ? `${deltaCashText}（待服务端提交序号确认）`
            : UNKNOWN_SERVER_CASH_IMPACT_LABEL,
      };
    }

    default: {
      return {
        currentUnresolvedNotionalText,
        currentKnownPendingCashText,
        projectedOffsetNotionalText: null,
        projectedPendingRemainingNotionalText: null,
        projectedEarmarkRemainingNotionalText: null,
        projectedBaselineDeltaNotionalText: null,
        projectedBaselineDeltaCashText: null,
        cashImpactKind: 'unknown_server_reconcile',
        cashImpactText: UNKNOWN_SERVER_CASH_IMPACT_LABEL,
      };
    }
  }
}

function resolveCurrencyUnit(
  snapshot: AccountBlotterSnapshot,
  pm: PendingMatchRecord,
): UnitRef | undefined {
  return (
    snapshot.openingBase?.units?.currencyAmount ??
    pm.unresolvedNotional?.unit ??
    pm.originalNotional?.unit ??
    pm.evidence?.oldNotional?.unit
  );
}

/**
 * Builds and strictly validates a `ResolvePendingMatchRequest` matching `reconcile.rs` and
 * `validateResolvePendingMatchRequest`, guaranteeing that forbidden fields for each action
 * are NEVER populated on the wire.
 */
export function buildResolvePendingMatchRequest(params: {
  readonly snapshot: AccountBlotterSnapshot;
  readonly pendingMatch: PendingMatchRecord;
  readonly form: ResolvePendingMatchFormInput;
  readonly writesPaused: boolean;
  readonly idempotencyKey?: string | undefined;
}): BuildResolveRequestResult {
  const { snapshot, pendingMatch: pm, form, writesPaused } = params;
  const issues: ValidationIssue[] = [];

  if (pm.accountId !== snapshot.accountId) {
    issues.push({
      field: 'account_id',
      message: `待核销账户 "${pm.accountId}" 与当前快照账户 "${snapshot.accountId}" 不一致`,
    });
  }

  const gate = computePendingMatchGateContext(snapshot, pm);
  const availability = evaluateActionAvailability(form.action, pm, gate, writesPaused);
  if (!availability.allowed) {
    issues.push({
      field: 'action',
      message: availability.disabledReason ?? '当前动作不可执行',
    });
    return { ok: false, issues };
  }

  const position = gate.position;
  if (!position) {
    issues.push({
      field: 'expected_position_version',
      message: `缺少券种 ${pm.instrumentCode} 的持仓版本`,
    });
    return { ok: false, issues };
  }

  if (!pm.pendingMatchId) {
    issues.push({ field: 'pending_match_id', message: '缺少待核销记录 ULID' });
    return { ok: false, issues };
  }

  const units: AnalysisUnits | undefined = snapshot.openingBase?.units;
  const currencyUnit = resolveCurrencyUnit(snapshot, pm);
  const trimmedReason = form.reasonInput.trim();
  const pmUnresolvedScaled = tryReadScaledI128(pm.unresolvedNotional);

  const findTargetEarmark = (): EarmarkRecord | undefined => {
    const id = form.targetEarmarkId.trim();
    if (id.length === 0) {
      return undefined;
    }
    return snapshot.activeEarmarks.find((e) => e.earmarkId?.value === id);
  };

  let request: ResolvePendingMatchRequest;
  let canonicalPayload: Record<string, unknown>;

  switch (form.action) {
    case PendingMatchResolutionAction.MATCH_FULL: {
      const target = findTargetEarmark();
      if (!target || !target.earmarkId) {
        issues.push({
          field: 'target_earmark_id',
          message: 'MATCH_FULL 必须选择一个同四键的 COMMITTED 占仓',
        });
        return { ok: false, issues };
      }
      if (target.state !== EarmarkState.COMMITTED) {
        issues.push({
          field: 'target_earmark_id',
          message: '目标占仓必须处于 COMMITTED 状态',
        });
      }
      if (!matchesFourKey(pm, target)) {
        issues.push({
          field: 'target_earmark_id',
          message: '目标占仓四键 (account, instrument, side, settlement_date) 与待核销记录不匹配',
        });
      }
      const earmarkRem = tryReadScaledI128(target.unreconciledNotional);
      if (pmUnresolvedScaled === null || pmUnresolvedScaled <= 0n) {
        issues.push({
          field: 'unresolved_notional',
          message: '待核销未决面额必须严格大于 0',
        });
      } else if (earmarkRem === null || earmarkRem < pmUnresolvedScaled) {
        issues.push({
          field: 'target_earmark_id',
          message: '目标占仓剩余未核销面额不足以覆盖全部待核销面额，请改用 SPLIT_OFFSET',
        });
      }
      if (issues.length > 0) {
        return { ok: false, issues };
      }

      // Explicitly OMIT offsetNotional and baselineDelta!
      request = create(ResolvePendingMatchRequestSchema, {
        idempotencyKey: params.idempotencyKey ?? '',
        accountId: pm.accountId,
        pendingMatchId: pm.pendingMatchId,
        expectedPendingRevision: pm.pendingRevision,
        expectedPositionVersion: position.positionVersion,
        action: PendingMatchResolutionAction.MATCH_FULL,
        targetEarmarkId: target.earmarkId,
        expectedEarmarkRevision: target.earmarkRevision,
        reason: trimmedReason,
      });
      canonicalPayload = {
        accountId: pm.accountId,
        pendingMatchId: pm.pendingMatchId.value,
        expectedPendingRevision: pm.pendingRevision,
        expectedPositionVersion: position.positionVersion,
        action: PendingMatchResolutionAction.MATCH_FULL,
        targetEarmarkId: target.earmarkId.value,
        expectedEarmarkRevision: target.earmarkRevision,
        reason: trimmedReason,
      };
      break;
    }

    case PendingMatchResolutionAction.SPLIT_OFFSET: {
      const target = findTargetEarmark();
      if (!target || !target.earmarkId) {
        issues.push({
          field: 'target_earmark_id',
          message: 'SPLIT_OFFSET 必须选择一个同四键的 COMMITTED 占仓',
        });
        return { ok: false, issues };
      }
      if (target.state !== EarmarkState.COMMITTED) {
        issues.push({
          field: 'target_earmark_id',
          message: '目标占仓必须处于 COMMITTED 状态',
        });
      }
      if (!matchesFourKey(pm, target)) {
        issues.push({
          field: 'target_earmark_id',
          message: '目标占仓四键 (account, instrument, side, settlement_date) 与待核销记录不匹配',
        });
      }
      if (!currencyUnit) {
        issues.push({
          field: 'offset_notional',
          message: '账户缺少 currency_amount 单位定义，无法编码冲抵面额',
        });
        return { ok: false, issues };
      }

      let offsetDecimal: DecimalValue | undefined;
      let offsetScaled: bigint | null = null;
      const trimmedOffset = form.offsetNotionalInput.trim();
      if (trimmedOffset.length === 0) {
        issues.push({ field: 'offset_notional', message: 'SPLIT_OFFSET 必须输入正数冲抵面额' });
      } else {
        try {
          const parsed = parseDecimalString(trimmedOffset, 'offset_notional');
          if (parsed.scaledI128 <= 0n) {
            issues.push({ field: 'offset_notional', message: '冲抵面额必须严格大于 0' });
          } else {
            offsetScaled = parsed.scaledI128;
            offsetDecimal = createDecimalValue(parsed, currencyUnit, 'offset_notional');
          }
        } catch (err) {
          issues.push({
            field: 'offset_notional',
            message: err instanceof Error ? err.message : '冲抵面额格式非法',
          });
        }
      }

      const earmarkRem = tryReadScaledI128(target.unreconciledNotional);
      if (offsetScaled !== null) {
        if (pmUnresolvedScaled === null || offsetScaled > pmUnresolvedScaled) {
          issues.push({
            field: 'offset_notional',
            message: '冲抵面额不能超过待核销剩余面额',
          });
        }
        if (earmarkRem === null || offsetScaled > earmarkRem) {
          issues.push({
            field: 'offset_notional',
            message: '冲抵面额不能超过目标占仓剩余未核销面额',
          });
        }
      }

      if (issues.length > 0 || !offsetDecimal) {
        return { ok: false, issues };
      }

      // Explicitly OMIT baselineDelta!
      request = create(ResolvePendingMatchRequestSchema, {
        idempotencyKey: params.idempotencyKey ?? '',
        accountId: pm.accountId,
        pendingMatchId: pm.pendingMatchId,
        expectedPendingRevision: pm.pendingRevision,
        expectedPositionVersion: position.positionVersion,
        action: PendingMatchResolutionAction.SPLIT_OFFSET,
        targetEarmarkId: target.earmarkId,
        expectedEarmarkRevision: target.earmarkRevision,
        offsetNotional: offsetDecimal,
        reason: trimmedReason,
      });
      canonicalPayload = {
        accountId: pm.accountId,
        pendingMatchId: pm.pendingMatchId.value,
        expectedPendingRevision: pm.pendingRevision,
        expectedPositionVersion: position.positionVersion,
        action: PendingMatchResolutionAction.SPLIT_OFFSET,
        targetEarmarkId: target.earmarkId.value,
        expectedEarmarkRevision: target.earmarkRevision,
        offsetNotional: {
          coefficient: offsetDecimal.coefficient,
          scale: offsetDecimal.scale,
          unitId: offsetDecimal.unit?.unitId?.value,
          unitVersion: offsetDecimal.unit?.version,
        },
        reason: trimmedReason,
      };
      break;
    }

    case PendingMatchResolutionAction.ACCEPT_ORPHAN: {
      // Explicitly OMIT targetEarmarkId, expectedEarmarkRevision, offsetNotional, baselineDelta!
      request = create(ResolvePendingMatchRequestSchema, {
        idempotencyKey: params.idempotencyKey ?? '',
        accountId: pm.accountId,
        pendingMatchId: pm.pendingMatchId,
        expectedPendingRevision: pm.pendingRevision,
        expectedPositionVersion: position.positionVersion,
        action: PendingMatchResolutionAction.ACCEPT_ORPHAN,
        reason: trimmedReason,
      });
      canonicalPayload = {
        accountId: pm.accountId,
        pendingMatchId: pm.pendingMatchId.value,
        expectedPendingRevision: pm.pendingRevision,
        expectedPositionVersion: position.positionVersion,
        action: PendingMatchResolutionAction.ACCEPT_ORPHAN,
        reason: trimmedReason,
      };
      break;
    }

    case PendingMatchResolutionAction.RELEASE_REJECT: {
      if (trimmedReason.length === 0) {
        issues.push({
          field: 'reason',
          message: 'RELEASE_REJECT 必须填写非空原因',
        });
        return { ok: false, issues };
      }

      // Explicitly OMIT targetEarmarkId, expectedEarmarkRevision, offsetNotional, baselineDelta!
      request = create(ResolvePendingMatchRequestSchema, {
        idempotencyKey: params.idempotencyKey ?? '',
        accountId: pm.accountId,
        pendingMatchId: pm.pendingMatchId,
        expectedPendingRevision: pm.pendingRevision,
        expectedPositionVersion: position.positionVersion,
        action: PendingMatchResolutionAction.RELEASE_REJECT,
        reason: trimmedReason,
      });
      canonicalPayload = {
        accountId: pm.accountId,
        pendingMatchId: pm.pendingMatchId.value,
        expectedPendingRevision: pm.pendingRevision,
        expectedPositionVersion: position.positionVersion,
        action: PendingMatchResolutionAction.RELEASE_REJECT,
        reason: trimmedReason,
      };
      break;
    }

    case PendingMatchResolutionAction.APPLY_BASELINE_DELTA: {
      const evidence = pm.evidence;
      if (!evidence) {
        issues.push({
          field: 'baseline_delta.evidence',
          message: '缺少已验证的原始 BaseEvidence',
        });
        return { ok: false, issues };
      }
      if (!currencyUnit) {
        issues.push({
          field: 'baseline_delta',
          message: '账户缺少 currency_amount 单位定义，无法编码基线差额',
        });
        return { ok: false, issues };
      }

      const targetBucket = form.baselineDeltaBucket ?? evidence.settlementBucket;
      if (
        targetBucket !== evidence.settlementBucket ||
        targetBucket !== pm.settlementBucket
      ) {
        issues.push({
          field: 'baseline_delta.settlement_bucket',
          message: '基线差额结算桶必须与原证据及待核销记录同桶（禁止跨桶确认）',
        });
      }

      const trimmedDeltaReason = form.baselineDeltaReasonInput.trim();
      if (trimmedDeltaReason.length === 0) {
        issues.push({
          field: 'baseline_delta.reason',
          message: 'APPLY_BASELINE_DELTA 必须填写非空基线修订原因',
        });
      }

      let deltaNotional: DecimalValue | undefined;
      const trimmedDeltaNotional = form.baselineDeltaNotionalInput.trim();
      if (trimmedDeltaNotional.length === 0) {
        issues.push({
          field: 'baseline_delta.delta_notional',
          message: '必须填写基线面额差额 delta_notional',
        });
      } else {
        try {
          deltaNotional = createDecimalValue(
            trimmedDeltaNotional,
            currencyUnit,
            'baseline_delta.delta_notional',
          );
        } catch (err) {
          issues.push({
            field: 'baseline_delta.delta_notional',
            message: err instanceof Error ? err.message : '基线面额差额格式非法',
          });
        }
      }

      let deltaCash: DecimalValue | undefined;
      const trimmedDeltaCash = form.baselineDeltaCashInput.trim();
      if (trimmedDeltaCash.length === 0) {
        issues.push({
          field: 'baseline_delta.delta_cash',
          message: '必须填写基线现金差额 delta_cash',
        });
      } else {
        try {
          deltaCash = createDecimalValue(
            trimmedDeltaCash,
            currencyUnit,
            'baseline_delta.delta_cash',
          );
        } catch (err) {
          issues.push({
            field: 'baseline_delta.delta_cash',
            message: err instanceof Error ? err.message : '基线现金差额格式非法',
          });
        }
      }

      if (issues.length > 0 || !deltaNotional || !deltaCash) {
        return { ok: false, issues };
      }

      const baselineDelta = create(BaselineDeltaSchema, {
        deltaNotional,
        deltaCash,
        settlementBucket: targetBucket,
        // Preserve original BaseEvidence verbatim (never mutate or fabricate).
        evidence,
        reason: trimmedDeltaReason,
      });

      // Explicitly OMIT targetEarmarkId, expectedEarmarkRevision, offsetNotional!
      request = create(ResolvePendingMatchRequestSchema, {
        idempotencyKey: params.idempotencyKey ?? '',
        accountId: pm.accountId,
        pendingMatchId: pm.pendingMatchId,
        expectedPendingRevision: pm.pendingRevision,
        expectedPositionVersion: position.positionVersion,
        action: PendingMatchResolutionAction.APPLY_BASELINE_DELTA,
        reason: trimmedReason || trimmedDeltaReason,
        baselineDelta,
      });
      canonicalPayload = {
        accountId: pm.accountId,
        pendingMatchId: pm.pendingMatchId.value,
        expectedPendingRevision: pm.pendingRevision,
        expectedPositionVersion: position.positionVersion,
        action: PendingMatchResolutionAction.APPLY_BASELINE_DELTA,
        reason: trimmedReason || trimmedDeltaReason,
        baselineDelta: {
          deltaNotional: {
            coefficient: deltaNotional.coefficient,
            scale: deltaNotional.scale,
            unitId: deltaNotional.unit?.unitId?.value,
            unitVersion: deltaNotional.unit?.version,
          },
          deltaCash: {
            coefficient: deltaCash.coefficient,
            scale: deltaCash.scale,
            unitId: deltaCash.unit?.unitId?.value,
            unitVersion: deltaCash.unit?.version,
          },
          settlementBucket: targetBucket,
          evidenceRef: evidence.evidenceRef,
          evidenceSourceId: evidence.sourceId,
          evidenceExternalTradeId: evidence.externalTradeId,
          evidenceOriginalRevision: evidence.originalRevision,
          evidenceHash: evidence.evidenceHash?.value,
          reason: trimmedDeltaReason,
        },
      };
      break;
    }

    default: {
      issues.push({ field: 'action', message: '不支持的核销动作' });
      return { ok: false, issues };
    }
  }

  const structuralValidation = validateResolvePendingMatchRequest(request, units);
  if (!structuralValidation.ok) {
    return { ok: false, issues: structuralValidation.issues };
  }

  return {
    ok: true,
    request,
    canonicalPayload,
  };
}
