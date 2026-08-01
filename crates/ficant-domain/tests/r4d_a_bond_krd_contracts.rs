use chrono::{NaiveDate, TimeZone, Utc};
use ficant_domain::ContentAddressed;
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::market::{
    ArtifactInputKind, Bond, BondBusinessDayConvention, BondCouponFrequency,
    BondDayCountConvention, BondPricingTerms, BondTaxAttributes, CurveSnapshot, CurveSnapshotInput,
    IncomeTaxStatus, Instrument, InstrumentInput, InstrumentKind, ValueAddedTaxStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    FactorDv01, PositionKeyRateExposure, SensitivityDirection, aggregate_bond_key_rate_exposures,
    key_rate_dv01,
};

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

#[test]
fn registered_bond_pricing_terms_are_complete_and_legacy_bonds_remain_ineligible() {
    let instrument = instrument();
    let currency = unit('C');
    let legacy = Bond::with_issuance(
        &instrument,
        date(2024, 1, 15),
        date(2024, 1, 15),
        date(2034, 1, 15),
        decimal("100000000", 0, currency.clone()),
        BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt),
        decimal("100", 0, currency),
    )
    .unwrap();
    assert!(legacy.pricing_terms().is_none());

    let priced = legacy
        .with_pricing_terms(
            BondPricingTerms::new(
                decimal("25", 3, unit('R')),
                BondCouponFrequency::Semiannual,
                BondDayCountConvention::ActActBondIsma,
                BondBusinessDayConvention::Following,
            )
            .unwrap(),
        )
        .unwrap();
    let terms = priced
        .pricing_terms()
        .expect("all pricing terms are present");
    assert_eq!(terms.coupon_rate().coefficient(), "25");
    assert_eq!(terms.coupon_rate().scale(), 3);
    assert_eq!(terms.frequency(), BondCouponFrequency::Semiannual);
    assert_eq!(terms.day_count(), BondDayCountConvention::ActActBondIsma);
    assert_eq!(terms.business_day(), BondBusinessDayConvention::Following);
}

#[test]
fn curve_snapshot_knowledge_time_and_family_are_explicit_and_fail_closed() {
    let as_of = market_time(2026, 8, 3, 1);
    let legacy = CurveSnapshot::new(CurveSnapshotInput {
        curve_snapshot_id: id('S'),
        owner: owner(),
        as_of: as_of.clone(),
        currency: unit('C'),
        curve_kind: "YTM".to_owned(),
        calendar: VersionRef::new(id('K'), version()),
        rule_pack: VersionRef::new(id('R'), version()),
        point_schema: "legacy.fixture".to_owned(),
        content_hash: ContentHash::digest(b"curve-points"),
        lineage: vec![lineage('L')],
        input_kind: ArtifactInputKind::ExternalFixture,
    })
    .unwrap();
    assert!(legacy.visible_at().is_none());
    assert!(legacy.curve_family_id().is_none());

    let visible_at = market_time(2026, 8, 3, 2);
    let complete = legacy
        .with_knowledge_time(visible_at.clone(), "cn.gov.yield-curve")
        .unwrap();
    assert_eq!(complete.visible_at(), Some(&visible_at));
    assert_eq!(complete.curve_family_id(), Some("cn.gov.yield-curve"));

    let earlier = market_time(2026, 8, 3, 0);
    assert!(
        complete
            .clone()
            .with_knowledge_time(earlier, "cn.gov.yield-curve")
            .is_err()
    );
}

#[test]
fn direction_formulas_and_bond_only_totals_are_exact_and_stably_sorted() {
    let base = fixed("100000000000000");
    let up = fixed("99900000000000");
    let down = fixed("100100000000000");
    let bump = FixedDecimal::ONE;
    let central = key_rate_dv01(base, up, down, bump, SensitivityDirection::Central).unwrap();
    let one_sided_up = key_rate_dv01(base, up, down, bump, SensitivityDirection::Up).unwrap();
    let one_sided_down = key_rate_dv01(base, up, down, bump, SensitivityDirection::Down).unwrap();
    assert_eq!(central, fixed("100000000000"));
    assert_eq!(one_sided_up, central);
    assert_eq!(one_sided_down, central);

    let dv01 = unit('D');
    let ten = FactorDv01::new(
        "cn.gov.yield.10y",
        ContentHash::digest(b"factor-10y"),
        fixed("200000000000"),
        dv01.clone(),
    )
    .unwrap();
    let five = FactorDv01::new(
        "cn.gov.yield.5y",
        ContentHash::digest(b"factor-5y"),
        fixed("100000000000"),
        dv01.clone(),
    )
    .unwrap();
    let p1 = PositionKeyRateExposure::new(
        id('P'),
        VersionRef::new(id('I'), version()),
        vec![five.clone(), ten.clone()],
        vec![lineage('A')],
    )
    .unwrap();
    let p2 = PositionKeyRateExposure::new(
        id('Q'),
        VersionRef::new(id('J'), version()),
        vec![
            FactorDv01::new(
                "cn.gov.yield.5y",
                five.factor_definition_hash().clone(),
                fixed("-50000000000"),
                dv01.clone(),
            )
            .unwrap(),
            FactorDv01::new(
                "cn.gov.yield.10y",
                ten.factor_definition_hash().clone(),
                FixedDecimal::ZERO,
                dv01,
            )
            .unwrap(),
        ],
        vec![lineage('B')],
    )
    .unwrap();

    let totals = aggregate_bond_key_rate_exposures(&[p1.clone(), p2.clone()]).unwrap();
    assert_eq!(totals.len(), 2);
    assert_eq!(totals[0].factor_id(), "cn.gov.yield.5y");
    assert_eq!(totals[0].value(), fixed("50000000000"));
    assert_eq!(totals[1].factor_id(), "cn.gov.yield.10y");
    assert_eq!(totals[1].value(), fixed("200000000000"));
    assert_ne!(p1.content_hash(), p2.content_hash());
}

fn instrument() -> Instrument {
    Instrument::new(InstrumentInput {
        instrument_id: id('I'),
        version: version(),
        owner: owner(),
        kind: InstrumentKind::Bond,
        market: "CN".to_owned(),
        symbol: "BOND-10Y".to_owned(),
        currency: unit('C'),
        calendar: VersionRef::new(id('K'), version()),
    })
    .unwrap()
}

fn fixed(value: &str) -> FixedDecimal {
    FixedDecimal::from_scaled(value.parse().unwrap())
}

fn decimal(value: &str, scale: u32, unit: UnitRef) -> DecimalValue {
    DecimalValue::new(value, scale, unit).unwrap()
}

fn market_time(year: i32, month: u32, day: u32, hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
            .single()
            .unwrap(),
        "Asia/Shanghai",
        date(year, month, day),
    )
    .unwrap()
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn lineage(suffix: char) -> LineageRef {
    LineageRef::new(
        id(suffix),
        Some(version()),
        Some(ContentHash::digest(&[suffix as u8])),
    )
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('O'))
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
        'U' => '3',
        value => value,
    };
    Ulid::new(format!("{PREFIX}{suffix}")).unwrap()
}
