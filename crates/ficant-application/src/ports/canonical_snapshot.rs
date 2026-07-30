use async_trait::async_trait;
use chrono::NaiveDate;
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::primitives::{MarketTime, UnitRef, VersionRef};
use ficant_domain::research::DataSnapshot;

use super::ApplicationResult;

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
    ) -> ApplicationResult<Vec<CanonicalQuote>>;
}
