//! AC42: a simulated execution that would breach a constraint is rejected or reduced as the
//! `PolicyArtifact` declares, and the result names the triggered constraint.

mod bar_support;
mod replay_support;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ficant_application::{ApplicationError, BarReplayRequest, replay_bars};
use ficant_domain::constraint::{ConstraintDefinition, ConstraintMeasure, ConstraintSet};
use ficant_domain::market::{BarFrequency, BarSettlement};
use ficant_domain::primitives::{ContentHash, EffectivePeriod, Ulid, Version, VersionRef};
use ficant_domain::replay::{
    COST_NONE, ConstraintAction, ConstraintHandling, ExecutionRules, NoFillReason, PointInTimeView,
    PolicyArtifact, PolicyArtifactInput, PolicyError, PolicyEvaluator, RemainderHandling,
    RemainderOutcome, ReplayAmount, SimulatedBook, SimulationResult, TargetExposure,
    YIELD_THRESHOLD_FAMILY, YieldThresholdPolicy,
};
use ficant_domain::subject::{SubjectStateSnapshot, SubjectVersion};
use rust_decimal::Decimal;

use bar_support::*;
use replay_support::*;

const SINGLE: &str = "bond.single_issue_face";
const TOTAL: &str = "bond.total_face";
const BORROWED: &str = "repo.borrowed_cash";

fn rule_pack() -> VersionRef {
    VersionRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FC0").unwrap(),
        Version::new(1).unwrap(),
    )
}

fn constraint_set() -> ConstraintSet {
    ConstraintSet::new(
        rule_pack(),
        vec![
            ConstraintDefinition::new(SINGLE, ConstraintMeasure::InstrumentFace).unwrap(),
            ConstraintDefinition::new(TOTAL, ConstraintMeasure::PortfolioFace).unwrap(),
            ConstraintDefinition::new(BORROWED, ConstraintMeasure::BorrowedCash).unwrap(),
        ],
    )
    .unwrap()
}

/// Ceilings that never bind (single 100m, total 100m, borrowed 200m) unless overridden.
fn ceilings(overrides: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut values = BTreeMap::from([
        (SINGLE.to_owned(), "100000000".to_owned()),
        (TOTAL.to_owned(), "100000000".to_owned()),
        (BORROWED.to_owned(), "200000000".to_owned()),
    ]);
    for (code, value) in overrides {
        values.insert((*code).to_owned(), (*value).to_owned());
    }
    values.into_iter().collect()
}

fn handling(overrides: &[(&str, ConstraintHandling)]) -> BTreeMap<String, ConstraintHandling> {
    let mut values = BTreeMap::from([
        (SINGLE.to_owned(), ConstraintHandling::Reduce),
        (TOTAL.to_owned(), ConstraintHandling::Reduce),
        (BORROWED.to_owned(), ConstraintHandling::Reduce),
    ]);
    for (code, value) in overrides {
        values.insert((*code).to_owned(), *value);
    }
    values
}

fn policy_input(
    family: &str,
    execution: ExecutionRules,
    constraints: BTreeMap<String, ConstraintHandling>,
) -> PolicyArtifactInput {
    PolicyArtifactInput {
        policy_artifact_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FP0").unwrap(),
        owner: owner(),
        family: family.to_owned(),
        family_version: 1,
        parameters: BTreeMap::from([
            ("entry_yield".to_owned(), amount("1.84")),
            ("exit_yield".to_owned(), amount("1.82")),
            ("target_face".to_owned(), amount("10000000")),
        ]),
        universe: vec![instrument()],
        frequency: BarFrequency::M1,
        settlement: BarSettlement::T1,
        valid: EffectivePeriod::new(
            market_time("2026-07-01T00:00:00Z"),
            market_time("2026-08-01T00:00:00Z"),
        )
        .unwrap(),
        execution,
        constraints,
    }
}

fn full_fills() -> ExecutionRules {
    ExecutionRules::new(amount("1"), RemainderHandling::Cancel).unwrap()
}

struct Case {
    policy: PolicyArtifact,
    subject: SubjectVersion,
    state: SubjectStateSnapshot,
    constraints: Option<ConstraintSet>,
}

impl Case {
    fn new(overrides: &[(&str, &str)], declared: BTreeMap<String, ConstraintHandling>) -> Self {
        let subject = subject_bound(1, &rule_pack());
        let values = ceilings(overrides);
        let pairs: Vec<(&str, &str)> = values
            .iter()
            .map(|(code, value)| (code.as_str(), value.as_str()))
            .collect();
        Self {
            policy: PolicyArtifact::new(policy_input(
                YIELD_THRESHOLD_FAMILY,
                full_fills(),
                declared,
            ))
            .unwrap(),
            state: state_with(&subject, "01ARZ3NDEKTSV4RRFFQ69G5FV1", &pairs),
            subject,
            constraints: Some(constraint_set()),
        }
    }

    fn run_with(
        &self,
        data: &SnapshotBars,
        evaluator: &dyn PolicyEvaluator,
    ) -> Result<SimulationResult, ApplicationError> {
        replay_bars(&BarReplayRequest {
            policy: &self.policy,
            evaluator,
            data,
            subject: &self.subject,
            subject_state: &self.state,
            constraints: self.constraints.as_ref(),
            funding: funding("0.0365", 365),
            carry: &TableCarry::new(&[("2026-07-20", "1.00")], &[]),
            cost_model: COST_NONE.to_owned(),
            cost_parameters: BTreeMap::new(),
            seed: 7,
            code: ContentHash::digest(b"code"),
            runtime: ContentHash::digest(b"runtime"),
        })
    }

    fn run(&self, data: &SnapshotBars) -> Result<SimulationResult, ApplicationError> {
        self.run_with(data, &YieldThresholdPolicy)
    }
}

async fn one_day() -> SnapshotBars {
    SnapshotBars(snapshot(ONE_DAY, "01ARZ3NDEKTSV4RRFFQ69G5FS5").await)
}

#[tokio::test]
async fn non_binding_constraints_leave_the_replay_unchanged() {
    let data = one_day().await;
    let result = Case::new(&[], handling(&[])).run(&data).unwrap();
    // The ADR-0024 path: buy 10m in b0932, blocked sell in b0936, sell 10m in b0938.
    let summary: Vec<(Option<&str>, String)> = result
        .fills()
        .iter()
        .map(|fill| (fill.execution_bar.as_deref(), fill.quantity.canonical()))
        .collect();
    assert_eq!(
        summary,
        vec![
            (Some("b0932"), "10000000".to_owned()),
            (Some("b0936"), "0".to_owned()),
            (Some("b0938"), "-10000000".to_owned()),
        ]
    );
    assert!(result.fills().iter().all(|fill| fill.constraint.is_none()));
    assert_eq!(result.book().cash(), amount("19000"));
}

#[tokio::test]
async fn a_breaching_buy_is_reduced_to_the_single_issue_ceiling() {
    // Single-issue ceiling 6m, declared REDUCE: the 10m buy executes 6m and names the constraint.
    let data = one_day().await;
    let result = Case::new(&[(SINGLE, "6000000")], handling(&[]))
        .run(&data)
        .unwrap();
    let buy = &result.fills()[0];
    assert_eq!(buy.execution_bar.as_deref(), Some("b0932"));
    assert_eq!(buy.requested, amount("10000000"));
    assert_eq!(buy.quantity, amount("6000000"));
    let outcome = buy.constraint.as_ref().unwrap();
    assert_eq!(outcome.unconstrained, amount("10000000"));
    assert_eq!(outcome.triggered, vec![SINGLE.to_owned()]);
    assert_eq!(outcome.action, ConstraintAction::Reduced);
    let remainder = buy.remainder.unwrap();
    assert_eq!(remainder.quantity, amount("4000000"));
    assert_eq!(remainder.reason, NoFillReason::Constraint);
    assert_eq!(remainder.outcome, RemainderOutcome::Cancelled);

    // The later sell reduces the position, so it is never constrained.
    let sell = result
        .fills()
        .iter()
        .find(|fill| fill.execution_bar.as_deref() == Some("b0938"))
        .unwrap();
    assert_eq!(sell.quantity, amount("-6000000"));
    assert!(sell.constraint.is_none());
    assert_eq!(result.book().positions().count(), 0);
}

#[tokio::test]
async fn a_breaching_buy_is_rejected_when_declared_so() {
    // Total-face ceiling 5m, declared REJECT: nothing executes and the record names it.
    let data = one_day().await;
    let result = Case::new(
        &[(TOTAL, "5000000")],
        handling(&[(TOTAL, ConstraintHandling::Reject)]),
    )
    .run(&data)
    .unwrap();
    let buy = &result.fills()[0];
    assert_eq!(buy.quantity, ReplayAmount::ZERO);
    assert_eq!(buy.no_fill, Some(NoFillReason::Constraint));
    let outcome = buy.constraint.as_ref().unwrap();
    assert_eq!(outcome.triggered, vec![TOTAL.to_owned()]);
    assert_eq!(outcome.action, ConstraintAction::Rejected);
    assert_eq!(result.book().positions().count(), 0);
    assert_eq!(result.book().cash(), ReplayAmount::ZERO);
    // Flat afterwards: the exit signals have nothing to sell.
    assert_eq!(result.fills().len(), 1, "{:#?}", result.fills());
}

#[tokio::test]
async fn one_reject_among_the_breached_constraints_rejects_the_whole_execution() {
    // Both single (REDUCE, 6m) and total (REJECT, 8m) are breached by the 10m buy.
    let data = one_day().await;
    let result = Case::new(
        &[(SINGLE, "6000000"), (TOTAL, "8000000")],
        handling(&[(TOTAL, ConstraintHandling::Reject)]),
    )
    .run(&data)
    .unwrap();
    let outcome = result.fills()[0].constraint.as_ref().unwrap();
    assert_eq!(outcome.triggered, vec![SINGLE.to_owned(), TOTAL.to_owned()]);
    assert_eq!(outcome.action, ConstraintAction::Rejected);
    assert_eq!(result.fills()[0].quantity, ReplayAmount::ZERO);
}

#[tokio::test]
async fn borrowed_cash_reduces_the_buy_to_what_the_ceiling_can_fund() {
    // Borrowed-cash ceiling 5m. Buying at VWAP 100.13 with accrued 1.00 borrows 1.0113 per face,
    // so the buy shrinks to 5,000,000 / 1.0113 = 4,944,131.316127756353 (12 places, toward zero).
    let data = one_day().await;
    let result = Case::new(&[(BORROWED, "5000000")], handling(&[]))
        .run(&data)
        .unwrap();
    let buy = &result.fills()[0];
    assert_eq!(buy.quantity, amount("4944131.316127756353"));
    assert_eq!(
        buy.constraint.as_ref().unwrap().triggered,
        vec![BORROWED.to_owned()]
    );
    let after_buy = result
        .pnl()
        .iter()
        .find(|row| row.bar_end == utc("2026-07-20T01:33:00Z"))
        .unwrap();
    let borrowed: Decimal = after_buy.cash.canonical().parse::<Decimal>().unwrap().abs();
    assert!(borrowed <= Decimal::from(5_000_000));
    assert!(Decimal::from(5_000_000) - borrowed < "0.000000000002".parse::<Decimal>().unwrap());
}

/// Targets 10m at the first decision and 20m at the second, then keeps whatever it holds.
struct TwoSteps;

impl PolicyEvaluator for TwoSteps {
    #[allow(clippy::unnecessary_literal_bound)]
    fn family(&self) -> &str {
        "test.policy.two-steps"
    }
    fn family_version(&self) -> u32 {
        1
    }
    fn decide(
        &self,
        policy: &PolicyArtifact,
        view: &PointInTimeView<'_>,
        book: &SimulatedBook,
    ) -> Result<TargetExposure, PolicyError> {
        let instrument = policy.universe()[0].clone();
        let face = match view.decision_time() {
            time if time == utc("2026-07-20T01:31:02Z") => amount("10000000"),
            time if time == utc("2026-07-20T01:32:02Z") => amount("20000000"),
            _ => book.position(&instrument),
        };
        let mut target = TargetExposure::default();
        target.set(instrument, face);
        Ok(target)
    }
}

#[tokio::test]
async fn attempts_in_one_bar_share_its_participation_cap() {
    // AC41 across trades: with a 25% cap and carried remainders, the first trade's carried part
    // and the second trade both reach b0934 (10m traded → 2.5m in total, not 2.5m each).
    let data = one_day().await;
    let mut case = Case::new(&[], handling(&[]));
    case.policy = PolicyArtifact::new(policy_input(
        "test.policy.two-steps",
        ExecutionRules::new(amount("0.25"), RemainderHandling::Carry).unwrap(),
        handling(&[]),
    ))
    .unwrap();
    let result = case.run_with(&data, &TwoSteps).unwrap();
    let mut per_bar: BTreeMap<&str, Decimal> = BTreeMap::new();
    for fill in result.fills() {
        if let Some(bar) = fill.execution_bar.as_deref() {
            *per_bar.entry(bar).or_default() +=
                fill.quantity.canonical().parse::<Decimal>().unwrap().abs();
        }
    }
    for bar in data.0.bars() {
        let volume = Decimal::from_i128_with_scale(
            bar.volume().coefficient().parse().unwrap(),
            bar.volume().scale(),
        );
        let executed = per_bar
            .get(bar.source_record_id())
            .copied()
            .unwrap_or_default();
        assert!(
            executed <= volume * "0.25".parse::<Decimal>().unwrap(),
            "{}: {executed} > 25% of {volume}",
            bar.source_record_id()
        );
    }
    let in_b0934: Vec<String> = result
        .fills()
        .iter()
        .filter(|fill| fill.execution_bar.as_deref() == Some("b0934"))
        .map(|fill| fill.quantity.canonical())
        .collect();
    assert_eq!(in_b0934, vec!["2500000".to_owned(), "0".to_owned()]);
}

#[tokio::test]
async fn constraint_inputs_fail_closed() {
    let data = one_day().await;

    // A constraint without a declared handling.
    let mut declared = handling(&[]);
    declared.remove(BORROWED);
    assert!(Case::new(&[], declared).run(&data).is_err());

    // A constraint without a ceiling in the state snapshot.
    let mut case = Case::new(&[], handling(&[]));
    case.state = state_with(
        &case.subject,
        "01ARZ3NDEKTSV4RRFFQ69G5FV1",
        &[(SINGLE, "1"), (TOTAL, "1")],
    );
    assert!(case.run(&data).is_err());

    // The Subject binds a constraint set but none is given, or another version is given.
    let mut case = Case::new(&[], handling(&[]));
    case.constraints = None;
    assert!(case.run(&data).is_err());
    let mut case = Case::new(&[], handling(&[]));
    case.constraints = Some(
        ConstraintSet::new(
            VersionRef::new(rule_pack().id().clone(), Version::new(2).unwrap()),
            constraint_set().definitions().to_vec(),
        )
        .unwrap(),
    );
    assert!(case.run(&data).is_err());

    // A state snapshot of another Subject version, or one not yet visible at the first decision.
    let mut case = Case::new(&[], handling(&[]));
    case.state = state_with(
        &subject_bound(2, &rule_pack()),
        "01ARZ3NDEKTSV4RRFFQ69G5FV1",
        &[],
    );
    assert!(case.run(&data).is_err());
    let mut case = Case::new(&[], handling(&[]));
    case.state = late_state(&case.subject, utc("2026-07-20T01:31:03Z"));
    assert!(case.run(&data).is_err());
}

fn late_state(subject: &SubjectVersion, visible_at: DateTime<Utc>) -> SubjectStateSnapshot {
    let early = state_with(subject, "01ARZ3NDEKTSV4RRFFQ69G5FV1", &[]);
    SubjectStateSnapshot::new(
        early.id().clone(),
        subject.reference().clone(),
        early.net_capital().clone(),
        Vec::new(),
        visible_at,
        visible_at,
        "Asia/Shanghai",
    )
    .unwrap()
}

#[tokio::test]
async fn ceilings_handlings_and_the_state_snapshot_are_part_of_the_result_id() {
    let data = one_day().await;
    let base = Case::new(&[], handling(&[])).run(&data).unwrap();
    let ids = [
        Case::new(&[(SINGLE, "100000001")], handling(&[]))
            .run(&data)
            .unwrap(),
        Case::new(&[], handling(&[(TOTAL, ConstraintHandling::Reject)]))
            .run(&data)
            .unwrap(),
        {
            let mut case = Case::new(&[], handling(&[]));
            let values = ceilings(&[]);
            let pairs: Vec<(&str, &str)> = values
                .iter()
                .map(|(code, value)| (code.as_str(), value.as_str()))
                .collect();
            case.state = state_with(&case.subject, "01ARZ3NDEKTSV4RRFFQ69G5FV2", &pairs);
            case.run(&data).unwrap()
        },
    ];
    for (index, result) in ids.iter().enumerate() {
        assert_eq!(
            result.fills(),
            base.fills(),
            "variant {index} trades the same"
        );
        assert_ne!(
            result.result_id(),
            base.result_id(),
            "variant {index} kept the ID"
        );
    }
}
