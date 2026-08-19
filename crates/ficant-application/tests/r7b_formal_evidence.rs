use std::sync::Mutex;

use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, ApplicationResult, ComparisonDimension, ExecutionExternalInput,
    FormalOutputRecord, FormalOutputRepository, NodeImplementation, ReproducibilityIdentity,
    ReproducibilityIdentityInput, RulePackBinding, compare_graph_run_dimensions,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, FormalOutputUseCase};
use ficant_domain::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    DeterminismClass, FilesystemPermission, GraphExternalInput, GraphExternalInputBinding,
    NodePermissions, PortType, ResearchGraph, ResearchGraphInput, ResearchNode,
    ResearchNodeContract, ResearchNodeContractInput, ResourceLimits, TypedValue,
};
use ficant_runtime::{
    CodeBinding, FormalImplementationBinding, FormalInputBinding, FormalInputBindingInput,
    FormalInputKind, FormalInputReference, FormalOutputEvidence, FormalOutputEvidenceInput,
    NamedContentRef, RuntimeBinding,
};

struct Repository {
    stored: Mutex<Option<FormalOutputRecord>>,
    fail_write: bool,
}

#[async_trait]
impl FormalOutputRepository for Repository {
    async fn publish(
        &self,
        _scope: &AccessScope,
        record: FormalOutputRecord,
    ) -> ApplicationResult<FormalOutputRecord> {
        if self.fail_write {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::StorageUnavailable,
                true,
            ));
        }
        *self.stored.lock().expect("repository lock") = Some(record.clone());
        Ok(record)
    }

    async fn get(
        &self,
        _scope: &AccessScope,
        output_identity: &ContentHash,
    ) -> ApplicationResult<Option<FormalOutputRecord>> {
        Ok(self
            .stored
            .lock()
            .expect("repository lock")
            .as_ref()
            .filter(|record| record.output_identity() == output_identity)
            .cloned())
    }
}

fn id(value: &str) -> Ulid {
    Ulid::new(value).expect("fixture ULID")
}

fn owner() -> OwnerRef {
    OwnerRef::new(
        id("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        id("01ARZ3NDEKTSV4RRFFQ69G5FAW"),
    )
}

fn scope() -> AccessScope {
    AccessScope::new(
        id("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        id("01ARZ3NDEKTSV4RRFFQ69G5FAY"),
        vec![id("01ARZ3NDEKTSV4RRFFQ69G5FAW")],
    )
    .expect("scope")
}

fn record(payload: &[u8]) -> FormalOutputRecord {
    let subject = FormalInputBinding::new(FormalInputBindingInput {
        role: "subject".to_owned(),
        kind: FormalInputKind::Subject,
        owner: owner(),
        reference: FormalInputReference::Object(
            LineageRef::new(
                id("01ARZ3NDEKTSV4RRFFQ69G5FAX"),
                Some(Version::new(1).expect("version")),
                Some(ContentHash::digest(b"subject")),
            )
            .expect("subject ref"),
        ),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .expect("subject");
    let evidence = FormalOutputEvidence::new(FormalOutputEvidenceInput {
        schema_id: "ficant.test.v1.Output".to_owned(),
        subject,
        consumed_inputs: vec![],
        code: CodeBinding::new(
            "34402344c7d2c9238dc171af52ac4db77eb6b462",
            "f66e03c55703837d6f2aee9959eba482612272f1",
        )
        .expect("code"),
        runtime: RuntimeBinding::new(
            ContentHash::digest(b"image"),
            ContentHash::digest(b"environment"),
        ),
        implementations: vec![],
        parameters_hash: ContentHash::digest(b"parameters"),
        seed: None,
        result_hash: ContentHash::digest(payload),
    })
    .expect("evidence");
    FormalOutputRecord::new(owner(), evidence, payload.to_vec()).expect("record")
}

fn alternate_owner() -> OwnerRef {
    OwnerRef::new(
        id("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        id("01ARZ3NDEKTSV4RRFFQ69G5FAZ"),
    )
}

fn market_time(nanos: u32) -> MarketTime {
    let instant = format!("2026-08-20T01:02:03.{nanos:09}Z")
        .parse()
        .expect("UTC instant");
    MarketTime::new(instant, "UTC", "2026-08-20".parse().expect("date")).expect("market time")
}

fn subject_binding(
    owner: OwnerRef,
    object_id: &str,
    version: u64,
    content: &[u8],
) -> FormalInputBinding {
    FormalInputBinding::new(FormalInputBindingInput {
        role: "subject".to_owned(),
        kind: FormalInputKind::Subject,
        owner,
        reference: FormalInputReference::Object(
            LineageRef::new(
                id(object_id),
                Some(Version::new(version).expect("subject version")),
                Some(ContentHash::digest(content)),
            )
            .expect("subject reference"),
        ),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .expect("subject binding")
}

#[allow(clippy::too_many_arguments)]
fn object_binding(
    role: &str,
    kind: FormalInputKind,
    owner: OwnerRef,
    object_id: &str,
    content: &[u8],
    observed_nanos: u32,
    visible_nanos: u32,
    effective_from_nanos: u32,
    effective_to_nanos: u32,
) -> FormalInputBinding {
    FormalInputBinding::new(FormalInputBindingInput {
        role: role.to_owned(),
        kind,
        owner,
        reference: FormalInputReference::Object(
            LineageRef::new(id(object_id), None, Some(ContentHash::digest(content)))
                .expect("object reference"),
        ),
        observed_at: Some(market_time(observed_nanos)),
        visible_at: Some(market_time(visible_nanos)),
        effective_from: Some(market_time(effective_from_nanos)),
        effective_to: Some(market_time(effective_to_nanos)),
    })
    .expect("object binding")
}

fn named_binding(
    role: &str,
    kind: FormalInputKind,
    identity: &str,
    content: &[u8],
) -> FormalInputBinding {
    FormalInputBinding::new(FormalInputBindingInput {
        role: role.to_owned(),
        kind,
        owner: owner(),
        reference: FormalInputReference::Named(
            NamedContentRef::new(identity, ContentHash::digest(content)).expect("named reference"),
        ),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .expect("named binding")
}

fn complete_evidence_input() -> FormalOutputEvidenceInput {
    FormalOutputEvidenceInput {
        schema_id: "ficant.test.v1.CompleteOutput".to_owned(),
        subject: subject_binding(owner(), "01ARZ3NDEKTSV4RRFFQ69G5FAX", 1, b"subject"),
        consumed_inputs: vec![
            object_binding(
                "data_snapshot",
                FormalInputKind::DataSnapshot,
                owner(),
                "01ARZ3NDEKTSV4RRFFQ69G5FB3",
                b"snapshot",
                10,
                20,
                5,
                30,
            ),
            named_binding(
                "factor_definition",
                FormalInputKind::FactorDefinition,
                "factor.cny.govt.10y",
                b"factor",
            ),
        ],
        code: CodeBinding::new(
            "34402344c7d2c9238dc171af52ac4db77eb6b462",
            "f66e03c55703837d6f2aee9959eba482612272f1",
        )
        .expect("code"),
        runtime: RuntimeBinding::new(
            ContentHash::digest(b"image"),
            ContentHash::digest(b"environment"),
        ),
        implementations: vec![
            FormalImplementationBinding::new("pricing", ContentHash::digest(b"pricing-v1"))
                .expect("implementation"),
        ],
        parameters_hash: ContentHash::digest(b"parameters"),
        seed: Some(42),
        result_hash: ContentHash::digest(b"result"),
    }
}

fn evidence_identity(input: FormalOutputEvidenceInput) -> ContentHash {
    FormalOutputEvidence::new(input)
        .expect("valid evidence")
        .output_identity()
        .clone()
}

#[tokio::test]
async fn publish_is_required_before_success_and_required_read_rechecks_integrity() {
    let repository = Repository {
        stored: Mutex::new(None),
        fail_write: false,
    };
    let use_case = FormalOutputUseCase::new(&repository);
    let stored = use_case
        .publish(&scope(), record(b"canonical-result"))
        .await
        .expect("publish");
    let loaded = use_case
        .get(&scope(), stored.output_identity())
        .await
        .expect("read")
        .expect("record exists");
    assert_eq!(loaded, stored);

    let failing = Repository {
        stored: Mutex::new(None),
        fail_write: true,
    };
    let error = FormalOutputUseCase::new(&failing)
        .publish(&scope(), record(b"canonical-result"))
        .await
        .expect_err("write failure must not become success");
    assert_eq!(
        error.category(),
        ApplicationErrorCategory::StorageUnavailable
    );
    assert!(failing.stored.lock().expect("repository lock").is_none());
}

#[test]
fn payload_drift_is_rejected_before_repository_use() {
    let baseline = record(b"canonical-result");
    assert_eq!(
        FormalOutputRecord::new(
            baseline.owner().clone(),
            baseline.evidence().clone(),
            b"drifted-result".to_vec(),
        )
        .expect_err("payload drift"),
        ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn formal_identity_binds_every_field_and_full_nanosecond_market_time() {
    let baseline_input = complete_evidence_input();
    let baseline = evidence_identity(baseline_input.clone());
    let mut cases = Vec::new();

    let mut changed = baseline_input.clone();
    changed.schema_id = "ficant.test.v1.ChangedOutput".to_owned();
    cases.push(("schema", changed));

    let mut changed = baseline_input.clone();
    changed.subject = subject_binding(
        alternate_owner(),
        "01ARZ3NDEKTSV4RRFFQ69G5FAX",
        1,
        b"subject",
    );
    cases.push(("subject owner", changed));

    let mut changed = baseline_input.clone();
    changed.subject = subject_binding(owner(), "01ARZ3NDEKTSV4RRFFQ69G5FB4", 1, b"subject");
    cases.push(("subject identity", changed));

    let mut changed = baseline_input.clone();
    changed.subject = subject_binding(owner(), "01ARZ3NDEKTSV4RRFFQ69G5FAX", 2, b"subject");
    cases.push(("subject version", changed));

    let mut changed = baseline_input.clone();
    changed.subject = subject_binding(owner(), "01ARZ3NDEKTSV4RRFFQ69G5FAX", 1, b"changed-subject");
    cases.push(("subject hash", changed));

    for (label, binding) in [
        (
            "input role",
            object_binding(
                "price_snapshot",
                FormalInputKind::DataSnapshot,
                owner(),
                "01ARZ3NDEKTSV4RRFFQ69G5FB3",
                b"snapshot",
                10,
                20,
                5,
                30,
            ),
        ),
        (
            "input kind",
            object_binding(
                "data_snapshot",
                FormalInputKind::CurveSnapshot,
                owner(),
                "01ARZ3NDEKTSV4RRFFQ69G5FB3",
                b"snapshot",
                10,
                20,
                5,
                30,
            ),
        ),
        (
            "input owner",
            object_binding(
                "data_snapshot",
                FormalInputKind::DataSnapshot,
                alternate_owner(),
                "01ARZ3NDEKTSV4RRFFQ69G5FB3",
                b"snapshot",
                10,
                20,
                5,
                30,
            ),
        ),
        (
            "input identity",
            object_binding(
                "data_snapshot",
                FormalInputKind::DataSnapshot,
                owner(),
                "01ARZ3NDEKTSV4RRFFQ69G5FB5",
                b"snapshot",
                10,
                20,
                5,
                30,
            ),
        ),
        (
            "input hash",
            object_binding(
                "data_snapshot",
                FormalInputKind::DataSnapshot,
                owner(),
                "01ARZ3NDEKTSV4RRFFQ69G5FB3",
                b"changed-snapshot",
                10,
                20,
                5,
                30,
            ),
        ),
        (
            "observed nanosecond",
            object_binding(
                "data_snapshot",
                FormalInputKind::DataSnapshot,
                owner(),
                "01ARZ3NDEKTSV4RRFFQ69G5FB3",
                b"snapshot",
                11,
                20,
                5,
                30,
            ),
        ),
        (
            "visible nanosecond",
            object_binding(
                "data_snapshot",
                FormalInputKind::DataSnapshot,
                owner(),
                "01ARZ3NDEKTSV4RRFFQ69G5FB3",
                b"snapshot",
                10,
                21,
                5,
                30,
            ),
        ),
        (
            "effective-from nanosecond",
            object_binding(
                "data_snapshot",
                FormalInputKind::DataSnapshot,
                owner(),
                "01ARZ3NDEKTSV4RRFFQ69G5FB3",
                b"snapshot",
                10,
                20,
                6,
                30,
            ),
        ),
        (
            "effective-to nanosecond",
            object_binding(
                "data_snapshot",
                FormalInputKind::DataSnapshot,
                owner(),
                "01ARZ3NDEKTSV4RRFFQ69G5FB3",
                b"snapshot",
                10,
                20,
                5,
                31,
            ),
        ),
    ] {
        let mut changed = baseline_input.clone();
        changed.consumed_inputs[0] = binding;
        cases.push((label, changed));
    }

    let mut changed = baseline_input.clone();
    changed.consumed_inputs[1] = named_binding(
        "factor_definition",
        FormalInputKind::CurveNodeDefinition,
        "factor.cny.govt.10y",
        b"factor",
    );
    cases.push(("named input kind", changed));

    let mut changed = baseline_input.clone();
    changed.consumed_inputs[1] = named_binding(
        "factor_definition",
        FormalInputKind::FactorDefinition,
        "factor.cny.govt.30y",
        b"factor",
    );
    cases.push(("named input identity", changed));

    let mut changed = baseline_input.clone();
    changed.consumed_inputs[1] = named_binding(
        "factor_definition",
        FormalInputKind::FactorDefinition,
        "factor.cny.govt.10y",
        b"changed-factor",
    );
    cases.push(("named input hash", changed));

    let mut changed = baseline_input.clone();
    changed.code = CodeBinding::new(
        "4444444444444444444444444444444444444444",
        "f66e03c55703837d6f2aee9959eba482612272f1",
    )
    .expect("changed commit");
    cases.push(("commit", changed));

    let mut changed = baseline_input.clone();
    changed.code = CodeBinding::new(
        "34402344c7d2c9238dc171af52ac4db77eb6b462",
        "5555555555555555555555555555555555555555",
    )
    .expect("changed tree");
    cases.push(("tree", changed));

    let mut changed = baseline_input.clone();
    changed.runtime = RuntimeBinding::new(
        ContentHash::digest(b"changed-image"),
        ContentHash::digest(b"environment"),
    );
    cases.push(("runtime image", changed));

    let mut changed = baseline_input.clone();
    changed.runtime = RuntimeBinding::new(
        ContentHash::digest(b"image"),
        ContentHash::digest(b"changed-environment"),
    );
    cases.push(("environment", changed));

    let mut changed = baseline_input.clone();
    changed.implementations = vec![
        FormalImplementationBinding::new("risk", ContentHash::digest(b"pricing-v1"))
            .expect("changed implementation role"),
    ];
    cases.push(("implementation role", changed));

    let mut changed = baseline_input.clone();
    changed.implementations = vec![
        FormalImplementationBinding::new("pricing", ContentHash::digest(b"pricing-v2"))
            .expect("changed implementation"),
    ];
    cases.push(("implementation hash", changed));

    let mut changed = baseline_input.clone();
    changed.parameters_hash = ContentHash::digest(b"changed-parameters");
    cases.push(("parameters", changed));

    let mut changed = baseline_input.clone();
    changed.seed = None;
    cases.push(("optional seed", changed));

    let mut changed = baseline_input.clone();
    changed.result_hash = ContentHash::digest(b"changed-result");
    cases.push(("result", changed));

    for (label, candidate) in cases {
        assert_ne!(
            baseline,
            evidence_identity(candidate),
            "{label} drift must change the formal output identity",
        );
    }
}

#[test]
fn formal_identity_is_order_independent_and_duplicate_roles_fail_closed() {
    let canonical = complete_evidence_input();
    let mut reversed = canonical.clone();
    reversed.consumed_inputs.reverse();
    assert_eq!(
        evidence_identity(canonical.clone()),
        evidence_identity(reversed)
    );

    let mut duplicate_input = canonical.clone();
    duplicate_input
        .consumed_inputs
        .push(canonical.consumed_inputs[0].clone());
    assert!(FormalOutputEvidence::new(duplicate_input).is_err());

    let mut duplicate_implementation = canonical;
    duplicate_implementation.implementations.push(
        FormalImplementationBinding::new("pricing", ContentHash::digest(b"other"))
            .expect("duplicate implementation fixture"),
    );
    assert!(FormalOutputEvidence::new(duplicate_implementation).is_err());
}

#[test]
fn graph_comparison_exposes_each_of_the_thirteen_independent_dimensions() {
    let graph = comparison_graph(1);
    let base_input = comparison_input(&graph);
    let base = comparison_identity(&graph, base_input.clone(), b"subject", '0');
    let mut cases = Vec::new();

    let mut changed = base_input.clone();
    changed.data_snapshot_hash = ContentHash::digest(b"changed-data");
    cases.push((
        ComparisonDimension::Data,
        comparison_identity(&graph, changed, b"subject", '0'),
    ));

    let mut changed = base_input.clone();
    changed.universe_snapshot_hash = ContentHash::digest(b"changed-universe");
    cases.push((
        ComparisonDimension::Universe,
        comparison_identity(&graph, changed, b"subject", '0'),
    ));

    let changed_graph = comparison_graph(2);
    cases.push((
        ComparisonDimension::Graph,
        comparison_identity(
            &changed_graph,
            comparison_input(&changed_graph),
            b"subject",
            '0',
        ),
    ));

    let mut changed = base_input.clone();
    changed.parameters_hash = ContentHash::digest(b"changed-parameters");
    cases.push((
        ComparisonDimension::Parameters,
        comparison_identity(&graph, changed, b"subject", '0'),
    ));

    let mut changed = base_input.clone();
    changed.runtime_image_digest = ContentHash::digest(b"changed-runtime");
    cases.push((
        ComparisonDimension::Runtime,
        comparison_identity(&graph, changed, b"subject", '0'),
    ));

    let mut changed = base_input.clone();
    changed.environment_digest = ContentHash::digest(b"changed-environment");
    cases.push((
        ComparisonDimension::Environment,
        comparison_identity(&graph, changed, b"subject", '0'),
    ));

    let mut changed = base_input.clone();
    changed.seed += 1;
    cases.push((
        ComparisonDimension::Seed,
        comparison_identity(&graph, changed, b"subject", '0'),
    ));

    let mut changed = base_input.clone();
    changed.rule_pack_bindings[0].version = Version::new(2).expect("version");
    cases.push((
        ComparisonDimension::RulePack,
        comparison_identity(&graph, changed, b"subject", '0'),
    ));

    let mut changed = base_input.clone();
    changed.node_implementations[0].implementation_digest =
        ContentHash::digest(b"changed-implementation");
    cases.push((
        ComparisonDimension::Implementation,
        comparison_identity(&graph, changed, b"subject", '0'),
    ));

    let mut changed = base_input.clone();
    changed.external_inputs = vec![
        ExecutionExternalInput::new(
            "market",
            graph.external_inputs()[0].value_type().clone(),
            b"changed-external-input".to_vec(),
        )
        .expect("external input"),
    ];
    cases.push((
        ComparisonDimension::ExternalInput,
        comparison_identity(&graph, changed, b"subject", '0'),
    ));

    cases.push((ComparisonDimension::Result, base.clone()));
    cases.push((
        ComparisonDimension::Subject,
        comparison_identity(&graph, base_input.clone(), b"changed-subject", '0'),
    ));
    cases.push((
        ComparisonDimension::Code,
        comparison_identity(&graph, base_input, b"subject", '1'),
    ));

    for (dimension, candidate) in cases {
        assert_eq!(
            compare_graph_run_dimensions(
                &base,
                &candidate,
                dimension == ComparisonDimension::Result,
            ),
            vec![dimension],
            "dimension {dimension:?} must remain independently visible",
        );
    }
}

fn comparison_graph(version: u64) -> ResearchGraph {
    let value_type = TypedValue::new(
        "ficant.r7b.fixture",
        Version::new(1).expect("version"),
        ContentHash::digest(b"fixture-schema"),
    )
    .expect("value type");
    let contract = ResearchNodeContract::new(ResearchNodeContractInput {
        contract_id: "r7b.comparison".to_owned(),
        contract_version: Version::new(1).expect("version"),
        input_types: vec![PortType::new("market", value_type.clone()).expect("input")],
        output_types: vec![PortType::new("result", value_type.clone()).expect("output")],
        state_schema: ContentHash::digest(b"state"),
        parameter_schema: ContentHash::digest(b"node-parameters"),
        determinism_class: DeterminismClass::Deterministic,
        permissions: NodePermissions {
            network: false,
            database: false,
            filesystem: FilesystemPermission::None,
        },
        resource_limits: ResourceLimits::new(1, 64, 10).expect("limits"),
        required_invariants: vec!["formal-evidence".to_owned()],
    })
    .expect("contract");
    ResearchGraph::new_with_external_inputs(
        ResearchGraphInput {
            graph_id: id("01ARZ3NDEKTSV4RRFFQ69G5FB0"),
            version: Version::new(version).expect("version"),
            owner: owner(),
            nodes: vec![ResearchNode::new(
                id("01ARZ3NDEKTSV4RRFFQ69G5FB1"),
                contract,
                ContentHash::digest(b"node-parameters"),
            )],
            edges: vec![],
        },
        vec![GraphExternalInput::new("market", value_type).expect("external input")],
        vec![
            GraphExternalInputBinding::new("market", id("01ARZ3NDEKTSV4RRFFQ69G5FB1"), "market")
                .expect("input binding"),
        ],
    )
    .expect("graph")
}

fn comparison_input(graph: &ResearchGraph) -> ReproducibilityIdentityInput {
    ReproducibilityIdentityInput {
        external_inputs: vec![
            ExecutionExternalInput::new(
                "market",
                graph.external_inputs()[0].value_type().clone(),
                b"external-input".to_vec(),
            )
            .expect("external input"),
        ],
        data_snapshot_hash: ContentHash::digest(b"data"),
        universe_snapshot_hash: ContentHash::digest(b"universe"),
        parameters_hash: ContentHash::digest(b"parameters"),
        runtime_image_digest: ContentHash::digest(b"runtime"),
        environment_digest: ContentHash::digest(b"environment"),
        seed: 42,
        rule_pack_bindings: vec![RulePackBinding {
            rule_pack_id: "rates".to_owned(),
            version: Version::new(1).expect("version"),
            content_hash: ContentHash::digest(b"rule-pack"),
        }],
        node_implementations: vec![NodeImplementation {
            node_id: graph.nodes()[0].node_id().clone(),
            implementation_digest: ContentHash::digest(b"implementation"),
        }],
    }
}

fn comparison_identity(
    graph: &ResearchGraph,
    input: ReproducibilityIdentityInput,
    subject_content: &[u8],
    code_digit: char,
) -> ReproducibilityIdentity {
    let subject = FormalInputBinding::new(FormalInputBindingInput {
        role: "subject".to_owned(),
        kind: FormalInputKind::Subject,
        owner: owner(),
        reference: FormalInputReference::Object(
            LineageRef::new(
                id("01ARZ3NDEKTSV4RRFFQ69G5FB2"),
                Some(Version::new(1).expect("version")),
                Some(ContentHash::digest(subject_content)),
            )
            .expect("subject ref"),
        ),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .expect("subject");
    let sha = code_digit.to_string().repeat(40);
    ReproducibilityIdentity::new_formal(
        graph,
        input,
        subject,
        CodeBinding::new(sha, "2222222222222222222222222222222222222222").expect("code"),
    )
    .expect("identity")
}
