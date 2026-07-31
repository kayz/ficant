use crate::core_error::CoreBusinessErrorMapper;
use crate::error::{PlatformFailure, PlatformFailureCode};
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;
use chrono::{DateTime, NaiveDate, Utc};
use ficant_application::ports::{
    AccessScope, BlobStore, IdempotencyKey, PositionSnapshotRepository, SnapshotRepository,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, PositionSnapshotPayload, PositionViewsUseCase,
    PublishPositionSnapshot, map_domain_error,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::research::v1 as pb;
use ficant_contracts::ficant::research::v1::position_snapshot_service_server::PositionSnapshotService;
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    AccountingBook, AccountingClassification, AccountingClassificationState, Position,
    PositionHoldingForm, PositionInput, PositionSnapshot, PositionSnapshotInput,
};
use ficant_domain::{ContentAddressed, Lineaged};
use prost_types::Timestamp;
use std::sync::Arc;
use tonic::{Request, Response, Status};

const READ_SCOPE: &str = "positions:read";
const WRITE_SCOPE: &str = "positions:write";

#[derive(Clone)]
pub struct PositionSnapshotGrpcService {
    identity: Arc<dyn PlatformPort>,
    access_scope: AccessScope,
    positions: Arc<dyn PositionSnapshotRepository>,
    snapshots: Arc<dyn SnapshotRepository>,
    blobs: Arc<dyn BlobStore>,
    errors: CoreBusinessErrorMapper,
}

impl PositionSnapshotGrpcService {
    /// Creates the authenticated position-snapshot transport adapter for one trusted owner scope.
    ///
    /// # Errors
    ///
    /// Returns an error when the trace-signing key is shorter than the frozen minimum.
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        access_scope: AccessScope,
        positions: Arc<dyn PositionSnapshotRepository>,
        snapshots: Arc<dyn SnapshotRepository>,
        blobs: Arc<dyn BlobStore>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            access_scope,
            positions,
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
        self.errors
            .map(operation, "position-snapshot-application", error)
    }
}

#[tonic::async_trait]
impl PositionSnapshotService for PositionSnapshotGrpcService {
    async fn publish_position_snapshot(
        &self,
        request: Request<pb::PublishPositionSnapshotRequest>,
    ) -> Result<Response<pb::PublishPositionSnapshotResponse>, Status> {
        const OPERATION: &str = "positions.publish";
        let result = match self.authorize(&request, WRITE_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => match (|| {
                let snapshot = parse_snapshot(request.get_ref().snapshot.as_ref())?;
                let key = IdempotencyKey::new(request.get_ref().idempotency_key.clone())?;
                let payload = PositionSnapshotPayload::new(snapshot, key)?;
                if self.access_scope.allows(payload.snapshot().owner()) {
                    Ok(payload)
                } else {
                    Err(forbidden())
                }
            })() {
                Ok(payload) => {
                    PublishPositionSnapshot::new(self.blobs.as_ref(), self.snapshots.as_ref())
                        .execute(&self.access_scope, payload)
                        .await
                }
                Err(error) => Err(error),
            },
        };
        Ok(Response::new(pb::PublishPositionSnapshotResponse {
            result: Some(match result {
                Ok(value) => {
                    pb::publish_position_snapshot_response::Result::Snapshot(snapshot(&value))
                }
                Err(error) => pb::publish_position_snapshot_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn get_position_snapshot(
        &self,
        request: Request<pb::GetPositionSnapshotRequest>,
    ) -> Result<Response<pb::GetPositionSnapshotResponse>, Status> {
        const OPERATION: &str = "positions.get";
        let result = match self.authorize(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => match (
                parse_ulid(request.get_ref().snapshot_id.as_ref()),
                parse_market_time(request.get_ref().knowledge_at.as_ref()),
            ) {
                (Ok(snapshot_id), Ok(knowledge_at)) => self
                    .positions
                    .get_position_snapshot(&self.access_scope, snapshot_id, knowledge_at)
                    .await
                    .and_then(|value| value.ok_or_else(not_found)),
                (Err(error), _) | (_, Err(error)) => Err(error),
            },
        };
        Ok(Response::new(pb::GetPositionSnapshotResponse {
            result: Some(match result {
                Ok(value) => pb::get_position_snapshot_response::Result::Snapshot(snapshot(&value)),
                Err(error) => {
                    pb::get_position_snapshot_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn resolve_position_snapshot(
        &self,
        request: Request<pb::ResolvePositionSnapshotRequest>,
    ) -> Result<Response<pb::ResolvePositionSnapshotResponse>, Status> {
        const OPERATION: &str = "positions.resolve";
        let result = match self.authorize(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => match (
                parse_version_ref(request.get_ref().subject_ref.as_ref()),
                parse_market_time(request.get_ref().observed_at.as_ref()),
                parse_market_time(request.get_ref().knowledge_at.as_ref()),
            ) {
                (Ok(subject_ref), Ok(observed_at), Ok(knowledge_at)) => {
                    PositionViewsUseCase::new(self.positions.as_ref())
                        .resolve(&self.access_scope, subject_ref, observed_at, knowledge_at)
                        .await
                }
                (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
            },
        };
        Ok(Response::new(pb::ResolvePositionSnapshotResponse {
            result: Some(match result {
                Ok(value) => {
                    pb::resolve_position_snapshot_response::Result::Snapshot(snapshot(&value))
                }
                Err(error) => pb::resolve_position_snapshot_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn get_position_views(
        &self,
        request: Request<pb::GetPositionViewsRequest>,
    ) -> Result<Response<pb::GetPositionViewsResponse>, Status> {
        const OPERATION: &str = "positions.views";
        let result = match self.authorize(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => match (
                parse_ulid(request.get_ref().snapshot_id.as_ref()),
                parse_market_time(request.get_ref().knowledge_at.as_ref()),
            ) {
                (Ok(snapshot_id), Ok(knowledge_at)) => {
                    PositionViewsUseCase::new(self.positions.as_ref())
                        .views(&self.access_scope, snapshot_id, knowledge_at)
                        .await
                }
                (Err(error), _) | (_, Err(error)) => Err(error),
            },
        };
        Ok(Response::new(pb::GetPositionViewsResponse {
            result: Some(match result {
                Ok(value) => pb::get_position_views_response::Result::Views(views(&value)),
                Err(error) => {
                    pb::get_position_views_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn calculate_capital_use(
        &self,
        request: Request<pb::CalculateCapitalUseRequest>,
    ) -> Result<Response<pb::CalculateCapitalUseResponse>, Status> {
        const OPERATION: &str = "positions.capital-use";
        let result = match self.authorize(&request, READ_SCOPE) {
            Err(error) => Err(error),
            Ok(()) => match (
                parse_ulid(request.get_ref().snapshot_id.as_ref()),
                parse_market_time(request.get_ref().knowledge_at.as_ref()),
            ) {
                (Ok(snapshot_id), Ok(knowledge_at)) => {
                    PositionViewsUseCase::new(self.positions.as_ref())
                        .capital_use(&self.access_scope, snapshot_id, knowledge_at)
                        .await
                }
                (Err(error), _) | (_, Err(error)) => Err(error),
            },
        };
        Ok(Response::new(pb::CalculateCapitalUseResponse {
            result: Some(match result {
                Ok(value) => {
                    pb::calculate_capital_use_response::Result::CapitalUse(capital_use(&value))
                }
                Err(error) => {
                    pb::calculate_capital_use_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }
}

fn parse_snapshot(
    value: Option<&pb::PositionSnapshot>,
) -> Result<PositionSnapshot, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let positions = value
        .positions
        .iter()
        .map(parse_position)
        .collect::<Result<Vec<_>, _>>()?;
    let lineage = value
        .lineage
        .iter()
        .map(parse_lineage)
        .collect::<Result<Vec<_>, _>>()?;
    let input = PositionSnapshotInput {
        snapshot_id: parse_ulid(value.snapshot_id.as_ref())?,
        owner: parse_owner(value.owner.as_ref())?,
        subject_ref: parse_version_ref(value.subject_ref.as_ref())?,
        observed_at: parse_market_time(value.observed_at.as_ref())?,
        visible_at: parse_market_time(value.visible_at.as_ref())?,
        content_hash: parse_hash(value.content_hash.as_ref())?,
        lineage,
        positions,
    };
    PositionSnapshot::new(input).map_err(map_domain_error)
}

fn parse_position(value: &pb::Position) -> Result<Position, ApplicationError> {
    let classification = value
        .accounting_classification
        .as_ref()
        .ok_or_else(invalid)?;
    let state = match pb::AccountingClassificationState::try_from(classification.state)
        .map_err(|_| invalid())?
    {
        pb::AccountingClassificationState::Classified => AccountingClassificationState::Classified,
        pb::AccountingClassificationState::NotApplicable => {
            AccountingClassificationState::NotApplicable
        }
        pb::AccountingClassificationState::Unknown => AccountingClassificationState::Unknown,
        pb::AccountingClassificationState::Unspecified => return Err(invalid()),
    };
    let book = match pb::AccountingBook::try_from(classification.book).map_err(|_| invalid())? {
        pb::AccountingBook::Ac => Some(AccountingBook::Ac),
        pb::AccountingBook::Fvoci => Some(AccountingBook::Fvoci),
        pb::AccountingBook::Fvtpl => Some(AccountingBook::Fvtpl),
        pb::AccountingBook::Unspecified => None,
    };
    let holding_form =
        match pb::PositionHoldingForm::try_from(value.holding_form).map_err(|_| invalid())? {
            pb::PositionHoldingForm::Owned => PositionHoldingForm::Owned,
            pb::PositionHoldingForm::RepoSold => PositionHoldingForm::RepoSold,
            pb::PositionHoldingForm::ReverseRepoCollateral => {
                PositionHoldingForm::ReverseRepoCollateral
            }
            pb::PositionHoldingForm::Unspecified => return Err(invalid()),
        };
    Position::new(PositionInput {
        position_id: parse_ulid(value.position_id.as_ref())?,
        instrument_ref: parse_version_ref(value.instrument_ref.as_ref())?,
        quantity: parse_decimal(value.quantity.as_ref())?,
        economic_value: parse_decimal(value.economic_value.as_ref())?,
        economic_pnl: parse_decimal(value.economic_pnl.as_ref())?,
        accounting_pnl: parse_decimal(value.accounting_pnl.as_ref())?,
        capital_requirement: parse_decimal(value.capital_requirement.as_ref())?,
        accounting_classification: AccountingClassification::new(state, book)
            .map_err(map_domain_error)?,
        holding_form,
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
    let version = if value.version == 0 {
        None
    } else {
        Some(Version::new(value.version).map_err(map_domain_error)?)
    };
    let hash = value
        .content_hash
        .as_ref()
        .map(|hash| parse_hash(Some(hash)))
        .transpose()?;
    LineageRef::new(parse_ulid(value.object_id.as_ref())?, version, hash).map_err(map_domain_error)
}

fn parse_decimal(value: Option<&core::DecimalValue>) -> Result<DecimalValue, ApplicationError> {
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

fn snapshot(value: &PositionSnapshot) -> pb::PositionSnapshot {
    pb::PositionSnapshot {
        snapshot_id: Some(ulid(value.id())),
        owner: Some(owner(value.owner())),
        subject_ref: Some(version_ref(value.subject_ref())),
        observed_at: Some(market_time(value.observed_at())),
        visible_at: Some(market_time(value.visible_at())),
        content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
        positions: value.positions().iter().map(position).collect(),
    }
}

fn position(value: &Position) -> pb::Position {
    let (state, book) = match value.accounting_classification().state() {
        AccountingClassificationState::Classified => (
            pb::AccountingClassificationState::Classified,
            match value.accounting_classification().book() {
                Some(AccountingBook::Ac) => pb::AccountingBook::Ac,
                Some(AccountingBook::Fvoci) => pb::AccountingBook::Fvoci,
                Some(AccountingBook::Fvtpl) => pb::AccountingBook::Fvtpl,
                None => unreachable!("domain classification invariant"),
            },
        ),
        AccountingClassificationState::NotApplicable => (
            pb::AccountingClassificationState::NotApplicable,
            pb::AccountingBook::Unspecified,
        ),
        AccountingClassificationState::Unknown => (
            pb::AccountingClassificationState::Unknown,
            pb::AccountingBook::Unspecified,
        ),
    };
    pb::Position {
        position_id: Some(ulid(value.id())),
        instrument_ref: Some(version_ref(value.instrument_ref())),
        quantity: Some(decimal(value.quantity())),
        economic_value: Some(decimal(value.economic_value())),
        economic_pnl: Some(decimal(value.economic_pnl())),
        accounting_pnl: Some(decimal(value.accounting_pnl())),
        capital_requirement: Some(decimal(value.capital_requirement())),
        accounting_classification: Some(pb::AccountingClassification {
            state: state as i32,
            book: book as i32,
        }),
        holding_form: match value.holding_form() {
            PositionHoldingForm::Owned => pb::PositionHoldingForm::Owned,
            PositionHoldingForm::RepoSold => pb::PositionHoldingForm::RepoSold,
            PositionHoldingForm::ReverseRepoCollateral => {
                pb::PositionHoldingForm::ReverseRepoCollateral
            }
        } as i32,
    }
}

fn views(value: &ficant_application::PositionViews) -> pb::PositionViews {
    pb::PositionViews {
        snapshot_id: Some(ulid(value.snapshot.id())),
        content_hash: Some(hash(value.snapshot.content_hash())),
        lineage: value.snapshot.lineage().iter().map(lineage).collect(),
        positions: value
            .positions
            .iter()
            .map(|position| pb::PositionView {
                position_id: Some(ulid(&position.position_id)),
                economic_value: Some(decimal(&position.economic_value)),
                economic_pnl: Some(decimal(&position.economic_pnl)),
                accounting_pnl: Some(decimal(&position.accounting_pnl)),
                included_in_position_exposure: position.included_in_position_exposure,
                included_in_available_liquidity: position.included_in_available_liquidity,
                collateral_fact: position.collateral_fact,
            })
            .collect(),
    }
}

fn capital_use(value: &ficant_application::CapitalUse) -> pb::CapitalUse {
    pb::CapitalUse {
        snapshot_id: Some(ulid(value.snapshot.id())),
        content_hash: Some(hash(value.snapshot.content_hash())),
        lineage: value.snapshot.lineage().iter().map(lineage).collect(),
        total_capital_requirement: Some(decimal(&value.total_capital_requirement)),
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
fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_parser_rejects_proto_unspecified_values_and_keeps_explicit_unknown() {
        let mut value = proto_position();
        value.holding_form = pb::PositionHoldingForm::Unspecified as i32;
        assert_eq!(
            parse_position(&value).unwrap_err().category(),
            ApplicationErrorCategory::ValidationFailed
        );

        let mut value = proto_position();
        value.accounting_classification.as_mut().unwrap().state =
            pb::AccountingClassificationState::Unspecified as i32;
        assert_eq!(
            parse_position(&value).unwrap_err().category(),
            ApplicationErrorCategory::ValidationFailed
        );

        let mut value = proto_position();
        value.accounting_classification.as_mut().unwrap().state =
            pb::AccountingClassificationState::Unknown as i32;
        value.accounting_classification.as_mut().unwrap().book =
            pb::AccountingBook::Unspecified as i32;
        assert_eq!(
            parse_position(&value)
                .unwrap()
                .accounting_classification()
                .state(),
            AccountingClassificationState::Unknown
        );
    }

    #[test]
    fn position_parser_requires_a_book_only_for_classified_positions() {
        let mut classified = proto_position();
        classified.accounting_classification.as_mut().unwrap().book =
            pb::AccountingBook::Unspecified as i32;
        assert_eq!(
            parse_position(&classified).unwrap_err().category(),
            ApplicationErrorCategory::ValidationFailed
        );

        let mut not_applicable = proto_position();
        not_applicable
            .accounting_classification
            .as_mut()
            .unwrap()
            .state = pb::AccountingClassificationState::NotApplicable as i32;
        not_applicable
            .accounting_classification
            .as_mut()
            .unwrap()
            .book = pb::AccountingBook::Unspecified as i32;
        assert_eq!(
            parse_position(&not_applicable)
                .unwrap()
                .accounting_classification()
                .state(),
            AccountingClassificationState::NotApplicable
        );
    }

    fn proto_position() -> pb::Position {
        pb::Position {
            position_id: Some(id("01ARZ3NDEKTSV4RRFFQ69G5FAV")),
            instrument_ref: Some(core::VersionRef {
                id: Some(id("01ARZ3NDEKTSV4RRFFQ69G5FAW")),
                version: 1,
            }),
            quantity: Some(decimal("1")),
            economic_value: Some(decimal("100")),
            economic_pnl: Some(decimal("0")),
            accounting_pnl: Some(decimal("0")),
            capital_requirement: Some(decimal("10")),
            accounting_classification: Some(pb::AccountingClassification {
                state: pb::AccountingClassificationState::Classified as i32,
                book: pb::AccountingBook::Ac as i32,
            }),
            holding_form: pb::PositionHoldingForm::Owned as i32,
        }
    }

    fn decimal(coefficient: &str) -> core::DecimalValue {
        core::DecimalValue {
            coefficient: coefficient.to_owned(),
            scale: 0,
            unit: Some(core::UnitRef {
                unit_id: Some(id("01ARZ3NDEKTSV4RRFFQ69G5FAX")),
                version: 1,
            }),
        }
    }

    fn id(value: &str) -> core::Ulid {
        core::Ulid {
            value: value.to_owned(),
        }
    }
}
