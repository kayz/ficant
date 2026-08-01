use ficant_domain::research::{
    CurveNodeDefinition, FactorDefinition, FactorTarget, FactorTargetBinding,
};

use crate::ports::{AccessScope, ApplicationResult, FactorTopologyRepository, IdempotencyKey};
use crate::{ApplicationError, ApplicationErrorCategory};

pub struct FactorTopologyUseCase<'a> {
    repository: &'a dyn FactorTopologyRepository,
}

impl<'a> FactorTopologyUseCase<'a> {
    #[must_use]
    pub fn new(repository: &'a dyn FactorTopologyRepository) -> Self {
        Self { repository }
    }

    /// Registers the globally immutable definition.
    ///
    /// # Errors
    ///
    /// Returns the repository error when persistence fails or the id already
    /// names different canonical content.
    pub async fn register_factor_definition(
        &self,
        scope: &AccessScope,
        definition: FactorDefinition,
        key: IdempotencyKey,
    ) -> ApplicationResult<FactorDefinition> {
        self.repository
            .register_factor_definition(scope, definition, key)
            .await
    }

    /// Registers a stable curve-node definition.
    ///
    /// # Errors
    ///
    /// Returns the repository error when persistence fails or the node id
    /// already names different canonical content.
    pub async fn register_curve_node_definition(
        &self,
        scope: &AccessScope,
        definition: CurveNodeDefinition,
        key: IdempotencyKey,
    ) -> ApplicationResult<CurveNodeDefinition> {
        self.repository
            .register_curve_node_definition(scope, definition, key)
            .await
    }

    /// Binds an exact, verified target to a factor.
    ///
    /// # Errors
    ///
    /// Returns an authorization, not-found, validation, integrity, or
    /// persistence error without creating a partial binding.
    pub async fn bind_factor_target(
        &self,
        scope: &AccessScope,
        binding: FactorTargetBinding,
        key: IdempotencyKey,
    ) -> ApplicationResult<FactorTargetBinding> {
        if let FactorTarget::Instrument(target) = binding.target() {
            scope.authorize(target.owner())?;
        }
        if !self
            .repository
            .exact_target_exists(binding.target())
            .await?
        {
            return Err(not_found());
        }
        self.repository
            .bind_factor_target(scope, binding, key)
            .await
    }

    /// Reads one global factor definition.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when the id is absent, or the repository error when
    /// the read cannot be completed.
    pub async fn get_factor_definition(
        &self,
        factor_id: &str,
    ) -> ApplicationResult<FactorDefinition> {
        self.repository
            .get_factor_definition(factor_id)
            .await?
            .ok_or_else(not_found)
    }

    /// Returns all exact targets bound to a factor in stable order.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for an empty topology, or an authorization,
    /// integrity, or repository error instead of a partial result.
    pub async fn get_factor_targets(
        &self,
        scope: &AccessScope,
        factor_id: &str,
    ) -> ApplicationResult<Vec<FactorTargetBinding>> {
        let values = self.repository.get_factor_targets(scope, factor_id).await?;
        if values.is_empty() {
            return Err(not_found());
        }
        Ok(values)
    }

    /// Returns all factor definitions bound to an exact target.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for an unbound target, or an authorization,
    /// integrity, or repository error instead of a partial result.
    pub async fn get_target_factors(
        &self,
        scope: &AccessScope,
        target: &FactorTarget,
    ) -> ApplicationResult<Vec<FactorDefinition>> {
        if let FactorTarget::Instrument(value) = target {
            scope.authorize(value.owner())?;
        }
        let values = self.repository.get_target_factors(scope, target).await?;
        if values.is_empty() {
            return Err(not_found());
        }
        Ok(values)
    }
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}
