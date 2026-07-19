#include "ficant_kernel.h"

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <limits>

static int failures = 0;

#define CHECK(cond, msg) do { if (!(cond)) { std::fprintf(stderr, "FAIL: %s\n", msg); ++failures; } } while (0)

ficant_kernel_cgb_futures_hedge_input_v1 input() {
    return {
        sizeof(ficant_kernel_cgb_futures_hedge_input_v1), FICANT_KERNEL_ABI_VERSION,
        FICANT_KERNEL_CGB_FUTURES_T, 0U, 500.0, 0.045, 0.9
    };
}

ficant_kernel_cgb_futures_hedge_result_v1 result() {
    ficant_kernel_cgb_futures_hedge_result_v1 value{};
    value.struct_size = sizeof(value);
    value.abi_version = FICANT_KERNEL_ABI_VERSION;
    return value;
}

int main() {
    auto request = input();
    auto output = result();
    CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(&request, &output)
              == FICANT_KERNEL_STATUS_OK,
          "valid hedge succeeds");
    CHECK(std::fabs(output.futures_contract_dv01 - 500.0) < 1e-12,
          "contract DV01 follows CTD/CF identity");
    CHECK(std::fabs(output.raw_contracts + 1.0) < 1e-12,
          "long cash risk requires one short futures contract");
    CHECK(output.recommended_contracts == -1, "recommended contract is short one");
    CHECK(std::fabs(output.residual_dv01) < 1e-12, "exact hedge has zero residual");
    CHECK(std::fabs(output.hedge_effectiveness - 1.0) < 1e-12,
          "exact hedge is fully effective");

    request.target_dv01 = -250.0;
    output = result();
    CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(&request, &output)
              == FICANT_KERNEL_STATUS_OK,
          "short cash risk succeeds");
    CHECK(output.recommended_contracts == 0,
          "half-contract tie chooses lower absolute position");
    CHECK(std::fabs(output.hedge_effectiveness) < 1e-12,
          "zero-hand tie leaves original risk");

    request.target_dv01 = std::numeric_limits<double>::infinity();
    output = result();
    CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(&request, &output)
              == FICANT_KERNEL_STATUS_NON_FINITE,
          "non-finite input fails closed");

    request = input();
    request.reserved = 1U;
    output = result();
    CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(&request, &output)
              == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
          "reserved drift fails closed");

    request = input();
    request.struct_size -= 1U;
    output = result();
    CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(&request, &output)
              == FICANT_KERNEL_STATUS_ABI_MISMATCH,
          "input layout drift fails closed");

    return failures == 0 ? 0 : 1;
}
