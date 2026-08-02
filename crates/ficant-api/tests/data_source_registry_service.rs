use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ficant_api::{
    DataSourceRegistryGrpcService, PlatformApplication, PlatformPort, SessionPolicy, SystemClock,
    TrustedIdentity,
};
use ficant_application::ports::{AccessScope, DataSourceRepository, RegisterDataSource};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as pb;
use ficant_contracts::ficant::market::v1::data_source_registry_service_server::DataSourceRegistryService;
use ficant_domain::VersionedDefinition;
use ficant_domain::market::DataSource;
use ficant_domain::primitives::{Ulid, VersionRef};
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct Repository {
    values: Mutex<Vec<DataSource>>,
    writes: AtomicUsize,
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

fn service(repository: Arc<Repository>) -> DataSourceRegistryGrpcService {
    let identity = TrustedIdentity::implicit(
        "data-source-test",
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
        AccessScope::new(id('T'), id('A'), vec![id('B')]).unwrap(),
        repository,
        KEY,
    )
    .unwrap()
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
