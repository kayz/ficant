use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{ContentHash, DecimalValue, MarketTime, Ulid, VersionRef};
use ficant_domain::research::{
    AccountingClassificationState, CoverageDeclaration, PositionHoldingForm, PositionSnapshot,
};

use crate::ports::{
    AccessScope, ApplicationResult, BeginBlobStage, BlobStore, IdempotencyKey,
    PositionSnapshotRepository, PublishSnapshot, SnapshotBlobRole, SnapshotRepository,
    SnapshotValue, StagedSnapshotBlob, StagedSnapshotProof, VerifiedSnapshotBlob,
    VerifiedSnapshotProof, VerifyBlobStage,
};
use crate::{ApplicationError, ApplicationErrorCategory};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionView {
    pub position_id: Ulid,
    pub economic_value: DecimalValue,
    pub economic_pnl: DecimalValue,
    pub accounting_pnl: DecimalValue,
    pub included_in_position_exposure: bool,
    pub included_in_available_liquidity: bool,
    pub collateral_fact: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionViews {
    pub snapshot: PositionSnapshot,
    pub positions: Vec<PositionView>,
    pub coverage: CoverageDeclaration,
    pub content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapitalUse {
    pub snapshot: PositionSnapshot,
    pub total_capital_requirement: DecimalValue,
    pub coverage: CoverageDeclaration,
    pub content_hash: ContentHash,
}

#[derive(Clone, Debug)]
pub struct PositionSnapshotPayload {
    snapshot: PositionSnapshot,
    idempotency_key: IdempotencyKey,
}

impl PositionSnapshotPayload {
    /// Binds one already-validated snapshot to a publish idempotency key.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless the canonical payload hashes to the snapshot content hash.
    pub fn new(
        snapshot: PositionSnapshot,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        let payload = snapshot.canonical_payload();
        if payload.is_empty() || ContentHash::digest(&payload) != *snapshot.content_hash() {
            return Err(validation());
        }
        Ok(Self {
            snapshot,
            idempotency_key,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> &PositionSnapshot {
        &self.snapshot
    }
}

pub struct PublishPositionSnapshot<'a> {
    blob_store: &'a dyn BlobStore,
    snapshots: &'a dyn SnapshotRepository,
}

impl<'a> PublishPositionSnapshot<'a> {
    #[must_use]
    pub fn new(blob_store: &'a dyn BlobStore, snapshots: &'a dyn SnapshotRepository) -> Self {
        Self {
            blob_store,
            snapshots,
        }
    }

    /// Stages, verifies, promotes, and publishes one immutable `PositionSnapshot` payload.
    ///
    /// # Errors
    ///
    /// Returns a classified access, hash, blob, or persistence failure without publishing metadata.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        payload: PositionSnapshotPayload,
    ) -> ApplicationResult<PositionSnapshot> {
        scope.authorize(payload.snapshot.owner())?;
        let bytes = payload.snapshot.canonical_payload();
        let expected_hash = payload.snapshot.content_hash().clone();
        if ContentHash::digest(&bytes) != expected_hash {
            return Err(validation());
        }
        let size = u64::try_from(bytes.len()).map_err(|_| validation())?;
        let staged_id = self
            .blob_store
            .begin_stage(BeginBlobStage::new(
                scope.clone(),
                payload.snapshot.owner().clone(),
                size,
                payload.idempotency_key.scoped("position-payload-stage")?,
            )?)
            .await?;
        if let Err(error) = self.blob_store.append_chunk(scope, &staged_id, bytes).await {
            let _ = self.blob_store.discard_stage(scope, &staged_id).await;
            return Err(error);
        }
        let staged = StagedSnapshotBlob::new(
            SnapshotBlobRole::PositionPayload,
            VerifyBlobStage::new(scope.clone(), staged_id, expected_hash, size)?,
        );
        let _proof = StagedSnapshotProof::position(staged.clone())?;
        let promoted = self
            .blob_store
            .verify_and_promote(staged.verification().clone())
            .await?;
        let proof =
            VerifiedSnapshotProof::position(VerifiedSnapshotBlob::from_staged(staged, promoted)?)?;
        let command = PublishSnapshot::new(
            SnapshotValue::Position(payload.snapshot),
            proof,
            payload.idempotency_key.scoped("position-metadata")?,
        )?;
        match self.snapshots.publish_verified_manifest(command).await? {
            SnapshotValue::Position(snapshot) => Ok(snapshot),
            SnapshotValue::Data(_) | SnapshotValue::Universe(_) => Err(validation()),
        }
    }
}

pub struct PositionViewsUseCase<'a> {
    repository: &'a dyn PositionSnapshotRepository,
}

impl<'a> PositionViewsUseCase<'a> {
    pub fn new(repository: &'a dyn PositionSnapshotRepository) -> Self {
        Self { repository }
    }

    /// Resolves the latest snapshot visible at the supplied knowledge time.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when no authorized snapshot is visible, or propagates a repository failure.
    pub async fn resolve(
        &self,
        scope: &AccessScope,
        subject_ref: VersionRef,
        observed_at: MarketTime,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<PositionSnapshot> {
        self.repository
            .resolve_position_snapshot(scope, subject_ref, observed_at, knowledge_at)
            .await?
            .ok_or_else(not_found)
    }

    /// Projects immutable position facts into the three non-derived position views.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when no authorized snapshot is visible, or propagates a repository failure.
    pub async fn views(
        &self,
        scope: &AccessScope,
        snapshot_id: Ulid,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<PositionViews> {
        let snapshot = self
            .repository
            .get_position_snapshot(scope, snapshot_id, knowledge_at)
            .await?
            .ok_or_else(not_found)?;
        let positions = snapshot
            .positions()
            .iter()
            .map(|position| PositionView {
                position_id: position.id().clone(),
                economic_value: position.economic_value().clone(),
                economic_pnl: position.economic_pnl().clone(),
                accounting_pnl: position.accounting_pnl().clone(),
                included_in_position_exposure: position.includes_position_exposure(),
                included_in_available_liquidity: position.includes_available_liquidity(),
                collateral_fact: matches!(
                    position.holding_form(),
                    PositionHoldingForm::ReverseRepoCollateral
                ),
            })
            .collect::<Vec<_>>();
        let coverage = complete_coverage(&snapshot)?;
        let content_hash = position_views_hash(&snapshot, &positions, &coverage);
        Ok(PositionViews {
            snapshot,
            positions,
            coverage,
            content_hash,
        })
    }

    /// Aggregates imported capital requirements only after all classifications are known.
    ///
    /// # Errors
    ///
    /// Returns validation failure for unknown classifications or incompatible units, and `NotFound`
    /// when no authorized snapshot is visible.
    pub async fn capital_use(
        &self,
        scope: &AccessScope,
        snapshot_id: Ulid,
        knowledge_at: MarketTime,
    ) -> ApplicationResult<CapitalUse> {
        let snapshot = self
            .repository
            .get_position_snapshot(scope, snapshot_id, knowledge_at)
            .await?
            .ok_or_else(not_found)?;
        let unknown = snapshot
            .positions()
            .iter()
            .filter(|position| {
                matches!(
                    position.accounting_classification().state(),
                    AccountingClassificationState::Unknown
                )
            })
            .map(|position| position.id().as_str().to_owned())
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            return Err(ApplicationError::unknown_accounting_positions(unknown));
        }
        let mut values = snapshot.positions().iter();
        let first = values
            .next()
            .ok_or_else(not_found)?
            .capital_requirement()
            .clone();
        let total = values.try_fold(first, |total, position| {
            total
                .checked_add(position.capital_requirement())
                .map_err(crate::map_domain_error)
        })?;
        let coverage = complete_coverage(&snapshot)?;
        let content_hash = capital_use_hash(&snapshot, &total, &coverage);
        Ok(CapitalUse {
            snapshot,
            total_capital_requirement: total,
            coverage,
            content_hash,
        })
    }
}

fn complete_coverage(snapshot: &PositionSnapshot) -> ApplicationResult<CoverageDeclaration> {
    let position_ids = snapshot
        .positions()
        .iter()
        .map(|position| position.id().clone())
        .collect::<Vec<_>>();
    CoverageDeclaration::for_complete_positions(snapshot.positions(), &position_ids, None, 0)
        .map_err(crate::map_domain_error)
}

fn position_views_hash(
    snapshot: &PositionSnapshot,
    positions: &[PositionView],
    coverage: &CoverageDeclaration,
) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, b"ficant.research.position-views.v1");
    append(&mut bytes, snapshot.content_hash().as_bytes());
    for position in positions {
        append(&mut bytes, position.position_id.as_str().as_bytes());
        append_decimal(&mut bytes, &position.economic_value);
        append_decimal(&mut bytes, &position.economic_pnl);
        append_decimal(&mut bytes, &position.accounting_pnl);
        append(
            &mut bytes,
            &[
                u8::from(position.included_in_position_exposure),
                u8::from(position.included_in_available_liquidity),
                u8::from(position.collateral_fact),
            ],
        );
    }
    append(&mut bytes, &coverage.canonical_bytes());
    ContentHash::digest(&bytes)
}

fn capital_use_hash(
    snapshot: &PositionSnapshot,
    total: &DecimalValue,
    coverage: &CoverageDeclaration,
) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, b"ficant.research.capital-use.v1");
    append(&mut bytes, snapshot.content_hash().as_bytes());
    append_decimal(&mut bytes, total);
    append(&mut bytes, &coverage.canonical_bytes());
    ContentHash::digest(&bytes)
}

fn append_decimal(bytes: &mut Vec<u8>, value: &DecimalValue) {
    append(bytes, value.coefficient().as_bytes());
    append(bytes, &value.scale().to_be_bytes());
    append(bytes, value.unit().unit_id().as_str().as_bytes());
    append(bytes, &value.unit().version().get().to_be_bytes());
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
