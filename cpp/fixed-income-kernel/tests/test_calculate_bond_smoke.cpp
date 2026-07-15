#include "ficant_kernel.h"

#include <cmath>
#include <cstdint>
#include <cstdio>

static int failures = 0;

#define CHECK(cond, msg) do {                         \
    if (!(cond)) {                                     \
        std::fprintf(stderr, "FAIL: %s\n", msg);       \
        ++failures;                                    \
    }                                                  \
} while (0)

int main() {
    /* ── 260013.IB: 2yr annual coupon, issue 2026-05-15, maturity 2028-05-15 ── */
    /* epoch days: 2026-05-15 = 20588, 2028-05-15 = 21319 */
    /* settlement 2026-07-14 (T+1 from 2026-07-13), epoch day = 20648 */
    ficant_kernel_bond_input_v1 bond{};
    bond.struct_size            = sizeof(bond);
    bond.abi_version            = FICANT_KERNEL_ABI_VERSION;
    bond.issue_date             = 20588;
    bond.maturity_date          = 21319;
    bond.coupon_rate            = 0.0130;
    bond.frequency              = FICANT_KERNEL_FREQUENCY_ANNUAL;
    bond.day_count_convention   = FICANT_KERNEL_DAY_COUNT_ACT_ACT_BOND_ISMA;
    bond.business_day_convention = FICANT_KERNEL_BDC_FOLLOWING;
    bond.face_value             = 100.0;

    ficant_kernel_calculate_input_v1 calc{};
    calc.struct_size              = sizeof(calc);
    calc.abi_version              = FICANT_KERNEL_ABI_VERSION;
    calc.settlement_date          = 20648;
    calc.input_mode               = FICANT_KERNEL_MODE_YIELD_IN;
    calc.input_value              = 0.0130;
    calc.calendar_requirement     = FICANT_KERNEL_CALENDAR_REQUIREMENT_REFERENCE_REPLAY;
    calc.calendar_coverage_start  = 0;
    calc.calendar_coverage_end    = 0;
    calc.non_business_days        = nullptr;
    calc.non_business_days_count  = 0;
    calc.work_weekends            = nullptr;
    calc.work_weekends_count      = 0;

    ficant_kernel_result_v1 result{};
    result.struct_size = sizeof(result);
    result.abi_version = FICANT_KERNEL_ABI_VERSION;

    /* First call: size the cashflow buffer.
     * Per ABI: returns BUFFER_TOO_SMALL when cf_count > 0, OK if zero. */
    uint32_t status = ficant_kernel_calculate_bond_v1(
        &bond, &calc, &result, nullptr, 0);

    CHECK(status == FICANT_KERNEL_STATUS_BUFFER_TOO_SMALL,
          "smoke sizing call: expected BUFFER_TOO_SMALL");

    uint32_t cf_count = result.cashflow_count;
    CHECK(cf_count == 2,
          "cashflow_count == 2 for 2yr annual bond");

    /* Second call: retrieve cashflows */
    ficant_kernel_cashflow_v1 cfs[16] = {};
    for (uint32_t i = 0; i < 16; ++i) {
        cfs[i].struct_size = sizeof(ficant_kernel_cashflow_v1);
        cfs[i].abi_version = FICANT_KERNEL_ABI_VERSION;
    }

    ficant_kernel_result_v1 result2{};
    result2.struct_size = sizeof(result2);
    result2.abi_version = FICANT_KERNEL_ABI_VERSION;
    status = ficant_kernel_calculate_bond_v1(
        &bond, &calc, &result2, cfs, cf_count);

    CHECK(status == FICANT_KERNEL_STATUS_OK,
          "smoke full call: expected OK");
    CHECK(std::isfinite(result2.dirty_price),
          "dirty_price is finite");
    CHECK(result2.dirty_price > 0.0,
          "dirty_price > 0");

    /* Regression: verify exact nominal cashflow schedule.
     * Expected: 2027-05-15 (20953) and 2028-05-15 (21319), distinct. */
    CHECK(result2.cashflow_count == 2,
          "result cashflow_count == 2");
    CHECK(cfs[0].nominal_date == 20953,
          "cf[0] nominal_date = 2027-05-15");
    CHECK(cfs[1].nominal_date == 21319,
          "cf[1] nominal_date = 2028-05-15");
    CHECK(cfs[0].nominal_date != cfs[1].nominal_date,
          "nominal dates are distinct (no duplicate maturity)");

    if (failures > 0) {
        std::fprintf(stderr, "%d failures\n", failures);
        return 1;
    }
    return 0;
}
