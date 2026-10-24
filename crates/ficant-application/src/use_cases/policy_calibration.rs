//! Policy calibration over a parameter grid (ADR-0028; AC43).
//!
//! Every candidate replays once over the training window and once over the validation window,
//! each time from a flat book with the policy valid only for that window. The candidate with
//! the best training objective is selected (the first one on a tie); validation results are
//! recorded for every candidate but never influence the choice.

use std::collections::BTreeMap;

use ficant_domain::primitives::{EffectivePeriod, Ulid};
use ficant_domain::replay::{
    CalibrationObjective, CalibrationRecord, CandidateResult, ParameterGrid, PolicyArtifact,
    ReplayAmount, WindowResult, best_training_candidate, check_windows,
};

use super::bar_replay::{BarReplayRequest, replay_bars};
use crate::{ApplicationError, ApplicationErrorCategory};

pub struct CalibrationRequest<'a> {
    /// Everything a replay needs; its `policy` is the base policy whose parameters the grid
    /// overrides.
    pub replay: BarReplayRequest<'a>,
    pub grid: ParameterGrid,
    pub training: EffectivePeriod,
    pub validation: EffectivePeriod,
    /// Objective function code, e.g. `ficant.objective.total-pnl`.
    pub objective: String,
    /// Identity and validity period of the calibrated artifact.
    pub output_id: Ulid,
    pub output_valid: EffectivePeriod,
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

/// Calibrates the base policy and returns the calibrated `PolicyArtifact`.
///
/// # Errors
///
/// Fails closed when the windows are empty, overlap or the validation window does not follow
/// the training window, the objective is unknown, or any candidate's replay over either window
/// fails; no artifact is produced then.
pub fn calibrate_policy(
    request: &CalibrationRequest<'_>,
) -> Result<PolicyArtifact, ApplicationError> {
    check_windows(&request.training, &request.validation).map_err(|_| invalid())?;
    let objective = CalibrationObjective::from_code(&request.objective).ok_or_else(invalid)?;
    let base = request.replay.policy;
    let mut candidates = Vec::new();
    for parameters in request.grid.candidates(&base.input().parameters) {
        let training = replay_window(request, &parameters, &request.training, objective)?;
        let validation = replay_window(request, &parameters, &request.validation, objective)?;
        candidates.push(CandidateResult {
            parameters,
            training,
            validation,
        });
    }
    let selected = best_training_candidate(&candidates).ok_or_else(invalid)?;
    let parameters = candidates[selected].parameters.clone();
    let record = CalibrationRecord::new(
        base.content_hash().clone(),
        request.training.clone(),
        request.validation.clone(),
        objective,
        request.grid.clone(),
        candidates,
        selected,
    )
    .map_err(|_| invalid())?;
    let mut input = base.input().clone();
    input.policy_artifact_id = request.output_id.clone();
    input.parameters = parameters;
    input.valid = request.output_valid.clone();
    input.calibration = Some(record);
    PolicyArtifact::new(input).map_err(|_| invalid())
}

/// Replays one candidate over one window from a flat book.
fn replay_window(
    request: &CalibrationRequest<'_>,
    parameters: &BTreeMap<String, ReplayAmount>,
    window: &EffectivePeriod,
    objective: CalibrationObjective,
) -> Result<WindowResult, ApplicationError> {
    let mut input = request.replay.policy.input().clone();
    input.parameters = parameters.clone();
    input.valid = window.clone();
    input.calibration = None;
    let policy = PolicyArtifact::new(input).map_err(|_| invalid())?;
    let base = &request.replay;
    let result = replay_bars(&BarReplayRequest {
        policy: &policy,
        funding: base.funding.clone(),
        cost_model: base.cost_model.clone(),
        cost_parameters: base.cost_parameters.clone(),
        code: base.code.clone(),
        runtime: base.runtime.clone(),
        ..*base
    })?;
    let value = match objective {
        CalibrationObjective::TotalPnl => result
            .pnl()
            .last()
            .map_or(ReplayAmount::ZERO, |row| row.equity),
    };
    Ok(WindowResult {
        result_id: result.result_id().clone(),
        objective: value,
    })
}
