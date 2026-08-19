mod support;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    DataHealthThresholdProfileRepository, IdempotencyKey, PositionSnapshotRepository,
    PublishSnapshot, SnapshotBlobRole, SnapshotValue, StagedBlobRef, StagedSnapshotBlob,
    SubjectRepository, VerifiedBlobRef, VerifiedSnapshotBlob, VerifiedSnapshotProof,
    VerifyBlobStage,
};
use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_domain::research::{
    DataHealthThresholdProfile, DataHealthThresholdProfileInput, PositionSnapshot,
    PositionSnapshotInput,
};

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn every_existing_knowledge_query_honors_early_edge_late_and_tamper_boundaries() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = OwnerRef::new(id('T'), id('O'));
    let scope = support::access_scope(&owner);
    let subject_ref = VersionRef::new(id('S'), Version::new(1).unwrap());
    insert_subject(&pool, &owner, &subject_ref).await;
    insert_lineage_unit(&pool, &owner).await;

    let visible_at = market_time(500_000_000);
    let early = market_time(499_999_999);
    let edge = visible_at.clone();
    let late = market_time(500_000_001);

    let position = position_snapshot(owner.clone(), subject_ref.clone(), visible_at.clone());
    publish_position(&pool, &repository, &owner, position.clone()).await;
    assert_eq!(
        repository
            .get_position_snapshot(&scope, position.id().clone(), early.clone())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        repository
            .resolve_position_snapshot(
                &scope,
                subject_ref.clone(),
                position.observed_at().clone(),
                early.clone(),
            )
            .await
            .unwrap(),
        None
    );
    for knowledge_at in [edge.clone(), late.clone()] {
        assert_eq!(
            repository
                .get_position_snapshot(&scope, position.id().clone(), knowledge_at.clone())
                .await
                .unwrap(),
            Some(position.clone())
        );
        assert_eq!(
            repository
                .resolve_position_snapshot(
                    &scope,
                    subject_ref.clone(),
                    position.observed_at().clone(),
                    knowledge_at,
                )
                .await
                .unwrap(),
            Some(position.clone())
        );
    }
    sqlx::query(
        "UPDATE research.position_snapshots SET visible_at=visible_at-INTERVAL '0.1 second'
         WHERE tenant_id=$1 AND snapshot_id=$2",
    )
    .bind(owner.tenant_id().as_str())
    .bind(position.id().as_str())
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        repository
            .get_position_snapshot(&scope, position.id().clone(), edge.clone())
            .await
            .is_err(),
        "SQL visibility drift must not make a mismatched Position payload readable"
    );
    sqlx::query(
        "UPDATE research.position_snapshots SET visible_at=$3
         WHERE tenant_id=$1 AND snapshot_id=$2",
    )
    .bind(owner.tenant_id().as_str())
    .bind(position.id().as_str())
    .bind(visible_at.instant())
    .execute(&pool)
    .await
    .unwrap();

    let profile = health_profile(owner.clone(), visible_at.clone());
    publish_profile(&pool, &repository, &owner, profile.clone()).await;
    assert_eq!(
        repository
            .get_exact(&scope, profile.profile_ref().clone(), early.clone())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        repository
            .resolve_active(&scope, owner.clone(), early.clone())
            .await
            .unwrap(),
        None
    );
    for knowledge_at in [edge.clone(), late.clone()] {
        assert_eq!(
            repository
                .get_exact(&scope, profile.profile_ref().clone(), knowledge_at.clone(),)
                .await
                .unwrap(),
            Some(profile.clone())
        );
        assert_eq!(
            repository
                .resolve_active(&scope, owner.clone(), knowledge_at)
                .await
                .unwrap(),
            Some(profile.clone())
        );
    }
    sqlx::query(
        "UPDATE research.data_health_threshold_profiles
         SET visible_at=visible_at-INTERVAL '0.1 second'
         WHERE tenant_id=$1 AND profile_snapshot_id=$2",
    )
    .bind(owner.tenant_id().as_str())
    .bind(profile.id().as_str())
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        repository
            .get_exact(&scope, profile.profile_ref().clone(), edge.clone())
            .await
            .is_err(),
        "SQL visibility drift must not make a mismatched DataHealth payload readable"
    );

    let state_id = id('Q');
    insert_subject_state(&pool, &owner, &subject_ref, &state_id, visible_at.instant()).await;
    assert_eq!(
        repository
            .get_subject_state(state_id.clone(), early.instant())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        repository
            .get_subject_state_scoped(&scope, state_id.clone(), early.instant())
            .await
            .unwrap(),
        None
    );
    for knowledge_at in [edge, late] {
        assert!(
            repository
                .get_subject_state(state_id.clone(), knowledge_at.instant())
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            repository
                .get_subject_state_scoped(&scope, state_id.clone(), knowledge_at.instant())
                .await
                .unwrap()
                .is_some()
        );
    }
    sqlx::query(
        "UPDATE core.subject_state_snapshots SET market_timezone='invalid/timezone'
         WHERE tenant_id=$1 AND snapshot_id=$2",
    )
    .bind(owner.tenant_id().as_str())
    .bind(state_id.as_str())
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        repository
            .get_subject_state_scoped(&scope, state_id, visible_at.instant())
            .await
            .is_err(),
        "decoded SubjectState time evidence must fail closed after SQL tamper"
    );
}

async fn insert_subject(pool: &sqlx::PgPool, owner: &OwnerRef, subject: &VersionRef) {
    sqlx::query(
        "INSERT INTO core.subject_identities
         (tenant_id, subject_id, owner_id, latest_version) VALUES ($1,$2,$3,1)",
    )
    .bind(owner.tenant_id().as_str())
    .bind(subject.id().as_str())
    .bind(owner.owner_id().as_str())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO core.subject_versions
         (tenant_id, subject_id, version, owner_id, display_name, market_codes, tool_codes,
          funding_tier, value_added_tax_profile, income_tax_profile, assessment_mechanism,
          liability_profile)
         VALUES ($1,$2,1,$3,'R7A Subject',ARRAY['CGB'],ARRAY['rates'],
                 'DR_AVAILABLE','','','daily','general')",
    )
    .bind(owner.tenant_id().as_str())
    .bind(subject.id().as_str())
    .bind(owner.owner_id().as_str())
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_lineage_unit(pool: &sqlx::PgPool, owner: &OwnerRef) {
    sqlx::query(
        "INSERT INTO market.units
         (tenant_id, unit_id, version, owner_id, code, dimension, scale, precision, payload)
         VALUES ($1,$2,1,$3,'R7A_LINEAGE','test',0,1,'\\x01')",
    )
    .bind(owner.tenant_id().as_str())
    .bind(id('K').as_str())
    .bind(owner.owner_id().as_str())
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_subject_state(
    pool: &sqlx::PgPool,
    owner: &OwnerRef,
    subject: &VersionRef,
    snapshot_id: &Ulid,
    visible_at: DateTime<Utc>,
) {
    let observed_at = base_instant();
    sqlx::query(
        "INSERT INTO core.subject_state_snapshots
         (snapshot_id, subject_id, subject_version, net_capital_coefficient,
          net_capital_scale, net_capital_unit_id, net_capital_unit_version,
          observed_at, visible_at, market_timezone, tenant_id, owner_id)
         VALUES ($1,$2,1,'1000000',2,$3,1,$4,$5,'Asia/Shanghai',$6,$7)",
    )
    .bind(snapshot_id.as_str())
    .bind(subject.id().as_str())
    .bind(id('Y').as_str())
    .bind(observed_at)
    .bind(visible_at)
    .bind(owner.tenant_id().as_str())
    .bind(owner.owner_id().as_str())
    .execute(pool)
    .await
    .unwrap();
}

fn position_snapshot(
    owner: OwnerRef,
    subject_ref: VersionRef,
    visible_at: MarketTime,
) -> PositionSnapshot {
    let mut input = PositionSnapshotInput {
        snapshot_id: id('P'),
        owner,
        subject_ref,
        observed_at: market_time(0),
        visible_at,
        content_hash: ContentHash::digest(b"pending"),
        lineage: vec![LineageRef::versioned(id('K'), Version::new(1).unwrap())],
        positions: Vec::new(),
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    PositionSnapshot::new(input).unwrap()
}

fn health_profile(owner: OwnerRef, visible_at: MarketTime) -> DataHealthThresholdProfile {
    let mut input = DataHealthThresholdProfileInput {
        profile_snapshot_id: id('H'),
        owner,
        profile_ref: VersionRef::new(id('D'), Version::new(1).unwrap()),
        visible_at,
        effective_from: market_time(0),
        effective_to: market_time(900_000_000),
        max_position_snapshot_age_seconds: 300,
        unknown_accounting_warning_basis_points: 5_000,
        max_data_snapshot_age_seconds: 300,
        model_valuation_warning_basis_points: 5_000,
        content_hash: ContentHash::digest(b"pending"),
        lineage: Vec::new(),
    };
    input.content_hash = DataHealthThresholdProfile::content_hash_for(&input);
    DataHealthThresholdProfile::new(input).unwrap()
}

async fn publish_position(
    pool: &sqlx::PgPool,
    repository: &ficant_storage::postgres::PostgresRepository,
    owner: &OwnerRef,
    snapshot: PositionSnapshot,
) {
    let payload = snapshot.canonical_payload();
    let verified = verified_blob(pool, owner, snapshot.content_hash(), &payload, 'A').await;
    let staged = StagedSnapshotBlob::new(
        SnapshotBlobRole::PositionPayload,
        VerifyBlobStage::new(
            support::access_scope(owner),
            StagedBlobRef::new(id('A'), owner.clone()),
            verified.content_hash().clone(),
            verified.size(),
        )
        .unwrap(),
    );
    let proof = VerifiedSnapshotProof::position(
        VerifiedSnapshotBlob::from_staged(staged, verified).unwrap(),
    )
    .unwrap();
    repository
        .publish_verified_manifest(
            PublishSnapshot::new(
                SnapshotValue::Position(snapshot),
                proof,
                IdempotencyKey::new("r7a-position").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn publish_profile(
    pool: &sqlx::PgPool,
    repository: &ficant_storage::postgres::PostgresRepository,
    owner: &OwnerRef,
    profile: DataHealthThresholdProfile,
) {
    let payload = profile.canonical_bytes();
    let verified = verified_blob(pool, owner, profile.content_hash(), &payload, 'B').await;
    let staged = StagedSnapshotBlob::new(
        SnapshotBlobRole::DataHealthThresholdProfilePayload,
        VerifyBlobStage::new(
            support::access_scope(owner),
            StagedBlobRef::new(id('B'), owner.clone()),
            verified.content_hash().clone(),
            verified.size(),
        )
        .unwrap(),
    );
    let proof = VerifiedSnapshotProof::data_health_threshold_profile(
        VerifiedSnapshotBlob::from_staged(staged, verified).unwrap(),
    )
    .unwrap();
    repository
        .publish_verified_manifest(
            PublishSnapshot::new(
                SnapshotValue::DataHealthThresholdProfile(profile),
                proof,
                IdempotencyKey::new("r7a-health").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn verified_blob(
    pool: &sqlx::PgPool,
    owner: &OwnerRef,
    hash: &ContentHash,
    payload: &[u8],
    _suffix: char,
) -> VerifiedBlobRef {
    let hash_text = hex(hash);
    sqlx::query(
        "INSERT INTO storage.blobs (tenant_id, content_hash, object_key, blob_size)
         VALUES ($1,$2,$3,$4)",
    )
    .bind(owner.tenant_id().as_str())
    .bind(&hash_text)
    .bind(format!("immutable/{hash_text}"))
    .bind(i64::try_from(payload.len()).unwrap())
    .execute(pool)
    .await
    .unwrap();
    VerifiedBlobRef::new(hash.clone(), u64::try_from(payload.len()).unwrap()).unwrap()
}

fn market_time(nanos: u32) -> MarketTime {
    let instant = Utc
        .timestamp_opt(base_instant().timestamp(), nanos)
        .single()
        .unwrap();
    MarketTime::new(
        instant,
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 8, 19).unwrap(),
    )
    .unwrap()
}

fn base_instant() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 19, 2, 0, 0).single().unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'O' => '3',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F{suffix}0")).unwrap()
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
