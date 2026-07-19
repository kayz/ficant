use async_trait::async_trait;
use ficant_application::ApplicationError;
use ficant_application::ports::{
    AccessScope, CreateExperimentRun, DefinitionValue, ExperimentRepository, ResolvedRunRuleProof,
    SnapshotValue, TransitionExperimentRun,
};
use ficant_domain::research::{ExperimentRun, RunState};
use ficant_domain::{ContentAddressed, Lineaged, VersionedDefinition};

use super::PostgresRepository;
use super::codec::{decode_definition, decode_run, decode_snapshot, encode_run};
use super::common::{
    IdempotencyOutcome, application_error, insert_lineage, lock_idempotency, map_sqlx_error,
};

#[async_trait]
impl ExperimentRepository for PostgresRepository {
    async fn create_run(
        &self,
        command: CreateExperimentRun,
    ) -> Result<ExperimentRun, ApplicationError> {
        PostgresRepository::create_experiment_run(self, command).await
    }

    async fn transition(
        &self,
        command: TransitionExperimentRun,
    ) -> Result<ExperimentRun, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let run = persist_transition(&mut transaction, &command).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(run)
    }

    async fn get_run(
        &self,
        scope: &AccessScope,
        run_id: ficant_domain::primitives::Ulid,
    ) -> Result<Option<ExperimentRun>, ApplicationError> {
        let owners = owner_strings(scope);
        let payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT payload FROM research.experiment_runs
             WHERE tenant_id = $1 AND experiment_run_id = $2
               AND owner_id::text = ANY($3::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(run_id.as_str())
        .bind(&owners)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        payload.map(|bytes| decode_run(&bytes)).transpose()
    }
}

impl PostgresRepository {
    /// Creates a run with frozen bindings and initializes its journal sequence atomically.
    ///
    /// # Errors
    ///
    /// Returns a classified application error on idempotency, lineage, or storage conflict.
    pub async fn create_experiment_run(
        &self,
        command: CreateExperimentRun,
    ) -> Result<ExperimentRun, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let value = persist_run(&mut transaction, &command).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }
}

pub(crate) async fn persist_run(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &CreateExperimentRun,
) -> Result<ExperimentRun, ApplicationError> {
    let run = command.run();
    command.scope().authorize(run.owner())?;
    validate_run_rule(transaction, run, command.proof()).await?;
    let tenant_id = run.owner().tenant_id().as_str();
    let run_id = run.id().as_str();
    let fingerprint = command.fingerprint().content_hash().as_bytes();
    let outcome = lock_idempotency(
        transaction,
        tenant_id,
        "experiment-run:create:v2",
        command.idempotency_key().as_str(),
        fingerprint,
        run_id,
    )
    .await?;
    if outcome == IdempotencyOutcome::Replay {
        return Ok(run.clone());
    }

    sqlx::query(
        "INSERT INTO research.experiment_runs
             (tenant_id, experiment_run_id, owner_id, state, revision,
              idempotency_key, fingerprint, payload)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(tenant_id)
    .bind(run_id)
    .bind(run.owner().owner_id().as_str())
    .bind(run_state(run.state()))
    .bind(i64::try_from(run.revision()).map_err(|_| {
        ficant_application::map_domain_error(ficant_domain::DomainErrorCode::InvalidValue)
    })?)
    .bind(command.idempotency_key().as_str())
    .bind(fingerprint.as_slice())
    .bind(encode_run(run))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    sqlx::query(
        "INSERT INTO research.experiment_run_revisions
             (tenant_id, experiment_run_id, revision, state, payload)
             VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(tenant_id)
    .bind(run_id)
    .bind(i64::try_from(run.revision()).map_err(|_| {
        ficant_application::map_domain_error(ficant_domain::DomainErrorCode::InvalidValue)
    })?)
    .bind(run_state(run.state()))
    .bind(encode_run(run))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    sqlx::query(
        "INSERT INTO research.run_journal_sequences (tenant_id, run_id, next_sequence)
             VALUES ($1, $2, 1)",
    )
    .bind(tenant_id)
    .bind(run_id)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    insert_lineage(transaction, tenant_id, run_id, run.lineage()).await?;
    Ok(run.clone())
}

pub(crate) async fn validate_run_rule(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run: &ExperimentRun,
    proof: &ResolvedRunRuleProof,
) -> Result<(), ApplicationError> {
    if proof.tenant_id() != run.owner().tenant_id()
        || proof.run_id() != run.id()
        || proof.snapshot_id() != run.data_snapshot().object_id()
        || run.data_snapshot().content_hash() != Some(proof.snapshot_content_hash())
        || run.rule_packs().len() != proof.bindings().len()
    {
        return Err(lineage_error());
    }
    let payload: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT payload FROM research.data_snapshots WHERE tenant_id=$1
         AND data_snapshot_id=$2 AND content_hash=$3 FOR SHARE",
    )
    .bind(proof.tenant_id().as_str())
    .bind(proof.snapshot_id().as_str())
    .bind(crate::s3::content_addressed::hash_hex(
        proof.snapshot_content_hash(),
    ))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let Some(payload) = payload else {
        return Err(lineage_error());
    };
    let SnapshotValue::Data(snapshot) = decode_snapshot(&payload)? else {
        return Err(lineage_error());
    };
    if snapshot.id() != proof.snapshot_id()
        || snapshot.content_hash() != proof.snapshot_content_hash()
        || snapshot.as_of() != proof.as_of()
        || snapshot.owner() != run.owner()
    {
        return Err(lineage_error());
    }
    for (reference, binding) in run.rule_packs().iter().zip(proof.bindings()) {
        if reference != binding.rule_pack() {
            return Err(lineage_error());
        }
        let row: Option<(
            sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
            sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
            Vec<u8>,
        )> = sqlx::query_as(
            "SELECT effective_from,effective_to,payload FROM market.market_rule_packs
             WHERE tenant_id=$1 AND rule_pack_id=$2 AND version=$3 FOR SHARE",
        )
        .bind(proof.tenant_id().as_str())
        .bind(reference.id().as_str())
        .bind(version_i64(reference.version().get())?)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
        let Some((from, to, payload)) = row else {
            return Err(lineage_error());
        };
        let DefinitionValue::MarketRulePack(rule) = decode_definition(&payload)? else {
            return Err(lineage_error());
        };
        if rule.identity() != reference.id().as_str()
            || rule.version() != reference.version().get()
            || rule.owner().tenant_id() != proof.tenant_id()
        {
            return Err(lineage_error());
        }
        let subject = proof.as_of().instant();
        if from > subject || subject >= to {
            return Err(invalid());
        }
        if from != rule.effective().from().instant()
            || to != rule.effective().to().instant()
            || rule.effective().from() != binding.effective_from()
            || rule.effective().to() != binding.effective_to()
        {
            return Err(lineage_error());
        }
    }
    Ok(())
}

pub(crate) async fn persist_transition(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &TransitionExperimentRun,
) -> Result<ExperimentRun, ApplicationError> {
    command.scope().authorize(command.target_owner())?;
    let tenant = command.target_owner().tenant_id().as_str();
    let owner = command.target_owner().owner_id().as_str();
    let run_id = command.run_id().as_str();
    let outcome = lock_idempotency(
        transaction,
        tenant,
        "experiment-run:transition:v2",
        command.idempotency_key().as_str(),
        command.fingerprint().content_hash().as_bytes(),
        run_id,
    )
    .await?;
    let result_revision = command.expected_revision().checked_add(1).ok_or_else(|| {
        application_error(
            ficant_application::ApplicationErrorCategory::VersionConflict,
            true,
        )
    })?;
    if outcome == IdempotencyOutcome::Replay {
        let payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT v.payload
             FROM research.experiment_run_revisions v
             JOIN research.experiment_runs r
               ON r.tenant_id = v.tenant_id AND r.experiment_run_id = v.experiment_run_id
             WHERE v.tenant_id = $1 AND v.experiment_run_id = $2 AND v.revision = $3
               AND r.owner_id = $4",
        )
        .bind(tenant)
        .bind(run_id)
        .bind(version_i64(result_revision)?)
        .bind(owner)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
        return payload
            .map(|bytes| decode_run(&bytes))
            .transpose()?
            .ok_or_else(storage_error);
    }
    let payload: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT payload FROM research.experiment_runs
         WHERE tenant_id = $1 AND experiment_run_id = $2 AND owner_id = $3
         FOR UPDATE",
    )
    .bind(tenant)
    .bind(run_id)
    .bind(owner)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let current = payload
        .map(|bytes| decode_run(&bytes))
        .transpose()?
        .ok_or_else(forbidden)?;
    let next = current
        .transition(command.next_state(), command.expected_revision())
        .map_err(ficant_application::map_domain_error)?;
    let updated = sqlx::query(
        "UPDATE research.experiment_runs
         SET state = $5, revision = $6, payload = $7
         WHERE tenant_id = $1 AND experiment_run_id = $2 AND owner_id = $3 AND revision = $4",
    )
    .bind(tenant)
    .bind(run_id)
    .bind(owner)
    .bind(version_i64(command.expected_revision())?)
    .bind(run_state(next.state()))
    .bind(version_i64(next.revision())?)
    .bind(encode_run(&next))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if updated.rows_affected() != 1 {
        return Err(application_error(
            ficant_application::ApplicationErrorCategory::ConcurrencyConflict,
            true,
        ));
    }
    sqlx::query(
        "INSERT INTO research.experiment_run_revisions
         (tenant_id, experiment_run_id, revision, state, payload)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(tenant)
    .bind(run_id)
    .bind(version_i64(next.revision())?)
    .bind(run_state(next.state()))
    .bind(encode_run(&next))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    Ok(next)
}

const fn run_state(value: RunState) -> &'static str {
    match value {
        RunState::Created => "CREATED",
        RunState::Running => "RUNNING",
        RunState::Succeeded => "SUCCEEDED",
        RunState::Failed => "FAILED",
        RunState::Cancelled => "CANCELLED",
    }
}

fn owner_strings(scope: &AccessScope) -> Vec<String> {
    scope
        .allowed_owner_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

fn version_i64(value: u64) -> Result<i64, ApplicationError> {
    i64::try_from(value).map_err(|_| {
        ficant_application::map_domain_error(ficant_domain::DomainErrorCode::InvalidValue)
    })
}

fn forbidden() -> ApplicationError {
    application_error(
        ficant_application::ApplicationErrorCategory::Forbidden,
        false,
    )
}

fn storage_error() -> ApplicationError {
    application_error(
        ficant_application::ApplicationErrorCategory::StorageUnavailable,
        false,
    )
}

fn invalid() -> ApplicationError {
    application_error(
        ficant_application::ApplicationErrorCategory::ValidationFailed,
        false,
    )
}

fn lineage_error() -> ApplicationError {
    application_error(
        ficant_application::ApplicationErrorCategory::LineageIncomplete,
        false,
    )
}
