use crate::market::require_text;
use crate::primitives::{ContentHash, OwnerRef, Ulid, Version};
use crate::{DomainErrorCode, DomainResult, VersionedDefinition};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataSourceKind {
    FileNdjson,
    Postgres,
}

/// Semantic provenance of price records supplied by one immutable `DataSource` version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PriceSourceType {
    RealTrade,
    ActiveQuote,
    ModelValuation,
    CurveInterpolation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataSource {
    data_source_id: Ulid,
    version: Version,
    owner: OwnerRef,
    kind: DataSourceKind,
    name: String,
    connection_binding: String,
    dataset: String,
    canonical_schema_id: String,
    canonical_schema_hash: ContentHash,
    price_source_type: Option<PriceSourceType>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataSourceInput {
    pub data_source_id: Ulid,
    pub version: Version,
    pub owner: OwnerRef,
    pub kind: DataSourceKind,
    pub name: String,
    pub connection_binding: String,
    pub dataset: String,
    pub canonical_schema_id: String,
    pub canonical_schema_hash: ContentHash,
}

impl DataSource {
    pub fn new(input: DataSourceInput) -> DomainResult<Self> {
        require_text(&input.name)?;
        require_binding(&input.connection_binding)?;
        require_binding(&input.dataset)?;
        require_schema_id(&input.canonical_schema_id)?;
        Ok(Self {
            data_source_id: input.data_source_id,
            version: input.version,
            owner: input.owner,
            kind: input.kind,
            name: input.name,
            connection_binding: input.connection_binding,
            dataset: input.dataset,
            canonical_schema_id: input.canonical_schema_id,
            canonical_schema_hash: input.canonical_schema_hash,
            price_source_type: None,
        })
    }

    /// Adds the semantic price type to a legacy-compatible source definition exactly once.
    ///
    /// `CurveInterpolation` is produced by ficant algorithms and cannot be registered as an
    /// external source property.
    pub fn with_price_source_type(mut self, value: PriceSourceType) -> DomainResult<Self> {
        if self.price_source_type.is_some() {
            return Err(DomainErrorCode::VersionConflict);
        }
        if value == PriceSourceType::CurveInterpolation {
            return Err(DomainErrorCode::InvalidValue);
        }
        self.price_source_type = Some(value);
        Ok(self)
    }

    pub fn id(&self) -> &Ulid {
        &self.data_source_id
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn kind(&self) -> DataSourceKind {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn connection_binding(&self) -> &str {
        &self.connection_binding
    }

    pub fn dataset(&self) -> &str {
        &self.dataset
    }

    pub fn canonical_schema_id(&self) -> &str {
        &self.canonical_schema_id
    }

    pub fn canonical_schema_hash(&self) -> &ContentHash {
        &self.canonical_schema_hash
    }

    pub const fn price_source_type(&self) -> Option<PriceSourceType> {
        self.price_source_type
    }
}

impl VersionedDefinition for DataSource {
    fn identity(&self) -> &str {
        self.data_source_id.as_str()
    }

    fn version(&self) -> u64 {
        self.version.get()
    }
}

fn require_binding(value: &str) -> DomainResult<()> {
    require_text(value)?;
    if value.len() > 128
        || !value.is_ascii()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}

fn require_schema_id(value: &str) -> DomainResult<()> {
    require_binding(value)?;
    if !value.starts_with("ficant.") || value.rsplit_once('.').map(|(_, tail)| tail) != Some("v1") {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}
