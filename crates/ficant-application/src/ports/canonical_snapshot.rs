use async_trait::async_trait;
use chrono::NaiveDate;
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::primitives::{MarketTime, UnitRef, VersionRef};
use ficant_domain::research::DataSnapshot;

use super::ApplicationResult;
use crate::map_domain_error;
use ficant_domain::DomainErrorCode;

/// One provider-neutral two-time quote projected from a verified canonical snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalQuote {
    instrument: VersionRef,
    observed_at: MarketTime,
    visible_at: MarketTime,
    local_trading_date: NaiveDate,
    bid: Option<FixedDecimal>,
    ask: Option<FixedDecimal>,
    unit: UnitRef,
}

impl CanonicalQuote {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        instrument: VersionRef,
        observed_at: MarketTime,
        visible_at: MarketTime,
        local_trading_date: NaiveDate,
        bid: Option<FixedDecimal>,
        ask: Option<FixedDecimal>,
        unit: UnitRef,
    ) -> Self {
        Self {
            instrument,
            observed_at,
            visible_at,
            local_trading_date,
            bid,
            ask,
            unit,
        }
    }

    #[must_use]
    pub fn instrument(&self) -> &VersionRef {
        &self.instrument
    }

    #[must_use]
    pub fn observed_at(&self) -> &MarketTime {
        &self.observed_at
    }

    #[must_use]
    pub fn visible_at(&self) -> &MarketTime {
        &self.visible_at
    }

    #[must_use]
    pub const fn local_trading_date(&self) -> NaiveDate {
        self.local_trading_date
    }

    #[must_use]
    pub const fn bid(&self) -> Option<FixedDecimal> {
        self.bid
    }

    #[must_use]
    pub const fn ask(&self) -> Option<FixedDecimal> {
        self.ask
    }

    #[must_use]
    pub fn unit(&self) -> &UnitRef {
        &self.unit
    }
}

/// One verified canonical quote projection plus the exact immutable `DataSource` version declared
/// by its canonical manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedCanonicalQuotes {
    data_source: VersionRef,
    quotes: Vec<CanonicalQuote>,
}

impl DecodedCanonicalQuotes {
    /// Binds one non-empty quote projection to its exact manifest source.
    ///
    /// # Errors
    ///
    /// Returns validation failure for an empty projection, which cannot supply a typed price
    /// record to any calculation.
    pub fn new(data_source: VersionRef, quotes: Vec<CanonicalQuote>) -> ApplicationResult<Self> {
        if quotes.is_empty() {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        Ok(Self {
            data_source,
            quotes,
        })
    }

    #[must_use]
    pub fn data_source(&self) -> &VersionRef {
        &self.data_source
    }

    #[must_use]
    pub fn quotes(&self) -> &[CanonicalQuote] {
        &self.quotes
    }

    #[must_use]
    pub fn into_parts(self) -> (VersionRef, Vec<CanonicalQuote>) {
        (self.data_source, self.quotes)
    }
}

/// Decodes only the quote projection needed by futures-delivery materialization.
///
/// The adapter receives bytes only after both snapshot roles have passed required-read
/// verification. Implementations must not perform storage reads or reinterpret the canonical
/// schema.
#[async_trait]
pub trait CanonicalSnapshotDecoder: Send + Sync {
    /// Decodes an exact Parquet/Manifest pair into provider-neutral quotes.
    ///
    /// # Errors
    ///
    /// Returns a stable application error for schema, manifest, or payload disagreement.
    async fn decode_quotes(
        &self,
        snapshot: &DataSnapshot,
        parquet: &[u8],
        manifest: &[u8],
    ) -> ApplicationResult<DecodedCanonicalQuotes>;
}
