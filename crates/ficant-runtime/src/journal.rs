use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ficant_domain::primitives::{ContentHash, Ulid};
use ficant_domain::research::{RunJournal, RunJournalInput};
use ficant_domain::{ContentAddressed, DomainErrorCode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    Domain(DomainErrorCode),
    IdempotencyConflict,
    ConcurrencyConflict { expected: u64, actual: u64 },
    RunIdentityConflict,
}

impl From<DomainErrorCode> for RuntimeError {
    fn from(error: DomainErrorCode) -> Self {
        Self::Domain(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Creates a nonblank stable idempotency key.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` when the key is blank or padded.
    pub fn new(value: impl Into<String>) -> Result<Self, RuntimeError> {
        let value = value.into();
        if value.trim().is_empty() || value != value.trim() {
            return Err(RuntimeError::Domain(DomainErrorCode::InvalidValue));
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalAppend {
    input: RunJournalInput,
    claimed_hash: ContentHash,
}

impl JournalAppend {
    #[must_use]
    pub fn new(input: RunJournalInput, claimed_hash: ContentHash) -> Self {
        Self {
            input,
            claimed_hash,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendResult {
    event: RunJournal,
    inserted: bool,
}

impl AppendResult {
    #[must_use]
    pub fn event(&self) -> &RunJournal {
        &self.event
    }

    #[must_use]
    pub fn inserted(&self) -> bool {
        self.inserted
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerRunJournal {
    run_id: Ulid,
    events: Vec<RunJournal>,
    idempotency: BTreeMap<IdempotencyKey, RunJournal>,
}

impl PerRunJournal {
    #[must_use]
    pub fn new(run_id: Ulid) -> Self {
        Self {
            run_id,
            events: Vec::new(),
            idempotency: BTreeMap::new(),
        }
    }

    /// Atomically validates and appends one canonical per-run event.
    ///
    /// # Errors
    ///
    /// Returns a stable runtime/domain conflict for invalid hash, run, sequence, or idempotency.
    pub fn append(
        &mut self,
        idempotency_key: IdempotencyKey,
        expected_next_sequence: u64,
        command: JournalAppend,
    ) -> Result<AppendResult, RuntimeError> {
        let event = RunJournal::new(command.input, &command.claimed_hash)?;

        if let Some(existing) = self.idempotency.get(&idempotency_key) {
            if existing.content_hash() == event.content_hash() {
                return Ok(AppendResult {
                    event: existing.clone(),
                    inserted: false,
                });
            }
            return Err(RuntimeError::IdempotencyConflict);
        }

        let actual_next_sequence = u64::try_from(self.events.len())
            .map_err(|_| RuntimeError::Domain(DomainErrorCode::JournalSequenceConflict))?
            .checked_add(1)
            .ok_or(RuntimeError::Domain(
                DomainErrorCode::JournalSequenceConflict,
            ))?;
        if expected_next_sequence != actual_next_sequence {
            return Err(RuntimeError::ConcurrencyConflict {
                expected: expected_next_sequence,
                actual: actual_next_sequence,
            });
        }
        if event.run_id() != &self.run_id {
            return Err(RuntimeError::RunIdentityConflict);
        }
        if event.sequence() != actual_next_sequence {
            return Err(RuntimeError::Domain(
                DomainErrorCode::JournalSequenceConflict,
            ));
        }
        if let Some(previous) = self.events.last() {
            event.validate_after(previous)?;
        }

        self.events.push(event.clone());
        self.idempotency.insert(idempotency_key, event.clone());
        Ok(AppendResult {
            event,
            inserted: true,
        })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    #[must_use]
    pub fn events(&self) -> &[RunJournal] {
        &self.events
    }

    #[must_use]
    pub fn run_id(&self) -> &Ulid {
        &self.run_id
    }
}

pub(crate) fn verify_canonical_event(event: &RunJournal) -> Result<(), RuntimeError> {
    let input = RunJournalInput {
        journal_event_id: event.id().clone(),
        run_id: event.run_id().clone(),
        sequence: event.sequence(),
        event_type: event.event_type(),
        occurred_at: event.occurred_at().clone(),
        payload_type: event.payload_type().to_owned(),
        payload_schema: event.payload_schema().to_owned(),
        payload: event.payload().to_vec(),
        prev_hash: event.prev_hash().cloned(),
    };
    if &input.canonical_hash()? != event.content_hash() {
        return Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct SharedRunJournal {
    inner: Arc<Mutex<PerRunJournal>>,
}

impl SharedRunJournal {
    #[must_use]
    pub fn new(run_id: Ulid) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PerRunJournal::new(run_id))),
        }
    }

    /// Appends through one OS-thread-safe critical section.
    ///
    /// # Errors
    ///
    /// Returns a stable conflict for invalid events, stale sequence, or poisoned state.
    pub fn append(
        &self,
        idempotency_key: IdempotencyKey,
        expected_next_sequence: u64,
        command: JournalAppend,
    ) -> Result<AppendResult, RuntimeError> {
        self.inner
            .lock()
            .map_err(|_| RuntimeError::ConcurrencyConflict {
                expected: expected_next_sequence,
                actual: 0,
            })?
            .append(idempotency_key, expected_next_sequence, command)
    }

    /// Returns the current immutable event count.
    ///
    /// # Errors
    ///
    /// Returns a concurrency conflict if the shared state is poisoned.
    pub fn len(&self) -> Result<usize, RuntimeError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RuntimeError::ConcurrencyConflict {
                expected: 0,
                actual: 0,
            })?
            .len())
    }

    /// Reports whether the shared journal has no events.
    ///
    /// # Errors
    ///
    /// Returns a concurrency conflict if the shared state is poisoned.
    pub fn is_empty(&self) -> Result<bool, RuntimeError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| RuntimeError::ConcurrencyConflict {
                expected: 0,
                actual: 0,
            })?
            .is_empty())
    }
}
