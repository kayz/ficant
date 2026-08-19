mod bond;
mod calendar;
mod cashflow;
mod curve_snapshot;
mod data_source;
mod data_source_authorization;
mod futures_contract;
mod instrument;
mod market_rule_pack;
mod quote;
mod trade;
mod unit;
mod valuation;

pub use bond::{
    Bond, BondBusinessDayConvention, BondCouponFrequency, BondDayCountConvention, BondPricingTerms,
    BondTaxAttributes, IncomeTaxStatus, ValueAddedTaxStatus,
};
pub use calendar::{Calendar, CalendarInput, CalendarSession};
pub use cashflow::{Cashflow, CashflowInput, CashflowType};
pub use curve_snapshot::{ArtifactInputKind, CurveSnapshot, CurveSnapshotInput};
pub use data_source::{DataSource, DataSourceInput, DataSourceKind, PriceSourceType};
pub use data_source_authorization::{
    DataSourceAuthorization, DataSourceAuthorizationInput, DataSourceAuthorizationState,
    ImportInterface, data_source_content_hash,
};
pub use futures_contract::FuturesContract;
pub use instrument::{Instrument, InstrumentInput, InstrumentKind};
pub use market_rule_pack::{
    MarketRulePack, MarketRulePackInput, MarketRulePackTimesInput, RulePackContent,
    VerificationStatus,
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
    data_source: Option<crate::primitives::VersionRef>,
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
            data_source: None,
        })
    }

    pub fn with_data_source(
        mut self,
        data_source: crate::primitives::VersionRef,
    ) -> DomainResult<Self> {
        if self.data_source.is_some() {
            return Err(DomainErrorCode::VersionConflict);
        }
        self.data_source = Some(data_source);
        Ok(self)
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

    pub fn data_source(&self) -> Option<&crate::primitives::VersionRef> {
        self.data_source.as_ref()
    }
}

pub(crate) fn require_text(value: &str) -> Result<(), DomainErrorCode> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}
