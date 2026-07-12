use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::{ContentHash, Ulid};
use ficant_domain::research::{JournalEventType, RunJournal, RunState};

use crate::digest::replay_digest;
use crate::journal::{RuntimeError, verify_canonical_event};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayResult {
    run_id: Ulid,
    terminal_state: RunState,
    event_count: usize,
    digest: ContentHash,
}

impl ReplayResult {
    #[must_use]
    pub fn run_id(&self) -> &Ulid {
        &self.run_id
    }

    #[must_use]
    pub fn terminal_state(&self) -> RunState {
        self.terminal_state
    }

    #[must_use]
    pub fn event_count(&self) -> usize {
        self.event_count
    }

    #[must_use]
    pub fn digest(&self) -> &ContentHash {
        &self.digest
    }
}

/// Replays events exactly in caller-provided order and derives a canonical digest.
///
/// # Errors
///
/// Returns a stable runtime/domain conflict for empty, broken, cross-run, or illegal journals.
pub fn replay(events: &[RunJournal]) -> Result<ReplayResult, RuntimeError> {
    let first = events.first().ok_or(RuntimeError::Domain(
        DomainErrorCode::InvalidStateTransition,
    ))?;
    let run_id = first.run_id().clone();
    let mut state = None;
    let mut previous = None;

    for (index, event) in events.iter().enumerate() {
        verify_canonical_event(event)?;
        if event.run_id() != &run_id {
            return Err(RuntimeError::RunIdentityConflict);
        }
        let expected_sequence = u64::try_from(index)
            .map_err(|_| RuntimeError::Domain(DomainErrorCode::JournalSequenceConflict))?
            .checked_add(1)
            .ok_or(RuntimeError::Domain(
                DomainErrorCode::JournalSequenceConflict,
            ))?;
        if event.sequence() != expected_sequence {
            return Err(RuntimeError::Domain(
                DomainErrorCode::JournalSequenceConflict,
            ));
        }
        if let Some(previous_event) = previous {
            event.validate_after(previous_event)?;
        }
        state = Some(transition(state, event.event_type())?);
        previous = Some(event);
    }

    let terminal_state = state.ok_or(RuntimeError::Domain(
        DomainErrorCode::InvalidStateTransition,
    ))?;
    if !matches!(
        terminal_state,
        RunState::Succeeded | RunState::Failed | RunState::Cancelled
    ) {
        return Err(RuntimeError::Domain(
            DomainErrorCode::InvalidStateTransition,
        ));
    }
    Ok(ReplayResult {
        run_id,
        terminal_state,
        event_count: events.len(),
        digest: replay_digest(events, terminal_state),
    })
}

fn transition(
    current: Option<RunState>,
    event_type: JournalEventType,
) -> Result<RunState, RuntimeError> {
    let next = match (current, event_type) {
        (None, JournalEventType::RunCreated) => RunState::Created,
        (Some(RunState::Created), JournalEventType::RunStarted)
        | (
            Some(RunState::Running),
            JournalEventType::ArtifactPublished | JournalEventType::SignalSetPublished,
        ) => RunState::Running,
        (Some(RunState::Running), JournalEventType::RunSucceeded) => RunState::Succeeded,
        (Some(RunState::Running), JournalEventType::RunFailed) => RunState::Failed,
        (Some(RunState::Running), JournalEventType::RunCancelled) => RunState::Cancelled,
        _ => {
            return Err(RuntimeError::Domain(
                DomainErrorCode::InvalidStateTransition,
            ));
        }
    };
    Ok(next)
}
