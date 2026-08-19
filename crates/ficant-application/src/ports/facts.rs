use async_trait::async_trait;
use ficant_domain::governance::{
    ChangeJustification, FoundationChangeOperation, FoundationChangeRecord,
    FoundationChangeRecordInput, FoundationResourceKind, FoundationResourceRef, PlatformRole,
};
use ficant_domain::market::{Cashflow, CurveSnapshot, Quote, Trade, Valuation};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged};

use super::blob_store::VerifiedBlobRef;
use super::fingerprint::{
    FingerprintBuilder, curve_snapshot_bytes, fact_bytes, market_time_bytes, owner_bytes,
    version_ref_bytes,
};
use super::{
    AccessScope, ApplicationResult, CursorPage, FoundationChangeContext, FullyValidatedMarketFact,
    IdempotencyKey, MarketFactRuleProof, OperationFingerprint, PageRequest,
    ResolvedMarketFactProof,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

pub const MARKET_FACT_WRITE_SCOPE: &str = "facts:write";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarketFact {
    Cashflow(Cashflow),
    Quote(Quote),
    Trade(Trade),
    Valuation(Valuation),
}

impl MarketFact {
    #[must_use]
    pub fn id(&self) -> &Ulid {
        match self {
            Self::Cashflow(value) => value.id(),
            Self::Quote(value) => value.id(),
            Self::Trade(value) => value.id(),
            Self::Valuation(value) => value.id(),
        }
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        match self {
            Self::Cashflow(value) => value.owner(),
            Self::Quote(value) => value.owner(),
            Self::Trade(value) => value.owner(),
            Self::Valuation(value) => value.owner(),
        }
    }

    #[must_use]
    pub fn source_revision(&self) -> u64 {
        match self {
            Self::Cashflow(value) => value.source().source_revision(),
            Self::Quote(value) => value.source().source_revision(),
            Self::Trade(value) => value.source().source_revision(),
            Self::Valuation(value) => value.source().source_revision(),
        }
    }

    /// Returns the exact fact ID + source-revision lineage reference.
    ///
    /// # Errors
    ///
    /// Returns validation failure if an invalid fact bypassed its domain constructor.
    pub fn lineage_ref(&self) -> ApplicationResult<LineageRef> {
        let revision = Version::new(self.source_revision()).map_err(map_domain_error)?;
        Ok(LineageRef::versioned(self.id().clone(), revision))
    }

    fn supersedes_id(&self) -> Option<&Ulid> {
        match self {
            Self::Cashflow(value) => value.supersedes_id(),
            Self::Quote(value) => value.supersedes_id(),
            Self::Trade(value) => value.supersedes_id(),
            Self::Valuation(value) => value.supersedes_id(),
        }
    }
}

/// Returns the immutable canonical identity of one stored `MarketFact` payload.
#[must_use]
pub fn market_fact_content_hash(fact: &MarketFact) -> ContentHash {
    ContentHash::digest(&fact_bytes(fact))
}

/// Raw facts cannot construct append commands without resolved-unit evidence.
///
/// ```compile_fail
/// use ficant_application::ports::{AppendMarketFact, MarketFact};
/// use ficant_application::IdempotencyKey;
/// let raw: MarketFact = panic!();
/// let _ = AppendMarketFact::new(raw, IdempotencyKey::new("append").unwrap());
/// ```
///
/// Unit-only evidence also cannot construct an append command.
///
/// ```compile_fail
/// use ficant_application::ports::{AppendMarketFact, ValidatedMarketFact};
/// use ficant_application::IdempotencyKey;
/// let unit_only: ValidatedMarketFact = panic!();
/// let _ = AppendMarketFact::new(unit_only, IdempotencyKey::new("append").unwrap());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendMarketFact {
    fact: MarketFact,
    proof: ResolvedMarketFactProof,
    rule_proof: MarketFactRuleProof,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl AppendMarketFact {
    /// Creates an append intent only from a fact carrying a valid resolved-unit proof.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the proof shape or fact binding is invalid.
    pub fn new(
        validated: FullyValidatedMarketFact,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        validated.validate()?;
        let (fact, proof, rule_proof) = validated.into_parts();
        if fact.supersedes_id().is_some() {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let mut canonical = FingerprintBuilder::new("append-market-fact/v1");
        canonical.field(2, &fact_bytes(&fact));
        let fingerprint = canonical.finish();
        Ok(Self {
            fact,
            proof,
            rule_proof,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn fact(&self) -> &MarketFact {
        &self.fact
    }

    #[must_use]
    pub fn proof(&self) -> &ResolvedMarketFactProof {
        &self.proof
    }

    #[must_use]
    pub fn rule_proof(&self) -> &MarketFactRuleProof {
        &self.rule_proof
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
}

/// Raw corrections cannot bypass resolved-unit validation.
///
/// ```compile_fail
/// use ficant_application::ports::{CorrectMarketFact, MarketFact};
/// use ficant_application::IdempotencyKey;
/// use ficant_domain::primitives::Ulid;
/// let raw: MarketFact = panic!();
/// let original: Ulid = panic!();
/// let _ = CorrectMarketFact::new(original, raw, IdempotencyKey::new("correct").unwrap());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorrectMarketFact {
    original_fact_id: Ulid,
    correction: MarketFact,
    proof: ResolvedMarketFactProof,
    rule_proof: MarketFactRuleProof,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl CorrectMarketFact {
    /// Creates a correction explicitly linked to the immutable original fact.
    ///
    /// # Errors
    ///
    /// Returns incomplete lineage unless `correction.supersedes_id` is the original identity.
    pub fn new(
        original_fact_id: Ulid,
        validated: FullyValidatedMarketFact,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        validated.validate()?;
        let (correction, proof, rule_proof) = validated.into_parts();
        if correction.supersedes_id() != Some(&original_fact_id) {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let mut canonical = FingerprintBuilder::new("correct-market-fact/v1");
        canonical.field(2, original_fact_id.as_str().as_bytes());
        canonical.field(3, &fact_bytes(&correction));
        let fingerprint = canonical.finish();
        Ok(Self {
            original_fact_id,
            correction,
            proof,
            rule_proof,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn original_fact_id(&self) -> &Ulid {
        &self.original_fact_id
    }

    #[must_use]
    pub fn correction(&self) -> &MarketFact {
        &self.correction
    }

    #[must_use]
    pub fn proof(&self) -> &ResolvedMarketFactProof {
        &self.proof
    }

    #[must_use]
    pub fn rule_proof(&self) -> &MarketFactRuleProof {
        &self.rule_proof
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketFactWindow {
    instrument: VersionRef,
    from: MarketTime,
    to: MarketTime,
    page: PageRequest,
}

impl MarketFactWindow {
    /// Creates an exact-instrument, closed time-window cursor query.
    ///
    /// # Errors
    ///
    /// Returns validation failure when `from` is after `to`.
    pub fn new(
        instrument: VersionRef,
        from: MarketTime,
        to: MarketTime,
        page: PageRequest,
    ) -> ApplicationResult<Self> {
        if from.instant() > to.instant() {
            return Err(map_domain_error(DomainErrorCode::InvalidEffectiveTime));
        }
        Ok(Self {
            instrument,
            from,
            to,
            page,
        })
    }

    #[must_use]
    pub fn instrument(&self) -> &VersionRef {
        &self.instrument
    }

    #[must_use]
    pub fn from(&self) -> &MarketTime {
        &self.from
    }

    #[must_use]
    pub fn to(&self) -> &MarketTime {
        &self.to
    }

    #[must_use]
    pub fn page(&self) -> &PageRequest {
        &self.page
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        self.page.scope()
    }

    /// Verifies the explicit repository scope against the scope bound into this query cursor.
    ///
    /// # Errors
    ///
    /// Returns forbidden when the two scopes differ.
    pub fn authorize_scope(&self, scope: &AccessScope) -> ApplicationResult<()> {
        self.page.authorize_scope(scope)
    }

    #[must_use]
    pub fn fingerprint(&self) -> OperationFingerprint {
        let mut canonical = FingerprintBuilder::new("market-fact-window/v1");
        canonical.field(2, &version_ref_bytes(&self.instrument));
        canonical.field(3, &market_time_bytes(&self.from));
        canonical.field(4, &market_time_bytes(&self.to));
        canonical.field(5, self.page.fingerprint().content_hash().as_bytes());
        canonical.finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishCurveSnapshot {
    scope: AccessScope,
    curve: CurveSnapshot,
    declared_blob_size: u64,
    verified_blob: VerifiedBlobRef,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl PublishCurveSnapshot {
    /// Creates an immutable `CurveSnapshot` publication command outside the `MarketFact` oneof.
    ///
    /// # Errors
    ///
    /// Returns forbidden for unauthorized ownership, hash mismatch for different verified content,
    /// validation failure for size drift, or incomplete lineage for missing source lineage.
    pub fn new(
        scope: AccessScope,
        curve: CurveSnapshot,
        declared_blob_size: u64,
        verified_blob: VerifiedBlobRef,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        scope.authorize(curve.owner())?;
        if curve.content_hash() != verified_blob.content_hash() {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        if declared_blob_size == 0 || declared_blob_size != verified_blob.size() {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        if curve.lineage().is_empty() {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let idempotency_key = idempotency_key.scoped_to(&scope)?;
        let mut canonical = FingerprintBuilder::new("publish-curve-snapshot/v1");
        canonical.field(2, scope.fingerprint().content_hash().as_bytes());
        canonical.field(3, &owner_bytes(curve.owner()));
        canonical.field(4, &curve_snapshot_bytes(&curve));
        canonical.field(5, verified_blob.content_hash().as_bytes());
        canonical.u64(6, verified_blob.size());
        let fingerprint = canonical.finish();
        Ok(Self {
            scope,
            curve,
            declared_blob_size,
            verified_blob,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        &self.scope
    }

    #[must_use]
    pub fn curve(&self) -> &CurveSnapshot {
        &self.curve
    }

    #[must_use]
    pub fn declared_blob_size(&self) -> u64 {
        self.declared_blob_size
    }

    #[must_use]
    pub fn verified_blob(&self) -> &VerifiedBlobRef {
        &self.verified_blob
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }

    /// Verifies a same-request replay against an existing immutable publication command.
    ///
    /// # Errors
    ///
    /// Returns validation failure for a different identity and immutable violation when the same
    /// identity is presented with a different key, scope, metadata, lineage, hash, or size.
    pub fn ensure_replay_compatible(&self, existing: &Self) -> ApplicationResult<()> {
        if self.curve.id() != existing.curve.id() {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        if self.idempotency_key != existing.idempotency_key
            || self.fingerprint != existing.fingerprint
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernedAppendMarketFact {
    change_context: FoundationChangeContext,
    append: AppendMarketFact,
    fingerprint: OperationFingerprint,
}

impl GovernedAppendMarketFact {
    /// Creates the only administrator command accepted by the R6A direct Fact append path.
    ///
    /// # Errors
    ///
    /// Returns authorization, validation, or idempotency failure.
    pub fn new(
        change_context: FoundationChangeContext,
        validated: FullyValidatedMarketFact,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        change_context.principal().authorize_mutation(
            PlatformRole::PlatformAdmin,
            MARKET_FACT_WRITE_SCOPE,
            validated.fact().owner(),
        )?;
        validated.authorize_scope(change_context.principal().access_scope())?;
        let append = AppendMarketFact::new(validated, idempotency_key)?;
        let fingerprint = governed_fingerprint(
            "governed-append-market-fact/v1",
            &change_context,
            append.fingerprint(),
            None,
        );
        Ok(Self {
            change_context,
            append,
            fingerprint,
        })
    }

    #[must_use]
    pub fn change_context(&self) -> &FoundationChangeContext {
        &self.change_context
    }
    #[must_use]
    pub fn command(&self) -> &AppendMarketFact {
        &self.append
    }
    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
    /// Builds the immutable audit record.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the audit record cannot be materialized.
    pub fn change_record(&self) -> ApplicationResult<FoundationChangeRecord> {
        fact_change_record(
            &self.change_context,
            FoundationChangeOperation::AppendMarketFact,
            self.append.fact(),
            None,
            &self.fingerprint,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernedCorrectMarketFact {
    change_context: FoundationChangeContext,
    correction: CorrectMarketFact,
    fingerprint: OperationFingerprint,
}

impl GovernedCorrectMarketFact {
    /// Creates the only administrator command accepted by the R6A Fact correction path.
    ///
    /// # Errors
    ///
    /// Returns authorization, validation, lineage, or idempotency failure.
    pub fn new(
        change_context: FoundationChangeContext,
        original_fact_id: Ulid,
        validated: FullyValidatedMarketFact,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        change_context.principal().authorize_mutation(
            PlatformRole::PlatformAdmin,
            MARKET_FACT_WRITE_SCOPE,
            validated.fact().owner(),
        )?;
        validated.authorize_scope(change_context.principal().access_scope())?;
        let correction = CorrectMarketFact::new(original_fact_id, validated, idempotency_key)?;
        let fingerprint = governed_fingerprint(
            "governed-correct-market-fact/v1",
            &change_context,
            correction.fingerprint(),
            None,
        );
        Ok(Self {
            change_context,
            correction,
            fingerprint,
        })
    }

    #[must_use]
    pub fn change_context(&self) -> &FoundationChangeContext {
        &self.change_context
    }
    #[must_use]
    pub fn command(&self) -> &CorrectMarketFact {
        &self.correction
    }
    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
    /// Builds the immutable correction audit record.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the audit record cannot be materialized.
    pub fn change_record(
        &self,
        before_hash: ContentHash,
    ) -> ApplicationResult<FoundationChangeRecord> {
        fact_change_record(
            &self.change_context,
            FoundationChangeOperation::CorrectMarketFact,
            self.correction.correction(),
            Some(before_hash),
            &self.fingerprint,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernedPublishCurveSnapshot {
    change_context: FoundationChangeContext,
    publish: PublishCurveSnapshot,
    fingerprint: OperationFingerprint,
}

impl GovernedPublishCurveSnapshot {
    /// Creates the only administrator command accepted by the R6A Curve publication path.
    ///
    /// # Errors
    ///
    /// Returns authorization, blob, validation, or idempotency failure.
    pub fn new(
        change_context: FoundationChangeContext,
        curve: CurveSnapshot,
        declared_blob_size: u64,
        verified_blob: VerifiedBlobRef,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        change_context.principal().authorize_mutation(
            PlatformRole::PlatformAdmin,
            MARKET_FACT_WRITE_SCOPE,
            curve.owner(),
        )?;
        let publish = PublishCurveSnapshot::new(
            change_context.principal().access_scope().clone(),
            curve,
            declared_blob_size,
            verified_blob,
            idempotency_key,
        )?;
        let fingerprint = governed_fingerprint(
            "governed-publish-curve-snapshot/v1",
            &change_context,
            publish.fingerprint(),
            None,
        );
        Ok(Self {
            change_context,
            publish,
            fingerprint,
        })
    }

    #[must_use]
    pub fn change_context(&self) -> &FoundationChangeContext {
        &self.change_context
    }
    #[must_use]
    pub fn command(&self) -> &PublishCurveSnapshot {
        &self.publish
    }
    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
    /// Builds the immutable curve publication audit record.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the audit record cannot be materialized.
    pub fn change_record(&self) -> ApplicationResult<FoundationChangeRecord> {
        FoundationChangeRecord::new(FoundationChangeRecordInput {
            record_id: self.change_context.record_id().clone(),
            actor_id: self.change_context.principal().actor_id().clone(),
            owner: self.publish.curve().owner().clone(),
            active_role: PlatformRole::PlatformAdmin,
            operation: FoundationChangeOperation::PublishCurveSnapshot,
            resource: FoundationResourceRef::unversioned(
                FoundationResourceKind::CurveSnapshot,
                self.publish.curve().id().clone(),
            ),
            before_hash: None,
            after_hash: self.publish.curve().content_hash().clone(),
            change: self.change_context.change().clone(),
            request_fingerprint: self.fingerprint.content_hash().clone(),
            occurred_at: self.change_context.occurred_at().clone(),
            authorization_ref: None,
        })
        .map_err(map_domain_error)
    }
}

fn fact_change_record(
    context: &FoundationChangeContext,
    operation: FoundationChangeOperation,
    fact: &MarketFact,
    before_hash: Option<ContentHash>,
    fingerprint: &OperationFingerprint,
) -> ApplicationResult<FoundationChangeRecord> {
    FoundationChangeRecord::new(FoundationChangeRecordInput {
        record_id: context.record_id().clone(),
        actor_id: context.principal().actor_id().clone(),
        owner: fact.owner().clone(),
        active_role: PlatformRole::PlatformAdmin,
        operation,
        resource: FoundationResourceRef::unversioned(
            FoundationResourceKind::MarketFact,
            fact.id().clone(),
        ),
        before_hash,
        after_hash: market_fact_content_hash(fact),
        change: context.change().clone(),
        request_fingerprint: fingerprint.content_hash().clone(),
        occurred_at: context.occurred_at().clone(),
        authorization_ref: None,
    })
    .map_err(map_domain_error)
}

fn governed_fingerprint(
    namespace: &'static str,
    context: &FoundationChangeContext,
    command: &OperationFingerprint,
    authorization: Option<&VersionRef>,
) -> OperationFingerprint {
    let mut canonical = FingerprintBuilder::new(namespace);
    canonical.field(
        2,
        context.principal().fingerprint().content_hash().as_bytes(),
    );
    canonical.field(3, command.content_hash().as_bytes());
    canonical.field(4, &change_bytes(context.change()));
    if let Some(reference) = authorization {
        canonical.field(5, &version_ref_bytes(reference));
    }
    canonical.finish()
}

fn change_bytes(change: &ChangeJustification) -> Vec<u8> {
    let mut bytes = change.reason().as_bytes().to_vec();
    for source in change.sources() {
        bytes.extend_from_slice(source.uri().as_bytes());
        bytes.extend_from_slice(source.sha256().as_bytes());
    }
    bytes
}

#[async_trait]
pub trait MarketFactRepository: Send + Sync {
    async fn append_governed_fact(
        &self,
        _command: GovernedAppendMarketFact,
    ) -> ApplicationResult<MarketFact> {
        Err(ApplicationError::new(
            ApplicationErrorCategory::StateConflict,
            false,
        ))
    }

    async fn append_governed_correction(
        &self,
        _command: GovernedCorrectMarketFact,
    ) -> ApplicationResult<MarketFact> {
        Err(ApplicationError::new(
            ApplicationErrorCategory::StateConflict,
            false,
        ))
    }

    async fn publish_governed_curve_snapshot(
        &self,
        _command: GovernedPublishCurveSnapshot,
    ) -> ApplicationResult<CurveSnapshot> {
        Err(ApplicationError::new(
            ApplicationErrorCategory::StateConflict,
            false,
        ))
    }

    /// Appends an immutable market fact.
    ///
    /// # Errors
    ///
    /// Returns an application error on validation or idempotency conflict.
    async fn append_fact(&self, command: AppendMarketFact) -> ApplicationResult<MarketFact>;

    /// Appends a correction linked to an earlier fact.
    ///
    /// # Errors
    ///
    /// Returns an application error when the original or correction is invalid.
    async fn append_correction(&self, command: CorrectMarketFact) -> ApplicationResult<MarketFact>;

    /// Queries a stable cursor page after tenant and allowed-owner filtering.
    ///
    /// # Errors
    ///
    /// Implementations must reject scope drift and apply tenant plus allowed-owner predicates
    /// before reading an exact instrument version and time window.
    async fn query_instrument_window(
        &self,
        scope: &AccessScope,
        query: MarketFactWindow,
    ) -> ApplicationResult<CursorPage<MarketFact>>;

    /// Publishes one immutable `CurveSnapshot` with verified content and complete lineage.
    ///
    /// # Errors
    ///
    /// Returns an application error for authorization, hash, size, lineage, idempotency, or
    /// immutable identity conflict.
    async fn publish_curve_snapshot(
        &self,
        command: PublishCurveSnapshot,
    ) -> ApplicationResult<CurveSnapshot>;

    /// Reads `CurveSnapshot` metadata after tenant and allowed-owner filtering.
    ///
    /// # Errors
    ///
    /// Returns an application error when the scoped query cannot be completed safely.
    async fn get_curve_snapshot(
        &self,
        scope: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> ApplicationResult<Option<CurveSnapshot>>;
}

/// Application boundary for validated immutable facts, corrections, queries, and curve metadata.
pub struct MarketFactUseCase<'a> {
    repository: &'a dyn MarketFactRepository,
}

impl<'a> MarketFactUseCase<'a> {
    #[must_use]
    pub const fn new(repository: &'a dyn MarketFactRepository) -> Self {
        Self { repository }
    }

    /// Appends one governed immutable fact.
    ///
    /// # Errors
    ///
    /// Returns a classified authorization, validation, lineage, or repository error.
    pub async fn append_governed(
        &self,
        command: GovernedAppendMarketFact,
    ) -> ApplicationResult<MarketFact> {
        self.repository.append_governed_fact(command).await
    }

    /// Appends one governed immutable correction.
    ///
    /// # Errors
    ///
    /// Returns a classified authorization, validation, lineage, or repository error.
    pub async fn correct_governed(
        &self,
        command: GovernedCorrectMarketFact,
    ) -> ApplicationResult<MarketFact> {
        self.repository.append_governed_correction(command).await
    }

    /// Publishes one governed immutable curve snapshot.
    ///
    /// # Errors
    ///
    /// Returns a classified authorization, blob, lineage, or repository error.
    pub async fn publish_curve_governed(
        &self,
        command: GovernedPublishCurveSnapshot,
    ) -> ApplicationResult<CurveSnapshot> {
        self.repository
            .publish_governed_curve_snapshot(command)
            .await
    }

    /// Appends one legacy internally validated fact under an exact scope.
    ///
    /// # Errors
    ///
    /// Returns a classified scope, validation, lineage, or repository error.
    pub async fn append(
        &self,
        scope: &AccessScope,
        command: AppendMarketFact,
    ) -> ApplicationResult<MarketFact> {
        scope.authorize(command.fact().owner())?;
        self.repository.append_fact(command).await
    }

    /// Appends one legacy internally validated correction under an exact scope.
    ///
    /// # Errors
    ///
    /// Returns a classified scope, validation, lineage, or repository error.
    pub async fn correct(
        &self,
        scope: &AccessScope,
        command: CorrectMarketFact,
    ) -> ApplicationResult<MarketFact> {
        scope.authorize(command.correction().owner())?;
        self.repository.append_correction(command).await
    }

    /// Queries one exact instrument fact window.
    ///
    /// # Errors
    ///
    /// Returns a classified scope, cursor, validation, or repository error.
    pub async fn query(
        &self,
        scope: &AccessScope,
        query: MarketFactWindow,
    ) -> ApplicationResult<CursorPage<MarketFact>> {
        query.authorize_scope(scope)?;
        self.repository.query_instrument_window(scope, query).await
    }

    /// Publishes one legacy internally validated curve snapshot.
    ///
    /// # Errors
    ///
    /// Returns a classified blob, lineage, validation, or repository error.
    pub async fn publish_curve(
        &self,
        command: PublishCurveSnapshot,
    ) -> ApplicationResult<CurveSnapshot> {
        self.repository.publish_curve_snapshot(command).await
    }

    /// Reads one exact immutable curve snapshot.
    ///
    /// # Errors
    ///
    /// Returns a classified scope, integrity, or repository error.
    pub async fn get_curve(
        &self,
        scope: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> ApplicationResult<Option<CurveSnapshot>> {
        self.repository
            .get_curve_snapshot(scope, curve_snapshot_id)
            .await
    }
}
