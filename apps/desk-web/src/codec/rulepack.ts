import { create } from '@bufbuild/protobuf';
import { AnySchema } from '@bufbuild/protobuf/wkt';
import {
  TRUSTED_RULEPACK_ANCHOR,
  type TrustedMarketTimeRecord,
  type TrustedRulePackBuildAnchor,
  type TrustedRulePackManifest,
} from 'virtual:ficant-trusted-rulepack';
import {
  OwnerRefSchema,
  Sha256Schema,
  UlidSchema,
  type MarketTime,
} from '../generated/ficant/core/v1/common_pb';
import {
  MarketRulePackSchema,
  VerificationStatus,
  type MarketRulePack,
} from '../generated/ficant/market/v1/rule_pb';
import { hexToBytes, sha256Hex, type SubtleCryptoLike } from './hash';
import {
  SHA256_HEX_PATTERN,
  assertNonEmptyTrimmed,
  createMarketTimeFromIso,
  isValidUlid,
  validateIsoDate,
  validateMarketTime,
} from './primitives';

/**
 * Independent G-1 pinned trust anchors (kept strictly separate from any runtime manifest payload).
 */
export const PINNED_RULEPACK_MANIFEST_SHA256 =
  'f3782535788e646190016ec9a60f4f588be3722b34f3f9778ae8ee87a2cc238e';
export const PINNED_RULEPACK_PAYLOAD_SHA256 =
  '8d8a0b944557cf87ed3d1f598ab3e02fa7f0aec125c1c26c17280acbd24367c5';
export const PINNED_RULEPACK_PAYLOAD_BYTES = 159;
export const PINNED_RULEPACK_TYPE_URL =
  'type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack';
export const PINNED_RULEPACK_ASSET_PATH = '/rulepacks/cgb-futures-v2.bin';

export type RulePackVerificationErrorCode =
  | 'MISSING_TRUST_ANCHOR'
  | 'MANIFEST_DIGEST_MISMATCH'
  | 'MANIFEST_INVALID'
  | 'RULEPACK_EXPIRED_OR_NOT_YET_EFFECTIVE'
  | 'PAYLOAD_FETCH_FAILED'
  | 'PAYLOAD_DIGEST_MISMATCH';

export class RulePackVerificationError extends Error {
  readonly code: RulePackVerificationErrorCode;

  constructor(code: RulePackVerificationErrorCode, message: string) {
    super(message);
    this.name = 'RulePackVerificationError';
    this.code = code;
  }
}

export interface LoadTrustedRulePackOptions {
  /** Valuation instant checked against `[effective_from, effective_to)`. Defaults to `new Date()`. */
  readonly valuationAt?: Date | MarketTime;
  /** Override build anchor (for testing tampered anchors/manifests). */
  readonly anchor?: TrustedRulePackBuildAnchor | null;
  /** Optional raw manifest UTF-8 bytes to verify directly against `expectedManifestSha256`. */
  readonly rawManifestBytes?: Uint8Array;
  /** Override payload bytes (skips fetch when explicitly provided in unit tests). */
  readonly payloadBytes?: Uint8Array;
  /** Override fetch implementation used to load `/rulepacks/cgb-futures-v2.bin`. */
  readonly fetchImpl?: typeof fetch;
  /** Override independent manifest SHA-256 trust anchor (testing missing/alternate anchor). */
  readonly expectedManifestSha256?: string;
  /** Override independent payload SHA-256 trust anchor (testing missing/alternate anchor). */
  readonly expectedPayloadSha256?: string;
  /** Optional SubtleCrypto override (e.g., `null` to test pure-TS SHA-256 fallback). */
  readonly subtleOverride?: SubtleCryptoLike | null;
}

export interface VerifiedRulePackEnvelope {
  readonly rulePack: MarketRulePack;
  readonly manifestSha256: string;
  readonly payloadSha256: string;
  readonly payloadByteLength: number;
  readonly manifest: TrustedRulePackManifest;
}

function parseManifestMarketTime(
  record: TrustedMarketTimeRecord | undefined,
  field: string,
): { marketTime: MarketTime; epochMs: number } {
  if (!record || typeof record !== 'object') {
    throw new RulePackVerificationError('MANIFEST_INVALID', `Missing ${field} MarketTime.`);
  }
  try {
    const instant = assertNonEmptyTrimmed(record.instant, `${field}.instant`);
    const tz = assertNonEmptyTrimmed(record.market_timezone, `${field}.market_timezone`);
    const localDate = validateIsoDate(record.local_trading_date, `${field}.local_trading_date`);
    const marketTime = createMarketTimeFromIso(instant, tz, localDate);
    const epochMs = new Date(instant).getTime();
    return { marketTime, epochMs };
  } catch (err) {
    const msg = err instanceof Error ? err.message : 'invalid MarketTime';
    throw new RulePackVerificationError('MANIFEST_INVALID', `${field}: ${msg}`);
  }
}

function resolveValuationEpochMs(valuationAt: Date | MarketTime | undefined): number {
  if (valuationAt === undefined) {
    return Date.now();
  }
  if (valuationAt instanceof Date) {
    const ms = valuationAt.getTime();
    if (Number.isNaN(ms)) {
      throw new RulePackVerificationError(
        'RULEPACK_EXPIRED_OR_NOT_YET_EFFECTIVE',
        'valuationAt Date is invalid.',
      );
    }
    return ms;
  }
  return validateMarketTime(valuationAt, 'valuation_at').epochMs;
}

function canonicalizeManifestBytes(manifest: TrustedRulePackManifest): Uint8Array {
  const ordered = {
    schema: manifest.schema,
    approval_ref: manifest.approval_ref,
    rule_pack_id: manifest.rule_pack_id,
    version: manifest.version,
    owner: {
      tenant_id: manifest.owner?.tenant_id,
      owner_id: manifest.owner?.owner_id,
    },
    market: manifest.market,
    rule_type: manifest.rule_type,
    source: manifest.source,
    verification_status: manifest.verification_status,
    effective_from: {
      instant: manifest.effective_from?.instant,
      market_timezone: manifest.effective_from?.market_timezone,
      local_trading_date: manifest.effective_from?.local_trading_date,
    },
    effective_to: {
      instant: manifest.effective_to?.instant,
      market_timezone: manifest.effective_to?.market_timezone,
      local_trading_date: manifest.effective_to?.local_trading_date,
    },
    type_url: manifest.type_url,
    payload_asset_path: manifest.payload_asset_path,
    payload_byte_length: manifest.payload_byte_length,
    payload_sha256: manifest.payload_sha256,
  };
  return new TextEncoder().encode(`${JSON.stringify(ordered, null, 2)}\n`);
}

function assertNoUtf8Bom(bytes: Uint8Array): void {
  if (bytes.byteLength >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
    throw new RulePackVerificationError(
      'MANIFEST_INVALID',
      'Trusted RulePack manifest must be raw UTF-8 without BOM.',
    );
  }
}

/**
 * Verifies the G-1 trusted delivery RulePack envelope, independent manifest & golden payload
 * SHA-256 digests, half-open validity window `[effective_from, effective_to)`, and constructs
 * the protobuf `MarketRulePack` ready for `PreviewCoveredHedge`.
 *
 * Fails closed BEFORE any RPC is ever invoked.
 */
export async function loadTrustedDeliveryRulePack(
  options: LoadTrustedRulePackOptions = {},
): Promise<VerifiedRulePackEnvelope> {
  const expectedManifestSha =
    options.expectedManifestSha256 !== undefined
      ? options.expectedManifestSha256
      : PINNED_RULEPACK_MANIFEST_SHA256;
  const expectedPayloadSha =
    options.expectedPayloadSha256 !== undefined
      ? options.expectedPayloadSha256
      : PINNED_RULEPACK_PAYLOAD_SHA256;

  if (!SHA256_HEX_PATTERN.test(expectedManifestSha)) {
    throw new RulePackVerificationError(
      'MISSING_TRUST_ANCHOR',
      'Missing or malformed independent manifest SHA-256 trust anchor.',
    );
  }
  if (!SHA256_HEX_PATTERN.test(expectedPayloadSha)) {
    throw new RulePackVerificationError(
      'MISSING_TRUST_ANCHOR',
      'Missing or malformed independent payload SHA-256 trust anchor.',
    );
  }

  const anchor = options.anchor !== undefined ? options.anchor : TRUSTED_RULEPACK_ANCHOR;
  if (!anchor || typeof anchor !== 'object' || !anchor.manifest) {
    throw new RulePackVerificationError(
      'MISSING_TRUST_ANCHOR',
      'Trusted RulePack build anchor is missing.',
    );
  }

  // Verify independent manifest digest against raw bytes (when supplied) AND canonicalized manifest.
  if (options.rawManifestBytes !== undefined) {
    assertNoUtf8Bom(options.rawManifestBytes);
    const rawDigest = await sha256Hex(options.rawManifestBytes, options.subtleOverride);
    if (rawDigest !== expectedManifestSha) {
      throw new RulePackVerificationError(
        'MANIFEST_DIGEST_MISMATCH',
        `Raw manifest SHA-256 mismatch: expected ${expectedManifestSha}, got ${rawDigest}.`,
      );
    }
  }

  if (anchor.manifestSha256 !== expectedManifestSha) {
    throw new RulePackVerificationError(
      'MANIFEST_DIGEST_MISMATCH',
      `Anchor manifestSha256 mismatch: expected ${expectedManifestSha}, got ${anchor.manifestSha256}.`,
    );
  }

  const manifest = anchor.manifest;
  const canonicalBytes = canonicalizeManifestBytes(manifest);
  const canonicalManifestDigest = await sha256Hex(canonicalBytes, options.subtleOverride);
  if (canonicalManifestDigest !== expectedManifestSha) {
    throw new RulePackVerificationError(
      'MANIFEST_DIGEST_MISMATCH',
      `Manifest contents SHA-256 mismatch: expected ${expectedManifestSha}, got ${canonicalManifestDigest}.`,
    );
  }

  // Structural & metadata contract checks (defense-in-depth beyond digest pinning).
  if (manifest.schema !== 'ficant.desk-web.rulepack-manifest.v1') {
    throw new RulePackVerificationError(
      'MANIFEST_INVALID',
      `Unsupported RulePack manifest schema: ${String(manifest.schema)}.`,
    );
  }
  if (!isValidUlid(manifest.rule_pack_id)) {
    throw new RulePackVerificationError('MANIFEST_INVALID', 'Invalid rule_pack_id ULID.');
  }
  if (manifest.version !== 2) {
    throw new RulePackVerificationError(
      'MANIFEST_INVALID',
      `Unexpected RulePack version: ${String(manifest.version)}.`,
    );
  }
  if (!manifest.owner || !isValidUlid(manifest.owner.tenant_id) || !isValidUlid(manifest.owner.owner_id)) {
    throw new RulePackVerificationError('MANIFEST_INVALID', 'Invalid owner ULIDs in manifest.');
  }
  if (manifest.verification_status !== 'VERIFIED') {
    throw new RulePackVerificationError(
      'MANIFEST_INVALID',
      `RulePack verification_status must be VERIFIED, got ${String(manifest.verification_status)}.`,
    );
  }
  if (manifest.type_url !== PINNED_RULEPACK_TYPE_URL) {
    throw new RulePackVerificationError(
      'MANIFEST_INVALID',
      `RulePack type_url mismatch: got "${manifest.type_url}".`,
    );
  }
  if (
    manifest.payload_asset_path !== PINNED_RULEPACK_ASSET_PATH ||
    anchor.payloadAssetPath !== PINNED_RULEPACK_ASSET_PATH
  ) {
    throw new RulePackVerificationError(
      'MANIFEST_INVALID',
      `RulePack payload_asset_path mismatch: got "${manifest.payload_asset_path}".`,
    );
  }
  if (
    manifest.payload_byte_length !== PINNED_RULEPACK_PAYLOAD_BYTES ||
    anchor.payloadByteLength !== PINNED_RULEPACK_PAYLOAD_BYTES
  ) {
    throw new RulePackVerificationError(
      'MANIFEST_INVALID',
      `RulePack payload_byte_length must be ${PINNED_RULEPACK_PAYLOAD_BYTES}.`,
    );
  }
  if (
    manifest.payload_sha256 !== expectedPayloadSha ||
    anchor.payloadSha256 !== expectedPayloadSha
  ) {
    throw new RulePackVerificationError(
      'PAYLOAD_DIGEST_MISMATCH',
      `Manifest payload_sha256 does not match pinned golden digest ${expectedPayloadSha}.`,
    );
  }

  // Half-open validity window check `[effective_from, effective_to)` at `valuationAt`.
  const effectiveFrom = parseManifestMarketTime(manifest.effective_from, 'effective_from');
  const effectiveTo = parseManifestMarketTime(manifest.effective_to, 'effective_to');
  if (effectiveFrom.epochMs >= effectiveTo.epochMs) {
    throw new RulePackVerificationError(
      'MANIFEST_INVALID',
      'effective_from must be strictly earlier than effective_to.',
    );
  }
  const valuationMs = resolveValuationEpochMs(options.valuationAt);
  if (valuationMs < effectiveFrom.epochMs || valuationMs >= effectiveTo.epochMs) {
    throw new RulePackVerificationError(
      'RULEPACK_EXPIRED_OR_NOT_YET_EFFECTIVE',
      `Valuation instant is outside half-open RulePack window [${manifest.effective_from.instant}, ${manifest.effective_to.instant}).`,
    );
  }

  // Fetch and verify actual binary payload bytes against pinned golden SHA-256.
  let payloadBytes: Uint8Array;
  if (options.payloadBytes !== undefined) {
    payloadBytes = options.payloadBytes;
  } else {
    const fetchFn = options.fetchImpl ?? globalThis.fetch;
    if (typeof fetchFn !== 'function') {
      throw new RulePackVerificationError(
        'PAYLOAD_FETCH_FAILED',
        'Fetch API is unavailable to load trusted RulePack payload.',
      );
    }
    let response: Response;
    try {
      response = await fetchFn(anchor.payloadAssetPath, { cache: 'no-store' });
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'network error';
      throw new RulePackVerificationError(
        'PAYLOAD_FETCH_FAILED',
        `Failed to fetch RulePack payload from ${anchor.payloadAssetPath}: ${msg}`,
      );
    }
    if (!response.ok) {
      throw new RulePackVerificationError(
        'PAYLOAD_FETCH_FAILED',
        `HTTP ${response.status} fetching RulePack payload from ${anchor.payloadAssetPath}.`,
      );
    }
    payloadBytes = new Uint8Array(await response.arrayBuffer());
  }

  if (payloadBytes.byteLength !== PINNED_RULEPACK_PAYLOAD_BYTES) {
    throw new RulePackVerificationError(
      'PAYLOAD_DIGEST_MISMATCH',
      `RulePack payload length mismatch: expected ${PINNED_RULEPACK_PAYLOAD_BYTES} bytes, got ${payloadBytes.byteLength}.`,
    );
  }

  const actualPayloadSha = await sha256Hex(payloadBytes, options.subtleOverride);
  if (actualPayloadSha !== expectedPayloadSha) {
    throw new RulePackVerificationError(
      'PAYLOAD_DIGEST_MISMATCH',
      `RulePack payload SHA-256 mismatch: expected ${expectedPayloadSha}, got ${actualPayloadSha}.`,
    );
  }

  const rulePack = create(MarketRulePackSchema, {
    rulePackId: create(UlidSchema, { value: manifest.rule_pack_id }),
    version: BigInt(manifest.version),
    owner: create(OwnerRefSchema, {
      tenantId: create(UlidSchema, { value: manifest.owner.tenant_id }),
      ownerId: create(UlidSchema, { value: manifest.owner.owner_id }),
    }),
    market: manifest.market,
    ruleType: manifest.rule_type,
    source: manifest.source,
    effectiveFrom: effectiveFrom.marketTime,
    effectiveTo: effectiveTo.marketTime,
    verificationStatus: VerificationStatus.VERIFIED,
    contentHash: create(Sha256Schema, { value: hexToBytes(actualPayloadSha) }),
    content: create(AnySchema, {
      typeUrl: manifest.type_url,
      value: new Uint8Array(payloadBytes),
    }),
  });

  return {
    rulePack,
    manifestSha256: canonicalManifestDigest,
    payloadSha256: actualPayloadSha,
    payloadByteLength: payloadBytes.byteLength,
    manifest,
  };
}
