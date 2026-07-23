use ficant_domain::ContentAddressed;
use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    DeterminismClass, FilesystemPermission, JournalEventType, NodePermissions, PortType,
    ResearchEdge, ResearchGraph, ResearchGraphInput, ResearchNode, ResearchNodeContract,
    ResearchNodeContractInput, ResourceLimits, RunJournal, RunJournalInput, RunState, TypedValue,
};
use ficant_runtime::{GraphNodeEvent, RuntimeError, replay_graph_execution};

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";
const EVENT_SUFFIXES: [char; 12] = ['B', 'C', 'D', 'E', 'F', 'G', 'H', 'J', 'K', 'M', 'N', 'P'];

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{ULID_PREFIX}{suffix}")).unwrap()
}

fn graph() -> ResearchGraph {
    let value = TypedValue::new(
        "ficant.test.value",
        Version::new(1).unwrap(),
        ContentHash::digest(b"value-v1"),
    )
    .unwrap();
    let contract = |contract_id: &str, input: bool, output_name: &str| {
        ResearchNodeContract::new(ResearchNodeContractInput {
            contract_id: contract_id.to_owned(),
            contract_version: Version::new(1).unwrap(),
            input_types: if input {
                vec![PortType::new("input", value.clone()).unwrap()]
            } else {
                vec![]
            },
            output_types: vec![PortType::new(output_name, value.clone()).unwrap()],
            state_schema: ContentHash::digest(b"stateless"),
            parameter_schema: ContentHash::digest(b"parameters"),
            determinism_class: DeterminismClass::Seeded,
            permissions: NodePermissions {
                network: false,
                database: false,
                filesystem: FilesystemPermission::TemporaryOnly,
            },
            resource_limits: ResourceLimits::new(1, 128, 30).unwrap(),
            required_invariants: vec!["deterministic_with_same_seed".to_owned()],
        })
        .unwrap()
    };
    let source = ResearchNode::new(
        id('A'),
        contract("ficant.test.source", false, "output"),
        ContentHash::digest(b"source-parameters"),
    );
    let sink = ResearchNode::new(
        id('B'),
        contract("ficant.test.sink", true, "result"),
        ContentHash::digest(b"sink-parameters"),
    );
    ResearchGraph::new(ResearchGraphInput {
        graph_id: id('R'),
        version: Version::new(1).unwrap(),
        owner: OwnerRef::new(id('T'), id('W')),
        nodes: vec![sink, source],
        edges: vec![ResearchEdge::new(id('A'), "output", id('B'), "input").unwrap()],
    })
    .unwrap()
}

fn time(sequence: u64) -> MarketTime {
    MarketTime::new(
        format!("2026-07-21T{:02}:00:00Z", sequence + 1)
            .parse()
            .unwrap(),
        "Asia/Shanghai",
        "2026-07-21".parse().unwrap(),
    )
    .unwrap()
}

fn push_event(
    events: &mut Vec<RunJournal>,
    event_type: JournalEventType,
    node_event: Option<GraphNodeEvent>,
) {
    let sequence = u64::try_from(events.len()).unwrap() + 1;
    let (payload_type, payload_schema, payload) = match node_event {
        Some(node_event) => (
            GraphNodeEvent::payload_type().to_owned(),
            GraphNodeEvent::payload_schema().to_owned(),
            node_event.encode(),
        ),
        None => (
            "ficant.research.run-event".to_owned(),
            "ficant.research.run-event.v1".to_owned(),
            vec![u8::try_from(sequence).unwrap()],
        ),
    };
    let input = RunJournalInput {
        journal_event_id: id(EVENT_SUFFIXES[events.len()]),
        run_id: id('Z'),
        sequence,
        event_type,
        occurred_at: time(sequence),
        payload_type,
        payload_schema,
        payload,
        prev_hash: events.last().map(|event| event.content_hash().clone()),
    };
    let hash = input.canonical_hash().unwrap();
    events.push(RunJournal::new(input, &hash).unwrap());
}

fn started(node: char, attempt: u32) -> GraphNodeEvent {
    GraphNodeEvent::started(id(node), attempt).unwrap()
}

fn evidenced(node: char, attempt: u32, hash: &ContentHash) -> GraphNodeEvent {
    GraphNodeEvent::evidenced(id(node), attempt, hash.clone()).unwrap()
}

fn run_prefix() -> Vec<RunJournal> {
    let mut events = Vec::new();
    push_event(&mut events, JournalEventType::RunCreated, None);
    push_event(&mut events, JournalEventType::RunStarted, None);
    events
}

fn checkpoint_node(
    events: &mut Vec<RunJournal>,
    node: char,
    attempt: u32,
    output_hash: &ContentHash,
) {
    push_event(
        events,
        JournalEventType::NodeStarted,
        Some(started(node, attempt)),
    );
    push_event(
        events,
        JournalEventType::NodeSucceeded,
        Some(evidenced(node, attempt, output_hash)),
    );
    push_event(
        events,
        JournalEventType::NodeCheckpointed,
        Some(evidenced(node, attempt, output_hash)),
    );
}

#[test]
fn complete_graph_replay_requires_each_topological_node_checkpoint() {
    let graph = graph();
    let first_output = ContentHash::digest(b"first-output");
    let second_output = ContentHash::digest(b"second-output");
    let mut events = run_prefix();
    checkpoint_node(&mut events, 'A', 1, &first_output);
    checkpoint_node(&mut events, 'B', 1, &second_output);
    push_event(&mut events, JournalEventType::RunSucceeded, None);

    let result = replay_graph_execution(&graph, &events).unwrap();
    assert_eq!(result.run_state(), RunState::Succeeded);
    assert_eq!(result.completed_nodes(), [id('A'), id('B')]);
    assert_eq!(result.resume_node(), None);
    assert_eq!(result.event_count(), 9);
    let checkpoint = result.last_checkpoint().unwrap();
    assert_eq!(checkpoint.node_id(), &id('B'));
    assert_eq!(checkpoint.output_hash(), &second_output);
    assert_eq!(checkpoint.journal_sequence(), 8);
    assert_eq!(checkpoint.journal_hash(), events[7].content_hash());
}

#[test]
fn interruption_before_checkpoint_reruns_the_same_node_with_next_attempt() {
    let graph = graph();
    let abandoned = ContentHash::digest(b"uncheckpointed-output");
    let committed = ContentHash::digest(b"committed-output");
    let mut events = run_prefix();
    push_event(
        &mut events,
        JournalEventType::NodeStarted,
        Some(started('A', 1)),
    );
    push_event(
        &mut events,
        JournalEventType::NodeSucceeded,
        Some(evidenced('A', 1, &abandoned)),
    );

    let interrupted = replay_graph_execution(&graph, &events).unwrap();
    assert_eq!(interrupted.run_state(), RunState::Running);
    assert_eq!(interrupted.completed_nodes(), []);
    assert_eq!(interrupted.resume_node(), Some(&id('A')));
    assert_eq!(interrupted.last_checkpoint(), None);

    push_event(
        &mut events,
        JournalEventType::NodeStarted,
        Some(started('A', 2)),
    );
    push_event(
        &mut events,
        JournalEventType::NodeSucceeded,
        Some(evidenced('A', 2, &committed)),
    );
    push_event(
        &mut events,
        JournalEventType::NodeCheckpointed,
        Some(evidenced('A', 2, &committed)),
    );
    let recovered = replay_graph_execution(&graph, &events).unwrap();
    assert_eq!(recovered.completed_nodes(), [id('A')]);
    assert_eq!(recovered.resume_node(), Some(&id('B')));
    assert_eq!(recovered.last_checkpoint().unwrap().attempt(), 2);
}

#[test]
fn replay_rejects_wrong_node_terminal_attempt_and_checkpoint_hash() {
    let graph = graph();

    let mut wrong_node = run_prefix();
    push_event(
        &mut wrong_node,
        JournalEventType::NodeStarted,
        Some(started('B', 1)),
    );
    assert_eq!(
        replay_graph_execution(&graph, &wrong_node),
        Err(RuntimeError::Domain(
            DomainErrorCode::InvalidStateTransition
        ))
    );

    let output = ContentHash::digest(b"output");
    let mut wrong_attempt = run_prefix();
    push_event(
        &mut wrong_attempt,
        JournalEventType::NodeStarted,
        Some(started('A', 2)),
    );
    push_event(
        &mut wrong_attempt,
        JournalEventType::NodeSucceeded,
        Some(evidenced('A', 1, &output)),
    );
    assert_eq!(
        replay_graph_execution(&graph, &wrong_attempt),
        Err(RuntimeError::Domain(
            DomainErrorCode::InvalidStateTransition
        ))
    );

    let mut drift = run_prefix();
    push_event(
        &mut drift,
        JournalEventType::NodeStarted,
        Some(started('A', 1)),
    );
    push_event(
        &mut drift,
        JournalEventType::NodeSucceeded,
        Some(evidenced('A', 1, &output)),
    );
    push_event(
        &mut drift,
        JournalEventType::NodeCheckpointed,
        Some(evidenced('A', 1, &ContentHash::digest(b"drift"))),
    );
    assert_eq!(
        replay_graph_execution(&graph, &drift),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
}

#[test]
fn fencing_claim_gaps_allow_first_attempt_two_and_active_one_to_jump_to_three() {
    let graph = graph();

    let mut first_gap = run_prefix();
    push_event(
        &mut first_gap,
        JournalEventType::NodeStarted,
        Some(started('A', 2)),
    );
    let first = replay_graph_execution(&graph, &first_gap).unwrap();
    assert_eq!(first.resume_node(), Some(&id('A')));

    let output = ContentHash::digest(b"attempt-three-output");
    let mut active_gap = run_prefix();
    push_event(
        &mut active_gap,
        JournalEventType::NodeStarted,
        Some(started('A', 1)),
    );
    push_event(
        &mut active_gap,
        JournalEventType::NodeStarted,
        Some(started('A', 3)),
    );
    push_event(
        &mut active_gap,
        JournalEventType::NodeSucceeded,
        Some(evidenced('A', 3, &output)),
    );
    push_event(
        &mut active_gap,
        JournalEventType::NodeCheckpointed,
        Some(evidenced('A', 3, &output)),
    );
    let recovered = replay_graph_execution(&graph, &active_gap).unwrap();
    assert_eq!(recovered.completed_nodes(), [id('A')]);
    assert_eq!(recovered.last_checkpoint().unwrap().attempt(), 3);
}

#[test]
fn fencing_attempts_must_be_strictly_increasing() {
    let graph = graph();
    for attempts in [[2, 2], [3, 2]] {
        let mut events = run_prefix();
        for attempt in attempts {
            push_event(
                &mut events,
                JournalEventType::NodeStarted,
                Some(started('A', attempt)),
            );
        }
        assert_eq!(
            replay_graph_execution(&graph, &events),
            Err(RuntimeError::Domain(
                DomainErrorCode::InvalidStateTransition
            ))
        );
    }
}

#[test]
fn terminal_success_before_all_nodes_and_node_failure_without_run_failure_are_not_complete() {
    let graph = graph();
    let mut premature = run_prefix();
    push_event(&mut premature, JournalEventType::RunSucceeded, None);
    assert_eq!(
        replay_graph_execution(&graph, &premature),
        Err(RuntimeError::Domain(
            DomainErrorCode::InvalidStateTransition
        ))
    );

    let mut failed = run_prefix();
    push_event(
        &mut failed,
        JournalEventType::NodeStarted,
        Some(started('A', 1)),
    );
    push_event(
        &mut failed,
        JournalEventType::NodeFailed,
        Some(evidenced('A', 1, &ContentHash::digest(b"safe-error"))),
    );
    let prefix = replay_graph_execution(&graph, &failed).unwrap();
    assert_eq!(prefix.run_state(), RunState::Running);
    assert_eq!(prefix.resume_node(), None);

    push_event(&mut failed, JournalEventType::RunFailed, None);
    assert_eq!(
        replay_graph_execution(&graph, &failed).unwrap().run_state(),
        RunState::Failed
    );
}
