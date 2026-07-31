mod support;

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    PositionSnapshotRepository, PublishSnapshot, SnapshotBlobRole, SnapshotValue, StagedBlobRef,
    StagedSnapshotBlob, VerifiedBlobRef, VerifiedSnapshotBlob, VerifiedSnapshotProof,
    VerifyBlobStage,
};
use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    AccountingBook, AccountingClassification, AccountingClassificationState, Position,
    PositionHoldingForm, PositionInput, PositionSnapshot, PositionSnapshotInput,
};

#[tokio::test]
async fn position_snapshot_reads_are_scoped_and_resolve_the_latest_visible_revision() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = OwnerRef::new(id('T'), id('N'));
    let subject = VersionRef::new(id('S'), Version::new(1).unwrap());
    sqlx::query(
        "INSERT INTO core.subject_versions (subject_id, version, display_name, market_codes, tool_codes, funding_tier, value_added_tax_profile, income_tax_profile, assessment_mechanism, liability_profile) VALUES ($1, $2, 'Position test', ARRAY['CN'], ARRAY['positions'], 'DR_AVAILABLE', '', '', 'test', 'test')",
    )
    .bind(subject.id().as_str())
    .bind(1_i64)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.units (tenant_id, unit_id, version, owner_id, code, dimension, scale, precision, payload) VALUES ($1, $2, 1, $3, 'LINEAGE', 'test', 0, 1, '\\x01')",
    )
    .bind(owner.tenant_id().as_str())
    .bind(id('K').as_str())
    .bind(owner.owner_id().as_str())
    .execute(&pool)
    .await
    .unwrap();

    let first = snapshot(id('A'), owner.clone(), subject.clone(), 8, 9, "100");
    let revised = snapshot(id('B'), owner.clone(), subject.clone(), 8, 11, "125");
    publish(&pool, &repository, &owner, first.clone(), "position:first").await;
    publish(
        &pool,
        &repository,
        &owner,
        revised.clone(),
        "position:revised",
    )
    .await;

    let scope = support::access_scope(&owner);
    assert_eq!(
        repository
            .get_position_snapshot(&scope, first.id().clone(), market_time(10))
            .await
            .unwrap(),
        Some(first.clone())
    );
    assert_eq!(
        repository
            .resolve_position_snapshot(&scope, subject, market_time(8), market_time(10))
            .await
            .unwrap(),
        Some(first)
    );
    assert_eq!(
        repository
            .resolve_position_snapshot(
                &scope,
                VersionRef::new(id('S'), Version::new(1).unwrap()),
                market_time(8),
                market_time(12),
            )
            .await
            .unwrap(),
        Some(revised)
    );
}

async fn publish(
    pool: &sqlx::PgPool,
    repository: &ficant_storage::postgres::PostgresRepository,
    owner: &OwnerRef,
    snapshot: PositionSnapshot,
    idempotency_key: &str,
) {
    let scope = support::access_scope(owner);
    let payload = snapshot.canonical_payload();
    let hash = snapshot.content_hash().clone();
    assert_eq!(ContentHash::digest(&payload), hash);
    let hash_text = hex(&hash);
    sqlx::query(
        "INSERT INTO storage.blobs (tenant_id, content_hash, object_key, blob_size) VALUES ($1, $2, $3, $4)",
    )
    .bind(owner.tenant_id().as_str())
    .bind(&hash_text)
    .bind(format!("immutable/{hash_text}"))
    .bind(i64::try_from(payload.len()).unwrap())
    .execute(pool)
    .await
    .unwrap();
    let staged = StagedSnapshotBlob::new(
        SnapshotBlobRole::PositionPayload,
        VerifyBlobStage::new(
            scope.clone(),
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
    let command = PublishSnapshot::new(
        SnapshotValue::Position(snapshot.clone()),
        VerifiedSnapshotProof::position(verified).unwrap(),
        ficant_application::ports::IdempotencyKey::new(idempotency_key).unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository.publish_verified_manifest(command).await.unwrap(),
        SnapshotValue::Position(snapshot)
    );
}

fn snapshot(
    snapshot_id: Ulid,
    owner: OwnerRef,
    subject_ref: VersionRef,
    observed_hour: u32,
    visible_hour: u32,
    value: &str,
) -> PositionSnapshot {
    let classification = AccountingClassification::new(
        AccountingClassificationState::Classified,
        Some(AccountingBook::Ac),
    )
    .unwrap();
    let unit = UnitRef::new(id('V'), Version::new(1).unwrap());
    let position = Position::new(PositionInput {
        position_id: id('P'),
        instrument_ref: VersionRef::new(id('J'), Version::new(1).unwrap()),
        quantity: DecimalValue::new("1", 0, unit.clone()).unwrap(),
        economic_value: DecimalValue::new(value, 0, unit.clone()).unwrap(),
        economic_pnl: DecimalValue::new("0", 0, unit.clone()).unwrap(),
        accounting_pnl: DecimalValue::new("0", 0, unit.clone()).unwrap(),
        capital_requirement: DecimalValue::new("10", 0, unit).unwrap(),
        accounting_classification: classification,
        holding_form: PositionHoldingForm::Owned,
    })
    .unwrap();
    let mut input = PositionSnapshotInput {
        snapshot_id,
        owner,
        subject_ref,
        observed_at: market_time(observed_hour),
        visible_at: market_time(visible_hour),
        content_hash: ContentHash::digest(b"placeholder"),
        lineage: vec![LineageRef::versioned(id('K'), Version::new(1).unwrap())],
        positions: vec![position],
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    PositionSnapshot::new(input).unwrap()
}

fn market_time(hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 7, 31, hour, 0, 0).unwrap(),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(),
    )
    .unwrap()
}

fn id(suffix: char) -> Ulid {
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
