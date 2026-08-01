use ficant_domain::primitives::{
    ContentHash, DecimalValue, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    CurveNodeDefinition, CurveNodeDefinitionInput, CurveNodeRef, CurveRebuildPolicy,
    FactorDefinition, FactorDefinitionInput, FactorTarget, FactorTargetBinding,
    InstrumentFactorTarget, SecondOrderPolicy, SensitivityConvention, SensitivityDirection,
};
use ficant_domain::{ContentAddressed, DomainErrorCode};

#[test]
fn immutable_factor_rejects_noncanonical_identity_and_convention_drift() {
    let mut input = factor_input();
    input.content_hash = FactorDefinition::content_hash_for(&input);
    let factor = FactorDefinition::new(input.clone()).unwrap();
    assert_eq!(factor.factor_id(), "cn.gov.yield.10y");
    assert_eq!(factor.content_hash(), &input.content_hash);

    let mut drifted = input.clone();
    drifted.convention = SensitivityConvention::new(
        decimal("2", 'R'),
        SensitivityDirection::Central,
        CurveRebuildPolicy::Rebuild,
        SecondOrderPolicy::Exclude,
    )
    .unwrap();
    drifted.content_hash = FactorDefinition::content_hash_for(&drifted);
    assert_ne!(factor.content_hash(), &drifted.content_hash);

    let mut noncanonical = factor_input();
    noncanonical.factor_id = "CN.gov.yield.10y".to_owned();
    noncanonical.content_hash = FactorDefinition::content_hash_for(&noncanonical);
    assert_eq!(
        FactorDefinition::new(noncanonical).unwrap_err(),
        DomainErrorCode::InvalidId
    );

    let mut five_segments = factor_input();
    five_segments.factor_id = "cn.gov.curve.yield.10y".to_owned();
    five_segments.content_hash = FactorDefinition::content_hash_for(&five_segments);
    assert_eq!(
        FactorDefinition::new(five_segments).unwrap_err(),
        DomainErrorCode::InvalidId,
        "FactorId must be exactly market.category.economic-quantity.tenor"
    );
}

#[test]
fn stable_curve_nodes_and_exact_targets_are_content_addressed() {
    let mut node_input = CurveNodeDefinitionInput {
        curve_node_id: "cn.gov.yield-curve.10y".to_owned(),
        curve_family_id: "cn.gov.yield-curve".to_owned(),
        tenor: "P10Y".to_owned(),
        factor_unit: unit('R'),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    node_input.content_hash = CurveNodeDefinition::content_hash_for(&node_input);
    let node = CurveNodeDefinition::new(node_input.clone()).unwrap();
    let node_ref = CurveNodeRef::new(node.curve_node_id(), node.content_hash().clone()).unwrap();

    let binding =
        FactorTargetBinding::new("cn.gov.yield.10y", FactorTarget::CurveNode(node_ref)).unwrap();
    assert_eq!(binding.factor_id(), "cn.gov.yield.10y");
    assert!(matches!(binding.target(), FactorTarget::CurveNode(_)));

    let instrument = InstrumentFactorTarget::new(
        OwnerRef::new(id('T'), id('N')),
        VersionRef::new(id('J'), Version::new(1).unwrap()),
    );
    assert_eq!(instrument.owner().owner_id(), &id('N'));
}

fn factor_input() -> FactorDefinitionInput {
    FactorDefinitionInput {
        factor_id: "cn.gov.yield.10y".to_owned(),
        factor_unit: unit('R'),
        convention: SensitivityConvention::new(
            decimal("1", 'R'),
            SensitivityDirection::Central,
            CurveRebuildPolicy::Rebuild,
            SecondOrderPolicy::Exclude,
        )
        .unwrap(),
        content_hash: ContentHash::digest(b"placeholder"),
    }
}

fn decimal(value: &str, suffix: char) -> DecimalValue {
    DecimalValue::new(value, 0, unit(suffix)).unwrap()
}

fn unit(suffix: char) -> UnitRef {
    UnitRef::new(id(suffix), Version::new(1).unwrap())
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
