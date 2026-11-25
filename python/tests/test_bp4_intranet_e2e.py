"""Dual-mode BP-4 single-binary intranet & cross-day rollover E2E verification (AC-001, AC-003..AC-010)."""

from __future__ import annotations

from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from decimal import Decimal
import hashlib
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time
from typing import Any, Iterator

from google.protobuf.timestamp_pb2 import Timestamp
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
from ficant_sdk import (  # noqa: E402
    BlotterError,
    DeskBlotterClient,
    NlpDraftOnlyClient,
    decimal_to_pb,
    default_reference_units,
    parse_market_time,
)
from ficant_sdk.sidecar import (  # noqa: E402
    parse_token_nlp_intent,
    probe_healthz,
)

SHANGHAI_TZ = timezone(timedelta(hours=8))
REGULATORY_PACK_FIXTURE = (
    REPO_ROOT / "apps" / "desk-web" / "e2e" / "fixtures" / "regulatory-rule-pack.json"
)
UI_VERIFIER_SCRIPT = (
    REPO_ROOT / "apps" / "desk-web" / "scripts" / "verify-single-binary-ui.mjs"
)
DESK_WEB_DIST_DIR = REPO_ROOT / "apps" / "desk-web" / "dist"

PINNED_RULEPACK_BYTES = 159
PINNED_RULEPACK_SHA256 = (
    "8d8a0b944557cf87ed3d1f598ab3e02fa7f0aec125c1c26c17280acbd24367c5"
)

TRADER_TOKEN = "bp4-e2e-trader-secret-token-001"
OPERATOR_TOKEN = "bp4-e2e-operator-secret-token-002"
FEED_TOKEN = "bp4-e2e-feed-secret-token-003"
PARSER_TOKEN = "bp4-e2e-parser-secret-token-004"

ACCOUNT_ID = "acct-bp4"
QUOTE_SOURCE_ID = "SRC-BP4"
TRADE_SOURCE_ID = "FEED-BP4"
DAY1_DATE = "2026-06-15"
DAY1_NEXT_SETTLE = "2026-06-16"
DAY2_DATE = "2026-06-16"
DAY2_NEXT_SETTLE = "2026-06-17"

NLP_COMMITTED_VECTOR = (
    "买入 T+0 INST=BOND_10Y NOTIONAL=10000000 CLEAN=100.25 TARGET=COMMITTED"
)


def _ulid(suffix: str) -> common_pb2.Ulid:
    padded = suffix.upper().rjust(2, "0")[:2]
    return common_pb2.Ulid(value=f"01ARZ3NDEKTSV4RRFFQ69G5F{padded}")


def _units() -> rates_pb2.AnalysisUnits:
    base = default_reference_units()
    base.years.CopyFrom(common_pb2.UnitRef(unit_id=_ulid("Y1"), version=1))
    base.years_squared.CopyFrom(common_pb2.UnitRef(unit_id=_ulid("Y2"), version=1))
    return base


def _decimal_from_pb(value: common_pb2.DecimalValue) -> Decimal:
    return Decimal(value.coefficient).scaleb(-value.scale)


def _decimal_str_from_pb(value: common_pb2.DecimalValue) -> str:
    return format(_decimal_from_pb(value), "f")


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


def _resolve_server_mode() -> tuple[Path, bool, bool]:
    """Return (server_binary_path, is_isolated_embedded_mode, require_browser_e2e)."""
    env_bin = os.environ.get("FICANT_BP4_SERVER_BIN", "").strip()
    require_browser = os.environ.get("FICANT_BP4_REQUIRE_BROWSER_E2E", "").strip() == "1"

    if env_bin:
        bin_path = Path(env_bin).resolve()
        assert bin_path.is_file(), f"FICANT_BP4_SERVER_BIN does not exist: {bin_path}"
        # Prove clean zero-source / zero-dist runtime directory around the isolated binary.
        parent = bin_path.parent
        assert not (parent / "Cargo.toml").exists(), "isolated binary dir must not contain Cargo.toml"
        assert not (parent / "apps").exists(), "isolated binary dir must not contain apps/"
        assert not (parent / "dist").exists(), "isolated binary dir must not contain dist/"
        return bin_path, True, require_browser

    exe = "ficant-server.exe" if sys.platform == "win32" else "ficant-server"
    debug_bin = REPO_ROOT / "target" / "debug" / exe
    if not debug_bin.exists():
        subprocess.run(
            ["cargo", "build", "--locked", "--offline", "--bin", "ficant-server"],
            cwd=REPO_ROOT,
            check=True,
        )
    assert debug_bin.is_file(), f"expected debug ficant-server binary at {debug_bin}"
    return debug_bin, False, require_browser


def _sidecar_command_prefix() -> list[str]:
    exe = "ficant-sidecar.exe" if sys.platform == "win32" else "ficant-sidecar"
    candidate = Path(sys.executable).parent / exe
    if candidate.is_file():
        return [str(candidate)]
    return [sys.executable, "-m", "ficant_sdk.sidecar"]


def _build_regulatory_rule_pack_binding() -> dict[str, Any]:
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


def _write_credentials(path: Path) -> Path:
    def _sha(tok: str) -> str:
        return hashlib.sha256(tok.encode("utf-8")).hexdigest()

    doc = {
        "schema_version": 1,
        "credentials": [
            {
                "actor_id": "trader-bp4",
                "role": "TRADER",
                "token_sha256_hex": _sha(TRADER_TOKEN),
                "allowed_accounts": [ACCOUNT_ID],
                "allowed_sources": [TRADE_SOURCE_ID, QUOTE_SOURCE_ID],
            },
            {
                "actor_id": "operator-bp4",
                "role": "OPERATOR",
                "token_sha256_hex": _sha(OPERATOR_TOKEN),
                "allowed_accounts": [ACCOUNT_ID],
                "allowed_sources": [TRADE_SOURCE_ID, QUOTE_SOURCE_ID],
            },
            {
                "actor_id": "feed-bp4",
                "role": "FEED_ADAPTER",
                "token_sha256_hex": _sha(FEED_TOKEN),
                "allowed_accounts": [ACCOUNT_ID],
                "allowed_sources": [TRADE_SOURCE_ID, QUOTE_SOURCE_ID],
            },
            {
                "actor_id": "parser-bp4",
                "role": "NLP_PARSER",
                "token_sha256_hex": _sha(PARSER_TOKEN),
                "allowed_accounts": [ACCOUNT_ID],
                "allowed_sources": [],
            },
        ],
    }
    path.write_text(json.dumps(doc, indent=2), encoding="utf-8")
    return path


def _write_policy(path: Path) -> Path:
    doc: dict[str, Any] = {
        "schema_version": 1,
        "policy_version": 1,
        "policy_profile": "B",
        "allowed_hedge_product_codes": ["T", "TF", "TL", "TS"],
        "blotter_config": {
            "clean_price_tolerance": "0.05",
            "yield_tolerance": "0.0005",
            "committed_overdue_seconds": 3600,
            "pre_close_alert_time": "16:30:00",
            "feed_staleness_seconds": 300,
        },
        "default_account_policy": {
            "default_quote_source_id": QUOTE_SOURCE_ID,
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
    *,
    base_suffix: str,
    trading_date: str,
    next_settlement_date: str,
) -> model_pb2.OpeningPositionBase:
    units = _units()
    y, m, d = (int(part) for part in trading_date.split("-"))
    return model_pb2.OpeningPositionBase(
        account_id=ACCOUNT_ID,
        base_id=_ulid(base_suffix),
        trading_date=trading_date,
        next_settlement_date=next_settlement_date,
        effective_from=_shanghai_time(y, m, d, 9, 0, 0),
        watermark_time=_shanghai_time(y, m, d, 9, 0, 0),
        watermark_source_seq=0,
        source_id=TRADE_SOURCE_ID,
        source_start_after_feed_seq=0,
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
    )


def _http_get_raw(port: int, path: str) -> tuple[int, dict[str, str], bytes]:
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
    try:
        conn.request("GET", path)
        resp = conn.getresponse()
        body = resp.read()
        headers = {k.lower(): v for k, v in resp.getheaders()}
        return resp.status, headers, body
    finally:
        conn.close()


def _assert_zero_cors(headers: dict[str, str], label: str) -> None:
    forbidden = [k for k in headers if k.lower().startswith("access-control-allow-")]
    assert not forbidden, f"{label} returned forbidden CORS headers: {forbidden}"


def _wait_tcp_ready(
    port: int, proc: subprocess.Popen[bytes], timeout_s: float = 15.0
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


def _graceful_shutdown(proc: subprocess.Popen[bytes], timeout_s: float = 10.0) -> int:
    if proc.poll() is not None:
        return int(proc.returncode)
    assert proc.stdin is not None
    proc.stdin.write(b"SHUTDOWN\n")
    proc.stdin.flush()
    proc.stdin.close()
    rc = proc.wait(timeout=timeout_s)
    return int(rc)


def _force_cleanup(proc: subprocess.Popen[bytes]) -> None:
    if proc.poll() is None:
        try:
            _graceful_shutdown(proc, timeout_s=5.0)
            return
        except Exception:
            pass
        proc.terminate()
        try:
            proc.wait(timeout=3.0)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=3.0)


@contextmanager
def _managed_server(
    *,
    server_bin: Path,
    isolated_embedded: bool,
    empty_cwd: Path,
    db_path: Path,
    policy_path: Path,
    creds_path: Path,
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
        and k not in ("FICANT_SERVER_ADDR", "FICANT_EMBED_WEB_DIST_DIR")
    }
    env["FICANT_SERVER_ADDR"] = addr
    env["FICANT_BLOTTER_DB_PATH"] = str(db_path)
    env["FICANT_BLOTTER_POLICY_PATH"] = str(policy_path)
    env["FICANT_BLOTTER_CREDENTIALS_PATH"] = str(creds_path)
    env["FICANT_BLOTTER_WATCHDOG_INTERVAL_MS"] = "0"
    env["FICANT_ENABLE_STDIN_SHUTDOWN"] = "1"

    if not isolated_embedded and (DESK_WEB_DIST_DIR / "index.html").is_file():
        env["FICANT_STATIC_WEB_DIR"] = str(DESK_WEB_DIST_DIR)

    proc = subprocess.Popen(
        [str(server_bin)],
        cwd=empty_cwd,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        _wait_tcp_ready(port, proc)
        yield proc, addr, port
    finally:
        _force_cleanup(proc)


def _file_fingerprint(path: Path) -> tuple[bool, int, str]:
    if not path.exists():
        return (False, 0, "")
    data = path.read_bytes()
    return (True, len(data), hashlib.sha256(data).hexdigest())


def _run_sidecar_cli(
    args: list[str],
    *,
    env: dict[str, str],
    cwd: Path,
    timeout_s: float = 25.0,
) -> subprocess.CompletedProcess[str]:
    cmd = [*_sidecar_command_prefix(), *args]
    return subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout_s,
        check=False,
    )


def test_bp4_single_binary_cross_day_intranet_e2e(tmp_path: Path) -> None:
    server_bin, isolated_embedded, require_browser = _resolve_server_mode()

    # Completely empty runtime CWD (zero source tree, zero dist directory).
    empty_cwd = tmp_path / "empty_runtime_cwd"
    empty_cwd.mkdir()

    data_dir = tmp_path / "ledger"
    data_dir.mkdir()
    db_path = data_dir / "desk_blotter.sqlite"
    wal_path = data_dir / "desk_blotter.sqlite-wal"
    archive_dir = tmp_path / "archives"
    archive_dir.mkdir()

    cfg_dir = tmp_path / "cfg"
    cfg_dir.mkdir()
    creds_path = _write_credentials(cfg_dir / "creds.json")
    policy_path = _write_policy(cfg_dir / "policy.json")

    cp_dir = tmp_path / "checkpoints"
    cp_dir.mkdir()
    quotes_cp = cp_dir / "quotes.cp.json"
    trades_cp = cp_dir / "trades.cp.json"
    nlp_cp = cp_dir / "nlp.cp.json"

    io_dir = tmp_path / "inputs"
    io_dir.mkdir()

    sidecar_env = {
        **os.environ,
        "FICANT_BP4_FEED_TOKEN": FEED_TOKEN,
        "FICANT_BP4_PARSER_TOKEN": PARSER_TOKEN,
    }

    evidence: dict[str, Any] = {
        "isolated_embedded_mode": isolated_embedded,
        "browser_e2e_executed": require_browser,
        "server_binary": str(server_bin),
    }

    # ==================================================================
    # DAY 1: Boot, /healthz & 159B RulePack proof, Sidecars, UI/SDK
    #        Confirm + ACCEPT_ORPHAN, Graceful Shutdown
    # ==================================================================
    with _managed_server(
        server_bin=server_bin,
        isolated_embedded=isolated_embedded,
        empty_cwd=empty_cwd,
        db_path=db_path,
        policy_path=policy_path,
        creds_path=creds_path,
    ) as (proc1, endpoint1, port1):
        # 1. Verify /healthz contract & zero CORS headers.
        hz_status, hz_headers, hz_body = _http_get_raw(port1, "/healthz")
        assert hz_status == 200
        _assert_zero_cors(hz_headers, "Day-1 /healthz")
        health_day1_raw = json.loads(hz_body.decode("utf-8"))
        assert health_day1_raw["status"] == "ok"
        assert health_day1_raw["service"] == "ficant-server"
        assert health_day1_raw["storage_mode"] == "persistent_wal"
        assert health_day1_raw["blotter_healthy"] is True
        assert health_day1_raw["ledger_epoch"] == 1
        inst1 = health_day1_raw["ledger_instance_id"]
        assert isinstance(inst1, str) and len(inst1) == 26
        evidence["day1_healthz"] = health_day1_raw

        if isolated_embedded:
            assert health_day1_raw["web_ui_mode"] == "embedded"
            assert health_day1_raw["embedded_assets_count"] > 0

            # Verify embedded 159B RulePack SHA-256 & zero CORS from zero-source/zero-dist process.
            rp_status, rp_headers, rp_body = _http_get_raw(
                port1, "/rulepacks/cgb-futures-v2.bin"
            )
            assert rp_status == 200
            _assert_zero_cors(rp_headers, "embedded /rulepacks/cgb-futures-v2.bin")
            assert rp_headers.get("x-content-type-options") == "nosniff"
            assert len(rp_body) == PINNED_RULEPACK_BYTES
            rp_sha = hashlib.sha256(rp_body).hexdigest()
            assert rp_sha == PINNED_RULEPACK_SHA256
            evidence["embedded_rulepack_bytes"] = len(rp_body)
            evidence["embedded_rulepack_sha256"] = rp_sha

            idx_status, idx_headers, idx_body = _http_get_raw(port1, "/")
            assert idx_status == 200
            _assert_zero_cors(idx_headers, "embedded /")
            assert len(idx_body) > 0

        # Also validate via SDK probe_healthz.
        sdk_health1 = probe_healthz(endpoint1, insecure=True)
        ledger1 = sdk_health1.require_healthy_ledger()
        assert ledger1.as_tuple() == (1, inst1)

        # 2. Import Day-1 opening base with two complete fixed-rate bonds (BOND_10Y, BOND_5Y).
        with DeskBlotterClient(endpoint1, bearer_token=OPERATOR_TOKEN, insecure=True) as op:
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="bp4-day1-import",
                    opening_base=_make_opening_base(
                        base_suffix="D1",
                        trading_date=DAY1_DATE,
                        next_settlement_date=DAY1_NEXT_SETTLE,
                    ),
                    expected_account_version=0,
                )
            )

        # 3a. Day-1 Market Quotes Sidecar CLI (FEED_ADAPTER) with fresh SystemBlotterClock timestamps.
        quotes_day1_csv = io_dir / "quotes_day1.csv"
        obs1 = _fresh_rfc3339(4)
        rec1 = _fresh_rfc3339(2)
        quotes_day1_csv.write_text(
            "account_id,source_id,instrument_code,quote_seq,source_type,mid_yield_to_maturity,observed_at,received_at\n"
            f"{ACCOUNT_ID},{QUOTE_SOURCE_ID},BOND_10Y,1,ACTIVE_QUOTE,0.025,{obs1},{rec1}\n"
            f"{ACCOUNT_ID},{QUOTE_SOURCE_ID},BOND_5Y,1,ACTIVE_QUOTE,0.022,{obs1},{rec1}\n",
            encoding="utf-8",
        )
        res_q1 = _run_sidecar_cli(
            [
                "quotes",
                "--endpoint",
                endpoint1,
                "--token-env",
                "FICANT_BP4_FEED_TOKEN",
                "--header-mode",
                "bearer",
                "--insecure",
                "--checkpoint",
                str(quotes_cp),
                "--trading-date",
                DAY1_DATE,
                "--sidecar-id",
                "bp4-quotes-sidecar",
                "--input",
                str(quotes_day1_csv),
                "--expected-ledger-epoch",
                "1",
                "--expected-ledger-instance",
                inst1,
                "--once",
            ],
            env=sidecar_env,
            cwd=empty_cwd,
        )
        assert res_q1.returncode == 0, f"Day-1 quotes sidecar failed: {res_q1.stderr}"
        q1_report = json.loads(res_q1.stdout.strip())
        assert q1_report["applied_tick_count"] == 2

        # 3b. Day-1 Trade Feed Sidecar CLI (FEED_ADAPTER) with an orphan fill on BOND_5Y.
        trades_day1_jsonl = io_dir / "trades_day1.jsonl"
        trades_day1_jsonl.write_text(
            json.dumps(
                {
                    "source_id": TRADE_SOURCE_ID,
                    "external_trade_id": "EXT-DAY1-BOND5Y-001",
                    "source_revision": 1,
                    "feed_seq": 1,
                    "action": "NEW_FILL",
                    "account_id": ACCOUNT_ID,
                    "instrument_code": "BOND_5Y",
                    "side": "BUY",
                    "settlement_date": DAY1_DATE,
                    "notional": "5000000",
                    "clean_price_per_100": "100.10",
                    "settlement_amount": "5005000",
                    "executed_at": f"{DAY1_DATE}T10:05:00+08:00",
                    "batch_first_feed_seq": 1,
                    "batch_last_feed_seq": 1,
                }
            )
            + "\n",
            encoding="utf-8",
        )
        res_t1 = _run_sidecar_cli(
            [
                "trade-feed",
                "--endpoint",
                endpoint1,
                "--token-env",
                "FICANT_BP4_FEED_TOKEN",
                "--header-mode",
                "x-ficant-credential",
                "--insecure",
                "--checkpoint",
                str(trades_cp),
                "--trading-date",
                DAY1_DATE,
                "--sidecar-id",
                "bp4-trades-sidecar",
                "--source-id",
                TRADE_SOURCE_ID,
                "--input",
                str(trades_day1_jsonl),
                "--expected-ledger-epoch",
                "1",
                "--expected-ledger-instance",
                inst1,
                "--once",
            ],
            env=sidecar_env,
            cwd=empty_cwd,
        )
        assert res_t1.returncode == 0, f"Day-1 trade-feed sidecar failed: {res_t1.stderr}"
        t1_report = json.loads(res_t1.stdout.strip())
        assert t1_report["fills_sent"] == 1
        assert t1_report["contiguous_feed_seq"] == 1

        # Verify preview context is settled: BOND_10Y is COVERED_PRE_TAX + PRE_TAX + non-zero unit-bearing DV01.
        with DeskBlotterClient(endpoint1, bearer_token=TRADER_TOKEN, insecure=True) as tr:
            snap_pre_draft = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id=ACCOUNT_ID)
            ).snapshot
            pos_10y = next(
                p for p in snap_pre_draft.positions if p.instrument_code == "BOND_10Y"
            )
            assert pos_10y.coverage_status == model_pb2.COVERAGE_STATUS_COVERED_PRE_TAX
            assert pos_10y.tax_basis == rates_pb2.BOND_TAX_BASIS_PRE_TAX
            assert pos_10y.HasField("pre_tax_dv01")
            assert pos_10y.pre_tax_dv01.HasField("unit")
            dv01_unit_id = pos_10y.pre_tax_dv01.unit.unit_id.value
            assert len(dv01_unit_id) == 26
            dv01_dec = _decimal_from_pb(pos_10y.pre_tax_dv01)
            assert dv01_dec != Decimal("0")
            dv01_exact_str = _decimal_str_from_pb(pos_10y.pre_tax_dv01)
            assert len(snap_pre_draft.open_pending_matches) == 1
            assert snap_pre_draft.open_pending_matches[0].instrument_code == "BOND_5Y"
            evidence["bond_10y_pre_tax_dv01"] = dv01_exact_str
            evidence["bond_10y_dv01_unit_id"] = dv01_unit_id

        # 4. Day-1 NLP Draft Sidecar CLI (NLP_PARSER) submitting BOND_10Y TARGET=COMMITTED draft.
        nlp_day1_jsonl = io_dir / "nlp_day1.jsonl"
        nlp_day1_jsonl.write_text(
            json.dumps(
                {
                    "request_id": "bp4-day1-nlp-001",
                    "account_id": ACCOUNT_ID,
                    "raw_source_text": NLP_COMMITTED_VECTOR,
                },
                ensure_ascii=False,
            )
            + "\n",
            encoding="utf-8",
        )
        res_n1 = _run_sidecar_cli(
            [
                "nlp-draft",
                "--endpoint",
                endpoint1,
                "--token-env",
                "FICANT_BP4_PARSER_TOKEN",
                "--header-mode",
                "bearer",
                "--insecure",
                "--checkpoint",
                str(nlp_cp),
                "--trading-date",
                DAY1_DATE,
                "--sidecar-id",
                "bp4-nlp-sidecar",
                "--input",
                str(nlp_day1_jsonl),
                "--adapter",
                "token",
                "--expected-ledger-epoch",
                "1",
                "--expected-ledger-instance",
                inst1,
                "--once",
            ],
            env=sidecar_env,
            cwd=empty_cwd,
        )
        assert res_n1.returncode == 0, f"Day-1 nlp-draft sidecar failed: {res_n1.stderr}"
        n1_report = json.loads(res_n1.stdout.strip())
        assert n1_report["drafts_submitted"] == 1
        assert len(n1_report["draft_ids"]) == 1
        draft_id = n1_report["draft_ids"][0]
        evidence["day1_draft_id"] = draft_id

        # 5. Confirm exact Sidecar draft_id & ACCEPT_ORPHAN BOND_5Y (via Chromium UI or SDK TRADER).
        if require_browser:
            ui_report_path = tmp_path / "ui_report.json"
            ui_env = {
                **os.environ,
                "FICANT_BP4_UI_TRADER_TOKEN": TRADER_TOKEN,
            }
            ui_res = subprocess.run(
                [
                    "node",
                    str(UI_VERIFIER_SCRIPT),
                    "--base-url",
                    f"http://127.0.0.1:{port1}/",
                    "--token-env",
                    "FICANT_BP4_UI_TRADER_TOKEN",
                    "--account-id",
                    ACCOUNT_ID,
                    "--draft-id",
                    draft_id,
                    "--expected-bond-code",
                    "BOND_10Y",
                    "--expected-pre-tax-dv01",
                    dv01_exact_str,
                    "--expected-dv01-unit",
                    dv01_unit_id,
                    "--orphan-instrument-code",
                    "BOND_5Y",
                    "--report-out",
                    str(ui_report_path),
                ],
                cwd=REPO_ROOT / "apps" / "desk-web",
                env=ui_env,
                capture_output=True,
                text=True,
                timeout=90,
                check=False,
            )
            assert (
                ui_res.returncode == 0
            ), f"verify-single-binary-ui.mjs failed (rc={ui_res.returncode}):\nSTDOUT:\n{ui_res.stdout}\nSTDERR:\n{ui_res.stderr}"
            ui_report = json.loads(ui_report_path.read_text(encoding="utf-8"))
            assert ui_report["status"] == "pass"
            assert ui_report["confirmed_draft_id"] == draft_id
            assert ui_report["cors_header_violations"] == 0
            assert ui_report["console_errors"] == 0
            assert ui_report["page_errors"] == 0
            assert ui_report["http_errors"] == 0
            evidence["browser_ui_report"] = ui_report
        else:
            with DeskBlotterClient(endpoint1, bearer_token=TRADER_TOKEN, insecure=True) as tr:
                snap_for_confirm = tr.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id=ACCOUNT_ID)
                ).snapshot
                target_draft = next(
                    d for d in snap_for_confirm.pending_drafts if d.draft_id.value == draft_id
                )
                pos_10y_now = next(
                    p for p in snap_for_confirm.positions if p.instrument_code == "BOND_10Y"
                )
                tr.confirm_draft_earmark(
                    service_pb2.ConfirmDraftEarmarkRequest(
                        idempotency_key="bp4-sdk-confirm-draft",
                        account_id=ACCOUNT_ID,
                        draft_id=target_draft.draft_id,
                        expected_draft_revision=target_draft.draft_revision,
                        expected_position_version=pos_10y_now.position_version,
                        confirmation_token=target_draft.confirmation_token,
                        acknowledge_uncovered_dv01=False,
                    )
                )
                snap_for_pm = tr.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id=ACCOUNT_ID)
                ).snapshot
                orphan_pm = next(
                    pm for pm in snap_for_pm.open_pending_matches if pm.instrument_code == "BOND_5Y"
                )
                pos_5y_now = next(
                    p for p in snap_for_pm.positions if p.instrument_code == "BOND_5Y"
                )
                tr.resolve_pending_match(
                    service_pb2.ResolvePendingMatchRequest(
                        idempotency_key="bp4-sdk-accept-orphan",
                        account_id=ACCOUNT_ID,
                        pending_match_id=orphan_pm.pending_match_id,
                        expected_pending_revision=orphan_pm.pending_revision,
                        expected_position_version=pos_5y_now.position_version,
                        action=model_pb2.PENDING_MATCH_RESOLUTION_ACTION_ACCEPT_ORPHAN,
                        reason="bp4-sdk-accept-orphan",
                    )
                )

        # Assert pre-shutdown snapshot has EXACTLY 1 COMMITTED (BOND_10Y) and 0 OPEN pending matches!
        with DeskBlotterClient(endpoint1, bearer_token=TRADER_TOKEN, insecure=True) as tr:
            snap_pre_stop = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id=ACCOUNT_ID)
            ).snapshot
            committed = [
                e
                for e in snap_pre_stop.active_earmarks
                if e.state == model_pb2.EARMARK_STATE_COMMITTED
            ]
            assert len(committed) == 1
            assert committed[0].instrument_code == "BOND_10Y"
            assert _decimal_from_pb(committed[0].notional) == Decimal("10000000")
            assert len(snap_pre_stop.open_pending_matches) == 0
            assert len(snap_pre_stop.pending_drafts) == 0
            committed_earmark_id = committed[0].earmark_id.value

        # 6. Graceful SHUTDOWN -> exit 0!
        shutdown_rc1 = _graceful_shutdown(proc1)
        assert shutdown_rc1 == 0, f"Day-1 graceful SHUTDOWN exited {shutdown_rc1}"

    # 6b. Unresolved rollover WITHOUT --allow-unresolved MUST block with 0-byte mutation!
    db_before_block = _file_fingerprint(db_path)
    wal_before_block = _file_fingerprint(wal_path)
    res_blocked = subprocess.run(
        [
            str(server_bin),
            "--sqlite-rollover",
            "--db-path",
            str(db_path),
            "--archive-dir",
            str(archive_dir),
        ],
        cwd=empty_cwd,
        capture_output=True,
        text=True,
        timeout=15,
        check=False,
    )
    assert res_blocked.returncode != 0, "Expected rollover without --allow-unresolved to block"
    assert _file_fingerprint(db_path) == db_before_block, "Blocked rollover mutated source DB!"
    assert _file_fingerprint(wal_path) == wal_before_block, "Blocked rollover mutated source WAL!"

    # ==================================================================
    # DAY 1 SETTLE: Restart, clear BOND_10Y COMMITTED earmark, SHUTDOWN,
    #               Rollover (epoch 1 -> 2), Verify Archive Manifest
    # ==================================================================
    with _managed_server(
        server_bin=server_bin,
        isolated_embedded=isolated_embedded,
        empty_cwd=empty_cwd,
        db_path=db_path,
        policy_path=policy_path,
        creds_path=creds_path,
    ) as (proc1b, endpoint1b, _port1b):
        h1b = probe_healthz(endpoint1b, insecure=True).require_healthy_ledger()
        assert h1b.as_tuple() == (1, inst1)

        with DeskBlotterClient(endpoint1b, bearer_token=TRADER_TOKEN, insecure=True) as tr:
            snap_settle = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id=ACCOUNT_ID)
            ).snapshot
            target_em = next(
                e
                for e in snap_settle.active_earmarks
                if e.earmark_id.value == committed_earmark_id
            )
            tr.cancel_committed(
                service_pb2.CancelCommittedRequest(
                    idempotency_key="bp4-day1-clear-committed",
                    account_id=ACCOUNT_ID,
                    earmark_id=target_em.earmark_id,
                    expected_earmark_revision=target_em.earmark_revision,
                    reason="eod-clear-before-rollover",
                )
            )
            snap_clean = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id=ACCOUNT_ID)
            ).snapshot
            assert len(snap_clean.active_earmarks) == 0
            assert len(snap_clean.open_pending_matches) == 0

        shutdown_rc1b = _graceful_shutdown(proc1b)
        assert shutdown_rc1b == 0, f"Day-1 settle SHUTDOWN exited {shutdown_rc1b}"

    # 8. Rollover WITHOUT --allow-unresolved MUST now succeed (epoch 1 -> 2, empty/absent WAL).
    res_rollover = subprocess.run(
        [
            str(server_bin),
            "--sqlite-rollover",
            "--db-path",
            str(db_path),
            "--archive-dir",
            str(archive_dir),
        ],
        cwd=empty_cwd,
        capture_output=True,
        text=True,
        timeout=20,
        check=False,
    )
    assert res_rollover.returncode == 0, f"--sqlite-rollover failed: {res_rollover.stderr}"
    rollover_report = json.loads(res_rollover.stdout)
    manifest = rollover_report["manifest"]
    assert manifest["mode"] == "rollover_reset"
    assert manifest["status"] == "completed_reset"
    assert manifest["source_reset_committed"] is True
    assert manifest["source_ledger_epoch"] == 1
    assert manifest["source_ledger_instance_id"] == inst1
    assert manifest["next_ledger_epoch"] == 2
    inst2 = manifest["next_ledger_instance_id"]
    assert isinstance(inst2, str) and len(inst2) == 26 and inst2 != inst1
    assert manifest["allow_unresolved_used"] is False
    assert manifest["wal_truncated"] is True
    assert (not wal_path.exists()) or wal_path.stat().st_size == 0

    manifest_path = Path(rollover_report["manifest_path"])
    assert manifest_path.is_file()
    res_verify = subprocess.run(
        [
            str(server_bin),
            "--verify-sqlite-archive",
            "--manifest",
            str(manifest_path),
        ],
        cwd=empty_cwd,
        capture_output=True,
        text=True,
        timeout=15,
        check=False,
    )
    assert res_verify.returncode == 0, f"--verify-sqlite-archive failed: {res_verify.stderr}"
    verify_report = json.loads(res_verify.stdout)
    assert verify_report["manifest"]["archive_sha256_hex"] == manifest["archive_sha256_hex"]
    assert verify_report["manifest"]["content_sha256_hex"] == manifest["content_sha256_hex"]
    evidence["rollover_epoch_transition"] = {
        "from_epoch": 1,
        "from_instance": inst1,
        "to_epoch": 2,
        "to_instance": inst2,
        "archive_sha256_hex": manifest["archive_sha256_hex"],
        "wal_truncated": manifest["wal_truncated"],
    }

    # ==================================================================
    # DAY 2: Boot epoch=2, Import Day-2 Base, Reject Day-1 Inputs,
    #        Accept Day-2 quote_seq=1 & feed_seq=1, Stale epoch=1 Fence
    # ==================================================================
    with _managed_server(
        server_bin=server_bin,
        isolated_embedded=isolated_embedded,
        empty_cwd=empty_cwd,
        db_path=db_path,
        policy_path=policy_path,
        creds_path=creds_path,
    ) as (proc2, endpoint2, _port2):
        sdk_health2 = probe_healthz(endpoint2, insecure=True)
        ledger2 = sdk_health2.require_healthy_ledger()
        assert ledger2.as_tuple() == (2, inst2)

        # Import Day-2 opening base (trading_date advanced, source_start_after_feed_seq=0, watermark_source_seq=0).
        with DeskBlotterClient(endpoint2, bearer_token=OPERATOR_TOKEN, insecure=True) as op:
            op.import_opening_base(
                service_pb2.ImportOpeningBaseRequest(
                    idempotency_key="bp4-day2-import",
                    opening_base=_make_opening_base(
                        base_suffix="D2",
                        trading_date=DAY2_DATE,
                        next_settlement_date=DAY2_NEXT_SETTLE,
                    ),
                    expected_account_version=0,
                )
            )

        with DeskBlotterClient(endpoint2, bearer_token=TRADER_TOKEN, insecure=True) as tr:
            seq_after_day2_import = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id=ACCOUNT_ID)
            ).snapshot.snapshot_seq

        # 10a. Both sidecars carrying ORIGINAL Day-1 checkpoints MUST reject yesterday's Day-1 input files!
        res_q_reject = _run_sidecar_cli(
            [
                "quotes",
                "--endpoint",
                endpoint2,
                "--token-env",
                "FICANT_BP4_FEED_TOKEN",
                "--insecure",
                "--checkpoint",
                str(quotes_cp),
                "--trading-date",
                DAY2_DATE,
                "--sidecar-id",
                "bp4-quotes-sidecar",
                "--input",
                str(quotes_day1_csv),
                "--expected-ledger-epoch",
                "2",
                "--expected-ledger-instance",
                inst2,
                "--once",
            ],
            env=sidecar_env,
            cwd=empty_cwd,
        )
        assert res_q_reject.returncode != 0, "Day-2 quotes sidecar must reject unchanged Day-1 input file"

        res_t_reject = _run_sidecar_cli(
            [
                "trade-feed",
                "--endpoint",
                endpoint2,
                "--token-env",
                "FICANT_BP4_FEED_TOKEN",
                "--insecure",
                "--checkpoint",
                str(trades_cp),
                "--trading-date",
                DAY2_DATE,
                "--sidecar-id",
                "bp4-trades-sidecar",
                "--source-id",
                TRADE_SOURCE_ID,
                "--input",
                str(trades_day1_jsonl),
                "--expected-ledger-epoch",
                "2",
                "--expected-ledger-instance",
                inst2,
                "--once",
            ],
            env=sidecar_env,
            cwd=empty_cwd,
        )
        assert res_t_reject.returncode != 0, "Day-2 trade-feed sidecar must reject unchanged Day-1 input file"

        with DeskBlotterClient(endpoint2, bearer_token=TRADER_TOKEN, insecure=True) as tr:
            assert (
                tr.get_account_snapshot(
                    service_pb2.GetAccountSnapshotRequest(account_id=ACCOUNT_ID)
                ).snapshot.snapshot_seq
                == seq_after_day2_import
            )

        # 10b. Accept Day-2 NEW input files starting at quote_seq=1 and feed_seq=1!
        quotes_day2_csv = io_dir / "quotes_day2.csv"
        obs2 = _fresh_rfc3339(3)
        rec2 = _fresh_rfc3339(1)
        quotes_day2_csv.write_text(
            "account_id,source_id,instrument_code,quote_seq,source_type,mid_yield_to_maturity,observed_at,received_at\n"
            f"{ACCOUNT_ID},{QUOTE_SOURCE_ID},BOND_10Y,1,ACTIVE_QUOTE,0.0265,{obs2},{rec2}\n"
            f"{ACCOUNT_ID},{QUOTE_SOURCE_ID},BOND_5Y,1,ACTIVE_QUOTE,0.0235,{obs2},{rec2}\n",
            encoding="utf-8",
        )
        res_q2 = _run_sidecar_cli(
            [
                "quotes",
                "--endpoint",
                endpoint2,
                "--token-env",
                "FICANT_BP4_FEED_TOKEN",
                "--insecure",
                "--checkpoint",
                str(quotes_cp),
                "--trading-date",
                DAY2_DATE,
                "--sidecar-id",
                "bp4-quotes-sidecar",
                "--input",
                str(quotes_day2_csv),
                "--expected-ledger-epoch",
                "2",
                "--expected-ledger-instance",
                inst2,
                "--once",
            ],
            env=sidecar_env,
            cwd=empty_cwd,
        )
        assert res_q2.returncode == 0, f"Day-2 quotes sidecar failed: {res_q2.stderr}"
        q2_report = json.loads(res_q2.stdout.strip())
        assert q2_report["applied_tick_count"] == 2
        assert q2_report["ledger_epoch"] == 2
        assert q2_report["ledger_instance_id"] == inst2

        trades_day2_jsonl = io_dir / "trades_day2.jsonl"
        trades_day2_jsonl.write_text(
            json.dumps(
                {
                    "source_id": TRADE_SOURCE_ID,
                    "external_trade_id": "EXT-DAY2-BOND5Y-001",
                    "source_revision": 1,
                    "feed_seq": 1,
                    "action": "NEW_FILL",
                    "account_id": ACCOUNT_ID,
                    "instrument_code": "BOND_5Y",
                    "side": "BUY",
                    "settlement_date": DAY2_DATE,
                    "notional": "3000000",
                    "clean_price_per_100": "100.20",
                    "settlement_amount": "3006000",
                    "executed_at": f"{DAY2_DATE}T09:35:00+08:00",
                    "batch_first_feed_seq": 1,
                    "batch_last_feed_seq": 1,
                }
            )
            + "\n",
            encoding="utf-8",
        )
        res_t2 = _run_sidecar_cli(
            [
                "trade-feed",
                "--endpoint",
                endpoint2,
                "--token-env",
                "FICANT_BP4_FEED_TOKEN",
                "--insecure",
                "--checkpoint",
                str(trades_cp),
                "--trading-date",
                DAY2_DATE,
                "--sidecar-id",
                "bp4-trades-sidecar",
                "--source-id",
                TRADE_SOURCE_ID,
                "--input",
                str(trades_day2_jsonl),
                "--expected-ledger-epoch",
                "2",
                "--expected-ledger-instance",
                inst2,
                "--once",
            ],
            env=sidecar_env,
            cwd=empty_cwd,
        )
        assert res_t2.returncode == 0, f"Day-2 trade-feed sidecar failed: {res_t2.stderr}"
        t2_report = json.loads(res_t2.stdout.strip())
        assert t2_report["fills_sent"] == 1
        assert t2_report["contiguous_feed_seq"] == 1
        assert t2_report["ledger_epoch"] == 2
        assert t2_report["ledger_instance_id"] == inst2

        # Verify Day-2 quote_seq=1 applied & feed_seq=1 advanced with exact 1 business effect and ZERO Day-1 replay.
        with DeskBlotterClient(endpoint2, bearer_token=TRADER_TOKEN, insecure=True) as tr:
            snap_day2 = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id=ACCOUNT_ID)
            ).snapshot
            day2_pos_10y = next(
                p for p in snap_day2.positions if p.instrument_code == "BOND_10Y"
            )
            assert day2_pos_10y.coverage_status == model_pb2.COVERAGE_STATUS_COVERED_PRE_TAX
            assert day2_pos_10y.active_quote_seq == 1
            assert _decimal_from_pb(day2_pos_10y.valuation_yield_to_maturity) == Decimal("0.0265")

            feed_status = next(
                f for f in snap_day2.feed_statuses if f.source_id == TRADE_SOURCE_ID
            )
            assert feed_status.contiguous_feed_seq == 1

            assert len(snap_day2.open_pending_matches) == 1
            day2_pm = snap_day2.open_pending_matches[0]
            assert day2_pm.external_trade_id == "EXT-DAY2-BOND5Y-001"
            assert _decimal_from_pb(day2_pm.unresolved_notional) == Decimal("3000000")
            assert len(snap_day2.active_earmarks) == 0
            assert len(snap_day2.pending_drafts) == 0
            day2_seq_before_stale = snap_day2.snapshot_seq

        # 10c. Inject stale epoch=1 metadata across ALL 3 write RPCs -> CONCURRENCY_CONFLICT + 0 writes!
        units = _units()
        stale_epoch1 = (1, inst1)
        with DeskBlotterClient(endpoint2, bearer_token=FEED_TOKEN, insecure=True) as feed_c:
            with pytest.raises(BlotterError) as q_err:
                feed_c.ingest_market_quotes(
                    service_pb2.IngestMarketQuotesRequest(
                        idempotency_key="bp4-day2-stale-q",
                        account_id=ACCOUNT_ID,
                        source_id=QUOTE_SOURCE_ID,
                        ticks=[
                            spi_pb2.MarketQuoteTick(
                                source_id=QUOTE_SOURCE_ID,
                                quote_seq=2,
                                instrument_code="BOND_10Y",
                                source_type=2,
                                mid_yield_to_maturity=decimal_to_pb("0.029", units.rate),
                                observed_at=parse_market_time(_fresh_rfc3339(3)),
                                received_at=parse_market_time(_fresh_rfc3339(1)),
                            )
                        ],
                    ),
                    expected_ledger=stale_epoch1,
                )
            assert q_err.value.code == error_pb2.ERROR_CODE_CONCURRENCY_CONFLICT
            assert q_err.value.retryable is False

            with pytest.raises(BlotterError) as tf_err:
                feed_c.ingest_trade_feed(
                    service_pb2.IngestTradeFeedRequest(
                        idempotency_key="bp4-day2-stale-tf",
                        source_id=TRADE_SOURCE_ID,
                        fills=[
                            spi_pb2.TradeFillRecord(
                                source_id=TRADE_SOURCE_ID,
                                external_trade_id="EXT-STALE-EPOCH1",
                                source_revision=1,
                                feed_seq=2,
                                action=model_pb2.TRADE_FEED_RECORD_ACTION_NEW_FILL,
                                account_id=ACCOUNT_ID,
                                instrument_code="BOND_5Y",
                                side=model_pb2.TRADE_SIDE_BUY,
                                settlement_date=DAY2_DATE,
                                notional=decimal_to_pb("2000000", units.currency_amount),
                                quote_terms=model_pb2.QuoteTerms(
                                    clean_price_per_100=decimal_to_pb(
                                        "100.00", units.price_per_100
                                    ),
                                    settlement_amount=decimal_to_pb(
                                        "2000000", units.currency_amount
                                    ),
                                ),
                                executed_at=_shanghai_time(2026, 6, 16, 10, 0, 0),
                                batch_first_feed_seq=2,
                                batch_last_feed_seq=2,
                            )
                        ],
                    ),
                    expected_ledger=stale_epoch1,
                )
            assert tf_err.value.code == error_pb2.ERROR_CODE_CONCURRENCY_CONFLICT
            assert tf_err.value.retryable is False

        with NlpDraftOnlyClient(endpoint2, bearer_token=PARSER_TOKEN, insecure=True) as nlp_c:
            proposal = parse_token_nlp_intent(
                request_id="bp4-day2-stale-nlp",
                account_id=ACCOUNT_ID,
                raw_source_text=NLP_COMMITTED_VECTOR,
                units=units,
            )
            with pytest.raises(BlotterError) as nlp_err:
                nlp_c.submit_draft_earmark(
                    service_pb2.SubmitDraftEarmarkRequest(
                        idempotency_key="bp4-day2-stale-nlp",
                        account_id=ACCOUNT_ID,
                        proposal=proposal,
                    ),
                    expected_ledger=stale_epoch1,
                )
            assert nlp_err.value.code == error_pb2.ERROR_CODE_CONCURRENCY_CONFLICT
            assert nlp_err.value.retryable is False

        # Verify ZERO writes occurred on Day-2 epoch=2 ledger from stale epoch=1 metadata!
        with DeskBlotterClient(endpoint2, bearer_token=TRADER_TOKEN, insecure=True) as tr:
            snap_day2_final = tr.get_account_snapshot(
                service_pb2.GetAccountSnapshotRequest(account_id=ACCOUNT_ID)
            ).snapshot
            assert snap_day2_final.snapshot_seq == day2_seq_before_stale
            assert len(snap_day2_final.open_pending_matches) == 1
            assert len(snap_day2_final.pending_drafts) == 0
            assert len(snap_day2_final.active_earmarks) == 0

        evidence["day2_verified"] = {
            "ledger_epoch": 2,
            "ledger_instance_id": inst2,
            "quote_seq_1_ytm": "0.0265",
            "feed_seq_1_contiguous": 1,
            "day1_files_rejected": True,
            "stale_epoch1_3_rpcs_zero_writes": True,
        }

        shutdown_rc2 = _graceful_shutdown(proc2)
        assert shutdown_rc2 == 0, f"Day-2 graceful SHUTDOWN exited {shutdown_rc2}"

    evidence_out = os.environ.get("FICANT_BP4_EVIDENCE_OUT", "").strip()
    if evidence_out:
        Path(evidence_out).write_text(
            json.dumps(evidence, indent=2, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )
