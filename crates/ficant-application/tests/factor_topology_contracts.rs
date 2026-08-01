use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, ApplicationResult, FactorTopologyRepository, IdempotencyKey,
};
use ficant_application::{ApplicationErrorCategory, FactorTopologyUseCase};
use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{
    ContentHash, DecimalValue, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    CurveNodeDefinition, CurveNodeDefinitionInput, CurveNodeRef, CurveRebuildPolicy,
    FactorDefinition, FactorDefinitionInput, FactorTarget, FactorTargetBinding,
    InstrumentFactorTarget, SecondOrderPolicy, SensitivityConvention, SensitivityDirection,
};

struct Repository {
    factor: FactorDefinition,
    curve: CurveNodeDefinition,
    instrument_exists: bool,
}

#[async_trait]
impl FactorTopologyRepository for Repository {
    async fn register_factor_definition(
        &self,
        _: &AccessScope,
        value: FactorDefinition,
        _: IdempotencyKey,
    ) -> ApplicationResult<FactorDefinition> {
        Ok(value)
    }
    async fn register_curve_node_definition(
        &self,
        _: &AccessScope,
        value: CurveNodeDefinition,
        _: IdempotencyKey,
    ) -> ApplicationResult<CurveNodeDefinition> {
        Ok(value)
    }
    async fn bind_factor_target(
        &self,
        _: &AccessScope,
        value: FactorTargetBinding,
        _: IdempotencyKey,
    ) -> ApplicationResult<FactorTargetBinding> {
        Ok(value)
    }
    async fn get_factor_definition(
        &self,
        factor_id: &str,
    ) -> ApplicationResult<Option<FactorDefinition>> {
        Ok((factor_id == self.factor.factor_id()).then(|| self.factor.clone()))
    }
    async fn get_factor_targets(
        &self,
        _: &AccessScope,
        factor_id: &str,
    ) -> ApplicationResult<Vec<FactorTargetBinding>> {
        Ok((factor_id == self.factor.factor_id())
            .then(|| {
                FactorTargetBinding::new(
                    factor_id,
                    FactorTarget::CurveNode(
                        CurveNodeRef::new(
                            self.curve.curve_node_id(),
                            self.curve.content_hash().clone(),
                        )
                        .unwrap(),
                    ),
                )
                .unwrap()
            })
            .into_iter()
            .collect())
    }
    async fn get_target_factors(
        &self,
        _: &AccessScope,
        target: &FactorTarget,
    ) -> ApplicationResult<Vec<FactorDefinition>> {
        Ok(matches!(target, FactorTarget::CurveNode(_))
            .then(|| self.factor.clone())
            .into_iter()
            .collect())
    }
    async fn exact_target_exists(&self, _: &FactorTarget) -> ApplicationResult<bool> {
        Ok(self.instrument_exists)
    }
}

#[tokio::test]
async fn application_fails_closed_before_persisting_unknown_or_unauthorized_targets() {
    let owner = OwnerRef::new(id('T'), id('P'));
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        id('A'),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    let missing = Repository {
        factor: factor(),
        curve: curve(),
        instrument_exists: false,
    };
    let target = FactorTarget::Instrument(InstrumentFactorTarget::new(
        owner.clone(),
        VersionRef::new(id('B'), Version::new(1).unwrap()),
    ));
    let binding = FactorTargetBinding::new("cn.gov.yield.10y", target).unwrap();
    let error = FactorTopologyUseCase::new(&missing)
        .bind_factor_target(&scope, binding, IdempotencyKey::new("missing").unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::NotFound);

    let other = OwnerRef::new(id('T'), id('Q'));
    let unauthorized = FactorTargetBinding::new(
        "cn.gov.yield.10y",
        FactorTarget::Instrument(InstrumentFactorTarget::new(
            other,
            VersionRef::new(id('C'), Version::new(1).unwrap()),
        )),
    )
    .unwrap();
    let error = FactorTopologyUseCase::new(&missing)
        .bind_factor_target(
            &scope,
            unauthorized,
            IdempotencyKey::new("forbidden").unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::Forbidden);
}

#[tokio::test]
async fn application_exposes_only_definition_and_static_topology_values() {
    let owner = OwnerRef::new(id('T'), id('P'));
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        id('A'),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    let repository = Repository {
        factor: factor(),
        curve: curve(),
        instrument_exists: true,
    };
    let use_case = FactorTopologyUseCase::new(&repository);
    assert_eq!(
        use_case
            .get_factor_definition("cn.gov.yield.10y")
            .await
            .unwrap(),
        repository.factor
    );
    assert_eq!(
        use_case
            .get_factor_targets(&scope, "cn.gov.yield.10y")
            .await
            .unwrap()
            .len(),
        1
    );
    let target = FactorTarget::CurveNode(
        CurveNodeRef::new(
            repository.curve.curve_node_id(),
            repository.curve.content_hash().clone(),
        )
        .unwrap(),
    );
    assert_eq!(
        use_case.get_target_factors(&scope, &target).await.unwrap(),
        vec![repository.factor.clone()]
    );
}

fn factor() -> FactorDefinition {
    let unit = UnitRef::new(id('V'), Version::new(1).unwrap());
    let convention = SensitivityConvention::new(
        DecimalValue::new("1", 4, unit.clone()).unwrap(),
        SensitivityDirection::Central,
        CurveRebuildPolicy::Rebuild,
        SecondOrderPolicy::Exclude,
    )
    .unwrap();
    let mut input = FactorDefinitionInput {
        factor_id: "cn.gov.yield.10y".to_owned(),
        factor_unit: unit,
        convention,
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = FactorDefinition::content_hash_for(&input);
    FactorDefinition::new(input).unwrap()
}

fn curve() -> CurveNodeDefinition {
    let mut input = CurveNodeDefinitionInput {
        curve_node_id: "cn.gov.curve.cny.10y".to_owned(),
        curve_family_id: "cn.gov.curve.cny".to_owned(),
        tenor: "P10Y".to_owned(),
        factor_unit: UnitRef::new(id('V'), Version::new(1).unwrap()),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = CurveNodeDefinition::content_hash_for(&input);
    CurveNodeDefinition::new(input).unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F0{suffix}")).unwrap()
}
