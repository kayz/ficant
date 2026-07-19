#include "futures_math.hpp"

#include "date_utils.hpp"
#include "ficant_kernel.h"

#include <cmath>
#include <cstdint>

namespace ficant::futures_math {

namespace {

struct EligibilityBounds {
    int original_months;
    int minimum_residual_months;
    int maximum_residual_months;
};

bool bounds_for(uint32_t product, EligibilityBounds& bounds) noexcept {
    switch (product) {
    case FICANT_KERNEL_CGB_FUTURES_TS:
        bounds = {60, 18, 27};
        return true;
    case FICANT_KERNEL_CGB_FUTURES_TF:
        bounds = {84, 48, 63};
        return true;
    case FICANT_KERNEL_CGB_FUTURES_T:
        bounds = {120, 78, 0};
        return true;
    case FICANT_KERNEL_CGB_FUTURES_TL:
        bounds = {360, 300, 0};
        return true;
    default:
        return false;
    }
}

} // namespace

bool is_cffex_deliverable(uint32_t product,
                          int32_t issue_date,
                          int32_t maturity_date,
                          int32_t delivery_month_first) noexcept {
    EligibilityBounds bounds{};
    if (!bounds_for(product, bounds) || issue_date >= maturity_date) {
        return false;
    }
    const int32_t original_limit = date_utils::add_months(issue_date, bounds.original_months);
    const int32_t residual_minimum =
        date_utils::add_months(delivery_month_first, bounds.minimum_residual_months);
    if (maturity_date > original_limit || maturity_date < residual_minimum) {
        return false;
    }
    if (bounds.maximum_residual_months > 0) {
        const int32_t residual_maximum =
            date_utils::add_months(delivery_month_first, bounds.maximum_residual_months);
        if (maturity_date > residual_maximum) {
            return false;
        }
    }
    return true;
}

double cffex_conversion_factor(double coupon_rate,
                               uint32_t frequency,
                               uint32_t months_to_next_coupon,
                               uint32_t remaining_coupon_count) noexcept {
    constexpr double standard_coupon = 0.03;
    const double f = static_cast<double>(frequency);
    const double x = static_cast<double>(months_to_next_coupon);
    const double n = static_cast<double>(remaining_coupon_count);
    const double base = 1.0 + standard_coupon / f;
    const double stub_period = x * f / 12.0;
    const double discounted = std::pow(base, -stub_period)
        * (coupon_rate / f + coupon_rate / standard_coupon
           + (1.0 - coupon_rate / standard_coupon) * std::pow(base, -(n - 1.0)));
    const double accrued_adjustment = coupon_rate / f * (1.0 - stub_period);
    return std::round((discounted - accrued_adjustment) * 10000.0) / 10000.0;
}

} // namespace ficant::futures_math
