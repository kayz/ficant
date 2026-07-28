#include "futures_math.hpp"

#include "date_utils.hpp"
#include "ficant_kernel.h"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <limits>
#include <vector>

namespace ficant::futures_math {

namespace {

bool scale_for_places(uint32_t places, double& scale) noexcept {
    if (places > 12U) {
        return false;
    }
    scale = std::pow(10.0, static_cast<double>(places));
    return std::isfinite(scale) && scale > 0.0;
}

double round_to(double value, uint32_t places) noexcept {
    double scale = 0.0;
    if (!scale_for_places(places, scale)) {
        return std::numeric_limits<double>::quiet_NaN();
    }
    return std::round(value * scale) / scale;
}

bool accrued_interest(const std::vector<int32_t>& coupons,
                      int32_t issue_date,
                      int32_t date,
                      double coupon_amount,
                      uint32_t day_count,
                      uint32_t rounding_places,
                      double& value) noexcept {
    if (day_count != FICANT_KERNEL_DAY_COUNT_ACT_ACT_BOND_ISMA) {
        return false;
    }
    const auto next = std::upper_bound(coupons.begin(), coupons.end(), date);
    if (next == coupons.end()) {
        value = 0.0;
        return true;
    }
    const int32_t previous = next == coupons.begin() ? issue_date : *(next - 1);
    const double elapsed = static_cast<double>(date - previous);
    const double period = static_cast<double>(*next - previous);
    if (period <= 0.0) {
        return false;
    }
    value = round_to(coupon_amount * elapsed / period, rounding_places);
    return std::isfinite(value);
}

} // namespace

bool is_deliverable(uint32_t original_term_max_months,
                    uint32_t residual_min_months,
                    uint32_t residual_max_months,
                    bool residual_max_months_unbounded,
                    int32_t issue_date,
                    int32_t maturity_date,
                    int32_t delivery_month_first) noexcept {
    if (original_term_max_months == 0U || residual_min_months == 0U
        || (!residual_max_months_unbounded
            && (residual_max_months == 0U || residual_max_months < residual_min_months))
        || issue_date >= maturity_date) {
        return false;
    }
    const int32_t original_limit =
        date_utils::add_months(issue_date, static_cast<int>(original_term_max_months));
    const int32_t residual_minimum =
        date_utils::add_months(delivery_month_first, static_cast<int>(residual_min_months));
    if (maturity_date > original_limit || maturity_date < residual_minimum) {
        return false;
    }
    if (!residual_max_months_unbounded) {
        const int32_t residual_maximum =
            date_utils::add_months(delivery_month_first, static_cast<int>(residual_max_months));
        if (maturity_date > residual_maximum) {
            return false;
        }
    }
    return true;
}

double cffex_conversion_factor(double coupon_rate,
                               double nominal_coupon,
                               uint32_t frequency,
                               uint32_t months_to_next_coupon,
                               uint32_t remaining_coupon_count,
                               uint32_t rounding_places) noexcept {
    if (nominal_coupon <= 0.0 || frequency == 0U || remaining_coupon_count == 0U) {
        return std::numeric_limits<double>::quiet_NaN();
    }
    const double f = static_cast<double>(frequency);
    const double x = static_cast<double>(months_to_next_coupon);
    const double n = static_cast<double>(remaining_coupon_count);
    const double base = 1.0 + nominal_coupon / f;
    const double stub_period = x * f / 12.0;
    const double discounted = std::pow(base, -stub_period)
        * (coupon_rate / f + coupon_rate / nominal_coupon
           + (1.0 - coupon_rate / nominal_coupon) * std::pow(base, -(n - 1.0)));
    const double accrued_adjustment = coupon_rate / f * (1.0 - stub_period);
    return round_to(discounted - accrued_adjustment, rounding_places);
}

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
                             CouponScheduleMetrics& metrics) {
    constexpr uint32_t maximum_coupons = 1000;
    if (frequency == 0U || face_quote_basis <= 0.0) {
        return false;
    }
    const int period_months = 12 / static_cast<int>(frequency);
    if (period_months == 0) {
        return false;
    }
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
    const double coupon_amount = coupon_rate * face_quote_basis / static_cast<double>(frequency);
    metrics.months_to_next_coupon = static_cast<uint32_t>(month_difference);
    metrics.remaining_coupon_count =
        static_cast<uint32_t>(coupons.end() - conversion_coupon);
    if (!accrued_interest(coupons, issue_date, purchase_date, coupon_amount,
                          accrued_interest_day_count, accrued_interest_rounding_places,
                          metrics.purchase_accrued_interest)
        || !accrued_interest(coupons, issue_date, delivery_date, coupon_amount,
                             accrued_interest_day_count, accrued_interest_rounding_places,
                             metrics.delivery_accrued_interest)) {
        return false;
    }
    metrics.interim_coupons = coupon_amount
        * static_cast<double>(std::count_if(
            coupons.begin(), coupons.end(), [purchase_date, delivery_date](int32_t value) {
                return value > purchase_date && value <= delivery_date;
            }));
    return metrics.months_to_next_coupon < 12U / frequency
        && metrics.remaining_coupon_count > 0;
}

} // namespace ficant::futures_math
