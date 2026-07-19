use async_trait::async_trait;
use ficant_application::ApplicationError;
use ficant_application::ports::{AccessScope, ArtifactRepository, PublishArtifact};
use ficant_domain::research::{Artifact, ArtifactKind};
use ficant_domain::{ContentAddressed, Lineaged};

use super::PostgresRepository;
use super::codec::encode_artifact;
use super::common::{
    IdempotencyOutcome, insert_lineage, lock_idempotency, map_sqlx_error, publish_blob_reference,
};

#[async_trait]
impl ArtifactRepository for PostgresRepository {
    async fn publish_verified_blob(
        &self,
        command: PublishArtifact,
    ) -> Result<Artifact, ApplicationError> {
        PostgresRepository::publish_verified_blob(self, command).await
    }

    async fn get_metadata(
        &self,
        scope: &AccessScope,
        artifact_id: ficant_domain::primitives::Ulid,
    ) -> Result<Option<Artifact>, ApplicationError> {
        let owners = owner_strings(scope);
        let payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT payload FROM research.artifacts
             WHERE tenant_id = $1 AND artifact_id = $2
               AND owner_id::text = ANY($3::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(artifact_id.as_str())
        .bind(&owners)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        payload
            .map(|bytes| super::codec::decode_artifact(&bytes))
            .transpose()
    }
}

impl PostgresRepository {
    /// Publishes artifact metadata and its immutable lineage in one transaction.
    ///
    /// # Errors
    ///
    /// Returns a classified application error when persistence cannot preserve the intent.
    pub async fn publish_verified_blob(
        &self,
        command: PublishArtifact,
    ) -> Result<Artifact, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let value = persist_artifact(&mut transaction, &command).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }
}

pub(crate) async fn persist_artifact(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &PublishArtifact,
) -> Result<Artifact, ApplicationError> {
    let artifact = command.artifact();
    let tenant_id = artifact.owner().tenant_id().as_str();
    let artifact_id = artifact.id().as_str();
    let fingerprint = command.fingerprint().content_hash().as_bytes();
    let outcome = lock_idempotency(
        transaction,
        tenant_id,
        "artifact:publish:v1",
        command.idempotency_key().as_str(),
        fingerprint,
        artifact_id,
    )
    .await?;
    if outcome == IdempotencyOutcome::Replay {
        return Ok(artifact.clone());
    }

    publish_blob_reference(
        transaction,
        tenant_id,
        artifact.content_hash(),
        artifact.blob_size(),
    )
    .await?;
    sqlx::query(
        "INSERT INTO research.artifacts
             (tenant_id, artifact_id, owner_id, kind, media_type, content_hash, blob_size,
              idempotency_key, fingerprint, payload)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(tenant_id)
    .bind(artifact_id)
    .bind(artifact.owner().owner_id().as_str())
    .bind(artifact_kind(artifact.kind()))
    .bind(artifact.media_type())
    .bind(crate::s3::content_addressed::hash_hex(
        artifact.content_hash(),
    ))
    .bind(i64::try_from(artifact.blob_size()).map_err(|_| {
        ficant_application::map_domain_error(ficant_domain::DomainErrorCode::InvalidValue)
    })?)
    .bind(command.idempotency_key().as_str())
    .bind(fingerprint.as_slice())
    .bind(encode_artifact(artifact))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    insert_lineage(transaction, tenant_id, artifact_id, artifact.lineage()).await?;
    Ok(artifact.clone())
}

fn owner_strings(scope: &AccessScope) -> Vec<String> {
    scope
        .allowed_owner_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

const fn artifact_kind(value: ArtifactKind) -> &'static str {
    match value {
        ArtifactKind::Generic => "GENERIC",
        ArtifactKind::CurveSnapshot => "CURVE_SNAPSHOT",
        ArtifactKind::DataSnapshot => "DATA_SNAPSHOT",
        ArtifactKind::UniverseSnapshot => "UNIVERSE_SNAPSHOT",
        ArtifactKind::SignalSet => "SIGNAL_SET",
    }
}
