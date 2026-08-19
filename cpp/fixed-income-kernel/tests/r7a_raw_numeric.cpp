#include "ficant_kernel.h"

#include <bit>
#include <cinttypes>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

namespace {

[[noreturn]] void fail(const char* message) {
    std::fprintf(stderr, "r7a raw numeric runner failed: %s\n", message);
    std::exit(2);
}

void require(bool condition, const char* message) {
    if (!condition) {
        fail(message);
    }
}

void emit_u32(const char* key, std::uint32_t value) {
    std::printf("%s\t%" PRIu32 "\n", key, value);
}

void emit_i32(const char* key, std::int32_t value) {
    std::printf("%s\t%" PRId32 "\n", key, value);
}

void emit_i64(const char* key, std::int64_t value) {
    std::printf("%s\t%" PRId64 "\n", key, value);
}

void emit_f64(const char* key, double value) {
    if (!std::isfinite(value)) {
        fail("a formal floating-point output is not finite");
    }
    const std::uint64_t bits = std::bit_cast<std::uint64_t>(value);
    std::printf("%s\t%016" PRIx64 "\n", key, bits);
}

std::int32_t epoch_date(int year, unsigned month, unsigned day) {
    year -= static_cast<int>(month <= 2U);
    const int era = (year >= 0 ? year : year - 399) / 400;
    const unsigned year_of_era = static_cast<unsigned>(year - era * 400);
    const unsigned day_of_year =
        (153U * (month > 2U ? month - 3U : month + 9U) + 2U) / 5U + day - 1U;
    const unsigned day_of_era =
        year_of_era * 365U + year_of_era / 4U - year_of_era / 100U + day_of_year;
    return static_cast<std::int32_t>(era * 146097LL + day_of_era - 719468LL);
}

ficant_kernel_yield_curve_node_v1 curve_node(std::int32_t date, double rate) {
    return {
        sizeof(ficant_kernel_yield_curve_node_v1),
        FICANT_KERNEL_ABI_VERSION,
        date,
        0U,
        rate,
    };
}

} // namespace

int main() {
    ficant_kernel_bond_input_v1 bond{};
    bond.struct_size = sizeof(bond);
    bond.abi_version = FICANT_KERNEL_ABI_VERSION;
    bond.issue_date = 20588;
    bond.maturity_date = 21319;
    bond.frequency = FICANT_KERNEL_FREQUENCY_ANNUAL;
    bond.day_count_convention = FICANT_KERNEL_DAY_COUNT_ACT_ACT_BOND_ISMA;
    bond.business_day_convention = FICANT_KERNEL_BDC_FOLLOWING;
    bond.coupon_rate = 0.0130;
    bond.face_value = 100.0;

    ficant_kernel_calculate_input_v1 calculation{};
    calculation.struct_size = sizeof(calculation);
    calculation.abi_version = FICANT_KERNEL_ABI_VERSION;
    calculation.settlement_date = 20648;
    calculation.input_mode = FICANT_KERNEL_MODE_YIELD_IN;
    calculation.input_value = 0.0130;
    calculation.calendar_requirement = FICANT_KERNEL_CALENDAR_REQUIREMENT_REFERENCE_REPLAY;

    ficant_kernel_result_v1 sizing{};
    sizing.struct_size = sizeof(sizing);
    sizing.abi_version = FICANT_KERNEL_ABI_VERSION;
    const std::uint32_t sizing_status =
        ficant_kernel_calculate_bond_v1(&bond, &calculation, &sizing, nullptr, 0U);
    require(sizing_status == FICANT_KERNEL_STATUS_BUFFER_TOO_SMALL,
            "bond sizing status changed");
    require(sizing.cashflow_count > 0U, "bond sizing returned no cashflows");

    std::vector<ficant_kernel_cashflow_v1> cashflows(sizing.cashflow_count);
    for (auto& cashflow : cashflows) {
        cashflow.struct_size = sizeof(ficant_kernel_cashflow_v1);
        cashflow.abi_version = FICANT_KERNEL_ABI_VERSION;
    }
    ficant_kernel_result_v1 bond_result{};
    bond_result.struct_size = sizeof(bond_result);
    bond_result.abi_version = FICANT_KERNEL_ABI_VERSION;
    const std::uint32_t bond_status = ficant_kernel_calculate_bond_v1(
        &bond, &calculation, &bond_result, cashflows.data(),
        static_cast<std::uint32_t>(cashflows.size()));
    require(bond_status == FICANT_KERNEL_STATUS_OK, "bond calculation failed");
    require(bond_result.status_code == bond_status, "bond statuses disagree");
    require(bond_result.cashflow_count == cashflows.size(), "bond cashflow count drifted");

    const ficant_kernel_yield_curve_node_v1 curve_nodes[] = {
        curve_node(20100, 0.015),
        curve_node(20300, 0.020),
        curve_node(20700, 0.028),
    };
    const ficant_kernel_yield_curve_input_v1 curve_input{
        sizeof(ficant_kernel_yield_curve_input_v1),
        FICANT_KERNEL_ABI_VERSION,
        20000,
        FICANT_KERNEL_CURVE_INTERPOLATION_LINEAR_YIELD,
        curve_nodes,
        3U,
        0U,
    };
    const ficant_kernel_yield_curve_query_v1 curve_query{
        sizeof(ficant_kernel_yield_curve_query_v1),
        FICANT_KERNEL_ABI_VERSION,
        20500,
        0U,
    };
    ficant_kernel_yield_curve_result_v1 curve_result{
        sizeof(ficant_kernel_yield_curve_result_v1),
        FICANT_KERNEL_ABI_VERSION,
        FICANT_KERNEL_STATUS_INTERNAL_ERROR,
        0U,
        0.0,
    };
    const std::uint32_t curve_status =
        ficant_kernel_interpolate_yield_curve_v1(&curve_input, &curve_query, &curve_result);
    require(curve_status == FICANT_KERNEL_STATUS_OK, "curve interpolation failed");
    require(curve_result.status_code == curve_status, "curve statuses disagree");

    const ficant_kernel_carry_roll_input_v1 carry_input{
        sizeof(ficant_kernel_carry_roll_input_v1),
        FICANT_KERNEL_ABI_VERSION,
        100.0,
        100.4,
        101.1,
        1.2,
    };
    ficant_kernel_carry_roll_result_v1 carry_result{
        sizeof(ficant_kernel_carry_roll_result_v1),
        FICANT_KERNEL_ABI_VERSION,
        FICANT_KERNEL_STATUS_INTERNAL_ERROR,
        0U,
        0.0,
        0.0,
        0.0,
    };
    const std::uint32_t carry_status =
        ficant_kernel_decompose_carry_roll_v1(&carry_input, &carry_result);
    require(carry_status == FICANT_KERNEL_STATUS_OK, "carry/roll calculation failed");
    require(carry_result.status_code == carry_status, "carry/roll statuses disagree");

    static const std::uint32_t delivery_months[] = {3U, 6U, 9U, 12U};
    const ficant_kernel_cgb_futures_delivery_input_v1 delivery_input{
        sizeof(ficant_kernel_cgb_futures_delivery_input_v1),
        FICANT_KERNEL_ABI_VERSION,
        FICANT_KERNEL_FREQUENCY_SEMIANNUAL,
        120U,
        78U,
        0U,
        1U,
        4U,
        delivery_months,
        epoch_date(2024, 8U, 15U),
        epoch_date(2034, 8U, 15U),
        epoch_date(2026, 9U, 1U),
        epoch_date(2026, 7U, 21U),
        epoch_date(2026, 9U, 18U),
        FICANT_KERNEL_DAY_COUNT_ACT_ACT_BOND_ISMA,
        4U,
        7U,
        365U,
        0U,
        0.03,
        100.0,
        0.025,
        101.25,
        99.50,
        0.018,
    };
    ficant_kernel_cgb_futures_delivery_result_v1 delivery_result{};
    delivery_result.struct_size = sizeof(delivery_result);
    delivery_result.abi_version = FICANT_KERNEL_ABI_VERSION;
    const std::uint32_t delivery_status =
        ficant_kernel_analyze_cgb_futures_delivery_v1(&delivery_input, &delivery_result);
    require(delivery_status == FICANT_KERNEL_STATUS_OK, "delivery analysis failed");
    require(delivery_result.status_code == delivery_status, "delivery statuses disagree");

    const ficant_kernel_cgb_futures_hedge_input_v1 hedge_input{
        sizeof(ficant_kernel_cgb_futures_hedge_input_v1),
        FICANT_KERNEL_ABI_VERSION,
        FICANT_KERNEL_CGB_FUTURES_T,
        0U,
        500.0,
        0.045,
        0.9,
    };
    ficant_kernel_cgb_futures_hedge_result_v1 hedge_result{};
    hedge_result.struct_size = sizeof(hedge_result);
    hedge_result.abi_version = FICANT_KERNEL_ABI_VERSION;
    const std::uint32_t hedge_status =
        ficant_kernel_calculate_cgb_futures_hedge_v1(&hedge_input, &hedge_result);
    require(hedge_status == FICANT_KERNEL_STATUS_OK, "futures hedge calculation failed");
    require(hedge_result.status_code == hedge_status, "hedge statuses disagree");

    emit_u32("i.kernel.abi_version", ficant_kernel_abi_version());
    emit_u32("i.bond.return_status", bond_status);
    emit_u32("i.bond.struct_size", bond_result.struct_size);
    emit_u32("i.bond.abi_version", bond_result.abi_version);
    emit_u32("i.bond.cashflow_count", bond_result.cashflow_count);
    emit_u32("i.bond.calendar_resolution", bond_result.calendar_resolution);
    emit_u32("i.bond.status_code", bond_result.status_code);
    emit_f64("f.bond.accrued_interest", bond_result.accrued_interest);
    emit_f64("f.bond.clean_price", bond_result.clean_price);
    emit_f64("f.bond.dirty_price", bond_result.dirty_price);
    emit_f64("f.bond.yield_to_maturity", bond_result.yield_to_maturity);
    emit_f64("f.bond.macaulay_duration", bond_result.macaulay_duration);
    emit_f64("f.bond.modified_duration", bond_result.modified_duration);
    emit_f64("f.bond.convexity", bond_result.convexity);
    emit_f64("f.bond.dv01", bond_result.dv01);
    for (std::size_t index = 0; index < cashflows.size(); ++index) {
        const auto& cashflow = cashflows[index];
        const std::string prefix = "bond.cashflow[" + std::to_string(index) + "].";
        emit_u32(("i." + prefix + "struct_size").c_str(), cashflow.struct_size);
        emit_u32(("i." + prefix + "abi_version").c_str(), cashflow.abi_version);
        emit_u32(("i." + prefix + "sequence").c_str(), cashflow.sequence);
        emit_i32(("i." + prefix + "nominal_date").c_str(), cashflow.nominal_date);
        emit_i32(("i." + prefix + "payment_date").c_str(), cashflow.payment_date);
        emit_f64(("f." + prefix + "coupon").c_str(), cashflow.coupon);
        emit_f64(("f." + prefix + "principal").c_str(), cashflow.principal);
        emit_f64(("f." + prefix + "total").c_str(), cashflow.total);
    }

    emit_u32("i.curve.return_status", curve_status);
    emit_u32("i.curve.struct_size", curve_result.struct_size);
    emit_u32("i.curve.abi_version", curve_result.abi_version);
    emit_u32("i.curve.status_code", curve_result.status_code);
    emit_f64("f.curve.yield_to_maturity", curve_result.yield_to_maturity);

    emit_u32("i.carry_roll.return_status", carry_status);
    emit_u32("i.carry_roll.struct_size", carry_result.struct_size);
    emit_u32("i.carry_roll.abi_version", carry_result.abi_version);
    emit_u32("i.carry_roll.status_code", carry_result.status_code);
    emit_f64("f.carry_roll.carry", carry_result.carry);
    emit_f64("f.carry_roll.roll_down", carry_result.roll_down);
    emit_f64("f.carry_roll.total_return", carry_result.total_return);

    emit_u32("i.delivery.return_status", delivery_status);
    emit_u32("i.delivery.struct_size", delivery_result.struct_size);
    emit_u32("i.delivery.abi_version", delivery_result.abi_version);
    emit_u32("i.delivery.status_code", delivery_result.status_code);
    emit_u32("i.delivery.eligible", delivery_result.eligible);
    emit_u32("i.delivery.months_to_next_coupon", delivery_result.months_to_next_coupon);
    emit_u32("i.delivery.remaining_coupon_count", delivery_result.remaining_coupon_count);
    emit_f64("f.delivery.conversion_factor", delivery_result.conversion_factor);
    emit_f64("f.delivery.purchase_accrued_interest", delivery_result.purchase_accrued_interest);
    emit_f64("f.delivery.delivery_accrued_interest", delivery_result.delivery_accrued_interest);
    emit_f64("f.delivery.interim_coupons", delivery_result.interim_coupons);
    emit_f64("f.delivery.invoice_price", delivery_result.invoice_price);
    emit_f64("f.delivery.purchase_dirty_price", delivery_result.purchase_dirty_price);
    emit_f64("f.delivery.gross_basis", delivery_result.gross_basis);
    emit_f64("f.delivery.financing_cost", delivery_result.financing_cost);
    emit_f64("f.delivery.holding_carry", delivery_result.holding_carry);
    emit_f64("f.delivery.net_basis", delivery_result.net_basis);
    emit_f64("f.delivery.implied_repo_rate", delivery_result.implied_repo_rate);
    emit_f64("f.delivery.delivery_profit", delivery_result.delivery_profit);

    emit_u32("i.hedge.return_status", hedge_status);
    emit_u32("i.hedge.struct_size", hedge_result.struct_size);
    emit_u32("i.hedge.abi_version", hedge_result.abi_version);
    emit_u32("i.hedge.status_code", hedge_result.status_code);
    emit_i64("i.hedge.recommended_contracts", hedge_result.recommended_contracts);
    emit_f64("f.hedge.futures_contract_dv01", hedge_result.futures_contract_dv01);
    emit_f64("f.hedge.raw_contracts", hedge_result.raw_contracts);
    emit_f64("f.hedge.residual_dv01", hedge_result.residual_dv01);
    emit_f64("f.hedge.hedge_effectiveness", hedge_result.hedge_effectiveness);

    require(std::fflush(stdout) == 0, "stdout flush failed");
    return 0;
}
