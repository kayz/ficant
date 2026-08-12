use chrono::{DateTime, NaiveDate, Utc};
use ficant_api::RatesGrpcService;
use ficant_contracts::ficant::core::v1::{
    DecimalValue, MarketTime as ProtoMarketTime, OwnerRef as ProtoOwnerRef, Sha256,
    Ulid as ProtoUlid, UnitRef, VersionRef,
};
use ficant_contracts::ficant::market::v1::{
    CouponTaxClaimScope, GrossCouponTaxBasis, SubjectCouponTaxTreatment, TaxRoundingMode,
};
use ficant_contracts::ficant::rates::v1::{
    AlgorithmBinding, AnalysisContext, AnalysisInputBinding, AnalysisInputRole, AnalysisUnits,
    AnalyzeBondRequest, AnalyzeBondResult, CalendarRequirement, ObjectBinding, ResultMetadata,
    RiskSummary, SnapshotBinding, analysis_input_binding, analyze_bond_request,
};
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{
    ABI_VERSION, ALGORITHM_ID, ALGORITHM_VERSION, AnalyticsMode, AnalyticsObjectRef, BondTerms,
    BusinessDayConvention, CONVENTION_PROFILE, CalendarBinding,
    CalendarRequirement as DomainCalendarRequirement, CouponFrequency, DayCountConvention,
    FixedDecimal,
};
use ficant_domain::market::{BondTaxAttributes, IncomeTaxStatus, ValueAddedTaxStatus};
use ficant_domain::primitives::{
    ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef as DomainVersionRef,
};
use ficant_domain::research::{
    GraphExternalInput, GraphExternalInputBinding, ResearchEdge, ResearchGraph, ResearchGraphInput,
    ResearchNode, TypedValue,
};
use ficant_native_nodes::{
    CgbBondAnalyticsNativeNode, CgbBondRiskSummaryNativeNode, MATERIALIZED_INPUT_PORT,
    REQUEST_PORT, RESULT_PORT, RISK_INPUT_PORT, RISK_OUTPUT_PORT, analyze_bond_request_type,
    cgb_bond_analytics_contract, cgb_bond_risk_summary_contract, encode_materialized_bond_input,
    materialized_bond_input_type, trusted_native_node,
};
use ficant_runtime::{
    ExecutionExternalInput, ExecutionInstanceIdentity, NativeNode, NativePortValue,
    NodeImplementation, ReproducibilityIdentity, ReproducibilityIdentityInput, RulePackBinding,
    RuntimeError, execute_native_node,
};
use prost::Message;
use serde_json::Value;

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";
const RATE_UNIT_ID: &str = "01K2CGBVAT0000000000000000";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{PREFIX}{suffix}")).unwrap()
}

fn proto_id(suffix: char) -> ProtoUlid {
    ProtoUlid {
        value: id(suffix).as_str().to_owned(),
    }
}

fn hash(label: &[u8]) -> ContentHash {
    ContentHash::digest(label)
}

fn proto_hash(value: &ContentHash) -> Sha256 {
    Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn object(suffix: char) -> ObjectBinding {
    ObjectBinding {
        object: Some(VersionRef {
            id: Some(proto_id(suffix)),
            version: 1,
        }),
        content_hash: Some(proto_hash(&hash(format!("object-{suffix}").as_bytes()))),
    }
}

fn snapshot(suffix: char) -> SnapshotBinding {
    SnapshotBinding {
        snapshot_id: Some(proto_id(suffix)),
        content_hash: Some(proto_hash(&hash(format!("object-{suffix}").as_bytes()))),
    }
}

fn unit(suffix: char) -> UnitRef {
    UnitRef {
        unit_id: Some(proto_id(suffix)),
        version: 1,
    }
}

fn rate_unit() -> UnitRef {
    UnitRef {
        unit_id: Some(ProtoUlid {
            value: RATE_UNIT_ID.to_owned(),
        }),
        version: 1,
    }
}

fn rate_unit_object() -> ObjectBinding {
    ObjectBinding {
        object: Some(VersionRef {
            id: rate_unit().unit_id,
            version: 1,
        }),
        content_hash: Some(proto_hash(&hash(b"authoritative-rate-unit"))),
    }
}

fn private_tax_treatment() -> SubjectCouponTaxTreatment {
    SubjectCouponTaxTreatment {
        value_added_tax_profile: "cn-vat-general-taxpayer".to_owned(),
        income_tax_profile: "cn-cgb-interest-cit-exempt".to_owned(),
        value_added_tax_rate: Some(decimal("6", 2, rate_unit())),
        income_tax_rate: Some(decimal("0", 0, rate_unit())),
        gross_coupon_basis: GrossCouponTaxBasis::VatIncluded as i32,
        rounding: TaxRoundingMode::TiesToEven as i32,
        claim_scope: CouponTaxClaimScope::CouponOutputVatBeforeInputCredit as i32,
    }
}

fn semantic_hash() -> ContentHash {
    ContentHash::from_bytes(&[
        0x54, 0xfa, 0x5a, 0xdb, 0xeb, 0x8b, 0x16, 0x4d, 0xc7, 0x79, 0xec, 0xc2, 0x50, 0xab, 0x62,
        0x2a, 0xb5, 0x74, 0xcd, 0xeb, 0x36, 0xf2, 0xb6, 0xda, 0x58, 0xf4, 0xd8, 0x77, 0xce, 0x51,
        0x06, 0x0a,
    ])
    .unwrap()
}

fn decimal(coefficient: &str, scale: u32, unit: UnitRef) -> DecimalValue {
    DecimalValue {
        coefficient: coefficient.to_owned(),
        scale,
        unit: Some(unit),
    }
}

fn proto_time(instant: &str, local_trading_date: &str) -> ProtoMarketTime {
    let instant = DateTime::parse_from_rfc3339(instant).unwrap();
    ProtoMarketTime {
        instant: Some(prost_types::Timestamp {
            seconds: instant.timestamp(),
            nanos: instant.timestamp_subsec_nanos().cast_signed(),
        }),
        market_timezone: "Asia/Shanghai".to_owned(),
        local_trading_date: local_trading_date.to_owned(),
    }
}

fn valuation_at() -> ProtoMarketTime {
    proto_time("2026-07-13T15:00:00+08:00", "2026-07-13")
}

fn analysis_units() -> AnalysisUnits {
    AnalysisUnits {
        currency_amount: Some(unit('A')),
        price_per_100: Some(unit('B')),
        rate: Some(rate_unit()),
        years: Some(unit('D')),
        years_squared: Some(unit('E')),
        dv01_per_100: Some(unit('F')),
        dv01: Some(unit('G')),
        dimensionless: Some(unit('H')),
        contract_count: Some(unit('J')),
    }
}

fn algorithm() -> AlgorithmBinding {
    AlgorithmBinding {
        algorithm_id: ALGORITHM_ID.to_owned(),
        algorithm_version: ALGORITHM_VERSION,
        convention_profile: CONVENTION_PROFILE.to_owned(),
        abi_version: ABI_VERSION,
    }
}

fn golden_request() -> AnalyzeBondRequest {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/golden-cases/china-rates/fixtures/bond-260008.IB.json"
    ))
    .unwrap();
    AnalyzeBondRequest {
        context: Some(AnalysisContext {
            owner: Some(ProtoOwnerRef {
                tenant_id: Some(proto_id('0')),
                owner_id: Some(proto_id('1')),
            }),
            algorithm: Some(algorithm()),
            units: Some(analysis_units()),
            subject_ref: Some(VersionRef {
                id: Some(proto_id('S')),
                version: 1,
            }),
            knowledge_at: Some(valuation_at()),
        }),
        bond: Some(object('N')),
        valuation_at: Some(valuation_at()),
        settlement_date: fixture["settlement_date"].as_str().unwrap().to_owned(),
        calendar_requirement: CalendarRequirement::ReferenceReplay as i32,
        calendar: Some(object('C')),
        input: Some(analyze_bond_request::Input::YieldToMaturity(decimal(
            "155",
            4,
            rate_unit(),
        ))),
        data_snapshot: Some(snapshot('M')),
        tax_rule_pack: Some(object('K')),
    }
}

fn domain_object(suffix: char) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        DomainVersionRef::new(id(suffix), Version::new(1).unwrap()),
        hash(format!("object-{suffix}").as_bytes()),
    )
}

fn materialized_input(input_value: FixedDecimal) -> ficant_domain::analytics::BondAnalyticsInput {
    let instant = DateTime::parse_from_rfc3339("2026-07-13T15:00:00+08:00")
        .unwrap()
        .with_timezone(&Utc);
    ficant_domain::analytics::BondAnalyticsInput::new(
        OwnerRef::new(id('0'), id('1')),
        domain_object('N'),
        domain_object('K'),
        domain_object('M'),
        MarketTime::new(
            instant,
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2026, 7, 13).unwrap(),
        )
        .unwrap(),
        NaiveDate::from_ymd_opt(2026, 7, 14).unwrap(),
        DomainCalendarRequirement::ReferenceReplay,
        CalendarBinding::new(
            id('C').to_string(),
            Version::new(1).unwrap(),
            hash(b"object-C"),
            NaiveDate::from_ymd_opt(2005, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2035, 12, 31).unwrap(),
            vec![],
            vec![],
        )
        .unwrap(),
        BondTerms::with_issuance(
            NaiveDate::from_ymd_opt(2026, 4, 15).unwrap(),
            NaiveDate::from_ymd_opt(2026, 4, 15).unwrap(),
            NaiveDate::from_ymd_opt(2031, 4, 15).unwrap(),
            CouponFrequency::Annual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            FixedDecimal::from_scaled(15_000_000_000),
            FixedDecimal::from_scaled(100_000_000_000_000),
            FixedDecimal::from_scaled(100_000_000_000_000),
            BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Exempt),
        )
        .unwrap(),
        AnalyticsMode::YieldIn,
        input_value,
    )
    .unwrap()
}

fn evidence_object(role: AnalysisInputRole, binding: ObjectBinding) -> AnalysisInputBinding {
    AnalysisInputBinding {
        role: role as i32,
        owner: Some(ProtoOwnerRef {
            tenant_id: Some(proto_id('0')),
            owner_id: Some(proto_id('1')),
        }),
        binding: Some(analysis_input_binding::Binding::Object(binding)),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    }
}

fn effective_evidence_object(
    role: AnalysisInputRole,
    binding: ObjectBinding,
) -> AnalysisInputBinding {
    let mut evidence = evidence_object(role, binding);
    evidence.effective_from = Some(proto_time("2005-01-01T00:00:00+08:00", "2005-01-01"));
    evidence.effective_to = Some(proto_time("2035-12-31T23:59:59+08:00", "2035-12-31"));
    evidence
}

fn supplied_metadata(
    request: &AnalyzeBondRequest,
    input: &ficant_domain::analytics::BondAnalyticsInput,
) -> ResultMetadata {
    let mut consumed_inputs = vec![
        evidence_object(AnalysisInputRole::Subject, object('S')),
        evidence_object(AnalysisInputRole::Bond, object('N')),
        effective_evidence_object(AnalysisInputRole::Calendar, object('C')),
        effective_evidence_object(AnalysisInputRole::TaxRulePack, object('K')),
        AnalysisInputBinding {
            role: AnalysisInputRole::DataSnapshot as i32,
            owner: Some(ProtoOwnerRef {
                tenant_id: Some(proto_id('0')),
                owner_id: Some(proto_id('1')),
            }),
            binding: Some(analysis_input_binding::Binding::Snapshot(snapshot('M'))),
            observed_at: Some(valuation_at()),
            visible_at: Some(valuation_at()),
            effective_from: None,
            effective_to: None,
        },
    ];
    for suffix in ['A', 'B', 'D', 'E', 'F', 'G', 'H', 'J'] {
        consumed_inputs.push(evidence_object(AnalysisInputRole::Unit, object(suffix)));
    }
    consumed_inputs.push(evidence_object(AnalysisInputRole::Unit, rate_unit_object()));
    consumed_inputs.sort_by_key(prost::Message::encode_to_vec);
    RatesGrpcService::canonical_materialized_bond_metadata(
        request,
        input,
        &RatesGrpcService::canonical_v2_coupon_tax_treatment(
            input,
            &private_tax_treatment(),
            semantic_hash().as_bytes(),
        )
        .unwrap(),
        &consumed_inputs,
    )
    .unwrap()
}

struct Fixture {
    graph: ResearchGraph,
    node: ResearchNode,
    executor: CgbBondAnalyticsNativeNode,
    request_external: ExecutionExternalInput,
    materialized_external: ExecutionExternalInput,
    identity: ReproducibilityIdentity,
}

fn fixture(
    request: &AnalyzeBondRequest,
    materialized: &ficant_domain::analytics::BondAnalyticsInput,
) -> Fixture {
    let node_id = id('A');
    let node = ResearchNode::new(
        node_id.clone(),
        cgb_bond_analytics_contract().unwrap(),
        hash(b"no-parameters"),
    );
    let graph = ResearchGraph::new_with_external_inputs(
        ResearchGraphInput {
            graph_id: id('P'),
            version: Version::new(1).unwrap(),
            owner: OwnerRef::new(id('0'), id('1')),
            nodes: vec![node.clone()],
            edges: vec![],
        },
        vec![
            GraphExternalInput::new("bond-request", analyze_bond_request_type()).unwrap(),
            GraphExternalInput::new("materialized-bond-input", materialized_bond_input_type())
                .unwrap(),
        ],
        vec![
            GraphExternalInputBinding::new("bond-request", node_id.clone(), REQUEST_PORT).unwrap(),
            GraphExternalInputBinding::new(
                "materialized-bond-input",
                node_id.clone(),
                MATERIALIZED_INPUT_PORT,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let executor = CgbBondAnalyticsNativeNode::new(node_id.clone()).unwrap();
    let request_external = ExecutionExternalInput::new(
        "bond-request",
        analyze_bond_request_type(),
        request.encode_to_vec(),
    )
    .unwrap();
    let materialized_external = ExecutionExternalInput::new(
        "materialized-bond-input",
        materialized_bond_input_type(),
        encode_materialized_bond_input(
            materialized,
            &private_tax_treatment(),
            &semantic_hash(),
            &supplied_metadata(request, materialized),
        ),
    )
    .unwrap();
    let rule = request.tax_rule_pack.as_ref().unwrap();
    let identity = ReproducibilityIdentity::new(
        &graph,
        identity_input(
            &executor,
            node_id,
            vec![request_external.clone(), materialized_external.clone()],
            rule,
        ),
    )
    .unwrap();
    Fixture {
        graph,
        node,
        executor,
        request_external,
        materialized_external,
        identity,
    }
}

fn identity_input(
    executor: &CgbBondAnalyticsNativeNode,
    node_id: Ulid,
    external_inputs: Vec<ExecutionExternalInput>,
    rule: &ObjectBinding,
) -> ReproducibilityIdentityInput {
    ReproducibilityIdentityInput {
        external_inputs,
        data_snapshot_hash: hash(b"object-M"),
        universe_snapshot_hash: hash(b"universe"),
        parameters_hash: hash(b"parameters"),
        runtime_image_digest: hash(b"runtime"),
        environment_digest: hash(b"environment"),
        seed: 7,
        rule_pack_bindings: vec![RulePackBinding {
            rule_pack_id: rule
                .object
                .as_ref()
                .unwrap()
                .id
                .as_ref()
                .unwrap()
                .value
                .clone(),
            version: Version::new(rule.object.as_ref().unwrap().version).unwrap(),
            content_hash: ContentHash::from_bytes(&rule.content_hash.as_ref().unwrap().value)
                .unwrap(),
        }],
        node_implementations: vec![NodeImplementation {
            node_id,
            implementation_digest: executor.implementation_digest().clone(),
        }],
    }
}

fn port_inputs(fixture: &Fixture) -> Vec<NativePortValue> {
    vec![
        NativePortValue::new(
            REQUEST_PORT,
            analyze_bond_request_type(),
            fixture.request_external.payload().to_vec(),
        )
        .unwrap(),
        NativePortValue::new(
            MATERIALIZED_INPUT_PORT,
            materialized_bond_input_type(),
            fixture.materialized_external.payload().to_vec(),
        )
        .unwrap(),
    ]
}

fn execute(fixture: &Fixture) -> Result<ficant_runtime::NativeNodeExecution, RuntimeError> {
    execute_native_node(
        &fixture.node,
        &fixture.identity,
        &fixture.executor,
        port_inputs(fixture),
        vec![
            fixture.request_external.content_hash().clone(),
            fixture.materialized_external.content_hash().clone(),
        ],
    )
}

#[test]
fn golden_cgb_node_calculates_from_exact_materialized_input() {
    let request = golden_request();
    let input = materialized_input(FixedDecimal::from_scaled(15_500_000_000));
    let fixture = fixture(&request, &input);
    let execution = execute(&fixture).unwrap();
    let decoded = AnalyzeBondResult::decode(execution.outputs()[0].payload()).unwrap();

    assert_eq!(decoded.cashflows.len(), 5);
    let measures = decoded.measures.unwrap();
    assert_eq!(measures.clean_price.unwrap().coefficient, "99770427052063");
    assert!(decoded.after_tax.is_some());
    assert_eq!(decoded.metadata, Some(supplied_metadata(&request, &input)));
    assert_eq!(
        execution.outputs()[0].content_hash(),
        &ContentHash::digest(execution.outputs()[0].payload())
    );
}

#[test]
fn run_identity_does_not_change_node_output_content() {
    let fixture = fixture(
        &golden_request(),
        &materialized_input(FixedDecimal::from_scaled(15_500_000_000)),
    );
    let first_instance =
        ExecutionInstanceIdentity::from_reproducibility(id('R'), fixture.identity.clone());
    let second_instance =
        ExecutionInstanceIdentity::from_reproducibility(id('S'), fixture.identity.clone());
    let first = execute_native_node(
        &fixture.node,
        first_instance.reproducibility(),
        &fixture.executor,
        port_inputs(&fixture),
        vec![
            fixture.request_external.content_hash().clone(),
            fixture.materialized_external.content_hash().clone(),
        ],
    )
    .unwrap();
    let second = execute_native_node(
        &fixture.node,
        second_instance.reproducibility(),
        &fixture.executor,
        port_inputs(&fixture),
        vec![
            fixture.request_external.content_hash().clone(),
            fixture.materialized_external.content_hash().clone(),
        ],
    )
    .unwrap();
    assert_ne!(first_instance.digest(), second_instance.digest());
    assert_eq!(
        first.outputs()[0].content_hash(),
        second.outputs()[0].content_hash()
    );
    assert_eq!(first.artifact(), second.artifact());
}

#[test]
fn missing_or_drifted_materialized_port_fails_closed() {
    let request = golden_request();
    let input = materialized_input(FixedDecimal::from_scaled(15_500_000_000));
    let fixture = fixture(&request, &input);
    let rule = request.tax_rule_pack.as_ref().unwrap();
    assert_eq!(
        ReproducibilityIdentity::new(
            &fixture.graph,
            identity_input(
                &fixture.executor,
                fixture.node.node_id().clone(),
                vec![fixture.request_external.clone()],
                rule,
            ),
        ),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
    assert_eq!(
        execute_native_node(
            &fixture.node,
            &fixture.identity,
            &fixture.executor,
            vec![port_inputs(&fixture).remove(0)],
            vec![],
        ),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
    let mut drifted = fixture.materialized_external.payload().to_vec();
    drifted.push(0);
    assert_eq!(
        execute_native_node(
            &fixture.node,
            &fixture.identity,
            &fixture.executor,
            vec![
                port_inputs(&fixture).remove(0),
                NativePortValue::new(
                    MATERIALIZED_INPUT_PORT,
                    materialized_bond_input_type(),
                    drifted,
                )
                .unwrap(),
            ],
            vec![],
        ),
        Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch))
    );

    let changed_input = materialized_input(FixedDecimal::from_scaled(16_000_000_000));
    let original_input = materialized_input(FixedDecimal::from_scaled(15_500_000_000));
    let original_request = golden_request();
    let changed_external = ExecutionExternalInput::new(
        "materialized-bond-input",
        materialized_bond_input_type(),
        encode_materialized_bond_input(
            &changed_input,
            &private_tax_treatment(),
            &semantic_hash(),
            &supplied_metadata(&original_request, &original_input),
        ),
    )
    .unwrap();
    let changed_identity = ReproducibilityIdentity::new(
        &fixture.graph,
        identity_input(
            &fixture.executor,
            fixture.node.node_id().clone(),
            vec![fixture.request_external.clone(), changed_external.clone()],
            rule,
        ),
    )
    .unwrap();
    assert_eq!(
        execute_native_node(
            &fixture.node,
            &changed_identity,
            &fixture.executor,
            vec![
                port_inputs(&fixture).remove(0),
                NativePortValue::new(
                    MATERIALIZED_INPUT_PORT,
                    materialized_bond_input_type(),
                    changed_external.payload().to_vec(),
                )
                .unwrap(),
            ],
            vec![
                fixture.request_external.content_hash().clone(),
                changed_external.content_hash().clone(),
            ],
        ),
        Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch))
    );
}

#[test]
fn public_snapshot_rule_and_algorithm_drift_fail_closed() {
    let original = golden_request();
    for mutation in 0..3 {
        let mut changed = original.clone();
        match mutation {
            0 => {
                changed.data_snapshot.as_mut().unwrap().content_hash =
                    Some(proto_hash(&hash(b"changed-snapshot")));
            }
            1 => {
                changed.tax_rule_pack.as_mut().unwrap().content_hash =
                    Some(proto_hash(&hash(b"changed-rule")));
            }
            _ => {
                changed
                    .context
                    .as_mut()
                    .unwrap()
                    .algorithm
                    .as_mut()
                    .unwrap()
                    .algorithm_id = "ficant.wrong.algorithm".to_owned();
            }
        }
        let base = fixture(
            &original,
            &materialized_input(FixedDecimal::from_scaled(15_500_000_000)),
        );
        let changed_request = ExecutionExternalInput::new(
            "bond-request",
            analyze_bond_request_type(),
            changed.encode_to_vec(),
        )
        .unwrap();
        let identity = ReproducibilityIdentity::new(
            &base.graph,
            identity_input(
                &base.executor,
                base.node.node_id().clone(),
                vec![changed_request.clone(), base.materialized_external.clone()],
                original.tax_rule_pack.as_ref().unwrap(),
            ),
        )
        .unwrap();
        let outcome = execute_native_node(
            &base.node,
            &identity,
            &base.executor,
            vec![
                NativePortValue::new(
                    REQUEST_PORT,
                    analyze_bond_request_type(),
                    changed_request.payload().to_vec(),
                )
                .unwrap(),
                port_inputs(&base).remove(1),
            ],
            vec![
                changed_request.content_hash().clone(),
                base.materialized_external.content_hash().clone(),
            ],
        );
        assert!(outcome.is_err(), "mutation {mutation} must fail closed");
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn real_bond_analysis_flows_through_typed_risk_summary_dependency() {
    let request = golden_request();
    let input = materialized_input(FixedDecimal::from_scaled(15_500_000_000));
    let analysis_id = id('A');
    let risk_id = id('B');
    let analysis = ResearchNode::new(
        analysis_id.clone(),
        cgb_bond_analytics_contract().unwrap(),
        hash(b"no-parameters"),
    );
    let risk = ResearchNode::new(
        risk_id.clone(),
        cgb_bond_risk_summary_contract().unwrap(),
        hash(b"no-parameters"),
    );
    let graph = ResearchGraph::new_with_external_inputs(
        ResearchGraphInput {
            graph_id: id('P'),
            version: Version::new(1).unwrap(),
            owner: OwnerRef::new(id('0'), id('1')),
            nodes: vec![risk.clone(), analysis.clone()],
            edges: vec![
                ResearchEdge::new(
                    analysis_id.clone(),
                    RESULT_PORT,
                    risk_id.clone(),
                    RISK_INPUT_PORT,
                )
                .unwrap(),
            ],
        },
        vec![
            GraphExternalInput::new("bond-request", analyze_bond_request_type()).unwrap(),
            GraphExternalInput::new("materialized-bond-input", materialized_bond_input_type())
                .unwrap(),
        ],
        vec![
            GraphExternalInputBinding::new("bond-request", analysis_id.clone(), REQUEST_PORT)
                .unwrap(),
            GraphExternalInputBinding::new(
                "materialized-bond-input",
                analysis_id.clone(),
                MATERIALIZED_INPUT_PORT,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let analytics = CgbBondAnalyticsNativeNode::new(analysis_id.clone()).unwrap();
    let risk_summary = CgbBondRiskSummaryNativeNode::new(risk_id.clone()).unwrap();
    let request_external = ExecutionExternalInput::new(
        "bond-request",
        analyze_bond_request_type(),
        request.encode_to_vec(),
    )
    .unwrap();
    let materialized_external = ExecutionExternalInput::new(
        "materialized-bond-input",
        materialized_bond_input_type(),
        encode_materialized_bond_input(
            &input,
            &private_tax_treatment(),
            &semantic_hash(),
            &supplied_metadata(&request, &input),
        ),
    )
    .unwrap();
    let mut identity_data = identity_input(
        &analytics,
        analysis_id,
        vec![request_external.clone(), materialized_external.clone()],
        request.tax_rule_pack.as_ref().unwrap(),
    );
    identity_data.node_implementations.push(NodeImplementation {
        node_id: risk_id,
        implementation_digest: risk_summary.implementation_digest().clone(),
    });
    let identity = ReproducibilityIdentity::new(&graph, identity_data).unwrap();

    let first = execute_native_node(
        &analysis,
        &identity,
        &analytics,
        vec![
            NativePortValue::new(
                REQUEST_PORT,
                analyze_bond_request_type(),
                request_external.payload().to_vec(),
            )
            .unwrap(),
            NativePortValue::new(
                MATERIALIZED_INPUT_PORT,
                materialized_bond_input_type(),
                materialized_external.payload().to_vec(),
            )
            .unwrap(),
        ],
        vec![
            request_external.content_hash().clone(),
            materialized_external.content_hash().clone(),
        ],
    )
    .unwrap();
    let second = execute_native_node(
        &risk,
        &identity,
        &risk_summary,
        vec![
            NativePortValue::new(
                RISK_INPUT_PORT,
                ficant_native_nodes::analyze_bond_result_type(),
                first.outputs()[0].payload().to_vec(),
            )
            .unwrap(),
        ],
        vec![first.artifact().output_envelope_hash().clone()],
    )
    .unwrap();
    let summary = RiskSummary::decode(second.outputs()[0].payload()).unwrap();
    let source = AnalyzeBondResult::decode(first.outputs()[0].payload()).unwrap();
    let measures = source.measures.unwrap();
    assert_eq!(second.outputs()[0].port_name(), RISK_OUTPUT_PORT);
    assert_eq!(summary.modified_duration, measures.modified_duration);
    assert_eq!(summary.convexity, measures.convexity);
    assert_eq!(summary.dv01, measures.dv01);
    assert_eq!(summary.source_metadata, source.metadata);
    assert!(trusted_native_node(&analysis).is_ok());
    assert!(trusted_native_node(&risk).is_ok());
}

#[test]
fn wrong_materialized_type_is_rejected_by_contract() {
    let fixture = fixture(
        &golden_request(),
        &materialized_input(FixedDecimal::from_scaled(15_500_000_000)),
    );
    let wrong_type = TypedValue::new(
        "ficant.test.wrong",
        Version::new(1).unwrap(),
        hash(b"wrong-schema"),
    )
    .unwrap();
    let mut inputs = port_inputs(&fixture);
    inputs[1] = NativePortValue::new(MATERIALIZED_INPUT_PORT, wrong_type, vec![1]).unwrap();
    assert_eq!(
        execute_native_node(
            &fixture.node,
            &fixture.identity,
            &fixture.executor,
            inputs,
            vec![],
        ),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
}
