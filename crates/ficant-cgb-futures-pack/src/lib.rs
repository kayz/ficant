//! L3 parser for the typed `cgb-futures` `RulePack` payload.

use ficant_application::ports::{ApplicationResult, FuturesDeliveryRuleParser};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1::DecimalValue;
use ficant_contracts::ficant::market::v1::{
    CgbFuturesDeliveryRulePack, CgbFuturesProductRule, cgb_futures_product_rule::ResidualUpperBound,
};
use ficant_domain::analytics::{DECIMAL_SCALE, FixedDecimal};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliveryRule, FuturesDeliveryRuleInput,
};
use ficant_domain::market::RulePackContent;
use ficant_domain::primitives::{DecimalValue as DomainDecimalValue, Ulid, UnitRef, Version};
use prost::Message;

pub const MARKET: &str = "CFFEX";
pub const RULE_TYPE: &str = "cgb-futures";
pub const TYPE_URL: &str = "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack";

/// Parses only the exact v1 CGB futures-delivery content schema.
#[derive(Clone, Copy, Debug, Default)]
pub struct CgbFuturesDeliveryRulePackParser;

impl FuturesDeliveryRuleParser for CgbFuturesDeliveryRulePackParser {
    fn market(&self) -> &'static str {
        MARKET
    }

    fn rule_type(&self) -> &'static str {
        RULE_TYPE
    }

    fn type_url(&self) -> &'static str {
        TYPE_URL
    }

    fn parse_product_code(&self, product_code: &str) -> ApplicationResult<CgbFuturesProduct> {
        match product_code {
            "TS" => Ok(CgbFuturesProduct::TwoYear),
            "TF" => Ok(CgbFuturesProduct::FiveYear),
            "T" => Ok(CgbFuturesProduct::TenYear),
            "TL" => Ok(CgbFuturesProduct::ThirtyYear),
            _ => Err(invalid()),
        }
    }

    fn parse(
        &self,
        content: &RulePackContent,
        product: CgbFuturesProduct,
    ) -> ApplicationResult<FuturesDeliveryRule> {
        if content.type_url() != TYPE_URL {
            return Err(invalid());
        }
        let payload = CgbFuturesDeliveryRulePack::decode(content.value()).map_err(|_| invalid())?;
        parse_payload(&payload, product)
    }
}

fn parse_payload(
    payload: &CgbFuturesDeliveryRulePack,
    product: CgbFuturesProduct,
) -> ApplicationResult<FuturesDeliveryRule> {
    validate_product_order(&payload.products)?;
    validate_delivery_months(&payload.delivery_months)?;
    let product_code = product.code();
    let selected = payload
        .products
        .iter()
        .find(|value| value.product_code.as_deref() == Some(product_code))
        .ok_or_else(|| missing(&format!("products[product_code={product_code}]")))?;
    let prefix = format!("products[product_code={product_code}]");
    let residual_max_months = match selected.residual_upper_bound.as_ref() {
        Some(ResidualUpperBound::ResidualMaxMonths(value)) => Some(*value),
        Some(ResidualUpperBound::ResidualMaxMonthsUnbounded(true)) => None,
        Some(ResidualUpperBound::ResidualMaxMonthsUnbounded(false)) => return Err(invalid()),
        None => return Err(missing(&format!("{prefix}.residual_max_months"))),
    };

    let rule = FuturesDeliveryRule::new(FuturesDeliveryRuleInput {
        original_term_max_months: required_u32(
            selected.original_term_max_months,
            &format!("{prefix}.original_term_max_months"),
        )?,
        residual_min_months: required_u32(
            selected.residual_min_months,
            &format!("{prefix}.residual_min_months"),
        )?,
        residual_max_months,
        delivery_months: payload.delivery_months.clone(),
        nominal_coupon: decimal(payload.nominal_coupon.as_ref(), "nominal_coupon")?,
        face_quote_basis: decimal(payload.face_quote_basis.as_ref(), "face_quote_basis")?,
        accrued_interest_day_count: required_u32(
            payload.accrued_interest_day_count,
            "accrued_interest_day_count",
        )?,
        conversion_factor_rounding_places: required_u32(
            payload.conversion_factor_rounding_places,
            "conversion_factor_rounding_places",
        )?,
        accrued_interest_rounding_places: required_u32(
            payload.accrued_interest_rounding_places,
            "accrued_interest_rounding_places",
        )?,
        annual_day_basis: required_u32(payload.annual_day_basis, "annual_day_basis")?,
    })
    .map_err(map_domain_error)?;
    match selected.contract_size_in_quote_units {
        Some(value) => rule
            .with_contract_size_in_quote_units(value)
            .map_err(map_domain_error),
        None => Ok(rule),
    }
}

fn validate_product_order(values: &[CgbFuturesProductRule]) -> ApplicationResult<()> {
    let mut previous: Option<&str> = None;
    for (index, value) in values.iter().enumerate() {
        let Some(code) = value.product_code.as_deref() else {
            return Err(missing(&format!("products[{index}].product_code")));
        };
        if code.trim().is_empty()
            || code != code.trim()
            || previous.is_some_and(|last| last >= code)
        {
            return Err(invalid());
        }
        previous = Some(code);
    }
    Ok(())
}

fn validate_delivery_months(values: &[u32]) -> ApplicationResult<()> {
    if values.is_empty()
        || values.iter().any(|value| !(1..=12).contains(value))
        || values.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(invalid());
    }
    Ok(())
}

fn required_u32(value: Option<u32>, path: &str) -> ApplicationResult<u32> {
    value.ok_or_else(|| missing(path))
}

fn decimal(value: Option<&DecimalValue>, path: &str) -> ApplicationResult<FixedDecimal> {
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
    let canonical = DomainDecimalValue::new(value.coefficient.clone(), value.scale, unit)
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
    Ok(FixedDecimal::from_scaled(scaled))
}

fn power_of_ten(exponent: u32) -> ApplicationResult<i128> {
    10_i128.checked_pow(exponent).ok_or_else(invalid)
}

fn missing(path: &str) -> ApplicationError {
    ApplicationError::rule_pack_item_missing(format!("context.rule_pack.content.{path}"))
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
    fn parses_complete_sorted_ten_year_rule_and_rejects_missing_item() {
        let parser = CgbFuturesDeliveryRulePackParser;
        let mut complete = fixture();
        complete.products = vec![
            product("T", 120, 78, None),
            product("TF", 84, 48, Some(63)),
            product("TL", 360, 300, None),
            product("TS", 60, 18, Some(27)),
        ];
        let complete_content = content(&complete);
        let rule = parser
            .parse(&complete_content, CgbFuturesProduct::TenYear)
            .unwrap();
        assert_eq!(rule.original_term_max_months(), 120);
        assert_eq!(rule.residual_min_months(), 78);
        assert_eq!(rule.residual_max_months(), None);
        assert_eq!(
            rule.nominal_coupon(),
            FixedDecimal::from_scaled(30_000_000_000)
        );
        assert_eq!(rule.contract_size_in_quote_units(), None);

        complete.products[0].contract_size_in_quote_units = Some(10_000);
        let risk_rule = parser
            .parse_for_portfolio_risk(&content(&complete), CgbFuturesProduct::TenYear)
            .unwrap();
        assert_eq!(risk_rule.contract_size_in_quote_units(), Some(10_000));

        let mut missing = complete;
        missing.products[0].residual_min_months = None;
        let error = parser
            .parse(&content(&missing), CgbFuturesProduct::TenYear)
            .unwrap_err();
        assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
        assert_eq!(
            error.detail(),
            Some(&ApplicationErrorDetail::RulePackItemMissing {
                path: "context.rule_pack.content.products[product_code=T].residual_min_months"
                    .to_owned(),
            })
        );
    }

    fn fixture() -> CgbFuturesDeliveryRulePack {
        CgbFuturesDeliveryRulePack {
            products: Vec::new(),
            delivery_months: vec![3, 6, 9, 12],
            nominal_coupon: Some(decimal("3", 2)),
            face_quote_basis: Some(decimal("100", 0)),
            accrued_interest_day_count: Some(1),
            conversion_factor_rounding_places: Some(4),
            accrued_interest_rounding_places: Some(7),
            annual_day_basis: Some(365),
        }
    }

    fn product(
        code: &str,
        original: u32,
        residual_min: u32,
        residual_max: Option<u32>,
    ) -> CgbFuturesProductRule {
        CgbFuturesProductRule {
            product_code: Some(code.to_owned()),
            original_term_max_months: Some(original),
            residual_min_months: Some(residual_min),
            residual_upper_bound: match residual_max {
                Some(value) => Some(ResidualUpperBound::ResidualMaxMonths(value)),
                None => Some(ResidualUpperBound::ResidualMaxMonthsUnbounded(true)),
            },
            contract_size_in_quote_units: None,
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

    fn content(value: &CgbFuturesDeliveryRulePack) -> RulePackContent {
        RulePackContent::new(TYPE_URL, value.encode_to_vec()).unwrap()
    }
}
