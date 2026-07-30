use std::collections::BTreeMap;
use std::future::Future;
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use chrono::NaiveDate;
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, CouponTaxRate, DefinitionIdentity, DefinitionRepository,
    DefinitionValue, TaxRulePackParser,
};
use ficant_application::use_cases::tax_rule::ResolveTaxRule;
use ficant_application::{ApplicationError, ApplicationErrorCategory, ApplicationErrorDetail};
use ficant_domain::analytics::{AnalyticsObjectRef, FixedDecimal};
use ficant_domain::market::{
    BondTaxAttributes, IncomeTaxStatus, MarketRulePack, MarketRulePackInput, RulePackContent,
    ValueAddedTaxStatus, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::subject::TaxTreatment;

const TYPE_URL: &str = "type.googleapis.com/ficant.market.v1.TaxRulePack";

#[test]
fn exact_tax_binding_selects_first_issue_date_and_subject_profile() {
    let payload = b"synthetic-tax-rules";
    let definitions = Definitions::new([DefinitionValue::MarketRulePack(pack(1, payload))]);
    let resolver = ResolveTaxRule::new(&definitions, &FixtureTaxParser);
    let selected = block_on(resolver.execute(
        &scope(),
        &binding(1, payload),
        time(12),
        date("2025-08-08"),
        taxable_attributes(),
        &TaxTreatment::new("synthetic-vat-taxable", "synthetic-income-taxable").unwrap(),
    ))
    .expect("exact first issue date and profile resolve");
    assert_eq!(
        selected.coupon_tax_rate(),
        FixedDecimal::from_scaled(130_000_000_000)
    );
    assert_eq!(selected.unit(), &unit());
}

#[test]
fn missing_tax_profile_fails_closed_with_a_named_item() {
    let payload = b"synthetic-tax-rules";
    let definitions = Definitions::new([DefinitionValue::MarketRulePack(pack(1, payload))]);
    let resolver = ResolveTaxRule::new(&definitions, &FixtureTaxParser);
    let error = block_on(resolver.execute(
        &scope(),
        &binding(1, payload),
        time(12),
        date("2025-08-08"),
        taxable_attributes(),
        &TaxTreatment::new("synthetic-vat-exempt", "synthetic-income-exempt").unwrap(),
    ))
    .expect_err("unknown profile must not fall back to another rate");
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert!(!error.retryable());
    assert_eq!(
        error.detail(),
        Some(&ApplicationErrorDetail::RulePackItemMissing {
            path: "context.tax_rule_pack.content.coupon_rules[first_issue_date=2025-08-08].rates[vat_profile=synthetic-vat-exempt][income_profile=synthetic-income-exempt]".to_owned(),
        })
    );
}

#[test]
fn exact_tax_binding_rejects_hash_and_effective_time_drift_before_parser() {
    let payload = b"synthetic-tax-rules";
    let definitions = Definitions::new([DefinitionValue::MarketRulePack(pack(1, payload))]);
    let resolver = ResolveTaxRule::new(&definitions, &FixtureTaxParser);
    let hash_drift = block_on(resolver.execute(
        &scope(),
        &AnalyticsObjectRef::new(
            VersionRef::new(id('R'), version(1)),
            ContentHash::digest(b"other-content"),
        ),
        time(12),
        date("2025-08-08"),
        taxable_attributes(),
        &TaxTreatment::new("synthetic-vat-taxable", "synthetic-income-taxable").unwrap(),
    ))
    .expect_err("content-hash drift fails before parser selection");
    assert_eq!(
        hash_drift.category(),
        ApplicationErrorCategory::HashMismatch
    );

    let expired = block_on(resolver.execute(
        &scope(),
        &binding(1, payload),
        time(15),
        date("2025-08-08"),
        taxable_attributes(),
        &TaxTreatment::new("synthetic-vat-taxable", "synthetic-income-taxable").unwrap(),
    ))
    .expect_err("expired pack fails before parser selection");
    assert_eq!(
        expired.category(),
        ApplicationErrorCategory::ValidationFailed
    );
}

struct FixtureTaxParser;

impl TaxRulePackParser for FixtureTaxParser {
    fn market(&self) -> &'static str {
        "CN"
    }

    fn rule_type(&self) -> &'static str {
        "tax"
    }

    fn type_url(&self) -> &'static str {
        TYPE_URL
    }

    fn parse(
        &self,
        content: &RulePackContent,
        first_issue_date: NaiveDate,
        tax_attributes: BondTaxAttributes,
        tax_treatment: &TaxTreatment,
    ) -> Result<CouponTaxRate, ApplicationError> {
        if content.value() != b"synthetic-tax-rules"
            || first_issue_date != date("2025-08-08")
            || tax_attributes != taxable_attributes()
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::ValidationFailed,
                false,
            ));
        }
        if tax_treatment.value_added_tax_profile() != "synthetic-vat-taxable"
            || tax_treatment.income_tax_profile() != "synthetic-income-taxable"
        {
            return Err(ApplicationError::rule_pack_item_missing(format!(
                "context.tax_rule_pack.content.coupon_rules[first_issue_date={first_issue_date}].rates[vat_profile={}][income_profile={}]",
                tax_treatment.value_added_tax_profile(),
                tax_treatment.income_tax_profile(),
            )));
        }
        Ok(CouponTaxRate::new(
            FixedDecimal::from_scaled(130_000_000_000),
            unit(),
        ))
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
        unreachable!("resolver performs only exact reads")
    }

    async fn append_version(
        &self,
        _: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        unreachable!("resolver performs only exact reads")
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
        unreachable!("R3b requires exact RulePack binding")
    }
}

fn pack(version_value: u64, payload: &[u8]) -> MarketRulePack {
    let content = RulePackContent::new(TYPE_URL, payload.to_vec()).unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('R'),
            version: version(version_value),
            owner: owner(),
            market: "CN".to_owned(),
            rule_type: "tax".to_owned(),
            source: "synthetic-r3b-fixture".to_owned(),
            effective: EffectivePeriod::new(time(1), time(15)).unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(payload),
        },
        content,
    )
    .unwrap()
}

fn binding(version_value: u64, payload: &[u8]) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(id('R'), version(version_value)),
        ContentHash::digest(payload),
    )
}

fn taxable_attributes() -> BondTaxAttributes {
    BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Taxable)
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('A'), vec![id('B')]).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('B'))
}

fn unit() -> UnitRef {
    UnitRef::new(id('P'), version(1))
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn version(value: u64) -> Version {
    Version::new(value).unwrap()
}

fn date(value: &str) -> NaiveDate {
    value.parse().unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-04".parse().unwrap(),
    )
    .unwrap()
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}
