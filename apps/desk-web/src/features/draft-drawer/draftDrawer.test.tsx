import { create, type MessageInitShape } from '@bufbuild/protobuf';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  AccountBlotterSnapshotSchema,
  AccountChangeSchema,
  BlotterEventEnvelopeSchema,
  CoverageStatus,
  DraftEarmarkRecordSchema,
  DraftEarmarkStatus,
  DraftTargetState,
  EarmarkRecordSchema,
  EarmarkState,
  InstrumentPositionViewSchema,
  OpeningPositionBaseSchema,
  PrincipalRole,
  QuoteTermsSchema,
  RiskAndCoverageSummarySchema,
  RuleLineageSchema,
  SettlementBucket,
  SourceTextAnchorSchema,
  TradeSide,
  UncoveredReason,
  type AccountBlotterSnapshot,
  type DraftEarmarkRecord,
} from '../../generated/ficant/blotter/v1/model_pb';
import {
  ConfirmDraftEarmarkResultSchema,
  SubmitDraftEarmarkResultSchema,
  type ConfirmDraftEarmarkRequest,
  type SubmitDraftEarmarkRequest,
} from '../../generated/ficant/blotter/v1/service_pb';
import {
  NlpEarmarkProposalSchema,
  type NlpEarmarkProposal,
} from '../../generated/ficant/blotter/v1/spi_pb';
import {
  ErrorCode,
  ErrorDetailSchema,
  FieldViolationSchema,
} from '../../generated/ficant/core/v1/error_pb';
import { AnalysisUnitsSchema, BondTaxBasis } from '../../generated/ficant/rates/v1/analytics_pb';
import {
  createDecimalValue,
  createSha256FromHex,
  createUlid,
  createUnitRef,
  utf8ByteLength,
} from '../../codec';
import { createBlotterStore, type BlotterStoreApi } from '../../store';
import { BlotterClientError, type DeskBlotterRpcClient } from '../../transport';
import { DraftDrawerPanel } from './DraftDrawerPanel';

afterEach(() => {
  cleanup();
});

const ULID_CCY = '01JNB0G1UNITCURRAMT0000001';
const ULID_PRICE = '01JNB0G1UNITPRICE000000002';
const ULID_RATE = '01JNB0G1UNITRATE0000000003';
const ULID_DV01 = '01JNB0G1UNITDV010000000004';
const ULID_DV01P100 = '01JNB0G1UNITDV01P100000005';
const ULID_DIM = '01JNB0G1UNITDIMLESS0000006';
const ULID_CNT = '01JNB0G1UNITCNTRCNT0000007';
const ULID_PACK = '01JNB0G1REGRPK000000000008';
const ULID_DRAFT_1 = '01JNB0G1DRAFTREC0000000001';
const ULID_DRAFT_2 = '01JNB0G1DRAFTREC0000000002';
const ULID_EARMARK_1 = '01JNB0G1EARMARKREC00000001';

const TOKEN_HEX_1 = '11'.repeat(32);
const TOKEN_HEX_2 = '22'.repeat(32);
const CONTENT_HASH_HEX = 'aa'.repeat(32);
const CONSTRAINTS_HASH_HEX = 'bb'.repeat(32);
const CONTEXT_HASH_HEX = 'cc'.repeat(32);

function makeUnits() {
  return create(AnalysisUnitsSchema, {
    currencyAmount: createUnitRef(ULID_CCY, 1n),
    pricePer100: createUnitRef(ULID_PRICE, 1n),
    rate: createUnitRef(ULID_RATE, 1n),
    dv01: createUnitRef(ULID_DV01, 1n),
    dv01Per100: createUnitRef(ULID_DV01P100, 1n),
    dimensionless: createUnitRef(ULID_DIM, 1n),
    contractCount: createUnitRef(ULID_CNT, 1n),
  });
}

function makeRuleLineage() {
  return create(RuleLineageSchema, {
    packId: createUlid(ULID_PACK),
    version: 3n,
    source: 'desk-regulatory-registry',
    contentHash: createSha256FromHex(CONTENT_HASH_HEX),
    parsedConstraintsHash: createSha256FromHex(CONSTRAINTS_HASH_HEX),
  });
}

function makeSnapshot(params: {
  readonly accountId?: string;
  readonly seq: bigint;
  readonly positionVersion?: bigint;
  readonly pendingDrafts?: DraftEarmarkRecord[];
  readonly activeEarmarksCount?: number;
}): AccountBlotterSnapshot {
  const accountId = params.accountId ?? 'acct-desk-1';
  const units = makeUnits();
  const activeEarmarks = Array.from({ length: params.activeEarmarksCount ?? 0 }, (_, idx) =>
    create(EarmarkRecordSchema, {
      earmarkId: createUlid(`01JNB0G1EARMARK0000000000${idx + 1}`),
      earmarkRevision: 1n,
      accountId,
      instrumentCode: '240011.IB',
      side: TradeSide.BUY,
      settlementBucket: SettlementBucket.T_PLUS_0,
      notional: createDecimalValue('10000000', units.currencyAmount!),
      state: EarmarkState.HOLD,
      createdSeq: params.seq,
      updatedSeq: params.seq,
    }),
  );

  return create(AccountBlotterSnapshotSchema, {
    snapshotSeq: params.seq,
    accountId,
    accountVersion: 1n,
    openingBase: create(OpeningPositionBaseSchema, {
      accountId,
      units,
    }),
    riskSummary: create(RiskAndCoverageSummarySchema, {
      coveredInstrumentCount: 1,
      uncoveredInstrumentCount: 0,
      coveredTaxBasis: BondTaxBasis.PRE_TAX,
      coveredNetPreTaxDv01: createDecimalValue('12000', units.dv01!),
      coveredGrossNotional: createDecimalValue('50000000', units.currencyAmount!),
      uncoveredGrossNotional: createDecimalValue('0', units.currencyAmount!),
      uncoveredNetNotional: createDecimalValue('0', units.currencyAmount!),
      wholeAccountHedgeAllowed: true,
    }),
    positions: [
      create(InstrumentPositionViewSchema, {
        accountId,
        instrumentCode: '240011.IB',
        positionVersion: params.positionVersion ?? 4n,
        economicNetNotional: createDecimalValue('50000000', units.currencyAmount!),
        coverageStatus: CoverageStatus.COVERED_PRE_TAX,
        taxBasis: BondTaxBasis.PRE_TAX,
        preTaxDv01: createDecimalValue('12000', units.dv01!),
      }),
    ],
    activeEarmarks,
    pendingDrafts: params.pendingDrafts ?? [],
  });
}

function makeSeededStore(
  snapshot: AccountBlotterSnapshot,
  roleHint: PrincipalRole | null = PrincipalRole.TRADER,
): {
  readonly store: BlotterStoreApi;
  readonly sessionGen: number;
  readonly transportEpoch: number;
  readonly readerId: string;
} {
  const store = createBlotterStore();
  const sessionGen = store.getState().beginSession({
    headerMode: 'authorization',
    roleHint,
    actorLabel: 'trader-1',
  });
  store.getState().setRequestedAccounts([snapshot.accountId], sessionGen);
  const fence = store.getState().activateReader(sessionGen, 'stream', 'stream_live');
  if (!fence) {
    throw new Error('Failed to activate test reader');
  }
  const commit = store.getState().commitBaselineSnapshots({
    sessionGeneration: fence.sessionGeneration,
    transportEpoch: fence.transportEpoch,
    readerId: fence.readerId,
    snapshots: new Map([[snapshot.accountId, snapshot]]),
    cursorSeq: snapshot.snapshotSeq,
    headSeq: snapshot.snapshotSeq,
    nextConnectionState: 'stream_live',
  });
  if (!commit.committed) {
    throw new Error(`Baseline commit failed: ${commit.reason}`);
  }
  return {
    store,
    sessionGen: fence.sessionGeneration,
    transportEpoch: fence.transportEpoch,
    readerId: fence.readerId,
  };
}

function makeChineseAnchor(rawText: string, fieldName: string, substring: string) {
  const idx = rawText.indexOf(substring);
  if (idx < 0) {
    throw new Error(`Substring "${substring}" not found in "${rawText}"`);
  }
  const byteStart = utf8ByteLength(rawText.slice(0, idx));
  const byteEnd = byteStart + utf8ByteLength(substring);
  return create(SourceTextAnchorSchema, {
    fieldName,
    byteStart,
    byteEnd,
    spanText: substring,
  });
}

function makeValidChineseProposal(
  overrides: MessageInitShape<typeof NlpEarmarkProposalSchema> = {},
): NlpEarmarkProposal {
  const units = makeUnits();
  const rawSourceText = '买入 240011.IB 1000万 T+0 净价 100.25';
  const anchors = [
    makeChineseAnchor(rawSourceText, 'side', '买入'),
    makeChineseAnchor(rawSourceText, 'instrument_code', '240011.IB'),
    makeChineseAnchor(rawSourceText, 'notional', '1000万'),
  ];
  return create(NlpEarmarkProposalSchema, {
    parserSourceId: 'intranet-nlp-v1',
    modelVersion: '2026.11',
    rawSourceText,
    anchors,
    confidenceScore: createDecimalValue('0.94', units.dimensionless!),
    accountId: 'acct-desk-1',
    instrumentCode: '240011.IB',
    side: TradeSide.BUY,
    settlementBucket: SettlementBucket.T_PLUS_0,
    notional: createDecimalValue('10000000', units.currencyAmount!),
    quoteTerms: create(QuoteTermsSchema, {
      priceOrYield: {
        case: 'cleanPricePer100',
        value: createDecimalValue('100.25', units.pricePer100!),
      },
    }),
    suggestedTargetState: DraftTargetState.COMMITTED,
    ...overrides,
  });
}

function makeDraftRecord(params: {
  readonly draftId?: string;
  readonly draftRevision?: bigint;
  readonly targetState?: DraftTargetState;
  readonly status?: DraftEarmarkStatus;
  readonly previewCoverageStatus?: CoverageStatus;
  readonly previewDeltaPreTaxDv01?: string | null;
  readonly previewUncoveredReason?: UncoveredReason;
  readonly previewDeltaUncoveredNotional?: string | null;
  readonly previewAtSeq?: bigint;
  readonly tokenHex?: string;
  readonly omitRuleLineage?: boolean;
}): DraftEarmarkRecord {
  const units = makeUnits();
  const coverage = params.previewCoverageStatus ?? CoverageStatus.COVERED_PRE_TAX;
  const rawSourceText = '买入 240011.IB 1000万 T+0 净价 100.25';

  const dv01Val =
    params.previewDeltaPreTaxDv01 === null
      ? undefined
      : params.previewDeltaPreTaxDv01 !== undefined
        ? createDecimalValue(params.previewDeltaPreTaxDv01, units.dv01!)
        : coverage === CoverageStatus.COVERED_PRE_TAX
          ? createDecimalValue('4820.5', units.dv01!)
          : undefined;

  const uncoveredNotionalVal =
    params.previewDeltaUncoveredNotional === null
      ? undefined
      : params.previewDeltaUncoveredNotional !== undefined
        ? createDecimalValue(params.previewDeltaUncoveredNotional, units.currencyAmount!)
        : coverage === CoverageStatus.UNCOVERED
          ? createDecimalValue('10000000', units.currencyAmount!)
          : undefined;

  return create(DraftEarmarkRecordSchema, {
    draftId: createUlid(params.draftId ?? ULID_DRAFT_1),
    draftRevision: params.draftRevision ?? 1n,
    accountId: 'acct-desk-1',
    instrumentCode: '240011.IB',
    side: TradeSide.BUY,
    settlementBucket: SettlementBucket.T_PLUS_0,
    notional: createDecimalValue('10000000', units.currencyAmount!),
    quoteTerms: create(QuoteTermsSchema, {
      priceOrYield: {
        case: 'cleanPricePer100',
        value: createDecimalValue('100.25', units.pricePer100!),
      },
    }),
    targetState: params.targetState ?? DraftTargetState.COMMITTED,
    status: params.status ?? DraftEarmarkStatus.PENDING_CONFIRMATION,
    rawSourceText,
    anchors: [makeChineseAnchor(rawSourceText, 'side', '买入')],
    confidenceScore: createDecimalValue('0.94', units.dimensionless!),
    parserSourceId: 'intranet-nlp-v1',
    previewCoverageStatus: coverage,
    ...(dv01Val !== undefined ? { previewDeltaPreTaxDv01: dv01Val } : {}),
    previewUncoveredReason:
      params.previewUncoveredReason ??
      (coverage === CoverageStatus.UNCOVERED
        ? UncoveredReason.MISSING_MARKET_QUOTE
        : UncoveredReason.UNSPECIFIED),
    ...(uncoveredNotionalVal !== undefined
      ? { previewDeltaUncoveredNotional: uncoveredNotionalVal }
      : {}),
    previewAtSeq: params.previewAtSeq ?? 11n,
    previewQuoteSeq: 5n,
    confirmationToken: createSha256FromHex(params.tokenHex ?? TOKEN_HEX_1),
    ...(params.omitRuleLineage ? {} : { ruleLineage: makeRuleLineage() }),
    previewQuoteSourceId: 'quote-src-1',
    contextHash: createSha256FromHex(CONTEXT_HASH_HEX),
    valuationSettlementDate: '2026-11-21',
  });
}

function makeMockClient(overrides: Partial<DeskBlotterRpcClient> = {}): DeskBlotterRpcClient {
  const notMocked = (name: string) => () => {
    throw new Error(`Unexpected RPC call: ${name}`);
  };
  return {
    getAccountSnapshot: vi.fn(notMocked('getAccountSnapshot')),
    pollEvents: vi.fn(notMocked('pollEvents')),
    subscribeEvents: vi.fn(notMocked('subscribeEvents')),
    previewCoveredHedge: vi.fn(notMocked('previewCoveredHedge')),
    importOpeningBase: vi.fn(notMocked('importOpeningBase')),
    ingestMarketQuotes: vi.fn(notMocked('ingestMarketQuotes')),
    createHold: vi.fn(notMocked('createHold')),
    extendHold: vi.fn(notMocked('extendHold')),
    confirmHold: vi.fn(notMocked('confirmHold')),
    releaseHold: vi.fn(notMocked('releaseHold')),
    createCommitted: vi.fn(notMocked('createCommitted')),
    cancelCommitted: vi.fn(notMocked('cancelCommitted')),
    submitDraftEarmark: vi.fn(notMocked('submitDraftEarmark')),
    confirmDraftEarmark: vi.fn(notMocked('confirmDraftEarmark')),
    ingestTradeFeed: vi.fn(notMocked('ingestTradeFeed')),
    resolvePendingMatch: vi.fn(notMocked('resolvePendingMatch')),
    sweepLifecycle: vi.fn(notMocked('sweepLifecycle')),
    setFeedControl: vi.fn(notMocked('setFeedControl')),
    fetchOpeningPositionBase: vi.fn(notMocked('fetchOpeningPositionBase')),
    parseNlpIntent: vi.fn(notMocked('parseNlpIntent')),
    ...overrides,
  };
}

describe('DraftDrawerPanel (T-006 / AC-006 / AC-008)', () => {
  it('enforces explicit paste only with zero silent clipboard reads and highlights Chinese UTF-8 anchors', async () => {
    const readTextSpy = vi.fn().mockRejectedValue(new Error('must never be called'));
    const originalClipboard = navigator.clipboard;
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { readText: readTextSpy },
    });

    try {
      const proposal = makeValidChineseProposal();
      const parseNlpIntent = vi.fn().mockResolvedValue(proposal);
      const client = makeMockClient({ parseNlpIntent });
      const { store } = makeSeededStore(makeSnapshot({ seq: 10n }));
      const onClose = vi.fn();

      render(<DraftDrawerPanel store={store} client={client} onClose={onClose} />);

      // Zero background polling or mount-time parse / clipboard read.
      expect(parseNlpIntent).not.toHaveBeenCalled();
      expect(readTextSpy).not.toHaveBeenCalled();

      const textarea = screen.getByTestId('draft-paste-textarea');
      fireEvent.paste(textarea, {
        clipboardData: {
          getData: (type: string) => (type === 'text/plain' ? proposal.rawSourceText : ''),
        },
      });

      await waitFor(() => {
        expect(parseNlpIntent).toHaveBeenCalledTimes(1);
      });
      expect(parseNlpIntent).toHaveBeenCalledWith(
        expect.objectContaining({
          accountId: 'acct-desk-1',
          rawSourceText: proposal.rawSourceText,
        }),
      );
      expect(readTextSpy).not.toHaveBeenCalled();
      expect(screen.getByTestId('draft-input-source').textContent).toContain('显式粘贴');

      // Exact CJK & ASCII highlights rendered as plain text-node <mark> elements.
      const marks = screen.getAllByTestId('draft-anchor-highlight');
      expect(marks.map((el) => el.textContent)).toEqual(['买入', '240011.IB', '1000万']);
      expect(screen.getByTestId('draft-highlighted-text').textContent).toBe(proposal.rawSourceText);

      // Escape closes drawer via onClose callback for T-007 shortcut integration.
      fireEvent.keyDown(screen.getByTestId('draft-drawer-panel'), { key: 'Escape' });
      expect(onClose).toHaveBeenCalledTimes(1);
    } finally {
      Object.defineProperty(navigator, 'clipboard', {
        configurable: true,
        value: originalClipboard,
      });
    }
  });

  it('rejects broken UTF-8 CJK anchor byte boundary, wrong confidence_score.unit ID/version, out-of-range confidence, and wrong notional unit', async () => {
    const units = makeUnits();
    const rawText = '买入 240011.IB 1000万';
    // Byte 1 splits inside the 3-byte UTF-8 character '买' (bytes 0..3).
    const brokenAnchorProposal = makeValidChineseProposal({
      rawSourceText: rawText,
      anchors: [
        create(SourceTextAnchorSchema, {
          fieldName: 'side',
          byteStart: 1,
          byteEnd: 4,
          spanText: '买',
        }),
      ],
    });
    const badConfidenceUnitVersionProposal = makeValidChineseProposal({
      confidenceScore: createDecimalValue('0.88', createUnitRef(ULID_DIM, 2n)),
    });
    const badConfidenceUnitIdProposal = makeValidChineseProposal({
      confidenceScore: createDecimalValue('0.88', units.currencyAmount!),
    });
    const outOfRangeConfidenceProposal = makeValidChineseProposal({
      confidenceScore: createDecimalValue('1.05', units.dimensionless!),
    });
    const badNotionalUnitProposal = makeValidChineseProposal({
      notional: createDecimalValue('10000000', units.pricePer100!),
    });

    const cases: ReadonlyArray<readonly [string, NlpEarmarkProposal, string]> = [
      ['mid-CJK UTF-8 byte boundary', brokenAnchorProposal, 'proposal.anchors'],
      ['confidence_score wrong unit version', badConfidenceUnitVersionProposal, 'proposal.confidence_score.unit'],
      ['confidence_score wrong unit ID', badConfidenceUnitIdProposal, 'proposal.confidence_score.unit'],
      ['confidence_score > 1', outOfRangeConfidenceProposal, 'proposal.confidence_score'],
      ['notional wrong unit', badNotionalUnitProposal, 'proposal.notional.unit'],
    ];

    for (const [_label, badProposal, expectedField] of cases) {
      cleanup();
      const submitDraftEarmark = vi.fn();
      const parseNlpIntent = vi.fn().mockResolvedValue(badProposal);
      const client = makeMockClient({ parseNlpIntent, submitDraftEarmark });
      const { store } = makeSeededStore(makeSnapshot({ seq: 10n }));

      render(<DraftDrawerPanel store={store} client={client} />);
      fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
        clipboardData: { getData: () => badProposal.rawSourceText },
      });

      const issuesBox = await screen.findByTestId('draft-proposal-issues');
      expect(issuesBox.textContent).toContain(expectedField);
      const submitBtn = screen.getByTestId('draft-submit-button') as HTMLButtonElement;
      expect(submitBtn.disabled).toBe(true);
      expect(submitDraftEarmark).not.toHaveBeenCalled();
    }
  });

  it('accepts positive single settlement_amount alone and rejects empty, zero, and negative quotes', async () => {
    const user = userEvent.setup();
    const proposal = makeValidChineseProposal();
    const submittedDraft = makeDraftRecord({ previewAtSeq: 11n });
    const parseNlpIntent = vi.fn().mockResolvedValue(proposal);
    const submitDraftEarmark = vi.fn().mockResolvedValue(
      create(SubmitDraftEarmarkResultSchema, {
        committedSeq: 11n,
        replayed: false,
        draft: submittedDraft,
      }),
    );
    const client = makeMockClient({ parseNlpIntent, submitDraftEarmark });
    const { store } = makeSeededStore(makeSnapshot({ seq: 10n }));

    render(<DraftDrawerPanel store={store} client={client} />);
    fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
      clipboardData: { getData: () => proposal.rawSourceText },
    });
    await screen.findByTestId('draft-quote-editor');

    // 1) Empty quote -> blocked
    await user.selectOptions(screen.getByTestId('draft-quote-mode'), 'none');
    expect((screen.getByTestId('draft-submit-button') as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId('draft-proposal-issues').textContent).toContain('quote_terms');

    // 2) Negative settlement_amount -> blocked
    const settlementInput = screen.getByTestId('draft-settlement-amount-input');
    await user.clear(settlementInput);
    await user.type(settlementInput, '-5000');
    expect((screen.getByTestId('draft-submit-button') as HTMLButtonElement).disabled).toBe(true);

    // 3) Zero settlement_amount -> blocked
    await user.clear(settlementInput);
    await user.type(settlementInput, '0');
    expect((screen.getByTestId('draft-submit-button') as HTMLButtonElement).disabled).toBe(true);

    // 4) Negative clean_price_per_100 -> blocked
    await user.clear(settlementInput);
    await user.selectOptions(screen.getByTestId('draft-quote-mode'), 'clean_price');
    const cleanInput = screen.getByTestId('draft-clean-price-input');
    await user.clear(cleanInput);
    await user.type(cleanInput, '-99.5');
    expect((screen.getByTestId('draft-submit-button') as HTMLButtonElement).disabled).toBe(true);
    expect(submitDraftEarmark).not.toHaveBeenCalled();

    // 5) Positive single settlement_amount alone (no price/yield) -> ACCEPTED and submitted!
    await user.selectOptions(screen.getByTestId('draft-quote-mode'), 'none');
    await user.clear(settlementInput);
    await user.type(settlementInput, '10025000.5');

    const submitBtn = screen.getByTestId('draft-submit-button') as HTMLButtonElement;
    expect(submitBtn.disabled).toBe(false);
    await user.click(submitBtn);

    await waitFor(() => {
      expect(submitDraftEarmark).toHaveBeenCalledTimes(1);
    });
    const sentReq = submitDraftEarmark.mock.calls[0]?.[0] as SubmitDraftEarmarkRequest;
    expect(sentReq.proposal?.quoteTerms?.priceOrYield.case).toBeUndefined();
    expect(sentReq.proposal?.quoteTerms?.settlementAmount?.coefficient).toBe('100250005');
    expect(sentReq.proposal?.quoteTerms?.settlementAmount?.scale).toBe(1);
  });

  it('displays PRE_TAX delta, unit, and preview_at_seq adjacent to Confirm for covered non-zero, covered zero HOLD, missing delta, and uncovered drafts, without any TTL field', async () => {
    const user = userEvent.setup();
    const proposal = makeValidChineseProposal();

    // Case A: Covered non-zero COMMITTED draft -> no checkbox, confirms without TTL field
    {
      const draft = makeDraftRecord({
        targetState: DraftTargetState.COMMITTED,
        previewCoverageStatus: CoverageStatus.COVERED_PRE_TAX,
        previewDeltaPreTaxDv01: '-4820.5',
        previewAtSeq: 21n,
      });
      const confirmDraftEarmark = vi.fn().mockResolvedValue(
        create(ConfirmDraftEarmarkResultSchema, {
          committedSeq: 22n,
          replayed: false,
          draft: makeDraftRecord({ status: DraftEarmarkStatus.CONFIRMED, previewAtSeq: 21n }),
          earmark: create(EarmarkRecordSchema, { earmarkId: createUlid(ULID_EARMARK_1) }),
        }),
      );
      const client = makeMockClient({
        parseNlpIntent: vi.fn().mockResolvedValue(proposal),
        submitDraftEarmark: vi.fn().mockResolvedValue(
          create(SubmitDraftEarmarkResultSchema, { committedSeq: 21n, replayed: false, draft }),
        ),
        confirmDraftEarmark,
      });
      const { store } = makeSeededStore(makeSnapshot({ seq: 20n, positionVersion: 7n }));

      render(<DraftDrawerPanel store={store} client={client} />);
      fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
        clipboardData: { getData: () => proposal.rawSourceText },
      });
      await screen.findByTestId('draft-submit-button');
      await user.click(screen.getByTestId('draft-submit-button'));

      await screen.findByTestId('draft-confirm-preview');
      expect(screen.getByTestId('preview-dv01-value').textContent).toBe('-4,820.5');
      expect(screen.getByTestId('preview-dv01-unit').textContent).toBe(`${ULID_DV01}@v1`);
      expect(screen.getByTestId('preview-at-seq').textContent).toBe('21');
      // Covered draft MUST NOT show uncovered risk checkbox.
      expect(screen.queryByTestId('acknowledge-uncovered-checkbox')).toBeNull();

      const confirmBtn = screen.getByTestId('draft-confirm-button') as HTMLButtonElement;
      expect(confirmBtn.disabled).toBe(false);
      await user.click(confirmBtn);

      await waitFor(() => {
        expect(confirmDraftEarmark).toHaveBeenCalledTimes(1);
      });
      const confirmReq = confirmDraftEarmark.mock.calls[0]?.[0] as ConfirmDraftEarmarkRequest &
        Record<string, unknown>;
      expect(confirmReq.expectedDraftRevision).toBe(1n);
      expect(confirmReq.expectedPositionVersion).toBe(7n);
      expect(confirmReq.acknowledgeUncoveredDv01).toBe(false);
      // ConfirmDraftEarmark MUST NOT carry any manual TTL field.
      expect('ttlSeconds' in confirmReq).toBe(false);
      expect('ttl_seconds' in confirmReq).toBe(false);
      cleanup();
    }

    // Case B: Covered HOLD draft with numeric 0 delta -> displays "0" (distinct from "未提供"), enabled
    {
      const zeroHoldDraft = makeDraftRecord({
        targetState: DraftTargetState.HOLD,
        previewCoverageStatus: CoverageStatus.COVERED_PRE_TAX,
        previewDeltaPreTaxDv01: '0',
        previewAtSeq: 31n,
      });
      const client = makeMockClient({
        parseNlpIntent: vi.fn().mockResolvedValue(proposal),
        submitDraftEarmark: vi.fn().mockResolvedValue(
          create(SubmitDraftEarmarkResultSchema, {
            committedSeq: 31n,
            replayed: false,
            draft: zeroHoldDraft,
          }),
        ),
      });
      const { store } = makeSeededStore(makeSnapshot({ seq: 30n }));

      render(<DraftDrawerPanel store={store} client={client} />);
      fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
        clipboardData: { getData: () => proposal.rawSourceText },
      });
      await screen.findByTestId('draft-submit-button');
      await user.click(screen.getByTestId('draft-submit-button'));

      await screen.findByTestId('preview-covered-zero-hold');
      expect(screen.getByTestId('preview-dv01-value').textContent).toBe('0');
      expect(screen.getByTestId('preview-dv01-unit').textContent).toBe(`${ULID_DV01}@v1`);
      expect(screen.getByTestId('preview-at-seq').textContent).toBe('31');
      expect(screen.queryByTestId('acknowledge-uncovered-checkbox')).toBeNull();
      expect((screen.getByTestId('draft-confirm-button') as HTMLButtonElement).disabled).toBe(
        false,
      );
      cleanup();
    }

    // Case C: Missing preview_delta_pre_tax_dv01 -> displays "未提供" and disables Confirm
    {
      const missingDeltaDraft = makeDraftRecord({
        previewCoverageStatus: CoverageStatus.COVERED_PRE_TAX,
        previewDeltaPreTaxDv01: null,
        previewAtSeq: 41n,
      });
      const client = makeMockClient({
        parseNlpIntent: vi.fn().mockResolvedValue(proposal),
        submitDraftEarmark: vi.fn().mockResolvedValue(
          create(SubmitDraftEarmarkResultSchema, {
            committedSeq: 41n,
            replayed: false,
            draft: missingDeltaDraft,
          }),
        ),
      });
      const { store } = makeSeededStore(makeSnapshot({ seq: 40n }));

      render(<DraftDrawerPanel store={store} client={client} />);
      fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
        clipboardData: { getData: () => proposal.rawSourceText },
      });
      await screen.findByTestId('draft-submit-button');
      await user.click(screen.getByTestId('draft-submit-button'));

      await screen.findByTestId('preview-missing-delta');
      expect(screen.getByTestId('preview-dv01-value').textContent).toBe('未提供');
      expect(screen.getByTestId('preview-at-seq').textContent).toBe('41');
      expect(screen.queryByTestId('acknowledge-uncovered-checkbox')).toBeNull();
      expect((screen.getByTestId('draft-confirm-button') as HTMLButtonElement).disabled).toBe(true);
      cleanup();
    }

    // Case D: Valid uncovered draft -> shows reason, notional delta, unit, preview_at_seq & requires checkbox
    {
      const uncoveredDraft = makeDraftRecord({
        previewCoverageStatus: CoverageStatus.UNCOVERED,
        previewUncoveredReason: UncoveredReason.MISSING_MARKET_QUOTE,
        previewDeltaUncoveredNotional: '10000000',
        previewAtSeq: 51n,
      });
      const confirmDraftEarmark = vi.fn().mockResolvedValue(
        create(ConfirmDraftEarmarkResultSchema, {
          committedSeq: 52n,
          replayed: false,
          draft: makeDraftRecord({ status: DraftEarmarkStatus.CONFIRMED, previewAtSeq: 51n }),
          earmark: create(EarmarkRecordSchema, { earmarkId: createUlid(ULID_EARMARK_1) }),
        }),
      );
      const client = makeMockClient({
        parseNlpIntent: vi.fn().mockResolvedValue(proposal),
        submitDraftEarmark: vi.fn().mockResolvedValue(
          create(SubmitDraftEarmarkResultSchema, {
            committedSeq: 51n,
            replayed: false,
            draft: uncoveredDraft,
          }),
        ),
        confirmDraftEarmark,
      });
      const { store } = makeSeededStore(makeSnapshot({ seq: 50n }));

      render(<DraftDrawerPanel store={store} client={client} />);
      fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
        clipboardData: { getData: () => proposal.rawSourceText },
      });
      await screen.findByTestId('draft-submit-button');
      await user.click(screen.getByTestId('draft-submit-button'));

      await screen.findByTestId('preview-uncovered-valid');
      expect(screen.getByTestId('preview-uncovered-reason').textContent).toContain(
        'MISSING_MARKET_QUOTE',
      );
      expect(screen.getByTestId('preview-uncovered-notional').textContent).toBe('10,000,000');
      expect(screen.getByTestId('preview-uncovered-unit').textContent).toBe(`${ULID_CCY}@v1`);
      expect(screen.getByTestId('preview-at-seq').textContent).toBe('51');

      const checkbox = screen.getByTestId('acknowledge-uncovered-checkbox') as HTMLInputElement;
      const confirmBtn = screen.getByTestId('draft-confirm-button') as HTMLButtonElement;
      expect(checkbox.checked).toBe(false);
      expect(confirmBtn.disabled).toBe(true);

      await user.click(checkbox);
      expect(checkbox.checked).toBe(true);
      expect(confirmBtn.disabled).toBe(false);

      await user.click(confirmBtn);
      await waitFor(() => {
        expect(confirmDraftEarmark).toHaveBeenCalledTimes(1);
      });
      const req = confirmDraftEarmark.mock.calls[0]?.[0] as ConfirmDraftEarmarkRequest;
      expect(req.acknowledgeUncoveredDv01).toBe(true);
    }
  });

  it('blocks confirmation and omits uncovered checkbox when regulatory rules are missing, lineage is missing, or limits are breached', async () => {
    const user = userEvent.setup();
    const proposal = makeValidChineseProposal();

    // 1) Missing regulatory rule pack returned by SubmitDraftEarmark
    {
      const rulePackError = BlotterClientError.fromErrorDetail(
        create(ErrorDetailSchema, {
          code: ErrorCode.VALIDATION_FAILED,
          message: 'regulatory rule pack missing',
          retryable: false,
          fieldViolations: [
            create(FieldViolationSchema, {
              field: 'regulatory_rule_pack',
              description: 'regulatory rule pack missing or invalid',
            }),
          ],
        }),
      );
      const client = makeMockClient({
        parseNlpIntent: vi.fn().mockResolvedValue(proposal),
        submitDraftEarmark: vi.fn().mockRejectedValue(rulePackError),
      });
      const { store } = makeSeededStore(makeSnapshot({ seq: 10n }));

      render(<DraftDrawerPanel store={store} client={client} />);
      fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
        clipboardData: { getData: () => proposal.rawSourceText },
      });
      await screen.findByTestId('draft-submit-button');
      await user.click(screen.getByTestId('draft-submit-button'));

      const regErr = await screen.findByTestId('draft-regulatory-error');
      expect(regErr.textContent).toContain('regulatory_rule_pack');
      expect(screen.queryByTestId('acknowledge-uncovered-checkbox')).toBeNull();
      expect(screen.queryByTestId('draft-confirm-button')).toBeNull();
      cleanup();
    }

    // 2) Breached regulatory limit returned by SubmitDraftEarmark
    {
      const limitBreachedError = BlotterClientError.fromErrorDetail(
        create(ErrorDetailSchema, {
          code: ErrorCode.VALIDATION_FAILED,
          message: 'limit exceeded',
          retryable: false,
          fieldViolations: [
            create(FieldViolationSchema, {
              field: 'constraints[MAX_INST_FACE]',
              description: 'proposed trade breaches bound constraint limit',
            }),
          ],
        }),
      );
      const client = makeMockClient({
        parseNlpIntent: vi.fn().mockResolvedValue(proposal),
        submitDraftEarmark: vi.fn().mockRejectedValue(limitBreachedError),
      });
      const { store } = makeSeededStore(makeSnapshot({ seq: 10n }));

      render(<DraftDrawerPanel store={store} client={client} />);
      fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
        clipboardData: { getData: () => proposal.rawSourceText },
      });
      await screen.findByTestId('draft-submit-button');
      await user.click(screen.getByTestId('draft-submit-button'));

      const regErr = await screen.findByTestId('draft-regulatory-error');
      expect(regErr.textContent).toContain('constraints[MAX_INST_FACE]');
      expect(screen.queryByTestId('acknowledge-uncovered-checkbox')).toBeNull();
      cleanup();
    }

    // 3) Uncovered draft missing RuleLineage -> confirm disabled & checkbox NOT shown
    {
      const noLineageUncoveredDraft = makeDraftRecord({
        previewCoverageStatus: CoverageStatus.UNCOVERED,
        previewUncoveredReason: UncoveredReason.STALE_QUOTE,
        previewDeltaUncoveredNotional: '10000000',
        omitRuleLineage: true,
      });
      const client = makeMockClient({
        parseNlpIntent: vi.fn().mockResolvedValue(proposal),
        submitDraftEarmark: vi.fn().mockResolvedValue(
          create(SubmitDraftEarmarkResultSchema, {
            committedSeq: 11n,
            replayed: false,
            draft: noLineageUncoveredDraft,
          }),
        ),
      });
      const { store } = makeSeededStore(makeSnapshot({ seq: 10n }));

      render(<DraftDrawerPanel store={store} client={client} />);
      fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
        clipboardData: { getData: () => proposal.rawSourceText },
      });
      await screen.findByTestId('draft-submit-button');
      await user.click(screen.getByTestId('draft-submit-button'));

      await screen.findByTestId('draft-lineage-invalid');
      expect(screen.queryByTestId('acknowledge-uncovered-checkbox')).toBeNull();
      expect((screen.getByTestId('draft-confirm-button') as HTMLButtonElement).disabled).toBe(true);
    }
  });

  it('invalidates prior preview and confirmation token on CONCURRENCY_CONFLICT, shows updated preview on resubmit, resets checkbox, and keeps confirm calls at 0 until explicit re-confirm', async () => {
    const user = userEvent.setup();
    const proposal = makeValidChineseProposal();

    const draftV1 = makeDraftRecord({
      draftId: ULID_DRAFT_1,
      previewCoverageStatus: CoverageStatus.UNCOVERED,
      previewUncoveredReason: UncoveredReason.MISSING_MARKET_QUOTE,
      previewDeltaUncoveredNotional: '10000000',
      previewAtSeq: 11n,
      tokenHex: TOKEN_HEX_1,
    });
    const draftV2 = makeDraftRecord({
      draftId: ULID_DRAFT_2,
      previewCoverageStatus: CoverageStatus.UNCOVERED,
      previewUncoveredReason: UncoveredReason.STALE_QUOTE,
      previewDeltaUncoveredNotional: '12500000',
      previewAtSeq: 15n,
      tokenHex: TOKEN_HEX_2,
    });

    const conflictErr = BlotterClientError.fromErrorDetail(
      create(ErrorDetailSchema, {
        code: ErrorCode.CONCURRENCY_CONFLICT,
        message: 'draft context hash changed after quote update',
        retryable: true,
      }),
    );

    const submitDraftEarmark = vi
      .fn()
      .mockResolvedValueOnce(
        create(SubmitDraftEarmarkResultSchema, {
          committedSeq: 11n,
          replayed: false,
          draft: draftV1,
        }),
      )
      .mockResolvedValueOnce(
        create(SubmitDraftEarmarkResultSchema, {
          committedSeq: 15n,
          replayed: false,
          draft: draftV2,
        }),
      );

    const confirmDraftEarmark = vi
      .fn()
      .mockRejectedValueOnce(conflictErr)
      .mockResolvedValueOnce(
        create(ConfirmDraftEarmarkResultSchema, {
          committedSeq: 16n,
          replayed: false,
          draft: makeDraftRecord({
            draftId: ULID_DRAFT_2,
            status: DraftEarmarkStatus.CONFIRMED,
            previewCoverageStatus: CoverageStatus.UNCOVERED,
            previewUncoveredReason: UncoveredReason.STALE_QUOTE,
            previewDeltaUncoveredNotional: '12500000',
            previewAtSeq: 15n,
            tokenHex: TOKEN_HEX_2,
          }),
          earmark: create(EarmarkRecordSchema, { earmarkId: createUlid(ULID_EARMARK_1) }),
        }),
      );

    const client = makeMockClient({
      parseNlpIntent: vi.fn().mockResolvedValue(proposal),
      submitDraftEarmark,
      confirmDraftEarmark,
    });
    const seeded = makeSeededStore(makeSnapshot({ seq: 10n, positionVersion: 4n }));
    const onRefreshContext = vi.fn(async () => {
      // Simulate context advancing to seq=14, positionVersion=5 before resubmit.
      seeded.store.getState().commitStreamEnvelope({
        sessionGeneration: seeded.sessionGen,
        transportEpoch: seeded.transportEpoch,
        readerId: seeded.readerId,
        envelope: create(BlotterEventEnvelopeSchema, {
          seq: 14n,
          eventType: 'quote_updated',
          changes: [
            create(AccountChangeSchema, {
              accountId: 'acct-desk-1',
              replaceSnapshot: makeSnapshot({ seq: 14n, positionVersion: 5n }),
            }),
          ],
        }),
      });
    });

    render(
      <DraftDrawerPanel
        store={seeded.store}
        client={client}
        onRefreshContext={onRefreshContext}
      />,
    );

    // Parse & Submit V1
    fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
      clipboardData: { getData: () => proposal.rawSourceText },
    });
    await screen.findByTestId('draft-submit-button');
    await user.click(screen.getByTestId('draft-submit-button'));
    await screen.findByTestId('preview-uncovered-valid');

    expect(seeded.store.getState().activeDraftConfirmation?.draftId).toBe(ULID_DRAFT_1);

    // Check box and click Confirm -> triggers CONCURRENCY_CONFLICT
    await user.click(screen.getByTestId('acknowledge-uncovered-checkbox'));
    await user.click(screen.getByTestId('draft-confirm-button'));

    await screen.findByTestId('draft-conflict-notice');
    expect(screen.getByTestId('preview-invalidated-badge')).toBeTruthy();
    // Old token MUST be immediately cleared from store.
    expect(seeded.store.getState().activeDraftConfirmation).toBeNull();
    // Checkbox hidden & Confirm disabled while invalidated.
    expect(screen.queryByTestId('acknowledge-uncovered-checkbox')).toBeNull();
    expect((screen.getByTestId('draft-confirm-button') as HTMLButtonElement).disabled).toBe(true);
    expect(confirmDraftEarmark).toHaveBeenCalledTimes(1);

    // Reset spy counter to strictly assert 0 confirm calls after resubmit until user explicitly clicks Confirm!
    confirmDraftEarmark.mockClear();

    // User clicks Resubmit -> refreshes context & submits draft V2 with a fresh idempotency key
    await user.click(screen.getByTestId('draft-resubmit-button'));
    await waitFor(() => {
      expect(submitDraftEarmark).toHaveBeenCalledTimes(2);
    });
    expect(onRefreshContext).toHaveBeenCalledTimes(1);

    const firstSubmitKey = (submitDraftEarmark.mock.calls[0]?.[0] as SubmitDraftEarmarkRequest)
      .idempotencyKey;
    const secondSubmitKey = (submitDraftEarmark.mock.calls[1]?.[0] as SubmitDraftEarmarkRequest)
      .idempotencyKey;
    expect(secondSubmitKey).not.toBe(firstSubmitKey);

    // Updated preview values displayed right next to Confirm button
    expect(screen.getByTestId('preview-uncovered-reason').textContent).toContain('STALE_QUOTE');
    expect(screen.getByTestId('preview-uncovered-notional').textContent).toBe('12,500,000');
    expect(screen.getByTestId('preview-at-seq').textContent).toBe('15');
    expect(seeded.store.getState().activeDraftConfirmation?.draftId).toBe(ULID_DRAFT_2);

    // Checkbox MUST be reset to unchecked; Confirm disabled; confirmDraftEarmark called 0 times!
    const refreshedCheckbox = screen.getByTestId(
      'acknowledge-uncovered-checkbox',
    ) as HTMLInputElement;
    expect(refreshedCheckbox.checked).toBe(false);
    expect((screen.getByTestId('draft-confirm-button') as HTMLButtonElement).disabled).toBe(true);
    expect(confirmDraftEarmark).toHaveBeenCalledTimes(0);

    // Only after explicit human re-check + explicit second click is ConfirmDraftEarmark invoked!
    await user.click(refreshedCheckbox);
    expect((screen.getByTestId('draft-confirm-button') as HTMLButtonElement).disabled).toBe(false);
    await user.click(screen.getByTestId('draft-confirm-button'));

    await waitFor(() => {
      expect(confirmDraftEarmark).toHaveBeenCalledTimes(1);
    });
    const secondConfirmReq = confirmDraftEarmark.mock.calls[0]?.[0] as ConfirmDraftEarmarkRequest;
    expect(secondConfirmReq.draftId?.value).toBe(ULID_DRAFT_2);
    expect(secondConfirmReq.expectedPositionVersion).toBe(5n);
    expect(secondConfirmReq.confirmationToken?.value).toEqual(
      createSha256FromHex(TOKEN_HEX_2).value,
    );
    expect(secondConfirmReq.acknowledgeUncoveredDv01).toBe(true);
    expect(await screen.findByTestId('draft-confirm-receipt')).toBeTruthy();
  });

  it('proves 3-stage snapshot boundary: Parse and Pending Draft create 0 active earmarks; only Confirm creates an active earmark', async () => {
    const user = userEvent.setup();
    const proposal = makeValidChineseProposal({ suggestedTargetState: DraftTargetState.HOLD });
    const pendingDraft = makeDraftRecord({
      targetState: DraftTargetState.HOLD,
      previewCoverageStatus: CoverageStatus.COVERED_PRE_TAX,
      previewDeltaPreTaxDv01: '0',
      previewAtSeq: 11n,
    });

    const seeded = makeSeededStore(makeSnapshot({ seq: 10n, activeEarmarksCount: 0 }));
    const client = makeMockClient({
      parseNlpIntent: vi.fn().mockResolvedValue(proposal),
      submitDraftEarmark: vi.fn().mockResolvedValue(
        create(SubmitDraftEarmarkResultSchema, {
          committedSeq: 11n,
          replayed: false,
          draft: pendingDraft,
        }),
      ),
      confirmDraftEarmark: vi.fn().mockResolvedValue(
        create(ConfirmDraftEarmarkResultSchema, {
          committedSeq: 12n,
          replayed: false,
          draft: makeDraftRecord({
            targetState: DraftTargetState.HOLD,
            status: DraftEarmarkStatus.CONFIRMED,
            previewDeltaPreTaxDv01: '0',
            previewAtSeq: 11n,
          }),
          earmark: create(EarmarkRecordSchema, {
            earmarkId: createUlid(ULID_EARMARK_1),
            state: EarmarkState.HOLD,
          }),
        }),
      ),
    });

    render(<DraftDrawerPanel store={seeded.store} client={client} />);

    // Stage 0: baseline seq=10, 0 earmarks, 0 pending drafts
    expect(screen.getByTestId('stage-snapshot-seq').textContent).toContain('10');
    expect(screen.getByTestId('stage-active-earmarks-count').textContent).toContain('0');
    expect(screen.getByTestId('stage-pending-drafts-count').textContent).toContain('0');

    // Stage 1: ParseNlpIntent (read-only) -> snapshot_seq unchanged (10), 0 earmarks, 0 pending drafts
    fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
      clipboardData: { getData: () => proposal.rawSourceText },
    });
    await screen.findByTestId('draft-proposal-review');
    expect(screen.getByTestId('draft-drawer-panel').getAttribute('data-draft-stage')).toBe(
      'parsed',
    );
    expect(seeded.store.getState().snapshots['acct-desk-1']?.snapshotSeq).toBe(10n);
    expect(seeded.store.getState().snapshots['acct-desk-1']?.activeEarmarks.length).toBe(0);
    expect(screen.getByTestId('stage-snapshot-seq').textContent).toContain('10');
    expect(screen.getByTestId('stage-active-earmarks-count').textContent).toContain('0');

    // Stage 2: SubmitDraftEarmark -> stream commits seq=11 with 1 pending draft, STILL 0 active earmarks!
    await user.click(screen.getByTestId('draft-submit-button'));
    await screen.findByTestId('draft-confirmation-section');
    act(() => {
      seeded.store.getState().commitStreamEnvelope({
        sessionGeneration: seeded.sessionGen,
        transportEpoch: seeded.transportEpoch,
        readerId: seeded.readerId,
        envelope: create(BlotterEventEnvelopeSchema, {
          seq: 11n,
          eventType: 'draft_submitted',
          changes: [
            create(AccountChangeSchema, {
              accountId: 'acct-desk-1',
              replaceSnapshot: makeSnapshot({
                seq: 11n,
                activeEarmarksCount: 0,
                pendingDrafts: [pendingDraft],
              }),
            }),
          ],
        }),
      });
    });

    expect(screen.getByTestId('stage-snapshot-seq').textContent).toContain('11');
    expect(screen.getByTestId('stage-pending-drafts-count').textContent).toContain('1');
    expect(screen.getByTestId('stage-active-earmarks-count').textContent).toContain('0');

    // Stage 3: ConfirmDraftEarmark -> stream commits seq=12 with 1 active earmark & 0 pending drafts!
    await user.click(screen.getByTestId('draft-confirm-button'));
    await screen.findByTestId('draft-confirm-receipt');
    act(() => {
      seeded.store.getState().commitStreamEnvelope({
        sessionGeneration: seeded.sessionGen,
        transportEpoch: seeded.transportEpoch,
        readerId: seeded.readerId,
        envelope: create(BlotterEventEnvelopeSchema, {
          seq: 12n,
          eventType: 'draft_confirmed',
          changes: [
            create(AccountChangeSchema, {
              accountId: 'acct-desk-1',
              replaceSnapshot: makeSnapshot({
                seq: 12n,
                activeEarmarksCount: 1,
                pendingDrafts: [],
              }),
            }),
          ],
        }),
      });
    });

    expect(screen.getByTestId('stage-snapshot-seq').textContent).toContain('12');
    expect(screen.getByTestId('stage-pending-drafts-count').textContent).toContain('0');
    expect(screen.getByTestId('stage-active-earmarks-count').textContent).toContain('1');
  });

  it('wipes pasted text, parsed proposal, and active draft synchronously on session switch', async () => {
    const proposal = makeValidChineseProposal();
    const client = makeMockClient({
      parseNlpIntent: vi.fn().mockResolvedValue(proposal),
    });
    const seeded = makeSeededStore(makeSnapshot({ seq: 10n }));

    render(<DraftDrawerPanel store={seeded.store} client={client} />);
    fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
      clipboardData: { getData: () => proposal.rawSourceText },
    });
    await screen.findByTestId('draft-proposal-review');

    act(() => {
      seeded.store.getState().beginSession({
        headerMode: 'authorization',
        roleHint: PrincipalRole.TRADER,
        actorLabel: 'trader-2',
      });
    });

    expect((screen.getByTestId('draft-paste-textarea') as HTMLTextAreaElement).value).toBe('');
    expect(screen.queryByTestId('draft-proposal-review')).toBeNull();
    expect(seeded.store.getState().activeDraftConfirmation).toBeNull();
  });

  it('distinguishes incomplete uncovered preview from valid zero-delta uncovered HOLD, blocks missing constraint ceiling, and enforces drawer open/focus/role gating', async () => {
    const user = userEvent.setup();
    const proposal = makeValidChineseProposal({ suggestedTargetState: DraftTargetState.HOLD });

    // 1) Incomplete uncovered draft (missing preview_delta_uncovered_notional) -> "未提供", disabled, no checkbox
    {
      const incompleteUncovered = makeDraftRecord({
        targetState: DraftTargetState.HOLD,
        previewCoverageStatus: CoverageStatus.UNCOVERED,
        previewUncoveredReason: UncoveredReason.MISSING_MARKET_QUOTE,
        previewDeltaUncoveredNotional: null,
        previewAtSeq: 61n,
      });
      const client = makeMockClient({
        parseNlpIntent: vi.fn().mockResolvedValue(proposal),
        submitDraftEarmark: vi.fn().mockResolvedValue(
          create(SubmitDraftEarmarkResultSchema, {
            committedSeq: 61n,
            replayed: false,
            draft: incompleteUncovered,
          }),
        ),
      });
      const { store } = makeSeededStore(makeSnapshot({ seq: 60n }));

      render(<DraftDrawerPanel store={store} client={client} />);
      fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
        clipboardData: { getData: () => proposal.rawSourceText },
      });
      await screen.findByTestId('draft-submit-button');
      await user.click(screen.getByTestId('draft-submit-button'));

      await screen.findByTestId('preview-uncovered-incomplete');
      expect(screen.getByTestId('preview-uncovered-notional').textContent).toBe('未提供');
      expect(screen.queryByTestId('acknowledge-uncovered-checkbox')).toBeNull();
      expect((screen.getByTestId('draft-confirm-button') as HTMLButtonElement).disabled).toBe(true);
      cleanup();
    }

    // 2) Valid uncovered HOLD draft with numeric 0 delta notional -> shows "0", requires checkbox
    {
      const zeroUncoveredHold = makeDraftRecord({
        targetState: DraftTargetState.HOLD,
        previewCoverageStatus: CoverageStatus.UNCOVERED,
        previewUncoveredReason: UncoveredReason.STALE_QUOTE,
        previewDeltaUncoveredNotional: '0',
        previewAtSeq: 71n,
      });
      const client = makeMockClient({
        parseNlpIntent: vi.fn().mockResolvedValue(proposal),
        submitDraftEarmark: vi.fn().mockResolvedValue(
          create(SubmitDraftEarmarkResultSchema, {
            committedSeq: 71n,
            replayed: false,
            draft: zeroUncoveredHold,
          }),
        ),
      });
      const { store } = makeSeededStore(makeSnapshot({ seq: 70n }));

      render(<DraftDrawerPanel store={store} client={client} />);
      fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
        clipboardData: { getData: () => proposal.rawSourceText },
      });
      await screen.findByTestId('draft-submit-button');
      await user.click(screen.getByTestId('draft-submit-button'));

      await screen.findByTestId('preview-uncovered-valid');
      expect(screen.getByTestId('preview-uncovered-notional').textContent).toBe('0');
      expect(screen.getByTestId('acknowledge-uncovered-checkbox')).toBeTruthy();
      expect((screen.getByTestId('draft-confirm-button') as HTMLButtonElement).disabled).toBe(true);
      cleanup();
    }

    // 3) Missing constraint limit ceiling (LINEAGE_INCOMPLETE) -> regulatory block, no checkbox
    {
      const missingLimitErr = BlotterClientError.fromErrorDetail(
        create(ErrorDetailSchema, {
          code: ErrorCode.LINEAGE_INCOMPLETE,
          message: 'lineage incomplete',
          retryable: false,
          fieldViolations: [
            create(FieldViolationSchema, {
              field: 'constraints[MAX_PORTFOLIO_FACE]',
              description: 'required constraint limit ceiling is missing',
            }),
          ],
        }),
      );
      const client = makeMockClient({
        parseNlpIntent: vi.fn().mockResolvedValue(proposal),
        submitDraftEarmark: vi.fn().mockRejectedValue(missingLimitErr),
      });
      const { store } = makeSeededStore(makeSnapshot({ seq: 80n }));

      render(<DraftDrawerPanel store={store} client={client} />);
      fireEvent.paste(screen.getByTestId('draft-paste-textarea'), {
        clipboardData: { getData: () => proposal.rawSourceText },
      });
      await screen.findByTestId('draft-submit-button');
      await user.click(screen.getByTestId('draft-submit-button'));

      const regErr = await screen.findByTestId('draft-regulatory-error');
      expect(regErr.textContent).toContain('MAX_PORTFOLIO_FACE');
      expect(screen.queryByTestId('acknowledge-uncovered-checkbox')).toBeNull();
      cleanup();
    }

    // 4) Drawer closed (isOpen=false) renders null; opening focuses textarea; NLP_PARSER cannot confirm
    {
      const draft = makeDraftRecord({ previewAtSeq: 91n });
      const client = makeMockClient({
        parseNlpIntent: vi.fn().mockResolvedValue(proposal),
        submitDraftEarmark: vi.fn().mockResolvedValue(
          create(SubmitDraftEarmarkResultSchema, {
            committedSeq: 91n,
            replayed: false,
            draft,
          }),
        ),
      });
      const { store } = makeSeededStore(makeSnapshot({ seq: 90n }), PrincipalRole.NLP_PARSER);

      const { rerender } = render(
        <DraftDrawerPanel store={store} client={client} isOpen={false} focusTrigger={0} />,
      );
      expect(screen.queryByTestId('draft-drawer-panel')).toBeNull();

      rerender(
        <DraftDrawerPanel store={store} client={client} isOpen={true} focusTrigger={1} />,
      );
      const textarea = screen.getByTestId('draft-paste-textarea');
      expect(document.activeElement).toBe(textarea);

      fireEvent.paste(textarea, {
        clipboardData: { getData: () => proposal.rawSourceText },
      });
      await screen.findByTestId('draft-submit-button');
      await user.click(screen.getByTestId('draft-submit-button'));
      await screen.findByTestId('draft-confirm-button');
      expect((screen.getByTestId('draft-confirm-button') as HTMLButtonElement).disabled).toBe(true);
    }
  });
});
