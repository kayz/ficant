import ast
import copy
import json
from decimal import Decimal
from pathlib import Path

import pytest

from r5e_tax_adjusted_decimal_oracle import build_expected

ROOT = Path(__file__).resolve().parents[3]
INPUTS = ROOT / "tests/golden-cases/china-rates/r5e-tax-adjusted-analytics-inputs.json"
EXPECTED = (
    ROOT
    / "tests/golden-cases/china-rates/expected/r5e-tax-adjusted-analytics-expected.json"
)
ORACLE = Path(__file__).with_name("r5e_tax_adjusted_decimal_oracle.py")


def _documents() -> tuple[dict, dict]:
    return (
        json.loads(INPUTS.read_text(encoding="utf-8")),
        json.loads(EXPECTED.read_text(encoding="utf-8")),
    )


def test_independent_decimal_oracle_matches_frozen_expected_exactly() -> None:
    inputs, expected = _documents()
    assert build_expected(inputs) == expected


def test_cutoff_reissuance_reversal_and_no_tax_difference_are_witnessed() -> None:
    inputs, _ = _documents()
    result = build_expected(inputs)
    bonds = {case["case_id"]: case for case in result["bond_cases"]}
    assert bonds["before-cutoff-exempt"]["value_added_tax_rate"] == "0.000000000000"
    assert (
        bonds["before-cutoff-exempt"]["gross_coupon_per_period"]
        == bonds["before-cutoff-exempt"]["tax_adjusted_coupon_per_period"]
    )
    for case_id in ["cutoff-day-taxable", "reissuance-inherits-taxable-first-issue"]:
        case = bonds[case_id]
        assert case["value_added_tax_rate"] == "0.060000000000"
        assert Decimal(case["tax_adjusted_coupon_per_period"]) < Decimal(
            case["gross_coupon_per_period"]
        )
        assert Decimal(case["subject_tax_adjusted_yield_to_maturity"]) < Decimal(
            case["market_pre_tax_yield_to_maturity"]
        )

    baskets = {basket["basket_id"]: basket for basket in result["delivery_baskets"]}
    reversal = baskets["market-subject-ctd-reversal"]
    assert reversal["market_ctd_bond_id"] == "CGB-TAXABLE"
    assert reversal["subject_ctd_bond_id"] == "CGB-EXEMPT"
    control = baskets["no-tax-difference-control"]
    assert control["market_ctd_bond_id"] == control["subject_ctd_bond_id"]

    permuted = copy.deepcopy(inputs)
    for basket in permuted["delivery_baskets"]:
        basket["candidates"].reverse()
    permuted_results = {
        basket["basket_id"]: (
            basket["market_ctd_bond_id"],
            basket["subject_ctd_bond_id"],
        )
        for basket in build_expected(permuted)["delivery_baskets"]
    }
    assert permuted_results == {
        basket_id: (value["market_ctd_bond_id"], value["subject_ctd_bond_id"])
        for basket_id, value in baskets.items()
    }


@pytest.mark.parametrize(
    ("field", "drift"),
    [
        ("semantic_sha256", "00" * 32),
        ("type_url", "type.googleapis.com/drift"),
        ("source", "unapproved"),
        ("rate_unit", "01K2CGBVAT0000000000000000@2"),
        ("cutoff", "2025-08-09"),
        ("value_added_tax_rate", "0.05"),
        ("gross_coupon_basis", "VAT_EXCLUDED"),
        ("rounding", "HALF_UP"),
        ("claim_scope", "UNSPECIFIED"),
    ],
)
def test_authority_envelope_drift_fails_closed(field: str, drift: str) -> None:
    inputs, _ = _documents()
    inputs["authority"][field] = drift
    with pytest.raises(ValueError, match="authority envelope drift"):
        build_expected(inputs)


def test_candidate_tax_attribute_drift_fails_closed() -> None:
    inputs, _ = _documents()
    inputs["delivery_baskets"][0]["candidates"][1]["value_added_tax_status"] = "EXEMPT"
    with pytest.raises(ValueError, match="tax attributes"):
        build_expected(inputs)


def test_oracle_imports_only_standard_decimal_witness_dependencies() -> None:
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
    assert imported_roots == {"__future__", "decimal", "typing"}
