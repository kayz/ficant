use ficant_domain::market::{FactSource, PriceSourceType};
use ficant_domain::primitives::{Ulid, Version, VersionRef};
use ficant_domain::research::{PriceSourceCount, PriceSourceSummary};

#[test]
fn fact_source_binds_an_exact_data_source_without_copying_its_type() {
    let reference = VersionRef::new(id('S'), version());
    let source = FactSource::new("vendor-feed", "quote-001", 1)
        .unwrap()
        .with_data_source(reference.clone())
        .unwrap();

    assert_eq!(source.data_source(), Some(&reference));
    assert!(source.with_data_source(reference).is_err());
}

#[test]
fn source_summary_is_sorted_nonempty_and_derives_mixed_exactly() {
    let single = PriceSourceSummary::new(vec![
        PriceSourceCount::new(PriceSourceType::CurveInterpolation, 2).unwrap(),
    ])
    .unwrap();
    assert!(!single.mixed());
    assert_eq!(single.counts()[0].record_count(), 2);

    let mixed = PriceSourceSummary::new(vec![
        PriceSourceCount::new(PriceSourceType::ActiveQuote, 3).unwrap(),
        PriceSourceCount::new(PriceSourceType::CurveInterpolation, 2).unwrap(),
    ])
    .unwrap();
    assert!(mixed.mixed());
    assert_eq!(mixed.counts().len(), 2);

    assert!(PriceSourceSummary::new(Vec::new()).is_err());
    assert!(
        PriceSourceSummary::new(vec![
            PriceSourceCount::new(PriceSourceType::CurveInterpolation, 1).unwrap(),
            PriceSourceCount::new(PriceSourceType::ActiveQuote, 1).unwrap(),
        ])
        .is_err(),
        "callers cannot smuggle unstable ordering into a content hash"
    );
    assert!(PriceSourceCount::new(PriceSourceType::RealTrade, 0).is_err());
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
