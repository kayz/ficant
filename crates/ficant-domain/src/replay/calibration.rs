//! Calibration records carried by a calibrated `PolicyArtifact` (ADR-0028; AC43).

use std::collections::BTreeMap;

use super::ReplayAmount;
use crate::primitives::{ContentHash, EffectivePeriod};
use crate::{DomainErrorCode, DomainResult};

/// Most candidate combinations one calibration may evaluate.
pub const MAX_CALIBRATION_CANDIDATES: usize = 1024;

/// Built-in objective functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CalibrationObjective {
    /// Equity at the end of the window, starting flat.
    TotalPnl,
}

impl CalibrationObjective {
    pub const fn code(self) -> &'static str {
        match self {
            Self::TotalPnl => "ficant.objective.total-pnl",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        (code == Self::TotalPnl.code()).then_some(Self::TotalPnl)
    }
}

/// Parameter name → candidate values. Candidates are the cartesian product, parameters in name
/// order and values in the given order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParameterGrid {
    axes: BTreeMap<String, Vec<ReplayAmount>>,
}

impl ParameterGrid {
    /// Fails on an empty grid, a blank name, an empty or repeating value list, or more than
    /// [`MAX_CALIBRATION_CANDIDATES`] combinations.
    pub fn new(axes: BTreeMap<String, Vec<ReplayAmount>>) -> DomainResult<Self> {
        let mut combinations: usize = 1;
        for (name, values) in &axes {
            let mut unique = values.clone();
            unique.sort();
            unique.dedup();
            if name.trim().is_empty()
                || name != name.trim()
                || values.is_empty()
                || unique.len() != values.len()
            {
                return Err(DomainErrorCode::InvalidValue);
            }
            combinations = combinations
                .checked_mul(values.len())
                .filter(|count| *count <= MAX_CALIBRATION_CANDIDATES)
                .ok_or(DomainErrorCode::InvalidValue)?;
        }
        if axes.is_empty() {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self { axes })
    }

    pub fn axes(&self) -> &BTreeMap<String, Vec<ReplayAmount>> {
        &self.axes
    }

    /// Every candidate: `base` overlaid with one combination of the grid, in grid order.
    pub fn candidates(
        &self,
        base: &BTreeMap<String, ReplayAmount>,
    ) -> Vec<BTreeMap<String, ReplayAmount>> {
        let mut candidates = vec![base.clone()];
        for (name, values) in &self.axes {
            candidates = candidates
                .into_iter()
                .flat_map(|candidate| {
                    values.iter().map(move |value| {
                        let mut next = candidate.clone();
                        next.insert(name.clone(), *value);
                        next
                    })
                })
                .collect();
        }
        candidates
    }
}

/// One replay of one candidate over one window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowResult {
    pub result_id: ContentHash,
    pub objective: ReplayAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateResult {
    /// The complete parameters the candidate replayed with.
    pub parameters: BTreeMap<String, ReplayAmount>,
    pub training: WindowResult,
    pub validation: WindowResult,
}

/// How a `PolicyArtifact` was calibrated (AC43).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalibrationRecord {
    base_policy: ContentHash,
    training: EffectivePeriod,
    validation: EffectivePeriod,
    objective: CalibrationObjective,
    grid: ParameterGrid,
    candidates: Vec<CandidateResult>,
    selected: usize,
}

impl CalibrationRecord {
    /// Fails when the validation window does not start at or after the end of the training
    /// window, there are no candidates, or `selected` is out of range or not the first candidate
    /// with the best training objective.
    pub fn new(
        base_policy: ContentHash,
        training: EffectivePeriod,
        validation: EffectivePeriod,
        objective: CalibrationObjective,
        grid: ParameterGrid,
        candidates: Vec<CandidateResult>,
        selected: usize,
    ) -> DomainResult<Self> {
        check_windows(&training, &validation)?;
        if best_training_candidate(&candidates) != Some(selected) {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            base_policy,
            training,
            validation,
            objective,
            grid,
            candidates,
            selected,
        })
    }

    pub fn base_policy(&self) -> &ContentHash {
        &self.base_policy
    }

    pub fn training(&self) -> &EffectivePeriod {
        &self.training
    }

    pub fn validation(&self) -> &EffectivePeriod {
        &self.validation
    }

    pub const fn objective(&self) -> CalibrationObjective {
        self.objective
    }

    pub fn grid(&self) -> &ParameterGrid {
        &self.grid
    }

    pub fn candidates(&self) -> &[CandidateResult] {
        &self.candidates
    }

    pub const fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_candidate(&self) -> &CandidateResult {
        &self.candidates[self.selected]
    }
}

/// Training and validation windows must be non-empty and the validation window must start at
/// or after the end of the training window (no overlap, no look-back).
pub fn check_windows(training: &EffectivePeriod, validation: &EffectivePeriod) -> DomainResult<()> {
    if training.from().instant() >= training.to().instant()
        || validation.from().instant() >= validation.to().instant()
        || training.to().instant() > validation.from().instant()
    {
        return Err(DomainErrorCode::InvalidEffectiveTime);
    }
    Ok(())
}

/// The first candidate with the highest training objective.
pub fn best_training_candidate(candidates: &[CandidateResult]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .fold(
            None,
            |best: Option<(usize, ReplayAmount)>, (index, candidate)| match best {
                Some((_, value)) if value >= candidate.training.objective => best,
                _ => Some((index, candidate.training.objective)),
            },
        )
        .map(|(index, _)| index)
}
