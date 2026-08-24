use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use ficant_domain::market::{Calendar, Unit};
use ficant_domain::portfolio::{
    BenchmarkLevelSnapshot, BenchmarkRef, BenchmarkSessionLevel, PortfolioDailyPerformancePoint,
    PortfolioPerformanceConventionRef, PortfolioSessionAggregate, PortfolioValuationSnapshot,
    calculate_daily_performance,
};
use ficant_domain::primitives::{
    ContentHash, FixedDecimal, LineageRef, MarketTime, OwnerRef, UnitRef, Version, VersionRef,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};

use crate::ports::{
    AeadCursorCodec, ApplicationResult, AuthorizedPrincipal, DefinitionRepository, DefinitionValue,
    ExactPortfolioScope, NormalizedPortfolioContextResolution, PORTFOLIO_READ_SCOPE,
    PortfolioCatalogEvidenceRole, PortfolioCatalogRepository, PortfolioPerformanceReadQuery,
    PortfolioPerformanceRepository, PositionSnapshotRepository, ResolvedPortfolioAggregationInputs,
    VisiblePortfolioPerformanceConvention, stored_definition_content_hash,
};
use crate::{ApplicationError, ApplicationErrorCategory, ListPortfolioCatalog};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PortfolioPerformanceEvidenceKind {
    Book = 1,
    PortfolioGroup = 2,
    Portfolio = 3,
    Benchmark = 4,
    PortfolioMetricConvention = 5,
    PortfolioPerformanceConvention = 6,
    Calendar = 7,
    Unit = 8,
    PortfolioValuationSnapshot = 9,
    PositionSnapshot = 10,
    BenchmarkLevelSnapshot = 11,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPerformanceEvidenceBinding {
    role: String,
    kind: PortfolioPerformanceEvidenceKind,
    owner: OwnerRef,
    reference: LineageRef,
    observed_at: Option<MarketTime>,
    visible_at: Option<MarketTime>,
    effective_from: Option<MarketTime>,
    effective_to: Option<MarketTime>,
}

impl PortfolioPerformanceEvidenceBinding {
    #[allow(clippy::too_many_arguments)]
    fn new(
        role: String,
        kind: PortfolioPerformanceEvidenceKind,
        owner: OwnerRef,
        reference: LineageRef,
        observed_at: Option<MarketTime>,
        visible_at: Option<MarketTime>,
        effective_from: Option<MarketTime>,
        effective_to: Option<MarketTime>,
    ) -> Self {
        Self {
            role,
            kind,
            owner,
            reference,
            observed_at,
            visible_at,
            effective_from,
            effective_to,
        }
    }

    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    #[must_use]
    pub const fn kind(&self) -> PortfolioPerformanceEvidenceKind {
        self.kind
    }

    #[must_use]
    pub const fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub const fn reference(&self) -> &LineageRef {
        &self.reference
    }

    #[must_use]
    pub const fn observed_at(&self) -> Option<&MarketTime> {
        self.observed_at.as_ref()
    }

    #[must_use]
    pub const fn visible_at(&self) -> Option<&MarketTime> {
        self.visible_at.as_ref()
    }

    #[must_use]
    pub const fn effective_from(&self) -> Option<&MarketTime> {
        self.effective_from.as_ref()
    }

    #[must_use]
    pub const fn effective_to(&self) -> Option<&MarketTime> {
        self.effective_to.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPerformanceCoverage {
    expected_sessions: u64,
    observed_sessions: u64,
    expected_portfolio_observations: u64,
    observed_portfolio_observations: u64,
    expected_benchmark_observations: u64,
    observed_benchmark_observations: u64,
}

impl PortfolioPerformanceCoverage {
    fn complete(session_count: usize, member_count: usize) -> ApplicationResult<Self> {
        let expected_session_count = checked_u64(session_count)?;
        let portfolio_count = session_count
            .checked_mul(member_count)
            .ok_or_else(validation)?;
        let expected_portfolio_observation_count = checked_u64(portfolio_count)?;
        Ok(Self {
            expected_sessions: expected_session_count,
            observed_sessions: expected_session_count,
            expected_portfolio_observations: expected_portfolio_observation_count,
            observed_portfolio_observations: expected_portfolio_observation_count,
            expected_benchmark_observations: expected_session_count,
            observed_benchmark_observations: expected_session_count,
        })
    }

    #[must_use]
    pub const fn expected_session_count(&self) -> u64 {
        self.expected_sessions
    }

    #[must_use]
    pub const fn observed_session_count(&self) -> u64 {
        self.observed_sessions
    }

    #[must_use]
    pub const fn expected_portfolio_observation_count(&self) -> u64 {
        self.expected_portfolio_observations
    }

    #[must_use]
    pub const fn observed_portfolio_observation_count(&self) -> u64 {
        self.observed_portfolio_observations
    }

    #[must_use]
    pub const fn expected_benchmark_observation_count(&self) -> u64 {
        self.expected_benchmark_observations
    }

    #[must_use]
    pub const fn observed_benchmark_observation_count(&self) -> u64 {
        self.observed_benchmark_observations
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPerformanceDraft {
    owner: OwnerRef,
    subject_ref: VersionRef,
    scope: ExactPortfolioScope,
    performance_convention: PortfolioPerformanceConventionRef,
    benchmark: BenchmarkRef,
    currency_unit: UnitRef,
    return_unit: UnitRef,
    period_from: MarketTime,
    period_to: MarketTime,
    points: Vec<PortfolioDailyPerformancePoint>,
    coverage: PortfolioPerformanceCoverage,
    request_fingerprint: ContentHash,
    evidence: Vec<PortfolioPerformanceEvidenceBinding>,
}

impl PortfolioPerformanceDraft {
    #[must_use]
    pub const fn owner(&self) -> &OwnerRef {
        &self.owner
    }
    #[must_use]
    pub const fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }
    #[must_use]
    pub const fn scope(&self) -> &ExactPortfolioScope {
        &self.scope
    }
    #[must_use]
    pub const fn performance_convention(&self) -> &PortfolioPerformanceConventionRef {
        &self.performance_convention
    }
    #[must_use]
    pub const fn benchmark(&self) -> &BenchmarkRef {
        &self.benchmark
    }
    #[must_use]
    pub const fn currency_unit(&self) -> &UnitRef {
        &self.currency_unit
    }
    #[must_use]
    pub const fn return_unit(&self) -> &UnitRef {
        &self.return_unit
    }
    #[must_use]
    pub const fn period_from(&self) -> &MarketTime {
        &self.period_from
    }
    #[must_use]
    pub const fn period_to(&self) -> &MarketTime {
        &self.period_to
    }
    #[must_use]
    pub fn points(&self) -> &[PortfolioDailyPerformancePoint] {
        &self.points
    }
    #[must_use]
    pub const fn coverage(&self) -> &PortfolioPerformanceCoverage {
        &self.coverage
    }
    #[must_use]
    pub const fn request_fingerprint(&self) -> &ContentHash {
        &self.request_fingerprint
    }
    #[must_use]
    pub fn evidence(&self) -> &[PortfolioPerformanceEvidenceBinding] {
        &self.evidence
    }
}

pub trait PortfolioPerformanceEngine: Send + Sync {
    /// Calculates exact daily portfolio and benchmark performance points.
    ///
    /// # Errors
    ///
    /// Returns an application error when the materialized observations are incomplete,
    /// inconsistent, or cannot be represented by the frozen decimal convention.
    fn calculate(
        &self,
        portfolio: &[PortfolioSessionAggregate],
        benchmark: &[BenchmarkSessionLevel],
    ) -> ApplicationResult<Vec<PortfolioDailyPerformancePoint>>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FixedDecimalPortfolioPerformanceEngine;

impl PortfolioPerformanceEngine for FixedDecimalPortfolioPerformanceEngine {
    fn calculate(
        &self,
        portfolio: &[PortfolioSessionAggregate],
        benchmark: &[BenchmarkSessionLevel],
    ) -> ApplicationResult<Vec<PortfolioDailyPerformancePoint>> {
        calculate_daily_performance(portfolio, benchmark).map_err(crate::map_domain_error)
    }
}

#[derive(Clone)]
pub struct OwnedPortfolioPerformanceCatalogAuthority {
    catalog: Arc<dyn PortfolioCatalogRepository>,
    cursor: Arc<AeadCursorCodec>,
}

impl OwnedPortfolioPerformanceCatalogAuthority {
    #[must_use]
    pub fn new(catalog: Arc<dyn PortfolioCatalogRepository>, cursor: Arc<AeadCursorCodec>) -> Self {
        Self { catalog, cursor }
    }
}

#[async_trait]
pub trait PortfolioPerformanceCatalogAuthority: Send + Sync {
    async fn resolve(
        &self,
        scope: &crate::ports::AccessScope,
        resolution: &NormalizedPortfolioContextResolution,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs>;
}

#[async_trait]
impl PortfolioPerformanceCatalogAuthority for OwnedPortfolioPerformanceCatalogAuthority {
    async fn resolve(
        &self,
        scope: &crate::ports::AccessScope,
        resolution: &NormalizedPortfolioContextResolution,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs> {
        ListPortfolioCatalog::new(self.catalog.as_ref(), self.cursor.as_ref())
            .resolve_aggregation_inputs_with_evidence(scope, resolution)
            .await
    }
}

#[derive(Clone)]
pub struct OwnedPortfolioPerformanceBackend {
    authority: Arc<dyn PortfolioPerformanceCatalogAuthority>,
    performance: Arc<dyn PortfolioPerformanceRepository>,
    definitions: Arc<dyn DefinitionRepository>,
    positions: Arc<dyn PositionSnapshotRepository>,
    engine: Arc<dyn PortfolioPerformanceEngine>,
}

impl OwnedPortfolioPerformanceBackend {
    #[must_use]
    pub fn new(
        authority: Arc<dyn PortfolioPerformanceCatalogAuthority>,
        performance: Arc<dyn PortfolioPerformanceRepository>,
        definitions: Arc<dyn DefinitionRepository>,
        positions: Arc<dyn PositionSnapshotRepository>,
        engine: Arc<dyn PortfolioPerformanceEngine>,
    ) -> Self {
        Self {
            authority,
            performance,
            definitions,
            positions,
            engine,
        }
    }

    /// Materializes every exact input before the first arithmetic call.
    ///
    /// # Errors
    ///
    /// Returns authorization, missing, integrity, temporal, Unit or arithmetic failures without
    /// producing a partial series.
    #[allow(clippy::too_many_lines)]
    pub async fn execute_resolution(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
    ) -> ApplicationResult<PortfolioPerformanceDraft> {
        principal.require_role(ficant_domain::governance::PlatformRole::Researcher)?;
        if !principal.has_scope(PORTFOLIO_READ_SCOPE) {
            return Err(forbidden());
        }
        let context = resolution.context();
        principal.access_scope().authorize(&context.owner)?;
        if context.knowledge_at.instant() < context.period_to.instant()
            || context.period_from.instant() >= context.period_to.instant()
            || context.scope.member_portfolios().is_empty()
        {
            return Err(validation());
        }

        let resolved = self
            .authority
            .resolve(principal.access_scope(), resolution)
            .await?;
        validate_catalog_context(context, &resolved)?;

        let benchmark = exact_benchmark(&context.benchmark)?;
        let query = PortfolioPerformanceReadQuery {
            owner: context.owner.clone(),
            subject_ref: context.subject_ref.clone(),
            member_portfolios: context.scope.member_portfolios().to_vec(),
            benchmark,
            period_from: context.period_from.clone(),
            period_to: context.period_to.clone(),
            knowledge_at: context.knowledge_at.clone(),
        };
        let valuations = self
            .performance
            .read_valuation_snapshots(principal.access_scope(), &query)
            .await?;
        let levels = self
            .performance
            .read_benchmark_level_snapshots(principal.access_scope(), &query)
            .await?;
        let convention_ref = unique_convention_ref(&valuations)?;
        let convention = self
            .performance
            .read_performance_convention_exact(
                principal.access_scope(),
                &context.owner,
                convention_ref.reference(),
                convention_ref.content_hash(),
                &context.knowledge_at,
            )
            .await?
            .ok_or_else(not_found)?;
        validate_convention(context, &convention)?;

        let calendar = read_calendar(
            self.definitions.as_ref(),
            principal.access_scope(),
            &context.owner,
            convention.value().calendar(),
        )
        .await?;
        let sessions = expected_sessions(context, &calendar)?;
        let (currency_unit, currency_hash) = read_unit(
            self.definitions.as_ref(),
            principal.access_scope(),
            &context.owner,
            &context.currency_unit,
            &["currency", "currency_amount"],
        )
        .await?;
        let return_unit_ref = levels.first().ok_or_else(not_found)?.level_unit().clone();
        let (return_unit, return_unit_hash) = read_unit(
            self.definitions.as_ref(),
            principal.access_scope(),
            &context.owner,
            &return_unit_ref,
            &["dimensionless"],
        )
        .await?;

        let mut evidence = catalog_evidence(&context.owner, &resolved)?;
        evidence.push(convention_evidence(&convention)?);
        evidence.push(definition_evidence(
            "calendar".to_owned(),
            PortfolioPerformanceEvidenceKind::Calendar,
            &context.owner,
            convention.value().calendar().clone(),
            calendar.effective().from().clone(),
            calendar.effective().to().clone(),
        ));
        evidence.push(unit_evidence(
            "currency-unit".to_owned(),
            &context.owner,
            &currency_unit,
            currency_hash,
        )?);
        evidence.push(unit_evidence(
            "return-unit".to_owned(),
            &context.owner,
            &return_unit,
            return_unit_hash,
        )?);

        let portfolio_series = self
            .materialize_portfolio_series(
                principal,
                context,
                &resolved,
                &sessions,
                &valuations,
                &convention_ref,
                &mut evidence,
            )
            .await?;
        let benchmark_series = materialize_benchmark_series(
            context,
            &query.benchmark,
            &sessions,
            &levels,
            &return_unit_ref,
            &mut evidence,
        )?;

        evidence.sort_by(evidence_order);
        if evidence.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(integrity());
        }
        let request_fingerprint = performance_fingerprint(context, &evidence);
        let points = self
            .engine
            .calculate(&portfolio_series, &benchmark_series)?;
        let coverage =
            PortfolioPerformanceCoverage::complete(sessions.len(), resolved.portfolios.len())?;
        Ok(PortfolioPerformanceDraft {
            owner: context.owner.clone(),
            subject_ref: context.subject_ref.clone(),
            scope: context.scope.clone(),
            performance_convention: convention_ref,
            benchmark: context.benchmark.clone(),
            currency_unit: context.currency_unit.clone(),
            return_unit: return_unit_ref,
            period_from: context.period_from.clone(),
            period_to: context.period_to.clone(),
            points,
            coverage,
            request_fingerprint,
            evidence,
        })
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn materialize_portfolio_series(
        &self,
        principal: &AuthorizedPrincipal,
        context: &crate::ports::NormalizedPortfolioContext,
        resolved: &ResolvedPortfolioAggregationInputs,
        sessions: &[MarketTime],
        valuations: &[PortfolioValuationSnapshot],
        convention_ref: &PortfolioPerformanceConventionRef,
        evidence: &mut Vec<PortfolioPerformanceEvidenceBinding>,
    ) -> ApplicationResult<Vec<PortfolioSessionAggregate>> {
        let expected_count = sessions
            .len()
            .checked_mul(resolved.portfolios.len())
            .ok_or_else(validation)?;
        if valuations.len() != expected_count {
            return Err(lineage());
        }
        let expected_members = resolved
            .portfolios
            .iter()
            .map(|record| exact_portfolio(record.value()))
            .collect::<ApplicationResult<Vec<_>>>()?;
        let mut values = BTreeMap::<(String, NaiveDate), &PortfolioValuationSnapshot>::new();
        let expected_times = sessions
            .iter()
            .map(|time| (time.local_trading_date(), time))
            .collect::<BTreeMap<_, _>>();
        let mut position_evidence = BTreeMap::<String, PortfolioPerformanceEvidenceBinding>::new();

        for snapshot in valuations {
            if snapshot.owner() != &context.owner
                || snapshot.subject_ref() != &context.subject_ref
                || snapshot.performance_convention() != convention_ref
                || snapshot.currency_unit() != &context.currency_unit
                || snapshot.visible_at().instant() > context.knowledge_at.instant()
                || !expected_members.contains(snapshot.portfolio())
            {
                return Err(lineage());
            }
            let expected_time = expected_times
                .get(&snapshot.valuation_at().local_trading_date())
                .ok_or_else(lineage)?;
            if snapshot.valuation_at() != *expected_time {
                return Err(lineage());
            }
            let key = (
                exact_key(snapshot.portfolio())?,
                snapshot.valuation_at().local_trading_date(),
            );
            if values.insert(key, snapshot).is_some() {
                return Err(lineage());
            }

            let position = self
                .positions
                .get_position_snapshot(
                    principal.access_scope(),
                    snapshot.position_snapshot().snapshot_id().clone(),
                    context.knowledge_at.clone(),
                )
                .await?
                .ok_or_else(not_found)?;
            if position.id() != snapshot.position_snapshot().snapshot_id()
                || position.owner() != &context.owner
                || position.subject_ref() != &context.subject_ref
                || position.content_hash() != snapshot.position_snapshot().content_hash()
                || position.observed_at() != snapshot.position_snapshot().observed_at()
                || position.visible_at() != snapshot.position_snapshot().visible_at()
                || position.visible_at().instant() > context.knowledge_at.instant()
            {
                return Err(lineage());
            }
            let position_reference = LineageRef::content_addressed(
                position.id().clone(),
                position.content_hash().clone(),
            );
            position_evidence
                .entry(exact_key(&position_reference)?)
                .or_insert_with(|| {
                    PortfolioPerformanceEvidenceBinding::new(
                        format!(
                            "position-snapshot.{}",
                            position.id().as_str().to_ascii_lowercase()
                        ),
                        PortfolioPerformanceEvidenceKind::PositionSnapshot,
                        context.owner.clone(),
                        position_reference,
                        Some(position.observed_at().clone()),
                        Some(position.visible_at().clone()),
                        None,
                        None,
                    )
                });
            evidence.push(snapshot_evidence(snapshot));
        }

        for record in &resolved.portfolios {
            let member = exact_portfolio(record.value())?;
            let end_snapshot = values
                .get(&(
                    exact_key(&member)?,
                    sessions.last().ok_or_else(lineage)?.local_trading_date(),
                ))
                .ok_or_else(lineage)?;
            if end_snapshot.position_snapshot() != record.value().position_snapshot() {
                return Err(lineage());
            }
        }
        evidence.extend(position_evidence.into_values());

        let mut result = Vec::with_capacity(sessions.len());
        for session in sessions {
            let mut nav = FixedDecimal::ZERO;
            let mut flow = FixedDecimal::ZERO;
            for member in &expected_members {
                let snapshot = values
                    .get(&(exact_key(member)?, session.local_trading_date()))
                    .ok_or_else(lineage)?;
                nav = nav
                    .checked_add(snapshot.net_asset_value())
                    .map_err(crate::map_domain_error)?;
                flow = flow
                    .checked_add(snapshot.net_external_flow())
                    .map_err(crate::map_domain_error)?;
            }
            result.push(
                PortfolioSessionAggregate::new(session.clone(), nav, flow)
                    .map_err(crate::map_domain_error)?,
            );
        }
        Ok(result)
    }
}

fn validate_catalog_context(
    context: &crate::ports::NormalizedPortfolioContext,
    resolved: &ResolvedPortfolioAggregationInputs,
) -> ApplicationResult<()> {
    if resolved.exact_scope != context.scope
        || resolved.portfolios.len() != context.scope.member_portfolios().len()
        || resolved.benchmark.value().reference() != context.benchmark.reference()
        || resolved.benchmark.value().content_hash() != context.benchmark.content_hash()
        || resolved.convention.value().reference() != context.metric_convention.reference()
        || resolved.convention.value().content_hash() != context.metric_convention.content_hash()
    {
        return Err(lineage());
    }
    Ok(())
}

fn validate_convention(
    context: &crate::ports::NormalizedPortfolioContext,
    convention: &VisiblePortfolioPerformanceConvention,
) -> ApplicationResult<()> {
    if convention.value().owner() != &context.owner
        || convention.visible_at().instant() > context.knowledge_at.instant()
        || convention.value().effective_from().instant() > context.period_from.instant()
        || convention.value().effective_to().instant() <= context.period_to.instant()
    {
        return Err(lineage());
    }
    Ok(())
}

async fn read_calendar(
    definitions: &dyn DefinitionRepository,
    scope: &crate::ports::AccessScope,
    owner: &OwnerRef,
    reference: &LineageRef,
) -> ApplicationResult<Calendar> {
    let version = reference.version().ok_or_else(lineage)?;
    let value = definitions
        .get_version(scope, reference.object_id().clone(), version)
        .await?
        .ok_or_else(not_found)?;
    if value.owner() != owner
        || stored_definition_content_hash(&value)
            != reference.content_hash().ok_or_else(lineage)?.clone()
    {
        return Err(lineage());
    }
    match value {
        DefinitionValue::Calendar(calendar) => Ok(calendar),
        _ => Err(lineage()),
    }
}

async fn read_unit(
    definitions: &dyn DefinitionRepository,
    scope: &crate::ports::AccessScope,
    owner: &OwnerRef,
    reference: &UnitRef,
    dimensions: &[&str],
) -> ApplicationResult<(Unit, ContentHash)> {
    let value = definitions
        .get_version(scope, reference.unit_id().clone(), reference.version())
        .await?
        .ok_or_else(not_found)?;
    let hash = stored_definition_content_hash(&value);
    let DefinitionValue::Unit(unit) = value else {
        return Err(lineage());
    };
    if unit.owner() != owner
        || unit.identity() != reference.unit_id().as_str()
        || unit.version() != reference.version().get()
        || !dimensions.contains(&unit.dimension())
    {
        return Err(lineage());
    }
    Ok((unit, hash))
}

fn expected_sessions(
    context: &crate::ports::NormalizedPortfolioContext,
    calendar: &Calendar,
) -> ApplicationResult<Vec<MarketTime>> {
    if calendar.owner() != &context.owner
        || context.period_from.market_timezone() != calendar.market_timezone()
        || context.period_to.market_timezone() != calendar.market_timezone()
        || calendar.effective().from().instant() > context.period_from.instant()
        || calendar.effective().to().instant() <= context.period_to.instant()
    {
        return Err(lineage());
    }
    let timezone = calendar
        .market_timezone()
        .parse::<Tz>()
        .map_err(|_| validation())?;
    let from = context.period_from.local_trading_date();
    let to = context.period_to.local_trading_date();
    let mut sessions = Vec::new();
    for session in calendar.sessions() {
        if session.local_date() < from || session.local_date() > to {
            continue;
        }
        let Some(close) = session.close_local_time() else {
            continue;
        };
        let local = session.local_date().and_time(close);
        let instant = timezone
            .from_local_datetime(&local)
            .single()
            .ok_or_else(validation)?
            .with_timezone(&Utc);
        sessions.push(
            MarketTime::new(instant, calendar.market_timezone(), session.local_date())
                .map_err(crate::map_domain_error)?,
        );
    }
    if sessions.len() < 2 {
        return Err(validation());
    }
    Ok(sessions)
}

fn materialize_benchmark_series(
    context: &crate::ports::NormalizedPortfolioContext,
    benchmark: &LineageRef,
    sessions: &[MarketTime],
    levels: &[BenchmarkLevelSnapshot],
    return_unit: &UnitRef,
    evidence: &mut Vec<PortfolioPerformanceEvidenceBinding>,
) -> ApplicationResult<Vec<BenchmarkSessionLevel>> {
    if levels.len() != sessions.len() {
        return Err(lineage());
    }
    let expected = sessions
        .iter()
        .map(|time| (time.local_trading_date(), time))
        .collect::<BTreeMap<_, _>>();
    let mut by_date = BTreeMap::new();
    for level in levels {
        if level.owner() != &context.owner
            || level.subject_ref() != &context.subject_ref
            || level.benchmark() != benchmark
            || level.level_unit() != return_unit
            || level.visible_at().instant() > context.knowledge_at.instant()
        {
            return Err(lineage());
        }
        let expected_time = expected
            .get(&level.valuation_at().local_trading_date())
            .ok_or_else(lineage)?;
        if level.valuation_at() != *expected_time
            || by_date
                .insert(level.valuation_at().local_trading_date(), level)
                .is_some()
        {
            return Err(lineage());
        }
        evidence.push(benchmark_level_evidence(level));
    }
    sessions
        .iter()
        .map(|session| {
            let level = by_date
                .get(&session.local_trading_date())
                .ok_or_else(lineage)?;
            BenchmarkSessionLevel::new(session.clone(), level.level())
                .map_err(crate::map_domain_error)
        })
        .collect()
}

fn unique_convention_ref(
    values: &[PortfolioValuationSnapshot],
) -> ApplicationResult<PortfolioPerformanceConventionRef> {
    let first = values
        .first()
        .ok_or_else(not_found)?
        .performance_convention()
        .clone();
    if values
        .iter()
        .any(|value| value.performance_convention() != &first)
    {
        return Err(lineage());
    }
    Ok(first)
}

fn catalog_evidence(
    owner: &OwnerRef,
    resolved: &ResolvedPortfolioAggregationInputs,
) -> ApplicationResult<Vec<PortfolioPerformanceEvidenceBinding>> {
    resolved
        .catalog_evidence
        .iter()
        .map(|binding| {
            let (kind, role) = match binding.role() {
                PortfolioCatalogEvidenceRole::SelectedBook => {
                    (PortfolioPerformanceEvidenceKind::Book, "selected-book")
                }
                PortfolioCatalogEvidenceRole::SelectedGroup => (
                    PortfolioPerformanceEvidenceKind::PortfolioGroup,
                    "selected-group",
                ),
                PortfolioCatalogEvidenceRole::SelectedPortfolio => (
                    PortfolioPerformanceEvidenceKind::Portfolio,
                    "selected-portfolio",
                ),
                PortfolioCatalogEvidenceRole::MemberPortfolio => (
                    PortfolioPerformanceEvidenceKind::Portfolio,
                    "member-portfolio",
                ),
                PortfolioCatalogEvidenceRole::Benchmark => {
                    (PortfolioPerformanceEvidenceKind::Benchmark, "benchmark")
                }
                PortfolioCatalogEvidenceRole::MetricConvention => (
                    PortfolioPerformanceEvidenceKind::PortfolioMetricConvention,
                    "metric-convention",
                ),
            };
            let reference = LineageRef::new(
                binding.reference().id().clone(),
                Some(binding.reference().version()),
                Some(binding.content_hash().clone()),
            )
            .map_err(crate::map_domain_error)?;
            Ok(PortfolioPerformanceEvidenceBinding::new(
                format!(
                    "{role}.{}",
                    binding.reference().id().as_str().to_ascii_lowercase()
                ),
                kind,
                owner.clone(),
                reference,
                None,
                Some(binding.visible_at().clone()),
                Some(binding.effective_from().clone()),
                Some(binding.effective_to().clone()),
            ))
        })
        .collect()
}

fn convention_evidence(
    value: &VisiblePortfolioPerformanceConvention,
) -> ApplicationResult<PortfolioPerformanceEvidenceBinding> {
    let reference = LineageRef::new(
        value.value().reference().id().clone(),
        Some(value.value().reference().version()),
        Some(value.value().content_hash().clone()),
    )
    .map_err(crate::map_domain_error)?;
    Ok(PortfolioPerformanceEvidenceBinding::new(
        "performance-convention".to_owned(),
        PortfolioPerformanceEvidenceKind::PortfolioPerformanceConvention,
        value.value().owner().clone(),
        reference,
        None,
        Some(value.visible_at().clone()),
        Some(value.value().effective_from().clone()),
        Some(value.value().effective_to().clone()),
    ))
}

fn definition_evidence(
    role: String,
    kind: PortfolioPerformanceEvidenceKind,
    owner: &OwnerRef,
    reference: LineageRef,
    effective_from: MarketTime,
    effective_to: MarketTime,
) -> PortfolioPerformanceEvidenceBinding {
    PortfolioPerformanceEvidenceBinding::new(
        role,
        kind,
        owner.clone(),
        reference,
        None,
        None,
        Some(effective_from),
        Some(effective_to),
    )
}

fn unit_evidence(
    role: String,
    owner: &OwnerRef,
    unit: &Unit,
    content_hash: ContentHash,
) -> ApplicationResult<PortfolioPerformanceEvidenceBinding> {
    let reference = LineageRef::new(
        ficant_domain::primitives::Ulid::new(unit.identity().to_owned())
            .map_err(crate::map_domain_error)?,
        Some(
            ficant_domain::primitives::Version::new(unit.version())
                .map_err(crate::map_domain_error)?,
        ),
        Some(content_hash),
    )
    .map_err(crate::map_domain_error)?;
    Ok(PortfolioPerformanceEvidenceBinding::new(
        role,
        PortfolioPerformanceEvidenceKind::Unit,
        owner.clone(),
        reference,
        None,
        None,
        None,
        None,
    ))
}

fn snapshot_evidence(value: &PortfolioValuationSnapshot) -> PortfolioPerformanceEvidenceBinding {
    PortfolioPerformanceEvidenceBinding::new(
        format!(
            "portfolio-valuation.{}",
            value.snapshot_id().as_str().to_ascii_lowercase()
        ),
        PortfolioPerformanceEvidenceKind::PortfolioValuationSnapshot,
        value.owner().clone(),
        LineageRef::content_addressed(value.snapshot_id().clone(), value.content_hash().clone()),
        Some(value.valuation_at().clone()),
        Some(value.visible_at().clone()),
        None,
        None,
    )
}

fn benchmark_level_evidence(value: &BenchmarkLevelSnapshot) -> PortfolioPerformanceEvidenceBinding {
    PortfolioPerformanceEvidenceBinding::new(
        format!(
            "benchmark-level.{}",
            value.snapshot_id().as_str().to_ascii_lowercase()
        ),
        PortfolioPerformanceEvidenceKind::BenchmarkLevelSnapshot,
        value.owner().clone(),
        LineageRef::content_addressed(value.snapshot_id().clone(), value.content_hash().clone()),
        Some(value.valuation_at().clone()),
        Some(value.visible_at().clone()),
        None,
        None,
    )
}

fn exact_portfolio(value: &ficant_domain::portfolio::Portfolio) -> ApplicationResult<LineageRef> {
    LineageRef::new(
        value.reference().id().clone(),
        Some(value.reference().version()),
        Some(value.content_hash().clone()),
    )
    .map_err(crate::map_domain_error)
}

fn exact_benchmark(value: &BenchmarkRef) -> ApplicationResult<LineageRef> {
    LineageRef::new(
        value.reference().id().clone(),
        Some(value.reference().version()),
        Some(value.content_hash().clone()),
    )
    .map_err(crate::map_domain_error)
}

fn exact_key(value: &LineageRef) -> ApplicationResult<String> {
    Ok(format!(
        "{}@{}#{}",
        value.object_id().as_str(),
        value.version().map_or(0, Version::get),
        hex(value.content_hash().ok_or_else(lineage)?.as_bytes())
    ))
}

fn evidence_order(
    left: &PortfolioPerformanceEvidenceBinding,
    right: &PortfolioPerformanceEvidenceBinding,
) -> std::cmp::Ordering {
    left.role
        .cmp(&right.role)
        .then_with(|| left.kind.cmp(&right.kind))
        .then_with(|| left.reference.object_id().cmp(right.reference.object_id()))
}

fn performance_fingerprint(
    context: &crate::ports::NormalizedPortfolioContext,
    evidence: &[PortfolioPerformanceEvidenceBinding],
) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, b"portfolio-performance-request/v1");
    append(&mut bytes, context.owner.tenant_id().as_str().as_bytes());
    append(&mut bytes, context.owner.owner_id().as_str().as_bytes());
    append(&mut bytes, context.subject_ref.id().as_str().as_bytes());
    append(
        &mut bytes,
        &context.subject_ref.version().get().to_be_bytes(),
    );
    append_time(&mut bytes, &context.period_from);
    append_time(&mut bytes, &context.period_to);
    append_time(&mut bytes, &context.knowledge_at);
    for binding in evidence {
        append(&mut bytes, binding.role.as_bytes());
        append(&mut bytes, &[binding.kind as u8]);
        append(
            &mut bytes,
            binding.reference.object_id().as_str().as_bytes(),
        );
        append(
            &mut bytes,
            &binding
                .reference
                .version()
                .map_or(0, Version::get)
                .to_be_bytes(),
        );
        append(
            &mut bytes,
            binding
                .reference
                .content_hash()
                .map_or(&[][..], |value| value.as_bytes().as_slice()),
        );
        for time in [
            binding.observed_at.as_ref(),
            binding.visible_at.as_ref(),
            binding.effective_from.as_ref(),
            binding.effective_to.as_ref(),
        ] {
            match time {
                Some(value) => {
                    append(&mut bytes, &[1]);
                    append_time(&mut bytes, value);
                }
                None => append(&mut bytes, &[0]),
            }
        }
    }
    ContentHash::digest(&bytes)
}

fn append_time(bytes: &mut Vec<u8>, value: &MarketTime) {
    append(bytes, &value.instant().timestamp().to_be_bytes());
    append(
        bytes,
        &value.instant().timestamp_subsec_nanos().to_be_bytes(),
    );
    append(bytes, value.market_timezone().as_bytes());
    append(bytes, value.local_trading_date().to_string().as_bytes());
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            write!(text, "{byte:02x}").expect("writing to String cannot fail");
            text
        })
}

fn checked_u64(value: usize) -> ApplicationResult<u64> {
    u64::try_from(value).map_err(|_| validation())
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn lineage() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

fn integrity() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}
