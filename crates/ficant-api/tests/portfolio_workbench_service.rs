use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use ficant_api::{
    PlatformApplication, PlatformPort, PortfolioWorkbenchBackend, PortfolioWorkbenchGrpcService,
    SessionPolicy, SystemClock, TrustedIdentity,
};
use ficant_application::ports::{
    ApplicationResult, AuthorizedPrincipal, ExactPortfolioScope, ExactPortfolioScopeKind,
    NormalizedPortfolioContext, PortfolioContextInput, PortfolioCurrencyMode,
    PortfolioLookThroughMode, PortfolioPeriodPreset,
};
use ficant_application::use_cases::portfolio_workbench::{
    PortfolioDefaultContextResult, PortfolioPageEnvelope, PortfolioPageSelection,
    PortfolioWorkbenchPageId,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::portfolio::v1 as pb;
use ficant_contracts::ficant::portfolio::v1::portfolio_workbench_service_server::PortfolioWorkbenchService;
use ficant_domain::governance::PlatformRole;
use ficant_domain::portfolio::{BenchmarkRef, PortfolioMetricConventionRef};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use prost_types::Timestamp;
use tonic::{Code, Request};

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

struct Backend {
    defaults: AtomicUsize,
    pages: AtomicUsize,
}

#[async_trait]
impl PortfolioWorkbenchBackend for Backend {
    async fn get_default_context(
        &self,
        principal: &AuthorizedPrincipal,
        request_owner: OwnerRef,
        subject_ref: VersionRef,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<PortfolioDefaultContextResult> {
        self.defaults.fetch_add(1, Ordering::SeqCst);
        assert_eq!(principal.active_role(), PlatformRole::Researcher);
        assert_eq!(request_owner, owner());
        assert_eq!(subject_ref, subject());
        assert_eq!(knowledge_at, time(11));
        Ok(PortfolioDefaultContextResult::Context(Box::new(
            normalized(),
        )))
    }

    async fn get_page(
        &self,
        principal: &AuthorizedPrincipal,
        page_id: PortfolioWorkbenchPageId,
        input: PortfolioContextInput,
        selection: Option<PortfolioPageSelection>,
    ) -> ApplicationResult<PortfolioPageEnvelope> {
        self.pages.fetch_add(1, Ordering::SeqCst);
        assert_eq!(principal.active_role(), PlatformRole::Researcher);
        assert_eq!(page_id, PortfolioWorkbenchPageId::P03);
        assert!(matches!(input.currency, PortfolioCurrencyMode::Cny));
        assert!(selection.is_none());
        Err(ApplicationError::new(
            ApplicationErrorCategory::NotFound,
            false,
        ))
    }
}

#[tokio::test]
async fn default_context_maps_the_application_normalization() {
    fn assert_service<T: PortfolioWorkbenchService>() {}
    assert_service::<PortfolioWorkbenchGrpcService>();

    let backend = Arc::new(Backend {
        defaults: AtomicUsize::new(0),
        pages: AtomicUsize::new(0),
    });
    let response = service(
        backend.clone(),
        PlatformRole::Researcher,
        vec!["portfolio:read"],
    )
    .get_default_context(Request::new(default_request()))
    .await
    .unwrap()
    .into_inner();
    let Some(pb::get_default_context_response::Result::Context(context)) = response.result else {
        panic!("authorized default context must be returned")
    };
    assert_eq!(context.currency, pb::PortfolioCurrencyMode::Cny as i32);
    assert_eq!(backend.defaults.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn page_request_is_typed_before_the_backend_and_never_guesses_owner() {
    let backend = Arc::new(Backend {
        defaults: AtomicUsize::new(0),
        pages: AtomicUsize::new(0),
    });
    let error = service(
        backend.clone(),
        PlatformRole::Researcher,
        vec!["portfolio:read"],
    )
    .get_page(Request::new(page_request()))
    .await
    .unwrap_err();
    assert_eq!(error.code(), Code::NotFound);
    assert_eq!(backend.pages.load(Ordering::SeqCst), 1);

    let mut malformed = page_request();
    malformed.context.as_mut().unwrap().currency = pb::PortfolioCurrencyMode::Unspecified as i32;
    let error = service(
        backend.clone(),
        PlatformRole::Researcher,
        vec!["portfolio:read"],
    )
    .get_page(Request::new(malformed))
    .await
    .unwrap_err();
    assert_eq!(error.code(), Code::InvalidArgument);
    assert_eq!(backend.pages.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn role_scope_and_owner_drift_fail_before_backend() {
    for (role, scopes, request) in [
        (
            PlatformRole::PlatformAdmin,
            vec!["portfolio:read"],
            default_request(),
        ),
        (PlatformRole::Researcher, Vec::new(), default_request()),
        (
            PlatformRole::Researcher,
            vec!["portfolio:read"],
            pb::GetDefaultContextRequest {
                owner: Some(proto_owner(&OwnerRef::new(
                    owner().tenant_id().clone(),
                    id("01ARZ3NDEKTSV4RRFFQ69G5F20"),
                ))),
                ..default_request()
            },
        ),
    ] {
        let backend = Arc::new(Backend {
            defaults: AtomicUsize::new(0),
            pages: AtomicUsize::new(0),
        });
        let error = service(backend.clone(), role, scopes)
            .get_default_context(Request::new(request))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::PermissionDenied);
        assert_eq!(backend.defaults.load(Ordering::SeqCst), 0);
    }
}

fn service(
    backend: Arc<Backend>,
    role: PlatformRole,
    scopes: Vec<&str>,
) -> PortfolioWorkbenchGrpcService {
    let identity = TrustedIdentity::implicit(
        "portfolio-workbench-test",
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
    PortfolioWorkbenchGrpcService::new(platform, backend, KEY).unwrap()
}

fn default_request() -> pb::GetDefaultContextRequest {
    pb::GetDefaultContextRequest {
        owner: Some(proto_owner(&owner())),
        subject_ref: Some(proto_version(&subject())),
        knowledge_at: Some(proto_time(11)),
    }
}

fn page_request() -> pb::GetPortfolioPageRequest {
    pb::GetPortfolioPageRequest {
        page_id: pb::PortfolioWorkbenchPageId::P03 as i32,
        context: Some(pb::PortfolioContextInput {
            scope: Some(pb::PortfolioScopeSelector {
                scope: Some(pb::portfolio_scope_selector::Scope::PortfolioId(proto_id(
                    &portfolio_id(),
                ))),
            }),
            valuation_at: Some(proto_time(10)),
            knowledge_at: Some(proto_time(11)),
            currency: pb::PortfolioCurrencyMode::Cny as i32,
            look_through: pb::PortfolioLookThroughMode::None as i32,
            benchmark_id: Some(proto_id(&benchmark_id())),
            period: pb::PortfolioPeriodPreset::OneDay as i32,
        }),
        selection: None,
    }
}

fn normalized() -> NormalizedPortfolioContext {
    let exact = exact_ref(portfolio_id(), b"portfolio");
    NormalizedPortfolioContext {
        owner: owner(),
        subject_ref: subject(),
        scope: ExactPortfolioScope::new(
            ExactPortfolioScopeKind::Portfolio(exact.clone()),
            vec![exact],
        ),
        valuation_at: time(10),
        knowledge_at: time(11),
        currency: PortfolioCurrencyMode::Cny,
        currency_unit: UnitRef::new(id("01ARZ3NDEKTSV4RRFFQ69G5F11"), version()),
        look_through: PortfolioLookThroughMode::None,
        benchmark: BenchmarkRef::new(
            VersionRef::new(benchmark_id(), version()),
            ContentHash::digest(b"benchmark"),
        ),
        period: PortfolioPeriodPreset::OneDay,
        period_from: time(9),
        period_to: time(10),
        metric_convention: PortfolioMetricConventionRef::new(
            VersionRef::new(id("01ARZ3NDEKTSV4RRFFQ69G5F13"), version()),
            ContentHash::digest(b"convention"),
        ),
    }
}

fn exact_ref(id: Ulid, bytes: &[u8]) -> LineageRef {
    LineageRef::new(id, Some(version()), Some(ContentHash::digest(bytes))).unwrap()
}

fn proto_owner(value: &OwnerRef) -> core::OwnerRef {
    core::OwnerRef {
        tenant_id: Some(proto_id(value.tenant_id())),
        owner_id: Some(proto_id(value.owner_id())),
    }
}

fn proto_version(value: &VersionRef) -> core::VersionRef {
    core::VersionRef {
        id: Some(proto_id(value.id())),
        version: value.version().get(),
    }
}

fn proto_id(value: &Ulid) -> core::Ulid {
    core::Ulid {
        value: value.as_str().to_owned(),
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

fn subject() -> VersionRef {
    VersionRef::new(id("01ARZ3NDEKTSV4RRFFQ69G5F03"), version())
}

fn portfolio_id() -> Ulid {
    id("01ARZ3NDEKTSV4RRFFQ69G5F10")
}

fn benchmark_id() -> Ulid {
    id("01ARZ3NDEKTSV4RRFFQ69G5F12")
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(value: &str) -> Ulid {
    Ulid::new(value).unwrap()
}
