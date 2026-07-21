use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    DeterminismClass, FilesystemPermission, NodePermissions, PortType, ResearchEdge, ResearchGraph,
    ResearchGraphInput, ResearchNode, ResearchNodeContract, ResearchNodeContractInput,
    ResourceLimits, TypedValue,
};

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{ULID_PREFIX}{suffix}")).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('H'))
}

fn value_type(type_id: &str, schema: &[u8]) -> TypedValue {
    TypedValue::new(
        type_id,
        Version::new(1).unwrap(),
        ContentHash::digest(schema),
    )
    .unwrap()
}

fn port(name: &str, value_type: &TypedValue) -> PortType {
    PortType::new(name, value_type.clone()).unwrap()
}

fn limits() -> ResourceLimits {
    ResourceLimits::new(1, 256, 60).unwrap()
}

fn contract(
    contract_id: &str,
    inputs: Vec<PortType>,
    outputs: Vec<PortType>,
) -> ResearchNodeContract {
    ResearchNodeContract::new(ResearchNodeContractInput {
        contract_id: contract_id.to_owned(),
        contract_version: Version::new(1).unwrap(),
        input_types: inputs,
        output_types: outputs,
        state_schema: ContentHash::digest(b"stateless"),
        parameter_schema: ContentHash::digest(b"parameters"),
        determinism_class: DeterminismClass::Seeded,
        permissions: NodePermissions {
            network: false,
            database: false,
            filesystem: FilesystemPermission::TemporaryOnly,
        },
        resource_limits: limits(),
        required_invariants: vec![
            "deterministic_with_same_seed".to_owned(),
            "schema_exact".to_owned(),
        ],
    })
    .unwrap()
}

fn node(suffix: char, contract: ResearchNodeContract) -> ResearchNode {
    ResearchNode::new(
        id(suffix),
        contract,
        ContentHash::digest(format!("parameters-{suffix}").as_bytes()),
    )
}

fn three_node_graph(reverse: bool) -> ResearchGraph {
    let raw = value_type("ficant.market.raw", b"raw-v1");
    let clean = value_type("ficant.market.clean", b"clean-v1");
    let result = value_type("ficant.risk.result", b"risk-v1");
    let source = node(
        'A',
        contract("ficant.data.source", vec![], vec![port("raw", &raw)]),
    );
    let cleaner = node(
        'B',
        contract(
            "ficant.data.cleaner",
            vec![port("raw", &raw)],
            vec![port("clean", &clean)],
        ),
    );
    let risk = node(
        'C',
        contract(
            "ficant.risk.calculate",
            vec![port("clean", &clean)],
            vec![port("result", &result)],
        ),
    );
    let first = ResearchEdge::new(id('A'), "raw", id('B'), "raw").unwrap();
    let second = ResearchEdge::new(id('B'), "clean", id('C'), "clean").unwrap();
    let (nodes, edges) = if reverse {
        (vec![risk, cleaner, source], vec![second, first])
    } else {
        (vec![source, cleaner, risk], vec![first, second])
    };
    ResearchGraph::new(ResearchGraphInput {
        graph_id: id('G'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        nodes,
        edges,
    })
    .unwrap()
}

#[test]
fn contract_canonicalizes_declarations_and_binds_every_execution_constraint() {
    let left = value_type("ficant.left", b"left");
    let right = value_type("ficant.right", b"right");
    let make = |reverse: bool| {
        let (inputs, invariants) = if reverse {
            (
                vec![port("right", &right), port("left", &left)],
                vec!["schema_exact".to_owned(), "no_network".to_owned()],
            )
        } else {
            (
                vec![port("left", &left), port("right", &right)],
                vec!["no_network".to_owned(), "schema_exact".to_owned()],
            )
        };
        ResearchNodeContract::new(ResearchNodeContractInput {
            contract_id: "ficant.factor.spread".to_owned(),
            contract_version: Version::new(2).unwrap(),
            input_types: inputs,
            output_types: vec![port("spread", &right)],
            state_schema: ContentHash::digest(b"state-v1"),
            parameter_schema: ContentHash::digest(b"parameters-v2"),
            determinism_class: DeterminismClass::Deterministic,
            permissions: NodePermissions {
                network: false,
                database: false,
                filesystem: FilesystemPermission::None,
            },
            resource_limits: limits(),
            required_invariants: invariants,
        })
        .unwrap()
    };
    let first = make(false);
    let second = make(true);

    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.input_types()[0].port_name(), "left");
    assert_eq!(first.required_invariants(), ["no_network", "schema_exact"]);
    assert_eq!(first.resource_limits().cpu_cores(), 1);
    assert_eq!(first.resource_limits().memory_mb(), 256);
    assert_eq!(first.resource_limits().timeout_seconds(), 60);
    assert_eq!(
        ResourceLimits::new(0, 256, 60),
        Err(DomainErrorCode::InvalidValue)
    );
    assert_eq!(
        TypedValue::new(
            ".invalid",
            Version::new(1).unwrap(),
            ContentHash::digest(b"invalid")
        ),
        Err(DomainErrorCode::InvalidValue)
    );

    let duplicate_port = ResearchNodeContract::new(ResearchNodeContractInput {
        contract_id: "ficant.invalid.duplicate".to_owned(),
        contract_version: Version::new(1).unwrap(),
        input_types: vec![port("same", &left), port("same", &right)],
        output_types: vec![port("out", &right)],
        state_schema: ContentHash::digest(b"state"),
        parameter_schema: ContentHash::digest(b"parameters"),
        determinism_class: DeterminismClass::Deterministic,
        permissions: NodePermissions {
            network: false,
            database: false,
            filesystem: FilesystemPermission::None,
        },
        resource_limits: limits(),
        required_invariants: vec!["schema_exact".to_owned()],
    });
    assert_eq!(duplicate_port, Err(DomainErrorCode::InvalidValue));
}

#[test]
fn graph_order_and_digest_are_independent_of_caller_collection_order() {
    let first = three_node_graph(false);
    let second = three_node_graph(true);

    assert_eq!(first.digest(), second.digest());
    assert_eq!(
        first
            .topological_order()
            .iter()
            .map(Ulid::as_str)
            .collect::<Vec<_>>(),
        vec![id('A').as_str(), id('B').as_str(), id('C').as_str()]
    );
    assert_eq!(first.nodes(), second.nodes());
    assert_eq!(first.edges(), second.edges());
}

#[test]
fn graph_rejects_missing_duplicate_unbound_and_type_mismatched_edges() {
    let raw = value_type("ficant.market.raw", b"raw-v1");
    let clean = value_type("ficant.market.clean", b"clean-v1");
    let source = node(
        'A',
        contract("ficant.data.source", vec![], vec![port("raw", &raw)]),
    );
    let other_source = node(
        'D',
        contract("ficant.data.other", vec![], vec![port("raw", &raw)]),
    );
    let target = node(
        'B',
        contract(
            "ficant.data.cleaner",
            vec![port("raw", &raw)],
            vec![port("clean", &clean)],
        ),
    );
    let valid = ResearchEdge::new(id('A'), "raw", id('B'), "raw").unwrap();

    let unbound = ResearchGraph::new(ResearchGraphInput {
        graph_id: id('G'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        nodes: vec![source.clone(), target.clone()],
        edges: vec![],
    });
    assert_eq!(unbound, Err(DomainErrorCode::BrokenLineage));

    let missing = ResearchGraph::new(ResearchGraphInput {
        graph_id: id('G'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        nodes: vec![source.clone(), target.clone()],
        edges: vec![ResearchEdge::new(id('Z'), "raw", id('B'), "raw").unwrap()],
    });
    assert_eq!(missing, Err(DomainErrorCode::BrokenLineage));

    let duplicate_binding = ResearchGraph::new(ResearchGraphInput {
        graph_id: id('G'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        nodes: vec![source.clone(), other_source, target.clone()],
        edges: vec![
            valid.clone(),
            ResearchEdge::new(id('D'), "raw", id('B'), "raw").unwrap(),
        ],
    });
    assert_eq!(duplicate_binding, Err(DomainErrorCode::InvalidValue));

    let duplicate_edge = ResearchGraph::new(ResearchGraphInput {
        graph_id: id('G'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        nodes: vec![source.clone(), target.clone()],
        edges: vec![valid.clone(), valid],
    });
    assert_eq!(duplicate_edge, Err(DomainErrorCode::InvalidValue));

    let wrong_target = node(
        'B',
        contract(
            "ficant.data.wrong",
            vec![port("raw", &clean)],
            vec![port("clean", &clean)],
        ),
    );
    let mismatched = ResearchGraph::new(ResearchGraphInput {
        graph_id: id('G'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        nodes: vec![source, wrong_target],
        edges: vec![ResearchEdge::new(id('A'), "raw", id('B'), "raw").unwrap()],
    });
    assert_eq!(mismatched, Err(DomainErrorCode::InvalidValue));
}

#[test]
fn graph_rejects_cycles_and_duplicate_node_identities() {
    let value = value_type("ficant.loop.value", b"loop-v1");
    let loop_contract = |name: &str| {
        contract(
            name,
            vec![port("input", &value)],
            vec![port("output", &value)],
        )
    };
    let left = node('A', loop_contract("ficant.loop.left"));
    let right = node('B', loop_contract("ficant.loop.right"));
    let cyclic = ResearchGraph::new(ResearchGraphInput {
        graph_id: id('G'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        nodes: vec![left.clone(), right.clone()],
        edges: vec![
            ResearchEdge::new(id('A'), "output", id('B'), "input").unwrap(),
            ResearchEdge::new(id('B'), "output", id('A'), "input").unwrap(),
        ],
    });
    assert_eq!(cyclic, Err(DomainErrorCode::InvalidValue));

    let duplicate = ResearchGraph::new(ResearchGraphInput {
        graph_id: id('G'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        nodes: vec![
            left.clone(),
            ResearchNode::new(
                id('A'),
                right.contract().clone(),
                ContentHash::digest(b"other"),
            ),
        ],
        edges: vec![],
    });
    assert_eq!(duplicate, Err(DomainErrorCode::InvalidValue));
}
