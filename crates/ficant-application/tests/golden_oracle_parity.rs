use std::fs;
use std::path::PathBuf;

use chrono::NaiveDate;
use ficant_application::ports::{CouponTaxClaimScope, CouponTaxTreatment, TaxRulePackParser};
use ficant_application::use_cases::{
    CalculateBondAnalytics, calculate_capital_use, project_position_views, subject_delivery_irr,
};
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondTerms, BusinessDayConvention,
    CalendarBinding, CalendarRequirement, CouponFrequency, DECIMAL_SCALE, DayCountConvention,
    FixedDecimal, MARKET_TIMEZONE,
};
use ficant_domain::market::{
    BondTaxAttributes, IncomeTaxStatus, RulePackContent, ValueAddedTaxStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    AccountingBook, AccountingClassification, AccountingClassificationState, Position,
    PositionHoldingForm, PositionInput, PositionSnapshot, PositionSnapshotInput,
};
use ficant_domain::subject::TaxTreatment;
use ficant_fixed_income_native::NativeBondAnalyticsEngine;
use ficant_tax_pack::{TYPE_URL_V2, TaxRulePackV2Parser};
use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn id(s: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{s}")).unwrap()
}

fn parse_fixed(text: &str) -> FixedDecimal {
    if let Some((whole, frac)) = text.split_once('.') {
        let padded = format!("{frac:0<12}");
        let whole_val: i128 = whole.parse().unwrap();
        let frac_val: i128 = padded[..12].parse().unwrap();
        let scaled = if text.starts_with('-') {
            whole_val * 1_000_000_000_000 - frac_val
        } else {
            whole_val * 1_000_000_000_000 + frac_val
        };
        FixedDecimal::from_scaled(scaled)
    } else {
        let whole_val: i128 = text.parse().unwrap();
        FixedDecimal::from_scaled(whole_val * 1_000_000_000_000)
    }
}

fn format_fixed(value: FixedDecimal) -> String {
    let scaled = value.scaled();
    let sign = if scaled < 0 { "-" } else { "" };
    let abs = scaled.unsigned_abs();
    let base = 10_u128.pow(DECIMAL_SCALE);
    format!("{sign}{}.{:012}", abs / base, abs % base)
}

#[test]
#[allow(clippy::too_many_lines)]
fn r5e_tax_adjusted_bond_and_delivery_golden_parity() {
    let root = repo_root();
    let inputs: Value = serde_json::from_str(
        &fs::read_to_string(
            root.join("tests/golden-cases/china-rates/r5e-tax-adjusted-analytics-inputs.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let expected: Value = serde_json::from_str(
        &fs::read_to_string(root.join(
            "tests/golden-cases/china-rates/expected/r5e-tax-adjusted-analytics-expected.json",
        ))
        .unwrap(),
    )
    .unwrap();
    let pack_bytes =
        fs::read(root.join("domain-packs/cgb-interest-tax/cgb-interest-tax-v1.bin")).unwrap();
    let content = RulePackContent::new(TYPE_URL_V2, pack_bytes).unwrap();
    let subject_tax =
        TaxTreatment::new("cn-vat-general-taxpayer", "cn-cgb-interest-cit-exempt").unwrap();
    let parser = TaxRulePackV2Parser;

    for (inp, exp) in inputs["bond_cases"]
        .as_array()
        .unwrap()
        .iter()
        .zip(expected["bond_cases"].as_array().unwrap())
    {
        let first_issue: NaiveDate = inp["first_issue_date"].as_str().unwrap().parse().unwrap();
        let current_issue: NaiveDate = inp["current_issue_date"].as_str().unwrap().parse().unwrap();
        let vat = match inp["value_added_tax_status"].as_str().unwrap() {
            "EXEMPT" => ValueAddedTaxStatus::Exempt,
            "TAXABLE" => ValueAddedTaxStatus::Taxable,
            other => panic!("unexpected vat {other}"),
        };
        let attrs = BondTaxAttributes::new(vat, IncomeTaxStatus::Exempt);
        let treatment: CouponTaxTreatment = parser
            .parse(&content, first_issue, attrs, &subject_tax)
            .unwrap();
        assert_eq!(
            format_fixed(treatment.value_added_tax_rate()),
            exp["value_added_tax_rate"].as_str().unwrap()
        );
        let gross = parse_fixed(inp["gross_coupon_per_period"].as_str().unwrap());
        let adj_coupon = treatment.adjust_coupon(gross).unwrap();
        assert_eq!(
            format_fixed(adj_coupon),
            exp["tax_adjusted_coupon_per_period"].as_str().unwrap()
        );
        assert_eq!(
            treatment.claim_scope(),
            CouponTaxClaimScope::CouponOutputVatBeforeInputCredit
        );

        // Verify live C++ BondAnalyticsEngine pre-tax & post-tax YTM on the authoritative R5E
        // 17-remaining-period semiannual schedule (2026-02-08 .. 2026-08-08 = 181 days,
        // settlement 2026-07-21 -> 18/181 fraction to next coupon, maturity 2034-08-08).
        let settlement = NaiveDate::from_ymd_opt(2026, 7, 21).unwrap();
        let maturity = NaiveDate::from_ymd_opt(2034, 8, 8).unwrap();
        let (sched_first_issue, sched_current_issue) = match inp["case_id"].as_str().unwrap() {
            "before-cutoff-exempt" => (
                NaiveDate::from_ymd_opt(2025, 2, 8).unwrap(),
                NaiveDate::from_ymd_opt(2025, 8, 8).unwrap(),
            ),
            _ => (first_issue, current_issue),
        };
        let terms = BondTerms::with_issuance(
            sched_first_issue,
            sched_current_issue,
            maturity,
            CouponFrequency::Semiannual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            parse_fixed("0.025"),
            parse_fixed("100"),
            parse_fixed("100"),
            attrs,
        )
        .unwrap();
        let owner = OwnerRef::new(id('A'), id('B'));
        let obj = AnalyticsObjectRef::new(
            VersionRef::new(id('C'), Version::new(1).unwrap()),
            ContentHash::digest(b"bond"),
        );
        let cal = CalendarBinding::new(
            id('D').as_str(),
            Version::new(1).unwrap(),
            ContentHash::digest(b"cal"),
            sched_first_issue,
            maturity,
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let val_at = MarketTime::new(
            "2026-07-21T04:00:00Z".parse().unwrap(),
            MARKET_TIMEZONE,
            settlement,
        )
        .unwrap();
        let bond_input = BondAnalyticsInput::new(
            owner,
            obj.clone(),
            obj.clone(),
            obj,
            val_at,
            settlement,
            CalendarRequirement::ReferenceReplay,
            cal,
            terms,
            AnalyticsMode::PriceIn,
            parse_fixed("100"),
        )
        .unwrap();
        let (pre_tax, after_tax) = CalculateBondAnalytics::new(&NativeBondAnalyticsEngine)
            .execute_with_tax(&bond_input, &treatment)
            .unwrap();
        assert_eq!(
            format_fixed(pre_tax.measures().yield_to_maturity()),
            exp["market_pre_tax_yield_to_maturity"].as_str().unwrap()
        );
        assert_eq!(
            format_fixed(after_tax.yield_to_maturity()),
            exp["subject_tax_adjusted_yield_to_maturity"]
                .as_str()
                .unwrap()
        );
    }

    // Verify delivery basket CTD reversal under Subject VAT treatment
    for (inp_b, exp_b) in inputs["delivery_baskets"]
        .as_array()
        .unwrap()
        .iter()
        .zip(expected["delivery_baskets"].as_array().unwrap())
    {
        let purchase = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let delivery = NaiveDate::from_ymd_opt(2026, 4, 30).unwrap(); // 60 actual days
        for (cand_in, cand_exp) in inp_b["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .zip(exp_b["candidates"].as_array().unwrap())
        {
            let first_issue: NaiveDate = cand_in["first_issue_date"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let vat = match cand_in["value_added_tax_status"].as_str().unwrap() {
                "EXEMPT" => ValueAddedTaxStatus::Exempt,
                "TAXABLE" => ValueAddedTaxStatus::Taxable,
                _ => unreachable!(),
            };
            let treatment = parser
                .parse(
                    &content,
                    first_issue,
                    BondTaxAttributes::new(vat, IncomeTaxStatus::Exempt),
                    &subject_tax,
                )
                .unwrap();
            let invoice = parse_fixed(cand_in["invoice_price"].as_str().unwrap());
            let gross_c = parse_fixed(cand_in["gross_interim_coupons"].as_str().unwrap());
            let dirty = parse_fixed(cand_in["purchase_dirty_price"].as_str().unwrap());
            let adj_c = treatment.adjust_coupon(gross_c).unwrap();
            let subj_irr = subject_delivery_irr(invoice, adj_c, dirty, purchase, delivery).unwrap();
            assert_eq!(
                format_fixed(adj_c),
                cand_exp["tax_adjusted_interim_coupons"].as_str().unwrap()
            );
            assert_eq!(
                format_fixed(subj_irr),
                cand_exp["subject_tax_adjusted_irr"].as_str().unwrap()
            );
        }
    }
}

#[test]
fn position_views_preserve_repo_penetration_and_fail_closed_on_unknown_accounting() {
    let unit = UnitRef::new(id('V'), Version::new(1).unwrap());
    let dec = |v: &str| DecimalValue::new(v, 0, unit.clone()).unwrap();
    let at = MarketTime::new(
        "2026-03-02T02:00:00Z".parse().unwrap(),
        MARKET_TIMEZONE,
        NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(),
    )
    .unwrap();
    let pos_owned = Position::new(PositionInput {
        position_id: id('1'),
        instrument_ref: VersionRef::new(id('B'), Version::new(1).unwrap()),
        quantity: dec("100"),
        economic_value: dec("10000"),
        economic_pnl: dec("50"),
        accounting_pnl: dec("0"),
        capital_requirement: dec("800"),
        accounting_classification: AccountingClassification::new(
            AccountingClassificationState::Classified,
            Some(AccountingBook::Fvoci),
        )
        .unwrap(),
        holding_form: PositionHoldingForm::RepoSold,
    })
    .unwrap();
    let pos_rev = Position::new(PositionInput {
        position_id: id('2'),
        instrument_ref: VersionRef::new(id('C'), Version::new(1).unwrap()),
        quantity: dec("50"),
        economic_value: dec("5000"),
        economic_pnl: dec("0"),
        accounting_pnl: dec("0"),
        capital_requirement: dec("100"),
        accounting_classification: AccountingClassification::new(
            AccountingClassificationState::NotApplicable,
            None,
        )
        .unwrap(),
        holding_form: PositionHoldingForm::ReverseRepoCollateral,
    })
    .unwrap();
    let mut snap_input = PositionSnapshotInput {
        snapshot_id: id('S'),
        owner: OwnerRef::new(id('T'), id('0')),
        subject_ref: VersionRef::new(id('J'), Version::new(1).unwrap()),
        observed_at: at.clone(),
        visible_at: at,
        content_hash: ContentHash::digest(b"placeholder"),
        lineage: vec![LineageRef::content_addressed(
            id('M'),
            ContentHash::digest(b"src"),
        )],
        positions: vec![pos_owned, pos_rev],
    };
    snap_input.content_hash = PositionSnapshot::content_hash_for(&snap_input);
    let snap = PositionSnapshot::new(snap_input).unwrap();

    let views = project_position_views(snap.clone()).unwrap();
    assert!(views.positions[0].included_in_position_exposure);
    assert!(!views.positions[0].included_in_available_liquidity);
    assert!(!views.positions[1].included_in_position_exposure);
    assert!(views.positions[1].collateral_fact);

    let cap = calculate_capital_use(snap).unwrap();
    assert_eq!(cap.total_capital_requirement.coefficient(), "900");
}
