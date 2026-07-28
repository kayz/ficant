use crate::market::require_text;
use crate::primitives::{ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version};
use crate::{ContentAddressed, DomainResult, VersionedDefinition};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationStatus {
    Unverified,
    Verified,
    Rejected,
}

/// Opaque, content-addressed `RulePack` payload.
///
/// The core owns only the transport-neutral envelope. A market adapter owns the schema named by
/// `type_url` and must parse its bytes before any calculation can use the pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RulePackContent {
    type_url: String,
    value: Vec<u8>,
}

impl RulePackContent {
    /// Creates one non-empty typed `RulePack` payload.
    ///
    /// # Errors
    ///
    /// Returns validation failure for blank type URLs or empty content bytes.
    pub fn new(type_url: impl Into<String>, value: Vec<u8>) -> DomainResult<Self> {
        let type_url = type_url.into();
        require_text(&type_url)?;
        if value.is_empty() {
            return Err(crate::DomainErrorCode::InvalidValue);
        }
        Ok(Self { type_url, value })
    }

    #[must_use]
    pub fn type_url(&self) -> &str {
        &self.type_url
    }

    #[must_use]
    pub fn value(&self) -> &[u8] {
        &self.value
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketRulePack {
    rule_pack_id: Ulid,
    version: Version,
    owner: OwnerRef,
    market: String,
    rule_type: String,
    source: String,
    effective: EffectivePeriod,
    verification_status: VerificationStatus,
    content_hash: ContentHash,
    content: Option<RulePackContent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketRulePackInput {
    pub rule_pack_id: Ulid,
    pub version: Version,
    pub owner: OwnerRef,
    pub market: String,
    pub rule_type: String,
    pub source: String,
    pub effective: EffectivePeriod,
    pub verification_status: VerificationStatus,
    pub content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketRulePackTimesInput {
    pub rule_pack_id: Ulid,
    pub version: Version,
    pub owner: OwnerRef,
    pub market: String,
    pub rule_type: String,
    pub source: String,
    pub from: MarketTime,
    pub to: MarketTime,
    pub verification_status: VerificationStatus,
    pub content_hash: ContentHash,
}

impl MarketRulePack {
    pub fn new(input: MarketRulePackInput) -> DomainResult<Self> {
        Self::new_inner(input, None)
    }

    /// Creates a `RulePack` with an inline typed payload whose bytes are bound to `content_hash`.
    ///
    /// Existing payload-less `RulePacks` remain readable through [`Self::new`], but callers that
    /// require typed rules must reject such historical packs before calculation.
    pub fn new_with_content(
        input: MarketRulePackInput,
        content: RulePackContent,
    ) -> DomainResult<Self> {
        Self::new_inner(input, Some(content))
    }

    fn new_inner(
        input: MarketRulePackInput,
        content: Option<RulePackContent>,
    ) -> DomainResult<Self> {
        let MarketRulePackInput {
            rule_pack_id,
            version,
            owner,
            market,
            rule_type,
            source,
            effective,
            verification_status,
            content_hash,
        } = input;
        require_text(&market)?;
        require_text(&rule_type)?;
        require_text(&source)?;
        if let Some(content) = &content {
            content_hash.verify(content.value())?;
        }
        Ok(Self {
            rule_pack_id,
            version,
            owner,
            market,
            rule_type,
            source,
            effective,
            verification_status,
            content_hash,
            content,
        })
    }

    pub fn new_with_times(input: MarketRulePackTimesInput) -> DomainResult<Self> {
        Self::new(MarketRulePackInput {
            rule_pack_id: input.rule_pack_id,
            version: input.version,
            owner: input.owner,
            market: input.market,
            rule_type: input.rule_type,
            source: input.source,
            effective: EffectivePeriod::new(input.from, input.to)?,
            verification_status: input.verification_status,
            content_hash: input.content_hash,
        })
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn market(&self) -> &str {
        &self.market
    }

    pub fn rule_type(&self) -> &str {
        &self.rule_type
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn effective(&self) -> &EffectivePeriod {
        &self.effective
    }

    pub fn verification_status(&self) -> VerificationStatus {
        self.verification_status
    }

    #[must_use]
    pub fn content(&self) -> Option<&RulePackContent> {
        self.content.as_ref()
    }
}

impl VersionedDefinition for MarketRulePack {
    fn identity(&self) -> &str {
        self.rule_pack_id.as_str()
    }

    fn version(&self) -> u64 {
        self.version.get()
    }
}

impl ContentAddressed for MarketRulePack {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}
