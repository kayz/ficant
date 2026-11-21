import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import { useStore } from 'zustand';
import {
  PendingMatchReason,
  PendingMatchResolutionAction,
  type AccountBlotterSnapshot,
  type EarmarkRecord,
  type PendingMatchRecord,
  type ReconcileAllocationView,
} from '../../generated/ficant/blotter/v1/model_pb';
import type { ValidationIssue } from '../../codec/validators';
import type { BlotterStoreApi } from '../../store/blotterStore';
import { selectIsWritesPaused } from '../../store/selectors';
import {
  lookupCommittedAllocationsReadOnly,
  type BlotterSupervisor,
} from '../../store/supervisor';
import type { DeskBlotterRpcClient } from '../../transport/client';
import { BlotterClientError } from '../../transport/error';
import { WriteIntentManager } from '../../transport/idempotency';
import {
  resolveSubmissionAttribution,
  shouldAutoLookupAllocations,
  type PendingMatchSubmissionRecord,
  type ResolvedAttributionView,
} from './allocationAttribution';
import {
  ALL_RESOLUTION_ACTIONS,
  buildResolvePendingMatchRequest,
  computePendingMatchGateContext,
  computePreSubmitPreview,
  evaluateAllActionsAvailability,
  formatOptionalSignedDecimal,
  formatPendingMatchReasonLabel,
  formatPendingMatchStateLabel,
  formatResolutionActionLabel,
  formatSettlementBucketLabel,
  formatTradeSideLabel,
  pickDefaultAction,
  type ResolvePendingMatchFormInput,
} from './pendingMatchRules';

export interface PendingMatchPanelProps {
  readonly store: BlotterStoreApi;
  readonly client: Pick<DeskBlotterRpcClient, 'resolvePendingMatch' | 'pollEvents'>;
  readonly supervisor?: Pick<BlotterSupervisor, 'writeIntents' | 'triggerCatchUp'> | undefined;
  readonly writeIntents?: WriteIntentManager | undefined;
  readonly accountId?: string | undefined;
  readonly lookupAllocations?: typeof lookupCommittedAllocationsReadOnly | undefined;
}

interface FormDraftState {
  readonly pendingMatchId: string;
  readonly action: PendingMatchResolutionAction | null;
  readonly targetEarmarkId: string;
  readonly offsetNotionalInput: string;
  readonly reasonInput: string;
  readonly baselineDeltaNotionalInput: string;
  readonly baselineDeltaCashInput: string;
  readonly baselineDeltaReasonInput: string;
}

function createEmptyFormDraft(pendingMatchId: string): FormDraftState {
  return {
    pendingMatchId,
    action: null,
    targetEarmarkId: '',
    offsetNotionalInput: '',
    reasonInput: '',
    baselineDeltaNotionalInput: '',
    baselineDeltaCashInput: '',
    baselineDeltaReasonInput: '',
  };
}

function renderAllocationRows(allocations: readonly ReconcileAllocationView[]) {
  return (
    <ul data-testid="transaction-allocation-list">
      {allocations.map((alloc, idx) => {
        const allocId = alloc.allocationId?.value ?? `alloc-${idx}`;
        return (
          <li key={allocId} data-testid={`allocation-row-${allocId}`}>
            <div>
              <span>分摊ID: {allocId}</span>
              {' | '}
              <span>成交源: {alloc.sourceId}</span>
              {' | '}
              <span>外部成交ID: {alloc.externalTradeId}</span>
              {' | '}
              <span>操作员: {alloc.actorId}</span>
            </div>
            <div>
              <span data-testid={`allocation-notional-${allocId}`}>
                分摊面额: {formatOptionalSignedDecimal(alloc.allocatedNotional, '未提供')}
              </span>
              {' | '}
              <span data-testid={`allocation-fill-cash-${allocId}`}>
                成交现金分摊 (allocated_fill_cash):{' '}
                {formatOptionalSignedDecimal(alloc.allocatedFillCash, '未提供')}
              </span>
              {' | '}
              <span data-testid={`allocation-released-cash-${allocId}`}>
                释放占仓现金 (released_earmark_cash):{' '}
                {formatOptionalSignedDecimal(alloc.releasedEarmarkCash, '无')}
              </span>
              {alloc.earmarkId?.value ? (
                <>
                  {' | '}
                  <span>目标占仓ID: {alloc.earmarkId.value}</span>
                </>
              ) : null}
              {alloc.reversedByRevision !== undefined ? (
                <>
                  {' | '}
                  <span>已被修订冲销: rev {alloc.reversedByRevision.toString()}</span>
                </>
              ) : null}
            </div>
            {alloc.baselineDelta ? (
              <div data-testid={`allocation-baseline-delta-${allocId}`}>
                <span>
                  基线差额 (baseline_delta): 面额{' '}
                  {formatOptionalSignedDecimal(alloc.baselineDelta.deltaNotional, '未提供')} / 现金{' '}
                  {formatOptionalSignedDecimal(alloc.baselineDelta.deltaCash, '未提供')} / 桶{' '}
                  {formatSettlementBucketLabel(alloc.baselineDelta.settlementBucket)} / 原因:{' '}
                  {alloc.baselineDelta.reason}
                </span>
              </div>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}

interface SubmissionCardProps {
  readonly submission: PendingMatchSubmissionRecord;
  readonly attribution: ResolvedAttributionView;
  readonly onManualLookup: (submission: PendingMatchSubmissionRecord) => void;
}

function SubmissionAttributionCard({
  submission,
  attribution,
  onManualLookup,
}: SubmissionCardProps) {
  const testId = `submission-${submission.committedSeq.toString()}-${submission.pendingMatchId}`;

  return (
    <article
      data-testid={testId}
      data-attribution-state={attribution.kind}
      data-committed-seq={submission.committedSeq.toString()}
      data-account-id={submission.accountId}
      data-pending-match-id={submission.pendingMatchId}
    >
      <header>
        <strong>{formatResolutionActionLabel(submission.action)}</strong>
        {' | '}
        <span>账户: {submission.accountId}</span>
        {' | '}
        <span>待核销ID: {submission.pendingMatchId}</span>
        {' | '}
        <span>提交事务序号: seq={submission.committedSeq.toString()}</span>
        {submission.replayed ? (
          <>
            {' | '}
            <span data-testid={`${testId}-replayed-badge`}>幂等重放 (replayed=true)</span>
          </>
        ) : null}
      </header>

      {submission.pendingMatchResult ? (
        <div>
          <span>
            结果状态: {formatPendingMatchStateLabel(submission.pendingMatchResult.state)} (rev{' '}
            {submission.pendingMatchResult.pendingRevision.toString()})
          </span>
          {' | '}
          <span>
            剩余待核销面额:{' '}
            {formatOptionalSignedDecimal(submission.pendingMatchResult.unresolvedNotional, '0')}
          </span>
        </div>
      ) : null}

      {attribution.kind === 'allocated' ? (
        <div>
          <p data-testid={`${testId}-status`}>
            已取得分摊明细（来源：
            {attribution.source === 'store_stream' ? '实时事件流/轮询' : '只读历史定点查询'}）
          </p>
          {renderAllocationRows(attribution.allocations)}
        </div>
      ) : null}

      {attribution.kind === 'no_allocations' ? (
        <div>
          <p data-testid={`${testId}-status`}>本动作无分摊</p>
          <p>释放/拒绝 (RELEASE_REJECT) 不产生现金分摊明细，不显示推算金额。</p>
        </div>
      ) : null}

      {attribution.kind === 'pending_correlation' ? (
        <div>
          <p data-testid={`${testId}-status`}>
            {attribution.isLoadingLookup
              ? '正在只读查询本次事务历史分摊明细...'
              : '等待本次事务明细'}
          </p>
          <p>
            仅按精确事务 (seq={submission.committedSeq.toString()}, account={submission.accountId},
            pending={submission.pendingMatchId}) 关联分摊，不把后续余额变动归因于本次操作。
          </p>
          <button
            type="button"
            disabled={attribution.isLoadingLookup}
            onClick={() => onManualLookup(submission)}
          >
            只读查询本次事务分摊
          </button>
        </div>
      ) : null}

      {attribution.kind === 'transient_disconnected' ? (
        <div>
          <p data-testid={`${testId}-status`}>暂时断线，等待重新获取本次事务明细</p>
          <p>
            当前为临时通信/查询中断，尚未确认历史日志裁剪。
            {attribution.error ? ` 原因: ${attribution.error.message}` : ''}
          </p>
          <button type="button" onClick={() => onManualLookup(submission)}>
            重试只读查询
          </button>
        </div>
      ) : null}

      {attribution.kind === 'pruned_history_unrecoverable' ? (
        <div>
          <p data-testid={`${testId}-status`}>历史分摊已裁剪，明细不可恢复</p>
          {attribution.fallback ? (
            <p>
              请求起点 after_seq={attribution.fallback.requestedAfterSeq.toString()}，保留起点
              oldest_retained_seq={attribution.fallback.oldestRetainedSeq.toString()}，当前头序号
              current_head_seq={attribution.fallback.currentHeadSeq.toString()}
              {attribution.fallback.reason ? `（${attribution.fallback.reason}）` : ''}
            </p>
          ) : null}
        </div>
      ) : null}

      {attribution.kind === 'unmatched_empty_history' ? (
        <div>
          <p data-testid={`${testId}-status`}>未找到匹配本次事务的分摊明细</p>
          <p>
            服务端历史日志完整（未裁剪），但在事务 seq={submission.committedSeq.toString()} / 账户{' '}
            {submission.accountId} / 待核销ID {submission.pendingMatchId}{' '}
            下无匹配分摊记录（无分摊或 pending_match_id / 账户不匹配）。
          </p>
        </div>
      ) : null}
    </article>
  );
}

export function PendingMatchPanel({
  store,
  client,
  supervisor,
  writeIntents,
  accountId,
  lookupAllocations,
}: PendingMatchPanelProps) {
  const state = useStore(store);
  const activeAccountId = accountId ?? state.selectedAccountId;
  const snapshot: AccountBlotterSnapshot | undefined =
    activeAccountId !== null ? state.snapshots[activeAccountId] : undefined;
  const writesPaused = selectIsWritesPaused(state);

  const [fallbackWriteIntents] = useState(() => new WriteIntentManager());
  const intentManager = writeIntents ?? supervisor?.writeIntents ?? fallbackWriteIntents;
  const lookupFn = lookupAllocations ?? lookupCommittedAllocationsReadOnly;

  const [selectedPendingId, setSelectedPendingId] = useState<string>('');
  const [formDraft, setFormDraft] = useState<FormDraftState>(() => createEmptyFormDraft(''));
  const [validationIssues, setValidationIssues] = useState<readonly ValidationIssue[]>([]);
  const [submitError, setSubmitError] = useState<BlotterClientError | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [submissions, setSubmissions] = useState<readonly PendingMatchSubmissionRecord[]>([]);

  const receiptSeqCounterRef = useRef(0);
  const lookupAbortControllersRef = useRef<Map<string, AbortController>>(new Map());
  const inFlightLookupKeysRef = useRef<Set<string>>(new Set());

  // Abort in-flight read-only lookups and clear session-scoped local state on session switch.
  useEffect(() => {
    const controllers = lookupAbortControllersRef.current;
    const inFlight = inFlightLookupKeysRef.current;
    return () => {
      for (const ctrl of controllers.values()) {
        ctrl.abort();
      }
      controllers.clear();
      inFlight.clear();
      fallbackWriteIntents.clearAll();
    };
  }, [state.sessionGeneration, fallbackWriteIntents]);

  const openPendingMatches: readonly PendingMatchRecord[] = useMemo(
    () => snapshot?.openPendingMatches ?? [],
    [snapshot],
  );

  const selectedPm: PendingMatchRecord | undefined = useMemo(() => {
    if (openPendingMatches.length === 0) {
      return undefined;
    }
    const found = openPendingMatches.find((pm) => pm.pendingMatchId?.value === selectedPendingId);
    return found ?? openPendingMatches[0];
  }, [openPendingMatches, selectedPendingId]);

  const selectedPmId = selectedPm?.pendingMatchId?.value ?? '';

  const activeDraft: FormDraftState = useMemo(
    () =>
      formDraft.pendingMatchId === selectedPmId
        ? formDraft
        : createEmptyFormDraft(selectedPmId),
    [formDraft, selectedPmId],
  );

  const gate = useMemo(
    () => (snapshot && selectedPm ? computePendingMatchGateContext(snapshot, selectedPm) : null),
    [snapshot, selectedPm],
  );

  const availability = useMemo(
    () => (selectedPm && gate ? evaluateAllActionsAvailability(selectedPm, gate, writesPaused) : null),
    [selectedPm, gate, writesPaused],
  );

  const activeAction: PendingMatchResolutionAction = useMemo(() => {
    if (!selectedPm || !availability) {
      return PendingMatchResolutionAction.MATCH_FULL;
    }
    if (activeDraft.action !== null && availability[activeDraft.action].allowed) {
      return activeDraft.action;
    }
    return pickDefaultAction(selectedPm, availability);
  }, [selectedPm, availability, activeDraft.action]);

  const candidateEarmarksForAction: readonly EarmarkRecord[] = useMemo(() => {
    if (!gate) {
      return [];
    }
    if (activeAction === PendingMatchResolutionAction.MATCH_FULL) {
      return gate.fullMatchEarmarks.length > 0
        ? gate.fullMatchEarmarks
        : gate.eligibleCommittedEarmarks;
    }
    return gate.eligibleCommittedEarmarks;
  }, [gate, activeAction]);

  const activeTargetEarmarkId: string = useMemo(() => {
    if (
      activeAction !== PendingMatchResolutionAction.MATCH_FULL &&
      activeAction !== PendingMatchResolutionAction.SPLIT_OFFSET
    ) {
      return '';
    }
    if (
      activeDraft.targetEarmarkId.length > 0 &&
      candidateEarmarksForAction.some((e) => e.earmarkId?.value === activeDraft.targetEarmarkId)
    ) {
      return activeDraft.targetEarmarkId;
    }
    return candidateEarmarksForAction[0]?.earmarkId?.value ?? '';
  }, [activeAction, activeDraft.targetEarmarkId, candidateEarmarksForAction]);

  const selectedTargetEarmark: EarmarkRecord | undefined = useMemo(
    () => candidateEarmarksForAction.find((e) => e.earmarkId?.value === activeTargetEarmarkId),
    [candidateEarmarksForAction, activeTargetEarmarkId],
  );

  const effectiveFormInput: ResolvePendingMatchFormInput = useMemo(
    () => ({
      action: activeAction,
      targetEarmarkId: activeTargetEarmarkId,
      offsetNotionalInput: activeDraft.offsetNotionalInput,
      reasonInput: activeDraft.reasonInput,
      baselineDeltaNotionalInput: activeDraft.baselineDeltaNotionalInput,
      baselineDeltaCashInput: activeDraft.baselineDeltaCashInput,
      baselineDeltaReasonInput: activeDraft.baselineDeltaReasonInput,
      baselineDeltaBucket: selectedPm?.evidence?.settlementBucket ?? selectedPm?.settlementBucket,
    }),
    [activeAction, activeTargetEarmarkId, activeDraft, selectedPm],
  );

  const preSubmitPreview = useMemo(
    () =>
      selectedPm
        ? computePreSubmitPreview(selectedPm, effectiveFormInput, selectedTargetEarmark)
        : null,
    [selectedPm, effectiveFormInput, selectedTargetEarmark],
  );

  const updateDraft = useCallback(
    (patch: Partial<Omit<FormDraftState, 'pendingMatchId'>>) => {
      setValidationIssues([]);
      setSubmitError(null);
      setFormDraft((prev) => {
        const base =
          prev.pendingMatchId === selectedPmId ? prev : createEmptyFormDraft(selectedPmId);
        return { ...base, ...patch };
      });
    },
    [selectedPmId],
  );

  const executeReadOnlyLookup = useCallback(
    async (submission: PendingMatchSubmissionRecord): Promise<void> => {
      const key = submission.receiptId;
      if (inFlightLookupKeysRef.current.has(key)) {
        return;
      }
      inFlightLookupKeysRef.current.add(key);

      const prevCtrl = lookupAbortControllersRef.current.get(key);
      if (prevCtrl) {
        prevCtrl.abort();
      }
      const abortCtrl = new AbortController();
      lookupAbortControllersRef.current.set(key, abortCtrl);

      setSubmissions((prev) =>
        prev.map((item) =>
          item.receiptId === key
            ? {
                ...item,
                lookupStatus: 'loading',
                lookupError: undefined,
              }
            : item,
        ),
      );

      try {
        const result = await lookupFn(client, {
          sessionGeneration: submission.sessionGeneration,
          getCurrentSessionGeneration: () => store.getState().sessionGeneration,
          accountId: submission.accountId,
          committedSeq: submission.committedSeq,
          pendingMatchId: submission.pendingMatchId,
          signal: abortCtrl.signal,
        });

        if (store.getState().sessionGeneration !== submission.sessionGeneration) {
          return;
        }

        setSubmissions((prev) =>
          prev.map((item) => {
            if (item.receiptId !== key) {
              return item;
            }
            switch (result.status) {
              case 'found':
                return {
                  ...item,
                  lookupStatus: 'found',
                  lookedUpAllocations: result.allocations,
                  lookupError: undefined,
                  lookupFallback: undefined,
                };
              case 'no_allocations':
                return {
                  ...item,
                  lookupStatus: 'no_allocations',
                  lookedUpAllocations: [],
                  lookupError: undefined,
                  lookupFallback: undefined,
                };
              case 'pruned_unrecoverable':
                return {
                  ...item,
                  lookupStatus: 'pruned_unrecoverable',
                  lookedUpAllocations: [],
                  lookupFallback: result.fallback,
                  lookupError: undefined,
                };
              case 'transient_error':
                return {
                  ...item,
                  lookupStatus: 'transient_error',
                  lookupError: result.error,
                };
              case 'stale':
                return item;
            }
          }),
        );
      } finally {
        inFlightLookupKeysRef.current.delete(key);
        if (lookupAbortControllersRef.current.get(key) === abortCtrl) {
          lookupAbortControllersRef.current.delete(key);
        }
      }
    },
    [client, lookupFn, store],
  );

  // Filter strictly to current sessionGeneration so session switches leave zero stale rows.
  const sessionSubmissions = useMemo(
    () => submissions.filter((s) => s.sessionGeneration === state.sessionGeneration),
    [submissions, state.sessionGeneration],
  );

  // Auto-trigger point-in-time read-only lookup when workspace cursorSeq reaches or passes
  // committedSeq and local store has no matching allocation (e.g. replayed old seq, recovery jump,
  // or mismatched/empty event). NEVER mutates workspace cursorSeq!
  useEffect(() => {
    for (const sub of sessionSubmissions) {
      if (shouldAutoLookupAllocations(sub, state)) {
        void executeReadOnlyLookup(sub);
      }
    }
  }, [sessionSubmissions, state, executeReadOnlyLookup]);

  const handleSubmit = async (event: FormEvent<HTMLFormElement>): Promise<void> => {
    event.preventDefault();
    if (!snapshot || !selectedPm || !selectedPm.pendingMatchId) {
      return;
    }

    const submitSessionGen = store.getState().sessionGeneration;
    const buildPreview = buildResolvePendingMatchRequest({
      snapshot,
      pendingMatch: selectedPm,
      form: effectiveFormInput,
      writesPaused,
    });

    if (!buildPreview.ok) {
      setValidationIssues(buildPreview.issues);
      return;
    }

    setValidationIssues([]);
    setSubmitError(null);

    const slotKey = `resolve_pending_match:${selectedPm.accountId}:${selectedPm.pendingMatchId.value}`;
    let intentKey: string;
    try {
      const intent = intentManager.acquire({
        operation: slotKey,
        canonicalPayload: buildPreview.canonicalPayload,
        sessionGeneration: submitSessionGen,
      });
      intentKey = intent.idempotencyKey;
    } catch (err) {
      setSubmitError(BlotterClientError.fromUnknown(err));
      return;
    }

    const finalBuilt = buildResolvePendingMatchRequest({
      snapshot,
      pendingMatch: selectedPm,
      form: effectiveFormInput,
      writesPaused,
      idempotencyKey: intentKey,
    });
    if (!finalBuilt.ok) {
      setValidationIssues(finalBuilt.issues);
      return;
    }

    setSubmitting(true);
    try {
      const result = await client.resolvePendingMatch(finalBuilt.request);
      if (store.getState().sessionGeneration !== submitSessionGen) {
        return;
      }

      intentManager.complete(slotKey, intentKey);

      receiptSeqCounterRef.current += 1;
      const receiptId = `pm-${submitSessionGen}-${selectedPm.accountId}-${result.committedSeq.toString()}-${selectedPm.pendingMatchId.value}-${receiptSeqCounterRef.current}`;

      // Record write receipt in store WITHOUT advancing workspace cursorSeq!
      store.getState().recordWriteReceipt(submitSessionGen, {
        id: receiptId,
        operation: 'ResolvePendingMatch',
        accountId: selectedPm.accountId,
        committedSeq: result.committedSeq,
        replayed: result.replayed,
        status: 'succeeded',
        pendingMatchId: selectedPm.pendingMatchId.value,
        ...(result.updatedEarmark?.earmarkId?.value !== undefined
          ? { earmarkId: result.updatedEarmark.earmarkId.value }
          : {}),
        ...(result.updatedEarmark !== undefined ? { earmark: result.updatedEarmark } : {}),
        recordedAtMs: Date.now(),
      });

      supervisor?.triggerCatchUp();

      const nextRecord: PendingMatchSubmissionRecord = {
        receiptId,
        sessionGeneration: submitSessionGen,
        accountId: selectedPm.accountId,
        pendingMatchId: selectedPm.pendingMatchId.value,
        instrumentCode: selectedPm.instrumentCode,
        action: effectiveFormInput.action,
        committedSeq: result.committedSeq,
        replayed: result.replayed,
        submittedAtSnapshotSeq: snapshot.snapshotSeq,
        ...(result.pendingMatch !== undefined ? { pendingMatchResult: result.pendingMatch } : {}),
        ...(result.updatedEarmark !== undefined ? { updatedEarmark: result.updatedEarmark } : {}),
        ...(result.fundingSummary !== undefined
          ? { fundingSummaryAtResult: result.fundingSummary }
          : {}),
        lookupStatus: 'idle',
        lookedUpAllocations: [],
      };

      setSubmissions((prev) => {
        const currentSessionItems = prev.filter((item) => item.sessionGeneration === submitSessionGen);
        const existingIdx = currentSessionItems.findIndex(
          (item) =>
            item.accountId === nextRecord.accountId &&
            item.committedSeq === nextRecord.committedSeq &&
            item.pendingMatchId === nextRecord.pendingMatchId &&
            item.action === nextRecord.action,
        );
        if (existingIdx >= 0) {
          const existing = currentSessionItems[existingIdx];
          if (!existing) {
            return [nextRecord, ...currentSessionItems];
          }
          const updated = [...currentSessionItems];
          updated[existingIdx] = {
            ...existing,
            replayed: nextRecord.replayed,
            ...(nextRecord.pendingMatchResult !== undefined
              ? { pendingMatchResult: nextRecord.pendingMatchResult }
              : {}),
            ...(nextRecord.updatedEarmark !== undefined
              ? { updatedEarmark: nextRecord.updatedEarmark }
              : {}),
          };
          return updated;
        }
        return [nextRecord, ...currentSessionItems];
      });
    } catch (err) {
      if (store.getState().sessionGeneration !== submitSessionGen) {
        return;
      }
      const clientErr = BlotterClientError.fromUnknown(err);
      // Retain sticky write intent ONLY on transport errors so retry reuses exact idempotency_key.
      if (clientErr.kind !== 'transport') {
        intentManager.complete(slotKey, intentKey);
      }
      setSubmitError(clientErr);
    } finally {
      setSubmitting(false);
    }
  };

  if (!snapshot) {
    return (
      <section aria-label="待核销核销面板" data-testid="pending-match-panel">
        <h2>待核销核销 (ResolvePendingMatch)</h2>
        <p>请先加载并选择账户快照。</p>
      </section>
    );
  }

  return (
    <section
      aria-label="待核销核销面板"
      data-testid="pending-match-panel"
      data-account-id={snapshot.accountId}
      data-snapshot-seq={snapshot.snapshotSeq.toString()}
    >
      <header>
        <h2>待核销核销 (ResolvePendingMatch)</h2>
        <div>
          <span>账户: {snapshot.accountId}</span>
          {' | '}
          <span>快照序号: seq={snapshot.snapshotSeq.toString()}</span>
          {' | '}
          <span>待核销笔数: {openPendingMatches.length}</span>
        </div>
      </header>

      {openPendingMatches.length === 0 ? (
        <p data-testid="pending-match-empty">当前账户无 OPEN 状态的待核销记录。</p>
      ) : (
        <div>
          <div role="region" aria-label="待核销列表">
            <h3>待核销队列</h3>
            <ul>
              {openPendingMatches.map((pm) => {
                const pmId = pm.pendingMatchId?.value ?? '';
                const isSelected = pmId === selectedPmId;
                return (
                  <li key={pmId}>
                    <button
                      type="button"
                      aria-pressed={isSelected}
                      data-testid={`pending-match-item-${pmId}`}
                      onClick={() => {
                        setSelectedPendingId(pmId);
                        setValidationIssues([]);
                        setSubmitError(null);
                      }}
                    >
                      <span>{pm.instrumentCode}</span>
                      {' | '}
                      <span>{formatTradeSideLabel(pm.side)}</span>
                      {' | '}
                      <span>
                        {formatSettlementBucketLabel(pm.settlementBucket)} ({pm.settlementDate})
                      </span>
                      {' | '}
                      <span>未决面额: {formatOptionalSignedDecimal(pm.unresolvedNotional, '0')}</span>
                      {' | '}
                      <span>{formatPendingMatchReasonLabel(pm.reason)}</span>
                      {' | '}
                      <span>rev {pm.pendingRevision.toString()}</span>
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>

          {selectedPm && gate && availability ? (
            <form
              aria-label="核销操作表单"
              data-testid="resolve-pending-match-form"
              onSubmit={(e) => {
                void handleSubmit(e);
              }}
              noValidate
            >
              <h3>当前待核销明细与版本绑定 (CAS)</h3>
              <dl data-testid="selected-pending-match-meta">
                <dt>待核销ID (pending_match_id)</dt>
                <dd>{selectedPm.pendingMatchId?.value ?? '—'}</dd>
                <dt>四键 (account / instrument / side / settlement_date)</dt>
                <dd>
                  {selectedPm.accountId} / {selectedPm.instrumentCode} /{' '}
                  {formatTradeSideLabel(selectedPm.side)} / {selectedPm.settlementDate} (
                  {formatSettlementBucketLabel(selectedPm.settlementBucket)})
                </dd>
                <dt>成交源与外部单号</dt>
                <dd>
                  {selectedPm.sourceId} / {selectedPm.externalTradeId} (rev{' '}
                  {selectedPm.sourceRevision.toString()})
                </dd>
                <dt>原因与状态</dt>
                <dd>
                  {formatPendingMatchReasonLabel(selectedPm.reason)} /{' '}
                  {formatPendingMatchStateLabel(selectedPm.state)}
                </dd>
                <dt>CAS 绑定版本</dt>
                <dd data-testid="cas-version-summary">
                  expected_pending_revision={selectedPm.pendingRevision.toString()},
                  expected_position_version=
                  {gate.position ? gate.position.positionVersion.toString() : '缺失'}
                  {selectedTargetEarmark
                    ? `, expected_earmark_revision=${selectedTargetEarmark.earmarkRevision.toString()}`
                    : ''}
                </dd>
              </dl>

              {gate.hasActiveSourceGap ? (
                <div role="status" data-testid="source-gap-banner">
                  <strong>源缺口提示：</strong>
                  成交源 {selectedPm.sourceId} 存在活动缺段 (
                  {gate.sourceFeed?.missingFeedRanges
                    .map((r) => `[${r.first.toString()}..${r.last.toString()}]`)
                    .join(', ')}
                  )。
                  {selectedPm.reason === PendingMatchReason.BASELINE_REVISION_MANUAL
                    ? '前四项核销动作已禁用；同桶有效原证据的 APPLY_BASELINE_DELTA 仍可合法提交。'
                    : '普通核销动作已暂停，等待缺段补齐。'}
                </div>
              ) : null}

              {gate.isSelfRevisionHold || gate.hasSameKeyRevisionHold ? (
                <div role="status" data-testid="revision-hold-banner">
                  <strong>修订暂挂提示：</strong>
                  {gate.isSelfRevisionHold
                    ? '当前记录处于 REVISION_HOLD，前四项动作禁止提交。'
                    : '同四键存在活动 REVISION_HOLD，前四项动作禁止提交。'}
                </div>
              ) : null}

              <fieldset>
                <legend>选择核销动作 (5-Action Matrix)</legend>
                {ALL_RESOLUTION_ACTIONS.map((action) => {
                  const avail = availability[action];
                  const inputId = `pm-action-${String(action)}`;
                  return (
                    <div key={String(action)}>
                      <label htmlFor={inputId}>
                        <input
                          id={inputId}
                          type="radio"
                          name="pending-match-action"
                          value={String(action)}
                          checked={activeAction === action}
                          disabled={!avail.allowed}
                          onChange={() => updateDraft({ action })}
                        />
                        {formatResolutionActionLabel(action)}
                      </label>
                      {!avail.allowed && avail.disabledReason ? (
                        <span data-testid={`action-disabled-reason-${String(action)}`}>
                          {' '}
                          — {avail.disabledReason}
                        </span>
                      ) : null}
                    </div>
                  );
                })}
              </fieldset>

              {activeAction === PendingMatchResolutionAction.MATCH_FULL ||
              activeAction === PendingMatchResolutionAction.SPLIT_OFFSET ? (
                <div>
                  <label htmlFor="pm-target-earmark">
                    目标 COMMITTED 占仓 (target_earmark_id)
                  </label>
                  <select
                    id="pm-target-earmark"
                    value={activeTargetEarmarkId}
                    onChange={(e) => updateDraft({ targetEarmarkId: e.target.value })}
                  >
                    {candidateEarmarksForAction.length === 0 ? (
                      <option value="">无可用同四键 COMMITTED 占仓</option>
                    ) : (
                      candidateEarmarksForAction.map((em) => {
                        const emId = em.earmarkId?.value ?? '';
                        return (
                          <option key={emId} value={emId}>
                            {emId} | rev {em.earmarkRevision.toString()} | 剩余面额{' '}
                            {formatOptionalSignedDecimal(em.unreconciledNotional, '0')} |{' '}
                            {em.settlementDate}
                          </option>
                        );
                      })
                    )}
                  </select>
                </div>
              ) : null}

              {activeAction === PendingMatchResolutionAction.SPLIT_OFFSET ? (
                <div>
                  <label htmlFor="pm-offset-notional">拆分冲抵面额 (offset_notional)</label>
                  <input
                    id="pm-offset-notional"
                    type="text"
                    inputMode="decimal"
                    placeholder="输入正数冲抵面额"
                    value={activeDraft.offsetNotionalInput}
                    onChange={(e) => updateDraft({ offsetNotionalInput: e.target.value })}
                  />
                </div>
              ) : null}

              {activeAction === PendingMatchResolutionAction.MATCH_FULL ||
              activeAction === PendingMatchResolutionAction.SPLIT_OFFSET ||
              activeAction === PendingMatchResolutionAction.ACCEPT_ORPHAN ||
              activeAction === PendingMatchResolutionAction.RELEASE_REJECT ? (
                <div>
                  <label htmlFor="pm-reason">
                    {activeAction === PendingMatchResolutionAction.RELEASE_REJECT
                      ? '释放/拒绝原因 (reason，必填)'
                      : '核销原因 (reason，可选)'}
                  </label>
                  <input
                    id="pm-reason"
                    type="text"
                    value={activeDraft.reasonInput}
                    onChange={(e) => updateDraft({ reasonInput: e.target.value })}
                  />
                </div>
              ) : null}

              {activeAction === PendingMatchResolutionAction.APPLY_BASELINE_DELTA ? (
                <div data-testid="baseline-delta-fields">
                  <h4>原始基线证据 (BaseEvidence，只读原样提交)</h4>
                  {selectedPm.evidence ? (
                    <dl data-testid="base-evidence-summary">
                      <dt>源 / 外部单号 / 原版本</dt>
                      <dd>
                        {selectedPm.evidence.sourceId} / {selectedPm.evidence.externalTradeId} / rev{' '}
                        {selectedPm.evidence.originalRevision.toString()}
                      </dd>
                      <dt>券种 / 方向 / 结算桶</dt>
                      <dd>
                        {selectedPm.evidence.instrumentCode} /{' '}
                        {formatTradeSideLabel(selectedPm.evidence.side)} /{' '}
                        {formatSettlementBucketLabel(selectedPm.evidence.settlementBucket)}
                      </dd>
                      <dt>原基线面额 / 原基线带符号现金</dt>
                      <dd>
                        {formatOptionalSignedDecimal(selectedPm.evidence.oldNotional, '未知')} /{' '}
                        {formatOptionalSignedDecimal(selectedPm.evidence.oldSignedCash, '未知')}
                      </dd>
                      <dt>证据引用 (evidence_ref)</dt>
                      <dd>{selectedPm.evidence.evidenceRef}</dd>
                    </dl>
                  ) : (
                    <p role="alert">缺少已验证的原始基线证据 BaseEvidence。</p>
                  )}

                  <div>
                    <label htmlFor="pm-baseline-delta-notional">
                      基线面额差额 (baseline_delta.delta_notional)
                    </label>
                    <input
                      id="pm-baseline-delta-notional"
                      type="text"
                      inputMode="decimal"
                      placeholder="支持正数、负数或 0"
                      value={activeDraft.baselineDeltaNotionalInput}
                      onChange={(e) =>
                        updateDraft({ baselineDeltaNotionalInput: e.target.value })
                      }
                    />
                  </div>

                  <div>
                    <label htmlFor="pm-baseline-delta-cash">
                      基线现金差额 (baseline_delta.delta_cash)
                    </label>
                    <input
                      id="pm-baseline-delta-cash"
                      type="text"
                      inputMode="decimal"
                      placeholder="支持正数、负数或 0"
                      value={activeDraft.baselineDeltaCashInput}
                      onChange={(e) => updateDraft({ baselineDeltaCashInput: e.target.value })}
                    />
                  </div>

                  <div>
                    <label htmlFor="pm-baseline-delta-reason">
                      基线修订原因 (baseline_delta.reason，必填)
                    </label>
                    <input
                      id="pm-baseline-delta-reason"
                      type="text"
                      value={activeDraft.baselineDeltaReasonInput}
                      onChange={(e) => updateDraft({ baselineDeltaReasonInput: e.target.value })}
                    />
                  </div>
                </div>
              ) : null}

              {preSubmitPreview ? (
                <div
                  role="region"
                  aria-label="提交前确定预览"
                  data-testid="pre-submit-preview"
                >
                  <h4>提交前预览（确定面额与已知现金）</h4>
                  <dl>
                    <dt>当前待核销面额</dt>
                    <dd data-testid="preview-current-notional">
                      {preSubmitPreview.currentUnresolvedNotionalText}
                    </dd>
                    <dt>当前待核销已知现金</dt>
                    <dd data-testid="preview-known-pending-cash">
                      {preSubmitPreview.currentKnownPendingCashText}
                    </dd>
                    {preSubmitPreview.projectedOffsetNotionalText !== null ? (
                      <>
                        <dt>本次确定冲抵面额</dt>
                        <dd data-testid="preview-offset-notional">
                          {preSubmitPreview.projectedOffsetNotionalText}
                        </dd>
                      </>
                    ) : null}
                    {preSubmitPreview.projectedPendingRemainingNotionalText !== null ? (
                      <>
                        <dt>预计待核销剩余面额</dt>
                        <dd data-testid="preview-pending-remaining">
                          {preSubmitPreview.projectedPendingRemainingNotionalText}
                        </dd>
                      </>
                    ) : null}
                    {preSubmitPreview.projectedEarmarkRemainingNotionalText !== null ? (
                      <>
                        <dt>预计目标占仓剩余面额</dt>
                        <dd data-testid="preview-earmark-remaining">
                          {preSubmitPreview.projectedEarmarkRemainingNotionalText}
                        </dd>
                      </>
                    ) : null}
                    {preSubmitPreview.projectedBaselineDeltaNotionalText !== null ? (
                      <>
                        <dt>提交基线面额差额</dt>
                        <dd data-testid="preview-baseline-delta-notional">
                          {preSubmitPreview.projectedBaselineDeltaNotionalText}
                        </dd>
                      </>
                    ) : null}
                    <dt>预计核销分摊现金影响</dt>
                    <dd
                      data-testid="preview-cash-impact"
                      data-cash-impact-kind={preSubmitPreview.cashImpactKind}
                    >
                      {preSubmitPreview.cashImpactText}
                    </dd>
                  </dl>
                </div>
              ) : null}

              {validationIssues.length > 0 ? (
                <div role="alert" data-testid="pending-match-validation-errors">
                  <ul>
                    {validationIssues.map((issue, idx) => (
                      <li key={`${issue.field}-${idx}`}>
                        {issue.field}: {issue.message}
                      </li>
                    ))}
                  </ul>
                </div>
              ) : null}

              {submitError ? (
                <div role="alert" data-testid="pending-match-submit-error">
                  <strong>提交失败 ({submitError.kind})：</strong>
                  <span>{submitError.message}</span>
                  {submitError.kind === 'transport' ? (
                    <span>（已保留原幂等键，可直接点击重试安全重发）</span>
                  ) : null}
                </div>
              ) : null}

              <div>
                <button
                  type="submit"
                  disabled={submitting || !availability[activeAction].allowed}
                  data-testid="submit-resolve-pending-match"
                >
                  {submitting ? '提交中...' : '提交核销'}
                </button>
              </div>
            </form>
          ) : null}
        </div>
      )}

      <section
        aria-label="提交后按事务归属的分摊现金明细"
        data-testid="post-submit-attribution-section"
      >
        <h3>提交后按事务归属的分摊现金明细</h3>
        {sessionSubmissions.length === 0 ? (
          <p data-testid="no-submissions-yet">本会话暂无已提交的核销事务。</p>
        ) : (
          sessionSubmissions.map((sub) => {
            const attribution = resolveSubmissionAttribution(sub, state);
            return (
              <SubmissionAttributionCard
                key={sub.receiptId}
                submission={sub}
                attribution={attribution}
                onManualLookup={(target) => {
                  void executeReadOnlyLookup(target);
                }}
              />
            );
          })
        )}
      </section>

      {snapshot.fundingSummary ? (
        <aside
          aria-label="账户资金汇总参考"
          data-testid="account-funding-snapshot-ref"
          data-snapshot-seq={snapshot.snapshotSeq.toString()}
        >
          <h4>账户资金汇总参考（快照序号 seq={snapshot.snapshotSeq.toString()}）</h4>
          <p>以下为账户当前快照水位汇总，后续无关余额变动不归因于单次核销事务。</p>
          <dl>
            <dt>预计资金 T+0</dt>
            <dd data-testid="funding-ref-projected-t0">
              {formatOptionalSignedDecimal(snapshot.fundingSummary.projectedCashT0, '未提供')}
            </dd>
            <dt>预计资金 T+1</dt>
            <dd data-testid="funding-ref-projected-t1">
              {formatOptionalSignedDecimal(snapshot.fundingSummary.projectedCashT1, '未提供')}
            </dd>
            <dt>待核销现金 T+0</dt>
            <dd data-testid="funding-ref-pending-t0">
              {formatOptionalSignedDecimal(snapshot.fundingSummary.pendingMatchCashT0, '未提供')}
            </dd>
            <dt>待核销现金 T+1</dt>
            <dd data-testid="funding-ref-pending-t1">
              {formatOptionalSignedDecimal(snapshot.fundingSummary.pendingMatchCashT1, '未提供')}
            </dd>
          </dl>
        </aside>
      ) : null}
    </section>
  );
}
