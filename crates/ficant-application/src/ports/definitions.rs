use async_trait::async_trait;
use ficant_domain::VersionedDefinition;
use ficant_domain::governance::{
    FoundationChangeOperation, FoundationChangeRecord, FoundationChangeRecordInput,
    FoundationResourceKind, FoundationResourceRef, PlatformRole,
};
use ficant_domain::market::{
    Bond, Calendar, FuturesContract, Instrument, InstrumentKind, MarketRulePack, Unit,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};

use super::fingerprint::{
    FingerprintBuilder, definition_bytes, definition_content_hash, owner_bytes,
};
use super::{
    AccessScope, ApplicationResult, CursorPage, FoundationChangeContext, IdempotencyKey,
    OperationFingerprint, PageRequest,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::DomainErrorCode;

pub const DEFINITION_READ_SCOPE: &str = "definitions:read";
pub const DEFINITION_WRITE_SCOPE: &str = "definitions:write";

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

/// Returns the canonical immutable content identity for a stored Definition value.
#[must_use]
pub fn stored_definition_content_hash(value: &DefinitionValue) -> ContentHash {
    definition_content_hash(value)
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

/// One complete Definition append bound to a server-derived administrator principal and evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernedAppendDefinitionVersion {
    change_context: FoundationChangeContext,
    append: AppendDefinitionVersion,
    fingerprint: OperationFingerprint,
}

impl GovernedAppendDefinitionVersion {
    /// Creates a governed Definition append command.
    ///
    /// # Errors
    ///
    /// Returns forbidden unless the active principal is a Platform Admin with Definition write
    /// scope for the exact owner. Also returns the same validation/version errors as the immutable
    /// Definition append intent.
    pub fn new(
        change_context: FoundationChangeContext,
        expected_latest_version: Option<Version>,
        value: DefinitionValue,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        change_context.principal().authorize_mutation(
            PlatformRole::PlatformAdmin,
            DEFINITION_WRITE_SCOPE,
            value.owner(),
        )?;
        let append = AppendDefinitionVersion::new(expected_latest_version, value, idempotency_key)?;
        let mut canonical = FingerprintBuilder::new("append-definition-version/v2");
        canonical.field(
            2,
            change_context
                .principal()
                .fingerprint()
                .content_hash()
                .as_bytes(),
        );
        canonical.optional_u64(3, append.expected_latest_version().map(Version::get));
        canonical.field(4, &definition_bytes(append.value()));
        canonical.field(5, &change_bytes(change_context.change()));
        let fingerprint = canonical.finish();
        Ok(Self {
            change_context,
            append,
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
        self.append.expected_latest_version()
    }

    #[must_use]
    pub fn value(&self) -> &DefinitionValue {
        self.append.value()
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        self.append.idempotency_key()
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }

    /// Returns the canonical immutable content identity for the appended Definition.
    #[must_use]
    pub fn value_content_hash(&self) -> ContentHash {
        definition_content_hash(self.value())
    }

    /// Materializes the append-only change record after storage resolves the previous version.
    ///
    /// # Errors
    ///
    /// Returns validation failure if the immutable Definition value cannot form an exact
    /// versioned governance resource.
    pub fn change_record(
        &self,
        before_hash: Option<ContentHash>,
    ) -> ApplicationResult<FoundationChangeRecord> {
        FoundationChangeRecord::new(FoundationChangeRecordInput {
            record_id: self.change_context.record_id().clone(),
            actor_id: self.change_context.principal().actor_id().clone(),
            owner: self.value().owner().clone(),
            active_role: PlatformRole::PlatformAdmin,
            operation: FoundationChangeOperation::AppendMarketDefinition,
            resource: FoundationResourceRef::versioned(
                FoundationResourceKind::MarketDefinition,
                VersionRef::new(
                    Ulid::new(self.value().identity().to_owned()).map_err(map_domain_error)?,
                    Version::new(self.value().version()).map_err(map_domain_error)?,
                ),
            ),
            before_hash,
            after_hash: definition_content_hash(self.value()),
            change: self.change_context.change().clone(),
            request_fingerprint: self.fingerprint.content_hash().clone(),
            occurred_at: self.change_context.occurred_at().clone(),
            authorization_ref: None,
        })
        .map_err(map_domain_error)
    }
}

#[async_trait]
pub trait DefinitionRepository: Send + Sync {
    /// Atomically creates the identity when appending v1 and appends the complete definition.
    ///
    /// Implementations must not expose an identity without its v1 value. The default is
    /// deliberately fail-closed so legacy fixture repositories cannot masquerade as R6A-ready.
    async fn append_complete(
        &self,
        _command: GovernedAppendDefinitionVersion,
    ) -> ApplicationResult<DefinitionValue> {
        Err(ApplicationError::new(
            ApplicationErrorCategory::StateConflict,
            false,
        ))
    }

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

    /// Lists immutable versions under the scope already bound into `page`.
    async fn list_versions(
        &self,
        _scope: &AccessScope,
        _definition_id: Ulid,
        _page: PageRequest,
    ) -> ApplicationResult<CursorPage<DefinitionValue>> {
        Err(ApplicationError::new(
            ApplicationErrorCategory::StateConflict,
            false,
        ))
    }
}

/// Application boundary for the complete Definition service surface.
pub struct DefinitionUseCase<'a> {
    repository: &'a dyn DefinitionRepository,
}

impl<'a> DefinitionUseCase<'a> {
    #[must_use]
    pub const fn new(repository: &'a dyn DefinitionRepository) -> Self {
        Self { repository }
    }

    /// Appends one fully governed immutable Definition version.
    ///
    /// # Errors
    ///
    /// Returns authorization, validation, concurrency, immutable, or repository failures without
    /// falling back to the legacy ungoverned append port.
    pub async fn append(
        &self,
        command: GovernedAppendDefinitionVersion,
    ) -> ApplicationResult<DefinitionValue> {
        command.scope().authorize(command.value().owner())?;
        self.repository.append_complete(command).await
    }

    /// Reads one exact immutable Definition version under the supplied access scope.
    ///
    /// # Errors
    ///
    /// Returns authorization or repository failures.
    pub async fn get_exact(
        &self,
        scope: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        self.repository
            .get_version(scope, definition_id, version)
            .await
    }

    /// Resolves the Definition version effective at one market instant.
    ///
    /// # Errors
    ///
    /// Returns authorization, validation, or repository failures.
    pub async fn resolve_as_of(
        &self,
        scope: &AccessScope,
        definition_id: Ulid,
        instant: MarketTime,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        self.repository
            .resolve_as_of(scope, definition_id, instant)
            .await
    }

    /// Lists immutable Definition versions using a cursor bound to the same access scope.
    ///
    /// # Errors
    ///
    /// Returns forbidden for a mismatched cursor scope and otherwise propagates repository
    /// failures.
    pub async fn list_versions(
        &self,
        scope: &AccessScope,
        definition_id: Ulid,
        page: PageRequest,
    ) -> ApplicationResult<CursorPage<DefinitionValue>> {
        page.authorize_scope(scope)?;
        self.repository
            .list_versions(scope, definition_id, page)
            .await
    }
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
