use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::CalculateFuturesHedge;
use ficant_application::ports::{FuturesDeliveryRuleParser, FuturesHedgeEngine};
use ficant_cgb_futures_pack::{CgbFuturesDeliveryRulePackParser, TYPE_URL};
use ficant_domain::analytics::{AnalyticsObjectRef, FixedDecimal};
use ficant_domain::futures_delivery::CgbFuturesProduct;
use ficant_domain::futures_hedge::FuturesHedgeInput;
use ficant_domain::market::RulePackContent;
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeFuturesHedgeEngine;
use serde_json::Value;

#[test]
fn safe_adapter_preserves_direction_exact_hedge_and_zero_hand_tie() {
    let root = repository_root();
    let inputs =
        read_json(&root.join("tests/golden-cases/china-rates/phase2d-futures-hedge-inputs.json"));
    let contract_notional = fixed(string(&inputs, "contract_notional"));
    let face_quote_basis = fixed(string(&inputs, "quote_face"));

    let exact = input(contract_notional, face_quote_basis, "500", "0.045", "0.9");
    let result = CalculateFuturesHedge::new(&NativeFuturesHedgeEngine)
        .execute(&exact)
        .unwrap();
    assert_eq!(result.measures().futures_contract_dv01(), fixed("500"));
    assert_eq!(result.measures().raw_contracts(), fixed("-1"));
    assert_eq!(result.measures().recommended_contracts(), -1);
    assert_eq!(result.measures().residual_dv01(), FixedDecimal::ZERO);
    assert_eq!(result.measures().hedge_effectiveness(), fixed("1"));

    let tie = input(contract_notional, face_quote_basis, "-250", "0.045", "0.9");
    let result = NativeFuturesHedgeEngine.calculate(&tie).unwrap();
    assert_eq!(result.measures().raw_contracts(), fixed("0.5"));
    assert_eq!(result.measures().recommended_contracts(), 0);
    assert_eq!(result.measures().residual_dv01(), fixed("-250"));
    assert_eq!(result.measures().hedge_effectiveness(), FixedDecimal::ZERO);

    let doubled_basis = FuturesHedgeInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        object('F'),
        object('G'),
        object('H'),
        object('J'),
        None,
        CgbFuturesProduct::TenYear,
        contract_notional,
        face_quote_basis.checked_mul_integer(2).unwrap(),
        fixed("500"),
        fixed("0.09"),
        fixed("0.9"),
    )
    .unwrap();
    let doubled_result = NativeFuturesHedgeEngine.calculate(&doubled_basis).unwrap();
    assert_eq!(
        doubled_result.measures().futures_contract_dv01(),
        fixed("500")
    );
    assert_eq!(doubled_result.measures().recommended_contracts(), -1);
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
    let contract_notional = fixed(string(&inputs, "contract_notional"));
    let face_quote_basis = fixed(string(&inputs, "quote_face"));
    let cases = inputs["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let case_id = string(case, "id");
        let input = frozen_input(case, contract_notional, face_quote_basis);
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

#[test]
fn production_matches_all_six_bp1_v2_rule_pack_decimal_oracle_cases() {
    const EXPECTED_V2_BIN_SHA256: &str =
        "8d8a0b944557cf87ed3d1f598ab3e02fa7f0aec125c1c26c17280acbd24367c5";

    let root = repository_root();
    let inputs =
        read_json(&root.join("tests/golden-cases/china-rates/bp1-futures-hedge-v2-inputs.json"));
    let expected = read_json(
        &root.join("tests/golden-cases/china-rates/expected/bp1-futures-hedge-v2-expected.json"),
    );

    let bin_path = root.join("domain-packs/cgb-futures/cgb-futures-v2.bin");
    let raw_bin = fs::read(&bin_path).expect("authoritative cgb-futures-v2.bin exists");
    let bin_hash = ContentHash::digest(&raw_bin);
    let actual_hex = hex_digest(&bin_hash);
    assert_eq!(actual_hex, EXPECTED_V2_BIN_SHA256);
    assert_eq!(string(&inputs["rule_pack"], "bin_sha256"), actual_hex);
    assert_eq!(string(&expected["rule_pack"], "bin_sha256"), actual_hex);

    let pack_content = RulePackContent::new(TYPE_URL, raw_bin).unwrap();
    let rule_pack_ref =
        AnalyticsObjectRef::new(VersionRef::new(id('J'), Version::new(2).unwrap()), bin_hash);
    let parser = CgbFuturesDeliveryRulePackParser;

    let cases = inputs["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 6);

    let mut product_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for case in cases {
        let case_id = string(case, "id");
        let product_code = string(case, "product");
        *product_counts.entry(product_code).or_default() += 1;

        let product = parser
            .parse_product_code(product_code)
            .unwrap_or_else(|error| panic!("{case_id} product parse failed: {error:?}"));
        let rule = parser
            .parse_for_per_100_hedge(&pack_content, product)
            .unwrap_or_else(|error| panic!("{case_id} v2 rule pack parse failed: {error:?}"));
        let contract_notional = rule
            .contract_notional()
            .unwrap()
            .expect("v2 pack defines contract notional");
        let face_quote_basis = rule.face_quote_basis();

        let reference = &expected["case_results"][case_id];
        assert_eq!(
            contract_notional,
            fixed(string(reference, "contract_notional")),
            "{case_id} contract_notional"
        );
        assert_eq!(
            face_quote_basis,
            fixed(string(reference, "face_quote_basis")),
            "{case_id} face_quote_basis"
        );

        let hedge_input = bp1_v2_hedge_input(
            case,
            product,
            rule_pack_ref.clone(),
            contract_notional,
            face_quote_basis,
        );
        let result = NativeFuturesHedgeEngine
            .calculate(&hedge_input)
            .unwrap_or_else(|error| panic!("{case_id} failed: {error:?}"));
        assert_hedge_measures_match(case_id, &result, reference, 10);
    }

    assert_eq!(product_counts.get("TS").copied(), Some(3));
    assert_eq!(product_counts.get("TF").copied(), Some(1));
    assert_eq!(product_counts.get("T").copied(), Some(1));
    assert_eq!(product_counts.get("TL").copied(), Some(1));
}

fn bp1_v2_hedge_input(
    case: &Value,
    product: CgbFuturesProduct,
    rule_pack_ref: AnalyticsObjectRef,
    contract_notional: FixedDecimal,
    face_quote_basis: FixedDecimal,
) -> FuturesHedgeInput {
    FuturesHedgeInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        object('F'),
        object('G'),
        rule_pack_ref,
        object('H'),
        Some(
            MarketTime::new(
                Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).single().unwrap(),
                "Asia/Shanghai",
                NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            )
            .unwrap(),
        ),
        product,
        contract_notional,
        face_quote_basis,
        fixed(string(case, "target_dv01")),
        fixed(string(case, "ctd_dv01_per_100")),
        fixed(string(case, "conversion_factor")),
    )
    .unwrap()
}

fn assert_hedge_measures_match(
    case_id: &str,
    result: &ficant_domain::futures_hedge::FuturesHedgeResult,
    reference: &Value,
    tolerance: u128,
) {
    let measures = result.measures();
    for (field, actual) in [
        ("futures_contract_dv01", measures.futures_contract_dv01()),
        ("raw_contracts", measures.raw_contracts()),
        ("residual_dv01", measures.residual_dv01()),
        ("hedge_effectiveness", measures.hedge_effectiveness()),
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

fn hex_digest(hash: &ContentHash) -> String {
    let mut out = String::with_capacity(hash.as_bytes().len() * 2);
    for byte in hash.as_bytes() {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn frozen_input(
    case: &Value,
    contract_notional: FixedDecimal,
    face_quote_basis: FixedDecimal,
) -> FuturesHedgeInput {
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
        Some(
            MarketTime::new(
                Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).single().unwrap(),
                "Asia/Shanghai",
                NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            )
            .unwrap(),
        ),
        product,
        contract_notional,
        face_quote_basis,
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

fn input(
    contract_notional: FixedDecimal,
    face_quote_basis: FixedDecimal,
    target: &str,
    ctd: &str,
    factor: &str,
) -> FuturesHedgeInput {
    FuturesHedgeInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        object('F'),
        object('G'),
        object('H'),
        object('J'),
        Some(
            MarketTime::new(
                Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).single().unwrap(),
                "Asia/Shanghai",
                NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            )
            .unwrap(),
        ),
        CgbFuturesProduct::TenYear,
        contract_notional,
        face_quote_basis,
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
