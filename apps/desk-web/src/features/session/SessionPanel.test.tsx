import '@testing-library/jest-dom/vitest';
import { create } from '@bufbuild/protobuf';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  AccountBlotterSnapshotSchema,
  CoverageStatus,
  FeedHealthState,
  FeedStatusViewSchema,
  InstrumentPositionViewSchema,
  OpeningPositionBaseSchema,
  PrincipalRole,
  RiskAndCoverageSummarySchema,
  SeqRangeSchema,
  type AccountBlotterSnapshot,
} from '../../generated/ficant/blotter/v1/model_pb';
import {
  GetAccountSnapshotResultSchema,
  ImportOpeningBaseResultSchema,
  SetFeedControlResultSchema,
  type GetAccountSnapshotResult,
  type SubscribeEventsResponse,
} from '../../generated/ficant/blotter/v1/service_pb';
import { UlidSchema } from '../../generated/ficant/core/v1/common_pb';
import { ErrorCode, ErrorDetailSchema } from '../../generated/ficant/core/v1/error_pb';
import { AnalysisUnitsSchema, BondTaxBasis } from '../../generated/ficant/rates/v1/analytics_pb';
import { createUnitRef } from '../../codec';
import {
  BlotterClientError,
  InMemoryCredentialStore,
  type DeskBlotterRpcClient,
} from '../../transport';
import { BlotterSupervisor, createBlotterStore } from '../../store';
import { SessionPanel } from './index';

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

function makeSnapshot(
  accountId: string,
  seq: bigint,
  options: {
    readonly accountVersion?: bigint;
    readonly sourceId?: string;
    readonly feedGapPaused?: boolean;
  } = {},
): AccountBlotterSnapshot {
  const sourceId = options.sourceId ?? 'feed-src-1';
  return create(AccountBlotterSnapshotSchema, {
    snapshotSeq: seq,
    accountId,
    accountVersion: options.accountVersion ?? 3n,
    openingBase: create(OpeningPositionBaseSchema, {
      accountId,
      baseId: create(UlidSchema, { value: '01JNB0G1BASEID000000000001' }),
      tradingDate: '2026-11-21',
      sourceId,
      units: makeUnits(),
    }),
    riskSummary: create(RiskAndCoverageSummarySchema, {
      coveredInstrumentCount: 1,
      uncoveredInstrumentCount: 0,
      coveredTaxBasis: BondTaxBasis.PRE_TAX,
      wholeAccountHedgeAllowed: true,
    }),
    feedStatuses: [
      create(FeedStatusViewSchema, {
        sourceId,
        feedStatusRevision: 7n,
        healthState: options.feedGapPaused
          ? FeedHealthState.GAP_DETECTED
          : FeedHealthState.HEALTHY,
        earmarksPaused: Boolean(options.feedGapPaused),
        pausedByGap: Boolean(options.feedGapPaused),
        pausedByStaleness: false,
        pausedByOperator: false,
        missingFeedRanges: options.feedGapPaused
          ? [create(SeqRangeSchema, { first: 11n, last: 13n })]
          : [],
      }),
    ],
    positions: [
      create(InstrumentPositionViewSchema, {
        accountId,
        instrumentCode: '240011.IB',
        positionVersion: 1n,
        coverageStatus: CoverageStatus.COVERED_PRE_TAX,
        taxBasis: BondTaxBasis.PRE_TAX,
      }),
    ],
    allowedHedgeProductCodes: ['T', 'TF', 'TL', 'TS'],
  });
}

function createHangingStream(): AsyncIterable<SubscribeEventsResponse> {
  return {
    [Symbol.asyncIterator](): AsyncIterator<SubscribeEventsResponse> {
      return {
        next: () => new Promise<IteratorResult<SubscribeEventsResponse>>(() => undefined),
      };
    },
  };
}

function createMockClient(
  overrides: Partial<DeskBlotterRpcClient> = {},
): DeskBlotterRpcClient {
  const unexpected = (name: string) => () => {
    throw new Error(`Unexpected RPC call: ${name}`);
  };
  return {
    getAccountSnapshot: vi.fn(async ({ accountId }) =>
      create(GetAccountSnapshotResultSchema, {
        snapshot: makeSnapshot(accountId, 10n),
      }),
    ),
    pollEvents: vi.fn(unexpected('pollEvents')),
    subscribeEvents: vi.fn(() => createHangingStream()),
    previewCoveredHedge: vi.fn(unexpected('previewCoveredHedge')),
    importOpeningBase: vi.fn(unexpected('importOpeningBase')),
    ingestMarketQuotes: vi.fn(unexpected('ingestMarketQuotes')),
    createHold: vi.fn(unexpected('createHold')),
    extendHold: vi.fn(unexpected('extendHold')),
    confirmHold: vi.fn(unexpected('confirmHold')),
    releaseHold: vi.fn(unexpected('releaseHold')),
    createCommitted: vi.fn(unexpected('createCommitted')),
    cancelCommitted: vi.fn(unexpected('cancelCommitted')),
    submitDraftEarmark: vi.fn(unexpected('submitDraftEarmark')),
    confirmDraftEarmark: vi.fn(unexpected('confirmDraftEarmark')),
    ingestTradeFeed: vi.fn(unexpected('ingestTradeFeed')),
    resolvePendingMatch: vi.fn(unexpected('resolvePendingMatch')),
    sweepLifecycle: vi.fn(unexpected('sweepLifecycle')),
    setFeedControl: vi.fn(unexpected('setFeedControl')),
    fetchOpeningPositionBase: vi.fn(unexpected('fetchOpeningPositionBase')),
    parseNlpIntent: vi.fn(unexpected('parseNlpIntent')),
    ...overrides,
  };
}

describe('SessionPanel', () => {
  it('blocks credential entry over non-loopback plain HTTP with INSECURE_HTTP_CONTEXT', () => {
    const store = createBlotterStore();
    const credStore = new InMemoryCredentialStore();
    const client = createMockClient();
    const insecureLocation = { protocol: 'http:', hostname: '10.20.30.40' };
    const supervisor = new BlotterSupervisor({
      store,
      client,
      credentialStore: credStore,
      location: insecureLocation,
    });

    render(
      <SessionPanel
        store={store}
        supervisor={supervisor}
        client={client}
        location={insecureLocation}
      />,
    );

    expect(screen.getByTestId('insecure-context-banner')).toHaveTextContent('INSECURE_HTTP_CONTEXT');
    expect(screen.getByTestId('login-submit-btn')).toBeDisabled();
    expect(screen.getByLabelText(/访问令牌/)).toBeDisabled();
    expect(credStore.hasCredential()).toBe(false);
    expect(client.getAccountSnapshot).not.toHaveBeenCalled();
    expect(client.subscribeEvents).not.toHaveBeenCalled();

    supervisor.dispose();
  });

  it('validates memory-only credentials, rejects whitespace/control tokens, and never touches Web Storage', async () => {
    const user = userEvent.setup();
    const store = createBlotterStore();
    const credStore = new InMemoryCredentialStore();
    const client = createMockClient();
    const secureLoc = { protocol: 'https:', hostname: 'desk.internal' };
    const supervisor = new BlotterSupervisor({
      store,
      client,
      credentialStore: credStore,
      location: secureLoc,
    });

    const localStorageSpy = vi.spyOn(Storage.prototype, 'setItem');

    render(
      <SessionPanel
        store={store}
        supervisor={supervisor}
        client={client}
        location={secureLoc}
      />,
    );

    // Reject empty token
    await user.click(screen.getByTestId('login-submit-btn'));
    expect(screen.getByTestId('auth-form-error')).toHaveTextContent(/must not be empty/i);
    expect(store.getState().authenticated).toBe(false);

    // Reject token containing whitespace
    await user.type(screen.getByLabelText(/访问令牌/), 'bad token');
    await user.click(screen.getByTestId('login-submit-btn'));
    expect(screen.getByTestId('auth-form-error')).toHaveTextContent(/whitespace|control/i);
    expect(store.getState().authenticated).toBe(false);

    // Switch to mutually exclusive x-ficant-credential mode and login with valid token
    await user.click(screen.getByLabelText(/Intranet Header Credential/));
    const tokenInput = screen.getByLabelText(/访问令牌/);
    await user.clear(tokenInput);
    await user.type(tokenInput, 'intranet-secret-tok-99');
    await user.click(screen.getByTestId('login-submit-btn'));

    expect(store.getState().authenticated).toBe(true);
    expect(store.getState().headerMode).toBe('x-ficant-credential');
    expect(screen.getByTestId('connection-badge')).toHaveAttribute('data-connection-state', 'idle');
    expect(screen.getByTestId('credential-mode-badge')).toHaveTextContent('Intranet Header');
    expect(screen.getByTestId('role-hint-badge')).toHaveTextContent('仅本地 UI 提示');
    expect(localStorageSpy).not.toHaveBeenCalled();
    expect(document.cookie).toBe('');

    // Empty account submit must NOT subscribe or poll
    await user.click(screen.getByTestId('submit-accounts-btn'));
    expect(screen.getByTestId('accounts-form-error')).toHaveTextContent('禁止空账户集合订阅');
    expect(client.getAccountSnapshot).not.toHaveBeenCalled();
    expect(client.subscribeEvents).not.toHaveBeenCalled();
    expect(client.pollEvents).not.toHaveBeenCalled();

    localStorageSpy.mockRestore();
    supervisor.dispose();
  });

  it('onboards explicit accounts, exposes server RBAC failure without role-hint override, allows remediation, and switches accounts', async () => {
    const user = userEvent.setup();
    const store = createBlotterStore();
    const credStore = new InMemoryCredentialStore();
    const client = createMockClient({
      getAccountSnapshot: vi.fn(async ({ accountId }) => {
        if (accountId === 'acct-forbidden') {
          throw BlotterClientError.fromErrorDetail(
            create(ErrorDetailSchema, {
              code: ErrorCode.FORBIDDEN,
              message: 'account acct-forbidden not in principal scope',
            }),
          );
        }
        return create(GetAccountSnapshotResultSchema, {
          snapshot: makeSnapshot(accountId, 20n),
        });
      }),
    });
    const secureLoc = { protocol: 'http:', hostname: '127.0.0.1' };
    const supervisor = new BlotterSupervisor({
      store,
      client,
      credentialStore: credStore,
      location: secureLoc,
    });

    render(
      <SessionPanel
        store={store}
        supervisor={supervisor}
        client={client}
        location={secureLoc}
        accountHintList={['acct-A', 'acct-B']}
      />,
    );

    // Login with OPERATOR local role hint (must NOT override server FORBIDDEN)
    await user.type(screen.getByLabelText(/访问令牌/), 'tok-trader-1');
    await user.selectOptions(screen.getByLabelText(/本地角色显示提示/), String(PrincipalRole.OPERATOR));
    await user.click(screen.getByTestId('login-submit-btn'));

    // Submit list containing a forbidden account
    await user.type(screen.getByLabelText(/显式账户 ID/), 'acct-A, acct-forbidden');
    await user.click(screen.getByTestId('submit-accounts-btn'));

    await waitFor(() => {
      expect(screen.getByTestId('account-error-acct-forbidden')).toHaveTextContent('FORBIDDEN');
    });
    expect(screen.getByTestId('account-error-acct-forbidden')).toHaveTextContent(
      'account acct-forbidden not in principal scope',
    );
    expect(client.subscribeEvents).not.toHaveBeenCalled();

    // Remove failed account and retry -> succeeds & subscribes
    await user.click(screen.getByTestId('remove-failed-account-acct-forbidden'));

    await waitFor(() => {
      expect(screen.getByTestId('connection-badge')).toHaveAttribute(
        'data-connection-state',
        'stream_live',
      );
    });
    expect(screen.getByTestId('account-tab-acct-A')).toHaveAttribute('aria-selected', 'true');
    expect(client.subscribeEvents).toHaveBeenCalledTimes(1);

    // Use account hint fill + submit for multi-account switching
    await user.click(screen.getByTestId('fill-account-hints-btn'));
    await user.click(screen.getByTestId('submit-accounts-btn'));

    await waitFor(() => {
      expect(screen.getByTestId('account-tab-acct-B')).toBeInTheDocument();
    });
    await user.click(screen.getByTestId('account-tab-acct-B'));
    expect(screen.getByTestId('active-account-badge')).toHaveTextContent('acct-B');

    supervisor.dispose();
  });

  it('wipes state on session switch/logout and drops late responses for old acct-B', async () => {
    const user = userEvent.setup();
    const store = createBlotterStore();
    const credStore = new InMemoryCredentialStore();

    let resolveLateB: ((val: GetAccountSnapshotResult) => void) | null = null;
    let callCount = 0;

    const client = createMockClient({
      getAccountSnapshot: vi.fn(({ accountId }): Promise<GetAccountSnapshotResult> => {
        callCount += 1;
        const id = accountId ?? '';
        if (id === 'acct-B' && callCount === 2) {
          return new Promise<GetAccountSnapshotResult>((resolve) => {
            resolveLateB = resolve;
          });
        }
        return Promise.resolve(
          create(GetAccountSnapshotResultSchema, {
            snapshot: makeSnapshot(id, 10n),
          }),
        );
      }),
    });
    const secureLoc = { protocol: 'http:', hostname: 'localhost' };
    const supervisor = new BlotterSupervisor({
      store,
      client,
      credentialStore: credStore,
      location: secureLoc,
    });

    render(
      <SessionPanel
        store={store}
        supervisor={supervisor}
        client={client}
        location={secureLoc}
      />,
    );

    // Session 1: login and start loading acct-A and slow acct-B
    await user.type(screen.getByLabelText(/访问令牌/), 'tok-session-1');
    await user.click(screen.getByTestId('login-submit-btn'));
    expect(screen.getByTestId('session-generation')).toHaveTextContent('Gen #1');

    await user.type(screen.getByLabelText(/显式账户 ID/), 'acct-A acct-B');
    await user.click(screen.getByTestId('submit-accounts-btn'));

    await waitFor(() => {
      expect(resolveLateB).not.toBeNull();
    });

    // Switch session via Logout -> increments sessionGeneration, wipes token & store
    await user.click(screen.getByTestId('logout-btn'));
    expect(store.getState().sessionGeneration).toBe(2);
    expect(credStore.hasCredential()).toBe(false);
    expect(Object.keys(store.getState().snapshots)).toHaveLength(0);

    // Login Session 3 with only acct-A
    await user.type(screen.getByLabelText(/访问令牌/), 'tok-session-2-only-a');
    await user.click(screen.getByTestId('login-submit-btn'));
    expect(store.getState().sessionGeneration).toBe(3);

    await user.type(screen.getByLabelText(/显式账户 ID/), 'acct-A');
    await user.click(screen.getByTestId('submit-accounts-btn'));

    await waitFor(() => {
      expect(screen.getByTestId('account-tab-acct-A')).toBeInTheDocument();
    });

    // Now resolve the late acct-B snapshot from Session 1!
    resolveLateB!(
      create(GetAccountSnapshotResultSchema, {
        snapshot: makeSnapshot('acct-B', 10n),
      }),
    );
    await new Promise((r) => setTimeout(r, 20));

    // Late acct-B MUST NOT appear in store or DOM
    expect(store.getState().snapshots['acct-B']).toBeUndefined();
    expect(screen.queryByTestId('account-tab-acct-B')).not.toBeInTheDocument();

    supervisor.dispose();
  });

  it('distinguishes SPI FetchOpeningPositionBase vs ImportOpeningBase role hints and surfaces server RBAC FORBIDDEN + mandatory force_clear_gap_pause reason', async () => {
    const user = userEvent.setup();
    const store = createBlotterStore();
    const credStore = new InMemoryCredentialStore();

    const fetchSpiMock = vi.fn(async () => {
      throw BlotterClientError.fromErrorDetail(
        create(ErrorDetailSchema, {
          code: ErrorCode.FORBIDDEN,
          message: 'FetchOpeningPositionBase requires FEED_ADAPTER or OPERATOR',
        }),
      );
    });
    const importBaseMock = vi.fn(
      async (_req: Parameters<DeskBlotterRpcClient['importOpeningBase']>[0]) =>
        create(ImportOpeningBaseResultSchema, {
          committedSeq: 25n,
          replayed: false,
        }),
    );
    const setFeedControlMock = vi.fn(async (req) => {
      if (req.forceClearGapPause) {
        throw BlotterClientError.fromErrorDetail(
          create(ErrorDetailSchema, {
            code: ErrorCode.FORBIDDEN,
            message: 'force_clear_gap_pause requires server OPERATOR role',
          }),
        );
      }
      return create(SetFeedControlResultSchema, {
        committedSeq: 26n,
        replayed: false,
      });
    });

    const client = createMockClient({
      getAccountSnapshot: vi.fn(async ({ accountId }) =>
        create(GetAccountSnapshotResultSchema, {
          snapshot: makeSnapshot(accountId, 20n, { accountVersion: 5n, feedGapPaused: true }),
        }),
      ),
      fetchOpeningPositionBase: fetchSpiMock,
      importOpeningBase: importBaseMock,
      setFeedControl: setFeedControlMock,
    });

    const secureLoc = { protocol: 'https:', hostname: 'desk.internal' };
    const supervisor = new BlotterSupervisor({
      store,
      client,
      credentialStore: credStore,
      location: secureLoc,
    });

    render(
      <SessionPanel
        store={store}
        supervisor={supervisor}
        client={client}
        location={secureLoc}
      />,
    );

    // 1) Login as TRADER hint -> SPI & force_clear_gap_pause disabled by local hint, Import & normal pause enabled
    await user.type(screen.getByLabelText(/访问令牌/), 'tok-trader');
    await user.selectOptions(screen.getByLabelText(/本地角色显示提示/), String(PrincipalRole.TRADER));
    await user.click(screen.getByTestId('login-submit-btn'));
    await user.type(screen.getByLabelText(/显式账户 ID/), 'acct-A');
    await user.click(screen.getByTestId('submit-accounts-btn'));

    await waitFor(() => {
      expect(screen.getByTestId('baseline-provenance-badge')).toBeInTheDocument();
    });

    expect(screen.getByTestId('fetch-opening-base-spi-btn')).toBeDisabled();
    expect(screen.getByTestId('spi-role-hint-notice')).toBeInTheDocument();
    expect(screen.getByTestId('import-opening-base-btn')).toBeEnabled();
    expect(screen.getByTestId('feed-force-clear-gap-feed-src-1')).toBeDisabled();
    expect(screen.getByTestId('feed-toggle-pause-feed-src-1')).toBeEnabled();

    // Execute ImportOpeningBase -> sends expectedAccountVersion=5n, does NOT advance cursorSeq!
    const cursorBefore = store.getState().cursorSeq;
    await user.click(screen.getByTestId('import-opening-base-btn'));
    await waitFor(() => {
      expect(importBaseMock).toHaveBeenCalledTimes(1);
    });
    expect(importBaseMock.mock.calls[0]?.[0]?.expectedAccountVersion).toBe(5n);
    expect(store.getState().cursorSeq).toBe(cursorBefore);
    expect(screen.getByTestId('baseline-provenance-badge')).toHaveAttribute(
      'data-provenance-kind',
      'import',
    );

    // 2) Switch to OPERATOR local hint (while server token is still TRADER) -> proves server RBAC cannot be bypassed!
    await user.click(screen.getByTestId('toggle-switch-auth-btn'));
    await user.type(screen.getByLabelText(/访问令牌/), 'tok-still-trader-on-server');
    await user.selectOptions(screen.getByLabelText(/本地角色显示提示/), String(PrincipalRole.OPERATOR));
    await user.click(screen.getByTestId('login-submit-btn'));
    await user.clear(screen.getByLabelText(/显式账户 ID/));
    await user.type(screen.getByLabelText(/显式账户 ID/), 'acct-A');
    await user.click(screen.getByTestId('submit-accounts-btn'));

    await waitFor(() => {
      expect(screen.getByTestId('fetch-opening-base-spi-btn')).toBeEnabled();
    });

    // SPI call with OPERATOR hint -> server rejects FORBIDDEN and UI surfaces RBAC denial
    await user.click(screen.getByTestId('fetch-opening-base-spi-btn'));
    await waitFor(() => {
      expect(screen.getByTestId('session-operation-feedback')).toHaveAttribute(
        'data-rbac-denied',
        'true',
      );
    });
    expect(screen.getByTestId('session-operation-feedback')).toHaveTextContent(
      'FetchOpeningPositionBase requires FEED_ADAPTER or OPERATOR',
    );

    // force_clear_gap_pause with empty reason -> blocked client-side BEFORE RPC
    await user.click(screen.getByTestId('feed-force-clear-gap-feed-src-1'));
    expect(setFeedControlMock).not.toHaveBeenCalled();
    expect(screen.getByTestId('session-operation-feedback')).toHaveTextContent(
      /force_clear_gap_pause requires a non-empty reason/,
    );

    // force_clear_gap_pause with non-empty reason -> calls RPC with expectedFeedStatusRevision=7n & surfaces server FORBIDDEN
    await user.type(screen.getByTestId('feed-reason-input-feed-src-1'), '人工核对缺段后确认恢复');
    await user.click(screen.getByTestId('feed-force-clear-gap-feed-src-1'));
    await waitFor(() => {
      expect(setFeedControlMock).toHaveBeenCalledTimes(1);
    });
    expect(setFeedControlMock.mock.calls[0]?.[0]).toMatchObject({
      sourceId: 'feed-src-1',
      expectedFeedStatusRevision: 7n,
      forceClearGapPause: true,
      reason: '人工核对缺段后确认恢复',
    });
    expect(screen.getByTestId('session-operation-feedback')).toHaveAttribute(
      'data-rbac-denied',
      'true',
    );
    expect(screen.getByTestId('session-operation-feedback')).toHaveTextContent(
      'force_clear_gap_pause requires server OPERATOR role',
    );

    supervisor.dispose();
  });
});
