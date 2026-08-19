use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ficant_api::{
    DataSourceRegistryGrpcService, PlatformApplication, PlatformPort, SessionPolicy, SystemClock,
    TrustedIdentity,
};
use ficant_application::ports::{
    AccessScope, AeadCursorCodec, Cursor, CursorKey, CursorPage, DataSourceAuthorizationRepository,
    DataSourceRepository, PageRequest, PublishDataSourceAuthorization, RegisterDataSource,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as pb;
use ficant_contracts::ficant::market::v1::data_source_registry_service_server::DataSourceRegistryService;
use ficant_domain::VersionedDefinition;
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::{DataSource, DataSourceAuthorization, ImportInterface};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, VersionRef};
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct Repository {
    values: Mutex<Vec<DataSource>>,
    writes: AtomicUsize,
    list_calls: AtomicUsize,
    seen_list_cursor: Mutex<Vec<bool>>,
    next_cursor: Mutex<Option<Cursor>>,
}

#[async_trait]
impl DataSourceRepository for Repository {
    async fn register(&self, command: RegisterDataSource) -> Result<DataSource, ApplicationError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        let mut values = self.values.lock().unwrap();
        if let Some(existing) = values.iter().find(|value| {
            value.id() == command.value().id() && value.version() == command.value().version()
        }) {
            return if existing == command.value() {
                Ok(existing.clone())
            } else {
                Err(ApplicationError::new(
                    ApplicationErrorCategory::ImmutableViolation,
                    false,
                ))
            };
        }
        values.push(command.value().clone());
        Ok(command.value().clone())
    }

    async fn get_exact(
        &self,
        _: &AccessScope,
        reference: VersionRef,
    ) -> Result<Option<DataSource>, ApplicationError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .iter()
            .find(|value| {
                value.id() == reference.id() && value.version() == reference.version().get()
            })
            .cloned())
    }
}

#[async_trait]
impl DataSourceAuthorizationRepository for Repository {
    async fn publish_authorization(
        &self,
        command: PublishDataSourceAuthorization,
    ) -> Result<DataSourceAuthorization, ApplicationError> {
        Ok(command.value().clone())
    }

    async fn get_authorization_exact(
        &self,
        _: &AccessScope,
        _: VersionRef,
    ) -> Result<Option<DataSourceAuthorization>, ApplicationError> {
        Ok(None)
    }

    async fn list_authorizations_for_source(
        &self,
        _: &AccessScope,
        _: &OwnerRef,
        _: &VersionRef,
        _: Option<ImportInterface>,
        page: PageRequest,
    ) -> Result<CursorPage<DataSourceAuthorization>, ApplicationError> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        self.seen_list_cursor
            .lock()
            .unwrap()
            .push(page.cursor().is_some());
        Ok(CursorPage::new(
            Vec::new(),
            self.next_cursor.lock().unwrap().clone(),
        ))
    }
}

#[tokio::test]
async fn registry_round_trips_three_external_types_and_rejects_invalid_types_before_write() {
    fn assert_service<T: DataSourceRegistryService>() {}
    assert_service::<DataSourceRegistryGrpcService>();

    let repository = Arc::new(Repository::default());
    let service = service(repository.clone());
    for (suffix, source_type) in [
        ('1', pb::PriceSourceType::RealTrade),
        ('2', pb::PriceSourceType::ActiveQuote),
        ('3', pb::PriceSourceType::ModelValuation),
    ] {
        let definition = definition(suffix, source_type as i32);
        let registered = service
            .register_data_source(Request::new(pb::RegisterDataSourceRequest {
                idempotency_key: format!("r5a-source-{suffix}"),
                expected_latest_version: 0,
                definition: Some(definition.clone()),
                change: Some(change()),
            }))
            .await
            .unwrap()
            .into_inner();
        let Some(pb::register_data_source_response::Result::Definition(stored)) = registered.result
        else {
            panic!("valid external source type must register");
        };
        assert_eq!(stored.price_source_type, source_type as i32);

        let queried = service
            .get_data_source(Request::new(pb::GetDataSourceRequest {
                data_source: definition.data_source,
            }))
            .await
            .unwrap()
            .into_inner();
        let Some(pb::get_data_source_response::Result::Definition(stored)) = queried.result else {
            panic!("exact registered source must be queryable");
        };
        assert_eq!(stored.price_source_type, source_type as i32);
    }
    assert_eq!(repository.writes.load(Ordering::SeqCst), 3);

    for invalid_type in [
        pb::PriceSourceType::Unspecified as i32,
        pb::PriceSourceType::CurveInterpolation as i32,
        99,
    ] {
        let response = service
            .register_data_source(Request::new(pb::RegisterDataSourceRequest {
                idempotency_key: format!("r5a-invalid-{invalid_type}"),
                expected_latest_version: 0,
                definition: Some(definition('9', invalid_type)),
                change: Some(change()),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            response.result,
            Some(pb::register_data_source_response::Result::Error(_))
        ));
    }
    assert_eq!(repository.writes.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn scope_only_identity_cannot_mutate_foundation_data() {
    let repository = Arc::new(Repository::default());
    let service = service_with_role(repository.clone(), PlatformRole::Researcher);

    let response = service
        .register_data_source(Request::new(pb::RegisterDataSourceRequest {
            idempotency_key: "r6a-scope-only-must-fail".to_owned(),
            expected_latest_version: 0,
            definition: Some(definition('8', pb::PriceSourceType::ActiveQuote as i32)),
            change: Some(change()),
        }))
        .await
        .unwrap()
        .into_inner();

    let Some(pb::register_data_source_response::Result::Error(error)) = response.result else {
        panic!("a write scope without an active Platform Admin role must fail closed");
    };
    assert_eq!(error.code, core::ErrorCode::Forbidden as i32);
    assert_eq!(repository.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn authorization_list_round_trips_a_scope_bound_page_cursor() {
    let repository = Arc::new(Repository::default());
    let codec = cursor_codec();
    let scope = AccessScope::new(id('T'), id('A'), vec![id('B')]).unwrap();
    *repository.next_cursor.lock().unwrap() =
        Some(Cursor::issue(codec.as_ref(), &scope, "authorization-page-1").unwrap());
    let service =
        service_with_role_and_codec(repository.clone(), PlatformRole::PlatformAdmin, codec);

    let first = service
        .list_data_source_authorizations(Request::new(list_request(String::new())))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::list_data_source_authorizations_response::Result::Authorizations(first)) =
        first.result
    else {
        panic!("a valid first page must succeed")
    };
    let cursor = first.page.unwrap().next_cursor;
    assert!(!cursor.is_empty());

    let second = service
        .list_data_source_authorizations(Request::new(list_request(cursor.clone())))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        second.result,
        Some(pb::list_data_source_authorizations_response::Result::Authorizations(_))
    ));
    assert_eq!(repository.list_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        &*repository.seen_list_cursor.lock().unwrap(),
        &[false, true]
    );

    let denied = service
        .list_data_source_authorizations(Request::new(list_request(format!("{cursor}x"))))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::list_data_source_authorizations_response::Result::Error(error)) = denied.result
    else {
        panic!("a modified cursor must fail closed")
    };
    assert_eq!(error.code, core::ErrorCode::Forbidden as i32);
    assert_eq!(repository.list_calls.load(Ordering::SeqCst), 2);
}

fn service(repository: Arc<Repository>) -> DataSourceRegistryGrpcService {
    service_with_role(repository, PlatformRole::PlatformAdmin)
}

fn service_with_role(
    repository: Arc<Repository>,
    active_role: PlatformRole,
) -> DataSourceRegistryGrpcService {
    service_with_role_and_codec(repository, active_role, cursor_codec())
}

fn service_with_role_and_codec(
    repository: Arc<Repository>,
    active_role: PlatformRole,
    cursor_codec: Arc<AeadCursorCodec>,
) -> DataSourceRegistryGrpcService {
    let identity = TrustedIdentity::implicit(
        "data-source-test",
        id('A'),
        id('T'),
        vec![id('B')],
        active_role,
        ["data-sources:read", "data-sources:write"],
    )
    .unwrap();
    let application: Arc<dyn PlatformPort> = Arc::new(
        PlatformApplication::try_new(
            Arc::new(SystemClock),
            SessionPolicy::new(900, 60).unwrap(),
            KEY,
            vec![],
            Some(identity),
            vec![],
        )
        .unwrap(),
    );
    DataSourceRegistryGrpcService::new(
        application,
        repository.clone(),
        repository,
        cursor_codec,
        KEY,
    )
    .unwrap()
}

fn cursor_codec() -> Arc<AeadCursorCodec> {
    let mut key_material = [0_u8; 32];
    key_material.copy_from_slice(KEY);
    Arc::new(
        AeadCursorCodec::new(
            CursorKey::new("data-source-test", key_material).unwrap(),
            Vec::new(),
        )
        .unwrap(),
    )
}

fn list_request(cursor: String) -> pb::ListDataSourceAuthorizationsRequest {
    pb::ListDataSourceAuthorizationsRequest {
        owner: Some(core::OwnerRef {
            tenant_id: Some(proto_id('T')),
            owner_id: Some(proto_id('B')),
        }),
        data_source: Some(core::VersionRef {
            id: Some(proto_id('D')),
            version: 1,
        }),
        import_interface: pb::ImportInterface::CanonicalQuoteSnapshot as i32,
        page: Some(core::PageRequest {
            cursor,
            page_size: 1,
        }),
    }
}

fn change() -> core::ChangeJustification {
    core::ChangeJustification {
        reason: "register exact fixture source".to_owned(),
        sources: vec![core::SourceDocumentRef {
            uri: "urn:test:data-source".to_owned(),
            sha256: Some(core::Sha256 {
                value: ContentHash::digest(b"fixture-source-evidence")
                    .as_bytes()
                    .to_vec(),
            }),
        }],
    }
}

fn definition(suffix: char, source_type: i32) -> pb::DataSourceDefinition {
    pb::DataSourceDefinition {
        data_source: Some(core::VersionRef {
            id: Some(proto_id(suffix)),
            version: 1,
        }),
        owner: Some(core::OwnerRef {
            tenant_id: Some(proto_id('T')),
            owner_id: Some(proto_id('B')),
        }),
        kind: pb::DataSourceKind::FileNdjson as i32,
        name: format!("R5a source {suffix}"),
        connection_binding: format!("r5a-source-{suffix}"),
        dataset: format!("r5a_source_{suffix}"),
        canonical_schema_id: "ficant.market.quote.canonical.v1".to_owned(),
        canonical_schema_hash: Some(core::Sha256 {
            value: vec![suffix as u8; 32],
        }),
        price_source_type: source_type,
    }
}

fn proto_id(suffix: char) -> core::Ulid {
    core::Ulid {
        value: id(suffix).as_str().to_owned(),
    }
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F0{suffix}")).unwrap()
}
