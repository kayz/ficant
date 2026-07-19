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

ficant_kernel_yield_curve_node_v1 node(int32_t date, double value) {
    return {
        sizeof(ficant_kernel_yield_curve_node_v1),
        FICANT_KERNEL_ABI_VERSION,
        date,
        0,
        value,
    };
}

uint32_t interpolate(const ficant_kernel_yield_curve_node_v1* nodes,
                     uint32_t count, int32_t query_date,
                     double& output) {
    const ficant_kernel_yield_curve_input_v1 curve{
        sizeof(ficant_kernel_yield_curve_input_v1),
        FICANT_KERNEL_ABI_VERSION,
        20000,
        FICANT_KERNEL_CURVE_INTERPOLATION_LINEAR_YIELD,
        nodes,
        count,
        0,
    };
    const ficant_kernel_yield_curve_query_v1 query{
        sizeof(ficant_kernel_yield_curve_query_v1),
        FICANT_KERNEL_ABI_VERSION,
        query_date,
        0,
    };
    ficant_kernel_yield_curve_result_v1 result{
        sizeof(ficant_kernel_yield_curve_result_v1),
        FICANT_KERNEL_ABI_VERSION,
        FICANT_KERNEL_STATUS_INTERNAL_ERROR,
        0,
        0.0,
    };
    const uint32_t status = ficant_kernel_interpolate_yield_curve_v1(
        &curve, &query, &result);
    CHECK(status == result.status_code, "return and result statuses agree");
    output = result.yield_to_maturity;
    return status;
}

} // namespace

int main() {
    const ficant_kernel_yield_curve_node_v1 nodes[] = {
        node(20100, 0.015),
        node(20300, 0.020),
        node(20700, 0.028),
    };
    double value = 0.0;
    CHECK(interpolate(nodes, 3, 20100, value) == FICANT_KERNEL_STATUS_OK,
          "first exact node succeeds");
    CHECK(value == 0.015, "first exact node preserves its rate");
    CHECK(interpolate(nodes, 3, 20300, value) == FICANT_KERNEL_STATUS_OK,
          "middle exact node succeeds");
    CHECK(value == 0.020, "middle exact node preserves its rate");
    CHECK(interpolate(nodes, 3, 20200, value) == FICANT_KERNEL_STATUS_OK,
          "interior query succeeds");
    CHECK(std::abs(value - 0.0175) < 1.0e-15,
          "interior query is linear in actual days");
    CHECK(interpolate(nodes, 3, 20500, value) == FICANT_KERNEL_STATUS_OK,
          "uneven interval query succeeds");
    CHECK(std::abs(value - 0.024) < 1.0e-15,
          "uneven interval uses local bracketing nodes");

    CHECK(interpolate(nodes, 3, 20099, value)
              == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
          "left extrapolation fails closed");
    CHECK(interpolate(nodes, 3, 20701, value)
              == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
          "right extrapolation fails closed");
    CHECK(interpolate(nodes, 1, 20100, value)
              == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
          "one-node curve is rejected");

    ficant_kernel_yield_curve_node_v1 duplicate[] = {
        node(20100, 0.015),
        node(20100, 0.016),
    };
    CHECK(interpolate(duplicate, 2, 20100, value)
              == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
          "duplicate maturity is rejected");

    ficant_kernel_yield_curve_node_v1 non_finite[] = {
        node(20100, 0.015),
        node(20300, std::numeric_limits<double>::quiet_NaN()),
    };
    CHECK(interpolate(non_finite, 2, 20150, value)
              == FICANT_KERNEL_STATUS_NON_FINITE,
          "non-finite yield has a stable failure");

    ficant_kernel_yield_curve_node_v1 abi_drift[] = {
        node(20100, 0.015),
        node(20300, 0.020),
    };
    abi_drift[1].abi_version += 1;
    CHECK(interpolate(abi_drift, 2, 20150, value)
              == FICANT_KERNEL_STATUS_ABI_MISMATCH,
          "node ABI drift is rejected");

    return failures == 0 ? 0 : 1;
}
