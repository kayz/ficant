use async_trait::async_trait;
use ficant_domain::research::{
    CurveNodeDefinition, FactorDefinition, FactorTarget, FactorTargetBinding,
};

use super::{AccessScope, ApplicationResult, IdempotencyKey};

/// Persistence boundary for global immutable Factor topology.
#[async_trait]
pub trait FactorTopologyRepository: Send + Sync {
    async fn register_factor_definition(
        &self,
        scope: &AccessScope,
        definition: FactorDefinition,
        key: IdempotencyKey,
    ) -> ApplicationResult<FactorDefinition>;

    async fn register_curve_node_definition(
        &self,
        scope: &AccessScope,
        definition: CurveNodeDefinition,
        key: IdempotencyKey,
    ) -> ApplicationResult<CurveNodeDefinition>;

    async fn bind_factor_target(
        &self,
        scope: &AccessScope,
        binding: FactorTargetBinding,
        key: IdempotencyKey,
    ) -> ApplicationResult<FactorTargetBinding>;

    async fn get_factor_definition(
        &self,
        factor_id: &str,
    ) -> ApplicationResult<Option<FactorDefinition>>;

    async fn get_factor_targets(
        &self,
        scope: &AccessScope,
        factor_id: &str,
    ) -> ApplicationResult<Vec<FactorTargetBinding>>;

    async fn get_target_factors(
        &self,
        scope: &AccessScope,
        target: &FactorTarget,
    ) -> ApplicationResult<Vec<FactorDefinition>>;

    async fn exact_target_exists(&self, target: &FactorTarget) -> ApplicationResult<bool>;
}
