use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ficant_application::ports::{
    AuthorizedPrincipal, Clock as ApplicationClock, DefinitionValue, IdGenerator, MarketFact,
    PORTFOLIO_READ_SCOPE, PortfolioContextInput, PortfolioCurrencyMode, PortfolioLookThroughMode,
    PortfolioPeriodPreset, PortfolioScopeSelector,
};
use ficant_application::use_cases::portfolio_aggregation::{
    PortfolioBondAnalysisResult, PortfolioRatesUnitBindings,
};
use ficant_application::use_cases::portfolio_workbench::{
    OwnedPortfolioWorkbenchBackend, PortfolioDefaultContextResult, PortfolioPageCoverage,
    PortfolioPageCoverageReason, PortfolioPageDataMode, PortfolioPageEnvelope,
    PortfolioPageProjection, PortfolioPageSelection, PortfolioPageState,
    PortfolioWorkbenchErrorCode, PortfolioWorkbenchPageId, PortfolioWorkbenchTypedError,
};
use ficant_application::use_cases::position_views::PositionViews;
use ficant_application::use_cases::rates_materialization::{
    RatesEvidenceBinding, RatesInputEvidence, RatesInputRole,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as market;
use ficant_contracts::ficant::portfolio::v1 as pb;
use ficant_contracts::ficant::portfolio::v1::portfolio_workbench_service_server::PortfolioWorkbenchService;
use ficant_contracts::ficant::rates::v1 as rates;
use ficant_contracts::ficant::research::v1 as research;
use ficant_domain::ContentAddressed;
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::{
    Bond, BondBusinessDayConvention, BondCouponFrequency, BondDayCountConvention, Calendar,
    CalendarSession, CashflowType, FactSource, FuturesContract, IncomeTaxStatus, Instrument,
    InstrumentKind, MarketRulePack, PriceSourceType, Unit, ValuationValueRole, ValueAddedTaxStatus,
    VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, FixedDecimal, MarketTime, OwnerRef, Ulid, UnitRef, VersionRef,
};
use ficant_domain::research::{
    FactorDv01, PortfolioKeyRateExposure, PositionKeyRateExposure, PriceSourceSummary,
};
use ficant_domain::{Lineaged, VersionedDefinition};
use prost_types::{Any, Timestamp};
use tonic::{Request, Response, Status};

use crate::formal_evidence::proto_formal_evidence;
use crate::grpc_web::request_credential;
use crate::market_definition::{
    decimal as domain_decimal, hash, market_time, owner, parse_market_time, parse_owner,
    parse_ulid, parse_version_ref, ulid, unit_ref, version_ref,
};
use crate::portfolio_aggregation::{coverage_declaration, normalized_context, overview};
use crate::portfolio_catalog::{catalog_page, lineage, non_formal_evidence};
use crate::registry::PlatformPort;

const FIXED_DECIMAL_SCALE: u32 = 12;

/// Owned application seam. It resolves the unique authorized owner and Subject for page requests.
#[async_trait]
pub trait PortfolioWorkbenchBackend: Send + Sync {
    async fn get_default_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        knowledge_at: MarketTime,
    ) -> Result<PortfolioDefaultContextResult, ApplicationError>;

    async fn get_page(
        &self,
        principal: &AuthorizedPrincipal,
        page_id: PortfolioWorkbenchPageId,
        input: PortfolioContextInput,
        selection: Option<PortfolioPageSelection>,
    ) -> Result<PortfolioPageEnvelope, ApplicationError>;
}

#[async_trait]
impl PortfolioWorkbenchBackend for OwnedPortfolioWorkbenchBackend {
    async fn get_default_context(
        &self,
        principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        knowledge_at: MarketTime,
    ) -> Result<PortfolioDefaultContextResult, ApplicationError> {
        self.get_default_context(principal, owner, subject_ref, knowledge_at)
            .await
    }

    async fn get_page(
        &self,
        principal: &AuthorizedPrincipal,
        page_id: PortfolioWorkbenchPageId,
        input: PortfolioContextInput,
        selection: Option<PortfolioPageSelection>,
    ) -> Result<PortfolioPageEnvelope, ApplicationError> {
        self.get_page_for_selector(principal, page_id, input, selection)
            .await
    }
}

/// Monotonic process-local ULID source for Workbench request identities.
#[derive(Debug, Default)]
pub struct SystemPortfolioRequestIdGenerator {
    sequence: AtomicU64,
}

impl IdGenerator for SystemPortfolioRequestIdGenerator {
    fn next_id(&self) -> Result<Ulid, ApplicationError> {
        let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
            ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
        })?;
        let timestamp_ms = u64::try_from(duration.as_millis()).map_err(|_| {
            ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
        })?;
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let mut seed = Vec::with_capacity(32);
        seed.extend_from_slice(&duration.as_nanos().to_be_bytes());
        seed.extend_from_slice(&sequence.to_be_bytes());
        seed.extend_from_slice(&std::process::id().to_be_bytes());
        let digest = ContentHash::digest(&seed);
        let randomness = digest.as_bytes()[..10]
            .iter()
            .fold(0_u128, |value, byte| (value << 8) | u128::from(*byte));
        Ulid::new(ulid::Ulid::from_parts(timestamp_ms, randomness).to_string())
            .map_err(map_domain_error)
    }
}

impl ApplicationClock for crate::session::SystemClock {
    fn now(&self) -> Result<MarketTime, ApplicationError> {
        let instant = chrono::Utc::now();
        MarketTime::new(instant, "UTC", instant.date_naive()).map_err(map_domain_error)
    }
}

#[derive(Clone)]
pub struct PortfolioWorkbenchGrpcService {
    identity: Arc<dyn PlatformPort>,
    backend: Arc<dyn PortfolioWorkbenchBackend>,
}

impl PortfolioWorkbenchGrpcService {
    /// Composes the Workbench transport over the typed application backend.
    ///
    /// # Errors
    ///
    /// Returns a configuration failure when the trace-signing key is too short.
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        backend: Arc<dyn PortfolioWorkbenchBackend>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        crate::core_error::CoreBusinessErrorMapper::new(trace_key)?;
        Ok(Self { identity, backend })
    }

    fn principal(
        &self,
        request: &Request<impl Sized>,
    ) -> Result<AuthorizedPrincipal, ApplicationError> {
        let session = self
            .identity
            .current_session(&request_credential(request.metadata()))
            .map_err(|_| unauthenticated())?;
        let principal = session.authorized_principal()?;
        principal.require_role(PlatformRole::Researcher)?;
        principal
            .has_scope(PORTFOLIO_READ_SCOPE)
            .then_some(principal)
            .ok_or_else(forbidden)
    }
}

#[tonic::async_trait]
impl PortfolioWorkbenchService for PortfolioWorkbenchGrpcService {
    async fn get_default_context(
        &self,
        request: Request<pb::GetDefaultContextRequest>,
    ) -> Result<Response<pb::GetDefaultContextResponse>, Status> {
        let principal = self.principal(&request).map_err(status)?;
        let value = request.get_ref();
        let owner = parse_owner(value.owner.as_ref()).map_err(status)?;
        principal.access_scope().authorize(&owner).map_err(status)?;
        let result = self
            .backend
            .get_default_context(
                &principal,
                owner,
                parse_version_ref(value.subject_ref.as_ref()).map_err(status)?,
                parse_market_time(value.knowledge_at.as_ref()).map_err(status)?,
            )
            .await
            .map_err(status)?;
        Ok(Response::new(pb::GetDefaultContextResponse {
            result: Some(match result {
                PortfolioDefaultContextResult::Context(context) => {
                    pb::get_default_context_response::Result::Context(normalized_context(&context))
                }
                PortfolioDefaultContextResult::Error(error) => {
                    pb::get_default_context_response::Result::Error(typed_error(&error))
                }
            }),
        }))
    }

    async fn get_page(
        &self,
        request: Request<pb::GetPortfolioPageRequest>,
    ) -> Result<Response<pb::PortfolioPageEnvelope>, Status> {
        let principal = self.principal(&request).map_err(status)?;
        let value = request.get_ref();
        let result = self
            .backend
            .get_page(
                &principal,
                parse_page_id(value.page_id).map_err(status)?,
                parse_context_input(value.context.as_ref()).map_err(status)?,
                value
                    .selection
                    .as_ref()
                    .map(parse_selection)
                    .transpose()
                    .map_err(status)?,
            )
            .await
            .map_err(status)?;
        Ok(Response::new(page_envelope(&result)))
    }
}

fn parse_page_id(value: i32) -> Result<PortfolioWorkbenchPageId, ApplicationError> {
    match pb::PortfolioWorkbenchPageId::try_from(value).map_err(|_| invalid())? {
        pb::PortfolioWorkbenchPageId::D01 => Ok(PortfolioWorkbenchPageId::D01),
        pb::PortfolioWorkbenchPageId::P01 => Ok(PortfolioWorkbenchPageId::P01),
        pb::PortfolioWorkbenchPageId::P02 => Ok(PortfolioWorkbenchPageId::P02),
        pb::PortfolioWorkbenchPageId::P03 => Ok(PortfolioWorkbenchPageId::P03),
        pb::PortfolioWorkbenchPageId::P04 => Ok(PortfolioWorkbenchPageId::P04),
        pb::PortfolioWorkbenchPageId::Unspecified => Err(invalid()),
    }
}

fn parse_context_input(
    value: Option<&pb::PortfolioContextInput>,
) -> Result<PortfolioContextInput, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let scope = match value.scope.as_ref().and_then(|value| value.scope.as_ref()) {
        Some(pb::portfolio_scope_selector::Scope::BookId(value)) => {
            PortfolioScopeSelector::Book(parse_ulid(Some(value))?)
        }
        Some(pb::portfolio_scope_selector::Scope::GroupId(value)) => {
            PortfolioScopeSelector::Group(parse_ulid(Some(value))?)
        }
        Some(pb::portfolio_scope_selector::Scope::PortfolioId(value)) => {
            PortfolioScopeSelector::Portfolio(parse_ulid(Some(value))?)
        }
        None => return Err(invalid()),
    };
    Ok(PortfolioContextInput {
        scope,
        valuation_at: parse_market_time(value.valuation_at.as_ref())?,
        knowledge_at: parse_market_time(value.knowledge_at.as_ref())?,
        currency: parse_currency(value.currency)?,
        look_through: parse_look_through(value.look_through)?,
        benchmark_id: parse_ulid(value.benchmark_id.as_ref())?,
        period: parse_period(value.period)?,
    })
}

fn parse_selection(
    value: &pb::PortfolioPageSelection,
) -> Result<PortfolioPageSelection, ApplicationError> {
    Ok(PortfolioPageSelection::new(parse_version_ref(
        value.instrument.as_ref(),
    )?))
}

fn parse_currency(value: i32) -> Result<PortfolioCurrencyMode, ApplicationError> {
    match pb::PortfolioCurrencyMode::try_from(value).map_err(|_| invalid())? {
        pb::PortfolioCurrencyMode::Original => Ok(PortfolioCurrencyMode::Original),
        pb::PortfolioCurrencyMode::Cny => Ok(PortfolioCurrencyMode::Cny),
        pb::PortfolioCurrencyMode::Unspecified => Err(invalid()),
    }
}

fn parse_look_through(value: i32) -> Result<PortfolioLookThroughMode, ApplicationError> {
    match pb::PortfolioLookThroughMode::try_from(value).map_err(|_| invalid())? {
        pb::PortfolioLookThroughMode::None => Ok(PortfolioLookThroughMode::None),
        pb::PortfolioLookThroughMode::Consolidated => Ok(PortfolioLookThroughMode::Consolidated),
        pb::PortfolioLookThroughMode::Separate => Ok(PortfolioLookThroughMode::Separate),
        pb::PortfolioLookThroughMode::Unspecified => Err(invalid()),
    }
}

fn parse_period(value: i32) -> Result<PortfolioPeriodPreset, ApplicationError> {
    match pb::PortfolioPeriodPreset::try_from(value).map_err(|_| invalid())? {
        pb::PortfolioPeriodPreset::OneDay => Ok(PortfolioPeriodPreset::OneDay),
        pb::PortfolioPeriodPreset::SevenDays => Ok(PortfolioPeriodPreset::SevenDays),
        pb::PortfolioPeriodPreset::ThirtyDays => Ok(PortfolioPeriodPreset::ThirtyDays),
        pb::PortfolioPeriodPreset::YearToDate => Ok(PortfolioPeriodPreset::YearToDate),
        pb::PortfolioPeriodPreset::OneYear => Ok(PortfolioPeriodPreset::OneYear),
        pb::PortfolioPeriodPreset::Unspecified => Err(invalid()),
    }
}

fn page_envelope(value: &PortfolioPageEnvelope) -> pb::PortfolioPageEnvelope {
    let catalog_read_evidence = value.provenance().and_then(|provenance| {
        provenance
            .non_formal_reads()
            .iter()
            .find(|evidence| evidence.schema_id() == "ficant.portfolio.v1.ListPortfolioCatalog")
    });
    let projection = value.projection().map(|projection| match projection {
        PortfolioPageProjection::D01(value) => {
            pb::portfolio_page_envelope::Projection::D01(pb::D01Projection {
                overview: Some(overview(value)),
            })
        }
        PortfolioPageProjection::P01(value) => {
            pb::portfolio_page_envelope::Projection::P01(pb::P01Projection {
                catalog: Some(catalog_page(&value.catalog, catalog_read_evidence)),
                structure: Some(pb::StructureMetrics {
                    book_count: value.structure.book_count,
                    group_count: value.structure.group_count,
                    portfolio_count: value.structure.portfolio_count,
                }),
            })
        }
        PortfolioPageProjection::P02(value) => {
            pb::portfolio_page_envelope::Projection::P02(pb::P02Projection {
                overview: Some(overview(value)),
            })
        }
        PortfolioPageProjection::P03(value) => {
            pb::portfolio_page_envelope::Projection::P03(pb::P03Projection {
                position_views: value.position_views.iter().map(position_views).collect(),
                key_rate_exposures: value
                    .key_rate_exposures
                    .iter()
                    .map(portfolio_key_rate_exposure)
                    .collect(),
                coverage: Some(page_coverage(&value.coverage)),
            })
        }
        PortfolioPageProjection::P04(value) => {
            pb::portfolio_page_envelope::Projection::P04(pb::P04Projection {
                definition: Some(market_definition(&value.definition)),
                facts: Some(market::InstrumentFacts {
                    facts: value.facts.iter().map(market_fact).collect(),
                    page: Some(core::PageResponse {
                        next_cursor: String::new(),
                    }),
                }),
                analysis: Some(bond_analysis(&value.analysis)),
            })
        }
    });
    pb::PortfolioPageEnvelope {
        schema_version: value.schema_version().to_owned(),
        page_id: proto_page_id(value.page_id()) as i32,
        request_id: value.request_id().to_owned(),
        generated_at: Some(timestamp(value.generated_at())),
        data_mode: proto_data_mode(value.data_mode()) as i32,
        normalized_context: value.normalized_context().map(normalized_context),
        page_state: proto_page_state(value.page_state()) as i32,
        permissions: value.permissions().to_vec(),
        provenance: value
            .provenance()
            .map(|provenance| pb::PortfolioPageProvenance {
                owner: Some(owner(provenance.owner())),
                subject_ref: Some(version_ref(provenance.subject_ref())),
                request_fingerprint: Some(hash(provenance.request_fingerprint())),
                formal_evidence: provenance
                    .formal_evidence()
                    .iter()
                    .map(proto_formal_evidence)
                    .collect(),
                non_formal_reads: provenance
                    .non_formal_reads()
                    .iter()
                    .map(non_formal_evidence)
                    .collect(),
            }),
        coverage: value.coverage().map(page_coverage),
        projection,
        typed_error: value.typed_error().map(typed_error),
    }
}

fn page_coverage(value: &PortfolioPageCoverage) -> pb::PortfolioCoverage {
    pb::PortfolioCoverage {
        participation: Some(coverage_declaration(value.participation())),
        missing_reasons: value
            .missing_reasons()
            .iter()
            .map(|reason| page_coverage_reason(*reason).to_owned())
            .collect(),
    }
}

const fn page_coverage_reason(value: PortfolioPageCoverageReason) -> &'static str {
    match value {
        PortfolioPageCoverageReason::ShortPositionExcludedFromWeightedAverages => {
            "SHORT_POSITION_EXCLUDED_FROM_WEIGHTED_AVERAGES"
        }
        PortfolioPageCoverageReason::NonBondExcludedFromWeightedAverages => {
            "NON_BOND_EXCLUDED_FROM_WEIGHTED_AVERAGES"
        }
        PortfolioPageCoverageReason::MissingBondMetricExcludedFromWeightedAverages => {
            "MISSING_BOND_METRIC_EXCLUDED_FROM_WEIGHTED_AVERAGES"
        }
        PortfolioPageCoverageReason::PositionExcludedFromPortfolioRisk => {
            "POSITION_EXCLUDED_FROM_PORTFOLIO_RISK"
        }
        PortfolioPageCoverageReason::BenchmarkPositionExcludedFromPortfolioRisk => {
            "BENCHMARK_POSITION_EXCLUDED_FROM_PORTFOLIO_RISK"
        }
        PortfolioPageCoverageReason::MissingCriticalField => "MISSING_CRITICAL_FIELD",
    }
}

fn typed_error(value: &PortfolioWorkbenchTypedError) -> pb::PortfolioWorkbenchTypedError {
    pb::PortfolioWorkbenchTypedError {
        code: match value.code() {
            PortfolioWorkbenchErrorCode::Unauthenticated => {
                pb::PortfolioWorkbenchErrorCode::Unauthenticated
            }
            PortfolioWorkbenchErrorCode::Forbidden => pb::PortfolioWorkbenchErrorCode::Forbidden,
            PortfolioWorkbenchErrorCode::NotFound => pb::PortfolioWorkbenchErrorCode::NotFound,
            PortfolioWorkbenchErrorCode::Conflict => pb::PortfolioWorkbenchErrorCode::Conflict,
            PortfolioWorkbenchErrorCode::Stale => pb::PortfolioWorkbenchErrorCode::Stale,
            PortfolioWorkbenchErrorCode::Integrity => pb::PortfolioWorkbenchErrorCode::Integrity,
            PortfolioWorkbenchErrorCode::Unavailable => {
                pb::PortfolioWorkbenchErrorCode::Unavailable
            }
        } as i32,
        safe_message: value.safe_message().to_owned(),
        trace_id: value.trace_id().to_owned(),
        retryable: value.retryable(),
    }
}

const fn proto_page_id(value: PortfolioWorkbenchPageId) -> pb::PortfolioWorkbenchPageId {
    match value {
        PortfolioWorkbenchPageId::D01 => pb::PortfolioWorkbenchPageId::D01,
        PortfolioWorkbenchPageId::P01 => pb::PortfolioWorkbenchPageId::P01,
        PortfolioWorkbenchPageId::P02 => pb::PortfolioWorkbenchPageId::P02,
        PortfolioWorkbenchPageId::P03 => pb::PortfolioWorkbenchPageId::P03,
        PortfolioWorkbenchPageId::P04 => pb::PortfolioWorkbenchPageId::P04,
    }
}

const fn proto_data_mode(value: PortfolioPageDataMode) -> pb::PortfolioPageDataMode {
    match value {
        PortfolioPageDataMode::Real => pb::PortfolioPageDataMode::Real,
        PortfolioPageDataMode::Partial => pb::PortfolioPageDataMode::Partial,
        PortfolioPageDataMode::Stale => pb::PortfolioPageDataMode::Stale,
        PortfolioPageDataMode::Error => pb::PortfolioPageDataMode::Error,
    }
}

const fn proto_page_state(value: PortfolioPageState) -> pb::PortfolioPageState {
    match value {
        PortfolioPageState::Ready => pb::PortfolioPageState::Ready,
        PortfolioPageState::Empty => pb::PortfolioPageState::Empty,
        PortfolioPageState::Blocked => pb::PortfolioPageState::Blocked,
    }
}

fn timestamp(value: &MarketTime) -> Timestamp {
    Timestamp {
        seconds: value.instant().timestamp(),
        nanos: i32::try_from(value.instant().timestamp_subsec_nanos())
            .expect("nanoseconds fit i32"),
    }
}

fn status(error: ApplicationError) -> Status {
    let category = error.category();
    drop(error);
    match category {
        ApplicationErrorCategory::Unauthenticated => {
            Status::unauthenticated("authentication required")
        }
        ApplicationErrorCategory::Forbidden => {
            Status::permission_denied("request is not authorized")
        }
        ApplicationErrorCategory::NotFound => Status::not_found("requested resource was not found"),
        ApplicationErrorCategory::ValidationFailed => {
            Status::invalid_argument("request is invalid")
        }
        ApplicationErrorCategory::StorageUnavailable => {
            Status::unavailable("service is unavailable")
        }
        _ => Status::failed_precondition("request failed closed"),
    }
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn unauthenticated() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Unauthenticated, false)
}

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

fn market_definition(value: &DefinitionValue) -> market::MarketDefinition {
    market::MarketDefinition {
        definition: Some(match value {
            DefinitionValue::Instrument(value) => {
                market::market_definition::Definition::Instrument(
                    market::CompleteInstrumentDefinition {
                        instrument: Some(instrument(value.instrument())),
                        subtype: value.subtype().map(|value| match value {
                            ficant_application::ports::InstrumentSubtype::Bond(value) => {
                                market::complete_instrument_definition::Subtype::Bond(bond(value))
                            }
                            ficant_application::ports::InstrumentSubtype::FuturesContract(
                                value,
                            ) => market::complete_instrument_definition::Subtype::FuturesContract(
                                futures(value),
                            ),
                        }),
                    },
                )
            }
            DefinitionValue::Calendar(value) => {
                market::market_definition::Definition::Calendar(calendar(value))
            }
            DefinitionValue::Unit(value) => {
                market::market_definition::Definition::Unit(definition_unit(value))
            }
            DefinitionValue::MarketRulePack(value) => {
                market::market_definition::Definition::MarketRulePack(rule_pack(value))
            }
        }),
    }
}

fn instrument(value: &Instrument) -> market::Instrument {
    market::Instrument {
        instrument_id: Some(ulid(value.id())),
        version: value.version(),
        owner: Some(owner(value.owner())),
        kind: match value.kind() {
            InstrumentKind::Bond => market::InstrumentKind::Bond,
            InstrumentKind::Futures => market::InstrumentKind::Futures,
            InstrumentKind::Other => market::InstrumentKind::Other,
        } as i32,
        market: value.market().to_owned(),
        symbol: value.symbol().to_owned(),
        currency: Some(unit_ref(value.currency())),
        calendar: Some(version_ref(value.calendar())),
    }
}

fn bond(value: &Bond) -> market::Bond {
    let tax = value
        .tax_attributes()
        .expect("complete Definition Bonds carry tax attributes");
    let pricing = value
        .pricing_terms()
        .expect("complete Definition Bonds carry pricing terms");
    market::Bond {
        instrument: Some(version_ref(value.instrument())),
        maturity_date: value.maturity_date().to_string(),
        face_value: Some(domain_decimal(value.face_value())),
        first_issue_date: value.first_issue_date().to_string(),
        current_issue_date: value.current_issue_date().to_string(),
        cumulative_issued_amount: Some(domain_decimal(value.cumulative_issued_amount())),
        tax_attributes: Some(market::BondTaxAttributes {
            value_added_tax_status: match tax.value_added_tax_status() {
                ValueAddedTaxStatus::Exempt => market::ValueAddedTaxStatus::Exempt,
                ValueAddedTaxStatus::Taxable => market::ValueAddedTaxStatus::Taxable,
            } as i32,
            income_tax_status: match tax.income_tax_status() {
                IncomeTaxStatus::Exempt => market::IncomeTaxStatus::Exempt,
                IncomeTaxStatus::Taxable => market::IncomeTaxStatus::Taxable,
            } as i32,
        }),
        coupon_rate: Some(domain_decimal(pricing.coupon_rate())),
        coupon_frequency: match pricing.frequency() {
            BondCouponFrequency::Annual => market::BondCouponFrequency::Annual,
            BondCouponFrequency::Semiannual => market::BondCouponFrequency::Semiannual,
        } as i32,
        day_count: match pricing.day_count() {
            BondDayCountConvention::ActActBondIsma => {
                market::BondDayCountConvention::ActActBondIsma
            }
        } as i32,
        business_day: match pricing.business_day() {
            BondBusinessDayConvention::Following => market::BondBusinessDayConvention::Following,
        } as i32,
    }
}

fn futures(value: &FuturesContract) -> market::FuturesContract {
    market::FuturesContract {
        instrument: Some(version_ref(value.instrument())),
        last_trade_time: Some(market_time(value.last_trade_time())),
        expiry_time: Some(market_time(value.expiry_time())),
        settlement_time: Some(market_time(value.settlement_time())),
        multiplier: Some(domain_decimal(value.multiplier())),
        rule_pack: Some(version_ref(value.rule_pack())),
        product_code: value
            .product_code()
            .expect("complete Definition Futures carry product code")
            .to_owned(),
        price_unit: Some(unit_ref(
            value
                .price_unit()
                .expect("complete Definition Futures carry price Unit"),
        )),
    }
}

fn calendar(value: &Calendar) -> market::Calendar {
    market::Calendar {
        calendar_id: Some(ulid(
            &Ulid::new(value.identity().to_owned()).expect("domain IDs are canonical"),
        )),
        version: value.version(),
        owner: Some(owner(value.owner())),
        market: value.market().to_owned(),
        market_timezone: value.market_timezone().to_owned(),
        effective_from: Some(market_time(value.effective().from())),
        effective_to: Some(market_time(value.effective().to())),
        sessions: value.sessions().iter().map(calendar_session).collect(),
    }
}

fn calendar_session(value: &CalendarSession) -> market::CalendarSession {
    market::CalendarSession {
        local_date: value.local_date().to_string(),
        open_local_time: value
            .open_local_time()
            .map_or_else(String::new, |time| time.format("%H:%M:%S").to_string()),
        close_local_time: value
            .close_local_time()
            .map_or_else(String::new, |time| time.format("%H:%M:%S").to_string()),
        closed: value.open_local_time().is_none(),
    }
}

fn definition_unit(value: &Unit) -> market::Unit {
    market::Unit {
        unit_id: Some(ulid(
            &Ulid::new(value.identity().to_owned()).expect("domain IDs are canonical"),
        )),
        version: value.version(),
        owner: Some(owner(value.owner())),
        code: value.code().to_owned(),
        dimension: value.dimension().to_owned(),
        scale: value.scale(),
        precision: value.precision(),
    }
}

fn rule_pack(value: &MarketRulePack) -> market::MarketRulePack {
    market::MarketRulePack {
        rule_pack_id: Some(ulid(
            &Ulid::new(value.identity().to_owned()).expect("domain IDs are canonical"),
        )),
        version: value.version(),
        owner: Some(owner(value.owner())),
        market: value.market().to_owned(),
        rule_type: value.rule_type().to_owned(),
        source: value.source().to_owned(),
        effective_from: Some(market_time(value.effective().from())),
        effective_to: Some(market_time(value.effective().to())),
        verification_status: match value.verification_status() {
            VerificationStatus::Unverified => market::VerificationStatus::Unverified,
            VerificationStatus::Verified => market::VerificationStatus::Verified,
            VerificationStatus::Rejected => market::VerificationStatus::Rejected,
        } as i32,
        content_hash: Some(hash(value.content_hash())),
        content: value.content().map(|value| Any {
            type_url: value.type_url().to_owned(),
            value: value.value().to_vec(),
        }),
    }
}

fn market_fact(value: &MarketFact) -> market::MarketFact {
    let fact = match value {
        MarketFact::Cashflow(value) => market::market_fact::Fact::Cashflow(market::Cashflow {
            cashflow_id: Some(ulid(value.id())),
            bond: Some(version_ref(value.bond())),
            payment_time: Some(market_time(value.payment_time())),
            amount: Some(domain_decimal(value.amount())),
            owner: Some(owner(value.owner())),
            source: Some(fact_source(value.source())),
            supersedes_id: value.supersedes_id().map(ulid),
            schedule_id: value.schedule_id().to_owned(),
            sequence: value.sequence(),
            cashflow_type: match value.cashflow_type() {
                CashflowType::Coupon => market::CashflowType::Coupon,
                CashflowType::Principal => market::CashflowType::Principal,
                CashflowType::Fee => market::CashflowType::Fee,
                CashflowType::Other => market::CashflowType::Other,
            } as i32,
        }),
        MarketFact::Quote(value) => market::market_fact::Fact::Quote(market::Quote {
            quote_id: Some(ulid(value.id())),
            instrument: Some(version_ref(value.instrument())),
            owner: Some(owner(value.owner())),
            source: Some(fact_source(value.source())),
            observed_at: Some(market_time(value.observed_at())),
            received_at: Some(market_time(value.received_at())),
            bid: value.bid().map(domain_decimal),
            ask: value.ask().map(domain_decimal),
            supersedes_id: value.supersedes_id().map(ulid),
        }),
        MarketFact::Trade(value) => market::market_fact::Fact::Trade(market::Trade {
            trade_id: Some(ulid(value.id())),
            instrument: Some(version_ref(value.instrument())),
            owner: Some(owner(value.owner())),
            source: Some(fact_source(value.source())),
            executed_at: Some(market_time(value.executed_at())),
            price: Some(domain_decimal(value.price())),
            quantity: Some(domain_decimal(value.quantity())),
            supersedes_id: value.supersedes_id().map(ulid),
        }),
        MarketFact::Valuation(value) => market::market_fact::Fact::Valuation(market::Valuation {
            valuation_id: Some(ulid(value.id())),
            instrument: Some(version_ref(value.instrument())),
            owner: Some(owner(value.owner())),
            source: Some(fact_source(value.source())),
            valuation_at: Some(market_time(value.valuation_at())),
            method: value.method().to_owned(),
            rule_pack: Some(version_ref(value.rule_pack())),
            values: value.values().iter().map(domain_decimal).collect(),
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
    market::MarketFact { fact: Some(fact) }
}

const fn proto_valuation_value_role(value: ValuationValueRole) -> market::ValuationValueRole {
    match value {
        ValuationValueRole::Price => market::ValuationValueRole::Price,
        ValuationValueRole::Yield => market::ValuationValueRole::Yield,
        ValuationValueRole::RemainingYears => market::ValuationValueRole::RemainingYears,
    }
}

fn fact_source(value: &FactSource) -> market::FactSource {
    market::FactSource {
        source_id: value.source_id().to_owned(),
        external_id: value.external_id().to_owned(),
        source_revision: value.source_revision(),
        data_source: value.data_source().map(version_ref),
    }
}

fn position_views(value: &PositionViews) -> research::PositionViews {
    research::PositionViews {
        snapshot_id: Some(ulid(value.snapshot.id())),
        content_hash: Some(hash(&value.content_hash)),
        lineage: value.snapshot.lineage().iter().map(lineage).collect(),
        positions: value
            .positions
            .iter()
            .map(|position| research::PositionView {
                position_id: Some(ulid(&position.position_id)),
                economic_value: Some(domain_decimal(&position.economic_value)),
                economic_pnl: Some(domain_decimal(&position.economic_pnl)),
                accounting_pnl: Some(domain_decimal(&position.accounting_pnl)),
                included_in_position_exposure: position.included_in_position_exposure,
                included_in_available_liquidity: position.included_in_available_liquidity,
                collateral_fact: position.collateral_fact,
            })
            .collect(),
        coverage: Some(coverage_declaration(&value.coverage)),
        formal_evidence: None,
    }
}

fn portfolio_key_rate_exposure(
    value: &PortfolioKeyRateExposure,
) -> research::PortfolioKeyRateExposure {
    research::PortfolioKeyRateExposure {
        position_snapshot_id: Some(ulid(value.position_snapshot_id())),
        curve_snapshot_id: Some(ulid(value.curve_snapshot_id())),
        positions: value.positions().iter().map(position_exposure).collect(),
        totals: value.totals().iter().map(factor_dv01).collect(),
        algorithm: Some(research::RiskAlgorithmBinding {
            algorithm_id: value.algorithm().algorithm_id().to_owned(),
            algorithm_version: value.algorithm().algorithm_version(),
            convention_profile: value.algorithm().convention_profile().to_owned(),
        }),
        content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
        futures_data_snapshot_id: value.futures_data_snapshot_id().map(ulid),
        source_confidence: Some(source_confidence(value.source_confidence())),
        coverage: Some(coverage_declaration(value.coverage())),
        formal_evidence: None,
    }
}

fn position_exposure(value: &PositionKeyRateExposure) -> research::PositionKeyRateExposure {
    research::PositionKeyRateExposure {
        position_id: Some(ulid(value.position_id())),
        instrument: Some(version_ref(value.instrument())),
        exposures: value.exposures().iter().map(factor_dv01).collect(),
        content_hash: Some(hash(value.content_hash())),
        lineage: value.lineage().iter().map(lineage).collect(),
    }
}

fn factor_dv01(value: &FactorDv01) -> research::FactorDv01 {
    research::FactorDv01 {
        factor_id: value.factor_id().to_owned(),
        factor_definition_hash: Some(hash(value.factor_definition_hash())),
        dv01: Some(core::DecimalValue {
            coefficient: value.value().scaled().to_string(),
            scale: FIXED_DECIMAL_SCALE,
            unit: Some(unit_ref(value.unit())),
        }),
    }
}

fn source_confidence(value: &PriceSourceSummary) -> research::PriceSourceSummary {
    research::PriceSourceSummary {
        counts: value
            .counts()
            .iter()
            .map(|count| research::PriceSourceCount {
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

fn bond_analysis(value: &PortfolioBondAnalysisResult) -> rates::AnalyzeBondResult {
    let units = value.units();
    let analytics = value.analytics();
    let measures = analytics.measures();
    rates::AnalyzeBondResult {
        cashflows: analytics
            .cashflows()
            .iter()
            .map(|cashflow| rates::DerivedCashflow {
                sequence: cashflow.sequence(),
                nominal_date: cashflow.nominal_date().to_string(),
                payment_date: cashflow.payment_date().to_string(),
                coupon: Some(fixed_decimal(
                    cashflow.coupon(),
                    units.currency_amount().reference(),
                )),
                principal: Some(fixed_decimal(
                    cashflow.principal(),
                    units.currency_amount().reference(),
                )),
                total: Some(fixed_decimal(
                    cashflow.total(),
                    units.currency_amount().reference(),
                )),
            })
            .collect(),
        measures: Some(rates::BondAnalyticsMeasures {
            accrued_interest: Some(fixed_decimal(
                measures.accrued_interest(),
                units.price_per_100().reference(),
            )),
            clean_price: Some(fixed_decimal(
                measures.clean_price(),
                units.price_per_100().reference(),
            )),
            dirty_price: Some(fixed_decimal(
                measures.dirty_price(),
                units.price_per_100().reference(),
            )),
            yield_to_maturity: Some(fixed_decimal(
                measures.yield_to_maturity(),
                units.rate().reference(),
            )),
            macaulay_duration: Some(fixed_decimal(
                measures.macaulay_duration(),
                units.years().reference(),
            )),
            modified_duration: Some(fixed_decimal(
                measures.modified_duration(),
                units.years().reference(),
            )),
            convexity: Some(fixed_decimal(
                measures.convexity(),
                units.years_squared().reference(),
            )),
            dv01: Some(fixed_decimal(
                measures.dv01(),
                units.dv01_per_100().reference(),
            )),
        }),
        metadata: Some(bond_metadata(value, units)),
        after_tax: None,
    }
}

fn bond_metadata(
    value: &PortfolioBondAnalysisResult,
    _units: &PortfolioRatesUnitBindings,
) -> rates::ResultMetadata {
    let metadata = value.metadata();
    let evidence = metadata.request_evidence();
    let algorithm = rates::AlgorithmBinding {
        algorithm_id: metadata.algorithm_id().to_owned(),
        algorithm_version: metadata.algorithm_version(),
        convention_profile: metadata.convention_profile().to_owned(),
        abi_version: metadata.abi_version(),
    };
    rates::ResultMetadata {
        schema_id: metadata.schema_id().to_owned(),
        engine_id: metadata.engine_id().to_owned(),
        engine_version: metadata.engine_version().to_owned(),
        algorithm: Some(algorithm.clone()),
        subject_ref: Some(version_ref(metadata.subject_ref())),
        consumed_inputs: evidence
            .consumed_inputs()
            .iter()
            .map(rates_input_evidence)
            .collect(),
        parameter_digest: Some(rates::ParameterDigest {
            algorithm: Some(algorithm),
            canonical_parameters_sha256: Some(hash(evidence.canonical_parameters_sha256())),
        }),
        request_fingerprint: Some(hash(evidence.request_fingerprint())),
        formal_evidence: metadata.formal_evidence().map(proto_formal_evidence),
    }
}

fn rates_input_evidence(value: &RatesInputEvidence) -> rates::AnalysisInputBinding {
    let binding = match value.binding() {
        RatesEvidenceBinding::Object(value) => {
            rates::analysis_input_binding::Binding::Object(rates::ObjectBinding {
                object: Some(version_ref(value.version_ref())),
                content_hash: Some(hash(value.content_hash())),
            })
        }
        RatesEvidenceBinding::Snapshot(value) => {
            rates::analysis_input_binding::Binding::Snapshot(rates::SnapshotBinding {
                snapshot_id: Some(ulid(value.id())),
                content_hash: Some(hash(value.content_hash())),
            })
        }
        RatesEvidenceBinding::Artifact(value) => {
            rates::analysis_input_binding::Binding::Artifact(rates::ArtifactBinding {
                artifact_id: Some(ulid(value.id())),
                content_hash: Some(hash(value.content_hash())),
            })
        }
        RatesEvidenceBinding::CurveNode(value) => {
            rates::analysis_input_binding::Binding::CurveNode(rates::CurveNodeBinding {
                curve_node_id: value.curve_node_id().to_owned(),
                content_hash: Some(hash(value.content_hash())),
            })
        }
    };
    rates::AnalysisInputBinding {
        role: proto_rates_role(value.role()) as i32,
        owner: Some(owner(value.owner())),
        binding: Some(binding),
        observed_at: value.observed_at().map(market_time),
        visible_at: value.visible_at().map(market_time),
        effective_from: value.effective_from().map(market_time),
        effective_to: value.effective_to().map(market_time),
    }
}

const fn proto_rates_role(value: RatesInputRole) -> rates::AnalysisInputRole {
    match value {
        RatesInputRole::Subject => rates::AnalysisInputRole::Subject,
        RatesInputRole::Unit => rates::AnalysisInputRole::Unit,
        RatesInputRole::Bond => rates::AnalysisInputRole::Bond,
        RatesInputRole::Calendar => rates::AnalysisInputRole::Calendar,
        RatesInputRole::CurveSnapshot => rates::AnalysisInputRole::CurveSnapshot,
        RatesInputRole::DataSnapshot => rates::AnalysisInputRole::DataSnapshot,
        RatesInputRole::DataSource => rates::AnalysisInputRole::DataSource,
        RatesInputRole::TaxRulePack => rates::AnalysisInputRole::TaxRulePack,
        RatesInputRole::FundingRulePack => rates::AnalysisInputRole::FundingRulePack,
        RatesInputRole::DeliveryRulePack => rates::AnalysisInputRole::DeliveryRulePack,
        RatesInputRole::FuturesContract => rates::AnalysisInputRole::FuturesContract,
        RatesInputRole::TargetRiskArtifact => rates::AnalysisInputRole::TargetRiskArtifact,
        RatesInputRole::DeliveryArtifact => rates::AnalysisInputRole::DeliveryArtifact,
        RatesInputRole::CtdAnalyticsArtifact => rates::AnalysisInputRole::CtdAnalyticsArtifact,
        RatesInputRole::CurveRulePack => rates::AnalysisInputRole::CurveRulePack,
        RatesInputRole::CurveNodeDefinition => rates::AnalysisInputRole::CurveNodeDefinition,
    }
}

fn fixed_decimal(value: FixedDecimal, unit: &UnitRef) -> core::DecimalValue {
    let scaled = value.scaled();
    if scaled == 0 {
        return core::DecimalValue {
            coefficient: "0".to_owned(),
            scale: 0,
            unit: Some(unit_ref(unit)),
        };
    }
    let mut coefficient = scaled.to_string();
    let mut scale = FIXED_DECIMAL_SCALE;
    while scale > 0 && coefficient.ends_with('0') {
        coefficient.pop();
        scale -= 1;
    }
    core::DecimalValue {
        coefficient,
        scale,
        unit: Some(unit_ref(unit)),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone, Utc};
    use ficant_domain::market::{Valuation, ValuationInput};
    use ficant_domain::primitives::{DecimalValue, Version};

    use super::*;

    #[test]
    fn p04_market_fact_projection_preserves_typed_roles_and_legacy_omission() {
        let rate = unit("01ARZ3NDEKTSV4RRFFQ69G5F91");
        let years = unit("01ARZ3NDEKTSV4RRFFQ69G5F92");
        let typed = Valuation::new_with_value_roles(
            valuation_input(
                "01ARZ3NDEKTSV4RRFFQ69G5F93",
                vec![
                    DecimalValue::new("17600000000", 12, rate).unwrap(),
                    DecimalValue::new("3482000000000", 12, years).unwrap(),
                ],
            ),
            vec![
                ValuationValueRole::Yield,
                ValuationValueRole::RemainingYears,
            ],
        )
        .unwrap();
        let projected = market_fact(&MarketFact::Valuation(typed));
        let Some(market::market_fact::Fact::Valuation(projected)) = projected.fact else {
            panic!("typed P04 fact must remain a Valuation")
        };
        assert_eq!(
            projected.value_roles,
            vec![
                market::ValuationValueRole::Yield as i32,
                market::ValuationValueRole::RemainingYears as i32,
            ]
        );

        let legacy = Valuation::new(valuation_input(
            "01ARZ3NDEKTSV4RRFFQ69G5F94",
            vec![
                DecimalValue::new("101230000000000", 12, unit("01ARZ3NDEKTSV4RRFFQ69G5F95"))
                    .unwrap(),
            ],
        ))
        .unwrap();
        let projected = market_fact(&MarketFact::Valuation(legacy));
        let Some(market::market_fact::Fact::Valuation(projected)) = projected.fact else {
            panic!("legacy P04 fact must remain a Valuation")
        };
        assert!(projected.value_roles.is_empty());
    }

    fn valuation_input(id: &str, values: Vec<DecimalValue>) -> ValuationInput {
        ValuationInput {
            valuation_id: Ulid::new(id).unwrap(),
            instrument: version("01ARZ3NDEKTSV4RRFFQ69G5F96"),
            owner: OwnerRef::new(
                Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F97").unwrap(),
                Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F98").unwrap(),
            ),
            source: FactSource::new("r8a-test", id, 1).unwrap(),
            valuation_at: MarketTime::new(
                Utc.with_ymd_and_hms(2026, 8, 21, 2, 0, 0).single().unwrap(),
                "Asia/Shanghai",
                NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
            )
            .unwrap(),
            method: "ficant.r8a.synthetic-yield-fixture.v1".to_owned(),
            rule_pack: version("01ARZ3NDEKTSV4RRFFQ69G5F99"),
            values,
            supersedes_id: None,
        }
    }

    fn unit(id: &str) -> UnitRef {
        UnitRef::new(Ulid::new(id).unwrap(), Version::new(1).unwrap())
    }

    fn version(id: &str) -> VersionRef {
        VersionRef::new(Ulid::new(id).unwrap(), Version::new(1).unwrap())
    }
}
