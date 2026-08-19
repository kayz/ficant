use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, IntegrityEventSink, IntegrityFailureReason, PublishSignalSet,
    RequiredVerifiedBlobRead, SafeTraceContext, SignalRepository, VerifiedBlobRole,
    VerifiedReadResourceKind,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid};
use ficant_domain::research::{Artifact, ArtifactKind, SignalSet};
use ficant_domain::{ContentAddressed, Lineaged};
use sqlx::PgConnection;
use sqlx::types::chrono::{DateTime, Utc};

use super::PostgresRepository;
use super::codec::{decode_signal, encode_signal};
use super::common::{
    IdempotencyOutcome, application_error, insert_lineage, lock_idempotency, map_sqlx_error,
    publish_blob_reference,
};

type SignalRow = (
    String,
    String,
    String,
    String,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
    Vec<u8>,
);

#[async_trait]
impl SignalRepository for PostgresRepository {
    async fn publish(&self, command: PublishSignalSet) -> Result<SignalSet, ApplicationError> {
        PostgresRepository::publish_signal_set(self, command).await
    }

    async fn get(
        &self,
        scope: &AccessScope,
        signal_set_id: Ulid,
    ) -> Result<Option<SignalSet>, ApplicationError> {
        let payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT payload FROM research.signal_sets
             WHERE tenant_id = $1 AND signal_set_id = $2
               AND owner_id::text = ANY($3::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(signal_set_id.as_str())
        .bind(owner_strings(scope))
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        payload.map(|bytes| decode_signal(&bytes)).transpose()
    }

    async fn get_integrity_checked(
        &self,
        scope: &AccessScope,
        signal_set_id: Ulid,
        trace: SafeTraceContext,
        sink: &dyn IntegrityEventSink,
    ) -> Result<Option<SignalSet>, ApplicationError> {
        let event_request = signal_event_request(self, scope, &signal_set_id, trace).await?;
        let mut connection = self.pool().acquire().await.map_err(map_sqlx_error)?;
        let result = load_persisted_signal(
            &mut connection,
            scope.tenant_id().as_str(),
            signal_set_id.as_str(),
        )
        .await
        .and_then(|signal| {
            if let Some(value) = &signal {
                scope.authorize(value.owner())?;
            }
            Ok(signal)
        });
        if let (Err(error), Some(request)) = (&result, event_request)
            && is_integrity_error(error)
        {
            let _ = request
                .fail_integrity(sink, IntegrityFailureReason::HashMismatch)
                .await;
        }
        result
    }
}

fn owner_strings(scope: &AccessScope) -> Vec<String> {
    scope
        .allowed_owner_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

async fn signal_event_request(
    repository: &PostgresRepository,
    scope: &AccessScope,
    signal_set_id: &Ulid,
    trace: SafeTraceContext,
) -> Result<Option<RequiredVerifiedBlobRead>, ApplicationError> {
    let row: Option<(String, String, Option<i64>)> = sqlx::query_as(
        "SELECT signal.owner_id::text, signal.content_hash::text,
                COALESCE(artifact.blob_size, blob.blob_size)
         FROM research.signal_sets signal
         LEFT JOIN research.artifacts artifact
           ON artifact.tenant_id=signal.tenant_id
          AND artifact.artifact_id=signal.artifact_id
         LEFT JOIN storage.blobs blob
           ON blob.tenant_id=signal.tenant_id
          AND blob.content_hash=signal.content_hash
         WHERE signal.tenant_id=$1 AND signal.signal_set_id=$2",
    )
    .bind(scope.tenant_id().as_str())
    .bind(signal_set_id.as_str())
    .fetch_optional(repository.pool())
    .await
    .map_err(map_sqlx_error)?;
    let Some((owner_id, content_hash, Some(blob_size))) = row else {
        return Ok(None);
    };
    let Some(request) = Ulid::new(owner_id)
        .ok()
        .zip(parse_hash(&content_hash))
        .zip(u64::try_from(blob_size).ok())
        .and_then(|((owner_id, content_hash), blob_size)| {
            RequiredVerifiedBlobRead::new(
                scope.clone(),
                OwnerRef::new(scope.tenant_id().clone(), owner_id),
                VerifiedReadResourceKind::SignalSet,
                signal_set_id.clone(),
                VerifiedBlobRole::SignalPayload,
                content_hash,
                blob_size,
                trace,
            )
            .ok()
        })
    else {
        return Ok(None);
    };
    Ok(Some(request))
}

fn parse_hash(value: &str) -> Option<ContentHash> {
    if value.len() != 64 {
        return None;
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    ContentHash::from_bytes(&bytes).ok()
}

fn is_integrity_error(error: &ApplicationError) -> bool {
    matches!(
        error.category(),
        ApplicationErrorCategory::ImmutableViolation
            | ApplicationErrorCategory::LineageIncomplete
            | ApplicationErrorCategory::HashMismatch
    )
}

impl PostgresRepository {
    /// Publishes a signal set only with its verified content and complete lineage.
    ///
    /// # Errors
    ///
    /// Returns a classified application error on immutable, lineage, or storage conflict.
    pub async fn publish_signal_set(
        &self,
        command: PublishSignalSet,
    ) -> Result<SignalSet, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let value = persist_signal(&mut transaction, &command).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }
}

pub(crate) async fn persist_signal(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &PublishSignalSet,
) -> Result<SignalSet, ApplicationError> {
    let signal = command.signal_set();
    let tenant_id = signal.owner().tenant_id().as_str();
    let signal_id = signal.id().as_str();
    let artifact_id = signal.artifact().object_id().as_str();
    let fingerprint = command.fingerprint().content_hash().as_bytes();
    validate_artifact_binding(transaction, command).await?;
    let outcome = lock_idempotency(
        transaction,
        tenant_id,
        "signal-set:publish:v1",
        command.idempotency_key().as_str(),
        fingerprint,
        signal_id,
    )
    .await?;
    if outcome == IdempotencyOutcome::Replay {
        return require_exact_persisted_signal(transaction, signal).await;
    }

    publish_blob_reference(
        transaction,
        tenant_id,
        signal.content_hash(),
        command.verified_blob().size(),
    )
    .await?;
    sqlx::query(
        "INSERT INTO research.signal_sets
             (tenant_id, signal_set_id, artifact_id, owner_id, experiment_run_id, content_hash,
              valid_from, valid_to, idempotency_key, fingerprint, payload)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(tenant_id)
    .bind(signal_id)
    .bind(artifact_id)
    .bind(signal.owner().owner_id().as_str())
    .bind(signal.experiment_run_id().as_str())
    .bind(crate::s3::content_addressed::hash_hex(
        signal.content_hash(),
    ))
    .bind(signal.valid().from().instant())
    .bind(signal.valid().to().instant())
    .bind(command.idempotency_key().as_str())
    .bind(fingerprint.as_slice())
    .bind(encode_signal(signal))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    insert_lineage(transaction, tenant_id, signal_id, signal.lineage()).await?;
    require_exact_persisted_signal(transaction, signal).await
}

async fn validate_artifact_binding(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &PublishSignalSet,
) -> Result<(), ApplicationError> {
    let signal = command.signal_set();
    let artifact = super::artifacts::load_persisted_artifact(
        transaction,
        signal.owner().tenant_id().as_str(),
        signal.artifact().object_id().as_str(),
    )
    .await?;
    let Some(artifact) = artifact else {
        return Err(application_error(
            ApplicationErrorCategory::LineageIncomplete,
            false,
        ));
    };
    if artifact.blob_size() != command.verified_blob().size()
        || !signal_matches_artifact(signal, &artifact)
    {
        return Err(application_error(
            ApplicationErrorCategory::LineageIncomplete,
            false,
        ));
    }
    Ok(())
}

async fn require_exact_persisted_signal(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    expected: &SignalSet,
) -> Result<SignalSet, ApplicationError> {
    let persisted = load_persisted_signal(
        transaction,
        expected.owner().tenant_id().as_str(),
        expected.id().as_str(),
    )
    .await?
    .ok_or_else(immutable_violation)?;
    if &persisted != expected {
        return Err(immutable_violation());
    }
    Ok(persisted)
}

async fn load_persisted_signal(
    connection: &mut PgConnection,
    tenant_id: &str,
    signal_set_id: &str,
) -> Result<Option<SignalSet>, ApplicationError> {
    let row: Option<SignalRow> = sqlx::query_as(
        "SELECT signal_set_id::text, artifact_id::text, owner_id::text,
                experiment_run_id::text, content_hash::text, valid_from, valid_to, payload
         FROM research.signal_sets
         WHERE tenant_id = $1 AND signal_set_id = $2",
    )
    .bind(tenant_id)
    .bind(signal_set_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sqlx_error)?;
    let Some((
        stored_id,
        artifact_id,
        owner_id,
        run_id,
        content_hash,
        valid_from,
        valid_to,
        payload,
    )) = row
    else {
        return Ok(None);
    };
    let signal = decode_signal(&payload).map_err(|_| immutable_violation())?;
    if stored_id != signal_set_id
        || signal.id().as_str() != stored_id
        || signal.owner().tenant_id().as_str() != tenant_id
        || signal.owner().owner_id().as_str() != owner_id
        || signal.artifact().object_id().as_str() != artifact_id
        || signal.experiment_run_id().as_str() != run_id
        || crate::s3::content_addressed::hash_hex(signal.content_hash()) != content_hash
        || signal.valid().from().instant() != valid_from
        || signal.valid().to().instant() != valid_to
    {
        return Err(immutable_violation());
    }
    verify_signal_lineage(connection, tenant_id, signal_set_id, &signal).await?;
    let artifact = super::artifacts::load_persisted_artifact(
        connection,
        tenant_id,
        signal.artifact().object_id().as_str(),
    )
    .await?
    .ok_or_else(lineage_incomplete)?;
    if !signal_matches_artifact(&signal, &artifact) {
        return Err(lineage_incomplete());
    }
    Ok(Some(signal))
}

async fn verify_signal_lineage(
    connection: &mut PgConnection,
    tenant_id: &str,
    signal_set_id: &str,
    signal: &SignalSet,
) -> Result<(), ApplicationError> {
    let actual: Vec<(i32, String, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT lineage_ordinal, target_object_id::text, target_version,
                target_content_hash::text
         FROM research.lineage_edges
         WHERE tenant_id = $1 AND source_object_id = $2
         ORDER BY lineage_ordinal ASC",
    )
    .bind(tenant_id)
    .bind(signal_set_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(map_sqlx_error)?;
    let expected = signal
        .lineage()
        .iter()
        .enumerate()
        .map(|(ordinal, reference)| {
            Ok((
                i32::try_from(ordinal).map_err(|_| lineage_incomplete())?,
                reference.object_id().as_str().to_owned(),
                reference
                    .version()
                    .map(|version| i64::try_from(version.get()))
                    .transpose()
                    .map_err(|_| lineage_incomplete())?,
                reference
                    .content_hash()
                    .map(crate::s3::content_addressed::hash_hex),
            ))
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    if actual != expected {
        return Err(lineage_incomplete());
    }
    Ok(())
}

fn signal_matches_artifact(signal: &SignalSet, artifact: &Artifact) -> bool {
    artifact.kind() == ArtifactKind::SignalSet
        && artifact.owner() == signal.owner()
        && signal.artifact().object_id() == artifact.id()
        && signal.artifact().version().is_none()
        && signal.artifact().content_hash() == Some(artifact.content_hash())
        && signal.content_hash() == artifact.content_hash()
        && signal.lineage().first() == Some(signal.artifact())
        && signal.lineage().get(1..) == Some(artifact.lineage())
}

fn immutable_violation() -> ApplicationError {
    application_error(ApplicationErrorCategory::ImmutableViolation, false)
}

fn lineage_incomplete() -> ApplicationError {
    application_error(ApplicationErrorCategory::LineageIncomplete, false)
}
