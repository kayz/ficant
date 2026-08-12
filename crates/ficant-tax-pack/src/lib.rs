//! L3 parser for the typed coupon-tax `RulePack` payload.

use chrono::NaiveDate;
use ficant_application::ports::{
    ApplicationResult, CouponTaxRate, CouponTaxTreatment, TaxRulePackParser,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1::DecimalValue;
use ficant_contracts::ficant::market::v1::{
    BondCouponTaxRule, BondCouponTaxTreatmentRule, BondTaxAttributes as ProtoBondTaxAttributes,
    CouponTaxClaimScope as ProtoClaimScope, GrossCouponTaxBasis as ProtoGrossBasis,
    IncomeTaxStatus as ProtoIncomeTaxStatus, SubjectCouponTaxRate, SubjectCouponTaxTreatment,
    TaxRoundingMode as ProtoRounding, TaxRulePack, TaxRulePackV2,
    ValueAddedTaxStatus as ProtoValueAddedTaxStatus,
};
use ficant_domain::analytics::{DECIMAL_SCALE, FixedDecimal};
use ficant_domain::market::{
    BondTaxAttributes, IncomeTaxStatus, RulePackContent, ValueAddedTaxStatus,
};
use ficant_domain::primitives::{DecimalValue as DomainDecimalValue, Ulid, UnitRef, Version};
use ficant_domain::subject::TaxTreatment;
use prost::Message;

pub const MARKET: &str = "CN";
pub const RULE_TYPE: &str = "tax";
pub const TYPE_URL: &str = "type.googleapis.com/ficant.market.v1.TaxRulePack";
pub const TYPE_URL_V2: &str = "type.googleapis.com/ficant.market.v1.TaxRulePackV2";
pub const SOURCE: &str = "ficant-authority/cgb-interest-tax/v1";
pub const RATE_UNIT_ID: &str = "01K2CGBVAT0000000000000000";
pub const AUTHORITATIVE_SEMANTIC_SHA256_HEX: &str =
    "54fa5adbeb8b164dc779ecc250ab622ab5747cdeb36f2b6da58f4d877ce5106a";
pub const AUTHORITATIVE_PAYLOAD_SHA256_HEX: &str =
    "14748fb4d27d01b35ebe466f72669937c850fd48f9bbd875542848d3800168db";

const AUTHORITATIVE_PAYLOAD: &[u8] =
    include_bytes!("../../../domain-packs/cgb-interest-tax/cgb-interest-tax-v1.bin");

/// Parses only the authority-approved v2 coupon treatment payload.
#[derive(Clone, Copy, Debug, Default)]
pub struct TaxRulePackV2Parser;

impl TaxRulePackParser for TaxRulePackV2Parser {
    fn market(&self) -> &'static str {
        MARKET
    }

    fn rule_type(&self) -> &'static str {
        RULE_TYPE
    }

    fn type_url(&self) -> &'static str {
        TYPE_URL_V2
    }

    fn expected_source(&self) -> Option<&'static str> {
        Some(SOURCE)
    }

    fn expected_effective_window(&self) -> Option<(&'static str, &'static str)> {
        Some(("2026-01-01T00:00:00+08:00", "2028-01-01T00:00:00+08:00"))
    }

    fn expected_rate_unit(
        &self,
    ) -> Option<(&'static str, u64, &'static str, &'static str, u32, u32)> {
        Some((RATE_UNIT_ID, 1, "RATE", "rate", 12, 18))
    }

    fn parse(
        &self,
        content: &RulePackContent,
        first_issue_date: NaiveDate,
        tax_attributes: BondTaxAttributes,
        tax_treatment: &TaxTreatment,
    ) -> ApplicationResult<CouponTaxTreatment> {
        if content.type_url() != TYPE_URL_V2 || content.value() != AUTHORITATIVE_PAYLOAD {
            return Err(invalid());
        }
        let payload = TaxRulePackV2::decode(content.value()).map_err(|_| invalid())?;
        parse_v2_payload(&payload, first_issue_date, tax_attributes, tax_treatment)
    }
}

fn parse_v2_payload(
    payload: &TaxRulePackV2,
    first_issue_date: NaiveDate,
    tax_attributes: BondTaxAttributes,
    tax_treatment: &TaxTreatment,
) -> ApplicationResult<CouponTaxTreatment> {
    validate_authoritative_v2_shape(&payload.coupon_rules)?;
    let date_path = format!("coupon_rules[first_issue_date={first_issue_date}]");
    let selected = payload
        .coupon_rules
        .iter()
        .find(|rule| v2_contains(rule, first_issue_date))
        .ok_or_else(|| missing(&date_path))?;
    let selected_attributes = attributes(
        selected.tax_attributes.as_ref(),
        &format!("{date_path}.tax_attributes"),
    )?;
    if selected_attributes != tax_attributes {
        return Err(missing(&format!("{date_path}.tax_attributes")));
    }
    let profile_path = format!(
        "{date_path}.treatments[vat_profile={}][income_profile={}]",
        tax_treatment.value_added_tax_profile(),
        tax_treatment.income_tax_profile()
    );
    let treatment = selected
        .treatments
        .iter()
        .find(|value| {
            value.value_added_tax_profile == tax_treatment.value_added_tax_profile()
                && value.income_tax_profile == tax_treatment.income_tax_profile()
        })
        .ok_or_else(|| missing(&profile_path))?;
    parse_v2_treatment(treatment, &profile_path)
}

fn validate_authoritative_v2_shape(rules: &[BondCouponTaxTreatmentRule]) -> ApplicationResult<()> {
    if rules.len() != 2
        || rules[0].first_issue_from != "0001-01-01"
        || rules[0].first_issue_to != "2025-08-08"
        || rules[1].first_issue_from != "2025-08-08"
        || !rules[1].first_issue_to.is_empty()
        || rules.iter().any(|rule| rule.treatments.len() != 1)
    {
        return Err(invalid());
    }
    let pre = attributes(
        rules[0].tax_attributes.as_ref(),
        "coupon_rules[0].tax_attributes",
    )?;
    let post = attributes(
        rules[1].tax_attributes.as_ref(),
        "coupon_rules[1].tax_attributes",
    )?;
    if pre != BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt)
        || post != BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Exempt)
    {
        return Err(invalid());
    }
    Ok(())
}

fn v2_contains(rule: &BondCouponTaxTreatmentRule, value: NaiveDate) -> bool {
    let Ok(from) = NaiveDate::parse_from_str(&rule.first_issue_from, "%Y-%m-%d") else {
        return false;
    };
    let to = if rule.first_issue_to.is_empty() {
        None
    } else {
        NaiveDate::parse_from_str(&rule.first_issue_to, "%Y-%m-%d").ok()
    };
    from <= value && to.is_none_or(|to| value < to)
}

fn parse_v2_treatment(
    value: &SubjectCouponTaxTreatment,
    path: &str,
) -> ApplicationResult<CouponTaxTreatment> {
    if value.value_added_tax_profile != "cn-vat-general-taxpayer"
        || value.income_tax_profile != "cn-cgb-interest-cit-exempt"
        || ProtoGrossBasis::try_from(value.gross_coupon_basis).map_err(|_| invalid())?
            != ProtoGrossBasis::VatIncluded
        || ProtoRounding::try_from(value.rounding).map_err(|_| invalid())?
            != ProtoRounding::TiesToEven
        || ProtoClaimScope::try_from(value.claim_scope).map_err(|_| invalid())?
            != ProtoClaimScope::CouponOutputVatBeforeInputCredit
    {
        return Err(invalid());
    }
    let (vat_rate, vat_unit) = decimal_parts(
        value.value_added_tax_rate.as_ref(),
        &format!("{path}.value_added_tax_rate"),
    )?;
    let (income_rate, income_unit) = decimal_parts(
        value.income_tax_rate.as_ref(),
        &format!("{path}.income_tax_rate"),
    )?;
    if vat_unit != income_unit
        || vat_unit.unit_id().as_str() != RATE_UNIT_ID
        || vat_unit.version().get() != 1
        || income_rate != FixedDecimal::ZERO
        || !matches!(vat_rate.scaled(), 0 | 60_000_000_000)
    {
        return Err(invalid());
    }
    CouponTaxTreatment::vat_included(
        vat_rate,
        income_rate,
        vat_unit,
        ficant_domain::primitives::ContentHash::from_bytes(&[
            0x54, 0xfa, 0x5a, 0xdb, 0xeb, 0x8b, 0x16, 0x4d, 0xc7, 0x79, 0xec, 0xc2, 0x50, 0xab,
            0x62, 0x2a, 0xb5, 0x74, 0xcd, 0xeb, 0x36, 0xf2, 0xb6, 0xda, 0x58, 0xf4, 0xd8, 0x77,
            0xce, 0x51, 0x06, 0x0a,
        ])
        .expect("authority hash has exactly 32 bytes"),
    )
}

/// Parses only the exact v1 coupon-tax rule content schema.
#[derive(Clone, Copy, Debug, Default)]
pub struct TaxRulePackV1Parser;

impl TaxRulePackParser for TaxRulePackV1Parser {
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
        first_issue_date: NaiveDate,
        tax_attributes: BondTaxAttributes,
        tax_treatment: &TaxTreatment,
    ) -> ApplicationResult<CouponTaxRate> {
        if content.type_url() != TYPE_URL {
            return Err(invalid());
        }
        let payload = TaxRulePack::decode(content.value()).map_err(|_| invalid())?;
        parse_payload(&payload, first_issue_date, tax_attributes, tax_treatment)
    }
}

fn parse_payload(
    payload: &TaxRulePack,
    first_issue_date: NaiveDate,
    tax_attributes: BondTaxAttributes,
    tax_treatment: &TaxTreatment,
) -> ApplicationResult<CouponTaxRate> {
    let rules = parse_rules(&payload.coupon_rules)?;
    let date_path = format!("coupon_rules[first_issue_date={first_issue_date}]");
    let selected = rules
        .iter()
        .find(|rule| rule.contains(first_issue_date))
        .ok_or_else(|| missing(&date_path))?;
    if !attributes_match(selected.attributes, tax_attributes) {
        return Err(missing(&format!("{date_path}.tax_attributes")));
    }
    let profile_path = format!(
        "{date_path}.rates[vat_profile={}][income_profile={}]",
        tax_treatment.value_added_tax_profile(),
        tax_treatment.income_tax_profile()
    );
    let rate = selected
        .rates
        .iter()
        .find(|rate| {
            rate.value_added_tax_profile == tax_treatment.value_added_tax_profile()
                && rate.income_tax_profile == tax_treatment.income_tax_profile()
        })
        .ok_or_else(|| missing(&profile_path))?;
    Ok(rate.rate.clone())
}

#[derive(Clone)]
struct ParsedRule {
    from: NaiveDate,
    to: Option<NaiveDate>,
    attributes: BondTaxAttributes,
    rates: Vec<CouponTaxRateEntry>,
}

impl ParsedRule {
    fn contains(&self, value: NaiveDate) -> bool {
        self.from <= value && self.to.is_none_or(|to| value < to)
    }
}

#[derive(Clone)]
struct CouponTaxRateEntry {
    value_added_tax_profile: String,
    income_tax_profile: String,
    rate: CouponTaxRate,
}

fn parse_rules(values: &[BondCouponTaxRule]) -> ApplicationResult<Vec<ParsedRule>> {
    let mut parsed = Vec::with_capacity(values.len());
    let mut previous_from = None;
    let mut previous_to = None;
    for (index, value) in values.iter().enumerate() {
        let path = format!("coupon_rules[{index}]");
        let from = date(&value.first_issue_from, &format!("{path}.first_issue_from"))?;
        let to = if value.first_issue_to.is_empty() {
            None
        } else {
            Some(date(
                &value.first_issue_to,
                &format!("{path}.first_issue_to"),
            )?)
        };
        if to.is_some_and(|to| to <= from)
            || previous_from.is_some_and(|previous| previous >= from)
            || previous_to.is_none() && previous_from.is_some()
            || previous_to.is_some_and(|previous| previous > from)
        {
            return Err(invalid());
        }
        let attributes = attributes(
            value.tax_attributes.as_ref(),
            &format!("{path}.tax_attributes"),
        )?;
        let rates = parse_rates(&value.rates, &format!("{path}.rates"))?;
        previous_from = Some(from);
        previous_to = to;
        parsed.push(ParsedRule {
            from,
            to,
            attributes,
            rates,
        });
    }
    Ok(parsed)
}

fn parse_rates(
    values: &[SubjectCouponTaxRate],
    path: &str,
) -> ApplicationResult<Vec<CouponTaxRateEntry>> {
    let mut parsed = Vec::with_capacity(values.len());
    let mut previous: Option<(&str, &str)> = None;
    for (index, value) in values.iter().enumerate() {
        if value.value_added_tax_profile.trim().is_empty()
            || value.value_added_tax_profile != value.value_added_tax_profile.trim()
            || value.income_tax_profile.trim().is_empty()
            || value.income_tax_profile != value.income_tax_profile.trim()
        {
            return Err(invalid());
        }
        let profile = (
            value.value_added_tax_profile.as_str(),
            value.income_tax_profile.as_str(),
        );
        if previous.is_some_and(|previous| previous >= profile) {
            return Err(invalid());
        }
        let rate = decimal(
            value.coupon_tax_rate.as_ref(),
            &format!("{path}[{index}].coupon_tax_rate"),
        )?;
        if rate.coupon_tax_rate() < FixedDecimal::ZERO || rate.coupon_tax_rate() > FixedDecimal::ONE
        {
            return Err(invalid());
        }
        previous = Some(profile);
        parsed.push(CouponTaxRateEntry {
            value_added_tax_profile: value.value_added_tax_profile.clone(),
            income_tax_profile: value.income_tax_profile.clone(),
            rate,
        });
    }
    Ok(parsed)
}

fn attributes(
    value: Option<&ProtoBondTaxAttributes>,
    path: &str,
) -> ApplicationResult<BondTaxAttributes> {
    let value = value.ok_or_else(|| missing(path))?;
    let value_added_tax_status =
        match ProtoValueAddedTaxStatus::try_from(value.value_added_tax_status)
            .map_err(|_| invalid())?
        {
            ProtoValueAddedTaxStatus::Exempt => ValueAddedTaxStatus::Exempt,
            ProtoValueAddedTaxStatus::Taxable => ValueAddedTaxStatus::Taxable,
            ProtoValueAddedTaxStatus::Unspecified => {
                return Err(missing(&format!("{path}.value_added_tax_status")));
            }
        };
    let income_tax_status =
        match ProtoIncomeTaxStatus::try_from(value.income_tax_status).map_err(|_| invalid())? {
            ProtoIncomeTaxStatus::Exempt => IncomeTaxStatus::Exempt,
            ProtoIncomeTaxStatus::Taxable => IncomeTaxStatus::Taxable,
            ProtoIncomeTaxStatus::Unspecified => {
                return Err(missing(&format!("{path}.income_tax_status")));
            }
        };
    Ok(BondTaxAttributes::new(
        value_added_tax_status,
        income_tax_status,
    ))
}

fn attributes_match(left: BondTaxAttributes, right: BondTaxAttributes) -> bool {
    left == right
}

fn date(value: &str, path: &str) -> ApplicationResult<NaiveDate> {
    let parsed = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| missing(path))?;
    if parsed.to_string() != value {
        return Err(missing(path));
    }
    Ok(parsed)
}

fn decimal_parts(
    value: Option<&DecimalValue>,
    path: &str,
) -> ApplicationResult<(FixedDecimal, UnitRef)> {
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

fn decimal(value: Option<&DecimalValue>, path: &str) -> ApplicationResult<CouponTaxRate> {
    let (value, unit) = decimal_parts(value, path)?;
    Ok(CouponTaxRate::new(value, unit))
}

fn power_of_ten(exponent: u32) -> ApplicationResult<i128> {
    10_i128.checked_pow(exponent).ok_or_else(invalid)
}

fn missing(path: &str) -> ApplicationError {
    ApplicationError::rule_pack_item_missing(format!("context.tax_rule_pack.content.{path}"))
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
    fn selects_first_issue_interval_and_complete_subject_profile_pair() {
        let parser = TaxRulePackV1Parser;
        let payload = TaxRulePack {
            coupon_rules: vec![
                rule(
                    "2020-01-01",
                    "2025-08-08",
                    decimal("0", 0),
                    decimal("13", 2),
                ),
                rule("2025-08-08", "", decimal("25", 2), decimal("3", 1)),
            ],
        };
        let attributes = taxable_attributes();
        let selected = parser
            .parse(
                &content(&payload),
                date("2025-08-07"),
                attributes,
                &TaxTreatment::new("synthetic-vat-b", "synthetic-income-b").unwrap(),
            )
            .expect("pre-cutoff profile resolves");
        assert_eq!(
            selected.coupon_tax_rate(),
            FixedDecimal::from_scaled(130_000_000_000)
        );

        let selected = parser
            .parse(
                &content(&payload),
                date("2025-08-08"),
                attributes,
                &TaxTreatment::new("synthetic-vat-b", "synthetic-income-b").unwrap(),
            )
            .expect("cutoff starts the later half-open interval");
        assert_eq!(
            selected.coupon_tax_rate(),
            FixedDecimal::from_scaled(300_000_000_000)
        );
    }

    #[test]
    fn missing_subject_profile_and_mismatched_attributes_fail_closed_with_safe_paths() {
        let parser = TaxRulePackV1Parser;
        let payload = TaxRulePack {
            coupon_rules: vec![rule("2020-01-01", "", decimal("0", 0), decimal("13", 2))],
        };
        let missing_profile = parser
            .parse(
                &content(&payload),
                date("2025-08-08"),
                taxable_attributes(),
                &TaxTreatment::new("synthetic-vat-c", "synthetic-income-c").unwrap(),
            )
            .expect_err("unknown complete profile pair fails closed");
        assert_eq!(
            missing_profile.category(),
            ApplicationErrorCategory::ValidationFailed
        );
        assert_eq!(
            missing_profile.detail(),
            Some(&ApplicationErrorDetail::RulePackItemMissing {
                path: "context.tax_rule_pack.content.coupon_rules[first_issue_date=2025-08-08].rates[vat_profile=synthetic-vat-c][income_profile=synthetic-income-c]".to_owned(),
            })
        );

        let mismatched_attributes = parser
            .parse(
                &content(&payload),
                date("2025-08-08"),
                BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Taxable),
                &TaxTreatment::new("synthetic-vat-a", "synthetic-income-a").unwrap(),
            )
            .expect_err("mismatched Bond attributes fail closed");
        assert_eq!(
            mismatched_attributes.detail(),
            Some(&ApplicationErrorDetail::RulePackItemMissing {
                path: "context.tax_rule_pack.content.coupon_rules[first_issue_date=2025-08-08].tax_attributes".to_owned(),
            })
        );
    }

    #[test]
    fn rejects_overlapping_intervals_and_noncanonical_rate() {
        let parser = TaxRulePackV1Parser;
        let overlapping = TaxRulePack {
            coupon_rules: vec![
                rule(
                    "2020-01-01",
                    "2025-08-09",
                    decimal("0", 0),
                    decimal("13", 2),
                ),
                rule("2025-08-08", "", decimal("25", 2), decimal("3", 1)),
            ],
        };
        let error = parser
            .parse(
                &content(&overlapping),
                date("2025-08-08"),
                taxable_attributes(),
                &TaxTreatment::new("synthetic-vat-a", "synthetic-income-a").unwrap(),
            )
            .expect_err("overlapping intervals are invalid");
        assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
        assert_eq!(error.detail(), None);

        let noncanonical = TaxRulePack {
            coupon_rules: vec![BondCouponTaxRule {
                first_issue_from: "2020-01-01".to_owned(),
                first_issue_to: String::new(),
                tax_attributes: Some(proto_taxable_attributes()),
                rates: vec![SubjectCouponTaxRate {
                    value_added_tax_profile: "synthetic-vat-a".to_owned(),
                    income_tax_profile: "synthetic-income-a".to_owned(),
                    coupon_tax_rate: Some(DecimalValue {
                        coefficient: "013".to_owned(),
                        scale: 2,
                        unit: Some(proto_unit()),
                    }),
                }],
            }],
        };
        let error = parser
            .parse(
                &content(&noncanonical),
                date("2025-08-08"),
                taxable_attributes(),
                &TaxTreatment::new("synthetic-vat-a", "synthetic-income-a").unwrap(),
            )
            .expect_err("noncanonical decimal coefficients are rejected");
        assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
        assert_eq!(error.detail(), None);
    }

    fn rule(
        from: &str,
        to: &str,
        first_rate: DecimalValue,
        second_rate: DecimalValue,
    ) -> BondCouponTaxRule {
        BondCouponTaxRule {
            first_issue_from: from.to_owned(),
            first_issue_to: to.to_owned(),
            tax_attributes: Some(proto_taxable_attributes()),
            rates: vec![
                SubjectCouponTaxRate {
                    value_added_tax_profile: "synthetic-vat-a".to_owned(),
                    income_tax_profile: "synthetic-income-a".to_owned(),
                    coupon_tax_rate: Some(first_rate),
                },
                SubjectCouponTaxRate {
                    value_added_tax_profile: "synthetic-vat-b".to_owned(),
                    income_tax_profile: "synthetic-income-b".to_owned(),
                    coupon_tax_rate: Some(second_rate),
                },
            ],
        }
    }

    fn taxable_attributes() -> BondTaxAttributes {
        BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Taxable)
    }

    fn proto_taxable_attributes() -> ProtoBondTaxAttributes {
        ProtoBondTaxAttributes {
            value_added_tax_status: ProtoValueAddedTaxStatus::Taxable as i32,
            income_tax_status: ProtoIncomeTaxStatus::Taxable as i32,
        }
    }

    fn decimal(coefficient: &str, scale: u32) -> DecimalValue {
        DecimalValue {
            coefficient: coefficient.to_owned(),
            scale,
            unit: Some(proto_unit()),
        }
    }

    fn proto_unit() -> ProtoUnitRef {
        ProtoUnitRef {
            unit_id: Some(ProtoUlid {
                value: "01ARZ3NDEKTSV4RRFFQ69G5FAP".to_owned(),
            }),
            version: 1,
        }
    }

    fn content(value: &TaxRulePack) -> RulePackContent {
        RulePackContent::new(TYPE_URL, value.encode_to_vec()).unwrap()
    }

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").unwrap()
    }
}
