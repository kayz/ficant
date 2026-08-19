use ficant_domain::governance::{FoundationChangeRecord, PlatformRole};
use ficant_domain::market::{
    DataSource, DataSourceAuthorization, DataSourceAuthorizationState, ImportInterface,
};
use ficant_domain::primitives::{ContentHash, MarketTime, Ulid, VersionRef};

use crate::ports::{
    ApplicationResult, AuthorizedPrincipal, CursorPage, DATA_SOURCE_IMPORT_SCOPE,
    DataSourceAuthorizationRepository, DataSourceAuthorizationResolution, DataSourceRepository,
    FoundationChangeFilter, FoundationChangeRepository, PageRequest,
    PublishDataSourceAuthorization, data_source_content_hash,
};
use crate::{ApplicationError, ApplicationErrorCategory};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedDataSource {
    authorization: DataSourceAuthorization,
    data_source: DataSource,
}

impl AuthorizedDataSource {
    #[must_use]
    pub fn authorization(&self) -> &DataSourceAuthorization {
        &self.authorization
    }
    #[must_use]
    pub fn data_source(&self) -> &DataSource {
        &self.data_source
    }
    #[must_use]
    pub fn authorization_ref(&self) -> VersionRef {
        self.authorization.version_ref()
    }
    #[must_use]
    pub fn authorization_hash(&self) -> &ContentHash {
        self.authorization.content_hash()
    }
    #[must_use]
    pub fn mapping_id(&self) -> &Ulid {
        self.authorization.mapping_id()
    }
    #[must_use]
    pub fn mapping_hash(&self) -> &ContentHash {
        self.authorization.mapping_hash()
    }
}

pub struct GovernedInputUseCase<'a> {
    authorizations: &'a dyn DataSourceAuthorizationRepository,
    data_sources: &'a dyn DataSourceRepository,
}

impl<'a> GovernedInputUseCase<'a> {
    pub const fn new(
        authorizations: &'a dyn DataSourceAuthorizationRepository,
        data_sources: &'a dyn DataSourceRepository,
    ) -> Self {
        Self {
            authorizations,
            data_sources,
        }
    }

    /// Publishes one administrator authorization only after its exact source identity is verified.
    ///
    /// # Errors
    ///
    /// Returns not found or validation failure for source drift, or propagates repository failure.
    pub async fn publish_authorization(
        &self,
        command: PublishDataSourceAuthorization,
    ) -> ApplicationResult<DataSourceAuthorization> {
        let expected = command.value();
        let source = self
            .data_sources
            .get_exact(command.scope(), expected.data_source().clone())
            .await?
            .ok_or_else(not_found)?;
        if source.owner() != expected.owner()
            || data_source_content_hash(&source) != *expected.data_source_hash()
            || source.canonical_schema_id() != expected.canonical_schema_id()
            || source.canonical_schema_hash() != expected.canonical_schema_hash()
        {
            return Err(validation());
        }
        self.authorizations.publish_authorization(command).await
    }

    /// Resolves a capability to its exact source before any external adapter can be selected.
    ///
    /// # Errors
    ///
    /// Returns forbidden for role/scope drift and `DataSourceNotAuthorized` for any capability,
    /// mapping, effective-window, state, or exact-source drift.
    pub async fn resolve_authorized_data_source(
        &self,
        principal: &AuthorizedPrincipal,
        authorization_ref: &VersionRef,
        mapping_id: &Ulid,
        mapping_hash: &ContentHash,
        import_interface: ImportInterface,
        at: &MarketTime,
    ) -> ApplicationResult<AuthorizedDataSource> {
        principal.require_role(PlatformRole::Researcher)?;
        if !principal.has_scope(DATA_SOURCE_IMPORT_SCOPE) {
            return Err(forbidden());
        }
        let authorization = self
            .authorizations
            .resolve_authorization_exact(principal.access_scope(), authorization_ref.clone())
            .await?
            .ok_or_else(|| unauthorized(authorization_ref, None, import_interface))?;
        let authorization = match authorization {
            DataSourceAuthorizationResolution::Authorized(authorization) => *authorization,
            DataSourceAuthorizationResolution::OwnerMismatch { data_source } => {
                return Err(unauthorized(
                    authorization_ref,
                    Some(data_source),
                    import_interface,
                ));
            }
        };
        let source_ref = authorization.data_source().clone();
        if principal
            .access_scope()
            .authorize(authorization.owner())
            .is_err()
            || authorization.import_interface() != import_interface
            || authorization.mapping_id() != mapping_id
            || authorization.mapping_hash() != mapping_hash
            || authorization.state() != DataSourceAuthorizationState::Active
            || at.instant() < authorization.effective().from().instant()
            || at.instant() >= authorization.effective().to().instant()
        {
            return Err(unauthorized(
                authorization_ref,
                Some(source_ref),
                import_interface,
            ));
        }
        let source = self
            .data_sources
            .get_exact(
                principal.access_scope(),
                authorization.data_source().clone(),
            )
            .await?
            .ok_or_else(|| {
                unauthorized(
                    authorization_ref,
                    Some(source_ref.clone()),
                    import_interface,
                )
            })?;
        if source.owner() != authorization.owner()
            || data_source_content_hash(&source) != *authorization.data_source_hash()
            || source.canonical_schema_id() != authorization.canonical_schema_id()
            || source.canonical_schema_hash() != authorization.canonical_schema_hash()
        {
            return Err(unauthorized(
                authorization_ref,
                Some(source_ref),
                import_interface,
            ));
        }
        Ok(AuthorizedDataSource {
            authorization,
            data_source: source,
        })
    }
}

pub const FOUNDATION_CHANGE_READ_SCOPE: &str = "governance:read";

pub struct FoundationChangeUseCase<'a> {
    repository: &'a dyn FoundationChangeRepository,
}

impl<'a> FoundationChangeUseCase<'a> {
    pub const fn new(repository: &'a dyn FoundationChangeRepository) -> Self {
        Self { repository }
    }

    /// Resolves one exact administrator-visible foundation change.
    ///
    /// # Errors
    ///
    /// Returns forbidden for role/scope drift, not found for absence, or repository failure.
    pub async fn get_exact(
        &self,
        principal: &AuthorizedPrincipal,
        record_id: &Ulid,
    ) -> ApplicationResult<FoundationChangeRecord> {
        authorize_governance_read(principal)?;
        self.repository
            .get_change(principal.access_scope(), record_id)
            .await?
            .ok_or_else(not_found)
    }

    /// Lists administrator-visible changes under a scope-bound cursor.
    ///
    /// # Errors
    ///
    /// Returns forbidden for role/scope/cursor drift or propagates repository failure.
    pub async fn list(
        &self,
        principal: &AuthorizedPrincipal,
        filter: &FoundationChangeFilter,
        page: PageRequest,
    ) -> ApplicationResult<CursorPage<FoundationChangeRecord>> {
        authorize_governance_read(principal)?;
        page.authorize_scope(principal.access_scope())?;
        self.repository
            .list_changes(principal.access_scope(), filter, page)
            .await
    }
}

fn authorize_governance_read(principal: &AuthorizedPrincipal) -> ApplicationResult<()> {
    principal.require_role(PlatformRole::PlatformAdmin)?;
    principal
        .has_scope(FOUNDATION_CHANGE_READ_SCOPE)
        .then_some(())
        .ok_or_else(forbidden)
}

fn unauthorized(
    authorization_ref: &VersionRef,
    data_source_ref: Option<VersionRef>,
    import_interface: ImportInterface,
) -> ApplicationError {
    ApplicationError::data_source_not_authorized(
        authorization_ref.clone(),
        data_source_ref,
        import_interface,
    )
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}
fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}
