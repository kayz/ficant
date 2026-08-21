use async_trait::async_trait;
use chrono::NaiveDate;
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, CalendarRequirement, FixedDecimal,
};
use ficant_domain::market::Valuation;
use ficant_domain::portfolio::{
    Benchmark, BenchmarkRef, Book, Portfolio, PortfolioGroup, PortfolioMetricConvention,
    PortfolioMetricConventionRef, PortfolioStatus,
};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};

use super::fingerprint::{FingerprintBuilder, market_time_bytes, owner_bytes, version_ref_bytes};
use super::{AccessScope, ApplicationResult, OperationFingerprint};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::DomainErrorCode;

pub const PORTFOLIO_READ_SCOPE: &str = "portfolio:read";
pub const PORTFOLIO_CATALOG_MAX_PAGE_SIZE: u32 = 1_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleCatalogRecord<T> {
    value: T,
    visible_at: MarketTime,
}

impl<T> VisibleCatalogRecord<T> {
    #[must_use]
    pub const fn new(value: T, visible_at: MarketTime) -> Self {
        Self { value, visible_at }
    }

    #[must_use]
    pub const fn value(&self) -> &T {
        &self.value
    }

    #[must_use]
    pub const fn visible_at(&self) -> &MarketTime {
        &self.visible_at
    }

    #[must_use]
    pub fn into_value(self) -> T {
        self.value
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioCatalogTemporalScope {
    owner: OwnerRef,
    subject_ref: VersionRef,
    as_of: MarketTime,
    knowledge_at: MarketTime,
}

impl PortfolioCatalogTemporalScope {
    /// Creates an exact owner/Subject bitemporal read boundary.
    ///
    /// # Errors
    ///
    /// Returns validation failure when knowledge precedes the requested valuation instant.
    pub fn new(
        owner: OwnerRef,
        subject_ref: VersionRef,
        as_of: MarketTime,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<Self> {
        if knowledge_at.instant() < as_of.instant() {
            return Err(map_domain_error(DomainErrorCode::InvalidEffectiveTime));
        }
        Ok(Self {
            owner,
            subject_ref,
            as_of,
            knowledge_at,
        })
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
    pub const fn as_of(&self) -> &MarketTime {
        &self.as_of
    }

    #[must_use]
    pub const fn knowledge_at(&self) -> &MarketTime {
        &self.knowledge_at
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioCatalogFilter {
    temporal: PortfolioCatalogTemporalScope,
    statuses: Vec<PortfolioStatus>,
    normalized_search: Option<String>,
    fingerprint: OperationFingerprint,
}

impl PortfolioCatalogFilter {
    /// Builds one normalized, stable catalog filter.
    ///
    /// # Errors
    ///
    /// Returns validation failure for blank/padded/control search text or overlong input.
    pub fn new(
        temporal: PortfolioCatalogTemporalScope,
        mut statuses: Vec<PortfolioStatus>,
        search: Option<String>,
    ) -> ApplicationResult<Self> {
        statuses.sort_by_key(|status| portfolio_status_code(*status));
        statuses.dedup();
        let normalized_search = search.map(|value| normalize_search(&value)).transpose()?;

        let mut canonical = FingerprintBuilder::new("portfolio-catalog-filter/v1");
        canonical.field(2, &owner_bytes(temporal.owner()));
        canonical.field(3, &version_ref_bytes(temporal.subject_ref()));
        canonical.field(4, &market_time_bytes(temporal.as_of()));
        canonical.field(5, &market_time_bytes(temporal.knowledge_at()));
        for status in &statuses {
            canonical.field(6, &[portfolio_status_code(*status)]);
        }
        if let Some(search) = &normalized_search {
            canonical.field(7, search.as_bytes());
        }
        let fingerprint = canonical.finish();
        Ok(Self {
            temporal,
            statuses,
            normalized_search,
            fingerprint,
        })
    }

    #[must_use]
    pub const fn temporal(&self) -> &PortfolioCatalogTemporalScope {
        &self.temporal
    }

    #[must_use]
    pub fn statuses(&self) -> &[PortfolioStatus] {
        &self.statuses
    }

    #[must_use]
    pub fn normalized_search(&self) -> Option<&str> {
        self.normalized_search.as_deref()
    }

    #[must_use]
    pub const fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }

    #[must_use]
    pub fn accepts_status(&self, status: PortfolioStatus) -> bool {
        self.statuses.is_empty()
            || self
                .statuses
                .binary_search_by_key(&portfolio_status_code(status), |candidate| {
                    portfolio_status_code(*candidate)
                })
                .is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PortfolioCatalogSortKey {
    book_code: String,
    group_path: String,
    portfolio_code: String,
    version: u64,
}

impl PortfolioCatalogSortKey {
    /// Creates the exact frozen R8A catalog order key.
    ///
    /// # Errors
    ///
    /// Returns validation failure when a component is blank or contains a cursor delimiter.
    pub fn new(
        book_code: String,
        group_path: String,
        portfolio_code: String,
        version: u64,
    ) -> ApplicationResult<Self> {
        if version == 0
            || [&book_code, &group_path, &portfolio_code]
                .iter()
                .any(|value| {
                    value.is_empty()
                        || value != &value.trim()
                        || value.chars().any(char::is_control)
                })
        {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        Ok(Self {
            book_code,
            group_path,
            portfolio_code,
            version,
        })
    }

    #[must_use]
    pub fn book_code(&self) -> &str {
        &self.book_code
    }

    #[must_use]
    pub fn group_path(&self) -> &str {
        &self.group_path
    }

    #[must_use]
    pub fn portfolio_code(&self) -> &str {
        &self.portfolio_code
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioCatalogEntry {
    record: VisibleCatalogRecord<Portfolio>,
    sort_key: PortfolioCatalogSortKey,
}

impl PortfolioCatalogEntry {
    #[must_use]
    pub const fn new(
        record: VisibleCatalogRecord<Portfolio>,
        sort_key: PortfolioCatalogSortKey,
    ) -> Self {
        Self { record, sort_key }
    }

    #[must_use]
    pub const fn record(&self) -> &VisibleCatalogRecord<Portfolio> {
        &self.record
    }

    #[must_use]
    pub const fn sort_key(&self) -> &PortfolioCatalogSortKey {
        &self.sort_key
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PortfolioCatalogSnapshot {
    books: Vec<VisibleCatalogRecord<Book>>,
    groups: Vec<VisibleCatalogRecord<PortfolioGroup>>,
    portfolios: Vec<VisibleCatalogRecord<Portfolio>>,
    benchmarks: Vec<VisibleCatalogRecord<Benchmark>>,
    metric_conventions: Vec<VisibleCatalogRecord<PortfolioMetricConvention>>,
}

impl PortfolioCatalogSnapshot {
    #[must_use]
    pub fn new(
        books: Vec<VisibleCatalogRecord<Book>>,
        groups: Vec<VisibleCatalogRecord<PortfolioGroup>>,
        portfolios: Vec<VisibleCatalogRecord<Portfolio>>,
        benchmarks: Vec<VisibleCatalogRecord<Benchmark>>,
        metric_conventions: Vec<VisibleCatalogRecord<PortfolioMetricConvention>>,
    ) -> Self {
        Self {
            books,
            groups,
            portfolios,
            benchmarks,
            metric_conventions,
        }
    }

    #[must_use]
    pub fn books(&self) -> &[VisibleCatalogRecord<Book>] {
        &self.books
    }

    #[must_use]
    pub fn groups(&self) -> &[VisibleCatalogRecord<PortfolioGroup>] {
        &self.groups
    }

    #[must_use]
    pub fn portfolios(&self) -> &[VisibleCatalogRecord<Portfolio>] {
        &self.portfolios
    }

    #[must_use]
    pub fn benchmarks(&self) -> &[VisibleCatalogRecord<Benchmark>] {
        &self.benchmarks
    }

    #[must_use]
    pub fn metric_conventions(&self) -> &[VisibleCatalogRecord<PortfolioMetricConvention>] {
        &self.metric_conventions
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioCatalogPage {
    books: Vec<VisibleCatalogRecord<Book>>,
    groups: Vec<VisibleCatalogRecord<PortfolioGroup>>,
    portfolios: Vec<PortfolioCatalogEntry>,
    next_cursor: Option<String>,
    request_fingerprint: OperationFingerprint,
}

impl PortfolioCatalogPage {
    #[must_use]
    pub fn new(
        books: Vec<VisibleCatalogRecord<Book>>,
        groups: Vec<VisibleCatalogRecord<PortfolioGroup>>,
        portfolios: Vec<PortfolioCatalogEntry>,
        next_cursor: Option<String>,
        request_fingerprint: OperationFingerprint,
    ) -> Self {
        Self {
            books,
            groups,
            portfolios,
            next_cursor,
            request_fingerprint,
        }
    }

    #[must_use]
    pub fn books(&self) -> &[VisibleCatalogRecord<Book>] {
        &self.books
    }

    #[must_use]
    pub fn groups(&self) -> &[VisibleCatalogRecord<PortfolioGroup>] {
        &self.groups
    }

    #[must_use]
    pub fn portfolios(&self) -> &[PortfolioCatalogEntry] {
        &self.portfolios
    }

    #[must_use]
    pub fn next_cursor(&self) -> Option<&str> {
        self.next_cursor.as_deref()
    }

    #[must_use]
    pub const fn request_fingerprint(&self) -> &OperationFingerprint {
        &self.request_fingerprint
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactCatalogRead {
    temporal: PortfolioCatalogTemporalScope,
    reference: VersionRef,
    content_hash: ContentHash,
}

impl ExactCatalogRead {
    #[must_use]
    pub const fn new(
        temporal: PortfolioCatalogTemporalScope,
        reference: VersionRef,
        content_hash: ContentHash,
    ) -> Self {
        Self {
            temporal,
            reference,
            content_hash,
        }
    }

    #[must_use]
    pub const fn temporal(&self) -> &PortfolioCatalogTemporalScope {
        &self.temporal
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
pub enum PortfolioScopeSelector {
    Book(Ulid),
    Group(Ulid),
    Portfolio(Ulid),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioScopeAuthority {
    owner: OwnerRef,
    subject_ref: VersionRef,
}

impl PortfolioScopeAuthority {
    #[must_use]
    pub const fn new(owner: OwnerRef, subject_ref: VersionRef) -> Self {
        Self { owner, subject_ref }
    }

    #[must_use]
    pub const fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub const fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioCurrencyMode {
    Original,
    Cny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioLookThroughMode {
    None,
    Consolidated,
    Separate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioPeriodPreset {
    OneDay,
    SevenDays,
    ThirtyDays,
    YearToDate,
    OneYear,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioContextInput {
    pub scope: PortfolioScopeSelector,
    pub valuation_at: MarketTime,
    pub knowledge_at: MarketTime,
    pub currency: PortfolioCurrencyMode,
    pub look_through: PortfolioLookThroughMode,
    pub benchmark_id: Ulid,
    pub period: PortfolioPeriodPreset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExactPortfolioScopeKind {
    Book(LineageRef),
    Group(LineageRef),
    Portfolio(LineageRef),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactPortfolioScope {
    selected: ExactPortfolioScopeKind,
    member_portfolios: Vec<LineageRef>,
}

impl ExactPortfolioScope {
    #[must_use]
    pub fn new(selected: ExactPortfolioScopeKind, mut member_portfolios: Vec<LineageRef>) -> Self {
        member_portfolios.sort_by(|left, right| {
            left.object_id()
                .cmp(right.object_id())
                .then_with(|| left.version().cmp(&right.version()))
                .then_with(|| left.content_hash().cmp(&right.content_hash()))
        });
        member_portfolios.dedup_by(|left, right| left == right);
        Self {
            selected,
            member_portfolios,
        }
    }

    #[must_use]
    pub const fn selected(&self) -> &ExactPortfolioScopeKind {
        &self.selected
    }

    #[must_use]
    pub fn member_portfolios(&self) -> &[LineageRef] {
        &self.member_portfolios
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedPortfolioContext {
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PortfolioCatalogEvidenceRole {
    SelectedBook = 1,
    SelectedGroup = 2,
    SelectedPortfolio = 3,
    MemberPortfolio = 4,
    Benchmark = 5,
    MetricConvention = 6,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioCatalogEvidenceBinding {
    role: PortfolioCatalogEvidenceRole,
    reference: VersionRef,
    content_hash: ContentHash,
    visible_at: MarketTime,
    effective_from: MarketTime,
    effective_to: MarketTime,
}

impl PortfolioCatalogEvidenceBinding {
    /// Binds one exact, visible catalog object to its effective interval.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless the effective interval is non-empty.
    pub fn new(
        role: PortfolioCatalogEvidenceRole,
        reference: VersionRef,
        content_hash: ContentHash,
        visible_at: MarketTime,
        effective_from: MarketTime,
        effective_to: MarketTime,
    ) -> ApplicationResult<Self> {
        if effective_from.instant() >= effective_to.instant() {
            return Err(map_domain_error(DomainErrorCode::InvalidEffectiveTime));
        }
        Ok(Self {
            role,
            reference,
            content_hash,
            visible_at,
            effective_from,
            effective_to,
        })
    }

    #[must_use]
    pub const fn role(&self) -> PortfolioCatalogEvidenceRole {
        self.role
    }

    #[must_use]
    pub const fn reference(&self) -> &VersionRef {
        &self.reference
    }

    #[must_use]
    pub const fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    #[must_use]
    pub const fn visible_at(&self) -> &MarketTime {
        &self.visible_at
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedPortfolioContextResolution {
    context: NormalizedPortfolioContext,
    catalog_evidence: Vec<PortfolioCatalogEvidenceBinding>,
}

impl NormalizedPortfolioContextResolution {
    /// Carries the normalized context with the exact catalog records used to derive it.
    ///
    /// # Errors
    ///
    /// Fails closed on empty, duplicate, future-visible, or ineffective catalog evidence.
    pub fn new(
        context: NormalizedPortfolioContext,
        mut catalog_evidence: Vec<PortfolioCatalogEvidenceBinding>,
    ) -> ApplicationResult<Self> {
        catalog_evidence.sort_by(|left, right| {
            left.role
                .cmp(&right.role)
                .then_with(|| left.reference.id().cmp(right.reference.id()))
                .then_with(|| left.reference.version().cmp(&right.reference.version()))
        });
        if catalog_evidence.is_empty()
            || catalog_evidence
                .windows(2)
                .any(|pair| pair[0].role == pair[1].role && pair[0].reference == pair[1].reference)
            || catalog_evidence.iter().any(|binding| {
                binding.visible_at.instant() > context.knowledge_at.instant()
                    || binding.effective_from.instant() > context.valuation_at.instant()
                    || binding.effective_to.instant() <= context.valuation_at.instant()
            })
        {
            return Err(map_domain_error(DomainErrorCode::InvalidEffectiveTime));
        }
        Ok(Self {
            context,
            catalog_evidence,
        })
    }

    #[must_use]
    pub const fn context(&self) -> &NormalizedPortfolioContext {
        &self.context
    }

    #[must_use]
    pub fn catalog_evidence(&self) -> &[PortfolioCatalogEvidenceBinding] {
        &self.catalog_evidence
    }

    #[must_use]
    pub fn into_context(self) -> NormalizedPortfolioContext {
        self.context
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        NormalizedPortfolioContext,
        Vec<PortfolioCatalogEvidenceBinding>,
    ) {
        (self.context, self.catalog_evidence)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPortfolioAggregationInputs {
    pub exact_scope: ExactPortfolioScope,
    pub portfolios: Vec<VisibleCatalogRecord<Portfolio>>,
    pub convention: VisibleCatalogRecord<PortfolioMetricConvention>,
    pub benchmark: VisibleCatalogRecord<Benchmark>,
    pub benchmark_snapshot: ficant_domain::portfolio::PortfolioSnapshotBinding,
    pub catalog_evidence: Vec<PortfolioCatalogEvidenceBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioAnalyticsAuthorityQuery {
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub position_snapshot: ficant_domain::portfolio::PortfolioSnapshotBinding,
    pub valuation_at: MarketTime,
    pub knowledge_at: MarketTime,
}

impl PortfolioAnalyticsAuthorityQuery {
    /// Creates the exact bitemporal boundary for one `PositionSnapshot` authority lookup.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the snapshot or knowledge boundary is in the future.
    pub fn new(
        owner: OwnerRef,
        subject_ref: VersionRef,
        position_snapshot: ficant_domain::portfolio::PortfolioSnapshotBinding,
        valuation_at: MarketTime,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<Self> {
        if position_snapshot.observed_at().instant() > valuation_at.instant()
            || position_snapshot.visible_at().instant() > knowledge_at.instant()
            || knowledge_at.instant() < valuation_at.instant()
        {
            return Err(map_domain_error(DomainErrorCode::InvalidEffectiveTime));
        }
        Ok(Self {
            owner,
            subject_ref,
            position_snapshot,
            valuation_at,
            knowledge_at,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioImmutableSnapshotAuthority {
    pub id: Ulid,
    pub content_hash: ContentHash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PortfolioRatesUnitRole {
    CurrencyAmount = 1,
    PricePer100 = 2,
    Rate = 3,
    Years = 4,
    YearsSquared = 5,
    Dv01Per100 = 6,
    Dv01 = 7,
    Dimensionless = 8,
    ContractCount = 9,
}

impl PortfolioRatesUnitRole {
    #[must_use]
    pub const fn expected_dimension(self) -> &'static str {
        match self {
            Self::CurrencyAmount => "currency_amount",
            Self::PricePer100 => "price_per_100",
            Self::Rate => "rate",
            Self::Years => "years",
            Self::YearsSquared => "years_squared",
            Self::Dv01Per100 => "dv01_per_100",
            Self::Dv01 => "dv01",
            Self::Dimensionless => "dimensionless",
            Self::ContractCount => "contract_count",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioUnitAuthorityBinding {
    pub role: PortfolioRatesUnitRole,
    pub reference: UnitRef,
    pub content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioValuationAuthorityBinding {
    pub valuation_id: Ulid,
    pub source_revision: u64,
    pub content_hash: ContentHash,
    pub value_index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioBondRatesAuthorityCandidate {
    pub position_id: Ulid,
    pub instrument_ref: VersionRef,
    pub valuation: PortfolioValuationAuthorityBinding,
    pub remaining_years_value_index: u32,
    pub mode: AnalyticsMode,
    pub input_value: FixedDecimal,
    pub remaining_years: FixedDecimal,
    pub settlement_date: NaiveDate,
    pub calendar_requirement: CalendarRequirement,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioAnalyticsAuthorityCandidate {
    pub authority_set_id: Ulid,
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub position_snapshot: PortfolioImmutableSnapshotAuthority,
    pub curve_snapshot: PortfolioImmutableSnapshotAuthority,
    pub data_snapshot: PortfolioImmutableSnapshotAuthority,
    pub futures_data_snapshot: Option<PortfolioImmutableSnapshotAuthority>,
    pub tax_rule_pack: AnalyticsObjectRef,
    pub effective_from: MarketTime,
    pub effective_to: MarketTime,
    pub visible_at: MarketTime,
    pub units: Vec<PortfolioUnitAuthorityBinding>,
    pub bond_rates: Vec<PortfolioBondRatesAuthorityCandidate>,
    pub content_hash: ContentHash,
}

impl PortfolioAnalyticsAuthorityCandidate {
    /// Returns the aggregate content identity over the header and stable child order.
    #[must_use]
    pub fn canonical_content_hash(&self) -> ContentHash {
        let mut units = self.units.clone();
        units.sort_by_key(|binding| binding.role);
        let mut bonds = self.bond_rates.clone();
        bonds.sort_by(|left, right| {
            left.position_id.cmp(&right.position_id).then_with(|| {
                left.instrument_ref
                    .id()
                    .cmp(right.instrument_ref.id())
                    .then_with(|| {
                        left.instrument_ref
                            .version()
                            .cmp(&right.instrument_ref.version())
                    })
            })
        });
        let mut canonical = FingerprintBuilder::new("portfolio-analytics-authority/v1");
        canonical.field(2, self.authority_set_id.as_str().as_bytes());
        canonical.field(3, &owner_bytes(&self.owner));
        canonical.field(4, &version_ref_bytes(&self.subject_ref));
        canonical.field(5, &snapshot_authority_bytes(&self.position_snapshot));
        canonical.field(6, &snapshot_authority_bytes(&self.curve_snapshot));
        canonical.field(7, &snapshot_authority_bytes(&self.data_snapshot));
        canonical.field(
            8,
            &self
                .futures_data_snapshot
                .as_ref()
                .map_or_else(Vec::new, snapshot_authority_bytes),
        );
        canonical.field(9, &analytics_object_bytes(&self.tax_rule_pack));
        canonical.field(10, &market_time_bytes(&self.effective_from));
        canonical.field(11, &market_time_bytes(&self.effective_to));
        canonical.field(12, &market_time_bytes(&self.visible_at));
        for unit in &units {
            canonical.field(13, &unit_authority_bytes(unit));
        }
        for bond in &bonds {
            canonical.field(14, &bond_authority_bytes(bond));
        }
        canonical.finish().content_hash().clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioRatesUnitAuthority {
    pub role: PortfolioRatesUnitRole,
    pub reference: UnitRef,
    pub content_hash: ContentHash,
    pub dimension: String,
    pub scale: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioRiskAuthority {
    pub curve_snapshot_id: Ulid,
    pub dv01_unit: UnitRef,
    pub futures_data_snapshot_id: Option<Ulid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioBondRatesAuthority {
    pub position_id: Ulid,
    pub instrument_ref: VersionRef,
    pub bond: AnalyticsObjectRef,
    pub calendar: AnalyticsObjectRef,
    pub data_snapshot: PortfolioImmutableSnapshotAuthority,
    pub tax_rule_pack: AnalyticsObjectRef,
    pub currency_unit: UnitRef,
    pub rate_unit: UnitRef,
    pub result_units: Vec<PortfolioRatesUnitAuthority>,
    pub settlement_date: NaiveDate,
    pub calendar_requirement: CalendarRequirement,
    pub mode: AnalyticsMode,
    pub input_value: FixedDecimal,
    pub remaining_years: FixedDecimal,
    pub valuation: PortfolioValuationAuthorityBinding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortfolioBondRatesAuthorityResolution {
    Bond(Box<PortfolioBondRatesAuthority>),
    NonBond {
        position_id: Ulid,
        instrument_ref: VersionRef,
    },
    Missing {
        position_id: Ulid,
        instrument_ref: VersionRef,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PortfolioAnalyticsEvidenceKind {
    PositionSnapshot = 1,
    CurveSnapshot = 2,
    DataSnapshot = 3,
    FuturesDataSnapshot = 4,
    CurveRulePack = 5,
    TaxRulePack = 6,
    Unit = 7,
    Instrument = 8,
    Calendar = 9,
    Valuation = 10,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioAnalyticsEvidenceBinding {
    pub kind: PortfolioAnalyticsEvidenceKind,
    pub object_id: Ulid,
    pub version: Option<Version>,
    pub content_hash: ContentHash,
    pub observed_at: Option<MarketTime>,
    pub visible_at: Option<MarketTime>,
    pub effective_from: Option<MarketTime>,
    pub effective_to: Option<MarketTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPortfolioAnalyticsAuthority {
    pub authority_set_id: Ulid,
    pub risk: PortfolioRiskAuthority,
    pub units: Vec<PortfolioRatesUnitAuthority>,
    pub bond_rates: Vec<PortfolioBondRatesAuthorityResolution>,
    pub request_fingerprint: OperationFingerprint,
    pub evidence: Vec<PortfolioAnalyticsEvidenceBinding>,
}

impl ResolvedPortfolioAnalyticsAuthority {
    /// Freezes the aggregate binding hash together with every actually required-read input.
    ///
    /// # Errors
    ///
    /// Returns a lineage failure for duplicate exact evidence identities.
    pub fn new(
        authority_set_id: Ulid,
        authority_content_hash: &ContentHash,
        risk: PortfolioRiskAuthority,
        units: Vec<PortfolioRatesUnitAuthority>,
        bond_rates: Vec<PortfolioBondRatesAuthorityResolution>,
        mut evidence: Vec<PortfolioAnalyticsEvidenceBinding>,
    ) -> ApplicationResult<Self> {
        if evidence.iter().any(invalid_analytics_evidence_time_shape) {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        evidence.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.object_id.cmp(&right.object_id))
                .then_with(|| left.version.cmp(&right.version))
                .then_with(|| left.content_hash.cmp(&right.content_hash))
        });
        if evidence.windows(2).any(|pair| {
            pair[0].kind == pair[1].kind
                && pair[0].object_id == pair[1].object_id
                && pair[0].version == pair[1].version
        }) {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let mut canonical = FingerprintBuilder::new("portfolio-analytics-request/v1");
        canonical.field(2, authority_set_id.as_str().as_bytes());
        canonical.field(3, authority_content_hash.as_bytes());
        for binding in &evidence {
            canonical.field(4, &analytics_evidence_bytes(binding));
        }
        Ok(Self {
            authority_set_id,
            risk,
            units,
            bond_rates,
            request_fingerprint: canonical.finish(),
            evidence,
        })
    }
}

#[async_trait]
pub trait PortfolioAnalyticsAuthorityRepository: Send + Sync {
    async fn read_candidates(
        &self,
        scope: &AccessScope,
        query: &PortfolioAnalyticsAuthorityQuery,
    ) -> ApplicationResult<Vec<PortfolioAnalyticsAuthorityCandidate>>;

    async fn read_valuation_exact(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        binding: &PortfolioValuationAuthorityBinding,
    ) -> ApplicationResult<Option<Valuation>>;
}

#[async_trait]
pub trait PortfolioCatalogRepository: Send + Sync {
    async fn find_scope_authorities(
        &self,
        scope: &AccessScope,
        selector: &PortfolioScopeSelector,
        valuation_at: &MarketTime,
        knowledge_at: &MarketTime,
    ) -> ApplicationResult<Vec<PortfolioScopeAuthority>>;

    async fn read_catalog_snapshot(
        &self,
        scope: &AccessScope,
        temporal: &PortfolioCatalogTemporalScope,
    ) -> ApplicationResult<PortfolioCatalogSnapshot>;

    async fn read_book_exact(
        &self,
        scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<Book>>>;

    async fn read_group_exact(
        &self,
        scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<PortfolioGroup>>>;

    async fn read_portfolio_exact(
        &self,
        scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<Portfolio>>>;

    async fn read_benchmark_exact(
        &self,
        scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<Benchmark>>>;

    async fn read_metric_convention_exact(
        &self,
        scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<PortfolioMetricConvention>>>;

    async fn resolve_currency_unit(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        currency_code: &str,
    ) -> ApplicationResult<Option<UnitRef>>;
}

fn snapshot_authority_bytes(binding: &PortfolioImmutableSnapshotAuthority) -> Vec<u8> {
    let mut canonical = FingerprintBuilder::new("portfolio-snapshot-authority/v1");
    canonical.field(2, binding.id.as_str().as_bytes());
    canonical.field(3, binding.content_hash.as_bytes());
    canonical.into_bytes()
}

fn analytics_object_bytes(binding: &AnalyticsObjectRef) -> Vec<u8> {
    let mut canonical = FingerprintBuilder::new("portfolio-object-authority/v1");
    canonical.field(2, &version_ref_bytes(binding.version_ref()));
    canonical.field(3, binding.content_hash().as_bytes());
    canonical.into_bytes()
}

fn unit_authority_bytes(binding: &PortfolioUnitAuthorityBinding) -> Vec<u8> {
    let mut canonical = FingerprintBuilder::new("portfolio-unit-authority/v1");
    canonical.field(2, &[binding.role as u8]);
    canonical.field(3, binding.reference.unit_id().as_str().as_bytes());
    canonical.u64(4, binding.reference.version().get());
    canonical.field(5, binding.content_hash.as_bytes());
    canonical.into_bytes()
}

fn bond_authority_bytes(binding: &PortfolioBondRatesAuthorityCandidate) -> Vec<u8> {
    let mut canonical = FingerprintBuilder::new("portfolio-bond-rates-authority/v1");
    canonical.field(2, binding.position_id.as_str().as_bytes());
    canonical.field(3, &version_ref_bytes(&binding.instrument_ref));
    canonical.field(4, binding.valuation.valuation_id.as_str().as_bytes());
    canonical.u64(5, binding.valuation.source_revision);
    canonical.field(6, binding.valuation.content_hash.as_bytes());
    canonical.u64(7, u64::from(binding.valuation.value_index));
    canonical.u64(8, u64::from(binding.remaining_years_value_index));
    canonical.field(9, &[binding.mode as u8]);
    canonical.field(10, &binding.input_value.scaled().to_be_bytes());
    canonical.field(11, &binding.remaining_years.scaled().to_be_bytes());
    canonical.field(12, binding.settlement_date.to_string().as_bytes());
    canonical.field(13, &[binding.calendar_requirement as u8]);
    canonical.into_bytes()
}

fn analytics_evidence_bytes(binding: &PortfolioAnalyticsEvidenceBinding) -> Vec<u8> {
    let mut canonical = FingerprintBuilder::new("portfolio-analytics-evidence/v1");
    canonical.field(2, &[binding.kind as u8]);
    canonical.field(3, binding.object_id.as_str().as_bytes());
    canonical.optional_u64(4, binding.version.map(Version::get));
    canonical.field(5, binding.content_hash.as_bytes());
    canonical.field(
        6,
        &binding
            .observed_at
            .as_ref()
            .map_or_else(Vec::new, market_time_bytes),
    );
    canonical.field(
        7,
        &binding
            .visible_at
            .as_ref()
            .map_or_else(Vec::new, market_time_bytes),
    );
    canonical.field(
        8,
        &binding
            .effective_from
            .as_ref()
            .map_or_else(Vec::new, market_time_bytes),
    );
    canonical.field(
        9,
        &binding
            .effective_to
            .as_ref()
            .map_or_else(Vec::new, market_time_bytes),
    );
    canonical.into_bytes()
}

fn invalid_analytics_evidence_time_shape(binding: &PortfolioAnalyticsEvidenceBinding) -> bool {
    binding
        .observed_at
        .as_ref()
        .zip(binding.visible_at.as_ref())
        .is_some_and(|(observed, visible)| observed.instant() > visible.instant())
        || match (&binding.effective_from, &binding.effective_to) {
            (None, None) => false,
            (Some(from), Some(to)) => from.instant() >= to.instant(),
            (None, Some(_)) | (Some(_), None) => true,
        }
}

fn portfolio_status_code(status: PortfolioStatus) -> u8 {
    match status {
        PortfolioStatus::Active => 1,
        PortfolioStatus::Suspended => 2,
        PortfolioStatus::Closed => 3,
    }
}

fn normalize_search(value: &str) -> ApplicationResult<String> {
    if value.is_empty()
        || value != value.trim()
        || value.len() > 128
        || value.chars().any(char::is_control)
    {
        return Err(map_domain_error(DomainErrorCode::InvalidValue));
    }
    let normalized = value.to_lowercase();
    if normalized.is_empty() {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::ValidationFailed,
            false,
        ));
    }
    Ok(normalized)
}
