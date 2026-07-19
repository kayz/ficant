#include "curve_math.hpp"

#include <limits>

namespace ficant::curve_math {

double linear_yield(const YieldNode* nodes, uint32_t count,
                    int32_t query_date) noexcept {
    if (nodes == nullptr || count < 2 || query_date < nodes[0].maturity_date
        || query_date > nodes[count - 1].maturity_date) {
        return std::numeric_limits<double>::quiet_NaN();
    }
    if (query_date == nodes[0].maturity_date) {
        return nodes[0].yield_to_maturity;
    }
    for (uint32_t index = 1; index < count; ++index) {
        const YieldNode& upper = nodes[index];
        if (query_date == upper.maturity_date) {
            return upper.yield_to_maturity;
        }
        if (query_date < upper.maturity_date) {
            const YieldNode& lower = nodes[index - 1];
            const double numerator = static_cast<double>(query_date - lower.maturity_date);
            const double denominator = static_cast<double>(upper.maturity_date - lower.maturity_date);
            const double weight = numerator / denominator;
            return lower.yield_to_maturity
                + weight * (upper.yield_to_maturity - lower.yield_to_maturity);
        }
    }
    return std::numeric_limits<double>::quiet_NaN();
}

} // namespace ficant::curve_math
