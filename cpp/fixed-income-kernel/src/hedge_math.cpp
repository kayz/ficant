#include "hedge_math.hpp"

#include "ficant_kernel.h"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cmath>
#include <cstdint>

namespace ficant::hedge_math {
namespace {

constexpr double CONTRACT_NOTIONAL = 1'000'000.0;
constexpr double FACE_QUOTE = 100.0;
constexpr double INT64_MIN_AS_DOUBLE = -9'223'372'036'854'775'808.0;
constexpr double INT64_MAX_EXCLUSIVE_AS_DOUBLE = 9'223'372'036'854'775'808.0;

bool supported_product(uint32_t product) noexcept {
    return product == FICANT_KERNEL_CGB_FUTURES_TS
        || product == FICANT_KERNEL_CGB_FUTURES_TF
        || product == FICANT_KERNEL_CGB_FUTURES_T
        || product == FICANT_KERNEL_CGB_FUTURES_TL;
}

bool better_candidate(int64_t candidate,
                      double candidate_risk,
                      int64_t incumbent,
                      double incumbent_risk,
                      double tolerance) noexcept {
    if (candidate_risk + tolerance < incumbent_risk) return true;
    if (std::fabs(candidate_risk - incumbent_risk) > tolerance) return false;
    const uint64_t candidate_abs = candidate < 0
        ? uint64_t(-(candidate + 1)) + 1U
        : uint64_t(candidate);
    const uint64_t incumbent_abs = incumbent < 0
        ? uint64_t(-(incumbent + 1)) + 1U
        : uint64_t(incumbent);
    return candidate_abs < incumbent_abs
        || (candidate_abs == incumbent_abs && candidate < incumbent);
}

} // namespace

bool calculate(uint32_t product,
               double target_dv01,
               double ctd_dv01_per_100,
               double conversion_factor,
               HedgeMeasures& output) noexcept {
    if (!supported_product(product)
        || !std::isfinite(target_dv01) || target_dv01 == 0.0
        || !std::isfinite(ctd_dv01_per_100) || ctd_dv01_per_100 <= 0.0
        || !std::isfinite(conversion_factor) || conversion_factor <= 0.0) {
        return false;
    }
    const double futures_dv01 =
        ctd_dv01_per_100 * (CONTRACT_NOTIONAL / FACE_QUOTE) / conversion_factor;
    const double raw = -target_dv01 / futures_dv01;
    if (!std::isfinite(futures_dv01) || futures_dv01 <= 0.0 || !std::isfinite(raw)
        || raw < INT64_MIN_AS_DOUBLE || raw >= INT64_MAX_EXCLUSIVE_AS_DOUBLE) {
        return false;
    }
    const double floor_value = std::floor(raw);
    const double ceil_value = std::ceil(raw);
    if (floor_value < INT64_MIN_AS_DOUBLE || ceil_value >= INT64_MAX_EXCLUSIVE_AS_DOUBLE) {
        return false;
    }
    const std::array<int64_t, 3> candidates{
        static_cast<int64_t>(floor_value),
        static_cast<int64_t>(ceil_value),
        INT64_C(0),
    };
    int64_t best = candidates[0];
    double best_residual = target_dv01 + static_cast<double>(best) * futures_dv01;
    double best_risk = std::fabs(best_residual);
    const double tolerance = 1e-12 * std::max(1.0, std::fabs(target_dv01));
    for (std::size_t index = 1; index < candidates.size(); ++index) {
        const int64_t candidate = candidates[index];
        const double residual = target_dv01 + static_cast<double>(candidate) * futures_dv01;
        const double risk = std::fabs(residual);
        if (better_candidate(candidate, risk, best, best_risk, tolerance)) {
            best = candidate;
            best_residual = residual;
            best_risk = risk;
        }
    }
    double effectiveness = 1.0 - best_risk / std::fabs(target_dv01);
    effectiveness = std::clamp(effectiveness, 0.0, 1.0);
    if (!std::isfinite(best_residual) || !std::isfinite(effectiveness)) return false;
    output = HedgeMeasures{futures_dv01, raw, best, best_residual, effectiveness};
    return true;
}

} // namespace ficant::hedge_math
