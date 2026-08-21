use std::collections::VecDeque;
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, AeadCursorCodec, AppendDefinitionVersion, AppendMarketFact, ArtifactRepository,
    BeginBlobStage, BlobStore, CursorKey, DefinitionIdentity, DefinitionRepository,
    DefinitionValue, ExperimentRepository, IdGenerator, IdempotencyKey, InstrumentDefinition,
    InstrumentSubtype, IntegrityEvent, IntegrityEventSink, MarketFact, MarketFactRepository,
    MarketFactRulePackResolver, MarketFactUnitResolver, MarketFactWindow, PageRequest,
    Phase1RunCandidateResolver, PublishCurveSnapshot, SafeTraceContext, SignalRepository,
    SnapshotBlobRole, SnapshotRepository, SnapshotValue, StagedSnapshotBlob, StagedSnapshotProof,
    VerifyBlobStage,
};
use ficant_application::{
    Phase1BusinessInput, Phase1BusinessLoop, StagedArtifact, StagedSnapshot, VerifiedReadFacade,
    VerifiedSnapshotRead,
};
use ficant_domain::market::{
    ArtifactInputKind, Bond, Calendar, CalendarInput, CalendarSession, Cashflow, CashflowInput,
    CashflowType, CurveSnapshot, CurveSnapshotInput, FactSource, Instrument, InstrumentInput,
    InstrumentKind, MarketRulePack, MarketRulePackInput, Quote, QuoteInput, Trade, TradeInput,
    Unit, UnitInput, Valuation, ValuationInput, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{
    Artifact, ArtifactKind, DataSnapshot, DataSnapshotInput, ExperimentRun, ExperimentRunInput,
    RunState, SignalSet, SignalSetInput, UniverseSnapshot,
};
use ficant_storage::postgres::PostgresRepository;
use ficant_storage::s3::S3BlobStore;
use serde_json::Value;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn phase1_business_loop_persists_restart_safe_complete_lineage() {
    let fixture = Fixture::load();
    assert_eq!(
        fixture.string(&["schema"]),
        "ficant.fixture.china-rates.phase1.v1"
    );
    assert_eq!(fixture.positive_ids().len(), 24);
    assert_eq!(fixture.array(&["negative_cases"]).len(), 12);

    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let repository = postgres_repository(pool.clone());
    let store = blob_store(pool.clone());
    let owner = fixture.owner();
    let scope = fixture.scope();
    let prefix = fixture.string(&["idempotency_prefix"]);
    let version = Version::new(1).unwrap();

    for unit in fixture.array(&["units"]) {
        let key = unit["key"].as_str().unwrap();
        let value = DefinitionValue::Unit(
            Unit::new(UnitInput {
                unit_id: fixture.id(&["ids", "units", key]),
                version,
                owner: owner.clone(),
                code: unit["code"].as_str().unwrap().to_owned(),
                dimension: unit["dimension"].as_str().unwrap().to_owned(),
                scale: u32::try_from(unit["scale"].as_u64().unwrap()).unwrap(),
                precision: u32::try_from(unit["precision"].as_u64().unwrap()).unwrap(),
            })
            .unwrap(),
        );
        publish_definition(&repository, &owner, prefix, value).await;
    }

    let cny = UnitRef::new(fixture.id(&["ids", "units", "cny"]), version);
    let price = UnitRef::new(fixture.id(&["ids", "units", "price"]), version);
    let face = UnitRef::new(fixture.id(&["ids", "units", "face"]), version);
    let calendar_ref = VersionRef::new(fixture.id(&["ids", "calendar"]), version);
    let calendar_period = EffectivePeriod::new(
        fixture.market_time(&["calendar", "effective_from"]),
        fixture.market_time(&["calendar", "effective_to"]),
    )
    .unwrap();
    let calendar = DefinitionValue::Calendar(
        Calendar::new(CalendarInput {
            calendar_id: calendar_ref.id().clone(),
            version,
            owner: owner.clone(),
            market: fixture.string(&["market", "code"]).to_owned(),
            market_timezone: fixture.string(&["market", "timezone"]).to_owned(),
            effective: calendar_period.clone(),
            sessions: vec![
                CalendarSession::open(
                    fixture.string(&["market", "trading_date"]).parse().unwrap(),
                    fixture
                        .string(&["calendar", "session_open"])
                        .parse()
                        .unwrap(),
                    fixture
                        .string(&["calendar", "session_close"])
                        .parse()
                        .unwrap(),
                )
                .unwrap(),
            ],
        })
        .unwrap(),
    );
    publish_definition(&repository, &owner, prefix, calendar).await;

    let rule_ref = VersionRef::new(fixture.id(&["ids", "rule_pack"]), version);
    let rule_hash = ContentHash::digest(fixture.string(&["rule_pack", "content"]).as_bytes());
    let rule_pack = DefinitionValue::MarketRulePack(
        MarketRulePack::new(MarketRulePackInput {
            rule_pack_id: rule_ref.id().clone(),
            version,
            owner: owner.clone(),
            market: fixture.string(&["market", "code"]).to_owned(),
            rule_type: fixture.string(&["rule_pack", "rule_type"]).to_owned(),
            source: fixture.string(&["rule_pack", "source"]).to_owned(),
            effective: calendar_period,
            verification_status: VerificationStatus::Verified,
            content_hash: rule_hash,
        })
        .unwrap(),
    );
    publish_definition(&repository, &owner, prefix, rule_pack).await;

    let instrument_ref = VersionRef::new(fixture.id(&["ids", "instrument"]), version);
    let instrument = Instrument::new(InstrumentInput {
        instrument_id: instrument_ref.id().clone(),
        version,
        owner: owner.clone(),
        kind: InstrumentKind::Bond,
        market: fixture.string(&["market", "code"]).to_owned(),
        symbol: fixture.string(&["instrument", "symbol"]).to_owned(),
        currency: cny.clone(),
        calendar: calendar_ref.clone(),
    })
    .unwrap();
    let bond = Bond::new(
        &instrument,
        fixture
            .string(&["instrument", "issue_date"])
            .parse()
            .unwrap(),
        fixture
            .string(&["instrument", "maturity_date"])
            .parse()
            .unwrap(),
        DecimalValue::new(
            fixture.string(&["instrument", "face_coefficient"]),
            fixture.u32(&["instrument", "face_scale"]),
            cny.clone(),
        )
        .unwrap(),
    )
    .unwrap();
    publish_definition(
        &repository,
        &owner,
        prefix,
        DefinitionValue::Instrument(
            InstrumentDefinition::new(instrument, Some(InstrumentSubtype::Bond(bond))).unwrap(),
        ),
    )
    .await;

    let source = fixture.string(&["facts", "source_id"]);
    let cashflow = MarketFact::Cashflow(
        Cashflow::new(CashflowInput {
            cashflow_id: fixture.id(&["ids", "cashflow"]),
            bond: instrument_ref.clone(),
            payment_time: fixture.market_time(&["facts", "cashflow", "payment_time"]),
            amount: DecimalValue::new(
                fixture.string(&["facts", "cashflow", "coefficient"]),
                fixture.u32(&["facts", "cashflow", "scale"]),
                cny,
            )
            .unwrap(),
            owner: owner.clone(),
            source: FactSource::new(
                source,
                fixture.string(&["facts", "cashflow", "external_id"]),
                1,
            )
            .unwrap(),
            supersedes_id: None,
            cashflow_type: CashflowType::Coupon,
            schedule_id: fixture
                .string(&["facts", "cashflow", "schedule_id"])
                .to_owned(),
            sequence: fixture.u64(&["facts", "cashflow", "sequence"]),
        })
        .unwrap(),
    );
    let quote = MarketFact::Quote(
        Quote::new(QuoteInput {
            quote_id: fixture.id(&["ids", "quote"]),
            instrument: instrument_ref.clone(),
            owner: owner.clone(),
            source: FactSource::new(
                source,
                fixture.string(&["facts", "quote", "external_id"]),
                1,
            )
            .unwrap(),
            observed_at: fixture.market_time(&["facts", "quote", "observed_at"]),
            received_at: fixture.market_time(&["facts", "quote", "received_at"]),
            bid: Some(
                DecimalValue::new(
                    fixture.string(&["facts", "quote", "bid"]),
                    fixture.u32(&["facts", "quote", "scale"]),
                    price.clone(),
                )
                .unwrap(),
            ),
            ask: Some(
                DecimalValue::new(
                    fixture.string(&["facts", "quote", "ask"]),
                    fixture.u32(&["facts", "quote", "scale"]),
                    price.clone(),
                )
                .unwrap(),
            ),
            supersedes_id: None,
        })
        .unwrap(),
    );
    let trade = MarketFact::Trade(
        Trade::new(TradeInput {
            trade_id: fixture.id(&["ids", "trade"]),
            instrument: instrument_ref.clone(),
            owner: owner.clone(),
            source: FactSource::new(
                source,
                fixture.string(&["facts", "trade", "external_id"]),
                1,
            )
            .unwrap(),
            executed_at: fixture.market_time(&["facts", "trade", "executed_at"]),
            price: DecimalValue::new(
                fixture.string(&["facts", "trade", "price"]),
                fixture.u32(&["facts", "trade", "price_scale"]),
                price.clone(),
            )
            .unwrap(),
            quantity: DecimalValue::new(
                fixture.string(&["facts", "trade", "quantity"]),
                fixture.u32(&["facts", "trade", "quantity_scale"]),
                face,
            )
            .unwrap(),
            supersedes_id: None,
        })
        .unwrap(),
    );
    let valuation = MarketFact::Valuation(
        Valuation::new(ValuationInput {
            valuation_id: fixture.id(&["ids", "valuation"]),
            instrument: instrument_ref.clone(),
            owner: owner.clone(),
            source: FactSource::new(
                source,
                fixture.string(&["facts", "valuation", "external_id"]),
                1,
            )
            .unwrap(),
            valuation_at: fixture.market_time(&["facts", "valuation", "valuation_at"]),
            method: fixture.string(&["facts", "valuation", "method"]).to_owned(),
            rule_pack: rule_ref.clone(),
            values: vec![
                DecimalValue::new(
                    fixture.string(&["facts", "valuation", "value"]),
                    fixture.u32(&["facts", "valuation", "scale"]),
                    price,
                )
                .unwrap(),
            ],
            supersedes_id: None,
        })
        .unwrap(),
    );
    for fact in [&cashflow, &quote, &trade] {
        let validated = MarketFactUnitResolver::new(&repository)
            .resolve(&scope, fact.clone())
            .await
            .unwrap();
        let validated = MarketFactRulePackResolver::new(&repository)
            .resolve(&scope, validated)
            .await
            .unwrap();
        let command =
            AppendMarketFact::new(validated, key(prefix, "fact", fact.id().as_str())).unwrap();
        assert_eq!(repository.append_fact(command).await.unwrap(), *fact);
    }

    let curve_bytes = serde_json::to_vec(fixture.value(&["curve", "points"])).unwrap();
    let curve_hash = ContentHash::digest(&curve_bytes);
    let curve = CurveSnapshot::new(CurveSnapshotInput {
        curve_snapshot_id: fixture.id(&["ids", "curve"]),
        owner: owner.clone(),
        as_of: fixture.market_time(&["curve", "as_of"]),
        currency: UnitRef::new(fixture.id(&["ids", "units", "rate"]), version),
        curve_kind: fixture.string(&["curve", "kind"]).to_owned(),
        calendar: calendar_ref.clone(),
        rule_pack: rule_ref.clone(),
        point_schema: fixture.string(&["curve", "point_schema"]).to_owned(),
        content_hash: curve_hash.clone(),
        lineage: vec![quote.lineage_ref().unwrap()],
        input_kind: ArtifactInputKind::ExternalFixture,
    })
    .unwrap();

    let data_bytes = fixture
        .string(&["snapshots", "data_payload"])
        .as_bytes()
        .to_vec();
    let manifest_bytes = fixture
        .string(&["snapshots", "data_manifest"])
        .as_bytes()
        .to_vec();
    let universe_bytes = fixture
        .string(&["snapshots", "universe_manifest"])
        .as_bytes()
        .to_vec();
    let artifact_bytes = fixture.string(&["artifact", "payload"]).as_bytes().to_vec();
    let data_hash = ContentHash::digest(&data_bytes);
    let manifest_hash = ContentHash::digest(&manifest_bytes);
    let universe_hash = ContentHash::digest(&universe_bytes);
    let artifact_hash = ContentHash::digest(&artifact_bytes);

    let mut data_lineage = vec![
        cashflow.lineage_ref().unwrap(),
        quote.lineage_ref().unwrap(),
        trade.lineage_ref().unwrap(),
        valuation.lineage_ref().unwrap(),
        LineageRef::content_addressed(curve.id().clone(), curve_hash.clone()),
        LineageRef::versioned(calendar_ref.id().clone(), calendar_ref.version()),
        LineageRef::versioned(rule_ref.id().clone(), rule_ref.version()),
    ];
    data_lineage.sort_by(|left, right| left.object_id().cmp(right.object_id()));
    let data_snapshot = SnapshotValue::Data(
        DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: fixture.id(&["ids", "data_snapshot"]),
            owner: owner.clone(),
            visible_at: fixture.market_time(&["snapshots", "data_visible_at"]),
            as_of: fixture.market_time(&["snapshots", "data_as_of"]),
            schema_hash: ContentHash::digest(
                fixture.string(&["snapshots", "data_schema"]).as_bytes(),
            ),
            manifest_hash: manifest_hash.clone(),
            blob_content_hash: data_hash.clone(),
            lineage: data_lineage,
        })
        .unwrap(),
    );
    let universe_snapshot = SnapshotValue::Universe(
        UniverseSnapshot::new(
            fixture.id(&["ids", "universe_snapshot"]),
            owner.clone(),
            vec![instrument_ref.clone()],
            ContentHash::digest(fixture.string(&["snapshots", "universe_filter"]).as_bytes()),
            universe_hash.clone(),
            vec![LineageRef::versioned(
                instrument_ref.id().clone(),
                instrument_ref.version(),
            )],
        )
        .unwrap(),
    );
    let data_ref =
        LineageRef::content_addressed(fixture.id(&["ids", "data_snapshot"]), data_hash.clone());
    let universe_ref = LineageRef::content_addressed(
        fixture.id(&["ids", "universe_snapshot"]),
        universe_hash.clone(),
    );
    let run = ExperimentRun::new(ExperimentRunInput {
        experiment_run_id: fixture.id(&["ids", "run"]),
        owner: owner.clone(),
        data_snapshot: data_ref.clone(),
        universe_snapshot: universe_ref.clone(),
        rule_packs: vec![rule_ref.clone()],
        runtime_image_digest: runtime_image_digest(),
        parameters_hash: ContentHash::digest(fixture.string(&["run", "parameters"]).as_bytes()),
        seed: fixture.u64(&["run", "seed"]),
    })
    .unwrap();
    let SnapshotValue::Data(data_snapshot_value) = &data_snapshot else {
        unreachable!()
    };
    let run = Phase1RunCandidateResolver::new(&repository)
        .resolve(&scope, run, data_snapshot_value)
        .await
        .unwrap();
    let curve_verification = stage(&store, &scope, &owner, prefix, "curve", &curve_bytes).await;
    let curve_verified = store.verify_and_promote(curve_verification).await.unwrap();
    repository
        .publish_curve_snapshot(
            PublishCurveSnapshot::new(
                scope.clone(),
                curve.clone(),
                u64::try_from(curve_bytes.len()).unwrap(),
                curve_verified,
                key(prefix, "curve", curve.id().as_str()),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let curve_ref = LineageRef::content_addressed(curve.id().clone(), curve_hash.clone());
    let artifact_lineage = vec![
        data_ref.clone(),
        universe_ref.clone(),
        LineageRef::versioned(rule_ref.id().clone(), rule_ref.version()),
        curve_ref.clone(),
    ];
    let artifact = Artifact::new(
        fixture.id(&["ids", "artifact"]),
        owner.clone(),
        ArtifactKind::SignalSet,
        fixture.string(&["artifact", "media_type"]),
        artifact_hash.clone(),
        u64::try_from(artifact_bytes.len()).unwrap(),
        artifact_lineage,
    )
    .unwrap();
    let signal = SignalSet::new(SignalSetInput {
        signal_set_id: fixture.id(&["ids", "signal"]),
        owner: owner.clone(),
        artifact: LineageRef::content_addressed(artifact.id().clone(), artifact_hash.clone()),
        experiment_run_id: run.run().id().clone(),
        data_snapshot: data_ref,
        universe_snapshot: universe_ref,
        rule_packs: vec![rule_ref],
        input_artifacts: vec![curve_ref],
        valid: EffectivePeriod::new(
            fixture.market_time(&["signal", "valid_from"]),
            fixture.market_time(&["signal", "valid_to"]),
        )
        .unwrap(),
    })
    .unwrap();

    let input = Phase1BusinessInput::new(
        scope.clone(),
        MarketFactRulePackResolver::new(&repository)
            .resolve(
                &scope,
                MarketFactUnitResolver::new(&repository)
                    .resolve(&scope, valuation)
                    .await
                    .unwrap(),
            )
            .await
            .unwrap(),
        StagedSnapshot::from_proof(
            data_snapshot.clone(),
            StagedSnapshotProof::data(
                StagedSnapshotBlob::new(
                    SnapshotBlobRole::DataParquet,
                    stage(&store, &scope, &owner, prefix, "data", &data_bytes).await,
                ),
                StagedSnapshotBlob::new(
                    SnapshotBlobRole::DataManifest,
                    stage(
                        &store,
                        &scope,
                        &owner,
                        prefix,
                        "data-manifest",
                        &manifest_bytes,
                    )
                    .await,
                ),
            )
            .unwrap(),
        )
        .unwrap(),
        StagedSnapshot::from_proof(
            universe_snapshot.clone(),
            StagedSnapshotProof::universe(StagedSnapshotBlob::new(
                SnapshotBlobRole::UniverseMembersManifest,
                stage(&store, &scope, &owner, prefix, "universe", &universe_bytes).await,
            ))
            .unwrap(),
        )
        .unwrap(),
        run.clone(),
        StagedArtifact::new(
            artifact.clone(),
            signal.clone(),
            stage(&store, &scope, &owner, prefix, "artifact", &artifact_bytes).await,
        )
        .unwrap(),
        key(prefix, "phase1", run.run().id().as_str()),
    )
    .unwrap();
    let clock = FixedClock(fixture.market_time(&["run", "journal_occurred_at"]));
    let ids = FixedIds(Mutex::new(
        fixture
            .array(&["ids", "journal"])
            .iter()
            .map(|value| Ulid::new(value.as_str().unwrap()).unwrap())
            .collect(),
    ));
    let business_loop = Phase1BusinessLoop::new(&clock, &ids, &store, &repository);
    let result = business_loop.execute(input).await.unwrap();
    assert_eq!(result.run_id(), run.run().id());
    assert_eq!(result.terminal_state(), RunState::Succeeded);

    let persisted_run = repository
        .get_run(&scope, run.run().id().clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (persisted_run.state(), persisted_run.revision()),
        (RunState::Succeeded, 3)
    );
    assert_eq!(
        repository
            .get_by_id(&scope, fixture.id(&["ids", "data_snapshot"]))
            .await
            .unwrap(),
        Some(data_snapshot.clone())
    );
    assert_eq!(
        repository
            .get_by_id(&scope, fixture.id(&["ids", "universe_snapshot"]))
            .await
            .unwrap(),
        Some(universe_snapshot.clone())
    );
    assert_eq!(
        repository
            .get_metadata(&scope, artifact.id().clone())
            .await
            .unwrap(),
        Some(artifact.clone())
    );
    assert_eq!(
        repository.get(&scope, signal.id().clone()).await.unwrap(),
        Some(signal.clone())
    );
    let facts = repository
        .query_instrument_window(
            &scope,
            MarketFactWindow::new(
                instrument_ref,
                fixture.market_time_literal("2025-01-15T00:00:00Z"),
                fixture.market_time_literal("2025-06-16T00:00:00Z"),
                fixture.market_time_literal("2025-06-16T00:00:00Z"),
                PageRequest::new(scope.clone(), None, 10).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(facts.items().len(), 4);
    assert!(facts.next_cursor().is_none());

    let replay_one = business_loop
        .replay_run(&repository, &scope, run.run().id().clone(), 2)
        .await
        .unwrap();
    let replay_two = business_loop
        .replay_run(&repository, &scope, run.run().id().clone(), 3)
        .await
        .unwrap();
    assert_eq!(replay_one, replay_two);
    assert_eq!(replay_one.event_count(), 5);

    pool.close().await;
    let restarted_pool = connect_postgres().await;
    let restarted_repository = postgres_repository(restarted_pool.clone());
    let restarted_store = blob_store(restarted_pool.clone());
    assert_eq!(
        restarted_repository
            .get_run(&scope, run.run().id().clone())
            .await
            .unwrap(),
        Some(persisted_run)
    );
    assert_eq!(
        restarted_repository
            .get_metadata(&scope, artifact.id().clone())
            .await
            .unwrap(),
        Some(artifact.clone())
    );
    let integrity_events = RecordingSink::default();
    let verified_reads = VerifiedReadFacade::new(
        &restarted_repository,
        &restarted_repository,
        &restarted_repository,
        &restarted_store,
        &integrity_events,
    );
    let read_trace = SafeTraceContext::new("0123456789abcdef0123456789abcdef").unwrap();
    let artifact_read = verified_reads
        .read_verified_artifact(&scope, artifact.id().clone(), read_trace.clone())
        .await
        .unwrap();
    assert_eq!(artifact_read.payload().bytes(), artifact_bytes);
    let signal_read = verified_reads
        .read_verified_signal(&scope, signal.id().clone(), read_trace.clone())
        .await
        .unwrap();
    assert_eq!(signal_read.payload().bytes(), artifact_bytes);
    match verified_reads
        .read_verified_snapshot(&scope, data_snapshot.id().clone(), read_trace.clone())
        .await
        .unwrap()
    {
        VerifiedSnapshotRead::Data {
            parquet, manifest, ..
        } => {
            assert_eq!(parquet.bytes(), data_bytes);
            assert_eq!(manifest.bytes(), manifest_bytes);
        }
        VerifiedSnapshotRead::Universe { .. } => panic!("expected DataSnapshot required read"),
    }
    match verified_reads
        .read_verified_snapshot(&scope, universe_snapshot.id().clone(), read_trace)
        .await
        .unwrap()
    {
        VerifiedSnapshotRead::Universe {
            members_manifest, ..
        } => assert_eq!(members_manifest.bytes(), universe_bytes),
        VerifiedSnapshotRead::Data { .. } => panic!("expected UniverseSnapshot required read"),
    }
    assert!(integrity_events.events.lock().unwrap().is_empty());

    assert_logical_counts(&restarted_pool).await;
    let blob_state: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM storage.blobs),
             (SELECT COUNT(*) FROM storage.orphan_candidates),
             (SELECT COUNT(*) FROM storage.staging_uploads)",
    )
    .fetch_one(&restarted_pool)
    .await
    .unwrap();
    assert_eq!(
        blob_state,
        (5, 0, 0),
        "DataSnapshot data and manifest must both be durable references; no promoted object may remain orphaned"
    );
}

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<IntegrityEvent>>,
}

#[async_trait]
impl IntegrityEventSink for RecordingSink {
    async fn emit(
        &self,
        event: IntegrityEvent,
    ) -> Result<(), ficant_application::ApplicationError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

struct Fixture {
    value: Value,
}

impl Fixture {
    fn load() -> Self {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let path = env::var_os("FICANT_ACCEPTANCE_FIXTURE").map_or_else(
            || manifest_dir.join("../../tests/golden-cases/china-rates/phase1-business-loop.json"),
            |configured| {
                let configured = PathBuf::from(configured);
                if configured.is_absolute() {
                    configured
                } else {
                    manifest_dir
                        .parent()
                        .and_then(|crates_dir| crates_dir.parent())
                        .expect(
                            "the acceptance crate must be inside the repository crates directory",
                        )
                        .join(configured)
                }
            },
        );
        let bytes = std::fs::read(path).expect("the single Phase 1 fixture must be readable");
        Self {
            value: serde_json::from_slice(&bytes).expect("the Phase 1 fixture must be valid JSON"),
        }
    }

    fn value(&self, path: &[&str]) -> &Value {
        path.iter().fold(&self.value, |value, key| {
            value
                .get(*key)
                .unwrap_or_else(|| panic!("missing fixture path {path:?}"))
        })
    }

    fn string(&self, path: &[&str]) -> &str {
        self.value(path)
            .as_str()
            .unwrap_or_else(|| panic!("fixture path is not text: {path:?}"))
    }

    fn u64(&self, path: &[&str]) -> u64 {
        self.value(path)
            .as_u64()
            .unwrap_or_else(|| panic!("fixture path is not u64: {path:?}"))
    }

    fn u32(&self, path: &[&str]) -> u32 {
        u32::try_from(self.u64(path)).expect("fixture u32 must fit")
    }

    fn array(&self, path: &[&str]) -> &[Value] {
        self.value(path)
            .as_array()
            .unwrap_or_else(|| panic!("fixture path is not an array: {path:?}"))
    }

    fn id(&self, path: &[&str]) -> Ulid {
        Ulid::new(self.string(path)).expect("fixture IDs must be canonical ULIDs")
    }

    fn owner(&self) -> OwnerRef {
        OwnerRef::new(
            self.id(&["scope", "tenant_id"]),
            self.id(&["scope", "owner_id"]),
        )
    }

    fn scope(&self) -> AccessScope {
        let owner = self.owner();
        AccessScope::new(
            owner.tenant_id().clone(),
            owner.owner_id().clone(),
            vec![owner.owner_id().clone()],
        )
        .unwrap()
    }

    fn market_time(&self, path: &[&str]) -> MarketTime {
        self.market_time_literal(self.string(path))
    }

    fn market_time_literal(&self, instant: &str) -> MarketTime {
        let local_date = instant.get(..10).expect("fixture UTC instant has a date");
        MarketTime::new(
            instant.parse().expect("fixture UTC instant must parse"),
            self.string(&["market", "timezone"]),
            local_date.parse().expect("fixture local date must parse"),
        )
        .unwrap()
    }

    fn positive_ids(&self) -> Vec<Ulid> {
        let mut result = vec![
            self.id(&["scope", "tenant_id"]),
            self.id(&["scope", "owner_id"]),
        ];
        for key in ["cny", "price", "face", "rate"] {
            result.push(self.id(&["ids", "units", key]));
        }
        for key in [
            "calendar",
            "rule_pack",
            "instrument",
            "cashflow",
            "quote",
            "trade",
            "valuation",
            "curve",
            "data_snapshot",
            "universe_snapshot",
            "run",
            "artifact",
            "signal",
        ] {
            result.push(self.id(&["ids", key]));
        }
        result.extend(
            self.array(&["ids", "journal"])
                .iter()
                .map(|value| Ulid::new(value.as_str().unwrap()).unwrap()),
        );
        result
    }
}

struct FixedClock(MarketTime);

impl ficant_application::ports::Clock for FixedClock {
    fn now(&self) -> ficant_application::ports::ApplicationResult<MarketTime> {
        Ok(self.0.clone())
    }
}

struct FixedIds(Mutex<VecDeque<Ulid>>);

impl IdGenerator for FixedIds {
    fn next_id(&self) -> ficant_application::ports::ApplicationResult<Ulid> {
        self.0.lock().unwrap().pop_front().ok_or_else(|| {
            ficant_application::ApplicationError::new(
                ficant_application::ApplicationErrorCategory::StorageUnavailable,
                false,
            )
        })
    }
}

async fn connect_postgres() -> PgPool {
    PgPoolOptions::new()
        .max_connections(8)
        .connect(
            &env::var("FICANT_TEST_DATABASE_URL")
                .expect("FICANT_TEST_DATABASE_URL must name the ready PostgreSQL test database"),
        )
        .await
        .expect("the real PostgreSQL test database must be reachable")
}

async fn reset_and_migrate(pool: &PgPool) {
    sqlx::raw_sql(
        "DROP SCHEMA IF EXISTS portfolio CASCADE;
         DROP SCHEMA IF EXISTS analytics CASCADE;
         DROP SCHEMA IF EXISTS data CASCADE;
         DROP SCHEMA IF EXISTS storage CASCADE;
         DROP SCHEMA IF EXISTS research CASCADE;
         DROP SCHEMA IF EXISTS market CASCADE;
         DROP SCHEMA IF EXISTS core CASCADE;
         DROP TABLE IF EXISTS public._sqlx_migrations;",
    )
    .execute(pool)
    .await
    .expect("test database reset must succeed");
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations/postgresql");
    sqlx::migrate::Migrator::new(path)
        .await
        .expect("migration directory must be readable")
        .run(pool)
        .await
        .expect("empty database migrations must apply");
}

fn postgres_repository(pool: PgPool) -> PostgresRepository {
    let cursor =
        AeadCursorCodec::new(CursorKey::new("acceptance", [41_u8; 32]).unwrap(), vec![]).unwrap();
    PostgresRepository::new(pool, Arc::new(cursor))
}

fn blob_store(pool: PgPool) -> S3BlobStore {
    S3BlobStore::new(
        &env::var("FICANT_TEST_S3_ENDPOINT").expect("real S3 endpoint must be configured"),
        env::var("FICANT_TEST_S3_BUCKET").expect("isolated S3 bucket must be configured"),
        &env::var("FICANT_TEST_S3_ACCESS_KEY").expect("S3 access key must be configured"),
        &env::var("FICANT_TEST_S3_SECRET_KEY").expect("S3 secret key must be configured"),
        pool,
    )
    .unwrap()
}

async fn publish_definition(
    repository: &PostgresRepository,
    owner: &OwnerRef,
    prefix: &str,
    value: DefinitionValue,
) {
    let identity = value.identity().to_owned();
    repository
        .create_identity(DefinitionIdentity::new(
            Ulid::new(&identity).unwrap(),
            owner.clone(),
            value.kind(),
            key(prefix, "definition-identity", &identity),
        ))
        .await
        .unwrap();
    let persisted = repository
        .append_version(
            AppendDefinitionVersion::new(
                None,
                value.clone(),
                key(prefix, "definition-version", &identity),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(persisted, value);
}

async fn stage(
    store: &S3BlobStore,
    scope: &AccessScope,
    owner: &OwnerRef,
    prefix: &str,
    label: &str,
    bytes: &[u8],
) -> VerifyBlobStage {
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner.clone(),
                u64::try_from(bytes.len()).unwrap(),
                key(prefix, "blob-stage", label),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(scope, &staged, bytes.to_vec())
        .await
        .unwrap();
    VerifyBlobStage::new(
        scope.clone(),
        staged,
        ContentHash::digest(bytes),
        u64::try_from(bytes.len()).unwrap(),
    )
    .unwrap()
}

fn key(prefix: &str, aggregate: &str, semantic_id: &str) -> IdempotencyKey {
    IdempotencyKey::new(format!("{prefix}:{aggregate}:{semantic_id}:v1")).unwrap()
}

fn runtime_image_digest() -> ContentHash {
    let value = env::var("FICANT_TEST_RUNTIME_IMAGE_DIGEST")
        .expect("Delivery must inject the current runtime image digest");
    let value = value.strip_prefix("sha256:").unwrap_or(&value);
    assert_eq!(value.len(), 64, "runtime image digest must be SHA-256 hex");
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16)
                .expect("runtime image digest must be hex")
        })
        .collect::<Vec<_>>();
    ContentHash::from_bytes(&bytes).unwrap()
}

async fn assert_logical_counts(pool: &PgPool) {
    for (table, expected) in [
        ("market.units", 4_i64),
        ("market.calendars", 1),
        ("market.market_rule_packs", 1),
        ("market.instruments", 1),
        ("market.bonds", 1),
        ("market.cashflows", 1),
        ("market.quotes", 1),
        ("market.trades", 1),
        ("market.valuations", 1),
        ("market.curve_snapshots", 1),
        ("research.data_snapshots", 1),
        ("research.universe_snapshots", 1),
        ("research.experiment_runs", 1),
        ("research.run_journal", 5),
        ("research.artifacts", 1),
        ("research.signal_sets", 1),
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(count, expected, "unexpected logical row count for {table}");
    }
}
