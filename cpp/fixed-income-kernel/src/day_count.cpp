#include "day_count.hpp"
#include "date_utils.hpp"

#include <algorithm>
#include <cstdint>

namespace ficant {
namespace day_count {

double act_act_bond_isma(int32_t start_date, int32_t end_date,
                         int32_t period_start, int32_t period_end,
                         uint32_t frequency) noexcept
{
    const int32_t actual_days = end_date - start_date;
    if (actual_days <= 0) return 0.0;
    const int32_t period_days = period_end - period_start;
    if (period_days <= 0) return 0.0;
    // year_frac = actual_days / (period_days * frequency)
    return static_cast<double>(actual_days)
        / (static_cast<double>(period_days) * static_cast<double>(frequency));
}

double act_act_natural(int32_t start_date, int32_t end_date) noexcept {
    if (end_date <= start_date) return 0.0;

    int y1, y2;
    unsigned m1, m2, d1, d2;
    date_utils::days_to_ymd(start_date, y1, m1, d1);
    date_utils::days_to_ymd(end_date,   y2, m2, d2);

    double yf = 0.0;

    // If within the same calendar year, simple computation.
    if (y1 == y2) {
        int32_t days = end_date - start_date;
        double year_len = date_utils::is_leap_year(y1) ? 366.0 : 365.0;
        return static_cast<double>(days) / year_len;
    }

    // Crosses year boundaries.
    // Days remaining in year y1.
    int32_t end_of_y1 = date_utils::ymd_to_days(y1, 12, 31);
    int32_t days_in_y1 = end_of_y1 - start_date + 1;
    yf += static_cast<double>(days_in_y1) / (date_utils::is_leap_year(y1) ? 366.0 : 365.0);

    // Full intermediate years (each contributes exactly 1.0).
    for (int y = y1 + 1; y < y2; ++y) {
        (void)y;
        yf += 1.0;
    }

    // Days in year y2 up to end_date.
    int32_t start_of_y2 = date_utils::ymd_to_days(y2, 1, 1);
    int32_t days_in_y2 = end_date - start_of_y2;
    yf += static_cast<double>(days_in_y2) / (date_utils::is_leap_year(y2) ? 366.0 : 365.0);

    return yf;
}

} // namespace day_count
} // namespace ficant
