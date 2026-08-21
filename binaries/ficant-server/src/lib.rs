use ficant_api::{
    ArtifactGrpcService, CanonicalCurvePointSetDecoder, CanonicalSnapshotCodecAdapter,
    DataHealthGrpcService, DataSourceRegistryGrpcService, ExperimentGrpcService,
    FactorRegistryGrpcService, FormalOutputPublisher, FoundationChangeGrpcService,
    GrpcWebServeError, GrpcWebServerConfig, MarketDefinitionGrpcService, MarketFactGrpcService,
    OwnedPortfolioAggregationApplicationBackend, PlatformApplication, PlatformGrpcService,
    PlatformPort, PortfolioAggregationGrpcService, PortfolioCatalogGrpcService,
    PortfolioRiskGrpcService, PortfolioWorkbenchGrpcService, PositionSnapshotGrpcService,
    ProductionGrpcServices, RatesGrpcService, SessionPolicy, SnapshotGrpcService,
    SubjectRegistryGrpcService, SystemClock, SystemPortfolioRequestIdGenerator,
    TrustedExperimentScope, TrustedIdentity, TrustedNodeCatalog, build_production_routes,
    serve_production_routes,
};
use ficant_application::ports::{
    AccessScope, AeadCursorCodec, ArtifactRepository, BlobStore, CursorKey,
    CurveSnapshotMetadataRepository, DataHealthThresholdProfileRepository,
    DataSourceAuthorizationRepository, DataSourceRepository, DefinitionRepository,
    ExperimentRepository, FactorTopologyRepository, FormalOutputRepository,
    FoundationChangeRepository, IntegrityEventSink, MarketFactRepository,
    Phase4ExecutionRepository, PortfolioAnalyticsAuthorityRepository, PortfolioCatalogRepository,
    PositionSnapshotRepository, RunJournalRepository, SignalRepository, SnapshotRepository,
    SnapshotVerifiedReadMetadataRepository, SubjectRepository, VerifiedBlobReader,
};
use ficant_application::use_cases::portfolio_aggregation::{
    ExistingPositionViewsHandoff, OwnedExactPortfolioBondAnalysisHandoff,
    OwnedExistingPortfolioRiskHandoff, OwnedPortfolioAggregationBackend,
    OwnedPortfolioAnalyticsAuthorityHandoff, OwnedPortfolioCatalogAggregationAuthority,
    OwnedPortfolioOverviewRecordFactory, OwnedPortfolioRiskFuturesDependencies,
    OwnedRequiredPortfolioOverviewPublisher, PortfolioFormalExecutionBinding,
};
use ficant_application::use_cases::portfolio_workbench::{
    OwnedPortfolioCatalogBackend, OwnedPortfolioCatalogReadEvidenceFactory,
    OwnedPortfolioWorkbenchBackend, OwnedPortfolioWorkbenchCatalogEvidenceFactory,
    OwnedPortfolioWorkbenchContextResolver, OwnedPortfolioWorkbenchInstrumentHandoff,
    OwnedPortfolioWorkbenchPageSource,
};
use ficant_application::{ApplicationError, map_runtime_error};
use ficant_cgb_futures_pack::CgbFuturesDeliveryRulePackParser;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, Ulid};
use ficant_fixed_income_native::{
    NativeBondAnalyticsEngine, NativeCarryRollEngine, NativeFuturesDeliveryEngine,
    NativeFuturesHedgeEngine, NativeYieldCurveEngine,
};
use ficant_funding_pack::FundingRulePackV1Parser;
use ficant_native_nodes::{native_node_source_digest, trusted_native_node};
use ficant_runtime::{CodeBinding, NativeNode, RuntimeBinding};
use ficant_storage::postgres::PostgresRepository;
use ficant_storage::s3::S3BlobStore;
use ficant_storage::{
    analytics_arrow::ArrowBondAnalyticsCodec, futures_arrow::ArrowFuturesDeliveryCodec,
};
use ficant_tax_pack::TaxRulePackV2Parser;
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

mod integrity_event;

pub use integrity_event::{JsonLineIntegrityEventSink, build_integrity_event_sink};

const DEFAULT_BIND: &str = "127.0.0.1:50051";
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(1);
const SESSION_TTL_SECONDS: i64 = 15 * 60;
const APP_GRANT_TTL_SECONDS: i64 = 60;

/// Returns the Git commit embedded by the server build script.
#[must_use]
pub const fn compiled_git_commit_sha() -> &'static str {
    env!("FICANT_COMPILED_GIT_COMMIT_SHA")
}

/// Returns the Git tree embedded by the server build script.
#[must_use]
pub const fn compiled_git_tree_sha() -> &'static str {
    env!("FICANT_COMPILED_GIT_TREE_SHA")
}

const ENV_KEYS: &[&str] = &[
    "FICANT_GRPC_BIND",
    "FICANT_GRPC_WEB_ALLOWED_ORIGINS",
    "FICANT_PLATFORM_SIGNING_KEY_HEX",
    "FICANT_PLATFORM_TRACE_KEY_HEX",
    "FICANT_CODE_COMMIT_SHA",
    "FICANT_CODE_TREE_SHA",
    "FICANT_SERVER_RUNTIME_IMAGE_DIGEST",
    "FICANT_SERVER_ENVIRONMENT_ATTESTATION",
    "FICANT_BOOTSTRAP_SUBJECT",
    "FICANT_BOOTSTRAP_BEARER_TOKEN",
    "FICANT_BOOTSTRAP_ACTOR_ID",
    "FICANT_BOOTSTRAP_TENANT_ID",
    "FICANT_BOOTSTRAP_ALLOWED_OWNER_IDS",
    "FICANT_BOOTSTRAP_ACTIVE_ROLE",
    "FICANT_BOOTSTRAP_SCOPES",
    "FICANT_LOOPBACK_SUBJECT",
    "FICANT_LOOPBACK_ACTOR_ID",
    "FICANT_LOOPBACK_TENANT_ID",
    "FICANT_LOOPBACK_ALLOWED_OWNER_IDS",
    "FICANT_LOOPBACK_ACTIVE_ROLE",
    "FICANT_LOOPBACK_SCOPES",
    "FICANT_EXPERIMENT_DATABASE_URL",
    "FICANT_EXPERIMENT_S3_ENDPOINT",
    "FICANT_EXPERIMENT_S3_BUCKET",
    "FICANT_EXPERIMENT_S3_ACCESS_KEY",
    "FICANT_EXPERIMENT_S3_SECRET_KEY",
    "FICANT_EXPERIMENT_CURSOR_KEY_HEX",
    "FICANT_EXPERIMENT_TENANT_ID",
    "FICANT_EXPERIMENT_OWNER_ID",
    "FICANT_EXPERIMENT_ACTOR_ID",
    "FICANT_EXPERIMENT_RUNTIME_IMAGE_DIGEST",
    "FICANT_EXPERIMENT_ENVIRONMENT_ATTESTATION",
    "FICANT_EXPERIMENT_NATIVE_SOURCE_DIGEST",
    "FICANT_INPUT_FILE_NDJSON_ROOT",
    "FICANT_INPUT_FILE_CONNECTION_BINDING",
    "FICANT_INPUT_POSTGRES_CONNECTION_BINDING",
];

pub struct ServerSettings {
    bind: SocketAddr,
    allowed_origins: Vec<String>,
    signing_key: Vec<u8>,
    trace_key: Vec<u8>,
    code: CodeBinding,
    server_runtime: RuntimeBinding,
    bearer_identity: Option<TrustedIdentity>,
    implicit_identity: Option<TrustedIdentity>,
    experiment_database_url: String,
    experiment_s3_endpoint: String,
    experiment_s3_bucket: String,
    experiment_s3_access_key: String,
    experiment_s3_secret_key: String,
    experiment_cursor_key: [u8; 32],
    experiment_tenant_id: Ulid,
    experiment_owner_id: Ulid,
    experiment_actor_id: Ulid,
    experiment_runtime_image_digest: ContentHash,
    experiment_environment_attestation: String,
    experiment_native_source_digest: ContentHash,
    input_file_ndjson_root: String,
    input_file_connection_binding: String,
    input_postgres_connection_binding: String,
}

impl fmt::Debug for ServerSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServerSettings")
            .field("bind", &self.bind)
            .field("allowed_origins", &self.allowed_origins)
            .field("signing_key", &"[REDACTED]")
            .field("trace_key", &"[REDACTED]")
            .field("code", &self.code.digest())
            .field("server_runtime", &"[ATTESTED]")
            .field("bearer_identity", &self.bearer_identity.is_some())
            .field("implicit_identity", &self.implicit_identity.is_some())
            .field("experiment_database_url", &"[REDACTED]")
            .field("experiment_s3_endpoint", &"[REDACTED]")
            .field("experiment_s3_bucket", &"[REDACTED]")
            .field("experiment_s3_access_key", &"[REDACTED]")
            .field("experiment_s3_secret_key", &"[REDACTED]")
            .field("experiment_cursor_key", &"[REDACTED]")
            .field("experiment_tenant_id", &"[REDACTED]")
            .field("experiment_owner_id", &"[REDACTED]")
            .field("experiment_actor_id", &"[REDACTED]")
            .field("experiment_runtime_image_digest", &"[REDACTED]")
            .field("experiment_environment_attestation", &"[REDACTED]")
            .field("experiment_native_source_digest", &"[REDACTED]")
            .field("input_file_ndjson_root", &"[REDACTED]")
            .field("input_file_connection_binding", &"[REDACTED]")
            .field("input_postgres_connection_binding", &"[REDACTED]")
            .finish()
    }
}

impl ServerSettings {
    /// Parses server settings from an explicit environment-value map.
    ///
    /// # Errors
    ///
    /// Returns an error for missing keys, unsafe listener/identity combinations, or invalid keys.
    pub fn try_from_values(values: &BTreeMap<String, String>) -> Result<Self, ServerError> {
        let bind = values
            .get("FICANT_GRPC_BIND")
            .map_or(DEFAULT_BIND, String::as_str)
            .parse::<SocketAddr>()
            .map_err(|_| config("FICANT_GRPC_BIND must be a socket address"))?;
        let allowed_origins = required(values, "FICANT_GRPC_WEB_ALLOWED_ORIGINS")?
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if allowed_origins.is_empty() {
            return Err(config("at least one exact gRPC-Web origin is required"));
        }

        let signing_key = decode_key(required(values, "FICANT_PLATFORM_SIGNING_KEY_HEX")?)?;
        let trace_key = decode_key(required(values, "FICANT_PLATFORM_TRACE_KEY_HEX")?)?;
        let code = CodeBinding::new(
            required(values, "FICANT_CODE_COMMIT_SHA")?,
            required(values, "FICANT_CODE_TREE_SHA")?,
        )
        .map_err(|_| config("FICANT Code binding is invalid"))?;
        if code.git_commit_sha() != compiled_git_commit_sha()
            || code.git_tree_sha() != compiled_git_tree_sha()
        {
            return Err(config(
                "configured FICANT Code binding does not match the compiled binary",
            ));
        }
        let server_runtime = RuntimeBinding::new(
            parse_digest_setting(required(values, "FICANT_SERVER_RUNTIME_IMAGE_DIGEST")?)?,
            parse_digest_setting(required(values, "FICANT_SERVER_ENVIRONMENT_ATTESTATION")?)?,
        );
        let bearer_identity = bearer_identity(values)?;
        let implicit_identity = implicit_identity(values)?;
        if implicit_identity.is_some() && !bind.ip().is_loopback() {
            return Err(config(
                "implicit identity is allowed only on a loopback listener",
            ));
        }
        let experiment_cursor_key =
            decode_exact_key(required(values, "FICANT_EXPERIMENT_CURSOR_KEY_HEX")?)?;
        let experiment_tenant_id =
            parse_ulid_setting(required(values, "FICANT_EXPERIMENT_TENANT_ID")?)?;
        let experiment_owner_id =
            parse_ulid_setting(required(values, "FICANT_EXPERIMENT_OWNER_ID")?)?;
        let experiment_actor_id =
            parse_ulid_setting(required(values, "FICANT_EXPERIMENT_ACTOR_ID")?)?;
        let experiment_runtime_image_digest =
            parse_digest_setting(required(values, "FICANT_EXPERIMENT_RUNTIME_IMAGE_DIGEST")?)?;
        let experiment_native_source_digest =
            parse_digest_setting(required(values, "FICANT_EXPERIMENT_NATIVE_SOURCE_DIGEST")?)?;

        Ok(Self {
            bind,
            allowed_origins,
            signing_key,
            trace_key,
            code,
            server_runtime,
            bearer_identity,
            implicit_identity,
            experiment_database_url: required(values, "FICANT_EXPERIMENT_DATABASE_URL")?.to_owned(),
            experiment_s3_endpoint: required(values, "FICANT_EXPERIMENT_S3_ENDPOINT")?.to_owned(),
            experiment_s3_bucket: required(values, "FICANT_EXPERIMENT_S3_BUCKET")?.to_owned(),
            experiment_s3_access_key: required(values, "FICANT_EXPERIMENT_S3_ACCESS_KEY")?
                .to_owned(),
            experiment_s3_secret_key: required(values, "FICANT_EXPERIMENT_S3_SECRET_KEY")?
                .to_owned(),
            experiment_cursor_key,
            experiment_tenant_id,
            experiment_owner_id,
            experiment_actor_id,
            experiment_runtime_image_digest,
            experiment_environment_attestation: required(
                values,
                "FICANT_EXPERIMENT_ENVIRONMENT_ATTESTATION",
            )?
            .to_owned(),
            experiment_native_source_digest,
            input_file_ndjson_root: required(values, "FICANT_INPUT_FILE_NDJSON_ROOT")?.to_owned(),
            input_file_connection_binding: required(
                values,
                "FICANT_INPUT_FILE_CONNECTION_BINDING",
            )?
            .to_owned(),
            input_postgres_connection_binding: required(
                values,
                "FICANT_INPUT_POSTGRES_CONNECTION_BINDING",
            )?
            .to_owned(),
        })
    }
}

#[derive(Debug)]
pub enum ServerError {
    Configuration(String),
    HealthCheck {
        address: SocketAddr,
        source: std::io::Error,
    },
    Serve(GrpcWebServeError),
    Usage(&'static str),
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(message) => {
                write!(formatter, "invalid server configuration: {message}")
            }
            Self::HealthCheck { address, source } => {
                write!(formatter, "health check failed for {address}: {source}")
            }
            Self::Serve(error) => write!(formatter, "{error}"),
            Self::Usage(message) => write!(formatter, "{message}"),
        }
    }
}

impl Error for ServerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Configuration(_) | Self::Usage(_) => None,
            Self::HealthCheck { source, .. } => Some(source),
            Self::Serve(error) => Some(error),
        }
    }
}

/// Dispatches the supported process mode before reading mode-specific configuration.
///
/// # Errors
///
/// Returns a usage error for unknown arguments, a probe error for an unavailable endpoint,
/// or the existing server configuration/transport errors in normal service mode.
pub async fn entry_from_env() -> Result<(), ServerError> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    match arguments.as_slice() {
        [] => run_from_env().await,
        [argument] if argument == "--health-check" => health_check_from_env(),
        _ => Err(ServerError::Usage("usage: ficant-server [--health-check]")),
    }
}

impl From<GrpcWebServeError> for ServerError {
    fn from(error: GrpcWebServeError) -> Self {
        Self::Serve(error)
    }
}

/// Composes the transport service from validated settings and an empty production App Registry.
///
/// # Errors
///
/// Returns an error if validated settings cannot construct the platform application boundary.
pub fn build_platform_service(
    settings: &ServerSettings,
) -> Result<PlatformGrpcService, ServerError> {
    let application = build_platform_application(settings)?;
    PlatformGrpcService::new(application, &settings.trace_key).map_err(config)
}

/// Composes the platform and Phase 2 analytics services over one identity boundary.
///
/// # Errors
///
/// Returns an error if validated settings cannot construct either transport adapter.
pub fn build_grpc_services(
    settings: &ServerSettings,
) -> Result<(PlatformGrpcService, RatesGrpcService), ServerError> {
    let application = build_platform_application(settings)?;
    let platform =
        PlatformGrpcService::new(Arc::clone(&application), &settings.trace_key).map_err(config)?;
    let (repository, pool, _) = build_repository(settings)?;
    let blob_store = Arc::new(
        S3BlobStore::new(
            &settings.experiment_s3_endpoint,
            settings.experiment_s3_bucket.clone(),
            &settings.experiment_s3_access_key,
            &settings.experiment_s3_secret_key,
            pool,
        )
        .map_err(|_| config("experiment S3 configuration is invalid"))?,
    );
    let definitions: Arc<dyn DefinitionRepository> = repository.clone();
    let subjects: Arc<dyn SubjectRepository> = repository.clone();
    let data_sources: Arc<dyn DataSourceRepository> = repository.clone();
    let curve_snapshots: Arc<dyn CurveSnapshotMetadataRepository> = repository.clone();
    let factors: Arc<dyn FactorTopologyRepository> = repository.clone();
    let artifacts: Arc<dyn ArtifactRepository> = repository.clone();
    let formal_outputs = build_formal_output_publisher(repository.clone(), settings);
    let snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository> = repository;
    let blobs: Arc<dyn VerifiedBlobReader> = blob_store;
    let rates = build_rates_service(
        Arc::clone(&application),
        definitions,
        subjects,
        snapshots.clone(),
        blobs,
        build_integrity_event_sink(),
        data_sources,
        curve_snapshots,
        factors,
        artifacts,
        formal_outputs,
        settings,
    )?;
    Ok((platform, rates))
}

/// Composes the Platform, Rates, and PostgreSQL/S3-backed Experiment production services.
///
/// # Errors
///
/// Returns a redacted composition error when trusted configuration or adapters cannot be built.
pub fn build_grpc_services_with_experiment(
    settings: &ServerSettings,
) -> Result<(PlatformGrpcService, RatesGrpcService, ExperimentGrpcService), ServerError> {
    let (platform, rates, experiment, _) =
        build_grpc_services_with_experiment_and_registry(settings)?;
    Ok((platform, rates, experiment))
}

/// Composes all production services, including the PostgreSQL-backed Subject Registry.
///
/// # Errors
///
/// Returns a redacted composition error when trusted configuration or adapters cannot be built.
pub fn build_grpc_services_with_experiment_and_registry(
    settings: &ServerSettings,
) -> Result<
    (
        PlatformGrpcService,
        RatesGrpcService,
        ExperimentGrpcService,
        SubjectRegistryGrpcService,
    ),
    ServerError,
> {
    let application = build_platform_application(settings)?;
    let platform =
        PlatformGrpcService::new(Arc::clone(&application), &settings.trace_key).map_err(config)?;
    let (repository, pool, cursor) = build_repository(settings)?;
    let blob_store = Arc::new(
        S3BlobStore::new(
            &settings.experiment_s3_endpoint,
            settings.experiment_s3_bucket.clone(),
            &settings.experiment_s3_access_key,
            &settings.experiment_s3_secret_key,
            pool,
        )
        .map_err(|_| config("experiment S3 configuration is invalid"))?,
    );
    let trusted = TrustedExperimentScope::new(
        settings.experiment_tenant_id.clone(),
        settings.experiment_owner_id.clone(),
        settings.experiment_actor_id.clone(),
        settings.experiment_runtime_image_digest.clone(),
        settings.experiment_environment_attestation.clone(),
        settings.experiment_native_source_digest.clone(),
    )
    .map_err(|_| config("trusted experiment scope is invalid"))?;
    let phase4: Arc<dyn Phase4ExecutionRepository> = repository.clone();
    let experiments: Arc<dyn ExperimentRepository> = repository.clone();
    let journals: Arc<dyn RunJournalRepository> = repository.clone();
    let snapshot_repository: Arc<dyn SnapshotRepository> = repository.clone();
    let artifacts: Arc<dyn ArtifactRepository> = repository.clone();
    let data_sources: Arc<dyn DataSourceRepository> = repository.clone();
    let curve_snapshots: Arc<dyn CurveSnapshotMetadataRepository> = repository.clone();
    let factors: Arc<dyn FactorTopologyRepository> = repository.clone();
    let definitions: Arc<dyn DefinitionRepository> = repository.clone();
    let subjects: Arc<dyn SubjectRepository> = repository.clone();
    let formal_outputs = build_formal_output_publisher(repository.clone(), settings);
    let snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository> = repository;
    let blobs: Arc<dyn VerifiedBlobReader> = blob_store;
    let rates = build_rates_service(
        Arc::clone(&application),
        definitions.clone(),
        subjects.clone(),
        snapshots.clone(),
        blobs.clone(),
        build_integrity_event_sink(),
        data_sources,
        curve_snapshots,
        factors,
        artifacts.clone(),
        formal_outputs,
        settings,
    )?;
    let registry_identity = Arc::clone(&application);
    let experiment = ExperimentGrpcService::new(
        application,
        experiments,
        journals,
        snapshot_repository,
        cursor,
        phase4,
        artifacts,
        definitions.clone(),
        snapshots.clone(),
        blobs.clone(),
        build_integrity_event_sink(),
        Arc::new(ProductionNativeCatalog),
        trusted,
        &settings.trace_key,
    )
    .map_err(config)?
    .with_formal_execution(subjects.clone(), settings.code.clone());
    let registry =
        SubjectRegistryGrpcService::new(registry_identity, subjects, &settings.trace_key)
            .map_err(config)?;
    Ok((platform, rates, experiment, registry))
}

/// Composes all production services, including `PositionSnapshot`, Factor, and Portfolio Risk.
///
/// # Errors
///
/// Returns a redacted composition error when trusted configuration or adapters cannot be built.
#[allow(clippy::type_complexity, clippy::too_many_lines)]
pub fn build_grpc_services_with_experiment_registry_and_positions_and_factors_and_portfolio_risk(
    settings: &ServerSettings,
) -> Result<
    (
        PlatformGrpcService,
        RatesGrpcService,
        ExperimentGrpcService,
        SubjectRegistryGrpcService,
        PositionSnapshotGrpcService,
        FactorRegistryGrpcService,
        PortfolioRiskGrpcService,
        DataSourceRegistryGrpcService,
    ),
    ServerError,
> {
    let (
        platform,
        rates,
        experiment,
        registry,
        positions,
        factors,
        portfolio_risk,
        data_sources,
        _,
    ) = build_grpc_services_with_experiment_registry_and_positions_and_factors_and_portfolio_risk_and_data_health(settings)?;
    Ok((
        platform,
        rates,
        experiment,
        registry,
        positions,
        factors,
        portfolio_risk,
        data_sources,
    ))
}

/// Preserves the pre-R6A complete production surface for focused compatibility tests.
///
/// # Errors
///
/// Returns a redacted composition error when trusted configuration or adapters cannot be built.
#[allow(clippy::type_complexity)]
pub fn build_grpc_services_with_experiment_registry_and_positions_and_factors_and_portfolio_risk_and_data_health(
    settings: &ServerSettings,
) -> Result<
    (
        PlatformGrpcService,
        RatesGrpcService,
        ExperimentGrpcService,
        SubjectRegistryGrpcService,
        PositionSnapshotGrpcService,
        FactorRegistryGrpcService,
        PortfolioRiskGrpcService,
        DataSourceRegistryGrpcService,
        DataHealthGrpcService,
    ),
    ServerError,
> {
    let (
        platform,
        rates,
        experiment,
        registry,
        positions,
        factors,
        portfolio_risk,
        data_sources,
        data_health,
        _,
        _,
        _,
        _,
    ) = build_grpc_services_with_r6a_input_plane(settings)?;
    Ok((
        platform,
        rates,
        experiment,
        registry,
        positions,
        factors,
        portfolio_risk,
        data_sources,
        data_health,
    ))
}

/// Composes every public production service over one identity and persistence boundary.
///
/// # Errors
///
/// Returns a redacted composition error when trusted configuration or adapters cannot be built.
#[allow(clippy::too_many_lines)]
pub fn build_production_grpc_services(
    settings: &ServerSettings,
) -> Result<ProductionGrpcServices, ServerError> {
    let application = build_platform_application(settings)?;
    let platform =
        PlatformGrpcService::new(Arc::clone(&application), &settings.trace_key).map_err(config)?;
    let (repository, pool, cursor) = build_repository(settings)?;
    let blob_store = Arc::new(
        S3BlobStore::new(
            &settings.experiment_s3_endpoint,
            settings.experiment_s3_bucket.clone(),
            &settings.experiment_s3_access_key,
            &settings.experiment_s3_secret_key,
            pool.clone(),
        )
        .map_err(|_| config("experiment S3 configuration is invalid"))?,
    );
    let trusted = TrustedExperimentScope::new(
        settings.experiment_tenant_id.clone(),
        settings.experiment_owner_id.clone(),
        settings.experiment_actor_id.clone(),
        settings.experiment_runtime_image_digest.clone(),
        settings.experiment_environment_attestation.clone(),
        settings.experiment_native_source_digest.clone(),
    )
    .map_err(|_| config("trusted experiment scope is invalid"))?;
    let phase4: Arc<dyn Phase4ExecutionRepository> = repository.clone();
    let experiments: Arc<dyn ExperimentRepository> = repository.clone();
    let journals: Arc<dyn RunJournalRepository> = repository.clone();
    let snapshot_repository: Arc<dyn SnapshotRepository> = repository.clone();
    let position_repository: Arc<dyn PositionSnapshotRepository> = repository.clone();
    let factor_repository: Arc<dyn FactorTopologyRepository> = repository.clone();
    let portfolio_catalog_repository: Arc<dyn PortfolioCatalogRepository> = repository.clone();
    let portfolio_analytics_authority_repository: Arc<dyn PortfolioAnalyticsAuthorityRepository> =
        repository.clone();
    let data_source_repository: Arc<dyn DataSourceRepository> = repository.clone();
    let data_source_authorization_repository: Arc<dyn DataSourceAuthorizationRepository> =
        repository.clone();
    let data_health_profile_repository: Arc<dyn DataHealthThresholdProfileRepository> =
        repository.clone();
    let foundation_changes: Arc<dyn FoundationChangeRepository> = repository.clone();
    let market_facts: Arc<dyn MarketFactRepository> = repository.clone();
    let curve_repository: Arc<dyn CurveSnapshotMetadataRepository> = repository.clone();
    let artifacts: Arc<dyn ArtifactRepository> = repository.clone();
    let signals: Arc<dyn SignalRepository> = repository.clone();
    let formal_output_repository: Arc<dyn FormalOutputRepository> = repository.clone();
    let definitions: Arc<dyn DefinitionRepository> = repository.clone();
    let subjects: Arc<dyn SubjectRepository> = repository.clone();
    let snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository> = repository.clone();
    let blobs: Arc<dyn VerifiedBlobReader> = blob_store.clone();
    let writable_blobs: Arc<dyn BlobStore> = blob_store.clone();
    let formal_outputs = FormalOutputPublisher::new(
        formal_output_repository.clone(),
        settings.code.clone(),
        settings.server_runtime.clone(),
    );
    let rates = build_rates_service(
        Arc::clone(&application),
        definitions.clone(),
        subjects.clone(),
        snapshots.clone(),
        blobs.clone(),
        build_integrity_event_sink(),
        data_source_repository.clone(),
        curve_repository.clone(),
        factor_repository.clone(),
        artifacts.clone(),
        formal_outputs.clone(),
        settings,
    )?;
    let experiment = ExperimentGrpcService::new(
        Arc::clone(&application),
        experiments,
        journals,
        snapshot_repository.clone(),
        Arc::clone(&cursor),
        phase4,
        artifacts.clone(),
        definitions.clone(),
        snapshots.clone(),
        blobs.clone(),
        build_integrity_event_sink(),
        Arc::new(ProductionNativeCatalog),
        trusted,
        &settings.trace_key,
    )
    .map_err(config)?
    .with_formal_execution(subjects.clone(), settings.code.clone());
    let registry = SubjectRegistryGrpcService::new(
        Arc::clone(&application),
        subjects.clone(),
        &settings.trace_key,
    )
    .map_err(config)?;
    let access_scope = AccessScope::new(
        settings.experiment_tenant_id.clone(),
        settings.experiment_actor_id.clone(),
        vec![settings.experiment_owner_id.clone()],
    )
    .map_err(|_| config("trusted position access scope is invalid"))?;
    let positions = PositionSnapshotGrpcService::new(
        Arc::clone(&application),
        access_scope.clone(),
        position_repository.clone(),
        snapshot_repository.clone(),
        writable_blobs.clone(),
        &settings.trace_key,
    )
    .map_err(config)?
    .with_formal_outputs(subjects.clone(), formal_outputs.clone());
    let data_health = DataHealthGrpcService::new(
        Arc::clone(&application),
        access_scope.clone(),
        position_repository.clone(),
        snapshots.clone(),
        blobs.clone(),
        build_integrity_event_sink(),
        Arc::new(CanonicalSnapshotCodecAdapter),
        data_source_repository.clone(),
        data_health_profile_repository,
        snapshot_repository.clone(),
        writable_blobs.clone(),
        &settings.trace_key,
    )
    .map_err(config)?
    .with_formal_outputs(subjects.clone(), formal_outputs.clone());
    let portfolio_risk = PortfolioRiskGrpcService::new(
        Arc::clone(&application),
        access_scope.clone(),
        position_repository.clone(),
        curve_repository.clone(),
        definitions.clone(),
        data_source_repository.clone(),
        factor_repository.clone(),
        blobs.clone(),
        build_integrity_event_sink(),
        Arc::new(CanonicalCurvePointSetDecoder),
        Arc::new(NativeYieldCurveEngine),
        Arc::new(NativeBondAnalyticsEngine),
        snapshots.clone(),
        Arc::new(CanonicalSnapshotCodecAdapter),
        Arc::new(CgbFuturesDeliveryRulePackParser),
        Arc::new(NativeFuturesDeliveryEngine),
        &settings.trace_key,
    )
    .map_err(config)?
    .with_formal_outputs(subjects.clone(), formal_outputs);
    let data_sources = DataSourceRegistryGrpcService::new(
        Arc::clone(&application),
        data_source_repository.clone(),
        data_source_authorization_repository.clone(),
        Arc::clone(&cursor),
        &settings.trace_key,
    )
    .map_err(config)?;
    let market_definitions = MarketDefinitionGrpcService::new(
        Arc::clone(&application),
        definitions.clone(),
        Arc::clone(&cursor),
        &settings.trace_key,
    )
    .map_err(config)?;
    let market_fact = MarketFactGrpcService::new(
        Arc::clone(&application),
        market_facts.clone(),
        definitions.clone(),
        writable_blobs.clone(),
        curve_repository.clone(),
        blobs.clone(),
        build_integrity_event_sink(),
        Arc::clone(&cursor),
        &settings.trace_key,
    )
    .map_err(config)?;
    let snapshot = SnapshotGrpcService::new_production(
        Arc::clone(&application),
        data_source_authorization_repository,
        data_source_repository.clone(),
        definitions.clone(),
        settings.input_file_connection_binding.clone(),
        settings.input_file_ndjson_root.clone().into(),
        settings.input_postgres_connection_binding.clone(),
        &settings.experiment_database_url,
        snapshot_repository.clone(),
        writable_blobs.clone(),
        snapshots.clone(),
        blobs.clone(),
        build_integrity_event_sink(),
        &settings.trace_key,
    )
    .map_err(config)?;
    let foundation_change = FoundationChangeGrpcService::new(
        Arc::clone(&application),
        foundation_changes,
        Arc::clone(&cursor),
        &settings.trace_key,
    )
    .map_err(config)?;
    let factors = FactorRegistryGrpcService::new(
        Arc::clone(&application),
        AccessScope::new(
            settings.experiment_tenant_id.clone(),
            settings.experiment_actor_id.clone(),
            vec![settings.experiment_owner_id.clone()],
        )
        .map_err(|_| config("trusted factor access scope is invalid"))?,
        factor_repository.clone(),
        &settings.trace_key,
    )
    .map_err(config)?;
    let artifact = ArtifactGrpcService::new(
        Arc::clone(&application),
        artifacts.clone(),
        signals,
        snapshots.clone(),
        blobs.clone(),
        build_integrity_event_sink(),
        Arc::clone(&cursor),
        &settings.trace_key,
    )
    .map_err(config)?;

    let catalog_read_evidence = Arc::new(OwnedPortfolioCatalogReadEvidenceFactory::new(
        subjects.clone(),
    ));
    let portfolio_catalog_backend = Arc::new(OwnedPortfolioCatalogBackend::new(
        portfolio_catalog_repository.clone(),
        Arc::clone(&cursor),
        catalog_read_evidence,
    ));
    let portfolio_catalog = PortfolioCatalogGrpcService::new(
        Arc::clone(&application),
        portfolio_catalog_backend,
        &settings.trace_key,
    )
    .map_err(config)?;

    let aggregation_authority = Arc::new(OwnedPortfolioCatalogAggregationAuthority::new(
        portfolio_catalog_repository.clone(),
        Arc::clone(&cursor),
    ));
    let analytics_authority = Arc::new(OwnedPortfolioAnalyticsAuthorityHandoff::new(
        portfolio_analytics_authority_repository,
        definitions.clone(),
        curve_repository.clone(),
        snapshots.clone(),
        blobs.clone(),
        build_integrity_event_sink(),
    ));
    let futures_dependencies = OwnedPortfolioRiskFuturesDependencies::new(
        snapshots.clone(),
        Arc::new(CanonicalSnapshotCodecAdapter),
        data_source_repository.clone(),
        Arc::new(CgbFuturesDeliveryRulePackParser),
        Arc::new(NativeFuturesDeliveryEngine),
    );
    let portfolio_risk_handoff = Arc::new(OwnedExistingPortfolioRiskHandoff::new(
        position_repository.clone(),
        curve_repository.clone(),
        definitions.clone(),
        factor_repository,
        blobs.clone(),
        build_integrity_event_sink(),
        Arc::new(CanonicalCurvePointSetDecoder),
        Arc::new(NativeYieldCurveEngine),
        Arc::new(NativeBondAnalyticsEngine),
        Some(futures_dependencies),
    ));
    let portfolio_bond_analysis = Arc::new(OwnedExactPortfolioBondAnalysisHandoff::new(
        definitions.clone(),
        subjects.clone(),
        snapshots,
        blobs,
        build_integrity_event_sink(),
        Arc::new(TaxRulePackV2Parser),
        Arc::new(NativeBondAnalyticsEngine),
    ));
    let overview_factory = Arc::new(OwnedPortfolioOverviewRecordFactory::new(
        subjects.clone(),
        PortfolioFormalExecutionBinding::new(
            settings.code.clone(),
            settings.server_runtime.clone(),
        ),
    ));
    let overview_publisher = Arc::new(OwnedRequiredPortfolioOverviewPublisher::new(
        formal_output_repository,
        overview_factory,
    ));
    let portfolio_aggregation_backend = Arc::new(OwnedPortfolioAggregationBackend::new(
        aggregation_authority,
        analytics_authority,
        position_repository,
        Arc::new(ExistingPositionViewsHandoff),
        portfolio_risk_handoff,
        portfolio_bond_analysis,
        overview_publisher,
    ));
    let portfolio_aggregation = PortfolioAggregationGrpcService::new(
        Arc::clone(&application),
        Arc::new(OwnedPortfolioAggregationApplicationBackend::new(
            portfolio_catalog_repository.clone(),
            Arc::clone(&cursor),
            portfolio_aggregation_backend.clone(),
        )),
        &settings.trace_key,
    )
    .map_err(config)?;

    let workbench_contexts = Arc::new(OwnedPortfolioWorkbenchContextResolver::new(
        portfolio_catalog_repository.clone(),
        Arc::clone(&cursor),
    ));
    let workbench_instrument = Arc::new(OwnedPortfolioWorkbenchInstrumentHandoff::new(
        portfolio_aggregation_backend.clone(),
        definitions,
        market_facts,
    ));
    let workbench_pages = Arc::new(OwnedPortfolioWorkbenchPageSource::new(
        portfolio_catalog_repository,
        cursor,
        portfolio_aggregation_backend,
        Arc::new(OwnedPortfolioWorkbenchCatalogEvidenceFactory::new(subjects)),
        workbench_instrument,
    ));
    let portfolio_workbench = PortfolioWorkbenchGrpcService::new(
        Arc::clone(&application),
        Arc::new(OwnedPortfolioWorkbenchBackend::new(
            workbench_contexts,
            workbench_pages,
            Arc::new(SystemClock),
            Arc::new(SystemPortfolioRequestIdGenerator::default()),
        )),
        &settings.trace_key,
    )
    .map_err(config)?;
    Ok(ProductionGrpcServices {
        platform,
        rates,
        experiment,
        registry,
        positions,
        factors,
        portfolio_risk,
        data_sources,
        data_health,
        definitions: market_definitions,
        facts: market_fact,
        snapshots: snapshot,
        governance: foundation_change,
        artifacts: artifact,
        portfolio_catalog,
        portfolio_aggregation,
        portfolio_workbench,
    })
}

/// Preserves the R6A focused-construction surface without defining a production route set.
///
/// # Errors
///
/// Returns a redacted composition error when trusted configuration or adapters cannot be built.
#[allow(clippy::type_complexity)]
pub fn build_grpc_services_with_r6a_input_plane(
    settings: &ServerSettings,
) -> Result<
    (
        PlatformGrpcService,
        RatesGrpcService,
        ExperimentGrpcService,
        SubjectRegistryGrpcService,
        PositionSnapshotGrpcService,
        FactorRegistryGrpcService,
        PortfolioRiskGrpcService,
        DataSourceRegistryGrpcService,
        DataHealthGrpcService,
        MarketDefinitionGrpcService,
        MarketFactGrpcService,
        SnapshotGrpcService,
        FoundationChangeGrpcService,
    ),
    ServerError,
> {
    let services = build_production_grpc_services(settings)?;
    Ok((
        services.platform,
        services.rates,
        services.experiment,
        services.registry,
        services.positions,
        services.factors,
        services.portfolio_risk,
        services.data_sources,
        services.data_health,
        services.definitions,
        services.facts,
        services.snapshots,
        services.governance,
    ))
}

/// Preserves the R4c production composition surface for focused Factor tests.
///
/// # Errors
///
/// Returns a configuration or composition error when any production dependency cannot be built.
pub fn build_grpc_services_with_experiment_registry_and_positions_and_factors(
    settings: &ServerSettings,
) -> Result<
    (
        PlatformGrpcService,
        RatesGrpcService,
        ExperimentGrpcService,
        SubjectRegistryGrpcService,
        PositionSnapshotGrpcService,
        FactorRegistryGrpcService,
    ),
    ServerError,
> {
    let (platform, rates, experiment, registry, positions, factors, _, _) =
        build_grpc_services_with_experiment_registry_and_positions_and_factors_and_portfolio_risk(
            settings,
        )?;
    Ok((platform, rates, experiment, registry, positions, factors))
}

/// Preserves the pre-R4c production composition surface without the additive Factor service.
///
/// # Errors
///
/// Returns a configuration or composition error when any production service
/// dependency cannot be constructed.
pub fn build_grpc_services_with_experiment_registry_and_positions(
    settings: &ServerSettings,
) -> Result<
    (
        PlatformGrpcService,
        RatesGrpcService,
        ExperimentGrpcService,
        SubjectRegistryGrpcService,
        PositionSnapshotGrpcService,
    ),
    ServerError,
> {
    let (platform, rates, experiment, registry, positions, _) =
        build_grpc_services_with_experiment_registry_and_positions_and_factors(settings)?;
    Ok((platform, rates, experiment, registry, positions))
}

#[allow(clippy::too_many_arguments)]
fn build_rates_service(
    application: Arc<dyn PlatformPort>,
    definitions: Arc<dyn DefinitionRepository>,
    subjects: Arc<dyn SubjectRepository>,
    snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
    blobs: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
    data_sources: Arc<dyn DataSourceRepository>,
    curve_snapshots: Arc<dyn CurveSnapshotMetadataRepository>,
    factors: Arc<dyn FactorTopologyRepository>,
    artifacts: Arc<dyn ArtifactRepository>,
    formal_outputs: FormalOutputPublisher,
    settings: &ServerSettings,
) -> Result<RatesGrpcService, ServerError> {
    RatesGrpcService::new_with_formal_materialization(
        application,
        Arc::new(NativeBondAnalyticsEngine),
        Arc::new(NativeYieldCurveEngine),
        Arc::new(NativeCarryRollEngine),
        Arc::new(NativeFuturesDeliveryEngine),
        definitions,
        subjects,
        Arc::new(CgbFuturesDeliveryRulePackParser),
        snapshots,
        blobs,
        integrity_events,
        Arc::new(CanonicalSnapshotCodecAdapter),
        Arc::new(FundingRulePackV1Parser),
        Arc::new(TaxRulePackV2Parser),
        Arc::new(NativeFuturesHedgeEngine),
        data_sources,
        curve_snapshots,
        factors,
        artifacts,
        Arc::new(CanonicalCurvePointSetDecoder),
        Arc::new(ArrowBondAnalyticsCodec),
        Arc::new(ArrowFuturesDeliveryCodec),
        formal_outputs,
        &settings.trace_key,
    )
    .map_err(config)
}

fn build_formal_output_publisher(
    repository: Arc<PostgresRepository>,
    settings: &ServerSettings,
) -> FormalOutputPublisher {
    let repository: Arc<dyn FormalOutputRepository> = repository;
    FormalOutputPublisher::new(
        repository,
        settings.code.clone(),
        settings.server_runtime.clone(),
    )
}

fn build_repository(
    settings: &ServerSettings,
) -> Result<(Arc<PostgresRepository>, sqlx::PgPool, Arc<AeadCursorCodec>), ServerError> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect_lazy(&settings.experiment_database_url)
        .map_err(|_| config("FICANT_EXPERIMENT_DATABASE_URL is invalid"))?;
    let cursor = Arc::new(
        AeadCursorCodec::new(
            CursorKey::new("server-active", settings.experiment_cursor_key)
                .map_err(|_| config("experiment cursor key is invalid"))?,
            Vec::new(),
        )
        .map_err(|_| config("experiment cursor configuration is invalid"))?,
    );
    Ok((
        Arc::new(PostgresRepository::new(pool.clone(), Arc::clone(&cursor))),
        pool,
        cursor,
    ))
}

struct ProductionNativeCatalog;

impl TrustedNodeCatalog for ProductionNativeCatalog {
    fn native_source_digest(&self) -> ContentHash {
        native_node_source_digest()
    }

    fn implementation_digest(
        &self,
        node: &ficant_domain::research::ResearchNode,
    ) -> Result<ContentHash, ApplicationError> {
        trusted_native_node(node)
            .map(|native| native.implementation_digest().clone())
            .map_err(|error| map_runtime_error(&error))
    }
}

fn build_platform_application(
    settings: &ServerSettings,
) -> Result<Arc<dyn PlatformPort>, ServerError> {
    let identities = settings.bearer_identity.clone().into_iter().collect();
    let application = PlatformApplication::try_new(
        Arc::new(SystemClock),
        SessionPolicy::new(SESSION_TTL_SECONDS, APP_GRANT_TTL_SECONDS).map_err(config)?,
        &settings.signing_key,
        identities,
        settings.implicit_identity.clone(),
        Vec::new(),
    )
    .map_err(config)?;
    Ok(Arc::new(application))
}

/// Loads settings, composes the service, and serves until the process is stopped.
///
/// # Errors
///
/// Returns a configuration or gRPC-Web transport error.
pub async fn run_from_env() -> Result<(), ServerError> {
    let values = ENV_KEYS
        .iter()
        .filter_map(|key| env::var(key).ok().map(|value| ((*key).to_owned(), value)))
        .collect();
    let settings = ServerSettings::try_from_values(&values)?;
    let services = build_production_grpc_services(&settings)?;
    let routes = build_production_routes(services)?;
    serve_production_routes(
        GrpcWebServerConfig {
            bind: settings.bind,
            allowed_origins: settings.allowed_origins.clone(),
        },
        routes,
    )
    .await?;
    Ok(())
}

fn health_check_from_env() -> Result<(), ServerError> {
    let bind = env::var("FICANT_GRPC_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let address = bind
        .parse::<SocketAddr>()
        .map_err(|_| config("FICANT_GRPC_BIND must be a socket address"))?;
    let address = probe_address(address);
    TcpStream::connect_timeout(&address, HEALTH_CHECK_TIMEOUT)
        .map(|_| ())
        .map_err(|source| ServerError::HealthCheck { address, source })
}

fn probe_address(address: SocketAddr) -> SocketAddr {
    match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), address.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), address.port())
        }
        _ => address,
    }
}

fn bearer_identity(
    values: &BTreeMap<String, String>,
) -> Result<Option<TrustedIdentity>, ServerError> {
    let subject = values.get("FICANT_BOOTSTRAP_SUBJECT");
    let credential = values.get("FICANT_BOOTSTRAP_BEARER_TOKEN");
    match (subject, credential) {
        (None, None) => Ok(None),
        (Some(subject), Some(credential)) => TrustedIdentity::bearer(
            subject,
            credential.as_bytes(),
            parse_ulid_setting(required(values, "FICANT_BOOTSTRAP_ACTOR_ID")?)?,
            parse_ulid_setting(required(values, "FICANT_BOOTSTRAP_TENANT_ID")?)?,
            allowed_owner_ids(required(values, "FICANT_BOOTSTRAP_ALLOWED_OWNER_IDS")?)?,
            platform_role(required(values, "FICANT_BOOTSTRAP_ACTIVE_ROLE")?)?,
            scopes(values.get("FICANT_BOOTSTRAP_SCOPES")),
        )
        .map(Some)
        .map_err(config),
        _ => Err(config(
            "bootstrap subject and bearer token must be configured together",
        )),
    }
}

fn implicit_identity(
    values: &BTreeMap<String, String>,
) -> Result<Option<TrustedIdentity>, ServerError> {
    match values.get("FICANT_LOOPBACK_SUBJECT") {
        Some(subject) => TrustedIdentity::implicit(
            subject,
            parse_ulid_setting(required(values, "FICANT_LOOPBACK_ACTOR_ID")?)?,
            parse_ulid_setting(required(values, "FICANT_LOOPBACK_TENANT_ID")?)?,
            allowed_owner_ids(required(values, "FICANT_LOOPBACK_ALLOWED_OWNER_IDS")?)?,
            platform_role(required(values, "FICANT_LOOPBACK_ACTIVE_ROLE")?)?,
            scopes(values.get("FICANT_LOOPBACK_SCOPES")),
        )
        .map(Some)
        .map_err(config),
        None if values.contains_key("FICANT_LOOPBACK_SCOPES") => Err(config(
            "loopback scopes require an explicit loopback subject",
        )),
        None => Ok(None),
    }
}

fn scopes(value: Option<&String>) -> Vec<String> {
    value
        .map_or("", String::as_str)
        .split(',')
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(str::to_owned)
        .collect()
}

fn allowed_owner_ids(value: &str) -> Result<Vec<Ulid>, ServerError> {
    let owners = value
        .split(',')
        .map(str::trim)
        .filter(|owner| !owner.is_empty())
        .map(parse_ulid_setting)
        .collect::<Result<Vec<_>, _>>()?;
    if owners.is_empty() {
        return Err(config("trusted identity must allow at least one owner"));
    }
    Ok(owners)
}

fn platform_role(value: &str) -> Result<PlatformRole, ServerError> {
    match value {
        "PLATFORM_ADMIN" => Ok(PlatformRole::PlatformAdmin),
        "RESEARCHER" => Ok(PlatformRole::Researcher),
        _ => Err(config(
            "trusted identity active role must be PLATFORM_ADMIN or RESEARCHER",
        )),
    }
}

fn required<'a>(
    values: &'a BTreeMap<String, String>,
    key: &'static str,
) -> Result<&'a str, ServerError> {
    values
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| config(format!("{key} is required")))
}

fn decode_key(value: &str) -> Result<Vec<u8>, ServerError> {
    if value.len() < 64 || !value.len().is_multiple_of(2) {
        return Err(config("security keys must contain at least 32 hex bytes"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn decode_exact_key(value: &str) -> Result<[u8; 32], ServerError> {
    let key = decode_key(value)?;
    key.try_into()
        .map_err(|_| config("cursor key must contain exactly 32 hex bytes"))
}

fn parse_ulid_setting(value: &str) -> Result<Ulid, ServerError> {
    Ulid::new(value).map_err(|_| config("experiment identity must be a canonical ULID"))
}

fn parse_digest_setting(value: &str) -> Result<ContentHash, ServerError> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or_else(|| config("experiment digest must use sha256:<lowercase-hex>"))?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(config("experiment digest must use sha256:<lowercase-hex>"));
    }
    let bytes = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Ok((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
        .collect::<Result<Vec<_>, ServerError>>()?;
    ContentHash::from_bytes(&bytes)
        .map_err(|_| config("experiment digest must contain exactly 32 bytes"))
}

fn hex_nibble(value: u8) -> Result<u8, ServerError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(config("security keys must be hexadecimal")),
    }
}

fn config(message: impl Into<String>) -> ServerError {
    ServerError::Configuration(message.into())
}
