mod support;

use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, BeginBlobStage, BlobStore, IdempotencyKey, IntegrityEvent,
    IntegrityEventSink, IntegrityFailureReason, SafeTraceContext, VerifyBlobStage,
};
use ficant_application::{ApplicationErrorCategory, PublishCarryRoll, ReplayCarryRoll};
use ficant_domain::ContentAddressed;
use ficant_domain::analytics::{
    AnalyticsObjectRef, BondTerms, BusinessDayConvention, CalendarBinding, CalendarRequirement,
    CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::curves::{
    CarryRollInput, YieldCurveBinding, YieldCurveInterpolation, YieldCurveNode,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeCarryRollEngine;
use ficant_storage::carry_arrow::ArrowCarryRollCodec;
use ficant_storage::s3::S3BlobStore;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn real_postgres_ceph_publish_restart_replay_and_tamper_fail_closed() {
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;

    let input = carry_input();
    let scope = support::access_scope(input.owner());
    let artifact_id = id('H');
    let repository = support::repository(pool.clone());
    let store = S3BlobStore::new(
        &endpoint,
        bucket.clone(),
        &access_key,
        &secret_key,
        pool.clone(),
    )
    .expect("Phase 2B Ceph adapter must initialize");
    seed_lineage(&pool, &store, &scope, &input).await;
    let artifact = PublishCarryRoll::new(
        &NativeCarryRollEngine,
        &ArrowCarryRollCodec,
        &store,
        &repository,
    )
    .execute(
        scope.clone(),
        artifact_id.clone(),
        &input,
        IdempotencyKey::new("phase2b-carry-roll-publish-v1").unwrap(),
    )
    .await
    .expect("real stage, verification, lineage and publication must succeed");
    assert_eq!(artifact.id(), &artifact_id);
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
    let replay = ReplayCarryRoll::new(
        &NativeCarryRollEngine,
        &ArrowCarryRollCodec,
        &restarted_repository,
        &restarted_store,
        &sink,
    )
    .execute(
        &scope,
        artifact_id.clone(),
        &input,
        SafeTraceContext::new("0123456789abcdef0123456789abcdef").unwrap(),
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
    let error = ReplayCarryRoll::new(
        &NativeCarryRollEngine,
        &ArrowCarryRollCodec,
        &restarted_repository,
        &restarted_store,
        &sink,
    )
    .execute(
        &scope,
        artifact_id,
        &input,
        SafeTraceContext::new("fedcba9876543210fedcba9876543210").unwrap(),
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
        .expect("staging count must be readable");
    let candidate_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM storage.orphan_candidates")
        .fetch_one(&pool)
        .await
        .expect("orphan candidate count must be readable");
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

fn carry_input() -> CarryRollInput {
    let valuation_date = date(2026, 7, 19);
    let issue = date(2026, 1, 1);
    let version = Version::new(1).unwrap();
    let market_time = MarketTime::new(
        Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap(),
        "Asia/Shanghai",
        valuation_date,
    )
    .unwrap();
    let calendar = CalendarBinding::new(
        "phase2b-weekend-calendar-v1",
        version,
        ContentHash::digest(b"phase2b-weekend-calendar-v1"),
        issue,
        date(2031, 1, 10),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let terms = BondTerms::new(
        issue,
        date(2029, 1, 1),
        CouponFrequency::Annual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        fixed(20_000_000_000),
        fixed(100_000_000_000_000),
    )
    .unwrap();
    let curve = YieldCurveBinding::new(
        object('F'),
        valuation_date,
        YieldCurveInterpolation::LinearYield,
        vec![
            YieldCurveNode::new(date(2027, 1, 1), fixed(12_500_000_000)).unwrap(),
            YieldCurveNode::new(date(2027, 7, 20), fixed(17_500_000_000)).unwrap(),
            YieldCurveNode::new(date(2028, 1, 1), fixed(19_000_000_000)).unwrap(),
            YieldCurveNode::new(date(2029, 1, 1), fixed(22_500_000_000)).unwrap(),
            YieldCurveNode::new(date(2030, 7, 19), fixed(30_000_000_000)).unwrap(),
        ],
    )
    .unwrap();
    CarryRollInput::new(
        OwnerRef::new(id('A'), id('B')),
        object('C'),
        object('D'),
        object('E'),
        market_time,
        date(2026, 7, 20),
        date(2027, 1, 2),
        CalendarRequirement::ExactMarket,
        calendar,
        terms,
        curve,
    )
    .unwrap()
}

#[allow(clippy::too_many_lines)]
async fn seed_lineage(
    pool: &sqlx::PgPool,
    store: &S3BlobStore,
    scope: &AccessScope,
    input: &CarryRollInput,
) {
    let tenant = input.owner().tenant_id().as_str();
    let owner = input.owner().owner_id().as_str();
    let unit_id = id('K');
    let calendar_id = id('J');
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
    .expect("SIT unit prerequisite must persist");
    sqlx::query(
        "INSERT INTO market.calendars
         (tenant_id, calendar_id, version, owner_id, market, market_timezone,
          effective_from, effective_to, payload)
         VALUES ($1, $2, 1, $3, 'CIBM', 'Asia/Shanghai',
                 '2026-01-01T00:00:00Z', '2031-01-11T00:00:00Z', decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(calendar_id.as_str())
    .bind(owner)
    .execute(pool)
    .await
    .expect("SIT calendar prerequisite must persist");
    sqlx::query(
        "INSERT INTO market.instruments
         (tenant_id, instrument_id, version, owner_id, kind, market, symbol,
          currency_unit_id, currency_unit_version, calendar_id, calendar_version, payload)
         VALUES ($1, $2, 1, $3, 'BOND', 'CIBM', 'PHASE2B.CGB', $4, 1, $5, 1,
                 decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(input.bond().version_ref().id().as_str())
    .bind(owner)
    .bind(unit_id.as_str())
    .bind(calendar_id.as_str())
    .execute(pool)
    .await
    .expect("SIT bond prerequisite must persist");
    sqlx::query(
        "INSERT INTO market.market_rule_packs
         (tenant_id, rule_pack_id, version, owner_id, market, rule_type, source,
          effective_from, effective_to, verification_status, content_hash, payload)
         VALUES ($1, $2, 1, $3, 'CIBM', 'phase2b-carry-roll', 'phase2b-fixture',
                 '2026-01-01T00:00:00Z', '2031-01-11T00:00:00Z', 'VERIFIED', $4,
                 decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(input.rule_pack().version_ref().id().as_str())
    .bind(owner)
    .bind(S3BlobStore::hash_hex(input.rule_pack().content_hash()))
    .execute(pool)
    .await
    .expect("SIT RulePack prerequisite must persist");

    persist_blob(
        pool,
        store,
        scope,
        input.snapshot().content_hash(),
        b"E".to_vec(),
        "phase2b-source-snapshot-stage-v1",
    )
    .await;
    let snapshot_hash = S3BlobStore::hash_hex(input.snapshot().content_hash());
    sqlx::query(
        "INSERT INTO research.data_snapshots
         (tenant_id, data_snapshot_id, owner_id, visible_at, as_of, schema_hash,
          manifest_hash, content_hash, idempotency_key, fingerprint, payload)
         VALUES ($1, $2, $3, '2026-07-19T01:00:00Z', '2026-07-19T00:00:00Z',
                 $4, $5, $5, 'phase2b-source-snapshot-v1', $6, decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(input.snapshot().version_ref().id().as_str())
    .bind(owner)
    .bind(S3BlobStore::hash_hex(&ContentHash::digest(
        b"phase2b-schema",
    )))
    .bind(&snapshot_hash)
    .bind(vec![7_u8; 32])
    .execute(pool)
    .await
    .expect("SIT data snapshot prerequisite must persist");

    let curve = input.curve().curve_snapshot();
    persist_blob(
        pool,
        store,
        scope,
        curve.content_hash(),
        b"F".to_vec(),
        "phase2b-curve-snapshot-stage-v1",
    )
    .await;
    sqlx::query(
        "INSERT INTO market.curve_snapshots
         (tenant_id, curve_snapshot_id, owner_id, as_of, currency_unit_id,
          currency_unit_version, curve_kind, calendar_id, calendar_version,
          rule_pack_id, rule_pack_version, point_schema, content_hash, blob_size,
          idempotency_key, fingerprint, payload)
         VALUES ($1, $2, $3, '2026-07-19T00:00:00Z', $4, 1,
                 'CFETS_YTM_LINEAR', $5, 1, $6, 1, 'ficant.yield-curve-node.v1',
                 $7, 1, 'phase2b-curve-snapshot-v1', $8, decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(curve.version_ref().id().as_str())
    .bind(owner)
    .bind(unit_id.as_str())
    .bind(calendar_id.as_str())
    .bind(input.rule_pack().version_ref().id().as_str())
    .bind(S3BlobStore::hash_hex(curve.content_hash()))
    .bind(vec![8_u8; 32])
    .execute(pool)
    .await
    .expect("SIT curve snapshot prerequisite must persist");
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
        .expect("SIT lineage blob staging must begin");
    store
        .append_chunk(scope, &staged, bytes.clone())
        .await
        .expect("SIT lineage blob bytes must stage");
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
        .expect("SIT lineage blob must verify");
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
    .expect("SIT lineage blob must become durable");
    sqlx::query("DELETE FROM storage.orphan_candidates WHERE content_hash = $1")
        .bind(&hash)
        .execute(pool)
        .await
        .expect("SIT lineage orphan candidate must finalize");
}

fn scope_owner(scope: &AccessScope) -> OwnerRef {
    OwnerRef::new(
        scope.tenant_id().clone(),
        scope
            .allowed_owner_ids()
            .first()
            .expect("SIT scope has one owner")
            .clone(),
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

fn fixed(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value)
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}
