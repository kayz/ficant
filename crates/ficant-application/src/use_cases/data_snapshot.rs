use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, LineageRef, OwnerRef, Ulid, VersionRef};
use ficant_domain::research::{DataSnapshot, UniverseSnapshot};
use ficant_domain::{ContentAddressed, Lineaged};

use crate::ports::{
    AccessScope, BeginBlobStage, BlobStore, CanonicalImportManifestEvidence, CanonicalImportReplay,
    CanonicalImportReplayRequest, FoundationChangeContext, GovernedPublishSnapshot, IdempotencyKey,
    PublishSnapshot, SNAPSHOT_WRITE_SCOPE, SnapshotBlobRole, SnapshotRepository, SnapshotValue,
    StagedSnapshotBlob, StagedSnapshotProof, VerifiedSnapshotBlob, VerifiedSnapshotProof,
    VerifyBlobStage,
};
use crate::{ApplicationError, ApplicationErrorCategory};

#[derive(Clone, Debug)]
pub struct DataSnapshotPayloads {
    snapshot: DataSnapshot,
    parquet: Vec<u8>,
    manifest: Vec<u8>,
    idempotency_key: IdempotencyKey,
    import_evidence: Option<CanonicalImportManifestEvidence>,
}

impl DataSnapshotPayloads {
    /// Binds exact Parquet and Manifest bytes to their already constructed domain snapshot.
    ///
    /// # Errors
    ///
    /// Returns validation failure before I/O for empty payloads or a hash mismatch.
    pub fn new(
        snapshot: DataSnapshot,
        parquet: Vec<u8>,
        manifest: Vec<u8>,
        idempotency_key: IdempotencyKey,
    ) -> Result<Self, ApplicationError> {
        require_payload(&parquet, snapshot.content_hash())?;
        require_payload(&manifest, snapshot.manifest_hash())?;
        if snapshot.content_hash() == snapshot.manifest_hash() {
            return Err(validation_error());
        }
        Ok(Self {
            snapshot,
            parquet,
            manifest,
            idempotency_key,
            import_evidence: None,
        })
    }

    /// Binds evidence extracted from a fully decoded canonical manifest to a publish payload.
    ///
    /// # Errors
    ///
    /// Returns hash or validation failure when either payload does not match the snapshot.
    pub fn new_authorized(
        snapshot: DataSnapshot,
        parquet: Vec<u8>,
        manifest: Vec<u8>,
        idempotency_key: IdempotencyKey,
        import_evidence: CanonicalImportManifestEvidence,
    ) -> Result<Self, ApplicationError> {
        let mut payloads = Self::new(snapshot, parquet, manifest, idempotency_key)?;
        payloads.import_evidence = Some(import_evidence);
        Ok(payloads)
    }

    #[must_use]
    pub fn snapshot(&self) -> &DataSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn parquet(&self) -> &[u8] {
        &self.parquet
    }

    #[must_use]
    pub fn manifest(&self) -> &[u8] {
        &self.manifest
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }
}

pub struct PublishDataSnapshot<'a> {
    blob_store: &'a dyn BlobStore,
    snapshots: &'a dyn SnapshotRepository,
}

#[derive(Clone, Debug)]
pub struct UniverseSnapshotIntent {
    snapshot_id: Ulid,
    owner: OwnerRef,
    instrument_versions: Vec<VersionRef>,
    filter_digest: ContentHash,
    lineage: Vec<LineageRef>,
    actor_id: Ulid,
    idempotency_key: IdempotencyKey,
}

impl UniverseSnapshotIntent {
    /// Canonicalizes the caller's complete member set before any hash or blob operation.
    ///
    /// # Errors
    ///
    /// Returns validation failure for an empty member set or missing lineage.
    pub fn new(
        snapshot_id: Ulid,
        owner: OwnerRef,
        mut instrument_versions: Vec<VersionRef>,
        filter_digest: ContentHash,
        lineage: Vec<LineageRef>,
        actor_id: Ulid,
        idempotency_key: IdempotencyKey,
    ) -> Result<Self, ApplicationError> {
        instrument_versions.sort();
        instrument_versions.dedup();
        if instrument_versions.is_empty() || lineage.is_empty() {
            return Err(validation_error());
        }
        Ok(Self {
            snapshot_id,
            owner,
            instrument_versions,
            filter_digest,
            lineage,
            actor_id,
            idempotency_key,
        })
    }

    #[must_use]
    pub fn actor_id(&self) -> &Ulid {
        &self.actor_id
    }
}

pub struct PublishUniverseSnapshot<'a> {
    blob_store: &'a dyn BlobStore,
    snapshots: &'a dyn SnapshotRepository,
}

impl<'a> PublishUniverseSnapshot<'a> {
    #[must_use]
    pub const fn new(blob_store: &'a dyn BlobStore, snapshots: &'a dyn SnapshotRepository) -> Self {
        Self {
            blob_store,
            snapshots,
        }
    }

    /// Canonically encodes the complete member set and derives the public content hash server-side.
    ///
    /// # Errors
    ///
    /// Returns a classified scope, blob, hash, lineage, or repository error.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        intent: UniverseSnapshotIntent,
    ) -> Result<UniverseSnapshot, ApplicationError> {
        self.execute_inner(scope, intent, None).await
    }

    /// Publishes a server-hashed `UniverseSnapshot` and its administrator change record atomically.
    ///
    /// # Errors
    ///
    /// Returns a classified authorization, blob, lineage, or atomic repository error.
    pub async fn execute_governed_admin(
        &self,
        change_context: FoundationChangeContext,
        intent: UniverseSnapshotIntent,
    ) -> Result<UniverseSnapshot, ApplicationError> {
        change_context.principal().authorize_mutation(
            PlatformRole::PlatformAdmin,
            SNAPSHOT_WRITE_SCOPE,
            &intent.owner,
        )?;
        if change_context.principal().actor_id() != &intent.actor_id {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::Forbidden,
                false,
            ));
        }
        let scope = change_context.principal().access_scope().clone();
        self.execute_inner(&scope, intent, Some(change_context))
            .await
    }

    async fn execute_inner(
        &self,
        scope: &AccessScope,
        intent: UniverseSnapshotIntent,
        change_context: Option<FoundationChangeContext>,
    ) -> Result<UniverseSnapshot, ApplicationError> {
        scope.authorize(&intent.owner)?;
        let manifest = universe_manifest_bytes(&intent);
        let content_hash = ContentHash::digest(&manifest);
        let snapshot = UniverseSnapshot::new(
            intent.snapshot_id,
            intent.owner.clone(),
            intent.instrument_versions,
            intent.filter_digest,
            content_hash.clone(),
            intent.lineage,
        )
        .map_err(crate::map_domain_error)?;
        let size = u64::try_from(manifest.len()).map_err(|_| validation_error())?;
        let staged = self
            .blob_store
            .begin_stage(BeginBlobStage::new(
                scope.clone(),
                snapshot.owner().clone(),
                size,
                intent.idempotency_key.scoped("universe-stage")?,
            )?)
            .await?;
        if let Err(error) = self.blob_store.append_chunk(scope, &staged, manifest).await {
            let _ = self.blob_store.discard_stage(scope, &staged).await;
            return Err(error);
        }
        let staged_blob = StagedSnapshotBlob::new(
            SnapshotBlobRole::UniverseMembersManifest,
            VerifyBlobStage::new(scope.clone(), staged, content_hash, size)?,
        );
        let verification = staged_blob.verification().clone();
        let verified = match self.blob_store.verify_and_promote(verification).await {
            Ok(value) => value,
            Err(error) => {
                let _ = self
                    .blob_store
                    .discard_stage(scope, staged_blob.verification().staged())
                    .await;
                return Err(error);
            }
        };
        let proof = VerifiedSnapshotProof::universe(VerifiedSnapshotBlob::from_staged(
            staged_blob,
            verified,
        )?)?;
        let idempotency_key = intent.idempotency_key.scoped("universe-metadata")?;
        let published = if let Some(context) = change_context {
            self.snapshots
                .publish_governed(GovernedPublishSnapshot::administrator_universe(
                    context,
                    snapshot,
                    proof,
                    idempotency_key,
                )?)
                .await?
        } else {
            self.snapshots
                .publish_verified_manifest(PublishSnapshot::new(
                    SnapshotValue::Universe(snapshot),
                    proof,
                    idempotency_key,
                )?)
                .await?
        };
        match published {
            SnapshotValue::Universe(snapshot) => Ok(snapshot),
            SnapshotValue::Data(_)
            | SnapshotValue::DataHealthThresholdProfile(_)
            | SnapshotValue::Position(_) => Err(validation_error()),
        }
    }
}

pub struct SnapshotUseCase<'a> {
    snapshots: &'a dyn SnapshotRepository,
}

impl<'a> SnapshotUseCase<'a> {
    #[must_use]
    pub const fn new(snapshots: &'a dyn SnapshotRepository) -> Self {
        Self { snapshots }
    }

    /// Reads one immutable snapshot under an exact access scope.
    ///
    /// # Errors
    ///
    /// Returns a classified repository or integrity error.
    pub async fn get(
        &self,
        scope: &AccessScope,
        snapshot_id: Ulid,
    ) -> Result<Option<SnapshotValue>, ApplicationError> {
        self.snapshots.get_by_id(scope, snapshot_id).await
    }
}

impl<'a> PublishDataSnapshot<'a> {
    #[must_use]
    pub fn new(blob_store: &'a dyn BlobStore, snapshots: &'a dyn SnapshotRepository) -> Self {
        Self {
            blob_store,
            snapshots,
        }
    }

    /// Stages, verifies, promotes, and publishes both required `DataSnapshot` payloads.
    ///
    /// # Errors
    ///
    /// Returns a classified application error without publishing metadata unless both immutable
    /// payloads have been promoted and bound to the existing two-role snapshot proof.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        payloads: DataSnapshotPayloads,
    ) -> Result<DataSnapshot, ApplicationError> {
        self.execute_inner(scope, payloads, None).await
    }

    /// Publishes an exact authorized canonical import with one atomic metadata/change commit.
    ///
    /// # Errors
    ///
    /// Returns a classified request evidence, blob, hash, lineage, or repository error.
    pub async fn execute_governed_import(
        &self,
        replay_request: CanonicalImportReplayRequest,
        payloads: DataSnapshotPayloads,
    ) -> Result<DataSnapshot, ApplicationError> {
        replay_request.validate_snapshot(payloads.snapshot())?;
        payloads
            .import_evidence
            .as_ref()
            .ok_or_else(|| {
                ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
            })?
            .validate_request(&replay_request)?;
        if replay_request.idempotency_key() != payloads.idempotency_key() {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        let scope = replay_request
            .change_context()
            .principal()
            .access_scope()
            .clone();
        self.execute_inner(&scope, payloads, Some(replay_request))
            .await
    }

    /// Probes replay identity before the caller selects or reads an external source adapter.
    ///
    /// # Errors
    ///
    /// Returns immutable conflict or integrity failure for any stored request evidence drift.
    pub async fn probe_replay(
        &self,
        request: &CanonicalImportReplayRequest,
    ) -> Result<Option<CanonicalImportReplay>, ApplicationError> {
        self.snapshots.probe_canonical_import_replay(request).await
    }

    async fn execute_inner(
        &self,
        scope: &AccessScope,
        payloads: DataSnapshotPayloads,
        governance: Option<CanonicalImportReplayRequest>,
    ) -> Result<DataSnapshot, ApplicationError> {
        scope.authorize(payloads.snapshot.owner())?;
        require_payload(&payloads.parquet, payloads.snapshot.content_hash())?;
        require_payload(&payloads.manifest, payloads.snapshot.manifest_hash())?;

        let parquet = self
            .stage(
                scope,
                payloads.snapshot.owner(),
                payloads.parquet,
                payloads.idempotency_key.scoped("parquet-stage")?,
                SnapshotBlobRole::DataParquet,
            )
            .await?;
        let manifest = match self
            .stage(
                scope,
                payloads.snapshot.owner(),
                payloads.manifest,
                payloads.idempotency_key.scoped("manifest-stage")?,
                SnapshotBlobRole::DataManifest,
            )
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = self
                    .blob_store
                    .discard_stage(scope, parquet.verification().staged())
                    .await;
                return Err(error);
            }
        };

        let staged = StagedSnapshotProof::data(parquet, manifest)?;
        let verified = self.promote(scope, staged).await?;
        let idempotency_key = payloads.idempotency_key.scoped("metadata")?;
        let published = if let Some(replay_request) = governance {
            self.snapshots
                .publish_governed(GovernedPublishSnapshot::authorized_import(
                    replay_request,
                    payloads.snapshot,
                    verified,
                    idempotency_key,
                )?)
                .await?
        } else {
            self.snapshots
                .publish_verified_manifest(PublishSnapshot::new(
                    SnapshotValue::Data(payloads.snapshot),
                    verified,
                    idempotency_key,
                )?)
                .await?
        };
        match published {
            SnapshotValue::Data(snapshot) => Ok(snapshot),
            SnapshotValue::DataHealthThresholdProfile(_)
            | SnapshotValue::Universe(_)
            | SnapshotValue::Position(_) => Err(validation_error()),
        }
    }

    async fn stage(
        &self,
        scope: &AccessScope,
        owner: &ficant_domain::primitives::OwnerRef,
        bytes: Vec<u8>,
        idempotency_key: IdempotencyKey,
        role: SnapshotBlobRole,
    ) -> Result<StagedSnapshotBlob, ApplicationError> {
        let size = u64::try_from(bytes.len()).map_err(|_| validation_error())?;
        let expected_hash = ContentHash::digest(&bytes);
        let staged = self
            .blob_store
            .begin_stage(BeginBlobStage::new(
                scope.clone(),
                owner.clone(),
                size,
                idempotency_key,
            )?)
            .await?;
        if let Err(error) = self.blob_store.append_chunk(scope, &staged, bytes).await {
            let _ = self.blob_store.discard_stage(scope, &staged).await;
            return Err(error);
        }
        Ok(StagedSnapshotBlob::new(
            role,
            VerifyBlobStage::new(scope.clone(), staged, expected_hash, size)?,
        ))
    }

    async fn promote(
        &self,
        scope: &AccessScope,
        staged: StagedSnapshotProof,
    ) -> Result<VerifiedSnapshotProof, ApplicationError> {
        let parquet = staged
            .get(SnapshotBlobRole::DataParquet)
            .cloned()
            .ok_or_else(validation_error)?;
        let manifest = staged
            .get(SnapshotBlobRole::DataManifest)
            .cloned()
            .ok_or_else(validation_error)?;
        let parquet_verified = self
            .blob_store
            .verify_and_promote(parquet.verification().clone())
            .await?;
        let parquet = VerifiedSnapshotBlob::from_staged(parquet, parquet_verified)?;
        let manifest_verified = match self
            .blob_store
            .verify_and_promote(manifest.verification().clone())
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = self
                    .blob_store
                    .discard_stage(scope, manifest.verification().staged())
                    .await;
                return Err(error);
            }
        };
        let manifest = VerifiedSnapshotBlob::from_staged(manifest, manifest_verified)?;
        VerifiedSnapshotProof::data(parquet, manifest)
    }
}

fn require_payload(bytes: &[u8], expected: &ContentHash) -> Result<(), ApplicationError> {
    if bytes.is_empty() {
        return Err(validation_error());
    }
    expected
        .verify(bytes)
        .map_err(|_| ApplicationError::new(ApplicationErrorCategory::HashMismatch, false))
}

fn validation_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

/// Verifies the canonical v1 Universe manifest and returns its bound actor evidence.
///
/// # Errors
///
/// Returns hash mismatch when the bytes do not match the snapshot content identity, and
/// validation failure when any canonical field, member, lineage item, or trailing byte differs.
pub fn verify_universe_snapshot_manifest(
    snapshot: &UniverseSnapshot,
    manifest: &[u8],
) -> Result<Ulid, ApplicationError> {
    snapshot
        .content_hash()
        .verify(manifest)
        .map_err(|_| ApplicationError::new(ApplicationErrorCategory::HashMismatch, false))?;
    let prefix = b"ficant-universe-members-manifest/v1\0";
    let Some(mut cursor) = manifest.starts_with(prefix).then_some(prefix.len()) else {
        return Err(validation_error());
    };
    expect_manifest_token(manifest, &mut cursor, snapshot.id().as_str().as_bytes())?;
    expect_manifest_token(
        manifest,
        &mut cursor,
        snapshot.owner().tenant_id().as_str().as_bytes(),
    )?;
    expect_manifest_token(
        manifest,
        &mut cursor,
        snapshot.owner().owner_id().as_str().as_bytes(),
    )?;
    expect_manifest_token(manifest, &mut cursor, snapshot.filter_digest().as_bytes())?;
    let actor = manifest_token(manifest, &mut cursor)?;
    let actor = std::str::from_utf8(actor)
        .ok()
        .and_then(|value| Ulid::new(value).ok())
        .ok_or_else(validation_error)?;
    for member in snapshot.instrument_versions() {
        expect_manifest_token(manifest, &mut cursor, member.id().as_str().as_bytes())?;
        expect_manifest_token(manifest, &mut cursor, &member.version().get().to_be_bytes())?;
    }
    for reference in snapshot.lineage() {
        expect_manifest_token(
            manifest,
            &mut cursor,
            reference.object_id().as_str().as_bytes(),
        )?;
        expect_manifest_token(
            manifest,
            &mut cursor,
            &reference
                .version()
                .map_or(0_u64, ficant_domain::primitives::Version::get)
                .to_be_bytes(),
        )?;
        expect_manifest_token(
            manifest,
            &mut cursor,
            reference
                .content_hash()
                .map_or(&[][..], |hash| hash.as_bytes().as_slice()),
        )?;
    }
    if cursor != manifest.len() {
        return Err(validation_error());
    }
    Ok(actor)
}

fn manifest_token<'a>(
    manifest: &'a [u8],
    cursor: &mut usize,
) -> Result<&'a [u8], ApplicationError> {
    let length_end = cursor.checked_add(8).ok_or_else(validation_error)?;
    let length_bytes: [u8; 8] = manifest
        .get(*cursor..length_end)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(validation_error)?;
    let length =
        usize::try_from(u64::from_be_bytes(length_bytes)).map_err(|_| validation_error())?;
    let value_end = length_end
        .checked_add(length)
        .ok_or_else(validation_error)?;
    let value = manifest
        .get(length_end..value_end)
        .ok_or_else(validation_error)?;
    *cursor = value_end;
    Ok(value)
}

fn expect_manifest_token(
    manifest: &[u8],
    cursor: &mut usize,
    expected: &[u8],
) -> Result<(), ApplicationError> {
    if manifest_token(manifest, cursor)? == expected {
        Ok(())
    } else {
        Err(validation_error())
    }
}

fn universe_manifest_bytes(intent: &UniverseSnapshotIntent) -> Vec<u8> {
    let mut bytes = b"ficant-universe-members-manifest/v1\0".to_vec();
    append_token(&mut bytes, intent.snapshot_id.as_str().as_bytes());
    append_token(&mut bytes, intent.owner.tenant_id().as_str().as_bytes());
    append_token(&mut bytes, intent.owner.owner_id().as_str().as_bytes());
    append_token(&mut bytes, intent.filter_digest.as_bytes());
    append_token(&mut bytes, intent.actor_id.as_str().as_bytes());
    for member in &intent.instrument_versions {
        append_token(&mut bytes, member.id().as_str().as_bytes());
        append_token(&mut bytes, &member.version().get().to_be_bytes());
    }
    for reference in &intent.lineage {
        append_token(&mut bytes, reference.object_id().as_str().as_bytes());
        append_token(
            &mut bytes,
            &reference
                .version()
                .map_or(0_u64, ficant_domain::primitives::Version::get)
                .to_be_bytes(),
        );
        append_token(
            &mut bytes,
            reference
                .content_hash()
                .map_or(&[][..], |hash| hash.as_bytes().as_slice()),
        );
    }
    bytes
}

fn append_token(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(
        &u64::try_from(value.len())
            .expect("universe manifest field length fits u64")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{NaiveDate, TimeZone, Utc};
    use ficant_domain::governance::ChangeJustification;
    use ficant_domain::primitives::Version;
    use ficant_domain::research::DataSnapshotInput;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn universe_manifest_round_trip_returns_actor_and_rejects_trailing_bytes() {
        let owner = OwnerRef::new(id('T'), id('P'));
        let members = vec![VersionRef::new(id('K'), Version::new(7).unwrap())];
        let lineage = vec![
            LineageRef::new(
                id('J'),
                Some(Version::new(3).unwrap()),
                Some(ContentHash::digest(b"lineage")),
            )
            .unwrap(),
        ];
        let actor = id('A');
        let intent = UniverseSnapshotIntent::new(
            id('H'),
            owner.clone(),
            members.clone(),
            ContentHash::digest(b"filter"),
            lineage.clone(),
            actor.clone(),
            IdempotencyKey::new("universe-manifest-round-trip").unwrap(),
        )
        .unwrap();
        let manifest = universe_manifest_bytes(&intent);
        let snapshot = UniverseSnapshot::new(
            intent.snapshot_id.clone(),
            owner.clone(),
            members.clone(),
            intent.filter_digest.clone(),
            ContentHash::digest(&manifest),
            lineage.clone(),
        )
        .unwrap();
        assert_eq!(
            verify_universe_snapshot_manifest(&snapshot, &manifest).unwrap(),
            actor
        );

        let mut trailing = manifest;
        trailing.push(0);
        let tampered = UniverseSnapshot::new(
            snapshot.id().clone(),
            owner,
            members,
            snapshot.filter_digest().clone(),
            ContentHash::digest(&trailing),
            lineage,
        )
        .unwrap();
        assert_eq!(
            verify_universe_snapshot_manifest(&tampered, &trailing)
                .unwrap_err()
                .category(),
            ApplicationErrorCategory::ValidationFailed
        );
        assert_eq!(snapshot.lineage().len(), 1);
        assert_eq!(
            snapshot.content_hash(),
            &ContentHash::digest(&trailing[..trailing.len() - 1])
        );
    }

    #[tokio::test]
    async fn replay_probe_short_circuits_adapter_and_blob_io_counters() {
        let owner = OwnerRef::new(id('T'), id('P'));
        let authorization = VersionRef::new(id('V'), Version::new(1).unwrap());
        let authorization_hash = ContentHash::digest(b"authorization");
        let mapping_id = id('M');
        let mapping_hash = ContentHash::digest(b"mapping");
        let calendar = VersionRef::new(id('C'), Version::new(1).unwrap());
        let unit = VersionRef::new(id('N'), Version::new(1).unwrap());
        let principal = crate::ports::AuthorizedPrincipal::new(
            "replay-researcher".to_owned(),
            id('A'),
            owner.tenant_id().clone(),
            vec![owner.owner_id().clone()],
            PlatformRole::Researcher,
            vec![crate::ports::DATA_SOURCE_IMPORT_SCOPE.to_owned()],
            ContentHash::digest(b"credential"),
        )
        .unwrap();
        let request = CanonicalImportReplayRequest::new(
            FoundationChangeContext::authorized_import(
                principal,
                ChangeJustification::for_authorized_import("replay existing import").unwrap(),
                id('K'),
                market_time(13),
            )
            .unwrap(),
            owner.clone(),
            id('B'),
            authorization.clone(),
            authorization_hash.clone(),
            mapping_id.clone(),
            mapping_hash.clone(),
            calendar.clone(),
            unit.clone(),
            market_time(12),
            market_time(13),
            IdempotencyKey::new("canonical-replay").unwrap(),
        )
        .unwrap();
        let snapshot = DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: request.target_snapshot_id().clone(),
            owner,
            visible_at: request.visible_at().clone(),
            as_of: request.as_of().clone(),
            schema_hash: ContentHash::digest(b"schema"),
            manifest_hash: ContentHash::digest(b"manifest"),
            blob_content_hash: ContentHash::digest(b"parquet"),
            lineage: vec![
                LineageRef::content_addressed(mapping_id, mapping_hash),
                LineageRef::new(
                    authorization.id().clone(),
                    Some(authorization.version()),
                    Some(authorization_hash.clone()),
                )
                .unwrap(),
                LineageRef::versioned(calendar.id().clone(), calendar.version()),
                LineageRef::versioned(unit.id().clone(), unit.version()),
            ],
        })
        .unwrap();
        let replay = CanonicalImportReplay::verified(
            &request,
            snapshot,
            request.change_context().principal().actor_id().clone(),
            authorization,
            authorization_hash,
        )
        .unwrap();
        let repository = ReplayRepository {
            replay,
            probes: AtomicUsize::new(0),
        };
        let blob_store = CountingBlobStore(AtomicUsize::new(0));
        let adapter_reads = AtomicUsize::new(0);
        let found = PublishDataSnapshot::new(&blob_store, &repository)
            .probe_replay(&request)
            .await
            .unwrap();
        if found.is_none() {
            adapter_reads.fetch_add(1, Ordering::SeqCst);
        }
        assert!(found.is_some());
        assert_eq!(repository.probes.load(Ordering::SeqCst), 1);
        assert_eq!(adapter_reads.load(Ordering::SeqCst), 0);
        assert_eq!(blob_store.0.load(Ordering::SeqCst), 0);
    }

    struct ReplayRepository {
        replay: CanonicalImportReplay,
        probes: AtomicUsize,
    }

    #[async_trait]
    impl SnapshotRepository for ReplayRepository {
        async fn probe_canonical_import_replay(
            &self,
            _request: &CanonicalImportReplayRequest,
        ) -> Result<Option<CanonicalImportReplay>, ApplicationError> {
            self.probes.fetch_add(1, Ordering::SeqCst);
            Ok(Some(self.replay.clone()))
        }

        async fn publish_verified_manifest(
            &self,
            _command: PublishSnapshot,
        ) -> Result<SnapshotValue, ApplicationError> {
            Err(validation_error())
        }

        async fn get_by_id(
            &self,
            _scope: &AccessScope,
            _snapshot_id: Ulid,
        ) -> Result<Option<SnapshotValue>, ApplicationError> {
            Err(validation_error())
        }
    }

    struct CountingBlobStore(AtomicUsize);

    #[async_trait]
    impl BlobStore for CountingBlobStore {
        async fn begin_stage(
            &self,
            _command: BeginBlobStage,
        ) -> Result<crate::ports::StagedBlobRef, ApplicationError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(validation_error())
        }

        async fn append_chunk(
            &self,
            _scope: &AccessScope,
            _staged: &crate::ports::StagedBlobRef,
            _chunk: Vec<u8>,
        ) -> Result<(), ApplicationError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(validation_error())
        }

        async fn verify_and_promote(
            &self,
            _command: VerifyBlobStage,
        ) -> Result<crate::ports::VerifiedBlobRef, ApplicationError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(validation_error())
        }

        async fn discard_stage(
            &self,
            _scope: &AccessScope,
            _staged: &crate::ports::StagedBlobRef,
        ) -> Result<(), ApplicationError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(validation_error())
        }
    }

    fn market_time(hour: u32) -> ficant_domain::primitives::MarketTime {
        ficant_domain::primitives::MarketTime::new(
            Utc.with_ymd_and_hms(2026, 8, 13, hour, 0, 0).unwrap(),
            "Asia/Shanghai",
            NaiveDate::from_ymd_opt(2026, 8, 13).unwrap(),
        )
        .unwrap()
    }

    fn id(suffix: char) -> Ulid {
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
    }
}
