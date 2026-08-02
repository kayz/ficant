use ficant_api::PositionSnapshotGrpcService;
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::research::v1 as research;
use ficant_contracts::ficant::research::v1::position_snapshot_service_server::PositionSnapshotService;
use prost::Message;

#[test]
fn position_snapshot_adapter_implements_the_frozen_grpc_service_contract() {
    fn assert_service<T: PositionSnapshotService>() {}
    assert_service::<PositionSnapshotGrpcService>();
}

#[test]
fn position_and_capital_outputs_round_trip_the_same_complete_coverage_shape() {
    let coverage = research::CoverageDeclaration {
        imported_position_count: 3,
        participating_position_count: 3,
        imported_gross_economic_value_by_unit: vec![money("1250")],
        participating_gross_economic_value_by_unit: vec![money("1250")],
        missing_critical_field_record_count: 0,
        source_confidence: None,
        distinct_external_data_source_version_count: 0,
    };
    let views = research::PositionViews {
        coverage: Some(coverage.clone()),
        ..Default::default()
    };
    let decoded_views = research::PositionViews::decode(views.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded_views.coverage, Some(coverage.clone()));

    let capital = research::CapitalUse {
        coverage: Some(coverage.clone()),
        ..Default::default()
    };
    let decoded_capital = research::CapitalUse::decode(capital.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded_capital.coverage, Some(coverage));
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
