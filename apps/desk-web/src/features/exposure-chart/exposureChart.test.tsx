import { create } from '@bufbuild/protobuf';
import { act, cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  AccountBlotterSnapshotSchema,
  AccountChangeSchema,
  BlotterEventEnvelopeSchema,
  BucketBalanceViewSchema,
  CoverageStatus,
  InstrumentPositionViewSchema,
  OpeningPositionBaseSchema,
  PrincipalRole,
  RiskAndCoverageSummarySchema,
  SettlementBucket,
  UncoveredReason,
  type AccountBlotterSnapshot,
} from '../../generated/ficant/blotter/v1/model_pb';
import { AnalysisUnitsSchema, BondTaxBasis } from '../../generated/ficant/rates/v1/analytics_pb';
import { createDecimalValue, createUnitRef } from '../../codec';
import { createBlotterStore, selectSelectedSnapshot } from '../../store';
import {
  ExposureChartPanel,
  buildExposureChartModel,
  buildExposureChartOption,
  type EChartsInitFactory,
  type EChartsInstanceLike,
  type ExposureChartOption,
} from './index';

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

function makeRichSnapshot(accountId: string, seq: bigint, dv01Value = '18450.25'): AccountBlotterSnapshot {
  const units = makeUnits();
  const ccy = units.currencyAmount!;
  const dv01 = units.dv01!;

  return create(AccountBlotterSnapshotSchema, {
    snapshotSeq: seq,
    accountId,
    accountVersion: 1n,
    openingBase: create(OpeningPositionBaseSchema, {
      accountId,
      units,
    }),
    riskSummary: create(RiskAndCoverageSummarySchema, {
      coveredInstrumentCount: 1,
      uncoveredInstrumentCount: 1,
      coveredTaxBasis: BondTaxBasis.PRE_TAX,
      coveredNetPreTaxDv01: createDecimalValue(dv01Value, dv01),
      coveredGrossNotional: createDecimalValue('50000000', ccy),
      uncoveredGrossNotional: createDecimalValue('20000000', ccy),
      uncoveredNetNotional: createDecimalValue('-20000000', ccy),
      wholeAccountHedgeAllowed: false,
    }),
    positions: [
      create(InstrumentPositionViewSchema, {
        accountId,
        instrumentCode: '240011.IB',
        positionVersion: 1n,
        economicNetNotional: createDecimalValue('50000000', ccy),
        availableT0Notional: createDecimalValue('40000000', ccy),
        availableT1Notional: createDecimalValue('45000000', ccy),
        bucketT0: create(BucketBalanceViewSchema, {
          settlementBucket: SettlementBucket.T_PLUS_0,
          availableNotional: createDecimalValue('40000000', ccy),
          holdBuyNotional: createDecimalValue('5000000', ccy),
          holdSellNotional: createDecimalValue('0', ccy),
          committedBuyUnreconciledNotional: createDecimalValue('3000000', ccy),
          committedSellUnreconciledNotional: createDecimalValue('0', ccy),
          pendingMatchBuyUnresolvedNotional: createDecimalValue('2000000', ccy),
          pendingMatchSellUnresolvedNotional: createDecimalValue('0', ccy),
        }),
        bucketT1: create(BucketBalanceViewSchema, {
          settlementBucket: SettlementBucket.T_PLUS_1,
          availableNotional: createDecimalValue('45000000', ccy),
          holdBuyNotional: createDecimalValue('0', ccy),
          holdSellNotional: createDecimalValue('2500000', ccy),
          committedBuyUnreconciledNotional: createDecimalValue('0', ccy),
          committedSellUnreconciledNotional: createDecimalValue('1500000', ccy),
          pendingMatchBuyUnresolvedNotional: createDecimalValue('0', ccy),
          pendingMatchSellUnresolvedNotional: createDecimalValue('1000000', ccy),
        }),
        coverageStatus: CoverageStatus.COVERED_PRE_TAX,
        taxBasis: BondTaxBasis.PRE_TAX,
        preTaxDv01: createDecimalValue(dv01Value, dv01),
      }),
      create(InstrumentPositionViewSchema, {
        accountId,
        instrumentCode: 'CORP-SHORT-01',
        positionVersion: 2n,
        economicNetNotional: createDecimalValue('-20000000', ccy),
        availableT0Notional: createDecimalValue('0', ccy),
        availableT1Notional: createDecimalValue('0', ccy),
        coverageStatus: CoverageStatus.UNCOVERED,
        taxBasis: BondTaxBasis.PRE_TAX,
        uncoveredReason: UncoveredReason.CREDIT_OR_NON_RATES_ISSUER,
      }),
    ],
  });
}

describe('ExposureChartPanel (T-006 / AC-008)', () => {
  it('syncs data-snapshot-seq with the exact selected store snapshot across updates and account switches', () => {
    const store = createBlotterStore();
    const setOptionSpy = vi.fn<(opt: ExposureChartOption) => void>();
    const resizeSpy = vi.fn();
    const disposeSpy = vi.fn();
    let disposed = false;

    const mockFactory: EChartsInitFactory = vi.fn(() => {
      disposed = false;
      const inst: EChartsInstanceLike = {
        setOption: setOptionSpy,
        resize: resizeSpy,
        dispose: () => {
          disposed = true;
          disposeSpy();
        },
        isDisposed: () => disposed,
      };
      return inst;
    });

    const { unmount } = render(
      <ExposureChartPanel store={store} echartsFactory={mockFactory} />,
    );

    // Empty session -> no data-snapshot-seq attribute
    const root = screen.getByTestId('exposure-chart-panel');
    expect(root.getAttribute('data-snapshot-seq')).toBeNull();
    expect(screen.getByTestId('exposure-chart-empty')).toBeTruthy();
    expect(mockFactory).not.toHaveBeenCalled();

    // Commit baseline for acct-A@20 and acct-B@25
    const sessionGen = store.getState().beginSession({
      headerMode: 'authorization',
      roleHint: PrincipalRole.TRADER,
      actorLabel: 'trader-1',
    });
    store.getState().setRequestedAccounts(['acct-A', 'acct-B'], sessionGen);
    const fence = store.getState().activateReader(sessionGen, 'stream', 'stream_live')!;

    act(() => {
      store.getState().commitBaselineSnapshots({
        sessionGeneration: fence.sessionGeneration,
        transportEpoch: fence.transportEpoch,
        readerId: fence.readerId,
        snapshots: new Map([
          ['acct-A', makeRichSnapshot('acct-A', 20n, '18450.25')],
          ['acct-B', makeRichSnapshot('acct-B', 25n, '-9200.5')],
        ]),
        cursorSeq: 25n,
        headSeq: 25n,
        nextConnectionState: 'stream_live',
      });
    });

    const activeSnap = selectSelectedSnapshot(store.getState());
    expect(activeSnap?.accountId).toBe('acct-A');
    expect(root.getAttribute('data-snapshot-seq')).toBe('20');
    expect(root.getAttribute('data-account-id')).toBe('acct-A');
    expect(screen.getByTestId('exposure-covered-dv01').textContent).toContain('18,450.25');
    expect(screen.getByTestId('exposure-uncovered-gross').textContent).toContain('20,000,000');
    expect(screen.getByTestId('exposure-uncovered-net').textContent).toContain('-20,000,000');
    expect(mockFactory).toHaveBeenCalledTimes(1);
    expect(setOptionSpy).toHaveBeenCalledTimes(1);

    // Stream advances acct-A to seq=26 -> data-snapshot-seq and chart option update without re-init
    act(() => {
      store.getState().commitStreamEnvelope({
        sessionGeneration: fence.sessionGeneration,
        transportEpoch: fence.transportEpoch,
        readerId: fence.readerId,
        envelope: create(BlotterEventEnvelopeSchema, {
          seq: 26n,
          eventType: 'quote_updated',
          changes: [
            create(AccountChangeSchema, {
              accountId: 'acct-A',
              replaceSnapshot: makeRichSnapshot('acct-A', 26n, '19100.75'),
            }),
          ],
        }),
      });
    });

    expect(root.getAttribute('data-snapshot-seq')).toBe('26');
    expect(screen.getByTestId('exposure-covered-dv01').textContent).toContain('19,100.75');
    expect(mockFactory).toHaveBeenCalledTimes(1);
    expect(setOptionSpy).toHaveBeenCalledTimes(2);

    // Switch selected account to acct-B -> data-snapshot-seq immediately reflects acct-B@25
    act(() => {
      store.getState().setSelectedAccountId('acct-B');
    });
    expect(root.getAttribute('data-snapshot-seq')).toBe('25');
    expect(root.getAttribute('data-account-id')).toBe('acct-B');
    expect(screen.getByTestId('exposure-covered-dv01').textContent).toContain('-9,200.5');

    unmount();
    expect(disposeSpy).toHaveBeenCalledTimes(1);
  });

  it('wires ResizeObserver cleanly, toggles bucket/factor views, and disposes on unmount', async () => {
    const user = userEvent.setup();
    const store = createBlotterStore();
    const sessionGen = store.getState().beginSession({
      headerMode: 'authorization',
      roleHint: PrincipalRole.TRADER,
    });
    const fence = store.getState().activateReader(sessionGen, 'stream', 'stream_live')!;
    store.getState().commitBaselineSnapshots({
      sessionGeneration: fence.sessionGeneration,
      transportEpoch: fence.transportEpoch,
      readerId: fence.readerId,
      snapshots: new Map([['acct-A', makeRichSnapshot('acct-A', 30n)]]),
      cursorSeq: 30n,
      headSeq: 30n,
      nextConnectionState: 'stream_live',
    });

    let resizeCallback: ResizeObserverCallback | null = null;
    const observeSpy = vi.fn();
    const disconnectSpy = vi.fn();
    const OriginalResizeObserver = globalThis.ResizeObserver;

    class MockResizeObserver {
      constructor(cb: ResizeObserverCallback) {
        resizeCallback = cb;
      }
      observe = observeSpy;
      unobserve = vi.fn();
      disconnect = disconnectSpy;
    }

    globalThis.ResizeObserver = MockResizeObserver as unknown as typeof ResizeObserver;

    const setOptionSpy = vi.fn<(opt: ExposureChartOption) => void>();
    const resizeSpy = vi.fn();
    const disposeSpy = vi.fn();
    const mockFactory: EChartsInitFactory = vi.fn(() => ({
      setOption: setOptionSpy,
      resize: resizeSpy,
      dispose: disposeSpy,
      isDisposed: () => false,
    }));

    try {
      const { unmount } = render(
        <ExposureChartPanel store={store} echartsFactory={mockFactory} />,
      );

      expect(observeSpy).toHaveBeenCalledTimes(1);
      expect(resizeCallback).not.toBeNull();

      // Trigger resize observer callback
      act(() => {
        resizeCallback!(
          [
            {
              contentRect: { width: 820, height: 320 } as DOMRectReadOnly,
            } as ResizeObserverEntry,
          ],
          {} as ResizeObserver,
        );
      });
      expect(resizeSpy).toHaveBeenCalledWith({ width: 820, height: 320 });

      // Switch to settlement_buckets view mode
      await user.click(screen.getByTestId('exposure-mode-buckets'));
      const lastOption = setOptionSpy.mock.calls.at(-1)?.[0];
      expect(lastOption).toBeDefined();
      expect(screen.getByTestId('exposure-chart-canvas-host').getAttribute('data-view-mode')).toBe(
        'settlement_buckets',
      );

      unmount();
      expect(disconnectSpy).toHaveBeenCalledTimes(1);
      expect(disposeSpy).toHaveBeenCalledTimes(1);
    } finally {
      globalThis.ResizeObserver = OriginalResizeObserver;
    }
  });

  it('mounts real ECharts v5 SVG renderer in jsdom without unhandled exceptions and degrades gracefully if renderer throws', () => {
    const store = createBlotterStore();
    const sessionGen = store.getState().beginSession({
      headerMode: 'authorization',
      roleHint: PrincipalRole.TRADER,
    });
    const fence = store.getState().activateReader(sessionGen, 'stream', 'stream_live')!;
    store.getState().commitBaselineSnapshots({
      sessionGeneration: fence.sessionGeneration,
      transportEpoch: fence.transportEpoch,
      readerId: fence.readerId,
      snapshots: new Map([['acct-svg', makeRichSnapshot('acct-svg', 40n)]]),
      cursorSeq: 40n,
      headSeq: 40n,
      nextConnectionState: 'stream_live',
    });

    // Real ECharts SVG mount + unmount in jsdom
    const { unmount } = render(<ExposureChartPanel store={store} renderer="svg" />);
    const host = screen.getByTestId('exposure-chart-canvas-host');
    expect(host.querySelector('svg')).not.toBeNull();
    unmount();

    // Graceful fallback if renderer init throws (e.g. broken canvas environment)
    const throwingFactory: EChartsInitFactory = () => {
      throw new Error('canvas context unavailable');
    };
    render(<ExposureChartPanel store={store} echartsFactory={throwingFactory} />);
    expect(screen.getByTestId('exposure-chart-fallback').textContent).toContain(
      'canvas context unavailable',
    );
    expect(screen.getByTestId('exposure-chart-panel').getAttribute('data-snapshot-seq')).toBe('40');
  });

  it('builds dynamic bucket/factor exposure options strictly from snapshot data', () => {
    const snap = makeRichSnapshot('acct-model', 99n, '22000');
    const model = buildExposureChartModel(snap);

    expect(model.instruments.map((i) => i.instrumentCode)).toEqual(['240011.IB', 'CORP-SHORT-01']);
    expect(model.buckets[0].label).toBe('T+0');
    expect(model.buckets[0].availablePlot).toBe(40_000_000);
    expect(model.buckets[0].holdPlot).toBe(5_000_000);
    expect(model.buckets[0].committedUnreconciledPlot).toBe(3_000_000);
    expect(model.buckets[0].pendingMatchPlot).toBe(2_000_000);
    expect(model.buckets[1].label).toBe('T+1');
    expect(model.buckets[1].availablePlot).toBe(45_000_000);
    expect(model.buckets[1].holdPlot).toBe(2_500_000);

    const dv01Opt = buildExposureChartOption(model, 'dv01_coverage');
    expect(Array.isArray(dv01Opt.series)).toBe(true);
    const bucketOpt = buildExposureChartOption(model, 'settlement_buckets');
    expect(Array.isArray(bucketOpt.series)).toBe(true);
  });
});
