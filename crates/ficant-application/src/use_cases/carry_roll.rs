use ficant_domain::curves::{CarryRollInput, CarryRollResult};
use ficant_domain::primitives::{LineageRef, Ulid};
use ficant_domain::research::{Artifact, ArtifactKind};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged};

use crate::ports::{
    AccessScope, ApplicationResult, ArtifactRepository, BeginBlobStage, BlobStore,
    CarryRollArtifactCodec, CarryRollEngine, IdempotencyKey, IntegrityEventSink, PublishArtifact,
    RequiredVerifiedBlobRead, SafeTraceContext, VerifiedBlobReader, VerifiedBlobRole,
    VerifiedReadResourceKind, VerifyBlobStage,
};
use crate::use_cases::bond_analytics::map_analytics_error;
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

pub const CARRY_ROLL_MEDIA_TYPE: &str =
    "application/vnd.apache.arrow.file; profile=ficant.carry-roll.v1";

pub struct CalculateCarryRoll<'a> {
    engine: &'a dyn CarryRollEngine,
}

impl<'a> CalculateCarryRoll<'a> {
    #[must_use]
    pub const fn new(engine: &'a dyn CarryRollEngine) -> Self {
        Self { engine }
    }

    /// Calculates and validates one result without mutable I/O.
    ///
    /// # Errors
    ///
    /// Returns an application error when calculation or exact input binding fails.
    pub fn execute(&self, input: &CarryRollInput) -> ApplicationResult<CarryRollResult> {
        let result = self.engine.calculate(input).map_err(map_analytics_error)?;
        result.validate_against(input).map_err(map_domain_error)?;
        Ok(result)
    }
}

pub struct PublishCarryRoll<'a> {
    engine: &'a dyn CarryRollEngine,
    codec: &'a dyn CarryRollArtifactCodec,
    blobs: &'a dyn BlobStore,
    artifacts: &'a dyn ArtifactRepository,
}

impl<'a> PublishCarryRoll<'a> {
    #[must_use]
    pub const fn new(
        engine: &'a dyn CarryRollEngine,
        codec: &'a dyn CarryRollArtifactCodec,
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

    /// Calculates, encodes, verifies, and publishes one immutable Generic Artifact.
    ///
    /// # Errors
    ///
    /// Returns without metadata publication when any prerequisite or storage step fails.
    pub async fn execute(
        &self,
        scope: AccessScope,
        artifact_id: Ulid,
        input: &CarryRollInput,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Artifact> {
        scope.authorize(input.owner())?;
        let result = CalculateCarryRoll::new(self.engine).execute(input)?;
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
            CARRY_ROLL_MEDIA_TYPE,
            expected_hash,
            expected_size,
            carry_roll_lineage(input)?,
        )
        .map_err(map_domain_error)?;
        self.artifacts
            .publish_verified_blob(PublishArtifact::new(artifact, verified, idempotency_key)?)
            .await
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarryRollReplay {
    artifact: Artifact,
    stored: CarryRollResult,
    recalculated: CarryRollResult,
}

impl CarryRollReplay {
    #[must_use]
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }

    #[must_use]
    pub fn stored(&self) -> &CarryRollResult {
        &self.stored
    }

    #[must_use]
    pub fn recalculated(&self) -> &CarryRollResult {
        &self.recalculated
    }
}

pub struct ReplayCarryRoll<'a> {
    engine: &'a dyn CarryRollEngine,
    codec: &'a dyn CarryRollArtifactCodec,
    artifacts: &'a dyn ArtifactRepository,
    reader: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
}

impl<'a> ReplayCarryRoll<'a> {
    #[must_use]
    pub const fn new(
        engine: &'a dyn CarryRollEngine,
        codec: &'a dyn CarryRollArtifactCodec,
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

    /// Verifies stored bytes, decodes exact input binding, and recalculates canonical bytes.
    ///
    /// # Errors
    ///
    /// Returns an application error for authorization, lineage, integrity, or replay drift.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        artifact_id: Ulid,
        expected_input: &CarryRollInput,
        trace: SafeTraceContext,
    ) -> ApplicationResult<CarryRollReplay> {
        scope.authorize(expected_input.owner())?;
        let artifact = self
            .artifacts
            .get_metadata(scope, artifact_id.clone())
            .await?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
        validate_artifact(scope, &artifact_id, &artifact, expected_input)?;
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
        let recalculated = CalculateCarryRoll::new(self.engine).execute(expected_input)?;
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
        Ok(CarryRollReplay {
            artifact,
            stored,
            recalculated,
        })
    }
}

fn carry_roll_lineage(input: &CarryRollInput) -> ApplicationResult<Vec<LineageRef>> {
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
        LineageRef::content_addressed(
            input.curve().curve_snapshot().version_ref().id().clone(),
            input.curve().curve_snapshot().content_hash().clone(),
        ),
    ])
}

fn validate_artifact(
    scope: &AccessScope,
    artifact_id: &Ulid,
    artifact: &Artifact,
    input: &CarryRollInput,
) -> ApplicationResult<()> {
    scope.authorize(artifact.owner())?;
    if artifact.id() != artifact_id
        || artifact.owner() != input.owner()
        || artifact.kind() != ArtifactKind::Generic
        || artifact.media_type() != CARRY_ROLL_MEDIA_TYPE
        || artifact.lineage() != carry_roll_lineage(input)?.as_slice()
    {
        return Err(map_domain_error(DomainErrorCode::BrokenLineage));
    }
    Ok(())
}
