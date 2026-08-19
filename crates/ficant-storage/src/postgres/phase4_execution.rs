use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, BeginNode, CompleteNode, EnqueueNode, ExecutionInstanceIdentity,
    ExternalInputArtifactBinding, FailNode, GraphNodeEvent, GraphRunComparison, GraphRunRecord,
    NodeBeginResult, NodeFailureResult, NodeJournalEvidence, NodeSuccessResult,
    OutputPublicationIntent, OutputPublicationIntentState, OutputTrace, Phase4ExecutionRepository,
    PrepareOutputPublication, StoredExecutionIdentity, StoredNodeManifest, SubmitGraphRun,
    compare_graph_run_dimensions, replay_graph_execution, stable_node_artifact_id,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::research::v1 as research_pb;
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    Artifact, ArtifactKind, JournalEventType, ResearchGraph, RunJournal, RunJournalInput, RunState,
};
use ficant_domain::{ContentAddressed, Lineaged};
use ficant_runtime::{CodeBinding, decode_canonical_output_bytes};
use prost::Message;
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};

use super::PostgresRepository;
use super::codec::{
    decode_artifact, decode_execution_identity, decode_journal, decode_research_graph, decode_run,
    encode_artifact, encode_execution_identity, encode_journal, encode_research_graph, encode_run,
};
use super::common::{application_error, insert_lineage, map_sqlx_error, publish_blob_reference};

#[allow(clippy::too_many_lines)]
#[async_trait]
impl Phase4ExecutionRepository for PostgresRepository {
    async fn submit_graph_run(
        &self,
        command: SubmitGraphRun,
    ) -> Result<GraphRunRecord, ApplicationError> {
        validate_submit_command(&command)?;
        let first_task = derive_first_task(&command);
        let request_fingerprint = submit_fingerprint(&command);
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let key_run: Option<String> = sqlx::query_scalar(
            "SELECT experiment_run_id::text FROM research.experiment_runs
             WHERE tenant_id=$1 AND idempotency_key=$2 FOR UPDATE",
        )
        .bind(command.scope.tenant_id().as_str())
        .bind(command.idempotency_key.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if key_run
            .as_deref()
            .is_some_and(|value| value != command.run.id().as_str())
        {
            return Err(immutable());
        }
        let existing: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT payload FROM research.experiment_runs
             WHERE tenant_id=$1 AND experiment_run_id=$2 FOR UPDATE",
        )
        .bind(command.scope.tenant_id().as_str())
        .bind(command.run.id().as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if let Some(payload) = existing {
            let run = decode_run(&payload)?;
            validate_submit_replay(&mut transaction, &command, &run, &request_fingerprint).await?;
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(GraphRunRecord {
                run,
                graph: command.graph,
                identity: command.identity,
            });
        }

        let tenant = command.run.owner().tenant_id().as_str();
        let owner = command.run.owner().owner_id().as_str();
        let run_id = command.run.id().as_str();
        let running = command
            .run
            .transition(RunState::Running, command.run.revision())
            .map_err(ficant_application::map_domain_error)?;
        sqlx::query(
            "INSERT INTO research.experiment_runs
             (tenant_id,experiment_run_id,owner_id,state,revision,idempotency_key,fingerprint,payload)
             VALUES ($1,$2,$3,'RUNNING',$4,$5,$6,$7)",
        )
        .bind(tenant)
        .bind(run_id)
        .bind(owner)
        .bind(version_i64(running.revision())?)
        .bind(command.idempotency_key.as_str())
        .bind(request_fingerprint.as_bytes().as_slice())
        .bind(encode_run(&running))
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        for run in [&command.run, &running] {
            sqlx::query(
                "INSERT INTO research.experiment_run_revisions
                 (tenant_id,experiment_run_id,revision,state,payload)
                 VALUES ($1,$2,$3,$4,$5)",
            )
            .bind(tenant)
            .bind(run_id)
            .bind(version_i64(run.revision())?)
            .bind(run_state_sql(run.state()))
            .bind(encode_run(run))
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        insert_lineage(&mut transaction, tenant, run_id, command.run.lineage()).await?;

        let graph_payload = encode_research_graph(&command.graph);
        sqlx::query(
            "INSERT INTO research.research_graphs
             (tenant_id,graph_id,version,owner_id,graph_digest,payload)
             VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(tenant)
        .bind(command.graph.graph_id().as_str())
        .bind(version_i64(command.graph.version().get())?)
        .bind(owner)
        .bind(hash_hex(command.graph.digest()))
        .bind(&graph_payload)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        persist_identity_rows(&mut transaction, &command.identity, &command.graph).await?;

        let database_now: DateTime<Utc> = sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
            .fetch_one(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        let lifecycle_events = submit_lifecycle_events(&command, database_now)?;
        for event in [&lifecycle_events.0, &lifecycle_events.1] {
            sqlx::query(
                "INSERT INTO research.run_journal
                 (tenant_id,run_id,sequence,journal_event_id,event_type,occurred_at,
                  prev_hash,event_hash,idempotency_key,fingerprint,payload)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
            )
            .bind(tenant)
            .bind(run_id)
            .bind(version_i64(event.sequence())?)
            .bind(event.id().as_str())
            .bind(event_type_sql(event.event_type()))
            .bind(event.occurred_at().instant())
            .bind(event.prev_hash().map(hash_hex))
            .bind(hash_hex(event.content_hash()))
            .bind(format!("phase4-submit/{run_id}/{}", event.sequence()))
            .bind(ContentHash::digest(event.payload()).as_bytes().as_slice())
            .bind(encode_journal(event))
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        }
        sqlx::query(
            "INSERT INTO research.run_journal_sequences (tenant_id,run_id,next_sequence)
             VALUES ($1,$2,3)",
        )
        .bind(tenant)
        .bind(run_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        persist_enqueue(&mut transaction, &first_task).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(GraphRunRecord {
            run: running,
            graph: command.graph,
            identity: command.identity,
        })
    }

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

    async fn get_graph_run(
        &self,
        scope: &AccessScope,
        run_id: &Ulid,
    ) -> Result<Option<GraphRunRecord>, ApplicationError> {
        let owners = allowed_owners(scope);
        let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>, String, i64)> = sqlx::query_as(
            "SELECT r.payload,g.payload,i.payload,i.owner_id::text,i.graph_version
             FROM research.experiment_runs r
             JOIN research.execution_identities i
               ON i.tenant_id=r.tenant_id AND i.run_id=r.experiment_run_id
             JOIN research.research_graphs g
               ON g.tenant_id=i.tenant_id AND g.graph_id=i.graph_id AND g.version=i.graph_version
             WHERE r.tenant_id=$1 AND r.experiment_run_id=$2
               AND r.owner_id::text=ANY($3::text[])",
        )
        .bind(scope.tenant_id().as_str())
        .bind(run_id.as_str())
        .bind(owners)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let Some((run_payload, graph_payload, identity_payload, owner_id, graph_version)) = row
        else {
            return Ok(None);
        };
        let run = decode_run(&run_payload)?;
        let graph = decode_research_graph(&graph_payload)?;
        let identity = decode_execution_identity(&identity_payload, &graph)?;
        let external_input_artifacts =
            load_external_bindings(self.pool(), scope.tenant_id(), run_id).await?;
        Ok(Some(GraphRunRecord {
            run,
            graph: graph.clone(),
            identity: StoredExecutionIdentity {
                owner: OwnerRef::new(scope.tenant_id().clone(), parse_id(&owner_id)?),
                graph_id: graph.graph_id().clone(),
                graph_version: Version::new(u64::try_from(graph_version).map_err(|_| invalid())?)
                    .map_err(ficant_application::map_domain_error)?,
                identity,
                external_input_artifacts,
            },
        }))
    }

    async fn list_node_manifests(
        &self,
        scope: &AccessScope,
        run_id: &Ulid,
    ) -> Result<Vec<StoredNodeManifest>, ApplicationError> {
        type ManifestRow = (String, i64, String, Vec<u8>, String, i64, String, Vec<u8>);
        let owners = allowed_owners(scope);
        let rows: Vec<ManifestRow> = sqlx::query_as(
            "SELECT n.node_id::text,n.attempt,n.output_manifest_hash::text,n.output_manifest,
                    n.artifact_id::text,n.checkpoint_journal_sequence,
                    n.checkpoint_journal_hash::text,a.payload
             FROM research.node_executions n
             JOIN research.experiment_runs r
               ON r.tenant_id=n.tenant_id AND r.experiment_run_id=n.run_id
             JOIN research.artifacts a
               ON a.tenant_id=n.tenant_id AND a.artifact_id=n.artifact_id
             WHERE n.tenant_id=$1 AND n.run_id=$2 AND n.state='SUCCEEDED'
               AND r.owner_id::text=ANY($3::text[])
             ORDER BY n.checkpoint_journal_sequence",
        )
        .bind(scope.tenant_id().as_str())
        .bind(run_id.as_str())
        .bind(owners)
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        rows.into_iter()
            .map(
                |(
                    node_id,
                    attempt,
                    manifest_hash,
                    manifest,
                    artifact_id,
                    sequence,
                    hash,
                    payload,
                )| {
                    let artifact = decode_artifact(&payload)?;
                    if artifact.id().as_str() != artifact_id {
                        return Err(storage());
                    }
                    Ok(StoredNodeManifest {
                        run_id: run_id.clone(),
                        node_id: parse_id(&node_id)?,
                        attempt: u64::try_from(attempt).map_err(|_| storage())?,
                        artifact,
                        manifest_hash: parse_hash(&manifest_hash)?,
                        manifest,
                        checkpoint: evidence(sequence, &hash)?,
                    })
                },
            )
            .collect()
    }

    async fn trace_output(
        &self,
        scope: &AccessScope,
        run_id: &Ulid,
        node_id: &Ulid,
    ) -> Result<Option<OutputTrace>, ApplicationError> {
        let Some(run) = self.get_graph_run(scope, run_id).await? else {
            return Ok(None);
        };
        if !run
            .graph
            .nodes()
            .iter()
            .any(|node| node.node_id() == node_id)
        {
            return Ok(None);
        }
        let all = self.list_node_manifests(scope, run_id).await?;
        if !all.iter().any(|manifest| &manifest.node_id == node_id) {
            return Ok(None);
        }
        let mut required = std::collections::BTreeSet::from([node_id.clone()]);
        loop {
            let before = required.len();
            for edge in run.graph.edges() {
                if required.contains(edge.to_node()) {
                    required.insert(edge.from_node().clone());
                }
            }
            if required.len() == before {
                break;
            }
        }
        let manifests = all
            .into_iter()
            .filter(|manifest| required.contains(&manifest.node_id))
            .collect();
        let external_inputs = run.identity.external_input_artifacts.clone();
        Ok(Some(OutputTrace {
            run,
            manifests,
            external_inputs,
        }))
    }

    async fn compare_graph_runs(
        &self,
        scope: &AccessScope,
        left_run_id: &Ulid,
        right_run_id: &Ulid,
    ) -> Result<Option<GraphRunComparison>, ApplicationError> {
        let Some(left) = self.get_graph_run(scope, left_run_id).await? else {
            return Ok(None);
        };
        let Some(right) = self.get_graph_run(scope, right_run_id).await? else {
            return Ok(None);
        };
        let left_r = left.identity.identity.reproducibility();
        let right_r = right.identity.identity.reproducibility();
        let left_results = self.list_node_manifests(scope, left_run_id).await?;
        let right_results = self.list_node_manifests(scope, right_run_id).await?;
        let differences = compare_graph_run_dimensions(
            left_r,
            right_r,
            terminal_result(&left.graph, &left_results)
                != terminal_result(&right.graph, &right_results),
        );
        Ok(Some(GraphRunComparison {
            left_run_id: left_run_id.clone(),
            right_run_id: right_run_id.clone(),
            differing_dimensions: differences,
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
                validate_resume_node(
                    &mut transaction,
                    &command.fence.tenant_id,
                    &command.fence.run_id,
                    &command.fence.node_id,
                )
                .await?;
                transaction.commit().await.map_err(map_sqlx_error)?;
                return Ok(NodeBeginResult {
                    evidence: evidence(sequence, &hash)?,
                    replayed: true,
                });
            }
            return Err(state_conflict());
        }
        validate_active_fence(&mut transaction, &command.fence).await?;
        validate_resume_node(
            &mut transaction,
            &command.fence.tenant_id,
            &command.fence.run_id,
            &command.fence.node_id,
        )
        .await?;
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

    async fn prepare_output_publication(
        &self,
        command: PrepareOutputPublication,
    ) -> Result<OutputPublicationIntent, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        validate_active_fence(&mut transaction, command.fence()).await?;
        validate_publication_source(&mut transaction, &command).await?;
        let artifact = command.artifact();
        let evidence = command.formal_evidence();
        let encoded = super::formal_outputs::encode_formal_evidence(evidence);
        let evidence_hash = ContentHash::digest(&encoded);
        sqlx::query(
            "INSERT INTO research.output_publication_intents
             (tenant_id,intent_id,run_id,node_id,task_id,execution_identity_digest,
              planned_artifact_id,output_identity,result_hash,blob_size,
              formal_evidence_hash,formal_evidence,state)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'PREPARED')
             ON CONFLICT DO NOTHING",
        )
        .bind(command.fence().tenant_id.as_str())
        .bind(command.intent_id().as_str())
        .bind(command.fence().run_id.as_str())
        .bind(command.fence().node_id.as_str())
        .bind(command.fence().task_id.as_str())
        .bind(hash_hex(&command.fence().execution_identity_digest))
        .bind(artifact.id().as_str())
        .bind(hash_hex(evidence.output_identity()))
        .bind(hash_hex(artifact.content_hash()))
        .bind(i64::try_from(artifact.blob_size()).map_err(|_| invalid())?)
        .bind(hash_hex(&evidence_hash))
        .bind(encoded)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        let record = load_publication_intent(
            &mut transaction,
            &command.fence().tenant_id,
            &command.fence().run_id,
            &command.fence().node_id,
        )
        .await?
        .ok_or_else(immutable)?;
        require_exact_publication_intent(&record, &command, &evidence_hash)?;
        if record.state == OutputPublicationIntentState::Abandoned {
            return Err(state_conflict());
        }
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(record)
    }

    async fn complete_node(
        &self,
        command: CompleteNode,
    ) -> Result<NodeSuccessResult, ApplicationError> {
        let manifest_hash = ContentHash::digest(&command.output_manifest);
        if command.output_manifest.is_empty()
            || command.verified_blob.content_hash() != command.artifact.content_hash()
            || command.verified_blob.size() != command.artifact.blob_size()
            || u64::try_from(command.verified_payload.len()).ok()
                != Some(command.verified_blob.size())
            || ContentHash::digest(&command.verified_payload)
                != *command.verified_blob.content_hash()
            || command.formal_evidence.result_hash() != command.artifact.content_hash()
            || command.formal_evidence.subject().owner() != command.artifact.owner()
        {
            return Err(invalid());
        }
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        validate_output_manifest(&mut transaction, &command).await?;
        if let Some(replayed) = replay_success(&mut transaction, &command, &manifest_hash).await? {
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(replayed);
        }
        validate_active_fence(&mut transaction, &command.fence).await?;
        require_publication_intent_state(
            &mut transaction,
            &command,
            OutputPublicationIntentState::Prepared,
        )
        .await?;
        validate_resume_node(
            &mut transaction,
            &command.fence.tenant_id,
            &command.fence.run_id,
            &command.fence.node_id,
        )
        .await?;
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
        persist_or_reuse_artifact(
            &mut transaction,
            &command.artifact,
            &command.formal_evidence,
        )
        .await?;
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
        let intent_completed = sqlx::query(
            "UPDATE research.output_publication_intents
             SET state='COMPLETED',completed_at=CURRENT_TIMESTAMP,updated_at=CURRENT_TIMESTAMP
             WHERE tenant_id=$1 AND intent_id=$2 AND state='PREPARED'",
        )
        .bind(command.fence.tenant_id.as_str())
        .bind(command.publication_intent_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if intent_completed.rows_affected() != 1 {
            return Err(immutable());
        }
        if let Some(next) = derive_next_task(&mut transaction, &command.fence).await? {
            persist_enqueue(&mut transaction, &next).await?;
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
        validate_resume_node(
            &mut transaction,
            &command.fence.tenant_id,
            &command.fence.run_id,
            &command.fence.node_id,
        )
        .await?;
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

fn validate_submit_command(command: &SubmitGraphRun) -> Result<(), ApplicationError> {
    command.scope.authorize(command.run.owner())?;
    let identity = &command.identity.identity;
    if command.run.state() != RunState::Created
        || command.run.revision() != 1
        || command.graph.owner() != command.run.owner()
        || command.identity.owner != *command.run.owner()
        || command.identity.graph_id != *command.graph.graph_id()
        || command.identity.graph_version != command.graph.version()
        || identity.run_id() != command.run.id()
        || identity.reproducibility().graph_digest() != command.graph.digest()
    {
        return Err(lineage());
    }
    Ok(())
}

fn submit_fingerprint(command: &SubmitGraphRun) -> ContentHash {
    fn field(bytes: &mut Vec<u8>, value: &[u8]) {
        bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
        bytes.extend_from_slice(value);
    }
    let mut bytes = b"ficant/submit-graph-run-fingerprint/v1".to_vec();
    field(&mut bytes, command.scope.tenant_id().as_str().as_bytes());
    field(
        &mut bytes,
        command.run.owner().owner_id().as_str().as_bytes(),
    );
    field(&mut bytes, &encode_run(&command.run));
    field(&mut bytes, &encode_research_graph(&command.graph));
    field(
        &mut bytes,
        &encode_execution_identity(&command.identity.identity),
    );
    let mut bindings = command.identity.external_input_artifacts.clone();
    bindings.sort_by(|left, right| left.input_id.cmp(&right.input_id));
    for binding in bindings {
        field(&mut bytes, binding.input_id.as_bytes());
        field(&mut bytes, binding.artifact_id.as_str().as_bytes());
        field(&mut bytes, binding.content_hash.as_bytes());
    }
    ContentHash::digest(&bytes)
}

fn submit_lifecycle_events(
    command: &SubmitGraphRun,
    occurred: DateTime<Utc>,
) -> Result<(RunJournal, RunJournal), ApplicationError> {
    let occurred_at = MarketTime::new(occurred, "UTC", occurred.date_naive())
        .map_err(ficant_application::map_domain_error)?;
    let event = |sequence: u64,
                 event_type: JournalEventType,
                 previous: Option<ContentHash>|
     -> Result<RunJournal, ApplicationError> {
        let mut domain = b"ficant/submit-graph-run-event/v1".to_vec();
        domain.extend_from_slice(command.scope.tenant_id().as_str().as_bytes());
        domain.extend_from_slice(command.run.id().as_str().as_bytes());
        domain.extend_from_slice(command.idempotency_key.as_str().as_bytes());
        domain.extend_from_slice(&sequence.to_be_bytes());
        let input = RunJournalInput {
            journal_event_id: stable_node_artifact_id(
                &ContentHash::digest(&domain),
                command.run.id(),
            ),
            run_id: command.run.id().clone(),
            sequence,
            event_type,
            occurred_at: occurred_at.clone(),
            payload_type: "ficant.graph-run-submission".to_owned(),
            payload_schema: "ficant.graph-run-submission.v1".to_owned(),
            payload: submit_fingerprint(command).as_bytes().to_vec(),
            prev_hash: previous,
        };
        let hash = input
            .canonical_hash()
            .map_err(ficant_application::map_domain_error)?;
        RunJournal::new(input, &hash).map_err(ficant_application::map_domain_error)
    };
    let created = event(1, JournalEventType::RunCreated, None)?;
    let started = event(
        2,
        JournalEventType::RunStarted,
        Some(created.content_hash().clone()),
    )?;
    Ok((created, started))
}

fn derive_first_task(command: &SubmitGraphRun) -> EnqueueNode {
    let node_id = command.graph.topological_order()[0].clone();
    let mut task_domain = b"ficant/repository-node-task/v1".to_vec();
    task_domain.extend_from_slice(command.run.id().as_str().as_bytes());
    EnqueueNode {
        tenant_id: command.run.owner().tenant_id().clone(),
        task_id: stable_node_artifact_id(&ContentHash::digest(&task_domain), &node_id),
        run_id: command.run.id().clone(),
        node_id: node_id.clone(),
        graph_digest: command.graph.digest().clone(),
        execution_identity_digest: command.identity.identity.digest().clone(),
        planned_artifact_id: stable_node_artifact_id(
            command.identity.identity.reproducibility_digest(),
            &node_id,
        ),
        task_key: format!("phase4-node/{}/{}", command.run.id(), node_id),
    }
}

// Replay validation intentionally compares every row written by the original atomic submission.
#[allow(clippy::too_many_lines)]
async fn validate_submit_replay(
    transaction: &mut Transaction<'_, Postgres>,
    command: &SubmitGraphRun,
    stored_run: &ficant_domain::research::ExperimentRun,
    request_fingerprint: &ContentHash,
) -> Result<(), ApplicationError> {
    let expected_running = command
        .run
        .transition(RunState::Running, command.run.revision())
        .map_err(ficant_application::map_domain_error)?;
    if stored_run != &expected_running {
        return Err(immutable());
    }
    let (stored_key, stored_fingerprint): (String, Vec<u8>) = sqlx::query_as(
        "SELECT idempotency_key,fingerprint FROM research.experiment_runs
         WHERE tenant_id=$1 AND experiment_run_id=$2 FOR SHARE",
    )
    .bind(command.scope.tenant_id().as_str())
    .bind(command.run.id().as_str())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if stored_key != command.idempotency_key.as_str()
        || stored_fingerprint != request_fingerprint.as_bytes()
    {
        return Err(immutable());
    }
    let graph: Option<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT graph_digest::text,payload FROM research.research_graphs
         WHERE tenant_id=$1 AND graph_id=$2 AND version=$3 FOR SHARE",
    )
    .bind(command.scope.tenant_id().as_str())
    .bind(command.graph.graph_id().as_str())
    .bind(version_i64(command.graph.version().get())?)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if graph
        != Some((
            hash_hex(command.graph.digest()),
            encode_research_graph(&command.graph),
        ))
    {
        return Err(immutable());
    }
    let identity: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT payload FROM research.execution_identities
         WHERE tenant_id=$1 AND run_id=$2 FOR SHARE",
    )
    .bind(command.scope.tenant_id().as_str())
    .bind(command.run.id().as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if identity.as_deref() != Some(encode_execution_identity(&command.identity.identity).as_slice())
    {
        return Err(immutable());
    }
    let persisted_bindings: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT input_id,artifact_id::text,content_hash::text
         FROM research.execution_external_inputs
         WHERE tenant_id=$1 AND run_id=$2 ORDER BY input_id",
    )
    .bind(command.scope.tenant_id().as_str())
    .bind(command.run.id().as_str())
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let mut requested_bindings = command.identity.external_input_artifacts.clone();
    requested_bindings.sort_by(|left, right| left.input_id.cmp(&right.input_id));
    let requested_bindings = requested_bindings
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
    let persisted_events: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT payload FROM research.run_journal
         WHERE tenant_id=$1 AND run_id=$2 AND sequence IN (1,2) ORDER BY sequence",
    )
    .bind(command.scope.tenant_id().as_str())
    .bind(command.run.id().as_str())
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let events = persisted_events
        .into_iter()
        .map(|payload| decode_journal(&payload))
        .collect::<Result<Vec<_>, _>>()?;
    if events.len() != 2 {
        return Err(immutable());
    }
    let expected_events = submit_lifecycle_events(command, events[0].occurred_at().instant())?;
    if events != [expected_events.0, expected_events.1]
        || replay_graph_execution(&command.graph, &events).is_err()
    {
        return Err(immutable());
    }
    let first_task = derive_first_task(command);
    let exact_task: bool = sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM research.execution_tasks
           WHERE tenant_id=$1 AND task_id=$2 AND run_id=$3 AND node_id=$4
             AND graph_digest=$5 AND execution_identity_digest=$6
             AND planned_artifact_id=$7 AND task_key=$8)",
    )
    .bind(first_task.tenant_id.as_str())
    .bind(first_task.task_id.as_str())
    .bind(first_task.run_id.as_str())
    .bind(first_task.node_id.as_str())
    .bind(hash_hex(&first_task.graph_digest))
    .bind(hash_hex(&first_task.execution_identity_digest))
    .bind(first_task.planned_artifact_id.as_str())
    .bind(&first_task.task_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if !exact_task {
        return Err(immutable());
    }
    Ok(())
}

// Keeping the identity aggregate in one transaction helper makes the all-or-nothing write set
// auditable against the reproducibility contract.
#[allow(clippy::too_many_lines)]
async fn persist_identity_rows(
    transaction: &mut Transaction<'_, Postgres>,
    value: &StoredExecutionIdentity,
    graph: &ResearchGraph,
) -> Result<(), ApplicationError> {
    validate_run_identity(transaction, value, graph).await?;
    let identity = &value.identity;
    let reproducibility = identity.reproducibility();
    let mut artifacts = value.external_input_artifacts.clone();
    artifacts.sort_by(|left, right| left.input_id.cmp(&right.input_id));
    if artifacts.len() != reproducibility.external_inputs().len()
        || !artifacts
            .iter()
            .zip(reproducibility.external_inputs())
            .all(|(artifact, input)| {
                artifact.input_id == input.input_id()
                    && artifact.content_hash == *input.content_hash()
            })
    {
        return Err(lineage());
    }
    sqlx::query(
        "INSERT INTO research.execution_identities
         (tenant_id,run_id,owner_id,graph_id,graph_version,graph_digest,
          reproducibility_digest,execution_identity_digest,payload)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(value.owner.tenant_id().as_str())
    .bind(identity.run_id().as_str())
    .bind(value.owner.owner_id().as_str())
    .bind(value.graph_id.as_str())
    .bind(version_i64(value.graph_version.get())?)
    .bind(hash_hex(reproducibility.graph_digest()))
    .bind(hash_hex(identity.reproducibility_digest()))
    .bind(hash_hex(identity.digest()))
    .bind(encode_execution_identity(identity))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    for (artifact, input) in artifacts.iter().zip(reproducibility.external_inputs()) {
        let persisted: Option<(String, String)> = sqlx::query_as(
            "SELECT owner_id::text,content_hash::text FROM research.artifacts
             WHERE tenant_id=$1 AND artifact_id=$2 FOR SHARE",
        )
        .bind(value.owner.tenant_id().as_str())
        .bind(artifact.artifact_id.as_str())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
        if persisted
            != Some((
                value.owner.owner_id().as_str().to_owned(),
                hash_hex(input.content_hash()),
            ))
        {
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
        .bind(artifact.artifact_id.as_str())
        .bind(hash_hex(input.content_hash()))
        .execute(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
    }
    for binding in reproducibility.rule_pack_bindings() {
        let rule_id =
            Ulid::new(&binding.rule_pack_id).map_err(ficant_application::map_domain_error)?;
        let persisted: Option<String> = sqlx::query_scalar(
            "SELECT content_hash::text FROM market.market_rule_packs
             WHERE tenant_id=$1 AND rule_pack_id=$2 AND version=$3 FOR SHARE",
        )
        .bind(value.owner.tenant_id().as_str())
        .bind(rule_id.as_str())
        .bind(version_i64(binding.version.get())?)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
        if persisted.as_deref() != Some(hash_hex(&binding.content_hash).as_str()) {
            return Err(lineage());
        }
        sqlx::query(
            "INSERT INTO research.execution_rule_packs
             (tenant_id,run_id,rule_pack_id,version,content_hash) VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(value.owner.tenant_id().as_str())
        .bind(identity.run_id().as_str())
        .bind(rule_id.as_str())
        .bind(version_i64(binding.version.get())?)
        .bind(hash_hex(&binding.content_hash))
        .execute(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
    }
    for binding in reproducibility.node_implementations() {
        if !graph
            .nodes()
            .iter()
            .any(|node| node.node_id() == &binding.node_id)
        {
            return Err(lineage());
        }
        sqlx::query(
            "INSERT INTO research.execution_node_implementations
             (tenant_id,run_id,node_id,implementation_digest) VALUES ($1,$2,$3,$4)",
        )
        .bind(value.owner.tenant_id().as_str())
        .bind(identity.run_id().as_str())
        .bind(binding.node_id.as_str())
        .bind(hash_hex(&binding.implementation_digest))
        .execute(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
    }
    Ok(())
}

async fn load_external_bindings(
    pool: &sqlx::PgPool,
    tenant_id: &Ulid,
    run_id: &Ulid,
) -> Result<Vec<ExternalInputArtifactBinding>, ApplicationError> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT input_id,artifact_id::text,content_hash::text
         FROM research.execution_external_inputs
         WHERE tenant_id=$1 AND run_id=$2 ORDER BY input_id",
    )
    .bind(tenant_id.as_str())
    .bind(run_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(map_sqlx_error)?;
    rows.into_iter()
        .map(|(input_id, artifact_id, content_hash)| {
            Ok(ExternalInputArtifactBinding {
                input_id,
                artifact_id: parse_id(&artifact_id)?,
                content_hash: parse_hash(&content_hash)?,
            })
        })
        .collect()
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
    validate_resume_node(
        transaction,
        &command.tenant_id,
        &command.run_id,
        &command.node_id,
    )
    .await?;
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

async fn validate_resume_node(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: &Ulid,
    run_id: &Ulid,
    node_id: &Ulid,
) -> Result<(), ApplicationError> {
    let graph_payload: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT g.payload FROM research.execution_identities i
         JOIN research.research_graphs g
           ON g.tenant_id=i.tenant_id AND g.graph_id=i.graph_id AND g.version=i.graph_version
         JOIN research.experiment_runs r
           ON r.tenant_id=i.tenant_id AND r.experiment_run_id=i.run_id
         WHERE i.tenant_id=$1 AND i.run_id=$2 AND r.state='RUNNING' FOR SHARE",
    )
    .bind(tenant_id.as_str())
    .bind(run_id.as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let graph = graph_payload
        .map(|payload| decode_research_graph(&payload))
        .transpose()?
        .ok_or_else(lineage)?;
    let journal_payloads: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT payload FROM research.run_journal
         WHERE tenant_id=$1 AND run_id=$2 ORDER BY sequence",
    )
    .bind(tenant_id.as_str())
    .bind(run_id.as_str())
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let journal = journal_payloads
        .into_iter()
        .map(|payload| decode_journal(&payload))
        .collect::<Result<Vec<_>, _>>()?;
    let replay = replay_graph_execution(&graph, &journal)
        .map_err(|error| ficant_application::map_runtime_error(&error))?;
    if replay.run_state() != RunState::Running || replay.resume_node() != Some(node_id) {
        return Err(lineage());
    }
    let completed: Vec<String> = sqlx::query_scalar(
        "SELECT node_id::text FROM research.execution_tasks
         WHERE tenant_id=$1 AND run_id=$2 AND state='COMPLETED'",
    )
    .bind(tenant_id.as_str())
    .bind(run_id.as_str())
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let completed = completed
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let replayed = replay
        .completed_nodes()
        .iter()
        .map(ToString::to_string)
        .collect::<std::collections::BTreeSet<_>>();
    if completed != replayed {
        return Err(lineage());
    }
    Ok(())
}

async fn derive_next_task(
    transaction: &mut Transaction<'_, Postgres>,
    fence: &ficant_application::ports::NodeLeaseFence,
) -> Result<Option<EnqueueNode>, ApplicationError> {
    let row: Option<(Vec<u8>, String, String)> = sqlx::query_as(
        "SELECT g.payload,i.graph_digest::text,i.reproducibility_digest::text
         FROM research.execution_identities i
         JOIN research.research_graphs g
           ON g.tenant_id=i.tenant_id AND g.graph_id=i.graph_id AND g.version=i.graph_version
         WHERE i.tenant_id=$1 AND i.run_id=$2 AND i.execution_identity_digest=$3 FOR SHARE",
    )
    .bind(fence.tenant_id.as_str())
    .bind(fence.run_id.as_str())
    .bind(hash_hex(&fence.execution_identity_digest))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let Some((payload, graph_digest, reproducibility_digest)) = row else {
        return Err(lineage());
    };
    let graph = decode_research_graph(&payload)?;
    let index = graph
        .topological_order()
        .iter()
        .position(|node| node == &fence.node_id)
        .ok_or_else(lineage)?;
    let Some(node_id) = graph.topological_order().get(index + 1) else {
        return Ok(None);
    };
    let mut task_domain = b"ficant/repository-node-task/v1".to_vec();
    task_domain.extend_from_slice(fence.run_id.as_str().as_bytes());
    let task_id = stable_node_artifact_id(&ContentHash::digest(&task_domain), node_id);
    Ok(Some(EnqueueNode {
        tenant_id: fence.tenant_id.clone(),
        task_id,
        run_id: fence.run_id.clone(),
        node_id: node_id.clone(),
        graph_digest: parse_hash(&graph_digest)?,
        execution_identity_digest: fence.execution_identity_digest.clone(),
        planned_artifact_id: stable_node_artifact_id(
            &parse_hash(&reproducibility_digest)?,
            node_id,
        ),
        task_key: format!("phase4-node/{}/{}", fence.run_id, node_id),
    }))
}

// Manifest validation is one fail-closed boundary: splitting it would risk accepting a partially
// validated execution, port, artifact, or lineage dimension.
#[allow(clippy::too_many_lines)]
async fn validate_output_manifest(
    transaction: &mut Transaction<'_, Postgres>,
    command: &CompleteNode,
) -> Result<(), ApplicationError> {
    let manifest = research_pb::NodeOutputManifest::decode(command.output_manifest.as_slice())
        .map_err(|_| invalid())?;
    let verified_outputs = decode_canonical_output_bytes(
        &command.verified_payload,
        Some(command.artifact.content_hash()),
    )
    .map_err(|error| ficant_application::map_runtime_error(&error))?;
    let execution = manifest.execution.as_ref().ok_or_else(lineage)?;
    let content = manifest.content.as_ref().ok_or_else(lineage)?;
    let stored: Option<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT i.payload,g.payload FROM research.execution_identities i
         JOIN research.research_graphs g
           ON g.tenant_id=i.tenant_id AND g.graph_id=i.graph_id AND g.version=i.graph_version
         WHERE i.tenant_id=$1 AND i.run_id=$2 AND i.execution_identity_digest=$3 FOR SHARE",
    )
    .bind(command.fence.tenant_id.as_str())
    .bind(command.fence.run_id.as_str())
    .bind(hash_hex(&command.fence.execution_identity_digest))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let Some((identity_payload, graph_payload)) = stored else {
        return Err(lineage());
    };
    let graph = decode_research_graph(&graph_payload)?;
    let identity = decode_execution_identity(&identity_payload, &graph)?;
    let external_bindings: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT input_id,artifact_id::text,content_hash::text
         FROM research.execution_external_inputs
         WHERE tenant_id=$1 AND run_id=$2 ORDER BY input_id",
    )
    .bind(command.fence.tenant_id.as_str())
    .bind(command.fence.run_id.as_str())
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    validate_proto_execution(execution, &identity, &external_bindings)?;
    let node = graph
        .nodes()
        .iter()
        .find(|node| node.node_id() == &command.fence.node_id)
        .ok_or_else(lineage)?;
    let implementation = identity
        .reproducibility()
        .node_implementations()
        .iter()
        .find(|binding| binding.node_id == command.fence.node_id)
        .ok_or_else(lineage)?;
    if manifest.attempt != u32::try_from(command.fence.attempt).map_err(|_| invalid())?
        || proto_id(execution.run_id.as_ref())? != *identity.run_id()
        || proto_hash(execution.digest.as_ref())? != *identity.digest()
        || proto_hash(
            execution
                .reproducibility
                .as_ref()
                .and_then(|value| value.digest.as_ref()),
        )? != *identity.reproducibility_digest()
        || proto_id(content.node_id.as_ref())? != command.fence.node_id
        || proto_hash(content.reproducibility_digest.as_ref())?
            != *identity.reproducibility_digest()
        || proto_hash(content.node_contract_digest.as_ref())? != *node.contract().digest()
        || proto_hash(content.implementation_digest.as_ref())?
            != implementation.implementation_digest
    {
        return Err(lineage());
    }
    let mut unhashed = content.clone();
    let claimed_manifest_hash = proto_hash(unhashed.manifest_hash.take().as_ref())?;
    if claimed_manifest_hash != ContentHash::digest(&unhashed.encode_to_vec()) {
        return Err(immutable());
    }
    if content.outputs.len() != node.contract().output_types().len()
        || verified_outputs.len() != content.outputs.len()
        || content.inputs.len() != node.contract().input_types().len()
    {
        return Err(lineage());
    }
    for expected in node.contract().output_types() {
        let output = content
            .outputs
            .iter()
            .find(|value| value.port_name == expected.port_name())
            .ok_or_else(lineage)?;
        validate_proto_type(output.value_type.as_ref(), expected.value_type())?;
        let artifact = output.artifact.as_ref().ok_or_else(lineage)?;
        let formal = &command.formal_evidence;
        let claimed_formal = output.formal_evidence.as_ref().ok_or_else(lineage)?;
        if claimed_formal.encode_to_vec() != super::formal_outputs::encode_formal_evidence(formal) {
            return Err(lineage());
        }
        // `output.content_hash` addresses the typed port payload inside the canonical output
        // envelope. The Artifact lineage reference addresses the envelope blob itself. They are
        // deliberately distinct hashes. The verified payload is first bound by hash and size to
        // `VerifiedBlobRef`, then independently decoded as the canonical envelope; its decoded
        // port hash must match this manifest. The Artifact row continues to address the envelope.
        let port_payload_hash = proto_hash(output.content_hash.as_ref())?;
        let verified_output = verified_outputs
            .iter()
            .find(|value| value.port_name() == expected.port_name())
            .ok_or_else(lineage)?;
        if proto_id(artifact.object_id.as_ref())? != *command.artifact.id()
            || proto_hash(artifact.content_hash.as_ref())? != *command.artifact.content_hash()
            || verified_output.value_type() != expected.value_type()
            || verified_output.content_hash() != &port_payload_hash
        {
            return Err(lineage());
        }
    }
    for expected in node.contract().input_types() {
        let input = content
            .inputs
            .iter()
            .find(|value| value.port_name == expected.port_name())
            .ok_or_else(lineage)?;
        if proto_id(input.node_id.as_ref())? != command.fence.node_id {
            return Err(lineage());
        }
        validate_proto_type(input.value_type.as_ref(), expected.value_type())?;
        let artifact = input.resolved_artifact.as_ref().ok_or_else(lineage)?;
        if proto_hash(artifact.content_hash.as_ref())? != proto_hash(input.content_hash.as_ref())? {
            return Err(lineage());
        }
        validate_manifest_input(transaction, command, &graph, input).await?;
    }
    if command.artifact.lineage().len() != content.inputs.len()
        || content.inputs.iter().any(|input| {
            let Some(resolved) = input.resolved_artifact.as_ref() else {
                return true;
            };
            let (Ok(id), Ok(hash)) = (
                proto_id(resolved.object_id.as_ref()),
                proto_hash(resolved.content_hash.as_ref()),
            ) else {
                return true;
            };
            !command.artifact.lineage().iter().any(|reference| {
                reference.object_id() == &id && reference.content_hash() == Some(&hash)
            })
        })
    {
        return Err(lineage());
    }
    Ok(())
}

async fn validate_manifest_input(
    transaction: &mut Transaction<'_, Postgres>,
    command: &CompleteNode,
    graph: &ResearchGraph,
    input: &research_pb::NodeInputBinding,
) -> Result<(), ApplicationError> {
    let artifact = input.resolved_artifact.as_ref().ok_or_else(lineage)?;
    let artifact_id = proto_id(artifact.object_id.as_ref())?;
    let hash = proto_hash(input.content_hash.as_ref())?;
    match input.declared_source.as_ref().ok_or_else(lineage)? {
        research_pb::node_input_binding::DeclaredSource::ExternalInputId(input_id) => {
            let binding = graph
                .external_input_bindings()
                .iter()
                .find(|binding| {
                    binding.input_id() == input_id
                        && binding.to_node() == &command.fence.node_id
                        && binding.to_port() == input.port_name
                })
                .ok_or_else(lineage)?;
            let _ = binding;
            let exact: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                  SELECT 1 FROM research.execution_external_inputs
                  WHERE tenant_id=$1 AND run_id=$2 AND input_id=$3
                    AND artifact_id=$4 AND content_hash=$5)",
            )
            .bind(command.fence.tenant_id.as_str())
            .bind(command.fence.run_id.as_str())
            .bind(input_id)
            .bind(artifact_id.as_str())
            .bind(hash_hex(&hash))
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
            if !exact {
                return Err(lineage());
            }
        }
        research_pb::node_input_binding::DeclaredSource::UpstreamOutput(upstream) => {
            let upstream_id = proto_id(upstream.node_id.as_ref())?;
            if !graph.edges().iter().any(|edge| {
                edge.from_node() == &upstream_id
                    && edge.from_port() == upstream.port_name
                    && edge.to_node() == &command.fence.node_id
                    && edge.to_port() == input.port_name
            }) {
                return Err(lineage());
            }
            let exact: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                  SELECT 1 FROM research.node_executions n
                  JOIN research.artifacts a
                    ON a.tenant_id=n.tenant_id AND a.artifact_id=n.artifact_id
                  WHERE n.tenant_id=$1 AND n.run_id=$2 AND n.node_id=$3
                    AND n.state='SUCCEEDED' AND a.artifact_id=$4 AND a.content_hash=$5)",
            )
            .bind(command.fence.tenant_id.as_str())
            .bind(command.fence.run_id.as_str())
            .bind(upstream_id.as_str())
            .bind(artifact_id.as_str())
            .bind(hash_hex(&hash))
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_sqlx_error)?;
            if !exact {
                return Err(lineage());
            }
        }
    }
    Ok(())
}

fn proto_id(
    value: Option<&ficant_contracts::ficant::core::v1::Ulid>,
) -> Result<Ulid, ApplicationError> {
    value
        .ok_or_else(lineage)
        .and_then(|value| parse_id(&value.value))
}

fn proto_hash(
    value: Option<&ficant_contracts::ficant::core::v1::Sha256>,
) -> Result<ContentHash, ApplicationError> {
    value.ok_or_else(lineage).and_then(|value| {
        ContentHash::from_bytes(&value.value).map_err(ficant_application::map_domain_error)
    })
}

fn validate_proto_execution(
    actual: &research_pb::ExecutionInstanceIdentity,
    expected: &ExecutionInstanceIdentity,
    external_bindings: &[(String, String, String)],
) -> Result<(), ApplicationError> {
    let expected_r = expected.reproducibility();
    let actual_r = actual.reproducibility.as_ref().ok_or_else(lineage)?;
    let expected_subject = expected_r.subject().ok_or_else(lineage)?;
    let actual_subject = actual_r.subject.as_ref().ok_or_else(lineage)?;
    let decoded_subject =
        super::formal_outputs::decode_formal_input(&actual_subject.encode_to_vec())?;
    let expected_code = expected_r.code().ok_or_else(lineage)?;
    let actual_code = actual_r.code.as_ref().ok_or_else(lineage)?;
    let decoded_code = CodeBinding::from_claimed(
        actual_code.git_commit_sha.clone(),
        actual_code.git_tree_sha.clone(),
        proto_hash(actual_code.digest.as_ref())?,
    )
    .map_err(ficant_application::map_domain_error)?;
    if proto_id(actual.run_id.as_ref())? != *expected.run_id()
        || proto_hash(actual.digest.as_ref())? != *expected.digest()
        || proto_hash(actual_r.digest.as_ref())? != *expected.reproducibility_digest()
        || proto_hash(actual_r.graph_digest.as_ref())? != *expected_r.graph_digest()
        || proto_hash(actual_r.data_snapshot_hash.as_ref())? != *expected_r.data_snapshot_hash()
        || proto_hash(actual_r.universe_snapshot_hash.as_ref())?
            != *expected_r.universe_snapshot_hash()
        || proto_hash(actual_r.parameters_hash.as_ref())? != *expected_r.parameters_hash()
        || proto_hash(actual_r.runtime_image_digest.as_ref())? != *expected_r.runtime_image_digest()
        || proto_hash(actual_r.environment_digest.as_ref())? != *expected_r.environment_digest()
        || actual_r.seed != expected_r.seed()
        || actual_r.rule_packs.len() != expected_r.rule_pack_bindings().len()
        || actual_r.node_implementations.len() != expected_r.node_implementations().len()
        || actual_r.external_inputs.len() != expected_r.external_inputs().len()
        || external_bindings.len() != expected_r.external_inputs().len()
        || decoded_subject != *expected_subject
        || decoded_code != *expected_code
    {
        return Err(lineage());
    }
    for (actual, expected) in actual_r
        .rule_packs
        .iter()
        .zip(expected_r.rule_pack_bindings())
    {
        if proto_id(actual.rule_pack_id.as_ref())?.as_str() != expected.rule_pack_id
            || actual.version != expected.version.get()
            || proto_hash(actual.content_hash.as_ref())? != expected.content_hash
        {
            return Err(lineage());
        }
    }
    for (actual, expected) in actual_r
        .node_implementations
        .iter()
        .zip(expected_r.node_implementations())
    {
        if proto_id(actual.node_id.as_ref())? != expected.node_id
            || proto_hash(actual.implementation_digest.as_ref())? != expected.implementation_digest
        {
            return Err(lineage());
        }
    }
    for ((actual, expected), binding) in actual_r
        .external_inputs
        .iter()
        .zip(expected_r.external_inputs())
        .zip(external_bindings)
    {
        let resolved = actual.resolved_artifact.as_ref().ok_or_else(lineage)?;
        if actual.input_id != expected.input_id()
            || binding.0 != expected.input_id()
            || binding.2 != hash_hex(expected.content_hash())
            || proto_id(resolved.object_id.as_ref())?.as_str() != binding.1
            || proto_hash(resolved.content_hash.as_ref())? != *expected.content_hash()
            || proto_hash(actual.content_hash.as_ref())? != *expected.content_hash()
        {
            return Err(lineage());
        }
        validate_proto_type(actual.value_type.as_ref(), expected.value_type())?;
    }
    Ok(())
}

fn validate_proto_type(
    value: Option<&research_pb::TypedValue>,
    expected: &ficant_domain::research::TypedValue,
) -> Result<(), ApplicationError> {
    let value = value.ok_or_else(lineage)?;
    if value.type_id != expected.type_id()
        || value.type_version != expected.type_version().get()
        || proto_hash(value.schema_hash.as_ref())? != *expected.schema_hash()
    {
        return Err(lineage());
    }
    Ok(())
}

async fn validate_publication_source(
    transaction: &mut Transaction<'_, Postgres>,
    command: &PrepareOutputPublication,
) -> Result<(), ApplicationError> {
    let fence = command.fence();
    let artifact = command.artifact();
    let source: Option<(String, bool)> = sqlx::query_as(
        "SELECT task.planned_artifact_id::text,
                EXISTS(
                  SELECT 1 FROM research.node_executions node
                  WHERE node.tenant_id=task.tenant_id AND node.run_id=task.run_id
                    AND node.node_id=task.node_id AND node.attempt=task.claim_count
                    AND node.task_id=task.task_id
                    AND node.execution_identity_digest=task.execution_identity_digest
                    AND node.state='STARTED')
         FROM research.execution_tasks task
         WHERE task.tenant_id=$1 AND task.task_id=$2 AND task.run_id=$3 AND task.node_id=$4
           AND task.execution_identity_digest=$5
         FOR UPDATE",
    )
    .bind(fence.tenant_id.as_str())
    .bind(fence.task_id.as_str())
    .bind(fence.run_id.as_str())
    .bind(fence.node_id.as_str())
    .bind(hash_hex(&fence.execution_identity_digest))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if source != Some((artifact.id().to_string(), true))
        || artifact.owner().tenant_id() != &fence.tenant_id
        || command.formal_evidence().result_hash() != artifact.content_hash()
        || command.formal_evidence().subject().owner() != artifact.owner()
    {
        return Err(lineage());
    }
    Ok(())
}

async fn load_publication_intent(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: &Ulid,
    run_id: &Ulid,
    node_id: &Ulid,
) -> Result<Option<OutputPublicationIntent>, ApplicationError> {
    type IntentRow = (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        String,
        Vec<u8>,
        String,
    );
    let row: Option<IntentRow> = sqlx::query_as(
        "SELECT intent_id::text,run_id::text,node_id::text,task_id::text,
                execution_identity_digest::text,planned_artifact_id::text,
                output_identity::text,result_hash::text,blob_size,
                formal_evidence_hash::text,formal_evidence,state
         FROM research.output_publication_intents
         WHERE tenant_id=$1 AND run_id=$2 AND node_id=$3 FOR UPDATE",
    )
    .bind(tenant_id.as_str())
    .bind(run_id.as_str())
    .bind(node_id.as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    row.map(|row| {
        let formal_evidence_hash = parse_hash(&row.9)?;
        if ContentHash::digest(&row.10) != formal_evidence_hash {
            return Err(immutable());
        }
        Ok(OutputPublicationIntent {
            tenant_id: tenant_id.clone(),
            intent_id: parse_id(&row.0)?,
            run_id: parse_id(&row.1)?,
            node_id: parse_id(&row.2)?,
            task_id: parse_id(&row.3)?,
            execution_identity_digest: parse_hash(&row.4)?,
            planned_artifact_id: parse_id(&row.5)?,
            output_identity: parse_hash(&row.6)?,
            result_hash: parse_hash(&row.7)?,
            blob_size: u64::try_from(row.8).map_err(|_| immutable())?,
            formal_evidence_hash,
            state: parse_publication_state(&row.11)?,
        })
    })
    .transpose()
}

fn require_exact_publication_intent(
    actual: &OutputPublicationIntent,
    command: &PrepareOutputPublication,
    evidence_hash: &ContentHash,
) -> Result<(), ApplicationError> {
    let fence = command.fence();
    let artifact = command.artifact();
    let evidence = command.formal_evidence();
    if actual.tenant_id != fence.tenant_id
        || actual.intent_id != *command.intent_id()
        || actual.run_id != fence.run_id
        || actual.node_id != fence.node_id
        || actual.task_id != fence.task_id
        || actual.execution_identity_digest != fence.execution_identity_digest
        || actual.planned_artifact_id != *artifact.id()
        || actual.output_identity != *evidence.output_identity()
        || actual.result_hash != *artifact.content_hash()
        || actual.blob_size != artifact.blob_size()
        || actual.formal_evidence_hash != *evidence_hash
    {
        return Err(immutable());
    }
    Ok(())
}

async fn require_publication_intent_state(
    transaction: &mut Transaction<'_, Postgres>,
    command: &CompleteNode,
    state: OutputPublicationIntentState,
) -> Result<(), ApplicationError> {
    let record = load_publication_intent(
        transaction,
        &command.fence.tenant_id,
        &command.fence.run_id,
        &command.fence.node_id,
    )
    .await?
    .ok_or_else(state_conflict)?;
    let prepare = PrepareOutputPublication::new(
        command.fence.clone(),
        command.publication_intent_id.clone(),
        command.artifact.clone(),
        command.formal_evidence.clone(),
    )?;
    let evidence = &command.formal_evidence;
    let evidence_hash =
        ContentHash::digest(&super::formal_outputs::encode_formal_evidence(evidence));
    require_exact_publication_intent(&record, &prepare, &evidence_hash)?;
    if record.state != state {
        return Err(immutable());
    }
    Ok(())
}

fn parse_publication_state(value: &str) -> Result<OutputPublicationIntentState, ApplicationError> {
    match value {
        "PREPARED" => Ok(OutputPublicationIntentState::Prepared),
        "COMPLETED" => Ok(OutputPublicationIntentState::Completed),
        "ABANDONED" => Ok(OutputPublicationIntentState::Abandoned),
        _ => Err(immutable()),
    }
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
    let expected_owner: String = sqlx::query_scalar(
        "SELECT owner_id::text FROM research.execution_identities
         WHERE tenant_id=$1 AND run_id=$2 AND execution_identity_digest=$3 FOR SHARE",
    )
    .bind(command.fence.tenant_id.as_str())
    .bind(command.fence.run_id.as_str())
    .bind(hash_hex(&command.fence.execution_identity_digest))
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if command.artifact.kind() != ArtifactKind::Generic
        || command.artifact.owner().tenant_id() != &command.fence.tenant_id
        || command.artifact.owner().owner_id().as_str() != expected_owner
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
    evidence: &ficant_runtime::FormalOutputEvidence,
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
        super::artifacts::persist_or_verify_artifact_formal_evidence(
            transaction,
            artifact,
            Some(evidence),
        )
        .await?;
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
    .await?;
    super::artifacts::persist_or_verify_artifact_formal_evidence(
        transaction,
        artifact,
        Some(evidence),
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
    if super::artifacts::load_artifact_formal_evidence(transaction, &artifact)
        .await?
        .as_ref()
        != Some(&command.formal_evidence)
    {
        return Err(immutable());
    }
    require_publication_intent_state(
        transaction,
        command,
        OutputPublicationIntentState::Completed,
    )
    .await?;
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

fn terminal_result(
    graph: &ResearchGraph,
    manifests: &[StoredNodeManifest],
) -> Option<(ContentHash, ContentHash)> {
    let terminal = graph.topological_order().last()?;
    manifests
        .iter()
        .find(|manifest| &manifest.node_id == terminal)
        .map(|manifest| {
            (
                manifest.artifact.content_hash().clone(),
                manifest.manifest_hash.clone(),
            )
        })
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
        JournalEventType::RunCreated => "RUN_CREATED",
        JournalEventType::RunStarted => "RUN_STARTED",
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
