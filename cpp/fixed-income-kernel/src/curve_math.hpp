#ifndef FICANT_KERNEL_CURVE_MATH_HPP
#define FICANT_KERNEL_CURVE_MATH_HPP

#include <cstdint>

namespace ficant::curve_math {

struct YieldNode {
    int32_t maturity_date;
    double yield_to_maturity;
};

/** Interpolate linearly in YTM against actual epoch-day distance. */
double linear_yield(const YieldNode* nodes, uint32_t count,
                    int32_t query_date) noexcept;

} // namespace ficant::curve_math

#endif
