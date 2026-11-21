import {
  PreviewHedgeScope,
  type AccountBlotterSnapshot,
} from '../../generated/ficant/blotter/v1/model_pb';
import type { PreviewCoveredHedgeResult } from '../../generated/ficant/blotter/v1/service_pb';
import type { DecimalValue, MarketTime, UnitRef } from '../../generated/ficant/core/v1/common_pb';
import { BondTaxBasis } from '../../generated/ficant/rates/v1/analytics_pb';
import { formatDecimalValue, validateCanonicalDecimalValue } from '../../codec/decimal';
import {
  createMarketTimeFromDate,
  formatUnitRef,
  isNonEmptyTrimmed,
  unitsEqual,
} from '../../codec/primitives';

export const HEDGE_MISSING_TEXT = '未提供';

export function resolveSnapshotTimezone(snapshot: AccountBlotterSnapshot | undefined): string {
  const fromTz = snapshot?.openingBase?.effectiveFrom?.marketTimezone?.trim();
  if (fromTz && fromTz.length > 0) {
    return fromTz;
  }
  const wmTz = snapshot?.openingBase?.watermarkTime?.marketTimezone?.trim();
  if (wmTz && wmTz.length > 0) {
    return wmTz;
  }
  return 'UTC';
}

export function buildValuationMarketTime(
  snapshot: AccountBlotterSnapshot | undefined,
  now: Date,
): MarketTime {
  return createMarketTimeFromDate(now, resolveSnapshotTimezone(snapshot));
}

export function resolveEffectiveScope(
  requestedScope: PreviewHedgeScope,
  wholeAccountBlocked: boolean,
): PreviewHedgeScope {
  if (wholeAccountBlocked) {
    return PreviewHedgeScope.COVERED_SUBSET_ONLY;
  }
  if (
    requestedScope !== PreviewHedgeScope.WHOLE_ACCOUNT &&
    requestedScope !== PreviewHedgeScope.COVERED_SUBSET_ONLY
  ) {
    return PreviewHedgeScope.COVERED_SUBSET_ONLY;
  }
  return requestedScope;
}

export function resolveEffectiveProductCode(
  selectedProductCode: string,
  allowedProducts: readonly string[],
): string {
  if (allowedProducts.length === 0) {
    return '';
  }
  if (allowedProducts.includes(selectedProductCode)) {
    return selectedProductCode;
  }
  return allowedProducts[0] ?? '';
}

export function formatHedgeScopeLabel(scope: PreviewHedgeScope): string {
  switch (scope) {
    case PreviewHedgeScope.WHOLE_ACCOUNT:
      return '全账户覆盖 (WHOLE_ACCOUNT)';
    case PreviewHedgeScope.COVERED_SUBSET_ONLY:
      return '仅已覆盖子集 (COVERED_SUBSET_ONLY)';
    case PreviewHedgeScope.UNSPECIFIED:
      return 'UNSPECIFIED';
  }
}

export function formatHedgeDecimal(
  value: DecimalValue | undefined,
): { readonly valueText: string; readonly unitText: string | null } {
  if (!value) {
    return { valueText: HEDGE_MISSING_TEXT, unitText: null };
  }
  try {
    const valueText = formatDecimalValue(value, {
      missingText: HEDGE_MISSING_TEXT,
      grouping: true,
    });
    const unitText = value.unit ? formatUnitRef(value.unit) : null;
    return { valueText, unitText };
  } catch {
    return { valueText: '数值非法', unitText: null };
  }
}

export interface RenderableHedgeValidation {
  readonly valid: boolean;
  readonly reason?: string;
}

/**
 * Defense-in-depth render guard ensuring computed lots are NEVER displayed if a
 * `PreviewCoveredHedgeResult` violates any AC-005 contract invariant.
 */
export function verifyRenderableHedgeResult(
  result: PreviewCoveredHedgeResult,
  expected: {
    readonly scope: PreviewHedgeScope;
    readonly productCode: string;
    readonly contractCode: string;
    readonly expectedSnapshotSeq: bigint;
    readonly expectedCurrencyUnit: UnitRef | undefined;
  },
): RenderableHedgeValidation {
  if (result.scope !== expected.scope) {
    return { valid: false, reason: '响应 scope 与请求不一致' };
  }
  if (!isNonEmptyTrimmed(result.productCode) || result.productCode !== expected.productCode) {
    return { valid: false, reason: '响应 product_code 与请求不一致' };
  }
  if (!isNonEmptyTrimmed(result.contractCode) || result.contractCode !== expected.contractCode) {
    return { valid: false, reason: '响应 contract_code 与请求不一致' };
  }
  if (result.evaluatedAtSeq !== expected.expectedSnapshotSeq) {
    return { valid: false, reason: '响应 evaluated_at_seq 与请求快照序号不一致' };
  }
  if (result.coveredTaxBasis !== BondTaxBasis.PRE_TAX) {
    return { valid: false, reason: '响应 covered_tax_basis 非 PRE_TAX' };
  }
  if (result.uncoveredInstrumentCount > 0 && result.scope === PreviewHedgeScope.WHOLE_ACCOUNT) {
    return { valid: false, reason: '存在未覆盖持仓但返回 WHOLE_ACCOUNT' };
  }
  if (result.coveredInstrumentCount === 0) {
    return { valid: false, reason: '当前范围无已覆盖持仓（空覆盖簿），不可展示建议手数' };
  }
  if (!result.measures || !result.measures.contractNotional) {
    return { valid: false, reason: '响应缺少 measures 或 contract_notional' };
  }
  try {
    validateCanonicalDecimalValue(result.measures.contractNotional, {
      field: 'contract_notional',
      sign: 'positive',
      requireUnit: true,
      ...(expected.expectedCurrencyUnit !== undefined
        ? { expectedUnit: expected.expectedCurrencyUnit }
        : {}),
    });
  } catch (err) {
    return {
      valid: false,
      reason: err instanceof Error ? err.message : 'contract_notional 校验失败',
    };
  }
  if (
    result.excludedUncoveredGrossNotional?.unit !== undefined &&
    !unitsEqual(result.measures.contractNotional.unit, result.excludedUncoveredGrossNotional.unit)
  ) {
    return {
      valid: false,
      reason: 'contract_notional 单位与 excluded_uncovered_gross_notional 单位不一致',
    };
  }
  return { valid: true };
}
