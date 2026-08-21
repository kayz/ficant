import ast
import json
from decimal import Decimal
from pathlib import Path

import pytest

from r8a_portfolio_metric_decimal_oracle import (
    aggregate_scope,
    build_expected,
    render_decimal,
    scaled_inputs,
)


ROOT = Path(__file__).resolve().parents[3]
INPUTS = ROOT / "tests/oracle/portfolio360/r8a_portfolio_metric_inputs.json"
EXPECTED = ROOT / "tests/oracle/portfolio360/r8a_portfolio_metric_expected.json"
ORACLE = Path(__file__).with_name("r8a_portfolio_metric_decimal_oracle.py")


def _documents() -> tuple[dict, dict]:
    return (
        json.loads(INPUTS.read_text(encoding="utf-8")),
        json.loads(EXPECTED.read_text(encoding="utf-8")),
    )


def _clone(document: dict) -> dict:
    return json.loads(json.dumps(document))


def test_independent_decimal_oracle_matches_frozen_expected_exactly() -> None:
    inputs, expected = _documents()
    assert build_expected(inputs) == expected


def test_fixture_covers_two_portfolios_three_cgb_bonds_three_factors_and_benchmark() -> None:
    inputs, _ = _documents()

    assert len(inputs["portfolios"]) == 2
    assert len(inputs["bonds"]) == 3
    assert {bond["instrument_type"] for bond in inputs["bonds"]} == {"CGB"}
    assert len(inputs["factors"]) == 3
    assert inputs["benchmark"]["benchmark_id"]
    assert inputs["benchmark"]["position_snapshot_ref"]["content_hash"].startswith(
        "sha256:"
    )


def test_scope_metrics_use_the_frozen_public_weighting_convention() -> None:
    inputs, _ = _documents()
    scope = aggregate_scope(inputs)

    bonds = {bond["instrument_id"]: bond for bond in inputs["bonds"]}
    positions = [
        position
        for portfolio in inputs["portfolios"]
        for position in portfolio["positions"]
    ]

    market_values = [
        Decimal(position["quantity"])
        * Decimal(bonds[position["instrument_id"]]["market_value_per_quantity"])
        for position in positions
    ]
    notionals = [
        Decimal(position["quantity"])
        * Decimal(bonds[position["instrument_id"]]["notional_per_quantity"])
        for position in positions
    ]
    market_value_total = sum(market_values, Decimal(0))
    ytm_denominator = sum(
        (
            market_value
            * Decimal(bonds[position["instrument_id"]]["modified_duration"])
        )
        for market_value, position in zip(market_values, positions, strict=True)
    )
    notional_total = sum(notionals, Decimal(0))
    expected_ytm = sum(
        (
            market_value
            * Decimal(bonds[position["instrument_id"]]["modified_duration"])
            * Decimal(bonds[position["instrument_id"]]["ytm"])
        )
        for market_value, position in zip(market_values, positions, strict=True)
    ) / ytm_denominator
    expected_duration = sum(
        (
            market_value
            * Decimal(bonds[position["instrument_id"]]["modified_duration"])
        )
        for market_value, position in zip(market_values, positions, strict=True)
    ) / market_value_total
    expected_convexity = sum(
        (
            market_value * Decimal(bonds[position["instrument_id"]]["convexity"])
        )
        for market_value, position in zip(market_values, positions, strict=True)
    ) / market_value_total
    expected_coupon = sum(
        (
            notional * Decimal(bonds[position["instrument_id"]]["coupon_rate"])
        )
        for notional, position in zip(notionals, positions, strict=True)
    ) / notional_total
    expected_remaining_years = sum(
        (
            notional * Decimal(bonds[position["instrument_id"]]["remaining_years"])
        )
        for notional, position in zip(notionals, positions, strict=True)
    ) / notional_total
    scale = inputs["output_scale"]

    assert scope["basic_metrics"]["market_value"] == render_decimal(
        market_value_total, scale
    )
    assert scope["basic_metrics"]["weighted_ytm"] == render_decimal(
        expected_ytm, scale
    )
    assert scope["basic_metrics"]["modified_duration"] == render_decimal(
        expected_duration, scale
    )
    assert scope["basic_metrics"]["convexity"] == render_decimal(
        expected_convexity, scale
    )
    assert scope["basic_metrics"]["weighted_coupon_rate"] == render_decimal(
        expected_coupon, scale
    )
    assert scope["basic_metrics"]["weighted_remaining_years"] == render_decimal(
        expected_remaining_years, scale
    )


def test_quantity_linearity_and_inverse_position_sign_are_exact() -> None:
    inputs, _ = _documents()
    base = aggregate_scope(inputs)
    multiplier = Decimal("3")
    scaled = aggregate_scope(scaled_inputs(inputs, multiplier))
    inverse = aggregate_scope(scaled_inputs(inputs, Decimal("-1")))

    for metric in ("market_value", "economic_pnl", "dv01"):
        base_value = Decimal(base["basic_metrics"][metric])
        assert Decimal(scaled["basic_metrics"][metric]) == base_value * multiplier
        assert Decimal(inverse["basic_metrics"][metric]) == -base_value

    for metric in (
        "weighted_ytm",
        "modified_duration",
        "convexity",
        "weighted_coupon_rate",
        "weighted_remaining_years",
    ):
        assert scaled["basic_metrics"][metric] == base["basic_metrics"][metric]
        assert metric not in inverse["basic_metrics"]

    for base_node, scaled_node, inverse_node in zip(
        base["krd_summary"]["factor_totals"],
        scaled["krd_summary"]["factor_totals"],
        inverse["krd_summary"]["factor_totals"],
        strict=True,
    ):
        base_value = Decimal(base_node["dv01"])
        assert Decimal(scaled_node["dv01"]) == base_value * multiplier
        assert Decimal(inverse_node["dv01"]) == -base_value

    assert inverse["data_mode"] == "PARTIAL"
    assert inverse["coverage"]["missing_reasons"] == [
        "short_or_non_positive_position_excluded_from_weighted_averages"
    ]


def test_parallel_dv01_equals_three_factor_krd_sum_for_every_result() -> None:
    inputs, _ = _documents()
    expected = build_expected(inputs)
    aggregates = [
        *expected["portfolios"],
        expected["scope"],
        expected["benchmark"],
    ]

    for aggregate in aggregates:
        node_sum = sum(
            (Decimal(node["dv01"]) for node in aggregate["krd_summary"]["factor_totals"]),
            Decimal(0),
        )
        assert node_sum == Decimal(aggregate["krd_summary"]["parallel_dv01"])
        assert node_sum == Decimal(aggregate["basic_metrics"]["dv01"])


def test_benchmark_differences_are_exact_portfolio_minus_benchmark() -> None:
    inputs, _ = _documents()
    expected = build_expected(inputs)
    benchmark = expected["benchmark"]
    portfolios = {item["aggregate_id"]: item for item in expected["portfolios"]}

    for difference in expected["benchmark_differences"]:
        portfolio = portfolios[difference["portfolio_id"]]
        for metric, value in difference["basic_metrics"].items():
            assert Decimal(value) == Decimal(portfolio["basic_metrics"][metric]) - Decimal(
                benchmark["basic_metrics"][metric]
            )
        benchmark_nodes = {
            node["factor_id"]: Decimal(node["dv01"])
            for node in benchmark["krd_summary"]["factor_totals"]
        }
        portfolio_nodes = {
            node["factor_id"]: Decimal(node["dv01"])
            for node in portfolio["krd_summary"]["factor_totals"]
        }
        for node in difference["krd_factor_differences"]:
            assert Decimal(node["dv01"]) == (
                portfolio_nodes[node["factor_id"]]
                - benchmark_nodes[node["factor_id"]]
            )


def test_final_decimal_rounding_is_ties_to_even_without_epsilon() -> None:
    assert render_decimal(Decimal("2.345"), 2) == "2.34"
    assert render_decimal(Decimal("2.355"), 2) == "2.36"
    assert render_decimal(Decimal("-2.345"), 2) == "-2.34"
    assert render_decimal(Decimal("-0.000"), 2) == "0.00"


def test_zero_denominator_fails_closed() -> None:
    inputs, _ = _documents()
    invalid = _clone(inputs)
    for bond in invalid["bonds"]:
        bond["modified_duration"] = "0"

    with pytest.raises(ValueError, match="ytm weighting denominator is zero"):
        aggregate_scope(invalid)


def test_missing_authority_fails_closed_before_any_metric_is_emitted() -> None:
    inputs, _ = _documents()
    invalid = _clone(inputs)
    del invalid["authority"]["metric_convention_ref"]["content_hash"]

    with pytest.raises(
        ValueError, match="authority.metric_convention_ref.content_hash is required"
    ):
        build_expected(invalid)


def test_mixed_currency_units_and_missing_bond_authority_fail_closed() -> None:
    inputs, _ = _documents()
    mixed_unit = _clone(inputs)
    mixed_unit["bonds"][0]["currency_unit"] = "USD"
    with pytest.raises(ValueError, match="currency unit drift"):
        aggregate_scope(mixed_unit)

    missing_metric = _clone(inputs)
    del missing_metric["bonds"][1]["modified_duration"]
    with pytest.raises(ValueError, match="modified_duration is required"):
        aggregate_scope(missing_metric)


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
    assert not any(
        isinstance(node, ast.Constant) and isinstance(node.value, float)
        for node in ast.walk(tree)
    )
    assert "float(" not in ORACLE.read_text(encoding="utf-8")

