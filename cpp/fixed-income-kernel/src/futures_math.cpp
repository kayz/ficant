#include "futures_math.hpp"

#include "date_utils.hpp"
#include "ficant_kernel.h"

#include <cmath>
#include <cstdint>
#include <algorithm>
#include <vector>

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

namespace {

double round_to(double value, double scale) noexcept {
    return std::round(value * scale) / scale;
}

double accrued_interest(const std::vector<int32_t>& coupons,
                        int32_t issue_date,
                        int32_t date,
                        double coupon_amount) noexcept {
    const auto next = std::upper_bound(coupons.begin(), coupons.end(), date);
    if (next == coupons.end()) {
        return 0.0;
    }
    const int32_t previous = next == coupons.begin() ? issue_date : *(next - 1);
    const double elapsed = static_cast<double>(date - previous);
    const double period = static_cast<double>(*next - previous);
    return round_to(coupon_amount * elapsed / period, 10000000.0);
}

} // namespace

bool coupon_schedule_metrics(int32_t issue_date,
                             int32_t maturity_date,
                             uint32_t frequency,
                             double coupon_rate,
                             int32_t purchase_date,
                             int32_t delivery_month_first,
                             int32_t delivery_date,
                             CouponScheduleMetrics& metrics) {
    constexpr uint32_t maximum_coupons = 1000;
    const int period_months = 12 / static_cast<int>(frequency);
    std::vector<int32_t> coupons;
    int32_t coupon_date = maturity_date;
    while (coupon_date > issue_date && coupons.size() < maximum_coupons) {
        coupons.push_back(coupon_date);
        coupon_date = date_utils::add_months(coupon_date, -period_months);
    }
    if (coupon_date != issue_date || coupons.empty() || coupons.size() >= maximum_coupons) {
        return false;
    }
    std::reverse(coupons.begin(), coupons.end());
    const auto conversion_coupon =
        std::lower_bound(coupons.begin(), coupons.end(), delivery_month_first);
    if (conversion_coupon == coupons.end()) {
        return false;
    }
    int delivery_year = 0;
    unsigned delivery_month = 0;
    unsigned delivery_day = 0;
    int coupon_year = 0;
    unsigned coupon_month = 0;
    unsigned coupon_day = 0;
    date_utils::days_to_ymd(
        delivery_month_first, delivery_year, delivery_month, delivery_day);
    date_utils::days_to_ymd(*conversion_coupon, coupon_year, coupon_month, coupon_day);
    static_cast<void>(delivery_day);
    static_cast<void>(coupon_day);
    const int month_difference =
        (coupon_year - delivery_year) * 12
        + static_cast<int>(coupon_month) - static_cast<int>(delivery_month);
    if (month_difference < 0) {
        return false;
    }
    const double coupon_amount = coupon_rate * 100.0 / static_cast<double>(frequency);
    metrics.months_to_next_coupon = static_cast<uint32_t>(month_difference);
    metrics.remaining_coupon_count =
        static_cast<uint32_t>(coupons.end() - conversion_coupon);
    metrics.purchase_accrued_interest =
        accrued_interest(coupons, issue_date, purchase_date, coupon_amount);
    metrics.delivery_accrued_interest =
        accrued_interest(coupons, issue_date, delivery_date, coupon_amount);
    metrics.interim_coupons = coupon_amount
        * static_cast<double>(std::count_if(
            coupons.begin(), coupons.end(), [purchase_date, delivery_date](int32_t value) {
                return value > purchase_date && value <= delivery_date;
            }));
    return metrics.months_to_next_coupon < 12U / frequency
        && metrics.remaining_coupon_count > 0;
}

} // namespace ficant::futures_math
