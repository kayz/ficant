use std::sync::Arc;

use chrono::{DateTime, Utc};
use ficant_application::ports::{
    AeadCursorCodec, Cursor, DataSourceAuthorizationRepository, DataSourceRepository,
    FoundationChangeContext, IdempotencyKey, PageRequest, PublishDataSourceAuthorization,
    RegisterDataSource,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, DataSourceUseCase, GovernedInputUseCase,
    map_domain_error,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as pb;
use ficant_contracts::ficant::market::v1::data_source_registry_service_server::DataSourceRegistryService;
use ficant_data::{InstrumentMapping, InstrumentMappingEntry};
use ficant_domain::VersionedDefinition;
use ficant_domain::governance::{
    ChangeJustification, FoundationResourceKind, FoundationResourceRef, SourceDocumentRef,
    deterministic_change_record_id,
};
use ficant_domain::market::{
    DataSource, DataSourceAuthorization, DataSourceAuthorizationInput,
    DataSourceAuthorizationState, DataSourceInput, DataSourceKind, ImportInterface,
    PriceSourceType,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;

const READ_SCOPE: &str = "data-sources:read";
const WRITE_SCOPE: &str = "data-sources:write";
const DEFAULT_PAGE_SIZE: u32 = 100;

#[derive(Clone)]
pub struct DataSourceRegistryGrpcService {
    identity: Arc<dyn PlatformPort>,
    repository: Arc<dyn DataSourceRepository>,
    authorizations: Arc<dyn DataSourceAuthorizationRepository>,
    cursor_codec: Arc<AeadCursorCodec>,
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
        repository: Arc<dyn DataSourceRepository>,
        authorizations: Arc<dyn DataSourceAuthorizationRepository>,
        cursor_codec: Arc<AeadCursorCodec>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            repository,
            authorizations,
            cursor_codec,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn principal(
        &self,
        request: &Request<impl Sized>,
        required_scope: &str,
    ) -> Result<ficant_application::ports::AuthorizedPrincipal, ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|_| forbidden())?;
        let principal = session.authorized_principal()?;
        principal
            .has_scope(required_scope)
            .then_some(principal)
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
        let result = match self.principal(&request, WRITE_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) => {
                let request = request.get_ref();
                match parse_definition(request.definition.as_ref()).and_then(|value| {
                    let expected_latest_version = if request.expected_latest_version == 0 {
                        None
                    } else {
                        Some(
                            Version::new(request.expected_latest_version)
                                .map_err(map_domain_error)?,
                        )
                    };
                    let resource = FoundationResourceRef::versioned(
                        FoundationResourceKind::DataSource,
                        VersionRef::new(
                            value.id().clone(),
                            Version::new(value.version()).map_err(map_domain_error)?,
                        ),
                    );
                    let occurred_at = server_market_time();
                    let record_id = deterministic_change_record_id(
                        &occurred_at,
                        principal.actor_id(),
                        &resource,
                        &request.idempotency_key,
                    )
                    .map_err(map_domain_error)?;
                    let change = parse_change(request.change.as_ref())?;
                    RegisterDataSource::new(
                        FoundationChangeContext::administrator(
                            principal,
                            change,
                            record_id,
                            occurred_at,
                        )?,
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
        let result = match self.principal(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) => match parse_version_ref(request.get_ref().data_source.as_ref()) {
                Ok(reference) => {
                    DataSourceUseCase::new(self.repository.as_ref())
                        .get_exact(principal.access_scope(), &reference)
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

    async fn publish_data_source_authorization(
        &self,
        request: Request<pb::PublishDataSourceAuthorizationRequest>,
    ) -> Result<Response<pb::PublishDataSourceAuthorizationResponse>, Status> {
        const OPERATION: &str = "data-sources.publish-authorization";
        let result = match self.principal(&request, WRITE_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) => {
                let request = request.get_ref();
                match parse_authorization(request.authorization.as_ref()).and_then(|value| {
                    let mapping = parse_mapping(request.mapping.as_ref())?;
                    if mapping.id() != value.mapping_id()
                        || mapping.content_hash() != value.mapping_hash()
                        || mapping.owner() != value.owner()
                        || mapping.source() != value.data_source()
                    {
                        return Err(invalid());
                    }
                    let expected = nonzero_version(request.expected_latest_version)?;
                    let occurred_at = server_market_time();
                    let resource = FoundationResourceRef::versioned(
                        FoundationResourceKind::DataSourceAuthorization,
                        value.version_ref(),
                    );
                    let record_id = deterministic_change_record_id(
                        &occurred_at,
                        principal.actor_id(),
                        &resource,
                        &request.idempotency_key,
                    )
                    .map_err(map_domain_error)?;
                    PublishDataSourceAuthorization::new(
                        FoundationChangeContext::administrator(
                            principal,
                            parse_change(request.change.as_ref())?,
                            record_id,
                            occurred_at,
                        )?,
                        expected,
                        value,
                        IdempotencyKey::new(request.idempotency_key.clone())?,
                    )
                }) {
                    Ok(command) => {
                        GovernedInputUseCase::new(
                            self.authorizations.as_ref(),
                            self.repository.as_ref(),
                        )
                        .publish_authorization(command)
                        .await
                    }
                    Err(error) => Err(error),
                }
            }
        };
        Ok(Response::new(pb::PublishDataSourceAuthorizationResponse {
            result: Some(match result {
                Ok(value) => pb::publish_data_source_authorization_response::Result::Authorization(
                    authorization(&value),
                ),
                Err(error) => pb::publish_data_source_authorization_response::Result::Error(
                    self.errors
                        .map(OPERATION, "data-source-authorization", &error),
                ),
            }),
        }))
    }

    async fn get_data_source_authorization(
        &self,
        request: Request<pb::GetDataSourceAuthorizationRequest>,
    ) -> Result<Response<pb::GetDataSourceAuthorizationResponse>, Status> {
        const OPERATION: &str = "data-sources.get-authorization";
        let result = match self.principal(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) => {
                match parse_version_ref(request.get_ref().authorization_ref.as_ref()) {
                    Err(error) => Err(error),
                    Ok(reference) => self
                        .authorizations
                        .get_authorization_exact(principal.access_scope(), reference)
                        .await
                        .and_then(|value| value.ok_or_else(not_found)),
                }
            }
        };
        Ok(Response::new(pb::GetDataSourceAuthorizationResponse {
            result: Some(match result {
                Ok(value) => pb::get_data_source_authorization_response::Result::Authorization(
                    authorization(&value),
                ),
                Err(error) => pb::get_data_source_authorization_response::Result::Error(
                    self.errors
                        .map(OPERATION, "data-source-authorization", &error),
                ),
            }),
        }))
    }

    async fn list_data_source_authorizations(
        &self,
        request: Request<pb::ListDataSourceAuthorizationsRequest>,
    ) -> Result<Response<pb::ListDataSourceAuthorizationsResponse>, Status> {
        const OPERATION: &str = "data-sources.list-authorizations";
        let result = match self.principal(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) => {
                let request = request.get_ref();
                let requested = request.page.clone().unwrap_or_default();
                let limit = if requested.page_size == 0 {
                    DEFAULT_PAGE_SIZE
                } else {
                    requested.page_size
                };
                match (
                    parse_owner(request.owner.as_ref()),
                    parse_version_ref(request.data_source.as_ref()),
                    parse_optional_import_interface(request.import_interface),
                    parse_cursor(
                        self.cursor_codec.as_ref(),
                        principal.access_scope(),
                        &requested.cursor,
                    ),
                ) {
                    (Ok(owner), Ok(source), Ok(interface), Ok(cursor)) => {
                        match PageRequest::new(principal.access_scope().clone(), cursor, limit) {
                            Ok(page) => {
                                self.authorizations
                                    .list_authorizations_for_source(
                                        principal.access_scope(),
                                        &owner,
                                        &source,
                                        interface,
                                        page,
                                    )
                                    .await
                            }
                            Err(error) => Err(error),
                        }
                    }
                    (Err(error), _, _, _)
                    | (_, Err(error), _, _)
                    | (_, _, Err(error), _)
                    | (_, _, _, Err(error)) => Err(error),
                }
            }
        };
        Ok(Response::new(pb::ListDataSourceAuthorizationsResponse {
            result: Some(match result {
                Ok(values) => pb::list_data_source_authorizations_response::Result::Authorizations(
                    pb::DataSourceAuthorizations {
                        authorizations: values.items().iter().map(authorization).collect(),
                        page: Some(core::PageResponse {
                            next_cursor: values
                                .next_cursor()
                                .map_or_else(String::new, |value| value.as_str().to_owned()),
                        }),
                    },
                ),
                Err(error) => pb::list_data_source_authorizations_response::Result::Error(
                    self.errors
                        .map(OPERATION, "data-source-authorization", &error),
                ),
            }),
        }))
    }
}

fn parse_cursor(
    codec: &AeadCursorCodec,
    scope: &ficant_application::AccessScope,
    value: &str,
) -> Result<Option<Cursor>, ApplicationError> {
    if value.is_empty() {
        Ok(None)
    } else {
        Cursor::resume(codec, scope, value.to_owned()).map(Some)
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

fn parse_authorization(
    value: Option<&pb::DataSourceAuthorization>,
) -> Result<DataSourceAuthorization, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let reference = parse_version_ref(value.r#ref.as_ref())?;
    let import_interface = parse_import_interface(value.interface)?;
    let state =
        match pb::DataSourceAuthorizationState::try_from(value.state).map_err(|_| invalid())? {
            pb::DataSourceAuthorizationState::Active => DataSourceAuthorizationState::Active,
            pb::DataSourceAuthorizationState::Revoked => DataSourceAuthorizationState::Revoked,
            pb::DataSourceAuthorizationState::Unspecified => return Err(invalid()),
        };
    let input = DataSourceAuthorizationInput {
        authorization_id: reference.id().clone(),
        version: reference.version(),
        owner: parse_owner(value.owner.as_ref())?,
        data_source: parse_version_ref(value.source.as_ref())?,
        data_source_hash: parse_hash(value.source_hash.as_ref())?,
        import_interface,
        canonical_schema_id: value.schema_id.clone(),
        canonical_schema_hash: parse_hash(value.schema_hash.as_ref())?,
        effective: EffectivePeriod::new(
            parse_market_time(value.effective_from.as_ref())?,
            parse_market_time(value.effective_to.as_ref())?,
        )
        .map_err(map_domain_error)?,
        state,
        supersedes: value
            .supersedes
            .as_ref()
            .map(|reference| parse_version_ref(Some(reference)))
            .transpose()?,
        mapping_id: parse_ulid(value.mapping_id.as_ref())?,
        mapping_hash: parse_hash(value.mapping_hash.as_ref())?,
    };
    DataSourceAuthorization::from_claimed_hash(input, parse_hash(value.content_hash.as_ref())?)
        .map_err(map_domain_error)
}

pub(crate) fn parse_mapping(
    value: Option<&pb::InstrumentMapping>,
) -> Result<InstrumentMapping, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let entries = value
        .entries
        .iter()
        .map(|entry| {
            InstrumentMappingEntry::new(
                entry.source_instrument_key.clone(),
                EffectivePeriod::new(
                    parse_market_time(entry.effective_from.as_ref())?,
                    parse_market_time(entry.effective_to.as_ref())?,
                )
                .map_err(map_domain_error)?,
                parse_version_ref(entry.instrument.as_ref())?,
            )
            .map_err(|_| invalid())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mapping = InstrumentMapping::new(
        parse_ulid(value.mapping_id.as_ref())?,
        parse_owner(value.owner.as_ref())?,
        parse_version_ref(value.source.as_ref())?,
        entries,
    )
    .map_err(|_| invalid())?;
    if mapping.content_hash() != &parse_hash(value.content_hash.as_ref())? {
        return Err(invalid());
    }
    Ok(mapping)
}

fn authorization(value: &DataSourceAuthorization) -> pb::DataSourceAuthorization {
    pb::DataSourceAuthorization {
        r#ref: Some(version_ref(&value.version_ref())),
        owner: Some(owner(value.owner())),
        source: Some(version_ref(value.data_source())),
        source_hash: Some(sha256(value.data_source_hash())),
        interface: match value.import_interface() {
            ImportInterface::CanonicalQuoteSnapshot => {
                pb::ImportInterface::CanonicalQuoteSnapshot as i32
            }
        },
        schema_id: value.canonical_schema_id().to_owned(),
        schema_hash: Some(sha256(value.canonical_schema_hash())),
        effective_from: Some(market_time(value.effective().from())),
        effective_to: Some(market_time(value.effective().to())),
        state: match value.state() {
            DataSourceAuthorizationState::Active => pb::DataSourceAuthorizationState::Active as i32,
            DataSourceAuthorizationState::Revoked => {
                pb::DataSourceAuthorizationState::Revoked as i32
            }
        },
        supersedes: value.supersedes().map(version_ref),
        content_hash: Some(sha256(value.content_hash())),
        mapping_id: Some(ulid(value.mapping_id())),
        mapping_hash: Some(sha256(value.mapping_hash())),
    }
}

fn parse_change(
    value: Option<&core::ChangeJustification>,
) -> Result<ChangeJustification, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let sources = value
        .sources
        .iter()
        .map(|source| {
            SourceDocumentRef::new(source.uri.clone(), parse_hash(source.sha256.as_ref())?)
                .map_err(map_domain_error)
        })
        .collect::<Result<Vec<_>, _>>()?;
    ChangeJustification::new(value.reason.clone(), sources).map_err(map_domain_error)
}

fn parse_market_time(value: Option<&core::MarketTime>) -> Result<MarketTime, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let timestamp = value.instant.as_ref().ok_or_else(invalid)?;
    let nanos = u32::try_from(timestamp.nanos).map_err(|_| invalid())?;
    let instant = DateTime::<Utc>::from_timestamp(timestamp.seconds, nanos).ok_or_else(invalid)?;
    let date = value.local_trading_date.parse().map_err(|_| invalid())?;
    MarketTime::new(instant, value.market_timezone.clone(), date).map_err(map_domain_error)
}

fn market_time(value: &MarketTime) -> core::MarketTime {
    core::MarketTime {
        instant: Some(prost_types::Timestamp {
            seconds: value.instant().timestamp(),
            nanos: i32::try_from(value.instant().timestamp_subsec_nanos())
                .expect("timestamp nanos fit i32"),
        }),
        market_timezone: value.market_timezone().to_owned(),
        local_trading_date: value.local_trading_date().to_string(),
    }
}

fn server_market_time() -> MarketTime {
    let instant = Utc::now();
    MarketTime::new(instant, "UTC", instant.date_naive())
        .expect("UTC system time is one valid MarketTime")
}

fn parse_import_interface(value: i32) -> Result<ImportInterface, ApplicationError> {
    match pb::ImportInterface::try_from(value).map_err(|_| invalid())? {
        pb::ImportInterface::CanonicalQuoteSnapshot => Ok(ImportInterface::CanonicalQuoteSnapshot),
        pb::ImportInterface::Unspecified => Err(invalid()),
    }
}

fn parse_optional_import_interface(
    value: i32,
) -> Result<Option<ImportInterface>, ApplicationError> {
    match pb::ImportInterface::try_from(value).map_err(|_| invalid())? {
        pb::ImportInterface::Unspecified => Ok(None),
        pb::ImportInterface::CanonicalQuoteSnapshot => {
            Ok(Some(ImportInterface::CanonicalQuoteSnapshot))
        }
    }
}

fn nonzero_version(value: u64) -> Result<Option<Version>, ApplicationError> {
    if value == 0 {
        Ok(None)
    } else {
        Ok(Some(Version::new(value).map_err(map_domain_error)?))
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

fn sha256(value: &ContentHash) -> core::Sha256 {
    core::Sha256 {
        value: value.as_bytes().to_vec(),
    }
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

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}
