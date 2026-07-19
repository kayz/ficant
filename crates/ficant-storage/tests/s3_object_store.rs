mod support;

use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    AppendDefinitionVersion, BeginBlobStage, BlobStore, DefinitionIdentity, DefinitionKind,
    DefinitionRepository, DefinitionValue, IdempotencyKey, PublishSnapshot, SnapshotBlobRole,
    SnapshotValue, StagedSnapshotBlob, VerifiedSnapshotBlob, VerifiedSnapshotProof,
    VerifyBlobStage,
};
use ficant_domain::market::{Unit, UnitInput};
use ficant_domain::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version};
use ficant_domain::research::{DataSnapshot, DataSnapshotInput};
use ficant_storage::s3::{OrphanCleaner, S3BlobStore};
use sqlx::types::chrono::{NaiveDate, TimeZone, Utc};

#[tokio::test]
async fn s3_stages_verifies_and_promotes_server_checked_content() {
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let store = S3BlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool)
        .expect("S3 adapter configuration must be valid");
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let scope = support::access_scope(&owner);
    let bytes = b"ficant-content-addressed-artifact".to_vec();
    let expected_hash = ContentHash::digest(&bytes);
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner,
                u64::try_from(bytes.len()).unwrap(),
                IdempotencyKey::new("s3-object-store:stage:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect("staging must begin");
    store
        .append_chunk(&scope, &staged, bytes.clone())
        .await
        .expect("staging chunk must persist");
    let verified = store
        .verify_and_promote(
            VerifyBlobStage::new(
                scope,
                staged,
                expected_hash.clone(),
                u64::try_from(bytes.len()).unwrap(),
            )
            .unwrap(),
        )
        .await
        .expect("server-verified content must promote");
    assert_eq!(verified.content_hash(), &expected_hash);
    assert_eq!(verified.size(), u64::try_from(bytes.len()).unwrap());
    assert_eq!(
        store.probe_verified(&expected_hash).await.unwrap(),
        Some(bytes)
    );
}

#[tokio::test]
// One scenario keeps both the public publication and cleanup lifecycle visible end to end.
#[allow(clippy::too_many_lines)]
async fn orphan_cleanup_deletes_only_unreferenced_content() {
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let store =
        S3BlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool.clone()).unwrap();
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let scope = support::access_scope(&owner);

    let orphan_bytes = b"unreferenced-orphan".to_vec();
    let orphan_hash = ContentHash::digest(&orphan_bytes);
    let orphan = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner.clone(),
                u64::try_from(orphan_bytes.len()).unwrap(),
                IdempotencyKey::new("s3-orphan:stage:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(&scope, &orphan, orphan_bytes.clone())
        .await
        .unwrap();
    store
        .verify_and_promote(
            VerifyBlobStage::new(
                scope.clone(),
                orphan,
                orphan_hash.clone(),
                u64::try_from(orphan_bytes.len()).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let protected_bytes = b"referenced-content".to_vec();
    let protected_hash = ContentHash::digest(&protected_bytes);
    let protected_manifest_bytes = b"referenced-manifest".to_vec();
    let protected_manifest_hash = ContentHash::digest(&protected_manifest_bytes);
    let protected = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner.clone(),
                u64::try_from(protected_bytes.len()).unwrap(),
                IdempotencyKey::new("s3-protected:stage:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(&scope, &protected, protected_bytes.clone())
        .await
        .unwrap();
    let protected_verification = VerifyBlobStage::new(
        scope.clone(),
        protected,
        protected_hash.clone(),
        u64::try_from(protected_bytes.len()).unwrap(),
    )
    .unwrap();
    let protected_parquet = StagedSnapshotBlob::new(
        SnapshotBlobRole::DataParquet,
        protected_verification.clone(),
    );
    let protected_verified = store
        .verify_and_promote(protected_verification)
        .await
        .unwrap();
    let manifest_staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner.clone(),
                u64::try_from(protected_manifest_bytes.len()).unwrap(),
                IdempotencyKey::new("s3-protected:manifest-stage:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(&scope, &manifest_staged, protected_manifest_bytes.clone())
        .await
        .unwrap();
    let manifest_verification = VerifyBlobStage::new(
        scope,
        manifest_staged,
        protected_manifest_hash.clone(),
        u64::try_from(protected_manifest_bytes.len()).unwrap(),
    )
    .unwrap();
    let protected_manifest = StagedSnapshotBlob::new(
        SnapshotBlobRole::DataManifest,
        manifest_verification.clone(),
    );
    let protected_manifest_verified = store
        .verify_and_promote(manifest_verification)
        .await
        .unwrap();
    let repository = support::repository(pool.clone());
    let lineage_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F03").unwrap();
    repository
        .create_identity(DefinitionIdentity::new(
            lineage_id.clone(),
            owner.clone(),
            DefinitionKind::Unit,
            IdempotencyKey::new("s3-protected:lineage:identity").unwrap(),
        ))
        .await
        .unwrap();
    repository
        .append_version(
            AppendDefinitionVersion::new(
                None,
                DefinitionValue::Unit(
                    Unit::new(UnitInput {
                        unit_id: lineage_id.clone(),
                        version: Version::new(1).unwrap(),
                        owner: owner.clone(),
                        code: "CNY".to_owned(),
                        dimension: "currency".to_owned(),
                        scale: 2,
                        precision: 18,
                    })
                    .unwrap(),
                ),
                IdempotencyKey::new("s3-protected:lineage:v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let market_time = |hour| {
        MarketTime::new(
            Utc.with_ymd_and_hms(2025, 1, 15, hour, 0, 0).unwrap(),
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
        )
        .unwrap()
    };
    let snapshot = SnapshotValue::Data(
        DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F30").unwrap(),
            owner: owner.clone(),
            visible_at: market_time(8),
            as_of: market_time(7),
            schema_hash: ContentHash::digest(b"protected-schema"),
            manifest_hash: protected_manifest_hash,
            blob_content_hash: protected_hash.clone(),
            lineage: vec![LineageRef::versioned(lineage_id, Version::new(1).unwrap())],
        })
        .unwrap(),
    );
    repository
        .publish_verified_manifest(
            PublishSnapshot::new(
                snapshot,
                VerifiedSnapshotProof::data(
                    VerifiedSnapshotBlob::from_staged(
                        protected_parquet,
                        protected_verified.clone(),
                    )
                    .unwrap(),
                    VerifiedSnapshotBlob::from_staged(
                        protected_manifest,
                        protected_manifest_verified,
                    )
                    .unwrap(),
                )
                .unwrap(),
                IdempotencyKey::new("s3-protected:snapshot:publish").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let cleaner = OrphanCleaner::new(store.clone(), pool);
    // 2100-01-01T00:00:00Z: safely after the test rows without exceeding
    // PostgreSQL's timestamp range.
    let report = cleaner.cleanup_before(4_102_444_800).await.unwrap();
    assert!(report.deleted_immutable() >= 1);
    assert_eq!(store.probe_verified(&orphan_hash).await.unwrap(), None);
    assert_eq!(
        store.probe_verified(&protected_hash).await.unwrap(),
        Some(protected_bytes)
    );
}

#[tokio::test]
async fn staging_rejects_idempotency_drift_and_oversized_chunks() {
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let store = S3BlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool).unwrap();
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let scope = support::access_scope(&owner);
    let idempotency = IdempotencyKey::new("s3-stage:drift:v1").unwrap();
    let staged = store
        .begin_stage(
            BeginBlobStage::new(scope.clone(), owner.clone(), 3, idempotency.clone()).unwrap(),
        )
        .await
        .unwrap();

    let drift = store
        .begin_stage(BeginBlobStage::new(scope.clone(), owner, 4, idempotency).unwrap())
        .await
        .unwrap_err();
    assert_eq!(drift.category(), ApplicationErrorCategory::AlreadyExists);
    let oversized = store
        .append_chunk(&scope, &staged, b"four".to_vec())
        .await
        .unwrap_err();
    assert_eq!(
        oversized.category(),
        ApplicationErrorCategory::ValidationFailed
    );

    store
        .append_chunk(&scope, &staged, b"yes".to_vec())
        .await
        .unwrap();
    let hash = ContentHash::digest(b"yes");
    store
        .verify_and_promote(VerifyBlobStage::new(scope, staged, hash.clone(), 3).unwrap())
        .await
        .unwrap();
    assert_eq!(
        store.probe_verified(&hash).await.unwrap(),
        Some(b"yes".to_vec())
    );
}

#[tokio::test]
async fn candidate_registration_failure_never_creates_untracked_immutable_object() {
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let store =
        S3BlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool.clone()).unwrap();
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let scope = support::access_scope(&owner);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let bytes = format!("candidate-registration-must-precede-object-write:{nonce}").into_bytes();
    let hash = ContentHash::digest(&bytes);
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner,
                u64::try_from(bytes.len()).unwrap(),
                IdempotencyKey::new(format!("s3-candidate-failure:stage:v1:{nonce}")).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let cleanup_staged = staged.clone();
    store
        .append_chunk(&scope, &staged, bytes.clone())
        .await
        .unwrap();
    sqlx::query("DROP TABLE storage.orphan_candidates")
        .execute(&pool)
        .await
        .unwrap();

    let failure = store
        .verify_and_promote(
            VerifyBlobStage::new(
                scope,
                staged,
                hash.clone(),
                u64::try_from(bytes.len()).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.category(),
        ApplicationErrorCategory::StorageUnavailable
    );
    assert_eq!(store.probe_verified(&hash).await.unwrap(), None);
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    store
        .discard_stage(
            &support::access_scope(cleanup_staged.owner()),
            &cleanup_staged,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn finalize_failure_remains_recoverable_after_adapter_restart() {
    let (endpoint, bucket, access_key, secret_key) = support::s3_environment();
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let store = S3BlobStore::new(
        &endpoint,
        bucket.clone(),
        &access_key,
        &secret_key,
        pool.clone(),
    )
    .unwrap();
    let owner = OwnerRef::new(
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap(),
        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
    );
    let scope = support::access_scope(&owner);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let bytes = format!("recoverable-finalize-failure:{nonce}").into_bytes();
    let hash = ContentHash::digest(&bytes);
    let staged = store
        .begin_stage(
            BeginBlobStage::new(
                scope.clone(),
                owner,
                u64::try_from(bytes.len()).unwrap(),
                IdempotencyKey::new(format!("s3-finalize-failure:stage:v1:{nonce}")).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .append_chunk(&scope, &staged, bytes.clone())
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION storage.reject_staging_delete() RETURNS trigger
         LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected finalize failure'; END $$;
         CREATE TRIGGER reject_staging_delete
         BEFORE DELETE ON storage.staging_uploads
         FOR EACH ROW EXECUTE FUNCTION storage.reject_staging_delete();",
    )
    .execute(&pool)
    .await
    .unwrap();

    let failure = store
        .verify_and_promote(
            VerifyBlobStage::new(
                scope,
                staged,
                hash.clone(),
                u64::try_from(bytes.len()).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.category(),
        ApplicationErrorCategory::StorageUnavailable
    );
    assert_eq!(store.probe_verified(&hash).await.unwrap(), Some(bytes));
    let candidate_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM storage.orphan_candidates WHERE content_hash = $1",
    )
    .bind(S3BlobStore::hash_hex(&hash))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(candidate_count, 1);

    sqlx::raw_sql(
        "DROP TRIGGER reject_staging_delete ON storage.staging_uploads;
         DROP FUNCTION storage.reject_staging_delete();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let restarted =
        S3BlobStore::new(&endpoint, bucket, &access_key, &secret_key, pool.clone()).unwrap();
    let cleaner = OrphanCleaner::new(restarted.clone(), pool);
    let report = cleaner.cleanup_before(4_102_444_800).await.unwrap();
    assert_eq!(report.deleted_immutable(), 1);
    assert_eq!(restarted.probe_verified(&hash).await.unwrap(), None);
}
