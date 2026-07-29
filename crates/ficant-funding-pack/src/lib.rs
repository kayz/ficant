//! L3 parser for the typed `funding` `RulePack` payload.

use ficant_application::ports::{ApplicationResult, FundingRate, FundingRulePackParser};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1::{DecimalValue, FundingTier as ProtoFundingTier};
use ficant_contracts::ficant::market::v1::{FundingRulePack, FundingTierRate};
use ficant_domain::analytics::{DECIMAL_SCALE, FixedDecimal};
use ficant_domain::market::RulePackContent;
use ficant_domain::primitives::{DecimalValue as DomainDecimalValue, Ulid, UnitRef, Version};
use ficant_domain::subject::FundingTier;
use prost::Message;

pub const MARKET: &str = "CN";
pub const RULE_TYPE: &str = "funding";
pub const TYPE_URL: &str = "type.googleapis.com/ficant.market.v1.FundingRulePack";

/// Parses only the exact v1 funding-rule content schema.
#[derive(Clone, Copy, Debug, Default)]
pub struct FundingRulePackV1Parser;

impl FundingRulePackParser for FundingRulePackV1Parser {
    fn market(&self) -> &'static str {
        MARKET
    }

    fn rule_type(&self) -> &'static str {
        RULE_TYPE
    }

    fn type_url(&self) -> &'static str {
        TYPE_URL
    }

    fn parse(
        &self,
        content: &RulePackContent,
        funding_tier: FundingTier,
    ) -> ApplicationResult<FundingRate> {
        if content.type_url() != TYPE_URL {
            return Err(invalid());
        }
        let payload = FundingRulePack::decode(content.value()).map_err(|_| invalid())?;
        parse_payload(&payload, funding_tier)
    }
}

fn parse_payload(
    payload: &FundingRulePack,
    funding_tier: FundingTier,
) -> ApplicationResult<FundingRate> {
    validate_tier_order(&payload.rates)?;
    let proto_tier = proto_tier(funding_tier);
    let label = tier_label(funding_tier);
    let selected = payload
        .rates
        .iter()
        .find(|value| value.funding_tier == proto_tier as i32)
        .ok_or_else(|| missing(&format!("rates[funding_tier={label}]")))?;
    let (annual_financing_rate, unit) = decimal(
        selected.annual_financing_rate.as_ref(),
        &format!("rates[funding_tier={label}].annual_financing_rate"),
    )?;
    Ok(FundingRate::new(annual_financing_rate, unit))
}

fn validate_tier_order(values: &[FundingTierRate]) -> ApplicationResult<()> {
    let mut previous: Option<i32> = None;
    for (index, value) in values.iter().enumerate() {
        let tier = ProtoFundingTier::try_from(value.funding_tier).map_err(|_| invalid())?;
        if tier == ProtoFundingTier::Unspecified
            || previous.is_some_and(|last| last >= value.funding_tier)
        {
            return Err(invalid());
        }
        previous = Some(value.funding_tier);
        if tier != ProtoFundingTier::DrAvailable && tier != ProtoFundingTier::ROnly {
            return Err(missing(&format!("rates[{index}].funding_tier")));
        }
    }
    Ok(())
}

fn proto_tier(value: FundingTier) -> ProtoFundingTier {
    match value {
        FundingTier::DrAvailable => ProtoFundingTier::DrAvailable,
        FundingTier::ROnly => ProtoFundingTier::ROnly,
    }
}

fn tier_label(value: FundingTier) -> &'static str {
    match value {
        FundingTier::DrAvailable => "DR_AVAILABLE",
        FundingTier::ROnly => "R_ONLY",
    }
}

fn decimal(value: Option<&DecimalValue>, path: &str) -> ApplicationResult<(FixedDecimal, UnitRef)> {
    let value = value.ok_or_else(|| missing(path))?;
    let unit = value
        .unit
        .as_ref()
        .ok_or_else(|| missing(&format!("{path}.unit")))?;
    let unit_id = unit
        .unit_id
        .as_ref()
        .ok_or_else(|| missing(&format!("{path}.unit.unit_id")))?;
    let unit = UnitRef::new(
        Ulid::new(unit_id.value.clone()).map_err(map_domain_error)?,
        Version::new(unit.version).map_err(map_domain_error)?,
    );
    let canonical = DomainDecimalValue::new(value.coefficient.clone(), value.scale, unit.clone())
        .map_err(map_domain_error)?;
    if canonical.coefficient() != value.coefficient || canonical.scale() != value.scale {
        return Err(invalid());
    }
    let coefficient = value.coefficient.parse::<i128>().map_err(|_| invalid())?;
    let scaled = if value.scale <= DECIMAL_SCALE {
        coefficient
            .checked_mul(power_of_ten(DECIMAL_SCALE - value.scale)?)
            .ok_or_else(invalid)?
    } else {
        let divisor = power_of_ten(value.scale - DECIMAL_SCALE)?;
        if coefficient % divisor != 0 {
            return Err(invalid());
        }
        coefficient / divisor
    };
    Ok((FixedDecimal::from_scaled(scaled), unit))
}

fn power_of_ten(exponent: u32) -> ApplicationResult<i128> {
    10_i128.checked_pow(exponent).ok_or_else(invalid)
}

fn missing(path: &str) -> ApplicationError {
    ApplicationError::rule_pack_item_missing(format!("context.funding_rule_pack.content.{path}"))
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ficant_application::ApplicationErrorDetail;
    use ficant_contracts::ficant::core::v1::{Ulid as ProtoUlid, UnitRef as ProtoUnitRef};

    #[test]
    fn selects_canonical_sorted_tier_and_rejects_missing_rate() {
        let parser = FundingRulePackV1Parser;
        let complete = FundingRulePack {
            rates: vec![
                rate(ProtoFundingTier::DrAvailable, Some(decimal("18", 3))),
                rate(ProtoFundingTier::ROnly, Some(decimal("25", 3))),
            ],
        };
        let selected = parser
            .parse(&content(&complete), FundingTier::ROnly)
            .expect("complete R-only rate resolves");
        assert_eq!(
            selected.annual_financing_rate(),
            FixedDecimal::from_scaled(25_000_000_000)
        );

        let missing = FundingRulePack {
            rates: vec![rate(ProtoFundingTier::DrAvailable, Some(decimal("18", 3)))],
        };
        let error = parser
            .parse(&content(&missing), FundingTier::ROnly)
            .expect_err("missing requested tier fails closed");
        assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
        assert_eq!(
            error.detail(),
            Some(&ApplicationErrorDetail::RulePackItemMissing {
                path: "context.funding_rule_pack.content.rates[funding_tier=R_ONLY]".to_owned(),
            })
        );
    }

    fn rate(
        tier: ProtoFundingTier,
        annual_financing_rate: Option<DecimalValue>,
    ) -> FundingTierRate {
        FundingTierRate {
            funding_tier: tier as i32,
            annual_financing_rate,
        }
    }

    fn decimal(coefficient: &str, scale: u32) -> DecimalValue {
        DecimalValue {
            coefficient: coefficient.to_owned(),
            scale,
            unit: Some(ProtoUnitRef {
                unit_id: Some(ProtoUlid {
                    value: "01ARZ3NDEKTSV4RRFFQ69G5FAP".to_owned(),
                }),
                version: 1,
            }),
        }
    }

    fn content(value: &FundingRulePack) -> RulePackContent {
        RulePackContent::new(TYPE_URL, value.encode_to_vec()).unwrap()
    }
}
