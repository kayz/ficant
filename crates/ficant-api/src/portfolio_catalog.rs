use std::sync::Arc;

use async_trait::async_trait;
use ficant_application::ports::{
    AuthorizedPrincipal, PORTFOLIO_READ_SCOPE, PortfolioCatalogFilter, PortfolioCatalogPage,
    PortfolioCatalogTemporalScope,
};
use ficant_application::use_cases::portfolio_workbench::NonFormalReadEvidence;
use ficant_application::use_cases::portfolio_workbench::OwnedPortfolioCatalogBackend as ApplicationPortfolioCatalogBackend;
use ficant_application::{ApplicationError, ApplicationErrorCategory, ListPortfolioCatalogCommand};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::portfolio::v1 as pb;
use ficant_contracts::ficant::portfolio::v1::portfolio_catalog_service_server::PortfolioCatalogService;
use ficant_domain::ContentAddressed;
use ficant_domain::governance::PlatformRole;
use ficant_domain::portfolio::{
    Book, Portfolio, PortfolioGroup, PortfolioSnapshotBinding, PortfolioStatus,
};
use ficant_domain::primitives::{LineageRef, Version};
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::formal_evidence::proto_formal_input;
use crate::grpc_web::request_credential;
use crate::market_definition::{
    hash, market_time, owner, parse_market_time, parse_owner, parse_version_ref, ulid, version_ref,
};
use crate::registry::PlatformPort;

const DEFAULT_PAGE_SIZE: u32 = 100;

/// Exact application result required by the Catalog transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioCatalogBackendResult {
    page: PortfolioCatalogPage,
    read_evidence: NonFormalReadEvidence,
}

impl PortfolioCatalogBackendResult {
    #[must_use]
    pub const fn new(page: PortfolioCatalogPage, read_evidence: NonFormalReadEvidence) -> Self {
        Self {
            page,
            read_evidence,
        }
    }

    #[must_use]
    pub const fn page(&self) -> &PortfolioCatalogPage {
        &self.page
    }

    #[must_use]
    pub const fn read_evidence(&self) -> &NonFormalReadEvidence {
        &self.read_evidence
    }
}

/// Owned typed application seam injected into the tonic Catalog adapter.
#[async_trait]
pub trait PortfolioCatalogBackend: Send + Sync {
    async fn list(
        &self,
        principal: &AuthorizedPrincipal,
        command: ListPortfolioCatalogCommand,
    ) -> Result<PortfolioCatalogBackendResult, ApplicationError>;
}

#[async_trait]
impl PortfolioCatalogBackend for ApplicationPortfolioCatalogBackend {
    async fn list(
        &self,
        principal: &AuthorizedPrincipal,
        command: ListPortfolioCatalogCommand,
    ) -> Result<PortfolioCatalogBackendResult, ApplicationError> {
        let (page, evidence) = self.list(principal, command).await?.into_parts();
        Ok(PortfolioCatalogBackendResult::new(page, evidence))
    }
}

/// Authenticated, read-only Portfolio directory transport.
#[derive(Clone)]
pub struct PortfolioCatalogGrpcService {
    identity: Arc<dyn PlatformPort>,
    backend: Arc<dyn PortfolioCatalogBackend>,
    errors: CoreBusinessErrorMapper,
}

impl PortfolioCatalogGrpcService {
    /// Composes the transport over a typed application backend.
    ///
    /// # Errors
    ///
    /// Returns a configuration failure when the trace-signing key is too short.
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        backend: Arc<dyn PortfolioCatalogBackend>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            backend,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn principal(
        &self,
        request: &Request<impl Sized>,
    ) -> Result<AuthorizedPrincipal, ApplicationError> {
        let session = self
            .identity
            .current_session(&request_credential(request.metadata()))
            .map_err(|_| unauthenticated())?;
        let principal = session.authorized_principal()?;
        principal.require_role(PlatformRole::Researcher)?;
        principal
            .has_scope(PORTFOLIO_READ_SCOPE)
            .then_some(principal)
            .ok_or_else(forbidden)
    }

    fn error(&self, operation: &str, error: &ApplicationError) -> core::ErrorDetail {
        self.errors
            .map(operation, "portfolio-catalog-application", error)
    }
}

#[tonic::async_trait]
impl PortfolioCatalogService for PortfolioCatalogGrpcService {
    async fn list_books_and_portfolios(
        &self,
        request: Request<pb::ListBooksAndPortfoliosRequest>,
    ) -> Result<Response<pb::ListBooksAndPortfoliosResponse>, Status> {
        const OPERATION: &str = "portfolio.catalog.list";
        let result = match self.principal(&request) {
            Err(error) => Err(error),
            Ok(principal) => match parse_catalog_command(&principal, request.get_ref()) {
                Err(error) => Err(error),
                Ok(command) => self.backend.list(&principal, command).await,
            },
        };
        Ok(Response::new(pb::ListBooksAndPortfoliosResponse {
            result: Some(match result {
                Ok(result) => pb::list_books_and_portfolios_response::Result::Catalog(
                    catalog_page(result.page(), Some(result.read_evidence())),
                ),
                Err(error) => pb::list_books_and_portfolios_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }
}

fn parse_catalog_command(
    principal: &AuthorizedPrincipal,
    request: &pb::ListBooksAndPortfoliosRequest,
) -> Result<ListPortfolioCatalogCommand, ApplicationError> {
    let owner = parse_owner(request.owner.as_ref())?;
    principal.access_scope().authorize(&owner)?;
    let temporal = PortfolioCatalogTemporalScope::new(
        owner,
        parse_version_ref(request.subject_ref.as_ref())?,
        parse_market_time(request.as_of.as_ref())?,
        parse_market_time(request.knowledge_at.as_ref())?,
    )?;
    let statuses = request
        .statuses
        .iter()
        .map(|value| parse_status(*value))
        .collect::<Result<Vec<_>, _>>()?;
    let search = (!request.search.is_empty()).then(|| request.search.clone());
    let filter = PortfolioCatalogFilter::new(temporal, statuses, search)?;
    let page = request.page.as_ref();
    let limit = page.map_or(DEFAULT_PAGE_SIZE, |page| {
        if page.page_size == 0 {
            DEFAULT_PAGE_SIZE
        } else {
            page.page_size
        }
    });
    let cursor = page
        .filter(|page| !page.cursor.is_empty())
        .map(|page| page.cursor.clone());
    ListPortfolioCatalogCommand::new(filter, cursor, limit)
}

fn parse_status(value: i32) -> Result<PortfolioStatus, ApplicationError> {
    match pb::PortfolioStatus::try_from(value).map_err(|_| invalid())? {
        pb::PortfolioStatus::Active => Ok(PortfolioStatus::Active),
        pb::PortfolioStatus::Suspended => Ok(PortfolioStatus::Suspended),
        pb::PortfolioStatus::Closed => Ok(PortfolioStatus::Closed),
        pb::PortfolioStatus::Unspecified => Err(invalid()),
    }
}

pub(crate) fn catalog_page(
    value: &PortfolioCatalogPage,
    evidence: Option<&NonFormalReadEvidence>,
) -> pb::PortfolioCatalogPage {
    pb::PortfolioCatalogPage {
        books: value
            .books()
            .iter()
            .map(|record| book(record.value()))
            .collect(),
        groups: value
            .groups()
            .iter()
            .map(|record| group(record.value()))
            .collect(),
        portfolios: value
            .portfolios()
            .iter()
            .map(|entry| portfolio(entry.record().value()))
            .collect(),
        page: Some(core::PageResponse {
            next_cursor: value.next_cursor().unwrap_or_default().to_owned(),
        }),
        read_evidence: evidence.map(non_formal_evidence),
    }
}

pub(crate) fn non_formal_evidence(value: &NonFormalReadEvidence) -> pb::NonFormalReadEvidence {
    pb::NonFormalReadEvidence {
        schema_id: value.schema_id().to_owned(),
        consumed_inputs: value
            .consumed_inputs()
            .iter()
            .map(proto_formal_input)
            .collect(),
        request_fingerprint: Some(hash(value.request_fingerprint())),
    }
}

pub(crate) fn lineage(value: &LineageRef) -> core::LineageRef {
    core::LineageRef {
        object_id: Some(ulid(value.object_id())),
        version: value.version().map_or(0, Version::get),
        content_hash: value.content_hash().map(hash),
    }
}

pub(crate) fn snapshot_binding(value: &PortfolioSnapshotBinding) -> pb::PortfolioSnapshotBinding {
    pb::PortfolioSnapshotBinding {
        snapshot_id: Some(ulid(value.snapshot_id())),
        content_hash: Some(hash(value.content_hash())),
        observed_at: Some(market_time(value.observed_at())),
        visible_at: Some(market_time(value.visible_at())),
    }
}

pub(crate) fn benchmark_ref(value: &ficant_domain::portfolio::BenchmarkRef) -> pb::BenchmarkRef {
    pb::BenchmarkRef {
        benchmark: Some(version_ref(value.reference())),
        content_hash: Some(hash(value.content_hash())),
    }
}

pub(crate) fn metric_convention_ref(
    value: &ficant_domain::portfolio::PortfolioMetricConventionRef,
) -> pb::PortfolioMetricConventionRef {
    pb::PortfolioMetricConventionRef {
        convention: Some(version_ref(value.reference())),
        content_hash: Some(hash(value.content_hash())),
    }
}

fn book(value: &Book) -> pb::Book {
    pb::Book {
        book: Some(version_ref(value.reference())),
        owner: Some(owner(value.owner())),
        subject_ref: Some(version_ref(value.subject_ref())),
        code: value.code().to_owned(),
        display_name: value.display_name().to_owned(),
        status: proto_status(value.status()) as i32,
        effective_from: Some(market_time(value.effective_from())),
        effective_to: Some(market_time(value.effective_to())),
        content_hash: Some(hash(value.content_hash())),
    }
}

fn group(value: &PortfolioGroup) -> pb::PortfolioGroup {
    pb::PortfolioGroup {
        group: Some(version_ref(value.reference())),
        owner: Some(owner(value.owner())),
        subject_ref: Some(version_ref(value.subject_ref())),
        book: Some(lineage(value.book())),
        parent_group: value.parent_group().map(lineage),
        code: value.code().to_owned(),
        display_name: value.display_name().to_owned(),
        status: proto_status(value.status()) as i32,
        effective_from: Some(market_time(value.effective_from())),
        effective_to: Some(market_time(value.effective_to())),
        content_hash: Some(hash(value.content_hash())),
    }
}

fn portfolio(value: &Portfolio) -> pb::Portfolio {
    pb::Portfolio {
        portfolio: Some(version_ref(value.reference())),
        owner: Some(owner(value.owner())),
        subject_ref: Some(version_ref(value.subject_ref())),
        book: Some(lineage(value.book())),
        group: Some(lineage(value.group())),
        code: value.code().to_owned(),
        display_name: value.display_name().to_owned(),
        status: proto_status(value.status()) as i32,
        position_snapshot: Some(snapshot_binding(value.position_snapshot())),
        benchmark: Some(benchmark_ref(value.benchmark())),
        metric_convention: Some(metric_convention_ref(value.metric_convention())),
        effective_from: Some(market_time(value.effective_from())),
        effective_to: Some(market_time(value.effective_to())),
        content_hash: Some(hash(value.content_hash())),
    }
}

const fn proto_status(value: PortfolioStatus) -> pb::PortfolioStatus {
    match value {
        PortfolioStatus::Active => pb::PortfolioStatus::Active,
        PortfolioStatus::Suspended => pb::PortfolioStatus::Suspended,
        PortfolioStatus::Closed => pb::PortfolioStatus::Closed,
    }
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

fn unauthenticated() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Unauthenticated, false)
}
