use std::path::PathBuf;

use arrow::array::{Array, StringArray, TimestampMicrosecondArray};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use ficant_data::{
    CANONICAL_QUOTE_SCHEMA_ID, CanonicalIngestRequest, CanonicalQuoteIngestor, DataError,
    DataResult, FileNdjsonQuoteSource, InstrumentMapping, InstrumentMappingEntry,
    PointInTimeWindow, RawDecimal, RawQuoteRow, RawQuoteSource, canonical_quote_schema,
    canonical_quote_schema_hash,
};
use ficant_domain::market::{
    Calendar, CalendarInput, CalendarSession, DataSource, DataSourceInput, DataSourceKind, Unit,
    UnitInput,
};
use ficant_domain::primitives::{EffectivePeriod, MarketTime, OwnerRef, Ulid, Version, VersionRef};

#[tokio::test]
async fn canonical_batch_is_exact_sorted_and_quality_bound() {
    let request = request(DataSourceKind::FileNdjson, "file-binding", "quotes");
    let adapter = MemorySource(vec![
        row(
            "record-2",
            "260011.IB",
            "2026-07-20T01:31:00Z",
            "2026-07-20T01:31:05Z",
            Some(("1012400", 4)),
            Some(("1012600", 4)),
        ),
        row(
            "record-1",
            "260011.IB",
            "2026-07-20T01:30:00Z",
            "2026-07-20T01:30:05Z",
            Some(("1012300", 4)),
            Some(("1012500", 4)),
        ),
    ]);
    let result = CanonicalQuoteIngestor
        .ingest(&adapter, &request)
        .await
        .unwrap();
    assert_eq!(result.batch().num_columns(), 16);
    assert_eq!(result.batch().num_rows(), 2);
    assert_eq!(result.batch().schema().as_ref(), &canonical_quote_schema());
    assert_eq!(result.schema_hash(), &canonical_quote_schema_hash());
    assert_eq!(
        result.quality().rule_set_id(),
        "ficant.market.quote.quality.v1"
    );
    assert_eq!(result.quality().validated_rows(), 2);
    assert_eq!(result.quality().emitted_rows(), 2);

    let record_ids = result
        .batch()
        .column(4)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(record_ids.value(0), "record-1");
    assert_eq!(record_ids.value(1), "record-2");
    let observed = result
        .batch()
        .column(7)
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .unwrap();
    assert!(observed.value(0) < observed.value(1));
    let bid = result
        .batch()
        .column(10)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(bid.value(0), "10123");
}

#[test]
fn canonical_schema_freezes_all_sixteen_fields_and_metadata() {
    use std::fmt::Write as _;

    let schema = canonical_quote_schema();
    let schema_hash = canonical_quote_schema_hash().as_bytes().iter().fold(
        String::with_capacity(64),
        |mut encoded, byte| {
            write!(encoded, "{byte:02x}").unwrap();
            encoded
        },
    );
    assert_eq!(
        schema_hash,
        "e804a0becec18e51dde1be4250384ffe667cf4149c34dc3d2cfc82a206d71502"
    );
    let names = schema
        .fields()
        .iter()
        .map(|field| field.name().as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "tenant_id",
            "owner_id",
            "data_source_id",
            "data_source_version",
            "source_record_id",
            "instrument_id",
            "instrument_version",
            "observed_at",
            "visible_at",
            "local_trading_date",
            "bid_coefficient",
            "bid_scale",
            "ask_coefficient",
            "ask_scale",
            "unit_id",
            "unit_version",
        ]
    );
    assert_eq!(
        schema.metadata()["ficant.schema.id"],
        CANONICAL_QUOTE_SCHEMA_ID
    );
    assert_eq!(schema.metadata()["ficant.market.timezone"], "Asia/Shanghai");
    assert_eq!(
        schema.metadata()["ficant.decimal.encoding"],
        "coefficient+scale"
    );
    assert!(schema.field(10).is_nullable() && schema.field(11).is_nullable());
    assert!(schema.field(12).is_nullable() && schema.field(13).is_nullable());
}

#[tokio::test]
async fn reversed_bitemporal_row_carries_safe_exact_source_identity_and_closed_reason() {
    let request = request(DataSourceKind::FileNdjson, "file-binding", "quotes");
    let error = ingest_error(
        &request,
        vec![row(
            "source-row-17",
            "260011.IB",
            "2026-07-20T01:31:00Z",
            "2026-07-20T01:30:00Z",
            Some(("10123", 2)),
            None,
        )],
    )
    .await;

    assert!(matches!(error, DataError::SourceRowViolation { .. }));
    assert_eq!(
        error.observed_after_visible_source_record_id(),
        Some("source-row-17")
    );
}

#[tokio::test]
async fn every_point_in_time_and_quality_violation_fails_the_whole_batch() {
    let request = request(DataSourceKind::FileNdjson, "file-binding", "quotes");
    let invalid_rows = [
        row(
            "duplicate",
            "260011.IB",
            "2026-07-20T01:30:00Z",
            "2026-07-20T01:30:05Z",
            Some(("10123", 2)),
            None,
        ),
        row(
            "duplicate",
            "260011.IB",
            "2026-07-20T01:31:00Z",
            "2026-07-20T01:31:05Z",
            Some(("10124", 2)),
            None,
        ),
    ];
    assert_eq!(
        ingest_error(&request, invalid_rows.to_vec()).await,
        DataError::QualityRuleFailed
    );

    let cases = [
        row(
            "late",
            "260011.IB",
            "2026-07-20T01:30:00Z",
            "2026-07-20T03:00:00Z",
            Some(("10123", 2)),
            None,
        ),
        row(
            "reversed-time",
            "260011.IB",
            "2026-07-20T01:31:00Z",
            "2026-07-20T01:30:00Z",
            Some(("10123", 2)),
            None,
        ),
        row(
            "unmapped",
            "missing",
            "2026-07-20T01:30:00Z",
            "2026-07-20T01:30:05Z",
            Some(("10123", 2)),
            None,
        ),
        row(
            "outside-session",
            "260011.IB",
            "2026-07-20T00:30:00Z",
            "2026-07-20T00:30:05Z",
            Some(("10123", 2)),
            None,
        ),
        row(
            "no-side",
            "260011.IB",
            "2026-07-20T01:30:00Z",
            "2026-07-20T01:30:05Z",
            None,
            None,
        ),
        row(
            "crossed",
            "260011.IB",
            "2026-07-20T01:30:00Z",
            "2026-07-20T01:30:05Z",
            Some(("10126", 2)),
            Some(("10125", 2)),
        ),
        row(
            "scale",
            "260011.IB",
            "2026-07-20T01:30:00Z",
            "2026-07-20T01:30:05Z",
            Some(("101230", 5)),
            None,
        ),
        row(
            "float",
            "260011.IB",
            "2026-07-20T01:30:00Z",
            "2026-07-20T01:30:05Z",
            Some(("101.23", 2)),
            None,
        ),
    ];
    for case in cases {
        assert!(matches!(
            ingest_error(&request, vec![case]).await,
            DataError::PointInTimeViolation
                | DataError::QualityRuleFailed
                | DataError::SourceRowViolation { .. }
        ));
    }
}

#[tokio::test]
async fn file_adapter_filters_late_visibility_without_guessing_current_time() {
    let root = temp_root();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("quotes.ndjson"),
        concat!(
            "{\"ask_coefficient\":\"10125\",\"ask_scale\":2,\"bid_coefficient\":\"10123\",\"bid_scale\":2,\"instrument_key\":\"260011.IB\",\"observed_at\":\"2026-07-20T01:30:00Z\",\"source_record_id\":\"visible\",\"visible_at\":\"2026-07-20T01:30:05Z\"}\n",
            "{\"ask_coefficient\":\"10126\",\"ask_scale\":2,\"bid_coefficient\":\"10124\",\"bid_scale\":2,\"instrument_key\":\"260011.IB\",\"observed_at\":\"2026-07-20T01:31:00Z\",\"source_record_id\":\"late\",\"visible_at\":\"2026-07-20T03:00:00Z\"}\n"
        ),
    )
    .unwrap();
    let request = request(DataSourceKind::FileNdjson, "file-binding", "quotes");
    let adapter = FileNdjsonQuoteSource::new("file-binding", root.clone()).unwrap();
    let result = CanonicalQuoteIngestor
        .ingest(&adapter, &request)
        .await
        .unwrap();
    assert_eq!(result.batch().num_rows(), 1);
    let ids = result
        .batch()
        .column(4)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(ids.value(0), "visible");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn adapter_returning_out_of_window_rows_is_rejected_not_silently_filtered() {
    let request = request(DataSourceKind::FileNdjson, "file-binding", "quotes");
    let adapter = MemorySource(vec![row(
        "late",
        "260011.IB",
        "2026-07-20T01:30:00Z",
        "2026-07-20T03:00:00Z",
        Some(("10123", 2)),
        None,
    )]);
    assert_eq!(
        CanonicalQuoteIngestor
            .ingest(&adapter, &request)
            .await
            .unwrap_err(),
        DataError::PointInTimeViolation
    );
}

async fn ingest_error(request: &CanonicalIngestRequest, rows: Vec<RawQuoteRow>) -> DataError {
    CanonicalQuoteIngestor
        .ingest(&MemorySource(rows), request)
        .await
        .unwrap_err()
}

#[derive(Clone)]
struct MemorySource(Vec<RawQuoteRow>);

#[async_trait]
impl RawQuoteSource for MemorySource {
    async fn read(
        &self,
        _source: &DataSource,
        _window: &PointInTimeWindow,
    ) -> DataResult<Vec<RawQuoteRow>> {
        Ok(self.0.clone())
    }
}

fn request(kind: DataSourceKind, binding: &str, dataset: &str) -> CanonicalIngestRequest {
    let owner = owner();
    let source = DataSource::new(DataSourceInput {
        data_source_id: Ulid::new(match kind {
            DataSourceKind::FileNdjson => "01ARZ3NDEKTSV4RRFFQ69G5F10",
            DataSourceKind::Postgres => "01ARZ3NDEKTSV4RRFFQ69G5F11",
        })
        .unwrap(),
        version: Version::new(1).unwrap(),
        owner: owner.clone(),
        kind,
        name: "CGB quotes".to_owned(),
        connection_binding: binding.to_owned(),
        dataset: dataset.to_owned(),
        canonical_schema_id: CANONICAL_QUOTE_SCHEMA_ID.to_owned(),
        canonical_schema_hash: canonical_quote_schema_hash(),
    })
    .unwrap();
    let effective = EffectivePeriod::new(
        market_time("2026-07-01T00:00:00Z"),
        market_time("2026-08-01T00:00:00Z"),
    )
    .unwrap();
    let mapping = InstrumentMapping::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F60").unwrap(),
        owner.clone(),
        VersionRef::new(source.id().clone(), Version::new(1).unwrap()),
        vec![
            InstrumentMappingEntry::new(
                "260011.IB",
                effective.clone(),
                VersionRef::new(
                    Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F20").unwrap(),
                    Version::new(7).unwrap(),
                ),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let calendar = Calendar::new(CalendarInput {
        calendar_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F30").unwrap(),
        version: Version::new(3).unwrap(),
        owner: owner.clone(),
        market: "CGB".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective,
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
        unit_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F40").unwrap(),
        version: Version::new(2).unwrap(),
        owner,
        code: "CNY_CLEAN_PRICE".to_owned(),
        dimension: "price".to_owned(),
        scale: 4,
        precision: 12,
    })
    .unwrap();
    CanonicalIngestRequest::new(
        source,
        mapping,
        calendar,
        unit,
        PointInTimeWindow::new(
            market_time("2026-07-20T02:00:00Z"),
            market_time("2026-07-20T02:05:00Z"),
        )
        .unwrap(),
    )
    .unwrap()
}

fn row(
    id: &str,
    instrument: &str,
    observed_at: &str,
    visible_at: &str,
    bid: Option<(&str, u32)>,
    ask: Option<(&str, u32)>,
) -> RawQuoteRow {
    RawQuoteRow::new(
        id,
        instrument,
        observed_at,
        visible_at,
        bid.map(|(coefficient, scale)| RawDecimal::new(coefficient, scale)),
        ask.map(|(coefficient, scale)| RawDecimal::new(coefficient, scale)),
    )
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

fn temp_root() -> PathBuf {
    std::env::temp_dir().join(format!("ficant-phase3a-file-{}", std::process::id()))
}
