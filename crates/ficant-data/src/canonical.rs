use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use arrow::array::{
    ArrayRef, Date32Array, RecordBatch, StringArray, TimestampMicrosecondArray, UInt32Array,
    UInt64Array,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use ficant_domain::VersionedDefinition;
use ficant_domain::market::{Calendar, DataSource, Unit};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, MarketTime, UnitRef, Version, VersionRef,
};
use rust_decimal::Decimal;

use crate::source::parse_source_time;
use crate::{
    DataError, DataResult, InstrumentMapping, PointInTimeWindow, RawDecimal, RawQuoteRow,
    RawQuoteSource,
};

pub const CANONICAL_QUOTE_SCHEMA_ID: &str = "ficant.market.quote.canonical.v1";
const QUALITY_RULE_SET_ID: &str = "ficant.market.quote.quality.v1";
const MARKET_TIMEZONE: &str = "Asia/Shanghai";

#[derive(Clone, Debug)]
pub struct CanonicalIngestRequest {
    source: DataSource,
    mapping: InstrumentMapping,
    calendar: Calendar,
    unit: Unit,
    window: PointInTimeWindow,
}

impl CanonicalIngestRequest {
    pub fn new(
        source: DataSource,
        mapping: InstrumentMapping,
        calendar: Calendar,
        unit: Unit,
        window: PointInTimeWindow,
    ) -> DataResult<Self> {
        let source_ref = VersionRef::new(
            source.id().clone(),
            Version::new(source.version()).map_err(|_| DataError::InvalidConfiguration)?,
        );
        if source.owner() != mapping.owner()
            || source.owner() != calendar.owner()
            || source.owner() != unit.owner()
            || mapping.source() != &source_ref
            || source.canonical_schema_id() != CANONICAL_QUOTE_SCHEMA_ID
            || source.canonical_schema_hash() != &canonical_quote_schema_hash()
            || unit.dimension() != "price"
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
            unit,
            window,
        })
    }

    pub fn source(&self) -> &DataSource {
        &self.source
    }

    pub fn window(&self) -> &PointInTimeWindow {
        &self.window
    }

    pub fn mapping(&self) -> &InstrumentMapping {
        &self.mapping
    }

    pub fn calendar(&self) -> &Calendar {
        &self.calendar
    }

    pub fn unit(&self) -> &Unit {
        &self.unit
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualityReport {
    rule_set_id: &'static str,
    validated_rows: usize,
    emitted_rows: usize,
}

impl QualityReport {
    pub fn rule_set_id(&self) -> &str {
        self.rule_set_id
    }

    pub fn validated_rows(&self) -> usize {
        self.validated_rows
    }

    pub fn emitted_rows(&self) -> usize {
        self.emitted_rows
    }
}

#[derive(Clone, Debug)]
pub struct CanonicalQuoteBatch {
    batch: RecordBatch,
    schema_hash: ContentHash,
    quality: QualityReport,
}

impl CanonicalQuoteBatch {
    pub fn batch(&self) -> &RecordBatch {
        &self.batch
    }

    pub fn schema_hash(&self) -> &ContentHash {
        &self.schema_hash
    }

    pub fn quality(&self) -> &QualityReport {
        &self.quality
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CanonicalQuoteIngestor;

impl CanonicalQuoteIngestor {
    pub async fn ingest(
        &self,
        adapter: &dyn RawQuoteSource,
        request: &CanonicalIngestRequest,
    ) -> DataResult<CanonicalQuoteBatch> {
        let raw_rows = adapter.read(request.source(), request.window()).await?;
        if raw_rows.is_empty() {
            return Err(DataError::QualityRuleFailed);
        }
        let validated_rows = raw_rows.len();
        let mut seen_ids = HashSet::with_capacity(raw_rows.len());
        let mut rows = Vec::with_capacity(raw_rows.len());
        for raw in raw_rows {
            if !seen_ids.insert(raw.source_record_id().to_owned()) {
                return Err(DataError::QualityRuleFailed);
            }
            rows.push(validate_row(request, &raw)?);
        }
        rows.sort_by(|left, right| {
            left.observed_at
                .cmp(&right.observed_at)
                .then_with(|| left.instrument.id().cmp(right.instrument.id()))
                .then_with(|| left.source_record_id.cmp(&right.source_record_id))
        });
        let batch = encode_batch(request, &rows)?;
        if batch.schema().as_ref() != &canonical_quote_schema() {
            return Err(DataError::SchemaMismatch);
        }
        Ok(CanonicalQuoteBatch {
            schema_hash: canonical_quote_schema_hash(),
            quality: QualityReport {
                rule_set_id: QUALITY_RULE_SET_ID,
                validated_rows,
                emitted_rows: rows.len(),
            },
            batch,
        })
    }
}

struct CanonicalRow {
    source_record_id: String,
    instrument: VersionRef,
    observed_at: DateTime<Utc>,
    visible_at: DateTime<Utc>,
    local_trading_date: NaiveDate,
    bid: Option<DecimalValue>,
    ask: Option<DecimalValue>,
}

fn validate_row(request: &CanonicalIngestRequest, raw: &RawQuoteRow) -> DataResult<CanonicalRow> {
    require_source_text(raw.source_record_id())?;
    require_source_text(raw.instrument_key())?;
    let observed_at = parse_source_time(raw.observed_at())?;
    let visible_at = parse_source_time(raw.visible_at())?;
    if observed_at > visible_at {
        return Err(DataError::QualityRuleFailed);
    }
    if observed_at > request.window.as_of().instant()
        || visible_at > request.window.visible_at_cutoff().instant()
    {
        return Err(DataError::PointInTimeViolation);
    }

    let timezone = MARKET_TIMEZONE
        .parse::<Tz>()
        .map_err(|_| DataError::InvalidConfiguration)?;
    let local = observed_at.with_timezone(&timezone);
    let local_date = local.date_naive();
    let market_time = MarketTime::new(observed_at, MARKET_TIMEZONE, local_date)
        .map_err(|_| DataError::QualityRuleFailed)?;
    validate_calendar(&request.calendar, &market_time, local.time())?;
    let instrument = request
        .mapping
        .resolve(raw.instrument_key(), &market_time)?
        .clone();

    if raw.bid().is_none() && raw.ask().is_none() {
        return Err(DataError::QualityRuleFailed);
    }
    let unit_ref = UnitRef::new(
        ficant_domain::primitives::Ulid::new(request.unit.identity())
            .map_err(|_| DataError::InvalidConfiguration)?,
        Version::new(request.unit.version()).map_err(|_| DataError::InvalidConfiguration)?,
    );
    let bid = raw
        .bid()
        .map(|value| normalize_decimal(value, &request.unit, &unit_ref))
        .transpose()?;
    let ask = raw
        .ask()
        .map(|value| normalize_decimal(value, &request.unit, &unit_ref))
        .transpose()?;
    if let (Some(bid), Some(ask)) = (&bid, &ask)
        && decimal_value(bid)? > decimal_value(ask)?
    {
        return Err(DataError::QualityRuleFailed);
    }
    Ok(CanonicalRow {
        source_record_id: raw.source_record_id().to_owned(),
        instrument,
        observed_at,
        visible_at,
        local_trading_date: local_date,
        bid,
        ask,
    })
}

fn require_source_text(value: &str) -> DataResult<()> {
    if value.trim().is_empty() || value != value.trim() || value.len() > 128 {
        return Err(DataError::QualityRuleFailed);
    }
    Ok(())
}

fn validate_calendar(
    calendar: &Calendar,
    observed_at: &MarketTime,
    local_time: chrono::NaiveTime,
) -> DataResult<()> {
    if calendar.effective().from().instant() > observed_at.instant()
        || observed_at.instant() >= calendar.effective().to().instant()
    {
        return Err(DataError::QualityRuleFailed);
    }
    let session = calendar
        .sessions()
        .iter()
        .find(|session| session.local_date() == observed_at.local_trading_date())
        .ok_or(DataError::QualityRuleFailed)?;
    let (Some(open), Some(close)) = (session.open_local_time(), session.close_local_time()) else {
        return Err(DataError::QualityRuleFailed);
    };
    if local_time < open || local_time > close {
        return Err(DataError::QualityRuleFailed);
    }
    Ok(())
}

fn normalize_decimal(
    raw: &RawDecimal,
    unit: &Unit,
    unit_ref: &UnitRef,
) -> DataResult<DecimalValue> {
    if raw.scale() > unit.scale() {
        return Err(DataError::QualityRuleFailed);
    }
    let value = DecimalValue::new(raw.coefficient(), raw.scale(), unit_ref.clone())
        .map_err(|_| DataError::QualityRuleFailed)?;
    let precision = value.coefficient().trim_start_matches('-').len();
    if u32::try_from(precision).map_err(|_| DataError::QualityRuleFailed)? > unit.precision() {
        return Err(DataError::QualityRuleFailed);
    }
    Ok(value)
}

fn decimal_value(value: &DecimalValue) -> DataResult<Decimal> {
    let coefficient = value
        .coefficient()
        .parse::<i128>()
        .map_err(|_| DataError::QualityRuleFailed)?;
    Ok(Decimal::from_i128_with_scale(coefficient, value.scale()))
}

pub fn canonical_quote_schema() -> Schema {
    Schema::new_with_metadata(
        vec![
            text_field("tenant_id", false),
            text_field("owner_id", false),
            text_field("data_source_id", false),
            Field::new("data_source_version", DataType::UInt64, false),
            text_field("source_record_id", false),
            text_field("instrument_id", false),
            Field::new("instrument_version", DataType::UInt64, false),
            timestamp_field("observed_at"),
            timestamp_field("visible_at"),
            Field::new("local_trading_date", DataType::Date32, false),
            text_field("bid_coefficient", true),
            Field::new("bid_scale", DataType::UInt32, true),
            text_field("ask_coefficient", true),
            Field::new("ask_scale", DataType::UInt32, true),
            text_field("unit_id", false),
            Field::new("unit_version", DataType::UInt64, false),
        ],
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
                CANONICAL_QUOTE_SCHEMA_ID.to_owned(),
            ),
            (
                "ficant.sort".to_owned(),
                "observed_at,instrument_id,source_record_id".to_owned(),
            ),
        ]),
    )
}

pub fn canonical_quote_schema_hash() -> ContentHash {
    ContentHash::digest(&schema_contract_bytes(&canonical_quote_schema()))
}

fn text_field(name: &str, nullable: bool) -> Field {
    Field::new(name, DataType::Utf8, nullable)
}

fn timestamp_field(name: &str) -> Field {
    Field::new(
        name,
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        false,
    )
}

fn schema_contract_bytes(schema: &Schema) -> Vec<u8> {
    let mut bytes = b"ficant-arrow-schema/v1\0".to_vec();
    for field in schema.fields() {
        append_contract_value(&mut bytes, field.name());
        append_contract_value(&mut bytes, data_type_code(field.data_type()));
        bytes.push(u8::from(field.is_nullable()));
    }
    let metadata = schema
        .metadata()
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<BTreeMap<_, _>>();
    for (key, value) in metadata {
        append_contract_value(&mut bytes, key);
        append_contract_value(&mut bytes, value);
    }
    bytes
}

fn append_contract_value(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(
        &u64::try_from(value.len())
            .expect("schema token length fits u64")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(value.as_bytes());
}

fn data_type_code(data_type: &DataType) -> &'static str {
    match data_type {
        DataType::Utf8 => "utf8",
        DataType::UInt32 => "uint32",
        DataType::UInt64 => "uint64",
        DataType::Date32 => "date32",
        DataType::Timestamp(TimeUnit::Microsecond, Some(timezone))
            if timezone.as_ref() == "UTC" =>
        {
            "timestamp-microsecond-utc"
        }
        _ => "unsupported",
    }
}

fn encode_batch(
    request: &CanonicalIngestRequest,
    rows: &[CanonicalRow],
) -> DataResult<RecordBatch> {
    let tenant = request.source.owner().tenant_id().as_str();
    let owner = request.source.owner().owner_id().as_str();
    let source_id = request.source.id().as_str();
    let source_version = request.source.version();
    let unit_id = request.unit.identity();
    let unit_version = request.unit.version();
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from_iter_values(std::iter::repeat_n(
            tenant,
            rows.len(),
        ))),
        Arc::new(StringArray::from_iter_values(std::iter::repeat_n(
            owner,
            rows.len(),
        ))),
        Arc::new(StringArray::from_iter_values(std::iter::repeat_n(
            source_id,
            rows.len(),
        ))),
        Arc::new(UInt64Array::from(vec![source_version; rows.len()])),
        Arc::new(StringArray::from_iter_values(
            rows.iter().map(|row| row.source_record_id.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            rows.iter().map(|row| row.instrument.id().as_str()),
        )),
        Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|row| row.instrument.version().get()),
        )),
        Arc::new(
            TimestampMicrosecondArray::from_iter_values(
                rows.iter().map(|row| row.observed_at.timestamp_micros()),
            )
            .with_timezone("UTC"),
        ),
        Arc::new(
            TimestampMicrosecondArray::from_iter_values(
                rows.iter().map(|row| row.visible_at.timestamp_micros()),
            )
            .with_timezone("UTC"),
        ),
        Arc::new(Date32Array::from_iter_values(
            rows.iter()
                .map(|row| epoch_days(row.local_trading_date))
                .collect::<DataResult<Vec<_>>>()?,
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|row| row.bid.as_ref().map(DecimalValue::coefficient))
                .collect::<Vec<_>>(),
        )),
        Arc::new(UInt32Array::from(
            rows.iter()
                .map(|row| row.bid.as_ref().map(DecimalValue::scale))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|row| row.ask.as_ref().map(DecimalValue::coefficient))
                .collect::<Vec<_>>(),
        )),
        Arc::new(UInt32Array::from(
            rows.iter()
                .map(|row| row.ask.as_ref().map(DecimalValue::scale))
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from_iter_values(std::iter::repeat_n(
            unit_id,
            rows.len(),
        ))),
        Arc::new(UInt64Array::from(vec![unit_version; rows.len()])),
    ];
    RecordBatch::try_new(Arc::new(canonical_quote_schema()), columns)
        .map_err(|_| DataError::SchemaMismatch)
}

fn epoch_days(value: NaiveDate) -> DataResult<i32> {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).ok_or(DataError::SchemaMismatch)?;
    i32::try_from(value.signed_duration_since(epoch).num_days())
        .map_err(|_| DataError::SchemaMismatch)
}
