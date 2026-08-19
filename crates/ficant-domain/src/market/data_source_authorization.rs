use super::data_source::{DataSource, DataSourceKind, PriceSourceType};
use crate::market::require_text;
use crate::primitives::{ContentHash, EffectivePeriod, OwnerRef, Ulid, Version, VersionRef};
use crate::{DomainErrorCode, DomainResult, VersionedDefinition};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ImportInterface {
    CanonicalQuoteSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DataSourceAuthorizationState {
    Active,
    Revoked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataSourceAuthorization {
    authorization_id: Ulid,
    version: Version,
    owner: OwnerRef,
    data_source: VersionRef,
    data_source_hash: ContentHash,
    import_interface: ImportInterface,
    canonical_schema_id: String,
    canonical_schema_hash: ContentHash,
    effective: EffectivePeriod,
    state: DataSourceAuthorizationState,
    supersedes: Option<VersionRef>,
    mapping_id: Ulid,
    mapping_hash: ContentHash,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataSourceAuthorizationInput {
    pub authorization_id: Ulid,
    pub version: Version,
    pub owner: OwnerRef,
    pub data_source: VersionRef,
    pub data_source_hash: ContentHash,
    pub import_interface: ImportInterface,
    pub canonical_schema_id: String,
    pub canonical_schema_hash: ContentHash,
    pub effective: EffectivePeriod,
    pub state: DataSourceAuthorizationState,
    pub supersedes: Option<VersionRef>,
    pub mapping_id: Ulid,
    pub mapping_hash: ContentHash,
}

impl DataSourceAuthorization {
    pub fn new(input: DataSourceAuthorizationInput) -> DomainResult<Self> {
        Self::validate(&input)?;
        let content_hash = canonical_content_hash(&input);
        Ok(Self::from_parts(input, content_hash))
    }

    pub fn from_claimed_hash(
        input: DataSourceAuthorizationInput,
        claimed_hash: ContentHash,
    ) -> DomainResult<Self> {
        Self::validate(&input)?;
        if canonical_content_hash(&input) != claimed_hash {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        Ok(Self::from_parts(input, claimed_hash))
    }

    fn validate(input: &DataSourceAuthorizationInput) -> DomainResult<()> {
        require_text(&input.canonical_schema_id)?;
        if input.canonical_schema_id.len() > 128
            || !input.canonical_schema_id.starts_with("ficant.")
            || input
                .canonical_schema_id
                .rsplit_once('.')
                .map(|(_, suffix)| suffix)
                != Some("v1")
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        if input.state == DataSourceAuthorizationState::Revoked && input.version.get() == 1 {
            return Err(DomainErrorCode::InvalidStateTransition);
        }
        let expected_supersedes = input.version.get().checked_sub(1);
        match (expected_supersedes, input.supersedes.as_ref()) {
            (None | Some(0), None) if input.state == DataSourceAuthorizationState::Active => {}
            (Some(previous), Some(reference))
                if reference.id() == &input.authorization_id
                    && reference.version().get() == previous => {}
            _ => return Err(DomainErrorCode::VersionConflict),
        }
        Ok(())
    }

    fn from_parts(input: DataSourceAuthorizationInput, content_hash: ContentHash) -> Self {
        Self {
            authorization_id: input.authorization_id,
            version: input.version,
            owner: input.owner,
            data_source: input.data_source,
            data_source_hash: input.data_source_hash,
            import_interface: input.import_interface,
            canonical_schema_id: input.canonical_schema_id,
            canonical_schema_hash: input.canonical_schema_hash,
            effective: input.effective,
            state: input.state,
            supersedes: input.supersedes,
            mapping_id: input.mapping_id,
            mapping_hash: input.mapping_hash,
            content_hash,
        }
    }

    pub fn id(&self) -> &Ulid {
        &self.authorization_id
    }
    pub const fn version_value(&self) -> Version {
        self.version
    }
    pub fn version_ref(&self) -> VersionRef {
        VersionRef::new(self.authorization_id.clone(), self.version)
    }
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }
    pub fn data_source(&self) -> &VersionRef {
        &self.data_source
    }
    pub fn data_source_hash(&self) -> &ContentHash {
        &self.data_source_hash
    }
    pub const fn import_interface(&self) -> ImportInterface {
        self.import_interface
    }
    pub fn canonical_schema_id(&self) -> &str {
        &self.canonical_schema_id
    }
    pub fn canonical_schema_hash(&self) -> &ContentHash {
        &self.canonical_schema_hash
    }
    pub fn effective(&self) -> &EffectivePeriod {
        &self.effective
    }
    pub const fn state(&self) -> DataSourceAuthorizationState {
        self.state
    }
    pub fn supersedes(&self) -> Option<&VersionRef> {
        self.supersedes.as_ref()
    }
    pub fn mapping_id(&self) -> &Ulid {
        &self.mapping_id
    }
    pub fn mapping_hash(&self) -> &ContentHash {
        &self.mapping_hash
    }
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl VersionedDefinition for DataSourceAuthorization {
    fn identity(&self) -> &str {
        self.authorization_id.as_str()
    }
    fn version(&self) -> u64 {
        self.version.get()
    }
}

/// Returns the canonical content identity used when an authorization binds one immutable source.
#[must_use]
pub fn data_source_content_hash(value: &DataSource) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, b"ficant.rates.data-source.v1");
    append(&mut bytes, value.id().as_str().as_bytes());
    append(&mut bytes, &value.version().to_be_bytes());
    append(&mut bytes, value.owner().tenant_id().as_str().as_bytes());
    append(&mut bytes, value.owner().owner_id().as_str().as_bytes());
    append(
        &mut bytes,
        &[match value.kind() {
            DataSourceKind::FileNdjson => 1,
            DataSourceKind::Postgres => 2,
        }],
    );
    append(&mut bytes, value.name().as_bytes());
    append(&mut bytes, value.connection_binding().as_bytes());
    append(&mut bytes, value.dataset().as_bytes());
    append(&mut bytes, value.canonical_schema_id().as_bytes());
    append(&mut bytes, value.canonical_schema_hash().as_bytes());
    if let Some(source_type) = value.price_source_type() {
        append(
            &mut bytes,
            &[match source_type {
                PriceSourceType::RealTrade => 1,
                PriceSourceType::ActiveQuote => 2,
                PriceSourceType::ModelValuation => 3,
                PriceSourceType::CurveInterpolation => 4,
            }],
        );
    }
    ContentHash::digest(&bytes)
}

fn canonical_content_hash(input: &DataSourceAuthorizationInput) -> ContentHash {
    let mut bytes = Vec::with_capacity(512);
    append(&mut bytes, b"ficant.data-source-authorization.v1");
    append(&mut bytes, input.authorization_id.as_str().as_bytes());
    append(&mut bytes, &input.version.get().to_be_bytes());
    append(&mut bytes, input.owner.tenant_id().as_str().as_bytes());
    append(&mut bytes, input.owner.owner_id().as_str().as_bytes());
    append(&mut bytes, input.data_source.id().as_str().as_bytes());
    append(&mut bytes, &input.data_source.version().get().to_be_bytes());
    append(&mut bytes, input.data_source_hash.as_bytes());
    append(&mut bytes, &[1]);
    append(&mut bytes, input.canonical_schema_id.as_bytes());
    append(&mut bytes, input.canonical_schema_hash.as_bytes());
    append_time(&mut bytes, input.effective.from());
    append_time(&mut bytes, input.effective.to());
    append(
        &mut bytes,
        &[match input.state {
            DataSourceAuthorizationState::Active => 1,
            DataSourceAuthorizationState::Revoked => 2,
        }],
    );
    if let Some(reference) = input.supersedes.as_ref() {
        append(&mut bytes, &[1]);
        append(&mut bytes, reference.id().as_str().as_bytes());
        append(&mut bytes, &reference.version().get().to_be_bytes());
    } else {
        append(&mut bytes, &[0]);
    }
    append(&mut bytes, input.mapping_id.as_str().as_bytes());
    append(&mut bytes, input.mapping_hash.as_bytes());
    ContentHash::digest(&bytes)
}

fn append_time(bytes: &mut Vec<u8>, value: &crate::primitives::MarketTime) {
    append(bytes, &value.instant().timestamp().to_be_bytes());
    append(
        bytes,
        &value.instant().timestamp_subsec_nanos().to_be_bytes(),
    );
    append(bytes, value.market_timezone().as_bytes());
    append(bytes, value.local_trading_date().to_string().as_bytes());
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(
        &u64::try_from(value.len())
            .expect("domain field length fits")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone, Utc};

    use super::*;

    #[test]
    fn authorization_hash_binds_source_schema_mapping_window_state_and_supersedes() {
        let first =
            DataSourceAuthorization::new(input(1, DataSourceAuthorizationState::Active, None))
                .unwrap();
        let replay = DataSourceAuthorization::from_claimed_hash(
            input(1, DataSourceAuthorizationState::Active, None),
            first.content_hash().clone(),
        )
        .unwrap();
        assert_eq!(first, replay);

        let mut drift = input(1, DataSourceAuthorizationState::Active, None);
        drift.mapping_hash = ContentHash::digest(b"different-mapping");
        assert_ne!(
            DataSourceAuthorization::new(drift).unwrap().content_hash(),
            first.content_hash()
        );
        assert_eq!(
            DataSourceAuthorization::from_claimed_hash(
                input(1, DataSourceAuthorizationState::Active, None),
                ContentHash::digest(b"caller-claim"),
            )
            .unwrap_err(),
            DomainErrorCode::ContentHashMismatch,
        );
    }

    #[test]
    fn authorization_versions_are_forward_only_and_revocation_cannot_be_v1() {
        assert_eq!(
            DataSourceAuthorization::new(input(1, DataSourceAuthorizationState::Revoked, None))
                .unwrap_err(),
            DomainErrorCode::InvalidStateTransition,
        );
        assert_eq!(
            DataSourceAuthorization::new(input(2, DataSourceAuthorizationState::Active, None))
                .unwrap_err(),
            DomainErrorCode::VersionConflict,
        );
        let previous = VersionRef::new(id('V'), Version::new(1).unwrap());
        DataSourceAuthorization::new(input(
            2,
            DataSourceAuthorizationState::Revoked,
            Some(previous),
        ))
        .unwrap();
    }

    fn input(
        version: u64,
        state: DataSourceAuthorizationState,
        supersedes: Option<VersionRef>,
    ) -> DataSourceAuthorizationInput {
        DataSourceAuthorizationInput {
            authorization_id: id('V'),
            version: Version::new(version).unwrap(),
            owner: OwnerRef::new(id('T'), id('P')),
            data_source: VersionRef::new(id('D'), Version::new(4).unwrap()),
            data_source_hash: ContentHash::digest(b"source"),
            import_interface: ImportInterface::CanonicalQuoteSnapshot,
            canonical_schema_id: "ficant.market.quote.canonical.v1".to_owned(),
            canonical_schema_hash: ContentHash::digest(b"schema"),
            effective: EffectivePeriod::new(time(2026, 1, 1), time(2027, 1, 1)).unwrap(),
            state,
            supersedes,
            mapping_id: id('M'),
            mapping_hash: ContentHash::digest(b"mapping"),
        }
    }

    fn time(year: i32, month: u32, day: u32) -> crate::primitives::MarketTime {
        crate::primitives::MarketTime::new(
            Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap(),
            "UTC",
            NaiveDate::from_ymd_opt(year, month, day).unwrap(),
        )
        .unwrap()
    }

    fn id(suffix: char) -> Ulid {
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
    }
}
