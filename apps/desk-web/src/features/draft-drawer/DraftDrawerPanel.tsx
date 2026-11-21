import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useStore } from 'zustand';
import {
  DraftEarmarkStatus,
  PrincipalRole,
  type DraftEarmarkRecord,
} from '../../generated/ficant/blotter/v1/model_pb';
import type {
  ConfirmDraftEarmarkResult,
  SubmitDraftEarmarkResult,
} from '../../generated/ficant/blotter/v1/service_pb';
import type { NlpEarmarkProposal } from '../../generated/ficant/blotter/v1/spi_pb';
import {
  buildHighlightedSegments,
  formatDecimalValue,
  formatUnitRef,
  type HighlightedTextSegment,
} from '../../codec';
import {
  selectAccountSnapshot,
  selectIsWritesPaused,
  selectSelectedSnapshot,
  type BlotterStoreApi,
  type BlotterSupervisor,
} from '../../store';
import {
  BlotterClientError,
  WriteIntentManager,
  generateUuidV4,
  hasSecureRandom,
  type DeskBlotterRpcClient,
} from '../../transport';
import {
  classifyRegulatoryError,
  evaluateDraftConfirmationGate,
  extractQuoteFormValues,
  formatDraftStatus,
  formatRuleLineageSummary,
  formatSettlementBucket,
  formatTargetState,
  formatTradeSide,
  validateProposalForSubmission,
  validateRuleLineage,
  type DraftLifecycleStage,
  type DraftQuoteFormValues,
  type QuoteInputMode,
  type RegulatoryBlockInfo,
} from './draftLogic';

export interface DraftDrawerPanelProps {
  readonly store: BlotterStoreApi;
  readonly client: DeskBlotterRpcClient;
  readonly writeIntents?: WriteIntentManager | undefined;
  readonly supervisor?: Pick<BlotterSupervisor, 'triggerCatchUp'> | undefined;
  readonly accountId?: string | undefined;
  readonly isOpen?: boolean | undefined;
  readonly onClose?: (() => void) | undefined;
  readonly focusTrigger?: number | undefined;
  readonly inputRef?: React.RefObject<HTMLTextAreaElement | null> | undefined;
  readonly autoParseOnPaste?: boolean | undefined;
  readonly onRefreshContext?: (() => Promise<void>) | undefined;
  readonly parserRequestIdFactory?: (() => string) | undefined;
}

interface ConfirmReceiptSummary {
  readonly committedSeq: bigint;
  readonly replayed: boolean;
  readonly earmarkId: string;
  readonly draftId: string;
  readonly targetStateLabel: string;
}

const EMPTY_QUOTE_FORM: DraftQuoteFormValues = {
  quoteMode: 'none',
  cleanPricePer100: '',
  yieldToMaturity: '',
  settlementAmount: '',
};

export function DraftDrawerPanel(props: DraftDrawerPanelProps): React.JSX.Element | null {
  const {
    store,
    client,
    writeIntents: externalWriteIntents,
    supervisor,
    accountId: explicitAccountId,
    isOpen = true,
    onClose,
    focusTrigger = 0,
    inputRef: externalInputRef,
    autoParseOnPaste = true,
    onRefreshContext,
    parserRequestIdFactory,
  } = props;

  const snapshot = useStore(
    store,
    explicitAccountId !== undefined
      ? (state) => selectAccountSnapshot(state, explicitAccountId)
      : selectSelectedSnapshot,
  );
  const selectedAccountId = useStore(store, (state) => state.selectedAccountId);
  const sessionGeneration = useStore(store, (state) => state.sessionGeneration);
  const roleHint = useStore(store, (state) => state.roleHint);
  const writesPaused = useStore(store, selectIsWritesPaused);

  const activeAccountId = explicitAccountId ?? snapshot?.accountId ?? selectedAccountId;
  const openingBaseUnits = snapshot?.openingBase?.units;

  const internalWriteIntentsRef = useRef<WriteIntentManager | null>(null);
  if (internalWriteIntentsRef.current === null) {
    internalWriteIntentsRef.current = new WriteIntentManager();
  }
  const writeIntents = externalWriteIntents ?? internalWriteIntentsRef.current;

  const internalTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const requestCounterRef = useRef<number>(0);
  const activeParseEpochRef = useRef<number>(0);
  const submitAttemptRef = useRef<number>(0);
  const prevSessionGenRef = useRef<number>(sessionGeneration);

  const [rawText, setRawText] = useState<string>('');
  const [inputSource, setInputSource] = useState<'none' | 'explicit_paste' | 'manual'>('none');
  const [parsing, setParsing] = useState<boolean>(false);
  const [parseError, setParseError] = useState<string | null>(null);

  const [parsedProposal, setParsedProposal] = useState<NlpEarmarkProposal | null>(null);
  const [quoteForm, setQuoteForm] = useState<DraftQuoteFormValues>(EMPTY_QUOTE_FORM);
  const [quoteDirty, setQuoteDirty] = useState<boolean>(false);

  const [submitting, setSubmitting] = useState<boolean>(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [regulatoryBlock, setRegulatoryBlock] = useState<RegulatoryBlockInfo | null>(null);

  const [activeDraft, setActiveDraft] = useState<DraftEarmarkRecord | null>(null);
  const [previewInvalidated, setPreviewInvalidated] = useState<boolean>(false);
  const [conflictNotice, setConflictNotice] = useState<string | null>(null);
  const [acknowledgeUncoveredDv01, setAcknowledgeUncoveredDv01] = useState<boolean>(false);

  const [confirming, setConfirming] = useState<boolean>(false);
  const [confirmError, setConfirmError] = useState<string | null>(null);
  const [confirmReceipt, setConfirmReceipt] = useState<ConfirmReceiptSummary | null>(null);

  // Synchronously reset all local draft state when sessionGeneration changes (credential switch / logout).
  useEffect(() => {
    if (prevSessionGenRef.current !== sessionGeneration) {
      prevSessionGenRef.current = sessionGeneration;
      activeParseEpochRef.current += 1;
      setRawText('');
      setInputSource('none');
      setParsing(false);
      setParseError(null);
      setParsedProposal(null);
      setQuoteForm(EMPTY_QUOTE_FORM);
      setQuoteDirty(false);
      setSubmitting(false);
      setSubmitError(null);
      setRegulatoryBlock(null);
      setActiveDraft(null);
      setPreviewInvalidated(false);
      setConflictNotice(null);
      setAcknowledgeUncoveredDv01(false);
      setConfirming(false);
      setConfirmError(null);
      setConfirmReceipt(null);
      if (externalWriteIntents === undefined) {
        writeIntents.clearForSessionSwitch(sessionGeneration);
      }
    }
  }, [sessionGeneration, externalWriteIntents, writeIntents]);

  // Focus the explicit paste textarea when drawer opens or focusTrigger increments (e.g. T-007 Ctrl+V).
  useEffect(() => {
    if (!isOpen) {
      return;
    }
    const target = internalTextareaRef.current;
    if (target) {
      target.focus();
    }
  }, [isOpen, focusTrigger]);

  const setCombinedTextareaRef = useCallback(
    (node: HTMLTextAreaElement | null) => {
      internalTextareaRef.current = node;
      if (externalInputRef) {
        externalInputRef.current = node;
      }
    },
    [externalInputRef],
  );

  const nextParseRequestId = useCallback((): string => {
    requestCounterRef.current += 1;
    if (parserRequestIdFactory) {
      return parserRequestIdFactory();
    }
    if (hasSecureRandom()) {
      return generateUuidV4();
    }
    return `nlp-parse-s${sessionGeneration}-r${requestCounterRef.current}`;
  }, [parserRequestIdFactory, sessionGeneration]);

  const invalidateProposalAndDraft = useCallback(
    (nextText: string) => {
      setParsedProposal(null);
      setQuoteForm(EMPTY_QUOTE_FORM);
      setQuoteDirty(false);
      setParseError(null);
      setSubmitError(null);
      setRegulatoryBlock(null);
      setActiveDraft(null);
      setPreviewInvalidated(false);
      setConflictNotice(null);
      setAcknowledgeUncoveredDv01(false);
      setConfirmError(null);
      setConfirmReceipt(null);
      store.getState().setDraftState(sessionGeneration, {
        draftInputText: nextText,
        parsedProposal: null,
        activeDraftConfirmation: null,
      });
    },
    [sessionGeneration, store],
  );

  const runParse = useCallback(
    async (textToParse: string): Promise<void> => {
      const trimmed = textToParse.trim();
      if (trimmed.length === 0) {
        setParseError('请先显式粘贴或输入交易原文后再解析');
        return;
      }
      if (!activeAccountId) {
        setParseError('未选中账户，请先加载并选择账户');
        return;
      }

      const captureSessionGen = store.getState().sessionGeneration;
      const parseEpoch = activeParseEpochRef.current + 1;
      activeParseEpochRef.current = parseEpoch;

      setParsing(true);
      setParseError(null);
      setSubmitError(null);
      setRegulatoryBlock(null);
      setActiveDraft(null);
      setPreviewInvalidated(false);
      setConflictNotice(null);
      setAcknowledgeUncoveredDv01(false);
      setConfirmError(null);
      setConfirmReceipt(null);
      store.getState().setDraftState(captureSessionGen, {
        draftInputText: textToParse,
        parsedProposal: null,
        activeDraftConfirmation: null,
      });

      try {
        const proposal = await client.parseNlpIntent({
          requestId: nextParseRequestId(),
          accountId: activeAccountId,
          rawSourceText: textToParse,
        });

        if (
          store.getState().sessionGeneration !== captureSessionGen ||
          activeParseEpochRef.current !== parseEpoch
        ) {
          return;
        }

        setParsedProposal(proposal);
        setQuoteForm(extractQuoteFormValues(proposal.quoteTerms));
        setQuoteDirty(false);
        store.getState().setDraftState(captureSessionGen, {
          draftInputText: textToParse,
          parsedProposal: proposal,
          activeDraftConfirmation: null,
        });
      } catch (err) {
        if (
          store.getState().sessionGeneration !== captureSessionGen ||
          activeParseEpochRef.current !== parseEpoch
        ) {
          return;
        }
        const clientErr = BlotterClientError.fromUnknown(err);
        setParseError(clientErr.message);
      } finally {
        if (
          store.getState().sessionGeneration === captureSessionGen &&
          activeParseEpochRef.current === parseEpoch
        ) {
          setParsing(false);
        }
      }
    },
    [activeAccountId, client, nextParseRequestId, store],
  );

  // Explicit paste handler: reads ONLY from the browser's PasteEvent.clipboardData.
  // Never performs silent background clipboard reads.
  const handleExplicitPaste = useCallback(
    (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
      const pastedText = event.clipboardData?.getData('text/plain') ?? '';
      if (pastedText.length === 0) {
        return;
      }
      event.preventDefault();
      const target = event.currentTarget;
      const start = target.selectionStart ?? rawText.length;
      const end = target.selectionEnd ?? rawText.length;
      const nextText = `${rawText.slice(0, start)}${pastedText}${rawText.slice(end)}`;
      setRawText(nextText);
      setInputSource('explicit_paste');
      invalidateProposalAndDraft(nextText);

      if (autoParseOnPaste && activeAccountId !== null) {
        void runParse(nextText);
      }
    },
    [activeAccountId, autoParseOnPaste, invalidateProposalAndDraft, rawText, runParse],
  );

  const handleManualChange = useCallback(
    (event: React.ChangeEvent<HTMLTextAreaElement>) => {
      const nextText = event.target.value;
      setRawText(nextText);
      setInputSource(nextText.length > 0 ? 'manual' : 'none');
      invalidateProposalAndDraft(nextText);
    },
    [invalidateProposalAndDraft],
  );

  const handleDrawerKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLElement>) => {
      if (event.key === 'Escape' && onClose) {
        event.stopPropagation();
        onClose();
      }
    },
    [onClose],
  );

  // Validate UTF-8 anchors & build safe React text-node segments for highlighting.
  const anchorHighlightResult = useMemo(():
    | { readonly ok: true; readonly segments: readonly HighlightedTextSegment[] }
    | { readonly ok: false; readonly error: string }
    | null => {
    if (!parsedProposal) {
      return null;
    }
    try {
      const segments = buildHighlightedSegments(
        parsedProposal.rawSourceText,
        parsedProposal.anchors,
      );
      return { ok: true, segments };
    } catch (err) {
      return {
        ok: false,
        error: err instanceof Error ? err.message : 'UTF-8 原文锚点校验失败',
      };
    }
  }, [parsedProposal]);

  const proposalValidation = useMemo(() => {
    if (!parsedProposal || !activeAccountId) {
      return null;
    }
    return validateProposalForSubmission(
      parsedProposal,
      quoteForm,
      quoteDirty,
      openingBaseUnits,
      activeAccountId,
    );
  }, [activeAccountId, openingBaseUnits, parsedProposal, quoteDirty, quoteForm]);

  const submitRoleBlocked =
    roleHint !== null &&
    roleHint !== PrincipalRole.TRADER &&
    roleHint !== PrincipalRole.NLP_PARSER;

  const traderRoleBlocked = roleHint !== null && roleHint !== PrincipalRole.TRADER;

  const executeSubmitDraft = useCallback(
    async (options: { readonly isResubmit?: boolean } = {}): Promise<void> => {
      if (!parsedProposal || !activeAccountId) {
        return;
      }

      // Re-validate strictly before issuing any RPC.
      const checked = validateProposalForSubmission(
        parsedProposal,
        quoteForm,
        quoteDirty,
        openingBaseUnits,
        activeAccountId,
      );
      if (!checked.ok) {
        setSubmitError(checked.issues.map((i) => `${i.field}: ${i.message}`).join('；'));
        return;
      }

      const captureSessionGen = store.getState().sessionGeneration;
      setSubmitting(true);
      setSubmitError(null);
      setRegulatoryBlock(null);
      setConfirmError(null);
      setConfirmReceipt(null);
      // Always reset checkbox and invalidate prior confirmation token on any submit / resubmit.
      setAcknowledgeUncoveredDv01(false);
      store.getState().setDraftState(captureSessionGen, {
        activeDraftConfirmation: null,
      });

      if (options.isResubmit && onRefreshContext) {
        try {
          await onRefreshContext();
        } catch (refreshErr) {
          if (store.getState().sessionGeneration !== captureSessionGen) {
            return;
          }
          const clientErr = BlotterClientError.fromUnknown(refreshErr);
          setSubmitError(`刷新上下文失败: ${clientErr.message}`);
          setSubmitting(false);
          return;
        }
      }

      if (store.getState().sessionGeneration !== captureSessionGen) {
        return;
      }

      const currentAttempt = submitAttemptRef.current;
      let intentKey: string;
      try {
        const intent = writeIntents.acquire({
          operation: 'SubmitDraftEarmark',
          sessionGeneration: captureSessionGen,
          canonicalPayload: {
            accountId: activeAccountId,
            attempt: currentAttempt,
            proposal: checked.proposal,
          },
        });
        intentKey = intent.idempotencyKey;
      } catch (err) {
        setSubmitError(err instanceof Error ? err.message : '无法分配安全幂等键');
        setSubmitting(false);
        return;
      }

      try {
        const result: SubmitDraftEarmarkResult = await client.submitDraftEarmark({
          idempotencyKey: intentKey,
          accountId: activeAccountId,
          proposal: checked.proposal,
        });

        if (store.getState().sessionGeneration !== captureSessionGen) {
          return;
        }

        writeIntents.complete('SubmitDraftEarmark', intentKey);
        submitAttemptRef.current += 1;

        const draft = result.draft;
        store.getState().recordWriteReceipt(captureSessionGen, {
          id: `submit-draft-${intentKey}`,
          operation: 'SubmitDraftEarmark',
          accountId: activeAccountId,
          committedSeq: result.committedSeq,
          replayed: result.replayed,
          status: 'succeeded',
          ...(draft?.draftId?.value !== undefined ? { draftId: draft.draftId.value } : {}),
          recordedAtMs: Date.now(),
        });
        supervisor?.triggerCatchUp();

        if (!draft) {
          setSubmitError('服务端返回空 DraftEarmarkRecord');
          setActiveDraft(null);
          return;
        }

        setActiveDraft(draft);
        setPreviewInvalidated(false);
        setConflictNotice(null);
        setAcknowledgeUncoveredDv01(false);

        const lineageCheck = validateRuleLineage(draft.ruleLineage);
        const tokenBytes = draft.confirmationToken?.value;
        if (
          lineageCheck.ok &&
          draft.draftId?.value &&
          tokenBytes !== undefined &&
          tokenBytes.byteLength === 32
        ) {
          store.getState().setDraftState(captureSessionGen, {
            activeDraftConfirmation: {
              draftId: draft.draftId.value,
              accountId: draft.accountId,
              confirmationTokenBytes: tokenBytes,
              previewAtSeq: draft.previewAtSeq,
            },
          });
        } else {
          store.getState().setDraftState(captureSessionGen, {
            activeDraftConfirmation: null,
          });
        }
      } catch (err) {
        if (store.getState().sessionGeneration !== captureSessionGen) {
          return;
        }
        const clientErr = BlotterClientError.fromUnknown(err);
        if (!clientErr.retryable) {
          writeIntents.complete('SubmitDraftEarmark', intentKey);
        }
        const regBlock = classifyRegulatoryError(clientErr);
        if (regBlock) {
          setRegulatoryBlock(regBlock);
          setActiveDraft(null);
          setAcknowledgeUncoveredDv01(false);
          store.getState().setDraftState(captureSessionGen, {
            activeDraftConfirmation: null,
          });
        }
        setSubmitError(clientErr.message);
      } finally {
        if (store.getState().sessionGeneration === captureSessionGen) {
          setSubmitting(false);
        }
      }
    },
    [
      activeAccountId,
      client,
      onRefreshContext,
      openingBaseUnits,
      parsedProposal,
      quoteDirty,
      quoteForm,
      store,
      supervisor,
      writeIntents,
    ],
  );

  const confirmationGate = useMemo(() => {
    if (!activeDraft) {
      return null;
    }
    return evaluateDraftConfirmationGate({
      draft: activeDraft,
      openingBaseUnits,
      previewInvalidated,
      regulatoryBlock,
      acknowledgeUncoveredDv01,
      writesPaused,
      traderRoleBlocked,
      confirming,
    });
  }, [
    acknowledgeUncoveredDv01,
    activeDraft,
    confirming,
    openingBaseUnits,
    previewInvalidated,
    regulatoryBlock,
    traderRoleBlocked,
    writesPaused,
  ]);

  const handleConfirmDraft = useCallback(async (): Promise<void> => {
    if (!activeDraft || !confirmationGate || !confirmationGate.canConfirm) {
      return;
    }
    if (!activeDraft.draftId || !activeDraft.confirmationToken) {
      return;
    }

    const captureSessionGen = store.getState().sessionGeneration;
    const latestSnapshot = store.getState().snapshots[activeDraft.accountId] ?? snapshot;
    const matchingPos = latestSnapshot?.positions.find(
      (p) => p.instrumentCode === activeDraft.instrumentCode,
    );
    const expectedPositionVersion = matchingPos?.positionVersion ?? 0n;
    const ackFlag = confirmationGate.showUncoveredAckCheckbox ? acknowledgeUncoveredDv01 : false;

    let intentKey: string;
    try {
      const intent = writeIntents.acquire({
        operation: 'ConfirmDraftEarmark',
        sessionGeneration: captureSessionGen,
        canonicalPayload: {
          accountId: activeDraft.accountId,
          draftId: activeDraft.draftId.value,
          expectedDraftRevision: activeDraft.draftRevision,
          expectedPositionVersion,
          confirmationToken: activeDraft.confirmationToken.value,
          acknowledgeUncoveredDv01: ackFlag,
        },
      });
      intentKey = intent.idempotencyKey;
    } catch (err) {
      setConfirmError(err instanceof Error ? err.message : '无法分配安全幂等键');
      return;
    }

    setConfirming(true);
    setConfirmError(null);

    try {
      // Strictly NO manual TTL field on ConfirmDraftEarmarkRequest (TTL is policy-driven on server).
      const result: ConfirmDraftEarmarkResult = await client.confirmDraftEarmark({
        idempotencyKey: intentKey,
        accountId: activeDraft.accountId,
        draftId: activeDraft.draftId,
        expectedDraftRevision: activeDraft.draftRevision,
        expectedPositionVersion,
        confirmationToken: activeDraft.confirmationToken,
        acknowledgeUncoveredDv01: ackFlag,
      });

      if (store.getState().sessionGeneration !== captureSessionGen) {
        return;
      }

      writeIntents.complete('ConfirmDraftEarmark', intentKey);

      store.getState().recordWriteReceipt(captureSessionGen, {
        id: `confirm-draft-${intentKey}`,
        operation: 'ConfirmDraftEarmark',
        accountId: activeDraft.accountId,
        committedSeq: result.committedSeq,
        replayed: result.replayed,
        status: 'succeeded',
        draftId: activeDraft.draftId.value,
        ...(result.earmark?.earmarkId?.value !== undefined
          ? { earmarkId: result.earmark.earmarkId.value }
          : {}),
        ...(result.earmark !== undefined ? { earmark: result.earmark } : {}),
        recordedAtMs: Date.now(),
      });
      store.getState().setDraftState(captureSessionGen, {
        activeDraftConfirmation: null,
      });
      supervisor?.triggerCatchUp();

      setAcknowledgeUncoveredDv01(false);
      setPreviewInvalidated(false);
      setConflictNotice(null);
      if (result.draft) {
        setActiveDraft(result.draft);
      }
      setConfirmReceipt({
        committedSeq: result.committedSeq,
        replayed: result.replayed,
        earmarkId: result.earmark?.earmarkId?.value ?? '未提供',
        draftId: result.draft?.draftId?.value ?? activeDraft.draftId.value,
        targetStateLabel: formatTargetState(result.draft?.targetState ?? activeDraft.targetState),
      });
    } catch (err) {
      if (store.getState().sessionGeneration !== captureSessionGen) {
        return;
      }
      const clientErr = BlotterClientError.fromUnknown(err);

      if (clientErr.isConcurrencyConflict()) {
        // CRITICAL AC-008: Immediately invalidate old preview + confirmation token,
        // reset acknowledgement checkbox to false, and require explicit resubmit + re-confirm!
        writeIntents.complete('ConfirmDraftEarmark', intentKey);
        setPreviewInvalidated(true);
        setAcknowledgeUncoveredDv01(false);
        setConflictNotice(
          `并发冲突 (CONCURRENCY_CONFLICT)：市场行情、持仓版本或监管规则已变化，旧预览与确认令牌已作废。请点击“刷新并重新提交草案”，查看最新预览后再次人工确认。(${clientErr.message})`,
        );
        store.getState().setDraftState(captureSessionGen, {
          activeDraftConfirmation: null,
        });
        setConfirmError(clientErr.message);
        return;
      }

      if (!clientErr.retryable) {
        writeIntents.complete('ConfirmDraftEarmark', intentKey);
      }

      const regBlock = classifyRegulatoryError(clientErr);
      if (regBlock) {
        setRegulatoryBlock(regBlock);
        setAcknowledgeUncoveredDv01(false);
        store.getState().setDraftState(captureSessionGen, {
          activeDraftConfirmation: null,
        });
      }
      setConfirmError(clientErr.message);
    } finally {
      if (store.getState().sessionGeneration === captureSessionGen) {
        setConfirming(false);
      }
    }
  }, [
    acknowledgeUncoveredDv01,
    activeDraft,
    client,
    confirmationGate,
    snapshot,
    store,
    supervisor,
    writeIntents,
  ]);

  const handleSelectSnapshotPendingDraft = useCallback(
    (draft: DraftEarmarkRecord) => {
      setActiveDraft(draft);
      setPreviewInvalidated(false);
      setConflictNotice(null);
      setRegulatoryBlock(null);
      setConfirmError(null);
      setConfirmReceipt(null);
      setAcknowledgeUncoveredDv01(false);

      const lineageCheck = validateRuleLineage(draft.ruleLineage);
      const tokenBytes = draft.confirmationToken?.value;
      if (
        lineageCheck.ok &&
        draft.draftId?.value &&
        tokenBytes !== undefined &&
        tokenBytes.byteLength === 32
      ) {
        store.getState().setDraftState(sessionGeneration, {
          activeDraftConfirmation: {
            draftId: draft.draftId.value,
            accountId: draft.accountId,
            confirmationTokenBytes: tokenBytes,
            previewAtSeq: draft.previewAtSeq,
          },
        });
      } else {
        store.getState().setDraftState(sessionGeneration, {
          activeDraftConfirmation: null,
        });
      }
    },
    [sessionGeneration, store],
  );

  const handleQuoteModeChange = useCallback((nextMode: QuoteInputMode) => {
    setQuoteDirty(true);
    setQuoteForm((prev) => ({ ...prev, quoteMode: nextMode }));
  }, []);

  const handleQuoteFieldChange = useCallback(
    (field: 'cleanPricePer100' | 'yieldToMaturity' | 'settlementAmount', value: string) => {
      setQuoteDirty(true);
      setQuoteForm((prev) => ({ ...prev, [field]: value }));
    },
    [],
  );

  if (!isOpen) {
    return null;
  }

  const currentStage: DraftLifecycleStage = confirmReceipt
    ? 'confirmed'
    : activeDraft
      ? previewInvalidated
        ? 'conflict_invalidated'
        : 'draft_pending'
      : parsedProposal
        ? 'parsed'
        : 'idle';

  const snapshotPendingDrafts = (snapshot?.pendingDrafts ?? []).filter(
    (d) => d.status === DraftEarmarkStatus.PENDING_CONFIRMATION,
  );

  const canSubmitDraft =
    parsedProposal !== null &&
    proposalValidation !== null &&
    proposalValidation.ok &&
    !submitting &&
    !writesPaused &&
    !submitRoleBlocked;

  const lineageSummary = activeDraft ? formatRuleLineageSummary(activeDraft.ruleLineage) : null;

  return (
    <aside
      aria-label="显式粘贴与草案确认抽屉"
      data-testid="draft-drawer-panel"
      data-snapshot-seq={snapshot ? snapshot.snapshotSeq.toString() : undefined}
      data-account-id={activeAccountId ?? undefined}
      data-draft-stage={currentStage}
      onKeyDown={handleDrawerKeyDown}
    >
      <header>
        <div>
          <h2>文本粘贴与草案人工再确认</h2>
          <p>
            账户: <strong data-testid="draft-active-account">{activeAccountId ?? '未选择'}</strong>
            {' · '}
            快照序号:{' '}
            <strong data-testid="draft-snapshot-seq">
              {snapshot ? snapshot.snapshotSeq.toString() : '—'}
            </strong>
          </p>
        </div>
        {onClose ? (
          <button
            type="button"
            data-testid="draft-drawer-close"
            onClick={onClose}
            aria-label="关闭草案抽屉"
          >
            关闭 (Esc)
          </button>
        ) : null}
      </header>

      {/* 3-stage ledger boundary status comparing Parse -> Draft Pending -> Confirmed */}
      <section aria-label="三阶段账务边界对照" data-testid="draft-three-stage-status">
        <h3>三阶段状态对照</h3>
        <div>
          <span data-testid="stage-snapshot-seq">
            当前 snapshot_seq: {snapshot ? snapshot.snapshotSeq.toString() : '—'}
          </span>
          {' | '}
          <span data-testid="stage-active-earmarks-count">
            活动占仓 active_earmarks: {snapshot ? snapshot.activeEarmarks.length : 0}
          </span>
          {' | '}
          <span data-testid="stage-pending-drafts-count">
            待确认草案 pending_drafts: {snapshotPendingDrafts.length}
          </span>
        </div>
        <ol>
          <li data-stage-active={currentStage === 'parsed' ? 'true' : 'false'}>
            <strong>阶段 1 · ParseNlpIntent (只读解析)</strong>：不改变 snapshot_seq，不产生任何占仓。
          </li>
          <li
            data-stage-active={
              currentStage === 'draft_pending' || currentStage === 'conflict_invalidated'
                ? 'true'
                : 'false'
            }
          >
            <strong>阶段 2 · SubmitDraftEarmark (待确认草案)</strong>：仅登记待确认草案并计算 PRE_TAX 风险预览，不产生占仓。
          </li>
          <li data-stage-active={currentStage === 'confirmed' ? 'true' : 'false'}>
            <strong>阶段 3 · ConfirmDraftEarmark (TRADER 人工确认)</strong>：人工核验预览后才原子创建 HOLD/COMMITTED 占仓。
          </li>
        </ol>
      </section>

      {/* Step 1: Explicit Paste / Manual Input Area */}
      <section aria-label="步骤 1：显式粘贴解析">
        <h3>步骤 1 · 显式粘贴 / 原文解析 (ParseNlpIntent)</h3>
        <label htmlFor="draft-paste-textarea">
          请在此输入框显式粘贴 (Ctrl+V) 或手动输入交易文本（系统不会后台静默读取剪贴板）
        </label>
        <textarea
          id="draft-paste-textarea"
          ref={setCombinedTextareaRef}
          data-testid="draft-paste-textarea"
          rows={4}
          value={rawText}
          onPaste={handleExplicitPaste}
          onChange={handleManualChange}
          placeholder="在此显式粘贴或输入交易意向原文..."
        />
        <div>
          <span data-testid="draft-input-source">
            输入方式：
            {inputSource === 'explicit_paste'
              ? '显式粘贴'
              : inputSource === 'manual'
                ? '手动输入'
                : '等待输入'}
          </span>
          <button
            type="button"
            data-testid="draft-parse-button"
            disabled={parsing || rawText.trim().length === 0 || !activeAccountId}
            onClick={() => {
              void runParse(rawText);
            }}
          >
            {parsing ? '解析中...' : '解析原文 (ParseNlpIntent)'}
          </button>
        </div>

        {parseError ? (
          <div role="alert" data-testid="draft-parse-error">
            解析失败: {parseError}
          </div>
        ) : null}

        {parsedProposal ? (
          <div data-testid="draft-proposal-review">
            <h4>解析提案核对</h4>

            {/* UTF-8 SourceTextAnchor highlighted text */}
            <div data-testid="draft-highlighted-source">
              <strong>原文 UTF-8 锚点高亮：</strong>
              {anchorHighlightResult?.ok ? (
                <p data-testid="draft-highlighted-text">
                  {anchorHighlightResult.segments.map((seg) =>
                    seg.highlighted ? (
                      <mark
                        key={`${seg.utf8Start}-${seg.utf8End}`}
                        data-testid="draft-anchor-highlight"
                        data-anchor-fields={seg.fieldNames.join(',')}
                        title={`字段: ${seg.fieldNames.join(', ')} [${seg.utf8Start}, ${seg.utf8End})`}
                      >
                        {seg.text}
                      </mark>
                    ) : (
                      <span key={`${seg.utf8Start}-${seg.utf8End}`}>{seg.text}</span>
                    ),
                  )}
                </p>
              ) : (
                <div role="alert" data-testid="draft-anchor-error">
                  UTF-8 锚点校验失败: {anchorHighlightResult?.error ?? '未知锚点错误'}
                </div>
              )}
            </div>

            {/* Structured proposal fields */}
            <dl data-testid="draft-proposal-fields">
              <dt>解析源 / 模型</dt>
              <dd>
                {parsedProposal.parserSourceId} / {parsedProposal.modelVersion}
              </dd>
              <dt>账户 / 券种</dt>
              <dd>
                {parsedProposal.accountId} / {parsedProposal.instrumentCode}
              </dd>
              <dt>方向 / 结算分桶 / 目标状态</dt>
              <dd>
                {formatTradeSide(parsedProposal.side)} ·{' '}
                {formatSettlementBucket(parsedProposal.settlementBucket)} ·{' '}
                {formatTargetState(parsedProposal.suggestedTargetState)}
              </dd>
              <dt>面额 (notional)</dt>
              <dd data-testid="proposal-notional">
                {parsedProposal.notional
                  ? (() => {
                      try {
                        return `${formatDecimalValue(parsedProposal.notional, { grouping: true })} (${formatUnitRef(parsedProposal.notional.unit)})`;
                      } catch {
                        return `非法数值 (${formatUnitRef(parsedProposal.notional.unit)})`;
                      }
                    })()
                  : '未提供'}
              </dd>
              <dt>置信度 (confidence_score)</dt>
              <dd data-testid="proposal-confidence">
                {parsedProposal.confidenceScore
                  ? (() => {
                      try {
                        return `${formatDecimalValue(parsedProposal.confidenceScore)} (${formatUnitRef(parsedProposal.confidenceScore.unit)})`;
                      } catch {
                        return `非法数值 (${formatUnitRef(parsedProposal.confidenceScore.unit)})`;
                      }
                    })()
                  : '未提供'}
              </dd>
            </dl>

            {/* Editable QuoteTerms review (supports single positive settlement_amount!) */}
            <fieldset data-testid="draft-quote-editor">
              <legend>报价条款 (QuoteTerms · 支持仅填正结算金额)</legend>
              <label htmlFor="draft-quote-mode">价格/收益率方式 (oneof)</label>
              <select
                id="draft-quote-mode"
                data-testid="draft-quote-mode"
                value={quoteForm.quoteMode}
                onChange={(e) => handleQuoteModeChange(e.target.value as QuoteInputMode)}
              >
                <option value="none">不填价格/收益率（仅结算金额）</option>
                <option value="clean_price">净价 (clean_price_per_100)</option>
                <option value="yield">到期收益率 (yield_to_maturity)</option>
              </select>

              {quoteForm.quoteMode === 'clean_price' ? (
                <div>
                  <label htmlFor="draft-clean-price">百元净价 (&gt; 0)</label>
                  <input
                    id="draft-clean-price"
                    data-testid="draft-clean-price-input"
                    type="text"
                    value={quoteForm.cleanPricePer100}
                    onChange={(e) => handleQuoteFieldChange('cleanPricePer100', e.target.value)}
                  />
                </div>
              ) : null}

              {quoteForm.quoteMode === 'yield' ? (
                <div>
                  <label htmlFor="draft-yield">到期收益率 (&gt;= 0)</label>
                  <input
                    id="draft-yield"
                    data-testid="draft-yield-input"
                    type="text"
                    value={quoteForm.yieldToMaturity}
                    onChange={(e) => handleQuoteFieldChange('yieldToMaturity', e.target.value)}
                  />
                </div>
              ) : null}

              <div>
                <label htmlFor="draft-settlement-amount">结算金额 settlement_amount (&gt; 0)</label>
                <input
                  id="draft-settlement-amount"
                  data-testid="draft-settlement-amount-input"
                  type="text"
                  value={quoteForm.settlementAmount}
                  onChange={(e) => handleQuoteFieldChange('settlementAmount', e.target.value)}
                />
              </div>
            </fieldset>

            {proposalValidation && !proposalValidation.ok ? (
              <div role="alert" data-testid="draft-proposal-issues">
                <strong>提案校验未通过，已阻断提交：</strong>
                <ul>
                  {proposalValidation.issues.map((issue, idx) => (
                    <li key={`${issue.field}-${idx}`}>
                      <code>{issue.field}</code>: {issue.message}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}

            <div>
              <button
                type="button"
                data-testid="draft-submit-button"
                disabled={!canSubmitDraft}
                onClick={() => {
                  void executeSubmitDraft({ isResubmit: false });
                }}
              >
                {submitting ? '提交草案中...' : '提交草案 (SubmitDraftEarmark)'}
              </button>
              {submitRoleBlocked ? (
                <span data-testid="draft-submit-role-hint">
                  当前角色提示不支持提交草案（需 TRADER 或 NLP_PARSER）
                </span>
              ) : null}
            </div>
          </div>
        ) : null}

        {regulatoryBlock ? (
          <div role="alert" data-testid="draft-regulatory-error">
            <strong>监管规则/限额阻断：</strong>
            <span>{regulatoryBlock.summary}</span>
            <ul>
              {regulatoryBlock.details.map((detail, idx) => (
                <li key={idx}>{detail}</li>
              ))}
            </ul>
          </div>
        ) : null}

        {submitError && !regulatoryBlock ? (
          <div role="alert" data-testid="draft-submit-error">
            提交草案失败: {submitError}
          </div>
        ) : null}
      </section>

      {/* Existing pending drafts from snapshot */}
      {snapshotPendingDrafts.length > 0 ? (
        <section aria-label="快照待确认草案列表" data-testid="snapshot-pending-drafts">
          <h3>快照中的待确认草案 ({snapshotPendingDrafts.length})</h3>
          <ul>
            {snapshotPendingDrafts.map((draft) => {
              const id = draft.draftId?.value ?? '';
              const isSelected = activeDraft?.draftId?.value === id;
              return (
                <li key={id}>
                  <button
                    type="button"
                    data-testid={`select-pending-draft-${id}`}
                    aria-pressed={isSelected}
                    onClick={() => handleSelectSnapshotPendingDraft(draft)}
                  >
                    {draft.instrumentCode} · {formatTradeSide(draft.side)} ·{' '}
                    {formatTargetState(draft.targetState)} · preview_seq=
                    {draft.previewAtSeq.toString()}
                  </button>
                </li>
              );
            })}
          </ul>
        </section>
      ) : null}

      {/* Step 2 & 3: Active Draft RuleLineage + Risk Preview adjacent to Confirm button */}
      {activeDraft && confirmationGate ? (
        <section aria-label="步骤 3：草案风险再确认" data-testid="draft-confirmation-section">
          <h3>步骤 3 · 草案人工再确认 (ConfirmDraftEarmark)</h3>
          <p>
            草案 ID: <code data-testid="active-draft-id">{activeDraft.draftId?.value ?? '未提供'}</code>
            {' · '}
            修订版: <span>{activeDraft.draftRevision.toString()}</span>
            {' · '}
            状态: <strong data-testid="active-draft-status">{formatDraftStatus(activeDraft.status)}</strong>
            {' · '}
            目标: <strong>{formatTargetState(activeDraft.targetState)}</strong>
          </p>

          {/* Regulatory RuleLineage */}
          <div data-testid="draft-rule-lineage">
            <strong>监管规则谱系 (RuleLineage)：</strong>
            {lineageSummary && confirmationGate.lineageValid ? (
              <dl>
                <dt>规则包 ID / 版本</dt>
                <dd>
                  <code>{lineageSummary.packId}</code> ({lineageSummary.version})
                </dd>
                <dt>来源 (source)</dt>
                <dd>{lineageSummary.source}</dd>
                <dt>内容摘要 (content_hash)</dt>
                <dd>
                  <code>{lineageSummary.contentHashHex}</code>
                </dd>
                <dt>约束摘要 (parsed_constraints_hash)</dt>
                <dd>
                  <code>{lineageSummary.parsedConstraintsHashHex}</code>
                </dd>
              </dl>
            ) : (
              <div role="alert" data-testid="draft-lineage-invalid">
                监管规则谱系缺失或不完整，禁止确认：
                {confirmationGate.lineageIssues.map((i) => `${i.field}: ${i.message}`).join('；')}
              </div>
            )}
          </div>

          {/* Conflict invalidation banner + explicit Resubmit button */}
          {conflictNotice ? (
            <div role="alert" data-testid="draft-conflict-notice">
              <p>{conflictNotice}</p>
              {parsedProposal ? (
                <button
                  type="button"
                  data-testid="draft-resubmit-button"
                  disabled={submitting || writesPaused}
                  onClick={() => {
                    void executeSubmitDraft({ isResubmit: true });
                  }}
                >
                  {submitting ? '正在重新提交...' : '刷新并重新提交草案'}
                </button>
              ) : null}
            </div>
          ) : null}

          {/* Risk preview strictly adjacent to Confirm button */}
          <div data-testid="draft-confirm-action-row">
            <div data-testid="draft-confirm-preview">
              <strong>确认前风险预览 (税基: PRE_TAX)</strong>
              <div>
                preview_at_seq:{' '}
                <code data-testid="preview-at-seq">{confirmationGate.preview.previewAtSeqText}</code>
                {previewInvalidated ? (
                  <strong data-testid="preview-invalidated-badge"> [已失效 · 需重提]</strong>
                ) : null}
              </div>

              {confirmationGate.preview.kind === 'covered_nonzero' ? (
                <div data-testid="preview-covered-nonzero">
                  <span>
                    PRE_TAX DV01 增量:{' '}
                    <strong data-testid="preview-dv01-value">
                      {confirmationGate.preview.dv01Text}
                    </strong>
                  </span>
                  {' · '}
                  <span>
                    单位:{' '}
                    <code data-testid="preview-dv01-unit">
                      {confirmationGate.preview.dv01UnitText}
                    </code>
                  </span>
                </div>
              ) : null}

              {confirmationGate.preview.kind === 'covered_zero_hold' ? (
                <div data-testid="preview-covered-zero-hold">
                  <span>
                    PRE_TAX DV01 增量:{' '}
                    <strong data-testid="preview-dv01-value">
                      {confirmationGate.preview.dv01Text}
                    </strong>
                  </span>
                  {' · '}
                  <span>
                    单位:{' '}
                    <code data-testid="preview-dv01-unit">
                      {confirmationGate.preview.dv01UnitText}
                    </code>
                  </span>
                  {' · '}
                  <span data-testid="preview-zero-hold-label">已覆盖 HOLD 增量确认为 0（非缺值）</span>
                </div>
              ) : null}

              {confirmationGate.preview.kind === 'covered_missing_delta' ? (
                <div role="alert" data-testid="preview-missing-delta">
                  <span>
                    PRE_TAX DV01 增量: <strong data-testid="preview-dv01-value">未提供</strong>
                  </span>
                  {' · '}
                  <span>{confirmationGate.preview.missingReason}</span>
                </div>
              ) : null}

              {confirmationGate.preview.kind === 'uncovered_valid' ? (
                <div data-testid="preview-uncovered-valid">
                  <div>
                    PRE_TAX DV01 增量: <span data-testid="preview-dv01-value">未提供（未覆盖）</span>
                  </div>
                  <div>
                    未覆盖原因:{' '}
                    <strong data-testid="preview-uncovered-reason">
                      {confirmationGate.preview.uncoveredReasonLabel} (
                      {confirmationGate.preview.uncoveredReasonCode})
                    </strong>
                  </div>
                  <div>
                    未覆盖面额增量:{' '}
                    <strong data-testid="preview-uncovered-notional">
                      {confirmationGate.preview.deltaNotionalText}
                    </strong>
                    {' · '}
                    单位:{' '}
                    <code data-testid="preview-uncovered-unit">
                      {confirmationGate.preview.deltaNotionalUnitText}
                    </code>
                    {confirmationGate.preview.deltaNotionalIsZero ? (
                      <span>（HOLD 增量确认为 0）</span>
                    ) : null}
                  </div>
                </div>
              ) : null}

              {confirmationGate.preview.kind === 'uncovered_incomplete' ? (
                <div role="alert" data-testid="preview-uncovered-incomplete">
                  <div>
                    未覆盖原因:{' '}
                    <strong data-testid="preview-uncovered-reason">
                      {confirmationGate.preview.uncoveredReasonLabel}
                    </strong>
                  </div>
                  <div>
                    未覆盖面额增量:{' '}
                    <strong data-testid="preview-uncovered-notional">
                      {confirmationGate.preview.deltaNotionalText}
                    </strong>
                  </div>
                  <div>{confirmationGate.preview.missingReason}</div>
                </div>
              ) : null}

              {confirmationGate.preview.kind === 'invalid_preview' ? (
                <div role="alert" data-testid="preview-invalid">
                  {confirmationGate.preview.missingReason}
                </div>
              ) : null}
            </div>

            {/* Uncovered risk acknowledgement checkbox: ONLY rendered for valid uncovered drafts */}
            {confirmationGate.showUncoveredAckCheckbox ? (
              <label data-testid="acknowledge-uncovered-label">
                <input
                  type="checkbox"
                  data-testid="acknowledge-uncovered-checkbox"
                  checked={acknowledgeUncoveredDv01}
                  onChange={(e) => setAcknowledgeUncoveredDv01(e.target.checked)}
                />
                我已核实该券种当前未覆盖 PRE_TAX DV01 原因与未覆盖面额增量，并确认承担未覆盖风险
                (acknowledge_uncovered_dv01)
              </label>
            ) : null}

            <div>
              <button
                type="button"
                data-testid="draft-confirm-button"
                disabled={!confirmationGate.canConfirm}
                onClick={() => {
                  void handleConfirmDraft();
                }}
              >
                {confirming ? '确认中...' : '人工确认草案 (ConfirmDraftEarmark)'}
              </button>
            </div>

            {confirmationGate.blockingReasons.length > 0 && !confirmReceipt ? (
              <ul data-testid="draft-confirm-blocking-reasons">
                {confirmationGate.blockingReasons.map((reason, idx) => (
                  <li key={idx}>{reason}</li>
                ))}
              </ul>
            ) : null}
          </div>

          {confirmError && !conflictNotice && !regulatoryBlock ? (
            <div role="alert" data-testid="draft-confirm-error">
              确认草案失败: {confirmError}
            </div>
          ) : null}

          {confirmReceipt ? (
            <div role="status" data-testid="draft-confirm-receipt">
              <strong>草案确认成功：</strong>
              committed_seq=<code>{confirmReceipt.committedSeq.toString()}</code> · earmark_id=
              <code>{confirmReceipt.earmarkId}</code> · 目标={confirmReceipt.targetStateLabel}
              {confirmReceipt.replayed ? ' (幂等重放)' : ''}
            </div>
          ) : null}
        </section>
      ) : null}
    </aside>
  );
}
