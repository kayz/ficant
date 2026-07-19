use ficant_domain::futures_delivery::{
    FuturesDeliverableInput, FuturesDeliveryBasketResult, FuturesDeliveryResult,
};
use ficant_domain::primitives::{LineageRef, Ulid};
use ficant_domain::research::{Artifact, ArtifactKind};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged};

use crate::ports::{
    AccessScope, ApplicationResult, ArtifactRepository, BeginBlobStage, BlobStore,
    FuturesDeliveryArtifactCodec, FuturesDeliveryEngine, IdempotencyKey, IntegrityEventSink,
    PublishArtifact, RequiredVerifiedBlobRead, SafeTraceContext, VerifiedBlobReader,
    VerifiedBlobRole, VerifiedReadResourceKind, VerifyBlobStage,
};
use crate::use_cases::bond_analytics::map_analytics_error;
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

pub const FUTURES_DELIVERY_MEDIA_TYPE: &str =
    "application/vnd.apache.arrow.file; profile=ficant.cgb-futures-delivery.v1";

pub struct CalculateFuturesDeliveryBasket<'a> {
    engine: &'a dyn FuturesDeliveryEngine,
}

impl<'a> CalculateFuturesDeliveryBasket<'a> {
    #[must_use]
    pub const fn new(engine: &'a dyn FuturesDeliveryEngine) -> Self {
        Self { engine }
    }

    /// Calculates a homogeneous delivery basket and selects CTD by maximum IRR.
    ///
    /// # Errors
    ///
    /// Returns validation failure for an empty, duplicate, or mixed-contract basket and maps
    /// stable engine failures without publishing partial results.
    pub fn execute(
        &self,
        inputs: &[FuturesDeliverableInput],
    ) -> ApplicationResult<FuturesDeliveryBasketResult> {
        let Some(first) = inputs.first() else {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        };
        if inputs.iter().skip(1).any(|input| {
            input.owner() != first.owner()
                || input.futures_contract() != first.futures_contract()
                || input.rule_pack() != first.rule_pack()
                || input.snapshot() != first.snapshot()
                || input.valuation_at() != first.valuation_at()
                || input.purchase_date() != first.purchase_date()
                || input.delivery_month_first() != first.delivery_month_first()
                || input.delivery_date() != first.delivery_date()
                || input.product() != first.product()
                || input.futures_clean_price() != first.futures_clean_price()
                || input.financing_rate() != first.financing_rate()
        }) {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let candidates = inputs
            .iter()
            .map(|input| {
                let result = self.engine.calculate(input).map_err(map_analytics_error)?;
                result.validate_against(input).map_err(map_domain_error)?;
                Ok(result)
            })
            .collect::<ApplicationResult<Vec<_>>>()?;
        let ctd_index = select_ctd(&candidates);
        FuturesDeliveryBasketResult::new(candidates, ctd_index).map_err(map_domain_error)
    }
}

fn select_ctd(candidates: &[FuturesDeliveryResult]) -> usize {
    let mut best = 0;
    for index in 1..candidates.len() {
        let candidate = candidates[index].measures();
        let incumbent = candidates[best].measures();
        let candidate_id = candidates[index].input().bond().version_ref().id();
        let incumbent_id = candidates[best].input().bond().version_ref().id();
        if candidate.implied_repo_rate() > incumbent.implied_repo_rate()
            || (candidate.implied_repo_rate() == incumbent.implied_repo_rate()
                && (candidate.net_basis() < incumbent.net_basis()
                    || (candidate.net_basis() == incumbent.net_basis()
                        && candidate_id < incumbent_id)))
        {
            best = index;
        }
    }
    best
}

pub struct PublishFuturesDelivery<'a> {
    engine: &'a dyn FuturesDeliveryEngine,
    codec: &'a dyn FuturesDeliveryArtifactCodec,
    blobs: &'a dyn BlobStore,
    artifacts: &'a dyn ArtifactRepository,
}

impl<'a> PublishFuturesDelivery<'a> {
    #[must_use]
    pub const fn new(
        engine: &'a dyn FuturesDeliveryEngine,
        codec: &'a dyn FuturesDeliveryArtifactCodec,
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

    /// Calculates, encodes, verifies and publishes one immutable basket Artifact.
    ///
    /// # Errors
    ///
    /// Returns without metadata publication when authorization, calculation, staging,
    /// verification or repository publication fails.
    pub async fn execute(
        &self,
        scope: AccessScope,
        artifact_id: Ulid,
        inputs: &[FuturesDeliverableInput],
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Artifact> {
        let first = first_input(inputs)?;
        scope.authorize(first.owner())?;
        let result = CalculateFuturesDeliveryBasket::new(self.engine).execute(inputs)?;
        let encoded = self.codec.encode(&result).map_err(map_analytics_error)?;
        let expected_hash = encoded.content_hash().clone();
        let expected_size = encoded.size();
        let stage = self
            .blobs
            .begin_stage(BeginBlobStage::new(
                scope.clone(),
                first.owner().clone(),
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
            first.owner().clone(),
            ArtifactKind::Generic,
            FUTURES_DELIVERY_MEDIA_TYPE,
            expected_hash,
            expected_size,
            futures_delivery_lineage(inputs)?,
        )
        .map_err(map_domain_error)?;
        self.artifacts
            .publish_verified_blob(PublishArtifact::new(artifact, verified, idempotency_key)?)
            .await
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesDeliveryReplay {
    artifact: Artifact,
    stored: FuturesDeliveryBasketResult,
    recalculated: FuturesDeliveryBasketResult,
}

impl FuturesDeliveryReplay {
    #[must_use]
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }
    #[must_use]
    pub fn stored(&self) -> &FuturesDeliveryBasketResult {
        &self.stored
    }
    #[must_use]
    pub fn recalculated(&self) -> &FuturesDeliveryBasketResult {
        &self.recalculated
    }
}

pub struct ReplayFuturesDelivery<'a> {
    engine: &'a dyn FuturesDeliveryEngine,
    codec: &'a dyn FuturesDeliveryArtifactCodec,
    artifacts: &'a dyn ArtifactRepository,
    reader: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
}

impl<'a> ReplayFuturesDelivery<'a> {
    #[must_use]
    pub const fn new(
        engine: &'a dyn FuturesDeliveryEngine,
        codec: &'a dyn FuturesDeliveryArtifactCodec,
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

    /// Reads verified bytes, decodes the exact input binding and deterministically recalculates.
    ///
    /// # Errors
    ///
    /// Returns for authorization, lineage, integrity, decoding or replay drift.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        artifact_id: Ulid,
        expected_inputs: &[FuturesDeliverableInput],
        trace: SafeTraceContext,
    ) -> ApplicationResult<FuturesDeliveryReplay> {
        let first = first_input(expected_inputs)?;
        scope.authorize(first.owner())?;
        let artifact = self
            .artifacts
            .get_metadata(scope, artifact_id.clone())
            .await?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
        validate_artifact(scope, &artifact_id, &artifact, expected_inputs)?;
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
            .decode(payload.bytes(), expected_inputs)
            .map_err(map_analytics_error)?;
        let recalculated =
            CalculateFuturesDeliveryBasket::new(self.engine).execute(expected_inputs)?;
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
        Ok(FuturesDeliveryReplay {
            artifact,
            stored,
            recalculated,
        })
    }
}

fn first_input(inputs: &[FuturesDeliverableInput]) -> ApplicationResult<&FuturesDeliverableInput> {
    inputs
        .first()
        .ok_or_else(|| map_domain_error(DomainErrorCode::InvalidValue))
}

fn futures_delivery_lineage(
    inputs: &[FuturesDeliverableInput],
) -> ApplicationResult<Vec<LineageRef>> {
    let first = first_input(inputs)?;
    let mut lineage = vec![LineageRef::versioned(
        first.futures_contract().version_ref().id().clone(),
        first.futures_contract().version_ref().version(),
    )];
    lineage.extend(inputs.iter().map(|input| {
        LineageRef::versioned(
            input.bond().version_ref().id().clone(),
            input.bond().version_ref().version(),
        )
    }));
    lineage.push(
        LineageRef::new(
            first.rule_pack().version_ref().id().clone(),
            Some(first.rule_pack().version_ref().version()),
            Some(first.rule_pack().content_hash().clone()),
        )
        .map_err(map_domain_error)?,
    );
    lineage.push(LineageRef::content_addressed(
        first.snapshot().version_ref().id().clone(),
        first.snapshot().content_hash().clone(),
    ));
    Ok(lineage)
}

fn validate_artifact(
    scope: &AccessScope,
    artifact_id: &Ulid,
    artifact: &Artifact,
    inputs: &[FuturesDeliverableInput],
) -> ApplicationResult<()> {
    let first = first_input(inputs)?;
    scope.authorize(artifact.owner())?;
    if artifact.id() != artifact_id
        || artifact.owner() != first.owner()
        || artifact.kind() != ArtifactKind::Generic
        || artifact.media_type() != FUTURES_DELIVERY_MEDIA_TYPE
        || artifact.lineage() != futures_delivery_lineage(inputs)?.as_slice()
    {
        return Err(map_domain_error(DomainErrorCode::BrokenLineage));
    }
    Ok(())
}
