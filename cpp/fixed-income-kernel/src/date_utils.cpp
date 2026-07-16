#include "date_utils.hpp"

#include <algorithm>
#include <cstdint>

namespace ficant {
namespace date_utils {

namespace {

/** Binary search: true if `value` is present in sorted array [begin, end). */
bool sorted_contains(const int32_t* begin, const int32_t* end, int32_t value) noexcept {
    return std::binary_search(begin, end, value);
}

} // anonymous namespace

bool is_business_day(int32_t date,
                     const int32_t* non_biz_days, uint32_t non_biz_count,
                     const int32_t* work_we,      uint32_t work_we_count) noexcept
{
    const bool weekend = is_weekend(date);
    const bool is_work_we = (work_we_count > 0)
        && sorted_contains(work_we, work_we + work_we_count, date);
    const bool is_holiday = (non_biz_count > 0)
        && sorted_contains(non_biz_days, non_biz_days + non_biz_count, date);

    // Business if: (not weekend, or explicitly work-weekend) AND not holiday
    return (!weekend || is_work_we) && !is_holiday;
}

int32_t following_adjust(int32_t date,
                         const int32_t* non_biz_days, uint32_t non_biz_count,
                         const int32_t* work_we,      uint32_t work_we_count,
                         int32_t calendar_coverage_end,
                         bool exact_coverage_required,
                         bool& used_provisional) noexcept
{
    used_provisional = false;
    int32_t adjusted = date;

    while (true) {
        // Determine which calendar to use for this date.
        bool beyond_coverage = adjusted > calendar_coverage_end;
        if (beyond_coverage && exact_coverage_required) {
            // Cannot adjust: date outside exact coverage and EXACT_MARKET required.
            // Return the date unadjusted — caller will detect the error.
            return adjusted;
        }

        bool is_biz;
        if (beyond_coverage) {
            // PROVISIONAL: weekend-only rule — business if not Saturday or Sunday.
            is_biz = !is_weekend(adjusted);
            used_provisional = true;
        } else {
            is_biz = is_business_day(adjusted,
                                     non_biz_days, non_biz_count,
                                     work_we, work_we_count);
        }

        if (is_biz) break;
        // Advance by one calendar day (epoch days increment).
        adjusted = adjusted + 1;
    }

    return adjusted;
}

bool dates_need_provisional(const int32_t* dates, uint32_t count,
                            int32_t coverage_start, int32_t coverage_end) noexcept
{
    for (uint32_t i = 0; i < count; ++i) {
        if (dates[i] < coverage_start || dates[i] > coverage_end) {
            return true;
        }
    }
    return false;
}

} // namespace date_utils
} // namespace ficant
