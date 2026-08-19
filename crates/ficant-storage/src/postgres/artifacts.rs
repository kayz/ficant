use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, ArtifactRepository, IntegrityEventSink, IntegrityFailureReason, PublishArtifact,
    RequiredVerifiedBlobRead, SafeTraceContext, VerifiedBlobRole, VerifiedReadResourceKind,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid};
use ficant_domain::research::{Artifact, ArtifactKind};
use ficant_domain::{ContentAddressed, Lineaged};
use ficant_runtime::{FormalInputReference, FormalOutputEvidence};
use sqlx::PgConnection;

use super::PostgresRepository;
use super::codec::{decode_artifact, encode_artifact};
use super::common::{
    IdempotencyOutcome, application_error, insert_lineage, lock_idempotency, map_sqlx_error,
    publish_blob_reference,
};

type ArtifactRow = (String, String, String, String, String, i64, Vec<u8>);

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
        artifact_id: Ulid,
    ) -> Result<Option<Artifact>, ApplicationError> {
        let payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT payload FROM research.artifacts
             WHERE tenant_id = $1 AND artifact_id = $2
               AND owner_id::text = ANY($3::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(artifact_id.as_str())
        .bind(owner_strings(scope))
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        payload.map(|bytes| decode_artifact(&bytes)).transpose()
    }

    async fn get_formal_evidence(
        &self,
        scope: &AccessScope,
        artifact_id: Ulid,
    ) -> Result<Option<FormalOutputEvidence>, ApplicationError> {
        let mut connection = self.pool().acquire().await.map_err(map_sqlx_error)?;
        let Some(artifact) = load_persisted_artifact(
            &mut connection,
            scope.tenant_id().as_str(),
            artifact_id.as_str(),
        )
        .await?
        else {
            return Ok(None);
        };
        scope.authorize(artifact.owner())?;
        load_artifact_formal_evidence(&mut connection, &artifact).await
    }

    async fn get_integrity_checked_metadata(
        &self,
        scope: &AccessScope,
        artifact_id: Ulid,
        trace: SafeTraceContext,
        sink: &dyn IntegrityEventSink,
    ) -> Result<Option<Artifact>, ApplicationError> {
        let event_request = artifact_event_request(self, scope, &artifact_id, trace).await?;
        let mut connection = self.pool().acquire().await.map_err(map_sqlx_error)?;
        let result = load_persisted_artifact(
            &mut connection,
            scope.tenant_id().as_str(),
            artifact_id.as_str(),
        )
        .await
        .and_then(|artifact| {
            if let Some(value) = &artifact {
                scope.authorize(value.owner())?;
            }
            Ok(artifact)
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

async fn artifact_event_request(
    repository: &PostgresRepository,
    scope: &AccessScope,
    artifact_id: &Ulid,
    trace: SafeTraceContext,
) -> Result<Option<RequiredVerifiedBlobRead>, ApplicationError> {
    let row: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT owner_id::text, content_hash::text, blob_size
         FROM research.artifacts
         WHERE tenant_id=$1 AND artifact_id=$2",
    )
    .bind(scope.tenant_id().as_str())
    .bind(artifact_id.as_str())
    .fetch_optional(repository.pool())
    .await
    .map_err(map_sqlx_error)?;
    let Some((owner_id, content_hash, blob_size)) = row else {
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
                VerifiedReadResourceKind::Artifact,
                artifact_id.clone(),
                VerifiedBlobRole::ArtifactPayload,
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
        return require_exact_persisted_artifact(transaction, artifact, command.formal_evidence())
            .await;
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
    persist_or_verify_artifact_formal_evidence(transaction, artifact, command.formal_evidence())
        .await?;
    require_exact_persisted_artifact(transaction, artifact, command.formal_evidence()).await
}

async fn require_exact_persisted_artifact(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    expected: &Artifact,
    expected_evidence: Option<&FormalOutputEvidence>,
) -> Result<Artifact, ApplicationError> {
    let persisted = load_persisted_artifact(
        transaction,
        expected.owner().tenant_id().as_str(),
        expected.id().as_str(),
    )
    .await?
    .ok_or_else(immutable_violation)?;
    if &persisted != expected {
        return Err(immutable_violation());
    }
    let actual_evidence = load_artifact_formal_evidence(transaction, &persisted).await?;
    if actual_evidence.as_ref() != expected_evidence {
        return Err(immutable_violation());
    }
    Ok(persisted)
}

pub(crate) async fn load_persisted_artifact(
    connection: &mut PgConnection,
    tenant_id: &str,
    artifact_id: &str,
) -> Result<Option<Artifact>, ApplicationError> {
    let row: Option<ArtifactRow> = sqlx::query_as(
        "SELECT artifact_id::text, owner_id::text, kind, media_type,
                content_hash::text, blob_size, payload
         FROM research.artifacts
         WHERE tenant_id = $1 AND artifact_id = $2",
    )
    .bind(tenant_id)
    .bind(artifact_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sqlx_error)?;
    let Some((stored_id, owner_id, kind, media_type, content_hash, blob_size, payload)) = row
    else {
        return Ok(None);
    };
    let artifact = decode_artifact(&payload).map_err(|_| immutable_violation())?;
    let expected_size = i64::try_from(artifact.blob_size()).map_err(|_| immutable_violation())?;
    if stored_id != artifact_id
        || artifact.id().as_str() != stored_id
        || artifact.owner().tenant_id().as_str() != tenant_id
        || artifact.owner().owner_id().as_str() != owner_id
        || artifact_kind(artifact.kind()) != kind
        || artifact.media_type() != media_type
        || crate::s3::content_addressed::hash_hex(artifact.content_hash()) != content_hash
        || expected_size != blob_size
    {
        return Err(immutable_violation());
    }

    verify_artifact_lineage(connection, tenant_id, artifact_id, &artifact).await?;
    verify_artifact_blob_reference(connection, tenant_id, &artifact).await?;
    let _ = load_artifact_formal_evidence(connection, &artifact).await?;
    Ok(Some(artifact))
}

pub(crate) async fn persist_or_verify_artifact_formal_evidence(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    artifact: &Artifact,
    expected: Option<&FormalOutputEvidence>,
) -> Result<(), ApplicationError> {
    if let Some(evidence) = expected {
        validate_evidence_binding(artifact, evidence)?;
        let FormalInputReference::Object(subject) = evidence.subject().reference() else {
            return Err(lineage_incomplete());
        };
        let subject_version = subject.version().ok_or_else(lineage_incomplete)?;
        let subject_hash = subject.content_hash().ok_or_else(lineage_incomplete)?;
        let encoded = super::formal_outputs::encode_formal_evidence(evidence);
        sqlx::query(
            "INSERT INTO research.artifact_formal_evidence
             (tenant_id,artifact_id,output_identity,subject_id,subject_version,
              subject_content_hash,code_commit_sha,code_tree_sha,code_digest,
              runtime_image_digest,environment_digest,parameters_hash,seed,result_hash,
              formal_evidence)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13::numeric,$14,$15)
             ON CONFLICT (tenant_id,artifact_id) DO NOTHING",
        )
        .bind(artifact.owner().tenant_id().as_str())
        .bind(artifact.id().as_str())
        .bind(hash_hex(evidence.output_identity()))
        .bind(subject.object_id().as_str())
        .bind(i64::try_from(subject_version.get()).map_err(|_| immutable_violation())?)
        .bind(hash_hex(subject_hash))
        .bind(evidence.code().git_commit_sha())
        .bind(evidence.code().git_tree_sha())
        .bind(hash_hex(evidence.code().digest()))
        .bind(hash_hex(evidence.runtime().image_digest()))
        .bind(hash_hex(evidence.runtime().environment_digest()))
        .bind(hash_hex(evidence.parameters_hash()))
        .bind(evidence.seed().map(|value| value.to_string()))
        .bind(hash_hex(evidence.result_hash()))
        .bind(encoded)
        .execute(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
    }
    let actual = load_artifact_formal_evidence(transaction, artifact).await?;
    (actual.as_ref() == expected)
        .then_some(())
        .ok_or_else(immutable_violation)
}

pub(crate) async fn load_artifact_formal_evidence(
    connection: &mut PgConnection,
    artifact: &Artifact,
) -> Result<Option<FormalOutputEvidence>, ApplicationError> {
    type EvidenceRow = (
        String,
        String,
        i64,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        Vec<u8>,
    );
    let row: Option<EvidenceRow> = sqlx::query_as(
        "SELECT output_identity::text,subject_id::text,subject_version,
                subject_content_hash::text,code_commit_sha,code_tree_sha,code_digest::text,
                runtime_image_digest::text,environment_digest::text,parameters_hash::text,
                seed::text,result_hash::text,formal_evidence
         FROM research.artifact_formal_evidence
         WHERE tenant_id=$1 AND artifact_id=$2",
    )
    .bind(artifact.owner().tenant_id().as_str())
    .bind(artifact.id().as_str())
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sqlx_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let evidence = super::formal_outputs::decode_formal_evidence(&row.12)?;
    validate_evidence_binding(artifact, &evidence)?;
    let FormalInputReference::Object(subject) = evidence.subject().reference() else {
        return Err(lineage_incomplete());
    };
    let subject_version = subject.version().ok_or_else(lineage_incomplete)?;
    let subject_hash = subject.content_hash().ok_or_else(lineage_incomplete)?;
    let (
        output_identity,
        subject_id,
        stored_subject_version,
        stored_subject_hash,
        code_commit,
        code_tree,
        code_digest,
        runtime_image,
        environment,
        parameters,
        seed,
        result_hash,
        encoded,
    ) = row;
    if output_identity != hash_hex(evidence.output_identity())
        || subject_id != subject.object_id().as_str()
        || stored_subject_version
            != i64::try_from(subject_version.get()).map_err(|_| immutable_violation())?
        || stored_subject_hash != hash_hex(subject_hash)
        || code_commit != evidence.code().git_commit_sha()
        || code_tree != evidence.code().git_tree_sha()
        || code_digest != hash_hex(evidence.code().digest())
        || runtime_image != hash_hex(evidence.runtime().image_digest())
        || environment != hash_hex(evidence.runtime().environment_digest())
        || parameters != hash_hex(evidence.parameters_hash())
        || seed != evidence.seed().map(|value| value.to_string())
        || result_hash != hash_hex(evidence.result_hash())
        || encoded != super::formal_outputs::encode_formal_evidence(&evidence)
    {
        return Err(immutable_violation());
    }
    Ok(Some(evidence))
}

fn validate_evidence_binding(
    artifact: &Artifact,
    evidence: &FormalOutputEvidence,
) -> Result<(), ApplicationError> {
    if evidence.result_hash() != artifact.content_hash()
        || evidence.subject().owner() != artifact.owner()
    {
        return Err(lineage_incomplete());
    }
    Ok(())
}

fn hash_hex(value: &ContentHash) -> String {
    crate::s3::content_addressed::hash_hex(value)
}

async fn verify_artifact_lineage(
    connection: &mut PgConnection,
    tenant_id: &str,
    artifact_id: &str,
    artifact: &Artifact,
) -> Result<(), ApplicationError> {
    let actual: Vec<(i32, String, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT lineage_ordinal, target_object_id::text, target_version,
                target_content_hash::text
         FROM research.lineage_edges
         WHERE tenant_id = $1 AND source_object_id = $2
         ORDER BY lineage_ordinal ASC",
    )
    .bind(tenant_id)
    .bind(artifact_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(map_sqlx_error)?;
    let expected = artifact
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

async fn verify_artifact_blob_reference(
    connection: &mut PgConnection,
    tenant_id: &str,
    artifact: &Artifact,
) -> Result<(), ApplicationError> {
    let hash = crate::s3::content_addressed::hash_hex(artifact.content_hash());
    let actual: Option<(String, i64)> = sqlx::query_as(
        "SELECT object_key, blob_size FROM storage.blobs
         WHERE tenant_id = $1 AND content_hash = $2",
    )
    .bind(tenant_id)
    .bind(&hash)
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sqlx_error)?;
    let size = i64::try_from(artifact.blob_size()).map_err(|_| immutable_violation())?;
    if actual != Some((format!("immutable/{hash}"), size)) {
        return Err(application_error(
            ApplicationErrorCategory::HashMismatch,
            false,
        ));
    }
    Ok(())
}

fn immutable_violation() -> ApplicationError {
    application_error(ApplicationErrorCategory::ImmutableViolation, false)
}

fn lineage_incomplete() -> ApplicationError {
    application_error(ApplicationErrorCategory::LineageIncomplete, false)
}

const fn artifact_kind(value: ArtifactKind) -> &'static str {
    match value {
        ArtifactKind::Generic => "GENERIC",
        ArtifactKind::SignalSet => "SIGNAL_SET",
    }
}
