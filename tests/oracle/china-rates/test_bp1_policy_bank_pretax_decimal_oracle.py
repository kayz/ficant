from __future__ import annotations

import ast
from datetime import date
from decimal import Decimal, ROUND_HALF_EVEN
import json
from pathlib import Path

import pytest

from bp1_policy_bank_pretax_decimal_oracle import (
    BISECTION_STEPS,
    CONTEXT,
    COVERAGE_END,
    COVERAGE_START,
    EXPECTED_BOND_IDS,
    EXPECTED_MODES,
    EXPECTED_OFFICIAL_NOTICES,
    build_calendar_spec,
    build_expected,
    evaluate_bond_mode,
    following_adjust,
    generate_nominal_schedule,
    is_in_coverage_business_day,
)

ROOT = Path(__file__).resolve().parents[3]
INPUTS = ROOT / "tests/golden-cases/china-rates/bp1-policy-bank-pretax-inputs.json"
EXPECTED = (
    ROOT
    / "tests/golden-cases/china-rates/expected/bp1-policy-bank-pretax-expected.json"
)
ORACLE = Path(__file__).with_name("bp1_policy_bank_pretax_decimal_oracle.py")


def _documents() -> tuple[dict, dict]:
    return (
        json.loads(INPUTS.read_text(encoding="utf-8")),
        json.loads(EXPECTED.read_text(encoding="utf-8")),
    )


def test_oracle_imports_only_standard_library_whitelist() -> None:
    tree = ast.parse(ORACLE.read_text(encoding="utf-8"))
    imported_roots = {
        alias.name.split(".", maxsplit=1)[0]
        for node in ast.walk(tree)
        if isinstance(node, ast.Import)
        for alias in node.names
    }
    imported_roots.update(
        node.module.split(".", maxsplit=1)[0]
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom) and node.module is not None
    )
    assert imported_roots == {
        "__future__",
        "calendar",
        "datetime",
        "decimal",
        "json",
        "pathlib",
        "typing",
    }
    assert CONTEXT.prec == 60
    assert CONTEXT.rounding == ROUND_HALF_EVEN
    assert BISECTION_STEPS == 260


def test_calendar_notices_real_sessions_and_provisional_boundary() -> None:
    inputs, expected = _documents()
    cal_doc = inputs["calendar"]
    cal_spec = build_calendar_spec(cal_doc)

    assert cal_doc["coverage_start"] == "2024-01-01"
    assert cal_doc["coverage_end"] == "2026-12-31"
    assert cal_doc["first_session"] == {"local_date": "2024-01-01", "closed": True}
    assert cal_doc["last_session"] == {"local_date": "2026-12-31", "closed": False}
    assert not is_in_coverage_business_day(date(2024, 1, 1), cal_spec)
    assert is_in_coverage_business_day(date(2026, 12, 31), cal_spec)

    assert len(cal_doc["sources"]) == 3
    for expected_notice, actual_notice in zip(
        EXPECTED_OFFICIAL_NOTICES, cal_doc["sources"], strict=True
    ):
        assert actual_notice["year"] == expected_notice["year"]
        assert actual_notice["document_number"] == expected_notice["document_number"]
        assert actual_notice["title"] == expected_notice["title"]
        assert actual_notice["url"] == expected_notice["url"]
    assert expected["calendar"]["sources"] == list(EXPECTED_OFFICIAL_NOTICES)

    # 2026 National Day closure (Oct 1-7 closed, Oct 8 open).
    for day in range(1, 8):
        assert not is_in_coverage_business_day(date(2026, 10, day), cal_spec)
    assert is_in_coverage_business_day(date(2026, 10, 8), cal_spec)
    assert following_adjust(date(2026, 10, 1), cal_spec, exact_required=True) == (
        date(2026, 10, 8),
        False,
    )

    # Work-weekend open inside coverage (2026-09-20 Sunday is open).
    assert is_in_coverage_business_day(date(2026, 9, 20), cal_spec)
    assert following_adjust(date(2026, 9, 20), cal_spec, exact_required=True) == (
        date(2026, 9, 20),
        False,
    )

    # Weekend-only rule applies strictly beyond 2026-12-31.
    assert following_adjust(date(2027, 3, 20), cal_spec, exact_required=False) == (
        date(2027, 3, 22),
        True,
    )
    with pytest.raises(ValueError, match="exceeds exact calendar coverage_end"):
        following_adjust(date(2027, 3, 20), cal_spec, exact_required=True)


def test_six_bonds_coverage_bounds_cartesian_keys_and_key_verifications() -> None:
    inputs, expected = _documents()
    bonds = inputs["bonds"]
    assert len(bonds) == 6
    assert tuple(b["case_id"] for b in bonds) == EXPECTED_BOND_IDS

    expected_keys = {
        f"{bond_id}:{mode}"
        for bond_id in EXPECTED_BOND_IDS
        for mode in EXPECTED_MODES
    }
    assert len(expected_keys) == 12
    assert set(expected["case_results"].keys()) == expected_keys

    bonds_by_id = {bond["case_id"]: bond for bond in bonds}
    for bond in bonds:
        first_issue = date.fromisoformat(bond["first_issue_date"])
        current_issue = date.fromisoformat(bond["current_issue_date"])
        settlement = date.fromisoformat(bond["settlement_date"])
        maturity = date.fromisoformat(bond["maturity_date"])
        assert COVERAGE_START <= first_issue <= COVERAGE_END
        assert first_issue <= current_issue < settlement < maturity
        assert bond["execution_mode"] == "PRE_TAX"
        assert bond["day_count"] == "ACT_ACT_BOND_ISMA"
        assert bond["business_day"] == "FOLLOWING"
        assert bond["face_value"] == "100.000000000000"
        assert Decimal(bond["coupon_rate"]) > Decimal(0)
        assert Decimal(bond["cumulative_issued_amount"]) > Decimal(0)

        expected_req = (
            "EXACT_MARKET"
            if bond["case_id"] == "ADBC-1Y-EXACT-ANNUAL"
            else "REFERENCE_REPLAY"
        )
        expected_res = (
            "EXACT"
            if bond["case_id"] == "ADBC-1Y-EXACT-ANNUAL"
            else "PROVISIONAL_WEEKEND_ONLY"
        )
        assert bond["calendar_requirement"] == expected_req

        for mode in EXPECTED_MODES:
            entry = expected["case_results"][f"{bond['case_id']}:{mode}"]
            assert entry["execution_mode"] == "PRE_TAX"
            assert entry["measures_tax_basis"] == "PRE_TAX"
            assert entry["after_tax"] is None
            assert entry["calendar_requirement"] == expected_req
            assert entry["calendar_resolution"] == expected_res

    # EIBC-3Y-SEMIANNUAL: 2026-10-01 -> 2026-10-08 and exact 183-day nominal period.
    eibc3 = bonds_by_id["EIBC-3Y-SEMIANNUAL"]
    eibc3_sched = generate_nominal_schedule(
        date.fromisoformat(eibc3["first_issue_date"]),
        date.fromisoformat(eibc3["maturity_date"]),
        2,
    )
    assert date(2026, 4, 1) in eibc3_sched
    assert date(2026, 10, 1) in eibc3_sched
    assert (date(2026, 10, 1) - date(2026, 4, 1)).days == 183
    for mode in EXPECTED_MODES:
        cfs = expected["case_results"][f"EIBC-3Y-SEMIANNUAL:{mode}"]["cashflows"]
        assert cfs[0]["nominal_date"] == "2026-10-01"
        assert cfs[0]["payment_date"] == "2026-10-08"
        assert cfs[-1]["nominal_date"] == "2028-04-01"
        assert cfs[-1]["payment_date"] == "2028-04-03"

    # ADBC-1Y-EXACT-ANNUAL: sole nominal 2026-11-15 -> payment 2026-11-16, Exact.
    for mode in EXPECTED_MODES:
        cfs = expected["case_results"][f"ADBC-1Y-EXACT-ANNUAL:{mode}"]["cashflows"]
        assert len(cfs) == 1
        assert cfs[0]["nominal_date"] == "2026-11-15"
        assert cfs[0]["payment_date"] == "2026-11-16"

    # ADBC-7Y-SEMIANNUAL: fixed re-issuance dates, non-face cumulative issued amount, leap year 2028.
    adbc7 = bonds_by_id["ADBC-7Y-SEMIANNUAL"]
    assert adbc7["first_issue_date"] == "2024-08-18"
    assert adbc7["current_issue_date"] == "2025-02-18"
    assert adbc7["maturity_date"] == "2031-08-18"
    assert Decimal(adbc7["cumulative_issued_amount"]) != Decimal(adbc7["face_value"])
    adbc7_cfs = expected["case_results"]["ADBC-7Y-SEMIANNUAL:YIELD_IN"]["cashflows"]
    adbc7_noms = [cf["nominal_date"] for cf in adbc7_cfs]
    assert "2028-02-18" in adbc7_noms and "2028-08-18" in adbc7_noms
    assert (date(2028, 8, 18) - date(2028, 2, 18)).days == 182

    # Post-2026 weekend shifts on the other 3 long bonds.
    cdb_ann_map = {
        cf["nominal_date"]: cf["payment_date"]
        for cf in expected["case_results"]["CDB-10Y-ANNUAL:YIELD_IN"]["cashflows"]
    }
    assert cdb_ann_map["2026-07-12"] == "2026-07-13"
    assert cdb_ann_map["2031-07-12"] == "2031-07-14"

    cdb_semi_map = {
        cf["nominal_date"]: cf["payment_date"]
        for cf in expected["case_results"]["CDB-10Y-SEMIANNUAL:YIELD_IN"]["cashflows"]
    }
    assert cdb_semi_map["2026-09-20"] == "2026-09-20"
    assert cdb_semi_map["2027-03-20"] == "2027-03-22"

    eibc5_map = {
        cf["nominal_date"]: cf["payment_date"]
        for cf in expected["case_results"]["EIBC-5Y-ANNUAL:YIELD_IN"]["cashflows"]
    }
    assert eibc5_map["2026-08-20"] == "2026-08-20"
    assert eibc5_map["2028-08-20"] == "2028-08-21"


def test_independent_decimal_oracle_matches_frozen_expected_and_round_trips() -> None:
    inputs, expected = _documents()
    computed = build_expected(inputs)
    assert computed == expected

    cal_spec = build_calendar_spec(inputs["calendar"])
    for bond in inputs["bonds"]:
        bid = bond["case_id"]
        y_res = computed["case_results"][f"{bid}:YIELD_IN"]
        p_res = computed["case_results"][f"{bid}:PRICE_IN"]

        for entry in (y_res, p_res):
            for field, val in entry["measures"].items():
                assert val != "-0.000000000000", f"{bid}:{field} negative zero"
                assert Decimal(val) > Decimal(0), f"{bid}:{field} must be positive"
            for cf in entry["cashflows"]:
                for field in ("coupon", "principal", "total"):
                    assert cf[field] != "-0.000000000000"

        # Round-trip YIELD_IN -> quantized 12dp clean_price -> PRICE_IN -> yield_to_maturity.
        # |dy/dP| = 1 / (D_mod * P) <= 1/40, so 0.5e-12 price quantization shifts YTM by < 2e-14.
        rt_price_bond = dict(bond, price_in_input=y_res["measures"]["clean_price"])
        rt_price_res = evaluate_bond_mode(rt_price_bond, "PRICE_IN", cal_spec)
        assert abs(
            Decimal(rt_price_res["measures"]["yield_to_maturity"])
            - Decimal(bond["yield_in_input"])
        ) <= Decimal("0.000000000002")

        # Round-trip PRICE_IN -> quantized 12dp yield_to_maturity -> YIELD_IN -> clean_price.
        # |dP/dy| = D_mod * P <= 900 for 10Y bonds, so 0.5e-12 YTM quantization shifts price by <= 4.5e-10.
        rt_yield_bond = dict(bond, yield_in_input=p_res["measures"]["yield_to_maturity"])
        rt_yield_res = evaluate_bond_mode(rt_yield_bond, "YIELD_IN", cal_spec)
        assert abs(
            Decimal(rt_yield_res["measures"]["clean_price"])
            - Decimal(bond["price_in_input"])
        ) <= Decimal("0.000000001000")
