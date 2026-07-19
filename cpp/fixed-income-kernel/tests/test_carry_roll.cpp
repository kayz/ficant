#include "ficant_kernel.h"

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <limits>

namespace {

int failures = 0;

#define CHECK(condition, message) do {                \
    if (!(condition)) {                               \
        std::fprintf(stderr, "FAIL: %s\n", message); \
        ++failures;                                   \
    }                                                 \
} while (0)

ficant_kernel_carry_roll_result_v1 result() {
    return {
        sizeof(ficant_kernel_carry_roll_result_v1),
        FICANT_KERNEL_ABI_VERSION,
        FICANT_KERNEL_STATUS_INTERNAL_ERROR,
        0,
        0.0,
        0.0,
        0.0,
    };
}

} // namespace

int main() {
    ficant_kernel_carry_roll_input_v1 input{
        sizeof(ficant_kernel_carry_roll_input_v1),
        FICANT_KERNEL_ABI_VERSION,
        100.0,
        100.4,
        101.1,
        1.2,
    };
    auto output = result();
    CHECK(ficant_kernel_decompose_carry_roll_v1(&input, &output)
              == FICANT_KERNEL_STATUS_OK,
          "valid decomposition succeeds");
    CHECK(std::abs(output.carry - 1.6) < 1.0e-12,
          "carry includes paid cashflows and unchanged-yield pull");
    CHECK(std::abs(output.roll_down - 0.7) < 1.0e-12,
          "roll-down is the horizon curve repricing difference");
    CHECK(std::abs(output.total_return - 2.3) < 1.0e-12,
          "total return equals carry plus roll-down");

    input.horizon_dirty_at_rolled_yield = 99.9;
    output = result();
    CHECK(ficant_kernel_decompose_carry_roll_v1(&input, &output)
              == FICANT_KERNEL_STATUS_OK,
          "negative roll-down remains a valid result");
    CHECK(output.roll_down < 0.0, "roll-down can be negative");

    input.initial_dirty_price = std::numeric_limits<double>::quiet_NaN();
    output = result();
    CHECK(ficant_kernel_decompose_carry_roll_v1(&input, &output)
              == FICANT_KERNEL_STATUS_NON_FINITE,
          "non-finite price fails with stable status");

    input.initial_dirty_price = 0.0;
    output = result();
    CHECK(ficant_kernel_decompose_carry_roll_v1(&input, &output)
              == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
          "non-positive dirty price is rejected");

    return failures == 0 ? 0 : 1;
}
