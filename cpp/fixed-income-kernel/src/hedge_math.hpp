#ifndef FICANT_HEDGE_MATH_HPP
#define FICANT_HEDGE_MATH_HPP

#include <cstdint>

namespace ficant::hedge_math {

struct HedgeMeasures {
    double futures_contract_dv01 = 0.0;
    double raw_contracts = 0.0;
    int64_t recommended_contracts = 0;
    double residual_dv01 = 0.0;
    double hedge_effectiveness = 0.0;
};

bool calculate(uint32_t product,
               double target_dv01,
               double ctd_dv01_per_100,
               double conversion_factor,
               HedgeMeasures& output) noexcept;

} // namespace ficant::hedge_math

#endif
