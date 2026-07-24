#![allow(clippy::duration_suboptimal_units)]

mod support;

use std::time::Duration;

use chrono::DateTime;
use ficant_application::ports::{
    AppendJournalEvent, ArtifactRepository, BeginBlobStage, BlobStore, EnqueueNode,
    ExecutionExternalInput, ExecutionInstanceIdentity, ExternalInputArtifactBinding,
    IdempotencyKey, NodeImplementation, PageRequest, Phase4ExecutionRepository, PublishArtifact,
    ReproducibilityIdentity, ReproducibilityIdentityInput, RulePackBinding, RunJournalRepository,
    StoredExecutionIdentity, VerifyBlobStage, stable_node_artifact_id,
};
use ficant_contracts::ficant::core::v1::{
    DecimalValue, MarketTime, OwnerRef as ProtoOwnerRef, Sha256, Ulid as ProtoUlid, UnitRef,
    VersionRef,
};
use ficant_contracts::ficant::rates::v1::{
    AlgorithmBinding, AnalysisContext, AnalysisUnits, AnalyzeBondRequest, BondTerms,
    CalendarBinding, CalendarRequirement, CouponFrequency, ObjectBinding, RiskSummary,
    analyze_bond_request,
};
use ficant_domain::analytics::{ALGORITHM_ID, CONVENTION_PROFILE};
use ficant_domain::primitives::{ContentHash, LineageRef, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    Artifact, ArtifactKind, GraphExternalInput, GraphExternalInputBinding, JournalEventType,
    ResearchEdge, ResearchGraph, ResearchGraphInput, ResearchNode, RunJournal, RunJournalInput,
    RunState,
};
use ficant_domain::{ContentAddressed, Lineaged};
use ficant_native_nodes::{
    CgbBondAnalyticsNativeNode, CgbBondRiskSummaryNativeNode, REQUEST_PORT, RESULT_PORT,
    RISK_INPUT_PORT, RISK_OUTPUT_PORT, analyze_bond_request_type, cgb_bond_analytics_contract,
    cgb_bond_risk_summary_contract, native_node_source_digest,
};
use ficant_runtime::{NativeNode, decode_canonical_output_bytes, replay_graph_execution};
use ficant_storage::s3::S3BlobStore;
use ficant_worker::{
    ProductionWorkerBackend, WorkerBackend, WorkerConfig, canonical_environment_digest, run_claimed,
};
use prost::Message;

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{PREFIX}{suffix}")).unwrap()
}

fn proto_id(suffix: char) -> ProtoUlid {
    ProtoUlid {
        value: id(suffix).as_str().to_owned(),
    }
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('0'), id('1'))
}

fn hash(label: &[u8]) -> ContentHash {
    ContentHash::digest(label)
}

fn proto_hash(value: &ContentHash) -> Sha256 {
    Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn object(suffix: char) -> ObjectBinding {
    ObjectBinding {
        object: Some(VersionRef {
            id: Some(proto_id(suffix)),
            version: 1,
        }),
        content_hash: Some(proto_hash(&hash(format!("object-{suffix}").as_bytes()))),
    }
}

fn unit(suffix: char) -> UnitRef {
    UnitRef {
        unit_id: Some(proto_id(suffix)),
        version: 1,
    }
}

fn decimal(coefficient: &str, scale: u32, unit: UnitRef) -> DecimalValue {
    DecimalValue {
        coefficient: coefficient.to_owned(),
        scale,
        unit: Some(unit),
    }
}

fn golden_request() -> AnalyzeBondRequest {
    let units = AnalysisUnits {
        currency_amount: Some(unit('A')),
        price_per_100: Some(unit('B')),
        rate: Some(unit('C')),
        years: Some(unit('D')),
        years_squared: Some(unit('E')),
        dv01_per_100: Some(unit('F')),
        dv01: Some(unit('G')),
        dimensionless: Some(unit('H')),
        contract_count: Some(unit('J')),
    };
    let instant = DateTime::parse_from_rfc3339("2026-07-13T10:00:00+08:00").unwrap();
    AnalyzeBondRequest {
        context: Some(AnalysisContext {
            owner: Some(ProtoOwnerRef {
                tenant_id: Some(proto_id('0')),
                owner_id: Some(proto_id('1')),
            }),
            rule_pack: Some(object('K')),
            data_snapshot: Some(object('M')),
            algorithm: Some(AlgorithmBinding {
                algorithm_id: ALGORITHM_ID.to_owned(),
                algorithm_version: 1,
                convention_profile: CONVENTION_PROFILE.to_owned(),
                abi_version: 1,
            }),
            units: Some(units.clone()),
        }),
        bond: Some(object('N')),
        valuation_at: Some(MarketTime {
            instant: Some(prost_types::Timestamp {
                seconds: instant.timestamp(),
                nanos: instant.timestamp_subsec_nanos().cast_signed(),
            }),
            market_timezone: "Asia/Shanghai".to_owned(),
            local_trading_date: "2026-07-13".to_owned(),
        }),
        settlement_date: "2026-07-14".to_owned(),
        calendar_requirement: CalendarRequirement::ReferenceReplay as i32,
        calendar: Some(CalendarBinding {
            calendar_id: "CGB-REFERENCE".to_owned(),
            version: 1,
            content_hash: Some(proto_hash(&hash(b"calendar-cgb-reference-v1"))),
            coverage_start: "2005-01-01".to_owned(),
            coverage_end: "2026-12-31".to_owned(),
            non_business_days: vec![],
            work_weekends: vec![],
        }),
        terms: Some(BondTerms {
            issue_date: "2026-04-15".to_owned(),
            maturity_date: "2030-04-15".to_owned(),
            frequency: CouponFrequency::Annual as i32,
            coupon_rate: Some(decimal("15", 3, units.rate.unwrap())),
            face_amount: Some(decimal("100", 0, units.currency_amount.unwrap())),
        }),
        input: Some(analyze_bond_request::Input::YieldToMaturity(decimal(
            "155",
            4,
            unit('C'),
        ))),
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn production_worker_recovers_and_executes_real_typed_cgb_graph() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let scope = support::access_scope(&owner());
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let blobs = S3BlobStore::new(
        &endpoint,
        bucket.clone(),
        &access_key,
        &secret_key,
        pool.clone(),
    )
    .unwrap();

    let request = golden_request();
    let request_bytes = request.encode_to_vec();
    let request_hash = ContentHash::digest(&request_bytes);
    let data_hash = hash(b"object-M");
    let universe_hash = hash(b"worker-sit-universe");
    let rule_hash = hash(b"object-K");
    seed_dependencies(&pool, &data_hash, &universe_hash, &rule_hash).await;
    let request_artifact_id = id('E');
    let verified = publish_blob(&blobs, &scope, &request_bytes, "worker-sit/request").await;
    let request_artifact = Artifact::new(
        request_artifact_id.clone(),
        owner(),
        ArtifactKind::Generic,
        "application/x-protobuf; message=ficant.rates.v1.AnalyzeBondRequest",
        request_hash.clone(),
        u64::try_from(request_bytes.len()).unwrap(),
        vec![LineageRef::content_addressed(id('M'), data_hash.clone())],
    )
    .unwrap();
    repository
        .publish_verified_blob(
            PublishArtifact::new(
                request_artifact,
                verified,
                IdempotencyKey::new("worker-sit/request-artifact").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let node_id = id('A');
    let second_node_id = id('B');
    let graph = ResearchGraph::new_with_external_inputs(
        ResearchGraphInput {
            graph_id: id('G'),
            version: Version::new(1).unwrap(),
            owner: owner(),
            nodes: vec![
                ResearchNode::new(
                    node_id.clone(),
                    cgb_bond_analytics_contract().unwrap(),
                    hash(b"no-parameters"),
                ),
                ResearchNode::new(
                    second_node_id.clone(),
                    cgb_bond_risk_summary_contract().unwrap(),
                    hash(b"no-parameters"),
                ),
            ],
            edges: vec![
                ResearchEdge::new(
                    node_id.clone(),
                    RESULT_PORT,
                    second_node_id.clone(),
                    RISK_INPUT_PORT,
                )
                .unwrap(),
            ],
        },
        vec![GraphExternalInput::new("bond-request", analyze_bond_request_type()).unwrap()],
        vec![
            GraphExternalInputBinding::new("bond-request", node_id.clone(), REQUEST_PORT).unwrap(),
        ],
    )
    .unwrap();
    repository
        .publish_graph(&scope, graph.clone())
        .await
        .unwrap();

    let run_id = id('R');
    let parameters_hash = hash(b"worker-sit-parameters");
    let runtime_hash = hash(b"worker-sit-runtime");
    seed_run(
        &pool,
        &run_id,
        &data_hash,
        &universe_hash,
        &parameters_hash,
        &runtime_hash,
    )
    .await;
    seed_run_events(&repository, &scope, &run_id).await;

    let executor = CgbBondAnalyticsNativeNode::new(node_id.clone()).unwrap();
    let second_executor = CgbBondRiskSummaryNativeNode::new(second_node_id.clone()).unwrap();
    let environment_attestation =
        "ficant.worker.environment.v1\narch=amd64\nos=linux\nprofile=integration-test";
    let environment_digest = canonical_environment_digest(environment_attestation).unwrap();
    let external =
        ExecutionExternalInput::new("bond-request", analyze_bond_request_type(), request_bytes)
            .unwrap();
    let reproducibility = ReproducibilityIdentity::new(
        &graph,
        ReproducibilityIdentityInput {
            external_inputs: vec![external],
            data_snapshot_hash: data_hash,
            universe_snapshot_hash: universe_hash,
            parameters_hash,
            runtime_image_digest: runtime_hash.clone(),
            environment_digest,
            seed: 7,
            rule_pack_bindings: vec![RulePackBinding {
                rule_pack_id: id('K').to_string(),
                version: Version::new(1).unwrap(),
                content_hash: rule_hash,
            }],
            node_implementations: vec![
                NodeImplementation {
                    node_id: node_id.clone(),
                    implementation_digest: executor.implementation_digest().clone(),
                },
                NodeImplementation {
                    node_id: second_node_id.clone(),
                    implementation_digest: second_executor.implementation_digest().clone(),
                },
            ],
        },
    )
    .unwrap();
    let identity = ExecutionInstanceIdentity::from_reproducibility(run_id.clone(), reproducibility);
    let stored = repository
        .publish_execution_identity(
            &scope,
            StoredExecutionIdentity {
                owner: owner(),
                graph_id: graph.graph_id().clone(),
                graph_version: graph.version(),
                identity,
                external_input_artifacts: vec![ExternalInputArtifactBinding {
                    input_id: "bond-request".to_owned(),
                    artifact_id: request_artifact_id.clone(),
                    content_hash: request_hash.clone(),
                }],
            },
        )
        .await
        .unwrap();
    let planned = stable_node_artifact_id(stored.identity.reproducibility_digest(), &node_id);
    let second_planned =
        stable_node_artifact_id(stored.identity.reproducibility_digest(), &second_node_id);
    repository
        .enqueue_node(EnqueueNode {
            tenant_id: owner().tenant_id().clone(),
            task_id: id('Q'),
            run_id: run_id.clone(),
            node_id,
            graph_digest: graph.digest().clone(),
            execution_identity_digest: stored.identity.digest().clone(),
            planned_artifact_id: planned.clone(),
            task_key: "worker-sit/run-r/node-a".to_owned(),
        })
        .await
        .unwrap();

    let config = WorkerConfig {
        database_url: std::env::var("FICANT_TEST_DATABASE_URL").unwrap(),
        s3_endpoint: endpoint,
        s3_bucket: bucket,
        s3_access_key: access_key,
        s3_secret_key: secret_key,
        worker_id: id('W'),
        runtime_image_digest: runtime_hash,
        environment_attestation: environment_attestation.to_owned(),
        native_source_digest: native_node_source_digest(),
        lease_duration: Duration::from_secs(60),
        renew_interval: Duration::from_secs(20),
        idle_poll_interval: Duration::from_millis(10),
        node_timeout: Duration::from_secs(30),
    };
    let backend = ProductionWorkerBackend::connect(&config).await.unwrap();
    let abandoned = backend
        .claim(&config.worker_id, &id('H'), 60)
        .await
        .unwrap()
        .unwrap();
    let loaded = backend.load(&abandoned, &config.worker_id).await.unwrap();
    backend.begin(&abandoned, &config.worker_id).await.unwrap();
    let inputs = backend.read_inputs(&abandoned, &loaded).await.unwrap();
    let executed = backend.execute(&abandoned, &loaded, inputs).await.unwrap();
    let abandoned_completion = backend
        .promote(&abandoned, &loaded, executed)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE research.execution_tasks
         SET lease_expires_at=CURRENT_TIMESTAMP-INTERVAL '1 second'
         WHERE tenant_id=$1 AND task_id=$2",
    )
    .bind(owner().tenant_id().as_str())
    .bind(id('Q').as_str())
    .execute(&pool)
    .await
    .unwrap();

    let mut recovered_config = config.clone();
    recovered_config.worker_id = id('X');
    let recovered_backend = ProductionWorkerBackend::connect(&recovered_config)
        .await
        .unwrap();
    let claimed = recovered_backend
        .claim(&recovered_config.worker_id, &id('J'), 60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.attempt, 2);
    assert!(
        backend
            .complete(&abandoned, &config.worker_id, abandoned_completion)
            .await
            .is_err(),
        "the expired worker fence must not finalize the promoted output"
    );
    run_claimed(&recovered_backend, &recovered_config, &claimed)
        .await
        .unwrap();
    let artifact = repository
        .get_metadata(&scope, planned)
        .await
        .unwrap()
        .unwrap();
    let second = recovered_backend
        .claim(&recovered_config.worker_id, &id('V'), 60)
        .await
        .unwrap()
        .expect("the first completion must enqueue the second graph node");
    assert_eq!(second.node_id, second_node_id);
    assert_eq!(second.attempt, 1);
    let second_loaded = recovered_backend
        .load(&second, &recovered_config.worker_id)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE storage.blobs SET blob_size=blob_size+1
         WHERE tenant_id=$1 AND content_hash=$2",
    )
    .bind(owner().tenant_id().as_str())
    .bind(hex(artifact.content_hash()))
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        recovered_backend
            .read_inputs(&second, &second_loaded)
            .await
            .is_err(),
        "tampered upstream blob metadata must fail before downstream execution"
    );
    sqlx::query(
        "UPDATE storage.blobs SET blob_size=$3
         WHERE tenant_id=$1 AND content_hash=$2",
    )
    .bind(owner().tenant_id().as_str())
    .bind(hex(artifact.content_hash()))
    .bind(i64::try_from(artifact.blob_size()).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    run_claimed(&recovered_backend, &recovered_config, &second)
        .await
        .unwrap();

    let states: (String, String) = sqlx::query_as(
        "SELECT task.state,run.state
         FROM research.execution_tasks task
         JOIN research.experiment_runs run
           ON run.tenant_id=task.tenant_id AND run.experiment_run_id=task.run_id
         WHERE task.tenant_id=$1 AND task.task_id=$2",
    )
    .bind(owner().tenant_id().as_str())
    .bind(id('Q').as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(states, ("COMPLETED".to_owned(), "SUCCEEDED".to_owned()));

    assert_eq!(artifact.lineage().len(), 1);
    assert_eq!(artifact.lineage()[0].object_id(), &request_artifact_id);
    let output_bytes = blobs
        .probe_verified(artifact.content_hash())
        .await
        .unwrap()
        .expect("completed Artifact must have exact verified Ceph content");
    assert_eq!(
        u64::try_from(output_bytes.len()).unwrap(),
        artifact.blob_size()
    );
    assert_eq!(
        ContentHash::digest(&output_bytes),
        artifact.content_hash().clone()
    );
    let outputs =
        decode_canonical_output_bytes(&output_bytes, Some(artifact.content_hash())).unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].port_name(), "result");
    let second_artifact = repository
        .get_metadata(&scope, second_planned)
        .await
        .unwrap()
        .expect("the second graph node must persist its planned Artifact");
    assert_eq!(second_artifact.lineage().len(), 1);
    assert_eq!(second_artifact.lineage()[0].object_id(), artifact.id());
    assert_eq!(
        second_artifact.lineage()[0].content_hash(),
        Some(artifact.content_hash())
    );
    let risk_bytes = blobs
        .probe_verified(second_artifact.content_hash())
        .await
        .unwrap()
        .expect("downstream RiskSummary Artifact must have verified Ceph content");
    let risk_outputs =
        decode_canonical_output_bytes(&risk_bytes, Some(second_artifact.content_hash())).unwrap();
    assert_eq!(risk_outputs.len(), 1);
    assert_eq!(risk_outputs[0].port_name(), RISK_OUTPUT_PORT);
    let summary = RiskSummary::decode(risk_outputs[0].payload()).unwrap();
    assert!(summary.modified_duration.is_some());
    assert!(summary.convexity.is_some());
    assert!(summary.dv01.is_some());
    assert!(summary.source_metadata.is_some());

    let events = read_journal(&repository, &scope, run_id).await;
    let replay = replay_graph_execution(&graph, &events).unwrap();
    assert_eq!(replay.run_state(), RunState::Succeeded);
    assert_eq!(replay.completed_nodes(), &[id('A'), id('B')]);
    assert_eq!(
        replay.last_checkpoint().unwrap().output_hash(),
        second_artifact.content_hash()
    );
}

async fn publish_blob(
    blobs: &S3BlobStore,
    scope: &ficant_application::ports::AccessScope,
    bytes: &[u8],
    key: &str,
) -> ficant_application::ports::VerifiedBlobRef {
    let size = u64::try_from(bytes.len()).unwrap();
    let staged = blobs
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner(),
                size,
                IdempotencyKey::new(key).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    blobs
        .append_chunk(scope, &staged, bytes.to_vec())
        .await
        .unwrap();
    blobs
        .verify_and_promote(
            VerifyBlobStage::new(scope.clone(), staged, ContentHash::digest(bytes), size).unwrap(),
        )
        .await
        .unwrap()
}

async fn seed_dependencies(
    pool: &sqlx::PgPool,
    data_hash: &ContentHash,
    universe_hash: &ContentHash,
    rule_hash: &ContentHash,
) {
    let manifest_hash = hash(b"worker-sit-manifest");
    seed_blob(pool, data_hash, 1).await;
    seed_blob(pool, universe_hash, 1).await;
    seed_blob(pool, &manifest_hash, 1).await;
    sqlx::query(
        "INSERT INTO research.data_snapshots
         (tenant_id,data_snapshot_id,owner_id,visible_at,as_of,schema_hash,manifest_hash,
          content_hash,idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,$4,$5,$6,$7,$8,$9)",
    )
    .bind(id('0').as_str())
    .bind(id('M').as_str())
    .bind(id('1').as_str())
    .bind(hex(&hash(b"worker-sit-schema")))
    .bind(hex(&manifest_hash))
    .bind(hex(data_hash))
    .bind("worker-sit-data")
    .bind(vec![1_u8; 32])
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO research.universe_snapshots
         (tenant_id,universe_snapshot_id,owner_id,filter_digest,content_hash,
          idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(id('0').as_str())
    .bind(id('V').as_str())
    .bind(id('1').as_str())
    .bind(hex(&hash(b"worker-sit-filter")))
    .bind(hex(universe_hash))
    .bind("worker-sit-universe")
    .bind(vec![2_u8; 32])
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.market_rule_packs
         (tenant_id,rule_pack_id,version,owner_id,market,rule_type,source,
          effective_from,effective_to,verification_status,content_hash,payload)
         VALUES ($1,$2,1,$3,'CGB','pricing','worker-sit',
                 CURRENT_TIMESTAMP-INTERVAL '1 day',CURRENT_TIMESTAMP+INTERVAL '1 day',
                 'VERIFIED',$4,$5)",
    )
    .bind(id('0').as_str())
    .bind(id('K').as_str())
    .bind(id('1').as_str())
    .bind(hex(rule_hash))
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_run(
    pool: &sqlx::PgPool,
    run_id: &Ulid,
    data_hash: &ContentHash,
    universe_hash: &ContentHash,
    parameters_hash: &ContentHash,
    runtime_hash: &ContentHash,
) {
    let payload = encode_run_payload(
        run_id,
        data_hash,
        universe_hash,
        parameters_hash,
        runtime_hash,
    );
    sqlx::query(
        "INSERT INTO research.experiment_runs
         (tenant_id,experiment_run_id,owner_id,state,revision,idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,'RUNNING',2,$4,$5,$6)",
    )
    .bind(id('0').as_str())
    .bind(run_id.as_str())
    .bind(id('1').as_str())
    .bind(format!("worker-sit-run-{run_id}"))
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
    .bind(id('0').as_str())
    .bind(run_id.as_str())
    .bind(payload)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO research.run_journal_sequences(tenant_id,run_id,next_sequence)
         VALUES ($1,$2,1)",
    )
    .bind(id('0').as_str())
    .bind(run_id.as_str())
    .execute(pool)
    .await
    .unwrap();
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
        let event_id = stable_node_artifact_id(
            &ContentHash::digest(format!("worker-sit/{run_id}/{sequence}").as_bytes()),
            run_id,
        );
        let input = RunJournalInput {
            journal_event_id: event_id,
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
        let event = RunJournal::new(input, &claimed).unwrap();
        repository
            .append(
                AppendJournalEvent::new(
                    scope.clone(),
                    owner(),
                    run_id.clone(),
                    sequence,
                    event.clone(),
                    IdempotencyKey::new(format!("worker-sit/{run_id}/{sequence}")).unwrap(),
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

fn encode_run_payload(
    run_id: &Ulid,
    data_hash: &ContentHash,
    universe_hash: &ContentHash,
    parameters_hash: &ContentHash,
    runtime_hash: &ContentHash,
) -> Vec<u8> {
    let mut output = b"FSTO\0\x01".to_vec();
    string(&mut output, run_id.as_str());
    string(&mut output, id('0').as_str());
    string(&mut output, id('1').as_str());
    lineage(
        &mut output,
        &LineageRef::content_addressed(id('M'), data_hash.clone()),
    );
    lineage(
        &mut output,
        &LineageRef::content_addressed(id('V'), universe_hash.clone()),
    );
    u64_value(&mut output, 1);
    string(&mut output, id('K').as_str());
    u64_value(&mut output, 1);
    bytes(&mut output, runtime_hash.as_bytes());
    bytes(&mut output, parameters_hash.as_bytes());
    u64_value(&mut output, 7);
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
    if let Some(content_hash) = value.content_hash() {
        bytes(output, content_hash.as_bytes());
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

fn hex(value: &ContentHash) -> String {
    use std::fmt::Write as _;

    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").unwrap();
            output
        })
}

async fn seed_blob(pool: &sqlx::PgPool, content_hash: &ContentHash, size: usize) {
    let encoded = hex(content_hash);
    sqlx::query(
        "INSERT INTO storage.blobs(tenant_id,content_hash,object_key,blob_size)
         VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING",
    )
    .bind(id('0').as_str())
    .bind(&encoded)
    .bind(format!("immutable/{encoded}"))
    .bind(i64::try_from(size).unwrap())
    .execute(pool)
    .await
    .unwrap();
}
