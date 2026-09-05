use ficant_contracts::ficant::app::v1::{
    GetAppRegistryRequest, GetCurrentSessionRequest, get_app_registry_response,
    get_current_session_response, platform_service_server::PlatformService,
};
use ficant_server::{ServerSettings, build_platform_service};
use std::collections::BTreeMap;
use tonic::Request;

const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";

#[allow(clippy::too_many_lines)]
fn values(bind: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("FICANT_GRPC_BIND".to_owned(), bind.to_owned()),
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
            format!("sha256:{}", "02".repeat(32)),
        ),
        (
            "FICANT_INPUT_FILE_NDJSON_ROOT".to_owned(),
            std::env::temp_dir()
                .join("ficant-composition-unused-input")
                .to_string_lossy()
                .into_owned(),
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
            "RESEARCHER".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_SCOPES".to_owned(),
            "portfolio:read,positions:read,rates:analyze,facts:read,definitions:read,artifacts:read"
                .to_owned(),
        ),
    ])
}

#[test]
fn implicit_identity_is_rejected_on_non_loopback_bind() {
    let error = ServerSettings::try_from_values(&values("0.0.0.0:50051"))
        .expect_err("implicit identity on a non-loopback listener must fail closed");
    assert!(error.to_string().contains("loopback"));
}

#[test]
fn debug_output_never_contains_signing_or_trace_key() {
    let settings = ServerSettings::try_from_values(&values("127.0.0.1:50051"))
        .expect("loopback settings are valid");
    let debug = format!("{settings:?}");
    assert!(!debug.contains(KEY));
    assert!(debug.contains("[REDACTED]"));
}

#[tokio::test]
async fn production_composition_has_real_session_and_empty_registry_without_fake_app() {
    let settings = ServerSettings::try_from_values(&values("127.0.0.1:50051"))
        .expect("loopback settings are valid");
    let service = build_platform_service(&settings).expect("service composes");

    let session = service
        .get_current_session(Request::new(GetCurrentSessionRequest {}))
        .await
        .expect("transport succeeds")
        .into_inner();
    assert!(matches!(
        session.result,
        Some(get_current_session_response::Result::Session(_))
    ));

    let registry = service
        .get_app_registry(Request::new(GetAppRegistryRequest {}))
        .await
        .expect("transport succeeds")
        .into_inner();
    let Some(get_app_registry_response::Result::Registry(registry)) = registry.result else {
        panic!("configured loopback subject receives registry");
    };
    assert!(
        registry.apps.is_empty(),
        "production composition must not install fixture apps"
    );
}
