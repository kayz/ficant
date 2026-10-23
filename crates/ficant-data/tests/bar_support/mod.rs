//! Shared fixtures for the canonical Bar tests (synthetic data only).
#![allow(dead_code, unused_imports)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use arrow::array::{Array, RecordBatch, StringArray};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use ficant_data::{
    BarRejection, BarRejectionReason, BarVisibilityRule, CANONICAL_BAR_SCHEMA_ID,
    CanonicalBarIngestRequest, CanonicalBarIngestor, CodeField, DataError, FileNdjsonBarSource,
    InstrumentMapping, InstrumentMappingEntry, NdjsonBarMapping, PointInTimeWindow,
    SourceTimeFormat, canonical_bar_schema, canonical_bar_schema_hash, parse_decimal_text,
};
use ficant_domain::market::{
    BarFrequency, BarSettlement, Calendar, CalendarInput, CalendarSession, DataSource,
    DataSourceInput, DataSourceKind, PriceSourceType, Unit, UnitInput,
};
use ficant_domain::primitives::{EffectivePeriod, MarketTime, OwnerRef, Ulid, Version, VersionRef};

pub const INSTRUMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5F20";
pub const SOURCE_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA0";
pub const SOURCE_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";

/// Vendor A: flat CSV-like records, Chinese-market local time without offset, string decimals,
/// a publication-time field.
pub fn vendor_a_mapping() -> NdjsonBarMapping {
    NdjsonBarMapping {
        source_record_id: "id".to_owned(),
        instrument_key: "code".to_owned(),
        frequency: CodeField::Field {
            path: "freq".to_owned(),
            codes: BTreeMap::from([
                ("1min".to_owned(), BarFrequency::M1),
                ("day".to_owned(), BarFrequency::D1),
            ]),
        },
        settlement: CodeField::Field {
            path: "settle".to_owned(),
            codes: BTreeMap::from([
                ("T+0".to_owned(), BarSettlement::T0),
                ("T+1".to_owned(), BarSettlement::T1),
            ]),
        },
        bar_start: Some("start".to_owned()),
        trading_date: Some("date".to_owned()),
        trading_date_format: "%Y%m%d".to_owned(),
        time_format: SourceTimeFormat::LocalNaive {
            format: "%Y-%m-%d %H:%M:%S".to_owned(),
            timezone: chrono_tz::Asia::Shanghai,
        },
        visibility: BarVisibilityRule::SourceField,
        visible_at: Some("pub".to_owned()),
        open: "o".to_owned(),
        high: "h".to_owned(),
        low: "l".to_owned(),
        close: "c".to_owned(),
        volume: "vol".to_owned(),
        turnover: Some("amt".to_owned()),
        trade_count: Some("n".to_owned()),
        vwap: None,
        close_yield: Some("ytm".to_owned()),
    }
}

/// Vendor B: nested objects, UTC epoch milliseconds, JSON numbers, numeric settlement codes and no
/// visibility field (bars are declared visible 2 seconds after they end).
pub fn vendor_b_mapping() -> NdjsonBarMapping {
    NdjsonBarMapping {
        source_record_id: "rec.uid".to_owned(),
        instrument_key: "instrument.symbol".to_owned(),
        frequency: CodeField::Field {
            path: "bar.interval".to_owned(),
            codes: BTreeMap::from([
                ("PT1M".to_owned(), BarFrequency::M1),
                ("P1D".to_owned(), BarFrequency::D1),
            ]),
        },
        settlement: CodeField::Field {
            path: "bar.settlementType".to_owned(),
            codes: BTreeMap::from([
                ("1".to_owned(), BarSettlement::T0),
                ("2".to_owned(), BarSettlement::T1),
            ]),
        },
        bar_start: Some("bar.startMs".to_owned()),
        trading_date: Some("bar.tradeDate".to_owned()),
        trading_date_format: "%Y-%m-%d".to_owned(),
        time_format: SourceTimeFormat::EpochMillis,
        visibility: BarVisibilityRule::FixedDelay(Duration::seconds(2)),
        visible_at: None,
        open: "px.open".to_owned(),
        high: "px.high".to_owned(),
        low: "px.low".to_owned(),
        close: "px.close".to_owned(),
        volume: "qty.face".to_owned(),
        turnover: Some("qty.amount".to_owned()),
        trade_count: Some("qty.trades".to_owned()),
        vwap: None,
        close_yield: Some("px.ytm".to_owned()),
    }
}

/// The same three synthetic one-minute bars, written in each vendor's layout.
pub const VENDOR_A_MINUTES: &str = r#"
{"id":"a-0931","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:31:00","pub":"2026-07-20 09:32:02","o":"100.1400","h":"100.1600","l":"100.1300","c":"100.1500","vol":"20000000","amt":"20030000.00","n":2,"ytm":"1.8350"}
{"id":"a-0930","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:30:00","pub":"2026-07-20 09:31:02","o":"100.1200","h":"100.1500","l":"100.1000","c":"100.1400","vol":"30000000","amt":"30040000.00","n":3,"ytm":"1.8360"}
{"id":"a-0932","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:32:00","pub":"2026-07-20 09:33:02","vol":"0","n":0}
"#;

pub const VENDOR_B_MINUTES: &str = r#"
{"rec":{"uid":"b-1"},"instrument":{"symbol":"CGB260011"},"bar":{"interval":"PT1M","settlementType":2,"startMs":1784511000000},"px":{"open":100.12,"high":100.15,"low":100.1,"close":100.14,"ytm":1.836},"qty":{"face":30000000,"amount":30040000,"trades":3}}
{"rec":{"uid":"b-2"},"instrument":{"symbol":"CGB260011"},"bar":{"interval":"PT1M","settlementType":2,"startMs":1784511060000},"px":{"open":100.14,"high":100.16,"low":100.13,"close":100.15,"ytm":1.835},"qty":{"face":20000000,"amount":20030000,"trades":2}}
{"rec":{"uid":"b-3"},"instrument":{"symbol":"CGB260011"},"bar":{"interval":"PT1M","settlementType":2,"startMs":1784511120000},"qty":{"face":0,"trades":0}}
"#;

pub async fn ingest_ok(
    mapping: NdjsonBarMapping,
    source_id: &str,
    instrument_key: &str,
    rows: &str,
) -> RecordBatch {
    let source = file_source(mapping, source_id, rows);
    let result = CanonicalBarIngestor
        .ingest(&source, &request(source_id, instrument_key))
        .await
        .unwrap();
    assert!(
        result.report().rejections.is_empty(),
        "{:?}",
        result.report().rejections
    );
    result.batch().clone()
}

pub fn file_source(mapping: NdjsonBarMapping, source_id: &str, rows: &str) -> FileNdjsonBarSource {
    // Tests run in parallel; every source gets its own directory so no test reads another's file.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = temp_root(&format!("{source_id}-{sequence}"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("bars.ndjson"), rows.trim_start()).unwrap();
    FileNdjsonBarSource::new("file-binding", root, mapping).unwrap()
}

pub fn request(source_id: &str, instrument_key: &str) -> CanonicalBarIngestRequest {
    build_request(
        data_source(source_id, Some(PriceSourceType::RealTrade)),
        instrument_key,
    )
    .unwrap()
}

pub fn source_input(source_id: &str) -> DataSourceInput {
    DataSourceInput {
        data_source_id: Ulid::new(source_id).unwrap(),
        version: Version::new(1).unwrap(),
        owner: owner(),
        kind: DataSourceKind::FileNdjson,
        name: "CGB bars".to_owned(),
        connection_binding: "file-binding".to_owned(),
        dataset: "bars".to_owned(),
        canonical_schema_id: CANONICAL_BAR_SCHEMA_ID.to_owned(),
        canonical_schema_hash: canonical_bar_schema_hash(),
    }
}

pub fn data_source(source_id: &str, price_source_type: Option<PriceSourceType>) -> DataSource {
    let source = DataSource::new(source_input(source_id)).unwrap();
    match price_source_type {
        Some(value) => source.with_price_source_type(value).unwrap(),
        None => source,
    }
}

pub fn build_request(
    source: DataSource,
    instrument_key: &str,
) -> ficant_data::DataResult<CanonicalBarIngestRequest> {
    let effective = EffectivePeriod::new(
        market_time("2026-07-01T00:00:00Z"),
        market_time("2026-08-01T00:00:00Z"),
    )
    .unwrap();
    let mapping = InstrumentMapping::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F60").unwrap(),
        owner(),
        VersionRef::new(source.id().clone(), Version::new(1).unwrap()),
        vec![
            InstrumentMappingEntry::new(
                instrument_key,
                effective.clone(),
                VersionRef::new(Ulid::new(INSTRUMENT).unwrap(), Version::new(7).unwrap()),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let session = |day| {
        CalendarSession::open(
            NaiveDate::from_ymd_opt(2026, 7, day).unwrap(),
            NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
        )
        .unwrap()
    };
    let calendar = Calendar::new(CalendarInput {
        calendar_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F30").unwrap(),
        version: Version::new(3).unwrap(),
        owner: owner(),
        market: "CGB".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective,
        sessions: vec![session(20), session(21)],
    })
    .unwrap();
    let unit = |id: &str, code: &str, dimension: &str, scale, precision| {
        Unit::new(UnitInput {
            unit_id: Ulid::new(id).unwrap(),
            version: Version::new(1).unwrap(),
            owner: owner(),
            code: code.to_owned(),
            dimension: dimension.to_owned(),
            scale,
            precision,
        })
        .unwrap()
    };
    CanonicalBarIngestRequest::new(
        source,
        mapping,
        calendar,
        unit(
            "01ARZ3NDEKTSV4RRFFQ69G5F40",
            "CNY_CLEAN_PRICE",
            "price",
            4,
            12,
        ),
        unit("01ARZ3NDEKTSV4RRFFQ69G5F41", "CNY_FACE", "amount", 2, 18),
        Some(unit("01ARZ3NDEKTSV4RRFFQ69G5F42", "PCT_YTM", "yield", 4, 8)),
        PointInTimeWindow::new(
            market_time("2026-07-20T09:00:00Z"),
            market_time("2026-07-21T09:00:00Z"),
        )
        .unwrap(),
    )
}

pub fn text(batch: &RecordBatch, column: &str) -> Vec<String> {
    let values = batch
        .column_by_name(column)
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    (0..values.len())
        .map(|index| values.value(index).to_owned())
        .collect()
}

pub fn timestamp(batch: &RecordBatch, column: &str, row: usize) -> DateTime<Utc> {
    let values = batch
        .column_by_name(column)
        .unwrap()
        .as_any()
        .downcast_ref::<arrow::array::TimestampMicrosecondArray>()
        .unwrap();
    DateTime::from_timestamp_micros(values.value(row)).unwrap()
}

pub fn utc(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

pub fn market_time(value: &str) -> MarketTime {
    let instant = utc(value);
    MarketTime::new(
        instant,
        "Asia/Shanghai",
        instant
            .with_timezone(&chrono_tz::Asia::Shanghai)
            .date_naive(),
    )
    .unwrap()
}

pub fn owner() -> OwnerRef {
    OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    )
}

pub fn temp_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ficant-bars-{}-{tag}", std::process::id()))
}
