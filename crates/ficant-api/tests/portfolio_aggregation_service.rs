use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use ficant_api::{
    PlatformApplication, PlatformPort, PortfolioAggregationBackend,
    PortfolioAggregationBackendResult, PortfolioAggregationGrpcService,
    RequestedNormalizedPortfolioContext, SessionPolicy, SystemClock, TrustedIdentity,
};
use ficant_application::ports::{ApplicationResult, AuthorizedPrincipal};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::portfolio::v1 as pb;
use ficant_contracts::ficant::portfolio::v1::portfolio_aggregation_service_server::PortfolioAggregationService;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{MarketTime, OwnerRef, Ulid};
use prost_types::Timestamp;
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

struct RejectingBackend {
    calls: AtomicUsize,
}

#[async_trait]
impl PortfolioAggregationBackend for RejectingBackend {
    async fn get_overview(
        &self,
        principal: &AuthorizedPrincipal,
        context: RequestedNormalizedPortfolioContext,
    ) -> ApplicationResult<PortfolioAggregationBackendResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(principal.active_role(), PlatformRole::Researcher);
        assert_eq!(context.scope.member_portfolios().len(), 1);
        Err(ApplicationError::new(
            ApplicationErrorCategory::NotFound,
            false,
        ))
    }
}

#[tokio::test]
async fn authorized_exact_context_reaches_typed_backend_once() {
    fn assert_service<T: PortfolioAggregationService>() {}
    assert_service::<PortfolioAggregationGrpcService>();

    let backend = Arc::new(RejectingBackend {
        calls: AtomicUsize::new(0),
    });
    let service = service(
        backend.clone(),
        PlatformRole::Researcher,
        vec!["portfolio:read"],
    );
    let response = service
        .get_portfolio_overview(Request::new(valid_request()))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::get_portfolio_overview_response::Result::Error(error)) = response.result else {
        panic!("typed backend failure must stay a safe business error")
    };
    assert_eq!(error.code, core::ErrorCode::NotFound as i32);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn role_and_scope_drift_fail_before_backend() {
    for (role, scopes) in [
        (PlatformRole::PlatformAdmin, vec!["portfolio:read"]),
        (PlatformRole::Researcher, Vec::new()),
    ] {
        let backend = Arc::new(RejectingBackend {
            calls: AtomicUsize::new(0),
        });
        let response = service(backend.clone(), role, scopes)
            .get_portfolio_overview(Request::new(valid_request()))
            .await
            .unwrap()
            .into_inner();
        let Some(pb::get_portfolio_overview_response::Result::Error(error)) = response.result
        else {
            panic!("authorization drift must fail closed")
        };
        assert_eq!(error.code, core::ErrorCode::Forbidden as i32);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn incomplete_or_non_exact_normalized_context_fails_before_backend() {
    let backend = Arc::new(RejectingBackend {
        calls: AtomicUsize::new(0),
    });
    let service = service(
        backend.clone(),
        PlatformRole::Researcher,
        vec!["portfolio:read"],
    );

    let missing = service
        .get_portfolio_overview(Request::new(pb::GetPortfolioOverviewRequest {
            context: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        missing.result,
        Some(pb::get_portfolio_overview_response::Result::Error(_))
    ));

    let mut drift = valid_request();
    drift
        .context
        .as_mut()
        .unwrap()
        .scope
        .as_mut()
        .unwrap()
        .member_portfolios[0]
        .content_hash = None;
    let drift = service
        .get_portfolio_overview(Request::new(drift))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        drift.result,
        Some(pb::get_portfolio_overview_response::Result::Error(_))
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}

fn service(
    backend: Arc<RejectingBackend>,
    role: PlatformRole,
    scopes: Vec<&str>,
) -> PortfolioAggregationGrpcService {
    let identity = TrustedIdentity::implicit(
        "portfolio-aggregation-test",
        id("01ARZ3NDEKTSV4RRFFQ69G5F00"),
        owner().tenant_id().clone(),
        vec![owner().owner_id().clone()],
        role,
        scopes,
    )
    .unwrap();
    let platform: Arc<dyn PlatformPort> = Arc::new(
        PlatformApplication::try_new(
            Arc::new(SystemClock),
            SessionPolicy::new(900, 60).unwrap(),
            KEY,
            vec![],
            Some(identity),
            vec![],
        )
        .unwrap(),
    );
    PortfolioAggregationGrpcService::new(platform, backend, KEY).unwrap()
}

fn valid_request() -> pb::GetPortfolioOverviewRequest {
    let exact = exact_lineage("01ARZ3NDEKTSV4RRFFQ69G5F10", b"portfolio");
    pb::GetPortfolioOverviewRequest {
        context: Some(pb::NormalizedPortfolioContext {
            scope: Some(pb::ExactPortfolioScope {
                scope: Some(pb::exact_portfolio_scope::Scope::Portfolio(exact.clone())),
                member_portfolios: vec![exact],
            }),
            valuation_at: Some(proto_time(10)),
            knowledge_at: Some(proto_time(11)),
            currency: pb::PortfolioCurrencyMode::Cny as i32,
            currency_unit: Some(versioned_unit("01ARZ3NDEKTSV4RRFFQ69G5F11")),
            look_through: pb::PortfolioLookThroughMode::None as i32,
            benchmark: Some(pb::BenchmarkRef {
                benchmark: Some(version_ref("01ARZ3NDEKTSV4RRFFQ69G5F12")),
                content_hash: Some(hash(b"benchmark")),
            }),
            period: pb::PortfolioPeriodPreset::OneDay as i32,
            period_from: Some(proto_time(9)),
            period_to: Some(proto_time(10)),
            metric_convention: Some(pb::PortfolioMetricConventionRef {
                convention: Some(version_ref("01ARZ3NDEKTSV4RRFFQ69G5F13")),
                content_hash: Some(hash(b"convention")),
            }),
        }),
    }
}

fn exact_lineage(value: &str, content: &[u8]) -> core::LineageRef {
    core::LineageRef {
        object_id: Some(core::Ulid {
            value: value.to_owned(),
        }),
        version: 1,
        content_hash: Some(hash(content)),
    }
}

fn version_ref(value: &str) -> core::VersionRef {
    core::VersionRef {
        id: Some(core::Ulid {
            value: value.to_owned(),
        }),
        version: 1,
    }
}

fn versioned_unit(value: &str) -> core::UnitRef {
    core::UnitRef {
        unit_id: Some(core::Ulid {
            value: value.to_owned(),
        }),
        version: 1,
    }
}

fn hash(value: &[u8]) -> core::Sha256 {
    core::Sha256 {
        value: ficant_domain::primitives::ContentHash::digest(value)
            .as_bytes()
            .to_vec(),
    }
}

fn proto_time(hour: u32) -> core::MarketTime {
    let value = time(hour);
    core::MarketTime {
        instant: Some(Timestamp {
            seconds: value.instant().timestamp(),
            nanos: 0,
        }),
        market_timezone: value.market_timezone().to_owned(),
        local_trading_date: value.local_trading_date().to_string(),
    }
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 8, 21, hour, 0, 0)
            .single()
            .unwrap(),
        "Asia/Shanghai",
        chrono::NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
    )
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(
        id("01ARZ3NDEKTSV4RRFFQ69G5F01"),
        id("01ARZ3NDEKTSV4RRFFQ69G5F02"),
    )
}

fn id(value: &str) -> Ulid {
    Ulid::new(value).unwrap()
}
