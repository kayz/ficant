mod support;

use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    ApplicationResult, ArtifactRepository, FormalOutputRecord, FormalOutputRepository,
    IdempotencyKey, IntegrityEvent, IntegrityEventSink, PublishArtifact, PublishSignalSet,
    SafeTraceContext, SignalRepository, VerifiedBlobRef,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_domain::research::{Artifact, ArtifactKind, SignalSet, SignalSetInput};
use ficant_domain::{ContentAddressed, Lineaged};
use ficant_runtime::{
    CodeBinding, FormalInputBinding, FormalInputBindingInput, FormalInputKind,
    FormalInputReference, FormalOutputEvidence, FormalOutputEvidenceInput, RuntimeBinding,
};

fn id(value: &str) -> Ulid {
    Ulid::new(value).expect("fixture ULID")
}

fn owner() -> OwnerRef {
    OwnerRef::new(
        id("01ARZ3NDEKTSV4RRFFQ69G5F01"),
        id("01ARZ3NDEKTSV4RRFFQ69G5F02"),
    )
}

fn record(payload: &[u8]) -> FormalOutputRecord {
    let subject = FormalInputBinding::new(FormalInputBindingInput {
        role: "subject".to_owned(),
        kind: FormalInputKind::Subject,
        owner: owner(),
        reference: FormalInputReference::Object(
            LineageRef::new(
                id("01ARZ3NDEKTSV4RRFFQ69G5F03"),
                Some(Version::new(1).expect("version")),
                Some(ContentHash::digest(b"subject")),
            )
            .expect("subject ref"),
        ),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .expect("subject");
    let evidence = FormalOutputEvidence::new(FormalOutputEvidenceInput {
        schema_id: "ficant.test.v1.Output".to_owned(),
        subject,
        consumed_inputs: vec![],
        code: CodeBinding::new(
            "34402344c7d2c9238dc171af52ac4db77eb6b462",
            "f66e03c55703837d6f2aee9959eba482612272f1",
        )
        .expect("code"),
        runtime: RuntimeBinding::new(
            ContentHash::digest(b"image"),
            ContentHash::digest(b"environment"),
        ),
        implementations: vec![],
        parameters_hash: ContentHash::digest(b"parameters"),
        seed: None,
        result_hash: ContentHash::digest(payload),
    })
    .expect("evidence");
    FormalOutputRecord::new(owner(), evidence, payload.to_vec()).expect("record")
}

#[tokio::test]
async fn formal_output_publish_replays_exactly_and_required_read_detects_sql_drift() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let scope = support::access_scope(&owner());
    let expected = record(b"canonical-result");

    assert_eq!(
        FormalOutputRepository::publish(&repository, &scope, expected.clone())
            .await
            .expect("first publish"),
        expected
    );
    assert_eq!(
        FormalOutputRepository::publish(&repository, &scope, expected.clone())
            .await
            .expect("exact replay"),
        expected
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM analytics.formal_outputs")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);

    sqlx::query("UPDATE analytics.formal_outputs SET result_payload=$1")
        .bind(b"tampered".as_slice())
        .execute(&pool)
        .await
        .expect("tamper fixture");
    let error = FormalOutputRepository::get(&repository, &scope, expected.output_identity())
        .await
        .expect_err("required read must reject payload drift");
    assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn formal_signal_set_and_artifact_share_one_cross_checked_evidence_record() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let scope = support::access_scope(&owner());
    let payload = b"formal-signal-payload";
    let payload_hash = ContentHash::digest(payload);
    let evidence = record(payload).evidence().clone();
    let data = LineageRef::content_addressed(
        id("01ARZ3NDEKTSV4RRFFQ69G5F04"),
        ContentHash::digest(b"data-snapshot"),
    );
    let universe = LineageRef::content_addressed(
        id("01ARZ3NDEKTSV4RRFFQ69G5F05"),
        ContentHash::digest(b"universe-snapshot"),
    );
    let rule_pack = VersionRef::new(
        id("01ARZ3NDEKTSV4RRFFQ69G5F06"),
        Version::new(1).expect("version"),
    );
    seed_lineage_dependencies(&pool, &data, &universe, &rule_pack).await;
    let input_payload = b"formal-signal-input";
    let input_hash = ContentHash::digest(input_payload);
    seed_blob(&pool, &input_hash, input_payload.len()).await;
    let input_artifact = Artifact::new(
        id("01ARZ3NDEKTSV4RRFFQ69G5F07"),
        owner(),
        ArtifactKind::Generic,
        "application/octet-stream",
        input_hash.clone(),
        u64::try_from(input_payload.len()).expect("blob size"),
        vec![data.clone()],
    )
    .expect("input Artifact");
    ArtifactRepository::publish_verified_blob(
        &repository,
        PublishArtifact::new(
            input_artifact.clone(),
            VerifiedBlobRef::new(
                input_hash.clone(),
                u64::try_from(input_payload.len()).expect("blob size"),
            )
            .expect("input blob"),
            IdempotencyKey::new("r7b/formal-signal-input").expect("key"),
        )
        .expect("publish input Artifact"),
    )
    .await
    .expect("input Artifact persisted");
    let input = LineageRef::content_addressed(input_artifact.id().clone(), input_hash);
    let artifact = Artifact::new(
        id("01ARZ3NDEKTSV4RRFFQ69G5F08"),
        owner(),
        ArtifactKind::SignalSet,
        "application/vnd.ficant.signal-set.v1",
        payload_hash.clone(),
        u64::try_from(payload.len()).expect("blob size"),
        vec![
            data.clone(),
            universe.clone(),
            LineageRef::versioned(rule_pack.id().clone(), rule_pack.version()),
            input.clone(),
        ],
    )
    .expect("formal Artifact");
    seed_blob(&pool, &payload_hash, payload.len()).await;
    let verified = VerifiedBlobRef::new(
        payload_hash,
        u64::try_from(payload.len()).expect("blob size"),
    )
    .expect("verified blob");
    ArtifactRepository::publish_verified_blob(
        &repository,
        PublishArtifact::new_formal(
            artifact.clone(),
            verified.clone(),
            IdempotencyKey::new("r7b/formal-artifact").expect("key"),
            evidence.clone(),
        )
        .expect("publish Artifact"),
    )
    .await
    .expect("Artifact persisted");

    let run_id = id("01ARZ3NDEKTSV4RRFFQ69G5F0A");
    seed_experiment_run(&pool, &run_id).await;
    let signal = SignalSet::new(SignalSetInput {
        signal_set_id: id("01ARZ3NDEKTSV4RRFFQ69G5F09"),
        owner: owner(),
        artifact: LineageRef::content_addressed(
            artifact.id().clone(),
            artifact.content_hash().clone(),
        ),
        experiment_run_id: run_id,
        data_snapshot: data,
        universe_snapshot: universe,
        rule_packs: vec![rule_pack],
        input_artifacts: vec![input],
        valid: EffectivePeriod::new(market_time(9), market_time(10)).expect("period"),
    })
    .expect("formal SignalSet");
    let stored_artifact =
        ArtifactRepository::get_metadata(&repository, &scope, artifact.id().clone())
            .await
            .expect("Artifact read")
            .expect("Artifact exists");
    assert_eq!(stored_artifact, artifact);
    assert_eq!(
        ArtifactRepository::get_formal_evidence(&repository, &scope, artifact.id().clone())
            .await
            .expect("Artifact evidence read"),
        Some(evidence.clone())
    );
    assert_eq!(signal.lineage().get(1..), Some(artifact.lineage()));
    SignalRepository::publish(
        &repository,
        PublishSignalSet::new_formal(
            signal.clone(),
            verified,
            IdempotencyKey::new("r7b/formal-signal").expect("key"),
            evidence.clone(),
        )
        .expect("publish SignalSet"),
    )
    .await
    .expect("SignalSet persisted");

    let loaded = SignalRepository::get(&repository, &scope, signal.id().clone())
        .await
        .expect("SignalSet read")
        .expect("SignalSet exists");
    assert_eq!(loaded, signal);
    assert_eq!(
        SignalRepository::get_formal_evidence(&repository, &scope, loaded.id().clone())
            .await
            .expect("Signal evidence read"),
        Some(evidence.clone())
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM research.artifact_formal_evidence")
            .fetch_one(&pool)
            .await
            .expect("evidence count"),
        1,
    );

    sqlx::query("UPDATE research.artifact_formal_evidence SET subject_content_hash=repeat('0',64)")
        .execute(&pool)
        .await
        .expect("tamper normalized evidence");
    let events = CountingIntegritySink::default();
    let error = SignalRepository::get_integrity_checked(
        &repository,
        &scope,
        signal.id().clone(),
        SafeTraceContext::new("0123456789abcdef0123456789abcdef").expect("trace"),
        &events,
    )
    .await
    .expect_err("normalized evidence drift must fail closed");
    assert!(matches!(
        error.category(),
        ApplicationErrorCategory::ImmutableViolation
            | ApplicationErrorCategory::LineageIncomplete
            | ApplicationErrorCategory::HashMismatch
    ));
    assert_eq!(events.count.load(Ordering::SeqCst), 1);
}

#[derive(Default)]
struct CountingIntegritySink {
    count: AtomicUsize,
}

#[async_trait]
impl IntegrityEventSink for CountingIntegritySink {
    async fn emit(&self, _event: IntegrityEvent) -> ApplicationResult<()> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn market_time(hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 8, 20, hour, 0, 0)
            .single()
            .expect("instant"),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 8, 20).expect("date"),
    )
    .expect("market time")
}

async fn seed_lineage_dependencies(
    pool: &sqlx::PgPool,
    data: &LineageRef,
    universe: &LineageRef,
    rule_pack: &VersionRef,
) {
    let manifest_hash = ContentHash::digest(b"manifest");
    seed_blob(pool, data.content_hash().expect("data hash"), 1).await;
    seed_blob(pool, universe.content_hash().expect("universe hash"), 1).await;
    seed_blob(pool, &manifest_hash, 1).await;
    sqlx::query(
        "INSERT INTO research.data_snapshots
         (tenant_id,data_snapshot_id,owner_id,visible_at,as_of,schema_hash,manifest_hash,
          content_hash,idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,$4,$5,$6,$7,$8,$9)",
    )
    .bind(owner().tenant_id().as_str())
    .bind(data.object_id().as_str())
    .bind(owner().owner_id().as_str())
    .bind(hash_hex(&ContentHash::digest(b"schema")))
    .bind(hash_hex(&manifest_hash))
    .bind(hash_hex(data.content_hash().expect("data hash")))
    .bind("r7b/formal-signal-data")
    .bind(vec![1_u8; 32])
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .expect("data snapshot lineage target");
    sqlx::query(
        "INSERT INTO research.universe_snapshots
         (tenant_id,universe_snapshot_id,owner_id,filter_digest,content_hash,
          idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(owner().tenant_id().as_str())
    .bind(universe.object_id().as_str())
    .bind(owner().owner_id().as_str())
    .bind(hash_hex(&ContentHash::digest(b"filter")))
    .bind(hash_hex(universe.content_hash().expect("universe hash")))
    .bind("r7b/formal-signal-universe")
    .bind(vec![2_u8; 32])
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .expect("universe snapshot lineage target");
    sqlx::query(
        "INSERT INTO market.market_rule_packs
         (tenant_id,rule_pack_id,version,owner_id,market,rule_type,source,
          effective_from,effective_to,verification_status,content_hash,payload)
         VALUES ($1,$2,$3,$4,'CGB','signal','r7b',
                 CURRENT_TIMESTAMP-INTERVAL '1 day',CURRENT_TIMESTAMP+INTERVAL '1 day',
                 'VERIFIED',$5,$6)",
    )
    .bind(owner().tenant_id().as_str())
    .bind(rule_pack.id().as_str())
    .bind(i64::try_from(rule_pack.version().get()).expect("version"))
    .bind(owner().owner_id().as_str())
    .bind(hash_hex(&ContentHash::digest(b"rule-pack")))
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .expect("RulePack lineage target");
}

async fn seed_blob(pool: &sqlx::PgPool, hash: &ContentHash, size: usize) {
    let encoded = hash_hex(hash);
    sqlx::query(
        "INSERT INTO storage.blobs(tenant_id,content_hash,object_key,blob_size)
         VALUES ($1,$2,$3,$4)",
    )
    .bind(owner().tenant_id().as_str())
    .bind(&encoded)
    .bind(format!("immutable/{encoded}"))
    .bind(i64::try_from(size).expect("blob size"))
    .execute(pool)
    .await
    .expect("seed verified blob metadata");
}

async fn seed_experiment_run(pool: &sqlx::PgPool, run_id: &Ulid) {
    sqlx::query(
        "INSERT INTO research.experiment_runs
         (tenant_id,experiment_run_id,owner_id,state,revision,idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,'SUCCEEDED',1,$4,$5,$6)",
    )
    .bind(owner().tenant_id().as_str())
    .bind(run_id.as_str())
    .bind(owner().owner_id().as_str())
    .bind("r7b/formal-signal-run")
    .bind(ContentHash::digest(b"run").as_bytes().as_slice())
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .expect("experiment run lineage target");
}

fn hash_hex(hash: &ContentHash) -> String {
    hash.as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut value, byte| {
            write!(value, "{byte:02x}").expect("write to String");
            value
        })
}
