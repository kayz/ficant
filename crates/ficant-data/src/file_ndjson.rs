use std::path::PathBuf;

use async_trait::async_trait;
use ficant_domain::market::{DataSource, DataSourceKind};
use serde_json::{Map, Value};

use crate::source::row_is_visible;
use crate::{DataError, DataResult, PointInTimeWindow, RawDecimal, RawQuoteRow, RawQuoteSource};

#[derive(Clone, Debug)]
pub struct FileNdjsonQuoteSource {
    connection_binding: String,
    root: PathBuf,
}

impl FileNdjsonQuoteSource {
    pub fn new(connection_binding: impl Into<String>, root: PathBuf) -> DataResult<Self> {
        let connection_binding = connection_binding.into();
        if connection_binding.trim().is_empty()
            || connection_binding != connection_binding.trim()
            || !root.is_absolute()
        {
            return Err(DataError::InvalidConfiguration);
        }
        Ok(Self {
            connection_binding,
            root,
        })
    }
}

#[async_trait]
impl RawQuoteSource for FileNdjsonQuoteSource {
    async fn read(
        &self,
        source: &DataSource,
        window: &PointInTimeWindow,
    ) -> DataResult<Vec<RawQuoteRow>> {
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
        for line in payload.lines() {
            if line.trim().is_empty() || line != line.trim() {
                return Err(DataError::InvalidSourceData);
            }
            let value: Value =
                serde_json::from_str(line).map_err(|_| DataError::InvalidSourceData)?;
            let row = parse_row(&value)?;
            if row_is_visible(&row, window)? {
                rows.push(row);
            }
        }
        Ok(rows)
    }
}

fn parse_row(value: &Value) -> DataResult<RawQuoteRow> {
    let object = value.as_object().ok_or(DataError::InvalidSourceData)?;
    let expected = [
        "ask_coefficient",
        "ask_scale",
        "bid_coefficient",
        "bid_scale",
        "instrument_key",
        "observed_at",
        "source_record_id",
        "visible_at",
    ];
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(DataError::InvalidSourceData);
    }
    Ok(RawQuoteRow::new(
        string(object, "source_record_id")?,
        string(object, "instrument_key")?,
        string(object, "observed_at")?,
        string(object, "visible_at")?,
        optional_decimal(object, "bid_coefficient", "bid_scale")?,
        optional_decimal(object, "ask_coefficient", "ask_scale")?,
    ))
}

fn string(object: &Map<String, Value>, key: &str) -> DataResult<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(DataError::InvalidSourceData)
}

fn optional_decimal(
    object: &Map<String, Value>,
    coefficient_key: &str,
    scale_key: &str,
) -> DataResult<Option<RawDecimal>> {
    let coefficient = object
        .get(coefficient_key)
        .ok_or(DataError::InvalidSourceData)?;
    let scale = object.get(scale_key).ok_or(DataError::InvalidSourceData)?;
    match (coefficient, scale) {
        (Value::Null, Value::Null) => Ok(None),
        (Value::String(coefficient), Value::Number(scale)) => {
            let scale = scale
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or(DataError::InvalidSourceData)?;
            Ok(Some(RawDecimal::new(coefficient.clone(), scale)))
        }
        _ => Err(DataError::InvalidSourceData),
    }
}
