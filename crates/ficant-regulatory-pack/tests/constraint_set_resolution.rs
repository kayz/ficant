//! ADR-0026: a Subject's constraint set resolves from its exact regulatory `RulePack`, parses
//! fail-closed, and binds to the ceilings of the Subject's state snapshot.

use std::collections::BTreeMap;
use std::future::Future;
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, DefinitionIdentity, DefinitionRepository,
    DefinitionValue, RegulatoryRulePackParser,
};
use ficant_application::use_cases::constraint_set::ResolveConstraintSet;
use ficant_application::{ApplicationError, ApplicationErrorCategory, ApplicationErrorDetail};
use ficant_contracts::ficant::market::v1::{
    ConstraintDefinition as ProtoDefinition, ConstraintMeasure as ProtoMeasure, RegulatoryRulePack,
};
use ficant_domain::constraint::{ConstraintMeasure, ConstraintSet};
use ficant_domain::market::{
    MarketRulePack, MarketRulePackInput, RulePackContent, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, MarketTime, OwnerRef, Ulid, UnitRef, Version,
    VersionRef,
};
use ficant_domain::subject::{
    AccessSet, ConstraintSetRef, FundingTier, LimitCeiling, SubjectStateSnapshot, SubjectVersion,
    TaxTreatment,
};
use ficant_regulatory_pack::{MARKET, RULE_TYPE, RegulatoryRulePackV1Parser, TYPE_URL};
use prost::Message;

fn definition(code: &str, measure: ProtoMeasure) -> ProtoDefinition {
    ProtoDefinition {
        limit_code: code.to_owned(),
        measure: measure as i32,
    }
}

fn three_constraints() -> Vec<ProtoDefinition> {
    vec![
        definition("repo.borrowed_cash", ProtoMeasure::BorrowedCash),
        definition("bond.single_issue_face", ProtoMeasure::InstrumentFace),
        definition("bond.total_face", ProtoMeasure::PortfolioFace),
    ]
}

fn payload(constraints: Vec<ProtoDefinition>) -> Vec<u8> {
    RegulatoryRulePack { constraints }.encode_to_vec()
}

fn pack(version_value: u64, rule_type: &str, bytes: &[u8]) -> MarketRulePack {
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('R'),
            version: version(version_value),
            owner: owner(),
            market: MARKET.to_owned(),
            rule_type: rule_type.to_owned(),
            source: "synthetic-adr-0026-fixture".to_owned(),
            effective: EffectivePeriod::new(time(1), time(15)).unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(bytes),
        },
        RulePackContent::new(TYPE_URL, bytes.to_vec()).unwrap(),
    )
    .unwrap()
}

fn subject(constraint_set: Option<u64>) -> SubjectVersion {
    SubjectVersion::new(
        VersionRef::new(id('S'), version(1)),
        AccessSet::new(["CN"], ["bond-replay"]).unwrap(),
        FundingTier::DrAvailable,
        TaxTreatment::new("synthetic-vat", "synthetic-income").unwrap(),
        "synthetic-assessment",
        "synthetic-liability",
        constraint_set.map(|value| ConstraintSetRef::new(VersionRef::new(id('R'), version(value)))),
    )
    .unwrap()
}

fn resolve(
    definitions: &Definitions,
    subject: &SubjectVersion,
    at: MarketTime,
) -> Result<ConstraintSet, ApplicationError> {
    block_on(
        ResolveConstraintSet::new(definitions, &RegulatoryRulePackV1Parser).execute(
            &scope(),
            subject,
            at,
        ),
    )
}

#[test]
fn the_subjects_exact_pack_resolves_into_ordered_definitions() {
    let definitions = Definitions::new([pack(1, RULE_TYPE, &payload(three_constraints()))]);
    let set = resolve(&definitions, &subject(Some(1)), time(12)).unwrap();
    assert_eq!(set.rule_pack(), &VersionRef::new(id('R'), version(1)));
    let parsed: Vec<(&str, ConstraintMeasure)> = set
        .definitions()
        .iter()
        .map(|item| (item.limit_code(), item.measure()))
        .collect();
    assert_eq!(
        parsed,
        vec![
            ("bond.single_issue_face", ConstraintMeasure::InstrumentFace),
            ("bond.total_face", ConstraintMeasure::PortfolioFace),
            ("repo.borrowed_cash", ConstraintMeasure::BorrowedCash),
        ]
    );
}

#[test]
fn a_subject_without_a_constraint_set_fails_closed_naming_it() {
    let definitions = Definitions::new([pack(1, RULE_TYPE, &payload(three_constraints()))]);
    let error = resolve(&definitions, &subject(None), time(12)).unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert_eq!(
        error.detail(),
        Some(&ApplicationErrorDetail::RulePackItemMissing {
            path: "subject.constraint_set_ref".to_owned(),
        })
    );
}

#[test]
fn binding_drift_and_envelope_mismatch_fail_closed() {
    let bytes = payload(three_constraints());
    let definitions = Definitions::new([pack(1, RULE_TYPE, &bytes), pack(2, "funding", &bytes)]);
    // Bound version does not exist.
    assert_eq!(
        resolve(&definitions, &subject(Some(3)), time(12))
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::LineageIncomplete
    );
    // Not effective at the valuation time (the pack is effective 01:00-15:00, end exclusive).
    assert!(resolve(&definitions, &subject(Some(1)), time(15)).is_err());
    // Another rule type behind the bound reference.
    assert!(resolve(&definitions, &subject(Some(2)), time(12)).is_err());
}

#[test]
fn invalid_payloads_fail_closed_with_the_offending_item() {
    let parser = RegulatoryRulePackV1Parser;
    let parse = |bytes: Vec<u8>| {
        parser.parse(
            &RulePackContent::new(TYPE_URL, bytes).unwrap(),
            VersionRef::new(id('R'), version(1)),
        )
    };
    let item = |result: Result<ConstraintSet, ApplicationError>| match result.unwrap_err().detail()
    {
        Some(ApplicationErrorDetail::RulePackItemMissing { path }) => path.clone(),
        other => panic!("unexpected detail {other:?}"),
    };
    // A pack without constraints encodes to zero bytes, which is not valid RulePack content.
    assert!(RulePackContent::new(TYPE_URL, payload(vec![])).is_err());
    assert_eq!(
        item(parse(payload(vec![
            definition("bond.total_face", ProtoMeasure::PortfolioFace),
            definition("repo.borrowed_cash", ProtoMeasure::Unspecified),
        ]))),
        "context.regulatory_rule_pack.content.constraints[1].measure"
    );
    assert_eq!(
        item(parse(payload(vec![ProtoDefinition {
            limit_code: "bond.total_face".to_owned(),
            measure: 99,
        }]))),
        "context.regulatory_rule_pack.content.constraints[0].measure"
    );
    assert_eq!(
        item(parse(payload(vec![definition(
            " ",
            ProtoMeasure::PortfolioFace
        )]))),
        "context.regulatory_rule_pack.content.constraints[0].limit_code"
    );
    // Duplicate limit codes, garbage bytes and a foreign type URL are invalid.
    assert!(
        parse(payload(vec![
            definition("bond.total_face", ProtoMeasure::PortfolioFace),
            definition("bond.total_face", ProtoMeasure::InstrumentFace),
        ]))
        .is_err()
    );
    assert!(parse(vec![0xff, 0xff, 0xff]).is_err());
    assert!(
        parser
            .parse(
                &RulePackContent::new(
                    "type.googleapis.com/ficant.market.v1.FundingRulePack",
                    payload(three_constraints()),
                )
                .unwrap(),
                VersionRef::new(id('R'), version(1)),
            )
            .is_err()
    );
}

#[test]
fn the_set_binds_to_the_subjects_ceilings_by_limit_code() {
    let definitions = Definitions::new([pack(1, RULE_TYPE, &payload(three_constraints()))]);
    let subject = subject(Some(1));
    let set = resolve(&definitions, &subject, time(12)).unwrap();
    let ceiling = |code: &str, coefficient: &str| {
        LimitCeiling::new(code, DecimalValue::new(coefficient, 0, unit()).unwrap()).unwrap()
    };
    let full = state(
        subject.reference(),
        vec![
            ceiling("bond.single_issue_face", "50000000"),
            ceiling("bond.total_face", "200000000"),
            ceiling("repo.borrowed_cash", "150000000"),
            ceiling("cffex.hedge_lots", "300"),
        ],
    );
    let bound = set.bind(subject.reference(), &full).unwrap();
    let summary: Vec<(&str, ConstraintMeasure, &str)> = bound
        .iter()
        .map(|item| {
            (
                item.limit_code(),
                item.measure(),
                item.ceiling().coefficient(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "bond.single_issue_face",
                ConstraintMeasure::InstrumentFace,
                "50000000"
            ),
            (
                "bond.total_face",
                ConstraintMeasure::PortfolioFace,
                "200000000"
            ),
            (
                "repo.borrowed_cash",
                ConstraintMeasure::BorrowedCash,
                "150000000"
            ),
        ],
        "ceilings the set does not define are ignored"
    );

    // A defined constraint without a ceiling, a negative ceiling, or another Subject's snapshot.
    let missing = state(
        subject.reference(),
        vec![
            ceiling("bond.single_issue_face", "50000000"),
            ceiling("bond.total_face", "200000000"),
        ],
    );
    assert!(set.bind(subject.reference(), &missing).is_err());
    let negative = state(
        subject.reference(),
        vec![
            ceiling("bond.single_issue_face", "50000000"),
            ceiling("bond.total_face", "-1"),
            ceiling("repo.borrowed_cash", "150000000"),
        ],
    );
    assert!(set.bind(subject.reference(), &negative).is_err());
    let other = VersionRef::new(id('S'), version(2));
    assert!(set.bind(&other, &full).is_err());
}

fn state(subject: &VersionRef, ceilings: Vec<LimitCeiling>) -> SubjectStateSnapshot {
    SubjectStateSnapshot::new(
        id('Q'),
        subject.clone(),
        DecimalValue::new("1000000000", 0, unit()).unwrap(),
        ceilings,
        time(1).instant(),
        time(2).instant(),
        "Asia/Shanghai",
    )
    .unwrap()
}

struct Definitions {
    values: BTreeMap<(String, u64), DefinitionValue>,
}

impl Definitions {
    fn new(values: impl IntoIterator<Item = MarketRulePack>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(DefinitionValue::MarketRulePack)
                .map(|value| ((value.identity().to_owned(), value.version()), value))
                .collect(),
        }
    }
}

#[async_trait]
impl DefinitionRepository for Definitions {
    async fn create_identity(&self, _: DefinitionIdentity) -> Result<(), ApplicationError> {
        unreachable!("resolver performs only exact reads")
    }

    async fn append_version(
        &self,
        _: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        unreachable!("resolver performs only exact reads")
    }

    async fn get_version(
        &self,
        _: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Ok(self
            .values
            .get(&(definition_id.as_str().to_owned(), version.get()))
            .cloned())
    }

    async fn resolve_as_of(
        &self,
        _: &AccessScope,
        _: Ulid,
        _: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        unreachable!("the constraint set is bound to an exact RulePack version")
    }
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('A'), vec![id('B')]).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('B'))
}

fn unit() -> UnitRef {
    UnitRef::new(id('P'), version(1))
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn version(value: u64) -> Version {
    Version::new(value).unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-04".parse().unwrap(),
    )
    .unwrap()
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
