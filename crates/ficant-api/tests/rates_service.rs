use chrono::{NaiveDate, TimeZone, Utc};
use ficant_api::{
    PlatformApplication, PlatformPort, RatesGrpcService, SessionPolicy, SystemClock,
    TrustedIdentity,
};
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, BondAnalyticsEngine, CarryRollEngine, DefinitionIdentity,
    DefinitionRepository, DefinitionValue, FuturesDeliveryEngine, FuturesDeliveryRuleParser,
    FuturesHedgeEngine, YieldCurveEngine,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_cgb_futures_pack::CgbFuturesDeliveryRulePackParser;
use ficant_contracts::ficant::core::v1::{
    DecimalValue, ErrorCode, MarketTime as ProtoMarketTime, OwnerRef as ProtoOwnerRef, Sha256,
    Ulid as ProtoUlid, UnitRef as ProtoUnitRef, VersionRef as ProtoVersionRef,
};
use ficant_contracts::ficant::market::v1::{
    CgbFuturesDeliveryRulePack, CgbFuturesProductRule, cgb_futures_product_rule::ResidualUpperBound,
};
use ficant_contracts::ficant::rates::v1::{
    AlgorithmBinding, AnalysisContext, AnalysisUnits, AnalyzeBondRequest,
    AnalyzeFuturesDeliveryRequest, BondTerms, CgbFuturesProduct as ProtoCgbFuturesProduct,
    CouponFrequency, FuturesDeliverableCandidate, ObjectBinding, analyze_bond_response,
    analyze_futures_delivery_response, rates_analytics_service_server::RatesAnalyticsService,
};
use ficant_domain::analytics::{AnalyticsError, BondAnalyticsInput, BondAnalyticsResult};
use ficant_domain::curves::{CarryRollInput, CarryRollResult, YieldCurvePoint, YieldCurveQuery};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliverableInput, FuturesDeliveryResult, FuturesDeliveryRule,
};
use ficant_domain::futures_hedge::{FuturesHedgeInput, FuturesHedgeResult};
use ficant_domain::market::{
    MarketRulePack, MarketRulePackInput, RulePackContent, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};
use ficant_fixed_income_native::NativeFuturesDeliveryEngine;
use prost::Message;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

#[derive(Clone)]
struct CountingFailureEngine(Arc<AtomicUsize>);

impl CountingFailureEngine {
    fn fail(&self) -> AnalyticsError {
        self.0.fetch_add(1, Ordering::SeqCst);
        AnalyticsError::Internal
    }
}

impl BondAnalyticsEngine for CountingFailureEngine {
    fn calculate(&self, _: &BondAnalyticsInput) -> Result<BondAnalyticsResult, AnalyticsError> {
        Err(self.fail())
    }
}

impl YieldCurveEngine for CountingFailureEngine {
    fn interpolate(&self, _: &YieldCurveQuery) -> Result<YieldCurvePoint, AnalyticsError> {
        Err(self.fail())
    }
}

impl CarryRollEngine for CountingFailureEngine {
    fn calculate(&self, _: &CarryRollInput) -> Result<CarryRollResult, AnalyticsError> {
        Err(self.fail())
    }
}

impl FuturesDeliveryEngine for CountingFailureEngine {
    fn calculate(
        &self,
        _: &FuturesDeliverableInput,
    ) -> Result<FuturesDeliveryResult, AnalyticsError> {
        Err(self.fail())
    }
}

impl FuturesHedgeEngine for CountingFailureEngine {
    fn calculate(&self, _: &FuturesHedgeInput) -> Result<FuturesHedgeResult, AnalyticsError> {
        Err(self.fail())
    }
}

struct NoDefinitionRepository;

#[tonic::async_trait]
impl DefinitionRepository for NoDefinitionRepository {
    async fn create_identity(&self, _: DefinitionIdentity) -> Result<(), ApplicationError> {
        Err(ApplicationError::new(
            ApplicationErrorCategory::StorageUnavailable,
            false,
        ))
    }

    async fn append_version(
        &self,
        _: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        Err(ApplicationError::new(
            ApplicationErrorCategory::StorageUnavailable,
            false,
        ))
    }

    async fn get_version(
        &self,
        _: &AccessScope,
        _: Ulid,
        _: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Err(ApplicationError::new(
            ApplicationErrorCategory::StorageUnavailable,
            false,
        ))
    }

    async fn resolve_as_of(
        &self,
        _: &AccessScope,
        _: Ulid,
        _: ficant_domain::primitives::MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Err(ApplicationError::new(
            ApplicationErrorCategory::StorageUnavailable,
            false,
        ))
    }
}

struct NoFuturesDeliveryRuleParser;

impl FuturesDeliveryRuleParser for NoFuturesDeliveryRuleParser {
    fn market(&self) -> &'static str {
        "CFFEX"
    }

    fn rule_type(&self) -> &'static str {
        "cgb-futures"
    }

    fn type_url(&self) -> &'static str {
        "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack"
    }

    fn parse(
        &self,
        _: &RulePackContent,
        _: CgbFuturesProduct,
    ) -> Result<FuturesDeliveryRule, ApplicationError> {
        Err(ApplicationError::new(
            ApplicationErrorCategory::StorageUnavailable,
            false,
        ))
    }
}

#[derive(Clone)]
struct FixtureDefinitions {
    values: Vec<DefinitionValue>,
}

#[tonic::async_trait]
impl DefinitionRepository for FixtureDefinitions {
    async fn create_identity(&self, _: DefinitionIdentity) -> Result<(), ApplicationError> {
        Err(storage_unavailable())
    }

    async fn append_version(
        &self,
        _: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        Err(storage_unavailable())
    }

    async fn get_version(
        &self,
        _: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Ok(self
            .values
            .iter()
            .find(|value| {
                value.identity() == definition_id.as_str() && value.version() == version.get()
            })
            .cloned())
    }

    async fn resolve_as_of(
        &self,
        _: &AccessScope,
        _: Ulid,
        _: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Err(storage_unavailable())
    }
}

#[derive(Clone)]
struct RecordingNativeDeliveryEngine(Arc<AtomicUsize>);

impl FuturesDeliveryEngine for RecordingNativeDeliveryEngine {
    fn calculate(
        &self,
        input: &FuturesDeliverableInput,
    ) -> Result<FuturesDeliveryResult, AnalyticsError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        NativeFuturesDeliveryEngine.calculate(input)
    }
}

fn storage_unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, false)
}

fn service(scopes: &[&str], calls: Arc<AtomicUsize>) -> RatesGrpcService {
    let identity = TrustedIdentity::implicit("rates-test", scopes.iter().copied())
        .expect("test identity is valid");
    let application: Arc<dyn PlatformPort> = Arc::new(
        PlatformApplication::try_new(
            Arc::new(SystemClock),
            SessionPolicy::new(900, 60).expect("test session policy is valid"),
            KEY,
            Vec::new(),
            Some(identity),
            Vec::new(),
        )
        .expect("test platform application is valid"),
    );
    let engine = CountingFailureEngine(calls);
    RatesGrpcService::new(
        application,
        Arc::new(engine.clone()),
        Arc::new(engine.clone()),
        Arc::new(engine.clone()),
        Arc::new(engine.clone()),
        Arc::new(NoDefinitionRepository),
        Arc::new(NoFuturesDeliveryRuleParser),
        Arc::new(engine),
        KEY,
    )
    .expect("rates service is valid")
}

fn delivery_service(
    values: Vec<DefinitionValue>,
    delivery_calls: Arc<AtomicUsize>,
) -> RatesGrpcService {
    let identity = TrustedIdentity::implicit("rates-delivery-test", ["rates:analyze"])
        .expect("test identity is valid");
    let application: Arc<dyn PlatformPort> = Arc::new(
        PlatformApplication::try_new(
            Arc::new(SystemClock),
            SessionPolicy::new(900, 60).expect("test session policy is valid"),
            KEY,
            Vec::new(),
            Some(identity),
            Vec::new(),
        )
        .expect("test platform application is valid"),
    );
    let unused_calls = Arc::new(AtomicUsize::new(0));
    let fallback = CountingFailureEngine(unused_calls);
    RatesGrpcService::new(
        application,
        Arc::new(fallback.clone()),
        Arc::new(fallback.clone()),
        Arc::new(fallback.clone()),
        Arc::new(RecordingNativeDeliveryEngine(delivery_calls)),
        Arc::new(FixtureDefinitions { values }),
        Arc::new(CgbFuturesDeliveryRulePackParser),
        Arc::new(fallback),
        KEY,
    )
    .expect("rates delivery service is valid")
}

#[tokio::test]
async fn missing_scope_returns_structured_forbidden_before_validation_or_provider() {
    let calls = Arc::new(AtomicUsize::new(0));
    let response = service(&["rates:read"], Arc::clone(&calls))
        .analyze_bond(Request::new(AnalyzeBondRequest::default()))
        .await
        .expect("business failure is transported in the response")
        .into_inner();

    let Some(analyze_bond_response::Result::Error(error)) = response.result else {
        panic!("missing scope must return a structured business error");
    };
    assert_eq!(error.code, ErrorCode::Forbidden as i32);
    assert!(!error.retryable);
    assert!(!error.trace_id.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_request_with_scope_fails_closed_before_provider() {
    let calls = Arc::new(AtomicUsize::new(0));
    let response = service(&["rates:analyze"], Arc::clone(&calls))
        .analyze_bond(Request::new(AnalyzeBondRequest::default()))
        .await
        .expect("validation failure is transported in the response")
        .into_inner();

    let Some(analyze_bond_response::Result::Error(error)) = response.result else {
        panic!("invalid request must return a structured business error");
    };
    assert_eq!(error.code, ErrorCode::ValidationFailed as i32);
    assert!(!error.retryable);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ac02_grpc_parses_exact_rule_pack_versions_and_reports_missing_item_before_engine() {
    let first = delivery_pack(1, "3", false);
    let second = delivery_pack(2, "4", false);
    let missing = delivery_pack(3, "3", true);
    let calls = Arc::new(AtomicUsize::new(0));
    let service = delivery_service(
        vec![
            DefinitionValue::MarketRulePack(first.clone()),
            DefinitionValue::MarketRulePack(second.clone()),
            DefinitionValue::MarketRulePack(missing.clone()),
        ],
        Arc::clone(&calls),
    );

    let first_response = service
        .analyze_futures_delivery(Request::new(delivery_request(&first)))
        .await
        .expect("business result is transported")
        .into_inner();
    let second_response = service
        .analyze_futures_delivery(Request::new(delivery_request(&second)))
        .await
        .expect("business result is transported")
        .into_inner();
    assert_ne!(
        conversion_factor_coefficient(&first_response),
        conversion_factor_coefficient(&second_response),
        "changing only the exact RulePack version, content value, and content hash must change the calculation"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    calls.store(0, Ordering::SeqCst);

    let missing_response = service
        .analyze_futures_delivery(Request::new(delivery_request(&missing)))
        .await
        .expect("business error is transported")
        .into_inner();
    let Some(analyze_futures_delivery_response::Result::Error(error)) = missing_response.result
    else {
        panic!("missing rule item must fail closed through ErrorDetail");
    };
    assert_eq!(error.code, ErrorCode::ValidationFailed as i32);
    assert!(!error.retryable);
    assert_eq!(error.field_violations.len(), 1);
    assert_eq!(
        error.field_violations[0].field,
        "context.rule_pack.content.products[product_code=T].residual_min_months"
    );
    assert_eq!(
        error.field_violations[0].description,
        "规则包缺少计算所需项"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

fn delivery_pack(version: u64, nominal_coupon: &str, missing_residual_min: bool) -> MarketRulePack {
    let payload = CgbFuturesDeliveryRulePack {
        products: vec![CgbFuturesProductRule {
            product_code: Some("T".to_owned()),
            original_term_max_months: Some(120),
            residual_min_months: (!missing_residual_min).then_some(78),
            residual_upper_bound: Some(ResidualUpperBound::ResidualMaxMonthsUnbounded(true)),
        }],
        delivery_months: vec![3, 6, 9, 12],
        nominal_coupon: Some(decimal(nominal_coupon, 2, &unit('P'))),
        face_quote_basis: Some(decimal("100", 0, &unit('P'))),
        accrued_interest_day_count: Some(1),
        conversion_factor_rounding_places: Some(4),
        accrued_interest_rounding_places: Some(7),
        annual_day_basis: Some(365),
    };
    let content = RulePackContent::new(
        "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack",
        payload.encode_to_vec(),
    )
    .unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('R'),
            version: Version::new(version).unwrap(),
            owner: owner(),
            market: "CFFEX".to_owned(),
            rule_type: "cgb-futures".to_owned(),
            source: "grpc-fixture".to_owned(),
            effective: EffectivePeriod::new(domain_time(1), domain_time(31)).unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(content.value()),
        },
        content,
    )
    .unwrap()
}

fn delivery_request(pack: &MarketRulePack) -> AnalyzeFuturesDeliveryRequest {
    let rate = unit('P');
    let price = unit('P');
    AnalyzeFuturesDeliveryRequest {
        context: Some(AnalysisContext {
            owner: Some(proto_owner()),
            rule_pack: Some(object_binding('R', pack.content_hash(), pack.version())),
            data_snapshot: Some(object_binding('E', &ContentHash::digest(b"snapshot"), 1)),
            algorithm: Some(AlgorithmBinding {
                algorithm_id: "ficant.cffex.cgb-futures-delivery".to_owned(),
                algorithm_version: 1,
                convention_profile: "cffex-cgb-futures-delivery-v1".to_owned(),
                abi_version: 1,
            }),
            units: Some(AnalysisUnits {
                currency_amount: Some(rate.clone()),
                price_per_100: Some(price.clone()),
                rate: Some(rate.clone()),
                years: Some(rate.clone()),
                years_squared: Some(rate.clone()),
                dv01_per_100: Some(rate.clone()),
                dv01: Some(rate.clone()),
                dimensionless: Some(rate.clone()),
                contract_count: Some(rate.clone()),
            }),
        }),
        futures_contract: Some(object_binding('C', &ContentHash::digest(b"contract"), 1)),
        valuation_at: Some(proto_time(20)),
        purchase_date: "2026-07-21".to_owned(),
        delivery_month_first: "2026-09-01".to_owned(),
        delivery_date: "2026-09-18".to_owned(),
        product: ProtoCgbFuturesProduct::T as i32,
        futures_clean_price: Some(decimal("995", 1, &price)),
        financing_rate: Some(decimal("18", 3, &rate)),
        candidates: vec![FuturesDeliverableCandidate {
            bond: Some(object_binding('D', &ContentHash::digest(b"bond"), 1)),
            terms: Some(BondTerms {
                issue_date: "2024-08-15".to_owned(),
                maturity_date: "2034-08-15".to_owned(),
                frequency: CouponFrequency::Semiannual as i32,
                coupon_rate: Some(decimal("25", 3, &rate)),
                face_amount: Some(decimal("100", 0, &rate)),
            }),
            spot_clean_price: Some(decimal("10125", 2, &price)),
        }],
    }
}

fn conversion_factor_coefficient(
    response: &ficant_contracts::ficant::rates::v1::AnalyzeFuturesDeliveryResponse,
) -> String {
    let Some(analyze_futures_delivery_response::Result::Analysis(result)) = &response.result else {
        panic!("complete RulePack must produce an analysis");
    };
    result.candidates[0]
        .measures
        .as_ref()
        .and_then(|measures| measures.conversion_factor.as_ref())
        .expect("conversion factor must be present")
        .coefficient
        .clone()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('A'), id('B'))
}

fn proto_owner() -> ProtoOwnerRef {
    ProtoOwnerRef {
        tenant_id: Some(proto_ulid('A')),
        owner_id: Some(proto_ulid('B')),
    }
}

fn object_binding(suffix: char, content_hash: &ContentHash, version: u64) -> ObjectBinding {
    ObjectBinding {
        object: Some(ProtoVersionRef {
            id: Some(proto_ulid(suffix)),
            version,
        }),
        content_hash: Some(Sha256 {
            value: content_hash.as_bytes().to_vec(),
        }),
    }
}

fn decimal(coefficient: &str, scale: u32, unit: &ProtoUnitRef) -> DecimalValue {
    DecimalValue {
        coefficient: coefficient.to_owned(),
        scale,
        unit: Some(unit.clone()),
    }
}

fn unit(suffix: char) -> ProtoUnitRef {
    ProtoUnitRef {
        unit_id: Some(proto_ulid(suffix)),
        version: 1,
    }
}

fn domain_time(day: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 7, day, 4, 0, 0).unwrap(),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 7, day).unwrap(),
    )
    .unwrap()
}

fn proto_time(day: u32) -> ProtoMarketTime {
    let time = Utc.with_ymd_and_hms(2026, 7, day, 4, 0, 0).unwrap();
    ProtoMarketTime {
        instant: Some(prost_types::Timestamp {
            seconds: time.timestamp(),
            nanos: 0,
        }),
        market_timezone: "Asia/Shanghai".to_owned(),
        local_trading_date: format!("2026-07-{day:02}"),
    }
}

fn proto_ulid(suffix: char) -> ProtoUlid {
    ProtoUlid {
        value: id(suffix).as_str().to_owned(),
    }
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
