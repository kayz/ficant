import { Code, ConnectError } from '@connectrpc/connect';
import type {
  EarmarkRecord,
  FundingGapSummary,
  InstrumentPositionView,
  RiskAndCoverageSummary,
} from '../generated/ficant/blotter/v1/model_pb';
import type { ConfirmHoldResult } from '../generated/ficant/blotter/v1/service_pb';
import {
  ErrorCode,
  type ErrorDetail,
  type FieldViolation,
} from '../generated/ficant/core/v1/error_pb';

export type BlotterClientErrorKind =
  | 'business'
  | 'confirm_hold_expired'
  | 'transport'
  | 'protocol'
  | 'auth'
  | 'validation'
  | 'aborted';

export interface BlotterClientErrorInit {
  readonly kind: BlotterClientErrorKind;
  readonly message: string;
  readonly errorCode?: ErrorCode;
  readonly connectCode?: Code;
  readonly retryable?: boolean;
  readonly traceId?: string;
  readonly resourceRef?: string;
  readonly fieldViolations?: readonly FieldViolation[];
  readonly committedSeq?: bigint;
  readonly replayed?: boolean;
  readonly earmark?: EarmarkRecord;
  readonly position?: InstrumentPositionView;
  readonly riskSummary?: RiskAndCoverageSummary;
  readonly fundingSummary?: FundingGapSummary;
  readonly detail?: ErrorDetail;
  readonly cause?: unknown;
}

const BEARER_REDACTION_PATTERN = /\bBearer\s+\S+/gi;

export function sanitizeErrorMessage(
  rawMessage: string,
  secrets: readonly string[] = [],
): string {
  let sanitized = rawMessage.replace(BEARER_REDACTION_PATTERN, 'Bearer [REDACTED]');
  for (const secret of secrets) {
    if (secret.length > 0) {
      sanitized = sanitized.split(secret).join('[REDACTED]');
    }
  }
  return sanitized;
}

function mapConnectCodeToErrorCode(code: Code): ErrorCode {
  switch (code) {
    case Code.Unauthenticated:
      return ErrorCode.UNAUTHENTICATED;
    case Code.PermissionDenied:
      return ErrorCode.FORBIDDEN;
    case Code.InvalidArgument:
      return ErrorCode.VALIDATION_FAILED;
    case Code.NotFound:
      return ErrorCode.NOT_FOUND;
    case Code.AlreadyExists:
      return ErrorCode.ALREADY_EXISTS;
    case Code.Aborted:
      return ErrorCode.CONCURRENCY_CONFLICT;
    case Code.Unavailable:
    case Code.DeadlineExceeded:
      return ErrorCode.STORAGE_UNAVAILABLE;
    default:
      return ErrorCode.INTERNAL;
  }
}

function isRetryableConnectCode(code: Code): boolean {
  return (
    code === Code.Unavailable ||
    code === Code.DeadlineExceeded ||
    code === Code.ResourceExhausted ||
    code === Code.Aborted
  );
}

/**
 * Structured error surface for Desk Blotter unary & streaming RPCs.
 *
 * Distinctly represents:
 * - Business `ErrorDetail` responses (`kind === 'business'` / `'auth'`)
 * - `ConfirmHold` lazy-expiry committed rejections (`kind === 'confirm_hold_expired'`, carrying
 *   `committedSeq`, `replayed`, `earmark`, `position`, `riskSummary`, `fundingSummary`, and
 *   `detail = reconfirmationError`)
 * - Transport / network errors (`kind === 'transport'`)
 * - Protocol / contract invariant violations (`kind === 'protocol'`)
 * - Client-side cancellations (`kind === 'aborted'`)
 */
export class BlotterClientError extends Error {
  readonly kind: BlotterClientErrorKind;
  readonly errorCode: ErrorCode;
  readonly connectCode: Code | undefined;
  readonly retryable: boolean;
  readonly traceId: string;
  readonly resourceRef: string;
  readonly fieldViolations: readonly FieldViolation[];
  readonly committedSeq: bigint | undefined;
  readonly replayed: boolean;
  readonly earmark: EarmarkRecord | undefined;
  readonly position: InstrumentPositionView | undefined;
  readonly riskSummary: RiskAndCoverageSummary | undefined;
  readonly fundingSummary: FundingGapSummary | undefined;
  readonly detail: ErrorDetail | undefined;

  constructor(init: BlotterClientErrorInit) {
    super(init.message, init.cause !== undefined ? { cause: init.cause } : undefined);
    this.name = 'BlotterClientError';
    this.kind = init.kind;
    this.errorCode = init.errorCode ?? ErrorCode.UNSPECIFIED;
    this.connectCode = init.connectCode;
    this.retryable = init.retryable ?? false;
    this.traceId = init.traceId ?? '';
    this.resourceRef = init.resourceRef ?? '';
    this.fieldViolations = init.fieldViolations ?? [];
    this.committedSeq = init.committedSeq;
    this.replayed = init.replayed ?? false;
    this.earmark = init.earmark;
    this.position = init.position;
    this.riskSummary = init.riskSummary;
    this.fundingSummary = init.fundingSummary;
    this.detail = init.detail;
  }

  isUnauthenticated(): boolean {
    return (
      this.errorCode === ErrorCode.UNAUTHENTICATED || this.connectCode === Code.Unauthenticated
    );
  }

  isForbidden(): boolean {
    return this.errorCode === ErrorCode.FORBIDDEN || this.connectCode === Code.PermissionDenied;
  }

  isConcurrencyConflict(): boolean {
    return (
      this.errorCode === ErrorCode.CONCURRENCY_CONFLICT ||
      this.errorCode === ErrorCode.VERSION_CONFLICT
    );
  }

  isAborted(): boolean {
    return this.kind === 'aborted' || this.connectCode === Code.Canceled;
  }

  static fromErrorDetail(
    detail: ErrorDetail,
    secrets: readonly string[] = [],
  ): BlotterClientError {
    const isAuth =
      detail.code === ErrorCode.UNAUTHENTICATED || detail.code === ErrorCode.FORBIDDEN;
    return new BlotterClientError({
      kind: isAuth ? 'auth' : 'business',
      message: sanitizeErrorMessage(detail.message || 'Desk Blotter request failed', secrets),
      errorCode: detail.code,
      retryable: detail.retryable,
      traceId: detail.traceId,
      resourceRef: detail.resourceRef,
      fieldViolations: [...detail.fieldViolations],
      detail,
    });
  }

  /**
   * Decodes a `ConfirmHoldResult` whose server-side lazy-expiry committed `EXPIRED`
   * and returned a populated `reconfirmation_error` along with `committed_seq` and `replayed`.
   */
  static fromConfirmHoldExpired(
    result: ConfirmHoldResult,
    secrets: readonly string[] = [],
  ): BlotterClientError {
    const detail = result.reconfirmationError;
    const message = sanitizeErrorMessage(
      detail?.message || 'HOLD expired before confirmation; reconfirmation rejected',
      secrets,
    );
    return new BlotterClientError({
      kind: 'confirm_hold_expired',
      message,
      errorCode: detail?.code ?? ErrorCode.VALIDATION_FAILED,
      retryable: detail?.retryable ?? false,
      traceId: detail?.traceId ?? '',
      resourceRef: detail?.resourceRef ?? '',
      fieldViolations: detail ? [...detail.fieldViolations] : [],
      committedSeq: result.committedSeq,
      replayed: result.replayed,
      ...(result.earmark !== undefined ? { earmark: result.earmark } : {}),
      ...(result.position !== undefined ? { position: result.position } : {}),
      ...(result.riskSummary !== undefined ? { riskSummary: result.riskSummary } : {}),
      ...(result.fundingSummary !== undefined ? { fundingSummary: result.fundingSummary } : {}),
      ...(detail !== undefined ? { detail } : {}),
    });
  }

  static protocol(message: string, cause?: unknown): BlotterClientError {
    return new BlotterClientError({
      kind: 'protocol',
      message,
      errorCode: ErrorCode.INTERNAL,
      retryable: false,
      ...(cause !== undefined ? { cause } : {}),
    });
  }

  static aborted(message = 'Request aborted', cause?: unknown): BlotterClientError {
    return new BlotterClientError({
      kind: 'aborted',
      message,
      connectCode: Code.Canceled,
      retryable: false,
      ...(cause !== undefined ? { cause } : {}),
    });
  }

  static fromUnknown(err: unknown, secrets: readonly string[] = []): BlotterClientError {
    if (err instanceof BlotterClientError) {
      return err;
    }
    if (err instanceof ConnectError) {
      if (err.code === Code.Canceled) {
        return BlotterClientError.aborted(sanitizeErrorMessage(err.rawMessage, secrets), err);
      }
      const mappedCode = mapConnectCodeToErrorCode(err.code);
      const isAuth =
        err.code === Code.Unauthenticated || err.code === Code.PermissionDenied;
      return new BlotterClientError({
        kind: isAuth ? 'auth' : 'transport',
        message: sanitizeErrorMessage(err.rawMessage || err.message, secrets),
        errorCode: mappedCode,
        connectCode: err.code,
        retryable: isRetryableConnectCode(err.code),
        cause: err,
      });
    }
    if (err instanceof DOMException && err.name === 'AbortError') {
      return BlotterClientError.aborted('Request aborted', err);
    }
    if (err instanceof Error && err.name === 'AbortError') {
      return BlotterClientError.aborted(sanitizeErrorMessage(err.message, secrets), err);
    }
    const rawMsg = err instanceof Error ? err.message : String(err);
    return new BlotterClientError({
      kind: 'transport',
      message: sanitizeErrorMessage(rawMsg, secrets),
      errorCode: ErrorCode.STORAGE_UNAVAILABLE,
      retryable: true,
      cause: err,
    });
  }
}
