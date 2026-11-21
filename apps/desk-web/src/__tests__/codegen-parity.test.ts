// @vitest-environment node
import { create, fromBinary, toBinary } from '@bufbuild/protobuf';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { TRUSTED_RULEPACK_ANCHOR } from 'virtual:ficant-trusted-rulepack';
import {
  DeskBlotterService,
  GetAccountSnapshotRequestSchema,
  SubscribeEventsResponseSchema,
} from '../generated/ficant/blotter/v1/service_pb';
import {
  IntranetNlpIntentParserService,
  MarketQuoteStreamService,
  PositionBaseProviderService,
  TradeFeedReconcilerService,
} from '../generated/ficant/blotter/v1/spi_pb';
import { CgbFuturesDeliveryRulePackSchema } from '../generated/ficant/market/v1/cgb_futures_rule_pb';
import { RatesAnalyticsService } from '../generated/ficant/rates/v1/analytics_pb';
import {
  PINNED_RULEPACK_MANIFEST_SHA256,
  PINNED_RULEPACK_PAYLOAD_BYTES,
  PINNED_RULEPACK_PAYLOAD_SHA256,
  PINNED_RULEPACK_TYPE_URL,
  verifyTrustedRulePackEnvelope,
} from '../../vite.config';

const THIS_DIR = dirname(fileURLToPath(import.meta.url));
const DESK_WEB_ROOT = resolve(THIS_DIR, '..', '..');
const REPO_ROOT = resolve(DESK_WEB_ROOT, '..', '..');

describe('Strategy A generated contracts parity', () => {
  it('exposes all six gRPC services with expected DeskBlotterService RPC count and streaming shape', () => {
    expect(DeskBlotterService.typeName).toBe('ficant.blotter.v1.DeskBlotterService');
    expect(DeskBlotterService.methods).toHaveLength(18);
    expect(DeskBlotterService.method.subscribeEvents.methodKind).toBe('server_streaming');
    expect(DeskBlotterService.method.getAccountSnapshot.methodKind).toBe('unary');

    expect(PositionBaseProviderService.typeName).toBe(
      'ficant.blotter.v1.PositionBaseProviderService',
    );
    expect(TradeFeedReconcilerService.typeName).toBe(
      'ficant.blotter.v1.TradeFeedReconcilerService',
    );
    expect(MarketQuoteStreamService.typeName).toBe(
      'ficant.blotter.v1.MarketQuoteStreamService',
    );
    expect(IntranetNlpIntentParserService.typeName).toBe(
      'ficant.blotter.v1.IntranetNlpIntentParserService',
    );
    expect(RatesAnalyticsService.typeName).toBe('ficant.rates.v1.RatesAnalyticsService');
  });

  it('round-trips generated binary messages and decodes the golden 159-byte cgb-futures-v2 payload', () => {
    const req = create(GetAccountSnapshotRequestSchema, { accountId: 'acct-parity-1' });
    const reqBytes = toBinary(GetAccountSnapshotRequestSchema, req);
    const decodedReq = fromBinary(GetAccountSnapshotRequestSchema, reqBytes);
    expect(decodedReq.accountId).toBe('acct-parity-1');

    const streamMsg = create(SubscribeEventsResponseSchema, {
      payload: {
        case: 'heartbeat',
        value: {
          headSeq: 123n,
          scannedThroughSeq: 120n,
        },
      },
    });
    const streamBytes = toBinary(SubscribeEventsResponseSchema, streamMsg);
    const decodedStream = fromBinary(SubscribeEventsResponseSchema, streamBytes);
    expect(decodedStream.payload.case).toBe('heartbeat');
    if (decodedStream.payload.case === 'heartbeat') {
      expect(decodedStream.payload.value.headSeq).toBe(123n);
      expect(decodedStream.payload.value.scannedThroughSeq).toBe(120n);
    }

    const binPath = resolve(REPO_ROOT, 'domain-packs', 'cgb-futures', 'cgb-futures-v2.bin');
    const rawBin = readFileSync(binPath);
    expect(rawBin.byteLength).toBe(PINNED_RULEPACK_PAYLOAD_BYTES);
    const decodedPack = fromBinary(CgbFuturesDeliveryRulePackSchema, rawBin);
    expect(decodedPack.products.length).toBeGreaterThan(0);
    expect(decodedPack.deliveryMonths.length).toBeGreaterThan(0);
  });
});

describe('G-1 Trusted RulePack build-time anchor verification', () => {
  it('injects the verified RulePack anchor into virtual module and global define', () => {
    expect(TRUSTED_RULEPACK_ANCHOR.manifestSha256).toBe(PINNED_RULEPACK_MANIFEST_SHA256);
    expect(TRUSTED_RULEPACK_ANCHOR.payloadSha256).toBe(PINNED_RULEPACK_PAYLOAD_SHA256);
    expect(TRUSTED_RULEPACK_ANCHOR.payloadByteLength).toBe(PINNED_RULEPACK_PAYLOAD_BYTES);
    expect(TRUSTED_RULEPACK_ANCHOR.manifest.market).toBe('CFFEX');
    expect(TRUSTED_RULEPACK_ANCHOR.manifest.rule_type).toBe('cgb-futures');
    expect(TRUSTED_RULEPACK_ANCHOR.manifest.type_url).toBe(PINNED_RULEPACK_TYPE_URL);
    expect(TRUSTED_RULEPACK_ANCHOR.manifest.verification_status).toBe('VERIFIED');
    expect(TRUSTED_RULEPACK_ANCHOR.manifest.version).toBe(2);
    expect(__FICANT_TRUSTED_RULEPACK__).toEqual(TRUSTED_RULEPACK_ANCHOR);
  });

  it('fails closed on missing anchor, tampered manifest/payload, extended expiry, expired boundary, and wrong type', () => {
    const tmpDir = mkdtempSync(join(tmpdir(), 'ficant-rulepack-verify-'));
    try {
      const realManifestPath = resolve(
        DESK_WEB_ROOT,
        'src',
        'assets',
        'rulepacks',
        'cgb-futures-v2.manifest.json',
      );
      const realBinPath = resolve(REPO_ROOT, 'domain-packs', 'cgb-futures', 'cgb-futures-v2.bin');
      const realManifest = JSON.parse(readFileSync(realManifestPath, 'utf8')) as Record<
        string,
        unknown
      >;
      const realBin = readFileSync(realBinPath);

      // 1. Missing/invalid trust anchor
      expect(() =>
        verifyTrustedRulePackEnvelope({ expectedManifestSha256: '' }),
      ).toThrow(/trust anchor/i);

      // 2. Joint manifest + payload replacement with self-recomputed payload_sha256 fails against pinned manifest anchor
      const tamperedBin = Buffer.from(realBin);
      tamperedBin[0] = (tamperedBin[0] ?? 0) ^ 0xff;
      const tamperedBinPath = join(tmpDir, 'tampered.bin');
      writeFileSync(tamperedBinPath, tamperedBin);
      const tamperedBinSha = createHash('sha256').update(tamperedBin).digest('hex');

      const jointManifestPath = join(tmpDir, 'joint.manifest.json');
      writeFileSync(
        jointManifestPath,
        `${JSON.stringify({ ...realManifest, payload_sha256: tamperedBinSha }, null, 2)}\n`,
        'utf8',
      );
      expect(() =>
        verifyTrustedRulePackEnvelope({
          manifestPath: jointManifestPath,
          payloadPath: tamperedBinPath,
        }),
      ).toThrow(/manifest SHA-256 mismatch/i);

      // 3. Extending effective_to alone fails against pinned manifest SHA-256
      const extendedManifestPath = join(tmpDir, 'extended.manifest.json');
      writeFileSync(
        extendedManifestPath,
        `${JSON.stringify(
          {
            ...realManifest,
            effective_to: {
              instant: '2040-12-31T15:59:59Z',
              market_timezone: 'Asia/Shanghai',
              local_trading_date: '2040-12-31',
            },
          },
          null,
          2,
        )}\n`,
        'utf8',
      );
      expect(() =>
        verifyTrustedRulePackEnvelope({ manifestPath: extendedManifestPath }),
      ).toThrow(/manifest SHA-256 mismatch/i);

      // 4. Half-open boundary: now === effective_to fails closed
      expect(() =>
        verifyTrustedRulePackEnvelope({
          now: new Date('2035-12-31T15:59:59Z'),
        }),
      ).toThrow(/outside its half-open valid window/i);

      // 5. Wrong type_url fails closed even if manifest hash were hypothetically re-pinned
      const wrongTypeManifestBuf = Buffer.from(
        `${JSON.stringify({ ...realManifest, type_url: 'type.googleapis.com/invalid.Type' }, null, 2)}\n`,
        'utf8',
      );
      const wrongTypePath = join(tmpDir, 'wrong-type.manifest.json');
      writeFileSync(wrongTypePath, wrongTypeManifestBuf);
      const wrongTypeSha = createHash('sha256').update(wrongTypeManifestBuf).digest('hex');
      expect(() =>
        verifyTrustedRulePackEnvelope({
          manifestPath: wrongTypePath,
          expectedManifestSha256: wrongTypeSha,
        }),
      ).toThrow(/type_url mismatch/i);

      // 6. Tampered payload binary alone fails closed
      expect(() =>
        verifyTrustedRulePackEnvelope({ payloadPath: tamperedBinPath }),
      ).toThrow(/RulePack binary SHA-256 mismatch/i);
    } finally {
      rmSync(tmpDir, { recursive: true, force: true });
    }
  });
});
