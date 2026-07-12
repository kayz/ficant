use std::collections::BTreeSet;

use ficant_domain::primitives::{LineageRef, MarketTime, Ulid};
use ficant_domain::research::{
    Artifact, ArtifactKind, ExperimentRun, JournalEventType, RunJournal, RunJournalInput, RunState,
    SignalSet,
};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged};
use ficant_runtime::{ReplayResult, RuntimeError, replay};

use crate::ports::{
    AccessScope, AppendJournalEvent, AppendMarketFact, ApplicationResult, BlobStore, Clock,
    CreateExperimentRun, FullyValidatedMarketFact, IdGenerator, IdempotencyKey,
    OperationFingerprint, PageRequest, Phase1AtomicWork, Phase1PublicationWork, Phase1RunWork,
    Phase1ValidatedExperimentRun, PublishArtifact, PublishSignalSet, PublishSnapshot,
    RunJournalRepository, SnapshotBlobRole, SnapshotValue, StagedSnapshotBlob, StagedSnapshotProof,
    StagedSnapshotProofParts, TransactionRunner, TransitionExperimentRun, VerifiedBlobRef,
    VerifiedSnapshotBlob, VerifiedSnapshotProof, VerifyBlobStage, cursor_cycle_error,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error, map_runtime_error};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedSnapshot {
    snapshot: SnapshotValue,
    proof: StagedSnapshotProof,
}

impl StagedSnapshot {
    /// Binds one snapshot to the exact staged hash that must be verified.
    ///
    /// # Errors
    ///
    /// Returns hash mismatch when the stage expectation differs from snapshot content.
    pub fn new(snapshot: SnapshotValue, verification: VerifyBlobStage) -> ApplicationResult<Self> {
        if matches!(snapshot, SnapshotValue::Data(_)) {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let proof = StagedSnapshotProof::universe(StagedSnapshotBlob::new(
            SnapshotBlobRole::UniverseMembersManifest,
            verification,
        ))?;
        Self::from_proof(snapshot, proof)
    }

    /// Binds a snapshot to its complete role-safe staged proof.
    ///
    /// # Errors
    ///
    /// Returns a safe error for missing, extra, swapped, unauthorized or mismatched candidates.
    pub fn from_proof(
        snapshot: SnapshotValue,
        proof: StagedSnapshotProof,
    ) -> ApplicationResult<Self> {
        proof.validate_for(&snapshot)?;
        Ok(Self { snapshot, proof })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedArtifact {
    artifact: Artifact,
    signal_set: SignalSet,
    verification: VerifyBlobStage,
}

impl StagedArtifact {
    /// Binds a `SignalSet` artifact and exact staged content into one publish intent.
    ///
    /// # Errors
    ///
    /// Returns a safe mismatch error for ID, hash or size disagreement.
    pub fn new(
        artifact: Artifact,
        signal_set: SignalSet,
        verification: VerifyBlobStage,
    ) -> ApplicationResult<Self> {
        if artifact.id() == signal_set.id() || signal_set.artifact().object_id() != artifact.id() {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        if artifact.kind() != ArtifactKind::SignalSet {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        if artifact.owner() != signal_set.owner() {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        if artifact.content_hash() != signal_set.content_hash()
            || artifact.content_hash() != verification.expected_hash()
        {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        if artifact.blob_size() != verification.expected_size() {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        if signal_set
            .lineage()
            .iter()
            .filter(|reference| *reference != signal_set.artifact())
            .any(|reference| !artifact.lineage().contains(reference))
        {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        Ok(Self {
            artifact,
            signal_set,
            verification,
        })
    }
}

/// Raw market facts cannot enter Phase 1 without a validated unit proof.
///
/// ```compile_fail
/// use ficant_application::{Phase1BusinessInput, IdempotencyKey};
/// use ficant_application::ports::MarketFact;
/// let raw: MarketFact = panic!();
/// let _ = Phase1BusinessInput::new(
///     panic!(), raw, panic!(), panic!(), panic!(), panic!(),
///     IdempotencyKey::new("phase1").unwrap(),
/// );
/// ```
///
/// A raw run cannot replace the Phase 1 candidate wrapper.
///
/// ```compile_fail
/// use ficant_application::{Phase1BusinessInput, IdempotencyKey};
/// use ficant_application::ports::FullyValidatedMarketFact;
/// use ficant_domain::research::ExperimentRun;
/// let fact: FullyValidatedMarketFact = panic!();
/// let raw: ExperimentRun = panic!();
/// let _ = Phase1BusinessInput::new(
///     panic!(), fact, panic!(), panic!(), raw, panic!(),
///     IdempotencyKey::new("phase1").unwrap(),
/// );
/// ```
///
/// A persisted-run wrapper cannot be substituted for a Phase 1 candidate.
///
/// ```compile_fail
/// use ficant_application::{Phase1BusinessInput, IdempotencyKey};
/// use ficant_application::ports::ValidatedExperimentRun;
/// let persisted: ValidatedExperimentRun = panic!();
/// let _ = Phase1BusinessInput::new(
///     panic!(), panic!(), panic!(), panic!(), persisted, panic!(),
///     IdempotencyKey::new("phase1").unwrap(),
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase1BusinessInput {
    scope: AccessScope,
    fact: FullyValidatedMarketFact,
    data_snapshot: StagedSnapshot,
    universe_snapshot: StagedSnapshot,
    run: Phase1ValidatedExperimentRun,
    artifact: StagedArtifact,
    idempotency_key: IdempotencyKey,
}

impl Phase1BusinessInput {
    /// Creates a cross-object Phase 1 business request before any I/O starts.
    ///
    /// # Errors
    ///
    /// Returns incomplete lineage unless snapshots, run, artifact and signal agree exactly.
    pub fn new(
        scope: AccessScope,
        fact: FullyValidatedMarketFact,
        data_snapshot: StagedSnapshot,
        universe_snapshot: StagedSnapshot,
        run: Phase1ValidatedExperimentRun,
        artifact: StagedArtifact,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        fact.authorize_scope(&scope)?;
        let raw_fact = fact.fact();
        run.authorize_scope(&scope)?;
        let SnapshotValue::Data(resolved_data_snapshot) = &data_snapshot.snapshot else {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        };
        run.validate_snapshot(resolved_data_snapshot)?;
        let raw_run = run.run();
        if !matches!(data_snapshot.snapshot, SnapshotValue::Data(_))
            || !matches!(universe_snapshot.snapshot, SnapshotValue::Universe(_))
        {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let data_ref = LineageRef::content_addressed(
            data_snapshot.snapshot.id().clone(),
            data_snapshot.snapshot.content_hash().clone(),
        );
        let universe_ref = LineageRef::content_addressed(
            universe_snapshot.snapshot.id().clone(),
            universe_snapshot.snapshot.content_hash().clone(),
        );
        let common_owner = raw_run.owner();
        scope.authorize(common_owner)?;
        if !data_snapshot.proof.all_scopes_match(&scope)
            || !universe_snapshot.proof.all_scopes_match(&scope)
            || artifact.verification.scope() != &scope
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::Forbidden,
                false,
            ));
        }
        if raw_fact.owner() != common_owner
            || data_snapshot.snapshot.owner() != common_owner
            || universe_snapshot.snapshot.owner() != common_owner
            || artifact.artifact.owner() != common_owner
            || artifact.signal_set.owner() != common_owner
            || !data_snapshot
                .snapshot
                .lineage()
                .contains(&raw_fact.lineage_ref()?)
        {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        if raw_run.data_snapshot() != &data_ref
            || raw_run.universe_snapshot() != &universe_ref
            || artifact.signal_set.experiment_run_id() != raw_run.id()
            || artifact.signal_set.data_snapshot() != &data_ref
            || artifact.signal_set.universe_snapshot() != &universe_ref
        {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let idempotency_key = idempotency_key.scoped_to(&scope)?;
        Ok(Self {
            scope,
            fact,
            data_snapshot,
            universe_snapshot,
            run,
            artifact,
            idempotency_key,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        &self.scope
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase1BusinessResult {
    run_id: Ulid,
    terminal_state: RunState,
    fingerprint: OperationFingerprint,
}

impl Phase1BusinessResult {
    #[must_use]
    pub fn run_id(&self) -> &Ulid {
        &self.run_id
    }

    #[must_use]
    pub fn terminal_state(&self) -> RunState {
        self.terminal_state
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
}

pub struct Phase1BusinessLoop<'a> {
    clock: &'a dyn Clock,
    id_generator: &'a dyn IdGenerator,
    blob_store: &'a dyn BlobStore,
    transaction_runner: &'a dyn TransactionRunner,
}

impl<'a> Phase1BusinessLoop<'a> {
    #[must_use]
    pub fn new(
        clock: &'a dyn Clock,
        id_generator: &'a dyn IdGenerator,
        blob_store: &'a dyn BlobStore,
        transaction_runner: &'a dyn TransactionRunner,
    ) -> Self {
        Self {
            clock,
            id_generator,
            blob_store,
            transaction_runner,
        }
    }

    /// Verifies staged content, prepares the complete Phase 1 work unit and commits once.
    ///
    /// # Errors
    ///
    /// Returns before transaction submission on verification, lineage or replay failure.
    pub async fn execute(
        &self,
        input: Phase1BusinessInput,
    ) -> ApplicationResult<Phase1BusinessResult> {
        let Phase1BusinessInput {
            scope,
            fact,
            data_snapshot,
            universe_snapshot,
            run,
            artifact,
            idempotency_key,
        } = input;
        let raw_run = run.run().clone();
        let persisted_run = run.into_persisted_validation(&scope)?;
        let run_command = CreateExperimentRun::new(
            scope.clone(),
            persisted_run,
            idempotency_key.scoped("run-create")?,
        )?;
        let occurred_at = self.clock.now()?;

        let data_verified = promote_snapshot_proof(self.blob_store, data_snapshot.proof).await?;
        let universe_verified =
            promote_snapshot_proof(self.blob_store, universe_snapshot.proof).await?;
        let artifact_verified = self
            .blob_store
            .verify_and_promote(artifact.verification)
            .await?;
        validate_artifact_verified(&artifact.artifact, &artifact_verified)?;

        let fact_command = AppendMarketFact::new(fact, idempotency_key.scoped("fact")?)?;
        let snapshot_commands = vec![
            PublishSnapshot::new(
                data_snapshot.snapshot,
                data_verified,
                idempotency_key.scoped("data-snapshot")?,
            )?,
            PublishSnapshot::new(
                universe_snapshot.snapshot,
                universe_verified,
                idempotency_key.scoped("universe-snapshot")?,
            )?,
        ];
        let transitions = vec![
            TransitionExperimentRun::new(
                scope.clone(),
                raw_run.owner().clone(),
                raw_run.id().clone(),
                1,
                RunState::Running,
                idempotency_key.scoped("run-running")?,
            )?,
            TransitionExperimentRun::new(
                scope.clone(),
                raw_run.owner().clone(),
                raw_run.id().clone(),
                2,
                RunState::Succeeded,
                idempotency_key.scoped("run-succeeded")?,
            )?,
        ];
        let journal = self.build_journal(
            &scope,
            &raw_run,
            &artifact.artifact,
            &artifact.signal_set,
            &occurred_at,
            &idempotency_key,
        )?;
        let artifact_command = PublishArtifact::new(
            artifact.artifact,
            artifact_verified.clone(),
            idempotency_key.scoped("artifact")?,
        )?;
        let signal_command = PublishSignalSet::new(
            artifact.signal_set,
            artifact_verified,
            idempotency_key.scoped("signal")?,
        )?;
        let work = Phase1AtomicWork::new(
            idempotency_key,
            fact_command,
            Phase1RunWork::new(run_command, transitions, journal)?,
            Phase1PublicationWork::new(snapshot_commands, artifact_command, signal_command),
        )?;
        let result = Phase1BusinessResult {
            run_id: raw_run.id().clone(),
            terminal_state: RunState::Succeeded,
            fingerprint: work.fingerprint().clone(),
        };
        self.transaction_runner.commit_phase1(&work).await?;
        Ok(result)
    }

    /// Reads every stable Journal page in adapter order and proves completed replay.
    ///
    /// # Errors
    ///
    /// Returns a safe application error for read failures, cursor cycles or invalid replay.
    pub async fn replay_run(
        &self,
        journal: &dyn RunJournalRepository,
        scope: &AccessScope,
        run_id: Ulid,
        page_size: u32,
    ) -> ApplicationResult<ReplayResult> {
        let mut cursor = None;
        let mut seen_cursors = BTreeSet::new();
        let mut events = Vec::new();
        loop {
            let request = PageRequest::new(scope.clone(), cursor, page_size)?;
            request.authorize_scope(scope)?;
            let page = journal.read(scope, run_id.clone(), request).await?;
            let (items, next_cursor) = page.into_parts();
            events.extend(items);
            match next_cursor {
                Some(next) => {
                    if !seen_cursors.insert(next.clone()) {
                        return Err(cursor_cycle_error());
                    }
                    cursor = Some(next);
                }
                None => break,
            }
        }
        replay_collected_journal(&run_id, &events)
    }

    fn build_journal(
        &self,
        scope: &AccessScope,
        run: &ExperimentRun,
        artifact: &Artifact,
        signal: &SignalSet,
        occurred_at: &MarketTime,
        idempotency_key: &IdempotencyKey,
    ) -> ApplicationResult<Vec<AppendJournalEvent>> {
        let specifications = [
            (
                JournalEventType::RunCreated,
                run.id().as_str().as_bytes().to_vec(),
            ),
            (
                JournalEventType::RunStarted,
                run.id().as_str().as_bytes().to_vec(),
            ),
            (
                JournalEventType::ArtifactPublished,
                artifact.content_hash().as_bytes().to_vec(),
            ),
            (
                JournalEventType::SignalSetPublished,
                signal.content_hash().as_bytes().to_vec(),
            ),
            (
                JournalEventType::RunSucceeded,
                run.id().as_str().as_bytes().to_vec(),
            ),
        ];
        let mut previous = None;
        let mut commands = Vec::with_capacity(specifications.len());
        for (index, (event_type, payload)) in specifications.into_iter().enumerate() {
            let sequence = u64::try_from(index)
                .map_err(|_| map_domain_error(DomainErrorCode::JournalSequenceConflict))?
                .checked_add(1)
                .ok_or_else(|| map_domain_error(DomainErrorCode::JournalSequenceConflict))?;
            let input = RunJournalInput {
                journal_event_id: self.id_generator.next_id()?,
                run_id: run.id().clone(),
                sequence,
                event_type,
                occurred_at: occurred_at.clone(),
                payload_type: "ficant.research.v1.Phase1Event".to_owned(),
                payload_schema: "v1".to_owned(),
                payload,
                prev_hash: previous,
            };
            let claimed_hash = input.canonical_hash().map_err(map_domain_error)?;
            let event = RunJournal::new(input, &claimed_hash).map_err(map_domain_error)?;
            previous = Some(event.content_hash().clone());
            commands.push(AppendJournalEvent::new(
                scope.clone(),
                run.owner().clone(),
                run.id().clone(),
                sequence,
                event,
                idempotency_key.scoped(&format!("journal-{sequence}"))?,
            )?);
        }
        Ok(commands)
    }
}

/// Replays already collected canonical Journal events as a completed-run proof.
///
/// # Errors
///
/// Returns a safe application error when the expected run or runtime replay is invalid.
pub fn replay_collected_journal(
    expected_run_id: &Ulid,
    events: &[RunJournal],
) -> ApplicationResult<ReplayResult> {
    let result = replay(events).map_err(|error| map_runtime_error(&error))?;
    if result.run_id() != expected_run_id {
        return Err(map_runtime_error(&RuntimeError::RunIdentityConflict));
    }
    Ok(result)
}

async fn promote_snapshot_proof(
    blob_store: &dyn BlobStore,
    proof: StagedSnapshotProof,
) -> ApplicationResult<VerifiedSnapshotProof> {
    match proof.into_parts() {
        StagedSnapshotProofParts::Data { parquet, manifest } => {
            let parquet = promote_snapshot_blob(blob_store, parquet).await?;
            let manifest = promote_snapshot_blob(blob_store, *manifest).await?;
            VerifiedSnapshotProof::data(parquet, manifest)
        }
        StagedSnapshotProofParts::Universe { members_manifest } => {
            let members_manifest = promote_snapshot_blob(blob_store, members_manifest).await?;
            VerifiedSnapshotProof::universe(members_manifest)
        }
    }
}

async fn promote_snapshot_blob(
    blob_store: &dyn BlobStore,
    staged: StagedSnapshotBlob,
) -> ApplicationResult<VerifiedSnapshotBlob> {
    let verified = blob_store
        .verify_and_promote(staged.verification().clone())
        .await?;
    VerifiedSnapshotBlob::from_staged(staged, verified)
}

fn validate_artifact_verified(
    artifact: &Artifact,
    verified: &VerifiedBlobRef,
) -> ApplicationResult<()> {
    if artifact.content_hash() != verified.content_hash() {
        return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
    }
    if artifact.blob_size() != verified.size() {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::ValidationFailed,
            false,
        ));
    }
    Ok(())
}
