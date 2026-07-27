use chrono::DateTime;
use ficant_api::analyze_bond_request;
use ficant_contracts::ficant::core::v1::{
    DecimalValue, MarketTime, OwnerRef as ProtoOwnerRef, Sha256, Ulid as ProtoUlid, UnitRef,
    VersionRef,
};
use ficant_contracts::ficant::rates::v1::{
    AlgorithmBinding, AnalysisContext, AnalysisUnits, AnalyzeBondRequest, AnalyzeBondResult,
    BondTerms, CalendarBinding, CalendarRequirement, CouponFrequency, ObjectBinding, RiskSummary,
    analyze_bond_request,
};
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{ALGORITHM_ID, CONVENTION_PROFILE};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, Version};
use ficant_domain::research::{
    GraphExternalInput, GraphExternalInputBinding, ResearchEdge, ResearchGraph, ResearchGraphInput,
    ResearchNode, TypedValue,
};
use ficant_fixed_income_native::NativeBondAnalyticsEngine;
use ficant_native_nodes::{
    CgbBondAnalyticsNativeNode, CgbBondRiskSummaryNativeNode, REQUEST_PORT, RESULT_PORT,
    RISK_INPUT_PORT, RISK_OUTPUT_PORT, analyze_bond_request_type, cgb_bond_analytics_contract,
    cgb_bond_risk_summary_contract, trusted_native_node,
};
use ficant_runtime::{
    ExecutionExternalInput, ExecutionInstanceIdentity, NativeNode, NativePortValue,
    NodeImplementation, ReproducibilityIdentity, ReproducibilityIdentityInput, RulePackBinding,
    RuntimeError, execute_native_node,
};
use prost::Message;
use serde_json::Value;

const PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

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

fn unit(suffix: char) -> UnitRef {
    UnitRef {
        unit_id: Some(proto_id(suffix)),
        version: 1,
    }
}

fn decimal(coefficient: &str, scale: u32, unit: UnitRef) -> DecimalValue {
    DecimalValue {
        coefficient: coefficient.to_owned(),
        scale,
        unit: Some(unit),
    }
}

fn golden_request() -> AnalyzeBondRequest {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/golden-cases/china-rates/fixtures/bond-260008.IB.json"
    ))
    .unwrap();
    let units = AnalysisUnits {
        currency_amount: Some(unit('A')),
        price_per_100: Some(unit('B')),
        rate: Some(unit('C')),
        years: Some(unit('D')),
        years_squared: Some(unit('E')),
        dv01_per_100: Some(unit('F')),
        dv01: Some(unit('G')),
        dimensionless: Some(unit('H')),
        contract_count: Some(unit('J')),
    };
    let instant = DateTime::parse_from_rfc3339(fixture["valuation_at"].as_str().unwrap()).unwrap();
    AnalyzeBondRequest {
        context: Some(AnalysisContext {
            owner: Some(ProtoOwnerRef {
                tenant_id: Some(proto_id('0')),
                owner_id: Some(proto_id('1')),
            }),
            rule_pack: Some(object('K')),
            data_snapshot: Some(object('M')),
            algorithm: Some(AlgorithmBinding {
                algorithm_id: ALGORITHM_ID.to_owned(),
                algorithm_version: 1,
                convention_profile: CONVENTION_PROFILE.to_owned(),
                abi_version: 1,
            }),
            units: Some(units.clone()),
        }),
        bond: Some(object('N')),
        valuation_at: Some(MarketTime {
            instant: Some(prost_types::Timestamp {
                seconds: instant.timestamp(),
                nanos: instant.timestamp_subsec_nanos().cast_signed(),
            }),
            market_timezone: "Asia/Shanghai".to_owned(),
            local_trading_date: "2026-07-13".to_owned(),
        }),
        settlement_date: fixture["settlement_date"].as_str().unwrap().to_owned(),
        calendar_requirement: CalendarRequirement::ReferenceReplay as i32,
        calendar: Some(CalendarBinding {
            calendar_id: fixture["calendar"].as_str().unwrap().to_owned(),
            version: 1,
            content_hash: Some(proto_hash(&hash(b"calendar-cgb-reference-v1"))),
            coverage_start: "2005-01-01".to_owned(),
            coverage_end: "2026-12-31".to_owned(),
            non_business_days: vec![],
            work_weekends: vec![],
        }),
        terms: Some(BondTerms {
            issue_date: fixture["issue_date"].as_str().unwrap().to_owned(),
            maturity_date: fixture["maturity_date"].as_str().unwrap().to_owned(),
            frequency: CouponFrequency::Annual as i32,
            coupon_rate: Some(decimal("15", 3, units.rate.unwrap())),
            face_amount: Some(decimal("100", 0, units.currency_amount.unwrap())),
        }),
        input: Some(analyze_bond_request::Input::YieldToMaturity(decimal(
            "155",
            4,
            unit('C'),
        ))),
        subject_ref: None,
    }
}

struct Fixture {
    graph: ResearchGraph,
    node: ResearchNode,
    executor: CgbBondAnalyticsNativeNode,
    external: ExecutionExternalInput,
    identity: ReproducibilityIdentity,
}

fn fixture(request: &AnalyzeBondRequest) -> Fixture {
    let node_id = id('A');
    let contract = cgb_bond_analytics_contract().unwrap();
    let node = ResearchNode::new(node_id.clone(), contract, hash(b"no-parameters"));
    let graph = ResearchGraph::new_with_external_inputs(
        ResearchGraphInput {
            graph_id: id('G'),
            version: Version::new(1).unwrap(),
            owner: OwnerRef::new(id('T'), id('W')),
            nodes: vec![node.clone()],
            edges: vec![],
        },
        vec![GraphExternalInput::new("bond-request", analyze_bond_request_type()).unwrap()],
        vec![
            GraphExternalInputBinding::new("bond-request", node_id.clone(), REQUEST_PORT).unwrap(),
        ],
    )
    .unwrap();
    let executor = CgbBondAnalyticsNativeNode::new(node_id.clone()).unwrap();
    let payload = request.encode_to_vec();
    let external =
        ExecutionExternalInput::new("bond-request", analyze_bond_request_type(), payload).unwrap();
    let context = request.context.as_ref().unwrap();
    let snapshot_hash = ContentHash::from_bytes(
        &context
            .data_snapshot
            .as_ref()
            .unwrap()
            .content_hash
            .as_ref()
            .unwrap()
            .value,
    )
    .unwrap();
    let rule = context.rule_pack.as_ref().unwrap();
    let rule_ref = rule.object.as_ref().unwrap();
    let identity = ReproducibilityIdentity::new(
        &graph,
        ReproducibilityIdentityInput {
            external_inputs: vec![external.clone()],
            data_snapshot_hash: snapshot_hash,
            universe_snapshot_hash: hash(b"universe"),
            parameters_hash: hash(b"parameters"),
            runtime_image_digest: hash(b"runtime"),
            environment_digest: hash(b"environment"),
            seed: 7,
            rule_pack_bindings: vec![RulePackBinding {
                rule_pack_id: rule_ref.id.as_ref().unwrap().value.clone(),
                version: Version::new(rule_ref.version).unwrap(),
                content_hash: ContentHash::from_bytes(&rule.content_hash.as_ref().unwrap().value)
                    .unwrap(),
            }],
            node_implementations: vec![NodeImplementation {
                node_id,
                implementation_digest: executor.implementation_digest().clone(),
            }],
        },
    )
    .unwrap();
    Fixture {
        graph,
        node,
        executor,
        external,
        identity,
    }
}

fn execute(fixture: &Fixture) -> Result<ficant_runtime::NativeNodeExecution, RuntimeError> {
    execute_native_node(
        &fixture.node,
        &fixture.identity,
        &fixture.executor,
        vec![NativePortValue::new(
            REQUEST_PORT,
            analyze_bond_request_type(),
            fixture.external.payload().to_vec(),
        )?],
        vec![fixture.external.content_hash().clone()],
    )
}

#[test]
fn golden_cgb_node_matches_the_direct_native_api_path() {
    let request = golden_request();
    let direct = analyze_bond_request(&NativeBondAnalyticsEngine, &request).unwrap();
    let fixture = fixture(&request);
    let execution = execute(&fixture).unwrap();
    let decoded = AnalyzeBondResult::decode(execution.outputs()[0].payload()).unwrap();

    assert_eq!(decoded, direct);
    assert_eq!(decoded.cashflows.len(), 5);
    let measures = decoded.measures.unwrap();
    assert_eq!(measures.clean_price.unwrap().coefficient, "99770427052063");
    assert_eq!(
        execution.outputs()[0].content_hash(),
        &ContentHash::digest(execution.outputs()[0].payload())
    );
}

#[test]
fn run_identity_does_not_change_node_output_content() {
    let fixture = fixture(&golden_request());
    let first_instance =
        ExecutionInstanceIdentity::from_reproducibility(id('R'), fixture.identity.clone());
    let second_instance =
        ExecutionInstanceIdentity::from_reproducibility(id('S'), fixture.identity.clone());
    let first = execute_native_node(
        &fixture.node,
        first_instance.reproducibility(),
        &fixture.executor,
        vec![
            NativePortValue::new(
                REQUEST_PORT,
                analyze_bond_request_type(),
                fixture.external.payload().to_vec(),
            )
            .unwrap(),
        ],
        vec![fixture.external.content_hash().clone()],
    )
    .unwrap();
    let second = execute_native_node(
        &fixture.node,
        second_instance.reproducibility(),
        &fixture.executor,
        vec![
            NativePortValue::new(
                REQUEST_PORT,
                analyze_bond_request_type(),
                fixture.external.payload().to_vec(),
            )
            .unwrap(),
        ],
        vec![fixture.external.content_hash().clone()],
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
fn missing_external_and_type_or_payload_drift_fail_closed() {
    let request = golden_request();
    let fixture = fixture(&request);
    let mut missing = ReproducibilityIdentityInput {
        external_inputs: vec![],
        data_snapshot_hash: fixture.identity.data_snapshot_hash().clone(),
        universe_snapshot_hash: hash(b"universe"),
        parameters_hash: hash(b"parameters"),
        runtime_image_digest: hash(b"runtime"),
        environment_digest: hash(b"environment"),
        seed: 7,
        rule_pack_bindings: fixture.identity.rule_pack_bindings().to_vec(),
        node_implementations: fixture.identity.node_implementations().to_vec(),
    };
    assert_eq!(
        ReproducibilityIdentity::new(&fixture.graph, missing.clone()),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
    missing.external_inputs = vec![fixture.external.clone()];

    let wrong_type = TypedValue::new(
        "ficant.test.wrong",
        Version::new(1).unwrap(),
        hash(b"wrong-schema"),
    )
    .unwrap();
    assert_eq!(
        execute_native_node(
            &fixture.node,
            &fixture.identity,
            &fixture.executor,
            vec![NativePortValue::new(REQUEST_PORT, wrong_type, request.encode_to_vec()).unwrap()],
            vec![]
        ),
        Err(RuntimeError::Domain(DomainErrorCode::InvalidValue))
    );
    let mut changed = request.encode_to_vec();
    changed.push(0);
    assert_eq!(
        execute_native_node(
            &fixture.node,
            &fixture.identity,
            &fixture.executor,
            vec![NativePortValue::new(REQUEST_PORT, analyze_bond_request_type(), changed).unwrap()],
            vec![]
        ),
        Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch))
    );
}

#[test]
fn snapshot_rule_and_algorithm_drift_fail_closed() {
    let original = golden_request();
    for mutation in 0..3 {
        let mut changed = original.clone();
        match mutation {
            0 => {
                changed
                    .context
                    .as_mut()
                    .unwrap()
                    .data_snapshot
                    .as_mut()
                    .unwrap()
                    .content_hash = Some(proto_hash(&hash(b"changed-snapshot")));
            }
            1 => {
                changed
                    .context
                    .as_mut()
                    .unwrap()
                    .rule_pack
                    .as_mut()
                    .unwrap()
                    .content_hash = Some(proto_hash(&hash(b"changed-rule")));
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
        let original_fixture = fixture(&original);
        let changed_external = ExecutionExternalInput::new(
            "bond-request",
            analyze_bond_request_type(),
            changed.encode_to_vec(),
        )
        .unwrap();
        let identity_input = ReproducibilityIdentityInput {
            external_inputs: vec![changed_external.clone()],
            data_snapshot_hash: original_fixture.identity.data_snapshot_hash().clone(),
            universe_snapshot_hash: hash(b"universe"),
            parameters_hash: hash(b"parameters"),
            runtime_image_digest: hash(b"runtime"),
            environment_digest: hash(b"environment"),
            seed: 7,
            rule_pack_bindings: original_fixture.identity.rule_pack_bindings().to_vec(),
            node_implementations: original_fixture.identity.node_implementations().to_vec(),
        };
        let identity =
            ReproducibilityIdentity::new(&original_fixture.graph, identity_input.clone()).unwrap();
        let outcome = execute_native_node(
            &original_fixture.node,
            &identity,
            &original_fixture.executor,
            vec![
                NativePortValue::new(
                    REQUEST_PORT,
                    analyze_bond_request_type(),
                    changed_external.payload().to_vec(),
                )
                .unwrap(),
            ],
            vec![changed_external.content_hash().clone()],
        );
        assert!(outcome.is_err(), "mutation {mutation} must fail closed");
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn real_bond_analysis_flows_through_typed_risk_summary_dependency() {
    let request = golden_request();
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
            graph_id: id('G'),
            version: Version::new(1).unwrap(),
            owner: OwnerRef::new(id('T'), id('W')),
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
        vec![GraphExternalInput::new("bond-request", analyze_bond_request_type()).unwrap()],
        vec![
            GraphExternalInputBinding::new("bond-request", analysis_id.clone(), REQUEST_PORT)
                .unwrap(),
        ],
    )
    .unwrap();
    let analytics = CgbBondAnalyticsNativeNode::new(analysis_id.clone()).unwrap();
    let risk_summary = CgbBondRiskSummaryNativeNode::new(risk_id.clone()).unwrap();
    let payload = request.encode_to_vec();
    let external =
        ExecutionExternalInput::new("bond-request", analyze_bond_request_type(), payload).unwrap();
    let context = request.context.as_ref().unwrap();
    let rule = context.rule_pack.as_ref().unwrap();
    let identity = ReproducibilityIdentity::new(
        &graph,
        ReproducibilityIdentityInput {
            external_inputs: vec![external.clone()],
            data_snapshot_hash: ContentHash::from_bytes(
                &context
                    .data_snapshot
                    .as_ref()
                    .unwrap()
                    .content_hash
                    .as_ref()
                    .unwrap()
                    .value,
            )
            .unwrap(),
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
            node_implementations: vec![
                NodeImplementation {
                    node_id: analysis_id,
                    implementation_digest: analytics.implementation_digest().clone(),
                },
                NodeImplementation {
                    node_id: risk_id,
                    implementation_digest: risk_summary.implementation_digest().clone(),
                },
            ],
        },
    )
    .unwrap();

    let first = execute_native_node(
        &analysis,
        &identity,
        &analytics,
        vec![
            NativePortValue::new(
                REQUEST_PORT,
                analyze_bond_request_type(),
                external.payload().to_vec(),
            )
            .unwrap(),
        ],
        vec![external.content_hash().clone()],
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
    assert_ne!(
        first.outputs()[0].content_hash(),
        first.artifact().output_envelope_hash(),
        "the per-port payload hash is distinct from the canonical output-envelope Artifact hash"
    );
    assert_eq!(second.outputs()[0].port_name(), RISK_OUTPUT_PORT);
    assert_eq!(summary.modified_duration, measures.modified_duration);
    assert_eq!(summary.convexity, measures.convexity);
    assert_eq!(summary.dv01, measures.dv01);
    assert_eq!(summary.source_metadata, source.metadata);
    assert!(trusted_native_node(&analysis).is_ok());
    assert!(trusted_native_node(&risk).is_ok());
}
