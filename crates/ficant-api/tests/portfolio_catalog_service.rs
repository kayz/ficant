use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use ficant_api::{
    PlatformApplication, PlatformPort, PortfolioCatalogBackend, PortfolioCatalogBackendResult,
    PortfolioCatalogGrpcService, SessionPolicy, SystemClock, TrustedIdentity,
};
use ficant_application::ListPortfolioCatalogCommand;
use ficant_application::ports::{ApplicationResult, AuthorizedPrincipal, PortfolioCatalogPage};
use ficant_application::use_cases::portfolio_workbench::NonFormalReadEvidence;
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::portfolio::v1 as pb;
use ficant_contracts::ficant::portfolio::v1::portfolio_catalog_service_server::PortfolioCatalogService;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version};
use ficant_runtime::{
    FormalInputBinding, FormalInputBindingInput, FormalInputKind, FormalInputReference,
};
use prost_types::Timestamp;
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

struct Backend {
    calls: AtomicUsize,
}

#[async_trait]
impl PortfolioCatalogBackend for Backend {
    async fn list(
        &self,
        principal: &AuthorizedPrincipal,
        command: ListPortfolioCatalogCommand,
    ) -> ApplicationResult<PortfolioCatalogBackendResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(command.filter().temporal().owner(), &owner());
        assert_eq!(principal.active_role(), PlatformRole::Researcher);
        let fingerprint = command.filter().fingerprint().clone();
        let page = PortfolioCatalogPage::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            fingerprint.clone(),
        );
        let binding = FormalInputBinding::new(FormalInputBindingInput {
            role: "subject".to_owned(),
            kind: FormalInputKind::Subject,
            owner: owner(),
            reference: FormalInputReference::Object(
                LineageRef::new(
                    subject_id(),
                    Some(Version::new(1).unwrap()),
                    Some(ContentHash::digest(b"subject")),
                )
                .unwrap(),
            ),
            observed_at: None,
            visible_at: None,
            effective_from: None,
            effective_to: None,
        })
        .unwrap();
        let evidence = NonFormalReadEvidence::new(
            "ficant.portfolio-catalog-read.v1".to_owned(),
            vec![binding],
            fingerprint.content_hash().clone(),
        )?;
        Ok(PortfolioCatalogBackendResult::new(page, evidence))
    }
}

#[tokio::test]
async fn researcher_receives_non_formal_catalog_evidence() {
    fn assert_service<T: PortfolioCatalogService>() {}
    assert_service::<PortfolioCatalogGrpcService>();

    let backend = Arc::new(Backend {
        calls: AtomicUsize::new(0),
    });
    let service = service(
        backend.clone(),
        PlatformRole::Researcher,
        vec!["portfolio:read"],
        vec![owner().owner_id().clone()],
    );
    let response = service
        .list_books_and_portfolios(Request::new(valid_request()))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::list_books_and_portfolios_response::Result::Catalog(catalog)) = response.result
    else {
        panic!("authorized Catalog read must succeed")
    };
    assert!(catalog.books.is_empty());
    assert_eq!(
        catalog.read_evidence.unwrap().schema_id,
        "ficant.portfolio-catalog-read.v1"
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn role_scope_and_owner_drift_fail_before_backend() {
    for (role, scopes, allowed) in [
        (
            PlatformRole::PlatformAdmin,
            vec!["portfolio:read"],
            vec![owner().owner_id().clone()],
        ),
        (
            PlatformRole::Researcher,
            Vec::new(),
            vec![owner().owner_id().clone()],
        ),
        (
            PlatformRole::Researcher,
            vec!["portfolio:read"],
            vec![id("01ARZ3NDEKTSV4RRFFQ69G5F09")],
        ),
    ] {
        let backend = Arc::new(Backend {
            calls: AtomicUsize::new(0),
        });
        let service = service(backend.clone(), role, scopes, allowed);
        let response = service
            .list_books_and_portfolios(Request::new(valid_request()))
            .await
            .unwrap()
            .into_inner();
        let Some(pb::list_books_and_portfolios_response::Result::Error(error)) = response.result
        else {
            panic!("authorization drift must return a closed business error")
        };
        assert_eq!(error.code, core::ErrorCode::Forbidden as i32);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn bitemporal_and_enum_drift_fail_before_backend() {
    let backend = Arc::new(Backend {
        calls: AtomicUsize::new(0),
    });
    let service = service(
        backend.clone(),
        PlatformRole::Researcher,
        vec!["portfolio:read"],
        vec![owner().owner_id().clone()],
    );
    let mut temporal = valid_request();
    temporal.knowledge_at = Some(proto_time(9));
    temporal.as_of = Some(proto_time(10));
    let temporal = service
        .list_books_and_portfolios(Request::new(temporal))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        temporal.result,
        Some(pb::list_books_and_portfolios_response::Result::Error(_))
    ));

    let mut status = valid_request();
    status.statuses = vec![pb::PortfolioStatus::Unspecified as i32];
    let status = service
        .list_books_and_portfolios(Request::new(status))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        status.result,
        Some(pb::list_books_and_portfolios_response::Result::Error(_))
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}

fn service(
    backend: Arc<Backend>,
    role: PlatformRole,
    scopes: Vec<&str>,
    allowed_owner_ids: Vec<Ulid>,
) -> PortfolioCatalogGrpcService {
    let identity = TrustedIdentity::implicit(
        "portfolio-catalog-test",
        id("01ARZ3NDEKTSV4RRFFQ69G5F00"),
        owner().tenant_id().clone(),
        allowed_owner_ids,
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
    PortfolioCatalogGrpcService::new(platform, backend, KEY).unwrap()
}

fn valid_request() -> pb::ListBooksAndPortfoliosRequest {
    pb::ListBooksAndPortfoliosRequest {
        owner: Some(core::OwnerRef {
            tenant_id: Some(proto_id(owner().tenant_id())),
            owner_id: Some(proto_id(owner().owner_id())),
        }),
        subject_ref: Some(core::VersionRef {
            id: Some(proto_id(&subject_id())),
            version: 1,
        }),
        as_of: Some(proto_time(10)),
        knowledge_at: Some(proto_time(11)),
        statuses: vec![pb::PortfolioStatus::Active as i32],
        search: String::new(),
        page: Some(core::PageRequest {
            page_size: 50,
            cursor: String::new(),
        }),
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

fn subject_id() -> Ulid {
    id("01ARZ3NDEKTSV4RRFFQ69G5F03")
}

fn proto_id(value: &Ulid) -> core::Ulid {
    core::Ulid {
        value: value.as_str().to_owned(),
    }
}

fn id(value: &str) -> Ulid {
    Ulid::new(value).unwrap()
}
