use async_trait::async_trait;
use ficant_application::ports::{AccessScope, PublishSignalSet, SignalRepository};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::research::SignalSet;
use ficant_domain::{ContentAddressed, Lineaged};
use std::collections::BTreeSet;

use super::PostgresRepository;
use super::codec::{decode_artifact, decode_signal, encode_signal};
use super::common::{
    IdempotencyOutcome, application_error, insert_lineage, lock_idempotency, map_sqlx_error,
    publish_blob_reference,
};

#[async_trait]
impl SignalRepository for PostgresRepository {
    async fn publish(&self, command: PublishSignalSet) -> Result<SignalSet, ApplicationError> {
        PostgresRepository::publish_signal_set(self, command).await
    }

    async fn get(
        &self,
        scope: &AccessScope,
        signal_set_id: ficant_domain::primitives::Ulid,
    ) -> Result<Option<SignalSet>, ApplicationError> {
        let owners = owner_strings(scope);
        let payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT payload FROM research.signal_sets
             WHERE tenant_id = $1 AND signal_set_id = $2
               AND owner_id::text = ANY($3::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(signal_set_id.as_str())
        .bind(&owners)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        payload
            .map(|bytes| super::codec::decode_signal(&bytes))
            .transpose()
    }
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
        let payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT payload FROM research.signal_sets
             WHERE tenant_id = $1 AND signal_set_id = $2 AND artifact_id = $3 AND owner_id = $4",
        )
        .bind(tenant_id)
        .bind(signal_id)
        .bind(artifact_id)
        .bind(signal.owner().owner_id().as_str())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
        let persisted = payload
            .map(|bytes| decode_signal(&bytes))
            .transpose()?
            .ok_or_else(|| application_error(ApplicationErrorCategory::StorageUnavailable, true))?;
        if &persisted != signal {
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        return Ok(persisted);
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
    .bind(crate::minio::content_addressed::hash_hex(
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
    Ok(signal.clone())
}

async fn validate_artifact_binding(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &PublishSignalSet,
) -> Result<(), ApplicationError> {
    let signal = command.signal_set();
    let artifact: Option<(String, String, String, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT owner_id::text, kind, content_hash::text, blob_size, payload
         FROM research.artifacts
         WHERE tenant_id = $1 AND artifact_id = $2
         FOR SHARE",
    )
    .bind(signal.owner().tenant_id().as_str())
    .bind(signal.artifact().object_id().as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let expected_size = i64::try_from(command.verified_blob().size()).map_err(|_| {
        ficant_application::map_domain_error(ficant_domain::DomainErrorCode::InvalidValue)
    })?;
    let expected_metadata = (
        signal.owner().owner_id().as_str().to_owned(),
        "SIGNAL_SET".to_owned(),
        crate::minio::content_addressed::hash_hex(signal.content_hash()),
        expected_size,
    );
    let Some((owner_id, kind, content_hash, blob_size, payload)) = artifact else {
        return Err(application_error(
            ApplicationErrorCategory::LineageIncomplete,
            false,
        ));
    };
    let persisted_artifact = decode_artifact(&payload)
        .map_err(|_| application_error(ApplicationErrorCategory::LineageIncomplete, false))?;
    let actual_metadata = (owner_id, kind, content_hash, blob_size);
    let signal_lineage = canonical_lineage(
        signal
            .lineage()
            .iter()
            .filter(|reference| *reference != signal.artifact()),
    );
    let artifact_lineage = canonical_lineage(persisted_artifact.lineage().iter());
    if actual_metadata != expected_metadata
        || persisted_artifact.id() != signal.artifact().object_id()
        || persisted_artifact.owner() != signal.owner()
        || persisted_artifact.kind() != ficant_domain::research::ArtifactKind::SignalSet
        || persisted_artifact.content_hash() != signal.content_hash()
        || persisted_artifact.blob_size() != command.verified_blob().size()
        || signal_lineage != artifact_lineage
    {
        return Err(application_error(
            ApplicationErrorCategory::LineageIncomplete,
            false,
        ));
    }
    Ok(())
}

fn canonical_lineage<'a>(
    lineage: impl Iterator<Item = &'a ficant_domain::primitives::LineageRef>,
) -> BTreeSet<(String, Option<u64>, Option<[u8; 32]>)> {
    lineage
        .map(|reference| {
            (
                reference.object_id().as_str().to_owned(),
                reference
                    .version()
                    .map(ficant_domain::primitives::Version::get),
                reference.content_hash().map(|hash| *hash.as_bytes()),
            )
        })
        .collect()
}

fn owner_strings(scope: &AccessScope) -> Vec<String> {
    scope
        .allowed_owner_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}
