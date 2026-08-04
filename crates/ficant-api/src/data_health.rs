use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use ficant_application::ports::{
    AccessScope, BlobStore, CanonicalSnapshotDecoder, DataHealthThresholdProfileRepository,
    DataSourceRepository, IdempotencyKey, IntegrityEventSink, PositionSnapshotRepository,
    SnapshotRepository, SnapshotVerifiedReadMetadataRepository, VerifiedBlobReader,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, DataHealthQuery, DataHealthThresholdProfilePayload,
    GetDataHealthReport, PublishDataHealthThresholdProfile, map_domain_error,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as market;
use ficant_contracts::ficant::research::v1 as pb;
use ficant_contracts::ficant::research::v1::data_health_service_server::DataHealthService;
use ficant_domain::market::PriceSourceType;
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_domain::research::{
    CoverageDeclaration, DataHealthIssue, DataHealthIssueCode, DataHealthReport, DataHealthState,
    DataHealthThresholdProfile, DataHealthThresholdProfileInput, PositionSetState,
    PriceSourceSummary,
};
use ficant_domain::{ContentAddressed, Lineaged};
use prost_types::Timestamp;
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::error::{PlatformFailure, PlatformFailureCode};
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;

const READ_SCOPE: &str = "data-health:read";
const CONFIGURE_SCOPE: &str = "data-health:configure";

#[derive(Clone)]
pub struct DataHealthGrpcService {
    identity: Arc<dyn PlatformPort>,
    access_scope: AccessScope,
    positions: Arc<dyn PositionSnapshotRepository>,
    snapshot_metadata: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
    blob_reader: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
    decoder: Arc<dyn CanonicalSnapshotDecoder>,
    data_sources: Arc<dyn DataSourceRepository>,
    threshold_profiles: Arc<dyn DataHealthThresholdProfileRepository>,
    snapshots: Arc<dyn SnapshotRepository>,
    blobs: Arc<dyn BlobStore>,
    errors: CoreBusinessErrorMapper,
}

impl DataHealthGrpcService {
    /// Composes the read-only health adapter from the exact production ports.
    ///
    /// # Errors
    ///
    /// Returns an error when the trace-key material cannot initialize the stable business-error
    /// mapper.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        access_scope: AccessScope,
        positions: Arc<dyn PositionSnapshotRepository>,
        snapshot_metadata: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
        blob_reader: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
        decoder: Arc<dyn CanonicalSnapshotDecoder>,
        data_sources: Arc<dyn DataSourceRepository>,
        threshold_profiles: Arc<dyn DataHealthThresholdProfileRepository>,
        snapshots: Arc<dyn SnapshotRepository>,
        blobs: Arc<dyn BlobStore>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            access_scope,
            positions,
            snapshot_metadata,
            blob_reader,
            integrity_events,
            decoder,
            data_sources,
            threshold_profiles,
            snapshots,
            blobs,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn authorize(
        &self,
        request: &Request<impl Sized>,
        required_scope: &str,
    ) -> Result<(), ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|failure| platform_application_error(&failure))?;
        if session.has_scope(required_scope) {
            Ok(())
        } else {
            Err(forbidden())
        }
    }

    fn error(&self, operation: &str, error: &ApplicationError) -> core::ErrorDetail {
        self.errors.map(operation, "data-health-application", error)
    }
}

#[tonic::async_trait]
impl DataHealthService for DataHealthGrpcService {
    async fn publish_data_health_threshold_profile(
        &self,
        request: Request<pb::PublishDataHealthThresholdProfileRequest>,
    ) -> Result<Response<pb::PublishDataHealthThresholdProfileResponse>, Status> {
        const OPERATION: &str = "data-health.configure";
        let result = match self.authorize(&request, CONFIGURE_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => match parse_threshold_profile(request.get_ref().threshold_profile.as_ref())
                .and_then(|profile| {
                    let key = IdempotencyKey::new(request.get_ref().idempotency_key.clone())?;
                    DataHealthThresholdProfilePayload::new(profile, key)
                }) {
                Ok(payload) if self.access_scope.allows(payload.profile().owner()) => {
                    PublishDataHealthThresholdProfile::new(
                        self.blobs.as_ref(),
                        self.snapshots.as_ref(),
                    )
                    .execute(&self.access_scope, payload)
                    .await
                }
                Ok(_) => Err(forbidden()),
                Err(error) => Err(error),
            },
        };
        Ok(Response::new(
            pb::PublishDataHealthThresholdProfileResponse {
                result: Some(match result {
                    Ok(value) => {
                        pb::publish_data_health_threshold_profile_response::Result::ThresholdProfile(
                            threshold_profile(&value),
                        )
                    }
                    Err(error) => {
                        pb::publish_data_health_threshold_profile_response::Result::Error(
                            self.error(OPERATION, &error),
                        )
                    }
                }),
            },
        ))
    }

    async fn get_data_health_report(
        &self,
        request: Request<pb::GetDataHealthReportRequest>,
    ) -> Result<Response<pb::GetDataHealthReportResponse>, Status> {
        const OPERATION: &str = "data-health.get";
        let result = match self.authorize(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => parse_query(request.get_ref()),
        };
        let result = match result {
            Ok(query) => {
                GetDataHealthReport::new(
                    self.positions.as_ref(),
                    self.snapshot_metadata.as_ref(),
                    self.blob_reader.as_ref(),
                    self.integrity_events.as_ref(),
                    self.decoder.as_ref(),
                    self.data_sources.as_ref(),
                    self.threshold_profiles.as_ref(),
                )
                .execute(&self.access_scope, query)
                .await
            }
            Err(error) => Err(error),
        };
        Ok(Response::new(pb::GetDataHealthReportResponse {
            result: Some(match result {
                Ok(value) => pb::get_data_health_report_response::Result::Report(report(&value)),
                Err(error) => pb::get_data_health_report_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }
}

fn parse_query(
    value: &pb::GetDataHealthReportRequest,
) -> Result<DataHealthQuery, ApplicationError> {
    let data_snapshot_id = value
        .data_snapshot_id
        .as_ref()
        .map(|value| parse_ulid(Some(value)))
        .transpose()?;
    Ok(DataHealthQuery::new(
        parse_version_ref(value.subject_ref.as_ref())?,
        parse_ulid(value.position_snapshot_id.as_ref())?,
        data_snapshot_id,
        parse_market_time(value.evaluated_at.as_ref())?,
    ))
}

fn parse_threshold_profile(
    value: Option<&pb::DataHealthThresholdProfile>,
) -> Result<DataHealthThresholdProfile, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    DataHealthThresholdProfile::new(DataHealthThresholdProfileInput {
        profile_snapshot_id: parse_ulid(value.profile_snapshot_id.as_ref())?,
        owner: parse_owner(value.owner.as_ref())?,
        profile_ref: parse_version_ref(value.profile_ref.as_ref())?,
        visible_at: parse_market_time(value.visible_at.as_ref())?,
        effective_from: parse_market_time(value.effective_from.as_ref())?,
        effective_to: parse_market_time(value.effective_to.as_ref())?,
        max_position_snapshot_age_seconds: value.max_position_snapshot_age_seconds,
        unknown_accounting_warning_basis_points: value.unknown_accounting_warning_basis_points,
        max_data_snapshot_age_seconds: value.max_data_snapshot_age_seconds,
        model_valuation_warning_basis_points: value.model_valuation_warning_basis_points,
        content_hash: parse_hash(value.content_hash.as_ref())?,
        lineage: value
            .lineage
            .iter()
            .map(parse_lineage)
            .collect::<Result<_, _>>()?,
    })
    .map_err(map_domain_error)
}

fn parse_owner(value: Option<&core::OwnerRef>) -> Result<OwnerRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(OwnerRef::new(
        parse_ulid(value.tenant_id.as_ref())?,
        parse_ulid(value.owner_id.as_ref())?,
    ))
}

fn parse_lineage(value: &core::LineageRef) -> Result<LineageRef, ApplicationError> {
    let object_id = parse_ulid(value.object_id.as_ref())?;
    match (value.version, value.content_hash.as_ref()) {
        (0, Some(hash)) => Ok(LineageRef::content_addressed(
            object_id,
            parse_hash(Some(hash))?,
        )),
        (version, None) if version > 0 => Ok(LineageRef::versioned(
            object_id,
            Version::new(version).map_err(map_domain_error)?,
        )),
        _ => Err(invalid()),
    }
}

fn parse_version_ref(value: Option<&core::VersionRef>) -> Result<VersionRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(VersionRef::new(
        parse_ulid(value.id.as_ref())?,
        Version::new(value.version).map_err(map_domain_error)?,
    ))
}

fn parse_ulid(value: Option<&core::Ulid>) -> Result<Ulid, ApplicationError> {
    Ulid::new(value.ok_or_else(invalid)?.value.clone()).map_err(map_domain_error)
}

fn parse_hash(value: Option<&core::Sha256>) -> Result<ContentHash, ApplicationError> {
    ContentHash::from_bytes(&value.ok_or_else(invalid)?.value).map_err(map_domain_error)
}

fn parse_market_time(value: Option<&core::MarketTime>) -> Result<MarketTime, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let instant = value.instant.as_ref().ok_or_else(invalid)?;
    let nanos = u32::try_from(instant.nanos).map_err(|_| invalid())?;
    let instant = DateTime::<Utc>::from_timestamp(instant.seconds, nanos).ok_or_else(invalid)?;
    let date =
        NaiveDate::parse_from_str(&value.local_trading_date, "%Y-%m-%d").map_err(|_| invalid())?;
    if date.to_string() != value.local_trading_date {
        return Err(invalid());
    }
    MarketTime::new(instant, value.market_timezone.clone(), date).map_err(map_domain_error)
}

fn report(value: &DataHealthReport) -> pb::DataHealthReport {
    pb::DataHealthReport {
        owner: Some(owner(value.owner())),
        subject_ref: Some(version_ref(value.subject_ref())),
        evaluated_at: Some(market_time(value.evaluated_at())),
        position_snapshot_id: Some(ulid(value.position_snapshot_id())),
        position_snapshot_hash: Some(hash(value.position_snapshot_hash())),
        data_snapshot_id: value.data_snapshot_id().map(ulid),
        data_snapshot_manifest_hash: value.data_snapshot_manifest_hash().map(hash),
        data_source_ref: value.data_source_ref().map(version_ref),
        threshold_profile: Some(threshold_profile(value.threshold_profile())),
        state: match value.state() {
            DataHealthState::Healthy => pb::DataHealthState::Healthy,
            DataHealthState::Warning => pb::DataHealthState::Warning,
        } as i32,
        issues: value.issues().iter().map(issue).collect(),
        price_evidence_evaluated: value.price_evidence_evaluated(),
        position_set_state: match value.position_set_state() {
            PositionSetState::NonEmpty => pb::PositionSetState::NonEmpty,
            PositionSetState::VerifiedEmpty => pb::PositionSetState::VerifiedEmpty,
        } as i32,
        coverage: Some(coverage(value.coverage())),
        request_fingerprint: Some(hash(value.request_fingerprint())),
        content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
    }
}

fn threshold_profile(value: &DataHealthThresholdProfile) -> pb::DataHealthThresholdProfile {
    pb::DataHealthThresholdProfile {
        profile_snapshot_id: Some(ulid(value.id())),
        owner: Some(owner(value.owner())),
        profile_ref: Some(version_ref(value.profile_ref())),
        visible_at: Some(market_time(value.visible_at())),
        effective_from: Some(market_time(value.effective_from())),
        effective_to: Some(market_time(value.effective_to())),
        max_position_snapshot_age_seconds: value.max_position_snapshot_age_seconds(),
        unknown_accounting_warning_basis_points: value.unknown_accounting_warning_basis_points(),
        max_data_snapshot_age_seconds: value.max_data_snapshot_age_seconds(),
        model_valuation_warning_basis_points: value.model_valuation_warning_basis_points(),
        content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
    }
}

fn issue(value: &DataHealthIssue) -> pb::DataHealthIssue {
    pb::DataHealthIssue {
        code: match value.code() {
            DataHealthIssueCode::EmptyPositions => pb::DataHealthIssueCode::EmptyPositions,
            DataHealthIssueCode::UnknownAccountingClassification => {
                pb::DataHealthIssueCode::UnknownAccountingClassification
            }
            DataHealthIssueCode::StalePositionSnapshot => {
                pb::DataHealthIssueCode::StalePositionSnapshot
            }
            DataHealthIssueCode::UntypedPriceSource => pb::DataHealthIssueCode::UntypedPriceSource,
            DataHealthIssueCode::ModelValuationShare => {
                pb::DataHealthIssueCode::ModelValuationShare
            }
            DataHealthIssueCode::StaleDataSnapshot => pb::DataHealthIssueCode::StaleDataSnapshot,
        } as i32,
        affected_position_ids: value.affected_position_ids().iter().map(ulid).collect(),
        data_source_ref: value.data_source_ref().map(version_ref),
        record_count: value.record_count(),
        ratio_basis_points: value.ratio_basis_points(),
        observed_age_seconds: value.observed_age_seconds(),
    }
}

fn coverage(value: &CoverageDeclaration) -> pb::CoverageDeclaration {
    pb::CoverageDeclaration {
        imported_position_count: value.imported_position_count(),
        participating_position_count: value.participating_position_count(),
        imported_gross_economic_value_by_unit: value
            .imported_gross_economic_value_by_unit()
            .iter()
            .map(decimal)
            .collect(),
        participating_gross_economic_value_by_unit: value
            .participating_gross_economic_value_by_unit()
            .iter()
            .map(decimal)
            .collect(),
        missing_critical_field_record_count: value.missing_critical_field_record_count(),
        source_confidence: value.source_confidence().map(source_confidence),
        distinct_external_data_source_version_count: value
            .distinct_external_data_source_version_count(),
    }
}

fn source_confidence(value: &PriceSourceSummary) -> pb::PriceSourceSummary {
    pb::PriceSourceSummary {
        counts: value
            .counts()
            .iter()
            .map(|count| pb::PriceSourceCount {
                source_type: price_source_type(count.source_type()) as i32,
                record_count: count.record_count(),
            })
            .collect(),
        mixed: value.mixed(),
    }
}

const fn price_source_type(value: PriceSourceType) -> market::PriceSourceType {
    match value {
        PriceSourceType::RealTrade => market::PriceSourceType::RealTrade,
        PriceSourceType::ActiveQuote => market::PriceSourceType::ActiveQuote,
        PriceSourceType::ModelValuation => market::PriceSourceType::ModelValuation,
        PriceSourceType::CurveInterpolation => market::PriceSourceType::CurveInterpolation,
    }
}

fn ulid(value: &Ulid) -> core::Ulid {
    core::Ulid {
        value: value.as_str().to_owned(),
    }
}

fn hash(value: &ContentHash) -> core::Sha256 {
    core::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn owner(value: &OwnerRef) -> core::OwnerRef {
    core::OwnerRef {
        tenant_id: Some(ulid(value.tenant_id())),
        owner_id: Some(ulid(value.owner_id())),
    }
}

fn version_ref(value: &VersionRef) -> core::VersionRef {
    core::VersionRef {
        id: Some(ulid(value.id())),
        version: value.version().get(),
    }
}

fn decimal(value: &DecimalValue) -> core::DecimalValue {
    core::DecimalValue {
        coefficient: value.coefficient().to_owned(),
        scale: value.scale(),
        unit: Some(core::UnitRef {
            unit_id: Some(ulid(value.unit().unit_id())),
            version: value.unit().version().get(),
        }),
    }
}

fn lineage(value: &LineageRef) -> core::LineageRef {
    core::LineageRef {
        object_id: Some(ulid(value.object_id())),
        version: value.version().map_or(0, Version::get),
        content_hash: value.content_hash().map(hash),
    }
}

fn market_time(value: &MarketTime) -> core::MarketTime {
    core::MarketTime {
        instant: Some(Timestamp {
            seconds: value.instant().timestamp(),
            nanos: value.instant().timestamp_subsec_nanos().cast_signed(),
        }),
        market_timezone: value.market_timezone().to_owned(),
        local_trading_date: value.local_trading_date().to_string(),
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
