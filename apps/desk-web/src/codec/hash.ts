/**
 * FIPS 180-4 SHA-256 hashing with automatic fallback from Web Crypto (`crypto.subtle`)
 * to a pure-TypeScript implementation when running without SubtleCrypto.
 */

const SHA256_K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

const SHA256_H0 = new Uint32Array([
  0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
]);

function rotr32(value: number, bits: number): number {
  return (value >>> bits) | (value << (32 - bits));
}

export function bytesToLowerHex(bytes: Uint8Array): string {
  const hexChars = '0123456789abcdef';
  let out = '';
  for (let i = 0; i < bytes.length; i += 1) {
    const b = bytes[i] ?? 0;
    out += (hexChars[b >>> 4] ?? '0') + (hexChars[b & 0x0f] ?? '0');
  }
  return out;
}

export function hexToBytes(hex: string): Uint8Array {
  if (hex.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(hex)) {
    throw new Error('Invalid hexadecimal string.');
  }
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i += 1) {
    out[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

/**
 * Synchronous pure-TypeScript FIPS 180-4 SHA-256 implementation.
 */
export function sha256Pure(input: Uint8Array): Uint8Array {
  const bitLen = BigInt(input.byteLength) * 8n;
  const paddedLen = Math.ceil((input.byteLength + 9) / 64) * 64;
  const padded = new Uint8Array(paddedLen);
  padded.set(input, 0);
  padded[input.byteLength] = 0x80;

  const view = new DataView(padded.buffer, padded.byteOffset, padded.byteLength);
  view.setUint32(paddedLen - 8, Number((bitLen >> 32n) & 0xffffffffn), false);
  view.setUint32(paddedLen - 4, Number(bitLen & 0xffffffffn), false);

  let h0 = SHA256_H0[0] ?? 0;
  let h1 = SHA256_H0[1] ?? 0;
  let h2 = SHA256_H0[2] ?? 0;
  let h3 = SHA256_H0[3] ?? 0;
  let h4 = SHA256_H0[4] ?? 0;
  let h5 = SHA256_H0[5] ?? 0;
  let h6 = SHA256_H0[6] ?? 0;
  let h7 = SHA256_H0[7] ?? 0;

  const w = new Uint32Array(64);

  for (let offset = 0; offset < paddedLen; offset += 64) {
    for (let i = 0; i < 16; i += 1) {
      w[i] = view.getUint32(offset + i * 4, false);
    }
    for (let i = 16; i < 64; i += 1) {
      const w15 = w[i - 15] ?? 0;
      const w2 = w[i - 2] ?? 0;
      const s0 = rotr32(w15, 7) ^ rotr32(w15, 18) ^ (w15 >>> 3);
      const s1 = rotr32(w2, 17) ^ rotr32(w2, 19) ^ (w2 >>> 10);
      w[i] = ((w[i - 16] ?? 0) + s0 + (w[i - 7] ?? 0) + s1) >>> 0;
    }

    let a = h0;
    let b = h1;
    let c = h2;
    let d = h3;
    let e = h4;
    let f = h5;
    let g = h6;
    let h = h7;

    for (let i = 0; i < 64; i += 1) {
      const s1 = rotr32(e, 6) ^ rotr32(e, 11) ^ rotr32(e, 25);
      const ch = (e & f) ^ (~e & g);
      const temp1 = (h + s1 + ch + (SHA256_K[i] ?? 0) + (w[i] ?? 0)) >>> 0;
      const s0 = rotr32(a, 2) ^ rotr32(a, 13) ^ rotr32(a, 22);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const temp2 = (s0 + maj) >>> 0;

      h = g;
      g = f;
      f = e;
      e = (d + temp1) >>> 0;
      d = c;
      c = b;
      b = a;
      a = (temp1 + temp2) >>> 0;
    }

    h0 = (h0 + a) >>> 0;
    h1 = (h1 + b) >>> 0;
    h2 = (h2 + c) >>> 0;
    h3 = (h3 + d) >>> 0;
    h4 = (h4 + e) >>> 0;
    h5 = (h5 + f) >>> 0;
    h6 = (h6 + g) >>> 0;
    h7 = (h7 + h) >>> 0;
  }

  const out = new Uint8Array(32);
  const outView = new DataView(out.buffer);
  outView.setUint32(0, h0, false);
  outView.setUint32(4, h1, false);
  outView.setUint32(8, h2, false);
  outView.setUint32(12, h3, false);
  outView.setUint32(16, h4, false);
  outView.setUint32(20, h5, false);
  outView.setUint32(24, h6, false);
  outView.setUint32(28, h7, false);
  return out;
}

export function sha256PureHex(input: Uint8Array): string {
  return bytesToLowerHex(sha256Pure(input));
}

export interface SubtleCryptoLike {
  readonly digest?: (
    algorithm: AlgorithmIdentifier,
    data: BufferSource,
  ) => Promise<ArrayBuffer>;
}

/**
 * Computes SHA-256 digest bytes using `crypto.subtle` when available,
 * transparently falling back to the pure-TypeScript implementation otherwise.
 */
export async function sha256Bytes(
  input: Uint8Array,
  subtleOverride?: SubtleCryptoLike | null,
): Promise<Uint8Array> {
  const subtle =
    subtleOverride !== undefined
      ? subtleOverride
      : (globalThis.crypto?.subtle as SubtleCryptoLike | undefined);

  if (subtle && typeof subtle.digest === 'function') {
    const copy = new Uint8Array(input.byteLength);
    copy.set(input);
    const buffer = await subtle.digest('SHA-256', copy.buffer);
    return new Uint8Array(buffer);
  }
  return sha256Pure(input);
}

export async function sha256Hex(
  input: Uint8Array,
  subtleOverride?: SubtleCryptoLike | null,
): Promise<string> {
  return bytesToLowerHex(await sha256Bytes(input, subtleOverride));
}
