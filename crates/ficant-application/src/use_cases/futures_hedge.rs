use ficant_domain::futures_hedge::{FuturesHedgeInput, FuturesHedgeResult};
use ficant_domain::primitives::{LineageRef, Ulid};
use ficant_domain::research::{Artifact, ArtifactKind};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged};

use crate::ports::{
    AccessScope, ApplicationResult, ArtifactRepository, BeginBlobStage, BlobStore,
    FuturesHedgeArtifactCodec, FuturesHedgeEngine, IdempotencyKey, IntegrityEventSink,
    PublishArtifact, RequiredVerifiedBlobRead, SafeTraceContext, VerifiedBlobReader,
    VerifiedBlobRole, VerifiedReadResourceKind, VerifyBlobStage,
};
use crate::use_cases::bond_analytics::map_analytics_error;
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

pub const FUTURES_HEDGE_MEDIA_TYPE: &str =
    "application/vnd.apache.arrow.file; profile=ficant.cgb-futures-hedge.v1";

pub struct CalculateFuturesHedge<'a> {
    engine: &'a dyn FuturesHedgeEngine,
}

pub struct PublishFuturesHedge<'a> {
    engine: &'a dyn FuturesHedgeEngine,
    codec: &'a dyn FuturesHedgeArtifactCodec,
    blobs: &'a dyn BlobStore,
    artifacts: &'a dyn ArtifactRepository,
}

impl<'a> PublishFuturesHedge<'a> {
    #[must_use]
    pub const fn new(
        engine: &'a dyn FuturesHedgeEngine,
        codec: &'a dyn FuturesHedgeArtifactCodec,
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

    /// Calculates, encodes, verifies, and publishes one immutable hedge Artifact.
    ///
    /// # Errors
    ///
    /// Returns without metadata publication when authorization, calculation, staging,
    /// verification, lineage, or repository publication fails.
    pub async fn execute(
        &self,
        scope: AccessScope,
        artifact_id: Ulid,
        input: &FuturesHedgeInput,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Artifact> {
        scope.authorize(input.owner())?;
        let result = CalculateFuturesHedge::new(self.engine).execute(input)?;
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
            FUTURES_HEDGE_MEDIA_TYPE,
            expected_hash,
            expected_size,
            futures_hedge_lineage(input)?,
        )
        .map_err(map_domain_error)?;
        self.artifacts
            .publish_verified_blob(PublishArtifact::new(artifact, verified, idempotency_key)?)
            .await
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesHedgeReplay {
    artifact: Artifact,
    stored: FuturesHedgeResult,
    recalculated: FuturesHedgeResult,
}

impl FuturesHedgeReplay {
    #[must_use]
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }
    #[must_use]
    pub fn stored(&self) -> &FuturesHedgeResult {
        &self.stored
    }
    #[must_use]
    pub fn recalculated(&self) -> &FuturesHedgeResult {
        &self.recalculated
    }
}

pub struct ReplayFuturesHedge<'a> {
    engine: &'a dyn FuturesHedgeEngine,
    codec: &'a dyn FuturesHedgeArtifactCodec,
    artifacts: &'a dyn ArtifactRepository,
    reader: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
}

impl<'a> ReplayFuturesHedge<'a> {
    #[must_use]
    pub const fn new(
        engine: &'a dyn FuturesHedgeEngine,
        codec: &'a dyn FuturesHedgeArtifactCodec,
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

    /// Reads verified bytes, checks exact lineage, and deterministically recalculates the hedge.
    ///
    /// # Errors
    ///
    /// Returns for authorization, lineage, integrity, decoding, or replay drift.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        artifact_id: Ulid,
        expected_input: &FuturesHedgeInput,
        trace: SafeTraceContext,
    ) -> ApplicationResult<FuturesHedgeReplay> {
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
        let recalculated = CalculateFuturesHedge::new(self.engine).execute(expected_input)?;
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
        Ok(FuturesHedgeReplay {
            artifact,
            stored,
            recalculated,
        })
    }
}

fn futures_hedge_lineage(input: &FuturesHedgeInput) -> ApplicationResult<Vec<LineageRef>> {
    Ok(vec![
        LineageRef::content_addressed(
            input.target_risk_artifact().version_ref().id().clone(),
            input.target_risk_artifact().content_hash().clone(),
        ),
        LineageRef::content_addressed(
            input.delivery_artifact().version_ref().id().clone(),
            input.delivery_artifact().content_hash().clone(),
        ),
        LineageRef::content_addressed(
            input.ctd_analytics_artifact().version_ref().id().clone(),
            input.ctd_analytics_artifact().content_hash().clone(),
        ),
        LineageRef::versioned(
            input.futures_contract().version_ref().id().clone(),
            input.futures_contract().version_ref().version(),
        ),
        LineageRef::versioned(
            input.ctd_bond().version_ref().id().clone(),
            input.ctd_bond().version_ref().version(),
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

fn validate_artifact(
    scope: &AccessScope,
    artifact_id: &Ulid,
    artifact: &Artifact,
    input: &FuturesHedgeInput,
) -> ApplicationResult<()> {
    scope.authorize(artifact.owner())?;
    if artifact.id() != artifact_id
        || artifact.owner() != input.owner()
        || artifact.kind() != ArtifactKind::Generic
        || artifact.media_type() != FUTURES_HEDGE_MEDIA_TYPE
        || artifact.lineage() != futures_hedge_lineage(input)?.as_slice()
    {
        return Err(map_domain_error(DomainErrorCode::BrokenLineage));
    }
    Ok(())
}

impl<'a> CalculateFuturesHedge<'a> {
    #[must_use]
    pub const fn new(engine: &'a dyn FuturesHedgeEngine) -> Self {
        Self { engine }
    }

    /// Calculates one exact-input-bound CTD DV01 hedge.
    ///
    /// # Errors
    ///
    /// Returns a stable validation or analytics failure without side effects.
    pub fn execute(&self, input: &FuturesHedgeInput) -> ApplicationResult<FuturesHedgeResult> {
        let result = self.engine.calculate(input).map_err(map_analytics_error)?;
        result.validate_against(input).map_err(map_domain_error)?;
        Ok(result)
    }
}
