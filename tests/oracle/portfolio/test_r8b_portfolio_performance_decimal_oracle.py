import json
from copy import deepcopy
from pathlib import Path

import pytest

from r8b_portfolio_performance_decimal_oracle import build_expected


ROOT = Path(__file__).resolve().parents[3]
INPUTS = ROOT / "tests/oracle/portfolio/r8b_portfolio_performance_inputs.json"
EXPECTED = ROOT / "tests/oracle/portfolio/r8b_portfolio_performance_expected.json"


def documents() -> tuple[dict, dict]:
    return (
        json.loads(INPUTS.read_text(encoding="utf-8")),
        json.loads(EXPECTED.read_text(encoding="utf-8")),
    )


def test_independent_decimal_oracle_matches_frozen_expected_exactly() -> None:
    inputs, expected = documents()
    assert build_expected(inputs) == expected
    assert expected["portfolio_a_economic_pnl"][0] == "-2.000000000000"
    assert expected["group_net_external_flow"] == [
        "10.000000000000",
        "-20.000000000000",
    ]


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        (lambda value: value["benchmark_levels"].pop(), "Benchmark"),
        (
            lambda value: value["convention"].update({"flow_timing": "BEGIN_OF_DAY"}),
            "convention",
        ),
        (lambda value: value["portfolio_ids"].reverse(), "two-member"),
    ],
)
def test_oracle_rejects_coverage_convention_and_scope_drift(mutation, message: str) -> None:
    inputs, _ = documents()
    changed = deepcopy(inputs)
    mutation(changed)
    with pytest.raises(ValueError, match=message):
        build_expected(changed)
