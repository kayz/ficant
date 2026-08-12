use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, CouponTaxClaimScope, CouponTaxTreatment,
    DefinitionIdentity, DefinitionRepository, DefinitionValue, TaxRulePackParser,
};
use ficant_application::use_cases::tax_rule::ResolveTaxRule;
use ficant_application::{ApplicationError, RatesRequestEvidence};
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
use ficant_domain::subject::TaxTreatment;

const RATE_UNIT_ID: &str = "01K2CGBVAT0000000000000000";
const SOURCE: &str = "ficant-authority/cgb-interest-tax/v1";
const TYPE_URL: &str = "type.googleapis.com/ficant.market.v1.TaxRulePackV2";
const PAYLOAD: &[u8] = b"r5e-authority-shaped-test-payload";

#[tokio::test]
async fn cutoff_and_reissuance_select_exact_treatments_and_bind_the_request_fingerprint() {
    let definitions = Definitions::new([DefinitionValue::MarketRulePack(pack(SOURCE, false))]);
    let parser = AuthorityShapedParser;
    assert_eq!(
        parser.expected_rate_unit(),
        Some((RATE_UNIT_ID, 1, "RATE", "rate", 12, 18))
    );
    let resolver = ResolveTaxRule::new(&definitions, &parser);
    let subject = TaxTreatment::new("cn-vat-general-taxpayer", "cn-cgb-interest-cit-exempt")
        .expect("approved Subject profile pair is valid");

    let exempt = resolver
        .execute(
            &scope(),
            &binding(ContentHash::digest(PAYLOAD)),
            valuation(),
            date("2025-08-07"),
            attributes(ValueAddedTaxStatus::Exempt),
            &subject,
        )
        .await
        .expect("pre-cutoff Bond selects the exempt treatment");
    let taxable = resolver
        .execute(
            &scope(),
            &binding(ContentHash::digest(PAYLOAD)),
            valuation(),
            date("2025-08-08"),
            attributes(ValueAddedTaxStatus::Taxable),
            &subject,
        )
        .await
        .expect("cutoff-day Bond selects the taxable treatment");
    let reissuance = resolver
        .execute(
            &scope(),
            &binding(ContentHash::digest(PAYLOAD)),
            valuation(),
            date("2025-08-08"),
            attributes(ValueAddedTaxStatus::Taxable),
            &subject,
        )
        .await
        .expect("a later reissuance inherits its taxable first-issue classification");

    assert_eq!(exempt.value_added_tax_rate(), FixedDecimal::ZERO);
    assert_eq!(taxable.value_added_tax_rate(), fixed("6", 2));
    assert_eq!(taxable, reissuance);
    assert_eq!(
        taxable.claim_scope(),
        CouponTaxClaimScope::CouponOutputVatBeforeInputCredit
    );
    assert_eq!(
        taxable
            .adjust_coupon(fixed("125", 2))
            .expect("approved ties-even division is representable"),
        fixed("1179245283019", 12)
    );

    let input = bond_input();
    let exempt_proof = RatesRequestEvidence::bond(vec![], &valuation(), &input, &exempt)
        .expect("exempt treatment is fingerprinted");
    let taxable_proof = RatesRequestEvidence::bond(vec![], &valuation(), &input, &taxable)
        .expect("taxable treatment is fingerprinted");
    assert_ne!(
        exempt_proof.canonical_parameters_sha256(),
        taxable_proof.canonical_parameters_sha256(),
        "the complete selected treatment must enter canonical parameters"
    );
    assert_ne!(
        exempt_proof.request_fingerprint(),
        taxable_proof.request_fingerprint(),
        "a treatment change must change the request fingerprint"
    );
}

#[tokio::test]
async fn authority_source_window_hash_profile_and_bond_attribute_drift_fail_closed() {
    let parser = AuthorityShapedParser;
    let approved_subject =
        TaxTreatment::new("cn-vat-general-taxpayer", "cn-cgb-interest-cit-exempt")
            .expect("approved profile pair is valid");

    for (name, definitions, exact_binding, attributes, subject) in [
        (
            "source drift",
            Definitions::new([DefinitionValue::MarketRulePack(pack("unapproved", false))]),
            binding(ContentHash::digest(PAYLOAD)),
            attributes(ValueAddedTaxStatus::Taxable),
            approved_subject.clone(),
        ),
        (
            "effective-window drift",
            Definitions::new([DefinitionValue::MarketRulePack(pack(SOURCE, true))]),
            binding(ContentHash::digest(PAYLOAD)),
            attributes(ValueAddedTaxStatus::Taxable),
            approved_subject.clone(),
        ),
        (
            "binding hash drift",
            Definitions::new([DefinitionValue::MarketRulePack(pack(SOURCE, false))]),
            binding(ContentHash::digest(b"drift")),
            attributes(ValueAddedTaxStatus::Taxable),
            approved_subject.clone(),
        ),
        (
            "Bond attribute drift",
            Definitions::new([DefinitionValue::MarketRulePack(pack(SOURCE, false))]),
            binding(ContentHash::digest(PAYLOAD)),
            attributes(ValueAddedTaxStatus::Exempt),
            approved_subject.clone(),
        ),
        (
            "Subject profile drift",
            Definitions::new([DefinitionValue::MarketRulePack(pack(SOURCE, false))]),
            binding(ContentHash::digest(PAYLOAD)),
            attributes(ValueAddedTaxStatus::Taxable),
            TaxTreatment::new("small-scale", "cn-cgb-interest-cit-exempt")
                .expect("the drifted profile is individually well-formed"),
        ),
    ] {
        let resolver = ResolveTaxRule::new(&definitions, &parser);
        assert!(
            resolver
                .execute(
                    &scope(),
                    &exact_binding,
                    valuation(),
                    date("2025-08-08"),
                    attributes,
                    &subject,
                )
                .await
                .is_err(),
            "{name} reached the post-materialization handoff"
        );
    }
}

struct AuthorityShapedParser;

impl TaxRulePackParser for AuthorityShapedParser {
    fn market(&self) -> &'static str {
        "CN"
    }

    fn rule_type(&self) -> &'static str {
        "cgb-interest-tax"
    }

    fn type_url(&self) -> &'static str {
        TYPE_URL
    }

    fn expected_source(&self) -> Option<&'static str> {
        Some(SOURCE)
    }

    fn expected_effective_window(&self) -> Option<(&'static str, &'static str)> {
        Some(("2026-01-01T00:00:00+08:00", "2028-01-01T00:00:00+08:00"))
    }

    fn expected_rate_unit(
        &self,
    ) -> Option<(&'static str, u64, &'static str, &'static str, u32, u32)> {
        Some((RATE_UNIT_ID, 1, "RATE", "rate", 12, 18))
    }

    fn parse(
        &self,
        content: &RulePackContent,
        first_issue_date: NaiveDate,
        tax_attributes: BondTaxAttributes,
        tax_treatment: &TaxTreatment,
    ) -> Result<CouponTaxTreatment, ApplicationError> {
        let taxable = first_issue_date >= date("2025-08-08");
        let expected = attributes(if taxable {
            ValueAddedTaxStatus::Taxable
        } else {
            ValueAddedTaxStatus::Exempt
        });
        if content.type_url() != TYPE_URL
            || content.value() != PAYLOAD
            || tax_attributes != expected
            || tax_treatment.value_added_tax_profile() != "cn-vat-general-taxpayer"
            || tax_treatment.income_tax_profile() != "cn-cgb-interest-cit-exempt"
        {
            return Err(ApplicationError::new(
                ficant_application::ApplicationErrorCategory::ValidationFailed,
                false,
            ));
        }
        CouponTaxTreatment::vat_included(
            if taxable {
                fixed("6", 2)
            } else {
                FixedDecimal::ZERO
            },
            FixedDecimal::ZERO,
            rate_unit(),
            semantic_hash(),
        )
    }
}

struct Definitions {
    values: BTreeMap<(String, u64), DefinitionValue>,
}

impl Definitions {
    fn new(values: impl IntoIterator<Item = DefinitionValue>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|value| ((value.identity().to_owned(), value.version()), value))
                .collect(),
        }
    }
}

#[async_trait]
impl DefinitionRepository for Definitions {
    async fn create_identity(&self, _: DefinitionIdentity) -> Result<(), ApplicationError> {
        unreachable!("the R5E materializer performs exact reads only")
    }

    async fn append_version(
        &self,
        _: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        unreachable!("the R5E materializer performs exact reads only")
    }

    async fn get_version(
        &self,
        _: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Ok(self
            .values
            .get(&(definition_id.as_str().to_owned(), version.get()))
            .cloned())
    }

    async fn resolve_as_of(
        &self,
        _: &AccessScope,
        _: Ulid,
        _: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        unreachable!("the R5E materializer never resolves a latest version")
    }
}

fn pack(source: &str, drift_window: bool) -> MarketRulePack {
    let content = RulePackContent::new(TYPE_URL, PAYLOAD.to_vec()).expect("content is valid");
    let from = if drift_window {
        market_time("2026-01-02T00:00:00+08:00", "2026-01-02")
    } else {
        market_time("2026-01-01T00:00:00+08:00", "2026-01-01")
    };
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('R'),
            version: version(1),
            owner: owner(),
            market: "CN".to_owned(),
            rule_type: "cgb-interest-tax".to_owned(),
            source: source.to_owned(),
            effective: EffectivePeriod::new(
                from,
                market_time("2028-01-01T00:00:00+08:00", "2028-01-01"),
            )
            .expect("effective period is valid"),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(PAYLOAD),
        },
        content,
    )
    .expect("RulePack is valid")
}

fn bond_input() -> BondAnalyticsInput {
    BondAnalyticsInput::new(
        owner(),
        object('B'),
        object('R'),
        object('S'),
        valuation(),
        date("2026-07-21"),
        CalendarRequirement::ReferenceReplay,
        CalendarBinding::new(
            id('C').to_string(),
            version(1),
            ContentHash::digest(b"calendar"),
            date("2020-01-01"),
            date("2040-01-01"),
            vec![],
            vec![],
        )
        .expect("calendar is valid"),
        BondTerms::with_issuance(
            date("2025-08-08"),
            date("2026-02-08"),
            date("2034-08-08"),
            CouponFrequency::Semiannual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            fixed("25", 3),
            fixed("100", 0),
            fixed("100000000", 0),
            attributes(ValueAddedTaxStatus::Taxable),
        )
        .expect("Bond terms are valid"),
        AnalyticsMode::PriceIn,
        fixed("10020", 2),
    )
    .expect("Bond input is valid")
}

fn binding(hash: ContentHash) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(VersionRef::new(id('R'), version(1)), hash)
}

fn object(suffix: char) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(id(suffix), version(1)),
        ContentHash::digest(format!("object-{suffix}").as_bytes()),
    )
}

fn attributes(vat: ValueAddedTaxStatus) -> BondTaxAttributes {
    BondTaxAttributes::new(vat, IncomeTaxStatus::Exempt)
}

fn semantic_hash() -> ContentHash {
    ContentHash::from_bytes(&[
        0x54, 0xfa, 0x5a, 0xdb, 0xeb, 0x8b, 0x16, 0x4d, 0xc7, 0x79, 0xec, 0xc2, 0x50, 0xab, 0x62,
        0x2a, 0xb5, 0x74, 0xcd, 0xeb, 0x36, 0xf2, 0xb6, 0xda, 0x58, 0xf4, 0xd8, 0x77, 0xce, 0x51,
        0x06, 0x0a,
    ])
    .expect("authority hash is exactly 32 bytes")
}

fn rate_unit() -> UnitRef {
    UnitRef::new(
        Ulid::new(RATE_UNIT_ID).expect("Unit ID is valid"),
        version(1),
    )
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('A'), vec![id('O')]).expect("scope is valid")
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('O'))
}

fn valuation() -> MarketTime {
    market_time("2026-07-20T12:00:00+08:00", "2026-07-20")
}

fn market_time(instant: &str, local_date: &str) -> MarketTime {
    MarketTime::new(
        instant
            .parse::<chrono::DateTime<chrono::FixedOffset>>()
            .expect("instant is valid")
            .with_timezone(&Utc),
        "Asia/Shanghai",
        date(local_date),
    )
    .expect("MarketTime is valid")
}

fn fixed(coefficient: &str, scale: u32) -> FixedDecimal {
    let coefficient = coefficient.parse::<i128>().expect("coefficient is valid");
    FixedDecimal::from_scaled(
        coefficient
            .checked_mul(10_i128.pow(12 - scale))
            .expect("fixture decimal is representable"),
    )
}

fn id(suffix: char) -> Ulid {
    let suffix = if suffix == 'O' { '4' } else { suffix };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).expect("fixture ULID is valid")
}

fn version(value: u64) -> Version {
    Version::new(value).expect("version is nonzero")
}

fn date(value: &str) -> NaiveDate {
    value.parse().expect("date is valid")
}
