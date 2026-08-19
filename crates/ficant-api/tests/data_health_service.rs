use ficant_api::DataHealthGrpcService;
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::research::v1 as pb;
use ficant_contracts::ficant::research::v1::data_health_service_server::DataHealthService;
use prost::Message;

#[test]
fn adapter_implements_the_frozen_data_health_service_contract() {
    fn assert_service<T: DataHealthService>() {}
    assert_service::<DataHealthGrpcService>();
}

#[test]
fn verified_empty_report_round_trips_a_positive_state_and_explicit_zero_coverage() {
    let report = pb::DataHealthReport {
        owner: Some(owner()),
        subject_ref: Some(version_ref('S')),
        position_snapshot_id: Some(ulid('P')),
        position_snapshot_hash: Some(hash(1)),
        threshold_profile: Some(pb::DataHealthThresholdProfile {
            profile_ref: Some(version_ref('T')),
            unknown_accounting_warning_basis_points: 5_000,
            model_valuation_warning_basis_points: 5_000,
            content_hash: Some(hash(2)),
            ..Default::default()
        }),
        state: pb::DataHealthState::Warning as i32,
        issues: vec![pb::DataHealthIssue {
            code: pb::DataHealthIssueCode::EmptyPositions as i32,
            ..Default::default()
        }],
        position_set_state: pb::PositionSetState::VerifiedEmpty as i32,
        coverage: Some(pb::CoverageDeclaration::default()),
        request_fingerprint: Some(hash(3)),
        content_hash: Some(hash(4)),
        lineage: vec![core::LineageRef {
            object_id: Some(ulid('L')),
            content_hash: Some(hash(5)),
            ..Default::default()
        }],
        ..Default::default()
    };

    let decoded = pb::DataHealthReport::decode(report.encode_to_vec().as_slice()).unwrap();
    assert_eq!(
        decoded.position_set_state,
        pb::PositionSetState::VerifiedEmpty as i32
    );
    let coverage = decoded.coverage.unwrap();
    assert_eq!(coverage.imported_position_count, 0);
    assert_eq!(coverage.participating_position_count, 0);
    assert!(coverage.imported_gross_economic_value_by_unit.is_empty());
    assert!(
        coverage
            .participating_gross_economic_value_by_unit
            .is_empty()
    );
}

#[test]
fn default_constructed_report_is_not_a_verified_empty_report() {
    let report = pb::DataHealthReport::default();
    assert_eq!(
        report.position_set_state,
        pb::PositionSetState::Unspecified as i32
    );
    assert!(report.coverage.is_none());
}

fn owner() -> core::OwnerRef {
    core::OwnerRef {
        tenant_id: Some(ulid('T')),
        owner_id: Some(ulid('O')),
    }
}

fn version_ref(suffix: char) -> core::VersionRef {
    core::VersionRef {
        id: Some(ulid(suffix)),
        version: 1,
    }
}

fn ulid(suffix: char) -> core::Ulid {
    let suffix = match suffix {
        'L' => '2',
        'O' => '3',
        _ => suffix,
    };
    core::Ulid {
        value: format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}"),
    }
}

fn hash(byte: u8) -> core::Sha256 {
    core::Sha256 {
        value: vec![byte; 32],
    }
}

mod wire_byte_regression {
    include!("../../ficant-application/tests/r4d_b_futures_krd_contracts.rs");

    #[async_trait::async_trait]
    impl ficant_application::ports::DataHealthThresholdProfileRepository for Fixture {
        async fn get_exact(
            &self,
            _: &ficant_application::ports::AccessScope,
            reference: VersionRef,
            knowledge_at: MarketTime,
        ) -> ficant_application::ports::ApplicationResult<
            Option<ficant_domain::research::DataHealthThresholdProfile>,
        > {
            let profile = health_profile();
            Ok((reference == *profile.profile_ref()
                && profile.visible_at().instant() <= knowledge_at.instant())
            .then_some(profile))
        }

        async fn resolve_active(
            &self,
            _: &ficant_application::ports::AccessScope,
            owner: OwnerRef,
            evaluated_at: MarketTime,
        ) -> ficant_application::ports::ApplicationResult<
            Option<ficant_domain::research::DataHealthThresholdProfile>,
        > {
            let profile = health_profile();
            Ok((owner == *profile.owner()
                && profile.visible_at().instant() <= evaluated_at.instant()
                && profile.effective_from().instant() <= evaluated_at.instant()
                && evaluated_at.instant() < profile.effective_to().instant())
            .then_some(profile))
        }
    }

    #[async_trait::async_trait]
    impl ficant_application::ports::SnapshotRepository for Fixture {
        async fn publish_verified_manifest(
            &self,
            _: ficant_application::ports::PublishSnapshot,
        ) -> ficant_application::ports::ApplicationResult<ficant_application::ports::SnapshotValue>
        {
            unreachable!("read-only adapter fixture")
        }

        async fn get_by_id(
            &self,
            _: &ficant_application::ports::AccessScope,
            _: Ulid,
        ) -> ficant_application::ports::ApplicationResult<
            Option<ficant_application::ports::SnapshotValue>,
        > {
            unreachable!("read-only adapter fixture")
        }
    }

    #[async_trait::async_trait]
    impl ficant_application::ports::BlobStore for Fixture {
        async fn begin_stage(
            &self,
            _: ficant_application::ports::BeginBlobStage,
        ) -> ficant_application::ports::ApplicationResult<ficant_application::ports::StagedBlobRef>
        {
            unreachable!("read-only adapter fixture")
        }

        async fn append_chunk(
            &self,
            _: &ficant_application::ports::AccessScope,
            _: &ficant_application::ports::StagedBlobRef,
            _: Vec<u8>,
        ) -> ficant_application::ports::ApplicationResult<()> {
            unreachable!("read-only adapter fixture")
        }

        async fn verify_and_promote(
            &self,
            _: ficant_application::ports::VerifyBlobStage,
        ) -> ficant_application::ports::ApplicationResult<ficant_application::ports::VerifiedBlobRef>
        {
            unreachable!("read-only adapter fixture")
        }

        async fn discard_stage(
            &self,
            _: &ficant_application::ports::AccessScope,
            _: &ficant_application::ports::StagedBlobRef,
        ) -> ficant_application::ports::ApplicationResult<()> {
            unreachable!("read-only adapter fixture")
        }
    }

    use ficant_api::{
        DataHealthGrpcService, PlatformApplication, PlatformPort, PortfolioRiskGrpcService,
        SessionPolicy, SystemClock, TrustedIdentity,
    };
    use ficant_contracts::ficant::core::v1 as core_pb;
    use ficant_contracts::ficant::research::v1 as research_pb;
    use ficant_contracts::ficant::research::v1::data_health_service_server::DataHealthService;
    use ficant_contracts::ficant::research::v1::portfolio_risk_service_server::PortfolioRiskService;
    use ficant_domain::governance::PlatformRole;
    use prost::Message as _;
    use tonic::Request;

    const TRACE_KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    struct StatelessDeliveryEngine {
        calls: Arc<AtomicUsize>,
    }

    impl FuturesDeliveryEngine for StatelessDeliveryEngine {
        fn calculate(
            &self,
            input: &FuturesDeliverableInput,
        ) -> Result<FuturesDeliveryResult, AnalyticsError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(input.financing_rate(), FixedDecimal::ZERO);
            let implied_repo_rate = if input.bond().version_ref().id() == &id('B') {
                fixed(2)
            } else {
                fixed(1)
            };
            Ok(FuturesDeliveryResult::new(
                input.clone(),
                FuturesDeliveryMeasures::new(
                    1,
                    1,
                    fixed(1),
                    FixedDecimal::ZERO,
                    FixedDecimal::ZERO,
                    FixedDecimal::ZERO,
                    fixed(100),
                    fixed(100),
                    FixedDecimal::ZERO,
                    FixedDecimal::ZERO,
                    FixedDecimal::ZERO,
                    FixedDecimal::ZERO,
                    implied_repo_rate,
                    FixedDecimal::ZERO,
                )
                .map_err(|_| AnalyticsError::Internal)?,
            ))
        }
    }

    #[tokio::test]
    async fn response_bytes_are_identical_before_and_after_a_warning_health_query() {
        let mut fixture = Fixture::new(true, false, Calls::default());
        fixture.snapshot = worst_health_snapshot(&fixture.snapshot);
        let fixture = Arc::new(fixture);
        let identity = identity();
        let delivery_calls = Arc::new(AtomicUsize::new(0));
        let portfolio = PortfolioRiskGrpcService::new(
            identity.clone(),
            fixture.scope.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            Arc::new(StatelessDeliveryEngine {
                calls: delivery_calls.clone(),
            }),
            TRACE_KEY,
        )
        .unwrap();
        let health = DataHealthGrpcService::new(
            identity,
            fixture.scope.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            TRACE_KEY,
        )
        .unwrap();

        let unauthorized_publish = health
            .publish_data_health_threshold_profile(Request::new(
                research_pb::PublishDataHealthThresholdProfileRequest::default(),
            ))
            .await
            .unwrap()
            .into_inner();
        let Some(research_pb::publish_data_health_threshold_profile_response::Result::Error(error)) =
            unauthorized_publish.result
        else {
            panic!("researcher must not publish a foundation threshold profile");
        };
        assert_eq!(error.code, core_pb::ErrorCode::Forbidden as i32);

        let frozen_request = krd_request(&fixture).encode_to_vec();
        let before = portfolio
            .calculate_key_rate_dv01(Request::new(
                research_pb::CalculateKeyRateDv01Request::decode(frozen_request.as_slice())
                    .unwrap(),
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            before.result,
            Some(research_pb::calculate_key_rate_dv01_response::Result::Exposure(_))
        ));
        let before_bytes = before.encode_to_vec();
        let engine_calls_before_health = engine_calls(&fixture, &delivery_calls);

        assert_repeatable_warning_health_report(
            &health,
            &fixture,
            &delivery_calls,
            engine_calls_before_health,
        )
        .await;

        let after = portfolio
            .calculate_key_rate_dv01(Request::new(
                research_pb::CalculateKeyRateDv01Request::decode(frozen_request.as_slice())
                    .unwrap(),
            ))
            .await
            .unwrap()
            .into_inner();
        let after_bytes = after.encode_to_vec();

        assert_eq!(
            after_bytes, before_bytes,
            "the complete protobuf response bytes must not change after a warning health query"
        );
    }

    async fn assert_repeatable_warning_health_report(
        health: &impl DataHealthService,
        fixture: &Fixture,
        delivery_calls: &AtomicUsize,
        engine_calls_before_health: (usize, usize, usize, usize),
    ) {
        let frozen_health_request = health_request(fixture).encode_to_vec();
        let health_response = health
            .get_data_health_report(Request::new(
                research_pb::GetDataHealthReportRequest::decode(frozen_health_request.as_slice())
                    .unwrap(),
            ))
            .await
            .unwrap()
            .into_inner();
        let health_response_bytes = health_response.encode_to_vec();
        assert_eq!(
            engine_calls(fixture, delivery_calls),
            engine_calls_before_health,
            "the health query must not call any numerical engine"
        );
        let repeated_health_response = health
            .get_data_health_report(Request::new(
                research_pb::GetDataHealthReportRequest::decode(frozen_health_request.as_slice())
                    .unwrap(),
            ))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            repeated_health_response.encode_to_vec(),
            health_response_bytes,
            "the same health request bytes and stored facts must produce identical response bytes"
        );
        assert_eq!(
            engine_calls(fixture, delivery_calls),
            engine_calls_before_health,
            "repeating the health query must not call any numerical engine"
        );
        let Some(research_pb::get_data_health_report_response::Result::Report(report)) =
            health_response.result
        else {
            panic!("the worst-health fixture must produce a report");
        };
        assert_eq!(report.state, research_pb::DataHealthState::Warning as i32);
        assert_eq!(
            report
                .issues
                .iter()
                .map(|issue| issue.code)
                .collect::<Vec<_>>(),
            vec![
                research_pb::DataHealthIssueCode::UnknownAccountingClassification as i32,
                research_pb::DataHealthIssueCode::StalePositionSnapshot as i32,
            ]
        );
    }

    fn engine_calls(
        fixture: &Fixture,
        delivery_calls: &AtomicUsize,
    ) -> (usize, usize, usize, usize) {
        (
            fixture.calls.curve.load(Ordering::SeqCst),
            fixture.calls.bond.load(Ordering::SeqCst),
            fixture.calls.parser.load(Ordering::SeqCst),
            delivery_calls.load(Ordering::SeqCst),
        )
    }

    fn identity() -> Arc<dyn PlatformPort> {
        let identity = TrustedIdentity::implicit(
            "r5c-wire-byte-regression",
            id('A'),
            id('T'),
            vec![id('0')],
            PlatformRole::Researcher,
            ["rates:analyze", "data-health:read", "data-health:configure"],
        )
        .unwrap();
        Arc::new(
            PlatformApplication::try_new(
                Arc::new(SystemClock),
                SessionPolicy::new(900, 60).unwrap(),
                TRACE_KEY,
                Vec::new(),
                Some(identity),
                Vec::new(),
            )
            .unwrap(),
        )
    }

    fn krd_request(fixture: &Fixture) -> research_pb::CalculateKeyRateDv01Request {
        research_pb::CalculateKeyRateDv01Request {
            position_snapshot_id: Some(proto_ulid(fixture.snapshot.id())),
            knowledge_at: Some(proto_time(2)),
            valuation_at: Some(proto_time(1)),
            curve_snapshot_id: Some(proto_ulid(fixture.curve.id())),
            dv01_unit: Some(core_pb::UnitRef {
                unit_id: Some(proto_ulid(&id('D'))),
                version: 1,
            }),
            futures_data_snapshot_id: Some(proto_ulid(fixture.data.id())),
        }
    }

    fn health_request(fixture: &Fixture) -> research_pb::GetDataHealthReportRequest {
        research_pb::GetDataHealthReportRequest {
            subject_ref: Some(core_pb::VersionRef {
                id: Some(proto_ulid(fixture.snapshot.subject_ref().id())),
                version: fixture.snapshot.subject_ref().version().get(),
            }),
            position_snapshot_id: Some(proto_ulid(fixture.snapshot.id())),
            data_snapshot_id: None,
            evaluated_at: Some(proto_time(2)),
        }
    }

    fn health_profile() -> ficant_domain::research::DataHealthThresholdProfile {
        let mut input = ficant_domain::research::DataHealthThresholdProfileInput {
            profile_snapshot_id: id('H'),
            owner: owner(),
            profile_ref: VersionRef::new(id('P'), version()),
            visible_at: time(0),
            effective_from: time(0),
            effective_to: time(10),
            max_position_snapshot_age_seconds: 5_400,
            unknown_accounting_warning_basis_points: 1,
            max_data_snapshot_age_seconds: 5_400,
            model_valuation_warning_basis_points: 1,
            content_hash: ContentHash::digest(b"pending"),
            lineage: Vec::new(),
        };
        input.content_hash =
            ficant_domain::research::DataHealthThresholdProfile::content_hash_for(&input);
        ficant_domain::research::DataHealthThresholdProfile::new(input).unwrap()
    }

    fn proto_ulid(value: &Ulid) -> core_pb::Ulid {
        core_pb::Ulid {
            value: value.to_string(),
        }
    }

    fn proto_time(hour: u32) -> core_pb::MarketTime {
        let instant = Utc
            .with_ymd_and_hms(2026, 8, 3, hour, 0, 0)
            .single()
            .unwrap();
        core_pb::MarketTime {
            instant: Some(prost_types::Timestamp {
                seconds: instant.timestamp(),
                nanos: 0,
            }),
            market_timezone: "Asia/Shanghai".to_owned(),
            local_trading_date: "2026-08-03".to_owned(),
        }
    }

    fn worst_health_snapshot(snapshot: &PositionSnapshot) -> PositionSnapshot {
        let positions = snapshot
            .positions()
            .iter()
            .map(|position| {
                Position::new(PositionInput {
                    position_id: position.id().clone(),
                    instrument_ref: position.instrument_ref().clone(),
                    quantity: position.quantity().clone(),
                    economic_value: position.economic_value().clone(),
                    economic_pnl: position.economic_pnl().clone(),
                    accounting_pnl: position.accounting_pnl().clone(),
                    capital_requirement: position.capital_requirement().clone(),
                    accounting_classification: AccountingClassification::new(
                        AccountingClassificationState::Unknown,
                        None,
                    )
                    .unwrap(),
                    holding_form: position.holding_form(),
                })
                .unwrap()
            })
            .collect();
        let mut input = PositionSnapshotInput {
            snapshot_id: snapshot.id().clone(),
            owner: snapshot.owner().clone(),
            subject_ref: snapshot.subject_ref().clone(),
            observed_at: time_for(2026, 8, 2, 0),
            visible_at: snapshot.visible_at().clone(),
            content_hash: ContentHash::digest(b"pending"),
            lineage: snapshot.lineage().to_vec(),
            positions,
        };
        input.content_hash = PositionSnapshot::content_hash_for(&input);
        PositionSnapshot::new(input).unwrap()
    }
}
