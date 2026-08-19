use std::sync::Arc;

use chrono::{DateTime, Utc};
use ficant_application::ports::{
    AeadCursorCodec, Cursor, FoundationChangeFilter, FoundationChangeRepository, PageRequest,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, FoundationChangeUseCase, map_domain_error,
};
use ficant_contracts::ficant::core::v1 as pb;
use ficant_contracts::ficant::core::v1::foundation_change_service_server::FoundationChangeService;
use ficant_domain::governance::{FoundationChangeRecord, PlatformRole};
use ficant_domain::primitives::{ContentHash, Ulid, VersionRef};
use prost_types::Timestamp;
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;

const READ_SCOPE: &str = "governance:read";

#[derive(Clone)]
pub struct FoundationChangeGrpcService {
    identity: Arc<dyn PlatformPort>,
    repository: Arc<dyn FoundationChangeRepository>,
    cursor_codec: Arc<AeadCursorCodec>,
    errors: CoreBusinessErrorMapper,
}

impl FoundationChangeGrpcService {
    /// Creates the Platform Admin-only foundation change query adapter.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the trace key is invalid.
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        repository: Arc<dyn FoundationChangeRepository>,
        cursor_codec: Arc<AeadCursorCodec>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            repository,
            cursor_codec,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn principal(
        &self,
        request: &Request<impl Sized>,
    ) -> Result<ficant_application::ports::AuthorizedPrincipal, ApplicationError> {
        let credential = request_credential(request.metadata());
        let principal = self
            .identity
            .current_session(&credential)
            .map_err(|_| forbidden())?
            .authorized_principal()?;
        principal.require_role(PlatformRole::PlatformAdmin)?;
        principal
            .has_scope(READ_SCOPE)
            .then_some(principal)
            .ok_or_else(forbidden)
    }

    fn error(&self, operation: &str, error: &ApplicationError) -> pb::ErrorDetail {
        self.errors
            .map(operation, "foundation-change-application", error)
    }
}

#[tonic::async_trait]
impl FoundationChangeService for FoundationChangeGrpcService {
    async fn get_foundation_change(
        &self,
        request: Request<pb::GetFoundationChangeRequest>,
    ) -> Result<Response<pb::GetFoundationChangeResponse>, Status> {
        const OPERATION: &str = "governance.get-foundation-change";
        let result = match self.principal(&request) {
            Err(error) => Err(error),
            Ok(principal) => match parse_ulid(request.get_ref().record_id.as_ref()) {
                Err(error) => Err(error),
                Ok(record_id) => {
                    FoundationChangeUseCase::new(self.repository.as_ref())
                        .get_exact(&principal, &record_id)
                        .await
                }
            },
        };
        Ok(Response::new(pb::GetFoundationChangeResponse {
            result: Some(match result {
                Ok(value) => pb::get_foundation_change_response::Result::Change(change(&value)),
                Err(error) => {
                    pb::get_foundation_change_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn list_foundation_changes(
        &self,
        request: Request<pb::ListFoundationChangesRequest>,
    ) -> Result<Response<pb::ListFoundationChangesResponse>, Status> {
        const OPERATION: &str = "governance.list-foundation-changes";
        let result = match self.principal(&request) {
            Err(error) => Err(error),
            Ok(principal) => {
                let value = request.get_ref();
                match (|| {
                    let filter = FoundationChangeFilter::new(
                        (!value.resource_ref.is_empty()).then(|| value.resource_ref.clone()),
                        value
                            .actor_id
                            .as_ref()
                            .map(|actor| parse_ulid(Some(actor)))
                            .transpose()?,
                        value
                            .occurred_from
                            .as_ref()
                            .map(parse_timestamp)
                            .transpose()?,
                        value
                            .occurred_to
                            .as_ref()
                            .map(parse_timestamp)
                            .transpose()?,
                    )?;
                    let page = value.page.as_ref().ok_or_else(invalid)?;
                    let cursor = (!page.cursor.is_empty())
                        .then(|| {
                            Cursor::resume(
                                self.cursor_codec.as_ref(),
                                principal.access_scope(),
                                page.cursor.clone(),
                            )
                        })
                        .transpose()?;
                    let page =
                        PageRequest::new(principal.access_scope().clone(), cursor, page.page_size)?;
                    Ok((filter, page))
                })() {
                    Err(error) => Err(error),
                    Ok((filter, page)) => {
                        FoundationChangeUseCase::new(self.repository.as_ref())
                            .list(&principal, &filter, page)
                            .await
                    }
                }
            }
        };
        Ok(Response::new(pb::ListFoundationChangesResponse {
            result: Some(match result {
                Ok(page) => {
                    let (values, next) = page.into_parts();
                    pb::list_foundation_changes_response::Result::Changes(
                        pb::FoundationChangeRecords {
                            changes: values.iter().map(change).collect(),
                            page: Some(pb::PageResponse {
                                next_cursor: next
                                    .map_or_else(String::new, |value| value.as_str().to_owned()),
                            }),
                        },
                    )
                }
                Err(error) => pb::list_foundation_changes_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }
}

fn change(value: &FoundationChangeRecord) -> pb::FoundationChangeRecord {
    pb::FoundationChangeRecord {
        record_id: Some(ulid(value.record_id())),
        actor_id: Some(ulid(value.actor_id())),
        active_role: match value.active_role() {
            PlatformRole::PlatformAdmin => pb::PlatformRole::PlatformAdmin as i32,
            PlatformRole::Researcher => pb::PlatformRole::Researcher as i32,
        },
        operation: value.operation().as_str().to_owned(),
        resource_ref: value.resource().canonical_ref(),
        before_hash: value.before_hash().map(sha256),
        after_hash: Some(sha256(value.after_hash())),
        change: Some(pb::ChangeJustification {
            reason: value.change().reason().to_owned(),
            sources: value
                .change()
                .sources()
                .iter()
                .map(|source| pb::SourceDocumentRef {
                    uri: source.uri().to_owned(),
                    sha256: Some(sha256(source.sha256())),
                })
                .collect(),
        }),
        request_fingerprint: Some(sha256(value.request_fingerprint())),
        occurred_at: Some(timestamp(value.occurred_at().instant())),
        authorization_ref: value.authorization_ref().map(version_ref),
    }
}

fn parse_timestamp(
    value: &Timestamp,
) -> Result<ficant_domain::primitives::MarketTime, ApplicationError> {
    let nanos = u32::try_from(value.nanos).map_err(|_| invalid())?;
    let instant = DateTime::<Utc>::from_timestamp(value.seconds, nanos).ok_or_else(invalid)?;
    ficant_domain::primitives::MarketTime::new(instant, "UTC", instant.date_naive())
        .map_err(map_domain_error)
}

fn timestamp(value: DateTime<Utc>) -> Timestamp {
    Timestamp {
        seconds: value.timestamp(),
        nanos: i32::try_from(value.timestamp_subsec_nanos()).expect("nanoseconds fit i32"),
    }
}

fn parse_ulid(value: Option<&pb::Ulid>) -> Result<Ulid, ApplicationError> {
    Ulid::new(value.ok_or_else(invalid)?.value.clone()).map_err(map_domain_error)
}

fn ulid(value: &Ulid) -> pb::Ulid {
    pb::Ulid {
        value: value.as_str().to_owned(),
    }
}

fn sha256(value: &ContentHash) -> pb::Sha256 {
    pb::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn version_ref(value: &VersionRef) -> pb::VersionRef {
    pb::VersionRef {
        id: Some(ulid(value.id())),
        version: value.version().get(),
    }
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}
