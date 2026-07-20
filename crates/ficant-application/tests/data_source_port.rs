use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{AccessScope, IdempotencyKey, RegisterDataSource};
use ficant_domain::market::{DataSource, DataSourceInput, DataSourceKind};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, Version};

#[test]
fn registration_binds_scope_version_and_every_non_secret_business_field() {
    let owner = owner();
    let scope = scope(&owner);
    let first = RegisterDataSource::new(
        scope.clone(),
        None,
        source(owner.clone(), 1, "binding-a", "quotes-a"),
        IdempotencyKey::new("source-v1").unwrap(),
    )
    .unwrap();
    assert_eq!(first.scope(), &scope);
    assert_eq!(first.expected_latest_version(), None);

    let changed = RegisterDataSource::new(
        scope,
        None,
        source(owner, 1, "binding-b", "quotes-a"),
        IdempotencyKey::new("source-v1").unwrap(),
    )
    .unwrap();
    assert_ne!(first.fingerprint(), changed.fingerprint());
}

#[test]
fn registration_rejects_unauthorized_owner_and_non_next_version() {
    let owner = owner();
    let forbidden_scope = AccessScope::new(
        owner.tenant_id().clone(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F00").unwrap(),
        vec![Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F09").unwrap()],
    )
    .unwrap();
    let forbidden = RegisterDataSource::new(
        forbidden_scope,
        None,
        source(owner.clone(), 1, "binding-a", "quotes-a"),
        IdempotencyKey::new("forbidden").unwrap(),
    )
    .unwrap_err();
    assert_eq!(forbidden.category(), ApplicationErrorCategory::Forbidden);

    let wrong_version = RegisterDataSource::new(
        scope(&owner),
        Some(Version::new(1).unwrap()),
        source(owner, 3, "binding-a", "quotes-a"),
        IdempotencyKey::new("wrong-version").unwrap(),
    )
    .unwrap_err();
    assert_eq!(
        wrong_version.category(),
        ApplicationErrorCategory::VersionConflict
    );
}

fn source(owner: OwnerRef, version: u64, binding: &str, dataset: &str) -> DataSource {
    DataSource::new(DataSourceInput {
        data_source_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F10").unwrap(),
        version: Version::new(version).unwrap(),
        owner,
        kind: DataSourceKind::FileNdjson,
        name: "CGB primary quotes".to_owned(),
        connection_binding: binding.to_owned(),
        dataset: dataset.to_owned(),
        canonical_schema_id: "ficant.market.quote.canonical.v1".to_owned(),
        canonical_schema_hash: ContentHash::digest(b"canonical-schema"),
    })
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    )
}

fn scope(owner: &OwnerRef) -> AccessScope {
    AccessScope::new(
        owner.tenant_id().clone(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F00").unwrap(),
        vec![owner.owner_id().clone()],
    )
    .unwrap()
}
