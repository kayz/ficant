import type { SourceTextAnchor } from '../generated/ficant/blotter/v1/model_pb';
import { isNonEmptyTrimmed } from './primitives';

export class SourceTextAnchorError extends Error {
  readonly anchorIndex: number;

  constructor(anchorIndex: number, message: string) {
    super(`anchor[${anchorIndex}]: ${message}`);
    this.name = 'SourceTextAnchorError';
    this.anchorIndex = anchorIndex;
  }
}

export interface ValidatedAnchorSpan {
  readonly fieldName: string;
  readonly byteStart: number;
  readonly byteEnd: number;
  readonly utf16Start: number;
  readonly utf16End: number;
  readonly spanText: string;
}

export interface HighlightedTextSegment {
  readonly text: string;
  readonly highlighted: boolean;
  readonly fieldNames: readonly string[];
  readonly utf8Start: number;
  readonly utf8End: number;
}

interface Utf8BoundaryIndex {
  readonly byteToUtf16: ReadonlyMap<number, number>;
  readonly utf16ToByte: ReadonlyMap<number, number>;
  readonly totalBytes: number;
}

function utf8LengthOfCodePoint(cp: number): number {
  if (cp <= 0x7f) {
    return 1;
  }
  if (cp <= 0x7ff) {
    return 2;
  }
  if (cp <= 0xffff) {
    return 3;
  }
  return 4;
}

function buildUtf8BoundaryIndex(text: string): Utf8BoundaryIndex {
  const byteToUtf16 = new Map<number, number>();
  const utf16ToByte = new Map<number, number>();
  let bytePos = 0;
  let utf16Pos = 0;

  while (utf16Pos < text.length) {
    byteToUtf16.set(bytePos, utf16Pos);
    utf16ToByte.set(utf16Pos, bytePos);

    const cp = text.codePointAt(utf16Pos);
    if (cp === undefined) {
      break;
    }
    const utf16Len = cp > 0xffff ? 2 : 1;
    bytePos += utf8LengthOfCodePoint(cp);
    utf16Pos += utf16Len;
  }

  byteToUtf16.set(bytePos, utf16Pos);
  utf16ToByte.set(utf16Pos, bytePos);

  return {
    byteToUtf16,
    utf16ToByte,
    totalBytes: bytePos,
  };
}

/**
 * Computes the exact UTF-8 byte length of a JavaScript UTF-16 string.
 */
export function utf8ByteLength(text: string): number {
  return buildUtf8BoundaryIndex(text).totalBytes;
}

/**
 * Extracts a half-open UTF-8 byte slice `[byteStart, byteEnd)` from `text`,
 * strictly enforcing valid UTF-8 code-point boundaries (matching Rust `str::is_char_boundary`).
 */
export function sliceUtf8HalfOpen(text: string, byteStart: number, byteEnd: number): string {
  const index = buildUtf8BoundaryIndex(text);
  if (
    !Number.isInteger(byteStart) ||
    !Number.isInteger(byteEnd) ||
    byteStart < 0 ||
    byteStart >= byteEnd ||
    byteEnd > index.totalBytes
  ) {
    throw new RangeError(
      `Invalid UTF-8 half-open byte range [${String(byteStart)}, ${String(byteEnd)}) for length ${index.totalBytes}`,
    );
  }
  const utf16Start = index.byteToUtf16.get(byteStart);
  const utf16End = index.byteToUtf16.get(byteEnd);
  if (utf16Start === undefined || utf16End === undefined) {
    throw new RangeError(
      `UTF-8 byte range [${byteStart}, ${byteEnd}) splits a multi-byte character`,
    );
  }
  return text.slice(utf16Start, utf16End);
}

/**
 * Validates `SourceTextAnchor[]` against `rawSourceText`, matching
 * `crates/ficant-application/src/use_cases/blotter/drafts.rs::validate_source_text_anchors`.
 */
export function validateSourceTextAnchors(
  rawSourceText: string,
  anchors: readonly SourceTextAnchor[],
): readonly ValidatedAnchorSpan[] {
  if (rawSourceText.trim().length === 0) {
    throw new SourceTextAnchorError(-1, 'raw_source_text must not be empty or whitespace-only');
  }

  const index = buildUtf8BoundaryIndex(rawSourceText);
  const validated: ValidatedAnchorSpan[] = [];

  for (let i = 0; i < anchors.length; i += 1) {
    const anchor = anchors[i];
    if (!anchor) {
      continue;
    }
    if (!isNonEmptyTrimmed(anchor.fieldName)) {
      throw new SourceTextAnchorError(i, 'field_name must be a non-empty trimmed string');
    }
    const { byteStart, byteEnd } = anchor;
    if (
      !Number.isInteger(byteStart) ||
      !Number.isInteger(byteEnd) ||
      byteStart < 0 ||
      byteStart >= byteEnd ||
      byteEnd > index.totalBytes
    ) {
      throw new SourceTextAnchorError(
        i,
        `byte range [${String(byteStart)}, ${String(byteEnd)}) is out of bounds for ${index.totalBytes} UTF-8 bytes`,
      );
    }
    const utf16Start = index.byteToUtf16.get(byteStart);
    const utf16End = index.byteToUtf16.get(byteEnd);
    if (utf16Start === undefined || utf16End === undefined) {
      throw new SourceTextAnchorError(
        i,
        `byte range [${byteStart}, ${byteEnd}) does not fall on valid UTF-8 character boundaries`,
      );
    }
    const actualSlice = rawSourceText.slice(utf16Start, utf16End);
    if (actualSlice !== anchor.spanText) {
      throw new SourceTextAnchorError(
        i,
        `span_text mismatch: expected "${actualSlice}", got "${anchor.spanText}"`,
      );
    }
    validated.push({
      fieldName: anchor.fieldName,
      byteStart,
      byteEnd,
      utf16Start,
      utf16End,
      spanText: anchor.spanText,
    });
  }

  return validated;
}

/**
 * Produces deterministic, non-overlapping plain text segments for safe React text-node highlighting
 * (zero HTML injection). Supports multi-byte CJK characters, emoji, adjacent and overlapping anchors.
 */
export function buildHighlightedSegments(
  rawSourceText: string,
  anchors: readonly SourceTextAnchor[],
): readonly HighlightedTextSegment[] {
  const validated = validateSourceTextAnchors(rawSourceText, anchors);
  const index = buildUtf8BoundaryIndex(rawSourceText);

  if (validated.length === 0) {
    return [
      {
        text: rawSourceText,
        highlighted: false,
        fieldNames: [],
        utf8Start: 0,
        utf8End: index.totalBytes,
      },
    ];
  }

  const boundarySet = new Set<number>([0, index.totalBytes]);
  for (const span of validated) {
    boundarySet.add(span.byteStart);
    boundarySet.add(span.byteEnd);
  }
  const boundaries = Array.from(boundarySet).sort((a, b) => a - b);

  const segments: HighlightedTextSegment[] = [];
  for (let i = 0; i < boundaries.length - 1; i += 1) {
    const startByte = boundaries[i] as number;
    const endByte = boundaries[i + 1] as number;
    if (startByte === endByte) {
      continue;
    }
    const utf16Start = index.byteToUtf16.get(startByte);
    const utf16End = index.byteToUtf16.get(endByte);
    if (utf16Start === undefined || utf16End === undefined) {
      throw new SourceTextAnchorError(-1, 'internal boundary mapping error');
    }
    const activeFields = validated
      .filter((span) => span.byteStart <= startByte && span.byteEnd >= endByte)
      .map((span) => span.fieldName);

    segments.push({
      text: rawSourceText.slice(utf16Start, utf16End),
      highlighted: activeFields.length > 0,
      fieldNames: activeFields,
      utf8Start: startByte,
      utf8End: endByte,
    });
  }

  return segments;
}
