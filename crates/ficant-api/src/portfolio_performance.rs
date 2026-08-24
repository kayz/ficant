use std::sync::Arc;

use async_trait::async_trait;
use ficant_application::ports::{
    AeadCursorCodec, AuthorizedPrincipal, ExactPortfolioScope, ExactPortfolioScopeKind,
    NormalizedPortfolioContext, PORTFOLIO_READ_SCOPE, PortfolioCatalogRepository,
    PortfolioContextInput, PortfolioScopeSelector, SubjectRepository,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, ListPortfolioCatalog,
    OwnedPortfolioPerformanceBackend, PortfolioPerformanceDraft,
    PortfolioPerformanceEvidenceBinding, PortfolioPerformanceEvidenceKind,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::portfolio::v1 as pb;
use ficant_contracts::ficant::portfolio::v1::portfolio_performance_service_server::PortfolioPerformanceService;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{FixedDecimal, UnitRef};
use ficant_runtime::{FormalInputBinding, FormalInputKind};
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::formal_evidence::{
    FormalInputTimes, FormalOutputPublisher, exact_subject_binding, implementation_binding,
    message_parameters_hash, object_binding,
};
use crate::grpc_web::request_credential;
use crate::market_definition::{hash, market_time, unit_ref};
use crate::portfolio_aggregation::{
    RequestedNormalizedPortfolioContext, exact_scope, parse_requested_context,
};
use crate::portfolio_catalog::benchmark_ref;
use crate::registry::PlatformPort;

const PERFORMANCE_SCHEMA: &str = "ficant.portfolio-performance-series.v1";

#[derive(Clone, Debug, PartialEq)]
pub struct PortfolioPerformanceBackendResult {
    series: pb::PortfolioPerformanceSeries,
}

impl PortfolioPerformanceBackendResult {
    #[must_use]
    pub const fn new(series: pb::PortfolioPerformanceSeries) -> Self {
        Self { series }
    }

    #[must_use]
    pub const fn series(&self) -> &pb::PortfolioPerformanceSeries {
        &self.series
    }
}

#[async_trait]
pub trait PortfolioPerformanceBackend: Send + Sync {
    async fn get_performance(
        &self,
        principal: &AuthorizedPrincipal,
        context: RequestedNormalizedPortfolioContext,
    ) -> Result<PortfolioPerformanceBackendResult, ApplicationError>;
}

#[derive(Clone)]
pub struct OwnedPortfolioPerformanceApplicationBackend {
    catalog: Arc<dyn PortfolioCatalogRepository>,
    cursor: Arc<AeadCursorCodec>,
    application: Arc<OwnedPortfolioPerformanceBackend>,
    subjects: Arc<dyn SubjectRepository>,
    publisher: FormalOutputPublisher,
}

impl OwnedPortfolioPerformanceApplicationBackend {
    #[must_use]
    pub fn new(
        catalog: Arc<dyn PortfolioCatalogRepository>,
        cursor: Arc<AeadCursorCodec>,
        application: Arc<OwnedPortfolioPerformanceBackend>,
        subjects: Arc<dyn SubjectRepository>,
        publisher: FormalOutputPublisher,
    ) -> Self {
        Self {
            catalog,
            cursor,
            application,
            subjects,
            publisher,
        }
    }
}

#[async_trait]
impl PortfolioPerformanceBackend for OwnedPortfolioPerformanceApplicationBackend {
    async fn get_performance(
        &self,
        principal: &AuthorizedPrincipal,
        requested: RequestedNormalizedPortfolioContext,
    ) -> Result<PortfolioPerformanceBackendResult, ApplicationError> {
        let selector = selector_from_exact_scope(&requested.scope);
        let catalog = ListPortfolioCatalog::new(self.catalog.as_ref(), self.cursor.as_ref());
        let authority = catalog
            .resolve_scope_authority(
                principal,
                &selector,
                &requested.valuation_at,
                &requested.knowledge_at,
            )
            .await?;
        let resolution = catalog
            .normalize_context_with_evidence(
                principal,
                authority.owner().clone(),
                authority.subject_ref().clone(),
                PortfolioContextInput {
                    scope: selector,
                    valuation_at: requested.valuation_at.clone(),
                    knowledge_at: requested.knowledge_at.clone(),
                    currency: requested.currency,
                    look_through: requested.look_through,
                    benchmark_id: requested.benchmark.reference().id().clone(),
                    period: requested.period,
                },
            )
            .await?;
        if !requested_context_matches(&requested, resolution.context()) {
            return Err(lineage());
        }
        let draft = self
            .application
            .execute_resolution(principal, &resolution)
            .await?;
        let mut message = series(&draft);
        let subject = exact_subject_binding(
            self.subjects.as_ref(),
            principal.access_scope(),
            draft.owner(),
            draft.subject_ref(),
        )
        .await?;
        let inputs = draft
            .evidence()
            .iter()
            .map(formal_input)
            .collect::<Result<Vec<_>, _>>()?;
        let parameters = message_parameters_hash(
            "ficant/portfolio-performance-parameters/v1",
            &pb::NormalizedPortfolioContext {
                scope: Some(exact_scope(draft.scope())),
                valuation_at: Some(market_time(draft.period_to())),
                knowledge_at: Some(market_time(&requested.knowledge_at)),
                currency: proto_currency(requested.currency) as i32,
                currency_unit: Some(unit_ref(draft.currency_unit())),
                look_through: proto_look_through(requested.look_through) as i32,
                benchmark: Some(benchmark_ref(draft.benchmark())),
                period: proto_period(requested.period) as i32,
                period_from: Some(market_time(draft.period_from())),
                period_to: Some(market_time(draft.period_to())),
                metric_convention: Some(crate::portfolio_catalog::metric_convention_ref(
                    &requested.metric_convention,
                )),
            },
        );
        let implementation = implementation_binding(
            "portfolio-performance",
            "ficant/portfolio-performance-implementation/v1",
            &[
                b"end-of-day-flow",
                b"daily-time-weighted-return",
                b"geometric-compounding",
                b"fixed-decimal-12-ties-to-even",
            ],
        )?;
        let evidence = self
            .publisher
            .publish_message(
                principal.access_scope(),
                draft.owner(),
                PERFORMANCE_SCHEMA,
                subject,
                inputs,
                vec![implementation],
                parameters,
                None,
                &message,
            )
            .await?;
        message.formal_evidence = Some(evidence);
        Ok(PortfolioPerformanceBackendResult::new(message))
    }
}

#[derive(Clone)]
pub struct PortfolioPerformanceGrpcService {
    identity: Arc<dyn PlatformPort>,
    backend: Arc<dyn PortfolioPerformanceBackend>,
    errors: CoreBusinessErrorMapper,
}

impl PortfolioPerformanceGrpcService {
    /// Builds the authenticated transport.
    ///
    /// # Errors
    ///
    /// Returns configuration failure when the trace key is too short.
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        backend: Arc<dyn PortfolioPerformanceBackend>,
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

    fn error(&self, error: &ApplicationError) -> core::ErrorDetail {
        self.errors.map(
            "portfolio.performance.get-performance",
            "portfolio-performance-application",
            error,
        )
    }
}

#[tonic::async_trait]
impl PortfolioPerformanceService for PortfolioPerformanceGrpcService {
    async fn get_portfolio_performance(
        &self,
        request: Request<pb::GetPortfolioPerformanceRequest>,
    ) -> Result<Response<pb::GetPortfolioPerformanceResponse>, Status> {
        let result = match self.principal(&request) {
            Err(error) => Err(error),
            Ok(principal) => match parse_requested_context(request.get_ref().context.as_ref()) {
                Err(error) => Err(error),
                Ok(context) => self.backend.get_performance(&principal, context).await,
            },
        };
        Ok(Response::new(pb::GetPortfolioPerformanceResponse {
            result: Some(match result {
                Ok(value) => {
                    pb::get_portfolio_performance_response::Result::Series(value.series().clone())
                }
                Err(error) => {
                    pb::get_portfolio_performance_response::Result::Error(self.error(&error))
                }
            }),
        }))
    }
}

fn selector_from_exact_scope(scope: &ExactPortfolioScope) -> PortfolioScopeSelector {
    match scope.selected() {
        ExactPortfolioScopeKind::Book(value) => {
            PortfolioScopeSelector::Book(value.object_id().clone())
        }
        ExactPortfolioScopeKind::Group(value) => {
            PortfolioScopeSelector::Group(value.object_id().clone())
        }
        ExactPortfolioScopeKind::Portfolio(value) => {
            PortfolioScopeSelector::Portfolio(value.object_id().clone())
        }
    }
}

fn requested_context_matches(
    requested: &RequestedNormalizedPortfolioContext,
    normalized: &NormalizedPortfolioContext,
) -> bool {
    requested.scope == normalized.scope
        && requested.valuation_at == normalized.valuation_at
        && requested.knowledge_at == normalized.knowledge_at
        && requested.currency == normalized.currency
        && requested.currency_unit == normalized.currency_unit
        && requested.look_through == normalized.look_through
        && requested.benchmark == normalized.benchmark
        && requested.period == normalized.period
        && requested.period_from == normalized.period_from
        && requested.period_to == normalized.period_to
        && requested.metric_convention == normalized.metric_convention
}

const fn proto_currency(
    value: ficant_application::ports::PortfolioCurrencyMode,
) -> pb::PortfolioCurrencyMode {
    match value {
        ficant_application::ports::PortfolioCurrencyMode::Original => {
            pb::PortfolioCurrencyMode::Original
        }
        ficant_application::ports::PortfolioCurrencyMode::Cny => pb::PortfolioCurrencyMode::Cny,
    }
}

const fn proto_look_through(
    value: ficant_application::ports::PortfolioLookThroughMode,
) -> pb::PortfolioLookThroughMode {
    match value {
        ficant_application::ports::PortfolioLookThroughMode::None => {
            pb::PortfolioLookThroughMode::None
        }
        ficant_application::ports::PortfolioLookThroughMode::Consolidated => {
            pb::PortfolioLookThroughMode::Consolidated
        }
        ficant_application::ports::PortfolioLookThroughMode::Separate => {
            pb::PortfolioLookThroughMode::Separate
        }
    }
}

const fn proto_period(
    value: ficant_application::ports::PortfolioPeriodPreset,
) -> pb::PortfolioPeriodPreset {
    match value {
        ficant_application::ports::PortfolioPeriodPreset::OneDay => {
            pb::PortfolioPeriodPreset::OneDay
        }
        ficant_application::ports::PortfolioPeriodPreset::SevenDays => {
            pb::PortfolioPeriodPreset::SevenDays
        }
        ficant_application::ports::PortfolioPeriodPreset::ThirtyDays => {
            pb::PortfolioPeriodPreset::ThirtyDays
        }
        ficant_application::ports::PortfolioPeriodPreset::YearToDate => {
            pb::PortfolioPeriodPreset::YearToDate
        }
        ficant_application::ports::PortfolioPeriodPreset::OneYear => {
            pb::PortfolioPeriodPreset::OneYear
        }
    }
}

fn formal_input(
    value: &PortfolioPerformanceEvidenceBinding,
) -> Result<FormalInputBinding, ApplicationError> {
    let kind = match value.kind() {
        PortfolioPerformanceEvidenceKind::Book => FormalInputKind::Book,
        PortfolioPerformanceEvidenceKind::PortfolioGroup => FormalInputKind::PortfolioGroup,
        PortfolioPerformanceEvidenceKind::Portfolio => FormalInputKind::Portfolio,
        PortfolioPerformanceEvidenceKind::Benchmark => FormalInputKind::Benchmark,
        PortfolioPerformanceEvidenceKind::PortfolioMetricConvention => {
            FormalInputKind::PortfolioMetricConvention
        }
        PortfolioPerformanceEvidenceKind::PortfolioPerformanceConvention => {
            FormalInputKind::PortfolioPerformanceConvention
        }
        PortfolioPerformanceEvidenceKind::Calendar => FormalInputKind::Calendar,
        PortfolioPerformanceEvidenceKind::Unit => FormalInputKind::Unit,
        PortfolioPerformanceEvidenceKind::PortfolioValuationSnapshot => {
            FormalInputKind::PortfolioValuationSnapshot
        }
        PortfolioPerformanceEvidenceKind::PositionSnapshot => FormalInputKind::PositionSnapshot,
        PortfolioPerformanceEvidenceKind::BenchmarkLevelSnapshot => {
            FormalInputKind::BenchmarkLevelSnapshot
        }
    };
    object_binding(
        value.role(),
        kind,
        value.owner(),
        value.reference().object_id(),
        value.reference().version(),
        value
            .reference()
            .content_hash()
            .cloned()
            .ok_or_else(lineage)?,
        FormalInputTimes {
            observed_at: value.observed_at().cloned(),
            visible_at: value.visible_at().cloned(),
            effective_from: value.effective_from().cloned(),
            effective_to: value.effective_to().cloned(),
        },
    )
}

pub(crate) fn series(value: &PortfolioPerformanceDraft) -> pb::PortfolioPerformanceSeries {
    pb::PortfolioPerformanceSeries {
        scope: Some(exact_scope(value.scope())),
        performance_convention: Some(pb::PortfolioPerformanceConventionRef {
            convention: Some(crate::market_definition::version_ref(
                value.performance_convention().reference(),
            )),
            content_hash: Some(hash(value.performance_convention().content_hash())),
        }),
        benchmark: Some(benchmark_ref(value.benchmark())),
        currency_unit: Some(unit_ref(value.currency_unit())),
        period_from: Some(market_time(value.period_from())),
        period_to: Some(market_time(value.period_to())),
        points: value
            .points()
            .iter()
            .map(|point| pb::PortfolioDailyPerformancePoint {
                valuation_at: Some(market_time(point.valuation_at())),
                opening_nav: Some(decimal(point.opening_nav(), value.currency_unit())),
                ending_nav: Some(decimal(point.ending_nav(), value.currency_unit())),
                net_external_flow: Some(decimal(point.net_external_flow(), value.currency_unit())),
                economic_pnl: Some(decimal(point.economic_pnl(), value.currency_unit())),
                daily_return: Some(decimal(point.daily_return(), value.return_unit())),
                benchmark_return: Some(decimal(point.benchmark_return(), value.return_unit())),
                active_return: Some(decimal(point.active_return(), value.return_unit())),
                cumulative_return: Some(decimal(point.cumulative_return(), value.return_unit())),
                benchmark_cumulative_return: Some(decimal(
                    point.benchmark_cumulative_return(),
                    value.return_unit(),
                )),
                active_cumulative_return: Some(decimal(
                    point.active_cumulative_return(),
                    value.return_unit(),
                )),
            })
            .collect(),
        coverage: Some(pb::PortfolioPerformanceCoverage {
            expected_session_count: value.coverage().expected_session_count(),
            observed_session_count: value.coverage().observed_session_count(),
            expected_portfolio_observation_count: value
                .coverage()
                .expected_portfolio_observation_count(),
            observed_portfolio_observation_count: value
                .coverage()
                .observed_portfolio_observation_count(),
            expected_benchmark_observation_count: value
                .coverage()
                .expected_benchmark_observation_count(),
            observed_benchmark_observation_count: value
                .coverage()
                .observed_benchmark_observation_count(),
            missing_sessions: Vec::new(),
        }),
        request_fingerprint: Some(hash(value.request_fingerprint())),
        formal_evidence: None,
    }
}

fn decimal(value: FixedDecimal, unit: &UnitRef) -> core::DecimalValue {
    core::DecimalValue {
        coefficient: value.scaled().to_string(),
        scale: ficant_domain::primitives::DECIMAL_SCALE,
        unit: Some(unit_ref(unit)),
    }
}

fn unauthenticated() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Unauthenticated, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

fn lineage() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}
