use std::fs;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::FuturesDeliveryEngine;
use ficant_application::use_cases::futures_delivery::CalculateFuturesDeliveryBasket;
use ficant_domain::analytics::{
    AnalyticsObjectRef, BondTerms, BusinessDayConvention, CouponFrequency, DayCountConvention,
    FixedDecimal,
};
use ficant_domain::futures_delivery::{CgbFuturesProduct, FuturesDeliverableInput};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeFuturesDeliveryEngine;
use rust_decimal::Decimal;
use serde_json::Value;

#[test]
fn ten_year_candidate_crosses_safe_rust_adapter_and_preserves_identities() {
    let input = input();
    let result = NativeFuturesDeliveryEngine.calculate(&input).unwrap();
    result.validate_against(&input).unwrap();
    let measures = result.measures();
    assert!(measures.conversion_factor().is_positive());
    assert_eq!(
        measures.net_basis(),
        measures
            .gross_basis()
            .checked_sub(measures.holding_carry())
            .unwrap()
    );
    assert_eq!(
        measures.delivery_profit(),
        FixedDecimal::ZERO
            .checked_sub(measures.net_basis())
            .unwrap()
    );
}

#[test]
fn basket_selects_maximum_irr_then_stable_bond_id() {
    let mut expensive = input();
    expensive = with_bond_and_price(&expensive, 'G', "102.00");
    let cheap = with_bond_and_price(&input(), 'H', "100.00");
    let basket = CalculateFuturesDeliveryBasket::new(&NativeFuturesDeliveryEngine)
        .execute(&[expensive, cheap])
        .unwrap();
    assert_eq!(basket.ctd().input().bond().version_ref().id(), &id('H'));

    let higher_id = with_bond_and_price(&input(), 'N', "100.00");
    let lower_id = with_bond_and_price(&input(), 'M', "100.00");
    let tied = CalculateFuturesDeliveryBasket::new(&NativeFuturesDeliveryEngine)
        .execute(&[higher_id, lower_id])
        .unwrap();
    assert_eq!(tied.ctd().input().bond().version_ref().id(), &id('M'));
}

#[test]
fn production_matches_all_four_frozen_decimal_oracle_cases() {
    let root = repository_root();
    let inputs = read_json(
        &root.join("tests/golden-cases/china-rates/phase2c-futures-delivery-inputs.json"),
    );
    let expected =
        read_json(&root.join(
            "tests/golden-cases/china-rates/expected/phase2c-futures-delivery-v1-expected.json",
        ));
    let cases = inputs["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for (index, case) in cases.iter().enumerate() {
        let case_id = string(case, "id");
        let suffix = char::from(b'P' + u8::try_from(index).unwrap());
        let input = frozen_input(&inputs, case, suffix);
        let result = NativeFuturesDeliveryEngine
            .calculate(&input)
            .unwrap_or_else(|error| panic!("{case_id} failed: {error:?}"));
        let measures = result.measures();
        let reference = &expected["case_results"][case_id];
        assert_eq!(
            measures.months_to_next_coupon(),
            u32::try_from(reference["months_to_next_coupon"].as_u64().unwrap()).unwrap()
        );
        assert_eq!(
            measures.remaining_coupon_count(),
            u32::try_from(reference["remaining_coupon_count"].as_u64().unwrap()).unwrap()
        );
        for (field, actual, tolerance) in [
            ("conversion_factor", measures.conversion_factor(), 2),
            (
                "purchase_accrued_interest",
                measures.purchase_accrued_interest(),
                2,
            ),
            (
                "delivery_accrued_interest",
                measures.delivery_accrued_interest(),
                2,
            ),
            ("interim_coupons", measures.interim_coupons(), 2),
            ("invoice_price", measures.invoice_price(), 10),
            ("purchase_dirty_price", measures.purchase_dirty_price(), 10),
            ("gross_basis", measures.gross_basis(), 10),
            ("financing_cost", measures.financing_cost(), 10),
            ("holding_carry", measures.holding_carry(), 10),
            ("net_basis", measures.net_basis(), 10),
            ("implied_repo_rate", measures.implied_repo_rate(), 10),
            ("delivery_profit", measures.delivery_profit(), 10),
        ] {
            let expected_value = fixed(string(reference, field));
            assert!(
                actual.scaled().abs_diff(expected_value.scaled()) <= tolerance,
                "{case_id} {field}: actual {}, expected {}",
                actual.scaled(),
                expected_value.scaled()
            );
        }
    }

    let suffixes = ['G', 'H', 'J'];
    let basket_inputs = inputs["t_basket"]
        .as_array()
        .unwrap()
        .iter()
        .zip(suffixes)
        .map(|(case, suffix)| frozen_input(&inputs, case, suffix))
        .collect::<Vec<_>>();
    let basket = CalculateFuturesDeliveryBasket::new(&NativeFuturesDeliveryEngine)
        .execute(&basket_inputs)
        .unwrap();
    assert_eq!(string(&expected, "ctd_bond_id"), "T-bond-ctd");
    assert_eq!(basket.ctd().input().bond().version_ref().id(), &id('H'));
}

fn frozen_input(common: &Value, case: &Value, bond_suffix: char) -> FuturesDeliverableInput {
    let product = match case["product"].as_str().unwrap_or("T") {
        "TS" => CgbFuturesProduct::TwoYear,
        "TF" => CgbFuturesProduct::FiveYear,
        "T" => CgbFuturesProduct::TenYear,
        "TL" => CgbFuturesProduct::ThirtyYear,
        other => panic!("unsupported product {other}"),
    };
    let frequency = match case["frequency"].as_u64().unwrap() {
        1 => CouponFrequency::Annual,
        2 => CouponFrequency::Semiannual,
        other => panic!("unsupported frequency {other}"),
    };
    FuturesDeliverableInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object(bond_suffix),
        object('D'),
        object('E'),
        MarketTime::new(
            Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).single().unwrap(),
            "Asia/Shanghai",
            date_value(common, "valuation_date"),
        )
        .unwrap(),
        date_value(common, "purchase_date"),
        date_value(common, "delivery_month_first"),
        date_value(common, "delivery_date"),
        product,
        BondTerms::new(
            date_value(case, "issue_date"),
            date_value(case, "maturity_date"),
            frequency,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            fixed(string(case, "coupon_rate")),
            fixed("100"),
        )
        .unwrap(),
        fixed(string(case, "spot_clean_price")),
        fixed(string(common, "futures_clean_price")),
        fixed(string(common, "financing_rate")),
    )
    .unwrap()
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn string<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap()
}

fn date_value(value: &Value, field: &str) -> NaiveDate {
    NaiveDate::parse_from_str(string(value, field), "%Y-%m-%d").unwrap()
}

fn with_bond_and_price(
    source: &FuturesDeliverableInput,
    bond_suffix: char,
    spot_clean_price: &str,
) -> FuturesDeliverableInput {
    FuturesDeliverableInput::new(
        source.owner().clone(),
        source.futures_contract().clone(),
        object(bond_suffix),
        source.rule_pack().clone(),
        source.snapshot().clone(),
        source.valuation_at().clone(),
        source.purchase_date(),
        source.delivery_month_first(),
        source.delivery_date(),
        source.product(),
        source.terms().clone(),
        fixed(spot_clean_price),
        source.futures_clean_price(),
        source.financing_rate(),
    )
    .unwrap()
}

fn input() -> FuturesDeliverableInput {
    FuturesDeliverableInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        object('F'),
        MarketTime::new(
            Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).single().unwrap(),
            "Asia/Shanghai",
            date(2026, 7, 20),
        )
        .unwrap(),
        date(2026, 7, 21),
        date(2026, 9, 1),
        date(2026, 9, 18),
        CgbFuturesProduct::TenYear,
        BondTerms::new(
            date(2024, 8, 15),
            date(2034, 8, 15),
            CouponFrequency::Semiannual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            fixed("0.025"),
            fixed("100"),
        )
        .unwrap(),
        fixed("101.25"),
        fixed("99.50"),
        fixed("0.018"),
    )
    .unwrap()
}

fn fixed(raw: &str) -> FixedDecimal {
    let mut value = raw.parse::<Decimal>().unwrap() * Decimal::from(1_000_000_000_000_i64);
    value.rescale(0);
    let scaled = value.mantissa();
    FixedDecimal::from_scaled(scaled)
}
fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}
fn object(suffix: char) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(id(suffix), Version::new(1).unwrap()),
        ContentHash::digest(suffix.to_string().as_bytes()),
    )
}
fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
