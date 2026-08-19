mod support;

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, AuthorizedPrincipal, CanonicalImportReplayRequest, Cursor,
    DataSourceAuthorizationRepository, DataSourceRepository, FoundationChangeContext,
    FoundationChangeFilter, FoundationChangeRepository, GovernedPublishSnapshot,
    GovernedPublishSubjectState, GovernedRegisterSubject, IdempotencyKey, PageRequest,
    PublishDataSourceAuthorization, RegisterDataSource, SnapshotBlobRole, SnapshotRepository,
    SnapshotValue, StagedBlobRef, StagedSnapshotBlob, SubjectRepository, VerifiedBlobRef,
    VerifiedSnapshotBlob, VerifiedSnapshotProof, VerifyBlobStage, data_source_content_hash,
};
use ficant_application::{ApplicationErrorCategory, ApplicationErrorDetail, GovernedInputUseCase};
use ficant_domain::governance::{ChangeJustification, PlatformRole, SourceDocumentRef};
use ficant_domain::market::{
    DataSource, DataSourceAuthorization, DataSourceAuthorizationInput,
    DataSourceAuthorizationState, DataSourceInput, DataSourceKind, ImportInterface,
    PriceSourceType,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{
    AccountingBook, AccountingClassification, AccountingClassificationState,
    DataHealthThresholdProfile, DataHealthThresholdProfileInput, DataSnapshot, DataSnapshotInput,
    Position, PositionHoldingForm, PositionInput, PositionSnapshot, PositionSnapshotInput,
};
use ficant_domain::subject::{
    AccessSet, FundingTier, LimitCeiling, Subject, SubjectRecord, SubjectStateSnapshot,
    SubjectVersion, TaxTreatment,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};
use ficant_storage::postgres::PostgresRepository;
use ficant_storage::s3::S3BlobStore;
use sqlx::PgPool;

#[tokio::test]
async fn registration_and_authorization_atomically_commit_one_change_each_and_replay() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = owner();
    let principal = admin(&owner);
    let source = source(owner.clone());
    let register = RegisterDataSource::new(
        context(principal.clone(), "register source", 'R'),
        None,
        source.clone(),
        IdempotencyKey::new("r6a-source-v1").unwrap(),
    )
    .unwrap();
    assert_eq!(repository.register(register.clone()).await.unwrap(), source);
    assert_eq!(repository.register(register).await.unwrap(), source);

    let authorization = authorization(&source);
    let publish = PublishDataSourceAuthorization::new(
        context(principal.clone(), "authorize canonical import", 'V'),
        None,
        authorization.clone(),
        IdempotencyKey::new("r6a-authorization-v1").unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository
            .publish_authorization(publish.clone())
            .await
            .unwrap(),
        authorization
    );
    assert_eq!(
        repository.publish_authorization(publish).await.unwrap(),
        authorization
    );

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM core.foundation_change_records")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 2,
        "idempotent replay must not duplicate audit records"
    );
    let source_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM core.foundation_change_sources")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(source_count, 2);

    let exact = repository
        .get_authorization_exact(principal.access_scope(), authorization.version_ref())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exact, authorization);
    let listed = repository
        .list_authorizations_for_source(
            principal.access_scope(),
            &owner,
            authorization.data_source(),
            Some(ImportInterface::CanonicalQuoteSnapshot),
            PageRequest::new(principal.access_scope().clone(), None, 100).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.items(), &[authorization]);
    let changes = repository
        .list_changes(
            principal.access_scope(),
            &FoundationChangeFilter::new(None, None, None, None).unwrap(),
            PageRequest::new(principal.access_scope().clone(), None, 100).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(changes.items().len(), 2);
    let first_page = repository
        .list_changes(
            principal.access_scope(),
            &FoundationChangeFilter::new(None, None, None, None).unwrap(),
            PageRequest::new(principal.access_scope().clone(), None, 1).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_page.items().len(), 1);
    let cursor = first_page.next_cursor().cloned().unwrap();
    assert!(!cursor.as_str().contains(source.id().as_str()));
    let second_page = repository
        .list_changes(
            principal.access_scope(),
            &FoundationChangeFilter::new(None, None, None, None).unwrap(),
            PageRequest::new(principal.access_scope().clone(), Some(cursor), 1).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_page.items().len(), 1);
    assert!(second_page.next_cursor().is_none());
}

#[tokio::test]
async fn authorization_list_paginates_with_an_encrypted_scope_bound_cursor() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool);
    let owner = owner();
    let principal = admin(&owner);
    let source = source(owner.clone());
    repository
        .register(
            RegisterDataSource::new(
                context(principal.clone(), "register paged source", 'R'),
                None,
                source.clone(),
                IdempotencyKey::new("paged-source-v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let first_authorization = authorization(&source);
    repository
        .publish_authorization(
            PublishDataSourceAuthorization::new(
                context(principal.clone(), "publish first authorization", 'V'),
                None,
                first_authorization.clone(),
                IdempotencyKey::new("paged-authorization-v1").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let second_authorization = DataSourceAuthorization::new(DataSourceAuthorizationInput {
        version: Version::new(2).unwrap(),
        supersedes: Some(first_authorization.version_ref()),
        ..authorization_input(&source)
    })
    .unwrap();
    repository
        .publish_authorization(
            PublishDataSourceAuthorization::new(
                context(principal.clone(), "publish second authorization", 'X'),
                Some(Version::new(1).unwrap()),
                second_authorization.clone(),
                IdempotencyKey::new("paged-authorization-v2").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let first_page = repository
        .list_authorizations_for_source(
            principal.access_scope(),
            &owner,
            first_authorization.data_source(),
            Some(ImportInterface::CanonicalQuoteSnapshot),
            PageRequest::new(principal.access_scope().clone(), None, 1).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_page.items(), &[first_authorization]);
    let cursor = first_page.next_cursor().cloned().unwrap();
    assert!(!cursor.as_str().contains(second_authorization.id().as_str()));

    let second_page = repository
        .list_authorizations_for_source(
            principal.access_scope(),
            &owner,
            second_authorization.data_source(),
            Some(ImportInterface::CanonicalQuoteSnapshot),
            PageRequest::new(principal.access_scope().clone(), Some(cursor.clone()), 1).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_page.items(), &[second_authorization]);
    assert!(second_page.next_cursor().is_none());

    let foreign_scope = AccessScope::new(
        owner.tenant_id().clone(),
        id('B'),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    assert_eq!(
        PageRequest::new(foreign_scope, Some(cursor.clone()), 1)
            .unwrap_err()
            .category(),
        ficant_application::ApplicationErrorCategory::Forbidden,
    );
    assert_eq!(
        Cursor::resume(
            repository.cursor_codec(),
            principal.access_scope(),
            format!("{}x", cursor.as_str()),
        )
        .unwrap_err()
        .category(),
        ficant_application::ApplicationErrorCategory::Forbidden,
    );
}

#[tokio::test]
async fn governed_import_owner_drift_returns_typed_exact_source_error() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let authority_owner = owner();
    let authority = seed_canonical_import_authority(&repository, &pool, &authority_owner).await;
    let foreign_owner = OwnerRef::new(authority_owner.tenant_id().clone(), id('Z'));
    let before_changes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM core.foundation_change_records")
            .fetch_one(&pool)
            .await
            .unwrap();

    let error = GovernedInputUseCase::new(&repository, &repository)
        .resolve_authorized_data_source(
            &researcher(&foreign_owner),
            &authority.authorization.version_ref(),
            authority.authorization.mapping_id(),
            authority.authorization.mapping_hash(),
            ImportInterface::CanonicalQuoteSnapshot,
            &time(2026, 8, 13),
        )
        .await
        .unwrap_err();

    assert_eq!(error.category(), ApplicationErrorCategory::Forbidden);
    assert_eq!(
        error.detail(),
        Some(&ApplicationErrorDetail::DataSourceNotAuthorized {
            authorization_ref: authority.authorization.version_ref(),
            data_source_ref: Some(authority.authorization.data_source().clone()),
            import_interface: ImportInterface::CanonicalQuoteSnapshot,
        })
    );
    let after_changes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM core.foundation_change_records")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after_changes, before_changes);
}

#[tokio::test]
async fn failed_authorization_fk_or_audit_record_rolls_back_every_business_row() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = owner();
    let source = source(owner.clone());
    let principal = admin(&owner);
    let changed = DataSourceAuthorization::new(DataSourceAuthorizationInput {
        data_source: VersionRef::new(id('Z'), Version::new(1).unwrap()),
        data_source_hash: ContentHash::digest(b"missing"),
        ..authorization_input(&source)
    })
    .unwrap();
    let command = PublishDataSourceAuthorization::new(
        context(principal, "must roll back", 'W'),
        None,
        changed,
        IdempotencyKey::new("r6a-invalid-authorization").unwrap(),
    )
    .unwrap();
    assert!(repository.publish_authorization(command).await.is_err());
    for table in [
        "data.source_authorization_identities",
        "data.source_authorizations",
        "core.foundation_change_records",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} must have no partial write");
    }
}

#[tokio::test]
async fn audit_collision_after_business_insert_rolls_back_identity_version_and_idempotency() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = owner();
    let principal = admin(&owner);
    let first = source(owner.clone());
    repository
        .register(
            RegisterDataSource::new(
                context(principal.clone(), "first mutation", 'R'),
                None,
                first,
                IdempotencyKey::new("audit-collision-first").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let second = DataSource::new(DataSourceInput {
        data_source_id: id('E'),
        version: Version::new(1).unwrap(),
        owner: owner.clone(),
        kind: DataSourceKind::FileNdjson,
        name: "Second CGB source".to_owned(),
        connection_binding: "cgb-secondary".to_owned(),
        dataset: "quotes-secondary".to_owned(),
        canonical_schema_id: "ficant.market.quote.canonical.v1".to_owned(),
        canonical_schema_hash: ContentHash::digest(b"schema"),
    })
    .unwrap()
    .with_price_source_type(PriceSourceType::ActiveQuote)
    .unwrap();
    let error = repository
        .register(
            RegisterDataSource::new(
                context(principal.clone(), "colliding audit id", 'R'),
                None,
                second.clone(),
                IdempotencyKey::new("audit-collision-second").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.category(),
        ficant_application::ApplicationErrorCategory::AlreadyExists,
    );
    assert!(
        repository
            .get_exact(
                principal.access_scope(),
                VersionRef::new(second.id().clone(), Version::new(1).unwrap()),
            )
            .await
            .unwrap()
            .is_none(),
        "business row inserted before the audit collision must roll back",
    );
    let second_identities: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM data.source_identities WHERE data_source_id=$1")
            .bind(second.id().as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(second_identities, 0);
    let second_idempotency: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM core.idempotency_records WHERE idempotency_key=$1",
    )
    .bind("audit-collision-second")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(second_idempotency, 0);
}

#[tokio::test]
async fn governed_position_and_health_snapshot_writes_commit_one_change_and_replay_without_legacy()
{
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let position = position_snapshot();
    let health = health_profile();
    seed_lineage_source(&pool).await;
    seed_blob_candidate(
        &pool,
        position.content_hash(),
        position.canonical_payload().len(),
    )
    .await;
    seed_blob_candidate(&pool, health.content_hash(), health.canonical_bytes().len()).await;
    let position_command = GovernedPublishSnapshot::administrator_position(
        context(admin(position.owner()), "publish position snapshot", 'R'),
        position.clone(),
        position_proof(&position),
        IdempotencyKey::new("r6a-position-snapshot").unwrap(),
    )
    .unwrap();
    let health_command = GovernedPublishSnapshot::administrator_data_health_threshold(
        context(admin(health.owner()), "configure health threshold", 'Q'),
        health.clone(),
        health_proof(&health),
        IdempotencyKey::new("r6a-health-threshold").unwrap(),
    )
    .unwrap();

    assert_eq!(
        repository
            .publish_governed(position_command.clone())
            .await
            .unwrap(),
        SnapshotValue::Position(position.clone()),
    );
    assert_eq!(
        repository.publish_governed(position_command).await.unwrap(),
        SnapshotValue::Position(position),
    );
    assert_eq!(
        repository
            .publish_governed(health_command.clone())
            .await
            .unwrap(),
        SnapshotValue::DataHealthThresholdProfile(health.clone()),
    );
    assert_eq!(
        repository.publish_governed(health_command).await.unwrap(),
        SnapshotValue::DataHealthThresholdProfile(health),
    );

    let changes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM core.foundation_change_records")
        .fetch_one(&pool)
        .await
        .unwrap();
    let positions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM research.position_snapshots")
        .fetch_one(&pool)
        .await
        .unwrap();
    let health_profiles: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM research.data_health_threshold_profiles")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((changes, positions, health_profiles), (2, 1, 1));
}

#[tokio::test]
async fn governed_snapshot_audit_collision_rolls_back_business_blob_and_idempotency_rows() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let first = position_snapshot();
    seed_lineage_source(&pool).await;
    seed_blob_candidate(&pool, first.content_hash(), first.canonical_payload().len()).await;
    repository
        .publish_governed(
            GovernedPublishSnapshot::administrator_position(
                context(admin(first.owner()), "first snapshot", 'R'),
                first.clone(),
                position_proof(&first),
                IdempotencyKey::new("r6a-snapshot-first").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let second = health_profile();
    seed_blob_candidate(&pool, second.content_hash(), second.canonical_bytes().len()).await;
    let second_proof = health_proof(&second);
    let error = repository
        .publish_governed(
            GovernedPublishSnapshot::administrator_data_health_threshold(
                context(admin(second.owner()), "colliding snapshot audit", 'R'),
                second,
                second_proof,
                IdempotencyKey::new("r6a-snapshot-second").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.category(),
        ficant_application::ApplicationErrorCategory::AlreadyExists,
    );
    for (table, expected) in [
        ("research.position_snapshots", 1_i64),
        ("research.data_health_threshold_profiles", 0),
        ("core.foundation_change_records", 1),
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, expected, "{table} must reflect one atomic mutation");
    }
    let second_idempotency: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM core.idempotency_records WHERE idempotency_key=$1",
    )
    .bind("r6a-snapshot-second")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(second_idempotency, 0);
}

#[tokio::test]
async fn canonical_import_replay_probe_binds_request_audit_and_hardened_snapshot() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = owner();
    let authority = seed_canonical_import_authority(&repository, &pool, &owner).await;
    let base_key = IdempotencyKey::new("r6a-canonical-import-replay").unwrap();
    let request = canonical_replay_request(
        &owner,
        &authority.authorization,
        &authority.calendar,
        &authority.unit,
        base_key.clone(),
        authority.authorization.mapping_hash().clone(),
    );
    let parquet = b"canonical replay parquet";
    let manifest = b"canonical replay manifest";
    let snapshot = canonical_replay_snapshot(&request, &owner, &authority, parquet, manifest);
    seed_blob_candidate(&pool, snapshot.content_hash(), parquet.len()).await;
    seed_blob_candidate(&pool, snapshot.manifest_hash(), manifest.len()).await;
    let command = GovernedPublishSnapshot::authorized_import(
        request.clone(),
        snapshot.clone(),
        data_snapshot_proof(&owner, parquet, manifest),
        IdempotencyKey::new("r6a-canonical-import-replay/metadata").unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository.publish_governed(command.clone()).await.unwrap(),
        SnapshotValue::Data(snapshot.clone())
    );
    assert_eq!(
        repository.publish_governed(command).await.unwrap(),
        SnapshotValue::Data(snapshot.clone())
    );

    let replay = repository
        .probe_canonical_import_replay(&request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.snapshot(), &snapshot);
    assert_eq!(
        replay.actor_id(),
        request.change_context().principal().actor_id()
    );
    assert_eq!(
        replay.authorization(),
        &authority.authorization.version_ref()
    );
    assert_eq!(
        replay.authorization_hash(),
        authority.authorization.content_hash()
    );
    assert_canonical_replay_counts(&pool).await;
    assert_canonical_replay_drift_and_tamper(
        &repository,
        &pool,
        &owner,
        &authority,
        base_key,
        &snapshot,
        &request,
    )
    .await;
}

struct CanonicalReplayAuthority {
    source: DataSource,
    authorization: DataSourceAuthorization,
    calendar: VersionRef,
    unit: VersionRef,
}

async fn seed_canonical_import_authority(
    repository: &PostgresRepository,
    pool: &PgPool,
    owner: &OwnerRef,
) -> CanonicalReplayAuthority {
    let source = source(owner.clone());
    repository
        .register(
            RegisterDataSource::new(
                context(admin(owner), "register replay source", 'R'),
                None,
                source.clone(),
                IdempotencyKey::new("r6a-replay-source").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let authorization = authorization(&source);
    repository
        .publish_authorization(
            PublishDataSourceAuthorization::new(
                context(admin(owner), "authorize replay import", 'V'),
                None,
                authorization.clone(),
                IdempotencyKey::new("r6a-replay-authorization").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let calendar = VersionRef::new(id('C'), Version::new(1).unwrap());
    let unit = VersionRef::new(id('N'), Version::new(1).unwrap());
    seed_import_definitions(pool, owner, &calendar, &unit).await;
    CanonicalReplayAuthority {
        source,
        authorization,
        calendar,
        unit,
    }
}

fn canonical_replay_snapshot(
    request: &CanonicalImportReplayRequest,
    owner: &OwnerRef,
    authority: &CanonicalReplayAuthority,
    parquet: &[u8],
    manifest: &[u8],
) -> DataSnapshot {
    DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: request.target_snapshot_id().clone(),
        owner: owner.clone(),
        visible_at: request.visible_at().clone(),
        as_of: request.as_of().clone(),
        schema_hash: ContentHash::digest(b"canonical-schema"),
        manifest_hash: ContentHash::digest(manifest),
        blob_content_hash: ContentHash::digest(parquet),
        lineage: vec![
            LineageRef::versioned(
                authority.source.id().clone(),
                Version::new(authority.source.version()).unwrap(),
            ),
            LineageRef::content_addressed(
                authority.authorization.mapping_id().clone(),
                authority.authorization.mapping_hash().clone(),
            ),
            LineageRef::new(
                authority.authorization.id().clone(),
                Some(Version::new(authority.authorization.version()).unwrap()),
                Some(authority.authorization.content_hash().clone()),
            )
            .unwrap(),
            LineageRef::versioned(
                authority.calendar.id().clone(),
                authority.calendar.version(),
            ),
            LineageRef::versioned(authority.unit.id().clone(), authority.unit.version()),
        ],
    })
    .unwrap()
}

async fn assert_canonical_replay_counts(pool: &PgPool) {
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM research.data_snapshots),
           (SELECT COUNT(*) FROM core.foundation_change_records
            WHERE operation='data-snapshot.import-canonical-quotes'),
           (SELECT COUNT(*) FROM core.idempotency_records
            WHERE scope='data-snapshot:canonical-import-request:v1')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(counts, (1, 1, 1));
}

async fn assert_canonical_replay_drift_and_tamper(
    repository: &PostgresRepository,
    pool: &PgPool,
    owner: &OwnerRef,
    authority: &CanonicalReplayAuthority,
    base_key: IdempotencyKey,
    snapshot: &DataSnapshot,
    request: &CanonicalImportReplayRequest,
) {
    let drifted = canonical_replay_request(
        owner,
        &authority.authorization,
        &authority.calendar,
        &authority.unit,
        base_key,
        ContentHash::digest(b"drifted-mapping"),
    );
    assert_eq!(
        repository
            .probe_canonical_import_replay(&drifted)
            .await
            .unwrap_err()
            .category(),
        ficant_application::ApplicationErrorCategory::AlreadyExists
    );
    sqlx::query(
        "UPDATE research.data_snapshots SET manifest_hash=content_hash
         WHERE tenant_id=$1 AND data_snapshot_id=$2",
    )
    .bind(owner.tenant_id().as_str())
    .bind(snapshot.id().as_str())
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(
        repository
            .probe_canonical_import_replay(request)
            .await
            .unwrap_err()
            .category(),
        ficant_application::ApplicationErrorCategory::StorageUnavailable
    );
}

#[tokio::test]
async fn governed_subject_and_state_commit_with_audit_replay_scope_and_rollback() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let repository = support::repository(pool.clone());
    let owner = owner();
    let principal = admin(&owner);
    let subject = subject_record(owner.clone());
    let register = GovernedRegisterSubject::new(
        context(principal.clone(), "register governed subject", 'S'),
        subject.clone(),
        IdempotencyKey::new("r6a-subject-v1").unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository
            .register_governed_subject(register.clone())
            .await
            .unwrap(),
        subject
    );
    assert_eq!(
        repository
            .register_governed_subject(register)
            .await
            .unwrap(),
        subject
    );

    let state = subject_state(owner.clone(), 'J');
    let publish = GovernedPublishSubjectState::new(
        context(principal.clone(), "publish governed subject state", 'Q'),
        state.clone(),
        IdempotencyKey::new("r6a-subject-state-v1").unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository
            .publish_governed_subject_state(publish.clone())
            .await
            .unwrap(),
        state
    );
    assert_eq!(
        repository
            .publish_governed_subject_state(publish)
            .await
            .unwrap(),
        state
    );

    assert_subject_scope_and_counts(&pool, &repository, &owner, &principal, &subject, &state).await;
    assert_subject_audit_collision_rolls_back(&pool, &repository, owner, principal).await;
}

async fn assert_subject_scope_and_counts(
    pool: &PgPool,
    repository: &PostgresRepository,
    owner: &OwnerRef,
    principal: &AuthorizedPrincipal,
    subject: &SubjectRecord,
    state: &SubjectStateSnapshot,
) {
    assert_eq!(
        repository
            .get_subject_scoped(
                principal.access_scope(),
                subject.version().reference().clone(),
            )
            .await
            .unwrap(),
        Some(subject.clone()),
    );
    assert_eq!(
        repository
            .get_subject_state_scoped(
                principal.access_scope(),
                state.id().clone(),
                state.visible_at(),
            )
            .await
            .unwrap(),
        Some(state.clone()),
    );
    let foreign_scope =
        AccessScope::new(owner.tenant_id().clone(), id('B'), vec![id('X')]).unwrap();
    assert_eq!(
        repository
            .get_subject_scoped(&foreign_scope, subject.version().reference().clone())
            .await
            .unwrap_err()
            .category(),
        ficant_application::ApplicationErrorCategory::Forbidden,
    );
    assert_eq!(
        repository
            .register_subject(subject.clone())
            .await
            .unwrap_err()
            .category(),
        ficant_application::ApplicationErrorCategory::StateConflict,
        "the legacy no-audit storage mutation must remain unreachable",
    );
    let change_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM core.foundation_change_records
         WHERE operation IN ('subject.register', 'subject-state.publish')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let idempotency_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM core.idempotency_records
         WHERE scope IN ('subject:register:v1', 'subject-state:publish:v1')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(change_count, 2);
    assert_eq!(idempotency_count, 2);
}

async fn assert_subject_audit_collision_rolls_back(
    pool: &PgPool,
    repository: &PostgresRepository,
    owner: OwnerRef,
    principal: AuthorizedPrincipal,
) {
    let colliding = subject_state(owner, 'K');
    let collision = GovernedPublishSubjectState::new(
        context(principal, "audit collision must roll back", 'S'),
        colliding.clone(),
        IdempotencyKey::new("r6a-subject-state-collision").unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository
            .publish_governed_subject_state(collision)
            .await
            .unwrap_err()
            .category(),
        ficant_application::ApplicationErrorCategory::AlreadyExists,
    );
    let rolled_back_state: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM core.subject_state_snapshots WHERE snapshot_id = $1",
    )
    .bind(colliding.id().as_str())
    .fetch_one(pool)
    .await
    .unwrap();
    let rolled_back_idempotency: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM core.idempotency_records
         WHERE scope = 'subject-state:publish:v1'
           AND idempotency_key = 'r6a-subject-state-collision'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(rolled_back_state, 0);
    assert_eq!(rolled_back_idempotency, 0);
}

fn subject_record(owner: OwnerRef) -> SubjectRecord {
    let subject_id = id('H');
    SubjectRecord::new(
        Subject::new_owned(subject_id.clone(), owner, "Governed test Subject").unwrap(),
        SubjectVersion::new(
            VersionRef::new(subject_id, Version::new(1).unwrap()),
            AccessSet::new(["CGB"], ["rates"]).unwrap(),
            FundingTier::DrAvailable,
            TaxTreatment::new("vat-standard", "income-standard").unwrap(),
            "daily-assessment",
            "general-liability",
            None,
        )
        .unwrap(),
    )
    .unwrap()
}

fn subject_state(owner: OwnerRef, snapshot_suffix: char) -> SubjectStateSnapshot {
    let instant = Utc.with_ymd_and_hms(2026, 8, 13, 8, 0, 0).unwrap();
    let unit = UnitRef::new(id('Y'), Version::new(1).unwrap());
    let net_capital = DecimalValue::new("1000000", 2, unit.clone()).unwrap();
    SubjectStateSnapshot::new_owned(
        id(snapshot_suffix),
        VersionRef::new(id('H'), Version::new(1).unwrap()),
        net_capital,
        vec![LimitCeiling::new("leverage", DecimalValue::new("250", 2, unit).unwrap()).unwrap()],
        instant,
        instant,
        "Asia/Shanghai",
        owner,
    )
    .unwrap()
}

fn authorization(source: &DataSource) -> DataSourceAuthorization {
    DataSourceAuthorization::new(authorization_input(source)).unwrap()
}
fn authorization_input(source: &DataSource) -> DataSourceAuthorizationInput {
    DataSourceAuthorizationInput {
        authorization_id: id('V'),
        version: Version::new(1).unwrap(),
        owner: source.owner().clone(),
        data_source: VersionRef::new(source.id().clone(), Version::new(source.version()).unwrap()),
        data_source_hash: data_source_content_hash(source),
        import_interface: ImportInterface::CanonicalQuoteSnapshot,
        canonical_schema_id: source.canonical_schema_id().to_owned(),
        canonical_schema_hash: source.canonical_schema_hash().clone(),
        effective: EffectivePeriod::new(time(2026, 1, 1), time(2027, 1, 1)).unwrap(),
        state: DataSourceAuthorizationState::Active,
        supersedes: None,
        mapping_id: id('M'),
        mapping_hash: ContentHash::digest(b"mapping"),
    }
}
fn source(owner: OwnerRef) -> DataSource {
    DataSource::new(DataSourceInput {
        data_source_id: id('D'),
        version: Version::new(1).unwrap(),
        owner,
        kind: DataSourceKind::FileNdjson,
        name: "CGB quotes".to_owned(),
        connection_binding: "cgb-primary".to_owned(),
        dataset: "quotes".to_owned(),
        canonical_schema_id: "ficant.market.quote.canonical.v1".to_owned(),
        canonical_schema_hash: ContentHash::digest(b"schema"),
    })
    .unwrap()
    .with_price_source_type(PriceSourceType::ActiveQuote)
    .unwrap()
}
fn context(principal: AuthorizedPrincipal, reason: &str, suffix: char) -> FoundationChangeContext {
    FoundationChangeContext::administrator(
        principal,
        ChangeJustification::new(
            reason,
            vec![
                SourceDocumentRef::new("urn:test:evidence", ContentHash::digest(b"evidence"))
                    .unwrap(),
            ],
        )
        .unwrap(),
        id(suffix),
        time(2026, 8, 13),
    )
    .unwrap()
}
fn admin(owner: &OwnerRef) -> AuthorizedPrincipal {
    AuthorizedPrincipal::new(
        "storage-admin".to_owned(),
        id('A'),
        owner.tenant_id().clone(),
        vec![owner.owner_id().clone()],
        PlatformRole::PlatformAdmin,
        vec![
            "data-health:configure".to_owned(),
            "data-sources:write".to_owned(),
            "governance:read".to_owned(),
            "positions:write".to_owned(),
            "registry:read".to_owned(),
            "registry:write".to_owned(),
        ],
        ContentHash::digest(b"credential"),
    )
    .unwrap()
}
fn researcher(owner: &OwnerRef) -> AuthorizedPrincipal {
    AuthorizedPrincipal::new(
        "storage-researcher".to_owned(),
        id('A'),
        owner.tenant_id().clone(),
        vec![owner.owner_id().clone()],
        PlatformRole::Researcher,
        vec!["data-sources:import".to_owned()],
        ContentHash::digest(b"researcher-credential"),
    )
    .unwrap()
}
fn canonical_replay_request(
    owner: &OwnerRef,
    authorization: &DataSourceAuthorization,
    calendar: &VersionRef,
    unit: &VersionRef,
    idempotency_key: IdempotencyKey,
    mapping_hash: ContentHash,
) -> CanonicalImportReplayRequest {
    CanonicalImportReplayRequest::new(
        FoundationChangeContext::authorized_import(
            researcher(owner),
            ChangeJustification::for_authorized_import("import canonical replay fixture").unwrap(),
            id('K'),
            time(2026, 8, 13),
        )
        .unwrap(),
        owner.clone(),
        id('B'),
        authorization.version_ref(),
        authorization.content_hash().clone(),
        authorization.mapping_id().clone(),
        mapping_hash,
        calendar.clone(),
        unit.clone(),
        time(2026, 8, 12),
        time(2026, 8, 13),
        idempotency_key,
    )
    .unwrap()
}
async fn seed_import_definitions(
    pool: &sqlx::PgPool,
    owner: &OwnerRef,
    calendar: &VersionRef,
    unit: &VersionRef,
) {
    sqlx::query(
        "INSERT INTO market.calendars
         (tenant_id, calendar_id, version, owner_id, market, market_timezone,
          effective_from, effective_to, payload)
         VALUES ($1,$2,$3,$4,'CGB','UTC',$5,$6,$7)",
    )
    .bind(owner.tenant_id().as_str())
    .bind(calendar.id().as_str())
    .bind(i64::try_from(calendar.version().get()).unwrap())
    .bind(owner.owner_id().as_str())
    .bind(time(2026, 1, 1).instant())
    .bind(time(2027, 1, 1).instant())
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.units
         (tenant_id, unit_id, version, owner_id, code, dimension, scale, precision, payload)
         VALUES ($1,$2,$3,$4,'CNY','currency',2,18,$5)",
    )
    .bind(owner.tenant_id().as_str())
    .bind(unit.id().as_str())
    .bind(i64::try_from(unit.version().get()).unwrap())
    .bind(owner.owner_id().as_str())
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
}
fn data_snapshot_proof(owner: &OwnerRef, parquet: &[u8], manifest: &[u8]) -> VerifiedSnapshotProof {
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        id('A'),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    let verified = |staging_id: Ulid, bytes: &[u8], role| {
        let size = u64::try_from(bytes.len()).unwrap();
        let hash = ContentHash::digest(bytes);
        let staged = StagedSnapshotBlob::new(
            role,
            VerifyBlobStage::new(
                scope.clone(),
                StagedBlobRef::new(staging_id, owner.clone()),
                hash.clone(),
                size,
            )
            .unwrap(),
        );
        VerifiedSnapshotBlob::from_staged(staged, VerifiedBlobRef::new(hash, size).unwrap())
            .unwrap()
    };
    VerifiedSnapshotProof::data(
        verified(id('G'), parquet, SnapshotBlobRole::DataParquet),
        verified(id('E'), manifest, SnapshotBlobRole::DataManifest),
    )
    .unwrap()
}
fn position_proof(snapshot: &PositionSnapshot) -> VerifiedSnapshotProof {
    verified_proof(
        snapshot.owner(),
        snapshot.content_hash(),
        snapshot.canonical_payload().len(),
        SnapshotBlobRole::PositionPayload,
        VerifiedSnapshotProof::position,
    )
}
async fn seed_blob_candidate(pool: &sqlx::PgPool, hash: &ContentHash, size: usize) {
    let hash = S3BlobStore::hash_hex(hash);
    sqlx::query(
        "INSERT INTO storage.orphan_candidates (content_hash, object_key, blob_size)
         VALUES ($1,$2,$3)",
    )
    .bind(&hash)
    .bind(format!("immutable/{hash}"))
    .bind(i64::try_from(size).unwrap())
    .execute(pool)
    .await
    .unwrap();
}
async fn seed_lineage_source(pool: &sqlx::PgPool) {
    let value = source(owner());
    sqlx::query(
        "INSERT INTO data.source_identities
         (tenant_id, data_source_id, owner_id, latest_version) VALUES ($1,$2,$3,1)",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.id().as_str())
    .bind(value.owner().owner_id().as_str())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO data.sources
         (tenant_id, data_source_id, version, owner_id, kind, name, connection_binding,
          dataset, canonical_schema_id, canonical_schema_hash, price_source_type)
         VALUES ($1,$2,1,$3,'FILE_NDJSON',$4,$5,$6,$7,$8,'ACTIVE_QUOTE')",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.id().as_str())
    .bind(value.owner().owner_id().as_str())
    .bind(value.name())
    .bind(value.connection_binding())
    .bind(value.dataset())
    .bind(value.canonical_schema_id())
    .bind(S3BlobStore::hash_hex(value.canonical_schema_hash()))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO core.subject_identities
         (tenant_id, subject_id, owner_id, latest_version) VALUES ($1,$2,$3,1)",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(id('B').as_str())
    .bind(value.owner().owner_id().as_str())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO core.subject_versions
         (tenant_id, subject_id, version, owner_id, display_name, market_codes, tool_codes,
          funding_tier, value_added_tax_profile, income_tax_profile, assessment_mechanism,
          liability_profile)
         VALUES ($1,$2,1,$3,'R6A subject','{}','{}','R_ONLY','','','assessment','liability')",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(id('B').as_str())
    .bind(value.owner().owner_id().as_str())
    .execute(pool)
    .await
    .unwrap();
}
fn health_proof(profile: &DataHealthThresholdProfile) -> VerifiedSnapshotProof {
    verified_proof(
        profile.owner(),
        profile.content_hash(),
        profile.canonical_bytes().len(),
        SnapshotBlobRole::DataHealthThresholdProfilePayload,
        VerifiedSnapshotProof::data_health_threshold_profile,
    )
}
fn verified_proof(
    owner: &OwnerRef,
    hash: &ContentHash,
    size: usize,
    role: SnapshotBlobRole,
    proof: fn(
        VerifiedSnapshotBlob,
    ) -> ficant_application::ports::ApplicationResult<VerifiedSnapshotProof>,
) -> VerifiedSnapshotProof {
    let scope = AccessScope::new(
        owner.tenant_id().clone(),
        id('A'),
        vec![owner.owner_id().clone()],
    )
    .unwrap();
    let size = u64::try_from(size).unwrap();
    let staged = StagedSnapshotBlob::new(
        role,
        VerifyBlobStage::new(
            scope,
            StagedBlobRef::new(id('G'), owner.clone()),
            hash.clone(),
            size,
        )
        .unwrap(),
    );
    proof(
        VerifiedSnapshotBlob::from_staged(
            staged,
            VerifiedBlobRef::new(hash.clone(), size).unwrap(),
        )
        .unwrap(),
    )
    .unwrap()
}
fn position_snapshot() -> PositionSnapshot {
    let unit = UnitRef::new(id('X'), Version::new(1).unwrap());
    let decimal = |value| DecimalValue::new(value, 0, unit.clone()).unwrap();
    let position = Position::new(PositionInput {
        position_id: id('J'),
        instrument_ref: VersionRef::new(id('C'), Version::new(1).unwrap()),
        quantity: decimal("1"),
        economic_value: decimal("100"),
        economic_pnl: decimal("2"),
        accounting_pnl: decimal("1"),
        capital_requirement: decimal("3"),
        accounting_classification: AccountingClassification::new(
            AccountingClassificationState::Classified,
            Some(AccountingBook::Ac),
        )
        .unwrap(),
        holding_form: PositionHoldingForm::Owned,
    })
    .unwrap();
    let mut input = PositionSnapshotInput {
        snapshot_id: id('S'),
        owner: owner(),
        subject_ref: VersionRef::new(id('B'), Version::new(1).unwrap()),
        observed_at: time(2026, 8, 12),
        visible_at: time(2026, 8, 13),
        content_hash: ContentHash::digest(b"pending"),
        lineage: vec![LineageRef::versioned(id('D'), Version::new(1).unwrap())],
        positions: vec![position],
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    PositionSnapshot::new(input).unwrap()
}
fn health_profile() -> DataHealthThresholdProfile {
    let mut input = DataHealthThresholdProfileInput {
        profile_snapshot_id: id('H'),
        owner: owner(),
        profile_ref: VersionRef::new(id('F'), Version::new(1).unwrap()),
        visible_at: time(2026, 8, 13),
        effective_from: time(2026, 8, 13),
        effective_to: time(2027, 8, 13),
        max_position_snapshot_age_seconds: 100,
        unknown_accounting_warning_basis_points: 5_000,
        max_data_snapshot_age_seconds: 100,
        model_valuation_warning_basis_points: 5_000,
        content_hash: ContentHash::digest(b"pending"),
        lineage: Vec::new(),
    };
    input.content_hash = DataHealthThresholdProfile::content_hash_for(&input);
    DataHealthThresholdProfile::new(input).unwrap()
}
fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('P'))
}
fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
fn time(year: i32, month: u32, day: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap(),
        "UTC",
        NaiveDate::from_ymd_opt(year, month, day).unwrap(),
    )
    .unwrap()
}
