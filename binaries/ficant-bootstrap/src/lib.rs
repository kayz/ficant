use serde::Deserialize;
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CONFIG_ENV: &str = "FICANT_CONFIG";
const DEFAULT_CONFIG_PATH: &str = "deploy/dev/config/ficant.toml";
const MAX_REQUEST_BYTES: usize = 8 * 1024;
const MAX_DISCARD_BYTES: usize = 64 * 1024;
const MAX_PROBE_RESPONSE_BYTES: u64 = 4 * 1024;
const REQUEST_DEADLINE: Duration = Duration::from_secs(2);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_ROUTE_BYTES: usize = 128;

#[derive(Clone, Copy, Debug)]
pub enum ServiceRole {
    Server,
    Worker,
}

impl ServiceRole {
    const fn name(self) -> &'static str {
        match self {
            Self::Server => "ficant-server",
            Self::Worker => "ficant-worker",
        }
    }
}

#[derive(Debug)]
pub enum BootstrapError {
    ConfigRead { path: PathBuf, source: io::Error },
    ConfigParse(toml::de::Error),
    InvalidConfig(String),
    Io(io::Error),
    Usage(String),
}

impl fmt::Display for BootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigRead { path, source } => {
                write!(
                    formatter,
                    "failed to read config {}: {source}",
                    path.display()
                )
            }
            Self::ConfigParse(source) => write!(formatter, "failed to parse config: {source}"),
            Self::InvalidConfig(message) => write!(formatter, "invalid config: {message}"),
            Self::Io(source) => write!(formatter, "I/O failure: {source}"),
            Self::Usage(message) => write!(formatter, "{message}"),
        }
    }
}

impl Error for BootstrapError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ConfigRead { source, .. } | Self::Io(source) => Some(source),
            Self::ConfigParse(source) => Some(source),
            Self::InvalidConfig(_) | Self::Usage(_) => None,
        }
    }
}

impl From<io::Error> for BootstrapError {
    fn from(source: io::Error) -> Self {
        Self::Io(source)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    server: ServiceConfig,
    worker: ServiceConfig,
}

impl Configuration {
    fn load(path: &Path) -> Result<Self, BootstrapError> {
        let input = fs::read_to_string(path).map_err(|source| BootstrapError::ConfigRead {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&input)
    }

    fn parse(input: &str) -> Result<Self, BootstrapError> {
        let configuration: Self = toml::from_str(input).map_err(BootstrapError::ConfigParse)?;
        configuration.server.validate("server")?;
        configuration.worker.validate("worker")?;
        Ok(configuration)
    }

    const fn service(&self, role: ServiceRole) -> &ServiceConfig {
        match role {
            ServiceRole::Server => &self.server,
            ServiceRole::Worker => &self.worker,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceConfig {
    bind: SocketAddr,
    health_path: String,
    readiness_path: String,
}

impl ServiceConfig {
    fn validate(&self, section: &str) -> Result<(), BootstrapError> {
        validate_route(section, "health_path", &self.health_path)?;
        validate_route(section, "readiness_path", &self.readiness_path)?;
        if self.health_path == self.readiness_path {
            return Err(BootstrapError::InvalidConfig(format!(
                "{section} health_path and readiness_path must differ"
            )));
        }
        Ok(())
    }
}

fn validate_route(section: &str, key: &str, route: &str) -> Result<(), BootstrapError> {
    let valid = route.starts_with('/')
        && route.len() <= MAX_ROUTE_BYTES
        && route
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'?' && byte != b'#');
    if valid {
        Ok(())
    } else {
        Err(BootstrapError::InvalidConfig(format!(
            "{section}.{key} must be an absolute path of at most {MAX_ROUTE_BYTES} ASCII bytes without query or fragment"
        )))
    }
}

/// Runs the selected service or its configured readiness probe.
///
/// # Errors
///
/// Returns an error when configuration is invalid, the listener or probe cannot
/// use its socket, or command-line arguments are unsupported.
pub fn entry(role: ServiceRole) -> Result<(), BootstrapError> {
    let config_path =
        env::var_os(CONFIG_ENV).map_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH), PathBuf::from);
    let configuration = Configuration::load(&config_path)?;
    let service = configuration.service(role);
    let arguments: Vec<String> = env::args().skip(1).collect();

    match arguments.as_slice() {
        [] => serve(role, service),
        [argument] if argument == "--health-check" => readiness_probe(service),
        _ => Err(BootstrapError::Usage(format!(
            "usage: {} [--health-check]",
            role.name()
        ))),
    }
}

fn serve(role: ServiceRole, service: &ServiceConfig) -> Result<(), BootstrapError> {
    let listener = TcpListener::bind(service.bind)?;
    eprintln!(
        "{} listening on {} (health={}, readiness={})",
        role.name(),
        service.bind,
        service.health_path,
        service.readiness_path
    );

    for connection in listener.incoming() {
        match connection {
            Ok(mut stream) => {
                if let Err(error) = handle_connection(&mut stream, service) {
                    eprintln!("{} request failed: {error}", role.name());
                }
            }
            Err(error) => eprintln!("{} accept failed: {error}", role.name()),
        }
    }
    Ok(())
}

#[derive(Debug)]
enum RequestReadError {
    Io(io::Error),
    Malformed,
    TimedOut,
    TooLarge,
}

fn handle_connection(stream: &mut TcpStream, service: &ServiceConfig) -> io::Result<()> {
    handle_connection_with_limit(stream, service, REQUEST_DEADLINE)
}

fn handle_connection_with_limit(
    stream: &mut TcpStream,
    service: &ServiceConfig,
    request_limit: Duration,
) -> io::Result<()> {
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let deadline = Instant::now() + request_limit;

    let result = match read_request_path(stream, deadline) {
        Ok(path) if path == service.health_path || path == service.readiness_path => {
            ("200 OK", b"ok\n".as_slice())
        }
        Ok(_) => ("404 Not Found", b"not found\n".as_slice()),
        Err(RequestReadError::Malformed) => ("400 Bad Request", b"bad request\n".as_slice()),
        Err(RequestReadError::TimedOut) => ("408 Request Timeout", b"request timeout\n".as_slice()),
        Err(RequestReadError::TooLarge) => {
            ("413 Payload Too Large", b"payload too large\n".as_slice())
        }
        Err(RequestReadError::Io(error)) => return Err(error),
    };
    write_response(stream, result.0, result.1)
}

fn read_request_path(
    stream: &mut TcpStream,
    deadline: Instant,
) -> Result<String, RequestReadError> {
    let mut buffer = [0_u8; MAX_REQUEST_BYTES];
    let mut used = 0;

    loop {
        if used == buffer.len() {
            drain_oversized_request(stream, deadline);
            return Err(RequestReadError::TooLarge);
        }

        set_read_timeout_to_deadline(stream, deadline)?;

        match stream.read(&mut buffer[used..]) {
            Ok(0) => return Err(RequestReadError::Malformed),
            Ok(read) => used += read,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(RequestReadError::TimedOut);
            }
            Err(error) => return Err(RequestReadError::Io(error)),
        }

        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut request = httparse::Request::new(&mut headers);
        match request.parse(&buffer[..used]) {
            Ok(httparse::Status::Partial) => {}
            Ok(httparse::Status::Complete(_)) => {
                if request.method != Some("GET") || request.version != Some(1) {
                    return Err(RequestReadError::Malformed);
                }
                return request
                    .path
                    .map(str::to_owned)
                    .ok_or(RequestReadError::Malformed);
            }
            Err(_) => return Err(RequestReadError::Malformed),
        }
    }
}

fn set_read_timeout_to_deadline(
    stream: &TcpStream,
    deadline: Instant,
) -> Result<(), RequestReadError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(RequestReadError::TimedOut)?;
    stream
        .set_read_timeout(Some(remaining))
        .map_err(RequestReadError::Io)
}

fn drain_oversized_request(stream: &mut TcpStream, deadline: Instant) {
    let mut discarded = 0;
    let mut buffer = [0_u8; 1024];
    while discarded < MAX_DISCARD_BYTES {
        if set_read_timeout_to_deadline(stream, deadline).is_err() {
            break;
        }
        let remaining = MAX_DISCARD_BYTES - discarded;
        let limit = remaining.min(buffer.len());
        match stream.read(&mut buffer[..limit]) {
            Ok(0) => break,
            Ok(read) => discarded += read,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                break;
            }
            Err(_) => break,
        }
    }
}

fn write_response(stream: &mut TcpStream, status: &str, body: &[u8]) -> io::Result<()> {
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn readiness_probe(service: &ServiceConfig) -> Result<(), BootstrapError> {
    let address = probe_address(service.bind);
    let mut stream = TcpStream::connect_timeout(&address, IO_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    write!(
        stream,
        "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        service.readiness_path
    )?;
    stream.flush()?;

    let mut response = Vec::new();
    stream
        .take(MAX_PROBE_RESPONSE_BYTES)
        .read_to_end(&mut response)?;
    if response.starts_with(b"HTTP/1.1 200 OK\r\n") {
        Ok(())
    } else {
        Err(BootstrapError::Io(io::Error::other(
            "configured readiness endpoint returned a non-200 response",
        )))
    }
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

#[cfg(test)]
mod tests {
    use super::{Configuration, ServiceRole, handle_connection, handle_connection_with_limit};
    use std::io::{self, Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::thread;
    use std::time::{Duration, Instant};

    const CONFIG: &str = r#"
        [server]
        bind = "127.0.0.1:0"
        health_path = "/live"
        readiness_path = "/prepared"

        [worker]
        bind = "127.0.0.1:4101"
        health_path = "/worker-health"
        readiness_path = "/worker-ready"

    "#;

    #[test]
    fn config_values_select_bind_and_routes_by_role() {
        let configuration = Configuration::parse(CONFIG).expect("configuration parses");

        let server = configuration.service(ServiceRole::Server);
        assert_eq!(server.bind.to_string(), "127.0.0.1:0");
        assert_eq!(server.health_path, "/live");
        assert_eq!(server.readiness_path, "/prepared");

        let worker = configuration.service(ServiceRole::Worker);
        assert_eq!(worker.bind.to_string(), "127.0.0.1:4101");
        assert_eq!(worker.health_path, "/worker-health");
        assert_eq!(worker.readiness_path, "/worker-ready");
    }

    #[test]
    fn invalid_config_is_rejected() {
        let invalid_bind = CONFIG.replace("127.0.0.1:0", "not-a-socket");
        assert!(Configuration::parse(&invalid_bind).is_err());

        let duplicate_routes = CONFIG.replace("/prepared", "/live");
        assert!(Configuration::parse(&duplicate_routes).is_err());

        let invalid_route = CONFIG.replace("/live", "live");
        assert!(Configuration::parse(&invalid_route).is_err());
    }

    #[test]
    fn configured_health_route_returns_complete_success_response() {
        let response = exchange(&[b"GET /live HTTP/1.1\r\nHost: localhost\r\n\r\n"]);
        assert_response(&response, "200 OK", b"ok\n");
    }

    #[test]
    fn configured_readiness_route_returns_complete_success_response() {
        let response = exchange(&[b"GET /prepared HTTP/1.1\r\nHost: localhost\r\n\r\n"]);
        assert_response(&response, "200 OK", b"ok\n");
    }

    #[test]
    fn unknown_route_returns_404() {
        let response = exchange(&[b"GET /unknown HTTP/1.1\r\nHost: localhost\r\n\r\n"]);
        assert_response(&response, "404 Not Found", b"not found\n");
    }

    #[test]
    fn segmented_request_is_read_until_headers_are_complete() {
        let response = exchange(&[b"GET /li", b"ve HTTP/1.1\r\nHo", b"st: localhost\r\n\r\n"]);
        assert_response(&response, "200 OK", b"ok\n");
    }

    #[test]
    fn malformed_request_returns_400() {
        let response = exchange(&[b"GET /live\r\n\r\n"]);
        assert_response(&response, "400 Bad Request", b"bad request\n");
    }

    #[test]
    fn oversized_request_returns_413() {
        let oversized = vec![b'a'; 9_000];
        let response = exchange(&[&oversized]);
        assert_response(&response, "413 Payload Too Large", b"payload too large\n");
    }

    #[test]
    fn slow_drip_request_cannot_extend_absolute_deadline() {
        const TEST_REQUEST_LIMIT: Duration = Duration::from_millis(200);

        let configuration = Configuration::parse(CONFIG).expect("configuration parses");
        let service = configuration.service(ServiceRole::Server).clone();
        let listener = TcpListener::bind(service.bind).expect("listener binds");
        let address = listener.local_addr().expect("listener has address");

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("server accepts connection");
            let started = Instant::now();
            handle_connection_with_limit(&mut stream, &service, TEST_REQUEST_LIMIT)
                .expect("server handles connection");
            started.elapsed()
        });

        let mut client = TcpStream::connect(address).expect("client connects");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout configured");
        let mut writer = client.try_clone().expect("client stream clones");
        let drip = thread::spawn(move || {
            for byte in b"GET /live " {
                if writer.write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(40));
            }
            let _ = writer.shutdown(Shutdown::Write);
        });

        let (response, response_end) =
            read_deadline_response(&mut client).expect("deadline response reads");
        drip.join().expect("drip thread joins");
        let elapsed = server.join().expect("server thread joins");
        eprintln!("slow-drip handler elapsed: {elapsed:?}");

        assert!(
            elapsed <= TEST_REQUEST_LIMIT + Duration::from_millis(150),
            "handler exceeded its absolute request deadline: {elapsed:?}"
        );
        match response_end {
            DeadlineResponseEnd::CleanEof => {
                assert_response(&response, "408 Request Timeout", b"request timeout\n");
            }
            DeadlineResponseEnd::PeerReset => {
                let expected = b"HTTP/1.1 408 Request Timeout\r\nContent-Type: text/plain\r\nContent-Length: 16\r\nConnection: close\r\n\r\nrequest timeout\n";
                assert!(
                    expected.starts_with(&response),
                    "reset response is not a valid 408 prefix: {response:?}"
                );
            }
        }
    }

    #[test]
    fn deadline_response_accepts_peer_reset_after_response_bytes() {
        let expected = b"HTTP/1.1 408 Request Timeout\r\nContent-Length: 0\r\n\r\n";
        let mut reader = BytesThenError::new(expected, io::ErrorKind::ConnectionReset);

        let (response, response_end) =
            read_deadline_response(&mut reader).expect("peer reset ends response");

        assert_eq!(response, expected);
        assert_eq!(response_end, DeadlineResponseEnd::PeerReset);
    }

    #[test]
    fn deadline_response_accepts_peer_reset_before_response_bytes() {
        let mut reader = BytesThenError::new(&[], io::ErrorKind::ConnectionReset);

        let (response, response_end) =
            read_deadline_response(&mut reader).expect("peer reset ends response");

        assert!(response.is_empty());
        assert_eq!(response_end, DeadlineResponseEnd::PeerReset);
    }

    #[test]
    fn deadline_response_rejects_unrelated_io_error_after_response_bytes() {
        let mut reader = BytesThenError::new(b"partial", io::ErrorKind::BrokenPipe);

        let error = read_deadline_response(&mut reader).expect_err("unrelated error is preserved");

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    struct BytesThenError<'a> {
        bytes: &'a [u8],
        error: io::ErrorKind,
    }

    impl<'a> BytesThenError<'a> {
        fn new(bytes: &'a [u8], error: io::ErrorKind) -> Self {
            Self { bytes, error }
        }
    }

    impl Read for BytesThenError<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.bytes.is_empty() {
                return Err(io::Error::from(self.error));
            }
            let count = buffer.len().min(self.bytes.len());
            buffer[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            Ok(count)
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum DeadlineResponseEnd {
        CleanEof,
        PeerReset,
    }

    fn read_deadline_response(
        reader: &mut impl Read,
    ) -> io::Result<(Vec<u8>, DeadlineResponseEnd)> {
        let mut response = Vec::new();
        match reader.read_to_end(&mut response) {
            Ok(_) => Ok((response, DeadlineResponseEnd::CleanEof)),
            Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {
                Ok((response, DeadlineResponseEnd::PeerReset))
            }
            Err(error) => Err(error),
        }
    }

    fn exchange(chunks: &[&[u8]]) -> Vec<u8> {
        let configuration = Configuration::parse(CONFIG).expect("configuration parses");
        let service = configuration.service(ServiceRole::Server).clone();
        let listener = TcpListener::bind(service.bind).expect("listener binds");
        let address = listener.local_addr().expect("listener has address");

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("server accepts connection");
            handle_connection(&mut stream, &service).expect("server handles connection");
        });

        let mut client = TcpStream::connect(address).expect("client connects");
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout configured");
        for chunk in chunks {
            client.write_all(chunk).expect("request chunk writes");
            thread::sleep(Duration::from_millis(5));
        }
        let _ = client.shutdown(Shutdown::Write);

        let mut response = Vec::new();
        client.read_to_end(&mut response).expect("response reads");
        server.join().expect("server thread joins");
        response
    }

    fn assert_response(response: &[u8], status: &str, body: &[u8]) {
        let response_text = String::from_utf8_lossy(response);
        assert!(response_text.starts_with(&format!("HTTP/1.1 {status}\r\n")));
        assert!(response_text.contains(&format!("Content-Length: {}\r\n", body.len())));
        assert!(response.ends_with(body));
    }
}
