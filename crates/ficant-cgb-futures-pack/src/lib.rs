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
pub const EXPECTED_DECIMAL_UNIT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAP";
pub const EXPECTED_DECIMAL_UNIT_VERSION: u64 = 1;

const PER_100_FACE_QUOTE_BASIS: FixedDecimal = FixedDecimal::from_scaled(100_000_000_000_000);

/// Parses only the exact v1/v2 CGB futures-delivery content schema.
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

    fn parse_for_per_100_hedge(
        &self,
        content: &RulePackContent,
        product: CgbFuturesProduct,
    ) -> ApplicationResult<FuturesDeliveryRule> {
        if content.type_url() != TYPE_URL {
            return Err(invalid());
        }
        let payload = CgbFuturesDeliveryRulePack::decode(content.value()).map_err(|_| invalid())?;
        validate_expected_unit(payload.nominal_coupon.as_ref(), "nominal_coupon")?;
        validate_expected_unit(payload.face_quote_basis.as_ref(), "face_quote_basis")?;
        let rule = parse_payload(&payload, product)?;
        if rule.face_quote_basis() != PER_100_FACE_QUOTE_BASIS {
            return Err(invalid());
        }
        if rule.contract_size_in_quote_units().is_none() {
            return Err(missing(&format!(
                "products[product_code={}].contract_size_in_quote_units",
                product.code()
            )));
        }
        let notional = rule
            .contract_notional()
            .map_err(map_domain_error)?
            .ok_or_else(invalid)?;
        if !notional.is_positive() {
            return Err(invalid());
        }
        Ok(rule)
    }
}

fn validate_expected_unit(value: Option<&DecimalValue>, path: &str) -> ApplicationResult<()> {
    let value = value.ok_or_else(|| missing(path))?;
    let unit = value
        .unit
        .as_ref()
        .ok_or_else(|| missing(&format!("{path}.unit")))?;
    let unit_id = unit
        .unit_id
        .as_ref()
        .ok_or_else(|| missing(&format!("{path}.unit.unit_id")))?;
    let parsed_unit = UnitRef::new(
        Ulid::new(unit_id.value.clone()).map_err(map_domain_error)?,
        Version::new(unit.version).map_err(map_domain_error)?,
    );
    if parsed_unit.unit_id().as_str() != EXPECTED_DECIMAL_UNIT_ID
        || parsed_unit.version().get() != EXPECTED_DECIMAL_UNIT_VERSION
    {
        return Err(invalid());
    }
    Ok(())
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
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use ficant_application::{
        ApplicationErrorDetail, parse_verified_delivery_rule_for_per_100_hedge,
    };
    use ficant_contracts::ficant::core::v1::{Ulid as ProtoUlid, UnitRef as ProtoUnitRef};
    use ficant_domain::market::{MarketRulePack, MarketRulePackInput, VerificationStatus};
    use ficant_domain::primitives::{ContentHash, EffectivePeriod, MarketTime, OwnerRef};

    struct LegacyOnlyParser;

    impl FuturesDeliveryRuleParser for LegacyOnlyParser {
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
            product: CgbFuturesProduct,
        ) -> ApplicationResult<FuturesDeliveryRule> {
            CgbFuturesDeliveryRulePackParser.parse(content, product)
        }
    }

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

    #[test]
    fn default_parse_for_per_100_hedge_fails_closed() {
        let raw = fs::read(repo_root().join("domain-packs/cgb-futures/cgb-futures-v2.bin"))
            .expect("v2 bin exists");
        let pack = verified_pack(raw);
        let error = parse_verified_delivery_rule_for_per_100_hedge(
            &pack,
            None,
            CgbFuturesProduct::TwoYear,
            &LegacyOnlyParser,
        )
        .expect_err("default trait implementation must reject per-100 hedge");
        assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    }

    #[test]
    fn authoritative_v2_bin_parses_all_four_products_for_per_100_hedge() {
        let raw = fs::read(repo_root().join("domain-packs/cgb-futures/cgb-futures-v2.bin"))
            .expect("v2 bin exists");
        let pack = verified_pack(raw);
        let parser = CgbFuturesDeliveryRulePackParser;

        let expected = [
            (CgbFuturesProduct::TwoYear, 20_000_u32, 2_000_000_i128),
            (CgbFuturesProduct::FiveYear, 10_000_u32, 1_000_000_i128),
            (CgbFuturesProduct::TenYear, 10_000_u32, 1_000_000_i128),
            (CgbFuturesProduct::ThirtyYear, 10_000_u32, 1_000_000_i128),
        ];

        for (product, size, notional) in expected {
            let rule =
                parse_verified_delivery_rule_for_per_100_hedge(&pack, None, product, &parser)
                    .expect("v2 pack parses for per-100 hedge");
            assert_eq!(rule.face_quote_basis(), PER_100_FACE_QUOTE_BASIS);
            assert_eq!(rule.contract_size_in_quote_units(), Some(size));
            assert_eq!(
                rule.contract_notional().expect("valid notional"),
                Some(FixedDecimal::from_scaled(notional * 1_000_000_000_000))
            );
        }
    }

    #[test]
    fn v1_bin_remains_compatible_for_delivery_and_reports_missing_size_path_for_hedge() {
        let raw = fs::read(repo_root().join("domain-packs/cgb-futures/cgb-futures-v1.bin"))
            .expect("v1 bin exists");
        let pack = verified_pack(raw.clone());
        let parser = CgbFuturesDeliveryRulePackParser;
        let v1_content = RulePackContent::new(TYPE_URL, raw).unwrap();

        for product in [
            CgbFuturesProduct::TwoYear,
            CgbFuturesProduct::FiveYear,
            CgbFuturesProduct::TenYear,
            CgbFuturesProduct::ThirtyYear,
        ] {
            let delivery_rule = parser
                .parse(&v1_content, product)
                .expect("v1 pack remains readable by delivery parser");
            assert_eq!(delivery_rule.contract_size_in_quote_units(), None);

            let error =
                parse_verified_delivery_rule_for_per_100_hedge(&pack, None, product, &parser)
                    .expect_err("v1 pack lacks contract_size_in_quote_units");
            assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
            assert_eq!(
                error.detail(),
                Some(&ApplicationErrorDetail::RulePackItemMissing {
                    path: format!(
                        "context.rule_pack.content.products[product_code={}].contract_size_in_quote_units",
                        product.code()
                    ),
                })
            );
        }
    }

    #[test]
    fn per_100_hedge_rejects_non_positive_contract_size_and_non_100_basis() {
        let parser = CgbFuturesDeliveryRulePackParser;
        let base = decode_v2_payload();

        let mut zero_size = base.clone();
        zero_size
            .products
            .iter_mut()
            .find(|p| p.product_code.as_deref() == Some("TS"))
            .expect("TS exists")
            .contract_size_in_quote_units = Some(0);
        let error = parse_verified_delivery_rule_for_per_100_hedge(
            &verified_pack(zero_size.encode_to_vec()),
            None,
            CgbFuturesProduct::TwoYear,
            &parser,
        )
        .expect_err("zero contract size must be rejected");
        assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);

        let mut basis_200 = base;
        basis_200.face_quote_basis = Some(decimal("200", 0));
        let error = parse_verified_delivery_rule_for_per_100_hedge(
            &verified_pack(basis_200.encode_to_vec()),
            None,
            CgbFuturesProduct::TwoYear,
            &parser,
        )
        .expect_err("non-100 face_quote_basis must be rejected");
        assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    }

    #[test]
    fn per_100_hedge_rejects_all_four_rehashed_unit_binding_mutations() {
        let parser = CgbFuturesDeliveryRulePackParser;
        let base = decode_v2_payload();
        let alternate_ulid = "01ARZ3NDEKTSV4RRFFQ69G5FAQ";

        // (a) Replace only face_quote_basis unit_id with another valid ULID.
        let mut mutated_face_id = base.clone();
        mutated_face_id.face_quote_basis = Some(decimal_with_unit("100", 0, alternate_ulid, 1));
        let err_a = parse_verified_delivery_rule_for_per_100_hedge(
            &verified_pack(mutated_face_id.encode_to_vec()),
            None,
            CgbFuturesProduct::TwoYear,
            &parser,
        )
        .expect_err("face_quote_basis unit_id mutation must fail");
        assert_eq!(err_a.category(), ApplicationErrorCategory::ValidationFailed);

        // (b) Replace only face_quote_basis unit version with 2.
        let mut mutated_face_ver = base.clone();
        mutated_face_ver.face_quote_basis =
            Some(decimal_with_unit("100", 0, EXPECTED_DECIMAL_UNIT_ID, 2));
        let err_b = parse_verified_delivery_rule_for_per_100_hedge(
            &verified_pack(mutated_face_ver.encode_to_vec()),
            None,
            CgbFuturesProduct::TwoYear,
            &parser,
        )
        .expect_err("face_quote_basis unit version mutation must fail");
        assert_eq!(err_b.category(), ApplicationErrorCategory::ValidationFailed);

        // (c) Replace only nominal_coupon unit binding (ID and version).
        let mut mutated_coupon_id = base.clone();
        mutated_coupon_id.nominal_coupon = Some(decimal_with_unit("3", 2, alternate_ulid, 1));
        let err_c_id = parse_verified_delivery_rule_for_per_100_hedge(
            &verified_pack(mutated_coupon_id.encode_to_vec()),
            None,
            CgbFuturesProduct::TwoYear,
            &parser,
        )
        .expect_err("nominal_coupon unit_id mutation must fail");
        assert_eq!(
            err_c_id.category(),
            ApplicationErrorCategory::ValidationFailed
        );

        let mut mutated_coupon_ver = base.clone();
        mutated_coupon_ver.nominal_coupon =
            Some(decimal_with_unit("3", 2, EXPECTED_DECIMAL_UNIT_ID, 2));
        let err_c_ver = parse_verified_delivery_rule_for_per_100_hedge(
            &verified_pack(mutated_coupon_ver.encode_to_vec()),
            None,
            CgbFuturesProduct::TwoYear,
            &parser,
        )
        .expect_err("nominal_coupon unit version mutation must fail");
        assert_eq!(
            err_c_ver.category(),
            ApplicationErrorCategory::ValidationFailed
        );

        // (d) Replace both nominal_coupon and face_quote_basis unit_id with another valid ULID.
        let mut mutated_both = base;
        mutated_both.nominal_coupon = Some(decimal_with_unit("3", 2, alternate_ulid, 1));
        mutated_both.face_quote_basis = Some(decimal_with_unit("100", 0, alternate_ulid, 1));
        let err_d = parse_verified_delivery_rule_for_per_100_hedge(
            &verified_pack(mutated_both.encode_to_vec()),
            None,
            CgbFuturesProduct::TwoYear,
            &parser,
        )
        .expect_err("both unit_id mutations must fail");
        assert_eq!(err_d.category(), ApplicationErrorCategory::ValidationFailed);
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    }

    fn decode_v2_payload() -> CgbFuturesDeliveryRulePack {
        let raw = fs::read(repo_root().join("domain-packs/cgb-futures/cgb-futures-v2.bin"))
            .expect("v2 bin exists");
        CgbFuturesDeliveryRulePack::decode(raw.as_slice()).expect("valid v2 payload")
    }

    fn verified_pack(raw: Vec<u8>) -> MarketRulePack {
        let hash = ContentHash::digest(&raw);
        let content = RulePackContent::new(TYPE_URL, raw).unwrap();
        let owner_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let from = MarketTime::new(
            "2025-12-31T16:00:00Z".parse().unwrap(),
            "Asia/Shanghai",
            "2026-01-01".parse().unwrap(),
        )
        .unwrap();
        let to = MarketTime::new(
            "2027-12-31T16:00:00Z".parse().unwrap(),
            "Asia/Shanghai",
            "2028-01-01".parse().unwrap(),
        )
        .unwrap();
        MarketRulePack::new_with_content(
            MarketRulePackInput {
                rule_pack_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FAF").unwrap(),
                version: Version::new(2).unwrap(),
                owner: OwnerRef::new(owner_id.clone(), owner_id),
                market: MARKET.to_owned(),
                rule_type: RULE_TYPE.to_owned(),
                source: "cffex-rulebook".to_owned(),
                effective: EffectivePeriod::new(from, to).unwrap(),
                verification_status: VerificationStatus::Verified,
                content_hash: hash,
            },
            content,
        )
        .unwrap()
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
        decimal_with_unit(
            coefficient,
            scale,
            EXPECTED_DECIMAL_UNIT_ID,
            EXPECTED_DECIMAL_UNIT_VERSION,
        )
    }

    fn decimal_with_unit(
        coefficient: &str,
        scale: u32,
        unit_id: &str,
        version: u64,
    ) -> DecimalValue {
        DecimalValue {
            coefficient: coefficient.to_owned(),
            scale,
            unit: Some(ProtoUnitRef {
                unit_id: Some(ProtoUlid {
                    value: unit_id.to_owned(),
                }),
                version,
            }),
        }
    }

    fn content(value: &CgbFuturesDeliveryRulePack) -> RulePackContent {
        RulePackContent::new(TYPE_URL, value.encode_to_vec()).unwrap()
    }
}
