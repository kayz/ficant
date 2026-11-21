import React, { useEffect, useMemo, useRef, useState } from 'react';
import * as echarts from 'echarts/core';
import { BarChart } from 'echarts/charts';
import {
  GridComponent,
  LegendComponent,
  TitleComponent,
  TooltipComponent,
} from 'echarts/components';
import { CanvasRenderer, SVGRenderer } from 'echarts/renderers';
import { useStore } from 'zustand';
import {
  selectAccountSnapshot,
  selectSelectedSnapshot,
  type BlotterStoreApi,
} from '../../store';
import {
  buildExposureChartModel,
  buildExposureChartOption,
  type ExposureChartOption,
  type ExposureChartViewMode,
} from './exposureData';

echarts.use([
  BarChart,
  GridComponent,
  LegendComponent,
  TitleComponent,
  TooltipComponent,
  SVGRenderer,
  CanvasRenderer,
]);

// In jsdom (unit tests without native node-canvas), zrender's default measureText invokes
// HTMLCanvasElement.prototype.getContext('2d') and prints noisy stderr warnings. Provide a
// deterministic fallback ONLY under jsdom; real browsers keep native text measurement.
if (typeof navigator !== 'undefined' && /jsdom/i.test(navigator.userAgent)) {
  echarts.setPlatformAPI({
    measureText: (text: string) => ({ width: text.length * 8 }),
  });
}

export interface EChartsInstanceLike {
  setOption(option: ExposureChartOption, opts?: { readonly notMerge?: boolean }): void;
  resize(opts?: { readonly width?: number; readonly height?: number }): void;
  dispose(): void;
  isDisposed?(): boolean;
}

export type EChartsInitFactory = (
  container: HTMLElement,
  opts: {
    readonly renderer: 'svg' | 'canvas';
    readonly width?: number | undefined;
    readonly height?: number | undefined;
  },
) => EChartsInstanceLike;

export interface ExposureChartPanelProps {
  readonly store: BlotterStoreApi;
  readonly accountId?: string | undefined;
  readonly heightPx?: number | undefined;
  readonly renderer?: 'svg' | 'canvas' | undefined;
  readonly initialViewMode?: ExposureChartViewMode | undefined;
  readonly echartsFactory?: EChartsInitFactory | undefined;
}

const DEFAULT_FALLBACK_WIDTH_PX = 640;
const DEFAULT_CHART_HEIGHT_PX = 280;

const defaultEChartsFactory: EChartsInitFactory = (container, opts) => {
  const initOpts: {
    renderer: 'svg' | 'canvas';
    width?: number;
    height?: number;
  } = {
    renderer: opts.renderer,
  };
  if (opts.width !== undefined) {
    initOpts.width = opts.width;
  }
  if (opts.height !== undefined) {
    initOpts.height = opts.height;
  }
  return echarts.init(container, undefined, initOpts) as unknown as EChartsInstanceLike;
};

export function ExposureChartPanel(props: ExposureChartPanelProps): React.JSX.Element {
  const {
    store,
    accountId: explicitAccountId,
    heightPx = DEFAULT_CHART_HEIGHT_PX,
    renderer = 'svg',
    initialViewMode = 'dv01_coverage',
    echartsFactory = defaultEChartsFactory,
  } = props;

  // Strictly consume the exact same published store snapshot as TopBar and PositionsGrid.
  const snapshot = useStore(
    store,
    explicitAccountId !== undefined
      ? (state) => selectAccountSnapshot(state, explicitAccountId)
      : selectSelectedSnapshot,
  );

  const [viewMode, setViewMode] = useState<ExposureChartViewMode>(initialViewMode);
  const [chartFallbackMessage, setChartFallbackMessage] = useState<string | null>(null);

  const chartContainerRef = useRef<HTMLDivElement | null>(null);
  const chartInstanceRef = useRef<EChartsInstanceLike | null>(null);

  const model = useMemo(
    () => (snapshot !== undefined ? buildExposureChartModel(snapshot) : null),
    [snapshot],
  );

  const chartOption = useMemo(
    () => (model !== null ? buildExposureChartOption(model, viewMode) : null),
    [model, viewMode],
  );

  const hasSnapshot = snapshot !== undefined;

  // Initialize ECharts instance + ResizeObserver when container mounts with an active snapshot,
  // and cleanly dispose / disconnect on unmount or snapshot removal.
  useEffect(() => {
    if (!hasSnapshot) {
      if (chartInstanceRef.current !== null) {
        try {
          chartInstanceRef.current.dispose();
        } catch {
          // Ignore disposal errors in restricted DOM environments.
        }
        chartInstanceRef.current = null;
      }
      setChartFallbackMessage(null);
      return;
    }

    const container = chartContainerRef.current;
    if (!container) {
      return;
    }

    let instance: EChartsInstanceLike | null = null;
    try {
      const measuredWidth = container.clientWidth;
      const measuredHeight = container.clientHeight;
      instance = echartsFactory(container, {
        renderer,
        width: measuredWidth > 0 ? measuredWidth : DEFAULT_FALLBACK_WIDTH_PX,
        height: measuredHeight > 0 ? measuredHeight : heightPx,
      });
      chartInstanceRef.current = instance;
      setChartFallbackMessage(null);
    } catch (err) {
      chartInstanceRef.current = null;
      const detail = err instanceof Error ? err.message : 'renderer unavailable';
      setChartFallbackMessage(`图表渲染器降级为摘要模式 (${detail})`);
      return;
    }

    let observer: ResizeObserver | null = null;
    if (typeof ResizeObserver !== 'undefined') {
      try {
        observer = new ResizeObserver((entries) => {
          const active = chartInstanceRef.current;
          if (!active || (typeof active.isDisposed === 'function' && active.isDisposed())) {
            return;
          }
          const entry = entries[0];
          const nextW = entry ? Math.floor(entry.contentRect.width) : container.clientWidth;
          const nextH = entry ? Math.floor(entry.contentRect.height) : container.clientHeight;
          if (nextW > 0 && nextH > 0) {
            try {
              active.resize({ width: nextW, height: nextH });
            } catch {
              // Guard against unhandled resize errors in headless/jsdom environments.
            }
          }
        });
        observer.observe(container);
      } catch {
        observer = null;
      }
    }

    return () => {
      if (observer !== null) {
        try {
          observer.disconnect();
        } catch {
          // Ignore disconnect errors on teardown.
        }
      }
      if (chartInstanceRef.current !== null) {
        try {
          chartInstanceRef.current.dispose();
        } catch {
          // Ignore dispose errors on teardown.
        }
        chartInstanceRef.current = null;
      }
    };
  }, [hasSnapshot, echartsFactory, heightPx, renderer]);

  // Apply updated ECharts option whenever the published snapshot or view mode changes.
  useEffect(() => {
    const active = chartInstanceRef.current;
    if (!active || !chartOption) {
      return;
    }
    if (typeof active.isDisposed === 'function' && active.isDisposed()) {
      return;
    }
    try {
      active.setOption(chartOption, { notMerge: true });
    } catch (err) {
      const detail = err instanceof Error ? err.message : 'setOption failed';
      setChartFallbackMessage(`图表更新降级为摘要模式 (${detail})`);
    }
  }, [chartOption]);

  return (
    <section
      aria-label="敞口分布图"
      data-testid="exposure-chart-panel"
      data-snapshot-seq={snapshot ? snapshot.snapshotSeq.toString() : undefined}
      data-account-id={snapshot?.accountId}
    >
      <header>
        <div>
          <h3>盘中敞口与覆盖分布</h3>
          {model ? (
            <p data-testid="exposure-chart-meta">
              账户: <strong>{model.accountId}</strong>
              {' · '}
              snapshot_seq: <code data-testid="exposure-chart-seq">{model.snapshotSeqText}</code>
            </p>
          ) : (
            <p data-testid="exposure-chart-empty">尚未加载账户快照</p>
          )}
        </div>

        {model ? (
          <div role="group" aria-label="敞口图表视图切换">
            <button
              type="button"
              data-testid="exposure-mode-dv01"
              aria-pressed={viewMode === 'dv01_coverage'}
              onClick={() => setViewMode('dv01_coverage')}
            >
              因子 DV01 / 覆盖分布
            </button>
            <button
              type="button"
              data-testid="exposure-mode-buckets"
              aria-pressed={viewMode === 'settlement_buckets'}
              onClick={() => setViewMode('settlement_buckets')}
            >
              T+0 / T+1 结算分桶
            </button>
          </div>
        ) : null}
      </header>

      {model ? (
        <>
          <div data-testid="exposure-coverage-breakdown">
            <span data-testid="exposure-covered-count">
              已覆盖券数: {model.coverage.coveredInstrumentCount}
            </span>
            {' | '}
            <span data-testid="exposure-covered-dv01">
              已覆盖 PRE_TAX DV01: {model.coverage.coveredNetPreTaxDv01Text} (
              {model.coverage.coveredNetPreTaxDv01UnitText})
            </span>
            {' | '}
            <span data-testid="exposure-uncovered-count">
              未覆盖券数: {model.coverage.uncoveredInstrumentCount}
            </span>
            {' | '}
            <span data-testid="exposure-uncovered-gross">
              未覆盖总面额: {model.coverage.uncoveredGrossNotionalText}
            </span>
            {' | '}
            <span data-testid="exposure-uncovered-net">
              未覆盖净面额: {model.coverage.uncoveredNetNotionalText} (
              {model.coverage.notionalUnitText})
            </span>
          </div>

          {chartFallbackMessage ? (
            <div role="status" data-testid="exposure-chart-fallback">
              {chartFallbackMessage}
            </div>
          ) : null}

          <div
            ref={chartContainerRef}
            data-testid="exposure-chart-canvas-host"
            data-renderer={renderer}
            data-view-mode={viewMode}
            style={{ width: '100%', height: `${heightPx}px` }}
          />
        </>
      ) : null}
    </section>
  );
}
