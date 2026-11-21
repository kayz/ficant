import '@testing-library/jest-dom/vitest';
import { create } from '@bufbuild/protobuf';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { TRUSTED_RULEPACK_ANCHOR } from 'virtual:ficant-trusted-rulepack';
import {
  AccountBlotterSnapshotSchema,
  AccountChangeSchema,
  BlotterEventEnvelopeSchema,
  CoverageStatus,
  InstrumentPositionViewSchema,
  OpeningPositionBaseSchema,
  PreviewHedgeScope,
  RiskAndCoverageSummarySchema,
  type AccountBlotterSnapshot,
} from '../../generated/ficant/blotter/v1/model_pb';
import {
  GetAccountSnapshotResultSchema,
  PreviewCoveredHedgeResultSchema,
  type PreviewCoveredHedgeResult,
} from '../../generated/ficant/blotter/v1/service_pb';
import { DecimalValueSchema } from '../../generated/ficant/core/v1/common_pb';
import { ErrorCode, ErrorDetailSchema } from '../../generated/ficant/core/v1/error_pb';
import {
  AnalysisUnitsSchema,
  BondTaxBasis,
  FuturesHedgeMeasuresSchema,
} from '../../generated/ficant/rates/v1/analytics_pb';
import {
  PINNED_RULEPACK_PAYLOAD_BYTES,
  createDecimalValue,
  createMarketTimeFromIso,
  createUnitRef,
  sha256PureHex,
  type LoadTrustedRulePackOptions,
} from '../../codec';
import { BlotterClientError, type DeskBlotterRpcClient } from '../../transport';
import { createBlotterStore, type BlotterStoreApi } from '../../store';
import { HedgePreviewPanel } from './index';

afterEach(() => {
  cleanup();
});

const UNIT_CCY = createUnitRef('01JNB0G1UNITCURRAMT0000001', 1n);
const UNIT_PRICE = createUnitRef('01JNB0G1UNITPRICE000000002', 1n);
const UNIT_RATE = createUnitRef('01JNB0G1UNITRATE0000000003', 1n);
const UNIT_DV01 = createUnitRef('01JNB0G1UNITDV010000000004', 1n);
const UNIT_DV01P100 = createUnitRef('01JNB0G1UNITDV01P100000005', 1n);
const UNIT_DIM = createUnitRef('01JNB0G1UNITDIMLESS0000006', 1n);
const UNIT_CNT = createUnitRef('01JNB0G1UNITCNTRCNT0000007', 1n);

const VALUATION_AT = createMarketTimeFromIso(
  '2026-06-15T02:00:00Z',
  'Asia/Shanghai',
  '2026-06-15',
);

function makeUnits() {
  return create(AnalysisUnitsSchema, {
    currencyAmount: UNIT_CCY,
    pricePer100: UNIT_PRICE,
    rate: UNIT_RATE,
    dv01: UNIT_DV01,
    dv01Per100: UNIT_DV01P100,
    dimensionless: UNIT_DIM,
    contractCount: UNIT_CNT,
  });
}

function makeTrustedRulePackOpts(): LoadTrustedRulePackOptions {
  const payload = new Uint8Array(PINNED_RULEPACK_PAYLOAD_BYTES).fill(0x42);
  const payloadSha = sha256PureHex(payload);
  const manifest = { ...TRUSTED_RULEPACK_ANCHOR.manifest, payload_sha256: payloadSha };
  const manifestBytes = new TextEncoder().encode(`${JSON.stringify(manifest, null, 2)}\n`);
  const manifestSha = sha256PureHex(manifestBytes);
  return {
    expectedManifestSha256: manifestSha,
    expectedPayloadSha256: payloadSha,
    anchor: {
      ...TRUSTED_RULEPACK_ANCHOR,
      manifestSha256: manifestSha,
      payloadSha256: payloadSha,
      manifest,
    },
    payloadBytes: payload,
    subtleOverride: null,
  };
}

function makeSnapshot(
  accountId: string,
  seq: bigint,
  options: {
    readonly allowedProducts?: readonly string[];
    readonly coveredCount?: number;
    readonly uncoveredCount?: number;
    readonly uncoveredGross?: string;
    readonly uncoveredNet?: string;
  } = {},
): AccountBlotterSnapshot {
  const uncoveredCount = options.uncoveredCount ?? 0;
  return create(AccountBlotterSnapshotSchema, {
    snapshotSeq: seq,
    accountId,
    accountVersion: 1n,
    openingBase: create(OpeningPositionBaseSchema, {
      accountId,
      effectiveFrom: VALUATION_AT,
      units: makeUnits(),
    }),
    riskSummary: create(RiskAndCoverageSummarySchema, {
      coveredInstrumentCount: options.coveredCount ?? 2,
      uncoveredInstrumentCount: uncoveredCount,
      coveredTaxBasis: BondTaxBasis.PRE_TAX,
      coveredNetPreTaxDv01: createDecimalValue('5000', UNIT_DV01),
      coveredGrossNotional: createDecimalValue('20000000', UNIT_CCY),
      uncoveredGrossNotional:
        options.uncoveredGross !== undefined
          ? createDecimalValue(options.uncoveredGross, UNIT_CCY)
          : createDecimalValue('0', UNIT_CCY),
      uncoveredNetNotional:
        options.uncoveredNet !== undefined
          ? createDecimalValue(options.uncoveredNet, UNIT_CCY)
          : createDecimalValue('0', UNIT_CCY),
      wholeAccountHedgeAllowed: uncoveredCount === 0,
    }),
    positions: [
      create(InstrumentPositionViewSchema, {
        accountId,
        instrumentCode: '240011.IB',
        positionVersion: 1n,
        coverageStatus: CoverageStatus.COVERED_PRE_TAX,
        taxBasis: BondTaxBasis.PRE_TAX,
      }),
    ],
    allowedHedgeProductCodes: [...(options.allowedProducts ?? ['T', 'TF', 'TL', 'TS'])],
  });
}

function seedStore(store: BlotterStoreApi, snapshots: readonly AccountBlotterSnapshot[]) {
  const gen = store.getState().beginSession({ headerMode: 'authorization' });
  const fence = store.getState().activateReader(gen, 'stream', 'stream_live');
  if (!fence) {
    throw new Error('failed to activate reader');
  }
  const map = new Map<string, AccountBlotterSnapshot>();
  let maxSeq = 0n;
  for (const snap of snapshots) {
    map.set(snap.accountId, snap);
    if (snap.snapshotSeq > maxSeq) {
      maxSeq = snap.snapshotSeq;
    }
  }
  const commit = store.getState().commitBaselineSnapshots({
    ...fence,
    snapshots: map,
    cursorSeq: maxSeq,
    headSeq: maxSeq,
    nextConnectionState: 'stream_live',
  });
  if (!commit.committed) {
    throw new Error(`commit failed: ${commit.reason}`);
  }
  return fence;
}

function makeValidHedgeResult(
  overrides: {
    readonly evaluatedAtSeq?: bigint;
    readonly scope?: PreviewHedgeScope;
    readonly productCode?: string;
    readonly contractCode?: string;
    readonly zeroNetExposure?: boolean;
    readonly coveredTaxBasis?: BondTaxBasis;
    readonly coveredInstrumentCount?: number;
    readonly uncoveredInstrumentCount?: number;
    readonly contractNotional?: string;
    readonly recommendedContracts?: bigint;
    readonly excludedUncoveredGrossNotional?: ReturnType<typeof createDecimalValue> | undefined;
  } = {},
): PreviewCoveredHedgeResult {
  const zeroNet = overrides.zeroNetExposure ?? false;
  return create(PreviewCoveredHedgeResultSchema, {
    evaluatedAtSeq: overrides.evaluatedAtSeq ?? 10n,
    scope: overrides.scope ?? PreviewHedgeScope.WHOLE_ACCOUNT,
    productCode: overrides.productCode ?? 'TS',
    contractCode: overrides.contractCode ?? 'TS2609',
    zeroNetExposure: zeroNet,
    coveredTaxBasis: overrides.coveredTaxBasis ?? BondTaxBasis.PRE_TAX,
    coveredInstrumentCount: overrides.coveredInstrumentCount ?? 2,
    uncoveredInstrumentCount: overrides.uncoveredInstrumentCount ?? 0,
    coveredNetPreTaxDv01: createDecimalValue(zeroNet ? '0' : '5000', UNIT_DV01),
    excludedUncoveredGrossNotional: overrides.excludedUncoveredGrossNotional,
    measures: create(FuturesHedgeMeasuresSchema, {
      futuresContractDv01: createDecimalValue('400', UNIT_DV01),
      rawContracts: createDecimalValue(zeroNet ? '0' : '12.5', UNIT_CNT),
      recommendedContracts: overrides.recommendedContracts ?? (zeroNet ? 0n : 13n),
      residualDv01: createDecimalValue(zeroNet ? '0' : '-200', UNIT_DV01),
      hedgeEffectiveness: createDecimalValue('0.96', UNIT_DIM),
      contractNotional: createDecimalValue(overrides.contractNotional ?? '2000000', UNIT_CCY),
    }),
  });
}

async function fillAndSubmitHedgeForm(
  user: ReturnType<typeof userEvent.setup>,
  opts: {
    readonly productCode?: string;
    readonly contractCode?: string;
    readonly ctdDv01?: string;
    readonly cf?: string;
  } = {},
) {
  if (opts.productCode !== undefined) {
    await user.selectOptions(screen.getByTestId('hedge-product-select'), opts.productCode);
  }
  const contractInput = screen.getByTestId('hedge-contract-input');
  await user.clear(contractInput);
  await user.type(contractInput, opts.contractCode ?? 'TS2609');

  const dv01Input = screen.getByTestId('hedge-ctd-dv01-input');
  await user.clear(dv01Input);
  await user.type(dv01Input, opts.ctdDv01 ?? '0.021');

  const cfInput = screen.getByTestId('hedge-cf-input');
  await user.clear(cfInput);
  await user.type(cfInput, opts.cf ?? '0.985');

  await user.click(screen.getByTestId('hedge-preview-submit-btn'));
}

describe('HedgePreviewPanel', () => {
  it('renders Policy B (4 products) and Policy A (3 products) dynamically from snapshot without hardcoding, and verifies TS 2,000,000 from server response', async () => {
    const user = userEvent.setup();
    const store = createBlotterStore();
    seedStore(store, [
      makeSnapshot('acct-policy-b', 10n, { allowedProducts: ['T', 'TF', 'TL', 'TS'] }),
      makeSnapshot('acct-policy-a', 10n, { allowedProducts: ['T', 'TF', 'TL'] }),
    ]);

    const previewCoveredHedge = vi.fn(async () =>
      makeValidHedgeResult({
        evaluatedAtSeq: 10n,
        scope: PreviewHedgeScope.WHOLE_ACCOUNT,
        productCode: 'TS',
        contractCode: 'TS2609',
        contractNotional: '2000000',
        recommendedContracts: 13n,
      }),
    );
    const client: Pick<DeskBlotterRpcClient, 'getAccountSnapshot' | 'previewCoveredHedge'> = {
      getAccountSnapshot: vi.fn(),
      previewCoveredHedge,
    };

    render(
      <HedgePreviewPanel
        store={store}
        client={client}
        rulePackOptions={makeTrustedRulePackOpts()}
        valuationAtOverride={VALUATION_AT}
      />,
    );

    const selectEl = screen.getByTestId('hedge-product-select') as HTMLSelectElement;
    expect(Array.from(selectEl.options).map((o) => o.value)).toEqual(['T', 'TF', 'TL', 'TS']);

    await fillAndSubmitHedgeForm(user, { productCode: 'TS', contractCode: 'TS2609' });

    await waitFor(() => {
      expect(screen.getByTestId('hedge-recommended-contracts')).toHaveTextContent('13');
    });
    expect(screen.getByTestId('hedge-contract-notional')).toHaveTextContent('2,000,000');
    expect(screen.getByTestId('hedge-contract-notional-unit')).toHaveTextContent(
      '01JNB0G1UNITCURRAMT0000001@v1',
    );
    expect(screen.getByTestId('hedge-result-metadata')).toHaveTextContent('evaluated_at_seq=10');

    // Switch to Policy A account -> dropdown shrinks dynamically to 3 products
    store.getState().setSelectedAccountId('acct-policy-a');
    await waitFor(() => {
      expect(Array.from(selectEl.options).map((o) => o.value)).toEqual(['T', 'TF', 'TL']);
    });
  });

  it('disables WHOLE_ACCOUNT whenever uncovered_instrument_count > 0 (pure short AND net-zero-gross-nonzero)', () => {
    const store = createBlotterStore();
    seedStore(store, [
      makeSnapshot('acct-pure-short', 10n, {
        uncoveredCount: 1,
        uncoveredGross: '10000000',
        uncoveredNet: '-10000000',
      }),
      makeSnapshot('acct-net-zero-gross', 10n, {
        uncoveredCount: 2,
        uncoveredGross: '20000000',
        uncoveredNet: '0',
      }),
    ]);

    const client: Pick<DeskBlotterRpcClient, 'getAccountSnapshot' | 'previewCoveredHedge'> = {
      getAccountSnapshot: vi.fn(),
      previewCoveredHedge: vi.fn(),
    };

    render(
      <HedgePreviewPanel
        store={store}
        client={client}
        rulePackOptions={makeTrustedRulePackOpts()}
        valuationAtOverride={VALUATION_AT}
      />,
    );

    // Pure short uncovered
    expect(screen.getByTestId('scope-whole-account')).toBeDisabled();
    expect(screen.getByTestId('scope-covered-subset')).toBeChecked();
    expect(screen.getByTestId('whole-account-disabled-reason')).toHaveTextContent(
      'uncovered_instrument_count > 0',
    );

    // Switch to net-zero-gross-nonzero uncovered -> WHOLE_ACCOUNT MUST STILL be disabled
    store.getState().setSelectedAccountId('acct-net-zero-gross');
    expect(screen.getByTestId('scope-whole-account')).toBeDisabled();
    expect(screen.getByTestId('scope-covered-subset')).toBeChecked();
  });

  it('distinguishes empty covered book rejection vs covered net-zero exposure (0 lots)', async () => {
    const user = userEvent.setup();
    const store = createBlotterStore();
    seedStore(store, [
      makeSnapshot('acct-empty-covered', 10n, { coveredCount: 0 }),
      makeSnapshot('acct-zero-net', 10n, { coveredCount: 2 }),
    ]);

    const previewCoveredHedge = vi.fn(async (req: { accountId?: string }) => {
      if (req.accountId === 'acct-empty-covered') {
        throw BlotterClientError.fromErrorDetail(
          create(ErrorDetailSchema, {
            code: ErrorCode.VALIDATION_FAILED,
            message: 'no covered instruments in scope',
          }),
        );
      }
      return makeValidHedgeResult({
        evaluatedAtSeq: 10n,
        scope: PreviewHedgeScope.WHOLE_ACCOUNT,
        productCode: 'T',
        contractCode: 'T2609',
        zeroNetExposure: true,
        coveredInstrumentCount: 2,
        recommendedContracts: 0n,
        contractNotional: '1000000',
      });
    });

    const client: Pick<DeskBlotterRpcClient, 'getAccountSnapshot' | 'previewCoveredHedge'> = {
      getAccountSnapshot: vi.fn(),
      previewCoveredHedge,
    };

    render(
      <HedgePreviewPanel
        store={store}
        client={client}
        rulePackOptions={makeTrustedRulePackOpts()}
        valuationAtOverride={VALUATION_AT}
      />,
    );

    // Empty covered book
    expect(screen.getByTestId('empty-covered-scope-notice')).toBeInTheDocument();
    await fillAndSubmitHedgeForm(user, { productCode: 'T', contractCode: 'T2609' });
    await waitFor(() => {
      expect(screen.getByTestId('hedge-preview-error')).toHaveTextContent(
        'no covered instruments in scope',
      );
    });
    expect(screen.queryByTestId('hedge-recommended-contracts')).not.toBeInTheDocument();

    // Switch to covered net-zero exposure -> succeeds with distinct zero-net-exposure banner and 0 lots
    store.getState().setSelectedAccountId('acct-zero-net');
    await fillAndSubmitHedgeForm(user, { productCode: 'T', contractCode: 'T2609' });
    await waitFor(() => {
      expect(screen.getByTestId('hedge-zero-net-exposure')).toHaveTextContent('零净敞口');
    });
    expect(screen.getByTestId('hedge-recommended-contracts')).toHaveTextContent('0');
  });

  it('performs bounded single private recompute on cross-account head_seq conflict without mutating workspace cursor, shows stale warning when workspace advances, and discards late responses', async () => {
    const user = userEvent.setup();
    const store = createBlotterStore();
    const fence = seedStore(store, [
      makeSnapshot('acct-A', 10n, { allowedProducts: ['T', 'TF'] }),
      makeSnapshot('acct-B', 10n, { allowedProducts: ['T', 'TF'] }),
    ]);

    let conflictAttempt = 0;
    const getAccountSnapshot = vi.fn(async () =>
      create(GetAccountSnapshotResultSchema, {
        snapshot: makeSnapshot('acct-A', 15n, { allowedProducts: ['T', 'TF'] }),
      }),
    );

    let resolveLateA: ((res: PreviewCoveredHedgeResult) => void) | null = null;
    const previewCoveredHedge = vi.fn(
      async (req: { accountId?: string; expectedSnapshotSeq?: bigint; contractCode?: string }) => {
        if (req.contractCode === 'T2609-LATE') {
          return new Promise<PreviewCoveredHedgeResult>((resolve) => {
            resolveLateA = resolve;
          });
        }
        conflictAttempt += 1;
        if (conflictAttempt === 1) {
          throw BlotterClientError.fromErrorDetail(
            create(ErrorDetailSchema, {
              code: ErrorCode.CONCURRENCY_CONFLICT,
              message: 'head_seq advanced by another account',
            }),
          );
        }
        return makeValidHedgeResult({
          evaluatedAtSeq: req.expectedSnapshotSeq ?? 15n,
          scope: PreviewHedgeScope.WHOLE_ACCOUNT,
          productCode: 'T',
          contractCode: req.contractCode ?? 'T2609',
          contractNotional: '1000000',
          recommendedContracts: 8n,
        });
      },
    );

    render(
      <HedgePreviewPanel
        store={store}
        client={{ getAccountSnapshot, previewCoveredHedge }}
        rulePackOptions={makeTrustedRulePackOpts()}
        valuationAtOverride={VALUATION_AT}
      />,
    );

    await fillAndSubmitHedgeForm(user, { productCode: 'T', contractCode: 'T2609' });

    await waitFor(() => {
      expect(screen.getByTestId('hedge-recommended-contracts')).toHaveTextContent('8');
    });
    expect(getAccountSnapshot).toHaveBeenCalledTimes(1);
    expect(previewCoveredHedge).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId('hedge-bounded-recompute-notice')).toHaveTextContent(
      'evaluated_at_seq=15',
    );
    // CRITICAL: Private hedge refresh MUST NOT mutate workspace store cursor or snapshot_seq!
    expect(store.getState().cursorSeq).toBe(10n);
    expect(store.getState().snapshots['acct-A']?.snapshotSeq).toBe(10n);

    // Advance workspace snapshot for acct-A to seq=20 -> stalePreviewWarning MUST appear
    const commitRes = store.getState().commitStreamEnvelope({
      ...fence,
      envelope: create(BlotterEventEnvelopeSchema, {
        seq: 20n,
        eventType: 'QUOTE_TICK',
        changes: [
          create(AccountChangeSchema, {
            accountId: 'acct-A',
            replaceSnapshot: makeSnapshot('acct-A', 20n, { allowedProducts: ['T', 'TF'] }),
          }),
        ],
      }),
    });
    expect(commitRes.committed).toBe(true);
    await waitFor(() => {
      expect(screen.getByTestId('stale-preview-warning')).toHaveTextContent('seq=20');
    });

    // Late response isolation: kick off slow request on acct-A, then switch to acct-B before it resolves
    await fillAndSubmitHedgeForm(user, { productCode: 'T', contractCode: 'T2609-LATE' });
    await waitFor(() => {
      expect(resolveLateA).not.toBeNull();
    });
    store.getState().setSelectedAccountId('acct-B');

    resolveLateA!(
      makeValidHedgeResult({
        evaluatedAtSeq: 20n,
        scope: PreviewHedgeScope.WHOLE_ACCOUNT,
        productCode: 'T',
        contractCode: 'T2609-LATE',
        recommendedContracts: 99n,
      }),
    );
    await new Promise((r) => setTimeout(r, 20));

    // Late 99-lot response for acct-A MUST be discarded
    expect(screen.queryByTestId('hedge-recommended-contracts')).not.toBeInTheDocument();
    expect(store.getState().hedgePreview?.result?.measures?.recommendedContracts).not.toBe(99n);
  });

  it('blocks RPCs (0 RPCs) on tampered/expired RulePack and clears prior lots fail-closed', async () => {
    const user = userEvent.setup();
    const store = createBlotterStore();
    seedStore(store, [makeSnapshot('acct-A', 10n)]);

    const previewCoveredHedge = vi.fn(async () =>
      makeValidHedgeResult({
        evaluatedAtSeq: 10n,
        productCode: 'T',
        contractCode: 'T2609',
      }),
    );

    const expiredAt = createMarketTimeFromIso(
      TRUSTED_RULEPACK_ANCHOR.manifest.effective_to.instant,
      TRUSTED_RULEPACK_ANCHOR.manifest.effective_to.market_timezone,
      TRUSTED_RULEPACK_ANCHOR.manifest.effective_to.local_trading_date,
    );

    render(
      <HedgePreviewPanel
        store={store}
        client={{ getAccountSnapshot: vi.fn(), previewCoveredHedge }}
        rulePackOptions={makeTrustedRulePackOpts()}
        valuationAtOverride={expiredAt}
      />,
    );

    await fillAndSubmitHedgeForm(user, { productCode: 'T', contractCode: 'T2609' });

    await waitFor(() => {
      expect(screen.getByTestId('hedge-preview-error')).toBeInTheDocument();
    });
    expect(previewCoveredHedge).toHaveBeenCalledTimes(0);
    expect(screen.queryByTestId('hedge-recommended-contracts')).not.toBeInTheDocument();
  });

  it.each<{ readonly label: string; readonly mutate: (base: PreviewCoveredHedgeResult) => PreviewCoveredHedgeResult }>([
    {
      label: 'missing measures',
      mutate: (r) => ({ ...r, measures: undefined }),
    },
    {
      label: 'missing contract_notional',
      mutate: (r) => ({
        ...r,
        measures: create(FuturesHedgeMeasuresSchema, {
          ...r.measures!,
          contractNotional: undefined,
        }),
      }),
    },
    {
      label: 'missing contract_notional.unit',
      mutate: (r) => ({
        ...r,
        measures: create(FuturesHedgeMeasuresSchema, {
          ...r.measures!,
          contractNotional: create(DecimalValueSchema, {
            coefficient: '1000000',
            scale: 0,
            unit: undefined,
          }),
        }),
      }),
    },
    {
      label: 'zero contract_notional (non-positive)',
      mutate: (r) => ({
        ...r,
        measures: create(FuturesHedgeMeasuresSchema, {
          ...r.measures!,
          contractNotional: createDecimalValue('0', UNIT_CCY),
        }),
      }),
    },
    {
      label: 'negative contract_notional',
      mutate: (r) => ({
        ...r,
        measures: create(FuturesHedgeMeasuresSchema, {
          ...r.measures!,
          contractNotional: createDecimalValue('-1000000', UNIT_CCY),
        }),
      }),
    },
    {
      label: 'non-canonical illegal decimal contract_notional',
      mutate: (r) => ({
        ...r,
        measures: create(FuturesHedgeMeasuresSchema, {
          ...r.measures!,
          contractNotional: create(DecimalValueSchema, {
            coefficient: '001000',
            scale: 0,
            unit: UNIT_CCY,
          }),
        }),
      }),
    },
    {
      label: 'wrong contract_notional unit_id',
      mutate: (r) => ({
        ...r,
        measures: create(FuturesHedgeMeasuresSchema, {
          ...r.measures!,
          contractNotional: createDecimalValue(
            '1000000',
            createUnitRef('01JNB0G1WRONGUNITID0000099', 1n),
          ),
        }),
      }),
    },
    {
      label: 'wrong contract_notional unit version',
      mutate: (r) => ({
        ...r,
        measures: create(FuturesHedgeMeasuresSchema, {
          ...r.measures!,
          contractNotional: createDecimalValue('1000000', createUnitRef(UNIT_CCY.unitId!.value, 2n)),
        }),
      }),
    },
    {
      label: 'forbidden non-currency unit (dimensionless) masquerading as contract_notional',
      mutate: (r) => ({
        ...r,
        measures: create(FuturesHedgeMeasuresSchema, {
          ...r.measures!,
          contractNotional: createDecimalValue('1000000', UNIT_DIM),
        }),
      }),
    },
    {
      label: 'wrong covered_tax_basis (TAX_ADJUSTED)',
      mutate: (r) => ({
        ...r,
        coveredTaxBasis: BondTaxBasis.TAX_ADJUSTED,
      }),
    },
    {
      label: 'WHOLE_ACCOUNT returned with uncovered_instrument_count > 0',
      mutate: (r) => ({
        ...r,
        scope: PreviewHedgeScope.WHOLE_ACCOUNT,
        uncoveredInstrumentCount: 1,
      }),
    },
    {
      label: 'mismatched scope',
      mutate: (r) => ({
        ...r,
        scope: PreviewHedgeScope.COVERED_SUBSET_ONLY,
      }),
    },
    {
      label: 'mismatched product_code',
      mutate: (r) => ({
        ...r,
        productCode: 'TF',
      }),
    },
    {
      label: 'mismatched contract_code',
      mutate: (r) => ({
        ...r,
        contractCode: 'T2612',
      }),
    },
    {
      label: 'mismatched evaluated_at_seq',
      mutate: (r) => ({
        ...r,
        evaluatedAtSeq: 999n,
      }),
    },
  ])(
    'fails closed and clears previously rendered lots on corrupt response: $label',
    async ({ mutate }) => {
      const user = userEvent.setup();
      const store = createBlotterStore();
      seedStore(store, [makeSnapshot('acct-A', 10n)]);

      let callNum = 0;
      const previewCoveredHedge = vi.fn(async () => {
        callNum += 1;
        const base = makeValidHedgeResult({
          evaluatedAtSeq: 10n,
          scope: PreviewHedgeScope.WHOLE_ACCOUNT,
          productCode: 'T',
          contractCode: 'T2609',
          contractNotional: '1000000',
          recommendedContracts: 7n,
        });
        return callNum === 1 ? base : mutate(base);
      });

      render(
        <HedgePreviewPanel
          store={store}
          client={{ getAccountSnapshot: vi.fn(), previewCoveredHedge }}
          rulePackOptions={makeTrustedRulePackOpts()}
          valuationAtOverride={VALUATION_AT}
        />,
      );

      // First call succeeds and displays 7 lots
      await fillAndSubmitHedgeForm(user, { productCode: 'T', contractCode: 'T2609' });
      await waitFor(() => {
        expect(screen.getByTestId('hedge-recommended-contracts')).toHaveTextContent('7');
      });

      // Second call returns corrupt response -> MUST clear lots and show fail-closed error!
      await user.click(screen.getByTestId('hedge-preview-submit-btn'));
      await waitFor(() => {
        expect(screen.getByTestId('hedge-preview-error')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('hedge-recommended-contracts')).not.toBeInTheDocument();
    },
  );
});
