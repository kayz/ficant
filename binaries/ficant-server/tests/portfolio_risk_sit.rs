use std::collections::BTreeMap;

use ficant_application::CalculateFuturesDeliveryBasket;
use ficant_application::ports::{FuturesDeliveryEngine, FuturesDeliveryRuleParser};
use ficant_cgb_futures_pack::{CgbFuturesDeliveryRulePackParser, TYPE_URL};
use ficant_contracts::ficant::research::v1::{
    CalculateKeyRateDv01Request, calculate_key_rate_dv01_response,
    portfolio_risk_service_server::PortfolioRiskService,
};
use ficant_domain::analytics::{
    AnalyticsObjectRef, BondTerms, BusinessDayConvention, CouponFrequency, DayCountConvention,
    FixedDecimal,
};
use ficant_domain::futures_delivery::{CgbFuturesProduct, FuturesDeliverableInput};
use ficant_domain::market::RulePackContent;
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeFuturesDeliveryEngine;
use ficant_server::{
    ServerSettings,
    build_grpc_services_with_experiment_registry_and_positions_and_factors_and_portfolio_risk,
};
use tonic::Request;

const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";

#[tokio::test]
async fn production_composition_exposes_portfolio_risk_and_rejects_malformed_input_before_io() {
    let settings = ServerSettings::try_from_values(&values()).unwrap();
    let (_, _, _, _, _, _, service, _) =
        build_grpc_services_with_experiment_registry_and_positions_and_factors_and_portfolio_risk(
            &settings,
        )
        .unwrap();
    let response = service
        .calculate_key_rate_dv01(Request::new(CalculateKeyRateDv01Request::default()))
        .await
        .unwrap()
        .into_inner();
    let Some(calculate_key_rate_dv01_response::Result::Error(error)) = response.result else {
        panic!("malformed public request must return the typed error arm");
    };
    assert_ne!(error.code, 0);
    assert!(!error.retryable);
}

#[test]
fn production_ctd_and_conversion_factor_are_funding_neutral() {
    let zero = delivery_inputs(FixedDecimal::ZERO);
    let funded = delivery_inputs(FixedDecimal::from_scaled(50_000_000_000));
    let engine = NativeFuturesDeliveryEngine;
    for (zero_input, funded_input) in zero.iter().zip(&funded) {
        let zero_result = engine.calculate(zero_input).unwrap();
        let funded_result = engine.calculate(funded_input).unwrap();
        assert_eq!(
            zero_result.measures().conversion_factor(),
            funded_result.measures().conversion_factor()
        );
        assert_eq!(
            zero_result.measures().implied_repo_rate(),
            funded_result.measures().implied_repo_rate()
        );
        assert_ne!(
            zero_result.measures().financing_cost(),
            funded_result.measures().financing_cost(),
            "the contrast must genuinely alter a funding-dependent measure"
        );
    }
    let zero_basket = CalculateFuturesDeliveryBasket::new(&engine)
        .execute(&zero)
        .unwrap();
    let funded_basket = CalculateFuturesDeliveryBasket::new(&engine)
        .execute(&funded)
        .unwrap();
    assert_eq!(
        zero_basket.ctd().input().bond().version_ref(),
        funded_basket.ctd().input().bond().version_ref()
    );
    assert_eq!(
        zero_basket.ctd().measures().conversion_factor(),
        funded_basket.ctd().measures().conversion_factor()
    );
}

fn delivery_inputs(financing_rate: FixedDecimal) -> Vec<FuturesDeliverableInput> {
    let content = RulePackContent::new(
        TYPE_URL,
        include_bytes!("../../../domain-packs/cgb-futures/cgb-futures-v2.bin").to_vec(),
    )
    .unwrap();
    let rule = CgbFuturesDeliveryRulePackParser
        .parse_for_portfolio_risk(&content, CgbFuturesProduct::TenYear)
        .unwrap();
    [('D', 101_250_000_000_000), ('G', 100_250_000_000_000)]
        .into_iter()
        .map(|(suffix, spot_clean_price)| {
            FuturesDeliverableInput::new(
                OwnerRef::new(id('A'), id('B')),
                object('C'),
                object(suffix),
                object('E'),
                object('F'),
                MarketTime::new(
                    "2026-07-20T04:00:00Z".parse().unwrap(),
                    "Asia/Shanghai",
                    "2026-07-20".parse().unwrap(),
                )
                .unwrap(),
                "2026-07-21".parse().unwrap(),
                "2026-09-01".parse().unwrap(),
                "2026-09-18".parse().unwrap(),
                CgbFuturesProduct::TenYear,
                rule.clone(),
                BondTerms::new(
                    "2024-08-15".parse().unwrap(),
                    "2034-08-15".parse().unwrap(),
                    CouponFrequency::Semiannual,
                    DayCountConvention::ActActBondIsma,
                    BusinessDayConvention::Following,
                    FixedDecimal::from_scaled(25_000_000_000),
                    FixedDecimal::from_scaled(100_000_000_000_000),
                )
                .unwrap(),
                FixedDecimal::from_scaled(spot_clean_price),
                FixedDecimal::from_scaled(99_500_000_000_000),
                financing_rate,
            )
            .unwrap()
        })
        .collect()
}

fn object(suffix: char) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(id(suffix), Version::new(1).unwrap()),
        ContentHash::digest(suffix.to_string().as_bytes()),
    )
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
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
