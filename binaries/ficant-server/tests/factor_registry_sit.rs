use ficant_api::FactorRegistryGrpcService;
use ficant_contracts::ficant::research::v1::factor_registry_service_server::FactorRegistryService;
use ficant_server::{
    ServerSettings, build_grpc_services_with_experiment_registry_and_positions_and_factors,
};
use std::collections::BTreeMap;

const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";

#[tokio::test]
async fn production_composition_includes_the_factor_registry_service() {
    fn assert_service<T: FactorRegistryService>() {}
    assert_service::<FactorRegistryGrpcService>();
    let settings = ServerSettings::try_from_values(&values()).expect("settings are valid");
    let (_, _, _, _, _, factors) =
        build_grpc_services_with_experiment_registry_and_positions_and_factors(&settings)
            .expect("factor registry composes with the production repository");
    drop(factors);
}

#[allow(clippy::too_many_lines)]
fn values() -> BTreeMap<String, String> {
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
            format!(
                "sha256:{}",
                hash_hex(&ficant_native_nodes::native_node_source_digest())
            ),
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
            "factor-user".to_owned(),
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
            "factors:read,factors:write".to_owned(),
        ),
    ])
}

fn hash_hex(value: &ficant_domain::primitives::ContentHash) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            use std::fmt::Write as _;
            write!(output, "{byte:02x}").unwrap();
            output
        })
}
