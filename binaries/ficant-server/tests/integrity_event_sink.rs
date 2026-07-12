use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    AccessScope, IntegrityEventSink, IntegrityFailureReason, RequiredVerifiedBlobRead,
    SafeTraceContext, VerifiedBlobRole, VerifiedReadResourceKind,
};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid};
use ficant_server::{JsonLineIntegrityEventSink, build_integrity_event_sink};
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const TENANT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const ACTOR_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const OWNER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const RESOURCE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
const TRACE_ID: &str = "0123456789abcdef0123456789abcdef";
const RAW_PAYLOAD: &[u8] = b"raw-payload-must-never-be-observable";

#[tokio::test]
async fn event_is_one_fixed_schema_json_line_using_only_safe_accessors() {
    let output = SharedBuffer::default();
    let captured = output.clone();
    let sink = JsonLineIntegrityEventSink::new(output);
    let request = request();

    sink.emit(request.integrity_event(IntegrityFailureReason::HashMismatch))
        .await
        .expect("safe event is written");

    let line = captured.text();
    assert_eq!(line.lines().count(), 1, "one event must produce one line");
    assert_eq!(
        line.matches("storage.published_content_integrity_failure")
            .count(),
        1,
        "the event name must appear exactly once"
    );
    let value: Value = serde_json::from_str(line.trim_end()).expect("valid JSON line");
    let object = value.as_object().expect("event is a JSON object");
    let actual_keys = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected_keys = BTreeSet::from([
        "blob_role",
        "event_name",
        "expected_hash",
        "expected_size",
        "reason",
        "resource_id",
        "resource_kind",
        "severity",
        "tenant_id",
        "trace_id",
    ]);
    assert_eq!(actual_keys, expected_keys);
    assert_eq!(
        object["event_name"],
        "storage.published_content_integrity_failure"
    );
    assert_eq!(object["severity"], "error");
    assert_eq!(object["reason"], "hash_mismatch");
    assert_eq!(object["tenant_id"], TENANT_ID);
    assert_eq!(object["resource_kind"], "artifact");
    assert_eq!(object["resource_id"], RESOURCE_ID);
    assert_eq!(object["blob_role"], "artifact_payload");
    assert_eq!(object["expected_size"], 37);
    assert_eq!(object["trace_id"], TRACE_ID);

    let hash = object["expected_hash"].as_str().expect("hash is a string");
    assert_eq!(hash.len(), 64);
    assert!(
        hash.bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    );

    let visible = line.to_ascii_lowercase();
    for forbidden in [
        OWNER_ID.to_ascii_lowercase(),
        "owner".to_owned(),
        "bucket".to_owned(),
        "key".to_owned(),
        "endpoint".to_owned(),
        "credential".to_owned(),
        "token".to_owned(),
        "sql".to_owned(),
        "stack".to_owned(),
        "cause".to_owned(),
        "message".to_owned(),
        String::from_utf8_lossy(RAW_PAYLOAD).to_ascii_lowercase(),
    ] {
        assert!(
            !visible.contains(&forbidden),
            "leaked {forbidden}: {visible}"
        );
    }
}

#[tokio::test]
async fn write_failure_is_reported_but_cannot_mask_the_integrity_error() {
    let direct_attempts = Arc::new(AtomicUsize::new(0));
    let direct_sink = JsonLineIntegrityEventSink::new(FailingWriter(direct_attempts.clone()));
    let request = request();
    let sink_error = direct_sink
        .emit(request.integrity_event(IntegrityFailureReason::Missing))
        .await
        .expect_err("writer failure is reported to direct callers");
    assert_eq!(
        sink_error.category(),
        ApplicationErrorCategory::StorageUnavailable
    );
    assert!(sink_error.retryable());
    assert_eq!(direct_attempts.load(Ordering::SeqCst), 1);

    let integrity_attempts = Arc::new(AtomicUsize::new(0));
    let integrity_sink = JsonLineIntegrityEventSink::new(FailingWriter(integrity_attempts.clone()));
    let integrity_error = request
        .fail_integrity(&integrity_sink, IntegrityFailureReason::Missing)
        .await;
    assert_eq!(
        integrity_error.category(),
        ApplicationErrorCategory::HashMismatch
    );
    assert!(!integrity_error.retryable());
    assert_eq!(
        integrity_attempts.load(Ordering::SeqCst),
        1,
        "one integrity failure must attempt exactly one event"
    );
}

#[test]
fn production_composition_constructs_the_real_integrity_sink() {
    fn accepts_production_sink(_: Arc<dyn IntegrityEventSink>) {}

    accepts_production_sink(build_integrity_event_sink());
}

fn request() -> RequiredVerifiedBlobRead {
    let tenant_id = Ulid::new(TENANT_ID).expect("valid tenant id");
    let actor_id = Ulid::new(ACTOR_ID).expect("valid actor id");
    let owner_id = Ulid::new(OWNER_ID).expect("valid owner id");
    let scope = AccessScope::new(tenant_id.clone(), actor_id, vec![owner_id.clone()])
        .expect("valid access scope");
    RequiredVerifiedBlobRead::new(
        scope,
        OwnerRef::new(tenant_id, owner_id),
        VerifiedReadResourceKind::Artifact,
        Ulid::new(RESOURCE_ID).expect("valid resource id"),
        VerifiedBlobRole::ArtifactPayload,
        ContentHash::digest(RAW_PAYLOAD),
        37,
        SafeTraceContext::new(TRACE_ID).expect("safe trace"),
    )
    .expect("valid required read")
}

#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("buffer lock").clone()).expect("UTF-8 output")
    }
}

impl Write for SharedBuffer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("buffer lock")
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FailingWriter(Arc<AtomicUsize>);

impl Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(io::Error::other("observable sink unavailable"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
