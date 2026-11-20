use std::sync::Arc;

use chrono::{Datelike, NaiveDate, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Shanghai;
use ficant_application::ports::{
    ApplicationResult, BondAnalyticsEngine, CarryRollEngine, CouponTaxClaimScope,
    FundingRulePackParser, FuturesDeliveryEngine, FuturesDeliveryRuleParser, FuturesHedgeEngine,
    TaxRulePackParser, YieldCurveEngine,
};
use ficant_application::use_cases::{
    CalculateBondAnalytics, CalculateCarryRoll, CalculateFuturesDeliveryBasket,
    CalculateFuturesHedge, InterpolateYieldCurve, parse_verified_delivery_rule,
    parse_verified_delivery_rule_for_per_100_hedge, parse_verified_funding_rule,
    parse_verified_tax_rule,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1 as core_pb;
use ficant_contracts::ficant::market::v1 as market_pb;
use ficant_contracts::ficant::rates::v1::rates_analytics_service_server::RatesAnalyticsService;
use ficant_contracts::ficant::rates::v1::{self as pb};
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondTaxBasis, BondTerms,
    BusinessDayConvention, CalendarBinding, CalendarRequirement, CouponFrequency, DECIMAL_SCALE,
    DayCountConvention, FixedDecimal, MARKET_TIMEZONE,
};
use ficant_domain::curves::{
    CarryRollInput, YieldCurveBinding, YieldCurveInterpolation, YieldCurveNode, YieldCurveQuery,
};
use ficant_domain::futures_delivery::FuturesDeliverableInput;
use ficant_domain::futures_hedge::FuturesHedgeInput;
use ficant_domain::market::{
    BondTaxAttributes, IncomeTaxStatus, MarketRulePack, MarketRulePackInput, RulePackContent,
    ValueAddedTaxStatus, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue as DomainDecimalValue, EffectivePeriod, MarketTime, OwnerRef, Ulid,
    UnitRef, Version, VersionRef,
};
use ficant_domain::subject::{FundingTier, TaxTreatment};
use ficant_domain::{ContentAddressed, VersionedDefinition};
use tonic::{Request, Response, Status};

use crate::to_error_detail;

const DEFAULT_ULID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

pub struct RatesGrpcService {
    bond_engine: Arc<dyn BondAnalyticsEngine>,
    curve_engine: Arc<dyn YieldCurveEngine>,
    carry_roll_engine: Arc<dyn CarryRollEngine>,
    delivery_engine: Arc<dyn FuturesDeliveryEngine>,
    hedge_engine: Arc<dyn FuturesHedgeEngine>,
    delivery_rule_parser: Arc<dyn FuturesDeliveryRuleParser>,
    funding_rule_parser: Arc<dyn FundingRulePackParser>,
    tax_rule_parser: Arc<dyn TaxRulePackParser>,
}

impl RatesGrpcService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bond_engine: Arc<dyn BondAnalyticsEngine>,
        curve_engine: Arc<dyn YieldCurveEngine>,
        carry_roll_engine: Arc<dyn CarryRollEngine>,
        delivery_engine: Arc<dyn FuturesDeliveryEngine>,
        hedge_engine: Arc<dyn FuturesHedgeEngine>,
        delivery_rule_parser: Arc<dyn FuturesDeliveryRuleParser>,
        funding_rule_parser: Arc<dyn FundingRulePackParser>,
        tax_rule_parser: Arc<dyn TaxRulePackParser>,
    ) -> Self {
        Self {
            bond_engine,
            curve_engine,
            carry_roll_engine,
            delivery_engine,
            hedge_engine,
            delivery_rule_parser,
            funding_rule_parser,
            tax_rule_parser,
        }
    }

    fn execute_bond(
        &self,
        req: &pb::AnalyzeBondRequest,
    ) -> ApplicationResult<pb::AnalyzeBondResult> {
        let execution_mode =
            resolve_bond_execution_mode(req.execution_mode, req.tax_context.is_some())?;
        let units = parse_units(req.units.as_ref())?;
        let bond_pb = req.bond.as_ref().ok_or_else(invalid)?;
        let (bond_ref, terms) = parse_bond(bond_pb, &units)?;
        let settlement_date = parse_date(&req.settlement_date)?;
        let calendar_requirement = parse_calendar_requirement(req.calendar_requirement)?;
        let calendar = parse_calendar(
            req.calendar.as_ref(),
            calendar_requirement,
            settlement_date,
            terms.maturity_date(),
        )?;
        let (mode, input_value) = match req.input.as_ref() {
            Some(pb::analyze_bond_request::Input::YieldToMaturity(v)) => {
                (AnalyticsMode::YieldIn, parse_fixed_decimal(v, &units.rate)?)
            }
            Some(pb::analyze_bond_request::Input::CleanPrice(v)) => (
                AnalyticsMode::PriceIn,
                parse_fixed_decimal(v, &units.price_per_100)?,
            ),
            None => return Err(invalid()),
        };

        let input = BondAnalyticsInput::new(
            default_owner(),
            AnalyticsObjectRef::new(bond_ref, ContentHash::digest(b"inline-bond")),
            placeholder_object_ref(),
            placeholder_object_ref(),
            shanghai_noon(settlement_date)?,
            settlement_date,
            calendar_requirement,
            calendar,
            terms.clone(),
            mode,
            input_value,
        )
        .map_err(map_domain_error)?;

        let treatment = match execution_mode {
            BondTaxBasis::PreTax => None,
            BondTaxBasis::TaxAdjusted => {
                let tax_ctx = req.tax_context.as_ref().ok_or_else(invalid)?;
                let tax_attrs = terms.tax_attributes().ok_or_else(invalid)?;
                let subject_tax = parse_tax_treatment(tax_ctx.subject_tax_treatment.as_ref())?;
                let pack = parse_market_rule_pack(tax_ctx.tax_rule_pack.as_ref())?;
                Some(parse_verified_tax_rule(
                    &pack,
                    None,
                    terms.first_issue_date(),
                    tax_attrs,
                    &subject_tax,
                    self.tax_rule_parser.as_ref(),
                )?)
            }
        };

        let calc = CalculateBondAnalytics::new(self.bond_engine.as_ref());
        let (pre_tax, after_tax) =
            calc.execute_for_mode(&input, execution_mode, treatment.as_ref())?;
        Ok(pb::AnalyzeBondResult {
            cashflows: to_pb_cashflows(pre_tax.cashflows(), &units.currency_amount),
            measures: Some(to_pb_bond_measures(pre_tax.measures(), &units)),
            after_tax: after_tax.map(|adj| pb::TaxAdjustedBondAnalytics {
                cashflows: to_pb_cashflows(adj.cashflows(), &units.currency_amount),
                yield_to_maturity: Some(decimal(adj.yield_to_maturity(), &units.rate)),
                claim_scope: to_pb_claim_scope(adj.claim_scope()),
                tax_basis: to_pb_tax_basis(adj.tax_basis()),
            }),
            execution_mode: to_pb_tax_basis(execution_mode),
            measures_tax_basis: to_pb_tax_basis(pre_tax.tax_basis()),
        })
    }

    fn execute_curve(
        &self,
        req: &pb::InterpolateYieldCurveRequest,
    ) -> ApplicationResult<pb::InterpolateYieldCurveResult> {
        let units = parse_units(req.units.as_ref())?;
        let as_of = parse_date(&req.as_of_date)?;
        let query_date = parse_date(&req.query_date)?;
        let binding = parse_curve_binding(as_of, &req.nodes, &units.rate)?;
        let query = YieldCurveQuery::new(binding, query_date).map_err(map_domain_error)?;
        let point = InterpolateYieldCurve::new(self.curve_engine.as_ref()).execute(&query)?;
        Ok(pb::InterpolateYieldCurveResult {
            query_date: point.query().query_date().to_string(),
            yield_to_maturity: Some(decimal(point.yield_to_maturity(), &units.rate)),
        })
    }

    fn execute_carry_roll(
        &self,
        req: &pb::AnalyzeCarryRollRequest,
    ) -> ApplicationResult<pb::AnalyzeCarryRollResult> {
        let units = parse_units(req.units.as_ref())?;
        let bond_pb = req.bond.as_ref().ok_or_else(invalid)?;
        let (bond_ref, terms) = parse_bond(bond_pb, &units)?;
        let initial = parse_date(&req.initial_settlement)?;
        let horizon = parse_date(&req.horizon_settlement)?;
        let curve_as_of = parse_date(&req.curve_as_of_date)?;
        let calendar_requirement = parse_calendar_requirement(req.calendar_requirement)?;
        let calendar = parse_calendar(
            req.calendar.as_ref(),
            calendar_requirement,
            initial,
            terms.maturity_date(),
        )?;
        let curve = parse_curve_binding(curve_as_of, &req.curve_nodes, &units.rate)?;

        let input = CarryRollInput::new(
            default_owner(),
            AnalyticsObjectRef::new(bond_ref, ContentHash::digest(b"inline-bond")),
            placeholder_object_ref(),
            placeholder_object_ref(),
            shanghai_noon(curve_as_of)?,
            initial,
            horizon,
            calendar_requirement,
            calendar,
            terms,
            curve,
        )
        .map_err(map_domain_error)?;

        let result = CalculateCarryRoll::new(self.carry_roll_engine.as_ref()).execute(&input)?;
        let m = result.measures();
        Ok(pb::AnalyzeCarryRollResult {
            measures: Some(pb::CarryRollMeasures {
                initial_yield: Some(decimal(m.initial_yield(), &units.rate)),
                rolled_yield: Some(decimal(m.rolled_yield(), &units.rate)),
                initial_dirty_price: Some(decimal(m.initial_dirty_price(), &units.price_per_100)),
                horizon_dirty_at_initial_yield: Some(decimal(
                    m.horizon_dirty_at_initial_yield(),
                    &units.price_per_100,
                )),
                horizon_dirty_at_rolled_yield: Some(decimal(
                    m.horizon_dirty_at_rolled_yield(),
                    &units.price_per_100,
                )),
                paid_cashflows: Some(decimal(m.paid_cashflows(), &units.price_per_100)),
                carry: Some(decimal(m.carry(), &units.price_per_100)),
                roll_down: Some(decimal(m.roll_down(), &units.price_per_100)),
                total_return: Some(decimal(m.total_return(), &units.price_per_100)),
            }),
        })
    }

    #[allow(clippy::too_many_lines)]
    fn execute_delivery(
        &self,
        req: &pb::AnalyzeFuturesDeliveryRequest,
    ) -> ApplicationResult<pb::AnalyzeFuturesDeliveryResult> {
        let units = parse_units(req.units.as_ref())?;
        let product = self
            .delivery_rule_parser
            .parse_product_code(&req.product_code)?;
        let delivery_month_first = parse_date(&req.delivery_month_first)?;
        let purchase_date = parse_date(&req.purchase_date)?;
        let delivery_date = parse_date(&req.delivery_date)?;
        let futures_clean_price =
            parse_required_decimal(req.futures_clean_price.as_ref(), &units.price_per_100)?;
        let delivery_pack = parse_market_rule_pack(req.delivery_rule_pack.as_ref())?;
        let rule = parse_verified_delivery_rule(
            &delivery_pack,
            None,
            product,
            self.delivery_rule_parser.as_ref(),
        )?;

        let financing_rate = match req.financing.as_ref() {
            Some(pb::analyze_futures_delivery_request::Financing::AnnualFinancingRate(v)) => {
                parse_fixed_decimal(v, &units.rate)?
            }
            Some(pb::analyze_futures_delivery_request::Financing::SubjectFunding(ctx)) => {
                let tier = parse_funding_tier(ctx.funding_tier)?;
                let pack = parse_market_rule_pack(ctx.funding_rule_pack.as_ref())?;
                let rate = parse_verified_funding_rule(
                    &pack,
                    None,
                    tier,
                    self.funding_rule_parser.as_ref(),
                )?;
                if rate.unit() != &units.rate {
                    return Err(invalid());
                }
                rate.annual_financing_rate()
            }
            None => return Err(invalid()),
        };

        if req.candidates.is_empty() {
            return Err(invalid());
        }

        let mut inputs = Vec::with_capacity(req.candidates.len());
        let mut bond_terms_list = Vec::with_capacity(req.candidates.len());
        for cand in &req.candidates {
            let bond_pb = cand.bond.as_ref().ok_or_else(invalid)?;
            let (bond_ref, terms) = parse_bond(bond_pb, &units)?;
            let spot =
                parse_required_decimal(cand.spot_clean_price.as_ref(), &units.price_per_100)?;
            inputs.push(
                FuturesDeliverableInput::new(
                    default_owner(),
                    placeholder_object_ref(),
                    AnalyticsObjectRef::new(bond_ref, ContentHash::digest(b"inline-bond")),
                    placeholder_object_ref(),
                    placeholder_object_ref(),
                    shanghai_noon(purchase_date)?,
                    purchase_date,
                    delivery_month_first,
                    delivery_date,
                    product,
                    rule.clone(),
                    terms.clone(),
                    spot,
                    futures_clean_price,
                    financing_rate,
                )
                .map_err(map_domain_error)?,
            );
            bond_terms_list.push(terms);
        }

        let calc = CalculateFuturesDeliveryBasket::new(self.delivery_engine.as_ref());
        if let Some(tax_ctx) = req.tax_context.as_ref() {
            let subject_tax = parse_tax_treatment(tax_ctx.subject_tax_treatment.as_ref())?;
            let tax_pack = parse_market_rule_pack(tax_ctx.tax_rule_pack.as_ref())?;
            let mut treatments = Vec::with_capacity(bond_terms_list.len());
            for terms in &bond_terms_list {
                let attrs = terms.tax_attributes().ok_or_else(invalid)?;
                treatments.push(parse_verified_tax_rule(
                    &tax_pack,
                    None,
                    terms.first_issue_date(),
                    attrs,
                    &subject_tax,
                    self.tax_rule_parser.as_ref(),
                )?);
            }
            let (basket, adjustments, subject_ctd) =
                calc.execute_for_subject(&inputs, financing_rate, &treatments)?;
            let candidates = basket
                .candidates()
                .iter()
                .zip(&adjustments)
                .map(|(cand, adj)| pb::FuturesDeliveryCandidateResult {
                    bond: Some(to_pb_version_ref(cand.input().bond().version_ref())),
                    measures: Some(to_pb_delivery_measures(cand.measures(), Some(adj), &units)),
                    claim_scope: to_pb_claim_scope(adj.claim_scope),
                })
                .collect();
            Ok(pb::AnalyzeFuturesDeliveryResult {
                candidates,
                ctd_index: u32::try_from(basket.ctd_index()).map_err(|_| invalid())?,
                subject_ctd_index: Some(u32::try_from(subject_ctd).map_err(|_| invalid())?),
            })
        } else {
            let basket = calc.execute(&inputs)?;
            let candidates = basket
                .candidates()
                .iter()
                .map(|cand| pb::FuturesDeliveryCandidateResult {
                    bond: Some(to_pb_version_ref(cand.input().bond().version_ref())),
                    measures: Some(to_pb_delivery_measures(cand.measures(), None, &units)),
                    claim_scope: market_pb::CouponTaxClaimScope::Unspecified as i32,
                })
                .collect();
            Ok(pb::AnalyzeFuturesDeliveryResult {
                candidates,
                ctd_index: u32::try_from(basket.ctd_index()).map_err(|_| invalid())?,
                subject_ctd_index: None,
            })
        }
    }

    fn execute_hedge(
        &self,
        req: &pb::AnalyzeFuturesHedgeRequest,
    ) -> ApplicationResult<pb::AnalyzeFuturesHedgeResult> {
        let units = parse_units(req.units.as_ref())?;
        let product = self
            .delivery_rule_parser
            .parse_product_code(&req.product_code)?;
        let target_dv01 = parse_required_decimal(req.target_dv01.as_ref(), &units.dv01)?;
        let ctd_dv01 = parse_required_decimal(req.ctd_dv01_per_100.as_ref(), &units.dv01_per_100)?;
        let cf = parse_required_decimal(req.conversion_factor.as_ref(), &units.dimensionless)?;
        let delivery_pack = parse_strict_market_rule_pack(req.delivery_rule_pack.as_ref())?;
        let valuation_at = req
            .valuation_at
            .as_ref()
            .map(|mt| {
                let parsed = parse_market_time(Some(mt))?;
                if parsed.market_timezone() != MARKET_TIMEZONE {
                    return Err(invalid());
                }
                Ok(parsed)
            })
            .transpose()?;
        let rule = parse_verified_delivery_rule_for_per_100_hedge(
            &delivery_pack,
            valuation_at.as_ref(),
            product,
            self.delivery_rule_parser.as_ref(),
        )?;
        let contract_notional = rule
            .contract_notional()
            .map_err(map_domain_error)?
            .ok_or_else(invalid)?;
        let pack_ref = AnalyticsObjectRef::new(
            VersionRef::new(
                Ulid::new(delivery_pack.identity()).map_err(map_domain_error)?,
                Version::new(delivery_pack.version()).map_err(map_domain_error)?,
            ),
            delivery_pack.content_hash().clone(),
        );
        let input = FuturesHedgeInput::new(
            default_owner(),
            placeholder_object_ref(),
            placeholder_object_ref(),
            placeholder_object_ref(),
            placeholder_object_ref(),
            placeholder_object_ref(),
            pack_ref,
            placeholder_object_ref(),
            valuation_at,
            product,
            contract_notional,
            rule.face_quote_basis(),
            target_dv01,
            ctd_dv01,
            cf,
        )
        .map_err(map_domain_error)?;

        let result = CalculateFuturesHedge::new(self.hedge_engine.as_ref()).execute(&input)?;
        let m = result.measures();
        Ok(pb::AnalyzeFuturesHedgeResult {
            measures: Some(pb::FuturesHedgeMeasures {
                futures_contract_dv01: Some(decimal(m.futures_contract_dv01(), &units.dv01)),
                raw_contracts: Some(decimal(m.raw_contracts(), &units.contract_count)),
                recommended_contracts: m.recommended_contracts(),
                residual_dv01: Some(decimal(m.residual_dv01(), &units.dv01)),
                hedge_effectiveness: Some(decimal(m.hedge_effectiveness(), &units.dimensionless)),
                contract_notional: Some(decimal(input.contract_notional(), &units.currency_amount)),
            }),
        })
    }
}

#[tonic::async_trait]
impl RatesAnalyticsService for RatesGrpcService {
    async fn analyze_bond(
        &self,
        request: Request<pb::AnalyzeBondRequest>,
    ) -> Result<Response<pb::AnalyzeBondResponse>, Status> {
        let result = match self.execute_bond(request.get_ref()) {
            Ok(analysis) => pb::analyze_bond_response::Result::Analysis(analysis),
            Err(err) => pb::analyze_bond_response::Result::Error(to_error_detail(&err)),
        };
        Ok(Response::new(pb::AnalyzeBondResponse {
            result: Some(result),
        }))
    }

    async fn interpolate_yield_curve(
        &self,
        request: Request<pb::InterpolateYieldCurveRequest>,
    ) -> Result<Response<pb::InterpolateYieldCurveResponse>, Status> {
        let result = match self.execute_curve(request.get_ref()) {
            Ok(point) => pb::interpolate_yield_curve_response::Result::Point(point),
            Err(err) => pb::interpolate_yield_curve_response::Result::Error(to_error_detail(&err)),
        };
        Ok(Response::new(pb::InterpolateYieldCurveResponse {
            result: Some(result),
        }))
    }

    async fn analyze_carry_roll(
        &self,
        request: Request<pb::AnalyzeCarryRollRequest>,
    ) -> Result<Response<pb::AnalyzeCarryRollResponse>, Status> {
        let result = match self.execute_carry_roll(request.get_ref()) {
            Ok(analysis) => pb::analyze_carry_roll_response::Result::Analysis(analysis),
            Err(err) => pb::analyze_carry_roll_response::Result::Error(to_error_detail(&err)),
        };
        Ok(Response::new(pb::AnalyzeCarryRollResponse {
            result: Some(result),
        }))
    }

    async fn analyze_futures_delivery(
        &self,
        request: Request<pb::AnalyzeFuturesDeliveryRequest>,
    ) -> Result<Response<pb::AnalyzeFuturesDeliveryResponse>, Status> {
        let result = match self.execute_delivery(request.get_ref()) {
            Ok(analysis) => pb::analyze_futures_delivery_response::Result::Analysis(analysis),
            Err(err) => pb::analyze_futures_delivery_response::Result::Error(to_error_detail(&err)),
        };
        Ok(Response::new(pb::AnalyzeFuturesDeliveryResponse {
            result: Some(result),
        }))
    }

    async fn analyze_futures_hedge(
        &self,
        request: Request<pb::AnalyzeFuturesHedgeRequest>,
    ) -> Result<Response<pb::AnalyzeFuturesHedgeResponse>, Status> {
        let result = match self.execute_hedge(request.get_ref()) {
            Ok(analysis) => pb::analyze_futures_hedge_response::Result::Analysis(analysis),
            Err(err) => pb::analyze_futures_hedge_response::Result::Error(to_error_detail(&err)),
        };
        Ok(Response::new(pb::AnalyzeFuturesHedgeResponse {
            result: Some(result),
        }))
    }
}

struct ParsedUnits {
    currency_amount: UnitRef,
    price_per_100: UnitRef,
    rate: UnitRef,
    years: UnitRef,
    years_squared: UnitRef,
    dv01_per_100: UnitRef,
    dv01: UnitRef,
    dimensionless: UnitRef,
    contract_count: UnitRef,
}

fn parse_units(units: Option<&pb::AnalysisUnits>) -> ApplicationResult<ParsedUnits> {
    let u = units.ok_or_else(invalid)?;
    Ok(ParsedUnits {
        currency_amount: parse_unit_ref(u.currency_amount.as_ref())?,
        price_per_100: parse_unit_ref(u.price_per_100.as_ref())?,
        rate: parse_unit_ref(u.rate.as_ref())?,
        years: parse_unit_ref(u.years.as_ref())?,
        years_squared: parse_unit_ref(u.years_squared.as_ref())?,
        dv01_per_100: parse_unit_ref(u.dv01_per_100.as_ref())?,
        dv01: parse_unit_ref(u.dv01.as_ref())?,
        dimensionless: parse_unit_ref(u.dimensionless.as_ref())?,
        contract_count: parse_unit_ref(u.contract_count.as_ref())?,
    })
}

fn parse_unit_ref(u: Option<&core_pb::UnitRef>) -> ApplicationResult<UnitRef> {
    let u = u.ok_or_else(invalid)?;
    let id = u.unit_id.as_ref().ok_or_else(invalid)?;
    Ok(UnitRef::new(
        Ulid::new(id.value.clone()).map_err(map_domain_error)?,
        Version::new(u.version).map_err(map_domain_error)?,
    ))
}

fn parse_version_ref(v: Option<&core_pb::VersionRef>) -> ApplicationResult<VersionRef> {
    let v = v.ok_or_else(invalid)?;
    let id = v.id.as_ref().ok_or_else(invalid)?;
    Ok(VersionRef::new(
        Ulid::new(id.value.clone()).map_err(map_domain_error)?,
        Version::new(v.version).map_err(map_domain_error)?,
    ))
}

fn parse_bond(
    bond: &market_pb::Bond,
    units: &ParsedUnits,
) -> ApplicationResult<(VersionRef, BondTerms)> {
    let inst = parse_version_ref(bond.instrument.as_ref())?;
    let first_issue = parse_date(&bond.first_issue_date)?;
    let current_issue = if bond.current_issue_date.is_empty() {
        first_issue
    } else {
        parse_date(&bond.current_issue_date)?
    };
    let maturity = parse_date(&bond.maturity_date)?;
    let freq = match market_pb::BondCouponFrequency::try_from(bond.coupon_frequency) {
        Ok(market_pb::BondCouponFrequency::Annual) => CouponFrequency::Annual,
        Ok(market_pb::BondCouponFrequency::Semiannual) => CouponFrequency::Semiannual,
        _ => return Err(invalid()),
    };
    let dc = match market_pb::BondDayCountConvention::try_from(bond.day_count) {
        Ok(market_pb::BondDayCountConvention::ActActBondIsma) => DayCountConvention::ActActBondIsma,
        _ => return Err(invalid()),
    };
    let bdc = match market_pb::BondBusinessDayConvention::try_from(bond.business_day) {
        Ok(market_pb::BondBusinessDayConvention::Following) => BusinessDayConvention::Following,
        _ => return Err(invalid()),
    };
    let coupon_rate = parse_required_decimal(bond.coupon_rate.as_ref(), &units.rate)?;
    let face_value = parse_required_decimal(bond.face_value.as_ref(), &units.price_per_100)
        .or_else(|_| parse_required_decimal(bond.face_value.as_ref(), &units.currency_amount))?;
    let cum_issued = bond
        .cumulative_issued_amount
        .as_ref()
        .map(parse_fixed_decimal_any_unit)
        .transpose()?
        .unwrap_or(face_value);
    let tax_attributes = bond
        .tax_attributes
        .as_ref()
        .map(|tax_pb| -> ApplicationResult<BondTaxAttributes> {
            let vat = match market_pb::ValueAddedTaxStatus::try_from(tax_pb.value_added_tax_status)
            {
                Ok(market_pb::ValueAddedTaxStatus::Exempt) => ValueAddedTaxStatus::Exempt,
                Ok(market_pb::ValueAddedTaxStatus::Taxable) => ValueAddedTaxStatus::Taxable,
                _ => return Err(invalid()),
            };
            let cit = match market_pb::IncomeTaxStatus::try_from(tax_pb.income_tax_status) {
                Ok(market_pb::IncomeTaxStatus::Exempt) => IncomeTaxStatus::Exempt,
                Ok(market_pb::IncomeTaxStatus::Taxable) => IncomeTaxStatus::Taxable,
                _ => return Err(invalid()),
            };
            Ok(BondTaxAttributes::new(vat, cit))
        })
        .transpose()?;
    let terms = BondTerms::with_issuance_and_optional_tax(
        first_issue,
        current_issue,
        maturity,
        freq,
        dc,
        bdc,
        coupon_rate,
        face_value,
        cum_issued,
        tax_attributes,
    )
    .map_err(map_domain_error)?;
    Ok((inst, terms))
}

fn resolve_bond_execution_mode(
    raw_mode: i32,
    has_tax_context: bool,
) -> ApplicationResult<BondTaxBasis> {
    match pb::BondTaxBasis::try_from(raw_mode) {
        Ok(pb::BondTaxBasis::PreTax) => {
            if has_tax_context {
                Err(invalid())
            } else {
                Ok(BondTaxBasis::PreTax)
            }
        }
        Ok(pb::BondTaxBasis::TaxAdjusted) => {
            if has_tax_context {
                Ok(BondTaxBasis::TaxAdjusted)
            } else {
                Err(invalid())
            }
        }
        Ok(pb::BondTaxBasis::Unspecified) => {
            if has_tax_context {
                Ok(BondTaxBasis::TaxAdjusted)
            } else {
                Ok(BondTaxBasis::PreTax)
            }
        }
        Err(_) => Err(invalid()),
    }
}

const fn to_pb_tax_basis(basis: BondTaxBasis) -> i32 {
    match basis {
        BondTaxBasis::PreTax => pb::BondTaxBasis::PreTax as i32,
        BondTaxBasis::TaxAdjusted => pb::BondTaxBasis::TaxAdjusted as i32,
    }
}

fn parse_calendar_requirement(raw: i32) -> ApplicationResult<CalendarRequirement> {
    match pb::CalendarRequirement::try_from(raw) {
        Ok(pb::CalendarRequirement::ReferenceReplay) => Ok(CalendarRequirement::ReferenceReplay),
        Ok(pb::CalendarRequirement::ExactMarket) => Ok(CalendarRequirement::ExactMarket),
        _ => Err(invalid()),
    }
}

fn parse_calendar(
    cal: Option<&market_pb::Calendar>,
    requirement: CalendarRequirement,
    start_hint: NaiveDate,
    end_hint: NaiveDate,
) -> ApplicationResult<CalendarBinding> {
    if let Some(c) = cal {
        if requirement == CalendarRequirement::ExactMarket && c.sessions.is_empty() {
            return Err(invalid());
        }
        let id = c
            .calendar_id
            .as_ref()
            .map_or_else(|| DEFAULT_ULID.to_owned(), |u| u.value.clone());
        let ver = Version::new(c.version.max(1)).map_err(map_domain_error)?;
        let mut holidays = Vec::new();
        let mut work_weekends = Vec::new();
        let mut min_date: Option<NaiveDate> = None;
        let mut max_date: Option<NaiveDate> = None;
        for s in &c.sessions {
            let d = parse_date(&s.local_date)?;
            min_date = Some(min_date.map_or(d, |m| m.min(d)));
            max_date = Some(max_date.map_or(d, |m| m.max(d)));
            let is_weekend = matches!(d.weekday(), Weekday::Sat | Weekday::Sun);
            if s.closed && !is_weekend {
                holidays.push(d);
            } else if !s.closed && is_weekend {
                work_weekends.push(d);
            }
        }
        holidays.sort();
        holidays.dedup();
        work_weekends.sort();
        work_weekends.dedup();
        let (cov_start, cov_end) = if let (Some(min_d), Some(max_d)) = (min_date, max_date) {
            (min_d, max_d)
        } else {
            if requirement == CalendarRequirement::ExactMarket {
                return Err(invalid());
            }
            (start_hint, end_hint)
        };
        CalendarBinding::new(
            id,
            ver,
            ContentHash::digest(b"inline-calendar"),
            cov_start,
            cov_end,
            holidays,
            work_weekends,
        )
        .map_err(map_domain_error)
    } else {
        if requirement == CalendarRequirement::ExactMarket {
            return Err(invalid());
        }
        CalendarBinding::new(
            DEFAULT_ULID,
            Version::new(1).expect("nonzero"),
            ContentHash::digest(b"reference-calendar"),
            start_hint,
            end_hint,
            Vec::new(),
            Vec::new(),
        )
        .map_err(map_domain_error)
    }
}

fn parse_curve_binding(
    as_of: NaiveDate,
    nodes: &[pb::YieldCurveNodeInput],
    rate_unit: &UnitRef,
) -> ApplicationResult<YieldCurveBinding> {
    let parsed = nodes
        .iter()
        .map(|n| {
            let d = parse_date(&n.maturity_date)?;
            let y = parse_required_decimal(n.yield_to_maturity.as_ref(), rate_unit)?;
            YieldCurveNode::new(d, y).map_err(map_domain_error)
        })
        .collect::<ApplicationResult<Vec<_>>>()?;
    YieldCurveBinding::new(
        placeholder_object_ref(),
        as_of,
        YieldCurveInterpolation::LinearYield,
        parsed,
    )
    .map_err(map_domain_error)
}

fn parse_strict_market_rule_pack(
    pack: Option<&market_pb::MarketRulePack>,
) -> ApplicationResult<MarketRulePack> {
    parse_market_rule_pack_inner(pack, true)
}

fn parse_market_rule_pack(
    pack: Option<&market_pb::MarketRulePack>,
) -> ApplicationResult<MarketRulePack> {
    parse_market_rule_pack_inner(pack, false)
}

fn parse_market_rule_pack_inner(
    pack: Option<&market_pb::MarketRulePack>,
    require_content_hash: bool,
) -> ApplicationResult<MarketRulePack> {
    let p = pack.ok_or_else(invalid)?;
    let id = Ulid::new(p.rule_pack_id.as_ref().ok_or_else(invalid)?.value.clone())
        .map_err(map_domain_error)?;
    let version = Version::new(p.version).map_err(map_domain_error)?;
    let owner = p
        .owner
        .as_ref()
        .map(|o| -> ApplicationResult<OwnerRef> {
            Ok(OwnerRef::new(
                Ulid::new(o.tenant_id.as_ref().ok_or_else(invalid)?.value.clone())
                    .map_err(map_domain_error)?,
                Ulid::new(o.owner_id.as_ref().ok_or_else(invalid)?.value.clone())
                    .map_err(map_domain_error)?,
            ))
        })
        .transpose()?
        .unwrap_or_else(default_owner);
    let from = parse_market_time(p.effective_from.as_ref())?;
    let to = parse_market_time(p.effective_to.as_ref())?;
    let effective = EffectivePeriod::new(from, to).map_err(map_domain_error)?;
    let status = match market_pb::VerificationStatus::try_from(p.verification_status) {
        Ok(market_pb::VerificationStatus::Verified) => VerificationStatus::Verified,
        Ok(market_pb::VerificationStatus::Unverified) => VerificationStatus::Unverified,
        Ok(market_pb::VerificationStatus::Rejected) => VerificationStatus::Rejected,
        _ => return Err(invalid()),
    };
    let content_any = p.content.as_ref().ok_or_else(invalid)?;
    let content = RulePackContent::new(content_any.type_url.clone(), content_any.value.clone())
        .map_err(map_domain_error)?;
    let hash = match p.content_hash.as_ref() {
        Some(h) => {
            if h.value.len() != 32 {
                return Err(invalid());
            }
            ContentHash::from_bytes(&h.value).map_err(map_domain_error)?
        }
        None if require_content_hash => return Err(invalid()),
        None => ContentHash::digest(content.value()),
    };
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id,
            version,
            owner,
            market: p.market.clone(),
            rule_type: p.rule_type.clone(),
            source: p.source.clone(),
            effective,
            verification_status: status,
            content_hash: hash,
        },
        content,
    )
    .map_err(map_domain_error)
}

fn parse_tax_treatment(t: Option<&core_pb::TaxTreatment>) -> ApplicationResult<TaxTreatment> {
    let t = t.ok_or_else(invalid)?;
    TaxTreatment::new(
        t.value_added_tax_profile.clone(),
        t.income_tax_profile.clone(),
    )
    .map_err(map_domain_error)
}

fn parse_funding_tier(raw: i32) -> ApplicationResult<FundingTier> {
    match core_pb::FundingTier::try_from(raw) {
        Ok(core_pb::FundingTier::DrAvailable) => Ok(FundingTier::DrAvailable),
        Ok(core_pb::FundingTier::ROnly) => Ok(FundingTier::ROnly),
        _ => Err(invalid()),
    }
}

fn parse_market_time(mt: Option<&core_pb::MarketTime>) -> ApplicationResult<MarketTime> {
    let mt = mt.ok_or_else(invalid)?;
    let ts = mt.instant.as_ref().ok_or_else(invalid)?;
    let instant = Utc
        .timestamp_opt(ts.seconds, u32::try_from(ts.nanos).map_err(|_| invalid())?)
        .single()
        .ok_or_else(invalid)?;
    let date = parse_date(&mt.local_trading_date)?;
    MarketTime::new(instant, mt.market_timezone.clone(), date).map_err(map_domain_error)
}

fn parse_date(s: &str) -> ApplicationResult<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| invalid())
}

fn parse_required_decimal(
    value: Option<&core_pb::DecimalValue>,
    expected_unit: &UnitRef,
) -> ApplicationResult<FixedDecimal> {
    parse_fixed_decimal(value.ok_or_else(invalid)?, expected_unit)
}

fn parse_fixed_decimal(
    value: &core_pb::DecimalValue,
    expected_unit: &UnitRef,
) -> ApplicationResult<FixedDecimal> {
    let unit = parse_unit_ref(value.unit.as_ref())?;
    if &unit != expected_unit {
        return Err(invalid());
    }
    parse_fixed_decimal_any_unit(value)
}

fn parse_fixed_decimal_any_unit(value: &core_pb::DecimalValue) -> ApplicationResult<FixedDecimal> {
    let unit = parse_unit_ref(value.unit.as_ref())?;
    let canonical = DomainDecimalValue::new(value.coefficient.clone(), value.scale, unit)
        .map_err(map_domain_error)?;
    if canonical.coefficient() != value.coefficient || canonical.scale() != value.scale {
        return Err(invalid());
    }
    let coeff = value.coefficient.parse::<i128>().map_err(|_| invalid())?;
    if value.scale > DECIMAL_SCALE {
        return Err(invalid());
    }
    let factor = 10_i128
        .checked_pow(DECIMAL_SCALE - value.scale)
        .ok_or_else(invalid)?;
    let scaled = coeff.checked_mul(factor).ok_or_else(invalid)?;
    Ok(FixedDecimal::from_scaled(scaled))
}

fn decimal(value: FixedDecimal, unit: &UnitRef) -> core_pb::DecimalValue {
    let canonical =
        DomainDecimalValue::new(value.scaled().to_string(), DECIMAL_SCALE, unit.clone())
            .expect("FixedDecimal fits canonical DecimalValue");
    core_pb::DecimalValue {
        coefficient: canonical.coefficient().to_owned(),
        scale: canonical.scale(),
        unit: Some(core_pb::UnitRef {
            unit_id: Some(core_pb::Ulid {
                value: unit.unit_id().as_str().to_owned(),
            }),
            version: unit.version().get(),
        }),
    }
}

fn to_pb_cashflows(
    cfs: &[ficant_domain::analytics::DerivedCashflow],
    currency: &UnitRef,
) -> Vec<pb::DerivedCashflow> {
    cfs.iter()
        .map(|cf| pb::DerivedCashflow {
            sequence: cf.sequence(),
            nominal_date: cf.nominal_date().to_string(),
            payment_date: cf.payment_date().to_string(),
            coupon: Some(decimal(cf.coupon(), currency)),
            principal: Some(decimal(cf.principal(), currency)),
            total: Some(decimal(cf.total(), currency)),
        })
        .collect()
}

fn to_pb_bond_measures(
    m: &ficant_domain::analytics::AnalyticsMeasures,
    units: &ParsedUnits,
) -> pb::BondAnalyticsMeasures {
    pb::BondAnalyticsMeasures {
        accrued_interest: Some(decimal(m.accrued_interest(), &units.price_per_100)),
        clean_price: Some(decimal(m.clean_price(), &units.price_per_100)),
        dirty_price: Some(decimal(m.dirty_price(), &units.price_per_100)),
        yield_to_maturity: Some(decimal(m.yield_to_maturity(), &units.rate)),
        macaulay_duration: Some(decimal(m.macaulay_duration(), &units.years)),
        modified_duration: Some(decimal(m.modified_duration(), &units.years)),
        convexity: Some(decimal(m.convexity(), &units.years_squared)),
        dv01: Some(decimal(m.dv01(), &units.dv01_per_100)),
    }
}

fn to_pb_delivery_measures(
    m: ficant_domain::futures_delivery::FuturesDeliveryMeasures,
    adj: Option<&ficant_application::use_cases::SubjectCandidateMeasures>,
    units: &ParsedUnits,
) -> pb::FuturesDeliveryMeasures {
    pb::FuturesDeliveryMeasures {
        months_to_next_coupon: m.months_to_next_coupon(),
        remaining_coupon_count: m.remaining_coupon_count(),
        conversion_factor: Some(decimal(m.conversion_factor(), &units.dimensionless)),
        purchase_accrued_interest: Some(decimal(
            m.purchase_accrued_interest(),
            &units.price_per_100,
        )),
        delivery_accrued_interest: Some(decimal(
            m.delivery_accrued_interest(),
            &units.price_per_100,
        )),
        interim_coupons: Some(decimal(m.interim_coupons(), &units.price_per_100)),
        invoice_price: Some(decimal(m.invoice_price(), &units.price_per_100)),
        purchase_dirty_price: Some(decimal(m.purchase_dirty_price(), &units.price_per_100)),
        gross_basis: Some(decimal(m.gross_basis(), &units.price_per_100)),
        financing_cost: Some(decimal(m.financing_cost(), &units.price_per_100)),
        holding_carry: Some(decimal(m.holding_carry(), &units.price_per_100)),
        net_basis: Some(decimal(m.net_basis(), &units.price_per_100)),
        implied_repo_rate: Some(decimal(m.implied_repo_rate(), &units.rate)),
        delivery_profit: Some(decimal(m.delivery_profit(), &units.price_per_100)),
        funding_adjusted_irr: adj.map(|a| decimal(a.funding_adjusted_irr, &units.rate)),
        tax_adjusted_interim_coupons: adj
            .map(|a| decimal(a.tax_adjusted_interim_coupons, &units.price_per_100)),
        subject_tax_adjusted_irr: adj.map(|a| decimal(a.subject_tax_adjusted_irr, &units.rate)),
    }
}

const fn to_pb_claim_scope(scope: CouponTaxClaimScope) -> i32 {
    match scope {
        CouponTaxClaimScope::LegacySyntheticRetainedRate => {
            market_pb::CouponTaxClaimScope::Unspecified as i32
        }
        CouponTaxClaimScope::CouponOutputVatBeforeInputCredit => {
            market_pb::CouponTaxClaimScope::CouponOutputVatBeforeInputCredit as i32
        }
    }
}

fn to_pb_version_ref(v: &VersionRef) -> core_pb::VersionRef {
    core_pb::VersionRef {
        id: Some(core_pb::Ulid {
            value: v.id().as_str().to_owned(),
        }),
        version: v.version().get(),
    }
}

fn shanghai_noon(date: NaiveDate) -> ApplicationResult<MarketTime> {
    let local = Shanghai
        .from_local_datetime(&date.and_hms_opt(12, 0, 0).ok_or_else(invalid)?)
        .single()
        .ok_or_else(invalid)?;
    MarketTime::new(local.with_timezone(&Utc), MARKET_TIMEZONE, date).map_err(map_domain_error)
}

fn default_owner() -> OwnerRef {
    let id = Ulid::new(DEFAULT_ULID).expect("valid default ULID");
    OwnerRef::new(id.clone(), id)
}

fn placeholder_object_ref() -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(
            Ulid::new(DEFAULT_ULID).expect("valid default ULID"),
            Version::new(1).expect("nonzero"),
        ),
        ContentHash::digest(b"stateless-inline"),
    )
}

const fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
