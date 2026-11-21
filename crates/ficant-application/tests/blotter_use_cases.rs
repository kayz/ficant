#![allow(clippy::too_many_lines, clippy::similar_names)]

use std::collections::BTreeMap;

use chrono::{NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Asia::Shanghai;
use ficant_application::ports::{
    ApplicationResult, FuturesDeliveryRuleParser, MAX_SPI_PAGE_SIZE, MIN_SPI_PAGE_SIZE,
    ManualBlotterClock, RegulatoryRulePackParser, TradeFeedReconcilerPort, validate_spi_page_size,
};
use ficant_application::use_cases::blotter::{
    AccountDeskPolicy, AnalysisUnits, BlotterEvaluationEngines, CalendarResolutionMode,
    ConfirmDraftEarmarkCommand, CoverageStatus, DeskBlotterOrchestrator, DeskBlotterWorkspaceState,
    DraftEarmarkStatus, DraftTargetState, FullOpeningPositionBase, ImportFullOpeningBaseCommand,
    IngestMarketQuotesInput, InstrumentCatalogEntry, MarketQuoteTick, NlpEarmarkProposal,
    PreviewCoveredHedgeInput, PreviewHedgeScope, PullTradeFeedBatchResult, RateCoverageEligibility,
    SourceTextAnchor, SubmitDraftEarmarkCommand, SwitchActiveQuoteSourceCommand, UncoveredReason,
    pending_missing_trade_revisions, price_original_face_pre_tax_bond,
    resolve_effective_calendar_requirement,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, ApplicationErrorDetail};
use ficant_domain::analytics::{
    AnalyticsMode, BondTaxBasis, CalendarBinding, CalendarRequirement, FixedDecimal,
    MARKET_TIMEZONE,
};
use ficant_domain::blotter::{
    BaseEvidence, BlotterConfig, ConfirmHoldCommand, CreateCommittedCommand, CreateHoldCommand,
    DeterministicIdGenerator, EarmarkState, ExtendHoldCommand, FundingPrecision,
    IngestTradeFeedInput, OpeningInstrumentBalance, OpeningPositionBase, PendingMatchReason,
    PendingMatchResolutionAction, PendingMatchState, QuoteBasis, QuoteTerms,
    ResolvePendingMatchInput, SetFeedControlCommand, SettlementBucket, SweepLifecycleCommand,
    TradeFeedRecordAction, TradeFillRecord, TradeSide, wide_mul_div_fixed_round_half_even,
};
use ficant_domain::constraint::{ConstraintDefinition, ConstraintMeasure, ConstraintSet};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliveryRule, FuturesDeliveryRuleInput,
};
use ficant_domain::market::{
    Bond, BondBusinessDayConvention, BondCouponFrequency, BondDayCountConvention, BondPricingTerms,
    BondTaxAttributes, IncomeTaxStatus, Instrument, InstrumentInput, InstrumentKind,
    MarketRulePack, MarketRulePackInput, PriceSourceType, RulePackContent, ValueAddedTaxStatus,
    VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, MarketTime, OwnerRef, Ulid, UnitRef, Version,
    VersionRef,
};
use ficant_fixed_income_native::{NativeBondAnalyticsEngine, NativeFuturesHedgeEngine};

const TEST_REG_MARKET: &str = "TEST-REG-MKT";
const TEST_REG_TYPE: &str = "regulatory";
const TEST_REG_TYPE_URL: &str = "type.googleapis.com/ficant.test.RegulatoryRulePack";

const TEST_DELIVERY_MARKET: &str = "TEST-FUT-MKT";
const TEST_DELIVERY_TYPE: &str = "cgb-futures";
const TEST_DELIVERY_TYPE_URL: &str = "type.googleapis.com/ficant.test.DeliveryRulePack";

fn ulid(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F{suffix}0")).unwrap()
}

fn ulid_pair(a: char, b: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F{a}{b}")).unwrap()
}

fn fixed(units: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(units * 1_000_000_000_000)
}

fn fixed_frac(units: i128, frac_1e12: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(units * 1_000_000_000_000 + frac_1e12)
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn shanghai_time(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> MarketTime {
    let dt = date(year, month, day)
        .and_hms_opt(hour, minute, second)
        .unwrap();
    let utc = Shanghai.from_local_datetime(&dt).single().unwrap().to_utc();
    MarketTime::new(utc, MARKET_TIMEZONE, date(year, month, day)).unwrap()
}

fn test_units() -> AnalysisUnits {
    let v1 = Version::new(1).unwrap();
    AnalysisUnits {
        currency_amount: UnitRef::new(ulid_pair('A', '1'), v1),
        price_per_100: UnitRef::new(ulid_pair('A', '2'), v1),
        rate: UnitRef::new(ulid_pair('A', '3'), v1),
        dv01: UnitRef::new(ulid_pair('A', '4'), v1),
        dv01_per_100: UnitRef::new(ulid_pair('A', '5'), v1),
        dimensionless: UnitRef::new(ulid_pair('A', '6'), v1),
        contract_count: UnitRef::new(ulid_pair('A', '7'), v1),
    }
}

fn test_owner() -> OwnerRef {
    OwnerRef::new(ulid_pair('B', '1'), ulid_pair('B', '2'))
}

fn make_bond(
    id_suffix: char,
    code: &str,
    issue: NaiveDate,
    maturity: NaiveDate,
    coupon_bps: u32,
    face_units: u32,
    units: &AnalysisUnits,
) -> Bond {
    let inst = Instrument::new(InstrumentInput {
        instrument_id: ulid_pair('C', id_suffix),
        version: Version::new(1).unwrap(),
        owner: test_owner(),
        symbol: code.to_owned(),
        kind: InstrumentKind::Bond,
        market: "TEST-BOND-MKT".to_owned(),
        currency: units.currency_amount.clone(),
        calendar: VersionRef::new(ulid_pair('D', '1'), Version::new(1).unwrap()),
    })
    .unwrap();
    let face_val =
        DecimalValue::new(face_units.to_string(), 0, units.currency_amount.clone()).unwrap();
    let issued = DecimalValue::new("50000000000", 0, units.currency_amount.clone()).unwrap();
    let attrs = BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt);
    let coupon = DecimalValue::new(coupon_bps.to_string(), 4, units.rate.clone()).unwrap();
    let pricing = BondPricingTerms::new(
        coupon,
        BondCouponFrequency::Semiannual,
        BondDayCountConvention::ActActBondIsma,
        BondBusinessDayConvention::Following,
    )
    .unwrap();
    Bond::with_issuance(&inst, issue, issue, maturity, issued, attrs, face_val)
        .unwrap()
        .with_pricing_terms(pricing)
        .unwrap()
}

fn make_calendar(start: NaiveDate, end: NaiveDate) -> CalendarBinding {
    CalendarBinding::new(
        ulid_pair('D', '1').as_str(),
        Version::new(1).unwrap(),
        ContentHash::digest(format!("cal:{start}:{end}").as_bytes()),
        start,
        end,
        Vec::new(),
        Vec::new(),
    )
    .unwrap()
}

/// Deterministic JSON-backed regulatory rule parser for L2 tests.
struct DeterministicRegulatoryParser;

impl RegulatoryRulePackParser for DeterministicRegulatoryParser {
    fn market(&self) -> &'static str {
        TEST_REG_MARKET
    }

    fn rule_type(&self) -> &'static str {
        TEST_REG_TYPE
    }

    fn type_url(&self) -> &'static str {
        TEST_REG_TYPE_URL
    }

    fn parse(
        &self,
        content: &RulePackContent,
        rule_pack: VersionRef,
    ) -> ApplicationResult<ConstraintSet> {
        if content.type_url() != TEST_REG_TYPE_URL {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::ValidationFailed,
                false,
            ));
        }
        let parsed: serde_json::Value = serde_json::from_slice(content.value()).map_err(|_| {
            ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
        })?;
        let items = parsed
            .get("constraints")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                ApplicationError::rule_pack_item_missing(
                    "context.regulatory_rule_pack.content.constraints",
                )
            })?;
        if items.is_empty() {
            return Err(ApplicationError::rule_pack_item_missing(
                "context.regulatory_rule_pack.content.constraints",
            ));
        }
        let mut defs = Vec::with_capacity(items.len());
        for item in items {
            let code = item
                .get("limit_code")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
                })?;
            let measure = match item.get("measure").and_then(|v| v.as_str()) {
                Some("INSTRUMENT_FACE") => ConstraintMeasure::InstrumentFace,
                Some("PORTFOLIO_FACE") => ConstraintMeasure::PortfolioFace,
                Some("BORROWED_CASH") => ConstraintMeasure::BorrowedCash,
                _ => {
                    return Err(ApplicationError::new(
                        ApplicationErrorCategory::ValidationFailed,
                        false,
                    ));
                }
            };
            defs.push(ConstraintDefinition::new(code, measure).map_err(|_| {
                ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
            })?);
        }
        ConstraintSet::new(rule_pack, defs)
            .map_err(|_| ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false))
    }
}

/// Deterministic JSON-backed delivery rule parser for L2 per-100 hedge tests.
struct DeterministicDeliveryParser;

impl FuturesDeliveryRuleParser for DeterministicDeliveryParser {
    fn market(&self) -> &'static str {
        TEST_DELIVERY_MARKET
    }

    fn rule_type(&self) -> &'static str {
        TEST_DELIVERY_TYPE
    }

    fn type_url(&self) -> &'static str {
        TEST_DELIVERY_TYPE_URL
    }

    fn parse_product_code(&self, product_code: &str) -> ApplicationResult<CgbFuturesProduct> {
        match product_code {
            "TS" => Ok(CgbFuturesProduct::TwoYear),
            "TF" => Ok(CgbFuturesProduct::FiveYear),
            "T" => Ok(CgbFuturesProduct::TenYear),
            "TL" => Ok(CgbFuturesProduct::ThirtyYear),
            _ => Err(ApplicationError::new(
                ApplicationErrorCategory::ValidationFailed,
                false,
            )),
        }
    }

    fn parse(
        &self,
        content: &RulePackContent,
        product: CgbFuturesProduct,
    ) -> ApplicationResult<FuturesDeliveryRule> {
        self.parse_for_per_100_hedge(content, product)
    }

    fn parse_for_per_100_hedge(
        &self,
        content: &RulePackContent,
        product: CgbFuturesProduct,
    ) -> ApplicationResult<FuturesDeliveryRule> {
        if content.type_url() != TEST_DELIVERY_TYPE_URL {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::ValidationFailed,
                false,
            ));
        }
        let parsed: serde_json::Value = serde_json::from_slice(content.value()).map_err(|_| {
            ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
        })?;
        let quote_units = parsed
            .get("products")
            .and_then(|p| p.get(product.code()))
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| {
                ApplicationError::rule_pack_item_missing(format!(
                    "context.rule_pack.content.products[product_code={}].contract_size_in_quote_units",
                    product.code()
                ))
            })?;
        let rule = FuturesDeliveryRule::new(FuturesDeliveryRuleInput {
            original_term_max_months: 120,
            residual_min_months: 48,
            residual_max_months: Some(123),
            delivery_months: vec![3, 6, 9, 12],
            nominal_coupon: fixed_frac(0, 30_000_000_000),
            face_quote_basis: fixed(100),
            accrued_interest_day_count: 365,
            conversion_factor_rounding_places: 4,
            accrued_interest_rounding_places: 7,
            annual_day_basis: 365,
        })
        .map_err(|_| ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false))?;
        rule.with_contract_size_in_quote_units(quote_units)
            .map_err(|_| ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false))
    }
}

fn make_regulatory_pack(
    version: u64,
    constraints_json: &serde_json::Value,
    status: VerificationStatus,
    from: MarketTime,
    to: MarketTime,
) -> MarketRulePack {
    let bytes = serde_json::to_vec(constraints_json).unwrap();
    let hash = ContentHash::digest(&bytes);
    let content = RulePackContent::new(TEST_REG_TYPE_URL, bytes).unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: ulid_pair('E', '1'),
            version: Version::new(version).unwrap(),
            owner: test_owner(),
            market: TEST_REG_MARKET.to_owned(),
            rule_type: TEST_REG_TYPE.to_owned(),
            source: "desk-compliance".to_owned(),
            effective: EffectivePeriod::new(from, to).unwrap(),
            verification_status: status,
            content_hash: hash,
        },
        content,
    )
    .unwrap()
}

fn default_regulatory_pack() -> MarketRulePack {
    make_regulatory_pack(
        1,
        &serde_json::json!({
            "constraints": [
                {"limit_code": "INST_MAX", "measure": "INSTRUMENT_FACE"},
                {"limit_code": "PORT_MAX", "measure": "PORTFOLIO_FACE"},
                {"limit_code": "BORROW_MAX", "measure": "BORROWED_CASH"}
            ]
        }),
        VerificationStatus::Verified,
        shanghai_time(2026, 1, 1, 0, 0, 0),
        shanghai_time(2027, 1, 1, 0, 0, 0),
    )
}

fn default_delivery_pack() -> MarketRulePack {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "products": {
            "TS": 20000,
            "TF": 10000,
            "T": 10000,
            "TL": 10000
        }
    }))
    .unwrap();
    let hash = ContentHash::digest(&bytes);
    let content = RulePackContent::new(TEST_DELIVERY_TYPE_URL, bytes).unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: ulid_pair('F', '1'),
            version: Version::new(2).unwrap(),
            owner: test_owner(),
            market: TEST_DELIVERY_MARKET.to_owned(),
            rule_type: TEST_DELIVERY_TYPE.to_owned(),
            source: "exchange-delivery".to_owned(),
            effective: EffectivePeriod::new(
                shanghai_time(2026, 1, 1, 0, 0, 0),
                shanghai_time(2027, 1, 1, 0, 0, 0),
            )
            .unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: hash,
        },
        content,
    )
    .unwrap()
}

fn default_policy(allowed_products: Vec<&str>) -> AccountDeskPolicy {
    let mut ceilings = BTreeMap::new();
    ceilings.insert("INST_MAX".to_owned(), fixed(200_000_000));
    ceilings.insert("PORT_MAX".to_owned(), fixed(500_000_000));
    ceilings.insert("BORROW_MAX".to_owned(), fixed(300_000_000));
    AccountDeskPolicy {
        policy_version: 1,
        default_quote_source_id: "q-src-a".to_owned(),
        instrument_quote_sources: BTreeMap::new(),
        quote_staleness_seconds: 300,
        default_draft_hold_ttl_seconds: 1_800,
        allowed_hedge_product_codes: allowed_products.into_iter().map(str::to_owned).collect(),
        regulatory_rule_pack: Some(default_regulatory_pack()),
        limit_ceilings: ceilings,
    }
}

fn engines() -> BlotterEvaluationEngines<'static> {
    static BOND: NativeBondAnalyticsEngine = NativeBondAnalyticsEngine;
    static HEDGE: NativeFuturesHedgeEngine = NativeFuturesHedgeEngine;
    static DELIVERY: DeterministicDeliveryParser = DeterministicDeliveryParser;
    static REG: DeterministicRegulatoryParser = DeterministicRegulatoryParser;
    BlotterEvaluationEngines {
        bond_engine: &BOND,
        hedge_engine: &HEDGE,
        delivery_rule_parser: &DELIVERY,
        regulatory_rule_parser: &REG,
    }
}

fn quote_tick(
    source_id: &str,
    quote_seq: u64,
    instrument_code: &str,
    mid_yield: FixedDecimal,
    observed_at: MarketTime,
    received_at: MarketTime,
) -> MarketQuoteTick {
    MarketQuoteTick {
        source_id: source_id.to_owned(),
        quote_seq,
        instrument_code: instrument_code.to_owned(),
        source_type: PriceSourceType::ActiveQuote,
        bid_clean_price: None,
        ask_clean_price: None,
        mid_clean_price: None,
        mid_yield_to_maturity: Some(mid_yield),
        observed_at,
        received_at,
    }
}

fn make_proposal(
    account_id: &str,
    instrument_code: &str,
    side: TradeSide,
    bucket: SettlementBucket,
    notional: FixedDecimal,
    quote_terms: QuoteTerms,
    target_state: DraftTargetState,
) -> NlpEarmarkProposal {
    let raw = format!("交易意向: 买入 {instrument_code} 1000万");
    let start = raw.find(instrument_code).unwrap();
    let end = start + instrument_code.len();
    NlpEarmarkProposal {
        parser_source_id: "nlp-parser-1".to_owned(),
        model_version: "v1".to_owned(),
        raw_source_text: raw,
        anchors: vec![SourceTextAnchor {
            field_name: "instrument_code".to_owned(),
            byte_start: u32::try_from(start).unwrap(),
            byte_end: u32::try_from(end).unwrap(),
            span_text: instrument_code.to_owned(),
        }],
        confidence_score: fixed_frac(0, 950_000_000_000),
        account_id: account_id.to_owned(),
        instrument_code: instrument_code.to_owned(),
        side,
        settlement_bucket: bucket,
        notional: Some(notional),
        quote_terms: Some(quote_terms),
        suggested_target_state: target_state,
    }
}

#[allow(clippy::too_many_arguments)]
fn base_opening(
    account_id: &str,
    base_ulid: Ulid,
    source_id: &str,
    balances: Vec<OpeningInstrumentBalance>,
    instruments: Vec<InstrumentCatalogEntry>,
    calendar: Option<CalendarBinding>,
    default_cal_req: Option<CalendarRequirement>,
    units: AnalysisUnits,
) -> FullOpeningPositionBase {
    let codes: Vec<String> = instruments
        .iter()
        .map(|e| e.instrument_code.clone())
        .collect();
    FullOpeningPositionBase {
        core: OpeningPositionBase {
            account_id: account_id.to_owned(),
            base_id: base_ulid,
            trading_date: date(2026, 7, 21),
            next_settlement_date: date(2026, 7, 22),
            effective_from: shanghai_time(2026, 7, 21, 9, 0, 0),
            watermark_time: shanghai_time(2026, 7, 21, 9, 0, 0),
            watermark_source_seq: Some(10),
            source_id: source_id.to_owned(),
            opening_cash_t0: fixed(500_000_000),
            opening_net_cash_delta_t1: fixed(0),
            balances,
            instrument_codes: codes,
            baseline_evidence: Vec::new(),
            source_start_after_feed_seq: 10,
        },
        instruments,
        calendar,
        default_calendar_requirement: default_cal_req,
        units,
    }
}

// ============================================================================
// 1. F-24 / AC-007: Face 100 vs Face 1000 Exact PRE_TAX DV01 Parity,
//    Long/Short Sign Convention, Valuation Date vs Trade Cash Settlement Date
// ============================================================================

#[test]
fn face_100_and_face_1000_yield_exact_equal_dv01_draft_delta_and_hedge_contracts() {
    let now = shanghai_time(2026, 7, 21, 10, 0, 0);
    let clock = ManualBlotterClock::new(now.clone());
    let orch = DeskBlotterOrchestrator::new(&clock, engines());
    let units = test_units();
    let cal = make_calendar(date(2025, 1, 1), date(2035, 12, 31));

    let bond_100 = make_bond(
        '1',
        "BOND-100",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );
    let bond_1000 = make_bond(
        '2',
        "BOND-1000",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        1000,
        &units,
    );

    let ws0 = DeskBlotterWorkspaceState::new(BlotterConfig::default()).unwrap();
    let (ws1, _, _) = orch
        .import_opening_base(
            &ws0,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-a".to_owned(),
                opening_base: base_opening(
                    "acct-100",
                    ulid('1'),
                    "feed-a",
                    vec![OpeningInstrumentBalance {
                        instrument_code: "BOND-100".to_owned(),
                        opening_economic_notional: fixed(50_000_000),
                        opening_available_notional: fixed(50_000_000),
                        opening_net_t0_notional: fixed(0),
                        opening_net_t1_notional: fixed(0),
                    }],
                    vec![InstrumentCatalogEntry {
                        instrument_code: "BOND-100".to_owned(),
                        bond: Some(bond_100.clone()),
                        eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
                        calendar_requirement: None,
                    }],
                    Some(cal.clone()),
                    Some(CalendarRequirement::ExactMarket),
                    units.clone(),
                ),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();

    let (ws2, _, _) = orch
        .import_opening_base(
            &ws1,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-b".to_owned(),
                opening_base: base_opening(
                    "acct-1000",
                    ulid('2'),
                    "feed-a",
                    vec![OpeningInstrumentBalance {
                        instrument_code: "BOND-1000".to_owned(),
                        opening_economic_notional: fixed(50_000_000),
                        opening_available_notional: fixed(50_000_000),
                        opening_net_t0_notional: fixed(0),
                        opening_net_t1_notional: fixed(0),
                    }],
                    vec![InstrumentCatalogEntry {
                        instrument_code: "BOND-1000".to_owned(),
                        bond: Some(bond_1000.clone()),
                        eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
                        calendar_requirement: None,
                    }],
                    Some(cal.clone()),
                    Some(CalendarRequirement::ExactMarket),
                    units.clone(),
                ),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();

    // Ingest identical YTM = 2.40% for both bonds.
    let ytm = fixed_frac(0, 24_000_000_000);
    let (ws3, _, _) = orch
        .ingest_market_quotes(
            &ws2,
            &IngestMarketQuotesInput {
                account_id: "acct-100".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![
                    quote_tick("q-src-a", 1, "BOND-100", ytm, now.clone(), now.clone()),
                    quote_tick("q-src-a", 1, "BOND-1000", ytm, now.clone(), now.clone()),
                ],
            },
        )
        .unwrap();

    let snap_100 = orch.get_account_snapshot(&ws3, "acct-100").unwrap();
    let snap_1000 = orch.get_account_snapshot(&ws3, "acct-1000").unwrap();

    // 1) Account PRE_TAX DV01 strictly equal.
    assert_eq!(
        snap_100.positions[0].pre_tax_dv01,
        snap_1000.positions[0].pre_tax_dv01
    );
    let dv01_100 = snap_100.positions[0].pre_tax_dv01.unwrap();
    assert!(dv01_100.is_positive());
    assert_eq!(
        snap_100.risk_summary.covered_net_pre_tax_dv01,
        snap_1000.risk_summary.covered_net_pre_tax_dv01
    );

    // Cross-check original face_value=1000 kernel call Q * dF / F agrees within kernel precision.
    let raw_1000 = price_original_face_pre_tax_bond(
        &bond_1000,
        &units,
        &cal,
        CalendarRequirement::ExactMarket,
        date(2026, 7, 21),
        &now,
        AnalyticsMode::YieldIn,
        ytm,
        &NativeBondAnalyticsEngine,
    )
    .unwrap();
    let raw_dv01_from_1000 = wide_mul_div_fixed_round_half_even(
        fixed(50_000_000),
        raw_1000.measures().dv01(),
        fixed(1000),
    )
    .unwrap();
    let diff = dv01_100.scaled().abs_diff(raw_dv01_from_1000.scaled());
    // Within 1e-4 currency units (f64 scaling tolerance across 10x face value)
    assert!(
        diff <= 100_000_000,
        "standardized vs raw face=1000 DV01 diff too large: {diff}"
    );

    // 2) Draft delta PRE_TAX DV01 strictly equal.
    let qt = QuoteTerms {
        basis: Some(QuoteBasis::YieldToMaturity(ytm)),
        settlement_amount: None,
    };
    let (_, d100_out, _) = orch
        .submit_draft_earmark(
            &ws3,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-100".to_owned(),
                draft_id: ulid_pair('D', 'A'),
                proposal: make_proposal(
                    "acct-100",
                    "BOND-100",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(10_000_000),
                    qt.clone(),
                    DraftTargetState::Committed,
                ),
            },
        )
        .unwrap();
    let (_, d1000_out, _) = orch
        .submit_draft_earmark(
            &ws3,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-1000".to_owned(),
                draft_id: ulid_pair('D', 'B'),
                proposal: make_proposal(
                    "acct-1000",
                    "BOND-1000",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(10_000_000),
                    qt,
                    DraftTargetState::Committed,
                ),
            },
        )
        .unwrap();
    assert_eq!(
        d100_out.draft.preview_delta_pre_tax_dv01,
        d1000_out.draft.preview_delta_pre_tax_dv01
    );
    assert!(
        d100_out
            .draft
            .preview_delta_pre_tax_dv01
            .unwrap()
            .is_positive()
    );

    // 3) PreviewCoveredHedge contracts strictly equal.
    let delivery_pack = default_delivery_pack();
    let hedge_100 = orch
        .preview_covered_hedge(
            &ws3,
            &PreviewCoveredHedgeInput {
                account_id: "acct-100".to_owned(),
                scope: PreviewHedgeScope::WholeAccount,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: fixed_frac(0, 75_000_000_000),
                conversion_factor: fixed_frac(1, 0),
                delivery_rule_pack: delivery_pack.clone(),
                valuation_at: None,
                expected_snapshot_seq: Some(ws3.domain.head_seq),
            },
        )
        .unwrap();
    let hedge_1000 = orch
        .preview_covered_hedge(
            &ws3,
            &PreviewCoveredHedgeInput {
                account_id: "acct-1000".to_owned(),
                scope: PreviewHedgeScope::WholeAccount,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: fixed_frac(0, 75_000_000_000),
                conversion_factor: fixed_frac(1, 0),
                delivery_rule_pack: delivery_pack,
                valuation_at: None,
                expected_snapshot_seq: Some(ws3.domain.head_seq),
            },
        )
        .unwrap();
    assert_eq!(hedge_100.measures, hedge_1000.measures);
    assert!(hedge_100.measures.recommended_contracts < 0); // long book -> short futures
}

#[test]
fn long_short_opposite_sign_and_quote_update_preserves_locked_trade_cash() {
    let now = shanghai_time(2026, 7, 21, 10, 0, 0);
    let clock = ManualBlotterClock::new(now.clone());
    let orch = DeskBlotterOrchestrator::new(&clock, engines());
    let units = test_units();
    let cal = make_calendar(date(2025, 1, 1), date(2035, 12, 31));
    let bond = make_bond(
        '3',
        "240001",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );

    let ws0 = DeskBlotterWorkspaceState::new(BlotterConfig::default()).unwrap();
    let (ws1, _, _) = orch
        .import_opening_base(
            &ws0,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-long".to_owned(),
                opening_base: base_opening(
                    "acct-long",
                    ulid('3'),
                    "feed-a",
                    vec![OpeningInstrumentBalance {
                        instrument_code: "240001".to_owned(),
                        opening_economic_notional: fixed(30_000_000),
                        opening_available_notional: fixed(30_000_000),
                        opening_net_t0_notional: fixed(0),
                        opening_net_t1_notional: fixed(0),
                    }],
                    vec![InstrumentCatalogEntry {
                        instrument_code: "240001".to_owned(),
                        bond: Some(bond.clone()),
                        eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
                        calendar_requirement: None,
                    }],
                    Some(cal.clone()),
                    None,
                    units.clone(),
                ),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();

    let (ws2, _, _) = orch
        .import_opening_base(
            &ws1,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-short".to_owned(),
                opening_base: base_opening(
                    "acct-short",
                    ulid('4'),
                    "feed-a",
                    vec![OpeningInstrumentBalance {
                        instrument_code: "240001".to_owned(),
                        opening_economic_notional: fixed(-30_000_000),
                        opening_available_notional: fixed(0),
                        opening_net_t0_notional: fixed(0),
                        opening_net_t1_notional: fixed(0),
                    }],
                    vec![InstrumentCatalogEntry {
                        instrument_code: "240001".to_owned(),
                        bond: Some(bond),
                        eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
                        calendar_requirement: None,
                    }],
                    Some(cal),
                    None,
                    units,
                ),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();

    let (ws3, _, _) = orch
        .ingest_market_quotes(
            &ws2,
            &IngestMarketQuotesInput {
                account_id: "acct-long".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![quote_tick(
                    "q-src-a",
                    1,
                    "240001",
                    fixed_frac(0, 24_000_000_000),
                    now.clone(),
                    now.clone(),
                )],
            },
        )
        .unwrap();

    let long_dv01 = orch
        .get_account_snapshot(&ws3, "acct-long")
        .unwrap()
        .positions[0]
        .pre_tax_dv01
        .unwrap();
    let short_dv01 = orch
        .get_account_snapshot(&ws3, "acct-short")
        .unwrap()
        .positions[0]
        .pre_tax_dv01
        .unwrap();
    assert_eq!(long_dv01.scaled(), -short_dv01.scaled());
    assert!(long_dv01.is_positive());

    // Commit a T+1 BUY trade -> cash locked at T+1 settlement date dirty price,
    // valuation still anchored at trading_date (T+0).
    let (ws4, commit_out, _) = orch
        .create_committed(
            &ws3,
            &CreateCommittedCommand {
                account_id: "acct-long".to_owned(),
                earmark_id: ulid_pair('E', '9'),
                expected_position_version: 1,
                instrument_code: "240001".to_owned(),
                side: TradeSide::Buy,
                settlement_bucket: SettlementBucket::TPlus1,
                notional: fixed(10_000_000),
                quote_terms: QuoteTerms {
                    basis: Some(QuoteBasis::YieldToMaturity(fixed_frac(0, 24_000_000_000))),
                    settlement_amount: None,
                },
                exact_dirty_price_input: None,
                note: "t1 buy".to_owned(),
                trader_id: "trader-1".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(
        commit_out.earmark.funding_precision,
        FundingPrecision::ExactDirtyPrice
    );
    let locked_cash = commit_out.earmark.locked_settlement_cash.unwrap();
    assert!(locked_cash.scaled() < 0);
    assert_eq!(
        commit_out.position.valuation_settlement_date,
        date(2026, 7, 21)
    );
    let dv01_before = commit_out.position.pre_tax_dv01.unwrap();

    // Subsequent market quote move changes pre_tax_dv01, MUST NOT alter locked settlement cash.
    let (ws5, _, _) = orch
        .ingest_market_quotes(
            &ws4,
            &IngestMarketQuotesInput {
                account_id: "acct-long".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![quote_tick(
                    "q-src-a",
                    2,
                    "240001",
                    fixed_frac(0, 32_000_000_000),
                    now.clone(),
                    now,
                )],
            },
        )
        .unwrap();
    let snap_after = orch.get_account_snapshot(&ws5, "acct-long").unwrap();
    assert_ne!(snap_after.positions[0].pre_tax_dv01.unwrap(), dv01_before);
    assert_eq!(
        snap_after.active_earmarks[0].locked_settlement_cash,
        Some(locked_cash)
    );
}

// ============================================================================
// 2. Calendar Precedence Matrix & Out-of-Range Valuation Date Rejection
// ============================================================================

#[test]
fn calendar_precedence_matrix_and_valuation_date_bounds() {
    let now = shanghai_time(2026, 7, 21, 10, 0, 0);
    let clock = ManualBlotterClock::new(now.clone());
    let orch = DeskBlotterOrchestrator::new(&clock, engines());
    let units = test_units();
    // Calendar covers only through 2028-12-31, while long bond matures in 2035-08-08.
    let short_cal = make_calendar(date(2025, 1, 1), date(2028, 12, 31));
    let long_bond = make_bond(
        'K',
        "LONG-2035",
        date(2025, 8, 8),
        date(2035, 8, 8),
        250,
        100,
        &units,
    );
    let short_bond = make_bond(
        'S',
        "SHORT-2027",
        date(2025, 8, 8),
        date(2027, 8, 8),
        250,
        100,
        &units,
    );

    // Direct out-of-range valuation date rejection (even under ReferenceReplay or Unspecified).
    assert_eq!(
        resolve_effective_calendar_requirement(
            Some(CalendarRequirement::ReferenceReplay),
            None,
            date(2029, 1, 15),
            &short_cal,
        ),
        Err(UncoveredReason::CalendarCoverageMissing)
    );
    assert_eq!(
        resolve_effective_calendar_requirement(None, None, date(2024, 12, 31), &short_cal),
        Err(UncoveredReason::CalendarCoverageMissing)
    );

    let scenarios = [
        // (acct_req, inst_req, expect_covered, expect_res_or_reason)
        (
            "acct-exact-over-replay",
            Some(CalendarRequirement::ExactMarket),
            Some(CalendarRequirement::ReferenceReplay),
            false,
        ),
        (
            "acct-replay-under-exact",
            Some(CalendarRequirement::ReferenceReplay),
            Some(CalendarRequirement::ExactMarket),
            false,
        ),
        (
            "acct-replay-inst-unspec",
            Some(CalendarRequirement::ReferenceReplay),
            None,
            true,
        ),
        ("acct-both-unspec", None, None, true),
    ];

    let mut ws = DeskBlotterWorkspaceState::new(BlotterConfig::default()).unwrap();
    for (idx, (acct_id, acct_req, inst_req, _)) in scenarios.iter().enumerate() {
        let base_id = ulid_pair(
            'M',
            char::from_digit(u32::try_from(idx).unwrap() + 1, 10).unwrap(),
        );
        let (next_ws, _, _) = orch
            .import_opening_base(
                &ws,
                &ImportFullOpeningBaseCommand {
                    actor_id: "op-1".to_owned(),
                    idempotency_key: format!("imp-{acct_id}"),
                    opening_base: base_opening(
                        acct_id,
                        base_id,
                        "feed-cal",
                        vec![
                            OpeningInstrumentBalance {
                                instrument_code: "LONG-2035".to_owned(),
                                opening_economic_notional: fixed(20_000_000),
                                opening_available_notional: fixed(20_000_000),
                                opening_net_t0_notional: fixed(0),
                                opening_net_t1_notional: fixed(0),
                            },
                            OpeningInstrumentBalance {
                                instrument_code: "SHORT-2027".to_owned(),
                                opening_economic_notional: fixed(10_000_000),
                                opening_available_notional: fixed(10_000_000),
                                opening_net_t0_notional: fixed(0),
                                opening_net_t1_notional: fixed(0),
                            },
                        ],
                        vec![
                            InstrumentCatalogEntry {
                                instrument_code: "LONG-2035".to_owned(),
                                bond: Some(long_bond.clone()),
                                eligibility_class: Some(
                                    RateCoverageEligibility::SovereignOrPolicyFixed,
                                ),
                                calendar_requirement: *inst_req,
                            },
                            InstrumentCatalogEntry {
                                instrument_code: "SHORT-2027".to_owned(),
                                bond: Some(short_bond.clone()),
                                eligibility_class: Some(
                                    RateCoverageEligibility::SovereignOrPolicyFixed,
                                ),
                                calendar_requirement: None,
                            },
                        ],
                        Some(short_cal.clone()),
                        *acct_req,
                        units.clone(),
                    ),
                    expected_account_version: 0,
                    policy: default_policy(vec!["T", "TF", "TL"]),
                },
            )
            .unwrap();
        ws = next_ws;
    }

    let (ws, _, _) = orch
        .ingest_market_quotes(
            &ws,
            &IngestMarketQuotesInput {
                account_id: "acct-both-unspec".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![
                    quote_tick(
                        "q-src-a",
                        1,
                        "LONG-2035",
                        fixed_frac(0, 25_000_000_000),
                        now.clone(),
                        now.clone(),
                    ),
                    quote_tick(
                        "q-src-a",
                        1,
                        "SHORT-2027",
                        fixed_frac(0, 22_000_000_000),
                        now.clone(),
                        now,
                    ),
                ],
            },
        )
        .unwrap();

    for (acct_id, _, _, expect_covered) in scenarios {
        let snap = orch.get_account_snapshot(&ws, acct_id).unwrap();
        let long_pos = snap
            .positions
            .iter()
            .find(|p| p.position.instrument_code == "LONG-2035")
            .unwrap();
        let short_pos = snap
            .positions
            .iter()
            .find(|p| p.position.instrument_code == "SHORT-2027")
            .unwrap();

        assert_eq!(short_pos.coverage_status, CoverageStatus::CoveredPreTax);
        assert_eq!(short_pos.calendar_resolution, CalendarResolutionMode::Exact);

        if expect_covered {
            assert_eq!(long_pos.coverage_status, CoverageStatus::CoveredPreTax);
            assert_eq!(
                long_pos.calendar_resolution,
                CalendarResolutionMode::ProvisionalWeekendBeyondCoverage
            );
        } else {
            assert_eq!(long_pos.coverage_status, CoverageStatus::Uncovered);
            assert_eq!(
                long_pos.uncovered_reason,
                Some(UncoveredReason::CalendarCoverageMissing)
            );
        }
    }
}

// ============================================================================
// 3. Positive Coverage, Zero Position, Zero Net Exposure, CF Change & Policy A/B
// ============================================================================

#[test]
fn positive_coverage_hedge_scopes_zero_net_exposure_cf_and_policy_ab() {
    let now = shanghai_time(2026, 7, 21, 10, 0, 0);
    let clock = ManualBlotterClock::new(now.clone());
    let orch = DeskBlotterOrchestrator::new(&clock, engines());
    let units = test_units();
    let cal = make_calendar(date(2025, 1, 1), date(2035, 12, 31));
    let gov_a = make_bond(
        '1',
        "GOV-A",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );
    let gov_b = make_bond(
        '2',
        "GOV-B",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );
    let cred = make_bond(
        '3',
        "CRED-1",
        date(2025, 8, 8),
        date(2030, 8, 8),
        350,
        100,
        &units,
    );

    let ws0 = DeskBlotterWorkspaceState::new(BlotterConfig::default()).unwrap();
    // Account with: covered GOV-A (+50M), zero-position GOV-ZERO (0), uncovered CRED-1 (+20M),
    // and unknown-eligibility UNK-1 (+10M).
    let (ws1, _, _) = orch
        .import_opening_base(
            &ws0,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-mixed".to_owned(),
                opening_base: base_opening(
                    "acct-mixed",
                    ulid('5'),
                    "feed-h",
                    vec![
                        OpeningInstrumentBalance {
                            instrument_code: "GOV-A".to_owned(),
                            opening_economic_notional: fixed(50_000_000),
                            opening_available_notional: fixed(50_000_000),
                            opening_net_t0_notional: fixed(0),
                            opening_net_t1_notional: fixed(0),
                        },
                        OpeningInstrumentBalance {
                            instrument_code: "GOV-ZERO".to_owned(),
                            opening_economic_notional: fixed(0),
                            opening_available_notional: fixed(0),
                            opening_net_t0_notional: fixed(0),
                            opening_net_t1_notional: fixed(0),
                        },
                        OpeningInstrumentBalance {
                            instrument_code: "CRED-1".to_owned(),
                            opening_economic_notional: fixed(20_000_000),
                            opening_available_notional: fixed(20_000_000),
                            opening_net_t0_notional: fixed(0),
                            opening_net_t1_notional: fixed(0),
                        },
                        OpeningInstrumentBalance {
                            instrument_code: "UNK-1".to_owned(),
                            opening_economic_notional: fixed(10_000_000),
                            opening_available_notional: fixed(10_000_000),
                            opening_net_t0_notional: fixed(0),
                            opening_net_t1_notional: fixed(0),
                        },
                    ],
                    vec![
                        InstrumentCatalogEntry {
                            instrument_code: "GOV-A".to_owned(),
                            bond: Some(gov_a.clone()),
                            eligibility_class: Some(
                                RateCoverageEligibility::SovereignOrPolicyFixed,
                            ),
                            calendar_requirement: None,
                        },
                        InstrumentCatalogEntry {
                            instrument_code: "GOV-ZERO".to_owned(),
                            bond: Some(gov_b.clone()),
                            eligibility_class: Some(RateCoverageEligibility::CreditOrSpreadProduct),
                            calendar_requirement: None,
                        },
                        InstrumentCatalogEntry {
                            instrument_code: "CRED-1".to_owned(),
                            bond: Some(cred),
                            eligibility_class: Some(RateCoverageEligibility::CreditOrSpreadProduct),
                            calendar_requirement: None,
                        },
                        InstrumentCatalogEntry {
                            instrument_code: "UNK-1".to_owned(),
                            bond: Some(gov_b.clone()),
                            eligibility_class: None,
                            calendar_requirement: None,
                        },
                    ],
                    Some(cal.clone()),
                    None,
                    units.clone(),
                ),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]), // Policy A: forbids TS
            },
        )
        .unwrap();

    // Account with covered GOV-A (+50M) and offsetting GOV-B (-50M) => net DV01 == 0,
    // plus zero-position credit bond (must NOT block WHOLE_ACCOUNT!).
    let (ws2, _, _) = orch
        .import_opening_base(
            &ws1,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-zero-net".to_owned(),
                opening_base: base_opening(
                    "acct-zero-net",
                    ulid('6'),
                    "feed-h",
                    vec![
                        OpeningInstrumentBalance {
                            instrument_code: "GOV-A".to_owned(),
                            opening_economic_notional: fixed(50_000_000),
                            opening_available_notional: fixed(50_000_000),
                            opening_net_t0_notional: fixed(0),
                            opening_net_t1_notional: fixed(0),
                        },
                        OpeningInstrumentBalance {
                            instrument_code: "GOV-B".to_owned(),
                            opening_economic_notional: fixed(-50_000_000),
                            opening_available_notional: fixed(0),
                            opening_net_t0_notional: fixed(0),
                            opening_net_t1_notional: fixed(0),
                        },
                        OpeningInstrumentBalance {
                            instrument_code: "GOV-ZERO".to_owned(),
                            opening_economic_notional: fixed(0),
                            opening_available_notional: fixed(0),
                            opening_net_t0_notional: fixed(0),
                            opening_net_t1_notional: fixed(0),
                        },
                    ],
                    vec![
                        InstrumentCatalogEntry {
                            instrument_code: "GOV-A".to_owned(),
                            bond: Some(gov_a),
                            eligibility_class: Some(
                                RateCoverageEligibility::SovereignOrPolicyFixed,
                            ),
                            calendar_requirement: None,
                        },
                        InstrumentCatalogEntry {
                            instrument_code: "GOV-B".to_owned(),
                            bond: Some(gov_b.clone()),
                            eligibility_class: Some(
                                RateCoverageEligibility::SovereignOrPolicyFixed,
                            ),
                            calendar_requirement: None,
                        },
                        InstrumentCatalogEntry {
                            instrument_code: "GOV-ZERO".to_owned(),
                            bond: None,
                            eligibility_class: Some(RateCoverageEligibility::CreditOrSpreadProduct),
                            calendar_requirement: None,
                        },
                    ],
                    Some(cal.clone()),
                    None,
                    units.clone(),
                ),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL", "TS"]), // Policy B: allows TS
            },
        )
        .unwrap();

    // Account with empty covered book (only zero position).
    let (ws3, _, _) = orch
        .import_opening_base(
            &ws2,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-empty".to_owned(),
                opening_base: base_opening(
                    "acct-empty",
                    ulid('7'),
                    "feed-h",
                    vec![OpeningInstrumentBalance {
                        instrument_code: "GOV-ZERO".to_owned(),
                        opening_economic_notional: fixed(0),
                        opening_available_notional: fixed(0),
                        opening_net_t0_notional: fixed(0),
                        opening_net_t1_notional: fixed(0),
                    }],
                    vec![InstrumentCatalogEntry {
                        instrument_code: "GOV-ZERO".to_owned(),
                        bond: Some(gov_b),
                        eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
                        calendar_requirement: None,
                    }],
                    Some(cal),
                    None,
                    units,
                ),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();

    let ytm = fixed_frac(0, 24_000_000_000);
    let (ws4, _, _) = orch
        .ingest_market_quotes(
            &ws3,
            &IngestMarketQuotesInput {
                account_id: "acct-mixed".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![
                    quote_tick("q-src-a", 1, "GOV-A", ytm, now.clone(), now.clone()),
                    quote_tick("q-src-a", 1, "GOV-B", ytm, now.clone(), now),
                ],
            },
        )
        .unwrap();

    let snap_mixed = orch.get_account_snapshot(&ws4, "acct-mixed").unwrap();
    assert_eq!(snap_mixed.risk_summary.covered_instrument_count, 1);
    assert_eq!(snap_mixed.risk_summary.uncovered_instrument_count, 2);
    assert!(!snap_mixed.risk_summary.whole_account_hedge_allowed);
    let zero_pos = snap_mixed
        .positions
        .iter()
        .find(|p| p.position.instrument_code == "GOV-ZERO")
        .unwrap();
    assert_eq!(
        zero_pos.coverage_status,
        CoverageStatus::InactiveZeroPosition
    );
    let cred_pos = snap_mixed
        .positions
        .iter()
        .find(|p| p.position.instrument_code == "CRED-1")
        .unwrap();
    assert_eq!(
        cred_pos.uncovered_reason,
        Some(UncoveredReason::CreditOrNonRatesIssuer)
    );
    let unk_pos = snap_mixed
        .positions
        .iter()
        .find(|p| p.position.instrument_code == "UNK-1")
        .unwrap();
    assert_eq!(
        unk_pos.uncovered_reason,
        Some(UncoveredReason::EligibilityUnverified)
    );

    let delivery_pack = default_delivery_pack();

    // WHOLE_ACCOUNT blocked when uncovered > 0.
    let err = orch
        .preview_covered_hedge(
            &ws4,
            &PreviewCoveredHedgeInput {
                account_id: "acct-mixed".to_owned(),
                scope: PreviewHedgeScope::WholeAccount,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: fixed_frac(0, 75_000_000_000),
                conversion_factor: fixed(1),
                delivery_rule_pack: delivery_pack.clone(),
                valuation_at: None,
                expected_snapshot_seq: None,
            },
        )
        .unwrap_err();
    assert_eq!(
        err.detail(),
        Some(&ApplicationErrorDetail::BlotterWholeAccountHedgeBlocked)
    );

    // Policy A rejects TS.
    let err_ts_a = orch
        .preview_covered_hedge(
            &ws4,
            &PreviewCoveredHedgeInput {
                account_id: "acct-mixed".to_owned(),
                scope: PreviewHedgeScope::CoveredSubsetOnly,
                product_code: "TS".to_owned(),
                contract_code: "TS2609".to_owned(),
                ctd_dv01_per_100: fixed_frac(0, 75_000_000_000),
                conversion_factor: fixed(1),
                delivery_rule_pack: delivery_pack.clone(),
                valuation_at: None,
                expected_snapshot_seq: None,
            },
        )
        .unwrap_err();
    assert_eq!(
        err_ts_a.detail(),
        Some(&ApplicationErrorDetail::BlotterHedgeProductNotAllowed)
    );

    // COVERED_SUBSET_ONLY succeeds, echoes excluded uncovered gross notional, and responds to CF.
    let subset_cf1 = orch
        .preview_covered_hedge(
            &ws4,
            &PreviewCoveredHedgeInput {
                account_id: "acct-mixed".to_owned(),
                scope: PreviewHedgeScope::CoveredSubsetOnly,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: fixed_frac(0, 75_000_000_000),
                conversion_factor: fixed(1),
                delivery_rule_pack: delivery_pack.clone(),
                valuation_at: None,
                expected_snapshot_seq: None,
            },
        )
        .unwrap();
    let subset_cf12 = orch
        .preview_covered_hedge(
            &ws4,
            &PreviewCoveredHedgeInput {
                account_id: "acct-mixed".to_owned(),
                scope: PreviewHedgeScope::CoveredSubsetOnly,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: fixed_frac(0, 75_000_000_000),
                conversion_factor: fixed_frac(1, 200_000_000_000), // 1.2
                delivery_rule_pack: delivery_pack.clone(),
                valuation_at: None,
                expected_snapshot_seq: None,
            },
        )
        .unwrap();
    assert_eq!(
        subset_cf1.excluded_uncovered_gross_notional,
        fixed(30_000_000)
    );
    assert_eq!(subset_cf1.measures.contract_notional, fixed(1_000_000));
    assert!(
        subset_cf12.measures.recommended_contracts.abs()
            > subset_cf1.measures.recommended_contracts.abs()
    );

    // Empty covered book rejected.
    let err_empty = orch
        .preview_covered_hedge(
            &ws4,
            &PreviewCoveredHedgeInput {
                account_id: "acct-empty".to_owned(),
                scope: PreviewHedgeScope::WholeAccount,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: fixed_frac(0, 75_000_000_000),
                conversion_factor: fixed(1),
                delivery_rule_pack: delivery_pack.clone(),
                valuation_at: None,
                expected_snapshot_seq: None,
            },
        )
        .unwrap_err();
    assert_eq!(
        err_empty.detail(),
        Some(&ApplicationErrorDetail::BlotterEmptyCoveredBook)
    );

    // Zero net exposure on Policy B (allows TS with 2M contract notional from delivery pack):
    // 1) Invalid CF still rejected BEFORE zero-net-exposure short-circuit.
    let err_zero_bad_cf = orch
        .preview_covered_hedge(
            &ws4,
            &PreviewCoveredHedgeInput {
                account_id: "acct-zero-net".to_owned(),
                scope: PreviewHedgeScope::WholeAccount,
                product_code: "TS".to_owned(),
                contract_code: "TS2609".to_owned(),
                ctd_dv01_per_100: fixed_frac(0, 75_000_000_000),
                conversion_factor: fixed(0),
                delivery_rule_pack: delivery_pack.clone(),
                valuation_at: None,
                expected_snapshot_seq: None,
            },
        )
        .unwrap_err();
    assert_eq!(
        err_zero_bad_cf.category(),
        ApplicationErrorCategory::ValidationFailed
    );

    // 2) Valid zero-net-exposure returns 0 contracts, 0 residual, effectiveness 1, notional 2M.
    let zero_net = orch
        .preview_covered_hedge(
            &ws4,
            &PreviewCoveredHedgeInput {
                account_id: "acct-zero-net".to_owned(),
                scope: PreviewHedgeScope::WholeAccount,
                product_code: "TS".to_owned(),
                contract_code: "TS2609".to_owned(),
                ctd_dv01_per_100: fixed_frac(0, 75_000_000_000),
                conversion_factor: fixed(1),
                delivery_rule_pack: delivery_pack,
                valuation_at: None,
                expected_snapshot_seq: None,
            },
        )
        .unwrap();
    assert!(zero_net.zero_net_exposure);
    assert_eq!(zero_net.covered_tax_basis, BondTaxBasis::PreTax);
    assert_eq!(zero_net.covered_instrument_count, 2);
    assert_eq!(zero_net.uncovered_instrument_count, 0);
    assert_eq!(zero_net.measures.recommended_contracts, 0);
    assert_eq!(zero_net.measures.raw_contracts, fixed(0));
    assert_eq!(zero_net.measures.residual_dv01, fixed(0));
    assert_eq!(zero_net.measures.hedge_effectiveness, fixed(1));
    assert_eq!(zero_net.measures.contract_notional, fixed(2_000_000));
}

// ============================================================================
// 4. F-05 / AC-008: Shared Deterministic RulePack Draft Validator (6-Group Matrix)
// ============================================================================

#[test]
#[allow(clippy::too_many_lines)]
fn draft_shared_rule_pack_validator_six_group_matrix_and_anchors() {
    let now = shanghai_time(2026, 7, 21, 10, 0, 0);
    let clock = ManualBlotterClock::new(now.clone());
    let orch = DeskBlotterOrchestrator::new(&clock, engines());
    let units = test_units();
    let cal = make_calendar(date(2025, 1, 1), date(2035, 12, 31));
    let gov = make_bond(
        '1',
        "GOV-1",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );
    let cred = make_bond(
        '2',
        "CRED-1",
        date(2025, 8, 8),
        date(2030, 8, 8),
        350,
        100,
        &units,
    );

    let ws0 = DeskBlotterWorkspaceState::new(BlotterConfig::default()).unwrap();
    let (ws1, _, _) = orch
        .import_opening_base(
            &ws0,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-draft".to_owned(),
                opening_base: base_opening(
                    "acct-d",
                    ulid('8'),
                    "feed-d",
                    vec![
                        OpeningInstrumentBalance {
                            instrument_code: "GOV-1".to_owned(),
                            opening_economic_notional: fixed(50_000_000),
                            opening_available_notional: fixed(50_000_000),
                            opening_net_t0_notional: fixed(0),
                            opening_net_t1_notional: fixed(0),
                        },
                        OpeningInstrumentBalance {
                            instrument_code: "CRED-1".to_owned(),
                            opening_economic_notional: fixed(10_000_000),
                            opening_available_notional: fixed(10_000_000),
                            opening_net_t0_notional: fixed(0),
                            opening_net_t1_notional: fixed(0),
                        },
                    ],
                    vec![
                        InstrumentCatalogEntry {
                            instrument_code: "GOV-1".to_owned(),
                            bond: Some(gov),
                            eligibility_class: Some(
                                RateCoverageEligibility::SovereignOrPolicyFixed,
                            ),
                            calendar_requirement: None,
                        },
                        InstrumentCatalogEntry {
                            instrument_code: "CRED-1".to_owned(),
                            bond: Some(cred),
                            eligibility_class: Some(RateCoverageEligibility::CreditOrSpreadProduct),
                            calendar_requirement: None,
                        },
                    ],
                    Some(cal),
                    None,
                    units,
                ),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();

    let (ws2, _, _) = orch
        .ingest_market_quotes(
            &ws1,
            &IngestMarketQuotesInput {
                account_id: "acct-d".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![quote_tick(
                    "q-src-a",
                    1,
                    "GOV-1",
                    fixed_frac(0, 24_000_000_000),
                    now.clone(),
                    now.clone(),
                )],
            },
        )
        .unwrap();

    let clean_qt = QuoteTerms {
        basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
        settlement_amount: None,
    };

    // Group 0: UTF-8 anchor validation rejects bad byte boundary & mismatched span.
    let mut bad_anchor_prop = make_proposal(
        "acct-d",
        "GOV-1",
        TradeSide::Buy,
        SettlementBucket::TPlus0,
        fixed(10_000_000),
        clean_qt.clone(),
        DraftTargetState::Committed,
    );
    bad_anchor_prop.anchors[0].byte_start = 1; // middle of multi-byte Chinese char '交'
    bad_anchor_prop.anchors[0].byte_end = 4;
    assert_eq!(
        orch.submit_draft_earmark(
            &ws2,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('Z', '1'),
                proposal: bad_anchor_prop,
            },
        )
        .unwrap_err()
        .category(),
        ApplicationErrorCategory::ValidationFailed
    );

    // Group 1: Missing regulatory RulePack -> Submit rejected.
    let mut ws_no_pack = ws2.clone();
    ws_no_pack
        .accounts_meta
        .get_mut("acct-d")
        .unwrap()
        .policy
        .regulatory_rule_pack = None;
    assert_eq!(
        orch.submit_draft_earmark(
            &ws_no_pack,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('Z', '2'),
                proposal: make_proposal(
                    "acct-d",
                    "GOV-1",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(10_000_000),
                    clean_qt.clone(),
                    DraftTargetState::Committed,
                ),
            },
        )
        .unwrap_err()
        .detail(),
        Some(&ApplicationErrorDetail::BlotterRulePackInvalid)
    );

    // Group 2: Tampered content hash, payload-less pack, unverified, expired, and delivery-pack
    // substituted for regulatory pack -> all rejected.
    let tampered_bytes =
        br#"{"constraints":[{"limit_code":"INST_MAX","measure":"INSTRUMENT_FACE"}]}"#;
    let tampered_res = MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: ulid_pair('E', '9'),
            version: Version::new(1).unwrap(),
            owner: test_owner(),
            market: TEST_REG_MARKET.to_owned(),
            rule_type: TEST_REG_TYPE.to_owned(),
            source: "desk-compliance".to_owned(),
            effective: EffectivePeriod::new(
                shanghai_time(2026, 1, 1, 0, 0, 0),
                shanghai_time(2027, 1, 1, 0, 0, 0),
            )
            .unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(b"different-bytes"),
        },
        RulePackContent::new(TEST_REG_TYPE_URL, tampered_bytes.to_vec()).unwrap(),
    );
    assert_eq!(
        tampered_res.unwrap_err(),
        ficant_domain::DomainErrorCode::ContentHashMismatch
    );

    let payloadless_pack = MarketRulePack::new(MarketRulePackInput {
        rule_pack_id: ulid_pair('E', '8'),
        version: Version::new(1).unwrap(),
        owner: test_owner(),
        market: TEST_REG_MARKET.to_owned(),
        rule_type: TEST_REG_TYPE.to_owned(),
        source: "desk-compliance".to_owned(),
        effective: EffectivePeriod::new(
            shanghai_time(2026, 1, 1, 0, 0, 0),
            shanghai_time(2027, 1, 1, 0, 0, 0),
        )
        .unwrap(),
        verification_status: VerificationStatus::Verified,
        content_hash: ContentHash::digest(b"payloadless"),
    })
    .unwrap();
    let unverified_pack = make_regulatory_pack(
        1,
        &serde_json::json!({"constraints": [{"limit_code": "INST_MAX", "measure": "INSTRUMENT_FACE"}]}),
        VerificationStatus::Unverified,
        shanghai_time(2026, 1, 1, 0, 0, 0),
        shanghai_time(2027, 1, 1, 0, 0, 0),
    );
    let expired_pack = make_regulatory_pack(
        1,
        &serde_json::json!({"constraints": [{"limit_code": "INST_MAX", "measure": "INSTRUMENT_FACE"}]}),
        VerificationStatus::Verified,
        shanghai_time(2025, 1, 1, 0, 0, 0),
        shanghai_time(2026, 7, 21, 9, 0, 0), // expired before 10:00
    );
    for bad_pack in [
        payloadless_pack,
        unverified_pack,
        expired_pack,
        default_delivery_pack(),
    ] {
        let mut ws_bad = ws2.clone();
        ws_bad
            .accounts_meta
            .get_mut("acct-d")
            .unwrap()
            .policy
            .regulatory_rule_pack = Some(bad_pack);
        assert_eq!(
            orch.submit_draft_earmark(
                &ws_bad,
                &SubmitDraftEarmarkCommand {
                    account_id: "acct-d".to_owned(),
                    draft_id: ulid_pair('Z', '3'),
                    proposal: make_proposal(
                        "acct-d",
                        "GOV-1",
                        TradeSide::Buy,
                        SettlementBucket::TPlus0,
                        fixed(10_000_000),
                        clean_qt.clone(),
                        DraftTargetState::Committed,
                    ),
                },
            )
            .unwrap_err()
            .category(),
            ApplicationErrorCategory::ValidationFailed
        );
    }

    // Group 3: Empty constraints or missing ceiling -> rejected.
    let empty_pack = make_regulatory_pack(
        1,
        &serde_json::json!({"constraints": []}),
        VerificationStatus::Verified,
        shanghai_time(2026, 1, 1, 0, 0, 0),
        shanghai_time(2027, 1, 1, 0, 0, 0),
    );
    let mut ws_empty_c = ws2.clone();
    ws_empty_c
        .accounts_meta
        .get_mut("acct-d")
        .unwrap()
        .policy
        .regulatory_rule_pack = Some(empty_pack);
    assert!(matches!(
        orch.submit_draft_earmark(
            &ws_empty_c,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('Z', '4'),
                proposal: make_proposal(
                    "acct-d",
                    "GOV-1",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(10_000_000),
                    clean_qt.clone(),
                    DraftTargetState::Committed,
                ),
            },
        )
        .unwrap_err()
        .detail(),
        Some(ApplicationErrorDetail::RulePackItemMissing { .. })
    ));

    let mut ws_missing_ceil = ws2.clone();
    ws_missing_ceil
        .accounts_meta
        .get_mut("acct-d")
        .unwrap()
        .policy
        .limit_ceilings
        .remove("BORROW_MAX");
    assert_eq!(
        orch.submit_draft_earmark(
            &ws_missing_ceil,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('Z', '5'),
                proposal: make_proposal(
                    "acct-d",
                    "GOV-1",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(10_000_000),
                    clean_qt.clone(),
                    DraftTargetState::Committed,
                ),
            },
        )
        .unwrap_err()
        .detail(),
        Some(&ApplicationErrorDetail::BlotterConstraintLimitMissing {
            limit_code: "BORROW_MAX".to_owned()
        })
    );

    // Group 4: Proposed trade breaches INST_MAX, PORT_MAX, or BORROW_MAX -> rejected.
    assert_eq!(
        orch.submit_draft_earmark(
            &ws2,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('Z', '6'),
                proposal: make_proposal(
                    "acct-d",
                    "GOV-1",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(160_000_000),
                    clean_qt.clone(),
                    DraftTargetState::Committed,
                ),
            },
        )
        .unwrap_err()
        .detail(),
        Some(&ApplicationErrorDetail::BlotterConstraintLimitExceeded {
            limit_code: "INST_MAX".to_owned()
        })
    );

    // Breach PORT_MAX while staying within INST_MAX.
    let mut ws_tight_port = ws2.clone();
    ws_tight_port
        .accounts_meta
        .get_mut("acct-d")
        .unwrap()
        .policy
        .limit_ceilings
        .insert("PORT_MAX".to_owned(), fixed(65_000_000));
    assert_eq!(
        orch.submit_draft_earmark(
            &ws_tight_port,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('Z', '7'),
                proposal: make_proposal(
                    "acct-d",
                    "GOV-1",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(10_000_000), // 60M + 10M = 70M > 65M
                    clean_qt.clone(),
                    DraftTargetState::Committed,
                ),
            },
        )
        .unwrap_err()
        .detail(),
        Some(&ApplicationErrorDetail::BlotterConstraintLimitExceeded {
            limit_code: "PORT_MAX".to_owned()
        })
    );

    // Breach BORROW_MAX (opening_cash_t0 = 500M; buy 600M -> borrowed cash > 50M ceiling).
    let mut ws_tight_borrow = ws2.clone();
    {
        let pol = &mut ws_tight_borrow
            .accounts_meta
            .get_mut("acct-d")
            .unwrap()
            .policy;
        pol.limit_ceilings
            .insert("INST_MAX".to_owned(), fixed(900_000_000));
        pol.limit_ceilings
            .insert("PORT_MAX".to_owned(), fixed(900_000_000));
        pol.limit_ceilings
            .insert("BORROW_MAX".to_owned(), fixed(50_000_000));
    }
    assert_eq!(
        orch.submit_draft_earmark(
            &ws_tight_borrow,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('Z', '8'),
                proposal: make_proposal(
                    "acct-d",
                    "GOV-1",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(600_000_000),
                    clean_qt.clone(),
                    DraftTargetState::Committed,
                ),
            },
        )
        .unwrap_err()
        .detail(),
        Some(&ApplicationErrorDetail::BlotterConstraintLimitExceeded {
            limit_code: "BORROW_MAX".to_owned()
        })
    );

    // Target HOLD draft reports immediate economic DV01 delta == 0.
    let (_, hold_draft_out, _) = orch
        .submit_draft_earmark(
            &ws2,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('H', '1'),
                proposal: make_proposal(
                    "acct-d",
                    "GOV-1",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(10_000_000),
                    clean_qt.clone(),
                    DraftTargetState::Hold,
                ),
            },
        )
        .unwrap();
    assert_eq!(
        hold_draft_out.draft.preview_delta_pre_tax_dv01,
        Some(fixed(0))
    );

    // Group 5: Rule valid but DV01 uncovered (CRED-1) -> Submit succeeds with RuleLineage;
    // Confirm without acknowledge_uncovered_dv01 rejected; Confirm WITH acknowledge_uncovered_dv01=true succeeds;
    // AND if rule pack is removed/breached before confirm, even acknowledge_uncovered_dv01=true STILL returns rule error!
    let (ws_uncov, uncov_sub, _) = orch
        .submit_draft_earmark(
            &ws2,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('W', '1'),
                proposal: make_proposal(
                    "acct-d",
                    "CRED-1",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(15_000_000),
                    clean_qt.clone(),
                    DraftTargetState::Committed,
                ),
            },
        )
        .unwrap();
    assert_eq!(
        uncov_sub.draft.preview_coverage_status,
        CoverageStatus::Uncovered
    );
    assert_eq!(
        uncov_sub.draft.preview_uncovered_reason,
        Some(UncoveredReason::CreditOrNonRatesIssuer)
    );
    assert_eq!(uncov_sub.draft.rule_lineage.version, 1);

    // Missing acknowledgement -> rejected.
    let err_no_ack = orch
        .confirm_draft_earmark(
            &ws_uncov,
            &ConfirmDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('W', '1'),
                earmark_id: ulid_pair('E', '1'),
                expected_draft_revision: 1,
                expected_position_version: 1,
                confirmation_token: uncov_sub.draft.confirmation_token.clone(),
                acknowledge_uncovered_dv01: false,
                trader_id: "trader-alice".to_owned(),
            },
        )
        .unwrap_err();
    assert_eq!(
        err_no_ack.detail(),
        Some(&ApplicationErrorDetail::BlotterUncoveredRiskAcknowledgementRequired)
    );

    // If rule pack vanishes, ceiling disappears, or ceiling is breached before confirm,
    // acknowledge_uncovered_dv01=true STILL returns the rule error!
    let mut ws_uncov_no_pack = ws_uncov.clone();
    ws_uncov_no_pack
        .accounts_meta
        .get_mut("acct-d")
        .unwrap()
        .policy
        .regulatory_rule_pack = None;
    let err_rule_on_confirm = orch
        .confirm_draft_earmark(
            &ws_uncov_no_pack,
            &ConfirmDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('W', '1'),
                earmark_id: ulid_pair('E', '1'),
                expected_draft_revision: 1,
                expected_position_version: 1,
                confirmation_token: uncov_sub.draft.confirmation_token.clone(),
                acknowledge_uncovered_dv01: true,
                trader_id: "trader-alice".to_owned(),
            },
        )
        .unwrap_err();
    assert_eq!(
        err_rule_on_confirm.detail(),
        Some(&ApplicationErrorDetail::BlotterRulePackInvalid)
    );

    let mut ws_uncov_breach = ws_uncov.clone();
    ws_uncov_breach
        .accounts_meta
        .get_mut("acct-d")
        .unwrap()
        .policy
        .limit_ceilings
        .insert("INST_MAX".to_owned(), fixed(20_000_000)); // 10M + 15M = 25M > 20M
    assert_eq!(
        orch.confirm_draft_earmark(
            &ws_uncov_breach,
            &ConfirmDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('W', '1'),
                earmark_id: ulid_pair('E', '1'),
                expected_draft_revision: 1,
                expected_position_version: 1,
                confirmation_token: uncov_sub.draft.confirmation_token.clone(),
                acknowledge_uncovered_dv01: true,
                trader_id: "trader-alice".to_owned(),
            },
        )
        .unwrap_err()
        .detail(),
        Some(&ApplicationErrorDetail::BlotterConstraintLimitExceeded {
            limit_code: "INST_MAX".to_owned()
        })
    );

    // With valid pack and acknowledge_uncovered_dv01=true -> succeeds and consumes draft once.
    let (ws_confirmed, conf_out, _) = orch
        .confirm_draft_earmark(
            &ws_uncov,
            &ConfirmDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('W', '1'),
                earmark_id: ulid_pair('E', '1'),
                expected_draft_revision: 1,
                expected_position_version: 1,
                confirmation_token: uncov_sub.draft.confirmation_token.clone(),
                acknowledge_uncovered_dv01: true,
                trader_id: "trader-alice".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(conf_out.draft.status, DraftEarmarkStatus::Confirmed);
    assert_eq!(
        conf_out.draft.confirmed_by_trader_id.as_deref(),
        Some("trader-alice")
    );
    assert_eq!(conf_out.earmark.state, EarmarkState::Committed);
    assert_eq!(
        conf_out.earmark.confirmed_by_trader_id.as_deref(),
        Some("trader-alice")
    );
    // Single-use consumption: confirming again fails.
    assert_eq!(
        orch.confirm_draft_earmark(
            &ws_confirmed,
            &ConfirmDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('W', '1'),
                earmark_id: ulid_pair('E', '2'),
                expected_draft_revision: 2,
                expected_position_version: 2,
                confirmation_token: uncov_sub.draft.confirmation_token,
                acknowledge_uncovered_dv01: true,
                trader_id: "trader-alice".to_owned(),
            },
        )
        .unwrap_err()
        .detail(),
        Some(&ApplicationErrorDetail::BlotterInvalidStateTransition)
    );

    // Group 6: Any upstream context change invalidates preview -> ConcurrencyConflict.
    let (ws_cov_draft, cov_sub, _) = orch
        .submit_draft_earmark(
            &ws2,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-d".to_owned(),
                draft_id: ulid_pair('V', '1'),
                proposal: make_proposal(
                    "acct-d",
                    "GOV-1",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(10_000_000),
                    clean_qt,
                    DraftTargetState::Committed,
                ),
            },
        )
        .unwrap();

    let confirm_cmd = ConfirmDraftEarmarkCommand {
        account_id: "acct-d".to_owned(),
        draft_id: ulid_pair('V', '1'),
        earmark_id: ulid_pair('E', '3'),
        expected_draft_revision: 1,
        expected_position_version: 1,
        confirmation_token: cov_sub.draft.confirmation_token,
        acknowledge_uncovered_dv01: true,
        trader_id: "trader-alice".to_owned(),
    };

    // 6a. Rule pack upgraded to v2 -> ConcurrencyConflict.
    let mut ws_rule_v2 = ws_cov_draft.clone();
    ws_rule_v2
        .accounts_meta
        .get_mut("acct-d")
        .unwrap()
        .policy
        .regulatory_rule_pack = Some(make_regulatory_pack(
        2,
        &serde_json::json!({
            "constraints": [
                {"limit_code": "INST_MAX", "measure": "INSTRUMENT_FACE"},
                {"limit_code": "PORT_MAX", "measure": "PORTFOLIO_FACE"},
                {"limit_code": "BORROW_MAX", "measure": "BORROWED_CASH"}
            ]
        }),
        VerificationStatus::Verified,
        shanghai_time(2026, 1, 1, 0, 0, 0),
        shanghai_time(2027, 1, 1, 0, 0, 0),
    ));
    assert_eq!(
        orch.confirm_draft_earmark(&ws_rule_v2, &confirm_cmd)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    // 6b. Limit ceiling adjusted (still valid) -> ConcurrencyConflict.
    let mut ws_ceil_chg = ws_cov_draft.clone();
    ws_ceil_chg
        .accounts_meta
        .get_mut("acct-d")
        .unwrap()
        .policy
        .limit_ceilings
        .insert("INST_MAX".to_owned(), fixed(250_000_000));
    assert_eq!(
        orch.confirm_draft_earmark(&ws_ceil_chg, &confirm_cmd)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    // 6c. New market quote arrives -> ConcurrencyConflict.
    let (ws_new_q, _, _) = orch
        .ingest_market_quotes(
            &ws_cov_draft,
            &IngestMarketQuotesInput {
                account_id: "acct-d".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![quote_tick(
                    "q-src-a",
                    2,
                    "GOV-1",
                    fixed_frac(0, 26_000_000_000),
                    now.clone(),
                    now,
                )],
            },
        )
        .unwrap();
    assert_eq!(
        orch.confirm_draft_earmark(&ws_new_q, &confirm_cmd)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    // 6d. Active quote source switched -> ConcurrencyConflict.
    let (ws_switched, _, _) = orch
        .switch_active_quote_source(
            &ws_cov_draft,
            &SwitchActiveQuoteSourceCommand {
                account_id: "acct-d".to_owned(),
                instrument_code: Some("GOV-1".to_owned()),
                new_source_id: "q-src-b".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(
        orch.confirm_draft_earmark(&ws_switched, &confirm_cmd)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    // 6e. Desk policy changes -> ConcurrencyConflict.
    let mut ws_policy_chg = ws_cov_draft.clone();
    ws_policy_chg
        .accounts_meta
        .get_mut("acct-d")
        .unwrap()
        .policy
        .policy_version += 1;
    assert_eq!(
        orch.confirm_draft_earmark(&ws_policy_chg, &confirm_cmd)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    // 6f. Underlying position changes -> ConcurrencyConflict (even if caller passes updated position_version).
    let (ws_pos_chg, _, _) = orch
        .create_hold(
            &ws_cov_draft,
            &CreateHoldCommand {
                account_id: "acct-d".to_owned(),
                earmark_id: ulid_pair('H', '7'),
                expected_position_version: 1,
                instrument_code: "GOV-1".to_owned(),
                side: TradeSide::Buy,
                settlement_bucket: SettlementBucket::TPlus0,
                notional: fixed(1_000_000),
                quote_terms: None,
                ttl_seconds: 1_800,
                note: "concurrent hold".to_owned(),
                trader_id: "trader-bob".to_owned(),
            },
        )
        .unwrap();
    let mut confirm_cmd_updated_pos = confirm_cmd.clone();
    confirm_cmd_updated_pos.expected_position_version = 2;
    assert_eq!(
        orch.confirm_draft_earmark(&ws_pos_chg, &confirm_cmd_updated_pos)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    // 6g. Clock advances so quote becomes stale -> ConcurrencyConflict.
    clock.advance_seconds(301).unwrap();
    assert_eq!(
        orch.confirm_draft_earmark(&ws_cov_draft, &confirm_cmd)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );
}

// ============================================================================
// 5. F-27 / AC-008: Per-(source_id, instrument) Quote Cursors, Backfill,
//    Replay Staleness Protection, Source Switch & Startup Reconciliation
// ============================================================================

#[test]
fn market_quotes_per_source_cursors_backfill_replay_staleness_and_switch() {
    let t0 = shanghai_time(2026, 7, 21, 10, 0, 0);
    let clock = ManualBlotterClock::new(t0.clone());
    let orch = DeskBlotterOrchestrator::new(&clock, engines());
    let units = test_units();
    let cal = make_calendar(date(2025, 1, 1), date(2035, 12, 31));
    let bond = make_bond(
        '1',
        "240010",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );

    let ws0 = DeskBlotterWorkspaceState::new(BlotterConfig::default()).unwrap();
    let (ws1, _, _) = orch
        .import_opening_base(
            &ws0,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-q1".to_owned(),
                opening_base: base_opening(
                    "acct-q1",
                    ulid('9'),
                    "feed-q",
                    vec![OpeningInstrumentBalance {
                        instrument_code: "240010".to_owned(),
                        opening_economic_notional: fixed(40_000_000),
                        opening_available_notional: fixed(40_000_000),
                        opening_net_t0_notional: fixed(0),
                        opening_net_t1_notional: fixed(0),
                    }],
                    vec![InstrumentCatalogEntry {
                        instrument_code: "240010".to_owned(),
                        bond: Some(bond.clone()),
                        eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
                        calendar_requirement: None,
                    }],
                    Some(cal.clone()),
                    None,
                    units.clone(),
                ),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();

    // Second account sharing q-src-a to verify shared-source event propagation.
    let (ws2, _, _) = orch
        .import_opening_base(
            &ws1,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-q2".to_owned(),
                opening_base: base_opening(
                    "acct-q2",
                    ulid_pair('9', 'B'),
                    "feed-q",
                    vec![OpeningInstrumentBalance {
                        instrument_code: "240010".to_owned(),
                        opening_economic_notional: fixed(10_000_000),
                        opening_available_notional: fixed(10_000_000),
                        opening_net_t0_notional: fixed(0),
                        opening_net_t1_notional: fixed(0),
                    }],
                    vec![InstrumentCatalogEntry {
                        instrument_code: "240010".to_owned(),
                        bond: Some(bond),
                        eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
                        calendar_requirement: None,
                    }],
                    Some(cal),
                    None,
                    units,
                ),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();

    // Missing quote initially.
    let snap0 = orch.get_account_snapshot(&ws2, "acct-q1").unwrap();
    assert_eq!(
        snap0.positions[0].uncovered_reason,
        Some(UncoveredReason::MissingMarketQuote)
    );

    // Source A seq=100 ingested; both accounts affected in one event envelope.
    let (ws3, q_out_a, ev_a) = orch
        .ingest_market_quotes(
            &ws2,
            &IngestMarketQuotesInput {
                account_id: "acct-q1".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![quote_tick(
                    "q-src-a",
                    100,
                    "240010",
                    fixed_frac(0, 24_000_000_000),
                    t0.clone(),
                    t0.clone(),
                )],
            },
        )
        .unwrap();
    assert_eq!(q_out_a.applied_tick_count, 1);
    assert_eq!(q_out_a.affected_accounts, vec!["acct-q1", "acct-q2"]);
    assert_eq!(ev_a.unwrap().changes.len(), 2);

    // Source B seq=1 coexists independently with Source A seq=100!
    let (ws4, q_out_b, _) = orch
        .ingest_market_quotes(
            &ws3,
            &IngestMarketQuotesInput {
                account_id: "acct-q1".to_owned(),
                source_id: "q-src-b".to_owned(),
                ticks: vec![quote_tick(
                    "q-src-b",
                    1,
                    "240010",
                    fixed_frac(0, 30_000_000_000),
                    t0.clone(),
                    t0.clone(),
                )],
            },
        )
        .unwrap();
    assert_eq!(q_out_b.applied_tick_count, 1);
    assert_eq!(
        ws4.quotes
            .heads
            .get(&("q-src-a".to_owned(), "240010".to_owned())),
        Some(&100)
    );
    assert_eq!(
        ws4.quotes
            .heads
            .get(&("q-src-b".to_owned(), "240010".to_owned())),
        Some(&1)
    );

    // Same (source, instrument, quote_seq) with conflicting payload -> whole batch HashMismatch!
    let conflict_err = orch
        .ingest_market_quotes(
            &ws4,
            &IngestMarketQuotesInput {
                account_id: "acct-q1".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![
                    quote_tick(
                        "q-src-a",
                        101,
                        "240010",
                        fixed_frac(0, 25_000_000_000),
                        t0.clone(),
                        t0.clone(),
                    ),
                    quote_tick(
                        "q-src-a",
                        100,
                        "240010",
                        fixed_frac(0, 99_000_000_000), // tampered yield!
                        t0.clone(),
                        t0.clone(),
                    ),
                ],
            },
        )
        .unwrap_err();
    assert_eq!(
        conflict_err.category(),
        ApplicationErrorCategory::HashMismatch
    );
    // Atomic rejection: seq 101 was NOT stored.
    assert!(
        !ws4.quotes
            .history
            .contains_key(&("q-src-a".to_owned(), "240010".to_owned(), 101))
    );

    // Backfill older unseen seq=50: stored in history, head stays at 100!
    let (ws5, bf_out, _) = orch
        .ingest_market_quotes(
            &ws4,
            &IngestMarketQuotesInput {
                account_id: "acct-q1".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![quote_tick(
                    "q-src-a",
                    50,
                    "240010",
                    fixed_frac(0, 23_000_000_000),
                    t0.clone(),
                    t0.clone(),
                )],
            },
        )
        .unwrap();
    assert_eq!(bf_out.applied_tick_count, 0);
    assert_eq!(bf_out.backfilled_tick_count, 1);
    assert_eq!(
        ws5.quotes
            .heads
            .get(&("q-src-a".to_owned(), "240010".to_owned())),
        Some(&100)
    );
    assert!(
        ws5.quotes
            .history
            .contains_key(&("q-src-a".to_owned(), "240010".to_owned(), 50))
    );

    // Switch acct-q1 active source to q-src-b -> uses seq=1 (yield 3.00%), bumps policy_version & seq.
    let dv01_src_a = orch
        .get_account_snapshot(&ws5, "acct-q1")
        .unwrap()
        .positions[0]
        .pre_tax_dv01
        .unwrap();
    let (ws6, sw_out, sw_ev) = orch
        .switch_active_quote_source(
            &ws5,
            &SwitchActiveQuoteSourceCommand {
                account_id: "acct-q1".to_owned(),
                instrument_code: Some("240010".to_owned()),
                new_source_id: "q-src-b".to_owned(),
            },
        )
        .unwrap();
    assert!(sw_out.mutated);
    assert_eq!(sw_out.policy_version, 2);
    assert!(sw_ev.is_some());
    let snap_b = orch.get_account_snapshot(&ws6, "acct-q1").unwrap();
    assert_eq!(snap_b.positions[0].active_quote_source_id, "q-src-b");
    assert_eq!(snap_b.positions[0].active_quote_seq, Some(1));
    assert_ne!(snap_b.positions[0].pre_tax_dv01.unwrap(), dv01_src_a);

    // Advance clock +301s and replay exact same q-src-b seq=1 tick with newer received_at:
    // MUST deduplicate without extending freshness -> StaleQuote!
    let t1 = clock.advance_seconds(301).unwrap();
    let (ws7, replay_out, replay_ev) = orch
        .ingest_market_quotes(
            &ws6,
            &IngestMarketQuotesInput {
                account_id: "acct-q1".to_owned(),
                source_id: "q-src-b".to_owned(),
                ticks: vec![quote_tick(
                    "q-src-b",
                    1,
                    "240010",
                    fixed_frac(0, 30_000_000_000),
                    t0,
                    t1, // bumped received_at on replay must NOT refresh first_received_at
                )],
            },
        )
        .unwrap();
    assert!(!replay_out.mutated);
    assert!(replay_ev.is_none());
    assert_eq!(replay_out.skipped_stale_tick_count, 1);
    let stale_snap = orch.get_account_snapshot(&ws7, "acct-q1").unwrap();
    assert_eq!(
        stale_snap.positions[0].coverage_status,
        CoverageStatus::Uncovered
    );
    assert_eq!(
        stale_snap.positions[0].uncovered_reason,
        Some(UncoveredReason::StaleQuote)
    );

    // Startup quote-source reconciliation commits visible transaction when config changed.
    let mut startup_policies = BTreeMap::new();
    let mut new_pol = default_policy(vec!["T", "TF", "TL"]);
    new_pol.default_quote_source_id = "q-src-c".to_owned();
    startup_policies.insert("acct-q2".to_owned(), new_pol);
    let (ws8, rec_out, rec_ev) = orch
        .reconcile_startup_quote_sources(&ws7, &startup_policies)
        .unwrap();
    assert!(rec_out.mutated);
    assert_eq!(rec_out.affected_accounts, vec!["acct-q2"]);
    assert!(rec_ev.is_some());
    assert_eq!(
        ws8.accounts_meta["acct-q2"].policy.default_quote_source_id,
        "q-src-c"
    );
}

// ============================================================================
// 6. [m-3] Cross-Account Baseline Evidence Defense, [m-2] Cross-Key Held Trade
//    Defense, FetchTradeRevision Backfill, Lifecycle & SPI Page Size Guard
// ============================================================================

struct StubReconciler {
    revisions: BTreeMap<(String, String, u64), TradeFillRecord>,
}

impl TradeFeedReconcilerPort for StubReconciler {
    fn pull_trade_feed_batch(
        &self,
        _source_id: &str,
        after_feed_seq: u64,
        max_records: u32,
    ) -> ApplicationResult<PullTradeFeedBatchResult> {
        validate_spi_page_size(max_records)?;
        Ok(PullTradeFeedBatchResult {
            fills: Vec::new(),
            scanned_through_feed_seq: after_feed_seq,
            has_more: false,
        })
    }

    fn fetch_trade_revision(
        &self,
        source_id: &str,
        external_trade_id: &str,
        source_revision: u64,
    ) -> ApplicationResult<TradeFillRecord> {
        self.revisions
            .get(&(
                source_id.to_owned(),
                external_trade_id.to_owned(),
                source_revision,
            ))
            .cloned()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn cross_account_baseline_conflict_fetch_trade_revision_and_cross_key_hold_defense() {
    assert!(validate_spi_page_size(0).is_err());
    assert!(validate_spi_page_size(MIN_SPI_PAGE_SIZE).is_ok());
    assert!(validate_spi_page_size(MAX_SPI_PAGE_SIZE).is_ok());
    assert!(validate_spi_page_size(MAX_SPI_PAGE_SIZE + 1).is_err());

    let t0 = shanghai_time(2026, 7, 21, 10, 0, 0);
    let clock = ManualBlotterClock::new(t0.clone());
    let orch = DeskBlotterOrchestrator::new(&clock, engines());
    let units = test_units();
    let cal = make_calendar(date(2025, 1, 1), date(2035, 12, 31));
    let bond_a = make_bond(
        '1',
        "BOND-A",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );
    let bond_b = make_bond(
        '2',
        "BOND-B",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );

    let shared_ev = BaseEvidence {
        source_id: "feed-shared".to_owned(),
        external_trade_id: "T-BASE-SHARED".to_owned(),
        original_feed_seq: 5,
        original_executed_at: shanghai_time(2026, 7, 21, 8, 30, 0),
        old_notional: fixed(50_000_000),
        old_signed_cash: fixed(50_000_000),
        settlement_bucket: SettlementBucket::TPlus0,
        evidence_ref: "base:5".to_owned(),
        evidence_hash: ContentHash::digest(b"base-ev-5"),
        instrument_code: "BOND-A".to_owned(),
        side: TradeSide::Sell,
        original_revision: 1,
    };

    let mut base_a = base_opening(
        "acct-a",
        ulid_pair('P', '1'),
        "feed-shared",
        vec![
            OpeningInstrumentBalance {
                instrument_code: "BOND-A".to_owned(),
                opening_economic_notional: fixed(100_000_000),
                opening_available_notional: fixed(100_000_000),
                opening_net_t0_notional: fixed(0),
                opening_net_t1_notional: fixed(0),
            },
            OpeningInstrumentBalance {
                instrument_code: "BOND-B".to_owned(),
                opening_economic_notional: fixed(100_000_000),
                opening_available_notional: fixed(100_000_000),
                opening_net_t0_notional: fixed(0),
                opening_net_t1_notional: fixed(0),
            },
        ],
        vec![
            InstrumentCatalogEntry {
                instrument_code: "BOND-A".to_owned(),
                bond: Some(bond_a.clone()),
                eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
                calendar_requirement: None,
            },
            InstrumentCatalogEntry {
                instrument_code: "BOND-B".to_owned(),
                bond: Some(bond_b),
                eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
                calendar_requirement: None,
            },
        ],
        Some(cal.clone()),
        None,
        units.clone(),
    );
    base_a.core.baseline_evidence.push(shared_ev.clone());

    let ws0 = DeskBlotterWorkspaceState::new(BlotterConfig::default()).unwrap();
    let (ws1, _, _) = orch
        .import_opening_base(
            &ws0,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-shared-a".to_owned(),
                opening_base: base_a,
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();

    // [m-3] Account B trying to claim the same (source_id, external_trade_id) baseline evidence -> rejected!
    let mut base_b_conflict = base_opening(
        "acct-b",
        ulid_pair('P', '2'),
        "feed-shared",
        vec![OpeningInstrumentBalance {
            instrument_code: "BOND-A".to_owned(),
            opening_economic_notional: fixed(10_000_000),
            opening_available_notional: fixed(10_000_000),
            opening_net_t0_notional: fixed(0),
            opening_net_t1_notional: fixed(0),
        }],
        vec![InstrumentCatalogEntry {
            instrument_code: "BOND-A".to_owned(),
            bond: Some(bond_a),
            eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
            calendar_requirement: None,
        }],
        Some(cal),
        None,
        units,
    );
    base_b_conflict.core.baseline_evidence.push(shared_ev);
    assert_eq!(
        orch.import_opening_base(
            &ws1,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-shared-b".to_owned(),
                opening_base: base_b_conflict,
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap_err()
        .category(),
        ApplicationErrorCategory::ValidationFailed
    );

    // Ingest rev 1 of trade T-CROSS on BOND-A (T+0) -> creates OPEN orphan pending match on K1.
    let mut id_gen = DeterministicIdGenerator::default();
    let (ws2, ing1, _) = orch
        .ingest_trade_feed(
            &ws1,
            &IngestTradeFeedInput {
                source_id: "feed-shared".to_owned(),
                fills: vec![TradeFillRecord {
                    source_id: "feed-shared".to_owned(),
                    external_trade_id: "T-CROSS".to_owned(),
                    source_revision: 1,
                    feed_seq: 11,
                    action: TradeFeedRecordAction::NewFill,
                    account_id: "acct-a".to_owned(),
                    instrument_code: "BOND-A".to_owned(),
                    side: TradeSide::Buy,
                    settlement_date: date(2026, 7, 21),
                    notional: fixed(10_000_000),
                    quote_terms: Some(QuoteTerms {
                        basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                        settlement_amount: Some(fixed(10_000_000)),
                    }),
                    executed_at: t0.clone(),
                    batch_first_feed_seq: 11,
                    batch_last_feed_seq: 11,
                    exact_dirty_price_input: None,
                }],
                heartbeat_at: Some(t0.clone()),
                actor_id: "adapter-1".to_owned(),
            },
            &mut id_gen,
        )
        .unwrap();
    let pm_k1_id = ing1.item_outcomes[0].pending_match_id.clone().unwrap();

    // Now rev 3 of T-CROSS arrives on a DIFFERENT four-key K2 (T+1), missing rev 2 -> REVISION_HOLD!
    let (ws3, ing2, _) = orch
        .ingest_trade_feed(
            &ws2,
            &IngestTradeFeedInput {
                source_id: "feed-shared".to_owned(),
                fills: vec![TradeFillRecord {
                    source_id: "feed-shared".to_owned(),
                    external_trade_id: "T-CROSS".to_owned(),
                    source_revision: 3,
                    feed_seq: 12,
                    action: TradeFeedRecordAction::Amend,
                    account_id: "acct-a".to_owned(),
                    instrument_code: "BOND-A".to_owned(),
                    side: TradeSide::Buy,
                    settlement_date: date(2026, 7, 22), // different settlement date K2!
                    notional: fixed(12_000_000),
                    quote_terms: Some(QuoteTerms {
                        basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                        settlement_amount: Some(fixed(12_000_000)),
                    }),
                    executed_at: t0.clone(),
                    batch_first_feed_seq: 12,
                    batch_last_feed_seq: 12,
                    exact_dirty_price_input: None,
                }],
                heartbeat_at: Some(t0.clone()),
                actor_id: "adapter-1".to_owned(),
            },
            &mut id_gen,
        )
        .unwrap();
    assert_eq!(ing2.missing_revisions.len(), 1);
    assert_eq!(ing2.missing_revisions[0].external_trade_id, "T-CROSS");
    assert_eq!(ing2.missing_revisions[0].source_revision, 2);
    assert_eq!(ws3.domain.feeds["feed-shared"].contiguous_feed_seq, 12);

    // [m-2] Premature manual resolution of K1 pending match MUST be blocked while T-CROSS has
    // an in-flight held rev 3 on K2!
    let k1_pos_ver = ws3.domain.accounts["acct-a"].positions["BOND-A"].position_version;
    let err_m2 = orch
        .resolve_pending_match(
            &ws3,
            &ResolvePendingMatchInput {
                account_id: "acct-a".to_owned(),
                pending_match_id: pm_k1_id,
                expected_pending_revision: 1,
                expected_position_version: k1_pos_ver,
                action: PendingMatchResolutionAction::AcceptOrphan,
                expected_earmark_revision: None,
                target_earmark_id: None,
                offset_notional: None,
                reason: "premature accept".to_owned(),
                baseline_delta: None,
                actor_id: "trader-1".to_owned(),
            },
            &mut id_gen,
        )
        .unwrap_err();
    assert_eq!(
        err_m2.detail(),
        Some(&ApplicationErrorDetail::BlotterInvalidStateTransition)
    );

    // FetchTradeRevision returning NotFound keeps trade frozen in REVISION_HOLD.
    let empty_reconciler = StubReconciler {
        revisions: BTreeMap::new(),
    };
    let (ws3_still_held, bf_nf, _) = orch
        .backfill_missing_trade_revisions(
            &ws3,
            "feed-shared",
            "adapter-1",
            &mut id_gen,
            &empty_reconciler,
        )
        .unwrap();
    assert_eq!(bf_nf.fetched_count, 0);
    assert_eq!(bf_nf.not_found_revisions.len(), 1);
    assert_eq!(
        pending_missing_trade_revisions(&ws3_still_held, "feed-shared").len(),
        1
    );

    // FetchTradeRevision returning rev 2 unblocks rev 2 -> rev 3 without rewinding contiguous_feed_seq!
    let mut rev_map = BTreeMap::new();
    rev_map.insert(
        ("feed-shared".to_owned(), "T-CROSS".to_owned(), 2),
        TradeFillRecord {
            source_id: "feed-shared".to_owned(),
            external_trade_id: "T-CROSS".to_owned(),
            source_revision: 2,
            feed_seq: 13,
            action: TradeFeedRecordAction::Amend,
            account_id: "acct-a".to_owned(),
            instrument_code: "BOND-A".to_owned(),
            side: TradeSide::Buy,
            settlement_date: date(2026, 7, 21),
            notional: fixed(11_000_000),
            quote_terms: Some(QuoteTerms {
                basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                settlement_amount: Some(fixed(11_000_000)),
            }),
            executed_at: t0.clone(),
            batch_first_feed_seq: 13,
            batch_last_feed_seq: 13,
            exact_dirty_price_input: None,
        },
    );
    let reconciler = StubReconciler { revisions: rev_map };
    let (ws4, bf_ok, bf_ev) = orch
        .backfill_missing_trade_revisions(
            &ws3_still_held,
            "feed-shared",
            "adapter-1",
            &mut id_gen,
            &reconciler,
        )
        .unwrap();
    assert_eq!(bf_ok.fetched_count, 1);
    assert!(bf_ok.not_found_revisions.is_empty());
    assert!(bf_ev.is_some());
    assert!(pending_missing_trade_revisions(&ws4, "feed-shared").is_empty());
    assert_eq!(ws4.domain.feeds["feed-shared"].contiguous_feed_seq, 13);

    // Now the surviving OPEN pending match for rev 3 can be resolved cleanly.
    let snap4 = orch.get_account_snapshot(&ws4, "acct-a").unwrap();
    let rev3_pm = snap4
        .open_pending_matches
        .iter()
        .find(|p| p.external_trade_id == "T-CROSS" && p.source_revision == 3)
        .unwrap();
    assert_eq!(rev3_pm.reason, PendingMatchReason::OrphanNoCommitment);
    let pos_ver4 = ws4.domain.accounts["acct-a"].positions["BOND-A"].position_version;
    let (ws5, res_out, _) = orch
        .resolve_pending_match(
            &ws4,
            &ResolvePendingMatchInput {
                account_id: "acct-a".to_owned(),
                pending_match_id: rev3_pm.pending_match_id.clone(),
                expected_pending_revision: rev3_pm.pending_revision,
                expected_position_version: pos_ver4,
                action: PendingMatchResolutionAction::AcceptOrphan,
                expected_earmark_revision: None,
                target_earmark_id: None,
                offset_notional: None,
                reason: "confirmed after rev2 backfill".to_owned(),
                baseline_delta: None,
                actor_id: "trader-1".to_owned(),
            },
            &mut id_gen,
        )
        .unwrap();
    assert_eq!(
        res_out.position.position.economic_net_notional,
        fixed(112_000_000)
    );

    // Exercise HOLD -> ExtendHold -> lazy-expired ConfirmHold rejected (feed paused) + SetFeedControl + SweepLifecycle.
    let pos_ver5 = ws5.domain.accounts["acct-a"].positions["BOND-A"].position_version;
    let (ws6, h_out, _) = orch
        .create_hold(
            &ws5,
            &CreateHoldCommand {
                account_id: "acct-a".to_owned(),
                earmark_id: ulid_pair('H', '9'),
                expected_position_version: pos_ver5,
                instrument_code: "BOND-A".to_owned(),
                side: TradeSide::Sell,
                settlement_bucket: SettlementBucket::TPlus0,
                notional: fixed(5_000_000),
                quote_terms: None,
                ttl_seconds: 1_800,
                note: "sell hold".to_owned(),
                trader_id: "trader-1".to_owned(),
            },
        )
        .unwrap();
    let (ws7, ext_out, _) = orch
        .extend_hold(
            &ws6,
            &ExtendHoldCommand {
                account_id: "acct-a".to_owned(),
                earmark_id: h_out.earmark.earmark_id.clone(),
                expected_earmark_revision: h_out.earmark.earmark_revision,
                ttl_seconds: 1_800,
            },
        )
        .unwrap();

    // Pause feed via SetFeedControl and advance clock to expiry -> ConfirmHold lazily expires with mapped ApplicationError!
    let feed_rev = ws7.domain.feeds["feed-shared"].feed_status_revision;
    let (ws8, fc_out, _) = orch
        .set_feed_control(
            &ws7,
            &SetFeedControlCommand {
                source_id: "feed-shared".to_owned(),
                expected_feed_status_revision: feed_rev,
                paused_by_operator: true,
                force_clear_gap_pause: false,
                reason: "maintenance".to_owned(),
            },
        )
        .unwrap();
    assert!(fc_out.feed_status.paused_by_operator);

    clock.advance_seconds(1_800).unwrap();
    let pos_ver8 = ws8.domain.accounts["acct-a"].positions["BOND-A"].position_version;
    let (ws9, conf_exp, _) = orch
        .confirm_hold(
            &ws8,
            &ConfirmHoldCommand {
                account_id: "acct-a".to_owned(),
                earmark_id: ext_out.earmark.earmark_id,
                expected_earmark_revision: ext_out.earmark.earmark_revision,
                expected_position_version: pos_ver8,
                quote_terms: QuoteTerms {
                    basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                    settlement_amount: None,
                },
                exact_dirty_price_input: None,
                trader_id: "trader-1".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(conf_exp.earmark.state, EarmarkState::Expired);
    assert_eq!(
        conf_exp
            .reconfirmation_error
            .as_ref()
            .and_then(ApplicationError::detail),
        Some(&ApplicationErrorDetail::BlotterFeedPaused)
    );

    // SweepLifecycle detects staleness on feed-shared (heartbeat > 300s old).
    let (ws10, sw1, sw_ev1) = orch
        .sweep_lifecycle(
            &ws9,
            &SweepLifecycleCommand {
                account_id: Some("acct-a".to_owned()),
                source_id: Some("feed-shared".to_owned()),
            },
        )
        .unwrap();
    assert!(sw1.mutated);
    assert_eq!(sw1.newly_stale_feed_count, 1);
    assert!(sw_ev1.is_some());

    // Repeated sweep with no transitions does NOT increment head_seq.
    let (_, sw2, sw_ev2) = orch
        .sweep_lifecycle(
            &ws10,
            &SweepLifecycleCommand {
                account_id: Some("acct-a".to_owned()),
                source_id: Some("feed-shared".to_owned()),
            },
        )
        .unwrap();
    assert!(!sw2.mutated);
    assert_eq!(sw2.head_seq, sw1.head_seq);
    assert!(sw_ev2.is_none());

    // Verify NaiveTime pre_close_alert_time usage compiles and remains clean.
    assert_eq!(
        ws10.domain.config.pre_close_alert_time,
        NaiveTime::from_hms_opt(16, 30, 0).unwrap()
    );
}

// ============================================================================
// 7. M2 Review Regression Tests: [B-1], [B-2], [m-1]
// ============================================================================

#[test]
#[allow(clippy::too_many_lines)]
fn resolve_pending_match_emits_sibling_account_change_on_shared_source_without_bumping_sibling_version()
 {
    let t0 = shanghai_time(2026, 7, 21, 10, 0, 0);
    let clock = ManualBlotterClock::new(t0.clone());
    let orch = DeskBlotterOrchestrator::new(&clock, engines());
    let units = test_units();
    let cal = make_calendar(date(2025, 1, 1), date(2035, 12, 31));
    let bond = make_bond(
        '1',
        "BOND-SHARED",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );

    let make_acct_base = |acct_id: &str, base_id: Ulid, src: &str| {
        base_opening(
            acct_id,
            base_id,
            src,
            vec![OpeningInstrumentBalance {
                instrument_code: "BOND-SHARED".to_owned(),
                opening_economic_notional: fixed(50_000_000),
                opening_available_notional: fixed(50_000_000),
                opening_net_t0_notional: fixed(0),
                opening_net_t1_notional: fixed(0),
            }],
            vec![InstrumentCatalogEntry {
                instrument_code: "BOND-SHARED".to_owned(),
                bond: Some(bond.clone()),
                eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
                calendar_requirement: None,
            }],
            Some(cal.clone()),
            Some(CalendarRequirement::ExactMarket),
            units.clone(),
        )
    };

    let ws0 = DeskBlotterWorkspaceState::new(BlotterConfig::default()).unwrap();
    // Import ACCT-B first, then ACCT-A (both on "feed-shared-b1"), plus ACCT-C on an unrelated feed.
    let (ws1, _, _) = orch
        .import_opening_base(
            &ws0,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-b1-b".to_owned(),
                opening_base: make_acct_base("ACCT-B", ulid_pair('R', '2'), "feed-shared-b1"),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();
    let (ws2, _, _) = orch
        .import_opening_base(
            &ws1,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-b1-a".to_owned(),
                opening_base: make_acct_base("ACCT-A", ulid_pair('R', '1'), "feed-shared-b1"),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();
    let (ws3, _, _) = orch
        .import_opening_base(
            &ws2,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-b1-c".to_owned(),
                opening_base: make_acct_base("ACCT-C", ulid_pair('R', '3'), "feed-other-b1"),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();

    // Ingest an orphan fill on ACCT-A -> leaves 1 OPEN pending match on shared source.
    let mut id_gen = DeterministicIdGenerator::default();
    let (ws4, ing_out, ing_ev) = orch
        .ingest_trade_feed(
            &ws3,
            &IngestTradeFeedInput {
                source_id: "feed-shared-b1".to_owned(),
                fills: vec![TradeFillRecord {
                    source_id: "feed-shared-b1".to_owned(),
                    external_trade_id: "T-B1-ORPHAN".to_owned(),
                    source_revision: 1,
                    feed_seq: 11,
                    action: TradeFeedRecordAction::NewFill,
                    account_id: "ACCT-A".to_owned(),
                    instrument_code: "BOND-SHARED".to_owned(),
                    side: TradeSide::Buy,
                    settlement_date: date(2026, 7, 21),
                    notional: fixed(10_000_000),
                    quote_terms: Some(QuoteTerms {
                        basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                        settlement_amount: Some(fixed(10_000_000)),
                    }),
                    executed_at: t0.clone(),
                    batch_first_feed_seq: 11,
                    batch_last_feed_seq: 11,
                    exact_dirty_price_input: None,
                }],
                heartbeat_at: Some(t0.clone()),
                actor_id: "adapter-1".to_owned(),
            },
            &mut id_gen,
        )
        .unwrap();
    let ing_ev = ing_ev.unwrap();
    let ing_accts: Vec<&str> = ing_ev
        .changes
        .iter()
        .map(|c| c.account_id.as_str())
        .collect();
    assert_eq!(ing_accts, vec!["ACCT-A", "ACCT-B"]);
    assert_eq!(
        ing_ev.changes[1].replace_snapshot.feed_statuses[0].open_pending_match_count,
        1
    );

    // Create a COMMITTED earmark of 4M on ACCT-A so we can test partial SplitOffset (stays OPEN)
    // followed by closing AcceptOrphan (drops open count 1 -> 0).
    let pos_ver_a = ws4.domain.accounts["ACCT-A"].positions["BOND-SHARED"].position_version;
    let (ws5, em_part, _) = orch
        .create_committed(
            &ws4,
            &CreateCommittedCommand {
                account_id: "ACCT-A".to_owned(),
                earmark_id: ulid_pair('R', '4'),
                expected_position_version: pos_ver_a,
                instrument_code: "BOND-SHARED".to_owned(),
                side: TradeSide::Buy,
                settlement_bucket: SettlementBucket::TPlus0,
                notional: fixed(4_000_000),
                quote_terms: QuoteTerms {
                    basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                    settlement_amount: Some(fixed(4_000_000)),
                },
                exact_dirty_price_input: None,
                note: "partial match target".to_owned(),
                trader_id: "trader-1".to_owned(),
            },
        )
        .unwrap();

    let pm_id = ing_out.item_outcomes[0].pending_match_id.clone().unwrap();
    let acct_b_ver_before = ws5.domain.accounts["ACCT-B"].account_version;
    let pos_ver_a5 = ws5.domain.accounts["ACCT-A"].positions["BOND-SHARED"].position_version;

    // Partial SplitOffset keeps pending match OPEN (open_pending_match_count stays 1) -> only ACCT-A in event.
    let (ws6, split_out, split_ev) = orch
        .resolve_pending_match(
            &ws5,
            &ResolvePendingMatchInput {
                account_id: "ACCT-A".to_owned(),
                pending_match_id: pm_id.clone(),
                expected_pending_revision: 1,
                expected_position_version: pos_ver_a5,
                action: PendingMatchResolutionAction::SplitOffset,
                expected_earmark_revision: Some(em_part.earmark.earmark_revision),
                target_earmark_id: Some(em_part.earmark.earmark_id.clone()),
                offset_notional: Some(fixed(4_000_000)),
                reason: "partial split".to_owned(),
                baseline_delta: None,
                actor_id: "trader-1".to_owned(),
            },
            &mut id_gen,
        )
        .unwrap();
    assert_eq!(split_out.pending_match.state, PendingMatchState::Open);
    assert_eq!(split_ev.changes.len(), 1);
    assert_eq!(split_ev.changes[0].account_id, "ACCT-A");
    assert_eq!(split_ev.changes[0].changed_allocations.len(), 1);

    // Create a 6M COMMITTED earmark so MatchFull closes the remaining 6M AND emits an allocation.
    let pos_ver_a6 = ws6.domain.accounts["ACCT-A"].positions["BOND-SHARED"].position_version;
    let (ws7, em_rest, _) = orch
        .create_committed(
            &ws6,
            &CreateCommittedCommand {
                account_id: "ACCT-A".to_owned(),
                earmark_id: ulid_pair('R', '5'),
                expected_position_version: pos_ver_a6,
                instrument_code: "BOND-SHARED".to_owned(),
                side: TradeSide::Buy,
                settlement_bucket: SettlementBucket::TPlus0,
                notional: fixed(6_000_000),
                quote_terms: QuoteTerms {
                    basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                    settlement_amount: Some(fixed(6_000_000)),
                },
                exact_dirty_price_input: None,
                note: "final match target".to_owned(),
                trader_id: "trader-1".to_owned(),
            },
        )
        .unwrap();

    let acct_a_ver_before_close = ws7.domain.accounts["ACCT-A"].account_version;
    let pos_ver_a7 = ws7.domain.accounts["ACCT-A"].positions["BOND-SHARED"].position_version;
    let (ws8, close_out, close_ev) = orch
        .resolve_pending_match(
            &ws7,
            &ResolvePendingMatchInput {
                account_id: "ACCT-A".to_owned(),
                pending_match_id: pm_id,
                expected_pending_revision: split_out.pending_match.pending_revision,
                expected_position_version: pos_ver_a7,
                action: PendingMatchResolutionAction::MatchFull,
                expected_earmark_revision: Some(em_rest.earmark.earmark_revision),
                target_earmark_id: Some(em_rest.earmark.earmark_id),
                offset_notional: None,
                reason: "close remaining via match full".to_owned(),
                baseline_delta: None,
                actor_id: "trader-1".to_owned(),
            },
            &mut id_gen,
        )
        .unwrap();

    assert_ne!(close_out.pending_match.state, PendingMatchState::Open);
    assert!(close_out.allocation.is_some());

    // Event changes MUST include both ACCT-A and ACCT-B (sorted), excluding unrelated ACCT-C.
    let close_accts: Vec<&str> = close_ev
        .changes
        .iter()
        .map(|c| c.account_id.as_str())
        .collect();
    assert_eq!(close_accts, vec!["ACCT-A", "ACCT-B"]);

    let snap_a = orch.get_account_snapshot(&ws8, "ACCT-A").unwrap();
    let snap_b = orch.get_account_snapshot(&ws8, "ACCT-B").unwrap();

    // Field-for-field snapshot parity for BOTH accounts.
    assert_eq!(close_ev.changes[0].replace_snapshot, snap_a);
    assert_eq!(close_ev.changes[1].replace_snapshot, snap_b);
    assert_eq!(snap_a.feed_statuses[0].open_pending_match_count, 0);
    assert_eq!(snap_b.feed_statuses[0].open_pending_match_count, 0);

    // changed_allocations attached ONLY to cmd.account_id (ACCT-A); ACCT-B has none.
    assert_eq!(close_ev.changes[0].changed_allocations.len(), 1);
    assert!(close_ev.changes[1].changed_allocations.is_empty());

    // ACCT-A's version incremented, while untouched sibling ACCT-B's account_version did NOT bump.
    assert_eq!(snap_a.account_version, acct_a_ver_before_close + 1);
    assert_eq!(snap_b.account_version, acct_b_ver_before);
    assert_eq!(snap_b.snapshot_seq, close_out.committed_seq);
}

#[test]
#[allow(clippy::too_many_lines)]
fn startup_policy_reconciliation_syncs_all_policy_fields_and_invalidates_stale_drafts() {
    let t0 = shanghai_time(2026, 7, 21, 10, 0, 0);
    let clock = ManualBlotterClock::new(t0.clone());
    let orch = DeskBlotterOrchestrator::new(&clock, engines());
    let units = test_units();
    let cal = make_calendar(date(2025, 1, 1), date(2035, 12, 31));
    let bond = make_bond(
        '1',
        "BOND-REC",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );

    let ws0 = DeskBlotterWorkspaceState::new(BlotterConfig::default()).unwrap();
    let initial_policy = default_policy(vec!["T", "TF"]);
    let (ws1, _, _) = orch
        .import_opening_base(
            &ws0,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-rec".to_owned(),
                opening_base: base_opening(
                    "acct-rec",
                    ulid_pair('S', '1'),
                    "feed-rec",
                    vec![OpeningInstrumentBalance {
                        instrument_code: "BOND-REC".to_owned(),
                        opening_economic_notional: fixed(50_000_000),
                        opening_available_notional: fixed(50_000_000),
                        opening_net_t0_notional: fixed(0),
                        opening_net_t1_notional: fixed(0),
                    }],
                    vec![InstrumentCatalogEntry {
                        instrument_code: "BOND-REC".to_owned(),
                        bond: Some(bond),
                        eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
                        calendar_requirement: None,
                    }],
                    Some(cal),
                    Some(CalendarRequirement::ExactMarket),
                    units,
                ),
                expected_account_version: 0,
                policy: initial_policy.clone(),
            },
        )
        .unwrap();

    // Ingest fresh quote so drafts and covered hedge previews work cleanly.
    let (ws2, _, _) = orch
        .ingest_market_quotes(
            &ws1,
            &IngestMarketQuotesInput {
                account_id: "acct-rec".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![quote_tick(
                    "q-src-a",
                    1,
                    "BOND-REC",
                    fixed_frac(0, 25_000_000_000),
                    t0.clone(),
                    t0.clone(),
                )],
            },
        )
        .unwrap();

    let submit_draft = |ws: &DeskBlotterWorkspaceState, draft_id: Ulid| {
        orch.submit_draft_earmark(
            ws,
            &SubmitDraftEarmarkCommand {
                account_id: "acct-rec".to_owned(),
                draft_id,
                proposal: make_proposal(
                    "acct-rec",
                    "BOND-REC",
                    TradeSide::Buy,
                    SettlementBucket::TPlus0,
                    fixed(10_000_000),
                    QuoteTerms {
                        basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                        settlement_amount: Some(fixed(10_000_000)),
                    },
                    DraftTargetState::Committed,
                ),
            },
        )
        .unwrap()
    };

    let try_confirm = |ws: &DeskBlotterWorkspaceState,
                       draft: &ficant_application::use_cases::blotter::DraftEarmarkRecord,
                       earmark_id: Ulid,
                       ack_uncovered: bool| {
        let pos_ver = ws.domain.accounts["acct-rec"].positions["BOND-REC"].position_version;
        orch.confirm_draft_earmark(
            ws,
            &ConfirmDraftEarmarkCommand {
                account_id: "acct-rec".to_owned(),
                draft_id: draft.draft_id.clone(),
                earmark_id,
                expected_draft_revision: draft.draft_revision,
                expected_position_version: pos_ver,
                confirmation_token: draft.confirmation_token.clone(),
                acknowledge_uncovered_dv01: ack_uncovered,
                trader_id: "trader-1".to_owned(),
            },
        )
    };

    // 0. Identical business fields (even with different policy_version) -> no-op.
    let mut noop_pol = initial_policy.clone();
    noop_pol.policy_version = 99;
    let mut cfg = BTreeMap::new();
    cfg.insert("acct-rec".to_owned(), noop_pol);
    let (ws_noop, noop_out, noop_ev) = orch.reconcile_startup_quote_sources(&ws2, &cfg).unwrap();
    assert!(!noop_out.mutated);
    assert!(noop_ev.is_none());
    assert_eq!(ws_noop.domain.head_seq, ws2.domain.head_seq);
    assert_eq!(ws_noop.accounts_meta["acct-rec"].policy.policy_version, 1);

    // 1. limit_ceilings change -> reconciles, bumps policy_version & account_version, invalidates draft.
    let (ws_d1, d1_out, _) = submit_draft(&ws_noop, ulid_pair('S', '2'));
    let mut pol1 = ws_d1.accounts_meta["acct-rec"].policy.clone();
    pol1.policy_version = 5; // higher desired version -> result must be max(1, 5) + 1 = 6
    pol1.limit_ceilings
        .insert("INST_MAX".to_owned(), fixed(180_000_000));
    cfg.insert("acct-rec".to_owned(), pol1);
    let acct_ver_before = ws_d1.domain.accounts["acct-rec"].account_version;
    let (ws_r1, r1_out, r1_ev) = orch.reconcile_startup_quote_sources(&ws_d1, &cfg).unwrap();
    assert!(r1_out.mutated);
    assert_eq!(r1_out.policy_version, 6);
    assert_eq!(ws_r1.accounts_meta["acct-rec"].policy.policy_version, 6);
    assert_eq!(
        ws_r1.accounts_meta["acct-rec"].policy.limit_ceilings["INST_MAX"],
        fixed(180_000_000)
    );
    assert_eq!(
        ws_r1.domain.accounts["acct-rec"].account_version,
        acct_ver_before + 1
    );
    let r1_ev = r1_ev.unwrap();
    assert_eq!(
        r1_ev.changes[0].replace_snapshot,
        orch.get_account_snapshot(&ws_r1, "acct-rec").unwrap()
    );
    assert_eq!(
        try_confirm(&ws_r1, &d1_out.draft, ulid_pair('T', '1'), false)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    // 2. allowed_hedge_product_codes change -> updates snapshot & PreviewCoveredHedge, invalidates draft.
    let (ws_d2, d2_out, _) = submit_draft(&ws_r1, ulid_pair('S', '3'));
    let mut pol2 = ws_d2.accounts_meta["acct-rec"].policy.clone();
    pol2.policy_version = 1; // lower desired version -> result must be max(6, 1) + 1 = 7
    pol2.allowed_hedge_product_codes = vec!["TL".to_owned()];
    cfg.insert("acct-rec".to_owned(), pol2);
    let (ws_r2, r2_out, r2_ev) = orch.reconcile_startup_quote_sources(&ws_d2, &cfg).unwrap();
    assert!(r2_out.mutated);
    assert_eq!(r2_out.policy_version, 7);
    assert!(r2_ev.is_some());
    assert_eq!(
        orch.get_account_snapshot(&ws_r2, "acct-rec")
            .unwrap()
            .allowed_hedge_product_codes,
        vec!["TL"]
    );
    assert_eq!(
        orch.preview_covered_hedge(
            &ws_r2,
            &PreviewCoveredHedgeInput {
                account_id: "acct-rec".to_owned(),
                expected_snapshot_seq: None,
                scope: PreviewHedgeScope::WholeAccount,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: fixed_frac(0, 75_000_000_000),
                conversion_factor: fixed_frac(1, 0),
                delivery_rule_pack: default_delivery_pack(),
                valuation_at: None,
            },
        )
        .unwrap_err()
        .detail(),
        Some(&ApplicationErrorDetail::BlotterHedgeProductNotAllowed)
    );
    assert_eq!(
        try_confirm(&ws_r2, &d2_out.draft, ulid_pair('T', '2'), false)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    // 3. quote_staleness_seconds change -> syncs & invalidates draft.
    let (ws_d3, d3_out, _) = submit_draft(&ws_r2, ulid_pair('S', '4'));
    let mut pol3 = ws_d3.accounts_meta["acct-rec"].policy.clone();
    pol3.quote_staleness_seconds = 120;
    cfg.insert("acct-rec".to_owned(), pol3);
    let (ws_r3, r3_out, r3_ev) = orch.reconcile_startup_quote_sources(&ws_d3, &cfg).unwrap();
    assert!(r3_out.mutated);
    assert_eq!(r3_out.policy_version, 8);
    assert!(r3_ev.is_some());
    assert_eq!(
        ws_r3.accounts_meta["acct-rec"]
            .policy
            .quote_staleness_seconds,
        120
    );
    assert_eq!(
        try_confirm(&ws_r3, &d3_out.draft, ulid_pair('T', '3'), false)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    // 4. default_draft_hold_ttl_seconds change -> syncs & invalidates draft.
    let (ws_d4, d4_out, _) = submit_draft(&ws_r3, ulid_pair('S', '5'));
    let mut pol4 = ws_d4.accounts_meta["acct-rec"].policy.clone();
    pol4.default_draft_hold_ttl_seconds = 2_400;
    cfg.insert("acct-rec".to_owned(), pol4);
    let (ws_r4, r4_out, r4_ev) = orch.reconcile_startup_quote_sources(&ws_d4, &cfg).unwrap();
    assert!(r4_out.mutated);
    assert_eq!(r4_out.policy_version, 9);
    assert!(r4_ev.is_some());
    assert_eq!(
        ws_r4.accounts_meta["acct-rec"]
            .policy
            .default_draft_hold_ttl_seconds,
        2_400
    );
    assert_eq!(
        try_confirm(&ws_r4, &d4_out.draft, ulid_pair('T', '4'), false)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    // 5. regulatory_rule_pack upgrade -> syncs & invalidates stale draft with ConcurrencyConflict.
    let (ws_d5, d5_out, _) = submit_draft(&ws_r4, ulid_pair('S', '6'));
    let upgraded_pack = make_regulatory_pack(
        2,
        &serde_json::json!({
            "constraints": [
                {"limit_code": "INST_MAX", "measure": "INSTRUMENT_FACE"},
                {"limit_code": "PORT_MAX", "measure": "PORTFOLIO_FACE"},
                {"limit_code": "BORROW_MAX", "measure": "BORROWED_CASH"}
            ]
        }),
        VerificationStatus::Verified,
        shanghai_time(2026, 1, 1, 0, 0, 0),
        shanghai_time(2027, 1, 1, 0, 0, 0),
    );
    let mut pol5 = ws_d5.accounts_meta["acct-rec"].policy.clone();
    pol5.policy_version = 1;
    pol5.regulatory_rule_pack = Some(upgraded_pack.clone());
    cfg.insert("acct-rec".to_owned(), pol5);
    let (ws_r5, r5_out, r5_ev) = orch.reconcile_startup_quote_sources(&ws_d5, &cfg).unwrap();
    assert!(r5_out.mutated);
    assert_eq!(r5_out.policy_version, 10);
    assert!(r5_ev.is_some());
    assert_eq!(
        ws_r5.accounts_meta["acct-rec"].policy.regulatory_rule_pack,
        Some(upgraded_pack)
    );
    assert_eq!(
        try_confirm(&ws_r5, &d5_out.draft, ulid_pair('T', '5'), false)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );

    // 6. regulatory_rule_pack removed on restart -> reconciles & returns rule error on confirm
    //    even with acknowledge_uncovered_dv01 = true.
    let (ws_d6, d6_out, _) = submit_draft(&ws_r5, ulid_pair('S', '7'));
    let mut pol6 = ws_d6.accounts_meta["acct-rec"].policy.clone();
    pol6.regulatory_rule_pack = None;
    cfg.insert("acct-rec".to_owned(), pol6);
    let (ws_r6, r6_out, r6_ev) = orch.reconcile_startup_quote_sources(&ws_d6, &cfg).unwrap();
    assert!(r6_out.mutated);
    assert_eq!(r6_out.policy_version, 11);
    assert!(r6_ev.is_some());
    assert_eq!(
        try_confirm(&ws_r6, &d6_out.draft, ulid_pair('T', '6'), true)
            .unwrap_err()
            .detail(),
        Some(&ApplicationErrorDetail::BlotterRulePackInvalid)
    );
}

#[test]
fn preview_covered_hedge_zero_net_exposure_rejects_underflowed_zero_contract_dv01() {
    let t0 = shanghai_time(2026, 7, 21, 10, 0, 0);
    let clock = ManualBlotterClock::new(t0.clone());
    let orch = DeskBlotterOrchestrator::new(&clock, engines());
    let units = test_units();
    let cal = make_calendar(date(2025, 1, 1), date(2035, 12, 31));
    let bond_long = make_bond(
        '1',
        "BOND-LONG",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );
    let bond_short = make_bond(
        '2',
        "BOND-SHORT",
        date(2025, 8, 8),
        date(2030, 8, 8),
        250,
        100,
        &units,
    );

    let ws0 = DeskBlotterWorkspaceState::new(BlotterConfig::default()).unwrap();
    let (ws1, _, _) = orch
        .import_opening_base(
            &ws0,
            &ImportFullOpeningBaseCommand {
                actor_id: "op-1".to_owned(),
                idempotency_key: "imp-zero-net".to_owned(),
                opening_base: base_opening(
                    "acct-zero",
                    ulid_pair('Z', '1'),
                    "feed-zero",
                    vec![
                        OpeningInstrumentBalance {
                            instrument_code: "BOND-LONG".to_owned(),
                            opening_economic_notional: fixed(50_000_000),
                            opening_available_notional: fixed(50_000_000),
                            opening_net_t0_notional: fixed(0),
                            opening_net_t1_notional: fixed(0),
                        },
                        OpeningInstrumentBalance {
                            instrument_code: "BOND-SHORT".to_owned(),
                            opening_economic_notional: fixed(-50_000_000),
                            opening_available_notional: fixed(0),
                            opening_net_t0_notional: fixed(0),
                            opening_net_t1_notional: fixed(0),
                        },
                    ],
                    vec![
                        InstrumentCatalogEntry {
                            instrument_code: "BOND-LONG".to_owned(),
                            bond: Some(bond_long),
                            eligibility_class: Some(
                                RateCoverageEligibility::SovereignOrPolicyFixed,
                            ),
                            calendar_requirement: None,
                        },
                        InstrumentCatalogEntry {
                            instrument_code: "BOND-SHORT".to_owned(),
                            bond: Some(bond_short),
                            eligibility_class: Some(
                                RateCoverageEligibility::SovereignOrPolicyFixed,
                            ),
                            calendar_requirement: None,
                        },
                    ],
                    Some(cal),
                    Some(CalendarRequirement::ExactMarket),
                    units,
                ),
                expected_account_version: 0,
                policy: default_policy(vec!["T", "TF", "TL"]),
            },
        )
        .unwrap();

    let (ws2, _, _) = orch
        .ingest_market_quotes(
            &ws1,
            &IngestMarketQuotesInput {
                account_id: "acct-zero".to_owned(),
                source_id: "q-src-a".to_owned(),
                ticks: vec![
                    quote_tick(
                        "q-src-a",
                        1,
                        "BOND-LONG",
                        fixed_frac(0, 25_000_000_000),
                        t0.clone(),
                        t0.clone(),
                    ),
                    quote_tick(
                        "q-src-a",
                        1,
                        "BOND-SHORT",
                        fixed_frac(0, 25_000_000_000),
                        t0.clone(),
                        t0,
                    ),
                ],
            },
        )
        .unwrap();

    // Baseline zero-net-exposure preview succeeds when futures_contract_dv01 > 0.
    let ok_out = orch
        .preview_covered_hedge(
            &ws2,
            &PreviewCoveredHedgeInput {
                account_id: "acct-zero".to_owned(),
                expected_snapshot_seq: None,
                scope: PreviewHedgeScope::WholeAccount,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: fixed_frac(0, 75_000_000_000),
                conversion_factor: fixed(1),
                delivery_rule_pack: default_delivery_pack(),
                valuation_at: None,
            },
        )
        .unwrap();
    assert!(ok_out.zero_net_exposure);
    assert_eq!(ok_out.covered_instrument_count, 2);
    assert_eq!(ok_out.covered_net_pre_tax_dv01, FixedDecimal::ZERO);
    assert!(ok_out.measures.futures_contract_dv01.is_positive());

    // Degenerate tiny ctd_dv01_per_100 / huge conversion_factor rounding futures_contract_dv01 to 0
    // MUST fail closed with ValidationFailed matching the non-zero exposure path.
    let err = orch
        .preview_covered_hedge(
            &ws2,
            &PreviewCoveredHedgeInput {
                account_id: "acct-zero".to_owned(),
                expected_snapshot_seq: None,
                scope: PreviewHedgeScope::WholeAccount,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: FixedDecimal::from_scaled(1),
                conversion_factor: fixed(100_000),
                delivery_rule_pack: default_delivery_pack(),
                valuation_at: None,
            },
        )
        .unwrap_err();
    assert_eq!(err.category(), ApplicationErrorCategory::ValidationFailed);
}
