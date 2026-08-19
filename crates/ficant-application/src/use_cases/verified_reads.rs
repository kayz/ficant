use ficant_domain::research::{Artifact, ArtifactKind, DataSnapshot, SignalSet, UniverseSnapshot};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged};

use crate::ports::{
    AccessScope, ApplicationResult, ArtifactRepository, IntegrityEventSink,
    RequiredVerifiedBlobRead, SafeTraceContext, SignalRepository,
    SnapshotVerifiedReadMetadataParts, SnapshotVerifiedReadMetadataRepository, VerifiedBlobPayload,
    VerifiedBlobReader, VerifiedBlobRole, VerifiedReadResourceKind,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::primitives::Ulid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedArtifactRead {
    artifact: Artifact,
    payload: VerifiedBlobPayload,
}

impl VerifiedArtifactRead {
    #[must_use]
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }

    #[must_use]
    pub fn payload(&self) -> &VerifiedBlobPayload {
        &self.payload
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedSignalRead {
    signal: SignalSet,
    artifact: Artifact,
    payload: VerifiedBlobPayload,
}

impl VerifiedSignalRead {
    #[must_use]
    pub fn signal(&self) -> &SignalSet {
        &self.signal
    }

    #[must_use]
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }

    #[must_use]
    pub fn payload(&self) -> &VerifiedBlobPayload {
        &self.payload
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifiedSnapshotRead {
    Data {
        snapshot: DataSnapshot,
        parquet: VerifiedBlobPayload,
        manifest: VerifiedBlobPayload,
    },
    Universe {
        snapshot: UniverseSnapshot,
        members_manifest: VerifiedBlobPayload,
    },
}

/// Minimal reusable verified snapshot reader.
///
/// It consumes metadata and every role declared by that metadata as one application operation.
/// No caller can receive a partial Data snapshot after only Parquet or Manifest succeeds.
pub struct VerifiedSnapshotReader<'a> {
    snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
    reader: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
}

impl<'a> VerifiedSnapshotReader<'a> {
    #[must_use]
    pub const fn new(
        snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
        reader: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
    ) -> Self {
        Self {
            snapshots,
            reader,
            integrity_events,
        }
    }

    /// Requires every immutable blob role declared by exact snapshot metadata.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` only for absent metadata and never returns a partial Data snapshot.
    pub async fn read(
        &self,
        scope: &AccessScope,
        snapshot_id: Ulid,
        trace: SafeTraceContext,
    ) -> ApplicationResult<VerifiedSnapshotRead> {
        let metadata = self
            .snapshots
            .get_verified_read_metadata(scope, snapshot_id.clone())
            .await?
            .ok_or_else(not_found)?;
        match metadata.into_parts() {
            SnapshotVerifiedReadMetadataParts::Data {
                snapshot,
                parquet_size,
                manifest_size,
            } => {
                validate_snapshot_identity(scope, &snapshot_id, snapshot.id(), snapshot.owner())?;
                let parquet_request = RequiredVerifiedBlobRead::new(
                    scope.clone(),
                    snapshot.owner().clone(),
                    VerifiedReadResourceKind::DataSnapshot,
                    snapshot.id().clone(),
                    VerifiedBlobRole::DataParquet,
                    snapshot.content_hash().clone(),
                    parquet_size,
                    trace.clone(),
                )?;
                let manifest_request = RequiredVerifiedBlobRead::new(
                    scope.clone(),
                    snapshot.owner().clone(),
                    VerifiedReadResourceKind::DataSnapshot,
                    snapshot.id().clone(),
                    VerifiedBlobRole::DataManifest,
                    snapshot.manifest_hash().clone(),
                    manifest_size,
                    trace,
                )?;
                let parquet = self
                    .reader
                    .read_required(&parquet_request, self.integrity_events)
                    .await?;
                let manifest = self
                    .reader
                    .read_required(&manifest_request, self.integrity_events)
                    .await?;
                Ok(VerifiedSnapshotRead::Data {
                    snapshot,
                    parquet,
                    manifest,
                })
            }
            SnapshotVerifiedReadMetadataParts::Universe {
                snapshot,
                members_manifest_size,
            } => {
                validate_snapshot_identity(scope, &snapshot_id, snapshot.id(), snapshot.owner())?;
                let request = RequiredVerifiedBlobRead::new(
                    scope.clone(),
                    snapshot.owner().clone(),
                    VerifiedReadResourceKind::UniverseSnapshot,
                    snapshot.id().clone(),
                    VerifiedBlobRole::UniverseMembersManifest,
                    snapshot.content_hash().clone(),
                    members_manifest_size,
                    trace,
                )?;
                let members_manifest = self
                    .reader
                    .read_required(&request, self.integrity_events)
                    .await?;
                Ok(VerifiedSnapshotRead::Universe {
                    snapshot,
                    members_manifest,
                })
            }
        }
    }
}

/// Required verified-read facade. Existing repository `get` methods remain metadata-only.
pub struct VerifiedReadFacade<'a> {
    artifacts: &'a dyn ArtifactRepository,
    signals: &'a dyn SignalRepository,
    snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
    reader: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
}

impl<'a> VerifiedReadFacade<'a> {
    #[must_use]
    pub fn new(
        artifacts: &'a dyn ArtifactRepository,
        signals: &'a dyn SignalRepository,
        snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
        reader: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
    ) -> Self {
        Self {
            artifacts,
            signals,
            snapshots,
            reader,
            integrity_events,
        }
    }

    /// Reads artifact metadata, then requires its exact payload to exist and verify.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` only when metadata is absent, `HashMismatch` for required-payload
    /// integrity loss, or `StorageUnavailable` for indeterminate transport failure.
    pub async fn read_verified_artifact(
        &self,
        scope: &AccessScope,
        artifact_id: Ulid,
        trace: SafeTraceContext,
    ) -> ApplicationResult<VerifiedArtifactRead> {
        let artifact = self
            .artifacts
            .get_integrity_checked_metadata(
                scope,
                artifact_id.clone(),
                trace.clone(),
                self.integrity_events,
            )
            .await?
            .ok_or_else(not_found)?;
        scope.authorize(artifact.owner())?;
        if artifact.id() != &artifact_id {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let request = RequiredVerifiedBlobRead::new(
            scope.clone(),
            artifact.owner().clone(),
            VerifiedReadResourceKind::Artifact,
            artifact.id().clone(),
            VerifiedBlobRole::ArtifactPayload,
            artifact.content_hash().clone(),
            artifact.blob_size(),
            trace,
        )?;
        let payload = self
            .reader
            .read_required(&request, self.integrity_events)
            .await?;
        Ok(VerifiedArtifactRead { artifact, payload })
    }

    /// Resolves `SignalSet` metadata to its exact Artifact metadata and requires that shared payload.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` only for absent metadata, `LineageIncomplete` for owner/hash/lineage
    /// disagreement, and the required-reader error for payload integrity or transport failure.
    pub async fn read_verified_signal(
        &self,
        scope: &AccessScope,
        signal_id: Ulid,
        trace: SafeTraceContext,
    ) -> ApplicationResult<VerifiedSignalRead> {
        let signal = self
            .signals
            .get_integrity_checked(
                scope,
                signal_id.clone(),
                trace.clone(),
                self.integrity_events,
            )
            .await?
            .ok_or_else(not_found)?;
        scope.authorize(signal.owner())?;
        if signal.id() != &signal_id {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let artifact = self
            .artifacts
            .get_integrity_checked_metadata(
                scope,
                signal.artifact().object_id().clone(),
                trace.clone(),
                self.integrity_events,
            )
            .await?
            .ok_or_else(not_found)?;
        validate_signal_artifact(&signal, &artifact)?;
        let request = RequiredVerifiedBlobRead::new(
            scope.clone(),
            signal.owner().clone(),
            VerifiedReadResourceKind::SignalSet,
            signal.id().clone(),
            VerifiedBlobRole::SignalPayload,
            artifact.content_hash().clone(),
            artifact.blob_size(),
            trace,
        )?;
        let payload = self
            .reader
            .read_required(&request, self.integrity_events)
            .await?;
        Ok(VerifiedSignalRead {
            signal,
            artifact,
            payload,
        })
    }

    /// Requires every role declared by snapshot metadata; Data returns only after both reads pass.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` only for absent metadata and never returns a partial Data snapshot.
    pub async fn read_verified_snapshot(
        &self,
        scope: &AccessScope,
        snapshot_id: Ulid,
        trace: SafeTraceContext,
    ) -> ApplicationResult<VerifiedSnapshotRead> {
        VerifiedSnapshotReader::new(self.snapshots, self.reader, self.integrity_events)
            .read(scope, snapshot_id, trace)
            .await
    }
}

fn validate_signal_artifact(signal: &SignalSet, artifact: &Artifact) -> ApplicationResult<()> {
    if artifact.kind() != ArtifactKind::SignalSet
        || artifact.owner() != signal.owner()
        || signal.artifact().object_id() != artifact.id()
        || signal.artifact().version().is_some()
        || signal.artifact().content_hash() != Some(artifact.content_hash())
        || signal.content_hash() != artifact.content_hash()
        || !lineage_sets_match_exactly(signal, artifact)
    {
        return Err(map_domain_error(DomainErrorCode::BrokenLineage));
    }
    Ok(())
}

fn lineage_sets_match_exactly(signal: &SignalSet, artifact: &Artifact) -> bool {
    if signal
        .lineage()
        .iter()
        .filter(|reference| *reference == signal.artifact())
        .count()
        != 1
    {
        return false;
    }
    let expected = signal.lineage().get(1..).unwrap_or_default();
    let actual = artifact.lineage();
    if expected.len() != actual.len() {
        return false;
    }
    if has_duplicate_lineage(expected) || has_duplicate_lineage(actual) {
        return false;
    }
    expected == actual
}

fn has_duplicate_lineage(lineage: &[ficant_domain::primitives::LineageRef]) -> bool {
    lineage
        .iter()
        .enumerate()
        .any(|(index, reference)| lineage[..index].contains(reference))
}

fn validate_snapshot_identity(
    scope: &AccessScope,
    requested_id: &Ulid,
    actual_id: &Ulid,
    owner: &ficant_domain::primitives::OwnerRef,
) -> ApplicationResult<()> {
    scope.authorize(owner)?;
    if requested_id != actual_id || owner.tenant_id() != scope.tenant_id() {
        return Err(map_domain_error(DomainErrorCode::BrokenLineage));
    }
    Ok(())
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}
