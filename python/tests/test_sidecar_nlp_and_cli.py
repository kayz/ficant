"""Unit & live ficant-server tests for BP-4 NLP draft gateway (F-20), write fences (F-08), and unified CLI."""

from __future__ import annotations

from collections.abc import Callable, Iterator
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from decimal import Decimal
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import threading
import time
from typing import Any

from google.protobuf.timestamp_pb2 import Timestamp
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
from ficant.market.v1 import definition_pb2, regulatory_rule_pb2  # noqa: E402
from ficant.rates.v1 import analytics_pb2 as rates_pb2  # noqa: E402
import ficant_sdk  # noqa: E402
from ficant_sdk import (  # noqa: E402
    AccountUnitsResolver,
    BackoffPolicy,
    BlotterError,
    DeskBlotterClient,
    JsonCheckpointStore,
    NlpDraftBatchReport,
    NlpDraftGatewayConfig,
    NlpDraftGatewaySidecar,
    NlpDraftOnlyClient,
    QuoteBatchReport,
    QuoteStreamConfig,
    QuoteStreamSidecar,
    TradeFeedBatchReport,
    TradeFeedConfig,
    TradeFeedSidecar,
    decimal_to_pb,
    default_reference_units,
    parse_market_time,
)
from ficant_sdk.blotter import (  # noqa: E402
    AUTHORIZATION_HEADER,
    EXPECTED_LEDGER_EPOCH_HEADER,
    EXPECTED_LEDGER_INSTANCE_HEADER,
    FICANT_CREDENTIAL_HEADER,
)
from ficant_sdk.sidecar import (  # noqa: E402
    MAX_WEBHOOK_RESPONSE_BYTES,
    NLP_ALLOWED_RPC_METHODS,
    PARSE_NLP_INTENT_METHOD,
    SUBMIT_DRAFT_EARMARK_METHOD,
    HealthCheckError,
    HealthzStatus,
    LedgerEpochRegressionError,
    LedgerInstanceMismatchError,
    SidecarError,
    cli_main,
    parse_token_nlp_intent,
    probe_healthz,
    validate_nlp_proposal,
    validate_source_text_anchors,
)

SHANGHAI_TZ = timezone(timedelta(hours=8))
REGULATORY_PACK_FIXTURE = (
    REPO_ROOT / "apps" / "desk-web" / "e2e" / "fixtures" / "regulatory-rule-pack.json"
)

TRADER_TOK = "bp4-t005-trader-secret-token-001"
OPERATOR_TOK = "bp4-t005-operator-secret-token-002"
FEED_TOK = "bp4-t005-feed-secret-token-003"
PARSER_TOK = "bp4-t005-parser-secret-token-004"

ULID_1 = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
ULID_2 = "01ARZ3NDEKTSV4RRFFQ69G5FAW"

VECTOR_CN_COMPACT = "买入十年国债BOND_10Y面额一千万"
VECTOR_HYBRID_KV = (
    "买入 T+0 INST=BOND_10Y NOTIONAL=10000000 CLEAN=100.25 TARGET=COMMITTED"
)


# ======================================================================
# Shared Live Subprocess & Fixture Helpers
# ======================================================================


def _ulid(suffix: str) -> common_pb2.Ulid:
    padded = suffix.upper().rjust(2, "0")[:2]
    return common_pb2.Ulid(value=f"01ARZ3NDEKTSV4RRFFQ69G5F{padded}")


def _units() -> rates_pb2.AnalysisUnits:
    base = default_reference_units()
    base.years.CopyFrom(common_pb2.UnitRef(unit_id=_ulid("Y1"), version=1))
    base.years_squared.CopyFrom(common_pb2.UnitRef(unit_id=_ulid("Y2"), version=1))
    return base


def _to_dec(value: common_pb2.DecimalValue) -> Decimal:
    return Decimal(value.coefficient).scaleb(-value.scale)


def _shanghai_time(
    y: int, m: int, d: int, hh: int = 10, mm: int = 0, ss: int = 0
) -> common_pb2.MarketTime:
    local_dt = datetime(y, m, d, hh, mm, ss, tzinfo=SHANGHAI_TZ)
    utc_dt = local_dt.astimezone(timezone.utc)
    ts = Timestamp()
    ts.FromDatetime(utc_dt)
    return common_pb2.MarketTime(
        instant=ts,
        market_timezone="Asia/Shanghai",
        local_trading_date=f"{y:04d}-{m:02d}-{d:02d}",
    )


def _fresh_rfc3339(seconds_ago: int = 3) -> str:
    dt = datetime.now(timezone.utc) - timedelta(seconds=seconds_ago)
    return dt.astimezone(SHANGHAI_TZ).strftime("%Y-%m-%dT%H:%M:%S.%f+08:00")


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def _server_binary() -> Path:
    exe = "ficant-server.exe" if sys.platform == "win32" else "ficant-server"
    path = REPO_ROOT / "target" / "debug" / exe
    if not path.exists():
        subprocess.run(
            ["cargo", "build", "--locked", "--offline", "--bin", "ficant-server"],
            cwd=REPO_ROOT,
            check=True,
        )
    assert path.exists(), f"expected ficant-server binary at {path}"
    return path


def _sidecar_console_script() -> Path:
    exe = "ficant-sidecar.exe" if sys.platform == "win32" else "ficant-sidecar"
    candidate = Path(sys.executable).parent / exe
    assert candidate.exists(), (
        f"expected installed ficant-sidecar script at {candidate}; "
        "run `uv sync --offline --locked --project python`"
    )
    return candidate


def _wait_tcp_ready(
    port: int, proc: subprocess.Popen[bytes], timeout_s: float = 10.0
) -> None:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            stderr = (
                proc.stderr.read().decode("utf-8", errors="replace")
                if proc.stderr
                else ""
            )
            raise RuntimeError(
                f"ficant-server exited early with code {proc.returncode}: {stderr}"
            )
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.05)
    proc.kill()
    proc.wait(timeout=5)
    raise RuntimeError(f"ficant-server did not listen on 127.0.0.1:{port} in time")


def _stop_proc(proc: subprocess.Popen[bytes]) -> None:
    if proc.poll() is None:
        if proc.stdin is not None:
            try:
                proc.stdin.write(b"SHUTDOWN\n")
                proc.stdin.flush()
                proc.wait(timeout=8)
                return
            except Exception:
                pass
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)


def _build_regulatory_rule_pack_binding() -> dict[str, Any]:
    """Build RegulatoryRulePack policy binding strictly from e2e fixture + protobuf encoding."""
    fixture_meta = json.loads(REGULATORY_PACK_FIXTURE.read_text(encoding="utf-8"))
    raw = regulatory_rule_pb2.RegulatoryRulePack(
        constraints=[
            regulatory_rule_pb2.ConstraintDefinition(
                limit_code="MAX_INST_FACE",
                measure=regulatory_rule_pb2.CONSTRAINT_MEASURE_INSTRUMENT_FACE,
            )
        ]
    ).SerializeToString()
    return {
        **fixture_meta,
        "content_hash_sha256_hex": hashlib.sha256(raw).hexdigest(),
        "content_hex": raw.hex(),
    }


def _write_credentials(
    path: Path, *, accounts: list[str], sources: list[str]
) -> Path:
    def _sha(tok: str) -> str:
        return hashlib.sha256(tok.encode("utf-8")).hexdigest()

    doc = {
        "schema_version": 1,
        "credentials": [
            {
                "actor_id": "trader-1",
                "role": "TRADER",
                "token_sha256_hex": _sha(TRADER_TOK),
                "allowed_accounts": accounts,
                "allowed_sources": sources,
            },
            {
                "actor_id": "operator-1",
                "role": "OPERATOR",
                "token_sha256_hex": _sha(OPERATOR_TOK),
                "allowed_accounts": accounts,
                "allowed_sources": sources,
            },
            {
                "actor_id": "feed-1",
                "role": "FEED_ADAPTER",
                "token_sha256_hex": _sha(FEED_TOK),
                "allowed_accounts": accounts,
                "allowed_sources": sources,
            },
            {
                "actor_id": "parser-1",
                "role": "NLP_PARSER",
                "token_sha256_hex": _sha(PARSER_TOK),
                "allowed_accounts": accounts,
                "allowed_sources": [],
            },
        ],
    }
    path.write_text(json.dumps(doc, indent=2), encoding="utf-8")
    return path


def _write_policy(
    path: Path,
    *,
    default_quote_source_id: str = "SRC-A",
) -> Path:
    doc: dict[str, Any] = {
        "schema_version": 1,
        "policy_version": 1,
        "policy_profile": "A",
        "allowed_hedge_product_codes": ["T", "TF", "TL"],
        "blotter_config": {
            "clean_price_tolerance": "0.05",
            "yield_tolerance": "0.0005",
            "committed_overdue_seconds": 3600,
            "pre_close_alert_time": "16:30:00",
            "feed_staleness_seconds": 300,
        },
        "default_account_policy": {
            "default_quote_source_id": default_quote_source_id,
            "instrument_quote_sources": {},
            "quote_staleness_seconds": 300,
            "default_draft_hold_ttl_seconds": 1800,
            "limit_ceilings": {"MAX_INST_FACE": "500000000"},
            "regulatory_rule_pack": _build_regulatory_rule_pack_binding(),
        },
        "accounts": {},
    }
    path.write_text(json.dumps(doc, indent=2), encoding="utf-8")
    return path


def _make_bond(
    *,
    ulid_suffix: str,
    maturity: str,
    coupon_rate: str,
    units: rates_pb2.AnalysisUnits,
) -> definition_pb2.Bond:
    return definition_pb2.Bond(
        instrument=common_pb2.VersionRef(id=_ulid(ulid_suffix), version=1),
        first_issue_date="2025-01-15",
        current_issue_date="2025-01-15",
        maturity_date=maturity,
        face_value=decimal_to_pb("100", units.currency_amount),
        cumulative_issued_amount=decimal_to_pb("50000000000", units.currency_amount),
        tax_attributes=definition_pb2.BondTaxAttributes(
            value_added_tax_status=definition_pb2.VALUE_ADDED_TAX_STATUS_EXEMPT,
            income_tax_status=definition_pb2.INCOME_TAX_STATUS_EXEMPT,
        ),
        coupon_rate=decimal_to_pb(coupon_rate, units.rate),
        coupon_frequency=definition_pb2.BOND_COUPON_FREQUENCY_SEMIANNUAL,
        day_count=definition_pb2.BOND_DAY_COUNT_CONVENTION_ACT_ACT_BOND_ISMA,
        business_day=definition_pb2.BOND_BUSINESS_DAY_CONVENTION_FOLLOWING,
    )


def _make_calendar() -> definition_pb2.Calendar:
    return definition_pb2.Calendar(
        calendar_id=_ulid("C1"),
        version=1,
        market="INTERBANK",
        market_timezone="Asia/Shanghai",
        effective_from=_shanghai_time(2025, 1, 1, 0, 0, 0),
        effective_to=_shanghai_time(2036, 1, 1, 0, 0, 0),
        sessions=[
            definition_pb2.CalendarSession(
                local_date="2025-01-01",
                open_local_time="09:00:00",
                close_local_time="17:00:00",
                closed=False,
            ),
            definition_pb2.CalendarSession(
                local_date="2035-12-31",
                open_local_time="09:00:00",
                close_local_time="17:00:00",
                closed=False,
            ),
        ],
    )


def _make_opening_base(
    account_id: str,
    *,
    trading_date: str = "2026-06-15",
    next_settlement_date: str = "2026-06-16",
    source_id: str = "FEED-1",
) -> model_pb2.OpeningPositionBase:
    units = _units()
    y, m, d = (int(part) for part in trading_date.split("-"))
    return model_pb2.OpeningPositionBase(
        account_id=account_id,
        base_id=_ulid("01"),
        trading_date=trading_date,
        next_settlement_date=next_settlement_date,
        effective_from=_shanghai_time(y, m, d, 9, 0, 0),
        watermark_time=_shanghai_time(y, m, d, 9, 0, 0),
        watermark_source_seq=0,
        source_id=source_id,
        opening_cash_t0=decimal_to_pb("500000000", units.currency_amount),
        opening_net_cash_delta_t1=decimal_to_pb("10000000", units.currency_amount),
        balances=[
            model_pb2.OpeningInstrumentBalance(
                instrument_code="BOND_10Y",
                opening_economic_notional=decimal_to_pb(
                    "100000000", units.currency_amount
                ),
                opening_available_notional=decimal_to_pb(
                    "100000000", units.currency_amount
                ),
                opening_net_t0_notional=decimal_to_pb("0", units.currency_amount),
                opening_net_t1_notional=decimal_to_pb("0", units.currency_amount),
            ),
            model_pb2.OpeningInstrumentBalance(
                instrument_code="BOND_5Y",
                opening_economic_notional=decimal_to_pb(
                    "80000000", units.currency_amount
                ),
                opening_available_notional=decimal_to_pb(
                    "80000000", units.currency_amount
                ),
                opening_net_t0_notional=decimal_to_pb("0", units.currency_amount),
                opening_net_t1_notional=decimal_to_pb("0", units.currency_amount),
            ),
        ],
        instruments=[
            model_pb2.InstrumentCatalogEntry(
                instrument_code="BOND_10Y",
                bond=_make_bond(
                    ulid_suffix="B1",
                    maturity="2035-01-15",
                    coupon_rate="0.025",
                    units=units,
                ),
                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_SOVEREIGN_OR_POLICY_FIXED,
                calendar_requirement=rates_pb2.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
            ),
            model_pb2.InstrumentCatalogEntry(
                instrument_code="BOND_5Y",
                bond=_make_bond(
                    ulid_suffix="B2",
                    maturity="2030-01-15",
                    coupon_rate="0.022",
                    units=units,
                ),
                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_SOVEREIGN_OR_POLICY_FIXED,
                calendar_requirement=rates_pb2.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
            ),
        ],
        calendar=_make_calendar(),
        default_calendar_requirement=rates_pb2.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
        units=units,
        baseline_evidence=[],
        source_start_after_feed_seq=0,
    )


@contextmanager
def _run_server(
    *,
    db_path: Path,
    policy_path: Path,
    creds_path: Path,
    extra_env: dict[str, str] | None = None,
) -> Iterator[tuple[subprocess.Popen[bytes], str, int]]:
    port = _free_port()
    addr = f"127.0.0.1:{port}"
    env = {
        k: v
        for k, v in os.environ.items()
        if not k.startswith("FICANT_BLOTTER_")
        and not k.startswith("FICANT_SPI_")
        and not k.startswith("FICANT_SERVE_WEB_")
        and not k.startswith("FICANT_STATIC_WEB_")
        and k != "FICANT_SERVER_ADDR"
    }
    env["FICANT_SERVER_ADDR"] = addr
    env["FICANT_BLOTTER_DB_PATH"] = str(db_path)
    env["FICANT_BLOTTER_POLICY_PATH"] = str(policy_path)
    env["FICANT_BLOTTER_CREDENTIALS_PATH"] = str(creds_path)
    env["FICANT_BLOTTER_WATCHDOG_INTERVAL_MS"] = "0"
    env["FICANT_ENABLE_STDIN_SHUTDOWN"] = "1"
    if extra_env:
        env.update(extra_env)

    proc = subprocess.Popen(
        [str(_server_binary())],
        cwd=REPO_ROOT,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        _wait_tcp_ready(port, proc)
        yield proc, addr, port
    finally:
        _stop_proc(proc)


@contextmanager
def _run_webhook_server(
    handler_fn: Callable[[dict[str, Any], int], tuple[int, bytes, dict[str, str]]],
) -> Iterator[tuple[str, list[dict[str, Any]]]]:
    """Run a deterministic local loopback HTTP webhook server."""
    received: list[dict[str, Any]] = []
    lock = threading.Lock()

    class _Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:  # noqa: N802
            length = int(self.headers.get("Content-Length", "0"))
            raw = self.rfile.read(length)
            req_json = json.loads(raw.decode("utf-8"))
            with lock:
                received.append(req_json)
                call_num = len(received)
            status, body, extra_headers = handler_fn(req_json, call_num)
            self.send_response(status)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            for hk, hv in extra_headers.items():
                self.send_header(hk, hv)
            if "Content-Length" not in extra_headers:
                self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            if body:
                self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:  # noqa: A002
            return

    server = HTTPServer(("127.0.0.1", 0), _Handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{port}/parse", received
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def _health(epoch: int = 1, instance_id: str = ULID_1) -> HealthzStatus:
    return HealthzStatus(
        status="ok",
        service="ficant-server",
        storage_mode="persistent_wal",
        blotter_healthy=True,
        ledger_epoch=epoch,
        ledger_instance_id=instance_id,
        web_ui_mode="disabled",
        embedded_assets_count=0,
    )


# ======================================================================
# 1. Public Exports & Structural 2-Path NlpDraftOnlyClient Isolation
# ======================================================================


def test_sdk_and_sidecar_public_exports() -> None:
    expected_top_level = [
        "QuoteStreamSidecar",
        "QuoteStreamConfig",
        "QuoteBatchReport",
        "TradeFeedSidecar",
        "TradeFeedConfig",
        "TradeFeedBatchReport",
        "NlpDraftGatewaySidecar",
        "NlpDraftGatewayConfig",
        "NlpDraftBatchReport",
        "NlpDraftOnlyClient",
        "default_reference_units",
        "AccountUnitsResolver",
        "decimal_to_pb",
        "parse_market_time",
        "JsonCheckpointStore",
        "BackoffPolicy",
    ]
    for name in expected_top_level:
        assert hasattr(ficant_sdk, name), f"ficant_sdk missing {name}"
        assert hasattr(ficant_sdk.sidecar, name), f"ficant_sdk.sidecar missing {name}"

    assert QuoteStreamSidecar is ficant_sdk.sidecar.QuoteStreamSidecar
    assert TradeFeedSidecar is ficant_sdk.sidecar.TradeFeedSidecar
    assert NlpDraftGatewaySidecar is ficant_sdk.sidecar.NlpDraftGatewaySidecar
    assert NlpDraftOnlyClient is ficant_sdk.sidecar.NlpDraftOnlyClient
    assert AccountUnitsResolver is ficant_sdk.sidecar.AccountUnitsResolver
    assert JsonCheckpointStore is ficant_sdk.sidecar.JsonCheckpointStore
    assert BackoffPolicy is ficant_sdk.sidecar.BackoffPolicy
    assert QuoteBatchReport is ficant_sdk.sidecar.QuoteBatchReport
    assert TradeFeedBatchReport is ficant_sdk.sidecar.TradeFeedBatchReport
    assert NlpDraftBatchReport is ficant_sdk.sidecar.NlpDraftBatchReport
    assert QuoteStreamConfig is ficant_sdk.sidecar.QuoteStreamConfig
    assert TradeFeedConfig is ficant_sdk.sidecar.TradeFeedConfig
    assert NlpDraftGatewayConfig is ficant_sdk.sidecar.NlpDraftGatewayConfig


class _RecordingChannel(grpc.Channel):
    """Recording grpc.Channel verifying exact RPC path registrations and metadata."""

    def __init__(self) -> None:
        self.unary_unary_paths: list[str] = []
        self.other_paths: list[tuple[str, str]] = []
        self.invocations: list[tuple[str, Any, tuple[tuple[str, str], ...]]] = []
        self.closed = False

    def unary_unary(
        self,
        method: str,
        request_serializer: Any = None,
        response_deserializer: Any = None,
        _registered_method: Any = False,
    ) -> Any:
        self.unary_unary_paths.append(method)

        def _invoke(request: Any, *, metadata: Any = None, timeout: Any = None) -> Any:
            md = tuple(metadata or ())
            self.invocations.append((method, request, md))
            if method == PARSE_NLP_INTENT_METHOD:
                units = default_reference_units()
                proposal = parse_token_nlp_intent(
                    request_id=request.request_id,
                    account_id=request.account_id,
                    raw_source_text=request.raw_source_text,
                    units=units,
                )
                return spi_pb2.ParseNlpIntentResponse(value=proposal)
            if method == SUBMIT_DRAFT_EARMARK_METHOD:
                return service_pb2.SubmitDraftEarmarkResponse(
                    value=service_pb2.SubmitDraftEarmarkResult(
                        committed_seq=7,
                        replayed=False,
                        draft=model_pb2.DraftEarmarkRecord(
                            draft_id=common_pb2.Ulid(value=ULID_1),
                            draft_revision=1,
                            account_id=request.account_id,
                        ),
                    )
                )
            raise AssertionError(f"unexpected RPC path called: {method}")

        return _invoke

    def unary_stream(self, method: str, *args: Any, **kwargs: Any) -> Any:
        self.other_paths.append(("unary_stream", method))
        raise AssertionError("NlpDraftOnlyClient must not register unary_stream")

    def stream_unary(self, method: str, *args: Any, **kwargs: Any) -> Any:
        self.other_paths.append(("stream_unary", method))
        raise AssertionError("NlpDraftOnlyClient must not register stream_unary")

    def stream_stream(self, method: str, *args: Any, **kwargs: Any) -> Any:
        self.other_paths.append(("stream_stream", method))
        raise AssertionError("NlpDraftOnlyClient must not register stream_stream")

    def subscribe(self, callback: Any, try_to_connect: bool = False) -> None:
        return

    def unsubscribe(self, callback: Any) -> None:
        return

    def close(self) -> None:
        self.closed = True


def test_nlp_draft_only_client_registers_strictly_two_unary_paths_and_zero_snapshots() -> None:
    rec = _RecordingChannel()
    with NlpDraftOnlyClient(
        channel=rec,
        bearer_token=PARSER_TOK,
        expected_ledger=(3, ULID_1),
    ) as client:
        assert set(rec.unary_unary_paths) == NLP_ALLOWED_RPC_METHODS
        assert len(rec.unary_unary_paths) == 2
        assert rec.other_paths == []
        for forbidden_attr in (
            "get_account_snapshot",
            "confirm_draft_earmark",
            "create_hold",
            "create_committed",
            "resolve_pending_match",
            "ingest_market_quotes",
            "ingest_trade_feed",
        ):
            assert not hasattr(client, forbidden_attr)

        proposal = client.parse_nlp_intent(
            spi_pb2.ParseNlpIntentRequest(
                request_id="req-1",
                account_id="acct-1",
                raw_source_text=VECTOR_HYBRID_KV,
            )
        )
        assert proposal.instrument_code == "BOND_10Y"

        res = client.submit_draft_earmark(
            service_pb2.SubmitDraftEarmarkRequest(
                idempotency_key="idem-1",
                account_id="acct-1",
                proposal=proposal,
            )
        )
        assert res.committed_seq == 7

    # Externally provided channel must not be closed by NlpDraftOnlyClient
    assert rec.closed is False

    assert len(rec.invocations) == 2
    parse_method, _parse_req, parse_md = rec.invocations[0]
    assert parse_method == PARSE_NLP_INTENT_METHOD
    assert (AUTHORIZATION_HEADER, f"Bearer {PARSER_TOK}") in parse_md

    submit_method, _submit_req, submit_md = rec.invocations[1]
    assert submit_method == SUBMIT_DRAFT_EARMARK_METHOD
    assert (AUTHORIZATION_HEADER, f"Bearer {PARSER_TOK}") in submit_md
    assert (EXPECTED_LEDGER_EPOCH_HEADER, "3") in submit_md
    assert (EXPECTED_LEDGER_INSTANCE_HEADER, ULID_1) in submit_md

    # Verify x-ficant-credential mode and missing token rejection
    rec_x = _RecordingChannel()
    client_x = NlpDraftOnlyClient(
        channel=rec_x,
        x_ficant_credential=PARSER_TOK,
        header_mode="x-ficant-credential",
    )
    client_x.parse_nlp_intent(
        spi_pb2.ParseNlpIntentRequest(
            request_id="req-x", account_id="acct-1", raw_source_text=VECTOR_CN_COMPACT
        )
    )
    assert (FICANT_CREDENTIAL_HEADER, PARSER_TOK) in rec_x.invocations[0][2]

    with pytest.raises(ValueError, match="credential"):
        NlpDraftOnlyClient(channel=_RecordingChannel())


# ======================================================================
# 2. Both Blueprint UTF-8 Vectors & Strict Webhook/Anchor Negative Matrix
# ======================================================================


def test_blueprint_utf8_byte_anchor_vectors() -> None:
    units = default_reference_units()

    # Vector 1: 41 UTF-8 bytes, BOND_10Y slice [18, 26)
    v1_bytes = VECTOR_CN_COMPACT.encode("utf-8")
    assert len(v1_bytes) == 41
    assert v1_bytes[18:26] == b"BOND_10Y"

    p1 = parse_token_nlp_intent(
        request_id="vec-1",
        account_id="acct-1",
        raw_source_text=VECTOR_CN_COMPACT,
        units=units,
    )
    assert p1.instrument_code == "BOND_10Y"
    assert p1.side == model_pb2.TRADE_SIDE_BUY
    assert _to_dec(p1.notional) == Decimal("10000000")
    inst_anchors_1 = [a for a in p1.anchors if a.field_name == "instrument_code"]
    assert len(inst_anchors_1) == 1
    assert (
        inst_anchors_1[0].byte_start,
        inst_anchors_1[0].byte_end,
        inst_anchors_1[0].span_text,
    ) == (18, 26, "BOND_10Y")

    # Vector 2: 72 UTF-8 bytes, BOND_10Y slice [16, 24)
    v2_bytes = VECTOR_HYBRID_KV.encode("utf-8")
    assert len(v2_bytes) == 72
    assert v2_bytes[16:24] == b"BOND_10Y"

    p2 = parse_token_nlp_intent(
        request_id="vec-2",
        account_id="acct-1",
        raw_source_text=VECTOR_HYBRID_KV,
        units=units,
    )
    assert p2.instrument_code == "BOND_10Y"
    assert p2.side == model_pb2.TRADE_SIDE_BUY
    assert p2.settlement_bucket == model_pb2.SETTLEMENT_BUCKET_T_PLUS_0
    assert _to_dec(p2.notional) == Decimal("10000000")
    assert _to_dec(p2.quote_terms.clean_price_per_100) == Decimal("100.25")
    assert p2.suggested_target_state == model_pb2.DRAFT_TARGET_STATE_COMMITTED
    inst_anchors_2 = [a for a in p2.anchors if a.field_name == "instrument_code"]
    assert len(inst_anchors_2) == 1
    assert (
        inst_anchors_2[0].byte_start,
        inst_anchors_2[0].byte_end,
        inst_anchors_2[0].span_text,
    ) == (16, 24, "BOND_10Y")


def test_anchor_and_proposal_negative_validations() -> None:
    units = default_reference_units()

    # Mid-codepoint offset (byte 1 inside 3-byte '买')
    with pytest.raises(ValueError, match="UTF-8 codepoint boundaries"):
        validate_source_text_anchors(
            VECTOR_CN_COMPACT,
            [
                model_pb2.SourceTextAnchor(
                    field_name="side", byte_start=1, byte_end=6, span_text="入"
                )
            ],
        )

    # Reversed / empty / out-of-bounds spans
    with pytest.raises(ValueError, match="out-of-range"):
        validate_source_text_anchors(
            VECTOR_CN_COMPACT,
            [
                model_pb2.SourceTextAnchor(
                    field_name="instrument_code",
                    byte_start=26,
                    byte_end=18,
                    span_text="",
                )
            ],
        )
    with pytest.raises(ValueError, match="out-of-range"):
        validate_source_text_anchors(
            VECTOR_CN_COMPACT,
            [
                model_pb2.SourceTextAnchor(
                    field_name="notional",
                    byte_start=32,
                    byte_end=45,
                    span_text="一千万",
                )
            ],
        )

    # Mismatched span_text echo
    with pytest.raises(ValueError, match="does not match"):
        validate_source_text_anchors(
            VECTOR_CN_COMPACT,
            [
                model_pb2.SourceTextAnchor(
                    field_name="instrument_code",
                    byte_start=18,
                    byte_end=26,
                    span_text="BOND_05Y",
                )
            ],
        )

    # Overlapping anchors
    with pytest.raises(ValueError, match="overlapping"):
        validate_source_text_anchors(
            VECTOR_CN_COMPACT,
            [
                model_pb2.SourceTextAnchor(
                    field_name="a", byte_start=0, byte_end=18, span_text="买入十年国债"
                ),
                model_pb2.SourceTextAnchor(
                    field_name="b",
                    byte_start=12,
                    byte_end=26,
                    span_text="国债BOND_10Y",
                ),
            ],
        )

    # Duplicate field_name
    with pytest.raises(ValueError, match="duplicate"):
        validate_source_text_anchors(
            VECTOR_HYBRID_KV,
            [
                model_pb2.SourceTextAnchor(
                    field_name="instrument_code",
                    byte_start=16,
                    byte_end=24,
                    span_text="BOND_10Y",
                ),
                model_pb2.SourceTextAnchor(
                    field_name="instrument_code",
                    byte_start=34,
                    byte_end=42,
                    span_text="10000000",
                ),
            ],
        )

    # Confidence out of [0, 1] and wrong unit
    p_valid = parse_token_nlp_intent(
        request_id="v",
        account_id="acct-1",
        raw_source_text=VECTOR_HYBRID_KV,
        units=units,
    )
    p_hi = spi_pb2.NlpEarmarkProposal()
    p_hi.CopyFrom(p_valid)
    p_hi.confidence_score.CopyFrom(decimal_to_pb("1.01", units.dimensionless))
    with pytest.raises(ValueError, match=r"\[0, 1\]"):
        validate_nlp_proposal(
            p_hi,
            expected_account_id="acct-1",
            expected_raw_source_text=VECTOR_HYBRID_KV,
            units=units,
        )

    p_neg = spi_pb2.NlpEarmarkProposal()
    p_neg.CopyFrom(p_valid)
    p_neg.confidence_score.CopyFrom(decimal_to_pb("-0.01", units.dimensionless))
    with pytest.raises(ValueError, match=r"\[0, 1\]"):
        validate_nlp_proposal(
            p_neg,
            expected_account_id="acct-1",
            expected_raw_source_text=VECTOR_HYBRID_KV,
            units=units,
        )

    p_bad_unit = spi_pb2.NlpEarmarkProposal()
    p_bad_unit.CopyFrom(p_valid)
    p_bad_unit.confidence_score.CopyFrom(decimal_to_pb("0.9", units.rate))
    with pytest.raises(ValueError, match="dimensionless"):
        validate_nlp_proposal(
            p_bad_unit,
            expected_account_id="acct-1",
            expected_raw_source_text=VECTOR_HYBRID_KV,
            units=units,
        )


def _base_webhook_payload(req_json: dict[str, Any]) -> dict[str, Any]:
    return {
        "request_id": req_json["request_id"],
        "parser_source_id": "intranet-webhook-v1",
        "model_version": "2026.11",
        "account_id": req_json["account_id"],
        "raw_source_text": req_json["raw_source_text"],
        "confidence_score": "0.92",
        "instrument_code": "BOND_10Y",
        "side": "BUY",
        "settlement_bucket": "T+0",
        "notional": "10000000",
        "quote_terms": {"clean_price_per_100": "100.25"},
        "suggested_target_state": "COMMITTED",
        "anchors": [
            {
                "field_name": "instrument_code",
                "byte_start": 16,
                "byte_end": 24,
                "span_text": "BOND_10Y",
            }
        ],
    }


@pytest.mark.parametrize(
    ("case_name", "mutator", "expected_exc"),
    [
        (
            "unknown_top_key",
            lambda p: (200, json.dumps({**p, "unexpected_key": 1}).encode("utf-8"), {}),
            SidecarError,
        ),
        (
            "unknown_nested_quote_terms_key",
            lambda p: (
                200,
                json.dumps(
                    {
                        **p,
                        "quote_terms": {
                            "clean_price_per_100": "100.25",
                            "extra_field": "x",
                        },
                    }
                ).encode("utf-8"),
                {},
            ),
            SidecarError,
        ),
        (
            "unknown_nested_anchor_key",
            lambda p: (
                200,
                json.dumps(
                    {
                        **p,
                        "anchors": [{**p["anchors"][0], "extra_anchor_key": True}],
                    }
                ).encode("utf-8"),
                {},
            ),
            SidecarError,
        ),
        (
            "oversized_response_over_256kib",
            lambda _p: (200, b"{" + (b" " * MAX_WEBHOOK_RESPONSE_BYTES) + b"}", {}),
            SidecarError,
        ),
        (
            "tampered_echo_account_id",
            lambda p: (
                200,
                json.dumps({**p, "account_id": "acct-tampered"}).encode("utf-8"),
                {},
            ),
            SidecarError,
        ),
        (
            "tampered_echo_raw_source_text",
            lambda p: (
                200,
                json.dumps(
                    {**p, "raw_source_text": p["raw_source_text"] + " "}
                ).encode("utf-8"),
                {},
            ),
            SidecarError,
        ),
        (
            "mid_codepoint_anchor",
            lambda p: (
                200,
                json.dumps(
                    {
                        **p,
                        "anchors": [
                            {
                                "field_name": "side",
                                "byte_start": 1,
                                "byte_end": 6,
                                "span_text": "入",
                            }
                        ],
                    }
                ).encode("utf-8"),
                {},
            ),
            ValueError,
        ),
        (
            "confidence_above_one",
            lambda p: (
                200,
                json.dumps({**p, "confidence_score": "1.0001"}).encode("utf-8"),
                {},
            ),
            ValueError,
        ),
        (
            "confidence_float_rejected",
            lambda p: (
                200,
                json.dumps({**p, "confidence_score": 0.92}).encode("utf-8"),
                {},
            ),
            ValueError,
        ),
        (
            "redirect_forbidden",
            lambda _p: (302, b"", {"Location": "http://127.0.0.1:9999/other"}),
            SidecarError,
        ),
    ],
)
def test_webhook_adapter_strict_security_and_validation_matrix(
    tmp_path: Path,
    case_name: str,
    mutator: Callable[[dict[str, Any]], tuple[int, bytes, dict[str, str]]],
    expected_exc: type[BaseException],
) -> None:
    def _handler(req_json: dict[str, Any], _call_num: int) -> tuple[int, bytes, dict[str, str]]:
        return mutator(_base_webhook_payload(req_json))

    input_path = tmp_path / f"{case_name}.jsonl"
    input_path.write_text(
        json.dumps(
            {
                "request_id": f"req-{case_name}",
                "account_id": "acct-1",
                "raw_source_text": VECTOR_HYBRID_KV,
            },
            ensure_ascii=False,
        )
        + "\n",
        encoding="utf-8",
    )
    cp_path = tmp_path / f"{case_name}.cp.json"

    with _run_webhook_server(_handler) as (url, _received):
        rec = _RecordingChannel()
        client = NlpDraftOnlyClient(channel=rec, bearer_token=PARSER_TOK)
        sidecar = NlpDraftGatewaySidecar(
            NlpDraftGatewayConfig(
                sidecar_id="nlp-neg",
                trading_date="2026-06-15",
                checkpoint_path=cp_path,
                input_path=input_path,
                adapter="webhook",
                webhook_url=url,
                backoff=BackoffPolicy(max_retries=0),
            ),
            client,
            health_probe=lambda: _health(),
        )
        with pytest.raises(expected_exc):
            sidecar.run_once()
        # No SubmitDraftEarmark RPC was ever sent for invalid webhook output
        assert rec.invocations == []


# ======================================================================
# 3. Live ficant-server Tests: RBAC, Mutating Webhook Crash Recovery,
#    SPI Adapter, Unified CLI Subcommands & Stale Ledger Write Fence
# ======================================================================


def test_live_nlp_rbac_mutating_webhook_crash_recovery_and_altered_request_id(
    tmp_path: Path,
) -> None:
    db_path = tmp_path / "nlp_live.sqlite"
    creds_path = _write_credentials(
        tmp_path / "creds.json", accounts=["acct-1"], sources=["FEED-1", "SRC-A"]
    )
    policy_path = _write_policy(tmp_path / "policy.json")

    with _run_server(db_path=db_path, policy_path=policy_path, creds_path=creds_path) as (
        _proc,
        endpoint,
        _port,
    ):
        health = probe_healthz(endpoint, insecure=True)
        ledger = health.require_healthy_ledger()
        units = _units()

        # Setup opening base + fresh quote
        with DeskBlotterClient(endpoint, bearer_token=OPERATOR_TOK, insecure=True) as op:
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-nlp-live",
                    opening_base=_make_opening_base("acct-1"),
                    expected_account_version=0,
                )
            )
        obs = parse_market_time(_fresh_rfc3339(4))
        rec_t = parse_market_time(_fresh_rfc3339(2))
        with DeskBlotterClient(endpoint, bearer_token=FEED_TOK, insecure=True) as feed:
            feed.ingest_market_quotes(
                service_pb2.IngestMarketQuotesRequest(
                    idempotency_key="q-init-1",
                    account_id="acct-1",
                    source_id="SRC-A",
                    ticks=[
                        spi_pb2.MarketQuoteTick(
                            source_id="SRC-A",
                            quote_seq=1,
                            instrument_code="BOND_10Y",
                            source_type=4,  # CURVE_INTERPOLATION
                            mid_yield_to_maturity=decimal_to_pb("0.025", units.rate),
                            observed_at=obs,
                            received_at=rec_t,
                        )
                    ],
                ),
                expected_ledger=ledger.as_tuple(),
            )

        # Capture baseline position before NLP actions
        with DeskBlotterClient(endpoint, bearer_token=TRADER_TOK, insecure=True) as tr:
            snap_before = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-1")
            ).snapshot
            pos_before = {p.instrument_code: p for p in snap_before.positions}

        # ------------------------------------------------------------------
        # A. Real-server RBAC: NLP_PARSER MUST receive FORBIDDEN on all 4 write RPCs
        # ------------------------------------------------------------------
        with DeskBlotterClient(endpoint, bearer_token=PARSER_TOK, insecure=True) as raw_parser:
            with pytest.raises(BlotterError) as err_confirm:
                raw_parser.confirm_draft_earmark(
                    service_pb2.ConfirmDraftEarmarkRequest(
                        idempotency_key="forbid-confirm",
                        account_id="acct-1",
                        draft_id=_ulid("D1"),
                        expected_draft_revision=1,
                        expected_position_version=1,
                        confirmation_token=common_pb2.Sha256(value=b"\x00" * 32),
                    )
                )
            assert err_confirm.value.code == error_pb2.ERROR_CODE_FORBIDDEN

            with pytest.raises(BlotterError) as err_hold:
                raw_parser.create_hold(
                    service_pb2.CreateHoldRequest(
                        idempotency_key="forbid-hold",
                        account_id="acct-1",
                        expected_position_version=1,
                        instrument_code="BOND_10Y",
                        side=model_pb2.TRADE_SIDE_BUY,
                        settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                        notional=decimal_to_pb("1000000", units.currency_amount),
                        quote_terms=model_pb2.QuoteTerms(
                            clean_price_per_100=decimal_to_pb("100", units.price_per_100)
                        ),
                        ttl_seconds=600,
                    )
                )
            assert err_hold.value.code == error_pb2.ERROR_CODE_FORBIDDEN

            with pytest.raises(BlotterError) as err_committed:
                raw_parser.create_committed(
                    service_pb2.CreateCommittedRequest(
                        idempotency_key="forbid-committed",
                        account_id="acct-1",
                        expected_position_version=1,
                        instrument_code="BOND_10Y",
                        side=model_pb2.TRADE_SIDE_BUY,
                        settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                        notional=decimal_to_pb("1000000", units.currency_amount),
                        quote_terms=model_pb2.QuoteTerms(
                            clean_price_per_100=decimal_to_pb("100", units.price_per_100)
                        ),
                    )
                )
            assert err_committed.value.code == error_pb2.ERROR_CODE_FORBIDDEN

            with pytest.raises(BlotterError) as err_resolve:
                raw_parser.resolve_pending_match(
                    service_pb2.ResolvePendingMatchRequest(
                        idempotency_key="forbid-resolve",
                        account_id="acct-1",
                        pending_match_id=_ulid("P1"),
                        expected_pending_revision=1,
                        expected_position_version=1,
                        action=model_pb2.PENDING_MATCH_RESOLUTION_ACTION_ACCEPT_ORPHAN,
                    )
                )
            assert err_resolve.value.code == error_pb2.ERROR_CODE_FORBIDDEN

        # ------------------------------------------------------------------
        # B1. Mutating webhook + PRE-RPC crash (replayed=False on recovery)
        # ------------------------------------------------------------------
        def _mutating_handler_pre(
            req_json: dict[str, Any], call_num: int
        ) -> tuple[int, bytes, dict[str, str]]:
            payload = _base_webhook_payload(req_json)
            if call_num >= 2:
                # Mutate proposal dramatically on 2nd call to prove 2nd call NEVER happens!
                payload["notional"] = "25000000"
                payload["quote_terms"] = {"clean_price_per_100": "98.50"}
            return (200, json.dumps(payload, ensure_ascii=False).encode("utf-8"), {})

        input_pre = tmp_path / "nlp_pre_crash.jsonl"
        input_pre.write_text(
            json.dumps(
                {
                    "request_id": "req-pre-crash-1",
                    "account_id": "acct-1",
                    "raw_source_text": VECTOR_HYBRID_KV,
                },
                ensure_ascii=False,
            )
            + "\n",
            encoding="utf-8",
        )
        cp_pre = tmp_path / "nlp_pre_crash.cp.json"

        class _PreRpcCrashClient:
            def __init__(self, inner: NlpDraftOnlyClient) -> None:
                self._inner = inner

            def submit_draft_earmark(self, req: Any, **kwargs: Any) -> Any:
                raise RuntimeError("simulated pre-RPC process crash after durable intent")

        with _run_webhook_server(_mutating_handler_pre) as (webhook_url, calls_pre):
            with NlpDraftOnlyClient(
                endpoint, bearer_token=PARSER_TOK, insecure=True
            ) as real_nlp_client:
                cfg_pre = NlpDraftGatewayConfig(
                    sidecar_id="nlp-pre-crash",
                    trading_date="2026-06-15",
                    checkpoint_path=cp_pre,
                    input_path=input_pre,
                    adapter="webhook",
                    webhook_url=webhook_url,
                    backoff=BackoffPolicy(max_retries=0),
                    endpoint=endpoint,
                    insecure=True,
                )
                crashed_sidecar = NlpDraftGatewaySidecar(
                    cfg_pre, _PreRpcCrashClient(real_nlp_client)
                )
                with pytest.raises(RuntimeError, match="pre-RPC process crash"):
                    crashed_sidecar.run_once()

                assert len(calls_pre) == 1
                cp_mid = json.loads(cp_pre.read_text(encoding="utf-8"))
                assert cp_mid["in_flight"] is not None
                assert cp_mid["in_flight"]["request_id"] == "req-pre-crash-1"
                assert PARSER_TOK not in cp_pre.read_text(encoding="utf-8")

                # Restart with healthy client: MUST replay frozen bytes without calling webhook again!
                recovered = NlpDraftGatewaySidecar(cfg_pre, real_nlp_client).run_once()
                assert len(calls_pre) == 1
                assert recovered.replayed_in_flight is True
                assert recovered.drafts_submitted == 1
                assert recovered.server_replayed_count == 0  # Pre-RPC crash -> fresh commit
                assert len(recovered.draft_ids) == 1
                pre_draft_id = recovered.draft_ids[0]

                # Verify altered raw_source_text or account_id with same request_id FAILS CLOSED
                with input_pre.open("a", encoding="utf-8") as fh:
                    fh.write(
                        json.dumps(
                            {
                                "request_id": "req-pre-crash-1",
                                "account_id": "acct-1",
                                "raw_source_text": VECTOR_CN_COMPACT,
                            },
                            ensure_ascii=False,
                        )
                        + "\n"
                    )
                with pytest.raises(SidecarError, match="conflicting reuse"):
                    NlpDraftGatewaySidecar(cfg_pre, real_nlp_client).run_once()
                assert len(calls_pre) == 1

        # ------------------------------------------------------------------
        # B2. Mutating webhook + POST-RPC lost-response crash (replayed=True on recovery)
        # ------------------------------------------------------------------
        input_post = tmp_path / "nlp_post_crash.jsonl"
        input_post.write_text(
            json.dumps(
                {
                    "request_id": "req-post-crash-1",
                    "account_id": "acct-1",
                    "raw_source_text": VECTOR_HYBRID_KV,
                },
                ensure_ascii=False,
            )
            + "\n",
            encoding="utf-8",
        )
        cp_post = tmp_path / "nlp_post_crash.cp.json"

        class _LostResponseClient:
            def __init__(self, inner: NlpDraftOnlyClient) -> None:
                self._inner = inner
                self.first_draft_id: str | None = None

            def submit_draft_earmark(self, req: Any, **kwargs: Any) -> Any:
                # Real RPC SUCCEEDS on ficant-server, then response is lost before caller sees it!
                res = self._inner.submit_draft_earmark(req, **kwargs)
                self.first_draft_id = res.draft.draft_id.value
                raise TimeoutError("simulated lost response after server commit")

        with _run_webhook_server(_mutating_handler_pre) as (webhook_url_2, calls_post):
            with NlpDraftOnlyClient(
                endpoint, bearer_token=PARSER_TOK, insecure=True
            ) as real_nlp_client:
                cfg_post = NlpDraftGatewayConfig(
                    sidecar_id="nlp-post-crash",
                    trading_date="2026-06-15",
                    checkpoint_path=cp_post,
                    input_path=input_post,
                    adapter="webhook",
                    webhook_url=webhook_url_2,
                    backoff=BackoffPolicy(max_retries=0),
                    endpoint=endpoint,
                    insecure=True,
                )
                lost_client = _LostResponseClient(real_nlp_client)
                with pytest.raises(TimeoutError, match="lost response"):
                    NlpDraftGatewaySidecar(cfg_post, lost_client).run_once()

                assert len(calls_post) == 1
                assert lost_client.first_draft_id is not None

                # Restart: replays exact persisted SubmitDraftEarmarkRequest & idempotency_key -> replayed=True!
                rec_post = NlpDraftGatewaySidecar(cfg_post, real_nlp_client).run_once()
                assert len(calls_post) == 1
                assert rec_post.replayed_in_flight is True
                assert rec_post.server_replayed_count == 1
                assert rec_post.drafts_submitted == 1
                assert rec_post.draft_ids == (lost_client.first_draft_id,)

                # Altered account_id on same request_id FAILS CLOSED
                with input_post.open("a", encoding="utf-8") as fh:
                    fh.write(
                        json.dumps(
                            {
                                "request_id": "req-post-crash-1",
                                "account_id": "acct-other",
                                "raw_source_text": VECTOR_HYBRID_KV,
                            },
                            ensure_ascii=False,
                        )
                        + "\n"
                    )
                with pytest.raises(SidecarError, match="conflicting reuse"):
                    NlpDraftGatewaySidecar(cfg_post, real_nlp_client).run_once()
                assert len(calls_post) == 1

        # Verify on server: exactly 2 unique drafts (1 from B1, 1 from B2), both with Proposal A notional=10000000,
        # and ZERO position changes!
        with DeskBlotterClient(endpoint, bearer_token=TRADER_TOK, insecure=True) as tr:
            snap_after = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-1")
            ).snapshot
            drafts_by_id = {d.draft_id.value: d for d in snap_after.pending_drafts}
            assert set(drafts_by_id.keys()) == {pre_draft_id, lost_client.first_draft_id}
            for draft in drafts_by_id.values():
                assert _to_dec(draft.notional) == Decimal("10000000")
                assert _to_dec(draft.quote_terms.clean_price_per_100) == Decimal("100.25")
            pos_after = {p.instrument_code: p for p in snap_after.positions}
            assert (
                pos_after["BOND_10Y"].position_version
                == pos_before["BOND_10Y"].position_version
            )
            assert _to_dec(pos_after["BOND_10Y"].economic_net_notional) == _to_dec(
                pos_before["BOND_10Y"].economic_net_notional
            )
            assert _to_dec(pos_after["BOND_10Y"].available_t0_notional) == _to_dec(
                pos_before["BOND_10Y"].available_t0_notional
            )


def test_live_spi_adapter_against_real_server(tmp_path: Path) -> None:
    db_path = tmp_path / "spi_nlp.sqlite"
    creds_path = _write_credentials(
        tmp_path / "creds.json", accounts=["acct-1"], sources=["FEED-1", "SRC-A"]
    )
    policy_path = _write_policy(tmp_path / "policy.json")

    nlp_csv = tmp_path / "spi_nlp_templates.csv"
    nlp_csv.write_text(
        "account_id,raw_source_text,parser_source_id,model_version,confidence_score,"
        "instrument_code,side,settlement_bucket,notional,clean_price_per_100,"
        "yield_to_maturity,settlement_amount,suggested_target_state,anchor_field,anchor_span\n"
        f"acct-1,{VECTOR_CN_COMPACT},spi-csv-parser,1.0.0,0.96,BOND_10Y,BUY,T_PLUS_0,"
        "10000000,100.25,,,COMMITTED,instrument_code,BOND_10Y\n",
        encoding="utf-8",
    )

    with _run_server(
        db_path=db_path,
        policy_path=policy_path,
        creds_path=creds_path,
        extra_env={"FICANT_SPI_NLP_CSV": str(nlp_csv)},
    ) as (_proc, endpoint, _port):
        with DeskBlotterClient(endpoint, bearer_token=OPERATOR_TOK, insecure=True) as op:
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-spi",
                    opening_base=_make_opening_base("acct-1"),
                    expected_account_version=0,
                )
            )
        units = _units()
        with DeskBlotterClient(endpoint, bearer_token=FEED_TOK, insecure=True) as feed:
            feed.ingest_market_quotes(
                service_pb2.IngestMarketQuotesRequest(
                    idempotency_key="q-spi",
                    account_id="acct-1",
                    source_id="SRC-A",
                    ticks=[
                        spi_pb2.MarketQuoteTick(
                            source_id="SRC-A",
                            quote_seq=1,
                            instrument_code="BOND_10Y",
                            source_type=2,
                            mid_yield_to_maturity=decimal_to_pb("0.025", units.rate),
                            observed_at=parse_market_time(_fresh_rfc3339(4)),
                            received_at=parse_market_time(_fresh_rfc3339(2)),
                        )
                    ],
                )
            )

        input_jsonl = tmp_path / "spi_input.jsonl"
        input_jsonl.write_text(
            json.dumps(
                {
                    "request_id": "spi-req-1",
                    "account_id": "acct-1",
                    "raw_source_text": VECTOR_CN_COMPACT,
                },
                ensure_ascii=False,
            )
            + "\n",
            encoding="utf-8",
        )
        cp_path = tmp_path / "spi.cp.json"
        with NlpDraftOnlyClient(
            endpoint,
            x_ficant_credential=PARSER_TOK,
            header_mode="x-ficant-credential",
            insecure=True,
        ) as nlp_client:
            report = NlpDraftGatewaySidecar(
                NlpDraftGatewayConfig(
                    sidecar_id="nlp-spi-live",
                    trading_date="2026-06-15",
                    checkpoint_path=cp_path,
                    input_path=input_jsonl,
                    adapter="spi",
                    endpoint=endpoint,
                    insecure=True,
                ),
                nlp_client,
            ).run_once()
        assert report.adapter == "spi"
        assert report.drafts_submitted == 1
        assert len(report.draft_ids) == 1


def test_live_unified_cli_all_three_subcommands_healthz_probe_and_stale_write_fence(
    tmp_path: Path,
) -> None:
    db_path = tmp_path / "cli_live.sqlite"
    creds_path = _write_credentials(
        tmp_path / "creds.json", accounts=["acct-1"], sources=["FEED-1", "SRC-A"]
    )
    policy_path = _write_policy(tmp_path / "policy.json")

    with _run_server(db_path=db_path, policy_path=policy_path, creds_path=creds_path) as (
        _proc,
        endpoint,
        _port,
    ):
        health = probe_healthz(endpoint, insecure=True)
        ledger = health.require_healthy_ledger()

        with DeskBlotterClient(endpoint, bearer_token=OPERATOR_TOK, insecure=True) as op:
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-cli-live",
                    opening_base=_make_opening_base("acct-1"),
                    expected_account_version=0,
                )
            )

        # Prepare token files & env
        feed_token_file = tmp_path / "feed.token"
        feed_token_file.write_text(FEED_TOK + "\n", encoding="utf-8")
        parser_token_file = tmp_path / "parser.token"
        parser_token_file.write_text(PARSER_TOK + "\n", encoding="utf-8")

        sub_env = {
            **os.environ,
            "FICANT_TEST_FEED_TOKEN": FEED_TOK,
            "FICANT_TEST_PARSER_TOKEN": PARSER_TOK,
        }

        # 1. quotes --once via `python -m ficant_sdk.sidecar` (--token-env, bearer, paired --expected-ledger-*)
        quotes_csv = tmp_path / "quotes.csv"
        obs_str = _fresh_rfc3339(4)
        rec_str = _fresh_rfc3339(2)
        quotes_csv.write_text(
            "account_id,source_id,instrument_code,quote_seq,source_type,mid_yield_to_maturity,observed_at,received_at\n"
            f"acct-1,SRC-A,BOND_10Y,1,ACTIVE_QUOTE,0.025,{obs_str},{rec_str}\n"
            f"acct-1,SRC-A,BOND_5Y,1,ACTIVE_QUOTE,0.022,{obs_str},{rec_str}\n",
            encoding="utf-8",
        )
        quotes_cp = tmp_path / "quotes.cp.json"

        res_quotes = subprocess.run(
            [
                sys.executable,
                "-m",
                "ficant_sdk.sidecar",
                "quotes",
                "--endpoint",
                endpoint,
                "--token-env",
                "FICANT_TEST_FEED_TOKEN",
                "--header-mode",
                "bearer",
                "--insecure",
                "--checkpoint",
                str(quotes_cp),
                "--trading-date",
                "2026-06-15",
                "--sidecar-id",
                "cli-quotes-1",
                "--input",
                str(quotes_csv),
                "--batch-size",
                "16",
                "--expected-ledger-epoch",
                str(ledger.ledger_epoch),
                "--expected-ledger-instance",
                ledger.ledger_instance_id,
                "--once",
            ],
            env=sub_env,
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )
        assert res_quotes.returncode == 0, res_quotes.stderr
        assert FEED_TOK not in res_quotes.stdout
        assert FEED_TOK not in res_quotes.stderr
        assert FEED_TOK not in quotes_cp.read_text(encoding="utf-8")
        quotes_report = json.loads(res_quotes.stdout.strip())
        assert quotes_report["applied_tick_count"] == 2
        assert quotes_report["ledger_epoch"] == ledger.ledger_epoch
        assert quotes_report["ledger_instance_id"] == ledger.ledger_instance_id

        # 2. trade-feed --once via `ficant-sidecar` console script (--token-file, x-ficant-credential)
        sidecar_bin = _sidecar_console_script()
        trades_jsonl = tmp_path / "trades.jsonl"
        trades_jsonl.write_text(
            json.dumps(
                {
                    "source_id": "FEED-1",
                    "external_trade_id": "EXT-CLI-001",
                    "source_revision": 1,
                    "feed_seq": 1,
                    "action": "NEW_FILL",
                    "account_id": "acct-1",
                    "instrument_code": "BOND_5Y",
                    "side": "BUY",
                    "settlement_date": "2026-06-15",
                    "notional": "10000000",
                    "clean_price_per_100": "100.10",
                    "settlement_amount": "10010000",
                    "executed_at": "2026-06-15T10:05:00+08:00",
                    "batch_first_feed_seq": 1,
                    "batch_last_feed_seq": 1,
                }
            )
            + "\n",
            encoding="utf-8",
        )
        trades_cp = tmp_path / "trades.cp.json"

        res_trades = subprocess.run(
            [
                str(sidecar_bin),
                "trade-feed",
                "--endpoint",
                endpoint,
                "--token-file",
                str(feed_token_file),
                "--header-mode",
                "x-ficant-credential",
                "--insecure",
                "--checkpoint",
                str(trades_cp),
                "--trading-date",
                "2026-06-15",
                "--sidecar-id",
                "cli-trades-1",
                "--source-id",
                "FEED-1",
                "--heartbeat-interval-ms",
                "15000",
                "--input",
                str(trades_jsonl),
                "--batch-size",
                "32",
                "--expected-ledger-epoch",
                str(ledger.ledger_epoch),
                "--expected-ledger-instance",
                ledger.ledger_instance_id,
                "--once",
            ],
            env=sub_env,
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )
        assert res_trades.returncode == 0, res_trades.stderr
        assert FEED_TOK not in res_trades.stdout
        assert FEED_TOK not in res_trades.stderr
        assert FEED_TOK not in trades_cp.read_text(encoding="utf-8")
        trades_report = json.loads(res_trades.stdout.strip())
        assert trades_report["fills_sent"] == 1
        assert trades_report["contiguous_feed_seq"] == 1

        # 3a. nlp-draft --once via `python -m ficant_sdk.sidecar` (token adapter, Vector 2)
        nlp_jsonl_1 = tmp_path / "nlp_token.jsonl"
        nlp_jsonl_1.write_text(
            json.dumps(
                {
                    "request_id": "cli-nlp-1",
                    "account_id": "acct-1",
                    "raw_source_text": VECTOR_HYBRID_KV,
                },
                ensure_ascii=False,
            )
            + "\n",
            encoding="utf-8",
        )
        nlp_cp_1 = tmp_path / "nlp_token.cp.json"

        res_nlp_mod = subprocess.run(
            [
                sys.executable,
                "-m",
                "ficant_sdk.sidecar",
                "nlp-draft",
                "--endpoint",
                endpoint,
                "--token-env",
                "FICANT_TEST_PARSER_TOKEN",
                "--header-mode",
                "bearer",
                "--insecure",
                "--checkpoint",
                str(nlp_cp_1),
                "--trading-date",
                "2026-06-15",
                "--sidecar-id",
                "cli-nlp-mod",
                "--input",
                str(nlp_jsonl_1),
                "--adapter",
                "token",
                "--parser-source",
                "cli-token-parser",
                "--model-version",
                "1.0.0",
                "--expected-ledger-epoch",
                str(ledger.ledger_epoch),
                "--expected-ledger-instance",
                ledger.ledger_instance_id,
                "--once",
            ],
            env=sub_env,
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )
        assert res_nlp_mod.returncode == 0, res_nlp_mod.stderr
        assert PARSER_TOK not in res_nlp_mod.stdout
        assert PARSER_TOK not in nlp_cp_1.read_text(encoding="utf-8")
        nlp_rep_1 = json.loads(res_nlp_mod.stdout.strip())
        assert nlp_rep_1["drafts_submitted"] == 1
        assert len(nlp_rep_1["draft_ids"]) == 1

        # 3b. nlp-draft --once via `ficant-sidecar` console script (token adapter, Vector 1)
        nlp_jsonl_2 = tmp_path / "nlp_cn.jsonl"
        nlp_jsonl_2.write_text(
            json.dumps(
                {
                    "request_id": "cli-nlp-2",
                    "account_id": "acct-1",
                    "raw_source_text": VECTOR_CN_COMPACT,
                },
                ensure_ascii=False,
            )
            + "\n",
            encoding="utf-8",
        )
        nlp_cp_2 = tmp_path / "nlp_cn.cp.json"

        res_nlp_bin = subprocess.run(
            [
                str(sidecar_bin),
                "nlp-draft",
                "--endpoint",
                endpoint,
                "--token-file",
                str(parser_token_file),
                "--header-mode",
                "x-ficant-credential",
                "--insecure",
                "--checkpoint",
                str(nlp_cp_2),
                "--trading-date",
                "2026-06-15",
                "--sidecar-id",
                "cli-nlp-bin",
                "--input",
                str(nlp_jsonl_2),
                "--adapter",
                "token",
                "--expected-ledger-epoch",
                str(ledger.ledger_epoch),
                "--expected-ledger-instance",
                ledger.ledger_instance_id,
                "--once",
            ],
            env=sub_env,
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )
        assert res_nlp_bin.returncode == 0, res_nlp_bin.stderr
        nlp_rep_2 = json.loads(res_nlp_bin.stdout.strip())
        assert nlp_rep_2["drafts_submitted"] == 1
        assert len(nlp_rep_2["draft_ids"]) == 1

        # Capture exact server snapshot_seq before stale write-fence tests
        with DeskBlotterClient(endpoint, bearer_token=TRADER_TOK, insecure=True) as tr:
            snap_baseline = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-1")
            ).snapshot
            baseline_seq = snap_baseline.snapshot_seq
            baseline_drafts = len(snap_baseline.pending_drafts)
            baseline_pending = len(snap_baseline.open_pending_matches)
            assert baseline_drafts == 2
            assert baseline_pending == 1

        # 4. Prove paired --expected-ledger-* STILL triggers /healthz probe:
        #    a) CLI against closed port with paired --expected-ledger-* fails on /healthz probe
        dead_port = _free_port()
        res_dead = subprocess.run(
            [
                str(sidecar_bin),
                "nlp-draft",
                "--endpoint",
                f"127.0.0.1:{dead_port}",
                "--token-file",
                str(parser_token_file),
                "--insecure",
                "--checkpoint",
                str(tmp_path / "dead.cp.json"),
                "--trading-date",
                "2026-06-15",
                "--sidecar-id",
                "cli-dead",
                "--input",
                str(nlp_jsonl_2),
                "--expected-ledger-epoch",
                str(ledger.ledger_epoch),
                "--expected-ledger-instance",
                ledger.ledger_instance_id,
                "--health-timeout-seconds",
                "0.5",
                "--once",
            ],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        assert res_dead.returncode != 0
        assert "/healthz" in res_dead.stderr

        #    b) Stale --expected-ledger-instance on CLI fails closed across all 3 subcommands with ZERO writes
        stale_instance = ULID_2 if ledger.ledger_instance_id != ULID_2 else ULID_1
        for sub_args in (
            [
                "quotes",
                "--input",
                str(quotes_csv),
                "--checkpoint",
                str(tmp_path / "stale_q.cp.json"),
                "--sidecar-id",
                "stale-q",
                "--token-file",
                str(feed_token_file),
            ],
            [
                "trade-feed",
                "--source-id",
                "FEED-1",
                "--input",
                str(trades_jsonl),
                "--checkpoint",
                str(tmp_path / "stale_t.cp.json"),
                "--sidecar-id",
                "stale-t",
                "--token-file",
                str(feed_token_file),
            ],
            [
                "nlp-draft",
                "--input",
                str(nlp_jsonl_1),
                "--checkpoint",
                str(tmp_path / "stale_n.cp.json"),
                "--sidecar-id",
                "stale-n",
                "--token-file",
                str(parser_token_file),
            ],
        ):
            r_stale = subprocess.run(
                [
                    str(sidecar_bin),
                    *sub_args,
                    "--endpoint",
                    endpoint,
                    "--insecure",
                    "--trading-date",
                    "2026-06-15",
                    "--expected-ledger-epoch",
                    str(ledger.ledger_epoch),
                    "--expected-ledger-instance",
                    stale_instance,
                    "--once",
                ],
                capture_output=True,
                text=True,
                timeout=15,
                check=False,
            )
            assert r_stale.returncode != 0

        # 5. Direct server-side write fence across ALL 3 write RPCs with stale epoch & stale instance ->
        #    CONCURRENCY_CONFLICT (retryable=False) and ZERO server writes!
        units = _units()
        stale_pairs = [
            (ledger.ledger_epoch + 1, ledger.ledger_instance_id),
            (ledger.ledger_epoch, stale_instance),
        ]
        with DeskBlotterClient(endpoint, bearer_token=FEED_TOK, insecure=True) as feed_c:
            for idx, stale_pair in enumerate(stale_pairs, start=10):
                with pytest.raises(BlotterError) as q_err:
                    feed_c.ingest_market_quotes(
                        service_pb2.IngestMarketQuotesRequest(
                            idempotency_key=f"stale-q-{idx}",
                            account_id="acct-1",
                            source_id="SRC-A",
                            ticks=[
                                spi_pb2.MarketQuoteTick(
                                    source_id="SRC-A",
                                    quote_seq=idx,
                                    instrument_code="BOND_10Y",
                                    source_type=2,
                                    mid_yield_to_maturity=decimal_to_pb(
                                        "0.026", units.rate
                                    ),
                                    observed_at=parse_market_time(_fresh_rfc3339(3)),
                                    received_at=parse_market_time(_fresh_rfc3339(1)),
                                )
                            ],
                        ),
                        expected_ledger=stale_pair,
                    )
                assert q_err.value.code == error_pb2.ERROR_CODE_CONCURRENCY_CONFLICT
                assert q_err.value.retryable is False

                with pytest.raises(BlotterError) as tf_err:
                    feed_c.ingest_trade_feed(
                        service_pb2.IngestTradeFeedRequest(
                            idempotency_key=f"stale-tf-{idx}",
                            source_id="FEED-1",
                            fills=[
                                spi_pb2.TradeFillRecord(
                                    source_id="FEED-1",
                                    external_trade_id=f"EXT-STALE-{idx}",
                                    source_revision=1,
                                    feed_seq=idx,
                                    action=model_pb2.TRADE_FEED_RECORD_ACTION_NEW_FILL,
                                    account_id="acct-1",
                                    instrument_code="BOND_5Y",
                                    side=model_pb2.TRADE_SIDE_BUY,
                                    settlement_date="2026-06-15",
                                    notional=decimal_to_pb(
                                        "5000000", units.currency_amount
                                    ),
                                    quote_terms=model_pb2.QuoteTerms(
                                        clean_price_per_100=decimal_to_pb(
                                            "100.00", units.price_per_100
                                        ),
                                        settlement_amount=decimal_to_pb(
                                            "5000000", units.currency_amount
                                        ),
                                    ),
                                    executed_at=_shanghai_time(2026, 6, 15, 10, 10, 0),
                                    batch_first_feed_seq=idx,
                                    batch_last_feed_seq=idx,
                                )
                            ],
                        ),
                        expected_ledger=stale_pair,
                    )
                assert tf_err.value.code == error_pb2.ERROR_CODE_CONCURRENCY_CONFLICT
                assert tf_err.value.retryable is False

        with NlpDraftOnlyClient(endpoint, bearer_token=PARSER_TOK, insecure=True) as nlp_c:
            proposal = parse_token_nlp_intent(
                request_id="stale-nlp",
                account_id="acct-1",
                raw_source_text=VECTOR_HYBRID_KV,
                units=units,
            )
            for idx, stale_pair in enumerate(stale_pairs, start=10):
                with pytest.raises(BlotterError) as nlp_err:
                    nlp_c.submit_draft_earmark(
                        service_pb2.SubmitDraftEarmarkRequest(
                            idempotency_key=f"stale-nlp-{idx}",
                            account_id="acct-1",
                            proposal=proposal,
                        ),
                        expected_ledger=stale_pair,
                    )
                assert nlp_err.value.code == error_pb2.ERROR_CODE_CONCURRENCY_CONFLICT
                assert nlp_err.value.retryable is False

        # Verify ZERO server writes occurred from any stale epoch/instance call!
        with DeskBlotterClient(endpoint, bearer_token=TRADER_TOK, insecure=True) as tr:
            snap_final = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-1")
            ).snapshot
            assert snap_final.snapshot_seq == baseline_seq
            assert len(snap_final.pending_drafts) == baseline_drafts
            assert len(snap_final.open_pending_matches) == baseline_pending


def test_cli_argument_validation_and_healthz_probe_invocation_count(
    tmp_path: Path,
) -> None:
    tok_file = tmp_path / "tok.txt"
    tok_file.write_text(PARSER_TOK + "\n", encoding="utf-8")
    inp = tmp_path / "in.jsonl"
    inp.write_text(
        json.dumps(
            {
                "request_id": "hp-1",
                "account_id": "acct-1",
                "raw_source_text": VECTOR_HYBRID_KV,
            },
            ensure_ascii=False,
        )
        + "\n",
        encoding="utf-8",
    )

    # Unpaired --expected-ledger-epoch without --expected-ledger-instance fails
    rc_unpaired = cli_main(
        [
            "nlp-draft",
            "--endpoint",
            "127.0.0.1:50051",
            "--token-file",
            str(tok_file),
            "--insecure",
            "--checkpoint",
            str(tmp_path / "cp.json"),
            "--trading-date",
            "2026-06-15",
            "--sidecar-id",
            "s1",
            "--input",
            str(inp),
            "--expected-ledger-epoch",
            "1",
            "--once",
        ]
    )
    assert rc_unpaired == 1

    # Both --token-env and --token-file rejected by argparse (exit 2)
    rc_dual_tok = cli_main(
        [
            "nlp-draft",
            "--endpoint",
            "127.0.0.1:50051",
            "--token-env",
            "SOME_ENV",
            "--token-file",
            str(tok_file),
            "--insecure",
            "--checkpoint",
            str(tmp_path / "cp.json"),
            "--trading-date",
            "2026-06-15",
            "--sidecar-id",
            "s1",
            "--input",
            str(inp),
            "--once",
        ]
    )
    assert rc_dual_tok == 2

    # Prove NlpDraftGatewaySidecar ALWAYS calls health_probe even when expected_ledger is set
    probe_calls = 0

    def _counting_probe() -> HealthzStatus:
        nonlocal probe_calls
        probe_calls += 1
        return _health(epoch=2, instance_id=ULID_2)

    rec = _RecordingChannel()
    client = NlpDraftOnlyClient(channel=rec, bearer_token=PARSER_TOK)
    sidecar = NlpDraftGatewaySidecar(
        NlpDraftGatewayConfig(
            sidecar_id="nlp-probe-check",
            trading_date="2026-06-15",
            checkpoint_path=tmp_path / "probe.cp.json",
            input_path=inp,
            adapter="token",
            expected_ledger=(2, ULID_2),
        ),
        client,
        health_probe=_counting_probe,
    )
    report = sidecar.run_once()
    assert probe_calls == 1
    assert report.drafts_submitted == 1

    # Server epoch regression below expected_ledger raises LedgerEpochRegressionError
    sidecar_reg = NlpDraftGatewaySidecar(
        NlpDraftGatewayConfig(
            sidecar_id="nlp-probe-reg",
            trading_date="2026-06-15",
            checkpoint_path=tmp_path / "probe_reg.cp.json",
            input_path=inp,
            adapter="token",
            expected_ledger=(3, ULID_2),
        ),
        client,
        health_probe=lambda: _health(epoch=2, instance_id=ULID_2),
    )
    with pytest.raises(LedgerEpochRegressionError):
        sidecar_reg.run_once()

    # Same epoch different instance raises LedgerInstanceMismatchError
    sidecar_mis = NlpDraftGatewaySidecar(
        NlpDraftGatewayConfig(
            sidecar_id="nlp-probe-mis",
            trading_date="2026-06-15",
            checkpoint_path=tmp_path / "probe_mis.cp.json",
            input_path=inp,
            adapter="token",
            expected_ledger=(2, ULID_1),
        ),
        client,
        health_probe=lambda: _health(epoch=2, instance_id=ULID_2),
    )
    with pytest.raises(LedgerInstanceMismatchError):
        sidecar_mis.run_once()

    # Disabled storage raises HealthCheckError
    sidecar_dis = NlpDraftGatewaySidecar(
        NlpDraftGatewayConfig(
            sidecar_id="nlp-probe-dis",
            trading_date="2026-06-15",
            checkpoint_path=tmp_path / "probe_dis.cp.json",
            input_path=inp,
            adapter="token",
        ),
        client,
        health_probe=lambda: HealthzStatus(
            status="ok",
            service="ficant-server",
            storage_mode="disabled",
            blotter_healthy=True,
            ledger_epoch=None,
            ledger_instance_id=None,
        ),
    )
    with pytest.raises(HealthCheckError):
        sidecar_dis.run_once()
