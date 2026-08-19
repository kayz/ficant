use std::sync::Arc;

use ficant_application::ports::{
    AccessScope, AeadCursorCodec, ArtifactRepository, AuthorizedPrincipal, ComparisonDimension,
    CreateExperimentRun, Cursor, DefinitionRepository, DefinitionValue, ExecutionExternalInput,
    ExperimentRepository, ExternalInputArtifactBinding, GraphRunRecord, IdempotencyKey,
    IntegrityEventSink, MarketRunRulePackResolver, NodeImplementation, OutputTrace, PageRequest,
    Phase4ExecutionRepository, ReproducibilityIdentity, ReproducibilityIdentityInput,
    RulePackBinding, RunJournalRepository, SafeTraceContext, SnapshotRepository,
    SnapshotVerifiedReadMetadataRepository, StoredNodeManifest, TransitionExperimentRun,
    VerifiedBlobReader,
};
use ficant_application::use_cases::phase4_submission::{Phase4Submission, PreparedGraphSubmission};
use ficant_application::use_cases::verified_reads::{VerifiedReadFacade, VerifiedSnapshotRead};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1 as core_pb;
use ficant_contracts::ficant::research::v1 as research_pb;
use ficant_contracts::ficant::research::v1::experiment_service_server::ExperimentService;
use ficant_domain::ContentAddressed;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, LineageRef, OwnerRef, Ulid, Version, VersionRef};
use ficant_domain::research::{
    DeterminismClass, ExperimentRun, ExperimentRunInput, FilesystemPermission, GraphExternalInput,
    GraphExternalInputBinding, JournalEventType, NodePermissions, PortType, ResearchEdge,
    ResearchGraph, ResearchGraphInput, ResearchNode, ResearchNodeContract,
    ResearchNodeContractInput, ResourceLimits, RunJournal, RunState, TypedValue,
};
use ficant_runtime::{NativePortValue, decode_canonical_output_bytes};
use prost::Message;
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;

const READ_SCOPE: &str = "experiment:read";
const WRITE_SCOPE: &str = "experiment:write";
const MAX_OBSERVED_OUTPUT_ENVELOPE_BYTES: u64 = 1_048_576;

pub trait TrustedNodeCatalog: Send + Sync {
    fn native_source_digest(&self) -> ContentHash;

    /// Resolves the deployment-trusted implementation digest for a graph node.
    ///
    /// # Errors
    ///
    /// Returns an application error when the node is not present in the trusted catalog or its
    /// contract does not match the registered native implementation.
    fn implementation_digest(&self, node: &ResearchNode) -> Result<ContentHash, ApplicationError>;
}

#[derive(Clone)]
pub struct TrustedExperimentScope {
    runtime_image_digest: ContentHash,
    environment_attestation: String,
    environment_digest: ContentHash,
    native_source_digest: ContentHash,
}

impl TrustedExperimentScope {
    /// Builds the deployment-owned runtime attestation. None of these values are accepted from an
    /// RPC request. The legacy identity parameters remain temporarily for constructor-call-site
    /// compatibility, but authorization is derived exclusively from each request principal.
    ///
    /// # Errors
    ///
    /// Returns an application error when the environment attestation is invalid.
    pub fn new(
        _tenant_id: Ulid,
        _owner_id: Ulid,
        _actor_id: Ulid,
        runtime_image_digest: ContentHash,
        environment_attestation: String,
        native_source_digest: ContentHash,
    ) -> Result<Self, ApplicationError> {
        validate_environment_attestation(&environment_attestation)?;
        let environment_digest = ContentHash::digest(environment_attestation.as_bytes());
        Ok(Self {
            runtime_image_digest,
            environment_attestation,
            environment_digest,
            native_source_digest,
        })
    }

    #[must_use]
    pub fn environment_attestation(&self) -> &str {
        &self.environment_attestation
    }

    #[must_use]
    pub fn native_source_digest(&self) -> &ContentHash {
        &self.native_source_digest
    }
}

#[derive(Clone)]
pub struct ExperimentGrpcService {
    platform: Arc<dyn PlatformPort>,
    experiments: Arc<dyn ExperimentRepository>,
    journals: Arc<dyn RunJournalRepository>,
    snapshot_repository: Arc<dyn SnapshotRepository>,
    cursor_codec: Arc<AeadCursorCodec>,
    phase4: Arc<dyn Phase4ExecutionRepository>,
    artifacts: Arc<dyn ArtifactRepository>,
    definitions: Arc<dyn DefinitionRepository>,
    snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
    blobs: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
    catalog: Arc<dyn TrustedNodeCatalog>,
    trusted: TrustedExperimentScope,
    errors: CoreBusinessErrorMapper,
}

impl ExperimentGrpcService {
    /// Builds an experiment service from deployment-owned adapters and trust settings.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured native source digest differs from the compiled
    /// catalog, or when the trace-key-backed error mapper cannot be initialized.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        platform: Arc<dyn PlatformPort>,
        experiments: Arc<dyn ExperimentRepository>,
        journals: Arc<dyn RunJournalRepository>,
        snapshot_repository: Arc<dyn SnapshotRepository>,
        cursor_codec: Arc<AeadCursorCodec>,
        phase4: Arc<dyn Phase4ExecutionRepository>,
        artifacts: Arc<dyn ArtifactRepository>,
        definitions: Arc<dyn DefinitionRepository>,
        snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
        blobs: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
        catalog: Arc<dyn TrustedNodeCatalog>,
        trusted: TrustedExperimentScope,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        if catalog.native_source_digest() != *trusted.native_source_digest() {
            return Err("trusted native source digest does not match the compiled catalog");
        }
        Ok(Self {
            platform,
            experiments,
            journals,
            snapshot_repository,
            cursor_codec,
            phase4,
            artifacts,
            definitions,
            snapshots,
            blobs,
            integrity_events,
            catalog,
            trusted,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn authorize(
        &self,
        metadata: &tonic::metadata::MetadataMap,
        scope: &str,
    ) -> Result<AuthorizedPrincipal, Status> {
        let session = self
            .platform
            .current_session(&request_credential(metadata))
            .map_err(|_| Status::unauthenticated("当前身份未通过认证"))?;
        let principal = session
            .authorized_principal()
            .map_err(|_| Status::permission_denied("当前身份无权执行此操作"))?;
        require_experiment_access(principal, scope)
    }

    fn status(&self, operation: &str, error: &ApplicationError) -> Status {
        self.errors.status(operation, operation, error)
    }
}

fn require_experiment_access(
    principal: AuthorizedPrincipal,
    scope: &str,
) -> Result<AuthorizedPrincipal, Status> {
    if principal.active_role() != PlatformRole::Researcher || !principal.has_scope(scope) {
        return Err(Status::permission_denied("当前身份无权执行此操作"));
    }
    Ok(principal)
}

#[tonic::async_trait]
impl ExperimentService for ExperimentGrpcService {
    async fn create_run(
        &self,
        request: Request<research_pb::CreateRunRequest>,
    ) -> Result<Response<research_pb::CreateRunResponse>, Status> {
        let principal = self.authorize(request.metadata(), WRITE_SCOPE)?;
        let value = request.into_inner();
        let run = legacy_run_from_proto(
            value
                .run
                .ok_or_else(|| Status::invalid_argument("run 缺失"))?,
            &self.trusted,
            &principal,
        )
        .map_err(|error| self.status("experiment-create-run", &error))?;
        let validated = MarketRunRulePackResolver::new(
            self.definitions.as_ref(),
            self.snapshot_repository.as_ref(),
        )
        .resolve(principal.access_scope(), run)
        .await
        .map_err(|error| self.status("experiment-create-run", &error))?;
        let command = CreateExperimentRun::new(
            principal.access_scope().clone(),
            validated,
            IdempotencyKey::new(value.idempotency_key)
                .map_err(|error| self.status("experiment-create-run", &error))?,
        )
        .map_err(|error| self.status("experiment-create-run", &error))?;
        let run = self
            .experiments
            .create_run(command)
            .await
            .map_err(|error| self.status("experiment-create-run", &error))?;
        Ok(Response::new(research_pb::CreateRunResponse {
            run: Some(run_to_proto(&run)),
        }))
    }

    async fn transition_run(
        &self,
        request: Request<research_pb::TransitionRunRequest>,
    ) -> Result<Response<research_pb::TransitionRunResponse>, Status> {
        let principal = self.authorize(request.metadata(), WRITE_SCOPE)?;
        let value = request.into_inner();
        let run_id = parse_ulid(value.run_id)?;
        let next_state = legacy_run_state(value.next_state)?;
        let current = self
            .experiments
            .get_run(principal.access_scope(), run_id.clone())
            .await
            .map_err(|error| self.status("experiment-transition-run", &error))?
            .ok_or_else(|| self.status("experiment-transition-run", &not_found()))?;
        let key = format!(
            "legacy-transition/{}/{}/{}",
            run_id.as_str(),
            value.expected_revision,
            value.next_state
        );
        let command = TransitionExperimentRun::new(
            principal.access_scope().clone(),
            current.owner().clone(),
            run_id,
            value.expected_revision,
            next_state,
            IdempotencyKey::new(key)
                .map_err(|error| self.status("experiment-transition-run", &error))?,
        )
        .map_err(|error| self.status("experiment-transition-run", &error))?;
        let run = self
            .experiments
            .transition(command)
            .await
            .map_err(|error| self.status("experiment-transition-run", &error))?;
        Ok(Response::new(research_pb::TransitionRunResponse {
            run: Some(run_to_proto(&run)),
        }))
    }

    async fn get_run(
        &self,
        request: Request<research_pb::GetRunRequest>,
    ) -> Result<Response<research_pb::GetRunResponse>, Status> {
        let principal = self.authorize(request.metadata(), READ_SCOPE)?;
        let run_id = parse_ulid(request.into_inner().run_id)?;
        let run = self
            .experiments
            .get_run(principal.access_scope(), run_id)
            .await
            .map_err(|error| self.status("experiment-get-run", &error))?
            .ok_or_else(|| self.status("experiment-get-run", &not_found()))?;
        Ok(Response::new(research_pb::GetRunResponse {
            run: Some(run_to_proto(&run)),
        }))
    }

    async fn read_run_journal(
        &self,
        request: Request<research_pb::ReadRunJournalRequest>,
    ) -> Result<Response<research_pb::ReadRunJournalResponse>, Status> {
        let principal = self.authorize(request.metadata(), READ_SCOPE)?;
        let value = request.into_inner();
        let run_id = parse_ulid(value.run_id)?;
        let requested_page = value.page.unwrap_or(core_pb::PageRequest {
            page_size: 100,
            cursor: String::new(),
        });
        let limit = if requested_page.page_size == 0 {
            100
        } else {
            requested_page.page_size
        };
        if !requested_page.cursor.is_empty() && value.from_sequence != 0 {
            return Err(Status::invalid_argument(
                "cursor 与 from_sequence 不得同时提供",
            ));
        }
        let cursor = if requested_page.cursor.is_empty() {
            (value.from_sequence != 0)
                .then(|| {
                    Cursor::issue(
                        self.cursor_codec.as_ref(),
                        principal.access_scope(),
                        value.from_sequence.to_string(),
                    )
                })
                .transpose()
        } else {
            Cursor::resume(
                self.cursor_codec.as_ref(),
                principal.access_scope(),
                requested_page.cursor,
            )
            .map(Some)
        }
        .map_err(|error| self.status("experiment-read-journal", &error))?;
        let page_request = PageRequest::new(principal.access_scope().clone(), cursor, limit)
            .map_err(|error| self.status("experiment-read-journal", &error))?;
        let page = self
            .journals
            .read(principal.access_scope(), run_id, page_request)
            .await
            .map_err(|error| self.status("experiment-read-journal", &error))?;
        let next_cursor = page
            .next_cursor()
            .map_or_else(String::new, |cursor| cursor.as_str().to_owned());
        Ok(Response::new(research_pb::ReadRunJournalResponse {
            events: page.items().iter().map(journal_to_proto).collect(),
            page: Some(core_pb::PageResponse { next_cursor }),
        }))
    }

    async fn submit_graph_run(
        &self,
        request: Request<research_pb::SubmitGraphRunRequest>,
    ) -> Result<Response<research_pb::SubmitGraphRunResponse>, Status> {
        let principal = self.authorize(request.metadata(), WRITE_SCOPE)?;
        let trace = trace_context(request.get_ref());
        let parsed = self
            .prepare_submission(request.into_inner(), trace, &principal)
            .await
            .map_err(|error| self.status("experiment-submit", &error))?;
        let record = Phase4Submission::new(self.phase4.as_ref())
            .submit(parsed)
            .await
            .map_err(|error| self.status("experiment-submit", &error))?;
        Ok(Response::new(research_pb::SubmitGraphRunResponse {
            run: Some(run_to_proto(&record.run)),
            execution: Some(execution_to_proto(&record.identity)?),
        }))
    }

    async fn get_graph_run(
        &self,
        request: Request<research_pb::GetGraphRunRequest>,
    ) -> Result<Response<research_pb::GetGraphRunResponse>, Status> {
        let principal = self.authorize(request.metadata(), READ_SCOPE)?;
        let run_id = parse_ulid(request.into_inner().run_id)?;
        let record = self
            .phase4
            .get_graph_run(principal.access_scope(), &run_id)
            .await
            .map_err(|error| self.status("experiment-get-graph-run", &error))?
            .ok_or_else(|| self.status("experiment-get-graph-run", &not_found()))?;
        Ok(Response::new(research_pb::GetGraphRunResponse {
            graph_run: Some(graph_run_to_proto(&record)?),
        }))
    }

    async fn list_node_output_manifests(
        &self,
        request: Request<research_pb::ListNodeOutputManifestsRequest>,
    ) -> Result<Response<research_pb::ListNodeOutputManifestsResponse>, Status> {
        let principal = self.authorize(request.metadata(), READ_SCOPE)?;
        let run_id = parse_ulid(request.into_inner().run_id)?;
        let manifests = self
            .phase4
            .list_node_manifests(principal.access_scope(), &run_id)
            .await
            .map_err(|error| self.status("experiment-list-manifests", &error))?
            .iter()
            .map(stored_manifest_to_proto)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Response::new(
            research_pb::ListNodeOutputManifestsResponse { manifests },
        ))
    }

    async fn trace_graph_output(
        &self,
        request: Request<research_pb::TraceGraphOutputRequest>,
    ) -> Result<Response<research_pb::TraceGraphOutputResponse>, Status> {
        let principal = self.authorize(request.metadata(), READ_SCOPE)?;
        let value = request.into_inner();
        let run_id = parse_ulid(value.run_id)?;
        let node_id = parse_ulid(value.node_id)?;
        let trace = self
            .phase4
            .trace_output(principal.access_scope(), &run_id, &node_id)
            .await
            .map_err(|error| self.status("experiment-trace-output", &error))?
            .ok_or_else(|| self.status("experiment-trace-output", &not_found()))?;
        Ok(Response::new(research_pb::TraceGraphOutputResponse {
            trace: Some(output_trace_to_proto(&trace)?),
        }))
    }

    async fn read_node_output(
        &self,
        request: Request<research_pb::ReadNodeOutputRequest>,
    ) -> Result<Response<research_pb::ReadNodeOutputResponse>, Status> {
        let principal = self.authorize(request.metadata(), READ_SCOPE)?;
        let trace_context = trace_context(request.get_ref());
        let value = request.into_inner();
        let run_id = parse_ulid(value.run_id)?;
        let node_id = parse_ulid(value.node_id)?;
        let trace = self
            .phase4
            .trace_output(principal.access_scope(), &run_id, &node_id)
            .await
            .map_err(|error| self.status("experiment-read-node-output", &error))?
            .ok_or_else(|| self.status("experiment-read-node-output", &not_found()))?;
        let mut selected = trace
            .manifests
            .iter()
            .filter(|stored| stored.node_id == node_id);
        let stored = selected
            .next()
            .ok_or_else(|| Status::data_loss("节点输出清单缺失"))?;
        if selected.next().is_some() {
            return Err(Status::data_loss("节点输出清单不唯一"));
        }
        if stored.artifact.blob_size() > MAX_OBSERVED_OUTPUT_ENVELOPE_BYTES {
            return Err(Status::resource_exhausted("节点输出超过观测接口大小上限"));
        }
        let reads = VerifiedReadFacade::new(
            self.artifacts.as_ref(),
            &NoSignals,
            self.snapshots.as_ref(),
            self.blobs.as_ref(),
            self.integrity_events.as_ref(),
        );
        let verified = reads
            .read_verified_artifact(
                principal.access_scope(),
                stored.artifact.id().clone(),
                trace_context,
            )
            .await
            .map_err(|error| self.status("experiment-read-node-output", &error))?;
        if verified.artifact() != &stored.artifact {
            return Err(Status::data_loss("节点输出 Artifact 与清单不一致"));
        }
        let verified_size = u64::try_from(verified.payload().bytes().len())
            .map_err(|_| Status::resource_exhausted("节点输出超过观测接口大小上限"))?;
        if verified_size > MAX_OBSERVED_OUTPUT_ENVELOPE_BYTES {
            return Err(Status::resource_exhausted("节点输出超过观测接口大小上限"));
        }
        let outputs = decode_canonical_output_bytes(
            verified.payload().bytes(),
            Some(stored.artifact.content_hash()),
        )
        .map_err(|_| Status::data_loss("节点输出 envelope 校验失败"))?;
        let stored_proto = stored_manifest_to_proto(stored)?;
        let manifest = stored_proto
            .manifest
            .ok_or_else(|| Status::data_loss("节点输出清单无法映射"))?;
        let observed = observed_outputs(&manifest, outputs)?;
        Ok(Response::new(research_pb::ReadNodeOutputResponse {
            manifest: Some(manifest),
            outputs: observed,
        }))
    }

    async fn compare_graph_runs(
        &self,
        request: Request<research_pb::CompareGraphRunsRequest>,
    ) -> Result<Response<research_pb::CompareGraphRunsResponse>, Status> {
        let principal = self.authorize(request.metadata(), READ_SCOPE)?;
        let value = request.into_inner();
        let left = parse_ulid(value.left_run_id)?;
        let right = parse_ulid(value.right_run_id)?;
        let comparison = self
            .phase4
            .compare_graph_runs(principal.access_scope(), &left, &right)
            .await
            .map_err(|error| self.status("experiment-compare-runs", &error))?
            .ok_or_else(|| self.status("experiment-compare-runs", &not_found()))?;
        Ok(Response::new(research_pb::CompareGraphRunsResponse {
            differing_dimensions: comparison
                .differing_dimensions
                .into_iter()
                .map(comparison_dimension)
                .collect(),
        }))
    }
}

impl ExperimentGrpcService {
    // Submission preparation deliberately keeps the complete trust boundary in one operation:
    // every caller-supplied lineage claim is verified before the repository transaction begins.
    #[allow(clippy::too_many_lines)]
    async fn prepare_submission(
        &self,
        request: research_pb::SubmitGraphRunRequest,
        trace: SafeTraceContext,
        principal: &AuthorizedPrincipal,
    ) -> Result<PreparedGraphSubmission, ApplicationError> {
        let run_id = parse_ulid_app(request.run_id)?;
        let graph = graph_from_proto(request.graph.ok_or_else(validation)?)?;
        let owner = graph.owner().clone();
        let scope = principal.access_scope();
        scope.authorize(&owner)?;
        let reads = VerifiedReadFacade::new(
            self.artifacts.as_ref(),
            &NoSignals,
            self.snapshots.as_ref(),
            self.blobs.as_ref(),
            self.integrity_events.as_ref(),
        );
        let data_ref = lineage_from_proto(request.data_snapshot.ok_or_else(validation)?)?;
        let universe_ref = lineage_from_proto(request.universe_snapshot.ok_or_else(validation)?)?;
        let claimed_data = parse_hash_app(request.data_snapshot_hash)?;
        let claimed_universe = parse_hash_app(request.universe_snapshot_hash)?;
        let data = reads
            .read_verified_snapshot(scope, data_ref.object_id().clone(), trace.clone())
            .await?;
        let universe = reads
            .read_verified_snapshot(scope, universe_ref.object_id().clone(), trace.clone())
            .await?;
        match data {
            VerifiedSnapshotRead::Data { snapshot, .. }
                if snapshot.owner() == &owner
                    && snapshot.id() == data_ref.object_id()
                    && data_ref.version().is_none()
                    && data_ref.content_hash() == Some(snapshot.content_hash())
                    && snapshot.content_hash() == &claimed_data => {}
            _ => return Err(lineage()),
        }
        match universe {
            VerifiedSnapshotRead::Universe { snapshot, .. }
                if snapshot.owner() == &owner
                    && snapshot.id() == universe_ref.object_id()
                    && universe_ref.version().is_none()
                    && universe_ref.content_hash() == Some(snapshot.content_hash())
                    && snapshot.content_hash() == &claimed_universe => {}
            _ => return Err(lineage()),
        }

        let mut rule_bindings = Vec::with_capacity(request.rule_packs.len());
        let mut run_rules = Vec::with_capacity(request.rule_packs.len());
        for binding in request.rule_packs {
            let id = parse_ulid_app(binding.rule_pack_id)?;
            let version = Version::new(binding.version).map_err(map_domain_error)?;
            let claimed = parse_hash_app(binding.content_hash)?;
            let value = self
                .definitions
                .get_version(scope, id.clone(), version)
                .await?
                .ok_or_else(not_found)?;
            let DefinitionValue::MarketRulePack(rule) = value else {
                return Err(lineage());
            };
            if rule.owner() != &owner || rule.content_hash() != &claimed {
                return Err(lineage());
            }
            rule_bindings.push(RulePackBinding {
                rule_pack_id: id.to_string(),
                version,
                content_hash: claimed,
            });
            run_rules.push(VersionRef::new(id, version));
        }

        let mut external_inputs = Vec::with_capacity(request.external_inputs.len());
        let mut artifact_bindings = Vec::with_capacity(request.external_inputs.len());
        for input in request.external_inputs {
            let artifact_ref = lineage_from_proto(input.resolved_artifact.ok_or_else(validation)?)?;
            let claimed = parse_hash_app(input.content_hash)?;
            let verified = reads
                .read_verified_artifact(scope, artifact_ref.object_id().clone(), trace.clone())
                .await?;
            if verified.artifact().owner() != &owner
                || verified.artifact().content_hash() != &claimed
                || artifact_ref.content_hash() != Some(&claimed)
            {
                return Err(lineage());
            }
            let value_type = typed_value_from_proto(input.value_type.ok_or_else(validation)?)?;
            external_inputs.push(
                ExecutionExternalInput::new(
                    input.input_id.clone(),
                    value_type,
                    verified.payload().bytes().to_vec(),
                )
                .map_err(|error| ficant_application::map_runtime_error(&error))?,
            );
            artifact_bindings.push(ExternalInputArtifactBinding {
                input_id: input.input_id,
                artifact_id: verified.artifact().id().clone(),
                content_hash: claimed,
            });
        }

        let mut implementations = Vec::with_capacity(graph.nodes().len());
        for node in graph.nodes() {
            implementations.push(NodeImplementation {
                node_id: node.node_id().clone(),
                implementation_digest: self.catalog.implementation_digest(node)?,
            });
        }
        let parameters_hash = parse_hash_app(request.parameters_hash)?;
        let reproducibility = ReproducibilityIdentity::new(
            &graph,
            ReproducibilityIdentityInput {
                external_inputs,
                data_snapshot_hash: claimed_data,
                universe_snapshot_hash: claimed_universe,
                parameters_hash: parameters_hash.clone(),
                runtime_image_digest: self.trusted.runtime_image_digest.clone(),
                environment_digest: self.trusted.environment_digest.clone(),
                seed: request.seed,
                rule_pack_bindings: rule_bindings,
                node_implementations: implementations,
            },
        )
        .map_err(|error| ficant_application::map_runtime_error(&error))?;
        let execution = ficant_application::ports::ExecutionInstanceIdentity::from_reproducibility(
            run_id.clone(),
            reproducibility,
        );
        Ok(PreparedGraphSubmission {
            idempotency_key: request.idempotency_key,
            scope: scope.clone(),
            owner,
            run_id,
            graph,
            data_snapshot: data_ref,
            universe_snapshot: universe_ref,
            rule_packs: run_rules,
            runtime_image_digest: self.trusted.runtime_image_digest.clone(),
            parameters_hash,
            seed: request.seed,
            execution,
            external_input_artifacts: artifact_bindings,
        })
    }
}

struct NoSignals;

#[tonic::async_trait]
impl ficant_application::ports::SignalRepository for NoSignals {
    async fn publish(
        &self,
        _command: ficant_application::ports::PublishSignalSet,
    ) -> Result<ficant_domain::research::SignalSet, ApplicationError> {
        Err(validation())
    }

    async fn get(
        &self,
        _scope: &AccessScope,
        _signal_id: Ulid,
    ) -> Result<Option<ficant_domain::research::SignalSet>, ApplicationError> {
        Err(validation())
    }
}

fn graph_from_proto(value: research_pb::ResearchGraph) -> Result<ResearchGraph, ApplicationError> {
    let claimed = parse_hash_app(value.digest)?;
    let owner = owner_from_proto(value.owner.ok_or_else(validation)?)?;
    let nodes = value
        .nodes
        .into_iter()
        .map(node_from_proto)
        .collect::<Result<Vec<_>, _>>()?;
    let edges = value
        .edges
        .into_iter()
        .map(|edge| {
            ResearchEdge::new(
                parse_ulid_app(edge.from_node_id)?,
                edge.from_port,
                parse_ulid_app(edge.to_node_id)?,
                edge.to_port,
            )
            .map_err(map_domain_error)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let external_inputs = value
        .external_inputs
        .into_iter()
        .map(|input| {
            GraphExternalInput::new(
                input.input_id,
                typed_value_from_proto(input.value_type.ok_or_else(validation)?)?,
            )
            .map_err(map_domain_error)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let external_bindings = value
        .external_input_bindings
        .into_iter()
        .map(|binding| {
            GraphExternalInputBinding::new(
                binding.input_id,
                parse_ulid_app(binding.to_node_id)?,
                binding.to_port,
            )
            .map_err(map_domain_error)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let graph = ResearchGraph::new_with_external_inputs(
        ResearchGraphInput {
            graph_id: parse_ulid_app(value.graph_id)?,
            version: Version::new(value.version).map_err(map_domain_error)?,
            owner,
            nodes,
            edges,
        },
        external_inputs,
        external_bindings,
    )
    .map_err(map_domain_error)?;
    let claimed_order = value
        .topological_order
        .into_iter()
        .map(|id| Ulid::new(id.value).map_err(map_domain_error))
        .collect::<Result<Vec<_>, _>>()?;
    if graph.digest() != &claimed || graph.topological_order() != claimed_order {
        return Err(hash_mismatch());
    }
    Ok(graph)
}

fn node_from_proto(value: research_pb::ResearchNode) -> Result<ResearchNode, ApplicationError> {
    let contract_value = value.contract.ok_or_else(validation)?;
    let claimed = parse_hash_app(contract_value.digest.clone())?;
    let permissions = contract_value.permissions.ok_or_else(validation)?;
    let limits = contract_value.resource_limits.ok_or_else(validation)?;
    let contract = ResearchNodeContract::new(ResearchNodeContractInput {
        contract_id: contract_value.contract_id,
        contract_version: Version::new(contract_value.contract_version)
            .map_err(map_domain_error)?,
        input_types: contract_value
            .input_types
            .into_iter()
            .map(port_from_proto)
            .collect::<Result<Vec<_>, _>>()?,
        output_types: contract_value
            .output_types
            .into_iter()
            .map(port_from_proto)
            .collect::<Result<Vec<_>, _>>()?,
        state_schema: parse_hash_app(contract_value.state_schema)?,
        parameter_schema: parse_hash_app(contract_value.parameter_schema)?,
        determinism_class: match research_pb::DeterminismClass::try_from(
            contract_value.determinism_class,
        ) {
            Ok(research_pb::DeterminismClass::Deterministic) => DeterminismClass::Deterministic,
            Ok(research_pb::DeterminismClass::Seeded) => DeterminismClass::Seeded,
            _ => return Err(validation()),
        },
        permissions: NodePermissions {
            network: permissions.network,
            database: permissions.database,
            filesystem: match research_pb::FilesystemPermission::try_from(permissions.filesystem) {
                Ok(research_pb::FilesystemPermission::None) => FilesystemPermission::None,
                Ok(research_pb::FilesystemPermission::TemporaryOnly) => {
                    FilesystemPermission::TemporaryOnly
                }
                _ => return Err(validation()),
            },
        },
        resource_limits: ResourceLimits::new(
            u16::try_from(limits.cpu_cores).map_err(|_| validation())?,
            limits.memory_mb,
            limits.timeout_seconds,
        )
        .map_err(map_domain_error)?,
        required_invariants: contract_value.required_invariants,
    })
    .map_err(map_domain_error)?;
    if contract.digest() != &claimed {
        return Err(hash_mismatch());
    }
    Ok(ResearchNode::new(
        parse_ulid_app(value.node_id)?,
        contract,
        parse_hash_app(value.parameters_hash)?,
    ))
}

fn port_from_proto(value: research_pb::PortType) -> Result<PortType, ApplicationError> {
    PortType::new(
        value.port_name,
        typed_value_from_proto(value.value_type.ok_or_else(validation)?)?,
    )
    .map_err(map_domain_error)
}

fn typed_value_from_proto(value: research_pb::TypedValue) -> Result<TypedValue, ApplicationError> {
    TypedValue::new(
        value.type_id,
        Version::new(value.type_version).map_err(map_domain_error)?,
        parse_hash_app(value.schema_hash)?,
    )
    .map_err(map_domain_error)
}

fn graph_run_to_proto(record: &GraphRunRecord) -> Result<research_pb::GraphRun, Status> {
    Ok(research_pb::GraphRun {
        run: Some(run_to_proto(&record.run)),
        graph: Some(graph_to_proto(&record.graph)),
        execution: Some(execution_to_proto(&record.identity)?),
    })
}

fn legacy_run_from_proto(
    value: research_pb::ExperimentRun,
    trusted: &TrustedExperimentScope,
    principal: &AuthorizedPrincipal,
) -> Result<ExperimentRun, ApplicationError> {
    if value.state != research_pb::RunState::Created as i32 || value.revision != 1 {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::StateConflict,
            false,
        ));
    }
    let owner = owner_from_proto(value.owner.ok_or_else(validation)?)?;
    principal.access_scope().authorize(&owner)?;
    let runtime_image_digest = parse_hash_app(value.runtime_image_digest)?;
    if runtime_image_digest != trusted.runtime_image_digest {
        return Err(forbidden());
    }
    ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: parse_ulid_app(value.experiment_run_id)?,
        owner,
        data_snapshot: lineage_from_proto(value.data_snapshot.ok_or_else(validation)?)?,
        universe_snapshot: lineage_from_proto(value.universe_snapshot.ok_or_else(validation)?)?,
        rule_packs: value
            .rule_packs
            .into_iter()
            .map(|reference| {
                Ok(VersionRef::new(
                    parse_ulid_app(reference.id)?,
                    Version::new(reference.version).map_err(map_domain_error)?,
                ))
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?,
        runtime_image_digest,
        parameters_hash: parse_hash_app(value.parameters_hash)?,
        seed: value.seed,
    })
    .map_err(map_domain_error)
}

fn legacy_run_state(value: i32) -> Result<RunState, Status> {
    match research_pb::RunState::try_from(value) {
        Ok(research_pb::RunState::Running) => Ok(RunState::Running),
        Ok(research_pb::RunState::Succeeded) => Ok(RunState::Succeeded),
        Ok(research_pb::RunState::Failed) => Ok(RunState::Failed),
        Ok(research_pb::RunState::Cancelled) => Ok(RunState::Cancelled),
        _ => Err(Status::invalid_argument("next_state 无效")),
    }
}

fn journal_to_proto(value: &RunJournal) -> research_pb::RunJournal {
    let instant = value.occurred_at().instant();
    research_pb::RunJournal {
        journal_event_id: Some(ulid_to_proto(value.id())),
        run_id: Some(ulid_to_proto(value.run_id())),
        sequence: value.sequence(),
        event_type: match value.event_type() {
            JournalEventType::RunCreated => research_pb::JournalEventType::RunCreated as i32,
            JournalEventType::RunStarted => research_pb::JournalEventType::RunStarted as i32,
            JournalEventType::RunSucceeded => research_pb::JournalEventType::RunSucceeded as i32,
            JournalEventType::RunFailed => research_pb::JournalEventType::RunFailed as i32,
            JournalEventType::RunCancelled => research_pb::JournalEventType::RunCancelled as i32,
            JournalEventType::ArtifactPublished => {
                research_pb::JournalEventType::ArtifactPublished as i32
            }
            JournalEventType::SignalSetPublished => {
                research_pb::JournalEventType::SignalSetPublished as i32
            }
            JournalEventType::NodeStarted => research_pb::JournalEventType::NodeStarted as i32,
            JournalEventType::NodeSucceeded => research_pb::JournalEventType::NodeSucceeded as i32,
            JournalEventType::NodeFailed => research_pb::JournalEventType::NodeFailed as i32,
            JournalEventType::NodeCheckpointed => {
                research_pb::JournalEventType::NodeCheckpointed as i32
            }
        },
        occurred_at: Some(core_pb::MarketTime {
            instant: Some(prost_types::Timestamp {
                seconds: instant.timestamp(),
                nanos: i32::try_from(instant.timestamp_subsec_nanos())
                    .expect("nanoseconds always fit i32"),
            }),
            market_timezone: value.occurred_at().market_timezone().to_owned(),
            local_trading_date: value.occurred_at().local_trading_date().to_string(),
        }),
        payload_type: value.payload_type().to_owned(),
        payload_schema: value.payload_schema().to_owned(),
        payload: value.payload().to_vec(),
        prev_hash: value.prev_hash().map(hash_to_proto),
        event_hash: Some(hash_to_proto(value.content_hash())),
    }
}

fn run_to_proto(run: &ficant_domain::research::ExperimentRun) -> research_pb::ExperimentRun {
    research_pb::ExperimentRun {
        experiment_run_id: Some(ulid_to_proto(run.id())),
        owner: Some(owner_to_proto(run.owner())),
        data_snapshot: Some(lineage_to_proto(run.data_snapshot())),
        universe_snapshot: Some(lineage_to_proto(run.universe_snapshot())),
        rule_packs: run
            .rule_packs()
            .iter()
            .map(|value| core_pb::VersionRef {
                id: Some(ulid_to_proto(value.id())),
                version: value.version().get(),
            })
            .collect(),
        runtime_image_digest: Some(hash_to_proto(run.runtime_image_digest())),
        parameters_hash: Some(hash_to_proto(run.parameters_hash())),
        seed: run.seed(),
        state: match run.state() {
            RunState::Created => research_pb::RunState::Created as i32,
            RunState::Running => research_pb::RunState::Running as i32,
            RunState::Succeeded => research_pb::RunState::Succeeded as i32,
            RunState::Failed => research_pb::RunState::Failed as i32,
            RunState::Cancelled => research_pb::RunState::Cancelled as i32,
        },
        revision: run.revision(),
    }
}

fn graph_to_proto(graph: &ResearchGraph) -> research_pb::ResearchGraph {
    research_pb::ResearchGraph {
        graph_id: Some(ulid_to_proto(graph.graph_id())),
        version: graph.version().get(),
        owner: Some(owner_to_proto(graph.owner())),
        nodes: graph
            .nodes()
            .iter()
            .map(|node| research_pb::ResearchNode {
                node_id: Some(ulid_to_proto(node.node_id())),
                contract: Some(contract_to_proto(node.contract())),
                parameters_hash: Some(hash_to_proto(node.parameters_hash())),
            })
            .collect(),
        edges: graph
            .edges()
            .iter()
            .map(|edge| research_pb::ResearchEdge {
                from_node_id: Some(ulid_to_proto(edge.from_node())),
                from_port: edge.from_port().to_owned(),
                to_node_id: Some(ulid_to_proto(edge.to_node())),
                to_port: edge.to_port().to_owned(),
            })
            .collect(),
        external_inputs: graph
            .external_inputs()
            .iter()
            .map(|input| research_pb::ExternalInputDeclaration {
                input_id: input.input_id().to_owned(),
                value_type: Some(typed_value_to_proto(input.value_type())),
            })
            .collect(),
        external_input_bindings: graph
            .external_input_bindings()
            .iter()
            .map(|binding| research_pb::ExternalInputBinding {
                input_id: binding.input_id().to_owned(),
                to_node_id: Some(ulid_to_proto(binding.to_node())),
                to_port: binding.to_port().to_owned(),
            })
            .collect(),
        topological_order: graph
            .topological_order()
            .iter()
            .map(ulid_to_proto)
            .collect(),
        digest: Some(hash_to_proto(graph.digest())),
    }
}

fn contract_to_proto(contract: &ResearchNodeContract) -> research_pb::ResearchNodeContract {
    let permissions = contract.permissions();
    let limits = contract.resource_limits();
    research_pb::ResearchNodeContract {
        contract_id: contract.contract_id().to_owned(),
        contract_version: contract.contract_version().get(),
        input_types: contract.input_types().iter().map(port_to_proto).collect(),
        output_types: contract.output_types().iter().map(port_to_proto).collect(),
        state_schema: Some(hash_to_proto(contract.state_schema())),
        parameter_schema: Some(hash_to_proto(contract.parameter_schema())),
        determinism_class: match contract.determinism_class() {
            DeterminismClass::Deterministic => research_pb::DeterminismClass::Deterministic as i32,
            DeterminismClass::Seeded => research_pb::DeterminismClass::Seeded as i32,
        },
        permissions: Some(research_pb::NodePermissions {
            network: permissions.network,
            database: permissions.database,
            filesystem: match permissions.filesystem {
                FilesystemPermission::None => research_pb::FilesystemPermission::None as i32,
                FilesystemPermission::TemporaryOnly => {
                    research_pb::FilesystemPermission::TemporaryOnly as i32
                }
            },
        }),
        resource_limits: Some(research_pb::ResourceLimits {
            cpu_cores: u32::from(limits.cpu_cores()),
            memory_mb: limits.memory_mb(),
            timeout_seconds: limits.timeout_seconds(),
        }),
        required_invariants: contract.required_invariants().to_vec(),
        digest: Some(hash_to_proto(contract.digest())),
    }
}

fn execution_to_proto(
    stored: &ficant_application::ports::StoredExecutionIdentity,
) -> Result<research_pb::ExecutionInstanceIdentity, Status> {
    let execution = &stored.identity;
    let reproducibility = execution.reproducibility();
    let external_inputs = reproducibility
        .external_inputs()
        .iter()
        .map(|input| {
            let binding = stored
                .external_input_artifacts
                .iter()
                .find(|binding| binding.input_id == input.input_id())
                .ok_or_else(|| Status::data_loss("持久化执行身份缺少外部输入制品血缘"))?;
            Ok(research_pb::ExecutionExternalInput {
                input_id: input.input_id().to_owned(),
                value_type: Some(typed_value_to_proto(input.value_type())),
                resolved_artifact: Some(core_pb::LineageRef {
                    object_id: Some(ulid_to_proto(&binding.artifact_id)),
                    version: 0,
                    content_hash: Some(hash_to_proto(&binding.content_hash)),
                }),
                content_hash: Some(hash_to_proto(&binding.content_hash)),
            })
        })
        .collect::<Result<Vec<_>, Status>>()?;
    if external_inputs.len() != stored.external_input_artifacts.len() {
        return Err(Status::data_loss(
            "持久化执行身份包含无法解析的外部输入制品血缘",
        ));
    }
    Ok(research_pb::ExecutionInstanceIdentity {
        run_id: Some(ulid_to_proto(execution.run_id())),
        reproducibility: Some(research_pb::ReproducibilityIdentity {
            graph_digest: Some(hash_to_proto(reproducibility.graph_digest())),
            data_snapshot_hash: Some(hash_to_proto(reproducibility.data_snapshot_hash())),
            universe_snapshot_hash: Some(hash_to_proto(reproducibility.universe_snapshot_hash())),
            parameters_hash: Some(hash_to_proto(reproducibility.parameters_hash())),
            runtime_image_digest: Some(hash_to_proto(reproducibility.runtime_image_digest())),
            environment_digest: Some(hash_to_proto(reproducibility.environment_digest())),
            seed: reproducibility.seed(),
            rule_packs: reproducibility
                .rule_pack_bindings()
                .iter()
                .map(|binding| research_pb::RulePackBinding {
                    rule_pack_id: Some(core_pb::Ulid {
                        value: binding.rule_pack_id.clone(),
                    }),
                    version: binding.version.get(),
                    content_hash: Some(hash_to_proto(&binding.content_hash)),
                })
                .collect(),
            node_implementations: reproducibility
                .node_implementations()
                .iter()
                .map(|binding| research_pb::NodeImplementationBinding {
                    node_id: Some(ulid_to_proto(&binding.node_id)),
                    implementation_digest: Some(hash_to_proto(&binding.implementation_digest)),
                })
                .collect(),
            external_inputs,
            digest: Some(hash_to_proto(reproducibility.digest())),
        }),
        digest: Some(hash_to_proto(execution.digest())),
    })
}

fn stored_manifest_to_proto(
    stored: &StoredNodeManifest,
) -> Result<research_pb::StoredNodeOutputManifest, Status> {
    let manifest = research_pb::NodeOutputManifest::decode(stored.manifest.as_slice())
        .map_err(|_| Status::data_loss("持久化输出清单无法解码"))?;
    if manifest.encode_to_vec() != stored.manifest {
        return Err(Status::data_loss("持久化输出清单不是规范编码"));
    }
    let execution = manifest.execution.clone();
    let mut checkpoint = research_pb::NodeCheckpoint {
        execution,
        node_id: Some(ulid_to_proto(&stored.node_id)),
        attempt: u32::try_from(stored.attempt)
            .map_err(|_| Status::data_loss("持久化 attempt 溢出"))?,
        output_manifest: Some(manifest.clone()),
        journal_sequence: stored.checkpoint.sequence,
        journal_hash: Some(hash_to_proto(&stored.checkpoint.event_hash)),
        checkpoint_hash: None,
    };
    checkpoint.checkpoint_hash = Some(hash_to_proto(&ContentHash::digest(
        &checkpoint.encode_to_vec(),
    )));
    Ok(research_pb::StoredNodeOutputManifest {
        manifest: Some(manifest),
        checkpoint: Some(checkpoint),
    })
}

fn output_trace_to_proto(trace: &OutputTrace) -> Result<research_pb::GraphOutputTrace, Status> {
    let manifests = trace
        .manifests
        .iter()
        .map(stored_manifest_to_proto)
        .collect::<Result<Vec<_>, _>>()?;
    let reproducibility = trace.run.identity.identity.reproducibility();
    let external_inputs = trace
        .external_inputs
        .iter()
        .map(|binding| {
            let input = reproducibility
                .external_inputs()
                .iter()
                .find(|input| input.input_id() == binding.input_id)
                .ok_or_else(|| Status::data_loss("外部输入血缘不完整"))?;
            Ok(research_pb::ExecutionExternalInput {
                input_id: binding.input_id.clone(),
                value_type: Some(typed_value_to_proto(input.value_type())),
                resolved_artifact: Some(core_pb::LineageRef {
                    object_id: Some(ulid_to_proto(&binding.artifact_id)),
                    version: 0,
                    content_hash: Some(hash_to_proto(&binding.content_hash)),
                }),
                content_hash: Some(hash_to_proto(&binding.content_hash)),
            })
        })
        .collect::<Result<Vec<_>, Status>>()?;
    Ok(research_pb::GraphOutputTrace {
        graph_run: Some(graph_run_to_proto(&trace.run)?),
        manifests,
        external_inputs,
    })
}

fn comparison_dimension(value: ComparisonDimension) -> i32 {
    match value {
        ComparisonDimension::Data => research_pb::GraphRunComparisonDimension::Data as i32,
        ComparisonDimension::Universe => research_pb::GraphRunComparisonDimension::Universe as i32,
        ComparisonDimension::Graph => research_pb::GraphRunComparisonDimension::Graph as i32,
        ComparisonDimension::Parameters => {
            research_pb::GraphRunComparisonDimension::Parameters as i32
        }
        ComparisonDimension::Runtime => research_pb::GraphRunComparisonDimension::Runtime as i32,
        ComparisonDimension::Environment => {
            research_pb::GraphRunComparisonDimension::Environment as i32
        }
        ComparisonDimension::Seed => research_pb::GraphRunComparisonDimension::Seed as i32,
        ComparisonDimension::RulePack => research_pb::GraphRunComparisonDimension::RulePack as i32,
        ComparisonDimension::Implementation => {
            research_pb::GraphRunComparisonDimension::Implementation as i32
        }
        ComparisonDimension::ExternalInput => {
            research_pb::GraphRunComparisonDimension::ExternalInput as i32
        }
        ComparisonDimension::Result => research_pb::GraphRunComparisonDimension::Result as i32,
    }
}

fn owner_from_proto(value: core_pb::OwnerRef) -> Result<OwnerRef, ApplicationError> {
    Ok(OwnerRef::new(
        parse_ulid_app(value.tenant_id)?,
        parse_ulid_app(value.owner_id)?,
    ))
}

fn lineage_from_proto(value: core_pb::LineageRef) -> Result<LineageRef, ApplicationError> {
    LineageRef::new(
        parse_ulid_app(value.object_id)?,
        (value.version != 0)
            .then(|| Version::new(value.version).map_err(map_domain_error))
            .transpose()?,
        value.content_hash.as_ref().map(parse_hash).transpose()?,
    )
    .map_err(map_domain_error)
}

fn parse_ulid(value: Option<core_pb::Ulid>) -> Result<Ulid, Status> {
    parse_ulid_app(value).map_err(|_| Status::invalid_argument("ULID 无效"))
}

fn parse_ulid_app(value: Option<core_pb::Ulid>) -> Result<Ulid, ApplicationError> {
    Ulid::new(value.ok_or_else(validation)?.value).map_err(map_domain_error)
}

fn parse_hash_app(value: Option<core_pb::Sha256>) -> Result<ContentHash, ApplicationError> {
    parse_hash(&value.ok_or_else(validation)?)
}

fn parse_hash(value: &core_pb::Sha256) -> Result<ContentHash, ApplicationError> {
    ContentHash::from_bytes(&value.value).map_err(map_domain_error)
}

fn hash_to_proto(value: &ContentHash) -> core_pb::Sha256 {
    core_pb::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn ulid_to_proto(value: &Ulid) -> core_pb::Ulid {
    core_pb::Ulid {
        value: value.to_string(),
    }
}

fn owner_to_proto(value: &OwnerRef) -> core_pb::OwnerRef {
    core_pb::OwnerRef {
        tenant_id: Some(ulid_to_proto(value.tenant_id())),
        owner_id: Some(ulid_to_proto(value.owner_id())),
    }
}

fn lineage_to_proto(value: &LineageRef) -> core_pb::LineageRef {
    core_pb::LineageRef {
        object_id: Some(ulid_to_proto(value.object_id())),
        version: value.version().map_or(0, Version::get),
        content_hash: value.content_hash().map(hash_to_proto),
    }
}

fn typed_value_to_proto(value: &TypedValue) -> research_pb::TypedValue {
    research_pb::TypedValue {
        type_id: value.type_id().to_owned(),
        type_version: value.type_version().get(),
        schema_hash: Some(hash_to_proto(value.schema_hash())),
    }
}

fn observed_outputs(
    manifest: &research_pb::NodeOutputManifest,
    outputs: Vec<NativePortValue>,
) -> Result<Vec<research_pb::ObservedNodeOutput>, Status> {
    let declared = manifest
        .content
        .as_ref()
        .ok_or_else(|| Status::data_loss("节点输出清单内容缺失"))?;
    if declared.outputs.len() != outputs.len() {
        return Err(Status::data_loss("节点输出数量与清单不一致"));
    }
    declared
        .outputs
        .iter()
        .zip(outputs)
        .map(|(binding, output)| {
            let value_type = typed_value_to_proto(output.value_type());
            let content_hash = hash_to_proto(output.content_hash());
            if binding.port_name != output.port_name()
                || binding.value_type.as_ref() != Some(&value_type)
                || binding.content_hash.as_ref() != Some(&content_hash)
            {
                return Err(Status::data_loss("节点输出端口与清单不一致"));
            }
            Ok(research_pb::ObservedNodeOutput {
                port_name: output.port_name().to_owned(),
                value_type: Some(value_type),
                content_hash: Some(content_hash),
                payload: output.payload().to_vec(),
            })
        })
        .collect()
}

fn port_to_proto(value: &PortType) -> research_pb::PortType {
    research_pb::PortType {
        port_name: value.port_name().to_owned(),
        value_type: Some(typed_value_to_proto(value.value_type())),
    }
}

fn trace_context(message: &impl Message) -> SafeTraceContext {
    let hash = ContentHash::digest(&message.encode_to_vec());
    let value = hash.as_bytes()[..16]
        .iter()
        .fold(String::with_capacity(32), |mut output, byte| {
            use std::fmt::Write as _;
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        });
    SafeTraceContext::new(value).expect("derived trace token is canonical")
}

fn validate_environment_attestation(value: &str) -> Result<(), ApplicationError> {
    if value.ends_with('\n') || value.contains('\r') {
        return Err(validation());
    }
    let mut lines = value.lines();
    if lines.next() != Some("ficant.worker.environment.v1") {
        return Err(validation());
    }
    let mut previous = None;
    let mut required = [false; 3];
    for line in lines {
        let (key, value) = line.split_once('=').ok_or_else(validation)?;
        if value.is_empty()
            || value.chars().any(char::is_control)
            || !key.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_lowercase()
                    || (index > 0 && (byte.is_ascii_digit() || matches!(byte, b'_' | b'-')))
            })
            || previous.is_some_and(|candidate: &str| candidate >= key)
        {
            return Err(validation());
        }
        match key {
            "arch" => required[0] = true,
            "os" => required[1] = true,
            "profile" => required[2] = true,
            _ => {}
        }
        previous = Some(key);
    }
    if previous.is_none() || required.contains(&false) {
        return Err(validation());
    }
    Ok(())
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

fn lineage() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

fn hash_mismatch() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    #[test]
    fn platform_administrator_cannot_enter_researcher_experiment_operations() {
        let principal = AuthorizedPrincipal::new(
            "experiment-admin".to_owned(),
            Ulid::new("01J00000000000000000000001").unwrap(),
            Ulid::new("01J00000000000000000000002").unwrap(),
            vec![Ulid::new("01J00000000000000000000003").unwrap()],
            PlatformRole::PlatformAdmin,
            vec![READ_SCOPE.to_owned(), WRITE_SCOPE.to_owned()],
            ContentHash::digest(b"credential-fingerprint"),
        )
        .unwrap();

        let error = require_experiment_access(principal, WRITE_SCOPE)
            .expect_err("administrator role must fail before any experiment repository call");
        assert_eq!(error.code(), Code::PermissionDenied);
    }

    fn fixture_output() -> NativePortValue {
        NativePortValue::new(
            "analysis",
            TypedValue::new(
                "ficant.rates.v1.analyze-bond-result",
                Version::new(1).expect("version"),
                ContentHash::digest(b"analysis-schema"),
            )
            .expect("typed value"),
            b"persisted-analysis".to_vec(),
        )
        .expect("native output")
    }

    fn fixture_manifest(output: &NativePortValue) -> research_pb::NodeOutputManifest {
        research_pb::NodeOutputManifest {
            attempt: 1,
            content: Some(research_pb::NodeOutputManifestContent {
                outputs: vec![research_pb::NodeOutputBinding {
                    port_name: output.port_name().to_owned(),
                    value_type: Some(typed_value_to_proto(output.value_type())),
                    content_hash: Some(hash_to_proto(output.content_hash())),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn observed_output_preserves_verified_payload_and_declared_metadata() {
        let output = fixture_output();
        let expected_payload = output.payload().to_vec();
        let manifest = fixture_manifest(&output);

        let observed = observed_outputs(&manifest, vec![output]).expect("matching output");

        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].port_name, "analysis");
        assert_eq!(observed[0].payload, expected_payload);
        assert_eq!(
            observed[0].value_type,
            manifest.content.as_ref().expect("content").outputs[0].value_type
        );
        assert_eq!(
            observed[0].content_hash,
            manifest.content.as_ref().expect("content").outputs[0].content_hash
        );
    }

    #[test]
    fn observed_output_rejects_missing_manifest_content() {
        let error = observed_outputs(
            &research_pb::NodeOutputManifest::default(),
            vec![fixture_output()],
        )
        .expect_err("missing content must fail");

        assert_eq!(error.code(), Code::DataLoss);
        assert_eq!(error.message(), "节点输出清单内容缺失");
    }

    #[test]
    fn observed_output_rejects_output_count_drift() {
        let output = fixture_output();
        let manifest = fixture_manifest(&output);
        let error = observed_outputs(&manifest, Vec::new()).expect_err("count drift must fail");

        assert_eq!(error.code(), Code::DataLoss);
        assert_eq!(error.message(), "节点输出数量与清单不一致");
    }

    #[test]
    fn observed_output_rejects_port_name_type_and_hash_drift() {
        for mutation in ["port", "type", "hash"] {
            let output = fixture_output();
            let mut manifest = fixture_manifest(&output);
            let binding = &mut manifest.content.as_mut().expect("content").outputs[0];
            match mutation {
                "port" => binding.port_name = "risk".to_owned(),
                "type" => {
                    binding.value_type.as_mut().expect("value type").type_id =
                        "ficant.rates.v1.risk-summary".to_owned();
                }
                "hash" => {
                    binding.content_hash = Some(hash_to_proto(&ContentHash::digest(b"tampered")));
                }
                _ => unreachable!(),
            }

            let error =
                observed_outputs(&manifest, vec![output]).expect_err("metadata drift must fail");
            assert_eq!(error.code(), Code::DataLoss, "mutation {mutation}");
            assert_eq!(
                error.message(),
                "节点输出端口与清单不一致",
                "mutation {mutation}"
            );
        }
    }
}
