use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{ContentHash, Ulid};
use ficant_domain::research::{JournalEventType, ResearchGraph, RunJournal, RunState};
use ficant_domain::{DomainErrorCode, DomainResult};

use crate::journal::{RuntimeError, verify_canonical_event};

const PAYLOAD_TYPE: &str = "ficant.graph-node-event";
const PAYLOAD_SCHEMA: &str = "ficant.graph-node-event.v1";
const MAGIC: &[u8; 4] = b"FGNE";
const VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphNodeEvent {
    node_id: Ulid,
    attempt: u32,
    evidence_hash: Option<ContentHash>,
}

impl GraphNodeEvent {
    /// Creates a node-start payload without output evidence.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` when `attempt` is zero.
    pub fn started(node_id: Ulid, attempt: u32) -> DomainResult<Self> {
        Self::new(node_id, attempt, None)
    }

    /// Creates a node payload carrying immutable result, failure, or checkpoint evidence.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` when `attempt` is zero.
    pub fn evidenced(
        node_id: Ulid,
        attempt: u32,
        evidence_hash: ContentHash,
    ) -> DomainResult<Self> {
        Self::new(node_id, attempt, Some(evidence_hash))
    }

    fn new(node_id: Ulid, attempt: u32, evidence_hash: Option<ContentHash>) -> DomainResult<Self> {
        if attempt == 0 {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            node_id,
            attempt,
            evidence_hash,
        })
    }

    #[must_use]
    pub fn node_id(&self) -> &Ulid {
        &self.node_id
    }

    #[must_use]
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    #[must_use]
    pub fn evidence_hash(&self) -> Option<&ContentHash> {
        self.evidence_hash.as_ref()
    }

    #[must_use]
    pub const fn payload_type() -> &'static str {
        PAYLOAD_TYPE
    }

    #[must_use]
    pub const fn payload_schema() -> &'static str {
        PAYLOAD_SCHEMA
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(69);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_be_bytes());
        bytes.extend_from_slice(self.node_id.as_str().as_bytes());
        bytes.extend_from_slice(&self.attempt.to_be_bytes());
        match &self.evidence_hash {
            Some(hash) => {
                bytes.push(1);
                bytes.extend_from_slice(hash.as_bytes());
            }
            None => bytes.push(0),
        }
        bytes
    }

    fn decode(event: &RunJournal) -> Result<Self, RuntimeError> {
        if event.payload_type() != PAYLOAD_TYPE || event.payload_schema() != PAYLOAD_SCHEMA {
            return Err(invalid_value());
        }
        let payload = event.payload();
        if payload.len() < 37 || &payload[..4] != MAGIC || payload[4..6] != VERSION.to_be_bytes() {
            return Err(invalid_value());
        }
        let node_id = std::str::from_utf8(&payload[6..32])
            .map_err(|_| invalid_value())
            .and_then(|value| Ulid::new(value.to_owned()).map_err(RuntimeError::from))?;
        let attempt = u32::from_be_bytes(payload[32..36].try_into().map_err(|_| invalid_value())?);
        let evidence_hash = match payload[36] {
            0 if payload.len() == 37 => None,
            1 if payload.len() == 69 => Some(ContentHash::from_bytes(&payload[37..69])?),
            _ => return Err(invalid_value()),
        };
        let result = Self::new(node_id, attempt, evidence_hash)?;
        let expects_evidence = !matches!(event.event_type(), JournalEventType::NodeStarted);
        if !matches!(
            event.event_type(),
            JournalEventType::NodeStarted
                | JournalEventType::NodeSucceeded
                | JournalEventType::NodeFailed
                | JournalEventType::NodeCheckpointed
        ) || expects_evidence != result.evidence_hash.is_some()
        {
            return Err(invalid_value());
        }
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphCheckpoint {
    node_id: Ulid,
    attempt: u32,
    output_hash: ContentHash,
    journal_sequence: u64,
    journal_hash: ContentHash,
}

impl GraphCheckpoint {
    #[must_use]
    pub fn node_id(&self) -> &Ulid {
        &self.node_id
    }

    #[must_use]
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    #[must_use]
    pub fn output_hash(&self) -> &ContentHash {
        &self.output_hash
    }

    #[must_use]
    pub fn journal_sequence(&self) -> u64 {
        self.journal_sequence
    }

    #[must_use]
    pub fn journal_hash(&self) -> &ContentHash {
        &self.journal_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphReplayResult {
    run_id: Ulid,
    run_state: RunState,
    completed_nodes: Vec<Ulid>,
    resume_node: Option<Ulid>,
    last_checkpoint: Option<GraphCheckpoint>,
    event_count: usize,
}

impl GraphReplayResult {
    #[must_use]
    pub fn run_id(&self) -> &Ulid {
        &self.run_id
    }

    #[must_use]
    pub fn run_state(&self) -> RunState {
        self.run_state
    }

    #[must_use]
    pub fn completed_nodes(&self) -> &[Ulid] {
        &self.completed_nodes
    }

    #[must_use]
    pub fn resume_node(&self) -> Option<&Ulid> {
        self.resume_node.as_ref()
    }

    #[must_use]
    pub fn last_checkpoint(&self) -> Option<&GraphCheckpoint> {
        self.last_checkpoint.as_ref()
    }

    #[must_use]
    pub fn event_count(&self) -> usize {
        self.event_count
    }
}

#[derive(Clone, Debug)]
enum Phase {
    Initial,
    Created,
    Ready {
        index: usize,
    },
    Active {
        index: usize,
        attempt: u32,
    },
    AwaitingCheckpoint {
        index: usize,
        attempt: u32,
        output_hash: ContentHash,
    },
    NodeFailed,
    Terminal(RunState),
}

/// Replays an append-only journal prefix against one frozen graph definition.
///
/// The result identifies only committed nodes as complete. An interrupted active node or an
/// uncheckpointed successful node is returned as `resume_node` and must be rerun.
///
/// # Errors
///
/// Returns a stable conflict for a broken hash chain, illegal run transition, wrong node order,
/// mismatched attempt, or checkpoint/output drift.
pub fn replay_graph_execution(
    graph: &ResearchGraph,
    events: &[RunJournal],
) -> Result<GraphReplayResult, RuntimeError> {
    let first = events.first().ok_or_else(invalid_transition)?;
    let run_id = first.run_id().clone();
    let order = graph.topological_order();
    let mut phase = Phase::Initial;
    let mut previous = None;
    let mut completed_nodes = Vec::new();
    let mut last_checkpoint = None;

    for (index, event) in events.iter().enumerate() {
        verify_journal_position(event, previous, &run_id, index)?;
        phase = transition(
            phase,
            event,
            order,
            &mut completed_nodes,
            &mut last_checkpoint,
        )?;
        previous = Some(event);
    }

    let (run_state, resume_node) = match phase {
        Phase::Initial => return Err(invalid_transition()),
        Phase::Created => (RunState::Created, None),
        Phase::Ready { index } => (RunState::Running, order.get(index).cloned()),
        Phase::Active { index, .. } | Phase::AwaitingCheckpoint { index, .. } => {
            (RunState::Running, order.get(index).cloned())
        }
        Phase::NodeFailed => (RunState::Running, None),
        Phase::Terminal(state) => (state, None),
    };
    Ok(GraphReplayResult {
        run_id,
        run_state,
        completed_nodes,
        resume_node,
        last_checkpoint,
        event_count: events.len(),
    })
}

fn verify_journal_position<'a>(
    event: &'a RunJournal,
    previous: Option<&'a RunJournal>,
    run_id: &Ulid,
    index: usize,
) -> Result<(), RuntimeError> {
    verify_canonical_event(event)?;
    if event.run_id() != run_id {
        return Err(RuntimeError::RunIdentityConflict);
    }
    let expected = u64::try_from(index)
        .map_err(|_| sequence_conflict())?
        .checked_add(1)
        .ok_or_else(sequence_conflict)?;
    if event.sequence() != expected {
        return Err(sequence_conflict());
    }
    if let Some(previous) = previous {
        event.validate_after(previous)?;
    }
    Ok(())
}

fn transition(
    phase: Phase,
    event: &RunJournal,
    order: &[Ulid],
    completed_nodes: &mut Vec<Ulid>,
    last_checkpoint: &mut Option<GraphCheckpoint>,
) -> Result<Phase, RuntimeError> {
    match (phase, event.event_type()) {
        (Phase::Initial, JournalEventType::RunCreated) => Ok(Phase::Created),
        (Phase::Created, JournalEventType::RunStarted) => Ok(Phase::Ready { index: 0 }),
        (Phase::Ready { index }, JournalEventType::NodeStarted) => {
            let payload = GraphNodeEvent::decode(event)?;
            require_node(&payload, order, index, 1)?;
            Ok(Phase::Active { index, attempt: 1 })
        }
        (
            Phase::Active { index, attempt } | Phase::AwaitingCheckpoint { index, attempt, .. },
            JournalEventType::NodeStarted,
        ) => {
            let payload = GraphNodeEvent::decode(event)?;
            let next_attempt = attempt.checked_add(1).ok_or_else(invalid_value)?;
            require_node(&payload, order, index, next_attempt)?;
            Ok(Phase::Active {
                index,
                attempt: next_attempt,
            })
        }
        (Phase::Active { index, attempt }, JournalEventType::NodeSucceeded) => {
            let payload = GraphNodeEvent::decode(event)?;
            require_node(&payload, order, index, attempt)?;
            Ok(Phase::AwaitingCheckpoint {
                index,
                attempt,
                output_hash: payload.evidence_hash.ok_or_else(invalid_value)?,
            })
        }
        (
            Phase::AwaitingCheckpoint {
                index,
                attempt,
                output_hash,
            },
            JournalEventType::NodeCheckpointed,
        ) => {
            let payload = GraphNodeEvent::decode(event)?;
            require_node(&payload, order, index, attempt)?;
            if payload.evidence_hash.as_ref() != Some(&output_hash) {
                return Err(invalid_value());
            }
            let node_id = order.get(index).ok_or_else(invalid_transition)?.clone();
            completed_nodes.push(node_id.clone());
            *last_checkpoint = Some(GraphCheckpoint {
                node_id,
                attempt,
                output_hash,
                journal_sequence: event.sequence(),
                journal_hash: event.content_hash().clone(),
            });
            Ok(Phase::Ready { index: index + 1 })
        }
        (Phase::Active { index, attempt }, JournalEventType::NodeFailed) => {
            let payload = GraphNodeEvent::decode(event)?;
            require_node(&payload, order, index, attempt)?;
            Ok(Phase::NodeFailed)
        }
        (Phase::Ready { index }, JournalEventType::RunSucceeded) if index == order.len() => {
            Ok(Phase::Terminal(RunState::Succeeded))
        }
        (Phase::NodeFailed, JournalEventType::RunFailed) => Ok(Phase::Terminal(RunState::Failed)),
        (
            Phase::Ready { .. } | Phase::Active { .. } | Phase::AwaitingCheckpoint { .. },
            JournalEventType::RunCancelled,
        ) => Ok(Phase::Terminal(RunState::Cancelled)),
        _ => Err(invalid_transition()),
    }
}

fn require_node(
    payload: &GraphNodeEvent,
    order: &[Ulid],
    index: usize,
    attempt: u32,
) -> Result<(), RuntimeError> {
    if order.get(index) != Some(&payload.node_id) || payload.attempt != attempt {
        return Err(invalid_transition());
    }
    Ok(())
}

fn invalid_value() -> RuntimeError {
    RuntimeError::Domain(DomainErrorCode::InvalidValue)
}

fn invalid_transition() -> RuntimeError {
    RuntimeError::Domain(DomainErrorCode::InvalidStateTransition)
}

fn sequence_conflict() -> RuntimeError {
    RuntimeError::Domain(DomainErrorCode::JournalSequenceConflict)
}
