"""Independent Decimal witness for the frozen R5D portfolio KRD fixture.

This module deliberately depends only on Python's standard-library Decimal primitives.  It
constructs prices, finite differences, position scaling, aggregation, and metamorphic witnesses
from the public JSON fixture; no FICANT module or earlier oracle helper is imported.
"""

from __future__ import annotations

from decimal import Context, Decimal, ROUND_HALF_EVEN, localcontext
from typing import Any

CONTEXT = Context(prec=50, rounding=ROUND_HALF_EVEN)
OUTPUT_QUANTUM = Decimal("0.000000000001")
BP_PER_UNIT = Decimal("10000")
TWO = Decimal("2")


def _decimal(value: str) -> Decimal:
    return Decimal(value)


def _render(value: Decimal) -> str:
    quantized = value.quantize(OUTPUT_QUANTUM, rounding=ROUND_HALF_EVEN)
    if quantized == 0:
        quantized = abs(quantized)
    return format(quantized, "f")


def _reference_price(inputs: dict[str, Any], yields: list[Decimal]) -> Decimal:
    model = inputs["reference_price_model"]
    weights = [_decimal(value) for value in model["curve_interpolation_weights"]]
    if len(weights) != len(yields):
        raise ValueError("one interpolation weight is required for every curve node")
    effective_yield = sum(
        (weight * node_yield for weight, node_yield in zip(weights, yields, strict=True)),
        Decimal(0),
    )
    return _decimal(model["registered_face_price"]) - (
        _decimal(model["yield_price_multiplier"]) * effective_yield
    )


def _central_registered_face_dv01(
    inputs: dict[str, Any],
    base_yields: list[Decimal],
    bumped_indices: tuple[int, ...],
) -> Decimal:
    factors = inputs["factors"]
    up_yields = base_yields.copy()
    down_yields = base_yields.copy()
    for index in bumped_indices:
        bump = _decimal(factors[index]["bump_yield"])
        up_yields[index] += bump
        down_yields[index] -= bump
    bumps_bp = {_decimal(factors[index]["bump_yield"]) * BP_PER_UNIT for index in bumped_indices}
    if len(bumps_bp) != 1:
        raise ValueError("the parallel witness requires one common bump size")
    bump_bp = bumps_bp.pop()
    up_price = _reference_price(inputs, up_yields)
    down_price = _reference_price(inputs, down_yields)
    return (down_price - up_price) / (TWO * bump_bp)


def _position_dv01(
    position: dict[str, Any], registered_face_dv01: Decimal, quantity_multiplier: Decimal
) -> Decimal:
    if position["kind"] == "bond":
        quantity = _decimal(position["quantity"]) * quantity_multiplier
        return registered_face_dv01 * quantity / _decimal(position["registered_face"])
    if position["kind"] == "futures":
        contracts = _decimal(position["quantity"]) * quantity_multiplier
        quote_krd = (
            registered_face_dv01
            * _decimal(position["face_quote_basis"])
            / _decimal(position["ctd_registered_face"])
        )
        return (
            quote_krd
            * _decimal(position["contract_size_in_quote_units"])
            * contracts
            / _decimal(position["conversion_factor"])
        )
    raise ValueError(f"unsupported position kind: {position['kind']}")


def _calculate_case(inputs: dict[str, Any], quantity_multiplier: Decimal) -> dict[str, Any]:
    factors = inputs["factors"]
    if len(factors) != 3 or any(factor["direction"] != "central" for factor in factors):
        raise ValueError("the R5D witness is frozen to exactly three central factors")
    positions = inputs["positions"]
    if [position["kind"] for position in positions] != ["bond", "futures"]:
        raise ValueError("the R5D witness requires one Bond followed by one Futures position")

    base_yields = [_decimal(factor["base_yield"]) for factor in factors]
    registered_node_dv01 = [
        _central_registered_face_dv01(inputs, base_yields, (index,))
        for index in range(len(factors))
    ]
    registered_parallel_dv01 = _central_registered_face_dv01(
        inputs, base_yields, tuple(range(len(factors)))
    )

    position_results: list[dict[str, Any]] = []
    for position in positions:
        node_values = [
            _position_dv01(position, value, quantity_multiplier)
            for value in registered_node_dv01
        ]
        parallel_value = _position_dv01(
            position, registered_parallel_dv01, quantity_multiplier
        )
        position_results.append(
            {
                "position_id": position["position_id"],
                "instrument_id": position["instrument_id"],
                "kind": position["kind"],
                "nodes": [
                    {"factor_id": factor["factor_id"], "dv01": _render(value)}
                    for factor, value in zip(factors, node_values, strict=True)
                ],
                "node_sum_dv01": _render(sum(node_values, Decimal(0))),
                "parallel_shift_dv01": _render(parallel_value),
            }
        )

    node_totals = [
        sum(
            (
                _decimal(position_result["nodes"][index]["dv01"])
                for position_result in position_results
            ),
            Decimal(0),
        )
        for index in range(len(factors))
    ]
    portfolio_node_sum = sum(node_totals, Decimal(0))
    portfolio_parallel = sum(
        (_decimal(position["parallel_shift_dv01"]) for position in position_results),
        Decimal(0),
    )
    return {
        "positions": position_results,
        "node_totals": [
            {"factor_id": factor["factor_id"], "dv01": _render(total)}
            for factor, total in zip(factors, node_totals, strict=True)
        ],
        "portfolio_node_sum_dv01": _render(portfolio_node_sum),
        "portfolio_parallel_shift_dv01": _render(portfolio_parallel),
    }


def build_expected(inputs: dict[str, Any]) -> dict[str, Any]:
    """Builds the frozen expected document from independent exact-Decimal calculations."""

    with localcontext(CONTEXT):
        base = _calculate_case(inputs, Decimal(1))
        quantity_multiplier = _decimal(inputs["metamorphic_cases"]["quantity_multiplier"])
        scaled = _calculate_case(inputs, quantity_multiplier)
        inverse_multiplier = _decimal(inputs["metamorphic_cases"]["inverse_position_multiplier"])
        inverse = _calculate_case(inputs, inverse_multiplier)
        return {
            "schema": "ficant.r5d-portfolio-krd-oracle.expected.v1",
            "positions": base["positions"],
            "node_totals": base["node_totals"],
            "portfolio": {
                "node_sum_dv01": base["portfolio_node_sum_dv01"],
                "parallel_shift_dv01": base["portfolio_parallel_shift_dv01"],
            },
            "metamorphic_results": {
                "quantity_multiplier": _render(quantity_multiplier),
                "scaled_positions": scaled["positions"],
                "scaled_node_totals": scaled["node_totals"],
                "scaled_portfolio_node_sum_dv01": scaled["portfolio_node_sum_dv01"],
                "scaled_portfolio_parallel_shift_dv01": scaled[
                    "portfolio_parallel_shift_dv01"
                ],
                "inverse_position_multiplier": _render(inverse_multiplier),
                "inverse_positions": inverse["positions"],
                "inverse_node_totals": inverse["node_totals"],
                "inverse_portfolio_node_sum_dv01": inverse["portfolio_node_sum_dv01"],
                "inverse_portfolio_parallel_shift_dv01": inverse[
                    "portfolio_parallel_shift_dv01"
                ],
            },
        }
