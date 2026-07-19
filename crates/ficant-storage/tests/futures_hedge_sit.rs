mod support;

use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, BeginBlobStage, BlobStore, IdempotencyKey, IntegrityEvent,
    IntegrityEventSink, IntegrityFailureReason, SafeTraceContext, VerifyBlobStage,
};
use ficant_application::{ApplicationErrorCategory, PublishFuturesHedge, ReplayFuturesHedge};
use ficant_domain::analytics::{AnalyticsObjectRef, FixedDecimal};
use ficant_domain::futures_delivery::CgbFuturesProduct;
use ficant_domain::futures_hedge::FuturesHedgeInput;
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_domain::{ContentAddressed, Lineaged};
use ficant_fixed_income_native::NativeFuturesHedgeEngine;
use ficant_storage::hedge_arrow::ArrowFuturesHedgeCodec;
use ficant_storage::s3::S3BlobStore;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn real_postgres_ceph_hedge_publish_restart_replay_and_tamper_fail_closed() {
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;

    let input = hedge_input();
    let scope = support::access_scope(input.owner());
    let artifact_id = id('M');
    let repository = support::repository(pool.clone());
    let store = S3BlobStore::new(
        &endpoint,
        bucket.clone(),
        &access_key,
        &secret_key,
        pool.clone(),
    )
    .expect("Phase 2D Ceph adapter must initialize");
    seed_lineage(&pool, &store, &scope, &input).await;
    let artifact = PublishFuturesHedge::new(
        &NativeFuturesHedgeEngine,
        &ArrowFuturesHedgeCodec,
        &store,
        &repository,
    )
    .execute(
        scope.clone(),
        artifact_id.clone(),
        &input,
        IdempotencyKey::new("phase2d-futures-hedge-publish-v1").unwrap(),
    )
    .await
    .expect("real stage, verification, seven-part lineage and publication must succeed");
    assert_eq!(artifact.id(), &artifact_id);
    assert_eq!(artifact.lineage().len(), 7);
    assert_eq!(
        store
            .probe_verified(artifact.content_hash())
            .await
            .expect("published Ceph object must be readable")
            .expect("published Ceph object must exist")
            .len(),
        usize::try_from(artifact.blob_size()).unwrap()
    );

    drop(store);
    drop(repository);
    let restarted_repository = support::repository(pool.clone());
    let restarted_store =
        S3BlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool.clone())
            .expect("restarted Ceph adapter must initialize");
    let sink = RecordingSink::default();
    let replay = ReplayFuturesHedge::new(
        &NativeFuturesHedgeEngine,
        &ArrowFuturesHedgeCodec,
        &restarted_repository,
        &restarted_store,
        &sink,
    )
    .execute(
        &scope,
        artifact_id.clone(),
        &input,
        SafeTraceContext::new("2123456789abcdef0123456789abcdef").unwrap(),
    )
    .await
    .expect("restart-safe replay must reproduce exact canonical bytes");
    assert_eq!(replay.stored(), replay.recalculated());
    assert_eq!(replay.artifact(), &artifact);
    assert!(sink.events.lock().unwrap().is_empty());

    sqlx::query(
        "UPDATE research.artifacts SET blob_size = blob_size + 1
         WHERE tenant_id = $1 AND artifact_id = $2",
    )
    .bind(input.owner().tenant_id().as_str())
    .bind(artifact_id.as_str())
    .execute(&pool)
    .await
    .expect("isolated metadata tamper injection must succeed");
    let error = ReplayFuturesHedge::new(
        &NativeFuturesHedgeEngine,
        &ArrowFuturesHedgeCodec,
        &restarted_repository,
        &restarted_store,
        &sink,
    )
    .execute(
        &scope,
        artifact_id,
        &input,
        SafeTraceContext::new("fedcba9876543210fedcba9876543212").unwrap(),
    )
    .await
    .expect_err("tampered formal size must fail closed");
    assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);
    assert_eq!(
        sink.events.lock().unwrap().as_slice(),
        &[IntegrityFailureReason::SizeMismatch]
    );
    let staging_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM storage.staging_uploads")
        .fetch_one(&pool)
        .await
        .unwrap();
    let candidate_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM storage.orphan_candidates")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(staging_count, 0);
    assert_eq!(candidate_count, 0);
}

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<IntegrityFailureReason>>,
}

#[async_trait]
impl IntegrityEventSink for RecordingSink {
    async fn emit(&self, event: IntegrityEvent) -> ApplicationResult<()> {
        self.events.lock().unwrap().push(event.reason());
        Ok(())
    }
}

fn hedge_input() -> FuturesHedgeInput {
    FuturesHedgeInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        object('F'),
        object('G'),
        object('H'),
        object('J'),
        MarketTime::new(
            Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).single().unwrap(),
            "Asia/Shanghai",
            date(2026, 7, 20),
        )
        .unwrap(),
        CgbFuturesProduct::TenYear,
        fixed(500_000_000_000_000),
        fixed(45_000_000_000),
        fixed(900_000_000_000),
    )
    .unwrap()
}

#[allow(clippy::too_many_lines)]
async fn seed_lineage(
    pool: &sqlx::PgPool,
    store: &S3BlobStore,
    scope: &AccessScope,
    input: &FuturesHedgeInput,
) {
    let tenant = input.owner().tenant_id().as_str();
    let owner = input.owner().owner_id().as_str();
    let unit_id = id('K');
    let calendar_id = id('N');
    sqlx::query(
        "INSERT INTO market.units
         (tenant_id, unit_id, version, owner_id, code, dimension, scale, precision, payload)
         VALUES ($1, $2, 1, $3, 'CNY100', 'price', 12, 38, decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(unit_id.as_str())
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.calendars
         (tenant_id, calendar_id, version, owner_id, market, market_timezone,
          effective_from, effective_to, payload)
         VALUES ($1, $2, 1, $3, 'CFFEX', 'Asia/Shanghai',
                 '2024-01-01T00:00:00Z', '2036-01-01T00:00:00Z', decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(calendar_id.as_str())
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.market_rule_packs
         (tenant_id, rule_pack_id, version, owner_id, market, rule_type, source,
          effective_from, effective_to, verification_status, content_hash, payload)
         VALUES ($1, $2, 1, $3, 'CFFEX', 'phase2d-futures-hedge', 'cffex-frozen-fixture',
                 '2024-01-01T00:00:00Z', '2036-01-01T00:00:00Z', 'VERIFIED', $4,
                 decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(input.rule_pack().version_ref().id().as_str())
    .bind(owner)
    .bind(S3BlobStore::hash_hex(input.rule_pack().content_hash()))
    .execute(pool)
    .await
    .unwrap();
    for (reference, kind, symbol) in [
        (input.futures_contract(), "FUTURES", "T2609"),
        (input.ctd_bond(), "BOND", "PHASE2D.CGB"),
    ] {
        sqlx::query(
            "INSERT INTO market.instruments
             (tenant_id, instrument_id, version, owner_id, kind, market, symbol,
              currency_unit_id, currency_unit_version, calendar_id, calendar_version, payload)
             VALUES ($1, $2, 1, $3, $4, 'CFFEX', $5, $6, 1, $7, 1, decode('01', 'hex'))",
        )
        .bind(tenant)
        .bind(reference.version_ref().id().as_str())
        .bind(owner)
        .bind(kind)
        .bind(symbol)
        .bind(unit_id.as_str())
        .bind(calendar_id.as_str())
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO market.futures_contracts
         (tenant_id, instrument_id, version, last_trade_time, expiry_time,
          settlement_time, multiplier_coefficient, multiplier_scale,
          multiplier_unit_id, multiplier_unit_version, rule_pack_id, rule_pack_version, payload)
         VALUES ($1, $2, 1, '2026-09-11T03:30:00Z', '2026-09-11T07:15:00Z',
                 '2026-09-18T09:00:00Z', 1000000000000, 0, $3, 1, $4, 1,
                 decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(input.futures_contract().version_ref().id().as_str())
    .bind(unit_id.as_str())
    .bind(input.rule_pack().version_ref().id().as_str())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.bonds
         (tenant_id, instrument_id, version, issue_date, maturity_date,
          face_coefficient, face_scale, face_unit_id, face_unit_version, payload)
         VALUES ($1, $2, 1, '2024-08-15', '2034-08-15',
                 100000000000000, 12, $3, 1, decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(input.ctd_bond().version_ref().id().as_str())
    .bind(unit_id.as_str())
    .execute(pool)
    .await
    .unwrap();

    persist_blob(
        pool,
        store,
        scope,
        input.snapshot().content_hash(),
        b"J".to_vec(),
        "phase2d-source-snapshot-stage-v1",
    )
    .await;
    let snapshot_hash = S3BlobStore::hash_hex(input.snapshot().content_hash());
    sqlx::query(
        "INSERT INTO research.data_snapshots
         (tenant_id, data_snapshot_id, owner_id, visible_at, as_of, schema_hash,
          manifest_hash, content_hash, idempotency_key, fingerprint, payload)
         VALUES ($1, $2, $3, '2026-07-20T05:00:00Z', '2026-07-20T04:00:00Z',
                 $4, $5, $5, 'phase2d-source-snapshot-v1', $6, decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(input.snapshot().version_ref().id().as_str())
    .bind(owner)
    .bind(S3BlobStore::hash_hex(&ContentHash::digest(
        b"phase2d-schema",
    )))
    .bind(&snapshot_hash)
    .bind(vec![10_u8; 32])
    .execute(pool)
    .await
    .unwrap();

    for (index, (reference, bytes)) in [
        (input.target_risk_artifact(), b"C".to_vec()),
        (input.delivery_artifact(), b"D".to_vec()),
        (input.ctd_analytics_artifact(), b"E".to_vec()),
    ]
    .into_iter()
    .enumerate()
    {
        let key = format!("phase2d-prerequisite-artifact-{index}-stage-v1");
        persist_blob(pool, store, scope, reference.content_hash(), bytes, &key).await;
        sqlx::query(
            "INSERT INTO research.artifacts
             (tenant_id, artifact_id, owner_id, kind, media_type, content_hash, blob_size,
              idempotency_key, fingerprint, payload)
             VALUES ($1, $2, $3, 'GENERIC', 'application/octet-stream', $4, 1, $5, $6,
                     decode('01', 'hex'))",
        )
        .bind(tenant)
        .bind(reference.version_ref().id().as_str())
        .bind(owner)
        .bind(S3BlobStore::hash_hex(reference.content_hash()))
        .bind(format!("phase2d-prerequisite-artifact-{index}-v1"))
        .bind(vec![u8::try_from(index + 11).unwrap(); 32])
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn persist_blob(
    pool: &sqlx::PgPool,
    store: &S3BlobStore,
    scope: &AccessScope,
    expected_hash: &ContentHash,
    bytes: Vec<u8>,
    key: &str,
) {
    assert_eq!(ContentHash::digest(&bytes), *expected_hash);
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                scope_owner(scope),
                u64::try_from(bytes.len()).unwrap(),
                IdempotencyKey::new(key).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(scope, &staged, bytes.clone())
        .await
        .unwrap();
    let verified = store
        .verify_and_promote(
            VerifyBlobStage::new(
                scope.clone(),
                staged,
                expected_hash.clone(),
                u64::try_from(bytes.len()).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let hash = S3BlobStore::hash_hex(verified.content_hash());
    sqlx::query(
        "INSERT INTO storage.blobs (tenant_id, content_hash, object_key, blob_size)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(scope.tenant_id().as_str())
    .bind(&hash)
    .bind(S3BlobStore::immutable_key(verified.content_hash()))
    .bind(i64::try_from(verified.size()).unwrap())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM storage.orphan_candidates WHERE content_hash = $1")
        .bind(&hash)
        .execute(pool)
        .await
        .unwrap();
}

fn scope_owner(scope: &AccessScope) -> OwnerRef {
    OwnerRef::new(
        scope.tenant_id().clone(),
        scope.allowed_owner_ids().first().unwrap().clone(),
    )
}

fn object(suffix: char) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(id(suffix), Version::new(1).unwrap()),
        ContentHash::digest(suffix.to_string().as_bytes()),
    )
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

const fn fixed(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value)
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}
