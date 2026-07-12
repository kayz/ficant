use async_trait::async_trait;
use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid};
use ficant_domain::research::{DataSnapshot, UniverseSnapshot};

use super::{AccessScope, ApplicationResult, SnapshotValue};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

pub const REQUIRED_BLOB_INTEGRITY_EVENT_NAME: &str = "storage.published_content_integrity_failure";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifiedReadResourceKind {
    Artifact,
    SignalSet,
    DataSnapshot,
    UniverseSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifiedBlobRole {
    ArtifactPayload,
    SignalPayload,
    DataParquet,
    DataManifest,
    UniverseMembersManifest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrityEventSeverity {
    Error,
}

impl IntegrityEventSeverity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrityFailureReason {
    Missing,
    HashMismatch,
    SizeMismatch,
}

impl IntegrityFailureReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::HashMismatch => "hash_mismatch",
            Self::SizeMismatch => "size_mismatch",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeTraceContext(String);

impl SafeTraceContext {
    /// Creates a log-safe correlation token with no arbitrary fields.
    ///
    /// # Errors
    ///
    /// Returns validation failure unless the token is exactly 32 lowercase hexadecimal digits.
    pub fn new(value: impl Into<String>) -> ApplicationResult<Self> {
        let value = value.into();
        if value.len() != 32
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn trace_id(&self) -> &str {
        &self.0
    }
}

/// Opaque, checked intent for a blob that metadata says must exist and match exactly.
///
/// ```compile_fail
/// use ficant_application::ports::RequiredVerifiedBlobRead;
/// let _ = RequiredVerifiedBlobRead { expected_size: 1 };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequiredVerifiedBlobRead {
    scope: AccessScope,
    tenant_id: Ulid,
    owner: OwnerRef,
    resource_kind: VerifiedReadResourceKind,
    resource_id: Ulid,
    blob_role: VerifiedBlobRole,
    expected_hash: ContentHash,
    expected_size: u64,
    trace: SafeTraceContext,
}

impl RequiredVerifiedBlobRead {
    /// Creates one exact, authorized required-read intent.
    ///
    /// # Errors
    ///
    /// Returns forbidden for scope/owner drift and validation failure for zero size or an invalid
    /// resource-kind/blob-role combination.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope: AccessScope,
        owner: OwnerRef,
        resource_kind: VerifiedReadResourceKind,
        resource_id: Ulid,
        blob_role: VerifiedBlobRole,
        expected_hash: ContentHash,
        expected_size: u64,
        trace: SafeTraceContext,
    ) -> ApplicationResult<Self> {
        scope.authorize(&owner)?;
        if owner.tenant_id() != scope.tenant_id()
            || expected_size == 0
            || !role_matches(resource_kind, blob_role)
        {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        let tenant_id = scope.tenant_id().clone();
        Ok(Self {
            scope,
            tenant_id,
            owner,
            resource_kind,
            resource_id,
            blob_role,
            expected_hash,
            expected_size,
            trace,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        &self.scope
    }

    #[must_use]
    pub fn tenant_id(&self) -> &Ulid {
        &self.tenant_id
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub fn resource_kind(&self) -> VerifiedReadResourceKind {
        self.resource_kind
    }

    #[must_use]
    pub fn resource_id(&self) -> &Ulid {
        &self.resource_id
    }

    #[must_use]
    pub fn blob_role(&self) -> VerifiedBlobRole {
        self.blob_role
    }

    #[must_use]
    pub fn expected_hash(&self) -> &ContentHash {
        &self.expected_hash
    }

    #[must_use]
    pub fn expected_size(&self) -> u64 {
        self.expected_size
    }

    #[must_use]
    pub fn trace(&self) -> &SafeTraceContext {
        &self.trace
    }

    #[must_use]
    pub fn integrity_event(&self, reason: IntegrityFailureReason) -> IntegrityEvent {
        IntegrityEvent {
            tenant_id: self.tenant_id.clone(),
            resource_kind: self.resource_kind,
            resource_id: self.resource_id.clone(),
            blob_role: self.blob_role,
            expected_hash: self.expected_hash.clone(),
            expected_size: self.expected_size,
            trace: self.trace.clone(),
            reason,
        }
    }

    /// Emits one best-effort structured event and always returns non-retryable hash mismatch.
    pub async fn fail_integrity(
        &self,
        sink: &dyn IntegrityEventSink,
        reason: IntegrityFailureReason,
    ) -> ApplicationError {
        let _ = sink.emit(self.integrity_event(reason)).await;
        ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
    }

    /// Verifies returned bytes and emits exactly one event on size or hash drift.
    ///
    /// # Errors
    ///
    /// Returns non-retryable `HashMismatch`. Event sink failure never replaces that result.
    pub async fn verify_bytes(
        &self,
        sink: &dyn IntegrityEventSink,
        bytes: Vec<u8>,
    ) -> ApplicationResult<VerifiedBlobPayload> {
        let Ok(size) = u64::try_from(bytes.len()) else {
            return Err(self
                .fail_integrity(sink, IntegrityFailureReason::SizeMismatch)
                .await);
        };
        if size != self.expected_size {
            return Err(self
                .fail_integrity(sink, IntegrityFailureReason::SizeMismatch)
                .await);
        }
        let content_hash = ContentHash::digest(&bytes);
        if content_hash != self.expected_hash {
            return Err(self
                .fail_integrity(sink, IntegrityFailureReason::HashMismatch)
                .await);
        }
        Ok(VerifiedBlobPayload {
            bytes,
            content_hash,
            size,
        })
    }
}

/// Safe structured event with no owner identity, storage key, payload, or arbitrary message.
///
/// ```compile_fail
/// use ficant_application::ports::IntegrityEvent;
/// let event: IntegrityEvent = panic!();
/// let _ = event.owner_id();
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityEvent {
    tenant_id: Ulid,
    resource_kind: VerifiedReadResourceKind,
    resource_id: Ulid,
    blob_role: VerifiedBlobRole,
    expected_hash: ContentHash,
    expected_size: u64,
    trace: SafeTraceContext,
    reason: IntegrityFailureReason,
}

impl IntegrityEvent {
    #[must_use]
    pub fn name(&self) -> &'static str {
        REQUIRED_BLOB_INTEGRITY_EVENT_NAME
    }

    #[must_use]
    pub fn severity(&self) -> IntegrityEventSeverity {
        IntegrityEventSeverity::Error
    }

    #[must_use]
    pub fn reason(&self) -> IntegrityFailureReason {
        self.reason
    }

    #[must_use]
    pub fn tenant_id(&self) -> &Ulid {
        &self.tenant_id
    }

    #[must_use]
    pub fn resource_kind(&self) -> VerifiedReadResourceKind {
        self.resource_kind
    }

    #[must_use]
    pub fn resource_id(&self) -> &Ulid {
        &self.resource_id
    }

    #[must_use]
    pub fn blob_role(&self) -> VerifiedBlobRole {
        self.blob_role
    }

    #[must_use]
    pub fn expected_hash(&self) -> &ContentHash {
        &self.expected_hash
    }

    #[must_use]
    pub fn expected_size(&self) -> u64 {
        self.expected_size
    }

    #[must_use]
    pub fn trace(&self) -> &SafeTraceContext {
        &self.trace
    }
}

/// Non-optional bytes proven against one exact required-read intent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedBlobPayload {
    bytes: Vec<u8>,
    content_hash: ContentHash,
    size: u64,
}

impl VerifiedBlobPayload {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }
}

#[async_trait]
pub trait IntegrityEventSink: Send + Sync {
    /// Emits one safe structured event.
    ///
    /// # Errors
    ///
    /// Returns a sink/transport error. Required readers must not replace integrity failure with
    /// this error; [`RequiredVerifiedBlobRead::fail_integrity`] provides that behavior.
    async fn emit(&self, event: IntegrityEvent) -> ApplicationResult<()>;
}

#[async_trait]
pub trait VerifiedBlobReader: Send + Sync {
    /// Reads a blob that metadata declares required. The result is deliberately not `Option`.
    ///
    /// Implementations must emit exactly one event through `sink` for missing, hash-mismatched,
    /// or size-mismatched content, then return non-retryable `HashMismatch`. Sink failure must not
    /// mask that result. Indeterminate transport failures return `StorageUnavailable`.
    ///
    /// # Errors
    ///
    /// Returns `HashMismatch` for integrity loss or `StorageUnavailable` when transport prevents a
    /// determination.
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<VerifiedBlobPayload>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SnapshotVerifiedReadMetadataInner {
    Data {
        snapshot: DataSnapshot,
        parquet_size: u64,
        manifest_size: u64,
    },
    Universe {
        snapshot: UniverseSnapshot,
        members_manifest_size: u64,
    },
}

/// Snapshot metadata plus the role-specific durable sizes absent from the Domain snapshot value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotVerifiedReadMetadata {
    inner: SnapshotVerifiedReadMetadataInner,
}

impl SnapshotVerifiedReadMetadata {
    /// Creates `DataSnapshot` required-read metadata.
    ///
    /// # Errors
    ///
    /// Returns validation failure for a zero role size.
    pub fn data(
        snapshot: DataSnapshot,
        parquet_size: u64,
        manifest_size: u64,
    ) -> ApplicationResult<Self> {
        if parquet_size == 0 || manifest_size == 0 {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        Ok(Self {
            inner: SnapshotVerifiedReadMetadataInner::Data {
                snapshot,
                parquet_size,
                manifest_size,
            },
        })
    }

    /// Creates `UniverseSnapshot` required-read metadata.
    ///
    /// # Errors
    ///
    /// Returns validation failure for a zero manifest size.
    pub fn universe(
        snapshot: UniverseSnapshot,
        members_manifest_size: u64,
    ) -> ApplicationResult<Self> {
        if members_manifest_size == 0 {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        Ok(Self {
            inner: SnapshotVerifiedReadMetadataInner::Universe {
                snapshot,
                members_manifest_size,
            },
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> SnapshotValue {
        match &self.inner {
            SnapshotVerifiedReadMetadataInner::Data { snapshot, .. } => snapshot.clone().into(),
            SnapshotVerifiedReadMetadataInner::Universe { snapshot, .. } => snapshot.clone().into(),
        }
    }

    pub(crate) fn into_parts(self) -> SnapshotVerifiedReadMetadataParts {
        match self.inner {
            SnapshotVerifiedReadMetadataInner::Data {
                snapshot,
                parquet_size,
                manifest_size,
            } => SnapshotVerifiedReadMetadataParts::Data {
                snapshot,
                parquet_size,
                manifest_size,
            },
            SnapshotVerifiedReadMetadataInner::Universe {
                snapshot,
                members_manifest_size,
            } => SnapshotVerifiedReadMetadataParts::Universe {
                snapshot,
                members_manifest_size,
            },
        }
    }
}

pub(crate) enum SnapshotVerifiedReadMetadataParts {
    Data {
        snapshot: DataSnapshot,
        parquet_size: u64,
        manifest_size: u64,
    },
    Universe {
        snapshot: UniverseSnapshot,
        members_manifest_size: u64,
    },
}

#[async_trait]
pub trait SnapshotVerifiedReadMetadataRepository: Send + Sync {
    /// Reads snapshot metadata and exact persisted sizes for every required blob role.
    ///
    /// # Errors
    ///
    /// Returns an application error when metadata cannot be read safely.
    async fn get_verified_read_metadata(
        &self,
        scope: &AccessScope,
        snapshot_id: Ulid,
    ) -> ApplicationResult<Option<SnapshotVerifiedReadMetadata>>;
}

fn role_matches(kind: VerifiedReadResourceKind, role: VerifiedBlobRole) -> bool {
    matches!(
        (kind, role),
        (
            VerifiedReadResourceKind::Artifact,
            VerifiedBlobRole::ArtifactPayload
        ) | (
            VerifiedReadResourceKind::SignalSet,
            VerifiedBlobRole::SignalPayload
        ) | (
            VerifiedReadResourceKind::DataSnapshot,
            VerifiedBlobRole::DataParquet | VerifiedBlobRole::DataManifest
        ) | (
            VerifiedReadResourceKind::UniverseSnapshot,
            VerifiedBlobRole::UniverseMembersManifest
        )
    )
}
