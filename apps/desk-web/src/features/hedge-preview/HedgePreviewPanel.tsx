import { useEffect, useRef, useState, type FormEvent } from 'react';
import { useStore } from 'zustand';
import { TRUSTED_RULEPACK_ANCHOR } from 'virtual:ficant-trusted-rulepack';
import { PreviewHedgeScope } from '../../generated/ficant/blotter/v1/model_pb';
import type { MarketTime } from '../../generated/ficant/core/v1/common_pb';
import {
  createDecimalValue,
  formatDecimalValue,
  validateCanonicalDecimalValue,
} from '../../codec/decimal';
import { formatUnitRef, isNonEmptyTrimmed } from '../../codec/primitives';
import {
  RulePackVerificationError,
  type LoadTrustedRulePackOptions,
} from '../../codec/rulepack';
import {
  ClientValidationError,
  PreviewCoveredHedgeValidationError,
} from '../../codec/validators';
import type { DeskBlotterRpcClient } from '../../transport/client';
import { BlotterClientError } from '../../transport/error';
import type { BlotterStoreApi } from '../../store/blotterStore';
import {
  selectAllowedHedgeProductCodes,
  selectIsWholeAccountHedgeBlocked,
  selectSelectedSnapshot,
} from '../../store/selectors';
import { executePreviewCoveredHedge } from '../../store/supervisor';
import {
  HEDGE_MISSING_TEXT,
  buildValuationMarketTime,
  formatHedgeScopeLabel,
  resolveEffectiveProductCode,
  resolveEffectiveScope,
  verifyRenderableHedgeResult,
} from './hedgePreviewHelpers';

export interface HedgePreviewPanelProps {
  readonly store: BlotterStoreApi;
  readonly client: Pick<DeskBlotterRpcClient, 'getAccountSnapshot' | 'previewCoveredHedge'>;
  readonly rulePackOptions?: LoadTrustedRulePackOptions | undefined;
  readonly valuationAtOverride?: MarketTime | undefined;
  readonly nowProvider?: (() => Date) | undefined;
}

interface PreviewExecutionMeta {
  readonly sessionGeneration: number;
  readonly accountId: string;
  readonly retriedConflict: boolean;
  readonly rulePackId: string;
  readonly rulePackVersion: number;
}

function toFailClosedClientError(err: unknown): BlotterClientError {
  if (err instanceof BlotterClientError) {
    return err;
  }
  if (
    err instanceof PreviewCoveredHedgeValidationError ||
    err instanceof RulePackVerificationError
  ) {
    return BlotterClientError.protocol(err.message, err);
  }
  if (err instanceof ClientValidationError) {
    return new BlotterClientError({
      kind: 'validation',
      message: err.message,
      cause: err,
    });
  }
  if (err instanceof Error) {
    return new BlotterClientError({
      kind: 'validation',
      message: err.message,
      cause: err,
    });
  }
  return BlotterClientError.fromUnknown(err);
}

export function HedgePreviewPanel({
  store,
  client,
  rulePackOptions,
  valuationAtOverride,
  nowProvider,
}: HedgePreviewPanelProps) {
  const state = useStore(store);
  const snapshot = selectSelectedSnapshot(state);
  const accountId = snapshot?.accountId ?? null;
  const allowedProducts =
    accountId !== null ? selectAllowedHedgeProductCodes(state, accountId) : [];
  const wholeAccountBlocked =
    accountId !== null ? selectIsWholeAccountHedgeBlocked(state, accountId) : true;

  const [requestedScope, setRequestedScope] = useState<PreviewHedgeScope>(
    PreviewHedgeScope.WHOLE_ACCOUNT,
  );
  const [selectedProductCode, setSelectedProductCode] = useState('');
  const [contractCode, setContractCode] = useState('');
  const [ctdDv01Input, setCtdDv01Input] = useState('');
  const [conversionFactorInput, setConversionFactorInput] = useState('');
  const [loadingCtx, setLoadingCtx] = useState<{
    readonly sessionGeneration: number;
    readonly accountId: string;
    readonly token: number;
  } | null>(null);
  const [execMeta, setExecMeta] = useState<PreviewExecutionMeta | null>(null);

  const requestTokenRef = useRef(0);
  const inFlightAbortRef = useRef<AbortController | null>(null);

  const loading =
    loadingCtx !== null &&
    loadingCtx.sessionGeneration === state.sessionGeneration &&
    loadingCtx.accountId === accountId;

  const effectiveScope = resolveEffectiveScope(requestedScope, wholeAccountBlocked);
  const effectiveProductCode = resolveEffectiveProductCode(selectedProductCode, allowedProducts);

  const cancelInFlight = (): void => {
    requestTokenRef.current += 1;
    if (inFlightAbortRef.current !== null) {
      inFlightAbortRef.current.abort();
      inFlightAbortRef.current = null;
    }
    setLoadingCtx(null);
  };

  // Invalidate any in-flight preview when session generation or selected account changes, or on unmount.
  useEffect(() => {
    return () => {
      requestTokenRef.current += 1;
      if (inFlightAbortRef.current !== null) {
        inFlightAbortRef.current.abort();
        inFlightAbortRef.current = null;
      }
    };
  }, [state.sessionGeneration, accountId]);

  const runPreview = async (): Promise<void> => {
    if (!snapshot || accountId === null) {
      return;
    }

    cancelInFlight();
    const myToken = requestTokenRef.current;
    const abortController = new AbortController();
    inFlightAbortRef.current = abortController;

    const sessionGen = store.getState().sessionGeneration;
    const expectedSnapshotSeq = snapshot.snapshotSeq;
    const trimmedContract = contractCode.trim();
    const units = snapshot.openingBase?.units;

    const isStillCurrent = (): boolean =>
      requestTokenRef.current === myToken &&
      store.getState().sessionGeneration === sessionGen &&
      store.getState().selectedAccountId === accountId;

    const failClosed = (err: unknown): void => {
      if (!isStillCurrent()) {
        return;
      }
      const clientErr = toFailClosedClientError(err);
      store.getState().setHedgePreview(sessionGen, {
        sessionGeneration: sessionGen,
        accountId,
        scope: effectiveScope,
        productCode: effectiveProductCode,
        contractCode: trimmedContract,
        expectedSnapshotSeq,
        result: null,
        error: clientErr,
      });
    };

    // Client-side pre-flight validation before invoking supervisor/RPC.
    if (allowedProducts.length === 0 || !isNonEmptyTrimmed(effectiveProductCode)) {
      failClosed(
        new ClientValidationError([
          { field: 'product_code', message: '当前快照未提供可用套保品种（allowed_hedge_product_codes 为空）' },
        ]),
      );
      return;
    }
    if (!isNonEmptyTrimmed(trimmedContract)) {
      failClosed(
        new ClientValidationError([{ field: 'contract_code', message: '请输入非空合约代码' }]),
      );
      return;
    }
    if (!units?.dv01Per100 || !units.dimensionless) {
      failClosed(
        new ClientValidationError([
          {
            field: 'opening_base.units',
            message: '当前快照缺少 AnalysisUnits（dv01_per_100 或 dimensionless）',
          },
        ]),
      );
      return;
    }

    let ctdDv01Per100;
    let conversionFactor;
    let valuationAt: MarketTime;
    try {
      ctdDv01Per100 = createDecimalValue(
        ctdDv01Input.trim(),
        units.dv01Per100,
        'ctd_dv01_per_100',
      );
      validateCanonicalDecimalValue(ctdDv01Per100, {
        field: 'ctd_dv01_per_100',
        sign: 'positive',
        requireUnit: true,
        expectedUnit: units.dv01Per100,
      });
      conversionFactor = createDecimalValue(
        conversionFactorInput.trim(),
        units.dimensionless,
        'conversion_factor',
      );
      validateCanonicalDecimalValue(conversionFactor, {
        field: 'conversion_factor',
        sign: 'positive',
        requireUnit: true,
        expectedUnit: units.dimensionless,
      });
      valuationAt =
        valuationAtOverride ??
        buildValuationMarketTime(snapshot, nowProvider ? nowProvider() : new Date());
    } catch (err) {
      failClosed(err);
      return;
    }

    setLoadingCtx({
      sessionGeneration: sessionGen,
      accountId,
      token: myToken,
    });
    try {
      const outcome = await executePreviewCoveredHedge(client, {
        sessionGeneration: sessionGen,
        getCurrentSessionGeneration: () => store.getState().sessionGeneration,
        isRequestCurrent: isStillCurrent,
        snapshot,
        scope: effectiveScope,
        productCode: effectiveProductCode,
        contractCode: trimmedContract,
        ctdDv01Per100,
        conversionFactor,
        valuationAt,
        expectedSnapshotSeq,
        rulePackOptions,
        signal: abortController.signal,
        maxConflictRetries: 1,
      });

      if (outcome.status === 'stale' || !isStillCurrent()) {
        return;
      }

      const renderCheck = verifyRenderableHedgeResult(outcome.result, {
        scope: effectiveScope,
        productCode: effectiveProductCode,
        contractCode: trimmedContract,
        expectedSnapshotSeq: outcome.usedSnapshotSeq,
        expectedCurrencyUnit: units.currencyAmount,
      });

      if (!renderCheck.valid) {
        failClosed(
          new PreviewCoveredHedgeValidationError(
            'render_guard',
            renderCheck.reason ?? '响应未通过展示校验',
          ),
        );
        return;
      }

      const anchorManifest =
        rulePackOptions?.anchor !== undefined
          ? rulePackOptions.anchor?.manifest
          : TRUSTED_RULEPACK_ANCHOR.manifest;

      setExecMeta({
        sessionGeneration: sessionGen,
        accountId,
        retriedConflict: outcome.retriedConflict,
        rulePackId: anchorManifest?.rule_pack_id ?? HEDGE_MISSING_TEXT,
        rulePackVersion: anchorManifest?.version ?? 0,
      });

      store.getState().setHedgePreview(sessionGen, {
        sessionGeneration: sessionGen,
        accountId,
        scope: effectiveScope,
        productCode: effectiveProductCode,
        contractCode: trimmedContract,
        expectedSnapshotSeq: outcome.usedSnapshotSeq,
        result: outcome.result,
        error: null,
      });
    } catch (err) {
      if (!isStillCurrent()) {
        return;
      }
      const clientErr = BlotterClientError.fromUnknown(err);
      if (clientErr.isAborted()) {
        return;
      }
      if (clientErr.isUnauthenticated()) {
        store.getState().handleUnauthenticated(sessionGen, clientErr);
        return;
      }
      failClosed(err);
    } finally {
      if (requestTokenRef.current === myToken) {
        inFlightAbortRef.current = null;
        setLoadingCtx((prev) => (prev?.token === myToken ? null : prev));
      }
    }
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    void runPreview();
  };

  if (!snapshot || accountId === null) {
    return (
      <section aria-label="已覆盖套保试算" data-testid="hedge-preview-panel">
        <h3>已覆盖套保试算</h3>
        <p data-testid="hedge-preview-no-snapshot">请先加载并选择一个账户快照后再进行套保试算。</p>
      </section>
    );
  }

  const units = snapshot.openingBase?.units;
  const coveredCount = snapshot.riskSummary?.coveredInstrumentCount ?? 0;
  const uncoveredCount = snapshot.riskSummary?.uncoveredInstrumentCount ?? 0;

  // Bind store hedgePreview strictly to current sessionGeneration and selected accountId.
  const activePreview =
    state.hedgePreview !== null &&
    state.hedgePreview.sessionGeneration === state.sessionGeneration &&
    state.hedgePreview.accountId === accountId
      ? state.hedgePreview
      : null;

  const activeExecMeta =
    execMeta !== null &&
    execMeta.sessionGeneration === state.sessionGeneration &&
    execMeta.accountId === accountId
      ? execMeta
      : null;

  // Fail-closed render check even if a preview exists in store.
  let safeResult = activePreview?.result ?? null;
  let renderErrorMsg: string | null = activePreview?.error?.message ?? null;

  if (safeResult !== null && activePreview !== null) {
    const check = verifyRenderableHedgeResult(safeResult, {
      scope: activePreview.scope,
      productCode: activePreview.productCode,
      contractCode: activePreview.contractCode,
      expectedSnapshotSeq: activePreview.expectedSnapshotSeq,
      expectedCurrencyUnit: units?.currencyAmount,
    });
    if (!check.valid) {
      safeResult = null;
      renderErrorMsg = check.reason ?? '试算结果未通过契约校验，已清除建议手数。';
    }
  }

  const stalePreviewWarning =
    safeResult !== null && snapshot.snapshotSeq > safeResult.evaluatedAtSeq;

  return (
    <section
      aria-label="已覆盖套保试算"
      data-testid="hedge-preview-panel"
      data-account-id={accountId}
      data-snapshot-seq={snapshot.snapshotSeq.toString()}
    >
      <h3>已覆盖套保试算（私有只读，不推进工作区游标）</h3>

      {coveredCount === 0 && (
        <div role="status" data-testid="empty-covered-scope-notice">
          当前账户已覆盖持仓笔数为 0（空覆盖簿），试算将被拒绝且不会输出有效对冲手数。
        </div>
      )}

      <form onSubmit={handleSubmit} data-testid="hedge-preview-form">
        {/* Scope selector */}
        <fieldset>
          <legend>套保范围 (PreviewHedgeScope)</legend>
          <label>
            <input
              type="radio"
              name="hedge-scope"
              data-testid="scope-whole-account"
              value={String(PreviewHedgeScope.WHOLE_ACCOUNT)}
              checked={effectiveScope === PreviewHedgeScope.WHOLE_ACCOUNT}
              disabled={wholeAccountBlocked || loading}
              onChange={() => {
                cancelInFlight();
                setRequestedScope(PreviewHedgeScope.WHOLE_ACCOUNT);
              }}
            />
            全账户覆盖 (WHOLE_ACCOUNT)
          </label>
          <label>
            <input
              type="radio"
              name="hedge-scope"
              data-testid="scope-covered-subset"
              value={String(PreviewHedgeScope.COVERED_SUBSET_ONLY)}
              checked={effectiveScope === PreviewHedgeScope.COVERED_SUBSET_ONLY}
              disabled={loading}
              onChange={() => {
                cancelInFlight();
                setRequestedScope(PreviewHedgeScope.COVERED_SUBSET_ONLY);
              }}
            />
            仅已覆盖子集 (COVERED_SUBSET_ONLY)
          </label>

          {wholeAccountBlocked && (
            <p role="note" data-testid="whole-account-disabled-reason">
              存在 {uncoveredCount} 只未覆盖持仓（uncovered_instrument_count &gt; 0），已自动禁用
              WHOLE_ACCOUNT，仅可使用 COVERED_SUBSET_ONLY。
            </p>
          )}
        </fieldset>

        {/* Dynamic Product Selector */}
        <div>
          <label htmlFor="hedge-product-select">允许套保品种（来自快照策略动态渲染）</label>
          <select
            id="hedge-product-select"
            data-testid="hedge-product-select"
            value={effectiveProductCode}
            disabled={loading || allowedProducts.length === 0}
            onChange={(e) => {
              cancelInFlight();
              setSelectedProductCode(e.target.value);
            }}
          >
            {allowedProducts.length === 0 ? (
              <option value="">无可用套保品种</option>
            ) : (
              allowedProducts.map((code) => (
                <option key={code} value={code}>
                  {code}
                </option>
              ))
            )}
          </select>
        </div>

        {/* Contract Code */}
        <div>
          <label htmlFor="hedge-contract-input">合约代码</label>
          <input
            id="hedge-contract-input"
            data-testid="hedge-contract-input"
            type="text"
            value={contractCode}
            disabled={loading}
            onChange={(e) => {
              cancelInFlight();
              setContractCode(e.target.value);
            }}
            placeholder="输入目标期货合约代码"
          />
        </div>

        {/* CTD DV01 per 100 */}
        <div>
          <label htmlFor="hedge-ctd-dv01-input">
            CTD 每百元面值 DV01（单位：{formatUnitRef(units?.dv01Per100)}）
          </label>
          <input
            id="hedge-ctd-dv01-input"
            data-testid="hedge-ctd-dv01-input"
            type="text"
            value={ctdDv01Input}
            disabled={loading}
            onChange={(e) => {
              cancelInFlight();
              setCtdDv01Input(e.target.value);
            }}
            placeholder="输入严格正十进制数值"
          />
        </div>

        {/* Conversion Factor */}
        <div>
          <label htmlFor="hedge-cf-input">
            转换因子（单位：{formatUnitRef(units?.dimensionless)}）
          </label>
          <input
            id="hedge-cf-input"
            data-testid="hedge-cf-input"
            type="text"
            value={conversionFactorInput}
            disabled={loading}
            onChange={(e) => {
              cancelInFlight();
              setConversionFactorInput(e.target.value);
            }}
            placeholder="输入严格正十进制数值"
          />
        </div>

        <div>
          <button
            type="submit"
            data-testid="hedge-preview-submit-btn"
            disabled={loading || allowedProducts.length === 0}
          >
            {loading ? '试算中...' : '执行套保试算'}
          </button>
        </div>
      </form>

      {/* Bounded Recompute Notice (cross-account head_seq conflict retry) */}
      {activeExecMeta?.retriedConflict && safeResult !== null && (
        <div role="status" data-testid="hedge-bounded-recompute-notice">
          检测到跨账户快照序号推进冲突，已完成有界单次私有快照刷新并重算（evaluated_at_seq=
          {safeResult.evaluatedAtSeq.toString()}）。
        </div>
      )}

      {/* Stale Preview Warning when account snapshot_seq advanced past evaluated_at_seq */}
      {stalePreviewWarning && safeResult !== null && (
        <div role="status" data-testid="stale-preview-warning">
          <span>
            当前账户快照已推进至 seq={snapshot.snapshotSeq.toString()}，下方试算结果基于旧快照
            seq={safeResult.evaluatedAtSeq.toString()}。
          </span>
          <button
            type="button"
            data-testid="repreview-stale-btn"
            disabled={loading}
            onClick={() => {
              void runPreview();
            }}
          >
            按最新快照重新试算
          </button>
        </div>
      )}

      {/* Fail-Closed Error Banner (NEVER renders lots alongside error) */}
      {renderErrorMsg !== null && (
        <div role="alert" data-testid="hedge-preview-error">
          <strong>套保试算阻断 / 校验失败（已清除建议手数）：</strong>
          <span>{renderErrorMsg}</span>
        </div>
      )}

      {/* Validated Preview Output (only rendered when fail-closed guard passes) */}
      {safeResult !== null && renderErrorMsg === null && safeResult.measures !== undefined && (
        <div
          data-testid="hedge-preview-result"
          data-evaluated-at-seq={safeResult.evaluatedAtSeq.toString()}
          data-zero-net-exposure={safeResult.zeroNetExposure ? 'true' : 'false'}
        >
          {safeResult.zeroNetExposure ? (
            <div role="status" data-testid="hedge-zero-net-exposure">
              已覆盖持仓（{safeResult.coveredInstrumentCount} 只）净税前 DV01 为
              0（零净敞口），无需额外对冲，建议手数：0 手。
            </div>
          ) : (
            <div data-testid="hedge-active-exposure">
              已覆盖持仓（{safeResult.coveredInstrumentCount} 只）有效套保试算结果：
            </div>
          )}

          <dl>
            <div>
              <dt>建议对冲手数 (recommended_contracts)</dt>
              <dd data-testid="hedge-recommended-contracts">
                {safeResult.measures.recommendedContracts.toString()}
              </dd>
            </div>
            <div>
              <dt>原始手数 (raw_contracts)</dt>
              <dd data-testid="hedge-raw-contracts">
                {formatDecimalValue(safeResult.measures.rawContracts, {
                  missingText: HEDGE_MISSING_TEXT,
                  grouping: true,
                })}
              </dd>
            </div>
            <div>
              <dt>合约面值 (contract_notional)</dt>
              <dd data-testid="hedge-contract-notional">
                {formatDecimalValue(safeResult.measures.contractNotional, {
                  missingText: HEDGE_MISSING_TEXT,
                  grouping: true,
                })}{' '}
                <span data-testid="hedge-contract-notional-unit">
                  ({formatUnitRef(safeResult.measures.contractNotional?.unit)})
                </span>
              </dd>
            </div>
            <div>
              <dt>单手期货 DV01 (futures_contract_dv01)</dt>
              <dd data-testid="hedge-futures-contract-dv01">
                {formatDecimalValue(safeResult.measures.futuresContractDv01, {
                  missingText: HEDGE_MISSING_TEXT,
                  grouping: true,
                })}{' '}
                ({formatUnitRef(safeResult.measures.futuresContractDv01?.unit)})
              </dd>
            </div>
            <div>
              <dt>残余 DV01 (residual_dv01)</dt>
              <dd data-testid="hedge-residual-dv01">
                {formatDecimalValue(safeResult.measures.residualDv01, {
                  missingText: HEDGE_MISSING_TEXT,
                  grouping: true,
                })}{' '}
                ({formatUnitRef(safeResult.measures.residualDv01?.unit)})
              </dd>
            </div>
            <div>
              <dt>套保有效度 (hedge_effectiveness)</dt>
              <dd data-testid="hedge-effectiveness">
                {formatDecimalValue(safeResult.measures.hedgeEffectiveness, {
                  missingText: HEDGE_MISSING_TEXT,
                  grouping: false,
                })}
              </dd>
            </div>
            <div>
              <dt>已覆盖税前净 DV01 (covered_net_pre_tax_dv01)</dt>
              <dd data-testid="hedge-covered-dv01">
                {formatDecimalValue(safeResult.coveredNetPreTaxDv01, {
                  missingText: HEDGE_MISSING_TEXT,
                  grouping: true,
                })}{' '}
                ({formatUnitRef(safeResult.coveredNetPreTaxDv01?.unit)})
              </dd>
            </div>
            {(safeResult.uncoveredInstrumentCount > 0 ||
              safeResult.excludedUncoveredGrossNotional !== undefined) && (
              <div>
                <dt>已排除未覆盖总面额 (excluded_uncovered_gross_notional)</dt>
                <dd data-testid="hedge-excluded-uncovered-gross">
                  {formatDecimalValue(safeResult.excludedUncoveredGrossNotional, {
                    missingText: HEDGE_MISSING_TEXT,
                    grouping: true,
                  })}{' '}
                  ({formatUnitRef(safeResult.excludedUncoveredGrossNotional?.unit)})
                </dd>
              </div>
            )}
            <div>
              <dt>试算绑定元信息</dt>
              <dd data-testid="hedge-result-metadata">
                scope={formatHedgeScopeLabel(safeResult.scope)} | product={safeResult.productCode}{' '}
                | contract={safeResult.contractCode} | evaluated_at_seq=
                {safeResult.evaluatedAtSeq.toString()} | tax_basis=PRE_TAX | rulepack=
                {activeExecMeta?.rulePackId ?? TRUSTED_RULEPACK_ANCHOR.manifest.rule_pack_id}@v
                {String(
                  activeExecMeta?.rulePackVersion ?? TRUSTED_RULEPACK_ANCHOR.manifest.version,
                )}
              </dd>
            </div>
          </dl>
        </div>
      )}
    </section>
  );
}
