use ficant_domain::VersionedDefinition;
use ficant_domain::market::{DataSource, DataSourceInput, DataSourceKind};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, Version};

#[test]
fn data_source_is_versioned_non_secret_and_schema_bound() {
    let source = source(
        "source-file",
        "cgb_quotes",
        "ficant.market.quote.canonical.v1",
    )
    .expect("valid source");
    assert_eq!(source.identity(), "01ARZ3NDEKTSV4RRFFQ69G5F10");
    assert_eq!(source.version(), 1);
    assert_eq!(source.kind(), DataSourceKind::FileNdjson);
    assert_eq!(source.connection_binding(), "source-file");
    assert_eq!(source.dataset(), "cgb_quotes");
    assert_eq!(
        source.canonical_schema_hash(),
        &ContentHash::digest(b"canonical-schema")
    );
}

#[test]
fn data_source_rejects_paths_urls_credentials_and_unversioned_schema_ids() {
    for invalid in [
        " CGB",
        "C:\\secret\\quotes.ndjson",
        "postgres://user:password@host/db",
        "../../quotes",
        "binding/child",
    ] {
        assert!(source(invalid, "cgb_quotes", "ficant.market.quote.canonical.v1").is_err());
        assert!(source("source-file", invalid, "ficant.market.quote.canonical.v1").is_err());
    }
    assert!(source("source-file", "cgb_quotes", "market.quote").is_err());
    assert!(source("source-file", "cgb_quotes", "other.market.quote.v1").is_err());
}

fn source(
    connection_binding: &str,
    dataset: &str,
    canonical_schema_id: &str,
) -> ficant_domain::DomainResult<DataSource> {
    DataSource::new(DataSourceInput {
        data_source_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F10").unwrap(),
        version: Version::new(1).unwrap(),
        owner: OwnerRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
        ),
        kind: DataSourceKind::FileNdjson,
        name: "CGB primary quotes".to_owned(),
        connection_binding: connection_binding.to_owned(),
        dataset: dataset.to_owned(),
        canonical_schema_id: canonical_schema_id.to_owned(),
        canonical_schema_hash: ContentHash::digest(b"canonical-schema"),
    })
}
