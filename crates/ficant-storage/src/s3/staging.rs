use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, ApplicationResult, BeginBlobStage, BlobStore, IntegrityEventSink,
    IntegrityFailureReason, RequiredVerifiedBlobRead, StagedBlobRef, VerifiedBlobPayload,
    VerifiedBlobReader, VerifiedBlobRef, VerifiedBlobRole, VerifiedReadResourceKind,
    VerifyBlobStage,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::primitives::{ContentHash, Ulid};
use object_store::aws::AmazonS3Builder;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};
use sqlx::PgPool;
use std::sync::Arc;

use super::content_addressed::content_key;
use crate::postgres::common::{application_error, lock_idempotency, map_sqlx_error};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImmutableObjectBackup {
    key: String,
    content_hash: ContentHash,
    size: u64,
    bytes: Vec<u8>,
}

impl ImmutableObjectBackup {
    /// Creates one recovery object after proving that its immutable key is the SHA-256 of its
    /// exact bytes.
    ///
    /// # Errors
    ///
    /// Returns a hash mismatch when the key, digest, or payload disagree.
    pub fn new(key: impl Into<String>, bytes: Vec<u8>) -> ApplicationResult<Self> {
        let key = key.into();
        let content_hash = ContentHash::digest(&bytes);
        if bytes.is_empty() || key != content_key(&content_hash) {
            return Err(application_error(
                ApplicationErrorCategory::HashMismatch,
                false,
            ));
        }
        let size = u64::try_from(bytes.len()).map_err(|_| validation_error())?;
        Ok(Self {
            key,
            content_hash,
            size,
            bytes,
        })
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }
}

#[derive(Clone)]
pub struct S3BlobStore {
    pub(super) client: Arc<dyn ObjectStore>,
    tracking_pool: PgPool,
}

struct FormalBlobReference {
    owner_id: String,
    content_hash: String,
    declared_size: Option<i64>,
    linkage_valid: bool,
}

type SignalReferenceRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
);

impl S3BlobStore {
    /// Creates a vendor-neutral S3 adapter without exposing credentials.
    ///
    /// # Errors
    ///
    /// Returns a validation error for an invalid endpoint or client configuration.
    pub fn new(
        endpoint: &str,
        bucket: String,
        access_key: &str,
        secret_key: &str,
        tracking_pool: PgPool,
    ) -> ApplicationResult<Self> {
        let client = AmazonS3Builder::new()
            .with_endpoint(endpoint)
            .with_bucket_name(bucket)
            .with_access_key_id(access_key)
            .with_secret_access_key(secret_key)
            .with_region("us-east-1")
            .with_allow_http(endpoint.starts_with("http://"))
            .with_virtual_hosted_style_request(false)
            .build()
            .map_err(|_| validation_error())?;
        Ok(Self {
            client: Arc::new(client),
            tracking_pool,
        })
    }

    /// Reads immutable content and recomputes its SHA-256 before returning it.
    ///
    /// # Errors
    ///
    /// Returns hash mismatch for corrupted content and storage unavailable for I/O failures.
    pub async fn probe_verified(&self, hash: &ContentHash) -> ApplicationResult<Option<Vec<u8>>> {
        let Some(bytes) = self.read_object(&content_key(hash)).await? else {
            return Ok(None);
        };
        hash.verify(&bytes).map_err(map_domain_error)?;
        Ok(Some(bytes))
    }

    /// Enumerates every immutable object and verifies its key, size, and bytes before returning a
    /// stable recovery snapshot. Staging objects are deliberately excluded.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed storage or hash error for an incomplete listing, nested immutable
    /// prefix, missing object, size drift, or key/content disagreement.
    pub async fn list_immutable_objects(&self) -> ApplicationResult<Vec<ImmutableObjectBackup>> {
        let listing = self
            .client
            .list_with_delimiter(Some(&Path::from("immutable")))
            .await
            .map_err(|_| storage_error())?;
        if !listing.common_prefixes.is_empty() {
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        let mut metadata = listing.objects;
        metadata.sort_by(|left, right| left.location.cmp(&right.location));
        let mut objects = Vec::with_capacity(metadata.len());
        for value in metadata {
            let key = value.location.to_string();
            let bytes = self.read_object(&key).await?.ok_or_else(storage_error)?;
            if u64::try_from(bytes.len()).map_err(|_| validation_error())? != value.size {
                return Err(application_error(
                    ApplicationErrorCategory::HashMismatch,
                    false,
                ));
            }
            objects.push(ImmutableObjectBackup::new(key, bytes)?);
        }
        Ok(objects)
    }

    /// Restores one pre-validated immutable recovery object. Existing exact bytes replay; any
    /// existing drift fails closed.
    ///
    /// # Errors
    ///
    /// Returns a storage or hash error when the object cannot be restored exactly.
    pub async fn restore_immutable_object(
        &self,
        object: ImmutableObjectBackup,
    ) -> ApplicationResult<()> {
        let exact = ImmutableObjectBackup::new(object.key.clone(), object.bytes.clone())?;
        match self.read_object(exact.key()).await? {
            Some(existing) if existing == exact.bytes => Ok(()),
            Some(_) => Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            )),
            None => {
                let key = exact.key.clone();
                self.put_object(&key, exact.bytes).await
            }
        }
    }

    #[must_use]
    pub fn immutable_key(hash: &ContentHash) -> String {
        content_key(hash)
    }

    #[must_use]
    pub fn hash_hex(hash: &ContentHash) -> String {
        super::content_addressed::hash_hex(hash)
    }

    async fn read_object(&self, key: &str) -> ApplicationResult<Option<Vec<u8>>> {
        let path = Path::from(key);
        let response = match self.client.get(&path).await {
            Ok(response) => response,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(_) => return Err(storage_error()),
        };
        let bytes = response
            .bytes()
            .await
            .map_err(|_| storage_error())?
            .to_vec();
        Ok(Some(bytes))
    }

    async fn put_object(&self, key: &str, bytes: Vec<u8>) -> ApplicationResult<()> {
        self.client
            .put(&Path::from(key), bytes.into())
            .await
            .map_err(|_| storage_error())?;
        Ok(())
    }

    pub(super) async fn delete_object(&self, key: &str) -> ApplicationResult<()> {
        self.client
            .delete(&Path::from(key))
            .await
            .map_err(|_| storage_error())?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn formal_blob_reference(
        &self,
        request: &RequiredVerifiedBlobRead,
    ) -> ApplicationResult<Option<FormalBlobReference>> {
        match (request.resource_kind(), request.blob_role()) {
            (VerifiedReadResourceKind::Artifact, VerifiedBlobRole::ArtifactPayload) => {
                let row: Option<(String, String, i64)> = sqlx::query_as(
                    "SELECT owner_id::text, content_hash::text, blob_size
                     FROM research.artifacts
                     WHERE tenant_id=$1 AND artifact_id=$2",
                )
                .bind(request.tenant_id().as_str())
                .bind(request.resource_id().as_str())
                .fetch_optional(&self.tracking_pool)
                .await
                .map_err(map_sqlx_error)?;
                Ok(row.map(
                    |(owner_id, content_hash, declared_size)| FormalBlobReference {
                        owner_id,
                        content_hash,
                        declared_size: Some(declared_size),
                        linkage_valid: true,
                    },
                ))
            }
            (VerifiedReadResourceKind::SignalSet, VerifiedBlobRole::SignalPayload) => {
                let row: Option<SignalReferenceRow> = sqlx::query_as(
                    "SELECT signal.owner_id::text, signal.content_hash::text,
                            artifact.owner_id::text, artifact.kind,
                            artifact.content_hash::text,
                            artifact.blob_size
                     FROM research.signal_sets signal
                     LEFT JOIN research.artifacts artifact
                       ON artifact.tenant_id=signal.tenant_id
                      AND artifact.artifact_id=signal.artifact_id
                     WHERE signal.tenant_id=$1 AND signal.signal_set_id=$2",
                )
                .bind(request.tenant_id().as_str())
                .bind(request.resource_id().as_str())
                .fetch_optional(&self.tracking_pool)
                .await
                .map_err(map_sqlx_error)?;
                Ok(row.map(
                    |(
                        owner_id,
                        content_hash,
                        artifact_owner,
                        artifact_kind,
                        artifact_hash,
                        artifact_size,
                    )| {
                        let linkage_valid = artifact_owner.as_deref() == Some(owner_id.as_str())
                            && artifact_kind.as_deref() == Some("SIGNAL_SET")
                            && artifact_hash.as_deref() == Some(content_hash.as_str());
                        FormalBlobReference {
                            owner_id,
                            content_hash,
                            declared_size: artifact_size,
                            linkage_valid,
                        }
                    },
                ))
            }
            (VerifiedReadResourceKind::DataSnapshot, VerifiedBlobRole::DataParquet) => {
                self.snapshot_blob_reference(request, "content_hash").await
            }
            (VerifiedReadResourceKind::DataSnapshot, VerifiedBlobRole::DataManifest) => {
                self.snapshot_blob_reference(request, "manifest_hash").await
            }
            (
                VerifiedReadResourceKind::UniverseSnapshot,
                VerifiedBlobRole::UniverseMembersManifest,
            ) => {
                let row: Option<(String, String)> = sqlx::query_as(
                    "SELECT owner_id::text, content_hash::text
                     FROM research.universe_snapshots
                     WHERE tenant_id=$1 AND universe_snapshot_id=$2",
                )
                .bind(request.tenant_id().as_str())
                .bind(request.resource_id().as_str())
                .fetch_optional(&self.tracking_pool)
                .await
                .map_err(map_sqlx_error)?;
                Ok(row.map(|(owner_id, content_hash)| FormalBlobReference {
                    owner_id,
                    content_hash,
                    declared_size: None,
                    linkage_valid: true,
                }))
            }
            (VerifiedReadResourceKind::CurveSnapshot, VerifiedBlobRole::CurvePoints) => {
                let row: Option<(String, String, i64)> = sqlx::query_as(
                    "SELECT owner_id::text, content_hash::text, blob_size
                     FROM market.curve_snapshots
                     WHERE tenant_id=$1 AND curve_snapshot_id=$2",
                )
                .bind(request.tenant_id().as_str())
                .bind(request.resource_id().as_str())
                .fetch_optional(&self.tracking_pool)
                .await
                .map_err(map_sqlx_error)?;
                Ok(row.map(
                    |(owner_id, content_hash, declared_size)| FormalBlobReference {
                        owner_id,
                        content_hash,
                        declared_size: Some(declared_size),
                        linkage_valid: true,
                    },
                ))
            }
            _ => Err(validation_error()),
        }
    }

    async fn snapshot_blob_reference(
        &self,
        request: &RequiredVerifiedBlobRead,
        hash_column: &str,
    ) -> ApplicationResult<Option<FormalBlobReference>> {
        let query = match hash_column {
            "content_hash" => {
                "SELECT owner_id::text, content_hash::text
                 FROM research.data_snapshots
                 WHERE tenant_id=$1 AND data_snapshot_id=$2"
            }
            "manifest_hash" => {
                "SELECT owner_id::text, manifest_hash::text
                 FROM research.data_snapshots
                 WHERE tenant_id=$1 AND data_snapshot_id=$2"
            }
            _ => return Err(validation_error()),
        };
        let row: Option<(String, String)> = sqlx::query_as(query)
            .bind(request.tenant_id().as_str())
            .bind(request.resource_id().as_str())
            .fetch_optional(&self.tracking_pool)
            .await
            .map_err(map_sqlx_error)?;
        Ok(row.map(|(owner_id, content_hash)| FormalBlobReference {
            owner_id,
            content_hash,
            declared_size: None,
            linkage_valid: true,
        }))
    }
}

#[async_trait]
impl VerifiedBlobReader for S3BlobStore {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<VerifiedBlobPayload> {
        request.scope().authorize(request.owner())?;
        let Some(reference) = self.formal_blob_reference(request).await? else {
            return Err(request
                .fail_integrity(sink, IntegrityFailureReason::Missing)
                .await);
        };
        let expected_hash = super::content_addressed::hash_hex(request.expected_hash());
        if !reference.linkage_valid
            || reference.owner_id != request.owner().owner_id().as_str()
            || reference.content_hash != expected_hash
        {
            return Err(request
                .fail_integrity(sink, IntegrityFailureReason::HashMismatch)
                .await);
        }
        let durable: Option<(String, String, i64)> = sqlx::query_as(
            "SELECT content_hash::text, object_key, blob_size FROM storage.blobs
             WHERE tenant_id = $1 AND content_hash = $2",
        )
        .bind(request.tenant_id().as_str())
        .bind(&reference.content_hash)
        .fetch_optional(&self.tracking_pool)
        .await
        .map_err(map_sqlx_error)?;
        let Some((durable_hash, object_key, durable_size)) = durable else {
            return Err(request
                .fail_integrity(sink, IntegrityFailureReason::Missing)
                .await);
        };
        if durable_hash != expected_hash || object_key != content_key(request.expected_hash()) {
            return Err(request
                .fail_integrity(sink, IntegrityFailureReason::HashMismatch)
                .await);
        }
        if reference
            .declared_size
            .is_some_and(|size| u64::try_from(size).ok() != Some(request.expected_size()))
            || u64::try_from(durable_size).ok() != Some(request.expected_size())
        {
            return Err(request
                .fail_integrity(sink, IntegrityFailureReason::SizeMismatch)
                .await);
        }
        let Some(bytes) = self.read_object(&object_key).await? else {
            return Err(request
                .fail_integrity(sink, IntegrityFailureReason::Missing)
                .await);
        };
        request.verify_bytes(sink, bytes).await
    }
}

#[async_trait]
impl BlobStore for S3BlobStore {
    async fn begin_stage(&self, command: BeginBlobStage) -> ApplicationResult<StagedBlobRef> {
        let mut identity_input = Vec::with_capacity(32 + command.idempotency_key().as_str().len());
        identity_input.extend_from_slice(command.fingerprint().content_hash().as_bytes());
        identity_input.extend_from_slice(command.idempotency_key().as_str().as_bytes());
        let staging_id = ulid_from_hash(&ContentHash::digest(&identity_input))?;
        let key = staging_key(&staging_id);
        let mut transaction = self.tracking_pool.begin().await.map_err(map_sqlx_error)?;
        lock_idempotency(
            &mut transaction,
            command.owner().tenant_id().as_str(),
            "blob-stage:begin:v1",
            command.idempotency_key().as_str(),
            command.fingerprint().content_hash().as_bytes(),
            staging_id.as_str(),
        )
        .await?;
        sqlx::query(
            "INSERT INTO storage.staging_uploads
             (staging_id, tenant_id, owner_id, expected_size, object_key)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (staging_id) DO NOTHING",
        )
        .bind(staging_id.as_str())
        .bind(command.owner().tenant_id().as_str())
        .bind(command.owner().owner_id().as_str())
        .bind(i64::try_from(command.expected_size()).map_err(|_| validation_error())?)
        .bind(&key)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        let persisted: Option<(String, String, String)> = sqlx::query_as(
            "SELECT tenant_id::text, owner_id::text, object_key
             FROM storage.staging_uploads WHERE staging_id = $1
             FOR UPDATE",
        )
        .bind(staging_id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if persisted
            != Some((
                command.owner().tenant_id().as_str().to_owned(),
                command.owner().owner_id().as_str().to_owned(),
                key.clone(),
            ))
        {
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        sqlx::query(
            "UPDATE storage.staging_uploads
             SET updated_at = CURRENT_TIMESTAMP
             WHERE staging_id = $1",
        )
        .bind(staging_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        if self.read_object(&key).await?.is_none() {
            self.put_object(&key, Vec::new()).await?;
        }
        Ok(StagedBlobRef::new(staging_id, command.owner().clone()))
    }

    async fn append_chunk(
        &self,
        scope: &AccessScope,
        staged: &StagedBlobRef,
        chunk: Vec<u8>,
    ) -> ApplicationResult<()> {
        staged.authorize(scope)?;
        if chunk.is_empty() {
            return Err(validation_error());
        }
        let key = staging_key(staged.id());
        let mut bytes = self.read_object(&key).await?.ok_or_else(not_found_error)?;
        bytes.extend_from_slice(&chunk);
        let expected_size: Option<i64> = sqlx::query_scalar(
            "UPDATE storage.staging_uploads
             SET updated_at = CURRENT_TIMESTAMP
             WHERE staging_id = $1 AND tenant_id = $2 AND owner_id = $3
             RETURNING expected_size",
        )
        .bind(staged.id().as_str())
        .bind(staged.owner().tenant_id().as_str())
        .bind(staged.owner().owner_id().as_str())
        .fetch_optional(&self.tracking_pool)
        .await
        .map_err(map_sqlx_error)?;
        let expected_size = expected_size.ok_or_else(not_found_error)?;
        if i64::try_from(bytes.len()).map_err(|_| validation_error())? > expected_size {
            return Err(validation_error());
        }
        self.put_object(&key, bytes).await
    }

    async fn verify_and_promote(
        &self,
        command: VerifyBlobStage,
    ) -> ApplicationResult<VerifiedBlobRef> {
        command.staged().authorize(command.scope())?;
        let tracked_size: Option<i64> = sqlx::query_scalar(
            "UPDATE storage.staging_uploads
             SET updated_at = CURRENT_TIMESTAMP
             WHERE staging_id = $1 AND tenant_id = $2 AND owner_id = $3
             RETURNING expected_size",
        )
        .bind(command.staged().id().as_str())
        .bind(command.staged().owner().tenant_id().as_str())
        .bind(command.staged().owner().owner_id().as_str())
        .fetch_optional(&self.tracking_pool)
        .await
        .map_err(map_sqlx_error)?;
        let tracked_size = tracked_size.ok_or_else(not_found_error)?;
        if u64::try_from(tracked_size).map_err(|_| storage_error())? != command.expected_size() {
            return Err(validation_error());
        }
        let staging_key = staging_key(command.staged().id());
        let bytes = self
            .read_object(&staging_key)
            .await?
            .ok_or_else(not_found_error)?;
        let actual_size = u64::try_from(bytes.len()).map_err(|_| validation_error())?;
        if actual_size != command.expected_size() {
            return Err(validation_error());
        }
        command
            .expected_hash()
            .verify(&bytes)
            .map_err(map_domain_error)?;

        let immutable_key = content_key(command.expected_hash());
        self.register_orphan_candidate(command.expected_hash(), &immutable_key, actual_size)
            .await?;
        match self.read_object(&immutable_key).await? {
            Some(existing) => command
                .expected_hash()
                .verify(&existing)
                .map_err(map_domain_error)?,
            None => self.put_object(&immutable_key, bytes).await?,
        }
        self.delete_object(&staging_key).await?;
        self.forget_staging(command.staged()).await?;
        VerifiedBlobRef::new(command.expected_hash().clone(), actual_size)
    }

    async fn discard_stage(
        &self,
        scope: &AccessScope,
        staged: &StagedBlobRef,
    ) -> ApplicationResult<()> {
        staged.authorize(scope)?;
        self.delete_object(&staging_key(staged.id())).await?;
        self.forget_staging(staged).await
    }
}

impl S3BlobStore {
    async fn register_orphan_candidate(
        &self,
        hash: &ContentHash,
        object_key: &str,
        blob_size: u64,
    ) -> ApplicationResult<()> {
        let hash = Self::hash_hex(hash);
        let blob_size = i64::try_from(blob_size).map_err(|_| validation_error())?;
        let mut transaction = self.tracking_pool.begin().await.map_err(map_sqlx_error)?;
        sqlx::query(
            "INSERT INTO storage.orphan_candidates(content_hash, object_key, blob_size)
             VALUES ($1, $2, $3)
             ON CONFLICT (content_hash) DO NOTHING",
        )
        .bind(&hash)
        .bind(object_key)
        .bind(blob_size)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        let persisted: Option<(String, i64)> = sqlx::query_as(
            "SELECT object_key, blob_size FROM storage.orphan_candidates
             WHERE content_hash = $1 FOR UPDATE",
        )
        .bind(&hash)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if persisted != Some((object_key.to_owned(), blob_size)) {
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        transaction.commit().await.map_err(map_sqlx_error)
    }

    async fn forget_staging(&self, staged: &StagedBlobRef) -> ApplicationResult<()> {
        sqlx::query(
            "DELETE FROM storage.staging_uploads
             WHERE staging_id = $1 AND tenant_id = $2 AND owner_id = $3",
        )
        .bind(staged.id().as_str())
        .bind(staged.owner().tenant_id().as_str())
        .bind(staged.owner().owner_id().as_str())
        .execute(&self.tracking_pool)
        .await
        .map_err(|_| storage_error())?;
        Ok(())
    }
}

fn staging_key(id: &Ulid) -> String {
    format!("staging/{id}")
}

fn ulid_from_hash(hash: &ContentHash) -> ApplicationResult<Ulid> {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let source = &hash.as_bytes()[..16];
    let mut encoded = [b'0'; 26];
    for (group, output) in encoded.iter_mut().enumerate() {
        let mut value = 0_u8;
        for offset in 0..5 {
            value <<= 1;
            let padded_bit = group * 5 + offset;
            if padded_bit >= 2 {
                let source_bit = padded_bit - 2;
                let byte = source[source_bit / 8];
                value |= (byte >> (7 - source_bit % 8)) & 1;
            }
        }
        *output = ALPHABET[usize::from(value)];
    }
    let value = String::from_utf8(encoded.to_vec()).map_err(|_| validation_error())?;
    Ulid::new(value).map_err(map_domain_error)
}

fn validation_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn not_found_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

fn storage_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
}
