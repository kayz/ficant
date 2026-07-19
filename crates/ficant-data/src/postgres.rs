use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ficant_domain::market::{DataSource, DataSourceKind};
use sqlx::PgPool;

use crate::{DataError, DataResult, PointInTimeWindow, RawDecimal, RawQuoteRow, RawQuoteSource};

#[derive(Clone, Debug)]
pub struct PostgresQuoteSource {
    connection_binding: String,
    pool: PgPool,
}

impl PostgresQuoteSource {
    pub fn new(connection_binding: impl Into<String>, pool: PgPool) -> DataResult<Self> {
        let connection_binding = connection_binding.into();
        if connection_binding.trim().is_empty() || connection_binding != connection_binding.trim() {
            return Err(DataError::InvalidConfiguration);
        }
        Ok(Self {
            connection_binding,
            pool,
        })
    }
}

#[async_trait]
impl RawQuoteSource for PostgresQuoteSource {
    async fn read(
        &self,
        source: &DataSource,
        window: &PointInTimeWindow,
    ) -> DataResult<Vec<RawQuoteRow>> {
        if source.kind() != DataSourceKind::Postgres
            || source.connection_binding() != self.connection_binding
            || source.dataset() != "ficant_source_quotes_v1"
        {
            return Err(DataError::InvalidConfiguration);
        }
        let rows: Vec<PostgresQuoteRow> = sqlx::query_as(
            "SELECT source_record_id, instrument_key, observed_at, visible_at,
                    bid_coefficient, bid_scale, ask_coefficient, ask_scale
             FROM external_data.ficant_source_quotes_v1
             WHERE observed_at <= $1 AND visible_at <= $2
             ORDER BY observed_at, instrument_key, source_record_id",
        )
        .bind(window.as_of().instant())
        .bind(window.visible_at_cutoff().instant())
        .fetch_all(&self.pool)
        .await
        .map_err(|_| DataError::SourceUnavailable)?;
        rows.into_iter().map(decode_row).collect()
    }
}

type PostgresQuoteRow = (
    String,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<String>,
    Option<i32>,
    Option<String>,
    Option<i32>,
);

fn decode_row(row: PostgresQuoteRow) -> DataResult<RawQuoteRow> {
    let (
        source_record_id,
        instrument_key,
        observed_at,
        visible_at,
        bid_coefficient,
        bid_scale,
        ask_coefficient,
        ask_scale,
    ) = row;
    Ok(RawQuoteRow::new(
        source_record_id,
        instrument_key,
        observed_at.to_rfc3339(),
        visible_at.to_rfc3339(),
        decimal(bid_coefficient, bid_scale)?,
        decimal(ask_coefficient, ask_scale)?,
    ))
}

fn decimal(coefficient: Option<String>, scale: Option<i32>) -> DataResult<Option<RawDecimal>> {
    match (coefficient, scale) {
        (None, None) => Ok(None),
        (Some(coefficient), Some(scale)) => Ok(Some(RawDecimal::new(
            coefficient,
            u32::try_from(scale).map_err(|_| DataError::InvalidSourceData)?,
        ))),
        _ => Err(DataError::InvalidSourceData),
    }
}
