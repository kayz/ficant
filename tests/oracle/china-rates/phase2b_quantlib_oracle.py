"""Independent QuantLib 1.42.1 oracle for frozen Phase 2B cases.

Run with the official ``QuantLib==1.42.1`` Python distribution on ``PYTHONPATH``.
This test-only path imports no FICANT production package or native library.
"""

from __future__ import annotations

import json

import QuantLib as ql


VALUATION = ql.Date(19, 7, 2026)
CURVE_NODES = [
    (ql.Date(1, 1, 2027), 0.0125),
    (ql.Date(20, 7, 2027), 0.0175),
    (ql.Date(1, 1, 2028), 0.0190),
    (ql.Date(1, 1, 2029), 0.0225),
    (ql.Date(19, 7, 2030), 0.0300),
]


def curve_yield(query: ql.Date) -> float:
    interpolation = ql.LinearInterpolation(
        [float(node.serialNumber()) for node, _ in CURVE_NODES],
        [yield_to_maturity for _, yield_to_maturity in CURVE_NODES],
    )
    return interpolation(float(query.serialNumber()), False)


def make_bond(case: dict):
    calendar = ql.WeekendsOnly()
    if case["coupon"] == 0.0:
        return ql.ZeroCouponBond(
            0,
            calendar,
            100.0,
            case["maturity"],
            ql.Following,
            100.0,
            case["issue"],
        )
    schedule = ql.Schedule(
        case["issue"],
        case["maturity"],
        ql.Period(case["frequency"]),
        calendar,
        ql.Unadjusted,
        ql.Unadjusted,
        ql.DateGeneration.Backward,
        False,
    )
    day_counter = ql.ActualActual(ql.ActualActual.Bond, schedule)
    return ql.FixedRateBond(
        0,
        100.0,
        schedule,
        [case["coupon"]],
        day_counter,
        ql.Following,
        100.0,
        case["issue"],
        calendar,
    )


def day_counter(case: dict):
    if case["coupon"] == 0.0:
        return ql.ActualActual(ql.ActualActual.ISDA)
    schedule = ql.Schedule(
        case["issue"],
        case["maturity"],
        ql.Period(case["frequency"]),
        ql.WeekendsOnly(),
        ql.Unadjusted,
        ql.Unadjusted,
        ql.DateGeneration.Backward,
        False,
    )
    return ql.ActualActual(ql.ActualActual.Bond, schedule)


def nominal_valuation_leg(case: dict, bond, settlement: ql.Date):
    result = ql.Leg()
    for flow in bond.cashflows():
        if flow.date() <= settlement:
            continue
        coupon = ql.as_coupon(flow)
        nominal_date = coupon.accrualEndDate() if coupon else case["maturity"]
        result.append(ql.SimpleCashFlow(flow.amount(), nominal_date))
    return result


def dirty_price(case: dict, settlement: ql.Date, yield_to_maturity: float) -> float:
    bond = make_bond(case)
    leg = nominal_valuation_leg(case, bond, settlement)
    discount_bond = case["coupon"] == 0.0
    rate = ql.InterestRate(
        yield_to_maturity,
        day_counter(case),
        ql.Simple if discount_bond else ql.Compounded,
        ql.Annual if discount_bond else case["frequency"],
    )
    return ql.CashFlows.npv(leg, rate, True, settlement, settlement)


def paid_cashflows(case: dict) -> float:
    return sum(
        flow.amount()
        for flow in make_bond(case).cashflows()
        if case["initial_settlement"] < flow.date() <= case["horizon_settlement"]
    )


def carry_result(case: dict) -> dict:
    initial_query = VALUATION + (
        case["maturity"].serialNumber() - case["initial_settlement"].serialNumber()
    )
    rolled_query = VALUATION + (
        case["maturity"].serialNumber() - case["horizon_settlement"].serialNumber()
    )
    initial_yield = curve_yield(initial_query)
    rolled_yield = curve_yield(rolled_query)
    initial_dirty = dirty_price(case, case["initial_settlement"], initial_yield)
    horizon_initial = dirty_price(case, case["horizon_settlement"], initial_yield)
    horizon_rolled = dirty_price(case, case["horizon_settlement"], rolled_yield)
    paid = paid_cashflows(case)
    carry = horizon_initial + paid - initial_dirty
    roll_down = horizon_rolled - horizon_initial
    return {
        "initial_curve_query_date": initial_query.ISO(),
        "rolled_curve_query_date": rolled_query.ISO(),
        "initial_yield": initial_yield,
        "rolled_yield": rolled_yield,
        "initial_dirty_price": initial_dirty,
        "horizon_dirty_at_initial_yield": horizon_initial,
        "horizon_dirty_at_rolled_yield": horizon_rolled,
        "paid_cashflows": paid,
        "carry": carry,
        "roll_down": roll_down,
        "total_return": carry + roll_down,
    }


def build_output() -> dict:
    ql.Settings.instance().evaluationDate = VALUATION
    cases = [
        {
            "id": "CARRY-COUPON-UPWARD",
            "issue": ql.Date(1, 1, 2026),
            "maturity": ql.Date(1, 1, 2029),
            "coupon": 0.0200,
            "frequency": ql.Annual,
            "initial_settlement": ql.Date(20, 7, 2026),
            "horizon_settlement": ql.Date(2, 1, 2027),
        },
        {
            "id": "CARRY-DISCOUNT-UPWARD",
            "issue": ql.Date(1, 1, 2026),
            "maturity": ql.Date(31, 12, 2029),
            "coupon": 0.0,
            "frequency": ql.Annual,
            "initial_settlement": ql.Date(20, 7, 2026),
            "horizon_settlement": ql.Date(20, 7, 2027),
        },
    ]
    curve_cases = {
        "CURVE-EXACT-NODE": ql.Date(1, 1, 2028),
        "CURVE-EXACT-MIDPOINT": ql.Date(11, 4, 2027),
        "CURVE-UNEVEN-INTERVAL": ql.Date(1, 10, 2027),
    }
    return {
        "schema": "ficant.test-oracle.phase2b-quantlib-output.v1",
        "quantlib_version": ql.__version__,
        "convention": "cfets-ytm-carry-roll-v1",
        "curve_results": {
            case_id: {
                "query_date": query.ISO(),
                "yield_to_maturity": curve_yield(query),
            }
            for case_id, query in curve_cases.items()
        },
        "carry_results": {case["id"]: carry_result(case) for case in cases},
    }


if __name__ == "__main__":
    print(json.dumps(build_output(), indent=2, sort_keys=True))
