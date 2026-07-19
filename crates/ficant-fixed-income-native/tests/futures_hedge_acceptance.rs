use std::fs;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::CalculateFuturesHedge;
use ficant_application::ports::FuturesHedgeEngine;
use ficant_domain::analytics::{AnalyticsObjectRef, FixedDecimal};
use ficant_domain::futures_delivery::CgbFuturesProduct;
use ficant_domain::futures_hedge::FuturesHedgeInput;
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeFuturesHedgeEngine;
use serde_json::Value;

#[test]
fn safe_adapter_preserves_direction_exact_hedge_and_zero_hand_tie() {
    let exact = input("500", "0.045", "0.9");
    let result = CalculateFuturesHedge::new(&NativeFuturesHedgeEngine)
        .execute(&exact)
        .unwrap();
    assert_eq!(result.measures().futures_contract_dv01(), fixed("500"));
    assert_eq!(result.measures().raw_contracts(), fixed("-1"));
    assert_eq!(result.measures().recommended_contracts(), -1);
    assert_eq!(result.measures().residual_dv01(), FixedDecimal::ZERO);
    assert_eq!(result.measures().hedge_effectiveness(), fixed("1"));

    let tie = input("-250", "0.045", "0.9");
    let result = NativeFuturesHedgeEngine.calculate(&tie).unwrap();
    assert_eq!(result.measures().raw_contracts(), fixed("0.5"));
    assert_eq!(result.measures().recommended_contracts(), 0);
    assert_eq!(result.measures().residual_dv01(), fixed("-250"));
    assert_eq!(result.measures().hedge_effectiveness(), FixedDecimal::ZERO);
}

#[test]
fn production_matches_all_four_frozen_decimal_oracle_cases() {
    let root = repository_root();
    let inputs =
        read_json(&root.join("tests/golden-cases/china-rates/phase2d-futures-hedge-inputs.json"));
    let expected = read_json(
        &root
            .join("tests/golden-cases/china-rates/expected/phase2d-futures-hedge-v1-expected.json"),
    );
    let cases = inputs["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let case_id = string(case, "id");
        let input = frozen_input(case);
        let result = NativeFuturesHedgeEngine
            .calculate(&input)
            .unwrap_or_else(|error| panic!("{case_id} failed: {error:?}"));
        let measures = result.measures();
        let reference = &expected["case_results"][case_id];
        for (field, actual, tolerance) in [
            (
                "futures_contract_dv01",
                measures.futures_contract_dv01(),
                10,
            ),
            ("raw_contracts", measures.raw_contracts(), 10),
            ("residual_dv01", measures.residual_dv01(), 10),
            ("hedge_effectiveness", measures.hedge_effectiveness(), 10),
        ] {
            let expected_value = fixed(string(reference, field));
            assert!(
                actual.scaled().abs_diff(expected_value.scaled()) <= tolerance,
                "{case_id} {field}: actual {}, expected {}",
                actual.scaled(),
                expected_value.scaled()
            );
        }
        assert_eq!(
            measures.recommended_contracts(),
            reference["recommended_contracts"].as_i64().unwrap(),
            "{case_id} recommended_contracts"
        );
    }
}

fn frozen_input(case: &Value) -> FuturesHedgeInput {
    let product = match string(case, "product") {
        "TS" => CgbFuturesProduct::TwoYear,
        "TF" => CgbFuturesProduct::FiveYear,
        "T" => CgbFuturesProduct::TenYear,
        "TL" => CgbFuturesProduct::ThirtyYear,
        other => panic!("unsupported product {other}"),
    };
    FuturesHedgeInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        object('F'),
        object('G'),
        object('H'),
        object('J'),
        MarketTime::new(
            Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).single().unwrap(),
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        )
        .unwrap(),
        product,
        fixed(string(case, "target_dv01")),
        fixed(string(case, "ctd_dv01_per_100")),
        fixed(string(case, "conversion_factor")),
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

fn input(target: &str, ctd: &str, factor: &str) -> FuturesHedgeInput {
    FuturesHedgeInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        object('F'),
        object('G'),
        object('H'),
        object('J'),
        MarketTime::new(
            Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).single().unwrap(),
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        )
        .unwrap(),
        CgbFuturesProduct::TenYear,
        fixed(target),
        fixed(ctd),
        fixed(factor),
    )
    .unwrap()
}
fn fixed(raw: &str) -> FixedDecimal {
    let mut value = raw.parse::<rust_decimal::Decimal>().unwrap()
        * rust_decimal::Decimal::from(1_000_000_000_000_i64);
    value.rescale(0);
    FixedDecimal::from_scaled(value.mantissa())
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
