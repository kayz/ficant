use async_trait::async_trait;
use ficant_domain::primitives::{ContentHash, LineageRef, OwnerRef, Ulid};
use ficant_domain::research::{
    DataHealthThresholdProfile, DataSnapshot, PositionSnapshot, UniverseSnapshot,
};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged};

use super::blob_store::{VerifiedBlobRef, VerifyBlobStage};
use super::fingerprint::{FingerprintBuilder, owner_bytes, snapshot_bytes};
use super::{AccessScope, ApplicationResult, IdempotencyKey, OperationFingerprint};
use crate::map_domain_error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotValue {
    Data(DataSnapshot),
    DataHealthThresholdProfile(DataHealthThresholdProfile),
    Position(PositionSnapshot),
    Universe(UniverseSnapshot),
}

impl SnapshotValue {
    #[must_use]
    pub fn id(&self) -> &Ulid {
        match self {
            Self::Data(value) => value.id(),
            Self::DataHealthThresholdProfile(value) => value.id(),
            Self::Position(value) => value.id(),
            Self::Universe(value) => value.id(),
        }
    }

    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        match self {
            Self::Data(value) => value.content_hash(),
            Self::DataHealthThresholdProfile(value) => value.content_hash(),
            Self::Position(value) => value.content_hash(),
            Self::Universe(value) => value.content_hash(),
        }
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        match self {
            Self::Data(value) => value.owner(),
            Self::DataHealthThresholdProfile(value) => value.owner(),
            Self::Position(value) => value.owner(),
            Self::Universe(value) => value.owner(),
        }
    }

    #[must_use]
    pub fn lineage(&self) -> &[LineageRef] {
        match self {
            Self::Data(value) => value.lineage(),
            Self::DataHealthThresholdProfile(value) => value.lineage(),
            Self::Position(value) => value.lineage(),
            Self::Universe(value) => value.lineage(),
        }
    }
}

impl From<DataSnapshot> for SnapshotValue {
    fn from(value: DataSnapshot) -> Self {
        Self::Data(value)
    }
}

impl From<DataHealthThresholdProfile> for SnapshotValue {
    fn from(value: DataHealthThresholdProfile) -> Self {
        Self::DataHealthThresholdProfile(value)
    }
}

impl From<PositionSnapshot> for SnapshotValue {
    fn from(value: PositionSnapshot) -> Self {
        Self::Position(value)
    }
}

impl From<UniverseSnapshot> for SnapshotValue {
    fn from(value: UniverseSnapshot) -> Self {
        Self::Universe(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotBlobRole {
    DataParquet,
    DataManifest,
    DataHealthThresholdProfilePayload,
    PositionPayload,
    UniverseMembersManifest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotProofKind {
    Data,
    DataHealthThresholdProfile,
    Position,
    Universe,
}

impl SnapshotBlobRole {
    fn code(self) -> u8 {
        match self {
            Self::DataParquet => 1,
            Self::DataManifest => 2,
            Self::DataHealthThresholdProfilePayload => 5,
            Self::PositionPayload => 4,
            Self::UniverseMembersManifest => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedSnapshotBlob {
    role: SnapshotBlobRole,
    verification: VerifyBlobStage,
}

impl StagedSnapshotBlob {
    #[must_use]
    pub fn new(role: SnapshotBlobRole, verification: VerifyBlobStage) -> Self {
        Self { role, verification }
    }

    #[must_use]
    pub fn role(&self) -> SnapshotBlobRole {
        self.role
    }

    #[must_use]
    pub fn verification(&self) -> &VerifyBlobStage {
        &self.verification
    }
}

/// An opaque stage proof has an exact shape: Data requires two roles and Universe exactly one.
///
/// ```compile_fail
/// use ficant_application::ports::{StagedSnapshotProof, StagedSnapshotBlob};
/// let _ = StagedSnapshotProof { inner: panic!() };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedSnapshotProof {
    inner: StagedSnapshotProofInner,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StagedSnapshotProofInner {
    Data {
        parquet: StagedSnapshotBlob,
        manifest: Box<StagedSnapshotBlob>,
    },
    Universe {
        members_manifest: StagedSnapshotBlob,
    },
    DataHealthThresholdProfile {
        payload: StagedSnapshotBlob,
    },
    Position {
        payload: StagedSnapshotBlob,
    },
}

pub(crate) enum StagedSnapshotProofParts {
    Data {
        parquet: StagedSnapshotBlob,
        manifest: Box<StagedSnapshotBlob>,
    },
    Universe {
        members_manifest: StagedSnapshotBlob,
    },
    DataHealthThresholdProfile {
        payload: StagedSnapshotBlob,
    },
    Position {
        payload: StagedSnapshotBlob,
    },
}

impl StagedSnapshotProof {
    /// Creates a complete `DataSnapshot` stage proof with distinct role-bound candidates.
    ///
    /// # Errors
    ///
    /// Returns validation failure for wrong roles or a reused staging identity.
    pub fn data(
        parquet: StagedSnapshotBlob,
        manifest: StagedSnapshotBlob,
    ) -> ApplicationResult<Self> {
        let proof = Self {
            inner: StagedSnapshotProofInner::Data {
                parquet,
                manifest: Box::new(manifest),
            },
        };
        proof.validate_shape()?;
        Ok(proof)
    }

    /// Creates the sole members-Manifest proof allowed for a `UniverseSnapshot`.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the candidate has another role.
    pub fn universe(members_manifest: StagedSnapshotBlob) -> ApplicationResult<Self> {
        let proof = Self {
            inner: StagedSnapshotProofInner::Universe { members_manifest },
        };
        proof.validate_shape()?;
        Ok(proof)
    }

    /// Creates the sole staged proof allowed for a `PositionSnapshot` payload.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless the payload has the `PositionPayload` role.
    pub fn position(payload: StagedSnapshotBlob) -> ApplicationResult<Self> {
        let proof = Self {
            inner: StagedSnapshotProofInner::Position { payload },
        };
        proof.validate_shape()?;
        Ok(proof)
    }

    /// Creates the sole staged proof allowed for a platform data-health profile payload.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless the payload has the dedicated profile role.
    pub fn data_health_threshold_profile(payload: StagedSnapshotBlob) -> ApplicationResult<Self> {
        let proof = Self {
            inner: StagedSnapshotProofInner::DataHealthThresholdProfile { payload },
        };
        proof.validate_shape()?;
        Ok(proof)
    }

    #[must_use]
    pub fn kind(&self) -> SnapshotProofKind {
        match self.inner {
            StagedSnapshotProofInner::Data { .. } => SnapshotProofKind::Data,
            StagedSnapshotProofInner::DataHealthThresholdProfile { .. } => {
                SnapshotProofKind::DataHealthThresholdProfile
            }
            StagedSnapshotProofInner::Universe { .. } => SnapshotProofKind::Universe,
            StagedSnapshotProofInner::Position { .. } => SnapshotProofKind::Position,
        }
    }

    #[must_use]
    pub fn get(&self, role: SnapshotBlobRole) -> Option<&StagedSnapshotBlob> {
        self.blobs().find(|blob| blob.role == role)
    }

    pub fn blobs(&self) -> impl Iterator<Item = &StagedSnapshotBlob> {
        let blobs = match &self.inner {
            StagedSnapshotProofInner::Data { parquet, manifest } => {
                [Some(parquet), Some(manifest.as_ref())]
            }
            StagedSnapshotProofInner::Universe { members_manifest } => {
                [Some(members_manifest), None]
            }
            StagedSnapshotProofInner::DataHealthThresholdProfile { payload }
            | StagedSnapshotProofInner::Position { payload } => [Some(payload), None],
        };
        blobs.into_iter().flatten()
    }

    pub(crate) fn validate_for(&self, snapshot: &SnapshotValue) -> ApplicationResult<()> {
        self.validate_shape()?;
        match (snapshot, &self.inner) {
            (SnapshotValue::Data(value), StagedSnapshotProofInner::Data { parquet, manifest }) => {
                validate_staged_blob(parquet, value.owner(), value.content_hash())?;
                validate_staged_blob(manifest, value.owner(), value.manifest_hash())?;
                if parquet.verification.scope() != manifest.verification.scope() {
                    return Err(forbidden());
                }
                Ok(())
            }
            (
                SnapshotValue::Universe(value),
                StagedSnapshotProofInner::Universe { members_manifest },
            ) => validate_staged_blob(members_manifest, value.owner(), value.content_hash()),
            (SnapshotValue::Position(value), StagedSnapshotProofInner::Position { payload }) => {
                validate_staged_blob(payload, value.owner(), value.content_hash())
            }
            (
                SnapshotValue::DataHealthThresholdProfile(value),
                StagedSnapshotProofInner::DataHealthThresholdProfile { payload },
            ) => validate_staged_blob(payload, value.owner(), value.content_hash()),
            _ => Err(map_domain_error(DomainErrorCode::BrokenLineage)),
        }
    }

    pub(crate) fn all_scopes_match(&self, expected: &AccessScope) -> bool {
        match &self.inner {
            StagedSnapshotProofInner::Data { parquet, manifest } => {
                parquet.verification.scope() == expected
                    && manifest.verification.scope() == expected
            }
            StagedSnapshotProofInner::Universe { members_manifest } => {
                members_manifest.verification.scope() == expected
            }
            StagedSnapshotProofInner::DataHealthThresholdProfile { payload }
            | StagedSnapshotProofInner::Position { payload } => {
                payload.verification.scope() == expected
            }
        }
    }

    fn validate_shape(&self) -> ApplicationResult<()> {
        match &self.inner {
            StagedSnapshotProofInner::Data { parquet, manifest } => {
                if parquet.role != SnapshotBlobRole::DataParquet
                    || manifest.role != SnapshotBlobRole::DataManifest
                    || parquet.verification.staged().id() == manifest.verification.staged().id()
                    || parquet.verification.expected_hash() == manifest.verification.expected_hash()
                {
                    return Err(map_domain_error(DomainErrorCode::InvalidValue));
                }
            }
            StagedSnapshotProofInner::Universe { members_manifest } => {
                if members_manifest.role != SnapshotBlobRole::UniverseMembersManifest {
                    return Err(map_domain_error(DomainErrorCode::InvalidValue));
                }
            }
            StagedSnapshotProofInner::Position { payload } => {
                if payload.role != SnapshotBlobRole::PositionPayload {
                    return Err(map_domain_error(DomainErrorCode::InvalidValue));
                }
            }
            StagedSnapshotProofInner::DataHealthThresholdProfile { payload } => {
                if payload.role != SnapshotBlobRole::DataHealthThresholdProfilePayload {
                    return Err(map_domain_error(DomainErrorCode::InvalidValue));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn into_parts(self) -> StagedSnapshotProofParts {
        match self.inner {
            StagedSnapshotProofInner::Data { parquet, manifest } => {
                StagedSnapshotProofParts::Data { parquet, manifest }
            }
            StagedSnapshotProofInner::Universe { members_manifest } => {
                StagedSnapshotProofParts::Universe { members_manifest }
            }
            StagedSnapshotProofInner::Position { payload } => {
                StagedSnapshotProofParts::Position { payload }
            }
            StagedSnapshotProofInner::DataHealthThresholdProfile { payload } => {
                StagedSnapshotProofParts::DataHealthThresholdProfile { payload }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedSnapshotBlob {
    role: SnapshotBlobRole,
    scope: AccessScope,
    owner: OwnerRef,
    verified_blob: VerifiedBlobRef,
}

impl VerifiedSnapshotBlob {
    /// Binds a promoted immutable blob back to the exact staged role and authority.
    ///
    /// # Errors
    ///
    /// Returns hash or size mismatch when promotion evidence differs from its stage command.
    pub fn from_staged(
        staged: StagedSnapshotBlob,
        verified_blob: VerifiedBlobRef,
    ) -> ApplicationResult<Self> {
        let StagedSnapshotBlob { role, verification } = staged;
        if verification.expected_hash() != verified_blob.content_hash() {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        if verification.expected_size() != verified_blob.size() {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        Ok(Self {
            role,
            scope: verification.scope().clone(),
            owner: verification.staged().owner().clone(),
            verified_blob,
        })
    }

    #[must_use]
    pub fn role(&self) -> SnapshotBlobRole {
        self.role
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
    pub fn verified_blob(&self) -> &VerifiedBlobRef {
        &self.verified_blob
    }
}

/// Opaque durable proof shape rejects missing or extra verified references at compile time.
///
/// ```compile_fail
/// use ficant_application::ports::VerifiedSnapshotProof;
/// let _ = VerifiedSnapshotProof { inner: panic!() };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedSnapshotProof {
    inner: VerifiedSnapshotProofInner,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum VerifiedSnapshotProofInner {
    Data {
        parquet: VerifiedSnapshotBlob,
        manifest: VerifiedSnapshotBlob,
    },
    Universe {
        members_manifest: VerifiedSnapshotBlob,
    },
    DataHealthThresholdProfile {
        payload: VerifiedSnapshotBlob,
    },
    Position {
        payload: VerifiedSnapshotBlob,
    },
}

impl VerifiedSnapshotProof {
    /// Creates a complete durable `DataSnapshot` proof.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless both exact roles are present.
    pub fn data(
        parquet: VerifiedSnapshotBlob,
        manifest: VerifiedSnapshotBlob,
    ) -> ApplicationResult<Self> {
        let proof = Self {
            inner: VerifiedSnapshotProofInner::Data { parquet, manifest },
        };
        proof.validate_shape()?;
        Ok(proof)
    }

    /// Creates the sole durable proof allowed for a `UniverseSnapshot`.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless the members-Manifest role is present.
    pub fn universe(members_manifest: VerifiedSnapshotBlob) -> ApplicationResult<Self> {
        let proof = Self {
            inner: VerifiedSnapshotProofInner::Universe { members_manifest },
        };
        proof.validate_shape()?;
        Ok(proof)
    }

    /// Creates the sole durable proof allowed for a `PositionSnapshot` payload.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless the payload has the `PositionPayload` role.
    pub fn position(payload: VerifiedSnapshotBlob) -> ApplicationResult<Self> {
        let proof = Self {
            inner: VerifiedSnapshotProofInner::Position { payload },
        };
        proof.validate_shape()?;
        Ok(proof)
    }

    /// Creates the sole durable proof allowed for a platform data-health profile payload.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless the payload has the dedicated profile role.
    pub fn data_health_threshold_profile(payload: VerifiedSnapshotBlob) -> ApplicationResult<Self> {
        let proof = Self {
            inner: VerifiedSnapshotProofInner::DataHealthThresholdProfile { payload },
        };
        proof.validate_shape()?;
        Ok(proof)
    }

    #[must_use]
    pub fn kind(&self) -> SnapshotProofKind {
        match self.inner {
            VerifiedSnapshotProofInner::Data { .. } => SnapshotProofKind::Data,
            VerifiedSnapshotProofInner::DataHealthThresholdProfile { .. } => {
                SnapshotProofKind::DataHealthThresholdProfile
            }
            VerifiedSnapshotProofInner::Universe { .. } => SnapshotProofKind::Universe,
            VerifiedSnapshotProofInner::Position { .. } => SnapshotProofKind::Position,
        }
    }

    #[must_use]
    pub fn get(&self, role: SnapshotBlobRole) -> Option<&VerifiedSnapshotBlob> {
        self.blobs().find(|blob| blob.role == role)
    }

    pub fn blobs(&self) -> impl Iterator<Item = &VerifiedSnapshotBlob> {
        let blobs = match &self.inner {
            VerifiedSnapshotProofInner::Data { parquet, manifest } => {
                [Some(parquet), Some(manifest)]
            }
            VerifiedSnapshotProofInner::Universe { members_manifest } => {
                [Some(members_manifest), None]
            }
            VerifiedSnapshotProofInner::DataHealthThresholdProfile { payload }
            | VerifiedSnapshotProofInner::Position { payload } => [Some(payload), None],
        };
        blobs.into_iter().flatten()
    }

    fn validate_for(&self, snapshot: &SnapshotValue) -> ApplicationResult<()> {
        self.validate_shape()?;
        match (snapshot, &self.inner) {
            (
                SnapshotValue::Data(value),
                VerifiedSnapshotProofInner::Data { parquet, manifest },
            ) => {
                validate_verified_snapshot_blob(parquet, value.owner(), value.content_hash())?;
                validate_verified_snapshot_blob(manifest, value.owner(), value.manifest_hash())?;
                if parquet.scope != manifest.scope {
                    return Err(forbidden());
                }
                Ok(())
            }
            (
                SnapshotValue::Universe(value),
                VerifiedSnapshotProofInner::Universe { members_manifest },
            ) => validate_verified_snapshot_blob(
                members_manifest,
                value.owner(),
                value.content_hash(),
            ),
            (SnapshotValue::Position(value), VerifiedSnapshotProofInner::Position { payload }) => {
                validate_verified_snapshot_blob(payload, value.owner(), value.content_hash())
            }
            (
                SnapshotValue::DataHealthThresholdProfile(value),
                VerifiedSnapshotProofInner::DataHealthThresholdProfile { payload },
            ) => validate_verified_snapshot_blob(payload, value.owner(), value.content_hash()),
            _ => Err(map_domain_error(DomainErrorCode::BrokenLineage)),
        }
    }

    fn primary(&self) -> &VerifiedSnapshotBlob {
        match &self.inner {
            VerifiedSnapshotProofInner::Data { parquet, .. } => parquet,
            VerifiedSnapshotProofInner::Universe { members_manifest } => members_manifest,
            VerifiedSnapshotProofInner::DataHealthThresholdProfile { payload }
            | VerifiedSnapshotProofInner::Position { payload } => payload,
        }
    }

    fn validate_shape(&self) -> ApplicationResult<()> {
        match &self.inner {
            VerifiedSnapshotProofInner::Data { parquet, manifest } => {
                if parquet.role != SnapshotBlobRole::DataParquet
                    || manifest.role != SnapshotBlobRole::DataManifest
                    || parquet.verified_blob.content_hash() == manifest.verified_blob.content_hash()
                {
                    return Err(map_domain_error(DomainErrorCode::InvalidValue));
                }
            }
            VerifiedSnapshotProofInner::Universe { members_manifest } => {
                if members_manifest.role != SnapshotBlobRole::UniverseMembersManifest {
                    return Err(map_domain_error(DomainErrorCode::InvalidValue));
                }
            }
            VerifiedSnapshotProofInner::Position { payload } => {
                if payload.role != SnapshotBlobRole::PositionPayload {
                    return Err(map_domain_error(DomainErrorCode::InvalidValue));
                }
            }
            VerifiedSnapshotProofInner::DataHealthThresholdProfile { payload } => {
                if payload.role != SnapshotBlobRole::DataHealthThresholdProfilePayload {
                    return Err(map_domain_error(DomainErrorCode::InvalidValue));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishSnapshot {
    snapshot: SnapshotValue,
    proof: VerifiedSnapshotProof,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl PublishSnapshot {
    /// Creates a publish intent bound to a server-verified blob.
    ///
    /// # Errors
    ///
    /// Returns hash mismatch when the verified blob does not own snapshot content.
    pub fn new(
        snapshot: SnapshotValue,
        proof: VerifiedSnapshotProof,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        proof.validate_for(&snapshot)?;
        let mut canonical = FingerprintBuilder::new("publish-snapshot/v2");
        canonical.field(2, &snapshot_bytes(&snapshot));
        canonical.field(
            3,
            proof
                .primary()
                .scope
                .fingerprint()
                .content_hash()
                .as_bytes(),
        );
        canonical.field(4, &owner_bytes(snapshot.owner()));
        canonical.field(5, idempotency_key.as_str().as_bytes());
        match &proof.inner {
            VerifiedSnapshotProofInner::Data { parquet, manifest } => {
                canonical.field(6, b"data");
                append_snapshot_blob_fingerprint(&mut canonical, 10, parquet);
                append_snapshot_blob_fingerprint(&mut canonical, 20, manifest);
            }
            VerifiedSnapshotProofInner::Universe { members_manifest } => {
                canonical.field(6, b"universe");
                append_snapshot_blob_fingerprint(&mut canonical, 30, members_manifest);
            }
            VerifiedSnapshotProofInner::Position { payload } => {
                canonical.field(6, b"position");
                append_snapshot_blob_fingerprint(&mut canonical, 40, payload);
            }
            VerifiedSnapshotProofInner::DataHealthThresholdProfile { payload } => {
                canonical.field(6, b"data-health-threshold-profile");
                append_snapshot_blob_fingerprint(&mut canonical, 50, payload);
            }
        }
        let fingerprint = canonical.finish();
        Ok(Self {
            snapshot,
            proof,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> &SnapshotValue {
        &self.snapshot
    }

    #[must_use]
    pub fn proof(&self) -> &VerifiedSnapshotProof {
        &self.proof
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

fn validate_staged_blob(
    blob: &StagedSnapshotBlob,
    expected_owner: &OwnerRef,
    expected_hash: &ContentHash,
) -> ApplicationResult<()> {
    let scope = blob.verification.scope();
    if scope.tenant_id() != expected_owner.tenant_id() {
        return Err(forbidden());
    }
    if blob.verification.staged().owner() != expected_owner {
        return Err(map_domain_error(DomainErrorCode::BrokenLineage));
    }
    if blob.verification.expected_hash() != expected_hash {
        return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
    }
    Ok(())
}

fn validate_verified_snapshot_blob(
    blob: &VerifiedSnapshotBlob,
    expected_owner: &OwnerRef,
    expected_hash: &ContentHash,
) -> ApplicationResult<()> {
    if blob.scope.tenant_id() != expected_owner.tenant_id() {
        return Err(forbidden());
    }
    if &blob.owner != expected_owner {
        return Err(map_domain_error(DomainErrorCode::BrokenLineage));
    }
    if blob.verified_blob.content_hash() != expected_hash {
        return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
    }
    Ok(())
}

fn append_snapshot_blob_fingerprint(
    canonical: &mut FingerprintBuilder,
    base_tag: u8,
    blob: &VerifiedSnapshotBlob,
) {
    canonical.field(base_tag, &[blob.role.code()]);
    canonical.field(base_tag + 1, blob.verified_blob.content_hash().as_bytes());
    canonical.u64(base_tag + 2, blob.verified_blob.size());
    canonical.field(base_tag + 3, &owner_bytes(&blob.owner));
}

fn forbidden() -> crate::ApplicationError {
    crate::ApplicationError::new(crate::ApplicationErrorCategory::Forbidden, false)
}

#[async_trait]
pub trait SnapshotRepository: Send + Sync {
    /// Publishes snapshot metadata for a verified immutable blob and lineage.
    ///
    /// # Errors
    ///
    /// Returns an application error when verification or lineage intent fails.
    async fn publish_verified_manifest(
        &self,
        command: PublishSnapshot,
    ) -> ApplicationResult<SnapshotValue>;

    /// Reads immutable snapshot metadata only; it does not prove required blob presence.
    ///
    /// # Errors
    ///
    /// Returns an application error when the query cannot be completed safely.
    async fn get_by_id(
        &self,
        scope: &AccessScope,
        snapshot_id: Ulid,
    ) -> ApplicationResult<Option<SnapshotValue>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApplicationErrorCategory;
    use crate::ports::StagedBlobRef;

    #[test]
    fn internal_noncanonical_proofs_are_rejected_defensively() {
        let staged = StagedSnapshotProof {
            inner: StagedSnapshotProofInner::Data {
                parquet: staged_blob(SnapshotBlobRole::DataManifest, 'V', 12),
                manifest: Box::new(staged_blob(SnapshotBlobRole::DataParquet, 'J', 11)),
            },
        };
        assert_eq!(
            staged.validate_shape().unwrap_err().category(),
            ApplicationErrorCategory::ValidationFailed
        );

        let durable = VerifiedSnapshotProof {
            inner: VerifiedSnapshotProofInner::Data {
                parquet: verified_blob(SnapshotBlobRole::DataManifest, 'V', 12),
                manifest: verified_blob(SnapshotBlobRole::DataParquet, 'J', 11),
            },
        };
        assert_eq!(
            durable.validate_shape().unwrap_err().category(),
            ApplicationErrorCategory::ValidationFailed
        );
    }

    fn staged_blob(role: SnapshotBlobRole, suffix: char, hash_byte: u8) -> StagedSnapshotBlob {
        StagedSnapshotBlob::new(
            role,
            VerifyBlobStage::new(
                scope(),
                StagedBlobRef::new(id(suffix), owner()),
                ContentHash::from_bytes(&[hash_byte; 32]).unwrap(),
                7,
            )
            .unwrap(),
        )
    }

    fn verified_blob(role: SnapshotBlobRole, suffix: char, hash_byte: u8) -> VerifiedSnapshotBlob {
        VerifiedSnapshotBlob::from_staged(
            staged_blob(role, suffix, hash_byte),
            VerifiedBlobRef::new(ContentHash::from_bytes(&[hash_byte; 32]).unwrap(), 7).unwrap(),
        )
        .unwrap()
    }

    fn id(suffix: char) -> Ulid {
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
    }

    fn owner() -> OwnerRef {
        OwnerRef::new(id('T'), id('Y'))
    }

    fn scope() -> AccessScope {
        AccessScope::new(id('T'), id('A'), vec![id('Y')]).unwrap()
    }
}
