#![allow(clippy::duration_suboptimal_units)]

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use ficant_api::RatesGrpcService;
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
use ficant_contracts::ficant::market::v1::{
    CouponTaxClaimScope, GrossCouponTaxBasis, SubjectCouponTaxTreatment, TaxRoundingMode,
};
use ficant_contracts::ficant::rates::v1::{
    AlgorithmBinding, AnalysisContext, AnalysisInputBinding, AnalysisInputRole, AnalysisUnits,
    AnalyzeBondRequest, CalendarRequirement, ObjectBinding, ResultMetadata, RiskSummary,
    SnapshotBinding, analysis_input_binding, analyze_bond_request,
};
use ficant_contracts::ficant::research::v1::{
    ReadNodeOutputRequest, experiment_service_server::ExperimentService,
};
use ficant_domain::analytics::{
    ALGORITHM_ID, AnalyticsMode, AnalyticsObjectRef, BondTerms, BusinessDayConvention,
    CONVENTION_PROFILE, CalendarBinding, CalendarRequirement as DomainCalendarRequirement,
    CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::market::{BondTaxAttributes, IncomeTaxStatus, ValueAddedTaxStatus};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime as DomainMarketTime, OwnerRef, Ulid, Version,
    VersionRef as DomainVersionRef,
};
use ficant_domain::research::{
    Artifact, ArtifactKind, GraphExternalInput, GraphExternalInputBinding, JournalEventType,
    ResearchEdge, ResearchGraph, ResearchGraphInput, ResearchNode, RunJournal, RunJournalInput,
    RunState,
};
use ficant_domain::{ContentAddressed, Lineaged};
use ficant_native_nodes::{
    CgbBondAnalyticsNativeNode, CgbBondRiskSummaryNativeNode, MATERIALIZED_INPUT_PORT,
    REQUEST_PORT, RESULT_PORT, RISK_INPUT_PORT, RISK_OUTPUT_PORT, analyze_bond_request_type,
    cgb_bond_analytics_contract, cgb_bond_risk_summary_contract, encode_materialized_bond_input,
    materialized_bond_input_type, native_node_source_digest,
};
use ficant_runtime::{NativeNode, decode_canonical_output_bytes, replay_graph_execution};
use ficant_server::{ServerSettings, build_grpc_services_with_experiment};
use ficant_storage::s3::S3BlobStore;
use ficant_worker::{
    ProductionWorkerBackend, WorkerBackend, WorkerConfig, canonical_environment_digest, run_claimed,
};
use prost::Message;
use tonic::{Code, Request};

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";
const RATE_UNIT_ID: &str = "01K2CGBVAT0000000000000000";

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

fn snapshot(suffix: char) -> SnapshotBinding {
    SnapshotBinding {
        snapshot_id: Some(proto_id(suffix)),
        content_hash: Some(proto_hash(&hash(format!("object-{suffix}").as_bytes()))),
    }
}

fn unit(suffix: char) -> UnitRef {
    UnitRef {
        unit_id: Some(proto_id(suffix)),
        version: 1,
    }
}

fn rate_unit() -> UnitRef {
    UnitRef {
        unit_id: Some(ProtoUlid {
            value: RATE_UNIT_ID.to_owned(),
        }),
        version: 1,
    }
}

fn rate_unit_object() -> ObjectBinding {
    ObjectBinding {
        object: Some(VersionRef {
            id: rate_unit().unit_id,
            version: 1,
        }),
        content_hash: Some(proto_hash(&hash(b"authoritative-rate-unit"))),
    }
}

fn private_tax_treatment() -> SubjectCouponTaxTreatment {
    SubjectCouponTaxTreatment {
        value_added_tax_profile: "cn-vat-general-taxpayer".to_owned(),
        income_tax_profile: "cn-cgb-interest-cit-exempt".to_owned(),
        value_added_tax_rate: Some(decimal("6", 2, rate_unit())),
        income_tax_rate: Some(decimal("0", 0, rate_unit())),
        gross_coupon_basis: GrossCouponTaxBasis::VatIncluded as i32,
        rounding: TaxRoundingMode::TiesToEven as i32,
        claim_scope: CouponTaxClaimScope::CouponOutputVatBeforeInputCredit as i32,
    }
}

fn semantic_hash() -> ContentHash {
    ContentHash::from_bytes(&[
        0x54, 0xfa, 0x5a, 0xdb, 0xeb, 0x8b, 0x16, 0x4d, 0xc7, 0x79, 0xec, 0xc2, 0x50, 0xab, 0x62,
        0x2a, 0xb5, 0x74, 0xcd, 0xeb, 0x36, 0xf2, 0xb6, 0xda, 0x58, 0xf4, 0xd8, 0x77, 0xce, 0x51,
        0x06, 0x0a,
    ])
    .unwrap()
}

fn decimal(coefficient: &str, scale: u32, unit: UnitRef) -> DecimalValue {
    DecimalValue {
        coefficient: coefficient.to_owned(),
        scale,
        unit: Some(unit),
    }
}

fn proto_time(instant: &str, local_trading_date: &str) -> MarketTime {
    let instant = DateTime::parse_from_rfc3339(instant).unwrap();
    MarketTime {
        instant: Some(prost_types::Timestamp {
            seconds: instant.timestamp(),
            nanos: instant.timestamp_subsec_nanos().cast_signed(),
        }),
        market_timezone: "Asia/Shanghai".to_owned(),
        local_trading_date: local_trading_date.to_owned(),
    }
}

fn golden_request() -> AnalyzeBondRequest {
    let units = AnalysisUnits {
        currency_amount: Some(unit('A')),
        price_per_100: Some(unit('B')),
        rate: Some(rate_unit()),
        years: Some(unit('D')),
        years_squared: Some(unit('E')),
        dv01_per_100: Some(unit('F')),
        dv01: Some(unit('G')),
        dimensionless: Some(unit('H')),
        contract_count: Some(unit('J')),
    };
    let valuation_at = proto_time("2026-07-13T10:00:00+08:00", "2026-07-13");
    AnalyzeBondRequest {
        context: Some(AnalysisContext {
            owner: Some(ProtoOwnerRef {
                tenant_id: Some(proto_id('0')),
                owner_id: Some(proto_id('1')),
            }),
            algorithm: Some(AlgorithmBinding {
                algorithm_id: ALGORITHM_ID.to_owned(),
                algorithm_version: 1,
                convention_profile: CONVENTION_PROFILE.to_owned(),
                abi_version: 1,
            }),
            units: Some(units.clone()),
            subject_ref: Some(VersionRef {
                id: Some(proto_id('S')),
                version: 1,
            }),
            knowledge_at: Some(valuation_at.clone()),
        }),
        bond: Some(object('N')),
        valuation_at: Some(valuation_at),
        settlement_date: "2026-07-14".to_owned(),
        calendar_requirement: CalendarRequirement::ReferenceReplay as i32,
        calendar: Some(object('C')),
        input: Some(analyze_bond_request::Input::YieldToMaturity(decimal(
            "155",
            4,
            rate_unit(),
        ))),
        data_snapshot: Some(snapshot('M')),
        tax_rule_pack: Some(object('K')),
    }
}

fn domain_object(suffix: char) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        DomainVersionRef::new(id(suffix), Version::new(1).unwrap()),
        hash(format!("object-{suffix}").as_bytes()),
    )
}

fn materialized_input() -> ficant_domain::analytics::BondAnalyticsInput {
    let instant = DateTime::parse_from_rfc3339("2026-07-13T10:00:00+08:00")
        .unwrap()
        .with_timezone(&Utc);
    ficant_domain::analytics::BondAnalyticsInput::new(
        owner(),
        domain_object('N'),
        domain_object('K'),
        domain_object('M'),
        DomainMarketTime::new(
            instant,
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2026, 7, 13).unwrap(),
        )
        .unwrap(),
        NaiveDate::from_ymd_opt(2026, 7, 14).unwrap(),
        DomainCalendarRequirement::ReferenceReplay,
        CalendarBinding::new(
            id('C').to_string(),
            Version::new(1).unwrap(),
            hash(b"object-C"),
            NaiveDate::from_ymd_opt(2005, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2035, 12, 31).unwrap(),
            vec![],
            vec![],
        )
        .unwrap(),
        BondTerms::with_issuance(
            NaiveDate::from_ymd_opt(2026, 4, 15).unwrap(),
            NaiveDate::from_ymd_opt(2026, 4, 15).unwrap(),
            NaiveDate::from_ymd_opt(2031, 4, 15).unwrap(),
            CouponFrequency::Annual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            FixedDecimal::from_scaled(15_000_000_000),
            FixedDecimal::from_scaled(100_000_000_000_000),
            FixedDecimal::from_scaled(100_000_000_000_000),
            BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Exempt),
        )
        .unwrap(),
        AnalyticsMode::YieldIn,
        FixedDecimal::from_scaled(15_500_000_000),
    )
    .unwrap()
}

fn evidence_object(role: AnalysisInputRole, binding: ObjectBinding) -> AnalysisInputBinding {
    AnalysisInputBinding {
        role: role as i32,
        owner: Some(ProtoOwnerRef {
            tenant_id: Some(proto_id('0')),
            owner_id: Some(proto_id('1')),
        }),
        binding: Some(analysis_input_binding::Binding::Object(binding)),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    }
}

fn effective_evidence_object(
    role: AnalysisInputRole,
    binding: ObjectBinding,
) -> AnalysisInputBinding {
    let mut evidence = evidence_object(role, binding);
    evidence.effective_from = Some(proto_time("2005-01-01T00:00:00+08:00", "2005-01-01"));
    evidence.effective_to = Some(proto_time("2035-12-31T23:59:59+08:00", "2035-12-31"));
    evidence
}

fn supplied_metadata(
    request: &AnalyzeBondRequest,
    input: &ficant_domain::analytics::BondAnalyticsInput,
) -> ResultMetadata {
    let mut consumed_inputs = vec![
        evidence_object(AnalysisInputRole::Subject, object('S')),
        evidence_object(AnalysisInputRole::Bond, object('N')),
        effective_evidence_object(AnalysisInputRole::Calendar, object('C')),
        effective_evidence_object(AnalysisInputRole::TaxRulePack, object('K')),
        AnalysisInputBinding {
            role: AnalysisInputRole::DataSnapshot as i32,
            owner: Some(ProtoOwnerRef {
                tenant_id: Some(proto_id('0')),
                owner_id: Some(proto_id('1')),
            }),
            binding: Some(analysis_input_binding::Binding::Snapshot(snapshot('M'))),
            observed_at: Some(golden_request().valuation_at.unwrap()),
            visible_at: Some(golden_request().valuation_at.unwrap()),
            effective_from: None,
            effective_to: None,
        },
    ];
    for suffix in ['A', 'B', 'D', 'E', 'F', 'G', 'H', 'J'] {
        consumed_inputs.push(evidence_object(AnalysisInputRole::Unit, object(suffix)));
    }
    consumed_inputs.push(evidence_object(AnalysisInputRole::Unit, rate_unit_object()));
    consumed_inputs.sort_by_key(prost::Message::encode_to_vec);
    RatesGrpcService::canonical_materialized_bond_metadata(
        request,
        input,
        &RatesGrpcService::canonical_v2_coupon_tax_treatment(
            input,
            &private_tax_treatment(),
            semantic_hash().as_bytes(),
        )
        .unwrap(),
        &consumed_inputs,
    )
    .unwrap()
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
    let materialized_input = materialized_input();
    let materialized_bytes = encode_materialized_bond_input(
        &materialized_input,
        &private_tax_treatment(),
        &semantic_hash(),
        &supplied_metadata(&request, &materialized_input),
    );
    let materialized_hash = ContentHash::digest(&materialized_bytes);
    let materialized_size = i64::try_from(materialized_bytes.len()).unwrap();
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
    let materialized_artifact_id = id('3');
    let materialized_verified = publish_blob(
        &blobs,
        &scope,
        &materialized_bytes,
        "worker-sit/materialized-bond-input",
    )
    .await;
    let materialized_artifact = Artifact::new(
        materialized_artifact_id.clone(),
        owner(),
        ArtifactKind::Generic,
        "application/x-ficant-materialized-bond-input; version=1",
        materialized_hash.clone(),
        u64::try_from(materialized_bytes.len()).unwrap(),
        vec![LineageRef::content_addressed(id('M'), data_hash.clone())],
    )
    .unwrap();
    repository
        .publish_verified_blob(
            PublishArtifact::new(
                materialized_artifact,
                materialized_verified,
                IdempotencyKey::new("worker-sit/materialized-bond-input-artifact").unwrap(),
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
        vec![
            GraphExternalInput::new("bond-request", analyze_bond_request_type()).unwrap(),
            GraphExternalInput::new("materialized-bond-input", materialized_bond_input_type())
                .unwrap(),
        ],
        vec![
            GraphExternalInputBinding::new("bond-request", node_id.clone(), REQUEST_PORT).unwrap(),
            GraphExternalInputBinding::new(
                "materialized-bond-input",
                node_id.clone(),
                MATERIALIZED_INPUT_PORT,
            )
            .unwrap(),
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
    let materialized_external = ExecutionExternalInput::new(
        "materialized-bond-input",
        materialized_bond_input_type(),
        materialized_bytes,
    )
    .unwrap();
    let reproducibility = ReproducibilityIdentity::new(
        &graph,
        ReproducibilityIdentityInput {
            external_inputs: vec![external, materialized_external],
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
                external_input_artifacts: vec![
                    ExternalInputArtifactBinding {
                        input_id: "bond-request".to_owned(),
                        artifact_id: request_artifact_id.clone(),
                        content_hash: request_hash.clone(),
                    },
                    ExternalInputArtifactBinding {
                        input_id: "materialized-bond-input".to_owned(),
                        artifact_id: materialized_artifact_id.clone(),
                        content_hash: materialized_hash.clone(),
                    },
                ],
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
    sqlx::query(
        "UPDATE storage.blobs SET blob_size=blob_size+1
         WHERE tenant_id=$1 AND content_hash=$2",
    )
    .bind(owner().tenant_id().as_str())
    .bind(hex(&materialized_hash))
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        backend.read_inputs(&abandoned, &loaded).await.is_err(),
        "tampered materialized input must fail before native execution"
    );
    sqlx::query(
        "UPDATE storage.blobs SET blob_size=$3
         WHERE tenant_id=$1 AND content_hash=$2",
    )
    .bind(owner().tenant_id().as_str())
    .bind(hex(&materialized_hash))
    .bind(materialized_size)
    .execute(&pool)
    .await
    .unwrap();
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

    assert_eq!(artifact.lineage().len(), 2);
    assert!(
        artifact
            .lineage()
            .iter()
            .any(|value| value.object_id() == &request_artifact_id)
    );
    assert!(
        artifact
            .lineage()
            .iter()
            .any(|value| value.object_id() == &materialized_artifact_id)
    );
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

    let settings = ServerSettings::try_from_values(&server_values(
        &config.database_url,
        &config.s3_endpoint,
        &config.s3_bucket,
        &config.s3_access_key,
        &config.s3_secret_key,
        &config.runtime_image_digest,
        &config.native_source_digest,
    ))
    .unwrap();
    let (_, _, experiment) = build_grpc_services_with_experiment(&settings).unwrap();
    let observed = experiment
        .read_node_output(Request::new(ReadNodeOutputRequest {
            run_id: Some(proto_id('R')),
            node_id: Some(proto_id('A')),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(observed.outputs.len(), 1);
    assert_eq!(observed.outputs[0].port_name, RESULT_PORT);
    assert_eq!(observed.outputs[0].payload, outputs[0].payload());
    assert_eq!(
        observed.outputs[0].content_hash.as_ref().unwrap(),
        &proto_hash(outputs[0].content_hash())
    );
    assert!(
        observed.manifest.is_some(),
        "observability read must return the persisted manifest"
    );

    sqlx::query(
        "UPDATE storage.blobs SET blob_size=blob_size+1
         WHERE tenant_id=$1 AND content_hash=$2",
    )
    .bind(owner().tenant_id().as_str())
    .bind(hex(artifact.content_hash()))
    .execute(&pool)
    .await
    .unwrap();
    let integrity_error = experiment
        .read_node_output(Request::new(ReadNodeOutputRequest {
            run_id: Some(proto_id('R')),
            node_id: Some(proto_id('A')),
        }))
        .await
        .expect_err("observability read must fail closed on Ceph metadata drift");
    assert_ne!(integrity_error.code(), Code::Ok);
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

    let events = read_journal(&repository, &scope, run_id).await;
    let replay = replay_graph_execution(&graph, &events).unwrap();
    assert_eq!(replay.run_state(), RunState::Succeeded);
    assert_eq!(replay.completed_nodes(), &[id('A'), id('B')]);
    assert_eq!(
        replay.last_checkpoint().unwrap().output_hash(),
        second_artifact.content_hash()
    );
}

fn server_values(
    database_url: &str,
    s3_endpoint: &str,
    s3_bucket: &str,
    s3_access_key: &str,
    s3_secret_key: &str,
    runtime_image_digest: &ContentHash,
    native_source_digest: &ContentHash,
) -> BTreeMap<String, String> {
    const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";
    BTreeMap::from([
        ("FICANT_GRPC_BIND".to_owned(), "127.0.0.1:50051".to_owned()),
        (
            "FICANT_GRPC_WEB_ALLOWED_ORIGINS".to_owned(),
            "http://127.0.0.1:4174".to_owned(),
        ),
        ("FICANT_PLATFORM_SIGNING_KEY_HEX".to_owned(), KEY.to_owned()),
        ("FICANT_PLATFORM_TRACE_KEY_HEX".to_owned(), KEY.to_owned()),
        (
            "FICANT_EXPERIMENT_DATABASE_URL".to_owned(),
            database_url.to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_S3_ENDPOINT".to_owned(),
            s3_endpoint.to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_S3_BUCKET".to_owned(),
            s3_bucket.to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_S3_ACCESS_KEY".to_owned(),
            s3_access_key.to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_S3_SECRET_KEY".to_owned(),
            s3_secret_key.to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_CURSOR_KEY_HEX".to_owned(),
            KEY.to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_TENANT_ID".to_owned(),
            id('0').as_str().to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_OWNER_ID".to_owned(),
            id('1').as_str().to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_ACTOR_ID".to_owned(),
            id('2').as_str().to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_RUNTIME_IMAGE_DIGEST".to_owned(),
            format!("sha256:{}", hex(runtime_image_digest)),
        ),
        (
            "FICANT_EXPERIMENT_ENVIRONMENT_ATTESTATION".to_owned(),
            "ficant.worker.environment.v1\narch=amd64\nos=linux\nprofile=worker-sit".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_NATIVE_SOURCE_DIGEST".to_owned(),
            format!("sha256:{}", hex(native_source_digest)),
        ),
        (
            "FICANT_LOOPBACK_SUBJECT".to_owned(),
            "phase5a-observer".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_ACTOR_ID".to_owned(),
            id('2').as_str().to_owned(),
        ),
        (
            "FICANT_LOOPBACK_TENANT_ID".to_owned(),
            id('0').as_str().to_owned(),
        ),
        (
            "FICANT_LOOPBACK_ALLOWED_OWNER_IDS".to_owned(),
            id('1').as_str().to_owned(),
        ),
        (
            "FICANT_LOOPBACK_ACTIVE_ROLE".to_owned(),
            "RESEARCHER".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_SCOPES".to_owned(),
            "experiment:read".to_owned(),
        ),
        (
            "FICANT_INPUT_FILE_NDJSON_ROOT".to_owned(),
            env!("CARGO_MANIFEST_DIR").to_owned(),
        ),
        (
            "FICANT_INPUT_FILE_CONNECTION_BINDING".to_owned(),
            "phase4-file-fixture".to_owned(),
        ),
        (
            "FICANT_INPUT_POSTGRES_CONNECTION_BINDING".to_owned(),
            "phase4-postgres-fixture".to_owned(),
        ),
    ])
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
