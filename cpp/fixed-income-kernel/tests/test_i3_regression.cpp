#include "ficant_kernel.h"
#include "../src/date_utils.hpp"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <limits>

namespace {

int failures = 0;

constexpr double PRICE_TOL = 1e-8;
constexpr double YIELD_TOL = 1e-10;
constexpr double RISK_REL_TOL = 1e-8;
constexpr double RISK_ABS_TOL = 1e-10;
constexpr double DV01_TOL = 1e-8;

void check(bool condition, const char* label) {
    if (!condition) {
        std::fprintf(stderr, "FAIL: %s\n", label);
        ++failures;
    }
}

void check_abs(double actual, double expected, double tolerance, const char* label) {
    if (std::fabs(actual - expected) > tolerance) {
        std::fprintf(stderr,
                     "FAIL: %s actual=%.15f expected=%.15f tolerance=%.3e\n",
                     label, actual, expected, tolerance);
        ++failures;
    }
}

void check_risk(double actual, double expected, const char* label) {
    const double tolerance = std::max(RISK_ABS_TOL, RISK_REL_TOL * std::fabs(expected));
    check_abs(actual, expected, tolerance, label);
}

void test_gregorian_january_february_round_trip() {
    for (const auto date : {
             ficant::date_utils::ymd_to_days(2024, 1, 31),
             ficant::date_utils::ymd_to_days(2024, 2, 29),
             ficant::date_utils::ymd_to_days(2026, 1, 1),
             ficant::date_utils::ymd_to_days(2026, 2, 28),
         }) {
        int year = 0;
        unsigned month = 0;
        unsigned day = 0;
        ficant::date_utils::days_to_ymd(date, year, month, day);
        check(ficant::date_utils::ymd_to_days(year, month, day) == date,
              "Gregorian January/February epoch round trip");
    }
    check(ficant::date_utils::add_months(
              ficant::date_utils::ymd_to_days(2026, 1, 1), 12)
              == ficant::date_utils::ymd_to_days(2027, 1, 1),
          "January annual schedule anchor remains January");
    check(ficant::date_utils::add_months(
              ficant::date_utils::ymd_to_days(2024, 2, 29), 12)
              == ficant::date_utils::ymd_to_days(2025, 2, 28),
          "leap-day annual schedule clamps to February month end");
}

constexpr int32_t ymd_to_days(int y, unsigned m, unsigned d) noexcept {
    y -= static_cast<int>(m <= 2);
    const int era = (y >= 0 ? y : y - 399) / 400;
    const unsigned yoe = static_cast<unsigned>(y - era * 400);
    const unsigned doy = (153 * (m > 2 ? m - 3 : m + 9) + 2) / 5 + d - 1;
    const unsigned doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    return static_cast<int32_t>(era * 146097LL + doe - 719468LL);
}

ficant_kernel_bond_input_v1 make_bond(
    int32_t issue_date,
    int32_t maturity_date,
    double coupon_rate,
    uint32_t frequency) {
    ficant_kernel_bond_input_v1 bond{};
    bond.struct_size = sizeof(bond);
    bond.abi_version = FICANT_KERNEL_ABI_VERSION;
    bond.issue_date = issue_date;
    bond.maturity_date = maturity_date;
    bond.frequency = frequency;
    bond.day_count_convention = FICANT_KERNEL_DAY_COUNT_ACT_ACT_BOND_ISMA;
    bond.business_day_convention = FICANT_KERNEL_BDC_FOLLOWING;
    bond.coupon_rate = coupon_rate;
    bond.face_value = 100.0;
    return bond;
}

ficant_kernel_calculate_input_v1 make_calc(double input_value) {
    ficant_kernel_calculate_input_v1 calc{};
    calc.struct_size = sizeof(calc);
    calc.abi_version = FICANT_KERNEL_ABI_VERSION;
    calc.settlement_date = ymd_to_days(2026, 7, 14);
    calc.input_mode = FICANT_KERNEL_MODE_YIELD_IN;
    calc.input_value = input_value;
    calc.calendar_requirement = FICANT_KERNEL_CALENDAR_REQUIREMENT_REFERENCE_REPLAY;
    calc.calendar_coverage_start = 0;
    calc.calendar_coverage_end = 0;
    return calc;
}

void init_result(ficant_kernel_result_v1& result) {
    std::memset(&result, 0, sizeof(result));
    result.struct_size = sizeof(result);
    result.abi_version = FICANT_KERNEL_ABI_VERSION;
}

void init_cashflows(ficant_kernel_cashflow_v1* cashflows, uint32_t count) {
    for (uint32_t i = 0; i < count; ++i) {
        std::memset(&cashflows[i], 0, sizeof(cashflows[i]));
        cashflows[i].struct_size = sizeof(cashflows[i]);
        cashflows[i].abi_version = FICANT_KERNEL_ABI_VERSION;
    }
}

uint32_t calculate_full(
    const ficant_kernel_bond_input_v1& bond,
    const ficant_kernel_calculate_input_v1& calc,
    ficant_kernel_result_v1& result,
    ficant_kernel_cashflow_v1* cashflows,
    uint32_t capacity) {
    init_result(result);
    init_cashflows(cashflows, capacity);
    return ficant_kernel_calculate_bond_v1(
        &bond, &calc, &result, cashflows, capacity);
}

void check_status_sync(
    const ficant_kernel_bond_input_v1* bond,
    const ficant_kernel_calculate_input_v1* calc,
    ficant_kernel_cashflow_v1* cashflows,
    uint32_t capacity,
    uint32_t expected,
    const char* label) {
    ficant_kernel_result_v1 result;
    init_result(result);
    result.status_code = UINT32_C(0xA5A5A5A5);
    const uint32_t actual = ficant_kernel_calculate_bond_v1(
        bond, calc, &result, cashflows, capacity);
    check(actual == expected, label);
    check(result.status_code == actual, label);
}

void check_result(
    const ficant_kernel_result_v1& result,
    double accrued,
    double clean,
    double dirty,
    double yield,
    double macaulay,
    double modified,
    double convexity,
    double dv01,
    const char* label) {
    check_abs(result.accrued_interest, accrued, PRICE_TOL, label);
    check_abs(result.clean_price, clean, PRICE_TOL, label);
    check_abs(result.dirty_price, dirty, PRICE_TOL, label);
    check_abs(result.yield_to_maturity, yield, YIELD_TOL, label);
    check_risk(result.macaulay_duration, macaulay, label);
    check_risk(result.modified_duration, modified, label);
    check_risk(result.convexity, convexity, label);
    check_abs(result.dv01, dv01, DV01_TOL, label);
}

void test_following_exact_result() {
    auto bond = make_bond(
        ymd_to_days(2026, 7, 14), ymd_to_days(2026, 7, 15),
        0.01, FICANT_KERNEL_FREQUENCY_ANNUAL);
    int32_t non_business_days[] = {ymd_to_days(2026, 7, 15)};
    auto calc = make_calc(0.01);
    calc.settlement_date = ymd_to_days(2026, 7, 15);
    calc.calendar_coverage_start = ymd_to_days(2026, 7, 15);
    calc.calendar_coverage_end = ymd_to_days(2026, 7, 20);
    calc.non_business_days = non_business_days;
    calc.non_business_days_count = 1;

    ficant_kernel_cashflow_v1 cashflows[1];
    ficant_kernel_result_v1 result;
    const uint32_t status = calculate_full(bond, calc, result, cashflows, 1);

    check(status == FICANT_KERNEL_STATUS_OK, "I3-D-CPP-008 Following status");
    check(result.status_code == status, "I3-D-CPP-008 Following status sync");
    check(result.cashflow_count == 1, "I3-D-CPP-008 Following cashflow count");
    check(cashflows[0].sequence == 1, "I3-D-CPP-008 Following sequence");
    check(cashflows[0].nominal_date == ymd_to_days(2026, 7, 15),
          "I3-D-CPP-008 Following nominal date");
    check(cashflows[0].payment_date == ymd_to_days(2026, 7, 16),
          "I3-D-CPP-008 Following payment date");
    check(cashflows[0].coupon == 1.0, "I3-D-CPP-008 Following coupon");
    check(cashflows[0].principal == 100.0, "I3-D-CPP-008 Following principal");
    check(cashflows[0].total == 101.0, "I3-D-CPP-008 Following total");
    check_result(result, 0.0, 101.0, 101.0, 0.01,
                 0.0, 0.0, 0.0, 0.0,
                 "I3-D-CPP-008 Following analytics");
}

void test_isma_and_price_round_trip() {
    const auto bond = make_bond(
        ymd_to_days(2026, 6, 25), ymd_to_days(2028, 6, 25),
        0.0121, FICANT_KERNEL_FREQUENCY_ANNUAL);
    auto calc = make_calc(0.013);

    ficant_kernel_cashflow_v1 cashflows[2];
    ficant_kernel_result_v1 yield_result;
    uint32_t status = calculate_full(bond, calc, yield_result, cashflows, 2);

    check(status == FICANT_KERNEL_STATUS_OK, "I3-D-CPP-009 ISMA YIELD_IN status");
    check(yield_result.cashflow_count == 2, "I3-D-CPP-009 ISMA cashflow count");
    check(cashflows[0].sequence == 1 && cashflows[1].sequence == 2,
          "I3-D-CPP-009 ISMA 1-based sequences");
    check(cashflows[0].nominal_date == ymd_to_days(2027, 6, 25)
              && cashflows[0].payment_date == ymd_to_days(2027, 6, 25)
              && cashflows[0].coupon == 1.21
              && cashflows[0].principal == 0.0
              && cashflows[0].total == 1.21,
          "I3-D-CPP-009 ISMA first cashflow exact");
    check(cashflows[1].nominal_date == ymd_to_days(2028, 6, 25)
              && cashflows[1].payment_date == ymd_to_days(2028, 6, 26)
              && cashflows[1].coupon == 1.21
              && cashflows[1].principal == 100.0
              && cashflows[1].total == 101.21,
          "I3-D-CPP-009 ISMA final cashflow exact");
    check_result(yield_result,
                 0.062986301370, 99.827602841728, 99.890589143098,
                 0.013, 1.935979361140, 1.911134611194,
                 5.550565365752, 0.019090436584,
                 "I3-D-CPP-009 ISMA YIELD_IN analytics");

    calc.input_mode = FICANT_KERNEL_MODE_PRICE_IN;
    calc.input_value = 99.827602841728;
    ficant_kernel_result_v1 price_result;
    status = calculate_full(bond, calc, price_result, cashflows, 2);
    check(status == FICANT_KERNEL_STATUS_OK, "I3-D-CPP-009 direct PRICE_IN status");
    check_result(price_result,
                 0.062986301370, 99.827602841728, 99.890589143098,
                 0.013, 1.935979361140, 1.911134611194,
                 5.550565365752, 0.019090436584,
                 "I3-D-CPP-009 direct PRICE_IN analytics");

    calc.input_value = yield_result.clean_price;
    ficant_kernel_result_v1 round_trip_result;
    status = calculate_full(bond, calc, round_trip_result, cashflows, 2);
    check(status == FICANT_KERNEL_STATUS_OK, "I3-D-CPP-009 round trip status");
    check_abs(round_trip_result.yield_to_maturity, 0.013, YIELD_TOL,
              "I3-D-CPP-009 YIELD_IN to PRICE_IN round trip yield");
    check_abs(round_trip_result.clean_price, yield_result.clean_price, PRICE_TOL,
              "I3-D-CPP-009 YIELD_IN to PRICE_IN round trip price");
}

void test_discount_full_result() {
    const auto bond = make_bond(
        ymd_to_days(2026, 7, 14), ymd_to_days(2026, 12, 17),
        0.0, FICANT_KERNEL_FREQUENCY_ANNUAL);
    auto calc = make_calc(0.011);

    ficant_kernel_cashflow_v1 cashflows[1];
    ficant_kernel_result_v1 result;
    uint32_t status = calculate_full(bond, calc, result, cashflows, 1);
    check(status == FICANT_KERNEL_STATUS_OK, "I3-D-CPP-010 discount YIELD_IN status");
    check(result.cashflow_count == 1, "I3-D-CPP-010 discount cashflow count");
    check(cashflows[0].sequence == 1
              && cashflows[0].nominal_date == ymd_to_days(2026, 12, 17)
              && cashflows[0].payment_date == ymd_to_days(2026, 12, 17)
              && cashflows[0].coupon == 0.0
              && cashflows[0].principal == 100.0
              && cashflows[0].total == 100.0,
          "I3-D-CPP-010 discount cashflow exact");
    check_result(result,
                 0.0, 99.532062958802, 99.532062958802,
                 0.011, 0.427397260274, 0.425397310180,
                 0.361925743017, 0.004234067194,
                 "I3-D-CPP-010 discount YIELD_IN analytics");

    calc.input_mode = FICANT_KERNEL_MODE_PRICE_IN;
    calc.input_value = 99.532062958802;
    status = calculate_full(bond, calc, result, cashflows, 1);
    check(status == FICANT_KERNEL_STATUS_OK, "I3-D-CPP-010 discount PRICE_IN status");
    check_result(result,
                 0.0, 99.532062958802, 99.532062958802,
                 0.011, 0.427397260274, 0.425397310180,
                 0.361925743017, 0.004234067194,
                 "I3-D-CPP-010 discount PRICE_IN analytics");
}

void test_semiannual_all_cashflows_and_risk() {
    const auto bond = make_bond(
        ymd_to_days(2026, 5, 15), ymd_to_days(2036, 5, 15),
        0.0172, FICANT_KERNEL_FREQUENCY_SEMIANNUAL);
    const auto calc = make_calc(0.018);

    ficant_kernel_cashflow_v1 cashflows[20];
    ficant_kernel_result_v1 result;
    const uint32_t status = calculate_full(bond, calc, result, cashflows, 20);
    check(status == FICANT_KERNEL_STATUS_OK, "I3-D-CPP-011 semiannual status");
    check(result.cashflow_count == 20, "I3-D-CPP-011 semiannual cashflow count");

    struct ExpectedDate { int y; unsigned m; unsigned d; };
    constexpr ExpectedDate nominal_dates[20] = {
        {2026, 11, 15}, {2027, 5, 15}, {2027, 11, 15}, {2028, 5, 15},
        {2028, 11, 15}, {2029, 5, 15}, {2029, 11, 15}, {2030, 5, 15},
        {2030, 11, 15}, {2031, 5, 15}, {2031, 11, 15}, {2032, 5, 15},
        {2032, 11, 15}, {2033, 5, 15}, {2033, 11, 15}, {2034, 5, 15},
        {2034, 11, 15}, {2035, 5, 15}, {2035, 11, 15}, {2036, 5, 15},
    };
    constexpr ExpectedDate payment_dates[20] = {
        {2026, 11, 16}, {2027, 5, 17}, {2027, 11, 15}, {2028, 5, 15},
        {2028, 11, 15}, {2029, 5, 15}, {2029, 11, 15}, {2030, 5, 15},
        {2030, 11, 15}, {2031, 5, 15}, {2031, 11, 17}, {2032, 5, 17},
        {2032, 11, 15}, {2033, 5, 16}, {2033, 11, 15}, {2034, 5, 15},
        {2034, 11, 15}, {2035, 5, 15}, {2035, 11, 15}, {2036, 5, 15},
    };

    for (uint32_t i = 0; i < 20; ++i) {
        check(cashflows[i].sequence == i + 1,
              "I3-D-CPP-011 every eligible sequence is 1-based");
        check(cashflows[i].nominal_date == ymd_to_days(
                  nominal_dates[i].y, nominal_dates[i].m, nominal_dates[i].d),
              "I3-D-CPP-011 every nominal date exact");
        check(cashflows[i].payment_date == ymd_to_days(
                  payment_dates[i].y, payment_dates[i].m, payment_dates[i].d),
              "I3-D-CPP-011 every payment date exact");
        check(cashflows[i].coupon == 0.86,
              "I3-D-CPP-011 every coupon exact");
        check(cashflows[i].principal == (i == 19 ? 100.0 : 0.0),
              "I3-D-CPP-011 every principal exact");
        check(cashflows[i].total == (i == 19 ? 100.86 : 0.86),
              "I3-D-CPP-011 every total exact");
    }
    check_result(result,
                 0.280434782609, 99.280882361317, 99.561317143926,
                 0.018, 9.063340749492, 8.982498265106,
                 89.558450732468, 0.089430951597,
                 "I3-D-CPP-011 semiannual analytics");
}

void test_error_matrix() {
    const auto valid_bond = make_bond(
        ymd_to_days(2026, 5, 15), ymd_to_days(2028, 5, 15),
        0.013, FICANT_KERNEL_FREQUENCY_ANNUAL);
    const auto valid_calc = make_calc(0.013);

    check_status_sync(nullptr, &valid_calc, nullptr, 0,
                      FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
                      "I3-D-CPP-008 null bond status sync");
    check_status_sync(&valid_bond, nullptr, nullptr, 0,
                      FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
                      "I3-D-CPP-008 null calc status sync");
    check(ficant_kernel_calculate_bond_v1(
              &valid_bond, &valid_calc, nullptr, nullptr, 0)
              == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
          "I3-D-CPP-008 null result is rejected without a write");

    ficant_kernel_bond_input_v1 bad_bonds[10];
    const char* bond_labels[10] = {
        "bond size", "bond ABI", "bond dates", "frequency enum", "day count enum",
        "business day enum", "negative coupon", "nonfinite coupon", "zero face",
        "nonfinite face",
    };
    for (auto& bond : bad_bonds) bond = valid_bond;
    bad_bonds[0].struct_size -= 1;
    bad_bonds[1].abi_version += 1;
    bad_bonds[2].maturity_date = bad_bonds[2].issue_date;
    bad_bonds[3].frequency = 3;
    bad_bonds[4].day_count_convention = 2;
    bad_bonds[5].business_day_convention = 2;
    bad_bonds[6].coupon_rate = -0.01;
    bad_bonds[7].coupon_rate = std::numeric_limits<double>::quiet_NaN();
    bad_bonds[8].face_value = 0.0;
    bad_bonds[9].face_value = std::numeric_limits<double>::infinity();
    for (uint32_t i = 0; i < 10; ++i) {
        const uint32_t expected = i == 1
            ? FICANT_KERNEL_STATUS_ABI_MISMATCH
            : FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
        check_status_sync(&bad_bonds[i], &valid_calc, nullptr, 0,
                          expected, bond_labels[i]);
    }

    int32_t unsorted_non_business[] = {
        ymd_to_days(2026, 7, 16), ymd_to_days(2026, 7, 15)};
    int32_t duplicate_work_weekends[] = {
        ymd_to_days(2026, 7, 18), ymd_to_days(2026, 7, 18)};
    ficant_kernel_calculate_input_v1 bad_calcs[8];
    const char* calc_labels[8] = {
        "calc size", "calc ABI", "input mode enum", "nonfinite input",
        "calendar enum", "calendar coverage", "unsorted holidays", "duplicate work weekends",
    };
    for (auto& calc : bad_calcs) calc = valid_calc;
    bad_calcs[0].struct_size -= 1;
    bad_calcs[1].abi_version += 1;
    bad_calcs[2].input_mode = 3;
    bad_calcs[3].input_value = std::numeric_limits<double>::quiet_NaN();
    bad_calcs[4].calendar_requirement = 3;
    bad_calcs[5].calendar_coverage_start = 1;
    bad_calcs[5].calendar_coverage_end = 0;
    bad_calcs[6].calendar_coverage_start = ymd_to_days(2026, 1, 1);
    bad_calcs[6].calendar_coverage_end = ymd_to_days(2028, 12, 31);
    bad_calcs[6].non_business_days = unsorted_non_business;
    bad_calcs[6].non_business_days_count = 2;
    bad_calcs[7].calendar_coverage_start = ymd_to_days(2026, 1, 1);
    bad_calcs[7].calendar_coverage_end = ymd_to_days(2028, 12, 31);
    bad_calcs[7].work_weekends = duplicate_work_weekends;
    bad_calcs[7].work_weekends_count = 2;
    for (uint32_t i = 0; i < 8; ++i) {
        const uint32_t expected = i == 1
            ? FICANT_KERNEL_STATUS_ABI_MISMATCH
            : FICANT_KERNEL_STATUS_INVALID_ARGUMENT;
        check_status_sync(&valid_bond, &bad_calcs[i], nullptr, 0,
                          expected, calc_labels[i]);
    }

    auto exact_calc = valid_calc;
    exact_calc.calendar_requirement = FICANT_KERNEL_CALENDAR_REQUIREMENT_EXACT_MARKET;
    exact_calc.calendar_coverage_start = ymd_to_days(2026, 7, 14);
    exact_calc.calendar_coverage_end = ymd_to_days(2026, 7, 14);
    check_status_sync(&valid_bond, &exact_calc, nullptr, 0,
                      FICANT_KERNEL_STATUS_CALENDAR_COVERAGE_MISSING,
                      "I3-D-CPP-008 calendar coverage status sync");

    ficant_kernel_cashflow_v1 cashflows[2];
    init_cashflows(cashflows, 2);
    ficant_kernel_result_v1 sizing_result;
    init_result(sizing_result);
    uint32_t status = ficant_kernel_calculate_bond_v1(
        &valid_bond, &valid_calc, &sizing_result, nullptr, 0);
    check(status == FICANT_KERNEL_STATUS_BUFFER_TOO_SMALL,
          "I3-D-CPP-008 sizing status");
    check(sizing_result.status_code == status && sizing_result.cashflow_count == 2,
          "I3-D-CPP-008 sizing status sync and exact count");

    check_status_sync(&valid_bond, &valid_calc, nullptr, 2,
                      FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
                      "I3-D-CPP-008 null buffer with capacity status sync");
    check_status_sync(&valid_bond, &valid_calc, cashflows, 0,
                      FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
                      "I3-D-CPP-008 buffer with zero capacity status sync");
    check_status_sync(&valid_bond, &valid_calc, cashflows, 1,
                      FICANT_KERNEL_STATUS_BUFFER_TOO_SMALL,
                      "I3-D-CPP-008 undersized buffer status sync");

    init_cashflows(cashflows, 2);
    cashflows[1].struct_size -= 1;
    check_status_sync(&valid_bond, &valid_calc, cashflows, 2,
                      FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
                      "I3-D-CPP-008 later cashflow size status sync");
    init_cashflows(cashflows, 2);
    cashflows[1].abi_version += 1;
    check_status_sync(&valid_bond, &valid_calc, cashflows, 2,
                      FICANT_KERNEL_STATUS_ABI_MISMATCH,
                      "I3-D-CPP-008 later cashflow ABI status sync");

    auto no_bracket_calc = valid_calc;
    no_bracket_calc.input_mode = FICANT_KERNEL_MODE_PRICE_IN;
    no_bracket_calc.input_value = 1e300;
    init_cashflows(cashflows, 2);
    check_status_sync(&valid_bond, &no_bracket_calc, cashflows, 2,
                      FICANT_KERNEL_STATUS_NO_BRACKET,
                      "I3-D-CPP-008 numerical no bracket status sync");

    const auto discount_bond = make_bond(
        ymd_to_days(2026, 7, 14), ymd_to_days(2026, 12, 17),
        0.0, FICANT_KERNEL_FREQUENCY_ANNUAL);
    auto nonfinite_calc = make_calc(0.0);
    nonfinite_calc.input_mode = FICANT_KERNEL_MODE_PRICE_IN;
    ficant_kernel_cashflow_v1 discount_cashflow[1];
    init_cashflows(discount_cashflow, 1);
    check_status_sync(&discount_bond, &nonfinite_calc, discount_cashflow, 1,
                      FICANT_KERNEL_STATUS_NON_FINITE,
                      "I3-D-CPP-008 numerical nonfinite status sync");

    ficant_kernel_result_v1 invalid_result;
    init_result(invalid_result);
    invalid_result.struct_size -= 1;
    invalid_result.status_code = UINT32_C(0xDEADBEEF);
    status = ficant_kernel_calculate_bond_v1(
        &valid_bond, &valid_calc, &invalid_result, nullptr, 0);
    check(status == FICANT_KERNEL_STATUS_INVALID_ARGUMENT,
          "I3-D-CPP-008 invalid result size status");
    check(invalid_result.status_code == UINT32_C(0xDEADBEEF),
          "I3-D-CPP-008 invalid result size is not written");

    init_result(invalid_result);
    invalid_result.abi_version += 1;
    invalid_result.status_code = UINT32_C(0xDEADBEEF);
    status = ficant_kernel_calculate_bond_v1(
        &valid_bond, &valid_calc, &invalid_result, nullptr, 0);
    check(status == FICANT_KERNEL_STATUS_ABI_MISMATCH,
          "I3-D-CPP-008 invalid result ABI status");
    check(invalid_result.status_code == UINT32_C(0xDEADBEEF),
          "I3-D-CPP-008 invalid result ABI is not written");
}

} // namespace

int main() {
    test_gregorian_january_february_round_trip();
    test_following_exact_result();
    test_isma_and_price_round_trip();
    test_discount_full_result();
    test_semiannual_all_cashflows_and_risk();
    test_error_matrix();

    if (failures != 0) {
        std::fprintf(stderr, "%d failures\n", failures);
        return 1;
    }
    return 0;
}
