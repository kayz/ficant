//! ADR-0026: a Subject's constraint set resolves from its exact regulatory `RulePack`, parses
//! fail-closed, and binds to the ceilings of the Subject's state snapshot.

use ficant_application::ports::RegulatoryRulePackParser;
use ficant_application::use_cases::parse_verified_regulatory_rule;
use ficant_application::{ApplicationError, ApplicationErrorDetail};
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
        AccessSet::new(["CN"], ["bond-pricing"]).unwrap(),
        FundingTier::DrAvailable,
        TaxTreatment::new("synthetic-vat", "synthetic-income").unwrap(),
        "synthetic-assessment",
        "synthetic-liability",
        constraint_set.map(|value| ConstraintSetRef::new(VersionRef::new(id('R'), version(value)))),
    )
    .unwrap()
}

#[test]
fn the_subjects_exact_pack_resolves_into_ordered_definitions() {
    let rule_pack = pack(1, RULE_TYPE, &payload(three_constraints()));
    let set =
        parse_verified_regulatory_rule(&rule_pack, Some(&time(12)), &RegulatoryRulePackV1Parser)
            .unwrap();
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
fn effective_window_and_envelope_mismatch_fail_closed() {
    let bytes = payload(three_constraints());
    let valid_pack = pack(1, RULE_TYPE, &bytes);
    let wrong_type = pack(2, "funding", &bytes);
    assert!(
        parse_verified_regulatory_rule(&valid_pack, Some(&time(15)), &RegulatoryRulePackV1Parser)
            .is_err()
    );
    assert!(
        parse_verified_regulatory_rule(&wrong_type, Some(&time(12)), &RegulatoryRulePackV1Parser)
            .is_err()
    );
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
    let rule_pack = pack(1, RULE_TYPE, &payload(three_constraints()));
    let subject = subject(Some(1));
    let set =
        parse_verified_regulatory_rule(&rule_pack, Some(&time(12)), &RegulatoryRulePackV1Parser)
            .unwrap();
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
