use crate::market::FactSource;
use crate::primitives::{DecimalValue, MarketTime, OwnerRef, Ulid, VersionRef};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trade {
    trade_id: Ulid,
    instrument: VersionRef,
    owner: OwnerRef,
    source: FactSource,
    executed_at: MarketTime,
    price: DecimalValue,
    quantity: DecimalValue,
    supersedes_id: Option<Ulid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeInput {
    pub trade_id: Ulid,
    pub instrument: VersionRef,
    pub owner: OwnerRef,
    pub source: FactSource,
    pub executed_at: MarketTime,
    pub price: DecimalValue,
    pub quantity: DecimalValue,
    pub supersedes_id: Option<Ulid>,
}

impl Trade {
    pub fn new(input: TradeInput) -> DomainResult<Self> {
        let TradeInput {
            trade_id,
            instrument,
            owner,
            source,
            executed_at,
            price,
            quantity,
            supersedes_id,
        } = input;
        if !price.is_positive() || !quantity.is_positive() {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            trade_id,
            instrument,
            owner,
            source,
            executed_at,
            price,
            quantity,
            supersedes_id,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.trade_id
    }

    pub fn instrument(&self) -> &VersionRef {
        &self.instrument
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn source(&self) -> &FactSource {
        &self.source
    }

    pub fn executed_at(&self) -> &MarketTime {
        &self.executed_at
    }

    pub fn price(&self) -> &DecimalValue {
        &self.price
    }

    pub fn quantity(&self) -> &DecimalValue {
        &self.quantity
    }

    pub fn supersedes_id(&self) -> Option<&Ulid> {
        self.supersedes_id.as_ref()
    }
}
