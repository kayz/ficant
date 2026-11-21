import { create, type MessageInitShape } from '@bufbuild/protobuf';
import { useEffect, useState, type FormEvent, type ReactElement } from 'react';
import {
  EarmarkState,
  PrincipalRole,
  SettlementBucket,
  TradeSide,
  type AccountBlotterSnapshot,
  type EarmarkRecord,
  type InstrumentPositionView,
} from '../../generated/ficant/blotter/v1/model_pb';
import {
  CancelCommittedRequestSchema,
  ConfirmHoldRequestSchema,
  CreateCommittedRequestSchema,
  CreateHoldRequestSchema,
  ExtendHoldRequestSchema,
  ReleaseHoldRequestSchema,
} from '../../generated/ficant/blotter/v1/service_pb';
import { createDecimalValue, DecimalValidationError, parseDecimalString } from '../../codec/decimal';
import { createUlid } from '../../codec/primitives';
import {
  MAX_HOLD_TTL_SECONDS,
  MIN_HOLD_TTL_SECONDS,
  validateCancelCommittedRequest,
  validateConfirmHoldRequest,
  validateCreateCommittedRequest,
  validateCreateHoldRequest,
  validateExtendHoldRequest,
  validateReleaseHoldRequest,
  type ValidationIssue,
} from '../../codec/validators';
import type { BlotterStoreApi, WriteReceiptRecord } from '../../store/blotterStore';
import { BlotterClientError } from '../../transport/error';
import type { WriteIntentManager } from '../../transport/idempotency';
import {
  buildQuoteTermsFromInputs,
  formatEarmarkState,
  formatErrorCode,
  formatHoldRemainingTtl,
  formatMarketTimeDisplay,
  formatSettlementBucket,
  formatTradeSide,
  parseStrictTtlSeconds,
  quoteTermsToDraft,
  type QuoteInputDraft,
} from './formatters';
import type {
  HoldActionMode,
  PositionsGridRpcClient,
  QuoteInputMode,
  ReceiptReplayHandle,
  ReplayableHoldRequest,
} from './types';

export interface HoldActionFormProps {
  readonly store: BlotterStoreApi;
  readonly client: PositionsGridRpcClient;
  readonly writeIntents: WriteIntentManager;
  readonly snapshot: AccountBlotterSnapshot;
  readonly sessionGeneration: number;
  readonly authenticated: boolean;
  readonly roleHint: PrincipalRole | null;
  readonly recoveryPausedWrites: boolean;
  readonly recoveryReason: string | null;
  readonly selectedPosition: InstrumentPositionView | undefined;
  readonly selectedEarmark: EarmarkRecord | undefined;
  readonly activeMode: HoldActionMode;
  readonly onModeChange: (mode: HoldActionMode) => void;
  readonly nowMs: () => number;
  readonly nextReceiptId: (sessionGeneration: number) => string;
  readonly onRegisterReplayHandle: (handle: ReceiptReplayHandle) => void;
  readonly onWriteCommitted?: ((committedSeq: bigint) => void) | undefined;
  readonly onResyncRequested?: ((accountId: string) => void | Promise<void>) | undefined;
}

function isTraderOnlyMode(mode: HoldActionMode): boolean {
  return mode !== 'cancel_committed';
}

export function HoldActionForm({
  store,
  client,
  writeIntents,
  snapshot,
  sessionGeneration,
  authenticated,
  roleHint,
  recoveryPausedWrites,
  recoveryReason,
  selectedPosition,
  selectedEarmark,
  activeMode,
  onModeChange,
  nowMs,
  nextReceiptId,
  onRegisterReplayHandle,
  onWriteCommitted,
  onResyncRequested,
}: HoldActionFormProps): ReactElement {
  const [side, setSide] = useState<TradeSide>(TradeSide.BUY);
  const [settlementBucket, setSettlementBucket] = useState<SettlementBucket>(
    SettlementBucket.T_PLUS_0,
  );
  const [notionalText, setNotionalText] = useState<string>('1000000');
  const [ttlText, setTtlText] = useState<string>(String(MIN_HOLD_TTL_SECONDS));
  const [noteText, setNoteText] = useState<string>('');
  const [reasonText, setReasonText] = useState<string>('');
  const [quoteDraft, setQuoteDraft] = useState<QuoteInputDraft>({
    mode: 'omit',
    cleanPriceText: '',
    yieldText: '',
    settlementAmountText: '',
  });
  const [validationIssues, setValidationIssues] = useState<readonly ValidationIssue[]>([]);
  const [submitError, setSubmitError] = useState<BlotterClientError | null>(null);
  const [lastFeedback, setLastFeedback] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState<boolean>(false);

  const selectedEarmarkId = selectedEarmark?.earmarkId?.value ?? '';
  const selectedInstrument = selectedPosition?.instrumentCode ?? '';

  // Reset form validation errors and sync quote defaults when mode or selected earmark changes.
  useEffect(() => {
    setValidationIssues([]);
    setSubmitError(null);
    setLastFeedback(null);
    if (activeMode === 'create_hold') {
      setQuoteDraft({
        mode: 'omit',
        cleanPriceText: '',
        yieldText: '',
        settlementAmountText: '',
      });
    } else if (activeMode === 'create_committed') {
      setQuoteDraft((prev) =>
        prev.mode === 'omit'
          ? { mode: 'settlement_only', cleanPriceText: '', yieldText: '', settlementAmountText: '' }
          : prev,
      );
    } else if (activeMode === 'confirm_hold') {
      setQuoteDraft(quoteTermsToDraft(selectedEarmark?.quoteTerms, false));
    }
  }, [activeMode, selectedEarmarkId, selectedInstrument, selectedEarmark?.quoteTerms]);

  const writesDisabledByState = !authenticated || recoveryPausedWrites;
  const roleBlocksMode =
    roleHint !== null &&
    roleHint !== PrincipalRole.UNSPECIFIED &&
    roleHint !== PrincipalRole.TRADER &&
    (isTraderOnlyMode(activeMode) || roleHint !== PrincipalRole.OPERATOR);

  const units = snapshot.openingBase?.units;

  const canExtendOrRelease =
    selectedEarmark !== undefined && selectedEarmark.state === EarmarkState.HOLD;
  const canConfirm =
    selectedEarmark !== undefined &&
    (selectedEarmark.state === EarmarkState.HOLD ||
      selectedEarmark.state === EarmarkState.EXPIRED);
  const canCancelCommitted =
    selectedEarmark !== undefined && selectedEarmark.state === EarmarkState.COMMITTED;

  const recordReceiptAndReplay = (
    capturedGen: number,
    receipt: WriteReceiptRecord,
    replay?: ReplayableHoldRequest,
  ): void => {
    if (store.getState().sessionGeneration !== capturedGen) {
      return;
    }
    store.getState().recordWriteReceipt(capturedGen, receipt);
    if (replay) {
      onRegisterReplayHandle({
        receiptId: receipt.id,
        sessionGeneration: capturedGen,
        accountId: receipt.accountId,
        replay,
      });
    }
    if (receipt.committedSeq > 0n) {
      onWriteCommitted?.(receipt.committedSeq);
    }
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>): Promise<void> => {
    event.preventDefault();
    if (submitting || writesDisabledByState || roleBlocksMode) {
      return;
    }

    setValidationIssues([]);
    setSubmitError(null);
    setLastFeedback(null);

    const capturedGen = sessionGeneration;
    const accountId = snapshot.accountId;

    if (activeMode === 'create_hold' || activeMode === 'create_committed') {
      if (!selectedPosition) {
        setValidationIssues([{ field: 'instrument_code', message: '请先在持仓表中选择目标券种' }]);
        return;
      }

      const issues: ValidationIssue[] = [];
      let notionalVal = undefined;
      const rawNotional = notionalText.trim();
      if (rawNotional.length === 0) {
        issues.push({ field: 'notional', message: '面额不能为空' });
      } else if (!units?.currencyAmount) {
        issues.push({
          field: 'opening_base.units.currency_amount',
          message: '账户缺少 currency_amount 单位定义',
        });
      } else {
        try {
          const parts = parseDecimalString(rawNotional, 'notional');
          notionalVal = createDecimalValue(parts, units.currencyAmount, 'notional');
        } catch (err) {
          issues.push({
            field: 'notional',
            message: err instanceof DecimalValidationError ? err.message : '面额格式非法',
          });
        }
      }

      const quoteRequired = activeMode === 'create_committed';
      const quoteBuilt = buildQuoteTermsFromInputs(quoteDraft, {
        required: quoteRequired,
        units,
      });
      if (!quoteBuilt.ok) {
        issues.push(...quoteBuilt.issues);
      }

      let parsedTtl = MIN_HOLD_TTL_SECONDS;
      if (activeMode === 'create_hold') {
        const ttlRes = parseStrictTtlSeconds(ttlText, 'ttl_seconds');
        if (!ttlRes.ok) {
          issues.push(ttlRes.issue);
        } else {
          parsedTtl = ttlRes.ttlSeconds;
        }
      }

      if (issues.length > 0) {
        setValidationIssues(issues);
        return;
      }

      const quoteTerms = quoteBuilt.ok ? quoteBuilt.quoteTerms : undefined;

      if (activeMode === 'create_hold') {
        const draftReq = create(CreateHoldRequestSchema, {
          idempotencyKey: '',
          accountId,
          expectedPositionVersion: selectedPosition.positionVersion,
          instrumentCode: selectedPosition.instrumentCode,
          side,
          settlementBucket,
          ...(notionalVal !== undefined ? { notional: notionalVal } : {}),
          ...(quoteTerms !== undefined ? { quoteTerms } : {}),
          ttlSeconds: parsedTtl,
          note: noteText.trim(),
        });

        const reqCheck = validateCreateHoldRequest(draftReq, units);
        if (!reqCheck.ok) {
          setValidationIssues(reqCheck.issues);
          return;
        }

        const slotKey = `CreateHold:${accountId}:${selectedPosition.instrumentCode}`;
        const intent = writeIntents.acquire({
          operation: slotKey,
          sessionGeneration: capturedGen,
          canonicalPayload: {
            accountId,
            instrumentCode: selectedPosition.instrumentCode,
            expectedPositionVersion: selectedPosition.positionVersion,
            side,
            settlementBucket,
            notional: notionalVal,
            quoteTerms,
            ttlSeconds: parsedTtl,
            note: noteText.trim(),
          },
        });

        const requestInit: MessageInitShape<typeof CreateHoldRequestSchema> = {
          idempotencyKey: intent.idempotencyKey,
          accountId,
          expectedPositionVersion: selectedPosition.positionVersion,
          instrumentCode: selectedPosition.instrumentCode,
          side,
          settlementBucket,
          ...(notionalVal !== undefined ? { notional: notionalVal } : {}),
          ...(quoteTerms !== undefined ? { quoteTerms } : {}),
          ttlSeconds: parsedTtl,
          note: noteText.trim(),
        };

        setSubmitting(true);
        try {
          const res = await client.createHold(requestInit);
          if (store.getState().sessionGeneration !== capturedGen) {
            return;
          }
          writeIntents.complete(slotKey, intent.idempotencyKey);
          const rcptId = nextReceiptId(capturedGen);
          recordReceiptAndReplay(
            capturedGen,
            {
              id: rcptId,
              operation: 'CreateHold',
              accountId,
              committedSeq: res.committedSeq,
              replayed: res.replayed,
              status: 'succeeded',
              earmarkId: res.earmark?.earmarkId?.value,
              earmark: res.earmark,
              recordedAtMs: nowMs(),
            },
            {
              operation: 'CreateHold',
              idempotencyKey: intent.idempotencyKey,
              request: requestInit,
            },
          );
          setLastFeedback(
            `CreateHold 已提交 (committed_seq=${res.committedSeq.toString()}, replayed=${String(res.replayed)})`,
          );
        } catch (err) {
          if (store.getState().sessionGeneration !== capturedGen) {
            return;
          }
          const clientErr = BlotterClientError.fromUnknown(err);
          setSubmitError(clientErr);
          if (clientErr.kind === 'business' && !clientErr.retryable) {
            writeIntents.complete(slotKey, intent.idempotencyKey);
          }
          const rcptId = nextReceiptId(capturedGen);
          recordReceiptAndReplay(
            capturedGen,
            {
              id: rcptId,
              operation: 'CreateHold',
              accountId,
              committedSeq: 0n,
              replayed: false,
              status: 'failed',
              error: clientErr,
              recordedAtMs: nowMs(),
            },
            clientErr.retryable || clientErr.kind === 'transport'
              ? {
                  operation: 'CreateHold',
                  idempotencyKey: intent.idempotencyKey,
                  request: requestInit,
                }
              : undefined,
          );
        } finally {
          setSubmitting(false);
        }
        return;
      }

      // CreateCommitted
      const draftReq = create(CreateCommittedRequestSchema, {
        idempotencyKey: '',
        accountId,
        expectedPositionVersion: selectedPosition.positionVersion,
        instrumentCode: selectedPosition.instrumentCode,
        side,
        settlementBucket,
        ...(notionalVal !== undefined ? { notional: notionalVal } : {}),
        ...(quoteTerms !== undefined ? { quoteTerms } : {}),
        note: noteText.trim(),
      });
      const reqCheck = validateCreateCommittedRequest(draftReq, units);
      if (!reqCheck.ok) {
        setValidationIssues(reqCheck.issues);
        return;
      }

      const slotKey = `CreateCommitted:${accountId}:${selectedPosition.instrumentCode}`;
      const intent = writeIntents.acquire({
        operation: slotKey,
        sessionGeneration: capturedGen,
        canonicalPayload: {
          accountId,
          instrumentCode: selectedPosition.instrumentCode,
          expectedPositionVersion: selectedPosition.positionVersion,
          side,
          settlementBucket,
          notional: notionalVal,
          quoteTerms,
          note: noteText.trim(),
        },
      });

      const requestInit: MessageInitShape<typeof CreateCommittedRequestSchema> = {
        idempotencyKey: intent.idempotencyKey,
        accountId,
        expectedPositionVersion: selectedPosition.positionVersion,
        instrumentCode: selectedPosition.instrumentCode,
        side,
        settlementBucket,
        ...(notionalVal !== undefined ? { notional: notionalVal } : {}),
        ...(quoteTerms !== undefined ? { quoteTerms } : {}),
        note: noteText.trim(),
      };

      setSubmitting(true);
      try {
        const res = await client.createCommitted(requestInit);
        if (store.getState().sessionGeneration !== capturedGen) {
          return;
        }
        writeIntents.complete(slotKey, intent.idempotencyKey);
        const rcptId = nextReceiptId(capturedGen);
        recordReceiptAndReplay(
          capturedGen,
          {
            id: rcptId,
            operation: 'CreateCommitted',
            accountId,
            committedSeq: res.committedSeq,
            replayed: res.replayed,
            status: 'succeeded',
            earmarkId: res.earmark?.earmarkId?.value,
            earmark: res.earmark,
            recordedAtMs: nowMs(),
          },
          {
            operation: 'CreateCommitted',
            idempotencyKey: intent.idempotencyKey,
            request: requestInit,
          },
        );
        setLastFeedback(
          `CreateCommitted 已提交 (committed_seq=${res.committedSeq.toString()}, replayed=${String(res.replayed)})`,
        );
      } catch (err) {
        if (store.getState().sessionGeneration !== capturedGen) {
          return;
        }
        const clientErr = BlotterClientError.fromUnknown(err);
        setSubmitError(clientErr);
        const rcptId = nextReceiptId(capturedGen);
        recordReceiptAndReplay(capturedGen, {
          id: rcptId,
          operation: 'CreateCommitted',
          accountId,
          committedSeq: 0n,
          replayed: false,
          status: 'failed',
          error: clientErr,
          recordedAtMs: nowMs(),
        });
      } finally {
        setSubmitting(false);
      }
      return;
    }

    // Earmark-targeted operations
    if (!selectedEarmark || !selectedEarmark.earmarkId) {
      setValidationIssues([{ field: 'earmark_id', message: '请先选择一条占仓记录' }]);
      return;
    }

    let earmarkUlid;
    try {
      earmarkUlid = createUlid(selectedEarmark.earmarkId.value, 'earmark_id');
    } catch {
      setValidationIssues([{ field: 'earmark_id', message: 'earmark_id 不是合法的 26 位 ULID' }]);
      return;
    }
    const earmarkIdStr = selectedEarmark.earmarkId.value;
    const expectedEarmarkRevision = selectedEarmark.earmarkRevision;

    if (activeMode === 'extend_hold') {
      const ttlRes = parseStrictTtlSeconds(ttlText, 'ttl_seconds');
      if (!ttlRes.ok) {
        setValidationIssues([ttlRes.issue]);
        return;
      }
      const draftReq = create(ExtendHoldRequestSchema, {
        idempotencyKey: '',
        accountId,
        earmarkId: earmarkUlid,
        expectedEarmarkRevision,
        ttlSeconds: ttlRes.ttlSeconds,
      });
      const reqCheck = validateExtendHoldRequest(draftReq);
      if (!reqCheck.ok) {
        setValidationIssues(reqCheck.issues);
        return;
      }

      const slotKey = `ExtendHold:${accountId}:${earmarkIdStr}`;
      const intent = writeIntents.acquire({
        operation: slotKey,
        sessionGeneration: capturedGen,
        canonicalPayload: {
          accountId,
          earmarkId: earmarkIdStr,
          expectedEarmarkRevision,
          ttlSeconds: ttlRes.ttlSeconds,
        },
      });

      const requestInit: MessageInitShape<typeof ExtendHoldRequestSchema> = {
        idempotencyKey: intent.idempotencyKey,
        accountId,
        earmarkId: earmarkUlid,
        expectedEarmarkRevision,
        ttlSeconds: ttlRes.ttlSeconds,
      };

      setSubmitting(true);
      try {
        const res = await client.extendHold(requestInit);
        if (store.getState().sessionGeneration !== capturedGen) {
          return;
        }
        writeIntents.complete(slotKey, intent.idempotencyKey);
        const rcptId = nextReceiptId(capturedGen);
        recordReceiptAndReplay(
          capturedGen,
          {
            id: rcptId,
            operation: 'ExtendHold',
            accountId,
            committedSeq: res.committedSeq,
            replayed: res.replayed,
            status: 'succeeded',
            earmarkId: earmarkIdStr,
            earmark: res.earmark,
            recordedAtMs: nowMs(),
          },
          {
            operation: 'ExtendHold',
            idempotencyKey: intent.idempotencyKey,
            request: requestInit,
          },
        );
        setLastFeedback(
          `ExtendHold 已提交 (committed_seq=${res.committedSeq.toString()}, ttl=${ttlRes.ttlSeconds}s)`,
        );
      } catch (err) {
        if (store.getState().sessionGeneration !== capturedGen) {
          return;
        }
        const clientErr = BlotterClientError.fromUnknown(err);
        setSubmitError(clientErr);
        const rcptId = nextReceiptId(capturedGen);
        recordReceiptAndReplay(capturedGen, {
          id: rcptId,
          operation: 'ExtendHold',
          accountId,
          committedSeq: 0n,
          replayed: false,
          status: 'failed',
          earmarkId: earmarkIdStr,
          error: clientErr,
          recordedAtMs: nowMs(),
        });
      } finally {
        setSubmitting(false);
      }
      return;
    }

    if (activeMode === 'release_hold' || activeMode === 'cancel_committed') {
      const trimmedReason = reasonText.trim();
      if (activeMode === 'release_hold') {
        const draftReq = create(ReleaseHoldRequestSchema, {
          idempotencyKey: '',
          accountId,
          earmarkId: earmarkUlid,
          expectedEarmarkRevision,
          reason: trimmedReason,
        });
        const reqCheck = validateReleaseHoldRequest(draftReq);
        if (!reqCheck.ok) {
          setValidationIssues(reqCheck.issues);
          return;
        }

        const slotKey = `ReleaseHold:${accountId}:${earmarkIdStr}`;
        const intent = writeIntents.acquire({
          operation: slotKey,
          sessionGeneration: capturedGen,
          canonicalPayload: {
            accountId,
            earmarkId: earmarkIdStr,
            expectedEarmarkRevision,
            reason: trimmedReason,
          },
        });

        const requestInit: MessageInitShape<typeof ReleaseHoldRequestSchema> = {
          idempotencyKey: intent.idempotencyKey,
          accountId,
          earmarkId: earmarkUlid,
          expectedEarmarkRevision,
          reason: trimmedReason,
        };

        setSubmitting(true);
        try {
          const res = await client.releaseHold(requestInit);
          if (store.getState().sessionGeneration !== capturedGen) {
            return;
          }
          writeIntents.complete(slotKey, intent.idempotencyKey);
          const rcptId = nextReceiptId(capturedGen);
          recordReceiptAndReplay(
            capturedGen,
            {
              id: rcptId,
              operation: 'ReleaseHold',
              accountId,
              committedSeq: res.committedSeq,
              replayed: res.replayed,
              status: 'succeeded',
              earmarkId: earmarkIdStr,
              earmark: res.earmark,
              recordedAtMs: nowMs(),
            },
            {
              operation: 'ReleaseHold',
              idempotencyKey: intent.idempotencyKey,
              request: requestInit,
            },
          );
          setLastFeedback(`ReleaseHold 已提交 (committed_seq=${res.committedSeq.toString()})`);
        } catch (err) {
          if (store.getState().sessionGeneration !== capturedGen) {
            return;
          }
          const clientErr = BlotterClientError.fromUnknown(err);
          setSubmitError(clientErr);
          const rcptId = nextReceiptId(capturedGen);
          recordReceiptAndReplay(capturedGen, {
            id: rcptId,
            operation: 'ReleaseHold',
            accountId,
            committedSeq: 0n,
            replayed: false,
            status: 'failed',
            earmarkId: earmarkIdStr,
            error: clientErr,
            recordedAtMs: nowMs(),
          });
        } finally {
          setSubmitting(false);
        }
        return;
      }

      // CancelCommitted
      const draftReq = create(CancelCommittedRequestSchema, {
        idempotencyKey: '',
        accountId,
        earmarkId: earmarkUlid,
        expectedEarmarkRevision,
        reason: trimmedReason,
      });
      const reqCheck = validateCancelCommittedRequest(draftReq);
      if (!reqCheck.ok) {
        setValidationIssues(reqCheck.issues);
        return;
      }

      const slotKey = `CancelCommitted:${accountId}:${earmarkIdStr}`;
      const intent = writeIntents.acquire({
        operation: slotKey,
        sessionGeneration: capturedGen,
        canonicalPayload: {
          accountId,
          earmarkId: earmarkIdStr,
          expectedEarmarkRevision,
          reason: trimmedReason,
        },
      });

      const requestInit: MessageInitShape<typeof CancelCommittedRequestSchema> = {
        idempotencyKey: intent.idempotencyKey,
        accountId,
        earmarkId: earmarkUlid,
        expectedEarmarkRevision,
        reason: trimmedReason,
      };

      setSubmitting(true);
      try {
        const res = await client.cancelCommitted(requestInit);
        if (store.getState().sessionGeneration !== capturedGen) {
          return;
        }
        writeIntents.complete(slotKey, intent.idempotencyKey);
        const rcptId = nextReceiptId(capturedGen);
        recordReceiptAndReplay(
          capturedGen,
          {
            id: rcptId,
            operation: 'CancelCommitted',
            accountId,
            committedSeq: res.committedSeq,
            replayed: res.replayed,
            status: 'succeeded',
            earmarkId: earmarkIdStr,
            earmark: res.earmark,
            recordedAtMs: nowMs(),
          },
          {
            operation: 'CancelCommitted',
            idempotencyKey: intent.idempotencyKey,
            request: requestInit,
          },
        );
        setLastFeedback(`CancelCommitted 已提交 (committed_seq=${res.committedSeq.toString()})`);
      } catch (err) {
        if (store.getState().sessionGeneration !== capturedGen) {
          return;
        }
        const clientErr = BlotterClientError.fromUnknown(err);
        setSubmitError(clientErr);
        const rcptId = nextReceiptId(capturedGen);
        recordReceiptAndReplay(capturedGen, {
          id: rcptId,
          operation: 'CancelCommitted',
          accountId,
          committedSeq: 0n,
          replayed: false,
          status: 'failed',
          earmarkId: earmarkIdStr,
          error: clientErr,
          recordedAtMs: nowMs(),
        });
      } finally {
        setSubmitting(false);
      }
      return;
    }

    // ConfirmHold
    const targetPosition =
      selectedPosition && selectedPosition.instrumentCode === selectedEarmark.instrumentCode
        ? selectedPosition
        : snapshot.positions.find((p) => p.instrumentCode === selectedEarmark.instrumentCode);

    if (!targetPosition) {
      setValidationIssues([
        { field: 'expected_position_version', message: '未找到占仓对应持仓记录，无法绑定持仓版本' },
      ]);
      return;
    }

    const quoteBuilt = buildQuoteTermsFromInputs(quoteDraft, {
      required: true,
      units,
    });
    if (!quoteBuilt.ok) {
      setValidationIssues(quoteBuilt.issues);
      return;
    }

    const draftReq = create(ConfirmHoldRequestSchema, {
      idempotencyKey: '',
      accountId,
      earmarkId: earmarkUlid,
      expectedEarmarkRevision,
      expectedPositionVersion: targetPosition.positionVersion,
      ...(quoteBuilt.quoteTerms !== undefined ? { quoteTerms: quoteBuilt.quoteTerms } : {}),
    });

    const reqCheck = validateConfirmHoldRequest(draftReq, units);
    if (!reqCheck.ok) {
      setValidationIssues(reqCheck.issues);
      return;
    }

    const slotKey = `ConfirmHold:${accountId}:${earmarkIdStr}`;
    const intent = writeIntents.acquire({
      operation: slotKey,
      sessionGeneration: capturedGen,
      canonicalPayload: {
        accountId,
        earmarkId: earmarkIdStr,
        expectedEarmarkRevision,
        expectedPositionVersion: targetPosition.positionVersion,
        quoteTerms: quoteBuilt.quoteTerms,
      },
    });

    const requestInit: MessageInitShape<typeof ConfirmHoldRequestSchema> = {
      idempotencyKey: intent.idempotencyKey,
      accountId,
      earmarkId: earmarkUlid,
      expectedEarmarkRevision,
      expectedPositionVersion: targetPosition.positionVersion,
      ...(quoteBuilt.quoteTerms !== undefined ? { quoteTerms: quoteBuilt.quoteTerms } : {}),
    };

    const handleExpiredCommit = (expiredErr: BlotterClientError): void => {
      setSubmitError(expiredErr);
      const rcptId = nextReceiptId(capturedGen);
      recordReceiptAndReplay(
        capturedGen,
        {
          id: rcptId,
          operation: 'ConfirmHold',
          accountId,
          committedSeq: expiredErr.committedSeq ?? 0n,
          replayed: expiredErr.replayed,
          status: 'confirm_hold_expired',
          earmarkId: earmarkIdStr,
          earmark: expiredErr.earmark,
          error: expiredErr,
          recordedAtMs: nowMs(),
        },
        {
          operation: 'ConfirmHold',
          idempotencyKey: intent.idempotencyKey,
          request: requestInit,
        },
      );
    };

    setSubmitting(true);
    try {
      const res = await client.confirmHold(requestInit);
      if (store.getState().sessionGeneration !== capturedGen) {
        return;
      }
      // Handle mocks that return ConfirmHoldResult with reconfirmationError directly
      // without going through `decodeConfirmHoldResult`.
      if (res.reconfirmationError !== undefined) {
        handleExpiredCommit(BlotterClientError.fromConfirmHoldExpired(res));
        return;
      }

      writeIntents.complete(slotKey, intent.idempotencyKey);
      const rcptId = nextReceiptId(capturedGen);
      recordReceiptAndReplay(
        capturedGen,
        {
          id: rcptId,
          operation: 'ConfirmHold',
          accountId,
          committedSeq: res.committedSeq,
          replayed: res.replayed,
          status: 'succeeded',
          earmarkId: earmarkIdStr,
          earmark: res.earmark,
          recordedAtMs: nowMs(),
        },
        {
          operation: 'ConfirmHold',
          idempotencyKey: intent.idempotencyKey,
          request: requestInit,
        },
      );
      setLastFeedback(
        `ConfirmHold 成功转为 COMMITTED (committed_seq=${res.committedSeq.toString()}, replayed=${String(res.replayed)})`,
      );
    } catch (err) {
      if (store.getState().sessionGeneration !== capturedGen) {
        return;
      }
      const clientErr = BlotterClientError.fromUnknown(err);
      if (clientErr.kind === 'confirm_hold_expired') {
        handleExpiredCommit(clientErr);
        return;
      }
      setSubmitError(clientErr);
      const rcptId = nextReceiptId(capturedGen);
      recordReceiptAndReplay(
        capturedGen,
        {
          id: rcptId,
          operation: 'ConfirmHold',
          accountId,
          committedSeq: 0n,
          replayed: false,
          status: 'failed',
          earmarkId: earmarkIdStr,
          error: clientErr,
          recordedAtMs: nowMs(),
        },
        clientErr.retryable || clientErr.kind === 'transport'
          ? {
              operation: 'ConfirmHold',
              idempotencyKey: intent.idempotencyKey,
              request: requestInit,
            }
          : undefined,
      );
    } finally {
      setSubmitting(false);
    }
  };

  const showQuoteSection =
    activeMode === 'create_hold' ||
    activeMode === 'create_committed' ||
    activeMode === 'confirm_hold';

  const quoteModeOptions: readonly { readonly value: QuoteInputMode; readonly label: string }[] =
    activeMode === 'create_hold'
      ? [
          { value: 'omit', label: '省略报价 (HOLD 允许)' },
          { value: 'settlement_only', label: '仅正结算金额 (settlement_amount)' },
          { value: 'clean_price', label: '百元净价 (+ 可选结算金额)' },
          { value: 'yield', label: '到期收益率 (+ 可选结算金额)' },
        ]
      : [
          { value: 'settlement_only', label: '仅正结算金额 (settlement_amount)' },
          { value: 'clean_price', label: '百元净价 (+ 可选结算金额)' },
          { value: 'yield', label: '到期收益率 (+ 可选结算金额)' },
        ];

  const holdExpiryInfo = selectedEarmark
    ? formatHoldRemainingTtl(selectedEarmark.expiresAt, nowMs())
    : null;

  return (
    <section
      aria-label="占仓与确认操作表单"
      data-testid="hold-action-panel"
      style={{
        border: '1px solid #cbd5e1',
        borderRadius: '6px',
        padding: '10px',
        backgroundColor: '#ffffff',
        display: 'flex',
        flexDirection: 'column',
        gap: '8px',
      }}
    >
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: '6px', alignItems: 'center' }}>
        <button
          type="button"
          data-testid="mode-btn-create_hold"
          aria-pressed={activeMode === 'create_hold'}
          onClick={() => onModeChange('create_hold')}
        >
          新建 HOLD (CreateHold)
        </button>
        <button
          type="button"
          data-testid="mode-btn-create_committed"
          aria-pressed={activeMode === 'create_committed'}
          onClick={() => onModeChange('create_committed')}
        >
          新建 COMMITTED (CreateCommitted)
        </button>
        <button
          type="button"
          data-testid="mode-btn-extend_hold"
          aria-pressed={activeMode === 'extend_hold'}
          disabled={!canExtendOrRelease}
          onClick={() => onModeChange('extend_hold')}
        >
          延长 HOLD (ExtendHold)
        </button>
        <button
          type="button"
          data-testid="mode-btn-confirm_hold"
          aria-pressed={activeMode === 'confirm_hold'}
          disabled={!canConfirm}
          onClick={() => onModeChange('confirm_hold')}
        >
          确认 HOLD (ConfirmHold)
        </button>
        <button
          type="button"
          data-testid="mode-btn-release_hold"
          aria-pressed={activeMode === 'release_hold'}
          disabled={!canExtendOrRelease}
          onClick={() => onModeChange('release_hold')}
        >
          释放 HOLD (ReleaseHold)
        </button>
        <button
          type="button"
          data-testid="mode-btn-cancel_committed"
          aria-pressed={activeMode === 'cancel_committed'}
          disabled={!canCancelCommitted}
          onClick={() => onModeChange('cancel_committed')}
        >
          撤销 COMMITTED (CancelCommitted)
        </button>
      </div>

      {writesDisabledByState ? (
        <div role="status" data-testid="writes-paused-banner" style={{ color: '#b45309', fontSize: '12px' }}>
          {!authenticated
            ? '当前会话未认证，写操作已禁用。'
            : `恢复中暂停版本依赖写入：${recoveryReason ?? 'snapshot fallback recovery'}`}
        </div>
      ) : null}

      {roleBlocksMode ? (
        <div role="status" data-testid="role-hint-banner" style={{ color: '#b45309', fontSize: '12px' }}>
          角色提示限制：当前模式仅限 TRADER 操作（最终权限以服务端 RBAC 为准）。
        </div>
      ) : null}

      <div data-testid="selected-target-context" style={{ fontSize: '12px', color: '#334155' }}>
        {selectedPosition ? (
          <span>
            选中券种: <strong>{selectedPosition.instrumentCode}</strong> · 持仓 CAS 版本:{' '}
            <code>v{selectedPosition.positionVersion.toString()}</code> · 账户版本:{' '}
            <code>v{snapshot.accountVersion.toString()}</code>
          </span>
        ) : (
          <span>尚未选择持仓券种。</span>
        )}
        {selectedEarmark ? (
          <div data-testid="selected-earmark-context" style={{ marginTop: '4px' }}>
            选中占仓: <code>{selectedEarmark.earmarkId?.value ?? '—'}</code> · 状态:{' '}
            <strong>{formatEarmarkState(selectedEarmark.state)}</strong> ·{' '}
            {formatTradeSide(selectedEarmark.side)} ·{' '}
            {formatSettlementBucket(selectedEarmark.settlementBucket)} · CAS rev:{' '}
            <code>{selectedEarmark.earmarkRevision.toString()}</code>
            {selectedEarmark.expiresAt ? (
              <span>
                {' '}
                · 到期: {formatMarketTimeDisplay(selectedEarmark.expiresAt)} (
                {holdExpiryInfo?.label ?? ''})
              </span>
            ) : null}
          </div>
        ) : null}
      </div>

      <form
        noValidate
        data-testid={`hold-form-${activeMode}`}
        onSubmit={(e) => {
          void handleSubmit(e);
        }}
        style={{ display: 'flex', flexDirection: 'column', gap: '8px', fontSize: '13px' }}
      >
        {(activeMode === 'create_hold' || activeMode === 'create_committed') ? (
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: '10px', alignItems: 'center' }}>
            <label>
              方向
              <select
                aria-label="交易方向"
                data-testid="input-side"
                value={side}
                onChange={(e) => setSide(Number(e.target.value) as TradeSide)}
              >
                <option value={TradeSide.BUY}>买入 (BUY)</option>
                <option value={TradeSide.SELL}>卖出 (SELL)</option>
              </select>
            </label>

            <label>
              结算桶
              <select
                aria-label="结算桶"
                data-testid="input-settlement-bucket"
                value={settlementBucket}
                onChange={(e) => setSettlementBucket(Number(e.target.value) as SettlementBucket)}
              >
                <option value={SettlementBucket.T_PLUS_0}>T+0</option>
                <option value={SettlementBucket.T_PLUS_1}>T+1</option>
              </select>
            </label>

            <label>
              面额 (notional)
              <input
                type="text"
                aria-label="面额"
                data-testid="input-notional"
                value={notionalText}
                onChange={(e) => setNotionalText(e.target.value)}
              />
            </label>

            <label>
              备注 (note)
              <input
                type="text"
                aria-label="备注"
                data-testid="input-note"
                value={noteText}
                onChange={(e) => setNoteText(e.target.value)}
              />
            </label>
          </div>
        ) : null}

        {(activeMode === 'create_hold' || activeMode === 'extend_hold') ? (
          <div>
            <label>
              TTL 秒数 ({MIN_HOLD_TTL_SECONDS}..={MAX_HOLD_TTL_SECONDS})
              <input
                type="text"
                inputMode="numeric"
                aria-label="TTL 秒数"
                data-testid="input-ttl-seconds"
                value={ttlText}
                onChange={(e) => setTtlText(e.target.value)}
              />
            </label>
          </div>
        ) : null}

        {(activeMode === 'release_hold' || activeMode === 'cancel_committed') ? (
          <div>
            <label>
              {activeMode === 'release_hold' ? '释放原因 (必填)' : '撤销原因 (必填)'}
              <input
                type="text"
                aria-label="操作原因"
                data-testid="input-reason"
                value={reasonText}
                onChange={(e) => setReasonText(e.target.value)}
              />
            </label>
          </div>
        ) : null}

        {showQuoteSection ? (
          <fieldset
            data-testid="quote-terms-fieldset"
            style={{ border: '1px solid #e2e8f0', borderRadius: '4px', padding: '8px' }}
          >
            <legend style={{ fontSize: '12px' }}>报价条件 (QuoteTerms)</legend>
            <div style={{ display: 'flex', flexWrap: 'wrap', gap: '10px', alignItems: 'center' }}>
              <label>
                报价模式
                <select
                  aria-label="报价模式"
                  data-testid="input-quote-mode"
                  value={quoteDraft.mode}
                  onChange={(e) =>
                    setQuoteDraft((prev) => ({
                      ...prev,
                      mode: e.target.value as QuoteInputMode,
                    }))
                  }
                >
                  {quoteModeOptions.map((opt) => (
                    <option key={opt.value} value={opt.value}>
                      {opt.label}
                    </option>
                  ))}
                </select>
              </label>

              {quoteDraft.mode === 'clean_price' ? (
                <label>
                  百元净价 (clean_price_per_100)
                  <input
                    type="text"
                    aria-label="百元净价"
                    data-testid="input-clean-price"
                    value={quoteDraft.cleanPriceText}
                    onChange={(e) =>
                      setQuoteDraft((prev) => ({ ...prev, cleanPriceText: e.target.value }))
                    }
                  />
                </label>
              ) : null}

              {quoteDraft.mode === 'yield' ? (
                <label>
                  到期收益率 (yield_to_maturity)
                  <input
                    type="text"
                    aria-label="到期收益率"
                    data-testid="input-yield"
                    value={quoteDraft.yieldText}
                    onChange={(e) =>
                      setQuoteDraft((prev) => ({ ...prev, yieldText: e.target.value }))
                    }
                  />
                </label>
              ) : null}

              {quoteDraft.mode !== 'omit' ? (
                <label>
                  结算金额 (settlement_amount{quoteDraft.mode === 'settlement_only' ? '，必填 > 0' : '，可选 > 0'})
                  <input
                    type="text"
                    aria-label="结算金额"
                    data-testid="input-settlement-amount"
                    value={quoteDraft.settlementAmountText}
                    onChange={(e) =>
                      setQuoteDraft((prev) => ({ ...prev, settlementAmountText: e.target.value }))
                    }
                  />
                </label>
              ) : null}
            </div>
          </fieldset>
        ) : null}

        {validationIssues.length > 0 ? (
          <div
            role="alert"
            data-testid="hold-form-validation-errors"
            style={{ color: '#b91c1c', fontSize: '12px' }}
          >
            {validationIssues.map((issue, idx) => (
              <div key={`${issue.field}-${idx}`}>
                {issue.field}: {issue.message}
              </div>
            ))}
          </div>
        ) : null}

        {submitError ? (
          <div
            role="alert"
            data-testid="hold-form-submit-error"
            style={{
              color: submitError.kind === 'confirm_hold_expired' ? '#92400e' : '#b91c1c',
              fontSize: '12px',
            }}
          >
            <div>
              {submitError.kind === 'confirm_hold_expired'
                ? `HOLD 已过期并提交 EXPIRED (committed_seq=${(submitError.committedSeq ?? 0n).toString()}, replayed=${String(submitError.replayed)}): ${formatErrorCode(submitError.errorCode)} · ${submitError.message}`
                : `${formatErrorCode(submitError.errorCode)}: ${submitError.message}`}
            </div>
            {submitError.isConcurrencyConflict() ? (
              <div style={{ marginTop: '4px' }}>
                <span>检测到版本并发冲突 (CONCURRENCY_CONFLICT)，请重新同步最新快照后重试。</span>
                {onResyncRequested ? (
                  <button
                    type="button"
                    data-testid="conflict-resync-btn"
                    style={{ marginLeft: '8px' }}
                    onClick={() => {
                      void onResyncRequested(snapshot.accountId);
                    }}
                  >
                    重新同步快照 (Re-sync)
                  </button>
                ) : null}
              </div>
            ) : null}
          </div>
        ) : null}

        {lastFeedback ? (
          <div role="status" data-testid="hold-form-feedback" style={{ color: '#15803d', fontSize: '12px' }}>
            {lastFeedback}
          </div>
        ) : null}

        <div>
          <button
            type="submit"
            data-testid="hold-form-submit-btn"
            disabled={submitting || writesDisabledByState || roleBlocksMode}
          >
            {submitting ? '提交中...' : `提交 ${activeMode}`}
          </button>
        </div>
      </form>
    </section>
  );
}
