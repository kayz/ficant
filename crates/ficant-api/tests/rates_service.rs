use chrono::{NaiveDate, TimeZone, Utc};
use ficant_api::{
    PlatformApplication, PlatformPort, RatesGrpcService, SessionPolicy, SystemClock,
    TrustedIdentity,
};
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, BondAnalyticsEngine, CarryRollEngine, DefinitionIdentity,
    DefinitionRepository, DefinitionValue, FuturesDeliveryEngine, FuturesDeliveryRuleParser,
    FuturesHedgeEngine, SubjectRepository, YieldCurveEngine,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_cgb_futures_pack::CgbFuturesDeliveryRulePackParser;
use ficant_contracts::ficant::core::v1::{
    DecimalValue, ErrorCode, FundingTier as ProtoFundingTier, MarketTime as ProtoMarketTime,
    OwnerRef as ProtoOwnerRef, Sha256, Ulid as ProtoUlid, UnitRef as ProtoUnitRef,
    VersionRef as ProtoVersionRef,
};
use ficant_contracts::ficant::market::v1::{
    BondCouponTaxRule, BondTaxAttributes, CgbFuturesDeliveryRulePack, CgbFuturesProductRule,
    FundingRulePack, FundingTierRate, IncomeTaxStatus as ProtoIncomeTaxStatus,
    SubjectCouponTaxRate, TaxRulePack, ValueAddedTaxStatus as ProtoValueAddedTaxStatus,
    cgb_futures_product_rule::ResidualUpperBound,
};
use ficant_contracts::ficant::rates::v1::{
    AlgorithmBinding, AnalysisContext, AnalysisUnits, AnalyzeBondRequest, AnalyzeCarryRollRequest,
    AnalyzeFuturesDeliveryRequest, AnalyzeFuturesHedgeRequest, BondTerms, CalendarBinding,
    CalendarRequirement, CgbFuturesProduct as ProtoCgbFuturesProduct, CouponFrequency,
    FuturesDeliverableCandidate, InterpolateYieldCurveRequest, ObjectBinding,
    analyze_bond_response, analyze_carry_roll_response, analyze_futures_delivery_response,
    analyze_futures_hedge_response, interpolate_yield_curve_response,
    rates_analytics_service_server::RatesAnalyticsService,
};
use ficant_domain::analytics::{
    ALGORITHM_ID, AnalyticsError, BondAnalyticsInput, BondAnalyticsResult, CONVENTION_PROFILE,
};
use ficant_domain::curves::{
    CARRY_ROLL_ALGORITHM_ID, CARRY_ROLL_CONVENTION_PROFILE, CURVE_ALGORITHM_ID,
    CURVE_CONVENTION_PROFILE, CarryRollInput, CarryRollResult, YieldCurvePoint, YieldCurveQuery,
};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FUTURES_DELIVERY_ALGORITHM_ID, FUTURES_DELIVERY_CONVENTION_PROFILE,
    FuturesDeliverableInput, FuturesDeliveryResult, FuturesDeliveryRule,
};
use ficant_domain::futures_hedge::{
    FUTURES_HEDGE_ALGORITHM_ID, FUTURES_HEDGE_CONVENTION_PROFILE, FuturesHedgeInput,
    FuturesHedgeResult,
};
use ficant_domain::market::{
    MarketRulePack, MarketRulePackInput, RulePackContent, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version,
};
use ficant_domain::subject::{
    AccessSet, FundingTier, Subject, SubjectRecord, SubjectStateSnapshot, SubjectVersion,
    TaxTreatment,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};
use ficant_fixed_income_native::{
    NativeBondAnalyticsEngine, NativeFuturesDeliveryEngine, NativeFuturesHedgeEngine,
};
use ficant_funding_pack::{
    FundingRulePackV1Parser, MARKET as FUNDING_MARKET, RULE_TYPE as FUNDING_RULE_TYPE,
    TYPE_URL as FUNDING_TYPE_URL,
};
use ficant_tax_pack::{
    MARKET as TAX_MARKET, RULE_TYPE as TAX_RULE_TYPE, TYPE_URL as TAX_TYPE_URL, TaxRulePackV1Parser,
};
use prost::Message;
use rust_decimal::{Decimal as ExactDecimal, RoundingStrategy};
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

struct NoSubjects;

#[tonic::async_trait]
impl SubjectRepository for NoSubjects {
    async fn register_subject(&self, _: SubjectRecord) -> Result<SubjectRecord, ApplicationError> {
        Err(storage_unavailable())
    }

    async fn get_subject(
        &self,
        _: ficant_domain::primitives::VersionRef,
    ) -> Result<Option<SubjectRecord>, ApplicationError> {
        Ok(None)
    }

    async fn register_subject_state(
        &self,
        _: SubjectStateSnapshot,
    ) -> Result<SubjectStateSnapshot, ApplicationError> {
        Err(storage_unavailable())
    }

    async fn get_subject_state(
        &self,
        _: Ulid,
        _: chrono::DateTime<Utc>,
    ) -> Result<Option<SubjectStateSnapshot>, ApplicationError> {
        Err(storage_unavailable())
    }
}

#[derive(Clone)]
struct FixtureSubjects {
    values: Vec<SubjectRecord>,
}

#[tonic::async_trait]
impl SubjectRepository for FixtureSubjects {
    async fn register_subject(&self, _: SubjectRecord) -> Result<SubjectRecord, ApplicationError> {
        Err(storage_unavailable())
    }

    async fn get_subject(
        &self,
        reference: ficant_domain::primitives::VersionRef,
    ) -> Result<Option<SubjectRecord>, ApplicationError> {
        Ok(self
            .values
            .iter()
            .find(|value| value.version().reference() == &reference)
            .cloned())
    }

    async fn register_subject_state(
        &self,
        _: SubjectStateSnapshot,
    ) -> Result<SubjectStateSnapshot, ApplicationError> {
        Err(storage_unavailable())
    }

    async fn get_subject_state(
        &self,
        _: Ulid,
        _: chrono::DateTime<Utc>,
    ) -> Result<Option<SubjectStateSnapshot>, ApplicationError> {
        Err(storage_unavailable())
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

#[derive(Clone)]
struct RecordingNativeBondEngine(Arc<AtomicUsize>);

impl BondAnalyticsEngine for RecordingNativeBondEngine {
    fn calculate(&self, input: &BondAnalyticsInput) -> Result<BondAnalyticsResult, AnalyticsError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        NativeBondAnalyticsEngine.calculate(input)
    }
}

#[derive(Clone)]
struct RecordingNativeHedgeEngine(Arc<AtomicUsize>);

impl FuturesHedgeEngine for RecordingNativeHedgeEngine {
    fn calculate(&self, input: &FuturesHedgeInput) -> Result<FuturesHedgeResult, AnalyticsError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        NativeFuturesHedgeEngine.calculate(input)
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
        Arc::new(NoSubjects),
        Arc::new(NoFuturesDeliveryRuleParser),
        Arc::new(FundingRulePackV1Parser),
        Arc::new(TaxRulePackV1Parser),
        Arc::new(engine),
        KEY,
    )
    .expect("rates service is valid")
}

fn delivery_service(
    values: Vec<DefinitionValue>,
    subjects: Vec<SubjectRecord>,
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
        Arc::new(FixtureDefinitions {
            values: values
                .into_iter()
                .chain(std::iter::once(DefinitionValue::MarketRulePack(
                    funding_pack(),
                )))
                .collect(),
        }),
        Arc::new(FixtureSubjects { values: subjects }),
        Arc::new(CgbFuturesDeliveryRulePackParser),
        Arc::new(FundingRulePackV1Parser),
        Arc::new(TaxRulePackV1Parser),
        Arc::new(fallback),
        KEY,
    )
    .expect("rates delivery service is valid")
}

fn tax_bond_service(
    values: Vec<DefinitionValue>,
    subjects: Vec<SubjectRecord>,
    bond_calls: Arc<AtomicUsize>,
) -> RatesGrpcService {
    let identity = TrustedIdentity::implicit("rates-tax-bond-test", ["rates:analyze"])
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
        Arc::new(RecordingNativeBondEngine(bond_calls)),
        Arc::new(fallback.clone()),
        Arc::new(fallback.clone()),
        Arc::new(fallback.clone()),
        Arc::new(FixtureDefinitions { values }),
        Arc::new(FixtureSubjects { values: subjects }),
        Arc::new(NoFuturesDeliveryRuleParser),
        Arc::new(FundingRulePackV1Parser),
        Arc::new(TaxRulePackV1Parser),
        Arc::new(fallback),
        KEY,
    )
    .expect("rates tax Bond service is valid")
}

fn hedge_service(subjects: Vec<SubjectRecord>, hedge_calls: Arc<AtomicUsize>) -> RatesGrpcService {
    let identity = TrustedIdentity::implicit("rates-hedge-test", ["rates:analyze"])
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
        Arc::new(fallback.clone()),
        Arc::new(NoDefinitionRepository),
        Arc::new(FixtureSubjects { values: subjects }),
        Arc::new(NoFuturesDeliveryRuleParser),
        Arc::new(FundingRulePackV1Parser),
        Arc::new(TaxRulePackV1Parser),
        Arc::new(RecordingNativeHedgeEngine(hedge_calls)),
        KEY,
    )
    .expect("rates hedge service is valid")
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
async fn ac07_all_rates_rpcs_reject_missing_subject_before_engines() {
    let calls = Arc::new(AtomicUsize::new(0));
    let service = service(&["rates:analyze"], Arc::clone(&calls));

    let missing_bond = service
        .analyze_bond(Request::new(AnalyzeBondRequest {
            context: Some(analysis_context(ALGORITHM_ID, CONVENTION_PROFILE, None)),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_subject_error(match missing_bond.result.unwrap() {
        analyze_bond_response::Result::Error(error) => error,
        analyze_bond_response::Result::Analysis(_) => {
            panic!("missing Subject must fail before bond engine")
        }
    });
    let missing_curve = service
        .interpolate_yield_curve(Request::new(InterpolateYieldCurveRequest {
            context: Some(analysis_context(
                CURVE_ALGORITHM_ID,
                CURVE_CONVENTION_PROFILE,
                None,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_subject_error(match missing_curve.result.unwrap() {
        interpolate_yield_curve_response::Result::Error(error) => error,
        interpolate_yield_curve_response::Result::Point(_) => {
            panic!("missing Subject must fail before curve engine")
        }
    });
    let missing_carry = service
        .analyze_carry_roll(Request::new(AnalyzeCarryRollRequest {
            context: Some(analysis_context(
                CARRY_ROLL_ALGORITHM_ID,
                CARRY_ROLL_CONVENTION_PROFILE,
                None,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_subject_error(match missing_carry.result.unwrap() {
        analyze_carry_roll_response::Result::Error(error) => error,
        analyze_carry_roll_response::Result::Analysis(_) => {
            panic!("missing Subject must fail before carry engine")
        }
    });
    let missing_delivery = service
        .analyze_futures_delivery(Request::new(AnalyzeFuturesDeliveryRequest {
            context: Some(analysis_context(
                FUTURES_DELIVERY_ALGORITHM_ID,
                FUTURES_DELIVERY_CONVENTION_PROFILE,
                None,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_subject_error(match missing_delivery.result.unwrap() {
        analyze_futures_delivery_response::Result::Error(error) => error,
        analyze_futures_delivery_response::Result::Analysis(_) => {
            panic!("missing Subject must fail before delivery engine")
        }
    });
    let missing_hedge = service
        .analyze_futures_hedge(Request::new(AnalyzeFuturesHedgeRequest {
            context: Some(analysis_context(
                FUTURES_HEDGE_ALGORITHM_ID,
                FUTURES_HEDGE_CONVENTION_PROFILE,
                None,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_subject_error(match missing_hedge.result.unwrap() {
        analyze_futures_hedge_response::Result::Error(error) => error,
        analyze_futures_hedge_response::Result::Analysis(_) => {
            panic!("missing Subject must fail before hedge engine")
        }
    });
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ac07_all_rates_rpcs_reject_unresolved_subject_before_engines() {
    let calls = Arc::new(AtomicUsize::new(0));
    let service = service(&["rates:analyze"], Arc::clone(&calls));
    let absent = Some(ProtoVersionRef {
        id: Some(proto_ulid('S')),
        version: 1,
    });
    let absent_bond = service
        .analyze_bond(Request::new(AnalyzeBondRequest {
            context: Some(analysis_context(
                ALGORITHM_ID,
                CONVENTION_PROFILE,
                absent.clone(),
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_subject_error(match absent_bond.result.unwrap() {
        analyze_bond_response::Result::Error(error) => error,
        analyze_bond_response::Result::Analysis(_) => {
            panic!("unresolved Subject must fail before bond engine")
        }
    });
    let absent_curve = service
        .interpolate_yield_curve(Request::new(InterpolateYieldCurveRequest {
            context: Some(analysis_context(
                CURVE_ALGORITHM_ID,
                CURVE_CONVENTION_PROFILE,
                absent.clone(),
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_subject_error(match absent_curve.result.unwrap() {
        interpolate_yield_curve_response::Result::Error(error) => error,
        interpolate_yield_curve_response::Result::Point(_) => {
            panic!("unresolved Subject must fail before curve engine")
        }
    });
    let absent_carry = service
        .analyze_carry_roll(Request::new(AnalyzeCarryRollRequest {
            context: Some(analysis_context(
                CARRY_ROLL_ALGORITHM_ID,
                CARRY_ROLL_CONVENTION_PROFILE,
                absent.clone(),
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_subject_error(match absent_carry.result.unwrap() {
        analyze_carry_roll_response::Result::Error(error) => error,
        analyze_carry_roll_response::Result::Analysis(_) => {
            panic!("unresolved Subject must fail before carry engine")
        }
    });
    let absent_delivery = service
        .analyze_futures_delivery(Request::new(AnalyzeFuturesDeliveryRequest {
            context: Some(analysis_context(
                FUTURES_DELIVERY_ALGORITHM_ID,
                FUTURES_DELIVERY_CONVENTION_PROFILE,
                absent.clone(),
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_subject_error(match absent_delivery.result.unwrap() {
        analyze_futures_delivery_response::Result::Error(error) => error,
        analyze_futures_delivery_response::Result::Analysis(_) => {
            panic!("unresolved Subject must fail before delivery engine")
        }
    });
    let absent_hedge = service
        .analyze_futures_hedge(Request::new(AnalyzeFuturesHedgeRequest {
            context: Some(analysis_context(
                FUTURES_HEDGE_ALGORITHM_ID,
                FUTURES_HEDGE_CONVENTION_PROFILE,
                absent,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_subject_error(match absent_hedge.result.unwrap() {
        analyze_futures_hedge_response::Result::Error(error) => error,
        analyze_futures_hedge_response::Result::Analysis(_) => {
            panic!("unresolved Subject must fail before hedge engine")
        }
    });
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn non_delivery_rpcs_reject_an_unconsumed_funding_rule_pack_before_engines() {
    let calls = Arc::new(AtomicUsize::new(0));
    let service = service(&["rates:analyze"], Arc::clone(&calls));

    let rejected_bond = service
        .analyze_bond(Request::new(AnalyzeBondRequest {
            context: Some(context_with_unconsumed_funding(
                ALGORITHM_ID,
                CONVENTION_PROFILE,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_unconsumed_funding_error(match rejected_bond.result.unwrap() {
        analyze_bond_response::Result::Error(error) => error,
        analyze_bond_response::Result::Analysis(_) => {
            panic!("bond must reject a FundingRulePack it does not consume")
        }
    });

    let rejected_curve = service
        .interpolate_yield_curve(Request::new(InterpolateYieldCurveRequest {
            context: Some(context_with_unconsumed_funding(
                CURVE_ALGORITHM_ID,
                CURVE_CONVENTION_PROFILE,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_unconsumed_funding_error(match rejected_curve.result.unwrap() {
        interpolate_yield_curve_response::Result::Error(error) => error,
        interpolate_yield_curve_response::Result::Point(_) => {
            panic!("curve must reject a FundingRulePack it does not consume")
        }
    });

    let rejected_carry = service
        .analyze_carry_roll(Request::new(AnalyzeCarryRollRequest {
            context: Some(context_with_unconsumed_funding(
                CARRY_ROLL_ALGORITHM_ID,
                CARRY_ROLL_CONVENTION_PROFILE,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_unconsumed_funding_error(match rejected_carry.result.unwrap() {
        analyze_carry_roll_response::Result::Error(error) => error,
        analyze_carry_roll_response::Result::Analysis(_) => {
            panic!("carry must reject a FundingRulePack it does not consume")
        }
    });

    let rejected_hedge = service
        .analyze_futures_hedge(Request::new(AnalyzeFuturesHedgeRequest {
            context: Some(context_with_unconsumed_funding(
                FUTURES_HEDGE_ALGORITHM_ID,
                FUTURES_HEDGE_CONVENTION_PROFILE,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_unconsumed_funding_error(match rejected_hedge.result.unwrap() {
        analyze_futures_hedge_response::Result::Error(error) => error,
        analyze_futures_hedge_response::Result::Analysis(_) => {
            panic!("hedge must reject a FundingRulePack it does not consume")
        }
    });
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
        vec![fixture_subject(
            'S',
            FundingTier::DrAvailable,
            &["CFFEX"],
            &["futures-delivery"],
        )],
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

#[tokio::test]
async fn ac07_missing_subject_is_rejected_before_delivery_engine() {
    let pack = delivery_pack(1, "3", false);
    let calls = Arc::new(AtomicUsize::new(0));
    let service = delivery_service(
        vec![DefinitionValue::MarketRulePack(pack.clone())],
        Vec::new(),
        Arc::clone(&calls),
    );

    let mut request = delivery_request(&pack);
    request.context.as_mut().unwrap().subject_ref = None;
    let response = service
        .analyze_futures_delivery(Request::new(request))
        .await
        .expect("business error is transported")
        .into_inner();
    let Some(analyze_futures_delivery_response::Result::Error(error)) = response.result else {
        panic!("an unbound Subject must fail closed before the delivery engine");
    };
    assert_eq!(error.code, ErrorCode::ValidationFailed as i32);
    assert!(!error.retryable);
    assert_eq!(error.field_violations.len(), 1);
    assert_eq!(error.field_violations[0].field, "context.subject_ref");
    assert_eq!(error.field_violations[0].description, "主体版本缺失或无效");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ac06_subject_funding_tier_selects_pack_rate_and_changes_delivery_amounts() {
    let pack = delivery_pack(1, "3", false);
    let calls = Arc::new(AtomicUsize::new(0));
    let service = delivery_service(
        vec![DefinitionValue::MarketRulePack(pack.clone())],
        vec![
            fixture_subject(
                'S',
                FundingTier::DrAvailable,
                &["CFFEX"],
                &["futures-delivery"],
            ),
            fixture_subject('T', FundingTier::ROnly, &["CFFEX"], &["futures-delivery"]),
        ],
        Arc::clone(&calls),
    );

    let dr_response = service
        .analyze_futures_delivery(Request::new(delivery_request(&pack)))
        .await
        .expect("DR request is transported")
        .into_inner();
    let mut r_request = delivery_request(&pack);
    r_request.context.as_mut().unwrap().subject_ref = Some(ProtoVersionRef {
        id: Some(proto_ulid('T')),
        version: 1,
    });
    let r_response = service
        .analyze_futures_delivery(Request::new(r_request))
        .await
        .expect("R-only request is transported")
        .into_inner();

    let dr = delivery_result(&dr_response);
    let r_only = delivery_result(&r_response);
    let dr_measures = dr.candidates[0].measures.as_ref().unwrap();
    let r_measures = r_only.candidates[0].measures.as_ref().unwrap();
    let rate_delta = ExactDecimal::new(7, 3);
    let actual_days = ExactDecimal::from(59_u32);
    let annual_day_basis = ExactDecimal::from(365_u32);
    let expected_financing_delta =
        (decimal_value(dr_measures.purchase_dirty_price.as_ref().unwrap())
            * rate_delta
            * actual_days
            / annual_day_basis)
            .round_dp_with_strategy(12, RoundingStrategy::MidpointNearestEven);
    let actual_financing_delta = decimal_value(r_measures.financing_cost.as_ref().unwrap())
        - decimal_value(dr_measures.financing_cost.as_ref().unwrap());
    assert_within_one_fixed_decimal_tick(actual_financing_delta, expected_financing_delta);
    let actual_carry_delta = decimal_value(r_measures.holding_carry.as_ref().unwrap())
        - decimal_value(dr_measures.holding_carry.as_ref().unwrap());
    assert_within_one_fixed_decimal_tick(actual_carry_delta, -expected_financing_delta);
    assert_eq!(
        decimal_value(r_measures.funding_adjusted_irr.as_ref().unwrap())
            - decimal_value(dr_measures.funding_adjusted_irr.as_ref().unwrap()),
        -rate_delta
    );
    assert_eq!(
        decimal_value(r_measures.implied_repo_rate.as_ref().unwrap()),
        decimal_value(dr_measures.implied_repo_rate.as_ref().unwrap())
    );
    assert_eq!(
        dr.metadata
            .as_ref()
            .and_then(|metadata| metadata.subject_ref.as_ref())
            .and_then(|reference| reference.id.as_ref())
            .map(|id| id.value.as_str()),
        Some(id('S').as_str())
    );
    assert_eq!(
        dr.metadata
            .as_ref()
            .and_then(|metadata| metadata.funding_rule_pack.as_ref()),
        Some(&object_binding(
            'F',
            funding_pack().content_hash(),
            funding_pack().version(),
        ))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn delivery_rejects_funding_rule_pack_rate_unit_mismatch_before_engine() {
    let pack = delivery_pack(1, "3", false);
    let calls = Arc::new(AtomicUsize::new(0));
    let service = delivery_service(
        vec![DefinitionValue::MarketRulePack(pack.clone())],
        vec![fixture_subject(
            'S',
            FundingTier::DrAvailable,
            &["CFFEX"],
            &["futures-delivery"],
        )],
        Arc::clone(&calls),
    );
    let mut request = delivery_request(&pack);
    request
        .context
        .as_mut()
        .unwrap()
        .units
        .as_mut()
        .unwrap()
        .rate = Some(unit('Q'));

    let rejected = service
        .analyze_futures_delivery(Request::new(request))
        .await
        .expect("business error is transported")
        .into_inner();
    let error = match rejected.result.unwrap() {
        analyze_futures_delivery_response::Result::Error(error) => error,
        analyze_futures_delivery_response::Result::Analysis(_) => {
            panic!("a FundingRulePack rate unit mismatch must fail before delivery engine")
        }
    };
    assert_eq!(error.code, ErrorCode::ValidationFailed as i32);
    assert!(!error.retryable);
    assert!(error.field_violations.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ac29_hedge_requires_exact_subject_access_and_preserves_hand_formula() {
    let calls = Arc::new(AtomicUsize::new(0));
    let service = hedge_service(
        vec![
            fixture_subject(
                'S',
                FundingTier::DrAvailable,
                &["CFFEX"],
                &["futures-hedge"],
            ),
            fixture_subject('M', FundingTier::DrAvailable, &["CN"], &["futures-hedge"]),
            fixture_subject(
                'T',
                FundingTier::DrAvailable,
                &["CFFEX"],
                &["futures-delivery"],
            ),
        ],
        Arc::clone(&calls),
    );

    let permitted = service
        .analyze_futures_hedge(Request::new(hedge_request('S')))
        .await
        .expect("permitted hedge is transported")
        .into_inner();
    let Some(analyze_futures_hedge_response::Result::Analysis(result)) = permitted.result else {
        panic!("permitted Subject must reach the hedge engine");
    };
    let measures = result
        .measures
        .as_ref()
        .expect("hedge measures are present");
    // 0.045 * (1,000,000 / 100) / 0.9 = 500; -500 / 500 = -1.
    assert_eq!(
        decimal_value(measures.futures_contract_dv01.as_ref().unwrap()),
        ExactDecimal::from(500_i64)
    );
    assert_eq!(
        decimal_value(measures.raw_contracts.as_ref().unwrap()),
        ExactDecimal::from(-1_i64)
    );
    assert_eq!(measures.recommended_contracts, -1);
    assert_eq!(
        decimal_value(measures.residual_dv01.as_ref().unwrap()),
        ExactDecimal::ZERO
    );
    assert_eq!(
        decimal_value(measures.hedge_effectiveness.as_ref().unwrap()),
        ExactDecimal::ONE
    );
    assert_eq!(
        result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.subject_ref.as_ref())
            .and_then(|reference| reference.id.as_ref())
            .map(|id| id.value.as_str()),
        Some(id('S').as_str())
    );
    assert!(
        result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.funding_rule_pack.as_ref())
            .is_none(),
        "only delivery may carry a consumed FundingRulePack"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    calls.store(0, Ordering::SeqCst);

    for subject in ['M', 'T'] {
        let rejected = service
            .analyze_futures_hedge(Request::new(hedge_request(subject)))
            .await
            .expect("business error is transported")
            .into_inner();
        assert_subject_error(match rejected.result.unwrap() {
            analyze_futures_hedge_response::Result::Error(error) => error,
            analyze_futures_hedge_response::Result::Analysis(_) => {
                panic!("missing market or tool access must fail before hedge engine")
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn ac09_first_issue_date_selects_tax_rule_and_recomputes_after_tax() {
    let pack = tax_pack(&canonical_tax_payload());
    let calls = Arc::new(AtomicUsize::new(0));
    let service = tax_bond_service(
        vec![DefinitionValue::MarketRulePack(pack.clone())],
        vec![tax_fixture_subject(
            'S',
            "synthetic-vat-taxable-a",
            "synthetic-income-taxable-a",
        )],
        Arc::clone(&calls),
    );

    let pre_cutoff = service
        .analyze_bond(Request::new(tax_bond_request(
            &pack,
            'S',
            "2025-08-07",
            "2025-08-09",
            ProtoValueAddedTaxStatus::Exempt,
            ProtoIncomeTaxStatus::Exempt,
        )))
        .await
        .expect("pre-cutoff Bond response is transported")
        .into_inner();
    let post_cutoff = service
        .analyze_bond(Request::new(tax_bond_request(
            &pack,
            'S',
            "2025-08-08",
            "2025-08-08",
            ProtoValueAddedTaxStatus::Taxable,
            ProtoIncomeTaxStatus::Taxable,
        )))
        .await
        .expect("post-cutoff Bond response is transported")
        .into_inner();

    let pre_cutoff = tax_bond_result(&pre_cutoff);
    let post_cutoff = tax_bond_result(&post_cutoff);
    let pre_cutoff_after_tax = pre_cutoff
        .after_tax
        .as_ref()
        .expect("AnalyzeBond emits the consumed tax-adjusted result");
    let post_cutoff_after_tax = post_cutoff
        .after_tax
        .as_ref()
        .expect("AnalyzeBond emits the consumed tax-adjusted result");
    assert_coupon_tax_rate(
        &pre_cutoff.cashflows,
        &pre_cutoff_after_tax.cashflows,
        ExactDecimal::ZERO,
    );
    assert_coupon_tax_rate(
        &post_cutoff.cashflows,
        &post_cutoff_after_tax.cashflows,
        ExactDecimal::new(13, 2),
    );
    assert_ne!(
        pre_cutoff_after_tax.yield_to_maturity, post_cutoff_after_tax.yield_to_maturity,
        "the 2025-08-08 boundary must select a different tax-adjusted YTM"
    );
    assert_eq!(
        post_cutoff
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.tax_rule_pack.as_ref()),
        Some(&object_binding('T', pack.content_hash(), pack.version(),))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn ac10_subject_tax_profiles_preserve_pre_tax_and_change_after_tax() {
    let pack = tax_pack(&canonical_tax_payload());
    let calls = Arc::new(AtomicUsize::new(0));
    let service = tax_bond_service(
        vec![DefinitionValue::MarketRulePack(pack.clone())],
        vec![
            tax_fixture_subject('S', "synthetic-vat-taxable-a", "synthetic-income-taxable-a"),
            tax_fixture_subject('V', "synthetic-vat-taxable-b", "synthetic-income-taxable-b"),
        ],
        Arc::clone(&calls),
    );

    let first = service
        .analyze_bond(Request::new(tax_bond_request(
            &pack,
            'S',
            "2025-08-08",
            "2025-08-08",
            ProtoValueAddedTaxStatus::Taxable,
            ProtoIncomeTaxStatus::Taxable,
        )))
        .await
        .expect("first exact Subject response is transported")
        .into_inner();
    let second = service
        .analyze_bond(Request::new(tax_bond_request(
            &pack,
            'V',
            "2025-08-08",
            "2025-08-08",
            ProtoValueAddedTaxStatus::Taxable,
            ProtoIncomeTaxStatus::Taxable,
        )))
        .await
        .expect("second exact Subject response is transported")
        .into_inner();

    let first = tax_bond_result(&first);
    let second = tax_bond_result(&second);
    assert_eq!(
        first.cashflows, second.cashflows,
        "TaxTreatment must not alter the native pre-tax cashflows"
    );
    assert_eq!(
        first.measures, second.measures,
        "TaxTreatment must not alter the native pre-tax measures"
    );
    let first_after_tax = first
        .after_tax
        .as_ref()
        .expect("first after-tax result exists");
    let second_after_tax = second
        .after_tax
        .as_ref()
        .expect("second after-tax result exists");
    assert_coupon_tax_rate(
        &first.cashflows,
        &first_after_tax.cashflows,
        ExactDecimal::new(13, 2),
    );
    assert_coupon_tax_rate(
        &second.cashflows,
        &second_after_tax.cashflows,
        ExactDecimal::new(25, 2),
    );
    assert_ne!(first_after_tax.cashflows, second_after_tax.cashflows);
    assert_ne!(
        first_after_tax.yield_to_maturity,
        second_after_tax.yield_to_maturity
    );
    assert_ne!(
        first
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.subject_ref.as_ref()),
        second
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.subject_ref.as_ref()),
        "metadata retains the exact Subject that selected each tax profile"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn ac09_tax_rule_missing_interval_attribute_or_profile_fails_before_bond_engine() {
    let cases = vec![
        (
            tax_pack(&tax_payload_without_pre_cutoff_interval()),
            'S',
            "2025-08-07",
            "2025-08-09",
            ProtoValueAddedTaxStatus::Exempt,
            ProtoIncomeTaxStatus::Exempt,
            "context.tax_rule_pack.content.coupon_rules[first_issue_date=2025-08-07]",
        ),
        (
            tax_pack(&canonical_tax_payload()),
            'S',
            "2025-08-08",
            "2025-08-08",
            ProtoValueAddedTaxStatus::Exempt,
            ProtoIncomeTaxStatus::Exempt,
            "context.tax_rule_pack.content.coupon_rules[first_issue_date=2025-08-08].tax_attributes",
        ),
        (
            tax_pack(&tax_payload_without_profile_b()),
            'V',
            "2025-08-08",
            "2025-08-08",
            ProtoValueAddedTaxStatus::Taxable,
            ProtoIncomeTaxStatus::Taxable,
            "context.tax_rule_pack.content.coupon_rules[first_issue_date=2025-08-08].rates[vat_profile=synthetic-vat-taxable-b][income_profile=synthetic-income-taxable-b]",
        ),
    ];

    for (pack, subject, first_issue, current_issue, vat, income, path) in cases {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = tax_bond_service(
            vec![DefinitionValue::MarketRulePack(pack.clone())],
            vec![tax_fixture_subject(
                subject,
                if subject == 'S' {
                    "synthetic-vat-taxable-a"
                } else {
                    "synthetic-vat-taxable-b"
                },
                if subject == 'S' {
                    "synthetic-income-taxable-a"
                } else {
                    "synthetic-income-taxable-b"
                },
            )],
            Arc::clone(&calls),
        );
        let response = service
            .analyze_bond(Request::new(tax_bond_request(
                &pack,
                subject,
                first_issue,
                current_issue,
                vat,
                income,
            )))
            .await
            .expect("business error is transported")
            .into_inner();
        let error = match response.result.unwrap() {
            analyze_bond_response::Result::Error(error) => error,
            analyze_bond_response::Result::Analysis(_) => {
                panic!("a missing TaxRulePack item must fail before bond engine")
            }
        };
        assert_tax_rule_missing_error(&error, path);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "{path}");
    }
}

#[tokio::test]
async fn non_bond_rpcs_reject_an_unconsumed_tax_rule_pack_before_engines() {
    let calls = Arc::new(AtomicUsize::new(0));
    let service = service(&["rates:analyze"], Arc::clone(&calls));

    let curve = service
        .interpolate_yield_curve(Request::new(InterpolateYieldCurveRequest {
            context: Some(context_with_unconsumed_tax(
                CURVE_ALGORITHM_ID,
                CURVE_CONVENTION_PROFILE,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    let curve_error = match curve.result.unwrap() {
        interpolate_yield_curve_response::Result::Error(error) => error,
        interpolate_yield_curve_response::Result::Point(_) => {
            panic!("curve must reject an unconsumed TaxRulePack")
        }
    };
    assert_unconsumed_tax_error(&curve_error);

    let carry = service
        .analyze_carry_roll(Request::new(AnalyzeCarryRollRequest {
            context: Some(context_with_unconsumed_tax(
                CARRY_ROLL_ALGORITHM_ID,
                CARRY_ROLL_CONVENTION_PROFILE,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    let carry_error = match carry.result.unwrap() {
        analyze_carry_roll_response::Result::Error(error) => error,
        analyze_carry_roll_response::Result::Analysis(_) => {
            panic!("carry must reject an unconsumed TaxRulePack")
        }
    };
    assert_unconsumed_tax_error(&carry_error);

    let delivery = service
        .analyze_futures_delivery(Request::new(AnalyzeFuturesDeliveryRequest {
            context: Some(context_with_unconsumed_tax(
                FUTURES_DELIVERY_ALGORITHM_ID,
                FUTURES_DELIVERY_CONVENTION_PROFILE,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    let delivery_error = match delivery.result.unwrap() {
        analyze_futures_delivery_response::Result::Error(error) => error,
        analyze_futures_delivery_response::Result::Analysis(_) => {
            panic!("delivery must reject an unconsumed TaxRulePack")
        }
    };
    assert_unconsumed_tax_error(&delivery_error);

    let hedge = service
        .analyze_futures_hedge(Request::new(AnalyzeFuturesHedgeRequest {
            context: Some(context_with_unconsumed_tax(
                FUTURES_HEDGE_ALGORITHM_ID,
                FUTURES_HEDGE_CONVENTION_PROFILE,
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    let hedge_error = match hedge.result.unwrap() {
        analyze_futures_hedge_response::Result::Error(error) => error,
        analyze_futures_hedge_response::Result::Analysis(_) => {
            panic!("hedge must reject an unconsumed TaxRulePack")
        }
    };
    assert_unconsumed_tax_error(&hedge_error);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

fn canonical_tax_payload() -> TaxRulePack {
    TaxRulePack {
        coupon_rules: vec![
            tax_rule(
                "2000-01-01",
                "2025-08-08",
                ProtoValueAddedTaxStatus::Exempt,
                ProtoIncomeTaxStatus::Exempt,
                vec![tax_rate(
                    "synthetic-vat-taxable-a",
                    "synthetic-income-taxable-a",
                    "0",
                    0,
                )],
            ),
            tax_rule(
                "2025-08-08",
                "",
                ProtoValueAddedTaxStatus::Taxable,
                ProtoIncomeTaxStatus::Taxable,
                vec![
                    tax_rate(
                        "synthetic-vat-taxable-a",
                        "synthetic-income-taxable-a",
                        "13",
                        2,
                    ),
                    tax_rate(
                        "synthetic-vat-taxable-b",
                        "synthetic-income-taxable-b",
                        "25",
                        2,
                    ),
                ],
            ),
        ],
    }
}

fn tax_payload_without_pre_cutoff_interval() -> TaxRulePack {
    let mut payload = canonical_tax_payload();
    payload.coupon_rules.remove(0);
    payload
}

fn tax_payload_without_profile_b() -> TaxRulePack {
    let mut payload = canonical_tax_payload();
    payload.coupon_rules[1].rates.pop();
    payload
}

fn tax_rule(
    first_issue_from: &str,
    first_issue_to: &str,
    value_added_tax_status: ProtoValueAddedTaxStatus,
    income_tax_status: ProtoIncomeTaxStatus,
    rates: Vec<SubjectCouponTaxRate>,
) -> BondCouponTaxRule {
    BondCouponTaxRule {
        first_issue_from: first_issue_from.to_owned(),
        first_issue_to: first_issue_to.to_owned(),
        tax_attributes: Some(BondTaxAttributes {
            value_added_tax_status: value_added_tax_status as i32,
            income_tax_status: income_tax_status as i32,
        }),
        rates,
    }
}

fn tax_rate(
    value_added_tax_profile: &str,
    income_tax_profile: &str,
    coefficient: &str,
    scale: u32,
) -> SubjectCouponTaxRate {
    SubjectCouponTaxRate {
        value_added_tax_profile: value_added_tax_profile.to_owned(),
        income_tax_profile: income_tax_profile.to_owned(),
        coupon_tax_rate: Some(decimal(coefficient, scale, &unit('P'))),
    }
}

fn tax_pack(payload: &TaxRulePack) -> MarketRulePack {
    let content = RulePackContent::new(TAX_TYPE_URL, payload.encode_to_vec()).unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('T'),
            version: Version::new(1).unwrap(),
            owner: owner(),
            market: TAX_MARKET.to_owned(),
            rule_type: TAX_RULE_TYPE.to_owned(),
            source: "synthetic-r3b-tax-fixture-not-authoritative".to_owned(),
            effective: EffectivePeriod::new(domain_time(1), domain_time(31)).unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(content.value()),
        },
        content,
    )
    .unwrap()
}

fn tax_bond_request(
    pack: &MarketRulePack,
    subject: char,
    first_issue_date: &str,
    current_issue_date: &str,
    value_added_tax_status: ProtoValueAddedTaxStatus,
    income_tax_status: ProtoIncomeTaxStatus,
) -> AnalyzeBondRequest {
    let rate = unit('P');
    let mut context = analysis_context(
        ALGORITHM_ID,
        CONVENTION_PROFILE,
        Some(ProtoVersionRef {
            id: Some(proto_ulid(subject)),
            version: 1,
        }),
    );
    context.tax_rule_pack = Some(object_binding('T', pack.content_hash(), pack.version()));
    AnalyzeBondRequest {
        context: Some(context),
        bond: Some(object_binding(
            'N',
            &ContentHash::digest(b"synthetic-tax-bond"),
            1,
        )),
        valuation_at: Some(proto_time(20)),
        settlement_date: "2026-07-14".to_owned(),
        calendar_requirement: CalendarRequirement::ReferenceReplay as i32,
        calendar: Some(CalendarBinding {
            calendar_id: "CN-SYNTHETIC-R3B".to_owned(),
            version: 1,
            content_hash: Some(Sha256 {
                value: ContentHash::digest(b"synthetic-tax-calendar")
                    .as_bytes()
                    .to_vec(),
            }),
            coverage_start: "2020-01-01".to_owned(),
            coverage_end: "2031-12-31".to_owned(),
            non_business_days: Vec::new(),
            work_weekends: Vec::new(),
        }),
        terms: Some(BondTerms {
            first_issue_date: first_issue_date.to_owned(),
            current_issue_date: current_issue_date.to_owned(),
            maturity_date: "2030-08-08".to_owned(),
            frequency: CouponFrequency::Annual as i32,
            coupon_rate: Some(decimal("25", 3, &rate)),
            face_amount: Some(decimal("100", 0, &rate)),
            cumulative_issued_amount: Some(decimal("100", 0, &rate)),
            tax_attributes: Some(BondTaxAttributes {
                value_added_tax_status: value_added_tax_status as i32,
                income_tax_status: income_tax_status as i32,
            }),
        }),
        input: Some(
            ficant_contracts::ficant::rates::v1::analyze_bond_request::Input::CleanPrice(decimal(
                "100", 0, &rate,
            )),
        ),
    }
}

fn tax_fixture_subject(
    suffix: char,
    value_added_tax_profile: &str,
    income_tax_profile: &str,
) -> SubjectRecord {
    let subject = Subject::new(id(suffix), "R3b synthetic tax Subject").unwrap();
    let reference =
        ficant_domain::primitives::VersionRef::new(subject.id().clone(), Version::new(1).unwrap());
    let version = SubjectVersion::new(
        reference,
        AccessSet::new(["CN"], ["bond-analytics"]).unwrap(),
        FundingTier::DrAvailable,
        TaxTreatment::new(value_added_tax_profile, income_tax_profile).unwrap(),
        "synthetic-assessment",
        "synthetic-liability",
        None,
    )
    .unwrap();
    SubjectRecord::new(subject, version).unwrap()
}

fn tax_bond_result(
    response: &ficant_contracts::ficant::rates::v1::AnalyzeBondResponse,
) -> &ficant_contracts::ficant::rates::v1::AnalyzeBondResult {
    let Some(analyze_bond_response::Result::Analysis(result)) = &response.result else {
        panic!("complete TaxRulePack request must produce Bond analysis");
    };
    result
}

fn assert_coupon_tax_rate(
    pre_tax: &[ficant_contracts::ficant::rates::v1::DerivedCashflow],
    after_tax: &[ficant_contracts::ficant::rates::v1::DerivedCashflow],
    coupon_tax_rate: ExactDecimal,
) {
    assert_eq!(pre_tax.len(), after_tax.len());
    for (pre_tax, after_tax) in pre_tax.iter().zip(after_tax) {
        assert_eq!(pre_tax.sequence, after_tax.sequence);
        assert_eq!(pre_tax.principal, after_tax.principal);
        let pre_coupon = decimal_value(pre_tax.coupon.as_ref().unwrap());
        let expected_coupon = (pre_coupon * (ExactDecimal::ONE - coupon_tax_rate))
            .round_dp_with_strategy(12, RoundingStrategy::MidpointNearestEven);
        let actual_coupon = decimal_value(after_tax.coupon.as_ref().unwrap());
        assert_within_one_fixed_decimal_tick(actual_coupon, expected_coupon);
        let expected_total = expected_coupon + decimal_value(pre_tax.principal.as_ref().unwrap());
        let actual_total = decimal_value(after_tax.total.as_ref().unwrap());
        assert_within_one_fixed_decimal_tick(actual_total, expected_total);
    }
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
    let funding = funding_pack();
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
            subject_ref: Some(ProtoVersionRef {
                id: Some(proto_ulid('S')),
                version: 1,
            }),
            funding_rule_pack: Some(object_binding(
                'F',
                funding.content_hash(),
                funding.version(),
            )),
            tax_rule_pack: None,
        }),
        futures_contract: Some(object_binding('C', &ContentHash::digest(b"contract"), 1)),
        valuation_at: Some(proto_time(20)),
        purchase_date: "2026-07-21".to_owned(),
        delivery_month_first: "2026-09-01".to_owned(),
        delivery_date: "2026-09-18".to_owned(),
        product: ProtoCgbFuturesProduct::T as i32,
        futures_clean_price: Some(decimal("995", 1, &price)),
        candidates: vec![FuturesDeliverableCandidate {
            bond: Some(object_binding('D', &ContentHash::digest(b"bond"), 1)),
            terms: Some(BondTerms {
                first_issue_date: "2024-08-15".to_owned(),
                current_issue_date: "2024-08-15".to_owned(),
                maturity_date: "2034-08-15".to_owned(),
                frequency: CouponFrequency::Semiannual as i32,
                coupon_rate: Some(decimal("25", 3, &rate)),
                face_amount: Some(decimal("100", 0, &rate)),
                cumulative_issued_amount: Some(decimal("100", 0, &rate)),
                tax_attributes: Some(BondTaxAttributes {
                    value_added_tax_status: ProtoValueAddedTaxStatus::Taxable as i32,
                    income_tax_status: ProtoIncomeTaxStatus::Taxable as i32,
                }),
            }),
            spot_clean_price: Some(decimal("10125", 2, &price)),
        }],
    }
}

fn hedge_request(subject_suffix: char) -> AnalyzeFuturesHedgeRequest {
    let rate = unit('P');
    AnalyzeFuturesHedgeRequest {
        context: Some(analysis_context(
            FUTURES_HEDGE_ALGORITHM_ID,
            FUTURES_HEDGE_CONVENTION_PROFILE,
            Some(ProtoVersionRef {
                id: Some(proto_ulid(subject_suffix)),
                version: 1,
            }),
        )),
        target_risk_artifact: Some(object_binding('Q', &ContentHash::digest(b"risk"), 1)),
        delivery_artifact: Some(object_binding('V', &ContentHash::digest(b"delivery"), 1)),
        ctd_analytics_artifact: Some(object_binding('W', &ContentHash::digest(b"ctd"), 1)),
        futures_contract: Some(object_binding('C', &ContentHash::digest(b"contract"), 1)),
        ctd_bond: Some(object_binding('D', &ContentHash::digest(b"bond"), 1)),
        valuation_at: Some(proto_time(20)),
        product: ProtoCgbFuturesProduct::T as i32,
        target_dv01: Some(decimal("500", 0, &rate)),
        ctd_dv01_per_100: Some(decimal("45", 3, &rate)),
        conversion_factor: Some(decimal("9", 1, &rate)),
    }
}

fn analysis_context(
    algorithm_id: &str,
    convention_profile: &str,
    subject_ref: Option<ProtoVersionRef>,
) -> AnalysisContext {
    let rate = unit('P');
    AnalysisContext {
        owner: Some(proto_owner()),
        rule_pack: Some(object_binding('R', &ContentHash::digest(b"rule"), 1)),
        data_snapshot: Some(object_binding('E', &ContentHash::digest(b"snapshot"), 1)),
        algorithm: Some(AlgorithmBinding {
            algorithm_id: algorithm_id.to_owned(),
            algorithm_version: 1,
            convention_profile: convention_profile.to_owned(),
            abi_version: 1,
        }),
        units: Some(AnalysisUnits {
            currency_amount: Some(rate.clone()),
            price_per_100: Some(rate.clone()),
            rate: Some(rate.clone()),
            years: Some(rate.clone()),
            years_squared: Some(rate.clone()),
            dv01_per_100: Some(rate.clone()),
            dv01: Some(rate.clone()),
            dimensionless: Some(rate.clone()),
            contract_count: Some(rate),
        }),
        subject_ref,
        funding_rule_pack: None,
        tax_rule_pack: None,
    }
}

fn context_with_unconsumed_funding(
    algorithm_id: &str,
    convention_profile: &str,
) -> AnalysisContext {
    let mut context = analysis_context(
        algorithm_id,
        convention_profile,
        Some(ProtoVersionRef {
            id: Some(proto_ulid('S')),
            version: 1,
        }),
    );
    context.funding_rule_pack = Some(object_binding(
        'F',
        &ContentHash::digest(b"unconsumed-funding-pack"),
        1,
    ));
    context
}

fn context_with_unconsumed_tax(algorithm_id: &str, convention_profile: &str) -> AnalysisContext {
    let mut context = analysis_context(
        algorithm_id,
        convention_profile,
        Some(ProtoVersionRef {
            id: Some(proto_ulid('S')),
            version: 1,
        }),
    );
    context.tax_rule_pack = Some(object_binding(
        'T',
        &ContentHash::digest(b"unconsumed-tax-pack"),
        1,
    ));
    context
}

fn assert_subject_error(error: ficant_contracts::ficant::core::v1::ErrorDetail) {
    let ficant_contracts::ficant::core::v1::ErrorDetail {
        code,
        retryable,
        field_violations,
        ..
    } = error;
    assert_eq!(code, ErrorCode::ValidationFailed as i32);
    assert!(!retryable);
    assert_eq!(field_violations.len(), 1);
    assert_eq!(field_violations[0].field, "context.subject_ref");
    assert_eq!(field_violations[0].description, "主体版本缺失或无效");
}

fn assert_unconsumed_funding_error(error: ficant_contracts::ficant::core::v1::ErrorDetail) {
    let ficant_contracts::ficant::core::v1::ErrorDetail {
        code,
        retryable,
        field_violations,
        ..
    } = error;
    assert_eq!(code, ErrorCode::ValidationFailed as i32);
    assert!(!retryable);
    assert!(
        field_violations.is_empty(),
        "an unconsumed pack is rejected as an invalid request without pretending it was read"
    );
}

fn assert_unconsumed_tax_error(error: &ficant_contracts::ficant::core::v1::ErrorDetail) {
    let ficant_contracts::ficant::core::v1::ErrorDetail {
        code,
        retryable,
        field_violations,
        ..
    } = error;
    assert_eq!(*code, ErrorCode::ValidationFailed as i32);
    assert!(!*retryable);
    assert!(
        field_violations.is_empty(),
        "an unconsumed TaxRulePack is rejected without pretending it was read"
    );
}

fn assert_tax_rule_missing_error(
    error: &ficant_contracts::ficant::core::v1::ErrorDetail,
    expected_path: &str,
) {
    assert_eq!(error.code, ErrorCode::ValidationFailed as i32);
    assert!(!error.retryable);
    assert_eq!(error.field_violations.len(), 1);
    assert_eq!(error.field_violations[0].field, expected_path);
    assert_eq!(
        error.field_violations[0].description,
        "规则包缺少计算所需项"
    );
}

fn funding_pack() -> MarketRulePack {
    let rate = unit('P');
    let payload = FundingRulePack {
        rates: vec![
            FundingTierRate {
                funding_tier: ProtoFundingTier::DrAvailable as i32,
                annual_financing_rate: Some(decimal("18", 3, &rate)),
            },
            FundingTierRate {
                funding_tier: ProtoFundingTier::ROnly as i32,
                annual_financing_rate: Some(decimal("25", 3, &rate)),
            },
        ],
    };
    let content = RulePackContent::new(FUNDING_TYPE_URL, payload.encode_to_vec()).unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('F'),
            version: Version::new(1).unwrap(),
            owner: owner(),
            market: FUNDING_MARKET.to_owned(),
            rule_type: FUNDING_RULE_TYPE.to_owned(),
            source: "synthetic-r3a-fixture".to_owned(),
            effective: EffectivePeriod::new(domain_time(1), domain_time(31)).unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(content.value()),
        },
        content,
    )
    .unwrap()
}

fn fixture_subject(
    suffix: char,
    funding_tier: FundingTier,
    markets: &[&str],
    tools: &[&str],
) -> SubjectRecord {
    let subject = Subject::new(id(suffix), "R3a fixture Subject").unwrap();
    let reference =
        ficant_domain::primitives::VersionRef::new(subject.id().clone(), Version::new(1).unwrap());
    let version = SubjectVersion::new(
        reference,
        AccessSet::new(markets.iter().copied(), tools.iter().copied()).unwrap(),
        funding_tier,
        TaxTreatment::new("synthetic-vat", "synthetic-income").unwrap(),
        "synthetic-assessment",
        "synthetic-liability",
        None,
    )
    .unwrap();
    SubjectRecord::new(subject, version).unwrap()
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

fn delivery_result(
    response: &ficant_contracts::ficant::rates::v1::AnalyzeFuturesDeliveryResponse,
) -> &ficant_contracts::ficant::rates::v1::AnalyzeFuturesDeliveryResult {
    let Some(analyze_futures_delivery_response::Result::Analysis(result)) = &response.result else {
        panic!("complete RulePack must produce an analysis");
    };
    result
}

fn decimal_value(value: &DecimalValue) -> ExactDecimal {
    ExactDecimal::from_i128_with_scale(value.coefficient.parse().unwrap(), value.scale)
}

fn assert_within_one_fixed_decimal_tick(actual: ExactDecimal, expected: ExactDecimal) {
    let one_tick = ExactDecimal::new(1, 12);
    assert!(
        (actual - expected).abs() <= one_tick,
        "actual {actual} differs from Decimal hand calculation {expected} by more than one native fixed-Decimal tick"
    );
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
