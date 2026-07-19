use chrono::{NaiveDate, TimeZone, Utc};
use ficant_domain::analytics::{
    AnalyticsObjectRef, BondTerms, BusinessDayConvention, CouponFrequency, DayCountConvention,
    FixedDecimal,
};
use ficant_domain::futures_delivery::{CgbFuturesProduct, FuturesDeliverableInput, is_deliverable};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};

#[test]
fn all_four_products_accept_and_reject_exact_residual_boundaries() {
    for (product, minimum, maximum) in [
        (CgbFuturesProduct::TwoYear, 18, Some(27)),
        (CgbFuturesProduct::FiveYear, 48, Some(63)),
        (CgbFuturesProduct::TenYear, 78, None),
        (CgbFuturesProduct::ThirtyYear, 300, None),
    ] {
        let delivery = date(2026, 9, 1);
        let minimum_terms = terms(date(2025, 1, 1), add_months(delivery, minimum), product);
        assert!(is_deliverable(product, &minimum_terms, delivery).unwrap());
        let too_short = terms(
            date(2025, 1, 1),
            add_days(add_months(delivery, minimum), -1),
            product,
        );
        assert!(!is_deliverable(product, &too_short, delivery).unwrap());
        if let Some(maximum) = maximum {
            let upper = terms(date(2025, 1, 1), add_months(delivery, maximum), product);
            assert!(is_deliverable(product, &upper, delivery).unwrap());
            let too_long = terms(
                date(2025, 1, 1),
                add_days(add_months(delivery, maximum), 1),
                product,
            );
            assert!(!is_deliverable(product, &too_long, delivery).unwrap());
        }
    }
}

#[test]
fn input_rejects_non_quarter_delivery_and_ineligible_bond() {
    let mut input = valid_input(CgbFuturesProduct::TenYear);
    input.7 = date(2026, 8, 1);
    input.8 = date(2026, 8, 18);
    assert!(build(input).is_err());

    let mut input = valid_input(CgbFuturesProduct::TenYear);
    input.10 = terms(
        date(2024, 1, 1),
        date(2032, 12, 31),
        CgbFuturesProduct::FiveYear,
    );
    assert!(build(input).is_err());
}

type InputTuple = (
    OwnerRef,
    AnalyticsObjectRef,
    AnalyticsObjectRef,
    AnalyticsObjectRef,
    AnalyticsObjectRef,
    MarketTime,
    NaiveDate,
    NaiveDate,
    NaiveDate,
    CgbFuturesProduct,
    BondTerms,
);

fn valid_input(product: CgbFuturesProduct) -> InputTuple {
    let delivery = date(2026, 9, 1);
    let maturity = match product {
        CgbFuturesProduct::TwoYear => date(2028, 9, 1),
        CgbFuturesProduct::FiveYear => date(2031, 9, 1),
        CgbFuturesProduct::TenYear => date(2034, 9, 1),
        CgbFuturesProduct::ThirtyYear => date(2052, 9, 1),
    };
    (
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
        delivery,
        date(2026, 9, 18),
        product,
        terms(date(2024, 1, 1), maturity, product),
    )
}

fn build(value: InputTuple) -> ficant_domain::DomainResult<FuturesDeliverableInput> {
    FuturesDeliverableInput::new(
        value.0,
        value.1,
        value.2,
        value.3,
        value.4,
        value.5,
        value.6,
        value.7,
        value.8,
        value.9,
        value.10,
        fixed("101.25"),
        fixed("99.50"),
        fixed("0.018"),
    )
}

fn terms(issue: NaiveDate, maturity: NaiveDate, product: CgbFuturesProduct) -> BondTerms {
    let max_issue = match product {
        CgbFuturesProduct::TwoYear => add_months(maturity, -60),
        CgbFuturesProduct::FiveYear => add_months(maturity, -84),
        CgbFuturesProduct::TenYear => add_months(maturity, -120),
        CgbFuturesProduct::ThirtyYear => add_months(maturity, -360),
    };
    BondTerms::new(
        issue.max(max_issue),
        maturity,
        CouponFrequency::Semiannual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        fixed("0.025"),
        fixed("100"),
    )
    .unwrap()
}

fn add_months(value: NaiveDate, months: i32) -> NaiveDate {
    if months >= 0 {
        value
            .checked_add_months(chrono::Months::new(months.unsigned_abs()))
            .unwrap()
    } else {
        value
            .checked_sub_months(chrono::Months::new(months.unsigned_abs()))
            .unwrap()
    }
}

fn add_days(value: NaiveDate, days: i64) -> NaiveDate {
    value
        .checked_add_signed(chrono::TimeDelta::days(days))
        .unwrap()
}

fn fixed(raw: &str) -> FixedDecimal {
    let decimal = raw.parse::<rust_decimal::Decimal>().unwrap();
    let mut value = decimal * rust_decimal::Decimal::from(1_000_000_000_000_i64);
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
