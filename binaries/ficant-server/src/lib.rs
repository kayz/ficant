use ficant_api::{
    GrpcWebServeError, GrpcWebServerConfig, PlatformApplication, PlatformGrpcService,
    SessionPolicy, SystemClock, TrustedIdentity, serve_grpc_web,
};
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
const ENV_KEYS: &[&str] = &[
    "FICANT_GRPC_BIND",
    "FICANT_GRPC_WEB_ALLOWED_ORIGINS",
    "FICANT_PLATFORM_SIGNING_KEY_HEX",
    "FICANT_PLATFORM_TRACE_KEY_HEX",
    "FICANT_BOOTSTRAP_SUBJECT",
    "FICANT_BOOTSTRAP_BEARER_TOKEN",
    "FICANT_BOOTSTRAP_SCOPES",
    "FICANT_LOOPBACK_SUBJECT",
    "FICANT_LOOPBACK_SCOPES",
];

pub struct ServerSettings {
    bind: SocketAddr,
    allowed_origins: Vec<String>,
    signing_key: Vec<u8>,
    trace_key: Vec<u8>,
    bearer_identity: Option<TrustedIdentity>,
    implicit_identity: Option<TrustedIdentity>,
}

impl fmt::Debug for ServerSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServerSettings")
            .field("bind", &self.bind)
            .field("allowed_origins", &self.allowed_origins)
            .field("signing_key", &"[REDACTED]")
            .field("trace_key", &"[REDACTED]")
            .field("bearer_identity", &self.bearer_identity.is_some())
            .field("implicit_identity", &self.implicit_identity.is_some())
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
        let bearer_identity = bearer_identity(values)?;
        let implicit_identity = implicit_identity(values)?;
        if implicit_identity.is_some() && !bind.ip().is_loopback() {
            return Err(config(
                "implicit identity is allowed only on a loopback listener",
            ));
        }

        Ok(Self {
            bind,
            allowed_origins,
            signing_key,
            trace_key,
            bearer_identity,
            implicit_identity,
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
    PlatformGrpcService::new(Arc::new(application), &settings.trace_key).map_err(config)
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
    let service = build_platform_service(&settings)?;
    serve_grpc_web(
        GrpcWebServerConfig {
            bind: settings.bind,
            allowed_origins: settings.allowed_origins.clone(),
        },
        service,
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
        Some(subject) => {
            TrustedIdentity::implicit(subject, scopes(values.get("FICANT_LOOPBACK_SCOPES")))
                .map(Some)
                .map_err(config)
        }
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
