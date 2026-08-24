use crate::primitives::{
    ContentHash, FixedDecimal, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, VersionRef,
};
use crate::{ContentAddressed, DomainErrorCode, DomainResult, VersionedDefinition};

use super::{
    PortfolioDecimalRounding, PortfolioSnapshotBinding, append, append_exact_ref,
    append_market_time, append_owner, append_version_ref, require_exact_ref,
    validate_effective_period, verify_content_hash,
};

pub const PORTFOLIO_PERFORMANCE_CONVENTION_SCHEMA_V1: &str =
    "ficant.portfolio-performance-convention.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioPerformanceReturnMethod {
    DailyTimeWeighted,
}

impl PortfolioPerformanceReturnMethod {
    const fn canonical_code(self) -> u8 {
        match self {
            Self::DailyTimeWeighted => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioExternalFlowTiming {
    EndOfDay,
}

impl PortfolioExternalFlowTiming {
    const fn canonical_code(self) -> u8 {
        match self {
            Self::EndOfDay => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioValuationFrequency {
    CalendarSessionClose,
}

impl PortfolioValuationFrequency {
    const fn canonical_code(self) -> u8 {
        match self {
            Self::CalendarSessionClose => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPerformanceConventionRef {
    reference: VersionRef,
    content_hash: ContentHash,
}

impl PortfolioPerformanceConventionRef {
    #[must_use]
    pub fn new(reference: VersionRef, content_hash: ContentHash) -> Self {
        Self {
            reference,
            content_hash,
        }
    }

    #[must_use]
    pub const fn reference(&self) -> &VersionRef {
        &self.reference
    }

    #[must_use]
    pub const fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPerformanceConvention {
    convention: VersionRef,
    owner: OwnerRef,
    schema_id: String,
    calendar: LineageRef,
    return_method: PortfolioPerformanceReturnMethod,
    flow_timing: PortfolioExternalFlowTiming,
    valuation_frequency: PortfolioValuationFrequency,
    rounding: PortfolioDecimalRounding,
    effective_from: MarketTime,
    effective_to: MarketTime,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPerformanceConventionInput {
    pub convention: VersionRef,
    pub owner: OwnerRef,
    pub schema_id: String,
    pub calendar: LineageRef,
    pub return_method: PortfolioPerformanceReturnMethod,
    pub flow_timing: PortfolioExternalFlowTiming,
    pub valuation_frequency: PortfolioValuationFrequency,
    pub rounding: PortfolioDecimalRounding,
    pub effective_from: MarketTime,
    pub effective_to: MarketTime,
    pub content_hash: ContentHash,
}

impl PortfolioPerformanceConvention {
    pub fn new(input: PortfolioPerformanceConventionInput) -> DomainResult<Self> {
        validate_effective_period(&input.effective_from, &input.effective_to)?;
        require_exact_ref(&input.calendar)?;
        if input.schema_id != PORTFOLIO_PERFORMANCE_CONVENTION_SCHEMA_V1
            || input.return_method != PortfolioPerformanceReturnMethod::DailyTimeWeighted
            || input.flow_timing != PortfolioExternalFlowTiming::EndOfDay
            || input.valuation_frequency != PortfolioValuationFrequency::CalendarSessionClose
            || input.rounding != PortfolioDecimalRounding::TiesToEven
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        verify_content_hash(
            &input.content_hash,
            &canonical_performance_convention(&input),
        )?;
        Ok(Self {
            convention: input.convention,
            owner: input.owner,
            schema_id: input.schema_id,
            calendar: input.calendar,
            return_method: input.return_method,
            flow_timing: input.flow_timing,
            valuation_frequency: input.valuation_frequency,
            rounding: input.rounding,
            effective_from: input.effective_from,
            effective_to: input.effective_to,
            content_hash: input.content_hash,
        })
    }

    #[must_use]
    pub fn content_hash_for(input: &PortfolioPerformanceConventionInput) -> ContentHash {
        ContentHash::digest(&canonical_performance_convention(input))
    }

    #[must_use]
    pub const fn reference(&self) -> &VersionRef {
        &self.convention
    }

    #[must_use]
    pub const fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub fn schema_id(&self) -> &str {
        &self.schema_id
    }

    #[must_use]
    pub const fn calendar(&self) -> &LineageRef {
        &self.calendar
    }

    #[must_use]
    pub const fn return_method(&self) -> PortfolioPerformanceReturnMethod {
        self.return_method
    }

    #[must_use]
    pub const fn flow_timing(&self) -> PortfolioExternalFlowTiming {
        self.flow_timing
    }

    #[must_use]
    pub const fn valuation_frequency(&self) -> PortfolioValuationFrequency {
        self.valuation_frequency
    }

    #[must_use]
    pub const fn rounding(&self) -> PortfolioDecimalRounding {
        self.rounding
    }

    #[must_use]
    pub const fn effective_from(&self) -> &MarketTime {
        &self.effective_from
    }

    #[must_use]
    pub const fn effective_to(&self) -> &MarketTime {
        &self.effective_to
    }
}

impl ContentAddressed for PortfolioPerformanceConvention {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl VersionedDefinition for PortfolioPerformanceConvention {
    fn identity(&self) -> &str {
        self.convention.id().as_str()
    }

    fn version(&self) -> u64 {
        self.convention.version().get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioValuationSnapshot {
    snapshot_id: Ulid,
    owner: OwnerRef,
    subject_ref: VersionRef,
    portfolio: LineageRef,
    position_snapshot: PortfolioSnapshotBinding,
    performance_convention: PortfolioPerformanceConventionRef,
    valuation_at: MarketTime,
    visible_at: MarketTime,
    currency_unit: UnitRef,
    gross_assets: FixedDecimal,
    liabilities: FixedDecimal,
    net_asset_value: FixedDecimal,
    net_external_flow: FixedDecimal,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioValuationSnapshotInput {
    pub snapshot_id: Ulid,
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub portfolio: LineageRef,
    pub position_snapshot: PortfolioSnapshotBinding,
    pub performance_convention: PortfolioPerformanceConventionRef,
    pub valuation_at: MarketTime,
    pub visible_at: MarketTime,
    pub currency_unit: UnitRef,
    pub gross_assets: FixedDecimal,
    pub liabilities: FixedDecimal,
    pub net_asset_value: FixedDecimal,
    pub net_external_flow: FixedDecimal,
    pub content_hash: ContentHash,
}

impl PortfolioValuationSnapshot {
    pub fn new(input: PortfolioValuationSnapshotInput) -> DomainResult<Self> {
        require_exact_ref(&input.portfolio)?;
        if input.visible_at.instant() < input.valuation_at.instant()
            || input.position_snapshot.observed_at().instant() > input.valuation_at.instant()
            || input.position_snapshot.visible_at().instant() > input.visible_at.instant()
            || !input.gross_assets.is_non_negative()
            || !input.liabilities.is_non_negative()
            || !input.net_asset_value.is_positive()
            || input.gross_assets.checked_sub(input.liabilities)? != input.net_asset_value
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        verify_content_hash(&input.content_hash, &canonical_valuation_snapshot(&input))?;
        Ok(Self {
            snapshot_id: input.snapshot_id,
            owner: input.owner,
            subject_ref: input.subject_ref,
            portfolio: input.portfolio,
            position_snapshot: input.position_snapshot,
            performance_convention: input.performance_convention,
            valuation_at: input.valuation_at,
            visible_at: input.visible_at,
            currency_unit: input.currency_unit,
            gross_assets: input.gross_assets,
            liabilities: input.liabilities,
            net_asset_value: input.net_asset_value,
            net_external_flow: input.net_external_flow,
            content_hash: input.content_hash,
        })
    }

    #[must_use]
    pub fn content_hash_for(input: &PortfolioValuationSnapshotInput) -> ContentHash {
        ContentHash::digest(&canonical_valuation_snapshot(input))
    }

    #[must_use]
    pub const fn snapshot_id(&self) -> &Ulid {
        &self.snapshot_id
    }

    #[must_use]
    pub const fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub const fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }

    #[must_use]
    pub const fn portfolio(&self) -> &LineageRef {
        &self.portfolio
    }

    #[must_use]
    pub const fn position_snapshot(&self) -> &PortfolioSnapshotBinding {
        &self.position_snapshot
    }

    #[must_use]
    pub const fn performance_convention(&self) -> &PortfolioPerformanceConventionRef {
        &self.performance_convention
    }

    #[must_use]
    pub const fn valuation_at(&self) -> &MarketTime {
        &self.valuation_at
    }

    #[must_use]
    pub const fn visible_at(&self) -> &MarketTime {
        &self.visible_at
    }

    #[must_use]
    pub const fn currency_unit(&self) -> &UnitRef {
        &self.currency_unit
    }

    #[must_use]
    pub const fn gross_assets(&self) -> FixedDecimal {
        self.gross_assets
    }

    #[must_use]
    pub const fn liabilities(&self) -> FixedDecimal {
        self.liabilities
    }

    #[must_use]
    pub const fn net_asset_value(&self) -> FixedDecimal {
        self.net_asset_value
    }

    #[must_use]
    pub const fn net_external_flow(&self) -> FixedDecimal {
        self.net_external_flow
    }
}

impl ContentAddressed for PortfolioValuationSnapshot {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchmarkLevelSnapshot {
    snapshot_id: Ulid,
    owner: OwnerRef,
    subject_ref: VersionRef,
    benchmark: LineageRef,
    valuation_at: MarketTime,
    visible_at: MarketTime,
    level_unit: UnitRef,
    level: FixedDecimal,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchmarkLevelSnapshotInput {
    pub snapshot_id: Ulid,
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub benchmark: LineageRef,
    pub valuation_at: MarketTime,
    pub visible_at: MarketTime,
    pub level_unit: UnitRef,
    pub level: FixedDecimal,
    pub content_hash: ContentHash,
}

impl BenchmarkLevelSnapshot {
    pub fn new(input: BenchmarkLevelSnapshotInput) -> DomainResult<Self> {
        require_exact_ref(&input.benchmark)?;
        if input.visible_at.instant() < input.valuation_at.instant() || !input.level.is_positive() {
            return Err(DomainErrorCode::InvalidValue);
        }
        verify_content_hash(&input.content_hash, &canonical_benchmark_level(&input))?;
        Ok(Self {
            snapshot_id: input.snapshot_id,
            owner: input.owner,
            subject_ref: input.subject_ref,
            benchmark: input.benchmark,
            valuation_at: input.valuation_at,
            visible_at: input.visible_at,
            level_unit: input.level_unit,
            level: input.level,
            content_hash: input.content_hash,
        })
    }

    #[must_use]
    pub fn content_hash_for(input: &BenchmarkLevelSnapshotInput) -> ContentHash {
        ContentHash::digest(&canonical_benchmark_level(input))
    }

    #[must_use]
    pub const fn snapshot_id(&self) -> &Ulid {
        &self.snapshot_id
    }

    #[must_use]
    pub const fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub const fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }

    #[must_use]
    pub const fn benchmark(&self) -> &LineageRef {
        &self.benchmark
    }

    #[must_use]
    pub const fn valuation_at(&self) -> &MarketTime {
        &self.valuation_at
    }

    #[must_use]
    pub const fn visible_at(&self) -> &MarketTime {
        &self.visible_at
    }

    #[must_use]
    pub const fn level_unit(&self) -> &UnitRef {
        &self.level_unit
    }

    #[must_use]
    pub const fn level(&self) -> FixedDecimal {
        self.level
    }
}

impl ContentAddressed for BenchmarkLevelSnapshot {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioSessionAggregate {
    valuation_at: MarketTime,
    opening_or_ending_nav: FixedDecimal,
    net_external_flow: FixedDecimal,
}

impl PortfolioSessionAggregate {
    pub fn new(
        valuation_at: MarketTime,
        net_asset_value: FixedDecimal,
        net_external_flow: FixedDecimal,
    ) -> DomainResult<Self> {
        if !net_asset_value.is_positive() {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            valuation_at,
            opening_or_ending_nav: net_asset_value,
            net_external_flow,
        })
    }

    #[must_use]
    pub const fn valuation_at(&self) -> &MarketTime {
        &self.valuation_at
    }

    #[must_use]
    pub const fn net_asset_value(&self) -> FixedDecimal {
        self.opening_or_ending_nav
    }

    #[must_use]
    pub const fn net_external_flow(&self) -> FixedDecimal {
        self.net_external_flow
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchmarkSessionLevel {
    valuation_at: MarketTime,
    level: FixedDecimal,
}

impl BenchmarkSessionLevel {
    pub fn new(valuation_at: MarketTime, level: FixedDecimal) -> DomainResult<Self> {
        if !level.is_positive() {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            valuation_at,
            level,
        })
    }

    #[must_use]
    pub const fn valuation_at(&self) -> &MarketTime {
        &self.valuation_at
    }

    #[must_use]
    pub const fn level(&self) -> FixedDecimal {
        self.level
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioDailyPerformancePoint {
    valuation_at: MarketTime,
    opening_nav: FixedDecimal,
    ending_nav: FixedDecimal,
    net_external_flow: FixedDecimal,
    economic_pnl: FixedDecimal,
    daily_return: FixedDecimal,
    benchmark_return: FixedDecimal,
    active_return: FixedDecimal,
    cumulative_return: FixedDecimal,
    benchmark_cumulative_return: FixedDecimal,
    active_cumulative_return: FixedDecimal,
}

impl PortfolioDailyPerformancePoint {
    #[must_use]
    pub const fn valuation_at(&self) -> &MarketTime {
        &self.valuation_at
    }

    #[must_use]
    pub const fn opening_nav(&self) -> FixedDecimal {
        self.opening_nav
    }

    #[must_use]
    pub const fn ending_nav(&self) -> FixedDecimal {
        self.ending_nav
    }

    #[must_use]
    pub const fn net_external_flow(&self) -> FixedDecimal {
        self.net_external_flow
    }

    #[must_use]
    pub const fn economic_pnl(&self) -> FixedDecimal {
        self.economic_pnl
    }

    #[must_use]
    pub const fn daily_return(&self) -> FixedDecimal {
        self.daily_return
    }

    #[must_use]
    pub const fn benchmark_return(&self) -> FixedDecimal {
        self.benchmark_return
    }

    #[must_use]
    pub const fn active_return(&self) -> FixedDecimal {
        self.active_return
    }

    #[must_use]
    pub const fn cumulative_return(&self) -> FixedDecimal {
        self.cumulative_return
    }

    #[must_use]
    pub const fn benchmark_cumulative_return(&self) -> FixedDecimal {
        self.benchmark_cumulative_return
    }

    #[must_use]
    pub const fn active_cumulative_return(&self) -> FixedDecimal {
        self.active_cumulative_return
    }
}

/// Calculates end-of-day-flow daily TWR and geometric cumulative returns.
///
/// Structural completeness, Calendar membership, owner and Unit checks are application
/// preconditions. This pure routine deliberately accepts only two already-aligned series.
pub fn calculate_daily_performance(
    portfolio: &[PortfolioSessionAggregate],
    benchmark: &[BenchmarkSessionLevel],
) -> DomainResult<Vec<PortfolioDailyPerformancePoint>> {
    if portfolio.len() < 2 || portfolio.len() != benchmark.len() {
        return Err(DomainErrorCode::InvalidValue);
    }
    for index in 0..portfolio.len() {
        if portfolio[index].valuation_at != benchmark[index].valuation_at
            || (index > 0
                && portfolio[index - 1].valuation_at.instant()
                    >= portfolio[index].valuation_at.instant())
        {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
    }

    let mut cumulative_factor = FixedDecimal::ONE;
    let mut benchmark_cumulative_factor = FixedDecimal::ONE;
    let mut points = Vec::with_capacity(portfolio.len() - 1);
    for index in 1..portfolio.len() {
        let opening_nav = portfolio[index - 1].net_asset_value();
        let ending_nav = portfolio[index].net_asset_value();
        let flow = portfolio[index].net_external_flow();
        let economic_pnl = ending_nav.checked_sub(flow)?.checked_sub(opening_nav)?;
        let daily_return = economic_pnl.checked_div_round_ties_even(opening_nav)?;
        let daily_factor = FixedDecimal::ONE.checked_add(daily_return)?;
        if !daily_factor.is_positive() {
            return Err(DomainErrorCode::InvalidValue);
        }

        let opening_level = benchmark[index - 1].level();
        let benchmark_pnl = benchmark[index].level().checked_sub(opening_level)?;
        let benchmark_return = benchmark_pnl.checked_div_round_ties_even(opening_level)?;
        let benchmark_factor = FixedDecimal::ONE.checked_add(benchmark_return)?;
        if !benchmark_factor.is_positive() {
            return Err(DomainErrorCode::InvalidValue);
        }

        cumulative_factor = cumulative_factor.checked_mul_round_ties_even(daily_factor)?;
        benchmark_cumulative_factor =
            benchmark_cumulative_factor.checked_mul_round_ties_even(benchmark_factor)?;
        let cumulative_return = cumulative_factor.checked_sub(FixedDecimal::ONE)?;
        let benchmark_cumulative_return =
            benchmark_cumulative_factor.checked_sub(FixedDecimal::ONE)?;
        points.push(PortfolioDailyPerformancePoint {
            valuation_at: portfolio[index].valuation_at.clone(),
            opening_nav,
            ending_nav,
            net_external_flow: flow,
            economic_pnl,
            daily_return,
            benchmark_return,
            active_return: daily_return.checked_sub(benchmark_return)?,
            cumulative_return,
            benchmark_cumulative_return,
            active_cumulative_return: cumulative_return.checked_sub(benchmark_cumulative_return)?,
        });
    }
    Ok(points)
}

fn canonical_performance_convention(input: &PortfolioPerformanceConventionInput) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_version_ref(&mut bytes, &input.convention);
    append_owner(&mut bytes, &input.owner);
    append(&mut bytes, input.schema_id.as_bytes());
    append_exact_ref(&mut bytes, &input.calendar);
    append(&mut bytes, &[input.return_method.canonical_code()]);
    append(&mut bytes, &[input.flow_timing.canonical_code()]);
    append(&mut bytes, &[input.valuation_frequency.canonical_code()]);
    append(&mut bytes, &[input.rounding.canonical_code()]);
    append_market_time(&mut bytes, &input.effective_from);
    append_market_time(&mut bytes, &input.effective_to);
    bytes
}

fn canonical_valuation_snapshot(input: &PortfolioValuationSnapshotInput) -> Vec<u8> {
    let mut bytes = Vec::new();
    append(&mut bytes, input.snapshot_id.as_str().as_bytes());
    append_owner(&mut bytes, &input.owner);
    append_version_ref(&mut bytes, &input.subject_ref);
    append_exact_ref(&mut bytes, &input.portfolio);
    append(
        &mut bytes,
        input.position_snapshot.snapshot_id().as_str().as_bytes(),
    );
    append(
        &mut bytes,
        input.position_snapshot.content_hash().as_bytes(),
    );
    append_market_time(&mut bytes, input.position_snapshot.observed_at());
    append_market_time(&mut bytes, input.position_snapshot.visible_at());
    append_version_ref(&mut bytes, input.performance_convention.reference());
    append(
        &mut bytes,
        input.performance_convention.content_hash().as_bytes(),
    );
    append_market_time(&mut bytes, &input.valuation_at);
    append_market_time(&mut bytes, &input.visible_at);
    append_unit(&mut bytes, &input.currency_unit);
    for value in [
        input.gross_assets,
        input.liabilities,
        input.net_asset_value,
        input.net_external_flow,
    ] {
        append(&mut bytes, &value.scaled().to_be_bytes());
    }
    bytes
}

fn canonical_benchmark_level(input: &BenchmarkLevelSnapshotInput) -> Vec<u8> {
    let mut bytes = Vec::new();
    append(&mut bytes, input.snapshot_id.as_str().as_bytes());
    append_owner(&mut bytes, &input.owner);
    append_version_ref(&mut bytes, &input.subject_ref);
    append_exact_ref(&mut bytes, &input.benchmark);
    append_market_time(&mut bytes, &input.valuation_at);
    append_market_time(&mut bytes, &input.visible_at);
    append_unit(&mut bytes, &input.level_unit);
    append(&mut bytes, &input.level.scaled().to_be_bytes());
    bytes
}

fn append_unit(bytes: &mut Vec<u8>, value: &UnitRef) {
    append(bytes, value.unit_id().as_str().as_bytes());
    append(bytes, &value.version().get().to_be_bytes());
}
