use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::CalculateFuturesHedge;
use ficant_application::ports::FuturesHedgeEngine;
use ficant_domain::analytics::{AnalyticsObjectRef, FixedDecimal};
use ficant_domain::futures_delivery::CgbFuturesProduct;
use ficant_domain::futures_hedge::FuturesHedgeInput;
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeFuturesHedgeEngine;

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
