use ficant_api::PositionSnapshotGrpcService;
use ficant_contracts::ficant::research::v1::position_snapshot_service_server::PositionSnapshotService;

#[test]
fn position_snapshot_adapter_implements_the_frozen_grpc_service_contract() {
    fn assert_service<T: PositionSnapshotService>() {}
    assert_service::<PositionSnapshotGrpcService>();
}
