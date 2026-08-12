use chrono::NaiveDate;
use ficant_application::ports::{
    CouponTaxClaimScope, GrossCouponTaxBasis, TaxRoundingMode, TaxRulePackParser,
};
use ficant_contracts::ficant::market::v1::TaxRulePackV2;
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::market::{
    BondTaxAttributes, IncomeTaxStatus, RulePackContent, ValueAddedTaxStatus,
};
use ficant_domain::subject::TaxTreatment;
use ficant_tax_pack::{
    AUTHORITATIVE_SEMANTIC_SHA256_HEX, RATE_UNIT_ID, SOURCE, TYPE_URL_V2, TaxRulePackV2Parser,
};
use prost::Message;

const PAYLOAD: &[u8] =
    include_bytes!("../../../domain-packs/cgb-interest-tax/cgb-interest-tax-v1.bin");

#[test]
fn authoritative_v2_pack_selects_cutoff_and_reissuance_by_first_issue_date() {
    let parser = TaxRulePackV2Parser;
    let subject = subject();

    let exempt = parser
        .parse(
            &content(PAYLOAD),
            date("2025-08-07"),
            attributes(ValueAddedTaxStatus::Exempt),
            &subject,
        )
        .unwrap();
    let cutoff = parser
        .parse(
            &content(PAYLOAD),
            date("2025-08-08"),
            attributes(ValueAddedTaxStatus::Taxable),
            &subject,
        )
        .unwrap();
    let reissuance = parser
        .parse(
            &content(PAYLOAD),
            date("2025-08-07"),
            attributes(ValueAddedTaxStatus::Exempt),
            &subject,
        )
        .unwrap();

    assert_eq!(
        exempt.adjust_coupon(fixed(3_000_000_000_000)).unwrap(),
        fixed(3_000_000_000_000)
    );
    assert_eq!(reissuance, exempt);
    assert_eq!(
        cutoff.adjust_coupon(fixed(3_000_000_000_000)).unwrap(),
        fixed(2_830_188_679_245)
    );
    assert_eq!(cutoff.value_added_tax_rate(), fixed(60_000_000_000));
    assert_eq!(cutoff.income_tax_rate(), FixedDecimal::ZERO);
    assert_eq!(cutoff.unit().unit_id().as_str(), RATE_UNIT_ID);
    assert_eq!(cutoff.unit().version().get(), 1);
    assert_eq!(
        cutoff.gross_coupon_basis(),
        GrossCouponTaxBasis::VatIncluded
    );
    assert_eq!(cutoff.rounding(), TaxRoundingMode::TiesToEven);
    assert_eq!(
        cutoff.claim_scope(),
        CouponTaxClaimScope::CouponOutputVatBeforeInputCredit
    );
    assert_eq!(parser.expected_source(), Some(SOURCE));
    assert_eq!(AUTHORITATIVE_SEMANTIC_SHA256_HEX.len(), 64);
}

#[test]
fn v2_pack_rejects_profile_attributes_type_unit_and_payload_drift() {
    let parser = TaxRulePackV2Parser;
    let exact = content(PAYLOAD);
    assert!(
        parser
            .parse(
                &exact,
                date("2025-08-08"),
                attributes(ValueAddedTaxStatus::Taxable),
                &TaxTreatment::new("other", "cn-cgb-interest-cit-exempt").unwrap(),
            )
            .is_err()
    );
    assert!(
        parser
            .parse(
                &exact,
                date("2025-08-08"),
                attributes(ValueAddedTaxStatus::Exempt),
                &subject(),
            )
            .is_err()
    );
    assert!(
        parser
            .parse(
                &RulePackContent::new("type.googleapis.com/other", PAYLOAD.to_vec()).unwrap(),
                date("2025-08-08"),
                attributes(ValueAddedTaxStatus::Taxable),
                &subject(),
            )
            .is_err()
    );

    let mut drifted = TaxRulePackV2::decode(PAYLOAD).unwrap();
    drifted.coupon_rules[1].treatments[0]
        .value_added_tax_rate
        .as_mut()
        .unwrap()
        .unit
        .as_mut()
        .unwrap()
        .version = 2;
    assert!(
        parser
            .parse(
                &content(&drifted.encode_to_vec()),
                date("2025-08-08"),
                attributes(ValueAddedTaxStatus::Taxable),
                &subject(),
            )
            .is_err()
    );

    let mut payload_drift = PAYLOAD.to_vec();
    payload_drift.push(0);
    assert!(
        parser
            .parse(
                &content(&payload_drift),
                date("2025-08-08"),
                attributes(ValueAddedTaxStatus::Taxable),
                &subject(),
            )
            .is_err()
    );
}

fn content(bytes: &[u8]) -> RulePackContent {
    RulePackContent::new(TYPE_URL_V2, bytes.to_vec()).unwrap()
}

fn subject() -> TaxTreatment {
    TaxTreatment::new("cn-vat-general-taxpayer", "cn-cgb-interest-cit-exempt").unwrap()
}

fn attributes(vat: ValueAddedTaxStatus) -> BondTaxAttributes {
    BondTaxAttributes::new(vat, IncomeTaxStatus::Exempt)
}

fn date(value: &str) -> NaiveDate {
    value.parse().unwrap()
}

const fn fixed(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value)
}
