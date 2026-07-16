"""
cgb-reference-calendar-v1: China Government Bond reference calendar.

Exact coverage: 2005-01-01 through 2026-12-31 using known Chinese interbank
holidays and adjusted workdays from State Council announcements.
Provisional: 2027+ weekend-only (Sat/Sun non-business days).

This calendar is test-only and must not be used as a production calendar source.
"""

from datetime import date, timedelta
from typing import Set, Tuple

# ── Known Chinese interbank holidays 2005–2026 ──────────────────────────
# Sourced from State Council General Office annual holiday notices.
# Format: date objects for non-weekend holidays.
# Working weekend adjustments are tracked separately.

_HOLIDAYS: Set[date] = set()

# 2005
_HOLIDAYS.update({
    date(2005, 1, 3),    # New Year (Jan 1 was Sat, observed Mon)
    date(2005, 2, 7), date(2005, 2, 8), date(2005, 2, 9),   # Spring Festival
    date(2005, 2, 10), date(2005, 2, 11),                    # Spring Festival
    date(2005, 5, 2), date(2005, 5, 3), date(2005, 5, 4),   # Labor Day
    date(2005, 5, 5), date(2005, 5, 6),                      # Labor Day
    date(2005, 10, 3), date(2005, 10, 4), date(2005, 10, 5), # National Day
    date(2005, 10, 6), date(2005, 10, 7),                    # National Day
})

# 2006
_HOLIDAYS.update({
    date(2006, 1, 2), date(2006, 1, 3),    # New Year
    date(2006, 1, 30), date(2006, 1, 31),   # Spring Festival
    date(2006, 2, 1), date(2006, 2, 2), date(2006, 2, 3),  # Spring Festival
    date(2006, 5, 1), date(2006, 5, 2), date(2006, 5, 3),  # Labor Day
    date(2006, 5, 4), date(2006, 5, 5),                     # Labor Day
    date(2006, 10, 2), date(2006, 10, 3), date(2006, 10, 4), # National Day
    date(2006, 10, 5), date(2006, 10, 6),                    # National Day
})

# 2007
_HOLIDAYS.update({
    date(2007, 1, 1), date(2007, 1, 2), date(2007, 1, 3),  # New Year
    date(2007, 2, 19), date(2007, 2, 20), date(2007, 2, 21), # Spring Festival
    date(2007, 2, 22), date(2007, 2, 23),                    # Spring Festival
    date(2007, 5, 1), date(2007, 5, 2), date(2007, 5, 3),  # Labor Day
    date(2007, 5, 4), date(2007, 5, 7),                     # Labor Day
    date(2007, 10, 1), date(2007, 10, 2), date(2007, 10, 3), # National Day
    date(2007, 10, 4), date(2007, 10, 5),                    # National Day
})

# 2008
_HOLIDAYS.update({
    date(2008, 1, 1),                       # New Year
    date(2008, 2, 6), date(2008, 2, 7), date(2008, 2, 8),  # Spring Festival
    date(2008, 2, 11), date(2008, 2, 12),                   # Spring Festival
    date(2008, 4, 4),                       # Qingming
    date(2008, 5, 1), date(2008, 5, 2),    # Labor Day
    date(2008, 6, 9),                       # Dragon Boat
    date(2008, 9, 15),                      # Mid-Autumn
    date(2008, 9, 29), date(2008, 9, 30),   # National Day
    date(2008, 10, 1), date(2008, 10, 2), date(2008, 10, 3), # National Day
})

# 2009
_HOLIDAYS.update({
    date(2009, 1, 1), date(2009, 1, 2),    # New Year
    date(2009, 1, 26), date(2009, 1, 27), date(2009, 1, 28), # Spring Festival
    date(2009, 1, 29), date(2009, 1, 30),                    # Spring Festival
    date(2009, 4, 6),                       # Qingming
    date(2009, 5, 1),                       # Labor Day
    date(2009, 5, 28), date(2009, 5, 29),   # Dragon Boat
    date(2009, 10, 1), date(2009, 10, 2),   # National Day
    date(2009, 10, 5), date(2009, 10, 6), date(2009, 10, 7), date(2009, 10, 8), # National Day
})

# 2010
_HOLIDAYS.update({
    date(2010, 1, 1),                       # New Year
    date(2010, 2, 15), date(2010, 2, 16), date(2010, 2, 17), # Spring Festival
    date(2010, 2, 18), date(2010, 2, 19),                    # Spring Festival
    date(2010, 4, 5),                       # Qingming
    date(2010, 5, 3),                       # Labor Day
    date(2010, 6, 14), date(2010, 6, 15), date(2010, 6, 16), # Dragon Boat
    date(2010, 9, 22), date(2010, 9, 23), date(2010, 9, 24), # Mid-Autumn
    date(2010, 10, 1), date(2010, 10, 4), date(2010, 10, 5), # National Day
    date(2010, 10, 6), date(2010, 10, 7),                    # National Day
})

# 2011
_HOLIDAYS.update({
    date(2011, 1, 3),                       # New Year
    date(2011, 2, 2), date(2011, 2, 3), date(2011, 2, 4),  # Spring Festival
    date(2011, 2, 7), date(2011, 2, 8),                     # Spring Festival
    date(2011, 4, 4), date(2011, 4, 5),    # Qingming
    date(2011, 5, 2),                       # Labor Day
    date(2011, 6, 6),                       # Dragon Boat
    date(2011, 9, 12),                      # Mid-Autumn
    date(2011, 10, 3), date(2011, 10, 4), date(2011, 10, 5), # National Day
    date(2011, 10, 6), date(2011, 10, 7),                    # National Day
})

# 2012
_HOLIDAYS.update({
    date(2012, 1, 2), date(2012, 1, 3),    # New Year
    date(2012, 1, 23), date(2012, 1, 24), date(2012, 1, 25), # Spring Festival
    date(2012, 1, 26), date(2012, 1, 27),                    # Spring Festival
    date(2012, 4, 2), date(2012, 4, 3), date(2012, 4, 4),   # Qingming
    date(2012, 4, 30), date(2012, 5, 1),   # Labor Day
    date(2012, 6, 22),                      # Dragon Boat (Sat was Jun 23)
    date(2012, 10, 1), date(2012, 10, 2), date(2012, 10, 3), # National Day
    date(2012, 10, 4), date(2012, 10, 5),                    # National Day
})

# 2013
_HOLIDAYS.update({
    date(2013, 1, 1), date(2013, 1, 2), date(2013, 1, 3),  # New Year
    date(2013, 2, 11), date(2013, 2, 12), date(2013, 2, 13), # Spring Festival
    date(2013, 2, 14), date(2013, 2, 15),                    # Spring Festival
    date(2013, 4, 4), date(2013, 4, 5),    # Qingming
    date(2013, 4, 29), date(2013, 4, 30), date(2013, 5, 1), # Labor Day
    date(2013, 6, 10), date(2013, 6, 11), date(2013, 6, 12), # Dragon Boat
    date(2013, 9, 19), date(2013, 9, 20),  # Mid-Autumn
    date(2013, 10, 1), date(2013, 10, 2), date(2013, 10, 3), # National Day
    date(2013, 10, 4), date(2013, 10, 7),                    # National Day
})

# 2014
_HOLIDAYS.update({
    date(2014, 1, 1),                       # New Year
    date(2014, 1, 31),                      # Spring Festival (Jan 31-Feb 6)
    date(2014, 2, 3), date(2014, 2, 4), date(2014, 2, 5), date(2014, 2, 6),
    date(2014, 4, 7),                       # Qingming
    date(2014, 5, 1), date(2014, 5, 2),    # Labor Day
    date(2014, 6, 2),                       # Dragon Boat
    date(2014, 9, 8),                       # Mid-Autumn
    date(2014, 10, 1), date(2014, 10, 2), date(2014, 10, 3), # National Day
    date(2014, 10, 6), date(2014, 10, 7),                    # National Day
})

# 2015
_HOLIDAYS.update({
    date(2015, 1, 1), date(2015, 1, 2),    # New Year
    date(2015, 2, 18), date(2015, 2, 19), date(2015, 2, 20), # Spring Festival
    date(2015, 2, 23), date(2015, 2, 24),                    # Spring Festival
    date(2015, 4, 6),                       # Qingming
    date(2015, 5, 1),                       # Labor Day
    date(2015, 6, 22),                      # Dragon Boat
    date(2015, 9, 28),                      # Mid-Autumn (Sep 27 was Sun)
    date(2015, 10, 1), date(2015, 10, 2),   # National Day
    date(2015, 10, 5), date(2015, 10, 6), date(2015, 10, 7), # National Day
})

# 2016
_HOLIDAYS.update({
    date(2016, 1, 1),                       # New Year
    date(2016, 2, 8), date(2016, 2, 9), date(2016, 2, 10),  # Spring Festival
    date(2016, 2, 11), date(2016, 2, 12),                    # Spring Festival
    date(2016, 4, 4),                       # Qingming
    date(2016, 5, 2),                       # Labor Day
    date(2016, 6, 9), date(2016, 6, 10),    # Dragon Boat
    date(2016, 9, 15), date(2016, 9, 16),   # Mid-Autumn
    date(2016, 10, 3), date(2016, 10, 4), date(2016, 10, 5), # National Day
    date(2016, 10, 6), date(2016, 10, 7),                    # National Day
})

# 2017
_HOLIDAYS.update({
    date(2017, 1, 2),                       # New Year
    date(2017, 1, 27),                      # Spring Festival (Jan 27-Feb 2)
    date(2017, 1, 30), date(2017, 1, 31), date(2017, 2, 1), date(2017, 2, 2),
    date(2017, 4, 3), date(2017, 4, 4),    # Qingming
    date(2017, 5, 1),                       # Labor Day
    date(2017, 5, 29), date(2017, 5, 30),   # Dragon Boat
    date(2017, 10, 2), date(2017, 10, 3), date(2017, 10, 4), # National Day
    date(2017, 10, 5), date(2017, 10, 6),                    # National Day
})

# 2018
_HOLIDAYS.update({
    date(2018, 1, 1),                       # New Year
    date(2018, 2, 15), date(2018, 2, 16),   # Spring Festival
    date(2018, 2, 19), date(2018, 2, 20), date(2018, 2, 21),
    date(2018, 4, 5), date(2018, 4, 6),    # Qingming
    date(2018, 4, 30), date(2018, 5, 1),    # Labor Day
    date(2018, 6, 18),                      # Dragon Boat
    date(2018, 9, 24),                      # Mid-Autumn
    date(2018, 10, 1), date(2018, 10, 2), date(2018, 10, 3), # National Day
    date(2018, 10, 4), date(2018, 10, 5),                    # National Day
})

# 2019
_HOLIDAYS.update({
    date(2019, 1, 1),                       # New Year
    date(2019, 2, 4), date(2019, 2, 5), date(2019, 2, 6),  # Spring Festival
    date(2019, 2, 7), date(2019, 2, 8),                     # Spring Festival
    date(2019, 4, 5),                       # Qingming
    date(2019, 5, 1), date(2019, 5, 2), date(2019, 5, 3),  # Labor Day
    date(2019, 6, 7),                       # Dragon Boat
    date(2019, 9, 13),                      # Mid-Autumn
    date(2019, 10, 1), date(2019, 10, 2), date(2019, 10, 3), # National Day
    date(2019, 10, 4), date(2019, 10, 7),                    # National Day
})

# 2020
_HOLIDAYS.update({
    date(2020, 1, 1),                       # New Year
    date(2020, 1, 24),                      # Spring Festival (extended for COVID)
    date(2020, 1, 27), date(2020, 1, 28), date(2020, 1, 29), date(2020, 1, 30),
    date(2020, 1, 31),                      # COVID extension
    date(2020, 4, 6),                       # Qingming
    date(2020, 5, 1), date(2020, 5, 4), date(2020, 5, 5),  # Labor Day
    date(2020, 6, 25), date(2020, 6, 26),   # Dragon Boat
    date(2020, 10, 1), date(2020, 10, 2),   # National Day + Mid-Autumn
    date(2020, 10, 5), date(2020, 10, 6), date(2020, 10, 7), date(2020, 10, 8),
})

# 2021
_HOLIDAYS.update({
    date(2021, 1, 1),                       # New Year
    date(2021, 2, 11), date(2021, 2, 12), date(2021, 2, 15), # Spring Festival
    date(2021, 2, 16), date(2021, 2, 17),                    # Spring Festival
    date(2021, 4, 5),                       # Qingming
    date(2021, 5, 3), date(2021, 5, 4), date(2021, 5, 5),   # Labor Day
    date(2021, 6, 14),                      # Dragon Boat
    date(2021, 9, 20), date(2021, 9, 21),   # Mid-Autumn
    date(2021, 10, 1), date(2021, 10, 4), date(2021, 10, 5), # National Day
    date(2021, 10, 6), date(2021, 10, 7),                    # National Day
})

# 2022
_HOLIDAYS.update({
    date(2022, 1, 3),                       # New Year
    date(2022, 1, 31), date(2022, 2, 1), date(2022, 2, 2),  # Spring Festival
    date(2022, 2, 3), date(2022, 2, 4),                     # Spring Festival
    date(2022, 4, 4), date(2022, 4, 5),    # Qingming
    date(2022, 5, 2), date(2022, 5, 3), date(2022, 5, 4),  # Labor Day
    date(2022, 6, 3),                       # Dragon Boat
    date(2022, 9, 12),                      # Mid-Autumn
    date(2022, 10, 3), date(2022, 10, 4), date(2022, 10, 5), # National Day
    date(2022, 10, 6), date(2022, 10, 7),                    # National Day
})

# 2023
_HOLIDAYS.update({
    date(2023, 1, 2),                       # New Year
    date(2023, 1, 23), date(2023, 1, 24), date(2023, 1, 25), # Spring Festival
    date(2023, 1, 26), date(2023, 1, 27),                    # Spring Festival
    date(2023, 4, 5),                       # Qingming
    date(2023, 5, 1), date(2023, 5, 2), date(2023, 5, 3),   # Labor Day
    date(2023, 6, 22), date(2023, 6, 23),   # Dragon Boat
    date(2023, 9, 29),                      # Mid-Autumn + National Day
    date(2023, 10, 2), date(2023, 10, 3), date(2023, 10, 4), # National Day
    date(2023, 10, 5), date(2023, 10, 6),                    # National Day
})

# 2024
_HOLIDAYS.update({
    date(2024, 1, 1),                       # New Year
    date(2024, 2, 12), date(2024, 2, 13), date(2024, 2, 14), # Spring Festival
    date(2024, 2, 15), date(2024, 2, 16),                    # Spring Festival
    date(2024, 4, 4), date(2024, 4, 5),    # Qingming
    date(2024, 5, 1), date(2024, 5, 2), date(2024, 5, 3),   # Labor Day
    date(2024, 6, 10),                      # Dragon Boat
    date(2024, 9, 16), date(2024, 9, 17),   # Mid-Autumn
    date(2024, 10, 1), date(2024, 10, 2), date(2024, 10, 3), # National Day
    date(2024, 10, 4), date(2024, 10, 7),                    # National Day
})

# 2025
_HOLIDAYS.update({
    date(2025, 1, 1),                       # New Year
    date(2025, 1, 28), date(2025, 1, 29), date(2025, 1, 30), # Spring Festival
    date(2025, 1, 31), date(2025, 2, 3), date(2025, 2, 4),   # Spring Festival
    date(2025, 4, 4),                       # Qingming
    date(2025, 5, 1), date(2025, 5, 2), date(2025, 5, 5),   # Labor Day
    date(2025, 6, 2),                       # Dragon Boat (May 31 was Sat)
    date(2025, 10, 1), date(2025, 10, 2), date(2025, 10, 3), # National Day + Mid-Autumn
    date(2025, 10, 6), date(2025, 10, 7), date(2025, 10, 8), # National Day
})

# 2026 — per State Council announcement (referenced in ADR-0004)
_HOLIDAYS.update({
    date(2026, 1, 1), date(2026, 1, 2),    # New Year
    date(2026, 2, 16), date(2026, 2, 17), date(2026, 2, 18), # Spring Festival (Feb 16-20)
    date(2026, 2, 19), date(2026, 2, 20),                    # Spring Festival
    date(2026, 4, 6),                       # Qingming (Apr 5 Sun, observed Mon)
    date(2026, 5, 1), date(2026, 5, 4), date(2026, 5, 5),   # Labor Day
    date(2026, 6, 22),                      # Dragon Boat (lunar 5/5 ~Jun 19 Fri → observed Jun 22 Mon)
    date(2026, 9, 25),                      # Mid-Autumn (lunar 8/15 ~Sep 25 Fri)
    date(2026, 10, 1), date(2026, 10, 2),   # National Day
    date(2026, 10, 5), date(2026, 10, 6), date(2026, 10, 7), date(2026, 10, 8), # National Day
})


# Working weekend adjustments — days that are Saturday/Sunday but are
# designated as working days to compensate for long holiday breaks.
_WORKING_WEEKENDS: Set[date] = set()

# 2026 working weekends (compensating for Spring Festival and National Day)
_WORKING_WEEKENDS.update({
    date(2026, 2, 14),   # Sat → work for Spring Festival
    date(2026, 10, 10),  # Sat → work for National Day
})

# 2025 working weekends
_WORKING_WEEKENDS.update({
    date(2025, 1, 25),   # Sun → work for Spring Festival
    date(2025, 2, 8),    # Sat → work for Spring Festival
    date(2025, 4, 27),   # Sun → work for Labor Day
    date(2025, 9, 28),   # Sun → work for National Day
    date(2025, 10, 11),  # Sat → work for National Day
})

# 2024 working weekends
_WORKING_WEEKENDS.update({
    date(2024, 2, 4),    # Sun → work for Spring Festival
    date(2024, 2, 17),   # Sat → work for Spring Festival (Feb 18 was Sun → rest)
    date(2024, 4, 7),    # Sun → work for Qingming
    date(2024, 4, 28),   # Sun → work for Labor Day
    date(2024, 5, 11),   # Sat → work for Labor Day
    date(2024, 9, 14),   # Sat → work for Mid-Autumn
    date(2024, 9, 29),   # Sun → work for National Day
    date(2024, 10, 12),  # Sat → work for National Day
})

# ── Calendar API ────────────────────────────────────────────────────────

CALENDAR_ID = "cgb-reference-calendar-v1"
COVERAGE_START = date(2005, 1, 1)
COVERAGE_END = date(2026, 12, 31)


def is_business_day(d: date) -> Tuple[bool, str]:
    """
    Return (is_business_day, resolution).

    resolution is:
      - 'EXACT' for dates within 2005-01-01..2026-12-31
      - 'PROVISIONAL_WEEKEND_ONLY' for dates 2027-01-01 and later
      - 'OUT_OF_COVERAGE' for dates before 2005-01-01
    """
    if d < COVERAGE_START:
        return False, "OUT_OF_COVERAGE"

    if d > COVERAGE_END:
        # 2027+ provisional: weekend-only
        if d.weekday() >= 5:  # Saturday=5, Sunday=6
            return False, "PROVISIONAL_WEEKEND_ONLY"
        return True, "PROVISIONAL_WEEKEND_ONLY"

    # Exact coverage: holidays override, working weekends override
    if d in _WORKING_WEEKENDS:
        return True, "EXACT"

    if d.weekday() >= 5:
        return False, "EXACT"

    if d in _HOLIDAYS:
        return False, "EXACT"

    return True, "EXACT"


def adjust_following(d: date) -> date:
    """Adjust to next business day using Following convention."""
    result = d
    for _ in range(30):  # safety limit
        is_bd, _ = is_business_day(result)
        if is_bd:
            return result
        result += timedelta(days=1)
    raise ValueError(f"Could not find business day after {d}")


def add_calendar_days(d: date, n: int) -> date:
    """Add n calendar days (skip non-business days using Following)."""
    result = d
    for _ in range(n):
        result += timedelta(days=1)
        result = adjust_following(result)
    return result
