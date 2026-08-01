mod support;

use ficant_application::ports::{FactorTopologyRepository, IdempotencyKey};
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

#[tokio::test]
async fn factor_topology_is_global_immutable_and_queryable_in_both_directions() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    seed_exact_instrument_targets(&pool).await;
    let repository = support::repository(pool);
    let owner = OwnerRef::new(id('T'), id('P'));
    let scope = support::access_scope(&owner);

    let definition = register_definition_and_reject_drifts(&repository, &scope).await;
    let curve = register_and_verify_topology(&repository, &scope, &owner, &definition).await;
    reject_invalid_targets(&repository, &scope, owner, &definition, &curve).await;
}

async fn register_definition_and_reject_drifts(
    repository: &impl FactorTopologyRepository,
    scope: &ficant_application::ports::AccessScope,
) -> FactorDefinition {
    let definition = factor("cn.gov.yield.10y", "1");
    assert_eq!(
        repository
            .register_factor_definition(
                scope,
                definition.clone(),
                IdempotencyKey::new("factor:definition:v1").unwrap(),
            )
            .await
            .unwrap(),
        definition
    );
    assert_eq!(
        repository
            .register_factor_definition(
                scope,
                definition.clone(),
                IdempotencyKey::new("factor:definition:v1").unwrap(),
            )
            .await
            .unwrap(),
        definition
    );
    let convention_drifts = [
        factor_with_convention(
            "cn.gov.yield.10y",
            'W',
            "1",
            SensitivityDirection::Central,
            CurveRebuildPolicy::Rebuild,
            SecondOrderPolicy::Include,
        ),
        factor_with_convention(
            "cn.gov.yield.10y",
            'V',
            "2",
            SensitivityDirection::Central,
            CurveRebuildPolicy::Rebuild,
            SecondOrderPolicy::Include,
        ),
        factor_with_convention(
            "cn.gov.yield.10y",
            'V',
            "1",
            SensitivityDirection::Up,
            CurveRebuildPolicy::Rebuild,
            SecondOrderPolicy::Include,
        ),
        factor_with_convention(
            "cn.gov.yield.10y",
            'V',
            "1",
            SensitivityDirection::Central,
            CurveRebuildPolicy::Hold,
            SecondOrderPolicy::Include,
        ),
        factor_with_convention(
            "cn.gov.yield.10y",
            'V',
            "1",
            SensitivityDirection::Central,
            CurveRebuildPolicy::Rebuild,
            SecondOrderPolicy::Exclude,
        ),
    ];
    for (index, drift) in convention_drifts.into_iter().enumerate() {
        assert_eq!(
            repository
                .register_factor_definition(
                    scope,
                    drift,
                    IdempotencyKey::new(format!("factor:definition:conflict:{index}:v1")).unwrap(),
                )
                .await
                .unwrap_err()
                .category(),
            ApplicationErrorCategory::AlreadyExists,
            "every canonical FactorDefinition field is immutable"
        );
    }
    definition
}

async fn register_and_verify_topology(
    repository: &impl FactorTopologyRepository,
    scope: &ficant_application::ports::AccessScope,
    owner: &OwnerRef,
    definition: &FactorDefinition,
) -> CurveNodeDefinition {
    let curve = curve_node("cn.gov.curve.cny.10y");
    assert_eq!(
        repository
            .register_curve_node_definition(
                scope,
                curve.clone(),
                IdempotencyKey::new("factor:curve:v1").unwrap(),
            )
            .await
            .unwrap(),
        curve
    );
    let curve_target = FactorTarget::CurveNode(
        CurveNodeRef::new(curve.curve_node_id(), curve.content_hash().clone()).unwrap(),
    );
    let bond_target = FactorTarget::Instrument(InstrumentFactorTarget::new(
        owner.clone(),
        VersionRef::new(id('B'), Version::new(1).unwrap()),
    ));
    let futures_target = FactorTarget::Instrument(InstrumentFactorTarget::new(
        owner.clone(),
        VersionRef::new(id('F'), Version::new(1).unwrap()),
    ));
    let use_case = FactorTopologyUseCase::new(repository);
    let targets = [
        curve_target.clone(),
        bond_target.clone(),
        futures_target.clone(),
    ];
    let mut bindings = Vec::new();
    for (index, target) in targets.iter().cloned().enumerate() {
        let binding = FactorTargetBinding::new(definition.factor_id(), target).unwrap();
        bindings.push(
            use_case
                .bind_factor_target(
                    scope,
                    binding,
                    IdempotencyKey::new(format!("factor:binding:{index}:v1")).unwrap(),
                )
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        use_case
            .bind_factor_target(
                scope,
                bindings[0].clone(),
                IdempotencyKey::new("factor:binding:0:v1").unwrap(),
            )
            .await
            .unwrap(),
        bindings[0],
        "an identical binding command is an idempotent replay"
    );
    assert_eq!(
        use_case
            .get_factor_targets(scope, definition.factor_id())
            .await
            .unwrap(),
        bindings
    );
    for target in &targets {
        assert_eq!(
            use_case.get_target_factors(scope, target).await.unwrap(),
            vec![definition.clone()]
        );
    }
    curve
}

async fn reject_invalid_targets(
    repository: &impl FactorTopologyRepository,
    scope: &ficant_application::ports::AccessScope,
    owner: OwnerRef,
    definition: &FactorDefinition,
    curve: &CurveNodeDefinition,
) {
    let use_case = FactorTopologyUseCase::new(repository);
    let drifted_curve = FactorTarget::CurveNode(
        CurveNodeRef::new(curve.curve_node_id(), ContentHash::digest(b"drift")).unwrap(),
    );
    let error = use_case
        .bind_factor_target(
            scope,
            FactorTargetBinding::new(definition.factor_id(), drifted_curve).unwrap(),
            IdempotencyKey::new("factor:binding:drift:v1").unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::NotFound);

    let missing_curve = FactorTarget::CurveNode(
        CurveNodeRef::new(
            "cn.gov.curve.cny.30y",
            ContentHash::digest(b"unregistered-curve-node"),
        )
        .unwrap(),
    );
    let error = use_case
        .bind_factor_target(
            scope,
            FactorTargetBinding::new(definition.factor_id(), missing_curve).unwrap(),
            IdempotencyKey::new("factor:binding:missing-curve:v1").unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::NotFound);

    let untyped_target = FactorTarget::Instrument(InstrumentFactorTarget::new(
        OwnerRef::new(id('T'), id('P')),
        VersionRef::new(id('D'), Version::new(1).unwrap()),
    ));
    let error = use_case
        .bind_factor_target(
            scope,
            FactorTargetBinding::new(definition.factor_id(), untyped_target).unwrap(),
            IdempotencyKey::new("factor:binding:untyped:v1").unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::NotFound);

    let unauthorized_scope =
        ficant_application::ports::AccessScope::new(id('T'), id('A'), vec![id('Q')]).unwrap();
    let error = use_case
        .get_factor_targets(&unauthorized_scope, definition.factor_id())
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::Forbidden);

    let missing_version = FactorTarget::Instrument(InstrumentFactorTarget::new(
        owner,
        VersionRef::new(id('B'), Version::new(2).unwrap()),
    ));
    let error = use_case
        .bind_factor_target(
            scope,
            FactorTargetBinding::new(definition.factor_id(), missing_version).unwrap(),
            IdempotencyKey::new("factor:binding:missing-version:v1").unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::NotFound);
    assert_eq!(
        use_case
            .get_factor_targets(scope, definition.factor_id())
            .await
            .unwrap()
            .len(),
        3,
        "failed bindings must not create partial topology"
    );
}

async fn seed_exact_instrument_targets(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "INSERT INTO market.units
         (tenant_id, unit_id, version, owner_id, code, dimension, scale, precision, payload)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0V', 1,
                 '01ARZ3NDEKTSV4RRFFQ69G5F0P', 'CNY', 'currency', 2, 18, '\\x01');
         INSERT INTO market.calendars
         (tenant_id, calendar_id, version, owner_id, market, market_timezone,
          effective_from, effective_to, payload)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0C', 1,
                 '01ARZ3NDEKTSV4RRFFQ69G5F0P', 'CFFEX', 'Asia/Shanghai',
                 '2020-01-01T00:00:00Z', '2030-01-01T00:00:00Z', '\\x01');
         INSERT INTO market.market_rule_packs
         (tenant_id, rule_pack_id, version, owner_id, market, rule_type, source,
          effective_from, effective_to, verification_status, content_hash, payload)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0R', 1,
                 '01ARZ3NDEKTSV4RRFFQ69G5F0P', 'CFFEX', 'delivery', 'fixture',
                 '2020-01-01T00:00:00Z', '2030-01-01T00:00:00Z', 'VERIFIED',
                 repeat('a', 64), '\\x01');
         INSERT INTO market.instruments
         (tenant_id, instrument_id, version, owner_id, kind, market, symbol,
          currency_unit_id, currency_unit_version, calendar_id, calendar_version, payload)
         VALUES
         ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0B', 1,
          '01ARZ3NDEKTSV4RRFFQ69G5F0P', 'BOND', 'CIBM', 'BOND-10Y',
          '01ARZ3NDEKTSV4RRFFQ69G5F0V', 1, '01ARZ3NDEKTSV4RRFFQ69G5F0C', 1, '\\x01'),
         ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0F', 1,
          '01ARZ3NDEKTSV4RRFFQ69G5F0P', 'FUTURES', 'CFFEX', 'T2603',
          '01ARZ3NDEKTSV4RRFFQ69G5F0V', 1, '01ARZ3NDEKTSV4RRFFQ69G5F0C', 1, '\\x01'),
         ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0D', 1,
          '01ARZ3NDEKTSV4RRFFQ69G5F0P', 'OTHER', 'CIBM', 'UNTYPED',
          '01ARZ3NDEKTSV4RRFFQ69G5F0V', 1, '01ARZ3NDEKTSV4RRFFQ69G5F0C', 1, '\\x01');
         INSERT INTO market.bonds
         (tenant_id, instrument_id, version, issue_date, maturity_date, face_coefficient,
          face_scale, face_unit_id, face_unit_version, payload)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0B', 1,
                 '2020-01-01', '2030-01-01', 100, 0, '01ARZ3NDEKTSV4RRFFQ69G5F0V', 1, '\\x01');
         INSERT INTO market.futures_contracts
         (tenant_id, instrument_id, version, last_trade_time, expiry_time, settlement_time,
          multiplier_coefficient, multiplier_scale, multiplier_unit_id, multiplier_unit_version,
          rule_pack_id, rule_pack_version, payload)
         VALUES ('01ARZ3NDEKTSV4RRFFQ69G5F0T', '01ARZ3NDEKTSV4RRFFQ69G5F0F', 1,
                 '2026-03-10T00:00:00Z', '2026-03-11T00:00:00Z', '2026-03-12T00:00:00Z',
                 10000, 0, '01ARZ3NDEKTSV4RRFFQ69G5F0V', 1,
                 '01ARZ3NDEKTSV4RRFFQ69G5F0R', 1, '\\x01');",
    )
    .execute(pool)
    .await
    .unwrap();
}

fn factor(factor_id: &str, coefficient: &str) -> FactorDefinition {
    factor_with_convention(
        factor_id,
        'V',
        coefficient,
        SensitivityDirection::Central,
        CurveRebuildPolicy::Rebuild,
        SecondOrderPolicy::Include,
    )
}

fn factor_with_convention(
    factor_id: &str,
    factor_unit_suffix: char,
    coefficient: &str,
    direction: SensitivityDirection,
    curve_rebuild: CurveRebuildPolicy,
    second_order: SecondOrderPolicy,
) -> FactorDefinition {
    let unit = UnitRef::new(id(factor_unit_suffix), Version::new(1).unwrap());
    let bump_unit = UnitRef::new(id('V'), Version::new(1).unwrap());
    let convention = SensitivityConvention::new(
        DecimalValue::new(coefficient, 0, bump_unit).unwrap(),
        direction,
        curve_rebuild,
        second_order,
    )
    .unwrap();
    let mut input = FactorDefinitionInput {
        factor_id: factor_id.to_owned(),
        factor_unit: unit,
        convention,
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = FactorDefinition::content_hash_for(&input);
    FactorDefinition::new(input).unwrap()
}

fn curve_node(curve_node_id: &str) -> CurveNodeDefinition {
    let mut input = CurveNodeDefinitionInput {
        curve_node_id: curve_node_id.to_owned(),
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
