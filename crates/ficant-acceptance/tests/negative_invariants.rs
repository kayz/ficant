use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use ficant_application::ports::{
    AccessScope, AeadCursorCodec, AppendDefinitionVersion, AppendMarketFact, BeginBlobStage,
    BlobStore, CursorKey, DefinitionIdentity, DefinitionRepository, DefinitionValue,
    IdempotencyKey, InstrumentDefinition, IntegrityEvent, IntegrityEventSink,
    IntegrityFailureReason, MarketFact, MarketFactFieldRole, MarketFactRepository,
    MarketFactRulePackResolver, MarketFactUnitResolver, Phase1RunCandidateResolver,
    PublishArtifact, PublishSnapshot, RequiredVerifiedBlobRead, SafeTraceContext, SnapshotBlobRole,
    SnapshotRepository, SnapshotValue, StagedSnapshotBlob, VerifiedBlobReader, VerifiedBlobRole,
    VerifiedReadResourceKind, VerifiedSnapshotBlob, VerifiedSnapshotProof, VerifyBlobStage,
};
use ficant_application::{ApplicationErrorCategory, VerifiedReadFacade, map_domain_error};
use ficant_domain::ContentAddressed;
use ficant_domain::market::{
    Calendar, CalendarInput, FactSource, Instrument, InstrumentInput, InstrumentKind,
    MarketRulePack, MarketRulePackInput, Quote, QuoteInput, Trade, TradeInput, Unit, UnitInput,
    Valuation, ValuationInput, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{
    Artifact, ArtifactKind, DataSnapshot, DataSnapshotInput, ExperimentRun, ExperimentRunInput,
    JournalEventType, RunJournalInput, SignalSet, SignalSetInput,
};
use ficant_storage::postgres::PostgresRepository;
use ficant_storage::s3::S3BlobStore;
use object_store::ObjectStoreExt;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as ObjectPath;
use serde_json::Value;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn q2_inv_01_rejects_semantically_wrong_units_without_side_effects() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let owner = OwnerRef::new(
        id(&fixture, &["scope", "tenant_id"]),
        id(&fixture, &["scope", "owner_id"]),
    );
    let repository = PostgresRepository::new(
        pool.clone(),
        Arc::new(
            AeadCursorCodec::new(
                CursorKey::new("acceptance-negative", [43_u8; 32]).unwrap(),
                vec![],
            )
            .unwrap(),
        ),
    );
    let _store = S3BlobStore::new(
        &env::var("FICANT_TEST_S3_ENDPOINT").unwrap(),
        env::var("FICANT_TEST_S3_BUCKET").unwrap(),
        &env::var("FICANT_TEST_S3_ACCESS_KEY").unwrap(),
        &env::var("FICANT_TEST_S3_SECRET_KEY").unwrap(),
        pool.clone(),
    )
    .unwrap();
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        owner.owner_id().clone(),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    publish_unit(
        &repository,
        &owner,
        id(&fixture, &["ids", "units", "price"]),
        "CNY_PER_100_FACE",
        "price",
        4,
    )
    .await;
    publish_unit(
        &repository,
        &owner,
        id(&fixture, &["ids", "units", "rate"]),
        "DECIMAL_RATE",
        "rate",
        8,
    )
    .await;
    let baseline = side_effect_counts(&pool).await;

    let rate = UnitRef::new(
        id(&fixture, &["ids", "units", "rate"]),
        Version::new(1).unwrap(),
    );
    let quote = Quote::new(QuoteInput {
        quote_id: id(&fixture, &["ids", "quote"]),
        instrument: VersionRef::new(
            id(&fixture, &["ids", "instrument"]),
            Version::new(1).unwrap(),
        ),
        owner: owner.clone(),
        source: FactSource::new("china-rates-fixture-v1", "QUOTE-WRONG-UNIT", 1).unwrap(),
        observed_at: market_time("2025-01-15T01:01:00Z"),
        received_at: market_time("2025-01-15T01:01:01Z"),
        bid: Some(DecimalValue::new("1012345", 4, rate).unwrap()),
        ask: None,
        supersedes_id: None,
    })
    .expect("Domain construction intentionally does not resolve persisted Unit semantics");
    let error = MarketFactUnitResolver::new(&repository)
        .resolve(&scope, MarketFact::Quote(quote))
        .await
        .expect_err("DECIMAL_RATE cannot resolve as a Quote price unit");
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert!(!error.retryable());
    assert_eq!(side_effect_counts(&pool).await, baseline);

    let price = UnitRef::new(
        id(&fixture, &["ids", "units", "price"]),
        Version::new(1).unwrap(),
    );
    let trade = Trade::new(TradeInput {
        trade_id: id(&fixture, &["ids", "trade"]),
        instrument: VersionRef::new(
            id(&fixture, &["ids", "instrument"]),
            Version::new(1).unwrap(),
        ),
        owner,
        source: FactSource::new("china-rates-fixture-v1", "TRADE-WRONG-UNIT", 1).unwrap(),
        executed_at: market_time("2025-01-15T01:02:00Z"),
        price: DecimalValue::new("1012400", 4, price.clone()).unwrap(),
        quantity: DecimalValue::new("100000000", 2, price).unwrap(),
        supersedes_id: None,
    })
    .unwrap();
    let error = MarketFactUnitResolver::new(&repository)
        .resolve(&scope, MarketFact::Trade(trade))
        .await
        .expect_err("a Price Unit cannot resolve as Trade notional");
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert!(!error.retryable());
    assert_eq!(side_effect_counts(&pool).await, baseline);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn q2_inv_01_accepts_double_sided_quote_with_explicit_price_bindings() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let owner = OwnerRef::new(
        id(&fixture, &["scope", "tenant_id"]),
        id(&fixture, &["scope", "owner_id"]),
    );
    let repository = PostgresRepository::new(
        pool.clone(),
        Arc::new(
            AeadCursorCodec::new(
                CursorKey::new("acceptance-negative-legal", [44_u8; 32]).unwrap(),
                vec![],
            )
            .unwrap(),
        ),
    );
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        owner.owner_id().clone(),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    let price = UnitRef::new(
        id(&fixture, &["ids", "units", "price"]),
        Version::new(1).unwrap(),
    );
    publish_unit(
        &repository,
        &owner,
        price.unit_id().clone(),
        "CNY_PER_100_FACE",
        "price",
        4,
    )
    .await;
    let currency = UnitRef::new(
        id(&fixture, &["ids", "units", "cny"]),
        Version::new(1).unwrap(),
    );
    publish_unit(
        &repository,
        &owner,
        currency.unit_id().clone(),
        "CNY",
        "currency",
        2,
    )
    .await;
    let calendar = VersionRef::new(id(&fixture, &["ids", "calendar"]), Version::new(1).unwrap());
    publish_definition(
        &repository,
        &owner,
        DefinitionValue::Calendar(
            Calendar::new(CalendarInput {
                calendar_id: calendar.id().clone(),
                version: calendar.version(),
                owner: owner.clone(),
                market: "CIBM".to_owned(),
                market_timezone: "Asia/Shanghai".to_owned(),
                effective: EffectivePeriod::new(
                    market_time("2025-01-01T00:00:00Z"),
                    market_time("2025-12-31T15:59:59Z"),
                )
                .unwrap(),
                sessions: vec![],
            })
            .unwrap(),
        ),
    )
    .await;
    let instrument = VersionRef::new(
        id(&fixture, &["ids", "instrument"]),
        Version::new(1).unwrap(),
    );
    publish_definition(
        &repository,
        &owner,
        DefinitionValue::Instrument(
            InstrumentDefinition::new(
                Instrument::new(InstrumentInput {
                    instrument_id: instrument.id().clone(),
                    version: instrument.version(),
                    owner: owner.clone(),
                    kind: InstrumentKind::Other,
                    market: "CIBM".to_owned(),
                    symbol: "2400001.IB".to_owned(),
                    currency,
                    calendar,
                })
                .unwrap(),
                None,
            )
            .unwrap(),
        ),
    )
    .await;
    let fact = MarketFact::Quote(
        Quote::new(QuoteInput {
            quote_id: id(&fixture, &["ids", "quote"]),
            instrument: VersionRef::new(
                id(&fixture, &["ids", "instrument"]),
                Version::new(1).unwrap(),
            ),
            owner,
            source: FactSource::new("china-rates-fixture-v1", "QUOTE-LEGAL", 1).unwrap(),
            observed_at: market_time("2025-01-15T01:01:00Z"),
            received_at: market_time("2025-01-15T01:01:01Z"),
            bid: Some(DecimalValue::new("1012345", 4, price.clone()).unwrap()),
            ask: Some(DecimalValue::new("1012500", 4, price).unwrap()),
            supersedes_id: None,
        })
        .unwrap(),
    );
    let validated = MarketFactUnitResolver::new(&repository)
        .resolve(&scope, fact.clone())
        .await
        .unwrap();
    assert_eq!(
        validated
            .proof()
            .bindings()
            .iter()
            .map(|binding| (binding.role(), binding.ordinal(), binding.dimension()))
            .collect::<Vec<_>>(),
        vec![
            (MarketFactFieldRole::Price, 0, "price"),
            (MarketFactFieldRole::Price, 1, "price"),
        ]
    );
    let validated = MarketFactRulePackResolver::new(&repository)
        .resolve(&scope, validated)
        .await
        .unwrap();
    assert_eq!(
        repository
            .append_fact(
                AppendMarketFact::new(
                    validated,
                    IdempotencyKey::new("q2-inv-01:legal-double-sided").unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap(),
        fact
    );
    assert_eq!(side_effect_counts(&pool).await, (1, 0, 0, 0, 0, 0, 1));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn q2_inv_12_rejects_rule_pack_interval_misses_before_staging() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let owner = OwnerRef::new(
        id(&fixture, &["scope", "tenant_id"]),
        id(&fixture, &["scope", "owner_id"]),
    );
    let repository = PostgresRepository::new(
        pool.clone(),
        Arc::new(
            AeadCursorCodec::new(
                CursorKey::new("acceptance-inv12", [45_u8; 32]).unwrap(),
                vec![],
            )
            .unwrap(),
        ),
    );
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        owner.owner_id().clone(),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    let price = UnitRef::new(
        id(&fixture, &["ids", "units", "price"]),
        Version::new(1).unwrap(),
    );
    publish_unit(
        &repository,
        &owner,
        price.unit_id().clone(),
        "CNY_PER_100_FACE",
        "price",
        4,
    )
    .await;
    let rule_ref = VersionRef::new(
        id(&fixture, &["ids", "rule_pack"]),
        Version::new(1).unwrap(),
    );
    let effective_to = market_time("2025-12-31T15:59:59Z");
    publish_definition(
        &repository,
        &owner,
        DefinitionValue::MarketRulePack(
            MarketRulePack::new(MarketRulePackInput {
                rule_pack_id: rule_ref.id().clone(),
                version: rule_ref.version(),
                owner: owner.clone(),
                market: "CIBM".to_owned(),
                rule_type: "INV12".to_owned(),
                source: "fixture".to_owned(),
                effective: EffectivePeriod::new(
                    market_time("2025-01-01T00:00:00Z"),
                    effective_to.clone(),
                )
                .unwrap(),
                verification_status: VerificationStatus::Verified,
                content_hash: ContentHash::digest(b"inv12-rule"),
            })
            .unwrap(),
        ),
    )
    .await;
    let baseline = side_effect_counts(&pool).await;
    for (label, subject) in [
        ("after", market_time("2026-01-01T00:00:00Z")),
        ("at-to", effective_to.clone()),
    ] {
        let fact = MarketFact::Valuation(
            Valuation::new(ValuationInput {
                valuation_id: id(&fixture, &["ids", "valuation"]),
                instrument: VersionRef::new(
                    id(&fixture, &["ids", "instrument"]),
                    Version::new(1).unwrap(),
                ),
                owner: owner.clone(),
                source: FactSource::new("fixture", label, 1).unwrap(),
                valuation_at: subject,
                method: "inv12".to_owned(),
                rule_pack: rule_ref.clone(),
                values: vec![DecimalValue::new("1012300", 4, price.clone()).unwrap()],
                supersedes_id: None,
            })
            .unwrap(),
        );
        let unit = MarketFactUnitResolver::new(&repository)
            .resolve(&scope, fact)
            .await
            .unwrap();
        let error = MarketFactRulePackResolver::new(&repository)
            .resolve(&scope, unit)
            .await
            .expect_err("half-open interval miss must fail before staging");
        assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
        assert!(!error.retryable());
        assert_eq!(side_effect_counts(&pool).await, baseline);
    }
    for as_of in [market_time("2026-01-01T00:00:00Z"), effective_to] {
        let data = DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: id(&fixture, &["ids", "data_snapshot"]),
            owner: owner.clone(),
            visible_at: market_time("2026-01-01T00:00:01Z"),
            as_of,
            schema_hash: ContentHash::digest(b"schema"),
            manifest_hash: ContentHash::digest(b"manifest"),
            blob_content_hash: ContentHash::digest(b"data"),
            lineage: vec![LineageRef::versioned(
                rule_ref.id().clone(),
                rule_ref.version(),
            )],
        })
        .unwrap();
        let run = ExperimentRun::new(ExperimentRunInput {
            experiment_run_id: id(&fixture, &["ids", "run"]),
            owner: owner.clone(),
            data_snapshot: LineageRef::content_addressed(
                data.id().clone(),
                data.content_hash().clone(),
            ),
            universe_snapshot: LineageRef::content_addressed(
                id(&fixture, &["ids", "universe_snapshot"]),
                ContentHash::digest(b"universe"),
            ),
            rule_packs: vec![rule_ref.clone()],
            runtime_image_digest: ContentHash::digest(b"runtime"),
            parameters_hash: ContentHash::digest(b"params"),
            seed: 1,
        })
        .unwrap();
        let error = Phase1RunCandidateResolver::new(&repository)
            .resolve(&scope, run, &data)
            .await
            .expect_err("run candidate interval miss must fail before staging");
        assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
        assert!(!error.retryable());
        assert_eq!(side_effect_counts(&pool).await, baseline);
    }
}

#[tokio::test]
async fn q2_inv_02_rejects_local_trading_date_drift_without_side_effects() {
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let baseline = side_effect_counts(&pool).await;
    let error = MarketTime::new(
        "2025-01-15T16:30:00Z".parse().unwrap(),
        "Asia/Shanghai",
        "2025-01-15".parse().unwrap(),
    )
    .map_err(map_domain_error)
    .expect_err("Asia/Shanghai instant belongs to the next local trading date");
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert!(!error.retryable());
    assert_eq!(side_effect_counts(&pool).await, baseline);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn q2_inv_11_required_reads_fail_closed_for_missing_corrupt_and_wrong_size() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let owner = OwnerRef::new(
        id(&fixture, &["scope", "tenant_id"]),
        id(&fixture, &["scope", "owner_id"]),
    );
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        owner.owner_id().clone(),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    let repository = PostgresRepository::new(
        pool.clone(),
        Arc::new(
            AeadCursorCodec::new(
                CursorKey::new("acceptance-inv11", [46_u8; 32]).unwrap(),
                vec![],
            )
            .unwrap(),
        ),
    );
    let store = S3BlobStore::new(
        &env::var("FICANT_TEST_S3_ENDPOINT").unwrap(),
        env::var("FICANT_TEST_S3_BUCKET").unwrap(),
        &env::var("FICANT_TEST_S3_ACCESS_KEY").unwrap(),
        &env::var("FICANT_TEST_S3_SECRET_KEY").unwrap(),
        pool.clone(),
    )
    .unwrap();
    let bytes = b"q2-inv-11-promoted-object".to_vec();
    let hash = ContentHash::digest(&bytes);
    let client = raw_s3_client();
    let object_key = S3BlobStore::immutable_key(&hash);
    client
        .delete(&ObjectPath::from(object_key.as_str()))
        .await
        .unwrap();
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner.clone(),
                u64::try_from(bytes.len()).unwrap(),
                unique_test_key("q2-inv-11:stage"),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store.append_chunk(&scope, &staged, bytes).await.unwrap();
    let verified = store
        .verify_and_promote(
            VerifyBlobStage::new(
                scope.clone(),
                staged,
                hash.clone(),
                u64::try_from(b"q2-inv-11-promoted-object".len()).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let anchor = id(&fixture, &["ids", "units", "cny"]);
    publish_unit(
        &repository,
        &owner,
        anchor.clone(),
        "INV11_ANCHOR",
        "currency",
        2,
    )
    .await;
    let artifact = Artifact::new(
        id(&fixture, &["ids", "artifact"]),
        owner.clone(),
        ArtifactKind::Generic,
        "application/octet-stream",
        hash.clone(),
        u64::try_from(b"q2-inv-11-promoted-object".len()).unwrap(),
        vec![LineageRef::versioned(anchor, Version::new(1).unwrap())],
    )
    .unwrap();
    repository
        .publish_verified_blob(
            PublishArtifact::new(
                artifact.clone(),
                verified,
                IdempotencyKey::new("q2-inv-11:publish").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let baseline = side_effect_counts(&pool).await;
    client
        .delete(&ObjectPath::from(object_key.as_str()))
        .await
        .unwrap();
    assert_required_artifact_integrity_failure(
        &repository,
        &store,
        &scope,
        &artifact,
        IntegrityFailureReason::Missing,
        "required business read must fail closed after administrator deletion",
    )
    .await;
    assert_eq!(side_effect_counts(&pool).await, baseline);

    client
        .put(
            &ObjectPath::from(object_key.as_str()),
            vec![b'x'; b"q2-inv-11-promoted-object".len()].into(),
        )
        .await
        .unwrap();
    assert_required_artifact_integrity_failure(
        &repository,
        &store,
        &scope,
        &artifact,
        IntegrityFailureReason::HashMismatch,
        "required business read must reject same-size corruption",
    )
    .await;
    assert_eq!(side_effect_counts(&pool).await, baseline);

    client
        .put(
            &ObjectPath::from(object_key.as_str()),
            vec![b'y'; b"q2-inv-11-promoted-object".len() + 1].into(),
        )
        .await
        .unwrap();
    assert_required_artifact_integrity_failure(
        &repository,
        &store,
        &scope,
        &artifact,
        IntegrityFailureReason::SizeMismatch,
        "required business read must reject size drift",
    )
    .await;
    assert_eq!(side_effect_counts(&pool).await, baseline);

    client
        .put(
            &ObjectPath::from(object_key.as_str()),
            b"q2-inv-11-promoted-object".to_vec().into(),
        )
        .await
        .unwrap();
    let required = RequiredVerifiedBlobRead::new(
        scope.clone(),
        owner.clone(),
        VerifiedReadResourceKind::Artifact,
        artifact.id().clone(),
        VerifiedBlobRole::ArtifactPayload,
        hash.clone(),
        u64::try_from(b"q2-inv-11-promoted-object".len()).unwrap(),
        SafeTraceContext::new("22222222222222222222222222222222").unwrap(),
    )
    .unwrap();
    let drift_owner = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F99").unwrap();
    sqlx::query(
        "UPDATE research.artifacts SET owner_id=$3
         WHERE tenant_id=$1 AND artifact_id=$2",
    )
    .bind(owner.tenant_id().as_str())
    .bind(artifact.id().as_str())
    .bind(drift_owner.as_str())
    .execute(&pool)
    .await
    .unwrap();
    let drift_baseline = durable_read_counts(&pool).await;
    let drift_sink = RecordingSink::default();
    let drift_error = store
        .read_required(&required, &drift_sink)
        .await
        .expect_err("formal durable reference owner drift must fail closed");
    assert_eq!(
        drift_error.category(),
        ApplicationErrorCategory::HashMismatch
    );
    assert!(!drift_error.retryable());
    {
        let drift_events = drift_sink.events.lock().unwrap();
        assert_eq!(drift_events.len(), 1);
        assert_eq!(
            drift_events[0].reason(),
            IntegrityFailureReason::HashMismatch
        );
    }
    assert_eq!(durable_read_counts(&pool).await, drift_baseline);

    sqlx::query(
        "UPDATE research.artifacts SET owner_id=$3
         WHERE tenant_id=$1 AND artifact_id=$2",
    )
    .bind(owner.tenant_id().as_str())
    .bind(artifact.id().as_str())
    .bind(owner.owner_id().as_str())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM research.artifacts WHERE tenant_id=$1 AND artifact_id=$2")
        .bind(owner.tenant_id().as_str())
        .bind(artifact.id().as_str())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.probe_verified(&hash).await.unwrap(),
        Some(b"q2-inv-11-promoted-object".to_vec())
    );
    let missing_ref_baseline = durable_read_counts(&pool).await;
    let missing_ref_sink = RecordingSink::default();
    let missing_ref_error = store
        .read_required(&required, &missing_ref_sink)
        .await
        .expect_err("blob without its formal durable reference must fail closed");
    assert_eq!(
        missing_ref_error.category(),
        ApplicationErrorCategory::HashMismatch
    );
    assert!(!missing_ref_error.retryable());
    {
        let missing_ref_events = missing_ref_sink.events.lock().unwrap();
        assert_eq!(missing_ref_events.len(), 1);
        assert_eq!(
            missing_ref_events[0].reason(),
            IntegrityFailureReason::Missing
        );
    }
    assert_eq!(durable_read_counts(&pool).await, missing_ref_baseline);
    client
        .delete(&ObjectPath::from(object_key.as_str()))
        .await
        .unwrap();
}

fn raw_s3_client() -> AmazonS3 {
    let endpoint = env::var("FICANT_TEST_S3_ENDPOINT").unwrap();
    AmazonS3Builder::new()
        .with_endpoint(&endpoint)
        .with_bucket_name(env::var("FICANT_TEST_S3_BUCKET").unwrap())
        .with_access_key_id(env::var("FICANT_TEST_S3_ACCESS_KEY").unwrap())
        .with_secret_access_key(env::var("FICANT_TEST_S3_SECRET_KEY").unwrap())
        .with_region("us-east-1")
        .with_allow_http(endpoint.starts_with("http://"))
        .with_virtual_hosted_style_request(false)
        .build()
        .unwrap()
}

#[tokio::test]
async fn q2_inv_05_definition_replay_is_idempotent_without_count_growth() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let repository = PostgresRepository::new(
        pool.clone(),
        Arc::new(
            AeadCursorCodec::new(
                CursorKey::new("acceptance-inv05", [47_u8; 32]).unwrap(),
                vec![],
            )
            .unwrap(),
        ),
    );
    let owner = OwnerRef::new(
        id(&fixture, &["scope", "tenant_id"]),
        id(&fixture, &["scope", "owner_id"]),
    );
    let unit_id = id(&fixture, &["ids", "units", "cny"]);
    let identity = DefinitionIdentity::new(
        unit_id.clone(),
        owner.clone(),
        ficant_application::ports::DefinitionKind::Unit,
        IdempotencyKey::new("q2-inv-05:identity").unwrap(),
    );
    repository.create_identity(identity.clone()).await.unwrap();
    repository.create_identity(identity).await.unwrap();
    let value = DefinitionValue::Unit(
        Unit::new(UnitInput {
            unit_id,
            version: Version::new(1).unwrap(),
            owner,
            code: "INV05_CNY".to_owned(),
            dimension: "currency".to_owned(),
            scale: 2,
            precision: 18,
        })
        .unwrap(),
    );
    let command = AppendDefinitionVersion::new(
        None,
        value.clone(),
        IdempotencyKey::new("q2-inv-05:version").unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository.append_version(command.clone()).await.unwrap(),
        value
    );
    let baseline: (i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM market.units), (SELECT COUNT(*) FROM core.idempotency_records WHERE scope='definition:append:v1')").fetch_one(&pool).await.unwrap();
    assert_eq!(repository.append_version(command).await.unwrap(), value);
    assert_eq!(sqlx::query_as::<_, (i64,i64)>("SELECT (SELECT COUNT(*) FROM market.units), (SELECT COUNT(*) FROM core.idempotency_records WHERE scope='definition:append:v1')").fetch_one(&pool).await.unwrap(), baseline);
}

#[tokio::test]
async fn q2_inv_07_hash_mismatch_never_promotes_or_publishes() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let owner = OwnerRef::new(
        id(&fixture, &["scope", "tenant_id"]),
        id(&fixture, &["scope", "owner_id"]),
    );
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        owner.owner_id().clone(),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    let store = S3BlobStore::new(
        &env::var("FICANT_TEST_S3_ENDPOINT").unwrap(),
        env::var("FICANT_TEST_S3_BUCKET").unwrap(),
        &env::var("FICANT_TEST_S3_ACCESS_KEY").unwrap(),
        &env::var("FICANT_TEST_S3_SECRET_KEY").unwrap(),
        pool.clone(),
    )
    .unwrap();
    let bytes = b"q2-inv-07-actual".to_vec();
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner,
                u64::try_from(bytes.len()).unwrap(),
                unique_test_key("q2-inv-07:stage"),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(&scope, &staged, bytes.clone())
        .await
        .unwrap();
    let baseline = side_effect_counts(&pool).await;
    let error = store
        .verify_and_promote(
            VerifyBlobStage::new(
                scope.clone(),
                staged.clone(),
                ContentHash::digest(b"different"),
                u64::try_from(bytes.len()).unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect_err("server hash mismatch must reject promotion");
    assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);
    assert!(!error.retryable());
    assert_eq!(side_effect_counts(&pool).await, baseline);
    store.discard_stage(&scope, &staged).await.unwrap();
}

#[tokio::test]
async fn q2_inv_10_concurrent_definition_append_has_one_winner() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let repository = PostgresRepository::new(
        pool.clone(),
        Arc::new(
            AeadCursorCodec::new(
                CursorKey::new("acceptance-inv10", [48_u8; 32]).unwrap(),
                vec![],
            )
            .unwrap(),
        ),
    );
    let owner = OwnerRef::new(
        id(&fixture, &["scope", "tenant_id"]),
        id(&fixture, &["scope", "owner_id"]),
    );
    let unit_id = id(&fixture, &["ids", "units", "cny"]);
    repository
        .create_identity(DefinitionIdentity::new(
            unit_id.clone(),
            owner.clone(),
            ficant_application::ports::DefinitionKind::Unit,
            IdempotencyKey::new("q2-inv-10:identity").unwrap(),
        ))
        .await
        .unwrap();
    repository
        .append_version(
            AppendDefinitionVersion::new(
                None,
                DefinitionValue::Unit(
                    Unit::new(UnitInput {
                        unit_id: unit_id.clone(),
                        version: Version::new(1).unwrap(),
                        owner: owner.clone(),
                        code: "INV10_V1".to_owned(),
                        dimension: "currency".to_owned(),
                        scale: 2,
                        precision: 18,
                    })
                    .unwrap(),
                ),
                IdempotencyKey::new("q2-inv-10:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let append = |code: &str, key: &str| {
        repository.append_version(
            AppendDefinitionVersion::new(
                Some(Version::new(1).unwrap()),
                DefinitionValue::Unit(
                    Unit::new(UnitInput {
                        unit_id: unit_id.clone(),
                        version: Version::new(2).unwrap(),
                        owner: owner.clone(),
                        code: code.to_owned(),
                        dimension: "currency".to_owned(),
                        scale: 2,
                        precision: 18,
                    })
                    .unwrap(),
                ),
                IdempotencyKey::new(key).unwrap(),
            )
            .unwrap(),
        )
    };
    let (left, right) = tokio::join!(
        append("INV10_LEFT", "q2-inv-10:left"),
        append("INV10_RIGHT", "q2-inv-10:right")
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let error = left.err().or_else(|| right.err()).unwrap();
    assert_eq!(error.category(), ApplicationErrorCategory::VersionConflict);
    assert!(error.retryable());
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM market.units WHERE unit_id=$1 AND version=2")
            .bind(unit_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn q2_inv_09_promote_transport_failure_leaves_no_formal_reference() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let owner = OwnerRef::new(
        id(&fixture, &["scope", "tenant_id"]),
        id(&fixture, &["scope", "owner_id"]),
    );
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        owner.owner_id().clone(),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    let good = S3BlobStore::new(
        &env::var("FICANT_TEST_S3_ENDPOINT").unwrap(),
        env::var("FICANT_TEST_S3_BUCKET").unwrap(),
        &env::var("FICANT_TEST_S3_ACCESS_KEY").unwrap(),
        &env::var("FICANT_TEST_S3_SECRET_KEY").unwrap(),
        pool.clone(),
    )
    .unwrap();
    let bytes = b"q2-inv-09-interrupted".to_vec();
    let hash = ContentHash::digest(&bytes);
    let staged = good
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner,
                u64::try_from(bytes.len()).unwrap(),
                IdempotencyKey::new("q2-inv-09:stage").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    good.append_chunk(&scope, &staged, bytes.clone())
        .await
        .unwrap();
    let baseline = side_effect_counts(&pool).await;
    let broken = S3BlobStore::new(
        "http://127.0.0.1:1",
        env::var("FICANT_TEST_S3_BUCKET").unwrap(),
        &env::var("FICANT_TEST_S3_ACCESS_KEY").unwrap(),
        &env::var("FICANT_TEST_S3_SECRET_KEY").unwrap(),
        pool.clone(),
    )
    .unwrap();
    let error = broken
        .verify_and_promote(
            VerifyBlobStage::new(
                scope.clone(),
                staged.clone(),
                hash.clone(),
                u64::try_from(bytes.len()).unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect_err("transport interruption must not create formal metadata");
    assert_eq!(
        error.category(),
        ApplicationErrorCategory::StorageUnavailable
    );
    assert_eq!(side_effect_counts(&pool).await, baseline);
    good.discard_stage(&scope, &staged).await.unwrap();
    assert_eq!(side_effect_counts(&pool).await, (0, 0, 0, 0, 0, 0, 0));
    assert_eq!(good.probe_verified(&hash).await.unwrap(), None);
}

#[tokio::test]
async fn q2_inv_03_same_instrument_version_cannot_change_symbol() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let repository = PostgresRepository::new(
        pool.clone(),
        Arc::new(
            AeadCursorCodec::new(
                CursorKey::new("acceptance-inv03", [49_u8; 32]).unwrap(),
                vec![],
            )
            .unwrap(),
        ),
    );
    let owner = OwnerRef::new(
        id(&fixture, &["scope", "tenant_id"]),
        id(&fixture, &["scope", "owner_id"]),
    );
    let currency = UnitRef::new(
        id(&fixture, &["ids", "units", "cny"]),
        Version::new(1).unwrap(),
    );
    publish_unit(
        &repository,
        &owner,
        currency.unit_id().clone(),
        "INV03_CNY",
        "currency",
        2,
    )
    .await;
    let calendar = VersionRef::new(id(&fixture, &["ids", "calendar"]), Version::new(1).unwrap());
    publish_definition(
        &repository,
        &owner,
        DefinitionValue::Calendar(
            Calendar::new(CalendarInput {
                calendar_id: calendar.id().clone(),
                version: calendar.version(),
                owner: owner.clone(),
                market: "CIBM".to_owned(),
                market_timezone: "Asia/Shanghai".to_owned(),
                effective: EffectivePeriod::new(
                    market_time("2025-01-01T00:00:00Z"),
                    market_time("2025-12-31T15:59:59Z"),
                )
                .unwrap(),
                sessions: vec![],
            })
            .unwrap(),
        ),
    )
    .await;
    let instrument_id = id(&fixture, &["ids", "instrument"]);
    let make = |symbol: &str| {
        DefinitionValue::Instrument(
            InstrumentDefinition::new(
                Instrument::new(InstrumentInput {
                    instrument_id: instrument_id.clone(),
                    version: Version::new(1).unwrap(),
                    owner: owner.clone(),
                    kind: InstrumentKind::Other,
                    market: "CIBM".to_owned(),
                    symbol: symbol.to_owned(),
                    currency: currency.clone(),
                    calendar: calendar.clone(),
                })
                .unwrap(),
                None,
            )
            .unwrap(),
        )
    };
    publish_definition(&repository, &owner, make("2400001.IB")).await;
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        owner.owner_id().clone(),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    let original = repository
        .get_version(&scope, instrument_id.clone(), Version::new(1).unwrap())
        .await
        .unwrap();
    let error = AppendDefinitionVersion::new(
        Some(Version::new(1).unwrap()),
        make("MUTATED.IB"),
        IdempotencyKey::new("q2-inv-03:rewrite").unwrap(),
    )
    .expect_err("same version rewrite must fail before storage");
    assert_eq!(error.category(), ApplicationErrorCategory::VersionConflict);
    assert!(error.retryable());
    assert_eq!(
        repository
            .get_version(&scope, instrument_id, Version::new(1).unwrap())
            .await
            .unwrap(),
        original
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn q2_inv_04_published_snapshot_does_not_drift_after_source_revision_two() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let repository = PostgresRepository::new(
        pool.clone(),
        Arc::new(
            AeadCursorCodec::new(
                CursorKey::new("acceptance-inv04", [50_u8; 32]).unwrap(),
                vec![],
            )
            .unwrap(),
        ),
    );
    let owner = OwnerRef::new(
        id(&fixture, &["scope", "tenant_id"]),
        id(&fixture, &["scope", "owner_id"]),
    );
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        owner.owner_id().clone(),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    let anchor = id(&fixture, &["ids", "units", "cny"]);
    publish_unit(
        &repository,
        &owner,
        anchor.clone(),
        "INV04_SOURCE",
        "currency",
        2,
    )
    .await;
    let store = S3BlobStore::new(
        &env::var("FICANT_TEST_S3_ENDPOINT").unwrap(),
        env::var("FICANT_TEST_S3_BUCKET").unwrap(),
        &env::var("FICANT_TEST_S3_ACCESS_KEY").unwrap(),
        &env::var("FICANT_TEST_S3_SECRET_KEY").unwrap(),
        pool,
    )
    .unwrap();
    let data = b"inv04-data";
    let manifest = b"inv04-manifest";
    let data_hash = ContentHash::digest(data);
    let manifest_hash = ContentHash::digest(manifest);
    let snapshot = SnapshotValue::Data(
        DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: id(&fixture, &["ids", "data_snapshot"]),
            owner: owner.clone(),
            visible_at: market_time("2025-01-15T07:05:00Z"),
            as_of: market_time("2025-01-15T07:00:00Z"),
            schema_hash: ContentHash::digest(b"schema"),
            manifest_hash: manifest_hash.clone(),
            blob_content_hash: data_hash.clone(),
            lineage: vec![LineageRef::versioned(
                anchor.clone(),
                Version::new(1).unwrap(),
            )],
        })
        .unwrap(),
    );
    let proof = VerifiedSnapshotProof::data(
        verified_snapshot_blob(
            &store,
            &scope,
            &owner,
            SnapshotBlobRole::DataParquet,
            "q2-inv-04:data",
            data,
        )
        .await,
        verified_snapshot_blob(
            &store,
            &scope,
            &owner,
            SnapshotBlobRole::DataManifest,
            "q2-inv-04:manifest",
            manifest,
        )
        .await,
    )
    .unwrap();
    repository
        .publish_verified_manifest(
            PublishSnapshot::new(
                snapshot.clone(),
                proof,
                IdempotencyKey::new("q2-inv-04:publish").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    repository
        .append_version(
            AppendDefinitionVersion::new(
                Some(Version::new(1).unwrap()),
                DefinitionValue::Unit(
                    Unit::new(UnitInput {
                        unit_id: anchor,
                        version: Version::new(2).unwrap(),
                        owner,
                        code: "INV04_SOURCE_V2".to_owned(),
                        dimension: "currency".to_owned(),
                        scale: 2,
                        precision: 18,
                    })
                    .unwrap(),
                ),
                IdempotencyKey::new("q2-inv-04:source-v2").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .get_by_id(&scope, id(&fixture, &["ids", "data_snapshot"]))
            .await
            .unwrap(),
        Some(snapshot)
    );
}

#[tokio::test]
async fn q2_inv_06_missing_rule_pack_lineage_is_rejected_without_publication() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let baseline = side_effect_counts(&pool).await;
    let owner = OwnerRef::new(
        id(&fixture, &["scope", "tenant_id"]),
        id(&fixture, &["scope", "owner_id"]),
    );
    let data = LineageRef::content_addressed(
        id(&fixture, &["ids", "data_snapshot"]),
        ContentHash::digest(b"data"),
    );
    let universe = LineageRef::content_addressed(
        id(&fixture, &["ids", "universe_snapshot"]),
        ContentHash::digest(b"universe"),
    );
    let artifact = LineageRef::content_addressed(
        id(&fixture, &["ids", "artifact"]),
        ContentHash::digest(b"artifact"),
    );
    let error = SignalSet::new(SignalSetInput {
        signal_set_id: id(&fixture, &["ids", "signal"]),
        owner,
        artifact,
        experiment_run_id: id(&fixture, &["ids", "run"]),
        data_snapshot: data,
        universe_snapshot: universe,
        rule_packs: vec![],
        input_artifacts: vec![LineageRef::versioned(
            id(&fixture, &["ids", "curve"]),
            Version::new(1).unwrap(),
        )],
        valid: EffectivePeriod::new(
            market_time("2025-01-15T07:15:00Z"),
            market_time("2025-01-16T07:15:00Z"),
        )
        .unwrap(),
    })
    .map_err(map_domain_error)
    .expect_err("SignalSet without RulePack lineage must fail");
    assert_eq!(
        error.category(),
        ApplicationErrorCategory::LineageIncomplete
    );
    assert!(!error.retryable());
    assert_eq!(side_effect_counts(&pool).await, baseline);
}

#[tokio::test]
async fn q2_inv_08_out_of_order_journal_replay_is_rejected_without_persistence() {
    let fixture = fixture();
    let pool = connect_postgres().await;
    reset_and_migrate(&pool).await;
    let baseline = side_effect_counts(&pool).await;
    let input = RunJournalInput {
        journal_event_id: Ulid::new(fixture["ids"]["journal"][0].as_str().unwrap()).unwrap(),
        run_id: id(&fixture, &["ids", "run"]),
        sequence: 4,
        event_type: JournalEventType::RunCreated,
        occurred_at: market_time("2025-01-15T07:16:00Z"),
        payload_type: "ficant.research.v1.RunCreated".to_owned(),
        payload_schema: "v1".to_owned(),
        payload: b"seq4".to_vec(),
        prev_hash: None,
    };
    let error = input
        .canonical_hash()
        .map_err(map_domain_error)
        .expect_err("sequence four cannot start a journal");
    assert_eq!(
        error.category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );
    assert!(error.retryable());
    assert_eq!(side_effect_counts(&pool).await, baseline);
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

async fn assert_required_artifact_integrity_failure(
    repository: &PostgresRepository,
    store: &S3BlobStore,
    scope: &AccessScope,
    artifact: &Artifact,
    reason: IntegrityFailureReason,
    expectation: &str,
) {
    let sink = RecordingSink::default();
    let facade = VerifiedReadFacade::new(repository, repository, repository, store, &sink);
    let error = facade
        .read_verified_artifact(
            scope,
            artifact.id().clone(),
            SafeTraceContext::new("11111111111111111111111111111111").unwrap(),
        )
        .await
        .expect_err(expectation);
    assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);
    assert!(!error.retryable());
    let events = sink.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].reason(), reason);
}

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden-cases/china-rates/phase1-business-loop.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn unique_test_key(prefix: &str) -> IdempotencyKey {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    IdempotencyKey::new(format!("{prefix}:process-{}:{nonce}", std::process::id())).unwrap()
}

fn id(value: &Value, path: &[&str]) -> Ulid {
    let value = path
        .iter()
        .fold(value, |current, key| current.get(*key).unwrap());
    Ulid::new(value.as_str().unwrap()).unwrap()
}

fn market_time(instant: &str) -> MarketTime {
    MarketTime::new(
        instant.parse().unwrap(),
        "Asia/Shanghai",
        instant[..10].parse().unwrap(),
    )
    .unwrap()
}

async fn connect_postgres() -> PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .connect(&env::var("FICANT_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap()
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
    .unwrap();
    sqlx::migrate::Migrator::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations/postgresql"),
    )
    .await
    .unwrap()
    .run(pool)
    .await
    .unwrap();
}

async fn publish_unit(
    repository: &PostgresRepository,
    owner: &OwnerRef,
    unit_id: Ulid,
    code: &str,
    dimension: &str,
    scale: u32,
) {
    publish_definition(
        repository,
        owner,
        DefinitionValue::Unit(
            Unit::new(UnitInput {
                unit_id,
                version: Version::new(1).unwrap(),
                owner: owner.clone(),
                code: code.to_owned(),
                dimension: dimension.to_owned(),
                scale,
                precision: 18,
            })
            .unwrap(),
        ),
    )
    .await;
}

async fn publish_definition(
    repository: &PostgresRepository,
    owner: &OwnerRef,
    value: DefinitionValue,
) {
    let identity = value.identity().to_owned();
    repository
        .create_identity(DefinitionIdentity::new(
            Ulid::new(&identity).unwrap(),
            owner.clone(),
            value.kind(),
            IdempotencyKey::new(format!("q2-inv-01:{identity}:identity")).unwrap(),
        ))
        .await
        .unwrap();
    repository
        .append_version(
            AppendDefinitionVersion::new(
                None,
                value,
                IdempotencyKey::new(format!("q2-inv-01:{identity}:version")).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn verified_snapshot_blob(
    store: &S3BlobStore,
    scope: &AccessScope,
    owner: &OwnerRef,
    role: SnapshotBlobRole,
    key: &str,
    bytes: &[u8],
) -> VerifiedSnapshotBlob {
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner.clone(),
                u64::try_from(bytes.len()).unwrap(),
                IdempotencyKey::new(key).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(scope, &staged, bytes.to_vec())
        .await
        .unwrap();
    let verification = VerifyBlobStage::new(
        scope.clone(),
        staged,
        ContentHash::digest(bytes),
        u64::try_from(bytes.len()).unwrap(),
    )
    .unwrap();
    let binding = StagedSnapshotBlob::new(role, verification.clone());
    let verified = store.verify_and_promote(verification).await.unwrap();
    VerifiedSnapshotBlob::from_staged(binding, verified).unwrap()
}

async fn side_effect_counts(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT
             ((SELECT COUNT(*) FROM market.quotes)
              + (SELECT COUNT(*) FROM market.trades)),
             (SELECT COUNT(*) FROM storage.blobs),
             (SELECT COUNT(*) FROM storage.staging_uploads),
             (SELECT COUNT(*) FROM storage.orphan_candidates),
             (SELECT COUNT(*) FROM research.experiment_runs),
             (SELECT COUNT(*) FROM research.run_journal),
             (SELECT COUNT(*) FROM core.idempotency_records
              WHERE scope = 'market-fact:write:v1')",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn durable_read_counts(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM storage.blobs),
             (SELECT COUNT(*) FROM research.artifacts),
             (SELECT COUNT(*) FROM research.signal_sets),
             (SELECT COUNT(*) FROM research.data_snapshots),
             (SELECT COUNT(*) FROM research.universe_snapshots),
             (SELECT COUNT(*) FROM research.experiment_runs),
             (SELECT COUNT(*) FROM research.run_journal)",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}
