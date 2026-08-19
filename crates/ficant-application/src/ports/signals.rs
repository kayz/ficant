use async_trait::async_trait;
use ficant_domain::primitives::Ulid;
use ficant_domain::research::SignalSet;
use ficant_domain::{ContentAddressed, DomainErrorCode};
use ficant_runtime::FormalOutputEvidence;

use super::blob_store::VerifiedBlobRef;
use super::fingerprint::{FingerprintBuilder, signal_bytes};
use super::{
    AccessScope, ApplicationResult, IdempotencyKey, IntegrityEventSink, OperationFingerprint,
    SafeTraceContext,
};
use crate::map_domain_error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishSignalSet {
    signal_set: SignalSet,
    verified_blob: VerifiedBlobRef,
    formal_evidence: Option<FormalOutputEvidence>,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl PublishSignalSet {
    /// Creates a signal-set publish intent bound to its verified artifact content.
    ///
    /// # Errors
    ///
    /// Returns hash mismatch when the signal and verified artifact disagree.
    pub fn new(
        signal_set: SignalSet,
        verified_blob: VerifiedBlobRef,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        Self::build(signal_set, verified_blob, idempotency_key, None)
    }

    /// Creates a formal signal publication bound to the producing Artifact evidence.
    ///
    /// # Errors
    ///
    /// Returns hash mismatch when the evidence result or owner differs from the signal.
    pub fn new_formal(
        signal_set: SignalSet,
        verified_blob: VerifiedBlobRef,
        idempotency_key: IdempotencyKey,
        formal_evidence: FormalOutputEvidence,
    ) -> ApplicationResult<Self> {
        Self::build(
            signal_set,
            verified_blob,
            idempotency_key,
            Some(formal_evidence),
        )
    }

    fn build(
        signal_set: SignalSet,
        verified_blob: VerifiedBlobRef,
        idempotency_key: IdempotencyKey,
        formal_evidence: Option<FormalOutputEvidence>,
    ) -> ApplicationResult<Self> {
        if signal_set.content_hash() != verified_blob.content_hash() {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        if formal_evidence.as_ref().is_some_and(|evidence| {
            evidence.result_hash() != signal_set.content_hash()
                || evidence.subject().owner() != signal_set.owner()
        }) {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        let mut canonical = FingerprintBuilder::new("publish-signal-set/v1");
        canonical.field(2, &signal_bytes(&signal_set));
        canonical.field(3, verified_blob.content_hash().as_bytes());
        canonical.u64(4, verified_blob.size());
        match &formal_evidence {
            Some(evidence) => {
                canonical.field(5, &[1]);
                canonical.field(6, &evidence.canonical_bytes());
            }
            None => {
                canonical.field(5, &[0]);
            }
        }
        let fingerprint = canonical.finish();
        Ok(Self {
            signal_set,
            verified_blob,
            formal_evidence,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn signal_set(&self) -> &SignalSet {
        &self.signal_set
    }

    #[must_use]
    pub fn verified_blob(&self) -> &VerifiedBlobRef {
        &self.verified_blob
    }

    #[must_use]
    pub fn formal_evidence(&self) -> Option<&FormalOutputEvidence> {
        self.formal_evidence.as_ref()
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

#[async_trait]
pub trait SignalRepository: Send + Sync {
    /// Publishes a signal set with complete immutable lineage.
    ///
    /// # Errors
    ///
    /// Returns an application error for lineage, immutability, or idempotency failure.
    async fn publish(&self, command: PublishSignalSet) -> ApplicationResult<SignalSet>;

    /// Reads immutable `SignalSet` metadata only; it does not prove payload presence or integrity.
    ///
    /// # Errors
    ///
    /// Returns an application error when the query cannot be completed safely.
    async fn get(
        &self,
        scope: &AccessScope,
        signal_set_id: Ulid,
    ) -> ApplicationResult<Option<SignalSet>>;

    /// Reads the producing Artifact's normalized formal evidence for this signal set.
    ///
    /// # Errors
    ///
    /// Returns an application error when the signal/artifact/evidence binding is invalid.
    async fn get_formal_evidence(
        &self,
        _scope: &AccessScope,
        _signal_set_id: Ulid,
    ) -> ApplicationResult<Option<FormalOutputEvidence>> {
        Ok(None)
    }

    /// Reads immutable metadata with a safe context for repository-level integrity events.
    ///
    /// Production repositories should override this method when SQL/payload/lineage validation
    /// happens below the application facade. The default preserves metadata-only fixture ports.
    ///
    /// # Errors
    ///
    /// Returns the same classified read error as [`Self::get`].
    async fn get_integrity_checked(
        &self,
        scope: &AccessScope,
        signal_set_id: Ulid,
        _trace: SafeTraceContext,
        _sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<Option<SignalSet>> {
        self.get(scope, signal_set_id).await
    }
}
