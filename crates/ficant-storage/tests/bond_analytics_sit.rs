mod support;

use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, BeginBlobStage, BlobStore, IdempotencyKey, IntegrityEvent,
    IntegrityEventSink, IntegrityFailureReason, SafeTraceContext, VerifyBlobStage,
};
use ficant_application::{ApplicationErrorCategory, PublishBondAnalytics, ReplayBondAnalytics};
use ficant_domain::ContentAddressed;
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondTerms, BusinessDayConvention,
    CalendarBinding, CalendarRequirement, CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_fixed_income_native::NativeBondAnalyticsEngine;
use ficant_storage::analytics_arrow::ArrowBondAnalyticsCodec;
use ficant_storage::s3::S3BlobStore;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn q035_q036_real_postgres_s3_publish_restart_replay_and_tamper_fail_closed() {
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;

    let input = analytics_input();
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
    .expect("SIT S3 adapter must initialize");
    seed_analytics_lineage(&pool, &store, &scope, &input).await;
    let artifact = PublishBondAnalytics::new(
        &NativeBondAnalyticsEngine,
        &ArrowBondAnalyticsCodec,
        &store,
        &repository,
    )
    .execute(
        scope.clone(),
        artifact_id.clone(),
        &input,
        IdempotencyKey::new("i3-3c-real-analytics-publish-v1").unwrap(),
    )
    .await
    .expect("real stage, verify and publication must succeed");
    assert_eq!(artifact.id(), &artifact_id);
    assert_eq!(
        store
            .probe_verified(artifact.content_hash())
            .await
            .expect("published object must be readable")
            .expect("published object must exist")
            .len(),
        usize::try_from(artifact.blob_size()).unwrap()
    );

    drop(store);
    drop(repository);
    let restarted_repository = support::repository(pool.clone());
    let restarted_store =
        S3BlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool.clone())
            .expect("restarted S3 adapter must initialize");
    let sink = RecordingSink::default();
    let replay = ReplayBondAnalytics::new(
        &NativeBondAnalyticsEngine,
        &ArrowBondAnalyticsCodec,
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
    .expect("restart-safe replay must reproduce exact canonical content");
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
    .expect("SIT tamper injection must update exactly the isolated row");
    let tampered = ReplayBondAnalytics::new(
        &NativeBondAnalyticsEngine,
        &ArrowBondAnalyticsCodec,
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
    assert_eq!(tampered.category(), ApplicationErrorCategory::HashMismatch);
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
    assert_eq!(
        staging_count, 0,
        "successful publication must clean private staging rows"
    );
    assert_eq!(
        candidate_count, 0,
        "successful publication must finalize orphan candidates"
    );
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

fn analytics_input() -> BondAnalyticsInput {
    let version = Version::new(1).unwrap();
    let owner = OwnerRef::new(id('A'), id('B'));
    let reference =
        |suffix, hash| AnalyticsObjectRef::new(VersionRef::new(id(suffix), version), hash);
    let valuation_at = MarketTime::new(
        Utc.with_ymd_and_hms(2026, 7, 13, 7, 0, 0).unwrap(),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 7, 13).unwrap(),
    )
    .unwrap();
    let calendar = CalendarBinding::new(
        "cgb-reference-calendar-v1",
        version,
        ContentHash::from_bytes(&[4; 32]).unwrap(),
        NaiveDate::from_ymd_opt(2005, 1, 1).unwrap(),
        NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let terms = BondTerms::new(
        NaiveDate::from_ymd_opt(2026, 6, 25).unwrap(),
        NaiveDate::from_ymd_opt(2028, 6, 25).unwrap(),
        CouponFrequency::Annual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        FixedDecimal::from_scaled(12_100_000_000),
        FixedDecimal::from_scaled(100_000_000_000_000),
    )
    .unwrap();
    BondAnalyticsInput::new(
        owner,
        reference('C', ContentHash::from_bytes(&[1; 32]).unwrap()),
        reference('D', ContentHash::from_bytes(&[2; 32]).unwrap()),
        reference('E', ContentHash::digest(b"i3c-analytics-source-snapshot")),
        valuation_at,
        NaiveDate::from_ymd_opt(2026, 7, 14).unwrap(),
        CalendarRequirement::ReferenceReplay,
        calendar,
        terms,
        AnalyticsMode::YieldIn,
        FixedDecimal::from_scaled(13_000_000_000),
    )
    .unwrap()
}

#[allow(clippy::too_many_lines)]
async fn seed_analytics_lineage(
    pool: &sqlx::PgPool,
    store: &S3BlobStore,
    scope: &AccessScope,
    input: &BondAnalyticsInput,
) {
    let tenant = input.owner().tenant_id().as_str();
    let owner = input.owner().owner_id().as_str();
    let unit_id = id('F');
    let calendar_id = id('G');
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
                 '2005-01-01T00:00:00Z', '2027-01-01T00:00:00Z', decode('01', 'hex'))",
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
         VALUES ($1, $2, 1, $3, 'BOND', 'CIBM', '260013.IB', $4, 1, $5, 1,
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
         VALUES ($1, $2, 1, $3, 'CIBM', 'cgb-reference', 'i3c-fixture',
                 '2005-01-01T00:00:00Z', '2027-01-01T00:00:00Z', 'VERIFIED', $4,
                 decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(input.rule_pack().version_ref().id().as_str())
    .bind(owner)
    .bind(S3BlobStore::hash_hex(input.rule_pack().content_hash()))
    .execute(pool)
    .await
    .expect("SIT RulePack prerequisite must persist");

    let snapshot_bytes = b"i3c-analytics-source-snapshot".to_vec();
    assert_eq!(
        ContentHash::digest(&snapshot_bytes),
        *input.snapshot().content_hash()
    );
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                input.owner().clone(),
                u64::try_from(snapshot_bytes.len()).unwrap(),
                IdempotencyKey::new("i3-3c-source-snapshot-stage-v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect("SIT snapshot staging must begin");
    store
        .append_chunk(scope, &staged, snapshot_bytes.clone())
        .await
        .expect("SIT snapshot bytes must stage");
    let verified = store
        .verify_and_promote(
            VerifyBlobStage::new(
                scope.clone(),
                staged,
                input.snapshot().content_hash().clone(),
                u64::try_from(snapshot_bytes.len()).unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect("SIT snapshot bytes must verify");
    let snapshot_hash = S3BlobStore::hash_hex(verified.content_hash());
    sqlx::query(
        "INSERT INTO storage.blobs (tenant_id, content_hash, object_key, blob_size)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant)
    .bind(&snapshot_hash)
    .bind(S3BlobStore::immutable_key(verified.content_hash()))
    .bind(i64::try_from(verified.size()).unwrap())
    .execute(pool)
    .await
    .expect("SIT snapshot verified blob must become durable");
    sqlx::query("DELETE FROM storage.orphan_candidates WHERE content_hash = $1")
        .bind(&snapshot_hash)
        .execute(pool)
        .await
        .expect("SIT snapshot candidate must finalize");
    sqlx::query(
        "INSERT INTO research.data_snapshots
         (tenant_id, data_snapshot_id, owner_id, visible_at, as_of, schema_hash,
          manifest_hash, content_hash, idempotency_key, fingerprint, payload)
         VALUES ($1, $2, $3, '2026-07-13T08:00:00Z', '2026-07-13T07:00:00Z',
                 $4, $5, $5, 'i3-3c-source-snapshot-publish-v1', $6, decode('01', 'hex'))",
    )
    .bind(tenant)
    .bind(input.snapshot().version_ref().id().as_str())
    .bind(owner)
    .bind(S3BlobStore::hash_hex(&ContentHash::digest(b"i3c-schema")))
    .bind(&snapshot_hash)
    .bind(vec![7_u8; 32])
    .execute(pool)
    .await
    .expect("SIT data snapshot prerequisite must persist");
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
