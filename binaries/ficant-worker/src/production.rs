use std::sync::Arc;

use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, AeadCursorCodec, ArtifactRepository, BeginBlobStage, BeginNode, BlobStore,
    CompleteNode, CursorKey, FailNode, IdempotencyKey, IntegrityEvent, IntegrityEventSink,
    NodeLeaseFence, PageRequest, Phase4ExecutionRepository, RequiredVerifiedBlobRead,
    RunJournalRepository, SafeTraceContext, VerifiedBlobReader, VerifiedBlobRole,
    VerifiedReadResourceKind, VerifyBlobStage, stable_node_artifact_id,
};
use ficant_contracts::ficant::{core::v1 as core_pb, research::v1 as research_pb};
use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{ContentHash, LineageRef, Ulid};
use ficant_domain::research::{Artifact, ArtifactKind, RunJournal, RunState};
use ficant_native_nodes::trusted_native_node;
use ficant_runtime::{
    NativePortValue, decode_canonical_output_bytes, execute_native_node, replay_graph_execution,
};
use ficant_storage::lease_queue::{LeaseQueueError, PostgresLeaseQueue};
use ficant_storage::postgres::PostgresRepository;
use ficant_storage::s3::S3BlobStore;
use prost::Message;
use sqlx::postgres::PgPoolOptions;

use crate::{
    ClaimedTask, ExecutedNode, InputEvidence, InputSource, LoadedTask, NodeCompletion,
    PreparedInputs, WorkerBackend, WorkerConfig, WorkerError, WorkerStep, derived_id,
};

const OUTPUT_MEDIA_TYPE: &str = "application/octet-stream";

pub struct ProductionWorkerBackend {
    queue: PostgresLeaseQueue,
    repository: PostgresRepository,
    blobs: S3BlobStore,
    integrity_events: WorkerIntegrityEventSink,
}

struct WorkerIntegrityEventSink;

#[async_trait]
impl IntegrityEventSink for WorkerIntegrityEventSink {
    async fn emit(
        &self,
        event: IntegrityEvent,
    ) -> Result<(), ficant_application::ApplicationError> {
        eprintln!(
            "event={} severity={} reason={} tenant={} resource={} trace={}",
            event.name(),
            event.severity().as_str(),
            event.reason().as_str(),
            event.tenant_id(),
            event.resource_id(),
            event.trace().trace_id(),
        );
        Ok(())
    }
}

impl ProductionWorkerBackend {
    /// Connects the production `PostgreSQL` and vendor-neutral `S3` adapters.
    ///
    /// # Errors
    ///
    /// Returns a retryable backend error without exposing connection strings or credentials.
    pub async fn connect(config: &WorkerConfig) -> Result<Self, WorkerError> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&config.database_url)
            .await
            .map_err(|_| backend(WorkerStep::Load, true))?;
        let cursor_material = *ContentHash::digest(config.worker_id.as_str().as_bytes()).as_bytes();
        let cursor = CursorKey::new("phase4-worker", cursor_material)
            .and_then(|key| AeadCursorCodec::new(key, vec![]))
            .map_err(|_| backend(WorkerStep::Load, false))?;
        let repository = PostgresRepository::new(pool.clone(), Arc::new(cursor));
        let blobs = S3BlobStore::new(
            &config.s3_endpoint,
            config.s3_bucket.clone(),
            &config.s3_access_key,
            &config.s3_secret_key,
            pool.clone(),
        )
        .map_err(|error| application_error(WorkerStep::Load, &error))?;
        Ok(Self {
            queue: PostgresLeaseQueue::new(pool),
            repository,
            blobs,
            integrity_events: WorkerIntegrityEventSink,
        })
    }

    fn fence(task: &ClaimedTask, worker_id: &Ulid) -> NodeLeaseFence {
        NodeLeaseFence {
            tenant_id: task.tenant_id.clone(),
            task_id: task.task_id.clone(),
            run_id: task.run_id.clone(),
            node_id: task.node_id.clone(),
            worker_id: worker_id.clone(),
            lease_id: task.lease_id.clone(),
            attempt: task.attempt,
            execution_identity_digest: task.execution_identity_digest.clone(),
        }
    }

    async fn trusted_scope(
        &self,
        task: &ClaimedTask,
        worker_id: &Ulid,
    ) -> Result<AccessScope, WorkerError> {
        let owner: Option<String> = sqlx::query_scalar(
            "SELECT identity.owner_id::text
             FROM research.execution_identities identity
             JOIN research.execution_tasks task
               ON task.tenant_id=identity.tenant_id AND task.run_id=identity.run_id
             WHERE task.tenant_id=$1 AND task.task_id=$2 AND task.run_id=$3
               AND task.node_id=$4 AND task.state='LEASED'
               AND task.lease_owner=$5 AND task.lease_id=$6
               AND task.claim_count=$7 AND task.lease_expires_at>CURRENT_TIMESTAMP
               AND identity.execution_identity_digest=$8",
        )
        .bind(task.tenant_id.as_str())
        .bind(task.task_id.as_str())
        .bind(task.run_id.as_str())
        .bind(task.node_id.as_str())
        .bind(worker_id.as_str())
        .bind(task.lease_id.as_str())
        .bind(i64::try_from(task.attempt).map_err(|_| invalid("attempt overflow"))?)
        .bind(S3BlobStore::hash_hex(&task.execution_identity_digest))
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| backend(WorkerStep::Load, true))?;
        let owner = owner
            .ok_or_else(|| invalid("active fenced identity not found"))
            .and_then(|value| Ulid::new(value).map_err(|_| invalid("invalid persisted owner")))?;
        AccessScope::new(task.tenant_id.clone(), worker_id.clone(), vec![owner])
            .map_err(|_| invalid("invalid trusted access scope"))
    }

    fn event_id(task: &ClaimedTask, event: &[u8]) -> Ulid {
        derived_id(
            b"ficant/worker-node-event/v1",
            &[
                task.tenant_id.as_str().as_bytes(),
                task.run_id.as_str().as_bytes(),
                task.node_id.as_str().as_bytes(),
                &task.attempt.to_be_bytes(),
                event,
            ],
        )
    }

    async fn read_verified_artifact(
        &self,
        scope: &AccessScope,
        artifact_id: &Ulid,
        expected_hash: Option<&ContentHash>,
        trace: SafeTraceContext,
    ) -> Result<(Artifact, Vec<u8>), WorkerError> {
        let artifact = self
            .repository
            .get_metadata(scope, artifact_id.clone())
            .await
            .map_err(|error| application_error(WorkerStep::ReadInput, &error))?
            .ok_or_else(|| invalid("input artifact metadata missing"))?;
        if artifact.id() != artifact_id
            || artifact.owner().tenant_id() != scope.tenant_id()
            || expected_hash.is_some_and(|expected| artifact.content_hash() != expected)
        {
            return Err(invalid("input artifact metadata mismatch"));
        }
        let request = RequiredVerifiedBlobRead::new(
            scope.clone(),
            artifact.owner().clone(),
            VerifiedReadResourceKind::Artifact,
            artifact.id().clone(),
            VerifiedBlobRole::ArtifactPayload,
            artifact.content_hash().clone(),
            artifact.blob_size(),
            trace,
        )
        .map_err(|_| invalid("required input read"))?;
        let payload = self
            .blobs
            .read_required(&request, &self.integrity_events)
            .await
            .map_err(|error| application_error(WorkerStep::ReadInput, &error))?;
        Ok((artifact, payload.bytes().to_vec()))
    }

    async fn read_and_verify_journal(
        &self,
        scope: &AccessScope,
        task: &ClaimedTask,
        graph: &ficant_domain::research::ResearchGraph,
    ) -> Result<Vec<RunJournal>, WorkerError> {
        let mut events = Vec::new();
        let mut cursor = None;
        loop {
            let request = PageRequest::new(scope.clone(), cursor, PageRequest::MAX_LIMIT)
                .map_err(|_| invalid("journal page request"))?;
            let page = self
                .repository
                .read(scope, task.run_id.clone(), request)
                .await
                .map_err(|error| application_error(WorkerStep::Load, &error))?;
            let (items, next) = page.into_parts();
            if items.is_empty() && next.is_some() {
                return Err(invalid("empty journal page with continuation"));
            }
            events.extend(items);
            match next {
                Some(value) => cursor = Some(value),
                None => break,
            }
        }
        let replay =
            replay_graph_execution(graph, &events).map_err(|_| invalid("journal replay failed"))?;
        if replay.run_id() != &task.run_id
            || replay.run_state() != RunState::Running
            || replay.resume_node() != Some(&task.node_id)
        {
            return Err(invalid("claimed task disagrees with journal replay"));
        }
        Ok(events)
    }
}

#[async_trait]
impl WorkerBackend for ProductionWorkerBackend {
    async fn claim(
        &self,
        worker_id: &Ulid,
        lease_id: &Ulid,
        lease_seconds: u32,
    ) -> Result<Option<ClaimedTask>, WorkerError> {
        let task = self
            .queue
            .claim_next(worker_id, lease_id, lease_seconds)
            .await
            .map_err(|error| lease_error(WorkerStep::Load, &error))?;
        Ok(task.map(|task| ClaimedTask {
            tenant_id: task.tenant_id().clone(),
            task_id: task.task_id().clone(),
            run_id: task.run_id().clone(),
            node_id: task.node_id().clone(),
            graph_digest: task.graph_digest().clone(),
            execution_identity_digest: task.execution_identity_digest().clone(),
            planned_artifact_id: task.planned_artifact_id().clone(),
            lease_id: lease_id.clone(),
            attempt: task.claim_count(),
        }))
    }

    async fn renew(
        &self,
        task: &ClaimedTask,
        worker_id: &Ulid,
        lease_seconds: u32,
    ) -> Result<(), WorkerError> {
        self.queue
            .renew(
                &task.tenant_id,
                &task.task_id,
                worker_id,
                &task.lease_id,
                lease_seconds,
            )
            .await
            .map(|_| ())
            .map_err(|error| lease_error(WorkerStep::Renew, &error))
    }

    async fn load(&self, task: &ClaimedTask, worker_id: &Ulid) -> Result<LoadedTask, WorkerError> {
        let scope = self.trusted_scope(task, worker_id).await?;
        let stored_identity = self
            .repository
            .load_execution_identity(&scope, &task.run_id)
            .await
            .map_err(|error| application_error(WorkerStep::Load, &error))?
            .ok_or_else(|| invalid("execution identity not found"))?;
        let graph = self
            .repository
            .load_graph(
                &scope,
                &stored_identity.graph_id,
                stored_identity.graph_version,
            )
            .await
            .map_err(|error| application_error(WorkerStep::Load, &error))?
            .ok_or_else(|| invalid("research graph not found"))?;
        self.read_and_verify_journal(&scope, task, &graph).await?;
        Ok(LoadedTask {
            owner: stored_identity.owner.clone(),
            graph,
            stored_identity,
        })
    }

    async fn begin(&self, task: &ClaimedTask, worker_id: &Ulid) -> Result<(), WorkerError> {
        self.repository
            .begin_node(BeginNode {
                fence: Self::fence(task, worker_id),
                started_event_id: Self::event_id(task, b"started"),
            })
            .await
            .map(|_| ())
            .map_err(|error| application_error(WorkerStep::Begin, &error))
    }

    async fn read_inputs(
        &self,
        task: &ClaimedTask,
        loaded: &LoadedTask,
    ) -> Result<PreparedInputs, WorkerError> {
        let identity = loaded.stored_identity.identity.reproducibility();
        let mut values = Vec::new();
        let mut evidence = Vec::new();
        let scope = AccessScope::new(
            task.tenant_id.clone(),
            task.lease_id.clone(),
            vec![loaded.owner.owner_id().clone()],
        )
        .map_err(|_| invalid("input access scope"))?;
        for binding in loaded
            .graph
            .external_input_bindings()
            .iter()
            .filter(|binding| binding.to_node() == &task.node_id)
        {
            let frozen = identity
                .external_inputs()
                .iter()
                .find(|input| input.input_id() == binding.input_id())
                .ok_or_else(|| invalid("external input identity missing"))?;
            let artifact = loaded
                .stored_identity
                .external_input_artifacts
                .iter()
                .find(|artifact| artifact.input_id == binding.input_id())
                .ok_or_else(|| invalid("external input artifact missing"))?;
            if artifact.content_hash != *frozen.content_hash() {
                return Err(invalid("external input content hash mismatch"));
            }
            let (metadata, payload) = self
                .read_verified_artifact(
                    &scope,
                    &artifact.artifact_id,
                    Some(frozen.content_hash()),
                    trace_id(task, binding.input_id().as_bytes())?,
                )
                .await?;
            values.push(
                NativePortValue::new(binding.to_port(), frozen.value_type().clone(), payload)
                    .map_err(|_| invalid("external input type or hash mismatch"))?,
            );
            evidence.push(InputEvidence {
                target_port: binding.to_port().to_owned(),
                value_type: frozen.value_type().clone(),
                artifact_id: metadata.id().clone(),
                content_hash: metadata.content_hash().clone(),
                source: InputSource::External {
                    input_id: binding.input_id().to_owned(),
                },
            });
        }
        for edge in loaded
            .graph
            .edges()
            .iter()
            .filter(|edge| edge.to_node() == &task.node_id)
        {
            let artifact_id = stable_node_artifact_id(identity.digest(), edge.from_node());
            let (metadata, bytes) = self
                .read_verified_artifact(
                    &scope,
                    &artifact_id,
                    None,
                    trace_id(task, edge.from_port().as_bytes())?,
                )
                .await?;
            let outputs = decode_canonical_output_bytes(&bytes, Some(metadata.content_hash()))
                .map_err(|_| invalid("upstream output envelope invalid"))?;
            let output = outputs
                .into_iter()
                .find(|output| output.port_name() == edge.from_port())
                .ok_or_else(|| invalid("upstream output port missing"))?;
            values.push(
                NativePortValue::new(
                    edge.to_port(),
                    output.value_type().clone(),
                    output.payload().to_vec(),
                )
                .map_err(|_| invalid("upstream output binding invalid"))?,
            );
            evidence.push(InputEvidence {
                target_port: edge.to_port().to_owned(),
                value_type: output.value_type().clone(),
                artifact_id: metadata.id().clone(),
                content_hash: metadata.content_hash().clone(),
                source: InputSource::Upstream {
                    node_id: edge.from_node().clone(),
                    port_name: edge.from_port().to_owned(),
                },
            });
        }
        Ok(PreparedInputs { values, evidence })
    }

    async fn execute(
        &self,
        task: &ClaimedTask,
        loaded: &LoadedTask,
        inputs: PreparedInputs,
    ) -> Result<ExecutedNode, WorkerError> {
        let node = loaded
            .graph
            .nodes()
            .iter()
            .find(|node| node.node_id() == &task.node_id)
            .cloned()
            .ok_or_else(|| invalid("claimed node not found"))?;
        let identity = loaded.stored_identity.identity.reproducibility().clone();
        let node_id = task.node_id.clone();
        let input_artifacts = inputs
            .evidence
            .iter()
            .map(|input| input.content_hash.clone())
            .collect();
        let input_evidence = inputs.evidence;
        let values = inputs.values;
        let execution = tokio::task::spawn_blocking(move || {
            if node.node_id() != &node_id {
                return Err(invalid("native node registry mismatch"));
            }
            let executor =
                trusted_native_node(&node).map_err(|_| invalid("native node registry mismatch"))?;
            execute_native_node(&node, &identity, &executor, values, input_artifacts)
                .map_err(|_| backend(WorkerStep::Execute, false))
        })
        .await
        .map_err(|_| backend(WorkerStep::Execute, false))??;
        Ok(ExecutedNode {
            outputs: execution.outputs().to_vec(),
            output_envelope: execution.output_envelope().to_vec(),
            output_envelope_hash: execution.artifact().output_envelope_hash().clone(),
            input_evidence,
        })
    }

    async fn promote(
        &self,
        task: &ClaimedTask,
        loaded: &LoadedTask,
        execution: ExecutedNode,
    ) -> Result<NodeCompletion, WorkerError> {
        let bytes = execution.output_envelope.clone();
        let size = u64::try_from(bytes.len()).map_err(|_| invalid("output too large"))?;
        let content_hash = execution.output_envelope_hash.clone();
        let scope = AccessScope::new(
            task.tenant_id.clone(),
            task.lease_id.clone(),
            vec![loaded.owner.owner_id().clone()],
        )
        .map_err(|_| invalid("output access scope"))?;
        let key = IdempotencyKey::new(format!(
            "phase4-node-output/{}/{}/{}",
            task.run_id, task.node_id, task.attempt
        ))
        .map_err(|_| invalid("output idempotency key"))?;
        let staged = self
            .blobs
            .begin_stage(
                BeginBlobStage::new(scope.clone(), loaded.owner.clone(), size, key)
                    .map_err(|_| invalid("output stage"))?,
            )
            .await
            .map_err(|error| application_error(WorkerStep::Promote, &error))?;
        self.blobs
            .append_chunk(&scope, &staged, bytes.clone())
            .await
            .map_err(|error| application_error(WorkerStep::Promote, &error))?;
        let verified_blob = self
            .blobs
            .verify_and_promote(
                VerifyBlobStage::new(scope, staged, content_hash.clone(), size)
                    .map_err(|_| invalid("output verification"))?,
            )
            .await
            .map_err(|error| application_error(WorkerStep::Promote, &error))?;
        let lineage = execution
            .input_evidence
            .iter()
            .map(|input| {
                LineageRef::content_addressed(input.artifact_id.clone(), input.content_hash.clone())
            })
            .collect::<Vec<_>>();
        let artifact = Artifact::new(
            task.planned_artifact_id.clone(),
            loaded.owner.clone(),
            ArtifactKind::Generic,
            OUTPUT_MEDIA_TYPE,
            content_hash,
            size,
            lineage,
        )
        .map_err(|_| invalid("output artifact"))?;
        let output_manifest = output_manifest(task, loaded, &execution, &artifact)?;
        Ok(NodeCompletion {
            artifact,
            verified_blob,
            verified_payload: bytes,
            output_manifest,
        })
    }

    async fn complete(
        &self,
        task: &ClaimedTask,
        worker_id: &Ulid,
        completion: NodeCompletion,
    ) -> Result<(), WorkerError> {
        self.repository
            .complete_node(CompleteNode {
                fence: Self::fence(task, worker_id),
                artifact: completion.artifact,
                verified_blob: completion.verified_blob,
                verified_payload: completion.verified_payload,
                output_manifest: completion.output_manifest,
                succeeded_event_id: Self::event_id(task, b"succeeded"),
                checkpoint_event_id: Self::event_id(task, b"checkpointed"),
            })
            .await
            .map(|_| ())
            .map_err(|error| application_error(WorkerStep::Complete, &error))
    }

    async fn fail(
        &self,
        task: &ClaimedTask,
        worker_id: &Ulid,
        failure_hash: ContentHash,
    ) -> Result<(), WorkerError> {
        self.repository
            .fail_node(FailNode {
                fence: Self::fence(task, worker_id),
                failure_hash,
                failed_event_id: Self::event_id(task, b"failed"),
            })
            .await
            .map(|_| ())
            .map_err(|error| application_error(WorkerStep::Fail, &error))
    }
}

fn output_manifest(
    task: &ClaimedTask,
    loaded: &LoadedTask,
    execution: &ExecutedNode,
    artifact: &Artifact,
) -> Result<Vec<u8>, WorkerError> {
    let node = loaded
        .graph
        .nodes()
        .iter()
        .find(|node| node.node_id() == &task.node_id)
        .ok_or_else(|| invalid("claimed node not found"))?;
    let reproducibility = loaded.stored_identity.identity.reproducibility();
    let implementation = reproducibility
        .node_implementations()
        .iter()
        .find(|binding| binding.node_id == task.node_id)
        .ok_or_else(|| invalid("node implementation missing"))?;
    let outputs = execution
        .outputs
        .iter()
        .map(|output| research_pb::NodeOutputBinding {
            port_name: output.port_name().to_owned(),
            value_type: Some(typed_value(output.value_type())),
            artifact: Some(lineage_ref(artifact.id(), artifact.content_hash())),
            content_hash: Some(hash(output.content_hash())),
        })
        .collect();
    let inputs = execution
        .input_evidence
        .iter()
        .map(|input| {
            let declared_source = match &input.source {
                InputSource::External { input_id } => {
                    research_pb::node_input_binding::DeclaredSource::ExternalInputId(
                        input_id.clone(),
                    )
                }
                InputSource::Upstream { node_id, port_name } => {
                    research_pb::node_input_binding::DeclaredSource::UpstreamOutput(
                        research_pb::UpstreamNodeOutput {
                            node_id: Some(id(node_id)),
                            port_name: port_name.clone(),
                        },
                    )
                }
            };
            research_pb::NodeInputBinding {
                node_id: Some(id(&task.node_id)),
                port_name: input.target_port.clone(),
                value_type: Some(typed_value(&input.value_type)),
                resolved_artifact: Some(lineage_ref(&input.artifact_id, &input.content_hash)),
                content_hash: Some(hash(&input.content_hash)),
                declared_source: Some(declared_source),
            }
        })
        .collect();
    let mut content = research_pb::NodeOutputManifestContent {
        reproducibility_digest: Some(hash(reproducibility.digest())),
        node_id: Some(id(&task.node_id)),
        node_contract_digest: Some(hash(node.contract().digest())),
        implementation_digest: Some(hash(&implementation.implementation_digest)),
        inputs,
        outputs,
        manifest_hash: None,
    };
    content.manifest_hash = Some(hash(&ContentHash::digest(&content.encode_to_vec())));
    let manifest = research_pb::NodeOutputManifest {
        execution: Some(execution_identity(loaded)),
        attempt: u32::try_from(task.attempt).map_err(|_| invalid("attempt overflow"))?,
        content: Some(content),
    };
    Ok(manifest.encode_to_vec())
}

fn trace_id(task: &ClaimedTask, discriminator: &[u8]) -> Result<SafeTraceContext, WorkerError> {
    use std::fmt::Write;

    let mut bytes = b"ficant/worker-read-trace/v1".to_vec();
    bytes.extend_from_slice(task.task_id.as_str().as_bytes());
    bytes.extend_from_slice(&task.attempt.to_be_bytes());
    bytes.extend_from_slice(discriminator);
    let digest = ContentHash::digest(&bytes);
    let mut value = String::with_capacity(32);
    for byte in &digest.as_bytes()[..16] {
        write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
    }
    SafeTraceContext::new(value).map_err(|_| invalid("safe trace id"))
}

fn execution_identity(loaded: &LoadedTask) -> research_pb::ExecutionInstanceIdentity {
    let identity = &loaded.stored_identity.identity;
    let reproducibility = identity.reproducibility();
    let external_inputs = reproducibility
        .external_inputs()
        .iter()
        .map(|input| {
            let artifact = loaded
                .stored_identity
                .external_input_artifacts
                .iter()
                .find(|artifact| artifact.input_id == input.input_id());
            research_pb::ExecutionExternalInput {
                input_id: input.input_id().to_owned(),
                value_type: Some(typed_value(input.value_type())),
                resolved_artifact: artifact
                    .map(|value| lineage_ref(&value.artifact_id, &value.content_hash)),
                content_hash: Some(hash(input.content_hash())),
            }
        })
        .collect();
    research_pb::ExecutionInstanceIdentity {
        run_id: Some(id(identity.run_id())),
        reproducibility: Some(research_pb::ReproducibilityIdentity {
            graph_digest: Some(hash(reproducibility.graph_digest())),
            data_snapshot_hash: Some(hash(reproducibility.data_snapshot_hash())),
            universe_snapshot_hash: Some(hash(reproducibility.universe_snapshot_hash())),
            parameters_hash: Some(hash(reproducibility.parameters_hash())),
            runtime_image_digest: Some(hash(reproducibility.runtime_image_digest())),
            environment_digest: Some(hash(reproducibility.environment_digest())),
            seed: reproducibility.seed(),
            rule_packs: reproducibility
                .rule_pack_bindings()
                .iter()
                .map(|binding| research_pb::RulePackBinding {
                    rule_pack_id: Ulid::new(&binding.rule_pack_id)
                        .ok()
                        .map(|value| id(&value)),
                    version: binding.version.get(),
                    content_hash: Some(hash(&binding.content_hash)),
                })
                .collect(),
            node_implementations: reproducibility
                .node_implementations()
                .iter()
                .map(|binding| research_pb::NodeImplementationBinding {
                    node_id: Some(id(&binding.node_id)),
                    implementation_digest: Some(hash(&binding.implementation_digest)),
                })
                .collect(),
            external_inputs,
            digest: Some(hash(reproducibility.digest())),
        }),
        digest: Some(hash(identity.digest())),
    }
}

fn typed_value(value: &ficant_domain::research::TypedValue) -> research_pb::TypedValue {
    research_pb::TypedValue {
        type_id: value.type_id().to_owned(),
        type_version: value.type_version().get(),
        schema_hash: Some(hash(value.schema_hash())),
    }
}

fn id(value: &Ulid) -> core_pb::Ulid {
    core_pb::Ulid {
        value: value.as_str().to_owned(),
    }
}

fn hash(value: &ContentHash) -> core_pb::Sha256 {
    core_pb::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn lineage_ref(object_id: &Ulid, content_hash: &ContentHash) -> core_pb::LineageRef {
    core_pb::LineageRef {
        object_id: Some(id(object_id)),
        version: 0,
        content_hash: Some(hash(content_hash)),
    }
}

fn lease_error(step: WorkerStep, error: &LeaseQueueError) -> WorkerError {
    let retryable = match error {
        LeaseQueueError::StorageUnavailable | LeaseQueueError::Conflict => true,
        LeaseQueueError::InvalidValue | LeaseQueueError::NotFound => false,
    };
    backend(step, retryable)
}

fn application_error(
    step: WorkerStep,
    error: &ficant_application::ApplicationError,
) -> WorkerError {
    backend(step, error.retryable())
}

const fn backend(step: WorkerStep, retryable: bool) -> WorkerError {
    WorkerError::Backend { step, retryable }
}

const fn invalid(reason: &'static str) -> WorkerError {
    WorkerError::InvalidTask(reason)
}
