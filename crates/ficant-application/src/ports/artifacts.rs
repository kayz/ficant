use async_trait::async_trait;
use ficant_domain::primitives::Ulid;
use ficant_domain::research::Artifact;
use ficant_domain::{ContentAddressed, DomainErrorCode};
use ficant_runtime::FormalOutputEvidence;

use super::blob_store::VerifiedBlobRef;
use super::fingerprint::{FingerprintBuilder, artifact_bytes};
use super::{
    AccessScope, ApplicationResult, IdempotencyKey, IntegrityEventSink, OperationFingerprint,
    SafeTraceContext,
};
use crate::map_domain_error;

pub const ARTIFACT_READ_SCOPE: &str = "artifacts:read";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishArtifact {
    artifact: Artifact,
    verified_blob: VerifiedBlobRef,
    formal_evidence: Option<FormalOutputEvidence>,
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
        Self::build(artifact, verified_blob, idempotency_key, None)
    }

    /// Creates a formal artifact publish intent with exact result evidence.
    ///
    /// # Errors
    ///
    /// Returns hash mismatch or lineage failure when the evidence is not bound to the artifact.
    pub fn new_formal(
        artifact: Artifact,
        verified_blob: VerifiedBlobRef,
        idempotency_key: IdempotencyKey,
        formal_evidence: FormalOutputEvidence,
    ) -> ApplicationResult<Self> {
        Self::build(
            artifact,
            verified_blob,
            idempotency_key,
            Some(formal_evidence),
        )
    }

    fn build(
        artifact: Artifact,
        verified_blob: VerifiedBlobRef,
        idempotency_key: IdempotencyKey,
        formal_evidence: Option<FormalOutputEvidence>,
    ) -> ApplicationResult<Self> {
        if artifact.content_hash() != verified_blob.content_hash() {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        if artifact.blob_size() != verified_blob.size() {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        if formal_evidence.as_ref().is_some_and(|evidence| {
            evidence.result_hash() != artifact.content_hash()
                || evidence.subject().owner() != artifact.owner()
        }) {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        let mut canonical = FingerprintBuilder::new("publish-artifact/v1");
        canonical.field(2, &artifact_bytes(&artifact));
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
            artifact,
            verified_blob,
            formal_evidence,
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

    /// Reads the normalized formal evidence associated with an immutable Artifact.
    ///
    /// # Errors
    ///
    /// Returns an application error when the evidence row cannot be verified safely.
    async fn get_formal_evidence(
        &self,
        _scope: &AccessScope,
        _artifact_id: Ulid,
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
    /// Returns the same classified read error as [`Self::get_metadata`].
    async fn get_integrity_checked_metadata(
        &self,
        scope: &AccessScope,
        artifact_id: Ulid,
        _trace: SafeTraceContext,
        _sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<Option<Artifact>> {
        self.get_metadata(scope, artifact_id).await
    }
}
