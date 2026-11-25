"""Pure unit & mock tests for BP-4 sidecar runtime, QuoteStreamSidecar (F-13), and TradeFeedSidecar (F-07)."""

from __future__ import annotations

from collections.abc import Sequence
from datetime import datetime, timezone
from decimal import Decimal
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
from pathlib import Path
import struct
import sys
import threading
from typing import Any

import grpc
import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
GENERATED_ROOT = (
    REPO_ROOT
    / "python"
    / "node-contracts"
    / "src"
    / "ficant_contracts"
    / "generated"
)
SDK_ROOT = REPO_ROOT / "python" / "sdk" / "src"

if str(GENERATED_ROOT) not in sys.path:
    sys.path.insert(0, str(GENERATED_ROOT))
if str(SDK_ROOT) not in sys.path:
    sys.path.insert(0, str(SDK_ROOT))

from ficant.blotter.v1 import model_pb2, service_pb2, spi_pb2  # noqa: E402
from ficant.core.v1 import common_pb2, error_pb2  # noqa: E402
from ficant.market.v1 import fact_pb2  # noqa: E402
from ficant.rates.v1 import analytics_pb2 as rates_pb2  # noqa: E402
from ficant_sdk.blotter import (  # noqa: E402
    AUTHORIZATION_HEADER,
    EXPECTED_LEDGER_EPOCH_HEADER,
    EXPECTED_LEDGER_INSTANCE_HEADER,
    FICANT_CREDENTIAL_HEADER,
    BlotterError,
    DeskBlotterClient,
)
from ficant_sdk.sidecar import (  # noqa: E402
    CHECKPOINT_SCHEMA_VERSION,
    VALID_TRADE_FILL_OUTCOME_KINDS,
    AccountUnitsResolver,
    BackoffPolicy,
    CheckpointCorruptionError,
    CheckpointLockError,
    HealthCheckError,
    HealthzStatus,
    JsonCheckpointStore,
    LedgerEpochRegressionError,
    LedgerInstanceMismatchError,
    LedgerRolloverValidationError,
    QuoteStreamConfig,
    QuoteStreamSidecar,
    SidecarError,
    TradeFeedConfig,
    TradeFeedSidecar,
    compute_file_fingerprint,
    decimal_to_pb,
    default_reference_units,
    fixed_decimal_scaled_i128,
    fixed_decimal_to_be_bytes,
    is_retryable_error,
    market_quote_canonical_payload_hash,
    market_time_timestamp_micros,
    parse_market_time,
    parse_quote_row,
    parse_trade_fill_row,
    parse_units_json,
    probe_healthz,
    reconcile_ledger_checkpoint,
    trade_fill_business_hash,
    trade_fill_payload_hash,
    validate_trade_feed_result,
)

ULID_1 = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
ULID_2 = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
ULID_3 = "01ARZ3NDEKTSV4RRFFQ69G5FAX"


def _health(
    epoch: int = 1,
    instance_id: str = ULID_1,
    *,
    status: str = "ok",
    storage_mode: str = "persistent_wal",
    blotter_healthy: bool = True,
) -> HealthzStatus:
    return HealthzStatus(
        status=status,
        service="ficant-server",
        storage_mode=storage_mode,
        blotter_healthy=blotter_healthy,
        ledger_epoch=epoch,
        ledger_instance_id=instance_id,
        web_ui_mode="embedded",
        embedded_assets_count=5,
    )


def _blotter_err(code: int, *, retryable: bool = False, msg: str = "err") -> BlotterError:
    return BlotterError(
        error_pb2.ErrorDetail(code=code, message=msg, retryable=retryable)
    )


class _FakeGrpcRpcError(grpc.RpcError):
    def __init__(self, status_code: grpc.StatusCode, details: str = "rpc error") -> None:
        super().__init__()
        self._code = status_code
        self._details = details

    def code(self) -> grpc.StatusCode:
        return self._code

    def details(self) -> str:
        return self._details


def _make_risk_summary() -> model_pb2.RiskAndCoverageSummary:
    return model_pb2.RiskAndCoverageSummary(
        covered_instrument_count=1,
        uncovered_instrument_count=0,
        covered_tax_basis=rates_pb2.BOND_TAX_BASIS_PRE_TAX,
        whole_account_hedge_allowed=True,
    )


# ======================================================================
# 1. DeskBlotterClient Auth Header Modes & Expected Ledger Metadata
# ======================================================================


class _RecordingStub:
    def __init__(self) -> None:
        self.calls: list[tuple[str, Any, tuple[tuple[str, str], ...]]] = []

    def IngestMarketQuotes(
        self,
        request: service_pb2.IngestMarketQuotesRequest,
        *,
        metadata: Sequence[tuple[str, str]] = (),
        timeout: float | None = None,
    ) -> service_pb2.IngestMarketQuotesResponse:
        self.calls.append(("IngestMarketQuotes", request, tuple(metadata)))
        return service_pb2.IngestMarketQuotesResponse(
            value=service_pb2.IngestMarketQuotesResult(
                committed_seq=1,
                replayed=False,
                applied_tick_count=len(request.ticks),
                skipped_stale_tick_count=0,
                risk_summary=_make_risk_summary(),
            )
        )

    def IngestTradeFeed(
        self,
        request: service_pb2.IngestTradeFeedRequest,
        *,
        metadata: Sequence[tuple[str, str]] = (),
        timeout: float | None = None,
    ) -> service_pb2.IngestTradeFeedResponse:
        self.calls.append(("IngestTradeFeed", request, tuple(metadata)))
        return service_pb2.IngestTradeFeedResponse(
            value=service_pb2.IngestTradeFeedResult(
                committed_seq=2,
                replayed=False,
                feed_status=model_pb2.FeedStatusView(
                    source_id=request.source_id,
                    contiguous_feed_seq=0,
                    max_seen_feed_seq=0,
                ),
            )
        )

    def SubmitDraftEarmark(
        self,
        request: service_pb2.SubmitDraftEarmarkRequest,
        *,
        metadata: Sequence[tuple[str, str]] = (),
        timeout: float | None = None,
    ) -> service_pb2.SubmitDraftEarmarkResponse:
        self.calls.append(("SubmitDraftEarmark", request, tuple(metadata)))
        return service_pb2.SubmitDraftEarmarkResponse(
            value=service_pb2.SubmitDraftEarmarkResult(committed_seq=3, replayed=False)
        )


def test_desk_blotter_client_auth_modes_and_expected_ledger_metadata() -> None:
    # Default bearer backward compatibility
    with DeskBlotterClient("127.0.0.1:50051", "secret-tok", insecure=True) as client:
        stub = _RecordingStub()
        client._stub = stub  # type: ignore[assignment]
        client.ingest_market_quotes(
            service_pb2.IngestMarketQuotesRequest(
                idempotency_key="k1", account_id="acct-1", source_id="qsrc-1"
            )
        )
        assert stub.calls[-1][2] == ((AUTHORIZATION_HEADER, "Bearer secret-tok"),)

    # x-ficant-credential parameter & client-level expected_ledger
    with DeskBlotterClient(
        "127.0.0.1:50051",
        x_ficant_credential="cred-tok",
        expected_ledger=(2, ULID_2),
        insecure=True,
    ) as client:
        stub = _RecordingStub()
        client._stub = stub  # type: ignore[assignment]
        client.ingest_market_quotes(
            service_pb2.IngestMarketQuotesRequest(
                idempotency_key="k2", account_id="acct-1", source_id="qsrc-1"
            )
        )
        client.ingest_trade_feed(
            service_pb2.IngestTradeFeedRequest(idempotency_key="k3", source_id="fsrc-1")
        )
        client.submit_draft_earmark(
            service_pb2.SubmitDraftEarmarkRequest(idempotency_key="k4", account_id="acct-1")
        )
        expected_meta = (
            (FICANT_CREDENTIAL_HEADER, "cred-tok"),
            (EXPECTED_LEDGER_EPOCH_HEADER, "2"),
            (EXPECTED_LEDGER_INSTANCE_HEADER, ULID_2),
        )
        for _rpc, _req, meta in stub.calls:
            assert meta == expected_meta

        # Per-call expected_ledger override
        client.ingest_trade_feed(
            service_pb2.IngestTradeFeedRequest(idempotency_key="k5", source_id="fsrc-1"),
            expected_ledger=(3, ULID_3),
        )
        assert stub.calls[-1][2] == (
            (FICANT_CREDENTIAL_HEADER, "cred-tok"),
            (EXPECTED_LEDGER_EPOCH_HEADER, "3"),
            (EXPECTED_LEDGER_INSTANCE_HEADER, ULID_3),
        )

    # header_mode="x-ficant-credential" with bearer_token positional argument
    with DeskBlotterClient(
        "127.0.0.1:50051",
        "same-tok",
        header_mode="x-ficant-credential",
        insecure=True,
    ) as client:
        stub = _RecordingStub()
        client._stub = stub  # type: ignore[assignment]
        client.submit_draft_earmark(
            service_pb2.SubmitDraftEarmarkRequest(idempotency_key="k6", account_id="acct-1"),
            metadata=((EXPECTED_LEDGER_EPOCH_HEADER, "1"), (EXPECTED_LEDGER_INSTANCE_HEADER, ULID_1)),
        )
        assert stub.calls[-1][2] == (
            (FICANT_CREDENTIAL_HEADER, "same-tok"),
            (EXPECTED_LEDGER_EPOCH_HEADER, "1"),
            (EXPECTED_LEDGER_INSTANCE_HEADER, ULID_1),
        )

    # Reject differing tokens, invalid tokens, invalid header_mode, invalid expected_ledger
    with pytest.raises(ValueError, match="must match"):
        DeskBlotterClient(
            "127.0.0.1:50051",
            bearer_token="tok-a",
            x_ficant_credential="tok-b",
            insecure=True,
        )
    with pytest.raises(ValueError, match="whitespace"):
        DeskBlotterClient("127.0.0.1:50051", bearer_token="bad tok", insecure=True)
    with pytest.raises(ValueError, match="non-empty"):
        DeskBlotterClient("127.0.0.1:50051", x_ficant_credential="", insecure=True)
    with pytest.raises(ValueError, match="header_mode"):
        DeskBlotterClient("127.0.0.1:50051", "tok", header_mode="basic", insecure=True)
    with pytest.raises(ValueError, match="positive uint64"):
        DeskBlotterClient(
            "127.0.0.1:50051", "tok", expected_ledger=(0, ULID_1), insecure=True
        )
    with pytest.raises(ValueError, match="positive uint64"):
        DeskBlotterClient(
            "127.0.0.1:50051", "tok", expected_ledger=(True, ULID_1), insecure=True  # type: ignore[arg-type]
        )
    with pytest.raises(ValueError, match="ULID"):
        DeskBlotterClient(
            "127.0.0.1:50051", "tok", expected_ledger=(1, "not-a-ulid"), insecure=True
        )


# ======================================================================
# 2. Codecs: decimal_to_pb, parse_market_time, Units, & Rust Hash Parity
# ======================================================================


def test_decimal_to_pb_validation_and_canonical_encoding() -> None:
    units = default_reference_units()
    u = units.price_per_100

    # Zero normalization
    for zero_in in (0, "0", "-0", "-0.0000", Decimal("0.0000000000000"), Decimal("0e-25")):
        pb = decimal_to_pb(zero_in, u)
        assert pb.coefficient == "0"
        assert pb.scale == 0
        assert fixed_decimal_scaled_i128(pb) == 0

    # Trailing fractional zero stripping and exact i128 scaling
    pb = decimal_to_pb("100.250000", u)
    assert pb.coefficient == "10025"
    assert pb.scale == 2
    assert fixed_decimal_scaled_i128(pb) == 100_250_000_000_000
    assert fixed_decimal_to_be_bytes(pb) == (100_250_000_000_000).to_bytes(16, "big", signed=True)

    pb_neg = decimal_to_pb(Decimal("-0.000000000001"), u)
    assert pb_neg.coefficient == "-1"
    assert pb_neg.scale == 12
    assert fixed_decimal_scaled_i128(pb_neg) == -1
    assert fixed_decimal_to_be_bytes(pb_neg) == (-1).to_bytes(16, "big", signed=True)

    # Trailing zeros beyond 12 digits still normalize cleanly if reduced scale <= 12
    pb_norm = decimal_to_pb("1.500000000000000", u)
    assert (pb_norm.coefficient, pb_norm.scale) == ("15", 1)

    # Rejections: bool, float, NaN/sNaN/Infinity, >28 significant digits, scale>12, i128 overflow
    for bad in (
        True,
        False,
        100.25,
        0.0,
        Decimal("NaN"),
        Decimal("sNaN"),
        Decimal("Infinity"),
        Decimal("-Infinity"),
        "NaN",
        "Infinity",
        " 100.25 ",
        "",
        "12345678901234567890123456789",  # 29 significant digits
        "1.0000000000001",  # normalized scale = 13 > 12
        "170141183460469231731687304",  # 27 digits, * 10^12 > 2^127 - 1
        "-170141183460469231731687304",  # 27 digits, * 10^12 < -2^127
        Decimal("1e27"),  # 28 digits, * 10^12 overflows i128
    ):
        with pytest.raises(ValueError):
            decimal_to_pb(bad, u)  # type: ignore[arg-type]


def test_parse_market_time_precision_and_validation() -> None:
    # Full 9-digit nanosecond precision preserved on wire, microsecond floor for hash
    mt = parse_market_time("2026-06-15T10:30:45.123456789+08:00")
    assert mt.market_timezone == "Asia/Shanghai"
    assert mt.local_trading_date == "2026-06-15"
    expected_secs = int(datetime(2026, 6, 15, 2, 30, 45, tzinfo=timezone.utc).timestamp())
    assert mt.instant.seconds == expected_secs
    assert mt.instant.nanos == 123_456_789
    # Microsecond floor truncates sub-microsecond 789ns -> 123456us
    assert market_time_timestamp_micros(mt) == expected_secs * 1_000_000 + 123_456

    # UTC 'Z' crossing into next day in Asia/Shanghai
    mt_cross = parse_market_time("2026-06-15T16:30:00.000000001Z")
    assert mt_cross.local_trading_date == "2026-06-16"
    assert mt_cross.instant.nanos == 1
    assert market_time_timestamp_micros(mt_cross) == mt_cross.instant.seconds * 1_000_000

    # Reject missing offset, >9 fractional digits, bad offset, wrong local_trading_date
    for bad_ts in (
        "2026-06-15T10:30:45",
        "2026-06-15 10:30:45+08:00",
        "2026-06-15T10:30:45.1234567890Z",
        "2026-06-15T10:30:45+25:00",
        "2026-02-30T10:30:45Z",
    ):
        with pytest.raises(ValueError):
            parse_market_time(bad_ts)

    with pytest.raises(ValueError, match="local_trading_date"):
        parse_market_time("2026-06-15T10:30:45+08:00", local_trading_date="2026-06-14")


def test_account_units_resolver_precedence_zero_snapshot_default_and_auth_fail_fast() -> None:
    ref = default_reference_units()
    assert ref.currency_amount.unit_id.value == "01ARZ3NDEKTSV4RRFFQ69G5FA1"
    assert ref.contract_count.unit_id.value == "01ARZ3NDEKTSV4RRFFQ69G5FA7"

    custom_units = parse_units_json(
        {
            "currency_amount": {"unit_id": ULID_1, "version": 2},
            "price_per_100": ULID_2,
            "rate": "01ARZ3NDEKTSV4RRFFQ69G5FA3",
            "dv01": "01ARZ3NDEKTSV4RRFFQ69G5FA4",
            "dv01_per_100": "01ARZ3NDEKTSV4RRFFQ69G5FA5",
            "dimensionless": "01ARZ3NDEKTSV4RRFFQ69G5FA6",
            "contract_count": "01ARZ3NDEKTSV4RRFFQ69G5FA7",
        }
    )

    class _SpyClient:
        def __init__(self, *, units_to_return: rates_pb2.AnalysisUnits | None = None, error: Exception | None = None) -> None:
            self.snapshot_calls: list[str] = []
            self._units = units_to_return
            self._error = error

        def get_account_snapshot(
            self, req: service_pb2.GetAccountSnapshotRequest
        ) -> service_pb2.GetAccountSnapshotResult:
            self.snapshot_calls.append(req.account_id)
            if self._error is not None:
                raise self._error
            return service_pb2.GetAccountSnapshotResult(
                snapshot=model_pb2.AccountBlotterSnapshot(
                    account_id=req.account_id,
                    opening_base=model_pb2.OpeningPositionBase(units=self._units),
                )
            )

    # 1. Default allow_snapshot_discovery=False MUST make 0 GetAccountSnapshot calls
    spy_default = _SpyClient(units_to_return=custom_units)
    resolver_default = AccountUnitsResolver(client=spy_default)
    resolved = resolver_default.resolve("acct-a")
    assert resolved == ref
    assert spy_default.snapshot_calls == []

    # 2. Explicit units take highest priority over snapshot discovery
    spy_explicit = _SpyClient(units_to_return=ref)
    resolver_explicit = AccountUnitsResolver(
        explicit_units=custom_units,
        client=spy_explicit,
        allow_snapshot_discovery=True,
    )
    assert resolver_explicit.resolve("acct-a") == custom_units
    assert spy_explicit.snapshot_calls == []

    # 3. Opt-in snapshot discovery queries once and caches
    spy_discover = _SpyClient(units_to_return=custom_units)
    resolver_discover = AccountUnitsResolver(
        client=spy_discover, allow_snapshot_discovery=True
    )
    assert resolver_discover.resolve("acct-a") == custom_units
    assert resolver_discover.resolve("acct-a") == custom_units
    assert spy_discover.snapshot_calls == ["acct-a"]

    # 4. UNAUTHENTICATED / FORBIDDEN MUST raise immediately without falling back
    for auth_code in (error_pb2.ERROR_CODE_UNAUTHENTICATED, error_pb2.ERROR_CODE_FORBIDDEN):
        spy_denied = _SpyClient(error=_blotter_err(auth_code))
        resolver_denied = AccountUnitsResolver(
            client=spy_denied, allow_snapshot_discovery=True
        )
        with pytest.raises(BlotterError) as exc_info:
            resolver_denied.resolve("acct-secret")
        assert exc_info.value.code == auth_code
        assert spy_denied.snapshot_calls == ["acct-secret"]


def test_canonical_hashes_match_rust_binary_layout_byte_for_byte() -> None:
    units = default_reference_units()

    def _r_str(s: str) -> bytes:
        b = s.encode("utf-8")
        return struct.pack(">Q", len(b)) + b

    def _r_u64(v: int) -> bytes:
        return struct.pack(">Q", v)

    def _r_fixed(scaled: int) -> bytes:
        return scaled.to_bytes(16, "big", signed=True)

    # --- MarketQuoteTick::canonical_payload_hash ---
    obs = parse_market_time("2026-06-15T10:00:00.123456999+08:00")
    rec_1 = parse_market_time("2026-06-15T10:00:01.000000000+08:00")
    rec_2 = parse_market_time("2026-06-15T10:00:05.987654321+08:00")
    tick1 = spi_pb2.MarketQuoteTick(
        source_id="qsrc-alpha",
        quote_seq=42,
        instrument_code="BOND_10Y",
        source_type=fact_pb2.PRICE_SOURCE_TYPE_ACTIVE_QUOTE,
        bid_clean_price=decimal_to_pb("100.125", units.price_per_100),
        ask_clean_price=decimal_to_pb("100.375", units.price_per_100),
        mid_yield_to_maturity=decimal_to_pb("0.0245", units.rate),
        observed_at=obs,
        received_at=rec_1,
    )
    tick2 = spi_pb2.MarketQuoteTick()
    tick2.CopyFrom(tick1)
    tick2.received_at.CopyFrom(rec_2)

    expected_obs_micros = obs.instant.seconds * 1_000_000 + 123_456
    rust_quote_buf = (
        _r_str("qsrc-alpha")
        + _r_str("BOND_10Y")
        + _r_u64(42)
        + bytes([2])  # ActiveQuote
        + bytes([1]) + _r_fixed(100_125_000_000_000)  # bid
        + bytes([1]) + _r_fixed(100_375_000_000_000)  # ask
        + bytes([0])  # mid = None
        + bytes([1]) + _r_fixed(24_500_000_000)  # ytm
        + struct.pack(">q", expected_obs_micros)
        + _r_str("Asia/Shanghai")
        + _r_str("2026-06-15")
    )
    expected_quote_digest = hashlib.sha256(rust_quote_buf).digest()
    assert market_quote_canonical_payload_hash(tick1) == expected_quote_digest
    # Changing only received_at yields the exact same canonical hash
    assert market_quote_canonical_payload_hash(tick2) == expected_quote_digest
    # Wire Timestamp preserves full 9-digit nanos
    assert tick1.observed_at.instant.nanos == 123_456_999

    # --- TradeFillRecord::business_hash & payload_hash ---
    exec_mt = parse_market_time("2026-06-15T11:15:30.654321987+08:00")
    fill = spi_pb2.TradeFillRecord(
        source_id="fsrc-main",
        external_trade_id="EXT-9001",
        source_revision=2,
        feed_seq=17,
        action=model_pb2.TRADE_FEED_RECORD_ACTION_AMEND,
        account_id="acct-desk-1",
        instrument_code="BOND_10Y",
        side=model_pb2.TRADE_SIDE_SELL,
        settlement_date="2026-06-16",
        notional=decimal_to_pb("10000000", units.currency_amount),
        quote_terms=model_pb2.QuoteTerms(
            clean_price_per_100=decimal_to_pb("99.85", units.price_per_100),
            settlement_amount=decimal_to_pb("10012345.67", units.currency_amount),
        ),
        executed_at=exec_mt,
        batch_first_feed_seq=15,
        batch_last_feed_seq=20,
    )
    exec_micros = exec_mt.instant.seconds * 1_000_000 + 654_321
    rust_biz_buf = (
        _r_str("fsrc-main")
        + _r_str("EXT-9001")
        + _r_u64(2)
        + bytes([2])  # Amend
        + _r_str("acct-desk-1")
        + _r_str("BOND_10Y")
        + bytes([2])  # Sell
        + _r_str("2026-06-16")
        + _r_fixed(10_000_000_000_000_000_000)
        + bytes([1, 1]) + _r_fixed(99_850_000_000_000)
        + bytes([1]) + _r_fixed(10_012_345_670_000_000_000)
        + struct.pack(">q", exec_micros)
        + _r_str("Asia/Shanghai")
        + _r_str("2026-06-15")
        + bytes([0])  # exact_dirty_price_input = None
    )
    expected_biz_digest = hashlib.sha256(rust_biz_buf).digest()
    assert trade_fill_business_hash(fill) == expected_biz_digest

    rust_payload_buf = (
        _r_u64(17)
        + _r_u64(15)
        + _r_u64(20)
        + _r_u64(32)
        + expected_biz_digest
    )
    expected_payload_digest = hashlib.sha256(rust_payload_buf).digest()
    assert trade_fill_payload_hash(fill) == expected_payload_digest

    # Changing feed_seq / batch bounds changes payload_hash but NOT business_hash
    fill_alias = spi_pb2.TradeFillRecord()
    fill_alias.CopyFrom(fill)
    fill_alias.feed_seq = 21
    fill_alias.batch_first_feed_seq = 21
    fill_alias.batch_last_feed_seq = 21
    assert trade_fill_business_hash(fill_alias) == expected_biz_digest
    assert trade_fill_payload_hash(fill_alias) != expected_payload_digest


# ======================================================================
# 3. BackoffPolicy Deterministic Retries & Immediate Non-Retry Stops
# ======================================================================


def test_backoff_policy_retries_transient_and_stops_on_non_retryable() -> None:
    sleeps: list[float] = []
    ticks = [0.0]

    def fake_clock() -> float:
        ticks[0] += 1.0
        return ticks[0]

    policy = BackoffPolicy(
        max_retries=3,
        initial_delay_ms=100,
        max_delay_ms=250,
        multiplier=2.0,
        clock=fake_clock,
        sleep_fn=sleeps.append,
    )

    # Retryable errors recover within max_retries
    attempts = {"n": 0}

    def flaky_storage() -> str:
        attempts["n"] += 1
        if attempts["n"] == 1:
            raise TimeoutError("timed out")
        if attempts["n"] == 2:
            raise _blotter_err(error_pb2.ERROR_CODE_STORAGE_UNAVAILABLE, retryable=True)
        if attempts["n"] == 3:
            raise _FakeGrpcRpcError(grpc.StatusCode.UNAVAILABLE)
        return "ok"

    assert policy.execute(flaky_storage) == "ok"
    assert sleeps == [0.1, 0.2, 0.25]  # 100ms, 200ms, capped at 250ms

    # Exhausting finite max_retries raises last exception
    sleeps.clear()
    with pytest.raises(TimeoutError):
        policy.execute(lambda: (_ for _ in ()).throw(TimeoutError("still down")))
    assert sleeps == [0.1, 0.2, 0.25]

    # Non-retryable BlotterErrors stop immediately with ZERO sleeps
    for non_retryable_err in (
        _blotter_err(error_pb2.ERROR_CODE_UNAUTHENTICATED, retryable=True),
        _blotter_err(error_pb2.ERROR_CODE_FORBIDDEN, retryable=True),
        _blotter_err(error_pb2.ERROR_CODE_VALIDATION_FAILED, retryable=True),
        _blotter_err(error_pb2.ERROR_CODE_HASH_MISMATCH, retryable=True),
        _blotter_err(error_pb2.ERROR_CODE_CONCURRENCY_CONFLICT, retryable=True),
        _blotter_err(error_pb2.ERROR_CODE_STORAGE_UNAVAILABLE, retryable=False),
        _FakeGrpcRpcError(grpc.StatusCode.PERMISSION_DENIED),
        _FakeGrpcRpcError(grpc.StatusCode.UNAUTHENTICATED),
    ):
        assert not is_retryable_error(non_retryable_err)
        sleeps.clear()
        calls = {"n": 0}

        def fail_once(err: BaseException = non_retryable_err) -> None:
            calls["n"] += 1
            raise err

        with pytest.raises(type(non_retryable_err)):
            policy.execute(fail_once)
        assert calls["n"] == 1
        assert sleeps == []


# ======================================================================
# 4. JsonCheckpointStore, /healthz Probe, & Ledger Transition Gate
# ======================================================================


def test_checkpoint_store_locking_atomic_save_and_secret_hygiene(tmp_path: Path) -> None:
    cp_path = tmp_path / "quotes.checkpoint.json"
    with JsonCheckpointStore(cp_path) as store1:
        # Second concurrent writer fails immediately
        store2 = JsonCheckpointStore(cp_path)
        with pytest.raises(CheckpointLockError):
            store2.acquire()

        payload = {
            "schema_version": CHECKPOINT_SCHEMA_VERSION,
            "kind": "quotes",
            "sidecar_id": "q-sidecar-1",
            "ledger_epoch": 1,
            "ledger_instance_id": ULID_1,
            "trading_date": "2026-06-15",
            "input_fingerprint": "a" * 64,
            "confirmed_offset": 128,
            "in_flight": None,
            "previous_epoch_summary": None,
            "state": {"acked_quotes": {}, "heads": {}},
        }
        store1.save(payload)
        assert store1.load(expected_kind="quotes", expected_sidecar_id="q-sidecar-1") == payload
        # Zero leftover temp files in checkpoint parent dir
        leftovers = [p for p in tmp_path.iterdir() if p.suffix == ".tmp"]
        assert leftovers == []

        # Secret/token persistence MUST be rejected
        bad_payload = dict(payload)
        bad_payload["state"] = {"bearer_token": "never-persist"}
        with pytest.raises(CheckpointCorruptionError, match="forbidden credential"):
            store1.save(bad_payload)

    # After release, another writer can acquire cleanly
    with JsonCheckpointStore(cp_path) as store3:
        loaded = store3.load()
        assert loaded is not None
        assert loaded["confirmed_offset"] == 128


def test_live_loopback_http_healthz_probe() -> None:
    response_state: dict[str, Any] = {
        "code": 200,
        "body": {
            "status": "ok",
            "service": "ficant-server",
            "storage_mode": "persistent_wal",
            "blotter_healthy": True,
            "ledger_epoch": 1,
            "ledger_instance_id": ULID_1,
            "web_ui_mode": "disabled",
            "embedded_assets_count": 0,
        },
    }

    class _Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            if self.path != "/healthz":
                self.send_response(404)
                self.end_headers()
                return
            raw = json.dumps(response_state["body"]).encode("utf-8")
            self.send_response(response_state["code"])
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def log_message(self, format: str, *args: object) -> None:
            return

    server = HTTPServer(("127.0.0.1", 0), _Handler)
    port = server.server_port
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        hz = probe_healthz(f"127.0.0.1:{port}", insecure=True, timeout_seconds=2.0)
        ledger = hz.require_healthy_ledger()
        assert ledger.as_tuple() == (1, ULID_1)

        # Disabled storage fails require_healthy_ledger
        response_state["body"] = {
            **response_state["body"],
            "storage_mode": "disabled",
            "ledger_epoch": None,
            "ledger_instance_id": None,
        }
        hz_disabled = probe_healthz(f"127.0.0.1:{port}", insecure=True, timeout_seconds=2.0)
        with pytest.raises(HealthCheckError, match="disabled"):
            hz_disabled.require_healthy_ledger()

        # 503 degraded raises HealthCheckError
        response_state["code"] = 503
        response_state["body"] = {
            **response_state["body"],
            "status": "degraded",
            "storage_mode": "persistent_wal",
            "blotter_healthy": False,
        }
        with pytest.raises(HealthCheckError, match="HTTP 503"):
            probe_healthz(f"127.0.0.1:{port}", insecure=True, timeout_seconds=2.0)
    finally:
        server.shutdown()
        server.server_close()


def test_ledger_transition_priority_order_rollover_quarantine_and_restore(tmp_path: Path) -> None:
    cp_path = tmp_path / "ledger_gate.checkpoint.json"
    with JsonCheckpointStore(cp_path) as store:
        # Initial baseline at epoch 2, ULID_2 with an unacknowledged in_flight request
        outcome0 = reconcile_ledger_checkpoint(
            store=store,
            server_health=_health(2, ULID_2),
            kind="quotes",
            sidecar_id="sc-1",
            trading_date="2026-06-15",
            input_fingerprint="day1-fp",
            initial_state_factory=lambda: {"acked_quotes": {}, "heads": {}},
        )
        cp = outcome0.checkpoint
        cp["in_flight"] = {"idempotency_key": "old-e2-batch", "pending_offset": 99}
        store.save(cp)

        # Priority 1: Epoch regression MUST raise LedgerEpochRegressionError
        # even when ledger_instance_id ALSO differs!
        with pytest.raises(LedgerEpochRegressionError):
            reconcile_ledger_checkpoint(
                store=store,
                server_health=_health(1, ULID_1),
                kind="quotes",
                sidecar_id="sc-1",
                trading_date="2026-06-15",
                input_fingerprint="day1-fp",
                initial_state_factory=lambda: {},
            )

        # Priority 2: Same epoch, different instance MUST raise LedgerInstanceMismatchError
        with pytest.raises(LedgerInstanceMismatchError):
            reconcile_ledger_checkpoint(
                store=store,
                server_health=_health(2, ULID_3),
                kind="quotes",
                sidecar_id="sc-1",
                trading_date="2026-06-15",
                input_fingerprint="day1-fp",
                initial_state_factory=lambda: {},
            )

        # Priority 3a: Epoch advance with SAME instance -> rejected
        with pytest.raises(LedgerRolloverValidationError, match="unchanged"):
            reconcile_ledger_checkpoint(
                store=store,
                server_health=_health(3, ULID_2),
                kind="quotes",
                sidecar_id="sc-1",
                trading_date="2026-06-16",
                input_fingerprint="day2-fp",
                initial_state_factory=lambda: {},
            )

        # Priority 3b: Epoch advance with non-advancing trading_date -> rejected
        with pytest.raises(LedgerRolloverValidationError, match="trading_date"):
            reconcile_ledger_checkpoint(
                store=store,
                server_health=_health(3, ULID_3),
                kind="quotes",
                sidecar_id="sc-1",
                trading_date="2026-06-15",
                input_fingerprint="day2-fp",
                initial_state_factory=lambda: {},
            )

        # Priority 3c: Epoch advance with yesterday's input_fingerprint -> rejected
        with pytest.raises(LedgerRolloverValidationError, match="input_fingerprint"):
            reconcile_ledger_checkpoint(
                store=store,
                server_health=_health(3, ULID_3),
                kind="quotes",
                sidecar_id="sc-1",
                trading_date="2026-06-16",
                input_fingerprint="day1-fp",
                initial_state_factory=lambda: {},
            )

        # Priority 3d: Valid rollover advance quarantines previous in_flight & clears live in_flight
        rolled = reconcile_ledger_checkpoint(
            store=store,
            server_health=_health(3, ULID_3),
            kind="quotes",
            sidecar_id="sc-1",
            trading_date="2026-06-16",
            input_fingerprint="day2-fp",
            initial_state_factory=lambda: {"acked_quotes": {}, "heads": {}},
        )
        assert rolled.rolled_over is True
        assert rolled.quarantined_previous_in_flight is True
        assert rolled.checkpoint["ledger_epoch"] == 3
        assert rolled.checkpoint["ledger_instance_id"] == ULID_3
        assert rolled.checkpoint["in_flight"] is None
        assert rolled.checkpoint["confirmed_offset"] == 0
        prev = rolled.checkpoint["previous_epoch_summary"]
        assert prev["ledger_epoch"] == 2
        assert prev["quarantined_in_flight"]["idempotency_key"] == "old-e2-batch"

        # Explicit restore reset to an older restored epoch (epoch 1, ULID_1)
        restored = reconcile_ledger_checkpoint(
            store=store,
            server_health=_health(1, ULID_1),
            kind="quotes",
            sidecar_id="sc-1",
            trading_date="2026-06-14",
            input_fingerprint="restore-fp",
            initial_state_factory=lambda: {"acked_quotes": {}, "heads": {}},
            expected_ledger=(1, ULID_1),
            reset_checkpoint_for_restore=True,
        )
        assert restored.reset_for_restore is True
        assert restored.quarantine_archive_path is not None
        assert restored.quarantine_archive_path.is_file()
        assert restored.checkpoint["ledger_epoch"] == 1
        assert restored.checkpoint["ledger_instance_id"] == ULID_1


# ======================================================================
# 5. QuoteStreamSidecar (F-13)
# ======================================================================


class _MockQuoteServerClient:
    """In-memory mock faithfully implementing F-13 quote store semantics."""

    def __init__(self) -> None:
        self.calls: list[tuple[service_pb2.IngestMarketQuotesRequest, tuple[int, str] | None]] = []
        self.snapshot_calls: int = 0
        self.history: dict[tuple[str, str, int], tuple[bytes, str]] = {}
        self.heads: dict[tuple[str, str], int] = {}
        self.commit_seq = 0
        self.scripted_errors: list[BaseException | None] = []
        self.post_apply_errors: list[BaseException | None] = []

    def get_account_snapshot(
        self, req: service_pb2.GetAccountSnapshotRequest
    ) -> service_pb2.GetAccountSnapshotResult:
        self.snapshot_calls += 1
        raise AssertionError("GetAccountSnapshot should not be called by default")

    def ingest_market_quotes(
        self,
        request: service_pb2.IngestMarketQuotesRequest,
        *,
        expected_ledger: tuple[int, str] | None = None,
        metadata: Sequence[tuple[str, str]] | None = None,
    ) -> service_pb2.IngestMarketQuotesResult:
        req_copy = service_pb2.IngestMarketQuotesRequest()
        req_copy.CopyFrom(request)
        self.calls.append((req_copy, expected_ledger))

        if self.scripted_errors:
            err = self.scripted_errors.pop(0)
            if err is not None:
                raise err

        # Phase 1: check conflicts
        batch_hashes: dict[tuple[str, str, int], bytes] = {}
        for tick in request.ticks:
            key = (tick.source_id, tick.instrument_code, tick.quote_seq)
            digest = market_quote_canonical_payload_hash(tick)
            if key in self.history and self.history[key][0] != digest:
                raise _blotter_err(error_pb2.ERROR_CODE_HASH_MISMATCH)
            if key in batch_hashes and batch_hashes[key] != digest:
                raise _blotter_err(error_pb2.ERROR_CODE_HASH_MISMATCH)
            batch_hashes[key] = digest

        # Phase 2: apply
        applied = 0
        skipped = 0
        mutated = False
        for tick in request.ticks:
            key = (tick.source_id, tick.instrument_code, tick.quote_seq)
            if key in self.history:
                skipped += 1
                continue
            digest = market_quote_canonical_payload_hash(tick)
            self.history[key] = (digest, f"{tick.received_at.instant.seconds}.{tick.received_at.instant.nanos}")
            mutated = True
            pair = (tick.source_id, tick.instrument_code)
            if tick.quote_seq > self.heads.get(pair, 0):
                self.heads[pair] = tick.quote_seq
                applied += 1
            else:
                skipped += 1

        if mutated:
            self.commit_seq += 1

        if self.post_apply_errors:
            err = self.post_apply_errors.pop(0)
            if err is not None:
                raise err

        return service_pb2.IngestMarketQuotesResult(
            committed_seq=self.commit_seq,
            replayed=False,
            applied_tick_count=applied,
            skipped_stale_tick_count=skipped,
            risk_summary=_make_risk_summary(),
        )


def test_quote_row_validation_matrix_all_source_types_and_price_combinations() -> None:
    resolver = AccountUnitsResolver()
    base = {
        "account_id": "acct-1",
        "source_id": "qsrc-1",
        "instrument_code": "BOND_10Y",
        "quote_seq": 1,
        "observed_at": "2026-06-15T10:00:00.123456789+08:00",
        "received_at": "2026-06-15T10:00:01.000000000+08:00",
    }

    # All 4 valid PriceSourceType values accepted
    for src_type_str, expected_enum in (
        ("REAL_TRADE", fact_pb2.PRICE_SOURCE_TYPE_REAL_TRADE),
        ("ACTIVE_QUOTE", fact_pb2.PRICE_SOURCE_TYPE_ACTIVE_QUOTE),
        ("MODEL_VALUATION", fact_pb2.PRICE_SOURCE_TYPE_MODEL_VALUATION),
        ("CURVE_INTERPOLATION", fact_pb2.PRICE_SOURCE_TYPE_CURVE_INTERPOLATION),
    ):
        _acct, tick, _h, _rec = parse_quote_row(
            {**base, "source_type": src_type_str, "mid_clean_price": "100.5"},
            units_resolver=resolver,
        )
        assert tick.source_type == expected_enum
        assert tick.observed_at.instant.nanos == 123_456_789

    # Reject unknown / unspecified source types
    for bad_type in ("UNSPECIFIED", "PRICE_SOURCE_TYPE_UNSPECIFIED", "SYNTHETIC", 1, ""):
        with pytest.raises(ValueError):
            parse_quote_row(
                {**base, "source_type": bad_type, "mid_clean_price": "100.5"},
                units_resolver=resolver,
            )

    # Valid price combinations: bilateral, mid, ytm==0, single-sided bid+mid, single-sided ask+ytm
    for valid_prices in (
        {"bid_clean_price": "100.10", "ask_clean_price": "100.20"},
        {"bid_clean_price": "100.10", "ask_clean_price": "100.10"},
        {"mid_clean_price": "100.15"},
        {"mid_yield_to_maturity": "0"},
        {"bid_clean_price": "100.10", "mid_clean_price": "100.15"},
        {"ask_clean_price": "100.20", "mid_yield_to_maturity": "0.025"},
    ):
        _acct, tick, _h, _rec = parse_quote_row(
            {**base, "source_type": "ACTIVE_QUOTE", **valid_prices},
            units_resolver=resolver,
        )
        if "bid_clean_price" in valid_prices:
            assert tick.HasField("bid_clean_price")
        if "ask_clean_price" in valid_prices:
            assert tick.HasField("ask_clean_price")

    # Invalid combinations: bid-only, ask-only, none, bid>ask, non-positive prices, negative ytm, observed>received
    for bad_row in (
        {**base, "source_type": "ACTIVE_QUOTE", "bid_clean_price": "100.10"},
        {**base, "source_type": "ACTIVE_QUOTE", "ask_clean_price": "100.20"},
        {**base, "source_type": "ACTIVE_QUOTE"},
        {**base, "source_type": "ACTIVE_QUOTE", "bid_clean_price": "100.30", "ask_clean_price": "100.20"},
        {**base, "source_type": "ACTIVE_QUOTE", "mid_clean_price": "0"},
        {**base, "source_type": "ACTIVE_QUOTE", "mid_clean_price": "-1"},
        {**base, "source_type": "ACTIVE_QUOTE", "bid_clean_price": "0", "ask_clean_price": "100"},
        {**base, "source_type": "ACTIVE_QUOTE", "mid_yield_to_maturity": "-0.001"},
        {
            **base,
            "source_type": "ACTIVE_QUOTE",
            "mid_clean_price": "100.1",
            "observed_at": "2026-06-15T10:00:02+08:00",
            "received_at": "2026-06-15T10:00:01+08:00",
        },
    ):
        with pytest.raises(ValueError):
            parse_quote_row(bad_row, units_resolver=resolver)


def test_quote_sidecar_dedup_dual_sources_backfill_hash_mismatch_and_healthz_probe(
    tmp_path: Path,
) -> None:
    csv_path = tmp_path / "quotes.csv"
    cp_path = tmp_path / "quotes.cp.json"

    header = "account_id,source_id,instrument_code,quote_seq,source_type,bid_clean_price,ask_clean_price,mid_clean_price,mid_yield_to_maturity,observed_at,received_at\n"
    row_a10 = "acct-1,src-A,BOND_10Y,10,ACTIVE_QUOTE,100.10,100.20,,,2026-06-15T10:00:00.123456789+08:00,2026-06-15T10:00:01+08:00\n"
    row_b10 = "acct-1,src-B,BOND_10Y,10,MODEL_VALUATION,,,100.15,0.024,2026-06-15T10:00:00+08:00,2026-06-15T10:00:01+08:00\n"
    # Backfill older unseen seq=5 for src-A
    row_a5 = "acct-1,src-A,BOND_10Y,5,REAL_TRADE,,,100.05,,2026-06-15T09:55:00+08:00,2026-06-15T09:55:01+08:00\n"
    csv_path.write_text(header + row_a10 + row_b10 + row_a5, encoding="utf-8")

    client = _MockQuoteServerClient()
    probe_count = {"n": 0}

    def probe() -> HealthzStatus:
        probe_count["n"] += 1
        return _health(1, ULID_1)

    cfg = QuoteStreamConfig(
        sidecar_id="quotes-1",
        trading_date="2026-06-15",
        checkpoint_path=cp_path,
        input_path=csv_path,
        batch_size=16,
        expected_ledger=(1, ULID_1),  # /healthz MUST still be probed!
        backoff=BackoffPolicy(max_retries=2, initial_delay_ms=10, max_delay_ms=20, sleep_fn=lambda _: None),
    )
    sidecar = QuoteStreamSidecar(cfg, client, health_probe=probe)
    rep1 = sidecar.run_once()

    assert probe_count["n"] == 1
    assert client.snapshot_calls == 0
    assert rep1.ticks_read == 3
    assert rep1.ticks_sent == 3
    assert rep1.ticks_skipped_dedup == 0
    assert client.heads[("src-A", "BOND_10Y")] == 10  # backfill seq=5 did NOT rewind head
    assert client.heads[("src-B", "BOND_10Y")] == 10  # independent source namespace
    # All RPCs carried paired expected_ledger
    assert all(exp == (1, ULID_1) for _req, exp in client.calls)
    # 9-digit nanos preserved on wire
    assert client.calls[0][0].ticks[0].observed_at.instant.nanos == 123_456_789

    cp_after_1 = json.loads(cp_path.read_text(encoding="utf-8"))
    orig_first_received = cp_after_1["state"]["acked_quotes"]["src-A"]["BOND_10Y"]["10"]["first_received_at"]

    # Append re-delivery of (src-A, BOND_10Y, 10) changing ONLY received_at -> skipped locally, first_received_at preserved!
    row_a10_redeliv = "acct-1,src-A,BOND_10Y,10,ACTIVE_QUOTE,100.10,100.20,,,2026-06-15T10:00:00.123456789+08:00,2026-06-15T10:05:00+08:00\n"
    with csv_path.open("a", encoding="utf-8") as f:
        f.write(row_a10_redeliv)

    prev_rpc_count = len(client.calls)
    rep2 = sidecar.run_once()
    assert probe_count["n"] == 2
    assert rep2.ticks_read == 1
    assert rep2.ticks_sent == 0
    assert rep2.ticks_skipped_dedup == 1
    assert len(client.calls) == prev_rpc_count
    cp_after_2 = json.loads(cp_path.read_text(encoding="utf-8"))
    assert (
        cp_after_2["state"]["acked_quotes"]["src-A"]["BOND_10Y"]["10"]["first_received_at"]
        == orig_first_received
    )

    # Append conflicting payload for (src-A, BOND_10Y, 10) -> MUST send to server for HASH_MISMATCH adjudication
    row_a10_conflict = "acct-1,src-A,BOND_10Y,10,ACTIVE_QUOTE,100.99,101.05,,,2026-06-15T10:00:00.123456789+08:00,2026-06-15T10:06:00+08:00\n"
    with csv_path.open("a", encoding="utf-8") as f:
        f.write(row_a10_conflict)

    with pytest.raises(BlotterError) as exc_info:
        sidecar.run_once()
    assert exc_info.value.code == error_pb2.ERROR_CODE_HASH_MISMATCH
    assert len(client.calls) == prev_rpc_count + 1  # sent exactly once, NOT retried!


def test_quote_sidecar_jsonl_timeout_retry_and_in_flight_recovery(tmp_path: Path) -> None:
    jsonl_path = tmp_path / "quotes.jsonl"
    cp_path = tmp_path / "quotes.cp.json"
    rows = [
        {
            "account_id": "acct-1",
            "source_id": "src-J",
            "instrument_code": "BOND_5Y",
            "quote_seq": 1,
            "source_type": "CURVE_INTERPOLATION",
            "mid_yield_to_maturity": "0.0215",
            "observed_at": "2026-06-15T09:30:00+08:00",
            "received_at": "2026-06-15T09:30:00.5+08:00",
        }
    ]
    jsonl_path.write_text("\n".join(json.dumps(r) for r in rows) + "\n", encoding="utf-8")

    client = _MockQuoteServerClient()
    # Exhaust retries on first run -> in_flight MUST stay frozen on disk
    client.scripted_errors = [TimeoutError("t1"), TimeoutError("t2")]
    sleeps: list[float] = []
    cfg = QuoteStreamConfig(
        sidecar_id="quotes-j",
        trading_date="2026-06-15",
        checkpoint_path=cp_path,
        input_path=jsonl_path,
        backoff=BackoffPolicy(max_retries=1, initial_delay_ms=50, max_delay_ms=100, sleep_fn=sleeps.append),
    )
    sidecar = QuoteStreamSidecar(cfg, client, health_probe=lambda: _health(1, ULID_1))
    with pytest.raises(TimeoutError):
        sidecar.run_once()
    assert sleeps == [0.05]

    frozen = json.loads(cp_path.read_text(encoding="utf-8"))
    assert frozen["in_flight"] is not None
    assert frozen["confirmed_offset"] == 0
    frozen_key = frozen["in_flight"]["idempotency_key"]

    # Append a 2nd row while crashed; recovery MUST replay exact frozen batch first!
    with jsonl_path.open("a", encoding="utf-8") as f:
        f.write(
            json.dumps(
                {
                    "account_id": "acct-1",
                    "source_id": "src-J",
                    "instrument_code": "BOND_5Y",
                    "quote_seq": 2,
                    "source_type": "REAL_TRADE",
                    "mid_clean_price": "101.25",
                    "observed_at": "2026-06-15T09:31:00+08:00",
                    "received_at": "2026-06-15T09:31:00.1+08:00",
                }
            )
            + "\n"
        )

    client.calls.clear()
    rep = sidecar.run_once()
    assert rep.replayed_in_flight is True
    assert rep.batches_sent == 2
    assert client.calls[0][0].idempotency_key == frozen_key
    assert [t.quote_seq for t in client.calls[0][0].ticks] == [1]
    assert [t.quote_seq for t in client.calls[1][0].ticks] == [2]


# ======================================================================
# 6. TradeFeedSidecar (F-07)
# ======================================================================


class _MockTradeFeedServerClient:
    """In-memory mock implementing transport watermarks, gaps, aliases, and idempotent replay."""

    def __init__(self, source_id: str) -> None:
        self.source_id = source_id
        self.calls: list[tuple[service_pb2.IngestTradeFeedRequest, tuple[int, str] | None]] = []
        self.snapshot_calls: int = 0
        self.seen_seqs: dict[int, bytes] = {}
        self.seen_biz: dict[tuple[str, str, int], tuple[bytes, int]] = {}
        self.business_effect_count: dict[tuple[str, str, int], int] = {}
        self.idempotency_receipts: dict[str, tuple[bytes, service_pb2.IngestTradeFeedResult]] = {}
        self.commit_seq = 0
        self.pre_rpc_errors: list[BaseException | None] = []
        self.post_commit_errors: list[BaseException | None] = []
        self.forced_outcome_kinds: list[int] | None = None

    def get_account_snapshot(
        self, req: service_pb2.GetAccountSnapshotRequest
    ) -> service_pb2.GetAccountSnapshotResult:
        self.snapshot_calls += 1
        raise AssertionError("GetAccountSnapshot should not be called by default")

    def _compute_watermarks(self) -> tuple[int, int, list[model_pb2.SeqRange]]:
        if not self.seen_seqs:
            return (0, 0, [])
        max_seen = max(self.seen_seqs)
        # Contiguous starting from min-1? In blotter feed starts after 0 (or first contiguous block).
        # For tests starting at 10 (start_after=9), support configurable base!
        base = getattr(self, "start_after_feed_seq", 0)
        contiguous = base
        while (contiguous + 1) in self.seen_seqs:
            contiguous += 1
        missing: list[model_pb2.SeqRange] = []
        cur = contiguous + 1
        while cur <= max_seen:
            if cur not in self.seen_seqs:
                start = cur
                while cur <= max_seen and cur not in self.seen_seqs:
                    cur += 1
                missing.append(model_pb2.SeqRange(first=start, last=cur - 1))
            else:
                cur += 1
        return (contiguous, max_seen, missing)

    def ingest_trade_feed(
        self,
        request: service_pb2.IngestTradeFeedRequest,
        *,
        expected_ledger: tuple[int, str] | None = None,
        metadata: Sequence[tuple[str, str]] | None = None,
    ) -> service_pb2.IngestTradeFeedResult:
        req_copy = service_pb2.IngestTradeFeedRequest()
        req_copy.CopyFrom(request)
        self.calls.append((req_copy, expected_ledger))

        if self.pre_rpc_errors:
            err = self.pre_rpc_errors.pop(0)
            if err is not None:
                raise err

        raw_req = request.SerializeToString()
        if request.idempotency_key in self.idempotency_receipts:
            saved_bytes, saved_res = self.idempotency_receipts[request.idempotency_key]
            if saved_bytes != raw_req:
                raise _blotter_err(error_pb2.ERROR_CODE_HASH_MISMATCH)
            replayed_res = service_pb2.IngestTradeFeedResult()
            replayed_res.CopyFrom(saved_res)
            replayed_res.replayed = True
            return replayed_res

        outcomes: list[spi_pb2.TradeFillIngestOutcome] = []
        for idx, fill in enumerate(request.fills):
            p_hash = trade_fill_payload_hash(fill)
            b_hash = trade_fill_business_hash(fill)
            if fill.feed_seq in self.seen_seqs and self.seen_seqs[fill.feed_seq] != p_hash:
                raise _blotter_err(error_pb2.ERROR_CODE_HASH_MISMATCH)
            biz_key = (fill.source_id, fill.external_trade_id, fill.source_revision)
            if biz_key in self.seen_biz and self.seen_biz[biz_key][0] != b_hash:
                raise _blotter_err(error_pb2.ERROR_CODE_HASH_MISMATCH)

            already_seq = fill.feed_seq in self.seen_seqs
            self.seen_seqs[fill.feed_seq] = p_hash

            if self.forced_outcome_kinds is not None:
                kind = self.forced_outcome_kinds[idx]
            elif already_seq:
                kind = spi_pb2.TRADE_FILL_OUTCOME_KIND_PER_ITEM_DEDUPLICATED
            elif biz_key in self.seen_biz:
                # Alias feed_seq: transport advances, business effect applies only ONCE!
                kind = spi_pb2.TRADE_FILL_OUTCOME_KIND_PER_ITEM_DEDUPLICATED
            else:
                self.seen_biz[biz_key] = (b_hash, fill.feed_seq)
                self.business_effect_count[biz_key] = (
                    self.business_effect_count.get(biz_key, 0) + 1
                )
                contiguous_now, _max_now, missing_now = self._compute_watermarks()
                if missing_now and fill.feed_seq > contiguous_now:
                    kind = spi_pb2.TRADE_FILL_OUTCOME_KIND_BUFFERED_GAP
                elif fill.action == model_pb2.TRADE_FEED_RECORD_ACTION_CANCEL_REVERSAL:
                    kind = spi_pb2.TRADE_FILL_OUTCOME_KIND_REVERSED
                else:
                    kind = spi_pb2.TRADE_FILL_OUTCOME_KIND_AUTO_MATCHED

            outcomes.append(
                spi_pb2.TradeFillIngestOutcome(
                    source_id=fill.source_id,
                    external_trade_id=fill.external_trade_id,
                    source_revision=fill.source_revision,
                    feed_seq=fill.feed_seq,
                    kind=kind,
                )
            )

        contiguous, max_seen, missing = self._compute_watermarks()
        self.commit_seq += 1
        res = service_pb2.IngestTradeFeedResult(
            committed_seq=self.commit_seq,
            replayed=False,
            item_outcomes=outcomes,
            feed_status=model_pb2.FeedStatusView(
                source_id=self.source_id,
                feed_status_revision=self.commit_seq,
                contiguous_feed_seq=contiguous,
                max_seen_feed_seq=max_seen,
                missing_feed_ranges=missing,
            ),
        )
        self.idempotency_receipts[request.idempotency_key] = (raw_req, res)

        if self.post_commit_errors:
            err = self.post_commit_errors.pop(0)
            if err is not None:
                raise err

        return res


def _trade_row(
    feed_seq: int,
    ext_id: str,
    rev: int = 1,
    *,
    action: str = "NEW_FILL",
    source_id: str = "cfets-1",
    notional: str = "10000000",
    clean_price: str | None = "100.25",
    ytm: str | None = None,
    batch_first: int | None = None,
    batch_last: int | None = None,
) -> dict[str, Any]:
    row: dict[str, Any] = {
        "source_id": source_id,
        "external_trade_id": ext_id,
        "source_revision": rev,
        "feed_seq": feed_seq,
        "action": action,
        "account_id": "acct-1",
        "instrument_code": "BOND_10Y",
        "side": "BUY",
        "settlement_date": "2026-06-15",
        "notional": notional,
        "executed_at": "2026-06-15T10:15:00.000000123+08:00",
    }
    if clean_price is not None:
        row["clean_price_per_100"] = clean_price
    if ytm is not None:
        row["yield_to_maturity"] = ytm
    if batch_first is not None:
        row["batch_first_feed_seq"] = batch_first
    if batch_last is not None:
        row["batch_last_feed_seq"] = batch_last
    return row


def test_trade_fill_row_validation_actions_and_batch_bounds() -> None:
    resolver = AccountUnitsResolver()
    # All 3 actions accepted (CANCEL_REVERSAL allows 0 notional)
    for action, notional in (("NEW_FILL", "1000"), ("AMEND", "2000"), ("CANCEL_REVERSAL", "0")):
        fill, _ph, _bh = parse_trade_fill_row(
            _trade_row(1, "T-1", action=action, notional=notional),
            expected_source_id="cfets-1",
            units_resolver=resolver,
        )
        assert fill.executed_at.instant.nanos == 123

    # Batch bounds violation rejected
    with pytest.raises(ValueError, match="batch bounds"):
        parse_trade_fill_row(
            _trade_row(5, "T-2", batch_first=6, batch_last=10),
            expected_source_id="cfets-1",
            units_resolver=resolver,
        )
    # Both clean_price and ytm rejected
    with pytest.raises(ValueError, match="cannot specify both"):
        parse_trade_fill_row(
            _trade_row(5, "T-3", clean_price="100.1", ytm="0.025"),
            expected_source_id="cfets-1",
            units_resolver=resolver,
        )
    # Unknown action rejected
    with pytest.raises(ValueError, match="TradeFeedRecordAction"):
        parse_trade_fill_row(
            _trade_row(5, "T-4", action="DELETE"),
            expected_source_id="cfets-1",
            units_resolver=resolver,
        )


def test_trade_feed_gap_10_12_11_alias_seq_and_dual_cursors(tmp_path: Path) -> None:
    feed_path = tmp_path / "trades.jsonl"
    cp_path = tmp_path / "trades.cp.json"

    client = _MockTradeFeedServerClient("cfets-1")
    client.start_after_feed_seq = 9  # type: ignore[attr-defined]

    cfg = TradeFeedConfig(
        sidecar_id="tf-1",
        source_id="cfets-1",
        trading_date="2026-06-15",
        checkpoint_path=cp_path,
        input_path=feed_path,
        batch_size=16,
        heartbeat_at="2026-06-15T10:20:00+08:00",
    )
    sidecar = TradeFeedSidecar(cfg, client, health_probe=lambda: _health(1, ULID_1))

    # Step 1: feed_seq=10 arrives -> contiguous=10
    feed_path.write_text(json.dumps(_trade_row(10, "T-10", 1)) + "\n", encoding="utf-8")
    r1 = sidecar.run_once()
    assert client.snapshot_calls == 0
    assert r1.contiguous_feed_seq == 10
    assert r1.max_seen_feed_seq == 10
    assert r1.missing_ranges == ()
    assert r1.backfill_from_feed_seq == 10

    # Step 2: feed_seq=12 arrives (gap at 11!) -> contiguous stays 10, backfill lower bound is 10
    with feed_path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(_trade_row(12, "T-12", 1)) + "\n")
    r2 = sidecar.run_once()
    assert r2.contiguous_feed_seq == 10
    assert r2.max_seen_feed_seq == 12
    assert r2.missing_ranges == ((11, 11),)
    assert r2.backfill_from_feed_seq == 10

    # Step 3: feed_seq=11 backfills gap -> contiguous jumps to 12, missing_ranges cleared
    with feed_path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(_trade_row(11, "T-11", 1)) + "\n")
    r3 = sidecar.run_once()
    assert r3.contiguous_feed_seq == 12
    assert r3.max_seen_feed_seq == 12
    assert r3.missing_ranges == ()
    assert r3.backfill_from_feed_seq == 12

    # Step 4: Same business revision (cfets-1, T-10, rev=1) arrives on ALIAS feed_seq=13!
    # MUST still be sent to server so transport watermark advances to 13 while business effect stays 1!
    with feed_path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(_trade_row(13, "T-10", 1)) + "\n")
    r4 = sidecar.run_once()
    assert r4.fills_sent == 1
    assert r4.fills_skipped_dedup == 0
    assert r4.contiguous_feed_seq == 13
    assert r4.outcome_counts == {"PER_ITEM_DEDUPLICATED": 1}
    assert client.business_effect_count[("cfets-1", "T-10", 1)] == 1

    # Verify persisted dual cursors in checkpoint JSON
    cp_data = json.loads(cp_path.read_text(encoding="utf-8"))
    transport = cp_data["state"]["transport"]
    assert transport["contiguous_feed_seq"] == 13
    assert transport["max_seen_feed_seq"] == 13
    assert transport["missing_ranges"] == []
    assert set(transport["feed_seq_hashes"].keys()) == {"10", "11", "12", "13"}

    biz_key = "cfets-1\x1fT-10\x1f1"
    biz = cp_data["state"]["business"][biz_key]
    assert biz["canonical_feed_seq"] == 10
    assert biz["alias_seqs"] == [13]


def test_trade_feed_frozen_in_flight_recovery_pre_rpc_and_lost_response_with_file_append(
    tmp_path: Path,
) -> None:
    feed_path = tmp_path / "trades_recover.csv"
    cp_path = tmp_path / "trades_recover.cp.json"

    header = "source_id,external_trade_id,source_revision,feed_seq,action,account_id,instrument_code,side,settlement_date,notional,clean_price_per_100,executed_at\n"
    line1 = "cfets-1,T-1,1,1,NEW_FILL,acct-1,BOND_10Y,BUY,2026-06-15,5000000,100.10,2026-06-15T10:00:00+08:00\n"
    feed_path.write_text(header + line1, encoding="utf-8")

    now_times = [
        datetime(2026, 6, 15, 2, 0, 0, tzinfo=timezone.utc),
        datetime(2026, 6, 15, 2, 5, 0, tzinfo=timezone.utc),
        datetime(2026, 6, 15, 2, 10, 0, tzinfo=timezone.utc),
    ]
    time_idx = {"i": 0}

    def deterministic_now() -> datetime:
        val = now_times[min(time_idx["i"], len(now_times) - 1)]
        time_idx["i"] += 1
        return val

    client = _MockTradeFeedServerClient("cfets-1")
    cfg = TradeFeedConfig(
        sidecar_id="tf-rec",
        source_id="cfets-1",
        trading_date="2026-06-15",
        checkpoint_path=cp_path,
        input_path=feed_path,
        batch_size=16,
        backoff=BackoffPolicy(max_retries=0, initial_delay_ms=10, max_delay_ms=10, sleep_fn=lambda _: None),
    )
    sidecar = TradeFeedSidecar(
        cfg,
        client,
        health_probe=lambda: _health(1, ULID_1),
        now_utc_fn=deterministic_now,
    )

    # --- Window A: Crash BEFORE RPC reaches server (recovery receives replayed=False) ---
    client.pre_rpc_errors = [ConnectionResetError("crash before send")]
    with pytest.raises(ConnectionResetError):
        sidecar.run_once()

    cp_frozen_a = json.loads(cp_path.read_text(encoding="utf-8"))
    assert cp_frozen_a["in_flight"] is not None
    assert cp_frozen_a["confirmed_offset"] == 0
    frozen_key_a = cp_frozen_a["in_flight"]["idempotency_key"]
    frozen_hb_a = cp_frozen_a["in_flight"]["heartbeat_at"]
    frozen_bytes_a = cp_frozen_a["in_flight"]["request_pb_b64"]

    # Append line 2 while crashed! Must NOT pollute frozen batch A!
    line2 = "cfets-1,T-2,1,2,NEW_FILL,acct-1,BOND_10Y,SELL,2026-06-15,6000000,100.20,2026-06-15T10:01:00+08:00\n"
    with feed_path.open("a", encoding="utf-8") as f:
        f.write(line2)

    client.calls.clear()
    # --- Window B: During recovery, Batch A succeeds (replayed=False), then Batch B commits on server
    # but response is lost (ConnectionError post-commit -> recovery will receive replayed=True)! ---
    client.post_commit_errors = [None, ConnectionError("response lost after commit")]
    with pytest.raises(ConnectionError):
        sidecar.run_once()

    # First RPC was exact frozen bytes of Batch A (ONLY feed_seq=1, original key & heartbeat)
    assert len(client.calls) == 2
    assert client.calls[0][0].idempotency_key == frozen_key_a
    assert [f.feed_seq for f in client.calls[0][0].fills] == [1]
    assert client.calls[0][0].SerializeToString() == __import__("base64").b64decode(frozen_bytes_a)
    assert client.calls[0][0].heartbeat_at.instant.seconds == parse_market_time(frozen_hb_a).instant.seconds

    # Batch B is now frozen on disk
    cp_frozen_b = json.loads(cp_path.read_text(encoding="utf-8"))
    assert cp_frozen_b["in_flight"] is not None
    frozen_key_b = cp_frozen_b["in_flight"]["idempotency_key"]
    assert cp_frozen_b["state"]["transport"]["contiguous_feed_seq"] == 1

    # Append line 3 while crashed after Batch B's commit!
    line3 = "cfets-1,T-3,1,3,NEW_FILL,acct-1,BOND_10Y,BUY,2026-06-15,7000000,100.30,2026-06-15T10:02:00+08:00\n"
    with feed_path.open("a", encoding="utf-8") as f:
        f.write(line3)

    client.calls.clear()
    rep_final = sidecar.run_once()
    assert rep_final.replayed_in_flight is True
    assert rep_final.batches_sent == 2
    assert rep_final.contiguous_feed_seq == 3
    # First RPC replayed Batch B with exact frozen key (server returned replayed=True)
    assert client.calls[0][0].idempotency_key == frozen_key_b
    assert [f.feed_seq for f in client.calls[0][0].fills] == [2]
    # Second RPC sent newly appended line 3
    assert [f.feed_seq for f in client.calls[1][0].fills] == [3]

    cp_clean = json.loads(cp_path.read_text(encoding="utf-8"))
    assert cp_clean["in_flight"] is None
    assert cp_clean["confirmed_offset"] == feed_path.stat().st_size


def test_trade_feed_validates_all_nine_outcome_kinds_and_rejects_unknown(tmp_path: Path) -> None:
    assert VALID_TRADE_FILL_OUTCOME_KINDS == frozenset(range(1, 10))
    feed_path = tmp_path / "outcomes.jsonl"
    cp_path = tmp_path / "outcomes.cp.json"

    rows = [_trade_row(i, f"T-{i}", 1) for i in range(1, 10)]
    feed_path.write_text("\n".join(json.dumps(r) for r in rows) + "\n", encoding="utf-8")

    client = _MockTradeFeedServerClient("cfets-1")
    client.forced_outcome_kinds = list(range(1, 10))

    cfg = TradeFeedConfig(
        sidecar_id="tf-outcomes",
        source_id="cfets-1",
        trading_date="2026-06-15",
        checkpoint_path=cp_path,
        input_path=feed_path,
        batch_size=16,
    )
    sidecar = TradeFeedSidecar(cfg, client, health_probe=lambda: _health(1, ULID_1))
    report = sidecar.run_once()
    assert len(report.outcome_counts) == 9
    assert sum(report.outcome_counts.values()) == 9

    # Reject UNSPECIFIED (0) and unknown enum (99) BEFORE advancing checkpoint!
    for bad_kind in (0, 99):
        bad_feed = tmp_path / f"bad_outcome_{bad_kind}.jsonl"
        bad_cp = tmp_path / f"bad_outcome_{bad_kind}.cp.json"
        bad_feed.write_text(json.dumps(_trade_row(1, "T-BAD", 1)) + "\n", encoding="utf-8")
        bad_client = _MockTradeFeedServerClient("cfets-1")
        bad_client.forced_outcome_kinds = [bad_kind]
        bad_sidecar = TradeFeedSidecar(
            TradeFeedConfig(
                sidecar_id="tf-bad",
                source_id="cfets-1",
                trading_date="2026-06-15",
                checkpoint_path=bad_cp,
                input_path=bad_feed,
            ),
            bad_client,
            health_probe=lambda: _health(1, ULID_1),
        )
        with pytest.raises(SidecarError, match="unsupported TradeFillOutcomeKind"):
            bad_sidecar.run_once()
        # Checkpoint MUST NOT have advanced confirmed_offset or cleared in_flight!
        cp_after = json.loads(bad_cp.read_text(encoding="utf-8"))
        assert cp_after["confirmed_offset"] == 0
        assert cp_after["in_flight"] is not None

    # Direct validator test for missing feed_status & outcome identity mismatch
    req = service_pb2.IngestTradeFeedRequest(source_id="cfets-1")
    with pytest.raises(SidecarError, match="missing feed_status"):
        validate_trade_feed_result(req, service_pb2.IngestTradeFeedResult())


def test_trade_feed_rollover_quarantines_in_flight_and_never_replays_to_new_epoch(
    tmp_path: Path,
) -> None:
    day1_path = tmp_path / "day1.jsonl"
    day2_path = tmp_path / "day2.jsonl"
    cp_path = tmp_path / "rollover.cp.json"

    day1_path.write_text(json.dumps(_trade_row(1, "DAY1-T1", 1)) + "\n", encoding="utf-8")
    day2_path.write_text(json.dumps(_trade_row(1, "DAY2-T1", 1)) + "\n", encoding="utf-8")

    client = _MockTradeFeedServerClient("cfets-1")
    client.pre_rpc_errors = [TimeoutError("day1 crash before send")]

    sidecar_d1 = TradeFeedSidecar(
        TradeFeedConfig(
            sidecar_id="tf-roll",
            source_id="cfets-1",
            trading_date="2026-06-15",
            checkpoint_path=cp_path,
            input_path=day1_path,
            backoff=BackoffPolicy(max_retries=0, initial_delay_ms=10, max_delay_ms=10, sleep_fn=lambda _: None),
        ),
        client,
        health_probe=lambda: _health(1, ULID_1),
    )
    with pytest.raises(TimeoutError):
        sidecar_d1.run_once()

    assert json.loads(cp_path.read_text(encoding="utf-8"))["in_flight"] is not None

    # Day 2 on epoch 2 / ULID_2: first reject yesterday's input file!
    client.calls.clear()
    sidecar_d2_bad = TradeFeedSidecar(
        TradeFeedConfig(
            sidecar_id="tf-roll",
            source_id="cfets-1",
            trading_date="2026-06-16",
            checkpoint_path=cp_path,
            input_path=day1_path,  # yesterday's file!
        ),
        client,
        health_probe=lambda: _health(2, ULID_2),
    )
    with pytest.raises(LedgerRolloverValidationError, match="input_fingerprint"):
        sidecar_d2_bad.run_once()
    assert client.calls == []

    # Day 2 with new input file: MUST quarantine Day 1 in_flight and send ONLY Day 2 fill!
    assert compute_file_fingerprint(day1_path) != compute_file_fingerprint(day2_path)
    sidecar_d2_ok = TradeFeedSidecar(
        TradeFeedConfig(
            sidecar_id="tf-roll",
            source_id="cfets-1",
            trading_date="2026-06-16",
            checkpoint_path=cp_path,
            input_path=day2_path,
        ),
        client,
        health_probe=lambda: _health(2, ULID_2),
    )
    rep_d2 = sidecar_d2_ok.run_once()
    assert rep_d2.quarantined_previous_in_flight is True
    assert rep_d2.replayed_in_flight is False
    assert rep_d2.ledger_epoch == 2
    assert rep_d2.ledger_instance_id == ULID_2
    assert len(client.calls) == 1
    sent_req, sent_ledger = client.calls[0]
    assert sent_ledger == (2, ULID_2)
    assert [f.external_trade_id for f in sent_req.fills] == ["DAY2-T1"]
    assert ("cfets-1", "DAY1-T1", 1) not in client.seen_biz

    cp_d2 = json.loads(cp_path.read_text(encoding="utf-8"))
    assert cp_d2["previous_epoch_summary"]["quarantined_in_flight"]["fills_meta"][0]["external_trade_id"] == "DAY1-T1"
