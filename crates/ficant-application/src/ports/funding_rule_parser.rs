use ficant_domain::analytics::FixedDecimal;
use ficant_domain::market::RulePackContent;
use ficant_domain::primitives::UnitRef;
use ficant_domain::subject::FundingTier;

use super::ApplicationResult;

/// Provider-neutral annual financing rate selected from one parsed L3 `FundingRulePack`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FundingRate {
    annual_financing_rate: FixedDecimal,
    unit: UnitRef,
}

impl FundingRate {
    #[must_use]
    pub const fn new(annual_financing_rate: FixedDecimal, unit: UnitRef) -> Self {
        Self {
            annual_financing_rate,
            unit,
        }
    }

    #[must_use]
    pub const fn annual_financing_rate(&self) -> FixedDecimal {
        self.annual_financing_rate
    }

    #[must_use]
    pub fn unit(&self) -> &UnitRef {
        &self.unit
    }
}

/// L3 adapter boundary for one typed funding-rule payload schema.
///
/// The application validates the exact definition binding and this adapter's declared envelope
/// before asking the adapter to select the Subject's funding tier.
pub trait FundingRulePackParser: Send + Sync {
    #[must_use]
    fn market(&self) -> &'static str;

    #[must_use]
    fn rule_type(&self) -> &'static str;

    #[must_use]
    fn type_url(&self) -> &'static str;

    /// Parses one exact Subject funding tier.
    ///
    /// # Errors
    ///
    /// Returns a non-retryable validation failure when a required rule item is missing or invalid.
    fn parse(
        &self,
        content: &RulePackContent,
        funding_tier: FundingTier,
    ) -> ApplicationResult<FundingRate>;
}
