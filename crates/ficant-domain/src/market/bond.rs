use chrono::NaiveDate;

use crate::market::{Instrument, InstrumentKind};
use crate::primitives::{DecimalValue, VersionRef};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bond {
    instrument: VersionRef,
    issue_date: NaiveDate,
    maturity_date: NaiveDate,
    face_value: DecimalValue,
}

impl Bond {
    pub fn new(
        instrument: &Instrument,
        issue_date: NaiveDate,
        maturity_date: NaiveDate,
        face_value: DecimalValue,
    ) -> DomainResult<Self> {
        if instrument.kind() != InstrumentKind::Bond || !face_value.is_positive() {
            return Err(DomainErrorCode::InvalidValue);
        }
        if issue_date >= maturity_date {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self {
            instrument: instrument.version_ref(),
            issue_date,
            maturity_date,
            face_value,
        })
    }

    pub fn instrument(&self) -> &VersionRef {
        &self.instrument
    }

    pub fn issue_date(&self) -> NaiveDate {
        self.issue_date
    }

    pub fn maturity_date(&self) -> NaiveDate {
        self.maturity_date
    }

    pub fn face_value(&self) -> &DecimalValue {
        &self.face_value
    }
}
