import { create } from '@bufbuild/protobuf';
import {
  DecimalValueSchema,
  type DecimalValue,
  type UnitRef,
} from '../generated/ficant/core/v1/common_pb';
import { unitsEqual, validateUnitRef } from './primitives';

export const MAX_DECIMAL_PRECISION = 28;
export const MAX_DECIMAL_SCALE = 12;
export const SCALE_12_FACTOR = 1_000_000_000_000n;
export const I128_MIN = -(1n << 127n);
export const I128_MAX = (1n << 127n) - 1n;

const CANONICAL_COEFFICIENT_PATTERN = /^(?:0|-?[1-9][0-9]*)$/;
const HUMAN_DECIMAL_PATTERN = /^(-)?([0-9]+)(?:\.([0-9]+))?$/;
const UNSIGNED_DIGITS_PATTERN = /^[0-9]+$/;

export class DecimalValidationError extends Error {
  readonly field: string;

  constructor(field: string, message: string) {
    super(`${field}: ${message}`);
    this.name = 'DecimalValidationError';
    this.field = field;
  }
}

export interface CanonicalDecimalParts {
  readonly coefficient: string;
  readonly scale: number;
  readonly scaledI128: bigint;
}

export interface ValidateDecimalOptions {
  readonly field?: string;
  readonly requireUnit?: boolean;
  readonly expectedUnit?: UnitRef;
  readonly sign?: 'any' | 'positive' | 'non-negative';
  readonly maxScale?: number;
}

export interface FormatDecimalOptions {
  readonly missingText?: string;
  readonly minFractionDigits?: number;
  readonly grouping?: boolean;
}

function checkScaledI128Bounds(coefficient: string, scale: number, field: string): bigint {
  if (!Number.isInteger(scale) || scale < 0 || scale > MAX_DECIMAL_SCALE) {
    throw new DecimalValidationError(
      field,
      `scale must be an integer in 0..=${MAX_DECIMAL_SCALE}, got ${String(scale)}`,
    );
  }
  const coeffBigInt = BigInt(coefficient);
  const multiplier = 10n ** BigInt(MAX_DECIMAL_SCALE - scale);
  const scaled = coeffBigInt * multiplier;
  if (scaled < I128_MIN || scaled > I128_MAX) {
    throw new DecimalValidationError(
      field,
      `scaled value coefficient * 10^(12 - scale) overflows signed i128`,
    );
  }
  return scaled;
}

/**
 * Normalizes a human-entered base-10 decimal string (e.g. `"100.2500"`, `"-0.005"`, `"0.00"`)
 * into canonical `(coefficient, scale, scaledI128)` parts.
 *
 * Rejects leading `"+"`, negative zero (`"-0"`, `"-0.0"`), whitespace, scientific notation,
 * precision > 28 digits, scale > 12, and signed i128 overflow at 10^12 scaling.
 */
export function parseDecimalString(input: string, field = 'decimal'): CanonicalDecimalParts {
  if (input.length === 0) {
    throw new DecimalValidationError(field, 'must not be empty');
  }
  if (input.startsWith('+')) {
    throw new DecimalValidationError(field, 'leading "+" sign is not permitted');
  }
  const match = HUMAN_DECIMAL_PATTERN.exec(input);
  if (!match) {
    throw new DecimalValidationError(field, `invalid base-10 decimal string "${input}"`);
  }

  const negative = match[1] === '-';
  const rawInt = match[2] ?? '';
  const rawFrac = match[3] ?? '';

  // Trim trailing fractional zeros first so scale reflects canonical precision.
  let frac = rawFrac;
  while (frac.length > 0 && frac.endsWith('0')) {
    frac = frac.slice(0, -1);
  }

  if (frac.length > MAX_DECIMAL_SCALE) {
    throw new DecimalValidationError(
      field,
      `fractional scale ${frac.length} exceeds maximum ${MAX_DECIMAL_SCALE}`,
    );
  }

  const combinedUnsigned = `${rawInt}${frac}`;
  const strippedLeading = combinedUnsigned.replace(/^0+/, '');
  const isZero = strippedLeading.length === 0;

  if (negative && isZero) {
    throw new DecimalValidationError(field, 'negative zero ("-0") is not permitted');
  }

  if (isZero) {
    return {
      coefficient: '0',
      scale: 0,
      scaledI128: 0n,
    };
  }

  if (strippedLeading.length > MAX_DECIMAL_PRECISION) {
    throw new DecimalValidationError(
      field,
      `significant precision ${strippedLeading.length} exceeds maximum ${MAX_DECIMAL_PRECISION} digits`,
    );
  }

  let digits = strippedLeading;
  let scale = frac.length;
  while (scale > 0 && digits.endsWith('0')) {
    digits = digits.slice(0, -1);
    scale -= 1;
  }

  const coefficient = negative ? `-${digits}` : digits;
  const scaledI128 = checkScaledI128Bounds(coefficient, scale, field);

  return {
    coefficient,
    scale,
    scaledI128,
  };
}

/**
 * Normalizes a `(coefficient, scale)` pair into canonical form.
 * Rejects `"+"`, `"-0"`, non-digit characters, precision > 28, scale > 12, and i128 overflow.
 */
export function normalizeDecimalParts(
  coefficient: string,
  scale: number,
  field = 'decimal',
): CanonicalDecimalParts {
  if (coefficient.length === 0) {
    throw new DecimalValidationError(field, 'coefficient must not be empty');
  }
  if (coefficient.startsWith('+')) {
    throw new DecimalValidationError(field, 'leading "+" sign is not permitted');
  }
  if (!Number.isInteger(scale) || scale < 0 || scale > MAX_DECIMAL_SCALE) {
    throw new DecimalValidationError(
      field,
      `scale must be an integer in 0..=${MAX_DECIMAL_SCALE}, got ${String(scale)}`,
    );
  }

  const negative = coefficient.startsWith('-');
  const unsigned = negative ? coefficient.slice(1) : coefficient;
  if (unsigned.length === 0 || !UNSIGNED_DIGITS_PATTERN.test(unsigned)) {
    throw new DecimalValidationError(field, `invalid coefficient "${coefficient}"`);
  }
  if (unsigned.length > MAX_DECIMAL_PRECISION) {
    throw new DecimalValidationError(
      field,
      `coefficient precision ${unsigned.length} exceeds maximum ${MAX_DECIMAL_PRECISION}`,
    );
  }

  const stripped = unsigned.replace(/^0+/, '');
  if (stripped.length === 0) {
    if (negative) {
      throw new DecimalValidationError(field, 'negative zero ("-0") is not permitted');
    }
    return {
      coefficient: '0',
      scale: 0,
      scaledI128: 0n,
    };
  }

  let digits = stripped;
  let normalizedScale = scale;
  while (normalizedScale > 0 && digits.endsWith('0')) {
    digits = digits.slice(0, -1);
    normalizedScale -= 1;
  }

  const canonicalCoefficient = negative ? `-${digits}` : digits;
  const scaledI128 = checkScaledI128Bounds(canonicalCoefficient, normalizedScale, field);
  return {
    coefficient: canonicalCoefficient,
    scale: normalizedScale,
    scaledI128,
  };
}

/**
 * Strictly validates a protobuf `DecimalValue` on the wire without silently normalizing away
 * non-canonical encodings (leading zeros, trailing fractional zeros, `"-0"`, `"+"`, out-of-range
 * precision/scale, i128 overflow, or missing/mismatched `UnitRef`).
 */
export function validateCanonicalDecimalValue(
  value: DecimalValue | undefined,
  options: ValidateDecimalOptions = {},
): CanonicalDecimalParts {
  const field = options.field ?? 'decimal';
  const requireUnit = options.requireUnit ?? true;
  const maxScale = options.maxScale ?? MAX_DECIMAL_SCALE;

  if (value === undefined) {
    throw new DecimalValidationError(field, 'is required');
  }

  const { coefficient, scale, unit } = value;
  if (typeof coefficient !== 'string' || !CANONICAL_COEFFICIENT_PATTERN.test(coefficient)) {
    throw new DecimalValidationError(
      field,
      `non-canonical or invalid coefficient "${String(coefficient)}"`,
    );
  }

  const unsigned = coefficient.startsWith('-') ? coefficient.slice(1) : coefficient;
  if (unsigned.length > MAX_DECIMAL_PRECISION) {
    throw new DecimalValidationError(
      field,
      `coefficient precision ${unsigned.length} exceeds maximum ${MAX_DECIMAL_PRECISION}`,
    );
  }

  if (!Number.isInteger(scale) || scale < 0 || scale > maxScale) {
    throw new DecimalValidationError(
      field,
      `scale must be an integer in 0..=${maxScale}, got ${String(scale)}`,
    );
  }

  if (coefficient === '0' && scale !== 0) {
    throw new DecimalValidationError(field, 'zero value must have scale 0');
  }
  if (scale > 0 && unsigned.endsWith('0')) {
    throw new DecimalValidationError(
      field,
      `non-canonical trailing zero in coefficient "${coefficient}" with scale ${scale}`,
    );
  }

  const scaledI128 = checkScaledI128Bounds(coefficient, scale, field);

  if (requireUnit || options.expectedUnit !== undefined) {
    try {
      validateUnitRef(unit, `${field}.unit`);
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'invalid unit';
      throw new DecimalValidationError(`${field}.unit`, msg);
    }
    if (options.expectedUnit !== undefined && !unitsEqual(unit, options.expectedUnit)) {
      throw new DecimalValidationError(
        `${field}.unit`,
        'unit does not match expected UnitRef (unit_id and version)',
      );
    }
  }

  if (options.sign === 'positive' && scaledI128 <= 0n) {
    throw new DecimalValidationError(field, 'must be strictly positive (> 0)');
  }
  if (options.sign === 'non-negative' && scaledI128 < 0n) {
    throw new DecimalValidationError(field, 'must be non-negative (>= 0)');
  }

  return {
    coefficient,
    scale,
    scaledI128,
  };
}

export function createDecimalValue(
  input: string | CanonicalDecimalParts,
  unit: UnitRef,
  field = 'decimal',
): DecimalValue {
  const validUnit = validateUnitRef(unit, `${field}.unit`);
  const parts = typeof input === 'string' ? parseDecimalString(input, field) : input;
  // Re-verify bounds in case caller constructed CanonicalDecimalParts manually.
  checkScaledI128Bounds(parts.coefficient, parts.scale, field);
  return create(DecimalValueSchema, {
    coefficient: parts.coefficient,
    scale: parts.scale,
    unit: validUnit,
  });
}

export function isPresentDecimal(value: DecimalValue | undefined): value is DecimalValue {
  return value !== undefined;
}

/**
 * Returns true ONLY when `value` is present AND numerically zero.
 * Missing (`undefined`) always returns `false`, preserving presence distinction.
 */
export function isZeroDecimal(value: DecimalValue | undefined): boolean {
  if (value === undefined) {
    return false;
  }
  const parts = validateCanonicalDecimalValue(value, { requireUnit: false });
  return parts.scaledI128 === 0n;
}

export function compareDecimalValues(
  lhs: DecimalValue,
  rhs: DecimalValue,
  options: { requireMatchingUnit?: boolean } = {},
): -1 | 0 | 1 {
  const requireMatchingUnit = options.requireMatchingUnit ?? true;
  const left = validateCanonicalDecimalValue(lhs, { requireUnit: requireMatchingUnit });
  const right = validateCanonicalDecimalValue(rhs, { requireUnit: requireMatchingUnit });
  if (requireMatchingUnit && !unitsEqual(lhs.unit, rhs.unit)) {
    throw new DecimalValidationError('unit', 'cannot compare decimals with differing UnitRef');
  }
  if (left.scaledI128 < right.scaledI128) {
    return -1;
  }
  if (left.scaledI128 > right.scaledI128) {
    return 1;
  }
  return 0;
}

function addGroupingCommas(integerDigits: string): string {
  return integerDigits.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

/**
 * Exact string-based decimal formatter (never routes through IEEE-754 `Number`).
 * Explicitly distinguishes missing (`undefined` -> `missingText`, default `"未提供"`)
 * from numeric zero (`"0"`).
 */
export function formatDecimalValue(
  value: DecimalValue | undefined,
  options: FormatDecimalOptions = {},
): string {
  if (value === undefined) {
    return options.missingText ?? '未提供';
  }
  const parts = validateCanonicalDecimalValue(value, { requireUnit: false });
  const negative = parts.coefficient.startsWith('-');
  const digits = negative ? parts.coefficient.slice(1) : parts.coefficient;

  let intPart: string;
  let fracPart: string;

  if (parts.scale === 0) {
    intPart = digits;
    fracPart = '';
  } else if (digits.length <= parts.scale) {
    intPart = '0';
    fracPart = '0'.repeat(parts.scale - digits.length) + digits;
  } else {
    const splitAt = digits.length - parts.scale;
    intPart = digits.slice(0, splitAt);
    fracPart = digits.slice(splitAt);
  }

  const minFrac = options.minFractionDigits ?? 0;
  if (minFrac > fracPart.length) {
    fracPart = fracPart.padEnd(minFrac, '0');
  }

  const formattedInt = options.grouping ? addGroupingCommas(intPart) : intPart;
  const sign = negative ? '-' : '';
  return fracPart.length > 0 ? `${sign}${formattedInt}.${fracPart}` : `${sign}${formattedInt}`;
}

/**
 * Lossy conversion strictly intended for chart plotting (ECharts).
 * Never use for table display, request encoding, or assertions.
 */
export function decimalToPlotNumber(value: DecimalValue | undefined): number | null {
  if (value === undefined) {
    return null;
  }
  const text = formatDecimalValue(value, { grouping: false });
  const num = Number(text);
  return Number.isFinite(num) ? num : null;
}
