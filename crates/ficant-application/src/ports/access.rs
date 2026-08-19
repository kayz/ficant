use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedPrincipal {
    subject_id: String,
    actor_id: Ulid,
    tenant_id: Ulid,
    allowed_owner_ids: Vec<Ulid>,
    active_role: PlatformRole,
    scopes: Vec<String>,
    credential_fingerprint: ContentHash,
    access_scope: AccessScope,
    fingerprint: OperationFingerprint,
}

impl AuthorizedPrincipal {
    /// Builds one trusted identity with exactly one active role and no raw credential material.
    ///
    /// # Errors
    ///
    /// Returns validation failure for a malformed subject or scope, or when no owner is authorized.
    pub fn new(
        subject_id: String,
        actor_id: Ulid,
        tenant_id: Ulid,
        mut allowed_owner_ids: Vec<Ulid>,
        active_role: PlatformRole,
        mut scopes: Vec<String>,
        credential_fingerprint: ContentHash,
    ) -> ApplicationResult<Self> {
        if subject_id.trim().is_empty()
            || subject_id != subject_id.trim()
            || subject_id.len() > 256
            || subject_id.chars().any(char::is_control)
            || scopes.iter().any(|scope| {
                scope.trim().is_empty()
                    || scope != scope.trim()
                    || scope.len() > 128
                    || !scope.is_ascii()
                    || !scope.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.')
                    })
            })
        {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        allowed_owner_ids.sort();
        allowed_owner_ids.dedup();
        scopes.sort();
        scopes.dedup();
        let access_scope = AccessScope::new(
            tenant_id.clone(),
            actor_id.clone(),
            allowed_owner_ids.clone(),
        )?;
        let mut canonical = FingerprintBuilder::new("authorized-principal/v1");
        canonical.field(2, subject_id.as_bytes());
        canonical.field(3, actor_id.as_str().as_bytes());
        canonical.field(4, tenant_id.as_str().as_bytes());
        for owner_id in &allowed_owner_ids {
            canonical.field(5, owner_id.as_str().as_bytes());
        }
        canonical.field(6, &[platform_role_code(active_role)]);
        for scope in &scopes {
            canonical.field(7, scope.as_bytes());
        }
        canonical.field(8, credential_fingerprint.as_bytes());
        let fingerprint = canonical.finish();
        Ok(Self {
            subject_id,
            actor_id,
            tenant_id,
            allowed_owner_ids,
            active_role,
            scopes,
            credential_fingerprint,
            access_scope,
            fingerprint,
        })
    }

    #[must_use]
    pub fn subject_id(&self) -> &str {
        &self.subject_id
    }
    #[must_use]
    pub fn actor_id(&self) -> &Ulid {
        &self.actor_id
    }
    #[must_use]
    pub fn tenant_id(&self) -> &Ulid {
        &self.tenant_id
    }
    #[must_use]
    pub fn allowed_owner_ids(&self) -> &[Ulid] {
        &self.allowed_owner_ids
    }
    #[must_use]
    pub const fn active_role(&self) -> PlatformRole {
        self.active_role
    }
    #[must_use]
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }
    #[must_use]
    pub fn credential_fingerprint(&self) -> &ContentHash {
        &self.credential_fingerprint
    }
    #[must_use]
    pub fn access_scope(&self) -> &AccessScope {
        &self.access_scope
    }
    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }

    #[must_use]
    pub fn has_scope(&self, required_scope: &str) -> bool {
        self.scopes
            .binary_search_by(|scope| scope.as_str().cmp(required_scope))
            .is_ok()
    }

    /// # Errors
    ///
    /// Returns forbidden when the request's active role is not the required role.
    pub fn require_role(&self, required_role: PlatformRole) -> ApplicationResult<()> {
        (self.active_role == required_role)
            .then_some(())
            .ok_or_else(forbidden)
    }

    /// # Errors
    ///
    /// Returns forbidden unless role, scope, tenant, and owner all match the mutation boundary.
    pub fn authorize_mutation(
        &self,
        required_role: PlatformRole,
        required_scope: &str,
        owner: &OwnerRef,
    ) -> ApplicationResult<()> {
        self.require_role(required_role)?;
        if !self.has_scope(required_scope) {
            return Err(forbidden());
        }
        self.access_scope.authorize(owner)
    }
}

const fn platform_role_code(role: PlatformRole) -> u8 {
    match role {
        PlatformRole::PlatformAdmin => 1,
        PlatformRole::Researcher => 2,
    }
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
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
