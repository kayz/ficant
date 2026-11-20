#include "ficant_kernel.h"

#include <array>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <limits>

static int failures = 0;

#define CHECK(cond, msg) do { if (!(cond)) { std::fprintf(stderr, "FAIL: %s\n", msg); ++failures; } } while (0)

namespace {

ficant_kernel_cgb_futures_hedge_input_v1 make_v1_input() {
    return {
        sizeof(ficant_kernel_cgb_futures_hedge_input_v1),
        FICANT_KERNEL_ABI_VERSION,
        FICANT_KERNEL_CGB_FUTURES_T,
        0U,
        500.0,
        0.045,
        0.9,
    };
}

ficant_kernel_cgb_futures_hedge_input_v2 make_v2_input(
    double contract_notional = 1'000'000.0,
    double face_quote_basis = 100.0,
    double target_dv01 = 500.0,
    double ctd_dv01_per_quote_unit = 0.045,
    double conversion_factor = 0.9) {
    return {
        sizeof(ficant_kernel_cgb_futures_hedge_input_v2),
        FICANT_KERNEL_ABI_VERSION,
        contract_notional,
        face_quote_basis,
        target_dv01,
        ctd_dv01_per_quote_unit,
        conversion_factor,
    };
}

ficant_kernel_cgb_futures_hedge_result_v1 make_sentinel_result(
    uint32_t struct_size = sizeof(ficant_kernel_cgb_futures_hedge_result_v1),
    uint32_t abi_version = FICANT_KERNEL_ABI_VERSION) {
    ficant_kernel_cgb_futures_hedge_result_v1 value{};
    std::memset(&value, 0xAA, sizeof(value));
    value.struct_size = struct_size;
    value.abi_version = abi_version;
    return value;
}

void check_zeroed_failure_result(
    const ficant_kernel_cgb_futures_hedge_result_v1& res,
    uint32_t expected_status,
    const char* label) {
    CHECK(res.struct_size == sizeof(ficant_kernel_cgb_futures_hedge_result_v1), label);
    CHECK(res.abi_version == FICANT_KERNEL_ABI_VERSION, label);
    CHECK(res.status_code == expected_status, label);
    CHECK(res.reserved == 0U, label);
    CHECK(res.futures_contract_dv01 == 0.0, label);
    CHECK(res.raw_contracts == 0.0, label);
    CHECK(res.recommended_contracts == 0, label);
    CHECK(res.residual_dv01 == 0.0, label);
    CHECK(res.hedge_effectiveness == 0.0, label);
}

void check_sentinel_zero_write_v1(
    const ficant_kernel_cgb_futures_hedge_input_v1* req,
    uint32_t bad_size,
    uint32_t bad_abi,
    const char* label) {
    auto res = make_sentinel_result(bad_size, bad_abi);
    std::array<uint8_t, sizeof(res)> before{};
    std::array<uint8_t, sizeof(res)> after{};
    std::memcpy(before.data(), &res, sizeof(res));
    const uint32_t status = ficant_kernel_calculate_cgb_futures_hedge_v1(req, &res);
    std::memcpy(after.data(), &res, sizeof(res));
    CHECK(status == FICANT_KERNEL_STATUS_ABI_MISMATCH, label);
    CHECK(std::memcmp(before.data(), after.data(), sizeof(res)) == 0, label);
}

void check_sentinel_zero_write_v2(
    const ficant_kernel_cgb_futures_hedge_input_v2* req,
    uint32_t bad_size,
    uint32_t bad_abi,
    const char* label) {
    auto res = make_sentinel_result(bad_size, bad_abi);
    std::array<uint8_t, sizeof(res)> before{};
    std::array<uint8_t, sizeof(res)> after{};
    std::memcpy(before.data(), &res, sizeof(res));
    const uint32_t status = ficant_kernel_calculate_cgb_futures_hedge_v2(req, &res);
    std::memcpy(after.data(), &res, sizeof(res));
    CHECK(status == FICANT_KERNEL_STATUS_ABI_MISMATCH, label);
    CHECK(std::memcmp(before.data(), after.data(), sizeof(res)) == 0, label);
}

} // namespace

int main() {
    /* ── Steps 1-3: invalid result pointer or header -> zero writes ── */
    {
        const auto v1_req = make_v1_input();
        const auto v2_req = make_v2_input();

        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(&v1_req, nullptr)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "v1 null result fails with INVALID_ARGUMENT");
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(nullptr, nullptr)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "v1 null input and null result fails with INVALID_ARGUMENT");
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&v2_req, nullptr)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "v2 null result fails with INVALID_ARGUMENT");
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(nullptr, nullptr)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "v2 null input and null result fails with INVALID_ARGUMENT");

        check_sentinel_zero_write_v1(
            &v1_req, sizeof(ficant_kernel_cgb_futures_hedge_result_v1) - 1U,
            FICANT_KERNEL_ABI_VERSION, "v1 bad result struct_size performs zero writes");
        check_sentinel_zero_write_v1(
            nullptr, 0U, FICANT_KERNEL_ABI_VERSION,
            "v1 bad result struct_size checked before null input with zero writes");
        check_sentinel_zero_write_v1(
            &v1_req, sizeof(ficant_kernel_cgb_futures_hedge_result_v1),
            FICANT_KERNEL_ABI_VERSION + 1U, "v1 bad result abi_version performs zero writes");
        check_sentinel_zero_write_v1(
            nullptr, sizeof(ficant_kernel_cgb_futures_hedge_result_v1),
            0U, "v1 zero result abi_version checked before null input with zero writes");

        check_sentinel_zero_write_v2(
            &v2_req, sizeof(ficant_kernel_cgb_futures_hedge_result_v1) - 1U,
            FICANT_KERNEL_ABI_VERSION, "v2 bad result struct_size performs zero writes");
        check_sentinel_zero_write_v2(
            &v2_req, sizeof(ficant_kernel_cgb_futures_hedge_result_v1) + 8U,
            FICANT_KERNEL_ABI_VERSION, "v2 oversized result struct_size performs zero writes");
        check_sentinel_zero_write_v2(
            nullptr, 0U, FICANT_KERNEL_ABI_VERSION,
            "v2 bad result struct_size checked before null input with zero writes");
        check_sentinel_zero_write_v2(
            &v2_req, sizeof(ficant_kernel_cgb_futures_hedge_result_v1),
            FICANT_KERNEL_ABI_VERSION + 1U, "v2 bad result abi_version performs zero writes");
        check_sentinel_zero_write_v2(
            nullptr, sizeof(ficant_kernel_cgb_futures_hedge_result_v1),
            0U, "v2 zero result abi_version checked before null input with zero writes");
    }

    /* ── Steps 4-5: v1 input validation and fail-closed rejection ──── */
    {
        auto out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(nullptr, &out)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "v1 null input returns INVALID_ARGUMENT");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT, "v1 null input zeros payload");

        auto bad_size = make_v1_input();
        bad_size.struct_size -= 1U;
        out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(&bad_size, &out)
                  == FICANT_KERNEL_STATUS_ABI_MISMATCH,
              "v1 input struct_size drift fails closed with ABI_MISMATCH");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_ABI_MISMATCH, "v1 bad size zeros payload");

        auto bad_abi = make_v1_input();
        bad_abi.abi_version = FICANT_KERNEL_ABI_VERSION + 1U;
        out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(&bad_abi, &out)
                  == FICANT_KERNEL_STATUS_ABI_MISMATCH,
              "v1 input abi_version drift fails closed with ABI_MISMATCH");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_ABI_MISMATCH, "v1 bad abi zeros payload");

        const uint32_t products[] = {
            FICANT_KERNEL_CGB_FUTURES_TS,
            FICANT_KERNEL_CGB_FUTURES_TF,
            FICANT_KERNEL_CGB_FUTURES_T,
            FICANT_KERNEL_CGB_FUTURES_TL,
        };
        for (uint32_t product : products) {
            auto req = make_v1_input();
            req.product = product;
            out = make_sentinel_result();
            CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(&req, &out)
                      == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
                  "valid v1 header fails closed with INVALID_ARGUMENT");
            check_zeroed_failure_result(
                out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
                "valid v1 header performs no calculation and zeros payload");
        }

        auto reserved_req = make_v1_input();
        reserved_req.reserved = 1U;
        out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(&reserved_req, &out)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "v1 reserved drift fails closed with INVALID_ARGUMENT");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT, "v1 reserved drift zeros payload");

        auto non_finite_v1 = make_v1_input();
        non_finite_v1.target_dv01 = std::numeric_limits<double>::quiet_NaN();
        out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v1(&non_finite_v1, &out)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "valid v1 header rejects before field evaluation with INVALID_ARGUMENT");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT, "v1 NaN still zeros payload");
    }

    /* ── v2 1M baseline (preserving original assertions & 1e-12 tol) ─ */
    {
        auto request = make_v2_input();
        auto output = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&request, &output)
                  == FICANT_KERNEL_STATUS_OK,
              "valid 1M v2 hedge succeeds");
        CHECK(output.status_code == FICANT_KERNEL_STATUS_OK, "1M status_code is OK");
        CHECK(output.reserved == 0U, "1M reserved cleared to 0");
        CHECK(std::fabs(output.futures_contract_dv01 - 500.0) < 1e-12,
              "contract DV01 follows CTD/CF identity");
        CHECK(std::fabs(output.raw_contracts + 1.0) < 1e-12,
              "long cash risk requires one short futures contract");
        CHECK(output.recommended_contracts == -1, "recommended contract is short one");
        CHECK(std::fabs(output.residual_dv01) < 1e-12, "exact hedge has zero residual");
        CHECK(std::fabs(output.hedge_effectiveness - 1.0) < 1e-12,
              "exact hedge is fully effective");

        request.target_dv01 = -250.0;
        output = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&request, &output)
                  == FICANT_KERNEL_STATUS_OK,
              "short cash risk succeeds");
        CHECK(output.recommended_contracts == 0,
              "half-contract tie chooses lower absolute position");
        CHECK(std::fabs(output.residual_dv01 + 250.0) < 1e-12,
              "zero-hand tie residual equals target");
        CHECK(std::fabs(output.hedge_effectiveness) < 1e-12,
              "zero-hand tie leaves original risk");

        request.target_dv01 = 750.0;
        output = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&request, &output)
                  == FICANT_KERNEL_STATUS_OK,
              "1.5-hand tie succeeds");
        CHECK(output.recommended_contracts == -1,
              "1.5-hand tie chooses smaller absolute contract count");

        request.target_dv01 = -750.0;
        output = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&request, &output)
                  == FICANT_KERNEL_STATUS_OK,
              "negative target 1.5-hand tie succeeds");
        CHECK(output.recommended_contracts == 1,
              "positive 1.5-hand tie chooses smaller absolute contract count");
    }

    /* ── v2 2M (20,000 x 100) notional scale & 1M vs 2M comparison ── */
    {
        auto req_2m_exact = make_v2_input(2'000'000.0, 100.0, 500.0, 0.045, 0.9);
        auto out_2m_exact = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&req_2m_exact, &out_2m_exact)
                  == FICANT_KERNEL_STATUS_OK,
              "valid 2M hedge succeeds");
        CHECK(std::fabs(out_2m_exact.futures_contract_dv01 - 1000.0) < 1e-12,
              "2M contract DV01 doubles relative to 1M");
        CHECK(std::fabs(out_2m_exact.raw_contracts + 0.5) < 1e-12,
              "2M raw contracts halves relative to 1M");
        CHECK(out_2m_exact.recommended_contracts == 0,
              "2M half-contract tie picks 0");
        CHECK(std::fabs(out_2m_exact.residual_dv01 - 500.0) < 1e-12,
              "2M half-contract residual is 500");
        CHECK(std::fabs(out_2m_exact.hedge_effectiveness) < 1e-12,
              "2M zero-hand hedge effectiveness is 0");

        auto req_2m_ts = make_v2_input(2'000'000.0, 100.0, 1234.50, 0.0145, 0.9987);
        auto out_2m_ts = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&req_2m_ts, &out_2m_ts)
                  == FICANT_KERNEL_STATUS_OK,
              "2M TS-scale hedge succeeds");
        const double expected_2m_dv01 = 0.0145 * (2'000'000.0 / 100.0) / 0.9987;
        CHECK(std::fabs(out_2m_ts.futures_contract_dv01 - expected_2m_dv01) < 1e-12,
              "2M TS-scale contract DV01 matches formula");
        CHECK(out_2m_ts.recommended_contracts == -4,
              "2M TS-scale recommends -4 contracts");

        auto req_1m_ts = make_v2_input(1'000'000.0, 100.0, 1234.50, 0.0145, 0.9987);
        auto out_1m_ts = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&req_1m_ts, &out_1m_ts)
                  == FICANT_KERNEL_STATUS_OK,
              "1M comparison hedge succeeds");
        CHECK(std::fabs(out_2m_ts.futures_contract_dv01 - 2.0 * out_1m_ts.futures_contract_dv01)
                  < 1e-12,
              "2M contract DV01 is 2x 1M contract DV01");
        CHECK(std::fabs(out_1m_ts.raw_contracts - 2.0 * out_2m_ts.raw_contracts) < 1e-12,
              "1M raw contracts is 2x 2M raw contracts");
        CHECK(out_1m_ts.recommended_contracts == -9,
              "1M comparison recommends -9 contracts");
    }

    /* ── Scale invariance: same notional, basis=200 & ctd_dv01=2x ── */
    {
        const auto basis_100 = make_v2_input(2'000'000.0, 100.0, 1234.50, 0.0145, 0.9987);
        const auto basis_200 = make_v2_input(2'000'000.0, 200.0, 1234.50, 0.0290, 0.9987);
        auto out_100 = make_sentinel_result();
        auto out_200 = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&basis_100, &out_100)
                  == FICANT_KERNEL_STATUS_OK,
              "basis=100 succeeds");
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&basis_200, &out_200)
                  == FICANT_KERNEL_STATUS_OK,
              "basis=200 succeeds");
        CHECK(std::fabs(out_100.futures_contract_dv01 - out_200.futures_contract_dv01) < 1e-12,
              "scale invariance preserves contract DV01");
        CHECK(std::fabs(out_100.raw_contracts - out_200.raw_contracts) < 1e-12,
              "scale invariance preserves raw contracts");
        CHECK(out_100.recommended_contracts == out_200.recommended_contracts,
              "scale invariance preserves recommended contracts");
        CHECK(std::fabs(out_100.residual_dv01 - out_200.residual_dv01) < 1e-12,
              "scale invariance preserves residual DV01");
        CHECK(std::fabs(out_100.hedge_effectiveness - out_200.hedge_effectiveness) < 1e-12,
              "scale invariance preserves hedge effectiveness");
    }

    /* ── v2 step 5: input null / layout / abi_version failures ───── */
    {
        auto out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(nullptr, &out)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "v2 null input fails closed");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT, "v2 null input zeros payload");

        auto bad_size = make_v2_input();
        bad_size.struct_size -= 1U;
        out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&bad_size, &out)
                  == FICANT_KERNEL_STATUS_ABI_MISMATCH,
              "v2 input layout drift fails closed");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_ABI_MISMATCH, "v2 bad size zeros payload");

        auto bad_abi = make_v2_input();
        bad_abi.abi_version = FICANT_KERNEL_ABI_VERSION + 1U;
        out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&bad_abi, &out)
                  == FICANT_KERNEL_STATUS_ABI_MISMATCH,
              "v2 input abi_version drift fails closed");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_ABI_MISMATCH, "v2 bad abi zeros payload");
    }

    /* ── v2 step 6: non-finite inputs on all 5 fields -> NON_FINITE ─ */
    {
        const double bad_values[] = {
            std::numeric_limits<double>::infinity(),
            -std::numeric_limits<double>::infinity(),
            std::numeric_limits<double>::quiet_NaN(),
        };
        for (double bad : bad_values) {
            for (int field = 0; field < 5; ++field) {
                auto req = make_v2_input();
                if (field == 0) req.contract_notional = bad;
                if (field == 1) req.face_quote_basis = bad;
                if (field == 2) req.target_dv01 = bad;
                if (field == 3) req.ctd_dv01_per_quote_unit = bad;
                if (field == 4) req.conversion_factor = bad;
                auto out = make_sentinel_result();
                CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&req, &out)
                          == FICANT_KERNEL_STATUS_NON_FINITE,
                      "non-finite v2 input fails closed with NON_FINITE");
                check_zeroed_failure_result(
                    out, FICANT_KERNEL_STATUS_NON_FINITE, "non-finite v2 zeros payload");
            }
        }
    }

    /* ── v2 step 6: domain & range failures -> INVALID_ARGUMENT ──── */
    {
        const double non_positive[] = {0.0, -0.0, -1.0};
        for (double bad : non_positive) {
            for (int field = 0; field < 4; ++field) {
                auto req = make_v2_input();
                if (field == 0) req.contract_notional = bad;
                if (field == 1) req.face_quote_basis = bad;
                if (field == 2) req.ctd_dv01_per_quote_unit = bad;
                if (field == 3) req.conversion_factor = bad;
                auto out = make_sentinel_result();
                CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&req, &out)
                          == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
                      "non-positive v2 positive field fails closed");
                check_zeroed_failure_result(
                    out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
                    "non-positive v2 zeros payload");
            }
        }

        const double zero_targets[] = {0.0, -0.0};
        for (double z : zero_targets) {
            auto req = make_v2_input();
            req.target_dv01 = z;
            auto out = make_sentinel_result();
            CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&req, &out)
                      == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
                  "zero target_dv01 fails closed");
            check_zeroed_failure_result(
                out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT, "zero target_dv01 zeros payload");
        }

        auto overflow_dv01 = make_v2_input(1e308, 1e-100, 500.0, 1.0, 1.0);
        auto out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&overflow_dv01, &out)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "overflowing contract DV01 fails closed");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT, "overflowing DV01 zeros payload");

        auto underflow_dv01 = make_v2_input(1e-300, 1e100, 500.0, 1e-100, 1.0);
        out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&underflow_dv01, &out)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "underflowing contract DV01 fails closed");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT, "underflowing DV01 zeros payload");
    }

    /* ── v2 step 6: int64 representability boundaries ────────────── */
    {
        constexpr double two_pow_63 = 9'223'372'036'854'775'808.0;
        constexpr double max_representable_double = 9'223'372'036'854'774'784.0;

        auto pos_overflow = make_v2_input(100.0, 100.0, -two_pow_63, 1.0, 1.0);
        auto out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&pos_overflow, &out)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "raw == +2^63 fails closed (exceeds INT64_MAX)");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT, "pos int64 overflow zeros payload");

        auto pos_boundary = make_v2_input(100.0, 100.0, -max_representable_double, 1.0, 1.0);
        out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&pos_boundary, &out)
                  == FICANT_KERNEL_STATUS_OK,
              "raw == 2^63 - 1024 succeeds");
        CHECK(out.recommended_contracts == INT64_C(9223372036854774784),
              "raw == 2^63 - 1024 produces exact int64");

        auto neg_boundary = make_v2_input(100.0, 100.0, two_pow_63, 1.0, 1.0);
        out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&neg_boundary, &out)
                  == FICANT_KERNEL_STATUS_OK,
              "raw == -2^63 succeeds");
        CHECK(out.recommended_contracts == std::numeric_limits<int64_t>::min(),
              "raw == -2^63 produces INT64_MIN");

        auto neg_overflow = make_v2_input(100.0, 100.0, two_pow_63 + 2048.0, 1.0, 1.0);
        out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&neg_overflow, &out)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "raw < -2^63 fails closed");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT, "neg int64 overflow zeros payload");

        auto raw_inf = make_v2_input(100.0, 100.0, 1e308, 1e-100, 1.0);
        out = make_sentinel_result();
        CHECK(ficant_kernel_calculate_cgb_futures_hedge_v2(&raw_inf, &out)
                  == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
              "overflowing raw_contracts fails closed");
        check_zeroed_failure_result(
            out, FICANT_KERNEL_STATUS_INVALID_ARGUMENT, "overflowing raw zeros payload");
    }

    return failures == 0 ? 0 : 1;
}
