use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use ficant_application::ports::{
    AccessScope, AeadCursorCodec, AppendDefinitionVersion, AuthorizedPrincipal,
    CanonicalImportManifestEvidence, CanonicalImportReplayRequest, CursorKey,
    DataSourceAuthorizationRepository, DataSourceRepository, DefinitionIdentity,
    DefinitionRepository, DefinitionValue, FoundationChangeContext, IdempotencyKey,
    InstrumentDefinition, IntegrityEvent, IntegrityEventSink, PublishDataSourceAuthorization,
    RegisterDataSource, SafeTraceContext, data_source_content_hash,
};
use ficant_application::{
    DataSnapshotPayloads, PublishDataSnapshot, VerifiedReadFacade, VerifiedSnapshotRead,
};
use ficant_data::{
    CANONICAL_QUOTE_SCHEMA_ID, CanonicalImportEvidence, CanonicalIngestRequest,
    CanonicalQuoteIngestor, CanonicalSnapshotCodec, DataError, DataResult, InstrumentMapping,
    InstrumentMappingEntry, PointInTimeWindow, RawDecimal, RawQuoteRow, RawQuoteSource,
    canonical_quote_schema_hash,
};
use ficant_domain::VersionedDefinition;
use ficant_domain::governance::{ChangeJustification, PlatformRole, SourceDocumentRef};
use ficant_domain::market::{
    Calendar, CalendarInput, CalendarSession, DataSource, DataSourceAuthorization,
    DataSourceAuthorizationInput, DataSourceAuthorizationState, DataSourceInput, DataSourceKind,
    ImportInterface, Instrument, InstrumentInput, InstrumentKind, Unit, UnitInput,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
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
            RegisterDataSource::new(
                admin_change(&owner),
                None,
                data_source.clone(),
                key("source"),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let mapping = InstrumentMapping::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F60").unwrap(),
        owner.clone(),
        VersionRef::new(data_source.id().clone(), version),
        vec![InstrumentMappingEntry::new("260011.IB", effective, instrument_ref).unwrap()],
    )
    .unwrap();
    let authorization = DataSourceAuthorization::new(DataSourceAuthorizationInput {
        authorization_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F70").unwrap(),
        version,
        owner: owner.clone(),
        data_source: VersionRef::new(data_source.id().clone(), version),
        data_source_hash: data_source_content_hash(&data_source),
        import_interface: ImportInterface::CanonicalQuoteSnapshot,
        canonical_schema_id: data_source.canonical_schema_id().to_owned(),
        canonical_schema_hash: data_source.canonical_schema_hash().clone(),
        effective: EffectivePeriod::new(
            market_time("2026-01-01T00:00:00Z"),
            market_time("2027-01-01T00:00:00Z"),
        )
        .unwrap(),
        state: DataSourceAuthorizationState::Active,
        supersedes: None,
        mapping_id: mapping.id().clone(),
        mapping_hash: mapping.content_hash().clone(),
    })
    .unwrap();
    repository
        .publish_authorization(
            PublishDataSourceAuthorization::new(
                admin_change_for(
                    &owner,
                    "01ARZ3NDEKTSV4RRFFQ69G5F81",
                    "authorize Phase 3B deterministic import",
                ),
                None,
                authorization.clone(),
                key("authorization"),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let request = CanonicalIngestRequest::new(
        data_source.clone(),
        mapping.clone(),
        calendar.clone(),
        price.clone(),
        PointInTimeWindow::new(
            market_time("2026-07-20T02:00:00Z"),
            market_time("2026-07-20T02:05:00Z"),
        )
        .unwrap(),
    )
    .unwrap();

    let source = DestroyableSource::new();
    let canonical = CanonicalQuoteIngestor
        .ingest(&source, &request)
        .await
        .unwrap();
    assert_eq!(source.call_count(), 1);
    let actor_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F00").unwrap();
    let import_evidence = CanonicalImportEvidence::new(
        authorization.version_ref(),
        authorization.content_hash().clone(),
        actor_id.clone(),
    );
    let package = CanonicalSnapshotCodec
        .build_authorized(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F50").unwrap(),
            &request,
            &canonical,
            &import_evidence,
        )
        .unwrap();
    let expected_parquet = package.parquet().to_vec();
    let expected_manifest = package.manifest().to_vec();
    let base_key = key("snapshot");
    let replay_request = CanonicalImportReplayRequest::new(
        authorized_import_change(&owner),
        owner.clone(),
        package.snapshot().id().clone(),
        authorization.version_ref(),
        authorization.content_hash().clone(),
        mapping.id().clone(),
        mapping.content_hash().clone(),
        VersionRef::new(
            Ulid::new(calendar.identity()).unwrap(),
            Version::new(calendar.version()).unwrap(),
        ),
        VersionRef::new(
            Ulid::new(price.identity()).unwrap(),
            Version::new(price.version()).unwrap(),
        ),
        request.window().as_of().clone(),
        request.window().visible_at_cutoff().clone(),
        base_key.clone(),
    )
    .unwrap();
    let payloads = DataSnapshotPayloads::new_authorized(
        package.snapshot().clone(),
        expected_parquet.clone(),
        expected_manifest.clone(),
        base_key,
        CanonicalImportManifestEvidence::new(
            actor_id,
            authorization.version_ref(),
            authorization.content_hash().clone(),
        ),
    )
    .unwrap();
    let store = make_store(pool.clone());
    let (published_snapshot, retry) = {
        let publication = PublishDataSnapshot::new(&store, &repository);
        let published_snapshot = publication
            .execute_governed_import(replay_request.clone(), payloads)
            .await
            .unwrap();
        let retry = publication
            .probe_replay(&replay_request)
            .await
            .unwrap()
            .unwrap()
            .snapshot()
            .clone();
        (published_snapshot, retry)
    };
    assert_eq!(published_snapshot, retry);
    source.destroy();
    assert_eq!(
        source
            .read(&data_source, request.window())
            .await
            .unwrap_err(),
        DataError::SourceUnavailable,
        "the external source contents must be physically unavailable before restart"
    );
    let calls_after_destroy_proof = source.call_count();
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
    assert_eq!(verified.batch().schema(), canonical.batch().schema());
    assert_eq!(verified.batch().num_rows(), canonical.batch().num_rows());
    assert_eq!(
        verified.batch().num_columns(),
        canonical.batch().num_columns()
    );
    assert_eq!(verified.batch(), canonical.batch());
    assert_eq!(source.call_count(), calls_after_destroy_proof);
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
    assert_eq!(counts, (1, 6, 2, 0, 0));
}

#[derive(Clone)]
struct DestroyableSource {
    calls: Arc<AtomicUsize>,
    rows: Arc<Mutex<Option<Vec<RawQuoteRow>>>>,
}

impl DestroyableSource {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            rows: Arc::new(Mutex::new(Some(vec![RawQuoteRow::new(
                "record-1",
                "260011.IB",
                "2026-07-20T01:30:00Z",
                "2026-07-20T01:30:05Z",
                Some(RawDecimal::new("1012300", 4)),
                Some(RawDecimal::new("1012500", 4)),
            )]))),
        }
    }

    fn destroy(&self) {
        self.rows.lock().unwrap().take();
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl RawQuoteSource for DestroyableSource {
    async fn read(
        &self,
        _source: &DataSource,
        _window: &PointInTimeWindow,
    ) -> DataResult<Vec<RawQuoteRow>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.rows
            .lock()
            .unwrap()
            .clone()
            .ok_or(DataError::SourceUnavailable)
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

fn admin_change(owner: &OwnerRef) -> FoundationChangeContext {
    admin_change_for(
        owner,
        "01ARZ3NDEKTSV4RRFFQ69G5F80",
        "Phase 3B deterministic source fixture",
    )
}

fn admin_change_for(owner: &OwnerRef, record_id: &str, reason: &str) -> FoundationChangeContext {
    let principal = AuthorizedPrincipal::new(
        "phase3b-admin".to_owned(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F00").unwrap(),
        owner.tenant_id().clone(),
        vec![owner.owner_id().clone()],
        PlatformRole::PlatformAdmin,
        vec!["data-sources:write".to_owned()],
        ContentHash::digest(b"phase3b-admin-credential"),
    )
    .unwrap();
    FoundationChangeContext::administrator(
        principal,
        ChangeJustification::new(
            reason,
            vec![
                SourceDocumentRef::new(
                    "fixture://phase3b/source",
                    ContentHash::digest(b"phase3b-source-fixture"),
                )
                .unwrap(),
            ],
        )
        .unwrap(),
        Ulid::new(record_id).unwrap(),
        market_time("2026-07-20T03:00:00Z"),
    )
    .unwrap()
}

fn authorized_import_change(owner: &OwnerRef) -> FoundationChangeContext {
    FoundationChangeContext::authorized_import(
        AuthorizedPrincipal::new(
            "phase3b-researcher".to_owned(),
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F00").unwrap(),
            owner.tenant_id().clone(),
            vec![owner.owner_id().clone()],
            PlatformRole::Researcher,
            vec!["data-sources:import".to_owned()],
            ContentHash::digest(b"phase3b-researcher-credential"),
        )
        .unwrap(),
        ChangeJustification::for_authorized_import("publish Phase 3B canonical import").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F82").unwrap(),
        market_time("2026-07-20T03:00:00Z"),
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
