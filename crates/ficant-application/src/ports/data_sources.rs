use async_trait::async_trait;
use ficant_domain::VersionedDefinition;
use ficant_domain::market::{DataSource, DataSourceKind, PriceSourceType};
use ficant_domain::primitives::{Version, VersionRef};

use super::fingerprint::{FingerprintBuilder, owner_bytes};
use super::{AccessScope, ApplicationResult, IdempotencyKey, OperationFingerprint};
use crate::{ApplicationError, map_domain_error};
use ficant_domain::DomainErrorCode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterDataSource {
    scope: AccessScope,
    expected_latest_version: Option<Version>,
    value: DataSource,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl RegisterDataSource {
    /// Creates an authorized append-only data-source registration command.
    ///
    /// # Errors
    ///
    /// Returns forbidden for tenant/owner scope drift and version conflict unless the candidate is
    /// v1 for a new identity or exactly the next version.
    pub fn new(
        scope: AccessScope,
        expected_latest_version: Option<Version>,
        value: DataSource,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        scope.authorize(value.owner())?;
        let expected_value_version = match expected_latest_version {
            Some(version) => version
                .get()
                .checked_add(1)
                .ok_or_else(|| map_domain_error(DomainErrorCode::VersionConflict))?,
            None => 1,
        };
        if value.version() != expected_value_version {
            return Err(map_domain_error(DomainErrorCode::VersionConflict));
        }

        let mut canonical = FingerprintBuilder::new("register-data-source/v1");
        canonical.field(2, scope.fingerprint().content_hash().as_bytes());
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
                .map(|source_type| u64::from(price_source_type_code(source_type))),
        );
        let fingerprint = canonical.finish();

        Ok(Self {
            scope,
            expected_latest_version,
            value,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        &self.scope
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
