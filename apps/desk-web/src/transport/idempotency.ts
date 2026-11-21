import { bytesToLowerHex, generateUuidV4, type CryptoLike } from './crypto';

export interface StickyWriteIntent {
  readonly operation: string;
  readonly idempotencyKey: string;
  readonly payloadFingerprint: string;
  readonly sessionGeneration: number;
}

export interface AcquireWriteIntentParams {
  readonly operation: string;
  readonly canonicalPayload: unknown;
  readonly sessionGeneration: number;
  readonly cryptoOverride?: CryptoLike | null;
}

/**
 * Produces a deterministic canonical string fingerprint for any write payload,
 * including `bigint` versions/seqs, `Uint8Array` hashes, nested objects (sorted keys), and arrays.
 */
export function canonicalizePayloadFingerprint(value: unknown): string {
  if (value === null) {
    return 'null';
  }
  if (value === undefined) {
    return 'undefined';
  }
  if (typeof value === 'bigint') {
    return `bigint:${value.toString()}`;
  }
  if (typeof value === 'string') {
    return JSON.stringify(value);
  }
  if (typeof value === 'number' || typeof value === 'boolean') {
    return String(value);
  }
  if (value instanceof Uint8Array) {
    return `bytes:${bytesToLowerHex(value)}`;
  }
  if (value instanceof Date) {
    return `date:${value.toISOString()}`;
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => canonicalizePayloadFingerprint(item)).join(',')}]`;
  }
  if (typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>)
      .filter(([, v]) => v !== undefined)
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    const serialized = entries
      .map(([k, v]) => `${JSON.stringify(k)}:${canonicalizePayloadFingerprint(v)}`)
      .join(',');
    return `{${serialized}}`;
  }
  return String(value);
}

/**
 * Manages sticky write intents per logical operation slot.
 *
 * Guarantees:
 * - Same `(operation, canonicalPayload, sessionGeneration)` reuses the EXACT same
 *   `idempotency_key` across transport retries.
 * - Changing ANY parameter, CAS version, or `sessionGeneration` immediately discards the old
 *   intent and allocates a fresh RFC 4122 UUIDv4 via `generateUuidV4` (blocking if secure PRNG
 *   is unavailable).
 * - Session switches/logouts wipe all pending intents synchronously.
 */
export class WriteIntentManager {
  readonly #active = new Map<string, StickyWriteIntent>();

  acquire(params: AcquireWriteIntentParams): StickyWriteIntent {
    const fingerprint = canonicalizePayloadFingerprint(params.canonicalPayload);
    const existing = this.#active.get(params.operation);

    if (
      existing !== undefined &&
      existing.sessionGeneration === params.sessionGeneration &&
      existing.payloadFingerprint === fingerprint
    ) {
      return existing;
    }

    const idempotencyKey = generateUuidV4(params.cryptoOverride);
    const next: StickyWriteIntent = {
      operation: params.operation,
      idempotencyKey,
      payloadFingerprint: fingerprint,
      sessionGeneration: params.sessionGeneration,
    };
    this.#active.set(params.operation, next);
    return next;
  }

  getActive(operation: string): StickyWriteIntent | undefined {
    return this.#active.get(operation);
  }

  complete(operation: string, idempotencyKey?: string): void {
    const existing = this.#active.get(operation);
    if (!existing) {
      return;
    }
    if (idempotencyKey === undefined || existing.idempotencyKey === idempotencyKey) {
      this.#active.delete(operation);
    }
  }

  clearAll(): void {
    this.#active.clear();
  }

  clearForSessionSwitch(nextSessionGeneration?: number): void {
    if (nextSessionGeneration === undefined) {
      this.#active.clear();
      return;
    }
    for (const [op, intent] of this.#active.entries()) {
      if (intent.sessionGeneration !== nextSessionGeneration) {
        this.#active.delete(op);
      }
    }
  }

  size(): number {
    return this.#active.size;
  }
}
