#include "ficant_kernel.h"

#include <cstddef>
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
    /* ── ABI version ───────────────────────────────────────────── */
    CHECK(FICANT_KERNEL_ABI_VERSION == UINT32_C(1),
          "ABI_VERSION == 1");

    /* ── Status codes ──────────────────────────────────────────── */
    CHECK(FICANT_KERNEL_STATUS_OK == UINT32_C(0),
          "STATUS_OK == 0");
    CHECK(FICANT_KERNEL_STATUS_INVALID_ARGUMENT == UINT32_C(1),
          "STATUS_INVALID_ARGUMENT == 1");
    CHECK(FICANT_KERNEL_STATUS_ABI_MISMATCH == UINT32_C(2),
          "STATUS_ABI_MISMATCH == 2");
    CHECK(FICANT_KERNEL_STATUS_BUFFER_TOO_SMALL == UINT32_C(3),
          "STATUS_BUFFER_TOO_SMALL == 3");
    CHECK(FICANT_KERNEL_STATUS_NO_BRACKET == UINT32_C(4),
          "STATUS_NO_BRACKET == 4");
    CHECK(FICANT_KERNEL_STATUS_NOT_CONVERGED == UINT32_C(5),
          "STATUS_NOT_CONVERGED == 5");
    CHECK(FICANT_KERNEL_STATUS_NON_FINITE == UINT32_C(6),
          "STATUS_NON_FINITE == 6");
    CHECK(FICANT_KERNEL_STATUS_CALENDAR_COVERAGE_MISSING == UINT32_C(7),
          "STATUS_CALENDAR_COVERAGE_MISSING == 7");
    CHECK(FICANT_KERNEL_STATUS_INTERNAL_ERROR == UINT32_C(255),
          "STATUS_INTERNAL_ERROR == 255");

    /* ── Frequency constants ───────────────────────────────────── */
    CHECK(FICANT_KERNEL_FREQUENCY_ANNUAL == UINT32_C(1),
          "FREQUENCY_ANNUAL == 1");
    CHECK(FICANT_KERNEL_FREQUENCY_SEMIANNUAL == UINT32_C(2),
          "FREQUENCY_SEMIANNUAL == 2");

    /* ── Day count ─────────────────────────────────────────────── */
    CHECK(FICANT_KERNEL_DAY_COUNT_ACT_ACT_BOND_ISMA == UINT32_C(1),
          "DAY_COUNT_ACT_ACT_BOND_ISMA == 1");

    /* ── Business-day convention ───────────────────────────────── */
    CHECK(FICANT_KERNEL_BDC_FOLLOWING == UINT32_C(1),
          "BDC_FOLLOWING == 1");

    /* ── Input mode ────────────────────────────────────────────── */
    CHECK(FICANT_KERNEL_MODE_YIELD_IN == UINT32_C(1),
          "MODE_YIELD_IN == 1");
    CHECK(FICANT_KERNEL_MODE_PRICE_IN == UINT32_C(2),
          "MODE_PRICE_IN == 2");

    /* ── Calendar requirement ──────────────────────────────────── */
    CHECK(FICANT_KERNEL_CALENDAR_REQUIREMENT_REFERENCE_REPLAY == UINT32_C(1),
          "CALENDAR_REQUIREMENT_REFERENCE_REPLAY == 1");
    CHECK(FICANT_KERNEL_CALENDAR_REQUIREMENT_EXACT_MARKET == UINT32_C(2),
          "CALENDAR_REQUIREMENT_EXACT_MARKET == 2");

    /* ── Calendar resolution ───────────────────────────────────── */
    CHECK(FICANT_KERNEL_CALENDAR_RESOLUTION_EXACT == UINT32_C(1),
          "CALENDAR_RESOLUTION_EXACT == 1");
    CHECK(FICANT_KERNEL_CALENDAR_RESOLUTION_PROVISIONAL_WEEKEND_ONLY == UINT32_C(2),
          "CALENDAR_RESOLUTION_PROVISIONAL_WEEKEND_ONLY == 2");
    CHECK(FICANT_KERNEL_CURVE_INTERPOLATION_LINEAR_YIELD == UINT32_C(1),
          "CURVE_INTERPOLATION_LINEAR_YIELD == 1");
    CHECK(FICANT_KERNEL_CGB_FUTURES_TS == UINT32_C(1), "CGB_FUTURES_TS == 1");
    CHECK(FICANT_KERNEL_CGB_FUTURES_TL == UINT32_C(4), "CGB_FUTURES_TL == 4");

    /* ── Struct sizes ──────────────────────────────────────────── */
    CHECK(sizeof(ficant_kernel_bond_input_v1) == 48,
          "sizeof(bond_input_v1) == 48");
    CHECK(sizeof(ficant_kernel_calculate_input_v1) == 72,
          "sizeof(calculate_input_v1) == 72");
    CHECK(sizeof(ficant_kernel_result_v1) == 88,
          "sizeof(result_v1) == 88");
    CHECK(sizeof(ficant_kernel_cashflow_v1) == 48,
          "sizeof(cashflow_v1) == 48");
    CHECK(sizeof(ficant_kernel_yield_curve_node_v1) == 24,
          "sizeof(yield_curve_node_v1) == 24");
    CHECK(sizeof(ficant_kernel_yield_curve_input_v1) == 32,
          "sizeof(yield_curve_input_v1) == 32");
    CHECK(sizeof(ficant_kernel_yield_curve_query_v1) == 16,
          "sizeof(yield_curve_query_v1) == 16");
    CHECK(sizeof(ficant_kernel_yield_curve_result_v1) == 24,
          "sizeof(yield_curve_result_v1) == 24");
    CHECK(sizeof(ficant_kernel_carry_roll_input_v1) == 40,
          "sizeof(carry_roll_input_v1) == 40");
    CHECK(sizeof(ficant_kernel_carry_roll_result_v1) == 40,
          "sizeof(carry_roll_result_v1) == 40");
    CHECK(sizeof(ficant_kernel_cgb_futures_delivery_input_v1) == 72,
          "sizeof(cgb_futures_delivery_input_v1) == 72");
    CHECK(sizeof(ficant_kernel_cgb_futures_delivery_result_v1) == 120,
          "sizeof(cgb_futures_delivery_result_v1) == 120");

    /* ── Key offsets (ABI stability) ───────────────────────────── */
    CHECK(offsetof(ficant_kernel_bond_input_v1, struct_size) == 0,
          "bond_input.struct_size offset 0");
    CHECK(offsetof(ficant_kernel_bond_input_v1, coupon_rate) == 32,
          "bond_input.coupon_rate offset 32");
    CHECK(offsetof(ficant_kernel_bond_input_v1, face_value) == 40,
          "bond_input.face_value offset 40");

    CHECK(offsetof(ficant_kernel_result_v1, struct_size) == 0,
          "result.struct_size offset 0");
    CHECK(offsetof(ficant_kernel_result_v1, abi_version) == 4,
          "result.abi_version offset 4");
    CHECK(offsetof(ficant_kernel_result_v1, status_code) == 16,
          "result.status_code offset 16");
    CHECK(offsetof(ficant_kernel_result_v1, accrued_interest) == 24,
          "result.accrued_interest offset 24");

    CHECK(offsetof(ficant_kernel_cashflow_v1, struct_size) == 0,
          "cashflow.struct_size offset 0");
    CHECK(offsetof(ficant_kernel_cashflow_v1, abi_version) == 4,
          "cashflow.abi_version offset 4");
    CHECK(offsetof(ficant_kernel_cashflow_v1, sequence) == 8,
          "cashflow.sequence offset 8");
    CHECK(offsetof(ficant_kernel_cashflow_v1, coupon) == 24,
          "cashflow.coupon offset 24");
    CHECK(offsetof(ficant_kernel_yield_curve_node_v1, yield_to_maturity) == 16,
          "yield_curve_node.yield_to_maturity offset 16");
    CHECK(offsetof(ficant_kernel_yield_curve_input_v1, nodes) == 16,
          "yield_curve_input.nodes offset 16");
    CHECK(offsetof(ficant_kernel_yield_curve_result_v1, yield_to_maturity) == 16,
          "yield_curve_result.yield_to_maturity offset 16");
    CHECK(offsetof(ficant_kernel_carry_roll_input_v1, initial_dirty_price) == 8,
          "carry_roll_input.initial_dirty_price offset 8");
    CHECK(offsetof(ficant_kernel_carry_roll_result_v1, carry) == 16,
          "carry_roll_result.carry offset 16");
    CHECK(offsetof(ficant_kernel_cgb_futures_delivery_input_v1, coupon_rate) == 40,
          "futures_delivery_input.coupon_rate offset 40");
    CHECK(offsetof(ficant_kernel_cgb_futures_delivery_result_v1, conversion_factor) == 24,
          "futures_delivery_result.conversion_factor offset 24");

    if (failures > 0) {
        std::fprintf(stderr, "%d failures\n", failures);
        return 1;
    }
    return 0;
}
