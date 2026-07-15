#!/usr/bin/env python3
"""Reproducibly build normalized six-bond fixtures and expected candidate.

The frozen Delivery facts below are normalized derivatives.  Raw third-party
records are neither needed nor accepted by this test-only builder.
"""

import argparse
import importlib.util
import json
import sys
from datetime import date
from pathlib import Path


HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent.parent
FIXTURES_DIR = ROOT / "tests" / "golden-cases" / "china-rates" / "fixtures"
EXPECTED_DIR = ROOT / "tests" / "golden-cases" / "china-rates" / "expected"


def _load_local(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


manual = _load_local("manual_oracle_for_builder", "oracle_manual.py")

CONVENTION = "cgb-reference-v1"
CALENDAR = "cgb-reference-calendar-v1"
TIMEZONE = "Asia/Shanghai"
VALUATION_AT = "2026-07-13T15:00:00+08:00"
SETTLEMENT_DATE = date(2026, 7, 14)
FACE_VALUE = "100.000000000000"
SOURCE_MANIFEST = "tests/golden-cases/china-rates/iteration-3-cgb-basic-info-source-manifest.json"
OBJECT_SHA = "765d8afe8605562dbf1c4d2a23513de25e98945496f8d297565c1d943eed8faf"
CANONICAL_SHA = "8216f586cbec959a08bb62a5e00c2492c99dc01e641e0c876a918b710e9d50ff"
SOURCE_MANIFEST_SHA = "078c14aaa67bc3d819d0a089e415d13029e09d88d43d0946dbdf10e7e8221dd1"
CALENDAR_COVERAGE_START = "2005-01-01"
CALENDAR_COVERAGE_END = "2026-12-31"

ORACLE_IDENTITY = {
    "role": "frozen_target_contract_identity_not_oracle_execution_claim",
    "result_schema": "ficant.bond-analytics.result.v1",
    "engine": "ficant-fixed-income-native/0.1.0",
    "algorithm": "ficant.cgb.fixed-rate.reference/1",
    "abi": "FICANT_FIXED_INCOME_ABI_V1=1",
    "calendar": {
        "id": CALENDAR,
        "requirement": "REFERENCE_REPLAY",
        "resolution_scope": "per_result",
        "resolution_policy": {
            "exact_if": "all_required_dates_inside_frozen_exact_coverage",
            "exact_resolution": "EXACT",
            "otherwise_resolution": "PROVISIONAL_WEEKEND_ONLY",
        },
        "coverage": f"{CALENDAR_COVERAGE_START}..{CALENDAR_COVERAGE_END}",
    },
    "rule_pack": {
        "status": "pending_production_proof",
        "id": None,
        "version": None,
        "content_sha256": None,
    },
    "snapshot": {
        "status": "source_manifest_only_no_production_snapshot_proof",
        "source_manifest": SOURCE_MANIFEST,
        "source_manifest_sha256": SOURCE_MANIFEST_SHA,
        "source_object_sha256": OBJECT_SHA,
        "canonical_records_sha256": CANONICAL_SHA,
        "production_id": None,
        "production_version": None,
        "production_content_sha256": None,
    },
}

CASHFLOW_SEMANTICS = {
    "sequence": "one_based_ascending_payment_eligible_cashflow",
    "component_identity": "ordered_subset_of_coupon_then_principal",
    "coupon": "coupon_amount_for_nominal_accrual_period",
    "principal": "face_redemption_amount",
    "total": "coupon_plus_principal",
    "unit": "CNY_PER_100_FACE",
}

TOLERANCES = {
    "date_cashflow_identity_unit_version_lineage": "exact",
    "price_accrued_abs": "0.000000010000",
    "ytm_abs": "0.000000000100",
    "duration_convexity_rel": "0.000000010000",
    "duration_convexity_abs_floor": "0.000000000100",
    "dv01_abs": "0.000000010000",
    "finite_difference_rel": "0.000100000000",
}

DERIVED_BONDS = [
    {
        "bond_id": "269937.IB", "instrument_type": "discount", "issue_date": "2026-06-18",
        "maturity_date": "2026-12-17", "coupon_rate_decimal": "0.000000000000",
        "coupon_rate_percent": "0.000000000000", "frequency": 0, "frequency_name": "discount",
        "coupon_anchors": [], "source_reference_yield_percent": "0.916700000000",
        "synthetic_yield_decimal": "0.011000000000",
    },
    {
        "bond_id": "260013.IB", "instrument_type": "fixed_rate", "issue_date": "2026-06-25",
        "maturity_date": "2028-06-25", "coupon_rate_decimal": "0.012100000000",
        "coupon_rate_percent": "1.210000000000", "frequency": 1, "frequency_name": "annual",
        "coupon_anchors": ["06-25"], "synthetic_yield_decimal": "0.013000000000",
    },
    {
        "bond_id": "260011.IB", "instrument_type": "fixed_rate", "issue_date": "2026-05-25",
        "maturity_date": "2029-05-25", "coupon_rate_decimal": "0.012600000000",
        "coupon_rate_percent": "1.260000000000", "frequency": 1, "frequency_name": "annual",
        "coupon_anchors": ["05-25"], "synthetic_yield_decimal": "0.013800000000",
    },
    {
        "bond_id": "260008.IB", "instrument_type": "fixed_rate", "issue_date": "2026-04-15",
        "maturity_date": "2031-04-15", "coupon_rate_decimal": "0.015000000000",
        "coupon_rate_percent": "1.500000000000", "frequency": 1, "frequency_name": "annual",
        "coupon_anchors": ["04-15"], "synthetic_yield_decimal": "0.015500000000",
    },
    {
        "bond_id": "260012.IB", "instrument_type": "fixed_rate", "issue_date": "2026-06-15",
        "maturity_date": "2033-06-15", "coupon_rate_decimal": "0.015600000000",
        "coupon_rate_percent": "1.560000000000", "frequency": 1, "frequency_name": "annual",
        "coupon_anchors": ["06-15"], "synthetic_yield_decimal": "0.016500000000",
    },
    {
        "bond_id": "260010.IB", "instrument_type": "fixed_rate", "issue_date": "2026-05-15",
        "maturity_date": "2036-05-15", "coupon_rate_decimal": "0.017200000000",
        "coupon_rate_percent": "1.720000000000", "frequency": 2, "frequency_name": "semiannual",
        "coupon_anchors": ["05-15", "11-15"], "synthetic_yield_decimal": "0.018000000000",
    },
]


def source_lineage():
    return {
        "source_manifest": SOURCE_MANIFEST,
        "object_sha256": OBJECT_SHA,
        "canonical_records_sha256": CANONICAL_SHA,
        "record_count": 6,
        "raw_third_party_data_retained": False,
    }


def fixture_for(bond):
    fixture = {
        "schema": "ficant.fixture.cgb-reference-v1.bond.v1",
        **bond,
        "face_value": FACE_VALUE,
        "market_timezone": TIMEZONE,
        "valuation_at": VALUATION_AT,
        "settlement_date": SETTLEMENT_DATE.isoformat(),
        "convention": CONVENTION,
        "calendar": CALENDAR,
        "source_lineage": source_lineage(),
    }
    return fixture


def serialize_cashflow(flow):
    components = []
    if flow["coupon"] != 0:
        components.append("coupon")
    if flow["principal"] != 0:
        components.append("principal")
    return {
        "sequence": flow["sequence"],
        "nominal_date": flow["nominal_date"],
        "payment_date": flow["payment_date"],
        "components": components,
        "coupon": manual.decimal12(flow["coupon"]),
        "principal": manual.decimal12(flow["principal"]),
        "total": manual.decimal12(flow["total"]),
        "time_years": manual.decimal12(flow["time_years"]),
    }


def case_identity(result):
    required_dates = [result["settlement_date"]] + [
        value
        for flow in result["cashflows"]
        for value in (flow["nominal_date"], flow["payment_date"])
    ]
    uses_provisional_calendar = not all(
        CALENDAR_COVERAGE_START <= value <= CALENDAR_COVERAGE_END
        for value in required_dates
    )
    return {
        "bond_id": result["bond_id"],
        "mode": result["mode"],
        "result_schema": ORACLE_IDENTITY["result_schema"],
        "engine": ORACLE_IDENTITY["engine"],
        "algorithm": ORACLE_IDENTITY["algorithm"],
        "abi": ORACLE_IDENTITY["abi"],
        "convention": CONVENTION,
        "calendar_id": CALENDAR,
        "calendar_requirement": "REFERENCE_REPLAY",
        "calendar_resolution": (
            "PROVISIONAL_WEEKEND_ONLY" if uses_provisional_calendar
            else "EXACT"
        ),
        "calendar_coverage": f"{CALENDAR_COVERAGE_START}..{CALENDAR_COVERAGE_END}",
        "market_timezone": TIMEZONE,
        "valuation_at": VALUATION_AT,
        "settlement_date": SETTLEMENT_DATE.isoformat(),
        "rule_pack_status": ORACLE_IDENTITY["rule_pack"]["status"],
        "rule_pack_content_sha256": None,
        "snapshot_status": ORACLE_IDENTITY["snapshot"]["status"],
        "snapshot_source_object_sha256": OBJECT_SHA,
        "snapshot_production_content_sha256": None,
    }


def serialize_result(result):
    finite = result["finite_diff"]
    accrued_interest = manual.decimal12(result["accrued_interest"])
    clean_price = manual.decimal12(result["clean_price"])
    dirty_price = manual.decimal12(manual.d(clean_price) + manual.d(accrued_interest))
    return {
        "bond_id": result["bond_id"],
        "mode": result["mode"],
        "input_value": manual.decimal12(result["input_value"]),
        "settlement_date": result["settlement_date"],
        "identity": case_identity(result),
        "cashflow_semantics": CASHFLOW_SEMANTICS,
        "cashflow_count": len(result["cashflows"]),
        "cashflows": [serialize_cashflow(flow) for flow in result["cashflows"]],
        "accrued_interest": accrued_interest,
        "clean_price": clean_price,
        "dirty_price": dirty_price,
        "yield_to_maturity": manual.decimal12(result["yield_to_maturity"]),
        "macaulay_duration": manual.decimal12(result["macaulay_duration"]),
        "modified_duration": manual.decimal12(result["modified_duration"]),
        "convexity": manual.decimal12(result["convexity"]),
        "dv01": manual.decimal12(result["dv01"]),
        "round_trip": {
            "yield_to_maturity": manual.decimal12(result["round_trip"]["round_trip_ytm"]),
            "absolute_difference": manual.decimal12(result["round_trip"]["ytm_diff"]),
        },
        "finite_difference": {
            "bump_decimal": "0.000100000000",
            "price_minus_1bp": manual.decimal12(finite["price_down"]),
            "price_plus_1bp": manual.decimal12(finite["price_up"]),
            "dv01": manual.decimal12(finite["dv01_finite_diff"]),
            "dv01_relative_difference": manual.decimal12(finite["dv01_rel_diff"]),
            "convexity": manual.decimal12(finite["convexity_finite_diff"]),
            "convexity_relative_difference": manual.decimal12(finite["convexity_rel_diff"]),
        },
        "units": {
            "price_accrued_dv01": "CNY_PER_100_FACE",
            "cashflow": "CNY_PER_100_FACE",
            "yield": "DECIMAL_RATE",
            "duration": "YEARS",
            "convexity": "YEARS_SQUARED",
        },
    }


def compute_results():
    results = {}
    for bond in DERIVED_BONDS:
        common = {
            "bond_id": bond["bond_id"],
            "issue_date": date.fromisoformat(bond["issue_date"]),
            "maturity_date": date.fromisoformat(bond["maturity_date"]),
            "coupon_rate": float(bond["coupon_rate_decimal"]),
            "freq": bond["frequency"],
            "settlement_date": SETTLEMENT_DATE,
            "face_value": 100.0,
        }
        synthetic_yield = float(bond["synthetic_yield_decimal"])
        by_yield = manual.value_bond(mode="YIELD_IN", input_value=synthetic_yield, **common)
        by_price = manual.value_bond(mode="PRICE_IN", input_value=by_yield["clean_price"], **common)
        results[f"{bond['bond_id']}:YIELD_IN"] = serialize_result(by_yield)
        results[f"{bond['bond_id']}:PRICE_IN"] = serialize_result(by_price)
    return results


def acceptance_mapping(results):
    case_keys = list(results)
    mapping = {
        f"Q-{number:03d}": {
            "cases": [case_key],
            "invariants": ["complete_case_result_and_frozen_identity"],
        }
        for number, case_key in enumerate(case_keys, 1)
    }
    invariant_names = [
        "asia_shanghai_epoch_to_market_date",
        "adjusted_payment_date_cashflow_inclusion",
        "following_adjustment_no_extra_interest",
        "discount_actual_actual_split_by_calendar_year",
        "discount_single_simple_yield_redemption",
        "semiannual_nominal_coupon_anchors",
        "strict_settlement_payment_boundary",
        "cashflow_sequence_component_and_amount_identity",
        "price_yield_round_trip",
        "decimal12_round_half_even",
        "centered_plus_minus_1bp_risk",
    ]
    for number, invariant in enumerate(invariant_names, 13):
        mapping[f"Q-{number:03d}"] = {
            "cases": [],
            "invariants": [invariant],
        }
    return mapping


def build_expected():
    results = compute_results()
    return {
        "schema": "ficant.test-expected.cgb-reference-v1.v1",
        "candidate_id": "I3-TW-ORACLE-REROUTE-1-R1",
        "quality_status": "pending_quality_approval",
        "convention": CONVENTION,
        "calendar": CALENDAR,
        "market_timezone": TIMEZONE,
        "valuation_at": VALUATION_AT,
        "settlement_date": SETTLEMENT_DATE.isoformat(),
        "face_value": FACE_VALUE,
        "source_lineage": source_lineage(),
        "oracle_identity": ORACLE_IDENTITY,
        "cashflow_semantics": CASHFLOW_SEMANTICS,
        "tolerances": TOLERANCES,
        "provenance": {
            "expected_source": "independent_manual_python_oracle",
            "production_cpp_used": False,
            "manual_formula_layer": "executed",
            "price_yield_round_trip": "executed",
            "centered_plus_minus_1bp": "executed",
            "quantlib_required_version": "1.42.1",
            "quantlib_agreement": "blocked_pending_independent_execution",
        },
        "acceptance_ids": [f"Q-{number:03d}" for number in range(1, 24)],
        "acceptance_mapping": acceptance_mapping(results),
        "results": results,
    }


def write_json(path, payload):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--expected-output", type=Path)
    args = parser.parse_args(argv)
    if args.expected_output:
        write_json(args.expected_output, build_expected())
        print(f"built expected candidate at {args.expected_output}")
        return 0
    for bond in DERIVED_BONDS:
        write_json(FIXTURES_DIR / f"bond-{bond['bond_id']}.json", fixture_for(bond))
    write_json(EXPECTED_DIR / "cgb-reference-v1-expected.json", build_expected())
    print("built 6 normalized fixtures and 12 expected candidate cases")
    print("quality_status=pending_quality_approval")
    print("quantlib_agreement=blocked_pending_independent_execution")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
