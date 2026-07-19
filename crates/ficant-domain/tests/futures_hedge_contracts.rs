use chrono::{NaiveDate, TimeZone, Utc};
use ficant_domain::analytics::{AnalyticsObjectRef, FixedDecimal};
use ficant_domain::futures_delivery::CgbFuturesProduct;
use ficant_domain::futures_hedge::{
    CGB_FUTURES_CONTRACT_NOTIONAL, FuturesHedgeInput, FuturesHedgeMeasures,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};

#[test]
fn input_binds_all_risk_and_delivery_lineage_and_rejects_zero_or_nonpositive_risk_terms() {
    let input = valid_input();
    assert_eq!(input.contract_notional(), CGB_FUTURES_CONTRACT_NOTIONAL);
    let drifted = FuturesHedgeInput::new(
        input.owner().clone(),
        object('Z'),
        input.delivery_artifact().clone(),
        input.ctd_analytics_artifact().clone(),
        input.futures_contract().clone(),
        input.ctd_bond().clone(),
        input.rule_pack().clone(),
        input.snapshot().clone(),
        input.valuation_at().clone(),
        input.product(),
        input.target_dv01(),
        input.ctd_dv01_per_100(),
        input.conversion_factor(),
    )
    .unwrap();
    assert_ne!(input.fingerprint(), drifted.fingerprint());

    assert!(build(FixedDecimal::ZERO, fixed("0.045"), fixed("0.965")).is_err());
    assert!(build(fixed("500"), FixedDecimal::ZERO, fixed("0.965")).is_err());
    assert!(build(fixed("500"), fixed("0.045"), FixedDecimal::ZERO).is_err());
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
