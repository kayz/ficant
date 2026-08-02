use ficant_domain::ContentAddressed;
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::market::PriceSourceType;
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    AccountingClassification, AccountingClassificationState, CoverageDeclaration, FactorDv01,
    PortfolioKeyRateExposure, Position, PositionHoldingForm, PositionInput,
    PositionKeyRateExposure, PriceSourceCount, PriceSourceSummary, RiskAlgorithmBinding,
};

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

#[test]
fn coverage_groups_gross_economic_value_by_exact_unit_and_keeps_the_imported_denominator() {
    let imported = vec![
        position('P', 'I', "1", "-100", unit('C')),
        position('Q', 'J', "2", "50", unit('C')),
        position('R', 'K', "3", "25", unit('D')),
    ];
    let source = mixed_source_summary();
    let coverage = CoverageDeclaration::for_complete_positions(
        &imported,
        &[id('P'), id('R')],
        Some(source.clone()),
        1,
    )
    .unwrap();

    assert_eq!(coverage.imported_position_count(), 3);
    assert_eq!(coverage.participating_position_count(), 2);
    assert_eq!(coverage.missing_critical_field_record_count(), 0);
    assert_eq!(coverage.source_confidence(), Some(&source));
    assert_eq!(coverage.distinct_external_data_source_version_count(), 1);
    assert_eq!(
        values(coverage.imported_gross_economic_value_by_unit()),
        vec![(unit('C'), "150"), (unit('D'), "25")]
    );
    assert_eq!(
        values(coverage.participating_gross_economic_value_by_unit()),
        vec![(unit('C'), "100"), (unit('D'), "25")]
    );

    assert!(
        CoverageDeclaration::for_complete_positions(
            &imported,
            &[id('R'), id('P')],
            Some(source.clone()),
            1,
        )
        .is_err(),
        "participating ids must be unique and stably sorted"
    );
    assert!(
        CoverageDeclaration::for_complete_positions(&imported, &[id('P')], None, 1).is_err(),
        "an external source count cannot exist without a source distribution"
    );
}

#[test]
fn portfolio_rejects_a_drifting_source_pair_and_hashes_the_complete_coverage() {
    let imported = vec![
        position('P', 'I', "1", "100", unit('C')),
        position('Q', 'J', "2", "50", unit('C')),
    ];
    let exposures = vec![exposure('P', 'I'), exposure('Q', 'J')];
    let curve_only = PriceSourceSummary::new(vec![
        PriceSourceCount::new(PriceSourceType::CurveInterpolation, 2).unwrap(),
    ])
    .unwrap();
    let coverage = CoverageDeclaration::for_complete_positions(
        &imported,
        &[id('P'), id('Q')],
        Some(curve_only.clone()),
        0,
    )
    .unwrap();
    let portfolio = PortfolioKeyRateExposure::new(
        id('S'),
        id('C'),
        exposures.clone(),
        algorithm(),
        (curve_only.clone(), coverage),
        vec![lineage('L')],
    )
    .unwrap();

    let changed_imported = vec![
        position('P', 'I', "1", "101", unit('C')),
        position('Q', 'J', "2", "50", unit('C')),
    ];
    let changed_coverage = CoverageDeclaration::for_complete_positions(
        &changed_imported,
        &[id('P'), id('Q')],
        Some(curve_only.clone()),
        0,
    )
    .unwrap();
    let changed = PortfolioKeyRateExposure::new(
        id('S'),
        id('C'),
        exposures.clone(),
        algorithm(),
        (curve_only.clone(), changed_coverage),
        vec![lineage('L')],
    )
    .unwrap();
    assert_ne!(portfolio.content_hash(), changed.content_hash());

    let mismatched = CoverageDeclaration::for_complete_positions(
        &imported,
        &[id('P'), id('Q')],
        Some(mixed_source_summary()),
        1,
    )
    .unwrap();
    assert!(
        PortfolioKeyRateExposure::new(
            id('S'),
            id('C'),
            exposures,
            algorithm(),
            (curve_only, mismatched),
            vec![lineage('L')],
        )
        .is_err(),
        "the top-level AC15 marker and nested coverage marker cannot drift"
    );
}

fn position(
    position_suffix: char,
    instrument_suffix: char,
    quantity: &str,
    economic_value: &str,
    money_unit: UnitRef,
) -> Position {
    Position::new(PositionInput {
        position_id: id(position_suffix),
        instrument_ref: VersionRef::new(id(instrument_suffix), version()),
        quantity: decimal(quantity, unit('N')),
        economic_value: decimal(economic_value, money_unit.clone()),
        economic_pnl: decimal("0", money_unit.clone()),
        accounting_pnl: decimal("0", money_unit.clone()),
        capital_requirement: decimal("1", money_unit),
        accounting_classification: AccountingClassification::new(
            AccountingClassificationState::Classified,
            Some(ficant_domain::research::AccountingBook::Ac),
        )
        .unwrap(),
        holding_form: PositionHoldingForm::Owned,
    })
    .unwrap()
}

fn exposure(position_suffix: char, instrument_suffix: char) -> PositionKeyRateExposure {
    PositionKeyRateExposure::new(
        id(position_suffix),
        VersionRef::new(id(instrument_suffix), version()),
        vec![
            FactorDv01::new(
                "cn.gov.yield.10y",
                ContentHash::digest(b"factor-10y"),
                FixedDecimal::from_scaled(1),
                unit('V'),
            )
            .unwrap(),
        ],
        vec![ContentHash::digest(&[position_suffix as u8])],
        vec![lineage(position_suffix)],
    )
    .unwrap()
}

fn algorithm() -> RiskAlgorithmBinding {
    RiskAlgorithmBinding::new(
        "ficant.fixed-rate-bond.key-rate-yield",
        1,
        "linear-ytm-registered-bond-v1",
    )
    .unwrap()
}

fn mixed_source_summary() -> PriceSourceSummary {
    PriceSourceSummary::new(vec![
        PriceSourceCount::new(PriceSourceType::ActiveQuote, 2).unwrap(),
        PriceSourceCount::new(PriceSourceType::CurveInterpolation, 2).unwrap(),
    ])
    .unwrap()
}

fn values(values: &[DecimalValue]) -> Vec<(UnitRef, &str)> {
    values
        .iter()
        .map(|value| (value.unit().clone(), value.coefficient()))
        .collect()
}

fn decimal(value: &str, unit: UnitRef) -> DecimalValue {
    DecimalValue::new(value, 0, unit).unwrap()
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
        value => value,
    };
    Ulid::new(format!("{PREFIX}{suffix}")).unwrap()
}
