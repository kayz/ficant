use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use ficant_application::ports::{
    AppendDefinitionVersion, AppendMarketFact, DefinitionIdentity, DefinitionRepository,
    DefinitionValue, FullyValidatedMarketFact, MarketFact, MarketFactRulePackResolver,
    MarketFactRuleProofKind, MarketFactUnitResolver, MarketRunRulePackResolver, PublishSnapshot,
    SnapshotRepository, SnapshotValue, ValidatedExperimentRun,
};
use ficant_application::{AccessScope, ApplicationError, ApplicationErrorCategory, IdempotencyKey};
use ficant_domain::ContentAddressed;
use ficant_domain::VersionedDefinition;
use ficant_domain::market::{
    FactSource, MarketRulePack, MarketRulePackTimesInput, Unit, UnitInput, Valuation,
    ValuationInput, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{DataSnapshot, DataSnapshotInput, ExperimentRun, ExperimentRunInput};

#[test]
fn valuation_rule_pack_uses_valuation_at_and_half_open_interval() {
    let definitions = Definitions::new([
        DefinitionValue::Unit(price_unit()),
        DefinitionValue::MarketRulePack(rule('R', 1, 3)),
    ]);

    let accepted = resolve_valuation(&definitions, 1).unwrap();
    assert_eq!(
        accepted.rule_proof().kind(),
        MarketFactRuleProofKind::Valuation
    );
    assert_eq!(
        accepted.rule_proof().valuation().unwrap().subject(),
        &time(1)
    );
    AppendMarketFact::new(accepted, key("accepted")).unwrap();
    assert_eq!(definitions.mutations.load(Ordering::SeqCst), 0);

    for boundary in [3, 4] {
        let error = resolve_valuation(&definitions, boundary).unwrap_err();
        assert_error(&error, ApplicationErrorCategory::ValidationFailed);
    }
}

#[test]
fn run_proof_resolves_snapshot_as_of_and_every_ordered_exact_reference() {
    let definitions = Definitions::new([
        DefinitionValue::MarketRulePack(rule('R', 1, 5)),
        DefinitionValue::MarketRulePack(rule('S', 2, 4)),
    ]);
    let snapshot = data_snapshot(3, hash(20));
    let snapshots = Snapshots::new(snapshot.clone());
    let references = vec![version_ref('S', 1), version_ref('R', 1)];
    let validated = resolve_run(&definitions, &snapshots, run(&snapshot, references)).unwrap();

    assert_eq!(validated.proof().as_of(), snapshot.as_of());
    assert_eq!(
        validated
            .proof()
            .bindings()
            .iter()
            .map(|binding| binding.rule_pack().clone())
            .collect::<Vec<_>>(),
        vec![version_ref('S', 1), version_ref('R', 1)]
    );
    assert_eq!(definitions.calls.load(Ordering::SeqCst), 2);
    assert_eq!(snapshots.calls.load(Ordering::SeqCst), 1);
    assert_eq!(definitions.mutations.load(Ordering::SeqCst), 0);
    assert_eq!(snapshots.mutations.load(Ordering::SeqCst), 0);
}

#[test]
fn run_resolution_rejects_out_of_range_missing_duplicate_and_wrong_snapshot_hash() {
    let snapshot = data_snapshot(3, hash(20));
    let snapshots = Snapshots::new(snapshot.clone());

    let out_of_range = Definitions::new([DefinitionValue::MarketRulePack(rule('R', 1, 3))]);
    let error = resolve_run(
        &out_of_range,
        &snapshots,
        run(&snapshot, vec![version_ref('R', 1)]),
    )
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::ValidationFailed);

    let missing = Definitions::new([]);
    let error = resolve_run(
        &missing,
        &snapshots,
        run(&snapshot, vec![version_ref('R', 1)]),
    )
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::LineageIncomplete);

    let valid = Definitions::new([DefinitionValue::MarketRulePack(rule('R', 1, 5))]);
    let error = resolve_run(
        &valid,
        &snapshots,
        run(&snapshot, vec![version_ref('R', 1), version_ref('R', 1)]),
    )
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::LineageIncomplete);
    assert_eq!(valid.calls.load(Ordering::SeqCst), 0);

    let wrong_ref = ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: id('X'),
        owner: owner(),
        data_snapshot: LineageRef::content_addressed(snapshot.id().clone(), hash(99)),
        universe_snapshot: LineageRef::content_addressed(id('U'), hash(21)),
        rule_packs: vec![version_ref('R', 1)],
        runtime_image_digest: hash(22),
        parameters_hash: hash(23),
        seed: 7,
    })
    .unwrap();
    let error = resolve_run(&valid, &snapshots, wrong_ref).unwrap_err();
    assert_error(&error, ApplicationErrorCategory::LineageIncomplete);
    assert_eq!(valid.calls.load(Ordering::SeqCst), 0);
}

fn resolve_valuation(
    definitions: &Definitions,
    subject_hour: u32,
) -> Result<FullyValidatedMarketFact, ApplicationError> {
    let fact = MarketFact::Valuation(
        Valuation::new(ValuationInput {
            valuation_id: id('V'),
            instrument: version_ref('I', 1),
            owner: owner(),
            source: FactSource::new("test", "valuation", 1).unwrap(),
            valuation_at: time(subject_hour),
            method: "mark".to_owned(),
            rule_pack: version_ref('R', 1),
            values: vec![DecimalValue::new("10125", 2, UnitRef::new(id('P'), version(1))).unwrap()],
            supersedes_id: None,
        })
        .unwrap(),
    );
    block_on(async {
        let unit = MarketFactUnitResolver::new(definitions)
            .resolve(&scope(), fact)
            .await?;
        MarketFactRulePackResolver::new(definitions)
            .resolve(&scope(), unit)
            .await
    })
}

fn resolve_run(
    definitions: &Definitions,
    snapshots: &Snapshots,
    run: ExperimentRun,
) -> Result<ValidatedExperimentRun, ApplicationError> {
    block_on(MarketRunRulePackResolver::new(definitions, snapshots).resolve(&scope(), run))
}

fn run(snapshot: &DataSnapshot, rule_packs: Vec<VersionRef>) -> ExperimentRun {
    ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: id('X'),
        owner: owner(),
        data_snapshot: LineageRef::content_addressed(
            snapshot.id().clone(),
            snapshot.content_hash().clone(),
        ),
        universe_snapshot: LineageRef::content_addressed(id('U'), hash(21)),
        rule_packs,
        runtime_image_digest: hash(22),
        parameters_hash: hash(23),
        seed: 7,
    })
    .unwrap()
}

fn data_snapshot(as_of_hour: u32, blob_hash: ContentHash) -> DataSnapshot {
    DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: id('D'),
        owner: owner(),
        visible_at: time(as_of_hour + 1),
        as_of: time(as_of_hour),
        schema_hash: hash(10),
        manifest_hash: hash(11),
        blob_content_hash: blob_hash,
        lineage: vec![LineageRef::versioned(id('I'), version(1))],
    })
    .unwrap()
}

fn rule(suffix: char, from: u32, to: u32) -> MarketRulePack {
    MarketRulePack::new_with_times(MarketRulePackTimesInput {
        rule_pack_id: id(suffix),
        version: version(1),
        owner: owner(),
        market: "XSHG".to_owned(),
        rule_type: "valuation".to_owned(),
        source: "test".to_owned(),
        from: time(from),
        to: time(to),
        verification_status: VerificationStatus::Verified,
        content_hash: ContentHash::digest(format!("rule-{suffix}").as_bytes()),
    })
    .unwrap()
}

fn price_unit() -> Unit {
    Unit::new(UnitInput {
        unit_id: id('P'),
        version: version(1),
        owner: owner(),
        code: "PRICE".to_owned(),
        dimension: "price".to_owned(),
        scale: 4,
        precision: 18,
    })
    .unwrap()
}

struct Definitions {
    values: BTreeMap<(String, u64), DefinitionValue>,
    calls: AtomicUsize,
    mutations: AtomicUsize,
}

impl Definitions {
    fn new(values: impl IntoIterator<Item = DefinitionValue>) -> Self {
        let values = values
            .into_iter()
            .map(|value| {
                let key = match &value {
                    DefinitionValue::Unit(unit) => (unit.identity().to_owned(), unit.version()),
                    DefinitionValue::MarketRulePack(rule) => {
                        (rule.identity().to_owned(), rule.version())
                    }
                    _ => unreachable!(),
                };
                (key, value)
            })
            .collect();
        Self {
            values,
            calls: AtomicUsize::new(0),
            mutations: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl DefinitionRepository for Definitions {
    async fn create_identity(&self, _identity: DefinitionIdentity) -> Result<(), ApplicationError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Err(ApplicationError::new(
            ApplicationErrorCategory::StateConflict,
            false,
        ))
    }

    async fn append_version(
        &self,
        _command: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Err(ApplicationError::new(
            ApplicationErrorCategory::StateConflict,
            false,
        ))
    }

    async fn get_version(
        &self,
        _scope: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .values
            .get(&(definition_id.as_str().to_owned(), version.get()))
            .cloned())
    }

    async fn resolve_as_of(
        &self,
        _scope: &AccessScope,
        _definition_id: Ulid,
        _instant: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        unreachable!()
    }
}

struct Snapshots {
    snapshot: DataSnapshot,
    calls: AtomicUsize,
    mutations: AtomicUsize,
}

impl Snapshots {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            calls: AtomicUsize::new(0),
            mutations: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl SnapshotRepository for Snapshots {
    async fn publish_verified_manifest(
        &self,
        _command: PublishSnapshot,
    ) -> Result<SnapshotValue, ApplicationError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Err(ApplicationError::new(
            ApplicationErrorCategory::StateConflict,
            false,
        ))
    }

    async fn get_by_id(
        &self,
        _scope: &AccessScope,
        snapshot_id: Ulid,
    ) -> Result<Option<SnapshotValue>, ApplicationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok((snapshot_id == *self.snapshot.id()).then(|| self.snapshot.clone().into()))
    }
}

fn assert_error(error: &ApplicationError, category: ApplicationErrorCategory) {
    assert_eq!(error.category(), category);
    assert!(!error.retryable());
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('A'), vec![id('O')]).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('O'))
}

fn version_ref(suffix: char, version_value: u64) -> VersionRef {
    VersionRef::new(id(suffix), version(version_value))
}

fn version(value: u64) -> Version {
    Version::new(value).unwrap()
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

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).unwrap()
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
