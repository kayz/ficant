use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    DeterminismClass, FilesystemPermission, NodePermissions, PortType, ResearchEdge, ResearchGraph,
    ResearchGraphInput, ResearchNode, ResearchNodeContract, ResearchNodeContractInput,
    ResourceLimits, TypedValue,
};
use ficant_runtime::{
    ComparisonDimension, ExecutionIdentity, ExecutionIdentityInput, NativeNode, NativeNodeRequest,
    NativePortValue, NodeImplementation, RuntimeError, compare_experiments, execute_native_graph,
    verify_native_replay,
};

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";
fn id(c: char) -> Ulid {
    Ulid::new(format!("{PREFIX}{c}")).unwrap()
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
        version: Version::new(1).unwrap(),
        owner: OwnerRef::new(id('T'), id('W')),
        nodes: vec![sink, source],
        edges: vec![ResearchEdge::new(id('A'), "output", id('B'), "input").unwrap()],
    })
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
