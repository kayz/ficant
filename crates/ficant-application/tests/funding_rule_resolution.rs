use std::collections::BTreeMap;
use std::future::Future;
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, DefinitionIdentity, DefinitionRepository,
    DefinitionValue, FundingRate, FundingRulePackParser, SubjectRepository,
};
use ficant_application::use_cases::{
    funding_rule::ResolveFundingRule, subject_resolution::ResolveSubject,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, ApplicationErrorDetail};
use ficant_domain::analytics::{AnalyticsObjectRef, FixedDecimal};
use ficant_domain::market::{
    MarketRulePack, MarketRulePackInput, RulePackContent, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::subject::{
    AccessSet, FundingTier, Subject, SubjectRecord, SubjectStateSnapshot, SubjectVersion,
    TaxTreatment,
};

const TYPE_URL: &str = "type.googleapis.com/ficant.market.v1.FundingRulePack";

#[test]
fn exact_funding_binding_selects_subject_tier_and_rejects_missing_item() {
    let definitions = Definitions::new([
        DefinitionValue::MarketRulePack(pack(1, "dr=18;r=25")),
        DefinitionValue::MarketRulePack(pack(2, "dr=18")),
    ]);
    let parser = FixtureFundingParser;
    let resolver = ResolveFundingRule::new(&definitions, &parser);

    let r_only = block_on(resolver.execute(
        &scope(),
        &binding(1, "dr=18;r=25"),
        time(12),
        FundingTier::ROnly,
    ))
    .expect("exact R-only funding rule resolves");
    assert_eq!(
        r_only.annual_financing_rate(),
        FixedDecimal::from_scaled(25_000_000_000)
    );
    assert_eq!(r_only.unit(), &unit());

    let missing =
        block_on(resolver.execute(&scope(), &binding(2, "dr=18"), time(12), FundingTier::ROnly))
            .expect_err("missing selected tier fails closed");
    assert_eq!(
        missing.category(),
        ApplicationErrorCategory::ValidationFailed
    );
    assert!(!missing.retryable());
    assert_eq!(
        missing.detail(),
        Some(&ApplicationErrorDetail::RulePackItemMissing {
            path: "context.funding_rule_pack.content.rates[funding_tier=R_ONLY]".to_owned(),
        })
    );
}

#[test]
fn exact_subject_resolution_rejects_missing_and_reference_drift_before_engine() {
    let requested = VersionRef::new(id('S'), version(1));
    let missing_subjects = Subjects::new(None);
    let missing = ResolveSubject::new(&missing_subjects);
    assert_subject_error(block_on(missing.execute(
        &requested,
        "CFFEX",
        "futures-hedge",
    )));

    let drifted_subjects = Subjects::new(Some(subject('T', FundingTier::DrAvailable)));
    let drifted = ResolveSubject::new(&drifted_subjects);
    assert_subject_error(block_on(drifted.execute(
        &requested,
        "CFFEX",
        "futures-hedge",
    )));

    let version_drifted_subjects =
        Subjects::new(Some(subject_at_version('S', 2, FundingTier::DrAvailable)));
    let version_drifted = ResolveSubject::new(&version_drifted_subjects);
    assert_subject_error(block_on(version_drifted.execute(
        &requested,
        "CFFEX",
        "futures-hedge",
    )));

    let allowed_subjects = Subjects::new(Some(subject('S', FundingTier::ROnly)));
    let allowed = ResolveSubject::new(&allowed_subjects);
    let resolved = block_on(allowed.execute(&requested, "CFFEX", "futures-hedge"))
        .expect("matching exact Subject with access resolves");
    assert_eq!(resolved.reference(), &requested);
    assert_eq!(resolved.funding_tier(), FundingTier::ROnly);
}

#[test]
fn subject_access_is_fail_closed_with_same_safe_binding_detail() {
    let requested = VersionRef::new(id('S'), version(1));
    let record = subject_with_access('S', FundingTier::DrAvailable, ["CN"], ["bond-analytics"]);
    let subjects = Subjects::new(Some(record));
    let resolver = ResolveSubject::new(&subjects);
    assert_subject_error(block_on(resolver.execute(
        &requested,
        "CFFEX",
        "futures-hedge",
    )));
}

fn assert_subject_error(result: Result<SubjectVersion, ApplicationError>) {
    let error = result.expect_err("invalid Subject must fail closed");
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert!(!error.retryable());
    assert_eq!(
        error.detail(),
        Some(&ApplicationErrorDetail::SubjectBindingInvalid)
    );
}

struct FixtureFundingParser;

impl FundingRulePackParser for FixtureFundingParser {
    fn market(&self) -> &'static str {
        "CN"
    }

    fn rule_type(&self) -> &'static str {
        "funding"
    }

    fn type_url(&self) -> &'static str {
        TYPE_URL
    }

    fn parse(
        &self,
        content: &RulePackContent,
        funding_tier: FundingTier,
    ) -> Result<FundingRate, ApplicationError> {
        match (content.value(), funding_tier) {
            (b"dr=18;r=25" | b"dr=18", FundingTier::DrAvailable) => Ok(FundingRate::new(
                FixedDecimal::from_scaled(18_000_000_000),
                unit(),
            )),
            (b"dr=18;r=25", FundingTier::ROnly) => Ok(FundingRate::new(
                FixedDecimal::from_scaled(25_000_000_000),
                unit(),
            )),
            (b"dr=18", FundingTier::ROnly) => Err(ApplicationError::rule_pack_item_missing(
                "context.funding_rule_pack.content.rates[funding_tier=R_ONLY]",
            )),
            _ => Err(ApplicationError::new(
                ApplicationErrorCategory::ValidationFailed,
                false,
            )),
        }
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
        unreachable!("R3a requires exact RulePack binding")
    }
}

struct Subjects {
    value: Option<SubjectRecord>,
}

impl Subjects {
    const fn new(value: Option<SubjectRecord>) -> Self {
        Self { value }
    }
}

#[async_trait]
impl SubjectRepository for Subjects {
    async fn register_subject(&self, _: SubjectRecord) -> Result<SubjectRecord, ApplicationError> {
        unreachable!("resolver performs only exact reads")
    }

    async fn get_subject(&self, _: VersionRef) -> Result<Option<SubjectRecord>, ApplicationError> {
        Ok(self.value.clone())
    }

    async fn register_subject_state(
        &self,
        _: SubjectStateSnapshot,
    ) -> Result<SubjectStateSnapshot, ApplicationError> {
        unreachable!("resolver performs only exact reads")
    }

    async fn get_subject_state(
        &self,
        _: Ulid,
        _: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<SubjectStateSnapshot>, ApplicationError> {
        unreachable!("resolver performs only exact reads")
    }
}

fn pack(version_value: u64, payload: &str) -> MarketRulePack {
    let content = RulePackContent::new(TYPE_URL, payload.as_bytes().to_vec()).unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('R'),
            version: version(version_value),
            owner: owner(),
            market: "CN".to_owned(),
            rule_type: "funding".to_owned(),
            source: "synthetic-r3a-fixture".to_owned(),
            effective: EffectivePeriod::new(time(1), time(15)).unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(payload.as_bytes()),
        },
        content,
    )
    .unwrap()
}

fn binding(version_value: u64, payload: &str) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(id('R'), version(version_value)),
        ContentHash::digest(payload.as_bytes()),
    )
}

fn subject(suffix: char, funding_tier: FundingTier) -> SubjectRecord {
    subject_at_version_with_access(
        suffix,
        1,
        funding_tier,
        ["CFFEX"],
        ["futures-delivery", "futures-hedge"],
    )
}

fn subject_at_version(
    suffix: char,
    version_value: u64,
    funding_tier: FundingTier,
) -> SubjectRecord {
    subject_at_version_with_access(
        suffix,
        version_value,
        funding_tier,
        ["CFFEX"],
        ["futures-delivery", "futures-hedge"],
    )
}

fn subject_with_access<M, T>(
    suffix: char,
    funding_tier: FundingTier,
    markets: M,
    tools: T,
) -> SubjectRecord
where
    M: IntoIterator<Item = &'static str>,
    T: IntoIterator<Item = &'static str>,
{
    subject_at_version_with_access(suffix, 1, funding_tier, markets, tools)
}

fn subject_at_version_with_access<M, T>(
    suffix: char,
    version_value: u64,
    funding_tier: FundingTier,
    markets: M,
    tools: T,
) -> SubjectRecord
where
    M: IntoIterator<Item = &'static str>,
    T: IntoIterator<Item = &'static str>,
{
    let subject = Subject::new(id(suffix), "R3a fixture Subject").unwrap();
    let version = SubjectVersion::new(
        VersionRef::new(subject.id().clone(), version(version_value)),
        AccessSet::new(markets, tools).unwrap(),
        funding_tier,
        TaxTreatment::new("synthetic-vat", "synthetic-income").unwrap(),
        "synthetic-assessment",
        "synthetic-liability",
        None,
    )
    .unwrap();
    SubjectRecord::new(subject, version).unwrap()
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
