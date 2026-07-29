use ficant_domain::analytics::AnalyticsObjectRef;
use ficant_domain::market::MarketRulePack;
use ficant_domain::primitives::MarketTime;
use ficant_domain::subject::FundingTier;
use ficant_domain::{ContentAddressed, DomainErrorCode, VersionedDefinition};

use crate::ports::{
    AccessScope, ApplicationResult, DefinitionRepository, DefinitionValue, FundingRate,
    FundingRulePackParser,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

/// Resolves the exact persisted funding `RulePack` into the provider-neutral annual rate shape.
pub struct ResolveFundingRule<'a> {
    definitions: &'a dyn DefinitionRepository,
    parser: &'a dyn FundingRulePackParser,
}

impl<'a> ResolveFundingRule<'a> {
    #[must_use]
    pub const fn new(
        definitions: &'a dyn DefinitionRepository,
        parser: &'a dyn FundingRulePackParser,
    ) -> Self {
        Self {
            definitions,
            parser,
        }
    }

    /// Reads and parses the exact funding `RulePack` before a delivery calculation.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed error for missing definitions/content, mismatched bindings, expired
    /// packs, hash drift, wrong typed envelopes, or missing required tier/rate items.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        binding: &AnalyticsObjectRef,
        valuation_at: MarketTime,
        funding_tier: FundingTier,
    ) -> ApplicationResult<FundingRate> {
        let resolved = self
            .definitions
            .get_version(
                scope,
                binding.version_ref().id().clone(),
                binding.version_ref().version(),
            )
            .await?
            .ok_or_else(lineage_incomplete)?;
        let DefinitionValue::MarketRulePack(rule_pack) = resolved else {
            return Err(lineage_incomplete());
        };
        validate_funding_rule_pack(scope, binding, &valuation_at, &rule_pack, self.parser)?;
        let content = rule_pack.content().ok_or_else(|| {
            ApplicationError::rule_pack_item_missing("context.funding_rule_pack.content")
        })?;
        self.parser.parse(content, funding_tier)
    }
}

fn validate_funding_rule_pack(
    scope: &AccessScope,
    binding: &AnalyticsObjectRef,
    valuation_at: &MarketTime,
    rule_pack: &MarketRulePack,
    parser: &dyn FundingRulePackParser,
) -> ApplicationResult<()> {
    if rule_pack.identity() != binding.version_ref().id().as_str()
        || rule_pack.version() != binding.version_ref().version().get()
    {
        return Err(lineage_incomplete());
    }
    scope.authorize(rule_pack.owner())?;
    if rule_pack.content_hash() != binding.content_hash() {
        return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
    }
    if rule_pack.effective().from().instant() > valuation_at.instant()
        || valuation_at.instant() >= rule_pack.effective().to().instant()
    {
        return Err(map_domain_error(DomainErrorCode::InvalidEffectiveTime));
    }
    let content = rule_pack.content().ok_or_else(|| {
        ApplicationError::rule_pack_item_missing("context.funding_rule_pack.content")
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
