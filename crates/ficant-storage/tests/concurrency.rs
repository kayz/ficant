mod support;

use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    AppendDefinitionVersion, AppendJournalEvent, BeginBlobStage, BlobStore, CreateExperimentRun,
    DefinitionIdentity, DefinitionKind, DefinitionRepository, DefinitionValue,
    ExperimentRepository, IdempotencyKey, MarketRunRulePackResolver, PublishSnapshot,
    RunJournalRepository, SnapshotBlobRole, SnapshotValue, StagedSnapshotBlob,
    TransitionExperimentRun, VerifiedSnapshotBlob, VerifiedSnapshotProof, VerifyBlobStage,
};
use ficant_domain::market::{
    MarketRulePack, MarketRulePackInput, Unit, UnitInput, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_domain::research::{
    DataSnapshot, DataSnapshotInput, ExperimentRun, ExperimentRunInput, JournalEventType,
    RunJournal, RunJournalInput, RunState,
};
use ficant_storage::minio::MinioBlobStore;
use sqlx::PgPool;
use sqlx::types::chrono::{NaiveDate, TimeZone, Utc};

#[tokio::test]
async fn concurrent_definition_version_appends_have_exactly_one_winner() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;

    let repository = support::repository(pool.clone());
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let unit_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F03").unwrap();
    repository
        .create_identity(DefinitionIdentity::new(
            unit_id.clone(),
            owner.clone(),
            DefinitionKind::Unit,
            IdempotencyKey::new("concurrency:unit:create").unwrap(),
        ))
        .await
        .unwrap();
    repository
        .append_version(
            AppendDefinitionVersion::new(
                None,
                DefinitionValue::Unit(unit(&unit_id, &owner, 1, "CNY")),
                IdempotencyKey::new("concurrency:unit:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let append = |code: &'static str, key: &'static str| {
        repository.append_version(
            AppendDefinitionVersion::new(
                Some(Version::new(1).unwrap()),
                DefinitionValue::Unit(unit(&unit_id, &owner, 2, code)),
                IdempotencyKey::new(key).unwrap(),
            )
            .unwrap(),
        )
    };
    let (left, right) = tokio::join!(
        append("CNY-LEFT", "concurrency:unit:v2:left"),
        append("CNY-RIGHT", "concurrency:unit:v2:right")
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let loser = left.err().or_else(|| right.err()).unwrap();
    assert_eq!(loser.category(), ApplicationErrorCategory::VersionConflict);
    assert!(loser.retryable());

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM core.definition_identities
         WHERE tenant_id = $1 AND definition_id = $2 AND latest_version = 2",
    )
    .bind("01ARZ3NDEKTSV4RRFFQ69G5F01")
    .bind("01ARZ3NDEKTSV4RRFFQ69G5F03")
    .fetch_one(&pool)
    .await
    .expect("identity count must be observable");
    assert_eq!(count, 1);
}

fn unit(unit_id: &Ulid, owner: &OwnerRef, version: u64, code: &str) -> Unit {
    Unit::new(UnitInput {
        unit_id: unit_id.clone(),
        version: Version::new(version).unwrap(),
        owner: owner.clone(),
        code: code.to_owned(),
        dimension: "currency".to_owned(),
        scale: 2,
        precision: 18,
    })
    .unwrap()
}

#[tokio::test]
async fn concurrent_different_intents_with_one_idempotency_key_have_one_winner() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let lineage_owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let lineage_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F31").unwrap();
    repository
        .create_identity(DefinitionIdentity::new(
            lineage_id.clone(),
            lineage_owner.clone(),
            DefinitionKind::Unit,
            IdempotencyKey::new("concurrency:snapshot:lineage:identity").unwrap(),
        ))
        .await
        .unwrap();
    repository
        .append_version(
            AppendDefinitionVersion::new(
                None,
                DefinitionValue::Unit(unit(&lineage_id, &lineage_owner, 1, "SNAPSHOT-SOURCE")),
                IdempotencyKey::new("concurrency:snapshot:lineage:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let left = snapshot_command(
        &pool,
        "01ARZ3NDEKTSV4RRFFQ69G5F40",
        b"left snapshot content",
    )
    .await;
    let right = snapshot_command(
        &pool,
        "01ARZ3NDEKTSV4RRFFQ69G5F41",
        b"right snapshot content",
    )
    .await;

    let (left, right) = tokio::join!(
        repository.publish_verified_manifest(left),
        repository.publish_verified_manifest(right),
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let loser = left.err().or_else(|| right.err()).unwrap();
    assert_eq!(loser.category(), ApplicationErrorCategory::AlreadyExists);
    assert!(!loser.retryable());

    let persisted: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM core.idempotency_records
              WHERE scope = 'snapshot:publish:v2' AND idempotency_key = 'snapshot:concurrent:v1'),
             (SELECT COUNT(*) FROM research.data_snapshots),
             (SELECT COUNT(*) FROM storage.blobs)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, (1, 1, 2));
}

#[tokio::test]
// One shared run makes the revision, sequence, and foreign-owner postconditions comparable.
#[allow(clippy::too_many_lines)]
async fn run_and_journal_cas_have_one_winner_and_foreign_writes_leave_no_side_effects() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let scope = support::access_scope(&owner);
    let run = seed_run_lineage(&repository, &pool, &owner).await;
    let validated = MarketRunRulePackResolver::new(&repository, &repository)
        .resolve(&scope, run.clone())
        .await
        .unwrap();
    repository
        .create_run(
            CreateExperimentRun::new(
                scope.clone(),
                validated,
                IdempotencyKey::new("concurrency:run:create").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let transition = |key: &'static str| {
        repository.transition(
            TransitionExperimentRun::new(
                scope.clone(),
                owner.clone(),
                run.id().clone(),
                1,
                RunState::Running,
                IdempotencyKey::new(key).unwrap(),
            )
            .unwrap(),
        )
    };
    let (left, right) = tokio::join!(
        transition("concurrency:run:left"),
        transition("concurrency:run:right")
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert_eq!(
        left.as_ref()
            .err()
            .or_else(|| right.as_ref().err())
            .unwrap()
            .category(),
        ApplicationErrorCategory::VersionConflict
    );

    let first_event = journal_event(run.id().clone(), "01ARZ3NDEKTSV4RRFFQ69G5F65", b"left");
    let second_event = journal_event(run.id().clone(), "01ARZ3NDEKTSV4RRFFQ69G5F66", b"right");
    let append = |event: RunJournal, key: &'static str| {
        repository.append(
            AppendJournalEvent::new(
                scope.clone(),
                owner.clone(),
                run.id().clone(),
                1,
                event,
                IdempotencyKey::new(key).unwrap(),
            )
            .unwrap(),
        )
    };
    let (left, right) = tokio::join!(
        append(first_event, "concurrency:journal:left"),
        append(second_event, "concurrency:journal:right")
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert_eq!(
        left.as_ref()
            .err()
            .or_else(|| right.as_ref().err())
            .unwrap()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    let foreign_owner = OwnerRef::new(
        owner.tenant_id().clone(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F09").unwrap(),
    );
    let foreign_scope = support::access_scope(&foreign_owner);
    let transition_error = repository
        .transition(
            TransitionExperimentRun::new(
                foreign_scope.clone(),
                foreign_owner.clone(),
                run.id().clone(),
                2,
                RunState::Succeeded,
                IdempotencyKey::new("concurrency:run:foreign").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        transition_error.category(),
        ApplicationErrorCategory::Forbidden
    );
    let journal_error = repository
        .append(
            AppendJournalEvent::new(
                foreign_scope,
                foreign_owner,
                run.id().clone(),
                1,
                journal_event(run.id().clone(), "01ARZ3NDEKTSV4RRFFQ69G5F67", b"foreign"),
                IdempotencyKey::new("concurrency:journal:foreign").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        journal_error.category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    let persisted: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT revision FROM research.experiment_runs WHERE experiment_run_id = $1),
             (SELECT COUNT(*) FROM research.experiment_run_revisions WHERE experiment_run_id = $1),
             (SELECT COUNT(*) FROM research.run_journal WHERE run_id = $1),
             (SELECT next_sequence FROM research.run_journal_sequences WHERE run_id = $1),
             (SELECT COUNT(*) FROM core.idempotency_records
               WHERE scope IN ('experiment-run:transition:v2', 'run-journal:append:v2'))",
    )
    .bind(run.id().as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, (2, 2, 1, 2, 2));
}

fn experiment_run(owner: OwnerRef) -> ExperimentRun {
    ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F60").unwrap(),
        owner,
        data_snapshot: LineageRef::content_addressed(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F61").unwrap(),
            ContentHash::digest(b"concurrency run data"),
        ),
        universe_snapshot: LineageRef::versioned(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F62").unwrap(),
            Version::new(1).unwrap(),
        ),
        rule_packs: vec![VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F63").unwrap(),
            Version::new(1).unwrap(),
        )],
        runtime_image_digest: ContentHash::digest(b"runtime-image"),
        parameters_hash: ContentHash::digest(b"parameters"),
        seed: 42,
    })
    .unwrap()
}

async fn seed_run_lineage(
    repository: &ficant_storage::postgres::PostgresRepository,
    pool: &PgPool,
    owner: &OwnerRef,
) -> ExperimentRun {
    for (id, code) in [
        ("01ARZ3NDEKTSV4RRFFQ69G5F31", "RUN-DATA-SOURCE"),
        ("01ARZ3NDEKTSV4RRFFQ69G5F62", "RUN-UNIVERSE"),
    ] {
        let unit_id = Ulid::new(id).unwrap();
        repository
            .create_identity(DefinitionIdentity::new(
                unit_id.clone(),
                owner.clone(),
                DefinitionKind::Unit,
                IdempotencyKey::new(format!("concurrency:{code}:identity")).unwrap(),
            ))
            .await
            .unwrap();
        repository
            .append_version(
                AppendDefinitionVersion::new(
                    None,
                    DefinitionValue::Unit(unit(&unit_id, owner, 1, code)),
                    IdempotencyKey::new(format!("concurrency:{code}:v1")).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let rule_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F63").unwrap();
    repository
        .create_identity(DefinitionIdentity::new(
            rule_id.clone(),
            owner.clone(),
            DefinitionKind::MarketRulePack,
            IdempotencyKey::new("concurrency:run-rule:identity").unwrap(),
        ))
        .await
        .unwrap();
    repository
        .append_version(
            AppendDefinitionVersion::new(
                None,
                DefinitionValue::MarketRulePack(
                    MarketRulePack::new(MarketRulePackInput {
                        rule_pack_id: rule_id,
                        version: Version::new(1).unwrap(),
                        owner: owner.clone(),
                        market: "XSHG".to_owned(),
                        rule_type: "CONCURRENCY".to_owned(),
                        source: "storage-fixture".to_owned(),
                        effective: EffectivePeriod::new(run_market_time(1), run_market_time(15))
                            .unwrap(),
                        verification_status: VerificationStatus::Verified,
                        content_hash: ContentHash::digest(b"concurrency-run-rule"),
                    })
                    .unwrap(),
                ),
                IdempotencyKey::new("concurrency:run-rule:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    repository
        .publish_verified_manifest(
            snapshot_command(pool, "01ARZ3NDEKTSV4RRFFQ69G5F61", b"concurrency run data").await,
        )
        .await
        .unwrap();
    experiment_run(owner.clone())
}

fn run_market_time(hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2025, 1, 15, hour, 0, 0).unwrap(),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
    )
    .unwrap()
}

fn journal_event(run_id: Ulid, event_id: &str, payload: &[u8]) -> RunJournal {
    let input = RunJournalInput {
        journal_event_id: Ulid::new(event_id).unwrap(),
        run_id,
        sequence: 1,
        event_type: JournalEventType::RunCreated,
        occurred_at: MarketTime::new(
            Utc.with_ymd_and_hms(2025, 1, 15, 8, 0, 0).unwrap(),
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
        )
        .unwrap(),
        payload_type: "ficant.research.v1.RunCreated".to_owned(),
        payload_schema: "v1".to_owned(),
        payload: payload.to_vec(),
        prev_hash: None,
    };
    let claimed = input.canonical_hash().unwrap();
    RunJournal::new(input, &claimed).unwrap()
}

#[allow(clippy::too_many_lines)]
async fn snapshot_command(pool: &PgPool, snapshot_id: &str, bytes: &[u8]) -> PublishSnapshot {
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let market_time = |hour| {
        MarketTime::new(
            Utc.with_ymd_and_hms(2025, 1, 15, hour, 0, 0).unwrap(),
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
        )
        .unwrap()
    };
    let content_hash = ContentHash::digest(bytes);
    let manifest_bytes = format!("manifest:{snapshot_id}").into_bytes();
    let manifest_hash = ContentHash::digest(&manifest_bytes);
    let scope = support::access_scope(&owner);
    let snapshot = SnapshotValue::Data(
        DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: Ulid::new(snapshot_id).unwrap(),
            owner: owner.clone(),
            visible_at: market_time(8),
            as_of: market_time(7),
            schema_hash: ContentHash::digest(b"schema-v1"),
            manifest_hash: manifest_hash.clone(),
            blob_content_hash: content_hash.clone(),
            lineage: vec![LineageRef::versioned(
                Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F31").unwrap(),
                Version::new(1).unwrap(),
            )],
        })
        .unwrap(),
    );
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let store =
        MinioBlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool.clone()).unwrap();
    let stage_key = format!("snapshot:concurrent:blob:{snapshot_id}");
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner.clone(),
                u64::try_from(bytes.len()).unwrap(),
                IdempotencyKey::new(stage_key).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(&scope, &staged, bytes.to_vec())
        .await
        .unwrap();
    let verification = VerifyBlobStage::new(
        scope.clone(),
        staged,
        content_hash,
        u64::try_from(bytes.len()).unwrap(),
    )
    .unwrap();
    let parquet = StagedSnapshotBlob::new(SnapshotBlobRole::DataParquet, verification.clone());
    let verified = store.verify_and_promote(verification).await.unwrap();
    let manifest_staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner,
                u64::try_from(manifest_bytes.len()).unwrap(),
                IdempotencyKey::new(format!("snapshot:concurrent:manifest:{snapshot_id}")).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(&scope, &manifest_staged, manifest_bytes.clone())
        .await
        .unwrap();
    let manifest_verification = VerifyBlobStage::new(
        scope,
        manifest_staged,
        manifest_hash,
        u64::try_from(manifest_bytes.len()).unwrap(),
    )
    .unwrap();
    let manifest = StagedSnapshotBlob::new(
        SnapshotBlobRole::DataManifest,
        manifest_verification.clone(),
    );
    let manifest_verified = store
        .verify_and_promote(manifest_verification)
        .await
        .unwrap();
    PublishSnapshot::new(
        snapshot,
        VerifiedSnapshotProof::data(
            VerifiedSnapshotBlob::from_staged(parquet, verified.clone()).unwrap(),
            VerifiedSnapshotBlob::from_staged(manifest, manifest_verified).unwrap(),
        )
        .unwrap(),
        IdempotencyKey::new("snapshot:concurrent:v1").unwrap(),
    )
    .unwrap()
}
