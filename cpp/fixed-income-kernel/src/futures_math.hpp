#ifndef FICANT_KERNEL_FUTURES_MATH_HPP
#define FICANT_KERNEL_FUTURES_MATH_HPP

#include <cstdint>

namespace ficant::futures_math {

struct CouponScheduleMetrics {
    uint32_t months_to_next_coupon;
    uint32_t remaining_coupon_count;
    double purchase_accrued_interest;
    double delivery_accrued_interest;
    double interim_coupons;
};

bool is_deliverable(uint32_t original_term_max_months,
                    uint32_t residual_min_months,
                    uint32_t residual_max_months,
                    bool residual_max_months_unbounded,
                    int32_t issue_date,
                          int32_t maturity_date,
                          int32_t delivery_month_first) noexcept;

double cffex_conversion_factor(double coupon_rate,
                               double nominal_coupon,
                               uint32_t frequency,
                               uint32_t months_to_next_coupon,
                               uint32_t remaining_coupon_count,
                               uint32_t rounding_places) noexcept;

bool coupon_schedule_metrics(int32_t issue_date,
                             int32_t maturity_date,
                             uint32_t frequency,
                             double coupon_rate,
                             double face_quote_basis,
                             uint32_t accrued_interest_day_count,
                             uint32_t accrued_interest_rounding_places,
                             int32_t purchase_date,
                             int32_t delivery_month_first,
                             int32_t delivery_date,
                             CouponScheduleMetrics& metrics);

} // namespace ficant::futures_math

#endif
