use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use async_trait::async_trait;
use ficant_application::ports::{
    AppendDefinitionVersion, AppendJournalEvent, BlobStore, Clock, CreateExperimentRun,
    DefinitionIdentity, DefinitionRepository, DefinitionValue, IdGenerator, MarketFact,
    MarketFactRulePackResolver, MarketFactUnitResolver, MarketFactWindow,
    MarketRunRulePackResolver, Phase1AtomicWork, Phase1RunCandidateResolver, PublishArtifact,
    PublishSignalSet, PublishSnapshot, SnapshotBlobRole, SnapshotProofKind, SnapshotRepository,
    SnapshotValue, StagedBlobRef, StagedSnapshotBlob, StagedSnapshotProof, TransactionRunner,
    TransitionExperimentRun, ValidatedMarketFact, VerifiedBlobRef, VerifiedSnapshotBlob,
    VerifiedSnapshotProof, VerifyBlobStage,
};
use ficant_application::{
    AccessScope, ApplicationError, ApplicationErrorCategory, IdempotencyKey, PageRequest,
    Phase1BusinessInput, Phase1BusinessLoop, StagedArtifact, StagedSnapshot,
};
use ficant_domain::market::{
    FactSource, MarketRulePack, MarketRulePackTimesInput, Quote, QuoteInput, Unit, UnitInput,
    VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{
    Artifact, ArtifactKind, DataSnapshot, DataSnapshotInput, ExperimentRun, ExperimentRunInput,
    JournalEventType, RunJournal, RunJournalInput, RunState, SignalSet, SignalSetInput,
    UniverseSnapshot,
};
use ficant_domain::{ContentAddressed, Lineaged};

#[test]
fn r5_commands_reject_invalid_intent_and_derive_fingerprint_from_every_business_field() {
    assert_category(
        &VerifiedBlobRef::new(hash(1), 0).unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );

    let reversed = MarketFactWindow::new(
        version_ref('K', 1),
        time(4),
        time(3),
        time(5),
        PageRequest::new(scope(), None, 10).unwrap(),
    )
    .unwrap_err();
    assert_category(&reversed, ApplicationErrorCategory::ValidationFailed);

    let run_id = id('R');
    assert_category(
        &TransitionExperimentRun::new(
            scope(),
            owner(),
            run_id.clone(),
            0,
            RunState::Running,
            key("run-transition"),
        )
        .unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );

    let event = journal_event(id('R'), 1, JournalEventType::RunCreated, None, 'J');
    assert_category(
        &AppendJournalEvent::new(scope(), owner(), id('X'), 1, event, key("journal")).unwrap_err(),
        ApplicationErrorCategory::LineageIncomplete,
    );

    let invalid_v1 = DefinitionValue::Unit(unit('M', 2));
    assert_category(
        &AppendDefinitionVersion::new(None, invalid_v1, key("definition")).unwrap_err(),
        ApplicationErrorCategory::VersionConflict,
    );

    let original = TransitionExperimentRun::new(
        scope(),
        owner(),
        run_id.clone(),
        1,
        RunState::Running,
        key("same-key"),
    )
    .unwrap();
    let identical = TransitionExperimentRun::new(
        scope(),
        owner(),
        run_id.clone(),
        1,
        RunState::Running,
        key("same-key"),
    )
    .unwrap();
    assert_eq!(original.fingerprint(), identical.fingerprint());

    let changed_run = TransitionExperimentRun::new(
        scope(),
        owner(),
        id('S'),
        1,
        RunState::Running,
        key("same-key"),
    )
    .unwrap();
    let changed_revision = TransitionExperimentRun::new(
        scope(),
        owner(),
        run_id.clone(),
        2,
        RunState::Running,
        key("same-key"),
    )
    .unwrap();
    let changed_state = TransitionExperimentRun::new(
        scope(),
        owner(),
        run_id,
        1,
        RunState::Succeeded,
        key("same-key"),
    )
    .unwrap();
    assert_ne!(original.fingerprint(), changed_run.fingerprint());
    assert_ne!(original.fingerprint(), changed_revision.fingerprint());
    assert_ne!(original.fingerprint(), changed_state.fingerprint());
}

#[test]
fn d018_data_snapshot_rejects_single_blob_proof_without_manifest_candidate() {
    let fixture = fixture();
    let parquet_only = VerifyBlobStage::new(
        scope(),
        StagedBlobRef::new(id('V'), owner()),
        fixture.data_snapshot.content_hash().clone(),
        11,
    )
    .unwrap();

    let error = StagedSnapshot::new(fixture.data_snapshot.into(), parquet_only)
        .expect_err("DataSnapshot must not stage without a durable Manifest candidate");

    assert_category(&error, ApplicationErrorCategory::LineageIncomplete);
}

#[test]
fn d018_data_snapshot_carries_parquet_and_manifest_as_distinct_durable_proofs() {
    let fixture = fixture();
    let parquet_stage = StagedSnapshotBlob::new(
        SnapshotBlobRole::DataParquet,
        VerifyBlobStage::new(
            scope(),
            StagedBlobRef::new(id('V'), owner()),
            fixture.data_snapshot.content_hash().clone(),
            11,
        )
        .unwrap(),
    );
    let manifest_stage = StagedSnapshotBlob::new(
        SnapshotBlobRole::DataManifest,
        VerifyBlobStage::new(
            scope(),
            StagedBlobRef::new(id('W'), owner()),
            fixture.data_snapshot.manifest_hash().clone(),
            7,
        )
        .unwrap(),
    );
    let staged_proof =
        StagedSnapshotProof::data(parquet_stage.clone(), manifest_stage.clone()).unwrap();
    StagedSnapshot::from_proof(fixture.data_snapshot.clone().into(), staged_proof)
        .expect("DataSnapshot must stage both role-bound blob candidates");

    let parquet = VerifiedSnapshotBlob::from_staged(
        parquet_stage,
        VerifiedBlobRef::new(fixture.data_snapshot.content_hash().clone(), 11).unwrap(),
    )
    .unwrap();
    let manifest = VerifiedSnapshotBlob::from_staged(
        manifest_stage,
        VerifiedBlobRef::new(fixture.data_snapshot.manifest_hash().clone(), 7).unwrap(),
    )
    .unwrap();
    let command = PublishSnapshot::new(
        fixture.data_snapshot.clone().into(),
        VerifiedSnapshotProof::data(parquet, manifest).unwrap(),
        key("d018-data"),
    )
    .expect("DataSnapshot publish must retain both durable verified refs");

    assert_eq!(command.proof().kind(), SnapshotProofKind::Data);
    assert_eq!(
        command
            .proof()
            .get(SnapshotBlobRole::DataParquet)
            .unwrap()
            .verified_blob()
            .content_hash(),
        fixture.data_snapshot.content_hash()
    );
    assert_eq!(
        command
            .proof()
            .get(SnapshotBlobRole::DataManifest)
            .unwrap()
            .verified_blob()
            .content_hash(),
        fixture.data_snapshot.manifest_hash()
    );
}

#[test]
fn d018_snapshot_proofs_reject_role_and_identity_misuse() {
    let fixture = fixture();

    let wrong_roles = StagedSnapshotProof::data(
        staged_snapshot_blob(
            SnapshotBlobRole::DataManifest,
            'V',
            scope(),
            owner(),
            fixture.data_snapshot.content_hash().clone(),
            11,
        ),
        staged_snapshot_blob(
            SnapshotBlobRole::DataParquet,
            'J',
            scope(),
            owner(),
            fixture.data_snapshot.manifest_hash().clone(),
            7,
        ),
    )
    .unwrap_err();
    assert_category(&wrong_roles, ApplicationErrorCategory::ValidationFailed);

    let reused_identity = StagedSnapshotProof::data(
        staged_snapshot_blob(
            SnapshotBlobRole::DataParquet,
            'V',
            scope(),
            owner(),
            fixture.data_snapshot.content_hash().clone(),
            11,
        ),
        staged_snapshot_blob(
            SnapshotBlobRole::DataManifest,
            'V',
            scope(),
            owner(),
            fixture.data_snapshot.manifest_hash().clone(),
            7,
        ),
    )
    .unwrap_err();
    assert_category(&reused_identity, ApplicationErrorCategory::ValidationFailed);

    let reused_content_identity = StagedSnapshotProof::data(
        staged_snapshot_blob(
            SnapshotBlobRole::DataParquet,
            'V',
            scope(),
            owner(),
            fixture.data_snapshot.content_hash().clone(),
            11,
        ),
        staged_snapshot_blob(
            SnapshotBlobRole::DataManifest,
            'J',
            scope(),
            owner(),
            fixture.data_snapshot.content_hash().clone(),
            11,
        ),
    )
    .expect_err("Parquet and Manifest must not reuse one durable content identity");
    assert_category(
        &reused_content_identity,
        ApplicationErrorCategory::ValidationFailed,
    );
}

#[test]
fn d018_snapshot_proofs_reject_hash_authority_and_zero_size() {
    let fixture = fixture();

    let wrong_hash = StagedSnapshotProof::data(
        staged_snapshot_blob(
            SnapshotBlobRole::DataParquet,
            'V',
            scope(),
            owner(),
            hash(90),
            11,
        ),
        staged_snapshot_blob(
            SnapshotBlobRole::DataManifest,
            'J',
            scope(),
            owner(),
            fixture.data_snapshot.manifest_hash().clone(),
            7,
        ),
    )
    .unwrap();
    assert_category(
        &StagedSnapshot::from_proof(fixture.data_snapshot.clone().into(), wrong_hash).unwrap_err(),
        ApplicationErrorCategory::HashMismatch,
    );

    let same_tenant_other_owner = OwnerRef::new(id('T'), id('B'));
    let owner_scope = AccessScope::new(id('T'), id('A'), vec![id('Y'), id('B')]).unwrap();
    let wrong_owner = StagedSnapshotProof::data(
        staged_snapshot_blob(
            SnapshotBlobRole::DataParquet,
            'V',
            owner_scope.clone(),
            same_tenant_other_owner.clone(),
            fixture.data_snapshot.content_hash().clone(),
            11,
        ),
        staged_snapshot_blob(
            SnapshotBlobRole::DataManifest,
            'J',
            owner_scope,
            same_tenant_other_owner,
            fixture.data_snapshot.manifest_hash().clone(),
            7,
        ),
    )
    .unwrap();
    assert_category(
        &StagedSnapshot::from_proof(fixture.data_snapshot.clone().into(), wrong_owner).unwrap_err(),
        ApplicationErrorCategory::LineageIncomplete,
    );

    let foreign_owner = OwnerRef::new(id('K'), id('Y'));
    let foreign_scope = AccessScope::new(id('K'), id('A'), vec![id('Y')]).unwrap();
    let wrong_tenant = StagedSnapshotProof::data(
        staged_snapshot_blob(
            SnapshotBlobRole::DataParquet,
            'V',
            foreign_scope.clone(),
            foreign_owner.clone(),
            fixture.data_snapshot.content_hash().clone(),
            11,
        ),
        staged_snapshot_blob(
            SnapshotBlobRole::DataManifest,
            'J',
            foreign_scope,
            foreign_owner,
            fixture.data_snapshot.manifest_hash().clone(),
            7,
        ),
    )
    .unwrap();
    assert_category(
        &StagedSnapshot::from_proof(fixture.data_snapshot.clone().into(), wrong_tenant)
            .unwrap_err(),
        ApplicationErrorCategory::Forbidden,
    );

    assert_category(
        &VerifyBlobStage::new(
            scope(),
            StagedBlobRef::new(id('V'), owner()),
            fixture.data_snapshot.content_hash().clone(),
            0,
        )
        .unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );
}

#[test]
fn d018_snapshot_proofs_reject_cross_type_misuse() {
    let fixture = fixture();

    let data_proof = staged_data_proof(&fixture, scope(), owner());
    assert_category(
        &StagedSnapshot::from_proof(fixture.universe_snapshot.clone().into(), data_proof)
            .unwrap_err(),
        ApplicationErrorCategory::LineageIncomplete,
    );
    let universe_proof = StagedSnapshotProof::universe(staged_snapshot_blob(
        SnapshotBlobRole::UniverseMembersManifest,
        'W',
        scope(),
        owner(),
        fixture.universe_snapshot.content_hash().clone(),
        12,
    ))
    .unwrap();
    assert_category(
        &StagedSnapshot::from_proof(fixture.data_snapshot.clone().into(), universe_proof)
            .unwrap_err(),
        ApplicationErrorCategory::LineageIncomplete,
    );
}

#[test]
fn d018_publish_snapshot_fingerprint_covers_proof_scope_owner_and_idempotency() {
    let fixture = fixture();
    let original = publish_data_snapshot(
        fixture.data_snapshot.clone(),
        scope(),
        owner(),
        11,
        7,
        "d018-fingerprint",
    );
    let parquet_size = publish_data_snapshot(
        fixture.data_snapshot.clone(),
        scope(),
        owner(),
        12,
        7,
        "d018-fingerprint",
    );
    let manifest_size = publish_data_snapshot(
        fixture.data_snapshot.clone(),
        scope(),
        owner(),
        11,
        8,
        "d018-fingerprint",
    );
    let changed_scope = publish_data_snapshot(
        fixture.data_snapshot.clone(),
        AccessScope::new(id('T'), id('B'), vec![id('Y')]).unwrap(),
        owner(),
        11,
        7,
        "d018-fingerprint",
    );
    let changed_key = publish_data_snapshot(
        fixture.data_snapshot.clone(),
        scope(),
        owner(),
        11,
        7,
        "d018-fingerprint-other-key",
    );
    let changed_manifest_snapshot = data_snapshot_variant(
        &fixture.data_snapshot,
        fixture.data_snapshot.owner().clone(),
        hash(91),
        fixture.data_snapshot.content_hash().clone(),
    );
    let changed_manifest = publish_data_snapshot(
        changed_manifest_snapshot,
        scope(),
        owner(),
        11,
        7,
        "d018-fingerprint",
    );
    let changed_parquet_snapshot = data_snapshot_variant(
        &fixture.data_snapshot,
        fixture.data_snapshot.owner().clone(),
        fixture.data_snapshot.manifest_hash().clone(),
        hash(92),
    );
    let changed_parquet = publish_data_snapshot(
        changed_parquet_snapshot,
        scope(),
        owner(),
        11,
        7,
        "d018-fingerprint",
    );
    let changed_owner_value = OwnerRef::new(id('T'), id('B'));
    let changed_owner_snapshot = data_snapshot_variant(
        &fixture.data_snapshot,
        changed_owner_value.clone(),
        fixture.data_snapshot.manifest_hash().clone(),
        fixture.data_snapshot.content_hash().clone(),
    );
    let changed_owner = publish_data_snapshot(
        changed_owner_snapshot,
        AccessScope::new(id('T'), id('A'), vec![id('B')]).unwrap(),
        changed_owner_value,
        11,
        7,
        "d018-fingerprint",
    );
    let universe = publish_universe_snapshot(&fixture, "d018-fingerprint");

    for changed in [
        parquet_size,
        manifest_size,
        changed_scope,
        changed_key,
        changed_manifest,
        changed_parquet,
        changed_owner,
        universe,
    ] {
        assert_ne!(original.fingerprint(), changed.fingerprint());
    }
}

#[test]
fn r5_publish_intents_reject_verified_blob_hash_or_size_mismatch() {
    let fixture = fixture();

    let wrong_snapshot_blob = VerifiedBlobRef::new(hash(90), 11).unwrap();
    let wrong_parquet = VerifiedSnapshotBlob::from_staged(
        StagedSnapshotBlob::new(
            SnapshotBlobRole::DataParquet,
            VerifyBlobStage::new(scope(), StagedBlobRef::new(id('Z'), owner()), hash(90), 11)
                .unwrap(),
        ),
        wrong_snapshot_blob,
    )
    .unwrap();
    let manifest = VerifiedSnapshotBlob::from_staged(
        StagedSnapshotBlob::new(
            SnapshotBlobRole::DataManifest,
            VerifyBlobStage::new(
                scope(),
                StagedBlobRef::new(id('J'), owner()),
                fixture.data_snapshot.manifest_hash().clone(),
                7,
            )
            .unwrap(),
        ),
        VerifiedBlobRef::new(fixture.data_snapshot.manifest_hash().clone(), 7).unwrap(),
    )
    .unwrap();
    assert_category(
        &PublishSnapshot::new(
            fixture.data_snapshot.clone().into(),
            VerifiedSnapshotProof::data(wrong_parquet, manifest).unwrap(),
            key("snapshot"),
        )
        .unwrap_err(),
        ApplicationErrorCategory::HashMismatch,
    );

    let wrong_artifact_size =
        VerifiedBlobRef::new(fixture.artifact.content_hash().clone(), 99).unwrap();
    assert_category(
        &PublishArtifact::new(
            fixture.artifact.clone(),
            wrong_artifact_size,
            key("artifact"),
        )
        .unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );

    let wrong_signal_blob = VerifiedBlobRef::new(hash(91), fixture.artifact.blob_size()).unwrap();
    assert_category(
        &PublishSignalSet::new(fixture.signal_set.clone(), wrong_signal_blob, key("signal"))
            .unwrap_err(),
        ApplicationErrorCategory::HashMismatch,
    );

    let incomplete_lineage_artifact = Artifact::new(
        fixture.artifact.id().clone(),
        fixture.artifact.owner().clone(),
        fixture.artifact.kind(),
        fixture.artifact.media_type(),
        fixture.artifact.content_hash().clone(),
        fixture.artifact.blob_size(),
        vec![LineageRef::content_addressed(
            fixture.data_snapshot.id().clone(),
            fixture.data_snapshot.content_hash().clone(),
        )],
    )
    .unwrap();
    let verification = VerifyBlobStage::new(
        scope(),
        StagedBlobRef::new(id('Z'), owner()),
        fixture.artifact.content_hash().clone(),
        fixture.artifact.blob_size(),
    )
    .unwrap();
    assert_category(
        &StagedArtifact::new(
            incomplete_lineage_artifact,
            fixture.signal_set.clone(),
            verification,
        )
        .unwrap_err(),
        ApplicationErrorCategory::LineageIncomplete,
    );
}

#[test]
fn task7_phase1_accepts_independent_artifact_and_signal_roots() {
    let fixture = fixture();
    assert_ne!(fixture.artifact.id(), fixture.signal_set.id());
    assert_eq!(
        fixture.signal_set.artifact().object_id(),
        fixture.artifact.id()
    );
    assert_eq!(
        fixture.signal_set.artifact().content_hash(),
        Some(fixture.artifact.content_hash())
    );

    let clock = FixedClock::new(time(8));
    let ids = FixedIds::new(['A', 'B', 'C', 'D', 'E'].map(id).to_vec());
    let blobs = VerifyingBlobStore::default();
    let transactions = RecordingTransaction::default();
    let application = Phase1BusinessLoop::new(&clock, &ids, &blobs, &transactions);

    let result = block_on(application.execute(fixture.input())).unwrap();

    assert_eq!(result.terminal_state(), RunState::Succeeded);
    assert_eq!(transactions.calls.load(Ordering::SeqCst), 1);
    let committed = transactions.work.lock().unwrap().clone().unwrap();
    assert_eq!(committed.artifact().artifact().id(), fixture.artifact.id());
    assert_eq!(
        committed.signal().signal_set().id(),
        fixture.signal_set.id()
    );
    assert_ne!(
        committed.artifact().artifact().id(),
        committed.signal().signal_set().id()
    );
    assert_eq!(committed.run().run().revision(), 1);
    assert_eq!(committed.transitions()[0].expected_revision(), 1);
    assert_eq!(committed.transitions()[1].expected_revision(), 2);
    assert_eq!(committed.journal()[0].event().sequence(), 1);
    assert_eq!(committed.journal()[0].event().prev_hash(), None);
}

#[test]
fn task7_staged_artifact_rejects_identity_hash_owner_and_blob_drift_safely() {
    let fixture = fixture();
    let verification = |expected_hash, expected_size| {
        VerifyBlobStage::new(
            scope(),
            StagedBlobRef::new(id('Z'), owner()),
            expected_hash,
            expected_size,
        )
        .unwrap()
    };

    let wrong_artifact_identity = signal_from(
        &fixture,
        fixture.signal_set.id().clone(),
        fixture.signal_set.owner().clone(),
        LineageRef::content_addressed(id('J'), fixture.artifact.content_hash().clone()),
    );
    assert_category(
        &StagedArtifact::new(
            fixture.artifact.clone(),
            wrong_artifact_identity,
            verification(
                fixture.artifact.content_hash().clone(),
                fixture.artifact.blob_size(),
            ),
        )
        .unwrap_err(),
        ApplicationErrorCategory::LineageIncomplete,
    );

    let wrong_artifact_hash = signal_from(
        &fixture,
        fixture.signal_set.id().clone(),
        fixture.signal_set.owner().clone(),
        LineageRef::content_addressed(fixture.artifact.id().clone(), hash(88)),
    );
    assert_category(
        &StagedArtifact::new(
            fixture.artifact.clone(),
            wrong_artifact_hash,
            verification(
                fixture.artifact.content_hash().clone(),
                fixture.artifact.blob_size(),
            ),
        )
        .unwrap_err(),
        ApplicationErrorCategory::HashMismatch,
    );

    let wrong_owner = signal_from(
        &fixture,
        fixture.signal_set.id().clone(),
        OwnerRef::new(id('F'), id('Y')),
        fixture.signal_set.artifact().clone(),
    );
    assert_category(
        &StagedArtifact::new(
            fixture.artifact.clone(),
            wrong_owner,
            verification(
                fixture.artifact.content_hash().clone(),
                fixture.artifact.blob_size(),
            ),
        )
        .unwrap_err(),
        ApplicationErrorCategory::LineageIncomplete,
    );

    assert_category(
        &StagedArtifact::new(
            fixture.artifact.clone(),
            fixture.signal_set.clone(),
            verification(hash(89), fixture.artifact.blob_size()),
        )
        .unwrap_err(),
        ApplicationErrorCategory::HashMismatch,
    );
    assert_category(
        &StagedArtifact::new(
            fixture.artifact.clone(),
            fixture.signal_set.clone(),
            verification(
                fixture.artifact.content_hash().clone(),
                fixture.artifact.blob_size() + 1,
            ),
        )
        .unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );
}

#[test]
fn task7_signal_fingerprint_encodes_signal_and_artifact_identities_separately() {
    let fixture = fixture();
    let verified = VerifiedBlobRef::new(
        fixture.artifact.content_hash().clone(),
        fixture.artifact.blob_size(),
    )
    .unwrap();
    let original = PublishSignalSet::new(
        fixture.signal_set.clone(),
        verified.clone(),
        key("task7-fingerprint"),
    )
    .unwrap();
    let changed_signal_id = signal_from(
        &fixture,
        id('J'),
        fixture.signal_set.owner().clone(),
        fixture.signal_set.artifact().clone(),
    );
    let changed_artifact_id = signal_from(
        &fixture,
        fixture.signal_set.id().clone(),
        fixture.signal_set.owner().clone(),
        LineageRef::content_addressed(id('Z'), fixture.artifact.content_hash().clone()),
    );

    let changed_signal = PublishSignalSet::new(
        changed_signal_id,
        verified.clone(),
        key("task7-fingerprint"),
    )
    .unwrap();
    let changed_artifact =
        PublishSignalSet::new(changed_artifact_id, verified, key("task7-fingerprint")).unwrap();

    assert_ne!(original.fingerprint(), changed_signal.fingerprint());
    assert_ne!(original.fingerprint(), changed_artifact.fingerprint());
}

#[test]
fn r5_phase1_loop_consumes_clock_ids_blob_verification_and_one_atomic_transaction() {
    let fixture = fixture();
    let clock = FixedClock::new(time(8));
    let ids = FixedIds::new(['A', 'B', 'C', 'D', 'E'].map(id).to_vec());
    let blobs = VerifyingBlobStore::default();
    let transactions = RecordingTransaction::default();
    let application = Phase1BusinessLoop::new(&clock, &ids, &blobs, &transactions);

    let result = block_on(application.execute(fixture.input())).unwrap();

    assert_eq!(clock.calls.load(Ordering::SeqCst), 1);
    assert_eq!(ids.calls.load(Ordering::SeqCst), 5);
    assert_eq!(blobs.verify.load(Ordering::SeqCst), 4);
    assert_eq!(transactions.calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.run_id(), fixture.run.id());
    assert_eq!(result.terminal_state(), RunState::Succeeded);

    let committed = transactions.work.lock().unwrap().clone().unwrap();
    assert_eq!(committed.journal().len(), 5);
    let data_proof = committed
        .snapshots()
        .iter()
        .find(|command| command.proof().kind() == SnapshotProofKind::Data)
        .map(PublishSnapshot::proof)
        .expect("Phase 1 must commit one durable DataSnapshot proof");
    assert_eq!(
        data_proof
            .get(SnapshotBlobRole::DataParquet)
            .unwrap()
            .verified_blob()
            .content_hash(),
        fixture.data_snapshot.content_hash()
    );
    assert_eq!(
        data_proof
            .get(SnapshotBlobRole::DataManifest)
            .unwrap()
            .verified_blob()
            .content_hash(),
        fixture.data_snapshot.manifest_hash()
    );
    assert!(
        committed
            .snapshots()
            .iter()
            .any(|command| command.proof().kind() == SnapshotProofKind::Universe)
    );
    assert!(
        committed
            .idempotency_key()
            .as_str()
            .starts_with("phase1-request/scope-")
    );
    assert_eq!(committed.journal()[0].event().sequence(), 1);
    assert_eq!(committed.journal()[4].event().sequence(), 5);
    assert_eq!(
        committed.journal()[4].event().event_type(),
        JournalEventType::RunSucceeded
    );
    assert_eq!(committed.fingerprint(), result.fingerprint());
}

#[test]
fn r5_phase1_loop_rejects_blob_verification_drift_before_transaction() {
    let fixture = fixture();
    let clock = FixedClock::new(time(8));
    let ids = FixedIds::new(['A', 'B', 'C', 'D', 'E'].map(id).to_vec());
    let blobs = DriftingBlobStore;
    let transactions = RecordingTransaction::default();
    let application = Phase1BusinessLoop::new(&clock, &ids, &blobs, &transactions);

    let error = block_on(application.execute(fixture.input())).unwrap_err();

    assert_category(&error, ApplicationErrorCategory::HashMismatch);
    assert_eq!(transactions.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn d019_wrong_unit_stops_before_phase1_clock_ids_blobs_and_transaction() {
    let fixture = fixture();
    let clock = FixedClock::new(time(8));
    let ids = FixedIds::new(['A', 'B', 'C', 'D', 'E'].map(id).to_vec());
    let blobs = VerifyingBlobStore::default();
    let transactions = RecordingTransaction::default();
    let application = Phase1BusinessLoop::new(&clock, &ids, &blobs, &transactions);

    let error = block_on(async {
        let validated =
            MarketFactUnitResolver::new(&SingleQuoteUnitDefinitions { dimension: "rate" })
                .resolve(&scope(), fixture.fact.clone())
                .await?;
        let input = fixture.try_input_with_scope_and_fact(scope(), validated)?;
        application.execute(input).await
    })
    .unwrap_err();

    assert_category(&error, ApplicationErrorCategory::ValidationFailed);
    assert!(!error.retryable());
    assert_eq!(clock.calls.load(Ordering::SeqCst), 0);
    assert_eq!(ids.calls.load(Ordering::SeqCst), 0);
    assert_eq!(blobs.verify.load(Ordering::SeqCst), 0);
    assert_eq!(transactions.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn d021_candidate_coverage_failure_stops_before_all_mutating_phase1_ports() {
    let fixture = fixture();
    let definitions = OutOfRangeRuleDefinitions::default();
    let clock = FixedClock::new(time(8));
    let ids = FixedIds::new(['A', 'B', 'C', 'D', 'E'].map(id).to_vec());
    let blobs = VerifyingBlobStore::default();
    let transactions = RecordingTransaction::default();

    let error = block_on(Phase1RunCandidateResolver::new(&definitions).resolve(
        &scope(),
        fixture.run.clone(),
        &fixture.data_snapshot,
    ))
    .unwrap_err();

    assert_category(&error, ApplicationErrorCategory::ValidationFailed);
    assert_eq!(definitions.reads.load(Ordering::SeqCst), 1);
    assert_eq!(definitions.mutations.load(Ordering::SeqCst), 0);
    assert_eq!(clock.calls.load(Ordering::SeqCst), 0);
    assert_eq!(ids.calls.load(Ordering::SeqCst), 0);
    assert_eq!(blobs.begin.load(Ordering::SeqCst), 0);
    assert_eq!(blobs.append.load(Ordering::SeqCst), 0);
    assert_eq!(blobs.verify.load(Ordering::SeqCst), 0);
    assert_eq!(blobs.discard.load(Ordering::SeqCst), 0);
    assert_eq!(transactions.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn r7_business_input_rejects_mixed_owner_and_unrelated_fact() {
    let mut mixed_owner = fixture();
    mixed_owner.data_snapshot = DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: mixed_owner.data_snapshot.id().clone(),
        owner: OwnerRef::new(id('F'), id('Y')),
        visible_at: mixed_owner.data_snapshot.visible_at().clone(),
        as_of: mixed_owner.data_snapshot.as_of().clone(),
        schema_hash: mixed_owner.data_snapshot.schema_hash().clone(),
        manifest_hash: mixed_owner.data_snapshot.manifest_hash().clone(),
        blob_content_hash: mixed_owner.data_snapshot.content_hash().clone(),
        lineage: mixed_owner.data_snapshot.lineage().to_vec(),
    })
    .unwrap();
    assert_category(
        &mixed_owner.try_input().unwrap_err(),
        ApplicationErrorCategory::LineageIncomplete,
    );

    let mut unrelated = fixture();
    unrelated.fact = quote_fact('G', 2, owner());
    assert_category(
        &unrelated.try_input().unwrap_err(),
        ApplicationErrorCategory::LineageIncomplete,
    );
}

#[test]
fn r7_staged_artifact_rejects_non_signal_kind_and_owner_drift() {
    let fixture = fixture();
    let generic = Artifact::new(
        fixture.artifact.id().clone(),
        fixture.artifact.owner().clone(),
        ArtifactKind::Generic,
        fixture.artifact.media_type(),
        fixture.artifact.content_hash().clone(),
        fixture.artifact.blob_size(),
        fixture.artifact.lineage().to_vec(),
    )
    .unwrap();
    let verification = VerifyBlobStage::new(
        scope(),
        StagedBlobRef::new(id('Z'), owner()),
        fixture.artifact.content_hash().clone(),
        fixture.artifact.blob_size(),
    )
    .unwrap();
    assert_category(
        &StagedArtifact::new(generic, fixture.signal_set.clone(), verification).unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );
}

#[test]
fn r7_lineage_reorder_is_one_fingerprint_and_transition_scope_v2_matches_fcmd_v1_golden() {
    let fixture = fixture();
    let verified = VerifiedBlobRef::new(
        fixture.artifact.content_hash().clone(),
        fixture.artifact.blob_size(),
    )
    .unwrap();
    let original = PublishArtifact::new(
        fixture.artifact.clone(),
        verified.clone(),
        key("artifact-reorder"),
    )
    .unwrap();
    let mut reordered_lineage = fixture.artifact.lineage().to_vec();
    reordered_lineage.reverse();
    let reordered_artifact = Artifact::new(
        fixture.artifact.id().clone(),
        fixture.artifact.owner().clone(),
        fixture.artifact.kind(),
        fixture.artifact.media_type(),
        fixture.artifact.content_hash().clone(),
        fixture.artifact.blob_size(),
        reordered_lineage,
    )
    .unwrap();
    let reordered =
        PublishArtifact::new(reordered_artifact, verified, key("artifact-reorder")).unwrap();
    assert_eq!(original.fingerprint(), reordered.fingerprint());

    let transition = TransitionExperimentRun::new(
        scope(),
        owner(),
        id('R'),
        1,
        RunState::Running,
        key("golden-key-is-not-part-of-payload"),
    )
    .unwrap();
    assert_eq!(
        transition.fingerprint().content_hash().as_bytes(),
        &[
            0xcd, 0x45, 0x01, 0xa2, 0x7c, 0x7d, 0x8e, 0xc0, 0x3c, 0x14, 0x80, 0x0c, 0xc2, 0x8e,
            0xd4, 0x05, 0x30, 0x15, 0x19, 0x8a, 0xb8, 0x81, 0x70, 0xcf, 0xdd, 0xbc, 0x2b, 0x5b,
            0xb7, 0x24, 0x28, 0x9f,
        ]
    );
}

#[test]
fn r8_run_write_commands_do_not_collapse_scope_owner_or_idempotency_identity() {
    let fixture = fixture();
    let scope_a = scope();
    let scope_b = AccessScope::new(id('T'), id('B'), vec![id('Y')]).unwrap();
    let scope_foreign = AccessScope::new(id('K'), id('B'), vec![id('Z')]).unwrap();
    let owner_a = owner();
    let owner_b = OwnerRef::new(id('K'), id('Z'));
    let event = journal_event(id('R'), 1, JournalEventType::RunCreated, None, 'J');

    let create_a = CreateExperimentRun::new(
        scope_a.clone(),
        validated_run(&fixture, &scope_a),
        key("create"),
    )
    .unwrap();
    let create_b = CreateExperimentRun::new(
        scope_b.clone(),
        validated_run(&fixture, &scope_b),
        key("create"),
    )
    .unwrap();
    let transition_a = TransitionExperimentRun::new(
        scope_a.clone(),
        owner_a.clone(),
        fixture.run.id().clone(),
        1,
        RunState::Running,
        key("transition"),
    )
    .unwrap();
    let transition_b = TransitionExperimentRun::new(
        scope_foreign.clone(),
        owner_b.clone(),
        fixture.run.id().clone(),
        1,
        RunState::Running,
        key("transition"),
    )
    .unwrap();
    let journal_a = AppendJournalEvent::new(
        scope_a,
        owner_a,
        fixture.run.id().clone(),
        1,
        event.clone(),
        key("journal"),
    )
    .unwrap();
    let journal_b = AppendJournalEvent::new(
        scope_foreign,
        owner_b,
        fixture.run.id().clone(),
        1,
        event,
        key("journal"),
    )
    .unwrap();

    assert_ne!(create_a.fingerprint(), create_b.fingerprint());
    assert_ne!(create_a.idempotency_key(), create_b.idempotency_key());
    assert_ne!(transition_a.fingerprint(), transition_b.fingerprint());
    assert_ne!(
        transition_a.idempotency_key(),
        transition_b.idempotency_key()
    );
    assert_ne!(journal_a.fingerprint(), journal_b.fingerprint());
    assert_ne!(journal_a.idempotency_key(), journal_b.idempotency_key());
}

#[test]
fn r8_phase1_business_input_rejects_scope_that_cannot_authorize_owner() {
    let fixture = fixture();
    let wrong_scope = AccessScope::new(id('K'), id('B'), vec![id('Y')]).unwrap();

    assert_category(
        &fixture.try_input_with_scope(wrong_scope).unwrap_err(),
        ApplicationErrorCategory::Forbidden,
    );
}

#[derive(Clone)]
struct Fixture {
    fact: MarketFact,
    data_snapshot: DataSnapshot,
    universe_snapshot: UniverseSnapshot,
    run: ExperimentRun,
    artifact: Artifact,
    signal_set: SignalSet,
}

impl Fixture {
    fn input(&self) -> Phase1BusinessInput {
        self.try_input().unwrap()
    }

    fn try_input(&self) -> Result<Phase1BusinessInput, ApplicationError> {
        self.try_input_with_scope(scope())
    }

    fn try_input_with_scope(
        &self,
        access_scope: AccessScope,
    ) -> Result<Phase1BusinessInput, ApplicationError> {
        let validated_fact = block_on(
            MarketFactUnitResolver::new(&SingleQuoteUnitDefinitions { dimension: "price" })
                .resolve(&access_scope, self.fact.clone()),
        )?;
        self.try_input_with_scope_and_fact(access_scope, validated_fact)
    }

    fn try_input_with_scope_and_fact(
        &self,
        access_scope: AccessScope,
        validated_fact: ValidatedMarketFact,
    ) -> Result<Phase1BusinessInput, ApplicationError> {
        let definitions = SingleQuoteUnitDefinitions { dimension: "price" };
        let fully_validated_fact = block_on(
            MarketFactRulePackResolver::new(&definitions).resolve(&access_scope, validated_fact),
        )?;
        let validated_run = block_on(Phase1RunCandidateResolver::new(&definitions).resolve(
            &access_scope,
            self.run.clone(),
            &self.data_snapshot,
        ))?;
        let data_owner = self.data_snapshot.owner().clone();
        let data_scope = scope_for_owner(&data_owner);
        let data = StagedSnapshot::from_proof(
            self.data_snapshot.clone().into(),
            StagedSnapshotProof::data(
                StagedSnapshotBlob::new(
                    SnapshotBlobRole::DataParquet,
                    VerifyBlobStage::new(
                        data_scope.clone(),
                        StagedBlobRef::new(id('V'), data_owner.clone()),
                        self.data_snapshot.content_hash().clone(),
                        11,
                    )?,
                ),
                StagedSnapshotBlob::new(
                    SnapshotBlobRole::DataManifest,
                    VerifyBlobStage::new(
                        data_scope,
                        StagedBlobRef::new(id('J'), data_owner),
                        self.data_snapshot.manifest_hash().clone(),
                        7,
                    )?,
                ),
            )?,
        )?;
        let universe = StagedSnapshot::from_proof(
            self.universe_snapshot.clone().into(),
            StagedSnapshotProof::universe(StagedSnapshotBlob::new(
                SnapshotBlobRole::UniverseMembersManifest,
                VerifyBlobStage::new(
                    scope(),
                    StagedBlobRef::new(id('W'), owner()),
                    self.universe_snapshot.content_hash().clone(),
                    12,
                )?,
            ))?,
        )?;
        let artifact = StagedArtifact::new(
            self.artifact.clone(),
            self.signal_set.clone(),
            VerifyBlobStage::new(
                scope(),
                StagedBlobRef::new(id('X'), owner()),
                self.artifact.content_hash().clone(),
                self.artifact.blob_size(),
            )?,
        )?;
        Phase1BusinessInput::new(
            access_scope,
            fully_validated_fact,
            data,
            universe,
            validated_run,
            artifact,
            key("phase1-request"),
        )
    }
}

fn fixture() -> Fixture {
    let owner = owner();
    let instrument = version_ref('K', 1);
    let fact = quote_fact('Q', 1, owner.clone());
    let source_lineage = LineageRef::versioned(instrument.clone().id().clone(), version(1));
    let data_snapshot = DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: id('N'),
        owner: owner.clone(),
        visible_at: time(4),
        as_of: time(3),
        schema_hash: hash(10),
        manifest_hash: hash(11),
        blob_content_hash: hash(12),
        lineage: vec![source_lineage.clone(), fact.lineage_ref().unwrap()],
    })
    .unwrap();
    let universe_snapshot = UniverseSnapshot::new(
        id('H'),
        owner.clone(),
        vec![instrument],
        hash(13),
        hash(14),
        vec![source_lineage],
    )
    .unwrap();
    let data_ref = LineageRef::content_addressed(
        data_snapshot.id().clone(),
        data_snapshot.content_hash().clone(),
    );
    let universe_ref = LineageRef::content_addressed(
        universe_snapshot.id().clone(),
        universe_snapshot.content_hash().clone(),
    );
    let rule_pack = version_ref('P', 1);
    let run = ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: id('R'),
        owner: owner.clone(),
        data_snapshot: data_ref.clone(),
        universe_snapshot: universe_ref.clone(),
        rule_packs: vec![rule_pack.clone()],
        runtime_image_digest: hash(15),
        parameters_hash: hash(16),
        seed: 7,
    })
    .unwrap();
    let artifact = Artifact::new(
        id('S'),
        owner.clone(),
        ArtifactKind::SignalSet,
        "application/vnd.ficant.signal-set",
        hash(17),
        13,
        vec![
            data_ref.clone(),
            universe_ref.clone(),
            LineageRef::versioned(rule_pack.id().clone(), rule_pack.version()),
        ],
    )
    .unwrap();
    let signal_set = SignalSet::new(SignalSetInput {
        signal_set_id: id('G'),
        owner,
        artifact: LineageRef::content_addressed(
            artifact.id().clone(),
            artifact.content_hash().clone(),
        ),
        experiment_run_id: run.id().clone(),
        data_snapshot: data_ref.clone(),
        universe_snapshot: universe_ref,
        rule_packs: vec![rule_pack],
        input_artifacts: vec![data_ref],
        valid: EffectivePeriod::new(time(9), time(10)).unwrap(),
    })
    .unwrap();
    Fixture {
        fact,
        data_snapshot,
        universe_snapshot,
        run,
        artifact,
        signal_set,
    }
}

fn validated_run(
    fixture: &Fixture,
    access_scope: &AccessScope,
) -> ficant_application::ports::ValidatedExperimentRun {
    let definitions = SingleQuoteUnitDefinitions { dimension: "price" };
    let snapshots = SingleDataSnapshotRepository {
        snapshot: fixture.data_snapshot.clone(),
    };
    block_on(
        MarketRunRulePackResolver::new(&definitions, &snapshots)
            .resolve(access_scope, fixture.run.clone()),
    )
    .unwrap()
}

fn fixture_rule_pack() -> MarketRulePack {
    MarketRulePack::new_with_times(MarketRulePackTimesInput {
        rule_pack_id: id('P'),
        version: version(1),
        owner: owner(),
        market: "XSHG".to_owned(),
        rule_type: "research".to_owned(),
        source: "fixture".to_owned(),
        from: time(2),
        to: time(5),
        verification_status: VerificationStatus::Verified,
        content_hash: hash(31),
    })
    .unwrap()
}

fn signal_from(
    fixture: &Fixture,
    signal_set_id: Ulid,
    signal_owner: OwnerRef,
    artifact: LineageRef,
) -> SignalSet {
    SignalSet::new(SignalSetInput {
        signal_set_id,
        owner: signal_owner,
        artifact,
        experiment_run_id: fixture.signal_set.experiment_run_id().clone(),
        data_snapshot: fixture.signal_set.data_snapshot().clone(),
        universe_snapshot: fixture.signal_set.universe_snapshot().clone(),
        rule_packs: fixture.signal_set.rule_packs().to_vec(),
        input_artifacts: fixture.signal_set.input_artifacts().to_vec(),
        valid: fixture.signal_set.valid().clone(),
    })
    .unwrap()
}

fn staged_snapshot_blob(
    role: SnapshotBlobRole,
    staging_suffix: char,
    access_scope: AccessScope,
    stage_owner: OwnerRef,
    expected_hash: ContentHash,
    expected_size: u64,
) -> StagedSnapshotBlob {
    StagedSnapshotBlob::new(
        role,
        VerifyBlobStage::new(
            access_scope,
            StagedBlobRef::new(id(staging_suffix), stage_owner),
            expected_hash,
            expected_size,
        )
        .unwrap(),
    )
}

fn staged_data_proof(
    fixture: &Fixture,
    access_scope: AccessScope,
    stage_owner: OwnerRef,
) -> StagedSnapshotProof {
    StagedSnapshotProof::data(
        staged_snapshot_blob(
            SnapshotBlobRole::DataParquet,
            'V',
            access_scope.clone(),
            stage_owner.clone(),
            fixture.data_snapshot.content_hash().clone(),
            11,
        ),
        staged_snapshot_blob(
            SnapshotBlobRole::DataManifest,
            'J',
            access_scope,
            stage_owner,
            fixture.data_snapshot.manifest_hash().clone(),
            7,
        ),
    )
    .unwrap()
}

fn verified_snapshot_blob(
    role: SnapshotBlobRole,
    staging_suffix: char,
    access_scope: AccessScope,
    stage_owner: OwnerRef,
    content_hash: ContentHash,
    size: u64,
) -> VerifiedSnapshotBlob {
    let staged = staged_snapshot_blob(
        role,
        staging_suffix,
        access_scope,
        stage_owner,
        content_hash.clone(),
        size,
    );
    VerifiedSnapshotBlob::from_staged(staged, VerifiedBlobRef::new(content_hash, size).unwrap())
        .unwrap()
}

fn publish_data_snapshot(
    snapshot: DataSnapshot,
    access_scope: AccessScope,
    snapshot_owner: OwnerRef,
    parquet_size: u64,
    manifest_size: u64,
    idempotency_key: &str,
) -> PublishSnapshot {
    let parquet = verified_snapshot_blob(
        SnapshotBlobRole::DataParquet,
        'V',
        access_scope.clone(),
        snapshot_owner.clone(),
        snapshot.content_hash().clone(),
        parquet_size,
    );
    let manifest = verified_snapshot_blob(
        SnapshotBlobRole::DataManifest,
        'J',
        access_scope,
        snapshot_owner,
        snapshot.manifest_hash().clone(),
        manifest_size,
    );
    PublishSnapshot::new(
        snapshot.into(),
        VerifiedSnapshotProof::data(parquet, manifest).unwrap(),
        key(idempotency_key),
    )
    .unwrap()
}

fn publish_universe_snapshot(fixture: &Fixture, idempotency_key: &str) -> PublishSnapshot {
    let members_manifest = verified_snapshot_blob(
        SnapshotBlobRole::UniverseMembersManifest,
        'W',
        scope(),
        owner(),
        fixture.universe_snapshot.content_hash().clone(),
        12,
    );
    PublishSnapshot::new(
        fixture.universe_snapshot.clone().into(),
        VerifiedSnapshotProof::universe(members_manifest).unwrap(),
        key(idempotency_key),
    )
    .unwrap()
}

fn data_snapshot_variant(
    source: &DataSnapshot,
    snapshot_owner: OwnerRef,
    manifest_hash: ContentHash,
    blob_content_hash: ContentHash,
) -> DataSnapshot {
    DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: source.id().clone(),
        owner: snapshot_owner,
        visible_at: source.visible_at().clone(),
        as_of: source.as_of().clone(),
        schema_hash: source.schema_hash().clone(),
        manifest_hash,
        blob_content_hash,
        lineage: source.lineage().to_vec(),
    })
    .unwrap()
}

fn quote_fact(fact_suffix: char, source_revision: u64, owner: OwnerRef) -> MarketFact {
    let source =
        FactSource::new("fixture", format!("quote-{fact_suffix}"), source_revision).unwrap();
    let price = DecimalValue::new("10125", 2, UnitRef::new(id('M'), version(1))).unwrap();
    MarketFact::Quote(
        Quote::new(QuoteInput {
            quote_id: id(fact_suffix),
            instrument: version_ref('K', 1),
            owner,
            source,
            observed_at: time(1),
            received_at: time(2),
            bid: Some(price),
            ask: None,
            supersedes_id: None,
        })
        .unwrap(),
    )
}

struct SingleQuoteUnitDefinitions {
    dimension: &'static str,
}

#[async_trait]
impl DefinitionRepository for SingleQuoteUnitDefinitions {
    async fn create_identity(&self, _identity: DefinitionIdentity) -> Result<(), ApplicationError> {
        Err(not_used())
    }

    async fn append_version(
        &self,
        _command: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        Err(not_used())
    }

    async fn get_version(
        &self,
        _scope: &AccessScope,
        definition_id: Ulid,
        requested_version: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        if definition_id == id('P') && requested_version == version(1) {
            return Ok(Some(DefinitionValue::MarketRulePack(fixture_rule_pack())));
        }
        if definition_id != id('M') || requested_version != version(1) {
            return Ok(None);
        }
        Ok(Some(DefinitionValue::Unit(
            Unit::new(UnitInput {
                unit_id: id('M'),
                version: version(1),
                owner: owner(),
                code: "PRICE".to_owned(),
                dimension: self.dimension.to_owned(),
                scale: 2,
                precision: 18,
            })
            .unwrap(),
        )))
    }

    async fn resolve_as_of(
        &self,
        _scope: &AccessScope,
        _definition_id: Ulid,
        _instant: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Err(not_used())
    }
}

struct SingleDataSnapshotRepository {
    snapshot: DataSnapshot,
}

#[derive(Default)]
struct OutOfRangeRuleDefinitions {
    reads: AtomicUsize,
    mutations: AtomicUsize,
}

#[async_trait]
impl DefinitionRepository for OutOfRangeRuleDefinitions {
    async fn create_identity(&self, _identity: DefinitionIdentity) -> Result<(), ApplicationError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }

    async fn append_version(
        &self,
        _command: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }

    async fn get_version(
        &self,
        _scope: &AccessScope,
        definition_id: Ulid,
        requested_version: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if definition_id != id('P') || requested_version != version(1) {
            return Ok(None);
        }
        Ok(Some(DefinitionValue::MarketRulePack(
            MarketRulePack::new_with_times(MarketRulePackTimesInput {
                rule_pack_id: id('P'),
                version: version(1),
                owner: owner(),
                market: "XSHG".to_owned(),
                rule_type: "research".to_owned(),
                source: "fixture".to_owned(),
                from: time(2),
                to: time(3),
                verification_status: VerificationStatus::Verified,
                content_hash: hash(41),
            })
            .unwrap(),
        )))
    }

    async fn resolve_as_of(
        &self,
        _scope: &AccessScope,
        _definition_id: Ulid,
        _instant: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Err(not_used())
    }
}

#[async_trait]
impl SnapshotRepository for SingleDataSnapshotRepository {
    async fn publish_verified_manifest(
        &self,
        _command: PublishSnapshot,
    ) -> Result<SnapshotValue, ApplicationError> {
        Err(not_used())
    }

    async fn get_by_id(
        &self,
        _scope: &AccessScope,
        snapshot_id: Ulid,
    ) -> Result<Option<SnapshotValue>, ApplicationError> {
        if snapshot_id == *self.snapshot.id() {
            Ok(Some(self.snapshot.clone().into()))
        } else {
            Ok(None)
        }
    }
}

struct FixedClock {
    now: MarketTime,
    calls: AtomicUsize,
}

impl FixedClock {
    fn new(now: MarketTime) -> Self {
        Self {
            now,
            calls: AtomicUsize::new(0),
        }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Result<MarketTime, ApplicationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.now.clone())
    }
}

struct FixedIds {
    values: Mutex<Vec<Ulid>>,
    calls: AtomicUsize,
}

impl FixedIds {
    fn new(mut values: Vec<Ulid>) -> Self {
        values.reverse();
        Self {
            values: Mutex::new(values),
            calls: AtomicUsize::new(0),
        }
    }
}

impl IdGenerator for FixedIds {
    fn next_id(&self) -> Result<Ulid, ApplicationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.values.lock().unwrap().pop().ok_or_else(|| {
            ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, false)
        })
    }
}

#[derive(Default)]
struct VerifyingBlobStore {
    begin: AtomicUsize,
    append: AtomicUsize,
    verify: AtomicUsize,
    discard: AtomicUsize,
}

#[async_trait]
impl BlobStore for VerifyingBlobStore {
    async fn begin_stage(
        &self,
        _command: ficant_application::ports::BeginBlobStage,
    ) -> Result<StagedBlobRef, ApplicationError> {
        self.begin.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }

    async fn append_chunk(
        &self,
        _scope: &AccessScope,
        _staged: &StagedBlobRef,
        _chunk: Vec<u8>,
    ) -> Result<(), ApplicationError> {
        self.append.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }

    async fn verify_and_promote(
        &self,
        command: VerifyBlobStage,
    ) -> Result<VerifiedBlobRef, ApplicationError> {
        self.verify.fetch_add(1, Ordering::SeqCst);
        VerifiedBlobRef::new(command.expected_hash().clone(), command.expected_size())
    }

    async fn discard_stage(
        &self,
        _scope: &AccessScope,
        _staged: &StagedBlobRef,
    ) -> Result<(), ApplicationError> {
        self.discard.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }
}

struct DriftingBlobStore;

#[async_trait]
impl BlobStore for DriftingBlobStore {
    async fn begin_stage(
        &self,
        _command: ficant_application::ports::BeginBlobStage,
    ) -> Result<StagedBlobRef, ApplicationError> {
        Err(not_used())
    }

    async fn append_chunk(
        &self,
        _scope: &AccessScope,
        _staged: &StagedBlobRef,
        _chunk: Vec<u8>,
    ) -> Result<(), ApplicationError> {
        Err(not_used())
    }

    async fn verify_and_promote(
        &self,
        command: VerifyBlobStage,
    ) -> Result<VerifiedBlobRef, ApplicationError> {
        VerifiedBlobRef::new(hash(99), command.expected_size())
    }

    async fn discard_stage(
        &self,
        _scope: &AccessScope,
        _staged: &StagedBlobRef,
    ) -> Result<(), ApplicationError> {
        Err(not_used())
    }
}

#[derive(Default)]
struct RecordingTransaction {
    calls: AtomicUsize,
    work: Mutex<Option<Phase1AtomicWork>>,
}

#[async_trait]
impl TransactionRunner for RecordingTransaction {
    async fn commit_phase1(&self, work: &Phase1AtomicWork) -> Result<(), ApplicationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.work.lock().unwrap() = Some(work.clone());
        Ok(())
    }
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    struct ThreadWake(std::thread::Thread);
    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

fn journal_event(
    run_id: Ulid,
    sequence: u64,
    event_type: JournalEventType,
    previous: Option<ContentHash>,
    event_suffix: char,
) -> RunJournal {
    let input = RunJournalInput {
        journal_event_id: id(event_suffix),
        run_id,
        sequence,
        event_type,
        occurred_at: time(8),
        payload_type: "ficant.research.v1.Phase1Event".to_owned(),
        payload_schema: "v1".to_owned(),
        payload: vec![event_suffix as u8],
        prev_hash: previous,
    };
    let claimed = input.canonical_hash().unwrap();
    RunJournal::new(input, &claimed).unwrap()
}

fn unit(suffix: char, value: u64) -> Unit {
    Unit::new(UnitInput {
        unit_id: id(suffix),
        version: version(value),
        owner: owner(),
        code: "USD".to_owned(),
        dimension: "currency".to_owned(),
        scale: 2,
        precision: 18,
    })
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('Y'))
}

fn scope_for_owner(owner: &OwnerRef) -> AccessScope {
    AccessScope::new(
        owner.tenant_id().clone(),
        id('A'),
        vec![owner.owner_id().clone()],
    )
    .unwrap()
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('A'), vec![id('Y')]).unwrap()
}

fn version_ref(suffix: char, value: u64) -> VersionRef {
    VersionRef::new(id(suffix), version(value))
}

fn version(value: u64) -> Version {
    Version::new(value).unwrap()
}

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).unwrap()
}

fn hash(byte: u8) -> ContentHash {
    ContentHash::from_bytes(&[byte; 32]).unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-04".parse().unwrap(),
    )
    .unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn not_used() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}

fn assert_category(error: &ApplicationError, expected: ApplicationErrorCategory) {
    assert_eq!(error.category(), expected);
}
