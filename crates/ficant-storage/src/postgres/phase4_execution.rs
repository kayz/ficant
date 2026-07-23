use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, BeginNode, CompleteNode, EnqueueNode, ExternalInputArtifactBinding, FailNode,
    GraphNodeEvent, NodeBeginResult, NodeFailureResult, NodeJournalEvidence, NodeSuccessResult,
    Phase4ExecutionRepository, StoredExecutionIdentity, stable_node_artifact_id,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    Artifact, ArtifactKind, JournalEventType, ResearchGraph, RunJournal, RunJournalInput, RunState,
};
use ficant_domain::{ContentAddressed, Lineaged};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};

use super::PostgresRepository;
use super::codec::{
    decode_artifact, decode_execution_identity, decode_research_graph, decode_run, encode_artifact,
    encode_execution_identity, encode_journal, encode_research_graph, encode_run,
};
use super::common::{application_error, insert_lineage, map_sqlx_error, publish_blob_reference};

#[allow(clippy::too_many_lines)]
#[async_trait]
impl Phase4ExecutionRepository for PostgresRepository {
    async fn publish_graph(
        &self,
        scope: &AccessScope,
        graph: ResearchGraph,
    ) -> Result<ResearchGraph, ApplicationError> {
        scope.authorize(graph.owner())?;
        let payload = encode_research_graph(&graph);
        let inserted = sqlx::query(
            "INSERT INTO research.research_graphs
             (tenant_id, graph_id, version, owner_id, graph_digest, payload)
             VALUES ($1,$2,$3,$4,$5,$6)
             ON CONFLICT (tenant_id, graph_id, version) DO NOTHING",
        )
        .bind(graph.owner().tenant_id().as_str())
        .bind(graph.graph_id().as_str())
        .bind(version_i64(graph.version().get())?)
        .bind(graph.owner().owner_id().as_str())
        .bind(hash_hex(graph.digest()))
        .bind(&payload)
        .execute(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let stored: Option<(String, String, Vec<u8>)> = sqlx::query_as(
            "SELECT owner_id::text,graph_digest::text,payload
             FROM research.research_graphs
             WHERE tenant_id=$1 AND graph_id=$2 AND version=$3",
        )
        .bind(graph.owner().tenant_id().as_str())
        .bind(graph.graph_id().as_str())
        .bind(version_i64(graph.version().get())?)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let Some((owner, digest, stored_payload)) = stored else {
            return Err(storage());
        };
        if owner != graph.owner().owner_id().as_str()
            || digest != hash_hex(graph.digest())
            || stored_payload != payload
        {
            return Err(immutable());
        }
        let result = decode_research_graph(&stored_payload)?;
        if inserted.rows_affected() == 0 && result != graph {
            return Err(immutable());
        }
        Ok(result)
    }

    async fn load_graph(
        &self,
        scope: &AccessScope,
        graph_id: &Ulid,
        version: Version,
    ) -> Result<Option<ResearchGraph>, ApplicationError> {
        let owners = allowed_owners(scope);
        let payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT payload FROM research.research_graphs
             WHERE tenant_id=$1 AND graph_id=$2 AND version=$3
               AND owner_id::text = ANY($4::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(graph_id.as_str())
        .bind(version_i64(version.get())?)
        .bind(owners)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        payload
            .map(|value| decode_research_graph(&value))
            .transpose()
    }

    async fn publish_execution_identity(
        &self,
        scope: &AccessScope,
        mut value: StoredExecutionIdentity,
    ) -> Result<StoredExecutionIdentity, ApplicationError> {
        scope.authorize(&value.owner)?;
        value
            .external_input_artifacts
            .sort_by(|left, right| left.input_id.cmp(&right.input_id));
        let identity = &value.identity;
        let reproducibility = identity.reproducibility();
        if value.external_input_artifacts.len() != reproducibility.external_inputs().len()
            || !value
                .external_input_artifacts
                .iter()
                .zip(reproducibility.external_inputs())
                .all(|(artifact, input)| {
                    artifact.input_id == input.input_id()
                        && artifact.content_hash == *input.content_hash()
                })
        {
            return Err(lineage());
        }
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let graph_payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT payload FROM research.research_graphs
             WHERE tenant_id=$1 AND graph_id=$2 AND version=$3 AND owner_id=$4
               AND graph_digest=$5 FOR SHARE",
        )
        .bind(value.owner.tenant_id().as_str())
        .bind(value.graph_id.as_str())
        .bind(version_i64(value.graph_version.get())?)
        .bind(value.owner.owner_id().as_str())
        .bind(hash_hex(reproducibility.graph_digest()))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        let graph = graph_payload
            .map(|payload| decode_research_graph(&payload))
            .transpose()?
            .ok_or_else(lineage)?;
        validate_run_identity(&mut transaction, &value, &graph).await?;
        let payload = encode_execution_identity(identity);
        let inserted = sqlx::query(
            "INSERT INTO research.execution_identities
             (tenant_id,run_id,owner_id,graph_id,graph_version,graph_digest,
              reproducibility_digest,execution_identity_digest,payload)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
             ON CONFLICT (tenant_id,run_id) DO NOTHING",
        )
        .bind(value.owner.tenant_id().as_str())
        .bind(identity.run_id().as_str())
        .bind(value.owner.owner_id().as_str())
        .bind(value.graph_id.as_str())
        .bind(version_i64(value.graph_version.get())?)
        .bind(hash_hex(reproducibility.graph_digest()))
        .bind(hash_hex(identity.reproducibility_digest()))
        .bind(hash_hex(identity.digest()))
        .bind(&payload)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if inserted.rows_affected() == 0 {
            let existing: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT payload FROM research.execution_identities
                 WHERE tenant_id=$1 AND run_id=$2 FOR SHARE",
            )
            .bind(value.owner.tenant_id().as_str())
            .bind(identity.run_id().as_str())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
            if existing.as_deref() != Some(payload.as_slice()) {
                return Err(immutable());
            }
            let persisted_bindings: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT input_id,artifact_id::text,content_hash::text
                 FROM research.execution_external_inputs
                 WHERE tenant_id=$1 AND run_id=$2 ORDER BY input_id",
            )
            .bind(value.owner.tenant_id().as_str())
            .bind(identity.run_id().as_str())
            .fetch_all(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
            let requested_bindings = value
                .external_input_artifacts
                .iter()
                .map(|binding| {
                    (
                        binding.input_id.clone(),
                        binding.artifact_id.as_str().to_owned(),
                        hash_hex(&binding.content_hash),
                    )
                })
                .collect::<Vec<_>>();
            if persisted_bindings != requested_bindings {
                return Err(immutable());
            }
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(value);
        }
        for (artifact_binding, input) in value
            .external_input_artifacts
            .iter()
            .zip(reproducibility.external_inputs())
        {
            let persisted: Option<String> = sqlx::query_scalar(
                "SELECT content_hash::text FROM research.artifacts
                 WHERE tenant_id=$1 AND artifact_id=$2 FOR SHARE",
            )
            .bind(value.owner.tenant_id().as_str())
            .bind(artifact_binding.artifact_id.as_str())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
            if persisted.as_deref() != Some(hash_hex(input.content_hash()).as_str()) {
                return Err(lineage());
            }
            sqlx::query(
                "INSERT INTO research.execution_external_inputs
                 (tenant_id,run_id,input_id,type_id,type_version,schema_hash,artifact_id,content_hash)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
            )
            .bind(value.owner.tenant_id().as_str())
            .bind(identity.run_id().as_str())
            .bind(input.input_id())
            .bind(input.value_type().type_id())
            .bind(version_i64(input.value_type().type_version().get())?)
            .bind(hash_hex(input.value_type().schema_hash()))
            .bind(artifact_binding.artifact_id.as_str())
            .bind(hash_hex(input.content_hash()))
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        for binding in reproducibility.rule_pack_bindings() {
            let rule_pack_id =
                Ulid::new(&binding.rule_pack_id).map_err(ficant_application::map_domain_error)?;
            let persisted: Option<String> = sqlx::query_scalar(
                "SELECT content_hash::text FROM market.market_rule_packs
                 WHERE tenant_id=$1 AND rule_pack_id=$2 AND version=$3 FOR SHARE",
            )
            .bind(value.owner.tenant_id().as_str())
            .bind(rule_pack_id.as_str())
            .bind(version_i64(binding.version.get())?)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
            if persisted.as_deref() != Some(hash_hex(&binding.content_hash).as_str()) {
                return Err(lineage());
            }
            sqlx::query(
                "INSERT INTO research.execution_rule_packs
                 (tenant_id,run_id,rule_pack_id,version,content_hash)
                 VALUES ($1,$2,$3,$4,$5)",
            )
            .bind(value.owner.tenant_id().as_str())
            .bind(identity.run_id().as_str())
            .bind(rule_pack_id.as_str())
            .bind(version_i64(binding.version.get())?)
            .bind(hash_hex(&binding.content_hash))
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        for binding in reproducibility.node_implementations() {
            sqlx::query(
                "INSERT INTO research.execution_node_implementations
                 (tenant_id,run_id,node_id,implementation_digest)
                 VALUES ($1,$2,$3,$4)",
            )
            .bind(value.owner.tenant_id().as_str())
            .bind(identity.run_id().as_str())
            .bind(binding.node_id.as_str())
            .bind(hash_hex(&binding.implementation_digest))
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value)
    }

    async fn load_execution_identity(
        &self,
        scope: &AccessScope,
        run_id: &Ulid,
    ) -> Result<Option<StoredExecutionIdentity>, ApplicationError> {
        let owners = allowed_owners(scope);
        let row: Option<(String, String, i64, Vec<u8>, Vec<u8>)> = sqlx::query_as(
            "SELECT i.owner_id::text,i.graph_id::text,i.graph_version,i.payload,g.payload
             FROM research.execution_identities i
             JOIN research.research_graphs g
               ON g.tenant_id=i.tenant_id AND g.graph_id=i.graph_id AND g.version=i.graph_version
             WHERE i.tenant_id=$1 AND i.run_id=$2 AND i.owner_id::text=ANY($3::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(run_id.as_str())
        .bind(owners)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let Some((owner_id, graph_id, graph_version, identity_payload, graph_payload)) = row else {
            return Ok(None);
        };
        let graph = decode_research_graph(&graph_payload)?;
        let identity = decode_execution_identity(&identity_payload, &graph)?;
        let bindings: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT input_id,artifact_id::text,content_hash::text
             FROM research.execution_external_inputs
             WHERE tenant_id=$1 AND run_id=$2 ORDER BY input_id",
        )
        .bind(scope.tenant_id().as_str())
        .bind(run_id.as_str())
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        Ok(Some(StoredExecutionIdentity {
            owner: OwnerRef::new(scope.tenant_id().clone(), parse_id(&owner_id)?),
            graph_id: parse_id(&graph_id)?,
            graph_version: Version::new(u64::try_from(graph_version).map_err(|_| invalid())?)
                .map_err(ficant_application::map_domain_error)?,
            identity,
            external_input_artifacts: bindings
                .into_iter()
                .map(|(input_id, artifact_id, content_hash)| {
                    Ok(ExternalInputArtifactBinding {
                        input_id,
                        artifact_id: parse_id(&artifact_id)?,
                        content_hash: parse_hash(&content_hash)?,
                    })
                })
                .collect::<Result<Vec<_>, ApplicationError>>()?,
        }))
    }

    async fn enqueue_node(&self, command: EnqueueNode) -> Result<(), ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        persist_enqueue(&mut transaction, &command).await?;
        transaction.commit().await.map_err(map_sqlx_error)
    }

    async fn begin_node(&self, command: BeginNode) -> Result<NodeBeginResult, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let existing: Option<(String, i64, String)> = sqlx::query_as(
            "SELECT state,started_journal_sequence,started_journal_hash::text
             FROM research.node_executions
             WHERE tenant_id=$1 AND run_id=$2 AND node_id=$3 AND attempt=$4 FOR UPDATE",
        )
        .bind(command.fence.tenant_id.as_str())
        .bind(command.fence.run_id.as_str())
        .bind(command.fence.node_id.as_str())
        .bind(version_i64(command.fence.attempt)?)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if let Some((state, sequence, hash)) = existing {
            if state == "STARTED" {
                validate_active_fence(&mut transaction, &command.fence).await?;
                transaction.commit().await.map_err(map_sqlx_error)?;
                return Ok(NodeBeginResult {
                    evidence: evidence(sequence, &hash)?,
                    replayed: true,
                });
            }
            return Err(state_conflict());
        }
        validate_active_fence(&mut transaction, &command.fence).await?;
        let started = append_node_event(
            &mut transaction,
            &command.fence,
            command.started_event_id,
            JournalEventType::NodeStarted,
            None,
        )
        .await?;
        sqlx::query(
            "INSERT INTO research.node_executions
             (tenant_id,run_id,node_id,attempt,task_id,execution_identity_digest,state,
              started_journal_sequence,started_journal_hash)
             VALUES ($1,$2,$3,$4,$5,$6,'STARTED',$7,$8)",
        )
        .bind(command.fence.tenant_id.as_str())
        .bind(command.fence.run_id.as_str())
        .bind(command.fence.node_id.as_str())
        .bind(version_i64(command.fence.attempt)?)
        .bind(command.fence.task_id.as_str())
        .bind(hash_hex(&command.fence.execution_identity_digest))
        .bind(version_i64(started.sequence)?)
        .bind(hash_hex(&started.event_hash))
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(NodeBeginResult {
            evidence: started,
            replayed: false,
        })
    }

    async fn complete_node(
        &self,
        command: CompleteNode,
    ) -> Result<NodeSuccessResult, ApplicationError> {
        let manifest_hash = ContentHash::digest(&command.output_manifest);
        if command.output_manifest.is_empty() {
            return Err(invalid());
        }
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        if let Some(replayed) = replay_success(&mut transaction, &command, &manifest_hash).await? {
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(replayed);
        }
        validate_active_fence(&mut transaction, &command.fence).await?;
        let planned_id: String = sqlx::query_scalar(
            "SELECT planned_artifact_id::text FROM research.execution_tasks
             WHERE tenant_id=$1 AND task_id=$2 FOR UPDATE",
        )
        .bind(command.fence.tenant_id.as_str())
        .bind(command.fence.task_id.as_str())
        .fetch_one(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if command.artifact.id().as_str() != planned_id {
            return Err(immutable());
        }
        publish_blob_reference(
            &mut transaction,
            command.artifact.owner().tenant_id().as_str(),
            command.artifact.content_hash(),
            command.artifact.blob_size(),
        )
        .await?;
        validate_planned_artifact(&mut transaction, &command).await?;
        persist_or_reuse_artifact(&mut transaction, &command.artifact).await?;
        let succeeded = append_node_event(
            &mut transaction,
            &command.fence,
            command.succeeded_event_id,
            JournalEventType::NodeSucceeded,
            Some(command.artifact.content_hash()),
        )
        .await?;
        let checkpointed = append_node_event(
            &mut transaction,
            &command.fence,
            command.checkpoint_event_id,
            JournalEventType::NodeCheckpointed,
            Some(command.artifact.content_hash()),
        )
        .await?;
        let updated = sqlx::query(
            "UPDATE research.node_executions
             SET state='SUCCEEDED',artifact_id=$5,output_manifest_hash=$6,output_manifest=$7,
                 terminal_journal_sequence=$8,terminal_journal_hash=$9,
                 checkpoint_journal_sequence=$10,checkpoint_journal_hash=$11,
                 completed_at=CURRENT_TIMESTAMP
             WHERE tenant_id=$1 AND run_id=$2 AND node_id=$3 AND attempt=$4 AND state='STARTED'",
        )
        .bind(command.fence.tenant_id.as_str())
        .bind(command.fence.run_id.as_str())
        .bind(command.fence.node_id.as_str())
        .bind(version_i64(command.fence.attempt)?)
        .bind(command.artifact.id().as_str())
        .bind(hash_hex(&manifest_hash))
        .bind(&command.output_manifest)
        .bind(version_i64(succeeded.sequence)?)
        .bind(hash_hex(&succeeded.event_hash))
        .bind(version_i64(checkpointed.sequence)?)
        .bind(hash_hex(&checkpointed.event_hash))
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if updated.rows_affected() != 1 {
            return Err(concurrency());
        }
        let completed = sqlx::query(
            "UPDATE research.execution_tasks
             SET state='COMPLETED',completion_hash=$5,updated_at=CURRENT_TIMESTAMP
             WHERE tenant_id=$1 AND task_id=$2 AND state='LEASED'
               AND lease_owner=$3 AND lease_id=$4 AND claim_count=$6
               AND lease_expires_at>CURRENT_TIMESTAMP AND planned_artifact_id=$7",
        )
        .bind(command.fence.tenant_id.as_str())
        .bind(command.fence.task_id.as_str())
        .bind(command.fence.worker_id.as_str())
        .bind(command.fence.lease_id.as_str())
        .bind(hash_hex(command.artifact.content_hash()))
        .bind(version_i64(command.fence.attempt)?)
        .bind(command.artifact.id().as_str())
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if completed.rows_affected() != 1 {
            return Err(concurrency());
        }
        if let Some(next) = command.next_task.as_ref() {
            persist_enqueue(&mut transaction, next).await?;
        } else {
            append_run_terminal_event(
                &mut transaction,
                &command.fence,
                JournalEventType::RunSucceeded,
            )
            .await?;
            transition_run_terminal(
                &mut transaction,
                &command.fence.tenant_id,
                &command.fence.run_id,
                RunState::Succeeded,
            )
            .await?;
        }
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(NodeSuccessResult {
            artifact: command.artifact,
            succeeded,
            checkpointed,
            replayed: false,
        })
    }

    async fn fail_node(&self, command: FailNode) -> Result<NodeFailureResult, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let existing: Option<(String, Option<String>, Option<i64>, Option<String>)> =
            sqlx::query_as(
                "SELECT state,failure_hash::text,terminal_journal_sequence,
                        terminal_journal_hash::text
                 FROM research.node_executions
                 WHERE tenant_id=$1 AND run_id=$2 AND node_id=$3 AND attempt=$4 FOR UPDATE",
            )
            .bind(command.fence.tenant_id.as_str())
            .bind(command.fence.run_id.as_str())
            .bind(command.fence.node_id.as_str())
            .bind(version_i64(command.fence.attempt)?)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        if let Some((state, failure, sequence, hash)) = existing.as_ref() {
            if state == "FAILED"
                && failure.as_deref() == Some(hash_hex(&command.failure_hash).as_str())
            {
                let result = NodeFailureResult {
                    failed: evidence(
                        sequence.ok_or_else(storage)?,
                        hash.as_deref().ok_or_else(storage)?,
                    )?,
                    replayed: true,
                };
                transaction.commit().await.map_err(map_sqlx_error)?;
                return Ok(result);
            }
            if state != "STARTED" {
                return Err(immutable());
            }
        } else {
            return Err(state_conflict());
        }
        validate_active_fence(&mut transaction, &command.fence).await?;
        let failed = append_node_event(
            &mut transaction,
            &command.fence,
            command.failed_event_id,
            JournalEventType::NodeFailed,
            Some(&command.failure_hash),
        )
        .await?;
        sqlx::query(
            "UPDATE research.node_executions
             SET state='FAILED',failure_hash=$5,terminal_journal_sequence=$6,
                 terminal_journal_hash=$7,completed_at=CURRENT_TIMESTAMP
             WHERE tenant_id=$1 AND run_id=$2 AND node_id=$3 AND attempt=$4 AND state='STARTED'",
        )
        .bind(command.fence.tenant_id.as_str())
        .bind(command.fence.run_id.as_str())
        .bind(command.fence.node_id.as_str())
        .bind(version_i64(command.fence.attempt)?)
        .bind(hash_hex(&command.failure_hash))
        .bind(version_i64(failed.sequence)?)
        .bind(hash_hex(&failed.event_hash))
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        let changed = sqlx::query(
            "UPDATE research.execution_tasks
             SET state='FAILED',failure_hash=$5,updated_at=CURRENT_TIMESTAMP
             WHERE tenant_id=$1 AND task_id=$2 AND state='LEASED'
               AND lease_owner=$3 AND lease_id=$4 AND claim_count=$6
               AND lease_expires_at>CURRENT_TIMESTAMP",
        )
        .bind(command.fence.tenant_id.as_str())
        .bind(command.fence.task_id.as_str())
        .bind(command.fence.worker_id.as_str())
        .bind(command.fence.lease_id.as_str())
        .bind(hash_hex(&command.failure_hash))
        .bind(version_i64(command.fence.attempt)?)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if changed.rows_affected() != 1 {
            return Err(concurrency());
        }
        append_run_terminal_event(
            &mut transaction,
            &command.fence,
            JournalEventType::RunFailed,
        )
        .await?;
        transition_run_terminal(
            &mut transaction,
            &command.fence.tenant_id,
            &command.fence.run_id,
            RunState::Failed,
        )
        .await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(NodeFailureResult {
            failed,
            replayed: false,
        })
    }
}

async fn validate_run_identity(
    transaction: &mut Transaction<'_, Postgres>,
    value: &StoredExecutionIdentity,
    graph: &ResearchGraph,
) -> Result<(), ApplicationError> {
    if graph.digest() != value.identity.reproducibility().graph_digest()
        || graph.graph_id() != &value.graph_id
        || graph.version() != value.graph_version
        || graph.owner() != &value.owner
    {
        return Err(lineage());
    }
    let payload: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT payload FROM research.experiment_runs
         WHERE tenant_id=$1 AND experiment_run_id=$2 AND owner_id=$3 FOR SHARE",
    )
    .bind(value.owner.tenant_id().as_str())
    .bind(value.identity.run_id().as_str())
    .bind(value.owner.owner_id().as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let run = payload
        .map(|payload| decode_run(&payload))
        .transpose()?
        .ok_or_else(lineage)?;
    let reproducibility = value.identity.reproducibility();
    if run.data_snapshot().content_hash() != Some(reproducibility.data_snapshot_hash())
        || run.universe_snapshot().content_hash() != Some(reproducibility.universe_snapshot_hash())
        || run.parameters_hash() != reproducibility.parameters_hash()
        || run.runtime_image_digest() != reproducibility.runtime_image_digest()
        || run.seed() != reproducibility.seed()
        || run.rule_packs().len() != reproducibility.rule_pack_bindings().len()
        || !run.rule_packs().iter().all(|run_rule| {
            reproducibility
                .rule_pack_bindings()
                .iter()
                .any(|identity_rule| {
                    run_rule.id().as_str() == identity_rule.rule_pack_id
                        && run_rule.version() == identity_rule.version
                })
        })
    {
        return Err(lineage());
    }
    Ok(())
}

async fn persist_enqueue(
    transaction: &mut Transaction<'_, Postgres>,
    command: &EnqueueNode,
) -> Result<(), ApplicationError> {
    if command.task_key.is_empty()
        || command.task_key.trim() != command.task_key
        || command.task_key.len() > 256
    {
        return Err(invalid());
    }
    let identity: Option<(String, String)> = sqlx::query_as(
        "SELECT graph_digest::text,reproducibility_digest::text
         FROM research.execution_identities
         WHERE tenant_id=$1 AND run_id=$2 AND execution_identity_digest=$3 FOR SHARE",
    )
    .bind(command.tenant_id.as_str())
    .bind(command.run_id.as_str())
    .bind(hash_hex(&command.execution_identity_digest))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let Some((graph_digest, reproducibility_digest)) = identity else {
        return Err(lineage());
    };
    if graph_digest != hash_hex(&command.graph_digest) {
        return Err(lineage());
    }
    let stable = stable_node_artifact_id(&parse_hash(&reproducibility_digest)?, &command.node_id);
    if stable != command.planned_artifact_id {
        return Err(immutable());
    }
    let inserted = sqlx::query(
        "INSERT INTO research.execution_tasks
         (tenant_id,task_id,run_id,node_id,graph_digest,execution_identity_digest,
          planned_artifact_id,task_key,state)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'PENDING')
         ON CONFLICT (tenant_id,task_key) DO NOTHING",
    )
    .bind(command.tenant_id.as_str())
    .bind(command.task_id.as_str())
    .bind(command.run_id.as_str())
    .bind(command.node_id.as_str())
    .bind(hash_hex(&command.graph_digest))
    .bind(hash_hex(&command.execution_identity_digest))
    .bind(command.planned_artifact_id.as_str())
    .bind(&command.task_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if inserted.rows_affected() == 0 {
        let exact: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM research.execution_tasks
                 WHERE tenant_id=$1 AND task_key=$2 AND task_id=$3 AND run_id=$4 AND node_id=$5
                   AND graph_digest=$6 AND execution_identity_digest=$7
                   AND planned_artifact_id=$8)",
        )
        .bind(command.tenant_id.as_str())
        .bind(&command.task_key)
        .bind(command.task_id.as_str())
        .bind(command.run_id.as_str())
        .bind(command.node_id.as_str())
        .bind(hash_hex(&command.graph_digest))
        .bind(hash_hex(&command.execution_identity_digest))
        .bind(command.planned_artifact_id.as_str())
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
        if !exact {
            return Err(immutable());
        }
    }
    Ok(())
}

async fn validate_active_fence(
    transaction: &mut Transaction<'_, Postgres>,
    fence: &ficant_application::ports::NodeLeaseFence,
) -> Result<(), ApplicationError> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM research.execution_tasks
             WHERE tenant_id=$1 AND task_id=$2 AND run_id=$3 AND node_id=$4
               AND state='LEASED' AND lease_owner=$5 AND lease_id=$6 AND claim_count=$7
               AND execution_identity_digest=$8 AND lease_expires_at>CURRENT_TIMESTAMP)",
    )
    .bind(fence.tenant_id.as_str())
    .bind(fence.task_id.as_str())
    .bind(fence.run_id.as_str())
    .bind(fence.node_id.as_str())
    .bind(fence.worker_id.as_str())
    .bind(fence.lease_id.as_str())
    .bind(version_i64(fence.attempt)?)
    .bind(hash_hex(&fence.execution_identity_digest))
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if !valid {
        return Err(concurrency());
    }
    Ok(())
}

async fn append_node_event(
    transaction: &mut Transaction<'_, Postgres>,
    fence: &ficant_application::ports::NodeLeaseFence,
    event_id: Ulid,
    event_type: JournalEventType,
    evidence: Option<&ContentHash>,
) -> Result<NodeJournalEvidence, ApplicationError> {
    let attempt = u32::try_from(fence.attempt).map_err(|_| invalid())?;
    let payload = match evidence {
        Some(hash) => GraphNodeEvent::evidenced(fence.node_id.clone(), attempt, hash.clone()),
        None => GraphNodeEvent::started(fence.node_id.clone(), attempt),
    }
    .map_err(ficant_application::map_domain_error)?
    .encode();
    append_journal_event(
        transaction,
        fence,
        event_id,
        event_type,
        GraphNodeEvent::payload_type(),
        GraphNodeEvent::payload_schema(),
        payload,
    )
    .await
}

async fn append_run_terminal_event(
    transaction: &mut Transaction<'_, Postgres>,
    fence: &ficant_application::ports::NodeLeaseFence,
    event_type: JournalEventType,
) -> Result<NodeJournalEvidence, ApplicationError> {
    let mut domain = b"ficant/phase4-run-terminal-event/v1".to_vec();
    domain.extend_from_slice(&fence.execution_identity_digest.as_bytes()[..]);
    domain.push(match event_type {
        JournalEventType::RunSucceeded => 1,
        JournalEventType::RunFailed => 2,
        _ => return Err(invalid()),
    });
    let event_id = stable_node_artifact_id(&ContentHash::digest(&domain), &fence.run_id);
    append_journal_event(
        transaction,
        fence,
        event_id,
        event_type,
        "ficant.run-terminal-event",
        "ficant.run-terminal-event.v1",
        fence.run_id.as_str().as_bytes().to_vec(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn append_journal_event(
    transaction: &mut Transaction<'_, Postgres>,
    fence: &ficant_application::ports::NodeLeaseFence,
    event_id: Ulid,
    event_type: JournalEventType,
    payload_type: &str,
    payload_schema: &str,
    payload: Vec<u8>,
) -> Result<NodeJournalEvidence, ApplicationError> {
    type JournalHead = (i64, Option<String>, Option<DateTime<Utc>>, DateTime<Utc>);
    let row: Option<JournalHead> = sqlx::query_as(
        "SELECT s.next_sequence,
                (SELECT event_hash::text FROM research.run_journal
                 WHERE tenant_id=s.tenant_id AND run_id=s.run_id
                 ORDER BY sequence DESC LIMIT 1),
                (SELECT occurred_at FROM research.run_journal
                 WHERE tenant_id=s.tenant_id AND run_id=s.run_id
                 ORDER BY sequence DESC LIMIT 1),
                CURRENT_TIMESTAMP
         FROM research.run_journal_sequences s
         WHERE tenant_id=$1 AND run_id=$2 FOR UPDATE",
    )
    .bind(fence.tenant_id.as_str())
    .bind(fence.run_id.as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let (sequence, previous, previous_occurred, database_now) = row.ok_or_else(lineage)?;
    let occurred = previous_occurred.map_or(database_now, |value| value.max(database_now));
    let input = RunJournalInput {
        journal_event_id: event_id,
        run_id: fence.run_id.clone(),
        sequence: u64::try_from(sequence).map_err(|_| storage())?,
        event_type,
        occurred_at: MarketTime::new(occurred, "UTC", occurred.date_naive())
            .map_err(ficant_application::map_domain_error)?,
        payload_type: payload_type.to_owned(),
        payload_schema: payload_schema.to_owned(),
        payload,
        prev_hash: previous.as_deref().map(parse_hash).transpose()?,
    };
    let hash = input
        .canonical_hash()
        .map_err(ficant_application::map_domain_error)?;
    let event = RunJournal::new(input, &hash).map_err(ficant_application::map_domain_error)?;
    sqlx::query(
        "INSERT INTO research.run_journal
         (tenant_id,run_id,sequence,journal_event_id,event_type,occurred_at,
          prev_hash,event_hash,idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(fence.tenant_id.as_str())
    .bind(fence.run_id.as_str())
    .bind(sequence)
    .bind(event.id().as_str())
    .bind(event_type_sql(event_type))
    .bind(occurred)
    .bind(previous)
    .bind(hash_hex(&hash))
    .bind(format!(
        "phase4/{}/{}/{}/{}",
        fence.run_id,
        fence.node_id,
        fence.attempt,
        event_type_sql(event_type)
    ))
    .bind(ContentHash::digest(event.payload()).as_bytes().as_slice())
    .bind(encode_journal(&event))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    sqlx::query(
        "UPDATE research.run_journal_sequences SET next_sequence=next_sequence+1
         WHERE tenant_id=$1 AND run_id=$2 AND next_sequence=$3",
    )
    .bind(fence.tenant_id.as_str())
    .bind(fence.run_id.as_str())
    .bind(sequence)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    Ok(NodeJournalEvidence {
        sequence: u64::try_from(sequence).map_err(|_| storage())?,
        event_hash: hash,
    })
}

async fn validate_planned_artifact(
    transaction: &mut Transaction<'_, Postgres>,
    command: &CompleteNode,
) -> Result<(), ApplicationError> {
    if command.artifact.kind() != ArtifactKind::Generic
        || command.artifact.owner().tenant_id() != &command.fence.tenant_id
        || command
            .artifact
            .lineage()
            .iter()
            .any(|reference| reference.object_id() == &command.fence.run_id)
    {
        return Err(lineage());
    }
    let reproducibility: String = sqlx::query_scalar(
        "SELECT reproducibility_digest::text FROM research.execution_identities
         WHERE tenant_id=$1 AND run_id=$2 AND execution_identity_digest=$3 FOR SHARE",
    )
    .bind(command.fence.tenant_id.as_str())
    .bind(command.fence.run_id.as_str())
    .bind(hash_hex(&command.fence.execution_identity_digest))
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let expected = stable_node_artifact_id(&parse_hash(&reproducibility)?, &command.fence.node_id);
    if command.artifact.id() != &expected {
        return Err(immutable());
    }
    let blob: Option<i64> = sqlx::query_scalar(
        "SELECT blob_size FROM storage.blobs
         WHERE tenant_id=$1 AND content_hash=$2 FOR SHARE",
    )
    .bind(command.fence.tenant_id.as_str())
    .bind(hash_hex(command.artifact.content_hash()))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if blob != Some(i64::try_from(command.artifact.blob_size()).map_err(|_| invalid())?) {
        return Err(lineage());
    }
    Ok(())
}

async fn persist_or_reuse_artifact(
    transaction: &mut Transaction<'_, Postgres>,
    artifact: &Artifact,
) -> Result<(), ApplicationError> {
    let by_id: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT payload FROM research.artifacts
         WHERE tenant_id=$1 AND artifact_id=$2 FOR UPDATE",
    )
    .bind(artifact.owner().tenant_id().as_str())
    .bind(artifact.id().as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if let Some(payload) = by_id {
        if decode_artifact(&payload)? != *artifact {
            return Err(immutable());
        }
        return Ok(());
    }
    let idempotency = format!("phase4/native-node/{}", artifact.id());
    let payload = encode_artifact(artifact);
    sqlx::query(
        "INSERT INTO research.artifacts
         (tenant_id,artifact_id,owner_id,kind,media_type,content_hash,blob_size,
          idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,'GENERIC',$4,$5,$6,$7,$8,$9)",
    )
    .bind(artifact.owner().tenant_id().as_str())
    .bind(artifact.id().as_str())
    .bind(artifact.owner().owner_id().as_str())
    .bind(artifact.media_type())
    .bind(hash_hex(artifact.content_hash()))
    .bind(i64::try_from(artifact.blob_size()).map_err(|_| invalid())?)
    .bind(idempotency)
    .bind(ContentHash::digest(&payload).as_bytes().as_slice())
    .bind(payload)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    insert_lineage(
        transaction,
        artifact.owner().tenant_id().as_str(),
        artifact.id().as_str(),
        artifact.lineage(),
    )
    .await
}

async fn replay_success(
    transaction: &mut Transaction<'_, Postgres>,
    command: &CompleteNode,
    manifest_hash: &ContentHash,
) -> Result<Option<NodeSuccessResult>, ApplicationError> {
    type ReplayRow = (
        String,
        Option<String>,
        Option<String>,
        Option<Vec<u8>>,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<String>,
    );
    let row: Option<ReplayRow> = sqlx::query_as(
        "SELECT state,artifact_id::text,output_manifest_hash::text,output_manifest,
                    terminal_journal_sequence,terminal_journal_hash::text,
                    checkpoint_journal_sequence,checkpoint_journal_hash::text
             FROM research.node_executions
             WHERE tenant_id=$1 AND run_id=$2 AND node_id=$3 AND attempt=$4 FOR UPDATE",
    )
    .bind(command.fence.tenant_id.as_str())
    .bind(command.fence.run_id.as_str())
    .bind(command.fence.node_id.as_str())
    .bind(version_i64(command.fence.attempt)?)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let Some((
        state,
        artifact_id,
        stored_manifest_hash,
        manifest,
        succeeded_sequence,
        succeeded_hash,
        checkpoint_sequence,
        checkpoint_hash,
    )) = row
    else {
        return Err(state_conflict());
    };
    if state == "STARTED" {
        return Ok(None);
    }
    if state != "SUCCEEDED"
        || artifact_id.as_deref() != Some(command.artifact.id().as_str())
        || stored_manifest_hash.as_deref() != Some(hash_hex(manifest_hash).as_str())
        || manifest.as_deref() != Some(command.output_manifest.as_slice())
    {
        return Err(immutable());
    }
    let payload: Vec<u8> = sqlx::query_scalar(
        "SELECT payload FROM research.artifacts WHERE tenant_id=$1 AND artifact_id=$2",
    )
    .bind(command.fence.tenant_id.as_str())
    .bind(command.artifact.id().as_str())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let artifact = decode_artifact(&payload)?;
    if artifact != command.artifact {
        return Err(immutable());
    }
    Ok(Some(NodeSuccessResult {
        artifact,
        succeeded: evidence(
            succeeded_sequence.ok_or_else(storage)?,
            succeeded_hash.as_deref().ok_or_else(storage)?,
        )?,
        checkpointed: evidence(
            checkpoint_sequence.ok_or_else(storage)?,
            checkpoint_hash.as_deref().ok_or_else(storage)?,
        )?,
        replayed: true,
    }))
}

async fn transition_run_terminal(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: &Ulid,
    run_id: &Ulid,
    next: RunState,
) -> Result<(), ApplicationError> {
    let payload: Vec<u8> = sqlx::query_scalar(
        "SELECT payload FROM research.experiment_runs
         WHERE tenant_id=$1 AND experiment_run_id=$2 FOR UPDATE",
    )
    .bind(tenant_id.as_str())
    .bind(run_id.as_str())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let current = decode_run(&payload)?;
    if current.state() == next {
        return Ok(());
    }
    let next_run = current
        .transition(next, current.revision())
        .map_err(ficant_application::map_domain_error)?;
    sqlx::query(
        "UPDATE research.experiment_runs
         SET state=$3,revision=$4,payload=$5
         WHERE tenant_id=$1 AND experiment_run_id=$2",
    )
    .bind(tenant_id.as_str())
    .bind(run_id.as_str())
    .bind(run_state_sql(next))
    .bind(version_i64(next_run.revision())?)
    .bind(encode_run(&next_run))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    sqlx::query(
        "INSERT INTO research.experiment_run_revisions
         (tenant_id,experiment_run_id,revision,state,payload)
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(tenant_id.as_str())
    .bind(run_id.as_str())
    .bind(version_i64(next_run.revision())?)
    .bind(run_state_sql(next))
    .bind(encode_run(&next_run))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

fn evidence(sequence: i64, hash: &str) -> Result<NodeJournalEvidence, ApplicationError> {
    Ok(NodeJournalEvidence {
        sequence: u64::try_from(sequence).map_err(|_| storage())?,
        event_hash: parse_hash(hash)?,
    })
}

fn allowed_owners(scope: &AccessScope) -> Vec<String> {
    scope
        .allowed_owner_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

fn parse_id(value: &str) -> Result<Ulid, ApplicationError> {
    Ulid::new(value).map_err(ficant_application::map_domain_error)
}

fn parse_hash(value: &str) -> Result<ContentHash, ApplicationError> {
    if value.len() != 64 {
        return Err(storage());
    }
    let mut bytes = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).map_err(|_| storage())?;
        bytes[index] = u8::from_str_radix(text, 16).map_err(|_| storage())?;
    }
    ContentHash::from_bytes(&bytes).map_err(ficant_application::map_domain_error)
}

fn hash_hex(value: &ContentHash) -> String {
    use std::fmt::Write as _;

    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

const fn event_type_sql(value: JournalEventType) -> &'static str {
    match value {
        JournalEventType::RunSucceeded => "RUN_SUCCEEDED",
        JournalEventType::RunFailed => "RUN_FAILED",
        JournalEventType::NodeStarted => "NODE_STARTED",
        JournalEventType::NodeSucceeded => "NODE_SUCCEEDED",
        JournalEventType::NodeFailed => "NODE_FAILED",
        JournalEventType::NodeCheckpointed => "NODE_CHECKPOINTED",
        _ => "INVALID",
    }
}

const fn run_state_sql(value: RunState) -> &'static str {
    match value {
        RunState::Created => "CREATED",
        RunState::Running => "RUNNING",
        RunState::Succeeded => "SUCCEEDED",
        RunState::Failed => "FAILED",
        RunState::Cancelled => "CANCELLED",
    }
}

fn version_i64(value: u64) -> Result<i64, ApplicationError> {
    i64::try_from(value).map_err(|_| invalid())
}

fn invalid() -> ApplicationError {
    application_error(ApplicationErrorCategory::ValidationFailed, false)
}
fn immutable() -> ApplicationError {
    application_error(ApplicationErrorCategory::ImmutableViolation, false)
}
fn lineage() -> ApplicationError {
    application_error(ApplicationErrorCategory::LineageIncomplete, false)
}
fn state_conflict() -> ApplicationError {
    application_error(ApplicationErrorCategory::StateConflict, false)
}
fn concurrency() -> ApplicationError {
    application_error(ApplicationErrorCategory::ConcurrencyConflict, true)
}
fn storage() -> ApplicationError {
    application_error(ApplicationErrorCategory::StorageUnavailable, true)
}
