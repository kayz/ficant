use ficant_domain::primitives::{ContentHash, LineageRef, MarketTime, Ulid, VersionRef};
use ficant_domain::research::{
    DataHealthPriceEvidence, DataHealthPriceEvidenceInput, DataHealthReport, DataHealthReportInput,
    DataHealthThresholdProfile, evaluate_position_snapshot,
};
use ficant_domain::{ContentAddressed, Lineaged, VersionedDefinition};

use crate::ports::{
    AccessScope, ApplicationResult, BeginBlobStage, BlobStore, CanonicalSnapshotDecoder,
    DataHealthThresholdProfileRepository, DataSourceRepository, FoundationChangeContext,
    GovernedPublishSnapshot, IdempotencyKey, IntegrityEventSink, PositionSnapshotRepository,
    SafeTraceContext, SnapshotBlobRole, SnapshotRepository, SnapshotValue,
    SnapshotVerifiedReadMetadataRepository, StagedSnapshotBlob, StagedSnapshotProof,
    VerifiedBlobReader, VerifiedSnapshotBlob, VerifiedSnapshotProof, VerifyBlobStage,
};
use crate::use_cases::verified_reads::{VerifiedSnapshotRead, VerifiedSnapshotReader};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataHealthQuery {
    subject_ref: VersionRef,
    position_snapshot_id: Ulid,
    data_snapshot_id: Option<Ulid>,
    evaluated_at: MarketTime,
}

impl DataHealthQuery {
    #[must_use]
    pub fn new(
        subject_ref: VersionRef,
        position_snapshot_id: Ulid,
        data_snapshot_id: Option<Ulid>,
        evaluated_at: MarketTime,
    ) -> Self {
        Self {
            subject_ref,
            position_snapshot_id,
            data_snapshot_id,
            evaluated_at,
        }
    }

    #[must_use]
    pub fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }

    #[must_use]
    pub fn position_snapshot_id(&self) -> &Ulid {
        &self.position_snapshot_id
    }

    #[must_use]
    pub fn data_snapshot_id(&self) -> Option<&Ulid> {
        self.data_snapshot_id.as_ref()
    }

    #[must_use]
    pub fn evaluated_at(&self) -> &MarketTime {
        &self.evaluated_at
    }
}

#[derive(Clone, Debug)]
pub struct DataHealthThresholdProfilePayload {
    profile: DataHealthThresholdProfile,
    idempotency_key: IdempotencyKey,
}

impl DataHealthThresholdProfilePayload {
    /// Binds an immutable platform profile to one idempotent publication intent.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the canonical payload does not reproduce the declared
    /// content hash.
    pub fn new(
        profile: DataHealthThresholdProfile,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        let bytes = profile.canonical_bytes();
        if bytes.is_empty() || ContentHash::digest(&bytes) != *profile.content_hash() {
            return Err(validation());
        }
        Ok(Self {
            profile,
            idempotency_key,
        })
    }

    #[must_use]
    pub fn profile(&self) -> &DataHealthThresholdProfile {
        &self.profile
    }
}

pub struct PublishDataHealthThresholdProfile<'a> {
    blob_store: &'a dyn BlobStore,
    snapshots: &'a dyn SnapshotRepository,
}

impl<'a> PublishDataHealthThresholdProfile<'a> {
    /// Composes the verified-blob publication path.
    #[must_use]
    pub fn new(blob_store: &'a dyn BlobStore, snapshots: &'a dyn SnapshotRepository) -> Self {
        Self {
            blob_store,
            snapshots,
        }
    }

    /// Publishes the profile only after its canonical bytes have been staged and verified.
    ///
    /// # Errors
    ///
    /// Returns a safe application error for authorization, hash, storage, proof, lineage, or
    /// immutable-identity failures.
    pub async fn execute(
        &self,
        change_context: FoundationChangeContext,
        payload: DataHealthThresholdProfilePayload,
    ) -> ApplicationResult<DataHealthThresholdProfile> {
        let scope = change_context.principal().access_scope().clone();
        scope.authorize(payload.profile.owner())?;
        let bytes = payload.profile.canonical_bytes();
        let expected_hash = payload.profile.content_hash().clone();
        if ContentHash::digest(&bytes) != expected_hash {
            return Err(validation());
        }
        let size = u64::try_from(bytes.len()).map_err(|_| validation())?;
        let staged_id = self
            .blob_store
            .begin_stage(BeginBlobStage::new(
                scope.clone(),
                payload.profile.owner().clone(),
                size,
                payload
                    .idempotency_key
                    .scoped("data-health-profile-stage")?,
            )?)
            .await?;
        if let Err(error) = self
            .blob_store
            .append_chunk(&scope, &staged_id, bytes)
            .await
        {
            let _ = self.blob_store.discard_stage(&scope, &staged_id).await;
            return Err(error);
        }
        let staged = StagedSnapshotBlob::new(
            SnapshotBlobRole::DataHealthThresholdProfilePayload,
            VerifyBlobStage::new(scope.clone(), staged_id, expected_hash, size)?,
        );
        let _proof = StagedSnapshotProof::data_health_threshold_profile(staged.clone())?;
        let promoted = self
            .blob_store
            .verify_and_promote(staged.verification().clone())
            .await?;
        let proof = VerifiedSnapshotProof::data_health_threshold_profile(
            VerifiedSnapshotBlob::from_staged(staged, promoted)?,
        )?;
        let command = GovernedPublishSnapshot::administrator_data_health_threshold(
            change_context,
            payload.profile,
            proof,
            payload
                .idempotency_key
                .scoped("data-health-profile-metadata")?,
        )?;
        match self.snapshots.publish_governed(command).await? {
            SnapshotValue::DataHealthThresholdProfile(profile) => Ok(profile),
            SnapshotValue::Data(_) | SnapshotValue::Position(_) | SnapshotValue::Universe(_) => {
                Err(validation())
            }
        }
    }
}

/// Stateless, read-only data-health orchestration.
///
/// Its dependency set deliberately contains no calculation use case, engine, cache, clock,
/// journal, lease, or mutable repository.
pub struct GetDataHealthReport<'a> {
    positions: &'a dyn PositionSnapshotRepository,
    snapshot_metadata: &'a dyn SnapshotVerifiedReadMetadataRepository,
    blob_reader: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
    decoder: &'a dyn CanonicalSnapshotDecoder,
    data_sources: &'a dyn DataSourceRepository,
    threshold_profiles: &'a dyn DataHealthThresholdProfileRepository,
}

impl<'a> GetDataHealthReport<'a> {
    #[must_use]
    pub const fn new(
        positions: &'a dyn PositionSnapshotRepository,
        snapshot_metadata: &'a dyn SnapshotVerifiedReadMetadataRepository,
        blob_reader: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
        decoder: &'a dyn CanonicalSnapshotDecoder,
        data_sources: &'a dyn DataSourceRepository,
        threshold_profiles: &'a dyn DataHealthThresholdProfileRepository,
    ) -> Self {
        Self {
            positions,
            snapshot_metadata,
            blob_reader,
            integrity_events,
            decoder,
            data_sources,
            threshold_profiles,
        }
    }

    /// Reads the exact immutable inputs, derives one health evaluation, and returns its report.
    ///
    /// Integrity, identity, owner, version, and time drift remain fail-closed. Only health facts
    /// whose inputs remain trustworthy are represented as warnings.
    ///
    /// # Errors
    ///
    /// Returns a stable application error when any referenced object is missing, unauthorized,
    /// not yet visible, content-inconsistent, or cannot be decoded through the verified-read path.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        query: DataHealthQuery,
    ) -> ApplicationResult<DataHealthReport> {
        let snapshot = self
            .positions
            .get_position_snapshot(
                scope,
                query.position_snapshot_id.clone(),
                query.evaluated_at.clone(),
            )
            .await?
            .ok_or_else(not_found)?;
        scope.authorize(snapshot.owner())?;
        if snapshot.id() != &query.position_snapshot_id
            || snapshot.subject_ref() != &query.subject_ref
            || snapshot.visible_at().instant() > query.evaluated_at.instant()
        {
            return Err(lineage());
        }
        let threshold_profile = self
            .threshold_profiles
            .resolve_active(scope, snapshot.owner().clone(), query.evaluated_at.clone())
            .await?
            .ok_or_else(|| {
                ApplicationError::rule_pack_item_missing("platform.data_health.threshold_profile")
            })?;
        if threshold_profile.owner() != snapshot.owner()
            || threshold_profile.visible_at().instant() > query.evaluated_at.instant()
            || threshold_profile.effective_from().instant() > query.evaluated_at.instant()
            || threshold_profile.effective_to().instant() <= query.evaluated_at.instant()
        {
            return Err(lineage());
        }
        let evaluation =
            evaluate_position_snapshot(&snapshot, &threshold_profile, &query.evaluated_at)
                .map_err(map_domain_error)?;
        let price_evidence = match query.data_snapshot_id.as_ref() {
            Some(data_snapshot_id) => Some(
                self.read_price_evidence(scope, data_snapshot_id.clone(), &query)
                    .await?,
            ),
            None => None,
        };
        DataHealthReport::new(DataHealthReportInput {
            position_snapshot: snapshot,
            evaluated_at: query.evaluated_at,
            position_evaluation: evaluation,
            threshold_profile,
            price_evidence,
        })
        .map_err(map_domain_error)
    }

    async fn read_price_evidence(
        &self,
        scope: &AccessScope,
        data_snapshot_id: Ulid,
        query: &DataHealthQuery,
    ) -> ApplicationResult<DataHealthPriceEvidence> {
        let reader = VerifiedSnapshotReader::new(
            self.snapshot_metadata,
            self.blob_reader,
            self.integrity_events,
        );
        let read = reader
            .read(scope, data_snapshot_id.clone(), trace_for(query)?)
            .await?;
        let VerifiedSnapshotRead::Data {
            snapshot,
            parquet,
            manifest,
        } = read
        else {
            return Err(lineage());
        };
        scope.authorize(snapshot.owner())?;
        if snapshot.id() != &data_snapshot_id
            || snapshot.visible_at().instant() > query.evaluated_at.instant()
            || snapshot.as_of().instant() > query.evaluated_at.instant()
        {
            return Err(lineage());
        }
        let decoded = self
            .decoder
            .decode_quotes(&snapshot, parquet.bytes(), manifest.bytes())
            .await?;
        let source_ref = decoded.data_source().clone();
        let source = self
            .data_sources
            .get_exact(scope, source_ref.clone())
            .await?
            .ok_or_else(not_found)?;
        scope.authorize(source.owner())?;
        if source.owner() != snapshot.owner()
            || source.identity() != source_ref.id().as_str()
            || source.version() != source_ref.version().get()
        {
            return Err(lineage());
        }
        let record_count = u64::try_from(decoded.quotes().len()).map_err(|_| validation())?;
        let mut lineage = snapshot.lineage().to_vec();
        lineage.push(LineageRef::versioned(
            source_ref.id().clone(),
            source_ref.version(),
        ));
        DataHealthPriceEvidence::new(DataHealthPriceEvidenceInput {
            data_snapshot_id,
            owner: snapshot.owner().clone(),
            data_snapshot_content_hash: snapshot.content_hash().clone(),
            data_snapshot_manifest_hash: snapshot.manifest_hash().clone(),
            data_source_ref: source_ref,
            source_type: source.price_source_type(),
            record_count,
            visible_at: snapshot.visible_at().clone(),
            as_of: snapshot.as_of().clone(),
            lineage,
        })
        .map_err(map_domain_error)
    }
}

fn trace_for(query: &DataHealthQuery) -> ApplicationResult<SafeTraceContext> {
    let mut bytes = Vec::new();
    append(&mut bytes, query.subject_ref.id().as_str().as_bytes());
    append(&mut bytes, &query.subject_ref.version().get().to_be_bytes());
    append(&mut bytes, query.position_snapshot_id.as_str().as_bytes());
    if let Some(data_snapshot_id) = query.data_snapshot_id.as_ref() {
        append(&mut bytes, data_snapshot_id.as_str().as_bytes());
    }
    append(
        &mut bytes,
        &query.evaluated_at.instant().timestamp().to_be_bytes(),
    );
    append(
        &mut bytes,
        &query
            .evaluated_at
            .instant()
            .timestamp_subsec_nanos()
            .to_be_bytes(),
    );
    append(&mut bytes, b"platform-resolved-profile");
    let digest = ContentHash::digest(&bytes);
    let token =
        digest.as_bytes()[..16]
            .iter()
            .fold(String::with_capacity(32), |mut output, byte| {
                use std::fmt::Write as _;
                write!(output, "{byte:02x}").expect("writing to String cannot fail");
                output
            });
    SafeTraceContext::new(token)
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn lineage() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}
