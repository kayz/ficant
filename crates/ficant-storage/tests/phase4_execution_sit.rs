mod support;

use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    AppendJournalEvent, BeginNode, CompleteNode, EnqueueNode, ExecutionExternalInput,
    ExecutionInstanceIdentity, ExternalInputArtifactBinding, FailNode, IdempotencyKey,
    NodeImplementation, NodeLeaseFence, PageRequest, Phase4ExecutionRepository,
    ReproducibilityIdentity, ReproducibilityIdentityInput, RulePackBinding, RunJournalRepository,
    StoredExecutionIdentity, replay_graph_execution, stable_node_artifact_id,
};
use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{ContentHash, LineageRef, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    Artifact, ArtifactKind, DeterminismClass, FilesystemPermission, GraphExternalInput,
    GraphExternalInputBinding, JournalEventType, NodePermissions, PortType, ResearchGraph,
    ResearchGraphInput, ResearchNode, ResearchNodeContract, ResearchNodeContractInput,
    ResourceLimits, RunJournal, RunJournalInput, RunState, TypedValue,
};
use ficant_storage::lease_queue::PostgresLeaseQueue;

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

    let output_payload = b"deterministic-node-output";
    let output_hash = ContentHash::digest(output_payload);
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

    let rollback = repository
        .complete_node(CompleteNode {
            fence: first_fence.clone(),
            artifact: artifact.clone(),
            output_manifest: b"manifest-v1".to_vec(),
            succeeded_event_id: id('C'),
            checkpoint_event_id: id('D'),
            next_task: Some(EnqueueNode {
                tenant_id: id('T'),
                task_id: id('P'),
                run_id: id('R'),
                node_id: id('N'),
                graph_digest: graph.digest().clone(),
                execution_identity_digest: first.identity.digest().clone(),
                planned_artifact_id: planned.clone(),
                task_key: " padded ".to_owned(),
            }),
        })
        .await
        .unwrap_err();
    assert_eq!(
        rollback.category(),
        ApplicationErrorCategory::ValidationFailed
    );
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
        output_manifest: b"manifest-v1".to_vec(),
        succeeded_event_id: id('C'),
        checkpoint_event_id: id('D'),
        next_task: None,
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
            node_implementations: vec![NodeImplementation {
                node_id: id('N'),
                implementation_digest: ContentHash::digest(b"native-implementation"),
            }],
        },
    )
    .unwrap();
    let value = StoredExecutionIdentity {
        owner: owner(),
        graph_id: graph.graph_id().clone(),
        graph_version: graph.version(),
        identity: ExecutionInstanceIdentity::from_reproducibility(run_id.clone(), reproducibility),
        external_input_artifacts: vec![ExternalInputArtifactBinding {
            input_id: "fixture".to_owned(),
            artifact_id: id('E'),
            content_hash: ContentHash::digest(external_payload),
        }],
    };
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

async fn seed_run_events(
    repository: &ficant_storage::postgres::PostgresRepository,
    scope: &ficant_application::ports::AccessScope,
    run_id: &Ulid,
) {
    let instant: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
        .fetch_one(repository.pool())
        .await
        .unwrap();
    let occurred_at =
        ficant_domain::primitives::MarketTime::new(instant, "UTC", instant.date_naive()).unwrap();
    let mut previous = None;
    for (sequence, event_type) in [
        (1_u64, JournalEventType::RunCreated),
        (2_u64, JournalEventType::RunStarted),
    ] {
        let mut domain = b"ficant/phase4-sit-run-event/v1".to_vec();
        domain.extend_from_slice(run_id.as_str().as_bytes());
        domain.extend_from_slice(&sequence.to_be_bytes());
        let event_id = stable_node_artifact_id(&ContentHash::digest(&domain), run_id);
        let input = RunJournalInput {
            journal_event_id: event_id,
            run_id: run_id.clone(),
            sequence,
            event_type,
            occurred_at: occurred_at.clone(),
            payload_type: "ficant.run-lifecycle-event".to_owned(),
            payload_schema: "ficant.run-lifecycle-event.v1".to_owned(),
            payload: sequence.to_be_bytes().to_vec(),
            prev_hash: previous.clone(),
        };
        let claimed = input.canonical_hash().unwrap();
        let event = RunJournal::new(input, &claimed).unwrap();
        repository
            .append(
                AppendJournalEvent::new(
                    scope.clone(),
                    owner(),
                    run_id.clone(),
                    sequence,
                    event.clone(),
                    IdempotencyKey::new(format!("phase4-sit/{run_id}/{sequence}")).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        previous = Some(event.content_hash().clone());
    }
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
