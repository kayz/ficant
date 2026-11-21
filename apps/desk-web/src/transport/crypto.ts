import {
  bytesToLowerHex,
  hexToBytes,
  sha256Bytes,
  sha256Hex,
  sha256Pure,
  sha256PureHex,
  type SubtleCryptoLike,
} from '../codec/hash';

export {
  bytesToLowerHex,
  hexToBytes,
  sha256Bytes,
  sha256Hex,
  sha256Pure,
  sha256PureHex,
  type SubtleCryptoLike,
};

export interface CryptoLike {
  readonly randomUUID?: () => string;
  readonly getRandomValues?: <T extends ArrayBufferView>(array: T) => T;
  readonly subtle?: SubtleCryptoLike | undefined;
}

export class SecureRandomUnavailableError extends Error {
  readonly code = 'SECURE_RANDOM_UNAVAILABLE' as const;

  constructor(
    message = 'Cryptographically secure random source (crypto.randomUUID / crypto.getRandomValues) is unavailable; new write intents are blocked.',
  ) {
    super(message);
    this.name = 'SecureRandomUnavailableError';
  }
}

function resolveCrypto(override?: CryptoLike | null): CryptoLike | undefined {
  if (override !== undefined) {
    return override ?? undefined;
  }
  return globalThis.crypto as CryptoLike | undefined;
}

export function hasSecureRandom(cryptoOverride?: CryptoLike | null): boolean {
  const c = resolveCrypto(cryptoOverride);
  if (!c) {
    return false;
  }
  return typeof c.randomUUID === 'function' || typeof c.getRandomValues === 'function';
}

/**
 * Generates an RFC 4122 version 4 UUID using `crypto.randomUUID` when available,
 * falling back to `crypto.getRandomValues` when `randomUUID` is unavailable.
 *
 * Throws `SecureRandomUnavailableError` if no cryptographically secure PRNG exists.
 * Never falls back to `Math.random()`.
 */
export function generateUuidV4(cryptoOverride?: CryptoLike | null): string {
  const c = resolveCrypto(cryptoOverride);
  if (!c) {
    throw new SecureRandomUnavailableError();
  }
  if (typeof c.randomUUID === 'function') {
    return c.randomUUID();
  }
  if (typeof c.getRandomValues === 'function') {
    const bytes = new Uint8Array(16);
    c.getRandomValues(bytes);
    bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
    bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
    const hex = bytesToLowerHex(bytes);
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
  }
  throw new SecureRandomUnavailableError();
}
