use crate::market::require_text;
use crate::primitives::{OwnerRef, Ulid, UnitRef, Version, VersionRef, ensure_next_version};
use crate::{DomainErrorCode, DomainResult, VersionedDefinition};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstrumentKind {
    Bond,
    Futures,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instrument {
    instrument_id: Ulid,
    version: Version,
    owner: OwnerRef,
    kind: InstrumentKind,
    market: String,
    symbol: String,
    currency: UnitRef,
    calendar: VersionRef,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentInput {
    pub instrument_id: Ulid,
    pub version: Version,
    pub owner: OwnerRef,
    pub kind: InstrumentKind,
    pub market: String,
    pub symbol: String,
    pub currency: UnitRef,
    pub calendar: VersionRef,
}

impl Instrument {
    pub fn new(input: InstrumentInput) -> DomainResult<Self> {
        let InstrumentInput {
            instrument_id,
            version,
            owner,
            kind,
            market,
            symbol,
            currency,
            calendar,
        } = input;
        require_text(&market)?;
        require_text(&symbol)?;
        Ok(Self {
            instrument_id,
            version,
            owner,
            kind,
            market,
            symbol,
            currency,
            calendar,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.instrument_id
    }

    pub fn kind(&self) -> InstrumentKind {
        self.kind
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn market(&self) -> &str {
        &self.market
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    pub fn currency(&self) -> &UnitRef {
        &self.currency
    }

    pub fn calendar(&self) -> &VersionRef {
        &self.calendar
    }

    pub fn version_ref(&self) -> VersionRef {
        VersionRef::new(self.instrument_id.clone(), self.version)
    }

    pub fn validate_successor(&self, candidate: &Self) -> DomainResult<()> {
        ensure_next_version(
            &self.instrument_id,
            self.version,
            &candidate.instrument_id,
            candidate.version,
        )?;
        if self.kind != candidate.kind {
            return Err(DomainErrorCode::VersionConflict);
        }
        Ok(())
    }
}

impl VersionedDefinition for Instrument {
    fn identity(&self) -> &str {
        self.instrument_id.as_str()
    }

    fn version(&self) -> u64 {
        self.version.get()
    }
}
