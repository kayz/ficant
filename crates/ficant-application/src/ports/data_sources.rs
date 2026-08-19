use async_trait::async_trait;
use ficant_domain::VersionedDefinition;
use ficant_domain::governance::{
    FoundationChangeOperation, FoundationChangeRecord, FoundationChangeRecordInput,
    FoundationResourceKind, FoundationResourceRef, PlatformRole,
};
use ficant_domain::market::{
    DataSource, DataSourceAuthorization, DataSourceKind, ImportInterface, PriceSourceType,
};
use ficant_domain::primitives::{ContentHash, OwnerRef, Version, VersionRef};

use super::fingerprint::{FingerprintBuilder, data_source_content_hash, owner_bytes};
use super::{
    AccessScope, ApplicationResult, CursorPage, FoundationChangeContext, IdempotencyKey,
    OperationFingerprint, PageRequest,
};
use crate::{ApplicationError, map_domain_error};
use ficant_domain::DomainErrorCode;

pub const DATA_SOURCE_READ_SCOPE: &str = "data-sources:read";
pub const DATA_SOURCE_WRITE_SCOPE: &str = "data-sources:write";
pub const DATA_SOURCE_IMPORT_SCOPE: &str = "data-sources:import";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterDataSource {
    change_context: FoundationChangeContext,
    expected_latest_version: Option<Version>,
    value: DataSource,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl RegisterDataSource {
    /// Builds one administrator-governed, forward-only data-source registration.
    ///
    /// # Errors
    ///
    /// Returns forbidden for role, scope, or owner drift and version conflict for a non-next value.
    pub fn new(
        change_context: FoundationChangeContext,
        expected_latest_version: Option<Version>,
        value: DataSource,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        change_context.principal().authorize_mutation(
            PlatformRole::PlatformAdmin,
            DATA_SOURCE_WRITE_SCOPE,
            value.owner(),
        )?;
        require_next_version(expected_latest_version, value.version())?;
        let mut canonical = FingerprintBuilder::new("register-data-source/v2");
        canonical.field(
            2,
            change_context
                .principal()
                .fingerprint()
                .content_hash()
                .as_bytes(),
        );
        canonical.optional_u64(3, expected_latest_version.map(Version::get));
        canonical.field(4, value.id().as_str().as_bytes());
        canonical.u64(5, value.version());
        canonical.field(6, &owner_bytes(value.owner()));
        canonical.field(7, &[data_source_kind_code(value.kind())]);
        canonical.field(8, value.name().as_bytes());
        canonical.field(9, value.connection_binding().as_bytes());
        canonical.field(10, value.dataset().as_bytes());
        canonical.field(11, value.canonical_schema_id().as_bytes());
        canonical.field(12, value.canonical_schema_hash().as_bytes());
        canonical.optional_u64(
            13,
            value
                .price_source_type()
                .map(|kind| u64::from(price_source_type_code(kind))),
        );
        canonical.field(14, &change_bytes(change_context.change()));
        let fingerprint = canonical.finish();
        Ok(Self {
            change_context,
            expected_latest_version,
            value,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn change_context(&self) -> &FoundationChangeContext {
        &self.change_context
    }
    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        self.change_context.principal().access_scope()
    }
    #[must_use]
    pub fn expected_latest_version(&self) -> Option<Version> {
        self.expected_latest_version
    }
    #[must_use]
    pub fn value(&self) -> &DataSource {
        &self.value
    }
    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }
    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }

    /// Materializes the immutable audit record bound to this exact command fingerprint.
    ///
    /// # Errors
    ///
    /// Returns validation failure if the command cannot form a valid typed change record.
    pub fn change_record(
        &self,
        before_hash: Option<ContentHash>,
    ) -> ApplicationResult<FoundationChangeRecord> {
        FoundationChangeRecord::new(FoundationChangeRecordInput {
            record_id: self.change_context.record_id().clone(),
            actor_id: self.change_context.principal().actor_id().clone(),
            owner: self.value.owner().clone(),
            active_role: PlatformRole::PlatformAdmin,
            operation: FoundationChangeOperation::RegisterDataSource,
            resource: FoundationResourceRef::versioned(
                FoundationResourceKind::DataSource,
                VersionRef::new(
                    self.value.id().clone(),
                    Version::new(self.value.version()).map_err(map_domain_error)?,
                ),
            ),
            before_hash,
            after_hash: data_source_content_hash(&self.value),
            change: self.change_context.change().clone(),
            request_fingerprint: self.fingerprint.content_hash().clone(),
            occurred_at: self.change_context.occurred_at().clone(),
            authorization_ref: None,
        })
        .map_err(map_domain_error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishDataSourceAuthorization {
    change_context: FoundationChangeContext,
    expected_latest_version: Option<Version>,
    value: DataSourceAuthorization,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl PublishDataSourceAuthorization {
    /// Builds one administrator-governed, forward-only source authorization publication.
    ///
    /// # Errors
    ///
    /// Returns forbidden for role, scope, or owner drift and version conflict for a non-next value.
    pub fn new(
        change_context: FoundationChangeContext,
        expected_latest_version: Option<Version>,
        value: DataSourceAuthorization,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        change_context.principal().authorize_mutation(
            PlatformRole::PlatformAdmin,
            DATA_SOURCE_WRITE_SCOPE,
            value.owner(),
        )?;
        require_next_version(expected_latest_version, value.version())?;
        let mut canonical = FingerprintBuilder::new("publish-data-source-authorization/v1");
        canonical.field(
            2,
            change_context
                .principal()
                .fingerprint()
                .content_hash()
                .as_bytes(),
        );
        canonical.optional_u64(3, expected_latest_version.map(Version::get));
        canonical.field(4, value.id().as_str().as_bytes());
        canonical.u64(5, value.version());
        canonical.field(6, value.content_hash().as_bytes());
        canonical.field(7, &change_bytes(change_context.change()));
        let fingerprint = canonical.finish();
        Ok(Self {
            change_context,
            expected_latest_version,
            value,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn change_context(&self) -> &FoundationChangeContext {
        &self.change_context
    }
    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        self.change_context.principal().access_scope()
    }
    #[must_use]
    pub fn expected_latest_version(&self) -> Option<Version> {
        self.expected_latest_version
    }
    #[must_use]
    pub fn value(&self) -> &DataSourceAuthorization {
        &self.value
    }
    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }
    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
    /// Materializes the immutable audit record bound to this exact command fingerprint.
    ///
    /// # Errors
    ///
    /// Returns validation failure if the command cannot form a valid typed change record.
    pub fn change_record(
        &self,
        before_hash: Option<ContentHash>,
    ) -> ApplicationResult<FoundationChangeRecord> {
        FoundationChangeRecord::new(FoundationChangeRecordInput {
            record_id: self.change_context.record_id().clone(),
            actor_id: self.change_context.principal().actor_id().clone(),
            owner: self.value.owner().clone(),
            active_role: PlatformRole::PlatformAdmin,
            operation: FoundationChangeOperation::PublishDataSourceAuthorization,
            resource: FoundationResourceRef::versioned(
                FoundationResourceKind::DataSourceAuthorization,
                self.value.version_ref(),
            ),
            before_hash,
            after_hash: self.value.content_hash().clone(),
            change: self.change_context.change().clone(),
            request_fingerprint: self.fingerprint.content_hash().clone(),
            occurred_at: self.change_context.occurred_at().clone(),
            authorization_ref: None,
        })
        .map_err(map_domain_error)
    }
}

#[async_trait]
pub trait DataSourceRepository: Send + Sync {
    async fn register(&self, command: RegisterDataSource) -> Result<DataSource, ApplicationError>;
    async fn get_exact(
        &self,
        scope: &AccessScope,
        reference: VersionRef,
    ) -> Result<Option<DataSource>, ApplicationError>;
}

/// Result of an authorization lookup performed for the governed import path.
///
/// A repository may expose the exact source reference needed for the public
/// `DataSourceNotAuthorized` error without returning the foreign-owner
/// authorization payload to the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataSourceAuthorizationResolution {
    Authorized(Box<DataSourceAuthorization>),
    OwnerMismatch { data_source: VersionRef },
}

#[async_trait]
pub trait DataSourceAuthorizationRepository: Send + Sync {
    async fn publish_authorization(
        &self,
        command: PublishDataSourceAuthorization,
    ) -> ApplicationResult<DataSourceAuthorization>;

    async fn get_authorization_exact(
        &self,
        scope: &AccessScope,
        reference: VersionRef,
    ) -> ApplicationResult<Option<DataSourceAuthorization>>;

    async fn resolve_authorization_exact(
        &self,
        scope: &AccessScope,
        reference: VersionRef,
    ) -> ApplicationResult<Option<DataSourceAuthorizationResolution>> {
        Ok(self
            .get_authorization_exact(scope, reference)
            .await?
            .map(Box::new)
            .map(DataSourceAuthorizationResolution::Authorized))
    }

    async fn list_authorizations_for_source(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        source: &VersionRef,
        import_interface: Option<ImportInterface>,
        page: PageRequest,
    ) -> ApplicationResult<CursorPage<DataSourceAuthorization>>;
}

fn require_next_version(expected: Option<Version>, actual: u64) -> ApplicationResult<()> {
    let expected_actual = match expected {
        None => 1,
        Some(version) => version
            .get()
            .checked_add(1)
            .ok_or_else(|| map_domain_error(DomainErrorCode::VersionConflict))?,
    };
    if actual != expected_actual {
        return Err(map_domain_error(DomainErrorCode::VersionConflict));
    }
    Ok(())
}

fn change_bytes(change: &ficant_domain::governance::ChangeJustification) -> Vec<u8> {
    let mut canonical = FingerprintBuilder::new("change-justification/v1");
    canonical.field(2, change.reason().as_bytes());
    for source in change.sources() {
        canonical.field(3, source.uri().as_bytes());
        canonical.field(4, source.sha256().as_bytes());
    }
    canonical.into_bytes()
}

const fn data_source_kind_code(kind: DataSourceKind) -> u8 {
    match kind {
        DataSourceKind::FileNdjson => 1,
        DataSourceKind::Postgres => 2,
    }
}

pub(crate) const fn price_source_type_code(source_type: PriceSourceType) -> u8 {
    match source_type {
        PriceSourceType::RealTrade => 1,
        PriceSourceType::ActiveQuote => 2,
        PriceSourceType::ModelValuation => 3,
        PriceSourceType::CurveInterpolation => 4,
    }
}
