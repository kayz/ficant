use ficant_domain::constraint::ConstraintSet;
use ficant_domain::market::RulePackContent;
use ficant_domain::primitives::VersionRef;

use super::ApplicationResult;

/// L3 adapter boundary for one typed regulatory-rule payload schema (ADR-0026).
///
/// The application validates the exact Subject binding and this adapter's declared envelope
/// before asking it to parse the constraint definitions.
pub trait RegulatoryRulePackParser: Send + Sync {
    #[must_use]
    fn market(&self) -> &'static str;

    #[must_use]
    fn rule_type(&self) -> &'static str;

    #[must_use]
    fn type_url(&self) -> &'static str;

    /// Parses the constraint set of the exact `rule_pack` version.
    ///
    /// # Errors
    ///
    /// Returns a non-retryable validation failure when the payload is empty, a limit code is
    /// blank or repeated, or a measure is unspecified or unknown.
    fn parse(
        &self,
        content: &RulePackContent,
        rule_pack: VersionRef,
    ) -> ApplicationResult<ConstraintSet>;
}
