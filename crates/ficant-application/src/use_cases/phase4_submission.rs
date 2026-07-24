use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, VersionRef};
use ficant_domain::research::{ExperimentRun, ExperimentRunInput, ResearchGraph};

use crate::ports::{
    AccessScope, ApplicationResult, ExecutionInstanceIdentity, ExternalInputArtifactBinding,
    IdempotencyKey, Phase4ExecutionRepository, StoredExecutionIdentity, SubmitGraphRun,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

/// Required, already verified material for one atomic graph submission.
pub struct PreparedGraphSubmission {
    pub idempotency_key: String,
    pub scope: AccessScope,
    pub owner: OwnerRef,
    pub run_id: Ulid,
    pub graph: ResearchGraph,
    pub data_snapshot: ficant_domain::primitives::LineageRef,
    pub universe_snapshot: ficant_domain::primitives::LineageRef,
    pub rule_packs: Vec<VersionRef>,
    pub runtime_image_digest: ContentHash,
    pub parameters_hash: ContentHash,
    pub seed: u64,
    pub execution: ExecutionInstanceIdentity,
    pub external_input_artifacts: Vec<ExternalInputArtifactBinding>,
}

/// Constructs the complete Phase 4 aggregate and delegates its only write to the atomic repository
/// boundary.
pub struct Phase4Submission<'a> {
    repository: &'a dyn Phase4ExecutionRepository,
}

impl<'a> Phase4Submission<'a> {
    #[must_use]
    pub fn new(repository: &'a dyn Phase4ExecutionRepository) -> Self {
        Self { repository }
    }

    /// Submits a fully verified graph run.
    ///
    /// # Errors
    ///
    /// Returns a validation or lineage error before storage, or the repository's atomic
    /// idempotency/storage result.
    pub async fn submit(
        &self,
        prepared: PreparedGraphSubmission,
    ) -> ApplicationResult<crate::ports::GraphRunRecord> {
        let idempotency_key = IdempotencyKey::new(prepared.idempotency_key)?;
        prepared.scope.authorize(&prepared.owner)?;
        if prepared.graph.owner() != &prepared.owner
            || prepared.execution.run_id() != &prepared.run_id
            || prepared.execution.reproducibility().graph_digest() != prepared.graph.digest()
        {
            return Err(lineage());
        }
        let run = ExperimentRun::new(ExperimentRunInput {
            experiment_run_id: prepared.run_id.clone(),
            owner: prepared.owner.clone(),
            data_snapshot: prepared.data_snapshot,
            universe_snapshot: prepared.universe_snapshot,
            rule_packs: prepared.rule_packs,
            runtime_image_digest: prepared.runtime_image_digest,
            parameters_hash: prepared.parameters_hash,
            seed: prepared.seed,
        })
        .map_err(map_domain_error)?;
        let identity = StoredExecutionIdentity {
            owner: prepared.owner,
            graph_id: prepared.graph.graph_id().clone(),
            graph_version: prepared.graph.version(),
            identity: prepared.execution,
            external_input_artifacts: prepared.external_input_artifacts,
        };
        self.repository
            .submit_graph_run(SubmitGraphRun {
                scope: prepared.scope,
                idempotency_key,
                run,
                graph: prepared.graph,
                identity,
            })
            .await
    }
}

fn lineage() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}
