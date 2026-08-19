use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ficant_api::{GrpcWebServerConfig, build_production_routes, serve_production_routes};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::core::v1::foundation_change_service_client::FoundationChangeServiceClient;
use ficant_contracts::ficant::market::v1 as market;
use ficant_contracts::ficant::market::v1::data_source_registry_service_client::DataSourceRegistryServiceClient;
use ficant_contracts::ficant::market::v1::market_definition_service_client::MarketDefinitionServiceClient;
use ficant_contracts::ficant::market::v1::market_fact_service_client::MarketFactServiceClient;
use ficant_contracts::ficant::research::v1 as research;
use ficant_contracts::ficant::research::v1::snapshot_service_client::SnapshotServiceClient;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, Ulid};
use ficant_server::{ServerSettings, build_production_grpc_services};
use sqlx::postgres::PgPoolOptions;
use tonic::{Request, transport::Channel};

const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";
const ADMIN_TOKEN: &str = "r6a-production-admin-token";
const ALLOWED_ORIGIN: &str = "http://127.0.0.1:4174";
const FILE_BINDING: &str = "r6a-file-fixture";
const POSTGRES_BINDING: &str = "r6a-postgres-fixture";
const SCHEMA_ID: &str = "ficant.market.quote.canonical.v1";
const SCHEMA_HASH_HEX: &str = "e804a0becec18e51dde1be4250384ffe667cf4149c34dc3d2cfc82a206d71502";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the explicit check.ps1 -IncludeIntegration environment"]
async fn production_r6a_input_plane_round_trips_native_grpc_and_grpc_web() {
    let Some(environment) = IntegrationEnvironment::load() else {
        eprintln!("skipping R6A production SIT: integration environment is not configured");
        return;
    };
    reset_and_migrate(&environment.database_url).await;
    let fixture = FixtureDirectory::new();
    fixture.write_quotes();

    let first_address = free_address();
    let first = RunningServer::start(
        first_address,
        &environment,
        fixture.path(),
        &server_values(first_address, &environment, fixture.path()),
    )
    .await;
    grpc_web_routes_are_reachable(first_address).await;
    let endpoint = format!("http://{first_address}");
    publish_input_authorities(&endpoint).await;
    let first_value =
        import_replay_and_reject(&endpoint, &fixture, &environment.database_url).await;
    first.stop().await;

    let second_address = free_address();
    let second = RunningServer::start(
        second_address,
        &environment,
        fixture.path(),
        &server_values(second_address, &environment, fixture.path()),
    )
    .await;
    assert_restarted_snapshot_and_audit(&format!("http://{second_address}"), &first_value).await;
    second.stop().await;
}

async fn publish_input_authorities(endpoint: &str) {
    assert_native_fact_route(endpoint).await;
    let mut definitions = MarketDefinitionServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Definition client connects");
    append_definition(&mut definitions, unit_definition(), "r6a/unit-v1").await;
    append_definition(&mut definitions, calendar_definition(), "r6a/calendar-v1").await;
    append_definition(
        &mut definitions,
        instrument_definition(),
        "r6a/instrument-v1",
    )
    .await;
    publish_source_authorization(endpoint).await;
}

async fn assert_native_fact_route(endpoint: &str) {
    let mut facts = MarketFactServiceClient::connect(endpoint.to_owned())
        .await
        .expect("MarketFact client connects");
    let malformed_fact_read = facts
        .get_curve_snapshot(admin_request(market::GetCurveSnapshotRequest::default()))
        .await
        .expect("native MarketFact route returns a typed business response")
        .into_inner();
    assert!(matches!(
        malformed_fact_read.result,
        Some(market::get_curve_snapshot_response::Result::Error(_))
    ));
}

async fn publish_source_authorization(endpoint: &str) {
    let source_hash = canonical_data_source_content_hash();
    let mapping_hash = mapping_content_hash();
    let authorization_hash = authorization_content_hash(&source_hash, &mapping_hash);
    let mut sources = DataSourceRegistryServiceClient::connect(endpoint.to_owned())
        .await
        .expect("DataSource client connects");
    let registered = sources
        .register_data_source(admin_request(market::RegisterDataSourceRequest {
            idempotency_key: "r6a/source-v1".to_owned(),
            expected_latest_version: 0,
            definition: Some(source_proto()),
            change: Some(change("register production SIT data source")),
        }))
        .await
        .expect("register transport succeeds")
        .into_inner();
    assert!(matches!(
        registered.result,
        Some(market::register_data_source_response::Result::Definition(_))
    ));
    let published = sources
        .publish_data_source_authorization(admin_request(
            market::PublishDataSourceAuthorizationRequest {
                idempotency_key: "r6a/authorization-v1".to_owned(),
                expected_latest_version: 0,
                authorization: Some(authorization_proto(
                    &source_hash,
                    &mapping_hash,
                    &authorization_hash,
                )),
                change: Some(change("authorize production SIT canonical import")),
                mapping: Some(mapping_proto(&mapping_hash)),
            },
        ))
        .await
        .expect("authorization transport succeeds")
        .into_inner();
    assert!(matches!(
        published.result,
        Some(market::publish_data_source_authorization_response::Result::Authorization(_))
    ));
}

async fn import_replay_and_reject(
    endpoint: &str,
    fixture: &FixtureDirectory,
    database_url: &str,
) -> research::DataSnapshot {
    let mut snapshots = SnapshotServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Snapshot client connects");
    let request = import_request();
    let imported = snapshots
        .import_canonical_quote_snapshot(Request::new(request.clone()))
        .await
        .expect("canonical import transport succeeds")
        .into_inner();
    let Some(research::import_canonical_quote_snapshot_response::Result::DataSnapshot(first_value)) =
        imported.result
    else {
        panic!("authorized fixture import must publish a DataSnapshot");
    };
    assert_eq!(first_value.data_snapshot_id, Some(proto_id('S')));
    assert_eq!(first_value.authorization_ref, Some(version_ref('V', 1)));
    assert_eq!(first_value.actor_id, Some(proto_id('R')));

    fixture.remove_quotes();
    let replayed = snapshots
        .import_canonical_quote_snapshot(Request::new(request))
        .await
        .expect("replay transport succeeds after source removal")
        .into_inner();
    let Some(research::import_canonical_quote_snapshot_response::Result::DataSnapshot(replayed)) =
        replayed.result
    else {
        panic!("idempotent replay must not reopen the removed source adapter");
    };
    assert_eq!(replayed, first_value);

    let before_rejection = mutation_counts(database_url).await;
    let rejected = snapshots
        .import_canonical_quote_snapshot(Request::new(unauthorized_mapping_request()))
        .await
        .expect("authorization rejection stays inside the typed transport")
        .into_inner();
    let Some(research::import_canonical_quote_snapshot_response::Result::Error(error)) =
        rejected.result
    else {
        panic!("mapping authority drift must be rejected before the removed adapter is opened");
    };
    assert_eq!(error.code, core::ErrorCode::Forbidden as i32);
    assert_eq!(error.resource_ref, format!("data-source:{}@1", id('D')));
    assert!(error.field_violations.iter().any(|violation| {
        violation.field == "authorization_ref" && violation.description.contains("Platform Admin")
    }));
    assert_eq!(
        mutation_counts(database_url).await,
        before_rejection,
        "authorization drift must not stage blobs or mutate snapshot/audit/idempotency state"
    );
    first_value
}

async fn assert_restarted_snapshot_and_audit(endpoint: &str, expected: &research::DataSnapshot) {
    let mut snapshots = SnapshotServiceClient::connect(endpoint.to_owned())
        .await
        .expect("restarted Snapshot client connects");
    let persisted = snapshots
        .get_snapshot(Request::new(research::GetSnapshotRequest {
            snapshot_id: Some(proto_id('S')),
        }))
        .await
        .expect("persisted read transport succeeds")
        .into_inner();
    let Some(research::get_snapshot_response::Result::DataSnapshot(persisted)) = persisted.result
    else {
        panic!("restarted service must read the verified persisted DataSnapshot");
    };
    assert_eq!(&persisted, expected);

    let mut governance = FoundationChangeServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Governance client connects");
    let listed = governance
        .list_foundation_changes(admin_request(core::ListFoundationChangesRequest {
            resource_ref: String::new(),
            actor_id: None,
            occurred_from: None,
            occurred_to: None,
            page: Some(core::PageRequest {
                page_size: 100,
                cursor: String::new(),
            }),
        }))
        .await
        .expect("governance list transport succeeds")
        .into_inner();
    let Some(core::list_foundation_changes_response::Result::Changes(changes)) = listed.result
    else {
        panic!("Platform Admin must read persisted FoundationChange records");
    };
    assert!(changes.changes.iter().any(|change| {
        change.operation == "data-snapshot.import-canonical-quotes"
            && change.authorization_ref == Some(version_ref('V', 1))
            && change.actor_id == Some(proto_id('R'))
    }));
}

async fn append_definition(
    client: &mut MarketDefinitionServiceClient<Channel>,
    definition: market::MarketDefinition,
    idempotency_key: &str,
) {
    let response = client
        .append_definition(admin_request(market::AppendDefinitionRequest {
            idempotency_key: idempotency_key.to_owned(),
            expected_latest_version: 0,
            definition: Some(definition),
            change: Some(change("publish production SIT exact Definition")),
        }))
        .await
        .expect("Definition append transport succeeds")
        .into_inner();
    assert!(matches!(
        response.result,
        Some(market::append_definition_response::Result::Definition(_))
    ));
}

struct RunningServer {
    handle: tokio::task::JoinHandle<Result<(), ficant_api::GrpcWebServeError>>,
}

impl RunningServer {
    async fn start(
        address: SocketAddr,
        _environment: &IntegrationEnvironment,
        _fixture_root: &Path,
        values: &BTreeMap<String, String>,
    ) -> Self {
        let settings = ServerSettings::try_from_values(values).expect("SIT settings are valid");
        let services =
            build_production_grpc_services(&settings).expect("production R6B services compose");
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

struct FixtureDirectory {
    root: PathBuf,
}

impl FixtureDirectory {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "ficant-r6a-governed-input-{}-{}",
            std::process::id(),
            id('S').as_str()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("stale scoped fixture directory is removable");
        }
        fs::create_dir_all(&root).expect("scoped fixture directory is created");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn quote_path(&self) -> PathBuf {
        self.root.join("quotes.ndjson")
    }

    fn write_quotes(&self) {
        fs::write(
            self.quote_path(),
            concat!(
                "{\"ask_coefficient\":\"1010100\",\"ask_scale\":4,",
                "\"bid_coefficient\":\"1010000\",\"bid_scale\":4,",
                "\"instrument_key\":\"260011.IB\",",
                "\"observed_at\":\"2026-08-13T02:00:00Z\",",
                "\"source_record_id\":\"record-1\",",
                "\"visible_at\":\"2026-08-13T02:00:01Z\"}\n"
            ),
        )
        .expect("deterministic quote fixture is written");
    }

    fn remove_quotes(&self) {
        fs::remove_file(self.quote_path()).expect("scoped quote fixture is removed");
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        if self.root.starts_with(env::temp_dir()) && self.root.exists() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

async fn reset_and_migrate(database_url: &str) {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(database_url)
        .await
        .expect("integration PostgreSQL is reachable");
    sqlx::raw_sql(
        "DROP SCHEMA IF EXISTS data CASCADE;
         DROP SCHEMA IF EXISTS storage CASCADE;
         DROP SCHEMA IF EXISTS research CASCADE;
         DROP SCHEMA IF EXISTS market CASCADE;
         DROP SCHEMA IF EXISTS core CASCADE;
         DROP TABLE IF EXISTS public._sqlx_migrations;",
    )
    .execute(&pool)
    .await
    .expect("integration database reset succeeds");
    let migrations = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations/postgresql");
    sqlx::migrate::Migrator::new(migrations)
        .await
        .expect("migration directory is readable")
        .run(&pool)
        .await
        .expect("R6A migrations apply");
    pool.close().await;
}

async fn mutation_counts(database_url: &str) -> (i64, i64, i64, i64, i64) {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
        .expect("integration PostgreSQL remains reachable");
    let counts = sqlx::query_as(
        "SELECT
           (SELECT count(*) FROM research.data_snapshots),
           (SELECT count(*) FROM core.foundation_change_records),
           (SELECT count(*) FROM core.idempotency_records),
           (SELECT count(*) FROM storage.orphan_candidates),
           (SELECT count(*) FROM storage.blobs)",
    )
    .fetch_one(&pool)
    .await
    .expect("R6A mutation counts are readable");
    pool.close().await;
    counts
}

async fn grpc_web_routes_are_reachable(address: SocketAddr) {
    for path in [
        "/ficant.market.v1.MarketDefinitionService/GetDefinitionVersion",
        "/ficant.market.v1.MarketFactService/GetMarketFact",
        "/ficant.research.v1.SnapshotService/GetSnapshot",
    ] {
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: {ALLOWED_ORIGIN}\r\nContent-Type: application/grpc-web+proto\r\nX-Grpc-Web: 1\r\nContent-Length: 5\r\nConnection: close\r\n\r\n"
        )
        .into_bytes();
        request.extend_from_slice(&[0, 0, 0, 0, 0]);
        let response = exchange(address, request).await;
        assert!(response.starts_with("http/1.1 200 ok\r\n"), "{path}");
        assert!(
            response.contains("content-type: application/grpc-web+proto\r\n"),
            "{path}"
        );
        assert!(
            response.contains(&format!(
                "access-control-allow-origin: {ALLOWED_ORIGIN}\r\n"
            )),
            "{path}"
        );
    }
}

async fn exchange(address: SocketAddr, request: Vec<u8>) -> String {
    tokio::task::spawn_blocking(move || {
        let mut stream =
            TcpStream::connect_timeout(&address, Duration::from_secs(2)).expect("client connects");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout configured");
        stream.write_all(&request).expect("request writes");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("response reads");
        String::from_utf8_lossy(&response).to_ascii_lowercase()
    })
    .await
    .expect("blocking exchange joins")
}

fn admin_request<T>(message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {ADMIN_TOKEN}")
            .parse()
            .expect("fixed bearer metadata is valid"),
    );
    request
}

fn import_request() -> research::ImportCanonicalQuoteSnapshotRequest {
    research::ImportCanonicalQuoteSnapshotRequest {
        idempotency_key: "r6a/snapshot-import-v1".to_owned(),
        target_snapshot_id: Some(proto_id('S')),
        authorization_ref: Some(version_ref('V', 1)),
        mapping: Some(mapping_proto(&mapping_content_hash())),
        calendar: Some(calendar_proto()),
        unit: Some(unit_proto()),
        as_of: Some(market_time(1_786_586_400, "2026-08-13")),
        visible_at: Some(market_time(1_786_586_700, "2026-08-13")),
        import_reason: "approved production SIT daily import".to_owned(),
    }
}

fn unauthorized_mapping_request() -> research::ImportCanonicalQuoteSnapshotRequest {
    let mut request = import_request();
    "r6a/rejected-mapping-import".clone_into(&mut request.idempotency_key);
    request.target_snapshot_id = Some(proto_id('X'));
    request.mapping = Some(mapping_proto_for('X', &mapping_content_hash_for('X')));
    request
}

fn source_proto() -> market::DataSourceDefinition {
    market::DataSourceDefinition {
        data_source: Some(version_ref('D', 1)),
        owner: Some(owner_proto()),
        kind: market::DataSourceKind::FileNdjson as i32,
        name: "R6A production SIT quotes".to_owned(),
        connection_binding: FILE_BINDING.to_owned(),
        dataset: "quotes".to_owned(),
        canonical_schema_id: SCHEMA_ID.to_owned(),
        canonical_schema_hash: Some(sha256(&schema_hash())),
        price_source_type: market::PriceSourceType::ActiveQuote as i32,
    }
}

fn authorization_proto(
    source_hash: &ContentHash,
    mapping_hash: &ContentHash,
    content_hash: &ContentHash,
) -> market::DataSourceAuthorization {
    market::DataSourceAuthorization {
        r#ref: Some(version_ref('V', 1)),
        owner: Some(owner_proto()),
        source: Some(version_ref('D', 1)),
        source_hash: Some(sha256(source_hash)),
        interface: market::ImportInterface::CanonicalQuoteSnapshot as i32,
        schema_id: SCHEMA_ID.to_owned(),
        schema_hash: Some(sha256(&schema_hash())),
        effective_from: Some(market_time(1_767_225_600, "2026-01-01")),
        effective_to: Some(market_time(1_798_761_600, "2027-01-01")),
        state: market::DataSourceAuthorizationState::Active as i32,
        supersedes: None,
        content_hash: Some(sha256(content_hash)),
        mapping_id: Some(proto_id('M')),
        mapping_hash: Some(sha256(mapping_hash)),
    }
}

fn mapping_proto(content_hash: &ContentHash) -> market::InstrumentMapping {
    mapping_proto_for('M', content_hash)
}

fn mapping_proto_for(
    mapping_suffix: char,
    content_hash: &ContentHash,
) -> market::InstrumentMapping {
    market::InstrumentMapping {
        mapping_id: Some(proto_id(mapping_suffix)),
        owner: Some(owner_proto()),
        source: Some(version_ref('D', 1)),
        entries: vec![market::InstrumentMappingEntry {
            source_instrument_key: "260011.IB".to_owned(),
            effective_from: Some(market_time(1_767_225_600, "2026-01-01")),
            effective_to: Some(market_time(1_798_761_600, "2027-01-01")),
            instrument: Some(version_ref('I', 1)),
        }],
        content_hash: Some(sha256(content_hash)),
    }
}

fn mapping_content_hash() -> ContentHash {
    mapping_content_hash_for('M')
}

fn mapping_content_hash_for(mapping_suffix: char) -> ContentHash {
    let mut bytes = b"ficant-instrument-mapping/v2\0".to_vec();
    append_mapping(&mut bytes, id(mapping_suffix).as_str());
    append_mapping(&mut bytes, id('T').as_str());
    append_mapping(&mut bytes, id('P').as_str());
    append_mapping(&mut bytes, id('D').as_str());
    bytes.extend_from_slice(&1_u64.to_be_bytes());
    append_mapping(&mut bytes, "260011.IB");
    append_mapping_time(&mut bytes, 1_767_225_600, "2026-01-01");
    append_mapping_time(&mut bytes, 1_798_761_600, "2027-01-01");
    append_mapping(&mut bytes, id('I').as_str());
    bytes.extend_from_slice(&1_u64.to_be_bytes());
    ContentHash::digest(&bytes)
}

fn append_mapping_time(bytes: &mut Vec<u8>, seconds: i64, local_date: &str) {
    bytes.extend_from_slice(&seconds.to_be_bytes());
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    append_mapping(bytes, "Asia/Shanghai");
    append_mapping(bytes, local_date);
}

fn append_mapping(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(
        &u64::try_from(value.len())
            .expect("fixture token length fits u64")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(value.as_bytes());
}

fn canonical_data_source_content_hash() -> ContentHash {
    let mut bytes = Vec::new();
    append_hash(&mut bytes, b"ficant.rates.data-source.v1");
    append_hash(&mut bytes, id('D').as_str().as_bytes());
    append_hash(&mut bytes, &1_u64.to_be_bytes());
    append_hash(&mut bytes, id('T').as_str().as_bytes());
    append_hash(&mut bytes, id('P').as_str().as_bytes());
    append_hash(&mut bytes, &[1]);
    append_hash(&mut bytes, b"R6A production SIT quotes");
    append_hash(&mut bytes, FILE_BINDING.as_bytes());
    append_hash(&mut bytes, b"quotes");
    append_hash(&mut bytes, SCHEMA_ID.as_bytes());
    append_hash(&mut bytes, schema_hash().as_bytes());
    append_hash(&mut bytes, &[2]);
    ContentHash::digest(&bytes)
}

fn authorization_content_hash(
    source_hash: &ContentHash,
    mapping_hash: &ContentHash,
) -> ContentHash {
    let mut bytes = Vec::new();
    append_hash(&mut bytes, b"ficant.data-source-authorization.v1");
    append_hash(&mut bytes, id('V').as_str().as_bytes());
    append_hash(&mut bytes, &1_u64.to_be_bytes());
    append_hash(&mut bytes, id('T').as_str().as_bytes());
    append_hash(&mut bytes, id('P').as_str().as_bytes());
    append_hash(&mut bytes, id('D').as_str().as_bytes());
    append_hash(&mut bytes, &1_u64.to_be_bytes());
    append_hash(&mut bytes, source_hash.as_bytes());
    append_hash(&mut bytes, &[1]);
    append_hash(&mut bytes, SCHEMA_ID.as_bytes());
    append_hash(&mut bytes, schema_hash().as_bytes());
    append_authorization_time(&mut bytes, 1_767_225_600, "2026-01-01");
    append_authorization_time(&mut bytes, 1_798_761_600, "2027-01-01");
    append_hash(&mut bytes, &[1]);
    append_hash(&mut bytes, &[0]);
    append_hash(&mut bytes, id('M').as_str().as_bytes());
    append_hash(&mut bytes, mapping_hash.as_bytes());
    ContentHash::digest(&bytes)
}

fn append_authorization_time(bytes: &mut Vec<u8>, seconds: i64, local_date: &str) {
    append_hash(bytes, &seconds.to_be_bytes());
    append_hash(bytes, &0_u32.to_be_bytes());
    append_hash(bytes, b"Asia/Shanghai");
    append_hash(bytes, local_date.as_bytes());
}

fn append_hash(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(
        &u64::try_from(value.len())
            .expect("fixture field length fits u64")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(value);
}

fn unit_definition() -> market::MarketDefinition {
    market::MarketDefinition {
        definition: Some(market::market_definition::Definition::Unit(unit_proto())),
    }
}

fn calendar_definition() -> market::MarketDefinition {
    market::MarketDefinition {
        definition: Some(market::market_definition::Definition::Calendar(
            calendar_proto(),
        )),
    }
}

fn instrument_definition() -> market::MarketDefinition {
    market::MarketDefinition {
        definition: Some(market::market_definition::Definition::Instrument(
            market::CompleteInstrumentDefinition {
                instrument: Some(market::Instrument {
                    instrument_id: Some(proto_id('I')),
                    version: 1,
                    owner: Some(owner_proto()),
                    kind: market::InstrumentKind::Other as i32,
                    market: "CGB".to_owned(),
                    symbol: "260011.IB".to_owned(),
                    currency: Some(core::UnitRef {
                        unit_id: Some(proto_id('N')),
                        version: 1,
                    }),
                    calendar: Some(version_ref('C', 1)),
                }),
                subtype: None,
            },
        )),
    }
}

fn unit_proto() -> market::Unit {
    market::Unit {
        unit_id: Some(proto_id('N')),
        version: 1,
        owner: Some(owner_proto()),
        code: "CNY_CLEAN_PRICE".to_owned(),
        dimension: "price".to_owned(),
        scale: 4,
        precision: 12,
    }
}

fn calendar_proto() -> market::Calendar {
    market::Calendar {
        calendar_id: Some(proto_id('C')),
        version: 1,
        owner: Some(owner_proto()),
        market: "CGB".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective_from: Some(market_time(1_767_225_600, "2026-01-01")),
        effective_to: Some(market_time(1_798_761_600, "2027-01-01")),
        sessions: vec![market::CalendarSession {
            local_date: "2026-08-13".to_owned(),
            open_local_time: "09:00:00".to_owned(),
            close_local_time: "17:00:00".to_owned(),
            closed: false,
        }],
    }
}

fn change(reason: &str) -> core::ChangeJustification {
    core::ChangeJustification {
        reason: reason.to_owned(),
        sources: vec![core::SourceDocumentRef {
            uri: "fixture://r6a/production-input-plane".to_owned(),
            sha256: Some(core::Sha256 {
                value: ContentHash::digest(b"r6a production SIT evidence")
                    .as_bytes()
                    .to_vec(),
            }),
        }],
    }
}

fn market_time(seconds: i64, local_date: &str) -> core::MarketTime {
    let mut value = core::MarketTime {
        instant: None,
        market_timezone: "Asia/Shanghai".to_owned(),
        local_trading_date: local_date.to_owned(),
    };
    let instant = value.instant.get_or_insert_default();
    instant.seconds = seconds;
    instant.nanos = 0;
    value
}

fn schema_hash() -> ContentHash {
    let bytes = (0..SCHEMA_HASH_HEX.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&SCHEMA_HASH_HEX[index..index + 2], 16).expect("hex"))
        .collect::<Vec<_>>();
    ContentHash::from_bytes(&bytes).expect("canonical schema hash is 32 bytes")
}

fn sha256(value: &ContentHash) -> core::Sha256 {
    core::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn owner_proto() -> core::OwnerRef {
    core::OwnerRef {
        tenant_id: Some(proto_id('T')),
        owner_id: Some(proto_id('P')),
    }
}

fn version_ref(suffix: char, version: u64) -> core::VersionRef {
    core::VersionRef {
        id: Some(proto_id(suffix)),
        version,
    }
}

fn proto_id(suffix: char) -> core::Ulid {
    core::Ulid {
        value: id(suffix).as_str().to_owned(),
    }
}

fn id(suffix: char) -> Ulid {
    let suffix = if suffix == 'I' { 'J' } else { suffix };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}"))
        .expect("fixture suffix forms a valid ULID")
}

fn server_values(
    address: SocketAddr,
    environment: &IntegrationEnvironment,
    fixture_root: &Path,
) -> BTreeMap<String, String> {
    let mut values = infrastructure_values(address, environment);
    values.extend(input_identity_values(fixture_root));
    values
}

fn infrastructure_values(
    address: SocketAddr,
    environment: &IntegrationEnvironment,
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
            format!("sha256:{}", "ab".repeat(32)),
        ),
        (
            "FICANT_SERVER_ENVIRONMENT_ATTESTATION".to_owned(),
            format!("sha256:{}", "cd".repeat(32)),
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
            id('T').as_str().to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_OWNER_ID".to_owned(),
            id('P').as_str().to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_ACTOR_ID".to_owned(),
            id('A').as_str().to_owned(),
        ),
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
    ])
}

fn input_identity_values(fixture_root: &Path) -> BTreeMap<String, String> {
    let admin_scopes = [
        "definitions:read",
        "definitions:write",
        "data-sources:read",
        "data-sources:write",
        "facts:read",
        "facts:write",
        "snapshots:read",
        "snapshots:write",
        "governance:read",
    ]
    .join(",");
    let researcher_scopes = [
        "definitions:read",
        "data-sources:read",
        "data-sources:import",
        "facts:read",
        "snapshots:read",
    ]
    .join(",");
    BTreeMap::from([
        (
            "FICANT_INPUT_FILE_NDJSON_ROOT".to_owned(),
            fixture_root.to_string_lossy().into_owned(),
        ),
        (
            "FICANT_INPUT_FILE_CONNECTION_BINDING".to_owned(),
            FILE_BINDING.to_owned(),
        ),
        (
            "FICANT_INPUT_POSTGRES_CONNECTION_BINDING".to_owned(),
            POSTGRES_BINDING.to_owned(),
        ),
        (
            "FICANT_BOOTSTRAP_SUBJECT".to_owned(),
            "r6a-admin".to_owned(),
        ),
        (
            "FICANT_BOOTSTRAP_BEARER_TOKEN".to_owned(),
            ADMIN_TOKEN.to_owned(),
        ),
        (
            "FICANT_BOOTSTRAP_ACTOR_ID".to_owned(),
            id('A').as_str().to_owned(),
        ),
        (
            "FICANT_BOOTSTRAP_TENANT_ID".to_owned(),
            id('T').as_str().to_owned(),
        ),
        (
            "FICANT_BOOTSTRAP_ALLOWED_OWNER_IDS".to_owned(),
            id('P').as_str().to_owned(),
        ),
        (
            "FICANT_BOOTSTRAP_ACTIVE_ROLE".to_owned(),
            role_name(PlatformRole::PlatformAdmin).to_owned(),
        ),
        ("FICANT_BOOTSTRAP_SCOPES".to_owned(), admin_scopes),
        (
            "FICANT_LOOPBACK_SUBJECT".to_owned(),
            "r6a-researcher".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_ACTOR_ID".to_owned(),
            id('R').as_str().to_owned(),
        ),
        (
            "FICANT_LOOPBACK_TENANT_ID".to_owned(),
            id('T').as_str().to_owned(),
        ),
        (
            "FICANT_LOOPBACK_ALLOWED_OWNER_IDS".to_owned(),
            id('P').as_str().to_owned(),
        ),
        (
            "FICANT_LOOPBACK_ACTIVE_ROLE".to_owned(),
            role_name(PlatformRole::Researcher).to_owned(),
        ),
        ("FICANT_LOOPBACK_SCOPES".to_owned(), researcher_scopes),
    ])
}

const fn role_name(role: PlatformRole) -> &'static str {
    match role {
        PlatformRole::PlatformAdmin => "PLATFORM_ADMIN",
        PlatformRole::Researcher => "RESEARCHER",
    }
}

fn free_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral listener binds");
    listener.local_addr().expect("listener has an address")
}

async fn wait_until_listening(address: SocketAddr) {
    for _ in 0..200 {
        if TcpStream::connect_timeout(&address, Duration::from_millis(20)).is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("R6A production server did not listen on {address}");
}
