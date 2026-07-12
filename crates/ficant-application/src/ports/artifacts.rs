use async_trait::async_trait;
use ficant_domain::primitives::Ulid;
use ficant_domain::research::Artifact;
use ficant_domain::{ContentAddressed, DomainErrorCode};

use super::blob_store::VerifiedBlobRef;
use super::fingerprint::{FingerprintBuilder, artifact_bytes};
use super::{AccessScope, ApplicationResult, IdempotencyKey, OperationFingerprint};
use crate::map_domain_error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishArtifact {
    artifact: Artifact,
    verified_blob: VerifiedBlobRef,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl PublishArtifact {
    /// Creates an artifact publish intent bound to verified hash and size.
    ///
    /// # Errors
    ///
    /// Returns hash mismatch or validation failure when blob metadata disagrees.
    pub fn new(
        artifact: Artifact,
        verified_blob: VerifiedBlobRef,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        if artifact.content_hash() != verified_blob.content_hash() {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        if artifact.blob_size() != verified_blob.size() {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        let mut canonical = FingerprintBuilder::new("publish-artifact/v1");
        canonical.field(2, &artifact_bytes(&artifact));
        canonical.field(3, verified_blob.content_hash().as_bytes());
        canonical.u64(4, verified_blob.size());
        let fingerprint = canonical.finish();
        Ok(Self {
            artifact,
            verified_blob,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
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
pub trait ArtifactRepository: Send + Sync {
    /// Publishes artifact metadata only after blob verification.
    ///
    /// # Errors
    ///
    /// Returns an application error for hash, lineage, immutability, or idempotency failure.
    async fn publish_verified_blob(&self, command: PublishArtifact) -> ApplicationResult<Artifact>;

    /// Reads immutable artifact metadata.
    ///
    /// # Errors
    ///
    /// Returns an application error when metadata cannot be read safely.
    async fn get_metadata(
        &self,
        scope: &AccessScope,
        artifact_id: Ulid,
    ) -> ApplicationResult<Option<Artifact>>;
}
