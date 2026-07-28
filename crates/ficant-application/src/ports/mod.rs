mod access;
mod analytics;
mod artifacts;
mod blob_store;
mod cursor;
mod curves;
mod data_sources;
mod definitions;
mod execution;
mod facts;
mod fingerprint;
mod futures_delivery;
mod futures_hedge;
mod journal;
mod phase4_execution;
mod required_reads;
mod rule_pack_parser;
mod rule_pack_resolution;
mod runs;
mod signals;
mod snapshots;
mod subjects;
mod unit_resolution;

pub use access::AccessScope;
pub use analytics::{
    BondAnalyticsArtifactCodec, BondAnalyticsEngine, EncodedBondAnalyticsArtifact,
};
pub use artifacts::{ArtifactRepository, PublishArtifact};
pub use blob_store::{BeginBlobStage, BlobStore, StagedBlobRef, VerifiedBlobRef, VerifyBlobStage};
pub use cursor::{AeadCursorCodec, Cursor, CursorKey};
pub use curves::{
    CarryRollArtifactCodec, CarryRollEngine, EncodedCarryRollArtifact, YieldCurveEngine,
};
pub use data_sources::{DataSourceRepository, RegisterDataSource};
pub use definitions::{
    AppendDefinitionVersion, DefinitionIdentity, DefinitionKind, DefinitionRepository,
    DefinitionValue, InstrumentDefinition, InstrumentSubtype,
};
pub use execution::{
    Clock, IdGenerator, Phase1AtomicWork, Phase1PublicationWork, Phase1RunWork, TransactionRunner,
};
pub use facts::{
    AppendMarketFact, CorrectMarketFact, MarketFact, MarketFactRepository, MarketFactWindow,
    PublishCurveSnapshot,
};
pub use fingerprint::OperationFingerprint;
pub use futures_delivery::{
    EncodedFuturesDeliveryArtifact, FuturesDeliveryArtifactCodec, FuturesDeliveryEngine,
};
pub use futures_hedge::{
    EncodedFuturesHedgeArtifact, FuturesHedgeArtifactCodec, FuturesHedgeEngine,
};
pub use journal::{AppendJournalEvent, RunJournalRepository};
pub use phase4_execution::{
    BeginNode, ComparisonDimension, CompleteNode, EnqueueNode, ExecutionExternalInput,
    ExecutionInstanceIdentity, ExternalInputArtifactBinding, FailNode, GraphNodeEvent,
    GraphReplayResult, GraphRunComparison, GraphRunRecord, NodeBeginResult, NodeFailureResult,
    NodeImplementation, NodeJournalEvidence, NodeLeaseFence, NodeSuccessResult, OutputTrace,
    Phase4ExecutionRepository, ReproducibilityIdentity, ReproducibilityIdentityInput,
    RulePackBinding, StoredExecutionIdentity, StoredNodeManifest, SubmitGraphRun,
    replay_graph_execution, stable_node_artifact_id,
};
pub(crate) use required_reads::SnapshotVerifiedReadMetadataParts;
pub use required_reads::{
    IntegrityEvent, IntegrityEventSeverity, IntegrityEventSink, IntegrityFailureReason,
    REQUIRED_BLOB_INTEGRITY_EVENT_NAME, RequiredVerifiedBlobRead, SafeTraceContext,
    SnapshotVerifiedReadMetadata, SnapshotVerifiedReadMetadataRepository, VerifiedBlobPayload,
    VerifiedBlobReader, VerifiedBlobRole, VerifiedReadResourceKind,
};
pub use rule_pack_parser::FuturesDeliveryRuleParser;
pub use rule_pack_resolution::{
    FullyValidatedMarketFact, MarketFactRulePackResolver, MarketFactRuleProof,
    MarketFactRuleProofKind, MarketRunRulePackResolver, Phase1ResolvedRunRuleProof,
    Phase1RunCandidateResolver, Phase1ValidatedExperimentRun, ResolvedRunRuleBinding,
    ResolvedRunRuleProof, ResolvedValuationRuleProof, ValidatedExperimentRun,
};
pub use runs::{CreateExperimentRun, ExperimentRepository, TransitionExperimentRun};
pub use signals::{PublishSignalSet, SignalRepository};
pub(crate) use snapshots::StagedSnapshotProofParts;
pub use snapshots::{
    PublishSnapshot, SnapshotBlobRole, SnapshotProofKind, SnapshotRepository, SnapshotValue,
    StagedSnapshotBlob, StagedSnapshotProof, VerifiedSnapshotBlob, VerifiedSnapshotProof,
};
pub use subjects::SubjectRepository;
pub use unit_resolution::{
    MarketFactFieldRole, MarketFactKind, MarketFactUnitResolver, ResolvedMarketFactProof,
    ResolvedUnitBinding, ValidatedMarketFact,
};

use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::DomainErrorCode;

pub type ApplicationResult<T> = Result<T, ApplicationError>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Creates a stable application idempotency key.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the key is blank or padded.
    pub fn new(value: impl Into<String>) -> ApplicationResult<Self> {
        let value = value.into();
        if value.trim().is_empty() || value != value.trim() {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn scoped(&self, suffix: &str) -> ApplicationResult<Self> {
        Self::new(format!("{}/{suffix}", self.as_str()))
    }

    pub(crate) fn scoped_to(self, scope: &AccessScope) -> ApplicationResult<Self> {
        let suffix = scope.fingerprint().content_hash().as_bytes().iter().fold(
            String::with_capacity(64),
            |mut encoded, byte| {
                use std::fmt::Write as _;
                write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
                encoded
            },
        );
        self.scoped(&format!("scope-{suffix}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageRequest {
    scope: AccessScope,
    cursor: Option<Cursor>,
    limit: u32,
    fingerprint: OperationFingerprint,
}

impl PageRequest {
    pub const MAX_LIMIT: u32 = 1_000;

    /// Creates a cursor page request.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless the limit is in `1..=1000`.
    pub fn new(scope: AccessScope, cursor: Option<Cursor>, limit: u32) -> ApplicationResult<Self> {
        if let Some(cursor) = cursor.as_ref() {
            cursor.authorize_scope(&scope)?;
        }
        if limit == 0 || limit > Self::MAX_LIMIT {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        let mut canonical = fingerprint::FingerprintBuilder::new("page-request/v1");
        canonical.field(2, scope.fingerprint().content_hash().as_bytes());
        canonical.field(
            3,
            cursor
                .as_ref()
                .map_or(&[], |value| value.as_str().as_bytes()),
        );
        canonical.u64(4, u64::from(limit));
        let fingerprint = canonical.finish();
        Ok(Self {
            scope,
            cursor,
            limit,
            fingerprint,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        &self.scope
    }

    #[must_use]
    pub fn cursor(&self) -> Option<&Cursor> {
        self.cursor.as_ref()
    }

    #[must_use]
    pub fn limit(&self) -> u32 {
        self.limit
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }

    /// Verifies that a repository call uses the same scope bound into this cursor request.
    ///
    /// # Errors
    ///
    /// Returns forbidden when the explicit repository scope and cursor scope differ.
    pub fn authorize_scope(&self, scope: &AccessScope) -> ApplicationResult<()> {
        if &self.scope != scope {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::Forbidden,
                false,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorPage<T> {
    items: Vec<T>,
    next_cursor: Option<Cursor>,
}

impl<T> CursorPage<T> {
    #[must_use]
    pub fn new(items: Vec<T>, next_cursor: Option<Cursor>) -> Self {
        Self { items, next_cursor }
    }

    #[must_use]
    pub fn items(&self) -> &[T] {
        &self.items
    }

    #[must_use]
    pub fn into_items(self) -> Vec<T> {
        self.items
    }

    #[must_use]
    pub fn next_cursor(&self) -> Option<&Cursor> {
        self.next_cursor.as_ref()
    }

    #[must_use]
    pub fn into_next_cursor(self) -> Option<Cursor> {
        self.next_cursor
    }

    #[must_use]
    pub fn into_parts(self) -> (Vec<T>, Option<Cursor>) {
        (self.items, self.next_cursor)
    }
}

pub(crate) fn cursor_cycle_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}
