import type { ComposeOption } from 'echarts/core';
import type { BarSeriesOption } from 'echarts/charts';
import type {
  GridComponentOption,
  LegendComponentOption,
  TitleComponentOption,
  TooltipComponentOption,
} from 'echarts/components';
import {
  CoverageStatus,
  type AccountBlotterSnapshot,
  type BucketBalanceView,
} from '../../generated/ficant/blotter/v1/model_pb';
import type { DecimalValue } from '../../generated/ficant/core/v1/common_pb';
import { decimalToPlotNumber, formatDecimalValue, formatUnitRef } from '../../codec';

export type ExposureChartOption = ComposeOption<
  | BarSeriesOption
  | GridComponentOption
  | LegendComponentOption
  | TitleComponentOption
  | TooltipComponentOption
>;

export type ExposureChartViewMode = 'dv01_coverage' | 'settlement_buckets';

export interface InstrumentExposurePoint {
  readonly instrumentCode: string;
  readonly coverageStatus: CoverageStatus;
  readonly isCovered: boolean;
  readonly isUncovered: boolean;
  readonly preTaxDv01Plot: number | null;
  readonly preTaxDv01Text: string;
  readonly economicNetNotionalPlot: number;
  readonly economicNetNotionalText: string;
  readonly availableT0Plot: number;
  readonly availableT1Plot: number;
  readonly holdGrossT0Plot: number;
  readonly holdGrossT1Plot: number;
  readonly committedUnreconciledT0Plot: number;
  readonly committedUnreconciledT1Plot: number;
  readonly pendingMatchT0Plot: number;
  readonly pendingMatchT1Plot: number;
}

export interface CoverageBreakdownSummary {
  readonly coveredInstrumentCount: number;
  readonly uncoveredInstrumentCount: number;
  readonly coveredNetPreTaxDv01Text: string;
  readonly coveredNetPreTaxDv01UnitText: string;
  readonly coveredGrossNotionalText: string;
  readonly uncoveredGrossNotionalText: string;
  readonly uncoveredNetNotionalText: string;
  readonly notionalUnitText: string;
  readonly wholeAccountHedgeAllowed: boolean;
}

export interface BucketAggregatePoint {
  readonly label: string;
  readonly availablePlot: number;
  readonly holdPlot: number;
  readonly committedUnreconciledPlot: number;
  readonly pendingMatchPlot: number;
}

export interface ExposureChartModel {
  readonly accountId: string;
  readonly snapshotSeq: bigint;
  readonly snapshotSeqText: string;
  readonly instruments: readonly InstrumentExposurePoint[];
  readonly buckets: readonly [BucketAggregatePoint, BucketAggregatePoint];
  readonly coverage: CoverageBreakdownSummary;
}

function plotOrZero(value: DecimalValue | undefined): number {
  return decimalToPlotNumber(value) ?? 0;
}

function sumBucketPair(
  buy: DecimalValue | undefined,
  sell: DecimalValue | undefined,
): number {
  return plotOrZero(buy) + plotOrZero(sell);
}

function readBucketMetrics(bucket: BucketBalanceView | undefined): {
  readonly available: number;
  readonly hold: number;
  readonly committedUnreconciled: number;
  readonly pendingMatch: number;
} {
  if (!bucket) {
    return {
      available: 0,
      hold: 0,
      committedUnreconciled: 0,
      pendingMatch: 0,
    };
  }
  return {
    available: plotOrZero(bucket.availableNotional),
    hold: sumBucketPair(bucket.holdBuyNotional, bucket.holdSellNotional),
    committedUnreconciled: sumBucketPair(
      bucket.committedBuyUnreconciledNotional,
      bucket.committedSellUnreconciledNotional,
    ),
    pendingMatch: sumBucketPair(
      bucket.pendingMatchBuyUnresolvedNotional,
      bucket.pendingMatchSellUnresolvedNotional,
    ),
  };
}

function safeFormatDecimal(value: DecimalValue | undefined): string {
  if (value === undefined) {
    return '未提供';
  }
  try {
    return formatDecimalValue(value, { grouping: true });
  } catch {
    return '未提供';
  }
}

/**
 * Projects an `AccountBlotterSnapshot` into chart-ready exposure series and exact-decimal
 * coverage summary metadata. Strictly driven by snapshot data with zero hardcoded instruments,
 * products, or market constants.
 */
export function buildExposureChartModel(snapshot: AccountBlotterSnapshot): ExposureChartModel {
  let t0Available = 0;
  let t0Hold = 0;
  let t0Committed = 0;
  let t0Pending = 0;

  let t1Available = 0;
  let t1Hold = 0;
  let t1Committed = 0;
  let t1Pending = 0;

  const instruments: InstrumentExposurePoint[] = snapshot.positions.map((pos) => {
    const b0 = readBucketMetrics(pos.bucketT0);
    const b1 = readBucketMetrics(pos.bucketT1);

    const availableT0 =
      pos.availableT0Notional !== undefined ? plotOrZero(pos.availableT0Notional) : b0.available;
    const availableT1 =
      pos.availableT1Notional !== undefined ? plotOrZero(pos.availableT1Notional) : b1.available;

    t0Available += availableT0;
    t0Hold += b0.hold;
    t0Committed += b0.committedUnreconciled;
    t0Pending += b0.pendingMatch;

    t1Available += availableT1;
    t1Hold += b1.hold;
    t1Committed += b1.committedUnreconciled;
    t1Pending += b1.pendingMatch;

    const isCovered = pos.coverageStatus === CoverageStatus.COVERED_PRE_TAX;
    const isUncovered = pos.coverageStatus === CoverageStatus.UNCOVERED;

    return {
      instrumentCode: pos.instrumentCode,
      coverageStatus: pos.coverageStatus,
      isCovered,
      isUncovered,
      preTaxDv01Plot: isCovered ? decimalToPlotNumber(pos.preTaxDv01) : null,
      preTaxDv01Text: isCovered ? safeFormatDecimal(pos.preTaxDv01) : '未覆盖',
      economicNetNotionalPlot: plotOrZero(pos.economicNetNotional),
      economicNetNotionalText: safeFormatDecimal(pos.economicNetNotional),
      availableT0Plot: availableT0,
      availableT1Plot: availableT1,
      holdGrossT0Plot: b0.hold,
      holdGrossT1Plot: b1.hold,
      committedUnreconciledT0Plot: b0.committedUnreconciled,
      committedUnreconciledT1Plot: b1.committedUnreconciled,
      pendingMatchT0Plot: b0.pendingMatch,
      pendingMatchT1Plot: b1.pendingMatch,
    };
  });

  const risk = snapshot.riskSummary;
  const units = snapshot.openingBase?.units;

  const coverage: CoverageBreakdownSummary = {
    coveredInstrumentCount: risk?.coveredInstrumentCount ?? 0,
    uncoveredInstrumentCount: risk?.uncoveredInstrumentCount ?? 0,
    coveredNetPreTaxDv01Text: safeFormatDecimal(risk?.coveredNetPreTaxDv01),
    coveredNetPreTaxDv01UnitText: formatUnitRef(
      risk?.coveredNetPreTaxDv01?.unit ?? units?.dv01,
    ),
    coveredGrossNotionalText: safeFormatDecimal(risk?.coveredGrossNotional),
    uncoveredGrossNotionalText: safeFormatDecimal(risk?.uncoveredGrossNotional),
    uncoveredNetNotionalText: safeFormatDecimal(risk?.uncoveredNetNotional),
    notionalUnitText: formatUnitRef(
      risk?.coveredGrossNotional?.unit ??
        risk?.uncoveredGrossNotional?.unit ??
        units?.currencyAmount,
    ),
    wholeAccountHedgeAllowed:
      risk !== undefined && risk.uncoveredInstrumentCount === 0 && risk.wholeAccountHedgeAllowed,
  };

  return {
    accountId: snapshot.accountId,
    snapshotSeq: snapshot.snapshotSeq,
    snapshotSeqText: snapshot.snapshotSeq.toString(),
    instruments,
    buckets: [
      {
        label: 'T+0',
        availablePlot: t0Available,
        holdPlot: t0Hold,
        committedUnreconciledPlot: t0Committed,
        pendingMatchPlot: t0Pending,
      },
      {
        label: 'T+1',
        availablePlot: t1Available,
        holdPlot: t1Hold,
        committedUnreconciledPlot: t1Committed,
        pendingMatchPlot: t1Pending,
      },
    ],
    coverage,
  };
}

/**
 * Constructs a pure ECharts v5 option object from `ExposureChartModel`.
 */
export function buildExposureChartOption(
  model: ExposureChartModel,
  viewMode: ExposureChartViewMode,
): ExposureChartOption {
  if (viewMode === 'settlement_buckets') {
    const categories = model.buckets.map((b) => b.label);
    return {
      animation: false,
      title: {
        text: `结算分桶敞口 (${model.accountId} · seq=${model.snapshotSeqText})`,
        left: 'left',
        textStyle: { fontSize: 13 },
      },
      tooltip: {
        trigger: 'axis',
        axisPointer: { type: 'shadow' },
      },
      legend: {
        bottom: 0,
      },
      grid: {
        left: 56,
        right: 24,
        top: 44,
        bottom: 42,
      },
      xAxis: {
        type: 'category',
        data: categories,
      },
      yAxis: {
        type: 'value',
        name: '面额',
      },
      series: [
        {
          name: '可用面额 (Available)',
          type: 'bar',
          data: model.buckets.map((b) => b.availablePlot),
        },
        {
          name: '占仓 HOLD',
          type: 'bar',
          data: model.buckets.map((b) => b.holdPlot),
        },
        {
          name: '未核销 COMMITTED',
          type: 'bar',
          data: model.buckets.map((b) => b.committedUnreconciledPlot),
        },
        {
          name: '待核销 PENDING_MATCH',
          type: 'bar',
          data: model.buckets.map((b) => b.pendingMatchPlot),
        },
      ],
    };
  }

  const categories = model.instruments.map((p) => p.instrumentCode);
  const coveredDv01Series = model.instruments.map((p) => (p.isCovered ? p.preTaxDv01Plot : null));
  const uncoveredNotionalSeries = model.instruments.map((p) =>
    p.isUncovered ? p.economicNetNotionalPlot : null,
  );

  return {
    animation: false,
    title: {
      text: `因子敞口与覆盖分布 (${model.accountId} · seq=${model.snapshotSeqText})`,
      left: 'left',
      textStyle: { fontSize: 13 },
    },
    tooltip: {
      trigger: 'axis',
      axisPointer: { type: 'shadow' },
    },
    legend: {
      bottom: 0,
    },
    grid: {
      left: 56,
      right: 64,
      top: 44,
      bottom: 42,
    },
    xAxis: {
      type: 'category',
      data: categories,
    },
    yAxis: [
      {
        type: 'value',
        name: 'PRE_TAX DV01',
      },
      {
        type: 'value',
        name: '未覆盖净面额',
      },
    ],
    series: [
      {
        name: '已覆盖 PRE_TAX DV01',
        type: 'bar',
        yAxisIndex: 0,
        data: coveredDv01Series,
      },
      {
        name: '未覆盖经济净面额',
        type: 'bar',
        yAxisIndex: 1,
        data: uncoveredNotionalSeries,
      },
    ],
  };
}
