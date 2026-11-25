"""Deterministic finite retry & exponential backoff policy for Ficant sidecars."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
import math
import time
from typing import TypeVar

from ficant.core.v1 import error_pb2
import grpc

from ficant_sdk.blotter import BlotterError

_T = TypeVar("_T")

_NON_RETRYABLE_BLOTTER_CODES = frozenset(
    {
        error_pb2.ERROR_CODE_UNAUTHENTICATED,
        error_pb2.ERROR_CODE_FORBIDDEN,
        error_pb2.ERROR_CODE_VALIDATION_FAILED,
        error_pb2.ERROR_CODE_HASH_MISMATCH,
        error_pb2.ERROR_CODE_CONCURRENCY_CONFLICT,
        error_pb2.ERROR_CODE_IMMUTABLE_VIOLATION,
        error_pb2.ERROR_CODE_NOT_FOUND,
        error_pb2.ERROR_CODE_ALREADY_EXISTS,
        error_pb2.ERROR_CODE_VERSION_CONFLICT,
        error_pb2.ERROR_CODE_LINEAGE_INCOMPLETE,
    }
)

_RETRYABLE_GRPC_STATUS_CODES = frozenset(
    {
        grpc.StatusCode.DEADLINE_EXCEEDED,
        grpc.StatusCode.UNAVAILABLE,
        grpc.StatusCode.RESOURCE_EXHAUSTED,
        grpc.StatusCode.ABORTED,
    }
)


def is_retryable_error(exc: BaseException) -> bool:
    """Classify whether an RPC/transport exception is eligible for bounded backoff retry.

    - ``BlotterError``: NEVER retries ``UNAUTHENTICATED``, ``FORBIDDEN``,
      ``VALIDATION_FAILED``, ``HASH_MISMATCH``, ``CONCURRENCY_CONFLICT``, or other
      non-retryable business errors. Retries ``STORAGE_UNAVAILABLE`` only when
      ``exc.retryable`` is ``True``.
    - ``grpc.RpcError``: Retries transient transport codes (``DEADLINE_EXCEEDED``,
      ``UNAVAILABLE``, ``RESOURCE_EXHAUSTED``, ``ABORTED``).
    - Network timeouts / connection errors: ``TimeoutError`` and ``ConnectionError``.
    """
    if isinstance(exc, BlotterError):
        if exc.code in _NON_RETRYABLE_BLOTTER_CODES:
            return False
        if exc.code == error_pb2.ERROR_CODE_STORAGE_UNAVAILABLE:
            return bool(exc.retryable)
        return False

    if isinstance(exc, grpc.RpcError):
        code_fn = getattr(exc, "code", None)
        if callable(code_fn):
            try:
                return code_fn() in _RETRYABLE_GRPC_STATUS_CODES
            except Exception:
                return False
        return False

    if isinstance(exc, (TimeoutError, ConnectionError)):
        return True

    return False


@dataclass(frozen=True)
class BackoffPolicy:
    """Deterministic finite exponential backoff with injectable ``clock`` and ``sleep_fn``."""

    max_retries: int = 3
    initial_delay_ms: int = 100
    max_delay_ms: int = 5_000
    multiplier: float = 2.0
    clock: Callable[[], float] = field(default=time.monotonic, repr=False)
    sleep_fn: Callable[[float], None] = field(default=time.sleep, repr=False)

    def __post_init__(self) -> None:
        if isinstance(self.max_retries, bool) or not isinstance(self.max_retries, int) or self.max_retries < 0:
            raise ValueError("max_retries must be a non-negative integer")
        if (
            isinstance(self.initial_delay_ms, bool)
            or not isinstance(self.initial_delay_ms, int)
            or self.initial_delay_ms <= 0
        ):
            raise ValueError("initial_delay_ms must be a positive integer")
        if (
            isinstance(self.max_delay_ms, bool)
            or not isinstance(self.max_delay_ms, int)
            or self.max_delay_ms < self.initial_delay_ms
        ):
            raise ValueError("max_delay_ms must be an integer >= initial_delay_ms")
        if (
            isinstance(self.multiplier, bool)
            or not isinstance(self.multiplier, (int, float))
            or not math.isfinite(self.multiplier)
            or self.multiplier < 1.0
        ):
            raise ValueError("multiplier must be a finite number >= 1.0")
        if not callable(self.clock):
            raise ValueError("clock must be callable")
        if not callable(self.sleep_fn):
            raise ValueError("sleep_fn must be callable")

    def delay_ms_for_attempt(self, attempt: int) -> int:
        """Compute bounded backoff delay in milliseconds for zero-based retry ``attempt``."""
        if attempt < 0:
            raise ValueError("attempt must be non-negative")
        raw = self.initial_delay_ms * (self.multiplier**attempt)
        if not math.isfinite(raw) or raw >= self.max_delay_ms:
            return self.max_delay_ms
        return max(self.initial_delay_ms, int(round(raw)))

    def delay_seconds_for_attempt(self, attempt: int) -> float:
        return self.delay_ms_for_attempt(attempt) / 1_000.0

    def execute(
        self,
        fn: Callable[[], _T],
        *,
        on_retry: Callable[[int, BaseException, float], None] | None = None,
    ) -> _T:
        """Execute ``fn`` with bounded retries on retryable failures only."""
        attempt = 0
        while True:
            _ = self.clock()
            try:
                return fn()
            except BaseException as exc:
                if attempt >= self.max_retries or not is_retryable_error(exc):
                    raise
                delay_seconds = self.delay_seconds_for_attempt(attempt)
                if on_retry is not None:
                    on_retry(attempt + 1, exc, delay_seconds)
                self.sleep_fn(delay_seconds)
                attempt += 1
