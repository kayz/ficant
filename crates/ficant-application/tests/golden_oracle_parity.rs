use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use chrono::NaiveDate;
use ficant_application::ports::{CouponTaxClaimScope, CouponTaxTreatment, TaxRulePackParser};
use ficant_application::use_cases::{
    CalculateBondAnalytics, calculate_capital_use, project_position_views, subject_delivery_irr,
};
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondTaxBasis, BondTerms,
    BusinessDayConvention, CalendarBinding, CalendarRequirement, CalendarResolution,
    CouponFrequency, DECIMAL_SCALE, DayCountConvention, FixedDecimal, MARKET_TIMEZONE,
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

fn parse_iso_date(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("valid ISO-8601 date")
}

fn parse_bp1_calendar(cal_doc: &Value) -> CalendarBinding {
    let cov_start = parse_iso_date(cal_doc["coverage_start"].as_str().unwrap());
    let cov_end = parse_iso_date(cal_doc["coverage_end"].as_str().unwrap());
    assert_eq!(cov_start, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap());
    assert_eq!(cov_end, NaiveDate::from_ymd_opt(2026, 12, 31).unwrap());
    assert_eq!(cal_doc["first_session"]["local_date"], "2024-01-01");
    assert_eq!(cal_doc["first_session"]["closed"], true);
    assert_eq!(cal_doc["last_session"]["local_date"], "2026-12-31");
    assert_eq!(cal_doc["last_session"]["closed"], false);

    let non_business_days: Vec<NaiveDate> = cal_doc["non_business_days"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| parse_iso_date(entry.as_str().unwrap()))
        .collect();
    let work_weekends: Vec<NaiveDate> = cal_doc["work_weekends"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| parse_iso_date(entry.as_str().unwrap()))
        .collect();

    CalendarBinding::new(
        cal_doc["calendar_id"].as_str().unwrap(),
        Version::new(cal_doc["version"].as_u64().unwrap()).unwrap(),
        ContentHash::digest(b"bp1-cfets-2024-2026"),
        cov_start,
        cov_end,
        non_business_days,
        work_weekends,
    )
    .unwrap()
}

fn parse_bp1_terms(bond: &Value, tax_attributes: Option<BondTaxAttributes>) -> BondTerms {
    let first_issue = parse_iso_date(bond["first_issue_date"].as_str().unwrap());
    let current_issue = parse_iso_date(bond["current_issue_date"].as_str().unwrap());
    let maturity = parse_iso_date(bond["maturity_date"].as_str().unwrap());

    let cov_start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
    let cov_end = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
    assert!(
        (cov_start..=cov_end).contains(&first_issue),
        "{}: first_issue_date {first_issue} must lie in [2024-01-01, 2026-12-31]",
        bond["case_id"]
    );

    let frequency = match bond["coupon_frequency"].as_str().unwrap() {
        "ANNUAL" => CouponFrequency::Annual,
        "SEMIANNUAL" => CouponFrequency::Semiannual,
        other => panic!("unexpected coupon_frequency {other}"),
    };
    let day_count = match bond["day_count"].as_str().unwrap() {
        "ACT_ACT_BOND_ISMA" => DayCountConvention::ActActBondIsma,
        other => panic!("unexpected day_count {other}"),
    };
    let business_day = match bond["business_day"].as_str().unwrap() {
        "FOLLOWING" => BusinessDayConvention::Following,
        other => panic!("unexpected business_day {other}"),
    };
    let coupon_rate = parse_fixed(bond["coupon_rate"].as_str().unwrap());
    let face_value = parse_fixed(bond["face_value"].as_str().unwrap());
    let cumulative_issued = parse_fixed(bond["cumulative_issued_amount"].as_str().unwrap());

    BondTerms::with_issuance_and_optional_tax(
        first_issue,
        current_issue,
        maturity,
        frequency,
        day_count,
        business_day,
        coupon_rate,
        face_value,
        cumulative_issued,
        tax_attributes,
    )
    .unwrap()
}

fn build_bp1_input(
    bond: &Value,
    calendar: &CalendarBinding,
    mode: AnalyticsMode,
    tax_attributes: Option<BondTaxAttributes>,
) -> BondAnalyticsInput {
    let settlement = parse_iso_date(bond["settlement_date"].as_str().unwrap());
    let requirement = match bond["calendar_requirement"].as_str().unwrap() {
        "REFERENCE_REPLAY" => CalendarRequirement::ReferenceReplay,
        "EXACT_MARKET" => CalendarRequirement::ExactMarket,
        other => panic!("unexpected calendar_requirement {other}"),
    };
    let terms = parse_bp1_terms(bond, tax_attributes);
    let input_value = match mode {
        AnalyticsMode::YieldIn => parse_fixed(bond["yield_in_input"].as_str().unwrap()),
        AnalyticsMode::PriceIn => parse_fixed(bond["price_in_input"].as_str().unwrap()),
    };

    let owner = OwnerRef::new(id('A'), id('B'));
    let obj = AnalyticsObjectRef::new(
        VersionRef::new(id('C'), Version::new(1).unwrap()),
        ContentHash::digest(bond["case_id"].as_str().unwrap().as_bytes()),
    );
    let val_at = MarketTime::new(
        format!("{settlement}T02:00:00Z").parse().unwrap(),
        MARKET_TIMEZONE,
        settlement,
    )
    .unwrap();

    BondAnalyticsInput::new(
        owner,
        obj.clone(),
        obj.clone(),
        obj,
        val_at,
        settlement,
        requirement,
        calendar.clone(),
        terms,
        mode,
        input_value,
    )
    .unwrap()
}

fn assert_scaled_within(label: &str, actual: FixedDecimal, expected: FixedDecimal, max_diff: u128) {
    let diff = actual.scaled().abs_diff(expected.scaled());
    assert!(
        diff <= max_diff,
        "{label}: actual={} expected={} diff={diff} > max_diff={max_diff}",
        format_fixed(actual),
        format_fixed(expected),
    );
}

fn risk_measure_tolerance(expected: FixedDecimal) -> u128 {
    expected
        .scaled()
        .unsigned_abs()
        .div_ceil(100_000_000)
        .max(100)
}

#[test]
#[allow(clippy::too_many_lines)]
fn bp1_policy_bank_pretax_twelve_case_native_parity_and_tax_attribute_invariance() {
    let root = repo_root();
    let inputs: Value = serde_json::from_str(
        &fs::read_to_string(
            root.join("tests/golden-cases/china-rates/bp1-policy-bank-pretax-inputs.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let expected: Value =
        serde_json::from_str(
            &fs::read_to_string(root.join(
                "tests/golden-cases/china-rates/expected/bp1-policy-bank-pretax-expected.json",
            ))
            .unwrap(),
        )
        .unwrap();

    let calendar = parse_bp1_calendar(&inputs["calendar"]);
    let bonds = inputs["bonds"].as_array().unwrap();
    assert_eq!(bonds.len(), 6);

    let expected_cases = expected["case_results"].as_object().unwrap();
    assert_eq!(expected_cases.len(), 12);

    let use_case = CalculateBondAnalytics::new(&NativeBondAnalyticsEngine);
    let mut seen_keys = BTreeSet::new();

    for bond in bonds {
        let case_id = bond["case_id"].as_str().unwrap();
        assert_eq!(bond["execution_mode"].as_str().unwrap(), "PRE_TAX");

        let is_exact_short = case_id == "ADBC-1Y-EXACT-ANNUAL";
        let expected_requirement = if is_exact_short {
            CalendarRequirement::ExactMarket
        } else {
            CalendarRequirement::ReferenceReplay
        };
        let expected_resolution = if is_exact_short {
            CalendarResolution::Exact
        } else {
            CalendarResolution::ProvisionalWeekendOnly
        };

        for (mode, mode_label) in [
            (AnalyticsMode::YieldIn, "YIELD_IN"),
            (AnalyticsMode::PriceIn, "PRICE_IN"),
        ] {
            let key = format!("{case_id}:{mode_label}");
            assert!(seen_keys.insert(key.clone()), "duplicate case key {key}");

            let exp = expected_cases
                .get(&key)
                .unwrap_or_else(|| panic!("missing expected entry for {key}"));
            assert_eq!(exp["execution_mode"].as_str().unwrap(), "PRE_TAX");
            assert_eq!(exp["measures_tax_basis"].as_str().unwrap(), "PRE_TAX");
            assert!(exp["after_tax"].is_null());

            let input = build_bp1_input(bond, &calendar, mode, None);
            assert_eq!(input.calendar_requirement(), expected_requirement);

            let (result, after_tax) = use_case
                .execute_for_mode(&input, BondTaxBasis::PreTax, None)
                .unwrap();
            assert!(after_tax.is_none());
            assert_eq!(result.tax_basis(), BondTaxBasis::PreTax);
            assert_eq!(result.calendar_resolution(), expected_resolution);

            if is_exact_short {
                assert_eq!(result.cashflows().len(), 1);
                assert_eq!(
                    result.cashflows()[0].nominal_date(),
                    NaiveDate::from_ymd_opt(2026, 11, 15).unwrap()
                );
                assert_eq!(
                    result.cashflows()[0].payment_date(),
                    NaiveDate::from_ymd_opt(2026, 11, 16).unwrap()
                );
            }
            if case_id == "EIBC-3Y-SEMIANNUAL" {
                assert_eq!(
                    result.cashflows()[0].nominal_date(),
                    NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
                );
                assert_eq!(
                    result.cashflows()[0].payment_date(),
                    NaiveDate::from_ymd_opt(2026, 10, 8).unwrap()
                );
            }

            let exp_cfs = exp["cashflows"].as_array().unwrap();
            assert_eq!(
                result.cashflows().len(),
                exp_cfs.len(),
                "{key}: cashflow count mismatch"
            );
            for (actual_cf, exp_cf) in result.cashflows().iter().zip(exp_cfs) {
                assert_eq!(
                    u64::from(actual_cf.sequence()),
                    exp_cf["sequence"].as_u64().unwrap(),
                    "{key}: cashflow sequence mismatch"
                );
                assert_eq!(
                    actual_cf.nominal_date(),
                    parse_iso_date(exp_cf["nominal_date"].as_str().unwrap()),
                    "{key}: cashflow nominal_date mismatch"
                );
                assert_eq!(
                    actual_cf.payment_date(),
                    parse_iso_date(exp_cf["payment_date"].as_str().unwrap()),
                    "{key}: cashflow payment_date mismatch"
                );
                assert_eq!(
                    actual_cf.coupon(),
                    parse_fixed(exp_cf["coupon"].as_str().unwrap()),
                    "{key}: cashflow coupon exact mismatch"
                );
                assert_eq!(
                    actual_cf.principal(),
                    parse_fixed(exp_cf["principal"].as_str().unwrap()),
                    "{key}: cashflow principal exact mismatch"
                );
                assert_eq!(
                    actual_cf.total(),
                    parse_fixed(exp_cf["total"].as_str().unwrap()),
                    "{key}: cashflow total exact mismatch"
                );
            }

            let exp_m = &exp["measures"];
            let act_m = result.measures();
            assert_scaled_within(
                &format!("{key}.accrued_interest"),
                act_m.accrued_interest(),
                parse_fixed(exp_m["accrued_interest"].as_str().unwrap()),
                10_000,
            );
            assert_scaled_within(
                &format!("{key}.clean_price"),
                act_m.clean_price(),
                parse_fixed(exp_m["clean_price"].as_str().unwrap()),
                10_000,
            );
            assert_scaled_within(
                &format!("{key}.dirty_price"),
                act_m.dirty_price(),
                parse_fixed(exp_m["dirty_price"].as_str().unwrap()),
                10_000,
            );
            assert_scaled_within(
                &format!("{key}.yield_to_maturity"),
                act_m.yield_to_maturity(),
                parse_fixed(exp_m["yield_to_maturity"].as_str().unwrap()),
                100,
            );
            let exp_mac = parse_fixed(exp_m["macaulay_duration"].as_str().unwrap());
            assert_scaled_within(
                &format!("{key}.macaulay_duration"),
                act_m.macaulay_duration(),
                exp_mac,
                risk_measure_tolerance(exp_mac),
            );
            let exp_mod = parse_fixed(exp_m["modified_duration"].as_str().unwrap());
            assert_scaled_within(
                &format!("{key}.modified_duration"),
                act_m.modified_duration(),
                exp_mod,
                risk_measure_tolerance(exp_mod),
            );
            let exp_conv = parse_fixed(exp_m["convexity"].as_str().unwrap());
            assert_scaled_within(
                &format!("{key}.convexity"),
                act_m.convexity(),
                exp_conv,
                risk_measure_tolerance(exp_conv),
            );
            assert_scaled_within(
                &format!("{key}.dv01"),
                act_m.dv01(),
                parse_fixed(exp_m["dv01"].as_str().unwrap()),
                10_000,
            );
        }
    }

    let expected_keys: BTreeSet<String> = expected_cases.keys().cloned().collect();
    assert_eq!(seen_keys.len(), 12);
    assert_eq!(seen_keys, expected_keys);

    // ADBC-7Y-SEMIANNUAL: fixed re-issuance facts and exact 0-diff parity across exempt/taxable/absent tax attributes under PRE_TAX.
    let adbc7 = bonds
        .iter()
        .find(|b| b["case_id"] == "ADBC-7Y-SEMIANNUAL")
        .expect("ADBC-7Y-SEMIANNUAL present");
    let exempt_attrs = Some(BondTaxAttributes::new(
        ValueAddedTaxStatus::Exempt,
        IncomeTaxStatus::Exempt,
    ));
    let taxable_attrs = Some(BondTaxAttributes::new(
        ValueAddedTaxStatus::Taxable,
        IncomeTaxStatus::Exempt,
    ));

    for mode in [AnalyticsMode::YieldIn, AnalyticsMode::PriceIn] {
        let inp_exempt = build_bp1_input(adbc7, &calendar, mode, exempt_attrs);
        let inp_taxable = build_bp1_input(adbc7, &calendar, mode, taxable_attrs);
        let inp_absent = build_bp1_input(adbc7, &calendar, mode, None);

        let (res_exempt, after_exempt) = use_case
            .execute_for_mode(&inp_exempt, BondTaxBasis::PreTax, None)
            .unwrap();
        let (res_taxable, after_taxable) = use_case
            .execute_for_mode(&inp_taxable, BondTaxBasis::PreTax, None)
            .unwrap();
        let (res_absent, after_absent) = use_case
            .execute_for_mode(&inp_absent, BondTaxBasis::PreTax, None)
            .unwrap();

        assert!(after_exempt.is_none() && after_taxable.is_none() && after_absent.is_none());

        for (res, expected_attrs) in [
            (&res_exempt, exempt_attrs),
            (&res_taxable, taxable_attrs),
            (&res_absent, None),
        ] {
            let terms = res.input().terms();
            assert_eq!(
                terms.first_issue_date(),
                NaiveDate::from_ymd_opt(2024, 8, 18).unwrap()
            );
            assert_eq!(
                terms.current_issue_date(),
                NaiveDate::from_ymd_opt(2025, 2, 18).unwrap()
            );
            assert_eq!(
                terms.maturity_date(),
                NaiveDate::from_ymd_opt(2031, 8, 18).unwrap()
            );
            assert_eq!(
                terms.cumulative_issued_amount(),
                parse_fixed("25000000000.000000000000")
            );
            assert_ne!(terms.cumulative_issued_amount(), terms.face_amount());
            assert_eq!(terms.tax_attributes(), expected_attrs);
            assert_eq!(res.tax_basis(), BondTaxBasis::PreTax);
            assert_eq!(
                res.calendar_resolution(),
                CalendarResolution::ProvisionalWeekendOnly
            );
        }

        // Exact 0-diff bitwise equality across all three tax-attribute configurations.
        assert_eq!(res_exempt.cashflows(), res_taxable.cashflows());
        assert_eq!(res_exempt.cashflows(), res_absent.cashflows());
        assert_eq!(res_exempt.measures(), res_taxable.measures());
        assert_eq!(res_exempt.measures(), res_absent.measures());
    }
}
