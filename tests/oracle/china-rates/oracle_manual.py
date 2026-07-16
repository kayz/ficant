"""Independent manual Oracle for the frozen ``cgb-reference-v1`` contract.

This test-only module intentionally has no dependency on production C++ or on
QuantLib.  QuantLib agreement is a separate, hard-gated integration layer.
"""

import calendar
import importlib.util
import math
import sys
from datetime import date
from decimal import Decimal, ROUND_HALF_EVEN, getcontext
from pathlib import Path
from typing import Dict, List, Optional, Tuple


getcontext().prec = 50
QUANTIZE = Decimal("0.000000000001")

_HERE = Path(__file__).resolve().parent


def _load_local(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, _HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules.setdefault(name, module)
    spec.loader.exec_module(module)
    return module


_calendar = _load_local("cgb_calendar_for_manual_oracle", "calendar_cgb.py")
_daycount = _load_local("cgb_daycount_for_manual_oracle", "daycount.py")
adjust_following = _calendar.adjust_following
actual_actual_discount = _daycount.actual_actual_discount


def d(value) -> Decimal:
    """Convert through text so binary float artifacts are not imported."""
    return value if isinstance(value, Decimal) else Decimal(str(value))


def decimal12(value) -> str:
    """Canonical 12-place round-half-even representation."""
    return format(d(value).quantize(QUANTIZE, rounding=ROUND_HALF_EVEN), "f")


def _previous_period(value: date, months: int) -> date:
    year = value.year
    month = value.month - months
    while month <= 0:
        month += 12
        year -= 1
    return date(year, month, min(value.day, calendar.monthrange(year, month)[1]))


def nominal_coupon_dates(issue_date: date, maturity_date: date, freq: int) -> List[date]:
    if freq == 0:
        return [maturity_date]
    months = 12 // freq
    cursor = maturity_date
    dates = []
    while cursor > issue_date:
        dates.append(cursor)
        cursor = _previous_period(cursor, months)
    dates.reverse()
    return dates


def _coupon_time_years(
    issue_date: date,
    maturity_date: date,
    freq: int,
    settlement_date: date,
    nominal_date: date,
) -> Decimal:
    """Bond/ISMA time from settlement to a nominal coupon date.

    The first stub is the remaining fraction of its reference coupon period;
    later coupons add exact ``1/freq`` periods.  Payment-date adjustment does
    not add interest or discount time.
    """
    if nominal_date <= settlement_date:
        return Decimal(0)
    schedule = [issue_date] + nominal_coupon_dates(issue_date, maturity_date, freq)
    next_index = next(i for i in range(1, len(schedule)) if schedule[i] > settlement_date)
    target_index = schedule.index(nominal_date)
    previous_date = schedule[next_index - 1]
    next_date = schedule[next_index]
    reference_days = Decimal((next_date - previous_date).days)
    remaining_days = Decimal((next_date - settlement_date).days)
    first = remaining_days / reference_days / Decimal(freq)
    return first + Decimal(target_index - next_index) / Decimal(freq)


def generate_cashflows(
    bond_id: str,
    issue_date: date,
    maturity_date: date,
    coupon_rate: float,
    freq: int,
    settlement_date: date,
    face_value: float = 100.0,
) -> List[Dict]:
    """Generate post-settlement cashflows using adjusted payment ownership."""
    if freq not in (0, 1, 2):
        raise ValueError("frequency must be discount, annual, or semiannual")
    if issue_date >= maturity_date:
        raise ValueError("issue date must precede maturity date")

    dates = nominal_coupon_dates(issue_date, maturity_date, freq)
    coupon = Decimal(0) if freq == 0 else d(face_value) * d(coupon_rate) / Decimal(freq)
    cashflows = []
    for nominal in dates:
        payment = adjust_following(nominal)
        if payment <= settlement_date:
            continue
        principal = d(face_value) if nominal == maturity_date else Decimal(0)
        time_years = (
            d(actual_actual_discount(settlement_date, nominal))
            if freq == 0
            else _coupon_time_years(issue_date, maturity_date, freq, settlement_date, nominal)
        )
        cashflows.append({
            "sequence": len(cashflows) + 1,
            "nominal_date": nominal.isoformat(),
            "payment_date": payment.isoformat(),
            "coupon": coupon,
            "principal": principal,
            "total": coupon + principal,
            "time_years": time_years,
        })
    return cashflows


def compute_accrued_interest(
    issue_date: date,
    maturity_date: date,
    coupon_rate: float,
    freq: int,
    settlement_date: date,
    face_value: float = 100.0,
) -> Decimal:
    if freq == 0 or d(coupon_rate) == 0 or settlement_date <= issue_date:
        return Decimal(0)
    schedule = [issue_date] + nominal_coupon_dates(issue_date, maturity_date, freq)
    for index in range(1, len(schedule)):
        previous_date, next_date = schedule[index - 1], schedule[index]
        if settlement_date <= next_date:
            elapsed = Decimal((settlement_date - previous_date).days)
            reference = Decimal((next_date - previous_date).days)
            coupon = d(face_value) * d(coupon_rate) / Decimal(freq)
            return coupon * elapsed / reference
    return Decimal(0)


def compute_pv_given_yield(
    cashflows: List[Dict],
    ytm: float,
    settlement_date: date,
    freq: int,
) -> Tuple[Decimal, Decimal]:
    rate = d(ytm)
    if not math.isfinite(float(rate)):
        raise ValueError("yield must be finite")
    pv = Decimal(0)
    for flow in cashflows:
        if date.fromisoformat(flow["payment_date"]) <= settlement_date:
            continue
        time_years = d(flow["time_years"])
        if freq == 0:
            denominator = Decimal(1) + rate * time_years
            if denominator <= 0:
                raise ValueError("discount yield produces a non-positive denominator")
            discount = Decimal(1) / denominator
        else:
            base = Decimal(1) + rate / Decimal(freq)
            if base <= 0:
                raise ValueError("coupon yield produces a non-positive base")
            discount = base ** (-(time_years * Decimal(freq)))
        pv += d(flow["total"]) * discount
    return pv, pv


def compute_dirty_clean_price(
    cashflows: List[Dict],
    ytm: float,
    settlement_date: date,
    freq: int,
    accrued_interest: Decimal,
) -> Tuple[Decimal, Decimal]:
    dirty, _ = compute_pv_given_yield(cashflows, ytm, settlement_date, freq)
    return dirty, dirty - accrued_interest


def _pv_residual(cashflows, rate, settlement_date, freq, target_dirty) -> float:
    pv, _ = compute_pv_given_yield(cashflows, rate, settlement_date, freq)
    return float(pv - d(target_dirty))


def solve_ytm_from_price(
    cashflows: List[Dict],
    target_dirty: Decimal,
    settlement_date: date,
    freq: int,
    a: float = -0.50,
    b: float = 1.00,
    max_iter: int = 100,
    tol: float = 1e-12,
) -> Optional[float]:
    """Brent root solve with the frozen bracket, limits, and tolerances."""
    fa = _pv_residual(cashflows, a, settlement_date, freq, target_dirty)
    fb = _pv_residual(cashflows, b, settlement_date, freq, target_dirty)
    if fa == 0:
        return a
    if fb == 0:
        return b
    if fa * fb > 0:
        return None

    c, fc = b, fb
    step = previous_step = b - a
    epsilon = sys.float_info.epsilon
    for _ in range(max_iter):
        if fb * fc > 0:
            c, fc = a, fa
            step = previous_step = b - a
        if abs(fc) < abs(fb):
            a, b, c = b, c, b
            fa, fb, fc = fb, fc, fb
        tolerance = 2 * epsilon * abs(b) + tol / 2
        midpoint = (c - b) / 2
        if abs(midpoint) <= tolerance or fb == 0:
            return b
        if abs(previous_step) >= tolerance and abs(fa) > abs(fb):
            ratio = fb / fa
            if a == c:
                p = 2 * midpoint * ratio
                q = 1 - ratio
            else:
                q0 = fa / fc
                r = fb / fc
                p = ratio * (2 * midpoint * q0 * (q0 - r) - (b - a) * (r - 1))
                q = (q0 - 1) * (r - 1) * (ratio - 1)
            if p > 0:
                q = -q
            else:
                p = -p
            if 2 * p < min(3 * midpoint * q - abs(tolerance * q), abs(previous_step * q)):
                previous_step, step = step, p / q
            else:
                step = midpoint
                previous_step = midpoint
        else:
            step = midpoint
            previous_step = midpoint
        a, fa = b, fb
        b += step if abs(step) > tolerance else math.copysign(tolerance, midpoint)
        fb = _pv_residual(cashflows, b, settlement_date, freq, target_dirty)
    return None


def compute_risk_measures(
    cashflows: List[Dict],
    ytm: float,
    settlement_date: date,
    freq: int,
    dirty_price: Decimal,
    face_value: float = 100.0,
) -> Dict:
    price = d(dirty_price)
    rate = d(ytm)
    macaulay_numerator = Decimal(0)
    modified_numerator = Decimal(0)
    convexity_numerator = Decimal(0)
    for flow in cashflows:
        total = d(flow["total"])
        time_years = d(flow["time_years"])
        if freq == 0:
            base = Decimal(1) + rate * time_years
            present = total / base
            macaulay_numerator += time_years * present
            modified_numerator += time_years * total / (base ** 2)
            convexity_numerator += Decimal(2) * time_years * time_years * total / (base ** 3)
        else:
            frequency = Decimal(freq)
            periods = time_years * frequency
            base = Decimal(1) + rate / frequency
            present = total * (base ** (-periods))
            macaulay_numerator += time_years * present
            modified_numerator += periods / frequency * total * (base ** (-periods - 1))
            convexity_numerator += periods * (periods + 1) / (frequency ** 2) * total * (base ** (-periods - 2))

    price_up, _ = compute_pv_given_yield(cashflows, rate + Decimal("0.0001"), settlement_date, freq)
    price_down, _ = compute_pv_given_yield(cashflows, rate - Decimal("0.0001"), settlement_date, freq)
    return {
        "macaulay_duration": macaulay_numerator / price,
        "modified_duration": modified_numerator / price,
        "convexity": convexity_numerator / price,
        "dv01": abs(price_down - price_up) / Decimal(2),
    }


def price_yield_round_trip(cashflows, ytm_input, settlement_date, freq):
    dirty, _ = compute_pv_given_yield(cashflows, ytm_input, settlement_date, freq)
    return ytm_input, solve_ytm_from_price(cashflows, dirty, settlement_date, freq)


def verify_finite_differences(
    cashflows: List[Dict],
    ytm: float,
    settlement_date: date,
    freq: int,
    dirty_price: Decimal,
    dv01: Decimal,
    convexity: Optional[Decimal] = None,
    rel_tol: float = 1e-4,
) -> Dict:
    bump = Decimal("0.0001")
    price = d(dirty_price)
    price_up, _ = compute_pv_given_yield(cashflows, d(ytm) + bump, settlement_date, freq)
    price_down, _ = compute_pv_given_yield(cashflows, d(ytm) - bump, settlement_date, freq)
    dv01_fd = abs(price_down - price_up) / Decimal(2)
    convexity_fd = (price_up + price_down - Decimal(2) * price) / (price * bump * bump)
    dv01_rel = abs(d(dv01) - dv01_fd) / max(abs(d(dv01)), Decimal("1e-30"))
    convexity_rel = (
        abs(d(convexity) - convexity_fd) / max(abs(d(convexity)), Decimal("1e-30"))
        if convexity is not None else Decimal(0)
    )
    return {
        "dv01_analytic": float(dv01),
        "dv01_finite_diff": float(dv01_fd),
        "dv01_diff": float(abs(d(dv01) - dv01_fd)),
        "dv01_rel_diff": float(dv01_rel),
        "price_up": float(price_up),
        "price_down": float(price_down),
        "convexity_analytic": float(convexity) if convexity is not None else None,
        "convexity_finite_diff": float(convexity_fd),
        "convexity_rel_diff": float(convexity_rel),
    }


def value_bond(
    bond_id: str,
    issue_date: date,
    maturity_date: date,
    coupon_rate: float,
    freq: int,
    settlement_date: date,
    mode: str,
    input_value: float,
    face_value: float = 100.0,
) -> Dict:
    cashflows = generate_cashflows(
        bond_id, issue_date, maturity_date, coupon_rate, freq, settlement_date, face_value
    )
    if not cashflows:
        raise ValueError("bond has no post-settlement cashflows")
    accrued = compute_accrued_interest(
        issue_date, maturity_date, coupon_rate, freq, settlement_date, face_value
    )
    if mode == "YIELD_IN":
        ytm = float(input_value)
        dirty, _ = compute_pv_given_yield(cashflows, ytm, settlement_date, freq)
        clean = dirty - accrued
    elif mode == "PRICE_IN":
        clean = d(input_value)
        dirty = clean + accrued
        solved = solve_ytm_from_price(cashflows, dirty, settlement_date, freq)
        if solved is None:
            raise ValueError("NO_BRACKET_OR_NOT_CONVERGED")
        ytm = solved
    else:
        raise ValueError(f"unknown mode: {mode}")

    risk = compute_risk_measures(cashflows, ytm, settlement_date, freq, dirty, face_value)
    original, solved = price_yield_round_trip(cashflows, ytm, settlement_date, freq)
    finite_difference = verify_finite_differences(
        cashflows, ytm, settlement_date, freq, dirty, risk["dv01"], risk["convexity"]
    )
    return {
        "bond_id": bond_id,
        "mode": mode,
        "input_value": float(input_value),
        "settlement_date": settlement_date.isoformat(),
        "cashflows": cashflows,
        "accrued_interest": float(accrued),
        "clean_price": float(clean),
        "dirty_price": float(dirty),
        "yield_to_maturity": ytm,
        "macaulay_duration": float(risk["macaulay_duration"]),
        "modified_duration": float(risk["modified_duration"]),
        "convexity": float(risk["convexity"]),
        "dv01": float(risk["dv01"]),
        "round_trip": {
            "original_ytm": original,
            "round_trip_ytm": solved,
            "ytm_diff": abs(original - solved) if solved is not None else None,
        },
        "finite_diff": finite_difference,
    }
