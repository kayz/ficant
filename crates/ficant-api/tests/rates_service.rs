use ficant_api::{
    PlatformApplication, PlatformPort, RatesGrpcService, SessionPolicy, SystemClock,
    TrustedIdentity,
};
use ficant_application::ports::{
    BondAnalyticsEngine, CarryRollEngine, FuturesDeliveryEngine, FuturesHedgeEngine,
    YieldCurveEngine,
};
use ficant_contracts::ficant::core::v1::ErrorCode;
use ficant_contracts::ficant::rates::v1::{
    AnalyzeBondRequest, analyze_bond_response,
    rates_analytics_service_server::RatesAnalyticsService,
};
use ficant_domain::analytics::{AnalyticsError, BondAnalyticsInput, BondAnalyticsResult};
use ficant_domain::curves::{CarryRollInput, CarryRollResult, YieldCurvePoint, YieldCurveQuery};
use ficant_domain::futures_delivery::{FuturesDeliverableInput, FuturesDeliveryResult};
use ficant_domain::futures_hedge::{FuturesHedgeInput, FuturesHedgeResult};
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
        Arc::new(engine),
        KEY,
    )
    .expect("rates service is valid")
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
