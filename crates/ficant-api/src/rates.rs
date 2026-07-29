use crate::core_error::CoreBusinessErrorMapper;
use crate::error::{PlatformFailure, PlatformFailureCode};
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;
use chrono::{DateTime, NaiveDate, Utc};
use ficant_application::ports::{
    BondAnalyticsEngine, CarryRollEngine, DefinitionRepository, FundingRulePackParser,
    FuturesDeliveryEngine, FuturesDeliveryRuleParser, FuturesHedgeEngine, SubjectRepository,
    YieldCurveEngine,
};
use ficant_application::use_cases::{
    funding_rule::ResolveFundingRule, subject_resolution::ResolveSubject,
};
use ficant_application::{
    AccessScope, ApplicationError, ApplicationErrorCategory, CalculateBondAnalytics,
    CalculateCarryRoll, CalculateFuturesDeliveryBasket, CalculateFuturesHedge,
    ResolveFuturesDeliveryRule, map_analytics_error, map_domain_error,
};
use ficant_contracts::ficant::core::v1::{
    DecimalValue, OwnerRef as ProtoOwnerRef, UnitRef as ProtoUnitRef,
};
use ficant_contracts::ficant::rates::v1 as pb;
use ficant_contracts::ficant::rates::v1::rates_analytics_service_server::RatesAnalyticsService;
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{
    ABI_VERSION, ALGORITHM_ID, ALGORITHM_VERSION, AnalyticsMode, AnalyticsObjectRef,
    BondAnalyticsInput, BondAnalyticsResult, BondTerms, BusinessDayConvention, CONVENTION_PROFILE,
    CalendarBinding, CalendarRequirement, CouponFrequency, DECIMAL_SCALE, DayCountConvention,
    ENGINE_ID, ENGINE_VERSION, FixedDecimal,
};
use ficant_domain::curves::{
    CARRY_ROLL_ALGORITHM_ID, CARRY_ROLL_ALGORITHM_VERSION, CARRY_ROLL_CONVENTION_PROFILE,
    CURVE_ALGORITHM_ID, CURVE_ALGORITHM_VERSION, CURVE_CONVENTION_PROFILE, CarryRollInput,
    CarryRollResult, YieldCurveBinding, YieldCurveInterpolation, YieldCurveNode, YieldCurvePoint,
    YieldCurveQuery,
};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FUTURES_DELIVERY_ALGORITHM_ID, FUTURES_DELIVERY_ALGORITHM_VERSION,
    FUTURES_DELIVERY_CONVENTION_PROFILE, FuturesDeliverableInput, FuturesDeliveryBasketResult,
    FuturesDeliveryMeasures,
};
use ficant_domain::futures_hedge::{
    FUTURES_HEDGE_ALGORITHM_ID, FUTURES_HEDGE_ALGORITHM_VERSION, FUTURES_HEDGE_CONVENTION_PROFILE,
    FuturesHedgeInput, FuturesHedgeResult,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue as DomainDecimalValue, MarketTime, OwnerRef, Ulid, UnitRef, Version,
    VersionRef,
};
use std::sync::Arc;
use tonic::{Request, Response, Status};

const REQUIRED_SCOPE: &str = "rates:analyze";
const CN_MARKET: &str = "CN";
const BOND_TOOL: &str = "bond-analytics";
const CURVE_TOOL: &str = "yield-curve";
const CARRY_ROLL_TOOL: &str = "carry-roll";
const FUTURES_DELIVERY_TOOL: &str = "futures-delivery";
const FUTURES_HEDGE_TOOL: &str = "futures-hedge";

#[derive(Clone)]
pub struct RatesGrpcService {
    identity: Arc<dyn PlatformPort>,
    bond: Arc<dyn BondAnalyticsEngine>,
    curve: Arc<dyn YieldCurveEngine>,
    carry_roll: Arc<dyn CarryRollEngine>,
    futures_delivery: Arc<dyn FuturesDeliveryEngine>,
    definitions: Arc<dyn DefinitionRepository>,
    subjects: Arc<dyn SubjectRepository>,
    futures_delivery_rule_parser: Arc<dyn FuturesDeliveryRuleParser>,
    funding_rule_pack_parser: Arc<dyn FundingRulePackParser>,
    futures_hedge: Arc<dyn FuturesHedgeEngine>,
    errors: CoreBusinessErrorMapper,
}

impl RatesGrpcService {
    /// Creates the authenticated transport adapter for all Phase 2 reference calculations.
    ///
    /// # Errors
    ///
    /// Returns an error when the trace-signing key contains fewer than 32 bytes.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        bond: Arc<dyn BondAnalyticsEngine>,
        curve: Arc<dyn YieldCurveEngine>,
        carry_roll: Arc<dyn CarryRollEngine>,
        futures_delivery: Arc<dyn FuturesDeliveryEngine>,
        definitions: Arc<dyn DefinitionRepository>,
        subjects: Arc<dyn SubjectRepository>,
        futures_delivery_rule_parser: Arc<dyn FuturesDeliveryRuleParser>,
        funding_rule_pack_parser: Arc<dyn FundingRulePackParser>,
        futures_hedge: Arc<dyn FuturesHedgeEngine>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            bond,
            curve,
            carry_roll,
            futures_delivery,
            definitions,
            subjects,
            futures_delivery_rule_parser,
            funding_rule_pack_parser,
            futures_hedge,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn authorize(&self, request: &Request<impl Sized>) -> Result<(), ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|failure| platform_application_error(&failure))?;
        if !session.has_scope(REQUIRED_SCOPE) {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::Forbidden,
                false,
            ));
        }
        Ok(())
    }

    fn error(
        &self,
        operation: &str,
        error: &ApplicationError,
    ) -> ficant_contracts::ficant::core::v1::ErrorDetail {
        self.errors.map(operation, "rates-application", error)
    }

    async fn analyze_bond_value(
        &self,
        request: &pb::AnalyzeBondRequest,
    ) -> Result<pb::AnalyzeBondResult, ApplicationError> {
        let context = parse_context(request.context.as_ref(), ExpectedAlgorithm::bond())?;
        reject_funding_rule_pack(&context)?;
        ResolveSubject::new(self.subjects.as_ref())
            .execute(&context.subject_ref, CN_MARKET, BOND_TOOL)
            .await?;
        let parsed = parse_analyze_bond_request(request)?;
        execute_parsed_bond_request(self.bond.as_ref(), &parsed)
    }

    async fn interpolate_curve_value(
        &self,
        request: &pb::InterpolateYieldCurveRequest,
    ) -> Result<pb::InterpolateYieldCurveResult, ApplicationError> {
        let context = parse_context(request.context.as_ref(), ExpectedAlgorithm::curve())?;
        reject_funding_rule_pack(&context)?;
        ResolveSubject::new(self.subjects.as_ref())
            .execute(&context.subject_ref, CN_MARKET, CURVE_TOOL)
            .await?;
        let query = YieldCurveQuery::new(
            parse_curve(request.curve.as_ref(), &context.units)?,
            parse_date(&request.query_date)?,
        )
        .map_err(map_domain_error)?;
        let point = self
            .curve
            .interpolate(&query)
            .map_err(map_analytics_error)?;
        point.validate_against(&query).map_err(map_domain_error)?;
        Ok(curve_result(&point, &context.units, &context.subject_ref))
    }

    async fn analyze_carry_roll_value(
        &self,
        request: &pb::AnalyzeCarryRollRequest,
    ) -> Result<pb::AnalyzeCarryRollResult, ApplicationError> {
        let context = parse_context(request.context.as_ref(), ExpectedAlgorithm::carry_roll())?;
        reject_funding_rule_pack(&context)?;
        ResolveSubject::new(self.subjects.as_ref())
            .execute(&context.subject_ref, CN_MARKET, CARRY_ROLL_TOOL)
            .await?;
        let input = CarryRollInput::new(
            context.owner,
            parse_object(request.bond.as_ref())?,
            context.rule_pack,
            context.data_snapshot,
            parse_market_time(request.valuation_at.as_ref())?,
            parse_date(&request.initial_settlement)?,
            parse_date(&request.horizon_settlement)?,
            parse_calendar_requirement(request.calendar_requirement)?,
            parse_calendar(request.calendar.as_ref())?,
            parse_bond_terms(request.terms.as_ref(), &context.units)?,
            parse_curve(request.curve.as_ref(), &context.units)?,
        )
        .map_err(map_domain_error)?;
        let result = CalculateCarryRoll::new(self.carry_roll.as_ref()).execute(&input)?;
        Ok(carry_roll_result(
            &result,
            &context.units,
            &context.subject_ref,
        ))
    }

    async fn analyze_futures_delivery_value(
        &self,
        request: &pb::AnalyzeFuturesDeliveryRequest,
    ) -> Result<pb::AnalyzeFuturesDeliveryResult, ApplicationError> {
        let context = parse_context(
            request.context.as_ref(),
            ExpectedAlgorithm::futures_delivery(),
        )?;
        let subject = ResolveSubject::new(self.subjects.as_ref())
            .execute(
                &context.subject_ref,
                self.futures_delivery_rule_parser.market(),
                FUTURES_DELIVERY_TOOL,
            )
            .await?;
        let funding_rule_pack = context.funding_rule_pack.as_ref().ok_or_else(invalid)?;
        let futures_contract = parse_object(request.futures_contract.as_ref())?;
        let valuation_at = parse_market_time(request.valuation_at.as_ref())?;
        let purchase_date = parse_date(&request.purchase_date)?;
        let delivery_month_first = parse_date(&request.delivery_month_first)?;
        let delivery_date = parse_date(&request.delivery_date)?;
        let product = parse_product(request.product)?;
        let access_scope = AccessScope::new(
            context.owner.tenant_id().clone(),
            context.owner.owner_id().clone(),
            vec![context.owner.owner_id().clone()],
        )?;
        let rule = ResolveFuturesDeliveryRule::new(
            self.definitions.as_ref(),
            self.futures_delivery_rule_parser.as_ref(),
        )
        .execute(
            &access_scope,
            &context.rule_pack,
            valuation_at.clone(),
            product,
        )
        .await?;
        let funding_rate = ResolveFundingRule::new(
            self.definitions.as_ref(),
            self.funding_rule_pack_parser.as_ref(),
        )
        .execute(
            &access_scope,
            funding_rule_pack,
            valuation_at.clone(),
            subject.funding_tier(),
        )
        .await?;
        if funding_rate.unit() != &parse_unit(&context.units.rate)? {
            return Err(map_domain_error(DomainErrorCode::InvalidUnit));
        }
        let futures_clean_price = parse_fixed_decimal(
            request.futures_clean_price.as_ref().ok_or_else(invalid)?,
            &context.units.price_per_100,
        )?;
        let financing_rate = funding_rate.annual_financing_rate();
        let inputs = request
            .candidates
            .iter()
            .map(|candidate| {
                FuturesDeliverableInput::new(
                    context.owner.clone(),
                    futures_contract.clone(),
                    parse_object(candidate.bond.as_ref())?,
                    context.rule_pack.clone(),
                    context.data_snapshot.clone(),
                    valuation_at.clone(),
                    purchase_date,
                    delivery_month_first,
                    delivery_date,
                    product,
                    rule.clone(),
                    parse_bond_terms(candidate.terms.as_ref(), &context.units)?,
                    parse_fixed_decimal(
                        candidate.spot_clean_price.as_ref().ok_or_else(invalid)?,
                        &context.units.price_per_100,
                    )?,
                    futures_clean_price,
                    financing_rate,
                )
                .map_err(map_domain_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let result =
            CalculateFuturesDeliveryBasket::new(self.futures_delivery.as_ref()).execute(&inputs)?;
        futures_delivery_result(
            &result,
            &context.units,
            &context.subject_ref,
            funding_rule_pack,
            financing_rate,
        )
    }

    async fn analyze_futures_hedge_value(
        &self,
        request: &pb::AnalyzeFuturesHedgeRequest,
    ) -> Result<pb::AnalyzeFuturesHedgeResult, ApplicationError> {
        let context = parse_context(request.context.as_ref(), ExpectedAlgorithm::futures_hedge())?;
        reject_funding_rule_pack(&context)?;
        ResolveSubject::new(self.subjects.as_ref())
            .execute(
                &context.subject_ref,
                self.futures_delivery_rule_parser.market(),
                FUTURES_HEDGE_TOOL,
            )
            .await?;
        let input = FuturesHedgeInput::new(
            context.owner,
            parse_object(request.target_risk_artifact.as_ref())?,
            parse_object(request.delivery_artifact.as_ref())?,
            parse_object(request.ctd_analytics_artifact.as_ref())?,
            parse_object(request.futures_contract.as_ref())?,
            parse_object(request.ctd_bond.as_ref())?,
            context.rule_pack,
            context.data_snapshot,
            parse_market_time(request.valuation_at.as_ref())?,
            parse_product(request.product)?,
            parse_fixed_decimal(
                request.target_dv01.as_ref().ok_or_else(invalid)?,
                &context.units.dv01,
            )?,
            parse_fixed_decimal(
                request.ctd_dv01_per_100.as_ref().ok_or_else(invalid)?,
                &context.units.dv01_per_100,
            )?,
            parse_fixed_decimal(
                request.conversion_factor.as_ref().ok_or_else(invalid)?,
                &context.units.dimensionless,
            )?,
        )
        .map_err(map_domain_error)?;
        let result = CalculateFuturesHedge::new(self.futures_hedge.as_ref()).execute(&input)?;
        Ok(futures_hedge_result(
            &result,
            &context.units,
            &context.subject_ref,
        ))
    }
}

/// Fully parsed bond-analysis request shared by gRPC and native research nodes.
pub struct ParsedBondAnalyticsRequest {
    input: BondAnalyticsInput,
    units: UnitBindings,
    subject_ref: VersionRef,
}

impl ParsedBondAnalyticsRequest {
    #[must_use]
    pub fn input(&self) -> &BondAnalyticsInput {
        &self.input
    }

    #[must_use]
    pub fn subject_ref(&self) -> &VersionRef {
        &self.subject_ref
    }
}

/// Parses and validates every field of the public bond-analysis contract without I/O.
///
/// # Errors
///
/// Returns a stable application validation error for any incomplete or inconsistent binding.
pub fn parse_analyze_bond_request(
    request: &pb::AnalyzeBondRequest,
) -> Result<ParsedBondAnalyticsRequest, ApplicationError> {
    let context = parse_context(request.context.as_ref(), ExpectedAlgorithm::bond())?;
    let terms = parse_bond_terms(request.terms.as_ref(), &context.units)?;
    let (mode, input_value) = match request.input.as_ref() {
        Some(pb::analyze_bond_request::Input::YieldToMaturity(value)) => (
            AnalyticsMode::YieldIn,
            parse_fixed_decimal(value, &context.units.rate)?,
        ),
        Some(pb::analyze_bond_request::Input::CleanPrice(value)) => (
            AnalyticsMode::PriceIn,
            parse_fixed_decimal(value, &context.units.price_per_100)?,
        ),
        None => return Err(invalid()),
    };
    let input = BondAnalyticsInput::new(
        context.owner,
        parse_object(request.bond.as_ref())?,
        context.rule_pack,
        context.data_snapshot,
        parse_market_time(request.valuation_at.as_ref())?,
        parse_date(&request.settlement_date)?,
        parse_calendar_requirement(request.calendar_requirement)?,
        parse_calendar(request.calendar.as_ref())?,
        terms,
        mode,
        input_value,
    )
    .map_err(map_domain_error)?;
    Ok(ParsedBondAnalyticsRequest {
        input,
        units: context.units,
        subject_ref: context.subject_ref,
    })
}

/// Executes an already parsed bond request and maps the domain result to its protobuf contract.
///
/// # Errors
///
/// Returns the application error produced by the real analytics engine.
pub fn execute_parsed_bond_request(
    engine: &dyn BondAnalyticsEngine,
    request: &ParsedBondAnalyticsRequest,
) -> Result<pb::AnalyzeBondResult, ApplicationError> {
    let result = CalculateBondAnalytics::new(engine).execute(&request.input)?;
    Ok(bond_result(&result, &request.units, request.subject_ref()))
}

/// Runs the shared pure parse, native calculation, and protobuf result mapping path.
///
/// # Errors
///
/// Returns a stable application error for validation or analytics failure.
pub fn analyze_bond_request(
    engine: &dyn BondAnalyticsEngine,
    request: &pb::AnalyzeBondRequest,
) -> Result<pb::AnalyzeBondResult, ApplicationError> {
    let parsed = parse_analyze_bond_request(request)?;
    execute_parsed_bond_request(engine, &parsed)
}

#[tonic::async_trait]
impl RatesAnalyticsService for RatesGrpcService {
    async fn analyze_bond(
        &self,
        request: Request<pb::AnalyzeBondRequest>,
    ) -> Result<Response<pb::AnalyzeBondResponse>, Status> {
        const OPERATION: &str = "rates.analyze-bond";
        let result = match self.authorize(&request) {
            Ok(()) => self.analyze_bond_value(request.get_ref()).await,
            Err(error) => Err(error),
        };
        Ok(Response::new(pb::AnalyzeBondResponse {
            result: Some(match result {
                Ok(value) => pb::analyze_bond_response::Result::Analysis(value),
                Err(error) => {
                    pb::analyze_bond_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn interpolate_yield_curve(
        &self,
        request: Request<pb::InterpolateYieldCurveRequest>,
    ) -> Result<Response<pb::InterpolateYieldCurveResponse>, Status> {
        const OPERATION: &str = "rates.interpolate-yield-curve";
        let result = match self.authorize(&request) {
            Ok(()) => self.interpolate_curve_value(request.get_ref()).await,
            Err(error) => Err(error),
        };
        Ok(Response::new(pb::InterpolateYieldCurveResponse {
            result: Some(match result {
                Ok(value) => pb::interpolate_yield_curve_response::Result::Point(value),
                Err(error) => pb::interpolate_yield_curve_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn analyze_carry_roll(
        &self,
        request: Request<pb::AnalyzeCarryRollRequest>,
    ) -> Result<Response<pb::AnalyzeCarryRollResponse>, Status> {
        const OPERATION: &str = "rates.analyze-carry-roll";
        let result = match self.authorize(&request) {
            Ok(()) => self.analyze_carry_roll_value(request.get_ref()).await,
            Err(error) => Err(error),
        };
        Ok(Response::new(pb::AnalyzeCarryRollResponse {
            result: Some(match result {
                Ok(value) => pb::analyze_carry_roll_response::Result::Analysis(value),
                Err(error) => {
                    pb::analyze_carry_roll_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn analyze_futures_delivery(
        &self,
        request: Request<pb::AnalyzeFuturesDeliveryRequest>,
    ) -> Result<Response<pb::AnalyzeFuturesDeliveryResponse>, Status> {
        const OPERATION: &str = "rates.analyze-futures-delivery";
        let result = match self.authorize(&request) {
            Ok(()) => self.analyze_futures_delivery_value(request.get_ref()).await,
            Err(error) => Err(error),
        };
        Ok(Response::new(pb::AnalyzeFuturesDeliveryResponse {
            result: Some(match result {
                Ok(value) => pb::analyze_futures_delivery_response::Result::Analysis(value),
                Err(error) => pb::analyze_futures_delivery_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn analyze_futures_hedge(
        &self,
        request: Request<pb::AnalyzeFuturesHedgeRequest>,
    ) -> Result<Response<pb::AnalyzeFuturesHedgeResponse>, Status> {
        const OPERATION: &str = "rates.analyze-futures-hedge";
        let result = match self.authorize(&request) {
            Ok(()) => self.analyze_futures_hedge_value(request.get_ref()).await,
            Err(error) => Err(error),
        };
        Ok(Response::new(pb::AnalyzeFuturesHedgeResponse {
            result: Some(match result {
                Ok(value) => pb::analyze_futures_hedge_response::Result::Analysis(value),
                Err(error) => {
                    pb::analyze_futures_hedge_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }
}

#[derive(Clone)]
struct ParsedContext {
    owner: OwnerRef,
    rule_pack: AnalyticsObjectRef,
    data_snapshot: AnalyticsObjectRef,
    units: UnitBindings,
    subject_ref: VersionRef,
    funding_rule_pack: Option<AnalyticsObjectRef>,
}

#[derive(Clone)]
struct UnitBindings {
    currency_amount: ProtoUnitRef,
    price_per_100: ProtoUnitRef,
    rate: ProtoUnitRef,
    years: ProtoUnitRef,
    years_squared: ProtoUnitRef,
    dv01_per_100: ProtoUnitRef,
    dv01: ProtoUnitRef,
    dimensionless: ProtoUnitRef,
    contract_count: ProtoUnitRef,
}

impl UnitBindings {
    fn parse(units: Option<&pb::AnalysisUnits>) -> Result<Self, ApplicationError> {
        let units = units.ok_or_else(invalid)?;
        Ok(Self {
            currency_amount: parse_proto_unit(units.currency_amount.as_ref())?,
            price_per_100: parse_proto_unit(units.price_per_100.as_ref())?,
            rate: parse_proto_unit(units.rate.as_ref())?,
            years: parse_proto_unit(units.years.as_ref())?,
            years_squared: parse_proto_unit(units.years_squared.as_ref())?,
            dv01_per_100: parse_proto_unit(units.dv01_per_100.as_ref())?,
            dv01: parse_proto_unit(units.dv01.as_ref())?,
            dimensionless: parse_proto_unit(units.dimensionless.as_ref())?,
            contract_count: parse_proto_unit(units.contract_count.as_ref())?,
        })
    }
}

#[derive(Clone, Copy)]
struct ExpectedAlgorithm {
    id: &'static str,
    version: u32,
    convention: &'static str,
}

impl ExpectedAlgorithm {
    const fn bond() -> Self {
        Self {
            id: ALGORITHM_ID,
            version: ALGORITHM_VERSION,
            convention: CONVENTION_PROFILE,
        }
    }
    const fn curve() -> Self {
        Self {
            id: CURVE_ALGORITHM_ID,
            version: CURVE_ALGORITHM_VERSION,
            convention: CURVE_CONVENTION_PROFILE,
        }
    }
    const fn carry_roll() -> Self {
        Self {
            id: CARRY_ROLL_ALGORITHM_ID,
            version: CARRY_ROLL_ALGORITHM_VERSION,
            convention: CARRY_ROLL_CONVENTION_PROFILE,
        }
    }
    const fn futures_delivery() -> Self {
        Self {
            id: FUTURES_DELIVERY_ALGORITHM_ID,
            version: FUTURES_DELIVERY_ALGORITHM_VERSION,
            convention: FUTURES_DELIVERY_CONVENTION_PROFILE,
        }
    }
    const fn futures_hedge() -> Self {
        Self {
            id: FUTURES_HEDGE_ALGORITHM_ID,
            version: FUTURES_HEDGE_ALGORITHM_VERSION,
            convention: FUTURES_HEDGE_CONVENTION_PROFILE,
        }
    }
}

fn parse_context(
    value: Option<&pb::AnalysisContext>,
    expected: ExpectedAlgorithm,
) -> Result<ParsedContext, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    validate_algorithm(value.algorithm.as_ref(), expected)?;
    Ok(ParsedContext {
        owner: parse_owner(value.owner.as_ref())?,
        rule_pack: parse_object(value.rule_pack.as_ref())?,
        data_snapshot: parse_object(value.data_snapshot.as_ref())?,
        units: UnitBindings::parse(value.units.as_ref())?,
        subject_ref: parse_subject_ref(value.subject_ref.as_ref())?,
        funding_rule_pack: value
            .funding_rule_pack
            .as_ref()
            .map(|binding| parse_object(Some(binding)))
            .transpose()?,
    })
}

fn reject_funding_rule_pack(context: &ParsedContext) -> Result<(), ApplicationError> {
    if context.funding_rule_pack.is_some() {
        return Err(invalid());
    }
    Ok(())
}

fn validate_algorithm(
    value: Option<&pb::AlgorithmBinding>,
    expected: ExpectedAlgorithm,
) -> Result<(), ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    if value.algorithm_id != expected.id
        || value.algorithm_version != expected.version
        || value.convention_profile != expected.convention
        || value.abi_version != ABI_VERSION
    {
        return Err(invalid());
    }
    Ok(())
}

fn parse_owner(value: Option<&ProtoOwnerRef>) -> Result<OwnerRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(OwnerRef::new(
        parse_ulid(value.tenant_id.as_ref())?,
        parse_ulid(value.owner_id.as_ref())?,
    ))
}

fn parse_object(value: Option<&pb::ObjectBinding>) -> Result<AnalyticsObjectRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let object = value.object.as_ref().ok_or_else(invalid)?;
    Ok(AnalyticsObjectRef::new(
        VersionRef::new(
            parse_ulid(object.id.as_ref())?,
            Version::new(object.version).map_err(map_domain_error)?,
        ),
        parse_hash(value.content_hash.as_ref())?,
    ))
}

fn parse_subject_ref(
    value: Option<&ficant_contracts::ficant::core::v1::VersionRef>,
) -> Result<VersionRef, ApplicationError> {
    let value = value.ok_or_else(ApplicationError::subject_binding_invalid)?;
    let id = value
        .id
        .as_ref()
        .ok_or_else(ApplicationError::subject_binding_invalid)
        .and_then(|value| {
            Ulid::new(value.value.clone()).map_err(|_| ApplicationError::subject_binding_invalid())
        })?;
    let version =
        Version::new(value.version).map_err(|_| ApplicationError::subject_binding_invalid())?;
    Ok(VersionRef::new(id, version))
}

fn parse_ulid(
    value: Option<&ficant_contracts::ficant::core::v1::Ulid>,
) -> Result<Ulid, ApplicationError> {
    Ulid::new(value.ok_or_else(invalid)?.value.clone()).map_err(map_domain_error)
}

fn parse_hash(
    value: Option<&ficant_contracts::ficant::core::v1::Sha256>,
) -> Result<ContentHash, ApplicationError> {
    ContentHash::from_bytes(&value.ok_or_else(invalid)?.value).map_err(map_domain_error)
}

fn parse_proto_unit(value: Option<&ProtoUnitRef>) -> Result<ProtoUnitRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    parse_unit(value)?;
    Ok(value.clone())
}

fn parse_unit(value: &ProtoUnitRef) -> Result<UnitRef, ApplicationError> {
    Ok(UnitRef::new(
        parse_ulid(value.unit_id.as_ref())?,
        Version::new(value.version).map_err(map_domain_error)?,
    ))
}

fn parse_fixed_decimal(
    value: &DecimalValue,
    expected_unit: &ProtoUnitRef,
) -> Result<FixedDecimal, ApplicationError> {
    if value.unit.as_ref() != Some(expected_unit) {
        return Err(map_domain_error(DomainErrorCode::InvalidUnit));
    }
    let canonical = DomainDecimalValue::new(
        value.coefficient.clone(),
        value.scale,
        parse_unit(expected_unit)?,
    )
    .map_err(map_domain_error)?;
    if canonical.coefficient() != value.coefficient || canonical.scale() != value.scale {
        return Err(invalid());
    }
    let coefficient = value.coefficient.parse::<i128>().map_err(|_| invalid())?;
    let scaled = if value.scale <= DECIMAL_SCALE {
        coefficient
            .checked_mul(power_of_ten(DECIMAL_SCALE - value.scale)?)
            .ok_or_else(invalid)?
    } else {
        let divisor = power_of_ten(value.scale - DECIMAL_SCALE)?;
        if coefficient % divisor != 0 {
            return Err(invalid());
        }
        coefficient / divisor
    };
    Ok(FixedDecimal::from_scaled(scaled))
}

fn power_of_ten(exponent: u32) -> Result<i128, ApplicationError> {
    10_i128.checked_pow(exponent).ok_or_else(invalid)
}

fn parse_date(value: &str) -> Result<NaiveDate, ApplicationError> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| invalid())?;
    if date.to_string() != value {
        return Err(invalid());
    }
    Ok(date)
}

fn parse_market_time(
    value: Option<&ficant_contracts::ficant::core::v1::MarketTime>,
) -> Result<MarketTime, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let instant = value.instant.as_ref().ok_or_else(invalid)?;
    let nanos = u32::try_from(instant.nanos).map_err(|_| invalid())?;
    let instant = DateTime::<Utc>::from_timestamp(instant.seconds, nanos).ok_or_else(invalid)?;
    MarketTime::new(
        instant,
        value.market_timezone.clone(),
        parse_date(&value.local_trading_date)?,
    )
    .map_err(map_domain_error)
}

fn parse_calendar_requirement(value: i32) -> Result<CalendarRequirement, ApplicationError> {
    match pb::CalendarRequirement::try_from(value).map_err(|_| invalid())? {
        pb::CalendarRequirement::ReferenceReplay => Ok(CalendarRequirement::ReferenceReplay),
        pb::CalendarRequirement::ExactMarket => Ok(CalendarRequirement::ExactMarket),
        pb::CalendarRequirement::Unspecified => Err(invalid()),
    }
}

fn parse_calendar(
    value: Option<&pb::CalendarBinding>,
) -> Result<CalendarBinding, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    CalendarBinding::new(
        value.calendar_id.clone(),
        Version::new(value.version).map_err(map_domain_error)?,
        parse_hash(value.content_hash.as_ref())?,
        parse_date(&value.coverage_start)?,
        parse_date(&value.coverage_end)?,
        value
            .non_business_days
            .iter()
            .map(|date| parse_date(date))
            .collect::<Result<Vec<_>, _>>()?,
        value
            .work_weekends
            .iter()
            .map(|date| parse_date(date))
            .collect::<Result<Vec<_>, _>>()?,
    )
    .map_err(map_domain_error)
}

fn parse_bond_terms(
    value: Option<&pb::BondTerms>,
    units: &UnitBindings,
) -> Result<BondTerms, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let frequency = match pb::CouponFrequency::try_from(value.frequency).map_err(|_| invalid())? {
        pb::CouponFrequency::Annual => CouponFrequency::Annual,
        pb::CouponFrequency::Semiannual => CouponFrequency::Semiannual,
        pb::CouponFrequency::Unspecified => return Err(invalid()),
    };
    BondTerms::new(
        parse_date(&value.issue_date)?,
        parse_date(&value.maturity_date)?,
        frequency,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        parse_fixed_decimal(value.coupon_rate.as_ref().ok_or_else(invalid)?, &units.rate)?,
        parse_fixed_decimal(
            value.face_amount.as_ref().ok_or_else(invalid)?,
            &units.currency_amount,
        )?,
    )
    .map_err(map_domain_error)
}

fn parse_curve(
    value: Option<&pb::YieldCurveBinding>,
    units: &UnitBindings,
) -> Result<YieldCurveBinding, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let interpolation =
        match pb::YieldCurveInterpolation::try_from(value.interpolation).map_err(|_| invalid())? {
            pb::YieldCurveInterpolation::LinearYield => YieldCurveInterpolation::LinearYield,
            pb::YieldCurveInterpolation::Unspecified => return Err(invalid()),
        };
    YieldCurveBinding::new(
        parse_object(value.curve_snapshot.as_ref())?,
        parse_date(&value.valuation_date)?,
        interpolation,
        value
            .nodes
            .iter()
            .map(|node| {
                YieldCurveNode::new(
                    parse_date(&node.maturity_date)?,
                    parse_fixed_decimal(
                        node.yield_to_maturity.as_ref().ok_or_else(invalid)?,
                        &units.rate,
                    )?,
                )
                .map_err(map_domain_error)
            })
            .collect::<Result<Vec<_>, _>>()?,
    )
    .map_err(map_domain_error)
}

fn parse_product(value: i32) -> Result<CgbFuturesProduct, ApplicationError> {
    match pb::CgbFuturesProduct::try_from(value).map_err(|_| invalid())? {
        pb::CgbFuturesProduct::Ts => Ok(CgbFuturesProduct::TwoYear),
        pb::CgbFuturesProduct::Tf => Ok(CgbFuturesProduct::FiveYear),
        pb::CgbFuturesProduct::T => Ok(CgbFuturesProduct::TenYear),
        pb::CgbFuturesProduct::Tl => Ok(CgbFuturesProduct::ThirtyYear),
        pb::CgbFuturesProduct::Unspecified => Err(invalid()),
    }
}

fn bond_result(
    result: &BondAnalyticsResult,
    units: &UnitBindings,
    subject_ref: &VersionRef,
) -> pb::AnalyzeBondResult {
    let measures = result.measures();
    pb::AnalyzeBondResult {
        cashflows: result
            .cashflows()
            .iter()
            .map(|cashflow| pb::DerivedCashflow {
                sequence: cashflow.sequence(),
                nominal_date: cashflow.nominal_date().to_string(),
                payment_date: cashflow.payment_date().to_string(),
                coupon: Some(decimal(cashflow.coupon(), &units.currency_amount)),
                principal: Some(decimal(cashflow.principal(), &units.currency_amount)),
                total: Some(decimal(cashflow.total(), &units.currency_amount)),
            })
            .collect(),
        measures: Some(pb::BondAnalyticsMeasures {
            accrued_interest: Some(decimal(measures.accrued_interest(), &units.price_per_100)),
            clean_price: Some(decimal(measures.clean_price(), &units.price_per_100)),
            dirty_price: Some(decimal(measures.dirty_price(), &units.price_per_100)),
            yield_to_maturity: Some(decimal(measures.yield_to_maturity(), &units.rate)),
            macaulay_duration: Some(decimal(measures.macaulay_duration(), &units.years)),
            modified_duration: Some(decimal(measures.modified_duration(), &units.years)),
            convexity: Some(decimal(measures.convexity(), &units.years_squared)),
            dv01: Some(decimal(measures.dv01(), &units.dv01_per_100)),
        }),
        metadata: Some(metadata(
            result.schema_id(),
            ExpectedAlgorithm::bond(),
            subject_ref,
            None,
        )),
    }
}

fn curve_result(
    point: &YieldCurvePoint,
    units: &UnitBindings,
    subject_ref: &VersionRef,
) -> pb::InterpolateYieldCurveResult {
    pb::InterpolateYieldCurveResult {
        query_date: point.query().query_date().to_string(),
        yield_to_maturity: Some(decimal(point.yield_to_maturity(), &units.rate)),
        metadata: Some(metadata(
            point.schema_id(),
            ExpectedAlgorithm::curve(),
            subject_ref,
            None,
        )),
    }
}

fn carry_roll_result(
    result: &CarryRollResult,
    units: &UnitBindings,
    subject_ref: &VersionRef,
) -> pb::AnalyzeCarryRollResult {
    let value = result.measures();
    pb::AnalyzeCarryRollResult {
        measures: Some(pb::CarryRollMeasures {
            initial_yield: Some(decimal(value.initial_yield(), &units.rate)),
            rolled_yield: Some(decimal(value.rolled_yield(), &units.rate)),
            initial_dirty_price: Some(decimal(value.initial_dirty_price(), &units.price_per_100)),
            horizon_dirty_at_initial_yield: Some(decimal(
                value.horizon_dirty_at_initial_yield(),
                &units.price_per_100,
            )),
            horizon_dirty_at_rolled_yield: Some(decimal(
                value.horizon_dirty_at_rolled_yield(),
                &units.price_per_100,
            )),
            paid_cashflows: Some(decimal(value.paid_cashflows(), &units.price_per_100)),
            carry: Some(decimal(value.carry(), &units.price_per_100)),
            roll_down: Some(decimal(value.roll_down(), &units.price_per_100)),
            total_return: Some(decimal(value.total_return(), &units.price_per_100)),
        }),
        metadata: Some(metadata(
            result.schema_id(),
            ExpectedAlgorithm::carry_roll(),
            subject_ref,
            None,
        )),
    }
}

fn futures_delivery_result(
    result: &FuturesDeliveryBasketResult,
    units: &UnitBindings,
    subject_ref: &VersionRef,
    funding_rule_pack: &AnalyticsObjectRef,
    annual_financing_rate: FixedDecimal,
) -> Result<pb::AnalyzeFuturesDeliveryResult, ApplicationError> {
    Ok(pb::AnalyzeFuturesDeliveryResult {
        candidates: result
            .candidates()
            .iter()
            .map(|candidate| {
                Ok(pb::FuturesDeliveryCandidateResult {
                    bond: Some(object_binding(candidate.input().bond())),
                    measures: Some(delivery_measures(
                        candidate.measures(),
                        units,
                        annual_financing_rate,
                    )?),
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?,
        ctd_index: u32::try_from(result.ctd_index()).map_err(|_| invalid())?,
        metadata: Some(metadata(
            result.ctd().schema_id(),
            ExpectedAlgorithm::futures_delivery(),
            subject_ref,
            Some(funding_rule_pack),
        )),
    })
}

fn delivery_measures(
    value: FuturesDeliveryMeasures,
    units: &UnitBindings,
    annual_financing_rate: FixedDecimal,
) -> Result<pb::FuturesDeliveryMeasures, ApplicationError> {
    let funding_adjusted_irr = value
        .implied_repo_rate()
        .checked_sub(annual_financing_rate)
        .map_err(map_domain_error)?;
    Ok(pb::FuturesDeliveryMeasures {
        months_to_next_coupon: value.months_to_next_coupon(),
        remaining_coupon_count: value.remaining_coupon_count(),
        conversion_factor: Some(decimal(value.conversion_factor(), &units.dimensionless)),
        purchase_accrued_interest: Some(decimal(
            value.purchase_accrued_interest(),
            &units.price_per_100,
        )),
        delivery_accrued_interest: Some(decimal(
            value.delivery_accrued_interest(),
            &units.price_per_100,
        )),
        interim_coupons: Some(decimal(value.interim_coupons(), &units.price_per_100)),
        invoice_price: Some(decimal(value.invoice_price(), &units.price_per_100)),
        purchase_dirty_price: Some(decimal(value.purchase_dirty_price(), &units.price_per_100)),
        gross_basis: Some(decimal(value.gross_basis(), &units.price_per_100)),
        financing_cost: Some(decimal(value.financing_cost(), &units.price_per_100)),
        holding_carry: Some(decimal(value.holding_carry(), &units.price_per_100)),
        net_basis: Some(decimal(value.net_basis(), &units.price_per_100)),
        implied_repo_rate: Some(decimal(value.implied_repo_rate(), &units.rate)),
        delivery_profit: Some(decimal(value.delivery_profit(), &units.price_per_100)),
        funding_adjusted_irr: Some(decimal(funding_adjusted_irr, &units.rate)),
    })
}

fn futures_hedge_result(
    result: &FuturesHedgeResult,
    units: &UnitBindings,
    subject_ref: &VersionRef,
) -> pb::AnalyzeFuturesHedgeResult {
    let value = result.measures();
    pb::AnalyzeFuturesHedgeResult {
        measures: Some(pb::FuturesHedgeMeasures {
            futures_contract_dv01: Some(decimal(value.futures_contract_dv01(), &units.dv01)),
            raw_contracts: Some(decimal(value.raw_contracts(), &units.contract_count)),
            recommended_contracts: value.recommended_contracts(),
            residual_dv01: Some(decimal(value.residual_dv01(), &units.dv01)),
            hedge_effectiveness: Some(decimal(value.hedge_effectiveness(), &units.dimensionless)),
        }),
        metadata: Some(metadata(
            result.schema_id(),
            ExpectedAlgorithm::futures_hedge(),
            subject_ref,
            None,
        )),
    }
}

fn object_binding(value: &AnalyticsObjectRef) -> pb::ObjectBinding {
    pb::ObjectBinding {
        object: Some(ficant_contracts::ficant::core::v1::VersionRef {
            id: Some(ficant_contracts::ficant::core::v1::Ulid {
                value: value.version_ref().id().as_str().to_owned(),
            }),
            version: value.version_ref().version().get(),
        }),
        content_hash: Some(ficant_contracts::ficant::core::v1::Sha256 {
            value: value.content_hash().as_bytes().to_vec(),
        }),
    }
}

fn metadata(
    schema_id: &str,
    algorithm: ExpectedAlgorithm,
    subject_ref: &VersionRef,
    funding_rule_pack: Option<&AnalyticsObjectRef>,
) -> pb::ResultMetadata {
    pb::ResultMetadata {
        schema_id: schema_id.to_owned(),
        engine_id: ENGINE_ID.to_owned(),
        engine_version: ENGINE_VERSION.to_owned(),
        algorithm: Some(pb::AlgorithmBinding {
            algorithm_id: algorithm.id.to_owned(),
            algorithm_version: algorithm.version,
            convention_profile: algorithm.convention.to_owned(),
            abi_version: ABI_VERSION,
        }),
        subject_ref: Some(proto_version_ref(subject_ref)),
        funding_rule_pack: funding_rule_pack.map(object_binding),
    }
}

fn proto_version_ref(value: &VersionRef) -> ficant_contracts::ficant::core::v1::VersionRef {
    ficant_contracts::ficant::core::v1::VersionRef {
        id: Some(ficant_contracts::ficant::core::v1::Ulid {
            value: value.id().as_str().to_owned(),
        }),
        version: value.version().get(),
    }
}

fn decimal(value: FixedDecimal, unit: &ProtoUnitRef) -> DecimalValue {
    let scaled = value.scaled();
    if scaled == 0 {
        return DecimalValue {
            coefficient: "0".to_owned(),
            scale: 0,
            unit: Some(unit.clone()),
        };
    }
    let mut coefficient = scaled.to_string();
    let mut scale = DECIMAL_SCALE;
    while scale > 0 && coefficient.ends_with('0') {
        coefficient.pop();
        scale -= 1;
    }
    DecimalValue {
        coefficient,
        scale,
        unit: Some(unit.clone()),
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
        PlatformFailureCode::Unavailable => (ApplicationErrorCategory::StorageUnavailable, true),
        PlatformFailureCode::Internal => (ApplicationErrorCategory::StateConflict, false),
    };
    ApplicationError::new(category, retryable)
}

fn invalid() -> ApplicationError {
    map_domain_error(DomainErrorCode::InvalidValue)
}

#[cfg(test)]
mod subject_lineage_tests {
    use super::proto_version_ref;
    use ficant_domain::primitives::{Ulid, Version, VersionRef};

    #[test]
    fn subject_version_reference_maps_without_numeric_payload() {
        let reference = VersionRef::new(
            Ulid::new("01J00000000000000000000009").unwrap(),
            Version::new(7).unwrap(),
        );
        let mapped = proto_version_ref(&reference);
        assert_eq!(mapped.id.unwrap().value, "01J00000000000000000000009");
        assert_eq!(mapped.version, 7);
    }
}
