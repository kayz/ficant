use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use ficant_application::ports::{
    AccessScope, AeadCursorCodec, AppendDefinitionVersion, CursorKey, DataSourceRepository,
    DefinitionIdentity, DefinitionRepository, DefinitionValue, IdempotencyKey,
    InstrumentDefinition, IntegrityEvent, IntegrityEventSink, RegisterDataSource, SafeTraceContext,
};
use ficant_application::{
    DataSnapshotPayloads, PublishDataSnapshot, VerifiedReadFacade, VerifiedSnapshotRead,
};
use ficant_data::{
    CANONICAL_QUOTE_SCHEMA_ID, CanonicalIngestRequest, CanonicalQuoteIngestor,
    CanonicalSnapshotCodec, DataResult, InstrumentMapping, InstrumentMappingEntry,
    PointInTimeWindow, RawDecimal, RawQuoteRow, RawQuoteSource, canonical_quote_schema_hash,
};
use ficant_domain::VersionedDefinition;
use ficant_domain::market::{
    Calendar, CalendarInput, CalendarSession, DataSource, DataSourceInput, DataSourceKind,
    Instrument, InstrumentInput, InstrumentKind, Unit, UnitInput,
};
use ficant_domain::primitives::{
    EffectivePeriod, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_storage::postgres::PostgresRepository;
use ficant_storage::s3::S3BlobStore;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn published_snapshot_restarts_and_never_reopens_the_external_source() {
    let pool = connect().await;
    reset_and_migrate(&pool).await;
    let repository = make_repository(pool.clone());
    let owner = owner();
    let scope = scope(&owner);
    let version = Version::new(1).unwrap();
    let currency = Unit::new(UnitInput {
        unit_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F41").unwrap(),
        version,
        owner: owner.clone(),
        code: "CNY".to_owned(),
        dimension: "currency".to_owned(),
        scale: 2,
        precision: 18,
    })
    .unwrap();
    publish_definition(
        &repository,
        DefinitionValue::Unit(currency.clone()),
        "currency",
    )
    .await;
    let price = Unit::new(UnitInput {
        unit_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F40").unwrap(),
        version,
        owner: owner.clone(),
        code: "CNY_CLEAN_PRICE".to_owned(),
        dimension: "price".to_owned(),
        scale: 4,
        precision: 12,
    })
    .unwrap();
    publish_definition(&repository, DefinitionValue::Unit(price.clone()), "price").await;

    let effective = EffectivePeriod::new(
        market_time("2026-07-01T00:00:00Z"),
        market_time("2026-08-01T00:00:00Z"),
    )
    .unwrap();
    let calendar = Calendar::new(CalendarInput {
        calendar_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F30").unwrap(),
        version,
        owner: owner.clone(),
        market: "CGB".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: effective.clone(),
        sessions: vec![
            CalendarSession::open(
                NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
                NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
            )
            .unwrap(),
        ],
    })
    .unwrap();
    publish_definition(
        &repository,
        DefinitionValue::Calendar(calendar.clone()),
        "calendar",
    )
    .await;

    let instrument_ref = VersionRef::new(Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F20").unwrap(), version);
    let instrument = Instrument::new(InstrumentInput {
        instrument_id: instrument_ref.id().clone(),
        version,
        owner: owner.clone(),
        kind: InstrumentKind::Other,
        market: "CGB".to_owned(),
        symbol: "260011.IB".to_owned(),
        currency: UnitRef::new(
            Ulid::new(currency.identity()).unwrap(),
            Version::new(currency.version()).unwrap(),
        ),
        calendar: VersionRef::new(
            Ulid::new(calendar.identity()).unwrap(),
            Version::new(calendar.version()).unwrap(),
        ),
    })
    .unwrap();
    publish_definition(
        &repository,
        DefinitionValue::Instrument(InstrumentDefinition::new(instrument, None).unwrap()),
        "instrument",
    )
    .await;

    let data_source = DataSource::new(DataSourceInput {
        data_source_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F10").unwrap(),
        version,
        owner: owner.clone(),
        kind: DataSourceKind::FileNdjson,
        name: "CGB quotes".to_owned(),
        connection_binding: "phase3b-source".to_owned(),
        dataset: "quotes".to_owned(),
        canonical_schema_id: CANONICAL_QUOTE_SCHEMA_ID.to_owned(),
        canonical_schema_hash: canonical_quote_schema_hash(),
    })
    .unwrap();
    repository
        .register(
            RegisterDataSource::new(scope.clone(), None, data_source.clone(), key("source"))
                .unwrap(),
        )
        .await
        .unwrap();
    let request = CanonicalIngestRequest::new(
        data_source.clone(),
        InstrumentMapping::new(
            owner.clone(),
            VersionRef::new(data_source.id().clone(), version),
            vec![InstrumentMappingEntry::new("260011.IB", effective, instrument_ref).unwrap()],
        )
        .unwrap(),
        calendar,
        price,
        PointInTimeWindow::new(
            market_time("2026-07-20T02:00:00Z"),
            market_time("2026-07-20T02:05:00Z"),
        )
        .unwrap(),
    )
    .unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let canonical = {
        let source = CountingSource {
            calls: Arc::clone(&calls),
        };
        CanonicalQuoteIngestor
            .ingest(&source, &request)
            .await
            .unwrap()
    };
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let package = CanonicalSnapshotCodec
        .build(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F50").unwrap(),
            &request,
            &canonical,
        )
        .unwrap();
    let expected_parquet = package.parquet().to_vec();
    let expected_manifest = package.manifest().to_vec();
    let payloads = DataSnapshotPayloads::new(
        package.snapshot().clone(),
        expected_parquet.clone(),
        expected_manifest.clone(),
        key("snapshot"),
    )
    .unwrap();
    let store = make_store(pool.clone());
    let (published_snapshot, retry) = {
        let publication = PublishDataSnapshot::new(&store, &repository);
        let published_snapshot = publication.execute(&scope, payloads).await.unwrap();
        let retry = publication
            .execute(
                &scope,
                DataSnapshotPayloads::new(
                    package.snapshot().clone(),
                    expected_parquet.clone(),
                    expected_manifest.clone(),
                    key("snapshot"),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        (published_snapshot, retry)
    };
    assert_eq!(published_snapshot, retry);
    drop(store);
    drop(repository);
    pool.close().await;

    let restarted_pool = connect().await;
    let restarted_repository = make_repository(restarted_pool.clone());
    let restarted_store = make_store(restarted_pool.clone());
    let events = RecordingSink::default();
    let reads = VerifiedReadFacade::new(
        &restarted_repository,
        &restarted_repository,
        &restarted_repository,
        &restarted_store,
        &events,
    );
    let required = reads
        .read_verified_snapshot(
            &scope,
            published_snapshot.id().clone(),
            SafeTraceContext::new("0123456789abcdef0123456789abcdef").unwrap(),
        )
        .await
        .unwrap();
    let VerifiedSnapshotRead::Data {
        snapshot,
        parquet,
        manifest,
    } = required
    else {
        panic!("expected DataSnapshot");
    };
    assert_eq!(parquet.bytes(), expected_parquet);
    assert_eq!(manifest.bytes(), expected_manifest);
    let verified = CanonicalSnapshotCodec
        .decode_verified(snapshot, parquet.bytes(), manifest.bytes())
        .unwrap();
    assert_eq!(verified.batch(), canonical.batch());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(events.0.lock().unwrap().is_empty());
    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM research.data_snapshots),
           (SELECT COUNT(*) FROM research.lineage_edges),
           (SELECT COUNT(*) FROM storage.blobs),
           (SELECT COUNT(*) FROM storage.orphan_candidates),
           (SELECT COUNT(*) FROM storage.staging_uploads)",
    )
    .fetch_one(&restarted_pool)
    .await
    .unwrap();
    assert_eq!(counts, (1, 4, 2, 0, 0));
}

struct CountingSource {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl RawQuoteSource for CountingSource {
    async fn read(
        &self,
        _source: &DataSource,
        _window: &PointInTimeWindow,
    ) -> DataResult<Vec<RawQuoteRow>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![RawQuoteRow::new(
            "record-1",
            "260011.IB",
            "2026-07-20T01:30:00Z",
            "2026-07-20T01:30:05Z",
            Some(RawDecimal::new("1012300", 4)),
            Some(RawDecimal::new("1012500", 4)),
        )])
    }
}

#[derive(Default)]
struct RecordingSink(Mutex<Vec<IntegrityEvent>>);

#[async_trait]
impl IntegrityEventSink for RecordingSink {
    async fn emit(
        &self,
        event: IntegrityEvent,
    ) -> Result<(), ficant_application::ApplicationError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

async fn publish_definition(repository: &PostgresRepository, value: DefinitionValue, label: &str) {
    repository
        .create_identity(DefinitionIdentity::new(
            Ulid::new(value.identity()).unwrap(),
            value.owner().clone(),
            value.kind(),
            key(&format!("{label}-identity")),
        ))
        .await
        .unwrap();
    repository
        .append_version(
            AppendDefinitionVersion::new(None, value, key(&format!("{label}-version"))).unwrap(),
        )
        .await
        .unwrap();
}

async fn connect() -> PgPool {
    PgPoolOptions::new()
        .max_connections(8)
        .connect(&env::var("FICANT_TEST_DATABASE_URL").expect("disposable PostgreSQL is required"))
        .await
        .unwrap()
}

async fn reset_and_migrate(pool: &PgPool) {
    sqlx::raw_sql(
        "DROP SCHEMA IF EXISTS data CASCADE;
         DROP SCHEMA IF EXISTS storage CASCADE;
         DROP SCHEMA IF EXISTS research CASCADE;
         DROP SCHEMA IF EXISTS market CASCADE;
         DROP SCHEMA IF EXISTS core CASCADE;
         DROP TABLE IF EXISTS public._sqlx_migrations;",
    )
    .execute(pool)
    .await
    .unwrap();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations/postgresql");
    sqlx::migrate::Migrator::new(path)
        .await
        .unwrap()
        .run(pool)
        .await
        .unwrap();
}

fn make_repository(pool: PgPool) -> PostgresRepository {
    let cursor =
        AeadCursorCodec::new(CursorKey::new("phase3b", [53_u8; 32]).unwrap(), vec![]).unwrap();
    PostgresRepository::new(pool, Arc::new(cursor))
}

fn make_store(pool: PgPool) -> S3BlobStore {
    S3BlobStore::new(
        &env::var("FICANT_TEST_S3_ENDPOINT").expect("Ceph RGW endpoint is required"),
        env::var("FICANT_TEST_S3_BUCKET").expect("isolated bucket is required"),
        &env::var("FICANT_TEST_S3_ACCESS_KEY").expect("Ceph access key is required"),
        &env::var("FICANT_TEST_S3_SECRET_KEY").expect("Ceph secret key is required"),
        pool,
    )
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    )
}

fn scope(owner: &OwnerRef) -> AccessScope {
    AccessScope::new(
        owner.tenant_id().clone(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F00").unwrap(),
        vec![owner.owner_id().clone()],
    )
    .unwrap()
}

fn key(label: &str) -> IdempotencyKey {
    IdempotencyKey::new(format!("phase3b/{label}")).unwrap()
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
