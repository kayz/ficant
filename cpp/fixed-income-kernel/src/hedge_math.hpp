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

bool calculate(double contract_notional,
               double face_quote_basis,
               double target_dv01,
               double ctd_dv01_per_quote_unit,
               double conversion_factor,
               HedgeMeasures& output) noexcept;

} // namespace ficant::hedge_math

#endif
