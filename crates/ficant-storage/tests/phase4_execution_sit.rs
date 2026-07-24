mod support;

use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    AppendJournalEvent, BeginNode, CompleteNode, EnqueueNode, ExecutionExternalInput,
    ExecutionInstanceIdentity, ExternalInputArtifactBinding, FailNode, IdempotencyKey,
    NodeImplementation, NodeLeaseFence, PageRequest, Phase4ExecutionRepository,
    ReproducibilityIdentity, ReproducibilityIdentityInput, RulePackBinding, RunJournalRepository,
    StoredExecutionIdentity, VerifiedBlobRef, replay_graph_execution, stable_node_artifact_id,
};
use ficant_application::use_cases::phase4_submission::{Phase4Submission, PreparedGraphSubmission};
use ficant_contracts::ficant::{core::v1 as core_pb, research::v1 as research_pb};
use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{ContentHash, LineageRef, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    Artifact, ArtifactKind, DeterminismClass, ExperimentRun, ExperimentRunInput,
    FilesystemPermission, GraphExternalInput, GraphExternalInputBinding, JournalEventType,
    NodePermissions, PortType, ResearchEdge, ResearchGraph, ResearchGraphInput, ResearchNode,
    ResearchNodeContract, ResearchNodeContractInput, ResourceLimits, RunJournal, RunJournalInput,
    RunState, TypedValue,
};
use ficant_runtime::{NativePortValue, canonical_output_bytes};
use ficant_storage::lease_queue::PostgresLeaseQueue;
use prost::Message;

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{PREFIX}{suffix}")).unwrap()
}

fn hex(hash: &ContentHash) -> String {
    use std::fmt::Write as _;

    hash.as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").unwrap();
            output
        })
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('Z'))
}

fn graph() -> ResearchGraph {
    let value_type = TypedValue::new(
        "ficant.fixture",
        Version::new(1).unwrap(),
        ContentHash::digest(b"fixture-schema"),
    )
    .unwrap();
    let contract = ResearchNodeContract::new(ResearchNodeContractInput {
        contract_id: "native.fixture".to_owned(),
        contract_version: Version::new(1).unwrap(),
        input_types: vec![PortType::new("market_input", value_type.clone()).unwrap()],
        output_types: vec![PortType::new("result", value_type.clone()).unwrap()],
        state_schema: ContentHash::digest(b"state"),
        parameter_schema: ContentHash::digest(b"parameters"),
        determinism_class: DeterminismClass::Deterministic,
        permissions: NodePermissions {
            network: false,
            database: false,
            filesystem: FilesystemPermission::None,
        },
        resource_limits: ResourceLimits::new(1, 64, 10).unwrap(),
        required_invariants: vec!["verified-input".to_owned()],
    })
    .unwrap();
    ResearchGraph::new_with_external_inputs(
        ResearchGraphInput {
            graph_id: id('G'),
            version: Version::new(1).unwrap(),
            owner: owner(),
            nodes: vec![ResearchNode::new(
                id('N'),
                contract,
                ContentHash::digest(b"node-parameters"),
            )],
            edges: vec![],
        },
        vec![GraphExternalInput::new("fixture", value_type).unwrap()],
        vec![GraphExternalInputBinding::new("fixture", id('N'), "market_input").unwrap()],
    )
    .unwrap()
}

fn graph_with_successor() -> ResearchGraph {
    let base = graph();
    let value_type = base.external_inputs()[0].value_type().clone();
    let contract = base.nodes()[0].contract().clone();
    ResearchGraph::new_with_external_inputs(
        ResearchGraphInput {
            graph_id: id('G'),
            version: Version::new(1).unwrap(),
            owner: owner(),
            nodes: vec![
                ResearchNode::new(
                    id('N'),
                    contract.clone(),
                    ContentHash::digest(b"node-parameters"),
                ),
                ResearchNode::new(
                    id('P'),
                    contract,
                    ContentHash::digest(b"successor-parameters"),
                ),
            ],
            edges: vec![ResearchEdge::new(id('N'), "result", id('P'), "market_input").unwrap()],
        },
        vec![GraphExternalInput::new("fixture", value_type).unwrap()],
        vec![GraphExternalInputBinding::new("fixture", id('N'), "market_input").unwrap()],
    )
    .unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn phase4_postgres_closure_is_fenced_atomic_idempotent_and_terminal() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let scope = support::access_scope(&owner());
    let graph = graph();
    seed_shared_dependencies(&pool).await;
    repository
        .publish_graph(&scope, graph.clone())
        .await
        .unwrap();
    let loaded = repository
        .load_graph(&scope, graph.graph_id(), graph.version())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded, graph);

    let external_payload = b"verified-external-input".to_vec();
    let external_hash = ContentHash::digest(&external_payload);
    seed_external_artifact(&pool, &external_hash, external_payload.len()).await;

    let first = publish_identity(
        &pool,
        &repository,
        &scope,
        &graph,
        id('R'),
        &external_payload,
    )
    .await;
    let second = publish_identity(
        &pool,
        &repository,
        &scope,
        &graph,
        id('S'),
        &external_payload,
    )
    .await;
    assert_eq!(
        first.identity.reproducibility_digest(),
        second.identity.reproducibility_digest()
    );
    let planned = stable_node_artifact_id(first.identity.reproducibility_digest(), &id('N'));

    let first_task = task(&first, planned.clone(), id('A'), "run-r/node-n");
    repository.enqueue_node(first_task.clone()).await.unwrap();
    let queue = PostgresLeaseQueue::new(pool.clone());
    let claimed = queue
        .claim_next(&id('W'), &id('K'), 60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.claim_count(), 1);
    let first_fence = fence(&claimed);
    let begun = repository
        .begin_node(BeginNode {
            fence: first_fence.clone(),
            started_event_id: id('B'),
        })
        .await
        .unwrap();
    assert!(!begun.replayed);
    assert!(
        repository
            .begin_node(BeginNode {
                fence: first_fence.clone(),
                started_event_id: id('B'),
            })
            .await
            .unwrap()
            .replayed
    );
    let interrupted_events = read_journal(&repository, &scope, id('R')).await;
    let interrupted = replay_graph_execution(&graph, &interrupted_events).unwrap();
    assert_eq!(interrupted.run_state(), RunState::Running);
    assert_eq!(interrupted.resume_node(), Some(&id('N')));

    let output_payload = canonical_output_bytes(&[NativePortValue::new(
        "result",
        graph.external_inputs()[0].value_type().clone(),
        b"deterministic-node-output".to_vec(),
    )
    .unwrap()]);
    let output_hash = ContentHash::digest(&output_payload);
    seed_blob(&pool, &output_hash, output_payload.len()).await;
    let artifact = Artifact::new(
        planned.clone(),
        owner(),
        ArtifactKind::Generic,
        "application/vnd.ficant.native-node.v1",
        output_hash.clone(),
        output_payload.len() as u64,
        vec![LineageRef::content_addressed(
            id('E'),
            external_hash.clone(),
        )],
    )
    .unwrap();

    let valid_manifest = manifest(&first, &graph, &artifact, first_fence.attempt);
    for tampered in tampered_manifests(&valid_manifest) {
        let rollback = repository
            .complete_node(CompleteNode {
                fence: first_fence.clone(),
                artifact: artifact.clone(),
                verified_blob: VerifiedBlobRef::new(
                    output_hash.clone(),
                    output_payload.len() as u64,
                )
                .unwrap(),
                verified_payload: output_payload.clone(),
                output_manifest: tampered,
                succeeded_event_id: id('C'),
                checkpoint_event_id: id('D'),
            })
            .await
            .unwrap_err();
        assert!(matches!(
            rollback.category(),
            ApplicationErrorCategory::LineageIncomplete
                | ApplicationErrorCategory::ImmutableViolation
                | ApplicationErrorCategory::ValidationFailed
        ));
    }
    let after_rollback: (String, i64, i64) = sqlx::query_as(
        "SELECT e.state,
                (SELECT COUNT(*) FROM research.artifacts WHERE artifact_id=$1),
                (SELECT COUNT(*) FROM research.run_journal WHERE run_id=$2)
         FROM research.node_executions e WHERE e.run_id=$2",
    )
    .bind(planned.as_str())
    .bind(id('R').as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after_rollback, ("STARTED".to_owned(), 0, 3));

    let success_command = CompleteNode {
        fence: first_fence.clone(),
        artifact: artifact.clone(),
        verified_blob: VerifiedBlobRef::new(output_hash.clone(), output_payload.len() as u64)
            .unwrap(),
        verified_payload: output_payload.clone(),
        output_manifest: valid_manifest,
        succeeded_event_id: id('C'),
        checkpoint_event_id: id('D'),
    };
    let success = repository
        .complete_node(success_command.clone())
        .await
        .unwrap();
    assert!(!success.replayed);
    let replay = repository.complete_node(success_command).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.artifact, artifact);
    let terminal: (String, String, i64) = sqlx::query_as(
        "SELECT t.state,r.state,
                (SELECT COUNT(*) FROM research.run_journal WHERE run_id=$1)
         FROM research.execution_tasks t
         JOIN research.experiment_runs r
           ON r.tenant_id=t.tenant_id AND r.experiment_run_id=t.run_id
         WHERE t.run_id=$1",
    )
    .bind(id('R').as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        terminal,
        ("COMPLETED".to_owned(), "SUCCEEDED".to_owned(), 6)
    );
    let succeeded_events = read_journal(&repository, &scope, id('R')).await;
    let succeeded_replay = replay_graph_execution(&graph, &succeeded_events).unwrap();
    assert_eq!(succeeded_replay.run_state(), RunState::Succeeded);
    assert_eq!(succeeded_replay.completed_nodes(), &[id('N')]);
    assert_eq!(
        succeeded_replay.last_checkpoint().unwrap().output_hash(),
        &output_hash
    );
    let manifests = repository
        .list_node_manifests(&scope, &id('R'))
        .await
        .unwrap();
    assert_eq!(manifests.len(), 1);
    let trace = repository
        .trace_output(&scope, &id('R'), &id('N'))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(trace.manifests, manifests);
    let comparison = repository
        .compare_graph_runs(&scope, &id('R'), &id('S'))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        comparison.differing_dimensions,
        vec![ficant_application::ports::ComparisonDimension::Result]
    );

    let second_task = task(&second, planned, id('F'), "run-s/node-n");
    repository.enqueue_node(second_task).await.unwrap();
    let abandoned = queue
        .claim_next(&id('W'), &id('H'), 60)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "UPDATE research.execution_tasks
         SET lease_expires_at=CURRENT_TIMESTAMP-INTERVAL '1 second'
         WHERE tenant_id=$1 AND task_id=$2",
    )
    .bind(abandoned.tenant_id().as_str())
    .bind(abandoned.task_id().as_str())
    .execute(&pool)
    .await
    .unwrap();
    let recovered = queue
        .claim_next(&id('X'), &id('J'), 60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.claim_count(), 2);
    let old_begin = repository
        .begin_node(BeginNode {
            fence: fence(&abandoned),
            started_event_id: id('K'),
        })
        .await
        .unwrap_err();
    assert_eq!(
        old_begin.category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );
    let recovered_fence = fence(&recovered);
    repository
        .begin_node(BeginNode {
            fence: recovered_fence.clone(),
            started_event_id: id('M'),
        })
        .await
        .unwrap();
    let failure_hash = ContentHash::digest(b"deterministic-failure");
    let failed = repository
        .fail_node(FailNode {
            fence: recovered_fence.clone(),
            failure_hash: failure_hash.clone(),
            failed_event_id: id('Q'),
        })
        .await
        .unwrap();
    assert!(!failed.replayed);
    assert!(
        repository
            .fail_node(FailNode {
                fence: recovered_fence,
                failure_hash,
                failed_event_id: id('Q'),
            })
            .await
            .unwrap()
            .replayed
    );
    let failed_terminal: (String, String) = sqlx::query_as(
        "SELECT t.state,r.state FROM research.execution_tasks t
         JOIN research.experiment_runs r
           ON r.tenant_id=t.tenant_id AND r.experiment_run_id=t.run_id
         WHERE t.run_id=$1",
    )
    .bind(id('S').as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(failed_terminal, ("FAILED".to_owned(), "FAILED".to_owned()));
    let failed_events = read_journal(&repository, &scope, id('S')).await;
    let failed_replay = replay_graph_execution(&graph, &failed_events).unwrap();
    assert_eq!(failed_replay.run_state(), RunState::Failed);
    assert_eq!(failed_replay.event_count(), 5);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn submit_is_atomic_replay_safe_and_persistent_queries_are_scope_bound() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let scope = support::access_scope(&owner());
    let graph = graph();
    seed_shared_dependencies(&pool).await;
    let external_payload = b"verified-external-input".to_vec();
    let external_hash = ContentHash::digest(&external_payload);
    seed_external_artifact(&pool, &external_hash, external_payload.len()).await;
    let run = ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: id('R'),
        owner: owner(),
        data_snapshot: LineageRef::content_addressed(
            id('Y'),
            ContentHash::digest(b"data-snapshot"),
        ),
        universe_snapshot: LineageRef::content_addressed(
            id('V'),
            ContentHash::digest(b"universe-snapshot"),
        ),
        rule_packs: vec![ficant_domain::primitives::VersionRef::new(
            id('Q'),
            Version::new(1).unwrap(),
        )],
        runtime_image_digest: ContentHash::digest(b"runtime-image"),
        parameters_hash: ContentHash::digest(b"run-parameters"),
        seed: 42,
    })
    .unwrap();
    let identity = identity_value(&graph, &id('R'), &external_payload);
    let command = ficant_application::ports::SubmitGraphRun {
        scope: scope.clone(),
        idempotency_key: IdempotencyKey::new("phase4-submit/run-r").unwrap(),
        run,
        graph: graph.clone(),
        identity: identity.clone(),
    };
    let submitted = repository.submit_graph_run(command.clone()).await.unwrap();
    assert_eq!(submitted.run.state(), RunState::Running);
    assert_eq!(
        repository.submit_graph_run(command.clone()).await.unwrap(),
        submitted
    );
    let mut different_key = command.clone();
    different_key.idempotency_key = IdempotencyKey::new("phase4-submit/run-r-different").unwrap();
    assert_eq!(
        repository
            .submit_graph_run(different_key)
            .await
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ImmutableViolation
    );
    let other_run = ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: id('S'),
        owner: owner(),
        data_snapshot: command.run.data_snapshot().clone(),
        universe_snapshot: command.run.universe_snapshot().clone(),
        rule_packs: command.run.rule_packs().to_vec(),
        runtime_image_digest: command.run.runtime_image_digest().clone(),
        parameters_hash: command.run.parameters_hash().clone(),
        seed: command.run.seed(),
    })
    .unwrap();
    let same_key_other_run = ficant_application::ports::SubmitGraphRun {
        scope: scope.clone(),
        idempotency_key: command.idempotency_key.clone(),
        run: other_run,
        graph: graph.clone(),
        identity: identity_value(&graph, &id('S'), &external_payload),
    };
    assert_eq!(
        repository
            .submit_graph_run(same_key_other_run)
            .await
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ImmutableViolation
    );
    let loaded = repository
        .get_graph_run(&scope, &id('R'))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded, submitted);
    assert!(
        repository
            .list_node_manifests(&scope, &id('R'))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .trace_output(&scope, &id('R'), &id('N'))
            .await
            .unwrap()
            .is_none()
    );
    let comparison = repository
        .compare_graph_runs(&scope, &id('R'), &id('R'))
        .await
        .unwrap()
        .unwrap();
    assert!(comparison.differing_dimensions.is_empty());

    let before = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM research.experiment_runs WHERE experiment_run_id=$1",
    )
    .bind(id('R').as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut drift = identity;
    drift.external_input_artifacts[0].content_hash = ContentHash::digest(b"drift");
    let stored_run = loaded.run;
    let created = ficant_domain::research::ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: id('R'),
        owner: owner(),
        data_snapshot: stored_run.data_snapshot().clone(),
        universe_snapshot: stored_run.universe_snapshot().clone(),
        rule_packs: stored_run.rule_packs().to_vec(),
        runtime_image_digest: stored_run.runtime_image_digest().clone(),
        parameters_hash: stored_run.parameters_hash().clone(),
        seed: stored_run.seed(),
    })
    .unwrap();
    let error = repository
        .submit_graph_run(ficant_application::ports::SubmitGraphRun {
            scope,
            idempotency_key: IdempotencyKey::new("phase4-submit/run-r").unwrap(),
            run: created,
            graph,
            identity: drift.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        error.category(),
        ApplicationErrorCategory::ImmutableViolation
    );
    let after = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM research.experiment_runs WHERE experiment_run_id=$1",
    )
    .bind(id('R').as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn completion_derives_the_only_legal_successor_from_the_frozen_graph() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let scope = support::access_scope(&owner());
    let graph = graph_with_successor();
    seed_shared_dependencies(&pool).await;
    let external_payload = b"verified-external-input".to_vec();
    let external_hash = ContentHash::digest(&external_payload);
    seed_external_artifact(&pool, &external_hash, external_payload.len()).await;
    let data_snapshot =
        LineageRef::content_addressed(id('Y'), ContentHash::digest(b"data-snapshot"));
    let universe_snapshot =
        LineageRef::content_addressed(id('V'), ContentHash::digest(b"universe-snapshot"));
    let rule_pack = ficant_domain::primitives::VersionRef::new(id('Q'), Version::new(1).unwrap());
    let runtime_image_digest = ContentHash::digest(b"runtime-image");
    let parameters_hash = ContentHash::digest(b"run-parameters");
    let identity = identity_value(&graph, &id('R'), &external_payload);
    let planned = stable_node_artifact_id(identity.identity.reproducibility_digest(), &id('N'));
    let submitted = Phase4Submission::new(&repository)
        .submit(PreparedGraphSubmission {
            idempotency_key: "phase4-submit/run-r".to_owned(),
            scope: scope.clone(),
            owner: owner(),
            run_id: id('R'),
            graph: graph.clone(),
            data_snapshot,
            universe_snapshot,
            rule_packs: vec![rule_pack],
            runtime_image_digest,
            parameters_hash,
            seed: 42,
            execution: identity.identity.clone(),
            external_input_artifacts: identity.external_input_artifacts.clone(),
        })
        .await
        .unwrap();
    assert_eq!(submitted.graph, graph);
    assert_eq!(
        repository
            .get_graph_run(&scope, &id('R'))
            .await
            .unwrap()
            .unwrap(),
        submitted
    );
    let queue = PostgresLeaseQueue::new(pool.clone());
    let claimed = queue
        .claim_next(&id('W'), &id('K'), 60)
        .await
        .unwrap()
        .unwrap();
    let fence = fence(&claimed);
    repository
        .begin_node(BeginNode {
            fence: fence.clone(),
            started_event_id: id('B'),
        })
        .await
        .unwrap();
    let output_payload = canonical_output_bytes(&[NativePortValue::new(
        "result",
        graph.external_inputs()[0].value_type().clone(),
        b"deterministic-node-output".to_vec(),
    )
    .unwrap()]);
    let output_hash = ContentHash::digest(&output_payload);
    seed_blob(&pool, &output_hash, output_payload.len()).await;
    let artifact = Artifact::new(
        planned,
        owner(),
        ArtifactKind::Generic,
        "application/vnd.ficant.native-node.v1",
        output_hash.clone(),
        output_payload.len() as u64,
        vec![LineageRef::content_addressed(id('E'), external_hash)],
    )
    .unwrap();
    repository
        .complete_node(CompleteNode {
            fence: fence.clone(),
            artifact: artifact.clone(),
            verified_blob: VerifiedBlobRef::new(output_hash, output_payload.len() as u64).unwrap(),
            verified_payload: output_payload.clone(),
            output_manifest: manifest(&identity, &graph, &artifact, fence.attempt),
            succeeded_event_id: id('C'),
            checkpoint_event_id: id('D'),
        })
        .await
        .unwrap();
    let tasks: Vec<(String, String)> = sqlx::query_as(
        "SELECT node_id::text,state FROM research.execution_tasks
         WHERE tenant_id=$1 AND run_id=$2 ORDER BY node_id",
    )
    .bind(id('T').as_str())
    .bind(id('R').as_str())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        tasks,
        vec![
            (id('N').to_string(), "COMPLETED".to_owned()),
            (id('P').to_string(), "PENDING".to_owned()),
        ]
    );
    let run_state: String = sqlx::query_scalar(
        "SELECT state FROM research.experiment_runs
         WHERE tenant_id=$1 AND experiment_run_id=$2",
    )
    .bind(id('T').as_str())
    .bind(id('R').as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(run_state, "RUNNING");
}

async fn publish_identity(
    pool: &sqlx::PgPool,
    repository: &ficant_storage::postgres::PostgresRepository,
    scope: &ficant_application::ports::AccessScope,
    graph: &ResearchGraph,
    run_id: Ulid,
    external_payload: &[u8],
) -> StoredExecutionIdentity {
    seed_run(pool, &run_id).await;
    seed_run_events(repository, scope, &run_id).await;
    let value = identity_value(graph, &run_id, external_payload);
    let published = repository
        .publish_execution_identity(scope, value.clone())
        .await
        .unwrap();
    assert_eq!(
        repository
            .load_execution_identity(scope, &run_id)
            .await
            .unwrap()
            .unwrap(),
        published
    );
    published
}

fn identity_value(
    graph: &ResearchGraph,
    run_id: &Ulid,
    external_payload: &[u8],
) -> StoredExecutionIdentity {
    let rule_hash = ContentHash::digest(b"rule-pack");
    let reproducibility = ReproducibilityIdentity::new(
        graph,
        ReproducibilityIdentityInput {
            external_inputs: vec![
                ExecutionExternalInput::new(
                    "fixture",
                    graph.external_inputs()[0].value_type().clone(),
                    external_payload.to_vec(),
                )
                .unwrap(),
            ],
            data_snapshot_hash: ContentHash::digest(b"data-snapshot"),
            universe_snapshot_hash: ContentHash::digest(b"universe-snapshot"),
            parameters_hash: ContentHash::digest(b"run-parameters"),
            runtime_image_digest: ContentHash::digest(b"runtime-image"),
            environment_digest: ContentHash::digest(b"environment"),
            seed: 42,
            rule_pack_bindings: vec![RulePackBinding {
                rule_pack_id: id('Q').to_string(),
                version: Version::new(1).unwrap(),
                content_hash: rule_hash,
            }],
            node_implementations: graph
                .nodes()
                .iter()
                .map(|node| NodeImplementation {
                    node_id: node.node_id().clone(),
                    implementation_digest: ContentHash::digest(b"native-implementation"),
                })
                .collect(),
        },
    )
    .unwrap();
    StoredExecutionIdentity {
        owner: owner(),
        graph_id: graph.graph_id().clone(),
        graph_version: graph.version(),
        identity: ExecutionInstanceIdentity::from_reproducibility(run_id.clone(), reproducibility),
        external_input_artifacts: vec![ExternalInputArtifactBinding {
            input_id: "fixture".to_owned(),
            artifact_id: id('E'),
            content_hash: ContentHash::digest(external_payload),
        }],
    }
}

async fn seed_run_events(
    repository: &ficant_storage::postgres::PostgresRepository,
    scope: &ficant_application::ports::AccessScope,
    run_id: &Ulid,
) {
    let instant: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
        .fetch_one(repository.pool())
        .await
        .unwrap();
    let events = run_events_at(run_id, instant);
    for event in [events.0, events.1] {
        repository
            .append(
                AppendJournalEvent::new(
                    scope.clone(),
                    owner(),
                    run_id.clone(),
                    event.sequence(),
                    event.clone(),
                    IdempotencyKey::new(format!("phase4-sit/{run_id}/{}", event.sequence()))
                        .unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }
}

fn run_events_at(
    run_id: &Ulid,
    instant: chrono::DateTime<chrono::Utc>,
) -> (RunJournal, RunJournal) {
    let occurred_at =
        ficant_domain::primitives::MarketTime::new(instant, "UTC", instant.date_naive()).unwrap();
    let event = |sequence: u64, event_type: JournalEventType, previous: Option<ContentHash>| {
        let mut domain = b"ficant/phase4-sit-run-event/v1".to_vec();
        domain.extend_from_slice(run_id.as_str().as_bytes());
        domain.extend_from_slice(&sequence.to_be_bytes());
        let input = RunJournalInput {
            journal_event_id: stable_node_artifact_id(&ContentHash::digest(&domain), run_id),
            run_id: run_id.clone(),
            sequence,
            event_type,
            occurred_at: occurred_at.clone(),
            payload_type: "ficant.run-lifecycle-event".to_owned(),
            payload_schema: "ficant.run-lifecycle-event.v1".to_owned(),
            payload: sequence.to_be_bytes().to_vec(),
            prev_hash: previous,
        };
        let claimed = input.canonical_hash().unwrap();
        RunJournal::new(input, &claimed).unwrap()
    };
    let created = event(1, JournalEventType::RunCreated, None);
    let started = event(
        2,
        JournalEventType::RunStarted,
        Some(created.content_hash().clone()),
    );
    (created, started)
}

async fn read_journal(
    repository: &ficant_storage::postgres::PostgresRepository,
    scope: &ficant_application::ports::AccessScope,
    run_id: Ulid,
) -> Vec<RunJournal> {
    repository
        .read(
            scope,
            run_id,
            PageRequest::new(scope.clone(), None, 100).unwrap(),
        )
        .await
        .unwrap()
        .into_items()
}

fn task(
    identity: &StoredExecutionIdentity,
    planned_artifact_id: Ulid,
    task_id: Ulid,
    key: &str,
) -> EnqueueNode {
    EnqueueNode {
        tenant_id: id('T'),
        task_id,
        run_id: identity.identity.run_id().clone(),
        node_id: id('N'),
        graph_digest: identity.identity.reproducibility().graph_digest().clone(),
        execution_identity_digest: identity.identity.digest().clone(),
        planned_artifact_id,
        task_key: key.to_owned(),
    }
}

fn fence(task: &ficant_storage::lease_queue::LeaseTask) -> NodeLeaseFence {
    NodeLeaseFence {
        tenant_id: task.tenant_id().clone(),
        task_id: task.task_id().clone(),
        run_id: task.run_id().clone(),
        node_id: task.node_id().clone(),
        worker_id: task.lease_owner().unwrap().clone(),
        lease_id: task.lease_id().unwrap().clone(),
        attempt: task.claim_count(),
        execution_identity_digest: task.execution_identity_digest().clone(),
    }
}

fn manifest(
    identity: &StoredExecutionIdentity,
    graph: &ResearchGraph,
    artifact: &Artifact,
    attempt: u64,
) -> Vec<u8> {
    let reproducibility = identity.identity.reproducibility();
    let node = &graph.nodes()[0];
    let implementation = &reproducibility.node_implementations()[0];
    let value_type = graph.external_inputs()[0].value_type();
    let typed = || research_pb::TypedValue {
        type_id: value_type.type_id().to_owned(),
        type_version: value_type.type_version().get(),
        schema_hash: Some(pb_hash(value_type.schema_hash())),
    };
    let lineage = |artifact_id: &Ulid, content_hash: &ContentHash| core_pb::LineageRef {
        object_id: Some(core_pb::Ulid {
            value: artifact_id.to_string(),
        }),
        version: 0,
        content_hash: Some(pb_hash(content_hash)),
    };
    let mut content = research_pb::NodeOutputManifestContent {
        reproducibility_digest: Some(pb_hash(identity.identity.reproducibility_digest())),
        node_id: Some(core_pb::Ulid {
            value: node.node_id().to_string(),
        }),
        node_contract_digest: Some(pb_hash(node.contract().digest())),
        implementation_digest: Some(pb_hash(&implementation.implementation_digest)),
        inputs: vec![research_pb::NodeInputBinding {
            node_id: Some(core_pb::Ulid {
                value: node.node_id().to_string(),
            }),
            port_name: "market_input".to_owned(),
            value_type: Some(typed()),
            resolved_artifact: Some(lineage(
                &identity.external_input_artifacts[0].artifact_id,
                &identity.external_input_artifacts[0].content_hash,
            )),
            content_hash: Some(pb_hash(&identity.external_input_artifacts[0].content_hash)),
            declared_source: Some(
                research_pb::node_input_binding::DeclaredSource::ExternalInputId(
                    "fixture".to_owned(),
                ),
            ),
        }],
        outputs: vec![research_pb::NodeOutputBinding {
            port_name: "result".to_owned(),
            value_type: Some(typed()),
            artifact: Some(lineage(artifact.id(), artifact.content_hash())),
            // Port payload and canonical envelope are intentionally different address spaces.
            content_hash: Some(pb_hash(&ContentHash::digest(b"deterministic-node-output"))),
        }],
        manifest_hash: None,
    };
    content.manifest_hash = Some(pb_hash(&ContentHash::digest(&content.encode_to_vec())));
    research_pb::NodeOutputManifest {
        execution: Some(pb_execution(identity)),
        attempt: u32::try_from(attempt).unwrap(),
        content: Some(content),
    }
    .encode_to_vec()
}

fn pb_hash(value: &ContentHash) -> core_pb::Sha256 {
    core_pb::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn tampered_manifests(valid: &[u8]) -> Vec<Vec<u8>> {
    fn reseal(mut value: research_pb::NodeOutputManifest) -> Vec<u8> {
        if let Some(content) = value.content.as_mut() {
            content.manifest_hash = None;
            content.manifest_hash = Some(pb_hash(&ContentHash::digest(&content.encode_to_vec())));
        }
        value.encode_to_vec()
    }
    let original = research_pb::NodeOutputManifest::decode(valid).unwrap();
    let mut values = Vec::new();

    let mut value = original.clone();
    value.execution.as_mut().unwrap().run_id = Some(core_pb::Ulid {
        value: id('P').to_string(),
    });
    values.push(reseal(value));

    let mut value = original.clone();
    value.execution.as_mut().unwrap().digest = Some(pb_hash(&ContentHash::digest(b"execution")));
    values.push(reseal(value));

    let mut value = original.clone();
    value
        .execution
        .as_mut()
        .unwrap()
        .reproducibility
        .as_mut()
        .unwrap()
        .runtime_image_digest = Some(pb_hash(&ContentHash::digest(b"runtime")));
    values.push(reseal(value));

    let mut value = original.clone();
    value.attempt = value.attempt.saturating_add(1);
    values.push(reseal(value));

    let mut value = original.clone();
    value.content.as_mut().unwrap().node_id = Some(core_pb::Ulid {
        value: id('P').to_string(),
    });
    values.push(reseal(value));

    let mut value = original.clone();
    value.content.as_mut().unwrap().node_contract_digest =
        Some(pb_hash(&ContentHash::digest(b"contract")));
    values.push(reseal(value));

    let mut value = original.clone();
    value.content.as_mut().unwrap().implementation_digest =
        Some(pb_hash(&ContentHash::digest(b"implementation")));
    values.push(reseal(value));

    let mut value = original.clone();
    "other_input".clone_into(&mut value.content.as_mut().unwrap().inputs[0].port_name);
    values.push(reseal(value));

    let mut value = original.clone();
    value.content.as_mut().unwrap().inputs[0].resolved_artifact = Some(core_pb::LineageRef {
        object_id: Some(core_pb::Ulid {
            value: id('P').to_string(),
        }),
        version: 0,
        content_hash: Some(pb_hash(&ContentHash::digest(b"input-artifact"))),
    });
    values.push(reseal(value));

    let mut value = original.clone();
    let mut wrong_type = value.content.as_ref().unwrap().outputs[0]
        .value_type
        .clone()
        .unwrap();
    "ficant.other".clone_into(&mut wrong_type.type_id);
    value.content.as_mut().unwrap().outputs[0].value_type = Some(research_pb::TypedValue {
        type_id: "ficant.other".to_owned(),
        ..wrong_type
    });
    values.push(reseal(value));

    let mut value = original.clone();
    value.content.as_mut().unwrap().outputs[0].content_hash =
        Some(pb_hash(&ContentHash::digest(b"output")));
    // A port-hash mutation without updating the canonical manifest hash must be rejected.
    values.push(value.encode_to_vec());

    let mut value = original;
    value.content.as_mut().unwrap().manifest_hash =
        Some(pb_hash(&ContentHash::digest(b"manifest")));
    values.push(value.encode_to_vec());
    values
}

fn pb_execution(identity: &StoredExecutionIdentity) -> research_pb::ExecutionInstanceIdentity {
    let expected = identity.identity.reproducibility();
    research_pb::ExecutionInstanceIdentity {
        run_id: Some(core_pb::Ulid {
            value: identity.identity.run_id().to_string(),
        }),
        reproducibility: Some(research_pb::ReproducibilityIdentity {
            graph_digest: Some(pb_hash(expected.graph_digest())),
            data_snapshot_hash: Some(pb_hash(expected.data_snapshot_hash())),
            universe_snapshot_hash: Some(pb_hash(expected.universe_snapshot_hash())),
            parameters_hash: Some(pb_hash(expected.parameters_hash())),
            runtime_image_digest: Some(pb_hash(expected.runtime_image_digest())),
            environment_digest: Some(pb_hash(expected.environment_digest())),
            seed: expected.seed(),
            rule_packs: expected
                .rule_pack_bindings()
                .iter()
                .map(|binding| research_pb::RulePackBinding {
                    rule_pack_id: Some(core_pb::Ulid {
                        value: binding.rule_pack_id.clone(),
                    }),
                    version: binding.version.get(),
                    content_hash: Some(pb_hash(&binding.content_hash)),
                })
                .collect(),
            node_implementations: expected
                .node_implementations()
                .iter()
                .map(|binding| research_pb::NodeImplementationBinding {
                    node_id: Some(core_pb::Ulid {
                        value: binding.node_id.to_string(),
                    }),
                    implementation_digest: Some(pb_hash(&binding.implementation_digest)),
                })
                .collect(),
            external_inputs: expected
                .external_inputs()
                .iter()
                .zip(&identity.external_input_artifacts)
                .map(|(input, binding)| research_pb::ExecutionExternalInput {
                    input_id: input.input_id().to_owned(),
                    value_type: Some(research_pb::TypedValue {
                        type_id: input.value_type().type_id().to_owned(),
                        type_version: input.value_type().type_version().get(),
                        schema_hash: Some(pb_hash(input.value_type().schema_hash())),
                    }),
                    resolved_artifact: Some(core_pb::LineageRef {
                        object_id: Some(core_pb::Ulid {
                            value: binding.artifact_id.to_string(),
                        }),
                        version: 0,
                        content_hash: Some(pb_hash(&binding.content_hash)),
                    }),
                    content_hash: Some(pb_hash(input.content_hash())),
                })
                .collect(),
            digest: Some(pb_hash(identity.identity.reproducibility_digest())),
        }),
        digest: Some(pb_hash(identity.identity.digest())),
    }
}

async fn seed_shared_dependencies(pool: &sqlx::PgPool) {
    let data_hash = ContentHash::digest(b"data-snapshot");
    let universe_hash = ContentHash::digest(b"universe-snapshot");
    let manifest_hash = ContentHash::digest(b"manifest");
    seed_blob(pool, &data_hash, 1).await;
    seed_blob(pool, &universe_hash, 1).await;
    seed_blob(pool, &manifest_hash, 1).await;
    sqlx::query(
        "INSERT INTO research.data_snapshots
         (tenant_id,data_snapshot_id,owner_id,visible_at,as_of,schema_hash,manifest_hash,
          content_hash,idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,$4,$5,$6,'phase4-data',$7,$8)",
    )
    .bind(id('T').as_str())
    .bind(id('Y').as_str())
    .bind(id('Z').as_str())
    .bind(hex(&ContentHash::digest(b"schema")))
    .bind(hex(&manifest_hash))
    .bind(hex(&data_hash))
    .bind(vec![1_u8; 32])
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO research.universe_snapshots
         (tenant_id,universe_snapshot_id,owner_id,filter_digest,content_hash,
          idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,$4,$5,'phase4-universe',$6,$7)",
    )
    .bind(id('T').as_str())
    .bind(id('V').as_str())
    .bind(id('Z').as_str())
    .bind(hex(&ContentHash::digest(b"filter")))
    .bind(hex(&universe_hash))
    .bind(vec![2_u8; 32])
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.market_rule_packs
         (tenant_id,rule_pack_id,version,owner_id,market,rule_type,source,
          effective_from,effective_to,verification_status,content_hash,payload)
         VALUES ($1,$2,1,$3,'CGB','pricing','fixture',
                 CURRENT_TIMESTAMP-INTERVAL '1 day',CURRENT_TIMESTAMP+INTERVAL '1 day',
                 'VERIFIED',$4,$5)",
    )
    .bind(id('T').as_str())
    .bind(id('Q').as_str())
    .bind(id('Z').as_str())
    .bind(hex(&ContentHash::digest(b"rule-pack")))
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_external_artifact(pool: &sqlx::PgPool, hash: &ContentHash, size: usize) {
    seed_blob(pool, hash, size).await;
    sqlx::query(
        "INSERT INTO research.artifacts
         (tenant_id,artifact_id,owner_id,kind,media_type,content_hash,blob_size,
          idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,'GENERIC','application/fixture',$4,$5,
                 'phase4-external',$6,$7)",
    )
    .bind(id('T').as_str())
    .bind(id('E').as_str())
    .bind(id('Z').as_str())
    .bind(hex(hash))
    .bind(i64::try_from(size).unwrap())
    .bind(vec![3_u8; 32])
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_blob(pool: &sqlx::PgPool, hash: &ContentHash, size: usize) {
    let encoded = hex(hash);
    sqlx::query(
        "INSERT INTO storage.blobs(tenant_id,content_hash,object_key,blob_size)
         VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING",
    )
    .bind(id('T').as_str())
    .bind(&encoded)
    .bind(format!("immutable/{encoded}"))
    .bind(i64::try_from(size).unwrap())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_run(pool: &sqlx::PgPool, run_id: &Ulid) {
    let payload = encode_run_payload(run_id);
    sqlx::query(
        "INSERT INTO research.experiment_runs
         (tenant_id,experiment_run_id,owner_id,state,revision,idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,'RUNNING',2,$4,$5,$6)",
    )
    .bind(id('T').as_str())
    .bind(run_id.as_str())
    .bind(id('Z').as_str())
    .bind(format!("phase4-run-{run_id}"))
    .bind(ContentHash::digest(&payload).as_bytes().as_slice())
    .bind(&payload)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO research.experiment_run_revisions
         (tenant_id,experiment_run_id,revision,state,payload)
         VALUES ($1,$2,2,'RUNNING',$3)",
    )
    .bind(id('T').as_str())
    .bind(run_id.as_str())
    .bind(payload)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO research.run_journal_sequences(tenant_id,run_id,next_sequence)
         VALUES ($1,$2,1)",
    )
    .bind(id('T').as_str())
    .bind(run_id.as_str())
    .execute(pool)
    .await
    .unwrap();
}

fn encode_run_payload(run_id: &Ulid) -> Vec<u8> {
    let mut output = b"FSTO\0\x01".to_vec();
    string(&mut output, run_id.as_str());
    string(&mut output, id('T').as_str());
    string(&mut output, id('Z').as_str());
    lineage(
        &mut output,
        &LineageRef::content_addressed(id('Y'), ContentHash::digest(b"data-snapshot")),
    );
    lineage(
        &mut output,
        &LineageRef::content_addressed(id('V'), ContentHash::digest(b"universe-snapshot")),
    );
    u64_value(&mut output, 1);
    string(&mut output, id('Q').as_str());
    u64_value(&mut output, 1);
    bytes(
        &mut output,
        ContentHash::digest(b"runtime-image").as_bytes(),
    );
    bytes(
        &mut output,
        ContentHash::digest(b"run-parameters").as_bytes(),
    );
    u64_value(&mut output, 42);
    output.push(2);
    u64_value(&mut output, 2);
    output
}

fn lineage(output: &mut Vec<u8>, value: &LineageRef) {
    string(output, value.object_id().as_str());
    output.push(u8::from(value.version().is_some()));
    if let Some(version) = value.version() {
        u64_value(output, version.get());
    }
    output.push(u8::from(value.content_hash().is_some()));
    if let Some(hash) = value.content_hash() {
        bytes(output, hash.as_bytes());
    }
}

fn string(output: &mut Vec<u8>, value: &str) {
    bytes(output, value.as_bytes());
}

fn bytes(output: &mut Vec<u8>, value: &[u8]) {
    u64_value(output, value.len() as u64);
    output.extend_from_slice(value);
}

fn u64_value(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}
