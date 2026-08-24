//! Immutable Portfolio catalog and research measurement definitions.
//!
//! These objects define scope and exact snapshot/convention bindings only. They
//! deliberately contain no positions, transactions, accounting ledger, cash, or pricing
//! algorithms. R8B research valuation snapshots and their pure return formula live in the
//! [`performance`] child module.

mod performance;

pub use performance::*;

use crate::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use crate::{ContentAddressed, DomainErrorCode, DomainResult, VersionedDefinition};

pub const PORTFOLIO_METRIC_CONVENTION_SCHEMA_V1: &str = "ficant.portfolio-metric-convention.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioStatus {
    Active,
    Suspended,
    Closed,
}

impl PortfolioStatus {
    const fn canonical_code(self) -> u8 {
        match self {
            Self::Active => 1,
            Self::Suspended => 2,
            Self::Closed => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioMetricWeighting {
    Unspecified,
    MarketValue,
    MarketValueTimesModifiedDuration,
    Notional,
}

impl PortfolioMetricWeighting {
    const fn canonical_code(self) -> u8 {
        match self {
            Self::Unspecified => 0,
            Self::MarketValue => 1,
            Self::MarketValueTimesModifiedDuration => 2,
            Self::Notional => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioDecimalRounding {
    Unspecified,
    TiesToEven,
}

impl PortfolioDecimalRounding {
    const fn canonical_code(self) -> u8 {
        match self {
            Self::Unspecified => 0,
            Self::TiesToEven => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioSnapshotBinding {
    snapshot_id: Ulid,
    content_hash: ContentHash,
    observed_at: MarketTime,
    visible_at: MarketTime,
}

impl PortfolioSnapshotBinding {
    pub fn new(
        snapshot_id: Ulid,
        content_hash: ContentHash,
        observed_at: MarketTime,
        visible_at: MarketTime,
    ) -> DomainResult<Self> {
        if observed_at.instant() > visible_at.instant() {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self {
            snapshot_id,
            content_hash,
            observed_at,
            visible_at,
        })
    }

    pub fn snapshot_id(&self) -> &Ulid {
        &self.snapshot_id
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    pub fn observed_at(&self) -> &MarketTime {
        &self.observed_at
    }

    pub fn visible_at(&self) -> &MarketTime {
        &self.visible_at
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchmarkRef {
    reference: VersionRef,
    content_hash: ContentHash,
}

impl BenchmarkRef {
    pub fn new(reference: VersionRef, content_hash: ContentHash) -> Self {
        Self {
            reference,
            content_hash,
        }
    }

    pub fn reference(&self) -> &VersionRef {
        &self.reference
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

/// Internal read-only benchmark catalog record. The public contract exposes
/// only [`BenchmarkRef`]; adapters resolve that ref to this exact record before
/// consuming its `PositionSnapshot` binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Benchmark {
    benchmark: VersionRef,
    owner: OwnerRef,
    subject_ref: VersionRef,
    code: String,
    display_name: String,
    position_snapshot: PortfolioSnapshotBinding,
    effective_from: MarketTime,
    effective_to: MarketTime,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchmarkInput {
    pub benchmark: VersionRef,
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub code: String,
    pub display_name: String,
    pub position_snapshot: PortfolioSnapshotBinding,
    pub effective_from: MarketTime,
    pub effective_to: MarketTime,
    pub content_hash: ContentHash,
}

impl Benchmark {
    pub fn new(input: BenchmarkInput) -> DomainResult<Self> {
        validate_code(&input.code)?;
        validate_display_name(&input.display_name)?;
        validate_effective_period(&input.effective_from, &input.effective_to)?;
        verify_content_hash(&input.content_hash, &canonical_benchmark(&input))?;
        Ok(Self {
            benchmark: input.benchmark,
            owner: input.owner,
            subject_ref: input.subject_ref,
            code: input.code,
            display_name: input.display_name,
            position_snapshot: input.position_snapshot,
            effective_from: input.effective_from,
            effective_to: input.effective_to,
            content_hash: input.content_hash,
        })
    }

    pub fn content_hash_for(input: &BenchmarkInput) -> ContentHash {
        ContentHash::digest(&canonical_benchmark(input))
    }

    pub fn reference(&self) -> &VersionRef {
        &self.benchmark
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn position_snapshot(&self) -> &PortfolioSnapshotBinding {
        &self.position_snapshot
    }

    pub fn effective_from(&self) -> &MarketTime {
        &self.effective_from
    }

    pub fn effective_to(&self) -> &MarketTime {
        &self.effective_to
    }
}

impl ContentAddressed for Benchmark {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl VersionedDefinition for Benchmark {
    fn identity(&self) -> &str {
        self.benchmark.id().as_str()
    }

    fn version(&self) -> u64 {
        self.benchmark.version().get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioMetricConventionRef {
    reference: VersionRef,
    content_hash: ContentHash,
}

impl PortfolioMetricConventionRef {
    pub fn new(reference: VersionRef, content_hash: ContentHash) -> Self {
        Self {
            reference,
            content_hash,
        }
    }

    pub fn reference(&self) -> &VersionRef {
        &self.reference
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Book {
    book: VersionRef,
    owner: OwnerRef,
    subject_ref: VersionRef,
    code: String,
    display_name: String,
    status: PortfolioStatus,
    effective_from: MarketTime,
    effective_to: MarketTime,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookInput {
    pub book: VersionRef,
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub code: String,
    pub display_name: String,
    pub status: PortfolioStatus,
    pub effective_from: MarketTime,
    pub effective_to: MarketTime,
    pub content_hash: ContentHash,
}

impl Book {
    pub fn new(input: BookInput) -> DomainResult<Self> {
        validate_code(&input.code)?;
        validate_display_name(&input.display_name)?;
        validate_effective_period(&input.effective_from, &input.effective_to)?;
        verify_content_hash(&input.content_hash, &canonical_book(&input))?;
        Ok(Self {
            book: input.book,
            owner: input.owner,
            subject_ref: input.subject_ref,
            code: input.code,
            display_name: input.display_name,
            status: input.status,
            effective_from: input.effective_from,
            effective_to: input.effective_to,
            content_hash: input.content_hash,
        })
    }

    pub fn content_hash_for(input: &BookInput) -> ContentHash {
        ContentHash::digest(&canonical_book(input))
    }

    pub fn reference(&self) -> &VersionRef {
        &self.book
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn status(&self) -> PortfolioStatus {
        self.status
    }

    pub fn effective_from(&self) -> &MarketTime {
        &self.effective_from
    }

    pub fn effective_to(&self) -> &MarketTime {
        &self.effective_to
    }
}

impl ContentAddressed for Book {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl VersionedDefinition for Book {
    fn identity(&self) -> &str {
        self.book.id().as_str()
    }

    fn version(&self) -> u64 {
        self.book.version().get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioGroup {
    group: VersionRef,
    owner: OwnerRef,
    subject_ref: VersionRef,
    book: LineageRef,
    parent_group: Option<LineageRef>,
    code: String,
    display_name: String,
    status: PortfolioStatus,
    effective_from: MarketTime,
    effective_to: MarketTime,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioGroupInput {
    pub group: VersionRef,
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub book: LineageRef,
    pub parent_group: Option<LineageRef>,
    pub code: String,
    pub display_name: String,
    pub status: PortfolioStatus,
    pub effective_from: MarketTime,
    pub effective_to: MarketTime,
    pub content_hash: ContentHash,
}

impl PortfolioGroup {
    pub fn new(input: PortfolioGroupInput) -> DomainResult<Self> {
        validate_code(&input.code)?;
        validate_display_name(&input.display_name)?;
        validate_effective_period(&input.effective_from, &input.effective_to)?;
        require_exact_ref(&input.book)?;
        if let Some(parent) = &input.parent_group {
            require_exact_ref(parent)?;
            if parent.object_id() == input.group.id() {
                return Err(DomainErrorCode::BrokenLineage);
            }
        }
        verify_content_hash(&input.content_hash, &canonical_group(&input))?;
        Ok(Self {
            group: input.group,
            owner: input.owner,
            subject_ref: input.subject_ref,
            book: input.book,
            parent_group: input.parent_group,
            code: input.code,
            display_name: input.display_name,
            status: input.status,
            effective_from: input.effective_from,
            effective_to: input.effective_to,
            content_hash: input.content_hash,
        })
    }

    pub fn content_hash_for(input: &PortfolioGroupInput) -> ContentHash {
        ContentHash::digest(&canonical_group(input))
    }

    pub fn reference(&self) -> &VersionRef {
        &self.group
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }

    pub fn book(&self) -> &LineageRef {
        &self.book
    }

    pub fn parent_group(&self) -> Option<&LineageRef> {
        self.parent_group.as_ref()
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn status(&self) -> PortfolioStatus {
        self.status
    }

    pub fn effective_from(&self) -> &MarketTime {
        &self.effective_from
    }

    pub fn effective_to(&self) -> &MarketTime {
        &self.effective_to
    }
}

impl ContentAddressed for PortfolioGroup {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl VersionedDefinition for PortfolioGroup {
    fn identity(&self) -> &str {
        self.group.id().as_str()
    }

    fn version(&self) -> u64 {
        self.group.version().get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Portfolio {
    portfolio: VersionRef,
    owner: OwnerRef,
    subject_ref: VersionRef,
    book: LineageRef,
    group: LineageRef,
    code: String,
    display_name: String,
    status: PortfolioStatus,
    position_snapshot: PortfolioSnapshotBinding,
    benchmark: BenchmarkRef,
    metric_convention: PortfolioMetricConventionRef,
    effective_from: MarketTime,
    effective_to: MarketTime,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioInput {
    pub portfolio: VersionRef,
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub book: LineageRef,
    pub group: LineageRef,
    pub code: String,
    pub display_name: String,
    pub status: PortfolioStatus,
    pub position_snapshot: PortfolioSnapshotBinding,
    pub benchmark: BenchmarkRef,
    pub metric_convention: PortfolioMetricConventionRef,
    pub effective_from: MarketTime,
    pub effective_to: MarketTime,
    pub content_hash: ContentHash,
}

impl Portfolio {
    pub fn new(input: PortfolioInput) -> DomainResult<Self> {
        validate_code(&input.code)?;
        validate_display_name(&input.display_name)?;
        validate_effective_period(&input.effective_from, &input.effective_to)?;
        require_exact_ref(&input.book)?;
        require_exact_ref(&input.group)?;
        verify_content_hash(&input.content_hash, &canonical_portfolio(&input))?;
        Ok(Self {
            portfolio: input.portfolio,
            owner: input.owner,
            subject_ref: input.subject_ref,
            book: input.book,
            group: input.group,
            code: input.code,
            display_name: input.display_name,
            status: input.status,
            position_snapshot: input.position_snapshot,
            benchmark: input.benchmark,
            metric_convention: input.metric_convention,
            effective_from: input.effective_from,
            effective_to: input.effective_to,
            content_hash: input.content_hash,
        })
    }

    pub fn content_hash_for(input: &PortfolioInput) -> ContentHash {
        ContentHash::digest(&canonical_portfolio(input))
    }

    pub fn reference(&self) -> &VersionRef {
        &self.portfolio
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }

    pub fn book(&self) -> &LineageRef {
        &self.book
    }

    pub fn group(&self) -> &LineageRef {
        &self.group
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn status(&self) -> PortfolioStatus {
        self.status
    }

    pub fn position_snapshot(&self) -> &PortfolioSnapshotBinding {
        &self.position_snapshot
    }

    pub fn benchmark(&self) -> &BenchmarkRef {
        &self.benchmark
    }

    pub fn metric_convention(&self) -> &PortfolioMetricConventionRef {
        &self.metric_convention
    }

    pub fn effective_from(&self) -> &MarketTime {
        &self.effective_from
    }

    pub fn effective_to(&self) -> &MarketTime {
        &self.effective_to
    }
}

impl ContentAddressed for Portfolio {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl VersionedDefinition for Portfolio {
    fn identity(&self) -> &str {
        self.portfolio.id().as_str()
    }

    fn version(&self) -> u64 {
        self.portfolio.version().get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioMetricConvention {
    convention: VersionRef,
    owner: OwnerRef,
    schema_id: String,
    ytm_weighting: PortfolioMetricWeighting,
    duration_weighting: PortfolioMetricWeighting,
    convexity_weighting: PortfolioMetricWeighting,
    coupon_weighting: PortfolioMetricWeighting,
    remaining_life_weighting: PortfolioMetricWeighting,
    rounding: PortfolioDecimalRounding,
    freshness_limit_seconds: u64,
    effective_from: MarketTime,
    effective_to: MarketTime,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioMetricConventionInput {
    pub convention: VersionRef,
    pub owner: OwnerRef,
    pub schema_id: String,
    pub ytm_weighting: PortfolioMetricWeighting,
    pub duration_weighting: PortfolioMetricWeighting,
    pub convexity_weighting: PortfolioMetricWeighting,
    pub coupon_weighting: PortfolioMetricWeighting,
    pub remaining_life_weighting: PortfolioMetricWeighting,
    pub rounding: PortfolioDecimalRounding,
    pub freshness_limit_seconds: u64,
    pub effective_from: MarketTime,
    pub effective_to: MarketTime,
    pub content_hash: ContentHash,
}

impl PortfolioMetricConvention {
    pub fn new(input: PortfolioMetricConventionInput) -> DomainResult<Self> {
        validate_effective_period(&input.effective_from, &input.effective_to)?;
        if input.schema_id != PORTFOLIO_METRIC_CONVENTION_SCHEMA_V1
            || input.ytm_weighting != PortfolioMetricWeighting::MarketValueTimesModifiedDuration
            || input.duration_weighting != PortfolioMetricWeighting::MarketValue
            || input.convexity_weighting != PortfolioMetricWeighting::MarketValue
            || input.coupon_weighting != PortfolioMetricWeighting::Notional
            || input.remaining_life_weighting != PortfolioMetricWeighting::Notional
            || input.rounding != PortfolioDecimalRounding::TiesToEven
            || input.freshness_limit_seconds == 0
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        verify_content_hash(&input.content_hash, &canonical_convention(&input))?;
        Ok(Self {
            convention: input.convention,
            owner: input.owner,
            schema_id: input.schema_id,
            ytm_weighting: input.ytm_weighting,
            duration_weighting: input.duration_weighting,
            convexity_weighting: input.convexity_weighting,
            coupon_weighting: input.coupon_weighting,
            remaining_life_weighting: input.remaining_life_weighting,
            rounding: input.rounding,
            freshness_limit_seconds: input.freshness_limit_seconds,
            effective_from: input.effective_from,
            effective_to: input.effective_to,
            content_hash: input.content_hash,
        })
    }

    pub fn content_hash_for(input: &PortfolioMetricConventionInput) -> ContentHash {
        ContentHash::digest(&canonical_convention(input))
    }

    pub fn reference(&self) -> &VersionRef {
        &self.convention
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn schema_id(&self) -> &str {
        &self.schema_id
    }

    pub fn ytm_weighting(&self) -> PortfolioMetricWeighting {
        self.ytm_weighting
    }

    pub fn duration_weighting(&self) -> PortfolioMetricWeighting {
        self.duration_weighting
    }

    pub fn convexity_weighting(&self) -> PortfolioMetricWeighting {
        self.convexity_weighting
    }

    pub fn coupon_weighting(&self) -> PortfolioMetricWeighting {
        self.coupon_weighting
    }

    pub fn remaining_life_weighting(&self) -> PortfolioMetricWeighting {
        self.remaining_life_weighting
    }

    pub fn rounding(&self) -> PortfolioDecimalRounding {
        self.rounding
    }

    pub fn freshness_limit_seconds(&self) -> u64 {
        self.freshness_limit_seconds
    }

    pub fn effective_from(&self) -> &MarketTime {
        &self.effective_from
    }

    pub fn effective_to(&self) -> &MarketTime {
        &self.effective_to
    }
}

impl ContentAddressed for PortfolioMetricConvention {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl VersionedDefinition for PortfolioMetricConvention {
    fn identity(&self) -> &str {
        self.convention.id().as_str()
    }

    fn version(&self) -> u64 {
        self.convention.version().get()
    }
}

fn validate_code(value: &str) -> DomainResult<()> {
    if value.is_empty()
        || value != value.trim()
        || value.len() > 64
        || !value.is_ascii()
        || !value.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}

fn validate_display_name(value: &str) -> DomainResult<()> {
    if value.is_empty() || value != value.trim() || value.len() > 256 {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}

fn validate_effective_period(from: &MarketTime, to: &MarketTime) -> DomainResult<()> {
    if from.instant() >= to.instant() {
        return Err(DomainErrorCode::InvalidEffectiveTime);
    }
    Ok(())
}

fn require_exact_ref(reference: &LineageRef) -> DomainResult<()> {
    if reference.version().is_none() || reference.content_hash().is_none() {
        return Err(DomainErrorCode::BrokenLineage);
    }
    Ok(())
}

fn verify_content_hash(expected: &ContentHash, canonical: &[u8]) -> DomainResult<()> {
    if expected != &ContentHash::digest(canonical) {
        return Err(DomainErrorCode::ContentHashMismatch);
    }
    Ok(())
}

fn canonical_book(input: &BookInput) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_version_ref(&mut bytes, &input.book);
    append_owner(&mut bytes, &input.owner);
    append_version_ref(&mut bytes, &input.subject_ref);
    append(&mut bytes, input.code.as_bytes());
    append(&mut bytes, input.display_name.as_bytes());
    append(&mut bytes, &[input.status.canonical_code()]);
    append_market_time(&mut bytes, &input.effective_from);
    append_market_time(&mut bytes, &input.effective_to);
    bytes
}

fn canonical_benchmark(input: &BenchmarkInput) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_version_ref(&mut bytes, &input.benchmark);
    append_owner(&mut bytes, &input.owner);
    append_version_ref(&mut bytes, &input.subject_ref);
    append(&mut bytes, input.code.as_bytes());
    append(&mut bytes, input.display_name.as_bytes());
    append(
        &mut bytes,
        input.position_snapshot.snapshot_id.as_str().as_bytes(),
    );
    append(&mut bytes, input.position_snapshot.content_hash.as_bytes());
    append_market_time(&mut bytes, &input.position_snapshot.observed_at);
    append_market_time(&mut bytes, &input.position_snapshot.visible_at);
    append_market_time(&mut bytes, &input.effective_from);
    append_market_time(&mut bytes, &input.effective_to);
    bytes
}

fn canonical_group(input: &PortfolioGroupInput) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_version_ref(&mut bytes, &input.group);
    append_owner(&mut bytes, &input.owner);
    append_version_ref(&mut bytes, &input.subject_ref);
    append_exact_ref(&mut bytes, &input.book);
    match &input.parent_group {
        Some(parent) => {
            append(&mut bytes, &[1]);
            append_exact_ref(&mut bytes, parent);
        }
        None => append(&mut bytes, &[0]),
    }
    append(&mut bytes, input.code.as_bytes());
    append(&mut bytes, input.display_name.as_bytes());
    append(&mut bytes, &[input.status.canonical_code()]);
    append_market_time(&mut bytes, &input.effective_from);
    append_market_time(&mut bytes, &input.effective_to);
    bytes
}

fn canonical_portfolio(input: &PortfolioInput) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_version_ref(&mut bytes, &input.portfolio);
    append_owner(&mut bytes, &input.owner);
    append_version_ref(&mut bytes, &input.subject_ref);
    append_exact_ref(&mut bytes, &input.book);
    append_exact_ref(&mut bytes, &input.group);
    append(&mut bytes, input.code.as_bytes());
    append(&mut bytes, input.display_name.as_bytes());
    append(&mut bytes, &[input.status.canonical_code()]);
    append(
        &mut bytes,
        input.position_snapshot.snapshot_id.as_str().as_bytes(),
    );
    append(&mut bytes, input.position_snapshot.content_hash.as_bytes());
    append_market_time(&mut bytes, &input.position_snapshot.observed_at);
    append_market_time(&mut bytes, &input.position_snapshot.visible_at);
    append_version_ref(&mut bytes, &input.benchmark.reference);
    append(&mut bytes, input.benchmark.content_hash.as_bytes());
    append_version_ref(&mut bytes, &input.metric_convention.reference);
    append(&mut bytes, input.metric_convention.content_hash.as_bytes());
    append_market_time(&mut bytes, &input.effective_from);
    append_market_time(&mut bytes, &input.effective_to);
    bytes
}

fn canonical_convention(input: &PortfolioMetricConventionInput) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_version_ref(&mut bytes, &input.convention);
    append_owner(&mut bytes, &input.owner);
    append(&mut bytes, input.schema_id.as_bytes());
    for weighting in [
        input.ytm_weighting,
        input.duration_weighting,
        input.convexity_weighting,
        input.coupon_weighting,
        input.remaining_life_weighting,
    ] {
        append(&mut bytes, &[weighting.canonical_code()]);
    }
    append(&mut bytes, &[input.rounding.canonical_code()]);
    append(&mut bytes, &input.freshness_limit_seconds.to_be_bytes());
    append_market_time(&mut bytes, &input.effective_from);
    append_market_time(&mut bytes, &input.effective_to);
    bytes
}

fn append_owner(bytes: &mut Vec<u8>, owner: &OwnerRef) {
    append(bytes, owner.tenant_id().as_str().as_bytes());
    append(bytes, owner.owner_id().as_str().as_bytes());
}

fn append_version_ref(bytes: &mut Vec<u8>, reference: &VersionRef) {
    append(bytes, reference.id().as_str().as_bytes());
    append(bytes, &reference.version().get().to_be_bytes());
}

fn append_exact_ref(bytes: &mut Vec<u8>, reference: &LineageRef) {
    append(bytes, reference.object_id().as_str().as_bytes());
    append(
        bytes,
        &reference.version().map_or(0, Version::get).to_be_bytes(),
    );
    let content_hash: &[u8] = reference
        .content_hash()
        .map_or(&[][..], |hash| hash.as_bytes().as_slice());
    append(bytes, content_hash);
}

fn append_market_time(bytes: &mut Vec<u8>, value: &MarketTime) {
    let instant = value.instant();
    append(bytes, &instant.timestamp().to_be_bytes());
    append(bytes, &instant.timestamp_subsec_nanos().to_be_bytes());
    append(bytes, value.market_timezone().as_bytes());
    append(bytes, value.local_trading_date().to_string().as_bytes());
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}
