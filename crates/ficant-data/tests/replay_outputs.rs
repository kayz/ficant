//! AC44: replay outputs are `SimulationResult`s (decisions carry `TargetExposure`s) whose full
//! lineage can be expanded, and no output contains an order object.

mod bar_support;
mod replay_support;

use std::collections::BTreeMap;
use std::path::Path;

use ficant_application::{BarReplayRequest, replay_bars};
use ficant_domain::ContentAddressed;
use ficant_domain::constraint::{ConstraintDefinition, ConstraintMeasure, ConstraintSet};
use ficant_domain::market::{BarFrequency, BarSettlement};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, LineageRef, Ulid, Version, VersionRef,
};
use ficant_domain::replay::{
    BondCarry, COST_NONE, ConstraintHandling, ExecutionRules, LineageRole, PolicyArtifact,
    PolicyArtifactInput, RemainderHandling, SimulationResult, TargetExposure,
    YIELD_THRESHOLD_FAMILY, YieldThresholdPolicy, subject_state_hash,
};
use ficant_domain::subject::{FundingTier, SubjectStateSnapshot, SubjectVersion};

use bar_support::*;
use replay_support::*;

fn rule_pack() -> VersionRef {
    VersionRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FC0").unwrap(),
        Version::new(1).unwrap(),
    )
}

fn policy(constraints: BTreeMap<String, ConstraintHandling>) -> PolicyArtifact {
    PolicyArtifact::new(PolicyArtifactInput {
        policy_artifact_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FP0").unwrap(),
        owner: owner(),
        family: YIELD_THRESHOLD_FAMILY.to_owned(),
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
        execution: ExecutionRules::new(amount("1"), RemainderHandling::Cancel).unwrap(),
        constraints,
        calibration: None,
    })
    .unwrap()
}

struct Inputs {
    data: SnapshotBars,
    policy: PolicyArtifact,
    subject: SubjectVersion,
    state: SubjectStateSnapshot,
    constraints: Option<ConstraintSet>,
    carry: TableCarry,
}

impl Inputs {
    fn run(&self) -> SimulationResult {
        replay_bars(&BarReplayRequest {
            policy: &self.policy,
            evaluator: &YieldThresholdPolicy,
            data: &self.data,
            subject: &self.subject,
            subject_state: &self.state,
            constraints: self.constraints.as_ref(),
            funding: funding("0.0365", 365),
            carry: &self.carry,
            cost_model: COST_NONE.to_owned(),
            cost_parameters: BTreeMap::new(),
            seed: 7,
            code: ContentHash::digest(b"code"),
            runtime: ContentHash::digest(b"runtime"),
        })
        .unwrap()
    }
}

async fn constrained(single_ceiling: &str) -> Inputs {
    let subject = subject_bound(1, &rule_pack());
    let state = state_with(
        &subject,
        "01ARZ3NDEKTSV4RRFFQ69G5FV1",
        &[("bond.single_issue_face", single_ceiling)],
    );
    Inputs {
        data: SnapshotBars(snapshot(ONE_DAY, "01ARZ3NDEKTSV4RRFFQ69G5FS8").await),
        policy: policy(BTreeMap::from([(
            "bond.single_issue_face".to_owned(),
            ConstraintHandling::Reduce,
        )])),
        subject,
        state,
        constraints: Some(
            ConstraintSet::new(
                rule_pack(),
                vec![
                    ConstraintDefinition::new(
                        "bond.single_issue_face",
                        ConstraintMeasure::InstrumentFace,
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
        ),
        carry: TableCarry::new(&[("2026-07-20", "1.00")], &[]),
    }
}

#[tokio::test]
async fn the_result_expands_into_every_input_it_was_computed_from() {
    let inputs = constrained("100000000").await;
    let result = inputs.run();
    let lineage = result.lineage();
    let roles: Vec<LineageRole> = lineage.iter().map(|item| item.role).collect();
    assert_eq!(
        roles,
        vec![
            LineageRole::Policy,
            LineageRole::DataSnapshot,
            LineageRole::Subject,
            LineageRole::SubjectState,
            LineageRole::ConstraintSet,
            LineageRole::FundingRulePack,
            LineageRole::CouponData,
        ]
    );
    let reference = |role: LineageRole| {
        lineage
            .iter()
            .find(|item| item.role == role)
            .unwrap()
            .reference
            .clone()
    };
    let hashed =
        |id: &Ulid, hash: &ContentHash| LineageRef::content_addressed(id.clone(), hash.clone());
    let versioned = |value: &VersionRef| LineageRef::versioned(value.id().clone(), value.version());
    assert_eq!(
        reference(LineageRole::Policy),
        hashed(inputs.policy.id(), inputs.policy.content_hash())
    );
    assert_eq!(
        reference(LineageRole::DataSnapshot),
        hashed(
            inputs.data.0.snapshot().id(),
            inputs.data.0.snapshot().content_hash()
        )
    );
    assert_eq!(
        reference(LineageRole::Subject),
        versioned(inputs.subject.reference())
    );
    assert_eq!(
        reference(LineageRole::SubjectState),
        hashed(inputs.state.id(), &subject_state_hash(&inputs.state))
    );
    assert_eq!(
        reference(LineageRole::ConstraintSet),
        versioned(&rule_pack())
    );
    assert_eq!(
        reference(LineageRole::FundingRulePack),
        versioned(funding("0.0365", 365).rule_pack())
    );
    assert_eq!(
        reference(LineageRole::CouponData),
        LineageRef::new(
            inputs.carry.source().id().clone(),
            Some(inputs.carry.source().version()),
            Some(inputs.carry.identity()),
        )
        .unwrap()
    );

    // Decisions are target exposures, never orders.
    let first: &TargetExposure = &result.decisions()[0].target;
    assert_eq!(first.get(&instrument()), Some(amount("10000000")));
}

#[tokio::test]
async fn lineage_follows_the_inputs() {
    // A different ceiling under the same snapshot ID is a different state: the state reference
    // and the result ID both change.
    let loose = constrained("100000000").await.run();
    let tight = constrained("6000000").await.run();
    let state = |result: &SimulationResult| {
        result
            .lineage()
            .into_iter()
            .find(|item| item.role == LineageRole::SubjectState)
            .unwrap()
            .reference
    };
    assert_ne!(state(&loose), state(&tight));
    assert_ne!(loose.result_id(), tight.result_id());

    // Without a constraint set there is nothing to reference for it.
    let mut unconstrained = constrained("100000000").await;
    unconstrained.subject = subject(1, FundingTier::DrAvailable);
    unconstrained.state = state_with(&unconstrained.subject, "01ARZ3NDEKTSV4RRFFQ69G5FV1", &[]);
    unconstrained.constraints = None;
    unconstrained.policy = policy(BTreeMap::new());
    assert!(
        unconstrained
            .run()
            .lineage()
            .iter()
            .all(|item| item.role != LineageRole::ConstraintSet)
    );
}

/// Identifiers in `source` (comments removed) with "order" or "orders" as one of their words.
fn order_identifiers(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in source.lines() {
        let code = line.split("//").next().unwrap_or_default();
        for identifier in code
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .filter(|token| {
                token
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic())
            })
        {
            if words(identifier)
                .iter()
                .any(|word| word == "order" || word == "orders")
            {
                found.push(identifier.to_owned());
            }
        }
    }
    found
}

/// `SimulatedOrderBook` → simulated, order, book; `order_id` → order, id.
fn words(identifier: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for character in identifier.chars() {
        if character == '_' || (character.is_ascii_uppercase() && !current.is_empty()) {
            if !current.is_empty() {
                words.push(current.to_ascii_lowercase());
            }
            current.clear();
        }
        if character != '_' {
            current.push(character);
        }
    }
    if !current.is_empty() {
        words.push(current.to_ascii_lowercase());
    }
    words
}

#[test]
fn the_gate_catches_order_objects_and_ignores_ordinary_words() {
    let planted = "pub struct SimulatedOrder;\nstruct Fill { order_id: u64 }\nenum X { Orders }\n";
    assert_eq!(
        order_identifiers(planted),
        vec!["SimulatedOrder", "order_id", "Orders"]
    );
    let legal = "let ordered = true; // never an order\nfn sort_by_order_of_time() {}\n";
    assert_eq!(order_identifiers(legal), vec!["sort_by_order_of_time"]);
    assert!(order_identifiers("let ordered = reorder(x); // Order").is_empty());
}

#[test]
fn no_replay_output_contains_an_order_object() {
    // AC44 / SPEC §2: the replay and calibration surface (domain module and use cases) declares
    // no order type, field, variant or function.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(root.join("ficant-domain/src/replay")).unwrap() {
        sources.push(entry.unwrap().path());
    }
    sources.push(root.join("ficant-application/src/use_cases/bar_replay.rs"));
    sources.push(root.join("ficant-application/src/use_cases/policy_calibration.rs"));
    assert!(
        sources.len() >= 7,
        "the gate must see the whole replay surface"
    );
    for path in sources {
        let source = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            order_identifiers(&source),
            Vec::<String>::new(),
            "{} declares order-like identifiers",
            path.display()
        );
    }
}
