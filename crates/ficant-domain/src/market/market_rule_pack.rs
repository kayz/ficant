use crate::market::require_text;
use crate::primitives::{ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version};
use crate::{ContentAddressed, DomainResult, VersionedDefinition};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationStatus {
    Unverified,
    Verified,
    Rejected,
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
