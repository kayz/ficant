use std::collections::BTreeMap;

use chrono::{Datelike, Days, Months, NaiveDate, Weekday};
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondTerms, BusinessDayConvention,
    CalendarBinding, CalendarRequirement, CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::curves::{
    YieldCurveBinding, YieldCurveInterpolation, YieldCurveNode, YieldCurveQuery,
};
use ficant_domain::futures_delivery::FuturesDeliveryRule;
use ficant_domain::market::{
    BondBusinessDayConvention, BondCouponFrequency, BondDayCountConvention, Calendar,
    MarketRulePack, Unit,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    CurveNodeDefinition, CurveNodeRef, CurveRebuildPolicy, FactorDefinition, FactorDv01,
    FactorTarget, FactorTargetBinding, InstrumentFactorTarget, PortfolioKeyRateExposure, Position,
    PositionKeyRateExposure, PositionSnapshot, RiskAlgorithmBinding, SecondOrderPolicy,
    SensitivityDirection, key_rate_dv01, scale_futures_key_rate_dv01,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};

use crate::ports::{
    AccessScope, ApplicationResult, BondAnalyticsEngine, CanonicalSnapshotDecoder,
    CurvePointSetDecoder, CurveSnapshotMetadataRepository, DefinitionRepository, DefinitionValue,
    FactorTopologyRepository, FuturesDeliveryEngine, FuturesDeliveryRuleParser, InstrumentSubtype,
    IntegrityEventSink, PositionSnapshotRepository, RequiredVerifiedBlobRead, SafeTraceContext,
    SnapshotVerifiedReadMetadataRepository, VerifiedBlobReader, VerifiedBlobRole,
    VerifiedReadResourceKind, YieldCurveEngine, definition_content_hash,
};
use crate::use_cases::bond_analytics::map_analytics_error;
use crate::use_cases::futures_delivery::{
    CalculateFuturesDeliveryBasket, MaterializeRegisteredFuturesDelivery,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

pub const R4D_A_ALGORITHM_ID: &str = "ficant.fixed-rate-bond.key-rate-yield";
pub const R4D_A_ALGORITHM_VERSION: u32 = 1;
pub const R4D_A_CONVENTION_PROFILE: &str = "linear-ytm-registered-bond-v1";
pub const R4D_B_ALGORITHM_ID: &str = "ficant.fixed-income.portfolio-key-rate-yield";
pub const R4D_B_ALGORITHM_VERSION: u32 = 1;
pub const R4D_B_CONVENTION_PROFILE: &str = "linear-ytm-fixed-base-ctd-v1";
const FIXED_DECIMAL_SCALE: u32 = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalculateBondKeyRateDv01Command {
    position_snapshot_id: Ulid,
    knowledge_at: MarketTime,
    valuation_at: MarketTime,
    curve_snapshot_id: Ulid,
    dv01_unit: UnitRef,
    futures_data_snapshot_id: Option<Ulid>,
}

impl CalculateBondKeyRateDv01Command {
    /// Creates one exact snapshot/time/unit-bound portfolio risk request.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the knowledge time precedes valuation.
    pub fn new(
        position_snapshot_id: Ulid,
        knowledge_at: MarketTime,
        valuation_at: MarketTime,
        curve_snapshot_id: Ulid,
        dv01_unit: UnitRef,
    ) -> ApplicationResult<Self> {
        if knowledge_at.instant() < valuation_at.instant() {
            return Err(validation());
        }
        Ok(Self {
            position_snapshot_id,
            knowledge_at,
            valuation_at,
            curve_snapshot_id,
            dv01_unit,
            futures_data_snapshot_id: None,
        })
    }

    /// Creates the full Bond + concrete Futures request bound to one verified quote snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the common portfolio-risk command fields are invalid.
    pub fn new_with_futures_data_snapshot(
        position_snapshot_id: Ulid,
        knowledge_at: MarketTime,
        valuation_at: MarketTime,
        curve_snapshot_id: Ulid,
        dv01_unit: UnitRef,
        futures_data_snapshot_id: Ulid,
    ) -> ApplicationResult<Self> {
        let mut command = Self::new(
            position_snapshot_id,
            knowledge_at,
            valuation_at,
            curve_snapshot_id,
            dv01_unit,
        )?;
        command.futures_data_snapshot_id = Some(futures_data_snapshot_id);
        Ok(command)
    }

    #[must_use]
    pub fn position_snapshot_id(&self) -> &Ulid {
        &self.position_snapshot_id
    }

    #[must_use]
    pub fn knowledge_at(&self) -> &MarketTime {
        &self.knowledge_at
    }

    #[must_use]
    pub fn valuation_at(&self) -> &MarketTime {
        &self.valuation_at
    }

    #[must_use]
    pub fn curve_snapshot_id(&self) -> &Ulid {
        &self.curve_snapshot_id
    }

    #[must_use]
    pub fn dv01_unit(&self) -> &UnitRef {
        &self.dv01_unit
    }

    #[must_use]
    pub fn futures_data_snapshot_id(&self) -> Option<&Ulid> {
        self.futures_data_snapshot_id.as_ref()
    }
}

pub struct CalculateBondKeyRateDv01<'a> {
    positions: &'a dyn PositionSnapshotRepository,
    curves: &'a dyn CurveSnapshotMetadataRepository,
    definitions: &'a dyn DefinitionRepository,
    factors: &'a dyn FactorTopologyRepository,
    blobs: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
    decoder: &'a dyn CurvePointSetDecoder,
    curve_engine: &'a dyn YieldCurveEngine,
    bond_engine: &'a dyn BondAnalyticsEngine,
    futures_snapshot_metadata: Option<&'a dyn SnapshotVerifiedReadMetadataRepository>,
    futures_snapshot_decoder: Option<&'a dyn CanonicalSnapshotDecoder>,
    futures_rule_parser: Option<&'a dyn FuturesDeliveryRuleParser>,
    futures_engine: Option<&'a dyn FuturesDeliveryEngine>,
}

impl<'a> CalculateBondKeyRateDv01<'a> {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        positions: &'a dyn PositionSnapshotRepository,
        curves: &'a dyn CurveSnapshotMetadataRepository,
        definitions: &'a dyn DefinitionRepository,
        factors: &'a dyn FactorTopologyRepository,
        blobs: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
        decoder: &'a dyn CurvePointSetDecoder,
        curve_engine: &'a dyn YieldCurveEngine,
        bond_engine: &'a dyn BondAnalyticsEngine,
    ) -> Self {
        Self {
            positions,
            curves,
            definitions,
            factors,
            blobs,
            integrity_events,
            decoder,
            curve_engine,
            bond_engine,
            futures_snapshot_metadata: None,
            futures_snapshot_decoder: None,
            futures_rule_parser: None,
            futures_engine: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn new_with_futures(
        positions: &'a dyn PositionSnapshotRepository,
        curves: &'a dyn CurveSnapshotMetadataRepository,
        definitions: &'a dyn DefinitionRepository,
        factors: &'a dyn FactorTopologyRepository,
        blobs: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
        decoder: &'a dyn CurvePointSetDecoder,
        curve_engine: &'a dyn YieldCurveEngine,
        bond_engine: &'a dyn BondAnalyticsEngine,
        futures_snapshot_metadata: &'a dyn SnapshotVerifiedReadMetadataRepository,
        futures_snapshot_decoder: &'a dyn CanonicalSnapshotDecoder,
        futures_rule_parser: &'a dyn FuturesDeliveryRuleParser,
        futures_engine: &'a dyn FuturesDeliveryEngine,
    ) -> Self {
        Self {
            positions,
            curves,
            definitions,
            factors,
            blobs,
            integrity_events,
            decoder,
            curve_engine,
            bond_engine,
            futures_snapshot_metadata: Some(futures_snapshot_metadata),
            futures_snapshot_decoder: Some(futures_snapshot_decoder),
            futures_rule_parser: Some(futures_rule_parser),
            futures_engine: Some(futures_engine),
        }
    }

    /// Materializes verified inputs and calculates the complete bond-only key-rate exposure.
    ///
    /// # Errors
    ///
    /// Fails closed on any missing, unauthorized, inconsistent, non-canonical, unsupported, or
    /// non-representable input; no partial exposure is returned.
    #[allow(clippy::too_many_lines)]
    pub async fn execute(
        &self,
        scope: &AccessScope,
        command: CalculateBondKeyRateDv01Command,
    ) -> ApplicationResult<PortfolioKeyRateExposure> {
        let snapshot = self
            .positions
            .get_position_snapshot(
                scope,
                command.position_snapshot_id.clone(),
                command.knowledge_at.clone(),
            )
            .await?
            .ok_or_else(not_found)?;
        validate_position_snapshot(scope, &snapshot, &command)?;

        let metadata = self
            .curves
            .get_curve_snapshot_metadata(scope, command.curve_snapshot_id.clone())
            .await?
            .ok_or_else(not_found)?;
        let curve = metadata.snapshot();
        validate_curve_snapshot(scope, curve, &snapshot, &command)?;
        let request = RequiredVerifiedBlobRead::new(
            scope.clone(),
            curve.owner().clone(),
            VerifiedReadResourceKind::CurveSnapshot,
            curve.id().clone(),
            VerifiedBlobRole::CurvePoints,
            curve.content_hash().clone(),
            metadata.blob_size(),
            trace_for(&command)?,
        )?;
        let payload = self
            .blobs
            .read_required(&request, self.integrity_events)
            .await?;
        let points = self.decoder.decode_canonical(payload.bytes())?;
        if points.curve_family_id() != curve.curve_family_id().ok_or_else(lineage)? {
            return Err(lineage());
        }

        let output_unit = self.read_unit(scope, command.dv01_unit()).await?;
        if output_unit.dimension() != "dv01"
            || output_unit.owner() != snapshot.owner()
            || output_unit.scale() < FIXED_DECIMAL_SCALE
        {
            return Err(validation());
        }
        let curve_currency = self.read_unit(scope, curve.currency()).await?;
        if curve_currency.dimension() != "currency" || curve_currency.owner() != snapshot.owner() {
            return Err(validation());
        }

        let calendar = self.read_calendar(scope, curve.calendar()).await?;
        if calendar.owner() != snapshot.owner() {
            return Err(lineage());
        }
        require_open_settlement(&calendar, command.valuation_at.local_trading_date())?;
        let rule_pack = self.read_rule_pack(scope, curve.rule_pack()).await?;
        if rule_pack.owner() != curve.owner() {
            return Err(lineage());
        }

        let axis = self.materialize_axis(scope, curve, &points).await?;
        let mut prepared = Vec::new();
        for position in snapshot
            .positions()
            .iter()
            .filter(|position| position.includes_position_exposure())
        {
            prepared.push(
                self.prepare_position(
                    scope, &snapshot, position, curve, &calendar, &rule_pack, &axis, &command,
                )
                .await?,
            );
        }
        if prepared.is_empty() {
            return Err(validation());
        }
        let has_futures = prepared
            .iter()
            .any(|value| matches!(value, PreparedRiskPosition::Futures(_)));
        if has_futures != command.futures_data_snapshot_id().is_some() {
            return Err(validation());
        }

        let mut selected = Vec::with_capacity(prepared.len());
        for position in prepared {
            selected.push(match position {
                PreparedRiskPosition::Bond(value) => SelectedRiskPosition::Bond(value),
                PreparedRiskPosition::Futures(value) => SelectedRiskPosition::Futures(Box::new(
                    self.select_futures_position(scope, *value, &axis).await?,
                )),
            });
        }

        let mut lineage = vec![
            LineageRef::new(
                snapshot.id().clone(),
                None,
                Some(snapshot.content_hash().clone()),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(curve.id().clone(), None, Some(curve.content_hash().clone()))
                .map_err(map_domain_error)?,
        ];
        for position in &selected {
            if let SelectedRiskPosition::Futures(value) = position {
                for reference in &value.materialization_lineage {
                    if !lineage.contains(reference) {
                        lineage.push(reference.clone());
                    }
                }
            }
        }
        lineage.sort_by(|left, right| {
            left.object_id()
                .cmp(right.object_id())
                .then_with(|| left.version().cmp(&right.version()))
        });

        let mut exposures = Vec::with_capacity(selected.len());
        for position in selected {
            exposures.push(match position {
                SelectedRiskPosition::Bond(value) => {
                    self.calculate_position(*value, &axis, command.dv01_unit(), &output_unit)?
                }
                SelectedRiskPosition::Futures(value) => self.calculate_futures_position(
                    *value,
                    &axis,
                    command.dv01_unit(),
                    &output_unit,
                )?,
            });
        }
        exposures.sort_by(|left, right| left.position_id().cmp(right.position_id()));
        let (algorithm_id, algorithm_version, convention_profile) = if has_futures {
            (
                R4D_B_ALGORITHM_ID,
                R4D_B_ALGORITHM_VERSION,
                R4D_B_CONVENTION_PROFILE,
            )
        } else {
            (
                R4D_A_ALGORITHM_ID,
                R4D_A_ALGORITHM_VERSION,
                R4D_A_CONVENTION_PROFILE,
            )
        };
        let algorithm =
            RiskAlgorithmBinding::new(algorithm_id, algorithm_version, convention_profile)
                .map_err(map_domain_error)?;
        match command.futures_data_snapshot_id {
            Some(data_snapshot_id) => PortfolioKeyRateExposure::new_with_futures_data_snapshot(
                snapshot.id().clone(),
                curve.id().clone(),
                data_snapshot_id,
                exposures,
                algorithm,
                lineage,
            ),
            None => PortfolioKeyRateExposure::new(
                snapshot.id().clone(),
                curve.id().clone(),
                exposures,
                algorithm,
                lineage,
            ),
        }
        .map_err(map_domain_error)
    }

    async fn materialize_axis(
        &self,
        scope: &AccessScope,
        curve: &ficant_domain::market::CurveSnapshot,
        points: &crate::ports::DecodedCurvePointSet,
    ) -> ApplicationResult<Vec<AxisPoint>> {
        let mut axis = Vec::with_capacity(points.points().len());
        for point in points.points() {
            let node = self
                .factors
                .get_curve_node_definition(point.curve_node_id())
                .await?
                .ok_or_else(not_found)?;
            validate_node(curve, point, &node)?;
            let target = FactorTarget::CurveNode(
                CurveNodeRef::new(node.curve_node_id(), node.content_hash().clone())
                    .map_err(map_domain_error)?,
            );
            let factors = self.factors.get_target_factors(scope, &target).await?;
            if factors.len() != 1 {
                return Err(lineage());
            }
            let factor = factors.into_iter().next().expect("length checked");
            let curve_binding =
                FactorTargetBinding::new(factor.factor_id(), target).map_err(map_domain_error)?;
            validate_factor(point, &node, &factor)?;
            let factor_unit = self.read_unit(scope, factor.factor_unit()).await?;
            validate_decimal_unit(
                point.yield_to_maturity(),
                &factor_unit,
                "rate",
                curve.owner(),
            )?;
            validate_decimal_unit(
                factor.convention().bump(),
                &factor_unit,
                "rate",
                curve.owner(),
            )?;
            axis.push(AxisPoint {
                maturity_date: tenor_date(curve.as_of().local_trading_date(), node.tenor())?,
                yield_to_maturity: decimal_to_fixed(point.yield_to_maturity())?,
                bump_yield: decimal_to_fixed(factor.convention().bump())?,
                node_content_hash: node.content_hash().clone(),
                curve_binding_hash: curve_binding.content_hash().clone(),
                factor,
                factor_unit,
            });
        }
        let first_unit = axis.first().ok_or_else(validation)?.factor.factor_unit();
        if axis
            .iter()
            .any(|point| point.factor.factor_unit() != first_unit)
        {
            return Err(lineage());
        }
        let mut dates = axis
            .iter()
            .map(|value| value.maturity_date)
            .collect::<Vec<_>>();
        dates.sort_unstable();
        if dates.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(validation());
        }
        axis.sort_by(|left, right| left.factor.factor_id().cmp(right.factor.factor_id()));
        if axis
            .windows(2)
            .any(|pair| pair[0].factor.factor_id() == pair[1].factor.factor_id())
        {
            return Err(lineage());
        }
        Ok(axis)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn prepare_position(
        &self,
        scope: &AccessScope,
        snapshot: &PositionSnapshot,
        position: &Position,
        curve: &ficant_domain::market::CurveSnapshot,
        calendar: &Calendar,
        rule_pack: &MarketRulePack,
        axis: &[AxisPoint],
        command: &CalculateBondKeyRateDv01Command,
    ) -> ApplicationResult<PreparedRiskPosition> {
        if position.quantity().coefficient() == "0" {
            return Err(validation());
        }
        let definition = self
            .definitions
            .get_version(
                scope,
                position.instrument_ref().id().clone(),
                position.instrument_ref().version(),
            )
            .await?
            .ok_or_else(not_found)?;
        let DefinitionValue::Instrument(instrument) = definition else {
            return Err(lineage());
        };
        if instrument.owner() != snapshot.owner()
            || instrument.instrument().version_ref() != *position.instrument_ref()
            || instrument.instrument().currency() != curve.currency()
            || instrument.instrument().calendar() != curve.calendar()
        {
            return Err(lineage());
        }
        let Some(subtype) = instrument.subtype() else {
            return Err(validation());
        };
        if matches!(subtype, InstrumentSubtype::FuturesContract(_)) {
            return self
                .prepare_futures_position(
                    scope, snapshot, position, curve, calendar, rule_pack, axis, command,
                )
                .await
                .map(|value| PreparedRiskPosition::Futures(Box::new(value)));
        }
        let InstrumentSubtype::Bond(bond) = subtype else {
            return Err(validation());
        };
        if position.quantity().unit() != bond.face_value().unit() {
            return Err(validation());
        }
        let quantity_unit = self.read_unit(scope, position.quantity().unit()).await?;
        validate_decimal_unit(
            position.quantity(),
            &quantity_unit,
            "notional",
            snapshot.owner(),
        )?;
        let pricing = bond.pricing_terms().ok_or_else(lineage)?;
        let tax = bond.tax_attributes().ok_or_else(lineage)?;
        validate_decimal_unit(
            bond.face_value(),
            &quantity_unit,
            "notional",
            snapshot.owner(),
        )?;
        validate_decimal_unit(
            bond.cumulative_issued_amount(),
            &quantity_unit,
            "notional",
            snapshot.owner(),
        )?;
        let rate_unit = &axis.first().ok_or_else(validation)?.factor_unit;
        if pricing.coupon_rate().unit() != axis[0].factor.factor_unit() {
            return Err(lineage());
        }
        validate_decimal_unit(pricing.coupon_rate(), rate_unit, "rate", snapshot.owner())?;
        let target = FactorTarget::Instrument(InstrumentFactorTarget::new(
            snapshot.owner().clone(),
            position.instrument_ref().clone(),
        ));
        let bound_factors = self
            .factors
            .get_target_factors(scope, &target)
            .await?
            .into_iter()
            .map(|value| {
                let binding = FactorTargetBinding::new(value.factor_id(), target.clone())
                    .map_err(map_domain_error)?;
                Ok((value.factor_id().to_owned(), binding.content_hash().clone()))
            })
            .collect::<ApplicationResult<BTreeMap<_, _>>>()?;
        if axis
            .iter()
            .any(|point| !bound_factors.contains_key(point.factor.factor_id()))
        {
            return Err(lineage());
        }

        let terms = BondTerms::with_issuance(
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
        .map_err(map_domain_error)?;
        let definition = DefinitionValue::Instrument(instrument.clone());
        let calendar_value = DefinitionValue::Calendar(calendar.clone());
        let rule_pack_value = DefinitionValue::MarketRulePack(rule_pack.clone());
        let quantity_unit_value = DefinitionValue::Unit(quantity_unit);
        let calendar_binding =
            calendar_binding(calendar, definition_content_hash(&calendar_value))?;
        let rule_pack_ref =
            AnalyticsObjectRef::new(curve.rule_pack().clone(), rule_pack.content_hash().clone());
        let snapshot_ref = AnalyticsObjectRef::new(
            VersionRef::new(
                curve.id().clone(),
                Version::new(1).map_err(map_domain_error)?,
            ),
            curve.content_hash().clone(),
        );
        let bond_ref = AnalyticsObjectRef::new(
            position.instrument_ref().clone(),
            definition_content_hash(&definition),
        );
        let curve_binding = yield_curve(curve, axis, None)?;
        if !curve_binding.covers(bond.maturity_date()) {
            return Err(validation());
        }
        let lineage = vec![
            LineageRef::new(
                snapshot.id().clone(),
                None,
                Some(snapshot.content_hash().clone()),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(
                position.instrument_ref().id().clone(),
                Some(position.instrument_ref().version()),
                Some(definition_content_hash(&definition)),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(curve.id().clone(), None, Some(curve.content_hash().clone()))
                .map_err(map_domain_error)?,
            LineageRef::new(
                curve.calendar().id().clone(),
                Some(curve.calendar().version()),
                Some(definition_content_hash(&calendar_value)),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(
                curve.rule_pack().id().clone(),
                Some(curve.rule_pack().version()),
                Some(definition_content_hash(&rule_pack_value)),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(
                position.quantity().unit().unit_id().clone(),
                Some(position.quantity().unit().version()),
                Some(definition_content_hash(&quantity_unit_value)),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(
                pricing.coupon_rate().unit().unit_id().clone(),
                Some(pricing.coupon_rate().unit().version()),
                Some(definition_content_hash(&DefinitionValue::Unit(
                    rate_unit.clone(),
                ))),
            )
            .map_err(map_domain_error)?,
        ];
        Ok(PreparedRiskPosition::Bond(Box::new(PreparedPosition {
            position_id: position.id().clone(),
            instrument: position.instrument_ref().clone(),
            quantity: position.quantity().clone(),
            owner: snapshot.owner().clone(),
            bond_ref,
            rule_pack_ref,
            snapshot_ref,
            curve_ref: AnalyticsObjectRef::new(
                VersionRef::new(
                    curve.id().clone(),
                    Version::new(1).map_err(map_domain_error)?,
                ),
                curve.content_hash().clone(),
            ),
            valuation_at: command.valuation_at.clone(),
            settlement_date: command.valuation_at.local_trading_date(),
            calendar_binding,
            terms,
            maturity_date: bond.maturity_date(),
            bound_factors,
            lineage,
        })))
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn prepare_futures_position(
        &self,
        scope: &AccessScope,
        snapshot: &PositionSnapshot,
        position: &Position,
        curve: &ficant_domain::market::CurveSnapshot,
        calendar: &Calendar,
        curve_rule_pack: &MarketRulePack,
        axis: &[AxisPoint],
        command: &CalculateBondKeyRateDv01Command,
    ) -> ApplicationResult<PreparedFuturesPosition> {
        if position.quantity().scale() != 0 {
            return Err(validation());
        }
        let signed_contract_count = position
            .quantity()
            .coefficient()
            .parse::<i64>()
            .map_err(|_| validation())?;
        if signed_contract_count == 0 {
            return Err(validation());
        }
        let quantity_unit = self.read_unit(scope, position.quantity().unit()).await?;
        validate_decimal_unit(
            position.quantity(),
            &quantity_unit,
            "contract_count",
            snapshot.owner(),
        )?;
        if quantity_unit.scale() != 0 {
            return Err(validation());
        }
        let data_snapshot_id = command
            .futures_data_snapshot_id()
            .ok_or_else(validation)?
            .clone();
        let snapshot_metadata = self.futures_snapshot_metadata.ok_or_else(validation)?;
        let snapshot_decoder = self.futures_snapshot_decoder.ok_or_else(validation)?;
        let rule_parser = self.futures_rule_parser.ok_or_else(validation)?;
        let materialization = MaterializeRegisteredFuturesDelivery::new(
            self.definitions,
            snapshot_metadata,
            self.blobs,
            self.integrity_events,
            snapshot_decoder,
            rule_parser,
        )
        .execute(
            scope,
            snapshot.owner(),
            position.instrument_ref(),
            data_snapshot_id,
            command.valuation_at(),
            command.knowledge_at(),
            trace_for(command)?,
        )
        .await?;
        let price_unit_ref = materialization
            .contract()
            .price_unit()
            .ok_or_else(lineage)?;
        let price_unit = self.read_unit(scope, price_unit_ref).await?;
        if price_unit.owner() != snapshot.owner()
            || price_unit.dimension() != "price_per_100"
            || price_unit.scale() < FIXED_DECIMAL_SCALE
        {
            return Err(validation());
        }
        let future_target = FactorTarget::Instrument(InstrumentFactorTarget::new(
            snapshot.owner().clone(),
            position.instrument_ref().clone(),
        ));
        let future_bound_factors = self
            .factors
            .get_target_factors(scope, &future_target)
            .await?
            .into_iter()
            .map(|value| {
                let binding = FactorTargetBinding::new(value.factor_id(), future_target.clone())
                    .map_err(map_domain_error)?;
                Ok((value.factor_id().to_owned(), binding.content_hash().clone()))
            })
            .collect::<ApplicationResult<BTreeMap<_, _>>>()?;
        if axis
            .iter()
            .any(|point| !future_bound_factors.contains_key(point.factor.factor_id()))
        {
            return Err(lineage());
        }

        let calendar_value = DefinitionValue::Calendar(calendar.clone());
        let curve_rule_pack_value = DefinitionValue::MarketRulePack(curve_rule_pack.clone());
        let quantity_unit_value = DefinitionValue::Unit(quantity_unit);
        let price_unit_value = DefinitionValue::Unit(price_unit);
        let rate_axis = axis.first().ok_or_else(validation)?;
        let rate_unit = &rate_axis.factor_unit;
        let rate_unit_ref = rate_axis.factor.factor_unit();
        let common_lineage = vec![
            LineageRef::new(
                snapshot.id().clone(),
                None,
                Some(snapshot.content_hash().clone()),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(curve.id().clone(), None, Some(curve.content_hash().clone()))
                .map_err(map_domain_error)?,
            LineageRef::new(
                curve.calendar().id().clone(),
                Some(curve.calendar().version()),
                Some(definition_content_hash(&calendar_value)),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(
                curve.rule_pack().id().clone(),
                Some(curve.rule_pack().version()),
                Some(definition_content_hash(&curve_rule_pack_value)),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(
                position.quantity().unit().unit_id().clone(),
                Some(position.quantity().unit().version()),
                Some(definition_content_hash(&quantity_unit_value)),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(
                price_unit_ref.unit_id().clone(),
                Some(price_unit_ref.version()),
                Some(definition_content_hash(&price_unit_value)),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(
                rate_unit_ref.unit_id().clone(),
                Some(rate_unit_ref.version()),
                Some(definition_content_hash(&DefinitionValue::Unit(
                    rate_unit.clone(),
                ))),
            )
            .map_err(map_domain_error)?,
        ];
        Ok(PreparedFuturesPosition {
            position_id: position.id().clone(),
            instrument: position.instrument_ref().clone(),
            quantity: position.quantity().clone(),
            signed_contract_count,
            owner: snapshot.owner().clone(),
            curve_rule_pack_ref: AnalyticsObjectRef::new(
                curve.rule_pack().clone(),
                curve_rule_pack.content_hash().clone(),
            ),
            curve_snapshot_ref: AnalyticsObjectRef::new(
                VersionRef::new(
                    curve.id().clone(),
                    Version::new(1).map_err(map_domain_error)?,
                ),
                curve.content_hash().clone(),
            ),
            curve_ref: AnalyticsObjectRef::new(
                VersionRef::new(
                    curve.id().clone(),
                    Version::new(1).map_err(map_domain_error)?,
                ),
                curve.content_hash().clone(),
            ),
            valuation_at: command.valuation_at.clone(),
            settlement_date: command.valuation_at.local_trading_date(),
            calendar_binding: calendar_binding(calendar, definition_content_hash(&calendar_value))?,
            future_bound_factors,
            common_lineage,
            materialization,
        })
    }

    async fn select_futures_position(
        &self,
        scope: &AccessScope,
        position: PreparedFuturesPosition,
        axis: &[AxisPoint],
    ) -> ApplicationResult<SelectedFuturesPosition> {
        let engine = self.futures_engine.ok_or_else(validation)?;
        let basket = CalculateFuturesDeliveryBasket::new(engine)
            .execute(position.materialization.inputs())?;
        let ctd = basket.ctd();
        let ctd_input = ctd.input();
        let ctd_target = FactorTarget::Instrument(InstrumentFactorTarget::new(
            position.owner.clone(),
            ctd_input.bond().version_ref().clone(),
        ));
        let ctd_bound_factors = self
            .factors
            .get_target_factors(scope, &ctd_target)
            .await?
            .into_iter()
            .map(|value| {
                let binding = FactorTargetBinding::new(value.factor_id(), ctd_target.clone())
                    .map_err(map_domain_error)?;
                Ok((value.factor_id().to_owned(), binding.content_hash().clone()))
            })
            .collect::<ApplicationResult<BTreeMap<_, _>>>()?;
        if axis.iter().any(|point| {
            !position
                .future_bound_factors
                .contains_key(point.factor.factor_id())
                || !ctd_bound_factors.contains_key(point.factor.factor_id())
        }) {
            return Err(lineage());
        }
        let maturity_date = ctd_input.terms().maturity_date();
        let curve_binding = yield_curve_from_axis(
            position.curve_ref.clone(),
            axis,
            position.valuation_at.local_trading_date(),
            None,
        )?;
        if !curve_binding.covers(maturity_date) {
            return Err(validation());
        }
        let mut evidence = position.materialization.input_evidence_hashes().to_vec();
        let mut selection_bytes = Vec::new();
        selection_bytes.extend_from_slice(ctd_input.bond().version_ref().id().as_str().as_bytes());
        selection_bytes
            .extend_from_slice(&ctd_input.bond().version_ref().version().get().to_be_bytes());
        selection_bytes
            .extend_from_slice(&ctd.measures().conversion_factor().scaled().to_be_bytes());
        selection_bytes.extend_from_slice(
            &position
                .materialization
                .rule()
                .contract_size_in_quote_units()
                .ok_or_else(validation)?
                .to_be_bytes(),
        );
        evidence.push(ContentHash::digest(&selection_bytes));
        evidence.sort_unstable();
        evidence.dedup();
        let mut lineage = position.common_lineage.clone();
        for reference in position.materialization.lineage() {
            if !lineage.contains(reference) {
                lineage.push(reference.clone());
            }
        }
        Ok(SelectedFuturesPosition {
            position_id: position.position_id.clone(),
            instrument: position.instrument.clone(),
            signed_contract_count: position.signed_contract_count,
            pricing: PreparedPosition {
                position_id: position.position_id,
                instrument: position.instrument,
                quantity: position.quantity,
                owner: position.owner,
                bond_ref: ctd_input.bond().clone(),
                rule_pack_ref: position.curve_rule_pack_ref,
                snapshot_ref: position.curve_snapshot_ref,
                curve_ref: position.curve_ref,
                valuation_at: position.valuation_at,
                settlement_date: position.settlement_date,
                calendar_binding: position.calendar_binding,
                terms: ctd_input.terms().clone(),
                maturity_date,
                bound_factors: ctd_bound_factors.clone(),
                lineage: lineage.clone(),
            },
            rule: position.materialization.rule().clone(),
            conversion_factor: ctd.measures().conversion_factor(),
            future_bound_factors: position.future_bound_factors,
            ctd_bound_factors,
            input_evidence_hashes: evidence,
            lineage,
            materialization_lineage: position.materialization.lineage().to_vec(),
        })
    }

    fn calculate_position(
        &self,
        mut position: PreparedPosition,
        axis: &[AxisPoint],
        output_unit: &UnitRef,
        output_unit_definition: &Unit,
    ) -> ApplicationResult<PositionKeyRateExposure> {
        let base_curve = yield_curve_from_axis(
            position.curve_ref.clone(),
            axis,
            position.valuation_at.local_trading_date(),
            None,
        )?;
        let base_price = self.price(&position, base_curve)?;
        let mut exposures = Vec::with_capacity(axis.len());
        let mut input_evidence_hashes = Vec::with_capacity(axis.len() * 3);
        for (index, point) in axis.iter().enumerate() {
            input_evidence_hashes.push(point.node_content_hash.clone());
            input_evidence_hashes.push(point.curve_binding_hash.clone());
            let direction = point.factor.convention().direction();
            let up_price = if matches!(
                direction,
                SensitivityDirection::Central | SensitivityDirection::Up
            ) {
                self.price(
                    &position,
                    yield_curve_from_axis(
                        position.curve_ref.clone(),
                        axis,
                        position.valuation_at.local_trading_date(),
                        Some((index, true)),
                    )?,
                )?
            } else {
                base_price
            };
            let down_price = if matches!(
                direction,
                SensitivityDirection::Central | SensitivityDirection::Down
            ) {
                self.price(
                    &position,
                    yield_curve_from_axis(
                        position.curve_ref.clone(),
                        axis,
                        position.valuation_at.local_trading_date(),
                        Some((index, false)),
                    )?,
                )?
            } else {
                base_price
            };
            let bump_bp = FixedDecimal::from_scaled(
                point
                    .bump_yield
                    .scaled()
                    .checked_mul(10_000)
                    .ok_or_else(validation)?,
            );
            let registered_face =
                key_rate_dv01(base_price, up_price, down_price, bump_bp, direction)
                    .map_err(map_domain_error)?;
            let value = scale_by_notional(
                registered_face,
                &position.quantity,
                position.terms.face_amount(),
            )?;
            validate_fixed_output(value, output_unit_definition)?;
            if value != FixedDecimal::ZERO {
                input_evidence_hashes.push(
                    position
                        .bound_factors
                        .get(point.factor.factor_id())
                        .ok_or_else(lineage)?
                        .clone(),
                );
            }
            exposures.push(
                FactorDv01::new(
                    point.factor.factor_id(),
                    point.factor.content_hash().clone(),
                    value,
                    output_unit.clone(),
                )
                .map_err(map_domain_error)?,
            );
        }
        position.lineage.push(
            LineageRef::new(
                output_unit.unit_id().clone(),
                Some(output_unit.version()),
                Some(definition_content_hash(&DefinitionValue::Unit(
                    output_unit_definition.clone(),
                ))),
            )
            .map_err(map_domain_error)?,
        );
        input_evidence_hashes.sort_unstable();
        input_evidence_hashes.dedup();
        PositionKeyRateExposure::new(
            position.position_id,
            position.instrument,
            exposures,
            input_evidence_hashes,
            position.lineage,
        )
        .map_err(map_domain_error)
    }

    #[allow(clippy::too_many_lines)]
    fn calculate_futures_position(
        &self,
        mut position: SelectedFuturesPosition,
        axis: &[AxisPoint],
        output_unit: &UnitRef,
        output_unit_definition: &Unit,
    ) -> ApplicationResult<PositionKeyRateExposure> {
        let base_curve = yield_curve_from_axis(
            position.pricing.curve_ref.clone(),
            axis,
            position.pricing.valuation_at.local_trading_date(),
            None,
        )?;
        let base_price = self.price(&position.pricing, base_curve)?;
        let mut exposures = Vec::with_capacity(axis.len());
        let mut input_evidence_hashes = position.input_evidence_hashes.clone();
        for (index, point) in axis.iter().enumerate() {
            input_evidence_hashes.push(point.node_content_hash.clone());
            input_evidence_hashes.push(point.curve_binding_hash.clone());
            let direction = point.factor.convention().direction();
            let up_price = if matches!(
                direction,
                SensitivityDirection::Central | SensitivityDirection::Up
            ) {
                self.price(
                    &position.pricing,
                    yield_curve_from_axis(
                        position.pricing.curve_ref.clone(),
                        axis,
                        position.pricing.valuation_at.local_trading_date(),
                        Some((index, true)),
                    )?,
                )?
            } else {
                base_price
            };
            let down_price = if matches!(
                direction,
                SensitivityDirection::Central | SensitivityDirection::Down
            ) {
                self.price(
                    &position.pricing,
                    yield_curve_from_axis(
                        position.pricing.curve_ref.clone(),
                        axis,
                        position.pricing.valuation_at.local_trading_date(),
                        Some((index, false)),
                    )?,
                )?
            } else {
                base_price
            };
            let bump_bp = FixedDecimal::from_scaled(
                point
                    .bump_yield
                    .scaled()
                    .checked_mul(10_000)
                    .ok_or_else(validation)?,
            );
            let registered_face_krd =
                key_rate_dv01(base_price, up_price, down_price, bump_bp, direction)
                    .map_err(map_domain_error)?;
            let value = scale_futures_key_rate_dv01(
                registered_face_krd,
                position.pricing.terms.face_amount(),
                position.rule.face_quote_basis(),
                position
                    .rule
                    .contract_size_in_quote_units()
                    .ok_or_else(validation)?,
                position.conversion_factor,
                position.signed_contract_count,
            )
            .map_err(map_domain_error)?;
            validate_fixed_output(value, output_unit_definition)?;
            if value != FixedDecimal::ZERO {
                input_evidence_hashes.push(
                    position
                        .future_bound_factors
                        .get(point.factor.factor_id())
                        .ok_or_else(lineage)?
                        .clone(),
                );
                input_evidence_hashes.push(
                    position
                        .ctd_bound_factors
                        .get(point.factor.factor_id())
                        .ok_or_else(lineage)?
                        .clone(),
                );
            }
            exposures.push(
                FactorDv01::new(
                    point.factor.factor_id(),
                    point.factor.content_hash().clone(),
                    value,
                    output_unit.clone(),
                )
                .map_err(map_domain_error)?,
            );
        }
        position.lineage.push(
            LineageRef::new(
                output_unit.unit_id().clone(),
                Some(output_unit.version()),
                Some(definition_content_hash(&DefinitionValue::Unit(
                    output_unit_definition.clone(),
                ))),
            )
            .map_err(map_domain_error)?,
        );
        input_evidence_hashes.sort_unstable();
        input_evidence_hashes.dedup();
        PositionKeyRateExposure::new(
            position.position_id,
            position.instrument,
            exposures,
            input_evidence_hashes,
            position.lineage,
        )
        .map_err(map_domain_error)
    }

    fn price(
        &self,
        position: &PreparedPosition,
        curve: YieldCurveBinding,
    ) -> ApplicationResult<FixedDecimal> {
        let query =
            YieldCurveQuery::new(curve, position.maturity_date).map_err(map_domain_error)?;
        let point = self
            .curve_engine
            .interpolate(&query)
            .map_err(map_analytics_error)?;
        point.validate_against(&query).map_err(map_domain_error)?;
        let input = BondAnalyticsInput::new(
            position.owner.clone(),
            position.bond_ref.clone(),
            position.rule_pack_ref.clone(),
            position.snapshot_ref.clone(),
            position.valuation_at.clone(),
            position.settlement_date,
            CalendarRequirement::ExactMarket,
            position.calendar_binding.clone(),
            position.terms.clone(),
            AnalyticsMode::YieldIn,
            point.yield_to_maturity(),
        )
        .map_err(map_domain_error)?;
        let result = self
            .bond_engine
            .calculate(&input)
            .map_err(map_analytics_error)?;
        result.validate_against(&input).map_err(map_domain_error)?;
        Ok(result.measures().dirty_price())
    }

    async fn read_unit(&self, scope: &AccessScope, reference: &UnitRef) -> ApplicationResult<Unit> {
        match self
            .definitions
            .get_version(scope, reference.unit_id().clone(), reference.version())
            .await?
            .ok_or_else(not_found)?
        {
            DefinitionValue::Unit(value)
                if value.identity() == reference.unit_id().as_str()
                    && value.version() == reference.version().get() =>
            {
                Ok(value)
            }
            _ => Err(lineage()),
        }
    }

    async fn read_calendar(
        &self,
        scope: &AccessScope,
        reference: &VersionRef,
    ) -> ApplicationResult<Calendar> {
        match self
            .definitions
            .get_version(scope, reference.id().clone(), reference.version())
            .await?
            .ok_or_else(not_found)?
        {
            DefinitionValue::Calendar(value)
                if value.identity() == reference.id().as_str()
                    && value.version() == reference.version().get() =>
            {
                Ok(value)
            }
            _ => Err(lineage()),
        }
    }

    async fn read_rule_pack(
        &self,
        scope: &AccessScope,
        reference: &VersionRef,
    ) -> ApplicationResult<MarketRulePack> {
        match self
            .definitions
            .get_version(scope, reference.id().clone(), reference.version())
            .await?
            .ok_or_else(not_found)?
        {
            DefinitionValue::MarketRulePack(value)
                if value.identity() == reference.id().as_str()
                    && value.version() == reference.version().get() =>
            {
                Ok(value)
            }
            _ => Err(lineage()),
        }
    }
}

enum PreparedRiskPosition {
    Bond(Box<PreparedPosition>),
    Futures(Box<PreparedFuturesPosition>),
}

enum SelectedRiskPosition {
    Bond(Box<PreparedPosition>),
    Futures(Box<SelectedFuturesPosition>),
}

struct PreparedFuturesPosition {
    position_id: Ulid,
    instrument: VersionRef,
    quantity: DecimalValue,
    signed_contract_count: i64,
    owner: ficant_domain::primitives::OwnerRef,
    curve_rule_pack_ref: AnalyticsObjectRef,
    curve_snapshot_ref: AnalyticsObjectRef,
    curve_ref: AnalyticsObjectRef,
    valuation_at: MarketTime,
    settlement_date: NaiveDate,
    calendar_binding: CalendarBinding,
    future_bound_factors: BTreeMap<String, ContentHash>,
    common_lineage: Vec<LineageRef>,
    materialization: crate::use_cases::futures_delivery::RegisteredFuturesDeliveryMaterialization,
}

struct SelectedFuturesPosition {
    position_id: Ulid,
    instrument: VersionRef,
    signed_contract_count: i64,
    pricing: PreparedPosition,
    rule: FuturesDeliveryRule,
    conversion_factor: FixedDecimal,
    future_bound_factors: BTreeMap<String, ContentHash>,
    ctd_bound_factors: BTreeMap<String, ContentHash>,
    input_evidence_hashes: Vec<ContentHash>,
    lineage: Vec<LineageRef>,
    materialization_lineage: Vec<LineageRef>,
}

struct AxisPoint {
    factor: FactorDefinition,
    factor_unit: Unit,
    maturity_date: NaiveDate,
    yield_to_maturity: FixedDecimal,
    bump_yield: FixedDecimal,
    node_content_hash: ContentHash,
    curve_binding_hash: ContentHash,
}

struct PreparedPosition {
    position_id: Ulid,
    instrument: VersionRef,
    quantity: DecimalValue,
    owner: ficant_domain::primitives::OwnerRef,
    bond_ref: AnalyticsObjectRef,
    rule_pack_ref: AnalyticsObjectRef,
    snapshot_ref: AnalyticsObjectRef,
    curve_ref: AnalyticsObjectRef,
    valuation_at: MarketTime,
    settlement_date: NaiveDate,
    calendar_binding: CalendarBinding,
    terms: BondTerms,
    maturity_date: NaiveDate,
    bound_factors: BTreeMap<String, ContentHash>,
    lineage: Vec<LineageRef>,
}

fn validate_position_snapshot(
    scope: &AccessScope,
    snapshot: &PositionSnapshot,
    command: &CalculateBondKeyRateDv01Command,
) -> ApplicationResult<()> {
    scope.authorize(snapshot.owner())?;
    if snapshot.id() != command.position_snapshot_id()
        || snapshot.visible_at().instant() > command.knowledge_at().instant()
    {
        return Err(lineage());
    }
    Ok(())
}

fn validate_curve_snapshot(
    scope: &AccessScope,
    curve: &ficant_domain::market::CurveSnapshot,
    snapshot: &PositionSnapshot,
    command: &CalculateBondKeyRateDv01Command,
) -> ApplicationResult<()> {
    scope.authorize(curve.owner())?;
    if curve.id() != command.curve_snapshot_id()
        || curve.owner() != snapshot.owner()
        || curve.as_of() != command.valuation_at()
        || curve
            .visible_at()
            .is_none_or(|value| value.instant() > command.knowledge_at().instant())
        || curve.curve_family_id().is_none()
        || curve.point_schema() != crate::ports::CURVE_POINT_SCHEMA
    {
        return Err(lineage());
    }
    Ok(())
}

fn validate_node(
    curve: &ficant_domain::market::CurveSnapshot,
    point: &crate::ports::DecodedCurvePoint,
    node: &CurveNodeDefinition,
) -> ApplicationResult<()> {
    if node.curve_node_id() != point.curve_node_id()
        || node.content_hash() != point.curve_node_content_hash()
        || Some(node.curve_family_id()) != curve.curve_family_id()
        || node.factor_unit() != point.yield_to_maturity().unit()
    {
        return Err(lineage());
    }
    Ok(())
}

fn validate_factor(
    point: &crate::ports::DecodedCurvePoint,
    node: &CurveNodeDefinition,
    factor: &FactorDefinition,
) -> ApplicationResult<()> {
    if factor.factor_unit() != node.factor_unit()
        || factor.convention().bump().unit() != point.yield_to_maturity().unit()
        || factor.convention().curve_rebuild() != CurveRebuildPolicy::Rebuild
        || factor.convention().second_order() != SecondOrderPolicy::Exclude
    {
        return Err(validation());
    }
    Ok(())
}

fn require_open_settlement(calendar: &Calendar, settlement: NaiveDate) -> ApplicationResult<()> {
    let is_open = calendar.sessions().iter().any(|session| {
        session.local_date() == settlement
            && session.open_local_time().is_some()
            && session.close_local_time().is_some()
    });
    if !is_open {
        return Err(validation());
    }
    Ok(())
}

fn calendar_binding(
    calendar: &Calendar,
    content_hash: ContentHash,
) -> ApplicationResult<CalendarBinding> {
    let mut non_business_days = Vec::new();
    let mut work_weekends = Vec::new();
    for session in calendar.sessions() {
        if session.open_local_time().is_none() {
            non_business_days.push(session.local_date());
        } else if matches!(session.local_date().weekday(), Weekday::Sat | Weekday::Sun) {
            work_weekends.push(session.local_date());
        }
    }
    CalendarBinding::new(
        calendar.identity(),
        Version::new(calendar.version()).map_err(map_domain_error)?,
        content_hash,
        calendar.effective().from().local_trading_date(),
        calendar.effective().to().local_trading_date(),
        non_business_days,
        work_weekends,
    )
    .map_err(map_domain_error)
}

fn yield_curve(
    curve: &ficant_domain::market::CurveSnapshot,
    axis: &[AxisPoint],
    bump: Option<(usize, bool)>,
) -> ApplicationResult<YieldCurveBinding> {
    let reference = AnalyticsObjectRef::new(
        VersionRef::new(
            curve.id().clone(),
            Version::new(1).map_err(map_domain_error)?,
        ),
        curve.content_hash().clone(),
    );
    yield_curve_binding(reference, curve.as_of().local_trading_date(), axis, bump)
}

fn yield_curve_from_axis(
    reference: AnalyticsObjectRef,
    axis: &[AxisPoint],
    valuation_date: NaiveDate,
    bump: Option<(usize, bool)>,
) -> ApplicationResult<YieldCurveBinding> {
    yield_curve_binding(reference, valuation_date, axis, bump)
}

fn yield_curve_binding(
    reference: AnalyticsObjectRef,
    valuation_date: NaiveDate,
    axis: &[AxisPoint],
    bump: Option<(usize, bool)>,
) -> ApplicationResult<YieldCurveBinding> {
    let mut nodes = axis
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let yield_to_maturity = match bump {
                Some((target, true)) if target == index => value
                    .yield_to_maturity
                    .checked_add(value.bump_yield)
                    .map_err(map_domain_error)?,
                Some((target, false)) if target == index => value
                    .yield_to_maturity
                    .checked_sub(value.bump_yield)
                    .map_err(map_domain_error)?,
                _ => value.yield_to_maturity,
            };
            YieldCurveNode::new(value.maturity_date, yield_to_maturity).map_err(map_domain_error)
        })
        .collect::<ApplicationResult<Vec<_>>>()?;
    nodes.sort_by_key(|node| node.maturity_date());
    YieldCurveBinding::new(
        reference,
        valuation_date,
        YieldCurveInterpolation::LinearYield,
        nodes,
    )
    .map_err(map_domain_error)
}

fn tenor_date(as_of: NaiveDate, tenor: &str) -> ApplicationResult<NaiveDate> {
    let amount = tenor
        .get(1..tenor.len().saturating_sub(1))
        .ok_or_else(validation)?
        .parse::<u32>()
        .map_err(|_| validation())?;
    match tenor.as_bytes().last() {
        Some(b'Y') => as_of
            .checked_add_months(Months::new(amount.checked_mul(12).ok_or_else(validation)?))
            .ok_or_else(validation),
        Some(b'M') => as_of
            .checked_add_months(Months::new(amount))
            .ok_or_else(validation),
        Some(b'D') => as_of
            .checked_add_days(Days::new(u64::from(amount)))
            .ok_or_else(validation),
        _ => Err(validation()),
    }
}

fn decimal_to_fixed(value: &DecimalValue) -> ApplicationResult<FixedDecimal> {
    if value.scale() > 12 {
        return Err(validation());
    }
    let coefficient = value
        .coefficient()
        .parse::<i128>()
        .map_err(|_| validation())?;
    let factor = 10_i128
        .checked_pow(12 - value.scale())
        .ok_or_else(validation)?;
    Ok(FixedDecimal::from_scaled(
        coefficient.checked_mul(factor).ok_or_else(validation)?,
    ))
}

fn validate_decimal_unit(
    value: &DecimalValue,
    unit: &Unit,
    expected_dimension: &str,
    expected_owner: &ficant_domain::primitives::OwnerRef,
) -> ApplicationResult<()> {
    let reference_matches = unit.identity() == value.unit().unit_id().as_str()
        && unit.version() == value.unit().version().get();
    if !reference_matches
        || unit.owner() != expected_owner
        || unit.dimension() != expected_dimension
        || value.scale() > unit.scale()
        || decimal_precision(value.coefficient()) > unit.precision()
    {
        return Err(validation());
    }
    Ok(())
}

fn validate_fixed_output(value: FixedDecimal, unit: &Unit) -> ApplicationResult<()> {
    if unit.scale() < FIXED_DECIMAL_SCALE
        || decimal_precision(&value.scaled().to_string()) > unit.precision()
    {
        return Err(validation());
    }
    Ok(())
}

fn decimal_precision(coefficient: &str) -> u32 {
    u32::try_from(coefficient.trim_start_matches('-').len()).unwrap_or(u32::MAX)
}

fn scale_by_notional(
    value: FixedDecimal,
    quantity: &DecimalValue,
    registered_face: FixedDecimal,
) -> ApplicationResult<FixedDecimal> {
    if !registered_face.is_positive() {
        return Err(validation());
    }
    let quantity = decimal_to_fixed(quantity)?;
    let numerator = value
        .scaled()
        .checked_mul(quantity.scaled())
        .ok_or_else(validation)?;
    let denominator = registered_face.scaled();
    if numerator % denominator != 0 {
        return Err(validation());
    }
    Ok(FixedDecimal::from_scaled(numerator / denominator))
}

fn trace_for(command: &CalculateBondKeyRateDv01Command) -> ApplicationResult<SafeTraceContext> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(command.position_snapshot_id().as_str().as_bytes());
    bytes.extend_from_slice(command.curve_snapshot_id().as_str().as_bytes());
    bytes.extend_from_slice(&command.knowledge_at().instant().timestamp().to_be_bytes());
    let digest = ContentHash::digest(&bytes);
    let token =
        digest.as_bytes()[..16]
            .iter()
            .fold(String::with_capacity(32), |mut output, byte| {
                use std::fmt::Write as _;
                write!(output, "{byte:02x}").expect("writing to String cannot fail");
                output
            });
    SafeTraceContext::new(token)
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn lineage() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

fn not_found() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::NotFound, false)
}
