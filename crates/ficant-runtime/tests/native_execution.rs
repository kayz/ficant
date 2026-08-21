use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::{ContentHash, LineageRef, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    DeterminismClass, FilesystemPermission, GraphExternalInput, GraphExternalInputBinding,
    NodePermissions, PortType, ResearchEdge, ResearchGraph, ResearchGraphInput, ResearchNode,
    ResearchNodeContract, ResearchNodeContractInput, ResourceLimits, TypedValue,
};
use ficant_runtime::{
    ComparisonDimension, ExecutionExternalInput, ExecutionIdentity, ExecutionIdentityInput,
    ExecutionInstanceIdentity, FormalInputBinding, FormalInputBindingInput, FormalInputKind,
    FormalInputReference, NativeNode, NativeNodeRequest, NativePortValue, NodeImplementation,
    ReproducibilityIdentity, ReproducibilityIdentityInput, RulePackBinding, RuntimeError,
    canonical_output_bytes, compare_experiments, decode_canonical_output_bytes,
    execute_native_graph, execute_native_graph_with_external_inputs, execute_native_node,
    verify_native_replay,
};

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";
fn id(c: char) -> Ulid {
    Ulid::new(format!("{PREFIX}{c}")).unwrap()
}

#[test]
fn portfolio_formal_input_kinds_have_frozen_codes_and_preserve_existing_identity() {
    let cases = [
        (FormalInputKind::Portfolio, 16),
        (FormalInputKind::Book, 17),
        (FormalInputKind::PortfolioGroup, 18),
        (FormalInputKind::Benchmark, 19),
        (FormalInputKind::PortfolioMetricConvention, 20),
        (FormalInputKind::Fact, 21),
    ];
    for (kind, expected_code) in cases {
        let binding = exact_formal_input(kind, "portfolio-authority", b"portfolio-authority");
        assert_eq!(
            canonical_field(&binding.canonical_bytes(), 3),
            [expected_code]
        );
    }

    let unversioned_fact = FormalInputBinding::new(FormalInputBindingInput {
        role: "valuation".to_owned(),
        kind: FormalInputKind::Fact,
        owner: OwnerRef::new(id('T'), id('W')),
        reference: FormalInputReference::Object(
            LineageRef::new(id('P'), None, Some(ContentHash::digest(b"valuation"))).unwrap(),
        ),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .unwrap_err();
    assert_eq!(unversioned_fact, DomainErrorCode::BrokenLineage);

    let existing = exact_formal_input(FormalInputKind::Subject, "existing-subject", b"subject");
    assert_eq!(
        ContentHash::digest(&existing.canonical_bytes()).as_bytes(),
        &[
            39, 208, 246, 112, 154, 109, 255, 161, 1, 221, 241, 116, 51, 213, 56, 34, 217, 122, 56,
            47, 222, 225, 59, 253, 142, 224, 38, 181, 95, 85, 84, 250,
        ]
    );
}

fn exact_formal_input(kind: FormalInputKind, role: &str, payload: &[u8]) -> FormalInputBinding {
    FormalInputBinding::new(FormalInputBindingInput {
        role: role.to_owned(),
        kind,
        owner: OwnerRef::new(id('T'), id('W')),
        reference: FormalInputReference::Object(
            LineageRef::new(
                id('P'),
                Some(Version::new(1).unwrap()),
                Some(ContentHash::digest(payload)),
            )
            .unwrap(),
        ),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .unwrap()
}

fn canonical_field(bytes: &[u8], wanted_tag: u16) -> &[u8] {
    let mut offset = b"FICANT-EVIDENCE\0".len();
    while offset < bytes.len() {
        let tag_end = offset + 2;
        let length_end = tag_end + 8;
        let tag = u16::from_be_bytes(bytes[offset..tag_end].try_into().unwrap());
        let length = usize::try_from(u64::from_be_bytes(
            bytes[tag_end..length_end].try_into().unwrap(),
        ))
        .unwrap();
        let value_end = length_end + length;
        if tag == wanted_tag {
            return &bytes[length_end..value_end];
        }
        offset = value_end;
    }
    panic!("canonical field {wanted_tag} is missing")
}

fn value_type() -> TypedValue {
    TypedValue::new(
        "ficant.test.value",
        Version::new(1).unwrap(),
        ContentHash::digest(b"schema"),
    )
    .unwrap()
}

fn contract(name: &str, input: bool, output: &str) -> ResearchNodeContract {
    let value = value_type();
    ResearchNodeContract::new(ResearchNodeContractInput {
        contract_id: name.to_owned(),
        contract_version: Version::new(1).unwrap(),
        input_types: if input {
            vec![PortType::new("input", value.clone()).unwrap()]
        } else {
            vec![]
        },
        output_types: vec![PortType::new(output, value).unwrap()],
        state_schema: ContentHash::digest(b"state"),
        parameter_schema: ContentHash::digest(b"params"),
        determinism_class: DeterminismClass::Seeded,
        permissions: NodePermissions {
            network: false,
            database: false,
            filesystem: FilesystemPermission::None,
        },
        resource_limits: ResourceLimits::new(1, 64, 10).unwrap(),
        required_invariants: vec!["deterministic_with_same_seed".to_owned()],
    })
    .unwrap()
}

fn graph() -> ResearchGraph {
    graph_version(1)
}

fn graph_version(version: u64) -> ResearchGraph {
    let source = ResearchNode::new(
        id('A'),
        contract("ficant.test.source", false, "output"),
        ContentHash::digest(b"p1"),
    );
    let sink = ResearchNode::new(
        id('B'),
        contract("ficant.test.sink", true, "result"),
        ContentHash::digest(b"p2"),
    );
    ResearchGraph::new(ResearchGraphInput {
        graph_id: id('G'),
        version: Version::new(version).unwrap(),
        owner: OwnerRef::new(id('T'), id('W')),
        nodes: vec![sink, source],
        edges: vec![ResearchEdge::new(id('A'), "output", id('B'), "input").unwrap()],
    })
    .unwrap()
}

fn external_graph() -> ResearchGraph {
    let target = ResearchNode::new(
        id('A'),
        contract("ficant.test.external", true, "result"),
        ContentHash::digest(b"p1"),
    );
    ResearchGraph::new_with_external_inputs(
        ResearchGraphInput {
            graph_id: id('G'),
            version: Version::new(1).unwrap(),
            owner: OwnerRef::new(id('T'), id('W')),
            nodes: vec![target],
            edges: vec![],
        },
        vec![GraphExternalInput::new("market-data", value_type()).unwrap()],
        vec![GraphExternalInputBinding::new("market-data", id('A'), "input").unwrap()],
    )
    .unwrap()
}

struct TestNode {
    node_id: Ulid,
    implementation: ContentHash,
    output: &'static str,
    wrong_type: bool,
}
impl NativeNode for TestNode {
    fn node_id(&self) -> &Ulid {
        &self.node_id
    }
    fn implementation_digest(&self) -> &ContentHash {
        &self.implementation
    }
    fn execute(
        &self,
        request: &NativeNodeRequest<'_>,
    ) -> Result<Vec<NativePortValue>, RuntimeError> {
        let mut payload = request.identity().seed().to_be_bytes().to_vec();
        for input in request.inputs() {
            payload.extend_from_slice(input.content_hash().as_bytes());
        }
        let value = if self.wrong_type {
            TypedValue::new(
                "ficant.test.wrong",
                Version::new(1).unwrap(),
                ContentHash::digest(b"wrong"),
            )
            .unwrap()
        } else {
            value_type()
        };
        Ok(vec![NativePortValue::new(self.output, value, payload)?])
    }
}

fn nodes() -> (TestNode, TestNode) {
    (
        TestNode {
            node_id: id('A'),
            implementation: ContentHash::digest(b"impl-a"),
            output: "output",
            wrong_type: false,
        },
        TestNode {
            node_id: id('B'),
            implementation: ContentHash::digest(b"impl-b"),
            output: "result",
            wrong_type: false,
        },
    )
}

fn identity(graph: &ResearchGraph, seed: u64) -> ExecutionIdentity {
    ExecutionIdentity::new(
        graph,
        ExecutionIdentityInput {
            run_id: id('R'),
            data_snapshot_hash: ContentHash::digest(b"data"),
            universe_snapshot_hash: ContentHash::digest(b"universe"),
            parameters_hash: ContentHash::digest(b"parameters"),
            runtime_image_digest: ContentHash::digest(b"image"),
            environment_digest: ContentHash::digest(b"environment"),
            seed,
            node_implementations: vec![
                NodeImplementation {
                    node_id: id('B'),
                    implementation_digest: ContentHash::digest(b"impl-b"),
                },
                NodeImplementation {
                    node_id: id('A'),
                    implementation_digest: ContentHash::digest(b"impl-a"),
                },
            ],
        },
    )
    .unwrap()
}

fn reproducibility_input(seed: u64) -> ReproducibilityIdentityInput {
    ReproducibilityIdentityInput {
        external_inputs: vec![],
        data_snapshot_hash: ContentHash::digest(b"data"),
        universe_snapshot_hash: ContentHash::digest(b"universe"),
        parameters_hash: ContentHash::digest(b"parameters"),
        runtime_image_digest: ContentHash::digest(b"image"),
        environment_digest: ContentHash::digest(b"environment"),
        seed,
        rule_pack_bindings: vec![RulePackBinding {
            rule_pack_id: "pricing-rules".to_owned(),
            version: Version::new(1).unwrap(),
            content_hash: ContentHash::digest(b"rules"),
        }],
        node_implementations: vec![
            NodeImplementation {
                node_id: id('B'),
                implementation_digest: ContentHash::digest(b"impl-b"),
            },
            NodeImplementation {
                node_id: id('A'),
                implementation_digest: ContentHash::digest(b"impl-a"),
            },
        ],
    }
}

fn instance(
    graph: &ResearchGraph,
    run_id: Ulid,
    input: ReproducibilityIdentityInput,
) -> ExecutionInstanceIdentity {
    ExecutionInstanceIdentity::from_reproducibility(
        run_id,
        ReproducibilityIdentity::new(graph, input).unwrap(),
    )
}

#[test]
fn same_frozen_identity_replays_to_exact_node_lineage_and_result() {
    let graph = graph();
    let identity = identity(&graph, 7);
    let (source, sink) = nodes();
    let first = execute_native_graph(&graph, &identity, &[&sink, &source]).unwrap();
    let second = execute_native_graph(&graph, &identity, &[&source, &sink]).unwrap();
    verify_native_replay(&first, &second).unwrap();
    assert_eq!(first.artifacts().len(), 2);
    assert!(first.artifacts()[0].input_artifacts().is_empty());
    assert_eq!(
        first.artifacts()[1].input_artifacts(),
        [first.artifacts()[0].artifact_digest().clone()]
    );
    assert!(compare_experiments(&first, &second).identical());
}

#[test]
fn comparison_identifies_seed_and_result_drift() {
    let graph = graph();
    let (source, sink) = nodes();
    let left = execute_native_graph(&graph, &identity(&graph, 7), &[&source, &sink]).unwrap();
    let right = execute_native_graph(&graph, &identity(&graph, 8), &[&source, &sink]).unwrap();
    assert_eq!(
        compare_experiments(&left, &right).differences(),
        [ComparisonDimension::Seed, ComparisonDimension::Result]
    );
    assert_eq!(
        verify_native_replay(&left, &right),
        Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch))
    );
}

#[test]
fn missing_implementation_and_output_type_drift_fail_closed() {
    let graph = graph();
    let identity = identity(&graph, 7);
    let (source, mut sink) = nodes();
    assert_eq!(
        execute_native_graph(&graph, &identity, &[&source]),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
    sink.wrong_type = true;
    assert_eq!(
        execute_native_graph(&graph, &identity, &[&source, &sink]),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
}

#[test]
fn different_runs_with_the_same_frozen_inputs_produce_the_same_content() {
    let graph = graph();
    let left_identity = instance(&graph, id('R'), reproducibility_input(7));
    let right_identity = instance(&graph, id('S'), reproducibility_input(7));
    let (source, sink) = nodes();
    let left = execute_native_graph(&graph, &left_identity, &[&source, &sink]).unwrap();
    let right = execute_native_graph(&graph, &right_identity, &[&sink, &source]).unwrap();

    assert_ne!(left.identity().digest(), right.identity().digest());
    assert_eq!(
        left.identity().reproducibility_digest(),
        right.identity().reproducibility_digest()
    );
    assert_eq!(left.artifacts(), right.artifacts());
    assert_eq!(left.result_digest(), right.result_digest());
    verify_native_replay(&left, &right).unwrap();
    assert!(compare_experiments(&left, &right).identical());
}

#[test]
fn external_inputs_fail_closed_on_missing_extra_type_or_hash_drift() {
    let payload = b"verified-market-data";
    let graph = external_graph();
    let valid = ExecutionExternalInput::new("market-data", value_type(), payload.to_vec()).unwrap();
    let mut input = reproducibility_input(7);
    input.external_inputs = vec![valid.clone()];
    input.node_implementations = vec![NodeImplementation {
        node_id: id('A'),
        implementation_digest: ContentHash::digest(b"impl-a"),
    }];
    let identity = instance(&graph, id('R'), input);
    let executor = TestNode {
        node_id: id('A'),
        implementation: ContentHash::digest(b"impl-a"),
        output: "result",
        wrong_type: false,
    };
    execute_native_graph_with_external_inputs(
        &graph,
        &identity,
        &[&executor],
        std::slice::from_ref(&valid),
    )
    .unwrap();

    assert_eq!(
        execute_native_graph_with_external_inputs(&graph, &identity, &[&executor], &[]),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
    let extra = ExecutionExternalInput::new("extra", value_type(), b"extra".to_vec()).unwrap();
    assert_eq!(
        execute_native_graph_with_external_inputs(
            &graph,
            &identity,
            &[&executor],
            &[valid.clone(), extra]
        ),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
    let wrong_type = TypedValue::new(
        "ficant.test.wrong",
        Version::new(1).unwrap(),
        ContentHash::digest(b"wrong"),
    )
    .unwrap();
    let wrong_type =
        ExecutionExternalInput::new("market-data", wrong_type, payload.to_vec()).unwrap();
    assert_eq!(
        execute_native_graph_with_external_inputs(&graph, &identity, &[&executor], &[wrong_type]),
        Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch))
    );
    let wrong_hash =
        ExecutionExternalInput::new("market-data", value_type(), b"changed".to_vec()).unwrap();
    assert_eq!(
        execute_native_graph_with_external_inputs(&graph, &identity, &[&executor], &[wrong_hash]),
        Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch))
    );
}

#[test]
fn single_node_execution_retains_payload_and_has_a_deterministic_envelope() {
    let graph = graph();
    let identity = instance(&graph, id('R'), reproducibility_input(7));
    let (source, _) = nodes();
    let node = graph
        .nodes()
        .iter()
        .find(|node| node.node_id() == source.node_id())
        .unwrap();
    let first =
        execute_native_node(node, identity.reproducibility(), &source, vec![], vec![]).unwrap();
    let second =
        execute_native_node(node, identity.reproducibility(), &source, vec![], vec![]).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.outputs()[0].payload(), 7_u64.to_be_bytes());
    assert!(!first.output_envelope().is_empty());
    assert_eq!(
        ContentHash::digest(first.output_envelope()),
        *first.artifact().output_envelope_hash()
    );
    assert_eq!(
        first.artifact().output_hashes(),
        [first.outputs()[0].content_hash().clone()]
    );
}

#[test]
fn every_frozen_identity_dimension_is_visible_to_comparison() {
    let graph = graph();
    let (source, sink) = nodes();
    let base_identity = instance(&graph, id('R'), reproducibility_input(7));
    let base = execute_native_graph(&graph, &base_identity, &[&source, &sink]).unwrap();

    let mut cases = Vec::new();
    let mut changed = reproducibility_input(7);
    changed.data_snapshot_hash = ContentHash::digest(b"data-2");
    cases.push((ComparisonDimension::DataSnapshot, graph.clone(), changed));
    let mut changed = reproducibility_input(7);
    changed.universe_snapshot_hash = ContentHash::digest(b"universe-2");
    cases.push((
        ComparisonDimension::UniverseSnapshot,
        graph.clone(),
        changed,
    ));
    let mut changed = reproducibility_input(7);
    changed.parameters_hash = ContentHash::digest(b"parameters-2");
    cases.push((ComparisonDimension::Parameters, graph.clone(), changed));
    let mut changed = reproducibility_input(7);
    changed.runtime_image_digest = ContentHash::digest(b"image-2");
    cases.push((ComparisonDimension::RuntimeImage, graph.clone(), changed));
    let mut changed = reproducibility_input(7);
    changed.environment_digest = ContentHash::digest(b"environment-2");
    cases.push((ComparisonDimension::Environment, graph.clone(), changed));
    let changed = reproducibility_input(8);
    cases.push((ComparisonDimension::Seed, graph.clone(), changed));
    let mut changed = reproducibility_input(7);
    changed.rule_pack_bindings[0].version = Version::new(2).unwrap();
    cases.push((ComparisonDimension::RulePack, graph.clone(), changed));

    for (dimension, candidate_graph, changed) in cases {
        let candidate_identity = instance(&candidate_graph, id('S'), changed);
        let candidate =
            execute_native_graph(&candidate_graph, &candidate_identity, &[&source, &sink]).unwrap();
        assert!(
            compare_experiments(&base, &candidate)
                .differences()
                .contains(&dimension),
            "missing comparison dimension {dimension:?}"
        );
    }

    let changed_graph = graph_version(2);
    let changed_identity = instance(&changed_graph, id('S'), reproducibility_input(7));
    let changed =
        execute_native_graph(&changed_graph, &changed_identity, &[&source, &sink]).unwrap();
    assert!(
        compare_experiments(&base, &changed)
            .differences()
            .contains(&ComparisonDimension::Graph)
    );

    let changed_source = TestNode {
        node_id: id('A'),
        implementation: ContentHash::digest(b"impl-a-2"),
        output: "output",
        wrong_type: false,
    };
    let mut changed_input = reproducibility_input(7);
    changed_input
        .node_implementations
        .iter_mut()
        .find(|binding| binding.node_id == id('A'))
        .unwrap()
        .implementation_digest = ContentHash::digest(b"impl-a-2");
    let changed_identity = instance(&graph, id('S'), changed_input);
    let changed =
        execute_native_graph(&graph, &changed_identity, &[&changed_source, &sink]).unwrap();
    assert!(
        compare_experiments(&base, &changed)
            .differences()
            .contains(&ComparisonDimension::Implementation)
    );
}

#[test]
fn external_input_content_is_a_comparable_frozen_dimension() {
    let external_graph = external_graph();
    let first_external =
        ExecutionExternalInput::new("market-data", value_type(), b"first".to_vec()).unwrap();
    let second_external =
        ExecutionExternalInput::new("market-data", value_type(), b"second".to_vec()).unwrap();
    let mut first_input = reproducibility_input(7);
    first_input.external_inputs = vec![first_external.clone()];
    first_input.node_implementations = vec![NodeImplementation {
        node_id: id('A'),
        implementation_digest: ContentHash::digest(b"impl-a"),
    }];
    let mut second_input = first_input.clone();
    second_input.external_inputs = vec![second_external.clone()];
    let external_executor = TestNode {
        node_id: id('A'),
        implementation: ContentHash::digest(b"impl-a"),
        output: "result",
        wrong_type: false,
    };
    let first_identity = instance(&external_graph, id('R'), first_input);
    let second_identity = instance(&external_graph, id('S'), second_input);
    let first = execute_native_graph_with_external_inputs(
        &external_graph,
        &first_identity,
        &[&external_executor],
        &[first_external],
    )
    .unwrap();
    let second = execute_native_graph_with_external_inputs(
        &external_graph,
        &second_identity,
        &[&external_executor],
        &[second_external],
    )
    .unwrap();
    assert_eq!(
        compare_experiments(&first, &second).differences(),
        [
            ComparisonDimension::ExternalInput,
            ComparisonDimension::Result
        ]
    );
}

fn envelope_value(port_name: &str, payload: &[u8]) -> NativePortValue {
    NativePortValue::new(port_name, value_type(), payload.to_vec()).unwrap()
}

fn first_entry_offsets(bytes: &[u8]) -> (usize, usize, usize, usize, usize) {
    const MAGIC_LEN: usize = b"ficant/native-node-output-envelope/v1".len();
    let read_length = |offset: usize| {
        let raw: [u8; 8] = bytes[offset..offset + 8].try_into().unwrap();
        usize::try_from(u64::from_be_bytes(raw)).unwrap()
    };
    let port_length_offset = MAGIC_LEN + 8;
    let port_offset = port_length_offset + 8;
    let type_length_offset = port_offset + read_length(port_length_offset);
    let type_offset = type_length_offset + 8;
    let version_offset = type_offset + read_length(type_length_offset);
    let schema_offset = version_offset + 8;
    let content_offset = schema_offset + 32;
    let payload_length_offset = content_offset + 32;
    let payload_offset = payload_length_offset + 8;
    (
        port_offset,
        version_offset,
        schema_offset,
        content_offset,
        payload_offset,
    )
}

#[test]
fn canonical_output_decoder_roundtrips_ordered_multi_port_payloads() {
    let outputs = vec![
        envelope_value("alpha", b"first-payload"),
        envelope_value("beta", b"second-payload"),
    ];
    let bytes = canonical_output_bytes(&outputs);
    let hash = ContentHash::digest(&bytes);

    assert_eq!(
        decode_canonical_output_bytes(&bytes, Some(&hash)).unwrap(),
        outputs
    );
    assert_eq!(
        decode_canonical_output_bytes(&bytes, None).unwrap(),
        outputs
    );
}

#[test]
fn canonical_output_decoder_rejects_format_length_utf8_type_schema_and_hash_drift() {
    let output = envelope_value("alpha", b"payload");
    let bytes = canonical_output_bytes(std::slice::from_ref(&output));
    let hash = ContentHash::digest(&bytes);
    let (port, version, schema, content, payload) = first_entry_offsets(&bytes);

    let mut wrong_magic = bytes.clone();
    wrong_magic[0] ^= 1;
    assert_eq!(
        decode_canonical_output_bytes(&wrong_magic, None),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );

    let mut invalid_utf8 = bytes.clone();
    invalid_utf8[port] = 0xff;
    assert_eq!(
        decode_canonical_output_bytes(&invalid_utf8, None),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );

    let mut zero_type_version = bytes.clone();
    zero_type_version[version..version + 8].copy_from_slice(&0_u64.to_be_bytes());
    assert_eq!(
        decode_canonical_output_bytes(&zero_type_version, None),
        Err(RuntimeError::Domain(DomainErrorCode::VersionConflict))
    );

    let mut impossible_length = bytes.clone();
    impossible_length[content + 32..content + 40].copy_from_slice(&u64::MAX.to_be_bytes());
    assert_eq!(
        decode_canonical_output_bytes(&impossible_length, None),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );

    let mut content_hash_drift = bytes.clone();
    content_hash_drift[content] ^= 1;
    assert_eq!(
        decode_canonical_output_bytes(&content_hash_drift, None),
        Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch))
    );

    let mut payload_drift = bytes.clone();
    payload_drift[payload] ^= 1;
    assert_eq!(
        decode_canonical_output_bytes(&payload_drift, None),
        Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch))
    );

    let mut schema_drift = bytes.clone();
    schema_drift[schema] ^= 1;
    assert_eq!(
        decode_canonical_output_bytes(&schema_drift, Some(&hash)),
        Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch))
    );

    assert_eq!(
        decode_canonical_output_bytes(&bytes[..bytes.len() - 1], None),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(
        decode_canonical_output_bytes(&trailing, None),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
}

#[test]
fn canonical_output_decoder_rejects_wrong_total_hash_duplicate_and_noncanonical_order() {
    let alpha = envelope_value("alpha", b"first");
    let beta = envelope_value("beta", b"second");
    let bytes = canonical_output_bytes(std::slice::from_ref(&alpha));
    assert_eq!(
        decode_canonical_output_bytes(&bytes, Some(&ContentHash::digest(b"wrong"))),
        Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch))
    );
    assert_eq!(
        decode_canonical_output_bytes(&canonical_output_bytes(&[]), None),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );

    let duplicate = canonical_output_bytes(&[alpha.clone(), alpha]);
    assert_eq!(
        decode_canonical_output_bytes(&duplicate, None),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );

    let reversed = canonical_output_bytes(&[beta, envelope_value("alpha", b"first")]);
    assert_eq!(
        decode_canonical_output_bytes(&reversed, None),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
}
