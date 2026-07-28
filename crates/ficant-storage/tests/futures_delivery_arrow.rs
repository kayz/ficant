use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::CalculateFuturesDeliveryBasket;
use ficant_application::ports::FuturesDeliveryArtifactCodec;
use ficant_domain::analytics::{
    AnalyticsObjectRef, BondTerms, BusinessDayConvention, CouponFrequency, DayCountConvention,
    FixedDecimal,
};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliverableInput, FuturesDeliveryRule, FuturesDeliveryRuleInput,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeFuturesDeliveryEngine;
use ficant_storage::futures_arrow::ArrowFuturesDeliveryCodec;

#[test]
fn native_basket_round_trips_through_deterministic_arrow_file() {
    let inputs = vec![
        input('G', fixed(102_000_000_000_000)),
        input('H', fixed(100_000_000_000_000)),
    ];
    let result = CalculateFuturesDeliveryBasket::new(&NativeFuturesDeliveryEngine)
        .execute(&inputs)
        .unwrap();
    let codec = ArrowFuturesDeliveryCodec;
    let first = codec.encode(&result).unwrap();
    assert_eq!(
        hex(first.content_hash().as_bytes()),
        "40fda230fb7f0736e332155d1fd252267caf0acff55896487142fd9046a8f523"
    );
    let second = codec.encode(&result).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.content_hash(), second.content_hash());
    assert_eq!(codec.decode(first.bytes(), &inputs).unwrap(), result);

    let drifted = vec![input('G', fixed(101_000_000_000_000)), inputs[1].clone()];
    assert_ne!(drifted[0].fingerprint(), inputs[0].fingerprint());
    assert!(codec.decode(first.bytes(), &drifted).is_err());
}

fn input(bond_suffix: char, spot_clean_price: FixedDecimal) -> FuturesDeliverableInput {
    FuturesDeliverableInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object(bond_suffix),
        object('D'),
        object('E'),
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
        rule(),
        BondTerms::new(
            date(2024, 8, 15),
            date(2034, 8, 15),
            CouponFrequency::Semiannual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            fixed(25_000_000_000),
            fixed(100_000_000_000_000),
        )
        .unwrap(),
        spot_clean_price,
        fixed(99_500_000_000_000),
        fixed(18_000_000_000),
    )
    .unwrap()
}

fn rule() -> FuturesDeliveryRule {
    FuturesDeliveryRule::new(FuturesDeliveryRuleInput {
        original_term_max_months: 120,
        residual_min_months: 78,
        residual_max_months: None,
        delivery_months: vec![3, 6, 9, 12],
        nominal_coupon: fixed(30_000_000_000),
        face_quote_basis: fixed(100_000_000_000_000),
        accrued_interest_day_count: 1,
        conversion_factor_rounding_places: 4,
        accrued_interest_rounding_places: 7,
        annual_day_basis: 365,
    })
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

const fn fixed(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value)
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut value, byte| {
        write!(value, "{byte:02x}").unwrap();
        value
    })
}
