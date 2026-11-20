use chrono::{NaiveDate, TimeZone, Utc};
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{AnalyticsObjectRef, FixedDecimal};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliveryRule, FuturesDeliveryRuleInput,
};
use ficant_domain::futures_hedge::{FuturesHedgeInput, FuturesHedgeMeasures, FuturesHedgeResult};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};

#[test]
fn delivery_rule_contract_notional_computes_exact_product_and_guards_overflow() {
    let base = FuturesDeliveryRule::new(FuturesDeliveryRuleInput {
        original_term_max_months: 120,
        residual_min_months: 78,
        residual_max_months: Some(123),
        delivery_months: vec![3, 6, 9, 12],
        nominal_coupon: fixed("0.03"),
        face_quote_basis: fixed("100"),
        accrued_interest_day_count: 1,
        conversion_factor_rounding_places: 4,
        accrued_interest_rounding_places: 7,
        annual_day_basis: 365,
    })
    .unwrap();
    assert_eq!(base.contract_notional().unwrap(), None);

    let with_20k = base
        .clone()
        .with_contract_size_in_quote_units(20_000)
        .unwrap();
    assert_eq!(
        with_20k.contract_notional().unwrap(),
        Some(fixed("2000000"))
    );

    let with_10k = base.with_contract_size_in_quote_units(10_000).unwrap();
    assert_eq!(
        with_10k.contract_notional().unwrap(),
        Some(fixed("1000000"))
    );

    let overflowing = FuturesDeliveryRule::new(FuturesDeliveryRuleInput {
        original_term_max_months: 120,
        residual_min_months: 78,
        residual_max_months: None,
        delivery_months: vec![3, 6, 9, 12],
        nominal_coupon: fixed("0.03"),
        face_quote_basis: FixedDecimal::from_scaled(i128::MAX),
        accrued_interest_day_count: 1,
        conversion_factor_rounding_places: 4,
        accrued_interest_rounding_places: 7,
        annual_day_basis: 365,
    })
    .unwrap()
    .with_contract_size_in_quote_units(2)
    .unwrap();
    assert_eq!(
        overflowing.contract_notional(),
        Err(DomainErrorCode::InvalidValue)
    );
}

#[test]
fn input_binds_lineage_and_optional_valuation_time_into_fingerprint() {
    let input = valid_input();
    assert_eq!(input.contract_notional(), fixed("1000000"));
    assert_eq!(input.face_quote_basis(), fixed("100"));
    assert_eq!(input.ctd_dv01_per_quote_unit(), fixed("0.045"));
    assert!(input.valuation_at().is_some());
    assert_eq!(input.fingerprint(), valid_input().fingerprint());

    let no_time = rebuild_with(&input, None, input.rule_pack().clone(), object('C'));
    assert!(no_time.valuation_at().is_none());
    assert_ne!(input.fingerprint(), no_time.fingerprint());

    let later_time = rebuild_with(
        &input,
        Some(
            MarketTime::new(
                Utc.with_ymd_and_hms(2026, 7, 21, 4, 0, 0).single().unwrap(),
                "Asia/Shanghai",
                NaiveDate::from_ymd_opt(2026, 7, 21).unwrap(),
            )
            .unwrap(),
        ),
        input.rule_pack().clone(),
        object('C'),
    );
    assert_ne!(input.fingerprint(), later_time.fingerprint());

    let pack_version_changed = rebuild_with(
        &input,
        input.valuation_at().cloned(),
        AnalyticsObjectRef::new(
            VersionRef::new(
                input.rule_pack().version_ref().id().clone(),
                Version::new(2).unwrap(),
            ),
            input.rule_pack().content_hash().clone(),
        ),
        object('C'),
    );
    assert_ne!(input.fingerprint(), pack_version_changed.fingerprint());

    let pack_hash_changed = rebuild_with(
        &input,
        input.valuation_at().cloned(),
        AnalyticsObjectRef::new(
            input.rule_pack().version_ref().clone(),
            ContentHash::digest(b"different-pack-payload"),
        ),
        object('C'),
    );
    assert_ne!(input.fingerprint(), pack_hash_changed.fingerprint());

    let drifted_target_artifact = rebuild_with(
        &input,
        input.valuation_at().cloned(),
        input.rule_pack().clone(),
        object('Z'),
    );
    assert_ne!(input.fingerprint(), drifted_target_artifact.fingerprint());

    let measures = FuturesHedgeMeasures::new(
        fixed("466.321243523316"),
        fixed("-1.072222222222"),
        -1,
        fixed("33.678756476684"),
        fixed("0.932642487047"),
    )
    .unwrap();
    let result = FuturesHedgeResult::new(input.clone(), measures);
    assert!(result.validate_against(&input).is_ok());
    assert_eq!(
        result.validate_against(&no_time),
        Err(DomainErrorCode::BrokenLineage)
    );
}

#[test]
fn input_fingerprint_changes_when_scale_or_numeric_values_change() {
    let input = valid_input();
    for mutated in [
        build_full(
            input.valuation_at().cloned(),
            fixed("2000000"),
            fixed("100"),
            fixed("500"),
            fixed("0.045"),
            fixed("0.965"),
        )
        .unwrap(),
        build_full(
            input.valuation_at().cloned(),
            fixed("1000000"),
            fixed("200"),
            fixed("500"),
            fixed("0.045"),
            fixed("0.965"),
        )
        .unwrap(),
        build_full(
            input.valuation_at().cloned(),
            fixed("1000000"),
            fixed("100"),
            fixed("501"),
            fixed("0.045"),
            fixed("0.965"),
        )
        .unwrap(),
        build_full(
            input.valuation_at().cloned(),
            fixed("1000000"),
            fixed("100"),
            fixed("500"),
            fixed("0.046"),
            fixed("0.965"),
        )
        .unwrap(),
        build_full(
            input.valuation_at().cloned(),
            fixed("1000000"),
            fixed("100"),
            fixed("500"),
            fixed("0.045"),
            fixed("0.966"),
        )
        .unwrap(),
    ] {
        assert_ne!(input.fingerprint(), mutated.fingerprint());
    }
}

#[test]
fn input_rejects_invalid_timezone_and_nonpositive_scale_or_risk_terms() {
    assert_eq!(
        build_full(
            Some(
                MarketTime::new(
                    Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).single().unwrap(),
                    "UTC",
                    NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
                )
                .unwrap()
            ),
            fixed("1000000"),
            fixed("100"),
            fixed("500"),
            fixed("0.045"),
            fixed("0.965"),
        ),
        Err(DomainErrorCode::InvalidValue)
    );
    for invalid in [
        build_full(
            None,
            FixedDecimal::ZERO,
            fixed("100"),
            fixed("500"),
            fixed("0.045"),
            fixed("0.965"),
        ),
        build_full(
            None,
            fixed("-1000000"),
            fixed("100"),
            fixed("500"),
            fixed("0.045"),
            fixed("0.965"),
        ),
        build_full(
            None,
            fixed("1000000"),
            FixedDecimal::ZERO,
            fixed("500"),
            fixed("0.045"),
            fixed("0.965"),
        ),
        build_full(
            None,
            fixed("1000000"),
            fixed("-100"),
            fixed("500"),
            fixed("0.045"),
            fixed("0.965"),
        ),
        build(FixedDecimal::ZERO, fixed("0.045"), fixed("0.965")),
        build(fixed("500"), FixedDecimal::ZERO, fixed("0.965")),
        build(fixed("500"), fixed("0.045"), FixedDecimal::ZERO),
    ] {
        assert_eq!(invalid, Err(DomainErrorCode::InvalidValue));
    }
}

fn rebuild_with(
    base: &FuturesHedgeInput,
    valuation_at: Option<MarketTime>,
    rule_pack: AnalyticsObjectRef,
    target_risk_artifact: AnalyticsObjectRef,
) -> FuturesHedgeInput {
    FuturesHedgeInput::new(
        base.owner().clone(),
        target_risk_artifact,
        base.delivery_artifact().clone(),
        base.ctd_analytics_artifact().clone(),
        base.futures_contract().clone(),
        base.ctd_bond().clone(),
        rule_pack,
        base.snapshot().clone(),
        valuation_at,
        base.product(),
        base.contract_notional(),
        base.face_quote_basis(),
        base.target_dv01(),
        base.ctd_dv01_per_quote_unit(),
        base.conversion_factor(),
    )
    .unwrap()
}

#[test]
fn measures_allow_signed_contracts_and_residual_but_bound_effectiveness() {
    assert!(
        FuturesHedgeMeasures::new(
            fixed("466.321243523316"),
            fixed("-1.072222222222"),
            -1,
            fixed("33.678756476684"),
            fixed("0.932642487047"),
        )
        .is_ok()
    );
    assert!(
        FuturesHedgeMeasures::new(
            fixed("466"),
            fixed("-1"),
            -1,
            FixedDecimal::ZERO,
            fixed("1.000000000001"),
        )
        .is_err()
    );
}

fn valid_input() -> FuturesHedgeInput {
    build(fixed("500"), fixed("0.045"), fixed("0.965")).unwrap()
}

fn build(
    target: FixedDecimal,
    ctd: FixedDecimal,
    factor: FixedDecimal,
) -> ficant_domain::DomainResult<FuturesHedgeInput> {
    build_full(
        Some(
            MarketTime::new(
                Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).single().unwrap(),
                "Asia/Shanghai",
                NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            )
            .unwrap(),
        ),
        fixed("1000000"),
        fixed("100"),
        target,
        ctd,
        factor,
    )
}

fn build_full(
    valuation_at: Option<MarketTime>,
    contract_notional: FixedDecimal,
    face_quote_basis: FixedDecimal,
    target: FixedDecimal,
    ctd: FixedDecimal,
    factor: FixedDecimal,
) -> ficant_domain::DomainResult<FuturesHedgeInput> {
    FuturesHedgeInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        object('F'),
        object('G'),
        object('H'),
        object('J'),
        valuation_at,
        CgbFuturesProduct::TenYear,
        contract_notional,
        face_quote_basis,
        target,
        ctd,
        factor,
    )
}

fn fixed(raw: &str) -> FixedDecimal {
    let decimal = raw.parse::<rust_decimal::Decimal>().unwrap();
    let mut value = decimal * rust_decimal::Decimal::from(1_000_000_000_000_i64);
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
