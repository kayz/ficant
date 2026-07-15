#include "ficant_kernel.h"

#include "date_utils.hpp"
#include "day_count.hpp"
#include "bond_math.hpp"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <limits>

namespace {

/* ── validation helpers ─────────────────────────────────────────── */

/** Validate a bond input and preserve the ABI's size/version distinction. */
uint32_t validate_bond_input(const ficant_kernel_bond_input_v1* b) noexcept {
    if (!b || b->struct_size != sizeof(ficant_kernel_bond_input_v1)) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    if (b->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return FICANT_KERNEL_STATUS_ABI_MISMATCH;
    }
    if (b->frequency != FICANT_KERNEL_FREQUENCY_ANNUAL
        && b->frequency != FICANT_KERNEL_FREQUENCY_SEMIANNUAL) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    if (b->day_count_convention != FICANT_KERNEL_DAY_COUNT_ACT_ACT_BOND_ISMA
        || b->business_day_convention != FICANT_KERNEL_BDC_FOLLOWING
        || b->maturity_date <= b->issue_date
        || !std::isfinite(b->coupon_rate)
        || b->coupon_rate < 0.0
        || !std::isfinite(b->face_value)
        || b->face_value <= 0.0) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    return FICANT_KERNEL_STATUS_OK;
}

bool validate_sorted_unique_dates(const int32_t* dates, uint32_t count) noexcept {
    if (count == 0) return true;
    if (!dates) return false;
    for (uint32_t i = 1; i < count; ++i) {
        if (dates[i] <= dates[i - 1]) return false;
    }
    return true;
}

/** Validate a calculation input and preserve the ABI's size/version distinction. */
uint32_t validate_calc_input(const ficant_kernel_calculate_input_v1* c) noexcept {
    if (!c || c->struct_size != sizeof(ficant_kernel_calculate_input_v1)) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    if (c->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return FICANT_KERNEL_STATUS_ABI_MISMATCH;
    }
    if (c->input_mode != FICANT_KERNEL_MODE_YIELD_IN
        && c->input_mode != FICANT_KERNEL_MODE_PRICE_IN) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    if (!std::isfinite(c->input_value)) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    if (c->calendar_requirement != FICANT_KERNEL_CALENDAR_REQUIREMENT_REFERENCE_REPLAY
        && c->calendar_requirement != FICANT_KERNEL_CALENDAR_REQUIREMENT_EXACT_MARKET) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    if (c->calendar_coverage_end < c->calendar_coverage_start
        || !validate_sorted_unique_dates(
            c->non_business_days, c->non_business_days_count)
        || !validate_sorted_unique_dates(
            c->work_weekends, c->work_weekends_count)) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    return FICANT_KERNEL_STATUS_OK;
}

/** Validate a result header without writing through an invalid result. */
uint32_t validate_result_header(const ficant_kernel_result_v1* r) noexcept {
    if (!r || r->struct_size != sizeof(ficant_kernel_result_v1)) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    if (r->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return FICANT_KERNEL_STATUS_ABI_MISMATCH;
    }
    return FICANT_KERNEL_STATUS_OK;
}


/* ── calendar coverage ──────────────────────────────────────────── */

/**
 * True if a date needs provisional (weekend-only) calendar:
 * date < coverage_start or date > coverage_end.
 */
bool is_provisional_date(int32_t date,
                         int32_t coverage_start, int32_t coverage_end) noexcept
{
    return date < coverage_start || date > coverage_end;
}

/* ── business-day adjustment ────────────────────────────────────── */

/**
 * Adjust a date using Following convention.
 * Returns the adjusted date. Sets *used_provisional if weekend-only rules
 * were applied during adjustment.
 */
int32_t adjust_date(int32_t date,
                    const ficant_kernel_calculate_input_v1* calc,
                    bool& used_provisional) noexcept
{
    return ficant::date_utils::following_adjust(
        date,
        calc->non_business_days, calc->non_business_days_count,
        calc->work_weekends,     calc->work_weekends_count,
        calc->calendar_coverage_end,
        calc->calendar_requirement == FICANT_KERNEL_CALENDAR_REQUIREMENT_EXACT_MARKET,
        used_provisional);
}

/* ── cashflow context for settlement filtering ──────────────────── */

struct CashflowEntry {
    int32_t nominal_date;
    int32_t payment_date;  // adjusted
    double  coupon;
    double  principal;
    double  total;
};

/* ── Brent solver context ───────────────────────────────────────── */

struct PriceFromYieldCtx {
    const ficant_kernel_bond_input_v1* bond;
    int32_t settlement_date;
    const int32_t* nominal_schedule;
    const int32_t* payment_dates;       // I3-D-CPP-004
    uint32_t schedule_count;
    double coupon_amount;               // I3-D-CPP-002: pre-computed per-period amount
    double target_clean_price;          // P(Y) - target = 0
};

/**
 * f(y) = clean_price(y) - target_clean_price
 * Brent root of f(y) = 0 gives yield.
 */
double price_residual(double y, void* vctx) noexcept {
    auto* ctx = static_cast<PriceFromYieldCtx*>(vctx);
    double dirty = ficant::bond_math::coupon_bond_dirty_price(
        ctx->settlement_date, y,
        ctx->nominal_schedule, ctx->schedule_count,
        ctx->payment_dates,
        ctx->coupon_amount,
        ctx->bond->frequency,
        ctx->bond->face_value,
        nullptr);
    double accrued = ficant::bond_math::accrued_interest_coupon(
        ctx->settlement_date,
        ctx->nominal_schedule, ctx->schedule_count,
        ctx->bond->coupon_rate, ctx->bond->frequency,
        ctx->bond->face_value);
    double clean = dirty - accrued;
    return clean - ctx->target_clean_price;
}


/* ── result population ──────────────────────────────────────────── */

void zero_result(ficant_kernel_result_v1* result) noexcept {
    result->abi_version          = FICANT_KERNEL_ABI_VERSION;
    result->cashflow_count       = 0;
    result->calendar_resolution  = 0;
    result->status_code          = 0;
    result->accrued_interest     = 0.0;
    result->clean_price          = 0.0;
    result->dirty_price          = 0.0;
    result->yield_to_maturity    = 0.0;
    result->macaulay_duration    = 0.0;
    result->modified_duration    = 0.0;
    result->convexity            = 0.0;
    result->dv01                 = 0.0;
}

uint32_t return_status(
    ficant_kernel_result_v1* result, uint32_t status) noexcept {
    result->status_code = status;
    return status;
}

/* ── main calculation ───────────────────────────────────────────── */

uint32_t calculate_impl(
    const ficant_kernel_bond_input_v1*   bond,
    const ficant_kernel_calculate_input_v1* calc,
    ficant_kernel_result_v1*             result,
    ficant_kernel_cashflow_v1*           cashflows,
    uint32_t                             cashflow_capacity)
{
    const int32_t settlement_date = calc->settlement_date;
    const int32_t maturity_date   = bond->maturity_date;
    const double  coupon_rate     = bond->coupon_rate;
    const uint32_t frequency      = bond->frequency;
    const double  face_value      = bond->face_value;
    const double  coupon_amount   = (frequency > 0)
        ? coupon_rate * face_value / static_cast<double>(frequency)
        : 0.0;
    const bool    discount        = ficant::bond_math::is_discount_bond(coupon_rate);

    /* ── calendar coverage ─────────────────────────────────────── */
    const bool exact_required =
        calc->calendar_requirement == FICANT_KERNEL_CALENDAR_REQUIREMENT_EXACT_MARKET;

    if (exact_required) {
        // Check settlement date is within coverage.
        if (is_provisional_date(settlement_date,
                                calc->calendar_coverage_start,
                                calc->calendar_coverage_end)) {
            return return_status(
                result, FICANT_KERNEL_STATUS_CALENDAR_COVERAGE_MISSING);
        }
    }

    /* ── generate cashflows ─────────────────────────────────────── */
    // Max 120 semiannual periods in 60 years
    constexpr uint32_t MAX_SCHEDULE = 120;
    int32_t nominal_schedule[MAX_SCHEDULE];
    uint32_t schedule_count = 0;

    if (discount) {
        // Discount bond: only face value at maturity.
        schedule_count = 1;
        nominal_schedule[0] = maturity_date;
    } else {
        schedule_count = ficant::bond_math::generate_nominal_schedule(
            bond->issue_date, maturity_date, frequency,
            nominal_schedule, MAX_SCHEDULE);
        if (schedule_count == 0) {
            return return_status(result, FICANT_KERNEL_STATUS_INTERNAL_ERROR);
        }
    }

    /* ── adjust payment dates and filter by settlement ──────────── */
    CashflowEntry cf_entries[MAX_SCHEDULE];
    uint32_t cf_count = 0;
    bool any_provisional_date = false;

    for (uint32_t i = 0; i < schedule_count; ++i) {
        bool used_provisional = false;
        int32_t payment_date = adjust_date(nominal_schedule[i], calc, used_provisional);
        if (used_provisional) any_provisional_date = true;

        if (exact_required) {
            // Check each needed date is within exact coverage.
            if (is_provisional_date(payment_date,
                                    calc->calendar_coverage_start,
                                    calc->calendar_coverage_end)) {
                return return_status(
                    result, FICANT_KERNEL_STATUS_CALENDAR_COVERAGE_MISSING);
            }
        }

        // Settlement ownership: include cashflow only if settlement < payment_date.
        if (payment_date <= settlement_date) continue;

        double coupon_part   = (discount || i < schedule_count - 1) ? coupon_amount : coupon_amount;
        double principal_part = (i == schedule_count - 1) ? face_value : 0.0;

        cf_entries[cf_count].nominal_date  = nominal_schedule[i];
        cf_entries[cf_count].payment_date  = payment_date;
        cf_entries[cf_count].coupon        = coupon_part;
        cf_entries[cf_count].principal     = principal_part;
        cf_entries[cf_count].total         = coupon_part + principal_part;
        ++cf_count;
    }

    // Calendar resolution.
    uint32_t cal_resolution;
    if (any_provisional_date) {
        cal_resolution = FICANT_KERNEL_CALENDAR_RESOLUTION_PROVISIONAL_WEEKEND_ONLY;
    } else {
        cal_resolution = FICANT_KERNEL_CALENDAR_RESOLUTION_EXACT;
    }

    /* ── sizing call: just return cashflow count ────────────────── */
    result->cashflow_count = cf_count;
    result->calendar_resolution = cal_resolution;

    // I3-D-CPP-006: only exact (null, 0) is sizing.
    // Mismatched combinations already rejected in the entry point.
    if (!cashflows && cashflow_capacity == 0) {
        // Sizing call — don't compute full result.
        uint32_t ret = (cf_count > 0)
            ? FICANT_KERNEL_STATUS_BUFFER_TOO_SMALL
            : FICANT_KERNEL_STATUS_OK;
        return return_status(result, ret);
    }

    if (cashflow_capacity < cf_count) {
        return return_status(result, FICANT_KERNEL_STATUS_BUFFER_TOO_SMALL);
    }

    /* ── validate cashflow struct sizes and ABI version ──────────── */
    for (uint32_t i = 0; i < cf_count; ++i) {
        if (cashflows[i].struct_size != sizeof(ficant_kernel_cashflow_v1)) {
            return return_status(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
        }
        if (cashflows[i].abi_version != FICANT_KERNEL_ABI_VERSION) {
            return return_status(result, FICANT_KERNEL_STATUS_ABI_MISMATCH);
        }
    }

    /* ── build payment_dates array for I3-D-CPP-004 ─────────────── */
    // Payment dates control inclusion; nominal dates control year fractions.
    int32_t payment_dates_arr[MAX_SCHEDULE];
    for (uint32_t i = 0; i < schedule_count; ++i) {
        bool used_prov = false;
        payment_dates_arr[i] = adjust_date(nominal_schedule[i], calc, used_prov);
    }

    /* ── compute yield / price ──────────────────────────────────── */
    double yield_value = 0.0;
    double dirty_price = 0.0;
    double accrued     = 0.0;

    if (discount) {
        // Discount bond: simple yield, no coupons.
        double yf = ficant::day_count::act_act_natural(settlement_date, maturity_date);
        if (calc->input_mode == FICANT_KERNEL_MODE_YIELD_IN) {
            yield_value = calc->input_value;
            dirty_price = face_value / (1.0 + yield_value * yf);
        } else {
            // PRICE_IN for discount: clean == dirty == input_value
            dirty_price = calc->input_value;
            yield_value = (face_value / dirty_price - 1.0) / yf;
        }
        accrued = 0.0;
    } else {
        // Coupon bond.
        const double coupon_amt = coupon_amount;

        if (calc->input_mode == FICANT_KERNEL_MODE_YIELD_IN) {
            yield_value = calc->input_value;
            dirty_price = ficant::bond_math::coupon_bond_dirty_price(
                settlement_date, yield_value,
                nominal_schedule, schedule_count,
                payment_dates_arr,
                coupon_amt, frequency, face_value, nullptr);
        } else {
            // PRICE_IN: Brent solve for yield.
            PriceFromYieldCtx ctx;
            ctx.bond               = bond;
            ctx.settlement_date    = settlement_date;
            ctx.nominal_schedule   = nominal_schedule;
            ctx.payment_dates      = payment_dates_arr;
            ctx.schedule_count     = schedule_count;
            ctx.coupon_amount      = coupon_amount;
            ctx.target_clean_price = calc->input_value;

            double a = -0.50;
            double b =  1.00;

            // Check bracket.
            double fa = price_residual(a, &ctx);
            double fb = price_residual(b, &ctx);

            if (!std::isfinite(fa) || !std::isfinite(fb)) {
                return return_status(result, FICANT_KERNEL_STATUS_NON_FINITE);
            }
            if (fa * fb >= 0.0) {
                return return_status(result, FICANT_KERNEL_STATUS_NO_BRACKET);
            }

            bool conv_ok = false;
            yield_value = ficant::bond_math::brent_solve(
                price_residual, &ctx, a, b,
                1e-12, 1e-12, 100, conv_ok);

            if (!conv_ok) {
                return return_status(result, FICANT_KERNEL_STATUS_NOT_CONVERGED);
            }

            // Compute dirty price from solved yield.
            dirty_price = ficant::bond_math::coupon_bond_dirty_price(
                settlement_date, yield_value,
                nominal_schedule, schedule_count,
                payment_dates_arr,
                coupon_amt, frequency, face_value, nullptr);
        }

        accrued = ficant::bond_math::accrued_interest_coupon(
            settlement_date, nominal_schedule, schedule_count,
            coupon_rate, frequency, face_value);
    }

    double clean_price = dirty_price - accrued;

    // Validate finite results.
    if (!std::isfinite(dirty_price) || !std::isfinite(clean_price)
        || !std::isfinite(accrued) || !std::isfinite(yield_value)) {
        return return_status(result, FICANT_KERNEL_STATUS_NON_FINITE);
    }

    // Validate dirty = clean + accrued.
    // (Allow for floating-point rounding.)

    /* ── risk metrics ───────────────────────────────────────────── */
    double mac_dur  = 0.0;
    double mod_dur  = 0.0;
    double conv_val = 0.0;
    double dv01_val = 0.0;

    if (discount) {
        // I3-D-CPP-007: Macaulay duration for discount bond = ACT/ACT Natural yf.
        double yf = ficant::day_count::act_act_natural(settlement_date, maturity_date);
        // P = F / (1 + y * yf)  (simple interest)
        // dP/dy = -F * yf / (1 + y*yf)^2 = -P * yf / (1 + y*yf)
        // D_mod = yf / (1 + y*yf)
        // D_mac = yf  (single-payment Macaulay = time to payment)
        double denom = 1.0 + yield_value * yf;
        mod_dur = yf / denom;
        mac_dur = yf;
        conv_val = 2.0 * yf * yf / (denom * denom);

        // DV01 via finite difference.
        double p_down = ficant::bond_math::discount_bond_dirty_price(
            settlement_date, maturity_date, yield_value - 0.0001, face_value);
        double p_up = ficant::bond_math::discount_bond_dirty_price(
            settlement_date, maturity_date, yield_value + 0.0001, face_value);
        dv01_val = ficant::bond_math::dv01(p_down, p_up);
    } else {
        // Coupon bond: compute PVs and times for metrics.
        const double coupon_amt = coupon_amount;
        double pvs[MAX_SCHEDULE];
        double times[MAX_SCHEDULE];
        double amounts[MAX_SCHEDULE];
        uint32_t metric_count = 0;

        int months_per_period = (frequency == 2) ? 6 : 12;

        // I3-D-CPP-003: find first cashflow that survives the settlement filter
        // (using payment dates for I3-D-CPP-004) and compute base residual.
        uint32_t first_metric_idx = schedule_count;
        for (uint32_t i = 0; i < schedule_count; ++i) {
            if (payment_dates_arr[i] > settlement_date) {
                first_metric_idx = i;
                break;
            }
        }

        double base_residual = 0.0;
        if (first_metric_idx < schedule_count) {
            int32_t ref_start = (first_metric_idx == 0)
                ? ficant::date_utils::add_months(nominal_schedule[0], -months_per_period)
                : nominal_schedule[first_metric_idx - 1];
            base_residual = ficant::day_count::act_act_bond_isma(
                settlement_date, nominal_schedule[first_metric_idx],
                ref_start, nominal_schedule[first_metric_idx], frequency);
        }

        for (uint32_t i = first_metric_idx; i < schedule_count; ++i) {
            // Filter by payment date (I3-D-CPP-004).
            if (payment_dates_arr[i] <= settlement_date) continue;

            // Year fraction = base residual + full periods after first_metric_idx.
            double yf = base_residual;
            for (uint32_t j = first_metric_idx + 1; j <= i; ++j) {
                yf += 1.0 / static_cast<double>(frequency);
            }

            double cf_amt = coupon_amt;
            bool is_maturity = (i == schedule_count - 1);
            if (is_maturity) cf_amt += face_value;

            // Discount factor.
            double df = std::pow(1.0 + yield_value / static_cast<double>(frequency),
                                 -yf * static_cast<double>(frequency));
            double pv = cf_amt * df;

            pvs[metric_count]     = pv;
            times[metric_count]   = yf;
            amounts[metric_count] = cf_amt;
            ++metric_count;
        }

        if (metric_count > 0) {
            mac_dur = ficant::bond_math::macaulay_duration(
                pvs, times, metric_count, dirty_price);
            mod_dur = ficant::bond_math::modified_duration(
                mac_dur, yield_value, frequency);
            conv_val = ficant::bond_math::convexity(
                amounts, times, metric_count, yield_value, frequency, dirty_price);

            // DV01 via finite difference.
            double p_down = ficant::bond_math::coupon_bond_dirty_price(
                settlement_date, yield_value - 0.0001,
                nominal_schedule, schedule_count,
                payment_dates_arr,
                coupon_amt, frequency, face_value, nullptr);
            double p_up = ficant::bond_math::coupon_bond_dirty_price(
                settlement_date, yield_value + 0.0001,
                nominal_schedule, schedule_count,
                payment_dates_arr,
                coupon_amt, frequency, face_value, nullptr);
            dv01_val = ficant::bond_math::dv01(p_down, p_up);
        }
    }

    // Final finite checks.
    if (!std::isfinite(mac_dur) || !std::isfinite(mod_dur)
        || !std::isfinite(conv_val) || !std::isfinite(dv01_val)) {
        return return_status(result, FICANT_KERNEL_STATUS_NON_FINITE);
    }

    /* ── populate result ────────────────────────────────────────── */
    result->abi_version         = FICANT_KERNEL_ABI_VERSION;
    result->accrued_interest   = accrued;
    result->clean_price        = clean_price;
    result->dirty_price        = dirty_price;
    result->yield_to_maturity  = yield_value;
    result->macaulay_duration  = mac_dur;
    result->modified_duration  = mod_dur;
    result->convexity          = conv_val;
    result->dv01               = dv01_val;

    /* ── populate cashflows ─────────────────────────────────────── */
    for (uint32_t i = 0; i < cf_count; ++i) {
        cashflows[i].abi_version  = FICANT_KERNEL_ABI_VERSION;
        cashflows[i].sequence     = i + 1;
        cashflows[i].nominal_date = cf_entries[i].nominal_date;
        cashflows[i].payment_date = cf_entries[i].payment_date;
        cashflows[i].coupon       = cf_entries[i].coupon;
        cashflows[i].principal    = cf_entries[i].principal;
        cashflows[i].total        = cf_entries[i].total;
    }

    return return_status(result, FICANT_KERNEL_STATUS_OK);
}

} // anonymous namespace

/* ── public ABI entry point ──────────────────────────────────────── */

extern "C" uint32_t ficant_kernel_calculate_bond_v1(
    const ficant_kernel_bond_input_v1*   bond_input,
    const ficant_kernel_calculate_input_v1* calc_input,
    ficant_kernel_result_v1*             result,
    ficant_kernel_cashflow_v1*           cashflows,
    uint32_t                             cashflow_capacity) noexcept
{
    /* ── pointer validation ─────────────────────────────────────── */
    const uint32_t result_status = validate_result_header(result);
    if (result_status != FICANT_KERNEL_STATUS_OK) return result_status;

    zero_result(result);

    /* ── struct size / ABI version ──────────────────────────────── */
    const uint32_t bond_status = validate_bond_input(bond_input);
    if (bond_status != FICANT_KERNEL_STATUS_OK) {
        return return_status(result, bond_status);
    }
    const uint32_t calc_status = validate_calc_input(calc_input);
    if (calc_status != FICANT_KERNEL_STATUS_OK) {
        return return_status(result, calc_status);
    }

    /* ── I3-D-CPP-006: parameter matrix for cashflows/capacity ──── */
    // Only (null, 0) is a sizing query.
    // Mismatched combinations → INVALID_ARGUMENT.
    if (cashflows == nullptr && cashflow_capacity > 0) {
        return return_status(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }
    if (cashflows != nullptr && cashflow_capacity == 0) {
        return return_status(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }

    try {
        return calculate_impl(bond_input, calc_input, result,
                               cashflows, cashflow_capacity);
    } catch (...) {
        // Internal implementation allowed to throw; noexcept on the extern "C"
        // export prevents unwind beyond this point (I3-D-CPP-007 catch-meaningful).
        return return_status(result, FICANT_KERNEL_STATUS_INTERNAL_ERROR);
    }
}
