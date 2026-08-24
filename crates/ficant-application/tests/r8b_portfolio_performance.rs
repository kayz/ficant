use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, CursorPage, DefinitionIdentity, DefinitionRepository,
    DefinitionValue, ExactPortfolioScope, ExactPortfolioScopeKind, NormalizedPortfolioContext,
    NormalizedPortfolioContextResolution, PageRequest, PortfolioCatalogEvidenceBinding,
    PortfolioCatalogEvidenceRole, PortfolioCurrencyMode, PortfolioLookThroughMode,
    PortfolioPerformanceReadQuery, PortfolioPerformanceRepository, PortfolioPeriodPreset,
    PositionSnapshotRepository, ResolvedPortfolioAggregationInputs, VisibleCatalogRecord,
    VisiblePortfolioPerformanceConvention,
};
use ficant_application::use_cases::portfolio_performance::{
    FixedDecimalPortfolioPerformanceEngine, OwnedPortfolioPerformanceBackend,
    PortfolioPerformanceCatalogAuthority, PortfolioPerformanceEngine,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::ContentAddressed;
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::{Calendar, CalendarInput, CalendarSession, Unit, UnitInput};
use ficant_domain::portfolio::{
    Benchmark, BenchmarkInput, BenchmarkLevelSnapshot, BenchmarkLevelSnapshotInput, BenchmarkRef,
    Portfolio, PortfolioDecimalRounding, PortfolioExternalFlowTiming, PortfolioInput,
    PortfolioMetricConvention, PortfolioMetricConventionInput, PortfolioMetricConventionRef,
    PortfolioMetricWeighting, PortfolioPerformanceConvention, PortfolioPerformanceConventionInput,
    PortfolioPerformanceConventionRef, PortfolioPerformanceReturnMethod, PortfolioSnapshotBinding,
    PortfolioStatus, PortfolioValuationFrequency, PortfolioValuationSnapshot,
    PortfolioValuationSnapshotInput,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, FixedDecimal, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{PositionSnapshot, PositionSnapshotInput};

#[tokio::test]
async fn group_series_aggregates_nav_before_daily_twr_and_binds_every_input() {
    let fixture = Fixture::new(false);
    let draft = fixture
        .backend()
        .execute_resolution(&fixture.principal, &fixture.resolution)
        .await
        .unwrap();

    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 1);
    assert_eq!(draft.points().len(), 2);
    assert_eq!(draft.coverage().expected_session_count(), 3);
    assert_eq!(draft.coverage().expected_portfolio_observation_count(), 6);
    assert_eq!(draft.coverage().expected_benchmark_observation_count(), 3);
    assert_eq!(draft.points()[0].opening_nav(), amount(300));
    assert_eq!(draft.points()[0].ending_nav(), amount(315));
    assert_eq!(draft.points()[0].net_external_flow(), amount(10));
    assert_eq!(draft.points()[0].economic_pnl(), amount(5));
    assert_eq!(draft.points()[0].daily_return(), scaled("0.016666666667"));
    assert!(
        draft
            .evidence()
            .iter()
            .any(|binding| binding.role().starts_with("portfolio-valuation."))
    );
    assert_eq!(
        draft
            .evidence()
            .iter()
            .filter(|binding| binding.role().starts_with("benchmark-level."))
            .count(),
        3
    );
    assert!(draft.evidence().iter().all(|binding| {
        let role = binding.role();
        role == role.to_ascii_lowercase()
            && role.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
    }));
}

#[tokio::test]
async fn missing_benchmark_session_fails_before_arithmetic() {
    let fixture = Fixture::new(true);
    let error = fixture
        .backend()
        .execute_resolution(&fixture.principal, &fixture.resolution)
        .await
        .unwrap_err();
    assert_eq!(
        error.category(),
        ApplicationErrorCategory::LineageIncomplete
    );
    assert_eq!(fixture.engine.calls.load(Ordering::SeqCst), 0);
}

#[derive(Clone)]
struct StaticCatalogAuthority {
    resolved: ResolvedPortfolioAggregationInputs,
}

#[async_trait]
impl PortfolioPerformanceCatalogAuthority for StaticCatalogAuthority {
    async fn resolve(
        &self,
        _scope: &AccessScope,
        _resolution: &NormalizedPortfolioContextResolution,
    ) -> Result<ResolvedPortfolioAggregationInputs, ApplicationError> {
        Ok(self.resolved.clone())
    }
}

#[derive(Clone)]
struct StaticPerformanceRepository {
    valuations: Vec<PortfolioValuationSnapshot>,
    levels: Vec<BenchmarkLevelSnapshot>,
    convention: VisiblePortfolioPerformanceConvention,
}

#[async_trait]
impl PortfolioPerformanceRepository for StaticPerformanceRepository {
    async fn read_valuation_snapshots(
        &self,
        _scope: &AccessScope,
        _query: &PortfolioPerformanceReadQuery,
    ) -> Result<Vec<PortfolioValuationSnapshot>, ApplicationError> {
        Ok(self.valuations.clone())
    }

    async fn read_benchmark_level_snapshots(
        &self,
        _scope: &AccessScope,
        _query: &PortfolioPerformanceReadQuery,
    ) -> Result<Vec<BenchmarkLevelSnapshot>, ApplicationError> {
        Ok(self.levels.clone())
    }

    async fn read_performance_convention_exact(
        &self,
        _scope: &AccessScope,
        _owner: &OwnerRef,
        _reference: &VersionRef,
        _content_hash: &ContentHash,
        _knowledge_at: &MarketTime,
    ) -> Result<Option<VisiblePortfolioPerformanceConvention>, ApplicationError> {
        Ok(Some(self.convention.clone()))
    }
}

#[derive(Clone)]
struct StaticDefinitions {
    values: Vec<DefinitionValue>,
}

#[async_trait]
impl DefinitionRepository for StaticDefinitions {
    async fn create_identity(&self, _identity: DefinitionIdentity) -> Result<(), ApplicationError> {
        Err(unused())
    }

    async fn append_version(
        &self,
        _command: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        Err(unused())
    }

    async fn get_version(
        &self,
        _scope: &AccessScope,
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
        _scope: &AccessScope,
        _definition_id: Ulid,
        _instant: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Err(unused())
    }

    async fn list_versions(
        &self,
        _scope: &AccessScope,
        _definition_id: Ulid,
        _page: PageRequest,
    ) -> Result<CursorPage<DefinitionValue>, ApplicationError> {
        Err(unused())
    }
}

#[derive(Clone)]
struct StaticPositions(Vec<PositionSnapshot>);

#[async_trait]
impl PositionSnapshotRepository for StaticPositions {
    async fn get_position_snapshot(
        &self,
        _scope: &AccessScope,
        snapshot_id: Ulid,
        _knowledge_at: MarketTime,
    ) -> Result<Option<PositionSnapshot>, ApplicationError> {
        Ok(self
            .0
            .iter()
            .find(|value| value.id() == &snapshot_id)
            .cloned())
    }

    async fn resolve_position_snapshot(
        &self,
        _scope: &AccessScope,
        _subject_ref: VersionRef,
        _observed_at: MarketTime,
        _knowledge_at: MarketTime,
    ) -> Result<Option<PositionSnapshot>, ApplicationError> {
        Err(unused())
    }
}

#[derive(Default)]
struct CountingEngine {
    calls: AtomicUsize,
}

impl PortfolioPerformanceEngine for CountingEngine {
    fn calculate(
        &self,
        portfolio: &[ficant_domain::portfolio::PortfolioSessionAggregate],
        benchmark: &[ficant_domain::portfolio::BenchmarkSessionLevel],
    ) -> Result<Vec<ficant_domain::portfolio::PortfolioDailyPerformancePoint>, ApplicationError>
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        FixedDecimalPortfolioPerformanceEngine.calculate(portfolio, benchmark)
    }
}

struct Fixture {
    principal: ficant_application::ports::AuthorizedPrincipal,
    resolution: NormalizedPortfolioContextResolution,
    authority: Arc<StaticCatalogAuthority>,
    performance: Arc<StaticPerformanceRepository>,
    definitions: Arc<StaticDefinitions>,
    positions: Arc<StaticPositions>,
    engine: Arc<CountingEngine>,
}

impl Fixture {
    #[allow(clippy::too_many_lines)]
    fn new(missing_benchmark: bool) -> Self {
        let owner = owner();
        let subject = version_ref(2);
        let currency = Unit::new(UnitInput {
            unit_id: id(3),
            version: version(),
            owner: owner.clone(),
            code: "CNY".to_owned(),
            dimension: "currency_amount".to_owned(),
            scale: 12,
            precision: 28,
        })
        .unwrap();
        let dimensionless = Unit::new(UnitInput {
            unit_id: id(4),
            version: version(),
            owner: owner.clone(),
            code: "ONE".to_owned(),
            dimension: "dimensionless".to_owned(),
            scale: 12,
            precision: 28,
        })
        .unwrap();
        let calendar = calendar();
        let calendar_hash = ficant_application::ports::stored_definition_content_hash(
            &DefinitionValue::Calendar(calendar.clone()),
        );
        let calendar_ref = LineageRef::new(id(5), Some(version()), Some(calendar_hash)).unwrap();
        let performance_convention = performance_convention(calendar_ref);
        let performance_ref = PortfolioPerformanceConventionRef::new(
            performance_convention.reference().clone(),
            performance_convention.content_hash().clone(),
        );

        let portfolios = [portfolio(6, 8), portfolio(7, 9)];
        let portfolio_refs = portfolios.iter().map(exact_portfolio).collect::<Vec<_>>();
        let benchmark = benchmark();
        let metric = metric_convention();
        let scope = ExactPortfolioScope::new(
            ExactPortfolioScopeKind::Group(exact_ref(10)),
            portfolio_refs.clone(),
        );
        let context = NormalizedPortfolioContext {
            owner: owner.clone(),
            subject_ref: subject.clone(),
            scope: scope.clone(),
            valuation_at: close_time(22),
            knowledge_at: close_time(23),
            currency: PortfolioCurrencyMode::Cny,
            currency_unit: UnitRef::new(id(3), version()),
            look_through: PortfolioLookThroughMode::Consolidated,
            benchmark: BenchmarkRef::new(
                benchmark.reference().clone(),
                benchmark.content_hash().clone(),
            ),
            period: PortfolioPeriodPreset::SevenDays,
            period_from: close_time(20),
            period_to: close_time(22),
            metric_convention: PortfolioMetricConventionRef::new(
                metric.reference().clone(),
                metric.content_hash().clone(),
            ),
        };
        let evidence = catalog_evidence(&portfolios, &benchmark, &metric);
        let resolution =
            NormalizedPortfolioContextResolution::new(context, evidence.clone()).unwrap();
        let resolved = ResolvedPortfolioAggregationInputs {
            exact_scope: scope,
            portfolios: portfolios
                .iter()
                .cloned()
                .map(|value| VisibleCatalogRecord::new(value, close_time(19)))
                .collect(),
            convention: VisibleCatalogRecord::new(metric, close_time(19)),
            benchmark: VisibleCatalogRecord::new(benchmark.clone(), close_time(19)),
            benchmark_snapshot: benchmark.position_snapshot().clone(),
            catalog_evidence: evidence,
        };

        let mut positions = Vec::new();
        let mut valuations = Vec::new();
        let navs = [
            [amount(100), amount(105), amount(100)],
            [amount(200), amount(210), amount(200)],
        ];
        let flows = [
            [amount(0), amount(5), amount(-10)],
            [amount(0), amount(5), amount(-10)],
        ];
        for (member_index, portfolio_ref) in portfolio_refs.iter().enumerate() {
            for day_index in 0..3 {
                let day = 20 + u32::try_from(day_index).unwrap();
                let position_id = if day_index == 2 {
                    8 + member_index
                } else {
                    11 + member_index * 2 + day_index
                };
                let position = position_snapshot(position_id, day);
                let binding = PortfolioSnapshotBinding::new(
                    position.id().clone(),
                    position.content_hash().clone(),
                    position.observed_at().clone(),
                    position.visible_at().clone(),
                )
                .unwrap();
                let mut input = PortfolioValuationSnapshotInput {
                    snapshot_id: id(17 + member_index * 3 + day_index),
                    owner: owner.clone(),
                    subject_ref: subject.clone(),
                    portfolio: portfolio_ref.clone(),
                    position_snapshot: binding,
                    performance_convention: performance_ref.clone(),
                    valuation_at: close_time(day),
                    visible_at: visible_time(day),
                    currency_unit: UnitRef::new(id(3), version()),
                    gross_assets: navs[member_index][day_index]
                        .checked_add(amount(20))
                        .unwrap(),
                    liabilities: amount(20),
                    net_asset_value: navs[member_index][day_index],
                    net_external_flow: flows[member_index][day_index],
                    content_hash: ContentHash::digest(b"placeholder"),
                };
                input.content_hash = PortfolioValuationSnapshot::content_hash_for(&input);
                valuations.push(PortfolioValuationSnapshot::new(input).unwrap());
                positions.push(position);
            }
        }
        let benchmark_ref = LineageRef::new(
            benchmark.reference().id().clone(),
            Some(benchmark.reference().version()),
            Some(benchmark.content_hash().clone()),
        )
        .unwrap();
        let mut levels = [amount(100), amount(101), scaled("100.500000000000")]
            .into_iter()
            .enumerate()
            .map(|(index, level)| {
                let day = 20 + u32::try_from(index).unwrap();
                let mut input = BenchmarkLevelSnapshotInput {
                    snapshot_id: id(23 + index),
                    owner: owner.clone(),
                    subject_ref: subject.clone(),
                    benchmark: benchmark_ref.clone(),
                    valuation_at: close_time(day),
                    visible_at: visible_time(day),
                    level_unit: UnitRef::new(id(4), version()),
                    level,
                    content_hash: ContentHash::digest(b"placeholder"),
                };
                input.content_hash = BenchmarkLevelSnapshot::content_hash_for(&input);
                BenchmarkLevelSnapshot::new(input).unwrap()
            })
            .collect::<Vec<_>>();
        if missing_benchmark {
            levels.pop();
        }

        Self {
            principal: ficant_application::ports::AuthorizedPrincipal::new(
                "researcher@example.test".to_owned(),
                id(27),
                owner.tenant_id().clone(),
                vec![owner.owner_id().clone()],
                PlatformRole::Researcher,
                vec!["portfolio:read".to_owned()],
                ContentHash::digest(b"credential"),
            )
            .unwrap(),
            resolution,
            authority: Arc::new(StaticCatalogAuthority { resolved }),
            performance: Arc::new(StaticPerformanceRepository {
                valuations,
                levels,
                convention: VisiblePortfolioPerformanceConvention::new(
                    performance_convention,
                    close_time(19),
                ),
            }),
            definitions: Arc::new(StaticDefinitions {
                values: vec![
                    DefinitionValue::Calendar(calendar),
                    DefinitionValue::Unit(currency),
                    DefinitionValue::Unit(dimensionless),
                ],
            }),
            positions: Arc::new(StaticPositions(positions)),
            engine: Arc::new(CountingEngine::default()),
        }
    }

    fn backend(&self) -> OwnedPortfolioPerformanceBackend {
        OwnedPortfolioPerformanceBackend::new(
            self.authority.clone(),
            self.performance.clone(),
            self.definitions.clone(),
            self.positions.clone(),
            self.engine.clone(),
        )
    }
}

fn calendar() -> Calendar {
    Calendar::new(CalendarInput {
        calendar_id: id(5),
        version: version(),
        owner: owner(),
        market: "CIBM".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: EffectivePeriod::new(close_time(19), close_time(24)).unwrap(),
        sessions: (20..=22)
            .map(|day| {
                CalendarSession::open(
                    NaiveDate::from_ymd_opt(2026, 8, day).unwrap(),
                    NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                    NaiveTime::from_hms_opt(15, 0, 0).unwrap(),
                )
                .unwrap()
            })
            .collect(),
    })
    .unwrap()
}

fn performance_convention(calendar: LineageRef) -> PortfolioPerformanceConvention {
    let mut input = PortfolioPerformanceConventionInput {
        convention: version_ref(28),
        owner: owner(),
        schema_id: "ficant.portfolio-performance-convention.v1".to_owned(),
        calendar,
        return_method: PortfolioPerformanceReturnMethod::DailyTimeWeighted,
        flow_timing: PortfolioExternalFlowTiming::EndOfDay,
        valuation_frequency: PortfolioValuationFrequency::CalendarSessionClose,
        rounding: PortfolioDecimalRounding::TiesToEven,
        effective_from: close_time(19),
        effective_to: close_time(24),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = PortfolioPerformanceConvention::content_hash_for(&input);
    PortfolioPerformanceConvention::new(input).unwrap()
}

fn metric_convention() -> PortfolioMetricConvention {
    let mut input = PortfolioMetricConventionInput {
        convention: version_ref(29),
        owner: owner(),
        schema_id: "ficant.portfolio-metric-convention.v1".to_owned(),
        ytm_weighting: PortfolioMetricWeighting::MarketValueTimesModifiedDuration,
        duration_weighting: PortfolioMetricWeighting::MarketValue,
        convexity_weighting: PortfolioMetricWeighting::MarketValue,
        coupon_weighting: PortfolioMetricWeighting::Notional,
        remaining_life_weighting: PortfolioMetricWeighting::Notional,
        rounding: PortfolioDecimalRounding::TiesToEven,
        freshness_limit_seconds: 86_400,
        effective_from: close_time(19),
        effective_to: close_time(24),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = PortfolioMetricConvention::content_hash_for(&input);
    PortfolioMetricConvention::new(input).unwrap()
}

fn portfolio(portfolio_id: usize, end_snapshot_id: usize) -> Portfolio {
    let end = position_snapshot(end_snapshot_id, 22);
    let mut input = PortfolioInput {
        portfolio: version_ref(portfolio_id),
        owner: owner(),
        subject_ref: version_ref(2),
        book: exact_ref(30),
        group: exact_ref(10),
        code: format!("P-{portfolio_id}"),
        display_name: format!("Portfolio {portfolio_id}"),
        status: PortfolioStatus::Active,
        position_snapshot: PortfolioSnapshotBinding::new(
            end.id().clone(),
            end.content_hash().clone(),
            end.observed_at().clone(),
            end.visible_at().clone(),
        )
        .unwrap(),
        benchmark: BenchmarkRef::new(version_ref(31), ContentHash::digest(b"temporary")),
        metric_convention: PortfolioMetricConventionRef::new(
            version_ref(29),
            metric_convention().content_hash().clone(),
        ),
        effective_from: close_time(19),
        effective_to: close_time(24),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    let benchmark = benchmark();
    input.benchmark = BenchmarkRef::new(
        benchmark.reference().clone(),
        benchmark.content_hash().clone(),
    );
    input.content_hash = Portfolio::content_hash_for(&input);
    Portfolio::new(input).unwrap()
}

fn benchmark() -> Benchmark {
    let snapshot = position_snapshot(16, 22);
    let mut input = BenchmarkInput {
        benchmark: version_ref(31),
        owner: owner(),
        subject_ref: version_ref(2),
        code: "CGB-BENCHMARK".to_owned(),
        display_name: "CGB Benchmark".to_owned(),
        position_snapshot: PortfolioSnapshotBinding::new(
            snapshot.id().clone(),
            snapshot.content_hash().clone(),
            snapshot.observed_at().clone(),
            snapshot.visible_at().clone(),
        )
        .unwrap(),
        effective_from: close_time(19),
        effective_to: close_time(24),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = Benchmark::content_hash_for(&input);
    Benchmark::new(input).unwrap()
}

fn catalog_evidence(
    portfolios: &[Portfolio; 2],
    benchmark: &Benchmark,
    metric: &PortfolioMetricConvention,
) -> Vec<PortfolioCatalogEvidenceBinding> {
    let mut values = vec![catalog_binding(
        PortfolioCatalogEvidenceRole::SelectedGroup,
        version_ref(10),
        exact_ref(10).content_hash().unwrap().clone(),
    )];
    values.extend(portfolios.iter().map(|portfolio| {
        catalog_binding(
            PortfolioCatalogEvidenceRole::MemberPortfolio,
            portfolio.reference().clone(),
            portfolio.content_hash().clone(),
        )
    }));
    values.push(catalog_binding(
        PortfolioCatalogEvidenceRole::Benchmark,
        benchmark.reference().clone(),
        benchmark.content_hash().clone(),
    ));
    values.push(catalog_binding(
        PortfolioCatalogEvidenceRole::MetricConvention,
        metric.reference().clone(),
        metric.content_hash().clone(),
    ));
    values.sort_by_key(|binding| (binding.role(), binding.reference().id().clone()));
    values
}

fn catalog_binding(
    role: PortfolioCatalogEvidenceRole,
    reference: VersionRef,
    hash: ContentHash,
) -> PortfolioCatalogEvidenceBinding {
    PortfolioCatalogEvidenceBinding::new(
        role,
        reference,
        hash,
        close_time(19),
        close_time(19),
        close_time(24),
    )
    .unwrap()
}

fn position_snapshot(snapshot_id: usize, day: u32) -> PositionSnapshot {
    let mut input = PositionSnapshotInput {
        snapshot_id: id(snapshot_id),
        owner: owner(),
        subject_ref: version_ref(2),
        observed_at: observed_time(day),
        visible_at: observed_time(day),
        content_hash: ContentHash::digest(b"placeholder"),
        lineage: vec![LineageRef::content_addressed(
            id(1),
            ContentHash::digest(b"position-source"),
        )],
        positions: Vec::new(),
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    PositionSnapshot::new(input).unwrap()
}

fn exact_portfolio(value: &Portfolio) -> LineageRef {
    LineageRef::new(
        value.reference().id().clone(),
        Some(value.reference().version()),
        Some(value.content_hash().clone()),
    )
    .unwrap()
}

fn exact_ref(value: usize) -> LineageRef {
    LineageRef::new(
        id(value),
        Some(version()),
        Some(ContentHash::digest(format!("exact-{value}").as_bytes())),
    )
    .unwrap()
}

fn amount(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value * 1_000_000_000_000)
}

fn scaled(value: &str) -> FixedDecimal {
    let (whole, fraction) = value.split_once('.').unwrap();
    FixedDecimal::from_scaled(
        whole.parse::<i128>().unwrap() * 1_000_000_000_000 + fraction.parse::<i128>().unwrap(),
    )
}

fn close_time(day: u32) -> MarketTime {
    market_time(day, 15, 0)
}

fn observed_time(day: u32) -> MarketTime {
    market_time(day, 14, 0)
}

fn visible_time(day: u32) -> MarketTime {
    market_time(day, 16, 0)
}

fn market_time(day: u32, hour: u32, minute: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 8, day, hour - 8, minute, 0)
            .unwrap(),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 8, day).unwrap(),
    )
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id(0), id(1))
}

fn version_ref(value: usize) -> VersionRef {
    VersionRef::new(id(value), version())
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(value: usize) -> Ulid {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let suffix = char::from(DIGITS[value]);
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn unused() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}
