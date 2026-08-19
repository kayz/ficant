use std::sync::Arc;

use chrono::{NaiveDate, Utc};
use ficant_application::ports::{
    ArtifactRepository, AuthorizedPrincipal, BondAnalyticsArtifactCodec, BondAnalyticsEngine,
    CanonicalSnapshotDecoder, CarryRollEngine, CouponTaxClaimScope, CurvePointSetDecoder,
    CurveSnapshotMetadataRepository, DataSourceRepository, DefinitionRepository,
    FactorTopologyRepository, FundingRulePackParser, FuturesDeliveryArtifactCodec,
    FuturesDeliveryEngine, FuturesDeliveryRuleParser, FuturesHedgeEngine, IntegrityEventSink,
    SnapshotVerifiedReadMetadataRepository, SubjectRepository, TaxRulePackParser,
    VerifiedBlobReader, YieldCurveEngine,
};
use ficant_application::{
    AccessScope, ApplicationError, ApplicationErrorCategory, BondRatesCommand,
    CalculateBondAnalytics, CalculateCarryRoll, CalculateFuturesDeliveryBasket,
    CalculateFuturesHedge, CarryRatesCommand, CurveRatesCommand, DeliveryRatesCommand,
    HedgeRatesCommand, ImmutableArtifactBinding, ImmutableCurveNodeBinding,
    ImmutableSnapshotBinding, MaterializeBondRatesInput, MaterializeCarryRatesInput,
    MaterializeCurveRatesInput, MaterializeDeliveryRatesInput, MaterializeHedgeRatesInput,
    RatesEvidenceBinding, RatesInputEvidence, RatesInputRole, RatesRequestEvidence,
    RatesUnitRequirement, map_analytics_error, map_domain_error,
};
use ficant_contracts::ficant::core::v1::{
    DecimalValue, OwnerRef as ProtoOwnerRef, UnitRef as ProtoUnitRef,
};
use ficant_contracts::ficant::market::v1::CouponTaxClaimScope as ProtoCouponTaxClaimScope;
use ficant_contracts::ficant::rates::v1 as pb;
use ficant_contracts::ficant::rates::v1::rates_analytics_service_server::RatesAnalyticsService;
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{
    ABI_VERSION, ALGORITHM_ID, ALGORITHM_VERSION, AnalyticsMode, AnalyticsObjectRef,
    BondAnalyticsResult, CONVENTION_PROFILE, CalendarRequirement, CouponFrequency, DECIMAL_SCALE,
    ENGINE_ID, ENGINE_VERSION, FixedDecimal, RESULT_SCHEMA_ID,
};
use ficant_domain::curves::{
    CARRY_ROLL_ALGORITHM_ID, CARRY_ROLL_ALGORITHM_VERSION, CARRY_ROLL_CONVENTION_PROFILE,
    CURVE_ALGORITHM_ID, CURVE_ALGORITHM_VERSION, CURVE_CONVENTION_PROFILE, CarryRollResult,
    YieldCurvePoint,
};
use ficant_domain::futures_delivery::{
    FUTURES_DELIVERY_ALGORITHM_ID, FUTURES_DELIVERY_ALGORITHM_VERSION,
    FUTURES_DELIVERY_CONVENTION_PROFILE, FuturesDeliveryBasketResult, FuturesDeliveryMeasures,
};
use ficant_domain::futures_hedge::{
    FUTURES_HEDGE_ALGORITHM_ID, FUTURES_HEDGE_ALGORITHM_VERSION, FUTURES_HEDGE_CONVENTION_PROFILE,
    FuturesHedgeResult,
};
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::{IncomeTaxStatus, ValueAddedTaxStatus};
use ficant_domain::primitives::{
    ContentHash, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use prost::Message;
use tonic::{Request, Response, Status};

pub use ficant_application::ports::CouponTaxTreatment;

use crate::core_error::CoreBusinessErrorMapper;
use crate::error::{PlatformFailure, PlatformFailureCode};
use crate::grpc_web::request_credential;
use crate::registry::PlatformPort;

const REQUIRED_SCOPE: &str = "rates:analyze";

#[derive(Clone)]
pub struct RatesGrpcService {
    identity: Arc<dyn PlatformPort>,
    bond: Arc<dyn BondAnalyticsEngine>,
    curve: Arc<dyn YieldCurveEngine>,
    carry_roll: Arc<dyn CarryRollEngine>,
    futures_delivery: Arc<dyn FuturesDeliveryEngine>,
    futures_hedge: Arc<dyn FuturesHedgeEngine>,
    definitions: Arc<dyn DefinitionRepository>,
    subjects: Arc<dyn SubjectRepository>,
    data_sources: Arc<dyn DataSourceRepository>,
    snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
    curve_snapshots: Arc<dyn CurveSnapshotMetadataRepository>,
    factors: Arc<dyn FactorTopologyRepository>,
    artifacts: Arc<dyn ArtifactRepository>,
    blobs: Arc<dyn VerifiedBlobReader>,
    integrity_events: Arc<dyn IntegrityEventSink>,
    snapshot_decoder: Arc<dyn CanonicalSnapshotDecoder>,
    curve_decoder: Arc<dyn CurvePointSetDecoder>,
    delivery_parser: Arc<dyn FuturesDeliveryRuleParser>,
    funding_parser: Arc<dyn FundingRulePackParser>,
    tax_parser: Arc<dyn TaxRulePackParser>,
    bond_codec: Arc<dyn BondAnalyticsArtifactCodec>,
    delivery_codec: Arc<dyn FuturesDeliveryArtifactCodec>,
    errors: CoreBusinessErrorMapper,
}

impl RatesGrpcService {
    /// Canonicalizes the metadata carried by the private materialized Bond port.
    ///
    /// The caller supplies only Application-built consumed-input evidence. This function binds
    /// that evidence to the public request, the fully materialized numerical input and the
    /// resolved coupon-tax scalar, then derives the fixed schema, parameter digest and request
    /// fingerprint used by execution.
    ///
    /// # Errors
    ///
    /// Returns a validation failure when the request, materialized facts, role closure, ordering,
    /// identities or time evidence are inconsistent.
    pub fn canonical_materialized_bond_metadata(
        request: &pb::AnalyzeBondRequest,
        input: &ficant_domain::analytics::BondAnalyticsInput,
        coupon_tax_treatment: &CouponTaxTreatment,
        consumed_inputs: &[pb::AnalysisInputBinding],
    ) -> Result<pb::ResultMetadata, ApplicationError> {
        materialized_bond_proof(request, input, coupon_tax_treatment, consumed_inputs)
            .map(|(_, metadata)| metadata)
    }

    /// Canonicalizes one private v2 treatment wire into the provider-neutral proof used by the
    /// native two-port seam.
    ///
    /// # Errors
    ///
    /// Rejects unknown enums, non-canonical decimals, unit drift, profile drift, or a missing /
    /// incorrect authority semantic hash.
    pub fn canonical_v2_coupon_tax_treatment(
        input: &ficant_domain::analytics::BondAnalyticsInput,
        treatment: &ficant_contracts::ficant::market::v1::SubjectCouponTaxTreatment,
        authority_semantic_hash: &[u8],
    ) -> Result<CouponTaxTreatment, ApplicationError> {
        use ficant_contracts::ficant::market::v1::{
            CouponTaxClaimScope, GrossCouponTaxBasis, TaxRoundingMode,
        };
        if treatment.value_added_tax_profile != "cn-vat-general-taxpayer"
            || treatment.income_tax_profile != "cn-cgb-interest-cit-exempt"
            || GrossCouponTaxBasis::try_from(treatment.gross_coupon_basis).map_err(|_| invalid())?
                != GrossCouponTaxBasis::VatIncluded
            || TaxRoundingMode::try_from(treatment.rounding).map_err(|_| invalid())?
                != TaxRoundingMode::TiesToEven
            || CouponTaxClaimScope::try_from(treatment.claim_scope).map_err(|_| invalid())?
                != CouponTaxClaimScope::CouponOutputVatBeforeInputCredit
        {
            return Err(invalid());
        }
        let vat = treatment
            .value_added_tax_rate
            .as_ref()
            .ok_or_else(invalid)?;
        let income = treatment.income_tax_rate.as_ref().ok_or_else(invalid)?;
        let vat_unit = vat.unit.as_ref().ok_or_else(invalid)?;
        if income.unit.as_ref() != Some(vat_unit)
            || vat_unit
                .unit_id
                .as_ref()
                .is_none_or(|value| value.value != "01K2CGBVAT0000000000000000")
            || vat_unit.version != 1
        {
            return Err(invalid());
        }
        let unit = parse_unit(vat_unit)?;
        let vat = parse_fixed_decimal(vat, vat_unit)?;
        let income = parse_fixed_decimal(income, vat_unit)?;
        let attributes = input.terms().tax_attributes().ok_or_else(invalid)?;
        let cutoff = NaiveDate::from_ymd_opt(2025, 8, 8).ok_or_else(invalid)?;
        let (expected_attributes, expected_vat) = if input.terms().first_issue_date() < cutoff {
            (
                ficant_domain::market::BondTaxAttributes::new(
                    ValueAddedTaxStatus::Exempt,
                    IncomeTaxStatus::Exempt,
                ),
                FixedDecimal::ZERO,
            )
        } else {
            (
                ficant_domain::market::BondTaxAttributes::new(
                    ValueAddedTaxStatus::Taxable,
                    IncomeTaxStatus::Exempt,
                ),
                FixedDecimal::from_scaled(60_000_000_000),
            )
        };
        if attributes != expected_attributes || vat != expected_vat || income != FixedDecimal::ZERO
        {
            return Err(invalid());
        }
        let semantic =
            ContentHash::from_bytes(authority_semantic_hash).map_err(map_domain_error)?;
        if semantic.as_bytes()
            != &[
                0x54, 0xfa, 0x5a, 0xdb, 0xeb, 0x8b, 0x16, 0x4d, 0xc7, 0x79, 0xec, 0xc2, 0x50, 0xab,
                0x62, 0x2a, 0xb5, 0x74, 0xcd, 0xeb, 0x36, 0xf2, 0xb6, 0xda, 0x58, 0xf4, 0xd8, 0x77,
                0xce, 0x51, 0x06, 0x0a,
            ]
        {
            return Err(invalid());
        }
        CouponTaxTreatment::vat_included(vat, income, unit, semantic)
    }

    /// Executes the native private port from its generated v2 treatment wire.
    ///
    /// # Errors
    ///
    /// Validates the wire proof before any numerical engine invocation.
    pub fn execute_materialized_v2_bond_request(
        engine: &dyn BondAnalyticsEngine,
        request: &pb::AnalyzeBondRequest,
        input: &ficant_domain::analytics::BondAnalyticsInput,
        treatment: &ficant_contracts::ficant::market::v1::SubjectCouponTaxTreatment,
        authority_semantic_hash: &[u8],
        supplied_metadata: pb::ResultMetadata,
    ) -> Result<pb::AnalyzeBondResult, ApplicationError> {
        let treatment =
            Self::canonical_v2_coupon_tax_treatment(input, treatment, authority_semantic_hash)?;
        Self::execute_materialized_bond_request(
            engine,
            request,
            input,
            &treatment,
            supplied_metadata,
        )
    }

    /// Constructs the complete R5D production dependency slice.
    ///
    /// # Errors
    ///
    /// Returns an error when the trace-key configuration is invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_materialization(
        identity: Arc<dyn PlatformPort>,
        bond: Arc<dyn BondAnalyticsEngine>,
        curve: Arc<dyn YieldCurveEngine>,
        carry_roll: Arc<dyn CarryRollEngine>,
        futures_delivery: Arc<dyn FuturesDeliveryEngine>,
        definitions: Arc<dyn DefinitionRepository>,
        subjects: Arc<dyn SubjectRepository>,
        delivery_parser: Arc<dyn FuturesDeliveryRuleParser>,
        snapshots: Arc<dyn SnapshotVerifiedReadMetadataRepository>,
        blobs: Arc<dyn VerifiedBlobReader>,
        integrity_events: Arc<dyn IntegrityEventSink>,
        snapshot_decoder: Arc<dyn CanonicalSnapshotDecoder>,
        funding_parser: Arc<dyn FundingRulePackParser>,
        tax_parser: Arc<dyn TaxRulePackParser>,
        futures_hedge: Arc<dyn FuturesHedgeEngine>,
        data_sources: Arc<dyn DataSourceRepository>,
        curve_snapshots: Arc<dyn CurveSnapshotMetadataRepository>,
        factors: Arc<dyn FactorTopologyRepository>,
        artifacts: Arc<dyn ArtifactRepository>,
        curve_decoder: Arc<dyn CurvePointSetDecoder>,
        bond_codec: Arc<dyn BondAnalyticsArtifactCodec>,
        delivery_codec: Arc<dyn FuturesDeliveryArtifactCodec>,
        trace_key: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            identity,
            bond,
            curve,
            carry_roll,
            futures_delivery,
            futures_hedge,
            definitions,
            subjects,
            data_sources,
            snapshots,
            curve_snapshots,
            factors,
            artifacts,
            blobs,
            integrity_events,
            snapshot_decoder,
            curve_decoder,
            delivery_parser,
            funding_parser,
            tax_parser,
            bond_codec,
            delivery_codec,
            errors: CoreBusinessErrorMapper::new(trace_key)?,
        })
    }

    fn authorize(
        &self,
        request: &Request<impl Sized>,
    ) -> Result<AuthorizedPrincipal, ApplicationError> {
        let credential = request_credential(request.metadata());
        let session = self
            .identity
            .current_session(&credential)
            .map_err(|failure| platform_application_error(&failure))?;
        let principal = session.authorized_principal()?;
        principal.require_role(PlatformRole::Researcher)?;
        principal
            .has_scope(REQUIRED_SCOPE)
            .then_some(principal)
            .ok_or_else(forbidden)
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
        scope: &AccessScope,
    ) -> Result<pb::AnalyzeBondResult, ApplicationError> {
        let context = parse_context(request.context.as_ref(), ExpectedAlgorithm::bond())?;
        scope.authorize(&context.owner)?;
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
        let materialized = MaterializeBondRatesInput::new(
            self.definitions.as_ref(),
            self.subjects.as_ref(),
            self.snapshots.as_ref(),
            self.blobs.as_ref(),
            self.integrity_events.as_ref(),
            self.tax_parser.as_ref(),
        )
        .execute(
            scope,
            BondRatesCommand {
                owner: context.owner.clone(),
                subject_ref: context.subject_ref.clone(),
                units: context.units.requirements()?,
                currency_unit: parse_unit(&context.units.currency_amount)?,
                rate_unit: parse_unit(&context.units.rate)?,
                knowledge_at: context.knowledge_at.clone(),
                bond: parse_object(request.bond.as_ref())?,
                calendar: parse_object(request.calendar.as_ref())?,
                data_snapshot: parse_snapshot(request.data_snapshot.as_ref())?,
                tax_rule_pack: parse_object(request.tax_rule_pack.as_ref())?,
                valuation_at: parse_market_time(request.valuation_at.as_ref())?,
                settlement_date: parse_date(&request.settlement_date)?,
                calendar_requirement: parse_calendar_requirement(request.calendar_requirement)?,
                mode,
                input_value,
            },
            trace_context(request),
        )
        .await?;
        if materialized.coupon_tax_treatment().unit() != &parse_unit(&context.units.rate)? {
            return Err(map_domain_error(DomainErrorCode::InvalidUnit));
        }
        let pre_tax =
            CalculateBondAnalytics::new(self.bond.as_ref()).execute(materialized.input())?;
        let terms = materialized
            .input()
            .terms()
            .with_coupon_rate(tax_adjusted_coupon_rate(
                materialized.input(),
                &pre_tax,
                materialized.coupon_tax_treatment(),
            )?)
            .map_err(map_domain_error)?;
        let after_tax_input = materialized
            .input()
            .with_terms_and_price_in(terms, pre_tax.measures().clean_price())
            .map_err(map_domain_error)?;
        let after_tax =
            CalculateBondAnalytics::new(self.bond.as_ref()).execute(&after_tax_input)?;
        bond_result(
            &pre_tax,
            Some((&after_tax, materialized.coupon_tax_treatment())),
            &context,
            materialized.evidence(),
        )
    }

    async fn interpolate_curve_value(
        &self,
        request: &pb::InterpolateYieldCurveRequest,
        scope: &AccessScope,
    ) -> Result<pb::InterpolateYieldCurveResult, ApplicationError> {
        let context = parse_context(request.context.as_ref(), ExpectedAlgorithm::curve())?;
        scope.authorize(&context.owner)?;
        let materialized = MaterializeCurveRatesInput::new(
            self.definitions.as_ref(),
            self.subjects.as_ref(),
            self.data_sources.as_ref(),
            self.snapshots.as_ref(),
            self.curve_snapshots.as_ref(),
            self.blobs.as_ref(),
            self.integrity_events.as_ref(),
            self.curve_decoder.as_ref(),
            self.factors.as_ref(),
        )
        .execute(
            scope,
            CurveRatesCommand {
                owner: context.owner.clone(),
                subject_ref: context.subject_ref.clone(),
                units: context.units.requirements()?,
                currency_unit: parse_unit(&context.units.currency_amount)?,
                rate_unit: parse_unit(&context.units.rate)?,
                knowledge_at: context.knowledge_at.clone(),
                curve: parse_snapshot(request.curve.as_ref())?,
                query_date: parse_date(&request.query_date)?,
            },
            trace_context(request),
        )
        .await?;
        let point = self
            .curve
            .interpolate(materialized.query())
            .map_err(map_analytics_error)?;
        point
            .validate_against(materialized.query())
            .map_err(map_domain_error)?;
        Ok(curve_result(&point, &context, materialized.evidence()))
    }

    async fn analyze_carry_roll_value(
        &self,
        request: &pb::AnalyzeCarryRollRequest,
        scope: &AccessScope,
    ) -> Result<pb::AnalyzeCarryRollResult, ApplicationError> {
        let context = parse_context(request.context.as_ref(), ExpectedAlgorithm::carry_roll())?;
        scope.authorize(&context.owner)?;
        let materialized = MaterializeCarryRatesInput::new(
            self.definitions.as_ref(),
            self.subjects.as_ref(),
            self.data_sources.as_ref(),
            self.snapshots.as_ref(),
            self.curve_snapshots.as_ref(),
            self.blobs.as_ref(),
            self.integrity_events.as_ref(),
            self.curve_decoder.as_ref(),
            self.factors.as_ref(),
        )
        .execute(
            scope,
            CarryRatesCommand {
                owner: context.owner.clone(),
                subject_ref: context.subject_ref.clone(),
                units: context.units.requirements()?,
                currency_unit: parse_unit(&context.units.currency_amount)?,
                rate_unit: parse_unit(&context.units.rate)?,
                knowledge_at: context.knowledge_at.clone(),
                bond: parse_object(request.bond.as_ref())?,
                curve: parse_snapshot(request.curve.as_ref())?,
                valuation_at: parse_market_time(request.valuation_at.as_ref())?,
                initial_settlement: parse_date(&request.initial_settlement)?,
                horizon_settlement: parse_date(&request.horizon_settlement)?,
                calendar_requirement: parse_calendar_requirement(request.calendar_requirement)?,
            },
            trace_context(request),
        )
        .await?;
        let result =
            CalculateCarryRoll::new(self.carry_roll.as_ref()).execute(materialized.input())?;
        Ok(carry_roll_result(
            &result,
            &context,
            materialized.evidence(),
        ))
    }

    async fn analyze_futures_delivery_value(
        &self,
        request: &pb::AnalyzeFuturesDeliveryRequest,
        scope: &AccessScope,
    ) -> Result<pb::AnalyzeFuturesDeliveryResult, ApplicationError> {
        let context = parse_context(
            request.context.as_ref(),
            ExpectedAlgorithm::futures_delivery(),
        )?;
        scope.authorize(&context.owner)?;
        let materialized = MaterializeDeliveryRatesInput::new(
            self.definitions.as_ref(),
            self.subjects.as_ref(),
            self.data_sources.as_ref(),
            self.snapshots.as_ref(),
            self.blobs.as_ref(),
            self.integrity_events.as_ref(),
            self.snapshot_decoder.as_ref(),
            self.delivery_parser.as_ref(),
            self.funding_parser.as_ref(),
            self.tax_parser.as_ref(),
        )
        .execute(
            scope,
            DeliveryRatesCommand {
                owner: context.owner.clone(),
                subject_ref: context.subject_ref.clone(),
                units: context.units.requirements()?,
                currency_unit: parse_unit(&context.units.currency_amount)?,
                price_unit: parse_unit(&context.units.price_per_100)?,
                rate_unit: parse_unit(&context.units.rate)?,
                knowledge_at: context.knowledge_at.clone(),
                futures_contract: parse_object(request.futures_contract.as_ref())?,
                data_snapshot: parse_snapshot(request.data_snapshot.as_ref())?,
                funding_rule_pack: parse_object(request.funding_rule_pack.as_ref())?,
                tax_rule_pack: parse_object(request.tax_rule_pack.as_ref())?,
                valuation_at: parse_market_time(request.valuation_at.as_ref())?,
                purchase_date: parse_date(&request.purchase_date)?,
            },
            trace_context(request),
        )
        .await?;
        if materialized.funding_rate().unit() != &parse_unit(&context.units.rate)? {
            return Err(map_domain_error(DomainErrorCode::InvalidUnit));
        }
        let result = CalculateFuturesDeliveryBasket::new(self.futures_delivery.as_ref())
            .execute(materialized.inputs())?;
        futures_delivery_result(
            &result,
            materialized.funding_rate().annual_financing_rate(),
            materialized.coupon_tax_treatments(),
            &context,
            materialized.evidence(),
        )
    }

    async fn analyze_futures_hedge_value(
        &self,
        request: &pb::AnalyzeFuturesHedgeRequest,
        scope: &AccessScope,
    ) -> Result<pb::AnalyzeFuturesHedgeResult, ApplicationError> {
        let context = parse_context(request.context.as_ref(), ExpectedAlgorithm::futures_hedge())?;
        scope.authorize(&context.owner)?;
        let materialized = MaterializeHedgeRatesInput::new(
            self.definitions.as_ref(),
            self.subjects.as_ref(),
            self.artifacts.as_ref(),
            self.blobs.as_ref(),
            self.integrity_events.as_ref(),
            self.bond_codec.as_ref(),
            self.delivery_codec.as_ref(),
            self.delivery_parser.as_ref(),
        )
        .execute(
            scope,
            HedgeRatesCommand {
                owner: context.owner.clone(),
                subject_ref: context.subject_ref.clone(),
                units: context.units.requirements()?,
                knowledge_at: context.knowledge_at.clone(),
                target_risk_artifact: parse_artifact(request.target_risk_artifact.as_ref())?,
                delivery_artifact: parse_artifact(request.delivery_artifact.as_ref())?,
                ctd_analytics_artifact: parse_artifact(request.ctd_analytics_artifact.as_ref())?,
                futures_contract: parse_object(request.futures_contract.as_ref())?,
                valuation_at: parse_market_time(request.valuation_at.as_ref())?,
            },
            trace_context(request),
        )
        .await?;
        let result = CalculateFuturesHedge::new(self.futures_hedge.as_ref())
            .execute(materialized.input())?;
        Ok(futures_hedge_result(
            &result,
            &context,
            materialized.evidence(),
        ))
    }
}

impl RatesGrpcService {
    /// Executes a Bond calculation from an input that was already materialized and verified by
    /// the Application layer.
    ///
    /// This is the private native-node seam. The public request still carries only exact
    /// identities; the numerical facts, resolved tax scalar, and response evidence must arrive
    /// through the private materialized port. Every public value that can affect the calculation
    /// is compared with the materialized input before the engine is called.
    ///
    /// # Errors
    ///
    /// Returns a non-retryable validation failure when either representation drifts, the supplied
    /// evidence is incomplete, or the numerical engine rejects the materialized input.
    pub fn execute_materialized_bond_request(
        engine: &dyn BondAnalyticsEngine,
        request: &pb::AnalyzeBondRequest,
        input: &ficant_domain::analytics::BondAnalyticsInput,
        coupon_tax_treatment: &CouponTaxTreatment,
        supplied_metadata: pb::ResultMetadata,
    ) -> Result<pb::AnalyzeBondResult, ApplicationError> {
        let (context, expected_metadata) = materialized_bond_proof(
            request,
            input,
            coupon_tax_treatment,
            &supplied_metadata.consumed_inputs,
        )?;
        if supplied_metadata != expected_metadata {
            return Err(invalid());
        }

        let pre_tax = CalculateBondAnalytics::new(engine).execute(input)?;
        if pre_tax.schema_id() != RESULT_SCHEMA_ID {
            return Err(invalid());
        }
        let terms = input
            .terms()
            .with_coupon_rate(tax_adjusted_coupon_rate(
                input,
                &pre_tax,
                coupon_tax_treatment,
            )?)
            .map_err(map_domain_error)?;
        let after_tax_input = input
            .with_terms_and_price_in(terms, pre_tax.measures().clean_price())
            .map_err(map_domain_error)?;
        let after_tax = CalculateBondAnalytics::new(engine).execute(&after_tax_input)?;
        bond_result_with_metadata(
            &pre_tax,
            Some((&after_tax, coupon_tax_treatment)),
            &context.units,
            supplied_metadata,
        )
    }
}

/// A fully materialized Bond request whose public identities and private proof have already been
/// jointly validated.
#[derive(Clone, Debug)]
pub struct ParsedBondAnalyticsRequest {
    request: pb::AnalyzeBondRequest,
    input: ficant_domain::analytics::BondAnalyticsInput,
    coupon_tax_treatment: CouponTaxTreatment,
    metadata: pb::ResultMetadata,
}

/// Parses and seals the exact two-port Bond representation without invoking a numerical engine.
///
/// # Errors
///
/// Returns a validation failure when any public identity, private fact, tax scalar or evidence
/// binding drifts.
pub fn parse_analyze_bond_request(
    request: pb::AnalyzeBondRequest,
    input: ficant_domain::analytics::BondAnalyticsInput,
    coupon_tax_treatment: CouponTaxTreatment,
    consumed_inputs: &[pb::AnalysisInputBinding],
) -> Result<ParsedBondAnalyticsRequest, ApplicationError> {
    let metadata = RatesGrpcService::canonical_materialized_bond_metadata(
        &request,
        &input,
        &coupon_tax_treatment,
        consumed_inputs,
    )?;
    Ok(ParsedBondAnalyticsRequest {
        request,
        input,
        coupon_tax_treatment,
        metadata,
    })
}

/// Executes an already parsed exact Bond request.
///
/// # Errors
///
/// Returns validation or numerical failures from the sealed R5D private execution seam.
pub fn execute_parsed_bond_request(
    engine: &dyn BondAnalyticsEngine,
    parsed: &ParsedBondAnalyticsRequest,
) -> Result<pb::AnalyzeBondResult, ApplicationError> {
    RatesGrpcService::execute_materialized_bond_request(
        engine,
        &parsed.request,
        &parsed.input,
        &parsed.coupon_tax_treatment,
        parsed.metadata.clone(),
    )
}

/// Parses, seals and executes one exact two-port Bond request.
///
/// # Errors
///
/// Returns validation or numerical failures from parsing or execution.
pub fn analyze_bond_request(
    engine: &dyn BondAnalyticsEngine,
    request: pb::AnalyzeBondRequest,
    input: ficant_domain::analytics::BondAnalyticsInput,
    coupon_tax_treatment: CouponTaxTreatment,
    consumed_inputs: &[pb::AnalysisInputBinding],
) -> Result<pb::AnalyzeBondResult, ApplicationError> {
    let parsed = parse_analyze_bond_request(request, input, coupon_tax_treatment, consumed_inputs)?;
    execute_parsed_bond_request(engine, &parsed)
}

#[allow(clippy::too_many_lines)]
fn materialized_bond_proof(
    request: &pb::AnalyzeBondRequest,
    input: &ficant_domain::analytics::BondAnalyticsInput,
    coupon_tax_treatment: &CouponTaxTreatment,
    consumed_inputs: &[pb::AnalysisInputBinding],
) -> Result<(ParsedContext, pb::ResultMetadata), ApplicationError> {
    let context = parse_context(request.context.as_ref(), ExpectedAlgorithm::bond())?;
    let bond = parse_object(request.bond.as_ref())?;
    let calendar = parse_object(request.calendar.as_ref())?;
    let snapshot = parse_snapshot(request.data_snapshot.as_ref())?;
    let tax_rule_pack = parse_object(request.tax_rule_pack.as_ref())?;
    let valuation_at = parse_market_time(request.valuation_at.as_ref())?;
    let settlement_date = parse_date(&request.settlement_date)?;
    let calendar_requirement = parse_calendar_requirement(request.calendar_requirement)?;
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
    if &context.owner != input.owner()
        || &bond != input.bond()
        || &tax_rule_pack != input.rule_pack()
        || snapshot.id() != input.snapshot().version_ref().id()
        || snapshot.content_hash() != input.snapshot().content_hash()
        || calendar.version_ref().id().as_str() != input.calendar().id()
        || calendar.version_ref().version() != input.calendar().version()
        || calendar.content_hash() != input.calendar().content_hash()
        || valuation_at != *input.valuation_at()
        || settlement_date != input.settlement_date()
        || calendar_requirement != input.calendar_requirement()
        || mode != input.mode()
        || input_value != input.input_value()
        || coupon_tax_treatment.unit() != &parse_unit(&context.units.rate)?
    {
        return Err(invalid());
    }
    require_exact_bond_roles(consumed_inputs)?;
    let parsed_inputs = consumed_inputs
        .iter()
        .map(parse_metadata_evidence)
        .collect::<Result<Vec<_>, _>>()?;
    validate_bond_evidence_times(&parsed_inputs, &valuation_at, &context.knowledge_at, input)?;
    let evidence = RatesRequestEvidence::bond(
        parsed_inputs,
        &context.knowledge_at,
        input,
        coupon_tax_treatment,
    )?;
    let expected = metadata(RESULT_SCHEMA_ID, &context, &evidence);
    if expected.consumed_inputs.as_slice() != consumed_inputs {
        return Err(invalid());
    }
    validate_materialized_metadata(&context, &expected, &context.units.all_proto_refs())?;
    require_metadata_version(
        &expected,
        pb::AnalysisInputRole::Subject,
        &context.subject_ref,
    )?;
    require_metadata_object(&expected, pb::AnalysisInputRole::Bond, input.bond())?;
    require_metadata_object(&expected, pb::AnalysisInputRole::Calendar, &calendar)?;
    require_metadata_snapshot(&expected, pb::AnalysisInputRole::DataSnapshot, &snapshot)?;
    require_metadata_object(
        &expected,
        pb::AnalysisInputRole::TaxRulePack,
        input.rule_pack(),
    )?;
    Ok((context, expected))
}

fn require_exact_bond_roles(inputs: &[pb::AnalysisInputBinding]) -> Result<(), ApplicationError> {
    let expected = [
        (pb::AnalysisInputRole::Subject, 1_usize),
        (pb::AnalysisInputRole::Unit, 9),
        (pb::AnalysisInputRole::Bond, 1),
        (pb::AnalysisInputRole::Calendar, 1),
        (pb::AnalysisInputRole::DataSnapshot, 1),
        (pb::AnalysisInputRole::TaxRulePack, 1),
    ];
    if inputs.len() != expected.iter().map(|(_, count)| count).sum::<usize>()
        || expected.iter().any(|(role, count)| {
            inputs
                .iter()
                .filter(|input| input.role == *role as i32)
                .count()
                != *count
        })
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_bond_evidence_times(
    inputs: &[RatesInputEvidence],
    valuation_at: &MarketTime,
    knowledge_at: &MarketTime,
    materialized: &ficant_domain::analytics::BondAnalyticsInput,
) -> Result<(), ApplicationError> {
    for input in inputs {
        match input.role() {
            RatesInputRole::Subject | RatesInputRole::Unit | RatesInputRole::Bond => {
                if input.observed_at().is_some()
                    || input.visible_at().is_some()
                    || input.effective_from().is_some()
                    || input.effective_to().is_some()
                {
                    return Err(invalid());
                }
            }
            RatesInputRole::Calendar => {
                let from = input.effective_from().ok_or_else(invalid)?;
                let to = input.effective_to().ok_or_else(invalid)?;
                if input.observed_at().is_some()
                    || input.visible_at().is_some()
                    || from.local_trading_date() != materialized.calendar().coverage_start()
                    || to.local_trading_date() != materialized.calendar().coverage_end()
                    || from.instant() > valuation_at.instant()
                    || valuation_at.instant() >= to.instant()
                {
                    return Err(invalid());
                }
            }
            RatesInputRole::DataSnapshot => {
                let observed = input.observed_at().ok_or_else(invalid)?;
                let visible = input.visible_at().ok_or_else(invalid)?;
                if observed != valuation_at
                    || visible.instant() < observed.instant()
                    || visible.instant() > knowledge_at.instant()
                    || input.effective_from().is_some()
                    || input.effective_to().is_some()
                {
                    return Err(invalid());
                }
            }
            RatesInputRole::TaxRulePack => {
                let from = input.effective_from().ok_or_else(invalid)?;
                let to = input.effective_to().ok_or_else(invalid)?;
                if input.observed_at().is_some()
                    || input.visible_at().is_some()
                    || from.instant() > valuation_at.instant()
                    || valuation_at.instant() >= to.instant()
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
    }
    Ok(())
}

#[tonic::async_trait]
impl RatesAnalyticsService for RatesGrpcService {
    async fn analyze_bond(
        &self,
        request: Request<pb::AnalyzeBondRequest>,
    ) -> Result<Response<pb::AnalyzeBondResponse>, Status> {
        const OPERATION: &str = "rates.analyze-bond";
        let result = match self.authorize(&request) {
            Err(error) => Err(error),
            Ok(principal) => {
                self.analyze_bond_value(request.get_ref(), principal.access_scope())
                    .await
            }
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
            Err(error) => Err(error),
            Ok(principal) => {
                self.interpolate_curve_value(request.get_ref(), principal.access_scope())
                    .await
            }
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
            Err(error) => Err(error),
            Ok(principal) => {
                self.analyze_carry_roll_value(request.get_ref(), principal.access_scope())
                    .await
            }
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
            Err(error) => Err(error),
            Ok(principal) => {
                self.analyze_futures_delivery_value(request.get_ref(), principal.access_scope())
                    .await
            }
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
            Err(error) => Err(error),
            Ok(principal) => {
                self.analyze_futures_hedge_value(request.get_ref(), principal.access_scope())
                    .await
            }
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
    algorithm: ExpectedAlgorithm,
    units: UnitBindings,
    subject_ref: VersionRef,
    knowledge_at: MarketTime,
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
    fn parse(value: Option<&pb::AnalysisUnits>) -> Result<Self, ApplicationError> {
        let value = value.ok_or_else(invalid)?;
        Ok(Self {
            currency_amount: parse_proto_unit(value.currency_amount.as_ref())?,
            price_per_100: parse_proto_unit(value.price_per_100.as_ref())?,
            rate: parse_proto_unit(value.rate.as_ref())?,
            years: parse_proto_unit(value.years.as_ref())?,
            years_squared: parse_proto_unit(value.years_squared.as_ref())?,
            dv01_per_100: parse_proto_unit(value.dv01_per_100.as_ref())?,
            dv01: parse_proto_unit(value.dv01.as_ref())?,
            dimensionless: parse_proto_unit(value.dimensionless.as_ref())?,
            contract_count: parse_proto_unit(value.contract_count.as_ref())?,
        })
    }

    fn all_proto_refs(&self) -> [&ProtoUnitRef; 9] {
        [
            &self.currency_amount,
            &self.price_per_100,
            &self.rate,
            &self.years,
            &self.years_squared,
            &self.dv01_per_100,
            &self.dv01,
            &self.dimensionless,
            &self.contract_count,
        ]
    }

    fn requirements(&self) -> Result<Vec<RatesUnitRequirement>, ApplicationError> {
        [
            (&self.currency_amount, "currency_amount"),
            (&self.price_per_100, "price_per_100"),
            (&self.rate, "rate"),
            (&self.years, "years"),
            (&self.years_squared, "years_squared"),
            (&self.dv01_per_100, "dv01_per_100"),
            (&self.dv01, "dv01"),
            (&self.dimensionless, "dimensionless"),
            (&self.contract_count, "contract_count"),
        ]
        .into_iter()
        .map(|(reference, dimension)| {
            parse_unit(reference).map(|reference| RatesUnitRequirement::new(reference, dimension))
        })
        .collect()
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
        algorithm: expected,
        units: UnitBindings::parse(value.units.as_ref())?,
        subject_ref: parse_subject_ref(value.subject_ref.as_ref())?,
        knowledge_at: parse_market_time(value.knowledge_at.as_ref())?,
    })
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

fn parse_snapshot(
    value: Option<&pb::SnapshotBinding>,
) -> Result<ImmutableSnapshotBinding, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(ImmutableSnapshotBinding::new(
        parse_ulid(value.snapshot_id.as_ref())?,
        parse_hash(value.content_hash.as_ref())?,
    ))
}

fn parse_artifact(
    value: Option<&pb::ArtifactBinding>,
) -> Result<ImmutableArtifactBinding, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    Ok(ImmutableArtifactBinding::new(
        parse_ulid(value.artifact_id.as_ref())?,
        parse_hash(value.content_hash.as_ref())?,
    ))
}

fn parse_metadata_evidence(
    value: &pb::AnalysisInputBinding,
) -> Result<RatesInputEvidence, ApplicationError> {
    let role = match pb::AnalysisInputRole::try_from(value.role).map_err(|_| invalid())? {
        pb::AnalysisInputRole::Subject => RatesInputRole::Subject,
        pb::AnalysisInputRole::Unit => RatesInputRole::Unit,
        pb::AnalysisInputRole::Bond => RatesInputRole::Bond,
        pb::AnalysisInputRole::Calendar => RatesInputRole::Calendar,
        pb::AnalysisInputRole::CurveSnapshot => RatesInputRole::CurveSnapshot,
        pb::AnalysisInputRole::DataSnapshot => RatesInputRole::DataSnapshot,
        pb::AnalysisInputRole::DataSource => RatesInputRole::DataSource,
        pb::AnalysisInputRole::TaxRulePack => RatesInputRole::TaxRulePack,
        pb::AnalysisInputRole::FundingRulePack => RatesInputRole::FundingRulePack,
        pb::AnalysisInputRole::DeliveryRulePack => RatesInputRole::DeliveryRulePack,
        pb::AnalysisInputRole::FuturesContract => RatesInputRole::FuturesContract,
        pb::AnalysisInputRole::TargetRiskArtifact => RatesInputRole::TargetRiskArtifact,
        pb::AnalysisInputRole::DeliveryArtifact => RatesInputRole::DeliveryArtifact,
        pb::AnalysisInputRole::CtdAnalyticsArtifact => RatesInputRole::CtdAnalyticsArtifact,
        pb::AnalysisInputRole::CurveRulePack => RatesInputRole::CurveRulePack,
        pb::AnalysisInputRole::CurveNodeDefinition => RatesInputRole::CurveNodeDefinition,
        pb::AnalysisInputRole::Unspecified => return Err(invalid()),
    };
    let binding = match value.binding.as_ref().ok_or_else(invalid)? {
        pb::analysis_input_binding::Binding::Object(value) => {
            RatesEvidenceBinding::Object(parse_object(Some(value))?)
        }
        pb::analysis_input_binding::Binding::Snapshot(value) => {
            RatesEvidenceBinding::Snapshot(parse_snapshot(Some(value))?)
        }
        pb::analysis_input_binding::Binding::Artifact(value) => {
            RatesEvidenceBinding::Artifact(parse_artifact(Some(value))?)
        }
        pb::analysis_input_binding::Binding::CurveNode(value) => {
            if value.curve_node_id.trim().is_empty()
                || value.curve_node_id != value.curve_node_id.trim()
            {
                return Err(invalid());
            }
            RatesEvidenceBinding::CurveNode(ImmutableCurveNodeBinding::new(
                value.curve_node_id.clone(),
                parse_hash(value.content_hash.as_ref())?,
            ))
        }
    };
    Ok(RatesInputEvidence::new(
        role,
        parse_owner(value.owner.as_ref())?,
        binding,
        parse_optional_market_time(value.observed_at.as_ref())?,
        parse_optional_market_time(value.visible_at.as_ref())?,
        parse_optional_market_time(value.effective_from.as_ref())?,
        parse_optional_market_time(value.effective_to.as_ref())?,
    ))
}

fn parse_optional_market_time(
    value: Option<&ficant_contracts::ficant::core::v1::MarketTime>,
) -> Result<Option<MarketTime>, ApplicationError> {
    value
        .map(|value| parse_market_time(Some(value)))
        .transpose()
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

fn parse_proto_unit(value: Option<&ProtoUnitRef>) -> Result<ProtoUnitRef, ApplicationError> {
    let value = value.ok_or_else(invalid)?.clone();
    parse_unit(&value)?;
    Ok(value)
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
    if value.unit.as_ref() != Some(expected_unit) || value.scale > DECIMAL_SCALE {
        return Err(map_domain_error(DomainErrorCode::InvalidUnit));
    }
    let coefficient = value.coefficient.parse::<i128>().map_err(|_| invalid())?;
    let factor = 10_i128
        .checked_pow(DECIMAL_SCALE - value.scale)
        .ok_or_else(invalid)?;
    Ok(FixedDecimal::from_scaled(
        coefficient.checked_mul(factor).ok_or_else(invalid)?,
    ))
}

fn parse_calendar_requirement(value: i32) -> Result<CalendarRequirement, ApplicationError> {
    match pb::CalendarRequirement::try_from(value).map_err(|_| invalid())? {
        pb::CalendarRequirement::ReferenceReplay => Ok(CalendarRequirement::ReferenceReplay),
        pb::CalendarRequirement::ExactMarket => Ok(CalendarRequirement::ExactMarket),
        pb::CalendarRequirement::Unspecified => Err(invalid()),
    }
}

fn parse_date(value: &str) -> Result<NaiveDate, ApplicationError> {
    value.parse().map_err(|_| invalid())
}

fn parse_market_time(
    value: Option<&ficant_contracts::ficant::core::v1::MarketTime>,
) -> Result<MarketTime, ApplicationError> {
    let value = value.ok_or_else(invalid)?;
    let timestamp = value.instant.as_ref().ok_or_else(invalid)?;
    let nanos = u32::try_from(timestamp.nanos).map_err(|_| invalid())?;
    let instant =
        chrono::DateTime::<Utc>::from_timestamp(timestamp.seconds, nanos).ok_or_else(invalid)?;
    MarketTime::new(
        instant,
        value.market_timezone.clone(),
        parse_date(&value.local_trading_date)?,
    )
    .map_err(map_domain_error)
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

fn trace_context(message: &impl Message) -> ficant_application::ports::SafeTraceContext {
    let hash = ContentHash::digest(&message.encode_to_vec());
    let value = hash.as_bytes()[..16]
        .iter()
        .fold(String::with_capacity(32), |mut output, byte| {
            use std::fmt::Write as _;
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        });
    ficant_application::ports::SafeTraceContext::new(value)
        .expect("derived trace token is canonical")
}

fn bond_result(
    result: &BondAnalyticsResult,
    after_tax: Option<(&BondAnalyticsResult, &CouponTaxTreatment)>,
    context: &ParsedContext,
    evidence: &RatesRequestEvidence,
) -> Result<pb::AnalyzeBondResult, ApplicationError> {
    bond_result_with_metadata(
        result,
        after_tax,
        &context.units,
        metadata(result.schema_id(), context, evidence),
    )
}

fn bond_result_with_metadata(
    result: &BondAnalyticsResult,
    after_tax: Option<(&BondAnalyticsResult, &CouponTaxTreatment)>,
    units: &UnitBindings,
    metadata: pb::ResultMetadata,
) -> Result<pb::AnalyzeBondResult, ApplicationError> {
    let measures = result.measures();
    let after_tax = after_tax
        .map(|(value, treatment)| {
            Ok(pb::TaxAdjustedBondAnalytics {
                cashflows: tax_adjusted_cashflows(result, units, treatment)?,
                yield_to_maturity: Some(decimal(value.measures().yield_to_maturity(), &units.rate)),
                claim_scope: proto_claim_scope(treatment.claim_scope()),
            })
        })
        .transpose()?;
    Ok(pb::AnalyzeBondResult {
        cashflows: derived_cashflows(result, units),
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
        metadata: Some(metadata),
        after_tax,
    })
}

fn tax_adjusted_cashflows(
    pre_tax: &BondAnalyticsResult,
    units: &UnitBindings,
    treatment: &CouponTaxTreatment,
) -> Result<Vec<pb::DerivedCashflow>, ApplicationError> {
    pre_tax
        .cashflows()
        .iter()
        .map(|value| {
            let coupon = treatment.adjust_coupon(value.coupon())?;
            let total = coupon
                .checked_add(value.principal())
                .map_err(map_domain_error)?;
            Ok(pb::DerivedCashflow {
                sequence: value.sequence(),
                nominal_date: value.nominal_date().to_string(),
                payment_date: value.payment_date().to_string(),
                coupon: Some(decimal(coupon, &units.currency_amount)),
                principal: Some(decimal(value.principal(), &units.currency_amount)),
                total: Some(decimal(total, &units.currency_amount)),
            })
        })
        .collect()
}

fn tax_adjusted_coupon_rate(
    input: &ficant_domain::analytics::BondAnalyticsInput,
    pre_tax: &BondAnalyticsResult,
    treatment: &CouponTaxTreatment,
) -> Result<FixedDecimal, ApplicationError> {
    let gross_coupon = pre_tax
        .cashflows()
        .first()
        .map(ficant_domain::analytics::DerivedCashflow::coupon)
        .ok_or_else(invalid)?;
    if pre_tax
        .cashflows()
        .iter()
        .any(|cashflow| cashflow.coupon() != gross_coupon)
    {
        return Err(invalid());
    }
    let periods_per_year = match input.terms().frequency() {
        CouponFrequency::Annual => 1,
        CouponFrequency::Semiannual => 2,
    };
    treatment
        .adjust_coupon(gross_coupon)?
        .checked_mul_integer(periods_per_year)
        .and_then(|value| value.checked_div_round_ties_even(input.terms().face_amount()))
        .map_err(map_domain_error)
}

fn derived_cashflows(
    result: &BondAnalyticsResult,
    units: &UnitBindings,
) -> Vec<pb::DerivedCashflow> {
    result
        .cashflows()
        .iter()
        .map(|value| pb::DerivedCashflow {
            sequence: value.sequence(),
            nominal_date: value.nominal_date().to_string(),
            payment_date: value.payment_date().to_string(),
            coupon: Some(decimal(value.coupon(), &units.currency_amount)),
            principal: Some(decimal(value.principal(), &units.currency_amount)),
            total: Some(decimal(value.total(), &units.currency_amount)),
        })
        .collect()
}

fn curve_result(
    point: &YieldCurvePoint,
    context: &ParsedContext,
    evidence: &RatesRequestEvidence,
) -> pb::InterpolateYieldCurveResult {
    pb::InterpolateYieldCurveResult {
        query_date: point.query().query_date().to_string(),
        yield_to_maturity: Some(decimal(point.yield_to_maturity(), &context.units.rate)),
        metadata: Some(metadata(point.schema_id(), context, evidence)),
    }
}

fn carry_roll_result(
    result: &CarryRollResult,
    context: &ParsedContext,
    evidence: &RatesRequestEvidence,
) -> pb::AnalyzeCarryRollResult {
    let value = result.measures();
    pb::AnalyzeCarryRollResult {
        measures: Some(pb::CarryRollMeasures {
            initial_yield: Some(decimal(value.initial_yield(), &context.units.rate)),
            rolled_yield: Some(decimal(value.rolled_yield(), &context.units.rate)),
            initial_dirty_price: Some(decimal(
                value.initial_dirty_price(),
                &context.units.price_per_100,
            )),
            horizon_dirty_at_initial_yield: Some(decimal(
                value.horizon_dirty_at_initial_yield(),
                &context.units.price_per_100,
            )),
            horizon_dirty_at_rolled_yield: Some(decimal(
                value.horizon_dirty_at_rolled_yield(),
                &context.units.price_per_100,
            )),
            paid_cashflows: Some(decimal(
                value.paid_cashflows(),
                &context.units.price_per_100,
            )),
            carry: Some(decimal(value.carry(), &context.units.price_per_100)),
            roll_down: Some(decimal(value.roll_down(), &context.units.price_per_100)),
            total_return: Some(decimal(value.total_return(), &context.units.price_per_100)),
        }),
        metadata: Some(metadata(result.schema_id(), context, evidence)),
    }
}

fn futures_delivery_result(
    result: &FuturesDeliveryBasketResult,
    annual_financing_rate: FixedDecimal,
    treatments: &[CouponTaxTreatment],
    context: &ParsedContext,
    evidence: &RatesRequestEvidence,
) -> Result<pb::AnalyzeFuturesDeliveryResult, ApplicationError> {
    if result.candidates().len() != treatments.len() || treatments.is_empty() {
        return Err(invalid());
    }
    let mut subject_rates = Vec::with_capacity(treatments.len());
    let candidates = result
        .candidates()
        .iter()
        .zip(treatments)
        .map(|(candidate, treatment)| {
            let (measures, subject_rate) = delivery_measures(
                candidate.measures(),
                candidate.input().purchase_date(),
                candidate.input().delivery_date(),
                &context.units,
                annual_financing_rate,
                treatment,
            )?;
            subject_rates.push(subject_rate);
            Ok(pb::FuturesDeliveryCandidateResult {
                bond: Some(object_binding(candidate.input().bond())),
                measures: Some(measures),
                claim_scope: proto_claim_scope(treatment.claim_scope()),
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    let subject_ctd_index = select_subject_ctd(result, &subject_rates)?;
    Ok(pb::AnalyzeFuturesDeliveryResult {
        candidates,
        ctd_index: u32::try_from(result.ctd_index()).map_err(|_| invalid())?,
        metadata: Some(metadata(result.ctd().schema_id(), context, evidence)),
        subject_ctd_index: u32::try_from(subject_ctd_index).map_err(|_| invalid())?,
    })
}

fn delivery_measures(
    value: FuturesDeliveryMeasures,
    purchase_date: NaiveDate,
    delivery_date: NaiveDate,
    units: &UnitBindings,
    annual_financing_rate: FixedDecimal,
    treatment: &CouponTaxTreatment,
) -> Result<(pb::FuturesDeliveryMeasures, FixedDecimal), ApplicationError> {
    let funding_adjusted_irr = value
        .implied_repo_rate()
        .checked_sub(annual_financing_rate)
        .map_err(map_domain_error)?;
    let tax_adjusted_interim_coupons = treatment.adjust_coupon(value.interim_coupons())?;
    let subject_tax_adjusted_irr = subject_delivery_irr(
        value.invoice_price(),
        tax_adjusted_interim_coupons,
        value.purchase_dirty_price(),
        purchase_date,
        delivery_date,
    )?;
    Ok((
        pb::FuturesDeliveryMeasures {
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
            tax_adjusted_interim_coupons: Some(decimal(
                tax_adjusted_interim_coupons,
                &units.price_per_100,
            )),
            subject_tax_adjusted_irr: Some(decimal(subject_tax_adjusted_irr, &units.rate)),
        },
        subject_tax_adjusted_irr,
    ))
}

fn subject_delivery_irr(
    invoice_price: FixedDecimal,
    tax_adjusted_interim_coupons: FixedDecimal,
    purchase_dirty_price: FixedDecimal,
    purchase_date: NaiveDate,
    delivery_date: NaiveDate,
) -> Result<FixedDecimal, ApplicationError> {
    let days = (delivery_date - purchase_date).num_days();
    if days <= 0 {
        return Err(invalid());
    }
    invoice_price
        .checked_add(tax_adjusted_interim_coupons)
        .and_then(|value| value.checked_div_round_ties_even(purchase_dirty_price))
        .and_then(|value| value.checked_sub(FixedDecimal::ONE))
        .and_then(|value| value.checked_mul_integer(365))
        .and_then(|value| {
            value.checked_div_round_ties_even(FixedDecimal::from_scaled(
                i128::from(days) * 1_000_000_000_000,
            ))
        })
        .map_err(map_domain_error)
}

fn select_subject_ctd(
    result: &FuturesDeliveryBasketResult,
    subject_rates: &[FixedDecimal],
) -> Result<usize, ApplicationError> {
    if result.candidates().len() != subject_rates.len() || subject_rates.is_empty() {
        return Err(invalid());
    }
    let mut best = 0;
    for index in 1..subject_rates.len() {
        let candidate = result.candidates()[index].measures();
        let incumbent = result.candidates()[best].measures();
        let candidate_id = result.candidates()[index].input().bond().version_ref().id();
        let incumbent_id = result.candidates()[best].input().bond().version_ref().id();
        if subject_rates[index] > subject_rates[best]
            || (subject_rates[index] == subject_rates[best]
                && (candidate.net_basis() < incumbent.net_basis()
                    || (candidate.net_basis() == incumbent.net_basis()
                        && candidate_id < incumbent_id)))
        {
            best = index;
        }
    }
    Ok(best)
}

const fn proto_claim_scope(value: CouponTaxClaimScope) -> i32 {
    match value {
        CouponTaxClaimScope::LegacySyntheticRetainedRate => {
            ProtoCouponTaxClaimScope::Unspecified as i32
        }
        CouponTaxClaimScope::CouponOutputVatBeforeInputCredit => {
            ProtoCouponTaxClaimScope::CouponOutputVatBeforeInputCredit as i32
        }
    }
}

fn futures_hedge_result(
    result: &FuturesHedgeResult,
    context: &ParsedContext,
    evidence: &RatesRequestEvidence,
) -> pb::AnalyzeFuturesHedgeResult {
    let value = result.measures();
    pb::AnalyzeFuturesHedgeResult {
        measures: Some(pb::FuturesHedgeMeasures {
            futures_contract_dv01: Some(decimal(
                value.futures_contract_dv01(),
                &context.units.dv01,
            )),
            raw_contracts: Some(decimal(
                value.raw_contracts(),
                &context.units.contract_count,
            )),
            recommended_contracts: value.recommended_contracts(),
            residual_dv01: Some(decimal(value.residual_dv01(), &context.units.dv01)),
            hedge_effectiveness: Some(decimal(
                value.hedge_effectiveness(),
                &context.units.dimensionless,
            )),
        }),
        metadata: Some(metadata(result.schema_id(), context, evidence)),
    }
}

fn metadata(
    schema_id: &str,
    context: &ParsedContext,
    evidence: &RatesRequestEvidence,
) -> pb::ResultMetadata {
    pb::ResultMetadata {
        schema_id: schema_id.to_owned(),
        engine_id: ENGINE_ID.to_owned(),
        engine_version: ENGINE_VERSION.to_owned(),
        algorithm: Some(algorithm_binding(context.algorithm)),
        subject_ref: Some(proto_version_ref(&context.subject_ref)),
        consumed_inputs: evidence
            .consumed_inputs()
            .iter()
            .map(proto_evidence)
            .collect(),
        parameter_digest: Some(pb::ParameterDigest {
            algorithm: Some(algorithm_binding(context.algorithm)),
            canonical_parameters_sha256: Some(proto_hash(evidence.canonical_parameters_sha256())),
        }),
        request_fingerprint: Some(proto_hash(evidence.request_fingerprint())),
    }
}

fn validate_materialized_metadata(
    context: &ParsedContext,
    value: &pb::ResultMetadata,
    expected_units: &[&ProtoUnitRef],
) -> Result<(), ApplicationError> {
    if value.schema_id.trim().is_empty()
        || value.engine_id != ENGINE_ID
        || value.engine_version != ENGINE_VERSION
        || value.consumed_inputs.is_empty()
    {
        return Err(invalid());
    }
    validate_algorithm(value.algorithm.as_ref(), context.algorithm)?;
    if parse_subject_ref(value.subject_ref.as_ref())? != context.subject_ref {
        return Err(invalid());
    }
    let parameter = value.parameter_digest.as_ref().ok_or_else(invalid)?;
    validate_algorithm(parameter.algorithm.as_ref(), context.algorithm)?;
    parse_hash(parameter.canonical_parameters_sha256.as_ref())?;
    parse_hash(value.request_fingerprint.as_ref())?;
    for input in &value.consumed_inputs {
        pb::AnalysisInputRole::try_from(input.role).map_err(|_| invalid())?;
        if parse_owner(input.owner.as_ref())? != context.owner || input.binding.is_none() {
            return Err(invalid());
        }
    }
    let mut expected_unit_refs = expected_units
        .iter()
        .map(|value| parse_unit(value))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|value| VersionRef::new(value.unit_id().clone(), value.version()))
        .collect::<Vec<_>>();
    expected_unit_refs.sort();
    expected_unit_refs.dedup();
    let mut actual_unit_refs = Vec::new();
    for input in value
        .consumed_inputs
        .iter()
        .filter(|input| input.role == pb::AnalysisInputRole::Unit as i32)
    {
        let Some(pb::analysis_input_binding::Binding::Object(binding)) = input.binding.as_ref()
        else {
            return Err(invalid());
        };
        actual_unit_refs.push(parse_object(Some(binding))?.version_ref().clone());
    }
    actual_unit_refs.sort();
    if actual_unit_refs.windows(2).any(|pair| pair[0] == pair[1])
        || actual_unit_refs != expected_unit_refs
    {
        return Err(invalid());
    }
    if value
        .consumed_inputs
        .windows(2)
        .any(|pair| pair[0].encode_to_vec().as_slice() > pair[1].encode_to_vec().as_slice())
    {
        return Err(invalid());
    }
    Ok(())
}

fn require_metadata_version(
    metadata: &pb::ResultMetadata,
    role: pb::AnalysisInputRole,
    expected: &VersionRef,
) -> Result<(), ApplicationError> {
    let matches = metadata
        .consumed_inputs
        .iter()
        .filter(|input| input.role == role as i32)
        .filter_map(|input| match input.binding.as_ref() {
            Some(pb::analysis_input_binding::Binding::Object(value)) => Some(value),
            _ => None,
        })
        .map(|value| parse_object(Some(value)))
        .collect::<Result<Vec<_>, _>>()?;
    if matches.len() != 1 || matches[0].version_ref() != expected {
        return Err(invalid());
    }
    Ok(())
}

fn require_metadata_object(
    metadata: &pb::ResultMetadata,
    role: pb::AnalysisInputRole,
    expected: &AnalyticsObjectRef,
) -> Result<(), ApplicationError> {
    let matches = metadata
        .consumed_inputs
        .iter()
        .filter(|input| input.role == role as i32)
        .filter_map(|input| match input.binding.as_ref() {
            Some(pb::analysis_input_binding::Binding::Object(value)) => Some(value),
            _ => None,
        })
        .map(|value| parse_object(Some(value)))
        .collect::<Result<Vec<_>, _>>()?;
    if matches.as_slice() != [expected.clone()] {
        return Err(invalid());
    }
    Ok(())
}

fn require_metadata_snapshot(
    metadata: &pb::ResultMetadata,
    role: pb::AnalysisInputRole,
    expected: &ImmutableSnapshotBinding,
) -> Result<(), ApplicationError> {
    let matches = metadata
        .consumed_inputs
        .iter()
        .filter(|input| input.role == role as i32)
        .filter_map(|input| match input.binding.as_ref() {
            Some(pb::analysis_input_binding::Binding::Snapshot(value)) => Some(value),
            _ => None,
        })
        .map(|value| parse_snapshot(Some(value)))
        .collect::<Result<Vec<_>, _>>()?;
    if matches.as_slice() != [expected.clone()] {
        return Err(invalid());
    }
    Ok(())
}

fn proto_evidence(value: &RatesInputEvidence) -> pb::AnalysisInputBinding {
    let binding = match value.binding() {
        RatesEvidenceBinding::Object(value) => {
            pb::analysis_input_binding::Binding::Object(object_binding(value))
        }
        RatesEvidenceBinding::Snapshot(value) => {
            pb::analysis_input_binding::Binding::Snapshot(pb::SnapshotBinding {
                snapshot_id: Some(proto_ulid(value.id())),
                content_hash: Some(proto_hash(value.content_hash())),
            })
        }
        RatesEvidenceBinding::Artifact(value) => {
            pb::analysis_input_binding::Binding::Artifact(pb::ArtifactBinding {
                artifact_id: Some(proto_ulid(value.id())),
                content_hash: Some(proto_hash(value.content_hash())),
            })
        }
        RatesEvidenceBinding::CurveNode(value) => {
            pb::analysis_input_binding::Binding::CurveNode(pb::CurveNodeBinding {
                curve_node_id: value.curve_node_id().to_owned(),
                content_hash: Some(proto_hash(value.content_hash())),
            })
        }
    };
    pb::AnalysisInputBinding {
        role: proto_role(value.role()) as i32,
        owner: Some(proto_owner(value.owner())),
        binding: Some(binding),
        observed_at: value.observed_at().map(proto_market_time),
        visible_at: value.visible_at().map(proto_market_time),
        effective_from: value.effective_from().map(proto_market_time),
        effective_to: value.effective_to().map(proto_market_time),
    }
}

fn proto_role(value: RatesInputRole) -> pb::AnalysisInputRole {
    match value {
        RatesInputRole::Subject => pb::AnalysisInputRole::Subject,
        RatesInputRole::Unit => pb::AnalysisInputRole::Unit,
        RatesInputRole::Bond => pb::AnalysisInputRole::Bond,
        RatesInputRole::Calendar => pb::AnalysisInputRole::Calendar,
        RatesInputRole::CurveSnapshot => pb::AnalysisInputRole::CurveSnapshot,
        RatesInputRole::DataSnapshot => pb::AnalysisInputRole::DataSnapshot,
        RatesInputRole::DataSource => pb::AnalysisInputRole::DataSource,
        RatesInputRole::TaxRulePack => pb::AnalysisInputRole::TaxRulePack,
        RatesInputRole::FundingRulePack => pb::AnalysisInputRole::FundingRulePack,
        RatesInputRole::DeliveryRulePack => pb::AnalysisInputRole::DeliveryRulePack,
        RatesInputRole::FuturesContract => pb::AnalysisInputRole::FuturesContract,
        RatesInputRole::TargetRiskArtifact => pb::AnalysisInputRole::TargetRiskArtifact,
        RatesInputRole::DeliveryArtifact => pb::AnalysisInputRole::DeliveryArtifact,
        RatesInputRole::CtdAnalyticsArtifact => pb::AnalysisInputRole::CtdAnalyticsArtifact,
        RatesInputRole::CurveRulePack => pb::AnalysisInputRole::CurveRulePack,
        RatesInputRole::CurveNodeDefinition => pb::AnalysisInputRole::CurveNodeDefinition,
    }
}

fn algorithm_binding(value: ExpectedAlgorithm) -> pb::AlgorithmBinding {
    pb::AlgorithmBinding {
        algorithm_id: value.id.to_owned(),
        algorithm_version: value.version,
        convention_profile: value.convention.to_owned(),
        abi_version: ABI_VERSION,
    }
}

fn object_binding(value: &AnalyticsObjectRef) -> pb::ObjectBinding {
    pb::ObjectBinding {
        object: Some(proto_version_ref(value.version_ref())),
        content_hash: Some(proto_hash(value.content_hash())),
    }
}

fn proto_owner(value: &OwnerRef) -> ProtoOwnerRef {
    ProtoOwnerRef {
        tenant_id: Some(proto_ulid(value.tenant_id())),
        owner_id: Some(proto_ulid(value.owner_id())),
    }
}

fn proto_version_ref(value: &VersionRef) -> ficant_contracts::ficant::core::v1::VersionRef {
    ficant_contracts::ficant::core::v1::VersionRef {
        id: Some(proto_ulid(value.id())),
        version: value.version().get(),
    }
}

fn proto_ulid(value: &Ulid) -> ficant_contracts::ficant::core::v1::Ulid {
    ficant_contracts::ficant::core::v1::Ulid {
        value: value.as_str().to_owned(),
    }
}

fn proto_hash(value: &ContentHash) -> ficant_contracts::ficant::core::v1::Sha256 {
    ficant_contracts::ficant::core::v1::Sha256 {
        value: value.as_bytes().to_vec(),
    }
}

fn proto_market_time(value: &MarketTime) -> ficant_contracts::ficant::core::v1::MarketTime {
    ficant_contracts::ficant::core::v1::MarketTime {
        instant: Some(prost_types::Timestamp {
            seconds: value.instant().timestamp(),
            nanos: i32::try_from(value.instant().timestamp_subsec_nanos())
                .expect("nanoseconds fit i32"),
        }),
        market_timezone: value.market_timezone().to_owned(),
        local_trading_date: value.local_trading_date().to_string(),
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

fn forbidden() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::Forbidden, false)
}

#[cfg(test)]
mod tests {
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
