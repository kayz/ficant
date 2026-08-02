use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, ApplicationResult, BondAnalyticsEngine,
    CurvePointSetDecoder, CurveSnapshotMetadata, CurveSnapshotMetadataRepository,
    DecodedCurvePoint, DecodedCurvePointSet, DefinitionIdentity, DefinitionRepository,
    DefinitionValue, FactorTopologyRepository, IdempotencyKey, IntegrityEvent, IntegrityEventSink,
    PositionSnapshotRepository, RequiredVerifiedBlobRead, VerifiedBlobPayload, VerifiedBlobReader,
    YieldCurveEngine,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, CalculateBondKeyRateDv01,
    CalculateBondKeyRateDv01Command,
};
use ficant_domain::ContentAddressed;
use ficant_domain::analytics::{
    AnalyticsError, AnalyticsMeasures, BondAnalyticsInput, BondAnalyticsResult, CalendarResolution,
    DerivedCashflow, FixedDecimal,
};
use ficant_domain::curves::{YieldCurvePoint, YieldCurveQuery};
use ficant_domain::market::{
    ArtifactInputKind, Bond, BondBusinessDayConvention, BondCouponFrequency,
    BondDayCountConvention, BondPricingTerms, BondTaxAttributes, Calendar, CalendarInput,
    CalendarSession, CurveSnapshot, CurveSnapshotInput, IncomeTaxStatus, Instrument,
    InstrumentInput, InstrumentKind, MarketRulePack, MarketRulePackInput, Unit, UnitInput,
    ValueAddedTaxStatus, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{
    AccountingClassification, AccountingClassificationState, CurveNodeDefinition,
    CurveNodeDefinitionInput, CurveRebuildPolicy, FactorDefinition, FactorDefinitionInput,
    FactorTarget, FactorTargetBinding, Position, PositionHoldingForm, PositionInput,
    PositionSnapshot, PositionSnapshotInput, SecondOrderPolicy, SensitivityConvention,
    SensitivityDirection,
};

#[tokio::test]
async fn materializes_two_bonds_three_factors_exact_totals_and_fails_before_unsupported_engines() {
    let calls = EngineCalls::default();
    let fixture = Fixture::new(false, SensitivityDirection::Central, "1", calls.clone());
    let result = fixture.execute().await.unwrap();
    assert_eq!(result.positions().len(), 2);
    assert_eq!(result.totals().len(), 3);
    assert_complete_bond_coverage(&result);
    for position in result.positions() {
        assert_eq!(
            position
                .exposures()
                .iter()
                .map(ficant_domain::research::FactorDv01::factor_id)
                .collect::<Vec<_>>(),
            FACTOR_IDS
        );
        assert!(
            position
                .input_evidence_hashes()
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
        for (node, factor) in fixture.nodes.iter().zip(&fixture.factors) {
            let curve_binding = FactorTargetBinding::new(
                factor.factor_id(),
                FactorTarget::CurveNode(
                    ficant_domain::research::CurveNodeRef::new(
                        node.curve_node_id(),
                        node.content_hash().clone(),
                    )
                    .unwrap(),
                ),
            )
            .unwrap();
            assert!(
                position
                    .input_evidence_hashes()
                    .contains(node.content_hash())
            );
            assert!(
                position
                    .input_evidence_hashes()
                    .contains(curve_binding.content_hash())
            );
        }
        for exposure in position
            .exposures()
            .iter()
            .filter(|exposure| exposure.value() != FixedDecimal::ZERO)
        {
            let instrument_binding = FactorTargetBinding::new(
                exposure.factor_id(),
                FactorTarget::Instrument(ficant_domain::research::InstrumentFactorTarget::new(
                    owner(),
                    position.instrument().clone(),
                )),
            )
            .unwrap();
            assert!(
                position
                    .input_evidence_hashes()
                    .contains(instrument_binding.content_hash())
            );
        }
    }
    for (index, total) in result.totals().iter().enumerate() {
        let exact = result
            .positions()
            .iter()
            .map(|position| position.exposures()[index].value())
            .try_fold(FixedDecimal::ZERO, FixedDecimal::checked_add)
            .unwrap();
        assert_eq!(total.value(), exact);
        assert_eq!(total.unit(), &unit_ref('D'));
    }
    assert_ne!(result.totals()[1].value(), FixedDecimal::ZERO);
    assert_ne!(result.totals()[2].value(), FixedDecimal::ZERO);
    assert!(calls.curve.load(Ordering::SeqCst) > 0);
    assert!(calls.bond.load(Ordering::SeqCst) > 0);

    let changed = Fixture::new(false, SensitivityDirection::Up, "2", EngineCalls::default())
        .execute()
        .await
        .unwrap();
    assert_ne!(changed.totals(), result.totals());

    let rejected_calls = EngineCalls::default();
    let error = Fixture::new(
        true,
        SensitivityDirection::Central,
        "1",
        rejected_calls.clone(),
    )
    .execute()
    .await
    .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert_eq!(rejected_calls.curve.load(Ordering::SeqCst), 0);
    assert_eq!(rejected_calls.bond.load(Ordering::SeqCst), 0);
}

fn assert_complete_bond_coverage(result: &ficant_domain::research::PortfolioKeyRateExposure) {
    assert_eq!(result.coverage().imported_position_count(), 2);
    assert_eq!(result.coverage().participating_position_count(), 2);
    assert_eq!(
        result.coverage().source_confidence(),
        Some(result.source_confidence())
    );
    assert_eq!(
        result
            .coverage()
            .distinct_external_data_source_version_count(),
        0
    );
}

#[tokio::test]
async fn registered_face_value_is_normalized_before_position_notional_scaling() {
    let baseline = Fixture::new(
        false,
        SensitivityDirection::Central,
        "1",
        EngineCalls::default(),
    )
    .execute()
    .await
    .unwrap();
    let nonstandard_face = Fixture::new_with_first_face(
        false,
        SensitivityDirection::Central,
        "1",
        EngineCalls::default(),
        "200",
    )
    .execute()
    .await
    .unwrap();

    assert_eq!(
        baseline.positions()[0].exposures(),
        nonstandard_face.positions()[0].exposures(),
        "the same notional holding must not change merely because the registered denomination changes"
    );
    assert_ne!(
        baseline.positions()[0].content_hash(),
        nonstandard_face.positions()[0].content_hash(),
        "the changed registered Bond definition must still remain visible in lineage"
    );
}

#[tokio::test]
async fn incompatible_registered_unit_shapes_fail_before_any_pricing_engine() {
    for replacement in [
        unit_with_shape("DV01_CNY", "dv01", 'D', 11, 28),
        unit_with_shape("FACE_CNY", "notional", 'N', 0, 2),
        unit_with_shape("RATE", "price", 'V', 12, 28),
    ] {
        let calls = EngineCalls::default();
        let mut fixture = Fixture::new(false, SensitivityDirection::Central, "1", calls.clone());
        fixture.replace_unit(replacement);

        let error = fixture.execute().await.unwrap_err();
        assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
        assert_eq!(calls.curve.load(Ordering::SeqCst), 0);
        assert_eq!(calls.bond.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn request_contract_binds_exact_snapshots_times_and_output_unit() {
    let fixture = Fixture::new(
        false,
        SensitivityDirection::Central,
        "1",
        EngineCalls::default(),
    );
    let command = fixture.command();
    assert_eq!(command.position_snapshot_id(), fixture.snapshot.id());
    assert_eq!(command.curve_snapshot_id(), fixture.curve.id());
    assert_eq!(command.knowledge_at(), &market_time(2));
    assert_eq!(command.valuation_at(), &market_time(1));
    assert_eq!(command.dv01_unit(), &unit_ref('D'));
}

#[derive(Clone, Default)]
struct EngineCalls {
    curve: std::sync::Arc<AtomicUsize>,
    bond: std::sync::Arc<AtomicUsize>,
}

struct Fixture {
    scope: AccessScope,
    snapshot: PositionSnapshot,
    curve: CurveSnapshot,
    bytes: Vec<u8>,
    points: DecodedCurvePointSet,
    definitions: Vec<DefinitionValue>,
    nodes: Vec<CurveNodeDefinition>,
    factors: Vec<FactorDefinition>,
    instrument_ids: Vec<Ulid>,
    calls: EngineCalls,
}

impl Fixture {
    #[allow(clippy::too_many_lines)]
    fn new(
        unsupported: bool,
        direction: SensitivityDirection,
        bump_coefficient: &str,
        calls: EngineCalls,
    ) -> Self {
        Self::new_with_first_face(unsupported, direction, bump_coefficient, calls, "100")
    }

    #[allow(clippy::too_many_lines)]
    fn new_with_first_face(
        unsupported: bool,
        direction: SensitivityDirection,
        bump_coefficient: &str,
        calls: EngineCalls,
        first_face: &str,
    ) -> Self {
        let owner = owner();
        let scope = AccessScope::new(id('T'), id('A'), vec![id('0')]).unwrap();
        let currency = unit("CNY", "currency", 'C');
        let notional = unit("FACE_CNY", "notional", 'N');
        let dv01 = unit("DV01_CNY", "dv01", 'D');
        let rate = unit("RATE", "rate", 'V');
        let calendar = calendar();
        let rule_pack = rule_pack();
        let first = instrument('B', InstrumentKind::Bond);
        let second = instrument('E', InstrumentKind::Bond);
        let first_definition = DefinitionValue::Instrument(
            ficant_application::ports::InstrumentDefinition::new(
                first.clone(),
                Some(ficant_application::ports::InstrumentSubtype::Bond(
                    bond_with_face(&first, date(2031, 8, 3), first_face),
                )),
            )
            .unwrap(),
        );
        let second_definition = DefinitionValue::Instrument(
            ficant_application::ports::InstrumentDefinition::new(
                second.clone(),
                Some(ficant_application::ports::InstrumentSubtype::Bond(bond(
                    &second,
                    date(2036, 8, 3),
                ))),
            )
            .unwrap(),
        );
        let other = instrument('F', InstrumentKind::Other);
        let other_definition = DefinitionValue::Instrument(
            ficant_application::ports::InstrumentDefinition::new(other.clone(), None).unwrap(),
        );
        let instruments = if unsupported {
            vec![first.clone(), other]
        } else {
            vec![first.clone(), second.clone()]
        };
        let positions = instruments
            .iter()
            .enumerate()
            .map(|(index, value)| position(char::from(b'P' + u8::try_from(index).unwrap()), value))
            .collect::<Vec<_>>();
        let snapshot = position_snapshot(positions);
        let nodes = [
            ("cn.gov.yield-curve.02y", "P2Y"),
            ("cn.gov.yield-curve.05y", "P5Y"),
            ("cn.gov.yield-curve.10y", "P10Y"),
        ]
        .into_iter()
        .map(|(node_id, tenor)| curve_node(node_id, tenor))
        .collect::<Vec<_>>();
        let factors = FACTOR_IDS
            .iter()
            .map(|factor_id| factor(factor_id, direction, bump_coefficient))
            .collect::<Vec<_>>();
        let points = DecodedCurvePointSet::new(
            FAMILY,
            nodes
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    DecodedCurvePoint::new(
                        node.curve_node_id(),
                        node.content_hash().clone(),
                        decimal(&(20 + index * 5).to_string(), 3, unit_ref('V')),
                    )
                    .unwrap()
                })
                .collect(),
        )
        .unwrap();
        let bytes = b"canonical-r4d-a-curve-points".to_vec();
        let curve = CurveSnapshot::new(CurveSnapshotInput {
            curve_snapshot_id: id('S'),
            owner: owner.clone(),
            as_of: market_time(1),
            currency: unit_ref('C'),
            curve_kind: "YTM".to_owned(),
            calendar: VersionRef::new(id('K'), version()),
            rule_pack: VersionRef::new(id('R'), version()),
            point_schema: ficant_application::ports::CURVE_POINT_SCHEMA.to_owned(),
            content_hash: ContentHash::digest(&bytes),
            lineage: vec![lineage('L')],
            input_kind: ArtifactInputKind::ExternalFixture,
        })
        .unwrap()
        .with_knowledge_time(market_time(2), FAMILY)
        .unwrap();
        Self {
            scope,
            snapshot,
            curve,
            bytes,
            points,
            definitions: vec![
                DefinitionValue::Unit(currency),
                DefinitionValue::Unit(notional),
                DefinitionValue::Unit(dv01),
                DefinitionValue::Unit(rate),
                DefinitionValue::Calendar(calendar),
                DefinitionValue::MarketRulePack(rule_pack),
                first_definition,
                second_definition,
                other_definition,
            ],
            nodes,
            factors,
            instrument_ids: instruments
                .into_iter()
                .map(|value| value.id().clone())
                .collect(),
            calls,
        }
    }

    fn command(&self) -> CalculateBondKeyRateDv01Command {
        CalculateBondKeyRateDv01Command::new(
            self.snapshot.id().clone(),
            market_time(2),
            market_time(1),
            self.curve.id().clone(),
            unit_ref('D'),
        )
        .unwrap()
    }

    async fn execute(
        &self,
    ) -> ApplicationResult<ficant_domain::research::PortfolioKeyRateExposure> {
        CalculateBondKeyRateDv01::new(self, self, self, self, self, self, self, self, self)
            .execute(&self.scope, self.command())
            .await
    }

    fn replace_unit(&mut self, replacement: Unit) {
        self.definitions.retain(|value| {
            !matches!(value, DefinitionValue::Unit(unit) if unit.code() == replacement.code())
        });
        self.definitions.push(DefinitionValue::Unit(replacement));
    }
}

#[async_trait]
impl PositionSnapshotRepository for Fixture {
    async fn get_position_snapshot(
        &self,
        _: &AccessScope,
        snapshot_id: Ulid,
        _: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>> {
        Ok((snapshot_id == *self.snapshot.id()).then(|| self.snapshot.clone()))
    }

    async fn resolve_position_snapshot(
        &self,
        _: &AccessScope,
        _: VersionRef,
        _: MarketTime,
        _: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>> {
        Ok(None)
    }
}

#[async_trait]
impl CurveSnapshotMetadataRepository for Fixture {
    async fn get_curve_snapshot_metadata(
        &self,
        _: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> ApplicationResult<Option<CurveSnapshotMetadata>> {
        Ok((curve_snapshot_id == *self.curve.id()).then(|| {
            CurveSnapshotMetadata::new(self.curve.clone(), self.bytes.len() as u64).unwrap()
        }))
    }
}

#[async_trait]
impl DefinitionRepository for Fixture {
    async fn create_identity(&self, _: DefinitionIdentity) -> ApplicationResult<()> {
        Err(unavailable())
    }

    async fn append_version(
        &self,
        _: AppendDefinitionVersion,
    ) -> ApplicationResult<DefinitionValue> {
        Err(unavailable())
    }

    async fn get_version(
        &self,
        _: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        Ok(self
            .definitions
            .iter()
            .find(|value| {
                value.identity() == definition_id.as_str() && value.version() == version.get()
            })
            .cloned())
    }

    async fn resolve_as_of(
        &self,
        _: &AccessScope,
        _: Ulid,
        _: MarketTime,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        Err(unavailable())
    }
}

#[async_trait]
impl FactorTopologyRepository for Fixture {
    async fn register_factor_definition(
        &self,
        _: &AccessScope,
        _: FactorDefinition,
        _: IdempotencyKey,
    ) -> ApplicationResult<FactorDefinition> {
        Err(unavailable())
    }

    async fn register_curve_node_definition(
        &self,
        _: &AccessScope,
        _: CurveNodeDefinition,
        _: IdempotencyKey,
    ) -> ApplicationResult<CurveNodeDefinition> {
        Err(unavailable())
    }

    async fn bind_factor_target(
        &self,
        _: &AccessScope,
        _: FactorTargetBinding,
        _: IdempotencyKey,
    ) -> ApplicationResult<FactorTargetBinding> {
        Err(unavailable())
    }

    async fn get_factor_definition(
        &self,
        factor_id: &str,
    ) -> ApplicationResult<Option<FactorDefinition>> {
        Ok(self
            .factors
            .iter()
            .find(|value| value.factor_id() == factor_id)
            .cloned())
    }

    async fn get_curve_node_definition(
        &self,
        curve_node_id: &str,
    ) -> ApplicationResult<Option<CurveNodeDefinition>> {
        Ok(self
            .nodes
            .iter()
            .find(|value| value.curve_node_id() == curve_node_id)
            .cloned())
    }

    async fn get_factor_targets(
        &self,
        _: &AccessScope,
        _: &str,
    ) -> ApplicationResult<Vec<FactorTargetBinding>> {
        Err(unavailable())
    }

    async fn get_target_factors(
        &self,
        _: &AccessScope,
        target: &FactorTarget,
    ) -> ApplicationResult<Vec<FactorDefinition>> {
        match target {
            FactorTarget::CurveNode(node) => self
                .nodes
                .iter()
                .position(|value| {
                    value.curve_node_id() == node.curve_node_id()
                        && value.content_hash() == node.content_hash()
                })
                .map_or_else(
                    || Ok(Vec::new()),
                    |index| Ok(vec![self.factors[index].clone()]),
                ),
            FactorTarget::Instrument(instrument)
                if instrument.owner() == &owner()
                    && self.instrument_ids.contains(instrument.instrument().id()) =>
            {
                Ok(self.factors.clone())
            }
            FactorTarget::Instrument(_) => Ok(Vec::new()),
        }
    }

    async fn exact_target_exists(&self, _: &FactorTarget) -> ApplicationResult<bool> {
        Ok(true)
    }
}

impl CurvePointSetDecoder for Fixture {
    fn decode_canonical(&self, _: &[u8]) -> ApplicationResult<DecodedCurvePointSet> {
        Ok(self.points.clone())
    }
}

#[async_trait]
impl VerifiedBlobReader for Fixture {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<VerifiedBlobPayload> {
        request.verify_bytes(sink, self.bytes.clone()).await
    }
}

#[async_trait]
impl IntegrityEventSink for Fixture {
    async fn emit(&self, _: IntegrityEvent) -> ApplicationResult<()> {
        Ok(())
    }
}

impl YieldCurveEngine for Fixture {
    fn interpolate(&self, query: &YieldCurveQuery) -> Result<YieldCurvePoint, AnalyticsError> {
        self.calls.curve.fetch_add(1, Ordering::SeqCst);
        let value = query
            .curve()
            .nodes()
            .iter()
            .find(|node| node.maturity_date() == query.query_date())
            .map(|node| node.yield_to_maturity())
            .ok_or(AnalyticsError::InvalidInput)?;
        YieldCurvePoint::new(query.clone(), value).map_err(|_| AnalyticsError::InvalidInput)
    }
}

impl BondAnalyticsEngine for Fixture {
    fn calculate(&self, input: &BondAnalyticsInput) -> Result<BondAnalyticsResult, AnalyticsError> {
        self.calls.bond.fetch_add(1, Ordering::SeqCst);
        let ytm = input.input_value();
        let quadratic = ytm
            .scaled()
            .checked_mul(ytm.scaled())
            .and_then(|value| value.checked_div(FixedDecimal::ONE.scaled()))
            .and_then(|value| value.checked_mul(1_000))
            .ok_or(AnalyticsError::NonFinite)?;
        let per_hundred = 100 * FixedDecimal::ONE.scaled() - 100 * ytm.scaled() - quadratic;
        let clean = FixedDecimal::from_scaled(
            per_hundred
                .checked_mul(input.terms().face_amount().scaled())
                .and_then(|value| value.checked_div(100 * FixedDecimal::ONE.scaled()))
                .ok_or(AnalyticsError::NonFinite)?,
        );
        let measures = AnalyticsMeasures::new(
            FixedDecimal::ZERO,
            clean,
            ytm,
            FixedDecimal::ONE,
            FixedDecimal::ONE,
            FixedDecimal::ONE,
            FixedDecimal::from_scaled(10_000_000_000),
        )
        .map_err(|_| AnalyticsError::InvalidInput)?;
        let cashflow = DerivedCashflow::new(
            1,
            input.terms().maturity_date(),
            input.terms().maturity_date(),
            FixedDecimal::ONE,
            FixedDecimal::from_scaled(100 * FixedDecimal::ONE.scaled()),
            FixedDecimal::from_scaled(101 * FixedDecimal::ONE.scaled()),
        )
        .map_err(|_| AnalyticsError::InvalidInput)?;
        BondAnalyticsResult::new(
            input.clone(),
            CalendarResolution::Exact,
            vec![cashflow],
            measures,
        )
        .map_err(|_| AnalyticsError::InvalidInput)
    }
}

const FAMILY: &str = "cn.gov.yield-curve";
const FACTOR_IDS: [&str; 3] = ["cn.gov.yield.02y", "cn.gov.yield.05y", "cn.gov.yield.10y"];

fn unit(code: &str, dimension: &str, suffix: char) -> Unit {
    unit_with_shape(code, dimension, suffix, 12, 28)
}

fn unit_with_shape(code: &str, dimension: &str, suffix: char, scale: u32, precision: u32) -> Unit {
    Unit::new(UnitInput {
        unit_id: id(suffix),
        version: version(),
        owner: owner(),
        code: code.to_owned(),
        dimension: dimension.to_owned(),
        scale,
        precision,
    })
    .unwrap()
}

fn calendar() -> Calendar {
    Calendar::new(CalendarInput {
        calendar_id: id('K'),
        version: version(),
        owner: owner(),
        market: "CN".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: EffectivePeriod::new(market_time_for(2020, 1), market_time_for(2040, 1))
            .unwrap(),
        sessions: vec![
            CalendarSession::open(
                date(2026, 8, 3),
                NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
            )
            .unwrap(),
        ],
    })
    .unwrap()
}

fn rule_pack() -> MarketRulePack {
    MarketRulePack::new(MarketRulePackInput {
        rule_pack_id: id('R'),
        version: version(),
        owner: owner(),
        market: "CN".to_owned(),
        rule_type: "bond-pricing".to_owned(),
        source: "fixture".to_owned(),
        effective: EffectivePeriod::new(market_time_for(2020, 1), market_time_for(2040, 1))
            .unwrap(),
        verification_status: VerificationStatus::Verified,
        content_hash: ContentHash::digest(b"r4d-a-rule-pack"),
    })
    .unwrap()
}

fn instrument(suffix: char, kind: InstrumentKind) -> Instrument {
    Instrument::new(InstrumentInput {
        instrument_id: id(suffix),
        version: version(),
        owner: owner(),
        kind,
        market: "CN".to_owned(),
        symbol: format!("INSTRUMENT-{suffix}"),
        currency: unit_ref('C'),
        calendar: VersionRef::new(id('K'), version()),
    })
    .unwrap()
}

fn bond(instrument: &Instrument, maturity: NaiveDate) -> Bond {
    bond_with_face(instrument, maturity, "100")
}

fn bond_with_face(instrument: &Instrument, maturity: NaiveDate, face: &str) -> Bond {
    Bond::with_issuance(
        instrument,
        date(2024, 1, 15),
        date(2024, 1, 15),
        maturity,
        decimal("100000000", 0, unit_ref('N')),
        BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt),
        decimal(face, 0, unit_ref('N')),
    )
    .unwrap()
    .with_pricing_terms(
        BondPricingTerms::new(
            decimal("25", 3, unit_ref('V')),
            BondCouponFrequency::Semiannual,
            BondDayCountConvention::ActActBondIsma,
            BondBusinessDayConvention::Following,
        )
        .unwrap(),
    )
    .unwrap()
}

fn position(suffix: char, instrument: &Instrument) -> Position {
    let value = decimal("1", 0, unit_ref('C'));
    Position::new(PositionInput {
        position_id: id(suffix),
        instrument_ref: instrument.version_ref(),
        quantity: decimal("100", 0, unit_ref('N')),
        economic_value: value.clone(),
        economic_pnl: value.clone(),
        accounting_pnl: value.clone(),
        capital_requirement: value,
        accounting_classification: AccountingClassification::new(
            AccountingClassificationState::NotApplicable,
            None,
        )
        .unwrap(),
        holding_form: PositionHoldingForm::Owned,
    })
    .unwrap()
}

fn position_snapshot(positions: Vec<Position>) -> PositionSnapshot {
    let mut input = PositionSnapshotInput {
        snapshot_id: id('P'),
        owner: owner(),
        subject_ref: VersionRef::new(id('Q'), version()),
        observed_at: market_time(1),
        visible_at: market_time(2),
        content_hash: ContentHash::digest(b"placeholder"),
        lineage: vec![lineage('L')],
        positions,
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    PositionSnapshot::new(input).unwrap()
}

fn curve_node(node_id: &str, tenor: &str) -> CurveNodeDefinition {
    let mut input = CurveNodeDefinitionInput {
        curve_node_id: node_id.to_owned(),
        curve_family_id: FAMILY.to_owned(),
        tenor: tenor.to_owned(),
        factor_unit: unit_ref('V'),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = CurveNodeDefinition::content_hash_for(&input);
    CurveNodeDefinition::new(input).unwrap()
}

fn factor(
    id_value: &str,
    direction: SensitivityDirection,
    bump_coefficient: &str,
) -> FactorDefinition {
    let mut input = FactorDefinitionInput {
        factor_id: id_value.to_owned(),
        factor_unit: unit_ref('V'),
        convention: SensitivityConvention::new(
            decimal(bump_coefficient, 4, unit_ref('V')),
            direction,
            CurveRebuildPolicy::Rebuild,
            SecondOrderPolicy::Exclude,
        )
        .unwrap(),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = FactorDefinition::content_hash_for(&input);
    FactorDefinition::new(input).unwrap()
}

fn market_time(hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 8, 3, hour, 0, 0)
            .single()
            .unwrap(),
        "Asia/Shanghai",
        date(2026, 8, 3),
    )
    .unwrap()
}

fn market_time_for(year: i32, hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, 1, 1, hour, 0, 0)
            .single()
            .unwrap(),
        "Asia/Shanghai",
        date(year, 1, 1),
    )
    .unwrap()
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn decimal(value: &str, scale: u32, unit: UnitRef) -> DecimalValue {
    DecimalValue::new(value, scale, unit).unwrap()
}

fn lineage(suffix: char) -> LineageRef {
    LineageRef::new(
        id(suffix),
        Some(version()),
        Some(ContentHash::digest(&[suffix as u8])),
    )
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('0'))
}

fn unit_ref(suffix: char) -> UnitRef {
    UnitRef::new(id(suffix), version())
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => '1',
        'L' => '2',
        'O' => '3',
        'U' => '4',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, false)
}
