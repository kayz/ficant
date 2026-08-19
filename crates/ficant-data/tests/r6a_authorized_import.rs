use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use ficant_application::ports::data_source_content_hash;
use ficant_application::rates_data_source_content_hash;
use ficant_data::{
    CANONICAL_QUOTE_SCHEMA_ID, DataError, DataResult, GovernedCanonicalImportRequest,
    GovernedCanonicalQuoteImporter, InstrumentMapping, InstrumentMappingEntry, PointInTimeWindow,
    QuoteSourceCatalog, RawDecimal, RawQuoteRow, RawQuoteSource, RegisteredQuoteSource,
    canonical_data_source_content_hash, canonical_quote_schema_hash,
};
use ficant_domain::ContentAddressed;
use ficant_domain::Lineaged;
use ficant_domain::market::{
    Calendar, CalendarInput, CalendarSession, DataSource, DataSourceAuthorization,
    DataSourceAuthorizationInput, DataSourceAuthorizationState, DataSourceInput, DataSourceKind,
    ImportInterface, PriceSourceType, Unit, UnitInput,
    data_source_content_hash as domain_data_source_content_hash,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};

#[tokio::test]
async fn exact_authorization_precedes_adapter_and_enters_manifest_and_lineage() {
    let fixture = fixture();
    let reads = Arc::new(AtomicUsize::new(0));
    let source = Arc::new(CountingSource {
        reads: reads.clone(),
        rows: rows(),
    });
    let catalog = QuoteSourceCatalog::new(vec![
        RegisteredQuoteSource::new(DataSourceKind::FileNdjson, "admin-file-binding", source)
            .unwrap(),
    ])
    .unwrap();
    let request = fixture.request(fixture.authorization.clone()).unwrap();
    let prepared = GovernedCanonicalQuoteImporter::new(&catalog)
        .prepare(request)
        .await
        .unwrap();

    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        prepared.authorization(),
        &fixture.authorization.version_ref()
    );
    assert_eq!(
        prepared.authorization_hash(),
        fixture.authorization.content_hash()
    );
    assert_eq!(prepared.import_reason(), "daily governed CGB quote import");
    assert_eq!(prepared.package().snapshot().lineage().len(), 6);
    let manifest = std::str::from_utf8(prepared.package().manifest()).unwrap();
    assert!(manifest.contains(fixture.authorization.id().as_str()));
    assert!(manifest.contains(fixture.mapping.id().as_str()));
    assert!(manifest.contains(ACTOR_ID));
    assert!(!manifest.contains("admin-file-binding"));
    assert!(!manifest.contains("postgres://"));
    assert_eq!(
        prepared.package().snapshot().content_hash(),
        &ContentHash::digest(prepared.package().parquet())
    );
}

#[tokio::test]
async fn every_authorization_drift_fails_before_source_read() {
    let fixture = fixture();
    let reads = Arc::new(AtomicUsize::new(0));
    let catalog = QuoteSourceCatalog::new(vec![
        RegisteredQuoteSource::new(
            DataSourceKind::FileNdjson,
            "admin-file-binding",
            Arc::new(CountingSource {
                reads: reads.clone(),
                rows: rows(),
            }),
        )
        .unwrap(),
    ])
    .unwrap();

    let wrong_mapping = authorization(
        &fixture.source,
        fixture.mapping.id().clone(),
        ContentHash::from_bytes(&[0xA5; 32]).unwrap(),
        active_period(),
        DataSourceAuthorizationState::Active,
        Version::new(1).unwrap(),
    );
    assert_eq!(
        fixture.request(wrong_mapping).unwrap_err(),
        DataError::InvalidConfiguration
    );

    let expired = authorization(
        &fixture.source,
        fixture.mapping.id().clone(),
        fixture.mapping.content_hash().clone(),
        EffectivePeriod::new(
            market_time("2026-06-01T00:00:00Z"),
            market_time("2026-07-01T00:00:00Z"),
        )
        .unwrap(),
        DataSourceAuthorizationState::Active,
        Version::new(1).unwrap(),
    );
    assert_eq!(
        fixture.request(expired).unwrap_err(),
        DataError::InvalidConfiguration
    );

    let revoked = authorization(
        &fixture.source,
        fixture.mapping.id().clone(),
        fixture.mapping.content_hash().clone(),
        active_period(),
        DataSourceAuthorizationState::Revoked,
        Version::new(2).unwrap(),
    );
    assert_eq!(
        fixture.request(revoked).unwrap_err(),
        DataError::InvalidConfiguration
    );

    let drifted_source = data_source("different-admin-binding");
    let result = GovernedCanonicalImportRequest::new(
        snapshot_id(),
        id(ACTOR_ID),
        authorized_at(),
        fixture.authorization,
        drifted_source,
        fixture.mapping,
        fixture.calendar,
        fixture.unit,
        window(),
        "daily governed CGB quote import",
    );
    assert_eq!(result.unwrap_err(), DataError::InvalidConfiguration);
    assert_eq!(reads.load(Ordering::SeqCst), 0);

    // No invalid request can reach catalog resolution or the adapter.
    drop(catalog);
}

#[test]
fn catalog_rejects_duplicate_admin_bindings_and_data_source_hash_matches_r5d() {
    let sources = [
        data_source("admin-file-binding"),
        data_source("admin-file-binding")
            .with_price_source_type(PriceSourceType::RealTrade)
            .unwrap(),
        data_source("admin-file-binding")
            .with_price_source_type(PriceSourceType::ActiveQuote)
            .unwrap(),
        data_source("admin-file-binding")
            .with_price_source_type(PriceSourceType::ModelValuation)
            .unwrap(),
    ];
    let observed = sources
        .iter()
        .map(|source| {
            let canonical = canonical_data_source_content_hash(source);
            assert_eq!(canonical, rates_data_source_content_hash(source));
            assert_eq!(canonical, data_source_content_hash(source));
            assert_eq!(canonical, domain_data_source_content_hash(source));
            hash_hex(&canonical)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            "703ede1e0d0b0850747aaaf70909c5b9b9d29a269909b9b52712fe821ba2414d",
            "be6abf8c709ce85e1efddc70a0df30bd78d6b58d819543344b50b8e7ff598c1b",
            "bb18ee670d817b97c95277ae5ebf2244a4fc87e50c23d51cad7f56b2cff70331",
            "19cd024078c7331305f5adf26b2623c9e640941c84c2649bf0725e0de2741b7c",
        ]
    );
    assert_eq!(
        data_source("admin-file-binding")
            .with_price_source_type(PriceSourceType::CurveInterpolation)
            .unwrap_err(),
        ficant_domain::DomainErrorCode::InvalidValue,
    );
    let adapter: Arc<dyn RawQuoteSource> = Arc::new(CountingSource {
        reads: Arc::new(AtomicUsize::new(0)),
        rows: rows(),
    });
    let duplicate = QuoteSourceCatalog::new(vec![
        RegisteredQuoteSource::new(
            DataSourceKind::FileNdjson,
            "admin-file-binding",
            adapter.clone(),
        )
        .unwrap(),
        RegisteredQuoteSource::new(DataSourceKind::FileNdjson, "admin-file-binding", adapter)
            .unwrap(),
    ]);
    assert!(matches!(duplicate, Err(DataError::InvalidConfiguration)));
}

fn hash_hex(value: &ContentHash) -> String {
    let mut result = String::with_capacity(value.as_bytes().len() * 2);
    for byte in value.as_bytes() {
        write!(&mut result, "{byte:02x}").expect("writing to String cannot fail");
    }
    result
}

#[test]
fn mapping_hash_binds_nanoseconds_timezone_and_local_trading_date() {
    let source = VersionRef::new(id(SOURCE_ID), Version::new(1).unwrap());
    let start = Utc.with_ymd_and_hms(2026, 7, 1, 18, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 8, 1, 18, 0, 0).unwrap();
    let mapping = |start: DateTime<Utc>, timezone: &str| {
        let timezone_value = timezone.parse::<chrono_tz::Tz>().unwrap();
        InstrumentMapping::new(
            id(MAPPING_ID),
            owner(),
            source.clone(),
            vec![
                InstrumentMappingEntry::new(
                    "260011.IB",
                    EffectivePeriod::new(
                        MarketTime::new(
                            start,
                            timezone,
                            start.with_timezone(&timezone_value).date_naive(),
                        )
                        .unwrap(),
                        MarketTime::new(
                            end,
                            timezone,
                            end.with_timezone(&timezone_value).date_naive(),
                        )
                        .unwrap(),
                    )
                    .unwrap(),
                    VersionRef::new(id(INSTRUMENT_ID), Version::new(7).unwrap()),
                )
                .unwrap(),
            ],
        )
        .unwrap()
        .contract_hash()
    };
    let baseline = mapping(start, "Asia/Shanghai");
    let nanosecond = mapping(start + chrono::Duration::nanoseconds(1), "Asia/Shanghai");
    // The same instant is 2026-07-02 in Asia/Shanghai and 2026-07-01 in UTC, so this
    // simultaneously proves timezone and canonical local-trading-date evidence is bound.
    let utc_timezone_and_local_date = mapping(start, "UTC");
    assert_ne!(baseline, nanosecond);
    assert_ne!(baseline, utc_timezone_and_local_date);
}

struct Fixture {
    source: DataSource,
    mapping: InstrumentMapping,
    calendar: Calendar,
    unit: Unit,
    authorization: DataSourceAuthorization,
}

impl Fixture {
    fn request(
        &self,
        authorization: DataSourceAuthorization,
    ) -> DataResult<GovernedCanonicalImportRequest> {
        GovernedCanonicalImportRequest::new(
            snapshot_id(),
            id(ACTOR_ID),
            authorized_at(),
            authorization,
            self.source.clone(),
            self.mapping.clone(),
            self.calendar.clone(),
            self.unit.clone(),
            window(),
            "daily governed CGB quote import",
        )
    }
}

fn fixture() -> Fixture {
    let owner = owner();
    let source = data_source("admin-file-binding");
    let mapping = InstrumentMapping::new(
        id(MAPPING_ID),
        owner.clone(),
        VersionRef::new(source.id().clone(), Version::new(1).unwrap()),
        vec![
            InstrumentMappingEntry::new(
                "260011.IB",
                active_period(),
                VersionRef::new(id(INSTRUMENT_ID), Version::new(7).unwrap()),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let calendar = Calendar::new(CalendarInput {
        calendar_id: id(CALENDAR_ID),
        version: Version::new(3).unwrap(),
        owner: owner.clone(),
        market: "CGB".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: active_period(),
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
    let unit = Unit::new(UnitInput {
        unit_id: id(UNIT_ID),
        version: Version::new(2).unwrap(),
        owner,
        code: "CNY_CLEAN_PRICE".to_owned(),
        dimension: "price".to_owned(),
        scale: 4,
        precision: 12,
    })
    .unwrap();
    let authorization = authorization(
        &source,
        mapping.id().clone(),
        mapping.content_hash().clone(),
        active_period(),
        DataSourceAuthorizationState::Active,
        Version::new(1).unwrap(),
    );
    Fixture {
        source,
        mapping,
        calendar,
        unit,
        authorization,
    }
}

fn authorization(
    source: &DataSource,
    mapping_id: Ulid,
    mapping_hash: ContentHash,
    effective: EffectivePeriod,
    state: DataSourceAuthorizationState,
    version: Version,
) -> DataSourceAuthorization {
    let authorization_id = id(AUTHORIZATION_ID);
    DataSourceAuthorization::new(DataSourceAuthorizationInput {
        authorization_id: authorization_id.clone(),
        version,
        owner: owner(),
        data_source: VersionRef::new(source.id().clone(), Version::new(1).unwrap()),
        data_source_hash: canonical_data_source_content_hash(source),
        import_interface: ImportInterface::CanonicalQuoteSnapshot,
        canonical_schema_id: CANONICAL_QUOTE_SCHEMA_ID.to_owned(),
        canonical_schema_hash: canonical_quote_schema_hash(),
        effective,
        state,
        supersedes: (version.get() > 1)
            .then(|| VersionRef::new(authorization_id, Version::new(version.get() - 1).unwrap())),
        mapping_id,
        mapping_hash,
    })
    .unwrap()
}

fn data_source(binding: &str) -> DataSource {
    DataSource::new(DataSourceInput {
        data_source_id: id(SOURCE_ID),
        version: Version::new(1).unwrap(),
        owner: owner(),
        kind: DataSourceKind::FileNdjson,
        name: "CGB quotes".to_owned(),
        connection_binding: binding.to_owned(),
        dataset: "quotes".to_owned(),
        canonical_schema_id: CANONICAL_QUOTE_SCHEMA_ID.to_owned(),
        canonical_schema_hash: canonical_quote_schema_hash(),
    })
    .unwrap()
}

#[derive(Clone)]
struct CountingSource {
    reads: Arc<AtomicUsize>,
    rows: Vec<RawQuoteRow>,
}

#[async_trait]
impl RawQuoteSource for CountingSource {
    async fn read(
        &self,
        _source: &DataSource,
        _window: &PointInTimeWindow,
    ) -> DataResult<Vec<RawQuoteRow>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.rows.clone())
    }
}

fn rows() -> Vec<RawQuoteRow> {
    vec![RawQuoteRow::new(
        "record-1",
        "260011.IB",
        "2026-07-20T02:00:00Z",
        "2026-07-20T02:00:01Z",
        Some(RawDecimal::new("1010000", 4)),
        Some(RawDecimal::new("1010100", 4)),
    )]
}

fn active_period() -> EffectivePeriod {
    EffectivePeriod::new(
        market_time("2026-07-01T00:00:00Z"),
        market_time("2026-08-01T00:00:00Z"),
    )
    .unwrap()
}

fn window() -> PointInTimeWindow {
    PointInTimeWindow::new(
        market_time("2026-07-20T02:00:00Z"),
        market_time("2026-07-20T02:05:00Z"),
    )
    .unwrap()
}

fn authorized_at() -> MarketTime {
    market_time("2026-07-20T03:00:00Z")
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
    OwnerRef::new(id(TENANT_ID), id(OWNER_ID))
}

fn snapshot_id() -> Ulid {
    id(SNAPSHOT_ID)
}

fn id(value: &str) -> Ulid {
    Ulid::new(value).unwrap()
}

const ACTOR_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5F00";
const TENANT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5F01";
const OWNER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5F02";
const SOURCE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5F10";
const INSTRUMENT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5F20";
const CALENDAR_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5F30";
const UNIT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5F40";
const SNAPSHOT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5F50";
const MAPPING_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5F60";
const AUTHORIZATION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5F70";
