use crate::error::{PlatformFailure, PlatformFailureCode, SafeErrorMapper};
use crate::registry::{
    AppGrantView, AppRegistration, PlatformPort, RequestCredential, SessionView,
};
use ficant_contracts::ficant::app::v1::{
    AppDescriptor, AppLaunchAuthorizationResponse, AppLaunchGrant, AppLaunchRevocation,
    AppRegistry, AuthorizeAppLaunchRequest, CspDirective, GetAppRegistryRequest,
    GetAppRegistryResponse, GetCurrentSessionRequest, GetCurrentSessionResponse,
    RefreshAppLaunchRequest, RefreshSessionRequest, RefreshSessionResponse, RevokeAppLaunchRequest,
    RevokeAppLaunchResponse, RevokeSessionRequest, RevokeSessionResponse, Session,
    SessionRevocation, app_launch_authorization_response, get_app_registry_response,
    get_current_session_response, platform_service_server::PlatformService,
    refresh_session_response, revoke_app_launch_response, revoke_session_response,
};
use prost_types::Timestamp;
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tonic::body::Body;
use tonic::codegen::Service;
use tonic::codegen::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE, ACCESS_CONTROL_REQUEST_METHOD, ORIGIN,
    VARY,
};
use tonic::codegen::http::{
    HeaderValue, Method, Request as HttpRequest, Response as HttpResponse, StatusCode, Uri,
};
use tonic::metadata::MetadataMap;
use tonic::server::NamedService;
use tonic::service::{Routes, RoutesBuilder};
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use tonic_web::GrpcWebLayer;
use tower_layer::Layer;

#[derive(Clone)]
pub struct PlatformGrpcService {
    application: Arc<dyn PlatformPort>,
    errors: SafeErrorMapper,
}

impl PlatformGrpcService {
    /// Creates the frozen platform transport adapter.
    ///
    /// # Errors
    ///
    /// Returns an error when the trace-signing key is too short.
    pub fn new(application: Arc<dyn PlatformPort>, trace_key: &[u8]) -> Result<Self, &'static str> {
        Ok(Self {
            application,
            errors: SafeErrorMapper::new(trace_key)?,
        })
    }

    fn error(
        &self,
        operation: &'static str,
        failure: &PlatformFailure,
    ) -> ficant_contracts::ficant::app::v1::SafeError {
        self.errors.map(operation, failure)
    }
}

#[tonic::async_trait]
impl PlatformService for PlatformGrpcService {
    async fn get_app_registry(
        &self,
        request: Request<GetAppRegistryRequest>,
    ) -> Result<Response<GetAppRegistryResponse>, Status> {
        let credential = request_credential(request.metadata());
        let result = match self.application.registry(&credential) {
            Ok(apps) => get_app_registry_response::Result::Registry(AppRegistry {
                apps: apps.into_iter().map(app_descriptor).collect(),
            }),
            Err(failure) => {
                get_app_registry_response::Result::Error(self.error("registry", &failure))
            }
        };
        Ok(Response::new(GetAppRegistryResponse {
            result: Some(result),
        }))
    }

    async fn get_current_session(
        &self,
        request: Request<GetCurrentSessionRequest>,
    ) -> Result<Response<GetCurrentSessionResponse>, Status> {
        let credential = request_credential(request.metadata());
        let result = match self.application.current_session(&credential) {
            Ok(session) => get_current_session_response::Result::Session(session_message(session)),
            Err(failure) => {
                get_current_session_response::Result::Error(self.error("current-session", &failure))
            }
        };
        Ok(Response::new(GetCurrentSessionResponse {
            result: Some(result),
        }))
    }

    async fn refresh_session(
        &self,
        request: Request<RefreshSessionRequest>,
    ) -> Result<Response<RefreshSessionResponse>, Status> {
        let credential = request_credential(request.metadata());
        let result = match self.application.refresh_session(&credential) {
            Ok(session) => refresh_session_response::Result::Session(session_message(session)),
            Err(failure) => {
                refresh_session_response::Result::Error(self.error("refresh-session", &failure))
            }
        };
        Ok(Response::new(RefreshSessionResponse {
            result: Some(result),
        }))
    }

    async fn revoke_session(
        &self,
        request: Request<RevokeSessionRequest>,
    ) -> Result<Response<RevokeSessionResponse>, Status> {
        let credential = request_credential(request.metadata());
        let result = match self.application.revoke_session(&credential) {
            Ok(revoked_at) => revoke_session_response::Result::Revocation(SessionRevocation {
                revoked_at: Some(timestamp(revoked_at)),
            }),
            Err(failure) => {
                revoke_session_response::Result::Error(self.error("revoke-session", &failure))
            }
        };
        Ok(Response::new(RevokeSessionResponse {
            result: Some(result),
        }))
    }

    async fn authorize_app_launch(
        &self,
        request: Request<AuthorizeAppLaunchRequest>,
    ) -> Result<Response<AppLaunchAuthorizationResponse>, Status> {
        Ok(app_authorization(
            self,
            request.metadata(),
            &request.get_ref().app_id,
            false,
        ))
    }

    async fn refresh_app_launch(
        &self,
        request: Request<RefreshAppLaunchRequest>,
    ) -> Result<Response<AppLaunchAuthorizationResponse>, Status> {
        Ok(app_authorization(
            self,
            request.metadata(),
            &request.get_ref().app_id,
            true,
        ))
    }

    async fn revoke_app_launch(
        &self,
        request: Request<RevokeAppLaunchRequest>,
    ) -> Result<Response<RevokeAppLaunchResponse>, Status> {
        let credential = request_credential(request.metadata());
        let app_id = request.get_ref().app_id.clone();
        let result = match self.application.revoke_app(&credential, &app_id) {
            Ok(revoked_at) => revoke_app_launch_response::Result::Revocation(AppLaunchRevocation {
                app_id,
                revoked_at: Some(timestamp(revoked_at)),
            }),
            Err(failure) => {
                revoke_app_launch_response::Result::Error(self.error("revoke-app", &failure))
            }
        };
        Ok(Response::new(RevokeAppLaunchResponse {
            result: Some(result),
        }))
    }
}

fn app_authorization(
    service: &PlatformGrpcService,
    metadata: &MetadataMap,
    app_id: &str,
    refresh: bool,
) -> Response<AppLaunchAuthorizationResponse> {
    let credential = request_credential(metadata);
    let grant = if refresh {
        service.application.refresh_app(&credential, app_id)
    } else {
        service.application.authorize_app(&credential, app_id)
    };
    let result = match grant {
        Ok(grant) => app_launch_authorization_response::Result::Grant(grant_message(grant)),
        Err(failure) => app_launch_authorization_response::Result::Error(service.error(
            if refresh {
                "refresh-app"
            } else {
                "authorize-app"
            },
            &failure,
        )),
    };
    Response::new(AppLaunchAuthorizationResponse {
        result: Some(result),
    })
}

pub(crate) fn request_credential(metadata: &MetadataMap) -> RequestCredential {
    let Some(value) = metadata.get("authorization") else {
        return RequestCredential::Implicit;
    };
    let Ok(value) = value.to_str() else {
        return RequestCredential::Invalid;
    };
    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .map_or(RequestCredential::Invalid, |token| {
            RequestCredential::Bearer(token.as_bytes().to_vec())
        })
}

fn app_descriptor(app: AppRegistration) -> AppDescriptor {
    AppDescriptor {
        app_id: app.app_id,
        display_name: app.display_name,
        entrypoint: app.entrypoint,
        allowed_origin: app.allowed_origin,
        capabilities: app.capabilities,
    }
}

fn session_message(session: SessionView) -> Session {
    Session {
        session_id: session.session_id,
        subject_id: session.subject_id,
        scopes: session.scopes,
        issued_at: Some(timestamp(session.issued_at)),
        expires_at: Some(timestamp(session.expires_at)),
        actor_id: Some(core_ulid(&session.actor_id)),
        active_role: match session.active_role {
            ficant_domain::governance::PlatformRole::PlatformAdmin => {
                ficant_contracts::ficant::core::v1::PlatformRole::PlatformAdmin as i32
            }
            ficant_domain::governance::PlatformRole::Researcher => {
                ficant_contracts::ficant::core::v1::PlatformRole::Researcher as i32
            }
        },
        tenant_id: Some(core_ulid(&session.tenant_id)),
        allowed_owner_ids: session.allowed_owner_ids.iter().map(core_ulid).collect(),
    }
}

fn core_ulid(value: &ficant_domain::primitives::Ulid) -> ficant_contracts::ficant::core::v1::Ulid {
    ficant_contracts::ficant::core::v1::Ulid {
        value: value.as_str().to_owned(),
    }
}

fn grant_message(grant: AppGrantView) -> AppLaunchGrant {
    AppLaunchGrant {
        app_id: grant.app.app_id,
        entrypoint: grant.app.entrypoint,
        allowed_origin: grant.app.allowed_origin,
        scopes: grant.scopes,
        issued_at: Some(timestamp(grant.issued_at)),
        expires_at: Some(timestamp(grant.expires_at)),
        launch_credential: grant.launch_credential,
        csp_directives: grant
            .app
            .csp
            .into_iter()
            .map(|policy| CspDirective {
                name: policy.name,
                values: policy.values,
            })
            .collect(),
        sandbox_tokens: grant.app.sandbox_tokens,
    }
}

const fn timestamp(seconds: i64) -> Timestamp {
    Timestamp { seconds, nanos: 0 }
}

#[derive(Clone, Debug)]
pub struct GrpcWebServerConfig {
    pub bind: SocketAddr,
    pub allowed_origins: Vec<String>,
}

#[derive(Debug)]
pub enum GrpcWebServeError {
    InvalidOrigin(String),
    DuplicateService(String),
    Transport(tonic::transport::Error),
}

impl fmt::Display for GrpcWebServeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOrigin(origin) => write!(formatter, "invalid exact CORS origin: {origin}"),
            Self::DuplicateService(service) => {
                write!(formatter, "duplicate production gRPC service: {service}")
            }
            Self::Transport(error) => write!(formatter, "gRPC-Web server failure: {error}"),
        }
    }
}

impl Error for GrpcWebServeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidOrigin(_) | Self::DuplicateService(_) => None,
            Self::Transport(error) => Some(error),
        }
    }
}

impl From<tonic::transport::Error> for GrpcWebServeError {
    fn from(error: tonic::transport::Error) -> Self {
        Self::Transport(error)
    }
}

/// The complete set of concrete adapters accepted by the production route builder.
pub struct ProductionGrpcServices {
    pub platform: PlatformGrpcService,
    pub rates: crate::rates::RatesGrpcService,
    pub experiment: crate::experiment::ExperimentGrpcService,
    pub registry: crate::subject_registry::SubjectRegistryGrpcService,
    pub positions: crate::position_snapshot::PositionSnapshotGrpcService,
    pub factors: crate::factor_registry::FactorRegistryGrpcService,
    pub portfolio_risk: crate::portfolio_risk::PortfolioRiskGrpcService,
    pub data_sources: crate::data_source_registry::DataSourceRegistryGrpcService,
    pub data_health: crate::data_health::DataHealthGrpcService,
    pub definitions: crate::market_definition::MarketDefinitionGrpcService,
    pub facts: crate::market_fact::MarketFactGrpcService,
    pub snapshots: crate::snapshot::SnapshotGrpcService,
    pub governance: crate::governance::FoundationChangeGrpcService,
    pub artifacts: crate::artifact::ArtifactGrpcService,
}

/// Tonic routes and the exact `NamedService` inventory produced by the same registrations.
pub struct ProductionRouteSet {
    routes: Routes,
    service_names: BTreeSet<String>,
}

impl ProductionRouteSet {
    #[must_use]
    pub fn service_names(&self) -> &BTreeSet<String> {
        &self.service_names
    }

    #[must_use]
    pub fn into_routes(self) -> Routes {
        self.routes
    }
}

/// Registers every production adapter once and records each actual `NamedService::NAME`.
///
/// # Errors
///
/// Returns an error if two registered server adapters claim the same service name.
pub fn build_production_routes(
    services: ProductionGrpcServices,
) -> Result<ProductionRouteSet, GrpcWebServeError> {
    use ficant_contracts::ficant::app::v1::platform_service_server::PlatformServiceServer;
    use ficant_contracts::ficant::core::v1::foundation_change_service_server::FoundationChangeServiceServer;
    use ficant_contracts::ficant::core::v1::registry_service_server::RegistryServiceServer;
    use ficant_contracts::ficant::market::v1::data_source_registry_service_server::DataSourceRegistryServiceServer;
    use ficant_contracts::ficant::market::v1::market_definition_service_server::MarketDefinitionServiceServer;
    use ficant_contracts::ficant::market::v1::market_fact_service_server::MarketFactServiceServer;
    use ficant_contracts::ficant::rates::v1::rates_analytics_service_server::RatesAnalyticsServiceServer;
    use ficant_contracts::ficant::research::v1::artifact_service_server::ArtifactServiceServer;
    use ficant_contracts::ficant::research::v1::data_health_service_server::DataHealthServiceServer;
    use ficant_contracts::ficant::research::v1::experiment_service_server::ExperimentServiceServer;
    use ficant_contracts::ficant::research::v1::factor_registry_service_server::FactorRegistryServiceServer;
    use ficant_contracts::ficant::research::v1::portfolio_risk_service_server::PortfolioRiskServiceServer;
    use ficant_contracts::ficant::research::v1::position_snapshot_service_server::PositionSnapshotServiceServer;
    use ficant_contracts::ficant::research::v1::snapshot_service_server::SnapshotServiceServer;

    let mut builder = RoutesBuilder::default();
    let mut service_names = BTreeSet::new();
    macro_rules! register {
        ($service:expr) => {{
            let service = $service;
            let name = named_service_name(&service);
            if !service_names.insert(name.to_owned()) {
                return Err(GrpcWebServeError::DuplicateService(name.to_owned()));
            }
            builder.add_service(service);
        }};
    }

    register!(PlatformServiceServer::new(services.platform));
    register!(RatesAnalyticsServiceServer::new(services.rates));
    register!(ExperimentServiceServer::new(services.experiment));
    register!(RegistryServiceServer::new(services.registry));
    register!(PositionSnapshotServiceServer::new(services.positions));
    register!(FactorRegistryServiceServer::new(services.factors));
    register!(PortfolioRiskServiceServer::new(services.portfolio_risk));
    register!(DataSourceRegistryServiceServer::new(services.data_sources));
    register!(DataHealthServiceServer::new(services.data_health));
    register!(MarketDefinitionServiceServer::new(services.definitions));
    register!(MarketFactServiceServer::new(services.facts));
    register!(SnapshotServiceServer::new(services.snapshots));
    register!(FoundationChangeServiceServer::new(services.governance));
    register!(ArtifactServiceServer::new(services.artifacts));

    Ok(ProductionRouteSet {
        routes: builder.routes(),
        service_names,
    })
}

fn named_service_name<S: NamedService>(_: &S) -> &'static str {
    S::NAME
}

/// Serves an explicitly constructed focused route set over native gRPC and gRPC-Web.
///
/// Production callers must use [`build_production_routes`] to create this value.
///
/// # Errors
///
/// Returns an error for an invalid exact CORS origin or a transport failure.
pub async fn serve_grpc_web_routes(
    config: GrpcWebServerConfig,
    routes: Routes,
) -> Result<(), GrpcWebServeError> {
    let cors = ExactCorsLayer::try_new(&config.allowed_origins)?;
    let service = GrpcWebLayer::new().layer(routes);
    let service = cors.layer(service);
    Server::builder()
        .accept_http1(true)
        .serve(config.bind, service)
        .await?;
    Ok(())
}

/// Serves the one complete production route set over native gRPC and gRPC-Web.
///
/// # Errors
///
/// Returns an error for an invalid exact CORS origin or a transport failure.
pub async fn serve_production_routes(
    config: GrpcWebServerConfig,
    routes: ProductionRouteSet,
) -> Result<(), GrpcWebServeError> {
    serve_grpc_web_routes(config, routes.into_routes()).await
}

#[derive(Clone, Debug)]
struct ExactCorsLayer {
    allowed_origins: Arc<Vec<HeaderValue>>,
}

impl ExactCorsLayer {
    fn try_new(origins: &[String]) -> Result<Self, GrpcWebServeError> {
        if origins.is_empty() {
            return Err(GrpcWebServeError::InvalidOrigin(
                "at least one exact origin is required".to_owned(),
            ));
        }
        let allowed_origins = origins
            .iter()
            .map(|origin| {
                validate_origin(origin)?;
                HeaderValue::from_str(origin)
                    .map_err(|_| GrpcWebServeError::InvalidOrigin(origin.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            allowed_origins: Arc::new(allowed_origins),
        })
    }
}

impl<S> Layer<S> for ExactCorsLayer {
    type Service = ExactCorsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ExactCorsService {
            inner,
            allowed_origins: Arc::clone(&self.allowed_origins),
        }
    }
}

#[derive(Clone, Debug)]
struct ExactCorsService<S> {
    inner: S,
    allowed_origins: Arc<Vec<HeaderValue>>,
}

impl<S, RequestBody> Service<HttpRequest<RequestBody>> for ExactCorsService<S>
where
    S: Service<HttpRequest<RequestBody>, Response = HttpResponse<Body>> + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    RequestBody: Send + 'static,
{
    type Response = HttpResponse<Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: HttpRequest<RequestBody>) -> Self::Future {
        let origin = request.headers().get(ORIGIN).cloned();
        let allowed = origin
            .as_ref()
            .is_some_and(|origin| self.allowed_origins.contains(origin));
        let preflight = request.method() == Method::OPTIONS
            && request
                .headers()
                .get(ACCESS_CONTROL_REQUEST_METHOD)
                .is_some_and(|method| method == HeaderValue::from_static("POST"));

        if preflight {
            let response = if allowed {
                cors_preflight(origin.expect("allowed origin is present"))
            } else {
                immediate(StatusCode::FORBIDDEN)
            };
            return Box::pin(async move { Ok(response) });
        }
        if origin.is_some() && !allowed {
            return Box::pin(async move { Ok(immediate(StatusCode::FORBIDDEN)) });
        }

        let future = self.inner.call(request);
        Box::pin(async move {
            let mut response = future.await?;
            if let Some(origin) = origin {
                apply_simple_cors(response.headers_mut(), origin);
            }
            Ok(response)
        })
    }
}

fn cors_preflight(origin: HeaderValue) -> HttpResponse<Body> {
    let mut response = immediate(StatusCode::NO_CONTENT);
    let headers = response.headers_mut();
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST"),
    );
    headers.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type,x-grpc-web,x-user-agent,grpc-timeout,authorization"),
    );
    headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
    headers.insert(
        VARY,
        HeaderValue::from_static(
            "origin,access-control-request-method,access-control-request-headers",
        ),
    );
    response
}

fn apply_simple_cors(headers: &mut tonic::codegen::http::HeaderMap, origin: HeaderValue) {
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    headers.insert(
        ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("grpc-status,grpc-message,grpc-status-details-bin"),
    );
    headers.insert(VARY, HeaderValue::from_static("origin"));
}

fn immediate(status: StatusCode) -> HttpResponse<Body> {
    let mut response = HttpResponse::new(Body::empty());
    *response.status_mut() = status;
    response
}

fn validate_origin(origin: &str) -> Result<(), GrpcWebServeError> {
    let uri: Uri = origin
        .parse()
        .map_err(|_| GrpcWebServeError::InvalidOrigin(origin.to_owned()))?;
    let Some(scheme) = uri.scheme_str() else {
        return Err(GrpcWebServeError::InvalidOrigin(origin.to_owned()));
    };
    let Some(host) = uri.host() else {
        return Err(GrpcWebServeError::InvalidOrigin(origin.to_owned()));
    };
    let secure = scheme == "https";
    let loopback_http = scheme == "http" && matches!(host, "127.0.0.1" | "localhost" | "::1");
    if (!secure && !loopback_http)
        || uri.path() != "/"
        || uri.query().is_some()
        || origin.ends_with('/')
    {
        return Err(GrpcWebServeError::InvalidOrigin(origin.to_owned()));
    }
    Ok(())
}

#[allow(dead_code)]
fn invalid_request() -> PlatformFailure {
    PlatformFailure::new(
        PlatformFailureCode::InvalidRequest,
        false,
        "invalid-request",
    )
}
