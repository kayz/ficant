import { create } from '@bufbuild/protobuf';
import {
  CoverageStatus,
  DraftEarmarkStatus,
  DraftTargetState,
  QuoteTermsSchema,
  SettlementBucket,
  TradeSide,
  UncoveredReason,
  type DraftEarmarkRecord,
  type QuoteTerms,
  type RuleLineage,
} from '../../generated/ficant/blotter/v1/model_pb';
import {
  NlpEarmarkProposalSchema,
  type NlpEarmarkProposal,
} from '../../generated/ficant/blotter/v1/spi_pb';
import { ErrorCode } from '../../generated/ficant/core/v1/error_pb';
import type { AnalysisUnits } from '../../generated/ficant/rates/v1/analytics_pb';
import {
  createDecimalValue,
  formatDecimalValue,
  formatUnitRef,
  isNonEmptyTrimmed,
  isValidUlid,
  isZeroDecimal,
  sha256ToHex,
  unitsEqual,
  validateDraftReadyForConfirmation,
  validateNlpEarmarkProposal,
  validateSha256,
  validateUnitRef,
  type ValidationIssue,
  type ValidationResult,
} from '../../codec';
import type { BlotterClientError } from '../../transport/error';

export type QuoteInputMode = 'none' | 'clean_price' | 'yield';

export interface DraftQuoteFormValues {
  readonly quoteMode: QuoteInputMode;
  readonly cleanPricePer100: string;
  readonly yieldToMaturity: string;
  readonly settlementAmount: string;
}

export type DraftLifecycleStage =
  | 'idle'
  | 'parsed'
  | 'draft_pending'
  | 'conflict_invalidated'
  | 'confirmed';

export interface RegulatoryBlockInfo {
  readonly blocked: boolean;
  readonly kind: 'missing_rule_pack' | 'missing_limit' | 'limit_exceeded' | 'lineage_invalid' | null;
  readonly summary: string;
  readonly details: readonly string[];
}

export type DraftPreviewClassification =
  | {
      readonly kind: 'covered_nonzero';
      readonly dv01Text: string;
      readonly dv01UnitText: string;
      readonly previewAtSeqText: string;
      readonly ready: true;
      readonly requiresUncoveredAck: false;
    }
  | {
      readonly kind: 'covered_zero_hold';
      readonly dv01Text: string;
      readonly dv01UnitText: string;
      readonly previewAtSeqText: string;
      readonly ready: true;
      readonly requiresUncoveredAck: false;
    }
  | {
      readonly kind: 'covered_missing_delta';
      readonly dv01Text: '未提供';
      readonly dv01UnitText: string;
      readonly previewAtSeqText: string;
      readonly ready: false;
      readonly requiresUncoveredAck: false;
      readonly missingReason: string;
    }
  | {
      readonly kind: 'uncovered_valid';
      readonly uncoveredReasonLabel: string;
      readonly uncoveredReasonCode: string;
      readonly deltaNotionalText: string;
      readonly deltaNotionalIsZero: boolean;
      readonly deltaNotionalUnitText: string;
      readonly previewAtSeqText: string;
      readonly ready: true;
      readonly requiresUncoveredAck: true;
    }
  | {
      readonly kind: 'uncovered_incomplete';
      readonly uncoveredReasonLabel: string;
      readonly uncoveredReasonCode: string;
      readonly deltaNotionalText: string;
      readonly deltaNotionalUnitText: string;
      readonly previewAtSeqText: string;
      readonly ready: false;
      readonly requiresUncoveredAck: false;
      readonly missingReason: string;
    }
  | {
      readonly kind: 'invalid_preview';
      readonly previewAtSeqText: string;
      readonly ready: false;
      readonly requiresUncoveredAck: false;
      readonly missingReason: string;
    };

export function formatTradeSide(side: TradeSide): string {
  switch (side) {
    case TradeSide.BUY:
      return '买入 (BUY)';
    case TradeSide.SELL:
      return '卖出 (SELL)';
    case TradeSide.UNSPECIFIED:
      return '未指定';
  }
}

export function formatSettlementBucket(bucket: SettlementBucket): string {
  switch (bucket) {
    case SettlementBucket.T_PLUS_0:
      return 'T+0';
    case SettlementBucket.T_PLUS_1:
      return 'T+1';
    case SettlementBucket.UNSPECIFIED:
      return '未指定';
  }
}

export function formatTargetState(target: DraftTargetState): string {
  switch (target) {
    case DraftTargetState.HOLD:
      return 'HOLD (占仓锁券)';
    case DraftTargetState.COMMITTED:
      return 'COMMITTED (已确认交易)';
    case DraftTargetState.UNSPECIFIED:
      return '未指定';
  }
}

export function formatDraftStatus(status: DraftEarmarkStatus): string {
  switch (status) {
    case DraftEarmarkStatus.PENDING_CONFIRMATION:
      return '待确认 (PENDING_CONFIRMATION)';
    case DraftEarmarkStatus.CONFIRMED:
      return '已确认 (CONFIRMED)';
    case DraftEarmarkStatus.DISCARDED:
      return '已作废 (DISCARDED)';
    case DraftEarmarkStatus.UNSPECIFIED:
      return '未指定';
  }
}

export function formatUncoveredReason(reason: UncoveredReason): {
  readonly code: string;
  readonly label: string;
} {
  switch (reason) {
    case UncoveredReason.INSTRUMENT_SPEC_MISSING:
      return { code: 'INSTRUMENT_SPEC_MISSING', label: '券种条款缺失' };
    case UncoveredReason.UNSUPPORTED_COUPON_STRUCTURE:
      return { code: 'UNSUPPORTED_COUPON_STRUCTURE', label: '不支持的付息结构' };
    case UncoveredReason.EMBEDDED_OPTION_OR_CALLABLE:
      return { code: 'EMBEDDED_OPTION_OR_CALLABLE', label: '含权/可赎回条款' };
    case UncoveredReason.UNSUPPORTED_CONVENTION:
      return { code: 'UNSUPPORTED_CONVENTION', label: '不支持的计息惯例' };
    case UncoveredReason.MISSING_MARKET_QUOTE:
      return { code: 'MISSING_MARKET_QUOTE', label: '缺失有效市场报价' };
    case UncoveredReason.CALENDAR_COVERAGE_MISSING:
      return { code: 'CALENDAR_COVERAGE_MISSING', label: '缺失精确日历覆盖' };
    case UncoveredReason.KERNEL_PRICING_FAILED:
      return { code: 'KERNEL_PRICING_FAILED', label: '内核定价失败' };
    case UncoveredReason.ELIGIBILITY_UNVERIFIED:
      return { code: 'ELIGIBILITY_UNVERIFIED', label: '利率覆盖资格未核验' };
    case UncoveredReason.CREDIT_OR_NON_RATES_ISSUER:
      return { code: 'CREDIT_OR_NON_RATES_ISSUER', label: '信用或非利率发行主体' };
    case UncoveredReason.STALE_QUOTE:
      return { code: 'STALE_QUOTE', label: '市场报价已陈旧' };
    case UncoveredReason.UNSPECIFIED:
      return { code: 'UNSPECIFIED', label: '未提供' };
  }
}

/**
 * Extracts initial editable quote form values from a parsed `NlpEarmarkProposal`.
 */
export function extractQuoteFormValues(quote: QuoteTerms | undefined): DraftQuoteFormValues {
  if (!quote) {
    return {
      quoteMode: 'none',
      cleanPricePer100: '',
      yieldToMaturity: '',
      settlementAmount: '',
    };
  }

  let quoteMode: QuoteInputMode = 'none';
  let cleanPricePer100 = '';
  let yieldToMaturity = '';

  if (quote.priceOrYield.case === 'cleanPricePer100') {
    quoteMode = 'clean_price';
    try {
      cleanPricePer100 = formatDecimalValue(quote.priceOrYield.value, { grouping: false });
    } catch {
      cleanPricePer100 = quote.priceOrYield.value.coefficient;
    }
  } else if (quote.priceOrYield.case === 'yieldToMaturity') {
    quoteMode = 'yield';
    try {
      yieldToMaturity = formatDecimalValue(quote.priceOrYield.value, { grouping: false });
    } catch {
      yieldToMaturity = quote.priceOrYield.value.coefficient;
    }
  }

  let settlementAmount = '';
  if (quote.settlementAmount !== undefined) {
    try {
      settlementAmount = formatDecimalValue(quote.settlementAmount, { grouping: false });
    } catch {
      settlementAmount = quote.settlementAmount.coefficient;
    }
  }

  return {
    quoteMode,
    cleanPricePer100,
    yieldToMaturity,
    settlementAmount,
  };
}

/**
 * Builds a protobuf `QuoteTerms` message from explicit user-edited form values.
 * Accepts:
 * - Positive `clean_price_per_100` alone, OR
 * - Non-negative `yield_to_maturity` alone, OR
 * - Positive `settlement_amount` alone (no price/yield required), OR
 * - One of price/yield combined with positive `settlement_amount`.
 *
 * Rejects empty quote terms, malformed decimals, zero/negative settlement amount,
 * non-positive clean price, or negative yield.
 */
export function buildQuoteTermsFromForm(
  form: DraftQuoteFormValues,
  units: AnalysisUnits | undefined,
):
  | { readonly ok: true; readonly quoteTerms: QuoteTerms }
  | { readonly ok: false; readonly issues: readonly ValidationIssue[] } {
  const issues: ValidationIssue[] = [];
  const trimmedClean = form.cleanPricePer100.trim();
  const trimmedYield = form.yieldToMaturity.trim();
  const trimmedSettlement = form.settlementAmount.trim();

  const hasClean = form.quoteMode === 'clean_price' && trimmedClean.length > 0;
  const hasYield = form.quoteMode === 'yield' && trimmedYield.length > 0;
  const hasSettlement = trimmedSettlement.length > 0;

  if (form.quoteMode === 'clean_price' && !hasClean && !hasSettlement) {
    issues.push({
      field: 'quote_terms.clean_price_per_100',
      message: '请填写正净价，或填写正结算金额',
    });
  } else if (form.quoteMode === 'yield' && !hasYield && !hasSettlement) {
    issues.push({
      field: 'quote_terms.yield_to_maturity',
      message: '请填写非负到期收益率，或填写正结算金额',
    });
  } else if (!hasClean && !hasYield && !hasSettlement) {
    issues.push({
      field: 'quote_terms',
      message: '报价不能为空：须提供正净价、非负收益率或正结算金额至少一项',
    });
  }

  if (issues.length > 0) {
    return { ok: false, issues };
  }

  let priceOrYield: QuoteTerms['priceOrYield'] = { case: undefined };

  if (hasClean) {
    if (!units?.pricePer100) {
      issues.push({
        field: 'opening_base.units.price_per_100',
        message: '账户快照缺少 price_per_100 单位定义',
      });
    } else {
      try {
        const dec = createDecimalValue(
          trimmedClean,
          units.pricePer100,
          'quote_terms.clean_price_per_100',
        );
        priceOrYield = { case: 'cleanPricePer100', value: dec };
      } catch (err) {
        issues.push({
          field: 'quote_terms.clean_price_per_100',
          message: err instanceof Error ? err.message : '净价格式非法',
        });
      }
    }
  } else if (hasYield) {
    if (!units?.rate) {
      issues.push({
        field: 'opening_base.units.rate',
        message: '账户快照缺少 rate 单位定义',
      });
    } else {
      try {
        const dec = createDecimalValue(trimmedYield, units.rate, 'quote_terms.yield_to_maturity');
        priceOrYield = { case: 'yieldToMaturity', value: dec };
      } catch (err) {
        issues.push({
          field: 'quote_terms.yield_to_maturity',
          message: err instanceof Error ? err.message : '收益率格式非法',
        });
      }
    }
  }

  let settlementAmount;
  if (hasSettlement) {
    if (!units?.currencyAmount) {
      issues.push({
        field: 'opening_base.units.currency_amount',
        message: '账户快照缺少 currency_amount 单位定义',
      });
    } else {
      try {
        settlementAmount = createDecimalValue(
          trimmedSettlement,
          units.currencyAmount,
          'quote_terms.settlement_amount',
        );
      } catch (err) {
        issues.push({
          field: 'quote_terms.settlement_amount',
          message: err instanceof Error ? err.message : '结算金额格式非法',
        });
      }
    }
  }

  if (issues.length > 0) {
    return { ok: false, issues };
  }

  const quoteTerms = create(QuoteTermsSchema, {
    priceOrYield,
    ...(settlementAmount !== undefined ? { settlementAmount } : {}),
  });

  return { ok: true, quoteTerms };
}

/**
 * Produces the effective proposal for validation & submission by merging user-edited quote
 * values only when the user actually modified the quote form; otherwise preserves the exact
 * wire `proposal.quoteTerms` (including any unit/value defect from the parser so validation
 * catches it strictly).
 */
export function buildEffectiveProposal(
  baseProposal: NlpEarmarkProposal,
  quoteForm: DraftQuoteFormValues,
  quoteDirty: boolean,
  units: AnalysisUnits | undefined,
):
  | { readonly ok: true; readonly proposal: NlpEarmarkProposal }
  | { readonly ok: false; readonly issues: readonly ValidationIssue[] } {
  if (!quoteDirty) {
    return { ok: true, proposal: baseProposal };
  }

  const built = buildQuoteTermsFromForm(quoteForm, units);
  if (!built.ok) {
    return built;
  }

  const merged = create(NlpEarmarkProposalSchema, {
    parserSourceId: baseProposal.parserSourceId,
    modelVersion: baseProposal.modelVersion,
    rawSourceText: baseProposal.rawSourceText,
    anchors: baseProposal.anchors,
    ...(baseProposal.confidenceScore !== undefined
      ? { confidenceScore: baseProposal.confidenceScore }
      : {}),
    accountId: baseProposal.accountId,
    instrumentCode: baseProposal.instrumentCode,
    side: baseProposal.side,
    settlementBucket: baseProposal.settlementBucket,
    ...(baseProposal.notional !== undefined ? { notional: baseProposal.notional } : {}),
    quoteTerms: built.quoteTerms,
    suggestedTargetState: baseProposal.suggestedTargetState,
  });

  return { ok: true, proposal: merged };
}

/**
 * Full proposal validation combining form-level quote construction and canonical protobuf
 * proposal validation (UTF-8 anchors, account binding, positive notional + currency unit,
 * quote terms + units, and `confidence_score` in `[0, 1]` with `dimensionless` unit ID+version).
 */
export function validateProposalForSubmission(
  baseProposal: NlpEarmarkProposal,
  quoteForm: DraftQuoteFormValues,
  quoteDirty: boolean,
  openingBaseUnits: AnalysisUnits | undefined,
  expectedAccountId: string,
):
  | { readonly ok: true; readonly proposal: NlpEarmarkProposal }
  | { readonly ok: false; readonly issues: readonly ValidationIssue[] } {
  const effective = buildEffectiveProposal(baseProposal, quoteForm, quoteDirty, openingBaseUnits);
  if (!effective.ok) {
    // Also run non-quote proposal checks so unit/anchor errors remain visible alongside quote errors.
    const baseCheck = validateNlpEarmarkProposal(baseProposal, openingBaseUnits, expectedAccountId);
    const nonQuoteIssues = baseCheck.ok
      ? []
      : baseCheck.issues.filter((item) => !item.field.startsWith('proposal.quote_terms'));
    return {
      ok: false,
      issues: [...effective.issues, ...nonQuoteIssues],
    };
  }

  const validation = validateNlpEarmarkProposal(
    effective.proposal,
    openingBaseUnits,
    expectedAccountId,
  );
  if (!validation.ok) {
    return { ok: false, issues: validation.issues };
  }

  return { ok: true, proposal: effective.proposal };
}

/**
 * Validates that a `DraftEarmarkRecord` carries a complete, verifiable `RuleLineage`.
 */
export function validateRuleLineage(lineage: RuleLineage | undefined): ValidationResult {
  const issues: ValidationIssue[] = [];
  if (!lineage) {
    issues.push({
      field: 'rule_lineage',
      message: '缺少监管规则谱系 (RuleLineage)，阻断确认',
    });
    return { ok: false, issues };
  }

  if (!lineage.packId || !isValidUlid(lineage.packId.value)) {
    issues.push({
      field: 'rule_lineage.pack_id',
      message: 'rule_lineage.pack_id 缺失或不是合法 ULID',
    });
  }
  if (lineage.version < 1n) {
    issues.push({
      field: 'rule_lineage.version',
      message: 'rule_lineage.version 必须 >= 1',
    });
  }
  if (!isNonEmptyTrimmed(lineage.source)) {
    issues.push({
      field: 'rule_lineage.source',
      message: 'rule_lineage.source 不能为空',
    });
  }
  try {
    validateSha256(lineage.contentHash, 'rule_lineage.content_hash');
  } catch (err) {
    issues.push({
      field: 'rule_lineage.content_hash',
      message: err instanceof Error ? err.message : 'content_hash 非法',
    });
  }
  try {
    validateSha256(lineage.parsedConstraintsHash, 'rule_lineage.parsed_constraints_hash');
  } catch (err) {
    issues.push({
      field: 'rule_lineage.parsed_constraints_hash',
      message: err instanceof Error ? err.message : 'parsed_constraints_hash 非法',
    });
  }

  return issues.length === 0 ? { ok: true } : { ok: false, issues };
}

export function formatRuleLineageSummary(lineage: RuleLineage | undefined): {
  readonly packId: string;
  readonly version: string;
  readonly source: string;
  readonly contentHashHex: string;
  readonly parsedConstraintsHashHex: string;
} | null {
  if (!lineage) {
    return null;
  }
  let contentHashHex = '未提供';
  try {
    contentHashHex = sha256ToHex(lineage.contentHash, 'rule_lineage.content_hash');
  } catch {
    contentHashHex = '无效摘要';
  }
  let parsedConstraintsHashHex = '未提供';
  try {
    parsedConstraintsHashHex = sha256ToHex(
      lineage.parsedConstraintsHash,
      'rule_lineage.parsed_constraints_hash',
    );
  } catch {
    parsedConstraintsHashHex = '无效摘要';
  }
  return {
    packId: lineage.packId?.value || '未提供',
    version: lineage.version >= 1n ? `v${lineage.version.toString()}` : '无效版本',
    source: lineage.source || '未提供',
    contentHashHex,
    parsedConstraintsHashHex,
  };
}

/**
 * Classifies server errors from `SubmitDraftEarmark` or `ConfirmDraftEarmark` to identify
 * regulatory rule-pack missing/invalid errors, missing limit ceilings, or breached limits.
 * Matches `crates/ficant-api/src/error.rs::map_field_violations`.
 */
export function classifyRegulatoryError(error: BlotterClientError): RegulatoryBlockInfo | null {
  const violationDetails = error.fieldViolations.map((v) => `${v.field}: ${v.description}`);

  const hasRulePackViolation = error.fieldViolations.some(
    (v) => v.field === 'regulatory_rule_pack',
  );
  const constraintViolations = error.fieldViolations.filter((v) =>
    v.field.startsWith('constraints['),
  );

  if (hasRulePackViolation) {
    return {
      blocked: true,
      kind: 'missing_rule_pack',
      summary: '监管规则包缺失、未验证或已失效，已阻断提交/确认',
      details: violationDetails.length > 0 ? violationDetails : [error.message],
    };
  }

  if (constraintViolations.length > 0) {
    const isMissingLimit =
      error.errorCode === ErrorCode.LINEAGE_INCOMPLETE ||
      constraintViolations.some((v) => v.description.toLowerCase().includes('missing'));
    return {
      blocked: true,
      kind: isMissingLimit ? 'missing_limit' : 'limit_exceeded',
      summary: isMissingLimit
        ? '监管限额上限配置缺失，已阻断提交/确认'
        : '拟定交易超出监管限额上限，已阻断提交/确认',
      details: violationDetails,
    };
  }

  if (
    error.errorCode === ErrorCode.LINEAGE_INCOMPLETE ||
    error.errorCode === ErrorCode.HASH_MISMATCH
  ) {
    return {
      blocked: true,
      kind: 'lineage_invalid',
      summary: '监管规则校验或摘要不完整，已阻断提交/确认',
      details: violationDetails.length > 0 ? violationDetails : [error.message],
    };
  }

  return null;
}

/**
 * Classifies the risk preview fields of a `DraftEarmarkRecord` right next to the Confirm button.
 *
 * Explicitly distinguishes:
 * - Covered non-zero PRE_TAX DV01 delta
 * - Covered zero PRE_TAX DV01 delta (e.g. HOLD draft where delta is numeric `0`)
 * - Missing PRE_TAX DV01 delta (`undefined` -> `"未提供"`, disables Confirm)
 * - Valid uncovered draft (`preview_uncovered_reason` + `preview_delta_uncovered_notional` + unit)
 * - Incomplete uncovered draft (missing reason or delta notional -> `"未提供"`, disables Confirm)
 */
export function classifyDraftPreview(
  draft: DraftEarmarkRecord,
  openingBaseUnits?: AnalysisUnits,
): DraftPreviewClassification {
  const previewAtSeqText = draft.previewAtSeq.toString();

  if (draft.previewCoverageStatus === CoverageStatus.COVERED_PRE_TAX) {
    if (draft.previewDeltaPreTaxDv01 === undefined) {
      return {
        kind: 'covered_missing_delta',
        dv01Text: '未提供',
        dv01UnitText: '未提供',
        previewAtSeqText,
        ready: false,
        requiresUncoveredAck: false,
        missingReason: '已覆盖草案缺少 preview_delta_pre_tax_dv01，禁止确认',
      };
    }

    let dv01Text: string;
    let zeroDelta: boolean;
    try {
      validateUnitRef(draft.previewDeltaPreTaxDv01.unit, 'preview_delta_pre_tax_dv01.unit');
      if (
        openingBaseUnits?.dv01 !== undefined &&
        !unitsEqual(draft.previewDeltaPreTaxDv01.unit, openingBaseUnits.dv01)
      ) {
        return {
          kind: 'covered_missing_delta',
          dv01Text: '未提供',
          dv01UnitText: formatUnitRef(draft.previewDeltaPreTaxDv01.unit),
          previewAtSeqText,
          ready: false,
          requiresUncoveredAck: false,
          missingReason: 'preview_delta_pre_tax_dv01 单位与账户 dv01 单位不匹配，禁止确认',
        };
      }
      dv01Text = formatDecimalValue(draft.previewDeltaPreTaxDv01, { grouping: true });
      zeroDelta = isZeroDecimal(draft.previewDeltaPreTaxDv01);
    } catch {
      return {
        kind: 'covered_missing_delta',
        dv01Text: '未提供',
        dv01UnitText: formatUnitRef(draft.previewDeltaPreTaxDv01.unit),
        previewAtSeqText,
        ready: false,
        requiresUncoveredAck: false,
        missingReason: 'preview_delta_pre_tax_dv01 数值或单位非法，禁止确认',
      };
    }

    const dv01UnitText = formatUnitRef(draft.previewDeltaPreTaxDv01.unit);
    if (zeroDelta) {
      return {
        kind: 'covered_zero_hold',
        dv01Text,
        dv01UnitText,
        previewAtSeqText,
        ready: true,
        requiresUncoveredAck: false,
      };
    }

    return {
      kind: 'covered_nonzero',
      dv01Text,
      dv01UnitText,
      previewAtSeqText,
      ready: true,
      requiresUncoveredAck: false,
    };
  }

  if (draft.previewCoverageStatus === CoverageStatus.UNCOVERED) {
    const reasonInfo = formatUncoveredReason(draft.previewUncoveredReason);
    const hasReason = draft.previewUncoveredReason !== UncoveredReason.UNSPECIFIED;
    const rawDelta = draft.previewDeltaUncoveredNotional;

    if (!hasReason || rawDelta === undefined) {
      const missingParts: string[] = [];
      if (!hasReason) {
        missingParts.push('preview_uncovered_reason');
      }
      if (rawDelta === undefined) {
        missingParts.push('preview_delta_uncovered_notional');
      }
      let partialDeltaText = '未提供';
      let partialUnitText = '未提供';
      if (rawDelta !== undefined) {
        try {
          partialDeltaText = formatDecimalValue(rawDelta, { grouping: true });
          partialUnitText = formatUnitRef(rawDelta.unit);
        } catch {
          partialDeltaText = '未提供';
          partialUnitText = '未提供';
        }
      }
      return {
        kind: 'uncovered_incomplete',
        uncoveredReasonLabel: reasonInfo.label,
        uncoveredReasonCode: reasonInfo.code,
        deltaNotionalText: partialDeltaText,
        deltaNotionalUnitText: partialUnitText,
        previewAtSeqText,
        ready: false,
        requiresUncoveredAck: false,
        missingReason: `未覆盖草案缺少必要预览字段 (${missingParts.join(', ')})，禁止确认`,
      };
    }

    let deltaNotionalText: string;
    let deltaNotionalIsZero: boolean;
    try {
      validateUnitRef(rawDelta.unit, 'preview_delta_uncovered_notional.unit');
      if (
        openingBaseUnits?.currencyAmount !== undefined &&
        !unitsEqual(rawDelta.unit, openingBaseUnits.currencyAmount)
      ) {
        return {
          kind: 'uncovered_incomplete',
          uncoveredReasonLabel: reasonInfo.label,
          uncoveredReasonCode: reasonInfo.code,
          deltaNotionalText: '未提供',
          deltaNotionalUnitText: formatUnitRef(rawDelta.unit),
          previewAtSeqText,
          ready: false,
          requiresUncoveredAck: false,
          missingReason: 'preview_delta_uncovered_notional 单位与账户 currency_amount 不匹配，禁止确认',
        };
      }
      deltaNotionalText = formatDecimalValue(rawDelta, { grouping: true });
      deltaNotionalIsZero = isZeroDecimal(rawDelta);
    } catch {
      return {
        kind: 'uncovered_incomplete',
        uncoveredReasonLabel: reasonInfo.label,
        uncoveredReasonCode: reasonInfo.code,
        deltaNotionalText: '未提供',
        deltaNotionalUnitText: formatUnitRef(rawDelta.unit),
        previewAtSeqText,
        ready: false,
        requiresUncoveredAck: false,
        missingReason: 'preview_delta_uncovered_notional 数值或单位非法，禁止确认',
      };
    }

    return {
      kind: 'uncovered_valid',
      uncoveredReasonLabel: reasonInfo.label,
      uncoveredReasonCode: reasonInfo.code,
      deltaNotionalText,
      deltaNotionalIsZero,
      deltaNotionalUnitText: formatUnitRef(rawDelta.unit),
      previewAtSeqText,
      ready: true,
      requiresUncoveredAck: true,
    };
  }

  return {
    kind: 'invalid_preview',
    previewAtSeqText,
    ready: false,
    requiresUncoveredAck: false,
    missingReason: '草案缺少合法的 preview_coverage_status，禁止确认',
  };
}

export interface DraftConfirmationGate {
  readonly preview: DraftPreviewClassification;
  readonly lineageValid: boolean;
  readonly lineageIssues: readonly ValidationIssue[];
  readonly readyIssues: readonly ValidationIssue[];
  readonly showUncoveredAckCheckbox: boolean;
  readonly canConfirm: boolean;
  readonly blockingReasons: readonly string[];
}

/**
 * Evaluates all confirmation preconditions for a `DraftEarmarkRecord`:
 * - Draft status must be `PENDING_CONFIRMATION`
 * - Preview must NOT be invalidated by `CONCURRENCY_CONFLICT`
 * - Regulatory check must NOT be blocked AND `RuleLineage` must be valid
 * - `validateDraftReadyForConfirmation(draft)` and `classifyDraftPreview` must both pass
 * - Uncovered-risk acknowledgement checkbox is shown ONLY when rules are valid AND the draft
 *   is a valid uncovered draft AND not invalidated
 * - Confirmation button is enabled ONLY when all preconditions pass (and checkbox is checked
 *   whenever `showUncoveredAckCheckbox` is true)
 */
export function evaluateDraftConfirmationGate(params: {
  readonly draft: DraftEarmarkRecord;
  readonly openingBaseUnits: AnalysisUnits | undefined;
  readonly previewInvalidated: boolean;
  readonly regulatoryBlock: RegulatoryBlockInfo | null;
  readonly acknowledgeUncoveredDv01: boolean;
  readonly writesPaused: boolean;
  readonly traderRoleBlocked: boolean;
  readonly confirming: boolean;
}): DraftConfirmationGate {
  const { draft, openingBaseUnits } = params;
  const preview = classifyDraftPreview(draft, openingBaseUnits);
  const readyRes = validateDraftReadyForConfirmation(draft);
  const lineageRes = validateRuleLineage(draft.ruleLineage);

  const blockingReasons: string[] = [];

  if (draft.status !== DraftEarmarkStatus.PENDING_CONFIRMATION) {
    blockingReasons.push(`当前草案状态为 ${formatDraftStatus(draft.status)}，不可确认`);
  }
  if (params.previewInvalidated) {
    blockingReasons.push('上下文已发生冲突 (CONCURRENCY_CONFLICT)，旧预览和确认令牌已失效，请重新提交');
  }
  if (params.regulatoryBlock?.blocked) {
    blockingReasons.push(params.regulatoryBlock.summary);
  }
  if (!lineageRes.ok) {
    for (const issue of lineageRes.issues) {
      blockingReasons.push(`${issue.field}: ${issue.message}`);
    }
  }
  if (!readyRes.ok) {
    for (const issue of readyRes.issues) {
      blockingReasons.push(`${issue.field}: ${issue.message}`);
    }
  }
  if (!preview.ready) {
    blockingReasons.push(preview.missingReason);
  }
  if (params.writesPaused) {
    blockingReasons.push('当前会话未认证或正在恢复屏障，已暂停写操作');
  }
  if (params.traderRoleBlocked) {
    blockingReasons.push('ConfirmDraftEarmark 仅限 TRADER 角色执行');
  }

  // Strictly show uncovered risk checkbox ONLY for valid uncovered drafts with valid regulatory
  // lineage and no conflict/rule block.
  const showUncoveredAckCheckbox =
    draft.status === DraftEarmarkStatus.PENDING_CONFIRMATION &&
    !params.previewInvalidated &&
    !params.regulatoryBlock?.blocked &&
    lineageRes.ok &&
    readyRes.ok &&
    preview.kind === 'uncovered_valid';

  if (showUncoveredAckCheckbox && !params.acknowledgeUncoveredDv01) {
    blockingReasons.push('未覆盖风险草案须先勾选人工风险确认项');
  }

  const canConfirm = blockingReasons.length === 0 && !params.confirming;

  return {
    preview,
    lineageValid: lineageRes.ok,
    lineageIssues: lineageRes.ok ? [] : lineageRes.issues,
    readyIssues: readyRes.ok ? [] : readyRes.issues,
    showUncoveredAckCheckbox,
    canConfirm,
    blockingReasons,
  };
}
