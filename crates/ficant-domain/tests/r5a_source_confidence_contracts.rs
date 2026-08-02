use ficant_domain::market::{
    DataSource, DataSourceInput, DataSourceKind, FactSource, PriceSourceType,
};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, Version, VersionRef};
use ficant_domain::research::{PriceSourceCount, PriceSourceSummary};

#[test]
fn exact_data_source_versions_have_closed_external_price_types() {
    for source_type in [
        PriceSourceType::RealTrade,
        PriceSourceType::ActiveQuote,
        PriceSourceType::ModelValuation,
    ] {
        let source = legacy_source()
            .with_price_source_type(source_type)
            .expect("an external source version declares one closed price type");
        assert_eq!(source.price_source_type(), Some(source_type));
        assert!(
            source
                .clone()
                .with_price_source_type(PriceSourceType::ActiveQuote)
                .is_err(),
            "an immutable exact version cannot be retyped"
        );
    }

    assert!(
        legacy_source()
            .with_price_source_type(PriceSourceType::CurveInterpolation)
            .is_err(),
        "curve interpolation is internal algorithm evidence, not an external DataSource property"
    );
    assert_eq!(legacy_source().price_source_type(), None);
}

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

fn legacy_source() -> DataSource {
    DataSource::new(DataSourceInput {
        data_source_id: id('S'),
        version: version(),
        owner: OwnerRef::new(id('T'), id('O')),
        kind: DataSourceKind::FileNdjson,
        name: "CGB price evidence".to_owned(),
        connection_binding: "source-file".to_owned(),
        dataset: "cgb_prices".to_owned(),
        canonical_schema_id: "ficant.market.quote.canonical.v1".to_owned(),
        canonical_schema_hash: ContentHash::digest(b"canonical-schema"),
    })
    .unwrap()
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'O' => '0',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
