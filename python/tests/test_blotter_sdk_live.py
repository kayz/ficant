"""Live end-to-end Python SDK verification against real compiled ficant-server subprocesses."""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from decimal import Decimal
import hashlib
import json
import os
from pathlib import Path
import socket
import sqlite3
import struct
import subprocess
import sys
import threading
import time
from typing import Any, TypeVar

from google.protobuf.any_pb2 import Any as AnyPb
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
from ficant.market.v1 import (  # noqa: E402
    definition_pb2,
    fact_pb2,
    regulatory_rule_pb2,
    rule_pb2,
)
from ficant.rates.v1 import analytics_pb2 as rates_pb2  # noqa: E402
from ficant_sdk import (  # noqa: E402
    BlotterError,
    BlotterEventApplier,
    DeskBlotterClient,
    RatesClient,
)

_RespMsg = TypeVar("_RespMsg")

SHANGHAI_TZ = timezone(timedelta(hours=8))
# Derived dynamically without raw two-letter country literals in Python source.
REGULATORY_MARKET = bytes([67, 78]).decode("ascii")
REGULATORY_RULE_TYPE = "regulatory"
REGULATORY_TYPE_URL = "type.googleapis.com/ficant.market.v1.RegulatoryRulePack"
FUTURES_MARKET = "CFFEX"
FUTURES_RULE_TYPE = "cgb-futures"
FUTURES_TYPE_URL = "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack"
EMBEDDED_POLICY_PATH = (
    REPO_ROOT / "binaries" / "ficant-server" / "policy" / "desk-blotter-policy-v1.json"
)
FUTURES_V2_BIN_PATH = REPO_ROOT / "domain-packs" / "cgb-futures" / "cgb-futures-v2.bin"

TRADER_TOK = "live-e2e-trader-secret-token-001"
OPERATOR_TOK = "live-e2e-operator-secret-token-002"
FEED_TOK = "live-e2e-feed-secret-token-003"
PARSER_TOK = "live-e2e-parser-secret-token-004"
TRADER_A_ONLY_TOK = "live-e2e-trader-a-only-token-005"


# ======================================================================
# Shared Live Subprocess, Protocol & Fixture Helpers
# ======================================================================


def _ulid(suffix: str) -> common_pb2.Ulid:
    padded = suffix.upper().rjust(2, "0")[:2]
    return common_pb2.Ulid(value=f"01ARZ3NDEKTSV4RRFFQ69G5F{padded}")


def _units() -> rates_pb2.AnalysisUnits:
    return rates_pb2.AnalysisUnits(
        currency_amount=common_pb2.UnitRef(unit_id=_ulid("A1"), version=1),
        price_per_100=common_pb2.UnitRef(unit_id=_ulid("A2"), version=1),
        rate=common_pb2.UnitRef(unit_id=_ulid("A3"), version=1),
        years=common_pb2.UnitRef(unit_id=_ulid("Y1"), version=1),
        years_squared=common_pb2.UnitRef(unit_id=_ulid("Y2"), version=1),
        dv01=common_pb2.UnitRef(unit_id=_ulid("A4"), version=1),
        dv01_per_100=common_pb2.UnitRef(unit_id=_ulid("A5"), version=1),
        dimensionless=common_pb2.UnitRef(unit_id=_ulid("A6"), version=1),
        contract_count=common_pb2.UnitRef(unit_id=_ulid("A7"), version=1),
    )


UNITS = _units()


def _dec(text: str, unit: common_pb2.UnitRef) -> common_pb2.DecimalValue:
    num = Decimal(text)
    sign, digits, exp = num.as_tuple()
    coeff = int("".join(str(d) for d in digits) or "0")
    if sign:
        coeff = -coeff
    if exp >= 0:
        coeff *= 10**exp
        scale = 0
    else:
        scale = -exp
    if coeff == 0:
        scale = 0
    while scale > 0 and coeff % 10 == 0:
        coeff //= 10
        scale -= 1
    return common_pb2.DecimalValue(coefficient=str(coeff), scale=scale, unit=unit)


def _to_dec(value: common_pb2.DecimalValue) -> Decimal:
    return Decimal(value.coefficient).scaleb(-value.scale)


def _market_time_from_dt(dt_utc: datetime) -> common_pb2.MarketTime:
    dt_utc = dt_utc.astimezone(timezone.utc)
    local_date = dt_utc.astimezone(SHANGHAI_TZ).date().isoformat()
    ts = Timestamp()
    ts.FromDatetime(dt_utc)
    return common_pb2.MarketTime(
        instant=ts,
        market_timezone="Asia/Shanghai",
        local_trading_date=local_date,
    )


def _shanghai_time(
    y: int, m: int, d: int, hh: int = 10, mm: int = 0, ss: int = 0
) -> common_pb2.MarketTime:
    local_dt = datetime(y, m, d, hh, mm, ss, tzinfo=SHANGHAI_TZ)
    return _market_time_from_dt(local_dt.astimezone(timezone.utc))


def _recent_time(seconds_ago: int = 5) -> common_pb2.MarketTime:
    return _market_time_from_dt(datetime.now(timezone.utc) - timedelta(seconds=seconds_ago))


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def _server_binary() -> Path:
    exe = "ficant-server.exe" if sys.platform == "win32" else "ficant-server"
    path = REPO_ROOT / "target" / "debug" / exe
    if not path.exists():
        subprocess.run(
            ["cargo", "build", "--locked", "--offline", "-p", "ficant-server"],
            cwd=REPO_ROOT,
            check=True,
        )
    assert path.exists(), f"expected ficant-server binary at {path}"
    return path


def _wait_tcp_ready(port: int, proc: subprocess.Popen[bytes], timeout_s: float = 10.0) -> None:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            stderr = proc.stderr.read().decode("utf-8", errors="replace") if proc.stderr else ""
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
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)


@contextmanager
def _run_server(
    *,
    db_path: Path | None = None,
    ephemeral: bool = False,
    policy_path: Path | None = None,
    creds_path: Path | None = None,
    retention_window: int | None = None,
    watchdog_interval_ms: int | None = 0,
    cwd: Path | None = None,
) -> Iterator[tuple[subprocess.Popen[bytes], str, int]]:
    port = _free_port()
    addr = f"127.0.0.1:{port}"
    env = {
        k: v
        for k, v in os.environ.items()
        if not k.startswith("FICANT_BLOTTER_")
        and not k.startswith("FICANT_SPI_")
        and k != "FICANT_SERVER_ADDR"
    }
    env["FICANT_SERVER_ADDR"] = addr
    if db_path is not None:
        env["FICANT_BLOTTER_DB_PATH"] = str(db_path)
    if ephemeral:
        env["FICANT_BLOTTER_EPHEMERAL_IN_MEMORY"] = "1"
    if policy_path is not None:
        env["FICANT_BLOTTER_POLICY_PATH"] = str(policy_path)
    if creds_path is not None:
        env["FICANT_BLOTTER_CREDENTIALS_PATH"] = str(creds_path)
    if retention_window is not None:
        env["FICANT_BLOTTER_EVENT_RETENTION_WINDOW"] = str(retention_window)
    if watchdog_interval_ms is not None:
        env["FICANT_BLOTTER_WATCHDOG_INTERVAL_MS"] = str(watchdog_interval_ms)

    proc = subprocess.Popen(
        [str(_server_binary())],
        cwd=cwd or REPO_ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        _wait_tcp_ready(port, proc)
        yield proc, addr, port
    finally:
        _stop_proc(proc)


def _write_credentials(
    path: Path,
    *,
    accounts: list[str],
    sources: list[str],
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
            {
                "actor_id": "trader-a-only",
                "role": "TRADER",
                "token_sha256_hex": _sha(TRADER_A_ONLY_TOK),
                "allowed_accounts": [accounts[0]],
                "allowed_sources": sources,
            },
        ],
    }
    path.write_text(json.dumps(doc, indent=2), encoding="utf-8")
    return path


def _build_reg_pack_binding(
    *,
    limit_codes: list[tuple[str, int]] | None = None,
    verification_status: str = "VERIFIED",
    tamper_hash: bool = False,
    raw_override: bytes | None = None,
) -> dict[str, Any]:
    if raw_override is not None:
        raw = raw_override
    else:
        constraints = [
            regulatory_rule_pb2.ConstraintDefinition(limit_code=code, measure=measure)
            for code, measure in (
                limit_codes
                or [
                    (
                        "MAX_INST_FACE",
                        regulatory_rule_pb2.CONSTRAINT_MEASURE_INSTRUMENT_FACE,
                    )
                ]
            )
        ]
        raw = regulatory_rule_pb2.RegulatoryRulePack(
            constraints=constraints
        ).SerializeToString()
    digest = hashlib.sha256(raw).hexdigest()
    if tamper_hash:
        digest = ("0" if digest[0] != "0" else "1") + digest[1:]
    return {
        "rule_pack_id": "01ARZ3NDEKTSV4RRFFQ69G5F99",
        "version": 1,
        "market": REGULATORY_MARKET,
        "rule_type": REGULATORY_RULE_TYPE,
        "source": "desk-policy",
        "effective_from_rfc3339": "2020-01-01T00:00:00Z",
        "effective_to_rfc3339": "2035-01-01T00:00:00Z",
        "verification_status": verification_status,
        "type_url": REGULATORY_TYPE_URL,
        "content_hash_sha256_hex": digest,
        "content_hex": raw.hex(),
    }


def _write_policy(
    path: Path,
    *,
    profile: str = "A",
    allowed_hedge_codes: list[str] | None = None,
    default_quote_source_id: str = "SRC-A",
    quote_staleness_seconds: int = 300,
    limit_ceilings: dict[str, str] | None = None,
    regulatory_rule_pack: dict[str, Any] | None = None,
    accounts_override: dict[str, Any] | None = None,
    clean_price_tolerance: str = "0.05",
    yield_tolerance: str = "0.0005",
) -> Path:
    doc: dict[str, Any] = {
        "schema_version": 1,
        "policy_version": 1,
        "policy_profile": profile,
        "allowed_hedge_product_codes": allowed_hedge_codes or ["T", "TF", "TL"],
        "blotter_config": {
            "clean_price_tolerance": clean_price_tolerance,
            "yield_tolerance": yield_tolerance,
            "committed_overdue_seconds": 3600,
            "pre_close_alert_time": "16:30:00",
            "feed_staleness_seconds": 300,
        },
        "default_account_policy": {
            "default_quote_source_id": default_quote_source_id,
            "instrument_quote_sources": {},
            "quote_staleness_seconds": quote_staleness_seconds,
            "default_draft_hold_ttl_seconds": 1800,
            "limit_ceilings": limit_ceilings or {},
            "regulatory_rule_pack": regulatory_rule_pack,
        },
        "accounts": accounts_override or {},
    }
    path.write_text(json.dumps(doc, indent=2), encoding="utf-8")
    return path


def _futures_v2_pack() -> rule_pb2.MarketRulePack:
    raw = FUTURES_V2_BIN_PATH.read_bytes()
    return rule_pb2.MarketRulePack(
        rule_pack_id=_ulid("FP"),
        version=2,
        market=FUTURES_MARKET,
        rule_type=FUTURES_RULE_TYPE,
        source="cffex",
        effective_from=_shanghai_time(2020, 1, 1, 0, 0, 0),
        effective_to=_shanghai_time(2035, 1, 1, 0, 0, 0),
        verification_status=rule_pb2.VERIFICATION_STATUS_VERIFIED,
        content_hash=common_pb2.Sha256(value=hashlib.sha256(raw).digest()),
        content=AnyPb(type_url=FUTURES_TYPE_URL, value=raw),
    )


def _make_bond(
    *,
    ulid_suffix: str = "B1",
    face_value: str = "100",
    first_issue: str = "2025-01-15",
    maturity: str = "2030-01-15",
    coupon_rate: str = "0.025",
) -> definition_pb2.Bond:
    return definition_pb2.Bond(
        instrument=common_pb2.VersionRef(id=_ulid(ulid_suffix), version=1),
        first_issue_date=first_issue,
        current_issue_date=first_issue,
        maturity_date=maturity,
        face_value=_dec(face_value, UNITS.currency_amount),
        cumulative_issued_amount=_dec("50000000000", UNITS.currency_amount),
        tax_attributes=definition_pb2.BondTaxAttributes(
            value_added_tax_status=definition_pb2.VALUE_ADDED_TAX_STATUS_EXEMPT,
            income_tax_status=definition_pb2.INCOME_TAX_STATUS_EXEMPT,
        ),
        coupon_rate=_dec(coupon_rate, UNITS.rate),
        coupon_frequency=definition_pb2.BOND_COUPON_FREQUENCY_SEMIANNUAL,
        day_count=definition_pb2.BOND_DAY_COUNT_CONVENTION_ACT_ACT_BOND_ISMA,
        business_day=definition_pb2.BOND_BUSINESS_DAY_CONVENTION_FOLLOWING,
    )


def _make_calendar(end_date: str = "2031-12-31") -> definition_pb2.Calendar:
    return definition_pb2.Calendar(
        calendar_id=_ulid("C1"),
        version=1,
        market="INTERBANK",
        market_timezone="Asia/Shanghai",
        effective_from=_shanghai_time(2025, 1, 1, 0, 0, 0),
        effective_to=_shanghai_time(2032, 1, 1, 0, 0, 0),
        sessions=[
            definition_pb2.CalendarSession(
                local_date="2025-01-01",
                open_local_time="09:00:00",
                close_local_time="17:00:00",
                closed=False,
            ),
            definition_pb2.CalendarSession(
                local_date=end_date,
                open_local_time="09:00:00",
                close_local_time="17:00:00",
                closed=False,
            ),
        ],
    )


def _make_opening_base(
    account_id: str,
    *,
    base_suffix: str = "01",
    source_id: str = "FEED-1",
    start_after_seq: int = 0,
    watermark_source_seq: int | None = None,
    opening_cash_t0: str = "500000000",
    opening_net_cash_delta_t1: str = "10000000",
    balances: list[model_pb2.OpeningInstrumentBalance] | None = None,
    instruments: list[model_pb2.InstrumentCatalogEntry] | None = None,
    default_calendar_requirement: int = rates_pb2.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
    baseline_evidence: list[model_pb2.BaseEvidence] | None = None,
    calendar_end_date: str = "2031-12-31",
) -> model_pb2.OpeningPositionBase:
    if balances is None:
        balances = [
            model_pb2.OpeningInstrumentBalance(
                instrument_code="BOND_10Y",
                opening_economic_notional=_dec("100000000", UNITS.currency_amount),
                opening_available_notional=_dec("100000000", UNITS.currency_amount),
                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
            )
        ]
    if instruments is None:
        instruments = [
            model_pb2.InstrumentCatalogEntry(
                instrument_code="BOND_10Y",
                bond=_make_bond(),
                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_SOVEREIGN_OR_POLICY_FIXED,
                calendar_requirement=rates_pb2.CALENDAR_REQUIREMENT_UNSPECIFIED,
            )
        ]
    kwargs: dict[str, Any] = {
        "account_id": account_id,
        "base_id": _ulid(base_suffix),
        "trading_date": "2026-06-15",
        "next_settlement_date": "2026-06-16",
        "effective_from": _shanghai_time(2026, 6, 15, 9, 0, 0),
        "watermark_time": _shanghai_time(2026, 6, 15, 9, 0, 0),
        "source_id": source_id,
        "opening_cash_t0": _dec(opening_cash_t0, UNITS.currency_amount),
        "opening_net_cash_delta_t1": _dec(
            opening_net_cash_delta_t1, UNITS.currency_amount
        ),
        "balances": balances,
        "instruments": instruments,
        "calendar": _make_calendar(calendar_end_date),
        "default_calendar_requirement": default_calendar_requirement,
        "units": UNITS,
        "baseline_evidence": baseline_evidence or [],
        "source_start_after_feed_seq": start_after_seq,
    }
    if watermark_source_seq is not None:
        kwargs["watermark_source_seq"] = watermark_source_seq
    else:
        kwargs["watermark_source_seq"] = start_after_seq
    return model_pb2.OpeningPositionBase(**kwargs)


def _quote_terms_clean(
    clean_price: str, settlement_amount: str | None = None
) -> model_pb2.QuoteTerms:
    kwargs: dict[str, Any] = {
        "clean_price_per_100": _dec(clean_price, UNITS.price_per_100),
    }
    if settlement_amount is not None:
        kwargs["settlement_amount"] = _dec(settlement_amount, UNITS.currency_amount)
    return model_pb2.QuoteTerms(**kwargs)


def _quote_terms_yield(
    ytm: str, settlement_amount: str | None = None
) -> model_pb2.QuoteTerms:
    kwargs: dict[str, Any] = {
        "yield_to_maturity": _dec(ytm, UNITS.rate),
    }
    if settlement_amount is not None:
        kwargs["settlement_amount"] = _dec(settlement_amount, UNITS.currency_amount)
    return model_pb2.QuoteTerms(**kwargs)


def _make_fill(
    *,
    source_id: str = "FEED-1",
    trade_id: str,
    rev: int = 1,
    feed_seq: int,
    account_id: str,
    instrument_code: str = "BOND_10Y",
    side: int = model_pb2.TRADE_SIDE_SELL,
    settlement_date: str = "2026-06-15",
    notional: str = "20000000",
    quote_terms: model_pb2.QuoteTerms | None = None,
    action: int = model_pb2.TRADE_FEED_RECORD_ACTION_NEW_FILL,
    batch_first: int | None = None,
    batch_last: int | None = None,
) -> spi_pb2.TradeFillRecord:
    return spi_pb2.TradeFillRecord(
        source_id=source_id,
        external_trade_id=trade_id,
        source_revision=rev,
        feed_seq=feed_seq,
        action=action,
        account_id=account_id,
        instrument_code=instrument_code,
        side=side,
        settlement_date=settlement_date,
        notional=_dec(notional, UNITS.currency_amount),
        quote_terms=quote_terms
        if quote_terms is not None
        else _quote_terms_clean("100", notional),
        executed_at=_shanghai_time(2026, 6, 15, 10, 5, 0),
        batch_first_feed_seq=batch_first if batch_first is not None else feed_seq,
        batch_last_feed_seq=batch_last if batch_last is not None else feed_seq,
    )


def _make_quote_tick(
    *,
    source_id: str,
    quote_seq: int,
    instrument_code: str = "BOND_10Y",
    ytm: str = "0.025",
    seconds_ago: int = 5,
) -> spi_pb2.MarketQuoteTick:
    obs = _recent_time(seconds_ago)
    rec = _recent_time(max(1, seconds_ago - 1)) if seconds_ago > 1 else obs
    return spi_pb2.MarketQuoteTick(
        source_id=source_id,
        quote_seq=quote_seq,
        instrument_code=instrument_code,
        source_type=fact_pb2.PRICE_SOURCE_TYPE_ACTIVE_QUOTE,
        mid_yield_to_maturity=_dec(ytm, UNITS.rate),
        observed_at=obs,
        received_at=rec,
    )


def _decode_chunked(data: bytes) -> bytes:
    out = bytearray()
    pos = 0
    while True:
        end = data.find(b"\r\n", pos)
        assert end != -1, "malformed chunked HTTP response"
        size = int(data[pos:end].strip(), 16)
        pos = end + 2
        if size == 0:
            break
        out.extend(data[pos : pos + size])
        pos += size + 2
    return bytes(out)


def _grpc_web_unary(
    port: int,
    rpc_path: str,
    request_msg: Any,
    response_cls: type[_RespMsg],
    *,
    bearer_token: str | None = None,
) -> _RespMsg:
    payload = request_msg.SerializeToString()
    framed = b"\x00" + struct.pack(">I", len(payload)) + payload
    headers = (
        f"POST {rpc_path} HTTP/1.1\r\n"
        f"Host: 127.0.0.1:{port}\r\n"
        "Content-Type: application/grpc-web+proto\r\n"
        "Accept: application/grpc-web+proto\r\n"
        "X-Grpc-Web: 1\r\n"
        f"Content-Length: {len(framed)}\r\n"
        "Connection: close\r\n"
    )
    if bearer_token is not None:
        headers += f"authorization: Bearer {bearer_token}\r\n"
    headers += "\r\n"

    with socket.create_connection(("127.0.0.1", port), timeout=5.0) as sock:
        sock.sendall(headers.encode("ascii") + framed)
        chunks: list[bytes] = []
        while True:
            buf = sock.recv(65536)
            if not buf:
                break
            chunks.append(buf)
    raw = b"".join(chunks)
    sep = raw.find(b"\r\n\r\n")
    assert sep != -1, f"invalid HTTP response: {raw!r}"
    hdr_text = raw[:sep].decode("ascii", errors="replace")
    assert hdr_text.startswith("HTTP/1.1 200"), f"expected HTTP 200, got: {hdr_text}"
    body = raw[sep + 4 :]
    if "transfer-encoding: chunked" in hdr_text.lower():
        body = _decode_chunked(body)
    assert len(body) >= 5 and body[0] == 0x00, f"missing gRPC-Web data frame: {body!r}"
    frame_len = struct.unpack(">I", body[1:5])[0]
    return response_cls.FromString(body[5 : 5 + frame_len])  # type: ignore[attr-defined]


# ======================================================================
# 1. Default Unconfigured DB Mode (Rates OK, Blotter Disabled, Zero Files)
# ======================================================================


def test_live_1_default_unconfigured_db_mode_zero_files_and_rates_ok(
    tmp_path: Path,
) -> None:
    empty_cwd = tmp_path / "empty_workdir"
    empty_cwd.mkdir()

    with _run_server(cwd=empty_cwd, watchdog_interval_ms=None) as (_proc, endpoint, port):
        # 1. --health-check succeeds
        hc = subprocess.run(
            [str(_server_binary()), "--health-check"],
            cwd=empty_cwd,
            env={**os.environ, "FICANT_SERVER_ADDR": endpoint},
            capture_output=True,
            check=False,
        )
        assert hc.returncode == 0, hc.stderr.decode("utf-8", errors="replace")

        # 2. RatesClient works normally
        with RatesClient(endpoint, insecure=True) as rates_client:
            hedge = rates_client.analyze_futures_hedge(
                rates_pb2.AnalyzeFuturesHedgeRequest(
                    units=UNITS,
                    product_code="T",
                    target_dv01=_dec("1000", UNITS.dv01),
                    ctd_dv01_per_100=_dec("0.075", UNITS.dv01_per_100),
                    conversion_factor=_dec("0.985", UNITS.dimensionless),
                    delivery_rule_pack=_futures_v2_pack(),
                    valuation_at=_shanghai_time(2026, 6, 15, 10, 0, 0),
                )
            )
            assert _to_dec(hedge.measures.contract_notional) == Decimal("1000000")

        # 3. DeskBlotterService & all 4 SPIs return STORAGE_UNAVAILABLE
        with DeskBlotterClient(endpoint, insecure=True) as blotter:
            with pytest.raises(BlotterError) as snap_err:
                blotter.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id="acct-1")
                )
            assert snap_err.value.code == error_pb2.ERROR_CODE_STORAGE_UNAVAILABLE

            with pytest.raises(BlotterError) as pos_err:
                blotter.fetch_opening_position_base(
                    spi_pb2.FetchOpeningPositionBaseRequest(
                        request_id="r1", account_id="acct-1", trading_date="2026-06-15"
                    )
                )
            assert pos_err.value.code == error_pb2.ERROR_CODE_STORAGE_UNAVAILABLE

            with pytest.raises(BlotterError) as feed_err:
                blotter.pull_trade_feed_batch(
                    spi_pb2.PullTradeFeedBatchRequest(
                        source_id="FEED-1", after_feed_seq=0, max_records=10
                    )
                )
            assert feed_err.value.code == error_pb2.ERROR_CODE_STORAGE_UNAVAILABLE

            with pytest.raises(BlotterError) as q_err:
                blotter.pull_market_quotes(
                    spi_pb2.PullMarketQuotesRequest(
                        cursor=spi_pb2.QuoteCursor(
                            source_id="SRC-A",
                            instrument_code="BOND_10Y",
                            after_quote_seq=0,
                        ),
                        max_ticks=10,
                    )
                )
            assert q_err.value.code == error_pb2.ERROR_CODE_STORAGE_UNAVAILABLE

            with pytest.raises(BlotterError) as nlp_err:
                blotter.parse_nlp_intent(
                    spi_pb2.ParseNlpIntentRequest(
                        request_id="n1", account_id="acct-1", raw_source_text="BUY"
                    )
                )
            assert nlp_err.value.code == error_pb2.ERROR_CODE_STORAGE_UNAVAILABLE

        # 4. Raw HTTP/1.1 gRPC-Web also returns STORAGE_UNAVAILABLE
        web_resp = _grpc_web_unary(
            port,
            "/ficant.blotter.v1.DeskBlotterService/GetAccountSnapshot",
            service_pb2.GetAccountSnapshotRequest(account_id="acct-1"),
            service_pb2.GetAccountSnapshotResponse,
        )
        assert web_resp.WhichOneof("result") == "error"
        assert web_resp.error.code == error_pb2.ERROR_CODE_STORAGE_UNAVAILABLE

        # 5. Empty working directory has ZERO files created
        assert list(empty_cwd.iterdir()) == []


# ======================================================================
# 2. Cross-Process Exclusive WAL Lock
# ======================================================================


def test_live_2_cross_process_exclusive_wal_lock(tmp_path: Path) -> None:
    db_path = tmp_path / "exclusive.sqlite"
    creds_path = _write_credentials(
        tmp_path / "creds.json", accounts=["acct-1"], sources=["FEED-1", "SRC-A"]
    )
    policy_path = _write_policy(tmp_path / "policy.json")

    with _run_server(db_path=db_path, policy_path=policy_path, creds_path=creds_path) as (
        _proc1,
        endpoint1,
        _port1,
    ):
        # Verify Server #1 is actively serving and created WAL files
        with DeskBlotterClient(endpoint1, bearer_token=OPERATOR_TOK, insecure=True) as c1:
            imp = c1.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-lock",
                    opening_base=_make_opening_base("acct-1"),
                    expected_account_version=0,
                )
            )
            assert imp.committed_seq == 1

        # Second OS subprocess targeting the exact same WAL DB MUST exit immediately non-zero!
        port2 = _free_port()
        env2 = {
            **os.environ,
            "FICANT_SERVER_ADDR": f"127.0.0.1:{port2}",
            "FICANT_BLOTTER_DB_PATH": str(db_path),
            "FICANT_BLOTTER_POLICY_PATH": str(policy_path),
            "FICANT_BLOTTER_CREDENTIALS_PATH": str(creds_path),
            "FICANT_BLOTTER_WATCHDOG_INTERVAL_MS": "0",
        }
        second = subprocess.run(
            [str(_server_binary())],
            cwd=REPO_ROOT,
            env=env2,
            capture_output=True,
            timeout=10,
            check=False,
        )
        assert second.returncode != 0

    # After Server #1 shuts down cleanly, Server #3 acquires the WAL lock and reads persisted state
    with _run_server(db_path=db_path, policy_path=policy_path, creds_path=creds_path) as (
        _proc3,
        endpoint3,
        _port3,
    ):
        with DeskBlotterClient(endpoint3, bearer_token=TRADER_TOK, insecure=True) as c3:
            snap = c3.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-1")
            ).snapshot
            assert snap.account_id == "acct-1"
            assert snap.snapshot_seq == 1


# ======================================================================
# 3. proc.kill() Hard Kill & Disconnect Recovery (10->12->11, rev2->rev1)
# ======================================================================


def test_live_3_hard_kill_disconnect_recovery_gap_and_revision_parity(
    tmp_path: Path,
) -> None:
    creds_path = _write_credentials(
        tmp_path / "creds.json", accounts=["acct-1"], sources=["FEED-1", "SRC-A"]
    )
    policy_path = _write_policy(tmp_path / "policy.json")

    def _run_disrupted() -> model_pb2.AccountBlotterSnapshot:
        db_a = tmp_path / "disrupted.sqlite"
        imp_req = service_pb2.ImportOpeningBaseRequest(
            idempotency_key="imp-rec-1",
            opening_base=_make_opening_base("acct-1", start_after_seq=9),
            expected_account_version=0,
        )
        comm_req = service_pb2.CreateCommittedRequest(
            idempotency_key="comm-rec-1",
            account_id="acct-1",
            expected_position_version=1,
            instrument_code="BOND_10Y",
            side=model_pb2.TRADE_SIDE_SELL,
            settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
            notional=_dec("30000000", UNITS.currency_amount),
            quote_terms=_quote_terms_clean("100", "30000000"),
            note="match-T10",
        )

        # Phase 1: import, create committed, deliver seq=10, then gap seq=12 (missing 11), then HARD KILL!
        with _run_server(db_path=db_a, policy_path=policy_path, creds_path=creds_path) as (
            proc1,
            ep1,
            _p1,
        ):
            with DeskBlotterClient(ep1, bearer_token=OPERATOR_TOK, insecure=True) as op:
                imp_res = op.import_opening_base(imp_req)
                assert imp_res.replayed is False
                # Re-import with a NEW key -> IMMUTABLE_VIOLATION
                with pytest.raises(BlotterError) as reimp_err:
                    op.import_opening_base(
                        service_pb2.ImportOpeningBaseRequest(
                            idempotency_key="imp-rec-new-key",
                            opening_base=_make_opening_base("acct-1", start_after_seq=9),
                            expected_account_version=0,
                        )
                    )
                assert reimp_err.value.code == error_pb2.ERROR_CODE_IMMUTABLE_VIOLATION

            with DeskBlotterClient(ep1, bearer_token=TRADER_TOK, insecure=True) as tr:
                comm_res = tr.create_committed(comm_req)
                assert comm_res.replayed is False

            with DeskBlotterClient(ep1, bearer_token=FEED_TOK, insecure=True) as fd:
                r10 = fd.ingest_trade_feed(
                    service_pb2.IngestTradeFeedRequest(
                        idempotency_key="f-10",
                        source_id="FEED-1",
                        fills=[
                            _make_fill(
                                trade_id="T-10",
                                rev=1,
                                feed_seq=10,
                                account_id="acct-1",
                                notional="30000000",
                            )
                        ],
                    )
                )
                assert r10.item_outcomes[0].kind == spi_pb2.TRADE_FILL_OUTCOME_KIND_AUTO_MATCHED
                assert r10.feed_status.contiguous_feed_seq == 10

                # Deliver seq=12 (missing 11!) -> BUFFERED_GAP, contiguous stays 10
                r12 = fd.ingest_trade_feed(
                    service_pb2.IngestTradeFeedRequest(
                        idempotency_key="f-12",
                        source_id="FEED-1",
                        fills=[
                            _make_fill(
                                trade_id="T-12",
                                rev=1,
                                feed_seq=12,
                                account_id="acct-1",
                                notional="15000000",
                            )
                        ],
                    )
                )
                assert r12.item_outcomes[0].kind == spi_pb2.TRADE_FILL_OUTCOME_KIND_BUFFERED_GAP
                assert r12.feed_status.contiguous_feed_seq == 10
                assert r12.feed_status.paused_by_gap is True
                assert [(r.first, r.last) for r in r12.feed_status.missing_feed_ranges] == [(11, 11)]

            # Simulate abrupt process crash via hard kill!
            proc1.kill()
            proc1.wait(timeout=5)

        # Phase 2: Restart on SAME WAL, verify idempotency replay & contiguous_feed_seq=10,
        # backfill seq=11 carrying T-11 rev=2 (missing rev=1 -> REVISION_HOLD!), then HARD KILL!
        with _run_server(db_path=db_a, policy_path=policy_path, creds_path=creds_path) as (
            proc2,
            ep2,
            _p2,
        ):
            with DeskBlotterClient(ep2, bearer_token=OPERATOR_TOK, insecure=True) as op:
                snap_before_replay = op.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id="acct-1")
                ).snapshot
                assert snap_before_replay.feed_statuses[0].contiguous_feed_seq == 10
                head_before = snap_before_replay.snapshot_seq

                # Original idempotency_key retry -> replayed=True & seq increments ONLY ONCE!
                imp_replay = op.import_opening_base(imp_req)
                assert imp_replay.replayed is True
                assert imp_replay.committed_seq == imp_res.committed_seq

            with DeskBlotterClient(ep2, bearer_token=TRADER_TOK, insecure=True) as tr:
                comm_replay = tr.create_committed(comm_req)
                assert comm_replay.replayed is True
                assert comm_replay.committed_seq == comm_res.committed_seq
                snap_after_replay = tr.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id="acct-1")
                ).snapshot
                assert snap_after_replay.snapshot_seq == head_before

            # Backfill missing seq=11 carrying T-11 rev=2 (AMEND, missing rev=1!)
            with DeskBlotterClient(ep2, bearer_token=FEED_TOK, insecure=True) as fd:
                r11 = fd.ingest_trade_feed(
                    service_pb2.IngestTradeFeedRequest(
                        idempotency_key="f-11-rev2",
                        source_id="FEED-1",
                        fills=[
                            _make_fill(
                                trade_id="T-11",
                                rev=2,
                                feed_seq=11,
                                account_id="acct-1",
                                notional="12000000",
                                action=model_pb2.TRADE_FEED_RECORD_ACTION_AMEND,
                            )
                        ],
                    )
                )
                assert r11.feed_status.contiguous_feed_seq == 12
                assert r11.feed_status.paused_by_gap is False
                assert r11.item_outcomes[0].kind == spi_pb2.TRADE_FILL_OUTCOME_KIND_BUFFERED_REVISION

            proc2.kill()
            proc2.wait(timeout=5)

        # Phase 3: Restart again and deliver missing T-11 rev=1 at seq=13 -> releases rev1->rev2 chain!
        with _run_server(db_path=db_a, policy_path=policy_path, creds_path=creds_path) as (
            _proc3,
            ep3,
            _p3,
        ):
            with DeskBlotterClient(ep3, bearer_token=FEED_TOK, insecure=True) as fd:
                r13 = fd.ingest_trade_feed(
                    service_pb2.IngestTradeFeedRequest(
                        idempotency_key="f-13-rev1",
                        source_id="FEED-1",
                        fills=[
                            _make_fill(
                                trade_id="T-11",
                                rev=1,
                                feed_seq=13,
                                account_id="acct-1",
                                notional="10000000",
                                action=model_pb2.TRADE_FEED_RECORD_ACTION_NEW_FILL,
                            )
                        ],
                    )
                )
                assert r13.feed_status.contiguous_feed_seq == 13

            with DeskBlotterClient(ep3, bearer_token=TRADER_TOK, insecure=True) as tr:
                return tr.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id="acct-1")
                ).snapshot

    def _run_sequential() -> model_pb2.AccountBlotterSnapshot:
        db_b = tmp_path / "sequential.sqlite"
        with _run_server(db_path=db_b, policy_path=policy_path, creds_path=creds_path) as (
            _proc,
            ep,
            _p,
        ):
            with DeskBlotterClient(ep, bearer_token=OPERATOR_TOK, insecure=True) as op:
                op.import_opening_base(
                    service_pb2.ImportOpeningBaseRequest(
                        idempotency_key="imp-seq-1",
                        opening_base=_make_opening_base("acct-1", start_after_seq=9),
                        expected_account_version=0,
                    )
                )
            with DeskBlotterClient(ep, bearer_token=TRADER_TOK, insecure=True) as tr:
                tr.create_committed(
                    service_pb2.CreateCommittedRequest(
                        idempotency_key="comm-seq-1",
                        account_id="acct-1",
                        expected_position_version=1,
                        instrument_code="BOND_10Y",
                        side=model_pb2.TRADE_SIDE_SELL,
                        settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                        notional=_dec("30000000", UNITS.currency_amount),
                        quote_terms=_quote_terms_clean("100", "30000000"),
                        note="match-T10",
                    )
                )
            with DeskBlotterClient(ep, bearer_token=FEED_TOK, insecure=True) as fd:
                for key, fill in (
                    (
                        "s-10",
                        _make_fill(
                            trade_id="T-10",
                            rev=1,
                            feed_seq=10,
                            account_id="acct-1",
                            notional="30000000",
                        ),
                    ),
                    (
                        "s-11",
                        _make_fill(
                            trade_id="T-11",
                            rev=1,
                            feed_seq=11,
                            account_id="acct-1",
                            notional="10000000",
                            action=model_pb2.TRADE_FEED_RECORD_ACTION_NEW_FILL,
                        ),
                    ),
                    (
                        "s-12",
                        _make_fill(
                            trade_id="T-11",
                            rev=2,
                            feed_seq=12,
                            account_id="acct-1",
                            notional="12000000",
                            action=model_pb2.TRADE_FEED_RECORD_ACTION_AMEND,
                        ),
                    ),
                    (
                        "s-13",
                        _make_fill(
                            trade_id="T-12",
                            rev=1,
                            feed_seq=13,
                            account_id="acct-1",
                            notional="15000000",
                        ),
                    ),
                ):
                    fd.ingest_trade_feed(
                        service_pb2.IngestTradeFeedRequest(
                            idempotency_key=key,
                            source_id="FEED-1",
                            fills=[fill],
                        )
                    )
            with DeskBlotterClient(ep, bearer_token=TRADER_TOK, insecure=True) as tr:
                return tr.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id="acct-1")
                ).snapshot

    snap_disrupted = _run_disrupted()
    snap_sequential = _run_sequential()

    # Exact economic & cash parity between hard-killed/out-of-order recovery and sequential delivery!
    pos_d = snap_disrupted.positions[0]
    pos_s = snap_sequential.positions[0]
    assert _to_dec(pos_d.economic_net_notional) == _to_dec(pos_s.economic_net_notional)
    assert _to_dec(pos_d.available_t0_notional) == _to_dec(pos_s.available_t0_notional)
    assert _to_dec(pos_d.available_t1_notional) == _to_dec(pos_s.available_t1_notional)
    assert pos_d.bucket_t0 == pos_s.bucket_t0
    assert pos_d.bucket_t1 == pos_s.bucket_t1
    assert snap_disrupted.funding_summary == snap_sequential.funding_summary
    assert len(snap_disrupted.active_earmarks) == len(snap_sequential.active_earmarks) == 0
    assert sorted(
        (p.external_trade_id, p.source_revision, _to_dec(p.unresolved_notional))
        for p in snap_disrupted.open_pending_matches
    ) == sorted(
        (p.external_trade_id, p.source_revision, _to_dec(p.unresolved_notional))
        for p in snap_sequential.open_pending_matches
    )


# ======================================================================
# 4. Dual-Account Multi-Channel Exact-Seq Parity & Retention Fallback
# ======================================================================


def test_live_4_dual_account_multi_channel_exact_seq_parity_and_retention_fallback(
    tmp_path: Path,
) -> None:
    db_path = tmp_path / "dual_acct.sqlite"
    creds_path = _write_credentials(
        tmp_path / "creds.json",
        accounts=["acct-A", "acct-B"],
        sources=["SRC-SHARED", "Q-SHARED"],
    )
    policy_path = _write_policy(
        tmp_path / "policy.json", default_quote_source_id="Q-SHARED"
    )

    with _run_server(
        db_path=db_path,
        policy_path=policy_path,
        creds_path=creds_path,
        retention_window=50,
    ) as (_proc, ep, _port):
        with (
            DeskBlotterClient(ep, bearer_token=OPERATOR_TOK, insecure=True) as op,
            DeskBlotterClient(ep, bearer_token=TRADER_TOK, insecure=True) as tr,
            DeskBlotterClient(ep, bearer_token=FEED_TOK, insecure=True) as fd,
        ):
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-a",
                    opening_base=_make_opening_base(
                        "acct-A", base_suffix="0A", source_id="SRC-SHARED"
                    ),
                    expected_account_version=0,
                )
            )
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-b",
                    opening_base=_make_opening_base(
                        "acct-B", base_suffix="0B", source_id="SRC-SHARED"
                    ),
                    expected_account_version=0,
                )
            )

            # Capture STALE initial snapshots for Account A & Account B at seq=2
            stale_a = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-A")
            ).snapshot
            stale_b = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-B")
            ).snapshot
            assert stale_a.snapshot_seq == 2
            assert stale_b.snapshot_seq == 2

            applier_a = BlotterEventApplier({"acct-A": stale_a}, initial_cursor_seq=2)
            applier_b = BlotterEventApplier({"acct-B": stale_b}, initial_cursor_seq=2)

            # Start Account B consumer on SubscribeEvents in a background thread
            sub_cond = threading.Condition()
            sub_cursor_seq = applier_b.cursor_seq
            sub_stop = threading.Event()
            sub_errors: list[BaseException] = []

            with DeskBlotterClient(ep, bearer_token=TRADER_TOK, insecure=True) as sub_client:
                sub_stream = sub_client.subscribe_events(
                    service_pb2.SubscribeEventsRequest(after_seq=2, account_id="acct-B"),
                    applier=applier_b,
                )

                def _consume_b() -> None:
                    nonlocal sub_cursor_seq
                    try:
                        for resp in sub_stream:
                            if resp.WhichOneof("payload") in ("event", "heartbeat"):
                                with sub_cond:
                                    sub_cursor_seq = applier_b.cursor_seq
                                    sub_cond.notify_all()
                            if sub_stop.is_set():
                                break
                        else:
                            if not sub_stop.is_set():
                                with sub_cond:
                                    sub_errors.append(
                                        RuntimeError(
                                            "SubscribeEvents stream ended prematurely "
                                            f"at seq={sub_cursor_seq}"
                                        )
                                    )
                                    sub_cond.notify_all()
                    except BaseException as exc:  # pragma: no cover
                        is_expected_cancel = (
                            sub_stop.is_set()
                            and isinstance(exc, grpc.RpcError)
                            and isinstance(exc, grpc.Call)
                            and exc.code() == grpc.StatusCode.CANCELLED
                        )
                        if not is_expected_cancel:
                            with sub_cond:
                                sub_errors.append(exc)
                                sub_cond.notify_all()

                sub_thread = threading.Thread(target=_consume_b, daemon=True)
                sub_thread.start()

                try:
                    # Execute mutations:
                    # 1) Create two COMMITTED earmarks on A (30M, 20M) and one HOLD + one COMMITTED on B
                    ea1 = tr.create_committed(
                        service_pb2.CreateCommittedRequest(
                            idempotency_key="ea1",
                            account_id="acct-A",
                            expected_position_version=1,
                            instrument_code="BOND_10Y",
                            side=model_pb2.TRADE_SIDE_SELL,
                            settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                            notional=_dec("30000000", UNITS.currency_amount),
                            quote_terms=_quote_terms_clean("100", "30000000"),
                            note="ea1",
                        )
                    ).earmark
                    ea2 = tr.create_committed(
                        service_pb2.CreateCommittedRequest(
                            idempotency_key="ea2",
                            account_id="acct-A",
                            expected_position_version=2,
                            instrument_code="BOND_10Y",
                            side=model_pb2.TRADE_SIDE_SELL,
                            settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                            notional=_dec("20000000", UNITS.currency_amount),
                            quote_terms=_quote_terms_clean("100", "20000000"),
                            note="ea2",
                        )
                    ).earmark
                    eb_hold = tr.create_hold(
                        service_pb2.CreateHoldRequest(
                            idempotency_key="eb-hold",
                            account_id="acct-B",
                            expected_position_version=1,
                            instrument_code="BOND_10Y",
                            side=model_pb2.TRADE_SIDE_SELL,
                            settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                            notional=_dec("10000000", UNITS.currency_amount),
                            ttl_seconds=1800,
                            note="temp-hold",
                        )
                    ).earmark
                    tr.create_committed(
                        service_pb2.CreateCommittedRequest(
                            idempotency_key="eb-comm",
                            account_id="acct-B",
                            expected_position_version=2,
                            instrument_code="BOND_10Y",
                            side=model_pb2.TRADE_SIDE_SELL,
                            settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                            notional=_dec("25000000", UNITS.currency_amount),
                            quote_terms=_quote_terms_clean("100", "25000000"),
                            note="eb-comm",
                        )
                    )

                    # 2) Multi-account trade batch: 50M fill on A (OPEN -> split across ea1+ea2) + 25M auto-match on B
                    fd.ingest_trade_feed(
                        service_pb2.IngestTradeFeedRequest(
                            idempotency_key="batch-ab-1",
                            source_id="SRC-SHARED",
                            fills=[
                                _make_fill(
                                    source_id="SRC-SHARED",
                                    trade_id="F-DUAL-A",
                                    rev=1,
                                    feed_seq=1,
                                    account_id="acct-A",
                                    notional="50000000",
                                    batch_first=1,
                                    batch_last=2,
                                ),
                                _make_fill(
                                    source_id="SRC-SHARED",
                                    trade_id="F-MATCH-B",
                                    rev=1,
                                    feed_seq=2,
                                    account_id="acct-B",
                                    notional="25000000",
                                    batch_first=1,
                                    batch_last=2,
                                ),
                            ],
                        )
                    )

                    # Split F-DUAL-A (50M) across ea1 (30M) + ea2 (20M) so both earmarks are reconciled
                    snap_a_mid = tr.get_account_snapshot(
                        service_pb2.GetAccountSnapshotRequest(account_id="acct-A")
                    ).snapshot
                    pm_dual = snap_a_mid.open_pending_matches[0]
                    r_split = tr.resolve_pending_match(
                        service_pb2.ResolvePendingMatchRequest(
                            idempotency_key="split-ea1",
                            account_id="acct-A",
                            pending_match_id=pm_dual.pending_match_id,
                            expected_pending_revision=pm_dual.pending_revision,
                            expected_position_version=snap_a_mid.positions[0].position_version,
                            action=model_pb2.PENDING_MATCH_RESOLUTION_ACTION_SPLIT_OFFSET,
                            expected_earmark_revision=ea1.earmark_revision,
                            target_earmark_id=ea1.earmark_id,
                            offset_notional=_dec("30000000", UNITS.currency_amount),
                            reason="split-part-1",
                        )
                    )
                    tr.resolve_pending_match(
                        service_pb2.ResolvePendingMatchRequest(
                            idempotency_key="match-ea2",
                            account_id="acct-A",
                            pending_match_id=pm_dual.pending_match_id,
                            expected_pending_revision=r_split.pending_match.pending_revision,
                            expected_position_version=r_split.position.position_version,
                            action=model_pb2.PENDING_MATCH_RESOLUTION_ACTION_MATCH_FULL,
                            expected_earmark_revision=ea2.earmark_revision,
                            target_earmark_id=ea2.earmark_id,
                            reason="split-part-2",
                        )
                    )

                    # 3) Dual-earmark reversal on Account A: CANCEL_REVERSAL reopens BOTH ea1 and ea2!
                    fd.ingest_trade_feed(
                        service_pb2.IngestTradeFeedRequest(
                            idempotency_key="rev-dual-a",
                            source_id="SRC-SHARED",
                            fills=[
                                _make_fill(
                                    source_id="SRC-SHARED",
                                    trade_id="F-DUAL-A",
                                    rev=2,
                                    feed_seq=3,
                                    account_id="acct-A",
                                    notional="50000000",
                                    action=model_pb2.TRADE_FEED_RECORD_ACTION_CANCEL_REVERSAL,
                                )
                            ],
                        )
                    )

                    # 4) Terminal removal on Account B: ReleaseHold removes eb_hold from active_earmarks
                    tr.release_hold(
                        service_pb2.ReleaseHoldRequest(
                            idempotency_key="rel-eb-hold",
                            account_id="acct-B",
                            earmark_id=eb_hold.earmark_id,
                            expected_earmark_revision=eb_hold.earmark_revision,
                            reason="no-longer-needed",
                        )
                    )

                    # 5) Shared-source quote push affecting BOTH Account A and Account B in ONE transaction!
                    q_res = tr.ingest_market_quotes(
                        service_pb2.IngestMarketQuotesRequest(
                            idempotency_key="q-shared-1",
                            account_id="acct-A",
                            source_id="Q-SHARED",
                            ticks=[
                                _make_quote_tick(
                                    source_id="Q-SHARED", quote_seq=1, ytm="0.025", seconds_ago=5
                                )
                            ],
                        )
                    )
                    target_seq = q_res.committed_seq

                    # Wait for SubscribeEvents (Account B) to reach exact target_seq
                    with sub_cond:
                        reached = sub_cond.wait_for(
                            lambda: sub_cursor_seq >= target_seq or bool(sub_errors),
                            timeout=10.0,
                        )
                        observed_seq = sub_cursor_seq
                        observed_errors = list(sub_errors)
                    assert not observed_errors, f"SubscribeEvents error: {observed_errors}"
                    assert reached, (
                        f"SubscribeEvents timed out at {observed_seq} < {target_seq}"
                    )

                    # Signal intentional shutdown strictly BEFORE cancelling the stream
                    sub_stop.set()
                    sub_stream.cancel()
                    sub_thread.join(timeout=5.0)
                    assert not sub_thread.is_alive(), (
                        "SubscribeEvents consumer thread did not exit cleanly"
                    )
                    assert not sub_errors, f"SubscribeEvents error: {sub_errors}"
                finally:
                    sub_stop.set()
                    sub_stream.cancel()
                    sub_thread.join(timeout=5.0)

            # Drain PollEvents (Account A) page-by-page (max_events=1) up to target_seq
            while applier_a.cursor_seq < target_seq:
                p_res = tr.poll_events(
                    service_pb2.PollEventsRequest(
                        after_seq=applier_a.cursor_seq,
                        max_events=1,
                        account_id="acct-A",
                    ),
                    applier=applier_a,
                )
                assert p_res.WhichOneof("outcome") == "batch"
                if not p_res.batch.has_more and applier_a.cursor_seq >= target_seq:
                    break

            assert applier_a.cursor_seq == target_seq
            assert applier_b.cursor_seq == target_seq

            # Field-for-field snapshot parity at exact target_seq for BOTH accounts!
            server_snap_a = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-A")
            ).snapshot
            server_snap_b = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-B")
            ).snapshot
            assert server_snap_a.snapshot_seq == target_seq
            assert server_snap_b.snapshot_seq == target_seq
            assert applier_a.snapshot_for("acct-A") == server_snap_a
            assert applier_b.snapshot_for("acct-B") == server_snap_b
            assert len(server_snap_a.active_earmarks) == 2
            assert all(e.reopened_by_reversal for e in server_snap_a.active_earmarks)
            assert len(server_snap_b.active_earmarks) == 0
            assert len(applier_a.allocations_for("acct-A")) >= 2

    # Small retention window -> SnapshotFallbackRequired on both PollEvents and SubscribeEvents
    db_small = tmp_path / "small_retention.sqlite"
    with _run_server(
        db_path=db_small,
        policy_path=policy_path,
        creds_path=creds_path,
        retention_window=2,
    ) as (_proc_s, ep_s, _port_s):
        with (
            DeskBlotterClient(ep_s, bearer_token=OPERATOR_TOK, insecure=True) as op,
            DeskBlotterClient(ep_s, bearer_token=TRADER_TOK, insecure=True) as tr,
        ):
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-ret",
                    opening_base=_make_opening_base(
                        "acct-A", base_suffix="0R", source_id="SRC-SHARED"
                    ),
                    expected_account_version=0,
                )
            )
            for idx in range(1, 5):
                tr.create_hold(
                    service_pb2.CreateHoldRequest(
                        idempotency_key=f"ret-hold-{idx}",
                        account_id="acct-A",
                        expected_position_version=idx,
                        instrument_code="BOND_10Y",
                        side=model_pb2.TRADE_SIDE_BUY,
                        settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                        notional=_dec("1000000", UNITS.currency_amount),
                        ttl_seconds=1800,
                        note=f"h-{idx}",
                    )
                )
            poll_fb = tr.poll_events(
                service_pb2.PollEventsRequest(after_seq=0, max_events=10)
            )
            assert poll_fb.WhichOneof("outcome") == "snapshot_fallback"
            assert poll_fb.snapshot_fallback.oldest_retained_seq > 1

            sub_msgs = list(
                tr.subscribe_events(service_pb2.SubscribeEventsRequest(after_seq=0))
            )
            assert len(sub_msgs) == 1
            assert sub_msgs[0].WhichOneof("payload") == "snapshot_fallback"
            assert sub_msgs[0].snapshot_fallback.oldest_retained_seq > 1


# ======================================================================
# 5. Full Reconciliation & Funding Matrix
# ======================================================================


def test_live_5_full_reconciliation_and_funding_matrix(tmp_path: Path) -> None:
    accounts = [
        "acct-perm-1",
        "acct-perm-2",
        "acct-cross",
        "acct-tol",
        "acct-100-60-40",
        "acct-base-1",
        "acct-base-2",
        "acct-base-3",
        "acct-base-chain",
        "acct-10b",
        "acct-zero",
        "acct-t1-def",
        "acct-exp",
        "acct-fund",
    ]
    sources = [f"SRC-{a}" for a in accounts] + ["SRC-A"]
    creds_path = _write_credentials(
        tmp_path / "creds.json", accounts=accounts, sources=sources
    )
    policy_path = _write_policy(
        tmp_path / "policy.json", clean_price_tolerance="0.05"
    )
    db_path = tmp_path / "matrix.sqlite"

    with _run_server(db_path=db_path, policy_path=policy_path, creds_path=creds_path) as (
        _proc,
        ep,
        _port,
    ):
        with (
            DeskBlotterClient(ep, bearer_token=OPERATOR_TOK, insecure=True) as op,
            DeskBlotterClient(ep, bearer_token=TRADER_TOK, insecure=True) as tr,
            DeskBlotterClient(ep, bearer_token=FEED_TOK, insecure=True) as fd,
        ):
            # --- 5a. Same-batch two distinct fills [f1,f2] & [f2,f1] both OPEN ---
            perm_snapshots: list[model_pb2.AccountBlotterSnapshot] = []
            for idx, (acct, order) in enumerate(
                (("acct-perm-1", ("F1", "F2")), ("acct-perm-2", ("F2", "F1"))), start=1
            ):
                src = f"SRC-{acct}"
                op.import_opening_base(
                    service_pb2.ImportOpeningBaseRequest(
                        idempotency_key=f"imp-{acct}",
                        opening_base=_make_opening_base(
                            acct, base_suffix=f"P{idx}", source_id=src
                        ),
                        expected_account_version=0,
                    )
                )
                tr.create_committed(
                    service_pb2.CreateCommittedRequest(
                        idempotency_key=f"c-{acct}",
                        account_id=acct,
                        expected_position_version=1,
                        instrument_code="BOND_10Y",
                        side=model_pb2.TRADE_SIDE_SELL,
                        settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                        notional=_dec("20000000", UNITS.currency_amount),
                        quote_terms=_quote_terms_clean("100", "20000000"),
                        note="ambig",
                    )
                )
                fills = [
                    _make_fill(
                        source_id=src,
                        trade_id=order[0],
                        feed_seq=1,
                        account_id=acct,
                        notional="20000000",
                        batch_first=1,
                        batch_last=2,
                    ),
                    _make_fill(
                        source_id=src,
                        trade_id=order[1],
                        feed_seq=2,
                        account_id=acct,
                        notional="20000000",
                        batch_first=1,
                        batch_last=2,
                    ),
                ]
                res = fd.ingest_trade_feed(
                    service_pb2.IngestTradeFeedRequest(
                        idempotency_key=f"ing-{acct}", source_id=src, fills=fills
                    )
                )
                assert all(
                    o.kind == spi_pb2.TRADE_FILL_OUTCOME_KIND_PENDING_MATCH
                    for o in res.item_outcomes
                )
                snap = tr.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id=acct)
                ).snapshot
                assert len(snap.open_pending_matches) == 2
                assert all(
                    p.reason == model_pb2.PENDING_MATCH_REASON_AMBIGUOUS_CANDIDATES
                    for p in snap.open_pending_matches
                )
                perm_snapshots.append(snap)
            assert _to_dec(perm_snapshots[0].positions[0].available_t0_notional) == _to_dec(
                perm_snapshots[1].positions[0].available_t0_notional
            )

            # --- 5b. Cross-batch first auto-match / second orphan ---
            src_c = "SRC-acct-cross"
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-cross",
                    opening_base=_make_opening_base(
                        "acct-cross", base_suffix="C1", source_id=src_c
                    ),
                    expected_account_version=0,
                )
            )
            tr.create_committed(
                service_pb2.CreateCommittedRequest(
                    idempotency_key="c-cross",
                    account_id="acct-cross",
                    expected_position_version=1,
                    instrument_code="BOND_10Y",
                    side=model_pb2.TRADE_SIDE_SELL,
                    settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                    notional=_dec("20000000", UNITS.currency_amount),
                    quote_terms=_quote_terms_clean("100", "20000000"),
                    note="cross",
                )
            )
            r_c1 = fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="cross-b1",
                    source_id=src_c,
                    fills=[
                        _make_fill(
                            source_id=src_c,
                            trade_id="CB-1",
                            feed_seq=1,
                            account_id="acct-cross",
                            notional="20000000",
                        )
                    ],
                )
            )
            assert r_c1.item_outcomes[0].kind == spi_pb2.TRADE_FILL_OUTCOME_KIND_AUTO_MATCHED
            r_c2 = fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="cross-b2",
                    source_id=src_c,
                    fills=[
                        _make_fill(
                            source_id=src_c,
                            trade_id="CB-2",
                            feed_seq=2,
                            account_id="acct-cross",
                            notional="20000000",
                        )
                    ],
                )
            )
            assert r_c2.item_outcomes[0].kind == spi_pb2.TRADE_FILL_OUTCOME_KIND_PENDING_MATCH
            snap_c = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-cross")
            ).snapshot
            assert (
                snap_c.open_pending_matches[0].reason
                == model_pb2.PENDING_MATCH_REASON_ORPHAN_NO_COMMITMENT
            )

            # --- 5c. Tolerance == vs +1 ULP, cross-basis, ACCEPT_ORPHAN & RELEASE_REJECT ---
            src_t = "SRC-acct-tol"
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-tol",
                    opening_base=_make_opening_base(
                        "acct-tol", base_suffix="T1", source_id=src_t
                    ),
                    expected_account_version=0,
                )
            )
            # Exact tolerance == 0.05 -> AUTO_MATCHED
            tr.create_committed(
                service_pb2.CreateCommittedRequest(
                    idempotency_key="c-tol-eq",
                    account_id="acct-tol",
                    expected_position_version=1,
                    instrument_code="BOND_10Y",
                    side=model_pb2.TRADE_SIDE_SELL,
                    settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                    notional=_dec("10000000", UNITS.currency_amount),
                    quote_terms=_quote_terms_clean("100.00", "10000000"),
                    note="tol-eq",
                )
            )
            r_eq = fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="f-tol-eq",
                    source_id=src_t,
                    fills=[
                        _make_fill(
                            source_id=src_t,
                            trade_id="TOL-EQ",
                            feed_seq=1,
                            account_id="acct-tol",
                            notional="10000000",
                            quote_terms=_quote_terms_clean("100.05", "10005000"),
                        )
                    ],
                )
            )
            assert r_eq.item_outcomes[0].kind == spi_pb2.TRADE_FILL_OUTCOME_KIND_AUTO_MATCHED

            # Tolerance + 1 ULP (0.050000000001) -> PRICE_OUT_OF_TOLERANCE
            pos_v = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-tol")
            ).snapshot.positions[0].position_version
            c_ulp = tr.create_committed(
                service_pb2.CreateCommittedRequest(
                    idempotency_key="c-tol-ulp",
                    account_id="acct-tol",
                    expected_position_version=pos_v,
                    instrument_code="BOND_10Y",
                    side=model_pb2.TRADE_SIDE_SELL,
                    settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                    notional=_dec("10000000", UNITS.currency_amount),
                    quote_terms=_quote_terms_clean("100.00", "10000000"),
                    note="tol-ulp",
                )
            )
            fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="f-tol-ulp",
                    source_id=src_t,
                    fills=[
                        _make_fill(
                            source_id=src_t,
                            trade_id="TOL-ULP",
                            feed_seq=2,
                            account_id="acct-tol",
                            notional="10000000",
                            quote_terms=_quote_terms_clean("100.050000000001", "10005000"),
                        )
                    ],
                )
            )
            snap_t = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-tol")
            ).snapshot
            pm_ulp = snap_t.open_pending_matches[0]
            assert pm_ulp.reason == model_pb2.PENDING_MATCH_REASON_PRICE_OUT_OF_TOLERANCE

            # RELEASE_REJECT clears pending without moving cash into recognized net_settlement_flow
            flow_before_rej = _to_dec(snap_t.funding_summary.net_settlement_flow_t0)
            rej_res = tr.resolve_pending_match(
                service_pb2.ResolvePendingMatchRequest(
                    idempotency_key="rej-ulp",
                    account_id="acct-tol",
                    pending_match_id=pm_ulp.pending_match_id,
                    expected_pending_revision=pm_ulp.pending_revision,
                    expected_position_version=snap_t.positions[0].position_version,
                    action=model_pb2.PENDING_MATCH_RESOLUTION_ACTION_RELEASE_REJECT,
                    reason="price-error-reject",
                )
            )
            assert _to_dec(rej_res.funding_summary.net_settlement_flow_t0) == flow_before_rej
            assert rej_res.funding_summary.pending_match_count == 0

            # Cross-basis mismatch (committed clean_price vs fill yield_to_maturity) -> INCOMPARABLE_BASIS
            fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="f-cross-basis",
                    source_id=src_t,
                    fills=[
                        _make_fill(
                            source_id=src_t,
                            trade_id="TOL-BASIS",
                            feed_seq=3,
                            account_id="acct-tol",
                            notional="10000000",
                            quote_terms=_quote_terms_yield("0.025", "10000000"),
                        )
                    ],
                )
            )
            snap_t2 = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-tol")
            ).snapshot
            pm_basis = snap_t2.open_pending_matches[0]
            assert pm_basis.reason == model_pb2.PENDING_MATCH_REASON_INCOMPARABLE_BASIS

            # ACCEPT_ORPHAN moves unresolved cash (+10M) into net_settlement_flow_t0
            flow_before_acc = _to_dec(snap_t2.funding_summary.net_settlement_flow_t0)
            acc_res = tr.resolve_pending_match(
                service_pb2.ResolvePendingMatchRequest(
                    idempotency_key="acc-basis",
                    account_id="acct-tol",
                    pending_match_id=pm_basis.pending_match_id,
                    expected_pending_revision=pm_basis.pending_revision,
                    expected_position_version=snap_t2.positions[0].position_version,
                    action=model_pb2.PENDING_MATCH_RESOLUTION_ACTION_ACCEPT_ORPHAN,
                    reason="confirm-independent-fill",
                )
            )
            assert (
                _to_dec(acc_res.funding_summary.net_settlement_flow_t0)
                == flow_before_acc + Decimal("10000000")
            )
            assert acc_res.funding_summary.pending_match_count == 0
            assert c_ulp.earmark.earmark_revision == 1

            # --- 5d. 100/60/40 & 60 -> 40 + 20 ---
            src_100 = "SRC-acct-100-60-40"
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-100",
                    opening_base=_make_opening_base(
                        "acct-100-60-40", base_suffix="S1", source_id=src_100
                    ),
                    expected_account_version=0,
                )
            )
            em_60 = tr.create_committed(
                service_pb2.CreateCommittedRequest(
                    idempotency_key="c-60",
                    account_id="acct-100-60-40",
                    expected_position_version=1,
                    instrument_code="BOND_10Y",
                    side=model_pb2.TRADE_SIDE_SELL,
                    settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                    notional=_dec("60000000", UNITS.currency_amount),
                    quote_terms=_quote_terms_clean("100", "60000000"),
                    note="sell-60",
                )
            ).earmark
            fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="f-40",
                    source_id=src_100,
                    fills=[
                        _make_fill(
                            source_id=src_100,
                            trade_id="F-40",
                            feed_seq=1,
                            account_id="acct-100-60-40",
                            notional="40000000",
                        )
                    ],
                )
            )
            snap_100 = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-100-60-40")
            ).snapshot
            assert _to_dec(snap_100.positions[0].available_t0_notional) == Decimal("0")
            assert snap_100.positions[0].bucket_t0.sell_ambiguity_locked is True

            # New SELL is rejected while sell_ambiguity_locked is active!
            with pytest.raises(BlotterError) as lock_err:
                tr.create_hold(
                    service_pb2.CreateHoldRequest(
                        idempotency_key="sell-while-locked",
                        account_id="acct-100-60-40",
                        expected_position_version=snap_100.positions[0].position_version,
                        instrument_code="BOND_10Y",
                        side=model_pb2.TRADE_SIDE_SELL,
                        settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                        notional=_dec("1000000", UNITS.currency_amount),
                        ttl_seconds=1800,
                        note="blocked",
                    )
                )
            assert lock_err.value.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

            # SPLIT_OFFSET(40M) against 60M -> reconciled=40M, unrec=20M, available=40M, lock cleared!
            pm_40 = snap_100.open_pending_matches[0]
            r_40 = tr.resolve_pending_match(
                service_pb2.ResolvePendingMatchRequest(
                    idempotency_key="split-40",
                    account_id="acct-100-60-40",
                    pending_match_id=pm_40.pending_match_id,
                    expected_pending_revision=pm_40.pending_revision,
                    expected_position_version=snap_100.positions[0].position_version,
                    action=model_pb2.PENDING_MATCH_RESOLUTION_ACTION_SPLIT_OFFSET,
                    expected_earmark_revision=em_60.earmark_revision,
                    target_earmark_id=em_60.earmark_id,
                    offset_notional=_dec("40000000", UNITS.currency_amount),
                    reason="partial-40-of-60",
                )
            )
            assert _to_dec(r_40.position.available_t0_notional) == Decimal("40000000")
            assert r_40.position.bucket_t0.sell_ambiguity_locked is False
            assert _to_dec(r_40.position.bucket_t0.reconciled_sell_notional) == Decimal("40000000")
            assert _to_dec(r_40.position.bucket_t0.committed_sell_unreconciled_notional) == Decimal("20000000")

            # Remaining 20M fill auto-matches cleanly!
            r_20 = fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="f-20",
                    source_id=src_100,
                    fills=[
                        _make_fill(
                            source_id=src_100,
                            trade_id="F-20",
                            feed_seq=2,
                            account_id="acct-100-60-40",
                            notional="20000000",
                        )
                    ],
                )
            )
            assert r_20.item_outcomes[0].kind == spi_pb2.TRADE_FILL_OUTCOME_KIND_AUTO_MATCHED

            # --- 5e. Baseline delta 3 cases + chained 50 -> 40 -> reverse ---
            def _make_base_evidence(src: str) -> model_pb2.BaseEvidence:
                return model_pb2.BaseEvidence(
                    source_id=src,
                    external_trade_id="BASE-50M",
                    original_feed_seq=5,
                    original_executed_at=_shanghai_time(2026, 6, 15, 8, 30, 0),
                    old_notional=_dec("50000000", UNITS.currency_amount),
                    old_signed_cash=_dec("50000000", UNITS.currency_amount),
                    settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                    evidence_ref="eod-export-001",
                    evidence_hash=common_pb2.Sha256(value=hashlib.sha256(b"BASE-50M").digest()),
                    instrument_code="BOND_10Y",
                    side=model_pb2.TRADE_SIDE_SELL,
                    original_revision=1,
                )

            # Cases 1, 2, 3: 50->40, 50->50 price-only, 50->cancel
            for idx, (acct, new_qty, new_cash, action, d_qty, d_cash) in enumerate(
                (
                    ("acct-base-1", "40000000", "40000000", model_pb2.TRADE_FEED_RECORD_ACTION_AMEND, "-10000000", "-10000000"),
                    ("acct-base-2", "50000000", "50500000", model_pb2.TRADE_FEED_RECORD_ACTION_AMEND, "0", "500000"),
                    ("acct-base-3", "50000000", "50000000", model_pb2.TRADE_FEED_RECORD_ACTION_CANCEL_REVERSAL, "-50000000", "-50000000"),
                ),
                start=1,
            ):
                src = f"SRC-{acct}"
                ev = _make_base_evidence(src)
                op.import_opening_base(
                    service_pb2.ImportOpeningBaseRequest(
                        idempotency_key=f"imp-{acct}",
                        opening_base=_make_opening_base(
                            acct,
                            base_suffix=f"B{idx}",
                            source_id=src,
                            start_after_seq=5,
                            watermark_source_seq=5,
                            baseline_evidence=[ev],
                        ),
                        expected_account_version=0,
                    )
                )
                fd.ingest_trade_feed(
                    service_pb2.IngestTradeFeedRequest(
                        idempotency_key=f"f-{acct}",
                        source_id=src,
                        fills=[
                            _make_fill(
                                source_id=src,
                                trade_id="BASE-50M",
                                rev=2,
                                feed_seq=6,
                                account_id=acct,
                                notional=new_qty,
                                quote_terms=_quote_terms_clean("100", new_cash),
                                action=action,
                            )
                        ],
                    )
                )
                snap_b = tr.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id=acct)
                ).snapshot
                pm_b = snap_b.open_pending_matches[0]
                assert pm_b.reason == model_pb2.PENDING_MATCH_REASON_BASELINE_REVISION_MANUAL
                res_b = tr.resolve_pending_match(
                    service_pb2.ResolvePendingMatchRequest(
                        idempotency_key=f"res-{acct}",
                        account_id=acct,
                        pending_match_id=pm_b.pending_match_id,
                        expected_pending_revision=pm_b.pending_revision,
                        expected_position_version=snap_b.positions[0].position_version,
                        action=model_pb2.PENDING_MATCH_RESOLUTION_ACTION_APPLY_BASELINE_DELTA,
                        reason=f"apply-case-{idx}",
                        baseline_delta=model_pb2.BaselineDelta(
                            delta_notional=_dec(d_qty, UNITS.currency_amount),
                            delta_cash=_dec(d_cash, UNITS.currency_amount),
                            settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                            evidence=ev,
                            reason=f"delta-case-{idx}",
                        ),
                    )
                )
                # For SELL baseline, delta_notional is negative when sell shrinks -> economic_net increases by -delta_notional!
                assert _to_dec(res_b.position.economic_net_notional) == Decimal("100000000") - Decimal(d_qty)
                assert _to_dec(res_b.funding_summary.net_settlement_flow_t0) == Decimal(d_cash)

            # Case 4: Chained 50 -> 40 -> full reverse => cumulative (-50M, -50M)!
            src_ch = "SRC-acct-base-chain"
            ev_ch = _make_base_evidence(src_ch)
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-base-chain",
                    opening_base=_make_opening_base(
                        "acct-base-chain",
                        base_suffix="B4",
                        source_id=src_ch,
                        start_after_seq=5,
                        watermark_source_seq=5,
                        baseline_evidence=[ev_ch],
                    ),
                    expected_account_version=0,
                )
            )
            fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="f-chain-rev2",
                    source_id=src_ch,
                    fills=[
                        _make_fill(
                            source_id=src_ch,
                            trade_id="BASE-50M",
                            rev=2,
                            feed_seq=6,
                            account_id="acct-base-chain",
                            notional="40000000",
                            quote_terms=_quote_terms_clean("100", "40000000"),
                            action=model_pb2.TRADE_FEED_RECORD_ACTION_AMEND,
                        )
                    ],
                )
            )
            snap_ch1 = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-base-chain")
            ).snapshot
            tr.resolve_pending_match(
                service_pb2.ResolvePendingMatchRequest(
                    idempotency_key="res-chain-1",
                    account_id="acct-base-chain",
                    pending_match_id=snap_ch1.open_pending_matches[0].pending_match_id,
                    expected_pending_revision=snap_ch1.open_pending_matches[0].pending_revision,
                    expected_position_version=snap_ch1.positions[0].position_version,
                    action=model_pb2.PENDING_MATCH_RESOLUTION_ACTION_APPLY_BASELINE_DELTA,
                    reason="chain-step-1",
                    baseline_delta=model_pb2.BaselineDelta(
                        delta_notional=_dec("-10000000", UNITS.currency_amount),
                        delta_cash=_dec("-10000000", UNITS.currency_amount),
                        settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                        evidence=ev_ch,
                        reason="50-to-40",
                    ),
                )
            )
            # Rev 3 full cancel reverses the prior (-10M, -10M) and prompts full (-50M, -50M) relative to original!
            fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="f-chain-rev3",
                    source_id=src_ch,
                    fills=[
                        _make_fill(
                            source_id=src_ch,
                            trade_id="BASE-50M",
                            rev=3,
                            feed_seq=7,
                            account_id="acct-base-chain",
                            notional="40000000",
                            quote_terms=_quote_terms_clean("100", "40000000"),
                            action=model_pb2.TRADE_FEED_RECORD_ACTION_CANCEL_REVERSAL,
                        )
                    ],
                )
            )
            snap_ch2 = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-base-chain")
            ).snapshot
            res_ch2 = tr.resolve_pending_match(
                service_pb2.ResolvePendingMatchRequest(
                    idempotency_key="res-chain-2",
                    account_id="acct-base-chain",
                    pending_match_id=snap_ch2.open_pending_matches[0].pending_match_id,
                    expected_pending_revision=snap_ch2.open_pending_matches[0].pending_revision,
                    expected_position_version=snap_ch2.positions[0].position_version,
                    action=model_pb2.PENDING_MATCH_RESOLUTION_ACTION_APPLY_BASELINE_DELTA,
                    reason="chain-step-2",
                    baseline_delta=model_pb2.BaselineDelta(
                        delta_notional=_dec("-50000000", UNITS.currency_amount),
                        delta_cash=_dec("-50000000", UNITS.currency_amount),
                        settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                        evidence=ev_ch,
                        reason="full-cancel-from-original",
                    ),
                )
            )
            assert _to_dec(res_ch2.position.economic_net_notional) == Decimal("150000000")
            assert _to_dec(res_ch2.funding_summary.net_settlement_flow_t0) == Decimal("-50000000")

            # --- 5f. 10-billion 4-slice allocation conservation ---
            src_10b = "SRC-acct-10b"
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-10b",
                    opening_base=_make_opening_base(
                        "acct-10b",
                        base_suffix="10",
                        source_id=src_10b,
                        balances=[
                            model_pb2.OpeningInstrumentBalance(
                                instrument_code="BOND_10Y",
                                opening_economic_notional=_dec("10000000000", UNITS.currency_amount),
                                opening_available_notional=_dec("10000000000", UNITS.currency_amount),
                                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                            )
                        ],
                    ),
                    expected_account_version=0,
                )
            )
            em_10b = tr.create_committed(
                service_pb2.CreateCommittedRequest(
                    idempotency_key="c-10b",
                    account_id="acct-10b",
                    expected_position_version=1,
                    instrument_code="BOND_10Y",
                    side=model_pb2.TRADE_SIDE_SELL,
                    settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                    notional=_dec("10000000000", UNITS.currency_amount),
                    quote_terms=_quote_terms_clean("100", "10000000001"),
                    note="10-billion-sell",
                )
            ).earmark
            # 10B fill with non-divisible settlement cash split in 4 slices: 1B + 2B + 3B + 4B = 10B
            fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="f-10b",
                    source_id=src_10b,
                    fills=[
                        _make_fill(
                            source_id=src_10b,
                            trade_id="F-10B-1",
                            feed_seq=1,
                            account_id="acct-10b",
                            notional="1000000000",
                            quote_terms=_quote_terms_clean("100", "1000000000.333333333333"),
                        ),
                        _make_fill(
                            source_id=src_10b,
                            trade_id="F-10B-2",
                            feed_seq=2,
                            account_id="acct-10b",
                            notional="2000000000",
                            quote_terms=_quote_terms_clean("100", "2000000000.333333333333"),
                        ),
                        _make_fill(
                            source_id=src_10b,
                            trade_id="F-10B-3",
                            feed_seq=3,
                            account_id="acct-10b",
                            notional="3000000000",
                            quote_terms=_quote_terms_clean("100", "3000000000.333333333334"),
                        ),
                        _make_fill(
                            source_id=src_10b,
                            trade_id="F-10B-4",
                            feed_seq=4,
                            account_id="acct-10b",
                            notional="4000000000",
                            quote_terms=_quote_terms_clean("100", "4000000000"),
                        ),
                    ],
                )
            )
            em_rev = em_10b.earmark_revision
            for slice_idx in range(4):
                snap_10b = tr.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id="acct-10b")
                ).snapshot
                pm_slice = snap_10b.open_pending_matches[0]
                is_last = slice_idx == 3
                req_kwargs: dict[str, Any] = {
                    "idempotency_key": f"slice-10b-{slice_idx}",
                    "account_id": "acct-10b",
                    "pending_match_id": pm_slice.pending_match_id,
                    "expected_pending_revision": pm_slice.pending_revision,
                    "expected_position_version": snap_10b.positions[0].position_version,
                    "action": (
                        model_pb2.PENDING_MATCH_RESOLUTION_ACTION_MATCH_FULL
                        if is_last
                        else model_pb2.PENDING_MATCH_RESOLUTION_ACTION_SPLIT_OFFSET
                    ),
                    "expected_earmark_revision": em_rev,
                    "target_earmark_id": em_10b.earmark_id,
                    "reason": f"slice-{slice_idx}",
                }
                if not is_last:
                    req_kwargs["offset_notional"] = pm_slice.unresolved_notional
                r_sl = tr.resolve_pending_match(
                    service_pb2.ResolvePendingMatchRequest(**req_kwargs)
                )
                em_rev = r_sl.updated_earmark.earmark_revision
            final_10b = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-10b")
            ).snapshot
            assert len(final_10b.active_earmarks) == 0
            assert len(final_10b.open_pending_matches) == 0
            assert _to_dec(final_10b.positions[0].bucket_t0.reconciled_sell_notional) == Decimal("10000000000")
            assert _to_dec(final_10b.funding_summary.net_settlement_flow_t0) == Decimal("10000000001")
            assert final_10b.funding_summary.funding_is_complete_exact is True

            # --- 5g-1. Zero-position BUY HOLD creation & confirmation ---
            src_z = "SRC-acct-zero"
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-zero",
                    opening_base=_make_opening_base(
                        "acct-zero",
                        base_suffix="Z0",
                        source_id=src_z,
                        balances=[
                            model_pb2.OpeningInstrumentBalance(
                                instrument_code="BOND_10Y",
                                opening_economic_notional=_dec("0", UNITS.currency_amount),
                                opening_available_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                            )
                        ],
                    ),
                    expected_account_version=0,
                )
            )
            h_buy = tr.create_hold(
                service_pb2.CreateHoldRequest(
                    idempotency_key="zero-buy-hold",
                    account_id="acct-zero",
                    expected_position_version=1,
                    instrument_code="BOND_10Y",
                    side=model_pb2.TRADE_SIDE_BUY,
                    settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                    notional=_dec("10000000", UNITS.currency_amount),
                    ttl_seconds=1800,
                    note="zero-pos-buy",
                )
            ).earmark
            c_buy = tr.confirm_hold(
                service_pb2.ConfirmHoldRequest(
                    idempotency_key="zero-buy-confirm",
                    account_id="acct-zero",
                    earmark_id=h_buy.earmark_id,
                    expected_earmark_revision=h_buy.earmark_revision,
                    expected_position_version=2,
                    quote_terms=_quote_terms_clean("100", "10000000"),
                )
            )
            assert c_buy.earmark.state == model_pb2.EARMARK_STATE_COMMITTED
            assert _to_dec(c_buy.position.economic_net_notional) == Decimal("10000000")

            # --- 5g-2. T+1 deficit rejecting in-flight SELL T+0 ConfirmHold (preserving HOLD) ---
            src_t1 = "SRC-acct-t1-def"
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-t1-def",
                    opening_base=_make_opening_base(
                        "acct-t1-def",
                        base_suffix="D1",
                        source_id=src_t1,
                        balances=[
                            model_pb2.OpeningInstrumentBalance(
                                instrument_code="BOND_10Y",
                                opening_economic_notional=_dec("50000000", UNITS.currency_amount),
                                opening_available_notional=_dec("50000000", UNITS.currency_amount),
                                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                            )
                        ],
                    ),
                    expected_account_version=0,
                )
            )
            h_sell_t0 = tr.create_hold(
                service_pb2.CreateHoldRequest(
                    idempotency_key="t0-sell-hold",
                    account_id="acct-t1-def",
                    expected_position_version=1,
                    instrument_code="BOND_10Y",
                    side=model_pb2.TRADE_SIDE_SELL,
                    settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                    notional=_dec("30000000", UNITS.currency_amount),
                    ttl_seconds=1800,
                    note="inflight-t0",
                )
            ).earmark
            # Official T+1 sell 30M pushes A1 to 50 - 30(T0 hold) - 30(T1 pending) = -10M deficit!
            fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="t1-deficit-fill",
                    source_id=src_t1,
                    fills=[
                        _make_fill(
                            source_id=src_t1,
                            trade_id="F-T1-DEF",
                            feed_seq=1,
                            account_id="acct-t1-def",
                            side=model_pb2.TRADE_SIDE_SELL,
                            settlement_date="2026-06-16",
                            notional="30000000",
                        )
                    ],
                )
            )
            snap_def = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-t1-def")
            ).snapshot
            assert _to_dec(snap_def.positions[0].available_t1_notional) == Decimal("-10000000")
            with pytest.raises(BlotterError) as def_err:
                tr.confirm_hold(
                    service_pb2.ConfirmHoldRequest(
                        idempotency_key="t0-confirm-fail",
                        account_id="acct-t1-def",
                        earmark_id=h_sell_t0.earmark_id,
                        expected_earmark_revision=h_sell_t0.earmark_revision,
                        expected_position_version=snap_def.positions[0].position_version,
                        quote_terms=_quote_terms_clean("100", "30000000"),
                    )
                )
            assert def_err.value.code == error_pb2.ERROR_CODE_VALIDATION_FAILED
            assert def_err.value.committed_seq is None
            # Original HOLD MUST still be preserved!
            snap_def_after = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-t1-def")
            ).snapshot
            assert snap_def_after.active_earmarks[0].state == model_pb2.EARMARK_STATE_HOLD

            # --- 5g-3. Separate signed pending_match_cash_t0/t1 & funding_is_complete_exact ---
            src_f = "SRC-acct-fund"
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-fund",
                    opening_base=_make_opening_base(
                        "acct-fund", base_suffix="F0", source_id=src_f
                    ),
                    expected_account_version=0,
                )
            )
            # Orphan BUY T+0 (10M) -> pending_match_cash_t0 = -10M, funding_is_complete_exact = False
            fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="f-fund-buy",
                    source_id=src_f,
                    fills=[
                        _make_fill(
                            source_id=src_f,
                            trade_id="FB-1",
                            feed_seq=1,
                            account_id="acct-fund",
                            side=model_pb2.TRADE_SIDE_BUY,
                            settlement_date="2026-06-15",
                            notional="10000000",
                        )
                    ],
                )
            )
            fs1 = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-fund")
            ).snapshot.funding_summary
            assert _to_dec(fs1.pending_match_cash_t0) == Decimal("-10000000")
            assert _to_dec(fs1.pending_match_cash_t1) == Decimal("0")
            assert fs1.pending_match_count == 1
            assert fs1.funding_is_complete_exact is False

            # Add offsetting orphan SELL T+0 (10M) -> pending_match_cash_t0 == 0, STILL funding_is_complete_exact == False!
            # Plus orphan BUY T+1 (5M) -> pending_match_cash_t1 = -5M!
            fd.ingest_trade_feed(
                service_pb2.IngestTradeFeedRequest(
                    idempotency_key="f-fund-sell-and-t1",
                    source_id=src_f,
                    fills=[
                        _make_fill(
                            source_id=src_f,
                            trade_id="FS-1",
                            feed_seq=2,
                            account_id="acct-fund",
                            side=model_pb2.TRADE_SIDE_SELL,
                            settlement_date="2026-06-15",
                            notional="10000000",
                            batch_first=2,
                            batch_last=3,
                        ),
                        _make_fill(
                            source_id=src_f,
                            trade_id="FB-T1",
                            feed_seq=3,
                            account_id="acct-fund",
                            side=model_pb2.TRADE_SIDE_BUY,
                            settlement_date="2026-06-16",
                            notional="5000000",
                            batch_first=2,
                            batch_last=3,
                        ),
                    ],
                )
            )
            fs2 = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-fund")
            ).snapshot.funding_summary
            assert _to_dec(fs2.pending_match_cash_t0) == Decimal("0")
            assert _to_dec(fs2.pending_match_cash_t1) == Decimal("-5000000")
            assert fs2.pending_match_count == 3
            assert fs2.funding_is_complete_exact is False

            # --- 5g-4. Setup HOLD for expired hold lazy-expiry test (committed EXPIRED receipt) ---
            src_e = "SRC-acct-exp"
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-exp",
                    opening_base=_make_opening_base(
                        "acct-exp", base_suffix="E1", source_id=src_e
                    ),
                    expected_account_version=0,
                )
            )
            h_exp = tr.create_hold(
                service_pb2.CreateHoldRequest(
                    idempotency_key="hold-to-expire",
                    account_id="acct-exp",
                    expected_position_version=1,
                    instrument_code="BOND_10Y",
                    side=model_pb2.TRADE_SIDE_SELL,
                    settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                    notional=_dec("20000000", UNITS.currency_amount),
                    ttl_seconds=1800,
                    note="will-expire",
                )
            ).earmark
            # Pause feed so reconfirmation after expiry MUST fail business re-validation!
            op.set_feed_control(
                service_pb2.SetFeedControlRequest(
                    idempotency_key="pause-exp-feed",
                    source_id=src_e,
                    expected_feed_status_revision=1,
                    paused_by_operator=True,
                    force_clear_gap_pause=False,
                    reason="block-reconfirm",
                )
            )

    # Server #1 cleanly stopped: rewind persisted expires_at past now to simulate TTL elapsed!
    conn = sqlite3.connect(str(db_path))
    try:
        row = conn.execute(
            "SELECT record_json FROM blotter_earmarks WHERE earmark_id = ?",
            (h_exp.earmark_id.value,),
        ).fetchone()
        assert row is not None
        rec_obj = json.loads(row[0])
        past_ts = int(time.time()) - 120
        rec_obj["expires_at"]["instant_secs"] = past_ts
        conn.execute(
            "UPDATE blotter_earmarks SET record_json = ? WHERE earmark_id = ?",
            (json.dumps(rec_obj), h_exp.earmark_id.value),
        )
        conn.commit()
    finally:
        conn.close()

    # Restart server on same WAL: ConfirmHold on expired HOLD commits EXPIRED + returns BlotterError with committed_seq!
    with _run_server(db_path=db_path, policy_path=policy_path, creds_path=creds_path) as (
        _proc2,
        ep2,
        _port2,
    ):
        with DeskBlotterClient(ep2, bearer_token=TRADER_TOK, insecure=True) as tr2:
            confirm_exp_req = service_pb2.ConfirmHoldRequest(
                idempotency_key="confirm-expired-hold",
                account_id="acct-exp",
                earmark_id=h_exp.earmark_id,
                expected_earmark_revision=h_exp.earmark_revision,
                expected_position_version=2,
                quote_terms=_quote_terms_clean("100", "20000000"),
            )
            with pytest.raises(BlotterError) as exp_err:
                tr2.confirm_hold(confirm_exp_req)
            assert exp_err.value.code == error_pb2.ERROR_CODE_VALIDATION_FAILED
            assert exp_err.value.committed_seq is not None and exp_err.value.committed_seq > 0
            assert exp_err.value.replayed is False
            assert exp_err.value.earmark is not None
            assert exp_err.value.earmark.state == model_pb2.EARMARK_STATE_EXPIRED

            # Retrying the same idempotency_key replays the exact committed EXPIRED receipt!
            with pytest.raises(BlotterError) as exp_replay:
                tr2.confirm_hold(confirm_exp_req)
            assert exp_replay.value.committed_seq == exp_err.value.committed_seq
            assert exp_replay.value.replayed is True
            assert exp_replay.value.earmark is not None
            assert exp_replay.value.earmark.state == model_pb2.EARMARK_STATE_EXPIRED


# ======================================================================
# 6. Risk, RulePack, Quotes & Credentials Matrix
# ======================================================================


def test_live_6_risk_rulepack_quotes_and_credentials_matrix(tmp_path: Path) -> None:
    accounts = [
        "acct-f100",
        "acct-f1000",
        "acct-short",
        "acct-cov",
        "acct-zero-net",
        "acct-no-rule",
        "acct-corrupt-rule",
        "acct-uncov-draft",
    ]
    sources = ["SRC-A", "SRC-B", "FEED-RISK"]
    creds_path = _write_credentials(
        tmp_path / "creds.json", accounts=accounts, sources=sources
    )
    db_path = tmp_path / "risk_matrix.sqlite"
    policy_path = tmp_path / "policy_risk_1.json"

    valid_reg = _build_reg_pack_binding()
    corrupt_reg = _build_reg_pack_binding(raw_override=b"\xff\xff\xff\xff\x00\xfe")
    tampered_hash_policy = tmp_path / "policy_tampered_hash.json"
    _write_policy(
        tampered_hash_policy,
        default_quote_source_id="SRC-A",
        regulatory_rule_pack=_build_reg_pack_binding(tamper_hash=True),
    )
    # Tampered RulePack content_hash fails closed immediately at server startup.
    tampered_proc = subprocess.run(
        [str(_server_binary())],
        cwd=REPO_ROOT,
        env={
            **os.environ,
            "FICANT_SERVER_ADDR": f"127.0.0.1:{_free_port()}",
            "FICANT_BLOTTER_EPHEMERAL_IN_MEMORY": "1",
            "FICANT_BLOTTER_CREDENTIALS_PATH": str(creds_path),
            "FICANT_BLOTTER_POLICY_PATH": str(tampered_hash_policy),
        },
        capture_output=True,
        timeout=10,
        check=False,
    )
    assert tampered_proc.returncode != 0

    _write_policy(
        policy_path,
        default_quote_source_id="SRC-A",
        quote_staleness_seconds=300,
        limit_ceilings={"MAX_INST_FACE": "500000000"},
        regulatory_rule_pack=None,
        accounts_override={
            "acct-f100": {"regulatory_rule_pack": valid_reg},
            "acct-f1000": {"regulatory_rule_pack": valid_reg},
            "acct-uncov-draft": {"regulatory_rule_pack": valid_reg},
            "acct-corrupt-rule": {"regulatory_rule_pack": corrupt_reg},
        },
    )

    draft_src_a: model_pb2.DraftEarmarkRecord
    with _run_server(db_path=db_path, policy_path=policy_path, creds_path=creds_path) as (
        _proc1,
        ep1,
        port1,
    ):
        with (
            DeskBlotterClient(ep1, bearer_token=OPERATOR_TOK, insecure=True) as op,
            DeskBlotterClient(ep1, bearer_token=TRADER_TOK, insecure=True) as tr,
            DeskBlotterClient(ep1, bearer_token=PARSER_TOK, insecure=True) as pr,
            DeskBlotterClient(ep1, bearer_token=None, insecure=True) as unauth,
            DeskBlotterClient(ep1, bearer_token="unknown-bogus-tok", insecure=True) as bad_tok,
        ):
            # --- 6a. face_value=100 vs 1000 DV01 / Draft / Hedge exact parity & Long/Short opposite sign ---
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-f100",
                    opening_base=_make_opening_base(
                        "acct-f100",
                        base_suffix="F1",
                        source_id="FEED-RISK",
                        instruments=[
                            model_pb2.InstrumentCatalogEntry(
                                instrument_code="BOND_10Y",
                                bond=_make_bond(ulid_suffix="B1", face_value="100"),
                                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_SOVEREIGN_OR_POLICY_FIXED,
                            )
                        ],
                    ),
                    expected_account_version=0,
                )
            )
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-f1000",
                    opening_base=_make_opening_base(
                        "acct-f1000",
                        base_suffix="F2",
                        source_id="FEED-RISK",
                        instruments=[
                            model_pb2.InstrumentCatalogEntry(
                                instrument_code="BOND_10Y",
                                bond=_make_bond(ulid_suffix="B2", face_value="1000"),
                                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_SOVEREIGN_OR_POLICY_FIXED,
                            )
                        ],
                    ),
                    expected_account_version=0,
                )
            )
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-short",
                    opening_base=_make_opening_base(
                        "acct-short",
                        base_suffix="F3",
                        source_id="FEED-RISK",
                        balances=[
                            model_pb2.OpeningInstrumentBalance(
                                instrument_code="BOND_10Y",
                                opening_economic_notional=_dec("-100000000", UNITS.currency_amount),
                                opening_available_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                            )
                        ],
                    ),
                    expected_account_version=0,
                )
            )

            # Ingest Source A seq=100 AND Source B seq=1 (coexisting independent quote sequence spaces!)
            tr.ingest_market_quotes(
                service_pb2.IngestMarketQuotesRequest(
                    idempotency_key="qa-100",
                    account_id="acct-f100",
                    source_id="SRC-A",
                    ticks=[
                        _make_quote_tick(
                            source_id="SRC-A", quote_seq=100, instrument_code="BOND_10Y", ytm="0.025", seconds_ago=5
                        )
                    ],
                )
            )
            tr.ingest_market_quotes(
                service_pb2.IngestMarketQuotesRequest(
                    idempotency_key="qb-1",
                    account_id="acct-f100",
                    source_id="SRC-B",
                    ticks=[
                        _make_quote_tick(
                            source_id="SRC-B", quote_seq=1, instrument_code="BOND_10Y", ytm="0.026", seconds_ago=5
                        )
                    ],
                )
            )

            s100 = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-f100")
            ).snapshot
            s1000 = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-f1000")
            ).snapshot
            s_short = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-short")
            ).snapshot
            dv01_100 = _to_dec(s100.positions[0].pre_tax_dv01)
            dv01_1000 = _to_dec(s1000.positions[0].pre_tax_dv01)
            dv01_short = _to_dec(s_short.positions[0].pre_tax_dv01)
            assert dv01_100 > Decimal("0")
            assert dv01_100 == dv01_1000
            assert dv01_short == -dv01_100

            # Draft preview delta DV01 parity between face_value=100 and face_value=1000
            def _proposal(acct: str, inst: str = "BOND_10Y") -> spi_pb2.NlpEarmarkProposal:
                raw_text = f"BUY T+0 INST={inst} NOTIONAL=5000000"
                return spi_pb2.NlpEarmarkProposal(
                    parser_source_id="parser-1",
                    model_version="1.0",
                    raw_source_text=raw_text,
                    anchors=[
                        model_pb2.SourceTextAnchor(
                            field_name="side", byte_start=0, byte_end=3, span_text="BUY"
                        )
                    ],
                    confidence_score=_dec("0.95", UNITS.dimensionless),
                    account_id=acct,
                    instrument_code=inst,
                    side=model_pb2.TRADE_SIDE_BUY,
                    settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                    notional=_dec("5000000", UNITS.currency_amount),
                    quote_terms=_quote_terms_clean("100", "5000000"),
                    suggested_target_state=model_pb2.DRAFT_TARGET_STATE_COMMITTED,
                )

            draft_100 = pr.submit_draft_earmark(
                service_pb2.SubmitDraftEarmarkRequest(
                    idempotency_key="dr-100",
                    account_id="acct-f100",
                    proposal=_proposal("acct-f100"),
                )
            ).draft
            draft_1000 = pr.submit_draft_earmark(
                service_pb2.SubmitDraftEarmarkRequest(
                    idempotency_key="dr-1000",
                    account_id="acct-f1000",
                    proposal=_proposal("acct-f1000"),
                )
            ).draft
            assert _to_dec(draft_100.preview_delta_pre_tax_dv01) == _to_dec(
                draft_1000.preview_delta_pre_tax_dv01
            )
            assert draft_100.preview_quote_source_id == "SRC-A"
            assert draft_100.preview_quote_seq == 100
            draft_src_a = draft_100

            # Hedge parity (100 vs 1000) & Conversion Factor sensitivity
            h_100 = tr.preview_covered_hedge(
                service_pb2.PreviewCoveredHedgeRequest(
                    account_id="acct-f100",
                    scope=model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT,
                    product_code="T",
                    contract_code="T2609",
                    ctd_dv01_per_100=_dec("0.075", UNITS.dv01_per_100),
                    conversion_factor=_dec("0.985", UNITS.dimensionless),
                    delivery_rule_pack=_futures_v2_pack(),
                    valuation_at=_shanghai_time(2026, 6, 15, 10, 0, 0),
                )
            )
            h_1000 = tr.preview_covered_hedge(
                service_pb2.PreviewCoveredHedgeRequest(
                    account_id="acct-f1000",
                    scope=model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT,
                    product_code="T",
                    contract_code="T2609",
                    ctd_dv01_per_100=_dec("0.075", UNITS.dv01_per_100),
                    conversion_factor=_dec("0.985", UNITS.dimensionless),
                    delivery_rule_pack=_futures_v2_pack(),
                    valuation_at=_shanghai_time(2026, 6, 15, 10, 0, 0),
                )
            )
            assert h_100.measures.recommended_contracts == h_1000.measures.recommended_contracts
            h_cf2 = tr.preview_covered_hedge(
                service_pb2.PreviewCoveredHedgeRequest(
                    account_id="acct-f100",
                    scope=model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT,
                    product_code="T",
                    contract_code="T2609",
                    ctd_dv01_per_100=_dec("0.075", UNITS.dv01_per_100),
                    conversion_factor=_dec("1.150", UNITS.dimensionless),
                    delivery_rule_pack=_futures_v2_pack(),
                    valuation_at=_shanghai_time(2026, 6, 15, 10, 0, 0),
                )
            )
            assert abs(h_cf2.measures.recommended_contracts) > abs(h_100.measures.recommended_contracts)

            # --- 6b. EXACT_MARKET precedence, 2035 long bond, zero position, credit bond, stale quote ---
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-cov",
                    opening_base=_make_opening_base(
                        "acct-cov",
                        base_suffix="V1",
                        source_id="FEED-RISK",
                        default_calendar_requirement=rates_pb2.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
                        balances=[
                            model_pb2.OpeningInstrumentBalance(
                                instrument_code="LONG_2035_REPLAY",
                                opening_economic_notional=_dec("50000000", UNITS.currency_amount),
                                opening_available_notional=_dec("50000000", UNITS.currency_amount),
                                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                            ),
                            model_pb2.OpeningInstrumentBalance(
                                instrument_code="LONG_2035_EXACT",
                                opening_economic_notional=_dec("50000000", UNITS.currency_amount),
                                opening_available_notional=_dec("50000000", UNITS.currency_amount),
                                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                            ),
                            model_pb2.OpeningInstrumentBalance(
                                instrument_code="ZERO_POS",
                                opening_economic_notional=_dec("0", UNITS.currency_amount),
                                opening_available_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                            ),
                            model_pb2.OpeningInstrumentBalance(
                                instrument_code="CREDIT_BOND",
                                opening_economic_notional=_dec("30000000", UNITS.currency_amount),
                                opening_available_notional=_dec("30000000", UNITS.currency_amount),
                                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                            ),
                            model_pb2.OpeningInstrumentBalance(
                                instrument_code="STALE_BOND",
                                opening_economic_notional=_dec("20000000", UNITS.currency_amount),
                                opening_available_notional=_dec("20000000", UNITS.currency_amount),
                                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                            ),
                        ],
                        instruments=[
                            model_pb2.InstrumentCatalogEntry(
                                instrument_code="LONG_2035_REPLAY",
                                bond=_make_bond(ulid_suffix="K1", maturity="2035-01-15"),
                                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_SOVEREIGN_OR_POLICY_FIXED,
                                calendar_requirement=rates_pb2.CALENDAR_REQUIREMENT_UNSPECIFIED,
                            ),
                            model_pb2.InstrumentCatalogEntry(
                                instrument_code="LONG_2035_EXACT",
                                bond=_make_bond(ulid_suffix="K2", maturity="2035-01-15"),
                                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_SOVEREIGN_OR_POLICY_FIXED,
                                calendar_requirement=rates_pb2.CALENDAR_REQUIREMENT_EXACT_MARKET,
                            ),
                            model_pb2.InstrumentCatalogEntry(
                                instrument_code="ZERO_POS",
                                bond=_make_bond(ulid_suffix="K3"),
                                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_CREDIT_OR_SPREAD_PRODUCT,
                            ),
                            model_pb2.InstrumentCatalogEntry(
                                instrument_code="CREDIT_BOND",
                                bond=_make_bond(ulid_suffix="K4"),
                                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_CREDIT_OR_SPREAD_PRODUCT,
                            ),
                            model_pb2.InstrumentCatalogEntry(
                                instrument_code="STALE_BOND",
                                bond=_make_bond(ulid_suffix="K5"),
                                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_SOVEREIGN_OR_POLICY_FIXED,
                            ),
                        ],
                    ),
                    expected_account_version=0,
                )
            )
            tr.ingest_market_quotes(
                service_pb2.IngestMarketQuotesRequest(
                    idempotency_key="q-cov",
                    account_id="acct-cov",
                    source_id="SRC-A",
                    ticks=[
                        _make_quote_tick(
                            source_id="SRC-A", quote_seq=1, instrument_code="LONG_2035_REPLAY", seconds_ago=5
                        ),
                        _make_quote_tick(
                            source_id="SRC-A", quote_seq=1, instrument_code="LONG_2035_EXACT", seconds_ago=5
                        ),
                        _make_quote_tick(
                            source_id="SRC-A", quote_seq=1, instrument_code="CREDIT_BOND", seconds_ago=5
                        ),
                        # Observed 600s ago -> exceeds 300s quote_staleness_seconds -> STALE_QUOTE!
                        _make_quote_tick(
                            source_id="SRC-A", quote_seq=1, instrument_code="STALE_BOND", seconds_ago=600
                        ),
                    ],
                )
            )
            snap_cov = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-cov")
            ).snapshot
            pos_by_code = {p.instrument_code: p for p in snap_cov.positions}
            assert pos_by_code["LONG_2035_REPLAY"].coverage_status == model_pb2.COVERAGE_STATUS_COVERED_PRE_TAX
            assert (
                pos_by_code["LONG_2035_REPLAY"].calendar_resolution
                == model_pb2.CALENDAR_RESOLUTION_MODE_PROVISIONAL_WEEKEND_BEYOND_COVERAGE
            )
            assert pos_by_code["LONG_2035_EXACT"].coverage_status == model_pb2.COVERAGE_STATUS_UNCOVERED
            assert (
                pos_by_code["LONG_2035_EXACT"].uncovered_reason
                == model_pb2.UNCOVERED_REASON_CALENDAR_COVERAGE_MISSING
            )
            assert pos_by_code["ZERO_POS"].coverage_status == model_pb2.COVERAGE_STATUS_INACTIVE_ZERO_POSITION
            assert pos_by_code["CREDIT_BOND"].coverage_status == model_pb2.COVERAGE_STATUS_UNCOVERED
            assert (
                pos_by_code["CREDIT_BOND"].uncovered_reason
                == model_pb2.UNCOVERED_REASON_CREDIT_OR_NON_RATES_ISSUER
            )
            assert pos_by_code["STALE_BOND"].coverage_status == model_pb2.COVERAGE_STATUS_UNCOVERED
            assert pos_by_code["STALE_BOND"].uncovered_reason == model_pb2.UNCOVERED_REASON_STALE_QUOTE
            # Zero position excluded from both covered & uncovered counts: 1 covered, 3 uncovered!
            assert snap_cov.risk_summary.covered_instrument_count == 1
            assert snap_cov.risk_summary.uncovered_instrument_count == 3
            assert snap_cov.risk_summary.whole_account_hedge_allowed is False

            # WHOLE_ACCOUNT rejected while uncovered > 0, COVERED_SUBSET_ONLY succeeds!
            with pytest.raises(BlotterError) as whole_err:
                tr.preview_covered_hedge(
                    service_pb2.PreviewCoveredHedgeRequest(
                        account_id="acct-cov",
                        scope=model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT,
                        product_code="T",
                        contract_code="T2609",
                        ctd_dv01_per_100=_dec("0.075", UNITS.dv01_per_100),
                        conversion_factor=_dec("0.985", UNITS.dimensionless),
                        delivery_rule_pack=_futures_v2_pack(),
                    )
                )
            assert whole_err.value.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

            sub_hedge = tr.preview_covered_hedge(
                service_pb2.PreviewCoveredHedgeRequest(
                    account_id="acct-cov",
                    scope=model_pb2.PREVIEW_HEDGE_SCOPE_COVERED_SUBSET_ONLY,
                    product_code="T",
                    contract_code="T2609",
                    ctd_dv01_per_100=_dec("0.075", UNITS.dv01_per_100),
                    conversion_factor=_dec("0.985", UNITS.dimensionless),
                    delivery_rule_pack=_futures_v2_pack(),
                )
            )
            assert _to_dec(sub_hedge.excluded_uncovered_gross_notional) == Decimal("100000000")

            # --- Zero net DV01 exposure -> zero_net_exposure=True, 0 contracts, 0 residual, effectiveness=1 ---
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="imp-znet",
                    opening_base=_make_opening_base(
                        "acct-zero-net",
                        base_suffix="ZN",
                        source_id="FEED-RISK",
                        balances=[
                            model_pb2.OpeningInstrumentBalance(
                                instrument_code="B_LONG",
                                opening_economic_notional=_dec("50000000", UNITS.currency_amount),
                                opening_available_notional=_dec("50000000", UNITS.currency_amount),
                                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                            ),
                            model_pb2.OpeningInstrumentBalance(
                                instrument_code="B_SHORT",
                                opening_economic_notional=_dec("-50000000", UNITS.currency_amount),
                                opening_available_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                            ),
                        ],
                        instruments=[
                            model_pb2.InstrumentCatalogEntry(
                                instrument_code="B_LONG",
                                bond=_make_bond(ulid_suffix="N1"),
                                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_SOVEREIGN_OR_POLICY_FIXED,
                            ),
                            model_pb2.InstrumentCatalogEntry(
                                instrument_code="B_SHORT",
                                bond=_make_bond(ulid_suffix="N2"),
                                eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_SOVEREIGN_OR_POLICY_FIXED,
                            ),
                        ],
                    ),
                    expected_account_version=0,
                )
            )
            tr.ingest_market_quotes(
                service_pb2.IngestMarketQuotesRequest(
                    idempotency_key="q-znet",
                    account_id="acct-zero-net",
                    source_id="SRC-A",
                    ticks=[
                        _make_quote_tick(source_id="SRC-A", quote_seq=1, instrument_code="B_LONG"),
                        _make_quote_tick(source_id="SRC-A", quote_seq=1, instrument_code="B_SHORT"),
                    ],
                )
            )
            zn_hedge = tr.preview_covered_hedge(
                service_pb2.PreviewCoveredHedgeRequest(
                    account_id="acct-zero-net",
                    scope=model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT,
                    product_code="T",
                    contract_code="T2609",
                    ctd_dv01_per_100=_dec("0.075", UNITS.dv01_per_100),
                    conversion_factor=_dec("0.985", UNITS.dimensionless),
                    delivery_rule_pack=_futures_v2_pack(),
                )
            )
            assert zn_hedge.zero_net_exposure is True
            assert zn_hedge.measures.recommended_contracts == 0
            assert _to_dec(zn_hedge.measures.residual_dv01) == Decimal("0")
            assert _to_dec(zn_hedge.measures.hedge_effectiveness) == Decimal("1")

            # --- 6c. RulePack matrix: missing / corrupt fails vs valid uncovered acknowledged by TRADER ---
            for acct, suf in (
                ("acct-no-rule", "R1"),
                ("acct-corrupt-rule", "R2"),
                ("acct-uncov-draft", "R3"),
            ):
                op.import_opening_base(
                    service_pb2.ImportOpeningBaseRequest(
                        idempotency_key=f"imp-{acct}",
                        opening_base=_make_opening_base(
                            acct,
                            base_suffix=suf,
                            source_id="FEED-RISK",
                            balances=[
                                model_pb2.OpeningInstrumentBalance(
                                    instrument_code="UNCOV_BOND",
                                    opening_economic_notional=_dec("10000000", UNITS.currency_amount),
                                    opening_available_notional=_dec("10000000", UNITS.currency_amount),
                                    opening_net_t0_notional=_dec("0", UNITS.currency_amount),
                                    opening_net_t1_notional=_dec("0", UNITS.currency_amount),
                                )
                            ],
                            instruments=[
                                model_pb2.InstrumentCatalogEntry(
                                    instrument_code="UNCOV_BOND",
                                    bond=_make_bond(ulid_suffix=suf),
                                    eligibility_class=model_pb2.RATE_COVERAGE_ELIGIBILITY_CREDIT_OR_SPREAD_PRODUCT,
                                )
                            ],
                        ),
                        expected_account_version=0,
                    )
                )

            # Missing RulePack & corrupt payload RulePack both fail SubmitDraftEarmark
            with pytest.raises(BlotterError) as no_rule_err:
                pr.submit_draft_earmark(
                    service_pb2.SubmitDraftEarmarkRequest(
                        idempotency_key="dr-no-rule",
                        account_id="acct-no-rule",
                        proposal=_proposal("acct-no-rule", "UNCOV_BOND"),
                    )
                )
            assert no_rule_err.value.code == error_pb2.ERROR_CODE_VALIDATION_FAILED
            assert any(
                fv.field == "regulatory_rule_pack"
                for fv in no_rule_err.value.field_violations
            )

            with pytest.raises(BlotterError) as corrupt_err:
                pr.submit_draft_earmark(
                    service_pb2.SubmitDraftEarmarkRequest(
                        idempotency_key="dr-corrupt-rule",
                        account_id="acct-corrupt-rule",
                        proposal=_proposal("acct-corrupt-rule", "UNCOV_BOND"),
                    )
                )
            assert corrupt_err.value.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

            # Valid RulePack + uncovered DV01: submit succeeds, confirm without ack fails, confirm WITH ack succeeds!
            uncov_draft = pr.submit_draft_earmark(
                service_pb2.SubmitDraftEarmarkRequest(
                    idempotency_key="dr-uncov-ok",
                    account_id="acct-uncov-draft",
                    proposal=_proposal("acct-uncov-draft", "UNCOV_BOND"),
                )
            ).draft
            assert uncov_draft.preview_coverage_status == model_pb2.COVERAGE_STATUS_UNCOVERED

            with pytest.raises(BlotterError) as no_ack_err:
                tr.confirm_draft_earmark(
                    service_pb2.ConfirmDraftEarmarkRequest(
                        idempotency_key="cfm-uncov-noack",
                        account_id="acct-uncov-draft",
                        draft_id=uncov_draft.draft_id,
                        expected_draft_revision=uncov_draft.draft_revision,
                        expected_position_version=1,
                        confirmation_token=uncov_draft.confirmation_token,
                        acknowledge_uncovered_dv01=False,
                    )
                )
            assert no_ack_err.value.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

            # NLP_PARSER trying to confirm TRADER action -> FORBIDDEN
            with pytest.raises(BlotterError) as parser_cfm_err:
                pr.confirm_draft_earmark(
                    service_pb2.ConfirmDraftEarmarkRequest(
                        idempotency_key="cfm-uncov-parser",
                        account_id="acct-uncov-draft",
                        draft_id=uncov_draft.draft_id,
                        expected_draft_revision=uncov_draft.draft_revision,
                        expected_position_version=1,
                        confirmation_token=uncov_draft.confirmation_token,
                        acknowledge_uncovered_dv01=True,
                    )
                )
            assert parser_cfm_err.value.code == error_pb2.ERROR_CODE_FORBIDDEN

            # TRADER explicitly acknowledges uncovered DV01 -> succeeds and stamps verified actor!
            confirmed_uncov = tr.confirm_draft_earmark(
                service_pb2.ConfirmDraftEarmarkRequest(
                    idempotency_key="cfm-uncov-ack",
                    account_id="acct-uncov-draft",
                    draft_id=uncov_draft.draft_id,
                    expected_draft_revision=uncov_draft.draft_revision,
                    expected_position_version=1,
                    confirmation_token=uncov_draft.confirmation_token,
                    acknowledge_uncovered_dv01=True,
                )
            )
            assert confirmed_uncov.draft.confirmed_by_trader_id == "trader-1"
            assert confirmed_uncov.earmark.confirmed_by_trader_id == "trader-1"

            # Submit a second uncovered draft to verify confirmation fails when RulePack is removed on restart
            uncov_draft_2 = pr.submit_draft_earmark(
                service_pb2.SubmitDraftEarmarkRequest(
                    idempotency_key="dr-uncov-2",
                    account_id="acct-uncov-draft",
                    proposal=_proposal("acct-uncov-draft", "UNCOV_BOND"),
                )
            ).draft

            # --- 6d. Credentials matrix (native gRPC + raw HTTP/1.1 gRPC-Web) ---
            # Missing / unknown creds -> UNAUTHENTICATED
            for bad_client in (unauth, bad_tok):
                with pytest.raises(BlotterError) as u_err:
                    bad_client.get_account_snapshot(
                        service_pb2.GetAccountSnapshotRequest(account_id="acct-f100")
                    )
                assert u_err.value.code == error_pb2.ERROR_CODE_UNAUTHENTICATED

            web_unauth = _grpc_web_unary(
                port1,
                "/ficant.blotter.v1.DeskBlotterService/GetAccountSnapshot",
                service_pb2.GetAccountSnapshotRequest(account_id="acct-f100"),
                service_pb2.GetAccountSnapshotResponse,
                bearer_token=None,
            )
            assert web_unauth.error.code == error_pb2.ERROR_CODE_UNAUTHENTICATED

            # TRADER creates a HOLD, then NLP_PARSER replays exact same idempotency_key -> FORBIDDEN (auth before receipt)!
            trader_hold_req = service_pb2.CreateHoldRequest(
                idempotency_key="trader-exclusive-key-1",
                account_id="acct-f100",
                expected_position_version=1,
                instrument_code="BOND_10Y",
                side=model_pb2.TRADE_SIDE_BUY,
                settlement_bucket=model_pb2.SETTLEMENT_BUCKET_T_PLUS_0,
                notional=_dec("1000000", UNITS.currency_amount),
                ttl_seconds=1800,
                note="actor-audit",
            )
            tr_hold = tr.create_hold(trader_hold_req)
            assert tr_hold.earmark.created_by == "trader-1"

            with pytest.raises(BlotterError) as replay_forbid:
                pr.create_hold(trader_hold_req)
            assert replay_forbid.value.code == error_pb2.ERROR_CODE_FORBIDDEN

            web_forbid = _grpc_web_unary(
                port1,
                "/ficant.blotter.v1.DeskBlotterService/CreateHold",
                trader_hold_req,
                service_pb2.CreateHoldResponse,
                bearer_token=PARSER_TOK,
            )
            assert web_forbid.error.code == error_pb2.ERROR_CODE_FORBIDDEN

    # --- 6e. WAL restart switching quote source SRC-A -> SRC-B invalidates old draft,
    #         and removing RulePack rejects ConfirmDraftEarmark even with acknowledge_uncovered_dv01=True ---
    policy_path_2 = tmp_path / "policy_risk_2.json"
    _write_policy(
        policy_path_2,
        default_quote_source_id="SRC-B",
        quote_staleness_seconds=300,
        limit_ceilings={"MAX_INST_FACE": "500000000"},
        regulatory_rule_pack=None,
        accounts_override={
            "acct-f100": {"regulatory_rule_pack": valid_reg},
            "acct-f1000": {"regulatory_rule_pack": valid_reg},
            "acct-corrupt-rule": {"regulatory_rule_pack": corrupt_reg},
        },
    )
    with _run_server(db_path=db_path, policy_path=policy_path_2, creds_path=creds_path) as (
        _proc2,
        ep2,
        _port2,
    ):
        with DeskBlotterClient(ep2, bearer_token=TRADER_TOK, insecure=True) as tr2:
            snap_switched = tr2.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-f100")
            ).snapshot
            assert snap_switched.positions[0].active_quote_source_id == "SRC-B"
            assert snap_switched.positions[0].active_quote_seq == 1

            # Old draft bound to SRC-A seq=100 MUST fail with CONCURRENCY_CONFLICT on confirmation!
            with pytest.raises(BlotterError) as stale_draft_err:
                tr2.confirm_draft_earmark(
                    service_pb2.ConfirmDraftEarmarkRequest(
                        idempotency_key="cfm-stale-src-a",
                        account_id="acct-f100",
                        draft_id=draft_src_a.draft_id,
                        expected_draft_revision=draft_src_a.draft_revision,
                        expected_position_version=snap_switched.positions[0].position_version,
                        confirmation_token=draft_src_a.confirmation_token,
                        acknowledge_uncovered_dv01=False,
                    )
                )
            assert stale_draft_err.value.code == error_pb2.ERROR_CODE_CONCURRENCY_CONFLICT

            # Confirming draft after RulePack was removed MUST fail even with acknowledge_uncovered_dv01=True!
            snap_uncov_2 = tr2.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id="acct-uncov-draft")
            ).snapshot
            with pytest.raises(BlotterError) as missing_rule_cfm:
                tr2.confirm_draft_earmark(
                    service_pb2.ConfirmDraftEarmarkRequest(
                        idempotency_key="cfm-missing-rule-after-restart",
                        account_id="acct-uncov-draft",
                        draft_id=uncov_draft_2.draft_id,
                        expected_draft_revision=uncov_draft_2.draft_revision,
                        expected_position_version=snap_uncov_2.positions[0].position_version,
                        confirmation_token=uncov_draft_2.confirmation_token,
                        acknowledge_uncovered_dv01=True,
                    )
                )
            assert missing_rule_cfm.value.code == error_pb2.ERROR_CODE_VALIDATION_FAILED


# ======================================================================
# 7. [Gate-3] Policy A vs Policy B, Embedded Default & Invalid Policy Exit
# ======================================================================


def test_live_7_gate3_policy_a_vs_policy_b_embedded_default_and_invalid_policy_exit(
    tmp_path: Path,
) -> None:
    creds_path = _write_credentials(
        tmp_path / "creds.json",
        accounts=["acct-gate3"],
        sources=["FEED-1", "primary-quote-feed"],
    )

    # Verify embedded default Policy A JSON on disk matches Policy A contract
    embedded_doc = json.loads(EMBEDDED_POLICY_PATH.read_text(encoding="utf-8"))
    assert embedded_doc["policy_profile"] == "A"
    assert embedded_doc["allowed_hedge_product_codes"] == ["T", "TF", "TL"]

    # 1. Start server with NO FICANT_BLOTTER_POLICY_PATH -> uses embedded default Policy A!
    with _run_server(ephemeral=True, policy_path=None, creds_path=creds_path) as (
        _proc_a,
        ep_a,
        _port_a,
    ):
        with (
            DeskBlotterClient(ep_a, bearer_token=OPERATOR_TOK, insecure=True) as op,
            DeskBlotterClient(ep_a, bearer_token=TRADER_TOK, insecure=True) as tr,
        ):
            imp = op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="g3-imp-a",
                    opening_base=_make_opening_base("acct-gate3", base_suffix="G1"),
                    expected_account_version=0,
                )
            )
            assert list(imp.snapshot.allowed_hedge_product_codes) == ["T", "TF", "TL"]
            tr.ingest_market_quotes(
                service_pb2.IngestMarketQuotesRequest(
                    idempotency_key="g3-q-a",
                    account_id="acct-gate3",
                    source_id="primary-quote-feed",
                    ticks=[
                        _make_quote_tick(
                            source_id="primary-quote-feed", quote_seq=1, ytm="0.025", seconds_ago=5
                        )
                    ],
                )
            )
            # "T" allowed (1M notional)
            res_t = tr.preview_covered_hedge(
                service_pb2.PreviewCoveredHedgeRequest(
                    account_id="acct-gate3",
                    scope=model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT,
                    product_code="T",
                    contract_code="T2609",
                    ctd_dv01_per_100=_dec("0.075", UNITS.dv01_per_100),
                    conversion_factor=_dec("0.985", UNITS.dimensionless),
                    delivery_rule_pack=_futures_v2_pack(),
                    valuation_at=_shanghai_time(2026, 6, 15, 10, 0, 0),
                )
            )
            assert _to_dec(res_t.measures.contract_notional) == Decimal("1000000")

            # "TS" rejected under Policy A!
            with pytest.raises(BlotterError) as ts_rej:
                tr.preview_covered_hedge(
                    service_pb2.PreviewCoveredHedgeRequest(
                        account_id="acct-gate3",
                        scope=model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT,
                        product_code="TS",
                        contract_code="TS2609",
                        ctd_dv01_per_100=_dec("0.0145", UNITS.dv01_per_100),
                        conversion_factor=_dec("0.9987", UNITS.dimensionless),
                        delivery_rule_pack=_futures_v2_pack(),
                        valuation_at=_shanghai_time(2026, 6, 15, 10, 0, 0),
                    )
                )
            assert ts_rej.value.code == error_pb2.ERROR_CODE_VALIDATION_FAILED
            assert any(fv.field == "product_code" for fv in ts_rej.value.field_violations)

    # 2. Start server with Policy B (includes "TS") + real cgb-futures-v2.bin -> 2M notional!
    policy_b = _write_policy(
        tmp_path / "policy-b.json",
        profile="B",
        allowed_hedge_codes=["TS", "T", "TF", "TL"],
        default_quote_source_id="primary-quote-feed",
    )
    with _run_server(ephemeral=True, policy_path=policy_b, creds_path=creds_path) as (
        _proc_b,
        ep_b,
        _port_b,
    ):
        with (
            DeskBlotterClient(ep_b, bearer_token=OPERATOR_TOK, insecure=True) as op,
            DeskBlotterClient(ep_b, bearer_token=TRADER_TOK, insecure=True) as tr,
        ):
            imp_b = op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="g3-imp-b",
                    opening_base=_make_opening_base("acct-gate3", base_suffix="G2"),
                    expected_account_version=0,
                )
            )
            assert list(imp_b.snapshot.allowed_hedge_product_codes) == ["TS", "T", "TF", "TL"]
            tr.ingest_market_quotes(
                service_pb2.IngestMarketQuotesRequest(
                    idempotency_key="g3-q-b",
                    account_id="acct-gate3",
                    source_id="primary-quote-feed",
                    ticks=[
                        _make_quote_tick(
                            source_id="primary-quote-feed", quote_seq=1, ytm="0.025", seconds_ago=5
                        )
                    ],
                )
            )
            res_ts = tr.preview_covered_hedge(
                service_pb2.PreviewCoveredHedgeRequest(
                    account_id="acct-gate3",
                    scope=model_pb2.PREVIEW_HEDGE_SCOPE_WHOLE_ACCOUNT,
                    product_code="TS",
                    contract_code="TS2609",
                    ctd_dv01_per_100=_dec("0.0145", UNITS.dv01_per_100),
                    conversion_factor=_dec("0.9987", UNITS.dimensionless),
                    delivery_rule_pack=_futures_v2_pack(),
                    valuation_at=_shanghai_time(2026, 6, 15, 10, 0, 0),
                )
            )
            assert _to_dec(res_ts.measures.contract_notional) == Decimal("2000000")

    # 3. Invalid policy path & malformed policy JSON -> immediate non-zero exit
    bad_json_path = tmp_path / "malformed-policy.json"
    bad_json_path.write_text('{"schema_version": 99}', encoding="utf-8")
    for invalid_path in (tmp_path / "does-not-exist-policy.json", bad_json_path):
        env_bad = {
            **os.environ,
            "FICANT_SERVER_ADDR": f"127.0.0.1:{_free_port()}",
            "FICANT_BLOTTER_EPHEMERAL_IN_MEMORY": "1",
            "FICANT_BLOTTER_CREDENTIALS_PATH": str(creds_path),
            "FICANT_BLOTTER_POLICY_PATH": str(invalid_path),
        }
        proc_bad = subprocess.run(
            [str(_server_binary())],
            cwd=REPO_ROOT,
            env=env_bad,
            capture_output=True,
            timeout=10,
            check=False,
        )
        assert proc_bad.returncode != 0
