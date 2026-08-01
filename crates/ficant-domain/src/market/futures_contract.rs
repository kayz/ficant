use crate::market::{Instrument, InstrumentKind};
use crate::primitives::{DecimalValue, MarketTime, UnitRef, VersionRef};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesContract {
    instrument: VersionRef,
    last_trade_time: MarketTime,
    expiry_time: MarketTime,
    settlement_time: MarketTime,
    multiplier: DecimalValue,
    rule_pack: VersionRef,
    product_code: Option<String>,
    price_unit: Option<UnitRef>,
}

impl FuturesContract {
    pub fn new(
        instrument: &Instrument,
        last_trade_time: MarketTime,
        expiry_time: MarketTime,
        settlement_time: MarketTime,
        multiplier: DecimalValue,
        rule_pack: VersionRef,
    ) -> DomainResult<Self> {
        if instrument.kind() != InstrumentKind::Futures || !multiplier.is_positive() {
            return Err(DomainErrorCode::InvalidValue);
        }
        if last_trade_time.instant() >= expiry_time.instant()
            || expiry_time.instant() > settlement_time.instant()
        {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self {
            instrument: instrument.version_ref(),
            last_trade_time,
            expiry_time,
            settlement_time,
            multiplier,
            rule_pack,
            product_code: None,
            price_unit: None,
        })
    }

    /// Adds the exact product selector and quote Unit required by portfolio risk.
    ///
    /// Legacy definitions remain readable through [`Self::new`], but are not risk-ready until
    /// these terms are present.
    pub fn with_risk_terms(
        mut self,
        product_code: impl Into<String>,
        price_unit: UnitRef,
    ) -> DomainResult<Self> {
        let product_code = product_code.into();
        if product_code.trim().is_empty()
            || product_code != product_code.trim()
            || product_code.chars().any(char::is_whitespace)
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        self.product_code = Some(product_code);
        self.price_unit = Some(price_unit);
        Ok(self)
    }

    pub fn instrument(&self) -> &VersionRef {
        &self.instrument
    }

    pub fn last_trade_time(&self) -> &MarketTime {
        &self.last_trade_time
    }

    pub fn expiry_time(&self) -> &MarketTime {
        &self.expiry_time
    }

    pub fn settlement_time(&self) -> &MarketTime {
        &self.settlement_time
    }

    pub fn multiplier(&self) -> &DecimalValue {
        &self.multiplier
    }

    pub fn rule_pack(&self) -> &VersionRef {
        &self.rule_pack
    }

    pub fn product_code(&self) -> Option<&str> {
        self.product_code.as_deref()
    }

    pub fn price_unit(&self) -> Option<&UnitRef> {
        self.price_unit.as_ref()
    }
}
