mod bond;
mod calendar;
mod cashflow;
mod curve_snapshot;
mod data_source;
mod futures_contract;
mod instrument;
mod market_rule_pack;
mod quote;
mod trade;
mod unit;
mod valuation;

pub use bond::Bond;
pub use calendar::{Calendar, CalendarInput, CalendarSession};
pub use cashflow::{Cashflow, CashflowInput, CashflowType};
pub use curve_snapshot::{ArtifactInputKind, CurveSnapshot, CurveSnapshotInput};
pub use data_source::{DataSource, DataSourceInput, DataSourceKind};
pub use futures_contract::FuturesContract;
pub use instrument::{Instrument, InstrumentInput, InstrumentKind};
pub use market_rule_pack::{
    MarketRulePack, MarketRulePackInput, MarketRulePackTimesInput, VerificationStatus,
};
pub use quote::{Quote, QuoteInput};
pub use trade::{Trade, TradeInput};
pub use unit::{Unit, UnitInput};
pub use valuation::{Valuation, ValuationInput};

use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactSource {
    source_id: String,
    external_id: String,
    source_revision: u64,
}

impl FactSource {
    pub fn new(
        source_id: impl Into<String>,
        external_id: impl Into<String>,
        source_revision: u64,
    ) -> DomainResult<Self> {
        let source_id = source_id.into();
        let external_id = external_id.into();
        require_text(&source_id)?;
        require_text(&external_id)?;
        if source_revision == 0 {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            source_id,
            external_id,
            source_revision,
        })
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn external_id(&self) -> &str {
        &self.external_id
    }

    pub fn source_revision(&self) -> u64 {
        self.source_revision
    }
}

pub(crate) fn require_text(value: &str) -> Result<(), DomainErrorCode> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}
