mod support;

use std::collections::BTreeSet;
use std::sync::OnceLock;

use sqlx::Row;

// Frozen output of the pre-D-016 FSTO v1 SignalSet encoder. That implementation used the same
// canonical ULID for SignalSet and its carrier Artifact, so this is a real legacy value rather
// than malformed dummy bytes that could make the migration test vacuous.
const LEGACY_SIGNAL_HEX: &str = concat!(
    "4653544f0001000000000000001a303141525a334e44454b54535634525246465136394735463138",
    "000000000000001a303141525a334e44454b54535634525246465136394735463031000000000000",
    "001a303141525a334e44454b54535634525246465136394735463032000000000000001a303141525a",
    "334e44454b5453563452524646513639473546313800010000000000000020aaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa000000000000001a303141525a334e44454b54",
    "535634525246465136394735463137000000000000001a303141525a334e44454b5453563452524646",
    "513639473546313500010000000000000020bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "bbbbbbbbbbbbbbbb000000000000001a303141525a334e44454b545356345252464651363947354631",
    "3600010000000000000020cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "0000000000000001000000000000001a303141525a334e44454b545356345252464651363947354630",
    "3800000000000000010000000000000001000000000000001a303141525a334e44454b545356345252",
    "4646513639473546313400010000000000000020dddddddddddddddddddddddddddddddddddddddddddd",
    "dddddddddddddddddddd000000006787607400000000000000000000000d417369612f5368616e6768",
    "6169000000000000000a323032352d30312d3135000000006788b1f400000000000000000000000d41",
    "7369612f5368616e67686169000000000000000a323032352d30312d3136"
);

fn migration_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn forward_migrations_cover_phase1_and_are_repeatable_and_atomic() {
    let _guard = migration_test_lock().lock().await;
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;

    let expected_migration_versions = (1_i64..=18).collect::<Vec<_>>();
    let applied_before_repeat: Vec<(i64, bool)> =
        sqlx::query_as("SELECT version, success FROM public._sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .expect("migration history must record every forward migration");
    assert_eq!(
        applied_before_repeat
            .iter()
            .map(|(version, _)| *version)
            .collect::<Vec<_>>(),
        expected_migration_versions
    );
    assert!(
        applied_before_repeat.iter().all(|(_, success)| *success),
        "every recorded forward migration must have succeeded"
    );
    assert_eq!(
        applied_before_repeat
            .iter()
            .filter(|(version, success)| *version == 17 && *success)
            .count(),
        1,
        "0017 must be recorded exactly once after its successful application"
    );
    assert_eq!(
        applied_before_repeat
            .iter()
            .filter(|(version, success)| *version == 18 && *success)
            .count(),
        1,
        "0018 must be recorded exactly once after its successful application"
    );

    let rows = sqlx::query(
        "SELECT schemaname, tablename
         FROM pg_catalog.pg_tables
         WHERE schemaname IN ('core', 'data', 'market', 'research', 'storage')",
    )
    .fetch_all(&pool)
    .await
    .expect("schema inventory query must succeed");
    let actual = rows
        .into_iter()
        .map(|row| {
            let schema: String = row.get("schemaname");
            let table: String = row.get("tablename");
            format!("{schema}.{table}")
        })
        .collect::<BTreeSet<_>>();
    let required = [
        "core.definition_identities",
        "core.idempotency_records",
        "core.subject_versions",
        "core.subject_state_snapshots",
        "core.subject_state_limit_ceilings",
        "data.source_identities",
        "data.sources",
        "market.bonds",
        "market.calendars",
        "market.cashflows",
        "market.curve_snapshots",
        "market.futures_contracts",
        "market.instruments",
        "market.market_rule_packs",
        "market.quotes",
        "market.trades",
        "market.units",
        "market.valuations",
        "research.artifacts",
        "research.data_snapshots",
        "research.experiment_runs",
        "research.execution_tasks",
        "research.execution_identities",
        "research.execution_external_inputs",
        "research.execution_node_implementations",
        "research.execution_rule_packs",
        "research.factor_definitions",
        "research.curve_node_definitions",
        "research.factor_target_bindings",
        "research.lineage_edges",
        "research.node_executions",
        "research.research_graphs",
        "research.run_journal",
        "research.run_journal_sequences",
        "research.signal_sets",
        "research.universe_members",
        "research.universe_snapshots",
        "storage.blobs",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    assert_eq!(actual.intersection(&required).count(), required.len());

    support::migrate(&pool).await;
    let applied_after_repeat: Vec<(i64, bool)> =
        sqlx::query_as("SELECT version, success FROM public._sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .expect("migration history must remain queryable after repeat");
    assert_eq!(
        applied_after_repeat, applied_before_repeat,
        "a repeated forward migration run must not alter the recorded migration set"
    );
    let artifact_column: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM information_schema.columns
             WHERE table_schema = 'research' AND table_name = 'signal_sets'
               AND column_name = 'artifact_id' AND is_nullable = 'NO'
         )",
    )
    .fetch_one(&pool)
    .await
    .expect("independent artifact identity column must be observable");
    assert!(artifact_column);
    let issuance_columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM information_schema.columns
         WHERE table_schema = 'market' AND table_name = 'bonds'
           AND column_name = ANY($1)
           AND is_nullable = 'YES'",
    )
    .bind([
        "first_issue_date",
        "current_issue_date",
        "cumulative_issued_coefficient",
        "cumulative_issued_scale",
        "cumulative_issued_unit_id",
        "cumulative_issued_unit_version",
        "value_added_tax_status",
        "income_tax_status",
    ])
    .fetch_one(&pool)
    .await
    .expect("Bond issuance columns must be observable");
    assert_eq!(issuance_columns, 8);
    let issuance_constraint: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM pg_constraint
             WHERE conname = 'bonds_issuance_shape_check'
         )",
    )
    .fetch_one(&pool)
    .await
    .expect("Bond issuance shape constraint must be observable");
    assert!(issuance_constraint);
    let curve_identity_constraints: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM pg_constraint
         WHERE conrelid = 'research.curve_node_definitions'::regclass
           AND conname = ANY($1)",
    )
    .bind([
        "curve_node_definitions_tenor_canonical_check",
        "curve_node_definitions_family_tenor_key",
    ])
    .fetch_one(&pool)
    .await
    .expect("0018 curve-node identity constraints must be observable");
    assert_eq!(curve_identity_constraints, 2);

    let fixture =
        std::env::temp_dir().join(format!("ficant-failing-migration-{}", std::process::id()));
    if fixture.exists() {
        std::fs::remove_dir_all(&fixture).expect("stale migration fixture must be removable");
    }
    std::fs::create_dir(&fixture).expect("migration fixture directory must be creatable");
    std::fs::write(
        fixture.join("9999_injected_failure.sql"),
        "CREATE TABLE core.atomicity_probe(id integer PRIMARY KEY);\n\
         INSERT INTO core.atomicity_probe(unknown_column) VALUES (1);\n",
    )
    .expect("failing migration fixture must be writable");
    let mut failing_migrator = sqlx::migrate::Migrator::new(fixture.clone())
        .await
        .expect("real SQLx migrator must load the injected fixture");
    failing_migrator.set_ignore_missing(true);
    let failure = failing_migrator.run(&pool).await;
    assert!(failure.is_err());
    let probe: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('core.atomicity_probe')::text")
            .fetch_one(&pool)
            .await
            .expect("rollback observation must succeed");
    assert_eq!(probe, None);
    let injected_history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM public._sqlx_migrations WHERE version = 9999")
            .fetch_one(&pool)
            .await
            .expect("failed migration history must be queryable");
    assert_eq!(injected_history, 0);
    std::fs::remove_dir_all(fixture).expect("migration fixture must be removed");

    pool.close().await;
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    let source =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations/postgresql");
    let fixture = std::env::temp_dir().join(format!(
        "ficant-failing-0017-migration-{}",
        std::process::id()
    ));
    if fixture.exists() {
        std::fs::remove_dir_all(&fixture).expect("stale 0017 migration fixture must be removable");
    }
    std::fs::create_dir(&fixture).expect("0017 migration fixture directory must be creatable");
    for version in 1..=16 {
        let prefix = format!("{version:04}_");
        let migration = std::fs::read_dir(&source)
            .expect("migration source must be readable")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .expect("each 0001..0016 migration must exist");
        std::fs::copy(migration.path(), fixture.join(migration.file_name()))
            .expect("pre-0017 migration must copy without mutation");
    }
    let pre_0017 = sqlx::migrate::Migrator::new(fixture.clone())
        .await
        .expect("pre-0017 migration fixture must load");
    pre_0017
        .run(&pool)
        .await
        .expect("0001..0016 migrations must apply before the 0017 failure check");
    let original_0017 = std::fs::read_to_string(source.join("0017_factor_topology.sql"))
        .expect("0017 migration source must be readable");
    std::fs::write(
        fixture.join("0017_factor_topology.sql"),
        format!(
            "{original_0017}\nINSERT INTO research.factor_definitions(unknown_column) VALUES ('x');\n"
        ),
    )
    .expect("failing 0017 migration fixture must be writable");
    let failing_0017 = sqlx::migrate::Migrator::new(fixture.clone())
        .await
        .expect("failing 0017 migration fixture must load");
    assert!(failing_0017.run(&pool).await.is_err());
    for relation in [
        "research.factor_definitions",
        "research.curve_node_definitions",
        "research.factor_target_bindings",
    ] {
        let relation_after_failure: Option<String> =
            sqlx::query_scalar("SELECT to_regclass($1)::text")
                .bind(relation)
                .fetch_one(&pool)
                .await
                .expect("0017 rollback observation must succeed");
        assert_eq!(
            relation_after_failure, None,
            "failed 0017 must not retain {relation}"
        );
    }
    let failed_0017_history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM public._sqlx_migrations WHERE version = 17")
            .fetch_one(&pool)
            .await
            .expect("0017 failure history must be queryable");
    assert_eq!(failed_0017_history, 0);
    std::fs::remove_dir_all(fixture).expect("0017 migration fixture must be removed");

    pool.close().await;
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    let fixture = std::env::temp_dir().join(format!(
        "ficant-failing-0018-migration-{}",
        std::process::id()
    ));
    if fixture.exists() {
        std::fs::remove_dir_all(&fixture).expect("stale 0018 migration fixture must be removable");
    }
    std::fs::create_dir(&fixture).expect("0018 migration fixture directory must be creatable");
    for version in 1..=17 {
        let prefix = format!("{version:04}_");
        let migration = std::fs::read_dir(&source)
            .expect("migration source must be readable")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .expect("each 0001..0017 migration must exist");
        std::fs::copy(migration.path(), fixture.join(migration.file_name()))
            .expect("pre-0018 migration must copy without mutation");
    }
    let pre_0018 = sqlx::migrate::Migrator::new(fixture.clone())
        .await
        .expect("pre-0018 migration fixture must load");
    pre_0018
        .run(&pool)
        .await
        .expect("0001..0017 migrations must apply before the 0018 failure check");
    let original_0018 =
        std::fs::read_to_string(source.join("0018_curve_node_identity_constraints.sql"))
            .expect("0018 migration source must be readable");
    std::fs::write(
        fixture.join("0018_curve_node_identity_constraints.sql"),
        format!(
            "{original_0018}\nINSERT INTO research.curve_node_definitions(unknown_column) VALUES ('x');\n"
        ),
    )
    .expect("failing 0018 migration fixture must be writable");
    let failing_0018 = sqlx::migrate::Migrator::new(fixture.clone())
        .await
        .expect("failing 0018 migration fixture must load");
    assert!(failing_0018.run(&pool).await.is_err());
    let new_constraints_after_failure: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM pg_constraint
         WHERE conrelid = 'research.curve_node_definitions'::regclass
           AND conname = ANY($1)",
    )
    .bind([
        "curve_node_definitions_tenor_canonical_check",
        "curve_node_definitions_family_tenor_key",
    ])
    .fetch_one(&pool)
    .await
    .expect("0018 rollback constraints must be observable");
    assert_eq!(new_constraints_after_failure, 0);
    let original_tenor_constraint_after_failure: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM pg_constraint
             WHERE conrelid = 'research.curve_node_definitions'::regclass
               AND conname = 'curve_node_definitions_tenor_check'
         )",
    )
    .fetch_one(&pool)
    .await
    .expect("0017 tenor constraint must remain after failed 0018");
    assert!(original_tenor_constraint_after_failure);
    let failed_0018_history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM public._sqlx_migrations WHERE version = 18")
            .fetch_one(&pool)
            .await
            .expect("0018 failure history must be queryable");
    assert_eq!(failed_0018_history, 0);
    std::fs::remove_dir_all(fixture).expect("0018 migration fixture must be removed");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn legacy_signal_rows_block_identity_migration_without_mutating_schema_or_payload() {
    let _guard = migration_test_lock().lock().await;
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    let source =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations/postgresql");
    let fixture = std::env::temp_dir().join(format!(
        "ficant-legacy-six-migrations-{}",
        std::process::id()
    ));
    if fixture.exists() {
        std::fs::remove_dir_all(&fixture).expect("stale legacy fixture must be removable");
    }
    std::fs::create_dir(&fixture).expect("legacy fixture directory must be creatable");
    for version in 1..=6 {
        let prefix = format!("{version:04}_");
        let migration = std::fs::read_dir(&source)
            .expect("migration source must be readable")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .expect("each legacy migration must exist");
        std::fs::copy(migration.path(), fixture.join(migration.file_name()))
            .expect("legacy migration must copy without mutation");
    }
    let legacy = sqlx::migrate::Migrator::new(fixture.clone())
        .await
        .expect("legacy migrator must load real 0001..0006 files");
    legacy
        .run(&pool)
        .await
        .expect("legacy migrations must apply before forward upgrade");
    let legacy_has_artifact_id: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM information_schema.columns
             WHERE table_schema = 'research' AND table_name = 'signal_sets'
               AND column_name = 'artifact_id'
         )",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!legacy_has_artifact_id);

    sqlx::raw_sql(
        "INSERT INTO storage.blobs(tenant_id, content_hash, object_key, blob_size)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F01', repeat('a', 64),
                 'immutable/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 1);
         INSERT INTO research.artifacts
             (tenant_id, artifact_id, owner_id, kind, media_type, content_hash, blob_size,
              idempotency_key, fingerprint, payload)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F01', '01ARZ3NDEKTSV4RRFFQ69G5F18',
                 '01ARZ3NDEKTSV4RRFFQ69G5F02', 'SIGNAL_SET', 'application/test', repeat('a', 64),
                 1, 'legacy-artifact', decode(repeat('11', 32), 'hex'), decode('01', 'hex'));
         INSERT INTO research.experiment_runs
             (tenant_id, experiment_run_id, owner_id, state, revision,
              idempotency_key, fingerprint, payload)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F01', '01ARZ3NDEKTSV4RRFFQ69G5F17',
                 '01ARZ3NDEKTSV4RRFFQ69G5F02', 'CREATED', 1,
                 'legacy-run', decode(repeat('22', 32), 'hex'), decode('02', 'hex'));",
    )
    .execute(&pool)
    .await
    .expect("migration-only legacy dependencies must satisfy the old schema");

    let legacy_payload = decode_hex(LEGACY_SIGNAL_HEX);
    assert!(legacy_payload.starts_with(b"FSTO\0\x01"));
    sqlx::query(
        "INSERT INTO research.signal_sets
             (tenant_id, signal_set_id, owner_id, experiment_run_id, content_hash,
              valid_from, valid_to, idempotency_key, fingerprint, payload)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F01', '01ARZ3NDEKTSV4RRFFQ69G5F18',
                 '01ARZ3NDEKTSV4RRFFQ69G5F02', '01ARZ3NDEKTSV4RRFFQ69G5F17', repeat('a', 64),
                 '2025-01-15T07:15:00Z', '2025-01-16T07:15:00Z',
                 'legacy-signal', decode(repeat('33', 32), 'hex'), $1)",
    )
    .bind(&legacy_payload)
    .execute(&pool)
    .await
    .expect("real canonical legacy SignalSet must satisfy the old schema");

    let migrator = sqlx::migrate::Migrator::new(source)
        .await
        .expect("full migration directory must remain readable");
    let failure = migrator.run(&pool).await;
    let message = failure
        .expect_err("legacy SignalSet rows must block migration 0007")
        .to_string();
    assert!(message.contains("export") && message.contains("rebuild"));
    let preserved: (bool, i64, bool, bool, Vec<u8>) = sqlx::query_as(
        "SELECT
             EXISTS(SELECT 1 FROM information_schema.columns
                    WHERE table_schema = 'research' AND table_name = 'signal_sets'
                      AND column_name = 'artifact_id'),
             (SELECT COUNT(*) FROM public._sqlx_migrations WHERE version = 7),
             EXISTS(SELECT 1 FROM pg_constraint WHERE conname = 'signal_sets_artifact_fkey'),
             EXISTS(SELECT 1 FROM pg_constraint
                    WHERE conname = 'signal_sets_tenant_id_signal_set_id_fkey'),
             payload
         FROM research.signal_sets WHERE signal_set_id = $1",
    )
    .bind("01ARZ3NDEKTSV4RRFFQ69G5F18")
    .fetch_one(&pool)
    .await
    .expect("failed migration must preserve the old table and row");
    assert_eq!(preserved, (false, 0, false, true, legacy_payload));
    std::fs::remove_dir_all(fixture).expect("legacy fixture must be removed");
}

fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).expect("frozen hex fixture must be ASCII");
            u8::from_str_radix(text, 16).expect("frozen hex fixture must be valid")
        })
        .collect()
}

#[tokio::test]
async fn manifest_blob_fk_upgrade_fails_before_mutating_invalid_legacy_schema() {
    let _guard = migration_test_lock().lock().await;
    let pool = support::postgres_pool().await;
    let source =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations/postgresql");

    support::reset_postgres(&pool).await;
    let invalid_fixture = migrate_through(&pool, &source, 7, "manifest-invalid").await;
    insert_legacy_data_snapshot(&pool, false).await;
    let legacy_payload: Vec<u8> = sqlx::query_scalar(
        "SELECT payload FROM research.data_snapshots
         WHERE data_snapshot_id = '01ARZ3NDEKTSV4RRFFQ69G5F15'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let full = sqlx::migrate::Migrator::new(source.clone())
        .await
        .expect("full migrator must load");
    let message = full
        .run(&pool)
        .await
        .expect_err("a legacy DataSnapshot without its manifest blob must block migration 0008")
        .to_string();
    assert!(
        message.contains("manifest") && message.contains("export") && message.contains("rebuild")
    );
    let preserved: (bool, bool, i64, Vec<u8>) = sqlx::query_as(
        "SELECT
             EXISTS(SELECT 1 FROM pg_constraint
                    WHERE conname = 'data_snapshots_manifest_blob_fkey'),
             EXISTS(SELECT 1 FROM pg_indexes
                    WHERE schemaname = 'research'
                      AND indexname = 'data_snapshots_manifest_blob_idx'),
             (SELECT COUNT(*) FROM public._sqlx_migrations WHERE version = 8),
             payload
         FROM research.data_snapshots
         WHERE data_snapshot_id = '01ARZ3NDEKTSV4RRFFQ69G5F15'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(preserved, (false, false, 0, legacy_payload));
    std::fs::remove_dir_all(invalid_fixture).unwrap();
}

#[tokio::test]
async fn manifest_blob_fk_upgrade_accepts_valid_legacy_schema_and_repeats() {
    let _guard = migration_test_lock().lock().await;
    let pool = support::postgres_pool().await;
    let source =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations/postgresql");
    support::reset_postgres(&pool).await;
    let valid_fixture = migrate_through(&pool, &source, 7, "manifest-valid").await;
    insert_legacy_data_snapshot(&pool, true).await;
    let full = sqlx::migrate::Migrator::new(source)
        .await
        .expect("full migrator must load");
    full.run(&pool)
        .await
        .expect("a legacy DataSnapshot with both durable blob refs must upgrade");
    full.run(&pool)
        .await
        .expect("migration 0008 must repeat safely");
    let upgraded: (bool, bool, i64, i64) = sqlx::query_as(
        "SELECT
             EXISTS(SELECT 1 FROM pg_constraint
                    WHERE conname = 'data_snapshots_manifest_blob_fkey'),
             EXISTS(SELECT 1 FROM pg_indexes
                    WHERE schemaname = 'research'
                      AND indexname = 'data_snapshots_manifest_blob_idx'),
             (SELECT COUNT(*) FROM public._sqlx_migrations WHERE version = 8 AND success = TRUE),
             (SELECT COUNT(*) FROM research.data_snapshots)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(upgraded, (true, true, 1, 1));
    std::fs::remove_dir_all(valid_fixture).unwrap();
}

async fn migrate_through(
    pool: &sqlx::PgPool,
    source: &std::path::Path,
    through: u64,
    label: &str,
) -> std::path::PathBuf {
    let fixture = std::env::temp_dir().join(format!("ficant-{label}-{}", std::process::id()));
    if fixture.exists() {
        std::fs::remove_dir_all(&fixture).unwrap();
    }
    std::fs::create_dir(&fixture).unwrap();
    for version in 1..=through {
        let prefix = format!("{version:04}_");
        let migration = std::fs::read_dir(source)
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .unwrap();
        std::fs::copy(migration.path(), fixture.join(migration.file_name())).unwrap();
    }
    sqlx::migrate::Migrator::new(fixture.clone())
        .await
        .unwrap()
        .run(pool)
        .await
        .unwrap();
    fixture
}

async fn insert_legacy_data_snapshot(pool: &sqlx::PgPool, include_manifest_blob: bool) {
    sqlx::raw_sql(
        "INSERT INTO storage.blobs(tenant_id, content_hash, object_key, blob_size)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F01', repeat('a', 64),
                 'immutable/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 1);
         INSERT INTO research.data_snapshots
             (tenant_id, data_snapshot_id, owner_id, visible_at, as_of, schema_hash,
              manifest_hash, content_hash, idempotency_key, fingerprint, payload)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F01', '01ARZ3NDEKTSV4RRFFQ69G5F15',
                 '01ARZ3NDEKTSV4RRFFQ69G5F02', '2025-01-15T07:05:00Z',
                 '2025-01-15T07:00:00Z', repeat('1', 64), repeat('b', 64), repeat('a', 64),
                 'legacy-data-snapshot', decode(repeat('22', 32), 'hex'),
                 decode('4653544f0001', 'hex'));",
    )
    .execute(pool)
    .await
    .unwrap();
    if include_manifest_blob {
        sqlx::query(
            "INSERT INTO storage.blobs(tenant_id, content_hash, object_key, blob_size)
             VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F01', repeat('b', 64),
                     'immutable/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 1)",
        )
        .execute(pool)
        .await
        .unwrap();
    }
}
