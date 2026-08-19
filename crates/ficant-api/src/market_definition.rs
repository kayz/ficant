use std::sync::Arc;

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use ficant_application::ports::{
    AeadCursorCodec, Cursor, DEFINITION_READ_SCOPE, DEFINITION_WRITE_SCOPE, DefinitionRepository,
    DefinitionUseCase, DefinitionValue, FoundationChangeContext, GovernedAppendDefinitionVersion,
    IdempotencyKey, InstrumentDefinition, InstrumentSubtype, PageRequest,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as pb;
use ficant_contracts::ficant::market::v1::market_definition_service_server::MarketDefinitionService;
use ficant_domain::governance::{
    ChangeJustification, FoundationResourceKind, FoundationResourceRef, PlatformRole,
    SourceDocumentRef, deterministic_change_record_id,
};
use ficant_domain::market::{
    Bond, BondBusinessDayConvention, BondCouponFrequency, BondDayCountConvention, BondPricingTerms,
    BondTaxAttributes, Calendar, CalendarInput, CalendarSession, FuturesContract, IncomeTaxStatus,
    Instrument, InstrumentInput, InstrumentKind, MarketRulePack, MarketRulePackInput,
    RulePackContent, Unit, UnitInput, ValueAddedTaxStatus, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, MarketTime, OwnerRef, Ulid, UnitRef, Version,
    VersionRef,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};
use prost_types::{Any, Timestamp};
use tonic::{Request, Response, Status};

use crate::core_error::CoreBusinessErrorMapper;
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;

const DEFAULT_PAGE_SIZE: u32 = 100;

/// Per-request authenticated transport for the governed Market Definition surface.
#[derive(Clone)]
pub struct MarketDefinitionGrpcService {
    identity: Arc<dyn PlatformPort>,
    repository: Arc<dyn DefinitionRepository>,
    cursor_codec: Arc<AeadCursorCodec>,
    errors: CoreBusinessErrorMapper,
}

impl MarketDefinitionGrpcService {
    /// Composes the governed Definition adapter.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the trace-key contract is invalid.
    pub fn new(
        identity: Arc<dyn PlatformPort>,
        repository: Arc<dyn DefinitionRepository>,
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
        required_scope: &str,
    ) -> Result<ficant_application::AuthorizedPrincipal, ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|_| forbidden())?;
        let principal = session.authorized_principal()?;
        principal
            .has_scope(required_scope)
            .then_some(principal)
            .ok_or_else(forbidden)
    }

    fn error(&self, operation: &str, error: &ApplicationError) -> core::ErrorDetail {
        self.errors
            .map(operation, "market-definition-application", error)
    }
}

#[tonic::async_trait]
impl MarketDefinitionService for MarketDefinitionGrpcService {
    async fn append_definition(
        &self,
        request: Request<pb::AppendDefinitionRequest>,
    ) -> Result<Response<pb::AppendDefinitionResponse>, Status> {
        const OPERATION: &str = "definitions.append";
        let result = match self.principal(&request, DEFINITION_WRITE_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) if principal.require_role(PlatformRole::PlatformAdmin).is_ok() => {
                let request = request.get_ref();
                parse_definition(request.definition.as_ref()).and_then(|value| {
                    let resource = FoundationResourceRef::versioned(
                        FoundationResourceKind::MarketDefinition,
                        VersionRef::new(
                            Ulid::new(value.identity().to_owned()).map_err(map_domain_error)?,
                            Version::new(value.version()).map_err(map_domain_error)?,
                        ),
                    );
                    let occurred_at = server_market_time();
                    let record_id = deterministic_change_record_id(
                        &occurred_at,
                        principal.actor_id(),
                        &resource,
                        &request.idempotency_key,
                    )
                    .map_err(map_domain_error)?;
                    GovernedAppendDefinitionVersion::new(
                        FoundationChangeContext::administrator(
                            principal,
                            parse_change(request.change.as_ref())?,
                            record_id,
                            occurred_at,
                        )?,
                        optional_version(request.expected_latest_version)?,
                        value,
                        IdempotencyKey::new(request.idempotency_key.clone())?,
                    )
                })
            }
            Ok(_) => Err(forbidden()),
        };
        let result = match result {
            Ok(command) => {
                DefinitionUseCase::new(self.repository.as_ref())
                    .append(command)
                    .await
            }
            Err(error) => Err(error),
        };
        Ok(Response::new(pb::AppendDefinitionResponse {
            result: Some(match result {
                Ok(value) => pb::append_definition_response::Result::Definition(definition(&value)),
                Err(error) => {
                    pb::append_definition_response::Result::Error(self.error(OPERATION, &error))
                }
            }),
        }))
    }

    async fn get_definition_version(
        &self,
        request: Request<pb::GetDefinitionVersionRequest>,
    ) -> Result<Response<pb::GetDefinitionVersionResponse>, Status> {
        const OPERATION: &str = "definitions.get-exact";
        let result = match self.principal(&request, DEFINITION_READ_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) => match (
                parse_ulid(request.get_ref().definition_id.as_ref()),
                Version::new(request.get_ref().version).map_err(map_domain_error),
            ) {
                (Ok(id), Ok(version)) => DefinitionUseCase::new(self.repository.as_ref())
                    .get_exact(principal.access_scope(), id, version)
                    .await
                    .and_then(|value| value.ok_or_else(not_found)),
                (Err(error), _) | (_, Err(error)) => Err(error),
            },
        };
        Ok(Response::new(pb::GetDefinitionVersionResponse {
            result: Some(match result {
                Ok(value) => {
                    pb::get_definition_version_response::Result::Definition(definition(&value))
                }
                Err(error) => pb::get_definition_version_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn resolve_definition_as_of(
        &self,
        request: Request<pb::ResolveDefinitionAsOfRequest>,
    ) -> Result<Response<pb::ResolveDefinitionAsOfResponse>, Status> {
        const OPERATION: &str = "definitions.resolve-as-of";
        let result = match self.principal(&request, DEFINITION_READ_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) => match (
                parse_ulid(request.get_ref().definition_id.as_ref()),
                parse_utc_market_time(request.get_ref().as_of.as_ref()),
            ) {
                (Ok(id), Ok(as_of)) => DefinitionUseCase::new(self.repository.as_ref())
                    .resolve_as_of(principal.access_scope(), id, as_of)
                    .await
                    .and_then(|value| value.ok_or_else(not_found)),
                (Err(error), _) | (_, Err(error)) => Err(error),
            },
        };
        Ok(Response::new(pb::ResolveDefinitionAsOfResponse {
            result: Some(match result {
                Ok(value) => {
                    pb::resolve_definition_as_of_response::Result::Definition(definition(&value))
                }
                Err(error) => pb::resolve_definition_as_of_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }

    async fn list_definition_versions(
        &self,
        request: Request<pb::ListDefinitionVersionsRequest>,
    ) -> Result<Response<pb::ListDefinitionVersionsResponse>, Status> {
        const OPERATION: &str = "definitions.list-versions";
        let result = match self.principal(&request, DEFINITION_READ_SCOPE) {
            Err(error) => Err(error),
            Ok(principal) => {
                let requested = request.get_ref().page.clone().unwrap_or_default();
                let limit = if requested.page_size == 0 {
                    DEFAULT_PAGE_SIZE
                } else {
                    requested.page_size
                };
                match (
                    parse_ulid(request.get_ref().definition_id.as_ref()),
                    parse_cursor(
                        self.cursor_codec.as_ref(),
                        principal.access_scope(),
                        &requested.cursor,
                    ),
                ) {
                    (Ok(id), Ok(cursor)) => {
                        match PageRequest::new(principal.access_scope().clone(), cursor, limit) {
                            Ok(page) => {
                                DefinitionUseCase::new(self.repository.as_ref())
                                    .list_versions(principal.access_scope(), id, page)
                                    .await
                            }
                            Err(error) => Err(error),
                        }
                    }
                    (Err(error), _) | (_, Err(error)) => Err(error),
                }
            }
        };
        Ok(Response::new(pb::ListDefinitionVersionsResponse {
            result: Some(match result {
                Ok(page) => pb::list_definition_versions_response::Result::Versions(
                    pb::DefinitionVersions {
                        definitions: page.items().iter().map(definition).collect(),
                        page: Some(core::PageResponse {
                            next_cursor: page
                                .next_cursor()
                                .map_or_else(String::new, |value| value.as_str().to_owned()),
                        }),
                    },
                ),
                Err(error) => pb::list_definition_versions_response::Result::Error(
                    self.error(OPERATION, &error),
                ),
            }),
        }))
    }
}

fn parse_definition(
    value: Option<&pb::MarketDefinition>,
) -> Result<DefinitionValue, ApplicationError> {
    match value
        .ok_or_else(invalid)?
        .definition
        .as_ref()
        .ok_or_else(invalid)?
    {
        pb::market_definition::Definition::Instrument(value) => {
            let instrument = parse_instrument(value.instrument.as_ref())?;
            let subtype = match value.subtype.as_ref() {
                Some(pb::complete_instrument_definition::Subtype::Bond(value)) => {
                    Some(InstrumentSubtype::Bond(parse_bond(value, &instrument)?))
                }
                Some(pb::complete_instrument_definition::Subtype::FuturesContract(value)) => Some(
                    InstrumentSubtype::FuturesContract(parse_futures(value, &instrument)?),
                ),
                None => None,
            };
            InstrumentDefinition::new(instrument, subtype).map(DefinitionValue::Instrument)
        }
        pb::market_definition::Definition::Calendar(value) => {
            parse_calendar(value).map(DefinitionValue::Calendar)
        }
        pb::market_definition::Definition::Unit(value) => {
            parse_unit_definition(value).map(DefinitionValue::Unit)
        }
        pb::market_definition::Definition::MarketRulePack(value) => {
            parse_rule_pack(value).map(DefinitionValue::MarketRulePack)
        }
    }
}

fn parse_instrument(value: Option<&pb::Instrument>) -> Result<Instrument, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let kind = match pb::InstrumentKind::try_from(value.kind).map_err(|_| invalid())? {
        pb::InstrumentKind::Bond => InstrumentKind::Bond,
        pb::InstrumentKind::Futures => InstrumentKind::Futures,
        pb::InstrumentKind::Other => InstrumentKind::Other,
        pb::InstrumentKind::Unspecified => return Err(invalid()),
    };
    Instrument::new(InstrumentInput {
        instrument_id: parse_ulid(value.instrument_id.as_ref())?,
        version: Version::new(value.version).map_err(map_domain_error)?,
        owner: parse_owner(value.owner.as_ref())?,
        kind,
        market: value.market.clone(),
        symbol: value.symbol.clone(),
        currency: parse_unit_ref(value.currency.as_ref())?,
        calendar: parse_version_ref(value.calendar.as_ref())?,
    })
    .map_err(map_domain_error)
}

fn parse_bond(value: &pb::Bond, instrument: &Instrument) -> Result<Bond, ApplicationError> {
    if parse_version_ref(value.instrument.as_ref())? != instrument.version_ref() {
        return Err(invalid());
    }
    let tax = value.tax_attributes.as_ref().ok_or_else(invalid)?;
    let tax = BondTaxAttributes::new(
        match pb::ValueAddedTaxStatus::try_from(tax.value_added_tax_status)
            .map_err(|_| invalid())?
        {
            pb::ValueAddedTaxStatus::Exempt => ValueAddedTaxStatus::Exempt,
            pb::ValueAddedTaxStatus::Taxable => ValueAddedTaxStatus::Taxable,
            pb::ValueAddedTaxStatus::Unspecified => return Err(invalid()),
        },
        match pb::IncomeTaxStatus::try_from(tax.income_tax_status).map_err(|_| invalid())? {
            pb::IncomeTaxStatus::Exempt => IncomeTaxStatus::Exempt,
            pb::IncomeTaxStatus::Taxable => IncomeTaxStatus::Taxable,
            pb::IncomeTaxStatus::Unspecified => return Err(invalid()),
        },
    );
    let bond = Bond::with_issuance(
        instrument,
        parse_date(&value.first_issue_date)?,
        parse_date(&value.current_issue_date)?,
        parse_date(&value.maturity_date)?,
        parse_decimal(value.cumulative_issued_amount.as_ref())?,
        tax,
        parse_decimal(value.face_value.as_ref())?,
    )
    .map_err(map_domain_error)?;
    let terms = BondPricingTerms::new(
        parse_decimal(value.coupon_rate.as_ref())?,
        match pb::BondCouponFrequency::try_from(value.coupon_frequency).map_err(|_| invalid())? {
            pb::BondCouponFrequency::Annual => BondCouponFrequency::Annual,
            pb::BondCouponFrequency::Semiannual => BondCouponFrequency::Semiannual,
            pb::BondCouponFrequency::Unspecified => return Err(invalid()),
        },
        match pb::BondDayCountConvention::try_from(value.day_count).map_err(|_| invalid())? {
            pb::BondDayCountConvention::ActActBondIsma => BondDayCountConvention::ActActBondIsma,
            pb::BondDayCountConvention::Unspecified => return Err(invalid()),
        },
        match pb::BondBusinessDayConvention::try_from(value.business_day).map_err(|_| invalid())? {
            pb::BondBusinessDayConvention::Following => BondBusinessDayConvention::Following,
            pb::BondBusinessDayConvention::Unspecified => return Err(invalid()),
        },
    )
    .map_err(map_domain_error)?;
    bond.with_pricing_terms(terms).map_err(map_domain_error)
}

fn parse_futures(
    value: &pb::FuturesContract,
    instrument: &Instrument,
) -> Result<FuturesContract, ApplicationError> {
    if parse_version_ref(value.instrument.as_ref())? != instrument.version_ref() {
        return Err(invalid());
    }
    FuturesContract::new(
        instrument,
        parse_market_time(value.last_trade_time.as_ref())?,
        parse_market_time(value.expiry_time.as_ref())?,
        parse_market_time(value.settlement_time.as_ref())?,
        parse_decimal(value.multiplier.as_ref())?,
        parse_version_ref(value.rule_pack.as_ref())?,
    )
    .and_then(|contract| {
        contract.with_risk_terms(
            value.product_code.clone(),
            parse_unit_ref(value.price_unit.as_ref())
                .map_err(|_| ficant_domain::DomainErrorCode::InvalidUnit)?,
        )
    })
    .map_err(map_domain_error)
}

pub(crate) fn parse_calendar(value: &pb::Calendar) -> Result<Calendar, ApplicationError> {
    let sessions = value
        .sessions
        .iter()
        .map(|value| {
            let date = parse_date(&value.local_date)?;
            if value.closed {
                if !value.open_local_time.is_empty() || !value.close_local_time.is_empty() {
                    return Err(invalid());
                }
                Ok(CalendarSession::closed(date))
            } else {
                CalendarSession::open(
                    date,
                    parse_time(&value.open_local_time)?,
                    parse_time(&value.close_local_time)?,
                )
                .map_err(map_domain_error)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Calendar::new(CalendarInput {
        calendar_id: parse_ulid(value.calendar_id.as_ref())?,
        version: Version::new(value.version).map_err(map_domain_error)?,
        owner: parse_owner(value.owner.as_ref())?,
        market: value.market.clone(),
        market_timezone: value.market_timezone.clone(),
        effective: EffectivePeriod::new(
            parse_market_time(value.effective_from.as_ref())?,
            parse_market_time(value.effective_to.as_ref())?,
        )
        .map_err(map_domain_error)?,
        sessions,
    })
    .map_err(map_domain_error)
}

pub(crate) fn parse_unit_definition(value: &pb::Unit) -> Result<Unit, ApplicationError> {
    Unit::new(UnitInput {
        unit_id: parse_ulid(value.unit_id.as_ref())?,
        version: Version::new(value.version).map_err(map_domain_error)?,
        owner: parse_owner(value.owner.as_ref())?,
        code: value.code.clone(),
        dimension: value.dimension.clone(),
        scale: value.scale,
        precision: value.precision,
    })
    .map_err(map_domain_error)
}

fn parse_rule_pack(value: &pb::MarketRulePack) -> Result<MarketRulePack, ApplicationError> {
    let input = MarketRulePackInput {
        rule_pack_id: parse_ulid(value.rule_pack_id.as_ref())?,
        version: Version::new(value.version).map_err(map_domain_error)?,
        owner: parse_owner(value.owner.as_ref())?,
        market: value.market.clone(),
        rule_type: value.rule_type.clone(),
        source: value.source.clone(),
        effective: EffectivePeriod::new(
            parse_market_time(value.effective_from.as_ref())?,
            parse_market_time(value.effective_to.as_ref())?,
        )
        .map_err(map_domain_error)?,
        verification_status: match pb::VerificationStatus::try_from(value.verification_status)
            .map_err(|_| invalid())?
        {
            pb::VerificationStatus::Unverified => VerificationStatus::Unverified,
            pb::VerificationStatus::Verified => VerificationStatus::Verified,
            pb::VerificationStatus::Rejected => VerificationStatus::Rejected,
            pb::VerificationStatus::Unspecified => return Err(invalid()),
        },
        content_hash: parse_hash(value.content_hash.as_ref())?,
    };
    match value.content.as_ref() {
        Some(content) => MarketRulePack::new_with_content(
            input,
            RulePackContent::new(content.type_url.clone(), content.value.clone())
                .map_err(map_domain_error)?,
        ),
        None => MarketRulePack::new(input),
    }
    .map_err(map_domain_error)
}

fn definition(value: &DefinitionValue) -> pb::MarketDefinition {
    pb::MarketDefinition {
        definition: Some(match value {
            DefinitionValue::Instrument(value) => {
                pb::market_definition::Definition::Instrument(pb::CompleteInstrumentDefinition {
                    instrument: Some(instrument(value.instrument())),
                    subtype: value.subtype().map(|value| match value {
                        InstrumentSubtype::Bond(value) => {
                            pb::complete_instrument_definition::Subtype::Bond(bond(value))
                        }
                        InstrumentSubtype::FuturesContract(value) => {
                            pb::complete_instrument_definition::Subtype::FuturesContract(futures(
                                value,
                            ))
                        }
                    }),
                })
            }
            DefinitionValue::Calendar(value) => {
                pb::market_definition::Definition::Calendar(calendar(value))
            }
            DefinitionValue::Unit(value) => pb::market_definition::Definition::Unit(unit(value)),
            DefinitionValue::MarketRulePack(value) => {
                pb::market_definition::Definition::MarketRulePack(rule_pack(value))
            }
        }),
    }
}

fn instrument(value: &Instrument) -> pb::Instrument {
    pb::Instrument {
        instrument_id: Some(ulid(value.id())),
        version: value.version(),
        owner: Some(owner(value.owner())),
        kind: match value.kind() {
            InstrumentKind::Bond => pb::InstrumentKind::Bond,
            InstrumentKind::Futures => pb::InstrumentKind::Futures,
            InstrumentKind::Other => pb::InstrumentKind::Other,
        } as i32,
        market: value.market().to_owned(),
        symbol: value.symbol().to_owned(),
        currency: Some(unit_ref(value.currency())),
        calendar: Some(version_ref(value.calendar())),
    }
}

fn bond(value: &Bond) -> pb::Bond {
    let tax = value
        .tax_attributes()
        .expect("complete Definition Bonds always carry tax attributes");
    let pricing = value
        .pricing_terms()
        .expect("complete Definition Bonds always carry pricing terms");
    pb::Bond {
        instrument: Some(version_ref(value.instrument())),
        maturity_date: value.maturity_date().to_string(),
        face_value: Some(decimal(value.face_value())),
        first_issue_date: value.first_issue_date().to_string(),
        current_issue_date: value.current_issue_date().to_string(),
        cumulative_issued_amount: Some(decimal(value.cumulative_issued_amount())),
        tax_attributes: Some(pb::BondTaxAttributes {
            value_added_tax_status: match tax.value_added_tax_status() {
                ValueAddedTaxStatus::Exempt => pb::ValueAddedTaxStatus::Exempt,
                ValueAddedTaxStatus::Taxable => pb::ValueAddedTaxStatus::Taxable,
            } as i32,
            income_tax_status: match tax.income_tax_status() {
                IncomeTaxStatus::Exempt => pb::IncomeTaxStatus::Exempt,
                IncomeTaxStatus::Taxable => pb::IncomeTaxStatus::Taxable,
            } as i32,
        }),
        coupon_rate: Some(decimal(pricing.coupon_rate())),
        coupon_frequency: match pricing.frequency() {
            BondCouponFrequency::Annual => pb::BondCouponFrequency::Annual,
            BondCouponFrequency::Semiannual => pb::BondCouponFrequency::Semiannual,
        } as i32,
        day_count: match pricing.day_count() {
            BondDayCountConvention::ActActBondIsma => pb::BondDayCountConvention::ActActBondIsma,
        } as i32,
        business_day: match pricing.business_day() {
            BondBusinessDayConvention::Following => pb::BondBusinessDayConvention::Following,
        } as i32,
    }
}

fn futures(value: &FuturesContract) -> pb::FuturesContract {
    pb::FuturesContract {
        instrument: Some(version_ref(value.instrument())),
        last_trade_time: Some(market_time(value.last_trade_time())),
        expiry_time: Some(market_time(value.expiry_time())),
        settlement_time: Some(market_time(value.settlement_time())),
        multiplier: Some(decimal(value.multiplier())),
        rule_pack: Some(version_ref(value.rule_pack())),
        product_code: value
            .product_code()
            .expect("complete Definition Futures carry risk terms")
            .to_owned(),
        price_unit: Some(unit_ref(
            value
                .price_unit()
                .expect("complete Definition Futures carry a price Unit"),
        )),
    }
}

pub(crate) fn calendar(value: &Calendar) -> pb::Calendar {
    pb::Calendar {
        calendar_id: Some(ulid(
            &Ulid::new(value.identity().to_owned()).expect("domain IDs valid"),
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

fn calendar_session(value: &CalendarSession) -> pb::CalendarSession {
    pb::CalendarSession {
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

pub(crate) fn unit(value: &Unit) -> pb::Unit {
    pb::Unit {
        unit_id: Some(ulid(
            &Ulid::new(value.identity().to_owned()).expect("domain IDs valid"),
        )),
        version: value.version(),
        owner: Some(owner(value.owner())),
        code: value.code().to_owned(),
        dimension: value.dimension().to_owned(),
        scale: value.scale(),
        precision: value.precision(),
    }
}

fn rule_pack(value: &MarketRulePack) -> pb::MarketRulePack {
    pb::MarketRulePack {
        rule_pack_id: Some(ulid(
            &Ulid::new(value.identity().to_owned()).expect("domain IDs valid"),
        )),
        version: value.version(),
        owner: Some(owner(value.owner())),
        market: value.market().to_owned(),
        rule_type: value.rule_type().to_owned(),
        source: value.source().to_owned(),
        effective_from: Some(market_time(value.effective().from())),
        effective_to: Some(market_time(value.effective().to())),
        verification_status: match value.verification_status() {
            VerificationStatus::Unverified => pb::VerificationStatus::Unverified,
            VerificationStatus::Verified => pb::VerificationStatus::Verified,
            VerificationStatus::Rejected => pb::VerificationStatus::Rejected,
        } as i32,
        content_hash: Some(hash(value.content_hash())),
        content: value.content().map(|value| Any {
            type_url: value.type_url().to_owned(),
            value: value.value().to_vec(),
        }),
    }
}

pub(crate) fn parse_change(
    value: Option<&core::ChangeJustification>,
) -> Result<ChangeJustification, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let sources = value
        .sources
        .iter()
        .map(|source| {
            SourceDocumentRef::new(source.uri.clone(), parse_hash(source.sha256.as_ref())?)
                .map_err(map_domain_error)
        })
        .collect::<Result<Vec<_>, _>>()?;
    ChangeJustification::new(value.reason.clone(), sources).map_err(map_domain_error)
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

fn optional_version(value: u64) -> Result<Option<Version>, ApplicationError> {
    if value == 0 {
        Ok(None)
    } else {
        Version::new(value).map(Some).map_err(map_domain_error)
    }
}

fn parse_date(value: &str) -> Result<NaiveDate, ApplicationError> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| invalid())?;
    (date.to_string() == value)
        .then_some(date)
        .ok_or_else(invalid)
}

fn parse_time(value: &str) -> Result<NaiveTime, ApplicationError> {
    let time = NaiveTime::parse_from_str(value, "%H:%M:%S").map_err(|_| invalid())?;
    (time.format("%H:%M:%S").to_string() == value)
        .then_some(time)
        .ok_or_else(invalid)
}

fn parse_utc_market_time(value: Option<&Timestamp>) -> Result<MarketTime, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let nanos = u32::try_from(value.nanos).map_err(|_| invalid())?;
    let instant = DateTime::<Utc>::from_timestamp(value.seconds, nanos).ok_or_else(invalid)?;
    MarketTime::new(instant, "UTC", instant.date_naive()).map_err(map_domain_error)
}

pub(crate) fn parse_market_time(
    value: Option<&core::MarketTime>,
) -> Result<MarketTime, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let timestamp = value.instant.as_ref().ok_or_else(invalid)?;
    let nanos = u32::try_from(timestamp.nanos).map_err(|_| invalid())?;
    let instant = DateTime::<Utc>::from_timestamp(timestamp.seconds, nanos).ok_or_else(invalid)?;
    MarketTime::new(
        instant,
        value.market_timezone.clone(),
        parse_date(&value.local_trading_date)?,
    )
    .map_err(map_domain_error)
}

pub(crate) fn parse_owner(value: Option<&core::OwnerRef>) -> Result<OwnerRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(OwnerRef::new(
        parse_ulid(value.tenant_id.as_ref())?,
        parse_ulid(value.owner_id.as_ref())?,
    ))
}

pub(crate) fn parse_version_ref(
    value: Option<&core::VersionRef>,
) -> Result<VersionRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(VersionRef::new(
        parse_ulid(value.id.as_ref())?,
        Version::new(value.version).map_err(map_domain_error)?,
    ))
}

pub(crate) fn parse_unit_ref(value: Option<&core::UnitRef>) -> Result<UnitRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(UnitRef::new(
        parse_ulid(value.unit_id.as_ref())?,
        Version::new(value.version).map_err(map_domain_error)?,
    ))
}

pub(crate) fn parse_decimal(
    value: Option<&core::DecimalValue>,
) -> Result<DecimalValue, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    DecimalValue::new(
        value.coefficient.clone(),
        value.scale,
        parse_unit_ref(value.unit.as_ref())?,
    )
    .map_err(map_domain_error)
}

pub(crate) fn parse_hash(value: Option<&core::Sha256>) -> Result<ContentHash, ApplicationError> {
    ContentHash::from_bytes(&value.ok_or_else(invalid)?.value).map_err(map_domain_error)
}

pub(crate) fn parse_ulid(value: Option<&core::Ulid>) -> Result<Ulid, ApplicationError> {
    Ulid::new(value.ok_or_else(invalid)?.value.clone()).map_err(map_domain_error)
}

pub(crate) fn owner(value: &OwnerRef) -> core::OwnerRef {
    core::OwnerRef {
        tenant_id: Some(ulid(value.tenant_id())),
        owner_id: Some(ulid(value.owner_id())),
    }
}

pub(crate) fn version_ref(value: &VersionRef) -> core::VersionRef {
    core::VersionRef {
        id: Some(ulid(value.id())),
        version: value.version().get(),
    }
}

pub(crate) fn unit_ref(value: &UnitRef) -> core::UnitRef {
    core::UnitRef {
        unit_id: Some(ulid(value.unit_id())),
        version: value.version().get(),
    }
}

pub(crate) fn decimal(value: &DecimalValue) -> core::DecimalValue {
    core::DecimalValue {
        coefficient: value.coefficient().to_owned(),
        scale: value.scale(),
        unit: Some(unit_ref(value.unit())),
    }
}

pub(crate) fn hash(value: &ContentHash) -> core::Sha256 {
    core::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

pub(crate) fn market_time(value: &MarketTime) -> core::MarketTime {
    core::MarketTime {
        instant: Some(Timestamp {
            seconds: value.instant().timestamp(),
            nanos: value.instant().timestamp_subsec_nanos().cast_signed(),
        }),
        market_timezone: value.market_timezone().to_owned(),
        local_trading_date: value.local_trading_date().to_string(),
    }
}

pub(crate) fn ulid(value: &Ulid) -> core::Ulid {
    core::Ulid {
        value: value.as_str().to_owned(),
    }
}

pub(crate) fn server_market_time() -> MarketTime {
    let instant = Utc::now();
    MarketTime::new(instant, "UTC", instant.date_naive())
        .expect("UTC system time is one valid MarketTime")
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
