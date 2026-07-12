use async_trait::async_trait;
use ficant_domain::primitives::Ulid;
use ficant_domain::research::SignalSet;
use ficant_domain::{ContentAddressed, DomainErrorCode};

use super::blob_store::VerifiedBlobRef;
use super::fingerprint::{FingerprintBuilder, signal_bytes};
use super::{AccessScope, ApplicationResult, IdempotencyKey, OperationFingerprint};
use crate::map_domain_error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishSignalSet {
    signal_set: SignalSet,
    verified_blob: VerifiedBlobRef,
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
        if signal_set.content_hash() != verified_blob.content_hash() {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        let mut canonical = FingerprintBuilder::new("publish-signal-set/v1");
        canonical.field(2, &signal_bytes(&signal_set));
        canonical.field(3, verified_blob.content_hash().as_bytes());
        canonical.u64(4, verified_blob.size());
        let fingerprint = canonical.finish();
        Ok(Self {
            signal_set,
            verified_blob,
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
}
