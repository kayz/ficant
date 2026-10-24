//! Canonical Bar ingestion (ADR-0023, AC45).
//!
//! Every adapter's rows pass through the same validation here and come out as one Arrow schema,
//! `ficant.market.bar.canonical.v1`. Bad rows are rejected one by one with their source identity
//! and a closed reason; the remaining rows are imported. The whole import fails only when no row
//! is accepted or the request itself is misconfigured.

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, Date32Array, RecordBatch, StringArray, TimestampMicrosecondArray, UInt32Array,
    UInt64Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use ficant_domain::VersionedDefinition;
use ficant_domain::market::{
    Bar, BarFrequency, BarInput, BarPrices, BarRuleViolation, Calendar, DataSource,
    PriceSourceType, Unit,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, MarketTime, UnitRef, Version, VersionRef,
};

use crate::bar_source::{BarSourceAdapter, BarVisibilityRule, RawBarRow};
use crate::canonical::{schema_contract_bytes, text_field, timestamp_field};
use crate::{DataError, DataResult, InstrumentMapping, PointInTimeWindow, RawDecimal};

pub const CANONICAL_BAR_SCHEMA_ID: &str = "ficant.market.bar.canonical.v1";
pub const BAR_QUALITY_RULE_SET_ID: &str = "ficant.market.bar.quality.v1";
const MARKET_TIMEZONE: &str = "Asia/Shanghai";

/// Closed set of reasons a source row is rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BarRejectionReason {
    MissingRequiredField,
    InvalidValue,
    OutsideTradingSession,
    ObservedAfterVisible,
    UnmappedInstrument,
    MisalignedInterval,
    InconsistentOhlc,
    NegativeVolume,
    DuplicateBar,
    PrecisionExceeded,
}

impl BarRejectionReason {
    pub const ALL: [Self; 10] = [
        Self::MissingRequiredField,
        Self::InvalidValue,
        Self::OutsideTradingSession,
        Self::ObservedAfterVisible,
        Self::UnmappedInstrument,
        Self::MisalignedInterval,
        Self::InconsistentOhlc,
        Self::NegativeVolume,
        Self::DuplicateBar,
        Self::PrecisionExceeded,
    ];

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.code() == code)
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::MissingRequiredField => "MISSING_REQUIRED_FIELD",
            Self::InvalidValue => "INVALID_VALUE",
            Self::OutsideTradingSession => "OUTSIDE_TRADING_SESSION",
            Self::ObservedAfterVisible => "OBSERVED_AFTER_VISIBLE",
            Self::UnmappedInstrument => "UNMAPPED_INSTRUMENT",
            Self::MisalignedInterval => "MISALIGNED_INTERVAL",
            Self::InconsistentOhlc => "INCONSISTENT_OHLC",
            Self::NegativeVolume => "NEGATIVE_VOLUME",
            Self::DuplicateBar => "DUPLICATE_BAR",
            Self::PrecisionExceeded => "PRECISION_EXCEEDED",
        }
    }
}

/// One rejected row: its source identity (record ID, or `row:<n>` when it has none), the reason,
/// and the field concerned when there is one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BarRejection {
    pub row_id: String,
    pub reason: BarRejectionReason,
    pub field: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarIngestReport {
    pub rule_set_id: &'static str,
    pub visibility_rule: String,
    pub read_rows: usize,
    pub outside_window_rows: usize,
    pub emitted_rows: usize,
    pub rejections: Vec<BarRejection>,
}

#[derive(Clone, Debug)]
pub struct CanonicalBarIngestRequest {
    pub(crate) source: DataSource,
    pub(crate) mapping: InstrumentMapping,
    pub(crate) calendar: Calendar,
    pub(crate) price_unit: Unit,
    pub(crate) amount_unit: Unit,
    pub(crate) yield_unit: Option<Unit>,
    pub(crate) window: PointInTimeWindow,
}

impl CanonicalBarIngestRequest {
    pub fn new(
        source: DataSource,
        mapping: InstrumentMapping,
        calendar: Calendar,
        price_unit: Unit,
        amount_unit: Unit,
        yield_unit: Option<Unit>,
        window: PointInTimeWindow,
    ) -> DataResult<Self> {
        let source_ref = VersionRef::new(
            source.id().clone(),
            Version::new(source.version()).map_err(|_| DataError::InvalidConfiguration)?,
        );
        let owner = source.owner();
        if owner != mapping.owner()
            || owner != calendar.owner()
            || owner != price_unit.owner()
            || owner != amount_unit.owner()
            || yield_unit.as_ref().is_some_and(|unit| unit.owner() != owner)
            || mapping.source() != &source_ref
            || source.canonical_schema_id() != CANONICAL_BAR_SCHEMA_ID
            || source.canonical_schema_hash() != &canonical_bar_schema_hash()
            // I9: every bar source declares the credibility of its prices.
            || source.price_source_type().is_none()
            || price_unit.dimension() != "price"
            || amount_unit.dimension() != "amount"
            || yield_unit.as_ref().is_some_and(|unit| unit.dimension() != "yield")
            || calendar.market_timezone() != MARKET_TIMEZONE
            || window.as_of().market_timezone() != MARKET_TIMEZONE
            || window.visible_at_cutoff().market_timezone() != MARKET_TIMEZONE
        {
            return Err(DataError::InvalidConfiguration);
        }
        Ok(Self {
            source,
            mapping,
            calendar,
            price_unit,
            amount_unit,
            yield_unit,
            window,
        })
    }

    pub fn source(&self) -> &DataSource {
        &self.source
    }

    pub fn window(&self) -> &PointInTimeWindow {
        &self.window
    }
}

#[derive(Clone, Debug)]
pub struct CanonicalBarBatch {
    pub(crate) batch: RecordBatch,
    pub(crate) schema_hash: ContentHash,
    pub(crate) report: BarIngestReport,
}

impl CanonicalBarBatch {
    pub fn batch(&self) -> &RecordBatch {
        &self.batch
    }

    pub fn schema_hash(&self) -> &ContentHash {
        &self.schema_hash
    }

    pub fn report(&self) -> &BarIngestReport {
        &self.report
    }
}

pub struct CanonicalBarIngestor;

impl CanonicalBarIngestor {
    /// Reads one source through its adapter and produces canonical bars plus a rejection report.
    ///
    /// Fails as a whole when the request is misconfigured, the source is unavailable, or no row
    /// inside the point-in-time window is accepted (`DataError::NoAcceptedBars`).
    pub async fn ingest(
        &self,
        adapter: &dyn BarSourceAdapter,
        request: &CanonicalBarIngestRequest,
    ) -> DataResult<CanonicalBarBatch> {
        let raw_rows = adapter.read(&request.source).await?;
        let units = Units::new(request)?;
        let visibility = adapter.visibility_rule();
        let mut rejections = Vec::new();
        let mut outside_window_rows = 0;
        let mut accepted = Vec::new();
        for raw in &raw_rows {
            match validate_row(request, &units, visibility, raw) {
                Ok(Some(bar)) => accepted.push(bar),
                Ok(None) => outside_window_rows += 1,
                Err(rejection) => rejections.push(rejection),
            }
        }
        let accepted = reject_duplicates(accepted, &mut rejections);
        rejections.sort();
        let report = BarIngestReport {
            rule_set_id: BAR_QUALITY_RULE_SET_ID,
            visibility_rule: visibility.describe(),
            read_rows: raw_rows.len(),
            outside_window_rows,
            emitted_rows: accepted.len(),
            rejections,
        };
        if accepted.is_empty() {
            return Err(DataError::NoAcceptedBars {
                rejections: report.rejections,
            });
        }
        let batch = encode_batch(request, &units, &accepted)?;
        if batch.schema().as_ref() != &canonical_bar_schema() {
            return Err(DataError::SchemaMismatch);
        }
        Ok(CanonicalBarBatch {
            batch,
            schema_hash: canonical_bar_schema_hash(),
            report,
        })
    }
}

struct Units {
    price: UnitRef,
    price_scale: u32,
    price_precision: u32,
    amount: UnitRef,
    amount_scale: u32,
    amount_precision: u32,
    yield_unit: Option<(UnitRef, u32, u32)>,
}

impl Units {
    fn new(request: &CanonicalBarIngestRequest) -> DataResult<Self> {
        let reference = |unit: &Unit| -> DataResult<UnitRef> {
            Ok(UnitRef::new(
                ficant_domain::primitives::Ulid::new(unit.identity())
                    .map_err(|_| DataError::InvalidConfiguration)?,
                Version::new(unit.version()).map_err(|_| DataError::InvalidConfiguration)?,
            ))
        };
        Ok(Self {
            price: reference(&request.price_unit)?,
            price_scale: request.price_unit.scale(),
            price_precision: request.price_unit.precision(),
            amount: reference(&request.amount_unit)?,
            amount_scale: request.amount_unit.scale(),
            amount_precision: request.amount_unit.precision(),
            yield_unit: request
                .yield_unit
                .as_ref()
                .map(|unit| Ok::<_, DataError>((reference(unit)?, unit.scale(), unit.precision())))
                .transpose()?,
        })
    }
}

type RowResult<T> = Result<T, BarRejection>;

/// Returns `Ok(None)` for a valid row outside the request's point-in-time window.
#[allow(clippy::too_many_lines)]
fn validate_row(
    request: &CanonicalBarIngestRequest,
    units: &Units,
    visibility: BarVisibilityRule,
    raw: &RawBarRow,
) -> RowResult<Option<Bar>> {
    let row_id = row_identity(raw);
    let reject = |reason, field: Option<&str>| BarRejection {
        row_id: row_id.clone(),
        reason,
        field: field.map(str::to_owned),
    };
    if let Some(field) = &raw.invalid_field {
        return Err(reject(BarRejectionReason::InvalidValue, Some(field)));
    }
    if raw.source_record_id.is_none() {
        return Err(reject(
            BarRejectionReason::MissingRequiredField,
            Some("source_record_id"),
        ));
    }
    let instrument_key = required(raw.instrument_key.as_ref(), "instrument_key", &reject)?;
    let frequency = *required(raw.frequency.as_ref(), "frequency", &reject)?;
    let settlement = *required(raw.settlement.as_ref(), "settlement", &reject)?;
    let volume = required(raw.volume.as_ref(), "volume", &reject)?;

    let timezone = MARKET_TIMEZONE
        .parse::<Tz>()
        .map_err(|_| reject(BarRejectionReason::InvalidValue, None))?;
    let (bar_start, bar_end, trading_date) = interval(request, frequency, raw, timezone, &reject)?;

    let visible_at = match visibility {
        BarVisibilityRule::SourceField => {
            *required(raw.visible_at.as_ref(), "visible_at", &reject)?
        }
        BarVisibilityRule::FixedDelay(delay) => bar_end + delay,
    };
    if bar_end > visible_at {
        return Err(reject(BarRejectionReason::ObservedAfterVisible, None));
    }
    if bar_end > request.window.as_of().instant()
        || visible_at > request.window.visible_at_cutoff().instant()
    {
        return Ok(None);
    }

    let observed = MarketTime::new(
        bar_end,
        MARKET_TIMEZONE,
        bar_end.with_timezone(&timezone).date_naive(),
    )
    .map_err(|_| reject(BarRejectionReason::InvalidValue, None))?;
    let instrument = request
        .mapping
        .resolve(instrument_key, &observed)
        .map_err(|_| {
            reject(
                BarRejectionReason::UnmappedInstrument,
                Some("instrument_key"),
            )
        })?
        .clone();

    let price = |value: &RawDecimal, field: &str| {
        decimal(
            value,
            &units.price,
            units.price_scale,
            units.price_precision,
        )
        .map_err(|reason| reject(reason, Some(field)))
    };
    let amount = |value: &RawDecimal, field: &str| {
        decimal(
            value,
            &units.amount,
            units.amount_scale,
            units.amount_precision,
        )
        .map_err(|reason| reject(reason, Some(field)))
    };
    let prices = match (&raw.open, &raw.high, &raw.low, &raw.close) {
        (Some(open), Some(high), Some(low), Some(close)) => Some(BarPrices {
            open: price(open, "open")?,
            high: price(high, "high")?,
            low: price(low, "low")?,
            close: price(close, "close")?,
        }),
        (None, None, None, None) => None,
        _ => return Err(reject(BarRejectionReason::InconsistentOhlc, None)),
    };
    let close_yield = match (&raw.close_yield, &units.yield_unit) {
        (None, _) => None,
        (Some(value), Some((unit, scale, precision))) => Some(
            decimal(value, unit, *scale, *precision)
                .map_err(|reason| reject(reason, Some("close_yield")))?,
        ),
        (Some(_), None) => {
            return Err(reject(
                BarRejectionReason::InvalidValue,
                Some("close_yield"),
            ));
        }
    };
    let input = BarInput {
        source_record_id: raw.source_record_id.clone().unwrap_or_default(),
        instrument,
        frequency,
        settlement,
        market_timezone: MARKET_TIMEZONE.to_owned(),
        local_trading_date: trading_date,
        bar_start,
        bar_end,
        visible_at,
        prices,
        volume: amount(volume, "volume")?,
        turnover: raw
            .turnover
            .as_ref()
            .map(|value| amount(value, "turnover"))
            .transpose()?,
        trade_count: raw.trade_count,
        vwap: raw
            .vwap
            .as_ref()
            .map(|value| price(value, "vwap"))
            .transpose()?,
        close_yield,
    };
    Bar::new(input).map(Some).map_err(|violation| {
        let reason = match violation {
            BarRuleViolation::MisalignedInterval => BarRejectionReason::MisalignedInterval,
            BarRuleViolation::InconsistentOhlc => BarRejectionReason::InconsistentOhlc,
            BarRuleViolation::NegativeVolume => BarRejectionReason::NegativeVolume,
            BarRuleViolation::ObservedAfterVisible => BarRejectionReason::ObservedAfterVisible,
            BarRuleViolation::InvalidValue => BarRejectionReason::InvalidValue,
        };
        reject(reason, None)
    })
}

fn row_identity(raw: &RawBarRow) -> String {
    match &raw.source_record_id {
        Some(id)
            if !id.is_empty()
                && id.len() <= 128
                && id.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                }) =>
        {
            id.clone()
        }
        _ => format!("row:{}", raw.row_ordinal),
    }
}

fn required<'a, T>(
    value: Option<&'a T>,
    field: &str,
    reject: &impl Fn(BarRejectionReason, Option<&str>) -> BarRejection,
) -> RowResult<&'a T> {
    value.ok_or_else(|| reject(BarRejectionReason::MissingRequiredField, Some(field)))
}

/// Resolves the interval: intraday bars from their start, daily bars from the calendar session.
fn interval(
    request: &CanonicalBarIngestRequest,
    frequency: BarFrequency,
    raw: &RawBarRow,
    timezone: Tz,
    reject: &impl Fn(BarRejectionReason, Option<&str>) -> BarRejection,
) -> RowResult<(DateTime<Utc>, DateTime<Utc>, NaiveDate)> {
    let session_bounds = |date: NaiveDate| -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let session = request
            .calendar
            .sessions()
            .iter()
            .find(|session| session.local_date() == date)?;
        let open = local_instant(timezone, date, session.open_local_time()?)?;
        let close = local_instant(timezone, date, session.close_local_time()?)?;
        let effective = request.calendar.effective();
        (effective.from().instant() <= open && close <= effective.to().instant())
            .then_some((open, close))
    };
    if let Some(minutes) = frequency.interval_minutes() {
        let start = *required(raw.bar_start.as_ref(), "bar_start", reject)?;
        let end = start + Duration::minutes(i64::from(minutes));
        let date = start.with_timezone(&timezone).date_naive();
        let (open, close) = session_bounds(date)
            .ok_or_else(|| reject(BarRejectionReason::OutsideTradingSession, None))?;
        if start < open || end > close {
            return Err(reject(BarRejectionReason::OutsideTradingSession, None));
        }
        return Ok((start, end, date));
    }
    let date = *required(raw.trading_date.as_ref(), "trading_date", reject)?;
    let (open, close) = session_bounds(date)
        .ok_or_else(|| reject(BarRejectionReason::OutsideTradingSession, None))?;
    Ok((open, close, date))
}

fn local_instant(timezone: Tz, date: NaiveDate, time: NaiveTime) -> Option<DateTime<Utc>> {
    timezone
        .from_local_datetime(&date.and_time(time))
        .single()
        .map(|value| value.with_timezone(&Utc))
}

fn decimal(
    raw: &RawDecimal,
    unit: &UnitRef,
    scale: u32,
    precision: u32,
) -> Result<DecimalValue, BarRejectionReason> {
    let value = DecimalValue::new(raw.coefficient(), raw.scale(), unit.clone())
        .map_err(|_| BarRejectionReason::InvalidValue)?;
    let digits = u32::try_from(value.coefficient().trim_start_matches('-').len())
        .map_err(|_| BarRejectionReason::InvalidValue)?;
    if value.scale() > scale || digits > precision {
        return Err(BarRejectionReason::PrecisionExceeded);
    }
    Ok(value)
}

/// Rejects every row whose bar key or source record ID occurs more than once: which copy is right
/// cannot be decided from the data, so none is kept.
fn reject_duplicates(accepted: Vec<Bar>, rejections: &mut Vec<BarRejection>) -> Vec<Bar> {
    let key = |bar: &Bar| {
        (
            bar.instrument().id().as_str().to_owned(),
            bar.instrument().version().get(),
            bar.frequency(),
            bar.settlement(),
            bar.bar_start(),
        )
    };
    let mut key_counts = HashMap::new();
    let mut id_counts = HashMap::new();
    for bar in &accepted {
        *key_counts.entry(key(bar)).or_insert(0_usize) += 1;
        *id_counts
            .entry(bar.source_record_id().to_owned())
            .or_insert(0_usize) += 1;
    }
    let mut kept = Vec::with_capacity(accepted.len());
    for bar in accepted {
        if key_counts[&key(&bar)] > 1 || id_counts[bar.source_record_id()] > 1 {
            rejections.push(BarRejection {
                row_id: bar.source_record_id().to_owned(),
                reason: BarRejectionReason::DuplicateBar,
                field: None,
            });
        } else {
            kept.push(bar);
        }
    }
    kept.sort_by(|left, right| {
        left.bar_start()
            .cmp(&right.bar_start())
            .then_with(|| {
                left.instrument()
                    .id()
                    .as_str()
                    .cmp(right.instrument().id().as_str())
            })
            .then_with(|| left.frequency().cmp(&right.frequency()))
            .then_with(|| left.settlement().cmp(&right.settlement()))
            .then_with(|| left.source_record_id().cmp(right.source_record_id()))
    });
    kept
}

pub(crate) fn price_source_from_code(code: &str) -> Option<PriceSourceType> {
    [
        PriceSourceType::RealTrade,
        PriceSourceType::ActiveQuote,
        PriceSourceType::ModelValuation,
        PriceSourceType::CurveInterpolation,
    ]
    .into_iter()
    .find(|value| price_source_code(*value) == code)
}

fn price_source_code(value: PriceSourceType) -> &'static str {
    match value {
        PriceSourceType::RealTrade => "REAL_TRADE",
        PriceSourceType::ActiveQuote => "ACTIVE_QUOTE",
        PriceSourceType::ModelValuation => "MODEL_VALUATION",
        PriceSourceType::CurveInterpolation => "CURVE_INTERPOLATION",
    }
}

fn decimal_field(prefix: &str, nullable: bool) -> [Field; 2] {
    [
        text_field(&format!("{prefix}_coefficient"), nullable),
        Field::new(format!("{prefix}_scale"), DataType::UInt32, nullable),
    ]
}

pub fn canonical_bar_schema() -> Schema {
    let mut fields = vec![
        text_field("tenant_id", false),
        text_field("owner_id", false),
        text_field("data_source_id", false),
        Field::new("data_source_version", DataType::UInt64, false),
        text_field("price_source_type", false),
        text_field("source_record_id", false),
        text_field("instrument_id", false),
        Field::new("instrument_version", DataType::UInt64, false),
        text_field("frequency", false),
        text_field("settlement", false),
        timestamp_field("bar_start"),
        timestamp_field("bar_end"),
        timestamp_field("observed_at"),
        timestamp_field("visible_at"),
        Field::new("local_trading_date", DataType::Date32, false),
    ];
    for prefix in ["open", "high", "low", "close"] {
        fields.extend(decimal_field(prefix, true));
    }
    fields.push(text_field("price_unit_id", false));
    fields.push(Field::new("price_unit_version", DataType::UInt64, false));
    fields.extend(decimal_field("volume", false));
    fields.extend(decimal_field("turnover", true));
    fields.push(text_field("amount_unit_id", false));
    fields.push(Field::new("amount_unit_version", DataType::UInt64, false));
    fields.push(Field::new("trade_count", DataType::UInt64, true));
    fields.extend(decimal_field("vwap", true));
    fields.extend(decimal_field("close_yield", true));
    fields.push(text_field("yield_unit_id", true));
    fields.push(Field::new("yield_unit_version", DataType::UInt64, true));
    Schema::new_with_metadata(
        fields,
        HashMap::from([
            (
                "ficant.decimal.encoding".to_owned(),
                "coefficient+scale".to_owned(),
            ),
            (
                "ficant.market.timezone".to_owned(),
                MARKET_TIMEZONE.to_owned(),
            ),
            (
                "ficant.schema.id".to_owned(),
                CANONICAL_BAR_SCHEMA_ID.to_owned(),
            ),
            (
                "ficant.sort".to_owned(),
                "bar_start,instrument_id,frequency,settlement,source_record_id".to_owned(),
            ),
        ]),
    )
}

pub fn canonical_bar_schema_hash() -> ContentHash {
    ContentHash::digest(&schema_contract_bytes(&canonical_bar_schema()))
}

#[allow(clippy::too_many_lines)]
fn encode_batch(
    request: &CanonicalBarIngestRequest,
    units: &Units,
    bars: &[Bar],
) -> DataResult<RecordBatch> {
    let owner = request.source.owner();
    let rows = bars.len();
    let repeat = |value: &str| -> ArrayRef { Arc::new(StringArray::from(vec![value; rows])) };
    let repeat_u64 = |value: u64| -> ArrayRef { Arc::new(UInt64Array::from(vec![value; rows])) };
    let price_source = request
        .source
        .price_source_type()
        .ok_or(DataError::InvalidConfiguration)?;
    let timestamps = |select: fn(&Bar) -> DateTime<Utc>| -> ArrayRef {
        Arc::new(
            TimestampMicrosecondArray::from(
                bars.iter()
                    .map(|bar| select(bar).timestamp_micros())
                    .collect::<Vec<_>>(),
            )
            .with_timezone("UTC"),
        )
    };
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).ok_or(DataError::InvalidConfiguration)?;
    let dates = bars
        .iter()
        .map(|bar| {
            i32::try_from((bar.local_trading_date() - epoch).num_days())
                .map_err(|_| DataError::InvalidConfiguration)
        })
        .collect::<DataResult<Vec<_>>>()?;
    let decimal_columns = |select: &dyn Fn(&Bar) -> Option<&DecimalValue>| -> [ArrayRef; 2] {
        [
            Arc::new(StringArray::from(
                bars.iter()
                    .map(|bar| select(bar).map(|value| value.coefficient().to_owned()))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(UInt32Array::from(
                bars.iter()
                    .map(|bar| select(bar).map(DecimalValue::scale))
                    .collect::<Vec<_>>(),
            )),
        ]
    };
    let mut columns: Vec<ArrayRef> = vec![
        repeat(owner.tenant_id().as_str()),
        repeat(owner.owner_id().as_str()),
        repeat(request.source.id().as_str()),
        repeat_u64(request.source.version()),
        repeat(price_source_code(price_source)),
        Arc::new(StringArray::from(
            bars.iter().map(Bar::source_record_id).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            bars.iter()
                .map(|bar| bar.instrument().id().as_str())
                .collect::<Vec<_>>(),
        )),
        Arc::new(UInt64Array::from(
            bars.iter()
                .map(|bar| bar.instrument().version().get())
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            bars.iter()
                .map(|bar| bar.frequency().code())
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            bars.iter()
                .map(|bar| bar.settlement().code())
                .collect::<Vec<_>>(),
        )),
        timestamps(Bar::bar_start),
        timestamps(Bar::bar_end),
        timestamps(Bar::observed_at),
        timestamps(Bar::visible_at),
        Arc::new(Date32Array::from(dates)),
    ];
    columns.extend(decimal_columns(&|bar| {
        bar.prices().map(|prices| &prices.open)
    }));
    columns.extend(decimal_columns(&|bar| {
        bar.prices().map(|prices| &prices.high)
    }));
    columns.extend(decimal_columns(&|bar| {
        bar.prices().map(|prices| &prices.low)
    }));
    columns.extend(decimal_columns(&|bar| {
        bar.prices().map(|prices| &prices.close)
    }));
    columns.push(repeat(units.price.unit_id().as_str()));
    columns.push(repeat_u64(units.price.version().get()));
    columns.extend(decimal_columns(&|bar| Some(bar.volume())));
    columns.extend(decimal_columns(&Bar::turnover));
    columns.push(repeat(units.amount.unit_id().as_str()));
    columns.push(repeat_u64(units.amount.version().get()));
    columns.push(Arc::new(UInt64Array::from(
        bars.iter().map(Bar::trade_count).collect::<Vec<_>>(),
    )));
    columns.extend(decimal_columns(&Bar::vwap));
    columns.extend(decimal_columns(&Bar::close_yield));
    let yield_id = units
        .yield_unit
        .as_ref()
        .map(|(unit, _, _)| unit.unit_id().as_str().to_owned());
    let yield_version = units
        .yield_unit
        .as_ref()
        .map(|(unit, _, _)| unit.version().get());
    columns.push(Arc::new(StringArray::from(
        bars.iter()
            .map(|bar| bar.close_yield().and(yield_id.clone()))
            .collect::<Vec<_>>(),
    )));
    columns.push(Arc::new(UInt64Array::from(
        bars.iter()
            .map(|bar| bar.close_yield().and(yield_version))
            .collect::<Vec<_>>(),
    )));
    RecordBatch::try_new(Arc::new(canonical_bar_schema()), columns)
        .map_err(|_| DataError::SchemaMismatch)
}
