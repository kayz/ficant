use async_trait::async_trait;
use ficant_domain::market::{Cashflow, CurveSnapshot, Quote, Trade, Valuation};
use ficant_domain::primitives::{LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged};

use super::blob_store::VerifiedBlobRef;
use super::fingerprint::{
    FingerprintBuilder, curve_snapshot_bytes, fact_bytes, market_time_bytes, owner_bytes,
    version_ref_bytes,
};
use super::{
    AccessScope, ApplicationResult, CursorPage, FullyValidatedMarketFact, IdempotencyKey,
    MarketFactRuleProof, OperationFingerprint, PageRequest, ResolvedMarketFactProof,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

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

#[async_trait]
pub trait MarketFactRepository: Send + Sync {
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
