use std::collections::BTreeMap;
use std::env;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_api::{GrpcWebServerConfig, build_production_routes, serve_production_routes};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::portfolio::v1 as portfolio;
use ficant_contracts::ficant::portfolio::v1::portfolio_performance_service_client::PortfolioPerformanceServiceClient;
use ficant_contracts::ficant::portfolio::v1::portfolio_workbench_service_client::PortfolioWorkbenchServiceClient;
use ficant_domain::primitives::{ContentHash, DECIMAL_SCALE};
use ficant_server::{ServerSettings, build_production_grpc_services};
use prost::Message;
use prost_types::Timestamp;
use sqlx::postgres::PgPoolOptions;
use tonic::Request;

const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";
const ALLOWED_ORIGIN: &str = "http://127.0.0.1:5173";
const TENANT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA1";
const OWNER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA2";
const ACTOR_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE0";
const GROUP_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA4";
const BENCHMARK_LEVEL_TO_DELETE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC8";
const PERFORMANCE_SCHEMA: &str = "ficant.portfolio-performance-series.v1";
const SCOPES: &str =
    "portfolio:read,positions:read,rates:analyze,facts:read,definitions:read,artifacts:read";
const SERVER_TEST_ENVIRONMENT: &str =
    "ficant.server.environment.v1\narch=amd64\nos=windows\nprofile=test";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the explicit check.ps1 -IncludeIntegration environment"]
async fn production_performance_is_exact_persisted_restartable_and_fail_closed() {
    let Some(environment) = IntegrationEnvironment::load() else {
        eprintln!("skipping R8B Portfolio Performance SIT: integration environment is absent");
        return;
    };
    reset_and_migrate(&environment.database_url).await;
    run_bootstrap(&environment).await;
    run_bootstrap(&environment).await;

    let first_address = free_address();
    let first_server = RunningServer::start(first_address, &environment, "RESEARCHER").await;
    let first_endpoint = format!("http://{first_address}");
    let normalized = normalized_group_context(&first_endpoint).await;
    let first_series = native_success(&first_endpoint, normalized.clone()).await;
    assert_grpc_web_success(first_address, normalized.clone()).await;
    assert_response_evidence_was_persisted(&environment.database_url, &first_series).await;
    assert_eq!(formal_output_count(&environment.database_url).await, 1);
    first_server.stop().await;

    let restart_address = free_address();
    let restart_server = RunningServer::start(restart_address, &environment, "RESEARCHER").await;
    let restart_endpoint = format!("http://{restart_address}");
    let restarted = native_success(&restart_endpoint, normalized.clone()).await;
    assert_eq!(
        restarted.request_fingerprint,
        first_series.request_fingerprint
    );
    assert_eq!(
        restarted
            .formal_evidence
            .as_ref()
            .and_then(|value| value.output_identity.as_ref()),
        first_series
            .formal_evidence
            .as_ref()
            .and_then(|value| value.output_identity.as_ref())
    );
    assert_eq!(formal_output_count(&environment.database_url).await, 1);

    let mut drifted = normalized.clone();
    drifted
        .benchmark
        .as_mut()
        .and_then(|value| value.content_hash.as_mut())
        .expect("normalized benchmark hash is present")
        .value[0] ^= 0xff;
    assert_typed_error(
        &restart_endpoint,
        drifted,
        core::ErrorCode::LineageIncomplete,
    )
    .await;
    assert_eq!(formal_output_count(&environment.database_url).await, 1);

    delete_exact_benchmark_level(&environment.database_url).await;
    assert_typed_error(
        &restart_endpoint,
        normalized.clone(),
        core::ErrorCode::LineageIncomplete,
    )
    .await;
    assert_eq!(formal_output_count(&environment.database_url).await, 1);
    restart_server.stop().await;

    let forbidden_address = free_address();
    let forbidden_server =
        RunningServer::start(forbidden_address, &environment, "PLATFORM_ADMIN").await;
    assert_typed_error(
        &format!("http://{forbidden_address}"),
        normalized,
        core::ErrorCode::Forbidden,
    )
    .await;
    assert_eq!(formal_output_count(&environment.database_url).await, 1);
    forbidden_server.stop().await;
}

async fn normalized_group_context(endpoint: &str) -> portfolio::NormalizedPortfolioContext {
    let mut client = PortfolioWorkbenchServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Portfolio Workbench native client connects");
    let response = client
        .get_page(Request::new(portfolio::GetPortfolioPageRequest {
            page_id: portfolio::PortfolioWorkbenchPageId::P01 as i32,
            context: Some(portfolio::PortfolioContextInput {
                scope: Some(portfolio::PortfolioScopeSelector {
                    scope: Some(portfolio::portfolio_scope_selector::Scope::GroupId(
                        proto_id(GROUP_ID),
                    )),
                }),
                valuation_at: Some(proto_time(21, 9)),
                knowledge_at: Some(proto_time(21, 12)),
                currency: portfolio::PortfolioCurrencyMode::Cny as i32,
                look_through: portfolio::PortfolioLookThroughMode::None as i32,
                benchmark_id: Some(proto_id("01ARZ3NDEKTSV4RRFFQ69G5FA7")),
                period: portfolio::PortfolioPeriodPreset::OneDay as i32,
            }),
            selection: None,
        }))
        .await
        .expect("group context normalizes through the production Workbench")
        .into_inner();
    assert!(response.typed_error.is_none());
    let context = response
        .normalized_context
        .expect("P01 returns the exact normalized context");
    assert_eq!(context.scope.as_ref().unwrap().member_portfolios.len(), 2);
    assert_eq!(
        context
            .period_from
            .as_ref()
            .and_then(|value| value.instant.as_ref())
            .map(|value| value.seconds),
        Some(fixture_instant(20, 9).timestamp())
    );
    context
}

async fn native_success(
    endpoint: &str,
    context: portfolio::NormalizedPortfolioContext,
) -> portfolio::PortfolioPerformanceSeries {
    let mut client = PortfolioPerformanceServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Portfolio Performance native client connects");
    let response = client
        .get_portfolio_performance(Request::new(performance_request(context)))
        .await
        .expect("Portfolio Performance transport succeeds")
        .into_inner();
    let series = match response.result {
        Some(portfolio::get_portfolio_performance_response::Result::Series(value)) => value,
        Some(portfolio::get_portfolio_performance_response::Result::Error(error)) => panic!(
            "Portfolio Performance returned code={} message={} retryable={}",
            error.code, error.message, error.retryable
        ),
        None => panic!("Portfolio Performance response omitted its result"),
    };
    assert_exact_series(&series);
    series
}

fn assert_exact_series(series: &portfolio::PortfolioPerformanceSeries) {
    assert_eq!(series.points.len(), 1);
    let point = &series.points[0];
    assert_decimal(point.opening_nav.as_ref(), "300000000000000");
    assert_decimal(point.ending_nav.as_ref(), "318000000000000");
    assert_decimal(point.net_external_flow.as_ref(), "10000000000000");
    assert_decimal(point.economic_pnl.as_ref(), "8000000000000");
    assert_decimal(point.daily_return.as_ref(), "26666666667");
    assert_decimal(point.benchmark_return.as_ref(), "10000000000");
    assert_decimal(point.active_return.as_ref(), "16666666667");
    assert_decimal(point.cumulative_return.as_ref(), "26666666667");
    assert_decimal(point.benchmark_cumulative_return.as_ref(), "10000000000");
    assert_decimal(point.active_cumulative_return.as_ref(), "16666666667");
    let coverage = series.coverage.as_ref().expect("coverage is present");
    assert_eq!(coverage.expected_session_count, 2);
    assert_eq!(coverage.observed_session_count, 2);
    assert_eq!(coverage.expected_portfolio_observation_count, 4);
    assert_eq!(coverage.observed_portfolio_observation_count, 4);
    assert_eq!(coverage.expected_benchmark_observation_count, 2);
    assert_eq!(coverage.observed_benchmark_observation_count, 2);
    assert!(coverage.missing_sessions.is_empty());
    assert!(series.request_fingerprint.is_some());
    let evidence = series
        .formal_evidence
        .as_ref()
        .expect("formal evidence is returned");
    assert_eq!(evidence.schema_id, PERFORMANCE_SCHEMA);
    assert!(evidence.output_identity.is_some());
    assert!(evidence.result_hash.is_some());
    assert!(!evidence.implementations.is_empty());
    for kind in [
        core::FormalInputKind::PortfolioValuationSnapshot,
        core::FormalInputKind::BenchmarkLevelSnapshot,
        core::FormalInputKind::PortfolioPerformanceConvention,
    ] {
        assert!(
            evidence
                .consumed_inputs
                .iter()
                .any(|binding| binding.kind == kind as i32),
            "formal evidence omitted input kind {kind:?}"
        );
    }
}

fn assert_decimal(value: Option<&core::DecimalValue>, coefficient: &str) {
    let value = value.expect("decimal value is present");
    assert_eq!(value.coefficient, coefficient);
    assert_eq!(value.scale, DECIMAL_SCALE);
    assert!(value.unit.is_some());
}

async fn assert_typed_error(
    endpoint: &str,
    context: portfolio::NormalizedPortfolioContext,
    expected: core::ErrorCode,
) {
    let mut client = PortfolioPerformanceServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Portfolio Performance negative client connects");
    let response = client
        .get_portfolio_performance(Request::new(performance_request(context)))
        .await
        .expect("business failure remains a typed response")
        .into_inner();
    let Some(portfolio::get_portfolio_performance_response::Result::Error(error)) = response.result
    else {
        panic!("negative Portfolio Performance call must return ErrorDetail")
    };
    assert_eq!(error.code, expected as i32);
    assert!(!error.message.is_empty());
    assert!(!error.trace_id.is_empty());
    assert!(!error.retryable);
}

async fn assert_grpc_web_success(
    address: SocketAddr,
    context: portfolio::NormalizedPortfolioContext,
) {
    let response = grpc_web_exchange(
        address,
        "/ficant.portfolio.v1.PortfolioPerformanceService/GetPortfolioPerformance",
        performance_request(context).encode_to_vec(),
    )
    .await;
    let header_end = response
        .windows(4)
        .position(|value| value == b"\r\n\r\n")
        .expect("gRPC-Web response contains HTTP headers");
    let headers = String::from_utf8_lossy(&response[..header_end]).to_ascii_lowercase();
    assert!(headers.starts_with("http/1.1 200 ok\r\n"));
    assert!(headers.contains("content-type: application/grpc-web+proto\r\n"));
    assert!(headers.contains("access-control-allow-origin: http://127.0.0.1:5173\r\n"));
    assert!(
        response
            .windows(PERFORMANCE_SCHEMA.len())
            .any(|value| value == PERFORMANCE_SCHEMA.as_bytes()),
        "raw gRPC-Web response contains the real formal performance series"
    );
}

async fn grpc_web_exchange(address: SocketAddr, path: &str, payload: Vec<u8>) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.push(0);
    frame.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
    frame.extend_from_slice(&payload);
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: {ALLOWED_ORIGIN}\r\nContent-Type: application/grpc-web+proto\r\nX-Grpc-Web: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        frame.len()
    )
    .into_bytes();
    request.extend_from_slice(&frame);
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))
            .expect("gRPC-Web client connects");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("gRPC-Web read timeout configures");
        stream.write_all(&request).expect("gRPC-Web request writes");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .expect("gRPC-Web response reads");
        response
    })
    .await
    .expect("blocking gRPC-Web exchange joins")
}

async fn assert_response_evidence_was_persisted(
    database_url: &str,
    series: &portfolio::PortfolioPerformanceSeries,
) {
    let response_identity = series
        .formal_evidence
        .as_ref()
        .and_then(|value| value.output_identity.as_ref())
        .expect("response identity is present");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .expect("formal-output database is reachable");
    let bytes: Vec<u8> = sqlx::query_scalar(
        "SELECT formal_evidence FROM analytics.formal_outputs
         WHERE tenant_id=$1 AND schema_id=$2 AND output_identity=$3",
    )
    .bind(TENANT_ID)
    .bind(PERFORMANCE_SCHEMA)
    .bind(hash_hex(&response_identity.value))
    .fetch_one(&pool)
    .await
    .expect("response evidence was durably published before success returned");
    let persisted = core::FormalOutputEvidence::decode(bytes.as_slice())
        .expect("persisted evidence is canonical protobuf");
    assert_eq!(persisted.output_identity.as_ref(), Some(response_identity));
    pool.close().await;
}

async fn formal_output_count(database_url: &str) -> i64 {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .expect("formal-output database is reachable");
    let count = sqlx::query_scalar(
        "SELECT count(*) FROM analytics.formal_outputs
         WHERE tenant_id=$1 AND schema_id=$2",
    )
    .bind(TENANT_ID)
    .bind(PERFORMANCE_SCHEMA)
    .fetch_one(&pool)
    .await
    .expect("formal-output count is readable");
    pool.close().await;
    count
}

async fn delete_exact_benchmark_level(database_url: &str) {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .expect("benchmark database is reachable");
    let mut transaction = pool.begin().await.expect("negative transaction begins");
    sqlx::query("SET LOCAL session_replication_role='replica'")
        .execute(&mut *transaction)
        .await
        .expect("isolated test can disable immutable triggers locally");
    let deleted = sqlx::query(
        "DELETE FROM portfolio.benchmark_level_snapshots
         WHERE tenant_id=$1 AND snapshot_id=$2",
    )
    .bind(TENANT_ID)
    .bind(BENCHMARK_LEVEL_TO_DELETE)
    .execute(&mut *transaction)
    .await
    .expect("exact benchmark fixture deletion succeeds")
    .rows_affected();
    assert_eq!(deleted, 1);
    transaction
        .commit()
        .await
        .expect("negative mutation commits");
    pool.close().await;
}

fn performance_request(
    context: portfolio::NormalizedPortfolioContext,
) -> portfolio::GetPortfolioPerformanceRequest {
    portfolio::GetPortfolioPerformanceRequest {
        context: Some(context),
    }
}

fn proto_id(value: &str) -> core::Ulid {
    core::Ulid {
        value: value.to_owned(),
    }
}

fn proto_time(day: u32, hour: u32) -> core::MarketTime {
    let instant = fixture_instant(day, hour);
    core::MarketTime {
        instant: Some(Timestamp {
            seconds: instant.timestamp(),
            nanos: 0,
        }),
        market_timezone: "Asia/Shanghai".to_owned(),
        local_trading_date: NaiveDate::from_ymd_opt(2026, 8, day)
            .expect("fixture date is valid")
            .to_string(),
    }
}

fn fixture_instant(day: u32, hour: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0)
        .single()
        .expect("fixture instant is valid")
}

fn hash_hex(value: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in value {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("String writes cannot fail");
    }
    encoded
}

async fn run_bootstrap(environment: &IntegrationEnvironment) {
    let repository = repository_root();
    let script = repository.join("scripts/bootstrap-portfolio-performance.ps1");
    let output = tokio::task::spawn_blocking({
        let environment = environment.clone();
        move || {
            Command::new("pwsh")
                .args(["-NoProfile", "-NonInteractive", "-File"])
                .arg(script)
                .current_dir(repository)
                .env("FICANT_EXPERIMENT_DATABASE_URL", &environment.database_url)
                .env("FICANT_EXPERIMENT_S3_ENDPOINT", &environment.s3_endpoint)
                .env("FICANT_EXPERIMENT_S3_BUCKET", &environment.s3_bucket)
                .env(
                    "FICANT_EXPERIMENT_S3_ACCESS_KEY",
                    &environment.s3_access_key,
                )
                .env(
                    "FICANT_EXPERIMENT_S3_SECRET_KEY",
                    &environment.s3_secret_key,
                )
                .output()
                .expect("Portfolio Performance bootstrap process starts")
        }
    })
    .await
    .expect("Portfolio Performance bootstrap process joins");
    assert!(
        output.status.success(),
        "Portfolio Performance bootstrap failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[derive(Clone)]
struct IntegrationEnvironment {
    database_url: String,
    s3_endpoint: String,
    s3_bucket: String,
    s3_access_key: String,
    s3_secret_key: String,
    runtime_digest: String,
}

impl IntegrationEnvironment {
    fn load() -> Option<Self> {
        Some(Self {
            database_url: env::var("FICANT_TEST_DATABASE_URL").ok()?,
            s3_endpoint: env::var("FICANT_TEST_S3_ENDPOINT").ok()?,
            s3_bucket: env::var("FICANT_TEST_S3_BUCKET").ok()?,
            s3_access_key: env::var("FICANT_TEST_S3_ACCESS_KEY").ok()?,
            s3_secret_key: env::var("FICANT_TEST_S3_SECRET_KEY").ok()?,
            runtime_digest: env::var("FICANT_TEST_RUNTIME_IMAGE_DIGEST").ok()?,
        })
    }
}

struct RunningServer {
    handle: tokio::task::JoinHandle<Result<(), ficant_api::GrpcWebServeError>>,
}

impl RunningServer {
    async fn start(address: SocketAddr, environment: &IntegrationEnvironment, role: &str) -> Self {
        let settings = ServerSettings::try_from_values(&server_values(address, environment, role))
            .expect("R8B Portfolio Performance SIT settings are valid");
        let services =
            build_production_grpc_services(&settings).expect("production services compose");
        let routes = build_production_routes(services).expect("production routes are unique");
        let handle = tokio::spawn(serve_production_routes(
            GrpcWebServerConfig {
                bind: address,
                allowed_origins: vec![ALLOWED_ORIGIN.to_owned()],
            },
            routes,
        ));
        wait_until_listening(address).await;
        Self { handle }
    }

    async fn stop(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

async fn reset_and_migrate(database_url: &str) {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(database_url)
        .await
        .expect("integration PostgreSQL is reachable");
    sqlx::raw_sql(
        "DROP SCHEMA IF EXISTS portfolio CASCADE;
         DROP SCHEMA IF EXISTS analytics CASCADE;
         DROP SCHEMA IF EXISTS data CASCADE;
         DROP SCHEMA IF EXISTS storage CASCADE;
         DROP SCHEMA IF EXISTS research CASCADE;
         DROP SCHEMA IF EXISTS market CASCADE;
         DROP SCHEMA IF EXISTS core CASCADE;
         DROP TABLE IF EXISTS public._sqlx_migrations;",
    )
    .execute(&pool)
    .await
    .expect("integration database reset succeeds");
    sqlx::migrate::Migrator::new(repository_root().join("migrations/postgresql"))
        .await
        .expect("migration directory is readable")
        .run(&pool)
        .await
        .expect("R8B migrations apply");
    pool.close().await;
}

#[allow(clippy::too_many_lines)]
fn server_values(
    address: SocketAddr,
    environment: &IntegrationEnvironment,
    role: &str,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("FICANT_GRPC_BIND".to_owned(), address.to_string()),
        (
            "FICANT_GRPC_WEB_ALLOWED_ORIGINS".to_owned(),
            ALLOWED_ORIGIN.to_owned(),
        ),
        ("FICANT_PLATFORM_SIGNING_KEY_HEX".to_owned(), KEY.to_owned()),
        ("FICANT_PLATFORM_TRACE_KEY_HEX".to_owned(), KEY.to_owned()),
        (
            "FICANT_CODE_COMMIT_SHA".to_owned(),
            ficant_server::compiled_git_commit_sha().to_owned(),
        ),
        (
            "FICANT_CODE_TREE_SHA".to_owned(),
            ficant_server::compiled_git_tree_sha().to_owned(),
        ),
        (
            "FICANT_SERVER_RUNTIME_IMAGE_DIGEST".to_owned(),
            environment.runtime_digest.clone(),
        ),
        (
            "FICANT_SERVER_ENVIRONMENT_ATTESTATION".to_owned(),
            content_digest(SERVER_TEST_ENVIRONMENT),
        ),
        (
            "FICANT_EXPERIMENT_DATABASE_URL".to_owned(),
            environment.database_url.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_ENDPOINT".to_owned(),
            environment.s3_endpoint.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_BUCKET".to_owned(),
            environment.s3_bucket.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_ACCESS_KEY".to_owned(),
            environment.s3_access_key.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_SECRET_KEY".to_owned(),
            environment.s3_secret_key.clone(),
        ),
        (
            "FICANT_EXPERIMENT_CURSOR_KEY_HEX".to_owned(),
            KEY.to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_TENANT_ID".to_owned(),
            TENANT_ID.to_owned(),
        ),
        ("FICANT_EXPERIMENT_OWNER_ID".to_owned(), OWNER_ID.to_owned()),
        ("FICANT_EXPERIMENT_ACTOR_ID".to_owned(), ACTOR_ID.to_owned()),
        (
            "FICANT_EXPERIMENT_RUNTIME_IMAGE_DIGEST".to_owned(),
            environment.runtime_digest.clone(),
        ),
        (
            "FICANT_EXPERIMENT_ENVIRONMENT_ATTESTATION".to_owned(),
            "ficant.worker.environment.v1\narch=amd64\nos=linux\nprofile=test".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_NATIVE_SOURCE_DIGEST".to_owned(),
            ficant_native_nodes::native_node_source_digest_attestation(),
        ),
        (
            "FICANT_INPUT_FILE_NDJSON_ROOT".to_owned(),
            env::temp_dir()
                .join("ficant-r8b-unused-input")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "FICANT_INPUT_FILE_CONNECTION_BINDING".to_owned(),
            "r8b-unused-file".to_owned(),
        ),
        (
            "FICANT_INPUT_POSTGRES_CONNECTION_BINDING".to_owned(),
            "r8b-unused-postgres".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_SUBJECT".to_owned(),
            "portfolio-performance-researcher".to_owned(),
        ),
        ("FICANT_LOOPBACK_ACTOR_ID".to_owned(), ACTOR_ID.to_owned()),
        ("FICANT_LOOPBACK_TENANT_ID".to_owned(), TENANT_ID.to_owned()),
        (
            "FICANT_LOOPBACK_ALLOWED_OWNER_IDS".to_owned(),
            OWNER_ID.to_owned(),
        ),
        ("FICANT_LOOPBACK_ACTIVE_ROLE".to_owned(), role.to_owned()),
        ("FICANT_LOOPBACK_SCOPES".to_owned(), SCOPES.to_owned()),
    ])
}

fn content_digest(value: &str) -> String {
    format!(
        "sha256:{}",
        hash_hex(ContentHash::digest(value.as_bytes()).as_bytes())
    )
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("server remains two levels below repository root")
        .to_path_buf()
}

fn free_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral listener binds");
    let address = listener.local_addr().expect("ephemeral address is known");
    drop(listener);
    address
}

async fn wait_until_listening(address: SocketAddr) {
    for _ in 0..100 {
        if TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("production server did not listen at {address}");
}
