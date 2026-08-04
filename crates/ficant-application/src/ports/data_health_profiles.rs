use async_trait::async_trait;
use ficant_domain::primitives::{MarketTime, OwnerRef, VersionRef};
use ficant_domain::research::DataHealthThresholdProfile;

use super::{AccessScope, ApplicationResult};

#[async_trait]
pub trait DataHealthThresholdProfileRepository: Send + Sync {
    async fn get_exact(
        &self,
        scope: &AccessScope,
        profile_ref: VersionRef,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<Option<DataHealthThresholdProfile>>;

    async fn resolve_active(
        &self,
        scope: &AccessScope,
        owner: OwnerRef,
        evaluated_at: MarketTime,
    ) -> ApplicationResult<Option<DataHealthThresholdProfile>>;
}
