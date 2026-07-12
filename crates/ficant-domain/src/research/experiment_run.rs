use crate::primitives::{ContentHash, LineageRef, OwnerRef, Ulid, VersionRef};
use crate::{DomainErrorCode, DomainResult, Lineaged};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Created,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExperimentRun {
    experiment_run_id: Ulid,
    owner: OwnerRef,
    data_snapshot: LineageRef,
    universe_snapshot: LineageRef,
    rule_packs: Vec<VersionRef>,
    runtime_image_digest: ContentHash,
    parameters_hash: ContentHash,
    seed: u64,
    state: RunState,
    revision: u64,
    lineage: Vec<LineageRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExperimentRunInput {
    pub experiment_run_id: Ulid,
    pub owner: OwnerRef,
    pub data_snapshot: LineageRef,
    pub universe_snapshot: LineageRef,
    pub rule_packs: Vec<VersionRef>,
    pub runtime_image_digest: ContentHash,
    pub parameters_hash: ContentHash,
    pub seed: u64,
}

impl ExperimentRun {
    pub fn new(input: ExperimentRunInput) -> DomainResult<Self> {
        let ExperimentRunInput {
            experiment_run_id,
            owner,
            data_snapshot,
            universe_snapshot,
            rule_packs,
            runtime_image_digest,
            parameters_hash,
            seed,
        } = input;
        if rule_packs.is_empty() {
            return Err(DomainErrorCode::BrokenLineage);
        }
        let mut lineage = vec![data_snapshot.clone(), universe_snapshot.clone()];
        lineage.extend(
            rule_packs.iter().map(|reference| {
                LineageRef::versioned(reference.id().clone(), reference.version())
            }),
        );
        Ok(Self {
            experiment_run_id,
            owner,
            data_snapshot,
            universe_snapshot,
            rule_packs,
            runtime_image_digest,
            parameters_hash,
            seed,
            state: RunState::Created,
            revision: 1,
            lineage,
        })
    }

    pub fn transition(&self, next: RunState, expected_revision: u64) -> DomainResult<Self> {
        if expected_revision != self.revision {
            return Err(DomainErrorCode::VersionConflict);
        }
        let allowed = matches!(
            (self.state, next),
            (RunState::Created, RunState::Running)
                | (
                    RunState::Running,
                    RunState::Succeeded | RunState::Failed | RunState::Cancelled,
                )
        );
        if !allowed {
            return Err(DomainErrorCode::InvalidStateTransition);
        }
        let mut result = self.clone();
        result.state = next;
        result.revision = result
            .revision
            .checked_add(1)
            .ok_or(DomainErrorCode::VersionConflict)?;
        Ok(result)
    }

    pub fn id(&self) -> &Ulid {
        &self.experiment_run_id
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn data_snapshot(&self) -> &LineageRef {
        &self.data_snapshot
    }

    pub fn universe_snapshot(&self) -> &LineageRef {
        &self.universe_snapshot
    }

    pub fn rule_packs(&self) -> &[VersionRef] {
        &self.rule_packs
    }

    pub fn runtime_image_digest(&self) -> &ContentHash {
        &self.runtime_image_digest
    }

    pub fn parameters_hash(&self) -> &ContentHash {
        &self.parameters_hash
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn state(&self) -> RunState {
        self.state
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

impl Lineaged for ExperimentRun {
    fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}
