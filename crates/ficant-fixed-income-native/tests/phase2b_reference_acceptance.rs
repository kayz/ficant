use std::fs;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{CarryRollEngine, YieldCurveEngine};
use ficant_domain::analytics::{
    AnalyticsObjectRef, BondTerms, BusinessDayConvention, CalendarBinding, CalendarRequirement,
    CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::curves::{
    CARRY_ROLL_ALGORITHM_ID, CARRY_ROLL_ALGORITHM_VERSION, CARRY_ROLL_CONVENTION_PROFILE,
    CARRY_ROLL_RESULT_SCHEMA_ID, CURVE_ALGORITHM_ID, CURVE_ALGORITHM_VERSION,
    CURVE_CONVENTION_PROFILE, CURVE_RESULT_SCHEMA_ID, CarryRollInput, YieldCurveBinding,
    YieldCurveInterpolation, YieldCurveNode, YieldCurveQuery,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::{NativeCarryRollEngine, NativeYieldCurveEngine};
use serde_json::Value;

const YIELD_TOLERANCE: u128 = 2;
// Reuse the independently validated Phase 2A absolute price tolerance.
const VALUE_TOLERANCE: u128 = 10_000;

#[test]
fn production_curve_matches_phase2b_frozen_manual_oracle() {
    let (inputs, expected) = frozen_cases();
    let curve = curve(&inputs);
    for case in inputs["curve_cases"]
        .as_array()
        .expect("curve_cases must be an array")
    {
        let case_id = string(case, "id");
        let query = YieldCurveQuery::new(curve.clone(), date_value(case, "query_date"))
            .expect("frozen curve query must be valid");
        let actual = NativeYieldCurveEngine
            .interpolate(&query)
            .unwrap_or_else(|error| panic!("{case_id} production interpolation failed: {error:?}"));
        let reference = &expected["curve_results"][case_id];
        assert_abs(
            actual.yield_to_maturity(),
            decimal_value(reference, "yield_to_maturity"),
            YIELD_TOLERANCE,
            case_id,
            "yield_to_maturity",
        );
        assert_eq!(actual.schema_id(), CURVE_RESULT_SCHEMA_ID);
        assert_eq!(actual.algorithm_id(), CURVE_ALGORITHM_ID);
        assert_eq!(actual.algorithm_version(), CURVE_ALGORITHM_VERSION);
        assert_eq!(actual.convention_profile(), CURVE_CONVENTION_PROFILE);
        actual
            .validate_against(&query)
            .expect("curve lineage must bind");
    }
}

#[test]
fn production_carry_roll_matches_coupon_and_discount_manual_oracle() {
    let (inputs, expected) = frozen_cases();
    let cases = inputs["carry_cases"]
        .as_array()
        .expect("carry_cases must be an array");
    assert_eq!(
        cases.len(),
        2,
        "frozen suite requires coupon and discount cases"
    );
    for (index, case) in cases.iter().enumerate() {
        let case_id = string(case, "id");
        let input = carry_input(&inputs, case, index);
        let actual = NativeCarryRollEngine
            .calculate(&input)
            .unwrap_or_else(|error| panic!("{case_id} production calculation failed: {error:?}"));
        actual
            .validate_against(&input)
            .expect("carry lineage must bind");
        assert_eq!(actual.schema_id(), CARRY_ROLL_RESULT_SCHEMA_ID);
        assert_eq!(actual.algorithm_id(), CARRY_ROLL_ALGORITHM_ID);
        assert_eq!(actual.algorithm_version(), CARRY_ROLL_ALGORITHM_VERSION);
        assert_eq!(actual.convention_profile(), CARRY_ROLL_CONVENTION_PROFILE);

        let reference = &expected["carry_results"][case_id];
        assert_eq!(
            input.initial_curve_query_date().unwrap(),
            date_value(reference, "initial_curve_query_date"),
            "{case_id} initial residual-tenor query"
        );
        assert_eq!(
            input.rolled_curve_query_date().unwrap(),
            date_value(reference, "rolled_curve_query_date"),
            "{case_id} rolled residual-tenor query"
        );
        let measures = actual.measures();
        for (field, observed, tolerance) in [
            ("initial_yield", measures.initial_yield(), YIELD_TOLERANCE),
            ("rolled_yield", measures.rolled_yield(), YIELD_TOLERANCE),
            (
                "initial_dirty_price",
                measures.initial_dirty_price(),
                VALUE_TOLERANCE,
            ),
            (
                "horizon_dirty_at_initial_yield",
                measures.horizon_dirty_at_initial_yield(),
                VALUE_TOLERANCE,
            ),
            (
                "horizon_dirty_at_rolled_yield",
                measures.horizon_dirty_at_rolled_yield(),
                VALUE_TOLERANCE,
            ),
            ("paid_cashflows", measures.paid_cashflows(), VALUE_TOLERANCE),
            ("carry", measures.carry(), VALUE_TOLERANCE),
            ("roll_down", measures.roll_down(), VALUE_TOLERANCE),
            ("total_return", measures.total_return(), VALUE_TOLERANCE),
        ] {
            assert_abs(
                observed,
                decimal_value(reference, field),
                tolerance,
                case_id,
                field,
            );
        }
        assert_eq!(
            measures.total_return(),
            measures.carry().checked_add(measures.roll_down()).unwrap(),
            "{case_id} total-return identity"
        );
        assert_eq!(
            measures.carry(),
            measures
                .horizon_dirty_at_initial_yield()
                .checked_add(measures.paid_cashflows())
                .and_then(|value| value.checked_sub(measures.initial_dirty_price()))
                .unwrap(),
            "{case_id} carry identity"
        );
    }
}

fn frozen_cases() -> (Value, Value) {
    let root = repository_root();
    (
        read_json(&root.join("tests/golden-cases/china-rates/phase2b-curve-carry-inputs.json")),
        read_json(
            &root.join(
                "tests/golden-cases/china-rates/expected/phase2b-curve-carry-v1-expected.json",
            ),
        ),
    )
}

fn curve(inputs: &Value) -> YieldCurveBinding {
    let nodes = inputs["curve"]["nodes"]
        .as_array()
        .expect("curve nodes must be an array")
        .iter()
        .map(|node| {
            YieldCurveNode::new(
                date_value(node, "maturity_date"),
                decimal_value(node, "yield_to_maturity"),
            )
            .expect("frozen node must be valid")
        })
        .collect();
    YieldCurveBinding::new(
        object('F'),
        date_value(inputs, "valuation_date"),
        YieldCurveInterpolation::LinearYield,
        nodes,
    )
    .expect("frozen curve must be valid")
}

fn carry_input(inputs: &Value, case: &Value, index: usize) -> CarryRollInput {
    let valuation_date = date_value(inputs, "valuation_date");
    let instant = Utc
        .with_ymd_and_hms(
            valuation_date.year(),
            valuation_date.month(),
            valuation_date.day(),
            0,
            0,
            0,
        )
        .single()
        .expect("valuation instant must be valid");
    let market_time = MarketTime::new(instant, string(inputs, "market_timezone"), valuation_date)
        .expect("market time must be valid");
    let calendar = &inputs["calendar"];
    let calendar = CalendarBinding::new(
        string(calendar, "id"),
        Version::new(1).unwrap(),
        ContentHash::digest(b"phase2b-weekend-calendar-v1"),
        date_value(calendar, "coverage_start"),
        date_value(calendar, "coverage_end"),
        dates(calendar, "non_business_days"),
        dates(calendar, "work_weekends"),
    )
    .expect("frozen calendar must be valid");
    let bond = &case["bond"];
    let frequency = match bond["frequency"]
        .as_u64()
        .expect("frequency must be numeric")
    {
        1 => CouponFrequency::Annual,
        2 => CouponFrequency::Semiannual,
        other => panic!("unsupported frozen frequency {other}"),
    };
    let terms = BondTerms::new(
        date_value(bond, "issue_date"),
        date_value(bond, "maturity_date"),
        frequency,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        decimal_value(bond, "coupon_rate"),
        decimal_value(bond, "face_value"),
    )
    .expect("frozen terms must be valid");
    CarryRollInput::new(
        OwnerRef::new(id('A'), id('B')),
        object(char::from(b'C' + u8::try_from(index).unwrap())),
        object('D'),
        object('E'),
        market_time,
        date_value(case, "initial_settlement"),
        date_value(case, "horizon_settlement"),
        CalendarRequirement::ExactMarket,
        calendar,
        terms,
        curve(inputs),
    )
    .expect("frozen carry input must be valid")
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

fn string<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field} must be a string"))
}

fn date_value(value: &Value, field: &str) -> NaiveDate {
    NaiveDate::parse_from_str(string(value, field), "%Y-%m-%d")
        .unwrap_or_else(|error| panic!("{field} must be a date: {error}"))
}

fn dates(value: &Value, field: &str) -> Vec<NaiveDate> {
    value[field]
        .as_array()
        .unwrap_or_else(|| panic!("{field} must be an array"))
        .iter()
        .map(|item| {
            NaiveDate::parse_from_str(
                item.as_str().expect("calendar date must be a string"),
                "%Y-%m-%d",
            )
            .expect("calendar date must be valid")
        })
        .collect()
}

fn decimal_value(value: &Value, field: &str) -> FixedDecimal {
    let raw = string(value, field);
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    assert!(fraction.len() <= 12, "{field} exceeds fixed decimal scale");
    let negative = whole.starts_with('-');
    let absolute_whole = whole.trim_start_matches('-');
    let whole_scaled = absolute_whole
        .parse::<i128>()
        .expect("whole decimal must be numeric")
        * 1_000_000_000_000;
    let fractional_scaled = format!("{fraction:0<12}")
        .parse::<i128>()
        .expect("fractional decimal must be numeric");
    FixedDecimal::from_scaled(if negative {
        -(whole_scaled + fractional_scaled)
    } else {
        whole_scaled + fractional_scaled
    })
}

fn assert_abs(
    actual: FixedDecimal,
    expected: FixedDecimal,
    tolerance: u128,
    case_id: &str,
    field: &str,
) {
    assert!(
        actual.scaled().abs_diff(expected.scaled()) <= tolerance,
        "{case_id} {field}: actual {}, expected {}, tolerance {tolerance}",
        actual.scaled(),
        expected.scaled()
    );
}

fn object(suffix: char) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(id(suffix), Version::new(1).unwrap()),
        ContentHash::digest(suffix.to_string().as_bytes()),
    )
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

use chrono::Datelike;
