use ficant_api::PositionSnapshotGrpcService;
use ficant_contracts::ficant::research::v1::position_snapshot_service_server::{
    PositionSnapshotService, PositionSnapshotServiceServer,
};
use tonic::server::NamedService;

#[test]
fn position_snapshot_service_is_a_distinct_public_grpc_route() {
    fn assert_service<T: PositionSnapshotService>() {}
    assert_service::<PositionSnapshotGrpcService>();
    assert_eq!(
        <PositionSnapshotServiceServer<PositionSnapshotGrpcService> as NamedService>::NAME,
        "ficant.research.v1.PositionSnapshotService"
    );
}
