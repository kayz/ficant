use ficant_api::{CanonicalCurvePointSetDecoder, PortfolioRiskGrpcService};
use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::CurvePointSetDecoder;
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as market;
use ficant_contracts::ficant::research::v1 as research;
use ficant_contracts::ficant::research::v1::portfolio_risk_service_server::PortfolioRiskService;
use prost::Message;

#[test]
fn portfolio_risk_service_implements_the_generated_contract() {
    fn assert_service<T: PortfolioRiskService>() {}
    assert_service::<PortfolioRiskGrpcService>();
}

#[test]
fn curve_point_decoder_rejects_unknown_fields_and_noncanonical_bytes() {
    let payload = market::CurvePointSet {
        curve_family_id: "cn.gov.yield-curve".to_owned(),
        points: vec![
            point("cn.gov.yield-curve.05y", 5),
            point("cn.gov.yield-curve.10y", 10),
        ],
    }
    .encode_to_vec();
    let decoder = CanonicalCurvePointSetDecoder;
    let point_set = decoder.decode_canonical(&payload).unwrap();
    assert_eq!(point_set.points().len(), 2);

    let mut unknown_field = payload;
    unknown_field.extend_from_slice(&[0x18, 0x01]);
    let error = decoder.decode_canonical(&unknown_field).unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);
}

#[test]
fn futures_snapshot_binding_round_trips_on_request_and_result_contracts() {
    let snapshot = core::Ulid {
        value: "01ARZ3NDEKTSV4RRFFQ69G5F0D".to_owned(),
    };
    let request = research::CalculateKeyRateDv01Request {
        futures_data_snapshot_id: Some(snapshot.clone()),
        ..Default::default()
    };
    let decoded =
        research::CalculateKeyRateDv01Request::decode(request.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.futures_data_snapshot_id, Some(snapshot.clone()));

    let exposure = research::PortfolioKeyRateExposure {
        futures_data_snapshot_id: Some(snapshot.clone()),
        source_confidence: Some(research::PriceSourceSummary {
            counts: vec![research::PriceSourceCount {
                source_type: market::PriceSourceType::ActiveQuote as i32,
                record_count: 3,
            }],
            mixed: false,
        }),
        coverage: Some(research::CoverageDeclaration {
            imported_position_count: 2,
            participating_position_count: 1,
            imported_gross_economic_value_by_unit: vec![money("200")],
            participating_gross_economic_value_by_unit: vec![money("80")],
            missing_critical_field_record_count: 0,
            source_confidence: Some(research::PriceSourceSummary {
                counts: vec![research::PriceSourceCount {
                    source_type: market::PriceSourceType::ActiveQuote as i32,
                    record_count: 3,
                }],
                mixed: false,
            }),
            distinct_external_data_source_version_count: 1,
        }),
        ..Default::default()
    };
    let decoded =
        research::PortfolioKeyRateExposure::decode(exposure.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.futures_data_snapshot_id, Some(snapshot));
    let source_confidence = decoded.source_confidence.unwrap();
    assert_eq!(
        source_confidence.counts[0].source_type,
        market::PriceSourceType::ActiveQuote as i32
    );
    let coverage = decoded.coverage.unwrap();
    assert_eq!(coverage.imported_position_count, 2);
    assert_eq!(coverage.participating_position_count, 1);
    assert_eq!(
        coverage.imported_gross_economic_value_by_unit[0],
        money("200")
    );
    assert_eq!(
        coverage.participating_gross_economic_value_by_unit[0],
        money("80")
    );
    assert_eq!(coverage.source_confidence.unwrap(), source_confidence);
    assert_eq!(coverage.distinct_external_data_source_version_count, 1);
}

fn money(coefficient: &str) -> core::DecimalValue {
    core::DecimalValue {
        coefficient: coefficient.to_owned(),
        scale: 0,
        unit: Some(core::UnitRef {
            unit_id: Some(core::Ulid {
                value: "01ARZ3NDEKTSV4RRFFQ69G5F0V".to_owned(),
            }),
            version: 1,
        }),
    }
}

fn point(id: &str, coefficient: i32) -> market::CurvePoint {
    market::CurvePoint {
        curve_node_id: id.to_owned(),
        curve_node_content_hash: Some(core::Sha256 {
            value: vec![u8::try_from(coefficient).unwrap(); 32],
        }),
        yield_to_maturity: Some(core::DecimalValue {
            coefficient: coefficient.to_string(),
            scale: 3,
            unit: Some(core::UnitRef {
                unit_id: Some(core::Ulid {
                    value: "01ARZ3NDEKTSV4RRFFQ69G5F0V".to_owned(),
                }),
                version: 1,
            }),
        }),
    }
}
