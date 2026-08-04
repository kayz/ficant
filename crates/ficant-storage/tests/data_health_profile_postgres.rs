mod support;

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    DataHealthThresholdProfileRepository, IdempotencyKey, PublishSnapshot, SnapshotBlobRole,
    SnapshotValue, StagedBlobRef, StagedSnapshotBlob, VerifiedBlobRef, VerifiedSnapshotBlob,
    VerifiedSnapshotProof, VerifyBlobStage,
};
use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_domain::research::{DataHealthThresholdProfile, DataHealthThresholdProfileInput};

#[tokio::test]
async fn threshold_profiles_are_immutable_verified_and_active_by_owner_and_time() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = OwnerRef::new(id('T'), id('O'));
    let scope = support::access_scope(&owner);

    let first = profile(id('A'), owner.clone(), 1, 1, 10, 100);
    publish(&pool, &repository, &owner, first.clone(), "health:first").await;
    assert_eq!(
        repository
            .get_exact(&scope, first.profile_ref().clone(), time(2))
            .await
            .unwrap(),
        Some(first.clone())
    );
    assert_eq!(
        repository
            .resolve_active(&scope, owner.clone(), time(2))
            .await
            .unwrap(),
        Some(first.clone())
    );

    let conflicting_same_version = profile(id('B'), owner.clone(), 1, 1, 10, 101);
    assert!(
        try_publish(
            &pool,
            &repository,
            &owner,
            conflicting_same_version,
            "health:conflict"
        )
        .await
        .is_err(),
        "one VersionRef cannot identify multiple threshold contents"
    );

    let overlapping = profile(id('C'), owner.clone(), 2, 1, 10, 200);
    publish(&pool, &repository, &owner, overlapping, "health:overlap").await;
    assert!(
        repository
            .resolve_active(&scope, owner, time(2))
            .await
            .is_err(),
        "multiple active platform profiles must fail instead of guessing"
    );
}

async fn publish(
    pool: &sqlx::PgPool,
    repository: &ficant_storage::postgres::PostgresRepository,
    owner: &OwnerRef,
    profile: DataHealthThresholdProfile,
    idempotency_key: &str,
) {
    try_publish(pool, repository, owner, profile, idempotency_key)
        .await
        .unwrap();
}

async fn try_publish(
    pool: &sqlx::PgPool,
    repository: &ficant_storage::postgres::PostgresRepository,
    owner: &OwnerRef,
    profile: DataHealthThresholdProfile,
    idempotency_key: &str,
) -> ficant_application::ports::ApplicationResult<SnapshotValue> {
    let scope = support::access_scope(owner);
    let payload = profile.canonical_bytes();
    let hash = profile.content_hash().clone();
    assert_eq!(ContentHash::digest(&payload), hash);
    let hash_text = hex(&hash);
    sqlx::query(
        "INSERT INTO storage.blobs (tenant_id, content_hash, object_key, blob_size)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (tenant_id, content_hash) DO NOTHING",
    )
    .bind(owner.tenant_id().as_str())
    .bind(&hash_text)
    .bind(format!("immutable/{hash_text}"))
    .bind(i64::try_from(payload.len()).unwrap())
    .execute(pool)
    .await
    .unwrap();
    let staged = StagedSnapshotBlob::new(
        SnapshotBlobRole::DataHealthThresholdProfilePayload,
        VerifyBlobStage::new(
            scope,
            StagedBlobRef::new(id('Z'), owner.clone()),
            hash.clone(),
            u64::try_from(payload.len()).unwrap(),
        )
        .unwrap(),
    );
    let verified = VerifiedSnapshotBlob::from_staged(
        staged,
        VerifiedBlobRef::new(hash, u64::try_from(payload.len()).unwrap()).unwrap(),
    )
    .unwrap();
    repository
        .publish_verified_manifest(
            PublishSnapshot::new(
                SnapshotValue::DataHealthThresholdProfile(profile),
                VerifiedSnapshotProof::data_health_threshold_profile(verified).unwrap(),
                IdempotencyKey::new(idempotency_key).unwrap(),
            )
            .unwrap(),
        )
        .await
}

fn profile(
    snapshot_id: Ulid,
    owner: OwnerRef,
    profile_version: u64,
    effective_from_hour: u32,
    effective_to_hour: u32,
    max_age: u64,
) -> DataHealthThresholdProfile {
    let mut input = DataHealthThresholdProfileInput {
        profile_snapshot_id: snapshot_id,
        owner,
        profile_ref: VersionRef::new(id('P'), Version::new(profile_version).unwrap()),
        visible_at: time(0),
        effective_from: time(effective_from_hour),
        effective_to: time(effective_to_hour),
        max_position_snapshot_age_seconds: max_age,
        unknown_accounting_warning_basis_points: 5_000,
        max_data_snapshot_age_seconds: max_age,
        model_valuation_warning_basis_points: 5_000,
        content_hash: ContentHash::digest(b"pending"),
        lineage: Vec::new(),
    };
    input.content_hash = DataHealthThresholdProfile::content_hash_for(&input);
    DataHealthThresholdProfile::new(input).unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 8, 4, hour, 0, 0).unwrap(),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 8, 4).unwrap(),
    )
    .unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => '1',
        'L' => '2',
        'O' => '3',
        'U' => '4',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn hex(value: &ContentHash) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            use std::fmt::Write as _;
            write!(output, "{byte:02x}").unwrap();
            output
        })
}
