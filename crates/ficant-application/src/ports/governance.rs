use async_trait::async_trait;
use ficant_domain::governance::{ChangeJustification, FoundationChangeRecord, PlatformRole};
use ficant_domain::primitives::{MarketTime, Ulid};

use super::{AccessScope, ApplicationResult, AuthorizedPrincipal, CursorPage, PageRequest};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundationChangeContext {
    principal: AuthorizedPrincipal,
    change: ChangeJustification,
    record_id: Ulid,
    occurred_at: MarketTime,
}

impl FoundationChangeContext {
    /// Builds the evidence context for a platform-administrator mutation.
    ///
    /// # Errors
    ///
    /// Returns forbidden for a non-administrator and validation failure for import-only evidence.
    pub fn administrator(
        principal: AuthorizedPrincipal,
        change: ChangeJustification,
        record_id: Ulid,
        occurred_at: MarketTime,
    ) -> ApplicationResult<Self> {
        principal.require_role(PlatformRole::PlatformAdmin)?;
        if change.is_authorized_import_reason() {
            return Err(crate::map_domain_error(
                ficant_domain::DomainErrorCode::InvalidValue,
            ));
        }
        Ok(Self {
            principal,
            change,
            record_id,
            occurred_at,
        })
    }

    /// Builds the evidence context for a researcher import already authorized by an admin.
    ///
    /// # Errors
    ///
    /// Returns forbidden for a non-researcher and validation failure for administrator evidence.
    pub fn authorized_import(
        principal: AuthorizedPrincipal,
        change: ChangeJustification,
        record_id: Ulid,
        occurred_at: MarketTime,
    ) -> ApplicationResult<Self> {
        principal.require_role(PlatformRole::Researcher)?;
        if !change.is_authorized_import_reason() {
            return Err(crate::map_domain_error(
                ficant_domain::DomainErrorCode::InvalidValue,
            ));
        }
        Ok(Self {
            principal,
            change,
            record_id,
            occurred_at,
        })
    }

    #[must_use]
    pub fn principal(&self) -> &AuthorizedPrincipal {
        &self.principal
    }
    #[must_use]
    pub fn change(&self) -> &ChangeJustification {
        &self.change
    }
    #[must_use]
    pub fn record_id(&self) -> &Ulid {
        &self.record_id
    }
    #[must_use]
    pub fn occurred_at(&self) -> &MarketTime {
        &self.occurred_at
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoundationChangeFilter {
    resource_ref: Option<String>,
    actor_id: Option<Ulid>,
    occurred_from: Option<MarketTime>,
    occurred_to: Option<MarketTime>,
}

impl FoundationChangeFilter {
    /// Builds one bounded, fail-closed audit query filter.
    ///
    /// # Errors
    ///
    /// Returns validation failure for malformed resource text or an empty/inverted time window.
    pub fn new(
        resource_ref: Option<String>,
        actor_id: Option<Ulid>,
        occurred_from: Option<MarketTime>,
        occurred_to: Option<MarketTime>,
    ) -> ApplicationResult<Self> {
        if resource_ref.as_ref().is_some_and(|value| {
            value.trim().is_empty() || value != value.trim() || value.len() > 256
        }) || matches!((&occurred_from, &occurred_to), (Some(from), Some(to)) if from.instant() >= to.instant())
        {
            return Err(crate::map_domain_error(
                ficant_domain::DomainErrorCode::InvalidValue,
            ));
        }
        Ok(Self {
            resource_ref,
            actor_id,
            occurred_from,
            occurred_to,
        })
    }
    #[must_use]
    pub fn resource_ref(&self) -> Option<&str> {
        self.resource_ref.as_deref()
    }
    #[must_use]
    pub fn actor_id(&self) -> Option<&Ulid> {
        self.actor_id.as_ref()
    }
    #[must_use]
    pub fn occurred_from(&self) -> Option<&MarketTime> {
        self.occurred_from.as_ref()
    }
    #[must_use]
    pub fn occurred_to(&self) -> Option<&MarketTime> {
        self.occurred_to.as_ref()
    }
}

#[async_trait]
pub trait FoundationChangeRepository: Send + Sync {
    async fn get_change(
        &self,
        scope: &AccessScope,
        record_id: &Ulid,
    ) -> ApplicationResult<Option<FoundationChangeRecord>>;

    async fn list_changes(
        &self,
        scope: &AccessScope,
        filter: &FoundationChangeFilter,
        page: PageRequest,
    ) -> ApplicationResult<CursorPage<FoundationChangeRecord>>;
}
