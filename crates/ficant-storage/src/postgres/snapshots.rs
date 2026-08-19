use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, CanonicalImportReplay, CanonicalImportReplayRequest, GovernedPublishSnapshot,
    PublishSnapshot, SnapshotBlobRole, SnapshotProofKind, SnapshotRepository, SnapshotValue,
    SnapshotVerifiedReadMetadata, SnapshotVerifiedReadMetadataRepository, VerifiedSnapshotBlob,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::ContentAddressed;
use sqlx::types::chrono::{DateTime, Utc};

use super::PostgresRepository;
use super::codec::encode_snapshot;
use super::common::{
    IdempotencyOutcome, application_error, lock_idempotency, map_sqlx_error, publish_blob_reference,
};
use super::ingestion::insert_snapshot_lineage;

struct StoredSnapshotRow {
    kind: String,
    payload: Vec<u8>,
    owner_id: String,
    content_hash: String,
    primary_hash: Option<String>,
    secondary_hash: Option<String>,
    reference_id: Option<String>,
    reference_version: Option<i64>,
    primary_time: Option<DateTime<Utc>>,
    secondary_time: Option<DateTime<Utc>>,
    tertiary_time: Option<DateTime<Utc>>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for StoredSnapshotRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            kind: row.try_get("kind")?,
            payload: row.try_get("payload")?,
            owner_id: row.try_get("owner_id")?,
            content_hash: row.try_get("content_hash")?,
            primary_hash: row.try_get("primary_hash")?,
            secondary_hash: row.try_get("secondary_hash")?,
            reference_id: row.try_get("reference_id")?,
            reference_version: row.try_get("reference_version")?,
            primary_time: row.try_get("primary_time")?,
            secondary_time: row.try_get("secondary_time")?,
            tertiary_time: row.try_get("tertiary_time")?,
        })
    }
}

#[async_trait]
impl SnapshotRepository for PostgresRepository {
    async fn probe_canonical_import_replay(
        &self,
        request: &CanonicalImportReplayRequest,
    ) -> Result<Option<CanonicalImportReplay>, ApplicationError> {
        let tenant = request.owner().tenant_id().as_str();
        let existing: Option<(Vec<u8>, String)> = sqlx::query_as(
            "SELECT fingerprint, result_id::text FROM core.idempotency_records
             WHERE tenant_id=$1 AND scope=$2 AND idempotency_key=$3",
        )
        .bind(tenant)
        .bind(CANONICAL_IMPORT_REPLAY_SCOPE)
        .bind(request.idempotency_key().as_str())
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let Some((fingerprint, result_id)) = existing else {
            return Ok(None);
        };
        if fingerprint != request.fingerprint().content_hash().as_bytes().as_slice()
            || result_id != request.target_snapshot_id().as_str()
        {
            return Err(application_error(
                ApplicationErrorCategory::AlreadyExists,
                false,
            ));
        }
        let snapshot = SnapshotRepository::get_by_id(
            self,
            request.change_context().principal().access_scope(),
            request.target_snapshot_id().clone(),
        )
        .await?
        .ok_or_else(immutable)?;
        let SnapshotValue::Data(snapshot) = snapshot else {
            return Err(immutable());
        };
        let audit_rows: Vec<(String, String, String, i64, String, String)> = sqlx::query_as(
            "SELECT actor_id::text, active_role, authorization_id::text,
                    authorization_version, reason, after_hash::text
             FROM core.foundation_change_records
             WHERE tenant_id=$1 AND operation='data-snapshot.import-canonical-quotes'
               AND resource_kind='data-snapshot' AND resource_id=$2 AND owner_id=$3",
        )
        .bind(tenant)
        .bind(request.target_snapshot_id().as_str())
        .bind(request.owner().owner_id().as_str())
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let [(actor_id, active_role, authorization_id, authorization_version, reason, after_hash)] =
            audit_rows.as_slice()
        else {
            return Err(immutable());
        };
        if active_role != "RESEARCHER"
            || authorization_id != request.authorization().id().as_str()
            || u64::try_from(*authorization_version).ok()
                != Some(request.authorization().version().get())
            || reason != request.change_context().change().reason()
            || after_hash != &crate::s3::content_addressed::hash_hex(snapshot.content_hash())
        {
            return Err(immutable());
        }
        CanonicalImportReplay::verified(
            request,
            snapshot,
            ficant_domain::primitives::Ulid::new(actor_id).map_err(|_| immutable())?,
            request.authorization().clone(),
            request.authorization_hash().clone(),
        )
        .map(Some)
    }

    async fn publish_governed(
        &self,
        command: GovernedPublishSnapshot,
    ) -> Result<SnapshotValue, ApplicationError> {
        let change = command.change_record()?;
        let tenant = command.command().snapshot().owner().tenant_id().as_str();
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let import_outcome = if let Some(request) = command.replay_request() {
            Some(
                lock_idempotency(
                    &mut transaction,
                    tenant,
                    CANONICAL_IMPORT_REPLAY_SCOPE,
                    request.idempotency_key().as_str(),
                    request.fingerprint().content_hash().as_bytes(),
                    request.target_snapshot_id().as_str(),
                )
                .await?,
            )
        } else {
            None
        };
        let (value, outcome) =
            persist_snapshot_with_outcome(&mut transaction, command.command()).await?;
        if import_outcome.is_some_and(|import| import != outcome) {
            return Err(immutable());
        }
        match outcome {
            IdempotencyOutcome::Fresh => {
                super::governance::insert_change(&mut transaction, tenant, &change).await?;
            }
            IdempotencyOutcome::Replay => {
                super::governance::verify_change_replay(
                    &mut transaction,
                    tenant,
                    change.operation(),
                    &change.resource().canonical_ref(),
                    command.fingerprint().content_hash(),
                )
                .await?;
            }
        }
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }

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
        let rows: Vec<StoredSnapshotRow> = sqlx::query_as(
            "SELECT 'data'::text AS kind, payload, owner_id::text, content_hash::text,
                    schema_hash::text AS primary_hash, manifest_hash::text AS secondary_hash,
                    NULL::text AS reference_id, NULL::bigint AS reference_version,
                    visible_at AS primary_time, as_of AS secondary_time,
                    NULL::timestamptz AS tertiary_time
             FROM research.data_snapshots
             WHERE tenant_id = $1 AND data_snapshot_id = $2
               AND owner_id::text = ANY($3::text[])
             UNION ALL
             SELECT 'universe', payload, owner_id::text, content_hash::text,
                    filter_digest::text, NULL::text, NULL::text, NULL::bigint,
                    NULL::timestamptz, NULL::timestamptz, NULL::timestamptz
             FROM research.universe_snapshots
             WHERE tenant_id = $1 AND universe_snapshot_id = $2
               AND owner_id::text = ANY($3::text[])
             UNION ALL
             SELECT 'position', payload, owner_id::text, content_hash::text,
                    NULL::text, NULL::text, subject_id::text, subject_version,
                    observed_at, visible_at, NULL::timestamptz
             FROM research.position_snapshots
             WHERE tenant_id = $1 AND snapshot_id = $2
               AND owner_id::text = ANY($3::text[])
             UNION ALL
             SELECT 'data-health', payload, owner_id::text, content_hash::text,
                    NULL::text, NULL::text, profile_id::text, profile_version,
                    visible_at, effective_from, effective_to
             FROM research.data_health_threshold_profiles
             WHERE tenant_id = $1 AND profile_snapshot_id = $2
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
            [row] => {
                let snapshot = super::codec::decode_snapshot(&row.payload)?;
                validate_snapshot_row(row, &snapshot, scope, &snapshot_id)?;
                validate_snapshot_blob_references(
                    self.pool(),
                    scope.tenant_id().as_str(),
                    &snapshot,
                )
                .await?;
                if let SnapshotValue::Universe(value) = &snapshot {
                    validate_universe_members(self.pool(), scope.tenant_id().as_str(), value)
                        .await?;
                }
                Ok(Some(snapshot))
            }
            _ => Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            )),
        }
    }
}

const CANONICAL_IMPORT_REPLAY_SCOPE: &str = "data-snapshot:canonical-import-request:v1";

fn validate_snapshot_row(
    row: &StoredSnapshotRow,
    snapshot: &SnapshotValue,
    scope: &AccessScope,
    requested_id: &ficant_domain::primitives::Ulid,
) -> Result<(), ApplicationError> {
    if snapshot.id() != requested_id
        || snapshot.owner().tenant_id() != scope.tenant_id()
        || snapshot.owner().owner_id().as_str() != row.owner_id
        || crate::s3::content_addressed::hash_hex(snapshot.content_hash()) != row.content_hash
    {
        return Err(storage_integrity_error());
    }
    let valid = match (row.kind.as_str(), snapshot) {
        ("data", SnapshotValue::Data(value)) => {
            row.primary_hash.as_deref()
                == Some(crate::s3::content_addressed::hash_hex(value.schema_hash()).as_str())
                && row.secondary_hash.as_deref()
                    == Some(crate::s3::content_addressed::hash_hex(value.manifest_hash()).as_str())
                && row
                    .primary_time
                    .as_ref()
                    .is_some_and(|stored| *stored == value.visible_at().instant())
                && row
                    .secondary_time
                    .as_ref()
                    .is_some_and(|stored| *stored == value.as_of().instant())
        }
        ("universe", SnapshotValue::Universe(value)) => {
            row.primary_hash.as_deref()
                == Some(crate::s3::content_addressed::hash_hex(value.filter_digest()).as_str())
        }
        ("position", SnapshotValue::Position(value)) => {
            row.reference_id.as_deref() == Some(value.subject_ref().id().as_str())
                && row.reference_version == i64::try_from(value.subject_ref().version().get()).ok()
                && row
                    .primary_time
                    .as_ref()
                    .is_some_and(|stored| *stored == value.observed_at().instant())
                && row
                    .secondary_time
                    .as_ref()
                    .is_some_and(|stored| *stored == value.visible_at().instant())
        }
        ("data-health", SnapshotValue::DataHealthThresholdProfile(value)) => {
            row.reference_id.as_deref() == Some(value.profile_ref().id().as_str())
                && row.reference_version == i64::try_from(value.profile_ref().version().get()).ok()
                && row
                    .primary_time
                    .as_ref()
                    .is_some_and(|stored| *stored == value.visible_at().instant())
                && row
                    .secondary_time
                    .as_ref()
                    .is_some_and(|stored| *stored == value.effective_from().instant())
                && row
                    .tertiary_time
                    .as_ref()
                    .is_some_and(|stored| *stored == value.effective_to().instant())
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(storage_integrity_error())
    }
}

async fn validate_snapshot_blob_references(
    pool: &sqlx::PgPool,
    tenant_id: &str,
    snapshot: &SnapshotValue,
) -> Result<(), ApplicationError> {
    let hashes = match snapshot {
        SnapshotValue::Data(value) => vec![value.content_hash(), value.manifest_hash()],
        SnapshotValue::DataHealthThresholdProfile(value) => vec![value.content_hash()],
        SnapshotValue::Position(value) => vec![value.content_hash()],
        SnapshotValue::Universe(value) => vec![value.content_hash()],
    };
    for hash in hashes {
        let size: Option<i64> = sqlx::query_scalar(
            "SELECT blob_size FROM storage.blobs WHERE tenant_id=$1 AND content_hash=$2",
        )
        .bind(tenant_id)
        .bind(crate::s3::content_addressed::hash_hex(hash))
        .fetch_optional(pool)
        .await
        .map_err(map_sqlx_error)?;
        match size {
            Some(value) if value > 0 => {}
            Some(_) => return Err(storage_integrity_error()),
            None => {
                return Err(application_error(
                    ApplicationErrorCategory::LineageIncomplete,
                    false,
                ));
            }
        }
    }
    Ok(())
}

async fn validate_universe_members(
    pool: &sqlx::PgPool,
    tenant_id: &str,
    snapshot: &ficant_domain::research::UniverseSnapshot,
) -> Result<(), ApplicationError> {
    let rows: Vec<(i32, String, i64)> = sqlx::query_as(
        "SELECT ordinal, instrument_id::text, instrument_version
         FROM research.universe_members
         WHERE tenant_id=$1 AND universe_snapshot_id=$2
         ORDER BY ordinal",
    )
    .bind(tenant_id)
    .bind(snapshot.id().as_str())
    .fetch_all(pool)
    .await
    .map_err(map_sqlx_error)?;
    if rows.len() != snapshot.instrument_versions().len() {
        return Err(storage_integrity_error());
    }
    for (index, ((ordinal, instrument_id, version), expected)) in
        rows.iter().zip(snapshot.instrument_versions()).enumerate()
    {
        if usize::try_from(*ordinal).ok() != Some(index)
            || instrument_id != expected.id().as_str()
            || u64::try_from(*version).ok() != Some(expected.version().get())
        {
            return Err(storage_integrity_error());
        }
    }
    Ok(())
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
            SnapshotValue::DataHealthThresholdProfile(_) | SnapshotValue::Position(_) => Err(
                application_error(ApplicationErrorCategory::ValidationFailed, false),
            ),
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
    Ok(persist_snapshot_with_outcome(transaction, command).await?.0)
}

async fn persist_snapshot_with_outcome(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &PublishSnapshot,
) -> Result<(SnapshotValue, IdempotencyOutcome), ApplicationError> {
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
        return Ok((persisted, outcome));
    }
    let payload = encode_snapshot(snapshot);
    insert_snapshot_metadata(transaction, command, &payload).await?;
    insert_snapshot_lineage(transaction, tenant_id, snapshot_id, snapshot.lineage()).await?;
    Ok((snapshot.clone(), outcome))
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
            insert_position_snapshot(
                transaction,
                value,
                command.idempotency_key().as_str(),
                fingerprint,
                payload,
            )
            .await?;
        }
        SnapshotValue::DataHealthThresholdProfile(value) => {
            insert_data_health_threshold_profile(
                transaction,
                value,
                command.idempotency_key().as_str(),
                fingerprint,
                payload,
            )
            .await?;
        }
    }
    Ok(())
}

async fn insert_position_snapshot(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    value: &ficant_domain::research::PositionSnapshot,
    idempotency_key: &str,
    fingerprint: &[u8],
    payload: &[u8],
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO research.position_snapshots
         (tenant_id, snapshot_id, owner_id, subject_id, subject_version, observed_at, visible_at,
          content_hash, idempotency_key, fingerprint, payload)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.id().as_str())
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get()).map_err(|_| invalid())?)
    .bind(value.observed_at().instant())
    .bind(value.visible_at().instant())
    .bind(crate::s3::content_addressed::hash_hex(value.content_hash()))
    .bind(idempotency_key)
    .bind(fingerprint)
    .bind(payload)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

async fn insert_data_health_threshold_profile(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    value: &ficant_domain::research::DataHealthThresholdProfile,
    idempotency_key: &str,
    fingerprint: &[u8],
    payload: &[u8],
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO research.data_health_threshold_profiles
         (tenant_id, profile_snapshot_id, owner_id, profile_id, profile_version,
          visible_at, effective_from, effective_to, content_hash,
          idempotency_key, fingerprint, payload)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.id().as_str())
    .bind(value.owner().owner_id().as_str())
    .bind(value.profile_ref().id().as_str())
    .bind(i64::try_from(value.profile_ref().version().get()).map_err(|_| invalid())?)
    .bind(value.visible_at().instant())
    .bind(value.effective_from().instant())
    .bind(value.effective_to().instant())
    .bind(crate::s3::content_addressed::hash_hex(value.content_hash()))
    .bind(idempotency_key)
    .bind(fingerprint)
    .bind(payload)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
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
        SnapshotValue::DataHealthThresholdProfile(value) => {
            if command.proof().kind() != SnapshotProofKind::DataHealthThresholdProfile {
                return Err(invalid());
            }
            validate_blob(
                snapshot,
                command
                    .proof()
                    .get(SnapshotBlobRole::DataHealthThresholdProfilePayload)
                    .ok_or_else(invalid)?,
                SnapshotBlobRole::DataHealthThresholdProfilePayload,
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
        SnapshotValue::DataHealthThresholdProfile(_) => sqlx::query_scalar(
            "SELECT payload FROM research.data_health_threshold_profiles
             WHERE tenant_id = $1 AND profile_snapshot_id = $2 AND owner_id = $3",
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

fn storage_integrity_error() -> ApplicationError {
    application_error(ApplicationErrorCategory::StorageUnavailable, false)
}

fn immutable() -> ApplicationError {
    application_error(ApplicationErrorCategory::ImmutableViolation, false)
}

fn owner_strings(scope: &AccessScope) -> Vec<String> {
    scope
        .allowed_owner_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}
