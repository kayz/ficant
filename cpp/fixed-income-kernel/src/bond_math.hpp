#ifndef FICANT_KERNEL_BOND_MATH_HPP
#define FICANT_KERNEL_BOND_MATH_HPP

#include <cstdint>

namespace ficant {
namespace bond_math {

/** Result of a bond pricing computation. */
struct BondResult {
    double dirty_price    = 0.0;
    double clean_price    = 0.0;
    double accrued_interest = 0.0;
    double yield_to_maturity = 0.0;
    double macaulay_duration = 0.0;
    double modified_duration = 0.0;
    double convexity      = 0.0;
    double dv01           = 0.0;
};

/**
 * True if the bond is a discount (zero-coupon) bond.
 * A bond with coupon_rate == 0.0 is treated as discount.
 */
bool is_discount_bond(double coupon_rate) noexcept;

/**
 * Generate nominal cashflow schedule dates from issue_date to maturity_date.
 *
 * Dates are stored in `out_dates` (max `capacity` entries).
 * Returns the number of dates written (including the maturity date).
 *
 * For annual frequency: dates at yearly intervals.
 * For semiannual frequency: dates at 6-month intervals.
 * The final date is always maturity_date.
 */
uint32_t generate_nominal_schedule(int32_t issue_date, int32_t maturity_date,
                                   uint32_t frequency,
                                   int32_t* out_dates, uint32_t capacity) noexcept;

/**
 * Compute the year fraction from settlement to a nominal cashflow date,
 * given the full nominal coupon schedule.
 *
 * The `period_start` for a given cashflow is either the settlement date
 * (for the first future cashflow) or the previous nominal date.
 * The `period_end` is the current nominal date (or the next nominal date
 * when computing the reference period for the partial first period).
 *
 * This function computes the appropriate year fraction based on the
 * Act/Act ISMA convention.
 */
double year_frac_to_cashflow(int32_t settlement_date, int32_t nominal_date,
                             const int32_t* schedule, uint32_t schedule_count,
                             uint32_t frequency) noexcept;

/**
 * Calculate the present value of all future cashflows for a coupon bond
 * given a yield.
 *
 * Returns dirty price.
 * `out_pvs` (if non-null, length `cf_count`) receives individual PVs.
 * `payment_dates` (if non-null, length `cf_count`) is used for the
 * settlement filter instead of nominal_dates, so that business-day-adjusted
 * payment dates control inclusion (I3-D-CPP-004).  When nullptr the
 * filter falls back to nominal_dates.
 */
double coupon_bond_dirty_price(int32_t settlement_date, double yield,
                               const int32_t* nominal_dates, uint32_t cf_count,
                               const int32_t* payment_dates,
                               double coupon, uint32_t frequency,
                               double face_value,
                               double* out_pvs) noexcept;

/**
 * Calculate accrued interest for a coupon bond.
 *
 * year_frac = days(settlement - last_coupon) / (days_in_period * freq)
 */
double accrued_interest_coupon(int32_t settlement_date,
                               const int32_t* nominal_dates, uint32_t cf_count,
                               double coupon_rate, uint32_t frequency,
                               double face_value) noexcept;

/**
 * Discount bond pricing: dirty_price = face_value / (1 + yield * year_frac).
 * Simple yield, Actual/Actual natural year fraction.
 */
double discount_bond_dirty_price(int32_t settlement_date, int32_t maturity_date,
                                 double yield, double face_value) noexcept;

/**
 * Discount bond yield from price: yield = (face_value/price - 1) / year_frac.
 */
double discount_bond_yield(int32_t settlement_date, int32_t maturity_date,
                           double dirty_price, double face_value) noexcept;

/**
 * Brent solver for f(x) = 0.
 *
 * @param f     objective function f(x).  f(x_target) == 0 at solution.
 * @param a,b   initial bracket.  Must satisfy f(a)*f(b) < 0.
 * @param xtol  convergence tolerance on |b-a|.
 * @param ytol  convergence tolerance on |f(b)|.
 * @param max_iter  maximum iterations.
 * @param ok    set to true if converged, false otherwise.
 * @return      estimated root.
 */
double brent_solve(double (*f)(double, void*), void* ctx,
                   double a, double b,
                   double xtol, double ytol,
                   uint32_t max_iter, bool& ok) noexcept;

/**
 * Calculate Macaulay duration.
 * D_mac = sum(t_i * PV_i) / sum(PV_i)
 */
double macaulay_duration(const double* pvs, const double* times,
                         uint32_t count, double dirty_price) noexcept;

/**
 * Calculate modified duration.
 * D_mod = D_mac / (1 + y/freq)
 */
double modified_duration(double mac_dur, double yield, uint32_t frequency) noexcept;

/**
 * Calculate convexity.
 * C = (1/P) * sum(CF_i * t_i * (t_i + 1/freq) / (1 + y/freq)^(t_i * freq + 2))
 *
 * @param cashflows  future cashflow amounts
 * @param times      year fractions from settlement to each cashflow
 * @param count      number of cashflows
 * @param yield      yield to maturity (decimal)
 * @param frequency  compounding frequency
 * @param dirty_price  dirty price
 * @return convexity
 */
double convexity(const double* cashflows, const double* times,
                 uint32_t count, double yield, uint32_t frequency,
                 double dirty_price) noexcept;

/**
 * DV01 via central finite difference: (P(y-1bp) - P(y+1bp)) / 2.
 * Always returns a positive number.
 */
double dv01(double p_down, double p_up) noexcept;

} // namespace bond_math
} // namespace ficant

#endif
