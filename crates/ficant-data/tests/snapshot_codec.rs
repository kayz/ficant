use arrow::array::StringArray;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use ficant_data::{
    CANONICAL_QUOTE_SCHEMA_ID, CanonicalIngestRequest, CanonicalQuoteIngestor,
    CanonicalSnapshotCodec, DataError, DataResult, InstrumentMapping, InstrumentMappingEntry,
    PARQUET_CREATED_BY, PointInTimeWindow, RawDecimal, RawQuoteRow, RawQuoteSource,
    SNAPSHOT_MANIFEST_SCHEMA_ID, VerifiedCanonicalSnapshot, canonical_quote_schema_hash,
};
use ficant_domain::market::{
    Calendar, CalendarInput, CalendarSession, DataSource, DataSourceInput, DataSourceKind, Unit,
    UnitInput,
};
use ficant_domain::primitives::{EffectivePeriod, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_domain::research::{DataSnapshot, DataSnapshotInput};
use ficant_domain::{ContentAddressed, Lineaged};
use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::basic::Compression;

#[tokio::test]
async fn deterministic_parquet_manifest_and_verified_round_trip_are_exact() {
    let request = request();
    let source = MemorySource(rows());
    let canonical = CanonicalQuoteIngestor
        .ingest(&source, &request)
        .await
        .unwrap();
    let snapshot_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F50").unwrap();
    let first = CanonicalSnapshotCodec
        .build(snapshot_id.clone(), &request, &canonical)
        .unwrap();
    let second = CanonicalSnapshotCodec
        .build(snapshot_id, &request, &canonical)
        .unwrap();

    assert_eq!(first.parquet(), second.parquet());
    assert_eq!(first.manifest(), second.manifest());
    assert_eq!(first.snapshot(), second.snapshot());
    assert!(first.manifest().ends_with(b"\n"));
    assert_eq!(
        first.snapshot().content_hash(),
        &ficant_domain::primitives::ContentHash::digest(first.parquet())
    );
    assert_eq!(
        first.snapshot().manifest_hash(),
        &ficant_domain::primitives::ContentHash::digest(first.manifest())
    );
    assert_eq!(
        first.snapshot().schema_hash(),
        &canonical_quote_schema_hash()
    );
    assert_eq!(first.snapshot().as_of(), request.window().as_of());
    assert_eq!(
        first.snapshot().visible_at(),
        request.window().visible_at_cutoff()
    );
    assert_eq!(first.snapshot().lineage().len(), 4);

    let manifest_text = std::str::from_utf8(first.manifest()).unwrap();
    assert!(manifest_text.contains(SNAPSHOT_MANIFEST_SCHEMA_ID));
    assert!(!manifest_text.contains("file-binding"));
    assert!(!manifest_text.contains("postgres://"));
    assert!(!manifest_text.contains("\\\\"));

    let builder = ParquetRecordBatchReaderBuilder::try_new_with_options(
        Bytes::copy_from_slice(first.parquet()),
        ArrowReaderOptions::new().with_skip_arrow_metadata(false),
    )
    .unwrap();
    assert_eq!(builder.metadata().num_row_groups(), 1);
    assert_eq!(builder.metadata().file_metadata().version(), 2);
    assert_eq!(
        builder.metadata().file_metadata().created_by(),
        Some(PARQUET_CREATED_BY)
    );
    assert!(
        builder
            .metadata()
            .row_group(0)
            .columns()
            .iter()
            .all(|column| column.compression() == Compression::UNCOMPRESSED)
    );
    let decoded = CanonicalSnapshotCodec
        .decode_verified(first.snapshot().clone(), first.parquet(), first.manifest())
        .unwrap();
    assert_eq!(decoded.batch(), canonical.batch());
    assert_exact_quote_projection(&decoded);
    assert_eq!(decoded.manifest().row_count(), 2);
    assert_eq!(
        decoded.manifest().data_source_id(),
        request.source().id().as_str()
    );
    assert_eq!(
        decoded.manifest().instrument_mapping_digest(),
        "8ec7f1016e6b00a9649e4c86202fbb90381948fec31bd12a7750b511c0d5e61e"
    );
    let ids = decoded
        .batch()
        .column(4)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!([ids.value(0), ids.value(1)], ["record-1", "record-2"]);

    let schema_hash = canonical_quote_schema_hash().as_bytes().iter().fold(
        String::with_capacity(64),
        |mut encoded, byte| {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").unwrap();
            encoded
        },
    );
    assert_eq!(
        schema_hash,
        "e804a0becec18e51dde1be4250384ffe667cf4149c34dc3d2cfc82a206d71502"
    );
}

fn assert_exact_quote_projection(decoded: &VerifiedCanonicalSnapshot) {
    let quotes = decoded.quotes().unwrap();
    assert_eq!(quotes.len(), 2);
    assert_eq!(
        quotes[0].instrument(),
        &VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F20").unwrap(),
            Version::new(7).unwrap(),
        )
    );
    assert_eq!(
        quotes[0].observed_at(),
        "2026-07-20T01:30:00Z".parse::<DateTime<Utc>>().unwrap()
    );
    assert_eq!(
        quotes[0].visible_at(),
        "2026-07-20T01:30:05Z".parse::<DateTime<Utc>>().unwrap()
    );
    assert_eq!(
        quotes[0].local_trading_date(),
        NaiveDate::from_ymd_opt(2026, 7, 20).unwrap()
    );
    assert_eq!(quotes[0].bid().unwrap().coefficient(), "10123");
    assert_eq!(quotes[0].bid().unwrap().scale(), 2);
    assert_eq!(quotes[0].ask().unwrap().coefficient(), "10125");
    assert_eq!(quotes[0].ask().unwrap().scale(), 2);
    assert_eq!(
        quotes[0].unit().unit_id(),
        &Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F40").unwrap()
    );
    assert_eq!(quotes[0].unit().version(), Version::new(2).unwrap());
    assert_eq!(quotes[0].bid().unwrap().unit(), quotes[0].unit());
    assert_eq!(quotes[0].ask().unwrap().unit(), quotes[0].unit());
}

#[tokio::test]
async fn verified_quote_projection_preserves_optional_sides_and_exact_unit() {
    let request = request();
    let source = MemorySource(vec![
        RawQuoteRow::new(
            "record-bid",
            "260011.IB",
            "2026-07-20T01:30:00Z",
            "2026-07-20T01:30:05Z",
            Some(RawDecimal::new("1012300", 4)),
            None,
        ),
        RawQuoteRow::new(
            "record-ask",
            "260011.IB",
            "2026-07-20T01:31:00Z",
            "2026-07-20T01:31:05Z",
            None,
            Some(RawDecimal::new("1012600", 4)),
        ),
    ]);
    let canonical = CanonicalQuoteIngestor
        .ingest(&source, &request)
        .await
        .unwrap();
    let package = CanonicalSnapshotCodec
        .build(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F51").unwrap(),
            &request,
            &canonical,
        )
        .unwrap();
    let verified = CanonicalSnapshotCodec
        .decode_verified(
            package.snapshot().clone(),
            package.parquet(),
            package.manifest(),
        )
        .unwrap();

    let quotes = verified.quotes().unwrap();
    assert_eq!(quotes.len(), 2);
    assert!(quotes[0].bid().is_some());
    assert!(quotes[0].ask().is_none());
    assert!(quotes[1].bid().is_none());
    assert!(quotes[1].ask().is_some());
    assert_eq!(quotes[0].unit(), quotes[1].unit());
    assert_eq!(
        verified.batch().schema().as_ref(),
        &ficant_data::canonical_quote_schema()
    );
    assert_eq!(
        verified.snapshot().schema_hash(),
        &canonical_quote_schema_hash()
    );
}

#[tokio::test]
async fn parquet_manifest_and_snapshot_lineage_tampering_fail_closed() {
    let request = request();
    let canonical = CanonicalQuoteIngestor
        .ingest(&MemorySource(rows()), &request)
        .await
        .unwrap();
    let package = CanonicalSnapshotCodec
        .build(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F50").unwrap(),
            &request,
            &canonical,
        )
        .unwrap();

    let mut parquet = package.parquet().to_vec();
    parquet[8] ^= 1;
    assert_eq!(
        CanonicalSnapshotCodec
            .decode_verified(package.snapshot().clone(), &parquet, package.manifest())
            .unwrap_err(),
        DataError::SnapshotIntegrityFailed
    );

    let mut manifest = package.manifest().to_vec();
    let index = manifest.iter().position(|value| *value == b'5').unwrap();
    manifest[index] = b'6';
    assert_eq!(
        CanonicalSnapshotCodec
            .decode_verified(package.snapshot().clone(), package.parquet(), &manifest)
            .unwrap_err(),
        DataError::SnapshotIntegrityFailed
    );

    let noncanonical = {
        let mut bytes = package.manifest().to_vec();
        bytes.insert(1, b' ');
        bytes
    };
    let rebound = DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: package.snapshot().id().clone(),
        owner: package.snapshot().owner().clone(),
        visible_at: package.snapshot().visible_at().clone(),
        as_of: package.snapshot().as_of().clone(),
        schema_hash: package.snapshot().schema_hash().clone(),
        manifest_hash: ficant_domain::primitives::ContentHash::digest(&noncanonical),
        blob_content_hash: package.snapshot().content_hash().clone(),
        lineage: package.snapshot().lineage().to_vec(),
    })
    .unwrap();
    assert_eq!(
        CanonicalSnapshotCodec
            .decode_verified(rebound, package.parquet(), &noncanonical)
            .unwrap_err(),
        DataError::SnapshotIntegrityFailed
    );

    let mut lineage = package.snapshot().lineage().to_vec();
    lineage.pop();
    let rebound = DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: package.snapshot().id().clone(),
        owner: package.snapshot().owner().clone(),
        visible_at: package.snapshot().visible_at().clone(),
        as_of: package.snapshot().as_of().clone(),
        schema_hash: package.snapshot().schema_hash().clone(),
        manifest_hash: package.snapshot().manifest_hash().clone(),
        blob_content_hash: package.snapshot().content_hash().clone(),
        lineage,
    })
    .unwrap();
    assert_eq!(
        CanonicalSnapshotCodec
            .decode_verified(rebound, package.parquet(), package.manifest())
            .unwrap_err(),
        DataError::SnapshotIntegrityFailed
    );
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

fn rows() -> Vec<RawQuoteRow> {
    vec![
        row(
            "record-2",
            "2026-07-20T01:31:00Z",
            "2026-07-20T01:31:05Z",
            "1012400",
            "1012600",
        ),
        row(
            "record-1",
            "2026-07-20T01:30:00Z",
            "2026-07-20T01:30:05Z",
            "1012300",
            "1012500",
        ),
    ]
}

fn row(id: &str, observed_at: &str, visible_at: &str, bid: &str, ask: &str) -> RawQuoteRow {
    RawQuoteRow::new(
        id,
        "260011.IB",
        observed_at,
        visible_at,
        Some(RawDecimal::new(bid, 4)),
        Some(RawDecimal::new(ask, 4)),
    )
}

fn request() -> CanonicalIngestRequest {
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let source = DataSource::new(DataSourceInput {
        data_source_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F10").unwrap(),
        version: Version::new(1).unwrap(),
        owner: owner.clone(),
        kind: DataSourceKind::FileNdjson,
        name: "CGB quotes".to_owned(),
        connection_binding: "file-binding".to_owned(),
        dataset: "quotes".to_owned(),
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
