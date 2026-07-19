#include "ficant_kernel.h"
#include "curve_math.hpp"

#include <cmath>
#include <cstdint>
#include <vector>

namespace {

constexpr uint32_t MAX_CURVE_NODES = 4096;

uint32_t finish(ficant_kernel_yield_curve_result_v1* result,
                uint32_t status) noexcept {
    if (result != nullptr) {
        result->status_code = status;
    }
    return status;
}

uint32_t validate_result(ficant_kernel_yield_curve_result_v1* result) noexcept {
    if (result == nullptr) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    if (result->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return FICANT_KERNEL_STATUS_ABI_MISMATCH;
    }
    if (result->struct_size != sizeof(ficant_kernel_yield_curve_result_v1)) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    result->status_code = FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    result->reserved = 0;
    result->yield_to_maturity = 0.0;
    return FICANT_KERNEL_STATUS_OK;
}

uint32_t finish(ficant_kernel_carry_roll_result_v1* result,
                uint32_t status) noexcept {
    if (result != nullptr) {
        result->status_code = status;
    }
    return status;
}

} // namespace

extern "C" uint32_t ficant_kernel_interpolate_yield_curve_v1(
    const ficant_kernel_yield_curve_input_v1* curve_input,
    const ficant_kernel_yield_curve_query_v1* query,
    ficant_kernel_yield_curve_result_v1* result) noexcept {
    const uint32_t result_status = validate_result(result);
    if (result_status != FICANT_KERNEL_STATUS_OK) {
        return result_status;
    }
    if (curve_input == nullptr || query == nullptr) {
        return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }
    if (curve_input->abi_version != FICANT_KERNEL_ABI_VERSION
        || query->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return finish(result, FICANT_KERNEL_STATUS_ABI_MISMATCH);
    }
    if (curve_input->struct_size != sizeof(ficant_kernel_yield_curve_input_v1)
        || query->struct_size != sizeof(ficant_kernel_yield_curve_query_v1)
        || curve_input->reserved != 0 || query->reserved != 0
        || curve_input->interpolation != FICANT_KERNEL_CURVE_INTERPOLATION_LINEAR_YIELD
        || curve_input->nodes == nullptr || curve_input->node_count < 2
        || curve_input->node_count > MAX_CURVE_NODES
        || query->query_date <= curve_input->valuation_date) {
        return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }

    try {
        std::vector<ficant::curve_math::YieldNode> nodes;
        nodes.reserve(curve_input->node_count);
        int32_t previous_date = curve_input->valuation_date;
        for (uint32_t index = 0; index < curve_input->node_count; ++index) {
            const ficant_kernel_yield_curve_node_v1& node = curve_input->nodes[index];
            if (node.abi_version != FICANT_KERNEL_ABI_VERSION) {
                return finish(result, FICANT_KERNEL_STATUS_ABI_MISMATCH);
            }
            if (node.struct_size != sizeof(ficant_kernel_yield_curve_node_v1)
                || node.reserved != 0 || node.maturity_date <= previous_date) {
                return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
            }
            if (!std::isfinite(node.yield_to_maturity)) {
                return finish(result, FICANT_KERNEL_STATUS_NON_FINITE);
            }
            if (node.yield_to_maturity <= -1.0) {
                return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
            }
            nodes.push_back({node.maturity_date, node.yield_to_maturity});
            previous_date = node.maturity_date;
        }
        if (query->query_date < nodes.front().maturity_date
            || query->query_date > nodes.back().maturity_date) {
            return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
        }
        const double interpolated = ficant::curve_math::linear_yield(
            nodes.data(), curve_input->node_count, query->query_date);
        if (!std::isfinite(interpolated)) {
            return finish(result, FICANT_KERNEL_STATUS_NON_FINITE);
        }
        result->yield_to_maturity = interpolated;
        return finish(result, FICANT_KERNEL_STATUS_OK);
    } catch (...) {
        return finish(result, FICANT_KERNEL_STATUS_INTERNAL_ERROR);
    }
}

extern "C" uint32_t ficant_kernel_decompose_carry_roll_v1(
    const ficant_kernel_carry_roll_input_v1* input,
    ficant_kernel_carry_roll_result_v1* result) noexcept {
    if (result == nullptr) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    if (result->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return FICANT_KERNEL_STATUS_ABI_MISMATCH;
    }
    if (result->struct_size != sizeof(ficant_kernel_carry_roll_result_v1)) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    result->status_code = FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    result->reserved = 0;
    result->carry = 0.0;
    result->roll_down = 0.0;
    result->total_return = 0.0;
    if (input == nullptr) {
        return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }
    if (input->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return finish(result, FICANT_KERNEL_STATUS_ABI_MISMATCH);
    }
    if (input->struct_size != sizeof(ficant_kernel_carry_roll_input_v1)) {
        return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }
    if (!std::isfinite(input->initial_dirty_price)
        || !std::isfinite(input->horizon_dirty_at_initial_yield)
        || !std::isfinite(input->horizon_dirty_at_rolled_yield)
        || !std::isfinite(input->paid_cashflows)) {
        return finish(result, FICANT_KERNEL_STATUS_NON_FINITE);
    }
    if (input->initial_dirty_price <= 0.0
        || input->horizon_dirty_at_initial_yield <= 0.0
        || input->horizon_dirty_at_rolled_yield <= 0.0
        || input->paid_cashflows < 0.0) {
        return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }
    const ficant::curve_math::CarryRollResult decomposed =
        ficant::curve_math::decompose_carry_roll(
            input->initial_dirty_price,
            input->horizon_dirty_at_initial_yield,
            input->horizon_dirty_at_rolled_yield,
            input->paid_cashflows);
    if (!std::isfinite(decomposed.carry)
        || !std::isfinite(decomposed.roll_down)
        || !std::isfinite(decomposed.total_return)) {
        return finish(result, FICANT_KERNEL_STATUS_NON_FINITE);
    }
    result->carry = decomposed.carry;
    result->roll_down = decomposed.roll_down;
    result->total_return = decomposed.total_return;
    return finish(result, FICANT_KERNEL_STATUS_OK);
}
