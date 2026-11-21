import { create, toBinary } from '@bufbuild/protobuf';
import { describe, expect, it, vi } from 'vitest';
import {
  EarmarkRecordSchema,
  EarmarkState,
  PrincipalRole,
} from '../generated/ficant/blotter/v1/model_pb';
import {
  ConfirmHoldResponseSchema,
  GetAccountSnapshotResponseSchema,
  SubscribeEventsResponseSchema,
} from '../generated/ficant/blotter/v1/service_pb';
import { ErrorCode, ErrorDetailSchema, FieldViolationSchema } from '../generated/ficant/core/v1/error_pb';
import { UlidSchema } from '../generated/ficant/core/v1/common_pb';
import {
  AUTHORIZATION_HEADER,
  BlotterClientError,
  CredentialValidationError,
  FICANT_CREDENTIAL_HEADER,
  InMemoryCredentialStore,
  InsecureHttpContextError,
  SecureRandomUnavailableError,
  WriteIntentManager,
  assertExclusiveCredentialHeaders,
  assertSecureCredentialContext,
  createDeskBlotterClient,
  generateUuidV4,
  hasSecureRandom,
  resolveSameOriginBaseUrl,
  sanitizeErrorMessage,
  sha256Hex,
  validateRawCredentialToken,
} from './index';

function encodeGrpcWebFrame(flag: number, payload: Uint8Array): Uint8Array {
  const frame = new Uint8Array(5 + payload.byteLength);
  frame[0] = flag;
  const view = new DataView(frame.buffer);
  view.setUint32(1, payload.byteLength, false);
  frame.set(payload, 5);
  return frame;
}

function concatBytes(...chunks: readonly Uint8Array[]): Uint8Array<ArrayBuffer> {
  const total = chunks.reduce((sum, c) => sum + c.byteLength, 0);
  const out = new Uint8Array(new ArrayBuffer(total));
  let offset = 0;
  for (const c of chunks) {
    out.set(c, offset);
    offset += c.byteLength;
  }
  return out;
}

function makeGrpcWebBinaryResponse(messageBytes: Uint8Array, grpcStatus = 0): Response {
  const dataFrame = encodeGrpcWebFrame(0x00, messageBytes);
  const trailerFrame = encodeGrpcWebFrame(
    0x80,
    new TextEncoder().encode(`grpc-status:${grpcStatus}\r\n`),
  );
  const body = concatBytes(dataFrame, trailerFrame);
  return new Response(body, {
    status: 200,
    headers: { 'content-type': 'application/grpc-web+proto' },
  });
}

describe('Secure PRNG fallback, UUIDv4, and SHA-256 fallback', () => {
  const UUID_V4_PATTERN =
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

  it('generates RFC 4122 UUIDv4 via getRandomValues when randomUUID is absent and blocks when PRNG is missing', () => {
    const fallbackCrypto = {
      getRandomValues<T extends ArrayBufferView>(arr: T): T {
        const view = new Uint8Array(arr.buffer, arr.byteOffset, arr.byteLength);
        for (let i = 0; i < view.length; i += 1) {
          view[i] = (i * 37 + 11) & 0xff;
        }
        return arr;
      },
    };
    expect(hasSecureRandom(fallbackCrypto)).toBe(true);
    const id = generateUuidV4(fallbackCrypto);
    expect(UUID_V4_PATTERN.test(id)).toBe(true);

    expect(hasSecureRandom({})).toBe(false);
    expect(hasSecureRandom(null)).toBe(false);
    expect(() => generateUuidV4({})).toThrow(SecureRandomUnavailableError);
    expect(() => generateUuidV4(null)).toThrow(SecureRandomUnavailableError);
  });

  it('computes FIPS 180-4 SHA-256 when crypto.subtle is unavailable', async () => {
    const digest = await sha256Hex(new TextEncoder().encode('abc'), null);
    expect(digest).toBe('ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
  });
});

describe('HTTPS/loopback enforcement, token validation, and mutually exclusive in-memory credentials', () => {
  it('blocks non-loopback plain HTTP with INSECURE_HTTP_CONTEXT and allows HTTPS & loopback HTTP', () => {
    expect(() =>
      assertSecureCredentialContext({ protocol: 'http:', hostname: 'desk.intranet.local' }),
    ).toThrow(InsecureHttpContextError);
    expect(() =>
      assertSecureCredentialContext({ protocol: 'http:', hostname: '10.0.1.25' }),
    ).toThrow(/INSECURE_HTTP_CONTEXT/);

    expect(() =>
      assertSecureCredentialContext({ protocol: 'https:', hostname: 'desk.intranet.local' }),
    ).not.toThrow();
    expect(() =>
      assertSecureCredentialContext({ protocol: 'http:', hostname: '127.0.0.1' }),
    ).not.toThrow();
    expect(() =>
      assertSecureCredentialContext({ protocol: 'http:', hostname: 'localhost' }),
    ).not.toThrow();
    expect(() =>
      assertSecureCredentialContext({ protocol: 'http:', hostname: '[::1]' }),
    ).not.toThrow();
  });

  it('rejects empty, whitespace, control characters, and conflicting/duplicate headers', () => {
    for (const bad of ['', ' ', 'tok with space', 'tok\ttab', 'tok\nnewline', 'tok\x00nul', 'tok中文']) {
      expect(() => validateRawCredentialToken(bad)).toThrow(CredentialValidationError);
    }
    expect(validateRawCredentialToken('valid-token_01.XYZ')).toBe('valid-token_01.XYZ');

    // Missing header
    expect(() => assertExclusiveCredentialHeaders({})).toThrow(CredentialValidationError);
    // Both headers simultaneously -> rejected
    expect(() =>
      assertExclusiveCredentialHeaders({
        [AUTHORIZATION_HEADER]: 'Bearer tok-1',
        [FICANT_CREDENTIAL_HEADER]: 'tok-1',
      }),
    ).toThrow(/mutually exclusive/i);
    // Duplicate authorization header -> rejected
    expect(() =>
      assertExclusiveCredentialHeaders({
        [AUTHORIZATION_HEADER]: ['Bearer tok-1', 'Bearer tok-2'],
      }),
    ).toThrow(/duplicate/i);
    // Bad authorization prefix -> rejected
    expect(() =>
      assertExclusiveCredentialHeaders({
        [AUTHORIZATION_HEADER]: 'Basic abc123',
      }),
    ).toThrow(/Bearer /i);
    // Valid single headers
    expect(
      assertExclusiveCredentialHeaders({ [AUTHORIZATION_HEADER]: 'Bearer tok-abc' }),
    ).toEqual({ mode: 'authorization', token: 'tok-abc' });
    expect(
      assertExclusiveCredentialHeaders({ [FICANT_CREDENTIAL_HEADER]: 'tok-xyz' }),
    ).toEqual({ mode: 'x-ficant-credential', token: 'tok-xyz' });
  });

  it('keeps credentials strictly in memory without touching Web Storage or cookies and redacts serialization', () => {
    const store = new InMemoryCredentialStore();
    globalThis.localStorage.clear();
    globalThis.sessionStorage.clear();

    store.setCredential(
      {
        token: 'super-secret-desk-token-999',
        headerMode: 'authorization',
        roleHint: PrincipalRole.TRADER,
        actorLabel: 'trader-1',
      },
      { protocol: 'https:', hostname: 'desk.intranet.local' },
    );

    expect(globalThis.localStorage.length).toBe(0);
    expect(globalThis.sessionStorage.length).toBe(0);
    expect(document.cookie).toBe('');
    expect(JSON.stringify(store)).not.toContain('super-secret-desk-token-999');
    expect(store.toString()).not.toContain('super-secret-desk-token-999');

    const headers = new Headers({ [FICANT_CREDENTIAL_HEADER]: 'stale-other-header' });
    store.applyToHeaders(headers);
    expect(headers.get(AUTHORIZATION_HEADER)).toBe('Bearer super-secret-desk-token-999');
    expect(headers.get(FICANT_CREDENTIAL_HEADER)).toBeNull();

    // Switching to x-ficant-credential strips authorization header
    store.setCredential(
      { token: 'alt-secret-token-777', headerMode: 'x-ficant-credential' },
      { protocol: 'http:', hostname: '127.0.0.1' },
    );
    store.applyToHeaders(headers);
    expect(headers.get(AUTHORIZATION_HEADER)).toBeNull();
    expect(headers.get(FICANT_CREDENTIAL_HEADER)).toBe('alt-secret-token-777');

    // Non-loopback plain HTTP rejected before storing
    expect(() =>
      store.setCredential(
        { token: 'blocked-token', headerMode: 'authorization' },
        { protocol: 'http:', hostname: '192.168.1.50' },
      ),
    ).toThrow(InsecureHttpContextError);
  });
});

describe('Same-origin binary gRPC-Web client, ErrorDetail, and ConfirmHold lazy-expiry decoding', () => {
  it('sends binary gRPC-Web requests with mutually exclusive single credential header and decodes unary/stream responses', async () => {
    const credStore = new InMemoryCredentialStore();
    credStore.setCredential(
      { token: 'bearer-tok-123', headerMode: 'authorization' },
      { protocol: 'http:', hostname: '127.0.0.1' },
    );

    const seenRequests: Array<{ url: string; contentType: string | null; auth: string | null; custom: string | null }> = [];

    const mockFetch: typeof fetch = async (input, init) => {
      const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
      const reqHeaders = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined));
      seenRequests.push({
        url,
        contentType: reqHeaders.get('content-type'),
        auth: reqHeaders.get(AUTHORIZATION_HEADER),
        custom: reqHeaders.get(FICANT_CREDENTIAL_HEADER),
      });

      if (url.endsWith('/ficant.blotter.v1.DeskBlotterService/GetAccountSnapshot')) {
        const resp = create(GetAccountSnapshotResponseSchema, {
          result: {
            case: 'value',
            value: {
              snapshot: {
                accountId: 'acct-a',
                snapshotSeq: 55n,
              },
            },
          },
        });
        return makeGrpcWebBinaryResponse(toBinary(GetAccountSnapshotResponseSchema, resp));
      }

      if (url.endsWith('/ficant.blotter.v1.DeskBlotterService/ConfirmHold')) {
        const resp = create(ConfirmHoldResponseSchema, {
          result: {
            case: 'value',
            value: {
              committedSeq: 88n,
              replayed: true,
              earmark: create(EarmarkRecordSchema, {
                earmarkId: create(UlidSchema, { value: '01JNB0G1EARMARKID000000008' }),
                earmarkRevision: 2n,
                accountId: 'acct-a',
                state: EarmarkState.EXPIRED,
              }),
              reconfirmationError: create(ErrorDetailSchema, {
                code: ErrorCode.VALIDATION_FAILED,
                message: 'hold expired; trade feed paused',
                retryable: true,
                fieldViolations: [
                  create(FieldViolationSchema, {
                    field: 'feed',
                    description: 'trade feed is paused',
                  }),
                ],
              }),
            },
          },
        });
        return makeGrpcWebBinaryResponse(toBinary(ConfirmHoldResponseSchema, resp));
      }

      if (url.endsWith('/ficant.blotter.v1.DeskBlotterService/ReleaseHold')) {
        const resp = create(ConfirmHoldResponseSchema, {
          result: {
            case: 'error',
            value: create(ErrorDetailSchema, {
              code: ErrorCode.FORBIDDEN,
              message: 'forbidden for Bearer bearer-tok-123',
              traceId: 'trace-xyz',
              retryable: false,
            }),
          },
        });
        return makeGrpcWebBinaryResponse(toBinary(ConfirmHoldResponseSchema, resp));
      }

      if (url.endsWith('/ficant.blotter.v1.DeskBlotterService/SubscribeEvents')) {
        const frameMsg = create(SubscribeEventsResponseSchema, {
          payload: {
            case: 'heartbeat',
            value: {
              headSeq: 90n,
              scannedThroughSeq: 90n,
            },
          },
        });
        return makeGrpcWebBinaryResponse(toBinary(SubscribeEventsResponseSchema, frameMsg));
      }

      return new Response('not found', { status: 404 });
    };

    const client = createDeskBlotterClient({
      credentialStore: credStore,
      baseUrl: 'http://127.0.0.1:5173',
      location: { protocol: 'http:', hostname: '127.0.0.1' },
      fetch: mockFetch,
    });

    // 1. Unary GetAccountSnapshot with authorization header
    const snapRes = await client.getAccountSnapshot({ accountId: 'acct-a' });
    expect(snapRes.snapshot?.snapshotSeq).toBe(55n);
    expect(seenRequests[0]?.contentType).toBe('application/grpc-web+proto');
    expect(seenRequests[0]?.auth).toBe('Bearer bearer-tok-123');
    expect(seenRequests[0]?.custom).toBeNull();

    // 2. Switch header mode to x-ficant-credential and verify ConfirmHold lazy-expiry decoding
    credStore.setCredential(
      { token: 'custom-tok-456', headerMode: 'x-ficant-credential' },
      { protocol: 'http:', hostname: '127.0.0.1' },
    );
    let caughtConfirmErr: unknown;
    try {
      await client.confirmHold({
        idempotencyKey: 'key-1',
        accountId: 'acct-a',
        earmarkId: create(UlidSchema, { value: '01JNB0G1EARMARKID000000008' }),
        expectedEarmarkRevision: 1n,
        expectedPositionVersion: 1n,
      });
    } catch (err) {
      caughtConfirmErr = err;
    }
    expect(caughtConfirmErr).toBeInstanceOf(BlotterClientError);
    const confirmErr = caughtConfirmErr as BlotterClientError;
    expect(confirmErr.kind).toBe('confirm_hold_expired');
    expect(confirmErr.committedSeq).toBe(88n);
    expect(confirmErr.replayed).toBe(true);
    expect(confirmErr.earmark?.state).toBe(EarmarkState.EXPIRED);
    expect(confirmErr.fieldViolations[0]?.field).toBe('feed');
    expect(seenRequests[1]?.auth).toBeNull();
    expect(seenRequests[1]?.custom).toBe('custom-tok-456');

    // 3. Business ErrorDetail decoding + secret redaction
    credStore.setCredential(
      { token: 'bearer-tok-123', headerMode: 'authorization' },
      { protocol: 'http:', hostname: '127.0.0.1' },
    );
    await expect(
      client.releaseHold({
        idempotencyKey: 'key-2',
        accountId: 'acct-a',
        earmarkId: create(UlidSchema, { value: '01JNB0G1EARMARKID000000008' }),
        expectedEarmarkRevision: 1n,
        reason: 'release',
      }),
    ).rejects.toSatisfy((err: unknown) => {
      if (!(err instanceof BlotterClientError)) return false;
      return (
        err.isForbidden() &&
        err.traceId === 'trace-xyz' &&
        !err.message.includes('bearer-tok-123')
      );
    });

    // 4. Streaming SubscribeEvents binary frame decoding
    const frames = [];
    for await (const f of client.subscribeEvents({ afterSeq: 55n })) {
      frames.push(f);
    }
    expect(frames).toHaveLength(1);
    expect(frames[0]?.payload.case).toBe('heartbeat');
    if (frames[0]?.payload.case === 'heartbeat') {
      expect(frames[0].payload.value.headSeq).toBe(90n);
    }

    expect(sanitizeErrorMessage('failed token=abc Bearer xyz', ['abc'])).toBe(
      'failed token=[REDACTED] Bearer [REDACTED]',
    );
  });

  it('preserves explicit non-default port in resolveSameOriginBaseUrl and default client URL resolution (M1-F03)', async () => {
    expect(
      resolveSameOriginBaseUrl({
        protocol: 'http:',
        hostname: '127.0.0.1',
        port: '5173',
        origin: 'http://127.0.0.1:5173',
      }),
    ).toBe('http://127.0.0.1:5173');
    expect(
      resolveSameOriginBaseUrl({
        protocol: 'http:',
        hostname: '127.0.0.1',
        port: '4173',
      }),
    ).toBe('http://127.0.0.1:4173');
    expect(
      resolveSameOriginBaseUrl({
        protocol: 'https:',
        hostname: 'desk.intranet.local',
        port: '8443',
        origin: 'null',
      }),
    ).toBe('https://desk.intranet.local:8443');
    expect(
      resolveSameOriginBaseUrl({
        protocol: 'https:',
        hostname: 'desk.intranet.local',
        port: '',
      }),
    ).toBe('https://desk.intranet.local');
    expect(
      resolveSameOriginBaseUrl({
        protocol: 'https:',
        hostname: 'desk.intranet.local',
      }),
    ).toBe('https://desk.intranet.local');

    // Verify createDeskBlotterClient without explicit baseUrl preserves port from locationOverride
    const credStore = new InMemoryCredentialStore();
    credStore.setCredential(
      { token: 'port-test-tok', headerMode: 'authorization' },
      { protocol: 'http:', hostname: '127.0.0.1', port: '4173' },
    );
    const seenUrls: string[] = [];
    const mockFetch: typeof fetch = async (input) => {
      const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
      seenUrls.push(url);
      const resp = create(GetAccountSnapshotResponseSchema, {
        result: {
          case: 'value',
          value: { snapshot: { accountId: 'acct-p', snapshotSeq: 1n } },
        },
      });
      return makeGrpcWebBinaryResponse(toBinary(GetAccountSnapshotResponseSchema, resp));
    };

    const client = createDeskBlotterClient({
      credentialStore: credStore,
      location: { protocol: 'http:', hostname: '127.0.0.1', port: '4173' },
      fetch: mockFetch,
    });
    await client.getAccountSnapshot({ accountId: 'acct-p' });
    expect(seenUrls[0]).toBe(
      'http://127.0.0.1:4173/ficant.blotter.v1.DeskBlotterService/GetAccountSnapshot',
    );
  });
});

describe('Sticky WriteIntentManager', () => {
  it('reuses identical idempotency_key across retries, regenerates on parameter/version/session changes, and blocks without PRNG', () => {
    const manager = new WriteIntentManager();
    const uuidSpy = vi
      .fn<() => string>()
      .mockReturnValueOnce('11111111-1111-4111-8111-111111111111')
      .mockReturnValueOnce('22222222-2222-4222-8222-222222222222')
      .mockReturnValueOnce('33333333-3333-4333-8333-333333333333')
      .mockReturnValueOnce('44444444-4444-4444-8444-444444444444');
    const mockCrypto = { randomUUID: uuidSpy };

    const first = manager.acquire({
      operation: 'CreateHold:acct-a:240011.IB',
      canonicalPayload: { notional: '10000000', expectedPositionVersion: 3n, ttlSeconds: 1800 },
      sessionGeneration: 1,
      cryptoOverride: mockCrypto,
    });
    const retry = manager.acquire({
      operation: 'CreateHold:acct-a:240011.IB',
      canonicalPayload: { ttlSeconds: 1800, expectedPositionVersion: 3n, notional: '10000000' },
      sessionGeneration: 1,
      cryptoOverride: mockCrypto,
    });
    expect(retry.idempotencyKey).toBe(first.idempotencyKey);
    expect(uuidSpy).toHaveBeenCalledTimes(1);

    // Changing CAS version allocates a fresh key
    const versionBump = manager.acquire({
      operation: 'CreateHold:acct-a:240011.IB',
      canonicalPayload: { notional: '10000000', expectedPositionVersion: 4n, ttlSeconds: 1800 },
      sessionGeneration: 1,
      cryptoOverride: mockCrypto,
    });
    expect(versionBump.idempotencyKey).not.toBe(first.idempotencyKey);

    // Changing parameter allocates a fresh key
    const paramChange = manager.acquire({
      operation: 'CreateHold:acct-a:240011.IB',
      canonicalPayload: { notional: '20000000', expectedPositionVersion: 4n, ttlSeconds: 1800 },
      sessionGeneration: 1,
      cryptoOverride: mockCrypto,
    });
    expect(paramChange.idempotencyKey).not.toBe(versionBump.idempotencyKey);

    // Session generation switch allocates a fresh key
    const nextSession = manager.acquire({
      operation: 'CreateHold:acct-a:240011.IB',
      canonicalPayload: { notional: '20000000', expectedPositionVersion: 4n, ttlSeconds: 1800 },
      sessionGeneration: 2,
      cryptoOverride: mockCrypto,
    });
    expect(nextSession.idempotencyKey).not.toBe(paramChange.idempotencyKey);

    // Session switch clears all pending intents
    manager.clearForSessionSwitch();
    expect(manager.size()).toBe(0);

    // Missing secure PRNG blocks creating new write intents
    expect(() =>
      manager.acquire({
        operation: 'CreateHold:acct-a:240011.IB',
        canonicalPayload: { notional: '5000000' },
        sessionGeneration: 2,
        cryptoOverride: {},
      }),
    ).toThrow(SecureRandomUnavailableError);
  });
});
