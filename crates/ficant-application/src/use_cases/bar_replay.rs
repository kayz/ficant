//! Point-in-time bar replay (ADR-0024, ADR-0025, ADR-0027; AC38–AC42).
//!
//! The loop decides at the visibility time of every bar the policy covers, gives the policy only
//! a [`PointInTimeView`] built from the data port's point-in-time read, and registers each
//! resulting trade. Trades execute bar by bar from the first bar of that instrument that starts at
//! or after the decision: each attempt is capped by the policy's participation cap, then checked
//! against the Subject's constraints on the book as it stands at that moment, then settled. A
//! ledger closes every step of the bar grid (one `bar_end`) and books its P&L as price change,
//! coupon, funding and transaction cost. Nothing here reads a clock or uses randomness, so
//! identical inputs give identical results.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use chrono::{DateTime, Utc};
use ficant_domain::constraint::ConstraintSet;
use ficant_domain::market::{Bar, PriceSourceType};
use ficant_domain::primitives::{ContentHash, Ulid, VersionRef};
use ficant_domain::replay::{
    BarPnl, BondCarry, CostModel, ExecutionRules, PointInTimeView, PolicyArtifact, PolicyError,
    PolicyEvaluator, RemainderOutcome, ReplayAmount, ReplayConstraint, ReplayDecision,
    ReplayFunding, ReplayIdentity, ReplayLedger, SimulatedBook, SimulatedFill, SimulationResult,
    attempt_in_bar, bind_constraints, check_execution, no_later_bar, policy_period_contains,
    subject_state_hash,
};
use ficant_domain::subject::{SubjectStateSnapshot, SubjectVersion};

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
    /// The Subject state snapshot the constraint ceilings come from.
    pub subject_state: &'a SubjectStateSnapshot,
    /// The Subject's constraint set (`ResolveConstraintSet`); `None` exactly when the Subject
    /// binds none.
    pub constraints: Option<&'a ConstraintSet>,
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

/// A trade still executing. `intent` is what the policy counts on: the face not yet executed,
/// until the trade's last attempt settles.
struct PendingTrade<'a> {
    decision_time: DateTime<Utc>,
    instrument: VersionRef,
    intent: ReplayAmount,
    remaining: ReplayAmount,
    bars: VecDeque<&'a Bar>,
}

/// Runs one deterministic replay.
///
/// # Errors
///
/// Fails closed when the evaluator does not implement the artifact's family, the funding tier is
/// not the Subject's, the constraint set is not the one the Subject binds, the state snapshot
/// belongs to another Subject or was not visible before the first decision, a constraint has no
/// ceiling or no declared handling, the cost model is unknown or mis-parameterised, the policy
/// needs a missing parameter, a view would contain a bar not yet visible, a held bond has no
/// coupon data, or the snapshot has no bar the policy covers.
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
    let first_decision = *decision_times.first().ok_or_else(invalid)?;
    let constraints = enforced_constraints(request, first_decision)?;

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
        constraints: &constraints,
        rules: policy.execution(),
        price_source: request.data.price_source_type(),
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
            let mut bars: Vec<&Bar> = covered
                .iter()
                .copied()
                .filter(|bar| bar.instrument() == &instrument && bar.bar_start() >= decision_time)
                .collect();
            bars.sort_by_key(|bar| (bar.bar_start(), bar.source_record_id().to_owned()));
            replay.register(decision_time, instrument, quantity, bars);
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
    let identity = replay_identity(request, constraints);
    SimulationResult::new(identity, decisions, fills, pnl, book).map_err(|_| invalid())
}

/// Binds the Subject's constraint set to its state snapshot and the policy's declarations.
fn enforced_constraints(
    request: &BarReplayRequest<'_>,
    first_decision: DateTime<Utc>,
) -> Result<Vec<ReplayConstraint>, ApplicationError> {
    let bound = request
        .subject
        .constraint_set_ref()
        .map(ficant_domain::subject::ConstraintSetRef::reference);
    let state = request.subject_state;
    if state.subject_ref() != request.subject.reference()
        || state.visible_at() > first_decision
        || bound != request.constraints.map(ConstraintSet::rule_pack)
    {
        return Err(invalid());
    }
    match request.constraints {
        None => Ok(Vec::new()),
        Some(set) => bind_constraints(
            set,
            request.subject.reference(),
            state,
            request.policy.constraint_handling(),
        )
        .map_err(|_| invalid()),
    }
}

/// Everything the result ID binds on the input side (AC39).
fn replay_identity(
    request: &BarReplayRequest<'_>,
    constraints: Vec<ReplayConstraint>,
) -> ReplayIdentity {
    let (snapshot, snapshot_content) = request.data.snapshot();
    ReplayIdentity {
        policy_id: request.policy.id().clone(),
        policy: request.policy.content_hash().clone(),
        data_snapshot: snapshot,
        data_snapshot_content: snapshot_content,
        subject: request.subject.reference().clone(),
        subject_state: request.subject_state.id().clone(),
        subject_state_content: subject_state_hash(request.subject_state),
        constraint_set: request.constraints.map(|set| set.rule_pack().clone()),
        constraints,
        funding: request.funding.clone(),
        carry_source: request.carry.source(),
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
    pending: Vec<PendingTrade<'a>>,
    fills: Vec<SimulatedFill>,
    constraints: &'a [ReplayConstraint],
    rules: ExecutionRules,
    price_source: PriceSourceType,
}

impl<'a> Replay<'a> {
    /// Registers a trade; with no bar left to execute in, it is recorded at once.
    fn register(
        &mut self,
        decision_time: DateTime<Utc>,
        instrument: VersionRef,
        quantity: ReplayAmount,
        bars: Vec<&'a Bar>,
    ) {
        if bars.is_empty() {
            self.fills.push(no_later_bar(
                decision_time,
                &instrument,
                quantity,
                decision_time,
                self.price_source,
            ));
            return;
        }
        self.pending.push(PendingTrade {
            decision_time,
            instrument,
            intent: quantity,
            remaining: quantity,
            bars: bars.into(),
        });
    }

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

    /// Closes every step ending at or before `until` (all of them when `None`), executing the
    /// attempts that fall into each.
    fn advance(&mut self, until: Option<DateTime<Utc>>) -> Result<(), ApplicationError> {
        while let Some((step_end, _)) = self.steps.front() {
            if until.is_some_and(|limit| *step_end > limit) {
                break;
            }
            let (step_end, bars) = self.steps.pop_front().expect("front exists");
            self.ledger.open_step(&bars).map_err(|_| invalid())?;
            let mut pending = std::mem::take(&mut self.pending);
            // Face already executed per bar, so all attempts in one bar share its cap (AC41).
            let mut used: BTreeMap<String, ReplayAmount> = BTreeMap::new();
            for trade in &mut pending {
                self.execute_due(trade, step_end, &mut used)?;
            }
            pending.retain(|trade| !trade.remaining.is_zero());
            self.pending = pending;
            self.ledger.finish_step().map_err(|_| invalid())?;
        }
        Ok(())
    }

    /// Executes the trade's attempt in the bar ending at `step_end`, if it has one: participation
    /// cap first, then the constraints on the current book, then settlement.
    fn execute_due(
        &mut self,
        trade: &mut PendingTrade<'a>,
        step_end: DateTime<Utc>,
        used: &mut BTreeMap<String, ReplayAmount>,
    ) -> Result<(), ApplicationError> {
        let Some(bar) = trade.bars.front().copied() else {
            return Ok(());
        };
        if bar.bar_end() != step_end || trade.remaining.is_zero() {
            return Ok(());
        }
        trade.bars.pop_front();
        let record = bar.source_record_id().to_owned();
        let mut attempt = attempt_in_bar(
            trade.decision_time,
            &trade.instrument,
            trade.remaining,
            bar,
            used.get(&record).copied().unwrap_or(ReplayAmount::ZERO),
            self.rules,
            self.price_source,
        );
        if let (Some(price), false) = (attempt.price, attempt.quantity.is_zero()) {
            let context = self
                .ledger
                .execution_context(&trade.instrument, price)
                .map_err(|_| invalid())?;
            let (allowed, outcome) = check_execution(self.constraints, &context, attempt.quantity);
            if let Some(outcome) = outcome {
                attempt.constrain(allowed, outcome);
            }
        }
        self.ledger.settle(&attempt).map_err(|_| invalid())?;
        let consumed = used.entry(record).or_insert(ReplayAmount::ZERO);
        *consumed = consumed.plus(attempt.quantity.abs());
        trade.remaining = trade.remaining.minus(attempt.quantity);
        let carried = attempt
            .remainder
            .is_some_and(|remainder| remainder.outcome == RemainderOutcome::Carried);
        let effective_at = attempt.effective_at;
        self.fills.push(attempt);
        if carried && trade.bars.is_empty() {
            self.fills.push(no_later_bar(
                trade.decision_time,
                &trade.instrument,
                trade.remaining,
                effective_at,
                self.price_source,
            ));
        }
        if carried && !trade.bars.is_empty() {
            trade.intent = trade.remaining;
        } else {
            trade.intent = ReplayAmount::ZERO;
            trade.remaining = ReplayAmount::ZERO;
            trade.bars.clear();
        }
        Ok(())
    }
}
