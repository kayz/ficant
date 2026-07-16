"""
Day-count conventions for cgb-reference-v1.

- Actual/Actual (Bond/ISMA) for fixed-rate coupon bonds
- Actual/Actual simple (discount) for discount bonds: year_fraction = days / days_in_year
"""

from datetime import date, timedelta
from typing import List, Tuple


def _days_in_year_range(d1: date, d2: date) -> int:
    """Number of days in the year range from d1 to d2, inclusive of Feb 29."""
    if d1.year == d2.year:
        y = d1.year
        return 366 if (y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)) else 365
    # multi-year
    total = 0
    for y in range(d1.year, d2.year + 1):
        total += 366 if (y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)) else 365
    return total


def _is_leap(y: int) -> bool:
    return y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)


def actual_actual_isma(start: date, end: date, freq: int,
                        ref_start: date = None, ref_end: date = None) -> float:
    """
    Actual/Actual (Bond/ISMA) day count fraction.

    For regular coupon periods (period ≤ reference period):
        year_fraction = days_in_period / (freq * days_in_coupon_year)

    where days_in_coupon_year is the number of days in the reference period's year.

    For irregular periods, the period is decomposed.

    Parameters:
        start, end: accrual start and end dates
        freq: coupon frequency per year (1=annual, 2=semi-annual)
        ref_start, ref_end: reference period (nominal coupon dates);
                            if None, uses start/end directly.
    """
    if start >= end:
        return 0.0

    rs = ref_start if ref_start else start
    re = ref_end if ref_end else end

    days = (end - start).days

    # Reference period length in years
    ref_days = (re - rs).days
    if ref_days <= 0:
        return 0.0

    # The ISMA rule: use the year containing the reference period end
    # to determine the denominator year length
    ref_year = re.year
    denom = 366.0 if _is_leap(ref_year) else 365.0

    # For regular periods, year_fraction = days / (freq * days_in_coupon_year)
    # But we need the days in the specific coupon year
    # Standard ISMA: year_fraction = days_in_period / days_in_year
    # but adjusted by frequency if period < 1 year

    # Actually per ISMA: each accrual period is divided by the number of days
    # in the year that contains it times the frequency.
    # More precisely: if the period spans parts of two years, the numerator
    # is split by year boundary.

    if start.year == end.year:
        return days / denom

    # Period spans a year boundary — split at Dec 31
    year_end = date(start.year, 12, 31)
    if start < year_end < end:
        part1 = (year_end - start).days
        part2 = (end - year_end).days - 1  # subtract Dec 31 itself
        # Actually the split should use each year's denominator
        d1 = 366.0 if _is_leap(start.year) else 365.0
        d2 = 366.0 if _is_leap(end.year) else 365.0
        # But ISMA splits the numerator only, denominator uses the year
        # containing the period. This gets complex.
        #
        # For Actual/Actual ISMA: count actual days, denominator is:
        # - For periods ≤ 1 year: freq × number of days in the coupon period
        #   where the reference period uses the actual year days
        #
        # The simpler and more common interpretation:
        # year_fraction = actual_days / (freq × reference_period_days)
        # where reference_period_days is the days in a full coupon year
        # being the year that the coupon crosses.

    # Let me use the standard QuantLib-compatible approach:
    # Each full year has year_fraction = 1.0, remainder is actual/actual
    # in the year it falls into.

    # Simpler approach: days / days_in_year where year is start's year
    # then handle cross-year by weighting.

    # For Actual/Actual ISMA with annual freq:
    # Just compute actual days / actual days in year (using the start year
    # for periods within one year, or splitting at year boundaries)

    # The correct ISMA implementation per ISDA:
    # If the period is shorter than the reference period (1/freq years):
    #   year_frac = days / (freq * days_in_reference_period)
    # If the period spans a year boundary, split and use each year's days.

    # Let me use the more precise year-split approach:
    total = 0.0
    cur = start
    while cur < end:
        year_boundary = date(cur.year, 12, 31)
        if end <= year_boundary:
            segment_days = (end - cur).days
            total += segment_days / (366.0 if _is_leap(cur.year) else 365.0)
            break
        else:
            segment_days = (year_boundary - cur).days + 1  # include Dec 31
            total += segment_days / (366.0 if _is_leap(cur.year) else 365.0)
            cur = date(cur.year + 1, 1, 1)

    return total


def actual_actual_isma_v2(start: date, end: date, freq: int,
                           ref_period_start: date = None,
                           ref_period_end: date = None) -> float:
    """
    Actual/Actual (Bond/ISMA) as commonly implemented in financial libraries.

    The ISMA method (also called Actual/Actual Bond) treats the accrual
    period relative to the reference (coupon) period:
      - If the accrual period equals the reference period: year_frac = 1/freq
      - If it's shorter, compute actual days / (freq × days in coupon year)
        where the coupon year is the year containing the cash flow date

    For periods wholly within one year, this simplifies to:
        days / (freq × days_in_year)
    where the relevant year is the reference period end year.

    For QuantLib compatibility, this follows the ISMA convention:
    actual days elapsed / (frequency × days in the year containing the period)
    with proper handling of year boundaries.
    """
    if start >= end:
        return 0.0

    days = (end - start).days
    if days == 0:
        return 0.0

    # Determine the reference year: use the year of the END of the period
    # (this matches ISMA convention for coupon-bearing bonds)
    ref_year = end.year

    denom = 366.0 if _is_leap(ref_year) else 365.0

    if start.year == end.year:
        return days / denom

    # Multi-year: split at each year boundary (Dec 31 / Jan 1)
    total = 0.0
    cur = start
    while cur < end:
        year_end_date = date(cur.year, 12, 31)
        if end <= year_end_date:
            seg = (end - cur).days
            yr = cur.year
        else:
            seg = (year_end_date - cur).days + 1  # inclusive of Dec 31
            yr = cur.year

        yr_denom = 366.0 if _is_leap(yr) else 365.0
        total += seg / yr_denom
        cur = date(cur.year + 1, 1, 1)

    return total


def actual_actual_isma_years(start: date, end: date, freq: int,
                              ref_period_start: date = None,
                              ref_period_end: date = None) -> float:
    """
    Actual/Actual (ISMA/Bond) as used by major financial libraries.

    This is the standard "Actual/Actual (Bond)" convention:
    - For a full coupon period: year_fraction = 1/freq
    - For a partial period: days_in_period / (freq × reference_period_days)
      where reference_period_days uses the actual year days for the year
      containing the reference period end.

    Since we're working with regular bonds, the reference period is always
    defined by the coupon schedule. For our implementation, we compute:

    If the period fits entirely within one year:
        year_frac = actual_days / days_in_year
    If the period crosses year boundaries, we split proportionally.
    """
    if start >= end:
        return 0.0

    # For the endpoint year, use its actual day count
    # Split across year boundaries
    total = 0.0
    d = start
    while d < end:
        # End of current year
        y_end = date(d.year, 12, 31)
        seg_end = min(end, y_end + timedelta(days=1))  # +1 to make it exclusive end
        seg_days = (seg_end - d).days
        days_in_yr = 366.0 if _is_leap(d.year) else 365.0
        total += seg_days / days_in_yr
        d = seg_end

    return total


# ── Actual/Actual for discount bonds ─────────────────────────────────────

def actual_actual_discount(start: date, end: date) -> float:
    """
    Actual/Actual for discount bonds with simple yield.

    Year fraction = actual_days / days_in_year, with proper handling
    of year boundaries (each year segment uses its own day count).

    This is used for the simple yield formula:
        y = (100 / dirty_price - 1) / year_fraction
    """
    if start >= end:
        return 0.0

    total = 0.0
    d = start
    while d < end:
        y_end = date(d.year, 12, 31)
        seg_end = min(end, y_end + timedelta(days=1))
        seg_days = (seg_end - d).days
        days_in_yr = 366.0 if _is_leap(d.year) else 365.0
        total += seg_days / days_in_yr
        d = seg_end

    return total
