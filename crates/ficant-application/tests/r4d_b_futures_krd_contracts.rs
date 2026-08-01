use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, ApplicationResult, BondAnalyticsEngine, CanonicalQuote,
    CanonicalSnapshotDecoder, CurvePointSetDecoder, CurveSnapshotMetadata,
    CurveSnapshotMetadataRepository, DecodedCurvePoint, DecodedCurvePointSet, DefinitionIdentity,
    DefinitionRepository, DefinitionValue, FactorTopologyRepository, FuturesDeliveryEngine,
    FuturesDeliveryRuleParser, IdempotencyKey, IntegrityEvent, IntegrityEventSink,
    PositionSnapshotRepository, RequiredVerifiedBlobRead, SnapshotVerifiedReadMetadata,
    SnapshotVerifiedReadMetadataRepository, VerifiedBlobPayload, VerifiedBlobReader,
    VerifiedBlobRole, YieldCurveEngine,
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
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliverableInput, FuturesDeliveryMeasures, FuturesDeliveryResult,
    FuturesDeliveryRule, FuturesDeliveryRuleInput,
};
use ficant_domain::market::{
    ArtifactInputKind, Bond, BondBusinessDayConvention, BondCouponFrequency,
    BondDayCountConvention, BondPricingTerms, BondTaxAttributes, Calendar, CalendarInput,
    CalendarSession, CurveSnapshot, CurveSnapshotInput, FuturesContract, IncomeTaxStatus,
    Instrument, InstrumentInput, InstrumentKind, MarketRulePack, MarketRulePackInput,
    RulePackContent, Unit, UnitInput, ValueAddedTaxStatus, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{
    AccountingClassification, AccountingClassificationState, CurveNodeDefinition,
    CurveNodeDefinitionInput, CurveRebuildPolicy, DataSnapshot, DataSnapshotInput,
    FactorDefinition, FactorDefinitionInput, FactorTarget, FactorTargetBinding, Position,
    PositionHoldingForm, PositionInput, PositionSnapshot, PositionSnapshotInput, SecondOrderPolicy,
    SensitivityConvention, SensitivityDirection,
};

const CURVE_BYTES: &[u8] = b"canonical-r4d-b-curve";
const PARQUET: &[u8] = b"canonical-r4d-b-quotes";
const MANIFEST: &[u8] = b"canonical-r4d-b-manifest";
const RULE_TYPE: &str = "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack";
const FACTOR_IDS: [&str; 3] = ["cn.gov.yield.02y", "cn.gov.yield.05y", "cn.gov.yield.10y"];

#[tokio::test]
async fn mixed_portfolio_uses_one_fixed_ctd_and_exact_contract_scaling() {
    let calls = Calls::default();
    let fixture = Fixture::new(true, false, calls.clone());
    let result = fixture.execute(true).await.unwrap();

    assert_eq!(
        result.algorithm().algorithm_id(),
        "ficant.fixed-income.portfolio-key-rate-yield"
    );
    assert_eq!(result.algorithm().algorithm_version(), 1);
    assert_eq!(
        result.algorithm().convention_profile(),
        "linear-ytm-fixed-base-ctd-v1"
    );
    assert_eq!(result.futures_data_snapshot_id(), Some(fixture.data.id()));
    assert_eq!(result.positions().len(), 2);
    assert_eq!(
        calls.delivery.load(Ordering::SeqCst),
        2,
        "two base candidates are priced once each and never revisited under shocks"
    );
    assert!(calls.bond.load(Ordering::SeqCst) > 0);

    let bond = result
        .positions()
        .iter()
        .find(|value| value.instrument().id() == &id('B'))
        .unwrap();
    let future = result
        .positions()
        .iter()
        .find(|value| value.instrument().id() == &id('F'))
        .unwrap();
    for (bond_factor, future_factor) in bond.exposures().iter().zip(future.exposures()) {
        assert_eq!(bond_factor.factor_id(), future_factor.factor_id());
        assert_eq!(
            future_factor.value().scaled(),
            bond_factor.value().scaled() * 20_000,
            "registered-face KRD × 100 quote basis ÷ 100 face × 10,000 units × 2 contracts"
        );
        if future_factor.value() != FixedDecimal::ZERO {
            for target in [id('F'), id('B')] {
                let binding = FactorTargetBinding::new(
                    future_factor.factor_id(),
                    FactorTarget::Instrument(ficant_domain::research::InstrumentFactorTarget::new(
                        owner(),
                        VersionRef::new(target, version()),
                    )),
                )
                .unwrap();
                assert!(
                    future
                        .input_evidence_hashes()
                        .contains(binding.content_hash())
                );
            }
        }
    }
    assert_ne!(future.exposures()[0].value(), FixedDecimal::ZERO);
    assert_ne!(future.exposures()[1].value(), FixedDecimal::ZERO);
    assert_eq!(future.exposures()[2].value(), FixedDecimal::ZERO);
    for (index, total) in result.totals().iter().enumerate() {
        let expected = result
            .positions()
            .iter()
            .map(|position| position.exposures()[index].value())
            .try_fold(FixedDecimal::ZERO, FixedDecimal::checked_add)
            .unwrap();
        assert_eq!(total.value(), expected);
    }
}

#[tokio::test]
async fn missing_ctd_factor_binding_fails_before_any_shock_pricing() {
    let calls = Calls::default();
    let fixture = Fixture::new(false, true, calls.clone());
    let error = fixture.execute(true).await.unwrap_err();
    assert_eq!(
        error.category(),
        ApplicationErrorCategory::LineageIncomplete
    );
    assert_eq!(calls.delivery.load(Ordering::SeqCst), 2);
    assert_eq!(calls.curve.load(Ordering::SeqCst), 0);
    assert_eq!(calls.bond.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn missing_futures_factor_binding_fails_before_any_numerical_engine() {
    let calls = Calls::default();
    let fixture = Fixture::missing_futures_binding(calls.clone());
    let error = fixture.execute(true).await.unwrap_err();
    assert_eq!(
        error.category(),
        ApplicationErrorCategory::LineageIncomplete
    );
    assert_eq!(calls.delivery.load(Ordering::SeqCst), 0);
    assert_eq!(calls.curve.load(Ordering::SeqCst), 0);
    assert_eq!(calls.bond.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn snapshot_binding_is_required_exactly_when_futures_are_present() {
    let futures_calls = Calls::default();
    let futures = Fixture::new(false, false, futures_calls.clone());
    let error = futures.execute(false).await.unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert_eq!(futures_calls.delivery.load(Ordering::SeqCst), 0);
    assert_eq!(futures_calls.bond.load(Ordering::SeqCst), 0);

    let bond_calls = Calls::default();
    let bond = Fixture::bond_only(bond_calls.clone());
    let error = bond.execute(true).await.unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert_eq!(bond_calls.delivery.load(Ordering::SeqCst), 0);
    assert_eq!(bond_calls.bond.load(Ordering::SeqCst), 0);
}

#[derive(Clone, Default)]
struct Calls {
    curve: Arc<AtomicUsize>,
    bond: Arc<AtomicUsize>,
    parser: Arc<AtomicUsize>,
    delivery: Arc<AtomicUsize>,
}

struct Fixture {
    scope: AccessScope,
    snapshot: PositionSnapshot,
    curve: CurveSnapshot,
    data: DataSnapshot,
    points: DecodedCurvePointSet,
    definitions: Vec<DefinitionValue>,
    nodes: Vec<CurveNodeDefinition>,
    factors: Vec<FactorDefinition>,
    missing_binding: MissingBinding,
    calls: Calls,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MissingBinding {
    None,
    Ctd,
    Futures,
}

impl Fixture {
    fn new(mixed: bool, missing_ctd_binding: bool, calls: Calls) -> Self {
        let missing_binding = if missing_ctd_binding {
            MissingBinding::Ctd
        } else {
            MissingBinding::None
        };
        Self::build(mixed, true, missing_binding, calls)
    }

    fn bond_only(calls: Calls) -> Self {
        Self::build(true, false, MissingBinding::None, calls)
    }

    fn missing_futures_binding(calls: Calls) -> Self {
        Self::build(true, true, MissingBinding::Futures, calls)
    }

    #[allow(clippy::too_many_lines)]
    fn build(
        include_bond: bool,
        include_future: bool,
        missing_binding: MissingBinding,
        calls: Calls,
    ) -> Self {
        let currency = unit("CNY", "currency", 'C', 2);
        let notional = unit("FACE_CNY", "notional", 'N', 0);
        let dv01 = unit("DV01_CNY", "dv01", 'D', 12);
        let rate = unit("RATE", "rate", 'V', 12);
        let contracts = unit("CONTRACT", "contract_count", 'H', 0);
        let price = unit("CNY100", "price_per_100", 'P', 12);
        let calendar = calendar();
        let curve_rule = curve_rule_pack();
        let delivery_rule = delivery_rule_pack();
        let bond_instrument = instrument('B', InstrumentKind::Bond);
        let bond_definition = DefinitionValue::Instrument(
            ficant_application::ports::InstrumentDefinition::new(
                bond_instrument.clone(),
                Some(ficant_application::ports::InstrumentSubtype::Bond(bond(
                    &bond_instrument,
                ))),
            )
            .unwrap(),
        );
        let second_bond_instrument = instrument('G', InstrumentKind::Bond);
        let second_bond_definition = DefinitionValue::Instrument(
            ficant_application::ports::InstrumentDefinition::new(
                second_bond_instrument.clone(),
                Some(ficant_application::ports::InstrumentSubtype::Bond(bond(
                    &second_bond_instrument,
                ))),
            )
            .unwrap(),
        );
        let future_instrument = instrument('F', InstrumentKind::Futures);
        let future = FuturesContract::new(
            &future_instrument,
            time_for(2026, 9, 11, 11),
            time_for(2026, 9, 11, 15),
            time_for(2026, 9, 18, 8),
            decimal("1", 0, unit_ref('N')),
            VersionRef::new(id('X'), version()),
        )
        .unwrap()
        .with_risk_terms("T", unit_ref('P'))
        .unwrap();
        let future_definition = DefinitionValue::Instrument(
            ficant_application::ports::InstrumentDefinition::new(
                future_instrument.clone(),
                Some(ficant_application::ports::InstrumentSubtype::FuturesContract(future)),
            )
            .unwrap(),
        );
        let mut positions = Vec::new();
        if include_bond {
            positions.push(position('Q', &bond_instrument, "100", unit_ref('N')));
        }
        if include_future {
            positions.push(position('Z', &future_instrument, "2", unit_ref('H')));
        }
        let snapshot = position_snapshot(positions);
        let nodes = [
            ("cn.gov.yield-curve.02y", "P2Y"),
            ("cn.gov.yield-curve.05y", "P5Y"),
            ("cn.gov.yield-curve.10y", "P10Y"),
        ]
        .into_iter()
        .map(|(node, tenor)| curve_node(node, tenor))
        .collect::<Vec<_>>();
        let factors = FACTOR_IDS
            .iter()
            .map(|value| factor(value))
            .collect::<Vec<_>>();
        let points = DecodedCurvePointSet::new(
            "cn.gov.yield-curve",
            nodes
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    DecodedCurvePoint::new(
                        node.curve_node_id(),
                        node.content_hash().clone(),
                        decimal(&(25 + index * 5).to_string(), 3, unit_ref('V')),
                    )
                    .unwrap()
                })
                .collect(),
        )
        .unwrap();
        let curve = CurveSnapshot::new(CurveSnapshotInput {
            curve_snapshot_id: id('S'),
            owner: owner(),
            as_of: time(1),
            currency: unit_ref('C'),
            curve_kind: "YTM".to_owned(),
            calendar: VersionRef::new(id('K'), version()),
            rule_pack: VersionRef::new(id('R'), version()),
            point_schema: ficant_application::ports::CURVE_POINT_SCHEMA.to_owned(),
            content_hash: ContentHash::digest(CURVE_BYTES),
            lineage: vec![lineage('M')],
            input_kind: ArtifactInputKind::ExternalFixture,
        })
        .unwrap()
        .with_knowledge_time(time(2), "cn.gov.yield-curve")
        .unwrap();
        let data = DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: id('J'),
            owner: owner(),
            visible_at: time(2),
            as_of: time(1),
            schema_hash: ContentHash::digest(b"r4d-b-schema"),
            manifest_hash: ContentHash::digest(MANIFEST),
            blob_content_hash: ContentHash::digest(PARQUET),
            lineage: vec![lineage('W')],
        })
        .unwrap();
        Self {
            scope: AccessScope::new(id('T'), id('A'), vec![id('0')]).unwrap(),
            snapshot,
            curve,
            data,
            points,
            definitions: vec![
                DefinitionValue::Unit(currency),
                DefinitionValue::Unit(notional),
                DefinitionValue::Unit(dv01),
                DefinitionValue::Unit(rate),
                DefinitionValue::Unit(contracts),
                DefinitionValue::Unit(price),
                DefinitionValue::Calendar(calendar),
                DefinitionValue::MarketRulePack(curve_rule),
                DefinitionValue::MarketRulePack(delivery_rule),
                bond_definition,
                second_bond_definition,
                future_definition,
            ],
            nodes,
            factors,
            missing_binding,
            calls,
        }
    }

    async fn execute(
        &self,
        bind_futures_snapshot: bool,
    ) -> ApplicationResult<ficant_domain::research::PortfolioKeyRateExposure> {
        let command = if bind_futures_snapshot {
            CalculateBondKeyRateDv01Command::new_with_futures_data_snapshot(
                self.snapshot.id().clone(),
                time(2),
                time(1),
                self.curve.id().clone(),
                unit_ref('D'),
                self.data.id().clone(),
            )?
        } else {
            CalculateBondKeyRateDv01Command::new(
                self.snapshot.id().clone(),
                time(2),
                time(1),
                self.curve.id().clone(),
                unit_ref('D'),
            )?
        };
        CalculateBondKeyRateDv01::new_with_futures(
            self, self, self, self, self, self, self, self, self, self, self, self, self,
        )
        .execute(&self.scope, command)
        .await
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
            CurveSnapshotMetadata::new(self.curve.clone(), CURVE_BYTES.len() as u64).unwrap()
        }))
    }
}

#[async_trait]
impl SnapshotVerifiedReadMetadataRepository for Fixture {
    async fn get_verified_read_metadata(
        &self,
        _: &AccessScope,
        snapshot_id: Ulid,
    ) -> ApplicationResult<Option<SnapshotVerifiedReadMetadata>> {
        if snapshot_id != *self.data.id() {
            return Ok(None);
        }
        SnapshotVerifiedReadMetadata::data(
            self.data.clone(),
            PARQUET.len() as u64,
            MANIFEST.len() as u64,
        )
        .map(Some)
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
                    && instrument.instrument().id() == &id('B')
                    && self.missing_binding == MissingBinding::Ctd =>
            {
                Ok(Vec::new())
            }
            FactorTarget::Instrument(instrument)
                if instrument.owner() == &owner()
                    && instrument.instrument().id() == &id('F')
                    && self.missing_binding == MissingBinding::Futures =>
            {
                Ok(Vec::new())
            }
            FactorTarget::Instrument(instrument)
                if instrument.owner() == &owner()
                    && [id('B'), id('F')].contains(instrument.instrument().id()) =>
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
    fn decode_canonical(&self, bytes: &[u8]) -> ApplicationResult<DecodedCurvePointSet> {
        assert_eq!(bytes, CURVE_BYTES);
        Ok(self.points.clone())
    }
}

#[async_trait]
impl CanonicalSnapshotDecoder for Fixture {
    async fn decode_quotes(
        &self,
        snapshot: &DataSnapshot,
        parquet: &[u8],
        manifest: &[u8],
    ) -> ApplicationResult<Vec<CanonicalQuote>> {
        assert_eq!(snapshot, &self.data);
        assert_eq!(parquet, PARQUET);
        assert_eq!(manifest, MANIFEST);
        Ok(vec![
            quote('B', 100, 102),
            quote('G', 101, 103),
            quote('F', 99, 101),
        ])
    }
}

#[async_trait]
impl VerifiedBlobReader for Fixture {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<VerifiedBlobPayload> {
        let bytes = match request.blob_role() {
            VerifiedBlobRole::CurvePoints => CURVE_BYTES,
            VerifiedBlobRole::DataParquet => PARQUET,
            VerifiedBlobRole::DataManifest => MANIFEST,
            _ => unreachable!("R4d-b reads curve points and exact quote snapshot roles only"),
        };
        request.verify_bytes(sink, bytes.to_vec()).await
    }
}

#[async_trait]
impl IntegrityEventSink for Fixture {
    async fn emit(&self, _: IntegrityEvent) -> ApplicationResult<()> {
        unreachable!("all fixture hashes and lengths are exact")
    }
}

impl FuturesDeliveryRuleParser for Fixture {
    fn market(&self) -> &'static str {
        "CFFEX"
    }

    fn rule_type(&self) -> &'static str {
        "cgb-futures"
    }

    fn type_url(&self) -> &'static str {
        RULE_TYPE
    }

    fn parse_product_code(&self, product_code: &str) -> ApplicationResult<CgbFuturesProduct> {
        (product_code == "T")
            .then_some(CgbFuturesProduct::TenYear)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false))
    }

    fn parse(
        &self,
        _: &RulePackContent,
        _: CgbFuturesProduct,
    ) -> ApplicationResult<FuturesDeliveryRule> {
        self.calls.parser.fetch_add(1, Ordering::SeqCst);
        Ok(delivery_rule())
    }
}

impl FuturesDeliveryEngine for Fixture {
    fn calculate(
        &self,
        input: &FuturesDeliverableInput,
    ) -> Result<FuturesDeliveryResult, AnalyticsError> {
        assert_eq!(input.financing_rate(), FixedDecimal::ZERO);
        let call = self.calls.delivery.fetch_add(1, Ordering::SeqCst);
        let is_base = call < 2;
        let is_first_bond = input.bond().version_ref().id() == &id('B');
        let implied_repo_rate = match (is_base, is_first_bond) {
            (true, true) => fixed(2),
            (true, false) => fixed(1),
            (false, true) => FixedDecimal::ZERO,
            (false, false) => fixed(3),
        };
        Ok(FuturesDeliveryResult::new(
            input.clone(),
            FuturesDeliveryMeasures::new(
                1,
                1,
                fixed(1),
                FixedDecimal::ZERO,
                FixedDecimal::ZERO,
                FixedDecimal::ZERO,
                fixed(100),
                fixed(100),
                FixedDecimal::ZERO,
                FixedDecimal::ZERO,
                FixedDecimal::ZERO,
                FixedDecimal::ZERO,
                implied_repo_rate,
                FixedDecimal::ZERO,
            )
            .map_err(|_| AnalyticsError::Internal)?,
        ))
    }
}

impl YieldCurveEngine for Fixture {
    fn interpolate(&self, query: &YieldCurveQuery) -> Result<YieldCurvePoint, AnalyticsError> {
        self.calls.curve.fetch_add(1, Ordering::SeqCst);
        let total = query
            .curve()
            .nodes()
            .iter()
            .take(2)
            .map(|node| node.yield_to_maturity().scaled())
            .try_fold(0_i128, i128::checked_add)
            .ok_or(AnalyticsError::NonFinite)?;
        YieldCurvePoint::new(query.clone(), FixedDecimal::from_scaled(total / 2))
            .map_err(|_| AnalyticsError::InvalidInput)
    }
}

impl BondAnalyticsEngine for Fixture {
    fn calculate(&self, input: &BondAnalyticsInput) -> Result<BondAnalyticsResult, AnalyticsError> {
        self.calls.bond.fetch_add(1, Ordering::SeqCst);
        let ytm = input.input_value();
        let clean = FixedDecimal::from_scaled(
            100 * FixedDecimal::ONE.scaled()
                - ytm
                    .scaled()
                    .checked_mul(100)
                    .ok_or(AnalyticsError::NonFinite)?,
        );
        let measures = AnalyticsMeasures::new(
            FixedDecimal::ZERO,
            clean,
            ytm,
            FixedDecimal::ONE,
            FixedDecimal::ONE,
            FixedDecimal::ONE,
            fixed(1),
        )
        .map_err(|_| AnalyticsError::InvalidInput)?;
        let cashflow = DerivedCashflow::new(
            1,
            input.terms().maturity_date(),
            input.terms().maturity_date(),
            FixedDecimal::ONE,
            fixed(100),
            fixed(101),
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

fn unit(code: &str, dimension: &str, suffix: char, scale: u32) -> Unit {
    Unit::new(UnitInput {
        unit_id: id(suffix),
        version: version(),
        owner: owner(),
        code: code.to_owned(),
        dimension: dimension.to_owned(),
        scale,
        precision: 28,
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
        effective: EffectivePeriod::new(time_for(2020, 1, 1, 0), time_for(2040, 1, 1, 0)).unwrap(),
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

fn curve_rule_pack() -> MarketRulePack {
    MarketRulePack::new(MarketRulePackInput {
        rule_pack_id: id('R'),
        version: version(),
        owner: owner(),
        market: "CN".to_owned(),
        rule_type: "bond-pricing".to_owned(),
        source: "fixture".to_owned(),
        effective: EffectivePeriod::new(time_for(2020, 1, 1, 0), time_for(2040, 1, 1, 0)).unwrap(),
        verification_status: VerificationStatus::Verified,
        content_hash: ContentHash::digest(b"r4d-b-curve-rule"),
    })
    .unwrap()
}

fn delivery_rule_pack() -> MarketRulePack {
    let content = RulePackContent::new(RULE_TYPE, b"r4d-b-delivery-v2".to_vec()).unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('X'),
            version: version(),
            owner: owner(),
            market: "CFFEX".to_owned(),
            rule_type: "cgb-futures".to_owned(),
            source: "fixture".to_owned(),
            effective: EffectivePeriod::new(time_for(2020, 1, 1, 0), time_for(2040, 1, 1, 0))
                .unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(content.value()),
        },
        content,
    )
    .unwrap()
}

fn delivery_rule() -> FuturesDeliveryRule {
    FuturesDeliveryRule::new(FuturesDeliveryRuleInput {
        original_term_max_months: 120,
        residual_min_months: 78,
        residual_max_months: None,
        delivery_months: vec![3, 6, 9, 12],
        nominal_coupon: fixed(3),
        face_quote_basis: fixed(100),
        accrued_interest_day_count: 1,
        conversion_factor_rounding_places: 4,
        accrued_interest_rounding_places: 7,
        annual_day_basis: 365,
    })
    .unwrap()
    .with_contract_size_in_quote_units(10_000)
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

fn bond(instrument: &Instrument) -> Bond {
    Bond::with_issuance(
        instrument,
        date(2026, 8, 3),
        date(2026, 8, 3),
        date(2036, 8, 3),
        decimal("100000000", 0, unit_ref('N')),
        BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt),
        decimal("100", 0, unit_ref('N')),
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

fn position(suffix: char, instrument: &Instrument, quantity: &str, unit: UnitRef) -> Position {
    let value = decimal("1", 0, unit_ref('C'));
    Position::new(PositionInput {
        position_id: id(suffix),
        instrument_ref: instrument.version_ref(),
        quantity: decimal(quantity, 0, unit),
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
        snapshot_id: id('Y'),
        owner: owner(),
        subject_ref: VersionRef::new(id('E'), version()),
        observed_at: time(1),
        visible_at: time(2),
        content_hash: ContentHash::digest(b"placeholder"),
        lineage: vec![lineage('G')],
        positions,
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    PositionSnapshot::new(input).unwrap()
}

fn curve_node(node_id: &str, tenor: &str) -> CurveNodeDefinition {
    let mut input = CurveNodeDefinitionInput {
        curve_node_id: node_id.to_owned(),
        curve_family_id: "cn.gov.yield-curve".to_owned(),
        tenor: tenor.to_owned(),
        factor_unit: unit_ref('V'),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = CurveNodeDefinition::content_hash_for(&input);
    CurveNodeDefinition::new(input).unwrap()
}

fn factor(factor_id: &str) -> FactorDefinition {
    let mut input = FactorDefinitionInput {
        factor_id: factor_id.to_owned(),
        factor_unit: unit_ref('V'),
        convention: SensitivityConvention::new(
            decimal("1", 4, unit_ref('V')),
            SensitivityDirection::Central,
            CurveRebuildPolicy::Rebuild,
            SecondOrderPolicy::Exclude,
        )
        .unwrap(),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = FactorDefinition::content_hash_for(&input);
    FactorDefinition::new(input).unwrap()
}

fn quote(suffix: char, bid: i128, ask: i128) -> CanonicalQuote {
    CanonicalQuote::new(
        VersionRef::new(id(suffix), version()),
        time(1),
        time(2),
        date(2026, 8, 3),
        Some(fixed(bid)),
        Some(fixed(ask)),
        unit_ref('P'),
    )
}

fn time(hour: u32) -> MarketTime {
    time_for(2026, 8, 3, hour)
}

fn time_for(year: i32, month: u32, day: u32, hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
            .single()
            .unwrap(),
        "Asia/Shanghai",
        date(year, month, day),
    )
    .unwrap()
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn decimal(value: &str, scale: u32, unit: UnitRef) -> DecimalValue {
    DecimalValue::new(value, scale, unit).unwrap()
}

fn fixed(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value * FixedDecimal::ONE.scaled())
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
