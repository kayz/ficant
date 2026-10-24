//! Resolves a Subject's constraint set from its exact regulatory `RulePack` (ADR-0026).

use ficant_domain::constraint::ConstraintSet;
use ficant_domain::market::MarketRulePack;
use ficant_domain::primitives::{MarketTime, VersionRef};
use ficant_domain::subject::SubjectVersion;
use ficant_domain::{ContentAddressed, DomainErrorCode, VersionedDefinition};

use crate::ports::{
    AccessScope, ApplicationResult, DefinitionRepository, DefinitionValue, RegulatoryRulePackParser,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

/// Reads and parses the regulatory `RulePack` a Subject version binds.
pub struct ResolveConstraintSet<'a> {
    definitions: &'a dyn DefinitionRepository,
    parser: &'a dyn RegulatoryRulePackParser,
}

impl<'a> ResolveConstraintSet<'a> {
    #[must_use]
    pub const fn new(
        definitions: &'a dyn DefinitionRepository,
        parser: &'a dyn RegulatoryRulePackParser,
    ) -> Self {
        Self {
            definitions,
            parser,
        }
    }

    /// Resolves the constraint set of `subject` effective at `valuation_at`.
    ///
    /// # Errors
    ///
    /// Fails closed when the Subject binds no constraint set (`subject.constraint_set_ref`), the
    /// bound definition is missing or not a `RulePack`, the pack is not visible to `scope`, not
    /// effective at `valuation_at`, its content hash drifts, its envelope is not this parser's,
    /// or the payload is invalid.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        subject: &SubjectVersion,
        valuation_at: MarketTime,
    ) -> ApplicationResult<ConstraintSet> {
        let binding = subject
            .constraint_set_ref()
            .ok_or_else(|| ApplicationError::rule_pack_item_missing("subject.constraint_set_ref"))?
            .reference()
            .clone();
        let resolved = self
            .definitions
            .get_version(scope, binding.id().clone(), binding.version())
            .await?
            .ok_or_else(lineage_incomplete)?;
        let DefinitionValue::MarketRulePack(rule_pack) = resolved else {
            return Err(lineage_incomplete());
        };
        validate_regulatory_rule_pack(scope, &binding, &valuation_at, &rule_pack, self.parser)?;
        let content = rule_pack.content().ok_or_else(|| {
            ApplicationError::rule_pack_item_missing("context.regulatory_rule_pack.content")
        })?;
        self.parser.parse(content, binding)
    }
}

fn validate_regulatory_rule_pack(
    scope: &AccessScope,
    binding: &VersionRef,
    valuation_at: &MarketTime,
    rule_pack: &MarketRulePack,
    parser: &dyn RegulatoryRulePackParser,
) -> ApplicationResult<()> {
    if rule_pack.identity() != binding.id().as_str()
        || rule_pack.version() != binding.version().get()
    {
        return Err(lineage_incomplete());
    }
    scope.authorize(rule_pack.owner())?;
    if rule_pack.effective().from().instant() > valuation_at.instant()
        || valuation_at.instant() >= rule_pack.effective().to().instant()
    {
        return Err(map_domain_error(DomainErrorCode::InvalidEffectiveTime));
    }
    let content = rule_pack.content().ok_or_else(|| {
        ApplicationError::rule_pack_item_missing("context.regulatory_rule_pack.content")
    })?;
    rule_pack
        .content_hash()
        .verify(content.value())
        .map_err(map_domain_error)?;
    if rule_pack.market() != parser.market()
        || rule_pack.rule_type() != parser.rule_type()
        || content.type_url() != parser.type_url()
    {
        return Err(map_domain_error(DomainErrorCode::InvalidValue));
    }
    Ok(())
}

fn lineage_incomplete() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}
