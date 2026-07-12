use async_trait::async_trait;
use ficant_application::ports::{
    ApplicationResult, IntegrityEvent, IntegrityEventSink, VerifiedBlobRole,
    VerifiedReadResourceKind,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

/// Emits the frozen integrity-event schema as one JSON line per event.
pub struct JsonLineIntegrityEventSink<W> {
    writer: Mutex<W>,
}

impl<W> JsonLineIntegrityEventSink<W> {
    /// Creates a production-capable sink around a writable observability destination.
    #[must_use]
    pub fn new(writer: W) -> Self {
        Self {
            writer: Mutex::new(writer),
        }
    }
}

#[async_trait]
impl<W> IntegrityEventSink for JsonLineIntegrityEventSink<W>
where
    W: Write + Send,
{
    async fn emit(&self, event: IntegrityEvent) -> ApplicationResult<()> {
        let payload = serde_json::json!({
            "event_name": event.name(),
            "severity": event.severity().as_str(),
            "reason": event.reason().as_str(),
            "tenant_id": event.tenant_id().as_str(),
            "resource_kind": resource_kind(event.resource_kind()),
            "resource_id": event.resource_id().as_str(),
            "blob_role": blob_role(event.blob_role()),
            "expected_hash": format_hex(event.expected_hash().as_bytes()),
            "expected_size": event.expected_size(),
            "trace_id": event.trace().trace_id(),
        });
        let mut line = serde_json::to_vec(&payload).map_err(|_| sink_error())?;
        line.push(b'\n');
        let mut writer = self.writer.lock().map_err(|_| sink_error())?;
        writer.write_all(&line).map_err(|_| sink_error())?;
        writer.flush().map_err(|_| sink_error())
    }
}

/// Constructs the process stderr integrity-event sink used by production composition.
#[must_use]
pub fn build_integrity_event_sink() -> Arc<dyn IntegrityEventSink> {
    Arc::new(JsonLineIntegrityEventSink::new(io::stderr()))
}

const fn resource_kind(kind: VerifiedReadResourceKind) -> &'static str {
    match kind {
        VerifiedReadResourceKind::Artifact => "artifact",
        VerifiedReadResourceKind::SignalSet => "signal_set",
        VerifiedReadResourceKind::DataSnapshot => "data_snapshot",
        VerifiedReadResourceKind::UniverseSnapshot => "universe_snapshot",
    }
}

const fn blob_role(role: VerifiedBlobRole) -> &'static str {
    match role {
        VerifiedBlobRole::ArtifactPayload => "artifact_payload",
        VerifiedBlobRole::SignalPayload => "signal_payload",
        VerifiedBlobRole::DataParquet => "data_parquet",
        VerifiedBlobRole::DataManifest => "data_manifest",
        VerifiedBlobRole::UniverseMembersManifest => "universe_members_manifest",
    }
}

fn format_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn sink_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
}
