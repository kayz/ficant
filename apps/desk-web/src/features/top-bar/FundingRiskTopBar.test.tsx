import '@testing-library/jest-dom/vitest';
import { create } from '@bufbuild/protobuf';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { TRUSTED_RULEPACK_ANCHOR } from 'virtual:ficant-trusted-rulepack';
import {
  AccountBlotterSnapshotSchema,
  CoverageStatus,
  FeedHealthState,
  FeedStatusViewSchema,
  FundingGapSummarySchema,
  InstrumentPositionViewSchema,
  OpeningPositionBaseSchema,
  RiskAndCoverageSummarySchema,
  SeqRangeSchema,
  type AccountBlotterSnapshot,
} from '../../generated/ficant/blotter/v1/model_pb';
import { AnalysisUnitsSchema, BondTaxBasis } from '../../generated/ficant/rates/v1/analytics_pb';
import { createDecimalValue, createUnitRef } from '../../codec';
import { createBlotterStore, type BlotterStoreApi } from '../../store';
import { FundingRiskTopBar } from './index';

afterEach(() => {
  cleanup();
});

const UNIT_CCY = createUnitRef('01JNB0G1UNITCURRAMT0000001', 1n);
const UNIT_DV01 = createUnitRef('01JNB0G1UNITDV010000000004', 1n);

function seedSnapshot(store: BlotterStoreApi, snapshot: AccountBlotterSnapshot): void {
  const gen = store.getState().beginSession({ headerMode: 'authorization' });
  const fence = store.getState().activateReader(gen, 'bootstrap', 'bootstrapping');
  if (!fence) {
    throw new Error('failed to activate bootstrap reader');
  }
  const outcome = store.getState().commitBaselineSnapshots({
    sessionGeneration: fence.sessionGeneration,
    transportEpoch: fence.transportEpoch,
    readerId: fence.readerId,
    snapshots: new Map([[snapshot.accountId, snapshot]]),
    cursorSeq: snapshot.snapshotSeq,
    headSeq: snapshot.snapshotSeq,
    nextConnectionState: 'stream_live',
  });
  if (!outcome.committed) {
    throw new Error(`failed to commit baseline snapshot: ${outcome.reason}`);
  }
}

function makeBaseSnapshot(
  overrides: {
    readonly seq?: bigint;
    readonly accountId?: string;
    readonly coveredCount?: number;
    readonly uncoveredCount?: number;
    readonly coveredDv01?: string | undefined;
    readonly coveredGross?: string | undefined;
    readonly uncoveredGross?: string | undefined;
    readonly uncoveredNet?: string | undefined;
    readonly wholeAccountAllowed?: boolean;
    readonly funding?: Partial<Parameters<typeof create<typeof FundingGapSummarySchema>>[1]>;
    readonly omitFunding?: boolean;
  } = {},
): AccountBlotterSnapshot {
  const accountId = overrides.accountId ?? 'acct-01';
  const uncoveredCount = overrides.uncoveredCount ?? 0;
  return create(AccountBlotterSnapshotSchema, {
    snapshotSeq: overrides.seq ?? 42n,
    accountId,
    accountVersion: 2n,
    openingBase: create(OpeningPositionBaseSchema, {
      accountId,
      units: create(AnalysisUnitsSchema, {
        currencyAmount: UNIT_CCY,
        dv01: UNIT_DV01,
      }),
    }),
    riskSummary: create(RiskAndCoverageSummarySchema, {
      coveredInstrumentCount: overrides.coveredCount ?? 3,
      uncoveredInstrumentCount: uncoveredCount,
      coveredTaxBasis: BondTaxBasis.PRE_TAX,
      coveredNetPreTaxDv01:
        overrides.coveredDv01 !== undefined
          ? createDecimalValue(overrides.coveredDv01, UNIT_DV01)
          : undefined,
      coveredGrossNotional:
        overrides.coveredGross !== undefined
          ? createDecimalValue(overrides.coveredGross, UNIT_CCY)
          : undefined,
      uncoveredGrossNotional:
        overrides.uncoveredGross !== undefined
          ? createDecimalValue(overrides.uncoveredGross, UNIT_CCY)
          : undefined,
      uncoveredNetNotional:
        overrides.uncoveredNet !== undefined
          ? createDecimalValue(overrides.uncoveredNet, UNIT_CCY)
          : undefined,
      wholeAccountHedgeAllowed: overrides.wholeAccountAllowed ?? uncoveredCount === 0,
    }),
    fundingSummary: overrides.omitFunding
      ? undefined
      : create(FundingGapSummarySchema, {
          openingCashT0: createDecimalValue('50000000', UNIT_CCY),
          openingNetCashDeltaT1: createDecimalValue('-1000000', UNIT_CCY),
          netSettlementFlowT0: createDecimalValue('-12000000', UNIT_CCY),
          netSettlementFlowT1: createDecimalValue('5000000', UNIT_CCY),
          projectedCashT0: createDecimalValue('38000000', UNIT_CCY),
          projectedCashT1: createDecimalValue('42000000', UNIT_CCY),
          fundingGapT0: createDecimalValue('0', UNIT_CCY),
          fundingGapT1: createDecimalValue('0', UNIT_CCY),
          pendingMatchCashT0: createDecimalValue('0', UNIT_CCY),
          pendingMatchCashT1: createDecimalValue('0', UNIT_CCY),
          cleanPriceEstimatedFundingCount: 0,
          unpricedFundingEarmarkCount: 0,
          pendingMatchCount: 0,
          pendingUnknownCashCount: 0,
          feedIncomplete: false,
          fundingIsCompleteExact: true,
          ...overrides.funding,
        }),
    feedStatuses: [
      create(FeedStatusViewSchema, {
        sourceId: 'src-trade-1',
        feedStatusRevision: 4n,
        healthState: FeedHealthState.HEALTHY,
        earmarksPaused: false,
        pausedByGap: false,
        pausedByStaleness: false,
        pausedByOperator: false,
        missingFeedRanges: [],
      }),
    ],
    positions: [
      create(InstrumentPositionViewSchema, {
        accountId,
        instrumentCode: '240011.IB',
        positionVersion: 1n,
        coverageStatus: CoverageStatus.COVERED_PRE_TAX,
        taxBasis: BondTaxBasis.PRE_TAX,
      }),
    ],
    allowedHedgeProductCodes: ['T', 'TF', 'TL', 'TS'],
  });
}

describe('FundingRiskTopBar', () => {
  it('renders empty state without fabricated data-snapshot-seq when no snapshot is loaded', () => {
    const store = createBlotterStore();
    render(<FundingRiskTopBar store={store} valuationAt={new Date('2026-11-21T08:00:00Z')} />);

    const header = screen.getByTestId('funding-risk-top-bar');
    expect(header).not.toHaveAttribute('data-snapshot-seq');
    expect(screen.getByTestId('top-bar-empty')).toBeInTheDocument();
    expect(screen.getByTestId('rulepack-trust-pill')).toHaveAttribute(
      'data-trust-status',
      'verified',
    );
    expect(screen.getByTestId('rulepack-trust-pill')).toHaveTextContent(
      TRUSTED_RULEPACK_ANCHOR.manifest.rule_pack_id,
    );
  });

  it('distinguishes absent measures as 未提供 (never "0") while rendering genuine zero as "0"', () => {
    const store = createBlotterStore();
    seedSnapshot(
      store,
      makeBaseSnapshot({
        seq: 88n,
        coveredDv01: undefined, // absent -> MUST show 未提供
        coveredGross: '0', // genuine zero -> MUST show 0
      }),
    );

    render(<FundingRiskTopBar store={store} valuationAt={new Date('2026-11-21T08:00:00Z')} />);

    expect(screen.getByTestId('funding-risk-top-bar')).toHaveAttribute('data-snapshot-seq', '88');
    expect(screen.getByTestId('covered-net-pre-tax-dv01-value')).toHaveTextContent('未提供');
    expect(screen.getByTestId('covered-gross-notional-value')).toHaveTextContent('0');
    expect(screen.queryByTestId('uncovered-risk-warning')).not.toBeInTheDocument();
  });

  it('alerts and flags WHOLE_ACCOUNT disabled on pure-short uncovered (net < 0)', () => {
    const store = createBlotterStore();
    seedSnapshot(
      store,
      makeBaseSnapshot({
        seq: 101n,
        uncoveredCount: 1,
        uncoveredGross: '10000000',
        uncoveredNet: '-10000000',
        wholeAccountAllowed: false,
      }),
    );

    render(<FundingRiskTopBar store={store} valuationAt={new Date('2026-11-21T08:00:00Z')} />);

    expect(screen.getByTestId('uncovered-instrument-count-value')).toHaveTextContent('1');
    expect(screen.getByTestId('uncovered-gross-notional-value')).toHaveTextContent('10,000,000');
    expect(screen.getByTestId('uncovered-net-notional-value')).toHaveTextContent('-10,000,000');

    const warning = screen.getByTestId('uncovered-risk-warning');
    expect(warning).toHaveAttribute('data-whole-account-blocked', 'true');
    expect(warning).toHaveAttribute('data-pure-short', 'true');
    expect(warning).toHaveTextContent('WHOLE_ACCOUNT');
  });

  it('alerts and flags WHOLE_ACCOUNT disabled on net-zero-gross-nonzero offsetting uncovered positions', () => {
    const store = createBlotterStore();
    seedSnapshot(
      store,
      makeBaseSnapshot({
        seq: 102n,
        uncoveredCount: 2,
        uncoveredGross: '20000000',
        uncoveredNet: '0',
        wholeAccountAllowed: false,
      }),
    );

    render(<FundingRiskTopBar store={store} valuationAt={new Date('2026-11-21T08:00:00Z')} />);

    expect(screen.getByTestId('uncovered-instrument-count-value')).toHaveTextContent('2');
    expect(screen.getByTestId('uncovered-gross-notional-value')).toHaveTextContent('20,000,000');
    expect(screen.getByTestId('uncovered-net-notional-value')).toHaveTextContent('0');

    const warning = screen.getByTestId('uncovered-risk-warning');
    expect(warning).toHaveAttribute('data-whole-account-blocked', 'true');
    expect(warning).toHaveAttribute('data-net-zero-gross-nonzero', 'true');
    expect(warning).toHaveTextContent('净额为 0 但总面额（gross）非零');
  });

  it('renders all 5 funding completeness factors and independent pending match cash even when offsetting pending items sum to 0', () => {
    const store = createBlotterStore();
    const snap = makeBaseSnapshot({
      seq: 103n,
      funding: {
        fundingGapT0: createDecimalValue('-2500000', UNIT_CCY),
        fundingGapT1: createDecimalValue('-1000000', UNIT_CCY),
        pendingMatchCashT0: createDecimalValue('0', UNIT_CCY),
        pendingMatchCashT1: createDecimalValue('0', UNIT_CCY),
        cleanPriceEstimatedFundingCount: 2,
        unpricedFundingEarmarkCount: 1,
        pendingMatchCount: 2,
        pendingUnknownCashCount: 1,
        feedIncomplete: true,
        fundingIsCompleteExact: false,
      },
    });
    snap.feedStatuses = [
      create(FeedStatusViewSchema, {
        sourceId: 'src-gap',
        feedStatusRevision: 9n,
        healthState: FeedHealthState.GAP_DETECTED,
        earmarksPaused: true,
        pausedByGap: true,
        missingFeedRanges: [create(SeqRangeSchema, { first: 5n, last: 8n })],
      }),
    ];
    seedSnapshot(store, snap);

    render(<FundingRiskTopBar store={store} valuationAt={new Date('2026-11-21T08:00:00Z')} />);

    expect(screen.getByTestId('funding-gap-t0-value')).toHaveTextContent('-2,500,000');
    expect(screen.getByTestId('funding-gap-t1-value')).toHaveTextContent('-1,000,000');
    expect(screen.getByTestId('pending-match-cash-t0-value')).toHaveTextContent('0');
    expect(screen.getByTestId('pending-match-count-value')).toHaveTextContent('2');
    expect(screen.getByTestId('pending-zero-net-warning')).toHaveTextContent('正负现金相抵为 0');

    expect(screen.getByTestId('funding-completeness-badge')).toHaveAttribute(
      'data-complete-exact',
      'false',
    );
    expect(
      screen.getByTestId('completeness-factor-cleanPriceEstimatedFundingCount'),
    ).toHaveAttribute('data-triggered', 'true');
    expect(screen.getByTestId('completeness-factor-unpricedFundingEarmarkCount')).toHaveAttribute(
      'data-triggered',
      'true',
    );
    expect(screen.getByTestId('completeness-factor-pendingMatchCount')).toHaveAttribute(
      'data-triggered',
      'true',
    );
    expect(screen.getByTestId('completeness-factor-pendingUnknownCashCount')).toHaveAttribute(
      'data-triggered',
      'true',
    );
    expect(screen.getByTestId('completeness-factor-feedIncomplete')).toHaveAttribute(
      'data-triggered',
      'true',
    );

    expect(screen.getByTestId('top-bar-feed-src-gap')).toHaveAttribute(
      'data-earmarks-paused',
      'true',
    );
    expect(screen.getByTestId('top-bar-feed-src-gap')).toHaveTextContent('[5..8]');
  });

  it('reports RulePack trust pill failure states (missing, tampered, expired)', () => {
    const store = createBlotterStore();
    seedSnapshot(store, makeBaseSnapshot({ seq: 50n }));

    const { rerender } = render(
      <FundingRiskTopBar store={store} rulePackAnchor={null} />,
    );
    expect(screen.getByTestId('rulepack-trust-pill')).toHaveAttribute(
      'data-trust-status',
      'missing_anchor',
    );

    rerender(
      <FundingRiskTopBar
        store={store}
        rulePackAnchor={{
          ...TRUSTED_RULEPACK_ANCHOR,
          payloadSha256: '0'.repeat(64),
        }}
      />,
    );
    expect(screen.getByTestId('rulepack-trust-pill')).toHaveAttribute(
      'data-trust-status',
      'invalid_anchor',
    );

    // Half-open boundary valuationAt === effective_to -> expired
    rerender(
      <FundingRiskTopBar
        store={store}
        rulePackAnchor={TRUSTED_RULEPACK_ANCHOR}
        valuationAt={new Date(TRUSTED_RULEPACK_ANCHOR.manifest.effective_to.instant)}
      />,
    );
    expect(screen.getByTestId('rulepack-trust-pill')).toHaveAttribute(
      'data-trust-status',
      'expired',
    );
  });
});
