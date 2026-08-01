use ficant_domain::futures_delivery::{CgbFuturesProduct, FuturesDeliveryRule};
use ficant_domain::market::RulePackContent;

use super::{ApplicationError, ApplicationResult};

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

    /// Resolves an exact provider product code without leaking market branches into application.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the provider code is unknown or non-canonical.
    fn parse_product_code(&self, _product_code: &str) -> ApplicationResult<CgbFuturesProduct> {
        Err(ApplicationError::new(
            crate::ApplicationErrorCategory::ValidationFailed,
            false,
        ))
    }

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

    /// Parses the delivery rule and requires the product-specific contract size used by risk.
    ///
    /// Existing delivery callers may continue to consume v1 content through [`Self::parse`].
    ///
    /// # Errors
    ///
    /// Returns the parser error or a named missing-item error when contract size is absent.
    fn parse_for_portfolio_risk(
        &self,
        content: &RulePackContent,
        product: CgbFuturesProduct,
    ) -> ApplicationResult<FuturesDeliveryRule> {
        let rule = self.parse(content, product)?;
        if rule.contract_size_in_quote_units().is_none() {
            return Err(ApplicationError::rule_pack_item_missing(format!(
                "context.rule_pack.content.products[product_code={}].contract_size_in_quote_units",
                product.code()
            )));
        }
        Ok(rule)
    }
}
