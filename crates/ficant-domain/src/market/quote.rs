use crate::market::FactSource;
use crate::primitives::{DecimalValue, MarketTime, OwnerRef, Ulid, VersionRef};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quote {
    quote_id: Ulid,
    instrument: VersionRef,
    owner: OwnerRef,
    source: FactSource,
    observed_at: MarketTime,
    received_at: MarketTime,
    bid: Option<DecimalValue>,
    ask: Option<DecimalValue>,
    supersedes_id: Option<Ulid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuoteInput {
    pub quote_id: Ulid,
    pub instrument: VersionRef,
    pub owner: OwnerRef,
    pub source: FactSource,
    pub observed_at: MarketTime,
    pub received_at: MarketTime,
    pub bid: Option<DecimalValue>,
    pub ask: Option<DecimalValue>,
    pub supersedes_id: Option<Ulid>,
}

impl Quote {
    pub fn new(input: QuoteInput) -> DomainResult<Self> {
        let QuoteInput {
            quote_id,
            instrument,
            owner,
            source,
            observed_at,
            received_at,
            bid,
            ask,
            supersedes_id,
        } = input;
        if observed_at.instant() > received_at.instant() || (bid.is_none() && ask.is_none()) {
            return Err(DomainErrorCode::InvalidValue);
        }
        if let (Some(bid), Some(ask)) = (&bid, &ask)
            && bid.compare(ask)? == std::cmp::Ordering::Greater
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            quote_id,
            instrument,
            owner,
            source,
            observed_at,
            received_at,
            bid,
            ask,
            supersedes_id,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.quote_id
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

    pub fn observed_at(&self) -> &MarketTime {
        &self.observed_at
    }

    pub fn received_at(&self) -> &MarketTime {
        &self.received_at
    }

    pub fn bid(&self) -> Option<&DecimalValue> {
        self.bid.as_ref()
    }

    pub fn ask(&self) -> Option<&DecimalValue> {
        self.ask.as_ref()
    }

    pub fn supersedes_id(&self) -> Option<&Ulid> {
        self.supersedes_id.as_ref()
    }
}
