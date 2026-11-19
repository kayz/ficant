use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};

use ficant_domain::market::{
    ArtifactInputKind, Bond, Calendar, CalendarInput, CalendarSession, Cashflow, CashflowInput,
    CashflowType, CurveSnapshot, CurveSnapshotInput, FactSource, FuturesContract, Instrument,
    InstrumentInput, InstrumentKind, MarketRulePack, MarketRulePackInput, MarketRulePackTimesInput,
    Quote, QuoteInput, Trade, TradeInput, Unit, UnitInput, Valuation, ValuationInput,
    VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged, VersionedDefinition};

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{ULID_PREFIX}{suffix}")).expect("fixture ULID must be valid")
}

fn version(value: u64) -> Version {
    Version::new(value).expect("fixture version must be positive")
}

fn version_ref(suffix: char) -> VersionRef {
    VersionRef::new(id(suffix), version(1))
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('A'), id('B'))
}

fn currency() -> UnitRef {
    UnitRef::new(id('C'), version(1))
}

fn decimal(coefficient: &str, scale: u32) -> DecimalValue {
    DecimalValue::new(coefficient, scale, currency()).expect("fixture decimal must be valid")
}

fn market_time(day: u32, hour: u32) -> MarketTime {
    let instant = Utc
        .with_ymd_and_hms(2026, 1, day, hour, 0, 0)
        .single()
        .expect("fixture UTC instant must be valid");
    let local_date = instant
        .with_timezone(&chrono_tz::Asia::Shanghai)
        .date_naive();
    MarketTime::new(instant, "Asia/Shanghai", local_date)
        .expect("fixture market time must be valid")
}

fn period(from_day: u32, to_day: u32) -> EffectivePeriod {
    EffectivePeriod::new(market_time(from_day, 1), market_time(to_day, 1))
        .expect("fixture period must be ordered")
}

fn hash(seed: u8) -> ContentHash {
    ContentHash::from_bytes(&[seed; 32]).expect("fixture hash must have 32 bytes")
}

fn lineage(suffix: char, seed: u8) -> LineageRef {
    LineageRef::new(id(suffix), Some(version(1)), Some(hash(seed)))
        .expect("fixture lineage must be exact")
}

fn source() -> FactSource {
    FactSource::new("fixture-feed", "external-1", 1).expect("fixture source must be valid")
}

fn instrument(kind: InstrumentKind, suffix: char) -> Instrument {
    Instrument::new(InstrumentInput {
        instrument_id: id(suffix),
        version: version(1),
        owner: owner(),
        kind,
        market: "XSHG".to_owned(),
        symbol: format!("SYM-{suffix}"),
        currency: currency(),
        calendar: version_ref('D'),
    })
    .expect("fixture instrument must be valid")
}

#[test]
fn q2_obj_01_instrument_is_a_valid_immutable_definition() {
    let value = instrument(InstrumentKind::Bond, 'E');
    assert_eq!(value.identity(), id('E').as_str());
    assert_eq!(value.version(), 1);
    assert_eq!(value.kind(), InstrumentKind::Bond);

    let error = Instrument::new(InstrumentInput {
        instrument_id: id('E'),
        version: version(1),
        owner: owner(),
        kind: InstrumentKind::Bond,
        market: "XSHG".to_owned(),
        symbol: String::new(),
        currency: currency(),
        calendar: version_ref('D'),
    })
    .expect_err("empty symbol must be rejected");
    assert_eq!(error, DomainErrorCode::InvalidValue);

    let changed_kind = Instrument::new(InstrumentInput {
        instrument_id: id('E'),
        version: version(2),
        owner: owner(),
        kind: InstrumentKind::Futures,
        market: "XSHG".to_owned(),
        symbol: "SYM-E".to_owned(),
        currency: currency(),
        calendar: version_ref('D'),
    })
    .expect("candidate instrument is independently valid");
    assert_eq!(
        value.validate_successor(&changed_kind).unwrap_err(),
        DomainErrorCode::VersionConflict
    );
}

#[test]
fn q2_obj_02_bond_requires_bond_kind_dates_and_positive_face_value() {
    let instrument = instrument(InstrumentKind::Bond, 'F');
    let value = Bond::new(
        &instrument,
        NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
        NaiveDate::from_ymd_opt(2030, 1, 1).unwrap(),
        decimal("100", 0),
    )
    .expect("valid bond must construct");
    assert_eq!(value.instrument().id(), instrument.id());

    let error = Bond::new(
        &instrument,
        NaiveDate::from_ymd_opt(2030, 1, 1).unwrap(),
        NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
        decimal("100", 0),
    )
    .expect_err("issue date at or after maturity must fail");
    assert_eq!(error, DomainErrorCode::InvalidEffectiveTime);
}

#[test]
fn q2_obj_03_futures_contract_requires_ordered_times_and_positive_multiplier() {
    let instrument = instrument(InstrumentKind::Futures, 'G');
    let value = FuturesContract::new(
        &instrument,
        market_time(1, 1),
        market_time(2, 1),
        market_time(3, 1),
        decimal("300", 0),
        version_ref('H'),
    )
    .expect("valid futures contract must construct");
    assert_eq!(value.instrument().id(), instrument.id());

    let error = FuturesContract::new(
        &instrument,
        market_time(3, 1),
        market_time(2, 1),
        market_time(1, 1),
        decimal("300", 0),
        version_ref('H'),
    )
    .expect_err("reversed futures dates must fail");
    assert_eq!(error, DomainErrorCode::InvalidEffectiveTime);
}

#[test]
fn q2_obj_04_cashflow_records_external_fact_without_generation_behavior() {
    let value = Cashflow::new(CashflowInput {
        cashflow_id: id('J'),
        bond: version_ref('F'),
        payment_time: market_time(3, 1),
        amount: decimal("250", 2),
        owner: owner(),
        source: source(),
        supersedes_id: None,
        cashflow_type: CashflowType::Coupon,
        schedule_id: "schedule-1".to_owned(),
        sequence: 1,
    })
    .expect("valid cashflow fact must construct");
    assert_eq!(value.schedule_id(), "schedule-1");
    assert_eq!(value.cashflow_type(), CashflowType::Coupon);
    assert_eq!(value.sequence(), 1);

    let error = Cashflow::new(CashflowInput {
        cashflow_id: id('J'),
        bond: version_ref('F'),
        payment_time: market_time(3, 1),
        amount: decimal("250", 2),
        owner: owner(),
        source: source(),
        supersedes_id: None,
        cashflow_type: CashflowType::Coupon,
        schedule_id: "schedule-1".to_owned(),
        sequence: 0,
    })
    .expect_err("cashflow sequence zero must fail");
    assert_eq!(error, DomainErrorCode::InvalidValue);
}

#[test]
fn q2_obj_05_calendar_requires_unique_ordered_sessions() {
    let session = CalendarSession::open(
        NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
        NaiveTime::from_hms_opt(9, 30, 0).unwrap(),
        NaiveTime::from_hms_opt(15, 0, 0).unwrap(),
    )
    .expect("valid session must construct");
    let value = Calendar::new(CalendarInput {
        calendar_id: id('K'),
        version: version(1),
        owner: owner(),
        market: "XSHG".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: period(1, 4),
        sessions: vec![session.clone()],
    })
    .expect("valid calendar must construct");
    assert_eq!(value.sessions().len(), 1);

    let error = Calendar::new(CalendarInput {
        calendar_id: id('K'),
        version: version(1),
        owner: owner(),
        market: "XSHG".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: period(1, 4),
        sessions: vec![session.clone(), session],
    })
    .expect_err("duplicate local session date must fail");
    assert_eq!(error, DomainErrorCode::InvalidEffectiveTime);
}

#[test]
fn q2_obj_06_unit_requires_normalized_code_dimension_and_precision() {
    let value = Unit::new(UnitInput {
        unit_id: id('M'),
        version: version(1),
        owner: owner(),
        code: "CNY".to_owned(),
        dimension: "currency".to_owned(),
        scale: 2,
        precision: 18,
    })
    .expect("valid unit must construct");
    assert_eq!(value.code(), "CNY");
    assert_eq!(value.identity(), id('M').as_str());

    let error = Unit::new(UnitInput {
        unit_id: id('M'),
        version: version(1),
        owner: owner(),
        code: "cny".to_owned(),
        dimension: "currency".to_owned(),
        scale: 2,
        precision: 18,
    })
    .expect_err("lowercase unit code must fail");
    assert_eq!(error, DomainErrorCode::InvalidUnit);

    let changed_dimension = Unit::new(UnitInput {
        unit_id: id('M'),
        version: version(2),
        owner: owner(),
        code: "CNY".to_owned(),
        dimension: "mass".to_owned(),
        scale: 2,
        precision: 18,
    })
    .expect("candidate unit is independently valid");
    assert_eq!(
        value.validate_successor(&changed_dimension).unwrap_err(),
        DomainErrorCode::VersionConflict
    );
}

#[test]
fn q2_obj_07_quote_requires_a_side_and_bid_not_above_ask() {
    let value = Quote::new(QuoteInput {
        quote_id: id('N'),
        instrument: version_ref('E'),
        owner: owner(),
        source: source(),
        observed_at: market_time(1, 1),
        received_at: market_time(1, 2),
        bid: Some(decimal("99", 0)),
        ask: Some(decimal("100", 0)),
        supersedes_id: None,
    })
    .expect("valid quote must construct");
    assert!(value.bid().is_some() && value.ask().is_some());

    let error = Quote::new(QuoteInput {
        quote_id: id('N'),
        instrument: version_ref('E'),
        owner: owner(),
        source: source(),
        observed_at: market_time(1, 1),
        received_at: market_time(1, 2),
        bid: Some(decimal("101", 0)),
        ask: Some(decimal("100", 0)),
        supersedes_id: None,
    })
    .expect_err("bid above ask must fail");
    assert_eq!(error, DomainErrorCode::InvalidValue);
}

#[test]
fn q2_obj_08_trade_requires_positive_quantity() {
    let value = Trade::new(TradeInput {
        trade_id: id('P'),
        instrument: version_ref('E'),
        owner: owner(),
        source: source(),
        executed_at: market_time(1, 1),
        price: decimal("100", 0),
        quantity: decimal("2", 0),
        supersedes_id: None,
    })
    .expect("valid trade must construct");
    assert!(value.quantity().is_positive());

    let error = Trade::new(TradeInput {
        trade_id: id('P'),
        instrument: version_ref('E'),
        owner: owner(),
        source: source(),
        executed_at: market_time(1, 1),
        price: decimal("100", 0),
        quantity: decimal("0", 0),
        supersedes_id: None,
    })
    .expect_err("zero quantity must fail");
    assert_eq!(error, DomainErrorCode::InvalidValue);
}

#[test]
fn q2_obj_09_valuation_records_external_values_without_pricing() {
    let value = Valuation::new(ValuationInput {
        valuation_id: id('Q'),
        instrument: version_ref('E'),
        owner: owner(),
        source: source(),
        valuation_at: market_time(1, 1),
        method: "external-clean-price".to_owned(),
        rule_pack: version_ref('H'),
        values: vec![decimal("10125", 2)],
        supersedes_id: None,
    })
    .expect("valid external valuation must construct");
    assert_eq!(value.values().len(), 1);

    let error = Valuation::new(ValuationInput {
        valuation_id: id('Q'),
        instrument: version_ref('E'),
        owner: owner(),
        source: source(),
        valuation_at: market_time(1, 1),
        method: "external-clean-price".to_owned(),
        rule_pack: version_ref('H'),
        values: vec![],
        supersedes_id: None,
    })
    .expect_err("valuation without supplied values must fail");
    assert_eq!(error, DomainErrorCode::InvalidValue);
}

#[test]
fn q2_obj_10_curve_snapshot_is_content_addressed_input_metadata() {
    let value = CurveSnapshot::new(CurveSnapshotInput {
        curve_snapshot_id: id('R'),
        owner: owner(),
        as_of: market_time(1, 1),
        currency: currency(),
        curve_kind: "government-zero".to_owned(),
        calendar: version_ref('D'),
        rule_pack: version_ref('H'),
        point_schema: "tenor,value".to_owned(),
        content_hash: hash(10),
        lineage: vec![lineage('N', 11)],
        input_kind: ArtifactInputKind::ExternalFixture,
    })
    .expect("valid curve snapshot metadata must construct");
    assert_eq!(value.content_hash(), &hash(10));
    assert_eq!(value.lineage().len(), 1);

    let error = CurveSnapshot::new(CurveSnapshotInput {
        curve_snapshot_id: id('R'),
        owner: owner(),
        as_of: market_time(1, 1),
        currency: currency(),
        curve_kind: "government-zero".to_owned(),
        calendar: version_ref('D'),
        rule_pack: version_ref('H'),
        point_schema: "tenor,value".to_owned(),
        content_hash: hash(10),
        lineage: vec![],
        input_kind: ArtifactInputKind::ExternalFixture,
    })
    .expect_err("curve snapshot without lineage must fail");
    assert_eq!(error, DomainErrorCode::BrokenLineage);
}

#[test]
fn q2_obj_11_market_rule_pack_requires_an_ordered_effective_period() {
    let value = MarketRulePack::new(MarketRulePackInput {
        rule_pack_id: id('S'),
        version: version(1),
        owner: owner(),
        market: "XSHG".to_owned(),
        rule_type: "calendar".to_owned(),
        source: "exchange-rulebook".to_owned(),
        effective: period(1, 3),
        verification_status: VerificationStatus::Verified,
        content_hash: hash(12),
    })
    .expect("valid rule pack must construct");
    assert_eq!(value.identity(), id('S').as_str());

    let error = MarketRulePack::new_with_times(MarketRulePackTimesInput {
        rule_pack_id: id('S'),
        version: version(1),
        owner: owner(),
        market: "XSHG".to_owned(),
        rule_type: "calendar".to_owned(),
        source: "exchange-rulebook".to_owned(),
        from: market_time(3, 1),
        to: market_time(1, 1),
        verification_status: VerificationStatus::Verified,
        content_hash: hash(12),
    })
    .expect_err("reversed rule effective interval must fail");
    assert_eq!(error, DomainErrorCode::InvalidEffectiveTime);
}
