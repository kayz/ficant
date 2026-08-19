use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use ficant_api::{GrpcWebServerConfig, build_production_routes, serve_production_routes};
use ficant_contracts::ficant::market::v1::{
    GetDataSourceRequest, data_source_registry_service_client::DataSourceRegistryServiceClient,
    get_data_source_response,
};
use ficant_server::{ServerSettings, build_production_grpc_services};
use tonic::Request;

const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";

#[tokio::test(flavor = "multi_thread")]
async fn production_composition_exposes_the_real_data_source_registry() {
    let address = free_address();
    let settings = ServerSettings::try_from_values(&values(address)).unwrap();
    let services = build_production_grpc_services(&settings).unwrap();
    let routes = build_production_routes(services).unwrap();
    let server = tokio::spawn(serve_production_routes(
        GrpcWebServerConfig {
            bind: address,
            allowed_origins: vec!["http://127.0.0.1:4174".to_owned()],
        },
        routes,
    ));
    wait_until_listening(address).await;

    let mut client = DataSourceRegistryServiceClient::connect(format!("http://{address}"))
        .await
        .unwrap();
    let response = client
        .get_data_source(Request::new(GetDataSourceRequest::default()))
        .await
        .unwrap()
        .into_inner();
    let Some(get_data_source_response::Result::Error(error)) = response.result else {
        panic!("malformed public request must return the typed error arm before repository I/O");
    };
    assert_ne!(error.code, 0);
    assert!(!error.retryable);
    server.abort();
}

fn free_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral listener binds");
    listener.local_addr().expect("listener has address")
}

async fn wait_until_listening(address: SocketAddr) {
    for _ in 0..100 {
        if TcpStream::connect_timeout(&address, Duration::from_millis(20)).is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("gRPC-Web server did not listen on {address}");
}

#[allow(clippy::too_many_lines)]
fn values(address: SocketAddr) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("FICANT_GRPC_BIND".to_owned(), address.to_string()),
        (
            "FICANT_GRPC_WEB_ALLOWED_ORIGINS".to_owned(),
            "http://127.0.0.1:4174".to_owned(),
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
            "postgres://ficant:secret@127.0.0.1:5432/ficant".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_S3_ENDPOINT".to_owned(),
            "http://127.0.0.1:9000".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_S3_BUCKET".to_owned(),
            "ficant".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_S3_ACCESS_KEY".to_owned(),
            "fixture-access".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_S3_SECRET_KEY".to_owned(),
            "fixture-secret".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_CURSOR_KEY_HEX".to_owned(),
            KEY.to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_TENANT_ID".to_owned(),
            "0000000000000000000000000T".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_OWNER_ID".to_owned(),
            "0000000000000000000000000B".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_ACTOR_ID".to_owned(),
            "0000000000000000000000000A".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_RUNTIME_IMAGE_DIGEST".to_owned(),
            format!("sha256:{}", "01".repeat(32)),
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
            "C:\\ficant-input".to_owned(),
        ),
        (
            "FICANT_INPUT_FILE_CONNECTION_BINDING".to_owned(),
            "fixture-file".to_owned(),
        ),
        (
            "FICANT_INPUT_POSTGRES_CONNECTION_BINDING".to_owned(),
            "fixture-postgres".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_SUBJECT".to_owned(),
            "browser-user".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_ACTOR_ID".to_owned(),
            "0000000000000000000000000A".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_TENANT_ID".to_owned(),
            "0000000000000000000000000T".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_ALLOWED_OWNER_IDS".to_owned(),
            "0000000000000000000000000B".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_ACTIVE_ROLE".to_owned(),
            "PLATFORM_ADMIN".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_SCOPES".to_owned(),
            "rates:analyze,data-sources:read,data-sources:write".to_owned(),
        ),
    ])
}
