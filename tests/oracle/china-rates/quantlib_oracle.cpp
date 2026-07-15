#include <ql/quantlib.hpp>

#include <iomanip>
#include <iostream>
#include <algorithm>
#include <cmath>
#include <memory>
#include <string>
#include <vector>

using namespace QuantLib;

struct CaseInput {
    const char* id;
    Date issue;
    Date maturity;
    Real coupon;
    Frequency frequency;
    Rate yield;
};

struct CashflowIdentity {
    Size sequence;
    Date nominal_date;
    Date payment_date;
    Real coupon;
    Real principal;
    Time time_years;
};

struct Metrics {
    Real clean;
    Real dirty;
    Real accrued;
    Rate yield;
    Time macaulay;
    Time modified;
    Real convexity;
    Real dv01;
    Real price_down;
    Real price_up;
    Real finite_dv01;
    Real finite_dv01_relative_difference;
    Real finite_convexity;
    Real finite_convexity_relative_difference;
    std::vector<CashflowIdentity> cashflows;
};

static const Date kCalendarCoverageStart(1, January, 2005);
static const Date kCalendarCoverageEnd(31, December, 2026);

static Metrics calculate(const CaseInput& input, const Date& settlement) {
    WeekendsOnly calendar;
    std::shared_ptr<Bond> bond;
    DayCounter day_counter;
    Compounding compounding;

    if (input.frequency == NoFrequency) {
        day_counter = ActualActual(ActualActual::ISDA);
        compounding = Simple;
        bond = std::make_shared<ZeroCouponBond>(
            0, calendar, 100.0, input.maturity, Following, 100.0, input.issue);
    } else {
        Schedule schedule(
            input.issue, input.maturity, Period(input.frequency), calendar,
            Unadjusted, Unadjusted, DateGeneration::Backward, false);
        day_counter = ActualActual(ActualActual::Bond, schedule);
        compounding = Compounded;
        bond = std::make_shared<FixedRateBond>(
            0, 100.0, schedule, std::vector<Rate>{input.coupon}, day_counter,
            Following, 100.0, input.issue, calendar);
    }

    const Frequency rate_frequency = input.frequency == NoFrequency ? Annual : input.frequency;
    const InterestRate rate(input.yield, day_counter, compounding, rate_frequency);
    const Leg& payment_leg = bond->cashflows();
    Leg valuation_leg;
    for (const auto& flow : payment_leg) {
        if (flow->date() <= settlement) continue;
        const auto coupon = std::dynamic_pointer_cast<Coupon>(flow);
        const Date nominal_date = coupon ? coupon->accrualEndDate() : input.maturity;
        valuation_leg.push_back(std::make_shared<SimpleCashFlow>(
            flow->amount(), std::max(nominal_date, settlement)));
    }
    const Duration::Type duration_type =
        input.frequency == NoFrequency ? Duration::Simple : Duration::Macaulay;
    const Real dirty = CashFlows::npv(
        valuation_leg, rate, true, settlement, settlement);
    const Real accrued = bond->accruedAmount(settlement);
    const Real clean = dirty - accrued;
    const Rate solved = CashFlows::yield(
        valuation_leg, dirty, day_counter, compounding, rate_frequency, true,
        settlement, settlement, 1.0e-12, 100, input.yield);
    const Time macaulay = CashFlows::duration(
        valuation_leg, rate, duration_type, true, settlement, settlement);
    const Time modified = CashFlows::duration(
        valuation_leg, rate, Duration::Modified, true, settlement, settlement);
    const Real convexity = CashFlows::convexity(
        valuation_leg, rate, true, settlement, settlement);
    const InterestRate up(input.yield + 0.0001, day_counter, compounding, rate_frequency);
    const InterestRate down(input.yield - 0.0001, day_counter, compounding, rate_frequency);
    const Real price_up = CashFlows::npv(
        valuation_leg, up, true, settlement, settlement);
    const Real price_down = CashFlows::npv(
        valuation_leg, down, true, settlement, settlement);
    const Real analytic_dv01 = modified * dirty * 0.0001;
    const Real finite_dv01 = std::abs(price_down - price_up) / 2.0;
    const Real finite_convexity =
        (price_down + price_up - 2.0 * dirty) / (dirty * 0.0001 * 0.0001);
    const Real finite_dv01_relative_difference =
        std::abs(finite_dv01 - analytic_dv01)
        / std::max(std::abs(analytic_dv01), 1.0e-30);
    const Real finite_convexity_relative_difference =
        std::abs(finite_convexity - convexity)
        / std::max(std::abs(convexity), 1.0e-30);

    std::vector<CashflowIdentity> rows;
    for (const auto& flow : payment_leg) {
        if (flow->date() <= settlement) continue;
        const auto coupon = std::dynamic_pointer_cast<Coupon>(flow);
        const Date nominal_date = coupon ? coupon->accrualEndDate() : input.maturity;
        const Real coupon_amount = coupon ? flow->amount() : 0.0;
        const Real principal_amount = coupon ? 0.0 : flow->amount();
        if (!rows.empty()
            && rows.back().nominal_date == nominal_date
            && rows.back().payment_date == flow->date()) {
            rows.back().coupon += coupon_amount;
            rows.back().principal += principal_amount;
        } else {
            rows.push_back({
                rows.size() + 1, nominal_date, flow->date(), coupon_amount, principal_amount,
                day_counter.yearFraction(settlement, nominal_date),
            });
        }
    }
    return {clean, dirty, accrued, solved, macaulay, modified, convexity,
            finite_dv01, price_down, price_up, finite_dv01,
            finite_dv01_relative_difference, finite_convexity,
            finite_convexity_relative_difference, rows};
}

static void print_cashflow_semantics() {
    std::cout
        << "{\"sequence\":\"one_based_ascending_payment_eligible_cashflow\""
        << ",\"component_identity\":\"ordered_subset_of_coupon_then_principal\""
        << ",\"coupon\":\"coupon_amount_for_nominal_accrual_period\""
        << ",\"principal\":\"face_redemption_amount\""
        << ",\"total\":\"coupon_plus_principal\""
        << ",\"unit\":\"CNY_PER_100_FACE\"}";
}

static bool uses_provisional_calendar(const Metrics& metrics, const Date& settlement) {
    if (settlement < kCalendarCoverageStart || settlement > kCalendarCoverageEnd) return true;
    for (const auto& flow : metrics.cashflows) {
        if (flow.nominal_date < kCalendarCoverageStart || flow.nominal_date > kCalendarCoverageEnd
            || flow.payment_date < kCalendarCoverageStart || flow.payment_date > kCalendarCoverageEnd) {
            return true;
        }
    }
    return false;
}

static void print_case_identity(
    const CaseInput& input, const char* mode, const Metrics& metrics, const Date& settlement) {
    std::cout
        << "{\"bond_id\":\"" << input.id << "\""
        << ",\"mode\":\"" << mode << "\""
        << ",\"result_schema\":\"ficant.bond-analytics.result.v1\""
        << ",\"engine\":\"ficant-fixed-income-native/0.1.0\""
        << ",\"algorithm\":\"ficant.cgb.fixed-rate.reference/1\""
        << ",\"abi\":\"FICANT_FIXED_INCOME_ABI_V1=1\""
        << ",\"convention\":\"cgb-reference-v1\""
        << ",\"calendar_id\":\"cgb-reference-calendar-v1\""
        << ",\"calendar_requirement\":\"REFERENCE_REPLAY\""
        << ",\"calendar_resolution\":\""
        << (uses_provisional_calendar(metrics, settlement) ? "PROVISIONAL_WEEKEND_ONLY" : "EXACT")
        << "\""
        << ",\"calendar_coverage\":\"2005-01-01..2026-12-31\""
        << ",\"market_timezone\":\"Asia/Shanghai\""
        << ",\"valuation_at\":\"2026-07-13T15:00:00+08:00\""
        << ",\"settlement_date\":\"" << io::iso_date(settlement) << "\""
        << ",\"rule_pack_status\":\"pending_production_proof\""
        << ",\"rule_pack_content_sha256\":null"
        << ",\"snapshot_status\":\"source_manifest_only_no_production_snapshot_proof\""
        << ",\"snapshot_source_object_sha256\":\"765d8afe8605562dbf1c4d2a23513de25e98945496f8d297565c1d943eed8faf\""
        << ",\"snapshot_production_content_sha256\":null}";
}

static void print_case(
    const CaseInput& input,
    const char* mode,
    const Metrics& metrics,
    const Date& settlement,
    bool& first) {
    if (!first) std::cout << ",\n";
    first = false;
    const Real input_value = std::string(mode) == "YIELD_IN" ? input.yield : metrics.clean;
    std::cout << "    \"" << input.id << ":" << mode << "\": {"
              << "\"bond_id\":\"" << input.id << "\""
              << ",\"mode\":\"" << mode << "\""
              << ",\"input_value\":" << input_value
              << ",\"settlement_date\":\"" << io::iso_date(settlement) << "\""
              << ",\"identity\":";
    print_case_identity(input, mode, metrics, settlement);
    std::cout << ",\"cashflow_semantics\":";
    print_cashflow_semantics();
    std::cout << ",\"cashflow_count\":" << metrics.cashflows.size()
              << ",\"cashflows\":[";
    bool first_flow = true;
    for (const auto& flow : metrics.cashflows) {
        if (!first_flow) std::cout << ",";
        first_flow = false;
        std::cout << "{\"sequence\":" << flow.sequence
                  << ",\"nominal_date\":\"" << io::iso_date(flow.nominal_date) << "\""
                  << ",\"payment_date\":\"" << io::iso_date(flow.payment_date) << "\""
                  << ",\"components\":[";
        bool has_component = false;
        if (flow.coupon != 0.0) {
            std::cout << "\"coupon\"";
            has_component = true;
        }
        if (flow.principal != 0.0) {
            if (has_component) std::cout << ",";
            std::cout << "\"principal\"";
        }
        std::cout << "]"
                  << ",\"coupon\":" << flow.coupon
                  << ",\"principal\":" << flow.principal
                  << ",\"total\":" << flow.coupon + flow.principal
                  << ",\"time_years\":" << flow.time_years << "}";
    }
    std::cout << "]"
              << ",\"accrued_interest\":" << metrics.accrued
              << ",\"clean_price\":" << metrics.clean
              << ",\"dirty_price\":" << metrics.dirty
              << ",\"yield_to_maturity\":" << metrics.yield
              << ",\"macaulay_duration\":" << metrics.macaulay
              << ",\"modified_duration\":" << metrics.modified
              << ",\"convexity\":" << metrics.convexity
              << ",\"dv01\":" << metrics.dv01
              << ",\"round_trip\":{\"yield_to_maturity\":" << metrics.yield
              << ",\"absolute_difference\":" << std::abs(metrics.yield - input.yield) << "}"
              << ",\"finite_difference\":{\"bump_decimal\":0.0001"
              << ",\"price_minus_1bp\":" << metrics.price_down
              << ",\"price_plus_1bp\":" << metrics.price_up
              << ",\"dv01\":" << metrics.finite_dv01
              << ",\"dv01_relative_difference\":" << metrics.finite_dv01_relative_difference
              << ",\"convexity\":" << metrics.finite_convexity
              << ",\"convexity_relative_difference\":"
              << metrics.finite_convexity_relative_difference << "}"
              << ",\"units\":{"
              << "\"price_accrued_dv01\":\"CNY_PER_100_FACE\""
              << ",\"cashflow\":\"CNY_PER_100_FACE\""
              << ",\"yield\":\"DECIMAL_RATE\""
              << ",\"duration\":\"YEARS\""
              << ",\"convexity\":\"YEARS_SQUARED\"}}";
}

static void print_oracle_identity() {
    std::cout
        << "{\"role\":\"frozen_target_contract_identity_not_oracle_execution_claim\""
        << ",\"result_schema\":\"ficant.bond-analytics.result.v1\""
        << ",\"engine\":\"ficant-fixed-income-native/0.1.0\""
        << ",\"algorithm\":\"ficant.cgb.fixed-rate.reference/1\""
        << ",\"abi\":\"FICANT_FIXED_INCOME_ABI_V1=1\""
        << ",\"calendar\":{\"id\":\"cgb-reference-calendar-v1\""
        << ",\"requirement\":\"REFERENCE_REPLAY\""
        << ",\"resolution_scope\":\"per_result\""
        << ",\"resolution_policy\":{\"exact_if\":\"all_required_dates_inside_frozen_exact_coverage\""
        << ",\"exact_resolution\":\"EXACT\""
        << ",\"otherwise_resolution\":\"PROVISIONAL_WEEKEND_ONLY\"}"
        << ",\"coverage\":\"2005-01-01..2026-12-31\"}"
        << ",\"rule_pack\":{\"status\":\"pending_production_proof\""
        << ",\"id\":null,\"version\":null,\"content_sha256\":null}"
        << ",\"snapshot\":{\"status\":\"source_manifest_only_no_production_snapshot_proof\""
        << ",\"source_manifest\":\"tests/golden-cases/china-rates/iteration-3-cgb-basic-info-source-manifest.json\""
        << ",\"source_manifest_sha256\":\"078c14aaa67bc3d819d0a089e415d13029e09d88d43d0946dbdf10e7e8221dd1\""
        << ",\"source_object_sha256\":\"765d8afe8605562dbf1c4d2a23513de25e98945496f8d297565c1d943eed8faf\""
        << ",\"canonical_records_sha256\":\"8216f586cbec959a08bb62a5e00c2492c99dc01e641e0c876a918b710e9d50ff\""
        << ",\"production_id\":null,\"production_version\":null"
        << ",\"production_content_sha256\":null}}";
}

int main() {
    Settings::instance().evaluationDate() = Date(13, July, 2026);
    const Date settlement(14, July, 2026);
    const std::vector<CaseInput> cases = {
        {"269937.IB", Date(18, June, 2026), Date(17, December, 2026), 0.0, NoFrequency, 0.0110},
        {"260013.IB", Date(25, June, 2026), Date(25, June, 2028), 0.0121, Annual, 0.0130},
        {"260011.IB", Date(25, May, 2026), Date(25, May, 2029), 0.0126, Annual, 0.0138},
        {"260008.IB", Date(15, April, 2026), Date(15, April, 2031), 0.0150, Annual, 0.0155},
        {"260012.IB", Date(15, June, 2026), Date(15, June, 2033), 0.0156, Annual, 0.0165},
        {"260010.IB", Date(15, May, 2026), Date(15, May, 2036), 0.0172, Semiannual, 0.0180},
    };

    std::cout << std::setprecision(17);
    std::cout << "{\n  \"schema\":\"ficant.test-oracle.quantlib-output.v2\""
              << ",\n  \"quantlib_version\":\"" << QL_VERSION << "\""
              << ",\n  \"case_count\":12"
              << ",\n  \"candidate_id\":\"I3-TW-ORACLE-REROUTE-1-R1\""
              << ",\n  \"convention\":\"cgb-reference-v1\""
              << ",\n  \"calendar\":\"cgb-reference-calendar-v1\""
              << ",\n  \"market_timezone\":\"Asia/Shanghai\""
              << ",\n  \"valuation_at\":\"2026-07-13T15:00:00+08:00\""
              << ",\n  \"settlement_date\":\"2026-07-14\""
              << ",\n  \"oracle_identity\":";
    print_oracle_identity();
    std::cout << ",\n  \"cashflow_semantics\":";
    print_cashflow_semantics();
    std::cout << ",\n  \"results\": {\n";
    bool first = true;
    for (const auto& input : cases) {
        const Metrics metrics = calculate(input, settlement);
        print_case(input, "YIELD_IN", metrics, settlement, first);
        print_case(input, "PRICE_IN", metrics, settlement, first);
    }
    std::cout << "\n  }\n}\n";
    return 0;
}
