use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{BondAnalyticsEngine, CarryRollEngine};
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondTerms, BusinessDayConvention,
    CalendarBinding, CalendarRequirement, CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::curves::{
    CARRY_ROLL_ALGORITHM_ID, CARRY_ROLL_ALGORITHM_VERSION, CARRY_ROLL_CONVENTION_PROFILE,
    CARRY_ROLL_RESULT_SCHEMA_ID, CarryRollInput, YieldCurveBinding, YieldCurveInterpolation,
    YieldCurveNode,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::{NativeBondAnalyticsEngine, NativeCarryRollEngine};

#[test]
fn coupon_bond_carry_roll_binds_curve_and_paid_cashflow_identity() {
    let input = carry_input();
    for settlement in [input.initial_settlement(), input.horizon_settlement()] {
        let bond_input = BondAnalyticsInput::new(
            input.owner().clone(),
            input.bond().clone(),
            input.rule_pack().clone(),
            input.snapshot().clone(),
            input.valuation_at().clone(),
            settlement,
            input.calendar_requirement(),
            input.calendar().clone(),
            input.terms().clone(),
            AnalyticsMode::YieldIn,
            fixed(20_000_000_000),
        )
        .unwrap();
        NativeBondAnalyticsEngine
            .calculate(&bond_input)
            .unwrap_or_else(|error| panic!("settlement {settlement}: {error:?}"));
    }
    let result = NativeCarryRollEngine.calculate(&input).unwrap();
    result.validate_against(&input).unwrap();
    assert_eq!(result.schema_id(), CARRY_ROLL_RESULT_SCHEMA_ID);
    assert_eq!(result.algorithm_id(), CARRY_ROLL_ALGORITHM_ID);
    assert_eq!(result.algorithm_version(), CARRY_ROLL_ALGORITHM_VERSION);
    assert_eq!(result.convention_profile(), CARRY_ROLL_CONVENTION_PROFILE);

    let measures = result.measures();
    assert_eq!(measures.paid_cashflows(), fixed(2_000_000_000_000));
    assert!(measures.rolled_yield() < measures.initial_yield());
    assert!(measures.roll_down().is_positive());
    assert_eq!(
        measures.total_return(),
        measures.carry().checked_add(measures.roll_down()).unwrap()
    );
    assert_eq!(
        measures.carry(),
        measures
            .horizon_dirty_at_initial_yield()
            .checked_add(measures.paid_cashflows())
            .and_then(|value| value.checked_sub(measures.initial_dirty_price()))
            .unwrap()
    );
}

fn carry_input() -> CarryRollInput {
    let valuation_date = date(2026, 7, 19);
    let initial_settlement = date(2026, 7, 20);
    let horizon_settlement = date(2027, 1, 2);
    let issue = date(2026, 1, 1);
    let maturity = date(2029, 1, 1);
    let instant = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
    let market_time = MarketTime::new(instant, "Asia/Shanghai", valuation_date).unwrap();
    let version = Version::new(1).unwrap();
    let calendar = CalendarBinding::new(
        "cgb-calendar-v1",
        version,
        ContentHash::digest(b"calendar"),
        issue,
        date(2029, 1, 8),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let terms = BondTerms::new(
        issue,
        maturity,
        CouponFrequency::Annual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        fixed(20_000_000_000),
        fixed(100_000_000_000_000),
    )
    .unwrap();
    let curve = YieldCurveBinding::new(
        object('F'),
        valuation_date,
        YieldCurveInterpolation::LinearYield,
        vec![
            YieldCurveNode::new(date(2028, 1, 1), fixed(15_000_000_000)).unwrap(),
            YieldCurveNode::new(date(2029, 12, 31), fixed(25_000_000_000)).unwrap(),
        ],
    )
    .unwrap();
    CarryRollInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        market_time,
        initial_settlement,
        horizon_settlement,
        CalendarRequirement::ExactMarket,
        calendar,
        terms,
        curve,
    )
    .unwrap()
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

fn fixed(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value)
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}
