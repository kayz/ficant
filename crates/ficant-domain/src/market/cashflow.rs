use crate::market::{FactSource, require_text};
use crate::primitives::{DecimalValue, MarketTime, OwnerRef, Ulid, VersionRef};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CashflowType {
    Coupon,
    Principal,
    Fee,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cashflow {
    cashflow_id: Ulid,
    bond: VersionRef,
    payment_time: MarketTime,
    amount: DecimalValue,
    owner: OwnerRef,
    source: FactSource,
    supersedes_id: Option<Ulid>,
    cashflow_type: CashflowType,
    schedule_id: String,
    sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CashflowInput {
    pub cashflow_id: Ulid,
    pub bond: VersionRef,
    pub payment_time: MarketTime,
    pub amount: DecimalValue,
    pub owner: OwnerRef,
    pub source: FactSource,
    pub supersedes_id: Option<Ulid>,
    pub cashflow_type: CashflowType,
    pub schedule_id: String,
    pub sequence: u64,
}

impl Cashflow {
    pub fn new(input: CashflowInput) -> DomainResult<Self> {
        let CashflowInput {
            cashflow_id,
            bond,
            payment_time,
            amount,
            owner,
            source,
            supersedes_id,
            cashflow_type,
            schedule_id,
            sequence,
        } = input;
        require_text(&schedule_id)?;
        if sequence == 0 {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            cashflow_id,
            bond,
            payment_time,
            amount,
            owner,
            source,
            supersedes_id,
            cashflow_type,
            schedule_id,
            sequence,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.cashflow_id
    }

    pub fn bond(&self) -> &VersionRef {
        &self.bond
    }

    pub fn payment_time(&self) -> &MarketTime {
        &self.payment_time
    }

    pub fn amount(&self) -> &DecimalValue {
        &self.amount
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn source(&self) -> &FactSource {
        &self.source
    }

    pub fn supersedes_id(&self) -> Option<&Ulid> {
        self.supersedes_id.as_ref()
    }

    pub fn cashflow_type(&self) -> CashflowType {
        self.cashflow_type
    }

    pub fn schedule_id(&self) -> &str {
        &self.schedule_id
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}
