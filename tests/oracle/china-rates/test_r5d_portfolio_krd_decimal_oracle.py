import ast
import json
from decimal import Decimal
from pathlib import Path

from r5d_portfolio_krd_decimal_oracle import build_expected

ROOT = Path(__file__).resolve().parents[3]
INPUTS = ROOT / "tests/golden-cases/china-rates/r5d-portfolio-krd-oracle-inputs.json"
EXPECTED = (
    ROOT
    / "tests/golden-cases/china-rates/expected/r5d-portfolio-krd-oracle-expected.json"
)
ORACLE = Path(__file__).with_name("r5d_portfolio_krd_decimal_oracle.py")


def _documents() -> tuple[dict, dict]:
    return (
        json.loads(INPUTS.read_text(encoding="utf-8")),
        json.loads(EXPECTED.read_text(encoding="utf-8")),
    )


def test_independent_decimal_oracle_matches_frozen_expected_exactly() -> None:
    inputs, expected = _documents()
    assert build_expected(inputs) == expected


def test_parallel_shift_quantity_linearity_and_inverse_sign_are_exact() -> None:
    inputs, _ = _documents()
    result = build_expected(inputs)

    assert len(inputs["factors"]) == 3
    assert [position["kind"] for position in inputs["positions"]] == ["bond", "futures"]
    for position in result["positions"]:
        assert Decimal(position["node_sum_dv01"]) == Decimal(position["parallel_shift_dv01"])
    assert Decimal(result["portfolio"]["node_sum_dv01"]) == Decimal(
        result["portfolio"]["parallel_shift_dv01"]
    )

    multiplier = Decimal(result["metamorphic_results"]["quantity_multiplier"])
    scaled_by_id = {
        position["position_id"]: position
        for position in result["metamorphic_results"]["scaled_positions"]
    }
    inverse_by_id = {
        position["position_id"]: position
        for position in result["metamorphic_results"]["inverse_positions"]
    }
    for position in result["positions"]:
        scaled = scaled_by_id[position["position_id"]]
        inverse = inverse_by_id[position["position_id"]]
        for base_node, scaled_node, inverse_node in zip(
            position["nodes"], scaled["nodes"], inverse["nodes"], strict=True
        ):
            base = Decimal(base_node["dv01"])
            assert Decimal(scaled_node["dv01"]) == base * multiplier
            assert Decimal(inverse_node["dv01"]) == -base
        assert Decimal(scaled["parallel_shift_dv01"]) == (
            Decimal(position["parallel_shift_dv01"]) * multiplier
        )
        assert Decimal(inverse["parallel_shift_dv01"]) == -Decimal(
            position["parallel_shift_dv01"]
        )

    for base_total, scaled_total, inverse_total in zip(
        result["node_totals"],
        result["metamorphic_results"]["scaled_node_totals"],
        result["metamorphic_results"]["inverse_node_totals"],
        strict=True,
    ):
        base = Decimal(base_total["dv01"])
        assert Decimal(scaled_total["dv01"]) == base * multiplier
        assert Decimal(inverse_total["dv01"]) == -base
    assert Decimal(
        result["metamorphic_results"]["scaled_portfolio_node_sum_dv01"]
    ) == Decimal(result["portfolio"]["node_sum_dv01"]) * multiplier
    assert Decimal(
        result["metamorphic_results"]["scaled_portfolio_parallel_shift_dv01"]
    ) == Decimal(result["portfolio"]["parallel_shift_dv01"]) * multiplier
    assert Decimal(
        result["metamorphic_results"]["inverse_portfolio_node_sum_dv01"]
    ) == -Decimal(result["portfolio"]["node_sum_dv01"])
    assert Decimal(
        result["metamorphic_results"]["inverse_portfolio_parallel_shift_dv01"]
    ) == -Decimal(result["portfolio"]["parallel_shift_dv01"])


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
