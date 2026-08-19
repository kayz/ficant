use std::collections::BTreeMap;

use ficant_contracts::ficant::rates::v1::{
    AnalyzeBondRequest, AnalyzeCarryRollRequest, AnalyzeFuturesDeliveryRequest,
    AnalyzeFuturesHedgeRequest, InterpolateYieldCurveRequest, analyze_bond_response,
    analyze_carry_roll_response, analyze_futures_delivery_response, analyze_futures_hedge_response,
    interpolate_yield_curve_response, rates_analytics_service_server::RatesAnalyticsService,
};
use ficant_server::{ServerSettings, build_grpc_services};
use tonic::Request;

const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";

#[tokio::test]
async fn production_rates_composition_fail_closes_all_five_rpcs_before_repository_io() {
    let settings = ServerSettings::try_from_values(&values()).expect("fixture settings are valid");
    let (_, service) = build_grpc_services(&settings).expect("production Rates service composes");

    let response = service
        .analyze_bond(Request::new(AnalyzeBondRequest::default()))
        .await
        .expect("typed business failures stay in the response")
        .into_inner();
    let Some(analyze_bond_response::Result::Error(error)) = response.result else {
        panic!("malformed Bond request must fail closed");
    };
    assert_business_validation(error.code, error.retryable);

    let response = service
        .interpolate_yield_curve(Request::new(InterpolateYieldCurveRequest::default()))
        .await
        .expect("typed business failures stay in the response")
        .into_inner();
    let Some(interpolate_yield_curve_response::Result::Error(error)) = response.result else {
        panic!("malformed Curve request must fail closed");
    };
    assert_business_validation(error.code, error.retryable);

    let response = service
        .analyze_carry_roll(Request::new(AnalyzeCarryRollRequest::default()))
        .await
        .expect("typed business failures stay in the response")
        .into_inner();
    let Some(analyze_carry_roll_response::Result::Error(error)) = response.result else {
        panic!("malformed Carry request must fail closed");
    };
    assert_business_validation(error.code, error.retryable);

    let response = service
        .analyze_futures_delivery(Request::new(AnalyzeFuturesDeliveryRequest::default()))
        .await
        .expect("typed business failures stay in the response")
        .into_inner();
    let Some(analyze_futures_delivery_response::Result::Error(error)) = response.result else {
        panic!("malformed Delivery request must fail closed");
    };
    assert_business_validation(error.code, error.retryable);

    let response = service
        .analyze_futures_hedge(Request::new(AnalyzeFuturesHedgeRequest::default()))
        .await
        .expect("typed business failures stay in the response")
        .into_inner();
    let Some(analyze_futures_hedge_response::Result::Error(error)) = response.result else {
        panic!("malformed Hedge request must fail closed");
    };
    assert_business_validation(error.code, error.retryable);
}

fn assert_business_validation(code: i32, retryable: bool) {
    assert_ne!(
        code, 0,
        "validation failure must have a stable non-success code"
    );
    assert!(!retryable, "invalid exact bindings must not be retried");
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
            "RESEARCHER".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_SCOPES".to_owned(),
            "rates:analyze".to_owned(),
        ),
    ])
}
