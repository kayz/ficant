//! Point-in-time bar replay (ADR-0024, ADR-0025; AC38–AC41).
//!
//! The loop decides at the visibility time of every bar the policy covers, gives the policy only
//! a [`PointInTimeView`] built from the data port's point-in-time read, and simulates each
//! resulting trade from the first bar of that instrument that starts at or after the decision,
//! within the policy's participation cap. A ledger closes every step of the bar grid (one
//! `bar_end`) and books its P&L as price change, coupon, funding and transaction cost. Nothing
//! here reads a clock or uses randomness, so identical inputs give identical results.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use chrono::{DateTime, Utc};
use ficant_domain::market::{Bar, PriceSourceType};
use ficant_domain::primitives::{ContentHash, Ulid, VersionRef};
use ficant_domain::replay::{
    BarPnl, BondCarry, CostModel, PointInTimeView, PolicyArtifact, PolicyError, PolicyEvaluator,
    ReplayAmount, ReplayDecision, ReplayFunding, ReplayIdentity, ReplayLedger, SimulatedBook,
    SimulatedFill, SimulationResult, policy_period_contains, simulate_trade,
};
use ficant_domain::subject::SubjectVersion;

use crate::{ApplicationError, ApplicationErrorCategory};

/// Read access to one verified, immutable Bar snapshot.
pub trait PointInTimeBars {
    /// Snapshot identity bound into the result.
    fn snapshot(&self) -> (Ulid, ContentHash);
    /// Price source type of the snapshot's bars (I9), recorded on every simulated execution.
    fn price_source_type(&self) -> PriceSourceType;
    /// All bars in canonical order. The loop uses this only to schedule decisions, to find the
    /// execution bars of a simulated trade and to mark the book, never to feed a policy.
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
    pub subject: &'a SubjectVersion,
    pub subject_state: VersionRef,
    /// Funding terms resolved for the Subject's funding tier (`ResolveFundingRule`).
    pub funding: ReplayFunding,
    pub carry: &'a dyn BondCarry,
    pub cost_model: String,
    pub cost_parameters: BTreeMap<String, ReplayAmount>,
    pub seed: u64,
    pub code: ContentHash,
    pub runtime: ContentHash,
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

/// A trade waiting for its executions to settle. `intent` is what the policy still counts on:
/// the face not yet executed, until the last execution attempt settles.
struct PendingTrade {
    instrument: VersionRef,
    intent: ReplayAmount,
    attempts: VecDeque<SimulatedFill>,
}

/// Runs one deterministic replay.
///
/// # Errors
///
/// Fails closed when the evaluator does not implement the artifact's family, the funding tier is
/// not the Subject's, the cost model is unknown or mis-parameterised, the policy needs a missing
/// parameter, a view would contain a bar not yet visible, a held bond has no coupon data, or the
/// snapshot has no bar the policy covers.
pub fn replay_bars(request: &BarReplayRequest<'_>) -> Result<SimulationResult, ApplicationError> {
    let policy = request.policy;
    if request.evaluator.family() != policy.family()
        || request.evaluator.family_version() != policy.family_version()
        || request.funding.tier() != request.subject.funding_tier()
    {
        return Err(invalid());
    }
    let costs = CostModel::from_declaration(&request.cost_model, &request.cost_parameters)
        .map_err(|_| invalid())?;
    let covered: Vec<&Bar> = request
        .data
        .all_bars()
        .iter()
        .filter(|bar| policy.covers(bar) && policy_period_contains(policy, bar.bar_start()))
        .collect();
    let mut steps: BTreeMap<DateTime<Utc>, Vec<&Bar>> = BTreeMap::new();
    for bar in &covered {
        steps.entry(bar.bar_end()).or_default().push(bar);
    }
    let decision_times: BTreeSet<DateTime<Utc>> =
        covered.iter().map(|bar| bar.visible_at()).collect();
    if decision_times.is_empty() {
        return Err(invalid());
    }
    let price_source = request.data.price_source_type();

    let mut replay = Replay {
        ledger: ReplayLedger::new(
            request.carry,
            request.funding.clone(),
            costs,
            policy.settlement(),
        ),
        steps: steps.into_iter().collect(),
        pending: Vec::new(),
        fills: Vec::new(),
    };
    let mut decisions = Vec::with_capacity(decision_times.len());
    for decision_time in decision_times {
        replay.advance(Some(decision_time))?;
        let visible = request.data.visible_at(decision_time)?;
        let inputs: Vec<&Bar> = visible
            .into_iter()
            .filter(|bar| policy.covers(bar))
            .collect();
        let inputs_visible_through = inputs.iter().map(|bar| bar.visible_at()).max();
        let input_bar_count = inputs.len();
        let view = PointInTimeView::new(decision_time, inputs).map_err(|_| invalid())?;
        // The policy sees settled positions plus what pending trades still intend to execute.
        let projected = replay.ledger.book().with_pending(
            replay
                .pending
                .iter()
                .map(|trade| (&trade.instrument, trade.intent)),
        );
        let target = request
            .evaluator
            .decide(policy, &view, &projected)
            .map_err(|error| match error {
                PolicyError::MissingParameter(_) | PolicyError::UnsupportedFamily => invalid(),
            })?;
        for (instrument, quantity) in projected.deltas_toward(&target) {
            let mut execution_bars: Vec<&Bar> = covered
                .iter()
                .copied()
                .filter(|bar| bar.instrument() == &instrument && bar.bar_start() >= decision_time)
                .collect();
            execution_bars.sort_by_key(|bar| (bar.bar_start(), bar.source_record_id().to_owned()));
            let attempts = simulate_trade(
                decision_time,
                &instrument,
                quantity,
                &execution_bars,
                policy.execution(),
                price_source,
            );
            replay.pending.push(PendingTrade {
                instrument,
                intent: quantity,
                attempts: attempts.into(),
            });
        }
        decisions.push(ReplayDecision {
            decision_time,
            inputs_visible_through,
            input_bar_count,
            target,
        });
    }
    replay.advance(None)?;

    let (fills, book, pnl) = replay.finish();
    let identity = replay_identity(request);
    SimulationResult::new(identity, decisions, fills, pnl, book).map_err(|_| invalid())
}

/// Everything the result ID binds on the input side (AC39).
fn replay_identity(request: &BarReplayRequest<'_>) -> ReplayIdentity {
    let (snapshot, snapshot_content) = request.data.snapshot();
    ReplayIdentity {
        policy: request.policy.content_hash().clone(),
        data_snapshot: snapshot,
        data_snapshot_content: snapshot_content,
        subject: request.subject.reference().clone(),
        subject_state: request.subject_state.clone(),
        funding: request.funding.clone(),
        carry: request.carry.identity(),
        cost_model: request.cost_model.clone(),
        cost_parameters: request.cost_parameters.clone(),
        seed: request.seed,
        code: request.code.clone(),
        runtime: request.runtime.clone(),
    }
}

struct Replay<'a> {
    ledger: ReplayLedger<'a>,
    steps: VecDeque<(DateTime<Utc>, Vec<&'a Bar>)>,
    pending: Vec<PendingTrade>,
    fills: Vec<SimulatedFill>,
}

impl Replay<'_> {
    /// Fills in time order, the final book and the P&L rows.
    fn finish(self) -> (Vec<SimulatedFill>, SimulatedBook, Vec<BarPnl>) {
        let mut fills = self.fills;
        fills.sort_by(|left, right| {
            (left.effective_at, &left.instrument, left.decision_time).cmp(&(
                right.effective_at,
                &right.instrument,
                right.decision_time,
            ))
        });
        let (book, pnl) = self.ledger.into_parts();
        (fills, book, pnl)
    }

    /// Closes every step ending at or before `until` (all of them when `None`), settling the
    /// execution attempts effective by then.
    fn advance(&mut self, until: Option<DateTime<Utc>>) -> Result<(), ApplicationError> {
        let due = |time: DateTime<Utc>| until.is_none_or(|limit| time <= limit);
        while let Some((step_end, _)) = self.steps.front() {
            if !due(*step_end) {
                break;
            }
            let (step_end, bars) = self.steps.pop_front().expect("front exists");
            let settled = self.settle(step_end);
            let executions: Vec<&SimulatedFill> = settled
                .iter()
                .filter(|fill| fill.execution_bar.is_some())
                .collect();
            if executions.iter().any(|fill| fill.effective_at != step_end) {
                return Err(invalid());
            }
            self.ledger
                .close_step(&bars, &executions)
                .map_err(|_| invalid())?;
            self.fills.extend(settled);
        }
        // Attempts with no bar to execute in settle at their own time and move no cash.
        let settled = match until {
            Some(limit) => self.settle(limit),
            None => self.settle(DateTime::<Utc>::MAX_UTC),
        };
        if settled.iter().any(|fill| fill.execution_bar.is_some()) {
            return Err(invalid());
        }
        self.fills.extend(settled);
        Ok(())
    }

    /// Takes every attempt effective at or before `time` off the pending trades.
    fn settle(&mut self, time: DateTime<Utc>) -> Vec<SimulatedFill> {
        let mut settled = Vec::new();
        for trade in &mut self.pending {
            while trade
                .attempts
                .front()
                .is_some_and(|attempt| attempt.effective_at <= time)
            {
                let attempt = trade.attempts.pop_front().expect("front exists");
                trade.intent = if trade.attempts.is_empty() {
                    ReplayAmount::ZERO
                } else {
                    trade.intent.minus(attempt.quantity)
                };
                settled.push(attempt);
            }
        }
        self.pending.retain(|trade| !trade.attempts.is_empty());
        settled
    }
}
