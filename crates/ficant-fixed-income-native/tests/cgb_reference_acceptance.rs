use std::fs;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::BondAnalyticsEngine;
use ficant_domain::analytics::{
    ABI_VERSION, ALGORITHM_ID, ALGORITHM_VERSION, AnalyticsMode, AnalyticsObjectRef,
    BondAnalyticsInput, BondTerms, BusinessDayConvention, CONVENTION_PROFILE, CalendarBinding,
    CalendarRequirement, CalendarResolution, CouponFrequency, DayCountConvention, ENGINE_ID,
    ENGINE_VERSION, FixedDecimal, RESULT_SCHEMA_ID,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeBondAnalyticsEngine;
use serde_json::Value;

const SCALE: i128 = 1_000_000_000_000;
const PRICE_TOLERANCE: i128 = 10_000;
const YIELD_TOLERANCE: i128 = 100;
const RISK_ABS_FLOOR: i128 = 100;
const DV01_TOLERANCE: i128 = 10_000;

#[test]
#[allow(clippy::too_many_lines)]
fn q001_q023_production_native_matches_frozen_reference_cases() {
    let root = repository_root();
    let expected: Value = read_json(
        &root.join("tests/golden-cases/china-rates/expected/cgb-reference-v1-expected.json"),
    );
    let results = expected["results"]
        .as_object()
        .expect("frozen expected results must be an object");

    assert_eq!(results.len(), 12, "Q-001..Q-012 require twelve cases");
    for (case_id, expected_result) in results {
        let (bond_id, mode) = case_id
            .split_once(':')
            .expect("case identity must contain bond and mode");
        let fixture: Value = read_json(&root.join(format!(
            "tests/golden-cases/china-rates/fixtures/bond-{bond_id}.json"
        )));
        let input = input(&fixture, expected_result, mode);
        let actual = NativeBondAnalyticsEngine
            .calculate(&input)
            .unwrap_or_else(|error| panic!("{case_id} production calculation failed: {error:?}"));

        assert_eq!(actual.schema_id(), RESULT_SCHEMA_ID, "{case_id} schema");
        assert_eq!(actual.engine_id(), ENGINE_ID, "{case_id} engine");
        assert_eq!(
            actual.engine_version(),
            ENGINE_VERSION,
            "{case_id} engine version"
        );
        assert_eq!(actual.algorithm_id(), ALGORITHM_ID, "{case_id} algorithm");
        assert_eq!(
            actual.algorithm_version(),
            ALGORITHM_VERSION,
            "{case_id} algorithm version"
        );
        assert_eq!(
            actual.convention_profile(),
            CONVENTION_PROFILE,
            "{case_id} convention"
        );
        assert_eq!(actual.abi_version(), ABI_VERSION, "{case_id} ABI");
        let expected_resolution = expected_result["identity"]["calendar_resolution"]
            .as_str()
            .expect("calendar resolution must be a string");
        assert_eq!(
            actual.calendar_resolution(),
            match expected_resolution {
                "EXACT" => CalendarResolution::Exact,
                "PROVISIONAL_WEEKEND_ONLY" => CalendarResolution::ProvisionalWeekendOnly,
                other => panic!("unsupported calendar resolution {other}"),
            },
            "{case_id} calendar resolution"
        );

        let expected_cashflows = expected_result["cashflows"]
            .as_array()
            .expect("cashflows must be an array");
        assert_eq!(
            actual.cashflows().len(),
            expected_cashflows.len(),
            "{case_id} cashflow count"
        );
        for (actual_flow, expected_flow) in actual.cashflows().iter().zip(expected_cashflows) {
            assert_eq!(
                actual_flow.sequence(),
                u32_value(expected_flow, "sequence"),
                "{case_id} sequence"
            );
            assert_eq!(
                actual_flow.nominal_date(),
                date_value(expected_flow, "nominal_date"),
                "{case_id} nominal date"
            );
            assert_eq!(
                actual_flow.payment_date(),
                date_value(expected_flow, "payment_date"),
                "{case_id} payment date"
            );
            assert_eq!(
                actual_flow.coupon().scaled(),
                decimal_value(expected_flow, "coupon"),
                "{case_id} coupon"
            );
            assert_eq!(
                actual_flow.principal().scaled(),
                decimal_value(expected_flow, "principal"),
                "{case_id} principal"
            );
            assert_eq!(
                actual_flow.total().scaled(),
                decimal_value(expected_flow, "total"),
                "{case_id} total"
            );
            assert_eq!(
                actual_flow.total().scaled(),
                actual_flow.coupon().scaled() + actual_flow.principal().scaled(),
                "{case_id} cashflow component identity"
            );
        }

        let measures = actual.measures();
        assert_abs(
            measures.accrued_interest().scaled(),
            decimal_value(expected_result, "accrued_interest"),
            PRICE_TOLERANCE,
            case_id,
            "accrued_interest",
        );
        assert_abs(
            measures.clean_price().scaled(),
            decimal_value(expected_result, "clean_price"),
            PRICE_TOLERANCE,
            case_id,
            "clean_price",
        );
        assert_abs(
            measures.dirty_price().scaled(),
            decimal_value(expected_result, "dirty_price"),
            PRICE_TOLERANCE,
            case_id,
            "dirty_price",
        );
        assert_abs(
            measures.yield_to_maturity().scaled(),
            decimal_value(expected_result, "yield_to_maturity"),
            YIELD_TOLERANCE,
            case_id,
            "yield_to_maturity",
        );
        for (field, actual_value) in [
            ("macaulay_duration", measures.macaulay_duration().scaled()),
            ("modified_duration", measures.modified_duration().scaled()),
            ("convexity", measures.convexity().scaled()),
        ] {
            let expected_value = decimal_value(expected_result, field);
            let relative = i128::try_from(expected_value.unsigned_abs().div_ceil(100_000_000))
                .expect("frozen risk tolerance must fit i128");
            assert_abs(
                actual_value,
                expected_value,
                relative.max(RISK_ABS_FLOOR),
                case_id,
                field,
            );
        }
        assert_abs(
            measures.dv01().scaled(),
            decimal_value(expected_result, "dv01"),
            DV01_TOLERANCE,
            case_id,
            "dv01",
        );
        assert_eq!(
            measures.dirty_price().scaled(),
            measures.clean_price().scaled() + measures.accrued_interest().scaled(),
            "{case_id} dirty price identity"
        );
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate must be two levels below repository root")
        .to_path_buf()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(
        &fs::read(path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn input(fixture: &Value, expected: &Value, mode: &str) -> BondAnalyticsInput {
    let version = Version::new(1).expect("version one is valid");
    let object = |suffix, byte| {
        AnalyticsObjectRef::new(
            VersionRef::new(id(suffix), version),
            ContentHash::from_bytes(&[byte; 32]).expect("test hash is valid"),
        )
    };
    let valuation_at = Utc
        .with_ymd_and_hms(2026, 7, 13, 7, 0, 0)
        .single()
        .expect("valuation instant is valid");
    let valuation_at = MarketTime::new(
        valuation_at,
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 7, 13).expect("market date is valid"),
    )
    .expect("market time is valid");
    let calendar = CalendarBinding::new(
        "cgb-reference-calendar-v1",
        version,
        ContentHash::from_bytes(&[4; 32]).expect("test hash is valid"),
        NaiveDate::from_ymd_opt(2005, 1, 1).expect("coverage start is valid"),
        NaiveDate::from_ymd_opt(2026, 12, 31).expect("coverage end is valid"),
        dates(&[
            "2026-01-01",
            "2026-01-02",
            "2026-02-16",
            "2026-02-17",
            "2026-02-18",
            "2026-02-19",
            "2026-02-20",
            "2026-04-06",
            "2026-05-01",
            "2026-05-04",
            "2026-05-05",
            "2026-06-22",
            "2026-09-25",
            "2026-10-01",
            "2026-10-02",
            "2026-10-05",
            "2026-10-06",
            "2026-10-07",
            "2026-10-08",
        ]),
        dates(&["2026-02-14", "2026-10-10"]),
    )
    .expect("reference calendar is valid");
    let frequency = match fixture["frequency"]
        .as_u64()
        .expect("frequency must be numeric")
    {
        0 | 1 => CouponFrequency::Annual,
        2 => CouponFrequency::Semiannual,
        other => panic!("unsupported coupon frequency {other}"),
    };
    let terms = BondTerms::new(
        date_value(fixture, "issue_date"),
        date_value(fixture, "maturity_date"),
        frequency,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        FixedDecimal::from_scaled(decimal_value(fixture, "coupon_rate_decimal")),
        FixedDecimal::from_scaled(decimal_value(fixture, "face_value")),
    )
    .expect("bond terms are valid");
    BondAnalyticsInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C', 1),
        object('D', 2),
        object('E', 3),
        valuation_at,
        date_value(expected, "settlement_date"),
        CalendarRequirement::ReferenceReplay,
        calendar,
        terms,
        match mode {
            "YIELD_IN" => AnalyticsMode::YieldIn,
            "PRICE_IN" => AnalyticsMode::PriceIn,
            other => panic!("unsupported analytics mode {other}"),
        },
        FixedDecimal::from_scaled(decimal_value(expected, "input_value")),
    )
    .expect("analytics input is valid")
}

fn dates(values: &[&str]) -> Vec<NaiveDate> {
    values
        .iter()
        .map(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date literal is valid"))
        .collect()
}

fn date_value(value: &Value, field: &str) -> NaiveDate {
    NaiveDate::parse_from_str(
        value[field]
            .as_str()
            .unwrap_or_else(|| panic!("{field} must be a string")),
        "%Y-%m-%d",
    )
    .unwrap_or_else(|error| panic!("{field} must be a date: {error}"))
}

fn decimal_value(value: &Value, field: &str) -> i128 {
    let literal = value[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field} must be a decimal string"));
    let (whole, fraction) = literal
        .split_once('.')
        .unwrap_or_else(|| panic!("{field} must have twelve decimal places"));
    assert_eq!(
        fraction.len(),
        12,
        "{field} must have twelve decimal places"
    );
    let negative = whole.starts_with('-');
    let whole_abs = whole
        .trim_start_matches('-')
        .parse::<i128>()
        .expect("whole part");
    let fraction = fraction.parse::<i128>().expect("fractional part");
    let scaled = whole_abs * SCALE + fraction;
    if negative { -scaled } else { scaled }
}

fn u32_value(value: &Value, field: &str) -> u32 {
    u32::try_from(
        value[field]
            .as_u64()
            .unwrap_or_else(|| panic!("{field} must be numeric")),
    )
    .expect("value must fit u32")
}

fn assert_abs(actual: i128, expected: i128, tolerance: i128, case_id: &str, field: &str) {
    let difference = actual.abs_diff(expected);
    assert!(
        difference <= tolerance.unsigned_abs(),
        "{case_id} {field}: actual={actual} expected={expected} difference={difference} tolerance={tolerance}"
    );
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).expect("test ULID is valid")
}
