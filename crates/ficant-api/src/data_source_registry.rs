use std::sync::Arc;

use ficant_application::ports::{
    AccessScope, DataSourceRepository, IdempotencyKey, RegisterDataSource,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, DataSourceUseCase, map_domain_error,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as pb;
use ficant_contracts::ficant::market::v1::data_source_registry_service_server::DataSourceRegistryService;
use ficant_domain::VersionedDefinition;
use ficant_domain::market::{DataSource, DataSourceInput, DataSourceKind, PriceSourceType};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, Version, VersionRef};
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;

const READ_SCOPE: &str = "data-sources:read";
const WRITE_SCOPE: &str = "data-sources:write";

#[derive(Clone)]
pub struct DataSourceRegistryGrpcService {
    identity: Arc<dyn PlatformPort>,
    access_scope: AccessScope,
    repository: Arc<dyn DataSourceRepository>,
    errors: CoreBusinessErrorMapper,
}

impl DataSourceRegistryGrpcService {
    /// Composes the authenticated exact-version `DataSource` registry.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the trace-key contract is invalid.
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        access_scope: AccessScope,
        repository: Arc<dyn DataSourceRepository>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            access_scope,
            repository,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn authorize(
        &self,
        request: &Request<impl Sized>,
        required_scope: &str,
    ) -> Result<(), ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|_| forbidden())?;
        session
            .has_scope(required_scope)
            .then_some(())
            .ok_or_else(forbidden)
    }
}

#[tonic::async_trait]
impl DataSourceRegistryService for DataSourceRegistryGrpcService {
    async fn register_data_source(
        &self,
        request: Request<pb::RegisterDataSourceRequest>,
    ) -> Result<Response<pb::RegisterDataSourceResponse>, Status> {
        const OPERATION: &str = "data-sources.register";
        let result = if let Err(error) = self.authorize(&request, WRITE_SCOPE) {
            Err(error)
        } else {
            let request = request.get_ref();
            match parse_definition(request.definition.as_ref()).and_then(|value| {
                let expected_latest_version = if request.expected_latest_version == 0 {
                    None
                } else {
                    Some(Version::new(request.expected_latest_version).map_err(map_domain_error)?)
                };
                RegisterDataSource::new(
                    self.access_scope.clone(),
                    expected_latest_version,
                    value,
                    IdempotencyKey::new(request.idempotency_key.clone())?,
                )
            }) {
                Ok(command) => {
                    DataSourceUseCase::new(self.repository.as_ref())
                        .register(command)
                        .await
                }
                Err(error) => Err(error),
            }
        };
        Ok(Response::new(pb::RegisterDataSourceResponse {
            result: Some(match result {
                Ok(value) => {
                    pb::register_data_source_response::Result::Definition(definition(&value))
                }
                Err(error) => pb::register_data_source_response::Result::Error(self.errors.map(
                    OPERATION,
                    "data-source-application",
                    &error,
                )),
            }),
        }))
    }

    async fn get_data_source(
        &self,
        request: Request<pb::GetDataSourceRequest>,
    ) -> Result<Response<pb::GetDataSourceResponse>, Status> {
        const OPERATION: &str = "data-sources.get-exact";
        let result = match self.authorize(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => match parse_version_ref(request.get_ref().data_source.as_ref()) {
                Ok(reference) => {
                    DataSourceUseCase::new(self.repository.as_ref())
                        .get_exact(&self.access_scope, &reference)
                        .await
                }
                Err(error) => Err(error),
            },
        };
        Ok(Response::new(pb::GetDataSourceResponse {
            result: Some(match result {
                Ok(value) => pb::get_data_source_response::Result::Definition(definition(&value)),
                Err(error) => pb::get_data_source_response::Result::Error(self.errors.map(
                    OPERATION,
                    "data-source-application",
                    &error,
                )),
            }),
        }))
    }
}

fn parse_definition(
    value: Option<&pb::DataSourceDefinition>,
) -> Result<DataSource, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let reference = parse_version_ref(value.data_source.as_ref())?;
    let kind = match pb::DataSourceKind::try_from(value.kind).map_err(|_| invalid())? {
        pb::DataSourceKind::FileNdjson => DataSourceKind::FileNdjson,
        pb::DataSourceKind::Postgres => DataSourceKind::Postgres,
        pb::DataSourceKind::Unspecified => return Err(invalid()),
    };
    let source_type =
        match pb::PriceSourceType::try_from(value.price_source_type).map_err(|_| invalid())? {
            pb::PriceSourceType::RealTrade => PriceSourceType::RealTrade,
            pb::PriceSourceType::ActiveQuote => PriceSourceType::ActiveQuote,
            pb::PriceSourceType::ModelValuation => PriceSourceType::ModelValuation,
            pb::PriceSourceType::CurveInterpolation | pb::PriceSourceType::Unspecified => {
                return Err(invalid());
            }
        };
    DataSource::new(DataSourceInput {
        data_source_id: reference.id().clone(),
        version: reference.version(),
        owner: parse_owner(value.owner.as_ref())?,
        kind,
        name: value.name.clone(),
        connection_binding: value.connection_binding.clone(),
        dataset: value.dataset.clone(),
        canonical_schema_id: value.canonical_schema_id.clone(),
        canonical_schema_hash: parse_hash(value.canonical_schema_hash.as_ref())?,
    })
    .and_then(|source| source.with_price_source_type(source_type))
    .map_err(map_domain_error)
}

fn definition(value: &DataSource) -> pb::DataSourceDefinition {
    pb::DataSourceDefinition {
        data_source: Some(version_ref(&VersionRef::new(
            value.id().clone(),
            Version::new(value.version()).expect("domain DataSource versions are non-zero"),
        ))),
        owner: Some(owner(value.owner())),
        kind: match value.kind() {
            DataSourceKind::FileNdjson => pb::DataSourceKind::FileNdjson as i32,
            DataSourceKind::Postgres => pb::DataSourceKind::Postgres as i32,
        },
        name: value.name().to_owned(),
        connection_binding: value.connection_binding().to_owned(),
        dataset: value.dataset().to_owned(),
        canonical_schema_id: value.canonical_schema_id().to_owned(),
        canonical_schema_hash: Some(core::Sha256 {
            value: value.canonical_schema_hash().as_bytes().to_vec(),
        }),
        price_source_type: match value
            .price_source_type()
            .expect("typed registry never returns legacy DataSource versions")
        {
            PriceSourceType::RealTrade => pb::PriceSourceType::RealTrade as i32,
            PriceSourceType::ActiveQuote => pb::PriceSourceType::ActiveQuote as i32,
            PriceSourceType::ModelValuation => pb::PriceSourceType::ModelValuation as i32,
            PriceSourceType::CurveInterpolation => {
                unreachable!("internal interpolation is not a registered DataSource")
            }
        },
    }
}

fn parse_owner(value: Option<&core::OwnerRef>) -> Result<OwnerRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(OwnerRef::new(
        parse_ulid(value.tenant_id.as_ref())?,
        parse_ulid(value.owner_id.as_ref())?,
    ))
}

fn parse_version_ref(value: Option<&core::VersionRef>) -> Result<VersionRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(VersionRef::new(
        parse_ulid(value.id.as_ref())?,
        Version::new(value.version).map_err(map_domain_error)?,
    ))
}

fn parse_hash(value: Option<&core::Sha256>) -> Result<ContentHash, ApplicationError> {
    ContentHash::from_bytes(&value.ok_or_else(invalid)?.value).map_err(map_domain_error)
}

fn parse_ulid(value: Option<&core::Ulid>) -> Result<Ulid, ApplicationError> {
    Ulid::new(value.ok_or_else(invalid)?.value.clone()).map_err(map_domain_error)
}

fn owner(value: &OwnerRef) -> core::OwnerRef {
    core::OwnerRef {
        tenant_id: Some(ulid(value.tenant_id())),
        owner_id: Some(ulid(value.owner_id())),
    }
}

fn version_ref(value: &VersionRef) -> core::VersionRef {
    core::VersionRef {
        id: Some(ulid(value.id())),
        version: value.version().get(),
    }
}

fn ulid(value: &Ulid) -> core::Ulid {
    core::Ulid {
        value: value.as_str().to_owned(),
    }
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}
