import { create } from '@bufbuild/protobuf';
import { TimestampSchema } from '@bufbuild/protobuf/wkt';
import {
  MarketTimeSchema,
  Sha256Schema,
  UlidSchema,
  UnitRefSchema,
  type MarketTime,
  type Sha256,
  type Ulid,
  type UnitRef,
} from '../generated/ficant/core/v1/common_pb';
import { bytesToLowerHex, hexToBytes } from './hash';

export const UINT64_MAX = (1n << 64n) - 1n;
export const ULID_PATTERN = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
export const ISO_DATE_PATTERN = /^(\d{4})-(\d{2})-(\d{2})$/;
export const SHA256_HEX_PATTERN = /^[0-9a-f]{64}$/;

export class PrimitiveValidationError extends Error {
  readonly field: string;

  constructor(field: string, message: string) {
    super(`${field}: ${message}`);
    this.name = 'PrimitiveValidationError';
    this.field = field;
  }
}

export function isNonEmptyTrimmed(value: string): boolean {
  return value.length > 0 && value.trim() === value;
}

export function assertNonEmptyTrimmed(value: string | undefined, field: string): string {
  if (typeof value !== 'string' || !isNonEmptyTrimmed(value)) {
    throw new PrimitiveValidationError(
      field,
      'must be a non-empty string without leading or trailing whitespace',
    );
  }
  return value;
}

export function isValidUlid(value: string): boolean {
  return ULID_PATTERN.test(value);
}

export function validateUlid(ulid: Ulid | undefined, field = 'ulid'): string {
  if (!ulid || !isValidUlid(ulid.value)) {
    throw new PrimitiveValidationError(field, 'must be a canonical 26-character ULID');
  }
  return ulid.value;
}

export function createUlid(value: string, field = 'ulid'): Ulid {
  if (!isValidUlid(value)) {
    throw new PrimitiveValidationError(field, 'must be a canonical 26-character ULID');
  }
  return create(UlidSchema, { value });
}

export function validateSha256(sha: Sha256 | undefined, field = 'sha256'): Uint8Array {
  if (!sha || sha.value.byteLength !== 32) {
    throw new PrimitiveValidationError(field, 'must contain exactly 32 bytes');
  }
  return sha.value;
}

export function createSha256FromHex(hex: string, field = 'sha256'): Sha256 {
  if (!SHA256_HEX_PATTERN.test(hex)) {
    throw new PrimitiveValidationError(field, 'must be 64 lowercase hexadecimal characters');
  }
  return create(Sha256Schema, { value: hexToBytes(hex) });
}

export function sha256ToHex(sha: Sha256 | undefined, field = 'sha256'): string {
  return bytesToLowerHex(validateSha256(sha, field));
}

export function validateUnitRef(unit: UnitRef | undefined, field = 'unit'): UnitRef {
  if (!unit) {
    throw new PrimitiveValidationError(field, 'is required');
  }
  if (!unit.unitId || !isNonEmptyTrimmed(unit.unitId.value)) {
    throw new PrimitiveValidationError(`${field}.unit_id`, 'must be a non-empty trimmed string');
  }
  if (unit.version < 1n || unit.version > UINT64_MAX) {
    throw new PrimitiveValidationError(`${field}.version`, 'must be >= 1');
  }
  return unit;
}

export function createUnitRef(unitId: string, version: bigint): UnitRef {
  const ref = create(UnitRefSchema, {
    unitId: create(UlidSchema, { value: unitId }),
    version,
  });
  return validateUnitRef(ref);
}

export function unitsEqual(lhs: UnitRef | undefined, rhs: UnitRef | undefined): boolean {
  if (!lhs || !rhs || !lhs.unitId || !rhs.unitId) {
    return false;
  }
  return lhs.unitId.value === rhs.unitId.value && lhs.version === rhs.version;
}

export function formatUnitRef(unit: UnitRef | undefined): string {
  if (!unit || !unit.unitId) {
    return '未提供';
  }
  return `${unit.unitId.value}@v${unit.version.toString()}`;
}

export function parseUint64(value: bigint | number | string, field = 'uint64'): bigint {
  let parsed: bigint;
  if (typeof value === 'bigint') {
    parsed = value;
  } else if (typeof value === 'number') {
    if (!Number.isSafeInteger(value) || value < 0) {
      throw new PrimitiveValidationError(field, 'must be a non-negative safe integer');
    }
    parsed = BigInt(value);
  } else {
    if (!/^(?:0|[1-9][0-9]*)$/.test(value)) {
      throw new PrimitiveValidationError(field, 'must be a canonical non-negative base-10 integer');
    }
    parsed = BigInt(value);
  }

  if (parsed < 0n || parsed > UINT64_MAX) {
    throw new PrimitiveValidationError(field, 'must be within unsigned 64-bit integer range');
  }
  return parsed;
}

export function formatSeq(seq: bigint | undefined): string {
  return seq === undefined ? '—' : seq.toString();
}

export function compareUint64(lhs: bigint, rhs: bigint): -1 | 0 | 1 {
  if (lhs < rhs) {
    return -1;
  }
  if (lhs > rhs) {
    return 1;
  }
  return 0;
}

export function minUint64(values: readonly bigint[]): bigint {
  if (values.length === 0) {
    throw new PrimitiveValidationError('values', 'cannot compute min of empty bigint array');
  }
  let current = values[0] as bigint;
  for (let i = 1; i < values.length; i += 1) {
    const next = values[i] as bigint;
    if (next < current) {
      current = next;
    }
  }
  return current;
}

export function maxUint64(values: readonly bigint[]): bigint {
  if (values.length === 0) {
    throw new PrimitiveValidationError('values', 'cannot compute max of empty bigint array');
  }
  let current = values[0] as bigint;
  for (let i = 1; i < values.length; i += 1) {
    const next = values[i] as bigint;
    if (next > current) {
      current = next;
    }
  }
  return current;
}

export function validateIsoDate(dateStr: string, field = 'date'): string {
  const match = ISO_DATE_PATTERN.exec(dateStr);
  if (!match) {
    throw new PrimitiveValidationError(field, 'must be formatted as YYYY-MM-DD');
  }
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  if (month < 1 || month > 12 || day < 1 || day > 31) {
    throw new PrimitiveValidationError(field, `invalid calendar date "${dateStr}"`);
  }
  const probe = new Date(Date.UTC(year, month - 1, day));
  if (
    probe.getUTCFullYear() !== year ||
    probe.getUTCMonth() !== month - 1 ||
    probe.getUTCDate() !== day
  ) {
    throw new PrimitiveValidationError(field, `invalid calendar date "${dateStr}"`);
  }
  return dateStr;
}

export function formatLocalDateInTimezone(date: Date, marketTimezone: string): string {
  try {
    return new Intl.DateTimeFormat('en-CA', {
      timeZone: marketTimezone,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
    }).format(date);
  } catch {
    throw new PrimitiveValidationError(
      'market_timezone',
      `invalid IANA timezone "${marketTimezone}"`,
    );
  }
}

export interface ValidatedMarketTime {
  readonly epochMs: number;
  readonly instantSeconds: bigint;
  readonly instantNanos: number;
  readonly marketTimezone: string;
  readonly localTradingDate: string;
}

export function validateMarketTime(
  mt: MarketTime | undefined,
  field = 'market_time',
): ValidatedMarketTime {
  if (!mt) {
    throw new PrimitiveValidationError(field, 'is required');
  }
  if (!mt.instant) {
    throw new PrimitiveValidationError(`${field}.instant`, 'is required');
  }
  const { seconds, nanos } = mt.instant;
  if (!Number.isInteger(nanos) || nanos < 0 || nanos >= 1_000_000_000) {
    throw new PrimitiveValidationError(`${field}.instant.nanos`, 'must be in 0..999999999');
  }
  const marketTimezone = assertNonEmptyTrimmed(mt.marketTimezone, `${field}.market_timezone`);
  const localTradingDate = validateIsoDate(mt.localTradingDate, `${field}.local_trading_date`);

  const epochMs = Number(seconds) * 1000 + Math.floor(nanos / 1_000_000);
  const date = new Date(epochMs);
  if (Number.isNaN(date.getTime())) {
    throw new PrimitiveValidationError(`${field}.instant`, 'represents an invalid timestamp');
  }

  const expectedLocalDate = formatLocalDateInTimezone(date, marketTimezone);
  if (expectedLocalDate !== localTradingDate) {
    throw new PrimitiveValidationError(
      `${field}.local_trading_date`,
      `local_trading_date (${localTradingDate}) does not match instant in ${marketTimezone} (${expectedLocalDate})`,
    );
  }

  return {
    epochMs,
    instantSeconds: seconds,
    instantNanos: nanos,
    marketTimezone,
    localTradingDate,
  };
}

export function marketTimeToEpochMs(mt: MarketTime | undefined, field = 'market_time'): number {
  return validateMarketTime(mt, field).epochMs;
}

export function createMarketTimeFromDate(date: Date, marketTimezone: string): MarketTime {
  const epochMs = date.getTime();
  if (Number.isNaN(epochMs)) {
    throw new PrimitiveValidationError('date', 'must be a valid Date');
  }
  const tz = assertNonEmptyTrimmed(marketTimezone, 'market_timezone');
  const localTradingDate = formatLocalDateInTimezone(date, tz);
  const seconds = BigInt(Math.floor(epochMs / 1000));
  const remMs = ((epochMs % 1000) + 1000) % 1000;
  return create(MarketTimeSchema, {
    instant: create(TimestampSchema, {
      seconds,
      nanos: remMs * 1_000_000,
    }),
    marketTimezone: tz,
    localTradingDate,
  });
}

export function createMarketTimeFromIso(
  isoInstant: string,
  marketTimezone: string,
  expectedLocalTradingDate?: string,
): MarketTime {
  const parsed = new Date(isoInstant);
  if (Number.isNaN(parsed.getTime())) {
    throw new PrimitiveValidationError('instant', `invalid ISO-8601 instant "${isoInstant}"`);
  }
  const mt = createMarketTimeFromDate(parsed, marketTimezone);
  if (expectedLocalTradingDate !== undefined && mt.localTradingDate !== expectedLocalTradingDate) {
    throw new PrimitiveValidationError(
      'local_trading_date',
      `expected ${expectedLocalTradingDate}, derived ${mt.localTradingDate} for ${marketTimezone}`,
    );
  }
  return mt;
}
