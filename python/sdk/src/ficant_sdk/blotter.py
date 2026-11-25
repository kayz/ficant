"""Synchronous gRPC client for the Ficant Desk Blotter and versioned SPI services."""

from __future__ import annotations

from collections.abc import Callable, Iterator, Sequence
from decimal import Decimal, InvalidOperation
import re
from typing import TypeVar

import grpc
from ficant.blotter.v1 import (
    model_pb2,
    service_pb2,
    service_pb2_grpc,
    spi_pb2,
    spi_pb2_grpc,
)
from ficant.core.v1.common_pb2 import DecimalValue, UnitRef
from ficant.core.v1.error_pb2 import ErrorDetail
from ficant.rates.v1 import analytics_pb2


_Response = TypeVar("_Response")
_Result = TypeVar("_Result")

_VALID_HEDGE_SCOPES = frozenset(
    {
        model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT,
        model_pb2.PREVIEW_HEDGE_SCOPE_COVERED_SUBSET_ONLY,
    }
)
_CANONICAL_POSITIVE_INT_RE = re.compile(r"\A[1-9][0-9]*\Z")
_MAX_DECIMAL_PRECISION = 28
_MAX_DECIMAL_SCALE = 28

AUTHORIZATION_HEADER = "authorization"
FICANT_CREDENTIAL_HEADER = "x-ficant-credential"
EXPECTED_LEDGER_EPOCH_HEADER = "x-ficant-expected-ledger-epoch"
EXPECTED_LEDGER_INSTANCE_HEADER = "x-ficant-expected-ledger-instance"
_ULID_ALPHABET = frozenset("0123456789ABCDEFGHJKMNPQRSTVWXYZ")
_ULID_FIRST_CHARS = frozenset("01234567")
_MAX_U64 = (1 << 64) - 1


def _validate_raw_token(token: str, field_name: str) -> str:
    if not isinstance(token, str) or not token:
        raise ValueError(f"{field_name} must be a non-empty string")
    if any(ord(ch) <= 0x20 or ord(ch) == 0x7F or ch.isspace() for ch in token):
        raise ValueError(
            f"{field_name} must not contain whitespace or ASCII control characters"
        )
    return token


def _is_canonical_ulid_str(value: object) -> bool:
    if not isinstance(value, str) or len(value) != 26 or not value.isascii():
        return False
    return value[0] in _ULID_FIRST_CHARS and all(ch in _ULID_ALPHABET for ch in value)


def _validate_expected_ledger(expected_ledger: tuple[int, str]) -> tuple[int, str]:
    if not isinstance(expected_ledger, tuple) or len(expected_ledger) != 2:
        raise ValueError("expected_ledger must be a (ledger_epoch, ledger_instance_id) 2-tuple")
    epoch, instance_id = expected_ledger
    if isinstance(epoch, bool) or not isinstance(epoch, int) or epoch < 1 or epoch > _MAX_U64:
        raise ValueError("expected_ledger epoch must be a positive uint64 integer")
    if not _is_canonical_ulid_str(instance_id):
        raise ValueError(
            "expected_ledger instance_id must be a canonical 26-character uppercase ULID"
        )
    return (epoch, instance_id)


def _expected_ledger_metadata(
    expected_ledger: tuple[int, str] | None,
) -> tuple[tuple[str, str], ...]:
    if expected_ledger is None:
        return ()
    epoch, instance_id = _validate_expected_ledger(expected_ledger)
    return (
        (EXPECTED_LEDGER_EPOCH_HEADER, str(epoch)),
        (EXPECTED_LEDGER_INSTANCE_HEADER, instance_id),
    )


class BlotterError(Exception):
    """Structured business failure returned by Desk Blotter or SPI services.

    For ``ConfirmHold`` lazy-expiry rejections where the server committed an
    ``EXPIRED`` transition before returning ``reconfirmation_error``, this
    exception carries ``committed_seq`` and the committed record views.
    """

    def __init__(
        self,
        detail: ErrorDetail,
        *,
        committed_seq: int | None = None,
        replayed: bool = False,
        earmark: model_pb2.EarmarkRecord | None = None,
        position: model_pb2.InstrumentPositionView | None = None,
        risk_summary: model_pb2.RiskAndCoverageSummary | None = None,
        funding_summary: model_pb2.FundingGapSummary | None = None,
    ) -> None:
        super().__init__(detail.message)
        self.detail = detail
        self.code = detail.code
        self.retryable = detail.retryable
        self.field_violations = tuple(detail.field_violations)
        self.committed_seq = committed_seq
        self.replayed = replayed
        self.earmark = earmark
        self.position = position
        self.risk_summary = risk_summary
        self.funding_summary = funding_summary


class BlotterEventApplier:
    """Whole-transaction snapshot applier and strict monotonic cursor tracker.

    Validates an entire transaction envelope (or poll batch) before mutating any
    account snapshot and commits/persists the cursor strictly AFTER atomic
    in-memory application succeeds.
    """

    def __init__(
        self,
        initial_snapshots: dict[str, model_pb2.AccountBlotterSnapshot] | None = None,
        *,
        initial_cursor_seq: int | None = None,
        cursor_sink: Callable[[int], None] | None = None,
    ) -> None:
        self._snapshots: dict[str, model_pb2.AccountBlotterSnapshot] = {}
        self._allocations: dict[str, list[model_pb2.ReconcileAllocationView]] = {}
        max_snap_seq = 0
        if initial_snapshots:
            for account_id, snapshot in initial_snapshots.items():
                if not account_id or snapshot.account_id != account_id:
                    raise RuntimeError(
                        "BlotterEventApplier initial snapshot account_id mismatch: "
                        f"key={account_id!r}, snapshot={snapshot.account_id!r}"
                    )
                _validate_account_snapshot(snapshot, "InitialSnapshot")
                copied = model_pb2.AccountBlotterSnapshot()
                copied.CopyFrom(snapshot)
                self._snapshots[account_id] = copied
                self._allocations[account_id] = []
                if snapshot.snapshot_seq > max_snap_seq:
                    max_snap_seq = snapshot.snapshot_seq
        if initial_cursor_seq is not None:
            if initial_cursor_seq < 0:
                raise ValueError("initial_cursor_seq must be non-negative")
            self._cursor_seq = initial_cursor_seq
        else:
            self._cursor_seq = max_snap_seq
        self._cursor_sink = cursor_sink

    @property
    def cursor_seq(self) -> int:
        return self._cursor_seq

    @property
    def snapshots(self) -> dict[str, model_pb2.AccountBlotterSnapshot]:
        return {acct_id: _clone_snapshot(snap) for acct_id, snap in self._snapshots.items()}

    def snapshot_for(self, account_id: str) -> model_pb2.AccountBlotterSnapshot | None:
        snap = self._snapshots.get(account_id)
        return _clone_snapshot(snap) if snap is not None else None

    def allocations_for(self, account_id: str) -> list[model_pb2.ReconcileAllocationView]:
        return [_clone_allocation(a) for a in self._allocations.get(account_id, ())]

    def apply_envelope(self, envelope: model_pb2.BlotterEventEnvelope) -> int:
        """Validates and atomically applies one complete transaction envelope."""
        _validate_strictly_increasing_seq(
            envelope.seq, self._cursor_seq, "BlotterEventEnvelope"
        )
        staged_snaps, staged_allocs = _stage_envelope(envelope, "BlotterEventEnvelope")

        # Atomic commit of all account changes in the transaction.
        for account_id, snap in staged_snaps.items():
            self._snapshots[account_id] = snap
            self._allocations.setdefault(account_id, []).extend(staged_allocs[account_id])
        self._cursor_seq = envelope.seq
        if self._cursor_sink is not None:
            self._cursor_sink(self._cursor_seq)
        return self._cursor_seq

    def apply_poll_batch(self, batch: model_pb2.PollEventsBatch) -> int:
        """Validates an entire PollEventsBatch before applying and persisting cursor."""
        if batch.scanned_through_seq < self._cursor_seq:
            raise RuntimeError(
                "PollEvents batch failed seq monotonicity validation: "
                f"scanned_through_seq={batch.scanned_through_seq} < cursor={self._cursor_seq}"
            )
        staged_txs: list[
            tuple[
                int,
                dict[str, model_pb2.AccountBlotterSnapshot],
                dict[str, list[model_pb2.ReconcileAllocationView]],
            ]
        ] = []
        running_seq = self._cursor_seq
        for envelope in batch.events:
            _validate_strictly_increasing_seq(envelope.seq, running_seq, "PollEvents")
            staged_snaps, staged_allocs = _stage_envelope(envelope, "PollEvents")
            staged_txs.append((envelope.seq, staged_snaps, staged_allocs))
            running_seq = envelope.seq

        if batch.events and batch.scanned_through_seq < running_seq:
            raise RuntimeError(
                "PollEvents batch failed scanned_through_seq validation: "
                f"scanned_through_seq={batch.scanned_through_seq} < last_event_seq={running_seq}"
            )

        # All envelopes in the batch validated cleanly; commit state then cursor.
        for _seq, staged_snaps, staged_allocs in staged_txs:
            for account_id, snap in staged_snaps.items():
                self._snapshots[account_id] = snap
                self._allocations.setdefault(account_id, []).extend(staged_allocs[account_id])

        new_cursor = max(running_seq, batch.scanned_through_seq)
        if new_cursor != self._cursor_seq:
            self._cursor_seq = new_cursor
            if self._cursor_sink is not None:
                self._cursor_sink(self._cursor_seq)
        return self._cursor_seq

    def apply_heartbeat(self, heartbeat: model_pb2.StreamHeartbeat) -> int:
        """Validates a stream heartbeat and advances the scanned cursor if higher."""
        if heartbeat.scanned_through_seq < self._cursor_seq:
            raise RuntimeError(
                "SubscribeEvents heartbeat failed seq monotonicity validation: "
                f"scanned_through_seq={heartbeat.scanned_through_seq} < cursor={self._cursor_seq}"
            )
        if heartbeat.scanned_through_seq > self._cursor_seq:
            self._cursor_seq = heartbeat.scanned_through_seq
            if self._cursor_sink is not None:
                self._cursor_sink(self._cursor_seq)
        return self._cursor_seq


class BlotterEventStream:
    """Validated iterator over ``SubscribeEvents`` server stream."""

    def __init__(
        self,
        call: grpc.Call,
        initial_after_seq: int,
        applier: BlotterEventApplier | None,
        on_snapshot: Callable[[model_pb2.AccountBlotterSnapshot], None] | None,
    ) -> None:
        self._call = call
        self._last_seq = (
            applier.cursor_seq if applier is not None else initial_after_seq
        )
        self._applier = applier
        self._on_snapshot = on_snapshot

    def cancel(self) -> None:
        self._call.cancel()

    def close(self) -> None:
        self.cancel()

    def __enter__(self) -> BlotterEventStream:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def __iter__(self) -> Iterator[service_pb2.SubscribeEventsResponse]:
        return self

    def __next__(self) -> service_pb2.SubscribeEventsResponse:
        response: service_pb2.SubscribeEventsResponse = next(self._call)
        branch = response.WhichOneof("payload")
        if branch == "error":
            raise BlotterError(response.error)
        if branch == "snapshot_fallback":
            return response
        if branch == "heartbeat":
            hb = response.heartbeat
            if hb.scanned_through_seq < self._last_seq:
                raise RuntimeError(
                    "SubscribeEvents heartbeat failed seq monotonicity validation: "
                    f"scanned_through_seq={hb.scanned_through_seq} < last_seq={self._last_seq}"
                )
            if self._applier is not None:
                self._applier.apply_heartbeat(hb)
            self._last_seq = max(self._last_seq, hb.scanned_through_seq)
            return response
        if branch == "event":
            env = response.event
            _validate_strictly_increasing_seq(env.seq, self._last_seq, "SubscribeEvents")
            if self._applier is not None:
                self._applier.apply_envelope(env)
            else:
                _stage_envelope(env, "SubscribeEvents")
            if self._on_snapshot is not None:
                for change in env.changes:
                    if change.HasField("replace_snapshot"):
                        self._on_snapshot(change.replace_snapshot)
            self._last_seq = env.seq
            return response
        raise RuntimeError("ficant-server returned an incomplete SubscribeEvents payload")


class MarketQuoteStream:
    """Validated iterator over ``StreamMarketQuotes`` SPI server stream."""

    def __init__(
        self,
        call: grpc.Call,
        expected_cursor: spi_pb2.QuoteCursor,
    ) -> None:
        self._call = call
        self._expected_source_id = expected_cursor.source_id
        self._expected_instrument_code = expected_cursor.instrument_code
        self._last_seq = expected_cursor.after_quote_seq

    def cancel(self) -> None:
        self._call.cancel()

    def close(self) -> None:
        self.cancel()

    def __enter__(self) -> MarketQuoteStream:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def __iter__(self) -> Iterator[spi_pb2.StreamMarketQuotesResponse]:
        return self

    def __next__(self) -> spi_pb2.StreamMarketQuotesResponse:
        response: spi_pb2.StreamMarketQuotesResponse = next(self._call)
        branch = response.WhichOneof("payload")
        if branch == "error":
            raise BlotterError(response.error)
        if branch == "tick":
            tick = response.tick
            _validate_quote_identity(
                tick.source_id,
                tick.instrument_code,
                self._expected_source_id,
                self._expected_instrument_code,
                "StreamMarketQuotes",
            )
            _validate_strictly_increasing_seq(
                tick.quote_seq, self._last_seq, "StreamMarketQuotes"
            )
            self._last_seq = tick.quote_seq
            return response
        if branch == "heartbeat":
            hb = response.heartbeat
            if not hb.HasField("cursor"):
                raise RuntimeError(
                    "StreamMarketQuotes heartbeat missing cursor"
                )
            _validate_quote_identity(
                hb.cursor.source_id,
                hb.cursor.instrument_code,
                self._expected_source_id,
                self._expected_instrument_code,
                "StreamMarketQuotes",
            )
            if hb.cursor.after_quote_seq < self._last_seq:
                raise RuntimeError(
                    "StreamMarketQuotes heartbeat failed quote_seq monotonicity: "
                    f"after_quote_seq={hb.cursor.after_quote_seq} < last_seq={self._last_seq}"
                )
            self._last_seq = hb.cursor.after_quote_seq
            return response
        raise RuntimeError("ficant-server returned an incomplete StreamMarketQuotes payload")


class TradeFeedStream:
    """Validated iterator over ``StreamTradeFeed`` SPI server stream."""

    def __init__(self, call: grpc.Call, expected_source_id: str, initial_after_seq: int) -> None:
        self._call = call
        self._expected_source_id = expected_source_id
        self._last_seq = initial_after_seq

    def cancel(self) -> None:
        self._call.cancel()

    def close(self) -> None:
        self.cancel()

    def __enter__(self) -> TradeFeedStream:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def __iter__(self) -> Iterator[spi_pb2.StreamTradeFeedResponse]:
        return self

    def __next__(self) -> spi_pb2.StreamTradeFeedResponse:
        response: spi_pb2.StreamTradeFeedResponse = next(self._call)
        branch = response.WhichOneof("payload")
        if branch == "error":
            raise BlotterError(response.error)
        if branch == "fill":
            if response.fill.source_id != self._expected_source_id:
                raise RuntimeError(
                    "StreamTradeFeed cross-source fill mismatch: "
                    f"expected {self._expected_source_id!r}, got {response.fill.source_id!r}"
                )
            _validate_strictly_increasing_seq(
                response.fill.feed_seq, self._last_seq, "StreamTradeFeed"
            )
            self._last_seq = response.fill.feed_seq
            return response
        if branch == "heartbeat":
            if response.heartbeat.source_id != self._expected_source_id:
                raise RuntimeError(
                    "StreamTradeFeed cross-source heartbeat mismatch: "
                    f"expected {self._expected_source_id!r}, got {response.heartbeat.source_id!r}"
                )
            if response.heartbeat.contiguous_feed_seq < self._last_seq:
                raise RuntimeError(
                    "StreamTradeFeed heartbeat regressed contiguous_feed_seq: "
                    f"{response.heartbeat.contiguous_feed_seq} < {self._last_seq}"
                )
            self._last_seq = response.heartbeat.contiguous_feed_seq
            return response
        raise RuntimeError("ficant-server returned an incomplete StreamTradeFeed payload")


class DeskBlotterClient:
    """Synchronous gRPC client for ``DeskBlotterService`` and the four blotter SPIs."""

    def __init__(
        self,
        endpoint: str,
        bearer_token: str | None = None,
        *,
        x_ficant_credential: str | None = None,
        header_mode: str | None = None,
        expected_ledger: tuple[int, str] | None = None,
        insecure: bool = False,
        timeout_seconds: float = 30.0,
        root_certificates: bytes | None = None,
    ) -> None:
        if not endpoint:
            raise ValueError("endpoint is required")
        if timeout_seconds <= 0:
            raise ValueError("timeout_seconds must be positive")
        if header_mode is not None and header_mode not in (
            "bearer",
            "x-ficant-credential",
        ):
            raise ValueError(
                "header_mode must be 'bearer' or 'x-ficant-credential'"
            )
        if bearer_token is not None:
            _validate_raw_token(bearer_token, "bearer_token")
        if x_ficant_credential is not None:
            _validate_raw_token(x_ficant_credential, "x_ficant_credential")
        if (
            bearer_token is not None
            and x_ficant_credential is not None
            and bearer_token != x_ficant_credential
        ):
            raise ValueError(
                "bearer_token and x_ficant_credential must match when both are set"
            )
        if bearer_token is None and x_ficant_credential is None:
            self._metadata: tuple[tuple[str, str], ...] = ()
        else:
            raw_token = (
                x_ficant_credential
                if x_ficant_credential is not None
                else bearer_token
            )
            assert raw_token is not None
            if header_mode is None:
                effective_mode = (
                    "x-ficant-credential"
                    if x_ficant_credential is not None and bearer_token is None
                    else "bearer"
                )
            else:
                if (
                    header_mode == "bearer"
                    and bearer_token is None
                    and x_ficant_credential is not None
                ):
                    raise ValueError(
                        "cannot use header_mode='bearer' when only x_ficant_credential is provided"
                    )
                effective_mode = header_mode
            if effective_mode == "bearer":
                self._metadata = ((AUTHORIZATION_HEADER, f"Bearer {raw_token}"),)
            else:
                self._metadata = ((FICANT_CREDENTIAL_HEADER, raw_token),)
        self._expected_ledger: tuple[int, str] | None = (
            _validate_expected_ledger(expected_ledger)
            if expected_ledger is not None
            else None
        )
        self._timeout = timeout_seconds
        if insecure:
            self._channel: grpc.Channel = grpc.insecure_channel(endpoint)
        else:
            credentials = grpc.ssl_channel_credentials(
                root_certificates=root_certificates
            )
            self._channel = grpc.secure_channel(endpoint, credentials)
        self._stub = service_pb2_grpc.DeskBlotterServiceStub(self._channel)
        self._position_base_stub = spi_pb2_grpc.PositionBaseProviderServiceStub(
            self._channel
        )
        self._trade_feed_stub = spi_pb2_grpc.TradeFeedReconcilerServiceStub(
            self._channel
        )
        self._market_quotes_stub = spi_pb2_grpc.MarketQuoteStreamServiceStub(
            self._channel
        )
        self._nlp_stub = spi_pb2_grpc.IntranetNlpIntentParserServiceStub(
            self._channel
        )
        self._account_currency_units: dict[str, UnitRef] = {}

    @property
    def expected_ledger(self) -> tuple[int, str] | None:
        return self._expected_ledger

    def set_expected_ledger(self, expected_ledger: tuple[int, str] | None) -> None:
        self._expected_ledger = (
            _validate_expected_ledger(expected_ledger)
            if expected_ledger is not None
            else None
        )

    def close(self) -> None:
        self._channel.close()

    def __enter__(self) -> DeskBlotterClient:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    # ------------------------------------------------------------------
    # DeskBlotterService (18 RPCs)
    # ------------------------------------------------------------------

    def import_opening_base(
        self, request: service_pb2.ImportOpeningBaseRequest
    ) -> service_pb2.ImportOpeningBaseResult:
        result: service_pb2.ImportOpeningBaseResult = self._call(
            self._stub.ImportOpeningBase, request
        )
        if not result.HasField("snapshot"):
            raise RuntimeError("ImportOpeningBase response missing snapshot")
        _validate_account_snapshot(result.snapshot, "ImportOpeningBase")
        self._remember_account_units(result.snapshot)
        return result

    def ingest_market_quotes(
        self,
        request: service_pb2.IngestMarketQuotesRequest,
        *,
        expected_ledger: tuple[int, str] | None = None,
        metadata: Sequence[tuple[str, str]] | None = None,
    ) -> service_pb2.IngestMarketQuotesResult:
        result: service_pb2.IngestMarketQuotesResult = self._call(
            self._stub.IngestMarketQuotes,
            request,
            expected_ledger=expected_ledger,
            use_default_expected_ledger=True,
            extra_metadata=metadata,
        )
        if not result.HasField("risk_summary"):
            raise RuntimeError("IngestMarketQuotes response missing risk_summary")
        _validate_risk_summary(result.risk_summary, "IngestMarketQuotes")
        return result

    def create_hold(
        self, request: service_pb2.CreateHoldRequest
    ) -> service_pb2.CreateHoldResult:
        result: service_pb2.CreateHoldResult = self._call(
            self._stub.CreateHold, request
        )
        if not result.HasField("position"):
            raise RuntimeError("CreateHold response missing position")
        _validate_position_view(result.position, "CreateHold")
        return result

    def extend_hold(
        self, request: service_pb2.ExtendHoldRequest
    ) -> service_pb2.ExtendHoldResult:
        return self._call(self._stub.ExtendHold, request)

    def confirm_hold(
        self, request: service_pb2.ConfirmHoldRequest
    ) -> service_pb2.ConfirmHoldResult:
        result: service_pb2.ConfirmHoldResult = self._call(
            self._stub.ConfirmHold, request
        )
        if result.HasField("position"):
            _validate_position_view(result.position, "ConfirmHold")
        if result.HasField("risk_summary"):
            _validate_risk_summary(result.risk_summary, "ConfirmHold")
        if result.HasField("reconfirmation_error"):
            if (
                not result.HasField("earmark")
                or result.earmark.state != model_pb2.EARMARK_STATE_EXPIRED
            ):
                raise RuntimeError(
                    "ConfirmHold response with reconfirmation_error must carry EXPIRED earmark"
                )
            if result.committed_seq == 0:
                raise RuntimeError(
                    "ConfirmHold response with reconfirmation_error must carry positive committed_seq"
                )
            raise BlotterError(
                result.reconfirmation_error,
                committed_seq=result.committed_seq,
                replayed=result.replayed,
                earmark=result.earmark,
                position=result.position if result.HasField("position") else None,
                risk_summary=(
                    result.risk_summary if result.HasField("risk_summary") else None
                ),
                funding_summary=(
                    result.funding_summary
                    if result.HasField("funding_summary")
                    else None
                ),
            )
        return result

    def release_hold(
        self, request: service_pb2.ReleaseHoldRequest
    ) -> service_pb2.ReleaseHoldResult:
        result: service_pb2.ReleaseHoldResult = self._call(
            self._stub.ReleaseHold, request
        )
        if not result.HasField("position"):
            raise RuntimeError("ReleaseHold response missing position")
        _validate_position_view(result.position, "ReleaseHold")
        return result

    def create_committed(
        self, request: service_pb2.CreateCommittedRequest
    ) -> service_pb2.CreateCommittedResult:
        result: service_pb2.CreateCommittedResult = self._call(
            self._stub.CreateCommitted, request
        )
        if not result.HasField("position"):
            raise RuntimeError("CreateCommitted response missing position")
        if not result.HasField("risk_summary"):
            raise RuntimeError("CreateCommitted response missing risk_summary")
        _validate_position_view(result.position, "CreateCommitted")
        _validate_risk_summary(result.risk_summary, "CreateCommitted")
        return result

    def cancel_committed(
        self, request: service_pb2.CancelCommittedRequest
    ) -> service_pb2.CancelCommittedResult:
        result: service_pb2.CancelCommittedResult = self._call(
            self._stub.CancelCommitted, request
        )
        if not result.HasField("position"):
            raise RuntimeError("CancelCommitted response missing position")
        if not result.HasField("risk_summary"):
            raise RuntimeError("CancelCommitted response missing risk_summary")
        _validate_position_view(result.position, "CancelCommitted")
        _validate_risk_summary(result.risk_summary, "CancelCommitted")
        return result

    def submit_draft_earmark(
        self,
        request: service_pb2.SubmitDraftEarmarkRequest,
        *,
        expected_ledger: tuple[int, str] | None = None,
        metadata: Sequence[tuple[str, str]] | None = None,
    ) -> service_pb2.SubmitDraftEarmarkResult:
        return self._call(
            self._stub.SubmitDraftEarmark,
            request,
            expected_ledger=expected_ledger,
            use_default_expected_ledger=True,
            extra_metadata=metadata,
        )

    def confirm_draft_earmark(
        self, request: service_pb2.ConfirmDraftEarmarkRequest
    ) -> service_pb2.ConfirmDraftEarmarkResult:
        result: service_pb2.ConfirmDraftEarmarkResult = self._call(
            self._stub.ConfirmDraftEarmark, request
        )
        if not result.HasField("position"):
            raise RuntimeError("ConfirmDraftEarmark response missing position")
        if not result.HasField("risk_summary"):
            raise RuntimeError("ConfirmDraftEarmark response missing risk_summary")
        _validate_position_view(result.position, "ConfirmDraftEarmark")
        _validate_risk_summary(result.risk_summary, "ConfirmDraftEarmark")
        return result

    def ingest_trade_feed(
        self,
        request: service_pb2.IngestTradeFeedRequest,
        *,
        expected_ledger: tuple[int, str] | None = None,
        metadata: Sequence[tuple[str, str]] | None = None,
    ) -> service_pb2.IngestTradeFeedResult:
        return self._call(
            self._stub.IngestTradeFeed,
            request,
            expected_ledger=expected_ledger,
            use_default_expected_ledger=True,
            extra_metadata=metadata,
        )

    def resolve_pending_match(
        self, request: service_pb2.ResolvePendingMatchRequest
    ) -> service_pb2.ResolvePendingMatchResult:
        result: service_pb2.ResolvePendingMatchResult = self._call(
            self._stub.ResolvePendingMatch, request
        )
        if not result.HasField("position"):
            raise RuntimeError("ResolvePendingMatch response missing position")
        if not result.HasField("risk_summary"):
            raise RuntimeError("ResolvePendingMatch response missing risk_summary")
        _validate_position_view(result.position, "ResolvePendingMatch")
        _validate_risk_summary(result.risk_summary, "ResolvePendingMatch")
        return result

    def sweep_lifecycle(
        self, request: service_pb2.SweepLifecycleRequest
    ) -> service_pb2.SweepLifecycleResult:
        return self._call(self._stub.SweepLifecycle, request)

    def set_feed_control(
        self, request: service_pb2.SetFeedControlRequest
    ) -> service_pb2.SetFeedControlResult:
        return self._call(self._stub.SetFeedControl, request)

    def preview_covered_hedge(
        self,
        request: service_pb2.PreviewCoveredHedgeRequest,
        *,
        expected_currency_unit: UnitRef | None = None,
    ) -> service_pb2.PreviewCoveredHedgeResult:
        result: service_pb2.PreviewCoveredHedgeResult = self._call(
            self._stub.PreviewCoveredHedge, request
        )
        resolved_currency_unit = expected_currency_unit or self._account_currency_units.get(
            request.account_id
        )
        _validate_preview_covered_hedge_result(request, result, resolved_currency_unit)
        return result

    def get_account_snapshot(
        self, request: service_pb2.GetAccountSnapshotRequest
    ) -> service_pb2.GetAccountSnapshotResult:
        result: service_pb2.GetAccountSnapshotResult = self._call(
            self._stub.GetAccountSnapshot, request
        )
        if not result.HasField("snapshot"):
            raise RuntimeError("GetAccountSnapshot response missing snapshot")
        _validate_account_snapshot(result.snapshot, "GetAccountSnapshot")
        self._remember_account_units(result.snapshot)
        return result

    def poll_events(
        self,
        request: service_pb2.PollEventsRequest,
        *,
        applier: BlotterEventApplier | None = None,
    ) -> service_pb2.PollEventsResult:
        result: service_pb2.PollEventsResult = self._call(
            self._stub.PollEvents, request
        )
        branch = result.WhichOneof("outcome")
        if branch == "snapshot_fallback":
            return result
        if branch != "batch":
            raise RuntimeError("PollEvents response missing outcome branch")
        batch = result.batch
        if applier is not None:
            applier.apply_poll_batch(batch)
        else:
            if batch.scanned_through_seq < request.after_seq:
                raise RuntimeError(
                    "PollEvents batch failed seq monotonicity validation: "
                    f"scanned_through_seq={batch.scanned_through_seq} < after_seq={request.after_seq}"
                )
            running_seq = request.after_seq
            for envelope in batch.events:
                _validate_strictly_increasing_seq(
                    envelope.seq, running_seq, "PollEvents"
                )
                _stage_envelope(envelope, "PollEvents")
                running_seq = envelope.seq
            if batch.events and batch.scanned_through_seq < running_seq:
                raise RuntimeError(
                    "PollEvents batch failed scanned_through_seq validation: "
                    f"scanned_through_seq={batch.scanned_through_seq} < last_event_seq={running_seq}"
                )
        for envelope in batch.events:
            for change in envelope.changes:
                if change.HasField("replace_snapshot"):
                    self._remember_account_units(change.replace_snapshot)
        return result

    def subscribe_events(
        self,
        request: service_pb2.SubscribeEventsRequest,
        *,
        applier: BlotterEventApplier | None = None,
        timeout_seconds: float | None = None,
    ) -> BlotterEventStream:
        call = self._stub.SubscribeEvents(
            request, metadata=self._metadata, timeout=timeout_seconds
        )
        return BlotterEventStream(
            call,
            initial_after_seq=request.after_seq,
            applier=applier,
            on_snapshot=self._remember_account_units,
        )

    # ------------------------------------------------------------------
    # Four Versioned SPIs (7 RPCs)
    # ------------------------------------------------------------------

    def fetch_opening_position_base(
        self, request: spi_pb2.FetchOpeningPositionBaseRequest
    ) -> model_pb2.OpeningPositionBase:
        return self._call(self._position_base_stub.FetchOpeningPositionBase, request)

    def pull_trade_feed_batch(
        self, request: spi_pb2.PullTradeFeedBatchRequest
    ) -> spi_pb2.PullTradeFeedBatchResult:
        result: spi_pb2.PullTradeFeedBatchResult = self._call(
            self._trade_feed_stub.PullTradeFeedBatch, request
        )
        if result.scanned_through_feed_seq < request.after_feed_seq:
            raise RuntimeError(
                "PullTradeFeedBatch regressed scanned_through_feed_seq: "
                f"{result.scanned_through_feed_seq} < {request.after_feed_seq}"
            )
        running_seq = request.after_feed_seq
        for fill in result.fills:
            if fill.source_id != request.source_id:
                raise RuntimeError(
                    "PullTradeFeedBatch cross-source fill mismatch: "
                    f"expected {request.source_id!r}, got {fill.source_id!r}"
                )
            _validate_strictly_increasing_seq(
                fill.feed_seq, running_seq, "PullTradeFeedBatch"
            )
            running_seq = fill.feed_seq
        return result

    def stream_trade_feed(
        self,
        request: spi_pb2.StreamTradeFeedRequest,
        *,
        timeout_seconds: float | None = None,
    ) -> TradeFeedStream:
        call = self._trade_feed_stub.StreamTradeFeed(
            request, metadata=self._metadata, timeout=timeout_seconds
        )
        return TradeFeedStream(
            call,
            expected_source_id=request.source_id,
            initial_after_seq=request.after_feed_seq,
        )

    def fetch_trade_revision(
        self, request: spi_pb2.FetchTradeRevisionRequest
    ) -> spi_pb2.TradeFillRecord:
        result: spi_pb2.TradeFillRecord = self._call(
            self._trade_feed_stub.FetchTradeRevision, request
        )
        if result.source_id != request.source_id:
            raise RuntimeError(
                "FetchTradeRevision cross-source mismatch: "
                f"expected {request.source_id!r}, got {result.source_id!r}"
            )
        return result

    def pull_market_quotes(
        self, request: spi_pb2.PullMarketQuotesRequest
    ) -> spi_pb2.PullMarketQuotesResult:
        if not request.HasField("cursor"):
            raise RuntimeError("PullMarketQuotes request missing cursor")
        result: spi_pb2.PullMarketQuotesResult = self._call(
            self._market_quotes_stub.PullMarketQuotes, request
        )
        _validate_pull_market_quotes_result(request.cursor, result)
        return result

    def stream_market_quotes(
        self,
        request: spi_pb2.StreamMarketQuotesRequest,
        *,
        timeout_seconds: float | None = None,
    ) -> MarketQuoteStream:
        if not request.HasField("cursor"):
            raise RuntimeError("StreamMarketQuotes request missing cursor")
        call = self._market_quotes_stub.StreamMarketQuotes(
            request, metadata=self._metadata, timeout=timeout_seconds
        )
        return MarketQuoteStream(call, expected_cursor=request.cursor)

    def parse_nlp_intent(
        self, request: spi_pb2.ParseNlpIntentRequest
    ) -> spi_pb2.NlpEarmarkProposal:
        return self._call(self._nlp_stub.ParseNlpIntent, request)

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _build_metadata(
        self,
        *,
        expected_ledger: tuple[int, str] | None = None,
        use_default_expected_ledger: bool = False,
        extra_metadata: Sequence[tuple[str, str]] | None = None,
    ) -> tuple[tuple[str, str], ...]:
        merged: list[tuple[str, str]] = list(self._metadata)
        extra_pairs: list[tuple[str, str]] = []
        extra_has_ledger = False
        if extra_metadata is not None:
            for item in extra_metadata:
                if (
                    not isinstance(item, tuple)
                    or len(item) != 2
                    or not isinstance(item[0], str)
                    or not isinstance(item[1], str)
                ):
                    raise ValueError("metadata entries must be (str, str) pairs")
                key_lower = item[0].lower()
                if key_lower in (
                    EXPECTED_LEDGER_EPOCH_HEADER,
                    EXPECTED_LEDGER_INSTANCE_HEADER,
                ):
                    extra_has_ledger = True
                extra_pairs.append(item)
        if expected_ledger is not None and extra_has_ledger:
            raise ValueError(
                "cannot pass both expected_ledger and explicit x-ficant-expected-ledger-* metadata"
            )
        effective_ledger = (
            expected_ledger
            if expected_ledger is not None
            else (
                self._expected_ledger
                if (use_default_expected_ledger and not extra_has_ledger)
                else None
            )
        )
        if effective_ledger is not None:
            merged.extend(_expected_ledger_metadata(effective_ledger))
        merged.extend(extra_pairs)
        return tuple(merged)

    def _call(
        self,
        operation: Callable[..., _Response],
        request: object,
        *,
        expected_ledger: tuple[int, str] | None = None,
        use_default_expected_ledger: bool = False,
        extra_metadata: Sequence[tuple[str, str]] | None = None,
    ) -> _Result:
        call_metadata = self._build_metadata(
            expected_ledger=expected_ledger,
            use_default_expected_ledger=use_default_expected_ledger,
            extra_metadata=extra_metadata,
        )
        response = operation(request, metadata=call_metadata, timeout=self._timeout)
        branch = response.WhichOneof("result")
        if branch == "error":
            raise BlotterError(response.error)
        if branch != "value":
            raise RuntimeError("ficant-server returned an incomplete blotter response")
        return response.value

    def _remember_account_units(
        self, snapshot: model_pb2.AccountBlotterSnapshot
    ) -> None:
        if (
            snapshot.account_id
            and snapshot.HasField("opening_base")
            and snapshot.opening_base.HasField("units")
            and snapshot.opening_base.units.HasField("currency_amount")
            and snapshot.opening_base.units.currency_amount.HasField("unit_id")
            and snapshot.opening_base.units.currency_amount.unit_id.value
            and snapshot.opening_base.units.currency_amount.version >= 1
        ):
            unit_copy = UnitRef()
            unit_copy.CopyFrom(snapshot.opening_base.units.currency_amount)
            self._account_currency_units[snapshot.account_id] = unit_copy


# ======================================================================
# Client-Side Fail-Closed Validation & Tamper Guards
# ======================================================================


def _clone_snapshot(
    snapshot: model_pb2.AccountBlotterSnapshot,
) -> model_pb2.AccountBlotterSnapshot:
    out = model_pb2.AccountBlotterSnapshot()
    out.CopyFrom(snapshot)
    return out


def _clone_allocation(
    alloc: model_pb2.ReconcileAllocationView,
) -> model_pb2.ReconcileAllocationView:
    out = model_pb2.ReconcileAllocationView()
    out.CopyFrom(alloc)
    return out


def _validate_strictly_increasing_seq(
    actual_seq: int, previous_seq: int, rpc_name: str
) -> None:
    if actual_seq == previous_seq:
        raise RuntimeError(
            f"{rpc_name} failed seq monotonicity validation: "
            f"duplicate equal seq={actual_seq}"
        )
    if actual_seq < previous_seq:
        raise RuntimeError(
            f"{rpc_name} failed seq monotonicity validation: "
            f"regressing seq={actual_seq} < previous={previous_seq}"
        )


def _validate_quote_identity(
    actual_source_id: str,
    actual_instrument_code: str,
    expected_source_id: str,
    expected_instrument_code: str,
    rpc_name: str,
) -> None:
    if actual_source_id != expected_source_id:
        raise RuntimeError(
            f"{rpc_name} failed quote cursor validation: cross-source mismatch "
            f"expected={expected_source_id!r}, got={actual_source_id!r}"
        )
    if actual_instrument_code != expected_instrument_code:
        raise RuntimeError(
            f"{rpc_name} failed quote cursor validation: cross-instrument mismatch "
            f"expected={expected_instrument_code!r}, got={actual_instrument_code!r}"
        )


def _validate_pull_market_quotes_result(
    request_cursor: spi_pb2.QuoteCursor,
    result: spi_pb2.PullMarketQuotesResult,
) -> None:
    if not result.HasField("next_cursor"):
        raise RuntimeError("PullMarketQuotes response missing next_cursor")
    _validate_quote_identity(
        result.next_cursor.source_id,
        result.next_cursor.instrument_code,
        request_cursor.source_id,
        request_cursor.instrument_code,
        "PullMarketQuotes",
    )
    if result.next_cursor.after_quote_seq < request_cursor.after_quote_seq:
        raise RuntimeError(
            "PullMarketQuotes response failed next_cursor validation: "
            f"regressing after_quote_seq={result.next_cursor.after_quote_seq} "
            f"< request={request_cursor.after_quote_seq}"
        )
    running_seq = request_cursor.after_quote_seq
    for tick in result.ticks:
        _validate_quote_identity(
            tick.source_id,
            tick.instrument_code,
            request_cursor.source_id,
            request_cursor.instrument_code,
            "PullMarketQuotes",
        )
        _validate_strictly_increasing_seq(
            tick.quote_seq, running_seq, "PullMarketQuotes"
        )
        running_seq = tick.quote_seq
    expected_next_seq = (
        result.ticks[-1].quote_seq if result.ticks else request_cursor.after_quote_seq
    )
    if result.next_cursor.after_quote_seq != expected_next_seq:
        raise RuntimeError(
            "PullMarketQuotes response failed next_cursor.after_quote_seq validation: "
            f"expected {expected_next_seq}, got {result.next_cursor.after_quote_seq}"
        )


def _validate_position_view(
    position: model_pb2.InstrumentPositionView, rpc_name: str
) -> None:
    if position.tax_basis != analytics_pb2.BOND_TAX_BASIS_PRE_TAX:
        raise RuntimeError(
            f"{rpc_name} response failed tax_basis validation: "
            f"expected BOND_TAX_BASIS_PRE_TAX, got {position.tax_basis} "
            f"for instrument={position.instrument_code!r}"
        )


def _validate_risk_summary(
    summary: model_pb2.RiskAndCoverageSummary, rpc_name: str
) -> None:
    if summary.covered_tax_basis != analytics_pb2.BOND_TAX_BASIS_PRE_TAX:
        raise RuntimeError(
            f"{rpc_name} response failed covered_tax_basis validation: "
            f"expected BOND_TAX_BASIS_PRE_TAX, got {summary.covered_tax_basis}"
        )
    if summary.uncovered_instrument_count > 0 and summary.whole_account_hedge_allowed:
        raise RuntimeError(
            f"{rpc_name} response failed whole_account_hedge_allowed validation: "
            f"whole_account_hedge_allowed=True with uncovered_instrument_count="
            f"{summary.uncovered_instrument_count}"
        )


def _validate_account_snapshot(
    snapshot: model_pb2.AccountBlotterSnapshot, rpc_name: str
) -> None:
    if not snapshot.account_id:
        raise RuntimeError(f"{rpc_name} response snapshot missing account_id")
    if not snapshot.HasField("risk_summary"):
        raise RuntimeError(f"{rpc_name} response snapshot missing risk_summary")
    _validate_risk_summary(snapshot.risk_summary, rpc_name)
    for position in snapshot.positions:
        _validate_position_view(position, rpc_name)


def _stage_envelope(
    envelope: model_pb2.BlotterEventEnvelope, rpc_name: str
) -> tuple[
    dict[str, model_pb2.AccountBlotterSnapshot],
    dict[str, list[model_pb2.ReconcileAllocationView]],
]:
    if envelope.seq == 0:
        raise RuntimeError(f"{rpc_name} event envelope has invalid seq=0")
    staged_snaps: dict[str, model_pb2.AccountBlotterSnapshot] = {}
    staged_allocs: dict[str, list[model_pb2.ReconcileAllocationView]] = {}
    for change in envelope.changes:
        if not change.account_id:
            raise RuntimeError(f"{rpc_name} AccountChange missing account_id")
        if change.account_id in staged_snaps:
            raise RuntimeError(
                f"{rpc_name} duplicate AccountChange for account_id={change.account_id!r} "
                f"in seq={envelope.seq}"
            )
        if not change.HasField("replace_snapshot"):
            raise RuntimeError(
                f"{rpc_name} AccountChange missing replace_snapshot for account_id={change.account_id!r}"
            )
        snap = change.replace_snapshot
        if snap.account_id != change.account_id:
            raise RuntimeError(
                f"{rpc_name} AccountChange account_id mismatch: "
                f"change={change.account_id!r}, snapshot={snap.account_id!r}"
            )
        if snap.snapshot_seq != envelope.seq:
            raise RuntimeError(
                f"{rpc_name} AccountChange snapshot_seq mismatch: "
                f"envelope.seq={envelope.seq}, snapshot_seq={snap.snapshot_seq}"
            )
        _validate_account_snapshot(snap, rpc_name)
        staged_snaps[change.account_id] = _clone_snapshot(snap)
        staged_allocs[change.account_id] = [
            _clone_allocation(a) for a in change.changed_allocations
        ]
    return staged_snaps, staged_allocs


def _validate_positive_canonical_decimal(
    value: DecimalValue,
    rpc_name: str,
    field_name: str,
) -> None:
    coeff = value.coefficient
    scale = value.scale
    if not isinstance(coeff, str) or not _CANONICAL_POSITIVE_INT_RE.fullmatch(coeff):
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"non-canonical or non-positive coefficient={coeff!r}"
        )
    if len(coeff) > _MAX_DECIMAL_PRECISION:
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"coefficient precision exceeds {_MAX_DECIMAL_PRECISION}"
        )
    if (
        isinstance(scale, bool)
        or not isinstance(scale, int)
        or scale < 0
        or scale > _MAX_DECIMAL_SCALE
    ):
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"invalid scale={scale!r}"
        )
    if scale > 0 and coeff.endswith("0"):
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"non-canonical trailing zero in coefficient={coeff!r} with scale={scale}"
        )
    try:
        decoded = Decimal(f"{coeff}e-{scale}")
    except InvalidOperation as exc:
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"invalid decimal representation ({coeff!r}, scale={scale})"
        ) from exc
    if not decoded.is_finite() or decoded <= 0:
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"value must be finite and strictly positive, got {decoded}"
        )


def _units_equal(lhs: UnitRef, rhs: UnitRef) -> bool:
    return (
        lhs.HasField("unit_id")
        and rhs.HasField("unit_id")
        and lhs.unit_id.value == rhs.unit_id.value
        and lhs.version == rhs.version
    )


def _validate_unit_ref_present(unit: UnitRef, rpc_name: str, field_name: str) -> None:
    if (
        not unit.HasField("unit_id")
        or not unit.unit_id.value
        or unit.unit_id.value != unit.unit_id.value.strip()
        or unit.version < 1
    ):
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"missing or invalid unit ({unit.unit_id.value!r}, v{unit.version})"
        )


def _validate_preview_covered_hedge_result(
    request: service_pb2.PreviewCoveredHedgeRequest,
    result: service_pb2.PreviewCoveredHedgeResult,
    expected_currency_unit: UnitRef | None,
) -> None:
    rpc_name = "PreviewCoveredHedge"
    if request.scope not in _VALID_HEDGE_SCOPES:
        raise RuntimeError(
            f"{rpc_name} response failed scope validation: "
            f"unsupported request scope={request.scope}"
        )
    if result.scope not in _VALID_HEDGE_SCOPES:
        raise RuntimeError(
            f"{rpc_name} response failed scope validation: "
            f"unsupported response scope={result.scope}"
        )
    if result.scope != request.scope:
        raise RuntimeError(
            f"{rpc_name} response failed scope validation: "
            f"expected {request.scope}, got {result.scope}"
        )
    if (
        result.uncovered_instrument_count > 0
        and result.scope == model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT
    ):
        raise RuntimeError(
            f"{rpc_name} response failed scope validation: "
            f"WHOLE_ACCOUNT returned while uncovered_instrument_count="
            f"{result.uncovered_instrument_count}"
        )
    if result.covered_tax_basis != analytics_pb2.BOND_TAX_BASIS_PRE_TAX:
        raise RuntimeError(
            f"{rpc_name} response failed covered_tax_basis validation: "
            f"expected BOND_TAX_BASIS_PRE_TAX, got {result.covered_tax_basis}"
        )
    if not result.HasField("measures"):
        raise RuntimeError(
            f"{rpc_name} response failed measures validation: missing measures"
        )
    if not result.measures.HasField("contract_notional"):
        raise RuntimeError(
            f"{rpc_name} response failed contract_notional validation: "
            "missing measures.contract_notional"
        )
    notional = result.measures.contract_notional
    _validate_positive_canonical_decimal(notional, rpc_name, "contract_notional")
    if not notional.HasField("unit"):
        raise RuntimeError(
            f"{rpc_name} response failed contract_notional.unit validation: missing unit"
        )
    _validate_unit_ref_present(notional.unit, rpc_name, "contract_notional.unit")

    # contract_notional MUST be denominated in the account's currency_amount unit,
    # never a percentage / dimensionless / DV01 / contract_count unit.
    if expected_currency_unit is not None and not _units_equal(
        notional.unit, expected_currency_unit
    ):
        raise RuntimeError(
            f"{rpc_name} response failed contract_notional.unit validation: "
            f"expected ({expected_currency_unit.unit_id.value!r}, v{expected_currency_unit.version}), "
            f"got ({notional.unit.unit_id.value!r}, v{notional.unit.version})"
        )
    if (
        result.HasField("excluded_uncovered_gross_notional")
        and result.excluded_uncovered_gross_notional.HasField("unit")
    ):
        excl_unit = result.excluded_uncovered_gross_notional.unit
        _validate_unit_ref_present(
            excl_unit, rpc_name, "excluded_uncovered_gross_notional.unit"
        )
        if not _units_equal(notional.unit, excl_unit):
            raise RuntimeError(
                f"{rpc_name} response failed contract_notional.unit validation: "
                "contract_notional unit does not match excluded_uncovered_gross_notional currency unit"
            )
    for forbidden_label, forbidden_dec in (
        (
            "conversion_factor",
            request.conversion_factor if request.HasField("conversion_factor") else None,
        ),
        (
            "ctd_dv01_per_100",
            request.ctd_dv01_per_100 if request.HasField("ctd_dv01_per_100") else None,
        ),
        (
            "hedge_effectiveness",
            result.measures.hedge_effectiveness
            if result.measures.HasField("hedge_effectiveness")
            else None,
        ),
        (
            "futures_contract_dv01",
            result.measures.futures_contract_dv01
            if result.measures.HasField("futures_contract_dv01")
            else None,
        ),
        (
            "raw_contracts",
            result.measures.raw_contracts
            if result.measures.HasField("raw_contracts")
            else None,
        ),
    ):
        if (
            forbidden_dec is not None
            and forbidden_dec.HasField("unit")
            and _units_equal(notional.unit, forbidden_dec.unit)
        ):
            raise RuntimeError(
                f"{rpc_name} response failed contract_notional.unit validation: "
                f"contract_notional carries non-currency {forbidden_label} unit"
            )
