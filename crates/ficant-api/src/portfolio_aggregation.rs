use std::sync::Arc;

use async_trait::async_trait;
use ficant_application::ports::{
    AeadCursorCodec, AuthorizedPrincipal, ExactPortfolioScope, ExactPortfolioScopeKind,
    NormalizedPortfolioContext, PORTFOLIO_READ_SCOPE, PortfolioCatalogRepository,
    PortfolioContextInput, PortfolioCurrencyMode, PortfolioLookThroughMode, PortfolioPeriodPreset,
    PortfolioScopeSelector,
};
use ficant_application::use_cases::portfolio_aggregation::{
    OwnedPortfolioAggregationBackend, PortfolioBasicMetrics, PortfolioCoverage,
    PortfolioCoverageReason, PortfolioKrdSummary, PortfolioOverview,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, ListPortfolioCatalog};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as market;
use ficant_contracts::ficant::portfolio::v1 as pb;
use ficant_contracts::ficant::portfolio::v1::portfolio_aggregation_service_server::PortfolioAggregationService;
use ficant_contracts::ficant::research::v1 as research;
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::PriceSourceType;
use ficant_domain::portfolio::{BenchmarkRef, PortfolioMetricConventionRef};
use ficant_domain::primitives::{LineageRef, MarketTime, UnitRef, Version};
use ficant_domain::research::{CoverageDeclaration, FactorDv01, PriceSourceSummary};
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::formal_evidence::proto_formal_evidence;
use crate::grpc_web::request_credential;
use crate::market_definition::{
    decimal, hash, market_time, parse_hash, parse_market_time, parse_ulid, parse_unit_ref,
    parse_version_ref, unit_ref,
};
use crate::portfolio_catalog::{benchmark_ref, lineage, metric_convention_ref, snapshot_binding};
use crate::registry::PlatformPort;

/// Public normalized context before the application resolves its unique owner and Subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestedNormalizedPortfolioContext {
    pub scope: ExactPortfolioScope,
    pub valuation_at: MarketTime,
    pub knowledge_at: MarketTime,
    pub currency: PortfolioCurrencyMode,
    pub currency_unit: UnitRef,
    pub look_through: PortfolioLookThroughMode,
    pub benchmark: BenchmarkRef,
    pub period: PortfolioPeriodPreset,
    pub period_from: MarketTime,
    pub period_to: MarketTime,
    pub metric_convention: PortfolioMetricConventionRef,
}

/// Formal application result plus the complete participation declaration required on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioAggregationBackendResult {
    overview: PortfolioOverview,
}

impl PortfolioAggregationBackendResult {
    #[must_use]
    pub const fn new(overview: PortfolioOverview) -> Self {
        Self { overview }
    }

    #[must_use]
    pub const fn overview(&self) -> &PortfolioOverview {
        &self.overview
    }
}

/// Owned typed application seam. Its implementation resolves owner/Subject without guessing.
#[async_trait]
pub trait PortfolioAggregationBackend: Send + Sync {
    async fn get_overview(
        &self,
        principal: &AuthorizedPrincipal,
        context: RequestedNormalizedPortfolioContext,
    ) -> Result<PortfolioAggregationBackendResult, ApplicationError>;
}

/// Production application adapter. It resolves the only authorized owner and Subject before the
/// exact aggregation preflight; no transport or session string supplies either authority.
#[derive(Clone)]
pub struct OwnedPortfolioAggregationApplicationBackend {
    catalog: Arc<dyn PortfolioCatalogRepository>,
    cursor: Arc<AeadCursorCodec>,
    aggregation: Arc<OwnedPortfolioAggregationBackend>,
}

impl OwnedPortfolioAggregationApplicationBackend {
    #[must_use]
    pub fn new(
        catalog: Arc<dyn PortfolioCatalogRepository>,
        cursor: Arc<AeadCursorCodec>,
        aggregation: Arc<OwnedPortfolioAggregationBackend>,
    ) -> Self {
        Self {
            catalog,
            cursor,
            aggregation,
        }
    }
}

#[async_trait]
impl PortfolioAggregationBackend for OwnedPortfolioAggregationApplicationBackend {
    async fn get_overview(
        &self,
        principal: &AuthorizedPrincipal,
        context: RequestedNormalizedPortfolioContext,
    ) -> Result<PortfolioAggregationBackendResult, ApplicationError> {
        let selector = selector_from_exact_scope(&context.scope);
        let catalog = ListPortfolioCatalog::new(self.catalog.as_ref(), self.cursor.as_ref());
        let authority = catalog
            .resolve_scope_authority(
                principal,
                &selector,
                &context.valuation_at,
                &context.knowledge_at,
            )
            .await?;
        let resolution = catalog
            .normalize_context_with_evidence(
                principal,
                authority.owner().clone(),
                authority.subject_ref().clone(),
                PortfolioContextInput {
                    scope: selector,
                    valuation_at: context.valuation_at.clone(),
                    knowledge_at: context.knowledge_at.clone(),
                    currency: context.currency,
                    look_through: context.look_through,
                    benchmark_id: context.benchmark.reference().id().clone(),
                    period: context.period,
                },
            )
            .await?;
        if !requested_context_matches(&context, resolution.context()) {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::LineageIncomplete,
                false,
            ));
        }
        self.aggregation
            .execute_resolution(principal, &resolution)
            .await
            .map(PortfolioAggregationBackendResult::new)
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

/// Authenticated transport for the formal Portfolio overview.
#[derive(Clone)]
pub struct PortfolioAggregationGrpcService {
    identity: Arc<dyn PlatformPort>,
    backend: Arc<dyn PortfolioAggregationBackend>,
    errors: CoreBusinessErrorMapper,
}

impl PortfolioAggregationGrpcService {
    /// Composes the transport over a typed application backend.
    ///
    /// # Errors
    ///
    /// Returns a configuration failure when the trace-signing key is too short.
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        backend: Arc<dyn PortfolioAggregationBackend>,
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
            .map(operation, "portfolio-aggregation-application", error)
    }
}

#[tonic::async_trait]
impl PortfolioAggregationService for PortfolioAggregationGrpcService {
    async fn get_portfolio_overview(
        &self,
        request: Request<pb::GetPortfolioOverviewRequest>,
    ) -> Result<Response<pb::GetPortfolioOverviewResponse>, Status> {
        const OPERATION: &str = "portfolio.aggregation.get-overview";
        let result = match self.principal(&request) {
            Err(error) => Err(error),
            Ok(principal) => match parse_requested_context(request.get_ref().context.as_ref()) {
                Err(error) => Err(error),
                Ok(context) => self.backend.get_overview(&principal, context).await,
            },
        };
        Ok(Response::new(pb::GetPortfolioOverviewResponse {
            result: Some(match result {
                Ok(result) => pb::get_portfolio_overview_response::Result::Overview(overview(
                    result.overview(),
                )),
                Err(error) => pb::get_portfolio_overview_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }
}

pub(crate) fn parse_requested_context(
    value: Option<&pb::NormalizedPortfolioContext>,
) -> Result<RequestedNormalizedPortfolioContext, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(RequestedNormalizedPortfolioContext {
        scope: parse_exact_scope(value.scope.as_ref())?,
        valuation_at: parse_market_time(value.valuation_at.as_ref())?,
        knowledge_at: parse_market_time(value.knowledge_at.as_ref())?,
        currency: parse_currency(value.currency)?,
        currency_unit: parse_unit_ref(value.currency_unit.as_ref())?,
        look_through: parse_look_through(value.look_through)?,
        benchmark: parse_benchmark_ref(value.benchmark.as_ref())?,
        period: parse_period(value.period)?,
        period_from: parse_market_time(value.period_from.as_ref())?,
        period_to: parse_market_time(value.period_to.as_ref())?,
        metric_convention: parse_metric_convention_ref(value.metric_convention.as_ref())?,
    })
}

fn parse_exact_scope(
    value: Option<&pb::ExactPortfolioScope>,
) -> Result<ExactPortfolioScope, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let selected = match value.scope.as_ref().ok_or_else(invalid)? {
        pb::exact_portfolio_scope::Scope::Book(value) => {
            ExactPortfolioScopeKind::Book(parse_exact_lineage(value)?)
        }
        pb::exact_portfolio_scope::Scope::Group(value) => {
            ExactPortfolioScopeKind::Group(parse_exact_lineage(value)?)
        }
        pb::exact_portfolio_scope::Scope::Portfolio(value) => {
            ExactPortfolioScopeKind::Portfolio(parse_exact_lineage(value)?)
        }
    };
    let members = value
        .member_portfolios
        .iter()
        .map(parse_exact_lineage)
        .collect::<Result<Vec<_>, _>>()?;
    if members.is_empty() {
        return Err(invalid());
    }
    Ok(ExactPortfolioScope::new(selected, members))
}

fn parse_exact_lineage(value: &core::LineageRef) -> Result<LineageRef, ApplicationError> {
    if value.version == 0 {
        return Err(invalid());
    }
    LineageRef::new(
        parse_ulid(value.object_id.as_ref())?,
        Some(Version::new(value.version).map_err(ficant_application::map_domain_error)?),
        Some(parse_hash(value.content_hash.as_ref())?),
    )
    .map_err(ficant_application::map_domain_error)
}

pub(crate) fn parse_currency(value: i32) -> Result<PortfolioCurrencyMode, ApplicationError> {
    match pb::PortfolioCurrencyMode::try_from(value).map_err(|_| invalid())? {
        pb::PortfolioCurrencyMode::Original => Ok(PortfolioCurrencyMode::Original),
        pb::PortfolioCurrencyMode::Cny => Ok(PortfolioCurrencyMode::Cny),
        pb::PortfolioCurrencyMode::Unspecified => Err(invalid()),
    }
}

pub(crate) fn parse_look_through(value: i32) -> Result<PortfolioLookThroughMode, ApplicationError> {
    match pb::PortfolioLookThroughMode::try_from(value).map_err(|_| invalid())? {
        pb::PortfolioLookThroughMode::None => Ok(PortfolioLookThroughMode::None),
        pb::PortfolioLookThroughMode::Consolidated => Ok(PortfolioLookThroughMode::Consolidated),
        pb::PortfolioLookThroughMode::Separate => Ok(PortfolioLookThroughMode::Separate),
        pb::PortfolioLookThroughMode::Unspecified => Err(invalid()),
    }
}

pub(crate) fn parse_period(value: i32) -> Result<PortfolioPeriodPreset, ApplicationError> {
    match pb::PortfolioPeriodPreset::try_from(value).map_err(|_| invalid())? {
        pb::PortfolioPeriodPreset::OneDay => Ok(PortfolioPeriodPreset::OneDay),
        pb::PortfolioPeriodPreset::SevenDays => Ok(PortfolioPeriodPreset::SevenDays),
        pb::PortfolioPeriodPreset::ThirtyDays => Ok(PortfolioPeriodPreset::ThirtyDays),
        pb::PortfolioPeriodPreset::YearToDate => Ok(PortfolioPeriodPreset::YearToDate),
        pb::PortfolioPeriodPreset::OneYear => Ok(PortfolioPeriodPreset::OneYear),
        pb::PortfolioPeriodPreset::Unspecified => Err(invalid()),
    }
}

fn parse_benchmark_ref(value: Option<&pb::BenchmarkRef>) -> Result<BenchmarkRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(BenchmarkRef::new(
        parse_version_ref(value.benchmark.as_ref())?,
        parse_hash(value.content_hash.as_ref())?,
    ))
}

fn parse_metric_convention_ref(
    value: Option<&pb::PortfolioMetricConventionRef>,
) -> Result<PortfolioMetricConventionRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(PortfolioMetricConventionRef::new(
        parse_version_ref(value.convention.as_ref())?,
        parse_hash(value.content_hash.as_ref())?,
    ))
}

pub(crate) fn normalized_context(
    value: &NormalizedPortfolioContext,
) -> pb::NormalizedPortfolioContext {
    pb::NormalizedPortfolioContext {
        scope: Some(exact_scope(&value.scope)),
        valuation_at: Some(market_time(&value.valuation_at)),
        knowledge_at: Some(market_time(&value.knowledge_at)),
        currency: proto_currency(value.currency) as i32,
        currency_unit: Some(unit_ref(&value.currency_unit)),
        look_through: proto_look_through(value.look_through) as i32,
        benchmark: Some(benchmark_ref(&value.benchmark)),
        period: proto_period(value.period) as i32,
        period_from: Some(market_time(&value.period_from)),
        period_to: Some(market_time(&value.period_to)),
        metric_convention: Some(metric_convention_ref(&value.metric_convention)),
    }
}

pub(crate) fn exact_scope(value: &ExactPortfolioScope) -> pb::ExactPortfolioScope {
    pb::ExactPortfolioScope {
        scope: Some(match value.selected() {
            ExactPortfolioScopeKind::Book(value) => {
                pb::exact_portfolio_scope::Scope::Book(lineage(value))
            }
            ExactPortfolioScopeKind::Group(value) => {
                pb::exact_portfolio_scope::Scope::Group(lineage(value))
            }
            ExactPortfolioScopeKind::Portfolio(value) => {
                pb::exact_portfolio_scope::Scope::Portfolio(lineage(value))
            }
        }),
        member_portfolios: value.member_portfolios().iter().map(lineage).collect(),
    }
}

pub(crate) fn overview(value: &PortfolioOverview) -> pb::PortfolioOverview {
    let draft = value.draft();
    pb::PortfolioOverview {
        scope: Some(exact_scope(draft.scope())),
        position_snapshots: draft
            .position_snapshots()
            .iter()
            .map(snapshot_binding)
            .collect(),
        basic_metrics: Some(basic_metrics(draft.basic_metrics())),
        krd_summary: Some(krd_summary(draft.krd_summary())),
        benchmark_metrics: Some(basic_metrics(draft.benchmark_metrics())),
        benchmark: Some(benchmark_ref(draft.benchmark())),
        metric_convention: Some(metric_convention_ref(draft.metric_convention())),
        coverage: Some(metric_coverage(draft.coverage())),
        members: draft
            .members()
            .iter()
            .map(|member| pb::PortfolioMemberOverview {
                portfolio: Some(lineage(member.portfolio())),
                position_snapshot: Some(snapshot_binding(member.position_snapshot())),
                basic_metrics: Some(basic_metrics(member.basic_metrics())),
                krd_summary: Some(krd_summary(member.krd_summary())),
            })
            .collect(),
        request_fingerprint: Some(hash(draft.request_fingerprint())),
        formal_evidence: Some(proto_formal_evidence(value.formal_evidence())),
    }
}

pub(crate) fn basic_metrics(value: &PortfolioBasicMetrics) -> pb::PortfolioBasicMetrics {
    pb::PortfolioBasicMetrics {
        market_value: Some(decimal(value.market_value())),
        economic_pnl: Some(decimal(value.economic_pnl())),
        weighted_ytm: value.weighted_ytm().map(decimal),
        modified_duration: value.modified_duration().map(decimal),
        convexity: value.convexity().map(decimal),
        weighted_coupon_rate: value.weighted_coupon_rate().map(decimal),
        weighted_remaining_years: value.weighted_remaining_years().map(decimal),
        dv01: Some(decimal(value.dv01())),
    }
}

pub(crate) fn krd_summary(value: &PortfolioKrdSummary) -> pb::PortfolioKrdSummary {
    pb::PortfolioKrdSummary {
        totals: value.totals().iter().map(factor).collect(),
        parallel_dv01: Some(decimal(value.parallel_dv01())),
    }
}

pub(crate) fn metric_coverage(value: &PortfolioCoverage) -> pb::PortfolioCoverage {
    pb::PortfolioCoverage {
        participation: Some(coverage_declaration(value.participation())),
        missing_reasons: value
            .missing_reasons()
            .iter()
            .map(|reason| coverage_reason(*reason).to_owned())
            .collect(),
    }
}

pub(crate) fn coverage_declaration(value: &CoverageDeclaration) -> research::CoverageDeclaration {
    research::CoverageDeclaration {
        imported_position_count: value.imported_position_count(),
        participating_position_count: value.participating_position_count(),
        imported_gross_economic_value_by_unit: value
            .imported_gross_economic_value_by_unit()
            .iter()
            .map(decimal)
            .collect(),
        participating_gross_economic_value_by_unit: value
            .participating_gross_economic_value_by_unit()
            .iter()
            .map(decimal)
            .collect(),
        missing_critical_field_record_count: value.missing_critical_field_record_count(),
        source_confidence: value.source_confidence().map(source_confidence),
        distinct_external_data_source_version_count: value
            .distinct_external_data_source_version_count(),
    }
}

pub(crate) fn factor(value: &FactorDv01) -> research::FactorDv01 {
    research::FactorDv01 {
        factor_id: value.factor_id().to_owned(),
        factor_definition_hash: Some(hash(value.factor_definition_hash())),
        dv01: Some(core::DecimalValue {
            coefficient: value.value().scaled().to_string(),
            scale: 12,
            unit: Some(unit_ref(value.unit())),
        }),
    }
}

fn source_confidence(value: &PriceSourceSummary) -> research::PriceSourceSummary {
    research::PriceSourceSummary {
        counts: value
            .counts()
            .iter()
            .map(|count| research::PriceSourceCount {
                source_type: price_source_type(count.source_type()) as i32,
                record_count: count.record_count(),
            })
            .collect(),
        mixed: value.mixed(),
    }
}

const fn price_source_type(value: PriceSourceType) -> market::PriceSourceType {
    match value {
        PriceSourceType::RealTrade => market::PriceSourceType::RealTrade,
        PriceSourceType::ActiveQuote => market::PriceSourceType::ActiveQuote,
        PriceSourceType::ModelValuation => market::PriceSourceType::ModelValuation,
        PriceSourceType::CurveInterpolation => market::PriceSourceType::CurveInterpolation,
    }
}

const fn coverage_reason(value: PortfolioCoverageReason) -> &'static str {
    match value {
        PortfolioCoverageReason::ShortPositionExcludedFromWeightedAverages => {
            "SHORT_POSITION_EXCLUDED_FROM_WEIGHTED_AVERAGES"
        }
        PortfolioCoverageReason::NonBondExcludedFromWeightedAverages => {
            "NON_BOND_EXCLUDED_FROM_WEIGHTED_AVERAGES"
        }
        PortfolioCoverageReason::MissingBondMetricExcludedFromWeightedAverages => {
            "MISSING_BOND_METRIC_EXCLUDED_FROM_WEIGHTED_AVERAGES"
        }
        PortfolioCoverageReason::PositionExcludedFromPortfolioRisk => {
            "POSITION_EXCLUDED_FROM_PORTFOLIO_RISK"
        }
        PortfolioCoverageReason::BenchmarkPositionExcludedFromPortfolioRisk => {
            "BENCHMARK_POSITION_EXCLUDED_FROM_PORTFOLIO_RISK"
        }
    }
}

const fn proto_currency(value: PortfolioCurrencyMode) -> pb::PortfolioCurrencyMode {
    match value {
        PortfolioCurrencyMode::Original => pb::PortfolioCurrencyMode::Original,
        PortfolioCurrencyMode::Cny => pb::PortfolioCurrencyMode::Cny,
    }
}

const fn proto_look_through(value: PortfolioLookThroughMode) -> pb::PortfolioLookThroughMode {
    match value {
        PortfolioLookThroughMode::None => pb::PortfolioLookThroughMode::None,
        PortfolioLookThroughMode::Consolidated => pb::PortfolioLookThroughMode::Consolidated,
        PortfolioLookThroughMode::Separate => pb::PortfolioLookThroughMode::Separate,
    }
}

const fn proto_period(value: PortfolioPeriodPreset) -> pb::PortfolioPeriodPreset {
    match value {
        PortfolioPeriodPreset::OneDay => pb::PortfolioPeriodPreset::OneDay,
        PortfolioPeriodPreset::SevenDays => pb::PortfolioPeriodPreset::SevenDays,
        PortfolioPeriodPreset::ThirtyDays => pb::PortfolioPeriodPreset::ThirtyDays,
        PortfolioPeriodPreset::YearToDate => pb::PortfolioPeriodPreset::YearToDate,
        PortfolioPeriodPreset::OneYear => pb::PortfolioPeriodPreset::OneYear,
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
