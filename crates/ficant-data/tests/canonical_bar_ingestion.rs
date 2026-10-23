//! AC45: two bar sources with different field layouts produce the same canonical Bar; daily and
//! minute bars differ only by frequency; bad rows are rejected with their identity and reason.

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

const INSTRUMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5F20";
const SOURCE_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA0";
const SOURCE_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";

/// Vendor A: flat CSV-like records, Chinese-market local time without offset, string decimals,
/// a publication-time field.
fn vendor_a_mapping() -> NdjsonBarMapping {
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
fn vendor_b_mapping() -> NdjsonBarMapping {
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
const VENDOR_A_MINUTES: &str = r#"
{"id":"a-0931","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:31:00","pub":"2026-07-20 09:32:02","o":"100.1400","h":"100.1600","l":"100.1300","c":"100.1500","vol":"20000000","amt":"20030000.00","n":2,"ytm":"1.8350"}
{"id":"a-0930","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:30:00","pub":"2026-07-20 09:31:02","o":"100.1200","h":"100.1500","l":"100.1000","c":"100.1400","vol":"30000000","amt":"30040000.00","n":3,"ytm":"1.8360"}
{"id":"a-0932","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:32:00","pub":"2026-07-20 09:33:02","vol":"0","n":0}
"#;

const VENDOR_B_MINUTES: &str = r#"
{"rec":{"uid":"b-1"},"instrument":{"symbol":"CGB260011"},"bar":{"interval":"PT1M","settlementType":2,"startMs":1784511000000},"px":{"open":100.12,"high":100.15,"low":100.1,"close":100.14,"ytm":1.836},"qty":{"face":30000000,"amount":30040000,"trades":3}}
{"rec":{"uid":"b-2"},"instrument":{"symbol":"CGB260011"},"bar":{"interval":"PT1M","settlementType":2,"startMs":1784511060000},"px":{"open":100.14,"high":100.16,"low":100.13,"close":100.15,"ytm":1.835},"qty":{"face":20000000,"amount":20030000,"trades":2}}
{"rec":{"uid":"b-3"},"instrument":{"symbol":"CGB260011"},"bar":{"interval":"PT1M","settlementType":2,"startMs":1784511120000},"qty":{"face":0,"trades":0}}
"#;

#[tokio::test]
async fn two_layouts_produce_the_same_canonical_bars() {
    let a = ingest_ok(vendor_a_mapping(), SOURCE_A, "260011.IB", VENDOR_A_MINUTES).await;
    let b = ingest_ok(vendor_b_mapping(), SOURCE_B, "CGB260011", VENDOR_B_MINUTES).await;

    for batch in [&a, &b] {
        assert_eq!(batch.schema().as_ref(), &canonical_bar_schema());
        assert_eq!(batch.num_rows(), 3);
    }
    // Only the columns that identify the source differ; every bar column is identical.
    let source_columns = ["data_source_id", "source_record_id", "visible_at"];
    for field in canonical_bar_schema().fields() {
        if source_columns.contains(&field.name().as_str()) {
            continue;
        }
        let name = field.name();
        assert_eq!(
            a.column_by_name(name).unwrap().to_data(),
            b.column_by_name(name).unwrap().to_data(),
            "column {name} differs between the two sources"
        );
    }
    // Bars are sorted by start time regardless of source order, and the empty bar keeps no prices.
    let ids = text(&a, "source_record_id");
    assert_eq!(ids, ["a-0930", "a-0931", "a-0932"]);
    assert_eq!(text(&a, "frequency"), ["M1", "M1", "M1"]);
    assert_eq!(text(&a, "settlement"), ["T1", "T1", "T1"]);
    assert_eq!(text(&a, "price_source_type")[0], "REAL_TRADE");
    let close = a
        .column_by_name("close_coefficient")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(close.value(0), "10014");
    assert!(close.is_null(2));
}

#[tokio::test]
async fn daily_and_minute_bars_share_one_structure() {
    let daily = r#"
{"rec":{"uid":"b-d1"},"instrument":{"symbol":"CGB260011"},"bar":{"interval":"P1D","settlementType":2,"tradeDate":"2026-07-20"},"px":{"open":100.12,"high":100.2,"low":100.05,"close":100.18,"ytm":1.833},"qty":{"face":900000000,"amount":901400000,"trades":61}}
"#;
    let minutes = ingest_ok(vendor_b_mapping(), SOURCE_B, "CGB260011", VENDOR_B_MINUTES).await;
    let days = ingest_ok(vendor_b_mapping(), SOURCE_B, "CGB260011", daily).await;
    assert_eq!(minutes.schema(), days.schema());
    assert_eq!(text(&days, "frequency"), ["D1"]);
    // A daily bar spans the calendar session (09:00–17:00 Asia/Shanghai).
    let start = timestamp(&days, "bar_start", 0);
    let end = timestamp(&days, "bar_end", 0);
    assert_eq!(start, utc("2026-07-20T01:00:00Z"));
    assert_eq!(end, utc("2026-07-20T09:00:00Z"));
    assert_eq!(
        timestamp(&days, "visible_at", 0),
        end + Duration::seconds(2)
    );
}

#[tokio::test]
async fn bad_rows_are_rejected_with_identity_and_reason_and_the_rest_imported() {
    let rows = r#"
{"id":"good","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:30:00","pub":"2026-07-20 09:31:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
{"id":"no-volume","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:31:00","pub":"2026-07-20 09:32:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1"}
{"id":"half-prices","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:32:00","pub":"2026-07-20 09:33:00","o":"100.1","h":"100.2","vol":"10000000"}
{"id":"before-open","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 08:30:00","pub":"2026-07-20 08:31:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
{"id":"after-close","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 16:59:30","pub":"2026-07-20 17:01:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
{"id":"known-early","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:40:00","pub":"2026-07-20 09:40:30","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
{"id":"unmapped","code":"999999.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:41:00","pub":"2026-07-20 09:42:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
{"id":"off-grid","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:42:30","pub":"2026-07-20 09:44:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
{"id":"high-below-close","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:43:00","pub":"2026-07-20 09:44:00","o":"100.1","h":"100.1","l":"100.0","c":"100.2","vol":"10000000"}
{"id":"negative","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:44:00","pub":"2026-07-20 09:45:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"-1"}
{"id":"bad-number","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:45:00","pub":"2026-07-20 09:46:00","o":"1e2","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
{"id":"too-precise","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:46:00","pub":"2026-07-20 09:47:00","o":"100.123456","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
{"id":"unknown-freq","code":"260011.IB","freq":"7min","settle":"T+1","start":"2026-07-20 09:47:00","pub":"2026-07-20 09:48:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
{"id":"dup-1","code":"260011.IB","freq":"1min","settle":"T+0","start":"2026-07-20 09:50:00","pub":"2026-07-20 09:51:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
{"id":"dup-2","code":"260011.IB","freq":"1min","settle":"T+0","start":"2026-07-20 09:50:00","pub":"2026-07-20 09:51:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"20000000"}
{"code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:55:00","pub":"2026-07-20 09:56:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
not json
{"id":"tomorrow","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-21 09:30:00","pub":"2026-07-21 09:31:00","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"10000000"}
"#;
    let source = file_source(vendor_a_mapping(), SOURCE_A, rows);
    let request = request(SOURCE_A, "260011.IB");
    let result = CanonicalBarIngestor
        .ingest(&source, &request)
        .await
        .unwrap();
    let report = result.report();
    assert_eq!(report.read_rows, 18);
    assert_eq!(report.emitted_rows, 1);
    assert_eq!(
        report.outside_window_rows, 1,
        "the next day's bar is not yet known"
    );
    assert_eq!(report.visibility_rule, "source-field");
    assert_eq!(text(result.batch(), "source_record_id"), ["good"]);

    let expected = [
        (
            "after-close",
            BarRejectionReason::OutsideTradingSession,
            None,
        ),
        ("bad-number", BarRejectionReason::InvalidValue, Some("o")),
        (
            "before-open",
            BarRejectionReason::OutsideTradingSession,
            None,
        ),
        ("dup-1", BarRejectionReason::DuplicateBar, None),
        ("dup-2", BarRejectionReason::DuplicateBar, None),
        ("half-prices", BarRejectionReason::InconsistentOhlc, None),
        (
            "high-below-close",
            BarRejectionReason::InconsistentOhlc,
            None,
        ),
        (
            "known-early",
            BarRejectionReason::ObservedAfterVisible,
            None,
        ),
        ("negative", BarRejectionReason::NegativeVolume, None),
        (
            "no-volume",
            BarRejectionReason::MissingRequiredField,
            Some("volume"),
        ),
        ("off-grid", BarRejectionReason::MisalignedInterval, None),
        (
            "row:16",
            BarRejectionReason::MissingRequiredField,
            Some("source_record_id"),
        ),
        ("row:17", BarRejectionReason::InvalidValue, Some("<json>")),
        (
            "too-precise",
            BarRejectionReason::PrecisionExceeded,
            Some("open"),
        ),
        (
            "unknown-freq",
            BarRejectionReason::InvalidValue,
            Some("freq"),
        ),
        (
            "unmapped",
            BarRejectionReason::UnmappedInstrument,
            Some("instrument_key"),
        ),
    ]
    .map(|(row_id, reason, field)| BarRejection {
        row_id: row_id.to_owned(),
        reason,
        field: field.map(str::to_owned),
    });
    assert_eq!(report.rejections, expected);
}

#[tokio::test]
async fn an_import_with_no_accepted_row_fails_and_still_reports_why() {
    let rows = r#"
{"id":"x","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:30:00","pub":"2026-07-20 09:30:10","o":"100.1","h":"100.2","l":"100.0","c":"100.1","vol":"1"}
"#;
    let source = file_source(vendor_a_mapping(), SOURCE_A, rows);
    let error = CanonicalBarIngestor
        .ingest(&source, &request(SOURCE_A, "260011.IB"))
        .await
        .unwrap_err();
    let DataError::NoAcceptedBars { rejections } = error else {
        panic!("expected NoAcceptedBars, got {error:?}");
    };
    assert_eq!(rejections.len(), 1);
    assert_eq!(
        rejections[0].reason,
        BarRejectionReason::ObservedAfterVisible
    );
}

#[test]
fn request_requires_bar_schema_and_declared_price_source() {
    let without_type = data_source(SOURCE_A, None);
    assert!(matches!(
        build_request(without_type, "260011.IB"),
        Err(DataError::InvalidConfiguration)
    ));
    let mut quote_schema = DataSourceInput {
        canonical_schema_id: ficant_data::CANONICAL_QUOTE_SCHEMA_ID.to_owned(),
        canonical_schema_hash: ficant_data::canonical_quote_schema_hash(),
        ..source_input(SOURCE_A)
    };
    quote_schema.name = "quote schema".to_owned();
    let source = DataSource::new(quote_schema)
        .unwrap()
        .with_price_source_type(PriceSourceType::RealTrade)
        .unwrap();
    assert!(matches!(
        build_request(source, "260011.IB"),
        Err(DataError::InvalidConfiguration)
    ));
}

#[test]
fn adapter_configuration_must_state_visibility_consistently() {
    let mut mapping = vendor_b_mapping();
    mapping.visible_at = Some("bar.knownMs".to_owned());
    assert!(FileNdjsonBarSource::new("file", temp_root("cfg"), mapping).is_err());
    let mut mapping = vendor_a_mapping();
    mapping.visible_at = None;
    assert!(FileNdjsonBarSource::new("file", temp_root("cfg"), mapping).is_err());
    let mut mapping = vendor_b_mapping();
    mapping.visibility = BarVisibilityRule::FixedDelay(Duration::seconds(-1));
    assert!(FileNdjsonBarSource::new("file", temp_root("cfg"), mapping).is_err());
}

#[test]
fn decimal_text_is_parsed_exactly() {
    let value = parse_decimal_text("-100.1200").unwrap();
    assert_eq!((value.coefficient(), value.scale()), ("-1001200", 4));
    let value = parse_decimal_text("30000000").unwrap();
    assert_eq!((value.coefficient(), value.scale()), ("30000000", 0));
    for bad in ["", "1e5", ".5", "5.", "1.2.3", "+1", " 1"] {
        assert!(
            parse_decimal_text(bad).is_none(),
            "{bad:?} must be rejected"
        );
    }
}

#[test]
fn bar_schema_hash_is_stable_and_distinct_from_quotes() {
    assert_eq!(canonical_bar_schema_hash(), canonical_bar_schema_hash());
    assert_ne!(
        canonical_bar_schema_hash(),
        ficant_data::canonical_quote_schema_hash()
    );
    assert_eq!(
        canonical_bar_schema().metadata()["ficant.schema.id"],
        CANONICAL_BAR_SCHEMA_ID
    );
}

async fn ingest_ok(
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

fn file_source(mapping: NdjsonBarMapping, source_id: &str, rows: &str) -> FileNdjsonBarSource {
    let root = temp_root(&format!("{source_id}-{}", rows.len()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("bars.ndjson"), rows.trim_start()).unwrap();
    FileNdjsonBarSource::new("file-binding", root, mapping).unwrap()
}

fn request(source_id: &str, instrument_key: &str) -> CanonicalBarIngestRequest {
    build_request(
        data_source(source_id, Some(PriceSourceType::RealTrade)),
        instrument_key,
    )
    .unwrap()
}

fn source_input(source_id: &str) -> DataSourceInput {
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

fn data_source(source_id: &str, price_source_type: Option<PriceSourceType>) -> DataSource {
    let source = DataSource::new(source_input(source_id)).unwrap();
    match price_source_type {
        Some(value) => source.with_price_source_type(value).unwrap(),
        None => source,
    }
}

fn build_request(
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

fn text(batch: &RecordBatch, column: &str) -> Vec<String> {
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

fn timestamp(batch: &RecordBatch, column: &str, row: usize) -> DateTime<Utc> {
    let values = batch
        .column_by_name(column)
        .unwrap()
        .as_any()
        .downcast_ref::<arrow::array::TimestampMicrosecondArray>()
        .unwrap();
    DateTime::from_timestamp_micros(values.value(row)).unwrap()
}

fn utc(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

fn market_time(value: &str) -> MarketTime {
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

fn owner() -> OwnerRef {
    OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    )
}

fn temp_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ficant-bars-{}-{tag}", std::process::id()))
}
