use chrono::{NaiveDate, TimeZone, Utc};
use ficant_domain::ContentAddressed;
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::market::{
    FuturesContract, Instrument, InstrumentInput, InstrumentKind, PriceSourceType,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    AccountingBook, AccountingClassification, AccountingClassificationState, CoverageDeclaration,
    FactorDv01, PortfolioKeyRateExposure, Position, PositionHoldingForm, PositionInput,
    PositionKeyRateExposure, PriceSourceCount, PriceSourceSummary, RiskAlgorithmBinding,
    scale_futures_key_rate_dv01,
};

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

#[test]
fn concrete_futures_risk_selectors_are_explicit_and_legacy_contracts_remain_readable() {
    let instrument = futures_instrument();
    let legacy = FuturesContract::new(
        &instrument,
        market_time(2026, 9, 17, 7),
        market_time(2026, 9, 18, 7),
        market_time(2026, 9, 18, 8),
        decimal("100", 0, unit('M')),
        VersionRef::new(id('R'), version()),
    )
    .unwrap();
    assert!(legacy.product_code().is_none());
    assert!(legacy.price_unit().is_none());

    let risk_ready = legacy
        .clone()
        .with_risk_terms("TS", unit('P'))
        .expect("a concrete product and exact quote Unit make the contract risk-ready");
    assert_eq!(risk_ready.product_code(), Some("TS"));
    assert_eq!(risk_ready.price_unit(), Some(&unit('P')));
    assert!(legacy.clone().with_risk_terms("", unit('P')).is_err());
    assert!(legacy.with_risk_terms(" TS", unit('P')).is_err());
}

#[test]
fn fixed_ctd_scaling_is_exact_signed_and_uses_rule_pack_contract_size() {
    let registered_face_krd = fixed("4000000000000");
    let registered_face = fixed("200000000000000");
    let quote_basis = fixed("100000000000000");
    let conversion_factor = fixed("800000000000");

    let ten_year = scale_futures_key_rate_dv01(
        registered_face_krd,
        registered_face,
        quote_basis,
        10_000,
        conversion_factor,
        2,
    )
    .unwrap();
    let two_year = scale_futures_key_rate_dv01(
        registered_face_krd,
        registered_face,
        quote_basis,
        20_000,
        conversion_factor,
        2,
    )
    .unwrap();
    let short = scale_futures_key_rate_dv01(
        registered_face_krd,
        registered_face,
        quote_basis,
        10_000,
        conversion_factor,
        -2,
    )
    .unwrap();

    assert_eq!(ten_year, fixed("50000000000000000"));
    assert_eq!(two_year, fixed("100000000000000000"));
    assert_eq!(short, fixed("-50000000000000000"));
    assert_eq!(
        scale_futures_key_rate_dv01(
            FixedDecimal::ZERO,
            registered_face,
            quote_basis,
            10_000,
            conversion_factor,
            2,
        )
        .unwrap(),
        FixedDecimal::ZERO
    );
    assert!(
        scale_futures_key_rate_dv01(
            registered_face_krd,
            registered_face,
            quote_basis,
            0,
            conversion_factor,
            1,
        )
        .is_err()
    );
    assert!(
        scale_futures_key_rate_dv01(
            registered_face_krd,
            registered_face,
            quote_basis,
            10_000,
            conversion_factor,
            0,
        )
        .is_err()
    );
}

#[test]
fn full_portfolio_hash_and_totals_commit_the_consumed_futures_snapshot() {
    let dv01 = unit('D');
    let factor_hash = ContentHash::digest(b"factor-10y");
    let bond = position(
        'B',
        'I',
        FactorDv01::new(
            "cn.gov.yield.10y",
            factor_hash.clone(),
            fixed("200000000000"),
            dv01.clone(),
        )
        .unwrap(),
    );
    let future = position(
        'F',
        'J',
        FactorDv01::new("cn.gov.yield.10y", factor_hash, fixed("-50000000000"), dv01).unwrap(),
    );
    let algorithm = RiskAlgorithmBinding::new(
        "ficant.fixed-income.portfolio-key-rate-yield",
        1,
        "linear-ytm-fixed-base-ctd-v1",
    )
    .unwrap();
    let data_snapshot = id('S');
    let source_confidence = mixed_source_confidence();
    let coverage = complete_coverage(source_confidence.clone());
    let portfolio = PortfolioKeyRateExposure::new_with_futures_data_snapshot(
        id('P'),
        id('C'),
        data_snapshot.clone(),
        vec![bond.clone(), future.clone()],
        algorithm.clone(),
        (source_confidence.clone(), coverage.clone()),
        vec![lineage('L')],
    )
    .unwrap();
    assert_eq!(portfolio.futures_data_snapshot_id(), Some(&data_snapshot));
    assert_eq!(portfolio.positions().len(), 2);
    assert_eq!(portfolio.totals()[0].value(), fixed("150000000000"));
    assert_eq!(
        portfolio
            .source_confidence()
            .counts()
            .iter()
            .map(|value| (value.source_type(), value.record_count()))
            .collect::<Vec<_>>(),
        vec![
            (PriceSourceType::ActiveQuote, 1),
            (PriceSourceType::CurveInterpolation, 2),
        ]
    );

    let other = PortfolioKeyRateExposure::new_with_futures_data_snapshot(
        id('P'),
        id('C'),
        id('T'),
        vec![bond, future],
        algorithm,
        (source_confidence, coverage),
        vec![lineage('L')],
    )
    .unwrap();
    assert_ne!(portfolio.content_hash(), other.content_hash());
}

fn mixed_source_confidence() -> PriceSourceSummary {
    PriceSourceSummary::new(vec![
        PriceSourceCount::new(PriceSourceType::ActiveQuote, 1).unwrap(),
        PriceSourceCount::new(PriceSourceType::CurveInterpolation, 2).unwrap(),
    ])
    .unwrap()
}

fn complete_coverage(source_confidence: PriceSourceSummary) -> CoverageDeclaration {
    let positions = vec![coverage_position('B', 'I'), coverage_position('F', 'J')];
    CoverageDeclaration::for_complete_positions(
        &positions,
        &[id('B'), id('F')],
        Some(source_confidence),
        1,
    )
    .unwrap()
}

fn coverage_position(position_suffix: char, instrument_suffix: char) -> Position {
    let money = unit('C');
    Position::new(PositionInput {
        position_id: id(position_suffix),
        instrument_ref: VersionRef::new(id(instrument_suffix), version()),
        quantity: decimal("1", 0, unit('M')),
        economic_value: decimal("100", 0, money.clone()),
        economic_pnl: decimal("0", 0, money.clone()),
        accounting_pnl: decimal("0", 0, money.clone()),
        capital_requirement: decimal("1", 0, money),
        accounting_classification: AccountingClassification::new(
            AccountingClassificationState::Classified,
            Some(AccountingBook::Ac),
        )
        .unwrap(),
        holding_form: PositionHoldingForm::Owned,
    })
    .unwrap()
}

fn position(
    position_suffix: char,
    instrument_suffix: char,
    exposure: FactorDv01,
) -> PositionKeyRateExposure {
    PositionKeyRateExposure::new(
        id(position_suffix),
        VersionRef::new(id(instrument_suffix), version()),
        vec![exposure],
        vec![ContentHash::digest(&[position_suffix as u8])],
        vec![lineage(position_suffix)],
    )
    .unwrap()
}

fn futures_instrument() -> Instrument {
    Instrument::new(InstrumentInput {
        instrument_id: id('I'),
        version: version(),
        owner: OwnerRef::new(id('T'), id('O')),
        kind: InstrumentKind::Futures,
        market: "CFFEX".to_owned(),
        symbol: "TS2609".to_owned(),
        currency: unit('C'),
        calendar: VersionRef::new(id('K'), version()),
    })
    .unwrap()
}

fn decimal(value: &str, scale: u32, unit: UnitRef) -> DecimalValue {
    DecimalValue::new(value, scale, unit).unwrap()
}

fn fixed(value: &str) -> FixedDecimal {
    FixedDecimal::from_scaled(value.parse().unwrap())
}

fn market_time(year: i32, month: u32, day: u32, hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
            .single()
            .unwrap(),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(year, month, day).unwrap(),
    )
    .unwrap()
}

fn lineage(suffix: char) -> LineageRef {
    LineageRef::new(
        id(suffix),
        Some(version()),
        Some(ContentHash::digest(&[suffix as u8])),
    )
    .unwrap()
}

fn unit(suffix: char) -> UnitRef {
    UnitRef::new(id(suffix), version())
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => '1',
        'L' => '2',
        'O' => '0',
        value => value,
    };
    Ulid::new(format!("{PREFIX}{suffix}")).unwrap()
}
