use chrono::{NaiveDate, TimeZone, Utc};

use ficant_domain::DomainErrorCode;
use ficant_domain::market::{
    FactSource, Quote, QuoteInput, Trade, TradeInput, Unit, UnitInput, Valuation, ValuationInput,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{ULID_PREFIX}{suffix}")).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('A'), id('B'))
}

fn version(value: u64) -> Version {
    Version::new(value).unwrap()
}

fn time(day: u32, hour: u32) -> MarketTime {
    let instant = Utc
        .with_ymd_and_hms(2026, 2, day, hour, 0, 0)
        .single()
        .unwrap();
    MarketTime::new(
        instant,
        "Asia/Shanghai",
        instant
            .with_timezone(&chrono_tz::Asia::Shanghai)
            .date_naive(),
    )
    .unwrap()
}

#[test]
fn domain_error_code_is_the_exact_stable_nine_variant_set() {
    let codes = [
        DomainErrorCode::InvalidId,
        DomainErrorCode::InvalidUnit,
        DomainErrorCode::InvalidEffectiveTime,
        DomainErrorCode::VersionConflict,
        DomainErrorCode::ContentHashMismatch,
        DomainErrorCode::BrokenLineage,
        DomainErrorCode::InvalidStateTransition,
        DomainErrorCode::JournalSequenceConflict,
        DomainErrorCode::InvalidValue,
    ];
    let stable_numbers = codes.map(|code| match code {
        DomainErrorCode::InvalidId => 1,
        DomainErrorCode::InvalidUnit => 2,
        DomainErrorCode::InvalidEffectiveTime => 3,
        DomainErrorCode::VersionConflict => 4,
        DomainErrorCode::ContentHashMismatch => 5,
        DomainErrorCode::BrokenLineage => 6,
        DomainErrorCode::InvalidStateTransition => 7,
        DomainErrorCode::JournalSequenceConflict => 8,
        DomainErrorCode::InvalidValue => 9,
    });
    assert_eq!(stable_numbers, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn ordinary_domain_values_use_invalid_value() {
    assert_eq!(
        FactSource::new("", "external-1", 1).unwrap_err(),
        DomainErrorCode::InvalidValue
    );
    assert_eq!(
        DecimalValue::new("1.25", 2, UnitRef::new(id('D'), version(1))).unwrap_err(),
        DomainErrorCode::InvalidValue
    );
    let unit = UnitRef::new(id('D'), version(1));
    let amount = |coefficient| DecimalValue::new(coefficient, 0, unit.clone()).unwrap();
    let instrument = VersionRef::new(id('F'), version(1));
    let source = || FactSource::new("feed", "external-1", 1).unwrap();
    assert_eq!(
        Trade::new(TradeInput {
            trade_id: id('G'),
            instrument: instrument.clone(),
            owner: owner(),
            source: source(),
            executed_at: time(1, 1),
            price: amount("100"),
            quantity: amount("0"),
            supersedes_id: None,
        })
        .unwrap_err(),
        DomainErrorCode::InvalidValue
    );
    assert_eq!(
        Valuation::new(ValuationInput {
            valuation_id: id('H'),
            instrument: instrument.clone(),
            owner: owner(),
            source: source(),
            valuation_at: time(1, 1),
            method: "external".to_owned(),
            rule_pack: VersionRef::new(id('J'), version(1)),
            values: vec![],
            supersedes_id: None,
        })
        .unwrap_err(),
        DomainErrorCode::InvalidValue
    );
    assert_eq!(
        Quote::new(QuoteInput {
            quote_id: id('K'),
            instrument,
            owner: owner(),
            source: source(),
            observed_at: time(1, 1),
            received_at: time(1, 2),
            bid: Some(amount("101")),
            ask: Some(amount("100")),
            supersedes_id: None,
        })
        .unwrap_err(),
        DomainErrorCode::InvalidValue
    );
}

#[test]
fn specialized_error_codes_take_precedence_over_invalid_value() {
    assert_eq!(
        Ulid::new("invalid").unwrap_err(),
        DomainErrorCode::InvalidId
    );
    assert_eq!(
        Unit::new(UnitInput {
            unit_id: id('C'),
            version: version(1),
            owner: owner(),
            code: "cny".to_owned(),
            dimension: "currency".to_owned(),
            scale: 2,
            precision: 18,
        })
        .unwrap_err(),
        DomainErrorCode::InvalidUnit
    );
    let instant = Utc.with_ymd_and_hms(2026, 2, 1, 1, 0, 0).single().unwrap();
    assert_eq!(
        MarketTime::new(
            instant,
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2026, 2, 2).unwrap(),
        )
        .unwrap_err(),
        DomainErrorCode::InvalidEffectiveTime
    );
    assert_eq!(
        Version::new(0).unwrap_err(),
        DomainErrorCode::VersionConflict
    );
    assert_eq!(
        ContentHash::from_bytes(&[0; 31]).unwrap_err(),
        DomainErrorCode::ContentHashMismatch
    );
    assert_eq!(
        LineageRef::new(id('C'), None, None).unwrap_err(),
        DomainErrorCode::BrokenLineage
    );
}

#[test]
fn decimal_is_canonical_and_never_accepts_float_input() {
    let unit = UnitRef::new(id('D'), version(1));
    let value = DecimalValue::new("+0001000", 3, unit.clone()).unwrap();
    assert_eq!(value.coefficient(), "1");
    assert_eq!(value.scale(), 0);
    assert_eq!(value.unit(), &unit);

    assert_eq!(
        DecimalValue::new("1.25", 2, unit).unwrap_err(),
        DomainErrorCode::InvalidValue
    );
}

#[test]
fn invalid_timezone_local_date_and_period_are_rejected() {
    let instant = Utc.with_ymd_and_hms(2026, 2, 1, 1, 0, 0).single().unwrap();
    assert_eq!(
        MarketTime::new(
            instant,
            "Invalid/Timezone",
            NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
        )
        .unwrap_err(),
        DomainErrorCode::InvalidEffectiveTime
    );
    assert_eq!(
        EffectivePeriod::new(time(2, 1), time(1, 1)).unwrap_err(),
        DomainErrorCode::InvalidEffectiveTime
    );
}
