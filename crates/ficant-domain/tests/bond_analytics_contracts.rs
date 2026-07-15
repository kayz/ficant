use chrono::{NaiveDate, TimeZone, Utc};
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondTerms, BusinessDayConvention,
    CalendarBinding, CalendarRequirement, CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};

#[test]
fn bond_terms_reject_nonpositive_face_and_reversed_dates() {
    let date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    assert_eq!(
        BondTerms::new(
            date,
            date,
            CouponFrequency::Annual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            FixedDecimal::ZERO,
            FixedDecimal::ZERO,
        ),
        Err(DomainErrorCode::InvalidValue)
    );
}

#[test]
fn calendar_rejects_unsorted_or_duplicate_exceptions() {
    let first = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let second = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
    assert_eq!(
        CalendarBinding::new(
            "cgb-reference-calendar-v1",
            Version::new(1).unwrap(),
            ContentHash::digest(b"calendar"),
            first,
            second,
            vec![second, first],
            Vec::new(),
        ),
        Err(DomainErrorCode::InvalidValue)
    );
}

#[test]
fn exact_market_input_requires_settlement_coverage() {
    let issue = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let settlement = NaiveDate::from_ymd_opt(2026, 7, 14).unwrap();
    let maturity = NaiveDate::from_ymd_opt(2028, 1, 1).unwrap();
    let version = Version::new(1).unwrap();
    let object = |suffix| {
        AnalyticsObjectRef::new(
            VersionRef::new(id(suffix), version),
            ContentHash::digest(suffix.to_string().as_bytes()),
        )
    };
    let instant = Utc.with_ymd_and_hms(2026, 7, 13, 0, 0, 0).unwrap();
    let market_time = MarketTime::new(instant, "Asia/Shanghai", instant.date_naive()).unwrap();
    let terms = BondTerms::new(
        issue,
        maturity,
        CouponFrequency::Annual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        FixedDecimal::from_scaled(10_000_000_000),
        FixedDecimal::from_scaled(100_000_000_000_000),
    )
    .unwrap();
    let calendar = CalendarBinding::new(
        "cgb-reference-calendar-v1",
        version,
        ContentHash::digest(b"calendar"),
        issue,
        issue,
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    assert_eq!(
        BondAnalyticsInput::new(
            OwnerRef::new(id('A'), id('B')),
            object('C'),
            object('D'),
            object('E'),
            market_time,
            settlement,
            CalendarRequirement::ExactMarket,
            calendar,
            terms,
            AnalyticsMode::YieldIn,
            FixedDecimal::from_scaled(10_000_000_000),
        ),
        Err(DomainErrorCode::InvalidEffectiveTime)
    );
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
