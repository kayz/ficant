import type { TrustedRulePackBuildAnchor } from 'virtual:ficant-trusted-rulepack';
import {
  FeedHealthState,
  type FundingGapSummary,
  type RiskAndCoverageSummary,
  type SeqRange,
} from '../../generated/ficant/blotter/v1/model_pb';
import type { DecimalValue } from '../../generated/ficant/core/v1/common_pb';
import { BondTaxBasis } from '../../generated/ficant/rates/v1/analytics_pb';
import {
  formatDecimalValue,
  validateCanonicalDecimalValue,
} from '../../codec/decimal';
import { formatUnitRef } from '../../codec/primitives';
import {
  PINNED_RULEPACK_MANIFEST_SHA256,
  PINNED_RULEPACK_PAYLOAD_BYTES,
  PINNED_RULEPACK_PAYLOAD_SHA256,
  PINNED_RULEPACK_TYPE_URL,
} from '../../codec/rulepack';
import {
  selectFundingCompletenessFactors,
  type FundingCompletenessFactors,
} from '../../store/selectors';

export const MISSING_MEASURE_TEXT = '未提供';

/**
 * Strict decimal formatter that NEVER converts missing/absent `DecimalValue` to `"0"`.
 * Missing values render as `"未提供"`, genuine numeric zero renders as `"0"`.
 */
export function formatDecimalStrict(
  value: DecimalValue | undefined,
  options?: { readonly grouping?: boolean },
): string {
  if (value === undefined) {
    return MISSING_MEASURE_TEXT;
  }
  try {
    return formatDecimalValue(value, {
      missingText: MISSING_MEASURE_TEXT,
      grouping: options?.grouping ?? true,
    });
  } catch {
    return '数值非法';
  }
}

export function formatDecimalWithUnit(
  value: DecimalValue | undefined,
  options?: { readonly grouping?: boolean },
): { readonly text: string; readonly unitText: string | null } {
  if (value === undefined) {
    return { text: MISSING_MEASURE_TEXT, unitText: null };
  }
  const text = formatDecimalStrict(value, options);
  const unitText = value.unit ? formatUnitRef(value.unit) : null;
  return { text, unitText };
}

export function safeDecimalSign(value: DecimalValue | undefined): -1 | 0 | 1 | null {
  if (value === undefined) {
    return null;
  }
  try {
    const parts = validateCanonicalDecimalValue(value, { requireUnit: false });
    if (parts.scaledI128 > 0n) {
      return 1;
    }
    if (parts.scaledI128 < 0n) {
      return -1;
    }
    return 0;
  } catch {
    return null;
  }
}

export interface UncoveredRiskAssessment {
  readonly shouldRenderBreakdown: boolean;
  readonly hasUncoveredWarning: boolean;
  readonly wholeAccountBlocked: boolean;
  readonly isNetZeroWithGrossNonZero: boolean;
  readonly isPureShortNetNegative: boolean;
  readonly count: number;
  readonly warningMessages: readonly string[];
}

export function assessUncoveredRisk(
  summary: RiskAndCoverageSummary | undefined,
): UncoveredRiskAssessment {
  if (!summary) {
    return {
      shouldRenderBreakdown: false,
      hasUncoveredWarning: false,
      wholeAccountBlocked: true,
      isNetZeroWithGrossNonZero: false,
      isPureShortNetNegative: false,
      count: 0,
      warningMessages: [],
    };
  }

  const grossSign = safeDecimalSign(summary.uncoveredGrossNotional);
  const netSign = safeDecimalSign(summary.uncoveredNetNotional);
  const hasPositiveGross = grossSign === 1;
  const count = summary.uncoveredInstrumentCount;
  const hasUncoveredWarning = count > 0 || hasPositiveGross;
  const isNetZeroWithGrossNonZero = hasPositiveGross && netSign === 0;
  const isPureShortNetNegative = netSign === -1;

  const warningMessages: string[] = [];
  if (count > 0) {
    warningMessages.push(
      `存在 ${count} 只未覆盖持仓（uncovered_instrument_count > 0），已禁用 WHOLE_ACCOUNT 全账户套保。`,
    );
  } else if (hasPositiveGross) {
    warningMessages.push('未覆盖总面额非零，存在未覆盖风险敞口。');
  }

  if (isNetZeroWithGrossNonZero) {
    warningMessages.push(
      '多空未覆盖持仓相抵后净额为 0 但总面额（gross）非零，仍按未覆盖风险告警并禁用全账户套保。',
    );
  }
  if (isPureShortNetNegative) {
    warningMessages.push(
      '未覆盖带符号净面额为负（纯空头/净空头未覆盖敞口），同样触发告警并禁用全账户套保。',
    );
  }

  return {
    shouldRenderBreakdown: hasUncoveredWarning,
    hasUncoveredWarning,
    wholeAccountBlocked: count > 0 || !summary.wholeAccountHedgeAllowed,
    isNetZeroWithGrossNonZero,
    isPureShortNetNegative,
    count,
    warningMessages,
  };
}

export interface FundingCompletenessFactorView {
  readonly key:
    | 'cleanPriceEstimatedFundingCount'
    | 'unpricedFundingEarmarkCount'
    | 'pendingMatchCount'
    | 'pendingUnknownCashCount'
    | 'feedIncomplete';
  readonly label: string;
  readonly triggered: boolean;
  readonly valueText: string;
  readonly explanation: string;
}

export function describeFundingCompletenessFactors(
  funding: FundingGapSummary | undefined,
): {
  readonly factors: FundingCompletenessFactors | null;
  readonly items: readonly FundingCompletenessFactorView[];
  readonly pendingZeroNetStillIncomplete: boolean;
} {
  const factors = selectFundingCompletenessFactors(funding);
  if (!factors || !funding) {
    return {
      factors: null,
      items: [],
      pendingZeroNetStillIncomplete: false,
    };
  }

  const pendingCashT0Sign = safeDecimalSign(funding.pendingMatchCashT0);
  const pendingCashT1Sign = safeDecimalSign(funding.pendingMatchCashT1);
  const pendingZeroNetStillIncomplete =
    factors.pendingMatchCount > 0 &&
    (pendingCashT0Sign === 0 || pendingCashT0Sign === null) &&
    (pendingCashT1Sign === 0 || pendingCashT1Sign === null);

  const items: FundingCompletenessFactorView[] = [
    {
      key: 'cleanPriceEstimatedFundingCount',
      label: '净价估算资金',
      triggered: factors.cleanPriceEstimatedFundingCount > 0,
      valueText: `${factors.cleanPriceEstimatedFundingCount} 笔`,
      explanation:
        factors.cleanPriceEstimatedFundingCount > 0
          ? '存在按净价估算的资金占仓，尚未获得精确结算金额'
          : '无净价估算项',
    },
    {
      key: 'unpricedFundingEarmarkCount',
      label: '未定价占仓',
      triggered: factors.unpricedFundingEarmarkCount > 0,
      valueText: `${factors.unpricedFundingEarmarkCount} 笔`,
      explanation:
        factors.unpricedFundingEarmarkCount > 0
          ? '存在缺少价格或仅含收益率的未定价占仓，无法精确计算现金流'
          : '无未定价占仓',
    },
    {
      key: 'pendingMatchCount',
      label: '待核销成交',
      triggered: factors.pendingMatchCount > 0,
      valueText: `${factors.pendingMatchCount} 笔`,
      explanation:
        factors.pendingMatchCount > 0
          ? '存在未解决待核销成交（即使正负现金相抵为 0 仍视为不完整）'
          : '无待核销成交',
    },
    {
      key: 'pendingUnknownCashCount',
      label: '待核销未知现金',
      triggered: factors.pendingUnknownCashCount > 0,
      valueText: `${factors.pendingUnknownCashCount} 笔`,
      explanation:
        factors.pendingUnknownCashCount > 0
          ? '存在缺少精确结算现金的待核销记录'
          : '无未知现金待核销',
    },
    {
      key: 'feedIncomplete',
      label: '成交源完整性',
      triggered: factors.feedIncomplete,
      valueText: factors.feedIncomplete ? '不完整' : '完整',
      explanation: factors.feedIncomplete
        ? '至少一个成交源处于缺口、陈旧或暂停状态'
        : '所有关联成交源健康且连续',
    },
  ];

  return {
    factors,
    items,
    pendingZeroNetStillIncomplete,
  };
}

export function formatTaxBasisLabel(basis: BondTaxBasis): string {
  switch (basis) {
    case BondTaxBasis.PRE_TAX:
      return 'PRE_TAX (税前)';
    case BondTaxBasis.TAX_ADJUSTED:
      return 'TAX_ADJUSTED (税后)';
    case BondTaxBasis.UNSPECIFIED:
      return 'UNSPECIFIED';
  }
}

export function formatTopBarFeedState(state: FeedHealthState): string {
  switch (state) {
    case FeedHealthState.HEALTHY:
      return 'HEALTHY';
    case FeedHealthState.STALE:
      return 'STALE';
    case FeedHealthState.GAP_DETECTED:
      return 'GAP_DETECTED';
    case FeedHealthState.PAUSED_BY_OPERATOR:
      return 'PAUSED_BY_OPERATOR';
    case FeedHealthState.UNSPECIFIED:
      return 'UNSPECIFIED';
  }
}

export function formatSeqRanges(ranges: readonly SeqRange[]): string {
  if (ranges.length === 0) {
    return '无';
  }
  return ranges.map((r) => `[${r.first.toString()}..${r.last.toString()}]`).join(', ');
}

export type RulePackTrustStatus = 'verified' | 'expired' | 'invalid_anchor' | 'missing_anchor';

export interface RulePackTrustSummary {
  readonly status: RulePackTrustStatus;
  readonly label: string;
  readonly rulePackId: string;
  readonly version: string;
  readonly effectiveWindow: string;
  readonly detail: string;
}

export function evaluateRulePackTrustAnchor(
  anchor: TrustedRulePackBuildAnchor | null | undefined,
  valuationAt: Date = new Date(),
): RulePackTrustSummary {
  if (!anchor || !anchor.manifest) {
    return {
      status: 'missing_anchor',
      label: '规则包缺信任锚 (MISSING_TRUST_ANCHOR)',
      rulePackId: MISSING_MEASURE_TEXT,
      version: MISSING_MEASURE_TEXT,
      effectiveWindow: MISSING_MEASURE_TEXT,
      detail: '未加载 G-1 独立固定交割规则包信任锚',
    };
  }

  const m = anchor.manifest;
  const windowText = `[${m.effective_from.local_trading_date}, ${m.effective_to.local_trading_date})`;

  const anchorValid =
    anchor.manifestSha256 === PINNED_RULEPACK_MANIFEST_SHA256 &&
    anchor.payloadSha256 === PINNED_RULEPACK_PAYLOAD_SHA256 &&
    m.payload_sha256 === PINNED_RULEPACK_PAYLOAD_SHA256 &&
    m.payload_byte_length === PINNED_RULEPACK_PAYLOAD_BYTES &&
    anchor.payloadByteLength === PINNED_RULEPACK_PAYLOAD_BYTES &&
    m.type_url === PINNED_RULEPACK_TYPE_URL &&
    m.verification_status === 'VERIFIED';

  if (!anchorValid) {
    return {
      status: 'invalid_anchor',
      label: '规则包信任锚校验失败 (UNTRUSTED)',
      rulePackId: m.rule_pack_id,
      version: `v${String(m.version)}`,
      effectiveWindow: windowText,
      detail: '清单摘要、载荷摘要、类型或验证状态与独立固定锚不一致',
    };
  }

  const fromMs = new Date(m.effective_from.instant).getTime();
  const toMs = new Date(m.effective_to.instant).getTime();
  const nowMs = valuationAt.getTime();
  if (
    Number.isNaN(fromMs) ||
    Number.isNaN(toMs) ||
    Number.isNaN(nowMs) ||
    nowMs < fromMs ||
    nowMs >= toMs
  ) {
    return {
      status: 'expired',
      label: '规则包不在有效期内 (EXPIRED)',
      rulePackId: m.rule_pack_id,
      version: `v${String(m.version)}`,
      effectiveWindow: windowText,
      detail: `当前估值时点不在半开有效期 [${m.effective_from.instant}, ${m.effective_to.instant}) 内`,
    };
  }

  return {
    status: 'verified',
    label: '规则包可信已固定 (VERIFIED)',
    rulePackId: m.rule_pack_id,
    version: `v${String(m.version)}`,
    effectiveWindow: windowText,
    detail: `approval=${m.approval_ref}, sha256=${anchor.payloadSha256.slice(0, 12)}...`,
  };
}
