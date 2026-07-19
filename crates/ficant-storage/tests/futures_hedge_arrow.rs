use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::CalculateFuturesHedge;
use ficant_application::ports::FuturesHedgeArtifactCodec;
use ficant_domain::analytics::{AnalyticsObjectRef, FixedDecimal};
use ficant_domain::futures_delivery::CgbFuturesProduct;
use ficant_domain::futures_hedge::FuturesHedgeInput;
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeFuturesHedgeEngine;
use ficant_storage::hedge_arrow::ArrowFuturesHedgeCodec;

#[test]
fn native_hedge_round_trips_through_deterministic_arrow_file() {
    let original = input(fixed(500_000_000_000_000));
    let result = CalculateFuturesHedge::new(&NativeFuturesHedgeEngine)
        .execute(&original)
        .unwrap();
    let codec = ArrowFuturesHedgeCodec;
    let first = codec.encode(&result).unwrap();
    assert_eq!(
        hex(first.content_hash().as_bytes()),
        "dc640200044a8c10c7b826e0a96ff8852d647656146f12508ef29ec39259e9e7"
    );
    let second = codec.encode(&result).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.content_hash(), second.content_hash());
    assert_eq!(codec.decode(first.bytes(), &original).unwrap(), result);

    let drifted = input(fixed(501_000_000_000_000));
    assert_ne!(drifted.fingerprint(), original.fingerprint());
    assert!(codec.decode(first.bytes(), &drifted).is_err());
}

fn input(target_dv01: FixedDecimal) -> FuturesHedgeInput {
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
        target_dv01,
        fixed(45_000_000_000),
        fixed(900_000_000_000),
    )
    .unwrap()
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

const fn fixed(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut value, byte| {
        write!(value, "{byte:02x}").unwrap();
        value
    })
}
