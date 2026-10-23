//! AC38 and AC39: point-in-time bar replay over verified Bar snapshots.

mod bar_support;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, BarReplayRequest, PointInTimeBars, replay_bars,
};
use ficant_data::{BarSnapshotCodec, CanonicalBarIngestor, DataError, VerifiedBarSnapshot};
use ficant_domain::ContentAddressed;
use ficant_domain::market::{Bar, BarFrequency, BarSettlement};
use ficant_domain::primitives::{ContentHash, EffectivePeriod, Ulid, Version, VersionRef};
use ficant_domain::research::{
    FillPriceBasis, NoFillReason, PointInTimeView, PolicyArtifact, PolicyArtifactInput,
    PolicyEvaluator, ReplayAmount, SimulationResult, YIELD_THRESHOLD_FAMILY, YieldThresholdPolicy,
};

use bar_support::*;

/// Minute bars for 260011.IB (T+1), each visible two seconds after it ends. Entry yield 1.84,
/// exit 1.82, target 10m face. Expected path:
/// - 09:31:02 sees 1.845 → buy 10m, executed in the 09:32 bar at its VWAP 100.13;
/// - 09:35:02 sees 1.815 → sell, but the 09:36 execution bar has no trades → no fill;
/// - 09:37:02 sees 1.816 and the unfilled sell has left the book → sell again, executed in the
///   09:38 bar at its close 100.32 (no VWAP).
const ROWS: &str = r#"
{"id":"b0930","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:30:00","pub":"2026-07-20 09:31:02","o":"100.10","h":"100.10","l":"100.10","c":"100.10","vw":"100.10","vol":"20000000","ytm":"1.845"}
{"id":"b0931","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:31:00","pub":"2026-07-20 09:32:02","o":"100.12","h":"100.12","l":"100.12","c":"100.12","vw":"100.12","vol":"20000000","ytm":"1.838"}
{"id":"b0932","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:32:00","pub":"2026-07-20 09:33:02","o":"100.12","h":"100.14","l":"100.12","c":"100.14","vw":"100.13","vol":"30000000","ytm":"1.836"}
{"id":"b0933","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:33:00","pub":"2026-07-20 09:34:02","vol":"0"}
{"id":"b0934","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:34:00","pub":"2026-07-20 09:35:02","o":"100.30","h":"100.30","l":"100.30","c":"100.30","vol":"10000000","ytm":"1.815"}
{"id":"b0935","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:35:00","pub":"2026-07-20 09:36:02","o":"100.29","h":"100.29","l":"100.29","c":"100.29","vol":"10000000","ytm":"1.816"}
{"id":"b0936","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:36:00","pub":"2026-07-20 09:37:02","vol":"0"}
{"id":"b0937","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:37:00","pub":"2026-07-20 09:38:02","o":"100.31","h":"100.31","l":"100.31","c":"100.31","vw":"100.31","vol":"10000000","ytm":"1.814"}
{"id":"b0938","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:38:00","pub":"2026-07-20 09:39:02","o":"100.32","h":"100.32","l":"100.32","c":"100.32","vol":"10000000","ytm":"1.813"}
"#;

/// One more bar, visible at 09:40:02, whose yield would make the policy buy again.
const LATER_BAR: &str = r#"{"id":"b0939","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:39:00","pub":"2026-07-20 09:40:02","o":"100.05","h":"100.05","l":"100.05","c":"100.05","vw":"100.05","vol":"10000000","ytm":"1.850"}"#;

struct SnapshotBars(VerifiedBarSnapshot);

impl PointInTimeBars for SnapshotBars {
    fn snapshot(&self) -> (Ulid, ContentHash) {
        (
            self.0.snapshot().id().clone(),
            self.0.snapshot().content_hash().clone(),
        )
    }

    fn all_bars(&self) -> &[Bar] {
        self.0.bars()
    }

    fn visible_at(&self, decision_time: DateTime<Utc>) -> Result<Vec<&Bar>, ApplicationError> {
        self.0.visible_at(decision_time).map_err(|_: DataError| {
            ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
        })
    }
}

/// A broken data port that hands the policy every bar, including future ones.
struct LeakyBars<'a>(&'a VerifiedBarSnapshot);

impl PointInTimeBars for LeakyBars<'_> {
    fn snapshot(&self) -> (Ulid, ContentHash) {
        (
            self.0.snapshot().id().clone(),
            self.0.snapshot().content_hash().clone(),
        )
    }

    fn all_bars(&self) -> &[Bar] {
        self.0.bars()
    }

    fn visible_at(&self, _: DateTime<Utc>) -> Result<Vec<&Bar>, ApplicationError> {
        Ok(self.0.bars().iter().collect())
    }
}

async fn snapshot(rows: &str, snapshot_id: &str) -> VerifiedBarSnapshot {
    let mut mapping = vendor_a_mapping();
    mapping.vwap = Some("vw".to_owned());
    let source = file_source(mapping, SOURCE_A, rows);
    let request = request(SOURCE_A, "260011.IB");
    let batch = CanonicalBarIngestor
        .ingest(&source, &request)
        .await
        .unwrap();
    assert!(
        batch.report().rejections.is_empty(),
        "{:?}",
        batch.report().rejections
    );
    let package = BarSnapshotCodec
        .build(Ulid::new(snapshot_id).unwrap(), &request, &batch)
        .unwrap();
    BarSnapshotCodec
        .decode_verified(
            package.snapshot().clone(),
            package.parquet(),
            package.manifest(),
        )
        .unwrap()
}

fn amount(text: &str) -> ReplayAmount {
    ReplayAmount::parse(text).unwrap()
}

fn instrument() -> VersionRef {
    VersionRef::new(Ulid::new(INSTRUMENT).unwrap(), Version::new(7).unwrap())
}

fn policy(entry_yield: &str) -> PolicyArtifact {
    PolicyArtifact::new(PolicyArtifactInput {
        policy_artifact_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FP0").unwrap(),
        owner: owner(),
        family: YIELD_THRESHOLD_FAMILY.to_owned(),
        family_version: 1,
        parameters: BTreeMap::from([
            ("entry_yield".to_owned(), amount(entry_yield)),
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
    })
    .unwrap()
}

#[derive(Clone)]
struct Inputs {
    policy: PolicyArtifact,
    subject: VersionRef,
    subject_state: VersionRef,
    cost_parameters: BTreeMap<String, ReplayAmount>,
    seed: u64,
    code: ContentHash,
    runtime: ContentHash,
}

fn base_inputs() -> Inputs {
    Inputs {
        policy: policy("1.84"),
        subject: VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FV0").unwrap(),
            Version::new(1).unwrap(),
        ),
        subject_state: VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FV1").unwrap(),
            Version::new(1).unwrap(),
        ),
        cost_parameters: BTreeMap::from([("spread_bp".to_owned(), amount("0"))]),
        seed: 7,
        code: ContentHash::digest(b"code"),
        runtime: ContentHash::digest(b"runtime"),
    }
}

fn run(data: &dyn PointInTimeBars, inputs: &Inputs) -> Result<SimulationResult, ApplicationError> {
    replay_bars(&BarReplayRequest {
        policy: &inputs.policy,
        evaluator: &YieldThresholdPolicy,
        data,
        subject: inputs.subject.clone(),
        subject_state: inputs.subject_state.clone(),
        cost_model: "ficant.cost.none".to_owned(),
        cost_parameters: inputs.cost_parameters.clone(),
        seed: inputs.seed,
        code: inputs.code.clone(),
        runtime: inputs.runtime.clone(),
    })
}

#[tokio::test]
async fn replay_follows_the_policy_with_next_bar_execution() {
    let data = SnapshotBars(snapshot(ROWS, "01ARZ3NDEKTSV4RRFFQ69G5FS1").await);
    let result = run(&data, &base_inputs()).unwrap();

    // One decision per bar, at its visibility time.
    assert_eq!(result.decisions().len(), 9);
    assert_eq!(
        result.decisions()[0].decision_time,
        utc("2026-07-20T01:31:02Z")
    );

    let fills = result.fills();
    assert_eq!(fills.len(), 3, "{fills:#?}");
    let buy = &fills[0];
    assert_eq!(buy.decision_time, utc("2026-07-20T01:31:02Z"));
    assert_eq!(buy.quantity, amount("10000000"));
    assert_eq!(
        buy.execution_bar.as_deref(),
        Some("b0932"),
        "never the decision bar"
    );
    assert_eq!(buy.price, Some(amount("100.13")));
    assert_eq!(buy.basis, Some(FillPriceBasis::Vwap));
    assert_eq!(buy.effective_at, utc("2026-07-20T01:33:00Z"));

    let blocked = &fills[1];
    assert_eq!(blocked.decision_time, utc("2026-07-20T01:35:02Z"));
    assert_eq!(blocked.execution_bar.as_deref(), Some("b0936"));
    assert_eq!(blocked.no_fill, Some(NoFillReason::NoPriceInBar));

    let sell = &fills[2];
    assert_eq!(sell.decision_time, utc("2026-07-20T01:37:02Z"));
    assert_eq!(sell.quantity, amount("-10000000"));
    assert_eq!(sell.execution_bar.as_deref(), Some("b0938"));
    assert_eq!(sell.price, Some(amount("100.32")));
    assert_eq!(sell.basis, Some(FillPriceBasis::Close));

    // Flat at the end; cash = -10m × 100.13% + 10m × 100.32% = 19,000.
    assert_eq!(result.book().positions().count(), 0);
    assert_eq!(result.book().cash(), amount("19000"));
}

#[tokio::test]
async fn every_decision_uses_only_records_visible_at_its_time() {
    // AC38, first half.
    let data = SnapshotBars(snapshot(ROWS, "01ARZ3NDEKTSV4RRFFQ69G5FS1").await);
    let result = run(&data, &base_inputs()).unwrap();
    for (index, decision) in result.decisions().iter().enumerate() {
        let through = decision.inputs_visible_through.unwrap();
        assert!(through <= decision.decision_time);
        assert_eq!(decision.input_bar_count, index + 1);
    }

    // A data port that leaks future bars cannot get them to the policy: the view refuses them.
    let snapshot = snapshot(ROWS, "01ARZ3NDEKTSV4RRFFQ69G5FS1").await;
    let leaky = LeakyBars(&snapshot);
    assert!(run(&leaky, &base_inputs()).is_err());
    let future: Vec<&Bar> = snapshot.bars().iter().collect();
    assert!(PointInTimeView::new(utc("2026-07-20T01:31:02Z"), future).is_err());
}

#[tokio::test]
async fn appending_a_later_visible_record_leaves_earlier_results_unchanged() {
    // AC38, second half: the extra bar is visible at 09:40:02.
    let base = SnapshotBars(snapshot(ROWS, "01ARZ3NDEKTSV4RRFFQ69G5FS1").await);
    let extended_rows = format!("{}\n{LATER_BAR}\n", ROWS.trim_end());
    let extended = SnapshotBars(snapshot(&extended_rows, "01ARZ3NDEKTSV4RRFFQ69G5FS1").await);
    let base = run(&base, &base_inputs()).unwrap();
    let extended = run(&extended, &base_inputs()).unwrap();

    let later_visible = utc("2026-07-20T01:40:02Z");
    let mut checked = 0;
    for decision in base.decisions() {
        let t = decision.decision_time;
        assert!(t < later_visible);
        assert_eq!(
            base.through(t),
            extended.through(t),
            "results through {t} differ"
        );
        checked += 1;
    }
    assert_eq!(checked, 9);
    // After the new bar becomes visible the policy reacts, so the full results do differ.
    assert_eq!(extended.decisions().len(), 10);
    assert_ne!(base.result_id(), extended.result_id());
    assert_ne!(base.through(later_visible), extended.through(later_visible));
}

#[tokio::test]
async fn identical_inputs_give_identical_results_and_any_change_changes_the_id() {
    // AC39.
    let data = SnapshotBars(snapshot(ROWS, "01ARZ3NDEKTSV4RRFFQ69G5FS1").await);
    let base = base_inputs();
    let first = run(&data, &base).unwrap();
    let second = run(&data, &base).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.result_id(), second.result_id());

    let other_snapshot = SnapshotBars(snapshot(ROWS, "01ARZ3NDEKTSV4RRFFQ69G5FS2").await);
    let mut variants: Vec<(&str, SimulationResult)> = Vec::new();
    variants.push(("snapshot", run(&other_snapshot, &base).unwrap()));
    let change = |edit: fn(&mut Inputs)| {
        let mut inputs = base.clone();
        edit(&mut inputs);
        run(&data, &inputs).unwrap()
    };
    variants.push(("policy", change(|inputs| inputs.policy = policy("1.841"))));
    variants.push((
        "subject",
        change(|inputs| {
            inputs.subject = VersionRef::new(inputs.subject.id().clone(), Version::new(2).unwrap());
        }),
    ));
    variants.push((
        "subject state",
        change(|inputs| {
            inputs.subject_state =
                VersionRef::new(inputs.subject_state.id().clone(), Version::new(2).unwrap());
        }),
    ));
    variants.push((
        "cost parameters",
        change(|inputs| {
            inputs
                .cost_parameters
                .insert("spread_bp".to_owned(), ReplayAmount::parse("0.5").unwrap());
        }),
    ));
    variants.push(("seed", change(|inputs| inputs.seed = 8)));
    variants.push((
        "code",
        change(|inputs| inputs.code = ContentHash::digest(b"other")),
    ));
    variants.push((
        "runtime",
        change(|inputs| inputs.runtime = ContentHash::digest(b"other")),
    ));

    let mut ids = vec![first.result_id().clone()];
    for (name, result) in &variants {
        assert_ne!(
            result.result_id(),
            first.result_id(),
            "{name} did not change the ID"
        );
        ids.push(result.result_id().clone());
    }
    let unique: std::collections::BTreeSet<_> = ids.iter().map(ContentHash::as_bytes).collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "two different inputs produced the same ID"
    );
    // Changing the policy only slightly still yields the same trades here, so the ID change comes
    // from the identity itself, not only from different outputs.
    assert_eq!(variants[1].1.fills(), first.fills());
}

#[tokio::test]
async fn replay_fails_closed_on_mismatched_evaluator_or_missing_parameters() {
    struct OtherFamily;
    impl PolicyEvaluator for OtherFamily {
        #[allow(clippy::unnecessary_literal_bound)]
        fn family(&self) -> &str {
            "ficant.policy.other"
        }
        fn family_version(&self) -> u32 {
            1
        }
        fn decide(
            &self,
            _: &PolicyArtifact,
            _: &PointInTimeView<'_>,
            _: &ficant_domain::research::SimulatedBook,
        ) -> Result<ficant_domain::research::TargetExposure, ficant_domain::research::PolicyError>
        {
            unreachable!("must not be called for a mismatched family")
        }
    }
    let data = SnapshotBars(snapshot(ROWS, "01ARZ3NDEKTSV4RRFFQ69G5FS1").await);
    let inputs = base_inputs();
    let mismatched = replay_bars(&BarReplayRequest {
        policy: &inputs.policy,
        evaluator: &OtherFamily,
        data: &data,
        subject: inputs.subject.clone(),
        subject_state: inputs.subject_state.clone(),
        cost_model: "ficant.cost.none".to_owned(),
        cost_parameters: inputs.cost_parameters.clone(),
        seed: inputs.seed,
        code: inputs.code.clone(),
        runtime: inputs.runtime.clone(),
    });
    assert!(mismatched.is_err());

    let mut missing = base_inputs();
    missing.policy = PolicyArtifact::new(PolicyArtifactInput {
        parameters: BTreeMap::from([("entry_yield".to_owned(), amount("1.84"))]),
        ..policy_input()
    })
    .unwrap();
    assert!(run(&data, &missing).is_err());
}

fn policy_input() -> PolicyArtifactInput {
    PolicyArtifactInput {
        policy_artifact_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FP0").unwrap(),
        owner: owner(),
        family: YIELD_THRESHOLD_FAMILY.to_owned(),
        family_version: 1,
        parameters: BTreeMap::new(),
        universe: vec![instrument()],
        frequency: BarFrequency::M1,
        settlement: BarSettlement::T1,
        valid: EffectivePeriod::new(
            market_time("2026-07-01T00:00:00Z"),
            market_time("2026-08-01T00:00:00Z"),
        )
        .unwrap(),
    }
}
