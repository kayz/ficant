"""Independent Decimal witness for the frozen R8B daily performance convention.

This module intentionally imports no FICANT implementation. It consumes only the public JSON
fixture and Python Decimal, aggregates member NAV/flow before return calculation, and rounds every
division and cumulative multiplication to scale 12 with ties-to-even.
"""

from __future__ import annotations

import json
from decimal import Context, Decimal, InvalidOperation, ROUND_HALF_EVEN, localcontext
from pathlib import Path
from typing import Any


CONTEXT = Context(prec=80, rounding=ROUND_HALF_EVEN)
QUANTUM = Decimal("0.000000000001")
ONE = Decimal(1)
ZERO = Decimal(0)
REQUIRED_CONVENTION = {
    "return_method": "DAILY_TIME_WEIGHTED",
    "flow_timing": "END_OF_DAY",
    "valuation_frequency": "CALENDAR_SESSION_CLOSE",
    "rounding": "TIES_TO_EVEN",
    "cumulative_method": "GEOMETRIC_STEPWISE",
}


def _decimal(value: Any, path: str) -> Decimal:
    if not isinstance(value, str):
        raise ValueError(f"{path} must be a Decimal string")
    try:
        result = Decimal(value)
    except InvalidOperation as error:
        raise ValueError(f"{path} must be a finite Decimal string") from error
    if not result.is_finite():
        raise ValueError(f"{path} must be finite")
    return result


def _round(value: Decimal) -> Decimal:
    with localcontext(CONTEXT):
        return value.quantize(QUANTUM, rounding=ROUND_HALF_EVEN)


def _render(value: Decimal) -> str:
    rounded = _round(value)
    if rounded == ZERO:
        rounded = ZERO
    return format(rounded, ".12f")


def _series(inputs: dict[str, Any], prefix: str) -> tuple[list[Decimal], list[Decimal]]:
    nav_raw = inputs.get(f"{prefix}_nav")
    flow_raw = inputs.get(f"{prefix}_flow")
    if not isinstance(nav_raw, list) or not isinstance(flow_raw, list):
        raise ValueError(f"{prefix} NAV and flow arrays are required")
    if len(nav_raw) != 3 or len(flow_raw) != 3:
        raise ValueError(f"{prefix} must contain exactly three sessions")
    nav = [_decimal(value, f"{prefix}_nav[{index}]") for index, value in enumerate(nav_raw)]
    flow = [
        _decimal(value, f"{prefix}_flow[{index}]") for index, value in enumerate(flow_raw)
    ]
    if any(value <= ZERO for value in nav):
        raise ValueError(f"{prefix} NAV must remain positive")
    return nav, flow


def _calculate(
    nav: list[Decimal], flow: list[Decimal], benchmark: list[Decimal]
) -> dict[str, list[str]]:
    result: dict[str, list[str]] = {
        key: []
        for key in (
            "opening_nav",
            "ending_nav",
            "net_external_flow",
            "economic_pnl",
            "daily_return",
            "benchmark_return",
            "active_return",
            "cumulative_return",
            "benchmark_cumulative_return",
            "active_cumulative_return",
        )
    }
    cumulative_factor = ONE
    benchmark_cumulative_factor = ONE
    with localcontext(CONTEXT):
        for index in range(1, len(nav)):
            pnl = nav[index] - flow[index] - nav[index - 1]
            daily_return = _round(pnl / nav[index - 1])
            benchmark_return = _round(
                (benchmark[index] - benchmark[index - 1]) / benchmark[index - 1]
            )
            cumulative_factor = _round(cumulative_factor * _round(ONE + daily_return))
            benchmark_cumulative_factor = _round(
                benchmark_cumulative_factor * _round(ONE + benchmark_return)
            )
            cumulative_return = cumulative_factor - ONE
            benchmark_cumulative_return = benchmark_cumulative_factor - ONE
            values = {
                "opening_nav": nav[index - 1],
                "ending_nav": nav[index],
                "net_external_flow": flow[index],
                "economic_pnl": pnl,
                "daily_return": daily_return,
                "benchmark_return": benchmark_return,
                "active_return": daily_return - benchmark_return,
                "cumulative_return": cumulative_return,
                "benchmark_cumulative_return": benchmark_cumulative_return,
                "active_cumulative_return": cumulative_return - benchmark_cumulative_return,
            }
            for key, value in values.items():
                result[key].append(_render(value))
    return result


def build_expected(inputs: dict[str, Any]) -> dict[str, Any]:
    """Build the exact expected document from the public R8B fixture."""

    if inputs.get("schema_id") != "ficant.portfolio.performance-oracle-input.v1":
        raise ValueError("unexpected R8B input schema")
    if inputs.get("scale") != 12:
        raise ValueError("R8B Oracle requires scale 12")
    convention = inputs.get("convention")
    if not isinstance(convention, dict) or convention != REQUIRED_CONVENTION:
        raise ValueError("R8B convention drift")
    dates = inputs.get("session_local_dates")
    if not isinstance(dates, list) or len(dates) != 3 or len(set(dates)) != 3:
        raise ValueError("three distinct ordered Calendar sessions are required")
    if dates != sorted(dates):
        raise ValueError("Calendar sessions must be strictly ordered")
    if inputs.get("portfolio_ids") != ["PORTFOLIO-A", "PORTFOLIO-B"]:
        raise ValueError("the frozen two-member scope must remain exact")

    portfolio_a = _series(inputs, "portfolio_a")
    portfolio_b = _series(inputs, "portfolio_b")
    benchmark_raw = inputs.get("benchmark_levels")
    if not isinstance(benchmark_raw, list) or len(benchmark_raw) != 3:
        raise ValueError("Benchmark must cover every Calendar session")
    benchmark = [
        _decimal(value, f"benchmark_levels[{index}]")
        for index, value in enumerate(benchmark_raw)
    ]
    if any(value <= ZERO for value in benchmark):
        raise ValueError("Benchmark levels must remain positive")

    group_nav = [left + right for left, right in zip(portfolio_a[0], portfolio_b[0], strict=True)]
    group_flow = [
        left + right for left, right in zip(portfolio_a[1], portfolio_b[1], strict=True)
    ]
    expected: dict[str, Any] = {
        "schema_id": "ficant.portfolio.performance-oracle-expected.v1",
        "scale": 12,
        "point_local_dates": dates[1:],
    }
    for prefix, (nav, flow) in {
        "portfolio_a": portfolio_a,
        "portfolio_b": portfolio_b,
        "group": (group_nav, group_flow),
    }.items():
        for metric, values in _calculate(nav, flow, benchmark).items():
            expected[f"{prefix}_{metric}"] = values
    return expected


def main() -> None:
    root = Path(__file__).resolve().parents[3]
    inputs = json.loads(
        (root / "tests/oracle/portfolio/r8b_portfolio_performance_inputs.json").read_text(
            encoding="utf-8"
        )
    )
    print(json.dumps(build_expected(inputs), indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
