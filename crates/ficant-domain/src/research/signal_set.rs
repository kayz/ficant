use crate::primitives::{ContentHash, EffectivePeriod, LineageRef, OwnerRef, Ulid, VersionRef};
use crate::{ContentAddressed, DomainErrorCode, DomainResult, Lineaged};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalSet {
    signal_set_id: Ulid,
    owner: OwnerRef,
    artifact: LineageRef,
    experiment_run_id: Ulid,
    data_snapshot: LineageRef,
    universe_snapshot: LineageRef,
    rule_packs: Vec<VersionRef>,
    input_artifacts: Vec<LineageRef>,
    valid: EffectivePeriod,
    lineage: Vec<LineageRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalSetInput {
    pub signal_set_id: Ulid,
    pub owner: OwnerRef,
    pub artifact: LineageRef,
    pub experiment_run_id: Ulid,
    pub data_snapshot: LineageRef,
    pub universe_snapshot: LineageRef,
    pub rule_packs: Vec<VersionRef>,
    pub input_artifacts: Vec<LineageRef>,
    pub valid: EffectivePeriod,
}

impl SignalSet {
    pub fn new(input: SignalSetInput) -> DomainResult<Self> {
        let SignalSetInput {
            signal_set_id,
            owner,
            artifact,
            experiment_run_id,
            data_snapshot,
            universe_snapshot,
            rule_packs,
            input_artifacts,
            valid,
        } = input;
        if artifact.object_id() == &signal_set_id
            || artifact.version().is_some()
            || artifact.content_hash().is_none()
        {
            return Err(DomainErrorCode::BrokenLineage);
        }
        if rule_packs.is_empty() || input_artifacts.is_empty() {
            return Err(DomainErrorCode::BrokenLineage);
        }
        let mut lineage = vec![
            artifact.clone(),
            data_snapshot.clone(),
            universe_snapshot.clone(),
        ];
        lineage.extend(
            rule_packs.iter().map(|reference| {
                LineageRef::versioned(reference.id().clone(), reference.version())
            }),
        );
        lineage.extend(input_artifacts.iter().cloned());
        Ok(Self {
            signal_set_id,
            owner,
            artifact,
            experiment_run_id,
            data_snapshot,
            universe_snapshot,
            rule_packs,
            input_artifacts,
            valid,
            lineage,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.signal_set_id
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn artifact(&self) -> &LineageRef {
        &self.artifact
    }

    pub fn experiment_run_id(&self) -> &Ulid {
        &self.experiment_run_id
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

    pub fn input_artifacts(&self) -> &[LineageRef] {
        &self.input_artifacts
    }

    pub fn valid(&self) -> &EffectivePeriod {
        &self.valid
    }
}

impl ContentAddressed for SignalSet {
    fn content_hash(&self) -> &ContentHash {
        self.artifact
            .content_hash()
            .expect("SignalSet construction guarantees artifact content hash")
    }
}

impl Lineaged for SignalSet {
    fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}
