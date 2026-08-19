use chrono::{TimeZone, Utc};
use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    AuthorizedPrincipal, FoundationChangeContext, IdempotencyKey, RegisterDataSource,
};
use ficant_domain::governance::{ChangeJustification, PlatformRole, SourceDocumentRef};
use ficant_domain::market::{DataSource, DataSourceInput, DataSourceKind};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, Version};

#[test]
fn registration_binds_scope_version_and_every_non_secret_business_field() {
    let owner = test_owner();
    let context = context(&owner, 'R');
    let first = RegisterDataSource::new(
        context.clone(),
        None,
        source(owner.clone(), 1, "binding-a", "quotes-a"),
        IdempotencyKey::new("source-v1").unwrap(),
    )
    .unwrap();
    assert_eq!(first.scope(), context.principal().access_scope());
    assert_eq!(first.expected_latest_version(), None);

    let changed = RegisterDataSource::new(
        context,
        None,
        source(owner, 1, "binding-b", "quotes-a"),
        IdempotencyKey::new("source-v1").unwrap(),
    )
    .unwrap();
    assert_ne!(first.fingerprint(), changed.fingerprint());
}

#[test]
fn registration_rejects_unauthorized_owner_and_non_next_version() {
    let owner = test_owner();
    let forbidden_owner = owner.clone();
    let forbidden_context = context(&OwnerRef::new(owner.tenant_id().clone(), id('9')), 'S');
    let forbidden = RegisterDataSource::new(
        forbidden_context,
        None,
        source(forbidden_owner, 1, "binding-a", "quotes-a"),
        IdempotencyKey::new("forbidden").unwrap(),
    )
    .unwrap_err();
    assert_eq!(forbidden.category(), ApplicationErrorCategory::Forbidden);

    let wrong_version = RegisterDataSource::new(
        context(&owner, 'T'),
        Some(Version::new(1).unwrap()),
        source(owner, 3, "binding-a", "quotes-a"),
        IdempotencyKey::new("wrong-version").unwrap(),
    )
    .unwrap_err();
    assert_eq!(
        wrong_version.category(),
        ApplicationErrorCategory::VersionConflict
    );

    let exhausted_owner = test_owner();
    let exhausted = RegisterDataSource::new(
        context(&exhausted_owner, 'W'),
        Some(Version::new(u64::MAX).unwrap()),
        source(exhausted_owner, u64::MAX, "binding-a", "quotes-a"),
        IdempotencyKey::new("exhausted-version-space").unwrap(),
    )
    .unwrap_err();
    assert_eq!(
        exhausted.category(),
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

fn test_owner() -> OwnerRef {
    OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    )
}

fn context(owner: &OwnerRef, record_suffix: char) -> FoundationChangeContext {
    let principal = AuthorizedPrincipal::new(
        "platform-admin".to_owned(),
        id('0'),
        owner.tenant_id().clone(),
        vec![owner.owner_id().clone()],
        PlatformRole::PlatformAdmin,
        vec!["data-sources:write".to_owned()],
        ContentHash::digest(b"credential"),
    )
    .unwrap();
    FoundationChangeContext::administrator(
        principal,
        ChangeJustification::new(
            "register exact data source",
            vec![
                SourceDocumentRef::new("urn:test:source", ContentHash::digest(b"evidence"))
                    .unwrap(),
            ],
        )
        .unwrap(),
        id(record_suffix),
        ficant_domain::primitives::MarketTime::new(
            Utc.with_ymd_and_hms(2026, 8, 13, 0, 0, 0).unwrap(),
            "UTC",
            chrono::NaiveDate::from_ymd_opt(2026, 8, 13).unwrap(),
        )
        .unwrap(),
    )
    .unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
