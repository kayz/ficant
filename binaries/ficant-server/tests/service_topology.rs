use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use ficant_api::build_production_routes;
use ficant_server::{ServerSettings, build_production_grpc_services};
use prost::Message;
use prost_types::FileDescriptorSet;

const BUF_VERSION: &str = "1.56.0";
const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";
const ARTIFACT_SERVICE: &str = "ficant.research.v1.ArtifactService";

static DESCRIPTOR_SERVICES: OnceLock<BTreeSet<String>> = OnceLock::new();

#[tokio::test]
async fn descriptor_and_production_routes_are_exactly_equal() {
    let descriptor = descriptor_service_names();
    let routes = production_route_names();

    assert_eq!(descriptor.len(), 14, "R6B freezes fourteen public services");
    assert_eq!(
        topology_drift(descriptor, &routes),
        TopologyDrift::default()
    );
}

#[tokio::test]
async fn descriptor_extra_service_fixture_fails_closed() {
    let mut descriptor = descriptor_service_names().clone();
    let extra = "ficant.fixture.v1.UncomposedService".to_owned();
    assert!(descriptor.insert(extra.clone()));

    assert_eq!(
        topology_drift(&descriptor, &production_route_names()),
        TopologyDrift {
            descriptor_only: vec![extra],
            route_only: Vec::new(),
        }
    );
}

#[tokio::test]
async fn route_missing_service_fixture_fails_closed() {
    let mut routes = production_route_names();
    assert!(routes.remove(ARTIFACT_SERVICE));

    assert_eq!(
        topology_drift(descriptor_service_names(), &routes),
        TopologyDrift {
            descriptor_only: vec![ARTIFACT_SERVICE.to_owned()],
            route_only: Vec::new(),
        }
    );
}

#[derive(Debug, Default, PartialEq, Eq)]
struct TopologyDrift {
    descriptor_only: Vec<String>,
    route_only: Vec<String>,
}

fn topology_drift(descriptor: &BTreeSet<String>, routes: &BTreeSet<String>) -> TopologyDrift {
    TopologyDrift {
        descriptor_only: descriptor.difference(routes).cloned().collect(),
        route_only: routes.difference(descriptor).cloned().collect(),
    }
}

fn production_route_names() -> BTreeSet<String> {
    let settings =
        ServerSettings::try_from_values(&settings()).expect("topology fixture settings are valid");
    let services =
        build_production_grpc_services(&settings).expect("all production adapters compose");
    build_production_routes(services)
        .expect("production service names are unique")
        .service_names()
        .clone()
}

fn descriptor_service_names() -> &'static BTreeSet<String> {
    DESCRIPTOR_SERVICES.get_or_init(|| {
        let descriptor = build_descriptor();
        descriptor
            .file
            .iter()
            .flat_map(|file| {
                let package = file.package.as_deref().unwrap_or_default();
                file.service.iter().map(move |service| {
                    let name = service.name.as_deref().expect("service name is required");
                    format!("{package}.{name}")
                })
            })
            .collect()
    })
}

fn build_descriptor() -> FileDescriptorSet {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("server crate must remain two levels below the repository root");
    let buf = std::env::var_os("FICANT_BUF").map_or_else(|| PathBuf::from("buf"), PathBuf::from);
    let version = Command::new(&buf)
        .arg("--version")
        .output()
        .expect("fixed Buf binary must be executable");
    assert!(version.status.success(), "fixed Buf version check failed");
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        BUF_VERSION,
        "topology gate requires the Delivery-pinned Buf version"
    );

    let output_path = std::env::temp_dir().join(format!(
        "ficant-r6b-topology-descriptor-{}.bin",
        std::process::id()
    ));
    let _ = fs::remove_file(&output_path);
    let output = Command::new(&buf)
        .args(["build", "interface", "--as-file-descriptor-set", "-o"])
        .arg(&output_path)
        .current_dir(repository)
        .output()
        .expect("fixed Buf binary must build the descriptor");
    assert!(
        output.status.success(),
        "descriptor build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(&output_path).expect("Buf must write the descriptor");
    fs::remove_file(&output_path).expect("topology gate must clean its temporary descriptor");
    FileDescriptorSet::decode(bytes.as_slice()).expect("descriptor must decode")
}

#[allow(clippy::too_many_lines)]
fn settings() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("FICANT_GRPC_BIND".to_owned(), "127.0.0.1:50051".to_owned()),
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
            "postgres://ficant:fixture@127.0.0.1:5432/ficant".to_owned(),
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
            "topology-user".to_owned(),
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
            "RESEARCHER".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_SCOPES".to_owned(),
            "artifacts:read,rates:read".to_owned(),
        ),
    ])
}
