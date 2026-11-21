import type {
  DraftEarmarkRecord,
  QuoteTerms,
} from '../generated/ficant/blotter/v1/model_pb';
import {
  CoverageStatus,
  DraftTargetState,
  PendingMatchResolutionAction,
  PreviewHedgeScope,
  SettlementBucket,
  TradeSide,
  UncoveredReason,
} from '../generated/ficant/blotter/v1/model_pb';
import type {
  CancelCommittedRequest,
  ConfirmHoldRequest,
  CreateCommittedRequest,
  CreateHoldRequest,
  ExtendHoldRequest,
  PreviewCoveredHedgeRequest,
  PreviewCoveredHedgeResult,
  ReleaseHoldRequest,
  ResolvePendingMatchRequest,
  SetFeedControlRequest,
} from '../generated/ficant/blotter/v1/service_pb';
import type { NlpEarmarkProposal } from '../generated/ficant/blotter/v1/spi_pb';
import type { DecimalValue, UnitRef } from '../generated/ficant/core/v1/common_pb';
import {
  BondTaxBasis,
  type AnalysisUnits,
} from '../generated/ficant/rates/v1/analytics_pb';
import {
  SCALE_12_FACTOR,
  validateCanonicalDecimalValue,
  type ValidateDecimalOptions,
} from './decimal';
import {
  isNonEmptyTrimmed,
  isValidUlid,
  unitsEqual,
  validateMarketTime,
  validateSha256,
  validateUnitRef,
} from './primitives';
import { validateSourceTextAnchors } from './utf8';

export const MIN_HOLD_TTL_SECONDS = 1800;
export const MAX_HOLD_TTL_SECONDS = 2700;

export interface ValidationIssue {
  readonly field: string;
  readonly message: string;
}

export type ValidationResult =
  | { readonly ok: true }
  | { readonly ok: false; readonly issues: readonly ValidationIssue[] };

export class ClientValidationError extends Error {
  readonly issues: readonly ValidationIssue[];

  constructor(issues: readonly ValidationIssue[], prefix = 'Validation failed') {
    const summary = issues.map((i) => `${i.field}: ${i.message}`).join('; ');
    super(`${prefix}: ${summary}`);
    this.name = 'ClientValidationError';
    this.issues = issues;
  }
}

export function assertValid(result: ValidationResult, prefix?: string): void {
  if (!result.ok) {
    throw new ClientValidationError(result.issues, prefix);
  }
}

function toValidationResult(issues: ValidationIssue[]): ValidationResult {
  return issues.length === 0 ? { ok: true } : { ok: false, issues };
}

function checkDecimal(
  issues: ValidationIssue[],
  value: DecimalValue | undefined,
  options: ValidateDecimalOptions & { readonly field: string },
): bigint | null {
  try {
    return validateCanonicalDecimalValue(value, options).scaledI128;
  } catch (err) {
    const msg = err instanceof Error ? err.message : 'invalid decimal value';
    issues.push({ field: options.field, message: msg });
    return null;
  }
}

export function validateHoldTtlSeconds(
  ttlSeconds: number,
  field = 'ttl_seconds',
): ValidationIssue | null {
  if (
    !Number.isInteger(ttlSeconds) ||
    ttlSeconds < MIN_HOLD_TTL_SECONDS ||
    ttlSeconds > MAX_HOLD_TTL_SECONDS
  ) {
    return {
      field,
      message: `must be an integer in ${MIN_HOLD_TTL_SECONDS}..=${MAX_HOLD_TTL_SECONDS}, got ${String(ttlSeconds)}`,
    };
  }
  return null;
}

export function validateNonEmptyReason(
  reason: string,
  field = 'reason',
): ValidationIssue | null {
  if (reason.trim().length === 0) {
    return { field, message: 'reason must not be empty' };
  }
  return null;
}

export function validateNonEmptyId(value: string, field: string): ValidationIssue | null {
  if (!isNonEmptyTrimmed(value)) {
    return { field, message: 'must be a non-empty string without leading/trailing whitespace' };
  }
  return null;
}

function validateSide(side: TradeSide, field = 'side'): ValidationIssue | null {
  if (side !== TradeSide.BUY && side !== TradeSide.SELL) {
    return { field, message: 'must be BUY or SELL' };
  }
  return null;
}

function validateBucket(
  bucket: SettlementBucket,
  field = 'settlement_bucket',
): ValidationIssue | null {
  if (bucket !== SettlementBucket.T_PLUS_0 && bucket !== SettlementBucket.T_PLUS_1) {
    return { field, message: 'must be T_PLUS_0 or T_PLUS_1' };
  }
  return null;
}

export interface ValidateQuoteTermsOptions {
  readonly required: boolean;
  readonly units?: AnalysisUnits | undefined;
  readonly fieldPrefix?: string;
}

/**
 * Validates `QuoteTerms` matching `crates/ficant-domain/src/blotter/model.rs::QuoteTerms::validate`:
 * - Accepts positive `settlement_amount` alone without requiring clean price or yield.
 * - `clean_price_per_100` (if selected in `price_or_yield`) must be strictly `> 0`.
 * - `yield_to_maturity` (if selected in `price_or_yield`) must be `>= 0`.
 * - `settlement_amount` (if present) must be strictly `> 0`.
 * - Rejects empty `QuoteTerms` (neither `price_or_yield` nor `settlement_amount`).
 */
export function validateQuoteTerms(
  quote: QuoteTerms | undefined,
  options: ValidateQuoteTermsOptions,
): ValidationResult {
  const prefix = options.fieldPrefix ?? 'quote_terms';
  const issues: ValidationIssue[] = [];

  if (quote === undefined) {
    if (options.required) {
      issues.push({ field: prefix, message: 'quote_terms is required' });
    }
    return toValidationResult(issues);
  }

  const hasPriceOrYield =
    quote.priceOrYield.case === 'cleanPricePer100' ||
    quote.priceOrYield.case === 'yieldToMaturity';
  const hasSettlementAmount = quote.settlementAmount !== undefined;

  if (!hasPriceOrYield && !hasSettlementAmount) {
    issues.push({
      field: prefix,
      message: 'must specify clean_price_per_100, yield_to_maturity, or positive settlement_amount',
    });
    return toValidationResult(issues);
  }

  if (quote.priceOrYield.case === 'cleanPricePer100') {
    const decimalOpts: ValidateDecimalOptions & { readonly field: string } = {
      field: `${prefix}.clean_price_per_100`,
      sign: 'positive',
      requireUnit: options.units !== undefined,
      ...(options.units?.pricePer100 !== undefined
        ? { expectedUnit: options.units.pricePer100 }
        : {}),
    };
    checkDecimal(issues, quote.priceOrYield.value, decimalOpts);
  } else if (quote.priceOrYield.case === 'yieldToMaturity') {
    const decimalOpts: ValidateDecimalOptions & { readonly field: string } = {
      field: `${prefix}.yield_to_maturity`,
      sign: 'non-negative',
      requireUnit: options.units !== undefined,
      ...(options.units?.rate !== undefined ? { expectedUnit: options.units.rate } : {}),
    };
    checkDecimal(issues, quote.priceOrYield.value, decimalOpts);
  }

  if (quote.settlementAmount !== undefined) {
    const decimalOpts: ValidateDecimalOptions & { readonly field: string } = {
      field: `${prefix}.settlement_amount`,
      sign: 'positive',
      requireUnit: options.units !== undefined,
      ...(options.units?.currencyAmount !== undefined
        ? { expectedUnit: options.units.currencyAmount }
        : {}),
    };
    checkDecimal(issues, quote.settlementAmount, decimalOpts);
  }

  return toValidationResult(issues);
}

export function validateCreateHoldRequest(
  req: Pick<
    CreateHoldRequest,
    | 'accountId'
    | 'instrumentCode'
    | 'side'
    | 'settlementBucket'
    | 'notional'
    | 'quoteTerms'
    | 'ttlSeconds'
    | 'expectedPositionVersion'
  >,
  units?: AnalysisUnits,
): ValidationResult {
  const issues: ValidationIssue[] = [];
  const acctErr = validateNonEmptyId(req.accountId, 'account_id');
  if (acctErr) issues.push(acctErr);
  const instErr = validateNonEmptyId(req.instrumentCode, 'instrument_code');
  if (instErr) issues.push(instErr);
  const sideErr = validateSide(req.side);
  if (sideErr) issues.push(sideErr);
  const bucketErr = validateBucket(req.settlementBucket);
  if (bucketErr) issues.push(bucketErr);

  checkDecimal(issues, req.notional, {
    field: 'notional',
    sign: 'positive',
    requireUnit: units !== undefined,
    ...(units?.currencyAmount !== undefined ? { expectedUnit: units.currencyAmount } : {}),
  });

  const quoteRes = validateQuoteTerms(req.quoteTerms, { required: false, units });
  if (!quoteRes.ok) {
    issues.push(...quoteRes.issues);
  }

  const ttlErr = validateHoldTtlSeconds(req.ttlSeconds, 'ttl_seconds');
  if (ttlErr) issues.push(ttlErr);

  if (req.expectedPositionVersion < 0n) {
    issues.push({ field: 'expected_position_version', message: 'must be non-negative' });
  }

  return toValidationResult(issues);
}

export function validateExtendHoldRequest(
  req: Pick<
    ExtendHoldRequest,
    'accountId' | 'earmarkId' | 'expectedEarmarkRevision' | 'ttlSeconds'
  >,
): ValidationResult {
  const issues: ValidationIssue[] = [];
  const acctErr = validateNonEmptyId(req.accountId, 'account_id');
  if (acctErr) issues.push(acctErr);
  if (!req.earmarkId || !isValidUlid(req.earmarkId.value)) {
    issues.push({ field: 'earmark_id', message: 'must be a valid ULID' });
  }
  if (req.expectedEarmarkRevision < 1n) {
    issues.push({ field: 'expected_earmark_revision', message: 'must be >= 1' });
  }
  const ttlErr = validateHoldTtlSeconds(req.ttlSeconds, 'ttl_seconds');
  if (ttlErr) issues.push(ttlErr);
  return toValidationResult(issues);
}

export function validateConfirmHoldRequest(
  req: Pick<
    ConfirmHoldRequest,
    | 'accountId'
    | 'earmarkId'
    | 'expectedEarmarkRevision'
    | 'expectedPositionVersion'
    | 'quoteTerms'
  >,
  units?: AnalysisUnits,
): ValidationResult {
  const issues: ValidationIssue[] = [];
  const acctErr = validateNonEmptyId(req.accountId, 'account_id');
  if (acctErr) issues.push(acctErr);
  if (!req.earmarkId || !isValidUlid(req.earmarkId.value)) {
    issues.push({ field: 'earmark_id', message: 'must be a valid ULID' });
  }
  if (req.expectedEarmarkRevision < 1n) {
    issues.push({ field: 'expected_earmark_revision', message: 'must be >= 1' });
  }
  if (req.expectedPositionVersion < 0n) {
    issues.push({ field: 'expected_position_version', message: 'must be non-negative' });
  }
  const quoteRes = validateQuoteTerms(req.quoteTerms, { required: true, units });
  if (!quoteRes.ok) {
    issues.push(...quoteRes.issues);
  }
  return toValidationResult(issues);
}

export function validateReleaseHoldRequest(
  req: Pick<
    ReleaseHoldRequest,
    'accountId' | 'earmarkId' | 'expectedEarmarkRevision' | 'reason'
  >,
): ValidationResult {
  const issues: ValidationIssue[] = [];
  const acctErr = validateNonEmptyId(req.accountId, 'account_id');
  if (acctErr) issues.push(acctErr);
  if (!req.earmarkId || !isValidUlid(req.earmarkId.value)) {
    issues.push({ field: 'earmark_id', message: 'must be a valid ULID' });
  }
  if (req.expectedEarmarkRevision < 1n) {
    issues.push({ field: 'expected_earmark_revision', message: 'must be >= 1' });
  }
  const reasonErr = validateNonEmptyReason(req.reason, 'reason');
  if (reasonErr) issues.push(reasonErr);
  return toValidationResult(issues);
}

export function validateCreateCommittedRequest(
  req: Pick<
    CreateCommittedRequest,
    | 'accountId'
    | 'instrumentCode'
    | 'side'
    | 'settlementBucket'
    | 'notional'
    | 'quoteTerms'
    | 'expectedPositionVersion'
  >,
  units?: AnalysisUnits,
): ValidationResult {
  const issues: ValidationIssue[] = [];
  const acctErr = validateNonEmptyId(req.accountId, 'account_id');
  if (acctErr) issues.push(acctErr);
  const instErr = validateNonEmptyId(req.instrumentCode, 'instrument_code');
  if (instErr) issues.push(instErr);
  const sideErr = validateSide(req.side);
  if (sideErr) issues.push(sideErr);
  const bucketErr = validateBucket(req.settlementBucket);
  if (bucketErr) issues.push(bucketErr);

  checkDecimal(issues, req.notional, {
    field: 'notional',
    sign: 'positive',
    requireUnit: units !== undefined,
    ...(units?.currencyAmount !== undefined ? { expectedUnit: units.currencyAmount } : {}),
  });

  const quoteRes = validateQuoteTerms(req.quoteTerms, { required: true, units });
  if (!quoteRes.ok) {
    issues.push(...quoteRes.issues);
  }

  if (req.expectedPositionVersion < 0n) {
    issues.push({ field: 'expected_position_version', message: 'must be non-negative' });
  }

  return toValidationResult(issues);
}

export function validateCancelCommittedRequest(
  req: Pick<
    CancelCommittedRequest,
    'accountId' | 'earmarkId' | 'expectedEarmarkRevision' | 'reason'
  >,
): ValidationResult {
  const issues: ValidationIssue[] = [];
  const acctErr = validateNonEmptyId(req.accountId, 'account_id');
  if (acctErr) issues.push(acctErr);
  if (!req.earmarkId || !isValidUlid(req.earmarkId.value)) {
    issues.push({ field: 'earmark_id', message: 'must be a valid ULID' });
  }
  if (req.expectedEarmarkRevision < 1n) {
    issues.push({ field: 'expected_earmark_revision', message: 'must be >= 1' });
  }
  const reasonErr = validateNonEmptyReason(req.reason, 'reason');
  if (reasonErr) issues.push(reasonErr);
  return toValidationResult(issues);
}

export function validateSetFeedControlRequest(
  req: Pick<
    SetFeedControlRequest,
    'sourceId' | 'expectedFeedStatusRevision' | 'pausedByOperator' | 'forceClearGapPause' | 'reason'
  >,
): ValidationResult {
  const issues: ValidationIssue[] = [];
  const srcErr = validateNonEmptyId(req.sourceId, 'source_id');
  if (srcErr) issues.push(srcErr);
  if (req.expectedFeedStatusRevision < 0n) {
    issues.push({ field: 'expected_feed_status_revision', message: 'must be non-negative' });
  }
  if (req.forceClearGapPause) {
    const reasonErr = validateNonEmptyReason(req.reason, 'reason');
    if (reasonErr) {
      issues.push({
        field: 'reason',
        message: 'force_clear_gap_pause requires a non-empty reason',
      });
    }
  }
  return toValidationResult(issues);
}

export function validateResolvePendingMatchRequest(
  req: Pick<
    ResolvePendingMatchRequest,
    | 'accountId'
    | 'pendingMatchId'
    | 'expectedPendingRevision'
    | 'expectedPositionVersion'
    | 'action'
    | 'expectedEarmarkRevision'
    | 'targetEarmarkId'
    | 'offsetNotional'
    | 'reason'
    | 'baselineDelta'
  >,
  units?: AnalysisUnits,
): ValidationResult {
  const issues: ValidationIssue[] = [];
  const acctErr = validateNonEmptyId(req.accountId, 'account_id');
  if (acctErr) issues.push(acctErr);
  if (!req.pendingMatchId || !isValidUlid(req.pendingMatchId.value)) {
    issues.push({ field: 'pending_match_id', message: 'must be a valid ULID' });
  }
  if (req.expectedPendingRevision < 1n) {
    issues.push({ field: 'expected_pending_revision', message: 'must be >= 1' });
  }
  if (req.expectedPositionVersion < 0n) {
    issues.push({ field: 'expected_position_version', message: 'must be non-negative' });
  }

  switch (req.action) {
    case PendingMatchResolutionAction.MATCH_FULL: {
      if (!req.targetEarmarkId || !isValidUlid(req.targetEarmarkId.value)) {
        issues.push({ field: 'target_earmark_id', message: 'MATCH_FULL requires target_earmark_id' });
      }
      if (req.expectedEarmarkRevision === undefined || req.expectedEarmarkRevision < 1n) {
        issues.push({
          field: 'expected_earmark_revision',
          message: 'MATCH_FULL requires expected_earmark_revision >= 1',
        });
      }
      if (req.offsetNotional !== undefined) {
        issues.push({ field: 'offset_notional', message: 'MATCH_FULL must omit offset_notional' });
      }
      if (req.baselineDelta !== undefined) {
        issues.push({ field: 'baseline_delta', message: 'MATCH_FULL must omit baseline_delta' });
      }
      break;
    }
    case PendingMatchResolutionAction.SPLIT_OFFSET: {
      if (!req.targetEarmarkId || !isValidUlid(req.targetEarmarkId.value)) {
        issues.push({
          field: 'target_earmark_id',
          message: 'SPLIT_OFFSET requires target_earmark_id',
        });
      }
      if (req.expectedEarmarkRevision === undefined || req.expectedEarmarkRevision < 1n) {
        issues.push({
          field: 'expected_earmark_revision',
          message: 'SPLIT_OFFSET requires expected_earmark_revision >= 1',
        });
      }
      checkDecimal(issues, req.offsetNotional, {
        field: 'offset_notional',
        sign: 'positive',
        requireUnit: units !== undefined,
        ...(units?.currencyAmount !== undefined ? { expectedUnit: units.currencyAmount } : {}),
      });
      if (req.baselineDelta !== undefined) {
        issues.push({ field: 'baseline_delta', message: 'SPLIT_OFFSET must omit baseline_delta' });
      }
      break;
    }
    case PendingMatchResolutionAction.ACCEPT_ORPHAN: {
      if (req.targetEarmarkId !== undefined) {
        issues.push({
          field: 'target_earmark_id',
          message: 'ACCEPT_ORPHAN must omit target_earmark_id',
        });
      }
      if (req.expectedEarmarkRevision !== undefined) {
        issues.push({
          field: 'expected_earmark_revision',
          message: 'ACCEPT_ORPHAN must omit expected_earmark_revision',
        });
      }
      if (req.offsetNotional !== undefined) {
        issues.push({
          field: 'offset_notional',
          message: 'ACCEPT_ORPHAN must omit offset_notional',
        });
      }
      if (req.baselineDelta !== undefined) {
        issues.push({
          field: 'baseline_delta',
          message: 'ACCEPT_ORPHAN must omit baseline_delta',
        });
      }
      break;
    }
    case PendingMatchResolutionAction.RELEASE_REJECT: {
      if (req.targetEarmarkId !== undefined) {
        issues.push({
          field: 'target_earmark_id',
          message: 'RELEASE_REJECT must omit target_earmark_id',
        });
      }
      if (req.expectedEarmarkRevision !== undefined) {
        issues.push({
          field: 'expected_earmark_revision',
          message: 'RELEASE_REJECT must omit expected_earmark_revision',
        });
      }
      if (req.offsetNotional !== undefined) {
        issues.push({
          field: 'offset_notional',
          message: 'RELEASE_REJECT must omit offset_notional',
        });
      }
      if (req.baselineDelta !== undefined) {
        issues.push({
          field: 'baseline_delta',
          message: 'RELEASE_REJECT must omit baseline_delta',
        });
      }
      const reasonErr = validateNonEmptyReason(req.reason, 'reason');
      if (reasonErr) issues.push(reasonErr);
      break;
    }
    case PendingMatchResolutionAction.APPLY_BASELINE_DELTA: {
      if (req.targetEarmarkId !== undefined) {
        issues.push({
          field: 'target_earmark_id',
          message: 'APPLY_BASELINE_DELTA must omit target_earmark_id',
        });
      }
      if (req.expectedEarmarkRevision !== undefined) {
        issues.push({
          field: 'expected_earmark_revision',
          message: 'APPLY_BASELINE_DELTA must omit expected_earmark_revision',
        });
      }
      if (req.offsetNotional !== undefined) {
        issues.push({
          field: 'offset_notional',
          message: 'APPLY_BASELINE_DELTA must omit offset_notional',
        });
      }
      if (!req.baselineDelta) {
        issues.push({
          field: 'baseline_delta',
          message: 'APPLY_BASELINE_DELTA requires baseline_delta',
        });
      } else {
        const bd = req.baselineDelta;
        const bErr = validateBucket(bd.settlementBucket, 'baseline_delta.settlement_bucket');
        if (bErr) issues.push(bErr);
        const rErr = validateNonEmptyReason(bd.reason, 'baseline_delta.reason');
        if (rErr) issues.push(rErr);
        checkDecimal(issues, bd.deltaNotional, {
          field: 'baseline_delta.delta_notional',
          sign: 'any',
          requireUnit: units !== undefined,
          ...(units?.currencyAmount !== undefined ? { expectedUnit: units.currencyAmount } : {}),
        });
        checkDecimal(issues, bd.deltaCash, {
          field: 'baseline_delta.delta_cash',
          sign: 'any',
          requireUnit: units !== undefined,
          ...(units?.currencyAmount !== undefined ? { expectedUnit: units.currencyAmount } : {}),
        });
        if (!bd.evidence) {
          issues.push({
            field: 'baseline_delta.evidence',
            message: 'APPLY_BASELINE_DELTA requires original BaseEvidence',
          });
        } else {
          const ev = bd.evidence;
          const evSrc = validateNonEmptyId(ev.sourceId, 'baseline_delta.evidence.source_id');
          if (evSrc) issues.push(evSrc);
          const evExt = validateNonEmptyId(
            ev.externalTradeId,
            'baseline_delta.evidence.external_trade_id',
          );
          if (evExt) issues.push(evExt);
          const evRef = validateNonEmptyId(ev.evidenceRef, 'baseline_delta.evidence.evidence_ref');
          if (evRef) issues.push(evRef);
          const evInst = validateNonEmptyId(
            ev.instrumentCode,
            'baseline_delta.evidence.instrument_code',
          );
          if (evInst) issues.push(evInst);
          if (ev.originalRevision < 1n) {
            issues.push({
              field: 'baseline_delta.evidence.original_revision',
              message: 'must be >= 1',
            });
          }
          if (ev.settlementBucket !== bd.settlementBucket) {
            issues.push({
              field: 'baseline_delta.settlement_bucket',
              message: 'baseline_delta.settlement_bucket must match evidence.settlement_bucket',
            });
          }
          try {
            validateSha256(ev.evidenceHash, 'baseline_delta.evidence.evidence_hash');
          } catch (err) {
            issues.push({
              field: 'baseline_delta.evidence.evidence_hash',
              message: err instanceof Error ? err.message : 'invalid sha256',
            });
          }
          try {
            validateMarketTime(ev.originalExecutedAt, 'baseline_delta.evidence.original_executed_at');
          } catch (err) {
            issues.push({
              field: 'baseline_delta.evidence.original_executed_at',
              message: err instanceof Error ? err.message : 'invalid market time',
            });
          }
        }
      }
      break;
    }
    default: {
      issues.push({ field: 'action', message: 'must be a supported PendingMatchResolutionAction' });
      break;
    }
  }

  return toValidationResult(issues);
}

/**
 * Validates an `NlpEarmarkProposal` before `SubmitDraftEarmark`.
 * Enforces:
 * - Required valid `quote_terms`
 * - Positive `notional` with `opening_base.units.currency_amount`
 * - `confidence_score` in `[0, 1]` with `unit == opening_base.units.dimensionless`
 * - Valid UTF-8 half-open byte `anchors` matching `raw_source_text`
 */
export function validateNlpEarmarkProposal(
  proposal: NlpEarmarkProposal | undefined,
  openingBaseUnits: AnalysisUnits | undefined,
  expectedAccountId?: string,
): ValidationResult {
  const issues: ValidationIssue[] = [];
  if (!proposal) {
    issues.push({ field: 'proposal', message: 'proposal is required' });
    return toValidationResult(issues);
  }

  const parserErr = validateNonEmptyId(proposal.parserSourceId, 'proposal.parser_source_id');
  if (parserErr) issues.push(parserErr);
  const modelErr = validateNonEmptyId(proposal.modelVersion, 'proposal.model_version');
  if (modelErr) issues.push(modelErr);
  const acctErr = validateNonEmptyId(proposal.accountId, 'proposal.account_id');
  if (acctErr) {
    issues.push(acctErr);
  } else if (expectedAccountId !== undefined && proposal.accountId !== expectedAccountId) {
    issues.push({
      field: 'proposal.account_id',
      message: `account_id "${proposal.accountId}" does not match expected "${expectedAccountId}"`,
    });
  }
  const instErr = validateNonEmptyId(proposal.instrumentCode, 'proposal.instrument_code');
  if (instErr) issues.push(instErr);
  const sideErr = validateSide(proposal.side, 'proposal.side');
  if (sideErr) issues.push(sideErr);
  const bucketErr = validateBucket(proposal.settlementBucket, 'proposal.settlement_bucket');
  if (bucketErr) issues.push(bucketErr);

  if (
    proposal.suggestedTargetState !== DraftTargetState.HOLD &&
    proposal.suggestedTargetState !== DraftTargetState.COMMITTED
  ) {
    issues.push({
      field: 'proposal.suggested_target_state',
      message: 'must be HOLD or COMMITTED',
    });
  }

  checkDecimal(issues, proposal.notional, {
    field: 'proposal.notional',
    sign: 'positive',
    requireUnit: true,
    ...(openingBaseUnits?.currencyAmount !== undefined
      ? { expectedUnit: openingBaseUnits.currencyAmount }
      : {}),
  });

  const quoteRes = validateQuoteTerms(proposal.quoteTerms, {
    required: true,
    units: openingBaseUnits,
    fieldPrefix: 'proposal.quote_terms',
  });
  if (!quoteRes.ok) {
    issues.push(...quoteRes.issues);
  }

  if (openingBaseUnits && !openingBaseUnits.dimensionless) {
    issues.push({
      field: 'opening_base.units.dimensionless',
      message: 'opening_base.units.dimensionless is missing',
    });
  }
  const confScaled = checkDecimal(issues, proposal.confidenceScore, {
    field: 'proposal.confidence_score',
    sign: 'non-negative',
    requireUnit: true,
    ...(openingBaseUnits?.dimensionless !== undefined
      ? { expectedUnit: openingBaseUnits.dimensionless }
      : {}),
  });
  if (confScaled !== null && confScaled > SCALE_12_FACTOR) {
    issues.push({
      field: 'proposal.confidence_score',
      message: 'confidence_score must be within [0, 1]',
    });
  }

  try {
    validateSourceTextAnchors(proposal.rawSourceText, proposal.anchors);
  } catch (err) {
    issues.push({
      field: 'proposal.anchors',
      message: err instanceof Error ? err.message : 'invalid UTF-8 source text anchors',
    });
  }

  return toValidationResult(issues);
}

/**
 * Validates that a `DraftEarmarkRecord` carries the required preview fields and confirmation
 * token before enabling trader confirmation (distinguishing numeric `0` from missing `undefined`).
 */
export function validateDraftReadyForConfirmation(draft: DraftEarmarkRecord): ValidationResult {
  const issues: ValidationIssue[] = [];
  if (!draft.draftId || !isValidUlid(draft.draftId.value)) {
    issues.push({ field: 'draft_id', message: 'must be a valid ULID' });
  }
  if (draft.draftRevision < 1n) {
    issues.push({ field: 'draft_revision', message: 'must be >= 1' });
  }
  try {
    validateSha256(draft.confirmationToken, 'confirmation_token');
  } catch (err) {
    issues.push({
      field: 'confirmation_token',
      message: err instanceof Error ? err.message : 'invalid confirmation_token',
    });
  }

  if (draft.previewCoverageStatus === CoverageStatus.COVERED_PRE_TAX) {
    if (draft.previewDeltaPreTaxDv01 === undefined) {
      issues.push({
        field: 'preview_delta_pre_tax_dv01',
        message: 'covered draft preview is missing preview_delta_pre_tax_dv01',
      });
    } else {
      checkDecimal(issues, draft.previewDeltaPreTaxDv01, {
        field: 'preview_delta_pre_tax_dv01',
        sign: 'any',
        requireUnit: true,
      });
    }
  } else if (draft.previewCoverageStatus === CoverageStatus.UNCOVERED) {
    if (draft.previewUncoveredReason === UncoveredReason.UNSPECIFIED) {
      issues.push({
        field: 'preview_uncovered_reason',
        message: 'uncovered draft preview must provide preview_uncovered_reason',
      });
    }
    if (draft.previewDeltaUncoveredNotional === undefined) {
      issues.push({
        field: 'preview_delta_uncovered_notional',
        message: 'uncovered draft preview is missing preview_delta_uncovered_notional',
      });
    } else {
      checkDecimal(issues, draft.previewDeltaUncoveredNotional, {
        field: 'preview_delta_uncovered_notional',
        sign: 'any',
        requireUnit: true,
      });
    }
  } else {
    issues.push({
      field: 'preview_coverage_status',
      message: 'preview_coverage_status must be COVERED_PRE_TAX or UNCOVERED',
    });
  }

  return toValidationResult(issues);
}

const VALID_HEDGE_SCOPES: ReadonlySet<PreviewHedgeScope> = new Set([
  PreviewHedgeScope.WHOLE_ACCOUNT,
  PreviewHedgeScope.COVERED_SUBSET_ONLY,
]);

export class PreviewCoveredHedgeValidationError extends Error {
  readonly field: string;

  constructor(field: string, message: string) {
    super(`PreviewCoveredHedge response failed ${field} validation: ${message}`);
    this.name = 'PreviewCoveredHedgeValidationError';
    this.field = field;
  }
}

/**
 * Full `PreviewCoveredHedgeResult` response validator matching
 * `python/sdk/src/ficant_sdk/blotter.py:1059-1170` plus strict request-echo binding
 * (`scope`, `product_code`, `contract_code`, `evaluated_at_seq`).
 */
export function validatePreviewCoveredHedgeResult(
  request: Pick<
    PreviewCoveredHedgeRequest,
    | 'scope'
    | 'productCode'
    | 'contractCode'
    | 'expectedSnapshotSeq'
    | 'conversionFactor'
    | 'ctdDv01Per100'
  >,
  result: PreviewCoveredHedgeResult,
  expectedCurrencyUnit: UnitRef | undefined,
): void {
  if (!VALID_HEDGE_SCOPES.has(request.scope)) {
    throw new PreviewCoveredHedgeValidationError(
      'scope',
      `unsupported request scope=${String(request.scope)}`,
    );
  }
  if (!VALID_HEDGE_SCOPES.has(result.scope)) {
    throw new PreviewCoveredHedgeValidationError(
      'scope',
      `unsupported response scope=${String(result.scope)}`,
    );
  }
  if (result.scope !== request.scope) {
    throw new PreviewCoveredHedgeValidationError(
      'scope',
      `expected ${String(request.scope)}, got ${String(result.scope)}`,
    );
  }

  if (!isNonEmptyTrimmed(result.productCode) || result.productCode !== request.productCode) {
    throw new PreviewCoveredHedgeValidationError(
      'product_code',
      `expected "${request.productCode}", got "${result.productCode}"`,
    );
  }
  if (!isNonEmptyTrimmed(result.contractCode) || result.contractCode !== request.contractCode) {
    throw new PreviewCoveredHedgeValidationError(
      'contract_code',
      `expected "${request.contractCode}", got "${result.contractCode}"`,
    );
  }
  if (
    request.expectedSnapshotSeq !== undefined &&
    result.evaluatedAtSeq !== request.expectedSnapshotSeq
  ) {
    throw new PreviewCoveredHedgeValidationError(
      'evaluated_at_seq',
      `expected ${request.expectedSnapshotSeq.toString()}, got ${result.evaluatedAtSeq.toString()}`,
    );
  }

  if (
    result.uncoveredInstrumentCount > 0 &&
    result.scope === PreviewHedgeScope.WHOLE_ACCOUNT
  ) {
    throw new PreviewCoveredHedgeValidationError(
      'scope',
      `WHOLE_ACCOUNT returned while uncovered_instrument_count=${result.uncoveredInstrumentCount}`,
    );
  }

  if (result.coveredTaxBasis !== BondTaxBasis.PRE_TAX) {
    throw new PreviewCoveredHedgeValidationError(
      'covered_tax_basis',
      `expected BOND_TAX_BASIS_PRE_TAX, got ${String(result.coveredTaxBasis)}`,
    );
  }

  if (!result.measures) {
    throw new PreviewCoveredHedgeValidationError('measures', 'missing measures');
  }

  const notional = result.measures.contractNotional;
  if (!notional) {
    throw new PreviewCoveredHedgeValidationError(
      'contract_notional',
      'missing measures.contract_notional',
    );
  }

  try {
    validateCanonicalDecimalValue(notional, {
      field: 'contract_notional',
      sign: 'positive',
      requireUnit: true,
    });
  } catch (err) {
    throw new PreviewCoveredHedgeValidationError(
      'contract_notional',
      err instanceof Error ? err.message : 'invalid contract_notional',
    );
  }

  if (!notional.unit) {
    throw new PreviewCoveredHedgeValidationError('contract_notional.unit', 'missing unit');
  }
  try {
    validateUnitRef(notional.unit, 'contract_notional.unit');
  } catch (err) {
    throw new PreviewCoveredHedgeValidationError(
      'contract_notional.unit',
      err instanceof Error ? err.message : 'invalid unit',
    );
  }

  // contract_notional MUST be denominated in the account's currency_amount unit.
  if (expectedCurrencyUnit !== undefined && !unitsEqual(notional.unit, expectedCurrencyUnit)) {
    throw new PreviewCoveredHedgeValidationError(
      'contract_notional.unit',
      `expected (${expectedCurrencyUnit.unitId?.value ?? ''}, v${expectedCurrencyUnit.version.toString()}), got (${notional.unit.unitId?.value ?? ''}, v${notional.unit.version.toString()})`,
    );
  }

  // If excluded_uncovered_gross_notional carries a unit, validate and ensure exact currency match.
  if (result.excludedUncoveredGrossNotional?.unit !== undefined) {
    const exclUnit = result.excludedUncoveredGrossNotional.unit;
    try {
      validateUnitRef(exclUnit, 'excluded_uncovered_gross_notional.unit');
    } catch (err) {
      throw new PreviewCoveredHedgeValidationError(
        'excluded_uncovered_gross_notional.unit',
        err instanceof Error ? err.message : 'invalid unit',
      );
    }
    if (!unitsEqual(notional.unit, exclUnit)) {
      throw new PreviewCoveredHedgeValidationError(
        'contract_notional.unit',
        'contract_notional unit does not match excluded_uncovered_gross_notional currency unit',
      );
    }
  }

  // Reject any non-currency unit masquerading as contract_notional.unit.
  const forbiddenUnits: ReadonlyArray<readonly [string, DecimalValue | undefined]> = [
    ['conversion_factor', request.conversionFactor],
    ['ctd_dv01_per_100', request.ctdDv01Per100],
    ['hedge_effectiveness', result.measures.hedgeEffectiveness],
    ['futures_contract_dv01', result.measures.futuresContractDv01],
    ['raw_contracts', result.measures.rawContracts],
  ];

  for (const [label, forbiddenDec] of forbiddenUnits) {
    if (forbiddenDec?.unit !== undefined && unitsEqual(notional.unit, forbiddenDec.unit)) {
      throw new PreviewCoveredHedgeValidationError(
        'contract_notional.unit',
        `contract_notional carries non-currency ${label} unit`,
      );
    }
  }
}
