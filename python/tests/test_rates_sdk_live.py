"""Live end-to-end Python SDK verification against zero-dependency ficant-server."""

from __future__ import annotations

import hashlib
import socket
import subprocess
import sys
import time
from datetime import datetime, timezone
from decimal import Decimal
from pathlib import Path

import pytest
from google.protobuf.any_pb2 import Any as AnyPb
from google.protobuf.timestamp_pb2 import Timestamp

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

from ficant.core.v1 import common_pb2, subject_pb2  # noqa: E402
from ficant.market.v1 import definition_pb2, rule_pb2, tax_rule_pb2  # noqa: E402
from ficant.rates.v1 import analytics_pb2 as rates  # noqa: E402
from ficant_sdk import RatesClient, RatesError  # noqa: E402

ULID_PREFIX = "01ARZ3NDEKTSV4RRFFQ69G5FA"


def _ulid(suffix: str) -> common_pb2.Ulid:
    return common_pb2.Ulid(value=f"{ULID_PREFIX}{suffix}")


def _unit(suffix: str) -> common_pb2.UnitRef:
    return common_pb2.UnitRef(unit_id=_ulid(suffix), version=1)


RATE_UNIT = common_pb2.UnitRef(
    unit_id=common_pb2.Ulid(value="01K2CGBVAT0000000000000000"), version=1
)

UNITS = rates.AnalysisUnits(
    currency_amount=_unit("A"),
    price_per_100=_unit("B"),
    rate=RATE_UNIT,
    years=_unit("D"),
    years_squared=_unit("E"),
    dv01_per_100=_unit("F"),
    dv01=_unit("G"),
    dimensionless=_unit("H"),
    contract_count=_unit("J"),
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
    if not path.exists():
        subprocess.run(
            ["cargo", "build", "--locked", "-p", "ficant-server"],
            cwd=REPO_ROOT,
            check=True,
        )
    return path


@pytest.fixture(scope="module")
def live_client() -> RatesClient:
    port = _free_port()
    addr = f"127.0.0.1:{port}"
    import os

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


def _sample_bond(first_issue: str = "2025-08-08", taxable: bool = True) -> definition_pb2.Bond:
    vat = (
        definition_pb2.VALUE_ADDED_TAX_STATUS_TAXABLE
        if taxable
        else definition_pb2.VALUE_ADDED_TAX_STATUS_EXEMPT
    )
    return definition_pb2.Bond(
        instrument=common_pb2.VersionRef(id=_ulid("1"), version=1),
        first_issue_date=first_issue,
        current_issue_date=first_issue,
        maturity_date="2035-08-08",
        face_value=_dec("100", UNITS.price_per_100),
        cumulative_issued_amount=_dec("10000000000", UNITS.currency_amount),
        tax_attributes=definition_pb2.BondTaxAttributes(
            value_added_tax_status=vat,
            income_tax_status=definition_pb2.INCOME_TAX_STATUS_EXEMPT,
        ),
        coupon_rate=_dec("0.025", RATE_UNIT),
        coupon_frequency=definition_pb2.BOND_COUPON_FREQUENCY_SEMIANNUAL,
        day_count=definition_pb2.BOND_DAY_COUNT_CONVENTION_ACT_ACT_BOND_ISMA,
        business_day=definition_pb2.BOND_BUSINESS_DAY_CONVENTION_FOLLOWING,
    )


def _tax_pack() -> rule_pb2.MarketRulePack:
    raw = (
        REPO_ROOT / "domain-packs" / "cgb-interest-tax" / "cgb-interest-tax-v1.bin"
    ).read_bytes()
    return rule_pb2.MarketRulePack(
        rule_pack_id=_ulid("T"),
        version=1,
        market="CN",
        rule_type="tax",
        source="ficant-authority/cgb-interest-tax/v1",
        effective_from=_market_time("2025-12-31T16:00:00Z", "2026-01-01"),
        effective_to=_market_time("2027-12-31T16:00:00Z", "2028-01-01"),
        verification_status=rule_pb2.VERIFICATION_STATUS_VERIFIED,
        content_hash=common_pb2.Sha256(value=hashlib.sha256(raw).digest()),
        content=AnyPb(
            type_url="type.googleapis.com/ficant.market.v1.TaxRulePackV2", value=raw
        ),
    )


def _delivery_pack() -> rule_pb2.MarketRulePack:
    raw = (
        REPO_ROOT / "domain-packs" / "cgb-futures" / "cgb-futures-v1.bin"
    ).read_bytes()
    return rule_pb2.MarketRulePack(
        rule_pack_id=_ulid("F"),
        version=1,
        market="CFFEX",
        rule_type="cgb-futures",
        source="cffex-rulebook",
        effective_from=_market_time("2025-12-31T16:00:00Z", "2026-01-01"),
        effective_to=_market_time("2027-12-31T16:00:00Z", "2028-01-01"),
        verification_status=rule_pb2.VERIFICATION_STATUS_VERIFIED,
        content_hash=common_pb2.Sha256(value=hashlib.sha256(raw).digest()),
        content=AnyPb(
            type_url="type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack",
            value=raw,
        ),
    )


def test_live_bond_curve_carry_delivery_and_hedge(live_client: RatesClient) -> None:
    # 1. Pre-tax + post-tax bond analysis
    bond_res = live_client.analyze_bond(
        rates.AnalyzeBondRequest(
            units=UNITS,
            bond=_sample_bond("2025-08-08", taxable=True),
            settlement_date="2026-03-02",
            calendar_requirement=rates.CALENDAR_REQUIREMENT_REFERENCE_REPLAY,
            clean_price=_dec("100", UNITS.price_per_100),
            tax_context=rates.BondTaxContext(
                subject_tax_treatment=subject_pb2.TaxTreatment(
                    value_added_tax_profile="cn-vat-general-taxpayer",
                    income_tax_profile="cn-cgb-interest-cit-exempt",
                ),
                tax_rule_pack=_tax_pack(),
            ),
        )
    )
    pre_ytm = _to_dec(bond_res.measures.yield_to_maturity)
    post_ytm = _to_dec(bond_res.after_tax.yield_to_maturity)
    assert pre_ytm > post_ytm > Decimal("0.02")
    assert (
        bond_res.after_tax.claim_scope
        == tax_rule_pb2.COUPON_TAX_CLAIM_SCOPE_COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT
    )

    # 2. Curve interpolation
    nodes = [
        rates.YieldCurveNodeInput(
            maturity_date="2028-03-02", yield_to_maturity=_dec("0.020", RATE_UNIT)
        ),
        rates.YieldCurveNodeInput(
            maturity_date="2036-03-02", yield_to_maturity=_dec("0.026", RATE_UNIT)
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

    # 3. Carry / Roll-down
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

    # 4. Futures delivery + CTD
    deliv_res = live_client.analyze_futures_delivery(
        rates.AnalyzeFuturesDeliveryRequest(
            units=UNITS,
            product_code="T",
            delivery_month_first="2026-06-01",
            purchase_date="2026-03-02",
            delivery_date="2026-06-15",
            futures_clean_price=_dec("101.50", UNITS.price_per_100),
            delivery_rule_pack=_delivery_pack(),
            candidates=[
                rates.FuturesCandidateInput(
                    bond=_sample_bond(),
                    spot_clean_price=_dec("100.20", UNITS.price_per_100),
                )
            ],
            annual_financing_rate=_dec("0.018", RATE_UNIT),
        )
    )
    assert deliv_res.ctd_index == 0
    cf = deliv_res.candidates[0].measures.conversion_factor

    # 5. Futures DV01 hedge
    hedge_res = live_client.analyze_futures_hedge(
        rates.AnalyzeFuturesHedgeRequest(
            units=UNITS,
            product_code="T",
            target_dv01=_dec("85000", UNITS.dv01),
            ctd_dv01_per_100=bond_res.measures.dv01,
            conversion_factor=cf,
        )
    )
    assert hedge_res.measures.recommended_contracts < 0

    # 6. Validation error path
    with pytest.raises(RatesError):
        live_client.analyze_futures_hedge(
            rates.AnalyzeFuturesHedgeRequest(
                units=UNITS,
                product_code="UNKNOWN",
                target_dv01=_dec("85000", UNITS.dv01),
                ctd_dv01_per_100=bond_res.measures.dv01,
                conversion_factor=cf,
            )
        )
