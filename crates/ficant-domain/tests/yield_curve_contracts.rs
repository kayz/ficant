use chrono::NaiveDate;
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{AnalyticsObjectRef, FixedDecimal};
use ficant_domain::curves::{
    YieldCurveBinding, YieldCurveInterpolation, YieldCurveNode, YieldCurveQuery,
};
use ficant_domain::primitives::{ContentHash, Ulid, Version, VersionRef};

#[test]
fn curve_rejects_too_few_unsorted_duplicate_and_prevaluation_nodes() {
    let valuation = date(2026, 7, 17);
    let first = YieldCurveNode::new(date(2027, 7, 17), rate("0.015")).unwrap();
    let second = YieldCurveNode::new(date(2028, 7, 17), rate("0.020")).unwrap();
    assert_eq!(
        YieldCurveBinding::new(
            curve_ref(),
            valuation,
            YieldCurveInterpolation::LinearYield,
            vec![first],
        ),
        Err(DomainErrorCode::InvalidValue)
    );
    assert_eq!(
        YieldCurveBinding::new(
            curve_ref(),
            valuation,
            YieldCurveInterpolation::LinearYield,
            vec![second, first],
        ),
        Err(DomainErrorCode::InvalidValue)
    );
    assert_eq!(
        YieldCurveBinding::new(
            curve_ref(),
            valuation,
            YieldCurveInterpolation::LinearYield,
            vec![first, first],
        ),
        Err(DomainErrorCode::InvalidValue)
    );
    let old = YieldCurveNode::new(valuation, rate("0.010")).unwrap();
    assert_eq!(
        YieldCurveBinding::new(
            curve_ref(),
            valuation,
            YieldCurveInterpolation::LinearYield,
            vec![old, first],
        ),
        Err(DomainErrorCode::InvalidValue)
    );
}

#[test]
fn curve_query_is_closed_to_the_frozen_node_range() {
    let valuation = date(2026, 7, 17);
    let first_date = date(2027, 7, 17);
    let last_date = date(2028, 7, 17);
    let curve = YieldCurveBinding::new(
        curve_ref(),
        valuation,
        YieldCurveInterpolation::LinearYield,
        vec![
            YieldCurveNode::new(first_date, rate("0.015")).unwrap(),
            YieldCurveNode::new(last_date, rate("0.020")).unwrap(),
        ],
    )
    .unwrap();
    assert!(YieldCurveQuery::new(curve.clone(), first_date).is_ok());
    assert!(YieldCurveQuery::new(curve.clone(), last_date).is_ok());
    assert_eq!(
        YieldCurveQuery::new(curve.clone(), date(2027, 7, 16)),
        Err(DomainErrorCode::InvalidEffectiveTime)
    );
    assert_eq!(
        YieldCurveQuery::new(curve, date(2028, 7, 18)),
        Err(DomainErrorCode::InvalidEffectiveTime)
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

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn rate(value: &str) -> FixedDecimal {
    let decimal = value.parse::<rust_decimal::Decimal>().unwrap();
    FixedDecimal::from_scaled(decimal.mantissa() * 10_i128.pow(12 - decimal.scale()))
}
