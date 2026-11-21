import { create, toBinary } from '@bufbuild/protobuf';
import { TimestampSchema } from '@bufbuild/protobuf/wkt';
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createConnection } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { fileURLToPath } from 'node:url';
import {
  BaseEvidenceSchema,
  InstrumentCatalogEntrySchema,
  OpeningInstrumentBalanceSchema,
  OpeningPositionBaseSchema,
  PrincipalRole,
  QuoteTermsSchema,
  RateCoverageEligibility,
  SettlementBucket,
  TradeFeedRecordAction,
  TradeSide,
  type BaseEvidence,
  type InstrumentCatalogEntry,
  type OpeningInstrumentBalance,
  type OpeningPositionBase,
  type QuoteTerms,
} from '../src/generated/ficant/blotter/v1/model_pb';
import {
  MarketQuoteTickSchema,
  TradeFillRecordSchema,
  type MarketQuoteTick,
  type TradeFillRecord,
} from '../src/generated/ficant/blotter/v1/spi_pb';
import {
  MarketTimeSchema,
  Sha256Schema,
  UlidSchema,
  UnitRefSchema,
  VersionRefSchema,
  type DecimalValue,
  type MarketTime,
  type Ulid,
  type UnitRef,
} from '../src/generated/ficant/core/v1/common_pb';
import {
  BondBusinessDayConvention,
  BondCouponFrequency,
  BondDayCountConvention,
  BondSchema,
  BondTaxAttributesSchema,
  CalendarSchema,
  CalendarSessionSchema,
  IncomeTaxStatus,
  ValueAddedTaxStatus,
  type Bond,
  type Calendar,
} from '../src/generated/ficant/market/v1/definition_pb';
import { PriceSourceType } from '../src/generated/ficant/market/v1/fact_pb';
import {
  ConstraintDefinitionSchema,
  ConstraintMeasure,
  RegulatoryRulePackSchema,
} from '../src/generated/ficant/market/v1/regulatory_rule_pb';
import {
  AnalysisUnitsSchema,
  CalendarRequirement,
  type AnalysisUnits,
} from '../src/generated/ficant/rates/v1/analytics_pb';
import { createDecimalValue } from '../src/codec/decimal';
import {
  createDeskBlotterClient,
  InMemoryCredentialStore,
  type DeskBlotterRpcClient,
} from '../src/transport';
import { getFreeLoopbackPort } from './controllable-proxy';

const THIS_DIR = dirname(fileURLToPath(import.meta.url));
const DESK_WEB_ROOT = resolve(THIS_DIR, '..');
const REPO_ROOT = resolve(DESK_WEB_ROOT, '..', '..');

export const E2E_FIXTURES_DIR = join(THIS_DIR, 'fixtures');
export const NLP_TEMPLATES_CSV_PATH = join(E2E_FIXTURES_DIR, 'nlp-templates.csv');
const REGULATORY_PACK_FIXTURE_PATH = join(E2E_FIXTURES_DIR, 'regulatory-rule-pack.json');

export const TRADER_TOKEN = 'bp3-e2e-trader-secret-token-001';
export const OPERATOR_TOKEN = 'bp3-e2e-operator-secret-token-002';
export const FEED_TOKEN = 'bp3-e2e-feed-secret-token-003';
export const PARSER_TOKEN = 'bp3-e2e-parser-secret-token-004';
export const TRADER_A_ONLY_TOKEN = 'bp3-e2e-trader-a-only-token-005';
export const BAD_TOKEN = 'bp3-e2e-invalid-token-999';

export interface RulePackBindingJson {
  readonly rule_pack_id: string;
  readonly version: number;
  readonly market: string;
  readonly rule_type: string;
  readonly source: string;
  readonly effective_from_rfc3339: string;
  readonly effective_to_rfc3339: string;
  readonly verification_status: string;
  readonly type_url: string;
  readonly content_hash_sha256_hex: string;
  readonly content_hex: string;
}

export interface PolicyFileOptions {
  readonly profile: string;
  readonly allowedHedgeProductCodes: readonly string[];
  readonly defaultQuoteSourceId?: string;
  readonly quoteStalenessSeconds?: number;
  readonly defaultDraftHoldTtlSeconds?: number;
  readonly limitCeilings?: Readonly<Record<string, string>>;
  readonly regulatoryRulePack?: RulePackBindingJson | null;
  readonly cleanPriceTolerance?: string;
  readonly yieldTolerance?: string;
}

export interface StartFicantServerOptions {
  readonly tempDir?: string;
  readonly mode: 'ephemeral' | { readonly kind: 'persistent_wal'; readonly dbPath: string };
  readonly policy: 'embedded_default' | PolicyFileOptions;
  readonly accounts: readonly string[];
  readonly sources: readonly string[];
  readonly traderAOnlyAccounts?: readonly string[];
  readonly retentionWindow?: number;
  readonly watchdogIntervalMs?: number;
  readonly nlpCsvPath?: string;
}

export interface FicantServerHandle {
  readonly origin: string;
  readonly port: number;
  readonly tempDir: string;
  readonly ownsTempDir: boolean;
  readonly dbPath: string | null;
  stop(): Promise<void>;
  cleanup(): Promise<void>;
}

export interface AdminBlotterClients {
  readonly trader: DeskBlotterRpcClient;
  readonly operator: DeskBlotterRpcClient;
  readonly feed: DeskBlotterRpcClient;
  readonly parser: DeskBlotterRpcClient;
}

export function createIsolatedTempDir(prefix = 'ficant-e2e-'): string {
  return mkdtempSync(join(tmpdir(), prefix));
}

export function removeIsolatedTempDir(dir: string): void {
  const resolvedTmp = resolve(tmpdir());
  const resolvedTarget = resolve(dir);
  if (!resolvedTarget.toLowerCase().startsWith(resolvedTmp.toLowerCase())) {
    throw new Error(`Refusing to remove directory outside OS temp dir: ${resolvedTarget}`);
  }
  rmSync(resolvedTarget, { recursive: true, force: true });
}

export function resolveFicantServerBinary(): string {
  const exe = process.platform === 'win32' ? 'ficant-server.exe' : 'ficant-server';
  const candidate = join(REPO_ROOT, 'target', 'debug', exe);
  if (!existsSync(candidate)) {
    throw new Error(
      `Missing compiled ficant-server binary at ${candidate}. Run "cargo build --locked -p ficant-server" first.`,
    );
  }
  return candidate;
}

function sha256HexUtf8(text: string): string {
  return createHash('sha256').update(text, 'utf8').digest('hex');
}

function sha256HexBytes(bytes: Uint8Array): string {
  return createHash('sha256').update(bytes).digest('hex');
}

export function buildRegulatoryRulePackBinding(
  constraints: readonly { readonly limitCode: string; readonly measure: ConstraintMeasure }[] = [
    { limitCode: 'MAX_INST_FACE', measure: ConstraintMeasure.INSTRUMENT_FACE },
  ],
): RulePackBindingJson {
  const fixtureRaw = readFileSync(REGULATORY_PACK_FIXTURE_PATH, 'utf8');
  const fixtureMeta = JSON.parse(fixtureRaw) as Omit<
    RulePackBindingJson,
    'content_hash_sha256_hex' | 'content_hex'
  >;

  const msg = create(RegulatoryRulePackSchema, {
    constraints: constraints.map((c) =>
      create(ConstraintDefinitionSchema, {
        limitCode: c.limitCode,
        measure: c.measure,
      }),
    ),
  });
  const encoded = toBinary(RegulatoryRulePackSchema, msg);
  const digestHex = sha256HexBytes(encoded);
  const contentHex = Buffer.from(encoded).toString('hex');

  return {
    ...fixtureMeta,
    content_hash_sha256_hex: digestHex,
    content_hex: contentHex,
  };
}

function writeCredentialsFile(
  path: string,
  accounts: readonly string[],
  sources: readonly string[],
  traderAOnlyAccounts: readonly string[],
): void {
  const doc = {
    schema_version: 1,
    credentials: [
      {
        actor_id: 'trader-1',
        role: 'TRADER',
        token_sha256_hex: sha256HexUtf8(TRADER_TOKEN),
        allowed_accounts: [...accounts],
        allowed_sources: [...sources],
      },
      {
        actor_id: 'operator-1',
        role: 'OPERATOR',
        token_sha256_hex: sha256HexUtf8(OPERATOR_TOKEN),
        allowed_accounts: [...accounts],
        allowed_sources: [...sources],
      },
      {
        actor_id: 'feed-1',
        role: 'FEED_ADAPTER',
        token_sha256_hex: sha256HexUtf8(FEED_TOKEN),
        allowed_accounts: [...accounts],
        allowed_sources: [...sources],
      },
      {
        actor_id: 'parser-1',
        role: 'NLP_PARSER',
        token_sha256_hex: sha256HexUtf8(PARSER_TOKEN),
        allowed_accounts: [...accounts],
        allowed_sources: [],
      },
      {
        actor_id: 'trader-a-only',
        role: 'TRADER',
        token_sha256_hex: sha256HexUtf8(TRADER_A_ONLY_TOKEN),
        allowed_accounts: [...traderAOnlyAccounts],
        allowed_sources: [...sources],
      },
    ],
  };
  writeFileSync(path, JSON.stringify(doc, null, 2), 'utf8');
}

function writePolicyFile(path: string, opts: PolicyFileOptions): void {
  const doc = {
    schema_version: 1,
    policy_version: 1,
    policy_profile: opts.profile,
    allowed_hedge_product_codes: [...opts.allowedHedgeProductCodes],
    blotter_config: {
      clean_price_tolerance: opts.cleanPriceTolerance ?? '0.05',
      yield_tolerance: opts.yieldTolerance ?? '0.0005',
      committed_overdue_seconds: 3600,
      pre_close_alert_time: '16:30:00',
      feed_staleness_seconds: 300,
    },
    default_account_policy: {
      default_quote_source_id: opts.defaultQuoteSourceId ?? 'primary-quote-feed',
      instrument_quote_sources: {},
      quote_staleness_seconds: opts.quoteStalenessSeconds ?? 300,
      default_draft_hold_ttl_seconds: opts.defaultDraftHoldTtlSeconds ?? 1800,
      limit_ceilings: opts.limitCeilings ?? {},
      regulatory_rule_pack: opts.regulatoryRulePack ?? null,
    },
    accounts: {},
  };
  writeFileSync(path, JSON.stringify(doc, null, 2), 'utf8');
}

async function waitForTcpPort(
  port: number,
  proc: ChildProcess,
  getStderr: () => string,
  timeoutMs = 15_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (proc.exitCode !== null) {
      throw new Error(
        `ficant-server exited prematurely (code=${String(proc.exitCode)}): ${getStderr()}`,
      );
    }
    const connected = await new Promise<boolean>((resolveConn) => {
      const sock = createConnection({ host: '127.0.0.1', port });
      sock.once('connect', () => {
        sock.destroy();
        resolveConn(true);
      });
      sock.once('error', () => {
        sock.destroy();
        resolveConn(false);
      });
    });
    if (connected) {
      return;
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`Timed out waiting for ficant-server on 127.0.0.1:${port}. Stderr: ${getStderr()}`);
}

async function stopServerProcess(proc: ChildProcess): Promise<void> {
  if (proc.exitCode !== null) {
    return;
  }
  const exited = new Promise<void>((resolveExit) => {
    proc.once('exit', () => resolveExit());
  });
  if (process.platform === 'win32' && proc.pid) {
    spawnSync('taskkill', ['/PID', String(proc.pid), '/T', '/F'], {
      stdio: 'ignore',
      windowsHide: true,
    });
  } else {
    proc.kill('SIGTERM');
  }
  await Promise.race([exited, new Promise((r) => setTimeout(r, 5000))]);
  if (proc.exitCode === null) {
    proc.kill('SIGKILL');
    await Promise.race([exited, new Promise((r) => setTimeout(r, 2000))]);
  }
  // Brief settle on Windows so OS releases exclusive SQLite WAL handles cleanly.
  await new Promise((r) => setTimeout(r, 150));
}

export async function startFicantServer(
  options: StartFicantServerOptions,
): Promise<FicantServerHandle> {
  const binary = resolveFicantServerBinary();
  const ownsTempDir = options.tempDir === undefined;
  const tempDir = options.tempDir ?? createIsolatedTempDir();
  const port = await getFreeLoopbackPort();
  const listenAddr = `127.0.0.1:${port}`;

  const credsPath = join(tempDir, `creds-${port}.json`);
  writeCredentialsFile(
    credsPath,
    options.accounts,
    options.sources,
    options.traderAOnlyAccounts ?? (options.accounts[0] !== undefined ? [options.accounts[0]] : []),
  );

  let policyPath: string | null = null;
  if (options.policy !== 'embedded_default') {
    policyPath = join(tempDir, `policy-${port}.json`);
    writePolicyFile(policyPath, options.policy);
  }

  const cleanEnv: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (
      !k.startsWith('FICANT_BLOTTER_') &&
      !k.startsWith('FICANT_SPI_') &&
      k !== 'FICANT_SERVER_ADDR'
    ) {
      cleanEnv[k] = v;
    }
  }
  cleanEnv['FICANT_SERVER_ADDR'] = listenAddr;
  cleanEnv['FICANT_BLOTTER_CREDENTIALS_PATH'] = credsPath;

  let dbPath: string | null = null;
  if (options.mode === 'ephemeral') {
    cleanEnv['FICANT_BLOTTER_EPHEMERAL_IN_MEMORY'] = '1';
  } else {
    dbPath = options.mode.dbPath;
    cleanEnv['FICANT_BLOTTER_DB_PATH'] = dbPath;
  }

  if (policyPath !== null) {
    cleanEnv['FICANT_BLOTTER_POLICY_PATH'] = policyPath;
  }
  if (options.retentionWindow !== undefined) {
    cleanEnv['FICANT_BLOTTER_EVENT_RETENTION_WINDOW'] = String(options.retentionWindow);
  }
  if (options.watchdogIntervalMs !== undefined) {
    cleanEnv['FICANT_BLOTTER_WATCHDOG_INTERVAL_MS'] = String(options.watchdogIntervalMs);
  }
  if (options.nlpCsvPath !== undefined) {
    cleanEnv['FICANT_SPI_NLP_CSV'] = options.nlpCsvPath;
  }

  let stderrBuf = '';
  const proc = spawn(binary, [], {
    cwd: tempDir,
    env: cleanEnv,
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
  });
  proc.stderr.on('data', (chunk: Buffer) => {
    stderrBuf += chunk.toString('utf8');
  });

  try {
    await waitForTcpPort(port, proc, () => stderrBuf, 15_000);
  } catch (err) {
    await stopServerProcess(proc);
    if (ownsTempDir) {
      removeIsolatedTempDir(tempDir);
    }
    throw err;
  }

  let stopped = false;
  const stop = async (): Promise<void> => {
    if (stopped) {
      return;
    }
    stopped = true;
    await stopServerProcess(proc);
  };

  return {
    origin: `http://${listenAddr}`,
    port,
    tempDir,
    ownsTempDir,
    dbPath,
    stop,
    async cleanup() {
      await stop();
      if (ownsTempDir) {
        removeIsolatedTempDir(tempDir);
      }
    },
  };
}

export function createAdminClients(origin: string): AdminBlotterClients {
  const makeClient = (token: string, roleHint: PrincipalRole): DeskBlotterRpcClient => {
    const credStore = new InMemoryCredentialStore();
    credStore.setCredential({
      token,
      headerMode: 'authorization',
      roleHint,
    });
    return createDeskBlotterClient({
      credentialStore: credStore,
      baseUrl: origin,
      location: {
        protocol: 'http:',
        hostname: '127.0.0.1',
        port: new URL(origin).port,
        origin,
      },
    });
  };

  return {
    trader: makeClient(TRADER_TOKEN, PrincipalRole.TRADER),
    operator: makeClient(OPERATOR_TOKEN, PrincipalRole.OPERATOR),
    feed: makeClient(FEED_TOKEN, PrincipalRole.FEED_ADAPTER),
    parser: makeClient(PARSER_TOKEN, PrincipalRole.NLP_PARSER),
  };
}

// ============================================================================
// Deterministic Proto Seed Builders (aligned with live Rust/Python contracts)
// ============================================================================

export function makeUlid(suffix: string): Ulid {
  const clean = suffix.toUpperCase().replace(/[^0-9A-HJKMNP-TV-Z]/g, '0');
  const padded = clean.padStart(4, '0').slice(-4);
  return create(UlidSchema, { value: `01ARZ3NDEKTSV4RRFFQ69G${padded}` });
}

export function makeReferenceUnits(): AnalysisUnits {
  const unitRef = (suffix: string): UnitRef =>
    create(UnitRefSchema, { unitId: makeUlid(suffix), version: 1n });
  return create(AnalysisUnitsSchema, {
    currencyAmount: unitRef('5FA1'),
    pricePer100: unitRef('5FA2'),
    rate: unitRef('5FA3'),
    years: unitRef('5FY1'),
    yearsSquared: unitRef('5FY2'),
    dv01: unitRef('5FA4'),
    dv01Per100: unitRef('5FA5'),
    dimensionless: unitRef('5FA6'),
    contractCount: unitRef('5FA7'),
  });
}

export const REFERENCE_UNITS = makeReferenceUnits();

export function makeDec(text: string, unit: UnitRef): DecimalValue {
  return createDecimalValue(text, unit);
}

export function formatLocalDateInTimezone(epochSeconds: number, timeZone: string): string {
  return new Intl.DateTimeFormat('en-CA', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(new Date(epochSeconds * 1000));
}

export function makeMarketTimeFromEpochSecs(
  epochSecs: number,
  marketTimezone = 'Asia/Shanghai',
): MarketTime {
  const localTradingDate = formatLocalDateInTimezone(epochSecs, marketTimezone);
  return create(MarketTimeSchema, {
    instant: create(TimestampSchema, { seconds: BigInt(epochSecs), nanos: 0 }),
    marketTimezone,
    localTradingDate,
  });
}

export function makeShanghaiMarketTime(isoWithOffset: string): MarketTime {
  const ms = Date.parse(isoWithOffset);
  if (Number.isNaN(ms)) {
    throw new Error(`Invalid ISO timestamp: ${isoWithOffset}`);
  }
  return makeMarketTimeFromEpochSecs(Math.floor(ms / 1000), 'Asia/Shanghai');
}

export function makeRecentMarketTime(secondsAgo = 5): MarketTime {
  return makeMarketTimeFromEpochSecs(Math.floor(Date.now() / 1000) - secondsAgo, 'Asia/Shanghai');
}

export function makeStandardBond(suffix = '00B1', couponRate = '0.025'): Bond {
  const units = REFERENCE_UNITS;
  return create(BondSchema, {
    instrument: create(VersionRefSchema, { id: makeUlid(suffix), version: 1n }),
    firstIssueDate: '2025-01-15',
    currentIssueDate: '2025-01-15',
    maturityDate: '2030-01-15',
    faceValue: makeDec('100', units.currencyAmount!),
    cumulativeIssuedAmount: makeDec('50000000000', units.currencyAmount!),
    taxAttributes: create(BondTaxAttributesSchema, {
      valueAddedTaxStatus: ValueAddedTaxStatus.EXEMPT,
      incomeTaxStatus: IncomeTaxStatus.EXEMPT,
    }),
    couponRate: makeDec(couponRate, units.rate!),
    couponFrequency: BondCouponFrequency.SEMIANNUAL,
    dayCount: BondDayCountConvention.ACT_ACT_BOND_ISMA,
    businessDay: BondBusinessDayConvention.FOLLOWING,
  });
}

export function makeStandardCalendar(): Calendar {
  return create(CalendarSchema, {
    calendarId: makeUlid('00C1'),
    version: 1n,
    market: 'INTERBANK',
    marketTimezone: 'Asia/Shanghai',
    effectiveFrom: makeShanghaiMarketTime('2025-01-01T00:00:00+08:00'),
    effectiveTo: makeShanghaiMarketTime('2032-01-01T00:00:00+08:00'),
    sessions: [
      create(CalendarSessionSchema, {
        localDate: '2025-01-01',
        openLocalTime: '09:00:00',
        closeLocalTime: '17:00:00',
        closed: false,
      }),
      create(CalendarSessionSchema, {
        localDate: '2031-12-31',
        openLocalTime: '09:00:00',
        closeLocalTime: '17:00:00',
        closed: false,
      }),
    ],
  });
}

export interface OpeningBaseSpec {
  readonly accountId: string;
  readonly baseSuffix: string;
  readonly sourceId: string;
  readonly tradingDate?: string;
  readonly nextSettlementDate?: string;
  readonly startAfterSeq?: bigint;
  readonly openingCashT0?: string;
  readonly openingNetCashDeltaT1?: string;
  readonly balances?: readonly OpeningInstrumentBalance[];
  readonly instruments?: readonly InstrumentCatalogEntry[];
  readonly baselineEvidence?: readonly BaseEvidence[];
}

export function makeOpeningBase(spec: OpeningBaseSpec): OpeningPositionBase {
  const units = REFERENCE_UNITS;
  const tradingDate = spec.tradingDate ?? '2026-06-15';
  const nextSettlementDate = spec.nextSettlementDate ?? '2026-06-16';
  const startAfterSeq = spec.startAfterSeq ?? 0n;

  const balances: OpeningInstrumentBalance[] = spec.balances
    ? [...spec.balances]
    : [
        create(OpeningInstrumentBalanceSchema, {
          instrumentCode: 'BOND_10Y',
          openingEconomicNotional: makeDec('100000000', units.currencyAmount!),
          openingAvailableNotional: makeDec('100000000', units.currencyAmount!),
          openingNetT0Notional: makeDec('0', units.currencyAmount!),
          openingNetT1Notional: makeDec('0', units.currencyAmount!),
        }),
      ];

  const instruments: InstrumentCatalogEntry[] = spec.instruments
    ? [...spec.instruments]
    : [
        create(InstrumentCatalogEntrySchema, {
          instrumentCode: 'BOND_10Y',
          bond: makeStandardBond('00B1'),
          eligibilityClass: RateCoverageEligibility.SOVEREIGN_OR_POLICY_FIXED,
          calendarRequirement: CalendarRequirement.UNSPECIFIED,
        }),
      ];

  return create(OpeningPositionBaseSchema, {
    accountId: spec.accountId,
    baseId: makeUlid(spec.baseSuffix),
    tradingDate,
    nextSettlementDate,
    effectiveFrom: makeShanghaiMarketTime(`${tradingDate}T09:00:00+08:00`),
    watermarkTime: makeShanghaiMarketTime(`${tradingDate}T09:00:00+08:00`),
    watermarkSourceSeq: startAfterSeq,
    sourceId: spec.sourceId,
    sourceStartAfterFeedSeq: startAfterSeq,
    openingCashT0: makeDec(spec.openingCashT0 ?? '500000000', units.currencyAmount!),
    openingNetCashDeltaT1: makeDec(spec.openingNetCashDeltaT1 ?? '10000000', units.currencyAmount!),
    balances,
    instruments,
    calendar: makeStandardCalendar(),
    defaultCalendarRequirement: CalendarRequirement.REFERENCE_REPLAY,
    units,
    baselineEvidence: spec.baselineEvidence ? [...spec.baselineEvidence] : [],
  });
}

export function makeQuoteTermsClean(cleanPrice: string, settlementAmount?: string): QuoteTerms {
  const units = REFERENCE_UNITS;
  return create(QuoteTermsSchema, {
    priceOrYield: {
      case: 'cleanPricePer100',
      value: makeDec(cleanPrice, units.pricePer100!),
    },
    ...(settlementAmount !== undefined
      ? { settlementAmount: makeDec(settlementAmount, units.currencyAmount!) }
      : {}),
  });
}

export function makeQuoteTick(opts: {
  readonly sourceId: string;
  readonly quoteSeq: bigint;
  readonly instrumentCode: string;
  readonly ytm?: string;
  readonly secondsAgo?: number;
}): MarketQuoteTick {
  const units = REFERENCE_UNITS;
  const ago = opts.secondsAgo ?? 5;
  const obs = makeRecentMarketTime(ago);
  const rec = makeRecentMarketTime(Math.max(1, ago - 1));
  return create(MarketQuoteTickSchema, {
    sourceId: opts.sourceId,
    quoteSeq: opts.quoteSeq,
    instrumentCode: opts.instrumentCode,
    sourceType: PriceSourceType.ACTIVE_QUOTE,
    midYieldToMaturity: makeDec(opts.ytm ?? '0.025', units.rate!),
    observedAt: obs,
    receivedAt: rec,
  });
}

export function makeTradeFill(opts: {
  readonly sourceId: string;
  readonly tradeId: string;
  readonly rev?: bigint;
  readonly feedSeq: bigint;
  readonly accountId: string;
  readonly instrumentCode: string;
  readonly side: TradeSide;
  readonly settlementDate?: string;
  readonly notional: string;
  readonly quoteTerms?: QuoteTerms;
  readonly action?: TradeFeedRecordAction;
  readonly batchFirst?: bigint;
  readonly batchLast?: bigint;
}): TradeFillRecord {
  const units = REFERENCE_UNITS;
  const settlementDate = opts.settlementDate ?? '2026-06-15';
  return create(TradeFillRecordSchema, {
    sourceId: opts.sourceId,
    externalTradeId: opts.tradeId,
    sourceRevision: opts.rev ?? 1n,
    feedSeq: opts.feedSeq,
    action: opts.action ?? TradeFeedRecordAction.NEW_FILL,
    accountId: opts.accountId,
    instrumentCode: opts.instrumentCode,
    side: opts.side,
    settlementDate,
    notional: makeDec(opts.notional, units.currencyAmount!),
    quoteTerms: opts.quoteTerms ?? makeQuoteTermsClean('100', opts.notional),
    executedAt: makeShanghaiMarketTime(`${settlementDate}T10:05:00+08:00`),
    batchFirstFeedSeq: opts.batchFirst ?? opts.feedSeq,
    batchLastFeedSeq: opts.batchLast ?? opts.feedSeq,
  });
}

export function makeBaseEvidence(opts: {
  readonly sourceId: string;
  readonly externalTradeId: string;
  readonly originalFeedSeq: bigint;
  readonly oldNotional: string;
  readonly oldSignedCash: string;
  readonly instrumentCode?: string;
  readonly side?: TradeSide;
  readonly settlementBucket?: SettlementBucket;
}): BaseEvidence {
  const units = REFERENCE_UNITS;
  return create(BaseEvidenceSchema, {
    sourceId: opts.sourceId,
    externalTradeId: opts.externalTradeId,
    originalFeedSeq: opts.originalFeedSeq,
    originalExecutedAt: makeShanghaiMarketTime('2026-06-15T08:30:00+08:00'),
    oldNotional: makeDec(opts.oldNotional, units.currencyAmount!),
    oldSignedCash: makeDec(opts.oldSignedCash, units.currencyAmount!),
    settlementBucket: opts.settlementBucket ?? SettlementBucket.T_PLUS_0,
    evidenceRef: 'eod-export-001',
    evidenceHash: create(Sha256Schema, {
      value: new Uint8Array(createHash('sha256').update(opts.externalTradeId, 'utf8').digest()),
    }),
    instrumentCode: opts.instrumentCode ?? 'BOND_10Y',
    side: opts.side ?? TradeSide.SELL,
    originalRevision: 1n,
  });
}

// ============================================================================
// Exclusive WAL SQLite HOLD Expiry Rollback via Node built-in `node:sqlite`
// ============================================================================

interface StoredMarketTimeDto {
  instant_secs: number;
  instant_nanos: number;
  market_timezone: string;
  local_trading_date: string;
}

interface StoredEarmarkRecordDto {
  earmark_id: string;
  state: string;
  expires_at: StoredMarketTimeDto | null;
  [key: string]: unknown;
}

export interface RollbackHoldResult {
  readonly earmarkId: string;
  readonly updatedInstantSecs: number;
  readonly updatedLocalTradingDate: string;
  readonly marketTimezone: string;
}

/**
 * Directly rolls back `blotter_earmarks.record_json.expires_at.instant_secs` for the specified
 * HOLD earmark IDs inside a single SQLite transaction while keeping `local_trading_date` strictly
 * synchronized with `market_timezone` (required by `MarketTime::new` on WAL restart).
 */
export function rollbackHoldExpiryInSqlite(
  dbPath: string,
  earmarkIds: readonly string[],
  secondsInPast = 120,
): readonly RollbackHoldResult[] {
  if (earmarkIds.length === 0) {
    throw new Error('rollbackHoldExpiryInSqlite requires at least one earmarkId.');
  }

  const db = new DatabaseSync(dbPath);
  const results: RollbackHoldResult[] = [];
  const targetSecs = Math.floor(Date.now() / 1000) - secondsInPast;

  try {
    db.exec('BEGIN IMMEDIATE');
    const selectStmt = db.prepare(
      'SELECT record_json FROM blotter_earmarks WHERE earmark_id = ?',
    );
    const updateStmt = db.prepare(
      'UPDATE blotter_earmarks SET record_json = ? WHERE earmark_id = ?',
    );

    for (const earmarkId of earmarkIds) {
      const row = selectStmt.get(earmarkId) as { record_json?: string } | undefined;
      if (!row || typeof row.record_json !== 'string') {
        throw new Error(`Earmark row not found in SQLite for earmark_id=${earmarkId}`);
      }

      const parsed = JSON.parse(row.record_json) as StoredEarmarkRecordDto;
      if (parsed.state !== 'HOLD' || !parsed.expires_at) {
        throw new Error(
          `Expected HOLD earmark with expires_at for ${earmarkId}, got state=${String(parsed.state)}`,
        );
      }

      const tz = parsed.expires_at.market_timezone;
      const syncedLocalDate = formatLocalDateInTimezone(targetSecs, tz);
      parsed.expires_at.instant_secs = targetSecs;
      parsed.expires_at.instant_nanos = 0;
      parsed.expires_at.local_trading_date = syncedLocalDate;

      const updatedJson = JSON.stringify(parsed);
      const runInfo = updateStmt.run(updatedJson, earmarkId);
      if (Number(runInfo.changes) !== 1) {
        throw new Error(
          `Expected exactly 1 updated row for earmark_id=${earmarkId}, got ${String(runInfo.changes)}`,
        );
      }

      // Read back and verify persisted JSON integrity before committing.
      const verifyRow = selectStmt.get(earmarkId) as { record_json?: string } | undefined;
      if (!verifyRow || typeof verifyRow.record_json !== 'string') {
        throw new Error(`Read-back verification failed for earmark_id=${earmarkId}`);
      }
      const verified = JSON.parse(verifyRow.record_json) as StoredEarmarkRecordDto;
      if (
        !verified.expires_at ||
        verified.expires_at.instant_secs !== targetSecs ||
        verified.expires_at.local_trading_date !== syncedLocalDate
      ) {
        throw new Error(`Read-back mismatch for earmark_id=${earmarkId}`);
      }

      results.push({
        earmarkId,
        updatedInstantSecs: targetSecs,
        updatedLocalTradingDate: syncedLocalDate,
        marketTimezone: tz,
      });
    }

    db.exec('COMMIT');
    db.exec('PRAGMA wal_checkpoint(TRUNCATE)');
  } catch (err) {
    try {
      db.exec('ROLLBACK');
    } catch {
      // Ignore rollback errors if transaction wasn't active.
    }
    throw err;
  } finally {
    db.close();
  }

  return results;
}
