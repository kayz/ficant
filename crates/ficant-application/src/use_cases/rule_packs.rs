use chrono::{DateTime, NaiveDate};
use ficant_domain::constraint::ConstraintSet;
use ficant_domain::futures_delivery::{CgbFuturesProduct, FuturesDeliveryRule};
use ficant_domain::market::{BondTaxAttributes, MarketRulePack, VerificationStatus};
use ficant_domain::primitives::{MarketTime, VersionRef};
use ficant_domain::subject::{FundingTier, TaxTreatment};
use ficant_domain::{ContentAddressed, DomainErrorCode, VersionedDefinition};

use crate::ports::{
    ApplicationResult, CouponTaxTreatment, FundingRate, FundingRulePackParser,
    FuturesDeliveryRuleParser, RegulatoryRulePackParser, TaxRulePackParser,
};
use crate::{ApplicationError, map_domain_error};

pub fn parse_verified_delivery_rule(
    rule_pack: &MarketRulePack,
    valuation_at: Option<&MarketTime>,
    product: CgbFuturesProduct,
    parser: &dyn FuturesDeliveryRuleParser,
) -> ApplicationResult<FuturesDeliveryRule> {
    let content = validate_envelope(
        rule_pack,
        valuation_at,
        parser.market(),
        parser.rule_type(),
        parser.type_url(),
        None,
        None,
        "context.rule_pack.content",
    )?;
    parser.parse(content, product)
}

pub fn parse_verified_funding_rule(
    rule_pack: &MarketRulePack,
    valuation_at: Option<&MarketTime>,
    funding_tier: FundingTier,
    parser: &dyn FundingRulePackParser,
) -> ApplicationResult<FundingRate> {
    let content = validate_envelope(
        rule_pack,
        valuation_at,
        parser.market(),
        parser.rule_type(),
        parser.type_url(),
        None,
        None,
        "context.funding_rule_pack.content",
    )?;
    parser.parse(content, funding_tier)
}

pub fn parse_verified_tax_rule(
    rule_pack: &MarketRulePack,
    valuation_at: Option<&MarketTime>,
    first_issue_date: NaiveDate,
    tax_attributes: BondTaxAttributes,
    tax_treatment: &TaxTreatment,
    parser: &dyn TaxRulePackParser,
) -> ApplicationResult<CouponTaxTreatment> {
    let content = validate_envelope(
        rule_pack,
        valuation_at,
        parser.market(),
        parser.rule_type(),
        parser.type_url(),
        parser.expected_source(),
        parser.expected_effective_window(),
        "context.tax_rule_pack.content",
    )?;
    parser.parse(content, first_issue_date, tax_attributes, tax_treatment)
}

pub fn parse_verified_regulatory_rule(
    rule_pack: &MarketRulePack,
    valuation_at: Option<&MarketTime>,
    parser: &dyn RegulatoryRulePackParser,
) -> ApplicationResult<ConstraintSet> {
    let content = validate_envelope(
        rule_pack,
        valuation_at,
        parser.market(),
        parser.rule_type(),
        parser.type_url(),
        None,
        None,
        "context.regulatory_rule_pack.content",
    )?;
    let reference = VersionRef::new(
        ficant_domain::primitives::Ulid::new(rule_pack.identity()).map_err(map_domain_error)?,
        ficant_domain::primitives::Version::new(rule_pack.version()).map_err(map_domain_error)?,
    );
    parser.parse(content, reference)
}

#[allow(clippy::too_many_arguments)]
fn validate_envelope<'a>(
    rule_pack: &'a MarketRulePack,
    valuation_at: Option<&MarketTime>,
    expected_market: &str,
    expected_rule_type: &str,
    expected_type_url: &str,
    expected_source: Option<&str>,
    expected_window: Option<(&str, &str)>,
    missing_path: &str,
) -> ApplicationResult<&'a ficant_domain::market::RulePackContent> {
    if let Some(at) = valuation_at
        && (rule_pack.effective().from().instant() > at.instant()
            || at.instant() >= rule_pack.effective().to().instant())
    {
        return Err(map_domain_error(DomainErrorCode::InvalidEffectiveTime));
    }
    let content = rule_pack
        .content()
        .ok_or_else(|| ApplicationError::rule_pack_item_missing(missing_path))?;
    rule_pack
        .content_hash()
        .verify(content.value())
        .map_err(map_domain_error)?;
    if rule_pack.market() != expected_market
        || rule_pack.rule_type() != expected_rule_type
        || rule_pack.verification_status() != VerificationStatus::Verified
        || content.type_url() != expected_type_url
        || expected_source.is_some_and(|expected| rule_pack.source() != expected)
    {
        return Err(map_domain_error(DomainErrorCode::InvalidValue));
    }
    if let Some((from_str, to_str)) = expected_window {
        let from_ts = DateTime::parse_from_rfc3339(from_str)
            .map_err(|_| map_domain_error(DomainErrorCode::InvalidValue))?;
        let to_ts = DateTime::parse_from_rfc3339(to_str)
            .map_err(|_| map_domain_error(DomainErrorCode::InvalidValue))?;
        if rule_pack.effective().from().instant() != from_ts.to_utc()
            || rule_pack.effective().to().instant() != to_ts.to_utc()
        {
            return Err(map_domain_error(DomainErrorCode::InvalidEffectiveTime));
        }
    }
    Ok(content)
}
