use std::collections::BTreeSet;

use chrono::{Datelike, Days, Months, NaiveDate};
use ficant_domain::analytics::{
    ABI_VERSION, ALGORITHM_ID, ALGORITHM_VERSION, AnalyticsMode, AnalyticsObjectRef,
    BondAnalyticsInput, BondTerms, BusinessDayConvention, CONVENTION_PROFILE, CalendarBinding,
    CalendarRequirement, CouponFrequency, DayCountConvention,
};
use ficant_domain::curves::{
    CARRY_ROLL_ALGORITHM_ID, CARRY_ROLL_ALGORITHM_VERSION, CARRY_ROLL_CONVENTION_PROFILE,
    CURVE_ALGORITHM_ID, CURVE_ALGORITHM_VERSION, CURVE_CONVENTION_PROFILE, CarryRollInput,
    YieldCurveBinding, YieldCurveInterpolation, YieldCurveNode, YieldCurveQuery,
};
use ficant_domain::futures_delivery::{
    FUTURES_DELIVERY_ALGORITHM_ID, FUTURES_DELIVERY_ALGORITHM_VERSION,
    FUTURES_DELIVERY_CONVENTION_PROFILE, FuturesDeliverableInput,
};
use ficant_domain::futures_hedge::{
    FUTURES_HEDGE_ALGORITHM_ID, FUTURES_HEDGE_ALGORITHM_VERSION, FUTURES_HEDGE_CONVENTION_PROFILE,
    FuturesHedgeInput,
};
use ficant_domain::market::{
    Bond, BondBusinessDayConvention, BondCouponFrequency, BondDayCountConvention, Calendar,
    DataSource, DataSourceKind, MarketRulePack, PriceSourceType, Unit, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, FixedDecimal, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{Artifact, ArtifactKind, DataSnapshot};
use ficant_domain::subject::SubjectVersion;
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged, VersionedDefinition};

use crate::ports::{
    AccessScope, ApplicationResult, ArtifactRepository, BondAnalyticsArtifactCodec,
    CanonicalSnapshotDecoder, CouponTaxTreatment, CurvePointSetDecoder,
    CurveSnapshotMetadataRepository, DataSourceRepository, DefinitionRepository, DefinitionValue,
    FactorTopologyRepository, FundingRate, FundingRulePackParser, FuturesDeliveryArtifactCodec,
    FuturesDeliveryRuleParser, InstrumentSubtype, IntegrityEventSink, RequiredVerifiedBlobRead,
    SafeTraceContext, SnapshotVerifiedReadMetadataRepository, SubjectRepository, TaxRulePackParser,
    VerifiedBlobReader, VerifiedBlobRole, VerifiedReadResourceKind, definition_content_hash,
};
use crate::use_cases::bond_analytics::{BOND_ANALYTICS_MEDIA_TYPE, map_analytics_error};
use crate::use_cases::funding_rule::ResolveFundingRule;
use crate::use_cases::futures_delivery::{
    FUTURES_DELIVERY_MEDIA_TYPE, MaterializeRegisteredFuturesDelivery,
};
use crate::use_cases::tax_rule::ResolveTaxRule;
use crate::use_cases::verified_reads::{VerifiedSnapshotRead, VerifiedSnapshotReader};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum RatesInputRole {
    Subject = 1,
    Unit = 2,
    Bond = 3,
    Calendar = 4,
    CurveSnapshot = 5,
    DataSnapshot = 6,
    DataSource = 7,
    TaxRulePack = 8,
    FundingRulePack = 9,
    DeliveryRulePack = 10,
    FuturesContract = 11,
    TargetRiskArtifact = 12,
    DeliveryArtifact = 13,
    CtdAnalyticsArtifact = 14,
    CurveRulePack = 15,
    CurveNodeDefinition = 16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImmutableSnapshotBinding {
    id: Ulid,
    content_hash: ContentHash,
}

impl ImmutableSnapshotBinding {
    #[must_use]
    pub fn new(id: Ulid, content_hash: ContentHash) -> Self {
        Self { id, content_hash }
    }

    #[must_use]
    pub fn id(&self) -> &Ulid {
        &self.id
    }

    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImmutableArtifactBinding {
    id: Ulid,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImmutableCurveNodeBinding {
    curve_node_id: String,
    content_hash: ContentHash,
}

impl ImmutableCurveNodeBinding {
    #[must_use]
    pub fn new(curve_node_id: impl Into<String>, content_hash: ContentHash) -> Self {
        Self {
            curve_node_id: curve_node_id.into(),
            content_hash,
        }
    }

    #[must_use]
    pub fn curve_node_id(&self) -> &str {
        &self.curve_node_id
    }

    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl ImmutableArtifactBinding {
    #[must_use]
    pub fn new(id: Ulid, content_hash: ContentHash) -> Self {
        Self { id, content_hash }
    }

    #[must_use]
    pub fn id(&self) -> &Ulid {
        &self.id
    }

    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RatesEvidenceBinding {
    Object(AnalyticsObjectRef),
    Snapshot(ImmutableSnapshotBinding),
    Artifact(ImmutableArtifactBinding),
    CurveNode(ImmutableCurveNodeBinding),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatesInputEvidence {
    role: RatesInputRole,
    owner: OwnerRef,
    binding: RatesEvidenceBinding,
    observed_at: Option<MarketTime>,
    visible_at: Option<MarketTime>,
    effective_from: Option<MarketTime>,
    effective_to: Option<MarketTime>,
}

impl RatesInputEvidence {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        role: RatesInputRole,
        owner: OwnerRef,
        binding: RatesEvidenceBinding,
        observed_at: Option<MarketTime>,
        visible_at: Option<MarketTime>,
        effective_from: Option<MarketTime>,
        effective_to: Option<MarketTime>,
    ) -> Self {
        Self {
            role,
            owner,
            binding,
            observed_at,
            visible_at,
            effective_from,
            effective_to,
        }
    }

    #[must_use]
    pub fn role(&self) -> RatesInputRole {
        self.role
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub fn binding(&self) -> &RatesEvidenceBinding {
        &self.binding
    }

    #[must_use]
    pub fn observed_at(&self) -> Option<&MarketTime> {
        self.observed_at.as_ref()
    }

    #[must_use]
    pub fn visible_at(&self) -> Option<&MarketTime> {
        self.visible_at.as_ref()
    }

    #[must_use]
    pub fn effective_from(&self) -> Option<&MarketTime> {
        self.effective_from.as_ref()
    }

    #[must_use]
    pub fn effective_to(&self) -> Option<&MarketTime> {
        self.effective_to.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatesRequestEvidence {
    consumed_inputs: Vec<RatesInputEvidence>,
    canonical_parameters_sha256: ContentHash,
    request_fingerprint: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatesUnitRequirement {
    reference: UnitRef,
    expected_dimension: &'static str,
}

impl RatesUnitRequirement {
    #[must_use]
    pub const fn new(reference: UnitRef, expected_dimension: &'static str) -> Self {
        Self {
            reference,
            expected_dimension,
        }
    }

    #[must_use]
    pub const fn reference(&self) -> &UnitRef {
        &self.reference
    }

    #[must_use]
    pub const fn expected_dimension(&self) -> &'static str {
        self.expected_dimension
    }
}

impl RatesRequestEvidence {
    /// Builds the canonical input evidence and rejects duplicate exact identities.
    ///
    /// # Errors
    ///
    /// Returns a lineage failure when the same role, owner, identity, version and hash appears
    /// more than once, including copies that disagree only in their time evidence.
    pub fn new(
        mut consumed_inputs: Vec<RatesInputEvidence>,
        parameters: &[u8],
    ) -> ApplicationResult<Self> {
        consumed_inputs.sort_by(compare_evidence);
        for (index, input) in consumed_inputs.iter().enumerate() {
            if consumed_inputs[index + 1..]
                .iter()
                .any(|other| same_evidence_identity(input, other))
            {
                return Err(lineage());
            }
        }
        let canonical_parameters_sha256 = ContentHash::digest(parameters);
        let mut bytes = Vec::new();
        append(&mut bytes, b"ficant.rates.request-fingerprint.v1");
        append(&mut bytes, canonical_parameters_sha256.as_bytes());
        for input in &consumed_inputs {
            append_evidence(&mut bytes, input);
        }
        let request_fingerprint = ContentHash::digest(&bytes);
        Ok(Self {
            consumed_inputs,
            canonical_parameters_sha256,
            request_fingerprint,
        })
    }

    #[must_use]
    pub fn consumed_inputs(&self) -> &[RatesInputEvidence] {
        &self.consumed_inputs
    }

    #[must_use]
    pub fn canonical_parameters_sha256(&self) -> &ContentHash {
        &self.canonical_parameters_sha256
    }

    #[must_use]
    pub fn request_fingerprint(&self) -> &ContentHash {
        &self.request_fingerprint
    }

    /// Rebuilds the exact Bond evidence proof used by the public and private execution paths.
    ///
    /// # Errors
    ///
    /// Returns a lineage failure when the evidence contains a duplicate exact identity.
    #[allow(clippy::too_many_arguments)]
    pub fn bond(
        consumed_inputs: Vec<RatesInputEvidence>,
        knowledge_at: &MarketTime,
        input: &BondAnalyticsInput,
        coupon_tax_treatment: &CouponTaxTreatment,
    ) -> ApplicationResult<Self> {
        let mut parameters = parameter_prefix(
            b"bond",
            knowledge_at,
            input.valuation_at(),
            ALGORITHM_ID,
            ALGORITHM_VERSION,
            CONVENTION_PROFILE,
        );
        append_bond_materialization(&mut parameters, input);
        append(&mut parameters, &coupon_tax_treatment.fingerprint_bytes());
        Self::new(consumed_inputs, &parameters)
    }
}

#[derive(Clone, Debug)]
struct CommonMaterialization {
    subject: SubjectVersion,
    evidence: Vec<RatesInputEvidence>,
}

pub struct BondRatesCommand {
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub units: Vec<RatesUnitRequirement>,
    pub currency_unit: UnitRef,
    pub rate_unit: UnitRef,
    pub knowledge_at: MarketTime,
    pub bond: AnalyticsObjectRef,
    pub calendar: AnalyticsObjectRef,
    pub data_snapshot: ImmutableSnapshotBinding,
    pub tax_rule_pack: AnalyticsObjectRef,
    pub valuation_at: MarketTime,
    pub settlement_date: NaiveDate,
    pub calendar_requirement: CalendarRequirement,
    pub mode: AnalyticsMode,
    pub input_value: FixedDecimal,
}

pub struct BondRatesMaterialization {
    input: BondAnalyticsInput,
    coupon_tax_treatment: CouponTaxTreatment,
    evidence: RatesRequestEvidence,
}

impl BondRatesMaterialization {
    #[must_use]
    pub fn input(&self) -> &BondAnalyticsInput {
        &self.input
    }

    #[must_use]
    pub fn coupon_tax_treatment(&self) -> &CouponTaxTreatment {
        &self.coupon_tax_treatment
    }

    #[must_use]
    pub fn evidence(&self) -> &RatesRequestEvidence {
        &self.evidence
    }
}

pub struct MaterializeBondRatesInput<'a> {
    definitions: &'a dyn DefinitionRepository,
    subjects: &'a dyn SubjectRepository,
    snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
    blobs: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
    tax_parser: &'a dyn TaxRulePackParser,
}

impl<'a> MaterializeBondRatesInput<'a> {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        definitions: &'a dyn DefinitionRepository,
        subjects: &'a dyn SubjectRepository,
        snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
        blobs: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
        tax_parser: &'a dyn TaxRulePackParser,
    ) -> Self {
        Self {
            definitions,
            subjects,
            snapshots,
            blobs,
            integrity_events,
            tax_parser,
        }
    }

    /// Resolves every authoritative bond input before any numerical engine can be reached.
    /// # Errors
    ///
    /// Fails closed when any exact owner, definition, snapshot, time, unit, Subject, or tax-pack
    /// binding cannot be verified and materialized.
    #[allow(clippy::too_many_lines)]
    pub async fn execute(
        &self,
        scope: &AccessScope,
        command: BondRatesCommand,
        trace: SafeTraceContext,
    ) -> ApplicationResult<BondRatesMaterialization> {
        let common = materialize_common(
            self.definitions,
            self.subjects,
            scope,
            &command.owner,
            &command.subject_ref,
            &command.units,
            &command.knowledge_at,
            "CN",
            "bond-analytics",
        )
        .await?;
        require_units(&command.units, [&command.currency_unit, &command.rate_unit])?;
        validate_authoritative_rate_unit(
            self.definitions,
            scope,
            &command.owner,
            &command.rate_unit,
            self.tax_parser,
        )
        .await?;
        let (bond_definition, bond) =
            read_bond(self.definitions, scope, &command.owner, &command.bond).await?;
        validate_bond_units(
            &bond_definition,
            &bond,
            &command.currency_unit,
            &command.rate_unit,
        )?;
        let calendar =
            read_calendar(self.definitions, scope, &command.owner, &command.calendar).await?;
        if bond_definition.instrument().calendar() != command.calendar.version_ref()
            || calendar.effective().from().instant() > command.valuation_at.instant()
            || command.valuation_at.instant() >= calendar.effective().to().instant()
        {
            return Err(lineage());
        }
        let snapshot = read_data_snapshot(
            self.snapshots,
            self.blobs,
            self.integrity_events,
            scope,
            &command.owner,
            &command.data_snapshot,
            &command.valuation_at,
            &command.knowledge_at,
            trace,
        )
        .await?;
        let tax_pack = read_rule_pack(
            self.definitions,
            scope,
            &command.owner,
            &command.tax_rule_pack,
        )
        .await?;
        let terms = bond_terms(&bond, &command.currency_unit, &command.rate_unit)?;
        let tax_attributes = terms.tax_attributes().ok_or_else(lineage)?;
        let tax_payload_binding = AnalyticsObjectRef::new(
            command.tax_rule_pack.version_ref().clone(),
            tax_pack.content_hash().clone(),
        );
        let coupon_tax_treatment = ResolveTaxRule::new(self.definitions, self.tax_parser)
            .parse_verified(
                scope,
                &tax_payload_binding,
                &command.valuation_at,
                &tax_pack,
                terms.first_issue_date(),
                tax_attributes,
                common.subject.tax_treatment(),
            )?;
        let input = BondAnalyticsInput::new(
            command.owner.clone(),
            command.bond.clone(),
            command.tax_rule_pack.clone(),
            snapshot_object_ref(&snapshot)?,
            command.valuation_at.clone(),
            command.settlement_date,
            command.calendar_requirement,
            calendar_binding(&calendar, command.calendar.content_hash().clone())?,
            terms,
            command.mode,
            command.input_value,
        )
        .map_err(map_domain_error)?;
        let mut evidence = common.evidence;
        evidence.push(object_evidence(
            RatesInputRole::Bond,
            &command.owner,
            command.bond,
            None,
        ));
        evidence.push(calendar_evidence(&calendar));
        evidence.push(data_snapshot_evidence(&snapshot));
        evidence.push(rule_pack_evidence(RatesInputRole::TaxRulePack, &tax_pack));
        let evidence = RatesRequestEvidence::bond(
            evidence,
            &command.knowledge_at,
            &input,
            &coupon_tax_treatment,
        )?;
        Ok(BondRatesMaterialization {
            input,
            coupon_tax_treatment,
            evidence,
        })
    }
}

pub struct CurveRatesCommand {
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub units: Vec<RatesUnitRequirement>,
    pub currency_unit: UnitRef,
    pub rate_unit: UnitRef,
    pub knowledge_at: MarketTime,
    pub curve: ImmutableSnapshotBinding,
    pub query_date: NaiveDate,
}

pub struct CurveRatesMaterialization {
    query: YieldCurveQuery,
    evidence: RatesRequestEvidence,
}

impl CurveRatesMaterialization {
    #[must_use]
    pub fn query(&self) -> &YieldCurveQuery {
        &self.query
    }

    #[must_use]
    pub fn evidence(&self) -> &RatesRequestEvidence {
        &self.evidence
    }
}

pub struct MaterializeCurveRatesInput<'a> {
    definitions: &'a dyn DefinitionRepository,
    subjects: &'a dyn SubjectRepository,
    data_sources: &'a dyn DataSourceRepository,
    snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
    curves: &'a dyn CurveSnapshotMetadataRepository,
    blobs: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
    decoder: &'a dyn CurvePointSetDecoder,
    factors: &'a dyn FactorTopologyRepository,
}

impl<'a> MaterializeCurveRatesInput<'a> {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        definitions: &'a dyn DefinitionRepository,
        subjects: &'a dyn SubjectRepository,
        data_sources: &'a dyn DataSourceRepository,
        snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
        curves: &'a dyn CurveSnapshotMetadataRepository,
        blobs: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
        decoder: &'a dyn CurvePointSetDecoder,
        factors: &'a dyn FactorTopologyRepository,
    ) -> Self {
        Self {
            definitions,
            subjects,
            data_sources,
            snapshots,
            curves,
            blobs,
            integrity_events,
            decoder,
            factors,
        }
    }

    /// # Errors
    ///
    /// Fails closed when the exact curve snapshot, calendar, rule pack, factor topology, time,
    /// owner, Subject, or unit binding is missing or inconsistent.
    #[allow(clippy::too_many_lines)]
    pub async fn execute(
        &self,
        scope: &AccessScope,
        command: CurveRatesCommand,
        trace: SafeTraceContext,
    ) -> ApplicationResult<CurveRatesMaterialization> {
        let common = materialize_common(
            self.definitions,
            self.subjects,
            scope,
            &command.owner,
            &command.subject_ref,
            &command.units,
            &command.knowledge_at,
            "CN",
            "yield-curve",
        )
        .await?;
        require_units(&command.units, [&command.currency_unit, &command.rate_unit])?;
        let curve = materialize_curve(
            self.definitions,
            self.data_sources,
            self.snapshots,
            self.curves,
            self.blobs,
            self.integrity_events,
            self.decoder,
            self.factors,
            scope,
            &command.owner,
            &command.curve,
            &command.knowledge_at,
            &command.currency_unit,
            &command.rate_unit,
            trace,
        )
        .await?;
        let query =
            YieldCurveQuery::new(curve.binding, command.query_date).map_err(map_domain_error)?;
        let mut evidence = common.evidence;
        evidence.extend(curve.evidence);
        let mut parameters = parameter_prefix(
            b"curve",
            &command.knowledge_at,
            curve.snapshot.as_of(),
            CURVE_ALGORITHM_ID,
            CURVE_ALGORITHM_VERSION,
            CURVE_CONVENTION_PROFILE,
        );
        append(&mut parameters, command.query_date.to_string().as_bytes());
        Ok(CurveRatesMaterialization {
            query,
            evidence: RatesRequestEvidence::new(evidence, &parameters)?,
        })
    }
}

pub struct CarryRatesCommand {
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub units: Vec<RatesUnitRequirement>,
    pub currency_unit: UnitRef,
    pub rate_unit: UnitRef,
    pub knowledge_at: MarketTime,
    pub bond: AnalyticsObjectRef,
    pub curve: ImmutableSnapshotBinding,
    pub valuation_at: MarketTime,
    pub initial_settlement: NaiveDate,
    pub horizon_settlement: NaiveDate,
    pub calendar_requirement: CalendarRequirement,
}

pub struct CarryRatesMaterialization {
    input: CarryRollInput,
    evidence: RatesRequestEvidence,
}

impl CarryRatesMaterialization {
    #[must_use]
    pub fn input(&self) -> &CarryRollInput {
        &self.input
    }

    #[must_use]
    pub fn evidence(&self) -> &RatesRequestEvidence {
        &self.evidence
    }
}

pub struct MaterializeCarryRatesInput<'a> {
    definitions: &'a dyn DefinitionRepository,
    subjects: &'a dyn SubjectRepository,
    data_sources: &'a dyn DataSourceRepository,
    snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
    curves: &'a dyn CurveSnapshotMetadataRepository,
    blobs: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
    decoder: &'a dyn CurvePointSetDecoder,
    factors: &'a dyn FactorTopologyRepository,
}

impl<'a> MaterializeCarryRatesInput<'a> {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        definitions: &'a dyn DefinitionRepository,
        subjects: &'a dyn SubjectRepository,
        data_sources: &'a dyn DataSourceRepository,
        snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
        curves: &'a dyn CurveSnapshotMetadataRepository,
        blobs: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
        decoder: &'a dyn CurvePointSetDecoder,
        factors: &'a dyn FactorTopologyRepository,
    ) -> Self {
        Self {
            definitions,
            subjects,
            data_sources,
            snapshots,
            curves,
            blobs,
            integrity_events,
            decoder,
            factors,
        }
    }

    /// # Errors
    ///
    /// Fails closed when the exact Bond and curve authorities cannot be jointly materialized or
    /// their owner, calendar, time, Subject, or unit bindings drift.
    #[allow(clippy::too_many_lines)]
    pub async fn execute(
        &self,
        scope: &AccessScope,
        command: CarryRatesCommand,
        trace: SafeTraceContext,
    ) -> ApplicationResult<CarryRatesMaterialization> {
        let common = materialize_common(
            self.definitions,
            self.subjects,
            scope,
            &command.owner,
            &command.subject_ref,
            &command.units,
            &command.knowledge_at,
            "CN",
            "carry-roll",
        )
        .await?;
        require_units(&command.units, [&command.currency_unit, &command.rate_unit])?;
        let (instrument, bond) =
            read_bond(self.definitions, scope, &command.owner, &command.bond).await?;
        validate_bond_units(
            &instrument,
            &bond,
            &command.currency_unit,
            &command.rate_unit,
        )?;
        let curve = materialize_curve(
            self.definitions,
            self.data_sources,
            self.snapshots,
            self.curves,
            self.blobs,
            self.integrity_events,
            self.decoder,
            self.factors,
            scope,
            &command.owner,
            &command.curve,
            &command.knowledge_at,
            &command.currency_unit,
            &command.rate_unit,
            trace,
        )
        .await?;
        if curve.snapshot.as_of() != &command.valuation_at
            || instrument.instrument().calendar() != curve.snapshot.calendar()
        {
            return Err(lineage());
        }
        let input = CarryRollInput::new(
            command.owner.clone(),
            command.bond.clone(),
            AnalyticsObjectRef::new(
                curve.snapshot.rule_pack().clone(),
                curve.rule_pack.content_hash().clone(),
            ),
            AnalyticsObjectRef::new(
                VersionRef::new(
                    curve.snapshot.id().clone(),
                    Version::new(1).map_err(map_domain_error)?,
                ),
                curve.snapshot.content_hash().clone(),
            ),
            command.valuation_at.clone(),
            command.initial_settlement,
            command.horizon_settlement,
            command.calendar_requirement,
            calendar_binding(
                &curve.calendar,
                definition_content_hash(&DefinitionValue::Calendar(curve.calendar.clone())),
            )?,
            bond_terms(&bond, &command.currency_unit, &command.rate_unit)?,
            curve.binding,
        )
        .map_err(map_domain_error)?;
        let mut evidence = common.evidence;
        evidence.push(object_evidence(
            RatesInputRole::Bond,
            &command.owner,
            command.bond,
            None,
        ));
        evidence.extend(curve.evidence);
        let mut parameters = parameter_prefix(
            b"carry",
            &command.knowledge_at,
            &command.valuation_at,
            CARRY_ROLL_ALGORITHM_ID,
            CARRY_ROLL_ALGORITHM_VERSION,
            CARRY_ROLL_CONVENTION_PROFILE,
        );
        append(
            &mut parameters,
            command.initial_settlement.to_string().as_bytes(),
        );
        append(
            &mut parameters,
            command.horizon_settlement.to_string().as_bytes(),
        );
        append(&mut parameters, &[command.calendar_requirement as u8]);
        Ok(CarryRatesMaterialization {
            input,
            evidence: RatesRequestEvidence::new(evidence, &parameters)?,
        })
    }
}

pub struct DeliveryRatesCommand {
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub units: Vec<RatesUnitRequirement>,
    pub currency_unit: UnitRef,
    pub price_unit: UnitRef,
    pub rate_unit: UnitRef,
    pub knowledge_at: MarketTime,
    pub futures_contract: AnalyticsObjectRef,
    pub data_snapshot: ImmutableSnapshotBinding,
    pub funding_rule_pack: AnalyticsObjectRef,
    pub tax_rule_pack: AnalyticsObjectRef,
    pub valuation_at: MarketTime,
    pub purchase_date: NaiveDate,
}

pub struct DeliveryRatesMaterialization {
    inputs: Vec<FuturesDeliverableInput>,
    funding_rate: FundingRate,
    coupon_tax_treatments: Vec<CouponTaxTreatment>,
    evidence: RatesRequestEvidence,
}

impl DeliveryRatesMaterialization {
    #[must_use]
    pub fn inputs(&self) -> &[FuturesDeliverableInput] {
        &self.inputs
    }

    #[must_use]
    pub fn funding_rate(&self) -> &FundingRate {
        &self.funding_rate
    }

    #[must_use]
    pub fn coupon_tax_treatments(&self) -> &[CouponTaxTreatment] {
        &self.coupon_tax_treatments
    }

    #[must_use]
    pub fn evidence(&self) -> &RatesRequestEvidence {
        &self.evidence
    }
}

pub struct MaterializeDeliveryRatesInput<'a> {
    definitions: &'a dyn DefinitionRepository,
    subjects: &'a dyn SubjectRepository,
    data_sources: &'a dyn DataSourceRepository,
    snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
    blobs: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
    decoder: &'a dyn CanonicalSnapshotDecoder,
    delivery_parser: &'a dyn FuturesDeliveryRuleParser,
    funding_parser: &'a dyn FundingRulePackParser,
    tax_parser: &'a dyn TaxRulePackParser,
}

impl<'a> MaterializeDeliveryRatesInput<'a> {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        definitions: &'a dyn DefinitionRepository,
        subjects: &'a dyn SubjectRepository,
        data_sources: &'a dyn DataSourceRepository,
        snapshots: &'a dyn SnapshotVerifiedReadMetadataRepository,
        blobs: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
        decoder: &'a dyn CanonicalSnapshotDecoder,
        delivery_parser: &'a dyn FuturesDeliveryRuleParser,
        funding_parser: &'a dyn FundingRulePackParser,
        tax_parser: &'a dyn TaxRulePackParser,
    ) -> Self {
        Self {
            definitions,
            subjects,
            data_sources,
            snapshots,
            blobs,
            integrity_events,
            decoder,
            delivery_parser,
            funding_parser,
            tax_parser,
        }
    }

    #[allow(clippy::too_many_lines)]
    /// # Errors
    ///
    /// Fails closed when the contract-derived product, delivery rule, candidates, prices,
    /// snapshot/source, funding pack, Subject, time, owner, or unit bindings cannot be verified.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        command: DeliveryRatesCommand,
        trace: SafeTraceContext,
    ) -> ApplicationResult<DeliveryRatesMaterialization> {
        let common = materialize_common(
            self.definitions,
            self.subjects,
            scope,
            &command.owner,
            &command.subject_ref,
            &command.units,
            &command.knowledge_at,
            self.delivery_parser.market(),
            "futures-delivery",
        )
        .await?;
        require_units(&command.units, [&command.price_unit, &command.rate_unit])?;
        validate_authoritative_rate_unit(
            self.definitions,
            scope,
            &command.owner,
            &command.rate_unit,
            self.tax_parser,
        )
        .await?;
        let contract_value = read_definition(
            self.definitions,
            scope,
            &command.owner,
            &command.futures_contract,
        )
        .await?;
        let DefinitionValue::Instrument(contract_definition) = contract_value else {
            return Err(lineage());
        };
        let Some(InstrumentSubtype::FuturesContract(contract)) = contract_definition.subtype()
        else {
            return Err(lineage());
        };
        if contract_definition.instrument().currency() != &command.currency_unit
            || contract.price_unit() != Some(&command.price_unit)
        {
            return Err(map_domain_error(DomainErrorCode::InvalidUnit));
        }
        let snapshot = read_data_snapshot(
            self.snapshots,
            self.blobs,
            self.integrity_events,
            scope,
            &command.owner,
            &command.data_snapshot,
            &command.valuation_at,
            &command.knowledge_at,
            trace.clone(),
        )
        .await?;
        let registered = MaterializeRegisteredFuturesDelivery::new(
            self.definitions,
            self.data_sources,
            self.snapshots,
            self.blobs,
            self.integrity_events,
            self.decoder,
            self.delivery_parser,
        )
        .execute(
            scope,
            &command.owner,
            command.futures_contract.version_ref(),
            command.data_snapshot.id().clone(),
            &command.valuation_at,
            &command.knowledge_at,
            trace,
        )
        .await?;
        let first = registered.inputs().first().ok_or_else(invalid)?;
        if first.futures_contract() != &command.futures_contract
            || first.snapshot().version_ref().id() != command.data_snapshot.id()
            || first.snapshot().content_hash() != command.data_snapshot.content_hash()
        {
            return Err(lineage());
        }
        let funding_pack = read_rule_pack(
            self.definitions,
            scope,
            &command.owner,
            &command.funding_rule_pack,
        )
        .await?;
        let funding_payload_binding = AnalyticsObjectRef::new(
            command.funding_rule_pack.version_ref().clone(),
            funding_pack.content_hash().clone(),
        );
        let funding_rate = ResolveFundingRule::new(self.definitions, self.funding_parser)
            .execute(
                scope,
                &funding_payload_binding,
                command.valuation_at.clone(),
                common.subject.funding_tier(),
            )
            .await?;
        if funding_rate.unit() != &command.rate_unit {
            return Err(map_domain_error(DomainErrorCode::InvalidUnit));
        }
        let tax_pack = read_rule_pack(
            self.definitions,
            scope,
            &command.owner,
            &command.tax_rule_pack,
        )
        .await?;
        let tax_payload_binding = AnalyticsObjectRef::new(
            command.tax_rule_pack.version_ref().clone(),
            tax_pack.content_hash().clone(),
        );
        let mut materialized_candidates = Vec::with_capacity(registered.inputs().len());
        for value in registered.inputs() {
            let (definition, bond) =
                read_bond(self.definitions, scope, &command.owner, value.bond()).await?;
            validate_bond_units(
                &definition,
                &bond,
                &command.currency_unit,
                &command.rate_unit,
            )?;
            let terms = bond_terms(&bond, &command.currency_unit, &command.rate_unit)?;
            let tax_attributes = terms.tax_attributes().ok_or_else(lineage)?;
            let coupon_tax_treatment = ResolveTaxRule::new(self.definitions, self.tax_parser)
                .parse_verified(
                    scope,
                    &tax_payload_binding,
                    &command.valuation_at,
                    &tax_pack,
                    terms.first_issue_date(),
                    tax_attributes,
                    common.subject.tax_treatment(),
                )?;
            let input = FuturesDeliverableInput::new(
                value.owner().clone(),
                value.futures_contract().clone(),
                value.bond().clone(),
                value.rule_pack().clone(),
                value.snapshot().clone(),
                value.valuation_at().clone(),
                command.purchase_date,
                value.delivery_month_first(),
                value.delivery_date(),
                value.product(),
                value.rule().clone(),
                value.terms().clone(),
                value.spot_clean_price(),
                value.futures_clean_price(),
                funding_rate.annual_financing_rate(),
            )
            .map_err(map_domain_error)?;
            materialized_candidates.push((input, coupon_tax_treatment));
        }
        materialized_candidates.sort_by(|left, right| {
            left.0
                .bond()
                .version_ref()
                .cmp(right.0.bond().version_ref())
        });
        let (inputs, coupon_tax_treatments): (Vec<_>, Vec<_>) =
            materialized_candidates.into_iter().unzip();
        // The delivery engine deliberately carries the parsed RulePack payload hash. Public exact
        // object bindings and response evidence bind the complete versioned definition instead.
        // Resolve both identities here and require them to describe the same immutable pack.
        let delivery_rule_binding = AnalyticsObjectRef::new(
            first.rule_pack().version_ref().clone(),
            definition_hash_for_reference(self.definitions, scope, first.rule_pack().version_ref())
                .await?,
        );
        let delivery_rule = read_rule_pack(
            self.definitions,
            scope,
            &command.owner,
            &delivery_rule_binding,
        )
        .await?;
        if delivery_rule.content_hash() != first.rule_pack().content_hash() {
            return Err(lineage());
        }
        let source = self
            .data_sources
            .get_exact(scope, registered.data_source_ref().clone())
            .await?
            .ok_or_else(not_found)?;
        let source_hash = rates_data_source_content_hash(&source);
        if source.owner() != &command.owner
            || source.id() != registered.data_source_ref().id()
            || source.version() != registered.data_source_ref().version().get()
            || source.price_source_type() != Some(PriceSourceType::ActiveQuote)
            || !snapshot.lineage().iter().any(|lineage| {
                lineage.object_id() == source.id()
                    && lineage.version() == Some(registered.data_source_ref().version())
                    && lineage.content_hash() == Some(&source_hash)
            })
        {
            return Err(lineage());
        }
        let mut evidence = common.evidence;
        evidence.push(object_evidence(
            RatesInputRole::FuturesContract,
            &command.owner,
            command.futures_contract,
            Some(command.valuation_at.clone()),
        ));
        evidence.push(data_snapshot_evidence(&snapshot));
        evidence.push(rule_pack_evidence(
            RatesInputRole::DeliveryRulePack,
            &delivery_rule,
        ));
        evidence.push(rule_pack_evidence(RatesInputRole::TaxRulePack, &tax_pack));
        evidence.push(rule_pack_evidence(
            RatesInputRole::FundingRulePack,
            &funding_pack,
        ));
        evidence.push(data_source_evidence(&source));
        for input in &inputs {
            evidence.push(object_evidence(
                RatesInputRole::Bond,
                &command.owner,
                input.bond().clone(),
                Some(command.valuation_at.clone()),
            ));
        }
        let mut parameters = parameter_prefix(
            b"delivery",
            &command.knowledge_at,
            &command.valuation_at,
            FUTURES_DELIVERY_ALGORITHM_ID,
            FUTURES_DELIVERY_ALGORITHM_VERSION,
            FUTURES_DELIVERY_CONVENTION_PROFILE,
        );
        append(
            &mut parameters,
            command.purchase_date.to_string().as_bytes(),
        );
        for treatment in &coupon_tax_treatments {
            append(&mut parameters, &treatment.fingerprint_bytes());
        }
        Ok(DeliveryRatesMaterialization {
            inputs,
            funding_rate,
            coupon_tax_treatments,
            evidence: RatesRequestEvidence::new(evidence, &parameters)?,
        })
    }
}

pub struct HedgeRatesCommand {
    pub owner: OwnerRef,
    pub subject_ref: VersionRef,
    pub units: Vec<RatesUnitRequirement>,
    pub knowledge_at: MarketTime,
    pub target_risk_artifact: ImmutableArtifactBinding,
    pub delivery_artifact: ImmutableArtifactBinding,
    pub ctd_analytics_artifact: ImmutableArtifactBinding,
    pub futures_contract: AnalyticsObjectRef,
    pub valuation_at: MarketTime,
}

pub struct HedgeRatesMaterialization {
    input: FuturesHedgeInput,
    evidence: RatesRequestEvidence,
}

impl HedgeRatesMaterialization {
    #[must_use]
    pub fn input(&self) -> &FuturesHedgeInput {
        &self.input
    }

    #[must_use]
    pub fn evidence(&self) -> &RatesRequestEvidence {
        &self.evidence
    }
}

pub struct MaterializeHedgeRatesInput<'a> {
    definitions: &'a dyn DefinitionRepository,
    subjects: &'a dyn SubjectRepository,
    artifacts: &'a dyn ArtifactRepository,
    blobs: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
    bond_codec: &'a dyn BondAnalyticsArtifactCodec,
    delivery_codec: &'a dyn FuturesDeliveryArtifactCodec,
    delivery_parser: &'a dyn FuturesDeliveryRuleParser,
}

impl<'a> MaterializeHedgeRatesInput<'a> {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        definitions: &'a dyn DefinitionRepository,
        subjects: &'a dyn SubjectRepository,
        artifacts: &'a dyn ArtifactRepository,
        blobs: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
        bond_codec: &'a dyn BondAnalyticsArtifactCodec,
        delivery_codec: &'a dyn FuturesDeliveryArtifactCodec,
        delivery_parser: &'a dyn FuturesDeliveryRuleParser,
    ) -> Self {
        Self {
            definitions,
            subjects,
            artifacts,
            blobs,
            integrity_events,
            bond_codec,
            delivery_codec,
            delivery_parser,
        }
    }

    #[allow(clippy::too_many_lines)]
    /// # Errors
    ///
    /// Fails closed when any exact Artifact, contract, CTD Bond, payload fact, lineage, owner,
    /// time, Subject, or unit binding cannot be jointly verified.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        command: HedgeRatesCommand,
        trace: SafeTraceContext,
    ) -> ApplicationResult<HedgeRatesMaterialization> {
        let common = materialize_common(
            self.definitions,
            self.subjects,
            scope,
            &command.owner,
            &command.subject_ref,
            &command.units,
            &command.knowledge_at,
            self.delivery_parser.market(),
            "futures-hedge",
        )
        .await?;
        if command.knowledge_at.instant() < command.valuation_at.instant() {
            return Err(lineage());
        }
        let target = read_artifact(
            self.artifacts,
            self.blobs,
            self.integrity_events,
            scope,
            &command.owner,
            &command.target_risk_artifact,
            BOND_ANALYTICS_MEDIA_TYPE,
            trace.clone(),
        )
        .await?;
        let delivery = read_artifact(
            self.artifacts,
            self.blobs,
            self.integrity_events,
            scope,
            &command.owner,
            &command.delivery_artifact,
            FUTURES_DELIVERY_MEDIA_TYPE,
            trace.clone(),
        )
        .await?;
        let ctd = read_artifact(
            self.artifacts,
            self.blobs,
            self.integrity_events,
            scope,
            &command.owner,
            &command.ctd_analytics_artifact,
            BOND_ANALYTICS_MEDIA_TYPE,
            trace,
        )
        .await?;
        let target_facts = self
            .bond_codec
            .decode_facts(target.1.as_slice())
            .map_err(map_analytics_error)?;
        let delivery_facts = self
            .delivery_codec
            .decode_facts(delivery.1.as_slice())
            .map_err(map_analytics_error)?;
        let ctd_facts = self
            .bond_codec
            .decode_facts(ctd.1.as_slice())
            .map_err(map_analytics_error)?;
        validate_bond_artifact_lineage(&target.0, &target_facts)?;
        validate_delivery_artifact_lineage(&delivery.0, &delivery_facts)?;
        validate_bond_artifact_lineage(&ctd.0, &ctd_facts)?;
        let ctd_delivery = delivery_facts.ctd().ok_or_else(lineage)?;
        if target_facts.valuation_at() != &command.valuation_at
            || delivery_facts.valuation_at() != &command.valuation_at
            || ctd_facts.valuation_at() != &command.valuation_at
            || delivery_facts.futures_contract() != &command.futures_contract
            || ctd_delivery.bond() != ctd_facts.bond()
            || target_facts.snapshot() != delivery_facts.snapshot()
            || delivery_facts.snapshot() != ctd_facts.snapshot()
        {
            return Err(lineage());
        }
        let contract_value = read_definition(
            self.definitions,
            scope,
            &command.owner,
            &command.futures_contract,
        )
        .await?;
        let DefinitionValue::Instrument(contract_definition) = contract_value else {
            return Err(lineage());
        };
        let Some(InstrumentSubtype::FuturesContract(contract)) = contract_definition.subtype()
        else {
            return Err(lineage());
        };
        let product = self
            .delivery_parser
            .parse_product_code(contract.product_code().ok_or_else(lineage)?)?;
        if product != delivery_facts.product()
            || contract.rule_pack() != delivery_facts.rule_pack().version_ref()
        {
            return Err(lineage());
        }
        let delivery_rule_binding = AnalyticsObjectRef::new(
            contract.rule_pack().clone(),
            definition_hash_for_reference(self.definitions, scope, contract.rule_pack()).await?,
        );
        let delivery_rule_pack = read_rule_pack(
            self.definitions,
            scope,
            &command.owner,
            &delivery_rule_binding,
        )
        .await?;
        if delivery_rule_pack.content_hash() != delivery_facts.rule_pack().content_hash()
            || delivery_rule_pack.market() != self.delivery_parser.market()
            || delivery_rule_pack.rule_type() != self.delivery_parser.rule_type()
            || delivery_rule_pack.effective().from().instant() > command.valuation_at.instant()
            || command.valuation_at.instant() >= delivery_rule_pack.effective().to().instant()
        {
            return Err(lineage());
        }
        let delivery_content = delivery_rule_pack.content().ok_or_else(lineage)?;
        if delivery_content.type_url() != self.delivery_parser.type_url() {
            return Err(lineage());
        }
        delivery_rule_pack
            .content_hash()
            .verify(delivery_content.value())
            .map_err(map_domain_error)?;
        self.delivery_parser.parse(delivery_content, product)?;
        read_bond(self.definitions, scope, &command.owner, ctd_delivery.bond()).await?;
        let input = FuturesHedgeInput::new(
            command.owner.clone(),
            artifact_object_ref(&command.target_risk_artifact)?,
            artifact_object_ref(&command.delivery_artifact)?,
            artifact_object_ref(&command.ctd_analytics_artifact)?,
            command.futures_contract.clone(),
            ctd_delivery.bond().clone(),
            delivery_facts.rule_pack().clone(),
            delivery_facts.snapshot().clone(),
            command.valuation_at.clone(),
            product,
            target_facts.dv01(),
            ctd_facts.dv01(),
            ctd_delivery.conversion_factor(),
        )
        .map_err(map_domain_error)?;
        let mut evidence = common.evidence;
        evidence.extend([
            artifact_evidence(
                RatesInputRole::TargetRiskArtifact,
                &command.owner,
                command.target_risk_artifact,
                &command.valuation_at,
            ),
            artifact_evidence(
                RatesInputRole::DeliveryArtifact,
                &command.owner,
                command.delivery_artifact,
                &command.valuation_at,
            ),
            artifact_evidence(
                RatesInputRole::CtdAnalyticsArtifact,
                &command.owner,
                command.ctd_analytics_artifact,
                &command.valuation_at,
            ),
            object_evidence(
                RatesInputRole::FuturesContract,
                &command.owner,
                command.futures_contract,
                Some(command.valuation_at.clone()),
            ),
            object_evidence(
                RatesInputRole::Bond,
                &command.owner,
                ctd_delivery.bond().clone(),
                Some(command.valuation_at.clone()),
            ),
            rule_pack_evidence(RatesInputRole::DeliveryRulePack, &delivery_rule_pack),
        ]);
        let mut parameters = parameter_prefix(
            b"hedge",
            &command.knowledge_at,
            &command.valuation_at,
            FUTURES_HEDGE_ALGORITHM_ID,
            FUTURES_HEDGE_ALGORITHM_VERSION,
            FUTURES_HEDGE_CONVENTION_PROFILE,
        );
        append(&mut parameters, &target_facts.dv01().scaled().to_be_bytes());
        append(&mut parameters, &ctd_facts.dv01().scaled().to_be_bytes());
        append(
            &mut parameters,
            &ctd_delivery.conversion_factor().scaled().to_be_bytes(),
        );
        Ok(HedgeRatesMaterialization {
            input,
            evidence: RatesRequestEvidence::new(evidence, &parameters)?,
        })
    }
}

struct MaterializedCurve {
    snapshot: ficant_domain::market::CurveSnapshot,
    calendar: Calendar,
    rule_pack: MarketRulePack,
    binding: YieldCurveBinding,
    evidence: Vec<RatesInputEvidence>,
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
async fn materialize_curve(
    definitions: &dyn DefinitionRepository,
    data_sources: &dyn DataSourceRepository,
    snapshots: &dyn SnapshotVerifiedReadMetadataRepository,
    curves: &dyn CurveSnapshotMetadataRepository,
    blobs: &dyn VerifiedBlobReader,
    integrity_events: &dyn IntegrityEventSink,
    decoder: &dyn CurvePointSetDecoder,
    factors: &dyn FactorTopologyRepository,
    scope: &AccessScope,
    owner: &OwnerRef,
    binding: &ImmutableSnapshotBinding,
    knowledge_at: &MarketTime,
    expected_currency_unit: &UnitRef,
    expected_rate_unit: &UnitRef,
    trace: SafeTraceContext,
) -> ApplicationResult<MaterializedCurve> {
    let metadata = curves
        .get_curve_snapshot_metadata(scope, binding.id().clone())
        .await?
        .ok_or_else(not_found)?;
    let snapshot = metadata.snapshot().clone();
    scope.authorize(snapshot.owner())?;
    let visible = snapshot.visible_at().ok_or_else(lineage)?;
    if snapshot.id() != binding.id()
        || snapshot.owner() != owner
        || snapshot.content_hash() != binding.content_hash()
        || snapshot.currency() != expected_currency_unit
        || snapshot.curve_kind() != "YTM"
        || snapshot.point_schema() != crate::ports::CURVE_POINT_SCHEMA
        || visible.instant() < snapshot.as_of().instant()
        || visible.instant() > knowledge_at.instant()
    {
        return Err(lineage());
    }
    let read = RequiredVerifiedBlobRead::new(
        scope.clone(),
        owner.clone(),
        VerifiedReadResourceKind::CurveSnapshot,
        snapshot.id().clone(),
        VerifiedBlobRole::CurvePoints,
        snapshot.content_hash().clone(),
        metadata.blob_size(),
        trace.clone(),
    )?;
    let payload = blobs.read_required(&read, integrity_events).await?;
    let points = decoder.decode_canonical(payload.bytes())?;
    if Some(points.curve_family_id()) != snapshot.curve_family_id() {
        return Err(lineage());
    }
    let mut nodes = Vec::with_capacity(points.points().len());
    let mut factor_evidence = Vec::with_capacity(points.points().len());
    for point in points.points() {
        let definition = factors
            .get_curve_node_definition(point.curve_node_id())
            .await?
            .ok_or_else(not_found)?;
        if point.yield_to_maturity().unit() != expected_rate_unit
            || definition.content_hash() != point.curve_node_content_hash()
            || definition.curve_family_id() != points.curve_family_id()
            || definition.factor_unit() != expected_rate_unit
        {
            return Err(lineage());
        }
        nodes.push(
            YieldCurveNode::new(
                tenor_date(snapshot.as_of().local_trading_date(), definition.tenor())?,
                decimal_to_fixed(point.yield_to_maturity())?,
            )
            .map_err(map_domain_error)?,
        );
        factor_evidence.push(curve_node_definition_evidence(owner, &definition));
    }
    nodes.sort_by_key(|value| value.maturity_date());
    let curve_ref = AnalyticsObjectRef::new(
        VersionRef::new(
            snapshot.id().clone(),
            Version::new(1).map_err(map_domain_error)?,
        ),
        snapshot.content_hash().clone(),
    );
    let curve_binding = YieldCurveBinding::new(
        curve_ref,
        snapshot.as_of().local_trading_date(),
        YieldCurveInterpolation::LinearYield,
        nodes,
    )
    .map_err(map_domain_error)?;
    let calendar_ref = AnalyticsObjectRef::new(
        snapshot.calendar().clone(),
        definition_hash_for_reference(definitions, scope, snapshot.calendar()).await?,
    );
    let calendar = read_calendar(definitions, scope, owner, &calendar_ref).await?;
    let rule_ref = AnalyticsObjectRef::new(
        snapshot.rule_pack().clone(),
        definition_hash_for_reference(definitions, scope, snapshot.rule_pack()).await?,
    );
    let rule_pack = read_rule_pack(definitions, scope, owner, &rule_ref).await?;
    let rule_content = rule_pack.content().ok_or_else(lineage)?;
    if calendar.market() != "CN"
        || calendar.market_timezone() != ficant_domain::analytics::MARKET_TIMEZONE
        || calendar.effective().from().instant() > snapshot.as_of().instant()
        || snapshot.as_of().instant() >= calendar.effective().to().instant()
        || rule_pack.effective().from().instant() > snapshot.as_of().instant()
        || snapshot.as_of().instant() >= rule_pack.effective().to().instant()
        || rule_pack.verification_status() != VerificationStatus::Verified
        || rule_pack.market() != "CN"
        || rule_pack.rule_type() != "yield-curve"
        || rule_content.type_url() != "type.googleapis.com/ficant.market.v1.CurveRulePack"
    {
        return Err(lineage());
    }
    rule_pack
        .content_hash()
        .verify(rule_content.value())
        .map_err(map_domain_error)?;
    let mut evidence = vec![
        RatesInputEvidence {
            role: RatesInputRole::CurveSnapshot,
            owner: owner.clone(),
            binding: RatesEvidenceBinding::Snapshot(binding.clone()),
            observed_at: Some(snapshot.as_of().clone()),
            visible_at: Some(visible.clone()),
            effective_from: None,
            effective_to: None,
        },
        calendar_evidence(&calendar),
        rule_pack_evidence(RatesInputRole::CurveRulePack, &rule_pack),
    ];
    evidence.append(&mut factor_evidence);
    let mut has_data_source = false;
    let mut has_data_snapshot = false;
    let mut data_source_refs = Vec::new();
    let mut data_snapshot_sources = Vec::new();
    for source in snapshot.lineage() {
        let hash = source.content_hash().ok_or_else(lineage)?.clone();
        if let Some(version) = source.version() {
            let reference = VersionRef::new(source.object_id().clone(), version);
            let value = data_sources
                .get_exact(scope, reference.clone())
                .await?
                .ok_or_else(not_found)?;
            scope.authorize(value.owner())?;
            if value.owner() != owner
                || value.id() != reference.id()
                || value.version() != reference.version().get()
                || value.price_source_type().is_none()
                || rates_data_source_content_hash(&value) != hash
            {
                return Err(lineage());
            }
            evidence.push(data_source_evidence(&value));
            data_source_refs.push(AnalyticsObjectRef::new(reference, hash));
            has_data_source = true;
        } else {
            let snapshot_binding = ImmutableSnapshotBinding::new(source.object_id().clone(), hash);
            let value = read_data_snapshot(
                snapshots,
                blobs,
                integrity_events,
                scope,
                owner,
                &snapshot_binding,
                snapshot.as_of(),
                knowledge_at,
                trace.clone(),
            )
            .await?;
            data_snapshot_sources.extend(value.lineage().iter().filter_map(|source| {
                source
                    .version()
                    .zip(source.content_hash())
                    .map(|(version, hash)| {
                        AnalyticsObjectRef::new(
                            VersionRef::new(source.object_id().clone(), version),
                            hash.clone(),
                        )
                    })
            }));
            evidence.push(data_snapshot_evidence(&value));
            has_data_snapshot = true;
        }
    }
    if !has_data_source
        || !has_data_snapshot
        || data_source_refs
            .iter()
            .any(|source| !data_snapshot_sources.contains(source))
    {
        return Err(lineage());
    }
    Ok(MaterializedCurve {
        snapshot,
        calendar,
        rule_pack,
        binding: curve_binding,
        evidence,
    })
}

#[allow(clippy::too_many_arguments)]
async fn materialize_common(
    definitions: &dyn DefinitionRepository,
    subjects: &dyn SubjectRepository,
    scope: &AccessScope,
    owner: &OwnerRef,
    subject_ref: &VersionRef,
    units: &[RatesUnitRequirement],
    knowledge_at: &MarketTime,
    market: &str,
    tool: &str,
) -> ApplicationResult<CommonMaterialization> {
    scope.authorize(owner)?;
    if owner.tenant_id() != scope.tenant_id() || knowledge_at.market_timezone() != "Asia/Shanghai" {
        return Err(lineage());
    }
    let record = subjects
        .get_subject(subject_ref.clone())
        .await?
        .ok_or_else(ApplicationError::subject_binding_invalid)?;
    if record.subject().id() != subject_ref.id()
        || record.version().reference() != subject_ref
        || record
            .version()
            .access_set()
            .market_codes()
            .binary_search_by(|value| value.as_str().cmp(market))
            .is_err()
        || record
            .version()
            .access_set()
            .tool_codes()
            .binary_search_by(|value| value.as_str().cmp(tool))
            .is_err()
    {
        return Err(ApplicationError::subject_binding_invalid());
    }
    let mut evidence = vec![object_evidence(
        RatesInputRole::Subject,
        owner,
        AnalyticsObjectRef::new(subject_ref.clone(), subject_hash(&record)),
        None,
    )];
    let mut unique = BTreeSet::new();
    for requirement in units {
        let unit_ref = requirement.reference();
        if !unique.insert(unit_ref.clone()) {
            return Err(map_domain_error(DomainErrorCode::InvalidUnit));
        }
        let unit = read_unit(definitions, scope, owner, unit_ref).await?;
        if unit.dimension() != requirement.expected_dimension() {
            return Err(map_domain_error(DomainErrorCode::InvalidUnit));
        }
        evidence.push(object_evidence(
            RatesInputRole::Unit,
            owner,
            AnalyticsObjectRef::new(
                VersionRef::new(unit_ref.unit_id().clone(), unit_ref.version()),
                definition_content_hash(&DefinitionValue::Unit(unit)),
            ),
            None,
        ));
    }
    if unique.is_empty() {
        return Err(invalid());
    }
    Ok(CommonMaterialization {
        subject: record.version().clone(),
        evidence,
    })
}

async fn read_bond(
    definitions: &dyn DefinitionRepository,
    scope: &AccessScope,
    owner: &OwnerRef,
    binding: &AnalyticsObjectRef,
) -> ApplicationResult<(crate::ports::InstrumentDefinition, Bond)> {
    let value = read_definition(definitions, scope, owner, binding).await?;
    let DefinitionValue::Instrument(instrument) = value else {
        return Err(lineage());
    };
    let Some(InstrumentSubtype::Bond(bond)) = instrument.subtype() else {
        return Err(lineage());
    };
    Ok((instrument.clone(), bond.clone()))
}

async fn read_calendar(
    definitions: &dyn DefinitionRepository,
    scope: &AccessScope,
    owner: &OwnerRef,
    binding: &AnalyticsObjectRef,
) -> ApplicationResult<Calendar> {
    match read_definition(definitions, scope, owner, binding).await? {
        DefinitionValue::Calendar(value) => Ok(value),
        _ => Err(lineage()),
    }
}

async fn read_rule_pack(
    definitions: &dyn DefinitionRepository,
    scope: &AccessScope,
    owner: &OwnerRef,
    binding: &AnalyticsObjectRef,
) -> ApplicationResult<MarketRulePack> {
    match read_definition(definitions, scope, owner, binding).await? {
        DefinitionValue::MarketRulePack(value)
            if value.verification_status() == VerificationStatus::Verified =>
        {
            Ok(value)
        }
        _ => Err(lineage()),
    }
}

async fn read_unit(
    definitions: &dyn DefinitionRepository,
    scope: &AccessScope,
    owner: &OwnerRef,
    reference: &UnitRef,
) -> ApplicationResult<Unit> {
    let binding = AnalyticsObjectRef::new(
        VersionRef::new(reference.unit_id().clone(), reference.version()),
        definition_hash_for_reference(
            definitions,
            scope,
            &VersionRef::new(reference.unit_id().clone(), reference.version()),
        )
        .await?,
    );
    match read_definition(definitions, scope, owner, &binding).await? {
        DefinitionValue::Unit(value) => Ok(value),
        _ => Err(lineage()),
    }
}

async fn validate_authoritative_rate_unit(
    definitions: &dyn DefinitionRepository,
    scope: &AccessScope,
    owner: &OwnerRef,
    rate_unit: &UnitRef,
    parser: &dyn TaxRulePackParser,
) -> ApplicationResult<()> {
    let Some((unit_id, version, code, dimension, scale, precision)) = parser.expected_rate_unit()
    else {
        return Ok(());
    };
    if rate_unit.unit_id().as_str() != unit_id || rate_unit.version().get() != version {
        return Err(map_domain_error(DomainErrorCode::InvalidUnit));
    }
    let unit = read_unit(definitions, scope, owner, rate_unit).await?;
    if unit.identity() != unit_id
        || unit.version() != version
        || unit.owner() != owner
        || unit.code() != code
        || unit.dimension() != dimension
        || unit.scale() != scale
        || unit.precision() != precision
    {
        return Err(map_domain_error(DomainErrorCode::InvalidUnit));
    }
    Ok(())
}

async fn definition_hash_for_reference(
    definitions: &dyn DefinitionRepository,
    scope: &AccessScope,
    reference: &VersionRef,
) -> ApplicationResult<ContentHash> {
    let value = definitions
        .get_version(scope, reference.id().clone(), reference.version())
        .await?
        .ok_or_else(not_found)?;
    if value.identity() != reference.id().as_str() || value.version() != reference.version().get() {
        return Err(lineage());
    }
    Ok(definition_content_hash(&value))
}

async fn read_definition(
    definitions: &dyn DefinitionRepository,
    scope: &AccessScope,
    owner: &OwnerRef,
    binding: &AnalyticsObjectRef,
) -> ApplicationResult<DefinitionValue> {
    let value = definitions
        .get_version(
            scope,
            binding.version_ref().id().clone(),
            binding.version_ref().version(),
        )
        .await?
        .ok_or_else(not_found)?;
    scope.authorize(value.owner())?;
    if value.owner() != owner
        || value.identity() != binding.version_ref().id().as_str()
        || value.version() != binding.version_ref().version().get()
        || definition_content_hash(&value) != *binding.content_hash()
    {
        return Err(lineage());
    }
    Ok(value)
}

#[allow(clippy::too_many_arguments)]
async fn read_data_snapshot(
    snapshots: &dyn SnapshotVerifiedReadMetadataRepository,
    blobs: &dyn VerifiedBlobReader,
    integrity_events: &dyn IntegrityEventSink,
    scope: &AccessScope,
    owner: &OwnerRef,
    binding: &ImmutableSnapshotBinding,
    valuation_at: &MarketTime,
    knowledge_at: &MarketTime,
    trace: SafeTraceContext,
) -> ApplicationResult<DataSnapshot> {
    let verified = VerifiedSnapshotReader::new(snapshots, blobs, integrity_events)
        .read(scope, binding.id().clone(), trace)
        .await?;
    let VerifiedSnapshotRead::Data { snapshot, .. } = verified else {
        return Err(lineage());
    };
    if snapshot.owner() != owner
        || snapshot.id() != binding.id()
        || snapshot.content_hash() != binding.content_hash()
        || snapshot.as_of() != valuation_at
        || snapshot.visible_at().instant() < snapshot.as_of().instant()
        || snapshot.visible_at().instant() > knowledge_at.instant()
    {
        return Err(lineage());
    }
    Ok(snapshot)
}

fn validate_bond_units(
    definition: &crate::ports::InstrumentDefinition,
    bond: &Bond,
    currency_unit: &UnitRef,
    rate_unit: &UnitRef,
) -> ApplicationResult<()> {
    let pricing = bond.pricing_terms().ok_or_else(lineage)?;
    if definition.instrument().currency() != currency_unit
        || bond.face_value().unit() != currency_unit
        || bond.cumulative_issued_amount().unit() != currency_unit
        || pricing.coupon_rate().unit() != rate_unit
    {
        return Err(map_domain_error(DomainErrorCode::InvalidUnit));
    }
    Ok(())
}

fn bond_terms(
    bond: &Bond,
    currency_unit: &UnitRef,
    rate_unit: &UnitRef,
) -> ApplicationResult<BondTerms> {
    if bond.face_value().unit() != currency_unit
        || bond.cumulative_issued_amount().unit() != currency_unit
    {
        return Err(map_domain_error(DomainErrorCode::InvalidUnit));
    }
    let pricing = bond.pricing_terms().ok_or_else(lineage)?;
    if pricing.coupon_rate().unit() != rate_unit {
        return Err(map_domain_error(DomainErrorCode::InvalidUnit));
    }
    let tax = bond.tax_attributes().ok_or_else(lineage)?;
    BondTerms::with_issuance(
        bond.first_issue_date(),
        bond.current_issue_date(),
        bond.maturity_date(),
        match pricing.frequency() {
            BondCouponFrequency::Annual => CouponFrequency::Annual,
            BondCouponFrequency::Semiannual => CouponFrequency::Semiannual,
        },
        match pricing.day_count() {
            BondDayCountConvention::ActActBondIsma => DayCountConvention::ActActBondIsma,
        },
        match pricing.business_day() {
            BondBusinessDayConvention::Following => BusinessDayConvention::Following,
        },
        decimal_to_fixed(pricing.coupon_rate())?,
        decimal_to_fixed(bond.face_value())?,
        decimal_to_fixed(bond.cumulative_issued_amount())?,
        tax,
    )
    .map_err(map_domain_error)
}

fn decimal_to_fixed(
    value: &ficant_domain::primitives::DecimalValue,
) -> ApplicationResult<FixedDecimal> {
    if value.scale() > 12 {
        return Err(invalid());
    }
    let coefficient = value.coefficient().parse::<i128>().map_err(|_| invalid())?;
    let factor = 10_i128
        .checked_pow(12 - value.scale())
        .ok_or_else(invalid)?;
    Ok(FixedDecimal::from_scaled(
        coefficient.checked_mul(factor).ok_or_else(invalid)?,
    ))
}

fn snapshot_object_ref(snapshot: &DataSnapshot) -> ApplicationResult<AnalyticsObjectRef> {
    Ok(AnalyticsObjectRef::new(
        VersionRef::new(
            snapshot.id().clone(),
            Version::new(1).map_err(map_domain_error)?,
        ),
        snapshot.content_hash().clone(),
    ))
}

fn calendar_binding(calendar: &Calendar, hash: ContentHash) -> ApplicationResult<CalendarBinding> {
    let mut non_business_days = Vec::new();
    let mut work_weekends = Vec::new();
    for session in calendar.sessions() {
        if session.open_local_time().is_none() {
            non_business_days.push(session.local_date());
        } else if matches!(
            session.local_date().weekday(),
            chrono::Weekday::Sat | chrono::Weekday::Sun
        ) {
            work_weekends.push(session.local_date());
        }
    }
    CalendarBinding::new(
        calendar.identity(),
        Version::new(calendar.version()).map_err(map_domain_error)?,
        hash,
        calendar.effective().from().local_trading_date(),
        calendar.effective().to().local_trading_date(),
        non_business_days,
        work_weekends,
    )
    .map_err(map_domain_error)
}

fn tenor_date(as_of: NaiveDate, tenor: &str) -> ApplicationResult<NaiveDate> {
    let amount = tenor
        .get(1..tenor.len().saturating_sub(1))
        .ok_or_else(invalid)?
        .parse::<u32>()
        .map_err(|_| invalid())?;
    match tenor.as_bytes().last() {
        Some(b'Y') => as_of
            .checked_add_months(Months::new(amount.checked_mul(12).ok_or_else(invalid)?))
            .ok_or_else(invalid),
        Some(b'M') => as_of
            .checked_add_months(Months::new(amount))
            .ok_or_else(invalid),
        Some(b'D') => as_of
            .checked_add_days(Days::new(u64::from(amount)))
            .ok_or_else(invalid),
        _ => Err(invalid()),
    }
}

fn object_evidence(
    role: RatesInputRole,
    owner: &OwnerRef,
    binding: AnalyticsObjectRef,
    observed_at: Option<MarketTime>,
) -> RatesInputEvidence {
    RatesInputEvidence {
        role,
        owner: owner.clone(),
        binding: RatesEvidenceBinding::Object(binding),
        observed_at,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    }
}

fn calendar_evidence(calendar: &Calendar) -> RatesInputEvidence {
    RatesInputEvidence {
        role: RatesInputRole::Calendar,
        owner: calendar.owner().clone(),
        binding: RatesEvidenceBinding::Object(AnalyticsObjectRef::new(
            VersionRef::new(
                Ulid::new(calendar.identity()).expect("validated Calendar identity"),
                Version::new(calendar.version()).expect("validated Calendar version"),
            ),
            definition_content_hash(&DefinitionValue::Calendar(calendar.clone())),
        )),
        observed_at: None,
        visible_at: None,
        effective_from: Some(calendar.effective().from().clone()),
        effective_to: Some(calendar.effective().to().clone()),
    }
}

fn rule_pack_evidence(role: RatesInputRole, value: &MarketRulePack) -> RatesInputEvidence {
    RatesInputEvidence {
        role,
        owner: value.owner().clone(),
        binding: RatesEvidenceBinding::Object(AnalyticsObjectRef::new(
            VersionRef::new(
                Ulid::new(value.identity()).expect("validated RulePack identity"),
                Version::new(value.version()).expect("validated RulePack version"),
            ),
            definition_content_hash(&DefinitionValue::MarketRulePack(value.clone())),
        )),
        observed_at: None,
        visible_at: None,
        effective_from: Some(value.effective().from().clone()),
        effective_to: Some(value.effective().to().clone()),
    }
}

fn data_snapshot_evidence(snapshot: &DataSnapshot) -> RatesInputEvidence {
    RatesInputEvidence {
        role: RatesInputRole::DataSnapshot,
        owner: snapshot.owner().clone(),
        binding: RatesEvidenceBinding::Snapshot(ImmutableSnapshotBinding::new(
            snapshot.id().clone(),
            snapshot.content_hash().clone(),
        )),
        observed_at: Some(snapshot.as_of().clone()),
        visible_at: Some(snapshot.visible_at().clone()),
        effective_from: None,
        effective_to: None,
    }
}

fn data_source_evidence(value: &DataSource) -> RatesInputEvidence {
    RatesInputEvidence {
        role: RatesInputRole::DataSource,
        owner: value.owner().clone(),
        binding: RatesEvidenceBinding::Object(AnalyticsObjectRef::new(
            VersionRef::new(
                value.id().clone(),
                Version::new(value.version()).expect("validated DataSource version"),
            ),
            rates_data_source_content_hash(value),
        )),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    }
}

fn curve_node_definition_evidence(
    owner: &OwnerRef,
    value: &ficant_domain::research::CurveNodeDefinition,
) -> RatesInputEvidence {
    RatesInputEvidence {
        role: RatesInputRole::CurveNodeDefinition,
        owner: owner.clone(),
        binding: RatesEvidenceBinding::CurveNode(ImmutableCurveNodeBinding::new(
            value.curve_node_id(),
            value.content_hash().clone(),
        )),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    }
}

/// Returns the R5D canonical content identity of one immutable `DataSource` definition.
#[must_use]
pub fn rates_data_source_content_hash(value: &DataSource) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, b"ficant.rates.data-source.v1");
    append(&mut bytes, value.id().as_str().as_bytes());
    append(&mut bytes, &value.version().to_be_bytes());
    append(&mut bytes, value.owner().tenant_id().as_str().as_bytes());
    append(&mut bytes, value.owner().owner_id().as_str().as_bytes());
    append(
        &mut bytes,
        &[match value.kind() {
            DataSourceKind::FileNdjson => 1,
            DataSourceKind::Postgres => 2,
        }],
    );
    append(&mut bytes, value.name().as_bytes());
    append(&mut bytes, value.connection_binding().as_bytes());
    append(&mut bytes, value.dataset().as_bytes());
    append(&mut bytes, value.canonical_schema_id().as_bytes());
    append(&mut bytes, value.canonical_schema_hash().as_bytes());
    if let Some(source_type) = value.price_source_type() {
        append(&mut bytes, &[(source_type as u8).saturating_add(1)]);
    }
    ContentHash::digest(&bytes)
}

fn artifact_evidence(
    role: RatesInputRole,
    owner: &OwnerRef,
    binding: ImmutableArtifactBinding,
    observed_at: &MarketTime,
) -> RatesInputEvidence {
    RatesInputEvidence {
        role,
        owner: owner.clone(),
        binding: RatesEvidenceBinding::Artifact(binding),
        observed_at: Some(observed_at.clone()),
        visible_at: None,
        effective_from: None,
        effective_to: None,
    }
}

fn artifact_object_ref(
    binding: &ImmutableArtifactBinding,
) -> ApplicationResult<AnalyticsObjectRef> {
    Ok(AnalyticsObjectRef::new(
        VersionRef::new(
            binding.id().clone(),
            Version::new(1).map_err(map_domain_error)?,
        ),
        binding.content_hash().clone(),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn read_artifact(
    artifacts: &dyn ArtifactRepository,
    blobs: &dyn VerifiedBlobReader,
    integrity_events: &dyn IntegrityEventSink,
    scope: &AccessScope,
    owner: &OwnerRef,
    binding: &ImmutableArtifactBinding,
    media_type: &str,
    trace: SafeTraceContext,
) -> ApplicationResult<(Artifact, Vec<u8>)> {
    let artifact = artifacts
        .get_metadata(scope, binding.id().clone())
        .await?
        .ok_or_else(not_found)?;
    scope.authorize(artifact.owner())?;
    if artifact.id() != binding.id()
        || artifact.owner() != owner
        || artifact.kind() != ArtifactKind::Generic
        || artifact.media_type() != media_type
        || artifact.content_hash() != binding.content_hash()
    {
        return Err(lineage());
    }
    let read = RequiredVerifiedBlobRead::new(
        scope.clone(),
        owner.clone(),
        VerifiedReadResourceKind::Artifact,
        artifact.id().clone(),
        VerifiedBlobRole::ArtifactPayload,
        artifact.content_hash().clone(),
        artifact.blob_size(),
        trace,
    )?;
    let payload = blobs.read_required(&read, integrity_events).await?;
    Ok((artifact, payload.bytes().to_vec()))
}

fn validate_bond_artifact_lineage(
    artifact: &Artifact,
    facts: &crate::ports::BondAnalyticsArtifactFacts,
) -> ApplicationResult<()> {
    let inputs = artifact.lineage();
    if inputs.len() != 3
        || !lineage_version_matches(&inputs[0], facts.bond(), false)
        || !lineage_version_matches(&inputs[1], facts.rule_pack(), true)
        || !lineage_content_matches(&inputs[2], facts.snapshot())
    {
        return Err(lineage());
    }
    Ok(())
}

fn validate_delivery_artifact_lineage(
    artifact: &Artifact,
    facts: &crate::ports::FuturesDeliveryArtifactFacts,
) -> ApplicationResult<()> {
    let inputs = artifact.lineage();
    let expected_len = facts
        .candidates()
        .len()
        .checked_add(3)
        .ok_or_else(invalid)?;
    if inputs.len() != expected_len
        || !lineage_version_matches(&inputs[0], facts.futures_contract(), false)
    {
        return Err(lineage());
    }
    for (actual, candidate) in inputs[1..=facts.candidates().len()]
        .iter()
        .zip(facts.candidates())
    {
        if !lineage_version_matches(actual, candidate.bond(), false) {
            return Err(lineage());
        }
    }
    let rule_index = facts.candidates().len() + 1;
    if !lineage_version_matches(&inputs[rule_index], facts.rule_pack(), true)
        || !lineage_content_matches(&inputs[rule_index + 1], facts.snapshot())
    {
        return Err(lineage());
    }
    Ok(())
}

fn lineage_version_matches(
    actual: &ficant_domain::primitives::LineageRef,
    expected: &AnalyticsObjectRef,
    require_hash: bool,
) -> bool {
    actual.object_id() == expected.version_ref().id()
        && actual.version() == Some(expected.version_ref().version())
        && if require_hash {
            actual.content_hash() == Some(expected.content_hash())
        } else {
            actual.content_hash().is_none()
        }
}

fn lineage_content_matches(
    actual: &ficant_domain::primitives::LineageRef,
    expected: &AnalyticsObjectRef,
) -> bool {
    actual.object_id() == expected.version_ref().id()
        && actual.version().is_none()
        && actual.content_hash() == Some(expected.content_hash())
}

fn subject_hash(record: &ficant_domain::subject::SubjectRecord) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, record.subject().id().as_str().as_bytes());
    append(&mut bytes, record.subject().display_name().as_bytes());
    append(
        &mut bytes,
        &record.version().reference().version().get().to_be_bytes(),
    );
    for value in record.version().access_set().market_codes() {
        append(&mut bytes, value.as_bytes());
    }
    for value in record.version().access_set().tool_codes() {
        append(&mut bytes, value.as_bytes());
    }
    append(
        &mut bytes,
        &[match record.version().funding_tier() {
            ficant_domain::subject::FundingTier::DrAvailable => 1,
            ficant_domain::subject::FundingTier::ROnly => 2,
        }],
    );
    append(
        &mut bytes,
        record
            .version()
            .tax_treatment()
            .value_added_tax_profile()
            .as_bytes(),
    );
    append(
        &mut bytes,
        record
            .version()
            .tax_treatment()
            .income_tax_profile()
            .as_bytes(),
    );
    append(
        &mut bytes,
        record.version().assessment_mechanism().as_bytes(),
    );
    append(&mut bytes, record.version().liability_profile().as_bytes());
    if let Some(reference) = record.version().constraint_set_ref() {
        append(&mut bytes, reference.reference().id().as_str().as_bytes());
        append(
            &mut bytes,
            &reference.reference().version().get().to_be_bytes(),
        );
    }
    ContentHash::digest(&bytes)
}

fn parameter_prefix(
    kind: &[u8],
    knowledge_at: &MarketTime,
    valuation_at: &MarketTime,
    algorithm_id: &str,
    algorithm_version: u32,
    convention_profile: &str,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    append(&mut bytes, b"ficant.rates.parameters.v1");
    append(&mut bytes, kind);
    append(&mut bytes, algorithm_id.as_bytes());
    append(&mut bytes, &algorithm_version.to_be_bytes());
    append(&mut bytes, convention_profile.as_bytes());
    append(&mut bytes, &ABI_VERSION.to_be_bytes());
    append_time(&mut bytes, knowledge_at);
    append_time(&mut bytes, valuation_at);
    bytes
}

fn append_bond_materialization(bytes: &mut Vec<u8>, input: &BondAnalyticsInput) {
    append(bytes, input.owner().tenant_id().as_str().as_bytes());
    append(bytes, input.owner().owner_id().as_str().as_bytes());
    for reference in [input.bond(), input.rule_pack(), input.snapshot()] {
        append(bytes, reference.version_ref().id().as_str().as_bytes());
        append(
            bytes,
            &reference.version_ref().version().get().to_be_bytes(),
        );
        append(bytes, reference.content_hash().as_bytes());
    }
    append(bytes, input.settlement_date().to_string().as_bytes());
    append(bytes, &[input.calendar_requirement() as u8]);
    append(bytes, input.calendar().id().as_bytes());
    append(bytes, &input.calendar().version().get().to_be_bytes());
    append(bytes, input.calendar().content_hash().as_bytes());
    append(
        bytes,
        input.calendar().coverage_start().to_string().as_bytes(),
    );
    append(
        bytes,
        input.calendar().coverage_end().to_string().as_bytes(),
    );
    append_dates(bytes, input.calendar().non_business_days());
    append_dates(bytes, input.calendar().work_weekends());
    let terms = input.terms();
    for date in [
        terms.first_issue_date(),
        terms.current_issue_date(),
        terms.maturity_date(),
    ] {
        append(bytes, date.to_string().as_bytes());
    }
    append(bytes, &[terms.frequency() as u8]);
    append(bytes, &[terms.day_count() as u8]);
    append(bytes, &[terms.business_day() as u8]);
    for value in [
        terms.coupon_rate(),
        terms.face_amount(),
        terms.cumulative_issued_amount(),
    ] {
        append(bytes, &value.scaled().to_be_bytes());
    }
    if let Some(tax) = terms.tax_attributes() {
        append(bytes, &[1]);
        append(bytes, &[tax.value_added_tax_status() as u8]);
        append(bytes, &[tax.income_tax_status() as u8]);
    } else {
        append(bytes, &[0]);
    }
    append(bytes, &[input.mode() as u8]);
    append(bytes, &input.input_value().scaled().to_be_bytes());
}

fn append_dates(bytes: &mut Vec<u8>, dates: &[NaiveDate]) {
    append(
        bytes,
        &u64::try_from(dates.len()).unwrap_or(u64::MAX).to_be_bytes(),
    );
    for date in dates {
        append(bytes, date.to_string().as_bytes());
    }
}

fn require_units<const N: usize>(
    actual: &[RatesUnitRequirement],
    required: [&UnitRef; N],
) -> ApplicationResult<()> {
    if required
        .into_iter()
        .any(|expected| !actual.iter().any(|value| value.reference() == expected))
    {
        return Err(map_domain_error(DomainErrorCode::InvalidUnit));
    }
    Ok(())
}

fn append_evidence(bytes: &mut Vec<u8>, value: &RatesInputEvidence) {
    append(bytes, &[value.role as u8]);
    append(bytes, value.owner.tenant_id().as_str().as_bytes());
    append(bytes, value.owner.owner_id().as_str().as_bytes());
    match &value.binding {
        RatesEvidenceBinding::Object(binding) => {
            append(bytes, &[1]);
            append(bytes, binding.version_ref().id().as_str().as_bytes());
            append(bytes, &binding.version_ref().version().get().to_be_bytes());
            append(bytes, binding.content_hash().as_bytes());
        }
        RatesEvidenceBinding::Snapshot(binding) => {
            append(bytes, &[2]);
            append(bytes, binding.id().as_str().as_bytes());
            append(bytes, binding.content_hash().as_bytes());
        }
        RatesEvidenceBinding::Artifact(binding) => {
            append(bytes, &[3]);
            append(bytes, binding.id().as_str().as_bytes());
            append(bytes, binding.content_hash().as_bytes());
        }
        RatesEvidenceBinding::CurveNode(binding) => {
            append(bytes, &[4]);
            append(bytes, binding.curve_node_id().as_bytes());
            append(bytes, binding.content_hash().as_bytes());
        }
    }
    append_optional_time(bytes, value.observed_at.as_ref());
    append_optional_time(bytes, value.visible_at.as_ref());
    append_optional_time(bytes, value.effective_from.as_ref());
    append_optional_time(bytes, value.effective_to.as_ref());
}

fn compare_evidence(left: &RatesInputEvidence, right: &RatesInputEvidence) -> std::cmp::Ordering {
    left.role
        .cmp(&right.role)
        .then_with(|| evidence_key(&left.binding).cmp(&evidence_key(&right.binding)))
        .then_with(|| left.owner.tenant_id().cmp(right.owner.tenant_id()))
        .then_with(|| left.owner.owner_id().cmp(right.owner.owner_id()))
}

fn same_evidence_identity(left: &RatesInputEvidence, right: &RatesInputEvidence) -> bool {
    if left.role != right.role || left.owner != right.owner {
        return false;
    }
    match (&left.binding, &right.binding) {
        (RatesEvidenceBinding::Object(left), RatesEvidenceBinding::Object(right)) => {
            left.version_ref() == right.version_ref()
        }
        (RatesEvidenceBinding::Snapshot(left), RatesEvidenceBinding::Snapshot(right)) => {
            left.id() == right.id()
        }
        (RatesEvidenceBinding::Artifact(left), RatesEvidenceBinding::Artifact(right)) => {
            left.id() == right.id()
        }
        (RatesEvidenceBinding::CurveNode(left), RatesEvidenceBinding::CurveNode(right)) => {
            left.curve_node_id() == right.curve_node_id()
        }
        _ => false,
    }
}

fn evidence_key(value: &RatesEvidenceBinding) -> (u8, &str, u64, &[u8; 32]) {
    match value {
        RatesEvidenceBinding::Object(value) => (
            1,
            value.version_ref().id().as_str(),
            value.version_ref().version().get(),
            value.content_hash().as_bytes(),
        ),
        RatesEvidenceBinding::Snapshot(value) => {
            (2, value.id().as_str(), 0, value.content_hash().as_bytes())
        }
        RatesEvidenceBinding::Artifact(value) => {
            (3, value.id().as_str(), 0, value.content_hash().as_bytes())
        }
        RatesEvidenceBinding::CurveNode(value) => {
            (4, value.curve_node_id(), 0, value.content_hash().as_bytes())
        }
    }
}

fn append_optional_time(bytes: &mut Vec<u8>, value: Option<&MarketTime>) {
    if let Some(value) = value {
        append(bytes, &[1]);
        append_time(bytes, value);
    } else {
        append(bytes, &[0]);
    }
}

fn append_time(bytes: &mut Vec<u8>, value: &MarketTime) {
    append(bytes, &value.instant().timestamp().to_be_bytes());
    append(
        bytes,
        &value.instant().timestamp_subsec_nanos().to_be_bytes(),
    );
    append(bytes, value.market_timezone().as_bytes());
    append(bytes, value.local_trading_date().to_string().as_bytes());
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn lineage() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}
