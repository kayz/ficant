import { useCallback, useEffect, useMemo, useRef, useState, type ReactElement } from 'react';
import { useStore } from 'zustand';
import {
  TRUSTED_RULEPACK_ANCHOR,
  type TrustedRulePackBuildAnchor,
} from 'virtual:ficant-trusted-rulepack';
import type { MarketTime } from '../generated/ficant/core/v1/common_pb';
import type { LoadTrustedRulePackOptions } from '../codec/rulepack';
import {
  createDeskBlotterClient,
  InMemoryCredentialStore,
  WriteIntentManager,
  type DeskBlotterRpcClient,
  type LocationLike,
} from '../transport';
import {
  BlotterSupervisor,
  createBlotterStore,
  selectSelectedSnapshot,
  type BlotterStoreApi,
} from '../store';
import { DraftDrawerPanel } from '../features/draft-drawer';
import { ExposureChartPanel, type EChartsInitFactory } from '../features/exposure-chart';
import { HedgePreviewPanel } from '../features/hedge-preview';
import { PendingMatchPanel } from '../features/pending-match';
import { PositionsGridPanel } from '../features/positions-grid';
import { parseExplicitAccountIds, SessionPanel } from '../features/session';
import { FundingRiskTopBar } from '../features/top-bar';
import { isDraftDrawerPasteShortcut } from './keyboardShortcuts';
import { useViewportLayout } from './useViewportLayout';

export interface DeskAppProps {
  readonly store?: BlotterStoreApi | undefined;
  readonly client?: DeskBlotterRpcClient | undefined;
  readonly credentialStore?: InMemoryCredentialStore | undefined;
  readonly supervisor?: BlotterSupervisor | undefined;
  readonly writeIntents?: WriteIntentManager | undefined;
  readonly location?: LocationLike | undefined;
  readonly initialViewportWidth?: number | undefined;
  readonly chartRenderer?: 'svg' | 'canvas' | undefined;
  readonly echartsFactory?: EChartsInitFactory | undefined;
  readonly accountHintList?: readonly string[] | undefined;
  readonly rulePackAnchor?: TrustedRulePackBuildAnchor | null | undefined;
  readonly rulePackOptions?: LoadTrustedRulePackOptions | undefined;
  readonly nowMs?: (() => number) | undefined;
  readonly nowProvider?: (() => Date) | undefined;
  readonly valuationAt?: Date | undefined;
  readonly valuationAtOverride?: MarketTime | undefined;
}

interface DeskRuntimeServices {
  readonly credentialStore: InMemoryCredentialStore;
  readonly store: BlotterStoreApi;
  readonly client: DeskBlotterRpcClient;
  readonly writeIntents: WriteIntentManager;
  readonly supervisor: BlotterSupervisor;
  readonly ownsSupervisor: boolean;
}

function scrollToSection(sectionId: string): void {
  if (typeof document === 'undefined') {
    return;
  }
  const el = document.getElementById(sectionId);
  if (el && typeof el.scrollIntoView === 'function') {
    el.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }
}

export function DeskApp(props: DeskAppProps): ReactElement {
  const {
    store: externalStore,
    client: externalClient,
    credentialStore: externalCredentialStore,
    supervisor: externalSupervisor,
    writeIntents: externalWriteIntents,
    location,
    initialViewportWidth,
    chartRenderer = 'svg',
    echartsFactory,
    accountHintList,
    rulePackAnchor,
    rulePackOptions,
    nowMs,
    nowProvider,
    valuationAt,
    valuationAtOverride,
  } = props;

  const runtimeRef = useRef<DeskRuntimeServices | null>(null);
  if (runtimeRef.current === null) {
    const credentialStore = externalCredentialStore ?? new InMemoryCredentialStore();
    const store = externalStore ?? createBlotterStore();
    const client =
      externalClient ??
      createDeskBlotterClient({
        credentialStore,
        ...(location !== undefined ? { location } : {}),
      });
    const writeIntents =
      externalWriteIntents ?? externalSupervisor?.writeIntents ?? new WriteIntentManager();
    const supervisor =
      externalSupervisor ??
      new BlotterSupervisor({
        store,
        client,
        credentialStore,
        writeIntents,
        ...(location !== undefined ? { location } : {}),
      });
    runtimeRef.current = {
      credentialStore,
      store,
      client,
      writeIntents,
      supervisor,
      ownsSupervisor: externalSupervisor === undefined,
    };
  }
  const runtime = runtimeRef.current;

  useEffect(() => {
    return () => {
      if (runtime.ownsSupervisor) {
        runtime.supervisor.dispose();
      }
    };
  }, [runtime]);

  const sessionGeneration = useStore(runtime.store, (s) => s.sessionGeneration);
  const authenticated = useStore(runtime.store, (s) => s.authenticated);
  const selectedSnapshot = useStore(runtime.store, selectSelectedSnapshot);
  const activeSnapshotSeq = selectedSnapshot ? selectedSnapshot.snapshotSeq.toString() : undefined;

  const { width: viewportWidth, mode: layoutMode } = useViewportLayout(initialViewportWidth);

  // Pre-auth account hint staging (cleared synchronously whenever sessionGeneration changes).
  const [stagedAccountsInput, setStagedAccountsInput] = useState<string>('');
  const [isDraftDrawerOpen, setIsDraftDrawerOpen] = useState<boolean>(false);
  const [draftFocusTrigger, setDraftFocusTrigger] = useState<number>(0);

  const prevSessionGenRef = useRef<number>(sessionGeneration);
  const draftTriggerBtnRef = useRef<HTMLButtonElement | null>(null);
  const draftTextareaRef = useRef<HTMLTextAreaElement | null>(null);

  useEffect(() => {
    if (prevSessionGenRef.current !== sessionGeneration) {
      prevSessionGenRef.current = sessionGeneration;
      setIsDraftDrawerOpen(false);
      if (!runtime.store.getState().authenticated) {
        setStagedAccountsInput('');
      }
    }
  }, [sessionGeneration, runtime.store]);

  const openDraftDrawer = useCallback((): void => {
    setIsDraftDrawerOpen(true);
    setDraftFocusTrigger((prev) => prev + 1);
  }, []);

  const closeDraftDrawer = useCallback((): void => {
    setIsDraftDrawerOpen(false);
    draftTriggerBtnRef.current?.focus();
  }, []);

  const toggleDraftDrawer = useCallback((): void => {
    if (isDraftDrawerOpen) {
      closeDraftDrawer();
    } else {
      openDraftDrawer();
    }
  }, [closeDraftDrawer, isDraftDrawerOpen, openDraftDrawer]);

  // Global keyboard handler:
  // - Ctrl+V / Cmd+V outside editable elements opens & focuses DraftDrawerPanel textarea
  //   without ever invoking silent clipboard APIs or hijacking normal input paste.
  // - Escape closes the drawer and restores focus to the toggle button.
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent): void => {
      if (isDraftDrawerPasteShortcut(event)) {
        event.preventDefault();
        openDraftDrawer();
        return;
      }
      if (event.key === 'Escape' && isDraftDrawerOpen) {
        event.preventDefault();
        closeDraftDrawer();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, [closeDraftDrawer, isDraftDrawerOpen, openDraftDrawer]);

  const stagedHintAccounts = useMemo(
    () => parseExplicitAccountIds(stagedAccountsInput),
    [stagedAccountsInput],
  );
  const effectiveAccountHintList = useMemo<readonly string[] | undefined>(() => {
    if (accountHintList !== undefined && accountHintList.length > 0) {
      return accountHintList;
    }
    if (stagedHintAccounts.length > 0) {
      return stagedHintAccounts;
    }
    return undefined;
  }, [accountHintList, stagedHintAccounts]);

  const resolvedRulePackAnchor =
    rulePackAnchor !== undefined ? rulePackAnchor : TRUSTED_RULEPACK_ANCHOR;
  const rulePackSummaryText =
    resolvedRulePackAnchor !== null
      ? `RulePack: ${resolvedRulePackAnchor.manifest.rule_pack_id} (v${resolvedRulePackAnchor.manifest.version})`
      : 'RulePack: UNAVAILABLE';

  const handleWriteCommitted = useCallback((): void => {
    runtime.supervisor.triggerCatchUp();
  }, [runtime.supervisor]);

  const handleResyncRequested = useCallback(async (): Promise<void> => {
    await runtime.supervisor.retryRecovery();
  }, [runtime.supervisor]);

  return (
    <div
      className="desk-shell"
      data-testid="desk-app-shell"
      data-desk-web-mode="desk"
      data-layout={layoutMode}
      data-viewport-width={String(viewportWidth)}
      data-snapshot-seq={activeSnapshotSeq}
    >
      {/* Formal Application Header */}
      <header className="desk-app-header">
        <div className="desk-app-header__identity">
          <h1 className="desk-app-header__title">FICANT 交易台盘中副屏 · Desk Blotter</h1>
          <span className="desk-app-header__rulepack" data-testid="trusted-rulepack-id">
            {rulePackSummaryText}
          </span>
        </div>

        <div className="desk-app-header__controls">
          <span className="desk-layout-badge" data-testid="desk-layout-badge">
            {layoutMode === 'narrow' ? '窄屏副屏 (<768px)' : '宽屏交易台 (>=768px)'}
          </span>
          <button
            ref={draftTriggerBtnRef}
            type="button"
            className="desk-drawer-trigger"
            data-testid="draft-drawer-toggle-btn"
            aria-expanded={isDraftDrawerOpen}
            aria-controls="desk-draft-drawer-region"
            onClick={toggleDraftDrawer}
          >
            {isDraftDrawerOpen ? '关闭草案抽屉 (Esc)' : '显式粘贴 / 草案抽屉 (Ctrl+V)'}
          </button>
        </div>
      </header>

      {/* Narrow-screen quick jump navigation (keeps all snapshot-synced panels mounted) */}
      {layoutMode === 'narrow' && (
        <nav
          className="desk-narrow-nav"
          aria-label="窄屏面板快捷导航"
          data-testid="narrow-panel-nav"
        >
          <button
            type="button"
            className="desk-narrow-nav__btn"
            onClick={() => scrollToSection('desk-section-session')}
          >
            会话与账户
          </button>
          <button
            type="button"
            className="desk-narrow-nav__btn"
            onClick={() => scrollToSection('desk-section-topbar')}
          >
            资金与风险
          </button>
          <button
            type="button"
            className="desk-narrow-nav__btn"
            onClick={() => scrollToSection('desk-section-positions')}
          >
            持仓与占仓
          </button>
          <button
            type="button"
            className="desk-narrow-nav__btn"
            onClick={() => scrollToSection('desk-section-pending')}
          >
            待核销
          </button>
          <button
            type="button"
            className="desk-narrow-nav__btn"
            onClick={() => scrollToSection('desk-section-hedge')}
          >
            套保试算
          </button>
          <button
            type="button"
            className="desk-narrow-nav__btn"
            onClick={() => scrollToSection('desk-section-chart')}
          >
            敞口图
          </button>
        </nav>
      )}

      {/* Session Onboarding, Auth & Explicit Account Subscription */}
      <div id="desk-section-session" className="desk-panel">
        <SessionPanel
          store={runtime.store}
          supervisor={runtime.supervisor}
          client={runtime.client}
          {...(location !== undefined ? { location } : {})}
          {...(effectiveAccountHintList !== undefined
            ? { accountHintList: effectiveAccountHintList }
            : {})}
        />

        {!authenticated && (
          <section
            className="desk-cold-start-staging"
            aria-label="显式账户 ID 预填入口"
            data-testid="cold-start-account-staging"
          >
            <label htmlFor="cold-start-account-ids-input">
              待订阅账户 ID（可选预填，建立内存会话后显式提交）
            </label>
            <input
              id="cold-start-account-ids-input"
              data-testid="cold-start-account-ids-input"
              type="text"
              autoComplete="off"
              value={stagedAccountsInput}
              onChange={(event) => setStagedAccountsInput(event.target.value)}
              placeholder="例如 acct-001, acct-002"
            />
            <span className="desk-cold-start-staging__hint">
              服务不提供账户列表枚举 RPC；建立会话后须由交易员显式提交账户 ID 以先取基线再订阅。
            </span>
          </section>
        )}
      </div>

      {/* Funding & Coverage Risk Top Bar */}
      <div id="desk-section-topbar" className="desk-panel desk-panel--topbar">
        <FundingRiskTopBar
          store={runtime.store}
          {...(rulePackAnchor !== undefined ? { rulePackAnchor } : {})}
          {...(valuationAt !== undefined ? { valuationAt } : {})}
        />
      </div>

      {/* Main Responsive Trading Desk Workspace */}
      <main className="desk-workspace" data-testid="desk-workspace-grid">
        <div className="desk-workspace__primary">
          <div id="desk-section-positions" className="desk-panel">
            <PositionsGridPanel
              store={runtime.store}
              client={runtime.client}
              writeIntents={runtime.writeIntents}
              onWriteCommitted={handleWriteCommitted}
              onResyncRequested={handleResyncRequested}
              {...(nowMs !== undefined ? { nowMs } : {})}
            />
          </div>

          <div id="desk-section-chart" className="desk-panel desk-panel--chart">
            <ExposureChartPanel
              store={runtime.store}
              renderer={chartRenderer}
              {...(echartsFactory !== undefined ? { echartsFactory } : {})}
            />
          </div>
        </div>

        <div className="desk-workspace__secondary">
          <div id="desk-section-hedge" className="desk-panel">
            <HedgePreviewPanel
              store={runtime.store}
              client={runtime.client}
              {...(rulePackOptions !== undefined ? { rulePackOptions } : {})}
              {...(valuationAtOverride !== undefined ? { valuationAtOverride } : {})}
              {...(nowProvider !== undefined ? { nowProvider } : {})}
            />
          </div>

          <div id="desk-section-pending" className="desk-panel">
            <PendingMatchPanel
              store={runtime.store}
              client={runtime.client}
              supervisor={runtime.supervisor}
              writeIntents={runtime.writeIntents}
            />
          </div>
        </div>
      </main>

      {/* Explicit Paste & Draft Reconfirmation Drawer */}
      {isDraftDrawerOpen && (
        <div
          id="desk-draft-drawer-region"
          className="desk-drawer-backdrop"
          data-testid="draft-drawer-backdrop"
          onClick={(event) => {
            if (event.target === event.currentTarget) {
              closeDraftDrawer();
            }
          }}
        >
          <div className="desk-drawer-host">
            <DraftDrawerPanel
              store={runtime.store}
              client={runtime.client}
              writeIntents={runtime.writeIntents}
              supervisor={runtime.supervisor}
              isOpen={isDraftDrawerOpen}
              onClose={closeDraftDrawer}
              focusTrigger={draftFocusTrigger}
              inputRef={draftTextareaRef}
              onRefreshContext={handleResyncRequested}
            />
          </div>
        </div>
      )}
    </div>
  );
}
