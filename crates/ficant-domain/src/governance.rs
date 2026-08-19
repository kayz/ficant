use crate::market::require_text;
use crate::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use crate::{DomainErrorCode, DomainResult};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlatformRole {
    PlatformAdmin,
    Researcher,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceDocumentRef {
    uri: String,
    sha256: ContentHash,
}

impl SourceDocumentRef {
    pub fn new(uri: impl Into<String>, sha256: ContentHash) -> DomainResult<Self> {
        let uri = uri.into();
        require_text(&uri)?;
        if uri.len() > 2_048 || uri.chars().any(char::is_control) {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self { uri, sha256 })
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn sha256(&self) -> &ContentHash {
        &self.sha256
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeJustification {
    reason: String,
    sources: Vec<SourceDocumentRef>,
}

impl ChangeJustification {
    pub fn new(
        reason: impl Into<String>,
        mut sources: Vec<SourceDocumentRef>,
    ) -> DomainResult<Self> {
        let reason = validate_reason(reason.into())?;
        sources.sort();
        sources.dedup();
        if sources.is_empty() {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self { reason, sources })
    }

    /// Creates the reason attached to an import that is already bound to an administrator's
    /// content-addressed authorization. The record constructor rejects this shape unless an exact
    /// authorization reference is also present.
    pub fn for_authorized_import(reason: impl Into<String>) -> DomainResult<Self> {
        Ok(Self {
            reason: validate_reason(reason.into())?,
            sources: Vec::new(),
        })
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn sources(&self) -> &[SourceDocumentRef] {
        &self.sources
    }

    pub fn is_authorized_import_reason(&self) -> bool {
        self.sources.is_empty()
    }
}

fn validate_reason(reason: String) -> DomainResult<String> {
    require_text(&reason)?;
    if reason.len() > 1_024 || reason.chars().any(char::is_control) {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(reason)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FoundationResourceKind {
    DataSource,
    DataSourceAuthorization,
    MarketDefinition,
    MarketFact,
    CurveSnapshot,
    DataSnapshot,
    UniverseSnapshot,
    Subject,
    SubjectState,
    PositionSnapshot,
    DataHealthThresholdProfile,
}

impl FoundationResourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DataSource => "data-source",
            Self::DataSourceAuthorization => "data-source-authorization",
            Self::MarketDefinition => "market-definition",
            Self::MarketFact => "market-fact",
            Self::CurveSnapshot => "curve-snapshot",
            Self::DataSnapshot => "data-snapshot",
            Self::UniverseSnapshot => "universe-snapshot",
            Self::Subject => "subject",
            Self::SubjectState => "subject-state",
            Self::PositionSnapshot => "position-snapshot",
            Self::DataHealthThresholdProfile => "data-health-threshold-profile",
        }
    }

    pub const fn is_versioned(self) -> bool {
        matches!(
            self,
            Self::DataSource
                | Self::DataSourceAuthorization
                | Self::MarketDefinition
                | Self::Subject
                | Self::DataHealthThresholdProfile
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FoundationResourceRef {
    kind: FoundationResourceKind,
    id: Ulid,
    version: Option<Version>,
}

impl FoundationResourceRef {
    #[allow(clippy::needless_pass_by_value)]
    pub fn versioned(kind: FoundationResourceKind, reference: VersionRef) -> Self {
        Self {
            kind,
            id: reference.id().clone(),
            version: Some(reference.version()),
        }
    }

    pub fn unversioned(kind: FoundationResourceKind, id: Ulid) -> Self {
        Self {
            kind,
            id,
            version: None,
        }
    }

    pub const fn kind(&self) -> FoundationResourceKind {
        self.kind
    }

    pub fn id(&self) -> &Ulid {
        &self.id
    }

    pub const fn version(&self) -> Option<Version> {
        self.version
    }

    pub fn canonical_ref(&self) -> String {
        self.version.map_or_else(
            || format!("{}:{}", self.kind.as_str(), self.id),
            |version| format!("{}:{}@{}", self.kind.as_str(), self.id, version.get()),
        )
    }
}

/// Derives a stable ULID for one idempotent change without accepting caller-owned identity bytes.
pub fn deterministic_change_record_id(
    _occurred_at: &MarketTime,
    actor_id: &Ulid,
    resource: &FoundationResourceRef,
    idempotency_key: &str,
) -> DomainResult<Ulid> {
    require_text(idempotency_key)?;
    if idempotency_key.len() > 256 {
        return Err(DomainErrorCode::InvalidValue);
    }
    let mut digest = Sha256::new();
    digest.update(b"ficant.foundation-change-record.v1");
    update_length_delimited(&mut digest, actor_id.as_str().as_bytes());
    update_length_delimited(&mut digest, resource.canonical_ref().as_bytes());
    update_length_delimited(&mut digest, idempotency_key.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Ulid::new(ulid::Ulid::from_bytes(bytes).to_string())
}

fn update_length_delimited(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FoundationChangeOperation {
    RegisterDataSource,
    PublishDataSourceAuthorization,
    AppendMarketDefinition,
    AppendMarketFact,
    CorrectMarketFact,
    PublishCurveSnapshot,
    ImportCanonicalQuoteSnapshot,
    PublishUniverseSnapshot,
    RegisterSubject,
    PublishSubjectState,
    PublishPositionSnapshot,
    ConfigureDataHealthThreshold,
}

impl FoundationChangeOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RegisterDataSource => "data-source.register",
            Self::PublishDataSourceAuthorization => "data-source-authorization.publish",
            Self::AppendMarketDefinition => "market-definition.append",
            Self::AppendMarketFact => "market-fact.append",
            Self::CorrectMarketFact => "market-fact.correct",
            Self::PublishCurveSnapshot => "curve-snapshot.publish",
            Self::ImportCanonicalQuoteSnapshot => "data-snapshot.import-canonical-quotes",
            Self::PublishUniverseSnapshot => "universe-snapshot.publish",
            Self::RegisterSubject => "subject.register",
            Self::PublishSubjectState => "subject-state.publish",
            Self::PublishPositionSnapshot => "position-snapshot.publish",
            Self::ConfigureDataHealthThreshold => "data-health-threshold.configure",
        }
    }

    pub const fn resource_kind(self) -> FoundationResourceKind {
        match self {
            Self::RegisterDataSource => FoundationResourceKind::DataSource,
            Self::PublishDataSourceAuthorization => FoundationResourceKind::DataSourceAuthorization,
            Self::AppendMarketDefinition => FoundationResourceKind::MarketDefinition,
            Self::AppendMarketFact | Self::CorrectMarketFact => FoundationResourceKind::MarketFact,
            Self::PublishCurveSnapshot => FoundationResourceKind::CurveSnapshot,
            Self::ImportCanonicalQuoteSnapshot => FoundationResourceKind::DataSnapshot,
            Self::PublishUniverseSnapshot => FoundationResourceKind::UniverseSnapshot,
            Self::RegisterSubject => FoundationResourceKind::Subject,
            Self::PublishSubjectState => FoundationResourceKind::SubjectState,
            Self::PublishPositionSnapshot => FoundationResourceKind::PositionSnapshot,
            Self::ConfigureDataHealthThreshold => {
                FoundationResourceKind::DataHealthThresholdProfile
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundationChangeRecord {
    record_id: Ulid,
    actor_id: Ulid,
    owner: OwnerRef,
    active_role: PlatformRole,
    operation: FoundationChangeOperation,
    resource: FoundationResourceRef,
    before_hash: Option<ContentHash>,
    after_hash: ContentHash,
    change: ChangeJustification,
    request_fingerprint: ContentHash,
    occurred_at: MarketTime,
    authorization_ref: Option<VersionRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundationChangeRecordInput {
    pub record_id: Ulid,
    pub actor_id: Ulid,
    pub owner: OwnerRef,
    pub active_role: PlatformRole,
    pub operation: FoundationChangeOperation,
    pub resource: FoundationResourceRef,
    pub before_hash: Option<ContentHash>,
    pub after_hash: ContentHash,
    pub change: ChangeJustification,
    pub request_fingerprint: ContentHash,
    pub occurred_at: MarketTime,
    pub authorization_ref: Option<VersionRef>,
}

impl FoundationChangeRecord {
    pub fn new(input: FoundationChangeRecordInput) -> DomainResult<Self> {
        let is_import = input.operation == FoundationChangeOperation::ImportCanonicalQuoteSnapshot;
        if input.change.is_authorized_import_reason() != is_import
            || is_import != input.authorization_ref.is_some()
            || (is_import && input.active_role != PlatformRole::Researcher)
            || (!is_import && input.active_role != PlatformRole::PlatformAdmin)
            || input.resource.kind() != input.operation.resource_kind()
            || input.resource.version().is_some() != input.resource.kind().is_versioned()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            record_id: input.record_id,
            actor_id: input.actor_id,
            owner: input.owner,
            active_role: input.active_role,
            operation: input.operation,
            resource: input.resource,
            before_hash: input.before_hash,
            after_hash: input.after_hash,
            change: input.change,
            request_fingerprint: input.request_fingerprint,
            occurred_at: input.occurred_at,
            authorization_ref: input.authorization_ref,
        })
    }

    pub fn record_id(&self) -> &Ulid {
        &self.record_id
    }
    pub fn actor_id(&self) -> &Ulid {
        &self.actor_id
    }
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }
    pub const fn active_role(&self) -> PlatformRole {
        self.active_role
    }
    pub const fn operation(&self) -> FoundationChangeOperation {
        self.operation
    }
    pub fn resource(&self) -> &FoundationResourceRef {
        &self.resource
    }
    pub fn before_hash(&self) -> Option<&ContentHash> {
        self.before_hash.as_ref()
    }
    pub fn after_hash(&self) -> &ContentHash {
        &self.after_hash
    }
    pub fn change(&self) -> &ChangeJustification {
        &self.change
    }
    pub fn request_fingerprint(&self) -> &ContentHash {
        &self.request_fingerprint
    }
    pub fn occurred_at(&self) -> &MarketTime {
        &self.occurred_at
    }
    pub fn authorization_ref(&self) -> Option<&VersionRef> {
        self.authorization_ref.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone, Utc};

    use super::*;

    #[test]
    fn deterministic_record_identity_binds_actor_resource_and_idempotency_key() {
        let resource = FoundationResourceRef::versioned(
            FoundationResourceKind::DataSource,
            VersionRef::new(id('D'), Version::new(1).unwrap()),
        );
        let first = deterministic_change_record_id(&time(), &id('A'), &resource, "key").unwrap();
        assert_eq!(
            first,
            deterministic_change_record_id(&time(), &id('A'), &resource, "key").unwrap(),
        );
        let later = MarketTime::new(
            Utc.with_ymd_and_hms(2026, 8, 14, 0, 0, 0).unwrap(),
            "UTC",
            NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
        )
        .unwrap();
        assert_eq!(
            first,
            deterministic_change_record_id(&later, &id('A'), &resource, "key").unwrap(),
            "replay identity must not depend on a later server clock reading",
        );
        assert_ne!(
            first,
            deterministic_change_record_id(&time(), &id('B'), &resource, "key").unwrap(),
        );
        assert_ne!(
            first,
            deterministic_change_record_id(&time(), &id('A'), &resource, "other").unwrap(),
        );
        let version_one = FoundationResourceRef::versioned(
            FoundationResourceKind::DataSource,
            VersionRef::new(id('D'), Version::new(1).unwrap()),
        );
        let version_twelve = FoundationResourceRef::versioned(
            FoundationResourceKind::DataSource,
            VersionRef::new(id('D'), Version::new(12).unwrap()),
        );
        assert_ne!(
            deterministic_change_record_id(&time(), &id('A'), &version_one, "23").unwrap(),
            deterministic_change_record_id(&time(), &id('A'), &version_twelve, "3").unwrap(),
            "resource and idempotency-key boundaries must be unambiguous",
        );
    }

    #[test]
    fn admin_and_authorized_import_evidence_shapes_cannot_be_crossed() {
        let evidence =
            SourceDocumentRef::new("urn:test:evidence", ContentHash::digest(b"evidence")).unwrap();
        FoundationChangeRecord::new(record_input(
            PlatformRole::PlatformAdmin,
            FoundationChangeOperation::RegisterDataSource,
            ChangeJustification::new("reason", vec![evidence]).unwrap(),
            None,
        ))
        .unwrap();
        FoundationChangeRecord::new(record_input(
            PlatformRole::Researcher,
            FoundationChangeOperation::ImportCanonicalQuoteSnapshot,
            ChangeJustification::for_authorized_import("run approved import").unwrap(),
            Some(VersionRef::new(id('V'), Version::new(1).unwrap())),
        ))
        .unwrap();
        assert_eq!(
            FoundationChangeRecord::new(record_input(
                PlatformRole::Researcher,
                FoundationChangeOperation::RegisterDataSource,
                ChangeJustification::for_authorized_import("wrong shape").unwrap(),
                None,
            ))
            .unwrap_err(),
            DomainErrorCode::InvalidValue,
        );
        let mut mismatched = record_input(
            PlatformRole::PlatformAdmin,
            FoundationChangeOperation::RegisterDataSource,
            ChangeJustification::new(
                "reason",
                vec![
                    SourceDocumentRef::new("urn:test:mismatch", ContentHash::digest(b"mismatch"))
                        .unwrap(),
                ],
            )
            .unwrap(),
            None,
        );
        mismatched.resource =
            FoundationResourceRef::unversioned(FoundationResourceKind::PositionSnapshot, id('S'));
        assert_eq!(
            FoundationChangeRecord::new(mismatched).unwrap_err(),
            DomainErrorCode::InvalidValue,
        );
    }

    #[test]
    fn resource_version_shape_is_closed_by_kind() {
        let mut unversioned_definition = record_input(
            PlatformRole::PlatformAdmin,
            FoundationChangeOperation::RegisterDataSource,
            ChangeJustification::new(
                "reason",
                vec![
                    SourceDocumentRef::new(
                        "urn:test:unversioned",
                        ContentHash::digest(b"unversioned"),
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
            None,
        );
        unversioned_definition.resource =
            FoundationResourceRef::unversioned(FoundationResourceKind::DataSource, id('S'));
        assert_eq!(
            FoundationChangeRecord::new(unversioned_definition).unwrap_err(),
            DomainErrorCode::InvalidValue,
        );

        let mut versioned_snapshot = record_input(
            PlatformRole::PlatformAdmin,
            FoundationChangeOperation::PublishPositionSnapshot,
            ChangeJustification::new(
                "reason",
                vec![
                    SourceDocumentRef::new("urn:test:versioned", ContentHash::digest(b"versioned"))
                        .unwrap(),
                ],
            )
            .unwrap(),
            None,
        );
        versioned_snapshot.resource = FoundationResourceRef::versioned(
            FoundationResourceKind::PositionSnapshot,
            VersionRef::new(id('S'), Version::new(1).unwrap()),
        );
        assert_eq!(
            FoundationChangeRecord::new(versioned_snapshot).unwrap_err(),
            DomainErrorCode::InvalidValue,
        );
    }

    fn record_input(
        role: PlatformRole,
        operation: FoundationChangeOperation,
        change: ChangeJustification,
        authorization_ref: Option<VersionRef>,
    ) -> FoundationChangeRecordInput {
        let resource_kind = operation.resource_kind();
        let resource = if resource_kind.is_versioned() {
            FoundationResourceRef::versioned(
                resource_kind,
                VersionRef::new(id('S'), Version::new(1).unwrap()),
            )
        } else {
            FoundationResourceRef::unversioned(resource_kind, id('S'))
        };
        FoundationChangeRecordInput {
            record_id: id('R'),
            actor_id: id('A'),
            owner: OwnerRef::new(id('T'), id('P')),
            active_role: role,
            operation,
            resource,
            before_hash: None,
            after_hash: ContentHash::digest(b"after"),
            change,
            request_fingerprint: ContentHash::digest(b"request"),
            occurred_at: time(),
            authorization_ref,
        }
    }

    fn time() -> MarketTime {
        MarketTime::new(
            Utc.with_ymd_and_hms(2026, 8, 13, 0, 0, 0).unwrap(),
            "UTC",
            NaiveDate::from_ymd_opt(2026, 8, 13).unwrap(),
        )
        .unwrap()
    }
    fn id(suffix: char) -> Ulid {
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
    }
}
