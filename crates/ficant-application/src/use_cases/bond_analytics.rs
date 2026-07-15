use ficant_domain::analytics::{AnalyticsError, BondAnalyticsInput, BondAnalyticsResult};
use ficant_domain::primitives::{LineageRef, Ulid};
use ficant_domain::research::{Artifact, ArtifactKind};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged};

use crate::ports::{
    AccessScope, ApplicationResult, ArtifactRepository, BeginBlobStage, BlobStore,
    BondAnalyticsArtifactCodec, BondAnalyticsEngine, IdempotencyKey, IntegrityEventSink,
    PublishArtifact, RequiredVerifiedBlobRead, SafeTraceContext, VerifiedBlobReader,
    VerifiedBlobRole, VerifiedReadResourceKind, VerifyBlobStage,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

pub const BOND_ANALYTICS_MEDIA_TYPE: &str =
    "application/vnd.apache.arrow.file; profile=ficant.bond-analytics.v1";

pub struct CalculateBondAnalytics<'a> {
    engine: &'a dyn BondAnalyticsEngine,
}

pub struct PublishBondAnalytics<'a> {
    engine: &'a dyn BondAnalyticsEngine,
    codec: &'a dyn BondAnalyticsArtifactCodec,
    blobs: &'a dyn BlobStore,
    artifacts: &'a dyn ArtifactRepository,
}

impl<'a> PublishBondAnalytics<'a> {
    #[must_use]
    pub const fn new(
        engine: &'a dyn BondAnalyticsEngine,
        codec: &'a dyn BondAnalyticsArtifactCodec,
        blobs: &'a dyn BlobStore,
        artifacts: &'a dyn ArtifactRepository,
    ) -> Self {
        Self {
            engine,
            codec,
            blobs,
            artifacts,
        }
    }

    /// Calculates, encodes, stages, verifies, and publishes one immutable Generic Artifact.
    ///
    /// # Errors
    ///
    /// Returns an application error without publishing metadata when any prerequisite, calculation,
    /// encoding, staging, verification, or repository operation fails.
    pub async fn execute(
        &self,
        scope: AccessScope,
        artifact_id: Ulid,
        input: &BondAnalyticsInput,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Artifact> {
        scope.authorize(input.owner())?;
        let result = CalculateBondAnalytics::new(self.engine).execute(input)?;
        let encoded = self.codec.encode(&result).map_err(map_analytics_error)?;
        let expected_hash = encoded.content_hash().clone();
        let expected_size = encoded.size();
        let stage = self
            .blobs
            .begin_stage(BeginBlobStage::new(
                scope.clone(),
                input.owner().clone(),
                expected_size,
                idempotency_key.clone(),
            )?)
            .await?;
        if let Err(error) = self
            .blobs
            .append_chunk(&scope, &stage, encoded.into_bytes())
            .await
        {
            let _ = self.blobs.discard_stage(&scope, &stage).await;
            return Err(error);
        }
        let verification = VerifyBlobStage::new(
            scope.clone(),
            stage.clone(),
            expected_hash.clone(),
            expected_size,
        )?;
        let verified = match self.blobs.verify_and_promote(verification).await {
            Ok(verified) => verified,
            Err(error) => {
                let _ = self.blobs.discard_stage(&scope, &stage).await;
                return Err(error);
            }
        };
        let artifact = Artifact::new(
            artifact_id,
            input.owner().clone(),
            ArtifactKind::Generic,
            BOND_ANALYTICS_MEDIA_TYPE,
            expected_hash,
            expected_size,
            analytics_lineage(input)?,
        )
        .map_err(map_domain_error)?;
        self.artifacts
            .publish_verified_blob(PublishArtifact::new(artifact, verified, idempotency_key)?)
            .await
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondAnalyticsReplay {
    artifact: Artifact,
    stored: BondAnalyticsResult,
    recalculated: BondAnalyticsResult,
}

impl BondAnalyticsReplay {
    #[must_use]
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }

    #[must_use]
    pub fn stored(&self) -> &BondAnalyticsResult {
        &self.stored
    }

    #[must_use]
    pub fn recalculated(&self) -> &BondAnalyticsResult {
        &self.recalculated
    }
}

pub struct ReplayBondAnalytics<'a> {
    engine: &'a dyn BondAnalyticsEngine,
    codec: &'a dyn BondAnalyticsArtifactCodec,
    artifacts: &'a dyn ArtifactRepository,
    reader: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
}

impl<'a> ReplayBondAnalytics<'a> {
    #[must_use]
    pub const fn new(
        engine: &'a dyn BondAnalyticsEngine,
        codec: &'a dyn BondAnalyticsArtifactCodec,
        artifacts: &'a dyn ArtifactRepository,
        reader: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
    ) -> Self {
        Self {
            engine,
            codec,
            artifacts,
            reader,
            integrity_events,
        }
    }

    /// Reads and verifies an existing payload, then recalculates and compares exact canonical bytes.
    ///
    /// # Errors
    ///
    /// Returns an application error for missing metadata, authorization or lineage drift, payload
    /// integrity loss, decode failure, calculation failure, or replay mismatch.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        artifact_id: Ulid,
        expected_input: &BondAnalyticsInput,
        trace: SafeTraceContext,
    ) -> ApplicationResult<BondAnalyticsReplay> {
        scope.authorize(expected_input.owner())?;
        let artifact = self
            .artifacts
            .get_metadata(scope, artifact_id.clone())
            .await?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
        validate_analytics_artifact(scope, &artifact_id, &artifact, expected_input)?;
        let request = RequiredVerifiedBlobRead::new(
            scope.clone(),
            artifact.owner().clone(),
            VerifiedReadResourceKind::Artifact,
            artifact.id().clone(),
            VerifiedBlobRole::ArtifactPayload,
            artifact.content_hash().clone(),
            artifact.blob_size(),
            trace,
        )?;
        let payload = self
            .reader
            .read_required(&request, self.integrity_events)
            .await?;
        let stored = self
            .codec
            .decode(payload.bytes(), expected_input)
            .map_err(map_analytics_error)?;
        let recalculated = CalculateBondAnalytics::new(self.engine).execute(expected_input)?;
        if stored != recalculated {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        let replay = self
            .codec
            .encode(&recalculated)
            .map_err(map_analytics_error)?;
        if replay.content_hash() != artifact.content_hash()
            || replay.size() != artifact.blob_size()
            || replay.bytes() != payload.bytes()
        {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        Ok(BondAnalyticsReplay {
            artifact,
            stored,
            recalculated,
        })
    }
}

fn analytics_lineage(input: &BondAnalyticsInput) -> ApplicationResult<Vec<LineageRef>> {
    Ok(vec![
        LineageRef::versioned(
            input.bond().version_ref().id().clone(),
            input.bond().version_ref().version(),
        ),
        LineageRef::new(
            input.rule_pack().version_ref().id().clone(),
            Some(input.rule_pack().version_ref().version()),
            Some(input.rule_pack().content_hash().clone()),
        )
        .map_err(map_domain_error)?,
        LineageRef::content_addressed(
            input.snapshot().version_ref().id().clone(),
            input.snapshot().content_hash().clone(),
        ),
    ])
}

fn validate_analytics_artifact(
    scope: &AccessScope,
    artifact_id: &Ulid,
    artifact: &Artifact,
    input: &BondAnalyticsInput,
) -> ApplicationResult<()> {
    scope.authorize(artifact.owner())?;
    if artifact.id() != artifact_id
        || artifact.owner() != input.owner()
        || artifact.kind() != ArtifactKind::Generic
        || artifact.media_type() != BOND_ANALYTICS_MEDIA_TYPE
        || artifact.lineage() != analytics_lineage(input)?.as_slice()
    {
        return Err(map_domain_error(DomainErrorCode::BrokenLineage));
    }
    Ok(())
}

impl<'a> CalculateBondAnalytics<'a> {
    #[must_use]
    pub const fn new(engine: &'a dyn BondAnalyticsEngine) -> Self {
        Self { engine }
    }

    /// Calculates and validates one analytics result without performing mutable I/O.
    ///
    /// # Errors
    ///
    /// Returns an application error when the engine fails or its result violates the input binding.
    pub fn execute(&self, input: &BondAnalyticsInput) -> ApplicationResult<BondAnalyticsResult> {
        let result = self.engine.calculate(input).map_err(map_analytics_error)?;
        result.validate_against(input).map_err(map_domain_error)?;
        Ok(result)
    }
}

#[must_use]
pub fn map_analytics_error(error: AnalyticsError) -> ApplicationError {
    match error {
        AnalyticsError::InvalidInput
        | AnalyticsError::AbiMismatch
        | AnalyticsError::BufferTooSmall
        | AnalyticsError::NoBracket
        | AnalyticsError::NotConverged
        | AnalyticsError::NonFinite
        | AnalyticsError::CalendarCoverageMissing => {
            ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
        }
        AnalyticsError::Internal => {
            ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
        }
    }
}
