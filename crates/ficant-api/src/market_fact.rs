use std::sync::Arc;

use ficant_application::ports::{
    AeadCursorCodec, BeginBlobStage, BlobStore, CURVE_POINT_SCHEMA, Cursor, CurvePointSetDecoder,
    CurveSnapshotMetadataRepository, DefinitionRepository, DefinitionValue,
    FoundationChangeContext, GovernedAppendMarketFact, GovernedCorrectMarketFact,
    GovernedPublishCurveSnapshot, IdempotencyKey, IntegrityEventSink, MARKET_FACT_WRITE_SCOPE,
    MarketFact, MarketFactRepository, MarketFactRulePackResolver, MarketFactUnitResolver,
    MarketFactUseCase, MarketFactWindow, PageRequest, RequiredVerifiedBlobRead, SafeTraceContext,
    VerifiedBlobReader, VerifiedBlobRef, VerifiedBlobRole, VerifiedReadResourceKind,
    VerifyBlobStage,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as pb;
use ficant_contracts::ficant::market::v1::market_fact_service_server::MarketFactService;
use ficant_domain::governance::{
    FoundationResourceKind, FoundationResourceRef, PlatformRole, deterministic_change_record_id,
};
use ficant_domain::market::{
    ArtifactInputKind, Cashflow, CashflowInput, CashflowType, CurveSnapshot, CurveSnapshotInput,
    FactSource, Quote, QuoteInput, Trade, TradeInput, Valuation, ValuationInput,
    ValuationValueRole, VerificationStatus,
};
use ficant_domain::primitives::{ContentHash, LineageRef, Ulid, Version};
use ficant_domain::{ContentAddressed, Lineaged, VersionedDefinition};
use prost::Message;
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::curve_points::CanonicalCurvePointSetDecoder;
use crate::grpc_web::request_credential;
use crate::market_definition::{
    decimal, hash, market_time, owner, parse_change, parse_decimal, parse_hash, parse_market_time,
    parse_owner, parse_ulid, parse_unit_ref, parse_version_ref, server_market_time, ulid, unit_ref,
    version_ref,
};
use crate::registry::PlatformPort;

const MARKET_FACT_READ_SCOPE: &str = "facts:read";
const DEFAULT_PAGE_SIZE: u32 = 100;

/// Per-request authenticated transport for governed market facts and immutable curve fixtures.
#[derive(Clone)]
pub struct MarketFactGrpcService {
    identity: Arc<dyn PlatformPort>,
    facts: Arc<dyn MarketFactRepository>,
    definitions: Arc<dyn DefinitionRepository>,
    blobs: Arc<dyn BlobStore>,
    curve_metadata: Arc<dyn CurveSnapshotMetadataRepository>,
    blob_reader: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
    cursor_codec: Arc<AeadCursorCodec>,
    errors: CoreBusinessErrorMapper,
}

impl MarketFactGrpcService {
    /// Composes the governed Fact adapter from production-shaped ports.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the error trace-key contract is invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        facts: Arc<dyn MarketFactRepository>,
        definitions: Arc<dyn DefinitionRepository>,
        blobs: Arc<dyn BlobStore>,
        curve_metadata: Arc<dyn CurveSnapshotMetadataRepository>,
        blob_reader: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
        cursor_codec: Arc<AeadCursorCodec>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            facts,
            definitions,
            blobs,
            curve_metadata,
            blob_reader,
            integrity_events,
            cursor_codec,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn principal(
        &self,
        request: &Request<impl Sized>,
        required_scope: &str,
        required_role: Option<PlatformRole>,
    ) -> Result<ficant_application::AuthorizedPrincipal, ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|_| forbidden())?;
        let principal = session.authorized_principal()?;
        if !principal.has_scope(required_scope) {
            return Err(forbidden());
        }
        if let Some(role) = required_role {
            principal.require_role(role)?;
        }
        Ok(principal)
    }

    fn error(&self, operation: &str, error: &ApplicationError) -> core::ErrorDetail {
        self.errors.map(operation, "market-fact-application", error)
    }

    async fn fully_validate_fact(
        &self,
        scope: &ficant_application::AccessScope,
        fact: MarketFact,
    ) -> Result<ficant_application::ports::FullyValidatedMarketFact, ApplicationError> {
        let unit_validated = MarketFactUnitResolver::new(self.definitions.as_ref())
            .resolve(scope, fact)
            .await?;
        MarketFactRulePackResolver::new(self.definitions.as_ref())
            .resolve(scope, unit_validated)
            .await
    }

    async fn append_fact(
        &self,
        principal: ficant_application::AuthorizedPrincipal,
        request: &pb::AppendMarketFactRequest,
    ) -> Result<MarketFact, ApplicationError> {
        let fact = parse_fact(request.fact.as_ref())?;
        let resource = FoundationResourceRef::unversioned(
            FoundationResourceKind::MarketFact,
            fact.id().clone(),
        );
        let occurred_at = server_market_time();
        let record_id = deterministic_change_record_id(
            &occurred_at,
            principal.actor_id(),
            &resource,
            &request.idempotency_key,
        )
        .map_err(map_domain_error)?;
        let context = FoundationChangeContext::administrator(
            principal.clone(),
            parse_change(request.change.as_ref())?,
            record_id,
            occurred_at,
        )?;
        let validated = self
            .fully_validate_fact(principal.access_scope(), fact)
            .await?;
        let command = GovernedAppendMarketFact::new(
            context,
            validated,
            IdempotencyKey::new(request.idempotency_key.clone())?,
        )?;
        MarketFactUseCase::new(self.facts.as_ref())
            .append_governed(command)
            .await
    }

    async fn correct_fact(
        &self,
        principal: ficant_application::AuthorizedPrincipal,
        request: &pb::CorrectMarketFactRequest,
    ) -> Result<MarketFact, ApplicationError> {
        let original_id = parse_ulid(request.original_fact_id.as_ref())?;
        let fact = parse_fact(request.fact.as_ref())?;
        let resource = FoundationResourceRef::unversioned(
            FoundationResourceKind::MarketFact,
            fact.id().clone(),
        );
        let occurred_at = server_market_time();
        let record_id = deterministic_change_record_id(
            &occurred_at,
            principal.actor_id(),
            &resource,
            &request.idempotency_key,
        )
        .map_err(map_domain_error)?;
        let context = FoundationChangeContext::administrator(
            principal.clone(),
            parse_change(request.change.as_ref())?,
            record_id,
            occurred_at,
        )?;
        let validated = self
            .fully_validate_fact(principal.access_scope(), fact)
            .await?;
        let command = GovernedCorrectMarketFact::new(
            context,
            original_id,
            validated,
            IdempotencyKey::new(request.idempotency_key.clone())?,
        )?;
        MarketFactUseCase::new(self.facts.as_ref())
            .correct_governed(command)
            .await
    }

    async fn publish_curve(
        &self,
        principal: ficant_application::AuthorizedPrincipal,
        request: &pb::PublishCurveSnapshotRequest,
    ) -> Result<CurveSnapshot, ApplicationError> {
        let points = request.points.as_ref().ok_or_else(invalid)?.clone();
        let canonical_bytes = points.encode_to_vec();
        let decoded = CanonicalCurvePointSetDecoder.decode_canonical(&canonical_bytes)?;
        if points
            .points
            .iter()
            .zip(decoded.points())
            .any(|(wire, decoded)| {
                wire.yield_to_maturity.as_ref() != Some(&decimal(decoded.yield_to_maturity()))
            })
        {
            return Err(invalid());
        }
        let content_hash = ContentHash::digest(&canonical_bytes);
        let declared_size = u64::try_from(canonical_bytes.len()).map_err(|_| invalid())?;
        let curve = parse_curve_input(request.curve.as_ref(), content_hash.clone())?;
        if curve.point_schema() != CURVE_POINT_SCHEMA
            || curve.curve_family_id() != Some(decoded.curve_family_id())
        {
            return Err(invalid());
        }
        validate_curve_definitions(
            self.definitions.as_ref(),
            principal.access_scope(),
            &curve,
            &decoded,
        )
        .await?;

        let key = IdempotencyKey::new(request.idempotency_key.clone())?;
        let resource = FoundationResourceRef::unversioned(
            FoundationResourceKind::CurveSnapshot,
            curve.id().clone(),
        );
        let occurred_at = server_market_time();
        let record_id = deterministic_change_record_id(
            &occurred_at,
            principal.actor_id(),
            &resource,
            key.as_str(),
        )
        .map_err(map_domain_error)?;
        let context = FoundationChangeContext::administrator(
            principal.clone(),
            parse_change(request.change.as_ref())?,
            record_id,
            occurred_at,
        )?;
        let expected_blob = VerifiedBlobRef::new(content_hash.clone(), declared_size)?;
        let command = GovernedPublishCurveSnapshot::new(
            context,
            curve,
            declared_size,
            expected_blob.clone(),
            key.clone(),
        )?;

        let scope = principal.access_scope();
        let staged = self
            .blobs
            .begin_stage(BeginBlobStage::new(
                scope.clone(),
                command.command().curve().owner().clone(),
                declared_size,
                key,
            )?)
            .await?;
        if let Err(error) = self
            .blobs
            .append_chunk(scope, &staged, canonical_bytes)
            .await
        {
            let _ = self.blobs.discard_stage(scope, &staged).await;
            return Err(error);
        }
        let promoted = match self
            .blobs
            .verify_and_promote(VerifyBlobStage::new(
                scope.clone(),
                staged.clone(),
                content_hash,
                declared_size,
            )?)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = self.blobs.discard_stage(scope, &staged).await;
                return Err(error);
            }
        };
        if promoted != expected_blob {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::HashMismatch,
                false,
            ));
        }
        MarketFactUseCase::new(self.facts.as_ref())
            .publish_curve_governed(command)
            .await
    }

    async fn read_curve(
        &self,
        principal: &ficant_application::AuthorizedPrincipal,
        request: &pb::GetCurveSnapshotRequest,
    ) -> Result<pb::CurveSnapshotPayload, ApplicationError> {
        let requested_id = parse_ulid(request.curve_snapshot_id.as_ref())?;
        let knowledge_at = parse_market_time(request.knowledge_at.as_ref())?;
        let historical = MarketFactUseCase::new(self.facts.as_ref())
            .get_curve_at(
                principal.access_scope(),
                requested_id.clone(),
                &knowledge_at,
            )
            .await?
            .ok_or_else(not_found)?;
        let metadata = self
            .curve_metadata
            .get_curve_snapshot_metadata(principal.access_scope(), requested_id.clone())
            .await?
            .ok_or_else(not_found)?;
        let snapshot = metadata.snapshot();
        if snapshot != &historical
            || snapshot.id() != &requested_id
            || snapshot.point_schema() != CURVE_POINT_SCHEMA
            || snapshot.curve_family_id().is_none()
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::HashMismatch,
                false,
            ));
        }
        let required = RequiredVerifiedBlobRead::new(
            principal.access_scope().clone(),
            snapshot.owner().clone(),
            VerifiedReadResourceKind::CurveSnapshot,
            requested_id,
            VerifiedBlobRole::CurvePoints,
            snapshot.content_hash().clone(),
            metadata.blob_size(),
            trace_context(request),
        )?;
        let payload = self
            .blob_reader
            .read_required(&required, self.integrity_events.as_ref())
            .await?;
        let decoded = CanonicalCurvePointSetDecoder.decode_canonical(payload.bytes())?;
        if snapshot.curve_family_id() != Some(decoded.curve_family_id())
            || snapshot.content_hash() != payload.content_hash()
            || metadata.blob_size() != payload.size()
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::HashMismatch,
                false,
            ));
        }
        let points = pb::CurvePointSet::decode(payload.bytes()).map_err(|_| invalid())?;
        Ok(pb::CurveSnapshotPayload {
            curve_snapshot: Some(curve_snapshot(snapshot)),
            points: Some(points),
        })
    }
}

#[tonic::async_trait]
impl MarketFactService for MarketFactGrpcService {
    async fn append_market_fact(
        &self,
        request: Request<pb::AppendMarketFactRequest>,
    ) -> Result<Response<pb::AppendMarketFactResponse>, Status> {
        const OPERATION: &str = "facts.append";
        let result = match self.principal(
            &request,
            MARKET_FACT_WRITE_SCOPE,
            Some(PlatformRole::PlatformAdmin),
        ) {
            Err(error) => Err(error),
            Ok(principal) => self.append_fact(principal, request.get_ref()).await,
        };
        Ok(Response::new(pb::AppendMarketFactResponse {
            result: Some(match result {
                Ok(value) => pb::append_market_fact_response::Result::Fact(market_fact(&value)),
                Err(error) => {
                    pb::append_market_fact_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn correct_market_fact(
        &self,
        request: Request<pb::CorrectMarketFactRequest>,
    ) -> Result<Response<pb::CorrectMarketFactResponse>, Status> {
        const OPERATION: &str = "facts.correct";
        let result = match self.principal(
            &request,
            MARKET_FACT_WRITE_SCOPE,
            Some(PlatformRole::PlatformAdmin),
        ) {
            Err(error) => Err(error),
            Ok(principal) => self.correct_fact(principal, request.get_ref()).await,
        };
        Ok(Response::new(pb::CorrectMarketFactResponse {
            result: Some(match result {
                Ok(value) => pb::correct_market_fact_response::Result::Fact(market_fact(&value)),
                Err(error) => {
                    pb::correct_market_fact_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn publish_curve_snapshot(
        &self,
        request: Request<pb::PublishCurveSnapshotRequest>,
    ) -> Result<Response<pb::PublishCurveSnapshotResponse>, Status> {
        const OPERATION: &str = "facts.publish-curve";
        let result = match self.principal(
            &request,
            MARKET_FACT_WRITE_SCOPE,
            Some(PlatformRole::PlatformAdmin),
        ) {
            Err(error) => Err(error),
            Ok(principal) => self.publish_curve(principal, request.get_ref()).await,
        };
        Ok(Response::new(pb::PublishCurveSnapshotResponse {
            result: Some(match result {
                Ok(value) => pb::publish_curve_snapshot_response::Result::CurveSnapshot(
                    curve_snapshot(&value),
                ),
                Err(error) => pb::publish_curve_snapshot_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn query_instrument_facts(
        &self,
        request: Request<pb::QueryInstrumentFactsRequest>,
    ) -> Result<Response<pb::QueryInstrumentFactsResponse>, Status> {
        const OPERATION: &str = "facts.query";
        let result = match self.principal(&request, MARKET_FACT_READ_SCOPE, None) {
            Err(error) => Err(error),
            Ok(principal) => {
                let payload = request.get_ref();
                let page = payload.page.clone().unwrap_or_default();
                let limit = if page.page_size == 0 {
                    DEFAULT_PAGE_SIZE
                } else {
                    page.page_size
                };
                match (
                    parse_version_ref(payload.instrument.as_ref()),
                    parse_market_time(payload.from.as_ref()),
                    parse_market_time(payload.to.as_ref()),
                    parse_market_time(payload.knowledge_at.as_ref()),
                    parse_cursor(
                        self.cursor_codec.as_ref(),
                        principal.access_scope(),
                        &page.cursor,
                    ),
                ) {
                    (Ok(instrument), Ok(from), Ok(to), Ok(knowledge_at), Ok(cursor)) => {
                        match PageRequest::new(principal.access_scope().clone(), cursor, limit)
                            .and_then(|page| {
                                MarketFactWindow::new(instrument, from, to, knowledge_at, page)
                            }) {
                            Err(error) => Err(error),
                            Ok(query) => {
                                MarketFactUseCase::new(self.facts.as_ref())
                                    .query(principal.access_scope(), query)
                                    .await
                            }
                        }
                    }
                    (Err(error), _, _, _, _)
                    | (_, Err(error), _, _, _)
                    | (_, _, Err(error), _, _)
                    | (_, _, _, Err(error), _)
                    | (_, _, _, _, Err(error)) => Err(error),
                }
            }
        };
        Ok(Response::new(pb::QueryInstrumentFactsResponse {
            result: Some(match result {
                Ok(page) => pb::query_instrument_facts_response::Result::InstrumentFacts(
                    pb::InstrumentFacts {
                        facts: page.items().iter().map(market_fact).collect(),
                        page: Some(core::PageResponse {
                            next_cursor: page
                                .next_cursor()
                                .map_or_else(String::new, |value| value.as_str().to_owned()),
                        }),
                    },
                ),
                Err(error) => pb::query_instrument_facts_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn get_curve_snapshot(
        &self,
        request: Request<pb::GetCurveSnapshotRequest>,
    ) -> Result<Response<pb::GetCurveSnapshotResponse>, Status> {
        const OPERATION: &str = "facts.get-curve";
        let result = match self.principal(&request, MARKET_FACT_READ_SCOPE, None) {
            Err(error) => Err(error),
            Ok(principal) => self.read_curve(&principal, request.get_ref()).await,
        };
        Ok(Response::new(pb::GetCurveSnapshotResponse {
            result: Some(match result {
                Ok(value) => pb::get_curve_snapshot_response::Result::Curve(value),
                Err(error) => {
                    pb::get_curve_snapshot_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }
}

fn parse_fact(value: Option<&pb::MarketFact>) -> Result<MarketFact, ApplicationError> {
    match value
        .ok_or_else(invalid)?
        .fact
        .as_ref()
        .ok_or_else(invalid)?
    {
        pb::market_fact::Fact::Cashflow(value) => Cashflow::new(CashflowInput {
            cashflow_id: parse_ulid(value.cashflow_id.as_ref())?,
            bond: parse_version_ref(value.bond.as_ref())?,
            payment_time: parse_market_time(value.payment_time.as_ref())?,
            amount: parse_fact_decimal(value.amount.as_ref())?,
            owner: parse_owner(value.owner.as_ref())?,
            source: parse_fact_source(value.source.as_ref())?,
            supersedes_id: parse_optional_ulid(value.supersedes_id.as_ref())?,
            cashflow_type: parse_cashflow_type(value.cashflow_type)?,
            schedule_id: value.schedule_id.clone(),
            sequence: value.sequence,
        })
        .map(MarketFact::Cashflow)
        .map_err(map_domain_error),
        pb::market_fact::Fact::Quote(value) => Quote::new(QuoteInput {
            quote_id: parse_ulid(value.quote_id.as_ref())?,
            instrument: parse_version_ref(value.instrument.as_ref())?,
            owner: parse_owner(value.owner.as_ref())?,
            source: parse_fact_source(value.source.as_ref())?,
            observed_at: parse_market_time(value.observed_at.as_ref())?,
            received_at: parse_market_time(value.received_at.as_ref())?,
            bid: value
                .bid
                .as_ref()
                .map(|value| parse_fact_decimal(Some(value)))
                .transpose()?,
            ask: value
                .ask
                .as_ref()
                .map(|value| parse_fact_decimal(Some(value)))
                .transpose()?,
            supersedes_id: parse_optional_ulid(value.supersedes_id.as_ref())?,
        })
        .map(MarketFact::Quote)
        .map_err(map_domain_error),
        pb::market_fact::Fact::Trade(value) => Trade::new(TradeInput {
            trade_id: parse_ulid(value.trade_id.as_ref())?,
            instrument: parse_version_ref(value.instrument.as_ref())?,
            owner: parse_owner(value.owner.as_ref())?,
            source: parse_fact_source(value.source.as_ref())?,
            executed_at: parse_market_time(value.executed_at.as_ref())?,
            price: parse_fact_decimal(value.price.as_ref())?,
            quantity: parse_fact_decimal(value.quantity.as_ref())?,
            supersedes_id: parse_optional_ulid(value.supersedes_id.as_ref())?,
        })
        .map(MarketFact::Trade)
        .map_err(map_domain_error),
        pb::market_fact::Fact::Valuation(value) => Valuation::new_with_value_roles(
            ValuationInput {
                valuation_id: parse_ulid(value.valuation_id.as_ref())?,
                instrument: parse_version_ref(value.instrument.as_ref())?,
                owner: parse_owner(value.owner.as_ref())?,
                source: parse_fact_source(value.source.as_ref())?,
                valuation_at: parse_market_time(value.valuation_at.as_ref())?,
                method: value.method.clone(),
                rule_pack: parse_version_ref(value.rule_pack.as_ref())?,
                values: value
                    .values
                    .iter()
                    .map(|value| parse_fact_decimal(Some(value)))
                    .collect::<Result<Vec<_>, _>>()?,
                supersedes_id: parse_optional_ulid(value.supersedes_id.as_ref())?,
            },
            parse_valuation_value_roles(&value.value_roles)?,
        )
        .map(MarketFact::Valuation)
        .map_err(map_domain_error),
    }
}

fn parse_fact_source(value: Option<&pb::FactSource>) -> Result<FactSource, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let data_source = parse_version_ref(value.data_source.as_ref())?;
    FactSource::new(
        value.source_id.clone(),
        value.external_id.clone(),
        value.source_revision,
    )
    .and_then(|source| source.with_data_source(data_source))
    .map_err(map_domain_error)
}

fn parse_fact_decimal(
    value: Option<&core::DecimalValue>,
) -> Result<ficant_domain::primitives::DecimalValue, ApplicationError> {
    let wire = value.ok_or_else(invalid)?;
    let parsed = parse_decimal(Some(wire))?;
    if decimal(&parsed) != *wire {
        return Err(invalid());
    }
    Ok(parsed)
}

fn parse_optional_ulid(value: Option<&core::Ulid>) -> Result<Option<Ulid>, ApplicationError> {
    value.map(|value| parse_ulid(Some(value))).transpose()
}

fn parse_cashflow_type(value: i32) -> Result<CashflowType, ApplicationError> {
    match pb::CashflowType::try_from(value).map_err(|_| invalid())? {
        pb::CashflowType::Coupon => Ok(CashflowType::Coupon),
        pb::CashflowType::Principal => Ok(CashflowType::Principal),
        pb::CashflowType::Fee => Ok(CashflowType::Fee),
        pb::CashflowType::Other => Ok(CashflowType::Other),
        pb::CashflowType::Unspecified => Err(invalid()),
    }
}

fn parse_valuation_value_roles(
    values: &[i32],
) -> Result<Vec<ValuationValueRole>, ApplicationError> {
    values
        .iter()
        .map(
            |value| match pb::ValuationValueRole::try_from(*value).map_err(|_| invalid())? {
                pb::ValuationValueRole::Unspecified => Err(invalid()),
                pb::ValuationValueRole::Price => Ok(ValuationValueRole::Price),
                pb::ValuationValueRole::Yield => Ok(ValuationValueRole::Yield),
                pb::ValuationValueRole::RemainingYears => Ok(ValuationValueRole::RemainingYears),
            },
        )
        .collect()
}

fn market_fact(value: &MarketFact) -> pb::MarketFact {
    let fact = match value {
        MarketFact::Cashflow(value) => pb::market_fact::Fact::Cashflow(pb::Cashflow {
            cashflow_id: Some(ulid(value.id())),
            bond: Some(version_ref(value.bond())),
            payment_time: Some(market_time(value.payment_time())),
            amount: Some(decimal(value.amount())),
            owner: Some(owner(value.owner())),
            source: Some(fact_source(value.source())),
            supersedes_id: value.supersedes_id().map(ulid),
            schedule_id: value.schedule_id().to_owned(),
            sequence: value.sequence(),
            cashflow_type: match value.cashflow_type() {
                CashflowType::Coupon => pb::CashflowType::Coupon,
                CashflowType::Principal => pb::CashflowType::Principal,
                CashflowType::Fee => pb::CashflowType::Fee,
                CashflowType::Other => pb::CashflowType::Other,
            } as i32,
        }),
        MarketFact::Quote(value) => pb::market_fact::Fact::Quote(pb::Quote {
            quote_id: Some(ulid(value.id())),
            instrument: Some(version_ref(value.instrument())),
            owner: Some(owner(value.owner())),
            source: Some(fact_source(value.source())),
            observed_at: Some(market_time(value.observed_at())),
            received_at: Some(market_time(value.received_at())),
            bid: value.bid().map(decimal),
            ask: value.ask().map(decimal),
            supersedes_id: value.supersedes_id().map(ulid),
        }),
        MarketFact::Trade(value) => pb::market_fact::Fact::Trade(pb::Trade {
            trade_id: Some(ulid(value.id())),
            instrument: Some(version_ref(value.instrument())),
            owner: Some(owner(value.owner())),
            source: Some(fact_source(value.source())),
            executed_at: Some(market_time(value.executed_at())),
            price: Some(decimal(value.price())),
            quantity: Some(decimal(value.quantity())),
            supersedes_id: value.supersedes_id().map(ulid),
        }),
        MarketFact::Valuation(value) => pb::market_fact::Fact::Valuation(pb::Valuation {
            valuation_id: Some(ulid(value.id())),
            instrument: Some(version_ref(value.instrument())),
            owner: Some(owner(value.owner())),
            source: Some(fact_source(value.source())),
            valuation_at: Some(market_time(value.valuation_at())),
            method: value.method().to_owned(),
            rule_pack: Some(version_ref(value.rule_pack())),
            values: value.values().iter().map(decimal).collect(),
            supersedes_id: value.supersedes_id().map(ulid),
            value_roles: if value.has_typed_value_roles() {
                value
                    .value_roles()
                    .iter()
                    .map(|role| proto_valuation_value_role(*role) as i32)
                    .collect()
            } else {
                Vec::new()
            },
        }),
    };
    pb::MarketFact { fact: Some(fact) }
}

const fn proto_valuation_value_role(value: ValuationValueRole) -> pb::ValuationValueRole {
    match value {
        ValuationValueRole::Price => pb::ValuationValueRole::Price,
        ValuationValueRole::Yield => pb::ValuationValueRole::Yield,
        ValuationValueRole::RemainingYears => pb::ValuationValueRole::RemainingYears,
    }
}

fn fact_source(value: &FactSource) -> pb::FactSource {
    pb::FactSource {
        source_id: value.source_id().to_owned(),
        external_id: value.external_id().to_owned(),
        source_revision: value.source_revision(),
        data_source: value.data_source().map(version_ref),
    }
}

fn parse_curve_input(
    value: Option<&pb::CurveSnapshotInput>,
    content_hash: ContentHash,
) -> Result<CurveSnapshot, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let curve = CurveSnapshot::new(CurveSnapshotInput {
        curve_snapshot_id: parse_ulid(value.curve_snapshot_id.as_ref())?,
        owner: parse_owner(value.owner.as_ref())?,
        as_of: parse_market_time(value.as_of.as_ref())?,
        currency: parse_unit_ref(value.currency.as_ref())?,
        curve_kind: value.curve_kind.clone(),
        calendar: parse_version_ref(value.calendar.as_ref())?,
        rule_pack: parse_version_ref(value.rule_pack.as_ref())?,
        point_schema: value.point_schema.clone(),
        content_hash,
        lineage: value
            .lineage
            .iter()
            .map(parse_lineage)
            .collect::<Result<Vec<_>, _>>()?,
        input_kind: ArtifactInputKind::ExternalFixture,
    })
    .map_err(map_domain_error)?;
    curve
        .with_knowledge_time(
            parse_market_time(value.visible_at.as_ref())?,
            value.curve_family_id.clone(),
        )
        .map_err(map_domain_error)
}

async fn validate_curve_definitions(
    definitions: &dyn DefinitionRepository,
    scope: &ficant_application::AccessScope,
    curve: &CurveSnapshot,
    points: &ficant_application::ports::DecodedCurvePointSet,
) -> Result<(), ApplicationError> {
    let unit = definitions
        .get_version(
            scope,
            curve.currency().unit_id().clone(),
            curve.currency().version(),
        )
        .await?
        .ok_or_else(invalid)?;
    let DefinitionValue::Unit(unit) = unit else {
        return Err(invalid());
    };
    let calendar = definitions
        .get_version(
            scope,
            curve.calendar().id().clone(),
            curve.calendar().version(),
        )
        .await?
        .ok_or_else(invalid)?;
    let DefinitionValue::Calendar(calendar) = calendar else {
        return Err(invalid());
    };
    let rule_pack = definitions
        .get_version(
            scope,
            curve.rule_pack().id().clone(),
            curve.rule_pack().version(),
        )
        .await?
        .ok_or_else(invalid)?;
    let DefinitionValue::MarketRulePack(rule_pack) = rule_pack else {
        return Err(invalid());
    };
    let subject = curve.as_of().instant();
    if unit.identity() != curve.currency().unit_id().as_str()
        || unit.version() != curve.currency().version().get()
        || unit.owner() != curve.owner()
        || unit.dimension() != "currency"
        || calendar.identity() != curve.calendar().id().as_str()
        || calendar.version() != curve.calendar().version().get()
        || calendar.owner() != curve.owner()
        || subject < calendar.effective().from().instant()
        || subject >= calendar.effective().to().instant()
        || rule_pack.identity() != curve.rule_pack().id().as_str()
        || rule_pack.version() != curve.rule_pack().version().get()
        || rule_pack.owner() != curve.owner()
        || rule_pack.verification_status() != VerificationStatus::Verified
        || subject < rule_pack.effective().from().instant()
        || subject >= rule_pack.effective().to().instant()
    {
        return Err(invalid());
    }
    for point in points.points() {
        let reference = point.yield_to_maturity().unit();
        let value = definitions
            .get_version(scope, reference.unit_id().clone(), reference.version())
            .await?
            .ok_or_else(invalid)?;
        let DefinitionValue::Unit(rate_unit) = value else {
            return Err(invalid());
        };
        if rate_unit.identity() != reference.unit_id().as_str()
            || rate_unit.version() != reference.version().get()
            || rate_unit.owner() != curve.owner()
            || rate_unit.dimension() != "rate"
            || point.yield_to_maturity().scale() > rate_unit.scale()
            || decimal_precision(point.yield_to_maturity().coefficient()) > rate_unit.precision()
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn decimal_precision(coefficient: &str) -> u32 {
    u32::try_from(coefficient.trim_start_matches('-').len()).unwrap_or(u32::MAX)
}

fn curve_snapshot(value: &CurveSnapshot) -> pb::CurveSnapshot {
    pb::CurveSnapshot {
        curve_snapshot_id: Some(ulid(value.id())),
        owner: Some(owner(value.owner())),
        as_of: Some(market_time(value.as_of())),
        currency: Some(unit_ref(value.currency())),
        curve_kind: value.curve_kind().to_owned(),
        calendar: Some(version_ref(value.calendar())),
        rule_pack: Some(version_ref(value.rule_pack())),
        point_schema: value.point_schema().to_owned(),
        content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
        visible_at: value.visible_at().map(market_time),
        curve_family_id: value.curve_family_id().unwrap_or_default().to_owned(),
    }
}

fn parse_lineage(value: &core::LineageRef) -> Result<LineageRef, ApplicationError> {
    let version = if value.version == 0 {
        None
    } else {
        Some(Version::new(value.version).map_err(map_domain_error)?)
    };
    let content_hash = value
        .content_hash
        .as_ref()
        .map(|value| parse_hash(Some(value)))
        .transpose()?;
    LineageRef::new(parse_ulid(value.object_id.as_ref())?, version, content_hash)
        .map_err(map_domain_error)
}

fn lineage(value: &LineageRef) -> core::LineageRef {
    core::LineageRef {
        object_id: Some(ulid(value.object_id())),
        version: value.version().map_or(0, Version::get),
        content_hash: value.content_hash().map(hash),
    }
}

fn parse_cursor(
    codec: &AeadCursorCodec,
    scope: &ficant_application::AccessScope,
    token: &str,
) -> Result<Option<Cursor>, ApplicationError> {
    if token.is_empty() {
        Ok(None)
    } else {
        Cursor::resume(codec, scope, token.to_owned()).map(Some)
    }
}

fn trace_context(message: &impl Message) -> SafeTraceContext {
    let hash = ContentHash::digest(&message.encode_to_vec());
    let value = hash.as_bytes()[..16]
        .iter()
        .fold(String::with_capacity(32), |mut output, byte| {
            use std::fmt::Write as _;
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        });
    SafeTraceContext::new(value).expect("derived trace token is canonical")
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
mod valuation_role_tests {
    use super::*;

    #[test]
    fn valuation_roles_accept_legacy_and_reject_unspecified_or_unknown_values() {
        assert!(parse_valuation_value_roles(&[]).unwrap().is_empty());
        assert_eq!(
            parse_valuation_value_roles(&[
                pb::ValuationValueRole::Yield as i32,
                pb::ValuationValueRole::RemainingYears as i32,
            ])
            .unwrap(),
            vec![
                ValuationValueRole::Yield,
                ValuationValueRole::RemainingYears,
            ]
        );
        assert!(
            parse_valuation_value_roles(&[pb::ValuationValueRole::Unspecified as i32]).is_err()
        );
        assert!(parse_valuation_value_roles(&[i32::MAX]).is_err());
    }

    #[test]
    fn all_price_is_the_legacy_canonical_transport_shape() {
        let roles = parse_valuation_value_roles(&[
            pb::ValuationValueRole::Price as i32,
            pb::ValuationValueRole::Price as i32,
        ])
        .unwrap();
        assert!(roles.iter().all(|role| *role == ValuationValueRole::Price));
        assert_eq!(
            proto_valuation_value_role(ValuationValueRole::Yield),
            pb::ValuationValueRole::Yield
        );
    }
}
