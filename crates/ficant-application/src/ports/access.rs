use ficant_domain::primitives::{OwnerRef, Ulid};

use super::fingerprint::FingerprintBuilder;
use super::{ApplicationResult, OperationFingerprint};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::DomainErrorCode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessScope {
    tenant_id: Ulid,
    actor_id: Ulid,
    allowed_owner_ids: Vec<Ulid>,
    fingerprint: OperationFingerprint,
}

impl AccessScope {
    /// Builds a fail-closed tenant and actor authorization scope.
    ///
    /// Owner identities are sorted and deduplicated so equivalent permissions have one canonical
    /// fingerprint.
    ///
    /// # Errors
    ///
    /// Returns validation failure when no owner is authorized.
    pub fn new(
        tenant_id: Ulid,
        actor_id: Ulid,
        mut allowed_owner_ids: Vec<Ulid>,
    ) -> ApplicationResult<Self> {
        allowed_owner_ids.sort();
        allowed_owner_ids.dedup();
        if allowed_owner_ids.is_empty() {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }

        let mut canonical = FingerprintBuilder::new("access-scope/v1");
        canonical.field(2, tenant_id.as_str().as_bytes());
        canonical.field(3, actor_id.as_str().as_bytes());
        for owner_id in &allowed_owner_ids {
            canonical.field(4, owner_id.as_str().as_bytes());
        }
        let fingerprint = canonical.finish();

        Ok(Self {
            tenant_id,
            actor_id,
            allowed_owner_ids,
            fingerprint,
        })
    }

    #[must_use]
    pub fn tenant_id(&self) -> &Ulid {
        &self.tenant_id
    }

    #[must_use]
    pub fn actor_id(&self) -> &Ulid {
        &self.actor_id
    }

    #[must_use]
    pub fn allowed_owner_ids(&self) -> &[Ulid] {
        &self.allowed_owner_ids
    }

    #[must_use]
    pub fn allows(&self, owner: &OwnerRef) -> bool {
        owner.tenant_id() == &self.tenant_id
            && self
                .allowed_owner_ids
                .binary_search(owner.owner_id())
                .is_ok()
    }

    /// Enforces this scope against one tenant-owned value.
    ///
    /// # Errors
    ///
    /// Returns a non-retryable forbidden error when tenant or owner authorization mismatches.
    pub fn authorize(&self, owner: &OwnerRef) -> ApplicationResult<()> {
        if !self.allows(owner) {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::Forbidden,
                false,
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
}
