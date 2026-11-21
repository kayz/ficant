import { useState, type FormEvent } from 'react';
import { useStore } from 'zustand';
import {
  PrincipalRole,
  type FeedStatusView,
  type OpeningPositionBase,
} from '../../generated/ficant/blotter/v1/model_pb';
import { ErrorCode } from '../../generated/ficant/core/v1/error_pb';
import { validateSetFeedControlRequest } from '../../codec/validators';
import {
  assertSecureCredentialContext,
  isSecureCredentialContext,
  validateRawCredentialToken,
  type CredentialHeaderMode,
  type LocationLike,
} from '../../transport/auth';
import type { DeskBlotterRpcClient } from '../../transport/client';
import { generateUuidV4 } from '../../transport/crypto';
import { BlotterClientError } from '../../transport/error';
import type { BlotterStoreApi } from '../../store/blotterStore';
import {
  selectFeedStatuses,
  selectIsWritesPaused,
  selectLoadedAccountIds,
  selectSelectedSnapshot,
} from '../../store/selectors';
import type { BlotterSupervisor } from '../../store/supervisor';
import {
  canHintFetchOpeningBaseSpi,
  canHintForceClearGapPause,
  canHintImportOpeningBase,
  canHintPauseFeed,
  describeBaselineProvenance,
  describeConnectionState,
  formatFeedHealthState,
  formatMissingFeedRanges,
  formatPrincipalRoleLabel,
  parseExplicitAccountIds,
} from './sessionHelpers';

export interface SessionPanelProps {
  readonly store: BlotterStoreApi;
  readonly supervisor: BlotterSupervisor;
  readonly client?: DeskBlotterRpcClient | undefined;
  readonly location?: LocationLike | undefined;
  readonly accountHintList?: readonly string[] | undefined;
}

interface OperationFeedback {
  readonly ok: boolean;
  readonly operation: string;
  readonly message: string;
  readonly isRbacDenied?: boolean;
  readonly committedSeq?: bigint;
  readonly replayed?: boolean;
}

function formatErrorCodeBadge(code: ErrorCode): string {
  switch (code) {
    case ErrorCode.UNAUTHENTICATED:
      return 'UNAUTHENTICATED';
    case ErrorCode.FORBIDDEN:
      return 'FORBIDDEN (RBAC拒绝)';
    case ErrorCode.NOT_FOUND:
      return 'NOT_FOUND';
    case ErrorCode.CONCURRENCY_CONFLICT:
      return 'CONCURRENCY_CONFLICT';
    case ErrorCode.VERSION_CONFLICT:
      return 'VERSION_CONFLICT';
    case ErrorCode.VALIDATION_FAILED:
      return 'VALIDATION_FAILED';
    case ErrorCode.STORAGE_UNAVAILABLE:
      return 'STORAGE_UNAVAILABLE';
    default:
      return `CODE_${String(code)}`;
  }
}

export function SessionPanel({
  store,
  supervisor,
  client,
  location,
  accountHintList,
}: SessionPanelProps) {
  const state = useStore(store);
  const selectedSnapshot = selectSelectedSnapshot(state);
  const loadedAccountIds = selectLoadedAccountIds(state);
  const writesPaused = selectIsWritesPaused(state);
  const feedStatuses: readonly FeedStatusView[] =
    state.selectedAccountId !== null ? selectFeedStatuses(state, state.selectedAccountId) : [];

  const secureContext = isSecureCredentialContext(location);
  const connBadge = describeConnectionState(state.connectionState);

  // Auth form local state (token strictly in component memory, cleared immediately on login/logout).
  const [headerMode, setHeaderMode] = useState<CredentialHeaderMode>('authorization');
  const [tokenInput, setTokenInput] = useState('');
  const [roleHintInput, setRoleHintInput] = useState<PrincipalRole>(PrincipalRole.TRADER);
  const [actorLabelInput, setActorLabelInput] = useState('');
  const [authFormError, setAuthFormError] = useState<string | null>(null);
  const [showSwitchAuth, setShowSwitchAuth] = useState(false);

  // Explicit account onboarding input state.
  const [accountsInput, setAccountsInput] = useState('');
  const [accountsFormError, setAccountsFormError] = useState<string | null>(null);
  const [submittingAccountsGen, setSubmittingAccountsGen] = useState<number | null>(null);

  // Baseline SPI vs Import & Feed Control state.
  const [spiTradingDateInput, setSpiTradingDateInput] = useState('');
  const [fetchedSpiBase, setFetchedSpiBase] = useState<{
    readonly sessionGeneration: number;
    readonly base: OpeningPositionBase;
  } | null>(null);
  const [baselineProvenanceOverride, setBaselineProvenanceOverride] = useState<{
    readonly sessionGeneration: number;
    readonly accountId: string;
    readonly source: 'spi' | 'import';
  } | null>(null);
  const [gapReasonsBySource, setGapReasonsBySource] = useState<Readonly<Record<string, string>>>(
    {},
  );
  const [opFeedback, setOpFeedback] = useState<{
    readonly sessionGeneration: number;
    readonly feedback: OperationFeedback;
  } | null>(null);
  const [opBusyGen, setOpBusyGen] = useState<number | null>(null);

  // Scope transient panel state strictly to current sessionGeneration.
  const submittingAccounts = submittingAccountsGen === state.sessionGeneration;
  const opBusy = opBusyGen === state.sessionGeneration;
  const activeFetchedSpiBase =
    fetchedSpiBase !== null && fetchedSpiBase.sessionGeneration === state.sessionGeneration
      ? fetchedSpiBase.base
      : null;
  const activeOpFeedback =
    opFeedback !== null && opFeedback.sessionGeneration === state.sessionGeneration
      ? opFeedback.feedback
      : null;
  const activeProvenanceAction =
    baselineProvenanceOverride !== null &&
    baselineProvenanceOverride.sessionGeneration === state.sessionGeneration &&
    baselineProvenanceOverride.accountId === state.selectedAccountId
      ? baselineProvenanceOverride.source
      : undefined;

  const provenance = describeBaselineProvenance(selectedSnapshot, activeProvenanceAction);
  const accountErrorEntries = Object.entries(state.accountLoadErrors);

  const handleLoginSubmit = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    setAuthFormError(null);

    try {
      assertSecureCredentialContext(location);
    } catch (err) {
      setAuthFormError(err instanceof Error ? err.message : 'INSECURE_HTTP_CONTEXT');
      return;
    }

    try {
      validateRawCredentialToken(tokenInput);
    } catch (err) {
      setAuthFormError(err instanceof Error ? err.message : '凭证格式无效');
      return;
    }

    try {
      supervisor.login({
        token: tokenInput,
        headerMode,
        roleHint: roleHintInput,
        ...(actorLabelInput.trim().length > 0 ? { actorLabel: actorLabelInput.trim() } : {}),
      });
      // Immediately wipe token from input state after moving into in-memory credential store.
      setTokenInput('');
      setAccountsFormError(null);
      setShowSwitchAuth(false);
    } catch (err) {
      setAuthFormError(err instanceof Error ? err.message : '会话初始化失败');
    }
  };

  const handleLogout = (): void => {
    setTokenInput('');
    setAuthFormError(null);
    setAccountsFormError(null);
    setAccountsInput('');
    setShowSwitchAuth(false);
    supervisor.logout();
  };

  const submitAccountList = async (ids: readonly string[]): Promise<void> => {
    setAccountsFormError(null);
    if (ids.length === 0) {
      setAccountsFormError('请至少输入一个有效账户 ID（禁止空账户集合订阅）。');
      return;
    }
    const gen = store.getState().sessionGeneration;
    setSubmittingAccountsGen(gen);
    try {
      await supervisor.submitAccounts(ids);
    } finally {
      setSubmittingAccountsGen((prev) => (prev === gen ? null : prev));
    }
  };

  const handleAccountsSubmit = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    const parsed = parseExplicitAccountIds(accountsInput);
    void submitAccountList(parsed);
  };

  const handleResetAccounts = (): void => {
    setAccountsInput('');
    setAccountsFormError(null);
    void supervisor.submitAccounts([]);
  };

  const handleRemoveFailedAccount = (failedId: string): void => {
    const baseList =
      parseExplicitAccountIds(accountsInput).length > 0
        ? parseExplicitAccountIds(accountsInput)
        : state.requestedAccountIds;
    const remaining = baseList.filter((id) => id !== failedId);
    setAccountsInput(remaining.join(', '));
    setAccountsFormError(null);
    if (remaining.length === 0) {
      void supervisor.submitAccounts([]);
    } else {
      void submitAccountList(remaining);
    }
  };

  const handleFetchOpeningBaseSpi = async (): Promise<void> => {
    if (!client || !state.authenticated) {
      return;
    }
    const targetAccountId =
      state.selectedAccountId ?? state.requestedAccountIds[0] ?? parseExplicitAccountIds(accountsInput)[0];
    if (!targetAccountId) {
      setOpFeedback({
        sessionGeneration: state.sessionGeneration,
        feedback: {
          ok: false,
          operation: 'FetchOpeningPositionBase (SPI)',
          message: '请先选择或输入目标账户 ID。',
        },
      });
      return;
    }
    const tradingDate =
      spiTradingDateInput.trim() || selectedSnapshot?.openingBase?.tradingDate || '';
    if (tradingDate.length === 0) {
      setOpFeedback({
        sessionGeneration: state.sessionGeneration,
        feedback: {
          ok: false,
          operation: 'FetchOpeningPositionBase (SPI)',
          message: '请填写交易日（YYYY-MM-DD）后再执行 SPI 拉取。',
        },
      });
      return;
    }

    const gen = state.sessionGeneration;
    setOpBusyGen(gen);
    try {
      const requestId = generateUuidV4();
      const base = await client.fetchOpeningPositionBase({
        requestId,
        accountId: targetAccountId,
        tradingDate,
      });
      if (store.getState().sessionGeneration !== gen) {
        return;
      }
      setFetchedSpiBase({ sessionGeneration: gen, base });
      setBaselineProvenanceOverride({
        sessionGeneration: gen,
        accountId: targetAccountId,
        source: 'spi',
      });
      setOpFeedback({
        sessionGeneration: gen,
        feedback: {
          ok: true,
          operation: 'FetchOpeningPositionBase (SPI)',
          message: `已通过 SPI 拉取账户 ${base.accountId} 基线（source=${base.sourceId || '未知'}，交易日=${base.tradingDate}）。`,
        },
      });
    } catch (err) {
      if (store.getState().sessionGeneration !== gen) {
        return;
      }
      const clientErr = BlotterClientError.fromUnknown(err);
      if (clientErr.isUnauthenticated()) {
        store.getState().handleUnauthenticated(gen, clientErr);
        return;
      }
      setOpFeedback({
        sessionGeneration: gen,
        feedback: {
          ok: false,
          operation: 'FetchOpeningPositionBase (SPI)',
          message: `${formatErrorCodeBadge(clientErr.errorCode)}: ${clientErr.message}`,
          isRbacDenied: clientErr.isForbidden(),
        },
      });
    } finally {
      setOpBusyGen((prev) => (prev === gen ? null : prev));
    }
  };

  const handleImportOpeningBase = async (): Promise<void> => {
    if (!client || !state.authenticated || writesPaused) {
      return;
    }
    const baseToImport = activeFetchedSpiBase ?? selectedSnapshot?.openingBase;
    if (!baseToImport) {
      setOpFeedback({
        sessionGeneration: state.sessionGeneration,
        feedback: {
          ok: false,
          operation: 'ImportOpeningBase',
          message: '当前无可供导入的 OpeningPositionBase（请先通过 SPI 拉取或加载基线快照）。',
        },
      });
      return;
    }
    const gen = state.sessionGeneration;
    const expectedAccountVersion = selectedSnapshot?.accountVersion ?? 0n;
    const intent = supervisor.writeIntents.acquire({
      operation: `ImportOpeningBase:${baseToImport.accountId}`,
      canonicalPayload: {
        accountId: baseToImport.accountId,
        baseId: baseToImport.baseId?.value ?? '',
        sourceId: baseToImport.sourceId,
        tradingDate: baseToImport.tradingDate,
        expectedAccountVersion,
      },
      sessionGeneration: gen,
    });

    setOpBusyGen(gen);
    try {
      const result = await client.importOpeningBase({
        idempotencyKey: intent.idempotencyKey,
        openingBase: baseToImport,
        expectedAccountVersion,
      });
      if (store.getState().sessionGeneration !== gen) {
        return;
      }
      supervisor.writeIntents.complete(intent.operation, intent.idempotencyKey);
      store.getState().recordWriteReceipt(gen, {
        id: intent.idempotencyKey,
        operation: 'ImportOpeningBase',
        accountId: baseToImport.accountId,
        committedSeq: result.committedSeq,
        replayed: result.replayed,
        status: 'succeeded',
        recordedAtMs: Date.now(),
      });
      supervisor.triggerCatchUp();
      setBaselineProvenanceOverride({
        sessionGeneration: gen,
        accountId: baseToImport.accountId,
        source: 'import',
      });
      setOpFeedback({
        sessionGeneration: gen,
        feedback: {
          ok: true,
          operation: 'ImportOpeningBase',
          message: `导入开盘基线已提交（seq=${result.committedSeq.toString()}${result.replayed ? '，幂等重放' : ''}）。`,
          committedSeq: result.committedSeq,
          replayed: result.replayed,
        },
      });
    } catch (err) {
      if (store.getState().sessionGeneration !== gen) {
        return;
      }
      const clientErr = BlotterClientError.fromUnknown(err);
      if (clientErr.isUnauthenticated()) {
        store.getState().handleUnauthenticated(gen, clientErr);
        return;
      }
      if (clientErr.kind === 'business' || clientErr.kind === 'auth') {
        supervisor.writeIntents.complete(intent.operation, intent.idempotencyKey);
      }
      setOpFeedback({
        sessionGeneration: gen,
        feedback: {
          ok: false,
          operation: 'ImportOpeningBase',
          message: `${formatErrorCodeBadge(clientErr.errorCode)}: ${clientErr.message}`,
          isRbacDenied: clientErr.isForbidden(),
        },
      });
    } finally {
      setOpBusyGen((prev) => (prev === gen ? null : prev));
    }
  };

  const handleSetFeedControl = async (
    feed: FeedStatusView,
    options: { readonly pausedByOperator: boolean; readonly forceClearGapPause: boolean },
  ): Promise<void> => {
    if (!client || !state.authenticated || writesPaused) {
      return;
    }
    const reason = (gapReasonsBySource[feed.sourceId] ?? '').trim();
    const validation = validateSetFeedControlRequest({
      sourceId: feed.sourceId,
      expectedFeedStatusRevision: feed.feedStatusRevision,
      pausedByOperator: options.pausedByOperator,
      forceClearGapPause: options.forceClearGapPause,
      reason,
    });
    if (!validation.ok) {
      setOpFeedback({
        sessionGeneration: state.sessionGeneration,
        feedback: {
          ok: false,
          operation: options.forceClearGapPause ? 'SetFeedControl (强制清除缺口暂停)' : 'SetFeedControl',
          message: validation.issues.map((i) => `${i.field}: ${i.message}`).join('; '),
        },
      });
      return;
    }

    const gen = state.sessionGeneration;
    const intent = supervisor.writeIntents.acquire({
      operation: `SetFeedControl:${feed.sourceId}`,
      canonicalPayload: {
        sourceId: feed.sourceId,
        expectedFeedStatusRevision: feed.feedStatusRevision,
        pausedByOperator: options.pausedByOperator,
        forceClearGapPause: options.forceClearGapPause,
        reason,
      },
      sessionGeneration: gen,
    });

    setOpBusyGen(gen);
    try {
      const result = await client.setFeedControl({
        idempotencyKey: intent.idempotencyKey,
        sourceId: feed.sourceId,
        expectedFeedStatusRevision: feed.feedStatusRevision,
        pausedByOperator: options.pausedByOperator,
        forceClearGapPause: options.forceClearGapPause,
        reason,
      });
      if (store.getState().sessionGeneration !== gen) {
        return;
      }
      supervisor.writeIntents.complete(intent.operation, intent.idempotencyKey);
      store.getState().recordWriteReceipt(gen, {
        id: intent.idempotencyKey,
        operation: 'SetFeedControl',
        accountId: state.selectedAccountId ?? '',
        committedSeq: result.committedSeq,
        replayed: result.replayed,
        status: 'succeeded',
        recordedAtMs: Date.now(),
      });
      supervisor.triggerCatchUp();
      setOpFeedback({
        sessionGeneration: gen,
        feedback: {
          ok: true,
          operation: options.forceClearGapPause ? 'SetFeedControl (强制清除缺口暂停)' : 'SetFeedControl',
          message: `源 ${feed.sourceId} 控制已提交（seq=${result.committedSeq.toString()}${result.replayed ? '，幂等重放' : ''}）。`,
          committedSeq: result.committedSeq,
          replayed: result.replayed,
        },
      });
    } catch (err) {
      if (store.getState().sessionGeneration !== gen) {
        return;
      }
      const clientErr = BlotterClientError.fromUnknown(err);
      if (clientErr.isUnauthenticated()) {
        store.getState().handleUnauthenticated(gen, clientErr);
        return;
      }
      if (clientErr.kind === 'business' || clientErr.kind === 'auth') {
        supervisor.writeIntents.complete(intent.operation, intent.idempotencyKey);
      }
      setOpFeedback({
        sessionGeneration: gen,
        feedback: {
          ok: false,
          operation: options.forceClearGapPause ? 'SetFeedControl (强制清除缺口暂停)' : 'SetFeedControl',
          message: `${formatErrorCodeBadge(clientErr.errorCode)}: ${clientErr.message}`,
          isRbacDenied: clientErr.isForbidden(),
        },
      });
    } finally {
      setOpBusyGen((prev) => (prev === gen ? null : prev));
    }
  };

  const showAuthForm = !state.authenticated || showSwitchAuth;
  const canSpiByHint = canHintFetchOpeningBaseSpi(state.roleHint);
  const canImportByHint = canHintImportOpeningBase(state.roleHint);
  const canPauseByHint = canHintPauseFeed(state.roleHint);
  const canForceClearByHint = canHintForceClearGapPause(state.roleHint);

  return (
    <section
      aria-label="会话与账户控制"
      data-testid="session-panel"
      data-session-generation={String(state.sessionGeneration)}
      data-connection-state={connBadge.badgeState}
    >
      {/* Status Header Strip */}
      <div data-testid="session-status-bar">
        <span
          data-testid="connection-badge"
          data-connection-state={connBadge.badgeState}
          data-raw-connection-state={connBadge.rawState}
        >
          {connBadge.label}
        </span>

        <span data-testid="session-generation">会话世代 Gen #{state.sessionGeneration}</span>

        <span data-testid="active-account-badge">
          当前账户：{state.selectedAccountId ?? '未选择'}
        </span>

        <span data-testid="workspace-cursor-badge">
          游标：{state.cursorSeq.toString()} / Head：{state.knownHeadSeq.toString()}
        </span>

        {state.authenticated && (
          <>
            <span data-testid="credential-mode-badge">
              凭证模式：{state.headerMode === 'authorization' ? 'Bearer Token' : 'Intranet Header'}
              （仅内存）
            </span>
            <span data-testid="role-hint-badge">
              本地角色提示：{formatPrincipalRoleLabel(state.roleHint)}
              （仅本地 UI 提示，实际权限由服务端 RBAC 强制执行）
            </span>
            {state.actorLabel !== null && (
              <span data-testid="actor-label-badge">操作标识：{state.actorLabel}</span>
            )}
          </>
        )}

        {state.recoveryPausedWrites && (
          <span data-testid="recovery-writes-paused-badge" role="status">
            恢复屏障中：版本依赖写入已暂停
            {state.recoveryReason ? `（${state.recoveryReason}）` : ''}
          </span>
        )}

        {state.authenticated && (
          <div>
            <button
              type="button"
              data-testid="toggle-switch-auth-btn"
              onClick={() => {
                setShowSwitchAuth((v) => !v);
                setAuthFormError(null);
              }}
            >
              {showSwitchAuth ? '取消切换凭证' : '切换凭证'}
            </button>
            <button type="button" data-testid="logout-btn" onClick={handleLogout}>
              切换会话 / 退出登录
            </button>
          </div>
        )}
      </div>

      {/* Secure Context Fail-Closed Gate */}
      {!secureContext && (
        <div role="alert" data-testid="insecure-context-banner">
          <strong>INSECURE_HTTP_CONTEXT</strong>：检测到非回环明文 HTTP，已阻断凭证输入与传输。请改用
          HTTPS 安全上下文访问。
        </div>
      )}

      {/* Last Transport / Protocol / Auth Error Banner */}
      {state.lastError !== null && (
        <div
          role="alert"
          data-testid="session-error-banner"
          data-error-severity={connBadge.failClosed ? 'fail_closed' : 'recoverable_warning'}
        >
          <strong>
            {connBadge.failClosed ? '连接/认证阻断：' : '传输降级告警（保留已发布快照）：'}
          </strong>
          <span>{formatErrorCodeBadge(state.lastError.errorCode)}</span> —{' '}
          <span>{state.lastError.message}</span>
          {state.lastError.traceId.length > 0 && <span> [trace: {state.lastError.traceId}]</span>}
          {state.connectionState === 'error' && state.authenticated && (
            <button
              type="button"
              data-testid="retry-recovery-btn"
              onClick={() => {
                void supervisor.retryRecovery();
              }}
            >
              重试恢复
            </button>
          )}
        </div>
      )}

      {/* Authentication / Credential Entry Form */}
      {showAuthForm && (
        <form
          onSubmit={handleLoginSubmit}
          data-testid="auth-form"
          aria-label="凭证认证表单"
        >
          <fieldset disabled={!secureContext}>
            <legend>{state.authenticated ? '切换会话凭证（同步清空旧会话状态）' : '内网凭证认证（仅内存驻留）'}</legend>

            <div role="radiogroup" aria-label="凭证传输模式">
              <label>
                <input
                  type="radio"
                  name="credential-header-mode"
                  value="authorization"
                  checked={headerMode === 'authorization'}
                  onChange={() => setHeaderMode('authorization')}
                />
                Bearer Token (authorization)
              </label>
              <label>
                <input
                  type="radio"
                  name="credential-header-mode"
                  value="x-ficant-credential"
                  checked={headerMode === 'x-ficant-credential'}
                  onChange={() => setHeaderMode('x-ficant-credential')}
                />
                Intranet Header Credential (x-ficant-credential)
              </label>
            </div>

            <div>
              <label htmlFor="session-token-input">访问令牌（严禁持久化存储）</label>
              <input
                id="session-token-input"
                type="password"
                autoComplete="off"
                value={tokenInput}
                onChange={(e) => setTokenInput(e.target.value)}
                placeholder="输入内网访问令牌"
              />
            </div>

            <div>
              <label htmlFor="session-role-hint-select">
                本地角色显示提示（仅控制界面呈现，不替代服务端 RBAC）
              </label>
              <select
                id="session-role-hint-select"
                value={String(roleHintInput)}
                onChange={(e) => setRoleHintInput(Number(e.target.value) as PrincipalRole)}
              >
                <option value={String(PrincipalRole.TRADER)}>TRADER</option>
                <option value={String(PrincipalRole.OPERATOR)}>OPERATOR</option>
                <option value={String(PrincipalRole.FEED_ADAPTER)}>FEED_ADAPTER</option>
                <option value={String(PrincipalRole.NLP_PARSER)}>NLP_PARSER</option>
                <option value={String(PrincipalRole.UNSPECIFIED)}>未指定</option>
              </select>
            </div>

            <div>
              <label htmlFor="session-actor-label-input">本地操作员标签（可选）</label>
              <input
                id="session-actor-label-input"
                type="text"
                autoComplete="off"
                value={actorLabelInput}
                onChange={(e) => setActorLabelInput(e.target.value)}
                placeholder="例如 desk-trader-01"
              />
            </div>

            {authFormError !== null && (
              <div role="alert" data-testid="auth-form-error">
                {authFormError}
              </div>
            )}

            <button type="submit" data-testid="login-submit-btn" disabled={!secureContext}>
              {state.authenticated ? '确认切换凭证' : '建立内存会话'}
            </button>
          </fieldset>
        </form>
      )}

      {/* Explicit Account Onboarding & Switcher */}
      {state.authenticated && (
        <div data-testid="account-onboarding-section">
          <form onSubmit={handleAccountsSubmit} data-testid="accounts-form" aria-label="显式账户订阅">
            <label htmlFor="explicit-account-ids-input">
              显式账户 ID（逗号或空格分隔，须先获取基线快照再订阅）
            </label>
            <input
              id="explicit-account-ids-input"
              type="text"
              value={accountsInput}
              onChange={(e) => setAccountsInput(e.target.value)}
              placeholder="例如 acct-001, acct-002"
              disabled={submittingAccounts}
            />
            <button
              type="submit"
              data-testid="submit-accounts-btn"
              disabled={submittingAccounts}
            >
              {submittingAccounts ? '加载基线中...' : '加载账户基线并订阅'}
            </button>
            <button
              type="button"
              data-testid="reset-accounts-btn"
              onClick={handleResetAccounts}
              disabled={submittingAccounts}
            >
              清空账户
            </button>
          </form>

          {accountHintList !== undefined && accountHintList.length > 0 && (
            <div data-testid="account-hint-list">
              <span>现场账户提示（需人工确认后提交）：</span>
              <button
                type="button"
                data-testid="fill-account-hints-btn"
                onClick={() => setAccountsInput(accountHintList.join(', '))}
              >
                填入提示账户 ({accountHintList.join(', ')})
              </button>
            </div>
          )}

          {accountsFormError !== null && (
            <div role="alert" data-testid="accounts-form-error">
              {accountsFormError}
            </div>
          )}

          {/* Per-Account Load / RBAC Rejection Errors with Remediation */}
          {accountErrorEntries.length > 0 && (
            <div role="alert" data-testid="account-load-errors">
              <p>以下账户加载失败（可修正或移除后重新提交）：</p>
              <ul>
                {accountErrorEntries.map(([acctId, err]) => (
                  <li key={acctId} data-testid={`account-error-${acctId}`}>
                    <span>{acctId}</span>: <strong>{formatErrorCodeBadge(err.errorCode)}</strong> —{' '}
                    <span>{err.message}</span>
                    <button
                      type="button"
                      data-testid={`remove-failed-account-${acctId}`}
                      onClick={() => handleRemoveFailedAccount(acctId)}
                    >
                      移除此账户并重试
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          )}

          {/* Subscribed Account Switcher Tabs */}
          {loadedAccountIds.length > 0 && (
            <div role="tablist" aria-label="已订阅账户切换" data-testid="account-switcher">
              {loadedAccountIds.map((acctId) => {
                const isSelected = state.selectedAccountId === acctId;
                const snap = state.snapshots[acctId];
                return (
                  <button
                    key={acctId}
                    type="button"
                    role="tab"
                    aria-selected={isSelected}
                    data-testid={`account-tab-${acctId}`}
                    onClick={() => store.getState().setSelectedAccountId(acctId)}
                  >
                    {acctId}
                    {snap !== undefined ? ` (seq ${snap.snapshotSeq.toString()})` : ''}
                  </button>
                );
              })}
            </div>
          )}
        </div>
      )}

      {/* Provenance, Baseline SPI vs Import, & Feed Control */}
      {state.authenticated && (selectedSnapshot !== undefined || client !== undefined) && (
        <div data-testid="provenance-and-feed-control">
          <div data-testid="baseline-provenance-bar">
            <span data-testid="baseline-provenance-badge" data-provenance-kind={provenance.kind}>
              基线来源：{provenance.label}
            </span>
            {provenance.sourceId !== null && (
              <span data-testid="baseline-source-id">source_id={provenance.sourceId}</span>
            )}
            {provenance.tradingDate !== null && (
              <span data-testid="baseline-trading-date">交易日={provenance.tradingDate}</span>
            )}
            {selectedSnapshot !== undefined && (
              <span data-testid="account-version-badge">
                account_version={selectedSnapshot.accountVersion.toString()}
              </span>
            )}
          </div>

          {client !== undefined && (
            <div data-testid="baseline-actions">
              <label htmlFor="spi-trading-date-input">SPI 拉取交易日</label>
              <input
                id="spi-trading-date-input"
                type="text"
                value={spiTradingDateInput}
                onChange={(e) => setSpiTradingDateInput(e.target.value)}
                placeholder={selectedSnapshot?.openingBase?.tradingDate ?? 'YYYY-MM-DD'}
                disabled={opBusy}
              />
              <button
                type="button"
                data-testid="fetch-opening-base-spi-btn"
                disabled={opBusy || !canSpiByHint}
                title="服务端 RBAC 要求 FEED_ADAPTER 或 OPERATOR"
                onClick={() => {
                  void handleFetchOpeningBaseSpi();
                }}
              >
                SPI 拉取开盘基线 (FEED_ADAPTER/OPERATOR)
              </button>
              {!canSpiByHint && (
                <span data-testid="spi-role-hint-notice">
                  当前本地角色提示 ({formatPrincipalRoleLabel(state.roleHint)}) 非 FEED_ADAPTER/OPERATOR，SPI
                  拉取已按提示禁用
                </span>
              )}

              <button
                type="button"
                data-testid="import-opening-base-btn"
                disabled={opBusy || writesPaused || !canImportByHint}
                title="服务端 RBAC 要求 TRADER 或 OPERATOR"
                onClick={() => {
                  void handleImportOpeningBase();
                }}
              >
                导入开盘基线 ImportOpeningBase (TRADER/OPERATOR)
              </button>
              {!canImportByHint && (
                <span data-testid="import-role-hint-notice">
                  当前本地角色提示 ({formatPrincipalRoleLabel(state.roleHint)}) 非 TRADER/OPERATOR，导入基线已按提示禁用
                </span>
              )}
            </div>
          )}

          {/* Feed Control for Active Account Sources */}
          {client !== undefined && feedStatuses.length > 0 && (
            <div data-testid="feed-control-list">
              <h4>行情/成交源控制 (SetFeedControl)</h4>
              {feedStatuses.map((feed) => {
                const reasonVal = gapReasonsBySource[feed.sourceId] ?? '';
                return (
                  <div key={feed.sourceId} data-testid={`feed-control-row-${feed.sourceId}`}>
                    <span>
                      源 <strong>{feed.sourceId}</strong> | 状态：{formatFeedHealthState(feed.healthState)}{' '}
                      | rev={feed.feedStatusRevision.toString()} | 占仓暂停：
                      {feed.earmarksPaused ? '是' : '否'} | 缺段：
                      {formatMissingFeedRanges(feed.missingFeedRanges)}
                    </span>

                    <div>
                      <label htmlFor={`feed-reason-${feed.sourceId}`}>操作原因（强制清除缺口暂停必填）</label>
                      <input
                        id={`feed-reason-${feed.sourceId}`}
                        data-testid={`feed-reason-input-${feed.sourceId}`}
                        type="text"
                        value={reasonVal}
                        onChange={(e) =>
                          setGapReasonsBySource((prev) => ({
                            ...prev,
                            [feed.sourceId]: e.target.value,
                          }))
                        }
                        placeholder="填写操作原因"
                        disabled={opBusy || writesPaused}
                      />

                      <button
                        type="button"
                        data-testid={`feed-toggle-pause-${feed.sourceId}`}
                        disabled={opBusy || writesPaused || !canPauseByHint}
                        onClick={() => {
                          void handleSetFeedControl(feed, {
                            pausedByOperator: !feed.pausedByOperator,
                            forceClearGapPause: false,
                          });
                        }}
                      >
                        {feed.pausedByOperator ? '恢复人工暂停' : '人工暂停该源'}
                      </button>

                      <button
                        type="button"
                        data-testid={`feed-force-clear-gap-${feed.sourceId}`}
                        disabled={opBusy || writesPaused || !canForceClearByHint}
                        title="仅 OPERATOR 且必须填写非空原因（服务端 RBAC 强制）"
                        onClick={() => {
                          void handleSetFeedControl(feed, {
                            pausedByOperator: false,
                            forceClearGapPause: true,
                          });
                        }}
                      >
                        强制清除缺口暂停 (仅 OPERATOR)
                      </button>
                      {!canForceClearByHint && (
                        <span data-testid={`feed-force-clear-hint-${feed.sourceId}`}>
                          仅 OPERATOR 可执行强制清除缺口暂停
                        </span>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          )}

          {activeOpFeedback !== null && (
            <div
              role={activeOpFeedback.ok ? 'status' : 'alert'}
              data-testid="session-operation-feedback"
              data-rbac-denied={activeOpFeedback.isRbacDenied ? 'true' : 'false'}
            >
              <strong>{activeOpFeedback.operation}</strong>: {activeOpFeedback.message}
            </div>
          )}
        </div>
      )}
    </section>
  );
}
