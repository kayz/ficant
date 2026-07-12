use async_trait::async_trait;
use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid};

use super::fingerprint::{FingerprintBuilder, owner_bytes};
use super::{AccessScope, ApplicationResult, IdempotencyKey, OperationFingerprint};
use crate::map_domain_error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedBlobRef {
    staging_id: Ulid,
    owner: OwnerRef,
}

impl StagedBlobRef {
    #[must_use]
    pub fn new(staging_id: Ulid, owner: OwnerRef) -> Self {
        Self { staging_id, owner }
    }

    #[must_use]
    pub fn id(&self) -> &Ulid {
        &self.staging_id
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    /// Checks that this staging capability belongs to the supplied access scope.
    ///
    /// # Errors
    ///
    /// Returns forbidden when the stage owner is outside the tenant/owner scope.
    pub fn authorize(&self, scope: &AccessScope) -> ApplicationResult<()> {
        scope.authorize(&self.owner)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedBlobRef {
    content_hash: ContentHash,
    size: u64,
}

impl VerifiedBlobRef {
    /// Creates a nonempty verified immutable blob reference.
    ///
    /// # Errors
    ///
    /// Returns validation failure when size is zero.
    pub fn new(content_hash: ContentHash, size: u64) -> ApplicationResult<Self> {
        if size == 0 {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        Ok(Self { content_hash, size })
    }

    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BeginBlobStage {
    scope: AccessScope,
    owner: OwnerRef,
    expected_size: u64,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl BeginBlobStage {
    /// Creates a validated blob staging command.
    ///
    /// # Errors
    ///
    /// Returns validation failure when expected size is zero.
    pub fn new(
        scope: AccessScope,
        owner: OwnerRef,
        expected_size: u64,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        if expected_size == 0 {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        scope.authorize(&owner)?;
        let mut canonical = FingerprintBuilder::new("begin-blob-stage/v2");
        canonical.field(2, scope.fingerprint().content_hash().as_bytes());
        canonical.field(3, &owner_bytes(&owner));
        canonical.u64(4, expected_size);
        let fingerprint = canonical.finish();
        Ok(Self {
            scope,
            owner,
            expected_size,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        &self.scope
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub fn expected_size(&self) -> u64 {
        self.expected_size
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
pub struct VerifyBlobStage {
    scope: AccessScope,
    staged: StagedBlobRef,
    expected_hash: ContentHash,
    expected_size: u64,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl VerifyBlobStage {
    /// Creates a verified-promote intent bound to one staging identity.
    ///
    /// # Errors
    ///
    /// Returns validation failure when expected size is zero.
    pub fn new(
        scope: AccessScope,
        staged: StagedBlobRef,
        expected_hash: ContentHash,
        expected_size: u64,
    ) -> ApplicationResult<Self> {
        if expected_size == 0 {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        staged.authorize(&scope)?;
        let idempotency_key = IdempotencyKey::new(format!(
            "blob-promote/{}/{}/{}",
            scope.tenant_id(),
            scope.actor_id(),
            staged.id()
        ))?;
        let mut canonical = FingerprintBuilder::new("verify-blob-stage/v2");
        canonical.field(2, scope.fingerprint().content_hash().as_bytes());
        canonical.field(3, staged.id().as_str().as_bytes());
        canonical.field(4, &owner_bytes(staged.owner()));
        canonical.field(5, expected_hash.as_bytes());
        canonical.u64(6, expected_size);
        let fingerprint = canonical.finish();
        Ok(Self {
            scope,
            staged,
            expected_hash,
            expected_size,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        &self.scope
    }

    #[must_use]
    pub fn staged(&self) -> &StagedBlobRef {
        &self.staged
    }

    #[must_use]
    pub fn expected_hash(&self) -> &ContentHash {
        &self.expected_hash
    }

    #[must_use]
    pub fn expected_size(&self) -> u64 {
        self.expected_size
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
pub trait BlobStore: Send + Sync {
    /// Begins a private staging upload without exposing storage keys.
    ///
    /// # Errors
    ///
    /// Returns an application error when staging cannot begin safely.
    async fn begin_stage(&self, command: BeginBlobStage) -> ApplicationResult<StagedBlobRef>;

    /// Appends one ordered byte chunk to a staging upload.
    ///
    /// # Errors
    ///
    /// Returns an application error when the staging reference or chunk is invalid.
    async fn append_chunk(
        &self,
        scope: &AccessScope,
        staged: &StagedBlobRef,
        chunk: Vec<u8>,
    ) -> ApplicationResult<()>;

    /// Recomputes hash and size, then promotes content to immutable storage.
    ///
    /// # Errors
    ///
    /// Returns an application error on hash, size, or storage failure.
    async fn verify_and_promote(
        &self,
        command: VerifyBlobStage,
    ) -> ApplicationResult<VerifiedBlobRef>;

    /// Discards an unverified staging upload.
    ///
    /// # Errors
    ///
    /// Returns an application error when the staging upload cannot be discarded safely.
    async fn discard_stage(
        &self,
        scope: &AccessScope,
        staged: &StagedBlobRef,
    ) -> ApplicationResult<()>;
}
