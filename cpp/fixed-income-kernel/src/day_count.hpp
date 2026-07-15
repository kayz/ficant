#ifndef FICANT_KERNEL_DAY_COUNT_HPP
#define FICANT_KERNEL_DAY_COUNT_HPP

#include <cstdint>

namespace ficant {
namespace day_count {

/**
 * Actual/Actual (Bond/ISMA) year fraction.
 *
 * For a period from `start_date` to `end_date` within a coupon period
 * starting at `period_start` and ending at `period_end`:
 *
 *   year_frac = actual_days / (days_in_period * frequency)
 *
 * For a full regular coupon period this yields exactly 1/frequency.
 *
 * Used for coupon-bearing bonds.
 */
double act_act_bond_isma(int32_t start_date, int32_t end_date,
                         int32_t period_start, int32_t period_end,
                         uint32_t frequency) noexcept;

/**
 * Actual/Actual year fraction for discount (zero-coupon) bonds.
 *
 * Splits the period by natural calendar year boundaries.
 * Each segment is weighted by its respective year length (365 or 366).
 *
 *   year_frac = sum(days_in_year_i / days_in_year_i_length)
 *
 * Used for the simple-yield discount bond formula.
 */
double act_act_natural(int32_t start_date, int32_t end_date) noexcept;

} // namespace day_count
} // namespace ficant

#endif
