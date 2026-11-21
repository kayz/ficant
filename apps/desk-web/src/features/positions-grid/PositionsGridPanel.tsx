import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  useReactTable,
} from '@tanstack/react-table';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useCallback, useEffect, useMemo, useRef, useState, type ReactElement } from 'react';
import { useStore } from 'zustand';
import {
  CoverageStatus,
  EarmarkState,
  FeedHealthState,
  UncoveredReason,
  type EarmarkRecord,
  type OpeningInstrumentBalance,
} from '../../generated/ficant/blotter/v1/model_pb';
import { formatDecimalValue } from '../../codec/decimal';
import type { WriteReceiptRecord } from '../../store/blotterStore';
import { BlotterClientError } from '../../transport/error';
import { WriteIntentManager } from '../../transport/idempotency';
import { BucketBalanceStrip } from './BucketBalanceStrip';
import {
  formatCalendarResolution,
  formatCoverageStatus,
  formatEarmarkState,
  formatHoldRemainingTtl,
  formatMarketTimeDisplay,
  formatSettlementBucket,
  formatTaxBasis,
  formatTradeSide,
  formatUncoveredReason,
} from './formatters';
import { HoldActionForm } from './HoldActionForm';
import { ReceiptsDrawer } from './ReceiptsDrawer';
import type {
  HoldActionMode,
  PositionRowModel,
  PositionsGridPanelProps,
  ReceiptReplayHandle,
} from './types';

const DEFAULT_VIEWPORT_HEIGHT_PX = 448;
const DEFAULT_ROW_HEIGHT_PX = 84;
const DEFAULT_OVERSCAN = 4;

const columnHelper = createColumnHelper<PositionRowModel>();

type CoverageFilter = 'all' | 'covered' | 'uncovered';

export function PositionsGridPanel({
  store,
  client,
  writeIntents: providedWriteIntents,
  accountId: accountIdOverride,
  onWriteCommitted,
  onResyncRequested,
  nowMs: providedNowMs,
  viewportHeightPx = DEFAULT_VIEWPORT_HEIGHT_PX,
  rowHeightPx = DEFAULT_ROW_HEIGHT_PX,
  overscan = DEFAULT_OVERSCAN,
}: PositionsGridPanelProps): ReactElement {
  const sessionGeneration = useStore(store, (s) => s.sessionGeneration);
  const authenticated = useStore(store, (s) => s.authenticated);
  const roleHint = useStore(store, (s) => s.roleHint);
  const recoveryPausedWrites = useStore(store, (s) => s.recoveryPausedWrites);
  const recoveryReason = useStore(store, (s) => s.recoveryReason);
  const selectedAccountId = useStore(store, (s) => s.selectedAccountId);
  const snapshots = useStore(store, (s) => s.snapshots);
  const writeReceipts = useStore(store, (s) => s.writeReceipts);

  const effectiveAccountId =
    accountIdOverride !== undefined ? accountIdOverride : selectedAccountId;
  const snapshot = effectiveAccountId !== null ? snapshots[effectiveAccountId] : undefined;

  const fallbackIntentsRef = useRef<WriteIntentManager | null>(null);
  if (fallbackIntentsRef.current === null) {
    fallbackIntentsRef.current = new WriteIntentManager();
  }
  const writeIntents = providedWriteIntents ?? fallbackIntentsRef.current;

  const nowMs = useCallback((): number => (providedNowMs ? providedNowMs() : Date.now()), [
    providedNowMs,
  ]);

  const receiptCounterRef = useRef<number>(0);
  const nextReceiptId = useCallback((gen: number): string => {
    receiptCounterRef.current += 1;
    return `rcpt-g${gen}-${receiptCounterRef.current}`;
  }, []);

  const [searchQuery, setSearchQuery] = useState<string>('');
  const [coverageFilter, setCoverageFilter] = useState<CoverageFilter>('all');
  const [onlyWithActiveEarmarks, setOnlyWithActiveEarmarks] = useState<boolean>(false);
  const [selectedInstrumentCode, setSelectedInstrumentCode] = useState<string | null>(null);
  const [selectedEarmarkId, setSelectedEarmarkId] = useState<string | null>(null);
  const [activeMode, setActiveMode] = useState<HoldActionMode>('create_hold');
  const [replayHandles, setReplayHandles] = useState<Readonly<Record<string, ReceiptReplayHandle>>>(
    {},
  );
  const [replayingReceiptId, setReplayingReceiptId] = useState<string | null>(null);

  // Synchronously wipe session-scoped replay handles and local intents on session switch.
  useEffect(() => {
    writeIntents.clearForSessionSwitch(sessionGeneration);
    setReplayHandles({});
    setSelectedInstrumentCode(null);
    setSelectedEarmarkId(null);
    setActiveMode('create_hold');
  }, [sessionGeneration, writeIntents]);

  const handleRegisterReplayHandle = useCallback((handle: ReceiptReplayHandle): void => {
    setReplayHandles((prev) => ({
      ...prev,
      [handle.receiptId]: handle,
    }));
  }, []);

  const rowModels = useMemo<readonly PositionRowModel[]>(() => {
    if (!snapshot) {
      return [];
    }

    const openingByInstrument = new Map<string, OpeningInstrumentBalance>();
    if (snapshot.openingBase) {
      for (const bal of snapshot.openingBase.balances) {
        openingByInstrument.set(bal.instrumentCode, bal);
      }
    }

    const earmarksByInstrument = new Map<string, EarmarkRecord[]>();
    for (const em of snapshot.activeEarmarks) {
      const list = earmarksByInstrument.get(em.instrumentCode);
      if (list) {
        list.push(em);
      } else {
        earmarksByInstrument.set(em.instrumentCode, [em]);
      }
    }

    const anyFeedStaleOrPaused = snapshot.feedStatuses.some(
      (f) => f.earmarksPaused || f.pausedByStaleness || f.healthState !== FeedHealthState.HEALTHY,
    );

    return snapshot.positions.map((position) => {
      const activeEarmarks = earmarksByInstrument.get(position.instrumentCode) ?? [];
      let activeHoldCount = 0;
      let activeCommittedCount = 0;
      let hasLifecycleAlert = false;
      for (const em of activeEarmarks) {
        if (em.state === EarmarkState.HOLD) {
          activeHoldCount += 1;
        } else if (em.state === EarmarkState.COMMITTED) {
          activeCommittedCount += 1;
        }
        if (em.overdueAlert || em.preCloseAlert || em.reopenedByReversal) {
          hasLifecycleAlert = true;
        }
      }

      return {
        position,
        openingBalance: openingByInstrument.get(position.instrumentCode),
        activeEarmarks,
        activeHoldCount,
        activeCommittedCount,
        hasLifecycleAlert,
        sellAmbiguityLockedT0: position.bucketT0?.sellAmbiguityLocked ?? false,
        sellAmbiguityLockedT1: position.bucketT1?.sellAmbiguityLocked ?? false,
        feedStaleOrPaused:
          anyFeedStaleOrPaused || position.uncoveredReason === UncoveredReason.STALE_QUOTE,
      };
    });
  }, [snapshot]);

  const filteredRows = useMemo<PositionRowModel[]>(() => {
    const q = searchQuery.trim().toLowerCase();
    return rowModels.filter((row) => {
      if (q.length > 0 && !row.position.instrumentCode.toLowerCase().includes(q)) {
        return false;
      }
      if (
        coverageFilter === 'covered' &&
        row.position.coverageStatus !== CoverageStatus.COVERED_PRE_TAX
      ) {
        return false;
      }
      if (
        coverageFilter === 'uncovered' &&
        row.position.coverageStatus !== CoverageStatus.UNCOVERED
      ) {
        return false;
      }
      if (onlyWithActiveEarmarks && row.activeEarmarks.length === 0) {
        return false;
      }
      return true;
    });
  }, [rowModels, searchQuery, coverageFilter, onlyWithActiveEarmarks]);

  const selectedPosition = useMemo(() => {
    if (!snapshot) {
      return undefined;
    }
    if (selectedInstrumentCode !== null) {
      const found = snapshot.positions.find((p) => p.instrumentCode === selectedInstrumentCode);
      if (found) {
        return found;
      }
    }
    return snapshot.positions[0];
  }, [snapshot, selectedInstrumentCode]);

  const selectedEarmark = useMemo(() => {
    if (!snapshot || selectedEarmarkId === null) {
      return undefined;
    }
    return snapshot.activeEarmarks.find((em) => em.earmarkId?.value === selectedEarmarkId);
  }, [snapshot, selectedEarmarkId]);

  const handleSelectEarmark = useCallback(
    (earmark: EarmarkRecord, preferredMode?: HoldActionMode): void => {
      const id = earmark.earmarkId?.value ?? null;
      setSelectedInstrumentCode(earmark.instrumentCode);
      setSelectedEarmarkId(id);
      if (preferredMode !== undefined) {
        setActiveMode(preferredMode);
      } else if (earmark.state === EarmarkState.HOLD) {
        setActiveMode('confirm_hold');
      } else if (earmark.state === EarmarkState.COMMITTED) {
        setActiveMode('cancel_committed');
      }
    },
    [],
  );

  const columns = useMemo(() => {
    return [
      columnHelper.accessor((row) => row.position.instrumentCode, {
        id: 'instrument',
        header: '券种 / 版本',
        cell: (info) => {
          const row = info.row.original;
          return (
            <div>
              <strong>{row.position.instrumentCode}</strong>
              <div style={{ fontSize: '11px', color: '#475569' }}>
                v{row.position.positionVersion.toString()}
                {row.openingBalance
                  ? ` · 期初: ${formatDecimalValue(row.openingBalance.openingEconomicNotional, { grouping: true })}`
                  : ''}
              </div>
            </div>
          );
        },
      }),
      columnHelper.accessor((row) => row.position.economicNetNotional, {
        id: 'notional',
        header: '经济净面额 / 可用量',
        cell: (info) => {
          const pos = info.row.original.position;
          return (
            <div style={{ fontSize: '12px' }}>
              <div>净额: {formatDecimalValue(pos.economicNetNotional, { grouping: true })}</div>
              <div style={{ fontSize: '11px', color: '#475569' }}>
                T+0 可用: {formatDecimalValue(pos.availableT0Notional, { grouping: true })} · T+1 可用:{' '}
                {formatDecimalValue(pos.availableT1Notional, { grouping: true })}
              </div>
            </div>
          );
        },
      }),
      columnHelper.display({
        id: 'buckets',
        header: '分桶占仓 / 核销 / 锁',
        cell: (info) => {
          const { position, sellAmbiguityLockedT0, sellAmbiguityLockedT1 } = info.row.original;
          const b0 = position.bucketT0;
          const b1 = position.bucketT1;
          return (
            <div style={{ fontSize: '11px' }}>
              <div>
                T+0 HOLD(买/卖): {formatDecimalValue(b0?.holdBuyNotional, { grouping: true })} /{' '}
                {formatDecimalValue(b0?.holdSellNotional, { grouping: true })} · 未核销:{' '}
                {formatDecimalValue(b0?.committedBuyUnreconciledNotional, { grouping: true })}/
                {formatDecimalValue(b0?.committedSellUnreconciledNotional, { grouping: true })}
              </div>
              <div>
                T+1 HOLD(买/卖): {formatDecimalValue(b1?.holdBuyNotional, { grouping: true })} /{' '}
                {formatDecimalValue(b1?.holdSellNotional, { grouping: true })} · 待核销:{' '}
                {formatDecimalValue(b1?.pendingMatchBuyUnresolvedNotional, { grouping: true })}/
                {formatDecimalValue(b1?.pendingMatchSellUnresolvedNotional, { grouping: true })}
              </div>
              {sellAmbiguityLockedT0 || sellAmbiguityLockedT1 ? (
                <span style={{ color: '#b91c1c', fontWeight: 600 }}>
                  sell_ambiguity_locked ({sellAmbiguityLockedT0 ? 'T+0' : ''}
                  {sellAmbiguityLockedT0 && sellAmbiguityLockedT1 ? '/' : ''}
                  {sellAmbiguityLockedT1 ? 'T+1' : ''})
                </span>
              ) : null}
            </div>
          );
        },
      }),
      columnHelper.display({
        id: 'risk_coverage',
        header: '风险与覆盖 (PRE_TAX / 未覆盖)',
        cell: (info) => {
          const pos = info.row.original.position;
          if (pos.coverageStatus === CoverageStatus.COVERED_PRE_TAX) {
            return (
              <div style={{ fontSize: '12px' }}>
                <div>
                  <strong>{formatTaxBasis(pos.taxBasis)}</strong> · DV01:{' '}
                  {formatDecimalValue(pos.preTaxDv01, { grouping: true })}
                </div>
                <div style={{ fontSize: '11px', color: '#475569' }}>
                  净价: {formatDecimalValue(pos.valuationCleanPricePer100)} · YTM:{' '}
                  {formatDecimalValue(pos.valuationYieldToMaturity)}
                </div>
              </div>
            );
          }
          if (pos.coverageStatus === CoverageStatus.UNCOVERED) {
            return (
              <div style={{ fontSize: '11px', color: '#b91c1c' }}>
                <div>
                  <strong>{formatCoverageStatus(pos.coverageStatus)}</strong>
                </div>
                <div>原因: {formatUncoveredReason(pos.uncoveredReason)}</div>
                {pos.uncoveredDetail ? <div>详情: {pos.uncoveredDetail}</div> : null}
              </div>
            );
          }
          return (
            <div style={{ fontSize: '11px', color: '#64748b' }}>
              {formatCoverageStatus(pos.coverageStatus)} ({formatTaxBasis(pos.taxBasis)})
            </div>
          );
        },
      }),
      columnHelper.display({
        id: 'quote_source',
        header: '报价源与陈旧度',
        cell: (info) => {
          const row = info.row.original;
          const pos = row.position;
          const isStale = pos.uncoveredReason === UncoveredReason.STALE_QUOTE;
          return (
            <div style={{ fontSize: '11px' }}>
              <div>
                源: {pos.activeQuoteSourceId || '无报价源'}
                {pos.activeQuoteSeq !== undefined ? ` #${pos.activeQuoteSeq.toString()}` : ''}
              </div>
              <div style={{ color: '#475569' }}>
                估值结算日: {pos.valuationSettlementDate || '—'} ·{' '}
                {formatCalendarResolution(pos.calendarResolution)}
              </div>
              {isStale ? (
                <span style={{ color: '#b91c1c', fontWeight: 600 }}>报价陈旧 (STALE_QUOTE)</span>
              ) : row.feedStaleOrPaused ? (
                <span style={{ color: '#b45309' }}>Feed 异常/暂停</span>
              ) : null}
            </div>
          );
        },
      }),
      columnHelper.display({
        id: 'lifecycle_earmarks',
        header: '活动占仓与生命周期',
        cell: (info) => {
          const row = info.row.original;
          if (row.activeEarmarks.length === 0) {
            return <span style={{ fontSize: '11px', color: '#64748b' }}>无活动占仓</span>;
          }
          return (
            <div style={{ display: 'flex', flexWrap: 'wrap', gap: '4px' }}>
              {row.activeEarmarks.map((em) => {
                const emId = em.earmarkId?.value ?? '';
                const ttlInfo =
                  em.state === EarmarkState.HOLD
                    ? formatHoldRemainingTtl(em.expiresAt, nowMs())
                    : null;
                const isSelectedEm = selectedEarmarkId === emId;

                return (
                  <button
                    key={emId}
                    type="button"
                    data-testid={`earmark-badge-${emId}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      handleSelectEarmark(em);
                    }}
                    style={{
                      fontSize: '11px',
                      padding: '2px 6px',
                      borderRadius: '4px',
                      border: isSelectedEm ? '2px solid #2563eb' : '1px solid #94a3b8',
                      backgroundColor: ttlInfo?.expired ? '#fef3c7' : '#f1f5f9',
                      cursor: 'pointer',
                      textAlign: 'left',
                    }}
                  >
                    <span>
                      {formatEarmarkState(em.state)} {formatTradeSide(em.side)}{' '}
                      {formatSettlementBucket(em.settlementBucket)}{' '}
                      {formatDecimalValue(em.notional, { grouping: true })} rev{' '}
                      {em.earmarkRevision.toString()}
                    </span>
                    {em.expiresAt ? (
                      <span>
                        {' '}
                        · {ttlInfo?.label ?? formatMarketTimeDisplay(em.expiresAt)}
                      </span>
                    ) : null}
                    {em.overdueAlert ? ' [逾期告警]' : ''}
                    {em.preCloseAlert ? ' [临近收市]' : ''}
                    {em.reopenedByReversal ? ' [冲正重开]' : ''}
                  </button>
                );
              })}
            </div>
          );
        },
      }),
    ];
  }, [handleSelectEarmark, nowMs, selectedEarmarkId]);

  const table = useReactTable({
    data: filteredRows,
    columns,
    getCoreRowModel: getCoreRowModel(),
    getRowId: (row) => row.position.instrumentCode,
  });

  const tableRows = table.getRowModel().rows;
  const scrollContainerRef = useRef<HTMLDivElement | null>(null);

  const rowVirtualizer = useVirtualizer({
    count: tableRows.length,
    getScrollElement: () => scrollContainerRef.current,
    estimateSize: () => rowHeightPx,
    overscan,
    useFlushSync: false,
    initialRect: { width: 1200, height: viewportHeightPx },
    observeElementRect: (instance, cb) => {
      const el = instance.scrollElement;
      if (!el) {
        return;
      }
      const measure = (): { width: number; height: number } => {
        const rect = el.getBoundingClientRect();
        const width = rect.width > 0 ? rect.width : 1200;
        const height =
          rect.height > 0
            ? rect.height
            : el.clientHeight > 0
              ? el.clientHeight
              : viewportHeightPx;
        return { width, height };
      };
      cb(measure());
      if (typeof ResizeObserver === 'function') {
        const observer = new ResizeObserver(() => {
          cb(measure());
        });
        observer.observe(el);
        return () => observer.disconnect();
      }
      return undefined;
    },
  });

  const virtualRows = rowVirtualizer.getVirtualItems();

  const handleReplayReceipt = useCallback(
    async (handle: ReceiptReplayHandle): Promise<void> => {
      if (store.getState().sessionGeneration !== handle.sessionGeneration) {
        return;
      }
      setReplayingReceiptId(handle.receiptId);
      const capturedGen = handle.sessionGeneration;
      const acctId = handle.accountId;

      const recordReplayOutcome = (receipt: WriteReceiptRecord): void => {
        if (store.getState().sessionGeneration !== capturedGen) {
          return;
        }
        store.getState().recordWriteReceipt(capturedGen, receipt);
        setReplayHandles((prev) => ({
          ...prev,
          [receipt.id]: {
            receiptId: receipt.id,
            sessionGeneration: capturedGen,
            accountId: acctId,
            replay: handle.replay,
          },
        }));
        if (receipt.committedSeq > 0n) {
          onWriteCommitted?.(receipt.committedSeq);
        }
      };

      try {
        switch (handle.replay.operation) {
          case 'ConfirmHold': {
            const res = await client.confirmHold(handle.replay.request);
            if (store.getState().sessionGeneration !== capturedGen) {
              return;
            }
            if (res.reconfirmationError !== undefined) {
              const expiredErr = BlotterClientError.fromConfirmHoldExpired(res);
              recordReplayOutcome({
                id: nextReceiptId(capturedGen),
                operation: 'ConfirmHold',
                accountId: acctId,
                committedSeq: expiredErr.committedSeq ?? 0n,
                replayed: expiredErr.replayed,
                status: 'confirm_hold_expired',
                earmarkId: handle.replay.request.earmarkId?.value,
                earmark: expiredErr.earmark,
                error: expiredErr,
                recordedAtMs: nowMs(),
              });
              return;
            }
            recordReplayOutcome({
              id: nextReceiptId(capturedGen),
              operation: 'ConfirmHold',
              accountId: acctId,
              committedSeq: res.committedSeq,
              replayed: res.replayed,
              status: 'succeeded',
              earmarkId: handle.replay.request.earmarkId?.value,
              earmark: res.earmark,
              recordedAtMs: nowMs(),
            });
            return;
          }
          case 'CreateHold': {
            const res = await client.createHold(handle.replay.request);
            recordReplayOutcome({
              id: nextReceiptId(capturedGen),
              operation: 'CreateHold',
              accountId: acctId,
              committedSeq: res.committedSeq,
              replayed: res.replayed,
              status: 'succeeded',
              earmarkId: res.earmark?.earmarkId?.value,
              earmark: res.earmark,
              recordedAtMs: nowMs(),
            });
            return;
          }
          case 'ExtendHold': {
            const res = await client.extendHold(handle.replay.request);
            recordReplayOutcome({
              id: nextReceiptId(capturedGen),
              operation: 'ExtendHold',
              accountId: acctId,
              committedSeq: res.committedSeq,
              replayed: res.replayed,
              status: 'succeeded',
              earmarkId: handle.replay.request.earmarkId?.value,
              earmark: res.earmark,
              recordedAtMs: nowMs(),
            });
            return;
          }
          case 'ReleaseHold': {
            const res = await client.releaseHold(handle.replay.request);
            recordReplayOutcome({
              id: nextReceiptId(capturedGen),
              operation: 'ReleaseHold',
              accountId: acctId,
              committedSeq: res.committedSeq,
              replayed: res.replayed,
              status: 'succeeded',
              earmarkId: handle.replay.request.earmarkId?.value,
              earmark: res.earmark,
              recordedAtMs: nowMs(),
            });
            return;
          }
          case 'CreateCommitted': {
            const res = await client.createCommitted(handle.replay.request);
            recordReplayOutcome({
              id: nextReceiptId(capturedGen),
              operation: 'CreateCommitted',
              accountId: acctId,
              committedSeq: res.committedSeq,
              replayed: res.replayed,
              status: 'succeeded',
              earmarkId: res.earmark?.earmarkId?.value,
              earmark: res.earmark,
              recordedAtMs: nowMs(),
            });
            return;
          }
          case 'CancelCommitted': {
            const res = await client.cancelCommitted(handle.replay.request);
            recordReplayOutcome({
              id: nextReceiptId(capturedGen),
              operation: 'CancelCommitted',
              accountId: acctId,
              committedSeq: res.committedSeq,
              replayed: res.replayed,
              status: 'succeeded',
              earmarkId: handle.replay.request.earmarkId?.value,
              earmark: res.earmark,
              recordedAtMs: nowMs(),
            });
            return;
          }
        }
      } catch (err) {
        if (store.getState().sessionGeneration !== capturedGen) {
          return;
        }
        const clientErr = BlotterClientError.fromUnknown(err);
        if (clientErr.kind === 'confirm_hold_expired') {
          recordReplayOutcome({
            id: nextReceiptId(capturedGen),
            operation: 'ConfirmHold',
            accountId: acctId,
            committedSeq: clientErr.committedSeq ?? 0n,
            replayed: clientErr.replayed,
            status: 'confirm_hold_expired',
            earmarkId:
              handle.replay.operation === 'ConfirmHold'
                ? handle.replay.request.earmarkId?.value
                : undefined,
            earmark: clientErr.earmark,
            error: clientErr,
            recordedAtMs: nowMs(),
          });
          return;
        }
        recordReplayOutcome({
          id: nextReceiptId(capturedGen),
          operation: handle.replay.operation,
          accountId: acctId,
          committedSeq: 0n,
          replayed: false,
          status: 'failed',
          error: clientErr,
          recordedAtMs: nowMs(),
        });
      } finally {
        setReplayingReceiptId(null);
      }
    },
    [client, nextReceiptId, nowMs, onWriteCommitted, store],
  );

  if (!snapshot) {
    return (
      <section
        data-testid="positions-grid-panel"
        aria-label="虚拟持仓表"
        style={{ padding: '12px', border: '1px solid #cbd5e1', borderRadius: '6px' }}
      >
        <div data-testid="positions-grid-empty">尚未加载账户快照。</div>
      </section>
    );
  }

  return (
    <section
      data-testid="positions-grid-panel"
      data-snapshot-seq={snapshot.snapshotSeq.toString()}
      aria-label="虚拟持仓表与占仓管理"
      style={{ display: 'flex', flexDirection: 'column', gap: '12px' }}
    >
      <BucketBalanceStrip snapshot={snapshot} selectedPosition={selectedPosition} />

      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: '8px',
          alignItems: 'center',
          fontSize: '13px',
        }}
      >
        <label>
          筛选券种
          <input
            type="search"
            aria-label="筛选券种"
            data-testid="positions-search-input"
            placeholder="输入券种代码..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
          />
        </label>

        <label>
          覆盖状态
          <select
            aria-label="覆盖状态筛选"
            data-testid="positions-coverage-filter"
            value={coverageFilter}
            onChange={(e) => setCoverageFilter(e.target.value as CoverageFilter)}
          >
            <option value="all">全部</option>
            <option value="covered">仅 PRE_TAX 已覆盖</option>
            <option value="uncovered">仅未覆盖</option>
          </select>
        </label>

        <label style={{ display: 'inline-flex', alignItems: 'center', gap: '4px' }}>
          <input
            type="checkbox"
            data-testid="positions-active-earmarks-filter"
            checked={onlyWithActiveEarmarks}
            onChange={(e) => setOnlyWithActiveEarmarks(e.target.checked)}
          />
          仅显示含活动占仓
        </label>

        <span data-testid="positions-row-counts" style={{ marginLeft: 'auto', fontSize: '12px', color: '#475569' }}>
          筛选行数: {tableRows.length} / 总计: {rowModels.length} · 已挂载窗口行: {virtualRows.length}
        </span>
      </div>

      <div
        role="table"
        aria-label="持仓明细虚拟表"
        aria-rowcount={tableRows.length}
        style={{
          border: '1px solid #cbd5e1',
          borderRadius: '6px',
          overflow: 'hidden',
          backgroundColor: '#ffffff',
        }}
      >
        <div role="rowgroup" style={{ backgroundColor: '#f1f5f9', borderBottom: '1px solid #cbd5e1' }}>
          {table.getHeaderGroups().map((headerGroup) => (
            <div
              key={headerGroup.id}
              role="row"
              style={{
                display: 'grid',
                gridTemplateColumns: '140px 180px 230px 210px 180px minmax(200px, 1fr)',
                gap: '8px',
                padding: '8px',
                fontSize: '12px',
                fontWeight: 600,
              }}
            >
              {headerGroup.headers.map((header) => (
                <div key={header.id} role="columnheader">
                  {header.isPlaceholder
                    ? null
                    : flexRender(header.column.columnDef.header, header.getContext())}
                </div>
              ))}
            </div>
          ))}
        </div>

        <div
          ref={scrollContainerRef}
          data-testid="positions-grid-scroll"
          style={{
            height: `${viewportHeightPx}px`,
            overflowY: 'auto',
            overflowX: 'auto',
            position: 'relative',
          }}
        >
          <div
            role="rowgroup"
            data-testid="positions-grid-virtual-body"
            style={{
              height: `${rowVirtualizer.getTotalSize()}px`,
              width: '100%',
              position: 'relative',
            }}
          >
            {virtualRows.map((virtualRow) => {
              const row = tableRows[virtualRow.index];
              if (!row) {
                return null;
              }
              const instrumentCode = row.original.position.instrumentCode;
              const isSelected = selectedPosition?.instrumentCode === instrumentCode;

              return (
                <div
                  key={row.id}
                  role="row"
                  aria-rowindex={virtualRow.index + 1}
                  aria-selected={isSelected}
                  data-testid={`position-row-${instrumentCode}`}
                  data-row-index={virtualRow.index}
                  onClick={() => {
                    setSelectedInstrumentCode(instrumentCode);
                    if (
                      selectedEarmark &&
                      selectedEarmark.instrumentCode !== instrumentCode
                    ) {
                      setSelectedEarmarkId(null);
                      if (
                        activeMode === 'extend_hold' ||
                        activeMode === 'confirm_hold' ||
                        activeMode === 'release_hold' ||
                        activeMode === 'cancel_committed'
                      ) {
                        setActiveMode('create_hold');
                      }
                    }
                  }}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    height: `${virtualRow.size}px`,
                    transform: `translateY(${virtualRow.start}px)`,
                    display: 'grid',
                    gridTemplateColumns: '140px 180px 230px 210px 180px minmax(200px, 1fr)',
                    gap: '8px',
                    padding: '6px 8px',
                    boxSizing: 'border-box',
                    borderBottom: '1px solid #e2e8f0',
                    backgroundColor: isSelected ? '#eff6ff' : '#ffffff',
                    cursor: 'pointer',
                    alignItems: 'center',
                  }}
                >
                  {row.getVisibleCells().map((cell) => (
                    <div key={cell.id} role="cell" style={{ overflow: 'hidden' }}>
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </div>
                  ))}
                </div>
              );
            })}
          </div>
        </div>
      </div>

      <HoldActionForm
        store={store}
        client={client}
        writeIntents={writeIntents}
        snapshot={snapshot}
        sessionGeneration={sessionGeneration}
        authenticated={authenticated}
        roleHint={roleHint}
        recoveryPausedWrites={recoveryPausedWrites}
        recoveryReason={recoveryReason}
        selectedPosition={selectedPosition}
        selectedEarmark={selectedEarmark}
        activeMode={activeMode}
        onModeChange={setActiveMode}
        nowMs={nowMs}
        nextReceiptId={nextReceiptId}
        onRegisterReplayHandle={handleRegisterReplayHandle}
        onWriteCommitted={onWriteCommitted}
        onResyncRequested={onResyncRequested}
      />

      <ReceiptsDrawer
        accountId={snapshot.accountId}
        sessionGeneration={sessionGeneration}
        receipts={writeReceipts}
        replayHandlesByReceiptId={replayHandles}
        replayingReceiptId={replayingReceiptId}
        onReplayReceipt={handleReplayReceipt}
        onResyncRequested={onResyncRequested}
      />
    </section>
  );
}
