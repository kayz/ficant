use std::collections::BTreeMap;

use ficant_api::DataHealthGrpcService;
use ficant_contracts::ficant::research::v1::{
    GetDataHealthReportRequest, data_health_service_server::DataHealthService,
    get_data_health_report_response,
};
use ficant_server::{
    ServerSettings,
    build_grpc_services_with_experiment_registry_and_positions_and_factors_and_portfolio_risk_and_data_health,
};
use tonic::Request;

const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";

#[tokio::test]
async fn production_composition_exposes_data_health_and_rejects_malformed_input_before_io() {
    fn assert_service<T: DataHealthService>() {}
    assert_service::<DataHealthGrpcService>();

    let settings = ServerSettings::try_from_values(&values()).unwrap();
    let (_, _, _, _, _, _, _, _, service) =
        build_grpc_services_with_experiment_registry_and_positions_and_factors_and_portfolio_risk_and_data_health(
            &settings,
        )
        .unwrap();
    let response = service
        .get_data_health_report(Request::new(GetDataHealthReportRequest::default()))
        .await
        .unwrap()
        .into_inner();
    let Some(get_data_health_report_response::Result::Error(error)) = response.result else {
        panic!("malformed public request must return the typed error arm");
    };
    assert_ne!(error.code, 0);
    assert!(!error.retryable);
}

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
            "FICANT_LOOPBACK_SUBJECT".to_owned(),
            "health-user".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_SCOPES".to_owned(),
            "data-health:read".to_owned(),
        ),
    ])
}
