use chrono::{NaiveDate, TimeZone, Utc};
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{
    AnalyticsMeasures, AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondAnalyticsResult,
    BondTaxBasis, BondTerms, BusinessDayConvention, CalendarBinding, CalendarRequirement,
    CalendarResolution, CouponFrequency, DayCountConvention, DerivedCashflow, FixedDecimal,
};
use ficant_domain::market::{BondTaxAttributes, IncomeTaxStatus, ValueAddedTaxStatus};
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

#[test]
fn bond_terms_preserve_reopening_facts_with_or_without_tax_attributes() {
    let first_issue = NaiveDate::from_ymd_opt(2024, 8, 18).unwrap();
    let current_issue = NaiveDate::from_ymd_opt(2025, 2, 18).unwrap();
    let maturity = NaiveDate::from_ymd_opt(2031, 8, 18).unwrap();
    let coupon = FixedDecimal::from_scaled(22_000_000_000);
    let face = FixedDecimal::from_scaled(100_000_000_000_000);
    let cumulative = FixedDecimal::from_scaled(20_000_000_000_000_000_000_000);
    let tax = BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Taxable);

    let without_tax = BondTerms::with_issuance_and_optional_tax(
        first_issue,
        current_issue,
        maturity,
        CouponFrequency::Semiannual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        coupon,
        face,
        cumulative,
        None,
    )
    .unwrap();
    assert_eq!(without_tax.first_issue_date(), first_issue);
    assert_eq!(without_tax.current_issue_date(), current_issue);
    assert_eq!(without_tax.maturity_date(), maturity);
    assert_eq!(without_tax.face_amount(), face);
    assert_eq!(without_tax.cumulative_issued_amount(), cumulative);
    assert_eq!(without_tax.tax_attributes(), None);

    let with_tax = BondTerms::with_issuance_and_optional_tax(
        first_issue,
        current_issue,
        maturity,
        CouponFrequency::Semiannual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        coupon,
        face,
        cumulative,
        Some(tax),
    )
    .unwrap();
    assert_eq!(
        with_tax,
        BondTerms::with_issuance(
            first_issue,
            current_issue,
            maturity,
            CouponFrequency::Semiannual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            coupon,
            face,
            cumulative,
            tax,
        )
        .unwrap()
    );
    assert_eq!(with_tax.tax_attributes(), Some(tax));
}

#[test]
fn bond_terms_reject_invalid_issuance_dates_and_amounts_with_or_without_tax() {
    let first_issue = NaiveDate::from_ymd_opt(2024, 8, 18).unwrap();
    let current_issue = NaiveDate::from_ymd_opt(2025, 2, 18).unwrap();
    let maturity = NaiveDate::from_ymd_opt(2031, 8, 18).unwrap();
    let earlier_than_first = NaiveDate::from_ymd_opt(2024, 8, 17).unwrap();
    let coupon = FixedDecimal::from_scaled(22_000_000_000);
    let face = FixedDecimal::from_scaled(100_000_000_000_000);
    let cumulative = FixedDecimal::from_scaled(20_000_000_000_000_000_000_000);
    let tax = BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Taxable);

    for tax_opt in [None, Some(tax)] {
        for (current, cum) in [
            (earlier_than_first, cumulative),
            (maturity, cumulative),
            (current_issue, FixedDecimal::ZERO),
            (current_issue, FixedDecimal::from_scaled(-1)),
        ] {
            assert_eq!(
                BondTerms::with_issuance_and_optional_tax(
                    first_issue,
                    current,
                    maturity,
                    CouponFrequency::Semiannual,
                    DayCountConvention::ActActBondIsma,
                    BusinessDayConvention::Following,
                    coupon,
                    face,
                    cum,
                    tax_opt,
                ),
                Err(DomainErrorCode::InvalidValue)
            );
        }
    }
}

#[test]
fn bond_analytics_input_and_result_bind_tax_basis_into_lineage() {
    let issue = NaiveDate::from_ymd_opt(2025, 1, 15).unwrap();
    let settlement = NaiveDate::from_ymd_opt(2025, 7, 15).unwrap();
    let maturity = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
    let clean_price = FixedDecimal::from_scaled(100_000_000_000_000);
    let terms = BondTerms::new(
        issue,
        maturity,
        CouponFrequency::Annual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        FixedDecimal::from_scaled(20_000_000_000),
        clean_price,
    )
    .unwrap();
    let pre_tax_input =
        sample_price_in_input(issue, settlement, maturity, terms.clone(), clean_price);
    assert_eq!(pre_tax_input.tax_basis(), BondTaxBasis::PreTax);

    let net_terms = terms
        .with_coupon_rate(FixedDecimal::from_scaled(15_000_000_000))
        .unwrap();
    let tax_adjusted_input = pre_tax_input
        .with_terms_and_price_in(net_terms, clean_price)
        .unwrap();
    assert_eq!(tax_adjusted_input.tax_basis(), BondTaxBasis::TaxAdjusted);
    assert_eq!(tax_adjusted_input.mode(), AnalyticsMode::PriceIn);

    let same_terms_tax_adjusted = pre_tax_input
        .with_terms_and_price_in(terms, clean_price)
        .unwrap();
    assert_ne!(pre_tax_input, same_terms_tax_adjusted);

    let (cashflows, measures) = sample_result_parts(maturity, clean_price);
    let pre_tax_result = BondAnalyticsResult::new(
        pre_tax_input.clone(),
        CalendarResolution::Exact,
        cashflows.clone(),
        measures.clone(),
    )
    .unwrap();
    assert_eq!(pre_tax_result.tax_basis(), BondTaxBasis::PreTax);
    assert!(pre_tax_result.validate_against(&pre_tax_input).is_ok());
    assert_eq!(
        pre_tax_result.validate_against(&same_terms_tax_adjusted),
        Err(DomainErrorCode::BrokenLineage)
    );

    let tax_adjusted_result = BondAnalyticsResult::new(
        tax_adjusted_input.clone(),
        CalendarResolution::Exact,
        cashflows,
        measures,
    )
    .unwrap();
    assert_eq!(tax_adjusted_result.tax_basis(), BondTaxBasis::TaxAdjusted);
    assert!(
        tax_adjusted_result
            .validate_against(&tax_adjusted_input)
            .is_ok()
    );
    assert_eq!(
        tax_adjusted_result.validate_against(&pre_tax_input),
        Err(DomainErrorCode::BrokenLineage)
    );
}

fn sample_price_in_input(
    issue: NaiveDate,
    settlement: NaiveDate,
    maturity: NaiveDate,
    terms: BondTerms,
    clean_price: FixedDecimal,
) -> BondAnalyticsInput {
    let version = Version::new(1).unwrap();
    let object = |suffix| {
        AnalyticsObjectRef::new(
            VersionRef::new(id(suffix), version),
            ContentHash::digest(suffix.to_string().as_bytes()),
        )
    };
    let instant = Utc.with_ymd_and_hms(2025, 7, 15, 4, 0, 0).unwrap();
    let market_time = MarketTime::new(instant, "Asia/Shanghai", settlement).unwrap();
    let calendar = CalendarBinding::new(
        "cgb-reference-calendar-v1",
        version,
        ContentHash::digest(b"calendar"),
        issue,
        maturity,
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
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
        AnalyticsMode::PriceIn,
        clean_price,
    )
    .unwrap()
}

fn sample_result_parts(
    maturity: NaiveDate,
    clean_price: FixedDecimal,
) -> (Vec<DerivedCashflow>, AnalyticsMeasures) {
    let cashflows = vec![
        DerivedCashflow::new(
            1,
            maturity,
            maturity,
            FixedDecimal::from_scaled(2_000_000_000_000),
            FixedDecimal::from_scaled(100_000_000_000_000),
            FixedDecimal::from_scaled(102_000_000_000_000),
        )
        .unwrap(),
    ];
    let measures = AnalyticsMeasures::new(
        FixedDecimal::from_scaled(1_000_000_000_000),
        clean_price,
        FixedDecimal::from_scaled(20_000_000_000),
        FixedDecimal::from_scaled(500_000_000_000),
        FixedDecimal::from_scaled(490_000_000_000),
        FixedDecimal::from_scaled(300_000_000_000),
        FixedDecimal::from_scaled(5_000_000_000),
    )
    .unwrap();
    (cashflows, measures)
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
