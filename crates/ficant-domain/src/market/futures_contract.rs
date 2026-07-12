use crate::market::{Instrument, InstrumentKind};
use crate::primitives::{DecimalValue, MarketTime, VersionRef};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesContract {
    instrument: VersionRef,
    last_trade_time: MarketTime,
    expiry_time: MarketTime,
    settlement_time: MarketTime,
    multiplier: DecimalValue,
    rule_pack: VersionRef,
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
        })
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
}
