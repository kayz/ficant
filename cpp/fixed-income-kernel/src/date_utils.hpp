#ifndef FICANT_KERNEL_DATE_UTILS_HPP
#define FICANT_KERNEL_DATE_UTILS_HPP

#include <cstdint>

namespace ficant {
namespace date_utils {

/** Convert year/month/day to epoch days (1970-01-01 = day 0). */
constexpr int32_t ymd_to_days(int y, unsigned m, unsigned d) noexcept;

/** Convert epoch days to year/month/day. */
constexpr void days_to_ymd(int32_t days, int& y, unsigned& m, unsigned& d) noexcept;

/** Number of days in a given month (1-12). */
constexpr unsigned days_in_month(int y, unsigned m) noexcept;

/** Whether a year is a leap year in the proleptic Gregorian calendar. */
constexpr bool is_leap_year(int y) noexcept;

/** Day of week: 0=Sunday, 1=Monday, …, 6=Saturday. */
constexpr int day_of_week(int32_t epoch_days) noexcept;

/** True if Saturday or Sunday. */
constexpr bool is_weekend(int32_t epoch_days) noexcept;

/**
 * True if `date` is a business day.
 *
 * A business day is a day that is NOT a weekend day,
 * OR is explicitly listed as a work-weekend,
 * AND is NOT listed as a non-business day (holiday).
 *
 * `non_biz_days` and `work_we` must each be sorted ascending with no duplicates.
 * Binary search is used for O(log n) lookup.
 */
bool is_business_day(int32_t date,
                     const int32_t* non_biz_days, uint32_t non_biz_count,
                     const int32_t* work_we,      uint32_t work_we_count) noexcept;

/**
 * Apply Following business-day convention.
 * If `date` is not a business day, advance day-by-day until it is.
 * Calendar parameters are passed explicitly.
 *
 * If after `calendar_coverage_end` and the calendar requirement is
 * REFERENCE_REPLAY, weekend-only logic is applied: Saturdays and Sundays
 * are non-business days; no holiday/weekend-override data is consulted.
 * The `used_provisional` flag is set to true when weekend-only logic is used.
 */
int32_t following_adjust(int32_t date,
                         const int32_t* non_biz_days, uint32_t non_biz_count,
                         const int32_t* work_we,      uint32_t work_we_count,
                         int32_t calendar_coverage_end,
                         bool exact_coverage_required,
                         bool& used_provisional) noexcept;

/** True if any date needed is outside exact calendar coverage. */
bool dates_need_provisional(const int32_t* dates, uint32_t count,
                            int32_t coverage_start, int32_t coverage_end) noexcept;

/** Add `months` months to epoch_days (end_of_month = false). */
constexpr int32_t add_months(int32_t epoch_days, int months) noexcept;

/* ── implementations ────────────────────────────────────────────── */

constexpr int32_t ymd_to_days(int y, unsigned m, unsigned d) noexcept {
    y -= static_cast<int>(m <= 2);
    const int era = (y >= 0 ? y : y - 399) / 400;
    const unsigned yoe = static_cast<unsigned>(y - era * 400);
    const unsigned doy = (153 * (m > 2 ? m - 3 : m + 9) + 2) / 5 + d - 1;
    const unsigned doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    return static_cast<int32_t>(era * 146097LL + doe - 719468LL);
}

constexpr void days_to_ymd(int32_t days, int& y, unsigned& m, unsigned& d) noexcept {
    const long long z = static_cast<long long>(days) + 719468LL;
    const long long era = (z >= 0 ? z : z - 146096LL) / 146097LL;
    const unsigned doe = static_cast<unsigned>(z - era * 146097LL);
    const unsigned yoe = (doe - doe / 1460U + doe / 36524U - doe / 146096U) / 365U;
    y = static_cast<int>(yoe + era * 400LL);
    const unsigned doy = doe - (365U * yoe + yoe / 4U - yoe / 100U);
    const unsigned mp = (5U * doy + 2U) / 153U;
    d = doy - (153U * mp + 2U) / 5U + 1U;
    m = mp + (mp < 10U ? 3U : 9U);
    y += static_cast<int>(m <= 2U);
}

constexpr unsigned days_in_month(int y, unsigned m) noexcept {
    constexpr unsigned md[] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};
    if (m == 2 && is_leap_year(y)) return 29;
    return md[m - 1];
}

constexpr bool is_leap_year(int y) noexcept {
    return (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
}

constexpr int day_of_week(int32_t epoch_days) noexcept {
    // 1970-01-01 was a Thursday.
    // (epoch_days + 4) % 7: 0=Sun, 1=Mon, ..., 6=Sat
    int d = static_cast<int>((epoch_days + 4LL) % 7LL);
    return d < 0 ? d + 7 : d;
}

constexpr bool is_weekend(int32_t epoch_days) noexcept {
    const int dow = day_of_week(epoch_days);
    return dow == 0 || dow == 6;
}

constexpr int32_t add_months(int32_t epoch_days, int months) noexcept {
    int y;
    unsigned m, d;
    days_to_ymd(epoch_days, y, m, d);
    int total = static_cast<int>(y) * 12 + static_cast<int>(m) - 1 + months;
    int ny = total / 12;
    unsigned nm = static_cast<unsigned>(total % 12 + 1);
    if (total < 0 && total % 12 != 0) {
        ny -= 1;
        nm = static_cast<unsigned>(12 + (total % 12) + 1);
    }
    // Clamp day to the number of days in the new month.
    unsigned dim = days_in_month(ny, nm);
    unsigned nd = (d > dim) ? dim : d;
    return ymd_to_days(ny, nm, nd);
}

} // namespace date_utils
} // namespace ficant

#endif
