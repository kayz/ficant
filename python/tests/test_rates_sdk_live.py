"""Live end-to-end Python SDK verification against freshly built ficant-server."""

from __future__ import annotations

from collections.abc import Iterator
from datetime import date, datetime, timedelta, timezone
from decimal import Decimal
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time
from types import ModuleType
from typing import Any

from google.protobuf.any_pb2 import Any as AnyPb
from google.protobuf.timestamp_pb2 import Timestamp
import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(
    0,
    str(
        REPO_ROOT
        / "python"
        / "node-contracts"
        / "src"
        / "ficant_contracts"
        / "generated"
    ),
)
sys.path.insert(0, str(REPO_ROOT / "python" / "sdk" / "src"))

from ficant.core.v1 import common_pb2, error_pb2, subject_pb2  # noqa: E402
from ficant.market.v1 import (  # noqa: E402
    cgb_futures_rule_pb2,
    definition_pb2,
    rule_pb2,
    tax_rule_pb2,
)
from ficant.rates.v1 import analytics_pb2 as rates  # noqa: E402
from ficant_sdk import RatesClient, RatesError  # noqa: E402

ULID_PREFIX = "01ARZ3NDEKTSV4RRFFQ69G5FA"
SCALED_FACTOR = Decimal("1000000000000")

HEDGE_V2_INPUTS_PATH = (
    REPO_ROOT / "tests/golden-cases/china-rates/bp1-futures-hedge-v2-inputs.json"
)
HEDGE_V2_EXPECTED_PATH = (
    REPO_ROOT
    / "tests/golden-cases/china-rates/expected/bp1-futures-hedge-v2-expected.json"
)
POLICY_BANK_INPUTS_PATH = (
    REPO_ROOT / "tests/golden-cases/china-rates/bp1-policy-bank-pretax-inputs.json"
)
POLICY_BANK_EXPECTED_PATH = (
    REPO_ROOT
    / "tests/golden-cases/china-rates/expected/bp1-policy-bank-pretax-expected.json"
)
R5E_INPUTS_PATH = (
    REPO_ROOT / "tests/golden-cases/china-rates/r5e-tax-adjusted-analytics-inputs.json"
)
R5E_EXPECTED_PATH = (
    REPO_ROOT
    / "tests/golden-cases/china-rates/expected/r5e-tax-adjusted-analytics-expected.json"
)
FUTURES_ORACLE_PATH = (
    REPO_ROOT / "tests/oracle/china-rates/bp1_futures_hedge_v2_decimal_oracle.py"
)
POLICY_BANK_ORACLE_PATH = (
    REPO_ROOT / "tests/oracle/china-rates/bp1_policy_bank_pretax_decimal_oracle.py"
)


def _load_oracle_by_path(module_name: str, file_path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(module_name, file_path)
    assert spec is not None and spec.loader is not None, f"cannot load {file_path}"
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


def _ulid(suffix: str) -> common_pb2.Ulid:
    return common_pb2.Ulid(value=f"{ULID_PREFIX}{suffix}")


def _unit(suffix: str, version: int) -> common_pb2.UnitRef:
    return common_pb2.UnitRef(unit_id=_ulid(suffix), version=version)


# Every one of the 9 AnalysisUnits has a distinct (unit_id, version) AND distinct version.
RATE_UNIT = common_pb2.UnitRef(
    unit_id=common_pb2.Ulid(value="01K2CGBVAT0000000000000000"), version=1
)

UNITS = rates.AnalysisUnits(
    currency_amount=_unit("A", 2),
    price_per_100=_unit("B", 3),
    rate=RATE_UNIT,
    years=_unit("D", 4),
    years_squared=_unit("E", 5),
    dv01_per_100=_unit("F", 6),
    dv01=_unit("G", 7),
    dimensionless=_unit("H", 8),
    contract_count=_unit("J", 9),
)


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


def _to_dec(v: common_pb2.DecimalValue) -> Decimal:
    return Decimal(v.coefficient).scaleb(-v.scale)


def _to_scaled(value: Decimal) -> int:
    scaled = value * SCALED_FACTOR
    assert scaled == scaled.to_integral_value(), f"non-12dp decimal: {value}"
    return int(scaled)


def _assert_unit_eq(actual: common_pb2.UnitRef, expected: common_pb2.UnitRef) -> None:
    assert actual.HasField("unit_id")
    assert actual.unit_id.value == expected.unit_id.value
    assert actual.version == expected.version


def _market_time(iso_utc: str, local_date: str) -> common_pb2.MarketTime:
    dt = datetime.fromisoformat(iso_utc).astimezone(timezone.utc)
    ts = Timestamp()
    ts.FromDatetime(dt)
    return common_pb2.MarketTime(
        instant=ts, market_timezone="Asia/Shanghai", local_trading_date=local_date
    )


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def _server_binary() -> Path:
    exe = "ficant-server.exe" if sys.platform == "win32" else "ficant-server"
    path = REPO_ROOT / "target" / "debug" / exe
    # Always invoke incremental cargo build so live tests run the current candidate binary.
    subprocess.run(
        ["cargo", "build", "--locked", "-p", "ficant-server"],
        cwd=REPO_ROOT,
        check=True,
    )
    assert path.exists(), f"expected built server binary at {path}"
    return path


@pytest.fixture(scope="module")
def live_client() -> Iterator[RatesClient]:
    port = _free_port()
    addr = f"127.0.0.1:{port}"

    proc = subprocess.Popen(
        [str(_server_binary())],
        cwd=REPO_ROOT,
        env={**os.environ, "FICANT_SERVER_ADDR": addr},
    )
    deadline = time.monotonic() + 10.0
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                break
        except OSError:
            time.sleep(0.05)
    else:
        proc.kill()
        raise RuntimeError("ficant-server did not start in time")

    client = RatesClient(addr, insecure=True)
    try:
        yield client
    finally:
        client.close()
        proc.terminate()
        proc.wait(timeout=5)


def _load_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


POLICY_BANK_INPUTS = _load_json(POLICY_BANK_INPUTS_PATH)
POLICY_BANK_EXPECTED = _load_json(POLICY_BANK_EXPECTED_PATH)
POLICY_BANK_BONDS_BY_ID: dict[str, dict[str, Any]] = {
    bond["case_id"]: bond for bond in POLICY_BANK_INPUTS["bonds"]
}
POLICY_BANK_TWELVE_CASES: list[tuple[str, str]] = [
    (bond["case_id"], mode)
    for bond in POLICY_BANK_INPUTS["bonds"]
    for mode in ("YIELD_IN", "PRICE_IN")
]


def _build_cfets_calendar(
    calendar_doc: dict[str, Any], *, end_date_override: str | None = None
) -> definition_pb2.Calendar:
    start = date.fromisoformat(calendar_doc["coverage_start"])
    end = date.fromisoformat(end_date_override or calendar_doc["coverage_end"])

    holidays_closed: set[date] = set()
    work_weekends_open: set[date] = set()
    for notice in calendar_doc["sources"]:
        for range_start_s, range_end_s in notice["holidays_closed"]:
            cur = date.fromisoformat(range_start_s)
            last = date.fromisoformat(range_end_s)
            while cur <= last:
                holidays_closed.add(cur)
                cur += timedelta(days=1)
        for open_s in notice["work_weekends_open"]:
            work_weekends_open.add(date.fromisoformat(open_s))

    sessions: list[definition_pb2.CalendarSession] = []
    cur = start
    while cur <= end:
        if cur in holidays_closed:
            closed = True
        elif cur in work_weekends_open:
            closed = False
        else:
            closed = cur.weekday() >= 5
        sessions.append(
            definition_pb2.CalendarSession(
                local_date=cur.isoformat(),
                open_local_time="" if closed else "09:00:00",
                close_local_time="" if closed else "17:00:00",
                closed=closed,
            )
        )
        cur += timedelta(days=1)

    return definition_pb2.Calendar(
        calendar_id=common_pb2.Ulid(value=calendar_doc["calendar_id"]),
        version=int(calendar_doc["version"]),
        market=calendar_doc["market"],
        market_timezone=calendar_doc["market_timezone"],
        effective_from=_market_time("2023-12-31T16:00:00Z", "2024-01-01"),
        effective_to=_market_time("2026-12-31T16:00:00Z", "2027-01-01"),
        sessions=sessions,
    )


FULL_CFETS_CALENDAR = _build_cfets_calendar(POLICY_BANK_INPUTS["calendar"])


def _policy_bank_bond_proto(
    bond_doc: dict[str, Any],
    *,
    tax_attributes: definition_pb2.BondTaxAttributes | None = None,
) -> definition_pb2.Bond:
    freq_map = {
        "ANNUAL": definition_pb2.BOND_COUPON_FREQUENCY_ANNUAL,
        "SEMIANNUAL": definition_pb2.BOND_COUPON_FREQUENCY_SEMIANNUAL,
    }
    dc_map = {
        "ACT_ACT_BOND_ISMA": definition_pb2.BOND_DAY_COUNT_CONVENTION_ACT_ACT_BOND_ISMA,
    }
    bdc_map = {
        "FOLLOWING": definition_pb2.BOND_BUSINESS_DAY_CONVENTION_FOLLOWING,
    }
    kwargs: dict[str, Any] = {
        "instrument": common_pb2.VersionRef(id=_ulid("K"), version=1),
        "first_issue_date": bond_doc["first_issue_date"],
        "current_issue_date": bond_doc["current_issue_date"],
        "maturity_date": bond_doc["maturity_date"],
        "face_value": _dec(bond_doc["face_value"], UNITS.price_per_100),
        "cumulative_issued_amount": _dec(
            bond_doc["cumulative_issued_amount"], UNITS.currency_amount
        ),
        "coupon_rate": _dec(bond_doc["coupon_rate"], UNITS.rate),
        "coupon_frequency": freq_map[bond_doc["coupon_frequency"]],
        "day_count": dc_map[bond_doc["day_count"]],
        "business_day": bdc_map[bond_doc["business_day"]],
    }
    if tax_attributes is not None:
        kwargs["tax_attributes"] = tax_attributes
    return definition_pb2.Bond(**kwargs)


def _calendar_requirement_proto(label: str) -> int:
    mapping = {
        "REFERENCE_REPLAY": rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
        "EXACT_MARKET": rates.CALENDAR_REQUIREMENT_EXACT_MARKET,
    }
    return mapping[label]


def _sample_bond(
    first_issue: str = "2025-08-08",
    *,
    current_issue: str | None = None,
    maturity: str = "2035-08-08",
    taxable: bool = True,
    include_tax_attributes: bool = True,
) -> definition_pb2.Bond:
    kwargs: dict[str, Any] = {
        "instrument": common_pb2.VersionRef(id=_ulid("1"), version=1),
        "first_issue_date": first_issue,
        "current_issue_date": current_issue or first_issue,
        "maturity_date": maturity,
        "face_value": _dec("100", UNITS.price_per_100),
        "cumulative_issued_amount": _dec("10000000000", UNITS.currency_amount),
        "coupon_rate": _dec("0.025", UNITS.rate),
        "coupon_frequency": definition_pb2.BOND_COUPON_FREQUENCY_SEMIANNUAL,
        "day_count": definition_pb2.BOND_DAY_COUNT_CONVENTION_ACT_ACT_BOND_ISMA,
        "business_day": definition_pb2.BOND_BUSINESS_DAY_CONVENTION_FOLLOWING,
    }
    if include_tax_attributes:
        vat = (
            definition_pb2.VALUE_ADDED_TAX_STATUS_TAXABLE
            if taxable
            else definition_pb2.VALUE_ADDED_TAX_STATUS_EXEMPT
        )
        kwargs["tax_attributes"] = definition_pb2.BondTaxAttributes(
            value_added_tax_status=vat,
            income_tax_status=definition_pb2.INCOME_TAX_STATUS_EXEMPT,
        )
    return definition_pb2.Bond(**kwargs)


def _tax_pack(*, content_hash_override: bytes | None = None) -> rule_pb2.MarketRulePack:
    raw = (
        REPO_ROOT / "domain-packs" / "cgb-interest-tax" / "cgb-interest-tax-v1.bin"
    ).read_bytes()
    digest = (
        content_hash_override
        if content_hash_override is not None
        else hashlib.sha256(raw).digest()
    )
    return rule_pb2.MarketRulePack(
        rule_pack_id=_ulid("T"),
        version=1,
        market="CN",
        rule_type="tax",
        source="ficant-authority/cgb-interest-tax/v1",
        effective_from=_market_time("2025-12-31T16:00:00Z", "2026-01-01"),
        effective_to=_market_time("2027-12-31T16:00:00Z", "2028-01-01"),
        verification_status=rule_pb2.VERIFICATION_STATUS_VERIFIED,
        content_hash=common_pb2.Sha256(value=digest),
        content=AnyPb(
            type_url="type.googleapis.com/ficant.market.v1.TaxRulePackV2", value=raw
        ),
    )


def _delivery_pack_from_bytes(
    raw: bytes,
    *,
    version: int = 2,
    include_content_hash: bool = True,
    content_hash_override: bytes | None = None,
) -> rule_pb2.MarketRulePack:
    kwargs: dict[str, Any] = {
        "rule_pack_id": _ulid("F"),
        "version": version,
        "market": "CFFEX",
        "rule_type": "cgb-futures",
        "source": "cffex-rulebook",
        "effective_from": _market_time("2025-12-31T16:00:00Z", "2026-01-01"),
        "effective_to": _market_time("2027-12-31T16:00:00Z", "2028-01-01"),
        "verification_status": rule_pb2.VERIFICATION_STATUS_VERIFIED,
        "content": AnyPb(
            type_url="type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack",
            value=raw,
        ),
    }
    if include_content_hash:
        digest = (
            content_hash_override
            if content_hash_override is not None
            else hashlib.sha256(raw).digest()
        )
        kwargs["content_hash"] = common_pb2.Sha256(value=digest)
    return rule_pb2.MarketRulePack(**kwargs)


def _delivery_pack_v1() -> rule_pb2.MarketRulePack:
    raw = (
        REPO_ROOT / "domain-packs" / "cgb-futures" / "cgb-futures-v1.bin"
    ).read_bytes()
    return _delivery_pack_from_bytes(raw, version=1)


def _delivery_pack_v2(
    *,
    include_content_hash: bool = True,
    content_hash_override: bytes | None = None,
) -> rule_pb2.MarketRulePack:
    raw = (
        REPO_ROOT / "domain-packs" / "cgb-futures" / "cgb-futures-v2.bin"
    ).read_bytes()
    return _delivery_pack_from_bytes(
        raw,
        version=2,
        include_content_hash=include_content_hash,
        content_hash_override=content_hash_override,
    )


def _r5e_tax_context(
    *,
    vat_profile: str = "cn-vat-general-taxpayer",
    cit_profile: str = "cn-cgb-interest-cit-exempt",
    pack: rule_pb2.MarketRulePack | None = None,
) -> rates.BondTaxContext:
    return rates.BondTaxContext(
        subject_tax_treatment=subject_pb2.TaxTreatment(
            value_added_tax_profile=vat_profile,
            income_tax_profile=cit_profile,
        ),
        tax_rule_pack=pack if pack is not None else _tax_pack(),
    )


def test_distinct_analysis_units_and_independent_oracles_match_frozen_expected() -> None:
    all_units = [
        UNITS.currency_amount,
        UNITS.price_per_100,
        UNITS.rate,
        UNITS.years,
        UNITS.years_squared,
        UNITS.dv01_per_100,
        UNITS.dv01,
        UNITS.dimensionless,
        UNITS.contract_count,
    ]
    assert len({u.unit_id.value for u in all_units}) == 9
    assert len({u.version for u in all_units}) == 9
    assert len({(u.unit_id.value, u.version) for u in all_units}) == 9

    futures_oracle = _load_oracle_by_path(
        "ficant_live_bp1_futures_oracle", FUTURES_ORACLE_PATH
    )
    bond_oracle = _load_oracle_by_path(
        "ficant_live_bp1_policy_bank_oracle", POLICY_BANK_ORACLE_PATH
    )

    hedge_inputs = _load_json(HEDGE_V2_INPUTS_PATH)
    hedge_expected = _load_json(HEDGE_V2_EXPECTED_PATH)
    assert futures_oracle.build_expected(hedge_inputs, repo_root=REPO_ROOT) == hedge_expected

    assert bond_oracle.build_expected(POLICY_BANK_INPUTS) == POLICY_BANK_EXPECTED

    # Verify exact 12-key Cartesian product and coverage bounds for all 6 bonds.
    expected_12_keys = {
        f"{bond_id}:{mode}"
        for bond_id in (
            "CDB-10Y-ANNUAL",
            "CDB-10Y-SEMIANNUAL",
            "EIBC-5Y-ANNUAL",
            "EIBC-3Y-SEMIANNUAL",
            "ADBC-1Y-EXACT-ANNUAL",
            "ADBC-7Y-SEMIANNUAL",
        )
        for mode in ("YIELD_IN", "PRICE_IN")
    }
    param_keys = [f"{b}:{m}" for b, m in POLICY_BANK_TWELVE_CASES]
    assert len(param_keys) == 12
    assert len(set(param_keys)) == 12
    assert set(param_keys) == expected_12_keys
    assert set(POLICY_BANK_EXPECTED["case_results"].keys()) == expected_12_keys

    for bond in POLICY_BANK_INPUTS["bonds"]:
        assert "2024-01-01" <= bond["first_issue_date"] <= "2026-12-31"

    assert FULL_CFETS_CALENDAR.sessions[0].local_date == "2024-01-01"
    assert FULL_CFETS_CALENDAR.sessions[0].closed is True
    assert FULL_CFETS_CALENDAR.sessions[-1].local_date == "2026-12-31"
    assert FULL_CFETS_CALENDAR.sessions[-1].closed is False


@pytest.mark.parametrize(
    ("bond_id", "mode"),
    POLICY_BANK_TWELVE_CASES,
    ids=[f"{bond_id}:{mode}" for bond_id, mode in POLICY_BANK_TWELVE_CASES],
)
def test_live_policy_bank_pretax_twelve_cases(
    live_client: RatesClient, bond_id: str, mode: str
) -> None:
    bond_doc = POLICY_BANK_BONDS_BY_ID[bond_id]
    exp = POLICY_BANK_EXPECTED["case_results"][f"{bond_id}:{mode}"]

    req_kwargs: dict[str, Any] = {
        "units": UNITS,
        "bond": _policy_bank_bond_proto(bond_doc),
        "settlement_date": bond_doc["settlement_date"],
        "calendar_requirement": _calendar_requirement_proto(
            bond_doc["calendar_requirement"]
        ),
        "calendar": FULL_CFETS_CALENDAR,
        "execution_mode": rates.BOND_TAX_BASIS_PRE_TAX,
    }
    if mode == "YIELD_IN":
        req_kwargs["yield_to_maturity"] = _dec(bond_doc["yield_in_input"], UNITS.rate)
    else:
        req_kwargs["clean_price"] = _dec(bond_doc["price_in_input"], UNITS.price_per_100)

    res = live_client.analyze_bond(rates.AnalyzeBondRequest(**req_kwargs))

    assert res.execution_mode == rates.BOND_TAX_BASIS_PRE_TAX
    assert res.measures_tax_basis == rates.BOND_TAX_BASIS_PRE_TAX
    assert not res.HasField("after_tax")
    assert res.HasField("measures")

    # Cashflows: exact count, order, dates, 0-diff amounts, and currency_amount UnitRef.
    exp_cfs = exp["cashflows"]
    assert len(res.cashflows) == len(exp_cfs)
    for actual_cf, exp_cf in zip(res.cashflows, exp_cfs, strict=True):
        assert actual_cf.sequence == exp_cf["sequence"]
        assert actual_cf.nominal_date == exp_cf["nominal_date"]
        assert actual_cf.payment_date == exp_cf["payment_date"]
        for amount_field in ("coupon", "principal", "total"):
            assert actual_cf.HasField(amount_field)
            actual_val = getattr(actual_cf, amount_field)
            assert _to_dec(actual_val) == Decimal(exp_cf[amount_field])
            _assert_unit_eq(actual_val.unit, UNITS.currency_amount)

    # Explicit holiday/maturity payment date verifications.
    if bond_id == "EIBC-3Y-SEMIANNUAL":
        assert res.cashflows[0].nominal_date == "2026-10-01"
        assert res.cashflows[0].payment_date == "2026-10-08"
    if bond_id == "ADBC-1Y-EXACT-ANNUAL":
        assert len(res.cashflows) == 1
        assert res.cashflows[0].nominal_date == "2026-11-15"
        assert res.cashflows[0].payment_date == "2026-11-16"

    # All 8 measures: AC-008 frozen scaled tolerances (1 scaled = 10^-12) + exact UnitRefs.
    exp_m = exp["measures"]
    measure_specs: list[tuple[str, common_pb2.UnitRef, str]] = [
        ("accrued_interest", UNITS.price_per_100, "price_or_dv01"),
        ("clean_price", UNITS.price_per_100, "price_or_dv01"),
        ("dirty_price", UNITS.price_per_100, "price_or_dv01"),
        ("yield_to_maturity", UNITS.rate, "ytm"),
        ("macaulay_duration", UNITS.years, "risk"),
        ("modified_duration", UNITS.years, "risk"),
        ("convexity", UNITS.years_squared, "risk"),
        ("dv01", UNITS.dv01_per_100, "price_or_dv01"),
    ]
    for field_name, expected_unit, tol_kind in measure_specs:
        assert res.measures.HasField(field_name)
        actual_pb = getattr(res.measures, field_name)
        _assert_unit_eq(actual_pb.unit, expected_unit)
        actual_scaled = _to_scaled(_to_dec(actual_pb))
        exp_scaled = _to_scaled(Decimal(exp_m[field_name]))
        if tol_kind == "price_or_dv01":
            tol = 10_000
        elif tol_kind == "ytm":
            tol = 100
        else:
            tol = max(100, (abs(exp_scaled) + 99_999_999) // 100_000_000)
        assert abs(actual_scaled - exp_scaled) <= tol, (
            f"{bond_id}:{mode}.{field_name} diff={abs(actual_scaled - exp_scaled)} > {tol}"
        )


def test_live_adbc_1y_exact_market_rejects_missing_empty_and_truncated_calendar(
    live_client: RatesClient,
) -> None:
    short_doc = POLICY_BANK_BONDS_BY_ID["ADBC-1Y-EXACT-ANNUAL"]
    base_kwargs: dict[str, Any] = {
        "units": UNITS,
        "bond": _policy_bank_bond_proto(short_doc),
        "settlement_date": short_doc["settlement_date"],
        "calendar_requirement": rates.CALENDAR_REQUIREMENT_EXACT_MARKET,
        "execution_mode": rates.BOND_TAX_BASIS_PRE_TAX,
        "yield_to_maturity": _dec(short_doc["yield_in_input"], UNITS.rate),
    }

    # 1. Missing Calendar
    with pytest.raises(RatesError) as exc_missing:
        live_client.analyze_bond(rates.AnalyzeBondRequest(**base_kwargs))
    assert exc_missing.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    # 2. Empty sessions
    empty_cal = definition_pb2.Calendar(
        calendar_id=common_pb2.Ulid(
            value=POLICY_BANK_INPUTS["calendar"]["calendar_id"]
        ),
        version=1,
        market="CFETS",
        market_timezone="Asia/Shanghai",
        effective_from=_market_time("2023-12-31T16:00:00Z", "2024-01-01"),
        effective_to=_market_time("2026-12-31T16:00:00Z", "2027-01-01"),
        sessions=[],
    )
    with pytest.raises(RatesError) as exc_empty:
        live_client.analyze_bond(
            rates.AnalyzeBondRequest(**base_kwargs, calendar=empty_cal)
        )
    assert exc_empty.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    # 3. Truncated to last_session = 2026-10-10
    truncated_cal = _build_cfets_calendar(
        POLICY_BANK_INPUTS["calendar"], end_date_override="2026-10-10"
    )
    assert truncated_cal.sessions[-1].local_date == "2026-10-10"
    assert truncated_cal.sessions[-1].closed is False
    with pytest.raises(RatesError) as exc_trunc:
        live_client.analyze_bond(
            rates.AnalyzeBondRequest(**base_kwargs, calendar=truncated_cal)
        )
    assert exc_trunc.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED


def test_live_adbc_7y_reissuance_pretax_invariant_across_tax_attributes(
    live_client: RatesClient,
) -> None:
    adbc7_doc = POLICY_BANK_BONDS_BY_ID["ADBC-7Y-SEMIANNUAL"]
    exempt_attrs = definition_pb2.BondTaxAttributes(
        value_added_tax_status=definition_pb2.VALUE_ADDED_TAX_STATUS_EXEMPT,
        income_tax_status=definition_pb2.INCOME_TAX_STATUS_EXEMPT,
    )
    taxable_attrs = definition_pb2.BondTaxAttributes(
        value_added_tax_status=definition_pb2.VALUE_ADDED_TAX_STATUS_TAXABLE,
        income_tax_status=definition_pb2.INCOME_TAX_STATUS_EXEMPT,
    )

    def _run(attrs: definition_pb2.BondTaxAttributes | None) -> rates.AnalyzeBondResult:
        return live_client.analyze_bond(
            rates.AnalyzeBondRequest(
                units=UNITS,
                bond=_policy_bank_bond_proto(adbc7_doc, tax_attributes=attrs),
                settlement_date=adbc7_doc["settlement_date"],
                calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
                calendar=FULL_CFETS_CALENDAR,
                execution_mode=rates.BOND_TAX_BASIS_PRE_TAX,
                yield_to_maturity=_dec(adbc7_doc["yield_in_input"], UNITS.rate),
            )
        )

    res_exempt = _run(exempt_attrs)
    res_taxable = _run(taxable_attrs)
    res_absent = _run(None)

    assert res_exempt.execution_mode == rates.BOND_TAX_BASIS_PRE_TAX
    assert res_exempt.measures_tax_basis == rates.BOND_TAX_BASIS_PRE_TAX
    assert not res_exempt.HasField("after_tax")
    assert res_exempt == res_taxable
    assert res_exempt == res_absent


def test_live_r5e_tax_adjusted_analytics_and_mismatch_boundaries(
    live_client: RatesClient,
) -> None:
    r5e_inputs = _load_json(R5E_INPUTS_PATH)
    r5e_expected = _load_json(R5E_EXPECTED_PATH)

    for case_in, case_exp in zip(
        r5e_inputs["bond_cases"], r5e_expected["bond_cases"], strict=True
    ):
        case_id = case_in["case_id"]
        taxable = case_in["value_added_tax_status"] == "TAXABLE"
        # Authoritative R5E 17-remaining-period semiannual schedule (settlement 2026-07-21,
        # maturity 2034-08-08; before-cutoff-exempt aligns nominal Feb 8 / Aug 8 dates).
        if case_id == "before-cutoff-exempt":
            first_issue, current_issue = "2025-02-08", "2025-08-08"
        else:
            first_issue = case_in["first_issue_date"]
            current_issue = case_in["current_issue_date"]

        bond_pb = _sample_bond(
            first_issue,
            current_issue=current_issue,
            maturity="2034-08-08",
            taxable=taxable,
            include_tax_attributes=True,
        )
        tax_res = live_client.analyze_bond(
            rates.AnalyzeBondRequest(
                units=UNITS,
                bond=bond_pb,
                settlement_date="2026-07-21",
                calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
                clean_price=_dec("100", UNITS.price_per_100),
                execution_mode=rates.BOND_TAX_BASIS_TAX_ADJUSTED,
                tax_context=_r5e_tax_context(),
            )
        )
        pre_only_res = live_client.analyze_bond(
            rates.AnalyzeBondRequest(
                units=UNITS,
                bond=bond_pb,
                settlement_date="2026-07-21",
                calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
                clean_price=_dec("100", UNITS.price_per_100),
                execution_mode=rates.BOND_TAX_BASIS_PRE_TAX,
            )
        )

        assert tax_res.execution_mode == rates.BOND_TAX_BASIS_TAX_ADJUSTED
        assert tax_res.measures_tax_basis == rates.BOND_TAX_BASIS_PRE_TAX
        assert tax_res.HasField("after_tax")
        assert tax_res.after_tax.tax_basis == rates.BOND_TAX_BASIS_TAX_ADJUSTED
        assert (
            tax_res.after_tax.claim_scope
            == tax_rule_pb2.COUPON_TAX_CLAIM_SCOPE_COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT
        )
        # Top-level measures (including dv01) are strictly PRE_TAX.
        assert tax_res.measures == pre_only_res.measures
        assert tax_res.measures.dv01 == pre_only_res.measures.dv01
        _assert_unit_eq(tax_res.measures.dv01.unit, UNITS.dv01_per_100)

        pre_ytm = _to_dec(tax_res.measures.yield_to_maturity)
        post_ytm = _to_dec(tax_res.after_tax.yield_to_maturity)
        assert pre_ytm == Decimal(case_exp["market_pre_tax_yield_to_maturity"])
        assert post_ytm == Decimal(case_exp["subject_tax_adjusted_yield_to_maturity"])
        if taxable:
            assert pre_ytm > post_ytm > Decimal(0)
        else:
            assert pre_ytm == post_ytm

    # Negative tax/subject mismatch boundaries -> ValidationFailed / HashMismatch.
    taxable_bond = _sample_bond("2025-08-08", maturity="2034-08-08", taxable=True)

    # 1. Missing bond.tax_attributes under TAX_ADJUSTED
    with pytest.raises(RatesError) as exc_no_attrs:
        live_client.analyze_bond(
            rates.AnalyzeBondRequest(
                units=UNITS,
                bond=_sample_bond(
                    "2025-08-08", maturity="2034-08-08", include_tax_attributes=False
                ),
                settlement_date="2026-07-21",
                calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
                clean_price=_dec("100", UNITS.price_per_100),
                execution_mode=rates.BOND_TAX_BASIS_TAX_ADJUSTED,
                tax_context=_r5e_tax_context(),
            )
        )
    assert exc_no_attrs.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    # 2. Bond tax_attributes mismatch vs first_issue_date cutoff (on-cutoff declared EXEMPT)
    with pytest.raises(RatesError) as exc_attr_mismatch:
        live_client.analyze_bond(
            rates.AnalyzeBondRequest(
                units=UNITS,
                bond=_sample_bond("2025-08-08", maturity="2034-08-08", taxable=False),
                settlement_date="2026-07-21",
                calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
                clean_price=_dec("100", UNITS.price_per_100),
                execution_mode=rates.BOND_TAX_BASIS_TAX_ADJUSTED,
                tax_context=_r5e_tax_context(),
            )
        )
    assert exc_attr_mismatch.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    # 3. Unknown subject tax profile mismatch
    with pytest.raises(RatesError) as exc_subj_mismatch:
        live_client.analyze_bond(
            rates.AnalyzeBondRequest(
                units=UNITS,
                bond=taxable_bond,
                settlement_date="2026-07-21",
                calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
                clean_price=_dec("100", UNITS.price_per_100),
                execution_mode=rates.BOND_TAX_BASIS_TAX_ADJUSTED,
                tax_context=_r5e_tax_context(vat_profile="cn-vat-unknown-profile"),
            )
        )
    assert exc_subj_mismatch.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    # 4. Explicit PRE_TAX with tax_context present -> ValidationFailed
    with pytest.raises(RatesError) as exc_pretax_ctx:
        live_client.analyze_bond(
            rates.AnalyzeBondRequest(
                units=UNITS,
                bond=taxable_bond,
                settlement_date="2026-07-21",
                calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
                clean_price=_dec("100", UNITS.price_per_100),
                execution_mode=rates.BOND_TAX_BASIS_PRE_TAX,
                tax_context=_r5e_tax_context(),
            )
        )
    assert exc_pretax_ctx.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    # 5. Explicit TAX_ADJUSTED without tax_context -> ValidationFailed
    with pytest.raises(RatesError) as exc_tax_no_ctx:
        live_client.analyze_bond(
            rates.AnalyzeBondRequest(
                units=UNITS,
                bond=taxable_bond,
                settlement_date="2026-07-21",
                calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
                clean_price=_dec("100", UNITS.price_per_100),
                execution_mode=rates.BOND_TAX_BASIS_TAX_ADJUSTED,
            )
        )
    assert exc_tax_no_ctx.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    # 6. Unknown non-zero execution_mode -> ValidationFailed
    with pytest.raises(RatesError) as exc_unknown_mode:
        live_client.analyze_bond(
            rates.AnalyzeBondRequest(
                units=UNITS,
                bond=taxable_bond,
                settlement_date="2026-07-21",
                calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
                clean_price=_dec("100", UNITS.price_per_100),
                execution_mode=99,
            )
        )
    assert exc_unknown_mode.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    # 7. TAX_ADJUSTED with mismatched tax pack digest -> HashMismatch
    with pytest.raises(RatesError) as exc_tax_hash:
        live_client.analyze_bond(
            rates.AnalyzeBondRequest(
                units=UNITS,
                bond=taxable_bond,
                settlement_date="2026-07-21",
                calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
                clean_price=_dec("100", UNITS.price_per_100),
                execution_mode=rates.BOND_TAX_BASIS_TAX_ADJUSTED,
                tax_context=_r5e_tax_context(pack=_tax_pack(content_hash_override=b"\xEE" * 32)),
            )
        )
    assert exc_tax_hash.value.detail.code == error_pb2.ERROR_CODE_HASH_MISMATCH


def test_live_legacy_requests_against_new_server(live_client: RatesClient) -> None:
    bond_pb = _sample_bond("2025-08-08", maturity="2034-08-08", taxable=True)

    # Legacy bond request (execution_mode=0, no tax_context) -> inferred PRE_TAX.
    legacy_pre = live_client.analyze_bond(
        rates.AnalyzeBondRequest(
            units=UNITS,
            bond=bond_pb,
            settlement_date="2026-07-21",
            calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
            clean_price=_dec("100", UNITS.price_per_100),
            execution_mode=rates.BOND_TAX_BASIS_UNSPECIFIED,
        )
    )
    assert legacy_pre.execution_mode == rates.BOND_TAX_BASIS_PRE_TAX
    assert legacy_pre.measures_tax_basis == rates.BOND_TAX_BASIS_PRE_TAX
    assert not legacy_pre.HasField("after_tax")

    # Legacy bond request (execution_mode=0, with tax_context) -> inferred TAX_ADJUSTED.
    legacy_tax = live_client.analyze_bond(
        rates.AnalyzeBondRequest(
            units=UNITS,
            bond=bond_pb,
            settlement_date="2026-07-21",
            calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
            clean_price=_dec("100", UNITS.price_per_100),
            execution_mode=rates.BOND_TAX_BASIS_UNSPECIFIED,
            tax_context=_r5e_tax_context(),
        )
    )
    assert legacy_tax.execution_mode == rates.BOND_TAX_BASIS_TAX_ADJUSTED
    assert legacy_tax.measures_tax_basis == rates.BOND_TAX_BASIS_PRE_TAX
    assert legacy_tax.HasField("after_tax")
    assert legacy_tax.after_tax.tax_basis == rates.BOND_TAX_BASIS_TAX_ADJUSTED

    # Legacy pack-less hedge request -> ValidationFailed on new server.
    with pytest.raises(RatesError) as exc_legacy_hedge:
        live_client.analyze_futures_hedge(
            rates.AnalyzeFuturesHedgeRequest(
                units=UNITS,
                product_code="TS",
                target_dv01=_dec("1234.50", UNITS.dv01),
                ctd_dv01_per_100=_dec("0.0145", UNITS.dv01_per_100),
                conversion_factor=_dec("0.9987", UNITS.dimensionless),
            )
        )
    assert exc_legacy_hedge.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED


def test_live_futures_hedge_v2_reconciliation_and_ts_size_mutation(
    live_client: RatesClient,
) -> None:
    hedge_inputs = _load_json(HEDGE_V2_INPUTS_PATH)
    hedge_expected = _load_json(HEDGE_V2_EXPECTED_PATH)
    pack_v2 = _delivery_pack_v2()

    # Reconcile all 6 authoritative cases (covering TS=2,000,000 and TF/T/TL=1,000,000).
    for case in hedge_inputs["cases"]:
        exp = hedge_expected["case_results"][case["id"]]
        res = live_client.analyze_futures_hedge(
            rates.AnalyzeFuturesHedgeRequest(
                units=UNITS,
                product_code=case["product"],
                target_dv01=_dec(case["target_dv01"], UNITS.dv01),
                ctd_dv01_per_100=_dec(case["ctd_dv01_per_100"], UNITS.dv01_per_100),
                conversion_factor=_dec(case["conversion_factor"], UNITS.dimensionless),
                delivery_rule_pack=pack_v2,
            )
        )
        m = res.measures
        assert m.recommended_contracts == exp["recommended_contracts"]
        assert _to_dec(m.contract_notional) == Decimal(exp["contract_notional"])
        _assert_unit_eq(m.contract_notional.unit, UNITS.currency_amount)

        for field_name, expected_unit in (
            ("futures_contract_dv01", UNITS.dv01),
            ("raw_contracts", UNITS.contract_count),
            ("residual_dv01", UNITS.dv01),
            ("hedge_effectiveness", UNITS.dimensionless),
        ):
            actual_pb = getattr(m, field_name)
            _assert_unit_eq(actual_pb.unit, expected_unit)
            diff = abs(_to_scaled(_to_dec(actual_pb)) - _to_scaled(Decimal(exp[field_name])))
            assert diff <= 10, f"{case['id']}.{field_name} diff={diff} > 10 scaled"

    # Explicit TS vs T contract_notional sanity check.
    ts_base = live_client.analyze_futures_hedge(
        rates.AnalyzeFuturesHedgeRequest(
            units=UNITS,
            product_code="TS",
            target_dv01=_dec("1234.50", UNITS.dv01),
            ctd_dv01_per_100=_dec("0.0145", UNITS.dv01_per_100),
            conversion_factor=_dec("0.9987", UNITS.dimensionless),
            delivery_rule_pack=pack_v2,
        )
    )
    assert _to_dec(ts_base.measures.contract_notional) == Decimal("2000000")
    assert _to_dec(ts_base.measures.futures_contract_dv01) == Decimal("290.377490737959")
    assert ts_base.measures.recommended_contracts == -4

    # In-memory TS contract_size_in_quote_units mutation 20,000 -> 10,000 with recomputed SHA-256.
    raw_v2 = (
        REPO_ROOT / "domain-packs" / "cgb-futures" / "cgb-futures-v2.bin"
    ).read_bytes()
    mutated_payload = cgb_futures_rule_pb2.CgbFuturesDeliveryRulePack.FromString(raw_v2)
    ts_product = next(p for p in mutated_payload.products if p.product_code == "TS")
    assert ts_product.contract_size_in_quote_units == 20_000
    ts_product.contract_size_in_quote_units = 10_000
    mutated_raw = mutated_payload.SerializeToString()

    ts_mutated = live_client.analyze_futures_hedge(
        rates.AnalyzeFuturesHedgeRequest(
            units=UNITS,
            product_code="TS",
            target_dv01=_dec("1234.50", UNITS.dv01),
            ctd_dv01_per_100=_dec("0.0145", UNITS.dv01_per_100),
            conversion_factor=_dec("0.9987", UNITS.dimensionless),
            delivery_rule_pack=_delivery_pack_from_bytes(mutated_raw, version=2),
        )
    )
    assert _to_dec(ts_mutated.measures.futures_contract_dv01) == Decimal("145.188745368980")
    assert ts_mutated.measures.recommended_contracts == -9
    assert _to_dec(ts_mutated.measures.contract_notional) == Decimal("1000000")
    _assert_unit_eq(ts_mutated.measures.contract_notional.unit, UNITS.currency_amount)


def test_live_futures_hedge_v2_validation_and_window_boundaries(
    live_client: RatesClient,
) -> None:
    def _hedge_req(
        *,
        product_code: str = "TS",
        pack: rule_pb2.MarketRulePack | None = None,
        valuation_at: common_pb2.MarketTime | None = None,
    ) -> rates.AnalyzeFuturesHedgeRequest:
        kwargs: dict[str, Any] = {
            "units": UNITS,
            "product_code": product_code,
            "target_dv01": _dec("1234.50", UNITS.dv01),
            "ctd_dv01_per_100": _dec("0.0145", UNITS.dv01_per_100),
            "conversion_factor": _dec("0.9987", UNITS.dimensionless),
        }
        if pack is not None:
            kwargs["delivery_rule_pack"] = pack
        if valuation_at is not None:
            kwargs["valuation_at"] = valuation_at
        return rates.AnalyzeFuturesHedgeRequest(**kwargs)

    # 1. Missing pack -> ValidationFailed
    with pytest.raises(RatesError) as exc_no_pack:
        live_client.analyze_futures_hedge(_hedge_req(pack=None))
    assert exc_no_pack.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    # 2. Missing content_hash -> ValidationFailed
    with pytest.raises(RatesError) as exc_no_hash:
        live_client.analyze_futures_hedge(
            _hedge_req(pack=_delivery_pack_v2(include_content_hash=False))
        )
    assert exc_no_hash.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    # 3. Wrong 32-byte content_hash -> HashMismatch
    with pytest.raises(RatesError) as exc_bad_hash:
        live_client.analyze_futures_hedge(
            _hedge_req(pack=_delivery_pack_v2(content_hash_override=b"\xFF" * 32))
        )
    assert exc_bad_hash.value.detail.code == error_pb2.ERROR_CODE_HASH_MISMATCH

    # 4. v1 pack missing contract_size_in_quote_units -> ValidationFailed + field violation path
    with pytest.raises(RatesError) as exc_v1:
        live_client.analyze_futures_hedge(_hedge_req(pack=_delivery_pack_v1()))
    assert exc_v1.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED
    assert any(
        v.field
        == "context.rule_pack.content.products[product_code=TS].contract_size_in_quote_units"
        for v in exc_v1.value.detail.field_violations
    )

    # 5. Non-100 face_quote_basis (mutated to 200 with recomputed SHA-256) -> ValidationFailed
    raw_v2 = (
        REPO_ROOT / "domain-packs" / "cgb-futures" / "cgb-futures-v2.bin"
    ).read_bytes()
    basis_200 = cgb_futures_rule_pb2.CgbFuturesDeliveryRulePack.FromString(raw_v2)
    basis_200.face_quote_basis.coefficient = "200"
    basis_200.face_quote_basis.scale = 0
    with pytest.raises(RatesError) as exc_basis:
        live_client.analyze_futures_hedge(
            _hedge_req(pack=_delivery_pack_from_bytes(basis_200.SerializeToString()))
        )
    assert exc_basis.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    # 6. Half-open valuation_at window [effective_from, effective_to):
    #    - valuation_at == effective_from (2026-01-01 00:00:00+08) succeeds.
    left_ok = live_client.analyze_futures_hedge(
        _hedge_req(
            pack=_delivery_pack_v2(),
            valuation_at=_market_time("2025-12-31T16:00:00Z", "2026-01-01"),
        )
    )
    assert left_ok.measures.recommended_contracts == -4

    #    - valuation_at == effective_to (2028-01-01 00:00:00+08) fails closed.
    with pytest.raises(RatesError) as exc_right:
        live_client.analyze_futures_hedge(
            _hedge_req(
                pack=_delivery_pack_v2(),
                valuation_at=_market_time("2027-12-31T16:00:00Z", "2028-01-01"),
            )
        )
    assert exc_right.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED

    #    - valuation_at < effective_from (2025-12-31 23:59:59+08) fails closed.
    with pytest.raises(RatesError) as exc_before:
        live_client.analyze_futures_hedge(
            _hedge_req(
                pack=_delivery_pack_v2(),
                valuation_at=_market_time("2025-12-31T15:59:59Z", "2025-12-31"),
            )
        )
    assert exc_before.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED


def test_live_curve_carry_roll_delivery_and_hedge_core_regression(
    live_client: RatesClient,
) -> None:
    # 1. Curve interpolation
    nodes = [
        rates.YieldCurveNodeInput(
            maturity_date="2028-03-02", yield_to_maturity=_dec("0.020", UNITS.rate)
        ),
        rates.YieldCurveNodeInput(
            maturity_date="2036-03-02", yield_to_maturity=_dec("0.026", UNITS.rate)
        ),
    ]
    curve_res = live_client.interpolate_yield_curve(
        rates.InterpolateYieldCurveRequest(
            units=UNITS,
            as_of_date="2026-03-02",
            nodes=nodes,
            query_date="2032-03-02",
        )
    )
    assert Decimal("0.020") < _to_dec(curve_res.yield_to_maturity) < Decimal("0.026")
    _assert_unit_eq(curve_res.yield_to_maturity.unit, UNITS.rate)

    # 2. Carry / Roll-down
    carry_res = live_client.analyze_carry_roll(
        rates.AnalyzeCarryRollRequest(
            units=UNITS,
            bond=_sample_bond(),
            initial_settlement="2026-03-02",
            horizon_settlement="2026-06-02",
            calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
            curve_as_of_date="2026-03-02",
            curve_nodes=nodes,
        )
    )
    assert _to_dec(carry_res.measures.carry) + _to_dec(
        carry_res.measures.roll_down
    ) == _to_dec(carry_res.measures.total_return)

    # 3. Futures delivery + CTD (v1 delivery pack remains compatible for delivery RPC)
    deliv_res = live_client.analyze_futures_delivery(
        rates.AnalyzeFuturesDeliveryRequest(
            units=UNITS,
            product_code="T",
            delivery_month_first="2026-06-01",
            purchase_date="2026-03-02",
            delivery_date="2026-06-15",
            futures_clean_price=_dec("101.50", UNITS.price_per_100),
            delivery_rule_pack=_delivery_pack_v1(),
            candidates=[
                rates.FuturesCandidateInput(
                    bond=_sample_bond(),
                    spot_clean_price=_dec("100.20", UNITS.price_per_100),
                )
            ],
            annual_financing_rate=_dec("0.018", UNITS.rate),
        )
    )
    assert deliv_res.ctd_index == 0
    cf = deliv_res.candidates[0].measures.conversion_factor

    # 4. Futures DV01 hedge with v2 pack + unknown product validation error
    bond_res = live_client.analyze_bond(
        rates.AnalyzeBondRequest(
            units=UNITS,
            bond=_sample_bond(),
            settlement_date="2026-03-02",
            calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
            clean_price=_dec("100", UNITS.price_per_100),
            execution_mode=rates.BOND_TAX_BASIS_PRE_TAX,
        )
    )
    hedge_res = live_client.analyze_futures_hedge(
        rates.AnalyzeFuturesHedgeRequest(
            units=UNITS,
            product_code="T",
            target_dv01=_dec("85000", UNITS.dv01),
            ctd_dv01_per_100=bond_res.measures.dv01,
            conversion_factor=cf,
            delivery_rule_pack=_delivery_pack_v2(),
        )
    )
    assert hedge_res.measures.recommended_contracts < 0
    assert _to_dec(hedge_res.measures.contract_notional) == Decimal("1000000")
    _assert_unit_eq(hedge_res.measures.contract_notional.unit, UNITS.currency_amount)

    with pytest.raises(RatesError) as exc_unknown:
        live_client.analyze_futures_hedge(
            rates.AnalyzeFuturesHedgeRequest(
                units=UNITS,
                product_code="UNKNOWN",
                target_dv01=_dec("85000", UNITS.dv01),
                ctd_dv01_per_100=bond_res.measures.dv01,
                conversion_factor=cf,
                delivery_rule_pack=_delivery_pack_v2(),
            )
        )
    assert exc_unknown.value.detail.code == error_pb2.ERROR_CODE_VALIDATION_FAILED
