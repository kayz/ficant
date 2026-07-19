use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::FuturesDeliveryEngine;
use ficant_domain::analytics::{
    AnalyticsObjectRef, BondTerms, BusinessDayConvention, CouponFrequency, DayCountConvention,
    FixedDecimal,
};
use ficant_domain::futures_delivery::{CgbFuturesProduct, FuturesDeliverableInput};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeFuturesDeliveryEngine;
use rust_decimal::Decimal;

#[test]
fn ten_year_candidate_crosses_safe_rust_adapter_and_preserves_identities() {
    let input = input();
    let result = NativeFuturesDeliveryEngine.calculate(&input).unwrap();
    result.validate_against(&input).unwrap();
    let measures = result.measures();
    assert!(measures.conversion_factor().is_positive());
    assert_eq!(
        measures.net_basis(),
        measures
            .gross_basis()
            .checked_sub(measures.holding_carry())
            .unwrap()
    );
    assert_eq!(
        measures.delivery_profit(),
        FixedDecimal::ZERO
            .checked_sub(measures.net_basis())
            .unwrap()
    );
}

fn input() -> FuturesDeliverableInput {
    FuturesDeliverableInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        object('F'),
        MarketTime::new(
            Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).single().unwrap(),
            "Asia/Shanghai",
            date(2026, 7, 20),
        )
        .unwrap(),
        date(2026, 7, 21),
        date(2026, 9, 1),
        date(2026, 9, 18),
        CgbFuturesProduct::TenYear,
        BondTerms::new(
            date(2024, 8, 15),
            date(2034, 8, 15),
            CouponFrequency::Semiannual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            fixed("0.025"),
            fixed("100"),
        )
        .unwrap(),
        5,
        16,
        fixed("101.25"),
        fixed("0.45"),
        fixed("0.80"),
        FixedDecimal::ZERO,
        fixed("99.50"),
        fixed("0.018"),
    )
    .unwrap()
}

fn fixed(raw: &str) -> FixedDecimal {
    let mut value = raw.parse::<Decimal>().unwrap() * Decimal::from(1_000_000_000_000_i64);
    value.rescale(0);
    let scaled = value.mantissa();
    FixedDecimal::from_scaled(scaled)
}
fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
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
