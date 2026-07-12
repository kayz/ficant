use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use ficant_application::ports::{
    AppendDefinitionVersion, DefinitionIdentity, DefinitionRepository, DefinitionValue,
    Phase1RunCandidateResolver,
};
use ficant_application::{AccessScope, ApplicationError, ApplicationErrorCategory};
use ficant_domain::market::{MarketRulePack, MarketRulePackTimesInput, VerificationStatus};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_domain::research::{DataSnapshot, DataSnapshotInput, ExperimentRun, ExperimentRunInput};

#[test]
fn first_use_candidate_needs_only_definitions_and_performs_zero_mutations() {
    let definitions = Definitions::new(rule(1, 4));
    let snapshot = snapshot(3, hash(20));
    let candidate = block_on(Phase1RunCandidateResolver::new(&definitions).resolve(
        &scope(),
        run(&snapshot, hash(20)),
        &snapshot,
    ))
    .unwrap();

    assert_eq!(candidate.proof().as_of(), snapshot.as_of());
    assert_eq!(candidate.proof().snapshot_owner(), snapshot.owner());
    assert_eq!(candidate.proof().bindings().len(), 1);
    assert_eq!(definitions.reads.load(Ordering::SeqCst), 1);
    assert_eq!(definitions.mutations.load(Ordering::SeqCst), 0);
}

#[test]
fn candidate_uses_half_open_snapshot_as_of_and_rejects_reference_drift() {
    for subject in [4, 5] {
        let definitions = Definitions::new(rule(1, 4));
        let snapshot = snapshot(subject, hash(20));
        let error = block_on(Phase1RunCandidateResolver::new(&definitions).resolve(
            &scope(),
            run(&snapshot, hash(20)),
            &snapshot,
        ))
        .unwrap_err();
        assert_error(&error, ApplicationErrorCategory::ValidationFailed);
        assert_eq!(definitions.mutations.load(Ordering::SeqCst), 0);
    }

    let definitions = Definitions::new(rule(1, 4));
    let snapshot = snapshot(3, hash(20));
    let error = block_on(Phase1RunCandidateResolver::new(&definitions).resolve(
        &scope(),
        run(&snapshot, hash(99)),
        &snapshot,
    ))
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::LineageIncomplete);
    assert_eq!(definitions.reads.load(Ordering::SeqCst), 0);
    assert_eq!(definitions.mutations.load(Ordering::SeqCst), 0);
}

struct Definitions {
    rule: MarketRulePack,
    reads: AtomicUsize,
    mutations: AtomicUsize,
}

impl Definitions {
    fn new(rule: MarketRulePack) -> Self {
        Self {
            rule,
            reads: AtomicUsize::new(0),
            mutations: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl DefinitionRepository for Definitions {
    async fn create_identity(&self, _identity: DefinitionIdentity) -> Result<(), ApplicationError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }

    async fn append_version(
        &self,
        _command: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }

    async fn get_version(
        &self,
        _scope: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(
            (definition_id == id('R') && version == Version::new(1).unwrap())
                .then(|| DefinitionValue::MarketRulePack(self.rule.clone())),
        )
    }

    async fn resolve_as_of(
        &self,
        _scope: &AccessScope,
        _definition_id: Ulid,
        _instant: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Err(not_used())
    }
}

fn run(snapshot: &DataSnapshot, claimed_hash: ContentHash) -> ExperimentRun {
    ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: id('X'),
        owner: owner(),
        data_snapshot: LineageRef::content_addressed(snapshot.id().clone(), claimed_hash),
        universe_snapshot: LineageRef::content_addressed(id('U'), hash(21)),
        rule_packs: vec![VersionRef::new(id('R'), Version::new(1).unwrap())],
        runtime_image_digest: hash(22),
        parameters_hash: hash(23),
        seed: 7,
    })
    .unwrap()
}

fn snapshot(as_of: u32, content_hash: ContentHash) -> DataSnapshot {
    DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: id('D'),
        owner: owner(),
        visible_at: time(as_of + 1),
        as_of: time(as_of),
        schema_hash: hash(10),
        manifest_hash: hash(11),
        blob_content_hash: content_hash,
        lineage: vec![LineageRef::versioned(id('I'), Version::new(1).unwrap())],
    })
    .unwrap()
}

fn rule(from: u32, to: u32) -> MarketRulePack {
    MarketRulePack::new_with_times(MarketRulePackTimesInput {
        rule_pack_id: id('R'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        market: "XSHG".to_owned(),
        rule_type: "research".to_owned(),
        source: "test".to_owned(),
        from: time(from),
        to: time(to),
        verification_status: VerificationStatus::Verified,
        content_hash: hash(30),
    })
    .unwrap()
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('A'), vec![id('O')]).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('O'))
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => 'J',
        'O' => 'Q',
        'U' => 'W',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-04".parse().unwrap(),
    )
    .unwrap()
}

fn hash(value: u8) -> ContentHash {
    ContentHash::digest(&[value])
}

fn assert_error(error: &ApplicationError, category: ApplicationErrorCategory) {
    assert_eq!(error.category(), category);
    assert!(!error.retryable());
}

fn not_used() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}
