use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{Datelike, NaiveDate, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Shanghai;
use ficant_api::RatesGrpcService;
use ficant_application::ports::{BondAnalyticsEngine, CarryRollEngine, FuturesHedgeEngine};
use ficant_cgb_futures_pack::{
    CgbFuturesDeliveryRulePackParser, MARKET as FUTURES_MARKET, RULE_TYPE as FUTURES_RULE_TYPE,
    TYPE_URL as FUTURES_TYPE_URL,
};
use ficant_contracts::ficant::core::v1 as core_pb;
use ficant_contracts::ficant::market::v1 as market_pb;
use ficant_contracts::ficant::rates::v1 as pb;
use ficant_contracts::ficant::rates::v1::rates_analytics_service_server::RatesAnalyticsService;
use ficant_domain::analytics::{
    AnalyticsError, BondAnalyticsInput, BondAnalyticsResult, FixedDecimal,
};
use ficant_domain::curves::{CarryRollInput, CarryRollResult};
use ficant_domain::futures_hedge::{FuturesHedgeInput, FuturesHedgeResult};
use ficant_domain::market::{BondTaxAttributes, IncomeTaxStatus, ValueAddedTaxStatus};
use ficant_domain::primitives::ContentHash;
use ficant_fixed_income_native::{
    NativeBondAnalyticsEngine, NativeCarryRollEngine, NativeFuturesDeliveryEngine,
    NativeFuturesHedgeEngine, NativeYieldCurveEngine,
};
use ficant_funding_pack::FundingRulePackV1Parser;
use ficant_tax_pack::{
    MARKET as TAX_MARKET, RATE_UNIT_ID as TAX_RATE_UNIT_ID, RULE_TYPE as TAX_RULE_TYPE,
    SOURCE as TAX_SOURCE, TYPE_URL_V2 as TAX_TYPE_URL_V2, TaxRulePackV2Parser,
};
use prost::Message;
use rust_decimal::Decimal;
use std::str::FromStr;
use tonic::Request;

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

struct RecordingBondEngine {
    calls: Mutex<Vec<BondAnalyticsInput>>,
}

impl RecordingBondEngine {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }

    fn take(&self) -> Vec<BondAnalyticsInput> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl BondAnalyticsEngine for RecordingBondEngine {
    fn calculate(&self, input: &BondAnalyticsInput) -> Result<BondAnalyticsResult, AnalyticsError> {
        self.calls.lock().unwrap().push(input.clone());
        NativeBondAnalyticsEngine.calculate(input)
    }
}

struct RecordingHedgeEngine {
    calls: Mutex<Vec<FuturesHedgeInput>>,
}

impl RecordingHedgeEngine {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }

    fn take(&self) -> Vec<FuturesHedgeInput> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl FuturesHedgeEngine for RecordingHedgeEngine {
    fn calculate(&self, input: &FuturesHedgeInput) -> Result<FuturesHedgeResult, AnalyticsError> {
        self.calls.lock().unwrap().push(input.clone());
        NativeFuturesHedgeEngine.calculate(input)
    }
}

struct RecordingCarryRollEngine {
    calls: Mutex<Vec<CarryRollInput>>,
}

impl RecordingCarryRollEngine {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl CarryRollEngine for RecordingCarryRollEngine {
    fn calculate(&self, input: &CarryRollInput) -> Result<CarryRollResult, AnalyticsError> {
        self.calls.lock().unwrap().push(input.clone());
        NativeCarryRollEngine.calculate(input)
    }
}

struct TestHarness {
    service: RatesGrpcService,
    bond_recorder: Arc<RecordingBondEngine>,
    hedge_recorder: Arc<RecordingHedgeEngine>,
}

impl TestHarness {
    fn new() -> Self {
        let bond_recorder = Arc::new(RecordingBondEngine::new());
        let hedge_recorder = Arc::new(RecordingHedgeEngine::new());
        let carry_recorder = Arc::new(RecordingCarryRollEngine::new());
        let service = RatesGrpcService::new(
            bond_recorder.clone(),
            Arc::new(NativeYieldCurveEngine),
            carry_recorder,
            Arc::new(NativeFuturesDeliveryEngine),
            hedge_recorder.clone(),
            Arc::new(CgbFuturesDeliveryRulePackParser),
            Arc::new(FundingRulePackV1Parser),
            Arc::new(TaxRulePackV2Parser),
        );
        Self {
            service,
            bond_recorder,
            hedge_recorder,
        }
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn ulid(suffix: char) -> core_pb::Ulid {
    core_pb::Ulid {
        value: format!("{ULID_PREFIX}{suffix}"),
    }
}

fn unit(suffix: char) -> core_pb::UnitRef {
    core_pb::UnitRef {
        unit_id: Some(ulid(suffix)),
        version: 1,
    }
}

fn rate_unit() -> core_pb::UnitRef {
    core_pb::UnitRef {
        unit_id: Some(core_pb::Ulid {
            value: TAX_RATE_UNIT_ID.to_owned(),
        }),
        version: 1,
    }
}

fn units() -> pb::AnalysisUnits {
    pb::AnalysisUnits {
        currency_amount: Some(unit('A')),
        price_per_100: Some(unit('B')),
        rate: Some(rate_unit()),
        years: Some(unit('D')),
        years_squared: Some(unit('E')),
        dv01_per_100: Some(unit('F')),
        dv01: Some(unit('G')),
        dimensionless: Some(unit('H')),
        contract_count: Some(unit('J')),
    }
}

fn dec(text: &str, u: &core_pb::UnitRef) -> core_pb::DecimalValue {
    let parsed = Decimal::from_str(text).expect("valid decimal");
    let normalized = parsed.normalize();
    core_pb::DecimalValue {
        coefficient: normalized.mantissa().to_string(),
        scale: normalized.scale(),
        unit: Some(u.clone()),
    }
}

fn dec_to_string_12(v: &core_pb::DecimalValue) -> String {
    let mantissa: i128 = v.coefficient.parse().expect("valid integer coefficient");
    let d = Decimal::from_i128_with_scale(mantissa, v.scale);
    format!("{d:.12}")
}

fn dec_to_decimal(v: &core_pb::DecimalValue) -> Decimal {
    let mantissa: i128 = v.coefficient.parse().expect("valid integer coefficient");
    Decimal::from_i128_with_scale(mantissa, v.scale)
}

fn shanghai_market_time(date: &str, hour: u32, min: u32, sec: u32) -> core_pb::MarketTime {
    let naive = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
    let local = Shanghai
        .from_local_datetime(&naive.and_hms_opt(hour, min, sec).unwrap())
        .single()
        .unwrap();
    let utc = local.with_timezone(&Utc);
    core_pb::MarketTime {
        instant: Some(prost_types::Timestamp {
            seconds: utc.timestamp(),
            nanos: i32::try_from(utc.timestamp_subsec_nanos()).unwrap(),
        }),
        market_timezone: "Asia/Shanghai".to_owned(),
        local_trading_date: date.to_owned(),
    }
}

fn futures_v2_raw() -> Vec<u8> {
    fs::read(repo_root().join("domain-packs/cgb-futures/cgb-futures-v2.bin"))
        .expect("cgb-futures-v2.bin exists")
}

fn futures_pack(raw: Vec<u8>, version: u64) -> market_pb::MarketRulePack {
    let digest = ContentHash::digest(&raw).as_bytes().to_vec();
    market_pb::MarketRulePack {
        rule_pack_id: Some(ulid('F')),
        version,
        owner: None,
        market: FUTURES_MARKET.to_owned(),
        rule_type: FUTURES_RULE_TYPE.to_owned(),
        source: "cffex-rulebook".to_owned(),
        effective_from: Some(shanghai_market_time("2026-01-01", 0, 0, 0)),
        effective_to: Some(shanghai_market_time("2028-01-01", 0, 0, 0)),
        verification_status: market_pb::VerificationStatus::Verified as i32,
        content_hash: Some(core_pb::Sha256 { value: digest }),
        content: Some(prost_types::Any {
            type_url: FUTURES_TYPE_URL.to_owned(),
            value: raw,
        }),
    }
}

fn tax_pack() -> market_pb::MarketRulePack {
    let raw = fs::read(repo_root().join("domain-packs/cgb-interest-tax/cgb-interest-tax-v1.bin"))
        .expect("cgb-interest-tax-v1.bin exists");
    let digest = ContentHash::digest(&raw).as_bytes().to_vec();
    market_pb::MarketRulePack {
        rule_pack_id: Some(ulid('T')),
        version: 1,
        owner: None,
        market: TAX_MARKET.to_owned(),
        rule_type: TAX_RULE_TYPE.to_owned(),
        source: TAX_SOURCE.to_owned(),
        effective_from: Some(shanghai_market_time("2026-01-01", 0, 0, 0)),
        effective_to: Some(shanghai_market_time("2028-01-01", 0, 0, 0)),
        verification_status: market_pb::VerificationStatus::Verified as i32,
        content_hash: Some(core_pb::Sha256 { value: digest }),
        content: Some(prost_types::Any {
            type_url: TAX_TYPE_URL_V2.to_owned(),
            value: raw,
        }),
    }
}

fn tax_context(pack: market_pb::MarketRulePack) -> pb::BondTaxContext {
    pb::BondTaxContext {
        subject_tax_treatment: Some(core_pb::TaxTreatment {
            value_added_tax_profile: "cn-vat-general-taxpayer".to_owned(),
            income_tax_profile: "cn-cgb-interest-cit-exempt".to_owned(),
        }),
        tax_rule_pack: Some(pack),
    }
}

fn ts_hedge_request(
    pack: Option<market_pb::MarketRulePack>,
    valuation_at: Option<core_pb::MarketTime>,
) -> pb::AnalyzeFuturesHedgeRequest {
    let u = units();
    pb::AnalyzeFuturesHedgeRequest {
        units: Some(u.clone()),
        product_code: "TS".to_owned(),
        target_dv01: Some(dec("1234.50", u.dv01.as_ref().unwrap())),
        ctd_dv01_per_100: Some(dec("0.0145", u.dv01_per_100.as_ref().unwrap())),
        conversion_factor: Some(dec("0.9987", u.dimensionless.as_ref().unwrap())),
        delivery_rule_pack: pack,
        valuation_at,
    }
}

async fn call_hedge_ok(
    service: &RatesGrpcService,
    req: pb::AnalyzeFuturesHedgeRequest,
) -> pb::AnalyzeFuturesHedgeResult {
    let resp = service
        .analyze_futures_hedge(Request::new(req))
        .await
        .expect("gRPC transport ok")
        .into_inner();
    match resp.result {
        Some(pb::analyze_futures_hedge_response::Result::Analysis(a)) => a,
        other => panic!("expected Analysis, got {other:?}"),
    }
}

async fn call_hedge_err(
    service: &RatesGrpcService,
    req: pb::AnalyzeFuturesHedgeRequest,
) -> core_pb::ErrorCode {
    let resp = service
        .analyze_futures_hedge(Request::new(req))
        .await
        .expect("gRPC transport ok")
        .into_inner();
    match resp.result {
        Some(pb::analyze_futures_hedge_response::Result::Error(e)) => {
            core_pb::ErrorCode::try_from(e.code).expect("valid error code")
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

async fn call_bond_ok(
    service: &RatesGrpcService,
    req: pb::AnalyzeBondRequest,
) -> pb::AnalyzeBondResult {
    let resp = service
        .analyze_bond(Request::new(req))
        .await
        .expect("gRPC transport ok")
        .into_inner();
    match resp.result {
        Some(pb::analyze_bond_response::Result::Analysis(a)) => a,
        other => panic!("expected Analysis, got {other:?}"),
    }
}

async fn call_bond_err(
    service: &RatesGrpcService,
    req: pb::AnalyzeBondRequest,
) -> core_pb::ErrorCode {
    let resp = service
        .analyze_bond(Request::new(req))
        .await
        .expect("gRPC transport ok")
        .into_inner();
    match resp.result {
        Some(pb::analyze_bond_response::Result::Error(e)) => {
            core_pb::ErrorCode::try_from(e.code).expect("valid error code")
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

fn cfets_2024_2026_calendar(end_date: NaiveDate) -> market_pb::Calendar {
    // Authoritative 2024-2026 CFETS interbank bond market notices:
    // 2024: 中汇交发〔2023〕301号
    // 2025: 中汇交发〔2024〕358号
    // 2026: 中汇交发〔2025〕460号
    let closed_ranges = [
        // 2024
        ("2024-01-01", "2024-01-01"),
        ("2024-02-10", "2024-02-17"),
        ("2024-04-04", "2024-04-06"),
        ("2024-05-01", "2024-05-05"),
        ("2024-06-10", "2024-06-10"),
        ("2024-09-15", "2024-09-17"),
        ("2024-10-01", "2024-10-07"),
        // 2025
        ("2025-01-01", "2025-01-01"),
        ("2025-01-28", "2025-02-04"),
        ("2025-04-04", "2025-04-06"),
        ("2025-05-01", "2025-05-05"),
        ("2025-05-31", "2025-06-02"),
        ("2025-10-01", "2025-10-08"),
        // 2026
        ("2026-01-01", "2026-01-03"),
        ("2026-02-15", "2026-02-23"),
        ("2026-04-04", "2026-04-06"),
        ("2026-05-01", "2026-05-05"),
        ("2026-06-19", "2026-06-21"),
        ("2026-09-25", "2026-09-27"),
        ("2026-10-01", "2026-10-07"),
    ];
    let open_weekends: BTreeSet<NaiveDate> = [
        // 2024
        "2024-02-04",
        "2024-02-18",
        "2024-04-07",
        "2024-04-28",
        "2024-05-11",
        "2024-09-14",
        "2024-09-29",
        "2024-10-12",
        // 2025
        "2025-01-26",
        "2025-02-08",
        "2025-04-27",
        "2025-09-28",
        "2025-10-11",
        // 2026
        "2026-01-04",
        "2026-02-14",
        "2026-02-28",
        "2026-05-09",
        "2026-09-20",
        "2026-10-10",
    ]
    .into_iter()
    .map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap())
    .collect();

    let mut holidays = BTreeSet::new();
    for (start_s, end_s) in closed_ranges {
        let mut d = NaiveDate::parse_from_str(start_s, "%Y-%m-%d").unwrap();
        let last = NaiveDate::parse_from_str(end_s, "%Y-%m-%d").unwrap();
        while d <= last {
            holidays.insert(d);
            d = d.succ_opt().unwrap();
        }
    }

    let mut sessions = Vec::new();
    let mut cur = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
    while cur <= end_date {
        let is_weekend = matches!(cur.weekday(), Weekday::Sat | Weekday::Sun);
        let closed = if holidays.contains(&cur) {
            true
        } else if open_weekends.contains(&cur) {
            false
        } else {
            is_weekend
        };
        sessions.push(market_pb::CalendarSession {
            local_date: cur.to_string(),
            open_local_time: if closed {
                String::new()
            } else {
                "09:00:00".to_owned()
            },
            close_local_time: if closed {
                String::new()
            } else {
                "17:00:00".to_owned()
            },
            closed,
        });
        cur = cur.succ_opt().unwrap();
    }

    market_pb::Calendar {
        calendar_id: Some(ulid('C')),
        version: 1,
        owner: None,
        market: "CFETS".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective_from: Some(shanghai_market_time("2024-01-01", 0, 0, 0)),
        effective_to: Some(shanghai_market_time("2027-01-01", 0, 0, 0)),
        sessions,
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn hedge_binds_real_rule_pack_fingerprint_and_reflects_ts_contract_size_mutation() {
    let harness = TestHarness::new();
    let raw_v2 = futures_v2_raw();
    let hash_v2 = ContentHash::digest(&raw_v2);
    let val_at = shanghai_market_time("2026-06-15", 10, 0, 0);

    // 1. Baseline TS request repeated twice -> reproducible fingerprint, real rule_pack ref, 2M notional.
    let req_base = ts_hedge_request(Some(futures_pack(raw_v2.clone(), 1)), Some(val_at.clone()));
    let res_1 = call_hedge_ok(&harness.service, req_base.clone()).await;
    let res_2 = call_hedge_ok(&harness.service, req_base).await;
    let inputs = harness.hedge_recorder.take();
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0].fingerprint(), inputs[1].fingerprint());
    assert_eq!(
        inputs[0].rule_pack().version_ref().id().as_str(),
        ulid('F').value
    );
    assert_eq!(inputs[0].rule_pack().version_ref().version().get(), 1);
    assert_eq!(inputs[0].rule_pack().content_hash(), &hash_v2);
    assert!(inputs[0].valuation_at().is_some());

    let m1 = res_1.measures.as_ref().expect("measures present");
    assert_eq!(m1, res_2.measures.as_ref().unwrap());
    assert_eq!(
        dec_to_string_12(m1.futures_contract_dv01.as_ref().unwrap()),
        "290.377490737959"
    );
    assert_eq!(m1.recommended_contracts, -4);
    let notional_1 = m1
        .contract_notional
        .as_ref()
        .expect("contract_notional echoed");
    assert_eq!(dec_to_decimal(notional_1), Decimal::from(2_000_000));
    assert_eq!(notional_1.unit, units().currency_amount);

    // 2. Bump only pack version (1 -> 2), same content/hash -> same numbers, different fingerprint.
    let versioned_req =
        ts_hedge_request(Some(futures_pack(raw_v2.clone(), 2)), Some(val_at.clone()));
    let versioned_reply = call_hedge_ok(&harness.service, versioned_req).await;
    let versioned_input = harness.hedge_recorder.take().pop().unwrap();
    assert_eq!(
        versioned_input.rule_pack().version_ref().id().as_str(),
        ulid('F').value
    );
    assert_eq!(versioned_input.rule_pack().version_ref().version().get(), 2);
    assert_eq!(versioned_input.rule_pack().content_hash(), &hash_v2);
    assert_ne!(versioned_input.fingerprint(), inputs[0].fingerprint());
    let versioned_measures = versioned_reply.measures.as_ref().unwrap();
    assert_eq!(
        versioned_measures.futures_contract_dv01,
        m1.futures_contract_dv01
    );
    assert_eq!(
        versioned_measures.recommended_contracts,
        m1.recommended_contracts
    );

    // 3. Mutate only unselected product TF's valid contract size and recompute hash -> same numbers, different fingerprint.
    let mut unselected_payload =
        market_pb::CgbFuturesDeliveryRulePack::decode(raw_v2.as_slice()).unwrap();
    unselected_payload
        .products
        .iter_mut()
        .find(|p| p.product_code.as_deref() == Some("TF"))
        .unwrap()
        .contract_size_in_quote_units = Some(15_000);
    let unselected_raw = unselected_payload.encode_to_vec();
    let unselected_hash = ContentHash::digest(&unselected_raw);
    assert_ne!(unselected_hash, hash_v2);
    let unselected_reply = call_hedge_ok(
        &harness.service,
        ts_hedge_request(Some(futures_pack(unselected_raw, 1)), Some(val_at.clone())),
    )
    .await;
    let unselected_input = harness.hedge_recorder.take().pop().unwrap();
    assert_eq!(
        unselected_input.rule_pack().version_ref().version().get(),
        1
    );
    assert_eq!(
        unselected_input.rule_pack().content_hash(),
        &unselected_hash
    );
    assert_ne!(unselected_input.fingerprint(), inputs[0].fingerprint());
    let unselected_measures = unselected_reply.measures.as_ref().unwrap();
    assert_eq!(
        unselected_measures.futures_contract_dv01,
        m1.futures_contract_dv01
    );
    assert_eq!(
        unselected_measures.recommended_contracts,
        m1.recommended_contracts
    );

    // 4. Mutate selected TS size 20,000 -> 10,000 and recompute hash -> DV01 halves & contracts -4 -> -9.
    let mut selected_payload =
        market_pb::CgbFuturesDeliveryRulePack::decode(raw_v2.as_slice()).unwrap();
    selected_payload
        .products
        .iter_mut()
        .find(|p| p.product_code.as_deref() == Some("TS"))
        .unwrap()
        .contract_size_in_quote_units = Some(10_000);
    let selected_raw = selected_payload.encode_to_vec();
    let selected_hash = ContentHash::digest(&selected_raw);
    let selected_reply = call_hedge_ok(
        &harness.service,
        ts_hedge_request(Some(futures_pack(selected_raw, 1)), Some(val_at)),
    )
    .await;
    let selected_input = harness.hedge_recorder.take().pop().unwrap();
    assert_eq!(selected_input.rule_pack().content_hash(), &selected_hash);
    assert_eq!(
        selected_input.contract_notional(),
        FixedDecimal::from_scaled(1_000_000_i128 * 1_000_000_000_000)
    );
    let selected_measures = selected_reply.measures.as_ref().unwrap();
    assert_eq!(
        dec_to_string_12(selected_measures.futures_contract_dv01.as_ref().unwrap()),
        "145.188745368980"
    );
    assert_eq!(selected_measures.recommended_contracts, -9);
    let notional_mut = selected_measures.contract_notional.as_ref().unwrap();
    assert_eq!(dec_to_decimal(notional_mut), Decimal::from(1_000_000));
    assert_eq!(notional_mut.unit, units().currency_amount);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn hedge_enforces_strict_envelope_and_half_open_optional_valuation_at() {
    let harness = TestHarness::new();
    let raw_v2 = futures_v2_raw();
    let base_pack = futures_pack(raw_v2, 1);

    // Missing pack -> ValidationFailed
    assert_eq!(
        call_hedge_err(&harness.service, ts_hedge_request(None, None)).await,
        core_pb::ErrorCode::ValidationFailed
    );

    // Missing content_hash -> ValidationFailed
    let mut no_hash = base_pack.clone();
    no_hash.content_hash = None;
    assert_eq!(
        call_hedge_err(&harness.service, ts_hedge_request(Some(no_hash), None)).await,
        core_pb::ErrorCode::ValidationFailed
    );

    // Wrong length content_hash -> ValidationFailed
    let mut short_hash = base_pack.clone();
    short_hash.content_hash = Some(core_pb::Sha256 {
        value: vec![0xAA; 16],
    });
    assert_eq!(
        call_hedge_err(&harness.service, ts_hedge_request(Some(short_hash), None)).await,
        core_pb::ErrorCode::ValidationFailed
    );

    // Wrong 32-byte content_hash -> HashMismatch
    let mut wrong_hash = base_pack.clone();
    wrong_hash.content_hash = Some(core_pb::Sha256 {
        value: vec![0xFF; 32],
    });
    assert_eq!(
        call_hedge_err(&harness.service, ts_hedge_request(Some(wrong_hash), None)).await,
        core_pb::ErrorCode::HashMismatch
    );

    // Unverified / Rejected status -> ValidationFailed
    for status in [
        market_pb::VerificationStatus::Unverified as i32,
        market_pb::VerificationStatus::Rejected as i32,
    ] {
        let mut bad_status = base_pack.clone();
        bad_status.verification_status = status;
        assert_eq!(
            call_hedge_err(&harness.service, ts_hedge_request(Some(bad_status), None)).await,
            core_pb::ErrorCode::ValidationFailed
        );
    }

    // Wrong market, rule_type, type_url -> ValidationFailed
    let mut bad_market = base_pack.clone();
    bad_market.market = "OTHER".to_owned();
    assert_eq!(
        call_hedge_err(&harness.service, ts_hedge_request(Some(bad_market), None)).await,
        core_pb::ErrorCode::ValidationFailed
    );

    let mut bad_rule_type = base_pack.clone();
    bad_rule_type.rule_type = "tax".to_owned();
    assert_eq!(
        call_hedge_err(
            &harness.service,
            ts_hedge_request(Some(bad_rule_type), None)
        )
        .await,
        core_pb::ErrorCode::ValidationFailed
    );

    let mut bad_type_url = base_pack.clone();
    bad_type_url.content.as_mut().unwrap().type_url =
        "type.googleapis.com/ficant.market.v1.TaxRulePackV2".to_owned();
    assert_eq!(
        call_hedge_err(&harness.service, ts_hedge_request(Some(bad_type_url), None)).await,
        core_pb::ErrorCode::ValidationFailed
    );

    // Unknown product_code -> ValidationFailed
    let mut bad_product = ts_hedge_request(Some(base_pack.clone()), None);
    bad_product.product_code = "UNKNOWN".to_owned();
    assert_eq!(
        call_hedge_err(&harness.service, bad_product).await,
        core_pb::ErrorCode::ValidationFailed
    );

    // Left boundary inclusive (at == effective_from) -> succeeds
    let left_at = shanghai_market_time("2026-01-01", 0, 0, 0);
    let _ = call_hedge_ok(
        &harness.service,
        ts_hedge_request(Some(base_pack.clone()), Some(left_at)),
    )
    .await;
    let input_left = harness.hedge_recorder.take().pop().unwrap();
    assert!(input_left.valuation_at().is_some());

    // Right boundary exclusive (at == effective_to) -> ValidationFailed
    let right_at = shanghai_market_time("2028-01-01", 0, 0, 0);
    assert_eq!(
        call_hedge_err(
            &harness.service,
            ts_hedge_request(Some(base_pack.clone()), Some(right_at))
        )
        .await,
        core_pb::ErrorCode::ValidationFailed
    );

    // Before left boundary -> ValidationFailed
    let before_at = shanghai_market_time("2025-12-31", 23, 59, 59);
    assert_eq!(
        call_hedge_err(
            &harness.service,
            ts_hedge_request(Some(base_pack.clone()), Some(before_at))
        )
        .await,
        core_pb::ErrorCode::ValidationFailed
    );

    // Wrong timezone -> ValidationFailed
    let utc_at = core_pb::MarketTime {
        instant: Some(prost_types::Timestamp {
            seconds: Utc
                .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
                .unwrap()
                .timestamp(),
            nanos: 0,
        }),
        market_timezone: "UTC".to_owned(),
        local_trading_date: "2026-06-15".to_owned(),
    };
    assert_eq!(
        call_hedge_err(
            &harness.service,
            ts_hedge_request(Some(base_pack.clone()), Some(utc_at))
        )
        .await,
        core_pb::ErrorCode::ValidationFailed
    );

    // Optional valuation_at == None -> pro-forma succeeds, valuation_at recorded as None, different fingerprint
    let _ = call_hedge_ok(
        &harness.service,
        ts_hedge_request(Some(base_pack.clone()), None),
    )
    .await;
    let input_none = harness.hedge_recorder.take().pop().unwrap();
    assert!(input_none.valuation_at().is_none());
    assert_ne!(input_none.fingerprint(), input_left.fingerprint());

    // Even when valuation_at == None, invalid envelope window (from >= to) must be rejected
    let mut inverted_window = base_pack;
    inverted_window.effective_from = Some(shanghai_market_time("2028-01-01", 0, 0, 0));
    inverted_window.effective_to = Some(shanghai_market_time("2026-01-01", 0, 0, 0));
    assert_eq!(
        call_hedge_err(
            &harness.service,
            ts_hedge_request(Some(inverted_window), None)
        )
        .await,
        core_pb::ErrorCode::ValidationFailed
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn bond_seven_cell_tax_matrix_and_r5e_nonzero_tax_difference() {
    let harness = TestHarness::new();
    let u = units();

    let taxable_bond = market_pb::Bond {
        instrument: Some(core_pb::VersionRef {
            id: Some(ulid('1')),
            version: 1,
        }),
        first_issue_date: "2025-08-08".to_owned(),
        current_issue_date: "2025-08-08".to_owned(),
        maturity_date: "2034-08-08".to_owned(),
        face_value: Some(dec("100", u.price_per_100.as_ref().unwrap())),
        cumulative_issued_amount: Some(dec("100", u.currency_amount.as_ref().unwrap())),
        tax_attributes: Some(market_pb::BondTaxAttributes {
            value_added_tax_status: market_pb::ValueAddedTaxStatus::Taxable as i32,
            income_tax_status: market_pb::IncomeTaxStatus::Exempt as i32,
        }),
        coupon_rate: Some(dec("0.025", u.rate.as_ref().unwrap())),
        coupon_frequency: market_pb::BondCouponFrequency::Semiannual as i32,
        day_count: market_pb::BondDayCountConvention::ActActBondIsma as i32,
        business_day: market_pb::BondBusinessDayConvention::Following as i32,
    };

    let make_req = |mode: i32, ctx: Option<pb::BondTaxContext>| pb::AnalyzeBondRequest {
        units: Some(u.clone()),
        bond: Some(taxable_bond.clone()),
        settlement_date: "2026-07-21".to_owned(),
        calendar_requirement: pb::CalendarRequirement::ReferenceReplay as i32,
        calendar: None,
        tax_context: ctx,
        execution_mode: mode,
        input: Some(pb::analyze_bond_request::Input::CleanPrice(dec(
            "100",
            u.price_per_100.as_ref().unwrap(),
        ))),
    };

    // Cell 1: PRE_TAX + None -> succeeds pre-tax
    let c1 = call_bond_ok(
        &harness.service,
        make_req(pb::BondTaxBasis::PreTax as i32, None),
    )
    .await;
    assert_eq!(c1.execution_mode, pb::BondTaxBasis::PreTax as i32);
    assert_eq!(c1.measures_tax_basis, pb::BondTaxBasis::PreTax as i32);
    assert!(c1.after_tax.is_none());

    // Cell 2: PRE_TAX + Some -> ValidationFailed
    assert_eq!(
        call_bond_err(
            &harness.service,
            make_req(
                pb::BondTaxBasis::PreTax as i32,
                Some(tax_context(tax_pack()))
            )
        )
        .await,
        core_pb::ErrorCode::ValidationFailed
    );

    // Cell 3: TAX_ADJUSTED + valid context -> pre-tax top-level + TAX_ADJUSTED sub-result with r5e non-zero tax diff
    let c3 = call_bond_ok(
        &harness.service,
        make_req(
            pb::BondTaxBasis::TaxAdjusted as i32,
            Some(tax_context(tax_pack())),
        ),
    )
    .await;
    assert_eq!(c3.execution_mode, pb::BondTaxBasis::TaxAdjusted as i32);
    assert_eq!(c3.measures_tax_basis, pb::BondTaxBasis::PreTax as i32);
    let after_tax = c3.after_tax.as_ref().expect("after_tax present");
    assert_eq!(after_tax.tax_basis, pb::BondTaxBasis::TaxAdjusted as i32);
    assert_eq!(
        after_tax.claim_scope,
        market_pb::CouponTaxClaimScope::CouponOutputVatBeforeInputCredit as i32
    );
    let pre_ytm = dec_to_decimal(
        c3.measures
            .as_ref()
            .unwrap()
            .yield_to_maturity
            .as_ref()
            .unwrap(),
    );
    let post_ytm = dec_to_decimal(after_tax.yield_to_maturity.as_ref().unwrap());
    assert!(
        pre_ytm > post_ytm && post_ytm > Decimal::ZERO,
        "expected non-zero r5e tax difference: pre={pre_ytm}, post={post_ytm}"
    );
    assert_eq!(c3.measures, c1.measures);
    assert_eq!(c3.cashflows, c1.cashflows);

    // Cell 4a: TAX_ADJUSTED + None -> ValidationFailed
    assert_eq!(
        call_bond_err(
            &harness.service,
            make_req(pb::BondTaxBasis::TaxAdjusted as i32, None)
        )
        .await,
        core_pb::ErrorCode::ValidationFailed
    );

    // Cell 4b: TAX_ADJUSTED + mismatched content_hash -> HashMismatch
    let mut bad_hash_tax_pack = tax_pack();
    bad_hash_tax_pack.content_hash = Some(core_pb::Sha256 {
        value: vec![0xEE; 32],
    });
    assert_eq!(
        call_bond_err(
            &harness.service,
            make_req(
                pb::BondTaxBasis::TaxAdjusted as i32,
                Some(tax_context(bad_hash_tax_pack))
            )
        )
        .await,
        core_pb::ErrorCode::HashMismatch
    );

    // Cell 5: UNSPECIFIED + None -> inferred PRE_TAX
    let c5 = call_bond_ok(
        &harness.service,
        make_req(pb::BondTaxBasis::Unspecified as i32, None),
    )
    .await;
    assert_eq!(c5.execution_mode, pb::BondTaxBasis::PreTax as i32);
    assert_eq!(c5.measures_tax_basis, pb::BondTaxBasis::PreTax as i32);
    assert!(c5.after_tax.is_none());
    assert_eq!(c5, c1);

    // Cell 6: UNSPECIFIED + valid context -> inferred TAX_ADJUSTED
    let c6 = call_bond_ok(
        &harness.service,
        make_req(
            pb::BondTaxBasis::Unspecified as i32,
            Some(tax_context(tax_pack())),
        ),
    )
    .await;
    assert_eq!(c6.execution_mode, pb::BondTaxBasis::TaxAdjusted as i32);
    assert_eq!(c6.measures_tax_basis, pb::BondTaxBasis::PreTax as i32);
    assert_eq!(c6, c3);

    // Cell 7: Unknown non-zero execution_mode -> ValidationFailed (with or without tax_context)
    assert_eq!(
        call_bond_err(&harness.service, make_req(99, None)).await,
        core_pb::ErrorCode::ValidationFailed
    );
    assert_eq!(
        call_bond_err(
            &harness.service,
            make_req(99, Some(tax_context(tax_pack())))
        )
        .await,
        core_pb::ErrorCode::ValidationFailed
    );
}

#[tokio::test]
async fn adbc_7y_reissuance_preserves_issuance_facts_with_and_without_tax_attrs_and_pretax_matches()
{
    let harness = TestHarness::new();
    let u = units();

    let make_adbc_bond = |tax_attrs: Option<market_pb::BondTaxAttributes>| market_pb::Bond {
        instrument: Some(core_pb::VersionRef {
            id: Some(ulid('7')),
            version: 1,
        }),
        first_issue_date: "2024-08-18".to_owned(),
        current_issue_date: "2025-02-18".to_owned(),
        maturity_date: "2031-08-18".to_owned(),
        face_value: Some(dec("100", u.price_per_100.as_ref().unwrap())),
        cumulative_issued_amount: Some(dec("25000000000", u.currency_amount.as_ref().unwrap())),
        tax_attributes: tax_attrs,
        coupon_rate: Some(dec("0.0235", u.rate.as_ref().unwrap())),
        coupon_frequency: market_pb::BondCouponFrequency::Semiannual as i32,
        day_count: market_pb::BondDayCountConvention::ActActBondIsma as i32,
        business_day: market_pb::BondBusinessDayConvention::Following as i32,
    };

    let make_req = |bond: market_pb::Bond| pb::AnalyzeBondRequest {
        units: Some(u.clone()),
        bond: Some(bond),
        settlement_date: "2026-06-15".to_owned(),
        calendar_requirement: pb::CalendarRequirement::ReferenceReplay as i32,
        calendar: None,
        tax_context: None,
        execution_mode: pb::BondTaxBasis::PreTax as i32,
        input: Some(pb::analyze_bond_request::Input::YieldToMaturity(dec(
            "0.0215",
            u.rate.as_ref().unwrap(),
        ))),
    };

    let exempt_attrs = Some(market_pb::BondTaxAttributes {
        value_added_tax_status: market_pb::ValueAddedTaxStatus::Exempt as i32,
        income_tax_status: market_pb::IncomeTaxStatus::Exempt as i32,
    });
    let taxable_attrs = Some(market_pb::BondTaxAttributes {
        value_added_tax_status: market_pb::ValueAddedTaxStatus::Taxable as i32,
        income_tax_status: market_pb::IncomeTaxStatus::Exempt as i32,
    });

    let res_exempt = call_bond_ok(&harness.service, make_req(make_adbc_bond(exempt_attrs))).await;
    let res_taxable = call_bond_ok(&harness.service, make_req(make_adbc_bond(taxable_attrs))).await;
    let res_absent = call_bond_ok(&harness.service, make_req(make_adbc_bond(None))).await;

    // Bitwise equality across all three tax attribute configurations under explicit PRE_TAX.
    assert_eq!(res_exempt.execution_mode, pb::BondTaxBasis::PreTax as i32);
    assert_eq!(
        res_exempt.measures_tax_basis,
        pb::BondTaxBasis::PreTax as i32
    );
    assert!(res_exempt.after_tax.is_none());
    assert_eq!(res_exempt, res_taxable);
    assert_eq!(res_exempt, res_absent);

    // Inspect actual recorded engine inputs for all three calls.
    let recorded = harness.bond_recorder.take();
    assert_eq!(recorded.len(), 3);
    let expected_first = NaiveDate::from_ymd_opt(2024, 8, 18).unwrap();
    let expected_current = NaiveDate::from_ymd_opt(2025, 2, 18).unwrap();
    let expected_cum = FixedDecimal::from_scaled(25_000_000_000_i128 * 1_000_000_000_000);
    let expected_face = FixedDecimal::from_scaled(100_i128 * 1_000_000_000_000);

    for inp in &recorded {
        assert_eq!(inp.terms().first_issue_date(), expected_first);
        assert_eq!(inp.terms().current_issue_date(), expected_current);
        assert_eq!(inp.terms().cumulative_issued_amount(), expected_cum);
        assert_eq!(inp.terms().face_amount(), expected_face);
        assert_ne!(
            inp.terms().cumulative_issued_amount(),
            inp.terms().face_amount()
        );
    }
    assert_eq!(
        recorded[0].terms().tax_attributes(),
        Some(BondTaxAttributes::new(
            ValueAddedTaxStatus::Exempt,
            IncomeTaxStatus::Exempt
        ))
    );
    assert_eq!(
        recorded[1].terms().tax_attributes(),
        Some(BondTaxAttributes::new(
            ValueAddedTaxStatus::Taxable,
            IncomeTaxStatus::Exempt
        ))
    );
    assert_eq!(recorded[2].terms().tax_attributes(), None);

    // Explicit invalid issuance facts without tax_attributes must also fail closed.
    let mut bad_reissue = make_adbc_bond(None);
    bad_reissue.current_issue_date = "2024-08-17".to_owned();
    assert_eq!(
        call_bond_err(&harness.service, make_req(bad_reissue)).await,
        core_pb::ErrorCode::ValidationFailed
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn adbc_1y_exact_market_calendar_succeeds_on_full_2024_2026_and_rejects_missing_empty_or_truncated()
 {
    let harness = TestHarness::new();
    let u = units();

    let short_bond = market_pb::Bond {
        instrument: Some(core_pb::VersionRef {
            id: Some(ulid('2')),
            version: 1,
        }),
        first_issue_date: "2025-11-15".to_owned(),
        current_issue_date: "2025-11-15".to_owned(),
        maturity_date: "2026-11-15".to_owned(),
        face_value: Some(dec("100", u.price_per_100.as_ref().unwrap())),
        cumulative_issued_amount: Some(dec("10000000000", u.currency_amount.as_ref().unwrap())),
        tax_attributes: None,
        coupon_rate: Some(dec("0.018", u.rate.as_ref().unwrap())),
        coupon_frequency: market_pb::BondCouponFrequency::Annual as i32,
        day_count: market_pb::BondDayCountConvention::ActActBondIsma as i32,
        business_day: market_pb::BondBusinessDayConvention::Following as i32,
    };

    let make_req = |cal: Option<market_pb::Calendar>| pb::AnalyzeBondRequest {
        units: Some(u.clone()),
        bond: Some(short_bond.clone()),
        settlement_date: "2026-06-15".to_owned(),
        calendar_requirement: pb::CalendarRequirement::ExactMarket as i32,
        calendar: cal,
        tax_context: None,
        execution_mode: pb::BondTaxBasis::PreTax as i32,
        input: Some(pb::analyze_bond_request::Input::YieldToMaturity(dec(
            "0.0175",
            u.rate.as_ref().unwrap(),
        ))),
    };

    // Full 2024-2026 CFETS calendar -> succeeds, nominal 2026-11-15 (Sun) -> payment 2026-11-16 (Mon).
    let full_cal = cfets_2024_2026_calendar(NaiveDate::from_ymd_opt(2026, 12, 31).unwrap());
    assert_eq!(full_cal.sessions.first().unwrap().local_date, "2024-01-01");
    assert!(full_cal.sessions.first().unwrap().closed);
    assert_eq!(full_cal.sessions.last().unwrap().local_date, "2026-12-31");
    assert!(!full_cal.sessions.last().unwrap().closed);

    let ok_res = call_bond_ok(&harness.service, make_req(Some(full_cal))).await;
    assert_eq!(ok_res.cashflows.len(), 1);
    assert_eq!(ok_res.cashflows[0].nominal_date, "2026-11-15");
    assert_eq!(ok_res.cashflows[0].payment_date, "2026-11-16");
    let ok_input = harness.bond_recorder.take().pop().unwrap();
    assert_eq!(
        ok_input.calendar().coverage_start(),
        NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()
    );
    assert_eq!(
        ok_input.calendar().coverage_end(),
        NaiveDate::from_ymd_opt(2026, 12, 31).unwrap()
    );

    // 1. Missing Calendar -> ValidationFailed
    assert_eq!(
        call_bond_err(&harness.service, make_req(None)).await,
        core_pb::ErrorCode::ValidationFailed
    );
    assert!(harness.bond_recorder.take().is_empty());

    // 2. Empty sessions -> ValidationFailed
    let empty_cal = market_pb::Calendar {
        calendar_id: Some(ulid('C')),
        version: 1,
        owner: None,
        market: "CFETS".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective_from: Some(shanghai_market_time("2024-01-01", 0, 0, 0)),
        effective_to: Some(shanghai_market_time("2027-01-01", 0, 0, 0)),
        sessions: Vec::new(),
    };
    assert_eq!(
        call_bond_err(&harness.service, make_req(Some(empty_cal.clone()))).await,
        core_pb::ErrorCode::ValidationFailed
    );
    assert!(harness.bond_recorder.take().is_empty());

    // 3. Truncated to last session 2026-10-10 -> ValidationFailed (coverage_end must NOT expand to 2026-11-15)
    let truncated_cal = cfets_2024_2026_calendar(NaiveDate::from_ymd_opt(2026, 10, 10).unwrap());
    assert_eq!(
        truncated_cal.sessions.last().unwrap().local_date,
        "2026-10-10"
    );
    assert!(!truncated_cal.sessions.last().unwrap().closed);
    assert_eq!(
        call_bond_err(&harness.service, make_req(Some(truncated_cal))).await,
        core_pb::ErrorCode::ValidationFailed
    );
    let truncated_input = harness.bond_recorder.take().pop().unwrap();
    assert_eq!(
        truncated_input.calendar().coverage_end(),
        NaiveDate::from_ymd_opt(2026, 10, 10).unwrap()
    );

    // 4. CarryRoll also enforces ExactMarket requirement on parse_calendar (missing & empty sessions).
    let make_carry_req = |cal: Option<market_pb::Calendar>| pb::AnalyzeCarryRollRequest {
        units: Some(u.clone()),
        bond: Some(short_bond.clone()),
        initial_settlement: "2026-06-15".to_owned(),
        horizon_settlement: "2026-09-15".to_owned(),
        calendar_requirement: pb::CalendarRequirement::ExactMarket as i32,
        calendar: cal,
        curve_as_of_date: "2026-06-15".to_owned(),
        curve_nodes: vec![
            pb::YieldCurveNodeInput {
                maturity_date: "2026-06-30".to_owned(),
                yield_to_maturity: Some(dec("0.017", u.rate.as_ref().unwrap())),
            },
            pb::YieldCurveNodeInput {
                maturity_date: "2027-06-30".to_owned(),
                yield_to_maturity: Some(dec("0.019", u.rate.as_ref().unwrap())),
            },
        ],
    };

    for cal in [None, Some(empty_cal)] {
        let resp = harness
            .service
            .analyze_carry_roll(Request::new(make_carry_req(cal)))
            .await
            .expect("gRPC transport ok")
            .into_inner();
        match resp.result {
            Some(pb::analyze_carry_roll_response::Result::Error(e)) => {
                assert_eq!(
                    core_pb::ErrorCode::try_from(e.code).unwrap(),
                    core_pb::ErrorCode::ValidationFailed
                );
            }
            other => panic!("expected ValidationFailed for carry-roll ExactMarket, got {other:?}"),
        }
    }
}
