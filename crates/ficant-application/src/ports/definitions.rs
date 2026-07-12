use async_trait::async_trait;
use ficant_domain::VersionedDefinition;
use ficant_domain::market::{
    Bond, Calendar, FuturesContract, Instrument, InstrumentKind, MarketRulePack, Unit,
};
use ficant_domain::primitives::{MarketTime, OwnerRef, Ulid, Version};

use super::fingerprint::{FingerprintBuilder, definition_bytes, owner_bytes};
use super::{AccessScope, ApplicationResult, IdempotencyKey, OperationFingerprint};
use crate::map_domain_error;
use ficant_domain::DomainErrorCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefinitionKind {
    Instrument,
    Calendar,
    Unit,
    MarketRulePack,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefinitionIdentity {
    definition_id: Ulid,
    owner: OwnerRef,
    kind: DefinitionKind,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl DefinitionIdentity {
    #[must_use]
    pub fn new(
        definition_id: Ulid,
        owner: OwnerRef,
        kind: DefinitionKind,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        let mut canonical = FingerprintBuilder::new("definition-identity/v1");
        canonical.field(2, definition_id.as_str().as_bytes());
        canonical.field(3, &owner_bytes(&owner));
        canonical.field(4, &[definition_kind_code(kind)]);
        let fingerprint = canonical.finish();
        Self {
            definition_id,
            owner,
            kind,
            idempotency_key,
            fingerprint,
        }
    }

    #[must_use]
    pub fn definition_id(&self) -> &Ulid {
        &self.definition_id
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub fn kind(&self) -> DefinitionKind {
        self.kind
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstrumentSubtype {
    Bond(Bond),
    FuturesContract(FuturesContract),
}

impl InstrumentSubtype {
    #[must_use]
    pub fn instrument(&self) -> &ficant_domain::primitives::VersionRef {
        match self {
            Self::Bond(value) => value.instrument(),
            Self::FuturesContract(value) => value.instrument(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentDefinition {
    instrument: Instrument,
    subtype: Option<InstrumentSubtype>,
}

impl InstrumentDefinition {
    /// Builds one complete instrument definition version.
    ///
    /// # Errors
    ///
    /// Returns validation failure when the instrument kind and subtype shape disagree, or a
    /// version conflict when the subtype does not reference this exact instrument version.
    pub fn new(
        instrument: Instrument,
        subtype: Option<InstrumentSubtype>,
    ) -> ApplicationResult<Self> {
        match (instrument.kind(), subtype.as_ref()) {
            (InstrumentKind::Bond, Some(InstrumentSubtype::Bond(value))) => {
                require_same_instrument_version(&instrument, value.instrument())?;
            }
            (InstrumentKind::Futures, Some(InstrumentSubtype::FuturesContract(value))) => {
                require_same_instrument_version(&instrument, value.instrument())?;
            }
            (InstrumentKind::Other, None) => {}
            _ => return Err(map_domain_error(DomainErrorCode::InvalidValue)),
        }
        Ok(Self {
            instrument,
            subtype,
        })
    }

    #[must_use]
    pub fn instrument(&self) -> &Instrument {
        &self.instrument
    }

    #[must_use]
    pub fn subtype(&self) -> Option<&InstrumentSubtype> {
        self.subtype.as_ref()
    }

    #[must_use]
    pub fn identity(&self) -> &str {
        self.instrument.identity()
    }

    #[must_use]
    pub fn version(&self) -> u64 {
        self.instrument.version()
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        self.instrument.owner()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefinitionValue {
    Instrument(InstrumentDefinition),
    Calendar(Calendar),
    Unit(Unit),
    MarketRulePack(MarketRulePack),
}

impl DefinitionValue {
    #[must_use]
    pub fn identity(&self) -> &str {
        match self {
            Self::Instrument(value) => value.identity(),
            Self::Calendar(value) => value.identity(),
            Self::Unit(value) => value.identity(),
            Self::MarketRulePack(value) => value.identity(),
        }
    }

    #[must_use]
    pub fn version(&self) -> u64 {
        match self {
            Self::Instrument(value) => value.version(),
            Self::Calendar(value) => value.version(),
            Self::Unit(value) => value.version(),
            Self::MarketRulePack(value) => value.version(),
        }
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        match self {
            Self::Instrument(value) => value.owner(),
            Self::Calendar(value) => value.owner(),
            Self::Unit(value) => value.owner(),
            Self::MarketRulePack(value) => value.owner(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> DefinitionKind {
        match self {
            Self::Instrument(_) => DefinitionKind::Instrument,
            Self::Calendar(_) => DefinitionKind::Calendar,
            Self::Unit(_) => DefinitionKind::Unit,
            Self::MarketRulePack(_) => DefinitionKind::MarketRulePack,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendDefinitionVersion {
    expected_latest_version: Option<Version>,
    value: DefinitionValue,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl AppendDefinitionVersion {
    /// Creates a validated append-only definition version command.
    ///
    /// # Errors
    ///
    /// Returns version conflict unless the value is v1 for a new identity or exactly next.
    pub fn new(
        expected_latest_version: Option<Version>,
        value: DefinitionValue,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
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
        let mut canonical = FingerprintBuilder::new("append-definition-version/v1");
        canonical.optional_u64(2, expected_latest_version.map(Version::get));
        canonical.field(3, &definition_bytes(&value));
        let fingerprint = canonical.finish();
        Ok(Self {
            expected_latest_version,
            value,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn expected_latest_version(&self) -> Option<Version> {
        self.expected_latest_version
    }

    #[must_use]
    pub fn value(&self) -> &DefinitionValue {
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
pub trait DefinitionRepository: Send + Sync {
    /// Creates an immutable definition identity.
    ///
    /// # Errors
    ///
    /// Returns an application error for duplicate or invalid identity intent.
    async fn create_identity(&self, identity: DefinitionIdentity) -> ApplicationResult<()>;

    /// Appends a definition version under expected-latest concurrency control.
    ///
    /// # Errors
    ///
    /// Returns an application error on version, idempotency, or validation conflict.
    async fn append_version(
        &self,
        command: AppendDefinitionVersion,
    ) -> ApplicationResult<DefinitionValue>;

    /// Reads one exact definition version.
    ///
    /// # Errors
    ///
    /// Returns an application error when the query cannot be completed safely.
    async fn get_version(
        &self,
        scope: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> ApplicationResult<Option<DefinitionValue>>;

    /// Resolves a definition version visible at a market instant.
    ///
    /// # Errors
    ///
    /// Returns an application error when the query cannot be completed safely.
    async fn resolve_as_of(
        &self,
        scope: &AccessScope,
        definition_id: Ulid,
        instant: MarketTime,
    ) -> ApplicationResult<Option<DefinitionValue>>;
}

const fn definition_kind_code(kind: DefinitionKind) -> u8 {
    match kind {
        DefinitionKind::Instrument => 1,
        DefinitionKind::Calendar => 4,
        DefinitionKind::Unit => 5,
        DefinitionKind::MarketRulePack => 6,
    }
}

fn require_same_instrument_version(
    instrument: &Instrument,
    reference: &ficant_domain::primitives::VersionRef,
) -> ApplicationResult<()> {
    if reference != &instrument.version_ref() {
        return Err(map_domain_error(DomainErrorCode::VersionConflict));
    }
    Ok(())
}
