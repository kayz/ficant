import { create } from '@bufbuild/protobuf';
import {
  CalendarResolutionMode,
  CoverageStatus,
  EarmarkState,
  FundingPrecision,
  QuoteTermsSchema,
  SettlementBucket,
  TradeSide,
  UncoveredReason,
  type QuoteTerms,
} from '../../generated/ficant/blotter/v1/model_pb';
import type { MarketTime } from '../../generated/ficant/core/v1/common_pb';
import { ErrorCode } from '../../generated/ficant/core/v1/error_pb';
import {
  BondTaxBasis,
  type AnalysisUnits,
} from '../../generated/ficant/rates/v1/analytics_pb';
import {
  createDecimalValue,
  DecimalValidationError,
  formatDecimalValue,
  parseDecimalString,
} from '../../codec/decimal';
import { marketTimeToEpochMs } from '../../codec/primitives';
import {
  MAX_HOLD_TTL_SECONDS,
  MIN_HOLD_TTL_SECONDS,
  validateHoldTtlSeconds,
  validateQuoteTerms,
  type ValidationIssue,
} from '../../codec/validators';
import type { QuoteInputMode } from './types';

const STRICT_INTEGER_PATTERN = /^[0-9]+$/;

export function formatCoverageStatus(status: CoverageStatus): string {
  switch (status) {
    case CoverageStatus.COVERED_PRE_TAX:
      return 'PRE_TAX 已覆盖';
    case CoverageStatus.UNCOVERED:
      return '未覆盖 (UNCOVERED)';
    case CoverageStatus.INACTIVE_ZERO_POSITION:
      return '零持仓未激活';
    case CoverageStatus.UNSPECIFIED:
    default:
      return '未指定';
  }
}

export function formatTaxBasis(taxBasis: BondTaxBasis): string {
  switch (taxBasis) {
    case BondTaxBasis.PRE_TAX:
      return 'PRE_TAX';
    case BondTaxBasis.TAX_ADJUSTED:
      return 'TAX_ADJUSTED';
    case BondTaxBasis.UNSPECIFIED:
    default:
      return 'UNSPECIFIED';
  }
}

export function formatUncoveredReason(reason: UncoveredReason): string {
  switch (reason) {
    case UncoveredReason.INSTRUMENT_SPEC_MISSING:
      return 'INSTRUMENT_SPEC_MISSING（缺债券要素）';
    case UncoveredReason.UNSUPPORTED_COUPON_STRUCTURE:
      return 'UNSUPPORTED_COUPON_STRUCTURE（不支持的票息结构）';
    case UncoveredReason.EMBEDDED_OPTION_OR_CALLABLE:
      return 'EMBEDDED_OPTION_OR_CALLABLE（含权/可赎回）';
    case UncoveredReason.UNSUPPORTED_CONVENTION:
      return 'UNSUPPORTED_CONVENTION（不支持的计息惯例）';
    case UncoveredReason.MISSING_MARKET_QUOTE:
      return 'MISSING_MARKET_QUOTE（缺市场报价）';
    case UncoveredReason.CALENDAR_COVERAGE_MISSING:
      return 'CALENDAR_COVERAGE_MISSING（缺交易日历覆盖）';
    case UncoveredReason.KERNEL_PRICING_FAILED:
      return 'KERNEL_PRICING_FAILED（估值内核失败）';
    case UncoveredReason.ELIGIBILITY_UNVERIFIED:
      return 'ELIGIBILITY_UNVERIFIED（覆盖资格未核验）';
    case UncoveredReason.CREDIT_OR_NON_RATES_ISSUER:
      return 'CREDIT_OR_NON_RATES_ISSUER（非利率债/信用债）';
    case UncoveredReason.STALE_QUOTE:
      return 'STALE_QUOTE（报价陈旧）';
    case UncoveredReason.UNSPECIFIED:
    default:
      return 'UNSPECIFIED（未提供原因）';
  }
}

export function formatCalendarResolution(mode: CalendarResolutionMode): string {
  switch (mode) {
    case CalendarResolutionMode.EXACT:
      return '精确日历';
    case CalendarResolutionMode.PROVISIONAL_WEEKEND_BEYOND_COVERAGE:
      return '超日历暂估';
    case CalendarResolutionMode.UNSPECIFIED:
    default:
      return '—';
  }
}

export function formatEarmarkState(state: EarmarkState): string {
  switch (state) {
    case EarmarkState.HOLD:
      return 'HOLD';
    case EarmarkState.COMMITTED:
      return 'COMMITTED';
    case EarmarkState.EXPIRED:
      return 'EXPIRED';
    case EarmarkState.RELEASED:
      return 'RELEASED';
    case EarmarkState.CANCELLED:
      return 'CANCELLED';
    case EarmarkState.RECONCILED:
      return 'RECONCILED';
    case EarmarkState.UNSPECIFIED:
    default:
      return 'UNSPECIFIED';
  }
}

export function formatTradeSide(side: TradeSide): string {
  switch (side) {
    case TradeSide.BUY:
      return '买入 (BUY)';
    case TradeSide.SELL:
      return '卖出 (SELL)';
    case TradeSide.UNSPECIFIED:
    default:
      return '—';
  }
}

export function formatSettlementBucket(bucket: SettlementBucket): string {
  switch (bucket) {
    case SettlementBucket.T_PLUS_0:
      return 'T+0';
    case SettlementBucket.T_PLUS_1:
      return 'T+1';
    case SettlementBucket.UNSPECIFIED:
    default:
      return '—';
  }
}

export function formatFundingPrecision(precision: FundingPrecision): string {
  switch (precision) {
    case FundingPrecision.EXACT_SETTLEMENT_AMOUNT:
      return '精确结算金额';
    case FundingPrecision.EXACT_DIRTY_PRICE:
      return '精确全价';
    case FundingPrecision.CLEAN_PRICE_ESTIMATE:
      return '净价估算';
    case FundingPrecision.UNPRICED_YIELD_ONLY:
      return '仅收益率未定价';
    case FundingPrecision.UNSPECIFIED:
    default:
      return '—';
  }
}

export function formatErrorCode(code: ErrorCode): string {
  switch (code) {
    case ErrorCode.VALIDATION_FAILED:
      return 'VALIDATION_FAILED';
    case ErrorCode.NOT_FOUND:
      return 'NOT_FOUND';
    case ErrorCode.ALREADY_EXISTS:
      return 'ALREADY_EXISTS';
    case ErrorCode.VERSION_CONFLICT:
      return 'VERSION_CONFLICT';
    case ErrorCode.CONCURRENCY_CONFLICT:
      return 'CONCURRENCY_CONFLICT';
    case ErrorCode.IMMUTABLE_VIOLATION:
      return 'IMMUTABLE_VIOLATION';
    case ErrorCode.LINEAGE_INCOMPLETE:
      return 'LINEAGE_INCOMPLETE';
    case ErrorCode.HASH_MISMATCH:
      return 'HASH_MISMATCH';
    case ErrorCode.UNAUTHENTICATED:
      return 'UNAUTHENTICATED';
    case ErrorCode.FORBIDDEN:
      return 'FORBIDDEN';
    case ErrorCode.STORAGE_UNAVAILABLE:
      return 'STORAGE_UNAVAILABLE';
    case ErrorCode.INTERNAL:
      return 'INTERNAL';
    case ErrorCode.UNSPECIFIED:
    default:
      return 'UNSPECIFIED';
  }
}

export function formatMarketTimeDisplay(mt: MarketTime | undefined): string {
  if (!mt || !mt.instant) {
    return '未提供';
  }
  try {
    const ms = marketTimeToEpochMs(mt, 'market_time');
    const iso = new Date(ms).toISOString().replace('.000Z', 'Z');
    return `${mt.localTradingDate} ${iso}`;
  } catch {
    return mt.localTradingDate || '未提供';
  }
}

export interface HoldTtlDisplay {
  readonly label: string;
  readonly expired: boolean;
  readonly remainingSeconds: number | null;
}

export function formatHoldRemainingTtl(
  expiresAt: MarketTime | undefined,
  nowMs: number,
): HoldTtlDisplay {
  if (!expiresAt || !expiresAt.instant) {
    return { label: '无到期时间', expired: false, remainingSeconds: null };
  }
  try {
    const expMs = marketTimeToEpochMs(expiresAt, 'expires_at');
    const diffSec = Math.floor((expMs - nowMs) / 1000);
    if (diffSec <= 0) {
      return {
        label: `已到期 (${Math.abs(diffSec)}s 前)`,
        expired: true,
        remainingSeconds: diffSec,
      };
    }
    const mins = Math.floor(diffSec / 60);
    const secs = diffSec % 60;
    return {
      label: `剩余 ${mins}m${secs.toString().padStart(2, '0')}s (${diffSec}s)`,
      expired: false,
      remainingSeconds: diffSec,
    };
  } catch {
    return { label: '到期时间无效', expired: false, remainingSeconds: null };
  }
}

export type ParseTtlResult =
  | { readonly ok: true; readonly ttlSeconds: number }
  | { readonly ok: false; readonly issue: ValidationIssue };

/**
 * Strictly parses a TTL string input and validates integer range `[1800, 2700]` inclusive.
 */
export function parseStrictTtlSeconds(raw: string, field = 'ttl_seconds'): ParseTtlResult {
  const trimmed = raw.trim();
  if (trimmed.length === 0 || !STRICT_INTEGER_PATTERN.test(trimmed)) {
    return {
      ok: false,
      issue: {
        field,
        message: `TTL 必须为 ${MIN_HOLD_TTL_SECONDS}..=${MAX_HOLD_TTL_SECONDS} 之间的整数，当前为 "${raw}"`,
      },
    };
  }
  const parsed = Number(trimmed);
  const issue = validateHoldTtlSeconds(parsed, field);
  if (issue !== null) {
    return { ok: false, issue };
  }
  return { ok: true, ttlSeconds: parsed };
}

export interface QuoteInputDraft {
  readonly mode: QuoteInputMode;
  readonly cleanPriceText: string;
  readonly yieldText: string;
  readonly settlementAmountText: string;
}

export type BuildQuoteTermsResult =
  | { readonly ok: true; readonly quoteTerms: QuoteTerms | undefined }
  | { readonly ok: false; readonly issues: readonly ValidationIssue[] };

/**
 * Builds and validates a protobuf `QuoteTerms` from user-entered decimal strings.
 * - `mode === 'omit'` is valid ONLY when `required === false` (e.g. `CreateHold`).
 * - `mode === 'settlement_only'` allows strictly positive `settlement_amount` alone.
 * - `mode === 'clean_price'` requires strictly positive `clean_price_per_100` (+ optional positive `settlement_amount`).
 * - `mode === 'yield'` requires non-negative `yield_to_maturity` (+ optional positive `settlement_amount`).
 * - Rejects empty quote, zero/negative settlement_amount or clean_price, negative yield, NaN, malformed text client-side.
 */
export function buildQuoteTermsFromInputs(
  draft: QuoteInputDraft,
  options: {
    readonly required: boolean;
    readonly units: AnalysisUnits | undefined;
  },
): BuildQuoteTermsResult {
  if (draft.mode === 'omit') {
    if (options.required) {
      return {
        ok: false,
        issues: [{ field: 'quote_terms', message: '该操作必须提供有效报价（净价、收益率或正结算金额）' }],
      };
    }
    return { ok: true, quoteTerms: undefined };
  }

  const units = options.units;
  const issues: ValidationIssue[] = [];

  let cleanPriceVal = undefined;
  let yieldVal = undefined;
  let settlementVal = undefined;

  if (draft.mode === 'clean_price') {
    const raw = draft.cleanPriceText.trim();
    if (raw.length === 0) {
      issues.push({
        field: 'quote_terms.clean_price_per_100',
        message: '请输入大于 0 的百元净价',
      });
    } else if (!units?.pricePer100) {
      issues.push({
        field: 'opening_base.units.price_per_100',
        message: '账户缺少 price_per_100 单位定义',
      });
    } else {
      try {
        const parts = parseDecimalString(raw, 'quote_terms.clean_price_per_100');
        cleanPriceVal = createDecimalValue(parts, units.pricePer100, 'quote_terms.clean_price_per_100');
      } catch (err) {
        issues.push({
          field: 'quote_terms.clean_price_per_100',
          message: err instanceof DecimalValidationError ? err.message : '净价格式非法',
        });
      }
    }
  } else if (draft.mode === 'yield') {
    const raw = draft.yieldText.trim();
    if (raw.length === 0) {
      issues.push({
        field: 'quote_terms.yield_to_maturity',
        message: '请输入非负到期收益率',
      });
    } else if (!units?.rate) {
      issues.push({
        field: 'opening_base.units.rate',
        message: '账户缺少 rate 单位定义',
      });
    } else {
      try {
        const parts = parseDecimalString(raw, 'quote_terms.yield_to_maturity');
        yieldVal = createDecimalValue(parts, units.rate, 'quote_terms.yield_to_maturity');
      } catch (err) {
        issues.push({
          field: 'quote_terms.yield_to_maturity',
          message: err instanceof DecimalValidationError ? err.message : '收益率格式非法',
        });
      }
    }
  }

  const rawSettlement = draft.settlementAmountText.trim();
  if (draft.mode === 'settlement_only' && rawSettlement.length === 0) {
    issues.push({
      field: 'quote_terms.settlement_amount',
      message: '仅结算金额模式下必须输入严格大于 0 的结算金额（不得为空报价）',
    });
  } else if (rawSettlement.length > 0) {
    if (!units?.currencyAmount) {
      issues.push({
        field: 'opening_base.units.currency_amount',
        message: '账户缺少 currency_amount 单位定义',
      });
    } else {
      try {
        const parts = parseDecimalString(rawSettlement, 'quote_terms.settlement_amount');
        settlementVal = createDecimalValue(
          parts,
          units.currencyAmount,
          'quote_terms.settlement_amount',
        );
      } catch (err) {
        issues.push({
          field: 'quote_terms.settlement_amount',
          message: err instanceof DecimalValidationError ? err.message : '结算金额格式非法',
        });
      }
    }
  }

  if (issues.length > 0) {
    return { ok: false, issues };
  }

  const priceOrYield =
    cleanPriceVal !== undefined
      ? ({ case: 'cleanPricePer100', value: cleanPriceVal } as const)
      : yieldVal !== undefined
        ? ({ case: 'yieldToMaturity', value: yieldVal } as const)
        : ({ case: undefined, value: undefined } as const);

  const quoteTerms = create(QuoteTermsSchema, {
    priceOrYield,
    ...(settlementVal !== undefined ? { settlementAmount: settlementVal } : {}),
  });

  const validated = validateQuoteTerms(quoteTerms, {
    required: options.required,
    units,
    fieldPrefix: 'quote_terms',
  });
  if (!validated.ok) {
    return { ok: false, issues: validated.issues };
  }

  return { ok: true, quoteTerms };
}

export function quoteTermsToDraft(quote: QuoteTerms | undefined, allowOmit: boolean): QuoteInputDraft {
  if (!quote) {
    return {
      mode: allowOmit ? 'omit' : 'settlement_only',
      cleanPriceText: '',
      yieldText: '',
      settlementAmountText: '',
    };
  }
  const settlementAmountText =
    quote.settlementAmount !== undefined
      ? formatDecimalValue(quote.settlementAmount, { grouping: false })
      : '';

  if (quote.priceOrYield.case === 'cleanPricePer100') {
    return {
      mode: 'clean_price',
      cleanPriceText: formatDecimalValue(quote.priceOrYield.value, { grouping: false }),
      yieldText: '',
      settlementAmountText,
    };
  }
  if (quote.priceOrYield.case === 'yieldToMaturity') {
    return {
      mode: 'yield',
      cleanPriceText: '',
      yieldText: formatDecimalValue(quote.priceOrYield.value, { grouping: false }),
      settlementAmountText,
    };
  }
  return {
    mode: 'settlement_only',
    cleanPriceText: '',
    yieldText: '',
    settlementAmountText,
  };
}
