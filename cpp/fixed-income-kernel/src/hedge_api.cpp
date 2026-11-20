#include "ficant_kernel.h"

#include "hedge_math.hpp"

#include <cmath>
#include <cstdint>

namespace {

void zero_result(ficant_kernel_cgb_futures_hedge_result_v1* result) noexcept {
    result->status_code = FICANT_KERNEL_STATUS_OK;
    result->reserved = 0;
    result->futures_contract_dv01 = 0.0;
    result->raw_contracts = 0.0;
    result->recommended_contracts = 0;
    result->residual_dv01 = 0.0;
    result->hedge_effectiveness = 0.0;
}

uint32_t validate_and_zero_result(ficant_kernel_cgb_futures_hedge_result_v1* result) noexcept {
    if (result == nullptr) {
        return FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
    }
    if (result->struct_size != sizeof(ficant_kernel_cgb_futures_hedge_result_v1)) {
        return FICANT_KERNEL_STATUS_ABI_MISMATCH;
    }
    if (result->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return FICANT_KERNEL_STATUS_ABI_MISMATCH;
    }
    zero_result(result);
    return FICANT_KERNEL_STATUS_OK;
}

uint32_t finish(ficant_kernel_cgb_futures_hedge_result_v1* result,
                uint32_t status) noexcept {
    result->status_code = status;
    return status;
}

} // namespace

extern "C" uint32_t ficant_kernel_calculate_cgb_futures_hedge_v1(
    const ficant_kernel_cgb_futures_hedge_input_v1* input,
    ficant_kernel_cgb_futures_hedge_result_v1* result) noexcept {
    const uint32_t header_status = validate_and_zero_result(result);
    if (header_status != FICANT_KERNEL_STATUS_OK) {
        return header_status;
    }
    if (input == nullptr) {
        return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }
    if (input->struct_size != sizeof(ficant_kernel_cgb_futures_hedge_input_v1)
        || input->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return finish(result, FICANT_KERNEL_STATUS_ABI_MISMATCH);
    }
    return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
}

extern "C" uint32_t ficant_kernel_calculate_cgb_futures_hedge_v2(
    const ficant_kernel_cgb_futures_hedge_input_v2* input,
    ficant_kernel_cgb_futures_hedge_result_v1* result) noexcept {
    const uint32_t header_status = validate_and_zero_result(result);
    if (header_status != FICANT_KERNEL_STATUS_OK) {
        return header_status;
    }
    if (input == nullptr) {
        return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }
    if (input->struct_size != sizeof(ficant_kernel_cgb_futures_hedge_input_v2)
        || input->abi_version != FICANT_KERNEL_ABI_VERSION) {
        return finish(result, FICANT_KERNEL_STATUS_ABI_MISMATCH);
    }
    if (!std::isfinite(input->contract_notional)
        || !std::isfinite(input->face_quote_basis)
        || !std::isfinite(input->target_dv01)
        || !std::isfinite(input->ctd_dv01_per_quote_unit)
        || !std::isfinite(input->conversion_factor)) {
        return finish(result, FICANT_KERNEL_STATUS_NON_FINITE);
    }
    ficant::hedge_math::HedgeMeasures measures{};
    if (!ficant::hedge_math::calculate(
            input->contract_notional,
            input->face_quote_basis,
            input->target_dv01,
            input->ctd_dv01_per_quote_unit,
            input->conversion_factor,
            measures)) {
        return finish(result, FICANT_KERNEL_STATUS_INVALID_ARGUMENT);
    }
    result->futures_contract_dv01 = measures.futures_contract_dv01;
    result->raw_contracts = measures.raw_contracts;
    result->recommended_contracts = measures.recommended_contracts;
    result->residual_dv01 = measures.residual_dv01;
    result->hedge_effectiveness = measures.hedge_effectiveness;
    return finish(result, FICANT_KERNEL_STATUS_OK);
}
