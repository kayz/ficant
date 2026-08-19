use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{BondAnalyticsEngine, CouponTaxTreatment, TaxRulePackParser};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondTerms, BusinessDayConvention,
    CalendarBinding, CalendarRequirement, CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::market::{
    BondTaxAttributes, IncomeTaxStatus, MarketRulePack, MarketRulePackInput, RulePackContent,
    ValueAddedTaxStatus, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::subject::{
    AccessSet, FundingTier, Subject, SubjectRecord, SubjectVersion, TaxTreatment,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};
use ficant_fixed_income_native::NativeBondAnalyticsEngine;
use serde_json::Value;

const MARKET: &str = "XZ";
const RULE_TYPE: &str = "fictional-bond-coupon-treatment";
const TYPE_URL: &str = "type.ficant.test/fictional.rates.v1.CouponRulePack";
const PAYLOAD: &[u8] =
    include_bytes!("../../../domain-packs/fictional-rates/fictional-coupon-rule-pack-v1.json");

#[test]
fn fictional_rule_pack_and_subject_complete_cashflows_and_valuation_without_core_changes() {
    let owner = owner();
    let subject = subject(owner.clone());
    assert_eq!(subject.version().access_set().market_codes(), &[MARKET]);

    let pack = rule_pack(owner.clone());
    let content = pack
        .content()
        .expect("fictional RulePack carries typed content");
    pack.content_hash()
        .verify(content.value())
        .expect("RulePack payload is content addressed");
    assert_eq!(pack.market(), FictionalCouponParser.market());
    assert_eq!(pack.rule_type(), FictionalCouponParser.rule_type());
    assert!(pack.effective().from().instant() <= valuation().instant());
    assert!(valuation().instant() < pack.effective().to().instant());

    let treatment = FictionalCouponParser
        .parse(
            content,
            date(2025, 1, 2),
            BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Exempt),
            subject.version().tax_treatment(),
        )
        .expect("fictional RulePack and Subject profile materialize one treatment");

    let engine = NativeBondAnalyticsEngine;
    let input = bond_input(owner, &pack, subject.version().reference().clone());
    let pre_tax = engine
        .calculate(&input)
        .expect("generic native engine prices the fictional bond");
    assert_eq!(pre_tax.cashflows().len(), 3);
    assert!(pre_tax.measures().clean_price().is_positive());
    assert!(pre_tax.measures().yield_to_maturity().is_positive());
    assert!(pre_tax.measures().modified_duration().is_positive());
    assert!(pre_tax.measures().convexity().is_positive());
    assert!(pre_tax.measures().dv01().is_positive());

    let gross_coupon = pre_tax.cashflows()[0].coupon();
    let net_coupon = treatment
        .adjust_coupon(gross_coupon)
        .expect("fictional treatment adjusts the coupon exactly");
    assert_eq!(net_coupon, fixed("225", 2));
    let adjusted_rate = net_coupon
        .checked_div_round_ties_even(input.terms().face_amount())
        .expect("fixture rate is representable");
    let after_tax_terms = input
        .terms()
        .with_coupon_rate(adjusted_rate)
        .expect("provider-neutral terms accept the parsed rate");
    let after_tax_input = input
        .with_terms_and_price_in(after_tax_terms, pre_tax.measures().clean_price())
        .expect("after-tax valuation reuses the exact market price");
    let after_tax = engine
        .calculate(&after_tax_input)
        .expect("generic native engine reprices the parsed treatment");

    assert_eq!(after_tax.cashflows().len(), pre_tax.cashflows().len());
    assert_eq!(after_tax.cashflows()[0].coupon(), net_coupon);
    assert!(
        after_tax
            .measures()
            .clean_price()
            .scaled()
            .abs_diff(pre_tax.measures().clean_price().scaled())
            <= 100,
        "the existing Decimal↔C ABI conversion boundary preserves price-in to 1e-10"
    );
    assert!(
        after_tax.measures().yield_to_maturity() < pre_tax.measures().yield_to_maturity(),
        "the Subject-selected lower coupon changes the valuation result"
    );

    let replay = engine
        .calculate(&after_tax_input)
        .expect("identical fictional inputs replay");
    assert_eq!(after_tax, replay);
}

#[test]
fn fictional_rule_pack_or_subject_profile_drift_fails_before_the_engine() {
    let content = rule_pack(owner())
        .content()
        .expect("fixture content")
        .clone();
    for (name, treatment, attributes) in [
        (
            "Subject profile",
            TaxTreatment::new("fictional-vat-exempt", "fictional-income-exempt").unwrap(),
            BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Exempt),
        ),
        (
            "Bond tax attributes",
            TaxTreatment::new("fictional-vat-standard", "fictional-income-exempt").unwrap(),
            BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt),
        ),
    ] {
        let error = FictionalCouponParser
            .parse(&content, date(2025, 1, 2), attributes, &treatment)
            .expect_err("drift must fail before any numerical handoff");
        assert_eq!(
            error.category(),
            ApplicationErrorCategory::ValidationFailed,
            "{name} drift returned the wrong category"
        );
    }

    let drifted = RulePackContent::new(TYPE_URL, b"{}".to_vec()).unwrap();
    assert_eq!(
        FictionalCouponParser
            .parse(
                &drifted,
                date(2025, 1, 2),
                BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Exempt,),
                subject(owner()).version().tax_treatment(),
            )
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ValidationFailed
    );
}

#[derive(Clone, Copy)]
struct FictionalCouponParser;

impl TaxRulePackParser for FictionalCouponParser {
    fn market(&self) -> &'static str {
        MARKET
    }

    fn rule_type(&self) -> &'static str {
        RULE_TYPE
    }

    fn type_url(&self) -> &'static str {
        TYPE_URL
    }

    fn parse(
        &self,
        content: &RulePackContent,
        _first_issue_date: NaiveDate,
        tax_attributes: BondTaxAttributes,
        tax_treatment: &TaxTreatment,
    ) -> Result<CouponTaxTreatment, ApplicationError> {
        if content.type_url() != TYPE_URL
            || content.value() != PAYLOAD
            || tax_attributes
                != BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Exempt)
            || tax_treatment.value_added_tax_profile() != "fictional-vat-standard"
            || tax_treatment.income_tax_profile() != "fictional-income-exempt"
        {
            return Err(invalid());
        }
        let value: Value = serde_json::from_slice(content.value()).map_err(|_| invalid())?;
        if value["schema"] != "fictional.rates.v1"
            || value["market"] != MARKET
            || value["rule_type"] != RULE_TYPE
            || value["subject_profile"] != "fictional-vat-standard/fictional-income-exempt"
            || value["coupon_retained_rate"]["coefficient"] != "9"
            || value["coupon_retained_rate"]["scale"] != 1
        {
            return Err(invalid());
        }
        Ok(CouponTaxTreatment::legacy_retained_rate(
            fixed("1", 1),
            UnitRef::new(id('U'), version(1)),
        ))
    }
}

fn rule_pack(owner: OwnerRef) -> MarketRulePack {
    let content = RulePackContent::new(TYPE_URL, PAYLOAD.to_vec()).unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('R'),
            version: version(1),
            owner,
            market: MARKET.to_owned(),
            rule_type: RULE_TYPE.to_owned(),
            source: "fixture://fictional-rates/rule-pack-v1".to_owned(),
            effective: EffectivePeriod::new(market_time(2024, 1, 1), market_time(2030, 1, 1))
                .unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(PAYLOAD),
        },
        content,
    )
    .unwrap()
}

fn subject(owner: OwnerRef) -> SubjectRecord {
    let subject = Subject::new_owned(id('S'), owner, "Fictional market taxable investor").unwrap();
    SubjectRecord::new(
        subject,
        SubjectVersion::new(
            VersionRef::new(id('S'), version(1)),
            AccessSet::new([MARKET], ["bond-analytics"]).unwrap(),
            FundingTier::ROnly,
            TaxTreatment::new("fictional-vat-standard", "fictional-income-exempt").unwrap(),
            "fictional total-return assessment",
            "unlevered fictional balance sheet",
            None,
        )
        .unwrap(),
    )
    .unwrap()
}

fn bond_input(
    owner: OwnerRef,
    pack: &MarketRulePack,
    subject_ref: VersionRef,
) -> BondAnalyticsInput {
    BondAnalyticsInput::new(
        owner,
        AnalyticsObjectRef::new(
            VersionRef::new(id('B'), version(1)),
            ContentHash::digest(b"fictional-bond-v1"),
        ),
        AnalyticsObjectRef::new(
            VersionRef::new(
                Ulid::new(pack.identity()).unwrap(),
                Version::new(pack.version()).unwrap(),
            ),
            pack.content_hash().clone(),
        ),
        AnalyticsObjectRef::new(subject_ref, ContentHash::digest(b"fictional-subject-v1")),
        valuation(),
        date(2026, 7, 21),
        CalendarRequirement::ExactMarket,
        CalendarBinding::new(
            id('C').to_string(),
            version(1),
            ContentHash::digest(b"fictional-calendar-v1"),
            date(2024, 1, 1),
            date(2030, 12, 31),
            vec![],
            vec![],
        )
        .unwrap(),
        BondTerms::with_issuance(
            date(2025, 1, 2),
            date(2025, 1, 2),
            date(2029, 1, 2),
            CouponFrequency::Annual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            fixed("25", 3),
            fixed("100", 0),
            fixed("1000000", 0),
            BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Exempt),
        )
        .unwrap(),
        AnalyticsMode::YieldIn,
        fixed("3", 2),
    )
    .unwrap()
}

fn valuation() -> MarketTime {
    market_time(2026, 7, 20)
}

fn market_time(year: i32, month: u32, day: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, month, day, 4, 0, 0).unwrap(),
        "Asia/Shanghai",
        date(year, month, day),
    )
    .unwrap()
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn fixed(coefficient: &str, scale: u32) -> FixedDecimal {
    let coefficient = coefficient.parse::<i128>().unwrap();
    FixedDecimal::from_scaled(coefficient * 10_i128.pow(12 - scale))
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('O'))
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'O' => '4',
        'U' => '7',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn version(value: u64) -> Version {
    Version::new(value).unwrap()
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
