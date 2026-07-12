mod support;

use ficant_application::ports::{
    AppendDefinitionVersion, AppendJournalEvent, AppendMarketFact, ArtifactRepository,
    BeginBlobStage, BlobStore, CreateExperimentRun, DefinitionIdentity, DefinitionKind,
    DefinitionRepository, DefinitionValue, ExperimentRepository, IdempotencyKey,
    InstrumentDefinition, MarketFact, MarketFactFieldRole, MarketFactRepository,
    MarketFactRulePackResolver, MarketFactUnitResolver, MarketFactWindow,
    MarketRunRulePackResolver, PageRequest, PublishArtifact, PublishCurveSnapshot,
    PublishSignalSet, PublishSnapshot, RunJournalRepository, SignalRepository, SnapshotBlobRole,
    SnapshotValue, TransitionExperimentRun, VerifiedBlobRef, VerifiedSnapshotBlob,
    VerifiedSnapshotProof, VerifyBlobStage,
};
use ficant_domain::ContentAddressed;
use ficant_domain::market::{
    ArtifactInputKind, Calendar, CalendarInput, CurveSnapshot, CurveSnapshotInput, FactSource,
    Instrument, InstrumentInput, InstrumentKind, MarketRulePack, MarketRulePackInput, Quote,
    QuoteInput, Unit, UnitInput, Valuation, ValuationInput, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{
    Artifact, ArtifactKind, DataSnapshot, DataSnapshotInput, ExperimentRun, ExperimentRunInput,
    JournalEventType, RunJournal, RunJournalInput, RunState, SignalSet, SignalSetInput,
};
use ficant_storage::minio::MinioBlobStore;
use sqlx::PgPool;
use sqlx::types::chrono::{NaiveDate, TimeZone, Utc};

#[tokio::test]
async fn definition_repository_preserves_versions_and_scoped_reads() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;

    let repository = support::repository(pool.clone());
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let unit_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F03").unwrap();
    let identity = DefinitionIdentity::new(
        unit_id.clone(),
        owner.clone(),
        DefinitionKind::Unit,
        IdempotencyKey::new("repo:unit:create:v1").unwrap(),
    );
    repository.create_identity(identity.clone()).await.unwrap();
    repository.create_identity(identity).await.unwrap();
    let unit = Unit::new(UnitInput {
        unit_id: unit_id.clone(),
        version: Version::new(1).unwrap(),
        owner: owner.clone(),
        code: "CNY".to_owned(),
        dimension: "currency".to_owned(),
        scale: 2,
        precision: 18,
    })
    .unwrap();
    let value = DefinitionValue::Unit(unit);
    let command = AppendDefinitionVersion::new(
        None,
        value.clone(),
        IdempotencyKey::new("repo:unit:append:v1").unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository.append_version(command.clone()).await.unwrap(),
        value
    );
    assert_eq!(repository.append_version(command).await.unwrap(), value);

    let allowed = support::access_scope(&owner);
    assert_eq!(
        repository
            .get_version(&allowed, unit_id.clone(), Version::new(1).unwrap())
            .await
            .unwrap(),
        Some(value)
    );
    let denied_owner = OwnerRef::new(
        owner.tenant_id().clone(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F09").unwrap(),
    );
    assert_eq!(
        repository
            .get_version(
                &support::access_scope(&denied_owner),
                unit_id,
                Version::new(1).unwrap(),
            )
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn snapshot_publication_is_idempotent_and_commits_blob_with_lineage() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    seed_unit(
        &repository,
        &owner,
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F31").unwrap(),
        "SNAPSHOT-SOURCE",
    )
    .await;
    let market_time = |hour| {
        MarketTime::new(
            Utc.with_ymd_and_hms(2025, 1, 15, hour, 0, 0).unwrap(),
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
        )
        .unwrap()
    };
    let bytes = b"real snapshot parquet";
    let manifest_bytes = b"real snapshot manifest";
    let content_hash = ContentHash::digest(bytes);
    let manifest_hash = ContentHash::digest(manifest_bytes);
    let snapshot = SnapshotValue::Data(
        DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F30").unwrap(),
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
    let proof = VerifiedSnapshotProof::data(
        stage_verified_snapshot_blob(
            &pool,
            owner.clone(),
            "snapshot:parquet:fixture:v1",
            bytes,
            SnapshotBlobRole::DataParquet,
        )
        .await,
        stage_verified_snapshot_blob(
            &pool,
            owner.clone(),
            "snapshot:manifest:fixture:v1",
            manifest_bytes,
            SnapshotBlobRole::DataManifest,
        )
        .await,
    )
    .unwrap();
    let command = PublishSnapshot::new(
        snapshot.clone(),
        proof,
        IdempotencyKey::new("snapshot:publish:fixture:v1").unwrap(),
    )
    .unwrap();

    assert_eq!(
        repository
            .publish_verified_manifest(command.clone())
            .await
            .unwrap(),
        snapshot
    );
    assert_eq!(
        repository
            .publish_verified_manifest(command.clone())
            .await
            .unwrap(),
        snapshot
    );

    let metadata_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM research.data_snapshots WHERE tenant_id = $1 AND data_snapshot_id = $2",
    )
    .bind(owner.tenant_id().as_str())
    .bind("01ARZ3NDEKTSV4RRFFQ69G5F30")
    .fetch_one(&pool)
    .await
    .unwrap();
    let blob_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM storage.blobs WHERE tenant_id = $1")
            .bind(owner.tenant_id().as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    let lineage_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM research.lineage_edges WHERE tenant_id = $1")
            .bind(owner.tenant_id().as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((metadata_count, blob_count, lineage_count), (1, 2, 1));

    let retry = PublishSnapshot::new(
        snapshot.clone(),
        VerifiedSnapshotProof::data(
            stage_verified_snapshot_blob(
                &pool,
                owner.clone(),
                "snapshot:parquet:retry:v1",
                bytes,
                SnapshotBlobRole::DataParquet,
            )
            .await,
            stage_verified_snapshot_blob(
                &pool,
                owner.clone(),
                "snapshot:manifest:retry:v1",
                manifest_bytes,
                SnapshotBlobRole::DataManifest,
            )
            .await,
        )
        .unwrap(),
        IdempotencyKey::new("snapshot:publish:fixture:v1").unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository.publish_verified_manifest(retry).await.unwrap(),
        snapshot
    );
    let candidates: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM storage.orphan_candidates")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(candidates, 0);

    sqlx::query(
        "UPDATE research.data_snapshots SET payload = decode('00', 'hex')
         WHERE tenant_id = $1 AND data_snapshot_id = $2",
    )
    .bind(owner.tenant_id().as_str())
    .bind(snapshot.id().as_str())
    .execute(&pool)
    .await
    .unwrap();
    let replay_error = repository
        .publish_verified_manifest(command)
        .await
        .expect_err("replay must decode persisted state instead of trusting the command");
    assert_eq!(
        replay_error.category(),
        ficant_application::ApplicationErrorCategory::StorageUnavailable
    );
}

#[tokio::test]
async fn experiment_transition_and_journal_are_replay_safe_and_scoped() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let scope = support::access_scope(&owner);
    let run = seed_experiment_run(&repository, &pool, &owner).await;
    let validated = MarketRunRulePackResolver::new(&repository, &repository)
        .resolve(&scope, run.clone())
        .await
        .unwrap();
    let create = CreateExperimentRun::new(
        scope.clone(),
        validated,
        IdempotencyKey::new("repo:run:create:v1").unwrap(),
    )
    .unwrap();
    assert_eq!(repository.create_run(create.clone()).await.unwrap(), run);
    assert_eq!(repository.create_run(create).await.unwrap(), run);

    let transition = TransitionExperimentRun::new(
        scope.clone(),
        owner.clone(),
        run.id().clone(),
        1,
        RunState::Running,
        IdempotencyKey::new("repo:run:running:v1").unwrap(),
    )
    .unwrap();
    let running = repository.transition(transition.clone()).await.unwrap();
    assert_eq!(
        (running.state(), running.revision()),
        (RunState::Running, 2)
    );
    assert_eq!(repository.transition(transition).await.unwrap(), running);

    let event = journal_event(run.id().clone());
    let append = AppendJournalEvent::new(
        scope.clone(),
        owner.clone(),
        run.id().clone(),
        1,
        event.clone(),
        IdempotencyKey::new("repo:journal:created:v1").unwrap(),
    )
    .unwrap();
    assert_eq!(repository.append(append.clone()).await.unwrap(), event);
    assert_eq!(repository.append(append).await.unwrap(), event);
    let page = repository
        .read(
            &scope,
            run.id().clone(),
            PageRequest::new(scope.clone(), None, 10).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.items(), &[event]);

    let denied_owner = OwnerRef::new(
        owner.tenant_id().clone(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F09").unwrap(),
    );
    assert_eq!(
        repository
            .get_run(&support::access_scope(&denied_owner), run.id().clone())
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn experiment_run_create_rechecks_persisted_snapshot_after_resolution() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = test_owner();
    let scope = support::access_scope(&owner);
    let run = seed_experiment_run(&repository, &pool, &owner).await;
    let validated = MarketRunRulePackResolver::new(&repository, &repository)
        .resolve(&scope, run.clone())
        .await
        .unwrap();
    let command = CreateExperimentRun::new(
        scope,
        validated,
        IdempotencyKey::new("repo:run:persisted-snapshot-drift").unwrap(),
    )
    .unwrap();
    let deleted = sqlx::query(
        "DELETE FROM research.data_snapshots
         WHERE tenant_id = $1 AND data_snapshot_id = $2",
    )
    .bind(owner.tenant_id().as_str())
    .bind(run.data_snapshot().object_id().as_str())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(deleted.rows_affected(), 1);
    let error = repository
        .create_run(command)
        .await
        .expect_err("persisted snapshot disappearance must fail closed");
    assert_eq!(
        error.category(),
        ficant_application::ApplicationErrorCategory::LineageIncomplete
    );
    assert!(!error.retryable());
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM research.experiment_runs),
             (SELECT COUNT(*) FROM core.idempotency_records
              WHERE scope = 'experiment-run:create:v2')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0));
}

#[tokio::test]
async fn market_fact_query_uses_scoped_aead_cursor_across_pages() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool);
    let owner = test_owner();
    let scope = support::access_scope(&owner);
    let definitions = seed_market_definitions(&repository, &owner).await;
    let first = quote_fact(
        "01ARZ3NDEKTSV4RRFFQ69G5F70",
        "quote-page-1",
        market_time(8),
        &owner,
        &definitions,
    );
    let second = quote_fact(
        "01ARZ3NDEKTSV4RRFFQ69G5F71",
        "quote-page-2",
        market_time(9),
        &owner,
        &definitions,
    );
    repository
        .append_fact(
            AppendMarketFact::new(
                MarketFactRulePackResolver::new(&repository)
                    .resolve(
                        &scope,
                        MarketFactUnitResolver::new(&repository)
                            .resolve(&scope, first.clone())
                            .await
                            .unwrap(),
                    )
                    .await
                    .unwrap(),
                IdempotencyKey::new("repo:quote:first").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    repository
        .append_fact(
            AppendMarketFact::new(
                MarketFactRulePackResolver::new(&repository)
                    .resolve(
                        &scope,
                        MarketFactUnitResolver::new(&repository)
                            .resolve(&scope, second.clone())
                            .await
                            .unwrap(),
                    )
                    .await
                    .unwrap(),
                IdempotencyKey::new("repo:quote:second").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let window = |page| {
        MarketFactWindow::new(
            definitions.instrument.clone(),
            market_time(7),
            market_time(10),
            page,
        )
        .unwrap()
    };
    let first_page = repository
        .query_instrument_window(
            &scope,
            window(PageRequest::new(scope.clone(), None, 1).unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(first_page.items(), &[first]);
    let cursor = first_page.next_cursor().cloned().unwrap();
    assert!(!cursor.as_str().contains("01ARZ3NDEKTSV4RRFFQ69G5F70"));
    let second_page = repository
        .query_instrument_window(
            &scope,
            window(PageRequest::new(scope.clone(), Some(cursor.clone()), 1).unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(second_page.items(), &[second]);
    assert!(second_page.next_cursor().is_none());

    let denied = OwnerRef::new(
        owner.tenant_id().clone(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F09").unwrap(),
    );
    assert!(PageRequest::new(support::access_scope(&denied), Some(cursor), 1).is_err());
}

#[tokio::test]
async fn market_fact_append_rechecks_resolved_units_before_idempotency() {
    for delete_unit in [false, true] {
        let pool = support::postgres_pool().await;
        support::reset_postgres(&pool).await;
        support::migrate(&pool).await;
        let repository = support::repository(pool.clone());
        let owner = test_owner();
        let scope = support::access_scope(&owner);
        let definitions = seed_market_definitions(&repository, &owner).await;
        let fact = quote_fact(
            "01ARZ3NDEKTSV4RRFFQ69G5F72",
            if delete_unit {
                "missing-unit"
            } else {
                "mismatched-unit"
            },
            market_time(8),
            &owner,
            &definitions,
        );
        let validated = MarketFactRulePackResolver::new(&repository)
            .resolve(
                &scope,
                MarketFactUnitResolver::new(&repository)
                    .resolve(&scope, fact)
                    .await
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(validated.unit_proof().bindings().len(), 2);
        assert_eq!(
            validated
                .unit_proof()
                .bindings()
                .iter()
                .map(|binding| (binding.role(), binding.ordinal()))
                .collect::<Vec<_>>(),
            vec![
                (MarketFactFieldRole::Price, 0),
                (MarketFactFieldRole::Price, 1),
            ]
        );

        if delete_unit {
            sqlx::query(
                "DELETE FROM market.units WHERE tenant_id = $1 AND unit_id = $2 AND version = $3",
            )
            .bind(owner.tenant_id().as_str())
            .bind(definitions.price.unit_id().as_str())
            .bind(i64::try_from(definitions.price.version().get()).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        } else {
            sqlx::query(
                "UPDATE market.units SET dimension = 'rate'
                 WHERE tenant_id = $1 AND unit_id = $2 AND version = $3",
            )
            .bind(owner.tenant_id().as_str())
            .bind(definitions.price.unit_id().as_str())
            .bind(i64::try_from(definitions.price.version().get()).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        }

        let error = repository
            .append_fact(
                AppendMarketFact::new(
                    validated,
                    IdempotencyKey::new(if delete_unit {
                        "repo:quote:missing-unit"
                    } else {
                        "repo:quote:mismatched-unit"
                    })
                    .unwrap(),
                )
                .unwrap(),
            )
            .await
            .expect_err("storage must fail closed when resolved Unit state has drifted");
        assert_eq!(
            error.category(),
            ficant_application::ApplicationErrorCategory::ValidationFailed
        );
        assert!(!error.retryable());
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT
                 (SELECT COUNT(*) FROM market.quotes),
                 (SELECT COUNT(*) FROM core.idempotency_records
                  WHERE scope = 'market-fact:write:v1')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(counts, (0, 0));
    }
}

#[tokio::test]
async fn market_fact_append_accepts_real_double_sided_price_bindings() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = test_owner();
    let scope = support::access_scope(&owner);
    let definitions = seed_market_definitions(&repository, &owner).await;
    let fact = quote_fact(
        "01ARZ3NDEKTSV4RRFFQ69G5F73",
        "legal-double-sided",
        market_time(8),
        &owner,
        &definitions,
    );
    let unit_validated = MarketFactUnitResolver::new(&repository)
        .resolve(&scope, fact.clone())
        .await
        .unwrap();
    assert_eq!(
        unit_validated
            .proof()
            .bindings()
            .iter()
            .map(|binding| {
                (
                    binding.role(),
                    binding.ordinal(),
                    binding.dimension().to_owned(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (MarketFactFieldRole::Price, 0, "price".to_owned()),
            (MarketFactFieldRole::Price, 1, "price".to_owned()),
        ]
    );
    let validated = MarketFactRulePackResolver::new(&repository)
        .resolve(&scope, unit_validated)
        .await
        .unwrap();
    let persisted = repository
        .append_fact(
            AppendMarketFact::new(
                validated,
                IdempotencyKey::new("repo:quote:legal-double-sided").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(persisted, fact);
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM market.quotes),
             (SELECT COUNT(*) FROM core.idempotency_records
              WHERE scope = 'market-fact:write:v1')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (1, 1));
}

#[tokio::test]
async fn market_fact_append_rechecks_persisted_rule_interval_before_idempotency() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = test_owner();
    let scope = support::access_scope(&owner);
    let definitions = seed_market_definitions(&repository, &owner).await;
    let fact = MarketFact::Valuation(
        Valuation::new(ValuationInput {
            valuation_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F74").unwrap(),
            instrument: definitions.instrument.clone(),
            owner: owner.clone(),
            source: FactSource::new("fixture-feed", "persisted-rule-drift", 1).unwrap(),
            valuation_at: market_time(8),
            method: "storage-recheck".to_owned(),
            rule_pack: definitions.rule_pack.clone(),
            values: vec![DecimalValue::new("1012300", 4, definitions.price.clone()).unwrap()],
            supersedes_id: None,
        })
        .unwrap(),
    );
    let validated = MarketFactRulePackResolver::new(&repository)
        .resolve(
            &scope,
            MarketFactUnitResolver::new(&repository)
                .resolve(&scope, fact)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE market.market_rule_packs SET effective_to = $4
         WHERE tenant_id = $1 AND rule_pack_id = $2 AND version = $3",
    )
    .bind(owner.tenant_id().as_str())
    .bind(definitions.rule_pack.id().as_str())
    .bind(i64::try_from(definitions.rule_pack.version().get()).unwrap())
    .bind(market_time(8).instant())
    .execute(&pool)
    .await
    .unwrap();
    let error = repository
        .append_fact(
            AppendMarketFact::new(
                validated,
                IdempotencyKey::new("repo:valuation:persisted-rule-drift").unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect_err("persisted half-open interval drift must fail closed");
    assert_eq!(
        error.category(),
        ficant_application::ApplicationErrorCategory::ValidationFailed
    );
    assert!(!error.retryable());
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM market.valuations),
             (SELECT COUNT(*) FROM core.idempotency_records
              WHERE scope = 'market-fact:write:v1')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0));
}

#[tokio::test]
// The valid publication and immutable conflict must share one committed baseline.
#[allow(clippy::too_many_lines)]
async fn curve_snapshot_publication_consumes_verified_blob_and_is_scoped() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = test_owner();
    let scope = support::access_scope(&owner);
    let definitions = seed_market_definitions(&repository, &owner).await;
    let bytes = b"tenor,rate\n1Y,0.021\n";
    let content_hash = ContentHash::digest(bytes);
    let curve = CurveSnapshot::new(CurveSnapshotInput {
        curve_snapshot_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F80").unwrap(),
        owner: owner.clone(),
        as_of: market_time(8),
        currency: definitions.currency.clone(),
        curve_kind: "ZERO_RATE".to_owned(),
        calendar: definitions.calendar.clone(),
        rule_pack: definitions.rule_pack.clone(),
        point_schema: "tenor:string,rate:decimal".to_owned(),
        content_hash: content_hash.clone(),
        lineage: vec![LineageRef::versioned(
            definitions.instrument.id().clone(),
            definitions.instrument.version(),
        )],
        input_kind: ArtifactInputKind::ExternalFixture,
    })
    .unwrap();
    let verified = stage_verified_blob(&pool, owner.clone(), "curve:blob:fixture:v1", bytes).await;
    let command = PublishCurveSnapshot::new(
        scope.clone(),
        curve.clone(),
        u64::try_from(bytes.len()).unwrap(),
        verified,
        IdempotencyKey::new("curve:publish:fixture:v1").unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository
            .publish_curve_snapshot(command.clone())
            .await
            .unwrap(),
        curve
    );
    assert_eq!(
        repository.publish_curve_snapshot(command).await.unwrap(),
        curve
    );
    assert_eq!(
        repository
            .get_curve_snapshot(&scope, curve.id().clone())
            .await
            .unwrap(),
        Some(curve.clone())
    );
    let committed: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM market.curve_snapshots),
             (SELECT COUNT(*) FROM storage.blobs),
             (SELECT COUNT(*) FROM research.lineage_edges)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(committed, (1, 1, 1));

    let conflicting_bytes = b"tenor,rate\n1Y,0.099\n";
    let conflicting_hash = ContentHash::digest(conflicting_bytes);
    let conflicting_curve = CurveSnapshot::new(CurveSnapshotInput {
        curve_snapshot_id: curve.id().clone(),
        owner: owner.clone(),
        as_of: market_time(8),
        currency: definitions.currency,
        curve_kind: "ZERO_RATE".to_owned(),
        calendar: definitions.calendar,
        rule_pack: definitions.rule_pack,
        point_schema: "tenor:string,rate:decimal".to_owned(),
        content_hash: conflicting_hash,
        lineage: vec![LineageRef::versioned(
            definitions.instrument.id().clone(),
            definitions.instrument.version(),
        )],
        input_kind: ArtifactInputKind::ExternalFixture,
    })
    .unwrap();
    let conflicting_verified = stage_verified_blob(
        &pool,
        owner.clone(),
        "curve:blob:conflict:v1",
        conflicting_bytes,
    )
    .await;
    let conflict = repository
        .publish_curve_snapshot(
            PublishCurveSnapshot::new(
                scope.clone(),
                conflicting_curve,
                u64::try_from(conflicting_bytes.len()).unwrap(),
                conflicting_verified,
                IdempotencyKey::new("curve:publish:conflict:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        conflict.category(),
        ficant_application::ApplicationErrorCategory::ImmutableViolation
    );
    let after_conflict: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM market.curve_snapshots),
             (SELECT COUNT(*) FROM storage.blobs),
             (SELECT COUNT(*) FROM research.lineage_edges)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after_conflict, committed);
    let denied = OwnerRef::new(
        owner.tenant_id().clone(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F09").unwrap(),
    );
    assert_eq!(
        repository
            .get_curve_snapshot(&support::access_scope(&denied), curve.id().clone())
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn curve_publication_rejects_unverifiable_lineage_and_rolls_back_all_tables() {
    for case in [
        InvalidLineageCase::MissingTarget,
        InvalidLineageCase::WrongTenant,
        InvalidLineageCase::WrongVersion,
        InvalidLineageCase::WrongHash,
    ] {
        let pool = support::postgres_pool().await;
        support::reset_postgres(&pool).await;
        support::migrate(&pool).await;
        let repository = support::repository(pool.clone());
        let owner = test_owner();
        let scope = support::access_scope(&owner);
        let definitions = seed_market_definitions(&repository, &owner).await;
        let lineage = match case {
            InvalidLineageCase::MissingTarget => LineageRef::versioned(
                Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F90").unwrap(),
                Version::new(1).unwrap(),
            ),
            InvalidLineageCase::WrongTenant => {
                let other_owner = OwnerRef::new(
                    Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F11").unwrap(),
                    Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F12").unwrap(),
                );
                let unit_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F91").unwrap();
                seed_unit(&repository, &other_owner, unit_id.clone(), "USD").await;
                LineageRef::versioned(unit_id, Version::new(1).unwrap())
            }
            InvalidLineageCase::WrongVersion => LineageRef::versioned(
                definitions.instrument.id().clone(),
                Version::new(2).unwrap(),
            ),
            InvalidLineageCase::WrongHash => LineageRef::new(
                definitions.rule_pack.id().clone(),
                Some(definitions.rule_pack.version()),
                Some(ContentHash::digest(b"wrong-rule-pack-hash")),
            )
            .unwrap(),
        };
        let label = case.label();
        let bytes = format!("invalid-lineage-curve:{label}").into_bytes();
        let content_hash = ContentHash::digest(&bytes);
        let curve = CurveSnapshot::new(CurveSnapshotInput {
            curve_snapshot_id: Ulid::new(case.curve_id()).unwrap(),
            owner: owner.clone(),
            as_of: market_time(8),
            currency: definitions.currency,
            curve_kind: "ZERO_RATE".to_owned(),
            calendar: definitions.calendar,
            rule_pack: definitions.rule_pack,
            point_schema: "tenor:string,rate:decimal".to_owned(),
            content_hash: content_hash.clone(),
            lineage: vec![lineage],
            input_kind: ArtifactInputKind::ExternalFixture,
        })
        .unwrap();
        let verified =
            stage_verified_blob(&pool, owner, &format!("curve:invalid:{label}:blob"), &bytes).await;
        let error = repository
            .publish_curve_snapshot(
                PublishCurveSnapshot::new(
                    scope,
                    curve,
                    u64::try_from(bytes.len()).unwrap(),
                    verified,
                    IdempotencyKey::new(format!("curve:invalid:{label}:publish")).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.category(),
            ficant_application::ApplicationErrorCategory::LineageIncomplete
        );
        let counts: (i64, i64, i64) = sqlx::query_as(
            "SELECT
                 (SELECT COUNT(*) FROM market.curve_snapshots),
                 (SELECT COUNT(*) FROM storage.blobs),
                 (SELECT COUNT(*) FROM research.lineage_edges)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(counts, (0, 0, 0));
    }
}

#[tokio::test]
// One scenario proves the independent IDs, idempotent replay, scoped gets, and persisted FK.
#[allow(clippy::too_many_lines)]
async fn signal_repository_publishes_distinct_signal_and_artifact_identities() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = test_owner();
    let scope = support::access_scope(&owner);
    let run = seed_experiment_run(&repository, &pool, &owner).await;
    seed_unit(
        &repository,
        &owner,
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F64").unwrap(),
        "CURVE-INPUT",
    )
    .await;
    let validated = MarketRunRulePackResolver::new(&repository, &repository)
        .resolve(&scope, run.clone())
        .await
        .unwrap();
    repository
        .create_run(
            CreateExperimentRun::new(
                scope.clone(),
                validated,
                IdempotencyKey::new("signal-storage:run:create").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let artifact_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F18").unwrap();
    let signal_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F19").unwrap();
    let data_ref = LineageRef::versioned(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F61").unwrap(),
        Version::new(1).unwrap(),
    );
    let universe_ref = LineageRef::versioned(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F62").unwrap(),
        Version::new(1).unwrap(),
    );
    let rule_ref = VersionRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F63").unwrap(),
        Version::new(1).unwrap(),
    );
    let input_ref = LineageRef::versioned(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F64").unwrap(),
        Version::new(1).unwrap(),
    );
    let bytes = b"distinct signal and artifact content";
    let hash = ContentHash::digest(bytes);
    let verified =
        stage_verified_blob(&pool, owner.clone(), "signal-storage:artifact:blob", bytes).await;
    let artifact = Artifact::new(
        artifact_id.clone(),
        owner.clone(),
        ArtifactKind::SignalSet,
        "application/vnd.ficant.signal-set",
        hash.clone(),
        u64::try_from(bytes.len()).unwrap(),
        vec![
            data_ref.clone(),
            universe_ref.clone(),
            LineageRef::versioned(rule_ref.id().clone(), rule_ref.version()),
            input_ref.clone(),
        ],
    )
    .unwrap();
    repository
        .publish_verified_blob(
            PublishArtifact::new(
                artifact.clone(),
                verified.clone(),
                IdempotencyKey::new("signal-storage:artifact:publish").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let signal = SignalSet::new(SignalSetInput {
        signal_set_id: signal_id.clone(),
        owner: owner.clone(),
        artifact: LineageRef::content_addressed(artifact_id.clone(), hash),
        experiment_run_id: run.id().clone(),
        data_snapshot: data_ref,
        universe_snapshot: universe_ref,
        rule_packs: vec![rule_ref],
        input_artifacts: vec![input_ref],
        valid: EffectivePeriod::new(market_time(9), market_time(10)).unwrap(),
    })
    .unwrap();
    let command = PublishSignalSet::new(
        signal.clone(),
        verified,
        IdempotencyKey::new("signal-storage:signal:publish").unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository
            .publish_signal_set(command.clone())
            .await
            .unwrap(),
        signal
    );
    assert_eq!(
        repository.publish_signal_set(command).await.unwrap(),
        signal
    );
    assert_eq!(
        SignalRepository::get(&repository, &scope, signal_id.clone())
            .await
            .unwrap(),
        Some(signal)
    );
    assert_eq!(
        SignalRepository::get(&repository, &scope, artifact_id.clone())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        ArtifactRepository::get_metadata(&repository, &scope, artifact_id)
            .await
            .unwrap(),
        Some(artifact)
    );
    assert_eq!(
        ArtifactRepository::get_metadata(&repository, &scope, signal_id)
            .await
            .unwrap(),
        None
    );
    let persisted: (String, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
             s.artifact_id::text,
             (SELECT COUNT(*) FROM research.signal_sets),
             (SELECT COUNT(*) FROM research.artifacts),
             (SELECT COUNT(*) FROM research.lineage_edges WHERE source_object_id = s.signal_set_id),
             (SELECT COUNT(*) FROM core.idempotency_records WHERE scope = 'signal-set:publish:v1')
         FROM research.signal_sets s WHERE s.signal_set_id = $1",
    )
    .bind("01ARZ3NDEKTSV4RRFFQ69G5F19")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        persisted,
        ("01ARZ3NDEKTSV4RRFFQ69G5F18".to_owned(), 1, 1, 5, 1)
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn signal_repository_rejects_invalid_artifact_bindings_without_half_state() {
    for case in [
        InvalidArtifactCase::Missing,
        InvalidArtifactCase::WrongTenant,
        InvalidArtifactCase::WrongOwner,
        InvalidArtifactCase::WrongKind,
        InvalidArtifactCase::WrongHash,
        InvalidArtifactCase::MissingLineage,
        InvalidArtifactCase::ExtraLineage,
        InvalidArtifactCase::VersionLineage,
        InvalidArtifactCase::HashLineage,
    ] {
        let pool = support::postgres_pool().await;
        support::reset_postgres(&pool).await;
        support::migrate(&pool).await;
        let repository = support::repository(pool.clone());
        let owner = test_owner();
        let scope = support::access_scope(&owner);
        let run = seed_experiment_run(&repository, &pool, &owner).await;
        for (id, code) in [
            ("01ARZ3NDEKTSV4RRFFQ69G5F64", "CURVE-INPUT"),
            ("01ARZ3NDEKTSV4RRFFQ69G5F65", "EXTRA-LINEAGE"),
        ] {
            seed_unit(&repository, &owner, Ulid::new(id).unwrap(), code).await;
        }
        if case == InvalidArtifactCase::VersionLineage {
            append_unit_version_two(
                &repository,
                &owner,
                Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F64").unwrap(),
                "CURVE-INPUT-V2",
            )
            .await;
        }
        if case == InvalidArtifactCase::HashLineage {
            seed_rule_pack(
                &repository,
                &owner,
                Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F66").unwrap(),
                ContentHash::digest(b"signal-lineage-rule"),
            )
            .await;
        }
        let validated = MarketRunRulePackResolver::new(&repository, &repository)
            .resolve(&scope, run.clone())
            .await
            .unwrap();
        repository
            .create_run(
                CreateExperimentRun::new(
                    scope.clone(),
                    validated,
                    IdempotencyKey::new(format!("signal-invalid:{}:run", case.label())).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let artifact_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F18").unwrap();
        let signal_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F19").unwrap();
        let data_ref = LineageRef::versioned(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F61").unwrap(),
            Version::new(1).unwrap(),
        );
        let universe_ref = LineageRef::versioned(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F62").unwrap(),
            Version::new(1).unwrap(),
        );
        let rule_ref = VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F63").unwrap(),
            Version::new(1).unwrap(),
        );
        let input_ref = LineageRef::versioned(
            Ulid::new(if case == InvalidArtifactCase::HashLineage {
                "01ARZ3NDEKTSV4RRFFQ69G5F66"
            } else {
                "01ARZ3NDEKTSV4RRFFQ69G5F64"
            })
            .unwrap(),
            Version::new(1).unwrap(),
        );
        let signal_bytes = format!("signal-invalid:{}", case.label()).into_bytes();
        let signal_hash = ContentHash::digest(&signal_bytes);
        let signal_verified = stage_verified_blob(
            &pool,
            owner.clone(),
            &format!("signal-invalid:{}:signal-blob", case.label()),
            &signal_bytes,
        )
        .await;

        if case != InvalidArtifactCase::Missing {
            let artifact_owner = match case {
                InvalidArtifactCase::WrongTenant => OwnerRef::new(
                    Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F71").unwrap(),
                    Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F72").unwrap(),
                ),
                InvalidArtifactCase::WrongOwner => OwnerRef::new(
                    owner.tenant_id().clone(),
                    Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F72").unwrap(),
                ),
                _ => owner.clone(),
            };
            let artifact_lineage = if case == InvalidArtifactCase::WrongTenant {
                let anchor = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F73").unwrap();
                seed_unit(
                    &repository,
                    &artifact_owner,
                    anchor.clone(),
                    "TENANT-ANCHOR",
                )
                .await;
                vec![LineageRef::versioned(anchor, Version::new(1).unwrap())]
            } else {
                let mut lineage = vec![
                    data_ref.clone(),
                    universe_ref.clone(),
                    LineageRef::versioned(rule_ref.id().clone(), rule_ref.version()),
                ];
                match case {
                    InvalidArtifactCase::MissingLineage => {}
                    InvalidArtifactCase::ExtraLineage => {
                        lineage.push(input_ref.clone());
                        lineage.push(LineageRef::versioned(
                            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F65").unwrap(),
                            Version::new(1).unwrap(),
                        ));
                    }
                    InvalidArtifactCase::VersionLineage => lineage.push(LineageRef::versioned(
                        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F64").unwrap(),
                        Version::new(2).unwrap(),
                    )),
                    InvalidArtifactCase::HashLineage => lineage.push(
                        LineageRef::new(
                            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F66").unwrap(),
                            Some(Version::new(1).unwrap()),
                            Some(ContentHash::digest(b"signal-lineage-rule")),
                        )
                        .unwrap(),
                    ),
                    _ => lineage.push(input_ref.clone()),
                }
                lineage
            };
            let artifact_bytes = if case == InvalidArtifactCase::WrongHash {
                format!("artifact-wrong-hash:{}", case.label()).into_bytes()
            } else {
                signal_bytes.clone()
            };
            let artifact_hash = ContentHash::digest(&artifact_bytes);
            let artifact_verified = stage_verified_blob(
                &pool,
                artifact_owner.clone(),
                &format!("signal-invalid:{}:artifact-blob", case.label()),
                &artifact_bytes,
            )
            .await;
            let kind = if case == InvalidArtifactCase::WrongKind {
                ArtifactKind::Generic
            } else {
                ArtifactKind::SignalSet
            };
            let artifact = Artifact::new(
                artifact_id.clone(),
                artifact_owner,
                kind,
                "application/vnd.ficant.signal-set",
                artifact_hash,
                u64::try_from(artifact_bytes.len()).unwrap(),
                artifact_lineage,
            )
            .unwrap();
            repository
                .publish_verified_blob(
                    PublishArtifact::new(
                        artifact,
                        artifact_verified,
                        IdempotencyKey::new(format!(
                            "signal-invalid:{}:artifact-publish",
                            case.label()
                        ))
                        .unwrap(),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        }

        let signal = SignalSet::new(SignalSetInput {
            signal_set_id: signal_id,
            owner: owner.clone(),
            artifact: LineageRef::content_addressed(artifact_id.clone(), signal_hash),
            experiment_run_id: run.id().clone(),
            data_snapshot: data_ref.clone(),
            universe_snapshot: universe_ref.clone(),
            rule_packs: vec![rule_ref.clone()],
            input_artifacts: vec![input_ref.clone()],
            valid: EffectivePeriod::new(market_time(9), market_time(10)).unwrap(),
        })
        .unwrap();
        let baseline: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT
                 (SELECT COUNT(*) FROM research.signal_sets),
                 (SELECT COUNT(*) FROM storage.blobs),
                 (SELECT COUNT(*) FROM research.lineage_edges),
                 (SELECT COUNT(*) FROM core.idempotency_records
                   WHERE scope = 'signal-set:publish:v1')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let error = repository
            .publish_signal_set(
                PublishSignalSet::new(
                    signal,
                    signal_verified,
                    IdempotencyKey::new(format!("signal-invalid:{}:signal-publish", case.label()))
                        .unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.category(),
            ficant_application::ApplicationErrorCategory::LineageIncomplete
        );
        let after: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT
                 (SELECT COUNT(*) FROM research.signal_sets),
                 (SELECT COUNT(*) FROM storage.blobs),
                 (SELECT COUNT(*) FROM research.lineage_edges),
                 (SELECT COUNT(*) FROM core.idempotency_records
                   WHERE scope = 'signal-set:publish:v1')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(after, baseline);

        let same_id = SignalSet::new(SignalSetInput {
            signal_set_id: artifact_id.clone(),
            owner,
            artifact: LineageRef::content_addressed(artifact_id, ContentHash::digest(b"same-id")),
            experiment_run_id: run.id().clone(),
            data_snapshot: data_ref,
            universe_snapshot: universe_ref,
            rule_packs: vec![rule_ref],
            input_artifacts: vec![input_ref],
            valid: EffectivePeriod::new(market_time(9), market_time(10)).unwrap(),
        })
        .unwrap_err();
        assert_eq!(same_id, ficant_domain::DomainErrorCode::BrokenLineage);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InvalidArtifactCase {
    Missing,
    WrongTenant,
    WrongOwner,
    WrongKind,
    WrongHash,
    MissingLineage,
    ExtraLineage,
    VersionLineage,
    HashLineage,
}

impl InvalidArtifactCase {
    const fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::WrongTenant => "wrong-tenant",
            Self::WrongOwner => "wrong-owner",
            Self::WrongKind => "wrong-kind",
            Self::WrongHash => "wrong-hash",
            Self::MissingLineage => "missing-lineage",
            Self::ExtraLineage => "extra-lineage",
            Self::VersionLineage => "version-lineage",
            Self::HashLineage => "hash-lineage",
        }
    }
}

#[derive(Clone, Copy)]
enum InvalidLineageCase {
    MissingTarget,
    WrongTenant,
    WrongVersion,
    WrongHash,
}

impl InvalidLineageCase {
    const fn label(self) -> &'static str {
        match self {
            Self::MissingTarget => "missing",
            Self::WrongTenant => "wrong-tenant",
            Self::WrongVersion => "wrong-version",
            Self::WrongHash => "wrong-hash",
        }
    }

    const fn curve_id(self) -> &'static str {
        match self {
            Self::MissingTarget => "01ARZ3NDEKTSV4RRFFQ69G5F81",
            Self::WrongTenant => "01ARZ3NDEKTSV4RRFFQ69G5F82",
            Self::WrongVersion => "01ARZ3NDEKTSV4RRFFQ69G5F83",
            Self::WrongHash => "01ARZ3NDEKTSV4RRFFQ69G5F84",
        }
    }
}

struct MarketDefinitions {
    currency: UnitRef,
    price: UnitRef,
    calendar: VersionRef,
    rule_pack: VersionRef,
    instrument: VersionRef,
}

// This fixture intentionally creates the complete public definition chain used by both adapters.
#[allow(clippy::too_many_lines)]
async fn seed_market_definitions(
    repository: &ficant_storage::postgres::PostgresRepository,
    owner: &OwnerRef,
) -> MarketDefinitions {
    let currency = UnitRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F03").unwrap(),
        Version::new(1).unwrap(),
    );
    let calendar = VersionRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F04").unwrap(),
        Version::new(1).unwrap(),
    );
    let price = UnitRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F07").unwrap(),
        Version::new(1).unwrap(),
    );
    let rule_pack = VersionRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F05").unwrap(),
        Version::new(1).unwrap(),
    );
    let instrument = VersionRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F06").unwrap(),
        Version::new(1).unwrap(),
    );
    let values = vec![
        DefinitionValue::Unit(
            Unit::new(UnitInput {
                unit_id: currency.unit_id().clone(),
                version: currency.version(),
                owner: owner.clone(),
                code: "CNY".to_owned(),
                dimension: "currency".to_owned(),
                scale: 2,
                precision: 18,
            })
            .unwrap(),
        ),
        DefinitionValue::Unit(
            Unit::new(UnitInput {
                unit_id: price.unit_id().clone(),
                version: price.version(),
                owner: owner.clone(),
                code: "TEST_PRICE".to_owned(),
                dimension: "price".to_owned(),
                scale: 4,
                precision: 18,
            })
            .unwrap(),
        ),
        DefinitionValue::Calendar(
            Calendar::new(CalendarInput {
                calendar_id: calendar.id().clone(),
                version: calendar.version(),
                owner: owner.clone(),
                market: "XSHG".to_owned(),
                market_timezone: "Asia/Shanghai".to_owned(),
                effective: EffectivePeriod::new(market_time(1), market_time(15)).unwrap(),
                sessions: vec![],
            })
            .unwrap(),
        ),
        DefinitionValue::MarketRulePack(
            MarketRulePack::new(MarketRulePackInput {
                rule_pack_id: rule_pack.id().clone(),
                version: rule_pack.version(),
                owner: owner.clone(),
                market: "XSHG".to_owned(),
                rule_type: "TRADING".to_owned(),
                source: "official-fixture".to_owned(),
                effective: EffectivePeriod::new(market_time(1), market_time(15)).unwrap(),
                verification_status: VerificationStatus::Verified,
                content_hash: ContentHash::digest(b"rule-pack"),
            })
            .unwrap(),
        ),
        DefinitionValue::Instrument(
            InstrumentDefinition::new(
                Instrument::new(InstrumentInput {
                    instrument_id: instrument.id().clone(),
                    version: instrument.version(),
                    owner: owner.clone(),
                    kind: InstrumentKind::Other,
                    market: "XSHG".to_owned(),
                    symbol: "TEST.CNY".to_owned(),
                    currency: currency.clone(),
                    calendar: calendar.clone(),
                })
                .unwrap(),
                None,
            )
            .unwrap(),
        ),
    ];
    for (index, value) in values.into_iter().enumerate() {
        repository
            .create_identity(DefinitionIdentity::new(
                Ulid::new(value.identity()).unwrap(),
                owner.clone(),
                value.kind(),
                IdempotencyKey::new(format!("fixture:definition:{index}:identity")).unwrap(),
            ))
            .await
            .unwrap();
        repository
            .append_version(
                AppendDefinitionVersion::new(
                    None,
                    value,
                    IdempotencyKey::new(format!("fixture:definition:{index}:v1")).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }
    MarketDefinitions {
        currency,
        price,
        calendar,
        rule_pack,
        instrument,
    }
}

async fn seed_unit(
    repository: &ficant_storage::postgres::PostgresRepository,
    owner: &OwnerRef,
    unit_id: Ulid,
    code: &str,
) {
    repository
        .create_identity(DefinitionIdentity::new(
            unit_id.clone(),
            owner.clone(),
            DefinitionKind::Unit,
            IdempotencyKey::new(format!("fixture:{code}:identity")).unwrap(),
        ))
        .await
        .unwrap();
    repository
        .append_version(
            AppendDefinitionVersion::new(
                None,
                DefinitionValue::Unit(
                    Unit::new(UnitInput {
                        unit_id,
                        version: Version::new(1).unwrap(),
                        owner: owner.clone(),
                        code: code.to_owned(),
                        dimension: "currency".to_owned(),
                        scale: 2,
                        precision: 18,
                    })
                    .unwrap(),
                ),
                IdempotencyKey::new(format!("fixture:{code}:v1")).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn append_unit_version_two(
    repository: &ficant_storage::postgres::PostgresRepository,
    owner: &OwnerRef,
    unit_id: Ulid,
    code: &str,
) {
    repository
        .append_version(
            AppendDefinitionVersion::new(
                Some(Version::new(1).unwrap()),
                DefinitionValue::Unit(
                    Unit::new(UnitInput {
                        unit_id,
                        version: Version::new(2).unwrap(),
                        owner: owner.clone(),
                        code: code.to_owned(),
                        dimension: "currency".to_owned(),
                        scale: 2,
                        precision: 18,
                    })
                    .unwrap(),
                ),
                IdempotencyKey::new(format!("fixture:{code}:v2")).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn seed_rule_pack(
    repository: &ficant_storage::postgres::PostgresRepository,
    owner: &OwnerRef,
    rule_pack_id: Ulid,
    content_hash: ContentHash,
) {
    let semantic_id = rule_pack_id.as_str().to_owned();
    repository
        .create_identity(DefinitionIdentity::new(
            rule_pack_id.clone(),
            owner.clone(),
            DefinitionKind::MarketRulePack,
            IdempotencyKey::new(format!("fixture:{semantic_id}:rule:identity")).unwrap(),
        ))
        .await
        .unwrap();
    repository
        .append_version(
            AppendDefinitionVersion::new(
                None,
                DefinitionValue::MarketRulePack(
                    MarketRulePack::new(MarketRulePackInput {
                        rule_pack_id,
                        version: Version::new(1).unwrap(),
                        owner: owner.clone(),
                        market: "CIBM".to_owned(),
                        rule_type: "SIGNAL_LINEAGE".to_owned(),
                        source: "storage-fixture".to_owned(),
                        effective: EffectivePeriod::new(market_time(1), market_time(15)).unwrap(),
                        verification_status: VerificationStatus::Verified,
                        content_hash,
                    })
                    .unwrap(),
                ),
                IdempotencyKey::new(format!("fixture:{semantic_id}:rule:v1")).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
}

fn quote_fact(
    quote_id: &str,
    external_id: &str,
    observed_at: MarketTime,
    owner: &OwnerRef,
    definitions: &MarketDefinitions,
) -> MarketFact {
    MarketFact::Quote(
        Quote::new(QuoteInput {
            quote_id: Ulid::new(quote_id).unwrap(),
            instrument: definitions.instrument.clone(),
            owner: owner.clone(),
            source: FactSource::new("fixture-feed", external_id, 1).unwrap(),
            received_at: observed_at.clone(),
            observed_at,
            bid: Some(DecimalValue::new("210", 4, definitions.price.clone()).unwrap()),
            ask: Some(DecimalValue::new("220", 4, definitions.price.clone()).unwrap()),
            supersedes_id: None,
        })
        .unwrap(),
    )
}

fn test_owner() -> OwnerRef {
    OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    )
}

fn market_time(hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2025, 1, 15, hour, 0, 0).unwrap(),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
    )
    .unwrap()
}

#[allow(clippy::too_many_lines)]
async fn seed_experiment_run(
    repository: &ficant_storage::postgres::PostgresRepository,
    pool: &PgPool,
    owner: &OwnerRef,
) -> ExperimentRun {
    for (id, code) in [
        ("01ARZ3NDEKTSV4RRFFQ69G5F61", "RUN-DATA-LINEAGE"),
        ("01ARZ3NDEKTSV4RRFFQ69G5F62", "RUN-UNIVERSE"),
        ("01ARZ3NDEKTSV4RRFFQ69G5F67", "RUN-DATA-SOURCE"),
    ] {
        seed_unit(repository, owner, Ulid::new(id).unwrap(), code).await;
    }
    seed_rule_pack(
        repository,
        owner,
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F63").unwrap(),
        ContentHash::digest(b"run-rule-pack"),
    )
    .await;
    let data = b"run-data-snapshot";
    let manifest = b"run-data-manifest";
    let snapshot = SnapshotValue::Data(
        DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F61").unwrap(),
            owner: owner.clone(),
            visible_at: market_time(8),
            as_of: market_time(7),
            schema_hash: ContentHash::digest(b"run-data-schema"),
            manifest_hash: ContentHash::digest(manifest),
            blob_content_hash: ContentHash::digest(data),
            lineage: vec![LineageRef::versioned(
                Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F67").unwrap(),
                Version::new(1).unwrap(),
            )],
        })
        .unwrap(),
    );
    let proof = VerifiedSnapshotProof::data(
        stage_verified_snapshot_blob(
            pool,
            owner.clone(),
            "run:data:parquet:v1",
            data,
            SnapshotBlobRole::DataParquet,
        )
        .await,
        stage_verified_snapshot_blob(
            pool,
            owner.clone(),
            "run:data:manifest:v1",
            manifest,
            SnapshotBlobRole::DataManifest,
        )
        .await,
    )
    .unwrap();
    repository
        .publish_verified_manifest(
            PublishSnapshot::new(
                snapshot,
                proof,
                IdempotencyKey::new("run:data:publish:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    experiment_run(owner.clone())
}

fn experiment_run(owner: OwnerRef) -> ExperimentRun {
    ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F60").unwrap(),
        owner,
        data_snapshot: LineageRef::content_addressed(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F61").unwrap(),
            ContentHash::digest(b"run-data-snapshot"),
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

fn journal_event(run_id: Ulid) -> RunJournal {
    let input = RunJournalInput {
        journal_event_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F64").unwrap(),
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
        payload: b"run-created".to_vec(),
        prev_hash: None,
    };
    let claimed = input.canonical_hash().unwrap();
    let event = RunJournal::new(input, &claimed).unwrap();
    assert_eq!(event.content_hash(), &claimed);
    event
}

async fn stage_verified_blob(
    pool: &PgPool,
    owner: OwnerRef,
    idempotency_key: &str,
    bytes: &[u8],
) -> VerifiedBlobRef {
    let scope = support::access_scope(&owner);
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let store =
        MinioBlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool.clone()).unwrap();
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner,
                u64::try_from(bytes.len()).unwrap(),
                IdempotencyKey::new(idempotency_key).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(&scope, &staged, bytes.to_vec())
        .await
        .unwrap();
    store
        .verify_and_promote(
            VerifyBlobStage::new(
                scope,
                staged,
                ContentHash::digest(bytes),
                u64::try_from(bytes.len()).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
}

async fn stage_verified_snapshot_blob(
    pool: &PgPool,
    owner: OwnerRef,
    idempotency_key: &str,
    bytes: &[u8],
    role: SnapshotBlobRole,
) -> VerifiedSnapshotBlob {
    let scope = support::access_scope(&owner);
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let store =
        MinioBlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool.clone()).unwrap();
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner,
                u64::try_from(bytes.len()).unwrap(),
                IdempotencyKey::new(idempotency_key).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(&scope, &staged, bytes.to_vec())
        .await
        .unwrap();
    let staged = ficant_application::ports::StagedSnapshotBlob::new(
        role,
        VerifyBlobStage::new(
            scope,
            staged,
            ContentHash::digest(bytes),
            u64::try_from(bytes.len()).unwrap(),
        )
        .unwrap(),
    );
    let verified = store
        .verify_and_promote(staged.verification().clone())
        .await
        .unwrap();
    VerifiedSnapshotBlob::from_staged(staged, verified).unwrap()
}
