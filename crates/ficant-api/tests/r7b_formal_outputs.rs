mod position_outputs {
    include!("../../ficant-application/tests/r4d_b_futures_krd_contracts.rs");

    #[async_trait::async_trait]
    impl ficant_application::ports::SnapshotRepository for Fixture {
        async fn publish_verified_manifest(
            &self,
            _: ficant_application::ports::PublishSnapshot,
        ) -> ficant_application::ports::ApplicationResult<ficant_application::ports::SnapshotValue>
        {
            unreachable!("read-only position fixture")
        }

        async fn get_by_id(
            &self,
            _: &ficant_application::ports::AccessScope,
            _: Ulid,
        ) -> ficant_application::ports::ApplicationResult<
            Option<ficant_application::ports::SnapshotValue>,
        > {
            unreachable!("derived position queries do not read the generic snapshot port")
        }
    }

    #[async_trait::async_trait]
    impl ficant_application::ports::BlobStore for Fixture {
        async fn begin_stage(
            &self,
            _: ficant_application::ports::BeginBlobStage,
        ) -> ficant_application::ports::ApplicationResult<ficant_application::ports::StagedBlobRef>
        {
            unreachable!("read-only position fixture")
        }

        async fn append_chunk(
            &self,
            _: &ficant_application::ports::AccessScope,
            _: &ficant_application::ports::StagedBlobRef,
            _: Vec<u8>,
        ) -> ficant_application::ports::ApplicationResult<()> {
            unreachable!("read-only position fixture")
        }

        async fn verify_and_promote(
            &self,
            _: ficant_application::ports::VerifyBlobStage,
        ) -> ficant_application::ports::ApplicationResult<ficant_application::ports::VerifiedBlobRef>
        {
            unreachable!("read-only position fixture")
        }

        async fn discard_stage(
            &self,
            _: &ficant_application::ports::AccessScope,
            _: &ficant_application::ports::StagedBlobRef,
        ) -> ficant_application::ports::ApplicationResult<()> {
            unreachable!("read-only position fixture")
        }
    }

    #[async_trait::async_trait]
    impl ficant_application::ports::SubjectRepository for Fixture {
        async fn register_subject(
            &self,
            _: ficant_domain::subject::SubjectRecord,
        ) -> ficant_application::ports::ApplicationResult<ficant_domain::subject::SubjectRecord>
        {
            unreachable!("read-only position fixture")
        }

        async fn get_subject(
            &self,
            reference: VersionRef,
        ) -> ficant_application::ports::ApplicationResult<
            Option<ficant_domain::subject::SubjectRecord>,
        > {
            Ok((reference == *self.snapshot.subject_ref()).then(|| fixture_subject(self)))
        }

        async fn register_subject_state(
            &self,
            _: ficant_domain::subject::SubjectStateSnapshot,
        ) -> ficant_application::ports::ApplicationResult<
            ficant_domain::subject::SubjectStateSnapshot,
        > {
            unreachable!("read-only position fixture")
        }

        async fn get_subject_state(
            &self,
            _: Ulid,
            _: chrono::DateTime<chrono::Utc>,
        ) -> ficant_application::ports::ApplicationResult<
            Option<ficant_domain::subject::SubjectStateSnapshot>,
        > {
            unreachable!("read-only position fixture")
        }
    }

    #[derive(Default)]
    struct RecordingFormalOutputs {
        fail: std::sync::atomic::AtomicBool,
        records: std::sync::Mutex<Vec<ficant_application::ports::FormalOutputRecord>>,
    }

    #[async_trait::async_trait]
    impl ficant_application::ports::FormalOutputRepository for RecordingFormalOutputs {
        async fn publish(
            &self,
            scope: &ficant_application::ports::AccessScope,
            record: ficant_application::ports::FormalOutputRecord,
        ) -> ficant_application::ports::ApplicationResult<
            ficant_application::ports::FormalOutputRecord,
        > {
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(ficant_application::ApplicationError::new(
                    ficant_application::ApplicationErrorCategory::StorageUnavailable,
                    true,
                ));
            }
            scope.authorize(record.owner())?;
            record.verify()?;
            self.records.lock().unwrap().push(record.clone());
            Ok(record)
        }

        async fn get(
            &self,
            _: &ficant_application::ports::AccessScope,
            output_identity: &ContentHash,
        ) -> ficant_application::ports::ApplicationResult<
            Option<ficant_application::ports::FormalOutputRecord>,
        > {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .find(|record| record.output_identity() == output_identity)
                .cloned())
        }
    }

    const TRACE_KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn position_service(
        fixture: &Arc<Fixture>,
        repository: Arc<RecordingFormalOutputs>,
    ) -> ficant_api::PositionSnapshotGrpcService {
        use ficant_api::{
            FormalOutputPublisher, PlatformApplication, PlatformPort, SessionPolicy, SystemClock,
            TrustedIdentity,
        };
        use ficant_domain::governance::PlatformRole;

        let publisher = FormalOutputPublisher::new(
            repository,
            ficant_runtime::CodeBinding::new(
                "34402344c7d2c9238dc171af52ac4db77eb6b462",
                "f66e03c55703837d6f2aee9959eba482612272f1",
            )
            .unwrap(),
            ficant_runtime::RuntimeBinding::new(
                ContentHash::digest(b"position-test-server-image"),
                ContentHash::digest(b"position-test-server-environment"),
            ),
        );
        let identity: Arc<dyn PlatformPort> = Arc::new(
            PlatformApplication::try_new(
                Arc::new(SystemClock),
                SessionPolicy::new(900, 60).unwrap(),
                TRACE_KEY,
                Vec::new(),
                Some(
                    TrustedIdentity::implicit(
                        "r7b-position-formal-output",
                        id('A'),
                        id('T'),
                        vec![id('0')],
                        PlatformRole::Researcher,
                        ["positions:read"],
                    )
                    .unwrap(),
                ),
                Vec::new(),
            )
            .unwrap(),
        );
        ficant_api::PositionSnapshotGrpcService::new(
            identity,
            fixture.scope.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            TRACE_KEY,
        )
        .unwrap()
        .with_formal_outputs(fixture.clone(), publisher)
    }

    fn assert_formal_position_output(
        evidence: &ficant_contracts::ficant::core::v1::FormalOutputEvidence,
        schema_id: &str,
        snapshot_id: &Ulid,
    ) {
        use ficant_contracts::ficant::core::v1 as core_pb;

        assert_eq!(evidence.schema_id, schema_id);
        assert!(evidence.subject.is_some());
        assert!(evidence.output_identity.is_some());
        assert_eq!(evidence.consumed_inputs.len(), 1);
        let input = &evidence.consumed_inputs[0];
        assert_eq!(input.role, "position-snapshot");
        assert_eq!(
            input.kind,
            core_pb::FormalInputKind::PositionSnapshot as i32
        );
        let Some(core_pb::formal_input_binding::Reference::ObjectRef(reference)) =
            input.reference.as_ref()
        else {
            panic!("PositionSnapshot evidence must use an exact object reference");
        };
        assert_eq!(
            reference.object_id.as_ref().unwrap().value,
            snapshot_id.as_str()
        );
        assert!(reference.content_hash.is_some());
    }

    fn proto_ulid(value: &Ulid) -> ficant_contracts::ficant::core::v1::Ulid {
        ficant_contracts::ficant::core::v1::Ulid {
            value: value.to_string(),
        }
    }

    fn proto_time(hour: u32) -> ficant_contracts::ficant::core::v1::MarketTime {
        let instant = chrono::Utc
            .with_ymd_and_hms(2026, 8, 3, hour, 0, 0)
            .single()
            .unwrap();
        ficant_contracts::ficant::core::v1::MarketTime {
            instant: Some(prost_types::Timestamp {
                seconds: instant.timestamp(),
                nanos: 0,
            }),
            market_timezone: "Asia/Shanghai".to_owned(),
            local_trading_date: "2026-08-03".to_owned(),
        }
    }

    #[tokio::test]
    async fn position_views_and_capital_use_persist_exact_formal_evidence_before_success() {
        use ficant_contracts::ficant::research::v1 as research_pb;
        use ficant_contracts::ficant::research::v1::position_snapshot_service_server::PositionSnapshotService;
        use tonic::Request;

        let fixture = Arc::new(Fixture::new(true, false, Calls::default()));
        let repository = Arc::new(RecordingFormalOutputs::default());
        let service = position_service(&fixture, repository.clone());

        let views = service
            .get_position_views(Request::new(research_pb::GetPositionViewsRequest {
                snapshot_id: Some(proto_ulid(fixture.snapshot.id())),
                knowledge_at: Some(proto_time(2)),
            }))
            .await
            .unwrap()
            .into_inner();
        let Some(research_pb::get_position_views_response::Result::Views(views)) = views.result
        else {
            panic!("position views must succeed with formal persistence configured");
        };
        assert_formal_position_output(
            views.formal_evidence.as_ref().unwrap(),
            "ficant.research.v1.PositionViews",
            fixture.snapshot.id(),
        );

        let capital = service
            .calculate_capital_use(Request::new(research_pb::CalculateCapitalUseRequest {
                snapshot_id: Some(proto_ulid(fixture.snapshot.id())),
                knowledge_at: Some(proto_time(2)),
            }))
            .await
            .unwrap()
            .into_inner();
        let Some(research_pb::calculate_capital_use_response::Result::CapitalUse(capital)) =
            capital.result
        else {
            panic!("capital use must succeed with formal persistence configured");
        };
        assert_formal_position_output(
            capital.formal_evidence.as_ref().unwrap(),
            "ficant.research.v1.CapitalUse",
            fixture.snapshot.id(),
        );

        {
            let records = repository.records.lock().unwrap();
            assert_eq!(records.len(), 2);
            assert_eq!(
                records
                    .iter()
                    .map(|record| record.evidence().schema_id())
                    .collect::<Vec<_>>(),
                vec![
                    "ficant.research.v1.PositionViews",
                    "ficant.research.v1.CapitalUse",
                ]
            );
        }

        repository
            .fail
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let failed = service
            .get_position_views(Request::new(research_pb::GetPositionViewsRequest {
                snapshot_id: Some(proto_ulid(fixture.snapshot.id())),
                knowledge_at: Some(proto_time(2)),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            failed.result,
            Some(research_pb::get_position_views_response::Result::Error(_))
        ));
        assert_eq!(repository.records.lock().unwrap().len(), 2);
    }

    fn fixture_subject(fixture: &Fixture) -> ficant_domain::subject::SubjectRecord {
        use ficant_domain::subject::{
            AccessSet, FundingTier, Subject, SubjectRecord, SubjectVersion, TaxTreatment,
        };
        let reference = fixture.snapshot.subject_ref().clone();
        SubjectRecord::new(
            Subject::new_owned(
                reference.id().clone(),
                fixture.snapshot.owner().clone(),
                "R7B position fixture subject",
            )
            .unwrap(),
            SubjectVersion::new(
                reference,
                AccessSet::new(["cn.gov"], ["rates"]).unwrap(),
                FundingTier::DrAvailable,
                TaxTreatment::new("vat-none", "income-none").unwrap(),
                "fixture-assessment",
                "fixture-liability",
                None,
            )
            .unwrap(),
        )
        .unwrap()
    }
}
