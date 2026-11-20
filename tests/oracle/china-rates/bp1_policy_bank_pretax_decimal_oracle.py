"""Independent standard-library Decimal oracle for BP-1 policy-bank PRE_TAX bonds.

This module uses only Python standard-library primitives (`calendar`, `datetime`,
`decimal`, `json`, `pathlib`, `typing`). It binds the authoritative 2024-2026
CFETS interbank bond market holiday notices, enforces exact [2024-01-01,
2026-12-31] calendar coverage with weekend-only provisional adjustment strictly
beyond coverage_end, and computes 60-digit Decimal Act/Act ISMA fixed-rate bond
analytics (260-step bracketed bisection for PRICE_IN) quantized to 12 decimal
places using ROUND_HALF_EVEN.
"""

from __future__ import annotations

import calendar
from datetime import date, timedelta
from decimal import Context, Decimal, ROUND_HALF_EVEN, localcontext
import json
from pathlib import Path
from typing import Any

CONTEXT = Context(prec=60, rounding=ROUND_HALF_EVEN)
OUTPUT_QUANTUM = Decimal("0.000000000001")
ZERO = Decimal(0)
ONE = Decimal(1)
TWO = Decimal(2)
ONE_BP = Decimal("0.0001")
BISECTION_STEPS = 260
BRACKET_LOW = Decimal("-0.50")
BRACKET_HIGH = Decimal("1.00")

COVERAGE_START = date(2024, 1, 1)
COVERAGE_END = date(2026, 12, 31)

EXPECTED_OFFICIAL_NOTICES: tuple[dict[str, Any], ...] = (
    {
        "year": 2024,
        "document_number": "中汇交发〔2023〕301号",
        "title": "关于2024年全国银行间债券市场节假日休市安排的通知",
        "url": "https://www.chinamoney.org.cn/chinese/rdgz/20231225/2757785.html",
    },
    {
        "year": 2025,
        "document_number": "中汇交发〔2024〕358号",
        "title": "关于2025年全国银行间债券市场节假日休市安排的通知",
        "url": "https://www.chinamoney.org.cn/chinese/rdgz/20241212/3017037.html",
    },
    {
        "year": 2026,
        "document_number": "中汇交发〔2025〕460号",
        "title": "关于2026年全国银行间债券市场节假日休市安排的通知",
        "url": "https://www.chinamoney.org.cn/chinese/rdgz/20251219/3254572.html",
    },
)

EXPECTED_BOND_IDS: tuple[str, ...] = (
    "CDB-10Y-ANNUAL",
    "CDB-10Y-SEMIANNUAL",
    "EIBC-5Y-ANNUAL",
    "EIBC-3Y-SEMIANNUAL",
    "ADBC-1Y-EXACT-ANNUAL",
    "ADBC-7Y-SEMIANNUAL",
)

EXPECTED_MODES: tuple[str, ...] = ("YIELD_IN", "PRICE_IN")


def _render(value: Decimal) -> str:
    quantized = value.quantize(OUTPUT_QUANTUM, rounding=ROUND_HALF_EVEN)
    if quantized == ZERO:
        quantized = abs(quantized)
    return format(quantized, "f")


def _parse_date(text: str) -> date:
    if not isinstance(text, str) or len(text) != 10:
        raise ValueError(f"invalid ISO date: {text!r}")
    return date.fromisoformat(text)


def add_months(base: date, months: int) -> date:
    """Adds signed whole months with end-of-month clamping."""

    total = base.year * 12 + (base.month - 1) + months
    year, month_zero = divmod(total, 12)
    month = month_zero + 1
    day = min(base.day, calendar.monthrange(year, month)[1])
    return date(year, month, day)


def build_calendar_spec(calendar_doc: dict[str, Any]) -> dict[str, Any]:
    """Validates official 2024-2026 CFETS notices and derives calendar lookup sets."""

    if not isinstance(calendar_doc, dict):
        raise ValueError("calendar metadata is required")

    cov_start = _parse_date(calendar_doc["coverage_start"])
    cov_end = _parse_date(calendar_doc["coverage_end"])
    if cov_start != COVERAGE_START or cov_end != COVERAGE_END:
        raise ValueError(
            f"calendar coverage must be [{COVERAGE_START}, {COVERAGE_END}], "
            f"got [{cov_start}, {cov_end}]"
        )

    first_session = calendar_doc.get("first_session")
    last_session = calendar_doc.get("last_session")
    if first_session != {"local_date": "2024-01-01", "closed": True}:
        raise ValueError(f"unexpected first_session: {first_session!r}")
    if last_session != {"local_date": "2026-12-31", "closed": False}:
        raise ValueError(f"unexpected last_session: {last_session!r}")

    sources = calendar_doc.get("sources")
    if not isinstance(sources, list) or len(sources) != len(EXPECTED_OFFICIAL_NOTICES):
        raise ValueError("calendar.sources must bind exactly 3 official CFETS notices")

    closed_holidays: set[date] = set()
    work_weekends: set[date] = set()
    verified_sources: list[dict[str, Any]] = []

    for expected_notice, actual_notice in zip(
        EXPECTED_OFFICIAL_NOTICES, sources, strict=True
    ):
        for key in ("year", "document_number", "title", "url"):
            if actual_notice.get(key) != expected_notice[key]:
                raise ValueError(
                    f"notice {expected_notice['year']}.{key} mismatch: "
                    f"got {actual_notice.get(key)!r}, expected {expected_notice[key]!r}"
                )
        year = expected_notice["year"]
        for start_s, end_s in actual_notice["holidays_closed"]:
            start_d = _parse_date(start_s)
            end_d = _parse_date(end_s)
            if start_d > end_d or start_d.year != year or end_d.year != year:
                raise ValueError(
                    f"invalid holiday range [{start_s}, {end_s}] for year {year}"
                )
            cursor = start_d
            while cursor <= end_d:
                closed_holidays.add(cursor)
                cursor += timedelta(days=1)

        for open_s in actual_notice["work_weekends_open"]:
            open_d = _parse_date(open_s)
            if open_d.year != year or open_d.weekday() < 5:
                raise ValueError(f"invalid work-weekend date {open_s} for year {year}")
            work_weekends.add(open_d)

        verified_sources.append(dict(expected_notice))

    overlap = closed_holidays & work_weekends
    if overlap:
        raise ValueError(f"dates cannot be both closed holiday and work-weekend: {sorted(overlap)}")

    weekday_holidays = sorted(d for d in closed_holidays if d.weekday() < 5)
    sorted_work_weekends = sorted(work_weekends)

    declared_non_biz = calendar_doc.get("non_business_days")
    declared_work_we = calendar_doc.get("work_weekends")
    if declared_non_biz != [d.isoformat() for d in weekday_holidays]:
        raise ValueError("calendar.non_business_days does not match derived weekday holidays")
    if declared_work_we != [d.isoformat() for d in sorted_work_weekends]:
        raise ValueError("calendar.work_weekends does not match derived work_weekends_open")

    # Sanity-verify real first/last sessions and 2026 National Day closure.
    if COVERAGE_START not in closed_holidays:
        raise ValueError("2024-01-01 must be closed")
    if COVERAGE_END in closed_holidays or COVERAGE_END.weekday() >= 5:
        raise ValueError("2026-12-31 must be open")
    for day in range(1, 8):
        if date(2026, 10, day) not in closed_holidays:
            raise ValueError(f"2026-10-{day:02d} must be closed for National Day")
    if date(2026, 10, 8) in closed_holidays or date(2026, 10, 8).weekday() >= 5:
        raise ValueError("2026-10-08 must be open after National Day")

    return {
        "calendar_id": calendar_doc["calendar_id"],
        "version": int(calendar_doc["version"]),
        "market": calendar_doc["market"],
        "market_timezone": calendar_doc["market_timezone"],
        "coverage_start": cov_start,
        "coverage_end": cov_end,
        "first_session": dict(first_session),
        "last_session": dict(last_session),
        "sources": verified_sources,
        "closed_holidays": frozenset(closed_holidays),
        "weekday_holidays": frozenset(weekday_holidays),
        "work_weekends": frozenset(sorted_work_weekends),
    }


def is_in_coverage_business_day(day: date, cal_spec: dict[str, Any]) -> bool:
    """Evaluates CFETS business-day status within exact calendar coverage."""

    weekend = day.weekday() >= 5
    is_work_we = day in cal_spec["work_weekends"]
    is_holiday = day in cal_spec["closed_holidays"]
    return (not weekend or is_work_we) and not is_holiday


def following_adjust(
    nominal: date,
    cal_spec: dict[str, Any],
    *,
    exact_required: bool,
) -> tuple[date, bool]:
    """Applies Following BDC; weekend-only provisional rule applies strictly after coverage_end."""

    cov_start: date = cal_spec["coverage_start"]
    cov_end: date = cal_spec["coverage_end"]
    if exact_required and nominal < cov_start:
        raise ValueError(
            f"date {nominal} precedes exact calendar coverage_start {cov_start}"
        )

    adjusted = nominal
    used_provisional = False
    while True:
        beyond_coverage = adjusted > cov_end
        if beyond_coverage and exact_required:
            raise ValueError(
                f"date {adjusted} exceeds exact calendar coverage_end {cov_end}"
            )
        if beyond_coverage:
            used_provisional = True
            is_biz = adjusted.weekday() < 5
        else:
            is_biz = is_in_coverage_business_day(adjusted, cal_spec)
        if is_biz:
            return adjusted, used_provisional
        adjusted += timedelta(days=1)


def generate_nominal_schedule(
    first_issue_date: date,
    maturity_date: date,
    frequency: int,
) -> list[date]:
    """Generates regular nominal coupon dates from first_issue_date to maturity_date."""

    if frequency not in (1, 2):
        raise ValueError(f"unsupported coupon frequency: {frequency}")
    if maturity_date <= first_issue_date:
        raise ValueError("maturity_date must be after first_issue_date")

    months_per_period = 12 // frequency
    count = 0
    cursor = maturity_date
    while cursor > first_issue_date:
        count += 1
        cursor = add_months(maturity_date, -count * months_per_period)

    schedule = [
        add_months(first_issue_date, (idx + 1) * months_per_period)
        for idx in range(count)
    ]
    if schedule and schedule[-1] != maturity_date:
        schedule[-1] = maturity_date
    return schedule


def _dirty_price_given_yield(
    cashflow_totals: list[Decimal],
    cashflow_times: list[Decimal],
    ytm: Decimal,
    frequency: int,
) -> Decimal:
    freq_dec = Decimal(frequency)
    with localcontext(CONTEXT):
        base = ONE + ytm / freq_dec
        if base <= ZERO:
            raise ValueError("1 + ytm / frequency must be positive")
        pv = ZERO
        for total, time_years in zip(cashflow_totals, cashflow_times, strict=True):
            exponent = -(time_years * freq_dec)
            pv += total * (base**exponent)
        return pv


def solve_ytm_bisection(
    cashflow_totals: list[Decimal],
    cashflow_times: list[Decimal],
    target_dirty: Decimal,
    frequency: int,
    *,
    steps: int = BISECTION_STEPS,
    lo: Decimal = BRACKET_LOW,
    hi: Decimal = BRACKET_HIGH,
) -> Decimal:
    """Solves yield-to-maturity via fixed 260-step bracketed bisection in 60-digit Decimal."""

    if steps < 1:
        raise ValueError("bisection steps must be positive")

    with localcontext(CONTEXT):
        f_lo = (
            _dirty_price_given_yield(cashflow_totals, cashflow_times, lo, frequency)
            - target_dirty
        )
        f_hi = (
            _dirty_price_given_yield(cashflow_totals, cashflow_times, hi, frequency)
            - target_dirty
        )
        if f_lo == ZERO:
            return lo
        if f_hi == ZERO:
            return hi
        if f_lo * f_hi > ZERO:
            raise ValueError(
                f"target_dirty={target_dirty} not bracketed on [{lo}, {hi}] "
                f"(f_lo={f_lo}, f_hi={f_hi})"
            )

        left, right = lo, hi
        f_left = f_lo
        for _ in range(steps):
            mid = (left + right) / TWO
            f_mid = (
                _dirty_price_given_yield(cashflow_totals, cashflow_times, mid, frequency)
                - target_dirty
            )
            if f_mid == ZERO:
                return mid
            if f_left * f_mid > ZERO:
                left = mid
                f_left = f_mid
            else:
                right = mid
        return (left + right) / TWO


def _frequency_from_label(label: str) -> int:
    if label == "ANNUAL":
        return 1
    if label == "SEMIANNUAL":
        return 2
    raise ValueError(f"unsupported coupon_frequency: {label!r}")


def evaluate_bond_mode(
    bond: dict[str, Any],
    mode: str,
    cal_spec: dict[str, Any],
) -> dict[str, Any]:
    """Independently computes cashflows and 8 analytics measures for one (bond, mode)."""

    if mode not in EXPECTED_MODES:
        raise ValueError(f"unsupported analytics mode: {mode!r}")

    bond_id = bond["case_id"]
    if bond.get("execution_mode") != "PRE_TAX":
        raise ValueError(f"{bond_id}: execution_mode must be PRE_TAX")
    if bond.get("day_count") != "ACT_ACT_BOND_ISMA":
        raise ValueError(f"{bond_id}: day_count must be ACT_ACT_BOND_ISMA")
    if bond.get("business_day") != "FOLLOWING":
        raise ValueError(f"{bond_id}: business_day must be FOLLOWING")

    requirement = bond.get("calendar_requirement")
    if requirement not in ("REFERENCE_REPLAY", "EXACT_MARKET"):
        raise ValueError(f"{bond_id}: invalid calendar_requirement {requirement!r}")
    exact_required = requirement == "EXACT_MARKET"

    first_issue = _parse_date(bond["first_issue_date"])
    current_issue = _parse_date(bond["current_issue_date"])
    maturity = _parse_date(bond["maturity_date"])
    settlement = _parse_date(bond["settlement_date"])

    if not (COVERAGE_START <= first_issue <= COVERAGE_END):
        raise ValueError(
            f"{bond_id}: first_issue_date {first_issue} must lie inside "
            f"[{COVERAGE_START}, {COVERAGE_END}]"
        )
    if not (first_issue <= current_issue < settlement < maturity):
        raise ValueError(
            f"{bond_id}: dates must satisfy first_issue <= current_issue < settlement < maturity"
        )
    if exact_required and not (COVERAGE_START <= settlement <= COVERAGE_END):
        raise ValueError(
            f"{bond_id}: settlement_date {settlement} outside exact calendar coverage"
        )

    frequency = _frequency_from_label(bond["coupon_frequency"])
    coupon_rate = Decimal(bond["coupon_rate"])
    face_value = Decimal(bond["face_value"])
    cumulative_issued_amount = Decimal(bond["cumulative_issued_amount"])
    if coupon_rate <= ZERO or face_value != Decimal("100") or cumulative_issued_amount <= ZERO:
        raise ValueError(f"{bond_id}: invalid coupon_rate/face_value/cumulative_issued_amount")

    nominal_dates = generate_nominal_schedule(first_issue, maturity, frequency)
    if not nominal_dates:
        raise ValueError(f"{bond_id}: empty nominal schedule")

    adjusted_pairs: list[tuple[date, date]] = []
    any_provisional = False
    for nominal in nominal_dates:
        payment, used_provisional = following_adjust(
            nominal, cal_spec, exact_required=exact_required
        )
        if used_provisional:
            any_provisional = True
        adjusted_pairs.append((nominal, payment))

    calendar_resolution = (
        "PROVISIONAL_WEEKEND_ONLY" if any_provisional else "EXACT"
    )
    if exact_required and calendar_resolution != "EXACT":
        raise ValueError(f"{bond_id}: EXACT_MARKET cannot resolve provisionally")

    with localcontext(CONTEXT):
        freq_dec = Decimal(frequency)
        coupon_per_period = face_value * coupon_rate / freq_dec

        # Accrued interest on nominal period containing settlement_date.
        accrued_interest = ZERO
        months_per_period = 12 // frequency
        prev_nom = add_months(nominal_dates[0], -months_per_period)
        for idx, nom in enumerate(nominal_dates):
            if nom > settlement:
                period_start = prev_nom if idx == 0 else nominal_dates[idx - 1]
                period_end = nom
                if settlement > period_start:
                    elapsed = Decimal((settlement - period_start).days)
                    period_days = Decimal((period_end - period_start).days)
                    accrued_interest = (
                        coupon_rate * face_value * elapsed / (period_days * freq_dec)
                    )
                break

        # Surviving cashflows filtered by adjusted payment_date > settlement_date.
        first_surviving_idx: int | None = None
        for idx, (_nom, pay) in enumerate(adjusted_pairs):
            if pay > settlement:
                first_surviving_idx = idx
                break
        if first_surviving_idx is None:
            raise ValueError(f"{bond_id}: no post-settlement cashflows")

        first_nom = nominal_dates[first_surviving_idx]
        ref_start = (
            add_months(nominal_dates[0], -months_per_period)
            if first_surviving_idx == 0
            else nominal_dates[first_surviving_idx - 1]
        )
        base_residual = (
            Decimal((first_nom - settlement).days)
            / (Decimal((first_nom - ref_start).days) * freq_dec)
        )

        cashflows_out: list[dict[str, Any]] = []
        cf_totals: list[Decimal] = []
        cf_times: list[Decimal] = []
        for idx in range(first_surviving_idx, len(adjusted_pairs)):
            nom, pay = adjusted_pairs[idx]
            if pay <= settlement:
                continue
            principal = face_value if idx == len(adjusted_pairs) - 1 else ZERO
            total = coupon_per_period + principal
            time_years = base_residual + Decimal(idx - first_surviving_idx) / freq_dec

            cf_totals.append(total)
            cf_times.append(time_years)
            cashflows_out.append(
                {
                    "sequence": len(cashflows_out) + 1,
                    "nominal_date": nom.isoformat(),
                    "payment_date": pay.isoformat(),
                    "coupon": _render(coupon_per_period),
                    "principal": _render(principal),
                    "total": _render(total),
                }
            )

        if mode == "YIELD_IN":
            input_value = Decimal(bond["yield_in_input"])
            ytm = input_value
            dirty_price = _dirty_price_given_yield(cf_totals, cf_times, ytm, frequency)
            clean_price = dirty_price - accrued_interest
        else:
            input_value = Decimal(bond["price_in_input"])
            clean_price = input_value
            dirty_price = clean_price + accrued_interest
            ytm = solve_ytm_bisection(cf_totals, cf_times, dirty_price, frequency)
            # Re-evaluate dirty/clean from solved yield at 60-digit precision for consistency.
            dirty_price = _dirty_price_given_yield(cf_totals, cf_times, ytm, frequency)
            clean_price = dirty_price - accrued_interest

        # Analytic Macaulay duration, Modified duration, and Convexity.
        inv_freq = ONE / freq_dec
        base = ONE + ytm * inv_freq
        mac_num = ZERO
        conv_num = ZERO
        for total, time_years in zip(cf_totals, cf_times, strict=True):
            periods = time_years * freq_dec
            pv = total * (base ** (-periods))
            mac_num += time_years * pv
            conv_num += total * time_years * (time_years + inv_freq) * (
                base ** (-(periods + TWO))
            )

        macaulay_duration = mac_num / dirty_price
        modified_duration = macaulay_duration / base
        convexity = conv_num / dirty_price

        # Central finite-difference DV01 at +/- 1 bp (0.0001).
        p_down = _dirty_price_given_yield(cf_totals, cf_times, ytm - ONE_BP, frequency)
        p_up = _dirty_price_given_yield(cf_totals, cf_times, ytm + ONE_BP, frequency)
        dv01 = (p_down - p_up) / TWO

        measures = {
            "accrued_interest": _render(accrued_interest),
            "clean_price": _render(clean_price),
            "dirty_price": _render(dirty_price),
            "yield_to_maturity": _render(ytm),
            "macaulay_duration": _render(macaulay_duration),
            "modified_duration": _render(modified_duration),
            "convexity": _render(convexity),
            "dv01": _render(dv01),
        }

    return {
        "bond_id": bond_id,
        "mode": mode,
        "execution_mode": "PRE_TAX",
        "measures_tax_basis": "PRE_TAX",
        "after_tax": None,
        "calendar_requirement": requirement,
        "calendar_resolution": calendar_resolution,
        "settlement_date": settlement.isoformat(),
        "input_value": _render(input_value),
        "cashflows": cashflows_out,
        "measures": measures,
    }


def build_expected(inputs: dict[str, Any]) -> dict[str, Any]:
    """Builds the frozen 12-case expected document from independent exact-Decimal math."""

    if inputs.get("schema") != "ficant.bp1-policy-bank-pretax.inputs.v1":
        raise ValueError(f"unexpected input schema: {inputs.get('schema')!r}")

    cal_spec = build_calendar_spec(inputs["calendar"])

    bonds = inputs.get("bonds")
    if not isinstance(bonds, list) or len(bonds) != len(EXPECTED_BOND_IDS):
        raise ValueError(f"inputs.bonds must contain exactly {len(EXPECTED_BOND_IDS)} bonds")

    seen_ids = tuple(bond["case_id"] for bond in bonds)
    if seen_ids != EXPECTED_BOND_IDS:
        raise ValueError(
            f"inputs.bonds case_id sequence mismatch: got {seen_ids!r}, expected {EXPECTED_BOND_IDS!r}"
        )

    case_results: dict[str, dict[str, Any]] = {}
    for bond in bonds:
        for mode in EXPECTED_MODES:
            key = f"{bond['case_id']}:{mode}"
            if key in case_results:
                raise ValueError(f"duplicate case key: {key}")
            case_results[key] = evaluate_bond_mode(bond, mode, cal_spec)

    return {
        "schema": "ficant.bp1-policy-bank-pretax.expected.v1",
        "calendar": {
            "calendar_id": cal_spec["calendar_id"],
            "version": cal_spec["version"],
            "market": cal_spec["market"],
            "market_timezone": cal_spec["market_timezone"],
            "coverage_start": cal_spec["coverage_start"].isoformat(),
            "coverage_end": cal_spec["coverage_end"].isoformat(),
            "first_session": cal_spec["first_session"],
            "last_session": cal_spec["last_session"],
            "sources": cal_spec["sources"],
        },
        "case_results": case_results,
    }


def _default_paths() -> tuple[Path, Path]:
    root = Path(__file__).resolve().parents[3]
    return (
        root / "tests/golden-cases/china-rates/bp1-policy-bank-pretax-inputs.json",
        root
        / "tests/golden-cases/china-rates/expected/bp1-policy-bank-pretax-expected.json",
    )


if __name__ == "__main__":
    inputs_path, expected_path = _default_paths()
    inputs_doc = json.loads(inputs_path.read_text(encoding="utf-8"))
    expected_doc = build_expected(inputs_doc)
    expected_path.write_text(
        json.dumps(expected_doc, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
        newline="\n",
    )
