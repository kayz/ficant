mod support;

use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{DataSourceRepository, IdempotencyKey, RegisterDataSource};
use ficant_domain::market::{DataSource, DataSourceInput, DataSourceKind, PriceSourceType};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, Version, VersionRef};

#[tokio::test]
async fn data_source_registration_is_append_only_idempotent_and_scope_bound() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool);
    let owner = owner();
    let scope = support::access_scope(&owner);

    let first = source(
        owner.clone(),
        1,
        DataSourceKind::FileNdjson,
        "source-file",
        PriceSourceType::ActiveQuote,
    );
    let command = RegisterDataSource::new(
        scope.clone(),
        None,
        first.clone(),
        IdempotencyKey::new("phase3a-source-v1").unwrap(),
    )
    .unwrap();
    assert_eq!(repository.register(command.clone()).await.unwrap(), first);
    assert_eq!(repository.register(command).await.unwrap(), first);

    let changed_replay = RegisterDataSource::new(
        scope.clone(),
        None,
        source(
            owner.clone(),
            1,
            DataSourceKind::FileNdjson,
            "other-binding",
            PriceSourceType::ActiveQuote,
        ),
        IdempotencyKey::new("phase3a-source-v1").unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository
            .register(changed_replay)
            .await
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::AlreadyExists
    );

    let second = source(
        owner.clone(),
        2,
        DataSourceKind::Postgres,
        "source-pg",
        PriceSourceType::RealTrade,
    );
    let append = RegisterDataSource::new(
        scope.clone(),
        Some(Version::new(1).unwrap()),
        second.clone(),
        IdempotencyKey::new("phase3a-source-v2").unwrap(),
    )
    .unwrap();
    assert_eq!(repository.register(append).await.unwrap(), second);

    let exact = repository
        .get_exact(
            &scope,
            VersionRef::new(first.id().clone(), Version::new(1).unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(exact, Some(first));
    assert_eq!(
        repository
            .get_exact(
                &scope,
                VersionRef::new(second.id().clone(), Version::new(2).unwrap()),
            )
            .await
            .unwrap()
            .unwrap()
            .price_source_type(),
        Some(PriceSourceType::RealTrade)
    );

    let forbidden_scope = ficant_application::ports::AccessScope::new(
        owner.tenant_id().clone(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F00").unwrap(),
        vec![Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F09").unwrap()],
    )
    .unwrap();
    let forbidden = repository
        .get_exact(
            &forbidden_scope,
            VersionRef::new(second.id().clone(), Version::new(2).unwrap()),
        )
        .await
        .unwrap_err();
    assert_eq!(forbidden.category(), ApplicationErrorCategory::Forbidden);
}

fn source(
    owner: OwnerRef,
    version: u64,
    kind: DataSourceKind,
    binding: &str,
    source_type: PriceSourceType,
) -> DataSource {
    DataSource::new(DataSourceInput {
        data_source_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F10").unwrap(),
        version: Version::new(version).unwrap(),
        owner,
        kind,
        name: "CGB primary quotes".to_owned(),
        connection_binding: binding.to_owned(),
        dataset: "cgb_quotes".to_owned(),
        canonical_schema_id: "ficant.market.quote.canonical.v1".to_owned(),
        canonical_schema_hash: ContentHash::digest(b"canonical-schema"),
    })
    .unwrap()
    .with_price_source_type(source_type)
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    )
}
