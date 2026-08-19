use ficant_domain::Lineaged;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, VersionRef};
use ficant_domain::research::DataSnapshot;

use super::fingerprint::{FingerprintBuilder, market_time_bytes, owner_bytes, version_ref_bytes};
use super::{
    ApplicationResult, DATA_SOURCE_IMPORT_SCOPE, FoundationChangeContext, IdempotencyKey,
    OperationFingerprint,
};
use crate::{ApplicationError, ApplicationErrorCategory};

/// Complete server-side identity of one canonical import before an adapter can be selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalImportReplayRequest {
    change_context: FoundationChangeContext,
    owner: OwnerRef,
    target_snapshot_id: Ulid,
    authorization: VersionRef,
    authorization_hash: ContentHash,
    mapping_id: Ulid,
    mapping_hash: ContentHash,
    calendar: VersionRef,
    unit: VersionRef,
    as_of: MarketTime,
    visible_at: MarketTime,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl CanonicalImportReplayRequest {
    /// Builds the exact pre-adapter replay identity after authorization and definition resolution.
    ///
    /// # Errors
    ///
    /// Returns a classified authorization or validation error for any incomplete request identity.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        change_context: FoundationChangeContext,
        owner: OwnerRef,
        target_snapshot_id: Ulid,
        authorization: VersionRef,
        authorization_hash: ContentHash,
        mapping_id: Ulid,
        mapping_hash: ContentHash,
        calendar: VersionRef,
        unit: VersionRef,
        as_of: MarketTime,
        visible_at: MarketTime,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        change_context.principal().authorize_mutation(
            PlatformRole::Researcher,
            DATA_SOURCE_IMPORT_SCOPE,
            &owner,
        )?;
        if !change_context.change().is_authorized_import_reason()
            || visible_at.instant() < as_of.instant()
        {
            return Err(validation());
        }
        let mut canonical = FingerprintBuilder::new("canonical-import-replay-request/v1");
        canonical.field(
            2,
            change_context
                .principal()
                .fingerprint()
                .content_hash()
                .as_bytes(),
        );
        canonical.field(
            3,
            change_context
                .principal()
                .access_scope()
                .fingerprint()
                .content_hash()
                .as_bytes(),
        );
        canonical.field(4, &owner_bytes(&owner));
        canonical.field(5, target_snapshot_id.as_str().as_bytes());
        canonical.field(6, &version_ref_bytes(&authorization));
        canonical.field(7, authorization_hash.as_bytes());
        canonical.field(8, mapping_id.as_str().as_bytes());
        canonical.field(9, mapping_hash.as_bytes());
        canonical.field(10, &version_ref_bytes(&calendar));
        canonical.field(11, &version_ref_bytes(&unit));
        canonical.field(12, &market_time_bytes(&as_of));
        canonical.field(13, &market_time_bytes(&visible_at));
        canonical.field(14, change_context.change().reason().as_bytes());
        canonical.field(15, idempotency_key.as_str().as_bytes());
        let fingerprint = canonical.finish();
        Ok(Self {
            change_context,
            owner,
            target_snapshot_id,
            authorization,
            authorization_hash,
            mapping_id,
            mapping_hash,
            calendar,
            unit,
            as_of,
            visible_at,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn change_context(&self) -> &FoundationChangeContext {
        &self.change_context
    }
    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }
    #[must_use]
    pub fn target_snapshot_id(&self) -> &Ulid {
        &self.target_snapshot_id
    }
    #[must_use]
    pub fn authorization(&self) -> &VersionRef {
        &self.authorization
    }
    #[must_use]
    pub fn authorization_hash(&self) -> &ContentHash {
        &self.authorization_hash
    }
    #[must_use]
    pub fn mapping_id(&self) -> &Ulid {
        &self.mapping_id
    }
    #[must_use]
    pub fn mapping_hash(&self) -> &ContentHash {
        &self.mapping_hash
    }
    #[must_use]
    pub fn calendar(&self) -> &VersionRef {
        &self.calendar
    }
    #[must_use]
    pub fn unit(&self) -> &VersionRef {
        &self.unit
    }
    #[must_use]
    pub fn as_of(&self) -> &MarketTime {
        &self.as_of
    }
    #[must_use]
    pub fn visible_at(&self) -> &MarketTime {
        &self.visible_at
    }
    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }
    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }

    pub(crate) fn validate_snapshot(&self, snapshot: &DataSnapshot) -> ApplicationResult<()> {
        if snapshot.id() != &self.target_snapshot_id
            || snapshot.owner() != &self.owner
            || snapshot.as_of() != &self.as_of
            || snapshot.visible_at() != &self.visible_at
            || !contains_exact_lineage(
                snapshot,
                &self.authorization,
                Some(&self.authorization_hash),
            )
            || !contains_content_lineage(snapshot, &self.mapping_id, &self.mapping_hash)
            || !contains_exact_lineage(snapshot, &self.calendar, None)
            || !contains_exact_lineage(snapshot, &self.unit, None)
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::LineageIncomplete,
                false,
            ));
        }
        Ok(())
    }
}

/// Hardened replay result returned before any external quote adapter is invoked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalImportReplay {
    snapshot: DataSnapshot,
    actor_id: Ulid,
    authorization: VersionRef,
    authorization_hash: ContentHash,
}

/// Evidence extracted from a fully decoded canonical manifest before governed publication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalImportManifestEvidence {
    actor_id: Ulid,
    authorization: VersionRef,
    authorization_hash: ContentHash,
}

impl CanonicalImportManifestEvidence {
    #[must_use]
    pub fn new(actor_id: Ulid, authorization: VersionRef, authorization_hash: ContentHash) -> Self {
        Self {
            actor_id,
            authorization,
            authorization_hash,
        }
    }

    #[must_use]
    pub fn actor_id(&self) -> &Ulid {
        &self.actor_id
    }
    #[must_use]
    pub fn authorization(&self) -> &VersionRef {
        &self.authorization
    }
    #[must_use]
    pub fn authorization_hash(&self) -> &ContentHash {
        &self.authorization_hash
    }

    pub(crate) fn validate_request(
        &self,
        request: &CanonicalImportReplayRequest,
    ) -> ApplicationResult<()> {
        if &self.actor_id != request.change_context().principal().actor_id()
            || &self.authorization != request.authorization()
            || &self.authorization_hash != request.authorization_hash()
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        Ok(())
    }
}

impl CanonicalImportReplay {
    /// Constructs a replay result only when all durable evidence matches the request.
    ///
    /// # Errors
    ///
    /// Returns immutable or lineage failure for any request, snapshot, actor, or authorization drift.
    pub fn verified(
        request: &CanonicalImportReplayRequest,
        snapshot: DataSnapshot,
        actor_id: Ulid,
        authorization: VersionRef,
        authorization_hash: ContentHash,
    ) -> ApplicationResult<Self> {
        request.validate_snapshot(&snapshot)?;
        if &actor_id != request.change_context().principal().actor_id()
            || &authorization != request.authorization()
            || &authorization_hash != request.authorization_hash()
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        Ok(Self {
            snapshot,
            actor_id,
            authorization,
            authorization_hash,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> &DataSnapshot {
        &self.snapshot
    }
    #[must_use]
    pub fn actor_id(&self) -> &Ulid {
        &self.actor_id
    }
    #[must_use]
    pub fn authorization(&self) -> &VersionRef {
        &self.authorization
    }
    #[must_use]
    pub fn authorization_hash(&self) -> &ContentHash {
        &self.authorization_hash
    }
}

fn contains_exact_lineage(
    snapshot: &DataSnapshot,
    reference: &VersionRef,
    content_hash: Option<&ContentHash>,
) -> bool {
    snapshot
        .lineage()
        .iter()
        .filter(|lineage| {
            lineage.object_id() == reference.id()
                && lineage.version() == Some(reference.version())
                && lineage.content_hash() == content_hash
        })
        .count()
        == 1
}

fn contains_content_lineage(
    snapshot: &DataSnapshot,
    object_id: &Ulid,
    content_hash: &ContentHash,
) -> bool {
    snapshot
        .lineage()
        .iter()
        .filter(|lineage| {
            lineage.object_id() == object_id
                && lineage.version().is_none()
                && lineage.content_hash() == Some(content_hash)
        })
        .count()
        == 1
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
