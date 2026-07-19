from __future__ import annotations

import calendar
from dataclasses import dataclass
from datetime import date
from decimal import Context, Decimal, ROUND_HALF_UP
from typing import Any

CONTEXT = Context(prec=50, rounding=ROUND_HALF_UP)
ONE = Decimal(1)
STANDARD_COUPON = Decimal("0.03")
CF_QUANTUM = Decimal("0.0001")
AI_QUANTUM = Decimal("0.0000001")
OUTPUT_QUANTUM = Decimal("0.000000000001")


@dataclass(frozen=True)
class ScheduleMetrics:
    months_to_next_coupon: int
    remaining_coupon_count: int
    purchase_accrued_interest: Decimal
    delivery_accrued_interest: Decimal
    interim_coupons: Decimal


def parse_date(value: str) -> date:
    return date.fromisoformat(value)


def add_months(value: date, months: int) -> date:
    total = value.year * 12 + value.month - 1 + months
    year, month_index = divmod(total, 12)
    month = month_index + 1
    day = min(value.day, calendar.monthrange(year, month)[1])
    return date(year, month, day)


def coupon_dates(issue: date, maturity: date, frequency: int) -> list[date]:
    period_months = 12 // frequency
    values: list[date] = []
    current = maturity
    while current > issue:
        values.append(current)
        current = add_months(current, -period_months)
    if current != issue:
        raise ValueError("irregular first coupon is outside the frozen reference profile")
    return list(reversed(values))


def accrued(coupons: list[date], issue: date, on_date: date, coupon_amount: Decimal) -> Decimal:
    next_index = next(index for index, value in enumerate(coupons) if value > on_date)
    previous = issue if next_index == 0 else coupons[next_index - 1]
    elapsed = Decimal((on_date - previous).days)
    period = Decimal((coupons[next_index] - previous).days)
    return CONTEXT.divide(CONTEXT.multiply(coupon_amount, elapsed), period).quantize(
        AI_QUANTUM, rounding=ROUND_HALF_UP
    )


def schedule_metrics(case: dict[str, Any], common: dict[str, Any]) -> ScheduleMetrics:
    issue = parse_date(case["issue_date"])
    maturity = parse_date(case["maturity_date"])
    purchase = parse_date(common["purchase_date"])
    delivery_month = parse_date(common["delivery_month_first"])
    delivery = parse_date(common["delivery_date"])
    frequency = int(case["frequency"])
    coupons = coupon_dates(issue, maturity, frequency)
    conversion_index = next(index for index, value in enumerate(coupons) if value >= delivery_month)
    next_coupon = coupons[conversion_index]
    months = (next_coupon.year - delivery_month.year) * 12 + next_coupon.month - delivery_month.month
    coupon_amount = CONTEXT.divide(
        CONTEXT.multiply(Decimal(case["coupon_rate"]), Decimal(100)), Decimal(frequency)
    )
    interim = CONTEXT.multiply(
        coupon_amount,
        Decimal(sum(1 for value in coupons if purchase < value <= delivery)),
    )
    return ScheduleMetrics(
        months_to_next_coupon=months,
        remaining_coupon_count=len(coupons) - conversion_index,
        purchase_accrued_interest=accrued(coupons, issue, purchase, coupon_amount),
        delivery_accrued_interest=accrued(coupons, issue, delivery, coupon_amount),
        interim_coupons=interim,
    )


def conversion_factor(case: dict[str, Any], schedule: ScheduleMetrics) -> Decimal:
    frequency = Decimal(case["frequency"])
    coupon = Decimal(case["coupon_rate"])
    stub = CONTEXT.divide(
        CONTEXT.multiply(Decimal(schedule.months_to_next_coupon), frequency), Decimal(12)
    )
    base = CONTEXT.add(ONE, CONTEXT.divide(STANDARD_COUPON, frequency))
    first_discount = CONTEXT.power(base, CONTEXT.minus(stub))
    tail_discount = CONTEXT.power(base, Decimal(-(schedule.remaining_coupon_count - 1)))
    bracket = CONTEXT.add(
        CONTEXT.add(CONTEXT.divide(coupon, frequency), CONTEXT.divide(coupon, STANDARD_COUPON)),
        CONTEXT.multiply(CONTEXT.subtract(ONE, CONTEXT.divide(coupon, STANDARD_COUPON)), tail_discount),
    )
    accrued_adjustment = CONTEXT.multiply(
        CONTEXT.divide(coupon, frequency), CONTEXT.subtract(ONE, stub)
    )
    raw = CONTEXT.subtract(CONTEXT.multiply(first_discount, bracket), accrued_adjustment)
    return raw.quantize(CF_QUANTUM, rounding=ROUND_HALF_UP)


def calculate(case: dict[str, Any], common: dict[str, Any]) -> dict[str, str | int]:
    schedule = schedule_metrics(case, common)
    factor = conversion_factor(case, schedule)
    spot = Decimal(case["spot_clean_price"])
    futures = Decimal(common["futures_clean_price"])
    financing_rate = Decimal(common["financing_rate"])
    actual_days = Decimal((parse_date(common["delivery_date"]) - parse_date(common["purchase_date"])).days)
    converted_futures = CONTEXT.multiply(futures, factor)
    invoice = CONTEXT.add(converted_futures, schedule.delivery_accrued_interest)
    purchase_dirty = CONTEXT.add(spot, schedule.purchase_accrued_interest)
    gross_basis = CONTEXT.subtract(spot, converted_futures)
    financing_cost = CONTEXT.divide(
        CONTEXT.multiply(CONTEXT.multiply(purchase_dirty, financing_rate), actual_days), Decimal(365)
    )
    holding_carry = CONTEXT.subtract(
        CONTEXT.add(
            CONTEXT.subtract(schedule.delivery_accrued_interest, schedule.purchase_accrued_interest),
            schedule.interim_coupons,
        ),
        financing_cost,
    )
    net_basis = CONTEXT.subtract(gross_basis, holding_carry)
    irr = CONTEXT.multiply(
        CONTEXT.subtract(
            CONTEXT.divide(CONTEXT.add(invoice, schedule.interim_coupons), purchase_dirty), ONE
        ),
        CONTEXT.divide(Decimal(365), actual_days),
    )
    values: dict[str, Decimal | int] = {
        "months_to_next_coupon": schedule.months_to_next_coupon,
        "remaining_coupon_count": schedule.remaining_coupon_count,
        "conversion_factor": factor,
        "purchase_accrued_interest": schedule.purchase_accrued_interest,
        "delivery_accrued_interest": schedule.delivery_accrued_interest,
        "interim_coupons": schedule.interim_coupons,
        "invoice_price": invoice,
        "purchase_dirty_price": purchase_dirty,
        "gross_basis": gross_basis,
        "financing_cost": financing_cost,
        "holding_carry": holding_carry,
        "net_basis": net_basis,
        "implied_repo_rate": irr,
        "delivery_profit": CONTEXT.minus(net_basis),
    }
    return {
        key: value if isinstance(value, int) else format(value.quantize(OUTPUT_QUANTUM), "f")
        for key, value in values.items()
    }


def build_expected(inputs: dict[str, Any]) -> dict[str, Any]:
    case_results = {case["id"]: calculate(case, inputs) for case in inputs["cases"]}
    basket_results = {case["bond_id"]: calculate(case, inputs) for case in inputs["t_basket"]}
    ctd = min(
        inputs["t_basket"],
        key=lambda case: (
            -Decimal(str(basket_results[case["bond_id"]]["implied_repo_rate"])),
            Decimal(str(basket_results[case["bond_id"]]["net_basis"])),
            case["bond_id"],
        ),
    )["bond_id"]
    return {
        "schema": "ficant.phase2c-futures-delivery.expected.v1",
        "case_results": case_results,
        "basket_results": basket_results,
        "ctd_bond_id": ctd,
    }
