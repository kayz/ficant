use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::NaiveDate;
use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    BondAnalyticsEngine, CouponTaxClaimScope, CouponTaxTreatment, TaxRulePackParser,
};
use ficant_application::use_cases::CalculateBondAnalytics;
use ficant_domain::analytics::{
    AnalyticsError, AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondAnalyticsResult,
    BondTaxBasis, BondTerms, BusinessDayConvention, CalendarBinding, CalendarRequirement,
    CouponFrequency, DayCountConvention, FixedDecimal, MARKET_TIMEZONE,
};
use ficant_domain::market::{
    BondTaxAttributes, IncomeTaxStatus, RulePackContent, ValueAddedTaxStatus,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_domain::subject::TaxTreatment;
use ficant_fixed_income_native::NativeBondAnalyticsEngine;
use ficant_tax_pack::{TYPE_URL_V2, TaxRulePackV2Parser};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn scaled(units: i128, fractional_1e12: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(units * 1_000_000_000_000 + fractional_1e12)
}

fn taxable_treatment() -> CouponTaxTreatment {
    let raw = fs::read(repo_root().join("domain-packs/cgb-interest-tax/cgb-interest-tax-v1.bin"))
        .expect("authoritative tax pack exists");
    let content = RulePackContent::new(TYPE_URL_V2, raw).unwrap();
    let attrs = BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Exempt);
    let subject =
        TaxTreatment::new("cn-vat-general-taxpayer", "cn-cgb-interest-cit-exempt").unwrap();
    TaxRulePackV2Parser
        .parse(
            &content,
            NaiveDate::from_ymd_opt(2025, 8, 8).unwrap(),
            attrs,
            &subject,
        )
        .expect("taxable treatment resolves")
}

fn sample_pre_tax_input() -> BondAnalyticsInput {
    let first_issue = NaiveDate::from_ymd_opt(2025, 8, 8).unwrap();
    let current_issue = NaiveDate::from_ymd_opt(2026, 2, 8).unwrap();
    let maturity = NaiveDate::from_ymd_opt(2034, 8, 8).unwrap();
    let settlement = NaiveDate::from_ymd_opt(2026, 7, 21).unwrap();
    let attrs = BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Exempt);
    let terms = BondTerms::with_issuance(
        first_issue,
        current_issue,
        maturity,
        CouponFrequency::Semiannual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        scaled(0, 25_000_000_000),
        scaled(100, 0),
        scaled(50_000_000_000, 0),
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
        first_issue,
        maturity,
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let valuation_at = MarketTime::new(
        "2026-07-21T04:00:00Z".parse().unwrap(),
        MARKET_TIMEZONE,
        settlement,
    )
    .unwrap();
    BondAnalyticsInput::new(
        owner,
        obj.clone(),
        obj.clone(),
        obj,
        valuation_at,
        settlement,
        CalendarRequirement::ReferenceReplay,
        cal,
        terms,
        AnalyticsMode::YieldIn,
        scaled(0, 25_000_000_000),
    )
    .unwrap()
}

struct RecordingBondEngine {
    calls: Mutex<Vec<BondAnalyticsInput>>,
}

impl RecordingBondEngine {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }

    fn recorded(&self) -> Vec<BondAnalyticsInput> {
        self.calls.lock().unwrap().clone()
    }
}

impl BondAnalyticsEngine for RecordingBondEngine {
    fn calculate(&self, input: &BondAnalyticsInput) -> Result<BondAnalyticsResult, AnalyticsError> {
        self.calls.lock().unwrap().push(input.clone());
        NativeBondAnalyticsEngine.calculate(input)
    }
}

struct MisbindingBondEngine {
    override_round: usize,
    replacement_input: BondAnalyticsInput,
    calls: Mutex<Vec<BondAnalyticsInput>>,
}

impl MisbindingBondEngine {
    fn new(override_round: usize, replacement_input: BondAnalyticsInput) -> Self {
        Self {
            override_round,
            replacement_input,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl BondAnalyticsEngine for MisbindingBondEngine {
    fn calculate(&self, input: &BondAnalyticsInput) -> Result<BondAnalyticsResult, AnalyticsError> {
        let mut guard = self.calls.lock().unwrap();
        let round = guard.len();
        guard.push(input.clone());
        drop(guard);
        let target = if round == self.override_round {
            &self.replacement_input
        } else {
            input
        };
        NativeBondAnalyticsEngine.calculate(target)
    }
}

#[test]
fn public_execute_and_execute_with_tax_reject_tax_adjusted_input_without_calling_engine() {
    let pre_tax_input = sample_pre_tax_input();
    let tax_adjusted_input = pre_tax_input
        .with_terms_and_price_in(pre_tax_input.terms().clone(), scaled(100, 0))
        .unwrap();
    assert_eq!(tax_adjusted_input.tax_basis(), BondTaxBasis::TaxAdjusted);

    let engine = RecordingBondEngine::new();
    let calc = CalculateBondAnalytics::new(&engine);

    let err = calc
        .execute(&tax_adjusted_input)
        .expect_err("execute must reject TaxAdjusted input");
    assert_eq!(err.category(), ApplicationErrorCategory::ValidationFailed);
    assert!(engine.recorded().is_empty());

    let treatment = taxable_treatment();
    let err = calc
        .execute_with_tax(&tax_adjusted_input, &treatment)
        .expect_err("execute_with_tax must reject TaxAdjusted round-1 input");
    assert_eq!(err.category(), ApplicationErrorCategory::ValidationFailed);
    assert!(engine.recorded().is_empty());
}

#[test]
fn execute_with_tax_runs_pre_tax_then_direct_engine_after_tax_with_net_coupon_and_clean_price() {
    let input = sample_pre_tax_input();
    let treatment = taxable_treatment();
    let engine = RecordingBondEngine::new();
    let calc = CalculateBondAnalytics::new(&engine);

    let (pre_tax, after_tax) = calc
        .execute_with_tax(&input, &treatment)
        .expect("two-round bond analytics succeeds");

    let recorded = engine.recorded();
    assert_eq!(recorded.len(), 2);

    // Round 1: exact PreTax input
    assert_eq!(recorded[0], input);
    assert_eq!(recorded[0].tax_basis(), BondTaxBasis::PreTax);
    assert_eq!(pre_tax.tax_basis(), BondTaxBasis::PreTax);

    // Round 2: direct engine call with net coupon, round-1 clean price, PriceIn, and TaxAdjusted
    let expected_net_coupon = treatment
        .adjust_coupon(pre_tax.cashflows()[0].coupon())
        .unwrap();
    let expected_after_tax_rate = expected_net_coupon
        .checked_mul_integer(2)
        .and_then(|v| v.checked_div_round_ties_even(input.terms().face_amount()))
        .unwrap();

    assert_eq!(recorded[1].tax_basis(), BondTaxBasis::TaxAdjusted);
    assert_eq!(recorded[1].mode(), AnalyticsMode::PriceIn);
    assert_eq!(recorded[1].input_value(), pre_tax.measures().clean_price());
    assert_eq!(recorded[1].terms().coupon_rate(), expected_after_tax_rate);
    assert_eq!(
        recorded[1].terms().first_issue_date(),
        input.terms().first_issue_date()
    );
    assert_eq!(
        recorded[1].terms().current_issue_date(),
        input.terms().current_issue_date()
    );
    assert_eq!(
        recorded[1].terms().cumulative_issued_amount(),
        input.terms().cumulative_issued_amount()
    );
    assert_eq!(
        recorded[1].terms().tax_attributes(),
        input.terms().tax_attributes()
    );
    assert_eq!(recorded[1].settlement_date(), input.settlement_date());
    assert_eq!(recorded[1].calendar(), input.calendar());

    assert_eq!(after_tax.tax_basis(), BondTaxBasis::TaxAdjusted);
    assert_eq!(
        after_tax.claim_scope(),
        CouponTaxClaimScope::CouponOutputVatBeforeInputCredit
    );
    assert!(after_tax.yield_to_maturity() < pre_tax.measures().yield_to_maturity());
}

#[test]
fn execute_for_mode_enforces_mode_and_treatment_pairing() {
    let input = sample_pre_tax_input();
    let treatment = taxable_treatment();
    let engine = RecordingBondEngine::new();
    let calc = CalculateBondAnalytics::new(&engine);

    let err = calc
        .execute_for_mode(&input, BondTaxBasis::PreTax, Some(&treatment))
        .expect_err("PreTax mode must reject treatment");
    assert_eq!(err.category(), ApplicationErrorCategory::ValidationFailed);
    assert!(engine.recorded().is_empty());

    let err = calc
        .execute_for_mode(&input, BondTaxBasis::TaxAdjusted, None)
        .expect_err("TaxAdjusted mode must require treatment");
    assert_eq!(err.category(), ApplicationErrorCategory::ValidationFailed);
    assert!(engine.recorded().is_empty());

    let (pre_tax, none_after_tax) = calc
        .execute_for_mode(&input, BondTaxBasis::PreTax, None)
        .expect("PreTax mode succeeds without treatment");
    assert_eq!(pre_tax.tax_basis(), BondTaxBasis::PreTax);
    assert!(none_after_tax.is_none());
    assert_eq!(engine.recorded().len(), 1);

    let (pre_tax_2, some_after_tax) = calc
        .execute_for_mode(&input, BondTaxBasis::TaxAdjusted, Some(&treatment))
        .expect("TaxAdjusted mode succeeds with treatment");
    assert_eq!(pre_tax_2.tax_basis(), BondTaxBasis::PreTax);
    let after_tax = some_after_tax.expect("after_tax present");
    assert_eq!(after_tax.tax_basis(), BondTaxBasis::TaxAdjusted);
    assert_eq!(engine.recorded().len(), 3);
}

#[test]
fn execute_and_execute_with_tax_reject_misbound_or_wrong_tax_basis_engine_results() {
    let input = sample_pre_tax_input();
    let treatment = taxable_treatment();

    // Round 1 returns a TaxAdjusted result instead of PreTax result.
    let tax_adjusted_wrong_basis = input
        .with_terms_and_price_in(input.terms().clone(), scaled(100, 0))
        .unwrap();
    let bad_round1_engine = MisbindingBondEngine::new(0, tax_adjusted_wrong_basis);
    let err = CalculateBondAnalytics::new(&bad_round1_engine)
        .execute(&input)
        .expect_err("round-1 wrong tax basis / lineage must be rejected");
    assert_eq!(err.category(), ApplicationErrorCategory::LineageIncomplete);

    // Round 2 returns a PreTax result bound to the round-1 input.
    let bad_round2_pretax = MisbindingBondEngine::new(1, input.clone());
    let err = CalculateBondAnalytics::new(&bad_round2_pretax)
        .execute_with_tax(&input, &treatment)
        .expect_err("round-2 PreTax result must be rejected");
    assert_eq!(err.category(), ApplicationErrorCategory::LineageIncomplete);

    // Round 2 returns a TaxAdjusted result bound to a different clean price.
    let wrong_price_tax_adjusted = input
        .with_terms_and_price_in(input.terms().clone(), scaled(99, 0))
        .unwrap();
    let bad_round2_lineage = MisbindingBondEngine::new(1, wrong_price_tax_adjusted);
    let err = CalculateBondAnalytics::new(&bad_round2_lineage)
        .execute_with_tax(&input, &treatment)
        .expect_err("round-2 misbound TaxAdjusted result must be rejected");
    assert_eq!(err.category(), ApplicationErrorCategory::LineageIncomplete);
}
