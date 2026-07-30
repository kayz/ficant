use chrono::NaiveDate;

use crate::market::{Instrument, InstrumentKind};
use crate::primitives::{DecimalValue, VersionRef};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueAddedTaxStatus {
    Exempt,
    Taxable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncomeTaxStatus {
    Exempt,
    Taxable,
}

/// L2 Bond facts consumed and validated by an exact L3 `TaxRulePack`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BondTaxAttributes {
    value_added_tax_status: ValueAddedTaxStatus,
    income_tax_status: IncomeTaxStatus,
}

impl BondTaxAttributes {
    #[must_use]
    pub const fn new(
        value_added_tax_status: ValueAddedTaxStatus,
        income_tax_status: IncomeTaxStatus,
    ) -> Self {
        Self {
            value_added_tax_status,
            income_tax_status,
        }
    }

    #[must_use]
    pub const fn value_added_tax_status(&self) -> ValueAddedTaxStatus {
        self.value_added_tax_status
    }

    #[must_use]
    pub const fn income_tax_status(&self) -> IncomeTaxStatus {
        self.income_tax_status
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bond {
    instrument: VersionRef,
    first_issue_date: NaiveDate,
    current_issue_date: NaiveDate,
    maturity_date: NaiveDate,
    cumulative_issued_amount: DecimalValue,
    tax_attributes: Option<BondTaxAttributes>,
    face_value: DecimalValue,
}

impl Bond {
    /// Legacy construction used only by frozen direct-test adapters.
    ///
    /// The public contract must use [`Self::with_issuance`]. A legacy Bond has no tax attributes
    /// and therefore cannot enter a `TaxRulePack` calculation.
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
            first_issue_date: issue_date,
            current_issue_date: issue_date,
            maturity_date,
            cumulative_issued_amount: face_value.clone(),
            tax_attributes: None,
            face_value,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_issuance(
        instrument: &Instrument,
        first_issue_date: NaiveDate,
        current_issue_date: NaiveDate,
        maturity_date: NaiveDate,
        cumulative_issued_amount: DecimalValue,
        tax_attributes: BondTaxAttributes,
        face_value: DecimalValue,
    ) -> DomainResult<Self> {
        if instrument.kind() != InstrumentKind::Bond
            || !face_value.is_positive()
            || !cumulative_issued_amount.is_positive()
            || cumulative_issued_amount.unit() != face_value.unit()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        if first_issue_date >= maturity_date
            || current_issue_date < first_issue_date
            || current_issue_date >= maturity_date
        {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self {
            instrument: instrument.version_ref(),
            first_issue_date,
            current_issue_date,
            maturity_date,
            cumulative_issued_amount,
            tax_attributes: Some(tax_attributes),
            face_value,
        })
    }

    pub fn instrument(&self) -> &VersionRef {
        &self.instrument
    }

    pub fn first_issue_date(&self) -> NaiveDate {
        self.first_issue_date
    }

    pub fn current_issue_date(&self) -> NaiveDate {
        self.current_issue_date
    }

    pub fn maturity_date(&self) -> NaiveDate {
        self.maturity_date
    }

    pub fn face_value(&self) -> &DecimalValue {
        &self.face_value
    }

    pub fn cumulative_issued_amount(&self) -> &DecimalValue {
        &self.cumulative_issued_amount
    }

    pub fn tax_attributes(&self) -> Option<BondTaxAttributes> {
        self.tax_attributes
    }
}
