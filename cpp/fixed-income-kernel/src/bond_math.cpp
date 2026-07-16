#include "bond_math.hpp"
#include "date_utils.hpp"
#include "day_count.hpp"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <limits>

namespace ficant {
namespace bond_math {

/* ── internal helpers ──────────────────────────────────────────── */

namespace {

/** True if `value` is a finite IEEE-754 double. */
bool is_finite(double v) noexcept {
    return std::isfinite(v);
}

/** Safely compute discount factor: (1 + y/f)^(-t*f) */
double discount_factor(double yield, uint32_t frequency, double year_frac) noexcept {
    double base = 1.0 + yield / static_cast<double>(frequency);
    double exponent = -year_frac * static_cast<double>(frequency);
    return std::pow(base, exponent);
}

} // anonymous namespace

/* ── public API ────────────────────────────────────────────────── */

bool is_discount_bond(double coupon_rate) noexcept {
    return coupon_rate == 0.0;
}

uint32_t generate_nominal_schedule(int32_t issue_date, int32_t maturity_date,
                                   uint32_t frequency,
                                   int32_t* out_dates, uint32_t capacity) noexcept
{
    if (capacity == 0 || !out_dates) return 0;
    if (maturity_date <= issue_date) return 0;

    int months_per_period = (frequency == 2) ? 6 : 12;

    // Count how many periods. Start from issue_date, step by months_per_period
    // months until we reach/pass maturity_date.
    uint32_t count = 0;
    int32_t d = maturity_date;
    while (d > issue_date && count < capacity) {
        d = date_utils::add_months(maturity_date,
                                   -static_cast<int>(count + 1) * months_per_period);
        ++count;
    }

    // Generate forward from issue_date.
    uint32_t idx = 0;
    int32_t current = issue_date;
    for (uint32_t i = 0; i < count && idx < capacity; ++i) {
        current = date_utils::add_months(issue_date,
                                         static_cast<int>(i + 1) * months_per_period);
        out_dates[idx++] = current;
    }

    // Ensure the last date is maturity_date.
    if (idx > 0 && out_dates[idx - 1] != maturity_date) {
        out_dates[idx - 1] = maturity_date;
    }

    return idx;
}

double year_frac_to_cashflow(int32_t settlement_date, int32_t nominal_date,
                             const int32_t* schedule, uint32_t schedule_count,
                             uint32_t frequency) noexcept
{
    if (schedule_count == 0) return 0.0;

    // Find the index of nominal_date in schedule, and the previous one.
    // schedule is sorted ascending.
    uint32_t idx = 0;
    for (uint32_t i = 0; i < schedule_count; ++i) {
        if (schedule[i] == nominal_date) {
            idx = i;
            break;
        }
    }

    int32_t period_start;
    if (idx == 0) {
        // First coupon: period from settlement to first nominal date.
        // The reference period is from issue_date (implicitly, the start of
        // this coupon period) to schedule[0].
        // But we don't have issue_date separately here — the previous nominal
        // "virtual" date would be issue_date. We compute the reference period
        // length as: schedule[idx] - (schedule[idx-1] or issue_date).
        // Since we don't have issue_date, use the full period from schedule[0]
        // to a date that's one period back.
        int months_back = (frequency == 2) ? 6 : 12;
        period_start = date_utils::add_months(schedule[0], -months_back);
    } else {
        period_start = schedule[idx - 1];
    }
    int32_t period_end = nominal_date;

    // If settlement_date is after period_start, the accrual starts at settlement.
    if (settlement_date > period_start) {
        return day_count::act_act_bond_isma(settlement_date, nominal_date,
                                            period_start, period_end, frequency);
    } else {
        return day_count::act_act_bond_isma(period_start, nominal_date,
                                            period_start, period_end, frequency);
    }
}

double coupon_bond_dirty_price(int32_t settlement_date, double yield,
                               const int32_t* nominal_dates, uint32_t cf_count,
                               const int32_t* payment_dates,
                               double coupon, uint32_t frequency,
                               double face_value,
                               double* out_pvs) noexcept
{
    double sum = 0.0;
    int months_per_period = (frequency == 2) ? 6 : 12;

    // Determine the first cashflow index that survives the settlement filter.
    // Use payment_dates when provided (I3-D-CPP-004), else nominal_dates.
    uint32_t first_idx = cf_count;
    for (uint32_t i = 0; i < cf_count; ++i) {
        int32_t filter_date = (payment_dates != nullptr)
            ? payment_dates[i] : nominal_dates[i];
        if (filter_date > settlement_date) {
            first_idx = i;
            break;
        }
    }
    if (first_idx == cf_count) return 0.0;  // no future cashflows

    // Compute the base residual: year fraction from settlement to the
    // first future nominal cashflow date.  This is the "stub" period.
    int32_t first_nom = nominal_dates[first_idx];
    int32_t ref_start;
    if (first_idx == 0) {
        ref_start = date_utils::add_months(first_nom, -months_per_period);
    } else {
        ref_start = nominal_dates[first_idx - 1];
    }

    double base_residual = day_count::act_act_bond_isma(
        settlement_date, first_nom, ref_start, first_nom, frequency);

    for (uint32_t i = first_idx; i < cf_count; ++i) {
        // Filter by payment_date if available, else nominal_date.
        int32_t filter_date = (payment_dates != nullptr)
            ? payment_dates[i] : nominal_dates[i];
        if (filter_date <= settlement_date) continue;

        // Year fraction = base residual + full periods between first_idx and i.
        double yf = base_residual;
        for (uint32_t j = first_idx + 1; j <= i; ++j) {
            yf += 1.0 / static_cast<double>(frequency);
        }

        double df = discount_factor(yield, frequency, yf);
        double cf_amount = coupon;
        bool is_maturity = (i == cf_count - 1);
        if (is_maturity) {
            cf_amount += face_value;
        }

        double pv = cf_amount * df;
        if (out_pvs) out_pvs[i] = pv;
        sum += pv;
    }

    return sum;
}

double accrued_interest_coupon(int32_t settlement_date,
                               const int32_t* nominal_dates, uint32_t cf_count,
                               double coupon_rate, uint32_t frequency,
                               double face_value) noexcept
{
    if (cf_count == 0) return 0.0;

    // Find the coupon period containing settlement_date.
    // The last nominal date <= settlement_date is the previous coupon date.
    // The first nominal date > settlement_date is the next coupon date.
    int32_t last_coupon = 0;
    int32_t next_coupon = 0;
    bool found = false;

    int months_per_period = (frequency == 2) ? 6 : 12;

    // Compute the first nominal date (one period before schedule[0]).
    int32_t prev_nominal = date_utils::add_months(nominal_dates[0], -months_per_period);

    for (uint32_t i = 0; i < cf_count; ++i) {
        if (nominal_dates[i] > settlement_date) {
            next_coupon = nominal_dates[i];
            last_coupon = (i == 0) ? prev_nominal : nominal_dates[i - 1];
            found = true;
            break;
        }
    }

    if (!found) {
        // Settlement is after all cashflow dates — no accrual.
        // Actually, if settlement == maturity, no accrued.
        return 0.0;
    }

    if (settlement_date <= last_coupon) return 0.0;

    // Year fraction for accrued period.
    double yf = day_count::act_act_bond_isma(last_coupon, settlement_date,
                                             last_coupon, next_coupon, frequency);
    return coupon_rate * face_value * yf;
}

double discount_bond_dirty_price(int32_t settlement_date, int32_t maturity_date,
                                 double yield, double face_value) noexcept
{
    double yf = day_count::act_act_natural(settlement_date, maturity_date);
    return face_value / (1.0 + yield * yf);
}

double discount_bond_yield(int32_t settlement_date, int32_t maturity_date,
                           double dirty_price, double face_value) noexcept
{
    double yf = day_count::act_act_natural(settlement_date, maturity_date);
    return (face_value / dirty_price - 1.0) / yf;
}

double brent_solve(double (*f)(double, void*), void* ctx,
                   double a, double b,
                   double xtol, double ytol,
                   uint32_t max_iter, bool& ok) noexcept
{
    ok = false;

    double fa = f(a, ctx);
    double fb = f(b, ctx);

    if (!is_finite(fa) || !is_finite(fb)) return 0.0;
    if (fa * fb >= 0.0) return 0.0;  // no bracket

    // Ensure |fa| >= |fb|.
    if (std::fabs(fa) < std::fabs(fb)) {
        std::swap(a, b);
        std::swap(fa, fb);
    }

    double c = a;
    double fc = fa;
    double d = b - a;
    double e = d;
    bool mflag = true;

    for (uint32_t iter = 0; iter < max_iter; ++iter) {
        if (fb == 0.0 || std::fabs(b - a) <= xtol || std::fabs(fb) <= ytol) {
            ok = true;
            return b;
        }

        double s;
        if (std::fabs(e) >= xtol && std::fabs(fc) > std::fabs(fb)) {
            // Inverse quadratic interpolation.
            s = a * fb * fc / ((fa - fb) * (fa - fc))
              + b * fa * fc / ((fb - fa) * (fb - fc))
              + c * fa * fb / ((fc - fa) * (fc - fb));
        } else {
            // Secant.
            s = b - fb * (b - a) / (fb - fa);
        }

        // Bisection checks.
        double mid = (a + b) * 0.5;
        bool do_bisect = false;
        if ((s < (3.0 * a + b) * 0.25 || s > b) ||
            (mflag && std::fabs(s - b) >= 0.5 * std::fabs(b - c)) ||
            (!mflag && std::fabs(s - b) >= 0.5 * std::fabs(c - d)) ||
            (mflag && std::fabs(b - c) < xtol) ||
            (!mflag && std::fabs(c - d) < xtol))
        {
            s = mid;
            do_bisect = true;
        }
        mflag = do_bisect;

        double fs = f(s, ctx);
        if (!is_finite(fs)) return 0.0;

        d = c;
        c = b;
        fc = fb;

        if (fa * fs < 0.0) {
            b = s;
            fb = fs;
        } else {
            a = s;
            fa = fs;
        }

        if (std::fabs(fa) < std::fabs(fb)) {
            std::swap(a, b);
            std::swap(fa, fb);
        }
    }

    // Check final condition.
    if (std::fabs(b - a) <= xtol || std::fabs(fb) <= ytol) {
        ok = true;
        return b;
    }
    return b;  // not converged
}

double macaulay_duration(const double* pvs, const double* times,
                         uint32_t count, double dirty_price) noexcept
{
    if (dirty_price == 0.0) return 0.0;
    double sum = 0.0;
    for (uint32_t i = 0; i < count; ++i) {
        sum += times[i] * pvs[i];
    }
    return sum / dirty_price;
}

double modified_duration(double mac_dur, double yield, uint32_t frequency) noexcept {
    return mac_dur / (1.0 + yield / static_cast<double>(frequency));
}

double convexity(const double* cashflows, const double* times,
                 uint32_t count, double yield, uint32_t frequency,
                 double dirty_price) noexcept
{
    if (dirty_price == 0.0) return 0.0;
    double inv_freq = 1.0 / static_cast<double>(frequency);
    double y_over_f = yield * inv_freq;
    double sum = 0.0;
    for (uint32_t i = 0; i < count; ++i) {
        double t = times[i];
        double df = std::pow(1.0 + y_over_f, -(t * static_cast<double>(frequency) + 2.0));
        sum += cashflows[i] * t * (t + inv_freq) * df;
    }
    return sum / dirty_price;
}

double dv01(double p_down, double p_up) noexcept {
    // DV01 = (P(y-1bp) - P(y+1bp)) / 2, should be positive.
    return (p_down - p_up) * 0.5;
}

} // namespace bond_math
} // namespace ficant
