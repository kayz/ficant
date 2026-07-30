use chrono::NaiveDate;
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::market::{BondTaxAttributes, RulePackContent};
use ficant_domain::primitives::UnitRef;
use ficant_domain::subject::TaxTreatment;

use super::ApplicationResult;

/// Provider-neutral coupon-tax rate selected from one parsed L3 `TaxRulePack`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CouponTaxRate {
    coupon_tax_rate: FixedDecimal,
    unit: UnitRef,
}

impl CouponTaxRate {
    #[must_use]
    pub const fn new(coupon_tax_rate: FixedDecimal, unit: UnitRef) -> Self {
        Self {
            coupon_tax_rate,
            unit,
        }
    }

    #[must_use]
    pub const fn coupon_tax_rate(&self) -> FixedDecimal {
        self.coupon_tax_rate
    }

    #[must_use]
    pub fn unit(&self) -> &UnitRef {
        &self.unit
    }
}

/// L3 adapter boundary for one typed coupon-tax rule payload schema.
///
/// The application validates the exact definition binding and this adapter's declared envelope
/// before asking the adapter to select the Bond interval, attributes, and Subject profile pair.
pub trait TaxRulePackParser: Send + Sync {
    #[must_use]
    fn market(&self) -> &'static str;

    #[must_use]
    fn rule_type(&self) -> &'static str;

    #[must_use]
    fn type_url(&self) -> &'static str;

    /// Parses one exact Bond and Subject tax treatment.
    ///
    /// # Errors
    ///
    /// Returns a non-retryable validation failure when a required interval, attribute, profile,
    /// or rate item is missing or invalid.
    fn parse(
        &self,
        content: &RulePackContent,
        first_issue_date: NaiveDate,
        tax_attributes: BondTaxAttributes,
        tax_treatment: &TaxTreatment,
    ) -> ApplicationResult<CouponTaxRate>;
}
