use ficant_domain::ContentAddressed;
use ficant_domain::primitives::ContentHash;
use ficant_domain::research::DataSnapshot;

use crate::ports::{
    AccessScope, BeginBlobStage, BlobStore, IdempotencyKey, PublishSnapshot, SnapshotBlobRole,
    SnapshotRepository, SnapshotValue, StagedSnapshotBlob, StagedSnapshotProof,
    VerifiedSnapshotBlob, VerifiedSnapshotProof, VerifyBlobStage,
};
use crate::{ApplicationError, ApplicationErrorCategory};

#[derive(Clone, Debug)]
pub struct DataSnapshotPayloads {
    snapshot: DataSnapshot,
    parquet: Vec<u8>,
    manifest: Vec<u8>,
    idempotency_key: IdempotencyKey,
}

impl DataSnapshotPayloads {
    /// Binds exact Parquet and Manifest bytes to their already constructed domain snapshot.
    ///
    /// # Errors
    ///
    /// Returns validation failure before I/O for empty payloads or a hash mismatch.
    pub fn new(
        snapshot: DataSnapshot,
        parquet: Vec<u8>,
        manifest: Vec<u8>,
        idempotency_key: IdempotencyKey,
    ) -> Result<Self, ApplicationError> {
        require_payload(&parquet, snapshot.content_hash())?;
        require_payload(&manifest, snapshot.manifest_hash())?;
        if snapshot.content_hash() == snapshot.manifest_hash() {
            return Err(validation_error());
        }
        Ok(Self {
            snapshot,
            parquet,
            manifest,
            idempotency_key,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> &DataSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn parquet(&self) -> &[u8] {
        &self.parquet
    }

    #[must_use]
    pub fn manifest(&self) -> &[u8] {
        &self.manifest
    }
}

pub struct PublishDataSnapshot<'a> {
    blob_store: &'a dyn BlobStore,
    snapshots: &'a dyn SnapshotRepository,
}

impl<'a> PublishDataSnapshot<'a> {
    #[must_use]
    pub fn new(blob_store: &'a dyn BlobStore, snapshots: &'a dyn SnapshotRepository) -> Self {
        Self {
            blob_store,
            snapshots,
        }
    }

    /// Stages, verifies, promotes, and publishes both required `DataSnapshot` payloads.
    ///
    /// # Errors
    ///
    /// Returns a classified application error without publishing metadata unless both immutable
    /// payloads have been promoted and bound to the existing two-role snapshot proof.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        payloads: DataSnapshotPayloads,
    ) -> Result<DataSnapshot, ApplicationError> {
        scope.authorize(payloads.snapshot.owner())?;
        require_payload(&payloads.parquet, payloads.snapshot.content_hash())?;
        require_payload(&payloads.manifest, payloads.snapshot.manifest_hash())?;

        let parquet = self
            .stage(
                scope,
                payloads.snapshot.owner(),
                payloads.parquet,
                payloads.idempotency_key.scoped("parquet-stage")?,
                SnapshotBlobRole::DataParquet,
            )
            .await?;
        let manifest = match self
            .stage(
                scope,
                payloads.snapshot.owner(),
                payloads.manifest,
                payloads.idempotency_key.scoped("manifest-stage")?,
                SnapshotBlobRole::DataManifest,
            )
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = self
                    .blob_store
                    .discard_stage(scope, parquet.verification().staged())
                    .await;
                return Err(error);
            }
        };

        let staged = StagedSnapshotProof::data(parquet, manifest)?;
        let verified = self.promote(scope, staged).await?;
        let command = PublishSnapshot::new(
            SnapshotValue::Data(payloads.snapshot),
            verified,
            payloads.idempotency_key.scoped("metadata")?,
        )?;
        match self.snapshots.publish_verified_manifest(command).await? {
            SnapshotValue::Data(snapshot) => Ok(snapshot),
            SnapshotValue::Universe(_) => Err(validation_error()),
        }
    }

    async fn stage(
        &self,
        scope: &AccessScope,
        owner: &ficant_domain::primitives::OwnerRef,
        bytes: Vec<u8>,
        idempotency_key: IdempotencyKey,
        role: SnapshotBlobRole,
    ) -> Result<StagedSnapshotBlob, ApplicationError> {
        let size = u64::try_from(bytes.len()).map_err(|_| validation_error())?;
        let expected_hash = ContentHash::digest(&bytes);
        let staged = self
            .blob_store
            .begin_stage(BeginBlobStage::new(
                scope.clone(),
                owner.clone(),
                size,
                idempotency_key,
            )?)
            .await?;
        if let Err(error) = self.blob_store.append_chunk(scope, &staged, bytes).await {
            let _ = self.blob_store.discard_stage(scope, &staged).await;
            return Err(error);
        }
        Ok(StagedSnapshotBlob::new(
            role,
            VerifyBlobStage::new(scope.clone(), staged, expected_hash, size)?,
        ))
    }

    async fn promote(
        &self,
        scope: &AccessScope,
        staged: StagedSnapshotProof,
    ) -> Result<VerifiedSnapshotProof, ApplicationError> {
        let parquet = staged
            .get(SnapshotBlobRole::DataParquet)
            .cloned()
            .ok_or_else(validation_error)?;
        let manifest = staged
            .get(SnapshotBlobRole::DataManifest)
            .cloned()
            .ok_or_else(validation_error)?;
        let parquet_verified = self
            .blob_store
            .verify_and_promote(parquet.verification().clone())
            .await?;
        let parquet = VerifiedSnapshotBlob::from_staged(parquet, parquet_verified)?;
        let manifest_verified = match self
            .blob_store
            .verify_and_promote(manifest.verification().clone())
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = self
                    .blob_store
                    .discard_stage(scope, manifest.verification().staged())
                    .await;
                return Err(error);
            }
        };
        let manifest = VerifiedSnapshotBlob::from_staged(manifest, manifest_verified)?;
        VerifiedSnapshotProof::data(parquet, manifest)
    }
}

fn require_payload(bytes: &[u8], expected: &ContentHash) -> Result<(), ApplicationError> {
    if bytes.is_empty() {
        return Err(validation_error());
    }
    expected
        .verify(bytes)
        .map_err(|_| ApplicationError::new(ApplicationErrorCategory::HashMismatch, false))
}

fn validation_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
