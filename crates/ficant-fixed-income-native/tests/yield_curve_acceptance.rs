use chrono::NaiveDate;
use ficant_application::ports::YieldCurveEngine;
use ficant_domain::analytics::{AnalyticsObjectRef, FixedDecimal};
use ficant_domain::curves::{
    CURVE_ALGORITHM_ID, CURVE_ALGORITHM_VERSION, CURVE_CONVENTION_PROFILE, CURVE_RESULT_SCHEMA_ID,
    YieldCurveBinding, YieldCurveInterpolation, YieldCurveNode, YieldCurveQuery,
};
use ficant_domain::primitives::{ContentHash, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeYieldCurveEngine;

#[test]
fn native_curve_preserves_nodes_and_interpolates_actual_day_distance() {
    let first = date(2027, 1, 1);
    let middle = date(2027, 7, 20);
    let last = date(2028, 8, 23);
    let curve = YieldCurveBinding::new(
        curve_ref(),
        date(2026, 7, 19),
        YieldCurveInterpolation::LinearYield,
        vec![
            YieldCurveNode::new(first, fixed(15_000_000_000)).unwrap(),
            YieldCurveNode::new(middle, fixed(20_000_000_000)).unwrap(),
            YieldCurveNode::new(last, fixed(28_000_000_000)).unwrap(),
        ],
    )
    .unwrap();

    let exact_query = YieldCurveQuery::new(curve.clone(), middle).unwrap();
    let exact = NativeYieldCurveEngine.interpolate(&exact_query).unwrap();
    assert_eq!(exact.yield_to_maturity(), fixed(20_000_000_000));
    assert_eq!(exact.schema_id(), CURVE_RESULT_SCHEMA_ID);
    assert_eq!(exact.algorithm_id(), CURVE_ALGORITHM_ID);
    assert_eq!(exact.algorithm_version(), CURVE_ALGORITHM_VERSION);
    assert_eq!(exact.convention_profile(), CURVE_CONVENTION_PROFILE);
    exact.validate_against(&exact_query).unwrap();

    let query_date = date(2027, 4, 11);
    let query = YieldCurveQuery::new(curve, query_date).unwrap();
    let point = NativeYieldCurveEngine.interpolate(&query).unwrap();
    assert_eq!(
        point.yield_to_maturity(),
        fixed(17_500_000_000),
        "query is exactly halfway through the 200-day interval"
    );
}

fn curve_ref() -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FAC").unwrap(),
            Version::new(1).unwrap(),
        ),
        ContentHash::digest(b"curve"),
    )
}

fn fixed(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value)
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}
