"""Independent Decimal oracle for Phase 2B curve and carry/roll-down cases.

This test-only module shares no implementation with the production Rust/C++
path.  It intentionally implements only the frozen Phase 2B convention:
actual-day linear YTM interpolation, weekend-calendar Following adjustment,
Act/Act Bond (ISMA) coupon timing, discount-bond simple yield, and unfunded
holding-period carry/roll-down decomposition.
"""

from __future__ import annotations

import calendar
import json
from datetime import date, timedelta
from decimal import Decimal, ROUND_HALF_EVEN, getcontext
from pathlib import Path
from typing import Any


getcontext().prec = 50
QUANTIZE = Decimal("0.000000000001")


def d(value: Any) -> Decimal:
    return value if isinstance(value, Decimal) else Decimal(str(value))


def decimal12(value: Any) -> str:
    return format(d(value).quantize(QUANTIZE, rounding=ROUND_HALF_EVEN), "f")


def parse_date(value: str) -> date:
    return date.fromisoformat(value)


def following_weekend(value: date) -> date:
    while value.weekday() >= 5:
        value += timedelta(days=1)
    return value


def previous_period(value: date, months: int) -> date:
    year = value.year
    month = value.month - months
    while month <= 0:
        month += 12
        year -= 1
    return date(year, month, min(value.day, calendar.monthrange(year, month)[1]))


def nominal_dates(issue: date, maturity: date, frequency: int) -> list[date]:
    if frequency not in (1, 2):
        raise ValueError("Phase 2B frequency must be annual or semiannual")
    cursor = maturity
    values = []
    while cursor > issue:
        values.append(cursor)
        cursor = previous_period(cursor, 12 // frequency)
    values.reverse()
    return values


def coupon_time(
    issue: date,
    maturity: date,
    frequency: int,
    settlement: date,
    nominal: date,
) -> Decimal:
    schedule = [issue, *nominal_dates(issue, maturity, frequency)]
    next_index = next(index for index in range(1, len(schedule)) if schedule[index] > settlement)
    target_index = schedule.index(nominal)
    previous_date = schedule[next_index - 1]
    next_date = schedule[next_index]
    first_fraction = (
        Decimal((next_date - settlement).days)
        / Decimal((next_date - previous_date).days)
        / Decimal(frequency)
    )
    return first_fraction + Decimal(target_index - next_index) / Decimal(frequency)


def actual_actual_years(start: date, end: date) -> Decimal:
    if end <= start:
        raise ValueError("end must be after start")
    cursor = start
    total = Decimal(0)
    while cursor < end:
        boundary = min(end, date(cursor.year + 1, 1, 1))
        year_days = 366 if calendar.isleap(cursor.year) else 365
        total += Decimal((boundary - cursor).days) / Decimal(year_days)
        cursor = boundary
    return total


def cashflows(bond: dict[str, Any], settlement: date) -> list[dict[str, Any]]:
    issue = parse_date(bond["issue_date"])
    maturity = parse_date(bond["maturity_date"])
    frequency = int(bond["frequency"])
    coupon_rate = d(bond["coupon_rate"])
    face = d(bond["face_value"])
    discount = coupon_rate == 0
    coupon = Decimal(0) if discount else face * coupon_rate / Decimal(frequency)
    result = []
    for nominal in nominal_dates(issue, maturity, frequency):
        payment = following_weekend(nominal)
        if payment <= settlement:
            continue
        principal = face if nominal == maturity else Decimal(0)
        result.append(
            {
                "nominal_date": nominal,
                "payment_date": payment,
                "amount": coupon + principal,
                "time_years": (
                    actual_actual_years(settlement, nominal)
                    if discount
                    else coupon_time(issue, maturity, frequency, settlement, nominal)
                ),
            }
        )
    if not result:
        raise ValueError("bond has no post-settlement cashflows")
    return result


def dirty_price(bond: dict[str, Any], settlement: date, ytm: Decimal) -> Decimal:
    frequency = int(bond["frequency"])
    discount = d(bond["coupon_rate"]) == 0
    total = Decimal(0)
    for flow in cashflows(bond, settlement):
        time_years = flow["time_years"]
        if discount:
            denominator = Decimal(1) + ytm * time_years
            if denominator <= 0:
                raise ValueError("discount yield produces a non-positive denominator")
            factor = Decimal(1) / denominator
        else:
            base = Decimal(1) + ytm / Decimal(frequency)
            if base <= 0:
                raise ValueError("coupon yield produces a non-positive base")
            factor = base ** (-(time_years * Decimal(frequency)))
        total += flow["amount"] * factor
    return total


def linear_yield(curve: dict[str, Any], query_date: date) -> Decimal:
    nodes = [
        (parse_date(node["maturity_date"]), d(node["yield_to_maturity"]))
        for node in curve["nodes"]
    ]
    if query_date < nodes[0][0] or query_date > nodes[-1][0]:
        raise ValueError("query outside frozen curve range")
    for index, (upper_date, upper_yield) in enumerate(nodes):
        if query_date == upper_date:
            return upper_yield
        if query_date < upper_date:
            lower_date, lower_yield = nodes[index - 1]
            weight = Decimal((query_date - lower_date).days) / Decimal(
                (upper_date - lower_date).days
            )
            return lower_yield + weight * (upper_yield - lower_yield)
    raise AssertionError("covered query must resolve")


def curve_query_date(valuation: date, maturity: date, settlement: date) -> date:
    return valuation + (maturity - settlement)


def carry_result(payload: dict[str, Any], case: dict[str, Any]) -> dict[str, Any]:
    valuation = parse_date(payload["valuation_date"])
    bond = case["bond"]
    maturity = parse_date(bond["maturity_date"])
    initial_settlement = parse_date(case["initial_settlement"])
    horizon_settlement = parse_date(case["horizon_settlement"])
    initial_query = curve_query_date(valuation, maturity, initial_settlement)
    rolled_query = curve_query_date(valuation, maturity, horizon_settlement)
    initial_yield = linear_yield(payload["curve"], initial_query)
    rolled_yield = linear_yield(payload["curve"], rolled_query)
    initial_dirty = dirty_price(bond, initial_settlement, initial_yield)
    horizon_initial = dirty_price(bond, horizon_settlement, initial_yield)
    horizon_rolled = dirty_price(bond, horizon_settlement, rolled_yield)
    paid = sum(
        (flow["amount"] for flow in cashflows(bond, initial_settlement)
         if flow["payment_date"] <= horizon_settlement),
        Decimal(0),
    )
    carry = horizon_initial + paid - initial_dirty
    roll_down = horizon_rolled - horizon_initial
    return {
        "initial_curve_query_date": initial_query.isoformat(),
        "rolled_curve_query_date": rolled_query.isoformat(),
        "initial_yield": decimal12(initial_yield),
        "rolled_yield": decimal12(rolled_yield),
        "initial_dirty_price": decimal12(initial_dirty),
        "horizon_dirty_at_initial_yield": decimal12(horizon_initial),
        "horizon_dirty_at_rolled_yield": decimal12(horizon_rolled),
        "paid_cashflows": decimal12(paid),
        "carry": decimal12(carry),
        "roll_down": decimal12(roll_down),
        "total_return": decimal12(carry + roll_down),
    }


def build_expected(payload: dict[str, Any]) -> dict[str, Any]:
    return {
        "schema": "ficant.test-expected.phase2b-curve-carry.v1",
        "convention": "cfets-ytm-carry-roll-v1",
        "tolerances": {
            "dates_metadata_identities": "exact",
            "yield_abs": "0.000000000002",
            "price_and_return_abs": "0.000000010000",
        },
        "provenance": {
            "expected_source": "independent_decimal_python_oracle",
            "production_rust_or_cpp_used": False,
            "manual_formula_layer": "executed",
            "quantlib_required_version": "1.42.1",
            "quantlib_agreement": "pending_independent_execution",
        },
        "curve_results": {
            case["id"]: {
                "query_date": case["query_date"],
                "yield_to_maturity": decimal12(
                    linear_yield(payload["curve"], parse_date(case["query_date"]))
                ),
            }
            for case in payload["curve_cases"]
        },
        "carry_results": {
            case["id"]: carry_result(payload, case) for case in payload["carry_cases"]
        },
    }


def repository_root() -> Path:
    return Path(__file__).resolve().parents[3]


def main() -> None:
    source = repository_root() / "tests/golden-cases/china-rates/phase2b-curve-carry-inputs.json"
    payload = json.loads(source.read_text(encoding="utf-8"))
    print(json.dumps(build_expected(payload), ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
