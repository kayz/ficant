use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, PublishSnapshot, SnapshotBlobRole, SnapshotProofKind, SnapshotRepository,
    SnapshotValue, SnapshotVerifiedReadMetadata, SnapshotVerifiedReadMetadataRepository,
    VerifiedSnapshotBlob,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::ContentAddressed;

use super::PostgresRepository;
use super::codec::encode_snapshot;
use super::common::{
    IdempotencyOutcome, application_error, insert_lineage, lock_idempotency, map_sqlx_error,
    publish_blob_reference,
};

#[async_trait]
impl SnapshotRepository for PostgresRepository {
    async fn publish_verified_manifest(
        &self,
        command: PublishSnapshot,
    ) -> Result<SnapshotValue, ApplicationError> {
        PostgresRepository::publish_verified_manifest(self, command).await
    }

    async fn get_by_id(
        &self,
        scope: &AccessScope,
        snapshot_id: ficant_domain::primitives::Ulid,
    ) -> Result<Option<SnapshotValue>, ApplicationError> {
        let owners = owner_strings(scope);
        let rows: Vec<(Vec<u8>,)> = sqlx::query_as(
            "SELECT payload FROM research.data_snapshots
             WHERE tenant_id = $1 AND data_snapshot_id = $2
               AND owner_id::text = ANY($3::text[])
             UNION ALL
             SELECT payload FROM research.universe_snapshots
             WHERE tenant_id = $1 AND universe_snapshot_id = $2
               AND owner_id::text = ANY($3::text[])
             UNION ALL
             SELECT payload FROM research.position_snapshots
             WHERE tenant_id = $1 AND snapshot_id = $2
               AND owner_id::text = ANY($3::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(snapshot_id.as_str())
        .bind(&owners)
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        match rows.as_slice() {
            [] => Ok(None),
            [(payload,)] => super::codec::decode_snapshot(payload).map(Some),
            _ => Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            )),
        }
    }
}

#[async_trait]
impl SnapshotVerifiedReadMetadataRepository for PostgresRepository {
    async fn get_verified_read_metadata(
        &self,
        scope: &AccessScope,
        snapshot_id: ficant_domain::primitives::Ulid,
    ) -> Result<Option<SnapshotVerifiedReadMetadata>, ApplicationError> {
        let Some(snapshot) = SnapshotRepository::get_by_id(self, scope, snapshot_id).await? else {
            return Ok(None);
        };
        match snapshot {
            SnapshotValue::Data(value) => {
                let sizes: Option<(i64, i64)> = sqlx::query_as(
                    "SELECT data.blob_size, manifest.blob_size
                     FROM storage.blobs data, storage.blobs manifest
                     WHERE data.tenant_id=$1 AND data.content_hash=$2
                       AND manifest.tenant_id=$1 AND manifest.content_hash=$3",
                )
                .bind(scope.tenant_id().as_str())
                .bind(crate::s3::content_addressed::hash_hex(value.content_hash()))
                .bind(crate::s3::content_addressed::hash_hex(
                    value.manifest_hash(),
                ))
                .fetch_optional(self.pool())
                .await
                .map_err(map_sqlx_error)?;
                let Some((data_size, manifest_size)) = sizes else {
                    return Err(application_error(
                        ApplicationErrorCategory::LineageIncomplete,
                        false,
                    ));
                };
                Ok(Some(SnapshotVerifiedReadMetadata::data(
                    value,
                    u64::try_from(data_size).map_err(|_| invalid())?,
                    u64::try_from(manifest_size).map_err(|_| invalid())?,
                )?))
            }
            SnapshotValue::Universe(value) => {
                let size: Option<i64> = sqlx::query_scalar(
                    "SELECT blob_size FROM storage.blobs WHERE tenant_id=$1 AND content_hash=$2",
                )
                .bind(scope.tenant_id().as_str())
                .bind(crate::s3::content_addressed::hash_hex(value.content_hash()))
                .fetch_optional(self.pool())
                .await
                .map_err(map_sqlx_error)?;
                let Some(size) = size else {
                    return Err(application_error(
                        ApplicationErrorCategory::LineageIncomplete,
                        false,
                    ));
                };
                Ok(Some(SnapshotVerifiedReadMetadata::universe(
                    value,
                    u64::try_from(size).map_err(|_| invalid())?,
                )?))
            }
            SnapshotValue::Position(_) => Err(application_error(
                ApplicationErrorCategory::ValidationFailed,
                false,
            )),
        }
    }
}

impl PostgresRepository {
    /// Persists immutable snapshot metadata only after binding the verified blob reference.
    ///
    /// # Errors
    ///
    /// Returns a classified application error for idempotency, lineage, or storage failure.
    pub async fn publish_verified_manifest(
        &self,
        command: PublishSnapshot,
    ) -> Result<SnapshotValue, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let value = persist_snapshot(&mut transaction, &command).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }
}

pub(crate) async fn persist_snapshot(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &PublishSnapshot,
) -> Result<SnapshotValue, ApplicationError> {
    let snapshot = command.snapshot();
    let tenant_id = snapshot.owner().tenant_id().as_str();
    let snapshot_id = snapshot.id().as_str();
    let fingerprint = command.fingerprint().content_hash().as_bytes();
    validate_proof(snapshot, command)?;
    let outcome = lock_idempotency(
        transaction,
        tenant_id,
        "snapshot:publish:v2",
        command.idempotency_key().as_str(),
        fingerprint,
        snapshot_id,
    )
    .await?;
    for blob in command.proof().blobs() {
        publish_blob_reference(
            transaction,
            tenant_id,
            blob.verified_blob().content_hash(),
            blob.verified_blob().size(),
        )
        .await?;
    }
    if outcome == IdempotencyOutcome::Replay {
        let persisted = load_persisted_snapshot(transaction, snapshot).await?;
        if persisted != *snapshot {
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        return Ok(persisted);
    }
    let payload = encode_snapshot(snapshot);
    insert_snapshot_metadata(transaction, command, &payload).await?;
    insert_lineage(transaction, tenant_id, snapshot_id, snapshot.lineage()).await?;
    Ok(snapshot.clone())
}

async fn insert_snapshot_metadata(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &PublishSnapshot,
    payload: &[u8],
) -> Result<(), ApplicationError> {
    let snapshot = command.snapshot();
    let tenant_id = snapshot.owner().tenant_id().as_str();
    let fingerprint = command.fingerprint().content_hash().as_bytes();
    match snapshot {
        SnapshotValue::Data(value) => {
            sqlx::query(
                    "INSERT INTO research.data_snapshots
                     (tenant_id, data_snapshot_id, owner_id, visible_at, as_of,
                      schema_hash, manifest_hash, content_hash, idempotency_key, fingerprint, payload)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                )
                .bind(tenant_id)
                .bind(value.id().as_str())
                .bind(value.owner().owner_id().as_str())
                .bind(value.visible_at().instant())
                .bind(value.as_of().instant())
                .bind(crate::s3::content_addressed::hash_hex(value.schema_hash()))
                .bind(crate::s3::content_addressed::hash_hex(value.manifest_hash()))
                .bind(crate::s3::content_addressed::hash_hex(value.content_hash()))
                .bind(command.idempotency_key().as_str())
                .bind(fingerprint.as_slice())
                .bind(payload)
                .execute(&mut **transaction)
                .await
                .map_err(map_sqlx_error)?;
        }
        SnapshotValue::Universe(value) => {
            sqlx::query(
                "INSERT INTO research.universe_snapshots
                     (tenant_id, universe_snapshot_id, owner_id, filter_digest, content_hash,
                      idempotency_key, fingerprint, payload)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(tenant_id)
            .bind(value.id().as_str())
            .bind(value.owner().owner_id().as_str())
            .bind(crate::s3::content_addressed::hash_hex(
                value.filter_digest(),
            ))
            .bind(crate::s3::content_addressed::hash_hex(value.content_hash()))
            .bind(command.idempotency_key().as_str())
            .bind(fingerprint.as_slice())
            .bind(payload)
            .execute(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
            for (ordinal, instrument) in value.instrument_versions().iter().enumerate() {
                let ordinal = i32::try_from(ordinal).map_err(|_| {
                    application_error(ApplicationErrorCategory::ValidationFailed, false)
                })?;
                let version = i64::try_from(instrument.version().get()).map_err(|_| {
                    application_error(ApplicationErrorCategory::ValidationFailed, false)
                })?;
                sqlx::query(
                        "INSERT INTO research.universe_members
                         (tenant_id, universe_snapshot_id, ordinal, instrument_id, instrument_version)
                         VALUES ($1, $2, $3, $4, $5)",
                    )
                    .bind(tenant_id)
                    .bind(value.id().as_str())
                    .bind(ordinal)
                    .bind(instrument.id().as_str())
                    .bind(version)
                    .execute(&mut **transaction)
                    .await
                    .map_err(map_sqlx_error)?;
            }
        }
        SnapshotValue::Position(value) => {
            sqlx::query(
                "INSERT INTO research.position_snapshots
                 (tenant_id, snapshot_id, owner_id, subject_id, subject_version, observed_at, visible_at,
                  content_hash, idempotency_key, fingerprint, payload)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
            )
            .bind(tenant_id)
            .bind(value.id().as_str())
            .bind(value.owner().owner_id().as_str())
            .bind(value.subject_ref().id().as_str())
            .bind(i64::try_from(value.subject_ref().version().get()).map_err(|_| invalid())?)
            .bind(value.observed_at().instant())
            .bind(value.visible_at().instant())
            .bind(crate::s3::content_addressed::hash_hex(value.content_hash()))
            .bind(command.idempotency_key().as_str())
            .bind(fingerprint.as_slice())
            .bind(payload)
            .execute(&mut **transaction).await.map_err(map_sqlx_error)?;
        }
    }
    Ok(())
}

fn validate_proof(
    snapshot: &SnapshotValue,
    command: &PublishSnapshot,
) -> Result<(), ApplicationError> {
    match snapshot {
        SnapshotValue::Data(value) => {
            if command.proof().kind() != SnapshotProofKind::Data {
                return Err(invalid());
            }
            validate_blob(
                snapshot,
                command
                    .proof()
                    .get(SnapshotBlobRole::DataParquet)
                    .ok_or_else(invalid)?,
                SnapshotBlobRole::DataParquet,
                value.content_hash(),
            )?;
            validate_blob(
                snapshot,
                command
                    .proof()
                    .get(SnapshotBlobRole::DataManifest)
                    .ok_or_else(invalid)?,
                SnapshotBlobRole::DataManifest,
                value.manifest_hash(),
            )?;
        }
        SnapshotValue::Universe(value) => {
            if command.proof().kind() != SnapshotProofKind::Universe {
                return Err(invalid());
            }
            validate_blob(
                snapshot,
                command
                    .proof()
                    .get(SnapshotBlobRole::UniverseMembersManifest)
                    .ok_or_else(invalid)?,
                SnapshotBlobRole::UniverseMembersManifest,
                value.content_hash(),
            )?;
        }
        SnapshotValue::Position(value) => {
            if command.proof().kind() != SnapshotProofKind::Position {
                return Err(invalid());
            }
            validate_blob(
                snapshot,
                command
                    .proof()
                    .get(SnapshotBlobRole::PositionPayload)
                    .ok_or_else(invalid)?,
                SnapshotBlobRole::PositionPayload,
                value.content_hash(),
            )?;
        }
    }
    Ok(())
}

fn validate_blob(
    snapshot: &SnapshotValue,
    blob: &VerifiedSnapshotBlob,
    role: SnapshotBlobRole,
    expected_hash: &ficant_domain::primitives::ContentHash,
) -> Result<(), ApplicationError> {
    if blob.role() != role
        || blob.scope().tenant_id() != snapshot.owner().tenant_id()
        || blob.owner() != snapshot.owner()
        || blob.verified_blob().content_hash() != expected_hash
        || blob.verified_blob().size() == 0
    {
        return Err(invalid());
    }
    blob.scope().authorize(snapshot.owner())?;
    Ok(())
}

async fn load_persisted_snapshot(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snapshot: &SnapshotValue,
) -> Result<SnapshotValue, ApplicationError> {
    let payload: Option<Vec<u8>> = match snapshot {
        SnapshotValue::Data(_) => sqlx::query_scalar(
            "SELECT payload FROM research.data_snapshots
             WHERE tenant_id = $1 AND data_snapshot_id = $2 AND owner_id = $3",
        )
        .bind(snapshot.owner().tenant_id().as_str())
        .bind(snapshot.id().as_str())
        .bind(snapshot.owner().owner_id().as_str())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?,
        SnapshotValue::Universe(_) => sqlx::query_scalar(
            "SELECT payload FROM research.universe_snapshots
             WHERE tenant_id = $1 AND universe_snapshot_id = $2 AND owner_id = $3",
        )
        .bind(snapshot.owner().tenant_id().as_str())
        .bind(snapshot.id().as_str())
        .bind(snapshot.owner().owner_id().as_str())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?,
        SnapshotValue::Position(_) => sqlx::query_scalar(
            "SELECT payload FROM research.position_snapshots
             WHERE tenant_id = $1 AND snapshot_id = $2 AND owner_id = $3",
        )
        .bind(snapshot.owner().tenant_id().as_str())
        .bind(snapshot.id().as_str())
        .bind(snapshot.owner().owner_id().as_str())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?,
    };
    payload
        .map(|bytes| super::codec::decode_snapshot(&bytes))
        .transpose()?
        .ok_or_else(|| application_error(ApplicationErrorCategory::StorageUnavailable, true))
}

fn invalid() -> ApplicationError {
    application_error(ApplicationErrorCategory::ValidationFailed, false)
}

fn owner_strings(scope: &AccessScope) -> Vec<String> {
    scope
        .allowed_owner_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}
