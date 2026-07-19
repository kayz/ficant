#ifndef FICANT_KERNEL_FUTURES_MATH_HPP
#define FICANT_KERNEL_FUTURES_MATH_HPP

#include <cstdint>

namespace ficant::futures_math {

bool is_cffex_deliverable(uint32_t product,
                          int32_t issue_date,
                          int32_t maturity_date,
                          int32_t delivery_month_first) noexcept;

double cffex_conversion_factor(double coupon_rate,
                               uint32_t frequency,
                               uint32_t months_to_next_coupon,
                               uint32_t remaining_coupon_count) noexcept;

} // namespace ficant::futures_math

#endif
