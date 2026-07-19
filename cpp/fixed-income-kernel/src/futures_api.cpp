#include "ficant_kernel.h"

#include "date_utils.hpp"
#include "futures_math.hpp"

#include <cmath>
#include <cstdint>

namespace {

uint32_t finish(ficant_kernel_cgb_futures_delivery_result_v1* result,
                uint32_t status) noexcept {
    if (result != nullptr) {
        result->status_code = status;
    }
    return status;
}

void zero_result(ficant_kernel_cgb_futures_delivery_result_v1* result) noexcept {
    result->status_code = FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    result->eligible = 0;
    result->conversion_factor = 0.0;
    result->invoice_price = 0.0;
    result->purchase_dirty_price = 0.0;
    result->gross_basis = 0.0;
    result->financing_cost = 0.0;
    result->holding_carry = 0.0;
    result->net_basis = 0.0;
    result->implied_repo_rate = 0.0;
    result->delivery_profit = 0.0;
}

bool valid_product(uint32_t product) noexcept {
    return product >= FICANT_KERNEL_CGB_FUTURES_TS
        && product <= FICANT_KERNEL_CGB_FUTURES_TL;
}

bool valid_delivery_month(int32_t date) noexcept {
    int year = 0;
    unsigned month = 0;
    unsigned day = 0;
    ficant::date_utils::days_to_ymd(date, year, month, day);
    static_cast<void>(year);
    return day == 1U && (month == 3U || month == 6U || month == 9U || month == 12U);
}

bool all_finite(const ficant_kernel_cgb_futures_delivery_input_v1& input) noexcept {
    return std::isfinite(input.coupon_rate)
        && std::isfinite(input.spot_clean_price)
        && std::isfinite(input.purchase_accrued_interest)
        && std::isfinite(input.delivery_accrued_interest)
        && std::isfinite(input.interim_coupons)
        && std::isfinite(input.futures_clean_price)
        && std::isfinite(input.financing_rate);
}

} // namespace

extern "C" uint32_t ficant_kernel_analyze_cgb_futures_delivery_v1(
    const ficant_kernel_cgb_futures_delivery_input_v1* input,
    ficant_kernel_cgb_futures_delivery_result_v1* result) noexcept {
    if (result == nullptr) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    if (result->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return FICANT_KERNEL_STATUS_ABI_MISMATCH;
    }
    if (result->struct_size != sizeof(ficant_kernel_cgb_futures_delivery_result_v1)) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    zero_result(result);
    if (input == nullptr) {
        return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }
    if (input->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return finish(result, FICANT_KERNEL_STATUS_ABI_MISMATCH);
    }
    if (input->struct_size != sizeof(ficant_kernel_cgb_futures_delivery_input_v1)
        || input->reserved != 0 || !valid_product(input->product)
        || (input->frequency != FICANT_KERNEL_FREQUENCY_ANNUAL
            && input->frequency != FICANT_KERNEL_FREQUENCY_SEMIANNUAL)
        || !valid_delivery_month(input->delivery_month_first)
        || input->purchase_date >= input->delivery_date
        || input->delivery_date >= input->maturity_date
        || input->remaining_coupon_count == 0
        || input->months_to_next_coupon >= 12U / input->frequency) {
        return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }
    if (!all_finite(*input)) {
        return finish(result, FICANT_KERNEL_STATUS_NON_FINITE);
    }
    if (input->coupon_rate <= 0.0 || input->spot_clean_price <= 0.0
        || input->purchase_accrued_interest < 0.0
        || input->delivery_accrued_interest < 0.0 || input->interim_coupons < 0.0
        || input->futures_clean_price <= 0.0 || input->financing_rate < 0.0) {
        return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }
    try {
        if (!ficant::futures_math::is_cffex_deliverable(
                input->product, input->issue_date, input->maturity_date,
                input->delivery_month_first)) {
            result->eligible = 0;
            return finish(result, FICANT_KERNEL_STATUS_OK);
        }
        const double conversion_factor = ficant::futures_math::cffex_conversion_factor(
            input->coupon_rate, input->frequency, input->months_to_next_coupon,
            input->remaining_coupon_count);
        const double purchase_dirty =
            input->spot_clean_price + input->purchase_accrued_interest;
        const double actual_days = static_cast<double>(input->delivery_date - input->purchase_date);
        const double invoice =
            input->futures_clean_price * conversion_factor + input->delivery_accrued_interest;
        const double gross_basis =
            input->spot_clean_price - input->futures_clean_price * conversion_factor;
        const double financing_cost =
            purchase_dirty * input->financing_rate * actual_days / 365.0;
        const double holding_carry = input->delivery_accrued_interest
            - input->purchase_accrued_interest + input->interim_coupons - financing_cost;
        const double net_basis = gross_basis - holding_carry;
        const double irr = ((invoice + input->interim_coupons) / purchase_dirty - 1.0)
            * 365.0 / actual_days;
        if (!std::isfinite(conversion_factor) || !std::isfinite(invoice)
            || !std::isfinite(purchase_dirty) || !std::isfinite(gross_basis)
            || !std::isfinite(financing_cost) || !std::isfinite(holding_carry)
            || !std::isfinite(net_basis) || !std::isfinite(irr)
            || conversion_factor <= 0.0 || invoice <= 0.0 || purchase_dirty <= 0.0) {
            return finish(result, FICANT_KERNEL_STATUS_NON_FINITE);
        }
        result->eligible = 1;
        result->conversion_factor = conversion_factor;
        result->invoice_price = invoice;
        result->purchase_dirty_price = purchase_dirty;
        result->gross_basis = gross_basis;
        result->financing_cost = financing_cost;
        result->holding_carry = holding_carry;
        result->net_basis = net_basis;
        result->implied_repo_rate = irr;
        result->delivery_profit = -net_basis;
        return finish(result, FICANT_KERNEL_STATUS_OK);
    } catch (...) {
        return finish(result, FICANT_KERNEL_STATUS_INTERNAL_ERROR);
    }
}
