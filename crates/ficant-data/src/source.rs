use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ficant_domain::market::DataSource;
use ficant_domain::primitives::MarketTime;

use crate::{DataError, DataResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawDecimal {
    coefficient: String,
    scale: u32,
}

impl RawDecimal {
    pub fn new(coefficient: impl Into<String>, scale: u32) -> Self {
        Self {
            coefficient: coefficient.into(),
            scale,
        }
    }

    pub fn coefficient(&self) -> &str {
        &self.coefficient
    }

    pub fn scale(&self) -> u32 {
        self.scale
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawQuoteRow {
    source_record_id: String,
    instrument_key: String,
    observed_at: String,
    visible_at: String,
    bid: Option<RawDecimal>,
    ask: Option<RawDecimal>,
}

impl RawQuoteRow {
    pub fn new(
        source_record_id: impl Into<String>,
        instrument_key: impl Into<String>,
        observed_at: impl Into<String>,
        visible_at: impl Into<String>,
        bid: Option<RawDecimal>,
        ask: Option<RawDecimal>,
    ) -> Self {
        Self {
            source_record_id: source_record_id.into(),
            instrument_key: instrument_key.into(),
            observed_at: observed_at.into(),
            visible_at: visible_at.into(),
            bid,
            ask,
        }
    }

    pub fn source_record_id(&self) -> &str {
        &self.source_record_id
    }

    pub fn instrument_key(&self) -> &str {
        &self.instrument_key
    }

    pub fn observed_at(&self) -> &str {
        &self.observed_at
    }

    pub fn visible_at(&self) -> &str {
        &self.visible_at
    }

    pub fn bid(&self) -> Option<&RawDecimal> {
        self.bid.as_ref()
    }

    pub fn ask(&self) -> Option<&RawDecimal> {
        self.ask.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PointInTimeWindow {
    as_of: MarketTime,
    visible_at_cutoff: MarketTime,
}

impl PointInTimeWindow {
    pub fn new(as_of: MarketTime, visible_at_cutoff: MarketTime) -> DataResult<Self> {
        if as_of.instant() > visible_at_cutoff.instant()
            || as_of.market_timezone() != visible_at_cutoff.market_timezone()
        {
            return Err(DataError::PointInTimeViolation);
        }
        Ok(Self {
            as_of,
            visible_at_cutoff,
        })
    }

    pub fn as_of(&self) -> &MarketTime {
        &self.as_of
    }

    pub fn visible_at_cutoff(&self) -> &MarketTime {
        &self.visible_at_cutoff
    }
}

#[async_trait]
pub trait RawQuoteSource: Send + Sync {
    async fn read(
        &self,
        source: &DataSource,
        window: &PointInTimeWindow,
    ) -> DataResult<Vec<RawQuoteRow>>;
}

pub(crate) fn parse_source_time(value: &str) -> DataResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|_| DataError::InvalidSourceData)
}

pub(crate) fn row_is_visible(row: &RawQuoteRow, window: &PointInTimeWindow) -> DataResult<bool> {
    let observed_at = parse_source_time(row.observed_at())?;
    let visible_at = parse_source_time(row.visible_at())?;
    Ok(observed_at <= window.as_of().instant()
        && visible_at <= window.visible_at_cutoff().instant())
}
