use crate::core_error::CoreBusinessErrorMapper;
use crate::error::{PlatformFailure, PlatformFailureCode};
use crate::grpc_web::request_credential;
use crate::market_definition::{owner, parse_change, parse_owner, server_market_time};
use crate::registry::PlatformPort;
use chrono::{DateTime, Utc};
use ficant_application::ports::{
    AuthorizedPrincipal, FoundationChangeContext, GovernedPublishSubjectState,
    GovernedRegisterSubject, IdempotencyKey, SubjectRepository,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1 as pb;
use ficant_contracts::ficant::core::v1::registry_service_server::RegistryService;
use ficant_domain::governance::{
    FoundationResourceKind, FoundationResourceRef, PlatformRole, deterministic_change_record_id,
};
use ficant_domain::primitives::{DecimalValue, Ulid, UnitRef, Version, VersionRef};
use ficant_domain::subject::{
    AccessSet, ConstraintSetRef, FundingTier, LimitCeiling, Subject, SubjectRecord,
    SubjectStateSnapshot, SubjectVersion, TaxTreatment,
};
use prost_types::Timestamp;
use std::sync::Arc;
use tonic::{Request, Response, Status};

const READ_SCOPE: &str = "registry:read";
const WRITE_SCOPE: &str = "registry:write";

#[derive(Clone)]
pub struct SubjectRegistryGrpcService {
    identity: Arc<dyn PlatformPort>,
    repository: Arc<dyn SubjectRepository>,
    errors: CoreBusinessErrorMapper,
}

impl SubjectRegistryGrpcService {
    /// Creates an authenticated Subject Registry transport adapter.
    ///
    /// # Errors
    ///
    /// Returns an error when the trace-signing key is shorter than the frozen minimum.
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        repository: Arc<dyn SubjectRepository>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            repository,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn authorize(
        &self,
        request: &Request<impl Sized>,
        required_scope: &str,
        required_role: Option<PlatformRole>,
    ) -> Result<AuthorizedPrincipal, ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|failure| platform_application_error(&failure))?;
        let principal = session.authorized_principal()?;
        if !principal.has_scope(required_scope) {
            return Err(forbidden());
        }
        if let Some(role) = required_role {
            principal.require_role(role)?;
        }
        Ok(principal)
    }

    fn error(
        &self,
        operation: &str,
        error: &ApplicationError,
    ) -> ficant_contracts::ficant::core::v1::ErrorDetail {
        self.errors
            .map(operation, "subject-registry-application", error)
    }

    async fn register_subject_command(
        &self,
        principal: AuthorizedPrincipal,
        request: &pb::RegisterSubjectRequest,
    ) -> Result<SubjectRecord, ApplicationError> {
        let value = parse_subject_record(request)?;
        let idempotency_key = IdempotencyKey::new(request.idempotency_key.clone())?;
        let change = parse_change(request.change.as_ref())?;
        let resource = FoundationResourceRef::versioned(
            FoundationResourceKind::Subject,
            value.version().reference().clone(),
        );
        let occurred_at = server_market_time();
        let record_id = deterministic_change_record_id(
            &occurred_at,
            principal.actor_id(),
            &resource,
            idempotency_key.as_str(),
        )
        .map_err(map_domain_error)?;
        let context =
            FoundationChangeContext::administrator(principal, change, record_id, occurred_at)?;
        self.repository
            .register_governed_subject(GovernedRegisterSubject::new(
                context,
                value,
                idempotency_key,
            )?)
            .await
    }

    async fn publish_subject_state_command(
        &self,
        principal: AuthorizedPrincipal,
        request: &pb::RegisterSubjectStateRequest,
    ) -> Result<SubjectStateSnapshot, ApplicationError> {
        let value = parse_subject_state(request.snapshot.as_ref())?;
        let idempotency_key = IdempotencyKey::new(request.idempotency_key.clone())?;
        let change = parse_change(request.change.as_ref())?;
        let resource = FoundationResourceRef::unversioned(
            FoundationResourceKind::SubjectState,
            value.id().clone(),
        );
        let occurred_at = server_market_time();
        let record_id = deterministic_change_record_id(
            &occurred_at,
            principal.actor_id(),
            &resource,
            idempotency_key.as_str(),
        )
        .map_err(map_domain_error)?;
        let context =
            FoundationChangeContext::administrator(principal, change, record_id, occurred_at)?;
        self.repository
            .publish_governed_subject_state(GovernedPublishSubjectState::new(
                context,
                value,
                idempotency_key,
            )?)
            .await
    }
}

#[tonic::async_trait]
impl RegistryService for SubjectRegistryGrpcService {
    async fn register_subject(
        &self,
        request: Request<pb::RegisterSubjectRequest>,
    ) -> Result<Response<pb::RegisterSubjectResponse>, Status> {
        const OPERATION: &str = "registry.register-subject";
        let result = match self.authorize(&request, WRITE_SCOPE, Some(PlatformRole::PlatformAdmin))
        {
            Err(error) => Err(error),
            Ok(principal) => {
                self.register_subject_command(principal, request.get_ref())
                    .await
            }
        };
        Ok(Response::new(pb::RegisterSubjectResponse {
            result: Some(match result {
                Ok(value) => pb::register_subject_response::Result::Subject(subject_record(&value)),
                Err(error) => {
                    pb::register_subject_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn get_subject(
        &self,
        request: Request<pb::GetSubjectRequest>,
    ) -> Result<Response<pb::GetSubjectResponse>, Status> {
        const OPERATION: &str = "registry.get-subject";
        let result = match self.authorize(&request, READ_SCOPE, None) {
            Err(error) => Err(error),
            Ok(principal) => match parse_version_ref(request.get_ref().subject_ref.as_ref()) {
                Err(error) => Err(error),
                Ok(reference) => self
                    .repository
                    .get_subject_scoped(principal.access_scope(), reference)
                    .await
                    .and_then(|value| value.ok_or_else(not_found)),
            },
        };
        Ok(Response::new(pb::GetSubjectResponse {
            result: Some(match result {
                Ok(value) => pb::get_subject_response::Result::Subject(subject_record(&value)),
                Err(error) => {
                    pb::get_subject_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn register_subject_state(
        &self,
        request: Request<pb::RegisterSubjectStateRequest>,
    ) -> Result<Response<pb::RegisterSubjectStateResponse>, Status> {
        const OPERATION: &str = "registry.register-subject-state";
        let result = match self.authorize(&request, WRITE_SCOPE, Some(PlatformRole::PlatformAdmin))
        {
            Err(error) => Err(error),
            Ok(principal) => {
                self.publish_subject_state_command(principal, request.get_ref())
                    .await
            }
        };
        Ok(Response::new(pb::RegisterSubjectStateResponse {
            result: Some(match result {
                Ok(value) => {
                    pb::register_subject_state_response::Result::Snapshot(state_snapshot(&value))
                }
                Err(error) => pb::register_subject_state_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn get_subject_state(
        &self,
        request: Request<pb::GetSubjectStateRequest>,
    ) -> Result<Response<pb::GetSubjectStateResponse>, Status> {
        const OPERATION: &str = "registry.get-subject-state";
        let result = match self.authorize(&request, READ_SCOPE, None) {
            Err(error) => Err(error),
            Ok(principal) => {
                let snapshot_id = parse_ulid(request.get_ref().snapshot_id.as_ref());
                let knowledge_at = parse_timestamp(request.get_ref().knowledge_at.as_ref());
                match (snapshot_id, knowledge_at) {
                    (Ok(snapshot_id), Ok(knowledge_at)) => self
                        .repository
                        .get_subject_state_scoped(
                            principal.access_scope(),
                            snapshot_id,
                            knowledge_at,
                        )
                        .await
                        .and_then(|value| value.ok_or_else(not_found)),
                    (Err(error), _) | (_, Err(error)) => Err(error),
                }
            }
        };
        Ok(Response::new(pb::GetSubjectStateResponse {
            result: Some(match result {
                Ok(value) => {
                    pb::get_subject_state_response::Result::Snapshot(state_snapshot(&value))
                }
                Err(error) => {
                    pb::get_subject_state_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }
}

fn parse_subject_record(
    request: &pb::RegisterSubjectRequest,
) -> Result<SubjectRecord, ApplicationError> {
    let subject = request.subject.as_ref().ok_or_else(invalid)?;
    let version = request.subject_version.as_ref().ok_or_else(invalid)?;
    SubjectRecord::new(parse_subject(subject)?, parse_subject_version(version)?)
        .map_err(map_domain_error)
}

fn parse_subject(value: &pb::Subject) -> Result<Subject, ApplicationError> {
    Subject::new_owned(
        parse_ulid(value.subject_id.as_ref())?,
        parse_owner(value.owner.as_ref())?,
        value.display_name.clone(),
    )
    .map_err(map_domain_error)
}

fn parse_subject_version(value: &pb::SubjectVersion) -> Result<SubjectVersion, ApplicationError> {
    let reference = parse_version_ref(value.subject_ref.as_ref())?;
    let access = value.access_set.as_ref().ok_or_else(invalid)?;
    let tax = value.tax_treatment.as_ref().ok_or_else(invalid)?;
    let constraint = value
        .constraint_set_ref
        .as_ref()
        .map(|value| parse_version_ref(value.r#ref.as_ref()).map(ConstraintSetRef::new))
        .transpose()?;
    let funding_tier = match pb::FundingTier::try_from(value.funding_tier).map_err(|_| invalid())? {
        pb::FundingTier::DrAvailable => FundingTier::DrAvailable,
        pb::FundingTier::ROnly => FundingTier::ROnly,
        pb::FundingTier::Unspecified => return Err(invalid()),
    };
    SubjectVersion::new(
        reference,
        AccessSet::new(access.market_codes.clone(), access.tool_codes.clone())
            .map_err(map_domain_error)?,
        funding_tier,
        TaxTreatment::new(
            tax.value_added_tax_profile.clone(),
            tax.income_tax_profile.clone(),
        )
        .map_err(map_domain_error)?,
        value.assessment_mechanism.clone(),
        value.liability_profile.clone(),
        constraint,
    )
    .map_err(map_domain_error)
}

fn parse_subject_state(
    value: Option<&pb::SubjectStateSnapshot>,
) -> Result<SubjectStateSnapshot, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let observed_at = parse_timestamp(value.observed_at.as_ref())?;
    let visible_at = parse_timestamp(value.visible_at.as_ref())?;
    let net_capital = parse_decimal(value.net_capital.as_ref())?;
    let limits = value
        .limit_ceilings
        .iter()
        .map(|value| {
            LimitCeiling::new(
                value.limit_code.clone(),
                parse_decimal(value.ceiling.as_ref())?,
            )
            .map_err(map_domain_error)
        })
        .collect::<Result<Vec<_>, _>>()?;
    SubjectStateSnapshot::new_owned(
        parse_ulid(value.snapshot_id.as_ref())?,
        parse_version_ref(value.subject_ref.as_ref())?,
        net_capital,
        limits,
        observed_at,
        visible_at,
        value.market_timezone.clone(),
        parse_owner(value.owner.as_ref())?,
    )
    .map_err(map_domain_error)
}

fn parse_decimal(value: Option<&pb::DecimalValue>) -> Result<DecimalValue, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let unit = value.unit.as_ref().ok_or_else(invalid)?;
    DecimalValue::new(
        value.coefficient.clone(),
        value.scale,
        UnitRef::new(
            parse_ulid(unit.unit_id.as_ref())?,
            Version::new(unit.version).map_err(map_domain_error)?,
        ),
    )
    .map_err(map_domain_error)
}

fn parse_version_ref(value: Option<&pb::VersionRef>) -> Result<VersionRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(VersionRef::new(
        parse_ulid(value.id.as_ref())?,
        Version::new(value.version).map_err(map_domain_error)?,
    ))
}

fn parse_ulid(value: Option<&pb::Ulid>) -> Result<Ulid, ApplicationError> {
    Ulid::new(value.ok_or_else(invalid)?.value.clone()).map_err(map_domain_error)
}

fn parse_timestamp(value: Option<&Timestamp>) -> Result<DateTime<Utc>, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    if !(0..1_000_000_000).contains(&value.nanos) {
        return Err(invalid());
    }
    DateTime::<Utc>::from_timestamp(value.seconds, value.nanos.cast_unsigned()).ok_or_else(invalid)
}

fn subject_record(value: &SubjectRecord) -> pb::SubjectRecord {
    pb::SubjectRecord {
        subject: Some(pb::Subject {
            subject_id: Some(pb::Ulid {
                value: value.subject().id().as_str().to_owned(),
            }),
            display_name: value.subject().display_name().to_owned(),
            owner: value.subject().owner().map(owner),
        }),
        subject_version: Some(subject_version(value.version())),
    }
}

fn subject_version(value: &SubjectVersion) -> pb::SubjectVersion {
    pb::SubjectVersion {
        subject_ref: Some(version_ref(value.reference())),
        access_set: Some(pb::AccessSet {
            market_codes: value.access_set().market_codes().to_vec(),
            tool_codes: value.access_set().tool_codes().to_vec(),
        }),
        funding_tier: match value.funding_tier() {
            FundingTier::DrAvailable => pb::FundingTier::DrAvailable as i32,
            FundingTier::ROnly => pb::FundingTier::ROnly as i32,
        },
        tax_treatment: Some(pb::TaxTreatment {
            value_added_tax_profile: value.tax_treatment().value_added_tax_profile().to_owned(),
            income_tax_profile: value.tax_treatment().income_tax_profile().to_owned(),
        }),
        assessment_mechanism: value.assessment_mechanism().to_owned(),
        liability_profile: value.liability_profile().to_owned(),
        constraint_set_ref: value
            .constraint_set_ref()
            .map(|value| pb::ConstraintSetRef {
                r#ref: Some(version_ref(value.reference())),
            }),
    }
}

fn state_snapshot(value: &SubjectStateSnapshot) -> pb::SubjectStateSnapshot {
    pb::SubjectStateSnapshot {
        snapshot_id: Some(pb::Ulid {
            value: value.id().as_str().to_owned(),
        }),
        subject_ref: Some(version_ref(value.subject_ref())),
        net_capital: Some(decimal(value.net_capital())),
        limit_ceilings: value
            .limit_ceilings()
            .iter()
            .map(|value| pb::LimitCeiling {
                limit_code: value.limit_code().to_owned(),
                ceiling: Some(decimal(value.ceiling())),
            })
            .collect(),
        observed_at: Some(timestamp(value.observed_at())),
        visible_at: Some(timestamp(value.visible_at())),
        market_timezone: value.market_timezone().to_owned(),
        owner: value.owner().map(owner),
    }
}

fn version_ref(value: &VersionRef) -> pb::VersionRef {
    pb::VersionRef {
        id: Some(pb::Ulid {
            value: value.id().as_str().to_owned(),
        }),
        version: value.version().get(),
    }
}

fn decimal(value: &DecimalValue) -> pb::DecimalValue {
    pb::DecimalValue {
        coefficient: value.coefficient().to_owned(),
        scale: value.scale(),
        unit: Some(pb::UnitRef {
            unit_id: Some(pb::Ulid {
                value: value.unit().unit_id().as_str().to_owned(),
            }),
            version: value.unit().version().get(),
        }),
    }
}

fn timestamp(value: DateTime<Utc>) -> Timestamp {
    Timestamp {
        seconds: value.timestamp(),
        nanos: value.timestamp_subsec_nanos().cast_signed(),
    }
}

fn platform_application_error(failure: &PlatformFailure) -> ApplicationError {
    let (category, retryable) = match failure.code() {
        PlatformFailureCode::Unauthenticated | PlatformFailureCode::Expired => {
            (ApplicationErrorCategory::Unauthenticated, false)
        }
        PlatformFailureCode::Forbidden => (ApplicationErrorCategory::Forbidden, false),
        PlatformFailureCode::NotFound => (ApplicationErrorCategory::NotFound, false),
        PlatformFailureCode::InvalidRequest => (ApplicationErrorCategory::ValidationFailed, false),
        PlatformFailureCode::Unavailable | PlatformFailureCode::Internal => {
            (ApplicationErrorCategory::StorageUnavailable, true)
        }
    };
    ApplicationError::new(category, retryable)
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::TrustedIdentity;
    use chrono::{DateTime, Utc};
    use ficant_application::ports::SubjectRepository;
    use ficant_contracts::ficant::core::v1::registry_service_server::RegistryService;
    use ficant_domain::governance::PlatformRole;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[derive(Default)]
    struct MemoryRepository {
        subjects: Mutex<BTreeMap<(String, u64), SubjectRecord>>,
        states: Mutex<BTreeMap<String, SubjectStateSnapshot>>,
        governed_writes: AtomicUsize,
        legacy_writes: AtomicUsize,
    }

    #[tonic::async_trait]
    impl SubjectRepository for MemoryRepository {
        async fn register_governed_subject(
            &self,
            command: GovernedRegisterSubject,
        ) -> Result<SubjectRecord, ApplicationError> {
            self.governed_writes.fetch_add(1, Ordering::SeqCst);
            let value = command.value().clone();
            let key = (
                value.subject().id().as_str().to_owned(),
                value.version().reference().version().get(),
            );
            let mut values = self.subjects.lock().unwrap();
            if let Some(existing) = values.get(&key) {
                return if existing == &value {
                    Ok(existing.clone())
                } else {
                    Err(ApplicationError::new(
                        ApplicationErrorCategory::ImmutableViolation,
                        false,
                    ))
                };
            }
            values.insert(key, value.clone());
            Ok(value)
        }

        async fn publish_governed_subject_state(
            &self,
            command: GovernedPublishSubjectState,
        ) -> Result<SubjectStateSnapshot, ApplicationError> {
            self.governed_writes.fetch_add(1, Ordering::SeqCst);
            let value = command.value().clone();
            let subject = self
                .subjects
                .lock()
                .unwrap()
                .get(&(
                    value.subject_ref().id().as_str().to_owned(),
                    value.subject_ref().version().get(),
                ))
                .cloned()
                .ok_or_else(|| {
                    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
                })?;
            if subject.subject().owner() != value.owner() {
                return Err(ApplicationError::new(
                    ApplicationErrorCategory::ImmutableViolation,
                    false,
                ));
            }
            let mut values = self.states.lock().unwrap();
            if let Some(existing) = values.get(value.id().as_str()) {
                return if existing == &value {
                    Ok(existing.clone())
                } else {
                    Err(ApplicationError::new(
                        ApplicationErrorCategory::ImmutableViolation,
                        false,
                    ))
                };
            }
            values.insert(value.id().as_str().to_owned(), value.clone());
            Ok(value)
        }

        async fn register_subject(
            &self,
            value: SubjectRecord,
        ) -> Result<SubjectRecord, ApplicationError> {
            self.legacy_writes.fetch_add(1, Ordering::SeqCst);
            let key = (
                value.subject().id().as_str().to_owned(),
                value.version().reference().version().get(),
            );
            let mut values = self.subjects.lock().unwrap();
            if let Some(existing) = values.get(&key) {
                return if existing == &value {
                    Ok(existing.clone())
                } else {
                    Err(ApplicationError::new(
                        ApplicationErrorCategory::ImmutableViolation,
                        false,
                    ))
                };
            }
            values.insert(key, value.clone());
            Ok(value)
        }

        async fn get_subject(
            &self,
            reference: VersionRef,
        ) -> Result<Option<SubjectRecord>, ApplicationError> {
            Ok(self
                .subjects
                .lock()
                .unwrap()
                .get(&(
                    reference.id().as_str().to_owned(),
                    reference.version().get(),
                ))
                .cloned())
        }

        async fn register_subject_state(
            &self,
            value: SubjectStateSnapshot,
        ) -> Result<SubjectStateSnapshot, ApplicationError> {
            self.legacy_writes.fetch_add(1, Ordering::SeqCst);
            let mut values = self.states.lock().unwrap();
            if let Some(existing) = values.get(value.id().as_str()) {
                return if existing == &value {
                    Ok(existing.clone())
                } else {
                    Err(ApplicationError::new(
                        ApplicationErrorCategory::ImmutableViolation,
                        false,
                    ))
                };
            }
            values.insert(value.id().as_str().to_owned(), value.clone());
            Ok(value)
        }

        async fn get_subject_state(
            &self,
            snapshot_id: Ulid,
            knowledge_at: DateTime<Utc>,
        ) -> Result<Option<SubjectStateSnapshot>, ApplicationError> {
            Ok(self
                .states
                .lock()
                .unwrap()
                .get(snapshot_id.as_str())
                .filter(|value| value.visible_at() <= knowledge_at)
                .cloned())
        }
    }

    fn service_with_role(
        role: PlatformRole,
    ) -> (SubjectRegistryGrpcService, Arc<MemoryRepository>) {
        let identity = TrustedIdentity::implicit(
            "registry-test",
            Ulid::new("01J00000000000000000000021").unwrap(),
            Ulid::new("01J00000000000000000000022").unwrap(),
            vec![Ulid::new("01J00000000000000000000023").unwrap()],
            role,
            [READ_SCOPE.to_owned(), WRITE_SCOPE.to_owned()],
        )
        .unwrap();
        let application: Arc<dyn PlatformPort> = Arc::new(
            crate::registry::PlatformApplication::try_new(
                Arc::new(crate::session::SystemClock),
                crate::session::SessionPolicy::new(900, 60).unwrap(),
                KEY,
                Vec::new(),
                Some(identity),
                Vec::new(),
            )
            .unwrap(),
        );
        let repository = Arc::new(MemoryRepository::default());
        (
            SubjectRegistryGrpcService::new(application, repository.clone(), KEY).unwrap(),
            repository,
        )
    }

    fn id(value: &str) -> pb::Ulid {
        pb::Ulid {
            value: value.to_owned(),
        }
    }

    fn version_ref(value: &str, version: u64) -> pb::VersionRef {
        pb::VersionRef {
            id: Some(id(value)),
            version,
        }
    }

    fn request_owner() -> pb::OwnerRef {
        pb::OwnerRef {
            tenant_id: Some(id("01J00000000000000000000022")),
            owner_id: Some(id("01J00000000000000000000023")),
        }
    }

    fn change(reason: &str) -> pb::ChangeJustification {
        pb::ChangeJustification {
            reason: reason.to_owned(),
            sources: vec![pb::SourceDocumentRef {
                uri: "fixture://subject-registry".to_owned(),
                sha256: Some(pb::Sha256 { value: vec![7; 32] }),
            }],
        }
    }

    fn subject_state_request(
        subject_id: &str,
        timestamp: Timestamp,
    ) -> pb::RegisterSubjectStateRequest {
        let decimal = pb::DecimalValue {
            coefficient: "1000".to_owned(),
            scale: 0,
            unit: Some(pb::UnitRef {
                unit_id: Some(id("01J00000000000000000000012")),
                version: 1,
            }),
        };
        pb::RegisterSubjectStateRequest {
            snapshot: Some(pb::SubjectStateSnapshot {
                snapshot_id: Some(id("01J00000000000000000000013")),
                subject_ref: Some(version_ref(subject_id, 1)),
                net_capital: Some(decimal.clone()),
                limit_ceilings: vec![pb::LimitCeiling {
                    limit_code: "credit".to_owned(),
                    ceiling: Some(decimal),
                }],
                observed_at: Some(timestamp),
                visible_at: Some(timestamp),
                market_timezone: "Asia/Shanghai".to_owned(),
                owner: Some(request_owner()),
            }),
            change: Some(change("register test subject state")),
            idempotency_key: "subject-state-test-v1".to_owned(),
        }
    }

    #[tokio::test]
    async fn researcher_cannot_mutate_subjects_even_with_the_write_scope() {
        let (service, repository) = service_with_role(PlatformRole::Researcher);
        let response = service
            .register_subject(Request::new(pb::RegisterSubjectRequest::default()))
            .await
            .unwrap()
            .into_inner();
        let Some(pb::register_subject_response::Result::Error(error)) = response.result else {
            panic!("researcher mutation must fail closed");
        };
        assert_eq!(error.code, pb::ErrorCode::Forbidden as i32);
        assert!(repository.subjects.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn registry_round_trip_preserves_subject_and_state_fields() {
        let (service, repository) = service_with_role(PlatformRole::PlatformAdmin);
        let subject_id = "01J00000000000000000000011";
        let subject_ref = version_ref(subject_id, 1);
        let register = service
            .register_subject(Request::new(pb::RegisterSubjectRequest {
                subject: Some(pb::Subject {
                    subject_id: Some(id(subject_id)),
                    display_name: "Registry test".to_owned(),
                    owner: Some(request_owner()),
                }),
                subject_version: Some(pb::SubjectVersion {
                    subject_ref: Some(subject_ref.clone()),
                    access_set: Some(pb::AccessSet {
                        market_codes: vec!["market-a".to_owned()],
                        tool_codes: vec!["tool-a".to_owned()],
                    }),
                    funding_tier: pb::FundingTier::DrAvailable as i32,
                    tax_treatment: Some(pb::TaxTreatment {
                        value_added_tax_profile: "vat".to_owned(),
                        income_tax_profile: "income".to_owned(),
                    }),
                    assessment_mechanism: "assessment".to_owned(),
                    liability_profile: "liability".to_owned(),
                    constraint_set_ref: None,
                }),
                change: Some(change("register test subject")),
                idempotency_key: "subject-test-v1".to_owned(),
            }))
            .await
            .unwrap()
            .into_inner();
        let pb::register_subject_response::Result::Subject(record) = register.result.unwrap()
        else {
            panic!("subject registration must succeed")
        };
        assert_eq!(record.subject.unwrap().display_name, "Registry test");

        let fetched = service
            .get_subject(Request::new(pb::GetSubjectRequest {
                subject_ref: Some(subject_ref),
            }))
            .await
            .unwrap()
            .into_inner();
        let pb::get_subject_response::Result::Subject(record) = fetched.result.unwrap() else {
            panic!("subject lookup must succeed")
        };
        assert_eq!(record.subject.unwrap().display_name, "Registry test");

        let timestamp = Timestamp {
            seconds: 1_783_152_000,
            nanos: 0,
        };
        let state = service
            .register_subject_state(Request::new(subject_state_request(subject_id, timestamp)))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            state.result,
            Some(pb::register_subject_state_response::Result::Snapshot(_))
        ));
        let hidden = service
            .get_subject_state(Request::new(pb::GetSubjectStateRequest {
                snapshot_id: Some(id("01J00000000000000000000013")),
                knowledge_at: Some(Timestamp {
                    seconds: timestamp.seconds - 1,
                    nanos: 0,
                }),
            }))
            .await
            .unwrap()
            .into_inner();
        let Some(pb::get_subject_state_response::Result::Error(error)) = hidden.result else {
            panic!("a not-yet-visible state must be hidden")
        };
        assert_eq!(error.code, pb::ErrorCode::NotFound as i32);
        assert_eq!(repository.governed_writes.load(Ordering::SeqCst), 2);
        assert_eq!(repository.legacy_writes.load(Ordering::SeqCst), 0);
    }
}
