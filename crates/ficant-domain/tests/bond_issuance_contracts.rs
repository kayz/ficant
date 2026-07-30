use chrono::NaiveDate;
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{
    BondTerms, BusinessDayConvention, CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::market::{
    Bond, BondTaxAttributes, IncomeTaxStatus, Instrument, InstrumentInput, InstrumentKind,
    ValueAddedTaxStatus,
};
use ficant_domain::primitives::{DecimalValue, OwnerRef, Ulid, UnitRef, Version, VersionRef};

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{ULID_PREFIX}{suffix}")).expect("fixture ULID is valid")
}

fn version() -> Version {
    Version::new(1).expect("fixture version is valid")
}

fn unit(suffix: char) -> UnitRef {
    UnitRef::new(id(suffix), version())
}

fn decimal(coefficient: &str, scale: u32, unit: UnitRef) -> DecimalValue {
    DecimalValue::new(coefficient, scale, unit).expect("fixture decimal is valid")
}

fn instrument() -> Instrument {
    Instrument::new(InstrumentInput {
        instrument_id: id('D'),
        version: version(),
        owner: OwnerRef::new(id('A'), id('B')),
        kind: InstrumentKind::Bond,
        market: "CN".to_owned(),
        symbol: "SYNTHETIC-BOND".to_owned(),
        currency: unit('C'),
        calendar: VersionRef::new(id('E'), version()),
    })
    .expect("fixture instrument is valid")
}

fn tax_attributes() -> BondTaxAttributes {
    BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt)
}

#[test]
fn bond_exposes_distinct_first_and_current_issuance_dates() {
    let currency = unit('C');
    let first_issue_date = NaiveDate::from_ymd_opt(2025, 8, 7).unwrap();
    let current_issue_date = NaiveDate::from_ymd_opt(2025, 8, 9).unwrap();
    let maturity_date = NaiveDate::from_ymd_opt(2035, 8, 7).unwrap();
    let bond = Bond::with_issuance(
        &instrument(),
        first_issue_date,
        current_issue_date,
        maturity_date,
        decimal("200000000", 0, currency.clone()),
        tax_attributes(),
        decimal("100", 0, currency),
    )
    .expect("a reissued Bond is valid");

    assert_eq!(bond.first_issue_date(), first_issue_date);
    assert_eq!(bond.current_issue_date(), current_issue_date);
    assert_ne!(bond.first_issue_date(), bond.current_issue_date());
    assert_eq!(
        bond.tax_attributes(),
        Some(BondTaxAttributes::new(
            ValueAddedTaxStatus::Exempt,
            IncomeTaxStatus::Exempt,
        ))
    );
    assert!(bond.cumulative_issued_amount().is_positive());
}

#[test]
fn issuance_shape_rejects_reversed_dates_and_invalid_amounts() {
    let currency = unit('C');
    let first_issue_date = NaiveDate::from_ymd_opt(2025, 8, 8).unwrap();
    let maturity_date = NaiveDate::from_ymd_opt(2035, 8, 8).unwrap();
    let reversed = Bond::with_issuance(
        &instrument(),
        first_issue_date,
        NaiveDate::from_ymd_opt(2025, 8, 7).unwrap(),
        maturity_date,
        decimal("200000000", 0, currency.clone()),
        tax_attributes(),
        decimal("100", 0, currency.clone()),
    )
    .expect_err("current issuance before first issuance is invalid");
    assert_eq!(reversed, DomainErrorCode::InvalidEffectiveTime);

    let unit_mismatch = Bond::with_issuance(
        &instrument(),
        first_issue_date,
        first_issue_date,
        maturity_date,
        decimal("200000000", 0, unit('D')),
        tax_attributes(),
        decimal("100", 0, currency),
    )
    .expect_err("cumulative issuance must use the Bond currency unit");
    assert_eq!(unit_mismatch, DomainErrorCode::InvalidValue);
}

#[test]
fn strict_terms_carry_issuance_facts_while_legacy_adapter_stays_unusable_for_tax() {
    let first_issue_date = NaiveDate::from_ymd_opt(2025, 8, 7).unwrap();
    let current_issue_date = NaiveDate::from_ymd_opt(2025, 8, 9).unwrap();
    let maturity_date = NaiveDate::from_ymd_opt(2035, 8, 7).unwrap();
    let terms = BondTerms::with_issuance(
        first_issue_date,
        current_issue_date,
        maturity_date,
        CouponFrequency::Semiannual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        FixedDecimal::from_scaled(25_000_000_000),
        FixedDecimal::from_scaled(100_000_000_000_000),
        FixedDecimal::from_scaled(200_000_000_000_000_000_000),
        tax_attributes(),
    )
    .expect("strict terms are valid");
    assert_eq!(terms.first_issue_date(), first_issue_date);
    assert_eq!(terms.current_issue_date(), current_issue_date);
    assert!(terms.tax_attributes().is_some());

    let legacy = BondTerms::new(
        first_issue_date,
        maturity_date,
        CouponFrequency::Semiannual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        FixedDecimal::from_scaled(25_000_000_000),
        FixedDecimal::from_scaled(100_000_000_000_000),
    )
    .expect("legacy frozen-test adapter remains valid");
    assert_eq!(legacy.first_issue_date(), legacy.current_issue_date());
    assert!(legacy.tax_attributes().is_none());
}
