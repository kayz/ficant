use async_trait::async_trait;
use ficant_domain::primitives::{ContentHash, LineageRef, MarketTime, Ulid, VersionRef};
use ficant_domain::research::{ArtifactKind, JournalEventType, RunState};
use ficant_domain::{ContentAddressed, Lineaged};
use ficant_runtime::replay;

use super::fingerprint::FingerprintBuilder;
use super::{
    AppendJournalEvent, AppendMarketFact, ApplicationResult, CreateExperimentRun, IdempotencyKey,
    OperationFingerprint, PublishArtifact, PublishSignalSet, PublishSnapshot, SnapshotValue,
    TransitionExperimentRun,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_runtime_error};

pub trait Clock: Send + Sync {
    /// Returns one application-controlled market instant.
    ///
    /// # Errors
    ///
    /// Returns an application error when the clock cannot provide a valid instant.
    fn now(&self) -> ApplicationResult<MarketTime>;
}

pub trait IdGenerator: Send + Sync {
    /// Returns one service-owned domain identity.
    ///
    /// # Errors
    ///
    /// Returns an application error when identity generation is unavailable.
    fn next_id(&self) -> ApplicationResult<Ulid>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase1RunWork {
    run: CreateExperimentRun,
    transitions: Vec<TransitionExperimentRun>,
    journal: Vec<AppendJournalEvent>,
}

impl Phase1RunWork {
    pub(crate) fn new(
        run: CreateExperimentRun,
        transitions: Vec<TransitionExperimentRun>,
        journal: Vec<AppendJournalEvent>,
    ) -> ApplicationResult<Self> {
        if transitions
            .iter()
            .any(|command| command.scope() != run.scope())
            || journal.iter().any(|command| command.scope() != run.scope())
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::Forbidden,
                false,
            ));
        }
        if transitions
            .iter()
            .any(|command| command.target_owner() != run.target_owner())
            || journal
                .iter()
                .any(|command| command.target_owner() != run.target_owner())
        {
            return Err(lineage_error());
        }
        Ok(Self {
            run,
            transitions,
            journal,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &super::AccessScope {
        self.run.scope()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase1PublicationWork {
    snapshots: Vec<PublishSnapshot>,
    artifact: PublishArtifact,
    signal: PublishSignalSet,
}

impl Phase1PublicationWork {
    #[must_use]
    pub(crate) fn new(
        snapshots: Vec<PublishSnapshot>,
        artifact: PublishArtifact,
        signal: PublishSignalSet,
    ) -> Self {
        Self {
            snapshots,
            artifact,
            signal,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Validated transaction input. External consumers can inspect it but cannot construct it.
///
/// ```compile_fail
/// use ficant_application::ports::{Phase1AtomicWork, Phase1PublicationWork, Phase1RunWork};
/// let run = Phase1RunWork::new(panic!(), Vec::new(), Vec::new());
/// let publications = Phase1PublicationWork::new(Vec::new(), panic!(), panic!());
/// let _work = Phase1AtomicWork::new(panic!(), panic!(), run, publications);
/// ```
pub struct Phase1AtomicWork {
    idempotency_key: IdempotencyKey,
    fact: AppendMarketFact,
    run_work: Phase1RunWork,
    publications: Phase1PublicationWork,
    fingerprint: OperationFingerprint,
}

impl Phase1AtomicWork {
    /// Creates the single application-owned Phase 1 transaction intent.
    ///
    /// # Errors
    ///
    /// Returns a safe error unless snapshots, run transitions, Journal and publications agree.
    pub(crate) fn new(
        idempotency_key: IdempotencyKey,
        fact: AppendMarketFact,
        run_work: Phase1RunWork,
        publications: Phase1PublicationWork,
    ) -> ApplicationResult<Self> {
        let has_data = publications
            .snapshots
            .iter()
            .any(|command| matches!(command.snapshot(), SnapshotValue::Data(_)));
        let has_universe = publications
            .snapshots
            .iter()
            .any(|command| matches!(command.snapshot(), SnapshotValue::Universe(_)));
        if publications.snapshots.len() != 2 || !has_data || !has_universe {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::LineageIncomplete,
                false,
            ));
        }

        let data_snapshot = publications
            .snapshots
            .iter()
            .find(|command| matches!(command.snapshot(), SnapshotValue::Data(_)))
            .expect("exactly one Data snapshot was validated");
        let universe_snapshot = publications
            .snapshots
            .iter()
            .find(|command| matches!(command.snapshot(), SnapshotValue::Universe(_)))
            .expect("exactly one Universe snapshot was validated");

        validate_aggregate_relationships(
            &fact,
            data_snapshot,
            universe_snapshot,
            &run_work,
            &publications,
        )?;

        if run_work.transitions.len() != 2
            || run_work.transitions[0].run_id() != run_work.run.run().id()
            || run_work.transitions[0].expected_revision() != 1
            || run_work.transitions[0].next_state() != RunState::Running
            || run_work.transitions[1].run_id() != run_work.run.run().id()
            || run_work.transitions[1].expected_revision() != 2
            || run_work.transitions[1].next_state() != RunState::Succeeded
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::StateConflict,
                false,
            ));
        }

        validate_journal_evidence(
            run_work.run.run().id(),
            publications.artifact.artifact().content_hash(),
            publications.signal.signal_set().content_hash(),
            &run_work.journal,
        )?;
        let events = run_work
            .journal
            .iter()
            .map(|command| command.event().clone())
            .collect::<Vec<_>>();
        let replayed = replay(&events).map_err(|error| map_runtime_error(&error))?;
        if replayed.run_id() != run_work.run.run().id()
            || replayed.terminal_state() != RunState::Succeeded
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::LineageIncomplete,
                false,
            ));
        }

        let idempotency_key = idempotency_key.scoped_to(run_work.scope())?;
        let fingerprint = atomic_fingerprint(&fact, &run_work, &publications);

        Ok(Self {
            idempotency_key,
            fact,
            run_work,
            publications,
            fingerprint,
        })
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    #[must_use]
    pub fn fact(&self) -> &AppendMarketFact {
        &self.fact
    }

    #[must_use]
    pub fn snapshots(&self) -> &[PublishSnapshot] {
        &self.publications.snapshots
    }

    #[must_use]
    pub fn run(&self) -> &CreateExperimentRun {
        &self.run_work.run
    }

    #[must_use]
    pub fn transitions(&self) -> &[TransitionExperimentRun] {
        &self.run_work.transitions
    }

    #[must_use]
    pub fn journal(&self) -> &[AppendJournalEvent] {
        &self.run_work.journal
    }

    #[must_use]
    pub fn artifact(&self) -> &PublishArtifact {
        &self.publications.artifact
    }

    #[must_use]
    pub fn signal(&self) -> &PublishSignalSet {
        &self.publications.signal
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
}

fn atomic_fingerprint(
    fact: &AppendMarketFact,
    run_work: &Phase1RunWork,
    publications: &Phase1PublicationWork,
) -> OperationFingerprint {
    let mut canonical = FingerprintBuilder::new("phase1-atomic-work/v2");
    canonical.field(2, run_work.scope().fingerprint().content_hash().as_bytes());
    canonical.field(
        3,
        &super::fingerprint::owner_bytes(run_work.run.target_owner()),
    );
    canonical.field(4, fact.fingerprint().content_hash().as_bytes());
    for snapshot in &publications.snapshots {
        canonical.field(5, snapshot.fingerprint().content_hash().as_bytes());
    }
    canonical.field(6, run_work.run.fingerprint().content_hash().as_bytes());
    for transition in &run_work.transitions {
        canonical.field(7, transition.fingerprint().content_hash().as_bytes());
    }
    for event in &run_work.journal {
        canonical.field(8, event.fingerprint().content_hash().as_bytes());
    }
    canonical.field(
        9,
        publications
            .artifact
            .fingerprint()
            .content_hash()
            .as_bytes(),
    );
    canonical.field(
        10,
        publications.signal.fingerprint().content_hash().as_bytes(),
    );
    canonical.finish()
}

fn validate_aggregate_relationships(
    fact: &AppendMarketFact,
    data_snapshot: &PublishSnapshot,
    universe_snapshot: &PublishSnapshot,
    run_work: &Phase1RunWork,
    publications: &Phase1PublicationWork,
) -> ApplicationResult<()> {
    let run = run_work.run.run();
    let artifact = publications.artifact.artifact();
    let signal = publications.signal.signal_set();
    let owner = run.owner();
    if fact.fact().owner() != owner
        || data_snapshot.snapshot().owner() != owner
        || universe_snapshot.snapshot().owner() != owner
        || artifact.owner() != owner
        || signal.owner() != owner
    {
        return Err(lineage_error());
    }
    if !data_snapshot
        .snapshot()
        .lineage()
        .contains(&fact.fact().lineage_ref()?)
    {
        return Err(lineage_error());
    }

    let data_ref = LineageRef::content_addressed(
        data_snapshot.snapshot().id().clone(),
        data_snapshot.snapshot().content_hash().clone(),
    );
    let universe_ref = LineageRef::content_addressed(
        universe_snapshot.snapshot().id().clone(),
        universe_snapshot.snapshot().content_hash().clone(),
    );
    if run.data_snapshot() != &data_ref
        || run.universe_snapshot() != &universe_ref
        || signal.experiment_run_id() != run.id()
        || signal.data_snapshot() != &data_ref
        || signal.universe_snapshot() != &universe_ref
        || signal.id() == artifact.id()
        || signal.artifact().object_id() != artifact.id()
        || signal.artifact().version().is_some()
        || signal.artifact().content_hash() != Some(artifact.content_hash())
        || signal.content_hash() != artifact.content_hash()
        || artifact.kind() != ArtifactKind::SignalSet
        || !same_version_ref_set(run.rule_packs(), signal.rule_packs())
    {
        return Err(lineage_error());
    }
    validate_verified_blob_match(
        publications.artifact.verified_blob(),
        publications.signal.verified_blob(),
    )?;
    if signal
        .lineage()
        .iter()
        .filter(|reference| *reference != signal.artifact())
        .any(|reference| !artifact.lineage().contains(reference))
    {
        return Err(lineage_error());
    }
    Ok(())
}

fn validate_journal_evidence(
    run_id: &Ulid,
    artifact_hash: &ContentHash,
    signal_hash: &ContentHash,
    journal: &[AppendJournalEvent],
) -> ApplicationResult<()> {
    let expected = [
        (JournalEventType::RunCreated, run_id.as_str().as_bytes()),
        (JournalEventType::RunStarted, run_id.as_str().as_bytes()),
        (
            JournalEventType::ArtifactPublished,
            artifact_hash.as_bytes(),
        ),
        (JournalEventType::SignalSetPublished, signal_hash.as_bytes()),
        (JournalEventType::RunSucceeded, run_id.as_str().as_bytes()),
    ];
    if journal.len() != expected.len() {
        return Err(state_error());
    }
    for (command, (expected_type, expected_payload)) in journal.iter().zip(expected) {
        let event = command.event();
        if event.event_type() != expected_type
            || event.payload() != expected_payload
            || event.payload_type() != "ficant.research.v1.Phase1Event"
            || event.payload_schema() != "v1"
        {
            return Err(state_error());
        }
    }
    Ok(())
}

fn validate_verified_blob_match(
    artifact: &super::VerifiedBlobRef,
    signal: &super::VerifiedBlobRef,
) -> ApplicationResult<()> {
    if artifact != signal {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::ValidationFailed,
            false,
        ));
    }
    Ok(())
}

fn same_version_ref_set(left: &[VersionRef], right: &[VersionRef]) -> bool {
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort();
    right.sort();
    left == right
}

fn lineage_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

fn state_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}

#[test]
fn r7_direct_composite_validator_rejects_three_event_journal() {
    use ficant_domain::ContentAddressed;
    use ficant_domain::research::{JournalEventType, RunJournal, RunJournalInput};

    fn id(suffix: char) -> Ulid {
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
    }
    fn time() -> MarketTime {
        MarketTime::new(
            "2026-03-04T08:00:00Z".parse().unwrap(),
            "Asia/Shanghai",
            "2026-03-04".parse().unwrap(),
        )
        .unwrap()
    }
    fn event(
        run_id: &Ulid,
        sequence: u64,
        event_type: JournalEventType,
        previous: Option<ContentHash>,
        suffix: char,
    ) -> RunJournal {
        let input = RunJournalInput {
            journal_event_id: id(suffix),
            run_id: run_id.clone(),
            sequence,
            event_type,
            occurred_at: time(),
            payload_type: "ficant.research.v1.Phase1Event".to_owned(),
            payload_schema: "v1".to_owned(),
            payload: run_id.as_str().as_bytes().to_vec(),
            prev_hash: previous,
        };
        let claimed = input.canonical_hash().unwrap();
        RunJournal::new(input, &claimed).unwrap()
    }

    let run_id = id('R');
    let owner = ficant_domain::primitives::OwnerRef::new(id('T'), id('Y'));
    let scope = super::AccessScope::new(id('T'), id('A'), vec![id('Y')]).unwrap();
    let created = event(&run_id, 1, JournalEventType::RunCreated, None, 'A');
    let started = event(
        &run_id,
        2,
        JournalEventType::RunStarted,
        Some(created.content_hash().clone()),
        'B',
    );
    let succeeded = event(
        &run_id,
        3,
        JournalEventType::RunSucceeded,
        Some(started.content_hash().clone()),
        'C',
    );
    let commands = [created, started, succeeded]
        .into_iter()
        .enumerate()
        .map(|(index, event)| {
            let sequence = u64::try_from(index).unwrap() + 1;
            AppendJournalEvent::new(
                scope.clone(),
                owner.clone(),
                run_id.clone(),
                sequence,
                event,
                IdempotencyKey::new(format!("direct-{sequence}")).unwrap(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        validate_journal_evidence(
            &run_id,
            &ContentHash::digest(b"a"),
            &ContentHash::digest(b"s"),
            &commands
        )
        .unwrap_err()
        .category(),
        ApplicationErrorCategory::StateConflict
    );
}

#[test]
fn r7_direct_composite_validator_rejects_wrong_artifact_payload() {
    use ficant_domain::ContentAddressed;
    use ficant_domain::research::{JournalEventType, RunJournal, RunJournalInput};

    fn id(suffix: char) -> Ulid {
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
    }
    fn time() -> MarketTime {
        MarketTime::new(
            "2026-03-04T08:00:00Z".parse().unwrap(),
            "Asia/Shanghai",
            "2026-03-04".parse().unwrap(),
        )
        .unwrap()
    }
    fn event(
        run_id: &Ulid,
        sequence: u64,
        event_type: JournalEventType,
        previous: Option<ContentHash>,
        suffix: char,
    ) -> RunJournal {
        let input = RunJournalInput {
            journal_event_id: id(suffix),
            run_id: run_id.clone(),
            sequence,
            event_type,
            occurred_at: time(),
            payload_type: "ficant.research.v1.Phase1Event".to_owned(),
            payload_schema: "v1".to_owned(),
            payload: run_id.as_str().as_bytes().to_vec(),
            prev_hash: previous,
        };
        let claimed = input.canonical_hash().unwrap();
        RunJournal::new(input, &claimed).unwrap()
    }

    let run_id = id('R');
    let owner = ficant_domain::primitives::OwnerRef::new(id('T'), id('Y'));
    let scope = super::AccessScope::new(id('T'), id('A'), vec![id('Y')]).unwrap();
    let created = event(&run_id, 1, JournalEventType::RunCreated, None, 'D');
    let started = event(
        &run_id,
        2,
        JournalEventType::RunStarted,
        Some(created.content_hash().clone()),
        'E',
    );
    let artifact = event(
        &run_id,
        3,
        JournalEventType::ArtifactPublished,
        Some(started.content_hash().clone()),
        'F',
    );
    let signal = event(
        &run_id,
        4,
        JournalEventType::SignalSetPublished,
        Some(artifact.content_hash().clone()),
        'G',
    );
    let succeeded = event(
        &run_id,
        5,
        JournalEventType::RunSucceeded,
        Some(signal.content_hash().clone()),
        'H',
    );
    let wrong_payload_commands = [created, started, artifact, signal, succeeded]
        .into_iter()
        .enumerate()
        .map(|(index, event)| {
            let sequence = u64::try_from(index).unwrap() + 1;
            AppendJournalEvent::new(
                scope.clone(),
                owner.clone(),
                run_id.clone(),
                sequence,
                event,
                IdempotencyKey::new(format!("wrong-payload-{sequence}")).unwrap(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        validate_journal_evidence(
            &run_id,
            &ContentHash::digest(b"artifact"),
            &ContentHash::digest(b"signal"),
            &wrong_payload_commands,
        )
        .unwrap_err()
        .category(),
        ApplicationErrorCategory::StateConflict
    );
}

#[test]
fn r7_direct_composite_validator_rejects_verified_size_drift() {
    let artifact = super::VerifiedBlobRef::new(ContentHash::digest(b"same"), 13).unwrap();
    let signal = super::VerifiedBlobRef::new(ContentHash::digest(b"same"), 14).unwrap();
    assert_eq!(
        validate_verified_blob_match(&artifact, &signal)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ValidationFailed
    );
}

#[async_trait]
pub trait TransactionRunner: Send + Sync {
    /// Atomically commits all repository intents in one storage-owned transaction.
    ///
    /// # Errors
    ///
    /// Returns an application error and commits none of the work on failure.
    async fn commit_phase1(&self, work: &Phase1AtomicWork) -> ApplicationResult<()>;
}
