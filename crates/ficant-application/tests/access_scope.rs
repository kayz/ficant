use ficant_application::ports::{BeginBlobStage, StagedBlobRef, VerifyBlobStage};
use ficant_application::{
    AccessScope, AeadCursorCodec, ApplicationError, ApplicationErrorCategory, AuthorizedPrincipal,
    Cursor, CursorKey, IdempotencyKey, PageRequest,
};
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid};

#[test]
fn allowed_owners_are_canonical_sorted_unique_and_fail_closed_by_tenant() {
    let canonical = AccessScope::new(id('T'), id('X'), vec![id('B'), id('A')]).unwrap();
    let reordered_with_duplicate =
        AccessScope::new(id('T'), id('X'), vec![id('A'), id('B'), id('A')]).unwrap();
    let expanded = AccessScope::new(id('T'), id('X'), vec![id('A'), id('B'), id('C')]).unwrap();

    assert_eq!(canonical.allowed_owner_ids(), &[id('A'), id('B')]);
    assert_eq!(canonical, reordered_with_duplicate);
    assert_eq!(
        canonical.fingerprint(),
        reordered_with_duplicate.fingerprint()
    );
    assert_ne!(canonical.fingerprint(), expanded.fingerprint());
    assert!(canonical.allows(&OwnerRef::new(id('T'), id('A'))));
    assert!(!canonical.allows(&OwnerRef::new(id('K'), id('A'))));
    assert!(!canonical.allows(&OwnerRef::new(id('T'), id('C'))));
    assert_category(
        &AccessScope::new(id('T'), id('X'), Vec::new()).unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );
}

#[test]
fn principal_requires_role_scope_and_owner_independently() {
    let owner = OwnerRef::new(id('T'), id('A'));
    let principal = AuthorizedPrincipal::new(
        "admin@example.test".to_owned(),
        id('X'),
        id('T'),
        vec![id('A'), id('A')],
        PlatformRole::PlatformAdmin,
        vec![
            "data-sources:write".to_owned(),
            "data-sources:read".to_owned(),
        ],
        ContentHash::digest(b"credential"),
    )
    .unwrap();
    principal
        .authorize_mutation(PlatformRole::PlatformAdmin, "data-sources:write", &owner)
        .unwrap();
    assert_eq!(principal.allowed_owner_ids(), &[id('A')]);
    for error in [
        principal.authorize_mutation(PlatformRole::Researcher, "data-sources:write", &owner),
        principal.authorize_mutation(PlatformRole::PlatformAdmin, "data-sources:import", &owner),
        principal.authorize_mutation(
            PlatformRole::PlatformAdmin,
            "data-sources:write",
            &OwnerRef::new(id('T'), id('B')),
        ),
    ] {
        assert_category(&error.unwrap_err(), ApplicationErrorCategory::Forbidden);
    }
}

#[test]
fn principal_fingerprint_binds_active_role_and_credential_fingerprint() {
    let build = |role, credential: &[u8]| {
        AuthorizedPrincipal::new(
            "same-human".to_owned(),
            id('X'),
            id('T'),
            vec![id('A')],
            role,
            vec!["data-sources:write".to_owned()],
            ContentHash::digest(credential),
        )
        .unwrap()
    };
    let admin = build(PlatformRole::PlatformAdmin, b"credential-a");
    let researcher = build(PlatformRole::Researcher, b"credential-a");
    let rotated = build(PlatformRole::PlatformAdmin, b"credential-b");
    assert_ne!(admin.fingerprint(), researcher.fingerprint());
    assert_ne!(admin.fingerprint(), rotated.fingerprint());
    assert_eq!(
        admin.credential_fingerprint(),
        &ContentHash::digest(b"credential-a")
    );
}

#[test]
fn issued_cursor_rejects_cross_scope_replay_and_accepts_canonical_scope() {
    let codec = codec("active", 7, Vec::new());
    let issuer = scope('T', 'X', &['B', 'A']);
    let canonical_equivalent = scope('T', 'X', &['A', 'B', 'A']);
    let wrong_tenant = scope('K', 'X', &['A', 'B']);
    let wrong_actor = scope('T', 'Z', &['A', 'B']);
    let wrong_owners = scope('T', 'X', &['A']);
    let cursor = Cursor::issue(&codec, &issuer, "storage-page-2").unwrap();
    let handoff_token = cursor.as_str().to_owned();

    PageRequest::new(issuer, Some(cursor.clone()), 25).unwrap();
    let resumed = Cursor::resume(&codec, &canonical_equivalent, handoff_token.clone()).unwrap();
    assert_eq!(resumed.opaque_value(), "storage-page-2");
    PageRequest::new(canonical_equivalent, Some(resumed), 25).unwrap();
    for replay_scope in [wrong_tenant, wrong_actor, wrong_owners] {
        assert_category(
            &Cursor::resume(&codec, &replay_scope, handoff_token.clone()).unwrap_err(),
            ApplicationErrorCategory::Forbidden,
        );
        assert_category(
            &PageRequest::new(replay_scope, Some(cursor.clone()), 25).unwrap_err(),
            ApplicationErrorCategory::Forbidden,
        );
    }
}

#[test]
fn cursor_token_does_not_disclose_storage_cursor_or_scope_identities() {
    let codec = codec("active", 7, Vec::new());
    let issuer = scope('T', 'X', &['A', 'B']);
    let raw_storage_cursor = "postgres-row-id=991-secret";
    let cursor = Cursor::issue(&codec, &issuer, raw_storage_cursor).unwrap();
    let token = cursor.as_str();
    let second = Cursor::issue(&codec, &issuer, raw_storage_cursor).unwrap();

    assert_ne!(token, second.as_str());
    assert!(!token.contains(raw_storage_cursor));
    assert!(!token.contains(issuer.tenant_id().as_str()));
    assert!(!token.contains(issuer.actor_id().as_str()));
    for owner_id in issuer.allowed_owner_ids() {
        assert!(!token.contains(owner_id.as_str()));
    }
}

#[test]
fn replacing_scope_binding_with_another_valid_scope_binding_is_forbidden() {
    let codec = codec("active", 7, Vec::new());
    let issuer = scope('T', 'X', &['A']);
    let other = scope('K', 'Z', &['B']);
    let cursor = Cursor::issue(&codec, &issuer, "postgres-page-7").unwrap();
    let issuer_binding = hash_hex(issuer.fingerprint().content_hash().as_bytes());
    let other_binding = hash_hex(other.fingerprint().content_hash().as_bytes());
    let forged = cursor.as_str().replace(&issuer_binding, &other_binding);

    assert_eq!(forged, cursor.as_str());
    assert!(!cursor.as_str().contains(&issuer_binding));
    assert_category(
        &Cursor::resume(&codec, &other, forged).unwrap_err(),
        ApplicationErrorCategory::Forbidden,
    );
}

#[test]
fn modifying_any_cursor_token_field_is_forbidden() {
    let codec = codec("active", 7, vec![cursor_key("retired", 9)]);
    let issuer = scope('T', 'X', &['A']);
    let cursor = Cursor::issue(&codec, &issuer, "postgres-page-7").unwrap();
    let fields = cursor.as_str().split('.').collect::<Vec<_>>();
    assert_eq!(fields.len(), 4);
    let tampered = [
        format!("FCUR3.{}.{}.{}", fields[1], fields[2], fields[3]),
        format!("{}.retired.{}.{}", fields[0], fields[2], fields[3]),
        format!(
            "{}.{}.{}.{}",
            fields[0],
            fields[1],
            flip_hex(fields[2], 0),
            fields[3]
        ),
        format!(
            "{}.{}.{}.{}",
            fields[0],
            fields[1],
            fields[2],
            flip_hex(fields[3], 0)
        ),
        format!(
            "{}.{}.{}.{}",
            fields[0],
            fields[1],
            fields[2],
            flip_hex(fields[3], fields[3].len() - 1)
        ),
    ];

    for token in tampered {
        assert_category(
            &Cursor::resume(&codec, &issuer, token).unwrap_err(),
            ApplicationErrorCategory::Forbidden,
        );
    }
}

#[test]
fn cursor_codec_has_explicit_key_rotation_and_key_miss_boundaries() {
    let issuer = scope('T', 'X', &['A']);
    let old_codec = codec("old", 3, Vec::new());
    let old = Cursor::issue(&old_codec, &issuer, "postgres-page-old").unwrap();
    let rotated = codec("new", 5, vec![cursor_key("old", 3)]);

    let resumed = Cursor::resume(&rotated, &issuer, old.as_str()).unwrap();
    assert_eq!(resumed.opaque_value(), "postgres-page-old");
    let current = Cursor::issue(&rotated, &issuer, "postgres-page-new").unwrap();
    assert_category(
        &Cursor::resume(&old_codec, &issuer, current.as_str()).unwrap_err(),
        ApplicationErrorCategory::Forbidden,
    );
    assert_category(
        &Cursor::resume(&rotated, &issuer, "FCUR2.missing.00.00").unwrap_err(),
        ApplicationErrorCategory::Forbidden,
    );
}

#[test]
fn scope_mismatch_is_rejected_before_blob_storage_is_called() {
    let authorized = scope('T', 'X', &['A']);
    let wrong_tenant_owner = OwnerRef::new(id('K'), id('A'));
    let disallowed_owner = OwnerRef::new(id('T'), id('B'));

    assert_category(
        &authorized.authorize(&wrong_tenant_owner).unwrap_err(),
        ApplicationErrorCategory::Forbidden,
    );
    assert_category(
        &BeginBlobStage::new(authorized.clone(), disallowed_owner, 8, key("wrong-owner"))
            .unwrap_err(),
        ApplicationErrorCategory::Forbidden,
    );

    let foreign_stage = StagedBlobRef::new(id('S'), wrong_tenant_owner);
    assert_category(
        &VerifyBlobStage::new(authorized, foreign_stage, hash(7), 8).unwrap_err(),
        ApplicationErrorCategory::Forbidden,
    );
}

#[test]
fn blob_mutation_fingerprints_bind_canonical_scope_and_stage_owner() {
    let owner = OwnerRef::new(id('T'), id('A'));
    let actor_x = scope('T', 'X', &['A']);
    let actor_z = scope('T', 'Z', &['A']);
    let begin_x = BeginBlobStage::new(actor_x.clone(), owner.clone(), 8, key("begin")).unwrap();
    let begin_z = BeginBlobStage::new(actor_z.clone(), owner.clone(), 8, key("begin")).unwrap();
    assert_ne!(begin_x.fingerprint(), begin_z.fingerprint());

    let stage_x = StagedBlobRef::new(id('S'), owner.clone());
    let stage_z = StagedBlobRef::new(id('S'), owner);
    let verify_x = VerifyBlobStage::new(actor_x, stage_x, hash(7), 8).unwrap();
    let verify_z = VerifyBlobStage::new(actor_z, stage_z, hash(7), 8).unwrap();
    assert_ne!(verify_x.fingerprint(), verify_z.fingerprint());
    assert_ne!(verify_x.idempotency_key(), verify_z.idempotency_key());
}

fn scope(tenant: char, actor: char, owners: &[char]) -> AccessScope {
    AccessScope::new(
        id(tenant),
        id(actor),
        owners.iter().copied().map(id).collect(),
    )
    .unwrap()
}

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).unwrap()
}

fn codec(active_id: &str, active_byte: u8, retired: Vec<CursorKey>) -> AeadCursorCodec {
    AeadCursorCodec::new(cursor_key(active_id, active_byte), retired).unwrap()
}

fn cursor_key(key_id: &str, byte: u8) -> CursorKey {
    CursorKey::new(key_id, [byte; 32]).unwrap()
}

fn flip_hex(value: &str, index: usize) -> String {
    let mut bytes = value.as_bytes().to_vec();
    bytes[index] = if bytes[index] == b'0' { b'1' } else { b'0' };
    String::from_utf8(bytes).unwrap()
}

fn hash_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut encoded, byte| {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").unwrap();
            encoded
        },
    )
}

fn hash(byte: u8) -> ContentHash {
    ContentHash::from_bytes(&[byte; 32]).unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn assert_category(error: &ApplicationError, expected: ApplicationErrorCategory) {
    assert_eq!(error.category(), expected);
}
