use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, BondAnalyticsEngine, CanonicalSnapshotDecoder, CurvePointSetDecoder,
    CurveSnapshotMetadataRepository, DataSourceRepository, DefinitionRepository,
    FactorTopologyRepository, FuturesDeliveryEngine, FuturesDeliveryRuleParser, IntegrityEventSink,
    PositionSnapshotRepository, SnapshotVerifiedReadMetadataRepository, VerifiedBlobReader,
    YieldCurveEngine,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, CalculateBondKeyRateDv01,
    CalculateBondKeyRateDv01Command, map_domain_error,
};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as market;
use ficant_contracts::ficant::research::v1 as pb;
use ficant_contracts::ficant::research::v1::portfolio_risk_service_server::PortfolioRiskService;
use ficant_domain::market::PriceSourceType;
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{FactorDv01, PortfolioKeyRateExposure, PositionKeyRateExposure};
use ficant_domain::{ContentAddressed, Lineaged};
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;

const ANALYZE_SCOPE: &str = "rates:analyze";

#[derive(Clone)]
pub struct PortfolioRiskGrpcService {
    identity: Arc<dyn PlatformPort>,
    access_scope: AccessScope,
    positions: Arc<dyn PositionSnapshotRepository>,
    curves: Arc<dyn CurveSnapshotMetadataRepository>,
    definitions: Arc<dyn DefinitionRepository>,
    data_sources: Arc<dyn DataSourceRepository>,
    factors: Arc<dyn FactorTopologyRepository>,
    blobs: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
    decoder: Arc<dyn CurvePointSetDecoder>,
    curve_engine: Arc<dyn YieldCurveEngine>,
    bond_engine: Arc<dyn BondAnalyticsEngine>,
    futures_snapshot_metadata: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
    futures_snapshot_decoder: Arc<dyn CanonicalSnapshotDecoder>,
    futures_rule_parser: Arc<dyn FuturesDeliveryRuleParser>,
    futures_engine: Arc<dyn FuturesDeliveryEngine>,
    errors: CoreBusinessErrorMapper,
}

impl PortfolioRiskGrpcService {
    /// Composes the authenticated `PortfolioRisk` transport with exact application ports.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the trace-key contract is invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        access_scope: AccessScope,
        positions: Arc<dyn PositionSnapshotRepository>,
        curves: Arc<dyn CurveSnapshotMetadataRepository>,
        definitions: Arc<dyn DefinitionRepository>,
        data_sources: Arc<dyn DataSourceRepository>,
        factors: Arc<dyn FactorTopologyRepository>,
        blobs: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
        decoder: Arc<dyn CurvePointSetDecoder>,
        curve_engine: Arc<dyn YieldCurveEngine>,
        bond_engine: Arc<dyn BondAnalyticsEngine>,
        futures_snapshot_metadata: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
        futures_snapshot_decoder: Arc<dyn CanonicalSnapshotDecoder>,
        futures_rule_parser: Arc<dyn FuturesDeliveryRuleParser>,
        futures_engine: Arc<dyn FuturesDeliveryEngine>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            access_scope,
            positions,
            curves,
            definitions,
            data_sources,
            factors,
            blobs,
            integrity_events,
            decoder,
            curve_engine,
            bond_engine,
            futures_snapshot_metadata,
            futures_snapshot_decoder,
            futures_rule_parser,
            futures_engine,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn authorize(&self, request: &Request<impl Sized>) -> Result<(), ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|_| forbidden())?;
        session
            .has_scope(ANALYZE_SCOPE)
            .then_some(())
            .ok_or_else(forbidden)
    }
}

#[tonic::async_trait]
impl PortfolioRiskService for PortfolioRiskGrpcService {
    async fn calculate_key_rate_dv01(
        &self,
        request: Request<pb::CalculateKeyRateDv01Request>,
    ) -> Result<Response<pb::CalculateKeyRateDv01Response>, Status> {
        const OPERATION: &str = "portfolio-risk.calculate-key-rate-dv01";
        let result = match self.authorize(&request) {
            Err(error) => Err(error),
            Ok(()) => match parse_command(request.get_ref()) {
                Err(error) => Err(error),
                Ok(command) => {
                    CalculateBondKeyRateDv01::new_with_futures(
                        self.positions.as_ref(),
                        self.curves.as_ref(),
                        self.definitions.as_ref(),
                        self.factors.as_ref(),
                        self.blobs.as_ref(),
                        self.integrity_events.as_ref(),
                        self.decoder.as_ref(),
                        self.curve_engine.as_ref(),
                        self.bond_engine.as_ref(),
                        self.futures_snapshot_metadata.as_ref(),
                        self.futures_snapshot_decoder.as_ref(),
                        self.data_sources.as_ref(),
                        self.futures_rule_parser.as_ref(),
                        self.futures_engine.as_ref(),
                    )
                    .execute(&self.access_scope, command)
                    .await
                }
            },
        };
        Ok(Response::new(pb::CalculateKeyRateDv01Response {
            result: Some(match result {
                Ok(value) => {
                    pb::calculate_key_rate_dv01_response::Result::Exposure(portfolio(&value))
                }
                Err(error) => pb::calculate_key_rate_dv01_response::Result::Error(self.errors.map(
                    OPERATION,
                    "portfolio-risk-application",
                    &error,
                )),
            }),
        }))
    }
}

fn parse_command(
    value: &pb::CalculateKeyRateDv01Request,
) -> Result<CalculateBondKeyRateDv01Command, ApplicationError> {
    let position_snapshot_id = parse_ulid(value.position_snapshot_id.as_ref())?;
    let knowledge_at = parse_market_time(value.knowledge_at.as_ref())?;
    let valuation_at = parse_market_time(value.valuation_at.as_ref())?;
    let curve_snapshot_id = parse_ulid(value.curve_snapshot_id.as_ref())?;
    let dv01_unit = parse_unit(value.dv01_unit.as_ref())?;
    match value.futures_data_snapshot_id.as_ref() {
        Some(snapshot_id) => CalculateBondKeyRateDv01Command::new_with_futures_data_snapshot(
            position_snapshot_id,
            knowledge_at,
            valuation_at,
            curve_snapshot_id,
            dv01_unit,
            parse_ulid(Some(snapshot_id))?,
        ),
        None => CalculateBondKeyRateDv01Command::new(
            position_snapshot_id,
            knowledge_at,
            valuation_at,
            curve_snapshot_id,
            dv01_unit,
        ),
    }
}

fn portfolio(value: &PortfolioKeyRateExposure) -> pb::PortfolioKeyRateExposure {
    pb::PortfolioKeyRateExposure {
        position_snapshot_id: Some(ulid(value.position_snapshot_id())),
        curve_snapshot_id: Some(ulid(value.curve_snapshot_id())),
        positions: value.positions().iter().map(position).collect(),
        totals: value.totals().iter().map(factor).collect(),
        algorithm: Some(pb::RiskAlgorithmBinding {
            algorithm_id: value.algorithm().algorithm_id().to_owned(),
            algorithm_version: value.algorithm().algorithm_version(),
            convention_profile: value.algorithm().convention_profile().to_owned(),
        }),
        content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
        futures_data_snapshot_id: value.futures_data_snapshot_id().map(ulid),
        source_confidence: Some(pb::PriceSourceSummary {
            counts: value
                .source_confidence()
                .counts()
                .iter()
                .map(|count| pb::PriceSourceCount {
                    source_type: price_source_type(count.source_type()) as i32,
                    record_count: count.record_count(),
                })
                .collect(),
            mixed: value.source_confidence().mixed(),
        }),
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

fn position(value: &PositionKeyRateExposure) -> pb::PositionKeyRateExposure {
    pb::PositionKeyRateExposure {
        position_id: Some(ulid(value.position_id())),
        instrument: Some(version_ref(value.instrument())),
        exposures: value.exposures().iter().map(factor).collect(),
        content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
    }
}

fn factor(value: &FactorDv01) -> pb::FactorDv01 {
    pb::FactorDv01 {
        factor_id: value.factor_id().to_owned(),
        factor_definition_hash: Some(hash(value.factor_definition_hash())),
        dv01: Some(core::DecimalValue {
            coefficient: value.value().scaled().to_string(),
            scale: 12,
            unit: Some(unit(value.unit())),
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

fn version_ref(value: &VersionRef) -> core::VersionRef {
    core::VersionRef {
        id: Some(ulid(value.id())),
        version: value.version().get(),
    }
}

fn unit(value: &UnitRef) -> core::UnitRef {
    core::UnitRef {
        unit_id: Some(ulid(value.unit_id())),
        version: value.version().get(),
    }
}

fn hash(value: &ContentHash) -> core::Sha256 {
    core::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn ulid(value: &Ulid) -> core::Ulid {
    core::Ulid {
        value: value.as_str().to_owned(),
    }
}

fn parse_market_time(value: Option<&core::MarketTime>) -> Result<MarketTime, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let instant = value.instant.as_ref().ok_or_else(invalid)?;
    let nanos = u32::try_from(instant.nanos).map_err(|_| invalid())?;
    let instant = Utc
        .timestamp_opt(instant.seconds, nanos)
        .single()
        .ok_or_else(invalid)?;
    let local_date =
        NaiveDate::parse_from_str(&value.local_trading_date, "%Y-%m-%d").map_err(|_| invalid())?;
    MarketTime::new(instant, value.market_timezone.clone(), local_date).map_err(map_domain_error)
}

fn parse_unit(value: Option<&core::UnitRef>) -> Result<UnitRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(UnitRef::new(
        parse_ulid(value.unit_id.as_ref())?,
        Version::new(value.version).map_err(map_domain_error)?,
    ))
}

fn parse_ulid(value: Option<&core::Ulid>) -> Result<Ulid, ApplicationError> {
    Ulid::new(value.ok_or_else(invalid)?.value.clone()).map_err(map_domain_error)
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}
