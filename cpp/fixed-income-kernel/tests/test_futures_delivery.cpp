#include "ficant_kernel.h"

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <limits>

namespace {
int failures = 0;
#define CHECK(condition, message) do { if (!(condition)) { std::fprintf(stderr, "FAIL: %s\n", message); ++failures; } } while (0)

int32_t date(int year, unsigned month, unsigned day) {
    year -= static_cast<int>(month <= 2U);
    const int era = (year >= 0 ? year : year - 399) / 400;
    const unsigned yoe = static_cast<unsigned>(year - era * 400);
    const unsigned doy = (153U * (month > 2U ? month - 3U : month + 9U) + 2U) / 5U + day - 1U;
    const unsigned doe = yoe * 365U + yoe / 4U - yoe / 100U + doy;
    return static_cast<int32_t>(era * 146097LL + doe - 719468LL);
}

ficant_kernel_cgb_futures_delivery_input_v1 input() {
    static const uint32_t delivery_months[] = {3U, 6U, 9U, 12U};
    return {
        sizeof(ficant_kernel_cgb_futures_delivery_input_v1), FICANT_KERNEL_ABI_VERSION,
        FICANT_KERNEL_FREQUENCY_SEMIANNUAL,
        120U, 78U, 0U, 1U,
        4U, delivery_months,
        date(2024, 8, 15), date(2034, 8, 15), date(2026, 9, 1),
        date(2026, 7, 21), date(2026, 9, 18),
        FICANT_KERNEL_DAY_COUNT_ACT_ACT_BOND_ISMA, 4U, 7U, 365U, 0U,
        0.03, 100.0, 0.025, 101.25, 99.50, 0.018
    };
}

ficant_kernel_cgb_futures_delivery_result_v1 result() {
    ficant_kernel_cgb_futures_delivery_result_v1 value{};
    value.struct_size = sizeof(value);
    value.abi_version = FICANT_KERNEL_ABI_VERSION;
    return value;
}
}

int main() {
    auto request = input();
    auto output = result();
    const uint32_t status = ficant_kernel_analyze_cgb_futures_delivery_v1(&request, &output);
    CHECK(status == FICANT_KERNEL_STATUS_OK, "T deliverable analysis succeeds");
    CHECK(output.eligible == 1U, "T candidate is deliverable");
    CHECK(output.conversion_factor > 0.9 && output.conversion_factor < 1.1, "CF plausible");
    CHECK(output.months_to_next_coupon == 5U, "x is derived from coupon schedule");
    CHECK(output.remaining_coupon_count == 16U, "n is derived from coupon schedule");
    CHECK(std::fabs(output.delivery_profit + output.net_basis) < 1e-12, "profit equals negative net basis");

    request.maturity_date = date(2032, 12, 31);
    output = result();
    CHECK(ficant_kernel_analyze_cgb_futures_delivery_v1(&request, &output) == FICANT_KERNEL_STATUS_OK,
          "ineligible candidate is a valid analytical outcome");
    CHECK(output.eligible == 0U, "short residual T candidate is ineligible");

    request = input();
    request.reserved = 1U;
    output = result();
    CHECK(ficant_kernel_analyze_cgb_futures_delivery_v1(&request, &output)
              == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
          "reserved drift fails closed");

    request = input();
    request.original_term_max_months = std::numeric_limits<uint32_t>::max();
    output = result();
    CHECK(ficant_kernel_analyze_cgb_futures_delivery_v1(&request, &output)
              == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
          "month rule values outside the C++ date range fail closed");
    return failures == 0 ? 0 : 1;
}
