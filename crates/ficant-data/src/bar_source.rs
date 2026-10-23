//! External bar sources (ADR-0023).
//!
//! An adapter only maps one source's field layout into [`RawBarRow`]. Every rule that defines the
//! canonical Bar is enforced once, in [`crate::CanonicalBarIngestor`]; adapters cannot relax it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use ficant_domain::market::{BarFrequency, BarSettlement, DataSource, DataSourceKind};
use serde_json::Value;

use crate::{DataError, DataResult, RawDecimal};

/// One source row after field mapping, before canonical validation.
///
/// Fields are `None` when the source did not provide them; `invalid_field` names the first field
/// that was present but could not be parsed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawBarRow {
    /// 1-based position in the source; identifies the row when it has no usable record ID.
    pub row_ordinal: u64,
    pub source_record_id: Option<String>,
    pub instrument_key: Option<String>,
    pub frequency: Option<BarFrequency>,
    pub settlement: Option<BarSettlement>,
    /// Interval start for intraday frequencies.
    pub bar_start: Option<DateTime<Utc>>,
    /// Trading date for daily bars.
    pub trading_date: Option<NaiveDate>,
    /// Source visibility time, when the source has one.
    pub visible_at: Option<DateTime<Utc>>,
    pub open: Option<RawDecimal>,
    pub high: Option<RawDecimal>,
    pub low: Option<RawDecimal>,
    pub close: Option<RawDecimal>,
    pub volume: Option<RawDecimal>,
    pub turnover: Option<RawDecimal>,
    pub trade_count: Option<u64>,
    pub vwap: Option<RawDecimal>,
    pub close_yield: Option<RawDecimal>,
    pub invalid_field: Option<String>,
}

/// How a source states when each bar became known (I1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarVisibilityRule {
    /// The source carries a visibility time per row.
    SourceField,
    /// The source has no visibility field; bars become visible a fixed delay after they end.
    FixedDelay(Duration),
}

impl BarVisibilityRule {
    /// Stable textual form recorded with imported bars.
    pub fn describe(&self) -> String {
        match self {
            Self::SourceField => "source-field".to_owned(),
            Self::FixedDelay(delay) => format!("fixed-delay:{}s", delay.num_seconds()),
        }
    }
}

#[async_trait]
pub trait BarSourceAdapter: Send + Sync {
    /// The visibility rule declared in this source's registration.
    fn visibility_rule(&self) -> BarVisibilityRule;

    /// Reads all rows of the source dataset. Point-in-time filtering happens in the ingestor,
    /// because a daily bar's end time needs the market calendar.
    async fn read(&self, source: &DataSource) -> DataResult<Vec<RawBarRow>>;
}

/// A value taken either from a source field (with a source-code translation) or fixed for the
/// whole source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeField<T> {
    Fixed(T),
    Field {
        path: String,
        codes: BTreeMap<String, T>,
    },
}

/// How a source writes timestamps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceTimeFormat {
    /// RFC 3339 with an explicit offset.
    Rfc3339,
    /// Milliseconds since the Unix epoch, UTC.
    EpochMillis,
    /// Local wall-clock text without an offset, interpreted in the given timezone.
    LocalNaive { format: String, timezone: Tz },
}

/// Field layout of one NDJSON bar source. Paths are dot-separated object keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NdjsonBarMapping {
    pub source_record_id: String,
    pub instrument_key: String,
    pub frequency: CodeField<BarFrequency>,
    pub settlement: CodeField<BarSettlement>,
    pub bar_start: Option<String>,
    pub trading_date: Option<String>,
    pub trading_date_format: String,
    pub time_format: SourceTimeFormat,
    pub visibility: BarVisibilityRule,
    pub visible_at: Option<String>,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
    pub turnover: Option<String>,
    pub trade_count: Option<String>,
    pub vwap: Option<String>,
    pub close_yield: Option<String>,
}

/// File-backed NDJSON bar adapter; one source registration owns one field mapping.
#[derive(Clone, Debug)]
pub struct FileNdjsonBarSource {
    connection_binding: String,
    root: PathBuf,
    mapping: NdjsonBarMapping,
}

impl FileNdjsonBarSource {
    pub fn new(
        connection_binding: impl Into<String>,
        root: PathBuf,
        mapping: NdjsonBarMapping,
    ) -> DataResult<Self> {
        let connection_binding = connection_binding.into();
        let visibility_field_consistent = matches!(
            (&mapping.visibility, &mapping.visible_at),
            (BarVisibilityRule::SourceField, Some(_)) | (BarVisibilityRule::FixedDelay(_), None)
        );
        let delay_valid = match mapping.visibility {
            BarVisibilityRule::FixedDelay(delay) => delay >= Duration::zero(),
            BarVisibilityRule::SourceField => true,
        };
        if connection_binding.trim().is_empty()
            || connection_binding != connection_binding.trim()
            || !root.is_absolute()
            || !visibility_field_consistent
            || !delay_valid
            || (mapping.bar_start.is_none() && mapping.trading_date.is_none())
        {
            return Err(DataError::InvalidConfiguration);
        }
        Ok(Self {
            connection_binding,
            root,
            mapping,
        })
    }
}

#[async_trait]
impl BarSourceAdapter for FileNdjsonBarSource {
    fn visibility_rule(&self) -> BarVisibilityRule {
        self.mapping.visibility
    }

    async fn read(&self, source: &DataSource) -> DataResult<Vec<RawBarRow>> {
        if source.kind() != DataSourceKind::FileNdjson
            || source.connection_binding() != self.connection_binding
        {
            return Err(DataError::InvalidConfiguration);
        }
        let path = self.root.join(format!("{}.ndjson", source.dataset()));
        let payload = tokio::fs::read_to_string(path)
            .await
            .map_err(|_| DataError::SourceUnavailable)?;
        let mut rows = Vec::new();
        let mut ordinal = 0_u64;
        for line in payload.lines() {
            if line.trim().is_empty() {
                continue;
            }
            ordinal += 1;
            rows.push(map_row(&self.mapping, ordinal, line));
        }
        Ok(rows)
    }
}

fn map_row(mapping: &NdjsonBarMapping, ordinal: u64, line: &str) -> RawBarRow {
    let mut row = RawBarRow {
        row_ordinal: ordinal,
        ..RawBarRow::default()
    };
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        row.invalid_field = Some("<json>".to_owned());
        return row;
    };
    let mut reader = RowReader {
        value: &value,
        invalid: None,
    };
    row.source_record_id = reader.text(&mapping.source_record_id);
    row.instrument_key = reader.text(&mapping.instrument_key);
    row.frequency = reader.code(&mapping.frequency);
    row.settlement = reader.code(&mapping.settlement);
    row.bar_start = mapping
        .bar_start
        .as_deref()
        .and_then(|path| reader.time(path, &mapping.time_format));
    row.trading_date = mapping
        .trading_date
        .as_deref()
        .and_then(|path| reader.date(path, &mapping.trading_date_format));
    row.visible_at = mapping
        .visible_at
        .as_deref()
        .and_then(|path| reader.time(path, &mapping.time_format));
    row.open = reader.decimal(&mapping.open);
    row.high = reader.decimal(&mapping.high);
    row.low = reader.decimal(&mapping.low);
    row.close = reader.decimal(&mapping.close);
    row.volume = reader.decimal(&mapping.volume);
    row.turnover = mapping
        .turnover
        .as_deref()
        .and_then(|path| reader.decimal(path));
    row.trade_count = mapping
        .trade_count
        .as_deref()
        .and_then(|path| reader.count(path));
    row.vwap = mapping
        .vwap
        .as_deref()
        .and_then(|path| reader.decimal(path));
    row.close_yield = mapping
        .close_yield
        .as_deref()
        .and_then(|path| reader.decimal(path));
    row.invalid_field = reader.invalid;
    row
}

struct RowReader<'a> {
    value: &'a Value,
    invalid: Option<String>,
}

impl RowReader<'_> {
    fn lookup(&self, path: &str) -> Option<&Value> {
        let mut current = self.value;
        for key in path.split('.') {
            current = current.as_object()?.get(key)?;
        }
        (!current.is_null()).then_some(current)
    }

    fn fail(&mut self, path: &str) {
        if self.invalid.is_none() {
            self.invalid = Some(path.to_owned());
        }
    }

    fn scalar_text(&mut self, path: &str) -> Option<String> {
        let value = self.lookup(path)?;
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            _ => {
                self.fail(path);
                return None;
            }
        };
        if text.trim().is_empty() {
            return None;
        }
        Some(text)
    }

    fn text(&mut self, path: &str) -> Option<String> {
        self.scalar_text(path)
    }

    fn code<T: Copy>(&mut self, field: &CodeField<T>) -> Option<T> {
        match field {
            CodeField::Fixed(value) => Some(*value),
            CodeField::Field { path, codes } => {
                let text = self.scalar_text(path)?;
                let value = codes.get(&text).copied();
                if value.is_none() {
                    self.fail(path);
                }
                value
            }
        }
    }

    fn time(&mut self, path: &str, format: &SourceTimeFormat) -> Option<DateTime<Utc>> {
        let text = self.scalar_text(path)?;
        let parsed = match format {
            SourceTimeFormat::Rfc3339 => DateTime::parse_from_rfc3339(&text)
                .ok()
                .map(|value| value.with_timezone(&Utc)),
            SourceTimeFormat::EpochMillis => text
                .parse::<i64>()
                .ok()
                .and_then(DateTime::<Utc>::from_timestamp_millis),
            SourceTimeFormat::LocalNaive { format, timezone } => {
                NaiveDateTime::parse_from_str(&text, format)
                    .ok()
                    .and_then(|local| timezone.from_local_datetime(&local).single())
                    .map(|value| value.with_timezone(&Utc))
            }
        };
        if parsed.is_none() {
            self.fail(path);
        }
        parsed
    }

    fn date(&mut self, path: &str, format: &str) -> Option<NaiveDate> {
        let text = self.scalar_text(path)?;
        let parsed = NaiveDate::parse_from_str(&text, format).ok();
        if parsed.is_none() {
            self.fail(path);
        }
        parsed
    }

    fn decimal(&mut self, path: &str) -> Option<RawDecimal> {
        let text = self.scalar_text(path)?;
        let parsed = parse_decimal_text(&text);
        if parsed.is_none() {
            self.fail(path);
        }
        parsed
    }

    fn count(&mut self, path: &str) -> Option<u64> {
        let text = self.scalar_text(path)?;
        let parsed = text.parse::<u64>().ok();
        if parsed.is_none() {
            self.fail(path);
        }
        parsed
    }
}

/// Parses plain decimal text (`-12.3400`, `100`) into coefficient and scale without going through
/// binary floating point. Exponent notation is rejected.
pub fn parse_decimal_text(text: &str) -> Option<RawDecimal> {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || (unsigned.contains('.') && fraction.is_empty())
    {
        return None;
    }
    let scale = u32::try_from(fraction.len()).ok()?;
    let digits = format!("{integer}{fraction}");
    let coefficient = if negative {
        format!("-{digits}")
    } else {
        digits
    };
    Some(RawDecimal::new(coefficient, scale))
}
