use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use ficant_application::ports::{
    ExecutionExternalInput, ExternalInputArtifactBinding, NodeImplementation,
    ReproducibilityIdentity, ReproducibilityIdentityInput, StoredExecutionIdentity,
};
use ficant_domain::primitives::{ContentHash, LineageRef, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    Artifact, ArtifactKind, GraphExternalInput, GraphExternalInputBinding, ResearchGraph,
    ResearchGraphInput, ResearchNode,
};
use ficant_native_nodes::{
    CgbBondAnalyticsNativeNode, analyze_bond_request_type, cgb_bond_analytics_contract,
    native_node_source_digest, native_node_source_digest_attestation,
};
use ficant_runtime::{ExecutionInstanceIdentity, NativeNode, NativePortValue};
use tokio::sync::watch;

use super::{
    ClaimedTask, ExecutedNode, InputEvidence, InputSource, LoadedTask, NodeCompletion,
    PreparedInputs, WorkerBackend, WorkerConfig, WorkerError, WorkerStep,
    canonical_environment_digest, run_claimed, run_worker,
};

struct FakeBackend {
    loaded: LoadedTask,
    steps: Mutex<Vec<WorkerStep>>,
    fail_at: Option<WorkerStep>,
    retryable_at: Option<WorkerStep>,
    execute_delay: Duration,
}

impl FakeBackend {
    fn new(fail_at: Option<WorkerStep>, execute_delay: Duration) -> Self {
        Self {
            loaded: loaded(),
            steps: Mutex::new(Vec::new()),
            fail_at,
            retryable_at: None,
            execute_delay,
        }
    }

    fn retryable(step: WorkerStep, execute_delay: Duration) -> Self {
        Self {
            loaded: loaded(),
            steps: Mutex::new(Vec::new()),
            fail_at: None,
            retryable_at: Some(step),
            execute_delay,
        }
    }

    fn record(&self, step: WorkerStep) -> Result<(), WorkerError> {
        self.steps.lock().unwrap().push(step);
        if self.fail_at == Some(step) {
            Err(WorkerError::Backend {
                step,
                retryable: false,
            })
        } else if self.retryable_at == Some(step) {
            Err(WorkerError::Backend {
                step,
                retryable: true,
            })
        } else {
            Ok(())
        }
    }

    fn steps(&self) -> Vec<WorkerStep> {
        self.steps.lock().unwrap().clone()
    }
}

#[async_trait]
impl WorkerBackend for FakeBackend {
    async fn claim(
        &self,
        _worker_id: &Ulid,
        _lease_id: &Ulid,
        _lease_seconds: u32,
    ) -> Result<Option<ClaimedTask>, WorkerError> {
        self.record(WorkerStep::Load)?;
        Ok(None)
    }

    async fn renew(
        &self,
        _task: &ClaimedTask,
        _worker_id: &Ulid,
        _lease_seconds: u32,
    ) -> Result<(), WorkerError> {
        self.record(WorkerStep::Renew)
    }

    async fn load(
        &self,
        _task: &ClaimedTask,
        _worker_id: &Ulid,
    ) -> Result<LoadedTask, WorkerError> {
        self.record(WorkerStep::Load)?;
        Ok(self.loaded.clone())
    }

    async fn begin(&self, _task: &ClaimedTask, _worker_id: &Ulid) -> Result<(), WorkerError> {
        self.record(WorkerStep::Begin)
    }

    async fn read_inputs(
        &self,
        _task: &ClaimedTask,
        loaded: &LoadedTask,
    ) -> Result<PreparedInputs, WorkerError> {
        self.record(WorkerStep::ReadInput)?;
        let frozen = &loaded
            .stored_identity
            .identity
            .reproducibility()
            .external_inputs()[0];
        Ok(PreparedInputs {
            values: vec![
                NativePortValue::new(
                    "request",
                    frozen.value_type().clone(),
                    frozen.payload().to_vec(),
                )
                .unwrap(),
            ],
            evidence: vec![InputEvidence {
                target_port: "request".to_owned(),
                value_type: frozen.value_type().clone(),
                artifact_id: id('I'),
                content_hash: frozen.content_hash().clone(),
                source: InputSource::External {
                    input_id: "market-data".to_owned(),
                },
            }],
        })
    }

    async fn execute(
        &self,
        _task: &ClaimedTask,
        loaded: &LoadedTask,
        inputs: PreparedInputs,
    ) -> Result<ExecutedNode, WorkerError> {
        self.record(WorkerStep::Execute)?;
        tokio::time::sleep(self.execute_delay).await;
        let value_type = loaded.graph.nodes()[0].contract().output_types()[0]
            .value_type()
            .clone();
        let output = NativePortValue::new("result", value_type, b"result".to_vec()).unwrap();
        let envelope = b"output-envelope".to_vec();
        Ok(ExecutedNode {
            outputs: vec![output],
            output_envelope_hash: ContentHash::digest(&envelope),
            output_envelope: envelope,
            input_evidence: inputs.evidence,
        })
    }

    async fn promote(
        &self,
        task: &ClaimedTask,
        loaded: &LoadedTask,
        execution: ExecutedNode,
    ) -> Result<NodeCompletion, WorkerError> {
        self.record(WorkerStep::Promote)?;
        let hash = ContentHash::digest(&execution.output_envelope);
        let artifact = Artifact::new(
            task.planned_artifact_id.clone(),
            loaded.owner.clone(),
            ArtifactKind::Generic,
            "application/test",
            hash,
            u64::try_from(execution.output_envelope.len()).unwrap(),
            vec![LineageRef::content_addressed(
                id('I'),
                ContentHash::digest(b"request"),
            )],
        )
        .unwrap();
        Ok(NodeCompletion {
            artifact,
            verified_blob: ficant_application::ports::VerifiedBlobRef::new(
                ContentHash::digest(&execution.output_envelope),
                u64::try_from(execution.output_envelope.len()).unwrap(),
            )
            .unwrap(),
            verified_payload: execution.output_envelope,
            output_manifest: b"manifest".to_vec(),
        })
    }

    async fn complete(
        &self,
        _task: &ClaimedTask,
        _worker_id: &Ulid,
        _completion: NodeCompletion,
    ) -> Result<(), WorkerError> {
        self.record(WorkerStep::Complete)
    }

    async fn fail(
        &self,
        _task: &ClaimedTask,
        _worker_id: &Ulid,
        _failure_hash: ContentHash,
    ) -> Result<(), WorkerError> {
        self.record(WorkerStep::Fail)
    }
}

#[tokio::test]
async fn successful_task_promotes_before_atomic_complete() {
    let backend = FakeBackend::new(None, Duration::ZERO);
    run_claimed(&backend, &config(), &claim()).await.unwrap();
    assert_eq!(
        backend.steps(),
        vec![
            WorkerStep::Load,
            WorkerStep::Begin,
            WorkerStep::ReadInput,
            WorkerStep::Execute,
            WorkerStep::Promote,
            WorkerStep::Complete,
        ]
    );
}

#[tokio::test]
async fn execution_failure_after_begin_is_atomically_failed() {
    let backend = FakeBackend::new(Some(WorkerStep::Promote), Duration::ZERO);
    assert_eq!(
        run_claimed(&backend, &config(), &claim()).await,
        Err(WorkerError::Backend {
            step: WorkerStep::Promote,
            retryable: false,
        })
    );
    assert_eq!(
        backend.steps(),
        vec![
            WorkerStep::Load,
            WorkerStep::Begin,
            WorkerStep::ReadInput,
            WorkerStep::Execute,
            WorkerStep::Promote,
            WorkerStep::Fail,
        ]
    );
}

#[tokio::test]
async fn retryable_failure_leaves_the_lease_for_reclaim_instead_of_marking_failed() {
    let backend = FakeBackend::retryable(WorkerStep::Promote, Duration::ZERO);
    assert_eq!(
        run_claimed(&backend, &config(), &claim()).await,
        Err(WorkerError::Backend {
            step: WorkerStep::Promote,
            retryable: true,
        })
    );
    assert_eq!(
        backend.steps(),
        vec![
            WorkerStep::Load,
            WorkerStep::Begin,
            WorkerStep::ReadInput,
            WorkerStep::Execute,
            WorkerStep::Promote,
        ]
    );
}

#[tokio::test]
async fn timeout_records_failure_without_promotion_or_completion() {
    let backend = FakeBackend::new(None, Duration::from_millis(500));
    let mut config = config();
    config.node_timeout = Duration::from_millis(200);
    config.renew_interval = Duration::from_millis(20);
    assert_eq!(
        run_claimed(&backend, &config, &claim()).await,
        Err(WorkerError::TimedOut)
    );
    let steps = backend.steps();
    assert!(steps.contains(&WorkerStep::Renew));
    assert_eq!(steps.last(), Some(&WorkerStep::Fail));
    assert!(!steps.contains(&WorkerStep::Promote));
    assert!(!steps.contains(&WorkerStep::Complete));
}

#[tokio::test]
async fn long_execution_renews_lease_before_promoting() {
    let backend = FakeBackend::new(None, Duration::from_millis(25));
    let mut config = config();
    config.renew_interval = Duration::from_millis(5);
    run_claimed(&backend, &config, &claim()).await.unwrap();
    let steps = backend.steps();
    let renew = steps
        .iter()
        .position(|step| *step == WorkerStep::Renew)
        .unwrap();
    let promote = steps
        .iter()
        .position(|step| *step == WorkerStep::Promote)
        .unwrap();
    assert!(renew < promote);
}

#[tokio::test]
async fn ambiguous_complete_failure_is_not_rewritten_as_node_failure() {
    let backend = FakeBackend::new(Some(WorkerStep::Complete), Duration::ZERO);
    assert!(run_claimed(&backend, &config(), &claim()).await.is_err());
    assert_eq!(backend.steps().last(), Some(&WorkerStep::Complete));
    assert!(!backend.steps().contains(&WorkerStep::Fail));
}

#[tokio::test]
async fn graceful_drain_prevents_another_claim() {
    let backend = FakeBackend::new(None, Duration::ZERO);
    let (_sender, receiver) = watch::channel(true);
    run_worker(&backend, &config(), receiver).await.unwrap();
    assert!(backend.steps().is_empty());
}

#[test]
fn renewal_must_be_shorter_than_lease_duration() {
    let mut config = config();
    config.renew_interval = config.lease_duration;
    assert_eq!(
        config.validate(),
        Err(WorkerError::InvalidConfiguration("worker duration"))
    );
}

#[test]
fn environment_attestation_is_exact_canonical_bytes() {
    let canonical = environment_attestation();
    assert_eq!(
        canonical_environment_digest(canonical),
        Ok(ContentHash::digest(canonical.as_bytes()))
    );
    for changed in [
        "ficant.worker.environment.v1\nos=linux\narch=amd64\nprofile=unit-test",
        "ficant.worker.environment.v1\narch=amd64\narch=amd64",
        "ficant.worker.environment.v1\narch=amd64\nos=linux\nprofile=unit-test\n",
        "ficant.worker.environment.v1\r\narch=amd64\nos=linux",
        "ficant.worker.environment.v2\narch=amd64",
        "ficant.worker.environment.v1",
        "ficant.worker.environment.v1\narch=amd64\nos=linux",
    ] {
        assert_eq!(
            canonical_environment_digest(changed),
            Err(WorkerError::InvalidConfiguration(
                "FICANT_WORKER_ENVIRONMENT_ATTESTATION"
            ))
        );
    }
}

#[test]
fn deployment_digests_require_exact_oci_sha256_encoding() {
    assert_eq!(
        super::parse_sha256_attestation(
            "FICANT_WORKER_NATIVE_SOURCE_DIGEST",
            &native_node_source_digest_attestation()
        ),
        Ok(native_node_source_digest())
    );
    for changed in [
        native_node_source_digest_attestation().to_uppercase(),
        native_node_source_digest_attestation().replace("sha256:", ""),
        "sha256:00".to_owned(),
    ] {
        assert_eq!(
            super::parse_sha256_attestation("FICANT_WORKER_NATIVE_SOURCE_DIGEST", &changed),
            Err(WorkerError::InvalidConfiguration(
                "FICANT_WORKER_NATIVE_SOURCE_DIGEST"
            ))
        );
    }
}

#[tokio::test]
async fn deployment_attestation_drift_fails_before_node_started() {
    let backend = FakeBackend::new(None, Duration::ZERO);
    let mut changed = config();
    changed.runtime_image_digest = ContentHash::digest(b"changed-runtime");
    assert_eq!(
        run_claimed(&backend, &changed, &claim()).await,
        Err(WorkerError::InvalidTask(
            "persisted identity or deployment attestation mismatch"
        ))
    );
    assert_eq!(backend.steps(), vec![WorkerStep::Load]);
}

fn config() -> WorkerConfig {
    WorkerConfig {
        database_url: "postgres://worker.invalid/ficant".to_owned(),
        s3_endpoint: "http://s3.invalid".to_owned(),
        s3_bucket: "ficant".to_owned(),
        s3_access_key: "access".to_owned(),
        s3_secret_key: "secret".to_owned(),
        worker_id: id('W'),
        runtime_image_digest: ContentHash::digest(b"runtime"),
        environment_attestation: environment_attestation().to_owned(),
        native_source_digest: native_node_source_digest(),
        lease_duration: Duration::from_secs(1),
        renew_interval: Duration::from_millis(100),
        idle_poll_interval: Duration::from_millis(1),
        node_timeout: Duration::from_millis(100),
    }
}

fn claim() -> ClaimedTask {
    let loaded = loaded();
    ClaimedTask {
        tenant_id: loaded.owner.tenant_id().clone(),
        task_id: id('T'),
        run_id: loaded.stored_identity.identity.run_id().clone(),
        node_id: loaded.graph.nodes()[0].node_id().clone(),
        graph_digest: loaded.graph.digest().clone(),
        execution_identity_digest: loaded.stored_identity.identity.digest().clone(),
        planned_artifact_id: id('A'),
        lease_id: id('L'),
        attempt: 1,
    }
}

fn loaded() -> LoadedTask {
    let owner = OwnerRef::new(id('N'), id('O'));
    let request = analyze_bond_request_type();
    let contract = cgb_bond_analytics_contract().unwrap();
    let node = ResearchNode::new(id('D'), contract, ContentHash::digest(b"parameters"));
    let executor = CgbBondAnalyticsNativeNode::new(node.node_id().clone()).unwrap();
    let graph = ResearchGraph::new_with_external_inputs(
        ResearchGraphInput {
            graph_id: id('G'),
            version: Version::new(1).unwrap(),
            owner: owner.clone(),
            nodes: vec![node.clone()],
            edges: vec![],
        },
        vec![GraphExternalInput::new("market-data", request.clone()).unwrap()],
        vec![
            GraphExternalInputBinding::new("market-data", node.node_id().clone(), "request")
                .unwrap(),
        ],
    )
    .unwrap();
    let input = ExecutionExternalInput::new("market-data", request, b"request".to_vec()).unwrap();
    let reproducibility = ReproducibilityIdentity::new(
        &graph,
        ReproducibilityIdentityInput {
            external_inputs: vec![input.clone()],
            data_snapshot_hash: ContentHash::digest(b"data"),
            universe_snapshot_hash: ContentHash::digest(b"universe"),
            parameters_hash: ContentHash::digest(b"parameters"),
            runtime_image_digest: ContentHash::digest(b"runtime"),
            environment_digest: canonical_environment_digest(environment_attestation()).unwrap(),
            seed: 7,
            rule_pack_bindings: vec![],
            node_implementations: vec![NodeImplementation {
                node_id: node.node_id().clone(),
                implementation_digest: executor.implementation_digest().clone(),
            }],
        },
    )
    .unwrap();
    let identity = ExecutionInstanceIdentity::from_reproducibility(id('R'), reproducibility);
    let stored_identity = StoredExecutionIdentity {
        owner: owner.clone(),
        graph_id: graph.graph_id().clone(),
        graph_version: graph.version(),
        identity,
        external_input_artifacts: vec![ExternalInputArtifactBinding {
            input_id: "market-data".to_owned(),
            artifact_id: id('I'),
            content_hash: input.content_hash().clone(),
        }],
    };
    LoadedTask {
        owner,
        graph,
        stored_identity,
    }
}

fn environment_attestation() -> &'static str {
    "ficant.worker.environment.v1\narch=amd64\nos=linux\nprofile=unit-test"
}

fn id(character: char) -> Ulid {
    super::derived_id(
        b"ficant/worker-test-id/v1",
        &[character.to_string().as_bytes()],
    )
}
