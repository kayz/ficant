use async_trait::async_trait;
use ficant_domain::primitives::{MarketTime, Ulid, VersionRef};
use ficant_domain::research::PositionSnapshot;

use super::{AccessScope, ApplicationResult};

#[async_trait]
pub trait PositionSnapshotRepository: Send + Sync {
    async fn get_position_snapshot(
        &self,
        scope: &AccessScope,
        snapshot_id: Ulid,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>>;

    async fn resolve_position_snapshot(
        &self,
        scope: &AccessScope,
        subject_ref: VersionRef,
        observed_at: MarketTime,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>>;
}
