use std::sync::Arc;

use ficant_application::ports::{
    ARTIFACT_READ_SCOPE, AeadCursorCodec, ArtifactRepository, AuthorizedPrincipal, Cursor,
    IntegrityEventSink, SafeTraceContext, SignalRepository, SnapshotVerifiedReadMetadataRepository,
    VerifiedBlobReader,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, VerifiedReadFacade, map_domain_error,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::research::v1 as pb;
use ficant_contracts::ficant::research::v1::artifact_service_server::ArtifactService;
use ficant_domain::governance::PlatformRole;
use ficant_domain::primitives::{ContentHash, LineageRef};
use ficant_domain::research::{Artifact, ArtifactKind, SignalSet};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged};
use prost::Message;
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::grpc_web::request_credential;
use crate::market_definition::{hash, market_time, owner, parse_ulid, ulid, version_ref};
use crate::registry::PlatformPort;

const DEFAULT_PAGE_SIZE: u32 = 50;
const MAX_PAGE_SIZE: u32 = 1_000;
const CURSOR_DOMAIN: &str = "r6b-lineage-v1";

/// Authenticated verified-read boundary for immutable Artifact and `SignalSet` metadata.
#[derive(Clone)]
pub struct ArtifactGrpcService {
    identity: Arc<dyn PlatformPort>,
    artifacts: Arc<dyn ArtifactRepository>,
    signals: Arc<dyn SignalRepository>,
    snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
    blob_reader: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
    cursor_codec: Arc<AeadCursorCodec>,
    errors: CoreBusinessErrorMapper,
}

impl ArtifactGrpcService {
    /// Composes the verified Artifact query surface from production-owned ports.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the safe business-error trace key is invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        artifacts: Arc<dyn ArtifactRepository>,
        signals: Arc<dyn SignalRepository>,
        snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
        blob_reader: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
        cursor_codec: Arc<AeadCursorCodec>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            artifacts,
            signals,
            snapshots,
            blob_reader,
            integrity_events,
            cursor_codec,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn principal(
        &self,
        request: &Request<impl Sized>,
    ) -> Result<AuthorizedPrincipal, ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|_| forbidden())?;
        let principal = session.authorized_principal()?;
        principal.require_role(PlatformRole::Researcher)?;
        principal
            .has_scope(ARTIFACT_READ_SCOPE)
            .then_some(principal)
            .ok_or_else(forbidden)
    }

    fn facade(&self) -> VerifiedReadFacade<'_> {
        VerifiedReadFacade::new(
            self.artifacts.as_ref(),
            self.signals.as_ref(),
            self.snapshots.as_ref(),
            self.blob_reader.as_ref(),
            self.integrity_events.as_ref(),
        )
    }

    fn error(&self, operation: &str, error: &ApplicationError) -> core::ErrorDetail {
        self.errors.map(operation, "artifact-application", error)
    }
}

#[tonic::async_trait]
impl ArtifactService for ArtifactGrpcService {
    async fn get_artifact(
        &self,
        request: Request<pb::GetArtifactRequest>,
    ) -> Result<Response<pb::GetArtifactResponse>, Status> {
        const OPERATION: &str = "artifacts.get";
        let result = match self.principal(&request) {
            Err(error) => Err(error),
            Ok(principal) => match parse_ulid(request.get_ref().artifact_id.as_ref()) {
                Err(error) => Err(error),
                Ok(artifact_id) => self
                    .facade()
                    .read_verified_artifact(
                        principal.access_scope(),
                        artifact_id,
                        trace_context(request.get_ref()),
                    )
                    .await
                    .map(|value| artifact(value.artifact())),
            },
        };
        Ok(Response::new(pb::GetArtifactResponse {
            result: Some(match result {
                Ok(value) => pb::get_artifact_response::Result::Artifact(value),
                Err(error) => {
                    pb::get_artifact_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn get_signal_set(
        &self,
        request: Request<pb::GetSignalSetRequest>,
    ) -> Result<Response<pb::GetSignalSetResponse>, Status> {
        const OPERATION: &str = "artifacts.get-signal-set";
        let result = match self.principal(&request) {
            Err(error) => Err(error),
            Ok(principal) => match parse_ulid(request.get_ref().signal_set_id.as_ref()) {
                Err(error) => Err(error),
                Ok(signal_id) => self
                    .facade()
                    .read_verified_signal(
                        principal.access_scope(),
                        signal_id,
                        trace_context(request.get_ref()),
                    )
                    .await
                    .map(|value| signal_set(value.signal())),
            },
        };
        Ok(Response::new(pb::GetSignalSetResponse {
            result: Some(match result {
                Ok(value) => pb::get_signal_set_response::Result::SignalSet(value),
                Err(error) => {
                    pb::get_signal_set_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn read_artifact_lineage(
        &self,
        request: Request<pb::ReadArtifactLineageRequest>,
    ) -> Result<Response<pb::ReadArtifactLineageResponse>, Status> {
        const OPERATION: &str = "artifacts.read-lineage";
        let result = match self.principal(&request) {
            Err(error) => Err(error),
            Ok(principal) => match parse_ulid(request.get_ref().artifact_id.as_ref()) {
                Err(error) => Err(error),
                Ok(artifact_id) => match self
                    .facade()
                    .read_verified_artifact(
                        principal.access_scope(),
                        artifact_id,
                        trace_context(request.get_ref()),
                    )
                    .await
                {
                    Err(error) => Err(error),
                    Ok(value) => paginate_lineage(
                        self.cursor_codec.as_ref(),
                        &principal,
                        LineageKind::Artifact,
                        value.artifact().id().as_str(),
                        value.artifact().content_hash(),
                        value.artifact().lineage(),
                        request.get_ref().page.as_ref(),
                    ),
                },
            },
        };
        Ok(Response::new(pb::ReadArtifactLineageResponse {
            result: Some(match result {
                Ok(value) => pb::read_artifact_lineage_response::Result::LineagePage(value),
                Err(error) => {
                    pb::read_artifact_lineage_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn read_signal_set_lineage(
        &self,
        request: Request<pb::ReadSignalSetLineageRequest>,
    ) -> Result<Response<pb::ReadSignalSetLineageResponse>, Status> {
        const OPERATION: &str = "artifacts.read-signal-lineage";
        let result = match self.principal(&request) {
            Err(error) => Err(error),
            Ok(principal) => match parse_ulid(request.get_ref().signal_set_id.as_ref()) {
                Err(error) => Err(error),
                Ok(signal_id) => match self
                    .facade()
                    .read_verified_signal(
                        principal.access_scope(),
                        signal_id,
                        trace_context(request.get_ref()),
                    )
                    .await
                {
                    Err(error) => Err(error),
                    Ok(value) => paginate_lineage(
                        self.cursor_codec.as_ref(),
                        &principal,
                        LineageKind::SignalSet,
                        value.signal().id().as_str(),
                        value.signal().content_hash(),
                        value.signal().lineage(),
                        request.get_ref().page.as_ref(),
                    ),
                },
            },
        };
        Ok(Response::new(pb::ReadSignalSetLineageResponse {
            result: Some(match result {
                Ok(value) => pb::read_signal_set_lineage_response::Result::LineagePage(value),
                Err(error) => pb::read_signal_set_lineage_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }
}

#[derive(Clone, Copy)]
enum LineageKind {
    Artifact,
    SignalSet,
}

impl LineageKind {
    const fn token(self) -> &'static str {
        match self {
            Self::Artifact => "artifact",
            Self::SignalSet => "signal-set",
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paginate_lineage(
    codec: &AeadCursorCodec,
    principal: &AuthorizedPrincipal,
    kind: LineageKind,
    resource_id: &str,
    content_hash: &ContentHash,
    lineage: &[LineageRef],
    request: Option<&core::PageRequest>,
) -> Result<pb::LineagePage, ApplicationError> {
    let request = request.cloned().unwrap_or_default();
    let page_size = if request.page_size == 0 {
        DEFAULT_PAGE_SIZE
    } else {
        request.page_size
    };
    if page_size > MAX_PAGE_SIZE {
        return Err(invalid());
    }
    let start = if request.cursor.is_empty() {
        0
    } else {
        let cursor = Cursor::resume(codec, principal.access_scope(), request.cursor.clone())?;
        parse_cursor(
            &cursor,
            principal,
            kind,
            resource_id,
            content_hash,
            page_size,
        )?
    };
    if start > lineage.len() {
        return Err(forbidden());
    }
    let limit = usize::try_from(page_size).map_err(|_| invalid())?;
    let end = start
        .checked_add(limit)
        .map(|value| value.min(lineage.len()))
        .ok_or_else(invalid)?;
    let next_cursor = if end < lineage.len() {
        let opaque = cursor_plaintext(principal, kind, resource_id, content_hash, page_size, end);
        Some(Cursor::issue(codec, principal.access_scope(), opaque)?)
    } else {
        None
    };
    Ok(pb::LineagePage {
        lineage: lineage[start..end].iter().map(lineage_ref).collect(),
        page: Some(core::PageResponse {
            next_cursor: next_cursor.map_or_else(String::new, |value| value.as_str().to_owned()),
        }),
    })
}

fn parse_cursor(
    cursor: &Cursor,
    principal: &AuthorizedPrincipal,
    kind: LineageKind,
    resource_id: &str,
    content_hash: &ContentHash,
    page_size: u32,
) -> Result<usize, ApplicationError> {
    let fields = cursor.opaque_value().split(':').collect::<Vec<_>>();
    if fields.len() != 7
        || fields[0] != CURSOR_DOMAIN
        || fields[1] != hash_hex(principal.fingerprint().content_hash())
        || fields[2] != kind.token()
        || fields[3] != resource_id
        || fields[4] != hash_hex(content_hash)
        || fields[5] != page_size.to_string()
    {
        return Err(forbidden());
    }
    fields[6].parse::<usize>().map_err(|_| forbidden())
}

fn cursor_plaintext(
    principal: &AuthorizedPrincipal,
    kind: LineageKind,
    resource_id: &str,
    content_hash: &ContentHash,
    page_size: u32,
    next_index: usize,
) -> String {
    format!(
        "{CURSOR_DOMAIN}:{}:{}:{resource_id}:{}:{page_size}:{next_index}",
        hash_hex(principal.fingerprint().content_hash()),
        kind.token(),
        hash_hex(content_hash),
    )
}

fn artifact(value: &Artifact) -> pb::Artifact {
    pb::Artifact {
        artifact_id: Some(ulid(value.id())),
        owner: Some(owner(value.owner())),
        kind: match value.kind() {
            ArtifactKind::Generic => pb::ArtifactKind::Generic as i32,
            ArtifactKind::SignalSet => pb::ArtifactKind::SignalSet as i32,
        },
        media_type: value.media_type().to_owned(),
        content_hash: Some(hash(value.content_hash())),
        blob_size: value.blob_size(),
        lineage: value.lineage().iter().map(lineage_ref).collect(),
    }
}

fn signal_set(value: &SignalSet) -> pb::SignalSet {
    pb::SignalSet {
        signal_set_id: Some(ulid(value.id())),
        owner: Some(owner(value.owner())),
        artifact: Some(lineage_ref(value.artifact())),
        experiment_run_id: Some(ulid(value.experiment_run_id())),
        data_snapshot: Some(lineage_ref(value.data_snapshot())),
        universe_snapshot: Some(lineage_ref(value.universe_snapshot())),
        rule_packs: value.rule_packs().iter().map(version_ref).collect(),
        input_artifacts: value.input_artifacts().iter().map(lineage_ref).collect(),
        valid_from: Some(market_time(value.valid().from())),
        valid_to: Some(market_time(value.valid().to())),
    }
}

fn lineage_ref(value: &LineageRef) -> core::LineageRef {
    core::LineageRef {
        object_id: Some(ulid(value.object_id())),
        version: value
            .version()
            .map_or(0, ficant_domain::primitives::Version::get),
        content_hash: value.content_hash().map(hash),
    }
}

fn trace_context(message: &impl Message) -> SafeTraceContext {
    let digest = ContentHash::digest(&message.encode_to_vec());
    let token =
        digest.as_bytes()[..16]
            .iter()
            .fold(String::with_capacity(32), |mut output, byte| {
                use std::fmt::Write as _;
                write!(output, "{byte:02x}").expect("writing to String cannot fail");
                output
            });
    SafeTraceContext::new(token).expect("derived trace token is canonical")
}

fn hash_hex(value: &ContentHash) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            use std::fmt::Write as _;
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

fn invalid() -> ApplicationError {
    map_domain_error(DomainErrorCode::InvalidValue)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}
