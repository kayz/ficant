"""Live Python SDK parity against the production Rust/C++ service composition."""

from __future__ import annotations

import hashlib
import http.client
import json
import os
import socket
import subprocess
import time
from datetime import datetime, timezone
from decimal import Decimal
from pathlib import Path

import pytest
from ficant.core.v1 import common_pb2
from ficant.rates.v1 import analytics_pb2 as rates
from ficant_sdk import RatesClient
from google.protobuf.timestamp_pb2 import Timestamp


REPO_ROOT = Path(__file__).resolve().parents[2]
GOLDEN_ROOT = REPO_ROOT / "tests" / "golden-cases" / "china-rates"
ULID_PREFIX = "01ARZ3NDEKTSV4RRFFQ69G5FA"
TOKEN = "phase2e-python-sdk-test-token"
KEY = "3031323334353637383961626364656630313233343536373839616263646566"


def _ulid(suffix: str) -> common_pb2.Ulid:
    return common_pb2.Ulid(value=f"{ULID_PREFIX}{suffix}")


def _hash(label: str) -> common_pb2.Sha256:
    return common_pb2.Sha256(value=hashlib.sha256(label.encode()).digest())


def _object(suffix: str) -> rates.ObjectBinding:
    return rates.ObjectBinding(
        object=common_pb2.VersionRef(id=_ulid(suffix), version=1),
        content_hash=_hash(f"object-{suffix}"),
    )


def _unit(suffix: str) -> common_pb2.UnitRef:
    return common_pb2.UnitRef(unit_id=_ulid(suffix), version=1)


UNITS = rates.AnalysisUnits(
    currency_amount=_unit("A"),
    price_per_100=_unit("B"),
    rate=_unit("C"),
    years=_unit("D"),
    years_squared=_unit("E"),
    dv01_per_100=_unit("F"),
    dv01=_unit("G"),
    dimensionless=_unit("H"),
    contract_count=_unit("J"),
)


def _decimal(value: str, unit: common_pb2.UnitRef) -> common_pb2.DecimalValue:
    number = Decimal(value)
    sign, digits, exponent = number.as_tuple()
    coefficient = int("".join(str(digit) for digit in digits) or "0")
    if sign:
        coefficient = -coefficient
    if exponent >= 0:
        coefficient *= 10**exponent
        scale = 0
    else:
        scale = -exponent
    if coefficient == 0:
        scale = 0
    while scale > 0 and coefficient % 10 == 0:
        coefficient //= 10
        scale -= 1
    return common_pb2.DecimalValue(
        coefficient=str(coefficient), scale=scale, unit=unit
    )


def _decimal_value(value: common_pb2.DecimalValue) -> Decimal:
    return Decimal(value.coefficient).scaleb(-value.scale)


def _context(
    algorithm_id: str, convention: str, *, rule_suffix: str, snapshot_suffix: str
) -> rates.AnalysisContext:
    return rates.AnalysisContext(
        owner=common_pb2.OwnerRef(tenant_id=_ulid("0"), owner_id=_ulid("1")),
        rule_pack=_object(rule_suffix),
        data_snapshot=_object(snapshot_suffix),
        algorithm=rates.AlgorithmBinding(
            algorithm_id=algorithm_id,
            algorithm_version=1,
            convention_profile=convention,
            abi_version=1,
        ),
        units=UNITS,
    )


def _market_time(iso_value: str, local_date: str) -> common_pb2.MarketTime:
    instant = datetime.fromisoformat(iso_value).astimezone(timezone.utc)
    timestamp = Timestamp(seconds=int(instant.timestamp()), nanos=instant.microsecond * 1_000)
    return common_pb2.MarketTime(
        instant=timestamp,
        market_timezone="Asia/Shanghai",
        local_trading_date=local_date,
    )


def _terms(value: dict[str, object]) -> rates.BondTerms:
    frequency = (
        rates.COUPON_FREQUENCY_ANNUAL
        if int(value["frequency"]) == 1
        else rates.COUPON_FREQUENCY_SEMIANNUAL
    )
    return rates.BondTerms(
        issue_date=str(value["issue_date"]),
        maturity_date=str(value["maturity_date"]),
        frequency=frequency,
        coupon_rate=_decimal(
            str(value.get("coupon_rate", value.get("coupon_rate_decimal"))), UNITS.rate
        ),
        face_amount=_decimal(
            str(value.get("face_value", "100.000000000000")),
            UNITS.currency_amount,
        ),
    )


def _calendar(value: dict[str, object]) -> rates.CalendarBinding:
    return rates.CalendarBinding(
        calendar_id=str(value["id"]),
        version=1,
        content_hash=_hash(f"calendar-{value['id']}"),
        coverage_start=str(value["coverage_start"]),
        coverage_end=str(value["coverage_end"]),
        non_business_days=[str(item) for item in value["non_business_days"]],
        work_weekends=[str(item) for item in value["work_weekends"]],
    )


def _curve(value: dict[str, object]) -> rates.YieldCurveBinding:
    return rates.YieldCurveBinding(
        curve_snapshot=_object("Q"),
        valuation_date=str(value["valuation_date"]),
        interpolation=rates.YIELD_CURVE_INTERPOLATION_LINEAR_YIELD,
        nodes=[
            rates.YieldCurveNode(
                maturity_date=str(node["maturity_date"]),
                yield_to_maturity=_decimal(
                    str(node["yield_to_maturity"]), UNITS.rate
                ),
            )
            for node in value["curve"]["nodes"]
        ],
    )


def _load(relative: str) -> dict[str, object]:
    return json.loads((GOLDEN_ROOT / relative).read_text(encoding="utf-8"))


def _assert_decimal_fields(
    message: object, expected: dict[str, object], tolerance: Decimal = Decimal(0)
) -> None:
    for field, value in expected.items():
        if isinstance(value, str):
            difference = abs(_decimal_value(getattr(message, field)) - Decimal(value))
            assert difference <= tolerance, field
        else:
            assert getattr(message, field) == value, field


def _bond_request() -> rates.AnalyzeBondRequest:
    fixture = _load("fixtures/bond-260008.IB.json")
    calendar = {
        "id": fixture["calendar"],
        "coverage_start": "2005-01-01",
        "coverage_end": "2026-12-31",
        "non_business_days": [],
        "work_weekends": [],
    }
    return rates.AnalyzeBondRequest(
        context=_context(
            "ficant.cgb.fixed-rate.reference",
            "cgb-reference-v1",
            rule_suffix="K",
            snapshot_suffix="M",
        ),
        bond=_object("N"),
        valuation_at=_market_time(str(fixture["valuation_at"]), "2026-07-13"),
        settlement_date=str(fixture["settlement_date"]),
        calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
        calendar=_calendar(calendar),
        terms=_terms(fixture),
        yield_to_maturity=_decimal(
            str(fixture["synthetic_yield_decimal"]), UNITS.rate
        ),
    )


def _assert_bond(client: RatesClient) -> None:
    expected = _load("expected/cgb-reference-v1-expected.json")["results"][
        "260008.IB:YIELD_IN"
    ]
    result = client.analyze_bond(_bond_request())
    assert len(result.cashflows) == expected["cashflow_count"]
    for actual, wanted in zip(result.cashflows, expected["cashflows"], strict=True):
        assert actual.sequence == wanted["sequence"]
        assert actual.nominal_date == wanted["nominal_date"]
        assert actual.payment_date == wanted["payment_date"]
        for field in ("coupon", "principal", "total"):
            assert _decimal_value(getattr(actual, field)) == Decimal(wanted[field])
    _assert_decimal_fields(
        result.measures,
        {
            field: expected[field]
            for field in (
                "accrued_interest",
                "clean_price",
                "dirty_price",
                "yield_to_maturity",
                "macaulay_duration",
                "modified_duration",
                "convexity",
                "dv01",
            )
        },
    )
    assert result.metadata.schema_id == "ficant.bond-analytics.result.v1"


def _assert_curve_and_carry(client: RatesClient) -> None:
    source = _load("phase2b-curve-carry-inputs.json")
    expected = _load("expected/phase2b-curve-carry-v1-expected.json")
    curve = _curve(source)
    point = client.interpolate_yield_curve(
        rates.InterpolateYieldCurveRequest(
            context=_context(
                "ficant.cgb.ytm-curve.linear",
                "cfets-ytm-linear-v1",
                rule_suffix="R",
                snapshot_suffix="S",
            ),
            curve=curve,
            query_date="2027-04-11",
        )
    )
    assert point.query_date == "2027-04-11"
    assert _decimal_value(point.yield_to_maturity) == Decimal(
        expected["curve_results"]["CURVE-EXACT-MIDPOINT"]["yield_to_maturity"]
    )

    carry_case = source["carry_cases"][0]
    carry = client.analyze_carry_roll(
        rates.AnalyzeCarryRollRequest(
            context=_context(
                "ficant.cgb.carry-roll.unfunded",
                "cfets-ytm-carry-roll-v1",
                rule_suffix="T",
                snapshot_suffix="V",
            ),
            bond=_object("W"),
            valuation_at=_market_time("2026-07-19T15:00:00+08:00", "2026-07-19"),
            initial_settlement=carry_case["initial_settlement"],
            horizon_settlement=carry_case["horizon_settlement"],
            calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
            calendar=_calendar(source["calendar"]),
            terms=_terms(carry_case["bond"]),
            curve=curve,
        )
    )
    carry_expected = expected["carry_results"]["CARRY-COUPON-UPWARD"]
    _assert_decimal_fields(
        carry.measures,
        {
            field: carry_expected[field]
            for field in ("initial_yield", "rolled_yield")
        },
        Decimal("0.000000000002"),
    )
    _assert_decimal_fields(
        carry.measures,
        {
            field: carry_expected[field]
            for field in (
                "initial_dirty_price",
                "horizon_dirty_at_initial_yield",
                "horizon_dirty_at_rolled_yield",
                "paid_cashflows",
                "carry",
                "roll_down",
                "total_return",
            )
        },
        Decimal("0.000000010000"),
    )


def _assert_futures_delivery(client: RatesClient) -> None:
    source = _load("phase2c-futures-delivery-inputs.json")
    expected = _load("expected/phase2c-futures-delivery-v1-expected.json")
    candidates = source["t_basket"]
    result = client.analyze_futures_delivery(
        rates.AnalyzeFuturesDeliveryRequest(
            context=_context(
                "ficant.cffex.cgb-futures-delivery",
                "cffex-cgb-futures-delivery-v1",
                rule_suffix="X",
                snapshot_suffix="Y",
            ),
            futures_contract=_object("Z"),
            valuation_at=_market_time("2026-07-20T15:00:00+08:00", "2026-07-20"),
            purchase_date=source["purchase_date"],
            delivery_month_first=source["delivery_month_first"],
            delivery_date=source["delivery_date"],
            product=rates.CGB_FUTURES_PRODUCT_T,
            futures_clean_price=_decimal(source["futures_clean_price"], UNITS.price_per_100),
            financing_rate=_decimal(source["financing_rate"], UNITS.rate),
            candidates=[
                rates.FuturesDeliverableCandidate(
                    bond=_object(suffix),
                    terms=_terms(candidate),
                    spot_clean_price=_decimal(
                        candidate["spot_clean_price"], UNITS.price_per_100
                    ),
                )
                for candidate, suffix in zip(candidates, ("2", "3", "4"), strict=True)
            ],
        )
    )
    assert result.ctd_index == 1
    for actual, candidate in zip(result.candidates, candidates, strict=True):
        _assert_decimal_fields(
            actual.measures, expected["basket_results"][candidate["bond_id"]]
        )


def _assert_futures_hedge(client: RatesClient) -> None:
    source = _load("phase2d-futures-hedge-inputs.json")
    expected = _load("expected/phase2d-futures-hedge-v1-expected.json")
    case = source["cases"][0]
    result = client.analyze_futures_hedge(
        rates.AnalyzeFuturesHedgeRequest(
            context=_context(
                "ficant.cffex.cgb-futures-dv01-hedge",
                "cffex-cgb-futures-dv01-hedge-v1",
                rule_suffix="5",
                snapshot_suffix="6",
            ),
            target_risk_artifact=_object("7"),
            delivery_artifact=_object("8"),
            ctd_analytics_artifact=_object("9"),
            futures_contract=_object("P"),
            ctd_bond=_object("R"),
            valuation_at=_market_time("2026-07-20T15:00:00+08:00", "2026-07-20"),
            product=rates.CGB_FUTURES_PRODUCT_TS,
            target_dv01=_decimal(case["target_dv01"], UNITS.dv01),
            ctd_dv01_per_100=_decimal(case["ctd_dv01_per_100"], UNITS.dv01_per_100),
            conversion_factor=_decimal(case["conversion_factor"], UNITS.dimensionless),
        )
    )
    _assert_decimal_fields(
        result.measures,
        expected["case_results"]["TS-long-risk-rounds-short-nine"],
    )


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def _wait_for_server(process: subprocess.Popen[bytes], port: int) -> None:
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if process.poll() is not None:
            _, stderr = process.communicate(timeout=1)
            raise AssertionError(
                f"ficant-server exited before readiness: {stderr.decode(errors='replace')}"
            )
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                return
        except OSError:
            time.sleep(0.05)
    raise AssertionError("ficant-server did not become ready within 15 seconds")


def _wait_for_endpoint(endpoint: str) -> None:
    host, raw_port = endpoint.rsplit(":", 1)
    port = int(raw_port)
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((host, port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.05)
    raise AssertionError(f"ficant-server endpoint did not become ready: {endpoint}")


def _assert_platform_service_still_routes(endpoint: str) -> None:
    host, raw_port = endpoint.rsplit(":", 1)
    connection = http.client.HTTPConnection(host, int(raw_port), timeout=5)
    try:
        connection.request(
            "POST",
            "/ficant.app.v1.PlatformService/GetCurrentSession",
            body=b"\x00\x00\x00\x00\x00",
            headers={
                "Content-Type": "application/grpc-web+proto",
                "X-Grpc-Web": "1",
                "Origin": "http://127.0.0.1:4174",
            },
        )
        response = connection.getresponse()
        assert response.status == 200
        assert response.getheader("access-control-allow-origin") == (
            "http://127.0.0.1:4174"
        )
        response.read()
    finally:
        connection.close()


@pytest.mark.skipif(
    "FICANT_PHASE2E_SERVER_BIN" not in os.environ
    and "FICANT_PHASE2E_ENDPOINT" not in os.environ,
    reason="live Phase 2E candidate binary or endpoint was not provided",
)
def test_python_sdk_matches_all_phase2_reference_slices_through_live_server() -> None:
    external_endpoint = os.environ.get("FICANT_PHASE2E_ENDPOINT")
    if external_endpoint is not None:
        _wait_for_endpoint(external_endpoint)
        _assert_platform_service_still_routes(external_endpoint)
        with RatesClient(external_endpoint, TOKEN, insecure=True) as client:
            _assert_bond(client)
            _assert_curve_and_carry(client)
            _assert_futures_delivery(client)
            _assert_futures_hedge(client)
        return

    server_bin = Path(os.environ["FICANT_PHASE2E_SERVER_BIN"]).resolve(strict=True)
    port = _free_port()
    environment = os.environ.copy()
    environment.update(
        {
            "FICANT_GRPC_BIND": f"127.0.0.1:{port}",
            "FICANT_GRPC_WEB_ALLOWED_ORIGINS": "http://127.0.0.1:4174",
            "FICANT_PLATFORM_SIGNING_KEY_HEX": KEY,
            "FICANT_PLATFORM_TRACE_KEY_HEX": KEY,
            "FICANT_BOOTSTRAP_SUBJECT": "phase2e-sdk-test",
            "FICANT_BOOTSTRAP_BEARER_TOKEN": TOKEN,
            "FICANT_BOOTSTRAP_SCOPES": "rates:analyze",
        }
    )
    process = subprocess.Popen(
        [str(server_bin)],
        cwd=REPO_ROOT,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        _wait_for_server(process, port)
        _assert_platform_service_still_routes(f"127.0.0.1:{port}")
        with RatesClient(f"127.0.0.1:{port}", TOKEN, insecure=True) as client:
            _assert_bond(client)
            _assert_curve_and_carry(client)
            _assert_futures_delivery(client)
            _assert_futures_hedge(client)
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
