use ficant_domain::futures_delivery::{CgbFuturesProduct, FuturesDeliveryRule};
use ficant_domain::market::RulePackContent;

use super::ApplicationResult;

/// L3 adapter boundary for one typed futures-delivery `RulePack` schema.
///
/// The application validates the exact definition binding and this adapter's declared envelope
/// before asking the adapter to parse the opaque payload.
pub trait FuturesDeliveryRuleParser: Send + Sync {
    #[must_use]
    fn market(&self) -> &'static str;

    #[must_use]
    fn rule_type(&self) -> &'static str;

    #[must_use]
    fn type_url(&self) -> &'static str;

    /// Parses the selected product's complete rule shape.
    ///
    /// # Errors
    ///
    /// Returns a non-retryable validation failure when a required rule item is missing or invalid.
    fn parse(
        &self,
        content: &RulePackContent,
        product: CgbFuturesProduct,
    ) -> ApplicationResult<FuturesDeliveryRule>;
}
