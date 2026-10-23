//! Point-in-time bar replay (ADR-0024; AC38, AC39).
//!
//! The loop decides at the visibility time of every bar the policy covers, gives the policy only
//! a [`PointInTimeView`] built from the data port's point-in-time read, and simulates each
//! resulting trade in the first bar of that instrument that starts at or after the decision.
//! Nothing here reads a clock or uses randomness, so identical inputs give identical results.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use ficant_domain::market::Bar;
use ficant_domain::primitives::{ContentHash, Ulid, VersionRef};
use ficant_domain::research::{
    PointInTimeView, PolicyArtifact, PolicyError, PolicyEvaluator, ReplayAmount, ReplayDecision,
    ReplayIdentity, SimulatedBook, SimulatedFill, SimulationResult, apply_fills,
    policy_period_contains,
};

use crate::{ApplicationError, ApplicationErrorCategory};

/// Read access to one verified, immutable Bar snapshot.
pub trait PointInTimeBars {
    /// Snapshot identity bound into the result.
    fn snapshot(&self) -> (Ulid, ContentHash);
    /// All bars in canonical order. The loop uses this only to schedule decisions and to find
    /// the execution bar for a simulated fill, never to feed a policy.
    fn all_bars(&self) -> &[Bar];
    /// Bars with `visible_at ≤ decision_time`.
    ///
    /// # Errors
    ///
    /// Fails past the snapshot's visibility cutoff.
    fn visible_at(&self, decision_time: DateTime<Utc>) -> Result<Vec<&Bar>, ApplicationError>;
}

pub struct BarReplayRequest<'a> {
    pub policy: &'a PolicyArtifact,
    pub evaluator: &'a dyn PolicyEvaluator,
    pub data: &'a dyn PointInTimeBars,
    pub subject: VersionRef,
    pub subject_state: VersionRef,
    pub cost_model: String,
    pub cost_parameters: std::collections::BTreeMap<String, ReplayAmount>,
    pub seed: u64,
    pub code: ContentHash,
    pub runtime: ContentHash,
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

/// Runs one deterministic replay.
///
/// # Errors
///
/// Fails closed when the evaluator does not implement the artifact's family, the policy needs a
/// missing parameter, a view would contain a bar not yet visible, or the snapshot has no bar the
/// policy covers.
pub fn replay_bars(request: &BarReplayRequest<'_>) -> Result<SimulationResult, ApplicationError> {
    let policy = request.policy;
    if request.evaluator.family() != policy.family()
        || request.evaluator.family_version() != policy.family_version()
        || request.cost_model.trim().is_empty()
    {
        return Err(invalid());
    }
    let covered: Vec<&Bar> = request
        .data
        .all_bars()
        .iter()
        .filter(|bar| policy.covers(bar) && policy_period_contains(policy, bar.bar_start()))
        .collect();
    let decision_times: BTreeSet<DateTime<Utc>> =
        covered.iter().map(|bar| bar.visible_at()).collect();
    if decision_times.is_empty() {
        return Err(invalid());
    }

    let mut book = SimulatedBook::default();
    let mut pending: Vec<SimulatedFill> = Vec::new();
    let mut fills: Vec<SimulatedFill> = Vec::new();
    let mut decisions = Vec::with_capacity(decision_times.len());
    for decision_time in decision_times {
        settle(&mut book, &mut pending, &mut fills, Some(decision_time));
        let visible = request.data.visible_at(decision_time)?;
        let inputs: Vec<&Bar> = visible
            .into_iter()
            .filter(|bar| policy.covers(bar))
            .collect();
        let inputs_visible_through = inputs.iter().map(|bar| bar.visible_at()).max();
        let input_bar_count = inputs.len();
        let view = PointInTimeView::new(decision_time, inputs).map_err(|_| invalid())?;
        // The policy sees settled positions plus trades still waiting for their execution bar.
        let projected = book.with_pending(&pending);
        let target = request
            .evaluator
            .decide(policy, &view, &projected)
            .map_err(|error| match error {
                PolicyError::MissingParameter(_) | PolicyError::UnsupportedFamily => invalid(),
            })?;
        for (instrument, quantity) in projected.deltas_toward(&target) {
            let execution_bar = covered
                .iter()
                .copied()
                .filter(|bar| bar.instrument() == &instrument && bar.bar_start() >= decision_time)
                .min_by_key(|bar| (bar.bar_start(), bar.source_record_id().to_owned()));
            pending.push(SimulatedFill::simulate(
                decision_time,
                instrument,
                quantity,
                execution_bar,
            ));
        }
        decisions.push(ReplayDecision {
            decision_time,
            inputs_visible_through,
            input_bar_count,
            target,
        });
    }
    settle(&mut book, &mut pending, &mut fills, None);

    let (snapshot, snapshot_content) = request.data.snapshot();
    let identity = ReplayIdentity {
        policy: policy.content_hash().clone(),
        data_snapshot: snapshot,
        data_snapshot_content: snapshot_content,
        subject: request.subject.clone(),
        subject_state: request.subject_state.clone(),
        cost_model: request.cost_model.clone(),
        cost_parameters: request.cost_parameters.clone(),
        seed: request.seed,
        code: request.code.clone(),
        runtime: request.runtime.clone(),
    };
    SimulationResult::new(identity, decisions, fills, book).map_err(|_| invalid())
}

/// Moves every pending fill effective at or before `until` (all of them when `None`) into the
/// book, in time order.
fn settle(
    book: &mut SimulatedBook,
    pending: &mut Vec<SimulatedFill>,
    fills: &mut Vec<SimulatedFill>,
    until: Option<DateTime<Utc>>,
) {
    pending.sort_by(|left, right| {
        (left.effective_at, &left.instrument, left.decision_time).cmp(&(
            right.effective_at,
            &right.instrument,
            right.decision_time,
        ))
    });
    let due = pending
        .iter()
        .take_while(|fill| until.is_none_or(|limit| fill.effective_at <= limit))
        .count();
    let settled: Vec<SimulatedFill> = pending.drain(..due).collect();
    apply_fills(book, &settled);
    fills.extend(settled);
}
