use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use ficant_application::ports::{
    AeadCursorCodec, AuthorizedPrincipal, CursorKey, DataSourceRepository, FoundationChangeContext,
    IdempotencyKey, RegisterDataSource,
};
use ficant_data::{
    CANONICAL_QUOTE_SCHEMA_ID, CanonicalIngestRequest, CanonicalQuoteIngestor,
    FileNdjsonQuoteSource, InstrumentMapping, InstrumentMappingEntry, PointInTimeWindow,
    PostgresQuoteSource, canonical_quote_schema_hash,
};
use ficant_domain::governance::{ChangeJustification, PlatformRole, SourceDocumentRef};
use ficant_domain::market::{
    Calendar, CalendarInput, CalendarSession, DataSource, DataSourceInput, DataSourceKind, Unit,
    UnitInput,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_storage::postgres::PostgresRepository;
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
async fn file_and_real_postgres_produce_one_canonical_schema_and_equal_business_rows() {
    let database_url = std::env::var("FICANT_TEST_DATABASE_URL")
        .expect("FICANT_TEST_DATABASE_URL must identify a disposable PostgreSQL 16 database");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("disposable PostgreSQL must be reachable");
    reset_and_migrate(&pool).await;
    seed_external_source(&pool).await;

    let owner = owner();
    let repository = PostgresRepository::new(pool.clone(), cursor_codec());
    let file_source = source(
        owner.clone(),
        "01ARZ3NDEKTSV4RRFFQ69G5F10",
        DataSourceKind::FileNdjson,
        "file-binding",
        "quotes",
    );
    let postgres_source = source(
        owner.clone(),
        "01ARZ3NDEKTSV4RRFFQ69G5F11",
        DataSourceKind::Postgres,
        "postgres-binding",
        "ficant_source_quotes_v1",
    );
    for (source, key) in [
        (file_source.clone(), "phase3a-file-source"),
        (postgres_source.clone(), "phase3a-postgres-source"),
    ] {
        let registered = repository
            .register(
                RegisterDataSource::new(
                    admin_change(&owner, key),
                    None,
                    source.clone(),
                    IdempotencyKey::new(key).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(registered, source);
    }

    let root = fixture_root();
    if root.exists() {
        std::fs::remove_dir_all(&root).unwrap();
    }
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("quotes.ndjson"),
        concat!(
            "{\"ask_coefficient\":\"10126\",\"ask_scale\":2,\"bid_coefficient\":\"10124\",\"bid_scale\":2,\"instrument_key\":\"260011.IB\",\"observed_at\":\"2026-07-20T01:31:00Z\",\"source_record_id\":\"record-2\",\"visible_at\":\"2026-07-20T01:31:05Z\"}\n",
            "{\"ask_coefficient\":\"10125\",\"ask_scale\":2,\"bid_coefficient\":\"10123\",\"bid_scale\":2,\"instrument_key\":\"260011.IB\",\"observed_at\":\"2026-07-20T01:30:00Z\",\"source_record_id\":\"record-1\",\"visible_at\":\"2026-07-20T01:30:05Z\"}\n",
            "{\"ask_coefficient\":\"10127\",\"ask_scale\":2,\"bid_coefficient\":\"10125\",\"bid_scale\":2,\"instrument_key\":\"260011.IB\",\"observed_at\":\"2026-07-20T01:32:00Z\",\"source_record_id\":\"late-record\",\"visible_at\":\"2026-07-20T03:00:00Z\"}\n"
        ),
    )
    .unwrap();

    let calendar = calendar(owner.clone());
    let unit = unit(owner.clone());
    let window = PointInTimeWindow::new(
        market_time("2026-07-20T02:00:00Z"),
        market_time("2026-07-20T02:05:00Z"),
    )
    .unwrap();
    let file_request = request(
        file_source,
        owner.clone(),
        calendar.clone(),
        unit.clone(),
        window.clone(),
    );
    let postgres_request = request(postgres_source, owner, calendar, unit, window);

    let file = CanonicalQuoteIngestor
        .ingest(
            &FileNdjsonQuoteSource::new("file-binding", root.clone()).unwrap(),
            &file_request,
        )
        .await
        .unwrap();
    let postgres = CanonicalQuoteIngestor
        .ingest(
            &PostgresQuoteSource::new("postgres-binding", pool).unwrap(),
            &postgres_request,
        )
        .await
        .unwrap();

    assert_eq!(file.schema_hash(), &canonical_quote_schema_hash());
    assert_eq!(file.schema_hash(), postgres.schema_hash());
    assert_eq!(file.batch().schema(), postgres.batch().schema());
    assert_eq!(file.batch().num_rows(), 2);
    assert_eq!(postgres.batch().num_rows(), 2);
    for column in 4..16 {
        assert_eq!(
            file.batch().column(column).to_data(),
            postgres.batch().column(column).to_data(),
            "canonical business column {column} differs"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

async fn reset_and_migrate(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "DROP SCHEMA IF EXISTS external_data CASCADE;
         DROP SCHEMA IF EXISTS portfolio CASCADE;
         DROP SCHEMA IF EXISTS analytics CASCADE;
         DROP SCHEMA IF EXISTS data CASCADE;
         DROP SCHEMA IF EXISTS storage CASCADE;
         DROP SCHEMA IF EXISTS research CASCADE;
         DROP SCHEMA IF EXISTS market CASCADE;
         DROP SCHEMA IF EXISTS core CASCADE;
         DROP TABLE IF EXISTS public._sqlx_migrations;",
    )
    .execute(pool)
    .await
    .unwrap();
    let migrations = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations/postgresql");
    sqlx::migrate::Migrator::new(migrations)
        .await
        .unwrap()
        .run(pool)
        .await
        .unwrap();
}

async fn seed_external_source(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "CREATE SCHEMA external_data;
         CREATE TABLE external_data.ficant_source_quotes_v1 (
             source_record_id text PRIMARY KEY,
             instrument_key text NOT NULL,
             observed_at timestamptz NOT NULL,
             visible_at timestamptz NOT NULL,
             bid_coefficient text,
             bid_scale integer,
             ask_coefficient text,
             ask_scale integer,
             CHECK ((bid_coefficient IS NULL) = (bid_scale IS NULL)),
             CHECK ((ask_coefficient IS NULL) = (ask_scale IS NULL))
         );
         INSERT INTO external_data.ficant_source_quotes_v1 VALUES
             ('record-2', '260011.IB', '2026-07-20T01:31:00Z', '2026-07-20T01:31:05Z', '10124', 2, '10126', 2),
             ('record-1', '260011.IB', '2026-07-20T01:30:00Z', '2026-07-20T01:30:05Z', '10123', 2, '10125', 2),
             ('late-record', '260011.IB', '2026-07-20T01:32:00Z', '2026-07-20T03:00:00Z', '10125', 2, '10127', 2);",
    )
    .execute(pool)
    .await
    .unwrap();
}

fn request(
    source: DataSource,
    owner: OwnerRef,
    calendar: Calendar,
    unit: Unit,
    window: PointInTimeWindow,
) -> CanonicalIngestRequest {
    let effective = calendar.effective().clone();
    let mapping = InstrumentMapping::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F60").unwrap(),
        owner,
        VersionRef::new(source.id().clone(), Version::new(1).unwrap()),
        vec![
            InstrumentMappingEntry::new(
                "260011.IB",
                effective,
                VersionRef::new(
                    Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F20").unwrap(),
                    Version::new(7).unwrap(),
                ),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    CanonicalIngestRequest::new(source, mapping, calendar, unit, window).unwrap()
}

fn source(
    owner: OwnerRef,
    source_id: &str,
    kind: DataSourceKind,
    binding: &str,
    dataset: &str,
) -> DataSource {
    DataSource::new(DataSourceInput {
        data_source_id: Ulid::new(source_id).unwrap(),
        version: Version::new(1).unwrap(),
        owner,
        kind,
        name: "CGB quotes".to_owned(),
        connection_binding: binding.to_owned(),
        dataset: dataset.to_owned(),
        canonical_schema_id: CANONICAL_QUOTE_SCHEMA_ID.to_owned(),
        canonical_schema_hash: canonical_quote_schema_hash(),
    })
    .unwrap()
}

fn calendar(owner: OwnerRef) -> Calendar {
    Calendar::new(CalendarInput {
        calendar_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F30").unwrap(),
        version: Version::new(3).unwrap(),
        owner,
        market: "CGB".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: EffectivePeriod::new(
            market_time("2026-07-01T00:00:00Z"),
            market_time("2026-08-01T00:00:00Z"),
        )
        .unwrap(),
        sessions: vec![
            CalendarSession::open(
                NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
                NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
            )
            .unwrap(),
        ],
    })
    .unwrap()
}

fn unit(owner: OwnerRef) -> Unit {
    Unit::new(UnitInput {
        unit_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F40").unwrap(),
        version: Version::new(2).unwrap(),
        owner,
        code: "CNY_CLEAN_PRICE".to_owned(),
        dimension: "price".to_owned(),
        scale: 4,
        precision: 12,
    })
    .unwrap()
}

fn market_time(value: &str) -> MarketTime {
    let instant = value.parse::<DateTime<Utc>>().unwrap();
    let timezone = "Asia/Shanghai".parse::<chrono_tz::Tz>().unwrap();
    MarketTime::new(
        instant,
        "Asia/Shanghai",
        instant.with_timezone(&timezone).date_naive(),
    )
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    )
}

fn admin_change(owner: &OwnerRef, label: &str) -> FoundationChangeContext {
    let principal = AuthorizedPrincipal::new(
        "phase3a-admin".to_owned(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F00").unwrap(),
        owner.tenant_id().clone(),
        vec![owner.owner_id().clone()],
        PlatformRole::PlatformAdmin,
        vec!["data-sources:write".to_owned()],
        ContentHash::digest(b"phase3a-admin-credential"),
    )
    .unwrap();
    let change = ChangeJustification::new(
        "Phase 3A deterministic source fixture",
        vec![
            SourceDocumentRef::new(
                "fixture://phase3a/source",
                ContentHash::digest(label.as_bytes()),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    FoundationChangeContext::administrator(
        principal,
        change,
        Ulid::new(if label.ends_with("file-source") {
            "01ARZ3NDEKTSV4RRFFQ69G5F80"
        } else {
            "01ARZ3NDEKTSV4RRFFQ69G5F81"
        })
        .unwrap(),
        market_time("2026-07-20T03:00:00Z"),
    )
    .unwrap()
}

fn cursor_codec() -> Arc<AeadCursorCodec> {
    Arc::new(AeadCursorCodec::new(CursorKey::new("phase3a", [9_u8; 32]).unwrap(), vec![]).unwrap())
}

fn fixture_root() -> PathBuf {
    std::env::temp_dir().join(format!("ficant-phase3a-dual-source-{}", std::process::id()))
}
