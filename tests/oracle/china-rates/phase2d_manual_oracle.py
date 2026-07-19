from __future__ import annotations

from decimal import Context, Decimal, ROUND_CEILING, ROUND_FLOOR, ROUND_HALF_UP
from typing import Any

CONTEXT = Context(prec=50, rounding=ROUND_HALF_UP)
ONE = Decimal(1)
OUTPUT_QUANTUM = Decimal("0.000000000001")


def calculate(case: dict[str, Any], common: dict[str, Any]) -> dict[str, str | int]:
    target = Decimal(case["target_dv01"])
    ctd_dv01 = Decimal(case["ctd_dv01_per_100"])
    conversion_factor = Decimal(case["conversion_factor"])
    notional = Decimal(common["contract_notional"])
    quote_face = Decimal(common["quote_face"])

    futures_dv01 = CONTEXT.divide(
        CONTEXT.multiply(ctd_dv01, CONTEXT.divide(notional, quote_face)),
        conversion_factor,
    )
    raw_contracts = CONTEXT.divide(CONTEXT.minus(target), futures_dv01)
    floor_contracts = int(raw_contracts.to_integral_value(rounding=ROUND_FLOOR))
    ceiling_contracts = int(raw_contracts.to_integral_value(rounding=ROUND_CEILING))

    def residual(contracts: int) -> Decimal:
        return CONTEXT.add(target, CONTEXT.multiply(Decimal(contracts), futures_dv01))

    recommended = min(
        {floor_contracts, ceiling_contracts, 0},
        key=lambda contracts: (abs(residual(contracts)), abs(contracts), contracts),
    )
    residual_dv01 = residual(recommended)
    effectiveness = CONTEXT.subtract(
        ONE,
        CONTEXT.divide(abs(residual_dv01), abs(target)),
    )
    effectiveness = min(ONE, max(Decimal(0), effectiveness))

    def rendered(value: Decimal) -> str:
        return format(value.quantize(OUTPUT_QUANTUM, rounding=ROUND_HALF_UP), "f")

    return {
        "futures_contract_dv01": rendered(futures_dv01),
        "raw_contracts": rendered(raw_contracts),
        "recommended_contracts": recommended,
        "residual_dv01": rendered(residual_dv01),
        "hedge_effectiveness": rendered(effectiveness),
    }


def build_expected(inputs: dict[str, Any]) -> dict[str, Any]:
    return {
        "schema": "ficant.phase2d-futures-hedge.expected.v1",
        "case_results": {case["id"]: calculate(case, inputs) for case in inputs["cases"]},
    }
