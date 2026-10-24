//! AC43: a calibrated `PolicyArtifact` references the training and validation windows, the
//! objective, and every candidate with its results; overlapping windows are rejected.

mod bar_support;
mod replay_support;

use std::collections::BTreeMap;

use ficant_application::{
    ApplicationError, BarReplayRequest, CalibrationRequest, calibrate_policy, replay_bars,
};
use ficant_domain::market::{BarFrequency, BarSettlement};
use ficant_domain::primitives::{ContentHash, EffectivePeriod, Ulid};
use ficant_domain::replay::{
    COST_NONE, CalibrationObjective, ExecutionRules, MAX_CALIBRATION_CANDIDATES, ParameterGrid,
    PolicyArtifact, PolicyArtifactInput, RemainderHandling, ReplayAmount, YIELD_THRESHOLD_FAMILY,
    YieldThresholdPolicy,
};
use ficant_domain::subject::{FundingTier, SubjectStateSnapshot, SubjectVersion};

use bar_support::*;
use replay_support::*;

/// A buy signal on each day: 1.845 on 07-20 and 1.848 on 07-21 (entry 1.84 buys, entry 1.85
/// does not). Day 1 buys execute at VWAP 100.10 and close the window at 100.20 (a gain); day 2
/// buys at VWAP 100.55 and close at 100.50 (a loss), so validation favours not trading.
const ROWS: &str = r#"
{"id":"d1-0930","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:30:00","pub":"2026-07-20 09:31:02","o":"100.00","h":"100.00","l":"100.00","c":"100.00","vw":"100.00","vol":"20000000","ytm":"1.845"}
{"id":"d1-0931","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:31:00","pub":"2026-07-20 09:32:02","o":"100.05","h":"100.05","l":"100.05","c":"100.05","vw":"100.05","vol":"20000000","ytm":"1.838"}
{"id":"d1-0932","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:32:00","pub":"2026-07-20 09:33:02","o":"100.10","h":"100.20","l":"100.10","c":"100.20","vw":"100.10","vol":"30000000","ytm":"1.836"}
{"id":"d2-0930","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-21 09:30:00","pub":"2026-07-21 09:31:02","o":"100.40","h":"100.40","l":"100.40","c":"100.40","vw":"100.40","vol":"20000000","ytm":"1.848"}
{"id":"d2-0931","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-21 09:31:00","pub":"2026-07-21 09:32:02","o":"100.50","h":"100.50","l":"100.50","c":"100.50","vw":"100.50","vol":"20000000","ytm":"1.838"}
{"id":"d2-0932","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-21 09:32:00","pub":"2026-07-21 09:33:02","o":"100.50","h":"100.60","l":"100.50","c":"100.50","vw":"100.55","vol":"30000000","ytm":"1.836"}
"#;

fn period(from: &str, to: &str) -> EffectivePeriod {
    EffectivePeriod::new(market_time(from), market_time(to)).unwrap()
}

fn training() -> EffectivePeriod {
    period("2026-07-20T00:00:00Z", "2026-07-21T00:00:00Z")
}

fn validation() -> EffectivePeriod {
    period("2026-07-21T00:00:00Z", "2026-07-22T00:00:00Z")
}

fn base_policy() -> PolicyArtifact {
    PolicyArtifact::new(PolicyArtifactInput {
        policy_artifact_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FP0").unwrap(),
        owner: owner(),
        family: YIELD_THRESHOLD_FAMILY.to_owned(),
        family_version: 1,
        parameters: BTreeMap::from([
            ("entry_yield".to_owned(), amount("1.84")),
            ("exit_yield".to_owned(), amount("1.82")),
            ("target_face".to_owned(), amount("1000000")),
        ]),
        universe: vec![instrument()],
        frequency: BarFrequency::M1,
        settlement: BarSettlement::T1,
        valid: period("2026-07-01T00:00:00Z", "2026-08-01T00:00:00Z"),
        execution: ExecutionRules::new(amount("1"), RemainderHandling::Cancel).unwrap(),
        constraints: BTreeMap::new(),
        calibration: None,
    })
    .unwrap()
}

fn grid(axes: &[(&str, &[&str])]) -> ParameterGrid {
    ParameterGrid::new(
        axes.iter()
            .map(|(name, values)| {
                (
                    (*name).to_owned(),
                    values.iter().map(|value| amount(value)).collect(),
                )
            })
            .collect(),
    )
    .unwrap()
}

/// `entry_yield` × `target_face`: (1.84, 5m), (1.84, 10m), (1.85, 5m), (1.85, 10m).
fn four_candidates() -> ParameterGrid {
    grid(&[
        ("entry_yield", &["1.84", "1.85"]),
        ("target_face", &["5000000", "10000000"]),
    ])
}

struct Fixture {
    data: SnapshotBars,
    policy: PolicyArtifact,
    subject: SubjectVersion,
    state: SubjectStateSnapshot,
    carry: TableCarry,
}

impl Fixture {
    async fn new() -> Self {
        let subject = subject(1, FundingTier::DrAvailable);
        Self {
            data: SnapshotBars(two_day_snapshot(ROWS, "01ARZ3NDEKTSV4RRFFQ69G5FS7").await),
            policy: base_policy(),
            state: state(&subject, "01ARZ3NDEKTSV4RRFFQ69G5FV1"),
            subject,
            carry: TableCarry::new(&[("2026-07-20", "1.50"), ("2026-07-21", "0.02")], &[]),
        }
    }

    fn replay_request<'a>(&'a self, policy: &'a PolicyArtifact) -> BarReplayRequest<'a> {
        BarReplayRequest {
            policy,
            evaluator: &YieldThresholdPolicy,
            data: &self.data,
            subject: &self.subject,
            subject_state: &self.state,
            constraints: None,
            funding: funding("0.0365", 365),
            carry: &self.carry,
            cost_model: COST_NONE.to_owned(),
            cost_parameters: BTreeMap::new(),
            seed: 7,
            code: ContentHash::digest(b"code"),
            runtime: ContentHash::digest(b"runtime"),
        }
    }

    fn calibrate(
        &self,
        grid: ParameterGrid,
        training: EffectivePeriod,
        validation: EffectivePeriod,
        objective: &str,
    ) -> Result<PolicyArtifact, ApplicationError> {
        calibrate_policy(&CalibrationRequest {
            replay: self.replay_request(&self.policy),
            grid,
            training,
            validation,
            objective: objective.to_owned(),
            output_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FP1").unwrap(),
            output_valid: period("2026-07-22T00:00:00Z", "2026-08-01T00:00:00Z"),
        })
    }
}

const TOTAL_PNL: &str = "ficant.objective.total-pnl";

#[tokio::test]
async fn the_calibrated_artifact_records_windows_objective_and_every_candidate() {
    // Training (07-20): buying at 100.10 and closing at 100.20 earns 0.10% of face:
    //   (1.84, 5m) 5,000; (1.84, 10m) 10,000; entry 1.85 never buys: 0, 0.
    // Validation (07-21): buying at 100.55 and closing at 100.50 loses 0.05% of face:
    //   (1.84, 5m) -2,500; (1.84, 10m) -5,000; 0, 0.
    let fixture = Fixture::new().await;
    let calibrated = fixture
        .calibrate(four_candidates(), training(), validation(), TOTAL_PNL)
        .unwrap();
    let record = calibrated.calibration().unwrap();
    assert_eq!(record.training(), &training());
    assert_eq!(record.validation(), &validation());
    assert_eq!(record.objective(), CalibrationObjective::TotalPnl);
    assert_eq!(record.base_policy(), fixture.policy.content_hash());
    assert_eq!(record.grid(), &four_candidates());

    let summary: Vec<(String, String, String, String)> = record
        .candidates()
        .iter()
        .map(|candidate| {
            (
                candidate.parameters["entry_yield"].canonical(),
                candidate.parameters["target_face"].canonical(),
                candidate.training.objective.canonical(),
                candidate.validation.objective.canonical(),
            )
        })
        .collect();
    let row = |a: &str, b: &str, c: &str, d: &str| {
        (a.to_owned(), b.to_owned(), c.to_owned(), d.to_owned())
    };
    assert_eq!(
        summary,
        vec![
            row("1.84", "5000000", "5000", "-2500"),
            row("1.84", "10000000", "10000", "-5000"),
            row("1.85", "5000000", "0", "0"),
            row("1.85", "10000000", "0", "0"),
        ]
    );
    // Parameters the grid does not cover keep the base value in every candidate.
    assert!(
        record
            .candidates()
            .iter()
            .all(|candidate| candidate.parameters["exit_yield"] == amount("1.82"))
    );

    // Selected on training only, although validation would have preferred not trading; the
    // artifact carries exactly the selected parameters.
    assert_eq!(record.selected(), 1);
    assert_eq!(
        calibrated.parameter("target_face"),
        Some(amount("10000000"))
    );
    assert_eq!(calibrated.parameter("entry_yield"), Some(amount("1.84")));
    assert_eq!(
        calibrated.id(),
        &Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FP1").unwrap()
    );
    assert_eq!(
        calibrated.valid(),
        &period("2026-07-22T00:00:00Z", "2026-08-01T00:00:00Z")
    );
}

#[tokio::test]
async fn every_recorded_result_is_the_candidates_own_replay() {
    let fixture = Fixture::new().await;
    let calibrated = fixture
        .calibrate(four_candidates(), training(), validation(), TOTAL_PNL)
        .unwrap();
    for candidate in calibrated.calibration().unwrap().candidates() {
        for (window, recorded) in [
            (training(), &candidate.training),
            (validation(), &candidate.validation),
        ] {
            let mut input = fixture.policy.input().clone();
            input.parameters = candidate.parameters.clone();
            input.valid = window;
            let policy = PolicyArtifact::new(input).unwrap();
            let replay = replay_bars(&fixture.replay_request(&policy)).unwrap();
            assert_eq!(replay.result_id(), &recorded.result_id);
            assert_eq!(
                replay.pnl().last().map(|row| row.equity),
                Some(recorded.objective)
            );
        }
    }
}

#[tokio::test]
async fn the_same_request_gives_the_same_artifact() {
    let fixture = Fixture::new().await;
    let first = fixture
        .calibrate(four_candidates(), training(), validation(), TOTAL_PNL)
        .unwrap();
    let second = fixture
        .calibrate(four_candidates(), training(), validation(), TOTAL_PNL)
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.content_hash(), second.content_hash());
    // A different validation window changes the record and so the artifact's identity.
    let shifted = fixture
        .calibrate(
            four_candidates(),
            training(),
            period("2026-07-21T01:00:00Z", "2026-07-22T00:00:00Z"),
            TOTAL_PNL,
        )
        .unwrap();
    assert_ne!(shifted.content_hash(), first.content_hash());
}

#[tokio::test]
async fn overlapping_or_backward_windows_are_rejected() {
    let fixture = Fixture::new().await;
    // Validation starts before training ends.
    assert!(
        fixture
            .calibrate(
                four_candidates(),
                period("2026-07-20T00:00:00Z", "2026-07-21T02:00:00Z"),
                validation(),
                TOTAL_PNL,
            )
            .is_err()
    );
    // Validation entirely before training.
    assert!(
        fixture
            .calibrate(four_candidates(), validation(), training(), TOTAL_PNL)
            .is_err()
    );
    // Touching windows are fine (training ends exactly where validation starts).
    assert!(
        fixture
            .calibrate(four_candidates(), training(), validation(), TOTAL_PNL)
            .is_ok()
    );
}

#[tokio::test]
async fn bad_requests_fail_closed() {
    let fixture = Fixture::new().await;
    assert!(
        fixture
            .calibrate(
                four_candidates(),
                training(),
                validation(),
                "ficant.objective.sharpe"
            )
            .is_err()
    );
    // A window without any bar cannot be replayed, so nothing is calibrated.
    assert!(
        fixture
            .calibrate(
                four_candidates(),
                training(),
                period("2026-07-23T00:00:00Z", "2026-07-24T00:00:00Z"),
                TOTAL_PNL,
            )
            .is_err()
    );

    let axes = |values: Vec<&str>| {
        BTreeMap::from([(
            "entry_yield".to_owned(),
            values
                .into_iter()
                .map(amount)
                .collect::<Vec<ReplayAmount>>(),
        )])
    };
    assert!(ParameterGrid::new(BTreeMap::new()).is_err());
    assert!(ParameterGrid::new(axes(vec![])).is_err());
    assert!(ParameterGrid::new(axes(vec!["1.84", "1.840"])).is_err());
    let too_many: BTreeMap<String, Vec<ReplayAmount>> =
        ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"]
            .iter()
            .map(|name| ((*name).to_owned(), vec![amount("1"), amount("2")]))
            .collect();
    assert!(2_usize.pow(11) > MAX_CALIBRATION_CANDIDATES);
    assert!(ParameterGrid::new(too_many).is_err());
}

#[tokio::test]
async fn ties_go_to_the_first_candidate_and_parameters_must_match_the_selection() {
    let fixture = Fixture::new().await;
    // Neither entry level buys on day 1: both score 0 and the first is selected.
    let calibrated = fixture
        .calibrate(
            grid(&[("entry_yield", &["1.86", "1.85"])]),
            training(),
            validation(),
            TOTAL_PNL,
        )
        .unwrap();
    let record = calibrated.calibration().unwrap();
    assert_eq!(record.selected(), 0);
    assert_eq!(calibrated.parameter("entry_yield"), Some(amount("1.86")));

    // An artifact whose parameters differ from its selected candidate cannot be built.
    let mut forged = calibrated.input().clone();
    forged
        .parameters
        .insert("entry_yield".to_owned(), amount("1.85"));
    assert!(PolicyArtifact::new(forged).is_err());
}
