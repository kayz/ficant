use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use ficant_application::ports::{
    ArtifactRepository, IntegrityEvent, IntegrityEventSink, IntegrityFailureReason,
    PublishArtifact, PublishSignalSet, RequiredVerifiedBlobRead, SafeTraceContext,
    SignalRepository, SnapshotVerifiedReadMetadata, SnapshotVerifiedReadMetadataRepository,
    VerifiedBlobPayload, VerifiedBlobReader, VerifiedBlobRole, VerifiedReadResourceKind,
};
use ficant_application::use_cases::verified_reads::{VerifiedReadFacade, VerifiedSnapshotRead};
use ficant_application::{AccessScope, ApplicationError, ApplicationErrorCategory};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_domain::research::{
    Artifact, ArtifactKind, DataSnapshot, DataSnapshotInput, SignalSet, SignalSetInput,
    UniverseSnapshot,
};
use ficant_domain::{ContentAddressed, Lineaged};

const SIGNAL_BYTES: &[u8] = b"signal-payload";
const PARQUET_BYTES: &[u8] = b"parquet";
const MANIFEST_BYTES: &[u8] = b"manifest";
const MEMBERS_BYTES: &[u8] = b"members";
const CURVE_BYTES: &[u8] = b"curve-points";

#[test]
fn artifact_signal_data_and_universe_require_complete_verified_payloads() {
    let fixture = fixture();
    let metadata = Metadata::from_fixture(&fixture);
    let reader = RecordingReader::good();
    let sink = RecordingSink::default();
    let facade = facade(&metadata, &reader, &sink);

    let artifact = block_on(facade.read_verified_artifact(&scope(), id('A'), trace())).unwrap();
    assert_eq!(artifact.payload().bytes(), SIGNAL_BYTES);

    let signal = block_on(facade.read_verified_signal(&scope(), id('S'), trace())).unwrap();
    assert_eq!(signal.artifact().id(), &id('A'));
    assert_eq!(signal.payload().bytes(), SIGNAL_BYTES);

    let data = block_on(facade.read_verified_snapshot(&scope(), id('D'), trace())).unwrap();
    let VerifiedSnapshotRead::Data {
        parquet, manifest, ..
    } = data
    else {
        panic!("expected Data snapshot")
    };
    assert_eq!(parquet.bytes(), PARQUET_BYTES);
    assert_eq!(manifest.bytes(), MANIFEST_BYTES);

    let universe = block_on(facade.read_verified_snapshot(&scope(), id('U'), trace())).unwrap();
    let VerifiedSnapshotRead::Universe {
        members_manifest, ..
    } = universe
    else {
        panic!("expected Universe snapshot")
    };
    assert_eq!(members_manifest.bytes(), MEMBERS_BYTES);
    assert_eq!(reader.calls.load(Ordering::SeqCst), 5);
    assert!(lock(&sink.events).is_empty());
}

#[test]
fn integrity_loss_emits_exactly_once_and_sink_failure_never_masks_hash_mismatch() {
    let fixture = fixture();
    let metadata = Metadata::from_fixture(&fixture);
    for (mode, reason) in [
        (FailureMode::Missing, IntegrityFailureReason::Missing),
        (
            FailureMode::HashMismatch,
            IntegrityFailureReason::HashMismatch,
        ),
        (
            FailureMode::SizeMismatch,
            IntegrityFailureReason::SizeMismatch,
        ),
    ] {
        let reader = RecordingReader::failing(VerifiedBlobRole::ArtifactPayload, mode);
        let sink = RecordingSink::default();
        let error = block_on(facade(&metadata, &reader, &sink).read_verified_artifact(
            &scope(),
            id('A'),
            trace(),
        ))
        .unwrap_err();
        assert_error(&error, ApplicationErrorCategory::HashMismatch, false);
        let events = lock(&sink.events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].reason(), reason);
        assert_eq!(
            events[0].name(),
            "storage.published_content_integrity_failure"
        );
    }

    let reader = RecordingReader::failing(VerifiedBlobRole::ArtifactPayload, FailureMode::Missing);
    let sink = RecordingSink {
        fail: true,
        ..RecordingSink::default()
    };
    let error = block_on(facade(&metadata, &reader, &sink).read_verified_artifact(
        &scope(),
        id('A'),
        trace(),
    ))
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::HashMismatch, false);
    assert_eq!(lock(&sink.events).len(), 1);
}

#[test]
fn metadata_absence_and_transport_are_distinct_from_integrity_loss() {
    let metadata = Metadata::default();
    let reader = RecordingReader::good();
    let sink = RecordingSink::default();
    let error = block_on(facade(&metadata, &reader, &sink).read_verified_artifact(
        &scope(),
        id('A'),
        trace(),
    ))
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::NotFound, false);
    assert_eq!(reader.calls.load(Ordering::SeqCst), 0);
    assert!(lock(&sink.events).is_empty());

    let fixture = fixture();
    let metadata = Metadata::from_fixture(&fixture);
    let reader =
        RecordingReader::failing(VerifiedBlobRole::ArtifactPayload, FailureMode::Transport);
    let error = block_on(facade(&metadata, &reader, &sink).read_verified_artifact(
        &scope(),
        id('A'),
        trace(),
    ))
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::StorageUnavailable, true);
    assert!(lock(&sink.events).is_empty());
}

#[test]
fn partial_data_snapshot_never_returns_and_only_failed_role_emits() {
    let fixture = fixture();
    let metadata = Metadata::from_fixture(&fixture);
    let reader = RecordingReader::failing(VerifiedBlobRole::DataManifest, FailureMode::Missing);
    let sink = RecordingSink::default();
    let error = block_on(facade(&metadata, &reader, &sink).read_verified_snapshot(
        &scope(),
        id('D'),
        trace(),
    ))
    .unwrap_err();

    assert_error(&error, ApplicationErrorCategory::HashMismatch, false);
    assert_eq!(reader.calls.load(Ordering::SeqCst), 2);
    let events = lock(&sink.events);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].blob_role(), VerifiedBlobRole::DataManifest);
}

#[test]
fn signal_artifact_lineage_drift_fails_before_required_reader() {
    let fixture = fixture();
    let mut metadata = Metadata::from_fixture(&fixture);
    metadata.artifact = Some(
        Artifact::new(
            fixture.artifact.id().clone(),
            fixture.artifact.owner().clone(),
            fixture.artifact.kind(),
            fixture.artifact.media_type(),
            fixture.artifact.content_hash().clone(),
            fixture.artifact.blob_size(),
            vec![fixture.artifact.lineage()[0].clone()],
        )
        .unwrap(),
    );
    let reader = RecordingReader::good();
    let sink = RecordingSink::default();
    let error = block_on(facade(&metadata, &reader, &sink).read_verified_signal(
        &scope(),
        id('S'),
        trace(),
    ))
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::LineageIncomplete, false);
    assert_eq!(reader.calls.load(Ordering::SeqCst), 0);
    assert!(lock(&sink.events).is_empty());
}

#[test]
fn signal_artifact_extra_lineage_ref_is_rejected_before_required_reader() {
    let fixture = fixture();
    let mut metadata = Metadata::from_fixture(&fixture);
    let mut lineage = fixture.artifact.lineage().to_vec();
    lineage.push(LineageRef::content_addressed(id('E'), hash(b"extra")));
    metadata.artifact = Some(
        Artifact::new(
            fixture.artifact.id().clone(),
            fixture.artifact.owner().clone(),
            fixture.artifact.kind(),
            fixture.artifact.media_type(),
            fixture.artifact.content_hash().clone(),
            fixture.artifact.blob_size(),
            lineage,
        )
        .unwrap(),
    );
    let reader = RecordingReader::good();
    let sink = RecordingSink::default();
    let error = block_on(facade(&metadata, &reader, &sink).read_verified_signal(
        &scope(),
        id('S'),
        trace(),
    ))
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::LineageIncomplete, false);
    assert_eq!(reader.calls.load(Ordering::SeqCst), 0);
    assert!(lock(&sink.events).is_empty());
}

#[test]
fn safe_trace_context_accepts_only_exact_lowercase_hex32() {
    assert!(SafeTraceContext::new("0123456789abcdef0123456789abcdef").is_ok());
    for invalid in [
        "trace-123",
        "0123456789ABCDEF0123456789ABCDEF",
        "0123456789abcdef0123456789abcde",
        "0123456789abcdef0123456789abcdef0",
        "g123456789abcdef0123456789abcdef",
        "not-a-safe-trace-context",
    ] {
        assert!(
            SafeTraceContext::new(invalid).is_err(),
            "accepted {invalid}"
        );
    }
}

#[test]
fn request_factory_rejects_scope_owner_role_size_and_payload_drift() {
    let valid = request(
        scope(),
        owner(),
        VerifiedReadResourceKind::Artifact,
        VerifiedBlobRole::ArtifactPayload,
        SIGNAL_BYTES,
    )
    .unwrap();
    assert_eq!(valid.tenant_id(), scope().tenant_id());
    let curve = request(
        scope(),
        owner(),
        VerifiedReadResourceKind::CurveSnapshot,
        VerifiedBlobRole::CurvePoints,
        CURVE_BYTES,
    )
    .unwrap();
    assert_eq!(curve.blob_role(), VerifiedBlobRole::CurvePoints);

    let wrong_scope = AccessScope::new(id('T'), id('B'), vec![id('Z')]).unwrap();
    let error = request(
        wrong_scope,
        owner(),
        VerifiedReadResourceKind::Artifact,
        VerifiedBlobRole::ArtifactPayload,
        SIGNAL_BYTES,
    )
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::Forbidden, false);

    let error = RequiredVerifiedBlobRead::new(
        scope(),
        owner(),
        VerifiedReadResourceKind::Artifact,
        id('A'),
        VerifiedBlobRole::DataParquet,
        hash(SIGNAL_BYTES),
        SIGNAL_BYTES.len() as u64,
        trace(),
    )
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::ValidationFailed, false);

    let error = RequiredVerifiedBlobRead::new(
        scope(),
        owner(),
        VerifiedReadResourceKind::Artifact,
        id('A'),
        VerifiedBlobRole::ArtifactPayload,
        hash(SIGNAL_BYTES),
        0,
        trace(),
    )
    .unwrap_err();
    assert_error(&error, ApplicationErrorCategory::ValidationFailed, false);

    let sink = RecordingSink::default();
    let error = block_on(valid.verify_bytes(&sink, b"corrupt".to_vec())).unwrap_err();
    assert_error(&error, ApplicationErrorCategory::HashMismatch, false);
    assert_eq!(lock(&sink.events).len(), 1);
}

#[derive(Clone, Copy)]
enum FailureMode {
    Missing,
    HashMismatch,
    SizeMismatch,
    Transport,
}

struct RecordingReader {
    failure: Option<(VerifiedBlobRole, FailureMode)>,
    calls: AtomicUsize,
}

impl RecordingReader {
    fn good() -> Self {
        Self {
            failure: None,
            calls: AtomicUsize::new(0),
        }
    }

    fn failing(role: VerifiedBlobRole, mode: FailureMode) -> Self {
        Self {
            failure: Some((role, mode)),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl VerifiedBlobReader for RecordingReader {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> Result<VerifiedBlobPayload, ApplicationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((_, mode)) = self
            .failure
            .filter(|(role, _)| *role == request.blob_role())
        {
            return match mode {
                FailureMode::Missing => Err(request
                    .fail_integrity(sink, IntegrityFailureReason::Missing)
                    .await),
                FailureMode::HashMismatch => Err(request
                    .fail_integrity(sink, IntegrityFailureReason::HashMismatch)
                    .await),
                FailureMode::SizeMismatch => Err(request
                    .fail_integrity(sink, IntegrityFailureReason::SizeMismatch)
                    .await),
                FailureMode::Transport => Err(ApplicationError::new(
                    ApplicationErrorCategory::StorageUnavailable,
                    true,
                )),
            };
        }
        request
            .verify_bytes(sink, bytes_for(request.blob_role()).to_vec())
            .await
    }
}

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<IntegrityEvent>>,
    fail: bool,
}

#[async_trait]
impl IntegrityEventSink for RecordingSink {
    async fn emit(&self, event: IntegrityEvent) -> Result<(), ApplicationError> {
        lock(&self.events).push(event);
        if self.fail {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::StorageUnavailable,
                true,
            ));
        }
        Ok(())
    }
}

#[derive(Default)]
struct Metadata {
    artifact: Option<Artifact>,
    signal: Option<SignalSet>,
    data: Option<SnapshotVerifiedReadMetadata>,
    universe: Option<SnapshotVerifiedReadMetadata>,
}

impl Metadata {
    fn from_fixture(fixture: &Fixture) -> Self {
        Self {
            artifact: Some(fixture.artifact.clone()),
            signal: Some(fixture.signal.clone()),
            data: Some(
                SnapshotVerifiedReadMetadata::data(
                    fixture.data.clone(),
                    PARQUET_BYTES.len() as u64,
                    MANIFEST_BYTES.len() as u64,
                )
                .unwrap(),
            ),
            universe: Some(
                SnapshotVerifiedReadMetadata::universe(
                    fixture.universe.clone(),
                    MEMBERS_BYTES.len() as u64,
                )
                .unwrap(),
            ),
        }
    }
}

#[async_trait]
impl ArtifactRepository for Metadata {
    async fn publish_verified_blob(
        &self,
        _command: PublishArtifact,
    ) -> Result<Artifact, ApplicationError> {
        unreachable!()
    }

    async fn get_metadata(
        &self,
        _scope: &AccessScope,
        artifact_id: Ulid,
    ) -> Result<Option<Artifact>, ApplicationError> {
        Ok(self
            .artifact
            .clone()
            .filter(|artifact| artifact.id() == &artifact_id))
    }
}

#[async_trait]
impl SignalRepository for Metadata {
    async fn publish(&self, _command: PublishSignalSet) -> Result<SignalSet, ApplicationError> {
        unreachable!()
    }

    async fn get(
        &self,
        _scope: &AccessScope,
        signal_set_id: Ulid,
    ) -> Result<Option<SignalSet>, ApplicationError> {
        Ok(self
            .signal
            .clone()
            .filter(|signal| signal.id() == &signal_set_id))
    }
}

#[async_trait]
impl SnapshotVerifiedReadMetadataRepository for Metadata {
    async fn get_verified_read_metadata(
        &self,
        _scope: &AccessScope,
        snapshot_id: Ulid,
    ) -> Result<Option<SnapshotVerifiedReadMetadata>, ApplicationError> {
        if snapshot_id == id('D') {
            Ok(self.data.clone())
        } else if snapshot_id == id('U') {
            Ok(self.universe.clone())
        } else {
            Ok(None)
        }
    }
}

struct Fixture {
    artifact: Artifact,
    signal: SignalSet,
    data: DataSnapshot,
    universe: UniverseSnapshot,
}

fn fixture() -> Fixture {
    let data_ref = LineageRef::content_addressed(id('D'), hash(PARQUET_BYTES));
    let universe_ref = LineageRef::content_addressed(id('U'), hash(MEMBERS_BYTES));
    let rule_ref = VersionRef::new(id('R'), Version::new(1).unwrap());
    let input_ref = LineageRef::content_addressed(id('J'), hash(b"input"));
    let artifact = Artifact::new(
        id('A'),
        owner(),
        ArtifactKind::SignalSet,
        "application/vnd.ficant.signal-set",
        hash(SIGNAL_BYTES),
        SIGNAL_BYTES.len() as u64,
        vec![
            data_ref.clone(),
            universe_ref.clone(),
            LineageRef::versioned(rule_ref.id().clone(), rule_ref.version()),
            input_ref.clone(),
        ],
    )
    .unwrap();
    let signal = SignalSet::new(SignalSetInput {
        signal_set_id: id('S'),
        owner: owner(),
        artifact: LineageRef::content_addressed(artifact.id().clone(), hash(SIGNAL_BYTES)),
        experiment_run_id: id('X'),
        data_snapshot: data_ref,
        universe_snapshot: universe_ref,
        rule_packs: vec![rule_ref],
        input_artifacts: vec![input_ref],
        valid: EffectivePeriod::new(time(1), time(2)).unwrap(),
    })
    .unwrap();
    let data = DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: id('D'),
        owner: owner(),
        visible_at: time(3),
        as_of: time(2),
        schema_hash: hash(b"schema"),
        manifest_hash: hash(MANIFEST_BYTES),
        blob_content_hash: hash(PARQUET_BYTES),
        lineage: vec![LineageRef::versioned(id('J'), Version::new(1).unwrap())],
    })
    .unwrap();
    let universe = UniverseSnapshot::new(
        id('U'),
        owner(),
        vec![VersionRef::new(id('J'), Version::new(1).unwrap())],
        hash(b"filter"),
        hash(MEMBERS_BYTES),
        vec![LineageRef::versioned(id('J'), Version::new(1).unwrap())],
    )
    .unwrap();
    Fixture {
        artifact,
        signal,
        data,
        universe,
    }
}

fn facade<'a>(
    metadata: &'a Metadata,
    reader: &'a RecordingReader,
    sink: &'a RecordingSink,
) -> VerifiedReadFacade<'a> {
    VerifiedReadFacade::new(metadata, metadata, metadata, reader, sink)
}

fn request(
    scope: AccessScope,
    owner: OwnerRef,
    kind: VerifiedReadResourceKind,
    role: VerifiedBlobRole,
    bytes: &[u8],
) -> Result<RequiredVerifiedBlobRead, ApplicationError> {
    RequiredVerifiedBlobRead::new(
        scope,
        owner,
        kind,
        id('A'),
        role,
        hash(bytes),
        bytes.len() as u64,
        trace(),
    )
}

fn bytes_for(role: VerifiedBlobRole) -> &'static [u8] {
    match role {
        VerifiedBlobRole::ArtifactPayload | VerifiedBlobRole::SignalPayload => SIGNAL_BYTES,
        VerifiedBlobRole::DataParquet => PARQUET_BYTES,
        VerifiedBlobRole::DataManifest => MANIFEST_BYTES,
        VerifiedBlobRole::UniverseMembersManifest => MEMBERS_BYTES,
        VerifiedBlobRole::CurvePoints => CURVE_BYTES,
    }
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('B'), vec![id('Y')]).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('Y'))
}

fn trace() -> SafeTraceContext {
    SafeTraceContext::new("0123456789abcdef0123456789abcdef").unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => 'J',
        'O' => 'Q',
        'U' => 'W',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-04".parse().unwrap(),
    )
    .unwrap()
}

fn hash(bytes: &[u8]) -> ContentHash {
    ContentHash::digest(bytes)
}

fn assert_error(error: &ApplicationError, category: ApplicationErrorCategory, retryable: bool) {
    assert_eq!(error.category(), category);
    assert_eq!(error.retryable(), retryable);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap()
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}
