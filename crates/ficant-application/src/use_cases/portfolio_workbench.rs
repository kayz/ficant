use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use ficant_domain::ContentAddressed;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, VersionRef};
use ficant_domain::research::{CoverageDeclaration, PortfolioKeyRateExposure};
use ficant_runtime::{
    FormalInputBinding, FormalInputBindingInput, FormalInputKind, FormalInputReference,
    FormalOutputEvidence,
};

use crate::ports::{
    AeadCursorCodec, ApplicationResult, AuthorizedPrincipal, Clock, DefinitionRepository,
    DefinitionValue, ExactPortfolioScopeKind, IdGenerator, MarketFact, MarketFactRepository,
    MarketFactWindow, NormalizedPortfolioContext, NormalizedPortfolioContextResolution,
    PageRequest, PortfolioCatalogEvidenceBinding, PortfolioCatalogEvidenceRole,
    PortfolioCatalogFilter, PortfolioCatalogPage, PortfolioCatalogRepository,
    PortfolioCatalogTemporalScope, PortfolioContextInput, PortfolioScopeAuthority,
    PortfolioScopeSelector, SubjectRepository, market_fact_content_hash,
    stored_definition_content_hash, subject_record_content_hash,
};
use crate::use_cases::portfolio_aggregation::{
    OwnedPortfolioAggregationBackend, PortfolioAggregationUseCase, PortfolioBondAnalysisResult,
    PortfolioCoverage, PortfolioCoverageReason, PortfolioOverview,
};
use crate::use_cases::portfolio_catalog::{ListPortfolioCatalog, ListPortfolioCatalogCommand};
use crate::use_cases::position_views::PositionViews;
use crate::{ApplicationError, ApplicationErrorCategory};

pub const PORTFOLIO_WORKBENCH_SCHEMA_VERSION: &str = "portfolio-workbench.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioWorkbenchPageId {
    D01,
    P01,
    P02,
    P03,
    P04,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioPageDataMode {
    Real,
    Partial,
    Stale,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioPageState {
    Ready,
    Empty,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioWorkbenchErrorCode {
    Unauthenticated,
    Forbidden,
    NotFound,
    Conflict,
    Stale,
    Integrity,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortfolioWorkbenchSourceError {
    Application(ApplicationError),
    Stale { retryable: bool },
}

impl From<ApplicationError> for PortfolioWorkbenchSourceError {
    fn from(value: ApplicationError) -> Self {
        Self::Application(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioWorkbenchTypedError {
    code: PortfolioWorkbenchErrorCode,
    safe_message: &'static str,
    trace_id: String,
    retryable: bool,
}

impl PortfolioWorkbenchTypedError {
    #[must_use]
    pub const fn code(&self) -> PortfolioWorkbenchErrorCode {
        self.code
    }

    #[must_use]
    pub const fn safe_message(&self) -> &'static str {
        self.safe_message
    }

    #[must_use]
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.retryable
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NonFormalReadEvidence {
    schema_id: String,
    consumed_inputs: Vec<FormalInputBinding>,
    request_fingerprint: ContentHash,
}

impl NonFormalReadEvidence {
    /// Creates explicit non-formal read evidence; it never creates an output identity.
    ///
    /// # Errors
    ///
    /// Returns validation failure for an unstable schema or no consumed authority, and lineage
    /// failure for duplicate roles.
    pub fn new(
        schema_id: String,
        mut consumed_inputs: Vec<FormalInputBinding>,
        request_fingerprint: ContentHash,
    ) -> ApplicationResult<Self> {
        if schema_id.is_empty()
            || schema_id != schema_id.trim()
            || !schema_id.is_ascii()
            || consumed_inputs.is_empty()
        {
            return Err(validation());
        }
        consumed_inputs.sort_by(|left, right| left.role().cmp(right.role()));
        if consumed_inputs
            .windows(2)
            .any(|pair| pair[0].role() == pair[1].role())
        {
            return Err(integrity());
        }
        Ok(Self {
            schema_id,
            consumed_inputs,
            request_fingerprint,
        })
    }

    #[must_use]
    pub fn schema_id(&self) -> &str {
        &self.schema_id
    }

    #[must_use]
    pub fn consumed_inputs(&self) -> &[FormalInputBinding] {
        &self.consumed_inputs
    }

    #[must_use]
    pub fn request_fingerprint(&self) -> &ContentHash {
        &self.request_fingerprint
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortfolioPageCoverageReason {
    ShortPositionExcludedFromWeightedAverages,
    NonBondExcludedFromWeightedAverages,
    MissingBondMetricExcludedFromWeightedAverages,
    PositionExcludedFromPortfolioRisk,
    BenchmarkPositionExcludedFromPortfolioRisk,
    MissingCriticalField,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPageCoverage {
    participation: CoverageDeclaration,
    missing_reasons: Vec<PortfolioPageCoverageReason>,
}

impl PortfolioPageCoverage {
    #[must_use]
    pub fn from_portfolio(value: &PortfolioCoverage) -> Self {
        Self {
            participation: value.participation().clone(),
            missing_reasons: value
                .missing_reasons()
                .iter()
                .copied()
                .map(PortfolioPageCoverageReason::from)
                .collect(),
        }
    }

    #[must_use]
    pub fn from_domain(value: &CoverageDeclaration) -> Self {
        let missing_reasons = if value.missing_critical_field_record_count() == 0 {
            Vec::new()
        } else {
            vec![PortfolioPageCoverageReason::MissingCriticalField]
        };
        Self {
            participation: value.clone(),
            missing_reasons,
        }
    }

    #[must_use]
    pub const fn participation(&self) -> &CoverageDeclaration {
        &self.participation
    }

    #[must_use]
    pub const fn participating_position_count(&self) -> u64 {
        self.participation.participating_position_count()
    }

    #[must_use]
    pub const fn imported_position_count(&self) -> u64 {
        self.participation.imported_position_count()
    }

    #[must_use]
    pub fn missing_reasons(&self) -> &[PortfolioPageCoverageReason] {
        &self.missing_reasons
    }
}

impl From<PortfolioCoverageReason> for PortfolioPageCoverageReason {
    fn from(value: PortfolioCoverageReason) -> Self {
        match value {
            PortfolioCoverageReason::ShortPositionExcludedFromWeightedAverages => {
                Self::ShortPositionExcludedFromWeightedAverages
            }
            PortfolioCoverageReason::NonBondExcludedFromWeightedAverages => {
                Self::NonBondExcludedFromWeightedAverages
            }
            PortfolioCoverageReason::MissingBondMetricExcludedFromWeightedAverages => {
                Self::MissingBondMetricExcludedFromWeightedAverages
            }
            PortfolioCoverageReason::PositionExcludedFromPortfolioRisk => {
                Self::PositionExcludedFromPortfolioRisk
            }
            PortfolioCoverageReason::BenchmarkPositionExcludedFromPortfolioRisk => {
                Self::BenchmarkPositionExcludedFromPortfolioRisk
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPageProvenance {
    owner: OwnerRef,
    subject_ref: VersionRef,
    request_fingerprint: ContentHash,
    formal_evidence: Vec<FormalOutputEvidence>,
    non_formal_reads: Vec<NonFormalReadEvidence>,
}

impl PortfolioPageProvenance {
    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }

    #[must_use]
    pub fn request_fingerprint(&self) -> &ContentHash {
        &self.request_fingerprint
    }

    #[must_use]
    pub fn formal_evidence(&self) -> &[FormalOutputEvidence] {
        &self.formal_evidence
    }

    #[must_use]
    pub fn non_formal_reads(&self) -> &[NonFormalReadEvidence] {
        &self.non_formal_reads
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructureMetrics {
    pub book_count: u64,
    pub group_count: u64,
    pub portfolio_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct P01Projection {
    pub catalog: PortfolioCatalogPage,
    pub structure: StructureMetrics,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct P03Projection {
    pub position_views: Vec<PositionViews>,
    pub key_rate_exposures: Vec<PortfolioKeyRateExposure>,
    pub coverage: PortfolioPageCoverage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct P04Projection {
    pub definition: crate::ports::DefinitionValue,
    pub facts: Vec<MarketFact>,
    pub analysis: PortfolioBondAnalysisResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortfolioPageProjection {
    D01(PortfolioOverview),
    P01(P01Projection),
    P02(PortfolioOverview),
    P03(P03Projection),
    P04(P04Projection),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioCatalogRead {
    catalog: PortfolioCatalogPage,
    data_mode: PortfolioPageDataMode,
    page_state: PortfolioPageState,
    non_formal_reads: Vec<NonFormalReadEvidence>,
    source_fingerprint: ContentHash,
}

impl PortfolioCatalogRead {
    /// Creates the explicitly non-formal P01 backend result.
    ///
    /// # Errors
    ///
    /// Rejects ERROR, invalid coverage-mode combinations, or missing read evidence.
    pub fn new(
        catalog: PortfolioCatalogPage,
        data_mode: PortfolioPageDataMode,
        page_state: PortfolioPageState,
        non_formal_reads: Vec<NonFormalReadEvidence>,
        source_fingerprint: ContentHash,
    ) -> ApplicationResult<Self> {
        if matches!(
            data_mode,
            PortfolioPageDataMode::Error | PortfolioPageDataMode::Partial
        ) || non_formal_reads.is_empty()
        {
            return Err(integrity());
        }
        Ok(Self {
            catalog,
            data_mode,
            page_state,
            non_formal_reads,
            source_fingerprint,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioInstrumentRead {
    projection: P04Projection,
    data_mode: PortfolioPageDataMode,
    page_state: PortfolioPageState,
    coverage: PortfolioPageCoverage,
    formal_evidence: Vec<FormalOutputEvidence>,
    non_formal_reads: Vec<NonFormalReadEvidence>,
    source_fingerprint: ContentHash,
}

impl PortfolioInstrumentRead {
    /// Creates an exact P04 instrument backend result.
    ///
    /// # Errors
    ///
    /// Rejects ERROR, invalid coverage-mode combinations, or a result without `AnalyzeBond` formal
    /// evidence.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        projection: P04Projection,
        data_mode: PortfolioPageDataMode,
        page_state: PortfolioPageState,
        coverage: PortfolioPageCoverage,
        formal_evidence: Vec<FormalOutputEvidence>,
        non_formal_reads: Vec<NonFormalReadEvidence>,
        source_fingerprint: ContentHash,
    ) -> ApplicationResult<Self> {
        validate_success_mode(data_mode, &coverage)?;
        if formal_evidence.is_empty() {
            return Err(integrity());
        }
        Ok(Self {
            projection,
            data_mode,
            page_state,
            coverage,
            formal_evidence,
            non_formal_reads,
            source_fingerprint,
        })
    }
}

impl PortfolioPageProjection {
    const fn page_id(&self) -> PortfolioWorkbenchPageId {
        match self {
            Self::D01(_) => PortfolioWorkbenchPageId::D01,
            Self::P01(_) => PortfolioWorkbenchPageId::P01,
            Self::P02(_) => PortfolioWorkbenchPageId::P02,
            Self::P03(_) => PortfolioWorkbenchPageId::P03,
            Self::P04(_) => PortfolioWorkbenchPageId::P04,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPageMaterialization {
    projection: PortfolioPageProjection,
    data_mode: PortfolioPageDataMode,
    page_state: PortfolioPageState,
    coverage: Option<PortfolioPageCoverage>,
    formal_evidence: Vec<FormalOutputEvidence>,
    non_formal_reads: Vec<NonFormalReadEvidence>,
    source_fingerprint: ContentHash,
}

impl PortfolioPageMaterialization {
    /// Binds a real backend projection to its two explicitly separate evidence classes.
    ///
    /// # Errors
    ///
    /// Rejects ERROR as a success mode, PARTIAL without a reason, REAL with omissions, or
    /// evidence owner/Subject drift.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        projection: PortfolioPageProjection,
        data_mode: PortfolioPageDataMode,
        page_state: PortfolioPageState,
        coverage: Option<PortfolioPageCoverage>,
        mut formal_evidence: Vec<FormalOutputEvidence>,
        mut non_formal_reads: Vec<NonFormalReadEvidence>,
        source_fingerprint: ContentHash,
        owner: &OwnerRef,
        subject_ref: &VersionRef,
    ) -> ApplicationResult<Self> {
        let coverage_shape_invalid = match (&projection, &coverage) {
            (PortfolioPageProjection::P01(_), None) => false,
            (PortfolioPageProjection::P01(_), Some(_)) | (_, None) => true,
            (_, Some(coverage)) => validate_success_mode(data_mode, coverage).is_err(),
        };
        if data_mode == PortfolioPageDataMode::Error
            || coverage_shape_invalid
            || evidence_shape_is_invalid(
                &projection,
                formal_evidence.as_slice(),
                non_formal_reads.as_slice(),
            )
            || formal_evidence.iter().any(|evidence| {
                evidence.subject().owner() != owner
                    || !formal_subject_matches(evidence, subject_ref)
            })
            || non_formal_reads.iter().any(|evidence| {
                evidence
                    .consumed_inputs()
                    .iter()
                    .any(|binding| binding.owner() != owner)
                    || evidence
                        .consumed_inputs()
                        .iter()
                        .find(|binding| binding.role() == "subject")
                        .is_some_and(|binding| !input_subject_matches(binding, subject_ref))
            })
        {
            return Err(integrity());
        }
        formal_evidence.sort_by(|left, right| left.output_identity().cmp(right.output_identity()));
        non_formal_reads.sort_by(|left, right| {
            left.request_fingerprint
                .cmp(&right.request_fingerprint)
                .then_with(|| left.schema_id.cmp(&right.schema_id))
        });
        Ok(Self {
            projection,
            data_mode,
            page_state,
            coverage,
            formal_evidence,
            non_formal_reads,
            source_fingerprint,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPageSelection {
    instrument: VersionRef,
}

impl PortfolioPageSelection {
    #[must_use]
    pub const fn new(instrument: VersionRef) -> Self {
        Self { instrument }
    }

    #[must_use]
    pub const fn instrument(&self) -> &VersionRef {
        &self.instrument
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioPageEnvelope {
    schema_version: &'static str,
    page_id: PortfolioWorkbenchPageId,
    request_id: String,
    generated_at: MarketTime,
    data_mode: PortfolioPageDataMode,
    normalized_context: Option<NormalizedPortfolioContext>,
    page_state: PortfolioPageState,
    permissions: Vec<String>,
    provenance: Option<PortfolioPageProvenance>,
    coverage: Option<PortfolioPageCoverage>,
    projection: Option<PortfolioPageProjection>,
    typed_error: Option<PortfolioWorkbenchTypedError>,
}

impl PortfolioPageEnvelope {
    #[must_use]
    pub const fn schema_version(&self) -> &'static str {
        self.schema_version
    }

    #[must_use]
    pub const fn page_id(&self) -> PortfolioWorkbenchPageId {
        self.page_id
    }

    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    #[must_use]
    pub const fn generated_at(&self) -> &MarketTime {
        &self.generated_at
    }

    #[must_use]
    pub const fn data_mode(&self) -> PortfolioPageDataMode {
        self.data_mode
    }

    #[must_use]
    pub fn normalized_context(&self) -> Option<&NormalizedPortfolioContext> {
        self.normalized_context.as_ref()
    }

    #[must_use]
    pub const fn page_state(&self) -> PortfolioPageState {
        self.page_state
    }

    #[must_use]
    pub fn permissions(&self) -> &[String] {
        &self.permissions
    }

    #[must_use]
    pub fn provenance(&self) -> Option<&PortfolioPageProvenance> {
        self.provenance.as_ref()
    }

    #[must_use]
    pub const fn coverage(&self) -> Option<&PortfolioPageCoverage> {
        self.coverage.as_ref()
    }

    #[must_use]
    pub fn projection(&self) -> Option<&PortfolioPageProjection> {
        self.projection.as_ref()
    }

    #[must_use]
    pub fn typed_error(&self) -> Option<&PortfolioWorkbenchTypedError> {
        self.typed_error.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortfolioDefaultContextResult {
    Context(Box<NormalizedPortfolioContext>),
    Error(PortfolioWorkbenchTypedError),
}

#[async_trait]
pub trait PortfolioWorkbenchContextResolver: Send + Sync {
    async fn resolve_scope_authority(
        &self,
        principal: &AuthorizedPrincipal,
        selector: &PortfolioScopeSelector,
        valuation_at: &MarketTime,
        knowledge_at: &MarketTime,
    ) -> ApplicationResult<PortfolioScopeAuthority>;

    async fn normalize_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        input: PortfolioContextInput,
    ) -> ApplicationResult<NormalizedPortfolioContextResolution>;

    async fn get_default_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<NormalizedPortfolioContext>;
}

#[async_trait]
impl PortfolioWorkbenchContextResolver for ListPortfolioCatalog<'_> {
    async fn resolve_scope_authority(
        &self,
        principal: &AuthorizedPrincipal,
        selector: &PortfolioScopeSelector,
        valuation_at: &MarketTime,
        knowledge_at: &MarketTime,
    ) -> ApplicationResult<PortfolioScopeAuthority> {
        ListPortfolioCatalog::resolve_scope_authority(
            self,
            principal,
            selector,
            valuation_at,
            knowledge_at,
        )
        .await
    }

    async fn normalize_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        input: PortfolioContextInput,
    ) -> ApplicationResult<NormalizedPortfolioContextResolution> {
        ListPortfolioCatalog::normalize_context_with_evidence(
            self,
            principal,
            owner,
            subject_ref,
            input,
        )
        .await
    }

    async fn get_default_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<NormalizedPortfolioContext> {
        ListPortfolioCatalog::get_default_context(self, principal, owner, subject_ref, knowledge_at)
            .await
    }
}

#[derive(Clone)]
pub struct OwnedPortfolioWorkbenchContextResolver {
    repository: Arc<dyn PortfolioCatalogRepository>,
    cursor_codec: Arc<AeadCursorCodec>,
}

impl OwnedPortfolioWorkbenchContextResolver {
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
impl PortfolioWorkbenchContextResolver for OwnedPortfolioWorkbenchContextResolver {
    async fn resolve_scope_authority(
        &self,
        principal: &AuthorizedPrincipal,
        selector: &PortfolioScopeSelector,
        valuation_at: &MarketTime,
        knowledge_at: &MarketTime,
    ) -> ApplicationResult<PortfolioScopeAuthority> {
        ListPortfolioCatalog::new(self.repository.as_ref(), self.cursor_codec.as_ref())
            .resolve_scope_authority(principal, selector, valuation_at, knowledge_at)
            .await
    }

    async fn normalize_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        input: PortfolioContextInput,
    ) -> ApplicationResult<NormalizedPortfolioContextResolution> {
        ListPortfolioCatalog::new(self.repository.as_ref(), self.cursor_codec.as_ref())
            .normalize_context_with_evidence(principal, owner, subject_ref, input)
            .await
    }

    async fn get_default_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<NormalizedPortfolioContext> {
        ListPortfolioCatalog::new(self.repository.as_ref(), self.cursor_codec.as_ref())
            .get_default_context(principal, owner, subject_ref, knowledge_at)
            .await
    }
}

#[async_trait]
pub trait PortfolioWorkbenchPageSource: Send + Sync {
    /// Reads P01 through the exact Catalog use case and returns only non-formal read evidence.
    async fn read_catalog(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
    ) -> Result<PortfolioCatalogRead, PortfolioWorkbenchSourceError>;

    /// Runs the formal `PortfolioOverview` aggregation used by D01/P02 and the P03 projection.
    async fn get_overview(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
    ) -> Result<PortfolioOverview, PortfolioWorkbenchSourceError>;

    /// Required-reads the selected snapshot member and composes Definition/Fact/AnalyzeBond P04.
    async fn read_instrument(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
        selection: &PortfolioPageSelection,
    ) -> Result<PortfolioInstrumentRead, PortfolioWorkbenchSourceError>;
}

#[async_trait]
pub trait PortfolioWorkbenchCatalogEvidenceFactory: Send + Sync {
    /// Binds the exact Catalog records as non-formal inputs without creating an output identity.
    ///
    /// # Errors
    ///
    /// Returns an integrity failure when the read authorities cannot be bound exactly.
    async fn evidence(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
        catalog: &PortfolioCatalogPage,
    ) -> ApplicationResult<Vec<NonFormalReadEvidence>>;
}

#[derive(Clone)]
pub struct OwnedPortfolioWorkbenchCatalogEvidenceFactory {
    subjects: Arc<dyn SubjectRepository>,
}

impl OwnedPortfolioWorkbenchCatalogEvidenceFactory {
    #[must_use]
    pub fn new(subjects: Arc<dyn SubjectRepository>) -> Self {
        Self { subjects }
    }
}

#[async_trait]
impl PortfolioWorkbenchCatalogEvidenceFactory for OwnedPortfolioWorkbenchCatalogEvidenceFactory {
    async fn evidence(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
        catalog: &PortfolioCatalogPage,
    ) -> ApplicationResult<Vec<NonFormalReadEvidence>> {
        let context = resolution.context();
        let subject = self
            .subjects
            .get_subject_scoped(principal.access_scope(), context.subject_ref.clone())
            .await?
            .ok_or_else(not_found)?;
        if subject.version().reference() != &context.subject_ref
            || subject.subject().owner() != Some(&context.owner)
        {
            return Err(integrity());
        }
        let mut inputs = vec![object_input(
            "subject".to_owned(),
            FormalInputKind::Subject,
            &context.owner,
            &context.subject_ref,
            subject_record_content_hash(&subject)?,
            None,
        )?];
        inputs.extend(normalized_catalog_inputs(resolution)?);
        let temporal = PortfolioCatalogTemporalScope::new(
            context.owner.clone(),
            context.subject_ref.clone(),
            context.valuation_at.clone(),
            context.knowledge_at.clone(),
        )?;
        for (index, record) in catalog.books().iter().enumerate() {
            let value = record.value();
            validate_catalog_times(
                value.owner(),
                value.subject_ref(),
                value.effective_from(),
                value.effective_to(),
                record.visible_at(),
                &temporal,
            )?;
            inputs.push(catalog_record_input(
                format!("catalog_book_{index:04}"),
                FormalInputKind::Book,
                &context.owner,
                value.reference(),
                value.content_hash().clone(),
                record.visible_at(),
                value.effective_from(),
                value.effective_to(),
            )?);
        }
        for (index, record) in catalog.groups().iter().enumerate() {
            let value = record.value();
            validate_catalog_times(
                value.owner(),
                value.subject_ref(),
                value.effective_from(),
                value.effective_to(),
                record.visible_at(),
                &temporal,
            )?;
            inputs.push(catalog_record_input(
                format!("catalog_group_{index:04}"),
                FormalInputKind::PortfolioGroup,
                &context.owner,
                value.reference(),
                value.content_hash().clone(),
                record.visible_at(),
                value.effective_from(),
                value.effective_to(),
            )?);
        }
        for (index, entry) in catalog.portfolios().iter().enumerate() {
            let record = entry.record();
            let value = record.value();
            validate_catalog_times(
                value.owner(),
                value.subject_ref(),
                value.effective_from(),
                value.effective_to(),
                record.visible_at(),
                &temporal,
            )?;
            inputs.push(catalog_record_input(
                format!("catalog_portfolio_{index:04}"),
                FormalInputKind::Portfolio,
                &context.owner,
                value.reference(),
                value.content_hash().clone(),
                record.visible_at(),
                value.effective_from(),
                value.effective_to(),
            )?);
        }
        let fingerprint = catalog_evidence_fingerprint(catalog, &inputs)?;
        Ok(vec![NonFormalReadEvidence::new(
            "ficant.portfolio.v1.ListPortfolioCatalog".to_owned(),
            inputs,
            fingerprint,
        )?])
    }
}

#[async_trait]
pub trait PortfolioCatalogReadEvidenceFactory: Send + Sync {
    /// Binds one exact Catalog command and its returned page as a non-formal read.
    async fn evidence(
        &self,
        principal: &AuthorizedPrincipal,
        command: &ListPortfolioCatalogCommand,
        catalog: &PortfolioCatalogPage,
    ) -> ApplicationResult<NonFormalReadEvidence>;
}

#[derive(Clone)]
pub struct OwnedPortfolioCatalogReadEvidenceFactory {
    subjects: Arc<dyn SubjectRepository>,
}

impl OwnedPortfolioCatalogReadEvidenceFactory {
    #[must_use]
    pub fn new(subjects: Arc<dyn SubjectRepository>) -> Self {
        Self { subjects }
    }
}

#[async_trait]
impl PortfolioCatalogReadEvidenceFactory for OwnedPortfolioCatalogReadEvidenceFactory {
    async fn evidence(
        &self,
        principal: &AuthorizedPrincipal,
        command: &ListPortfolioCatalogCommand,
        catalog: &PortfolioCatalogPage,
    ) -> ApplicationResult<NonFormalReadEvidence> {
        let temporal = command.filter().temporal();
        principal.access_scope().authorize(temporal.owner())?;
        if catalog.request_fingerprint() != command.filter().fingerprint() {
            return Err(integrity());
        }
        let subject = self
            .subjects
            .get_subject_scoped(principal.access_scope(), temporal.subject_ref().clone())
            .await?
            .ok_or_else(not_found)?;
        if subject.subject().owner() != Some(temporal.owner())
            || subject.version().reference() != temporal.subject_ref()
        {
            return Err(integrity());
        }
        let mut inputs = vec![object_input(
            "subject".to_owned(),
            FormalInputKind::Subject,
            temporal.owner(),
            temporal.subject_ref(),
            subject_record_content_hash(&subject)?,
            None,
        )?];
        for (index, record) in catalog.books().iter().enumerate() {
            let value = record.value();
            validate_catalog_times(
                value.owner(),
                value.subject_ref(),
                value.effective_from(),
                value.effective_to(),
                record.visible_at(),
                temporal,
            )?;
            inputs.push(catalog_record_input(
                format!("catalog_book_{index:04}"),
                FormalInputKind::Book,
                temporal.owner(),
                value.reference(),
                value.content_hash().clone(),
                record.visible_at(),
                value.effective_from(),
                value.effective_to(),
            )?);
        }
        for (index, record) in catalog.groups().iter().enumerate() {
            let value = record.value();
            validate_catalog_times(
                value.owner(),
                value.subject_ref(),
                value.effective_from(),
                value.effective_to(),
                record.visible_at(),
                temporal,
            )?;
            inputs.push(catalog_record_input(
                format!("catalog_group_{index:04}"),
                FormalInputKind::PortfolioGroup,
                temporal.owner(),
                value.reference(),
                value.content_hash().clone(),
                record.visible_at(),
                value.effective_from(),
                value.effective_to(),
            )?);
        }
        for (index, entry) in catalog.portfolios().iter().enumerate() {
            let record = entry.record();
            let value = record.value();
            validate_catalog_times(
                value.owner(),
                value.subject_ref(),
                value.effective_from(),
                value.effective_to(),
                record.visible_at(),
                temporal,
            )?;
            inputs.push(catalog_record_input(
                format!("catalog_portfolio_{index:04}"),
                FormalInputKind::Portfolio,
                temporal.owner(),
                value.reference(),
                value.content_hash().clone(),
                record.visible_at(),
                value.effective_from(),
                value.effective_to(),
            )?);
        }
        let request_fingerprint = catalog_command_evidence_fingerprint(command, catalog, &inputs)?;
        NonFormalReadEvidence::new(
            "ficant.portfolio.v1.ListPortfolioCatalog".to_owned(),
            inputs,
            request_fingerprint,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioCatalogApplicationRead {
    catalog: PortfolioCatalogPage,
    evidence: NonFormalReadEvidence,
}

impl PortfolioCatalogApplicationRead {
    #[must_use]
    pub const fn catalog(&self) -> &PortfolioCatalogPage {
        &self.catalog
    }

    #[must_use]
    pub const fn evidence(&self) -> &NonFormalReadEvidence {
        &self.evidence
    }

    #[must_use]
    pub fn into_parts(self) -> (PortfolioCatalogPage, NonFormalReadEvidence) {
        (self.catalog, self.evidence)
    }
}

/// Arc-owned Catalog read boundary; transports receive both the page and its exact evidence.
#[derive(Clone)]
pub struct OwnedPortfolioCatalogBackend {
    repository: Arc<dyn PortfolioCatalogRepository>,
    cursor_codec: Arc<AeadCursorCodec>,
    evidence: Arc<dyn PortfolioCatalogReadEvidenceFactory>,
}

impl OwnedPortfolioCatalogBackend {
    #[must_use]
    pub fn new(
        repository: Arc<dyn PortfolioCatalogRepository>,
        cursor_codec: Arc<AeadCursorCodec>,
        evidence: Arc<dyn PortfolioCatalogReadEvidenceFactory>,
    ) -> Self {
        Self {
            repository,
            cursor_codec,
            evidence,
        }
    }

    /// Executes the Catalog use case and binds its exact non-formal evidence in Application.
    ///
    /// # Errors
    ///
    /// Fails closed on authorization, catalog drift, or evidence identity mismatch.
    pub async fn list(
        &self,
        principal: &AuthorizedPrincipal,
        command: ListPortfolioCatalogCommand,
    ) -> ApplicationResult<PortfolioCatalogApplicationRead> {
        let catalog =
            ListPortfolioCatalog::new(self.repository.as_ref(), self.cursor_codec.as_ref())
                .execute(principal, command.clone())
                .await?;
        let evidence = self
            .evidence
            .evidence(principal, &command, &catalog)
            .await?;
        Ok(PortfolioCatalogApplicationRead { catalog, evidence })
    }
}

#[async_trait]
pub trait PortfolioWorkbenchInstrumentHandoff: Send + Sync {
    /// Required-reads a selected member instrument and calls the existing Definition, Fact and
    /// `AnalyzeBond` paths. Implementations must reject instruments absent from the resolved scope.
    async fn read(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
        selection: &PortfolioPageSelection,
    ) -> Result<PortfolioInstrumentRead, PortfolioWorkbenchSourceError>;
}

/// Arc-owned P04 composition. It reuses the fresh formal Overview and only performs exact
/// Definition/Fact display reads after the selected instrument is proven present in that result.
#[derive(Clone)]
pub struct OwnedPortfolioWorkbenchInstrumentHandoff {
    aggregation: Arc<OwnedPortfolioAggregationBackend>,
    definitions: Arc<dyn DefinitionRepository>,
    facts: Arc<dyn MarketFactRepository>,
}

impl OwnedPortfolioWorkbenchInstrumentHandoff {
    #[must_use]
    pub fn new(
        aggregation: Arc<OwnedPortfolioAggregationBackend>,
        definitions: Arc<dyn DefinitionRepository>,
        facts: Arc<dyn MarketFactRepository>,
    ) -> Self {
        Self {
            aggregation,
            definitions,
            facts,
        }
    }

    async fn query_facts(
        &self,
        principal: &AuthorizedPrincipal,
        context: &NormalizedPortfolioContext,
        instrument: &VersionRef,
    ) -> ApplicationResult<Vec<MarketFact>> {
        let mut cursor = None;
        let mut seen_cursors = BTreeSet::new();
        let mut result = Vec::new();
        loop {
            let page = PageRequest::new(
                principal.access_scope().clone(),
                cursor,
                PageRequest::MAX_LIMIT,
            )?;
            let query = MarketFactWindow::new(
                instrument.clone(),
                context.period_from.clone(),
                context.period_to.clone(),
                context.knowledge_at.clone(),
                page,
            )?;
            let fact_page = self
                .facts
                .query_instrument_window(principal.access_scope(), query)
                .await?;
            let (items, next_cursor) = fact_page.into_parts();
            for fact in items {
                validate_p04_fact(context, instrument, &fact)?;
                result.push(fact);
            }
            cursor = match next_cursor {
                Some(value) => {
                    if !seen_cursors.insert(value.as_str().to_owned()) {
                        return Err(integrity());
                    }
                    Some(value)
                }
                None => break,
            };
        }
        result.sort_by(|left, right| {
            left.id()
                .cmp(right.id())
                .then_with(|| left.source_revision().cmp(&right.source_revision()))
                .then_with(|| market_fact_content_hash(left).cmp(&market_fact_content_hash(right)))
        });
        if result.windows(2).any(|pair| {
            pair[0].id() == pair[1].id() && pair[0].source_revision() == pair[1].source_revision()
        }) {
            return Err(integrity());
        }
        Ok(result)
    }
}

#[async_trait]
impl PortfolioWorkbenchInstrumentHandoff for OwnedPortfolioWorkbenchInstrumentHandoff {
    #[allow(clippy::too_many_lines)]
    async fn read(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
        selection: &PortfolioPageSelection,
    ) -> Result<PortfolioInstrumentRead, PortfolioWorkbenchSourceError> {
        let context = resolution.context();
        let overview = self
            .aggregation
            .execute_resolution(principal, resolution)
            .await?;
        let selected = selected_overview_analysis(&overview, selection.instrument())?;
        let definition = self
            .definitions
            .get_version(
                principal.access_scope(),
                selection.instrument().id().clone(),
                selection.instrument().version(),
            )
            .await?
            .ok_or_else(not_found)?;
        validate_selected_definition(context, selection.instrument(), &definition, &selected)?;
        let definition_hash = stored_definition_content_hash(&definition);
        let facts = self
            .query_facts(principal, context, selection.instrument())
            .await?;
        validate_selected_valuation(&selected, &facts)?;

        let definition_input = object_input(
            "definition".to_owned(),
            FormalInputKind::Definition,
            &context.owner,
            selection.instrument(),
            definition_hash.clone(),
            None,
        )?;
        let definition_read = NonFormalReadEvidence::new(
            "ficant.definition.v1.GetDefinition".to_owned(),
            vec![definition_input],
            p04_definition_fingerprint(context, selection.instrument(), &definition_hash),
        )?;

        let fact_kind = portfolio_fact_input_kind();
        let mut fact_inputs = Vec::with_capacity(facts.len());
        for (index, fact) in facts.iter().enumerate() {
            fact_inputs.push(p04_fact_input(
                format!("fact_{index:04}"),
                fact_kind,
                &context.owner,
                fact,
            )?);
        }
        let fact_read = NonFormalReadEvidence::new(
            "ficant.fact.v1.QueryInstrumentFacts".to_owned(),
            fact_inputs,
            p04_fact_fingerprint(context, selection.instrument(), &facts),
        )?;

        let data_mode = overview_page_data_mode(&overview);
        let coverage = PortfolioPageCoverage::from_portfolio(overview.draft().coverage());
        let formal_evidence = vec![overview.formal_evidence().clone()];
        let source_fingerprint = p04_source_fingerprint(
            overview.draft().request_fingerprint(),
            &definition_hash,
            &facts,
        );
        PortfolioInstrumentRead::new(
            P04Projection {
                definition,
                facts,
                analysis: selected.analysis,
            },
            data_mode,
            PortfolioPageState::Ready,
            coverage,
            formal_evidence,
            vec![definition_read, fact_read],
            source_fingerprint,
        )
        .map_err(PortfolioWorkbenchSourceError::from)
    }
}

#[derive(Clone)]
struct SelectedOverviewAnalysis {
    analysis: PortfolioBondAnalysisResult,
    valuation: crate::ports::PortfolioValuationAuthorityBinding,
}

fn selected_overview_analysis(
    overview: &PortfolioOverview,
    instrument: &VersionRef,
) -> ApplicationResult<SelectedOverviewAnalysis> {
    let mut selected = None::<SelectedOverviewAnalysis>;
    for member in overview.draft().members() {
        for candidate in member
            .bond_analyses()
            .iter()
            .filter(|candidate| candidate.instrument_ref() == instrument)
        {
            let value = SelectedOverviewAnalysis {
                analysis: candidate.analysis().clone(),
                valuation: candidate.valuation().clone(),
            };
            if selected.as_ref().is_some_and(|existing| {
                existing.analysis != value.analysis || existing.valuation != value.valuation
            }) {
                return Err(integrity());
            }
            selected = Some(value);
        }
    }
    selected.ok_or_else(not_found)
}

fn validate_selected_definition(
    context: &NormalizedPortfolioContext,
    instrument: &VersionRef,
    definition: &DefinitionValue,
    selected: &SelectedOverviewAnalysis,
) -> ApplicationResult<()> {
    let DefinitionValue::Instrument(definition) = definition else {
        return Err(integrity());
    };
    let definition_hash =
        stored_definition_content_hash(&DefinitionValue::Instrument(definition.clone()));
    let input = selected.analysis.analytics().input();
    if definition.owner() != &context.owner
        || definition.identity() != instrument.id().as_str()
        || definition.version() != instrument.version().get()
        || input.owner() != &context.owner
        || input.valuation_at() != &context.valuation_at
        || input.bond().version_ref() != instrument
        || input.bond().content_hash() != &definition_hash
        || selected.analysis.metadata().subject_ref() != &context.subject_ref
        || selected.analysis.metadata().formal_evidence().is_some()
    {
        return Err(integrity());
    }
    let mut bond_inputs = selected
        .analysis
        .metadata()
        .request_evidence()
        .consumed_inputs()
        .iter()
        .filter(|evidence| {
            evidence.role() == crate::use_cases::rates_materialization::RatesInputRole::Bond
        });
    let Some(bond_input) = bond_inputs.next() else {
        return Err(integrity());
    };
    if bond_inputs.next().is_some()
        || bond_input.owner() != &context.owner
        || !matches!(
            bond_input.binding(),
            crate::use_cases::rates_materialization::RatesEvidenceBinding::Object(reference)
                if reference.version_ref() == instrument
                    && reference.content_hash() == &definition_hash
        )
    {
        return Err(integrity());
    }
    Ok(())
}

fn validate_selected_valuation(
    selected: &SelectedOverviewAnalysis,
    facts: &[MarketFact],
) -> ApplicationResult<()> {
    validate_valuation_binding(&selected.valuation, facts)
}

fn validate_valuation_binding(
    binding: &crate::ports::PortfolioValuationAuthorityBinding,
    facts: &[MarketFact],
) -> ApplicationResult<()> {
    let matching = facts
        .iter()
        .filter(|fact| {
            fact.id() == &binding.valuation_id
                && fact.source_revision() == binding.source_revision
                && market_fact_content_hash(fact) == binding.content_hash
        })
        .collect::<Vec<_>>();
    let [MarketFact::Valuation(valuation)] = matching.as_slice() else {
        return Err(integrity());
    };
    let index = usize::try_from(binding.value_index).map_err(|_| validation())?;
    if valuation.values().get(index).is_none() {
        return Err(integrity());
    }
    Ok(())
}

fn validate_p04_fact(
    context: &NormalizedPortfolioContext,
    instrument: &VersionRef,
    fact: &MarketFact,
) -> ApplicationResult<()> {
    let (fact_instrument, event_time) = match fact {
        MarketFact::Cashflow(value) => (value.bond(), value.payment_time()),
        MarketFact::Quote(value) => (value.instrument(), value.observed_at()),
        MarketFact::Trade(value) => (value.instrument(), value.executed_at()),
        MarketFact::Valuation(value) => (value.instrument(), value.valuation_at()),
    };
    if fact.owner() != &context.owner
        || fact_instrument != instrument
        || event_time.instant() < context.period_from.instant()
        || event_time.instant() > context.period_to.instant()
        || event_time.instant() > context.knowledge_at.instant()
    {
        return Err(integrity());
    }
    Ok(())
}

const fn portfolio_fact_input_kind() -> FormalInputKind {
    FormalInputKind::Fact
}

fn p04_fact_input(
    role: String,
    kind: FormalInputKind,
    owner: &OwnerRef,
    fact: &MarketFact,
) -> ApplicationResult<FormalInputBinding> {
    let version = ficant_domain::primitives::Version::new(fact.source_revision())
        .map_err(crate::map_domain_error)?;
    let lineage = LineageRef::new(
        fact.id().clone(),
        Some(version),
        Some(market_fact_content_hash(fact)),
    )
    .map_err(crate::map_domain_error)?;
    let observed_at = match fact {
        MarketFact::Cashflow(value) => value.payment_time(),
        MarketFact::Quote(value) => value.observed_at(),
        MarketFact::Trade(value) => value.executed_at(),
        MarketFact::Valuation(value) => value.valuation_at(),
    };
    FormalInputBinding::new(FormalInputBindingInput {
        role,
        kind,
        owner: owner.clone(),
        reference: FormalInputReference::Object(lineage),
        observed_at: Some(observed_at.clone()),
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .map_err(crate::map_domain_error)
}

fn overview_page_data_mode(overview: &PortfolioOverview) -> PortfolioPageDataMode {
    match overview.draft().data_mode() {
        crate::use_cases::portfolio_aggregation::PortfolioMetricDataMode::Real => {
            PortfolioPageDataMode::Real
        }
        crate::use_cases::portfolio_aggregation::PortfolioMetricDataMode::Partial => {
            PortfolioPageDataMode::Partial
        }
        crate::use_cases::portfolio_aggregation::PortfolioMetricDataMode::Stale => {
            PortfolioPageDataMode::Stale
        }
    }
}

fn p04_definition_fingerprint(
    context: &NormalizedPortfolioContext,
    instrument: &VersionRef,
    content_hash: &ContentHash,
) -> ContentHash {
    let mut bytes = b"ficant.portfolio.p04-definition.v1".to_vec();
    append(&mut bytes, context.owner.owner_id().as_str().as_bytes());
    append(&mut bytes, instrument.id().as_str().as_bytes());
    append(&mut bytes, &instrument.version().get().to_be_bytes());
    append(&mut bytes, content_hash.as_bytes());
    append_market_time(&mut bytes, &context.knowledge_at);
    ContentHash::digest(&bytes)
}

fn p04_fact_fingerprint(
    context: &NormalizedPortfolioContext,
    instrument: &VersionRef,
    facts: &[MarketFact],
) -> ContentHash {
    let mut bytes = b"ficant.portfolio.p04-facts.v1".to_vec();
    append(&mut bytes, instrument.id().as_str().as_bytes());
    append(&mut bytes, &instrument.version().get().to_be_bytes());
    append_market_time(&mut bytes, &context.period_from);
    append_market_time(&mut bytes, &context.period_to);
    append_market_time(&mut bytes, &context.knowledge_at);
    for fact in facts {
        append(&mut bytes, fact.id().as_str().as_bytes());
        append(&mut bytes, &fact.source_revision().to_be_bytes());
        append(&mut bytes, market_fact_content_hash(fact).as_bytes());
    }
    ContentHash::digest(&bytes)
}

fn p04_source_fingerprint(
    overview_fingerprint: &ContentHash,
    definition_hash: &ContentHash,
    facts: &[MarketFact],
) -> ContentHash {
    let mut bytes = b"ficant.portfolio.p04-source.v1".to_vec();
    append(&mut bytes, overview_fingerprint.as_bytes());
    append(&mut bytes, definition_hash.as_bytes());
    for fact in facts {
        append(&mut bytes, market_fact_content_hash(fact).as_bytes());
    }
    ContentHash::digest(&bytes)
}

/// Real application composition for the five Workbench pages. API/server layers inject this
/// adapter and never build page projections themselves.
pub struct ExistingPortfolioWorkbenchPageSource<'a> {
    catalog: &'a ListPortfolioCatalog<'a>,
    aggregation: &'a PortfolioAggregationUseCase<'a>,
    catalog_evidence: &'a dyn PortfolioWorkbenchCatalogEvidenceFactory,
    instrument: &'a dyn PortfolioWorkbenchInstrumentHandoff,
}

impl<'a> ExistingPortfolioWorkbenchPageSource<'a> {
    #[must_use]
    pub const fn new(
        catalog: &'a ListPortfolioCatalog<'a>,
        aggregation: &'a PortfolioAggregationUseCase<'a>,
        catalog_evidence: &'a dyn PortfolioWorkbenchCatalogEvidenceFactory,
        instrument: &'a dyn PortfolioWorkbenchInstrumentHandoff,
    ) -> Self {
        Self {
            catalog,
            aggregation,
            catalog_evidence,
            instrument,
        }
    }
}

#[async_trait]
impl PortfolioWorkbenchPageSource for ExistingPortfolioWorkbenchPageSource<'_> {
    async fn read_catalog(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
    ) -> Result<PortfolioCatalogRead, PortfolioWorkbenchSourceError> {
        let context = resolution.context();
        let temporal = PortfolioCatalogTemporalScope::new(
            context.owner.clone(),
            context.subject_ref.clone(),
            context.valuation_at.clone(),
            context.knowledge_at.clone(),
        )?;
        let filter = PortfolioCatalogFilter::new(temporal, Vec::new(), None)?;
        let command = ListPortfolioCatalogCommand::new(
            filter,
            None,
            crate::ports::PORTFOLIO_CATALOG_MAX_PAGE_SIZE,
        )?;
        let catalog = self.catalog.execute(principal, command).await?;
        let non_formal_reads = self
            .catalog_evidence
            .evidence(principal, resolution, &catalog)
            .await?;
        let count = u64::try_from(catalog.portfolios().len()).map_err(|_| validation())?;
        let page_state = if count == 0 {
            PortfolioPageState::Empty
        } else {
            PortfolioPageState::Ready
        };
        let source_fingerprint = catalog.request_fingerprint().content_hash().clone();
        Ok(PortfolioCatalogRead::new(
            catalog,
            PortfolioPageDataMode::Real,
            page_state,
            non_formal_reads,
            source_fingerprint,
        )?)
    }

    async fn get_overview(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
    ) -> Result<PortfolioOverview, PortfolioWorkbenchSourceError> {
        Ok(self
            .aggregation
            .execute_resolution(principal, resolution)
            .await?)
    }

    async fn read_instrument(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
        selection: &PortfolioPageSelection,
    ) -> Result<PortfolioInstrumentRead, PortfolioWorkbenchSourceError> {
        self.instrument.read(principal, resolution, selection).await
    }
}

/// Arc-owned production page source; transports never assemble catalog or analytics factories.
#[derive(Clone)]
pub struct OwnedPortfolioWorkbenchPageSource {
    catalog_repository: Arc<dyn PortfolioCatalogRepository>,
    cursor_codec: Arc<AeadCursorCodec>,
    aggregation: Arc<OwnedPortfolioAggregationBackend>,
    catalog_evidence: Arc<dyn PortfolioWorkbenchCatalogEvidenceFactory>,
    instrument: Arc<dyn PortfolioWorkbenchInstrumentHandoff>,
}

impl OwnedPortfolioWorkbenchPageSource {
    #[must_use]
    pub fn new(
        catalog_repository: Arc<dyn PortfolioCatalogRepository>,
        cursor_codec: Arc<AeadCursorCodec>,
        aggregation: Arc<OwnedPortfolioAggregationBackend>,
        catalog_evidence: Arc<dyn PortfolioWorkbenchCatalogEvidenceFactory>,
        instrument: Arc<dyn PortfolioWorkbenchInstrumentHandoff>,
    ) -> Self {
        Self {
            catalog_repository,
            cursor_codec,
            aggregation,
            catalog_evidence,
            instrument,
        }
    }
}

#[async_trait]
impl PortfolioWorkbenchPageSource for OwnedPortfolioWorkbenchPageSource {
    async fn read_catalog(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
    ) -> Result<PortfolioCatalogRead, PortfolioWorkbenchSourceError> {
        let context = resolution.context();
        let temporal = PortfolioCatalogTemporalScope::new(
            context.owner.clone(),
            context.subject_ref.clone(),
            context.valuation_at.clone(),
            context.knowledge_at.clone(),
        )?;
        let filter = PortfolioCatalogFilter::new(temporal, Vec::new(), None)?;
        let command = ListPortfolioCatalogCommand::new(
            filter,
            None,
            crate::ports::PORTFOLIO_CATALOG_MAX_PAGE_SIZE,
        )?;
        let catalog =
            ListPortfolioCatalog::new(self.catalog_repository.as_ref(), self.cursor_codec.as_ref())
                .execute(principal, command)
                .await?;
        let non_formal_reads = self
            .catalog_evidence
            .evidence(principal, resolution, &catalog)
            .await?;
        let page_state = if catalog.portfolios().is_empty() {
            PortfolioPageState::Empty
        } else {
            PortfolioPageState::Ready
        };
        let source_fingerprint = catalog.request_fingerprint().content_hash().clone();
        Ok(PortfolioCatalogRead::new(
            catalog,
            PortfolioPageDataMode::Real,
            page_state,
            non_formal_reads,
            source_fingerprint,
        )?)
    }

    async fn get_overview(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
    ) -> Result<PortfolioOverview, PortfolioWorkbenchSourceError> {
        Ok(self
            .aggregation
            .execute_resolution(principal, resolution)
            .await?)
    }

    async fn read_instrument(
        &self,
        principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
        selection: &PortfolioPageSelection,
    ) -> Result<PortfolioInstrumentRead, PortfolioWorkbenchSourceError> {
        self.instrument.read(principal, resolution, selection).await
    }
}

pub struct PortfolioWorkbenchUseCase<'a> {
    contexts: &'a dyn PortfolioWorkbenchContextResolver,
    pages: &'a dyn PortfolioWorkbenchPageSource,
    clock: &'a dyn Clock,
    ids: &'a dyn IdGenerator,
}

impl<'a> PortfolioWorkbenchUseCase<'a> {
    #[must_use]
    pub const fn new(
        contexts: &'a dyn PortfolioWorkbenchContextResolver,
        pages: &'a dyn PortfolioWorkbenchPageSource,
        clock: &'a dyn Clock,
        ids: &'a dyn IdGenerator,
    ) -> Self {
        Self {
            contexts,
            pages,
            clock,
            ids,
        }
    }

    /// Resolves the stable first active Portfolio without server-clock or UI defaults.
    ///
    /// # Errors
    ///
    /// Only request-metadata generation failures escape; closed business errors become one of the
    /// seven typed Workbench errors.
    pub async fn get_default_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<PortfolioDefaultContextResult> {
        let (_, _, trace_id) = self.request_metadata()?;
        match authorize_page(principal, PortfolioWorkbenchPageId::P01, &owner) {
            Ok(()) => {}
            Err(error) => {
                return Ok(PortfolioDefaultContextResult::Error(typed_error(
                    &error, trace_id,
                )));
            }
        }
        Ok(
            match self
                .contexts
                .get_default_context(principal, owner, subject_ref, knowledge_at)
                .await
            {
                Ok(context) => PortfolioDefaultContextResult::Context(Box::new(context)),
                Err(error) => PortfolioDefaultContextResult::Error(typed_error(&error, trace_id)),
            },
        )
    }

    /// Resolves all six context dimensions and returns a domain-only `PageEnvelope`.
    ///
    /// # Errors
    ///
    /// Malformed page/selection shape and request-metadata failures remain transport errors.
    /// Closed business failures become ERROR envelopes with no success projection.
    pub async fn get_page_for_selector(
        &self,
        principal: &AuthorizedPrincipal,
        page_id: PortfolioWorkbenchPageId,
        input: PortfolioContextInput,
        selection: Option<PortfolioPageSelection>,
    ) -> ApplicationResult<PortfolioPageEnvelope> {
        let authority = self
            .contexts
            .resolve_scope_authority(
                principal,
                &input.scope,
                &input.valuation_at,
                &input.knowledge_at,
            )
            .await?;
        self.get_page(
            principal,
            authority.owner().clone(),
            authority.subject_ref().clone(),
            page_id,
            input,
            selection,
        )
        .await
    }

    /// Materializes a page after a trusted application resolver has frozen owner and Subject.
    ///
    /// # Errors
    ///
    /// Malformed page/selection shape and request-metadata failures remain transport errors.
    /// Closed business failures become ERROR envelopes with no success projection.
    #[allow(clippy::too_many_arguments)]
    pub async fn get_page(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        page_id: PortfolioWorkbenchPageId,
        input: PortfolioContextInput,
        selection: Option<PortfolioPageSelection>,
    ) -> ApplicationResult<PortfolioPageEnvelope> {
        validate_selection(page_id, selection.as_ref())?;
        let (request_id, generated_at, trace_id) = self.request_metadata()?;
        let permissions = permissions_for(page_id);
        if let Err(error) = authorize_page(principal, page_id, &owner) {
            return Ok(error_envelope(
                page_id,
                request_id,
                generated_at,
                permissions,
                None,
                typed_error(&error, trace_id),
            ));
        }
        let resolution = match self
            .contexts
            .normalize_context(principal, owner, subject_ref, input)
            .await
        {
            Ok(resolution) => resolution,
            Err(error) => {
                return Ok(error_envelope(
                    page_id,
                    request_id,
                    generated_at,
                    permissions,
                    None,
                    typed_error(&error, trace_id),
                ));
            }
        };
        let context = resolution.context();
        let materialized = match self
            .materialize_page(principal, page_id, &resolution, selection.as_ref())
            .await
        {
            Ok(materialized) => materialized,
            Err(error) => {
                return Ok(error_envelope(
                    page_id,
                    request_id,
                    generated_at,
                    permissions,
                    Some(context.clone()),
                    typed_source_error(&error, trace_id),
                ));
            }
        };
        if materialized.projection.page_id() != page_id {
            return Ok(error_envelope(
                page_id,
                request_id,
                generated_at,
                permissions,
                Some(context.clone()),
                typed_error(&integrity(), trace_id),
            ));
        }
        let request_fingerprint =
            page_request_fingerprint(page_id, context, &materialized.source_fingerprint);
        let provenance = PortfolioPageProvenance {
            owner: context.owner.clone(),
            subject_ref: context.subject_ref.clone(),
            request_fingerprint,
            formal_evidence: materialized.formal_evidence,
            non_formal_reads: materialized.non_formal_reads,
        };
        Ok(PortfolioPageEnvelope {
            schema_version: PORTFOLIO_WORKBENCH_SCHEMA_VERSION,
            page_id,
            request_id,
            generated_at,
            data_mode: materialized.data_mode,
            normalized_context: Some(context.clone()),
            page_state: materialized.page_state,
            permissions,
            provenance: Some(provenance),
            coverage: materialized.coverage,
            projection: Some(materialized.projection),
            typed_error: None,
        })
    }

    fn request_metadata(&self) -> ApplicationResult<(String, MarketTime, String)> {
        let request_id = self.ids.next_id()?.as_str().to_owned();
        let generated_at = self.clock.now()?;
        let digest = ContentHash::digest(request_id.as_bytes());
        let trace_id =
            digest.as_bytes()[..16]
                .iter()
                .fold(String::with_capacity(32), |mut encoded, byte| {
                    use std::fmt::Write as _;
                    write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
                    encoded
                });
        Ok((request_id, generated_at, trace_id))
    }

    async fn materialize_page(
        &self,
        principal: &AuthorizedPrincipal,
        page_id: PortfolioWorkbenchPageId,
        resolution: &NormalizedPortfolioContextResolution,
        selection: Option<&PortfolioPageSelection>,
    ) -> Result<PortfolioPageMaterialization, PortfolioWorkbenchSourceError> {
        let context = resolution.context();
        match page_id {
            PortfolioWorkbenchPageId::P01 => {
                let read = self.pages.read_catalog(principal, resolution).await?;
                PortfolioPageMaterialization::new(
                    PortfolioPageProjection::P01(P01Projection {
                        structure: StructureMetrics {
                            book_count: u64::try_from(read.catalog.books().len())
                                .map_err(|_| validation())?,
                            group_count: u64::try_from(read.catalog.groups().len())
                                .map_err(|_| validation())?,
                            portfolio_count: u64::try_from(read.catalog.portfolios().len())
                                .map_err(|_| validation())?,
                        },
                        catalog: read.catalog,
                    }),
                    read.data_mode,
                    read.page_state,
                    None,
                    Vec::new(),
                    read.non_formal_reads,
                    read.source_fingerprint,
                    &context.owner,
                    &context.subject_ref,
                )
                .map_err(PortfolioWorkbenchSourceError::from)
            }
            PortfolioWorkbenchPageId::D01
            | PortfolioWorkbenchPageId::P02
            | PortfolioWorkbenchPageId::P03 => {
                let overview = self.pages.get_overview(principal, resolution).await?;
                overview_materialization(page_id, overview, context)
                    .map_err(PortfolioWorkbenchSourceError::from)
            }
            PortfolioWorkbenchPageId::P04 => {
                let selection =
                    selection.ok_or_else(|| PortfolioWorkbenchSourceError::from(validation()))?;
                let read = self
                    .pages
                    .read_instrument(principal, resolution, selection)
                    .await?;
                PortfolioPageMaterialization::new(
                    PortfolioPageProjection::P04(read.projection),
                    read.data_mode,
                    read.page_state,
                    Some(read.coverage),
                    read.formal_evidence,
                    read.non_formal_reads,
                    read.source_fingerprint,
                    &context.owner,
                    &context.subject_ref,
                )
                .map_err(PortfolioWorkbenchSourceError::from)
            }
        }
    }
}

/// Arc-owned, `'static` Workbench boundary held directly by a tonic service.
#[derive(Clone)]
pub struct OwnedPortfolioWorkbenchBackend {
    contexts: Arc<dyn PortfolioWorkbenchContextResolver>,
    pages: Arc<dyn PortfolioWorkbenchPageSource>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl OwnedPortfolioWorkbenchBackend {
    #[must_use]
    pub fn new(
        contexts: Arc<dyn PortfolioWorkbenchContextResolver>,
        pages: Arc<dyn PortfolioWorkbenchPageSource>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            contexts,
            pages,
            clock,
            ids,
        }
    }

    /// Returns the exact default context or one typed closed error.
    ///
    /// # Errors
    ///
    /// Propagates request-metadata generation failures.
    pub async fn get_default_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<PortfolioDefaultContextResult> {
        PortfolioWorkbenchUseCase::new(
            self.contexts.as_ref(),
            self.pages.as_ref(),
            self.clock.as_ref(),
            self.ids.as_ref(),
        )
        .get_default_context(principal, owner, subject_ref, knowledge_at)
        .await
    }

    /// Returns one fully materialized Workbench page without transport-side composition.
    ///
    /// # Errors
    ///
    /// Propagates malformed page/selection and request-metadata failures.
    pub async fn get_page_for_selector(
        &self,
        principal: &AuthorizedPrincipal,
        page_id: PortfolioWorkbenchPageId,
        input: PortfolioContextInput,
        selection: Option<PortfolioPageSelection>,
    ) -> ApplicationResult<PortfolioPageEnvelope> {
        PortfolioWorkbenchUseCase::new(
            self.contexts.as_ref(),
            self.pages.as_ref(),
            self.clock.as_ref(),
            self.ids.as_ref(),
        )
        .get_page_for_selector(principal, page_id, input, selection)
        .await
    }

    /// Returns a page after an application-owned selector resolver froze owner and Subject.
    ///
    /// # Errors
    ///
    /// Propagates malformed page/selection and request-metadata failures.
    #[allow(clippy::too_many_arguments)]
    pub async fn get_page(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        page_id: PortfolioWorkbenchPageId,
        input: PortfolioContextInput,
        selection: Option<PortfolioPageSelection>,
    ) -> ApplicationResult<PortfolioPageEnvelope> {
        PortfolioWorkbenchUseCase::new(
            self.contexts.as_ref(),
            self.pages.as_ref(),
            self.clock.as_ref(),
            self.ids.as_ref(),
        )
        .get_page(principal, owner, subject_ref, page_id, input, selection)
        .await
    }
}

fn overview_materialization(
    page_id: PortfolioWorkbenchPageId,
    overview: PortfolioOverview,
    context: &NormalizedPortfolioContext,
) -> ApplicationResult<PortfolioPageMaterialization> {
    let data_mode = match overview.draft().data_mode() {
        crate::use_cases::portfolio_aggregation::PortfolioMetricDataMode::Real => {
            PortfolioPageDataMode::Real
        }
        crate::use_cases::portfolio_aggregation::PortfolioMetricDataMode::Partial => {
            PortfolioPageDataMode::Partial
        }
        crate::use_cases::portfolio_aggregation::PortfolioMetricDataMode::Stale => {
            PortfolioPageDataMode::Stale
        }
    };
    let coverage = PortfolioPageCoverage::from_portfolio(overview.draft().coverage());
    validate_success_mode(data_mode, &coverage)?;
    let page_state = if coverage.imported_position_count() == 0 {
        PortfolioPageState::Empty
    } else {
        PortfolioPageState::Ready
    };
    let formal_evidence = vec![overview.formal_evidence().clone()];
    let source_fingerprint = overview.draft().request_fingerprint().clone();
    let projection = match page_id {
        PortfolioWorkbenchPageId::D01 => PortfolioPageProjection::D01(overview),
        PortfolioWorkbenchPageId::P02 => PortfolioPageProjection::P02(overview),
        PortfolioWorkbenchPageId::P03 => {
            let members = overview.draft().members();
            PortfolioPageProjection::P03(P03Projection {
                position_views: members
                    .iter()
                    .map(|member| member.position_views().clone())
                    .collect(),
                key_rate_exposures: members
                    .iter()
                    .map(|member| member.key_rate_exposure().clone())
                    .collect(),
                coverage: coverage.clone(),
            })
        }
        PortfolioWorkbenchPageId::P01 | PortfolioWorkbenchPageId::P04 => {
            return Err(validation());
        }
    };
    PortfolioPageMaterialization::new(
        projection,
        data_mode,
        page_state,
        Some(coverage),
        formal_evidence,
        Vec::new(),
        source_fingerprint,
        &context.owner,
        &context.subject_ref,
    )
}

fn validate_success_mode(
    data_mode: PortfolioPageDataMode,
    coverage: &PortfolioPageCoverage,
) -> ApplicationResult<()> {
    if data_mode == PortfolioPageDataMode::Error
        || (data_mode == PortfolioPageDataMode::Partial && coverage.missing_reasons().is_empty())
        || (data_mode == PortfolioPageDataMode::Real && !coverage.missing_reasons().is_empty())
    {
        return Err(integrity());
    }
    Ok(())
}

fn object_input(
    role: String,
    kind: FormalInputKind,
    owner: &OwnerRef,
    reference: &VersionRef,
    content_hash: ContentHash,
    visible_at: Option<MarketTime>,
) -> ApplicationResult<FormalInputBinding> {
    let lineage = LineageRef::new(
        reference.id().clone(),
        Some(reference.version()),
        Some(content_hash),
    )
    .map_err(crate::map_domain_error)?;
    lineage_input(role, kind, owner, &lineage, visible_at)
}

#[allow(clippy::too_many_arguments)]
fn catalog_record_input(
    role: String,
    kind: FormalInputKind,
    owner: &OwnerRef,
    reference: &VersionRef,
    content_hash: ContentHash,
    visible_at: &MarketTime,
    effective_from: &MarketTime,
    effective_to: &MarketTime,
) -> ApplicationResult<FormalInputBinding> {
    let lineage = LineageRef::new(
        reference.id().clone(),
        Some(reference.version()),
        Some(content_hash),
    )
    .map_err(crate::map_domain_error)?;
    FormalInputBinding::new(FormalInputBindingInput {
        role,
        kind,
        owner: owner.clone(),
        reference: FormalInputReference::Object(lineage),
        observed_at: None,
        visible_at: Some(visible_at.clone()),
        effective_from: Some(effective_from.clone()),
        effective_to: Some(effective_to.clone()),
    })
    .map_err(crate::map_domain_error)
}

fn validate_catalog_times(
    owner: &OwnerRef,
    subject_ref: &VersionRef,
    effective_from: &MarketTime,
    effective_to: &MarketTime,
    visible_at: &MarketTime,
    temporal: &PortfolioCatalogTemporalScope,
) -> ApplicationResult<()> {
    if owner != temporal.owner()
        || subject_ref != temporal.subject_ref()
        || visible_at.instant() > temporal.knowledge_at().instant()
        || effective_from.instant() > temporal.as_of().instant()
        || effective_to.instant() <= temporal.as_of().instant()
    {
        return Err(integrity());
    }
    Ok(())
}

fn normalized_catalog_inputs(
    resolution: &NormalizedPortfolioContextResolution,
) -> ApplicationResult<Vec<FormalInputBinding>> {
    let context = resolution.context();
    let evidence = resolution.catalog_evidence();
    validate_normalized_catalog_evidence(context, evidence)?;
    let mut member_index = 0_usize;
    evidence
        .iter()
        .map(|binding| {
            let (role, kind) = normalized_catalog_role(binding.role(), &mut member_index);
            catalog_record_input(
                role,
                kind,
                &context.owner,
                binding.reference(),
                binding.content_hash().clone(),
                binding.visible_at(),
                binding.effective_from(),
                binding.effective_to(),
            )
        })
        .collect()
}

fn validate_normalized_catalog_evidence(
    context: &NormalizedPortfolioContext,
    evidence: &[PortfolioCatalogEvidenceBinding],
) -> ApplicationResult<()> {
    let selected = evidence
        .iter()
        .filter(|binding| {
            matches!(
                binding.role(),
                PortfolioCatalogEvidenceRole::SelectedBook
                    | PortfolioCatalogEvidenceRole::SelectedGroup
                    | PortfolioCatalogEvidenceRole::SelectedPortfolio
            )
        })
        .collect::<Vec<_>>();
    let members = evidence
        .iter()
        .filter(|binding| binding.role() == PortfolioCatalogEvidenceRole::MemberPortfolio)
        .collect::<Vec<_>>();
    let benchmarks = evidence
        .iter()
        .filter(|binding| binding.role() == PortfolioCatalogEvidenceRole::Benchmark)
        .collect::<Vec<_>>();
    let conventions = evidence
        .iter()
        .filter(|binding| binding.role() == PortfolioCatalogEvidenceRole::MetricConvention)
        .collect::<Vec<_>>();
    if selected.len() != 1
        || members.len() != context.scope.member_portfolios().len()
        || benchmarks.len() != 1
        || conventions.len() != 1
        || evidence.len() != members.len() + 3
        || !matches!(
            (selected[0].role(), context.scope.selected()),
            (
                PortfolioCatalogEvidenceRole::SelectedBook,
                ExactPortfolioScopeKind::Book(_)
            ) | (
                PortfolioCatalogEvidenceRole::SelectedGroup,
                ExactPortfolioScopeKind::Group(_)
            ) | (
                PortfolioCatalogEvidenceRole::SelectedPortfolio,
                ExactPortfolioScopeKind::Portfolio(_)
            )
        )
        || !catalog_binding_matches_lineage(selected[0], selected_lineage(context.scope.selected()))
        || !catalog_binding_matches_reference(
            benchmarks[0],
            context.benchmark.reference(),
            context.benchmark.content_hash(),
        )
        || !catalog_binding_matches_reference(
            conventions[0],
            context.metric_convention.reference(),
            context.metric_convention.content_hash(),
        )
        || context.scope.member_portfolios().iter().any(|member| {
            members
                .iter()
                .filter(|binding| catalog_binding_matches_lineage(binding, member))
                .count()
                != 1
        })
    {
        return Err(integrity());
    }
    if evidence.iter().any(|binding| {
        binding.visible_at().instant() > context.knowledge_at.instant()
            || binding.effective_from().instant() > context.valuation_at.instant()
            || binding.effective_to().instant() <= context.valuation_at.instant()
    }) {
        return Err(integrity());
    }
    Ok(())
}

fn normalized_catalog_role(
    role: PortfolioCatalogEvidenceRole,
    member_index: &mut usize,
) -> (String, FormalInputKind) {
    match role {
        PortfolioCatalogEvidenceRole::SelectedBook => (
            "normalized_scope_selected".to_owned(),
            FormalInputKind::Book,
        ),
        PortfolioCatalogEvidenceRole::SelectedGroup => (
            "normalized_scope_selected".to_owned(),
            FormalInputKind::PortfolioGroup,
        ),
        PortfolioCatalogEvidenceRole::SelectedPortfolio => (
            "normalized_scope_selected".to_owned(),
            FormalInputKind::Portfolio,
        ),
        PortfolioCatalogEvidenceRole::MemberPortfolio => {
            let role = format!("normalized_portfolio_member_{member_index:04}");
            *member_index += 1;
            (role, FormalInputKind::Portfolio)
        }
        PortfolioCatalogEvidenceRole::Benchmark => (
            "normalized_benchmark".to_owned(),
            FormalInputKind::Benchmark,
        ),
        PortfolioCatalogEvidenceRole::MetricConvention => (
            "normalized_metric_convention".to_owned(),
            FormalInputKind::PortfolioMetricConvention,
        ),
    }
}

fn catalog_binding_matches_lineage(
    binding: &PortfolioCatalogEvidenceBinding,
    reference: &LineageRef,
) -> bool {
    reference.object_id() == binding.reference().id()
        && reference.version() == Some(binding.reference().version())
        && reference.content_hash() == Some(binding.content_hash())
}

fn catalog_binding_matches_reference(
    binding: &PortfolioCatalogEvidenceBinding,
    reference: &VersionRef,
    content_hash: &ContentHash,
) -> bool {
    binding.reference() == reference && binding.content_hash() == content_hash
}

fn lineage_input(
    role: String,
    kind: FormalInputKind,
    owner: &OwnerRef,
    reference: &LineageRef,
    visible_at: Option<MarketTime>,
) -> ApplicationResult<FormalInputBinding> {
    FormalInputBinding::new(FormalInputBindingInput {
        role,
        kind,
        owner: owner.clone(),
        reference: FormalInputReference::Object(reference.clone()),
        observed_at: None,
        visible_at,
        effective_from: None,
        effective_to: None,
    })
    .map_err(crate::map_domain_error)
}

const fn selected_lineage(value: &ExactPortfolioScopeKind) -> &LineageRef {
    match value {
        ExactPortfolioScopeKind::Book(reference)
        | ExactPortfolioScopeKind::Group(reference)
        | ExactPortfolioScopeKind::Portfolio(reference) => reference,
    }
}

fn catalog_evidence_fingerprint(
    catalog: &PortfolioCatalogPage,
    inputs: &[FormalInputBinding],
) -> ApplicationResult<ContentHash> {
    let mut bytes = catalog
        .request_fingerprint()
        .content_hash()
        .as_bytes()
        .to_vec();
    let mut ordered = inputs.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.role().cmp(right.role()));
    for input in ordered {
        if matches!(input.reference(), FormalInputReference::Named(_)) {
            return Err(integrity());
        }
        append(&mut bytes, &input.canonical_bytes());
    }
    Ok(ContentHash::digest(&bytes))
}

fn catalog_command_evidence_fingerprint(
    command: &ListPortfolioCatalogCommand,
    catalog: &PortfolioCatalogPage,
    inputs: &[FormalInputBinding],
) -> ApplicationResult<ContentHash> {
    let page_evidence = catalog_evidence_fingerprint(catalog, inputs)?;
    let mut bytes = b"ficant.portfolio.catalog-read-evidence.v1".to_vec();
    append(
        &mut bytes,
        command.filter().fingerprint().content_hash().as_bytes(),
    );
    append(&mut bytes, command.cursor().unwrap_or_default().as_bytes());
    append(&mut bytes, &command.limit().to_be_bytes());
    append(&mut bytes, page_evidence.as_bytes());
    Ok(ContentHash::digest(&bytes))
}

fn error_envelope(
    page_id: PortfolioWorkbenchPageId,
    request_id: String,
    generated_at: MarketTime,
    permissions: Vec<String>,
    normalized_context: Option<NormalizedPortfolioContext>,
    typed_error: PortfolioWorkbenchTypedError,
) -> PortfolioPageEnvelope {
    PortfolioPageEnvelope {
        schema_version: PORTFOLIO_WORKBENCH_SCHEMA_VERSION,
        page_id,
        request_id,
        generated_at,
        data_mode: PortfolioPageDataMode::Error,
        normalized_context,
        page_state: PortfolioPageState::Blocked,
        permissions,
        provenance: None,
        coverage: None,
        projection: None,
        typed_error: Some(typed_error),
    }
}

fn validate_selection(
    page_id: PortfolioWorkbenchPageId,
    selection: Option<&PortfolioPageSelection>,
) -> ApplicationResult<()> {
    if (page_id == PortfolioWorkbenchPageId::P04) != selection.is_some() {
        return Err(validation());
    }
    Ok(())
}

fn authorize_page(
    principal: &AuthorizedPrincipal,
    page_id: PortfolioWorkbenchPageId,
    owner: &OwnerRef,
) -> ApplicationResult<()> {
    principal.require_role(PlatformRole::Researcher)?;
    principal.access_scope().authorize(owner)?;
    if permissions_for(page_id)
        .iter()
        .any(|required| !principal.has_scope(required))
    {
        return Err(forbidden());
    }
    Ok(())
}

fn permissions_for(page_id: PortfolioWorkbenchPageId) -> Vec<String> {
    let values: &[&str] = match page_id {
        PortfolioWorkbenchPageId::P01 => &["portfolio:read"],
        PortfolioWorkbenchPageId::P03 => &["portfolio:read", "positions:read"],
        PortfolioWorkbenchPageId::P04 => &[
            "portfolio:read",
            "positions:read",
            "rates:analyze",
            "facts:read",
            "definitions:read",
        ],
        PortfolioWorkbenchPageId::D01 | PortfolioWorkbenchPageId::P02 => &[
            "portfolio:read",
            "positions:read",
            "rates:analyze",
            "facts:read",
            "definitions:read",
            "artifacts:read",
        ],
    };
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn typed_error(error: &ApplicationError, trace_id: String) -> PortfolioWorkbenchTypedError {
    let (code, safe_message) = match error.category() {
        ApplicationErrorCategory::Unauthenticated => (
            PortfolioWorkbenchErrorCode::Unauthenticated,
            "authentication required",
        ),
        ApplicationErrorCategory::Forbidden => (
            PortfolioWorkbenchErrorCode::Forbidden,
            "portfolio access denied",
        ),
        ApplicationErrorCategory::NotFound => (
            PortfolioWorkbenchErrorCode::NotFound,
            "portfolio data not found",
        ),
        ApplicationErrorCategory::AlreadyExists
        | ApplicationErrorCategory::VersionConflict
        | ApplicationErrorCategory::ConcurrencyConflict
        | ApplicationErrorCategory::ImmutableViolation
        | ApplicationErrorCategory::StateConflict => (
            PortfolioWorkbenchErrorCode::Conflict,
            "portfolio state conflict",
        ),
        ApplicationErrorCategory::ValidationFailed
        | ApplicationErrorCategory::HashMismatch
        | ApplicationErrorCategory::LineageIncomplete => (
            PortfolioWorkbenchErrorCode::Integrity,
            "portfolio input integrity failure",
        ),
        ApplicationErrorCategory::StorageUnavailable => (
            PortfolioWorkbenchErrorCode::Unavailable,
            "portfolio service unavailable",
        ),
    };
    PortfolioWorkbenchTypedError {
        code,
        safe_message,
        trace_id,
        retryable: error.retryable(),
    }
}

fn typed_source_error(
    error: &PortfolioWorkbenchSourceError,
    trace_id: String,
) -> PortfolioWorkbenchTypedError {
    match error {
        PortfolioWorkbenchSourceError::Application(error) => typed_error(error, trace_id),
        PortfolioWorkbenchSourceError::Stale { retryable } => PortfolioWorkbenchTypedError {
            code: PortfolioWorkbenchErrorCode::Stale,
            safe_message: "portfolio data is stale",
            trace_id,
            retryable: *retryable,
        },
    }
}

fn page_request_fingerprint(
    page_id: PortfolioWorkbenchPageId,
    context: &NormalizedPortfolioContext,
    source_fingerprint: &ContentHash,
) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, b"ficant.portfolio-workbench-page.v1");
    append(&mut bytes, &[page_code(page_id)]);
    append(&mut bytes, context.owner.tenant_id().as_str().as_bytes());
    append(&mut bytes, context.owner.owner_id().as_str().as_bytes());
    append(&mut bytes, context.subject_ref.id().as_str().as_bytes());
    append(
        &mut bytes,
        &context.subject_ref.version().get().to_be_bytes(),
    );
    match context.scope.selected() {
        ExactPortfolioScopeKind::Book(reference) => {
            append(&mut bytes, &[1]);
            append_lineage(&mut bytes, reference);
        }
        ExactPortfolioScopeKind::Group(reference) => {
            append(&mut bytes, &[2]);
            append_lineage(&mut bytes, reference);
        }
        ExactPortfolioScopeKind::Portfolio(reference) => {
            append(&mut bytes, &[3]);
            append_lineage(&mut bytes, reference);
        }
    }
    for member in context.scope.member_portfolios() {
        append_lineage(&mut bytes, member);
    }
    append(
        &mut bytes,
        &context.valuation_at.instant().timestamp().to_be_bytes(),
    );
    append(
        &mut bytes,
        &context
            .valuation_at
            .instant()
            .timestamp_subsec_nanos()
            .to_be_bytes(),
    );
    append(
        &mut bytes,
        &context.knowledge_at.instant().timestamp().to_be_bytes(),
    );
    append(
        &mut bytes,
        &context
            .knowledge_at
            .instant()
            .timestamp_subsec_nanos()
            .to_be_bytes(),
    );
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
    append(
        &mut bytes,
        context.benchmark.reference().id().as_str().as_bytes(),
    );
    append(
        &mut bytes,
        &context.benchmark.reference().version().get().to_be_bytes(),
    );
    append(&mut bytes, context.benchmark.content_hash().as_bytes());
    append(&mut bytes, &[period_code(context.period)]);
    append_market_time(&mut bytes, &context.period_from);
    append_market_time(&mut bytes, &context.period_to);
    append(
        &mut bytes,
        context
            .metric_convention
            .reference()
            .id()
            .as_str()
            .as_bytes(),
    );
    append(
        &mut bytes,
        &context
            .metric_convention
            .reference()
            .version()
            .get()
            .to_be_bytes(),
    );
    append(
        &mut bytes,
        context.metric_convention.content_hash().as_bytes(),
    );
    append(&mut bytes, source_fingerprint.as_bytes());
    ContentHash::digest(&bytes)
}

fn evidence_shape_is_invalid(
    projection: &PortfolioPageProjection,
    formal_evidence: &[FormalOutputEvidence],
    non_formal_reads: &[NonFormalReadEvidence],
) -> bool {
    match projection {
        PortfolioPageProjection::P01(_) => {
            !formal_evidence.is_empty() || non_formal_reads.is_empty()
        }
        PortfolioPageProjection::D01(_)
        | PortfolioPageProjection::P02(_)
        | PortfolioPageProjection::P03(_)
        | PortfolioPageProjection::P04(_) => formal_evidence.is_empty(),
    }
}

fn formal_subject_matches(evidence: &FormalOutputEvidence, subject: &VersionRef) -> bool {
    match evidence.subject().reference() {
        ficant_runtime::FormalInputReference::Object(reference) => {
            reference.object_id() == subject.id() && reference.version() == Some(subject.version())
        }
        ficant_runtime::FormalInputReference::Named(_) => false,
    }
}

fn input_subject_matches(binding: &FormalInputBinding, subject: &VersionRef) -> bool {
    match binding.reference() {
        ficant_runtime::FormalInputReference::Object(reference) => {
            reference.object_id() == subject.id() && reference.version() == Some(subject.version())
        }
        ficant_runtime::FormalInputReference::Named(_) => false,
    }
}

fn append_lineage(bytes: &mut Vec<u8>, reference: &ficant_domain::primitives::LineageRef) {
    append(bytes, reference.object_id().as_str().as_bytes());
    match reference.version() {
        Some(version) => {
            append(bytes, &[1]);
            append(bytes, &version.get().to_be_bytes());
        }
        None => append(bytes, &[0]),
    }
    match reference.content_hash() {
        Some(content_hash) => {
            append(bytes, &[1]);
            append(bytes, content_hash.as_bytes());
        }
        None => append(bytes, &[0]),
    }
}

fn append_market_time(bytes: &mut Vec<u8>, value: &MarketTime) {
    append(bytes, &value.instant().timestamp().to_be_bytes());
    append(
        bytes,
        &value.instant().timestamp_subsec_nanos().to_be_bytes(),
    );
    append(bytes, value.market_timezone().as_bytes());
    append(bytes, value.local_trading_date().to_string().as_bytes());
}

const fn currency_code(value: crate::ports::PortfolioCurrencyMode) -> u8 {
    match value {
        crate::ports::PortfolioCurrencyMode::Original => 1,
        crate::ports::PortfolioCurrencyMode::Cny => 2,
    }
}

const fn look_through_code(value: crate::ports::PortfolioLookThroughMode) -> u8 {
    match value {
        crate::ports::PortfolioLookThroughMode::None => 1,
        crate::ports::PortfolioLookThroughMode::Consolidated => 2,
        crate::ports::PortfolioLookThroughMode::Separate => 3,
    }
}

const fn period_code(value: crate::ports::PortfolioPeriodPreset) -> u8 {
    match value {
        crate::ports::PortfolioPeriodPreset::OneDay => 1,
        crate::ports::PortfolioPeriodPreset::SevenDays => 2,
        crate::ports::PortfolioPeriodPreset::ThirtyDays => 3,
        crate::ports::PortfolioPeriodPreset::YearToDate => 4,
        crate::ports::PortfolioPeriodPreset::OneYear => 5,
    }
}

const fn page_code(page_id: PortfolioWorkbenchPageId) -> u8 {
    match page_id {
        PortfolioWorkbenchPageId::D01 => 1,
        PortfolioWorkbenchPageId::P01 => 2,
        PortfolioWorkbenchPageId::P02 => 3,
        PortfolioWorkbenchPageId::P03 => 4,
        PortfolioWorkbenchPageId::P04 => 5,
    }
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

fn integrity() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use ficant_domain::market::{FactSource, Valuation, ValuationInput, ValuationValueRole};
    use ficant_domain::primitives::{DecimalValue, Ulid, UnitRef, Version};

    use super::*;

    #[test]
    fn p04_fact_evidence_is_exact_and_content_drift_fails_closed() {
        let fact = valuation_fact("10123");
        let binding = crate::ports::PortfolioValuationAuthorityBinding {
            valuation_id: fact.id().clone(),
            source_revision: fact.source_revision(),
            content_hash: market_fact_content_hash(&fact),
            value_index: 0,
        };
        assert!(validate_valuation_binding(&binding, std::slice::from_ref(&fact)).is_ok());

        let input = p04_fact_input(
            "fact_0000".to_owned(),
            portfolio_fact_input_kind(),
            &test_owner(),
            &fact,
        )
        .expect("typed Fact evidence");
        assert_eq!(input.kind(), FormalInputKind::Fact);
        assert!(matches!(
            input.reference(),
            FormalInputReference::Object(reference)
                if reference.object_id() == fact.id()
                    && reference.version() == Some(Version::new(1).expect("source revision"))
                    && reference.content_hash() == Some(&binding.content_hash)
        ));

        let drifted = valuation_fact("10124");
        assert_eq!(drifted.id(), fact.id());
        assert_eq!(drifted.source_revision(), fact.source_revision());
        assert_ne!(market_fact_content_hash(&drifted), binding.content_hash);
        assert!(validate_valuation_binding(&binding, &[drifted]).is_err());
    }

    fn valuation_fact(coefficient: &str) -> MarketFact {
        MarketFact::Valuation(
            Valuation::new_with_value_roles(
                ValuationInput {
                    valuation_id: test_id('V'),
                    instrument: VersionRef::new(
                        test_id('J'),
                        Version::new(1).expect("instrument version"),
                    ),
                    owner: test_owner(),
                    source: FactSource::new("test-source", "valuation", 1).expect("fact source"),
                    valuation_at: test_time(),
                    method: "external-price".to_owned(),
                    rule_pack: VersionRef::new(
                        test_id('R'),
                        Version::new(1).expect("rule version"),
                    ),
                    values: vec![
                        DecimalValue::new(
                            coefficient,
                            2,
                            UnitRef::new(test_id('W'), Version::new(1).expect("unit version")),
                        )
                        .expect("valuation value"),
                    ],
                    supersedes_id: None,
                },
                vec![ValuationValueRole::Price],
            )
            .expect("valuation"),
        )
    }

    fn test_owner() -> OwnerRef {
        OwnerRef::new(test_id('T'), test_id('N'))
    }

    fn test_time() -> MarketTime {
        let instant = Utc.with_ymd_and_hms(2026, 8, 21, 9, 0, 0).unwrap();
        let local_date = instant
            .with_timezone(&chrono_tz::Asia::Shanghai)
            .date_naive();
        MarketTime::new(instant, "Asia/Shanghai", local_date).expect("market time")
    }

    fn test_id(suffix: char) -> Ulid {
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).expect("ULID")
    }
}
