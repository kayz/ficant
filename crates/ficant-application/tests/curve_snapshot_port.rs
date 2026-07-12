use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, AppendMarketFact, ApplicationResult, CorrectMarketFact, CursorPage, MarketFact,
    MarketFactRepository, MarketFactWindow, PublishCurveSnapshot, VerifiedBlobRef,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, IdempotencyKey};
use ficant_domain::ContentAddressed;
use ficant_domain::market::{ArtifactInputKind, CurveSnapshot, CurveSnapshotInput};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};

#[test]
fn curve_publish_rejects_unauthorized_owner_hash_and_size_before_repository() {
    let curve = curve(base_fixture());
    let wrong_scope = access_scope('K', 'A', &['Y']);
    assert_category(
        &PublishCurveSnapshot::new(
            wrong_scope,
            curve.clone(),
            11,
            verified(7, 11),
            key("curve"),
        )
        .unwrap_err(),
        ApplicationErrorCategory::Forbidden,
    );

    let scope = access_scope('T', 'A', &['Y']);
    assert_category(
        &PublishCurveSnapshot::new(
            scope.clone(),
            curve.clone(),
            11,
            verified(8, 11),
            key("curve"),
        )
        .unwrap_err(),
        ApplicationErrorCategory::HashMismatch,
    );
    assert_category(
        &PublishCurveSnapshot::new(scope, curve, 12, verified(7, 11), key("curve")).unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );
}

#[test]
fn identical_curve_publication_is_idempotent_but_same_id_changed_content_is_immutable() {
    let scope = access_scope('T', 'A', &['Y']);
    let original_curve = curve(base_fixture());
    let replay = publish(scope.clone(), original_curve.clone(), 11, "same-request");
    let original = publish(scope.clone(), original_curve, 11, "same-request");
    assert_eq!(original.fingerprint(), replay.fingerprint());
    assert_eq!(original.idempotency_key(), replay.idempotency_key());
    original.ensure_replay_compatible(&replay).unwrap();

    let changed = publish(
        scope,
        curve(CurveFixture {
            schema: "tenor,discount_factor",
            hash: 8,
            ..base_fixture()
        }),
        11,
        "same-request",
    );
    assert_ne!(original.fingerprint(), changed.fingerprint());
    assert_category(
        &original.ensure_replay_compatible(&changed).unwrap_err(),
        ApplicationErrorCategory::ImmutableViolation,
    );
}

#[test]
fn curve_fingerprint_covers_scope_owner_all_metadata_lineage_and_verified_blob() {
    let scope = access_scope('T', 'A', &['Y']);
    let original = publish(scope.clone(), curve(base_fixture()), 11, "curve");
    let variants = [
        CurveFixture {
            id: 'Q',
            ..base_fixture()
        },
        CurveFixture {
            hour: 2,
            ..base_fixture()
        },
        CurveFixture {
            currency: 'P',
            ..base_fixture()
        },
        CurveFixture {
            kind: "swap-zero",
            ..base_fixture()
        },
        CurveFixture {
            calendar: 'E',
            ..base_fixture()
        },
        CurveFixture {
            rule_pack: 'J',
            ..base_fixture()
        },
        CurveFixture {
            schema: "tenor,df",
            ..base_fixture()
        },
        CurveFixture {
            hash: 8,
            ..base_fixture()
        },
        CurveFixture {
            lineage: 'P',
            ..base_fixture()
        },
    ];
    for variant in variants {
        let changed = publish(scope.clone(), curve(variant), 11, "curve");
        assert_ne!(original.fingerprint(), changed.fingerprint());
    }

    let actor_changed = publish(
        access_scope('T', 'B', &['Y']),
        curve(base_fixture()),
        11,
        "curve",
    );
    let owner_changed = publish(
        access_scope('T', 'A', &['Z']),
        curve(CurveFixture {
            owner: 'Z',
            ..base_fixture()
        }),
        11,
        "curve",
    );
    let size_changed = publish(scope, curve(base_fixture()), 12, "curve");
    assert_ne!(original.fingerprint(), actor_changed.fingerprint());
    assert_ne!(original.fingerprint(), owner_changed.fingerprint());
    assert_ne!(original.fingerprint(), size_changed.fingerprint());
}

#[test]
fn market_fact_repository_exposes_dedicated_curve_publish_and_scoped_get() {
    fn assert_repository<T: MarketFactRepository>() {}
    assert_repository::<ContractMarketFactRepository>();
}

struct ContractMarketFactRepository;

#[async_trait]
impl MarketFactRepository for ContractMarketFactRepository {
    async fn append_fact(&self, _command: AppendMarketFact) -> ApplicationResult<MarketFact> {
        Err(not_used())
    }

    async fn append_correction(
        &self,
        _command: CorrectMarketFact,
    ) -> ApplicationResult<MarketFact> {
        Err(not_used())
    }

    async fn query_instrument_window(
        &self,
        _scope: &AccessScope,
        _query: MarketFactWindow,
    ) -> ApplicationResult<CursorPage<MarketFact>> {
        Err(not_used())
    }

    async fn publish_curve_snapshot(
        &self,
        _command: PublishCurveSnapshot,
    ) -> ApplicationResult<CurveSnapshot> {
        Err(not_used())
    }

    async fn get_curve_snapshot(
        &self,
        _scope: &AccessScope,
        _curve_snapshot_id: Ulid,
    ) -> ApplicationResult<Option<CurveSnapshot>> {
        Err(not_used())
    }
}

#[derive(Clone, Copy)]
struct CurveFixture {
    id: char,
    tenant: char,
    owner: char,
    hour: u32,
    currency: char,
    kind: &'static str,
    calendar: char,
    rule_pack: char,
    schema: &'static str,
    hash: u8,
    lineage: char,
}

fn base_fixture() -> CurveFixture {
    CurveFixture {
        id: 'C',
        tenant: 'T',
        owner: 'Y',
        hour: 1,
        currency: 'M',
        kind: "government-zero",
        calendar: 'D',
        rule_pack: 'H',
        schema: "tenor,value",
        hash: 7,
        lineage: 'N',
    }
}

fn curve(value: CurveFixture) -> CurveSnapshot {
    CurveSnapshot::new(CurveSnapshotInput {
        curve_snapshot_id: id(value.id),
        owner: OwnerRef::new(id(value.tenant), id(value.owner)),
        as_of: time(value.hour),
        currency: UnitRef::new(id(value.currency), version(1)),
        curve_kind: value.kind.to_owned(),
        calendar: VersionRef::new(id(value.calendar), version(1)),
        rule_pack: VersionRef::new(id(value.rule_pack), version(1)),
        point_schema: value.schema.to_owned(),
        content_hash: hash(value.hash),
        lineage: vec![LineageRef::versioned(id(value.lineage), version(1))],
        input_kind: ArtifactInputKind::ExternalFixture,
    })
    .unwrap()
}

fn publish(
    scope: AccessScope,
    curve: CurveSnapshot,
    size: u64,
    idempotency_key: &str,
) -> PublishCurveSnapshot {
    let content_hash = curve.content_hash().clone();
    PublishCurveSnapshot::new(
        scope,
        curve,
        size,
        VerifiedBlobRef::new(content_hash, size).unwrap(),
        key(idempotency_key),
    )
    .unwrap()
}

fn access_scope(tenant: char, actor: char, owners: &[char]) -> AccessScope {
    AccessScope::new(
        id(tenant),
        id(actor),
        owners.iter().copied().map(id).collect(),
    )
    .unwrap()
}

fn verified(hash_byte: u8, size: u64) -> VerifiedBlobRef {
    VerifiedBlobRef::new(hash(hash_byte), size).unwrap()
}

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).unwrap()
}

fn hash(byte: u8) -> ContentHash {
    ContentHash::from_bytes(&[byte; 32]).unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-04".parse().unwrap(),
    )
    .unwrap()
}

fn version(value: u64) -> Version {
    Version::new(value).unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn not_used() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}

fn assert_category(error: &ApplicationError, expected: ApplicationErrorCategory) {
    assert_eq!(error.category(), expected);
}
