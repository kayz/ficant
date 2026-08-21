use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use ficant_domain::ContentAddressed;
use ficant_domain::analytics::BondAnalyticsResult;
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::PriceSourceType;
use ficant_domain::portfolio::{
    BenchmarkRef, Portfolio, PortfolioMetricConventionRef, PortfolioSnapshotBinding,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, FixedDecimal, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version,
};
use ficant_domain::research::{
    CoverageDeclaration, FactorDv01, PortfolioKeyRateExposure, Position, PositionKeyRateExposure,
    PositionSnapshot, PriceSourceCount, PriceSourceSummary,
};
use ficant_runtime::{
    CodeBinding, FormalImplementationBinding, FormalInputBinding, FormalInputBindingInput,
    FormalInputKind, FormalInputReference, FormalOutputEvidence, FormalOutputEvidenceInput,
    NamedContentRef, RuntimeBinding,
};

use crate::ports::{
    AccessScope, AeadCursorCodec, ApplicationResult, AuthorizedPrincipal, BondAnalyticsEngine,
    CanonicalSnapshotDecoder, CurvePointSetDecoder, CurveSnapshotMetadataRepository,
    DataSourceRepository, DefinitionRepository, FactorTopologyRepository, FormalOutputRecord,
    FormalOutputRepository, FuturesDeliveryEngine, FuturesDeliveryRuleParser, IntegrityEventSink,
    NormalizedPortfolioContext, OperationFingerprint, PortfolioAnalyticsAuthorityRepository,
    PortfolioBondRatesAuthorityResolution, PortfolioCatalogRepository, PortfolioRatesUnitAuthority,
    PortfolioRatesUnitRole as AuthorityUnitRole, PortfolioRiskAuthority,
    PositionSnapshotRepository, ResolvedPortfolioAggregationInputs,
    ResolvedPortfolioAnalyticsAuthority, SafeTraceContext, SnapshotVerifiedReadMetadataRepository,
    SubjectRepository, TaxRulePackParser, VerifiedBlobReader, YieldCurveEngine,
};
use crate::use_cases::bond_analytics::CalculateBondAnalytics;
use crate::use_cases::formal_outputs::FormalOutputUseCase;
use crate::use_cases::portfolio_catalog::{
    ListPortfolioCatalog, ResolvePortfolioAnalyticsAuthority,
};
use crate::use_cases::portfolio_risk::{
    CalculateBondKeyRateDv01, CalculateBondKeyRateDv01Command, PortfolioRiskInputKind,
};
use crate::use_cases::position_views::{PositionViews, project_verified_position_views};
use crate::use_cases::rates_materialization::{
    BondRatesCommand, MaterializeBondRatesInput, RatesEvidenceBinding, RatesInputRole,
    RatesRequestEvidence, RatesUnitRequirement,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

const FIXED_DECIMAL_SCALE: u32 = 12;
const MAX_DECIMAL_SCALE: u32 = 28;
pub const PORTFOLIO_OVERVIEW_SCHEMA_ID: &str = "ficant.portfolio.v1.PortfolioOverview";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortfolioRatesUnitRole {
    CurrencyAmount,
    PricePer100,
    Rate,
    Years,
    YearsSquared,
    Dv01Per100,
    Dv01,
    Dimensionless,
    ContractCount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioRatesUnitBindings {
    currency_amount: RatesUnitRequirement,
    price_per_100: RatesUnitRequirement,
    rate: RatesUnitRequirement,
    years: RatesUnitRequirement,
    years_squared: RatesUnitRequirement,
    dv01_per_100: RatesUnitRequirement,
    dv01: RatesUnitRequirement,
    dimensionless: RatesUnitRequirement,
    contract_count: RatesUnitRequirement,
}

impl PortfolioRatesUnitBindings {
    /// Freezes one exact Unit requirement for every existing Rates response role.
    ///
    /// # Errors
    ///
    /// Fails closed unless all nine roles occur exactly once with their canonical dimensions.
    pub fn new(
        mut values: Vec<(PortfolioRatesUnitRole, RatesUnitRequirement)>,
    ) -> ApplicationResult<Self> {
        values.sort_by_key(|(role, _)| *role);
        if values.len() != 9 || values.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(invalid_unit());
        }
        let mut take = |role, dimension| {
            let index = values
                .iter()
                .position(|(candidate, _)| *candidate == role)
                .ok_or_else(invalid_unit)?;
            let (_, value) = values.remove(index);
            if value.expected_dimension() != dimension {
                return Err(invalid_unit());
            }
            Ok(value)
        };
        Ok(Self {
            currency_amount: take(PortfolioRatesUnitRole::CurrencyAmount, "currency_amount")?,
            price_per_100: take(PortfolioRatesUnitRole::PricePer100, "price_per_100")?,
            rate: take(PortfolioRatesUnitRole::Rate, "rate")?,
            years: take(PortfolioRatesUnitRole::Years, "years")?,
            years_squared: take(PortfolioRatesUnitRole::YearsSquared, "years_squared")?,
            dv01_per_100: take(PortfolioRatesUnitRole::Dv01Per100, "dv01_per_100")?,
            dv01: take(PortfolioRatesUnitRole::Dv01, "dv01")?,
            dimensionless: take(PortfolioRatesUnitRole::Dimensionless, "dimensionless")?,
            contract_count: take(PortfolioRatesUnitRole::ContractCount, "contract_count")?,
        })
    }

    #[must_use]
    pub const fn currency_amount(&self) -> &RatesUnitRequirement {
        &self.currency_amount
    }
    #[must_use]
    pub const fn price_per_100(&self) -> &RatesUnitRequirement {
        &self.price_per_100
    }
    #[must_use]
    pub const fn rate(&self) -> &RatesUnitRequirement {
        &self.rate
    }
    #[must_use]
    pub const fn years(&self) -> &RatesUnitRequirement {
        &self.years
    }
    #[must_use]
    pub const fn years_squared(&self) -> &RatesUnitRequirement {
        &self.years_squared
    }
    #[must_use]
    pub const fn dv01_per_100(&self) -> &RatesUnitRequirement {
        &self.dv01_per_100
    }
    #[must_use]
    pub const fn dv01(&self) -> &RatesUnitRequirement {
        &self.dv01
    }
    #[must_use]
    pub const fn dimensionless(&self) -> &RatesUnitRequirement {
        &self.dimensionless
    }
    #[must_use]
    pub const fn contract_count(&self) -> &RatesUnitRequirement {
        &self.contract_count
    }

    #[must_use]
    pub fn requirements(&self) -> Vec<RatesUnitRequirement> {
        vec![
            self.currency_amount.clone(),
            self.price_per_100.clone(),
            self.rate.clone(),
            self.years.clone(),
            self.years_squared.clone(),
            self.dv01_per_100.clone(),
            self.dv01.clone(),
            self.dimensionless.clone(),
            self.contract_count.clone(),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioBondResultMetadata {
    schema_id: String,
    engine_id: String,
    engine_version: String,
    algorithm_id: String,
    algorithm_version: u32,
    convention_profile: String,
    abi_version: u32,
    subject_ref: ficant_domain::primitives::VersionRef,
    request_evidence: RatesRequestEvidence,
    formal_evidence: Option<FormalOutputEvidence>,
}

impl PortfolioBondResultMetadata {
    #[must_use]
    pub fn schema_id(&self) -> &str {
        &self.schema_id
    }
    #[must_use]
    pub fn engine_id(&self) -> &str {
        &self.engine_id
    }
    #[must_use]
    pub fn engine_version(&self) -> &str {
        &self.engine_version
    }
    #[must_use]
    pub fn algorithm_id(&self) -> &str {
        &self.algorithm_id
    }
    #[must_use]
    pub const fn algorithm_version(&self) -> u32 {
        self.algorithm_version
    }
    #[must_use]
    pub fn convention_profile(&self) -> &str {
        &self.convention_profile
    }
    #[must_use]
    pub const fn abi_version(&self) -> u32 {
        self.abi_version
    }
    #[must_use]
    pub const fn subject_ref(&self) -> &ficant_domain::primitives::VersionRef {
        &self.subject_ref
    }
    #[must_use]
    pub const fn request_evidence(&self) -> &RatesRequestEvidence {
        &self.request_evidence
    }
    #[must_use]
    pub const fn formal_evidence(&self) -> Option<&FormalOutputEvidence> {
        self.formal_evidence.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioBondAnalysisResult {
    analytics: BondAnalyticsResult,
    units: PortfolioRatesUnitBindings,
    metadata: PortfolioBondResultMetadata,
}

impl PortfolioBondAnalysisResult {
    /// Captures the complete wire-neutral metadata of one already verified R5D result.
    ///
    /// # Errors
    ///
    /// Fails closed unless the evidence contains the exact Subject consumed by the result owner.
    pub fn from_verified(
        analytics: BondAnalyticsResult,
        units: PortfolioRatesUnitBindings,
        subject_ref: ficant_domain::primitives::VersionRef,
        request_evidence: RatesRequestEvidence,
    ) -> ApplicationResult<Self> {
        let subject_matches = request_evidence.consumed_inputs().iter().any(|input| {
            input.role() == RatesInputRole::Subject
                && input.owner() == analytics.input().owner()
                && matches!(
                    input.binding(),
                    RatesEvidenceBinding::Object(reference)
                        if reference.version_ref() == &subject_ref
                )
        });
        if !subject_matches {
            return Err(integrity());
        }
        let metadata = PortfolioBondResultMetadata {
            schema_id: analytics.schema_id().to_owned(),
            engine_id: analytics.engine_id().to_owned(),
            engine_version: analytics.engine_version().to_owned(),
            algorithm_id: analytics.algorithm_id().to_owned(),
            algorithm_version: analytics.algorithm_version(),
            convention_profile: analytics.convention_profile().to_owned(),
            abi_version: analytics.abi_version(),
            subject_ref,
            request_evidence,
            formal_evidence: None,
        };
        Ok(Self {
            analytics,
            units,
            metadata,
        })
    }

    #[must_use]
    pub const fn analytics(&self) -> &BondAnalyticsResult {
        &self.analytics
    }
    #[must_use]
    pub const fn units(&self) -> &PortfolioRatesUnitBindings {
        &self.units
    }
    #[must_use]
    pub const fn metadata(&self) -> &PortfolioBondResultMetadata {
        &self.metadata
    }

    /// P04 is deliberately the existing pre-tax R5D analysis; R5E owns tax-adjusted analytics.
    #[must_use]
    pub const fn after_tax(&self) -> Option<()> {
        None
    }

    #[must_use]
    pub fn canonical_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        append(&mut bytes, b"ficant.portfolio.bond-analysis-result.v1");
        append_bond_analysis(&mut bytes, self);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortfolioMetricOutputScales {
    money: u32,
    rate: u32,
    years: u32,
    years_squared: u32,
    dv01: u32,
}

impl PortfolioMetricOutputScales {
    /// Freezes the exact output scale of every metric Unit before numerical handoff.
    ///
    /// # Errors
    ///
    /// Returns validation failure when any scale exceeds the `DecimalValue` boundary.
    pub fn new(
        money: u32,
        rate: u32,
        years: u32,
        years_squared: u32,
        dv01: u32,
    ) -> ApplicationResult<Self> {
        if [money, rate, years, years_squared, dv01]
            .into_iter()
            .any(|scale| scale > MAX_DECIMAL_SCALE)
        {
            return Err(validation());
        }
        Ok(Self {
            money,
            rate,
            years,
            years_squared,
            dv01,
        })
    }

    #[must_use]
    pub const fn money(self) -> u32 {
        self.money
    }

    #[must_use]
    pub const fn rate(self) -> u32 {
        self.rate
    }

    #[must_use]
    pub const fn years(self) -> u32 {
        self.years
    }

    #[must_use]
    pub const fn years_squared(self) -> u32 {
        self.years_squared
    }

    #[must_use]
    pub const fn dv01(self) -> u32 {
        self.dv01
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioMetricDataMode {
    Real,
    Partial,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortfolioCoverageReason {
    ShortPositionExcludedFromWeightedAverages,
    NonBondExcludedFromWeightedAverages,
    MissingBondMetricExcludedFromWeightedAverages,
    PositionExcludedFromPortfolioRisk,
    BenchmarkPositionExcludedFromPortfolioRisk,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioCoverage {
    participation: CoverageDeclaration,
    missing_reasons: Vec<PortfolioCoverageReason>,
}

impl PortfolioCoverage {
    #[must_use]
    pub const fn participation(&self) -> &CoverageDeclaration {
        &self.participation
    }

    #[must_use]
    pub fn missing_reasons(&self) -> &[PortfolioCoverageReason] {
        &self.missing_reasons
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioMetricCoverage {
    imported_position_count: u64,
    weighted_average_participating_position_count: u64,
    missing_reasons: Vec<PortfolioCoverageReason>,
}

impl PortfolioMetricCoverage {
    #[must_use]
    pub const fn imported_position_count(&self) -> u64 {
        self.imported_position_count
    }

    #[must_use]
    pub const fn weighted_average_participating_position_count(&self) -> u64 {
        self.weighted_average_participating_position_count
    }

    #[must_use]
    pub fn missing_reasons(&self) -> &[PortfolioCoverageReason] {
        &self.missing_reasons
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioWeightedMetricUnits {
    ytm: UnitRef,
    duration: UnitRef,
    convexity: UnitRef,
    coupon_rate: UnitRef,
    remaining_years: UnitRef,
}

impl PortfolioWeightedMetricUnits {
    #[must_use]
    pub fn new(
        ytm: UnitRef,
        duration: UnitRef,
        convexity: UnitRef,
        coupon_rate: UnitRef,
        remaining_years: UnitRef,
    ) -> Self {
        Self {
            ytm,
            duration,
            convexity,
            coupon_rate,
            remaining_years,
        }
    }

    #[must_use]
    pub fn ytm(&self) -> &UnitRef {
        &self.ytm
    }

    #[must_use]
    pub fn duration(&self) -> &UnitRef {
        &self.duration
    }

    #[must_use]
    pub fn convexity(&self) -> &UnitRef {
        &self.convexity
    }

    #[must_use]
    pub fn coupon_rate(&self) -> &UnitRef {
        &self.coupon_rate
    }

    #[must_use]
    pub fn remaining_years(&self) -> &UnitRef {
        &self.remaining_years
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioBondMetricFacts {
    yield_to_maturity: FixedDecimal,
    modified_duration: FixedDecimal,
    convexity: FixedDecimal,
    coupon_rate: FixedDecimal,
    remaining_years: FixedDecimal,
    units: PortfolioWeightedMetricUnits,
}

impl PortfolioBondMetricFacts {
    /// Constructs the point-in-time measures returned by the exact `AnalyzeBond` path.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless every approved weighted measure is positive, except the
    /// coupon rate which may be zero.
    pub fn new(
        yield_to_maturity: FixedDecimal,
        modified_duration: FixedDecimal,
        convexity: FixedDecimal,
        coupon_rate: FixedDecimal,
        remaining_years: FixedDecimal,
        units: PortfolioWeightedMetricUnits,
    ) -> ApplicationResult<Self> {
        if !yield_to_maturity.is_positive()
            || !modified_duration.is_positive()
            || !convexity.is_positive()
            || !coupon_rate.is_non_negative()
            || !remaining_years.is_positive()
        {
            return Err(validation());
        }
        Ok(Self {
            yield_to_maturity,
            modified_duration,
            convexity,
            coupon_rate,
            remaining_years,
            units,
        })
    }

    #[must_use]
    pub const fn yield_to_maturity(&self) -> FixedDecimal {
        self.yield_to_maturity
    }

    #[must_use]
    pub const fn modified_duration(&self) -> FixedDecimal {
        self.modified_duration
    }

    #[must_use]
    pub const fn convexity(&self) -> FixedDecimal {
        self.convexity
    }

    #[must_use]
    pub const fn coupon_rate(&self) -> FixedDecimal {
        self.coupon_rate
    }

    #[must_use]
    pub const fn remaining_years(&self) -> FixedDecimal {
        self.remaining_years
    }

    #[must_use]
    pub fn units(&self) -> &PortfolioWeightedMetricUnits {
        &self.units
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortfolioWeightedMetricEligibility {
    Bond(Box<PortfolioBondMetricFacts>),
    NonBond,
    MissingBondMetric,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioMetricPosition {
    position_id: Ulid,
    market_value: DecimalValue,
    economic_pnl: DecimalValue,
    signed_notional: FixedDecimal,
    weighted_metric_eligibility: PortfolioWeightedMetricEligibility,
}

impl PortfolioMetricPosition {
    /// Binds already imported signed totals to the exact per-bond analysis result.
    ///
    /// # Errors
    ///
    /// Returns validation failure for mixed money units or an unrepresentable decimal.
    pub fn from_totals(
        position_id: Ulid,
        market_value: DecimalValue,
        economic_pnl: DecimalValue,
        signed_notional: FixedDecimal,
        weighted_metric_eligibility: PortfolioWeightedMetricEligibility,
    ) -> ApplicationResult<Self> {
        if market_value.unit() != economic_pnl.unit() {
            return Err(invalid_unit());
        }
        decimal_to_fixed(&market_value)?;
        decimal_to_fixed(&economic_pnl)?;
        Ok(Self {
            position_id,
            market_value,
            economic_pnl,
            signed_notional,
            weighted_metric_eligibility,
        })
    }

    /// Scales per-quantity imported facts without converting through binary floating point.
    ///
    /// This constructor exists for deterministic fixtures and import adapters; the aggregation
    /// itself consumes the signed totals stored in this value.
    ///
    /// # Errors
    ///
    /// Returns validation failure on overflow or a value that cannot be represented at the
    /// canonical fixed-decimal scale.
    #[allow(clippy::too_many_arguments)]
    pub fn from_per_quantity(
        position_id: Ulid,
        quantity: FixedDecimal,
        notional_per_quantity: FixedDecimal,
        market_value_per_quantity: FixedDecimal,
        economic_pnl_per_quantity: FixedDecimal,
        money_unit: UnitRef,
        weighted_metric_eligibility: PortfolioWeightedMetricEligibility,
    ) -> ApplicationResult<Self> {
        let signed_notional = quantity
            .checked_mul(notional_per_quantity)
            .map_err(map_domain_error)?;
        let market_value = fixed_to_decimal(
            quantity
                .checked_mul(market_value_per_quantity)
                .map_err(map_domain_error)?,
            FIXED_DECIMAL_SCALE,
            money_unit.clone(),
        )?;
        let economic_pnl = fixed_to_decimal(
            quantity
                .checked_mul(economic_pnl_per_quantity)
                .map_err(map_domain_error)?,
            FIXED_DECIMAL_SCALE,
            money_unit,
        )?;
        Self::from_totals(
            position_id,
            market_value,
            economic_pnl,
            signed_notional,
            weighted_metric_eligibility,
        )
    }

    #[must_use]
    pub fn position_id(&self) -> &Ulid {
        &self.position_id
    }

    #[must_use]
    pub fn market_value(&self) -> &DecimalValue {
        &self.market_value
    }

    #[must_use]
    pub fn economic_pnl(&self) -> &DecimalValue {
        &self.economic_pnl
    }

    #[must_use]
    pub const fn signed_notional(&self) -> FixedDecimal {
        self.signed_notional
    }

    #[must_use]
    pub fn weighted_metric_eligibility(&self) -> &PortfolioWeightedMetricEligibility {
        &self.weighted_metric_eligibility
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioBasicMetrics {
    market_value: DecimalValue,
    economic_pnl: DecimalValue,
    weighted_ytm: Option<DecimalValue>,
    modified_duration: Option<DecimalValue>,
    convexity: Option<DecimalValue>,
    weighted_coupon_rate: Option<DecimalValue>,
    weighted_remaining_years: Option<DecimalValue>,
    dv01: DecimalValue,
}

impl PortfolioBasicMetrics {
    #[must_use]
    pub fn market_value(&self) -> &DecimalValue {
        &self.market_value
    }

    #[must_use]
    pub fn economic_pnl(&self) -> &DecimalValue {
        &self.economic_pnl
    }

    #[must_use]
    pub fn weighted_ytm(&self) -> Option<&DecimalValue> {
        self.weighted_ytm.as_ref()
    }

    #[must_use]
    pub fn modified_duration(&self) -> Option<&DecimalValue> {
        self.modified_duration.as_ref()
    }

    #[must_use]
    pub fn convexity(&self) -> Option<&DecimalValue> {
        self.convexity.as_ref()
    }

    #[must_use]
    pub fn weighted_coupon_rate(&self) -> Option<&DecimalValue> {
        self.weighted_coupon_rate.as_ref()
    }

    #[must_use]
    pub fn weighted_remaining_years(&self) -> Option<&DecimalValue> {
        self.weighted_remaining_years.as_ref()
    }

    #[must_use]
    pub fn dv01(&self) -> &DecimalValue {
        &self.dv01
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioKrdSummary {
    totals: Vec<FactorDv01>,
    parallel_dv01: DecimalValue,
}

impl PortfolioKrdSummary {
    #[must_use]
    pub fn totals(&self) -> &[FactorDv01] {
        &self.totals
    }

    #[must_use]
    pub fn parallel_dv01(&self) -> &DecimalValue {
        &self.parallel_dv01
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioMetricAggregation {
    data_mode: PortfolioMetricDataMode,
    basic_metrics: PortfolioBasicMetrics,
    krd_summary: PortfolioKrdSummary,
    coverage: PortfolioMetricCoverage,
}

impl PortfolioMetricAggregation {
    #[must_use]
    pub const fn data_mode(&self) -> PortfolioMetricDataMode {
        self.data_mode
    }

    #[must_use]
    pub fn basic_metrics(&self) -> &PortfolioBasicMetrics {
        &self.basic_metrics
    }

    #[must_use]
    pub fn krd_summary(&self) -> &PortfolioKrdSummary {
        &self.krd_summary
    }

    #[must_use]
    pub fn coverage(&self) -> &PortfolioMetricCoverage {
        &self.coverage
    }
}

/// Applies the frozen R8A point-in-time convention to verified position totals and the existing
/// `PortfolioRisk` result.
///
/// # Errors
///
/// Fails closed on an empty or duplicate position set, mixed units, invalid KRD topology,
/// overflow, zero denominators, or values that cannot be represented without binary floating
/// point. KRD values are consumed from the existing risk seam and are never recalculated here.
pub fn aggregate_portfolio_metrics(
    positions: &[PortfolioMetricPosition],
    krd_totals: &[FactorDv01],
    output_scales: PortfolioMetricOutputScales,
) -> ApplicationResult<PortfolioMetricAggregation> {
    validate_positions(positions)?;
    let money_unit = positions[0].market_value.unit().clone();
    let market_value = sum_money(
        positions.iter().map(PortfolioMetricPosition::market_value),
        output_scales.money(),
        &money_unit,
    )?;
    let economic_pnl = sum_money(
        positions.iter().map(PortfolioMetricPosition::economic_pnl),
        output_scales.money(),
        &money_unit,
    )?;
    let (krd_summary, dv01) = krd_summary(krd_totals, output_scales.dv01())?;

    let mut reasons = BTreeSet::new();
    let mut participating = Vec::new();
    for position in positions {
        if !position.signed_notional.is_positive() {
            reasons.insert(PortfolioCoverageReason::ShortPositionExcludedFromWeightedAverages);
            continue;
        }
        match &position.weighted_metric_eligibility {
            PortfolioWeightedMetricEligibility::Bond(metrics) => {
                if !decimal_to_fixed(position.market_value())?.is_positive() {
                    return Err(validation());
                }
                participating.push((position, metrics.as_ref()));
            }
            PortfolioWeightedMetricEligibility::NonBond => {
                reasons.insert(PortfolioCoverageReason::NonBondExcludedFromWeightedAverages);
            }
            PortfolioWeightedMetricEligibility::MissingBondMetric => {
                reasons
                    .insert(PortfolioCoverageReason::MissingBondMetricExcludedFromWeightedAverages);
            }
        }
    }

    let (
        weighted_ytm,
        modified_duration,
        convexity,
        weighted_coupon_rate,
        weighted_remaining_years,
    ) = if reasons.is_empty() {
        let averages = weighted_averages(&participating, output_scales)?;
        (
            Some(averages.0),
            Some(averages.1),
            Some(averages.2),
            Some(averages.3),
            Some(averages.4),
        )
    } else {
        (None, None, None, None, None)
    };
    let data_mode = if reasons.is_empty() {
        PortfolioMetricDataMode::Real
    } else {
        PortfolioMetricDataMode::Partial
    };
    let coverage = PortfolioMetricCoverage {
        imported_position_count: u64::try_from(positions.len()).map_err(|_| validation())?,
        weighted_average_participating_position_count: u64::try_from(participating.len())
            .map_err(|_| validation())?,
        missing_reasons: reasons.into_iter().collect(),
    };
    Ok(PortfolioMetricAggregation {
        data_mode,
        basic_metrics: PortfolioBasicMetrics {
            market_value,
            economic_pnl,
            weighted_ytm,
            modified_duration,
            convexity,
            weighted_coupon_rate,
            weighted_remaining_years,
            dv01,
        },
        krd_summary,
        coverage,
    })
}

fn validate_positions(positions: &[PortfolioMetricPosition]) -> ApplicationResult<()> {
    if positions.is_empty()
        || positions
            .windows(2)
            .any(|pair| pair[0].position_id >= pair[1].position_id)
    {
        return Err(validation());
    }
    let unit = positions[0].market_value.unit();
    if positions.iter().any(|position| {
        position.market_value.unit() != unit || position.economic_pnl.unit() != unit
    }) {
        return Err(invalid_unit());
    }
    Ok(())
}

fn sum_money<'a>(
    mut values: impl Iterator<Item = &'a DecimalValue>,
    output_scale: u32,
    unit: &UnitRef,
) -> ApplicationResult<DecimalValue> {
    let first = values.next().ok_or_else(validation)?.clone();
    let total = values.try_fold(first, |total, value| {
        total.checked_add(value).map_err(map_domain_error)
    })?;
    fixed_to_decimal(decimal_to_fixed(&total)?, output_scale, unit.clone())
}

type WeightedAverages = (
    DecimalValue,
    DecimalValue,
    DecimalValue,
    DecimalValue,
    DecimalValue,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RawWeightedAverage {
    numerator: i128,
    denominator: i128,
}

impl RawWeightedAverage {
    fn new(raw_weight: i128, value: FixedDecimal) -> ApplicationResult<Self> {
        let mut average = Self {
            numerator: 0,
            denominator: 0,
        };
        average.add(raw_weight, value)?;
        Ok(average)
    }

    fn add(&mut self, raw_weight: i128, value: FixedDecimal) -> ApplicationResult<()> {
        if raw_weight <= 0 || value.scaled() < 0 {
            return Err(validation());
        }
        let weighted_value = raw_weight
            .checked_mul(value.scaled())
            .ok_or_else(validation)?;
        self.numerator = self
            .numerator
            .checked_add(weighted_value)
            .ok_or_else(validation)?;
        self.denominator = self
            .denominator
            .checked_add(raw_weight)
            .ok_or_else(validation)?;
        Ok(())
    }

    fn into_decimal(self, output_scale: u32, unit: UnitRef) -> ApplicationResult<DecimalValue> {
        rational_weighted_average_to_decimal(self.numerator, self.denominator, output_scale, unit)
    }
}

#[derive(Clone, Copy, Debug)]
struct RawWeightedPosition<'a> {
    market_value: i128,
    notional: i128,
    ytm_weight: i128,
    metrics: &'a PortfolioBondMetricFacts,
}

fn weighted_averages(
    participating: &[(&PortfolioMetricPosition, &PortfolioBondMetricFacts)],
    output_scales: PortfolioMetricOutputScales,
) -> ApplicationResult<WeightedAverages> {
    let (_, first_metrics) = participating.first().ok_or_else(validation)?;
    let units = first_metrics.units.clone();
    let weighted_positions = participating
        .iter()
        .map(|(position, metrics)| {
            if metrics.units != units {
                return Err(invalid_unit());
            }
            let market_value = decimal_to_fixed(position.market_value())?.scaled();
            let notional = position.signed_notional.scaled();
            Ok(RawWeightedPosition {
                market_value,
                notional,
                ytm_weight: checked_raw_product(market_value, metrics.modified_duration.scaled())?,
                metrics,
            })
        })
        .collect::<ApplicationResult<Vec<_>>>()?;
    let market_value_divisor = common_positive_divisor(
        weighted_positions
            .iter()
            .map(|position| position.market_value),
    )?;
    let notional_divisor =
        common_positive_divisor(weighted_positions.iter().map(|position| position.notional))?;
    let ytm_weight_divisor = common_positive_divisor(
        weighted_positions
            .iter()
            .map(|position| position.ytm_weight),
    )?;
    let first = weighted_positions.first().ok_or_else(validation)?;
    let mut ytm = RawWeightedAverage::new(
        first.ytm_weight / ytm_weight_divisor,
        first.metrics.yield_to_maturity,
    )?;
    let mut duration = RawWeightedAverage::new(
        first.market_value / market_value_divisor,
        first.metrics.modified_duration,
    )?;
    let mut convexity = RawWeightedAverage::new(
        first.market_value / market_value_divisor,
        first.metrics.convexity,
    )?;
    let mut coupon =
        RawWeightedAverage::new(first.notional / notional_divisor, first.metrics.coupon_rate)?;
    let mut remaining = RawWeightedAverage::new(
        first.notional / notional_divisor,
        first.metrics.remaining_years,
    )?;

    for position in &weighted_positions[1..] {
        let metrics = position.metrics;
        if metrics.units != units {
            return Err(invalid_unit());
        }
        ytm.add(
            position.ytm_weight / ytm_weight_divisor,
            metrics.yield_to_maturity,
        )?;
        duration.add(
            position.market_value / market_value_divisor,
            metrics.modified_duration,
        )?;
        convexity.add(
            position.market_value / market_value_divisor,
            metrics.convexity,
        )?;
        coupon.add(position.notional / notional_divisor, metrics.coupon_rate)?;
        remaining.add(
            position.notional / notional_divisor,
            metrics.remaining_years,
        )?;
    }
    Ok((
        ytm.into_decimal(output_scales.rate(), units.ytm)?,
        duration.into_decimal(output_scales.years(), units.duration)?,
        convexity.into_decimal(output_scales.years_squared(), units.convexity)?,
        coupon.into_decimal(output_scales.rate(), units.coupon_rate)?,
        remaining.into_decimal(output_scales.years(), units.remaining_years)?,
    ))
}

fn checked_raw_product(left: i128, right: i128) -> ApplicationResult<i128> {
    if left <= 0 || right <= 0 {
        return Err(validation());
    }
    left.checked_mul(right).ok_or_else(validation)
}

fn common_positive_divisor(mut values: impl Iterator<Item = i128>) -> ApplicationResult<i128> {
    let first = values.next().ok_or_else(validation)?;
    if first <= 0 {
        return Err(validation());
    }
    values.try_fold(first, |divisor, value| {
        if value <= 0 {
            return Err(validation());
        }
        Ok(positive_greatest_common_divisor(divisor, value))
    })
}

fn positive_greatest_common_divisor(mut left: i128, mut right: i128) -> i128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn rational_weighted_average_to_decimal(
    numerator: i128,
    denominator: i128,
    output_scale: u32,
    unit: UnitRef,
) -> ApplicationResult<DecimalValue> {
    if numerator < 0 || denominator <= 0 || output_scale > MAX_DECIMAL_SCALE {
        return Err(validation());
    }
    let coefficient = if output_scale <= FIXED_DECIMAL_SCALE {
        let scale_divisor = checked_power_of_ten(FIXED_DECIMAL_SCALE - output_scale)?;
        let final_denominator = denominator
            .checked_mul(scale_divisor)
            .ok_or_else(validation)?;
        round_div_ties_even(numerator, final_denominator)?
    } else {
        expand_rational_scale(numerator, denominator, output_scale - FIXED_DECIMAL_SCALE)?
    };
    DecimalValue::new(coefficient.to_string(), output_scale, unit).map_err(map_domain_error)
}

fn expand_rational_scale(
    numerator: i128,
    denominator: i128,
    additional_scale: u32,
) -> ApplicationResult<i128> {
    let mut coefficient = numerator.checked_div(denominator).ok_or_else(validation)?;
    let mut remainder = numerator.checked_rem(denominator).ok_or_else(validation)?;
    for _ in 0..additional_scale {
        coefficient = coefficient.checked_mul(10).ok_or_else(validation)?;
        let expanded_remainder = remainder.checked_mul(10).ok_or_else(validation)?;
        coefficient = coefficient
            .checked_add(expanded_remainder / denominator)
            .ok_or_else(validation)?;
        remainder = expanded_remainder % denominator;
    }
    let distance_to_denominator = denominator.checked_sub(remainder).ok_or_else(validation)?;
    if remainder > distance_to_denominator
        || (remainder == distance_to_denominator && coefficient % 2 == 1)
    {
        coefficient = coefficient.checked_add(1).ok_or_else(validation)?;
    }
    Ok(coefficient)
}

fn krd_summary(
    totals: &[FactorDv01],
    output_scale: u32,
) -> ApplicationResult<(PortfolioKrdSummary, DecimalValue)> {
    let first = totals.first().ok_or_else(validation)?;
    let mut factor_ids = BTreeSet::new();
    if totals
        .iter()
        .any(|factor| factor.unit() != first.unit() || !factor_ids.insert(factor.factor_id()))
    {
        return Err(lineage());
    }
    let parallel = totals[1..].iter().try_fold(first.value(), |sum, factor| {
        sum.checked_add(factor.value()).map_err(map_domain_error)
    })?;
    let parallel_dv01 = fixed_to_decimal(parallel, output_scale, first.unit().clone())?;
    Ok((
        PortfolioKrdSummary {
            totals: totals.to_vec(),
            parallel_dv01: parallel_dv01.clone(),
        },
        parallel_dv01,
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioMemberOverview {
    portfolio: LineageRef,
    position_snapshot: PortfolioSnapshotBinding,
    basic_metrics: PortfolioBasicMetrics,
    krd_summary: PortfolioKrdSummary,
    position_views: PositionViews,
    key_rate_exposure: PortfolioKeyRateExposure,
    risk_inputs: Vec<PortfolioRiskNamedEvidenceBinding>,
    rates_evidence: Vec<RatesRequestEvidence>,
    bond_analyses: Vec<PortfolioMemberBondAnalysis>,
    analytics_authority_evidence: Vec<crate::ports::PortfolioAnalyticsEvidenceBinding>,
    analytics_authority_fingerprint: ContentHash,
}

impl PortfolioMemberOverview {
    #[must_use]
    pub fn portfolio(&self) -> &LineageRef {
        &self.portfolio
    }

    #[must_use]
    pub fn position_snapshot(&self) -> &PortfolioSnapshotBinding {
        &self.position_snapshot
    }

    #[must_use]
    pub fn basic_metrics(&self) -> &PortfolioBasicMetrics {
        &self.basic_metrics
    }

    #[must_use]
    pub fn krd_summary(&self) -> &PortfolioKrdSummary {
        &self.krd_summary
    }

    #[must_use]
    pub fn position_views(&self) -> &PositionViews {
        &self.position_views
    }

    #[must_use]
    pub fn key_rate_exposure(&self) -> &PortfolioKeyRateExposure {
        &self.key_rate_exposure
    }

    #[must_use]
    pub fn risk_inputs(&self) -> &[PortfolioRiskNamedEvidenceBinding] {
        &self.risk_inputs
    }

    #[must_use]
    pub fn rates_evidence(&self) -> &[RatesRequestEvidence] {
        &self.rates_evidence
    }

    #[must_use]
    pub fn bond_analyses(&self) -> &[PortfolioMemberBondAnalysis] {
        &self.bond_analyses
    }

    #[must_use]
    pub fn analytics_authority_evidence(
        &self,
    ) -> &[crate::ports::PortfolioAnalyticsEvidenceBinding] {
        &self.analytics_authority_evidence
    }

    #[must_use]
    pub fn analytics_authority_fingerprint(&self) -> &ContentHash {
        &self.analytics_authority_fingerprint
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioMemberBondAnalysis {
    position_id: Ulid,
    instrument_ref: ficant_domain::primitives::VersionRef,
    valuation: crate::ports::PortfolioValuationAuthorityBinding,
    analysis: PortfolioBondAnalysisResult,
}

impl PortfolioMemberBondAnalysis {
    #[must_use]
    pub const fn position_id(&self) -> &Ulid {
        &self.position_id
    }
    #[must_use]
    pub const fn instrument_ref(&self) -> &ficant_domain::primitives::VersionRef {
        &self.instrument_ref
    }
    #[must_use]
    pub const fn valuation(&self) -> &crate::ports::PortfolioValuationAuthorityBinding {
        &self.valuation
    }
    #[must_use]
    pub const fn analysis(&self) -> &PortfolioBondAnalysisResult {
        &self.analysis
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioOverviewDraft {
    subject_ref: ficant_domain::primitives::VersionRef,
    scope: crate::ports::ExactPortfolioScope,
    catalog_evidence: Vec<crate::ports::PortfolioCatalogEvidenceBinding>,
    position_snapshots: Vec<PortfolioSnapshotBinding>,
    basic_metrics: PortfolioBasicMetrics,
    krd_summary: PortfolioKrdSummary,
    benchmark_metrics: PortfolioBasicMetrics,
    benchmark: BenchmarkRef,
    metric_convention: PortfolioMetricConventionRef,
    coverage: PortfolioCoverage,
    members: Vec<PortfolioMemberOverview>,
    benchmark_key_rate_exposure: PortfolioKeyRateExposure,
    benchmark_risk_inputs: Vec<PortfolioRiskNamedEvidenceBinding>,
    benchmark_bond_analyses: Vec<PortfolioMemberBondAnalysis>,
    benchmark_rates_evidence: Vec<RatesRequestEvidence>,
    benchmark_analytics_authority_evidence: Vec<crate::ports::PortfolioAnalyticsEvidenceBinding>,
    benchmark_analytics_authority_fingerprint: ContentHash,
    request_fingerprint: ContentHash,
    data_mode: PortfolioMetricDataMode,
}

impl PortfolioOverviewDraft {
    #[must_use]
    pub const fn subject_ref(&self) -> &ficant_domain::primitives::VersionRef {
        &self.subject_ref
    }

    #[must_use]
    pub fn scope(&self) -> &crate::ports::ExactPortfolioScope {
        &self.scope
    }

    #[must_use]
    pub fn catalog_evidence(&self) -> &[crate::ports::PortfolioCatalogEvidenceBinding] {
        &self.catalog_evidence
    }

    #[must_use]
    pub fn position_snapshots(&self) -> &[PortfolioSnapshotBinding] {
        &self.position_snapshots
    }

    #[must_use]
    pub fn basic_metrics(&self) -> &PortfolioBasicMetrics {
        &self.basic_metrics
    }

    #[must_use]
    pub fn krd_summary(&self) -> &PortfolioKrdSummary {
        &self.krd_summary
    }

    #[must_use]
    pub fn benchmark_metrics(&self) -> &PortfolioBasicMetrics {
        &self.benchmark_metrics
    }

    #[must_use]
    pub fn benchmark(&self) -> &BenchmarkRef {
        &self.benchmark
    }

    #[must_use]
    pub fn metric_convention(&self) -> &PortfolioMetricConventionRef {
        &self.metric_convention
    }

    #[must_use]
    pub fn coverage(&self) -> &PortfolioCoverage {
        &self.coverage
    }

    #[must_use]
    pub fn members(&self) -> &[PortfolioMemberOverview] {
        &self.members
    }

    #[must_use]
    pub fn benchmark_rates_evidence(&self) -> &[RatesRequestEvidence] {
        &self.benchmark_rates_evidence
    }

    #[must_use]
    pub fn benchmark_risk_inputs(&self) -> &[PortfolioRiskNamedEvidenceBinding] {
        &self.benchmark_risk_inputs
    }

    /// Derives the complete implementation set from only the verified calculation results.
    ///
    /// # Errors
    ///
    /// Fails closed if an implementation role is absent, duplicated, or cannot be canonicalized.
    pub fn implementation_bindings(&self) -> ApplicationResult<Vec<FormalImplementationBinding>> {
        portfolio_implementation_bindings(self)
    }

    /// Derives every formal input from verified scope, authority, KRD, and Rates evidence.
    ///
    /// # Errors
    ///
    /// Fails closed if an input is incomplete, duplicated, or disagrees with the exact Subject.
    pub fn formal_input_bindings(
        &self,
        owner: &OwnerRef,
        subject_hash: &ContentHash,
    ) -> ApplicationResult<Vec<FormalInputBinding>> {
        portfolio_formal_inputs(owner, self, subject_hash)
    }

    #[must_use]
    pub fn benchmark_analytics_authority_evidence(
        &self,
    ) -> &[crate::ports::PortfolioAnalyticsEvidenceBinding] {
        &self.benchmark_analytics_authority_evidence
    }

    #[must_use]
    pub fn request_fingerprint(&self) -> &ContentHash {
        &self.request_fingerprint
    }

    #[must_use]
    pub const fn data_mode(&self) -> PortfolioMetricDataMode {
        self.data_mode
    }

    #[must_use]
    pub fn canonical_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        append(&mut bytes, PORTFOLIO_OVERVIEW_SCHEMA_ID.as_bytes());
        append(&mut bytes, self.subject_ref.id().as_str().as_bytes());
        append(&mut bytes, &self.subject_ref.version().get().to_be_bytes());
        append_scope(&mut bytes, &self.scope);
        append_catalog_evidence(&mut bytes, &self.catalog_evidence);
        for snapshot in &self.position_snapshots {
            append_snapshot_binding(&mut bytes, snapshot);
        }
        append_basic_metrics(&mut bytes, &self.basic_metrics);
        append_krd(&mut bytes, &self.krd_summary);
        append_basic_metrics(&mut bytes, &self.benchmark_metrics);
        append_version_ref(
            &mut bytes,
            self.benchmark.reference().id().as_str().as_bytes(),
            self.benchmark.reference().version().get(),
            self.benchmark.content_hash(),
        );
        append_version_ref(
            &mut bytes,
            self.metric_convention.reference().id().as_str().as_bytes(),
            self.metric_convention.reference().version().get(),
            self.metric_convention.content_hash(),
        );
        append_coverage(&mut bytes, &self.coverage);
        for member in &self.members {
            append_lineage(&mut bytes, &member.portfolio);
            append_snapshot_binding(&mut bytes, &member.position_snapshot);
            append_basic_metrics(&mut bytes, &member.basic_metrics);
            append_krd(&mut bytes, &member.krd_summary);
            append_risk_named_evidence(&mut bytes, &member.risk_inputs);
            for evidence in &member.rates_evidence {
                append(&mut bytes, evidence.request_fingerprint().as_bytes());
            }
            for analysis in &member.bond_analyses {
                append(&mut bytes, analysis.position_id.as_str().as_bytes());
                append(&mut bytes, analysis.instrument_ref.id().as_str().as_bytes());
                append(
                    &mut bytes,
                    &analysis.instrument_ref.version().get().to_be_bytes(),
                );
                append(
                    &mut bytes,
                    analysis.valuation.valuation_id.as_str().as_bytes(),
                );
                append(
                    &mut bytes,
                    &analysis.valuation.source_revision.to_be_bytes(),
                );
                append(&mut bytes, analysis.valuation.content_hash.as_bytes());
                append(&mut bytes, &analysis.valuation.value_index.to_be_bytes());
                append_bond_analysis(&mut bytes, &analysis.analysis);
            }
            append(
                &mut bytes,
                member.analytics_authority_fingerprint.as_bytes(),
            );
            append_analytics_authority_evidence(&mut bytes, &member.analytics_authority_evidence);
        }
        for evidence in &self.benchmark_rates_evidence {
            append(&mut bytes, evidence.request_fingerprint().as_bytes());
        }
        append_risk_named_evidence(&mut bytes, &self.benchmark_risk_inputs);
        append(
            &mut bytes,
            self.benchmark_analytics_authority_fingerprint.as_bytes(),
        );
        append_analytics_authority_evidence(
            &mut bytes,
            &self.benchmark_analytics_authority_evidence,
        );
        append(&mut bytes, self.request_fingerprint.as_bytes());
        append(&mut bytes, &[metric_mode_code(self.data_mode)]);
        bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioOverview {
    draft: PortfolioOverviewDraft,
    formal_evidence: FormalOutputEvidence,
}

impl PortfolioOverview {
    #[must_use]
    pub fn draft(&self) -> &PortfolioOverviewDraft {
        &self.draft
    }

    #[must_use]
    pub fn formal_evidence(&self) -> &FormalOutputEvidence {
        &self.formal_evidence
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioBondAnalysis {
    signed_notional: FixedDecimal,
    eligibility: PortfolioWeightedMetricEligibility,
    rates_evidence: Option<RatesRequestEvidence>,
    analysis_result: Option<PortfolioBondAnalysisResult>,
}

impl PortfolioBondAnalysis {
    #[must_use]
    pub fn bond(
        signed_notional: FixedDecimal,
        metrics: PortfolioBondMetricFacts,
        analysis_result: PortfolioBondAnalysisResult,
    ) -> Self {
        let rates_evidence = analysis_result.metadata.request_evidence.clone();
        Self {
            signed_notional,
            eligibility: PortfolioWeightedMetricEligibility::Bond(Box::new(metrics)),
            rates_evidence: Some(rates_evidence),
            analysis_result: Some(analysis_result),
        }
    }

    #[must_use]
    pub const fn non_bond(signed_notional: FixedDecimal) -> Self {
        Self {
            signed_notional,
            eligibility: PortfolioWeightedMetricEligibility::NonBond,
            rates_evidence: None,
            analysis_result: None,
        }
    }

    #[must_use]
    pub const fn missing_bond_metric(signed_notional: FixedDecimal) -> Self {
        Self {
            signed_notional,
            eligibility: PortfolioWeightedMetricEligibility::MissingBondMetric,
            rates_evidence: None,
            analysis_result: None,
        }
    }

    #[must_use]
    pub const fn signed_notional(&self) -> FixedDecimal {
        self.signed_notional
    }

    #[must_use]
    pub fn eligibility(&self) -> &PortfolioWeightedMetricEligibility {
        &self.eligibility
    }

    #[must_use]
    pub fn rates_evidence(&self) -> Option<&RatesRequestEvidence> {
        self.rates_evidence.as_ref()
    }

    #[must_use]
    pub fn analysis_result(&self) -> Option<&PortfolioBondAnalysisResult> {
        self.analysis_result.as_ref()
    }
}

#[async_trait]
pub trait PortfolioAggregationAuthority: Send + Sync {
    /// Re-resolves the caller-supplied normalized context into exact catalog authorities.
    async fn resolve_aggregation_inputs(
        &self,
        principal: &AuthorizedPrincipal,
        context: &NormalizedPortfolioContext,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs>;

    /// Re-resolves and compares the exact catalog evidence captured during normalization.
    async fn resolve_aggregation_inputs_with_evidence(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &crate::ports::NormalizedPortfolioContextResolution,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs> {
        let resolved = self
            .resolve_aggregation_inputs(principal, resolution.context())
            .await?;
        if resolved.catalog_evidence.as_slice() != resolution.catalog_evidence() {
            return Err(integrity());
        }
        Ok(resolved)
    }
}

#[async_trait]
impl PortfolioAggregationAuthority for ListPortfolioCatalog<'_> {
    async fn resolve_aggregation_inputs(
        &self,
        principal: &AuthorizedPrincipal,
        context: &NormalizedPortfolioContext,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs> {
        ListPortfolioCatalog::resolve_aggregation_inputs(self, principal.access_scope(), context)
            .await
    }

    async fn resolve_aggregation_inputs_with_evidence(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &crate::ports::NormalizedPortfolioContextResolution,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs> {
        ListPortfolioCatalog::resolve_aggregation_inputs_with_evidence(
            self,
            principal.access_scope(),
            resolution,
        )
        .await
    }
}

#[derive(Clone)]
pub struct OwnedPortfolioCatalogAggregationAuthority {
    repository: Arc<dyn PortfolioCatalogRepository>,
    cursor_codec: Arc<AeadCursorCodec>,
}

impl OwnedPortfolioCatalogAggregationAuthority {
    #[must_use]
    pub fn new(
        repository: Arc<dyn PortfolioCatalogRepository>,
        cursor_codec: Arc<AeadCursorCodec>,
    ) -> Self {
        Self {
            repository,
            cursor_codec,
        }
    }
}

#[async_trait]
impl PortfolioAggregationAuthority for OwnedPortfolioCatalogAggregationAuthority {
    async fn resolve_aggregation_inputs(
        &self,
        principal: &AuthorizedPrincipal,
        context: &NormalizedPortfolioContext,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs> {
        ListPortfolioCatalog::new(self.repository.as_ref(), self.cursor_codec.as_ref())
            .resolve_aggregation_inputs(principal.access_scope(), context)
            .await
    }

    async fn resolve_aggregation_inputs_with_evidence(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &crate::ports::NormalizedPortfolioContextResolution,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs> {
        ListPortfolioCatalog::new(self.repository.as_ref(), self.cursor_codec.as_ref())
            .resolve_aggregation_inputs_with_evidence(principal.access_scope(), resolution)
            .await
    }
}

#[async_trait]
pub trait PortfolioAnalyticsAuthorityHandoff: Send + Sync {
    /// Required-reads the one exact analytics authority set for a verified `PositionSnapshot`.
    async fn resolve(
        &self,
        principal: &AuthorizedPrincipal,
        context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
    ) -> ApplicationResult<ResolvedPortfolioAnalyticsAuthority>;
}

#[async_trait]
impl PortfolioAnalyticsAuthorityHandoff for ResolvePortfolioAnalyticsAuthority<'_> {
    async fn resolve(
        &self,
        principal: &AuthorizedPrincipal,
        context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
    ) -> ApplicationResult<ResolvedPortfolioAnalyticsAuthority> {
        self.execute(principal.access_scope(), context, snapshot)
            .await
    }
}

#[derive(Clone)]
pub struct OwnedPortfolioAnalyticsAuthorityHandoff {
    authority: Arc<dyn PortfolioAnalyticsAuthorityRepository>,
    definitions: Arc<dyn DefinitionRepository>,
    curves: Arc<dyn CurveSnapshotMetadataRepository>,
    snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
    blobs: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
}

impl OwnedPortfolioAnalyticsAuthorityHandoff {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        authority: Arc<dyn PortfolioAnalyticsAuthorityRepository>,
        definitions: Arc<dyn DefinitionRepository>,
        curves: Arc<dyn CurveSnapshotMetadataRepository>,
        snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
        blobs: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
    ) -> Self {
        Self {
            authority,
            definitions,
            curves,
            snapshots,
            blobs,
            integrity_events,
        }
    }
}

#[async_trait]
impl PortfolioAnalyticsAuthorityHandoff for OwnedPortfolioAnalyticsAuthorityHandoff {
    async fn resolve(
        &self,
        principal: &AuthorizedPrincipal,
        context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
    ) -> ApplicationResult<ResolvedPortfolioAnalyticsAuthority> {
        ResolvePortfolioAnalyticsAuthority::new(
            self.authority.as_ref(),
            self.definitions.as_ref(),
            self.curves.as_ref(),
            self.snapshots.as_ref(),
            self.blobs.as_ref(),
            self.integrity_events.as_ref(),
        )
        .execute(principal.access_scope(), context, snapshot)
        .await
    }
}

pub trait PortfolioPositionViewsHandoff: Send + Sync {
    /// Projects an already required-read snapshot through the existing `PositionViews` seam.
    ///
    /// # Errors
    ///
    /// Returns an integrity or validation failure when the verified snapshot cannot be projected.
    fn project(&self, snapshot: PositionSnapshot) -> ApplicationResult<PositionViews>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ExistingPositionViewsHandoff;

impl PortfolioPositionViewsHandoff for ExistingPositionViewsHandoff {
    fn project(&self, snapshot: PositionSnapshot) -> ApplicationResult<PositionViews> {
        project_verified_position_views(snapshot)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortfolioRiskNamedEvidenceKind {
    FactorDefinition,
    CurveNodeDefinition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioRiskNamedEvidenceBinding {
    kind: PortfolioRiskNamedEvidenceKind,
    identity: String,
    content_hash: ContentHash,
    observed_at: Option<MarketTime>,
    visible_at: Option<MarketTime>,
    effective_from: Option<MarketTime>,
    effective_to: Option<MarketTime>,
}

impl PortfolioRiskNamedEvidenceBinding {
    /// Captures one exact immutable named definition consumed by verified KRD execution.
    ///
    /// # Errors
    ///
    /// Fails closed when the definition identity is not canonical.
    pub fn immutable_definition(
        kind: PortfolioRiskNamedEvidenceKind,
        identity: impl Into<String>,
        content_hash: ContentHash,
    ) -> ApplicationResult<Self> {
        let identity = identity.into();
        NamedContentRef::new(identity.clone(), content_hash.clone()).map_err(map_domain_error)?;
        Ok(Self {
            kind,
            identity,
            content_hash,
            observed_at: None,
            visible_at: None,
            effective_from: None,
            effective_to: None,
        })
    }

    #[must_use]
    pub const fn kind(&self) -> PortfolioRiskNamedEvidenceKind {
        self.kind
    }

    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
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
pub struct PortfolioRiskAnalysis {
    exposure: PortfolioKeyRateExposure,
    actual_inputs: Vec<PortfolioRiskNamedEvidenceBinding>,
}

impl PortfolioRiskAnalysis {
    /// Binds a verified KRD result to every Factor and `CurveNode` definition it consumed.
    ///
    /// # Errors
    ///
    /// Fails closed on missing, extra, duplicate, or hash-drifted definition evidence.
    pub fn new(
        exposure: PortfolioKeyRateExposure,
        mut actual_inputs: Vec<PortfolioRiskNamedEvidenceBinding>,
    ) -> ApplicationResult<Self> {
        actual_inputs.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.identity.cmp(&right.identity))
        });
        if exposure.totals().is_empty()
            || actual_inputs
                .windows(2)
                .any(|pair| pair[0].kind == pair[1].kind && pair[0].identity == pair[1].identity)
        {
            return Err(lineage());
        }
        let factors = actual_inputs
            .iter()
            .filter(|input| input.kind == PortfolioRiskNamedEvidenceKind::FactorDefinition)
            .collect::<Vec<_>>();
        let node_count = actual_inputs
            .iter()
            .filter(|input| input.kind == PortfolioRiskNamedEvidenceKind::CurveNodeDefinition)
            .count();
        let input_hashes = exposure
            .positions()
            .iter()
            .flat_map(PositionKeyRateExposure::input_evidence_hashes)
            .collect::<BTreeSet<_>>();
        if factors.len() != exposure.totals().len()
            || node_count != exposure.totals().len()
            || exposure.totals().iter().any(|factor| {
                !factors.iter().any(|input| {
                    input.identity == factor.factor_id()
                        && input.content_hash == *factor.factor_definition_hash()
                })
            })
            || actual_inputs.iter().any(|input| {
                input.kind == PortfolioRiskNamedEvidenceKind::CurveNodeDefinition
                    && !input_hashes.contains(&input.content_hash)
            })
        {
            return Err(lineage());
        }
        Ok(Self {
            exposure,
            actual_inputs,
        })
    }

    #[must_use]
    pub const fn exposure(&self) -> &PortfolioKeyRateExposure {
        &self.exposure
    }

    #[must_use]
    pub fn actual_inputs(&self) -> &[PortfolioRiskNamedEvidenceBinding] {
        &self.actual_inputs
    }

    fn into_parts(
        self,
    ) -> (
        PortfolioKeyRateExposure,
        Vec<PortfolioRiskNamedEvidenceBinding>,
    ) {
        (self.exposure, self.actual_inputs)
    }
}

#[async_trait]
pub trait PortfolioRiskHandoff: Send + Sync {
    /// Calls the existing `CalculateKeyRateDv01` path for the exact verified snapshot.
    async fn calculate(
        &self,
        scope: &AccessScope,
        context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
        authority: &PortfolioRiskAuthority,
    ) -> ApplicationResult<PortfolioRiskAnalysis>;
}

pub struct ExistingPortfolioRiskHandoff<'a, 'b> {
    calculator: &'a CalculateBondKeyRateDv01<'b>,
}

impl<'a, 'b> ExistingPortfolioRiskHandoff<'a, 'b> {
    #[must_use]
    pub const fn new(calculator: &'a CalculateBondKeyRateDv01<'b>) -> Self {
        Self { calculator }
    }
}

#[async_trait]
impl PortfolioRiskHandoff for ExistingPortfolioRiskHandoff<'_, '_> {
    async fn calculate(
        &self,
        scope: &AccessScope,
        context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
        authority: &PortfolioRiskAuthority,
    ) -> ApplicationResult<PortfolioRiskAnalysis> {
        let command = match &authority.futures_data_snapshot_id {
            Some(futures) => CalculateBondKeyRateDv01Command::new_with_futures_data_snapshot(
                snapshot.id().clone(),
                context.knowledge_at.clone(),
                context.valuation_at.clone(),
                authority.curve_snapshot_id.clone(),
                authority.dv01_unit.clone(),
                futures.clone(),
            )?,
            None => CalculateBondKeyRateDv01Command::new(
                snapshot.id().clone(),
                context.knowledge_at.clone(),
                context.valuation_at.clone(),
                authority.curve_snapshot_id.clone(),
                authority.dv01_unit.clone(),
            )?,
        };
        let execution = self
            .calculator
            .execute_with_evidence(scope, command)
            .await?;
        let (exposure, inputs) = execution.into_parts();
        let inputs = inputs
            .into_iter()
            .map(|input| {
                PortfolioRiskNamedEvidenceBinding::immutable_definition(
                    match input.kind() {
                        PortfolioRiskInputKind::FactorDefinition => {
                            PortfolioRiskNamedEvidenceKind::FactorDefinition
                        }
                        PortfolioRiskInputKind::CurveNodeDefinition => {
                            PortfolioRiskNamedEvidenceKind::CurveNodeDefinition
                        }
                    },
                    input.identity(),
                    input.content_hash().clone(),
                )
            })
            .collect::<ApplicationResult<Vec<_>>>()?;
        PortfolioRiskAnalysis::new(exposure, inputs)
    }
}

#[derive(Clone)]
pub struct OwnedPortfolioRiskFuturesDependencies {
    snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
    decoder: Arc<dyn CanonicalSnapshotDecoder>,
    data_sources: Arc<dyn DataSourceRepository>,
    rule_parser: Arc<dyn FuturesDeliveryRuleParser>,
    engine: Arc<dyn FuturesDeliveryEngine>,
}

impl OwnedPortfolioRiskFuturesDependencies {
    #[must_use]
    pub fn new(
        snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
        decoder: Arc<dyn CanonicalSnapshotDecoder>,
        data_sources: Arc<dyn DataSourceRepository>,
        rule_parser: Arc<dyn FuturesDeliveryRuleParser>,
        engine: Arc<dyn FuturesDeliveryEngine>,
    ) -> Self {
        Self {
            snapshots,
            decoder,
            data_sources,
            rule_parser,
            engine,
        }
    }
}

#[derive(Clone)]
pub struct OwnedExistingPortfolioRiskHandoff {
    positions: Arc<dyn PositionSnapshotRepository>,
    curves: Arc<dyn CurveSnapshotMetadataRepository>,
    definitions: Arc<dyn DefinitionRepository>,
    factors: Arc<dyn FactorTopologyRepository>,
    blobs: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
    decoder: Arc<dyn CurvePointSetDecoder>,
    curve_engine: Arc<dyn YieldCurveEngine>,
    bond_engine: Arc<dyn BondAnalyticsEngine>,
    futures: Option<OwnedPortfolioRiskFuturesDependencies>,
}

impl OwnedExistingPortfolioRiskHandoff {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        positions: Arc<dyn PositionSnapshotRepository>,
        curves: Arc<dyn CurveSnapshotMetadataRepository>,
        definitions: Arc<dyn DefinitionRepository>,
        factors: Arc<dyn FactorTopologyRepository>,
        blobs: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
        decoder: Arc<dyn CurvePointSetDecoder>,
        curve_engine: Arc<dyn YieldCurveEngine>,
        bond_engine: Arc<dyn BondAnalyticsEngine>,
        futures: Option<OwnedPortfolioRiskFuturesDependencies>,
    ) -> Self {
        Self {
            positions,
            curves,
            definitions,
            factors,
            blobs,
            integrity_events,
            decoder,
            curve_engine,
            bond_engine,
            futures,
        }
    }
}

#[async_trait]
impl PortfolioRiskHandoff for OwnedExistingPortfolioRiskHandoff {
    async fn calculate(
        &self,
        scope: &AccessScope,
        context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
        authority: &PortfolioRiskAuthority,
    ) -> ApplicationResult<PortfolioRiskAnalysis> {
        if authority.futures_data_snapshot_id.is_some() && self.futures.is_none() {
            return Err(integrity());
        }
        if let Some(futures) = &self.futures {
            let calculator = CalculateBondKeyRateDv01::new_with_futures(
                self.positions.as_ref(),
                self.curves.as_ref(),
                self.definitions.as_ref(),
                self.factors.as_ref(),
                self.blobs.as_ref(),
                self.integrity_events.as_ref(),
                self.decoder.as_ref(),
                self.curve_engine.as_ref(),
                self.bond_engine.as_ref(),
                futures.snapshots.as_ref(),
                futures.decoder.as_ref(),
                futures.data_sources.as_ref(),
                futures.rule_parser.as_ref(),
                futures.engine.as_ref(),
            );
            ExistingPortfolioRiskHandoff::new(&calculator)
                .calculate(scope, context, snapshot, authority)
                .await
        } else {
            let calculator = CalculateBondKeyRateDv01::new(
                self.positions.as_ref(),
                self.curves.as_ref(),
                self.definitions.as_ref(),
                self.factors.as_ref(),
                self.blobs.as_ref(),
                self.integrity_events.as_ref(),
                self.decoder.as_ref(),
                self.curve_engine.as_ref(),
                self.bond_engine.as_ref(),
            );
            ExistingPortfolioRiskHandoff::new(&calculator)
                .calculate(scope, context, snapshot, authority)
                .await
        }
    }
}

#[async_trait]
pub trait PortfolioBondAnalysisHandoff: Send + Sync {
    /// Calls the existing R5D exact materialization and `AnalyzeBond` engine path.
    async fn analyze(
        &self,
        scope: &AccessScope,
        context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
        position: &ficant_domain::research::Position,
        authority: &PortfolioBondRatesAuthorityResolution,
        authority_fingerprint: &OperationFingerprint,
    ) -> ApplicationResult<PortfolioBondAnalysis>;
}

pub struct ExactPortfolioBondAnalysisHandoff<'a, 'b> {
    materializer: &'a MaterializeBondRatesInput<'b>,
    engine: &'a dyn BondAnalyticsEngine,
}

impl<'a, 'b> ExactPortfolioBondAnalysisHandoff<'a, 'b> {
    #[must_use]
    pub const fn new(
        materializer: &'a MaterializeBondRatesInput<'b>,
        engine: &'a dyn BondAnalyticsEngine,
    ) -> Self {
        Self {
            materializer,
            engine,
        }
    }
}

#[async_trait]
impl PortfolioBondAnalysisHandoff for ExactPortfolioBondAnalysisHandoff<'_, '_> {
    async fn analyze(
        &self,
        scope: &AccessScope,
        context: &NormalizedPortfolioContext,
        _snapshot: &PositionSnapshot,
        position: &ficant_domain::research::Position,
        authority: &PortfolioBondRatesAuthorityResolution,
        authority_fingerprint: &OperationFingerprint,
    ) -> ApplicationResult<PortfolioBondAnalysis> {
        let signed_notional = decimal_to_fixed(position.quantity())?;
        match authority {
            PortfolioBondRatesAuthorityResolution::NonBond {
                position_id,
                instrument_ref,
            } => {
                validate_position_authority(position, position_id, instrument_ref)?;
                Ok(PortfolioBondAnalysis::non_bond(signed_notional))
            }
            PortfolioBondRatesAuthorityResolution::Missing {
                position_id,
                instrument_ref,
            } => {
                validate_position_authority(position, position_id, instrument_ref)?;
                Ok(PortfolioBondAnalysis::missing_bond_metric(signed_notional))
            }
            PortfolioBondRatesAuthorityResolution::Bond(resolved) => {
                validate_position_authority(
                    position,
                    &resolved.position_id,
                    &resolved.instrument_ref,
                )?;
                let result_units = portfolio_result_units(&resolved.result_units)?;
                let units = PortfolioWeightedMetricUnits::new(
                    result_units.rate().reference().clone(),
                    result_units.years().reference().clone(),
                    result_units.years_squared().reference().clone(),
                    result_units.rate().reference().clone(),
                    result_units.years().reference().clone(),
                );
                let command = BondRatesCommand {
                    owner: context.owner.clone(),
                    subject_ref: context.subject_ref.clone(),
                    units: result_units.requirements(),
                    currency_unit: resolved.currency_unit.clone(),
                    rate_unit: resolved.rate_unit.clone(),
                    knowledge_at: context.knowledge_at.clone(),
                    bond: resolved.bond.clone(),
                    calendar: resolved.calendar.clone(),
                    data_snapshot:
                        crate::use_cases::rates_materialization::ImmutableSnapshotBinding::new(
                            resolved.data_snapshot.id.clone(),
                            resolved.data_snapshot.content_hash.clone(),
                        ),
                    tax_rule_pack: resolved.tax_rule_pack.clone(),
                    valuation_at: context.valuation_at.clone(),
                    settlement_date: resolved.settlement_date,
                    calendar_requirement: resolved.calendar_requirement,
                    mode: resolved.mode,
                    input_value: resolved.input_value,
                };
                if command.bond.version_ref() != position.instrument_ref() {
                    return Err(integrity());
                }
                let trace = trace_from_fingerprint(authority_fingerprint)?;
                let materialized = self.materializer.execute(scope, command, trace).await?;
                if materialized.input().bond().version_ref() != position.instrument_ref()
                    || materialized.input().owner() != &context.owner
                    || materialized.input().valuation_at() != &context.valuation_at
                {
                    return Err(integrity());
                }
                let result =
                    CalculateBondAnalytics::new(self.engine).execute(materialized.input())?;
                result
                    .validate_against(materialized.input())
                    .map_err(map_domain_error)?;
                let metrics = PortfolioBondMetricFacts::new(
                    result.measures().yield_to_maturity(),
                    result.measures().modified_duration(),
                    result.measures().convexity(),
                    materialized.input().terms().coupon_rate(),
                    resolved.remaining_years,
                    units,
                )?;
                let analysis_result = PortfolioBondAnalysisResult::from_verified(
                    result,
                    result_units,
                    context.subject_ref.clone(),
                    materialized.evidence().clone(),
                )?;
                Ok(PortfolioBondAnalysis::bond(
                    signed_notional,
                    metrics,
                    analysis_result,
                ))
            }
        }
    }
}

#[derive(Clone)]
pub struct OwnedExactPortfolioBondAnalysisHandoff {
    definitions: Arc<dyn DefinitionRepository>,
    subjects: Arc<dyn SubjectRepository>,
    snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
    blobs: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
    tax_parser: Arc<dyn TaxRulePackParser>,
    engine: Arc<dyn BondAnalyticsEngine>,
}

impl OwnedExactPortfolioBondAnalysisHandoff {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        definitions: Arc<dyn DefinitionRepository>,
        subjects: Arc<dyn SubjectRepository>,
        snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
        blobs: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
        tax_parser: Arc<dyn TaxRulePackParser>,
        engine: Arc<dyn BondAnalyticsEngine>,
    ) -> Self {
        Self {
            definitions,
            subjects,
            snapshots,
            blobs,
            integrity_events,
            tax_parser,
            engine,
        }
    }
}

#[async_trait]
impl PortfolioBondAnalysisHandoff for OwnedExactPortfolioBondAnalysisHandoff {
    async fn analyze(
        &self,
        scope: &AccessScope,
        context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
        position: &Position,
        authority: &PortfolioBondRatesAuthorityResolution,
        authority_fingerprint: &OperationFingerprint,
    ) -> ApplicationResult<PortfolioBondAnalysis> {
        let materializer = MaterializeBondRatesInput::new(
            self.definitions.as_ref(),
            self.subjects.as_ref(),
            self.snapshots.as_ref(),
            self.blobs.as_ref(),
            self.integrity_events.as_ref(),
            self.tax_parser.as_ref(),
        );
        ExactPortfolioBondAnalysisHandoff::new(&materializer, self.engine.as_ref())
            .analyze(
                scope,
                context,
                snapshot,
                position,
                authority,
                authority_fingerprint,
            )
            .await
    }
}

fn validate_position_authority(
    position: &Position,
    position_id: &Ulid,
    instrument_ref: &ficant_domain::primitives::VersionRef,
) -> ApplicationResult<()> {
    if position.id() != position_id || position.instrument_ref() != instrument_ref {
        return Err(integrity());
    }
    Ok(())
}

fn portfolio_result_units(
    values: &[PortfolioRatesUnitAuthority],
) -> ApplicationResult<PortfolioRatesUnitBindings> {
    let bindings = values
        .iter()
        .map(|value| {
            if value.dimension != value.role.expected_dimension() || value.scale > MAX_DECIMAL_SCALE
            {
                return Err(invalid_unit());
            }
            Ok((
                portfolio_unit_role(value.role),
                RatesUnitRequirement::new(value.reference.clone(), value.role.expected_dimension()),
            ))
        })
        .collect::<ApplicationResult<Vec<_>>>()?;
    PortfolioRatesUnitBindings::new(bindings)
}

const fn portfolio_unit_role(value: AuthorityUnitRole) -> PortfolioRatesUnitRole {
    match value {
        AuthorityUnitRole::CurrencyAmount => PortfolioRatesUnitRole::CurrencyAmount,
        AuthorityUnitRole::PricePer100 => PortfolioRatesUnitRole::PricePer100,
        AuthorityUnitRole::Rate => PortfolioRatesUnitRole::Rate,
        AuthorityUnitRole::Years => PortfolioRatesUnitRole::Years,
        AuthorityUnitRole::YearsSquared => PortfolioRatesUnitRole::YearsSquared,
        AuthorityUnitRole::Dv01Per100 => PortfolioRatesUnitRole::Dv01Per100,
        AuthorityUnitRole::Dv01 => PortfolioRatesUnitRole::Dv01,
        AuthorityUnitRole::Dimensionless => PortfolioRatesUnitRole::Dimensionless,
        AuthorityUnitRole::ContractCount => PortfolioRatesUnitRole::ContractCount,
    }
}

fn trace_from_fingerprint(value: &OperationFingerprint) -> ApplicationResult<SafeTraceContext> {
    let trace = value.content_hash().as_bytes()[..16].iter().fold(
        String::with_capacity(32),
        |mut encoded, byte| {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
            encoded
        },
    );
    SafeTraceContext::new(trace)
}

#[async_trait]
pub trait PortfolioOverviewPublisher: Send + Sync {
    /// Persists the formal record before the overview may cross the application boundary.
    async fn publish(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        draft: &PortfolioOverviewDraft,
    ) -> ApplicationResult<FormalOutputEvidence>;
}

#[async_trait]
pub trait PortfolioOverviewRecordFactory: Send + Sync {
    /// Builds the canonical R7B record for this exact `PortfolioOverview` payload.
    ///
    /// # Errors
    ///
    /// Returns a closed binding or serialization failure when the record is not exact.
    async fn build(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        draft: &PortfolioOverviewDraft,
    ) -> ApplicationResult<FormalOutputRecord>;
}

#[derive(Clone, Debug)]
pub struct PortfolioFormalExecutionBinding {
    code: CodeBinding,
    runtime: RuntimeBinding,
}

impl PortfolioFormalExecutionBinding {
    #[must_use]
    pub const fn new(code: CodeBinding, runtime: RuntimeBinding) -> Self {
        Self { code, runtime }
    }
}

#[derive(Clone)]
pub struct OwnedPortfolioOverviewRecordFactory {
    subjects: Arc<dyn SubjectRepository>,
    execution: PortfolioFormalExecutionBinding,
}

impl OwnedPortfolioOverviewRecordFactory {
    #[must_use]
    pub fn new(
        subjects: Arc<dyn SubjectRepository>,
        execution: PortfolioFormalExecutionBinding,
    ) -> Self {
        Self {
            subjects,
            execution,
        }
    }
}

#[async_trait]
impl PortfolioOverviewRecordFactory for OwnedPortfolioOverviewRecordFactory {
    async fn build(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        draft: &PortfolioOverviewDraft,
    ) -> ApplicationResult<FormalOutputRecord> {
        scope.authorize(owner)?;
        let subject_ref = draft.subject_ref().clone();
        let subject = self
            .subjects
            .get_subject_scoped(scope, subject_ref.clone())
            .await?
            .ok_or_else(not_found)?;
        if subject.subject().owner() != Some(owner) || subject.version().reference() != &subject_ref
        {
            return Err(integrity());
        }
        let subject_hash = crate::ports::subject_record_content_hash(&subject)?;
        let subject_binding = formal_object_input(
            "subject".to_owned(),
            FormalInputKind::Subject,
            owner,
            &subject_ref,
            subject_hash.clone(),
        )?;
        let consumed_inputs = draft.formal_input_bindings(owner, &subject_hash)?;
        let implementations = draft.implementation_bindings()?;
        let payload = draft.canonical_payload();
        let evidence = FormalOutputEvidence::new(FormalOutputEvidenceInput {
            schema_id: PORTFOLIO_OVERVIEW_SCHEMA_ID.to_owned(),
            subject: subject_binding,
            consumed_inputs,
            code: self.execution.code.clone(),
            runtime: self.execution.runtime.clone(),
            implementations,
            parameters_hash: draft.request_fingerprint.clone(),
            seed: None,
            result_hash: ContentHash::digest(&payload),
        })
        .map_err(map_domain_error)?;
        FormalOutputRecord::new(owner.clone(), evidence, payload)
    }
}

fn formal_object_input(
    role: String,
    kind: FormalInputKind,
    owner: &OwnerRef,
    reference: &ficant_domain::primitives::VersionRef,
    content_hash: ContentHash,
) -> ApplicationResult<FormalInputBinding> {
    let lineage = LineageRef::new(
        reference.id().clone(),
        Some(reference.version()),
        Some(content_hash),
    )
    .map_err(map_domain_error)?;
    formal_lineage_input(role, kind, owner, lineage, None, None, None, None)
}

#[allow(clippy::too_many_arguments)]
fn formal_lineage_input(
    role: String,
    kind: FormalInputKind,
    owner: &OwnerRef,
    reference: LineageRef,
    observed_at: Option<ficant_domain::primitives::MarketTime>,
    visible_at: Option<ficant_domain::primitives::MarketTime>,
    effective_from: Option<ficant_domain::primitives::MarketTime>,
    effective_to: Option<ficant_domain::primitives::MarketTime>,
) -> ApplicationResult<FormalInputBinding> {
    FormalInputBinding::new(FormalInputBindingInput {
        role,
        kind,
        owner: owner.clone(),
        reference: FormalInputReference::Object(reference),
        observed_at,
        visible_at,
        effective_from,
        effective_to,
    })
    .map_err(map_domain_error)
}

fn portfolio_formal_inputs(
    owner: &OwnerRef,
    draft: &PortfolioOverviewDraft,
    _subject_hash: &ContentHash,
) -> ApplicationResult<Vec<FormalInputBinding>> {
    if draft.position_snapshots.len() != draft.members.len()
        || draft
            .position_snapshots
            .iter()
            .zip(&draft.members)
            .any(|(snapshot, member)| snapshot != &member.position_snapshot)
    {
        return Err(integrity());
    }

    let mut inputs = portfolio_scope_formal_inputs(owner, draft)?;
    for (member_index, member) in draft.members.iter().enumerate() {
        for (input_index, input) in member.risk_inputs.iter().enumerate() {
            inputs.push(formal_risk_named_input(
                format!("member.{member_index:04}.risk.{input_index:04}"),
                owner,
                input,
            )?);
        }
        for (evidence_index, evidence) in member.analytics_authority_evidence.iter().enumerate() {
            inputs.push(formal_analytics_authority_input(
                format!("member.{member_index:04}.authority.{evidence_index:04}"),
                owner,
                evidence,
            )?);
        }
        append_rates_formal_inputs(
            &mut inputs,
            owner,
            draft.subject_ref(),
            &format!("member.{member_index:04}.rates"),
            &member.rates_evidence,
        )?;
    }
    for (input_index, input) in draft.benchmark_risk_inputs.iter().enumerate() {
        inputs.push(formal_risk_named_input(
            format!("benchmark.risk.{input_index:04}"),
            owner,
            input,
        )?);
    }
    for (evidence_index, evidence) in draft
        .benchmark_analytics_authority_evidence
        .iter()
        .enumerate()
    {
        inputs.push(formal_analytics_authority_input(
            format!("benchmark.authority.{evidence_index:04}"),
            owner,
            evidence,
        )?);
    }
    append_rates_formal_inputs(
        &mut inputs,
        owner,
        draft.subject_ref(),
        "benchmark.rates",
        &draft.benchmark_rates_evidence,
    )?;
    inputs.sort_by(|left, right| left.role().cmp(right.role()));
    if inputs
        .windows(2)
        .any(|pair| pair[0].role() == pair[1].role())
    {
        return Err(lineage());
    }
    Ok(inputs)
}

fn formal_risk_named_input(
    role: String,
    owner: &OwnerRef,
    input: &PortfolioRiskNamedEvidenceBinding,
) -> ApplicationResult<FormalInputBinding> {
    let kind = match input.kind {
        PortfolioRiskNamedEvidenceKind::FactorDefinition => FormalInputKind::FactorDefinition,
        PortfolioRiskNamedEvidenceKind::CurveNodeDefinition => FormalInputKind::CurveNodeDefinition,
    };
    let reference = NamedContentRef::new(input.identity.clone(), input.content_hash.clone())
        .map_err(map_domain_error)?;
    FormalInputBinding::new(FormalInputBindingInput {
        role,
        kind,
        owner: owner.clone(),
        reference: FormalInputReference::Named(reference),
        observed_at: input.observed_at.clone(),
        visible_at: input.visible_at.clone(),
        effective_from: input.effective_from.clone(),
        effective_to: input.effective_to.clone(),
    })
    .map_err(map_domain_error)
}

fn portfolio_scope_formal_inputs(
    owner: &OwnerRef,
    draft: &PortfolioOverviewDraft,
) -> ApplicationResult<Vec<FormalInputBinding>> {
    let mut inputs = Vec::new();
    let mut member_index = 0_usize;
    for binding in &draft.catalog_evidence {
        let (role, kind) = match binding.role() {
            crate::ports::PortfolioCatalogEvidenceRole::SelectedBook => {
                ("catalog.selected".to_owned(), FormalInputKind::Book)
            }
            crate::ports::PortfolioCatalogEvidenceRole::SelectedGroup => (
                "catalog.selected".to_owned(),
                FormalInputKind::PortfolioGroup,
            ),
            crate::ports::PortfolioCatalogEvidenceRole::SelectedPortfolio => {
                ("catalog.selected".to_owned(), FormalInputKind::Portfolio)
            }
            crate::ports::PortfolioCatalogEvidenceRole::MemberPortfolio => {
                let role = format!("catalog.member.{member_index:04}");
                member_index += 1;
                (role, FormalInputKind::Portfolio)
            }
            crate::ports::PortfolioCatalogEvidenceRole::Benchmark => {
                ("catalog.benchmark".to_owned(), FormalInputKind::Benchmark)
            }
            crate::ports::PortfolioCatalogEvidenceRole::MetricConvention => (
                "catalog.metric_convention".to_owned(),
                FormalInputKind::PortfolioMetricConvention,
            ),
        };
        inputs.push(formal_catalog_input(role, kind, owner, binding)?);
    }
    for (index, snapshot) in draft.position_snapshots.iter().enumerate() {
        inputs.push(formal_position_snapshot_input(
            format!("position_snapshot.{index:04}"),
            owner,
            snapshot,
        )?);
    }
    Ok(inputs)
}

fn formal_catalog_input(
    role: String,
    kind: FormalInputKind,
    owner: &OwnerRef,
    binding: &crate::ports::PortfolioCatalogEvidenceBinding,
) -> ApplicationResult<FormalInputBinding> {
    let reference = LineageRef::new(
        binding.reference().id().clone(),
        Some(binding.reference().version()),
        Some(binding.content_hash().clone()),
    )
    .map_err(map_domain_error)?;
    formal_lineage_input(
        role,
        kind,
        owner,
        reference,
        None,
        Some(binding.visible_at().clone()),
        Some(binding.effective_from().clone()),
        Some(binding.effective_to().clone()),
    )
}

fn formal_position_snapshot_input(
    role: String,
    owner: &OwnerRef,
    snapshot: &PortfolioSnapshotBinding,
) -> ApplicationResult<FormalInputBinding> {
    let reference = LineageRef::new(
        snapshot.snapshot_id().clone(),
        None,
        Some(snapshot.content_hash().clone()),
    )
    .map_err(map_domain_error)?;
    formal_lineage_input(
        role,
        FormalInputKind::PositionSnapshot,
        owner,
        reference,
        Some(snapshot.observed_at().clone()),
        Some(snapshot.visible_at().clone()),
        None,
        None,
    )
}

fn formal_analytics_authority_input(
    role: String,
    owner: &OwnerRef,
    evidence: &crate::ports::PortfolioAnalyticsEvidenceBinding,
) -> ApplicationResult<FormalInputBinding> {
    let kind = match evidence.kind {
        crate::ports::PortfolioAnalyticsEvidenceKind::PositionSnapshot => {
            FormalInputKind::PositionSnapshot
        }
        crate::ports::PortfolioAnalyticsEvidenceKind::CurveSnapshot => {
            FormalInputKind::CurveSnapshot
        }
        crate::ports::PortfolioAnalyticsEvidenceKind::DataSnapshot
        | crate::ports::PortfolioAnalyticsEvidenceKind::FuturesDataSnapshot => {
            FormalInputKind::DataSnapshot
        }
        crate::ports::PortfolioAnalyticsEvidenceKind::CurveRulePack
        | crate::ports::PortfolioAnalyticsEvidenceKind::TaxRulePack => FormalInputKind::RulePack,
        crate::ports::PortfolioAnalyticsEvidenceKind::Unit => FormalInputKind::Unit,
        crate::ports::PortfolioAnalyticsEvidenceKind::Instrument => FormalInputKind::Instrument,
        crate::ports::PortfolioAnalyticsEvidenceKind::Calendar => FormalInputKind::Calendar,
        crate::ports::PortfolioAnalyticsEvidenceKind::Valuation => FormalInputKind::Fact,
    };
    let reference = LineageRef::new(
        evidence.object_id.clone(),
        evidence.version,
        Some(evidence.content_hash.clone()),
    )
    .map_err(map_domain_error)?;
    formal_lineage_input(
        role,
        kind,
        owner,
        reference,
        evidence.observed_at.clone(),
        evidence.visible_at.clone(),
        evidence.effective_from.clone(),
        evidence.effective_to.clone(),
    )
}

fn append_rates_formal_inputs(
    inputs: &mut Vec<FormalInputBinding>,
    owner: &OwnerRef,
    subject_ref: &ficant_domain::primitives::VersionRef,
    prefix: &str,
    requests: &[RatesRequestEvidence],
) -> ApplicationResult<()> {
    for (request_index, request) in requests.iter().enumerate() {
        for (input_index, input) in request.consumed_inputs().iter().enumerate() {
            if input.owner() != owner {
                return Err(integrity());
            }
            if input.role() == RatesInputRole::Subject {
                validate_rates_subject(input.binding(), subject_ref)?;
                continue;
            }
            inputs.push(formal_rates_input(
                format!("{prefix}.{request_index:04}.input.{input_index:04}"),
                owner,
                input,
            )?);
        }
    }
    Ok(())
}

fn validate_rates_subject(
    binding: &RatesEvidenceBinding,
    subject_ref: &ficant_domain::primitives::VersionRef,
) -> ApplicationResult<()> {
    // R5D AnalyzeBond Subject evidence uses rates_materialization's subject hash,
    // not `subject_record_content_hash`. The overview Subject binding keeps the
    // latter; this check only proves the same exact VersionRef was consumed.
    match binding {
        RatesEvidenceBinding::Object(reference) if reference.version_ref() == subject_ref => Ok(()),
        RatesEvidenceBinding::Object(_)
        | RatesEvidenceBinding::Snapshot(_)
        | RatesEvidenceBinding::Artifact(_)
        | RatesEvidenceBinding::CurveNode(_) => Err(integrity()),
    }
}

fn formal_rates_input(
    role: String,
    owner: &OwnerRef,
    input: &crate::use_cases::rates_materialization::RatesInputEvidence,
) -> ApplicationResult<FormalInputBinding> {
    let kind = formal_rates_kind(input.role())?;
    match input.binding() {
        RatesEvidenceBinding::Object(reference) => {
            let lineage = LineageRef::new(
                reference.version_ref().id().clone(),
                Some(reference.version_ref().version()),
                Some(reference.content_hash().clone()),
            )
            .map_err(map_domain_error)?;
            formal_lineage_input(
                role,
                kind,
                owner,
                lineage,
                input.observed_at().cloned(),
                input.visible_at().cloned(),
                input.effective_from().cloned(),
                input.effective_to().cloned(),
            )
        }
        RatesEvidenceBinding::Snapshot(binding) => formal_unversioned_rates_input(
            role,
            kind,
            owner,
            binding.id(),
            binding.content_hash(),
            input,
        ),
        RatesEvidenceBinding::Artifact(binding) => formal_unversioned_rates_input(
            role,
            kind,
            owner,
            binding.id(),
            binding.content_hash(),
            input,
        ),
        RatesEvidenceBinding::CurveNode(binding) => {
            let named = NamedContentRef::new(
                binding.curve_node_id().to_owned(),
                binding.content_hash().clone(),
            )
            .map_err(map_domain_error)?;
            FormalInputBinding::new(FormalInputBindingInput {
                role,
                kind,
                owner: owner.clone(),
                reference: FormalInputReference::Named(named),
                observed_at: input.observed_at().cloned(),
                visible_at: input.visible_at().cloned(),
                effective_from: input.effective_from().cloned(),
                effective_to: input.effective_to().cloned(),
            })
            .map_err(map_domain_error)
        }
    }
}

fn formal_unversioned_rates_input(
    role: String,
    kind: FormalInputKind,
    owner: &OwnerRef,
    id: &Ulid,
    content_hash: &ContentHash,
    input: &crate::use_cases::rates_materialization::RatesInputEvidence,
) -> ApplicationResult<FormalInputBinding> {
    let lineage =
        LineageRef::new(id.clone(), None, Some(content_hash.clone())).map_err(map_domain_error)?;
    formal_lineage_input(
        role,
        kind,
        owner,
        lineage,
        input.observed_at().cloned(),
        input.visible_at().cloned(),
        input.effective_from().cloned(),
        input.effective_to().cloned(),
    )
}

fn formal_rates_kind(role: RatesInputRole) -> ApplicationResult<FormalInputKind> {
    match role {
        RatesInputRole::Subject => Err(integrity()),
        RatesInputRole::Unit => Ok(FormalInputKind::Unit),
        RatesInputRole::Bond | RatesInputRole::FuturesContract => Ok(FormalInputKind::Instrument),
        RatesInputRole::Calendar => Ok(FormalInputKind::Calendar),
        RatesInputRole::CurveSnapshot => Ok(FormalInputKind::CurveSnapshot),
        RatesInputRole::DataSnapshot => Ok(FormalInputKind::DataSnapshot),
        RatesInputRole::DataSource => Ok(FormalInputKind::DataSource),
        RatesInputRole::TaxRulePack
        | RatesInputRole::FundingRulePack
        | RatesInputRole::DeliveryRulePack
        | RatesInputRole::CurveRulePack => Ok(FormalInputKind::RulePack),
        RatesInputRole::TargetRiskArtifact
        | RatesInputRole::DeliveryArtifact
        | RatesInputRole::CtdAnalyticsArtifact => Ok(FormalInputKind::Artifact),
        RatesInputRole::CurveNodeDefinition => Ok(FormalInputKind::CurveNodeDefinition),
    }
}

const fn selected_scope_lineage(value: &crate::ports::ExactPortfolioScopeKind) -> &LineageRef {
    match value {
        crate::ports::ExactPortfolioScopeKind::Book(reference)
        | crate::ports::ExactPortfolioScopeKind::Group(reference)
        | crate::ports::ExactPortfolioScopeKind::Portfolio(reference) => reference,
    }
}

fn portfolio_implementation_bindings(
    draft: &PortfolioOverviewDraft,
) -> ApplicationResult<Vec<FormalImplementationBinding>> {
    if draft.members.is_empty()
        || draft.position_snapshots.len() != draft.members.len()
        || draft.benchmark_key_rate_exposure.totals().is_empty()
    {
        return Err(integrity());
    }
    let mut result = vec![
        formal_implementation(
            "portfolio-aggregation",
            portfolio_aggregation_implementation_digest(draft),
        )?,
        formal_implementation(
            "position-views",
            ContentHash::digest(b"ficant.research.position-views.v1"),
        )?,
    ];
    for (index, member) in draft.members.iter().enumerate() {
        append_krd_implementation_bindings(
            &mut result,
            &format!("member.{index:04}"),
            member.key_rate_exposure(),
        )?;
        for (analysis_index, analysis) in member.bond_analyses.iter().enumerate() {
            result.push(formal_implementation(
                &format!("analyze-bond.member.{index:04}.{analysis_index:04}"),
                analyze_bond_implementation_digest(analysis.analysis()),
            )?);
        }
    }
    append_krd_implementation_bindings(
        &mut result,
        "benchmark",
        &draft.benchmark_key_rate_exposure,
    )?;
    for (index, analysis) in draft.benchmark_bond_analyses.iter().enumerate() {
        result.push(formal_implementation(
            &format!("analyze-bond.benchmark.{index:04}"),
            analyze_bond_implementation_digest(analysis.analysis()),
        )?);
    }
    result.sort_by(|left, right| left.role().cmp(right.role()));
    if result.is_empty()
        || result
            .windows(2)
            .any(|pair| pair[0].role() == pair[1].role())
    {
        return Err(integrity());
    }
    Ok(result)
}

fn formal_implementation(
    role: &str,
    digest: ContentHash,
) -> ApplicationResult<FormalImplementationBinding> {
    FormalImplementationBinding::new(role, digest).map_err(map_domain_error)
}

fn portfolio_aggregation_implementation_digest(draft: &PortfolioOverviewDraft) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, b"ficant.portfolio.aggregation.v1");
    append(
        &mut bytes,
        draft.metric_convention.reference().id().as_str().as_bytes(),
    );
    append(
        &mut bytes,
        &draft
            .metric_convention
            .reference()
            .version()
            .get()
            .to_be_bytes(),
    );
    append(
        &mut bytes,
        draft.metric_convention.content_hash().as_bytes(),
    );
    ContentHash::digest(&bytes)
}

fn append_krd_implementation_bindings(
    result: &mut Vec<FormalImplementationBinding>,
    suffix: &str,
    exposure: &PortfolioKeyRateExposure,
) -> ApplicationResult<()> {
    let algorithm = exposure.algorithm();
    let mut algorithm_bytes = Vec::new();
    append(&mut algorithm_bytes, b"ficant.portfolio.krd-algorithm.v1");
    append(&mut algorithm_bytes, algorithm.algorithm_id().as_bytes());
    append(
        &mut algorithm_bytes,
        &algorithm.algorithm_version().to_be_bytes(),
    );
    append(
        &mut algorithm_bytes,
        algorithm.convention_profile().as_bytes(),
    );
    result.push(formal_implementation(
        &format!("krd-algorithm.{suffix}"),
        ContentHash::digest(&algorithm_bytes),
    )?);

    let mut topology_bytes = Vec::new();
    append(&mut topology_bytes, b"ficant.portfolio.factor-topology.v1");
    let mut factor_ids = BTreeSet::new();
    for factor in exposure.totals() {
        if !factor_ids.insert(factor.factor_id()) {
            return Err(integrity());
        }
        append(&mut topology_bytes, factor.factor_id().as_bytes());
        append(
            &mut topology_bytes,
            factor.factor_definition_hash().as_bytes(),
        );
        append(
            &mut topology_bytes,
            factor.unit().unit_id().as_str().as_bytes(),
        );
        append(
            &mut topology_bytes,
            &factor.unit().version().get().to_be_bytes(),
        );
    }
    result.push(formal_implementation(
        &format!("factor-topology.{suffix}"),
        ContentHash::digest(&topology_bytes),
    )?);
    Ok(())
}

fn analyze_bond_implementation_digest(result: &PortfolioBondAnalysisResult) -> ContentHash {
    let metadata = result.metadata();
    let mut bytes = Vec::new();
    append(
        &mut bytes,
        b"ficant.portfolio.analyze-bond-implementation.v1",
    );
    append(&mut bytes, metadata.engine_id().as_bytes());
    append(&mut bytes, metadata.engine_version().as_bytes());
    append(&mut bytes, metadata.algorithm_id().as_bytes());
    append(&mut bytes, &metadata.algorithm_version().to_be_bytes());
    append(&mut bytes, metadata.convention_profile().as_bytes());
    append(&mut bytes, &metadata.abi_version().to_be_bytes());
    ContentHash::digest(&bytes)
}

pub struct RequiredPortfolioOverviewPublisher<'a, 'b> {
    formal_outputs: &'a FormalOutputUseCase<'b>,
    factory: &'a dyn PortfolioOverviewRecordFactory,
}

impl<'a, 'b> RequiredPortfolioOverviewPublisher<'a, 'b> {
    #[must_use]
    pub const fn new(
        formal_outputs: &'a FormalOutputUseCase<'b>,
        factory: &'a dyn PortfolioOverviewRecordFactory,
    ) -> Self {
        Self {
            formal_outputs,
            factory,
        }
    }
}

#[async_trait]
impl PortfolioOverviewPublisher for RequiredPortfolioOverviewPublisher<'_, '_> {
    async fn publish(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        draft: &PortfolioOverviewDraft,
    ) -> ApplicationResult<FormalOutputEvidence> {
        let record = self.factory.build(scope, owner, draft).await?;
        if record.owner() != owner
            || record.evidence().schema_id() != PORTFOLIO_OVERVIEW_SCHEMA_ID
            || record.canonical_payload() != draft.canonical_payload()
        {
            return Err(integrity());
        }
        let stored = self.formal_outputs.publish(scope, record).await?;
        if stored.owner() != owner
            || stored.evidence().schema_id() != PORTFOLIO_OVERVIEW_SCHEMA_ID
            || stored.canonical_payload() != draft.canonical_payload()
        {
            return Err(integrity());
        }
        Ok(stored.evidence().clone())
    }
}

#[derive(Clone)]
pub struct OwnedRequiredPortfolioOverviewPublisher {
    repository: Arc<dyn FormalOutputRepository>,
    factory: Arc<dyn PortfolioOverviewRecordFactory>,
}

impl OwnedRequiredPortfolioOverviewPublisher {
    #[must_use]
    pub fn new(
        repository: Arc<dyn FormalOutputRepository>,
        factory: Arc<dyn PortfolioOverviewRecordFactory>,
    ) -> Self {
        Self {
            repository,
            factory,
        }
    }
}

#[async_trait]
impl PortfolioOverviewPublisher for OwnedRequiredPortfolioOverviewPublisher {
    async fn publish(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        draft: &PortfolioOverviewDraft,
    ) -> ApplicationResult<FormalOutputEvidence> {
        let use_case = FormalOutputUseCase::new(self.repository.as_ref());
        RequiredPortfolioOverviewPublisher::new(&use_case, self.factory.as_ref())
            .publish(scope, owner, draft)
            .await
    }
}

pub struct PortfolioAggregationUseCase<'a> {
    authority: &'a dyn PortfolioAggregationAuthority,
    analytics_authority: &'a dyn PortfolioAnalyticsAuthorityHandoff,
    positions: &'a dyn PositionSnapshotRepository,
    position_views: &'a dyn PortfolioPositionViewsHandoff,
    risk: &'a dyn PortfolioRiskHandoff,
    bond_analysis: &'a dyn PortfolioBondAnalysisHandoff,
    publisher: &'a dyn PortfolioOverviewPublisher,
}

impl<'a> PortfolioAggregationUseCase<'a> {
    #[must_use]
    pub const fn new(
        authority: &'a dyn PortfolioAggregationAuthority,
        analytics_authority: &'a dyn PortfolioAnalyticsAuthorityHandoff,
        positions: &'a dyn PositionSnapshotRepository,
        position_views: &'a dyn PortfolioPositionViewsHandoff,
        risk: &'a dyn PortfolioRiskHandoff,
        bond_analysis: &'a dyn PortfolioBondAnalysisHandoff,
        publisher: &'a dyn PortfolioOverviewPublisher,
    ) -> Self {
        Self {
            authority,
            analytics_authority,
            positions,
            position_views,
            risk,
            bond_analysis,
            publisher,
        }
    }

    /// Resolves every exact authority before handing any value to `PositionViews`,
    /// `PortfolioRisk` or `AnalyzeBond`, then publishes the formal record before returning success.
    ///
    /// # Errors
    ///
    /// Fails closed on authorization, exact-reference, owner, Subject, bitemporal, unit,
    /// convention, benchmark, snapshot, numerical, or formal-publication drift.
    pub async fn execute(
        &self,
        principal: &AuthorizedPrincipal,
        context: &NormalizedPortfolioContext,
    ) -> ApplicationResult<PortfolioOverview> {
        self.execute_with_catalog_evidence(principal, context, None)
            .await
    }

    /// Executes a production request with the exact catalog evidence captured at normalization.
    ///
    /// # Errors
    ///
    /// Fails closed if required reads differ from any normalized catalog identity or time.
    pub async fn execute_resolution(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &crate::ports::NormalizedPortfolioContextResolution,
    ) -> ApplicationResult<PortfolioOverview> {
        self.execute_with_catalog_evidence(principal, resolution.context(), Some(resolution))
            .await
    }

    #[allow(clippy::too_many_lines)]
    async fn execute_with_catalog_evidence(
        &self,
        principal: &AuthorizedPrincipal,
        context: &NormalizedPortfolioContext,
        normalized_resolution: Option<&crate::ports::NormalizedPortfolioContextResolution>,
    ) -> ApplicationResult<PortfolioOverview> {
        authorize_portfolio_read(principal, &context.owner)?;
        validate_normalized_context(context)?;
        let resolved_result = match normalized_resolution {
            Some(resolution) => {
                self.authority
                    .resolve_aggregation_inputs_with_evidence(principal, resolution)
                    .await
            }
            None => {
                self.authority
                    .resolve_aggregation_inputs(principal, context)
                    .await
            }
        };
        let resolved = resolved_result?;
        validate_resolved_inputs(principal.access_scope(), context, &resolved)?;
        if normalized_resolution.is_some_and(|resolution| {
            resolution.catalog_evidence() != resolved.catalog_evidence.as_slice()
        }) {
            return Err(integrity());
        }

        let mut verified_members = Vec::with_capacity(resolved.portfolios.len());
        for record in &resolved.portfolios {
            let portfolio = record.value();
            let snapshot = self
                .positions
                .get_position_snapshot(
                    principal.access_scope(),
                    portfolio.position_snapshot().snapshot_id().clone(),
                    context.knowledge_at.clone(),
                )
                .await?
                .ok_or_else(not_found)?;
            validate_position_snapshot(context, portfolio, &snapshot)?;
            verified_members.push((record.clone(), snapshot));
        }
        let benchmark_snapshot = self
            .positions
            .get_position_snapshot(
                principal.access_scope(),
                resolved.benchmark_snapshot.snapshot_id().clone(),
                context.knowledge_at.clone(),
            )
            .await?
            .ok_or_else(not_found)?;
        validate_benchmark_snapshot(context, &resolved, &benchmark_snapshot)?;

        let mut analytics_authorities = Vec::with_capacity(verified_members.len());
        for (_, snapshot) in &verified_members {
            let authority = self
                .analytics_authority
                .resolve(principal, context, snapshot)
                .await?;
            validate_analytics_authority(context, snapshot, &authority)?;
            analytics_authorities.push(authority);
        }
        let benchmark_authority = self
            .analytics_authority
            .resolve(principal, context, &benchmark_snapshot)
            .await?;
        validate_analytics_authority(context, &benchmark_snapshot, &benchmark_authority)?;
        let output_scales = metric_output_scales(
            context,
            analytics_authorities
                .iter()
                .chain(std::iter::once(&benchmark_authority)),
        )?;
        let analytics_fingerprints = analytics_authorities
            .iter()
            .chain(std::iter::once(&benchmark_authority))
            .map(|authority| authority.request_fingerprint.content_hash().clone())
            .collect::<Vec<_>>();

        let mut member_results = Vec::with_capacity(verified_members.len());
        let mut scope_positions = Vec::new();
        let mut scope_imported_positions = Vec::new();
        let mut member_krd = Vec::new();
        let mut stale = false;
        for ((record, snapshot), analytics_authority) in
            verified_members.into_iter().zip(analytics_authorities)
        {
            stale |= is_stale(
                &context.knowledge_at,
                snapshot.observed_at(),
                resolved.convention.value().freshness_limit_seconds(),
            )?;
            let position_views = self.position_views.project(snapshot.clone())?;
            validate_views(&snapshot, &position_views)?;
            let risk = self
                .risk
                .calculate(
                    principal.access_scope(),
                    context,
                    &snapshot,
                    &analytics_authority.risk,
                )
                .await?;
            validate_risk(&snapshot, risk.exposure())?;
            scope_imported_positions.extend(snapshot.positions().iter().cloned());
            let (metric_positions, rates_evidence, bond_analyses) = self
                .materialize_metric_positions(
                    principal.access_scope(),
                    context,
                    &snapshot,
                    &position_views,
                    &analytics_authority,
                )
                .await?;
            let metrics = aggregate_portfolio_metrics(
                &metric_positions,
                risk.exposure().totals(),
                output_scales,
            )?;
            scope_positions.extend(metric_positions);
            member_krd.push(risk.exposure().totals().to_vec());
            let (key_rate_exposure, risk_inputs) = risk.into_parts();
            member_results.push(PortfolioMemberOverview {
                portfolio: exact_portfolio_ref(record.value())?,
                position_snapshot: record.value().position_snapshot().clone(),
                basic_metrics: metrics.basic_metrics.clone(),
                krd_summary: metrics.krd_summary.clone(),
                position_views,
                key_rate_exposure,
                risk_inputs,
                rates_evidence,
                bond_analyses,
                analytics_authority_evidence: analytics_authority.evidence,
                analytics_authority_fingerprint: analytics_authority
                    .request_fingerprint
                    .content_hash()
                    .clone(),
            });
        }
        scope_positions.sort_by(|left, right| left.position_id.cmp(&right.position_id));
        scope_imported_positions.sort_by(|left, right| left.id().cmp(right.id()));
        if scope_positions
            .windows(2)
            .any(|pair| pair[0].position_id == pair[1].position_id)
            || scope_imported_positions
                .windows(2)
                .any(|pair| pair[0].id() == pair[1].id())
        {
            return Err(lineage());
        }
        let scope_krd = aggregate_existing_krd(&member_krd)?;
        let scope_metrics =
            aggregate_portfolio_metrics(&scope_positions, &scope_krd, output_scales)?;

        stale |= is_stale(
            &context.knowledge_at,
            benchmark_snapshot.observed_at(),
            resolved.convention.value().freshness_limit_seconds(),
        )?;
        let benchmark_views = self.position_views.project(benchmark_snapshot.clone())?;
        validate_views(&benchmark_snapshot, &benchmark_views)?;
        let benchmark_risk = self
            .risk
            .calculate(
                principal.access_scope(),
                context,
                &benchmark_snapshot,
                &benchmark_authority.risk,
            )
            .await?;
        validate_risk(&benchmark_snapshot, benchmark_risk.exposure())?;
        let (benchmark_positions, benchmark_rates_evidence, benchmark_bond_analyses) = self
            .materialize_metric_positions(
                principal.access_scope(),
                context,
                &benchmark_snapshot,
                &benchmark_views,
                &benchmark_authority,
            )
            .await?;
        let benchmark_metrics = aggregate_portfolio_metrics(
            &benchmark_positions,
            benchmark_risk.exposure().totals(),
            output_scales,
        )?;

        let mut coverage = aggregate_portfolio_coverage(
            &scope_imported_positions,
            member_results
                .iter()
                .map(PortfolioMemberOverview::key_rate_exposure),
            scope_metrics.coverage.missing_reasons(),
        )?;
        if benchmark_risk
            .exposure()
            .coverage()
            .participating_position_count()
            != benchmark_risk
                .exposure()
                .coverage()
                .imported_position_count()
        {
            coverage
                .missing_reasons
                .push(PortfolioCoverageReason::BenchmarkPositionExcludedFromPortfolioRisk);
        }
        coverage.missing_reasons.sort_unstable();
        coverage.missing_reasons.dedup();

        let data_mode = if !coverage.missing_reasons.is_empty()
            || scope_metrics.data_mode == PortfolioMetricDataMode::Partial
            || benchmark_metrics.data_mode == PortfolioMetricDataMode::Partial
        {
            PortfolioMetricDataMode::Partial
        } else if stale {
            PortfolioMetricDataMode::Stale
        } else {
            PortfolioMetricDataMode::Real
        };
        let risk_input_sets = member_results
            .iter()
            .map(PortfolioMemberOverview::risk_inputs)
            .chain(std::iter::once(benchmark_risk.actual_inputs()))
            .collect::<Vec<_>>();
        let request_fingerprint = aggregation_request_fingerprint(
            context,
            &resolved,
            &analytics_fingerprints,
            &risk_input_sets,
        );
        let (benchmark_key_rate_exposure, benchmark_risk_inputs) = benchmark_risk.into_parts();
        let draft = PortfolioOverviewDraft {
            subject_ref: context.subject_ref.clone(),
            scope: resolved.exact_scope.clone(),
            catalog_evidence: resolved.catalog_evidence,
            position_snapshots: member_results
                .iter()
                .map(|member| member.position_snapshot.clone())
                .collect(),
            basic_metrics: scope_metrics.basic_metrics,
            krd_summary: scope_metrics.krd_summary,
            benchmark_metrics: benchmark_metrics.basic_metrics,
            benchmark: context.benchmark.clone(),
            metric_convention: context.metric_convention.clone(),
            coverage,
            members: member_results,
            benchmark_key_rate_exposure,
            benchmark_risk_inputs,
            benchmark_bond_analyses,
            benchmark_rates_evidence,
            benchmark_analytics_authority_evidence: benchmark_authority.evidence,
            benchmark_analytics_authority_fingerprint: benchmark_authority
                .request_fingerprint
                .content_hash()
                .clone(),
            request_fingerprint,
            data_mode,
        };
        let formal_evidence = self
            .publisher
            .publish(principal.access_scope(), &context.owner, &draft)
            .await?;
        validate_formal_evidence(context, &draft, &formal_evidence)?;
        Ok(PortfolioOverview {
            draft,
            formal_evidence,
        })
    }

    async fn materialize_metric_positions(
        &self,
        scope: &AccessScope,
        context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
        views: &PositionViews,
        authority: &ResolvedPortfolioAnalyticsAuthority,
    ) -> ApplicationResult<(
        Vec<PortfolioMetricPosition>,
        Vec<RatesRequestEvidence>,
        Vec<PortfolioMemberBondAnalysis>,
    )> {
        let mut result = Vec::with_capacity(snapshot.positions().len());
        let mut rates_evidence = Vec::new();
        let mut bond_analyses = Vec::new();
        for (position, position_authority) in snapshot.positions().iter().zip(&authority.bond_rates)
        {
            let view = views
                .positions
                .iter()
                .find(|candidate| candidate.position_id == *position.id())
                .ok_or_else(lineage)?;
            let analysis = self
                .bond_analysis
                .analyze(
                    scope,
                    context,
                    snapshot,
                    position,
                    position_authority,
                    &authority.request_fingerprint,
                )
                .await?;
            if let Some(evidence) = analysis.rates_evidence.clone() {
                rates_evidence.push(evidence);
            }
            if let Some(result) = analysis.analysis_result.clone() {
                let PortfolioBondRatesAuthorityResolution::Bond(resolved) = position_authority
                else {
                    return Err(integrity());
                };
                bond_analyses.push(PortfolioMemberBondAnalysis {
                    position_id: position.id().clone(),
                    instrument_ref: position.instrument_ref().clone(),
                    valuation: resolved.valuation.clone(),
                    analysis: result,
                });
            }
            result.push(PortfolioMetricPosition::from_totals(
                position.id().clone(),
                view.economic_value.clone(),
                view.economic_pnl.clone(),
                analysis.signed_notional,
                analysis.eligibility,
            )?);
        }
        result.sort_by(|left, right| left.position_id.cmp(&right.position_id));
        rates_evidence
            .sort_by(|left, right| left.request_fingerprint().cmp(right.request_fingerprint()));
        bond_analyses.sort_by(|left, right| left.position_id.cmp(&right.position_id));
        Ok((result, rates_evidence, bond_analyses))
    }
}

/// Arc-owned, `'static` composition boundary used by production transports.
#[derive(Clone)]
pub struct OwnedPortfolioAggregationBackend {
    authority: Arc<dyn PortfolioAggregationAuthority>,
    analytics_authority: Arc<dyn PortfolioAnalyticsAuthorityHandoff>,
    positions: Arc<dyn PositionSnapshotRepository>,
    position_views: Arc<dyn PortfolioPositionViewsHandoff>,
    risk: Arc<dyn PortfolioRiskHandoff>,
    bond_analysis: Arc<dyn PortfolioBondAnalysisHandoff>,
    publisher: Arc<dyn PortfolioOverviewPublisher>,
}

impl OwnedPortfolioAggregationBackend {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        authority: Arc<dyn PortfolioAggregationAuthority>,
        analytics_authority: Arc<dyn PortfolioAnalyticsAuthorityHandoff>,
        positions: Arc<dyn PositionSnapshotRepository>,
        position_views: Arc<dyn PortfolioPositionViewsHandoff>,
        risk: Arc<dyn PortfolioRiskHandoff>,
        bond_analysis: Arc<dyn PortfolioBondAnalysisHandoff>,
        publisher: Arc<dyn PortfolioOverviewPublisher>,
    ) -> Self {
        Self {
            authority,
            analytics_authority,
            positions,
            position_views,
            risk,
            bond_analysis,
            publisher,
        }
    }

    /// Executes one request using only application-owned authority and analytics seams.
    ///
    /// # Errors
    ///
    /// Propagates the same fail-closed result as `PortfolioAggregationUseCase::execute`.
    pub async fn execute(
        &self,
        principal: &AuthorizedPrincipal,
        context: &NormalizedPortfolioContext,
    ) -> ApplicationResult<PortfolioOverview> {
        PortfolioAggregationUseCase::new(
            self.authority.as_ref(),
            self.analytics_authority.as_ref(),
            self.positions.as_ref(),
            self.position_views.as_ref(),
            self.risk.as_ref(),
            self.bond_analysis.as_ref(),
            self.publisher.as_ref(),
        )
        .execute(principal, context)
        .await
    }

    /// Executes the production path with normalization-time catalog evidence preserved.
    ///
    /// # Errors
    ///
    /// Propagates any authority, catalog-evidence, numerical, or publication failure.
    pub async fn execute_resolution(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &crate::ports::NormalizedPortfolioContextResolution,
    ) -> ApplicationResult<PortfolioOverview> {
        PortfolioAggregationUseCase::new(
            self.authority.as_ref(),
            self.analytics_authority.as_ref(),
            self.positions.as_ref(),
            self.position_views.as_ref(),
            self.risk.as_ref(),
            self.bond_analysis.as_ref(),
            self.publisher.as_ref(),
        )
        .execute_resolution(principal, resolution)
        .await
    }
}

fn aggregate_portfolio_coverage<'a>(
    imported: &[Position],
    exposures: impl IntoIterator<Item = &'a PortfolioKeyRateExposure>,
    metric_reasons: &[PortfolioCoverageReason],
) -> ApplicationResult<PortfolioCoverage> {
    let mut participating_ids = Vec::new();
    let mut source_counts = BTreeMap::<PriceSourceType, u64>::new();
    let mut external_version_count = None;
    for exposure in exposures {
        if exposure.coverage().source_confidence() != Some(exposure.source_confidence()) {
            return Err(lineage());
        }
        participating_ids.extend(
            exposure
                .positions()
                .iter()
                .map(|position| position.position_id().clone()),
        );
        for count in exposure.source_confidence().counts() {
            let total = source_counts.entry(count.source_type()).or_default();
            *total = total
                .checked_add(count.record_count())
                .ok_or_else(validation)?;
        }
        let count = exposure
            .coverage()
            .distinct_external_data_source_version_count();
        if count != 0 && external_version_count.replace(count).is_some() {
            // Counts alone cannot prove whether two members consumed the same exact DataSource
            // versions, so a multi-member external-source aggregate closes instead of guessing.
            return Err(lineage());
        }
    }
    participating_ids.sort();
    if participating_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(lineage());
    }
    let counts = source_counts
        .into_iter()
        .map(|(source_type, count)| {
            PriceSourceCount::new(source_type, count).map_err(map_domain_error)
        })
        .collect::<ApplicationResult<Vec<_>>>()?;
    let source_confidence = PriceSourceSummary::new(counts).map_err(map_domain_error)?;
    let participation = CoverageDeclaration::for_complete_positions(
        imported,
        &participating_ids,
        Some(source_confidence),
        external_version_count.unwrap_or(0),
    )
    .map_err(map_domain_error)?;
    let mut missing_reasons = metric_reasons.to_vec();
    if participation.participating_position_count() != participation.imported_position_count() {
        missing_reasons.push(PortfolioCoverageReason::PositionExcludedFromPortfolioRisk);
    }
    missing_reasons.sort_unstable();
    missing_reasons.dedup();
    Ok(PortfolioCoverage {
        participation,
        missing_reasons,
    })
}

fn validate_analytics_authority(
    context: &NormalizedPortfolioContext,
    snapshot: &PositionSnapshot,
    authority: &ResolvedPortfolioAnalyticsAuthority,
) -> ApplicationResult<()> {
    let units = portfolio_result_units(&authority.units)?;
    if authority.bond_rates.len() != snapshot.positions().len()
        || authority.risk.dv01_unit != *units.dv01().reference()
    {
        return Err(integrity());
    }
    for (position, resolution) in snapshot.positions().iter().zip(&authority.bond_rates) {
        match resolution {
            PortfolioBondRatesAuthorityResolution::Bond(value) => {
                validate_position_authority(position, &value.position_id, &value.instrument_ref)?;
                if value.currency_unit != *units.currency_amount().reference()
                    || value.rate_unit != *units.rate().reference()
                    || value.result_units != authority.units
                    || !value.remaining_years.is_positive()
                {
                    return Err(integrity());
                }
            }
            PortfolioBondRatesAuthorityResolution::NonBond {
                position_id,
                instrument_ref,
            }
            | PortfolioBondRatesAuthorityResolution::Missing {
                position_id,
                instrument_ref,
            } => validate_position_authority(position, position_id, instrument_ref)?,
        }
    }
    if context.currency_unit != *units.currency_amount().reference()
        || authority.evidence.is_empty()
    {
        return Err(integrity());
    }
    let position_inputs = authority
        .evidence
        .iter()
        .filter(|input| {
            input.kind == crate::ports::PortfolioAnalyticsEvidenceKind::PositionSnapshot
        })
        .collect::<Vec<_>>();
    if position_inputs.len() != 1
        || position_inputs[0].object_id != *snapshot.id()
        || position_inputs[0].version.is_some()
        || position_inputs[0].content_hash != *snapshot.content_hash()
        || position_inputs[0].observed_at.as_ref() != Some(snapshot.observed_at())
        || position_inputs[0].visible_at.as_ref() != Some(snapshot.visible_at())
        || position_inputs[0].effective_from.is_some()
        || position_inputs[0].effective_to.is_some()
        || authority.evidence.iter().any(|input| {
            input
                .observed_at
                .as_ref()
                .is_some_and(|time| time.instant() > context.valuation_at.instant())
                || input
                    .visible_at
                    .as_ref()
                    .is_some_and(|time| time.instant() > context.knowledge_at.instant())
                || matches!(
                    (input.effective_from.as_ref(), input.effective_to.as_ref()),
                    (Some(from), Some(to))
                        if from.instant() > context.valuation_at.instant()
                            || to.instant() <= context.valuation_at.instant()
                )
        })
    {
        return Err(integrity());
    }
    Ok(())
}

fn metric_output_scales<'a>(
    context: &NormalizedPortfolioContext,
    authorities: impl IntoIterator<Item = &'a ResolvedPortfolioAnalyticsAuthority>,
) -> ApplicationResult<PortfolioMetricOutputScales> {
    let mut values = authorities.into_iter();
    let first = values.next().ok_or_else(integrity)?;
    if values.any(|value| value.units != first.units) {
        return Err(invalid_unit());
    }
    let currency = authority_unit(&first.units, AuthorityUnitRole::CurrencyAmount)?;
    let rate = authority_unit(&first.units, AuthorityUnitRole::Rate)?;
    let years = authority_unit(&first.units, AuthorityUnitRole::Years)?;
    let years_squared = authority_unit(&first.units, AuthorityUnitRole::YearsSquared)?;
    let dv01 = authority_unit(&first.units, AuthorityUnitRole::Dv01)?;
    if currency.reference != context.currency_unit {
        return Err(invalid_unit());
    }
    PortfolioMetricOutputScales::new(
        currency.scale,
        rate.scale,
        years.scale,
        years_squared.scale,
        dv01.scale,
    )
}

fn authority_unit(
    values: &[PortfolioRatesUnitAuthority],
    role: AuthorityUnitRole,
) -> ApplicationResult<&PortfolioRatesUnitAuthority> {
    let mut matches = values.iter().filter(|value| value.role == role);
    let value = matches.next().ok_or_else(invalid_unit)?;
    if matches.next().is_some()
        || value.dimension != role.expected_dimension()
        || value.scale > MAX_DECIMAL_SCALE
    {
        return Err(invalid_unit());
    }
    Ok(value)
}

fn validate_normalized_context(context: &NormalizedPortfolioContext) -> ApplicationResult<()> {
    if context.knowledge_at.instant() < context.valuation_at.instant()
        || context.period_from.instant() > context.period_to.instant()
        || context.period_to != context.valuation_at
        || context.scope.member_portfolios().is_empty()
    {
        return Err(validation());
    }
    require_exact_lineage_set(context.scope.member_portfolios())
}

fn validate_resolved_inputs(
    scope: &AccessScope,
    context: &NormalizedPortfolioContext,
    resolved: &ResolvedPortfolioAggregationInputs,
) -> ApplicationResult<()> {
    if resolved.exact_scope != context.scope
        || resolved.portfolios.is_empty()
        || resolved.convention.value().reference() != context.metric_convention.reference()
        || resolved.convention.value().content_hash() != context.metric_convention.content_hash()
        || resolved.benchmark.value().reference() != context.benchmark.reference()
        || resolved.benchmark.value().content_hash() != context.benchmark.content_hash()
        || &resolved.benchmark_snapshot != resolved.benchmark.value().position_snapshot()
    {
        return Err(integrity());
    }
    scope.authorize(&context.owner)?;
    validate_visible_record(
        context,
        resolved.convention.value().owner(),
        None,
        resolved.convention.value().effective_from(),
        resolved.convention.value().effective_to(),
        resolved.convention.visible_at(),
    )?;
    validate_visible_record(
        context,
        resolved.benchmark.value().owner(),
        Some(resolved.benchmark.value().subject_ref()),
        resolved.benchmark.value().effective_from(),
        resolved.benchmark.value().effective_to(),
        resolved.benchmark.visible_at(),
    )?;
    let expected_members = context.scope.member_portfolios();
    if resolved.portfolios.len() != expected_members.len() {
        return Err(integrity());
    }
    for (record, expected) in resolved.portfolios.iter().zip(expected_members) {
        let portfolio = record.value();
        if exact_portfolio_ref(portfolio)? != *expected
            || portfolio.benchmark() != &context.benchmark
            || portfolio.metric_convention() != &context.metric_convention
        {
            return Err(integrity());
        }
        validate_visible_record(
            context,
            portfolio.owner(),
            Some(portfolio.subject_ref()),
            portfolio.effective_from(),
            portfolio.effective_to(),
            record.visible_at(),
        )?;
        validate_snapshot_binding(context, portfolio.position_snapshot())?;
    }
    validate_catalog_evidence(context, resolved)?;
    validate_snapshot_binding(context, &resolved.benchmark_snapshot)
}

fn validate_catalog_evidence(
    context: &NormalizedPortfolioContext,
    resolved: &ResolvedPortfolioAggregationInputs,
) -> ApplicationResult<()> {
    use crate::ports::PortfolioCatalogEvidenceRole as Role;

    let selected = resolved
        .catalog_evidence
        .iter()
        .filter(|binding| {
            matches!(
                binding.role(),
                Role::SelectedBook | Role::SelectedGroup | Role::SelectedPortfolio
            )
        })
        .collect::<Vec<_>>();
    let members = resolved
        .catalog_evidence
        .iter()
        .filter(|binding| binding.role() == Role::MemberPortfolio)
        .collect::<Vec<_>>();
    let benchmark = resolved
        .catalog_evidence
        .iter()
        .filter(|binding| binding.role() == Role::Benchmark)
        .collect::<Vec<_>>();
    let convention = resolved
        .catalog_evidence
        .iter()
        .filter(|binding| binding.role() == Role::MetricConvention)
        .collect::<Vec<_>>();
    if selected.len() != 1
        || members.len() != resolved.portfolios.len()
        || benchmark.len() != 1
        || convention.len() != 1
        || resolved.catalog_evidence.len() != members.len() + 3
        || !matches!(
            (selected[0].role(), context.scope.selected()),
            (
                Role::SelectedBook,
                crate::ports::ExactPortfolioScopeKind::Book(_)
            ) | (
                Role::SelectedGroup,
                crate::ports::ExactPortfolioScopeKind::Group(_)
            ) | (
                Role::SelectedPortfolio,
                crate::ports::ExactPortfolioScopeKind::Portfolio(_)
            )
        )
        || !catalog_binding_matches_lineage(
            selected[0],
            selected_scope_lineage(context.scope.selected()),
        )
        || !catalog_binding_matches_definition(
            benchmark[0],
            resolved.benchmark.value().reference(),
            resolved.benchmark.value().content_hash(),
            resolved.benchmark.visible_at(),
            resolved.benchmark.value().effective_from(),
            resolved.benchmark.value().effective_to(),
        )
        || !catalog_binding_matches_definition(
            convention[0],
            resolved.convention.value().reference(),
            resolved.convention.value().content_hash(),
            resolved.convention.visible_at(),
            resolved.convention.value().effective_from(),
            resolved.convention.value().effective_to(),
        )
        || members
            .iter()
            .zip(&resolved.portfolios)
            .any(|(binding, record)| {
                !catalog_binding_matches_definition(
                    binding,
                    record.value().reference(),
                    record.value().content_hash(),
                    record.visible_at(),
                    record.value().effective_from(),
                    record.value().effective_to(),
                )
            })
    {
        return Err(integrity());
    }
    for binding in &resolved.catalog_evidence {
        validate_visible_record(
            context,
            &context.owner,
            None,
            binding.effective_from(),
            binding.effective_to(),
            binding.visible_at(),
        )?;
    }
    Ok(())
}

fn catalog_binding_matches_lineage(
    binding: &crate::ports::PortfolioCatalogEvidenceBinding,
    reference: &LineageRef,
) -> bool {
    reference.version() == Some(binding.reference().version())
        && reference.object_id() == binding.reference().id()
        && reference.content_hash() == Some(binding.content_hash())
}

#[allow(clippy::too_many_arguments)]
fn catalog_binding_matches_definition(
    binding: &crate::ports::PortfolioCatalogEvidenceBinding,
    reference: &ficant_domain::primitives::VersionRef,
    content_hash: &ContentHash,
    visible_at: &MarketTime,
    effective_from: &MarketTime,
    effective_to: &MarketTime,
) -> bool {
    binding.reference() == reference
        && binding.content_hash() == content_hash
        && binding.visible_at() == visible_at
        && binding.effective_from() == effective_from
        && binding.effective_to() == effective_to
}

fn validate_visible_record(
    context: &NormalizedPortfolioContext,
    owner: &OwnerRef,
    subject: Option<&ficant_domain::primitives::VersionRef>,
    effective_from: &ficant_domain::primitives::MarketTime,
    effective_to: &ficant_domain::primitives::MarketTime,
    visible_at: &ficant_domain::primitives::MarketTime,
) -> ApplicationResult<()> {
    if owner != &context.owner
        || subject.is_some_and(|subject| subject != &context.subject_ref)
        || effective_from.instant() > context.valuation_at.instant()
        || effective_to.instant() <= context.valuation_at.instant()
        || visible_at.instant() > context.knowledge_at.instant()
    {
        return Err(integrity());
    }
    Ok(())
}

fn validate_snapshot_binding(
    context: &NormalizedPortfolioContext,
    binding: &PortfolioSnapshotBinding,
) -> ApplicationResult<()> {
    if binding.observed_at() != &context.valuation_at
        || binding.visible_at().instant() > context.knowledge_at.instant()
        || binding.visible_at().instant() < binding.observed_at().instant()
    {
        return Err(integrity());
    }
    Ok(())
}

fn validate_position_snapshot(
    context: &NormalizedPortfolioContext,
    portfolio: &Portfolio,
    snapshot: &PositionSnapshot,
) -> ApplicationResult<()> {
    let binding = portfolio.position_snapshot();
    if snapshot.id() != binding.snapshot_id()
        || snapshot.content_hash() != binding.content_hash()
        || snapshot.owner() != &context.owner
        || snapshot.subject_ref() != &context.subject_ref
        || snapshot.observed_at() != binding.observed_at()
        || snapshot.visible_at() != binding.visible_at()
    {
        return Err(integrity());
    }
    Ok(())
}

fn validate_benchmark_snapshot(
    context: &NormalizedPortfolioContext,
    resolved: &ResolvedPortfolioAggregationInputs,
    snapshot: &PositionSnapshot,
) -> ApplicationResult<()> {
    if snapshot.id() != resolved.benchmark_snapshot.snapshot_id()
        || snapshot.content_hash() != resolved.benchmark_snapshot.content_hash()
        || snapshot.owner() != &context.owner
        || snapshot.subject_ref() != &context.subject_ref
        || snapshot.observed_at() != resolved.benchmark_snapshot.observed_at()
        || snapshot.visible_at() != resolved.benchmark_snapshot.visible_at()
    {
        return Err(integrity());
    }
    Ok(())
}

fn validate_views(snapshot: &PositionSnapshot, views: &PositionViews) -> ApplicationResult<()> {
    if views.snapshot.id() != snapshot.id()
        || views.snapshot.content_hash() != snapshot.content_hash()
        || views.positions.len() != snapshot.positions().len()
        || views.coverage.imported_position_count()
            != u64::try_from(snapshot.positions().len()).map_err(|_| validation())?
    {
        return Err(integrity());
    }
    Ok(())
}

fn validate_risk(
    snapshot: &PositionSnapshot,
    risk: &PortfolioKeyRateExposure,
) -> ApplicationResult<()> {
    if risk.position_snapshot_id() != snapshot.id() || risk.totals().is_empty() {
        return Err(integrity());
    }
    Ok(())
}

fn aggregate_existing_krd(member_totals: &[Vec<FactorDv01>]) -> ApplicationResult<Vec<FactorDv01>> {
    let first = member_totals.first().ok_or_else(validation)?;
    if first.is_empty() {
        return Err(validation());
    }
    let mut totals = first.clone();
    for member in &member_totals[1..] {
        if member.len() != totals.len() {
            return Err(lineage());
        }
        for (total, value) in totals.iter_mut().zip(member) {
            if total.factor_id() != value.factor_id()
                || total.factor_definition_hash() != value.factor_definition_hash()
                || total.unit() != value.unit()
            {
                return Err(lineage());
            }
            *total = FactorDv01::new(
                total.factor_id(),
                total.factor_definition_hash().clone(),
                total
                    .value()
                    .checked_add(value.value())
                    .map_err(map_domain_error)?,
                total.unit().clone(),
            )
            .map_err(map_domain_error)?;
        }
    }
    Ok(totals)
}

fn aggregation_request_fingerprint(
    context: &NormalizedPortfolioContext,
    resolved: &ResolvedPortfolioAggregationInputs,
    analytics_fingerprints: &[ContentHash],
    risk_input_sets: &[&[PortfolioRiskNamedEvidenceBinding]],
) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, b"ficant.portfolio-overview-request.v1");
    append(&mut bytes, context.owner.tenant_id().as_str().as_bytes());
    append(&mut bytes, context.owner.owner_id().as_str().as_bytes());
    append_version_ref(
        &mut bytes,
        context.subject_ref.id().as_str().as_bytes(),
        context.subject_ref.version().get(),
        &ContentHash::digest(b"subject-ref-version-only"),
    );
    append_scope(&mut bytes, &context.scope);
    append_market_time(&mut bytes, &context.valuation_at);
    append_market_time(&mut bytes, &context.knowledge_at);
    append(&mut bytes, &[currency_code(context.currency)]);
    append(
        &mut bytes,
        context.currency_unit.unit_id().as_str().as_bytes(),
    );
    append(
        &mut bytes,
        &context.currency_unit.version().get().to_be_bytes(),
    );
    append(&mut bytes, &[look_through_code(context.look_through)]);
    append(&mut bytes, &[period_code(context.period)]);
    append_market_time(&mut bytes, &context.period_from);
    append_market_time(&mut bytes, &context.period_to);
    append(&mut bytes, context.benchmark.content_hash().as_bytes());
    append(
        &mut bytes,
        context.metric_convention.content_hash().as_bytes(),
    );
    for portfolio in &resolved.portfolios {
        append(&mut bytes, portfolio.value().content_hash().as_bytes());
        append_snapshot_binding(&mut bytes, portfolio.value().position_snapshot());
    }
    append(
        &mut bytes,
        resolved.benchmark.value().content_hash().as_bytes(),
    );
    append_snapshot_binding(&mut bytes, &resolved.benchmark_snapshot);
    append(
        &mut bytes,
        resolved.convention.value().content_hash().as_bytes(),
    );
    append_catalog_evidence(&mut bytes, &resolved.catalog_evidence);
    for fingerprint in analytics_fingerprints {
        append(&mut bytes, fingerprint.as_bytes());
    }
    for inputs in risk_input_sets {
        append_risk_named_evidence(&mut bytes, inputs);
    }
    ContentHash::digest(&bytes)
}

fn validate_formal_evidence(
    context: &NormalizedPortfolioContext,
    draft: &PortfolioOverviewDraft,
    evidence: &FormalOutputEvidence,
) -> ApplicationResult<()> {
    let expected_implementations = draft.implementation_bindings()?;
    let subject_hash = match evidence.subject().reference() {
        FormalInputReference::Object(reference)
            if reference.object_id() == context.subject_ref.id()
                && reference.version() == Some(context.subject_ref.version()) =>
        {
            reference.content_hash().ok_or_else(integrity)?
        }
        FormalInputReference::Object(_) | FormalInputReference::Named(_) => {
            return Err(integrity());
        }
    };
    let expected_inputs = draft.formal_input_bindings(&context.owner, subject_hash)?;
    if evidence.schema_id() != PORTFOLIO_OVERVIEW_SCHEMA_ID
        || evidence.subject().owner() != &context.owner
        || evidence.result_hash() != &ContentHash::digest(&draft.canonical_payload())
        || evidence.implementations() != expected_implementations
        || evidence.consumed_inputs() != expected_inputs
    {
        return Err(integrity());
    }
    Ok(())
}

fn exact_portfolio_ref(portfolio: &Portfolio) -> ApplicationResult<LineageRef> {
    LineageRef::new(
        portfolio.reference().id().clone(),
        Some(portfolio.reference().version()),
        Some(portfolio.content_hash().clone()),
    )
    .map_err(map_domain_error)
}

fn require_exact_lineage_set(values: &[LineageRef]) -> ApplicationResult<()> {
    if values
        .iter()
        .any(|reference| reference.version().is_none() || reference.content_hash().is_none())
        || values.windows(2).any(|pair| {
            pair[0]
                .object_id()
                .cmp(pair[1].object_id())
                .then_with(|| pair[0].version().cmp(&pair[1].version()))
                .then_with(|| pair[0].content_hash().cmp(&pair[1].content_hash()))
                .is_ge()
        })
    {
        return Err(lineage());
    }
    Ok(())
}

fn is_stale(
    knowledge_at: &ficant_domain::primitives::MarketTime,
    observed_at: &ficant_domain::primitives::MarketTime,
    freshness_limit_seconds: u64,
) -> ApplicationResult<bool> {
    let age = knowledge_at
        .instant()
        .signed_duration_since(observed_at.instant());
    if age.num_nanoseconds().is_some_and(|nanos| nanos < 0) {
        return Err(integrity());
    }
    let limit = i64::try_from(freshness_limit_seconds).map_err(|_| validation())?;
    Ok(age.num_seconds() > limit)
}

fn append_scope(bytes: &mut Vec<u8>, scope: &crate::ports::ExactPortfolioScope) {
    match scope.selected() {
        crate::ports::ExactPortfolioScopeKind::Book(reference) => {
            append(bytes, &[1]);
            append_lineage(bytes, reference);
        }
        crate::ports::ExactPortfolioScopeKind::Group(reference) => {
            append(bytes, &[2]);
            append_lineage(bytes, reference);
        }
        crate::ports::ExactPortfolioScopeKind::Portfolio(reference) => {
            append(bytes, &[3]);
            append_lineage(bytes, reference);
        }
    }
    for member in scope.member_portfolios() {
        append_lineage(bytes, member);
    }
}

fn append_snapshot_binding(bytes: &mut Vec<u8>, binding: &PortfolioSnapshotBinding) {
    append(bytes, binding.snapshot_id().as_str().as_bytes());
    append(bytes, binding.content_hash().as_bytes());
    append_market_time(bytes, binding.observed_at());
    append_market_time(bytes, binding.visible_at());
}

fn append_market_time(bytes: &mut Vec<u8>, time: &ficant_domain::primitives::MarketTime) {
    append(bytes, &time.instant().timestamp().to_be_bytes());
    append(
        bytes,
        &time.instant().timestamp_subsec_nanos().to_be_bytes(),
    );
    append(bytes, time.market_timezone().as_bytes());
    append(bytes, time.local_trading_date().to_string().as_bytes());
}

fn append_basic_metrics(bytes: &mut Vec<u8>, metrics: &PortfolioBasicMetrics) {
    append_decimal(bytes, &metrics.market_value);
    append_decimal(bytes, &metrics.economic_pnl);
    for value in [
        metrics.weighted_ytm.as_ref(),
        metrics.modified_duration.as_ref(),
        metrics.convexity.as_ref(),
        metrics.weighted_coupon_rate.as_ref(),
        metrics.weighted_remaining_years.as_ref(),
    ] {
        match value {
            Some(value) => {
                append(bytes, &[1]);
                append_decimal(bytes, value);
            }
            None => append(bytes, &[0]),
        }
    }
    append_decimal(bytes, &metrics.dv01);
}

fn append_bond_analysis(bytes: &mut Vec<u8>, analysis: &PortfolioBondAnalysisResult) {
    let metadata = analysis.metadata();
    for value in [
        metadata.schema_id(),
        metadata.engine_id(),
        metadata.engine_version(),
        metadata.algorithm_id(),
        metadata.convention_profile(),
    ] {
        append(bytes, value.as_bytes());
    }
    append(bytes, &metadata.algorithm_version().to_be_bytes());
    append(bytes, &metadata.abi_version().to_be_bytes());
    append(bytes, metadata.subject_ref().id().as_str().as_bytes());
    append(bytes, &metadata.subject_ref().version().get().to_be_bytes());
    append(
        bytes,
        metadata
            .request_evidence()
            .canonical_parameters_sha256()
            .as_bytes(),
    );
    append(
        bytes,
        metadata.request_evidence().request_fingerprint().as_bytes(),
    );
    for requirement in [
        analysis.units().currency_amount(),
        analysis.units().price_per_100(),
        analysis.units().rate(),
        analysis.units().years(),
        analysis.units().years_squared(),
        analysis.units().dv01_per_100(),
        analysis.units().dv01(),
        analysis.units().dimensionless(),
        analysis.units().contract_count(),
    ] {
        append(bytes, requirement.reference().unit_id().as_str().as_bytes());
        append(
            bytes,
            &requirement.reference().version().get().to_be_bytes(),
        );
        append(bytes, requirement.expected_dimension().as_bytes());
    }
    for cashflow in analysis.analytics().cashflows() {
        append(bytes, &cashflow.sequence().to_be_bytes());
        append(bytes, cashflow.nominal_date().to_string().as_bytes());
        append(bytes, cashflow.payment_date().to_string().as_bytes());
        for value in [cashflow.coupon(), cashflow.principal(), cashflow.total()] {
            append(bytes, &value.scaled().to_be_bytes());
        }
    }
    let measures = analysis.analytics().measures();
    for value in [
        measures.accrued_interest(),
        measures.clean_price(),
        measures.dirty_price(),
        measures.yield_to_maturity(),
        measures.macaulay_duration(),
        measures.modified_duration(),
        measures.convexity(),
        measures.dv01(),
    ] {
        append(bytes, &value.scaled().to_be_bytes());
    }
}

fn append_analytics_authority_evidence(
    bytes: &mut Vec<u8>,
    evidence: &[crate::ports::PortfolioAnalyticsEvidenceBinding],
) {
    for binding in evidence {
        append(bytes, &[binding.kind as u8]);
        append(bytes, binding.object_id.as_str().as_bytes());
        append(
            bytes,
            &binding.version.map_or(0, Version::get).to_be_bytes(),
        );
        append(bytes, binding.content_hash.as_bytes());
        append_optional_market_time(bytes, binding.observed_at.as_ref());
        append_optional_market_time(bytes, binding.visible_at.as_ref());
        append_optional_market_time(bytes, binding.effective_from.as_ref());
        append_optional_market_time(bytes, binding.effective_to.as_ref());
    }
}

fn append_catalog_evidence(
    bytes: &mut Vec<u8>,
    evidence: &[crate::ports::PortfolioCatalogEvidenceBinding],
) {
    for binding in evidence {
        append(bytes, &[binding.role() as u8]);
        append(bytes, binding.reference().id().as_str().as_bytes());
        append(bytes, &binding.reference().version().get().to_be_bytes());
        append(bytes, binding.content_hash().as_bytes());
        append_market_time(bytes, binding.visible_at());
        append_market_time(bytes, binding.effective_from());
        append_market_time(bytes, binding.effective_to());
    }
}

fn append_risk_named_evidence(bytes: &mut Vec<u8>, evidence: &[PortfolioRiskNamedEvidenceBinding]) {
    for binding in evidence {
        append(bytes, &[binding.kind as u8]);
        append(bytes, binding.identity.as_bytes());
        append(bytes, binding.content_hash.as_bytes());
        append_optional_market_time(bytes, binding.observed_at.as_ref());
        append_optional_market_time(bytes, binding.visible_at.as_ref());
        append_optional_market_time(bytes, binding.effective_from.as_ref());
        append_optional_market_time(bytes, binding.effective_to.as_ref());
    }
}

fn append_optional_market_time(bytes: &mut Vec<u8>, value: Option<&MarketTime>) {
    match value {
        Some(value) => {
            append(bytes, &[1]);
            append_market_time(bytes, value);
        }
        None => append(bytes, &[0]),
    }
}

fn append_krd(bytes: &mut Vec<u8>, summary: &PortfolioKrdSummary) {
    for factor in &summary.totals {
        append(bytes, factor.factor_id().as_bytes());
        append(bytes, factor.factor_definition_hash().as_bytes());
        append(bytes, &factor.value().scaled().to_be_bytes());
        append(bytes, factor.unit().unit_id().as_str().as_bytes());
        append(bytes, &factor.unit().version().get().to_be_bytes());
    }
    append_decimal(bytes, &summary.parallel_dv01);
}

fn append_coverage(bytes: &mut Vec<u8>, coverage: &PortfolioCoverage) {
    append(bytes, &coverage.participation.canonical_bytes());
    for reason in &coverage.missing_reasons {
        append(bytes, &[coverage_reason_code(*reason)]);
    }
}

fn append_decimal(bytes: &mut Vec<u8>, value: &DecimalValue) {
    append(bytes, value.coefficient().as_bytes());
    append(bytes, &value.scale().to_be_bytes());
    append(bytes, value.unit().unit_id().as_str().as_bytes());
    append(bytes, &value.unit().version().get().to_be_bytes());
}

fn append_version_ref(
    bytes: &mut Vec<u8>,
    identity: &[u8],
    version: u64,
    content_hash: &ContentHash,
) {
    append(bytes, identity);
    append(bytes, &version.to_be_bytes());
    append(bytes, content_hash.as_bytes());
}

fn append_lineage(bytes: &mut Vec<u8>, reference: &LineageRef) {
    append(bytes, reference.object_id().as_str().as_bytes());
    append(
        bytes,
        &reference.version().map_or(0, Version::get).to_be_bytes(),
    );
    append(
        bytes,
        reference
            .content_hash()
            .map_or(&[][..], |value| value.as_bytes().as_slice()),
    );
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn authorize_portfolio_read(
    principal: &AuthorizedPrincipal,
    owner: &OwnerRef,
) -> ApplicationResult<()> {
    principal.require_role(PlatformRole::Researcher)?;
    if !principal.has_scope(crate::ports::PORTFOLIO_READ_SCOPE) {
        return Err(forbidden());
    }
    principal.access_scope().authorize(owner)
}

const fn coverage_reason_code(reason: PortfolioCoverageReason) -> u8 {
    match reason {
        PortfolioCoverageReason::ShortPositionExcludedFromWeightedAverages => 1,
        PortfolioCoverageReason::NonBondExcludedFromWeightedAverages => 2,
        PortfolioCoverageReason::MissingBondMetricExcludedFromWeightedAverages => 3,
        PortfolioCoverageReason::PositionExcludedFromPortfolioRisk => 4,
        PortfolioCoverageReason::BenchmarkPositionExcludedFromPortfolioRisk => 5,
    }
}

const fn metric_mode_code(mode: PortfolioMetricDataMode) -> u8 {
    match mode {
        PortfolioMetricDataMode::Real => 1,
        PortfolioMetricDataMode::Partial => 2,
        PortfolioMetricDataMode::Stale => 3,
    }
}

const fn currency_code(currency: crate::ports::PortfolioCurrencyMode) -> u8 {
    match currency {
        crate::ports::PortfolioCurrencyMode::Original => 1,
        crate::ports::PortfolioCurrencyMode::Cny => 2,
    }
}

const fn look_through_code(mode: crate::ports::PortfolioLookThroughMode) -> u8 {
    match mode {
        crate::ports::PortfolioLookThroughMode::None => 1,
        crate::ports::PortfolioLookThroughMode::Consolidated => 2,
        crate::ports::PortfolioLookThroughMode::Separate => 3,
    }
}

const fn period_code(period: crate::ports::PortfolioPeriodPreset) -> u8 {
    match period {
        crate::ports::PortfolioPeriodPreset::OneDay => 1,
        crate::ports::PortfolioPeriodPreset::SevenDays => 2,
        crate::ports::PortfolioPeriodPreset::ThirtyDays => 3,
        crate::ports::PortfolioPeriodPreset::YearToDate => 4,
        crate::ports::PortfolioPeriodPreset::OneYear => 5,
    }
}

fn decimal_to_fixed(value: &DecimalValue) -> ApplicationResult<FixedDecimal> {
    let coefficient = value
        .coefficient()
        .parse::<i128>()
        .map_err(|_| validation())?;
    let scaled = if value.scale() <= FIXED_DECIMAL_SCALE {
        let factor = checked_power_of_ten(FIXED_DECIMAL_SCALE - value.scale())?;
        coefficient.checked_mul(factor).ok_or_else(validation)?
    } else {
        let divisor = checked_power_of_ten(value.scale() - FIXED_DECIMAL_SCALE)?;
        if coefficient % divisor != 0 {
            return Err(validation());
        }
        coefficient / divisor
    };
    Ok(FixedDecimal::from_scaled(scaled))
}

fn fixed_to_decimal(
    value: FixedDecimal,
    output_scale: u32,
    unit: UnitRef,
) -> ApplicationResult<DecimalValue> {
    if output_scale > MAX_DECIMAL_SCALE {
        return Err(validation());
    }
    let coefficient = if output_scale <= FIXED_DECIMAL_SCALE {
        let divisor = checked_power_of_ten(FIXED_DECIMAL_SCALE - output_scale)?;
        round_div_ties_even(value.scaled(), divisor)?
    } else {
        value
            .scaled()
            .checked_mul(checked_power_of_ten(output_scale - FIXED_DECIMAL_SCALE)?)
            .ok_or_else(validation)?
    };
    DecimalValue::new(coefficient.to_string(), output_scale, unit).map_err(map_domain_error)
}

fn checked_power_of_ten(exponent: u32) -> ApplicationResult<i128> {
    (0..exponent).try_fold(1_i128, |value, _| {
        value.checked_mul(10).ok_or_else(validation)
    })
}

fn round_div_ties_even(value: i128, divisor: i128) -> ApplicationResult<i128> {
    if divisor <= 0 {
        return Err(validation());
    }
    let negative = value < 0;
    let magnitude = value.unsigned_abs();
    let divisor = u128::try_from(divisor).map_err(|_| validation())?;
    let mut quotient = magnitude / divisor;
    let remainder = magnitude % divisor;
    let distance_to_divisor = divisor - remainder;
    if remainder > distance_to_divisor || (remainder == distance_to_divisor && quotient % 2 == 1) {
        quotient = quotient.checked_add(1).ok_or_else(validation)?;
    }
    if !negative {
        return i128::try_from(quotient).map_err(|_| validation());
    }
    if quotient == i128::MIN.unsigned_abs() {
        return Ok(i128::MIN);
    }
    i128::try_from(quotient)
        .ok()
        .and_then(i128::checked_neg)
        .ok_or_else(validation)
}

fn invalid_unit() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn lineage() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

fn integrity() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
