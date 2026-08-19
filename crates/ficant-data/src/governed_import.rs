use ficant_domain::VersionedDefinition;
use ficant_domain::market::{
    Calendar, DataSource, DataSourceAuthorization, DataSourceAuthorizationState, ImportInterface,
    Unit,
};
use ficant_domain::primitives::{ContentHash, MarketTime, Ulid, Version, VersionRef};

use crate::{
    CanonicalImportEvidence, CanonicalIngestRequest, CanonicalQuoteIngestor,
    CanonicalSnapshotCodec, CanonicalSnapshotPackage, DataError, DataResult, InstrumentMapping,
    PointInTimeWindow, QuoteSourceCatalog,
};

/// Data-plane input that can only be built from the authorization result and server clock.
#[derive(Clone, Debug)]
pub struct GovernedCanonicalImportRequest {
    snapshot_id: Ulid,
    actor_id: Ulid,
    authorized_at: MarketTime,
    authorization: DataSourceAuthorization,
    source: DataSource,
    mapping: InstrumentMapping,
    calendar: Calendar,
    unit: Unit,
    window: PointInTimeWindow,
    import_reason: String,
}

impl GovernedCanonicalImportRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        snapshot_id: Ulid,
        actor_id: Ulid,
        authorized_at: MarketTime,
        authorization: DataSourceAuthorization,
        source: DataSource,
        mapping: InstrumentMapping,
        calendar: Calendar,
        unit: Unit,
        window: PointInTimeWindow,
        import_reason: impl Into<String>,
    ) -> DataResult<Self> {
        let import_reason = import_reason.into();
        let source_ref = VersionRef::new(
            source.id().clone(),
            Version::new(source.version()).map_err(|_| DataError::InvalidConfiguration)?,
        );
        if import_reason.trim().is_empty()
            || import_reason != import_reason.trim()
            || import_reason.len() > 512
            || authorization.state() != DataSourceAuthorizationState::Active
            || authorization.import_interface() != ImportInterface::CanonicalQuoteSnapshot
            || authorized_at.instant() < authorization.effective().from().instant()
            || authorized_at.instant() >= authorization.effective().to().instant()
            || authorization.owner() != source.owner()
            || authorization.owner() != mapping.owner()
            || authorization.owner() != calendar.owner()
            || authorization.owner() != unit.owner()
            || authorization.data_source() != &source_ref
            || authorization.data_source_hash() != &canonical_data_source_content_hash(&source)
            || authorization.canonical_schema_id() != source.canonical_schema_id()
            || authorization.canonical_schema_hash() != source.canonical_schema_hash()
            || authorization.mapping_id() != mapping.id()
            || authorization.mapping_hash() != mapping.content_hash()
            || mapping.source() != &source_ref
        {
            return Err(DataError::InvalidConfiguration);
        }
        Ok(Self {
            snapshot_id,
            actor_id,
            authorized_at,
            authorization,
            source,
            mapping,
            calendar,
            unit,
            window,
            import_reason,
        })
    }

    #[must_use]
    pub fn authorization(&self) -> &DataSourceAuthorization {
        &self.authorization
    }

    #[must_use]
    pub fn source(&self) -> &DataSource {
        &self.source
    }

    #[must_use]
    pub fn mapping(&self) -> &InstrumentMapping {
        &self.mapping
    }

    #[must_use]
    pub fn actor_id(&self) -> &Ulid {
        &self.actor_id
    }

    #[must_use]
    pub fn authorized_at(&self) -> &MarketTime {
        &self.authorized_at
    }

    #[must_use]
    pub fn import_reason(&self) -> &str {
        &self.import_reason
    }
}

#[derive(Clone, Debug)]
pub struct PreparedCanonicalImport {
    package: CanonicalSnapshotPackage,
    authorization: VersionRef,
    authorization_hash: ContentHash,
    actor_id: Ulid,
    authorized_at: MarketTime,
    import_reason: String,
}

impl PreparedCanonicalImport {
    #[must_use]
    pub fn package(&self) -> &CanonicalSnapshotPackage {
        &self.package
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
    pub fn actor_id(&self) -> &Ulid {
        &self.actor_id
    }

    #[must_use]
    pub fn authorized_at(&self) -> &MarketTime {
        &self.authorized_at
    }

    #[must_use]
    pub fn import_reason(&self) -> &str {
        &self.import_reason
    }

    #[must_use]
    pub fn into_package(self) -> CanonicalSnapshotPackage {
        self.package
    }
}

pub struct GovernedCanonicalQuoteImporter<'a> {
    catalog: &'a QuoteSourceCatalog,
}

impl<'a> GovernedCanonicalQuoteImporter<'a> {
    #[must_use]
    pub const fn new(catalog: &'a QuoteSourceCatalog) -> Self {
        Self { catalog }
    }

    /// Reads an adapter only after all exact authorization evidence has been validated.
    pub async fn prepare(
        &self,
        request: GovernedCanonicalImportRequest,
    ) -> DataResult<PreparedCanonicalImport> {
        let adapter = self.catalog.resolve(&request.source)?;
        let canonical_request = CanonicalIngestRequest::new(
            request.source,
            request.mapping,
            request.calendar,
            request.unit,
            request.window,
        )?;
        let canonical = CanonicalQuoteIngestor
            .ingest(adapter.as_ref(), &canonical_request)
            .await?;
        let authorization = request.authorization.version_ref();
        let authorization_hash = request.authorization.content_hash().clone();
        let evidence = CanonicalImportEvidence::new(
            authorization.clone(),
            authorization_hash.clone(),
            request.actor_id.clone(),
        );
        let package = CanonicalSnapshotCodec.build_authorized(
            request.snapshot_id,
            &canonical_request,
            &canonical,
            &evidence,
        )?;
        Ok(PreparedCanonicalImport {
            package,
            authorization,
            authorization_hash,
            actor_id: request.actor_id,
            authorized_at: request.authorized_at,
            import_reason: request.import_reason,
        })
    }
}

/// Canonical `DataSource` content identity shared with the R5D materialization gate.
#[must_use]
pub fn canonical_data_source_content_hash(value: &DataSource) -> ContentHash {
    ficant_domain::market::data_source_content_hash(value)
}
