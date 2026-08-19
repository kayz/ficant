use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, AuthorizedPrincipal, BeginBlobStage, BlobStore,
    FoundationChangeContext, GovernedPublishSnapshot, IdempotencyKey, PublishSnapshot,
    SnapshotRepository, SnapshotValue, StagedBlobRef, VerifiedBlobRef, VerifyBlobStage,
};
use ficant_application::{
    DataHealthThresholdProfilePayload, PositionSnapshotPayload, PublishDataHealthThresholdProfile,
    PublishPositionSnapshot,
};
use ficant_domain::governance::{
    ChangeJustification, FoundationChangeOperation, PlatformRole, SourceDocumentRef,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    AccountingBook, AccountingClassification, AccountingClassificationState,
    DataHealthThresholdProfile, DataHealthThresholdProfileInput, Position, PositionHoldingForm,
    PositionInput, PositionSnapshot, PositionSnapshotInput,
};

#[derive(Default)]
struct RecordingBlobStore {
    begin: AtomicUsize,
    append: AtomicUsize,
    promote: AtomicUsize,
}

#[async_trait]
impl BlobStore for RecordingBlobStore {
    async fn begin_stage(&self, command: BeginBlobStage) -> ApplicationResult<StagedBlobRef> {
        self.begin.fetch_add(1, Ordering::SeqCst);
        Ok(StagedBlobRef::new(id('G'), command.owner().clone()))
    }

    async fn append_chunk(
        &self,
        scope: &AccessScope,
        staged: &StagedBlobRef,
        chunk: Vec<u8>,
    ) -> ApplicationResult<()> {
        staged.authorize(scope)?;
        assert!(!chunk.is_empty());
        self.append.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn verify_and_promote(
        &self,
        command: VerifyBlobStage,
    ) -> ApplicationResult<VerifiedBlobRef> {
        self.promote.fetch_add(1, Ordering::SeqCst);
        VerifiedBlobRef::new(command.expected_hash().clone(), command.expected_size())
    }

    async fn discard_stage(
        &self,
        _scope: &AccessScope,
        _staged: &StagedBlobRef,
    ) -> ApplicationResult<()> {
        Ok(())
    }
}

#[derive(Default)]
struct RecordingSnapshots {
    governed: AtomicUsize,
    legacy: AtomicUsize,
    changes: Mutex<Vec<(FoundationChangeOperation, String)>>,
}

#[async_trait]
impl SnapshotRepository for RecordingSnapshots {
    async fn publish_governed(
        &self,
        command: GovernedPublishSnapshot,
    ) -> ApplicationResult<SnapshotValue> {
        let change = command.change_record()?;
        self.governed.fetch_add(1, Ordering::SeqCst);
        self.changes
            .lock()
            .unwrap()
            .push((change.operation(), change.resource().canonical_ref()));
        Ok(command.command().snapshot().clone())
    }

    async fn publish_verified_manifest(
        &self,
        _command: PublishSnapshot,
    ) -> ApplicationResult<SnapshotValue> {
        self.legacy.fetch_add(1, Ordering::SeqCst);
        panic!("admin snapshot mutations must never call the legacy write port")
    }

    async fn get_by_id(
        &self,
        _scope: &AccessScope,
        _snapshot_id: Ulid,
    ) -> ApplicationResult<Option<SnapshotValue>> {
        Ok(None)
    }
}

#[tokio::test]
async fn admin_position_and_health_mutations_use_only_governed_repository_writes() {
    let blobs = Arc::new(RecordingBlobStore::default());
    let snapshots = Arc::new(RecordingSnapshots::default());
    let position = position_snapshot();
    let health = health_profile();

    let stored_position = PublishPositionSnapshot::new(blobs.as_ref(), snapshots.as_ref())
        .execute(
            context('R'),
            PositionSnapshotPayload::new(
                position.clone(),
                IdempotencyKey::new("position-r6a").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let stored_health = PublishDataHealthThresholdProfile::new(blobs.as_ref(), snapshots.as_ref())
        .execute(
            context('Q'),
            DataHealthThresholdProfilePayload::new(
                health.clone(),
                IdempotencyKey::new("health-r6a").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(stored_position, position);
    assert_eq!(stored_health, health);
    assert_eq!(snapshots.governed.load(Ordering::SeqCst), 2);
    assert_eq!(snapshots.legacy.load(Ordering::SeqCst), 0);
    assert_eq!(blobs.begin.load(Ordering::SeqCst), 2);
    assert_eq!(blobs.append.load(Ordering::SeqCst), 2);
    assert_eq!(blobs.promote.load(Ordering::SeqCst), 2);
    assert_eq!(
        snapshots.changes.lock().unwrap().as_slice(),
        [
            (
                FoundationChangeOperation::PublishPositionSnapshot,
                format!("position-snapshot:{}", position.id()),
            ),
            (
                FoundationChangeOperation::ConfigureDataHealthThreshold,
                format!(
                    "data-health-threshold-profile:{}@1",
                    health.profile_ref().id()
                ),
            ),
        ]
    );
}

fn context(record_suffix: char) -> FoundationChangeContext {
    let principal = AuthorizedPrincipal::new(
        "r6a-admin".to_owned(),
        id('A'),
        id('T'),
        vec![id('N')],
        PlatformRole::PlatformAdmin,
        vec![
            "data-health:configure".to_owned(),
            "positions:write".to_owned(),
        ],
        ContentHash::digest(b"credential-fingerprint"),
    )
    .unwrap();
    FoundationChangeContext::administrator(
        principal,
        ChangeJustification::new(
            "approved fixture change",
            vec![
                SourceDocumentRef::new("urn:ficant:test", ContentHash::digest(b"source")).unwrap(),
            ],
        )
        .unwrap(),
        id(record_suffix),
        time(0),
    )
    .unwrap()
}

fn position_snapshot() -> PositionSnapshot {
    let unit = UnitRef::new(id('M'), Version::new(1).unwrap());
    let decimal = |value| DecimalValue::new(value, 0, unit.clone()).unwrap();
    let position = Position::new(PositionInput {
        position_id: id('P'),
        instrument_ref: VersionRef::new(id('J'), Version::new(1).unwrap()),
        quantity: decimal("1"),
        economic_value: decimal("100"),
        economic_pnl: decimal("2"),
        accounting_pnl: decimal("1"),
        capital_requirement: decimal("3"),
        accounting_classification: AccountingClassification::new(
            AccountingClassificationState::Classified,
            Some(AccountingBook::Ac),
        )
        .unwrap(),
        holding_form: PositionHoldingForm::Owned,
    })
    .unwrap();
    let mut input = PositionSnapshotInput {
        snapshot_id: id('S'),
        owner: OwnerRef::new(id('T'), id('N')),
        subject_ref: VersionRef::new(id('V'), Version::new(1).unwrap()),
        observed_at: time(1),
        visible_at: time(1),
        content_hash: ContentHash::digest(b"pending"),
        lineage: vec![LineageRef::content_addressed(
            id('K'),
            ContentHash::digest(b"lineage"),
        )],
        positions: vec![position],
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    PositionSnapshot::new(input).unwrap()
}

fn health_profile() -> DataHealthThresholdProfile {
    let mut input = DataHealthThresholdProfileInput {
        profile_snapshot_id: id('H'),
        owner: OwnerRef::new(id('T'), id('N')),
        profile_ref: VersionRef::new(id('F'), Version::new(1).unwrap()),
        visible_at: time(0),
        effective_from: time(0),
        effective_to: time(2),
        max_position_snapshot_age_seconds: 100,
        unknown_accounting_warning_basis_points: 5_000,
        max_data_snapshot_age_seconds: 100,
        model_valuation_warning_basis_points: 5_000,
        content_hash: ContentHash::digest(b"pending"),
        lineage: Vec::new(),
    };
    input.content_hash = DataHealthThresholdProfile::content_hash_for(&input);
    DataHealthThresholdProfile::new(input).unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 8, 13, hour, 0, 0).unwrap(),
        "UTC",
        NaiveDate::from_ymd_opt(2026, 8, 13).unwrap(),
    )
    .unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
