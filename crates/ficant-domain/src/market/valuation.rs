use crate::market::{FactSource, require_text};
use crate::primitives::{DecimalValue, MarketTime, OwnerRef, Ulid, VersionRef};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Valuation {
    valuation_id: Ulid,
    instrument: VersionRef,
    owner: OwnerRef,
    source: FactSource,
    valuation_at: MarketTime,
    method: String,
    rule_pack: VersionRef,
    values: Vec<DecimalValue>,
    supersedes_id: Option<Ulid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValuationInput {
    pub valuation_id: Ulid,
    pub instrument: VersionRef,
    pub owner: OwnerRef,
    pub source: FactSource,
    pub valuation_at: MarketTime,
    pub method: String,
    pub rule_pack: VersionRef,
    pub values: Vec<DecimalValue>,
    pub supersedes_id: Option<Ulid>,
}

impl Valuation {
    pub fn new(input: ValuationInput) -> DomainResult<Self> {
        let ValuationInput {
            valuation_id,
            instrument,
            owner,
            source,
            valuation_at,
            method,
            rule_pack,
            values,
            supersedes_id,
        } = input;
        require_text(&method)?;
        if values.is_empty() {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            valuation_id,
            instrument,
            owner,
            source,
            valuation_at,
            method,
            rule_pack,
            values,
            supersedes_id,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.valuation_id
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

    pub fn valuation_at(&self) -> &MarketTime {
        &self.valuation_at
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn rule_pack(&self) -> &VersionRef {
        &self.rule_pack
    }

    pub fn values(&self) -> &[DecimalValue] {
        &self.values
    }

    pub fn supersedes_id(&self) -> Option<&Ulid> {
        self.supersedes_id.as_ref()
    }
}
