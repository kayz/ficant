import { create } from '@bufbuild/protobuf';
import { TimestampSchema } from '@bufbuild/protobuf/wkt';
import { describe, expect, it, vi } from 'vitest';
import { TRUSTED_RULEPACK_ANCHOR } from 'virtual:ficant-trusted-rulepack';
import {
  BaseEvidenceSchema,
  BaselineDeltaSchema,
  CoverageStatus,
  DraftEarmarkRecordSchema,
  DraftTargetState,
  PendingMatchResolutionAction,
  PreviewHedgeScope,
  QuoteTermsSchema,
  SettlementBucket,
  SourceTextAnchorSchema,
  TradeSide,
  UncoveredReason,
} from '../generated/ficant/blotter/v1/model_pb';
import {
  PreviewCoveredHedgeRequestSchema,
  PreviewCoveredHedgeResultSchema,
} from '../generated/ficant/blotter/v1/service_pb';
import { NlpEarmarkProposalSchema } from '../generated/ficant/blotter/v1/spi_pb';
import {
  DecimalValueSchema,
  MarketTimeSchema,
  Sha256Schema,
  UlidSchema,
} from '../generated/ficant/core/v1/common_pb';
import {
  AnalysisUnitsSchema,
  BondTaxBasis,
  FuturesHedgeMeasuresSchema,
} from '../generated/ficant/rates/v1/analytics_pb';
import { VerificationStatus } from '../generated/ficant/market/v1/rule_pb';
import {
  I128_MAX,
  I128_MIN,
  PINNED_RULEPACK_MANIFEST_SHA256,
  PINNED_RULEPACK_PAYLOAD_BYTES,
  PINNED_RULEPACK_PAYLOAD_SHA256,
  PINNED_RULEPACK_TYPE_URL,
  PrimitiveValidationError,
  RulePackVerificationError,
  UINT64_MAX,
  buildHighlightedSegments,
  compareDecimalValues,
  createDecimalValue,
  createMarketTimeFromIso,
  createUlid,
  createUnitRef,
  decimalToPlotNumber,
  formatDecimalValue,
  isZeroDecimal,
  loadTrustedDeliveryRulePack,
  normalizeDecimalParts,
  parseDecimalString,
  parseUint64,
  sha256Hex,
  sha256PureHex,
  sliceUtf8HalfOpen,
  unitsEqual,
  utf8ByteLength,
  validateCancelCommittedRequest,
  validateCanonicalDecimalValue,
  validateConfirmHoldRequest,
  validateCreateCommittedRequest,
  validateCreateHoldRequest,
  validateDraftReadyForConfirmation,
  validateExtendHoldRequest,
  validateIsoDate,
  validateMarketTime,
  validateNlpEarmarkProposal,
  validatePreviewCoveredHedgeResult,
  validateQuoteTerms,
  validateReleaseHoldRequest,
  validateResolvePendingMatchRequest,
  validateSetFeedControlRequest,
  validateSourceTextAnchors,
  validateUnitRef,
} from './index';

const ULID_A = '01JNB0G1000000000000000001';
const ULID_B = '01JNB0G1000000000000000002';
const ULID_C = '01JNB0G1000000000000000003';
const ULID_D = '01JNB0G1000000000000000004';
const ULID_E = '01JNB0G1000000000000000005';
const ULID_F = '01JNB0G1000000000000000006';
const ULID_G = '01JNB0G1000000000000000007';
const ULID_EARMARK = '01JNB0G1000000000000000008';
const ULID_PENDING = '01JNB0G1000000000000000009';
const ULID_DRAFT = '01JNB0G1000000000000000010';

function makeAnalysisUnits() {
  return create(AnalysisUnitsSchema, {
    currencyAmount: createUnitRef(ULID_A, 1n),
    pricePer100: createUnitRef(ULID_B, 1n),
    rate: createUnitRef(ULID_C, 1n),
    dv01: createUnitRef(ULID_D, 1n),
    dv01Per100: createUnitRef(ULID_E, 1n),
    dimensionless: createUnitRef(ULID_F, 1n),
    contractCount: createUnitRef(ULID_G, 1n),
  });
}

describe('SHA-256 pure-TS & WebCrypto fallback', () => {
  const enc = new TextEncoder();

  it('matches FIPS 180-4 standard test vectors with SubtleCrypto disabled', async () => {
    expect(sha256PureHex(enc.encode(''))).toBe(
      'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
    );
    expect(sha256PureHex(enc.encode('abc'))).toBe(
      'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
    );
    expect(
      sha256PureHex(enc.encode('abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq')),
    ).toBe('248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1');

    // Async wrapper with subtleOverride=null uses pure-TS fallback
    expect(await sha256Hex(enc.encode('abc'), null)).toBe(
      'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
    );
    // Async wrapper with default WebCrypto matches pure-TS identically
    expect(await sha256Hex(enc.encode('abc'))).toBe(sha256PureHex(enc.encode('abc')));
  });
});

describe('Canonical DecimalValue, i128 bounds, and presence vs zero', () => {
  const ccy = createUnitRef(ULID_A, 1n);

  it('parses and normalizes canonical decimals and distinguishes zero from missing', () => {
    expect(parseDecimalString('0012.3400')).toEqual({
      coefficient: '1234',
      scale: 2,
      scaledI128: 12340000000000n,
    });
    expect(parseDecimalString('0.0000')).toEqual({
      coefficient: '0',
      scale: 0,
      scaledI128: 0n,
    });
    expect(parseDecimalString('-0.0050')).toEqual({
      coefficient: '-5',
      scale: 3,
      scaledI128: -5000000000n,
    });

    const zeroVal = createDecimalValue('0', ccy);
    expect(isZeroDecimal(zeroVal)).toBe(true);
    expect(isZeroDecimal(undefined)).toBe(false);
    expect(formatDecimalValue(undefined)).toBe('未提供');
    expect(formatDecimalValue(zeroVal)).toBe('0');
    expect(formatDecimalValue(createDecimalValue('1234567.89', ccy), { grouping: true })).toBe(
      '1,234,567.89',
    );
    expect(decimalToPlotNumber(undefined)).toBeNull();
    expect(decimalToPlotNumber(zeroVal)).toBe(0);
    expect(compareDecimalValues(createDecimalValue('1.5', ccy), createDecimalValue('2', ccy))).toBe(
      -1,
    );
  });

  it('rejects -0, leading +, invalid chars, precision > 28, scale > 12, and non-canonical wire forms', () => {
    for (const bad of ['', '-0', '-0.00', '+1', '+0', ' 1', '1 ', '1e5', '12.3.4', '.5', '5.', 'abc']) {
      expect(() => parseDecimalString(bad)).toThrow();
    }
    expect(() => parseDecimalString('12345678901234567890123456789')).toThrow(/precision/i);
    expect(() => parseDecimalString('0.0000000000001')).toThrow(/scale/i);
    expect(() => normalizeDecimalParts('-0', 0)).toThrow(/negative zero/i);
    expect(() => normalizeDecimalParts('+10', 0)).toThrow(/\+/);

    // Wire validation rejects non-canonical leading/trailing zeros or zero with non-zero scale
    expect(() =>
      validateCanonicalDecimalValue(
        create(DecimalValueSchema, { coefficient: '01', scale: 0, unit: ccy }),
      ),
    ).toThrow();
    expect(() =>
      validateCanonicalDecimalValue(
        create(DecimalValueSchema, { coefficient: '120', scale: 1, unit: ccy }),
      ),
    ).toThrow(/trailing zero/i);
    expect(() =>
      validateCanonicalDecimalValue(
        create(DecimalValueSchema, { coefficient: '0', scale: 2, unit: ccy }),
      ),
    ).toThrow(/scale 0/i);
  });

  it('enforces exact signed i128 bounds after scaling by 10^(12 - scale)', () => {
    // 27 digits at scale=0: 170141183460469231731687303 * 10^12 <= I128_MAX
    const maxScale0Ok = parseDecimalString('170141183460469231731687303');
    expect(maxScale0Ok.scaledI128 <= I128_MAX).toBe(true);
    expect(() => parseDecimalString('170141183460469231731687304')).toThrow(/i128/i);

    const minScale0Ok = parseDecimalString('-170141183460469231731687303');
    expect(minScale0Ok.scaledI128 >= I128_MIN).toBe(true);
    expect(() => parseDecimalString('-170141183460469231731687304')).toThrow(/i128/i);

    // 28 digits at scale=1: 170141183460469231731687303.7 * 10^12 fits, .8 overflows
    const maxScale1Ok = parseDecimalString('170141183460469231731687303.7');
    expect(maxScale1Ok.scaledI128 <= I128_MAX).toBe(true);
    expect(() => parseDecimalString('170141183460469231731687303.8')).toThrow(/i128/i);
    expect(() => parseDecimalString('-170141183460469231731687303.8')).toThrow(/i128/i);
  });
});

describe('UnitRef, uint64 bigint, ISO date, and MarketTime primitives', () => {
  it('compares UnitRef by both unit_id and version, and enforces uint64/date/MarketTime bounds', () => {
    const u1 = createUnitRef(ULID_A, 1n);
    const u1Same = createUnitRef(ULID_A, 1n);
    const u2Ver = createUnitRef(ULID_A, 2n);
    expect(unitsEqual(u1, u1Same)).toBe(true);
    expect(unitsEqual(u1, u2Ver)).toBe(false);
    expect(() => validateUnitRef(create(DecimalValueSchema, {}).unit)).toThrow();
    expect(() => createUnitRef(ULID_A, 0n)).toThrow();

    expect(parseUint64(0n)).toBe(0n);
    expect(parseUint64(UINT64_MAX)).toBe(UINT64_MAX);
    expect(() => parseUint64(-1n)).toThrow();
    expect(() => parseUint64(UINT64_MAX + 1n)).toThrow();
    expect(() => parseUint64(1.5)).toThrow();

    expect(validateIsoDate('2026-06-15')).toBe('2026-06-15');
    expect(() => validateIsoDate('2026-02-30')).toThrow(/invalid calendar date/i);

    const mt = createMarketTimeFromIso('2026-06-15T02:00:00Z', 'Asia/Shanghai', '2026-06-15');
    expect(validateMarketTime(mt).localTradingDate).toBe('2026-06-15');
    expect(() =>
      createMarketTimeFromIso('2026-06-15T02:00:00Z', 'Asia/Shanghai', '2026-06-14'),
    ).toThrow();
  });
});

describe('QuoteTerms, TTL boundaries (1799/1800/2700/2701), reasons, and Draft proposal validations', () => {
  const units = makeAnalysisUnits();

  it('accepts positive settlement_amount alone and rejects empty/zero/negative quotes', () => {
    const settlementOnly = create(QuoteTermsSchema, {
      settlementAmount: createDecimalValue('10123456.78', units.currencyAmount!),
    });
    expect(validateQuoteTerms(settlementOnly, { required: true, units }).ok).toBe(true);

    const cleanPrice = create(QuoteTermsSchema, {
      priceOrYield: {
        case: 'cleanPricePer100',
        value: createDecimalValue('100.25', units.pricePer100!),
      },
    });
    expect(validateQuoteTerms(cleanPrice, { required: true, units }).ok).toBe(true);

    const zeroYield = create(QuoteTermsSchema, {
      priceOrYield: {
        case: 'yieldToMaturity',
        value: createDecimalValue('0', units.rate!),
      },
    });
    expect(validateQuoteTerms(zeroYield, { required: true, units }).ok).toBe(true);

    // Reject empty quote
    expect(validateQuoteTerms(create(QuoteTermsSchema, {}), { required: true, units }).ok).toBe(
      false,
    );
    // Reject zero clean price
    expect(
      validateQuoteTerms(
        create(QuoteTermsSchema, {
          priceOrYield: {
            case: 'cleanPricePer100',
            value: createDecimalValue('0', units.pricePer100!),
          },
        }),
        { required: true, units },
      ).ok,
    ).toBe(false);
    // Reject negative yield
    expect(
      validateQuoteTerms(
        create(QuoteTermsSchema, {
          priceOrYield: {
            case: 'yieldToMaturity',
            value: createDecimalValue('-0.001', units.rate!),
          },
        }),
        { required: true, units },
      ).ok,
    ).toBe(false);
    // Reject zero or negative settlement_amount
    expect(
      validateQuoteTerms(
        create(QuoteTermsSchema, {
          settlementAmount: createDecimalValue('0', units.currencyAmount!),
        }),
        { required: true, units },
      ).ok,
    ).toBe(false);
    expect(
      validateQuoteTerms(
        create(QuoteTermsSchema, {
          settlementAmount: createDecimalValue('-100', units.currencyAmount!),
        }),
        { required: true, units },
      ).ok,
    ).toBe(false);
  });

  it('enforces CreateHold and ExtendHold integer TTL strictly in 1800..=2700', () => {
    const baseHold = {
      accountId: 'acct-a',
      instrumentCode: '240011.IB',
      side: TradeSide.BUY,
      settlementBucket: SettlementBucket.T_PLUS_0,
      notional: createDecimalValue('10000000', units.currencyAmount!),
      quoteTerms: undefined,
      expectedPositionVersion: 1n,
    };

    expect(validateCreateHoldRequest({ ...baseHold, ttlSeconds: 1799 }, units).ok).toBe(false);
    expect(validateCreateHoldRequest({ ...baseHold, ttlSeconds: 1800 }, units).ok).toBe(true);
    expect(validateCreateHoldRequest({ ...baseHold, ttlSeconds: 2700 }, units).ok).toBe(true);
    expect(validateCreateHoldRequest({ ...baseHold, ttlSeconds: 2701 }, units).ok).toBe(false);
    expect(validateCreateHoldRequest({ ...baseHold, ttlSeconds: 1800.5 }, units).ok).toBe(false);

    const baseExtend = {
      accountId: 'acct-a',
      earmarkId: createUlid(ULID_EARMARK),
      expectedEarmarkRevision: 1n,
    };
    expect(validateExtendHoldRequest({ ...baseExtend, ttlSeconds: 1799 }).ok).toBe(false);
    expect(validateExtendHoldRequest({ ...baseExtend, ttlSeconds: 1800 }).ok).toBe(true);
    expect(validateExtendHoldRequest({ ...baseExtend, ttlSeconds: 2700 }).ok).toBe(true);
    expect(validateExtendHoldRequest({ ...baseExtend, ttlSeconds: 2701 }).ok).toBe(false);
  });

  it('enforces required quote on ConfirmHold/CreateCommitted and non-empty reason on Release/Cancel/Feed/Reject', () => {
    const settlementQuote = create(QuoteTermsSchema, {
      settlementAmount: createDecimalValue('5000000', units.currencyAmount!),
    });

    expect(
      validateConfirmHoldRequest(
        {
          accountId: 'acct-a',
          earmarkId: createUlid(ULID_EARMARK),
          expectedEarmarkRevision: 1n,
          expectedPositionVersion: 1n,
          quoteTerms: undefined,
        },
        units,
      ).ok,
    ).toBe(false);
    expect(
      validateConfirmHoldRequest(
        {
          accountId: 'acct-a',
          earmarkId: createUlid(ULID_EARMARK),
          expectedEarmarkRevision: 1n,
          expectedPositionVersion: 1n,
          quoteTerms: settlementQuote,
        },
        units,
      ).ok,
    ).toBe(true);

    expect(
      validateCreateCommittedRequest(
        {
          accountId: 'acct-a',
          instrumentCode: '240011.IB',
          side: TradeSide.BUY,
          settlementBucket: SettlementBucket.T_PLUS_0,
          notional: createDecimalValue('5000000', units.currencyAmount!),
          quoteTerms: undefined,
          expectedPositionVersion: 1n,
        },
        units,
      ).ok,
    ).toBe(false);

    expect(
      validateReleaseHoldRequest({
        accountId: 'acct-a',
        earmarkId: createUlid(ULID_EARMARK),
        expectedEarmarkRevision: 1n,
        reason: '   ',
      }).ok,
    ).toBe(false);
    expect(
      validateCancelCommittedRequest({
        accountId: 'acct-a',
        earmarkId: createUlid(ULID_EARMARK),
        expectedEarmarkRevision: 2n,
        reason: '',
      }).ok,
    ).toBe(false);
    expect(
      validateSetFeedControlRequest({
        sourceId: 'src-1',
        expectedFeedStatusRevision: 1n,
        pausedByOperator: false,
        forceClearGapPause: true,
        reason: '',
      }).ok,
    ).toBe(false);
    expect(
      validateSetFeedControlRequest({
        sourceId: 'src-1',
        expectedFeedStatusRevision: 1n,
        pausedByOperator: false,
        forceClearGapPause: true,
        reason: 'verified feed continuity with ops',
      }).ok,
    ).toBe(true);

    // ResolvePendingMatch RELEASE_REJECT requires non-empty reason and omits target/offset/baseline
    expect(
      validateResolvePendingMatchRequest(
        {
          accountId: 'acct-a',
          pendingMatchId: createUlid(ULID_PENDING),
          expectedPendingRevision: 1n,
          expectedPositionVersion: 1n,
          action: PendingMatchResolutionAction.RELEASE_REJECT,
          reason: '',
        },
        units,
      ).ok,
    ).toBe(false);

    // APPLY_BASELINE_DELTA enforces same bucket between delta and BaseEvidence
    const ev = create(BaseEvidenceSchema, {
      sourceId: 'src-1',
      externalTradeId: 'ext-1',
      originalFeedSeq: 1n,
      originalExecutedAt: createMarketTimeFromIso(
        '2026-06-15T01:00:00Z',
        'Asia/Shanghai',
        '2026-06-15',
      ),
      oldNotional: createDecimalValue('1000000', units.currencyAmount!),
      oldSignedCash: createDecimalValue('-1000000', units.currencyAmount!),
      settlementBucket: SettlementBucket.T_PLUS_0,
      evidenceRef: 'ev-ref-1',
      evidenceHash: create(Sha256Schema, { value: new Uint8Array(32).fill(1) }),
      instrumentCode: '240011.IB',
      side: TradeSide.BUY,
      originalRevision: 1n,
    });
    expect(
      validateResolvePendingMatchRequest(
        {
          accountId: 'acct-a',
          pendingMatchId: createUlid(ULID_PENDING),
          expectedPendingRevision: 1n,
          expectedPositionVersion: 1n,
          action: PendingMatchResolutionAction.APPLY_BASELINE_DELTA,
          reason: '',
          baselineDelta: create(BaselineDeltaSchema, {
            deltaNotional: createDecimalValue('500000', units.currencyAmount!),
            deltaCash: createDecimalValue('-500000', units.currencyAmount!),
            settlementBucket: SettlementBucket.T_PLUS_1, // mismatch vs evidence T_PLUS_0
            evidence: ev,
            reason: 'manual baseline revision',
          }),
        },
        units,
      ).ok,
    ).toBe(false);
  });

  it('validates Draft confidence_score [0,1] + dimensionless unit and Draft confirmation preview presence vs zero', () => {
    const rawText = '买入 240011.IB 5000万 净价100.50';
    const buyEnd = utf8ByteLength('买入');
    const validProposal = create(NlpEarmarkProposalSchema, {
      parserSourceId: 'nlp-1',
      modelVersion: 'v1',
      rawSourceText: rawText,
      anchors: [
        create(SourceTextAnchorSchema, {
          fieldName: 'side',
          byteStart: 0,
          byteEnd: buyEnd,
          spanText: '买入',
        }),
      ],
      confidenceScore: createDecimalValue('0.95', units.dimensionless!),
      accountId: 'acct-a',
      instrumentCode: '240011.IB',
      side: TradeSide.BUY,
      settlementBucket: SettlementBucket.T_PLUS_0,
      notional: createDecimalValue('50000000', units.currencyAmount!),
      quoteTerms: create(QuoteTermsSchema, {
        priceOrYield: {
          case: 'cleanPricePer100',
          value: createDecimalValue('100.5', units.pricePer100!),
        },
      }),
      suggestedTargetState: DraftTargetState.HOLD,
    });

    expect(validateNlpEarmarkProposal(validProposal, units, 'acct-a').ok).toBe(true);

    // Wrong confidence_score unit (using rate instead of dimensionless)
    const wrongUnit = create(NlpEarmarkProposalSchema, {
      ...validProposal,
      confidenceScore: createDecimalValue('0.95', units.rate!),
    });
    expect(validateNlpEarmarkProposal(wrongUnit, units, 'acct-a').ok).toBe(false);

    // Out-of-range confidence_score (> 1)
    const overOne = create(NlpEarmarkProposalSchema, {
      ...validProposal,
      confidenceScore: createDecimalValue('1.01', units.dimensionless!),
    });
    expect(validateNlpEarmarkProposal(overOne, units, 'acct-a').ok).toBe(false);

    // Draft confirmation: zero preview_delta_pre_tax_dv01 is VALID, undefined is INVALID
    const zeroDv01Draft = create(DraftEarmarkRecordSchema, {
      draftId: createUlid(ULID_DRAFT),
      draftRevision: 1n,
      confirmationToken: create(Sha256Schema, { value: new Uint8Array(32).fill(7) }),
      previewCoverageStatus: CoverageStatus.COVERED_PRE_TAX,
      previewDeltaPreTaxDv01: createDecimalValue('0', units.dv01!),
    });
    expect(validateDraftReadyForConfirmation(zeroDv01Draft).ok).toBe(true);

    const missingDv01Draft = create(DraftEarmarkRecordSchema, {
      draftId: createUlid(ULID_DRAFT),
      draftRevision: 1n,
      confirmationToken: create(Sha256Schema, { value: new Uint8Array(32).fill(7) }),
      previewCoverageStatus: CoverageStatus.COVERED_PRE_TAX,
    });
    expect(validateDraftReadyForConfirmation(missingDv01Draft).ok).toBe(false);

    const uncoveredMissingReason = create(DraftEarmarkRecordSchema, {
      draftId: createUlid(ULID_DRAFT),
      draftRevision: 1n,
      confirmationToken: create(Sha256Schema, { value: new Uint8Array(32).fill(7) }),
      previewCoverageStatus: CoverageStatus.UNCOVERED,
      previewUncoveredReason: UncoveredReason.UNSPECIFIED,
      previewDeltaUncoveredNotional: createDecimalValue('1000000', units.currencyAmount!),
    });
    expect(validateDraftReadyForConfirmation(uncoveredMissingReason).ok).toBe(false);
  });
});

describe('UTF-8 half-open SourceTextAnchor validation and highlighting with Chinese text', () => {
  it('validates multi-byte Chinese byte offsets, builds highlight segments, and rejects mid-character splits', () => {
    const text = '买入国债240011，面额5000万，净价101.25';
    const buyBytes = utf8ByteLength('买入'); // 6 bytes
    const prefixBeforeFace = utf8ByteLength('买入国债240011，');
    const faceBytes = utf8ByteLength('面额5000万');

    expect(sliceUtf8HalfOpen(text, 0, buyBytes)).toBe('买入');
    expect(sliceUtf8HalfOpen(text, prefixBeforeFace, prefixBeforeFace + faceBytes)).toBe(
      '面额5000万',
    );

    const anchors = [
      create(SourceTextAnchorSchema, {
        fieldName: 'side',
        byteStart: 0,
        byteEnd: buyBytes,
        spanText: '买入',
      }),
      create(SourceTextAnchorSchema, {
        fieldName: 'notional',
        byteStart: prefixBeforeFace,
        byteEnd: prefixBeforeFace + faceBytes,
        spanText: '面额5000万',
      }),
    ];

    const spans = validateSourceTextAnchors(text, anchors);
    expect(spans).toHaveLength(2);

    const segments = buildHighlightedSegments(text, anchors);
    expect(segments.map((s) => s.text).join('')).toBe(text);
    expect(segments.filter((s) => s.highlighted).map((s) => s.text)).toEqual([
      '买入',
      '面额5000万',
    ]);

    // Mid-character byte boundary (byte 1 inside 3-byte Chinese char "买") MUST be rejected!
    const brokenMidChar = [
      create(SourceTextAnchorSchema, {
        fieldName: 'side',
        byteStart: 1,
        byteEnd: buyBytes,
        spanText: '入',
      }),
    ];
    expect(() => validateSourceTextAnchors(text, brokenMidChar)).toThrow(/character boundaries/i);

    // Mismatched span_text MUST be rejected
    const wrongSpan = [
      create(SourceTextAnchorSchema, {
        fieldName: 'side',
        byteStart: 0,
        byteEnd: buyBytes,
        spanText: '卖出',
      }),
    ];
    expect(() => validateSourceTextAnchors(text, wrongSpan)).toThrow(/span_text mismatch/i);
  });
});

describe('Trusted RulePack runtime verification (independent trust anchor & half-open window)', () => {
  const validValuationAt = new Date('2026-06-15T02:00:00Z');

  it('loads and verifies payload via pure-TS SHA-256 fallback and rejects tampered/expired/swapped packs with 0 RPCs', async () => {
    const fetchMock = vi.fn<typeof fetch>();

    // 1. Missing trust anchor fails before fetch (0 calls)
    await expect(
      loadTrustedDeliveryRulePack({
        valuationAt: validValuationAt,
        expectedManifestSha256: '',
        fetchImpl: fetchMock,
      }),
    ).rejects.toThrow(RulePackVerificationError);
    expect(fetchMock).not.toHaveBeenCalled();

    // 2. Missing build anchor fails before fetch
    await expect(
      loadTrustedDeliveryRulePack({
        valuationAt: validValuationAt,
        anchor: null,
        fetchImpl: fetchMock,
      }),
    ).rejects.toThrow(/anchor is missing/i);
    expect(fetchMock).not.toHaveBeenCalled();

    // 3. Joint manifest + payload replacement with self-recomputed digest fails against pinned anchor
    const fakePayload = new Uint8Array(PINNED_RULEPACK_PAYLOAD_BYTES).fill(0xab);
    const fakePayloadSha = sha256PureHex(fakePayload);
    const swappedManifest = {
      ...TRUSTED_RULEPACK_ANCHOR.manifest,
      payload_sha256: fakePayloadSha,
    };
    const swappedManifestBytes = new TextEncoder().encode(
      `${JSON.stringify(swappedManifest, null, 2)}\n`,
    );
    const swappedManifestSha = sha256PureHex(swappedManifestBytes);

    await expect(
      loadTrustedDeliveryRulePack({
        valuationAt: validValuationAt,
        anchor: {
          ...TRUSTED_RULEPACK_ANCHOR,
          manifestSha256: swappedManifestSha,
          payloadSha256: fakePayloadSha,
          manifest: swappedManifest,
        },
        rawManifestBytes: swappedManifestBytes,
        payloadBytes: fakePayload,
        subtleOverride: null,
      }),
    ).rejects.toThrow(/SHA-256 mismatch/i);

    // 4. Extending effective_to alone fails against pinned manifest SHA-256
    const extendedManifest = {
      ...TRUSTED_RULEPACK_ANCHOR.manifest,
      effective_to: {
        instant: '2040-12-31T15:59:59Z',
        market_timezone: 'Asia/Shanghai',
        local_trading_date: '2040-12-31',
      },
    };
    await expect(
      loadTrustedDeliveryRulePack({
        valuationAt: validValuationAt,
        anchor: {
          ...TRUSTED_RULEPACK_ANCHOR,
          manifest: extendedManifest,
        },
        subtleOverride: null,
      }),
    ).rejects.toThrow(/SHA-256 mismatch/i);

    // 5. Half-open validity boundary: valuationAt === effective_to MUST be rejected
    await expect(
      loadTrustedDeliveryRulePack({
        valuationAt: new Date(TRUSTED_RULEPACK_ANCHOR.manifest.effective_to.instant),
        fetchImpl: fetchMock,
        subtleOverride: null,
      }),
    ).rejects.toThrow(/half-open/i);
    expect(fetchMock).not.toHaveBeenCalled();

    // 6. Tampered binary payload fails SHA-256 check
    fetchMock.mockResolvedValueOnce(
      new Response(fakePayload, {
        status: 200,
        headers: { 'content-type': 'application/octet-stream' },
      }),
    );
    await expect(
      loadTrustedDeliveryRulePack({
        valuationAt: validValuationAt,
        fetchImpl: fetchMock,
        subtleOverride: null,
      }),
    ).rejects.toThrow(/payload SHA-256 mismatch/i);

    // 7. Wrong type_url fails even if manifest hash were hypothetically re-pinned
    const wrongTypeManifest = {
      ...TRUSTED_RULEPACK_ANCHOR.manifest,
      type_url: 'type.googleapis.com/invalid.WrongRulePack',
    };
    const wrongTypeBytes = new TextEncoder().encode(
      `${JSON.stringify(wrongTypeManifest, null, 2)}\n`,
    );
    const wrongTypeSha = sha256PureHex(wrongTypeBytes);
    await expect(
      loadTrustedDeliveryRulePack({
        valuationAt: validValuationAt,
        expectedManifestSha256: wrongTypeSha,
        anchor: {
          ...TRUSTED_RULEPACK_ANCHOR,
          manifestSha256: wrongTypeSha,
          manifest: wrongTypeManifest,
        },
        subtleOverride: null,
      }),
    ).rejects.toThrow(/type_url mismatch/i);

    // 8. Valid synthetic test verifying complete MarketRulePack construction with subtleOverride=null
    const syntheticPayload = new Uint8Array(PINNED_RULEPACK_PAYLOAD_BYTES).fill(0x55);
    const syntheticPayloadSha = sha256PureHex(syntheticPayload);
    const syntheticManifest = {
      ...TRUSTED_RULEPACK_ANCHOR.manifest,
      payload_sha256: syntheticPayloadSha,
    };
    const syntheticManifestBytes = new TextEncoder().encode(
      `${JSON.stringify(syntheticManifest, null, 2)}\n`,
    );
    const syntheticManifestSha = sha256PureHex(syntheticManifestBytes);

    fetchMock.mockResolvedValueOnce(
      new Response(syntheticPayload, {
        status: 200,
        headers: { 'content-type': 'application/octet-stream' },
      }),
    );

    const verified = await loadTrustedDeliveryRulePack({
      valuationAt: validValuationAt,
      expectedManifestSha256: syntheticManifestSha,
      expectedPayloadSha256: syntheticPayloadSha,
      anchor: {
        ...TRUSTED_RULEPACK_ANCHOR,
        manifestSha256: syntheticManifestSha,
        payloadSha256: syntheticPayloadSha,
        manifest: syntheticManifest,
      },
      fetchImpl: fetchMock,
      subtleOverride: null,
    });
    expect(verified.rulePack.verificationStatus).toBe(VerificationStatus.VERIFIED);
    expect(verified.rulePack.content?.typeUrl).toBe(PINNED_RULEPACK_TYPE_URL);
    expect(verified.rulePack.content?.value.byteLength).toBe(PINNED_RULEPACK_PAYLOAD_BYTES);
    expect(verified.rulePack.contentHash?.value.byteLength).toBe(32);
    expect(TRUSTED_RULEPACK_ANCHOR.manifestSha256).toBe(PINNED_RULEPACK_MANIFEST_SHA256);
    expect(TRUSTED_RULEPACK_ANCHOR.payloadSha256).toBe(PINNED_RULEPACK_PAYLOAD_SHA256);
  });

  it('rejects malformed MarketTime valuationAt invariants before fetching payload (M1-F04)', async () => {
    const fetchMock = vi.fn<typeof fetch>();
    const validInstant = create(TimestampSchema, {
      seconds: BigInt(Math.floor(validValuationAt.getTime() / 1000)),
      nanos: 0,
    });

    const malformedCases = [
      // Mismatched local_trading_date vs instant in Asia/Shanghai
      create(MarketTimeSchema, {
        instant: validInstant,
        marketTimezone: 'Asia/Shanghai',
        localTradingDate: '2026-06-14',
      }),
      // Invalid IANA market_timezone
      create(MarketTimeSchema, {
        instant: validInstant,
        marketTimezone: 'Mars/Olympus',
        localTradingDate: '2026-06-15',
      }),
      // Out-of-range nanos
      create(MarketTimeSchema, {
        instant: create(TimestampSchema, {
          seconds: validInstant.seconds,
          nanos: 1_000_000_000,
        }),
        marketTimezone: 'Asia/Shanghai',
        localTradingDate: '2026-06-15',
      }),
      // Missing instant
      create(MarketTimeSchema, {
        marketTimezone: 'Asia/Shanghai',
        localTradingDate: '2026-06-15',
      }),
    ];

    for (const badMarketTime of malformedCases) {
      await expect(
        loadTrustedDeliveryRulePack({
          valuationAt: badMarketTime,
          fetchImpl: fetchMock,
          subtleOverride: null,
        }),
      ).rejects.toThrow(PrimitiveValidationError);
    }
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe('validatePreviewCoveredHedgeResult parity with blotter.py:1059-1170', () => {
  const units = makeAnalysisUnits();

  function makeValidReqAndRes() {
    const req = create(PreviewCoveredHedgeRequestSchema, {
      accountId: 'acct-a',
      scope: PreviewHedgeScope.COVERED_SUBSET_ONLY,
      productCode: 'PROD1',
      contractCode: 'PROD1-2609',
      ctdDv01Per100: createDecimalValue('0.085', units.dv01Per100!),
      conversionFactor: createDecimalValue('1.0123', units.dimensionless!),
      expectedSnapshotSeq: 42n,
    });
    const res = create(PreviewCoveredHedgeResultSchema, {
      evaluatedAtSeq: 42n,
      scope: PreviewHedgeScope.COVERED_SUBSET_ONLY,
      productCode: 'PROD1',
      contractCode: 'PROD1-2609',
      zeroNetExposure: false,
      coveredTaxBasis: BondTaxBasis.PRE_TAX,
      coveredInstrumentCount: 2,
      uncoveredInstrumentCount: 1,
      coveredNetPreTaxDv01: createDecimalValue('5000', units.dv01!),
      excludedUncoveredGrossNotional: createDecimalValue('10000000', units.currencyAmount!),
      measures: create(FuturesHedgeMeasuresSchema, {
        futuresContractDv01: createDecimalValue('800', units.dv01!),
        rawContracts: createDecimalValue('6.25', units.contractCount!),
        recommendedContracts: 6n,
        residualDv01: createDecimalValue('200', units.dv01!),
        hedgeEffectiveness: createDecimalValue('0.96', units.dimensionless!),
        contractNotional: createDecimalValue('1000000', units.currencyAmount!),
      }),
    });
    return { req, res };
  }

  it('accepts valid covered hedge preview including zero net exposure', () => {
    const { req, res } = makeValidReqAndRes();
    expect(() => validatePreviewCoveredHedgeResult(req, res, units.currencyAmount)).not.toThrow();

    const zeroNetRes = create(PreviewCoveredHedgeResultSchema, {
      ...res,
      zeroNetExposure: true,
      coveredNetPreTaxDv01: createDecimalValue('0', units.dv01!),
      measures: create(FuturesHedgeMeasuresSchema, {
        ...res.measures!,
        rawContracts: createDecimalValue('0', units.contractCount!),
        recommendedContracts: 0n,
      }),
    });
    expect(() =>
      validatePreviewCoveredHedgeResult(req, zeroNetRes, units.currencyAmount),
    ).not.toThrow();
  });

  it('rejects scope, product/contract/seq mismatch, uncovered WHOLE_ACCOUNT, tax basis, bad contract_notional, and non-currency units', () => {
    const { req, res } = makeValidReqAndRes();

    // evaluated_at_seq mismatch
    expect(() =>
      validatePreviewCoveredHedgeResult(
        req,
        create(PreviewCoveredHedgeResultSchema, { ...res, evaluatedAtSeq: 41n }),
        units.currencyAmount,
      ),
    ).toThrow(/evaluated_at_seq/i);

    // product_code mismatch
    expect(() =>
      validatePreviewCoveredHedgeResult(
        req,
        create(PreviewCoveredHedgeResultSchema, { ...res, productCode: 'OTHER' }),
        units.currencyAmount,
      ),
    ).toThrow(/product_code/i);

    // contract_code mismatch
    expect(() =>
      validatePreviewCoveredHedgeResult(
        req,
        create(PreviewCoveredHedgeResultSchema, { ...res, contractCode: 'OTHER-2612' }),
        units.currencyAmount,
      ),
    ).toThrow(/contract_code/i);

    // WHOLE_ACCOUNT with uncovered_instrument_count > 0
    const wholeReq = create(PreviewCoveredHedgeRequestSchema, {
      ...req,
      scope: PreviewHedgeScope.WHOLE_ACCOUNT,
    });
    const wholeUncoveredRes = create(PreviewCoveredHedgeResultSchema, {
      ...res,
      scope: PreviewHedgeScope.WHOLE_ACCOUNT,
      uncoveredInstrumentCount: 1,
    });
    expect(() =>
      validatePreviewCoveredHedgeResult(wholeReq, wholeUncoveredRes, units.currencyAmount),
    ).toThrow(/WHOLE_ACCOUNT/i);

    // Non-PRE_TAX covered_tax_basis
    expect(() =>
      validatePreviewCoveredHedgeResult(
        req,
        create(PreviewCoveredHedgeResultSchema, {
          ...res,
          coveredTaxBasis: BondTaxBasis.TAX_ADJUSTED,
        }),
        units.currencyAmount,
      ),
    ).toThrow(/covered_tax_basis/i);

    // Missing measures or contract_notional
    expect(() =>
      validatePreviewCoveredHedgeResult(
        req,
        create(PreviewCoveredHedgeResultSchema, { ...res, measures: undefined }),
        units.currencyAmount,
      ),
    ).toThrow(/missing measures/i);
    expect(() =>
      validatePreviewCoveredHedgeResult(
        req,
        create(PreviewCoveredHedgeResultSchema, {
          ...res,
          measures: create(FuturesHedgeMeasuresSchema, {
            ...res.measures!,
            contractNotional: undefined,
          }),
        }),
        units.currencyAmount,
      ),
    ).toThrow(/contract_notional/i);

    // Zero or negative contract_notional
    for (const badVal of ['0', '-1000000']) {
      expect(() =>
        validatePreviewCoveredHedgeResult(
          req,
          create(PreviewCoveredHedgeResultSchema, {
            ...res,
            measures: create(FuturesHedgeMeasuresSchema, {
              ...res.measures!,
              contractNotional: createDecimalValue(badVal, units.currencyAmount!),
            }),
          }),
          units.currencyAmount,
        ),
      ).toThrow(/contract_notional/i);
    }

    // Wrong version on currency_amount unit
    expect(() =>
      validatePreviewCoveredHedgeResult(
        req,
        create(PreviewCoveredHedgeResultSchema, {
          ...res,
          measures: create(FuturesHedgeMeasuresSchema, {
            ...res.measures!,
            contractNotional: createDecimalValue('1000000', createUnitRef(ULID_A, 2n)),
          }),
        }),
        units.currencyAmount,
      ),
    ).toThrow(/contract_notional\.unit/i);

    // Excluded uncovered notional unit mismatch
    expect(() =>
      validatePreviewCoveredHedgeResult(
        req,
        create(PreviewCoveredHedgeResultSchema, {
          ...res,
          excludedUncoveredGrossNotional: createDecimalValue(
            '10000000',
            createUnitRef(ULID_A, 2n),
          ),
        }),
        undefined,
      ),
    ).toThrow(/excluded_uncovered_gross_notional/i);

    // Forbidden non-currency units masquerading as contract_notional.unit (even without expectedCurrencyUnit)
    for (const forbiddenUnit of [
      units.dimensionless!,
      units.dv01Per100!,
      units.dv01!,
      units.contractCount!,
    ]) {
      const noExclRes = create(PreviewCoveredHedgeResultSchema, {
        ...res,
        excludedUncoveredGrossNotional: undefined,
        measures: create(FuturesHedgeMeasuresSchema, {
          ...res.measures!,
          contractNotional: createDecimalValue('1000000', forbiddenUnit),
        }),
      });
      expect(() => validatePreviewCoveredHedgeResult(req, noExclRes, undefined)).toThrow(
        /non-currency/i,
      );
    }

    // Missing unit on contract_notional
    expect(() =>
      validatePreviewCoveredHedgeResult(
        req,
        create(PreviewCoveredHedgeResultSchema, {
          ...res,
          measures: create(FuturesHedgeMeasuresSchema, {
            ...res.measures!,
            contractNotional: create(DecimalValueSchema, {
              coefficient: '1000000',
              scale: 0,
              unit: create(UlidSchema, { value: '' }) ? undefined : undefined,
            }),
          }),
        }),
        units.currencyAmount,
      ),
    ).toThrow(/unit/i);
  });
});
