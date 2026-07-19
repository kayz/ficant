#ifndef FICANT_KERNEL_CURVE_MATH_HPP
#define FICANT_KERNEL_CURVE_MATH_HPP

#include <cstdint>

namespace ficant::curve_math {

struct YieldNode {
    int32_t maturity_date;
    double yield_to_maturity;
};

struct CarryRollResult {
    double carry;
    double roll_down;
    double total_return;
};

/** Interpolate linearly in YTM against actual epoch-day distance. */
double linear_yield(const YieldNode* nodes, uint32_t count,
                    int32_t query_date) noexcept;

CarryRollResult decompose_carry_roll(
    double initial_dirty_price,
    double horizon_dirty_at_initial_yield,
    double horizon_dirty_at_rolled_yield,
    double paid_cashflows) noexcept;

} // namespace ficant::curve_math

#endif
