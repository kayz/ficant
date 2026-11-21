import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig, type Plugin, type ProxyOptions } from 'vite';

const DESK_WEB_ROOT = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(DESK_WEB_ROOT, '..', '..');

export const PINNED_RULEPACK_MANIFEST_SHA256 =
  'f3782535788e646190016ec9a60f4f588be3722b34f3f9778ae8ee87a2cc238e';
export const PINNED_RULEPACK_PAYLOAD_SHA256 =
  '8d8a0b944557cf87ed3d1f598ab3e02fa7f0aec125c1c26c17280acbd24367c5';
export const PINNED_RULEPACK_PAYLOAD_BYTES = 159;
export const PINNED_RULEPACK_TYPE_URL =
  'type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack';
export const PINNED_RULEPACK_ASSET_PATH = '/rulepacks/cgb-futures-v2.bin';
export const VIRTUAL_TRUSTED_RULEPACK_ID = 'virtual:ficant-trusted-rulepack';
const RESOLVED_VIRTUAL_TRUSTED_RULEPACK_ID = `\0${VIRTUAL_TRUSTED_RULEPACK_ID}`;

const ULID_PATTERN = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
const SHA256_HEX_PATTERN = /^[0-9a-f]{64}$/;
const ISO_DATE_PATTERN = /^\d{4}-\d{2}-\d{2}$/;

export interface TrustedMarketTimeRecord {
  readonly instant: string;
  readonly market_timezone: string;
  readonly local_trading_date: string;
}

export interface TrustedRulePackManifest {
  readonly schema: 'ficant.desk-web.rulepack-manifest.v1';
  readonly approval_ref: string;
  readonly rule_pack_id: string;
  readonly version: number;
  readonly owner: {
    readonly tenant_id: string;
    readonly owner_id: string;
  };
  readonly market: string;
  readonly rule_type: string;
  readonly source: string;
  readonly verification_status: 'VERIFIED';
  readonly effective_from: TrustedMarketTimeRecord;
  readonly effective_to: TrustedMarketTimeRecord;
  readonly type_url: string;
  readonly payload_asset_path: string;
  readonly payload_byte_length: number;
  readonly payload_sha256: string;
}

export interface TrustedRulePackBuildAnchor {
  readonly manifestSha256: string;
  readonly payloadSha256: string;
  readonly payloadByteLength: number;
  readonly payloadAssetPath: string;
  readonly manifest: TrustedRulePackManifest;
}

export interface VerifyRulePackOptions {
  readonly manifestPath?: string;
  readonly payloadPath?: string;
  readonly goldenInputsPath?: string;
  readonly expectedManifestSha256?: string;
  readonly expectedPayloadSha256?: string;
  readonly now?: Date;
}

function sha256Hex(buffer: Uint8Array): string {
  return createHash('sha256').update(buffer).digest('hex');
}

function assertNonEmptyTrimmed(value: unknown, field: string): string {
  if (typeof value !== 'string' || value.trim().length === 0 || value !== value.trim()) {
    throw new Error(`Trusted RulePack manifest field "${field}" must be a non-empty trimmed string.`);
  }
  return value;
}

function assertUlid(value: unknown, field: string): string {
  const str = assertNonEmptyTrimmed(value, field);
  if (!ULID_PATTERN.test(str)) {
    throw new Error(`Trusted RulePack manifest field "${field}" is not a canonical ULID: ${str}`);
  }
  return str;
}

function parseAndValidateMarketTime(raw: unknown, field: string): { record: TrustedMarketTimeRecord; epochMs: number } {
  if (typeof raw !== 'object' || raw === null) {
    throw new Error(`Trusted RulePack manifest field "${field}" must be an object.`);
  }
  const obj = raw as Record<string, unknown>;
  const instant = assertNonEmptyTrimmed(obj['instant'], `${field}.instant`);
  const marketTimezone = assertNonEmptyTrimmed(obj['market_timezone'], `${field}.market_timezone`);
  const localTradingDate = assertNonEmptyTrimmed(obj['local_trading_date'], `${field}.local_trading_date`);

  if (!ISO_DATE_PATTERN.test(localTradingDate)) {
    throw new Error(`Trusted RulePack manifest "${field}.local_trading_date" must be YYYY-MM-DD.`);
  }

  const parsedDate = new Date(instant);
  const epochMs = parsedDate.getTime();
  if (Number.isNaN(epochMs)) {
    throw new Error(`Trusted RulePack manifest "${field}.instant" is not a valid ISO-8601 timestamp.`);
  }

  const formattedLocalDate = new Intl.DateTimeFormat('en-CA', {
    timeZone: marketTimezone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(parsedDate);

  if (formattedLocalDate !== localTradingDate) {
    throw new Error(
      `Trusted RulePack manifest "${field}" local_trading_date (${localTradingDate}) does not match instant (${instant}) in ${marketTimezone} (${formattedLocalDate}).`,
    );
  }

  return {
    record: {
      instant,
      market_timezone: marketTimezone,
      local_trading_date: localTradingDate,
    },
    epochMs,
  };
}

export function verifyTrustedRulePackEnvelope(options: VerifyRulePackOptions = {}): {
  anchor: TrustedRulePackBuildAnchor;
  payloadBytes: Buffer;
} {
  const manifestPath =
    options.manifestPath ??
    resolve(DESK_WEB_ROOT, 'src', 'assets', 'rulepacks', 'cgb-futures-v2.manifest.json');
  const payloadPath =
    options.payloadPath ??
    resolve(REPO_ROOT, 'domain-packs', 'cgb-futures', 'cgb-futures-v2.bin');
  const goldenInputsPath =
    options.goldenInputsPath ??
    resolve(REPO_ROOT, 'tests', 'golden-cases', 'china-rates', 'bp1-futures-hedge-v2-inputs.json');
  const expectedManifestSha256 = options.expectedManifestSha256 ?? PINNED_RULEPACK_MANIFEST_SHA256;
  const expectedPayloadSha256 = options.expectedPayloadSha256 ?? PINNED_RULEPACK_PAYLOAD_SHA256;
  const nowMs = (options.now ?? new Date()).getTime();

  if (!SHA256_HEX_PATTERN.test(expectedManifestSha256)) {
    throw new Error('Missing or invalid pinned manifest SHA-256 trust anchor.');
  }
  if (!SHA256_HEX_PATTERN.test(expectedPayloadSha256)) {
    throw new Error('Missing or invalid pinned payload SHA-256 trust anchor.');
  }

  const manifestBytes = readFileSync(manifestPath);
  if (
    manifestBytes.length >= 3 &&
    manifestBytes[0] === 0xef &&
    manifestBytes[1] === 0xbb &&
    manifestBytes[2] === 0xbf
  ) {
    throw new Error('Trusted RulePack manifest must be raw UTF-8 without BOM.');
  }

  const actualManifestSha256 = sha256Hex(manifestBytes);
  if (actualManifestSha256 !== expectedManifestSha256) {
    throw new Error(
      `Trusted RulePack manifest SHA-256 mismatch: expected ${expectedManifestSha256}, got ${actualManifestSha256}.`,
    );
  }

  const parsed: unknown = JSON.parse(manifestBytes.toString('utf8'));
  if (typeof parsed !== 'object' || parsed === null) {
    throw new Error('Trusted RulePack manifest must be a JSON object.');
  }
  const obj = parsed as Record<string, unknown>;

  if (obj['schema'] !== 'ficant.desk-web.rulepack-manifest.v1') {
    throw new Error(`Unsupported RulePack manifest schema: ${String(obj['schema'])}`);
  }
  const approvalRef = assertNonEmptyTrimmed(obj['approval_ref'], 'approval_ref');
  const rulePackId = assertUlid(obj['rule_pack_id'], 'rule_pack_id');
  if (obj['version'] !== 2) {
    throw new Error(`Trusted RulePack manifest version must be 2, got ${String(obj['version'])}.`);
  }

  const ownerRaw = obj['owner'];
  if (typeof ownerRaw !== 'object' || ownerRaw === null) {
    throw new Error('Trusted RulePack manifest "owner" must be an object.');
  }
  const ownerObj = ownerRaw as Record<string, unknown>;
  const tenantId = assertUlid(ownerObj['tenant_id'], 'owner.tenant_id');
  const ownerId = assertUlid(ownerObj['owner_id'], 'owner.owner_id');

  const market = assertNonEmptyTrimmed(obj['market'], 'market');
  if (market !== 'CFFEX') {
    throw new Error(`Trusted RulePack manifest market must be "CFFEX", got "${market}".`);
  }
  const ruleType = assertNonEmptyTrimmed(obj['rule_type'], 'rule_type');
  if (ruleType !== 'cgb-futures') {
    throw new Error(`Trusted RulePack manifest rule_type must be "cgb-futures", got "${ruleType}".`);
  }
  const source = assertNonEmptyTrimmed(obj['source'], 'source');
  if (obj['verification_status'] !== 'VERIFIED') {
    throw new Error(
      `Trusted RulePack manifest verification_status must be "VERIFIED", got ${String(obj['verification_status'])}.`,
    );
  }
  const typeUrl = assertNonEmptyTrimmed(obj['type_url'], 'type_url');
  if (typeUrl !== PINNED_RULEPACK_TYPE_URL) {
    throw new Error(`Trusted RulePack manifest type_url mismatch: got "${typeUrl}".`);
  }
  const payloadAssetPath = assertNonEmptyTrimmed(obj['payload_asset_path'], 'payload_asset_path');
  if (payloadAssetPath !== PINNED_RULEPACK_ASSET_PATH) {
    throw new Error(`Trusted RulePack manifest payload_asset_path mismatch: got "${payloadAssetPath}".`);
  }
  if (obj['payload_byte_length'] !== PINNED_RULEPACK_PAYLOAD_BYTES) {
    throw new Error(
      `Trusted RulePack manifest payload_byte_length must be ${PINNED_RULEPACK_PAYLOAD_BYTES}, got ${String(obj['payload_byte_length'])}.`,
    );
  }
  const manifestPayloadSha256 = assertNonEmptyTrimmed(obj['payload_sha256'], 'payload_sha256');
  if (manifestPayloadSha256 !== expectedPayloadSha256) {
    throw new Error(
      `Trusted RulePack manifest payload_sha256 (${manifestPayloadSha256}) does not match pinned golden digest (${expectedPayloadSha256}).`,
    );
  }

  const effectiveFrom = parseAndValidateMarketTime(obj['effective_from'], 'effective_from');
  const effectiveTo = parseAndValidateMarketTime(obj['effective_to'], 'effective_to');
  if (effectiveFrom.epochMs >= effectiveTo.epochMs) {
    throw new Error('Trusted RulePack manifest effective_from must be strictly before effective_to.');
  }
  if (nowMs < effectiveFrom.epochMs || nowMs >= effectiveTo.epochMs) {
    throw new Error(
      `Trusted RulePack manifest is outside its half-open valid window [${effectiveFrom.record.instant}, ${effectiveTo.record.instant}).`,
    );
  }

  const payloadBytes = readFileSync(payloadPath);
  if (payloadBytes.length !== PINNED_RULEPACK_PAYLOAD_BYTES) {
    throw new Error(
      `RulePack binary length mismatch: expected ${PINNED_RULEPACK_PAYLOAD_BYTES} bytes, got ${payloadBytes.length}.`,
    );
  }
  const actualPayloadSha256 = sha256Hex(payloadBytes);
  if (actualPayloadSha256 !== expectedPayloadSha256) {
    throw new Error(
      `RulePack binary SHA-256 mismatch: expected ${expectedPayloadSha256}, got ${actualPayloadSha256}.`,
    );
  }

  const goldenJson: unknown = JSON.parse(readFileSync(goldenInputsPath, 'utf8'));
  const goldenSha =
    typeof goldenJson === 'object' &&
    goldenJson !== null &&
    typeof (goldenJson as Record<string, unknown>)['rule_pack'] === 'object' &&
    (goldenJson as Record<string, unknown>)['rule_pack'] !== null
      ? ((goldenJson as Record<string, Record<string, unknown>>)['rule_pack']?.['bin_sha256'])
      : undefined;
  if (goldenSha !== expectedPayloadSha256) {
    throw new Error(
      `Golden inputs bin_sha256 (${String(goldenSha)}) does not match pinned payload digest (${expectedPayloadSha256}).`,
    );
  }

  const manifest: TrustedRulePackManifest = {
    schema: 'ficant.desk-web.rulepack-manifest.v1',
    approval_ref: approvalRef,
    rule_pack_id: rulePackId,
    version: 2,
    owner: {
      tenant_id: tenantId,
      owner_id: ownerId,
    },
    market,
    rule_type: ruleType,
    source,
    verification_status: 'VERIFIED',
    effective_from: effectiveFrom.record,
    effective_to: effectiveTo.record,
    type_url: typeUrl,
    payload_asset_path: payloadAssetPath,
    payload_byte_length: PINNED_RULEPACK_PAYLOAD_BYTES,
    payload_sha256: manifestPayloadSha256,
  };

  return {
    anchor: {
      manifestSha256: actualManifestSha256,
      payloadSha256: actualPayloadSha256,
      payloadByteLength: payloadBytes.length,
      payloadAssetPath,
      manifest,
    },
    payloadBytes,
  };
}

export function createTrustedRulePackPlugin(): Plugin {
  let verified: { anchor: TrustedRulePackBuildAnchor; payloadBytes: Buffer } | null = null;

  const ensureVerified = (): { anchor: TrustedRulePackBuildAnchor; payloadBytes: Buffer } => {
    if (verified === null) {
      verified = verifyTrustedRulePackEnvelope();
    }
    return verified;
  };

  return {
    name: 'ficant-trusted-rulepack',
    config() {
      const { anchor } = ensureVerified();
      return {
        define: {
          __FICANT_TRUSTED_RULEPACK__: JSON.stringify(anchor),
        },
      };
    },
    buildStart() {
      verified = verifyTrustedRulePackEnvelope();
    },
    resolveId(id) {
      if (id === VIRTUAL_TRUSTED_RULEPACK_ID) {
        return RESOLVED_VIRTUAL_TRUSTED_RULEPACK_ID;
      }
      return null;
    },
    load(id) {
      if (id === RESOLVED_VIRTUAL_TRUSTED_RULEPACK_ID) {
        const { anchor } = ensureVerified();
        const serialized = JSON.stringify(anchor);
        return `export const TRUSTED_RULEPACK_ANCHOR = Object.freeze(${serialized});\nexport default TRUSTED_RULEPACK_ANCHOR;\n`;
      }
      return null;
    },
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        if (req.method === 'GET' && req.url === PINNED_RULEPACK_ASSET_PATH) {
          const { payloadBytes } = ensureVerified();
          res.statusCode = 200;
          res.setHeader('Content-Type', 'application/octet-stream');
          res.setHeader('Content-Length', String(payloadBytes.length));
          res.setHeader('Cache-Control', 'no-store');
          res.end(payloadBytes);
          return;
        }
        next();
      });
    },
    generateBundle() {
      const { payloadBytes } = ensureVerified();
      this.emitFile({
        type: 'asset',
        fileName: 'rulepacks/cgb-futures-v2.bin',
        source: payloadBytes,
      });
    },
  };
}

function createFicantRpcProxy(upstreamOrigin: string): Record<string, ProxyOptions> {
  return {
    '^/ficant\\.': {
      target: upstreamOrigin,
      changeOrigin: false,
      ws: false,
      configure(proxy) {
        proxy.on('proxyReq', (proxyReq, req) => {
          proxyReq.setHeader('accept-encoding', 'identity');
          const abortUpstream = (): void => {
            if (!proxyReq.destroyed) {
              proxyReq.destroy();
            }
          };
          req.on('aborted', abortUpstream);
          req.on('close', () => {
            if (!req.complete) {
              abortUpstream();
            }
          });
        });
        proxy.on('proxyRes', (proxyRes, _req, res) => {
          res.setHeader('X-Accel-Buffering', 'no');
          if (res.socket) {
            res.socket.setNoDelay(true);
          }
          res.on('close', () => {
            if (!res.writableEnded && !proxyRes.destroyed) {
              proxyRes.destroy();
            }
          });
        });
      },
    },
  };
}

export default defineConfig(() => {
  const upstreamOrigin = process.env['FICANT_UPSTREAM_ORIGIN'] || 'http://127.0.0.1:50051';
  const proxy = createFicantRpcProxy(upstreamOrigin);

  return {
    plugins: [react(), createTrustedRulePackPlugin()],
    server: {
      host: '127.0.0.1',
      proxy,
    },
    preview: {
      host: '127.0.0.1',
      proxy,
    },
    build: {
      outDir: 'dist',
      emptyOutDir: true,
    },
  };
});
