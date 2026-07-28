use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use ficant_application::ports::{
    AppendDefinitionVersion, DefinitionIdentity, DefinitionRepository, DefinitionValue,
    FuturesDeliveryEngine, FuturesDeliveryRuleParser,
};
use ficant_application::{
    AccessScope, ApplicationError, ApplicationErrorCategory, ApplicationErrorDetail,
    CalculateFuturesDeliveryBasket, ResolveFuturesDeliveryRule,
};
use ficant_domain::analytics::{
    AnalyticsError, AnalyticsObjectRef, BondTerms, BusinessDayConvention, CouponFrequency,
    DayCountConvention, FixedDecimal,
};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliverableInput, FuturesDeliveryMeasures, FuturesDeliveryResult,
    FuturesDeliveryRule, FuturesDeliveryRuleInput,
};
use ficant_domain::market::{
    MarketRulePack, MarketRulePackInput, RulePackContent, VerificationStatus,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};

const CGB_FUTURES_TYPE_URL: &str =
    "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack";
const MISSING_PATH: &str = "context.rule_pack.content.products[product_code=T].residual_min_months";
const FIXTURE: &str = include_str!("fixtures/cgb-futures-rule-packs.fixture");

#[test]
fn ac02_rule_pack_content_changes_result_and_missing_item_fails_closed() {
    let definitions = Definitions::new([
        DefinitionValue::MarketRulePack(pack(1, fixture_value("v1"))),
        DefinitionValue::MarketRulePack(pack(2, fixture_value("v2"))),
        DefinitionValue::MarketRulePack(pack(3, fixture_value("missing"))),
    ]);
    let parser = FixtureRuleParser;
    let resolver = ResolveFuturesDeliveryRule::new(&definitions, &parser);

    let first_binding = binding(1, fixture_value("v1"));
    let second_binding = binding(2, fixture_value("v2"));
    let first_rule = block_on(resolver.execute(
        &scope(),
        &first_binding,
        time(12),
        CgbFuturesProduct::TenYear,
    ))
    .expect("first complete pack resolves");
    let second_rule = block_on(resolver.execute(
        &scope(),
        &second_binding,
        time(12),
        CgbFuturesProduct::TenYear,
    ))
    .expect("second complete pack resolves");

    let engine = RuleObservingEngine::default();
    let first = CalculateFuturesDeliveryBasket::new(&engine)
        .execute(&[input(first_binding, first_rule)])
        .expect("first pack reaches calculation");
    let second = CalculateFuturesDeliveryBasket::new(&engine)
        .execute(&[input(second_binding, second_rule)])
        .expect("second pack reaches calculation");
    assert_ne!(
        first.ctd().measures().conversion_factor(),
        second.ctd().measures().conversion_factor(),
        "only the exact RulePack version/content/hash differs"
    );

    engine.calls.store(0, Ordering::SeqCst);
    let missing = block_on(resolver.execute(
        &scope(),
        &binding(3, fixture_value("missing")),
        time(12),
        CgbFuturesProduct::TenYear,
    ))
    .expect_err("missing required rule item must fail closed before the engine");
    assert_eq!(
        missing.category(),
        ApplicationErrorCategory::ValidationFailed
    );
    assert!(!missing.retryable());
    assert_eq!(
        missing.detail(),
        Some(&ApplicationErrorDetail::RulePackItemMissing {
            path: MISSING_PATH.to_owned(),
        })
    );
    assert_eq!(engine.calls(), 0);
}

struct FixtureRuleParser;

impl FuturesDeliveryRuleParser for FixtureRuleParser {
    fn market(&self) -> &'static str {
        "CFFEX"
    }

    fn rule_type(&self) -> &'static str {
        "cgb-futures"
    }

    fn type_url(&self) -> &'static str {
        CGB_FUTURES_TYPE_URL
    }

    fn parse(
        &self,
        content: &RulePackContent,
        product: CgbFuturesProduct,
    ) -> Result<FuturesDeliveryRule, ApplicationError> {
        assert_eq!(product, CgbFuturesProduct::TenYear);
        assert_eq!(content.type_url(), CGB_FUTURES_TYPE_URL);
        match content.value() {
            value if value == fixture_value("v1").as_bytes() => Ok(rule(30_000_000_000)),
            value if value == fixture_value("v2").as_bytes() => Ok(rule(40_000_000_000)),
            value if value == fixture_value("missing").as_bytes() => {
                Err(ApplicationError::rule_pack_item_missing(MISSING_PATH))
            }
            _ => Err(ApplicationError::new(
                ApplicationErrorCategory::ValidationFailed,
                false,
            )),
        }
    }
}

#[derive(Default)]
struct RuleObservingEngine {
    calls: AtomicUsize,
}

impl RuleObservingEngine {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl FuturesDeliveryEngine for RuleObservingEngine {
    fn calculate(
        &self,
        input: &FuturesDeliverableInput,
    ) -> Result<FuturesDeliveryResult, AnalyticsError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let conversion_factor = input.rule().nominal_coupon();
        let measures = FuturesDeliveryMeasures::new(
            1,
            1,
            conversion_factor,
            FixedDecimal::ZERO,
            FixedDecimal::ZERO,
            FixedDecimal::ZERO,
            FixedDecimal::from_scaled(100_000_000_000_000),
            FixedDecimal::from_scaled(100_000_000_000_000),
            FixedDecimal::from_scaled(1_000_000_000_000),
            FixedDecimal::ZERO,
            FixedDecimal::ZERO,
            FixedDecimal::from_scaled(1_000_000_000_000),
            FixedDecimal::from_scaled(20_000_000_000),
            FixedDecimal::from_scaled(-1_000_000_000_000),
        )
        .map_err(|_| AnalyticsError::Internal)?;
        Ok(FuturesDeliveryResult::new(input.clone(), measures))
    }
}

fn input(rule_pack: AnalyticsObjectRef, rule: FuturesDeliveryRule) -> FuturesDeliverableInput {
    FuturesDeliverableInput::new(
        owner(),
        object('F'),
        object('B'),
        rule_pack,
        object('S'),
        time(12),
        "2026-08-04".parse().unwrap(),
        "2026-09-01".parse().unwrap(),
        "2026-09-15".parse().unwrap(),
        CgbFuturesProduct::TenYear,
        rule,
        BondTerms::new(
            "2025-01-01".parse().unwrap(),
            "2034-06-15".parse().unwrap(),
            CouponFrequency::Semiannual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            FixedDecimal::from_scaled(30_000_000_000),
            FixedDecimal::from_scaled(100_000_000_000_000),
        )
        .unwrap(),
        FixedDecimal::from_scaled(100_000_000_000_000),
        FixedDecimal::from_scaled(100_000_000_000_000),
        FixedDecimal::from_scaled(20_000_000_000),
    )
    .unwrap()
}

fn rule(nominal_coupon: i128) -> FuturesDeliveryRule {
    FuturesDeliveryRule::new(FuturesDeliveryRuleInput {
        original_term_max_months: 120,
        residual_min_months: 78,
        residual_max_months: None,
        delivery_months: vec![3, 6, 9, 12],
        nominal_coupon: FixedDecimal::from_scaled(nominal_coupon),
        face_quote_basis: FixedDecimal::from_scaled(100_000_000_000_000),
        accrued_interest_day_count: 365,
        conversion_factor_rounding_places: 4,
        accrued_interest_rounding_places: 7,
        annual_day_basis: 365,
    })
    .unwrap()
}

fn pack(version_value: u64, payload: &str) -> MarketRulePack {
    let content = RulePackContent::new(CGB_FUTURES_TYPE_URL, payload.as_bytes().to_vec()).unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('R'),
            version: version(version_value),
            owner: owner(),
            market: "CFFEX".to_owned(),
            rule_type: "cgb-futures".to_owned(),
            source: "fixture".to_owned(),
            effective: ficant_domain::primitives::EffectivePeriod::new(time(1), time(15)).unwrap(),
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

fn fixture_value(key: &str) -> &str {
    FIXTURE
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name == key).then_some(value)
        })
        .unwrap_or_else(|| panic!("missing fixture key {key}"))
}

struct Definitions {
    values: BTreeMap<(String, u64), DefinitionValue>,
}

impl Definitions {
    fn new(values: impl IntoIterator<Item = DefinitionValue>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|value| {
                    let key = (value.identity().to_owned(), value.version());
                    (key, value)
                })
                .collect(),
        }
    }
}

#[async_trait]
impl DefinitionRepository for Definitions {
    async fn create_identity(&self, _identity: DefinitionIdentity) -> Result<(), ApplicationError> {
        unreachable!("the resolver only performs exact reads")
    }

    async fn append_version(
        &self,
        _command: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        unreachable!("the resolver only performs exact reads")
    }

    async fn get_version(
        &self,
        _scope: &AccessScope,
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
        _scope: &AccessScope,
        _definition_id: Ulid,
        _instant: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        unreachable!("R2 requires an exact RulePack binding")
    }
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('A'), vec![id('O')]).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('O'))
}

fn object(suffix: char) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(id(suffix), version(1)),
        ContentHash::digest(format!("object-{suffix}").as_bytes()),
    )
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => 'J',
        'O' => 'Q',
        'U' => 'W',
        value => value,
    };
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
