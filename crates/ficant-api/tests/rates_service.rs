use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use ficant_api::{
    FormalOutputPublisher, PlatformApplication, PlatformPort, RatesGrpcService, SessionPolicy,
    SystemClock, TrustedIdentity,
};
use ficant_application::ports::CouponTaxTreatment;
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, ApplicationResult, ArtifactRepository,
    BondAnalyticsArtifactCodec, BondAnalyticsArtifactFacts, BondAnalyticsEngine, CanonicalQuote,
    CanonicalSnapshotDecoder, CarryRollEngine, CurvePointSetDecoder, CurveSnapshotMetadata,
    CurveSnapshotMetadataRepository, DataSourceRepository, DecodedCanonicalQuotes,
    DecodedCurvePoint, DecodedCurvePointSet, DefinitionIdentity, DefinitionRepository,
    DefinitionValue, EncodedBondAnalyticsArtifact, EncodedFuturesDeliveryArtifact,
    FactorTopologyRepository, FormalOutputRecord, FormalOutputRepository,
    FuturesDeliveryArtifactCandidateFacts, FuturesDeliveryArtifactCodec,
    FuturesDeliveryArtifactFacts, FuturesDeliveryEngine, FuturesHedgeEngine, IdempotencyKey,
    InstrumentDefinition, InstrumentSubtype, IntegrityEvent, IntegrityEventSink, PublishArtifact,
    RegisterDataSource, RequiredVerifiedBlobRead, SafeTraceContext, SnapshotVerifiedReadMetadata,
    SnapshotVerifiedReadMetadataRepository, SubjectRepository, VerifiedBlobPayload,
    VerifiedBlobReader, VerifiedBlobRole, YieldCurveEngine,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, BOND_ANALYTICS_MEDIA_TYPE, BondRatesCommand,
    BondRatesMaterialization, DeliveryRatesCommand, DeliveryRatesMaterialization,
    FUTURES_DELIVERY_MEDIA_TYPE, ImmutableSnapshotBinding, MaterializeBondRatesInput,
    MaterializeDeliveryRatesInput, RatesUnitRequirement, rates_data_source_content_hash,
};
use ficant_cgb_futures_pack::CgbFuturesDeliveryRulePackParser;
use ficant_contracts::ficant::core::v1::{
    DecimalValue, ErrorCode, FundingTier as ProtoFundingTier, MarketTime as ProtoMarketTime,
    OwnerRef as ProtoOwnerRef, Sha256, Ulid as ProtoUlid, UnitRef as ProtoUnitRef,
    VersionRef as ProtoVersionRef,
};
use ficant_contracts::ficant::market::v1::{
    CgbFuturesDeliveryRulePack, CgbFuturesProductRule, FundingRulePack, FundingTierRate,
    cgb_futures_product_rule::ResidualUpperBound,
};
use ficant_contracts::ficant::rates::v1::{
    AlgorithmBinding, AnalysisContext, AnalysisInputRole, AnalysisUnits, AnalyzeBondRequest,
    AnalyzeCarryRollRequest, AnalyzeFuturesDeliveryRequest, AnalyzeFuturesHedgeRequest,
    ArtifactBinding, CalendarRequirement as ProtoCalendarRequirement, InterpolateYieldCurveRequest,
    ObjectBinding, ResultMetadata, SnapshotBinding, analyze_bond_request, analyze_bond_response,
    analyze_carry_roll_response, analyze_futures_delivery_response, analyze_futures_hedge_response,
    interpolate_yield_curve_response, rates_analytics_service_server::RatesAnalyticsService,
};
use ficant_domain::analytics::{
    ABI_VERSION, ALGORITHM_ID, ALGORITHM_VERSION, AnalyticsError, AnalyticsMode,
    AnalyticsObjectRef, BondAnalyticsInput, BondAnalyticsResult, BondTerms, BusinessDayConvention,
    CONVENTION_PROFILE, CalendarRequirement, CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::curves::{
    CARRY_ROLL_ALGORITHM_ID, CARRY_ROLL_ALGORITHM_VERSION, CARRY_ROLL_CONVENTION_PROFILE,
    CURVE_ALGORITHM_ID, CURVE_ALGORITHM_VERSION, CURVE_CONVENTION_PROFILE, CarryRollInput,
    CarryRollResult, YieldCurvePoint, YieldCurveQuery,
};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FUTURES_DELIVERY_ALGORITHM_ID, FUTURES_DELIVERY_ALGORITHM_VERSION,
    FUTURES_DELIVERY_CONVENTION_PROFILE, FuturesDeliverableInput, FuturesDeliveryBasketResult,
    FuturesDeliveryResult,
};
use ficant_domain::futures_hedge::{
    FUTURES_HEDGE_ALGORITHM_ID, FUTURES_HEDGE_ALGORITHM_VERSION, FUTURES_HEDGE_CONVENTION_PROFILE,
    FuturesHedgeInput, FuturesHedgeResult,
};
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::{
    ArtifactInputKind, Bond, BondBusinessDayConvention, BondCouponFrequency,
    BondDayCountConvention, BondPricingTerms, BondTaxAttributes, Calendar, CalendarInput,
    CalendarSession, CurveSnapshot, CurveSnapshotInput, DataSource, DataSourceInput,
    DataSourceKind, FuturesContract, IncomeTaxStatus, Instrument, InstrumentInput, InstrumentKind,
    MarketRulePack, MarketRulePackInput, PriceSourceType, RulePackContent, Unit, UnitInput,
    ValueAddedTaxStatus, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue as DomainDecimalValue, EffectivePeriod, LineageRef, MarketTime,
    OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    Artifact, ArtifactKind, CurveNodeDefinition, CurveNodeDefinitionInput, DataSnapshot,
    DataSnapshotInput, FactorDefinition, FactorTarget, FactorTargetBinding,
};
use ficant_domain::subject::{
    AccessSet, FundingTier, Subject, SubjectRecord, SubjectStateSnapshot, SubjectVersion,
    TaxTreatment,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};
use ficant_fixed_income_native::{
    NativeBondAnalyticsEngine, NativeCarryRollEngine, NativeFuturesDeliveryEngine,
    NativeFuturesHedgeEngine, NativeYieldCurveEngine,
};
use ficant_funding_pack::{
    FundingRulePackV1Parser, MARKET as FUNDING_MARKET, RULE_TYPE as FUNDING_RULE_TYPE,
    TYPE_URL as FUNDING_TYPE_URL,
};
use ficant_runtime::{CodeBinding, RuntimeBinding};
use ficant_tax_pack::{
    MARKET as TAX_MARKET, RULE_TYPE as TAX_RULE_TYPE, SOURCE as TAX_SOURCE,
    TYPE_URL_V2 as TAX_TYPE_URL, TaxRulePackV2Parser,
};
use prost::Message;
use rust_decimal::Decimal as ExactDecimal;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";
const DATA_BYTES: &[u8] = b"quotes";
const MANIFEST_BYTES: &[u8] = b"manifest";
const CURVE_BYTES: &[u8] = b"curve-points";
const TARGET_BYTES: &[u8] = b"target-risk";
const DELIVERY_BYTES: &[u8] = b"delivery-basket";
const CTD_BYTES: &[u8] = b"ctd-risk";
const CURVE_FAMILY: &str = "cn.gov.yield-curve";
const R5E_ORACLE_INPUTS: &str =
    include_str!("../../../tests/golden-cases/china-rates/r5e-tax-adjusted-analytics-inputs.json");
const R5E_ORACLE_EXPECTED: &str = include_str!(
    "../../../tests/golden-cases/china-rates/expected/r5e-tax-adjusted-analytics-expected.json"
);

#[derive(Clone, Default)]
struct Calls {
    bond: Arc<AtomicUsize>,
    curve: Arc<AtomicUsize>,
    carry: Arc<AtomicUsize>,
    delivery: Arc<AtomicUsize>,
    hedge: Arc<AtomicUsize>,
}

impl Calls {
    fn total(&self) -> usize {
        [
            &self.bond,
            &self.curve,
            &self.carry,
            &self.delivery,
            &self.hedge,
        ]
        .into_iter()
        .map(|value| value.load(Ordering::SeqCst))
        .sum()
    }
}

#[derive(Clone)]
struct RecordingEngines {
    calls: Calls,
}

impl BondAnalyticsEngine for RecordingEngines {
    fn calculate(&self, input: &BondAnalyticsInput) -> Result<BondAnalyticsResult, AnalyticsError> {
        self.calls.bond.fetch_add(1, Ordering::SeqCst);
        NativeBondAnalyticsEngine.calculate(input)
    }
}

impl YieldCurveEngine for RecordingEngines {
    fn interpolate(&self, input: &YieldCurveQuery) -> Result<YieldCurvePoint, AnalyticsError> {
        self.calls.curve.fetch_add(1, Ordering::SeqCst);
        NativeYieldCurveEngine.interpolate(input)
    }
}

impl CarryRollEngine for RecordingEngines {
    fn calculate(&self, input: &CarryRollInput) -> Result<CarryRollResult, AnalyticsError> {
        self.calls.carry.fetch_add(1, Ordering::SeqCst);
        NativeCarryRollEngine.calculate(input)
    }
}

impl FuturesDeliveryEngine for RecordingEngines {
    fn calculate(
        &self,
        input: &FuturesDeliverableInput,
    ) -> Result<FuturesDeliveryResult, AnalyticsError> {
        self.calls.delivery.fetch_add(1, Ordering::SeqCst);
        NativeFuturesDeliveryEngine.calculate(input)
    }
}

#[derive(Clone)]
struct R5eOracleDeliveryEngine {
    calls: Arc<AtomicUsize>,
}

impl FuturesDeliveryEngine for R5eOracleDeliveryEngine {
    fn calculate(
        &self,
        input: &FuturesDeliverableInput,
    ) -> Result<FuturesDeliveryResult, AnalyticsError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let bond_id = input.bond().version_ref().id();
        let oracle_inputs = R5E_ORACLE_INPUTS;
        let candidate = if bond_id == &id('B') {
            oracle_candidate(oracle_inputs, "market-subject-ctd-reversal", "CGB-EXEMPT")
        } else if bond_id == &id('D') {
            oracle_candidate(oracle_inputs, "market-subject-ctd-reversal", "CGB-TAXABLE")
        } else {
            return Err(AnalyticsError::InvalidInput);
        };
        let expected = R5E_ORACLE_EXPECTED;
        let expected = oracle_candidate(
            expected,
            "market-subject-ctd-reversal",
            oracle_string(candidate, "bond_id"),
        );
        let gross_coupon = oracle_fixed(candidate, "gross_interim_coupons");
        let market_irr = oracle_fixed(expected, "market_pre_tax_irr");
        let net_basis = oracle_fixed(candidate, "market_net_basis");
        let invoice_price = oracle_fixed(candidate, "invoice_price");
        let purchase_dirty_price = oracle_fixed(candidate, "purchase_dirty_price");
        let delivery_profit = FixedDecimal::ZERO
            .checked_sub(net_basis)
            .map_err(|_| AnalyticsError::InvalidInput)?;
        let measures = ficant_domain::futures_delivery::FuturesDeliveryMeasures::new(
            1,
            1,
            FixedDecimal::ONE,
            FixedDecimal::ZERO,
            FixedDecimal::ZERO,
            gross_coupon,
            invoice_price,
            purchase_dirty_price,
            net_basis,
            FixedDecimal::ZERO,
            FixedDecimal::ZERO,
            net_basis,
            market_irr,
            delivery_profit,
        )
        .map_err(|_| AnalyticsError::InvalidInput)?;
        Ok(FuturesDeliveryResult::new(input.clone(), measures))
    }
}

impl FuturesHedgeEngine for RecordingEngines {
    fn calculate(&self, input: &FuturesHedgeInput) -> Result<FuturesHedgeResult, AnalyticsError> {
        self.calls.hedge.fetch_add(1, Ordering::SeqCst);
        NativeFuturesHedgeEngine.calculate(input)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drift {
    None,
    AllExempt,
    DataOwner,
    CurveVisibleAfterKnowledge,
    TaxEffectiveAfterValuation,
    TaxSource,
    TaxVerification,
    TaxPayload,
    RateUnitDefinition,
    SubjectTaxProfile,
    CandidateTaxAttributes,
    CorruptDataContent,
    ReverseQuoteOrder,
}

struct FixturePorts {
    definitions: Vec<DefinitionValue>,
    subjects: Vec<SubjectRecord>,
    data: DataSnapshot,
    curve: CurveSnapshot,
    points: DecodedCurvePointSet,
    nodes: Vec<CurveNodeDefinition>,
    source: DataSource,
    artifacts: Vec<(Artifact, Vec<u8>)>,
    target_facts: BondAnalyticsArtifactFacts,
    delivery_facts: FuturesDeliveryArtifactFacts,
    ctd_facts: BondAnalyticsArtifactFacts,
    corrupt_data: bool,
    reverse_quotes: bool,
    integrity_events: AtomicUsize,
    tax_reads: AtomicUsize,
    formal_outputs: Mutex<Vec<FormalOutputRecord>>,
}

impl FixturePorts {
    fn definition(&self, suffix: char) -> &DefinitionValue {
        self.definitions
            .iter()
            .find(|value| value.identity() == id(suffix).as_str())
            .expect("fixture definition exists")
    }

    fn binding(&self, suffix: char) -> ObjectBinding {
        let value = self.definition(suffix);
        object_binding(suffix, value.version(), &definition_content_hash(value))
    }

    fn artifact_binding(&self, suffix: char) -> ArtifactBinding {
        let (artifact, _) = self
            .artifacts
            .iter()
            .find(|(artifact, _)| artifact.id() == &id(suffix))
            .expect("fixture Artifact exists");
        ArtifactBinding {
            artifact_id: Some(proto_ulid(suffix)),
            content_hash: Some(proto_hash(artifact.content_hash())),
        }
    }
}

#[tonic::async_trait]
impl DefinitionRepository for FixturePorts {
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
        if definition_id == id('T') {
            self.tax_reads.fetch_add(1, Ordering::SeqCst);
        }
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

#[tonic::async_trait]
impl SubjectRepository for FixturePorts {
    async fn register_subject(&self, _: SubjectRecord) -> ApplicationResult<SubjectRecord> {
        Err(unavailable())
    }

    async fn get_subject(&self, reference: VersionRef) -> ApplicationResult<Option<SubjectRecord>> {
        Ok(self
            .subjects
            .iter()
            .find(|value| value.version().reference() == &reference)
            .cloned())
    }

    async fn register_subject_state(
        &self,
        _: SubjectStateSnapshot,
    ) -> ApplicationResult<SubjectStateSnapshot> {
        Err(unavailable())
    }

    async fn get_subject_state(
        &self,
        _: Ulid,
        _: chrono::DateTime<Utc>,
    ) -> ApplicationResult<Option<SubjectStateSnapshot>> {
        Err(unavailable())
    }
}

#[tonic::async_trait]
impl SnapshotVerifiedReadMetadataRepository for FixturePorts {
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
            DATA_BYTES.len() as u64,
            MANIFEST_BYTES.len() as u64,
        )
        .map(Some)
    }
}

#[tonic::async_trait]
impl CurveSnapshotMetadataRepository for FixturePorts {
    async fn get_curve_snapshot_metadata(
        &self,
        _: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> ApplicationResult<Option<CurveSnapshotMetadata>> {
        Ok((curve_snapshot_id == *self.curve.id()).then(|| {
            CurveSnapshotMetadata::new(self.curve.clone(), CURVE_BYTES.len() as u64)
                .expect("fixture CurveSnapshot metadata is valid")
        }))
    }
}

#[tonic::async_trait]
impl VerifiedBlobReader for FixturePorts {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<VerifiedBlobPayload> {
        let bytes = match request.blob_role() {
            VerifiedBlobRole::DataParquet => {
                if self.corrupt_data {
                    b"broken".as_slice()
                } else {
                    DATA_BYTES
                }
            }
            VerifiedBlobRole::DataManifest => MANIFEST_BYTES,
            VerifiedBlobRole::CurvePoints => CURVE_BYTES,
            VerifiedBlobRole::ArtifactPayload => self
                .artifacts
                .iter()
                .find(|(artifact, _)| artifact.id() == request.resource_id())
                .map(|(_, bytes)| bytes.as_slice())
                .expect("fixture Artifact payload exists"),
            _ => unreachable!("Rates reads only data, curve and Artifact payload roles"),
        };
        request.verify_bytes(sink, bytes.to_vec()).await
    }
}

#[tonic::async_trait]
impl IntegrityEventSink for FixturePorts {
    async fn emit(&self, _: IntegrityEvent) -> ApplicationResult<()> {
        self.integrity_events.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tonic::async_trait]
impl CanonicalSnapshotDecoder for FixturePorts {
    async fn decode_quotes(
        &self,
        snapshot: &DataSnapshot,
        parquet: &[u8],
        manifest: &[u8],
    ) -> ApplicationResult<DecodedCanonicalQuotes> {
        assert_eq!(snapshot, &self.data);
        assert_eq!(parquet, DATA_BYTES);
        assert_eq!(manifest, MANIFEST_BYTES);
        let mut quotes = vec![
            quote('B', "10127", 2),
            quote('D', "10125", 2),
            quote('C', "995", 1),
        ];
        if self.reverse_quotes {
            quotes.reverse();
        }
        DecodedCanonicalQuotes::new(VersionRef::new(id('Q'), version(1)), quotes)
    }
}

impl CurvePointSetDecoder for FixturePorts {
    fn decode_canonical(&self, bytes: &[u8]) -> ApplicationResult<DecodedCurvePointSet> {
        assert_eq!(bytes, CURVE_BYTES);
        Ok(self.points.clone())
    }
}

#[tonic::async_trait]
impl DataSourceRepository for FixturePorts {
    async fn register(&self, _: RegisterDataSource) -> Result<DataSource, ApplicationError> {
        Err(unavailable())
    }

    async fn get_exact(
        &self,
        _: &AccessScope,
        reference: VersionRef,
    ) -> Result<Option<DataSource>, ApplicationError> {
        Ok((self.source.id() == reference.id()
            && self.source.version() == reference.version().get())
        .then(|| self.source.clone()))
    }
}

#[tonic::async_trait]
impl FactorTopologyRepository for FixturePorts {
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

    async fn get_factor_definition(&self, _: &str) -> ApplicationResult<Option<FactorDefinition>> {
        Ok(None)
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
        _: &FactorTarget,
    ) -> ApplicationResult<Vec<FactorDefinition>> {
        Err(unavailable())
    }

    async fn exact_target_exists(&self, _: &FactorTarget) -> ApplicationResult<bool> {
        Ok(false)
    }
}

#[tonic::async_trait]
impl ArtifactRepository for FixturePorts {
    async fn publish_verified_blob(&self, _: PublishArtifact) -> ApplicationResult<Artifact> {
        Err(unavailable())
    }

    async fn get_metadata(
        &self,
        _: &AccessScope,
        artifact_id: Ulid,
    ) -> ApplicationResult<Option<Artifact>> {
        Ok(self
            .artifacts
            .iter()
            .find(|(artifact, _)| artifact.id() == &artifact_id)
            .map(|(artifact, _)| artifact.clone()))
    }
}

#[tonic::async_trait]
impl FormalOutputRepository for FixturePorts {
    async fn publish(
        &self,
        _: &AccessScope,
        record: FormalOutputRecord,
    ) -> ApplicationResult<FormalOutputRecord> {
        let mut outputs = self.formal_outputs.lock().expect("formal output lock");
        if let Some(existing) = outputs
            .iter()
            .find(|existing| existing.output_identity() == record.output_identity())
        {
            return (existing == &record)
                .then(|| existing.clone())
                .ok_or_else(|| {
                    ApplicationError::new(ApplicationErrorCategory::ImmutableViolation, false)
                });
        }
        outputs.push(record.clone());
        Ok(record)
    }

    async fn get(
        &self,
        _: &AccessScope,
        output_identity: &ContentHash,
    ) -> ApplicationResult<Option<FormalOutputRecord>> {
        Ok(self
            .formal_outputs
            .lock()
            .expect("formal output lock")
            .iter()
            .find(|record| record.output_identity() == output_identity)
            .cloned())
    }
}

impl BondAnalyticsArtifactCodec for FixturePorts {
    fn encode(
        &self,
        _: &BondAnalyticsResult,
    ) -> Result<EncodedBondAnalyticsArtifact, AnalyticsError> {
        Err(AnalyticsError::Internal)
    }

    fn decode(
        &self,
        _: &[u8],
        _: &BondAnalyticsInput,
    ) -> Result<BondAnalyticsResult, AnalyticsError> {
        Err(AnalyticsError::Internal)
    }

    fn decode_facts(&self, bytes: &[u8]) -> Result<BondAnalyticsArtifactFacts, AnalyticsError> {
        match bytes {
            TARGET_BYTES => Ok(self.target_facts.clone()),
            CTD_BYTES => Ok(self.ctd_facts.clone()),
            _ => Err(AnalyticsError::InvalidInput),
        }
    }
}

impl FuturesDeliveryArtifactCodec for FixturePorts {
    fn encode(
        &self,
        _: &FuturesDeliveryBasketResult,
    ) -> Result<EncodedFuturesDeliveryArtifact, AnalyticsError> {
        Err(AnalyticsError::Internal)
    }

    fn encode_self_describing(
        &self,
        _: &FuturesDeliveryBasketResult,
    ) -> Result<EncodedFuturesDeliveryArtifact, AnalyticsError> {
        Err(AnalyticsError::Internal)
    }

    fn decode(
        &self,
        _: &[u8],
        _: &[FuturesDeliverableInput],
    ) -> Result<FuturesDeliveryBasketResult, AnalyticsError> {
        Err(AnalyticsError::Internal)
    }

    fn decode_facts(&self, bytes: &[u8]) -> Result<FuturesDeliveryArtifactFacts, AnalyticsError> {
        if bytes == DELIVERY_BYTES {
            Ok(self.delivery_facts.clone())
        } else {
            Err(AnalyticsError::InvalidInput)
        }
    }
}

struct Fixture {
    service: RatesGrpcService,
    ports: Arc<FixturePorts>,
    calls: Calls,
}

impl Fixture {
    #[allow(clippy::too_many_lines)]
    fn new(drift: Drift) -> Self {
        Self::build(drift, false)
    }

    #[allow(clippy::too_many_lines)]
    fn with_oracle_delivery(drift: Drift) -> Self {
        Self::build(drift, true)
    }

    #[allow(clippy::too_many_lines)]
    fn build(drift: Drift, oracle_delivery: bool) -> Self {
        let units = unit_definitions(drift == Drift::RateUnitDefinition);
        let calendar = calendar();
        let curve_rule = curve_rule_pack();
        let delivery_rule = delivery_rule_pack();
        let tax_rule = tax_rule_pack(drift);
        let funding_rule = funding_rule_pack();
        let bond = bond_definition(
            drift == Drift::AllExempt,
            drift == Drift::CandidateTaxAttributes,
        );
        let exempt_bond = exempt_bond_definition();
        let contract = futures_contract_definition();
        let definitions = units
            .into_iter()
            .chain([
                DefinitionValue::Calendar(calendar),
                DefinitionValue::MarketRulePack(curve_rule),
                DefinitionValue::MarketRulePack(delivery_rule.clone()),
                DefinitionValue::MarketRulePack(tax_rule.clone()),
                DefinitionValue::MarketRulePack(funding_rule),
                exempt_bond,
                bond.clone(),
                contract.clone(),
            ])
            .collect::<Vec<_>>();
        let bond_ref = analytics_ref(&bond);
        let contract_ref = analytics_ref(&contract);
        let delivery_rule_ref = AnalyticsObjectRef::new(
            VersionRef::new(id('R'), version(1)),
            delivery_rule.content_hash().clone(),
        );
        let tax_rule_ref = AnalyticsObjectRef::new(
            VersionRef::new(id('T'), version(1)),
            tax_rule.content_hash().clone(),
        );
        let snapshot_ref = AnalyticsObjectRef::new(
            VersionRef::new(id('E'), version(1)),
            ContentHash::digest(DATA_BYTES),
        );
        let source = DataSource::new(DataSourceInput {
            data_source_id: id('Q'),
            version: version(1),
            owner: owner(),
            kind: DataSourceKind::FileNdjson,
            name: "Rates exact quote source".to_owned(),
            connection_binding: "rates-fixture".to_owned(),
            dataset: "rates_quotes".to_owned(),
            canonical_schema_id: "ficant.market.quote.canonical.v1".to_owned(),
            canonical_schema_hash: ContentHash::digest(b"canonical-schema"),
        })
        .expect("fixture DataSource is valid")
        .with_price_source_type(PriceSourceType::ActiveQuote)
        .expect("fixture DataSource price role is valid");
        let data_owner = if drift == Drift::DataOwner {
            OwnerRef::new(owner().tenant_id().clone(), id('Z'))
        } else {
            owner()
        };
        let data = DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: id('E'),
            owner: data_owner,
            visible_at: time(20, 6),
            as_of: time(20, 4),
            schema_hash: ContentHash::digest(b"canonical-schema"),
            manifest_hash: ContentHash::digest(MANIFEST_BYTES),
            blob_content_hash: ContentHash::digest(DATA_BYTES),
            lineage: vec![
                LineageRef::new(
                    id('Q'),
                    Some(version(1)),
                    Some(rates_data_source_content_hash(&source)),
                )
                .expect("fixture DataSnapshot lineage is valid"),
            ],
        })
        .expect("fixture DataSnapshot is valid");
        let nodes = vec![
            curve_node("cn.gov.yield-curve.06y", "P6Y"),
            curve_node("cn.gov.yield-curve.10y", "P10Y"),
        ];
        let points = DecodedCurvePointSet::new(
            CURVE_FAMILY,
            nodes
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    DecodedCurvePoint::new(
                        node.curve_node_id(),
                        node.content_hash().clone(),
                        domain_decimal(&(25 + index * 5).to_string(), 3, 'Z'),
                    )
                    .expect("fixture curve point is valid")
                })
                .collect(),
        )
        .expect("fixture curve set is valid");
        let curve_visible_hour = if drift == Drift::CurveVisibleAfterKnowledge {
            9
        } else {
            6
        };
        let curve = CurveSnapshot::new(CurveSnapshotInput {
            curve_snapshot_id: id('L'),
            owner: owner(),
            as_of: time(20, 4),
            currency: unit_ref('M'),
            curve_kind: "YTM".to_owned(),
            calendar: VersionRef::new(id('K'), version(1)),
            rule_pack: VersionRef::new(id('U'), version(1)),
            point_schema: ficant_application::ports::CURVE_POINT_SCHEMA.to_owned(),
            content_hash: ContentHash::digest(CURVE_BYTES),
            lineage: vec![
                LineageRef::new(
                    id('Q'),
                    Some(version(1)),
                    Some(rates_data_source_content_hash(&source)),
                )
                .expect("fixture curve lineage is valid"),
                LineageRef::content_addressed(id('E'), ContentHash::digest(DATA_BYTES)),
            ],
            input_kind: ArtifactInputKind::ExternalFixture,
        })
        .expect("fixture CurveSnapshot is valid")
        .with_knowledge_time(time(20, curve_visible_hour), CURVE_FAMILY)
        .expect("fixture CurveSnapshot knowledge time is valid");
        let target_facts = BondAnalyticsArtifactFacts::new(
            time(20, 4),
            bond_ref.clone(),
            tax_rule_ref,
            snapshot_ref.clone(),
            fixed("-1000", 0),
        );
        let ctd_facts = BondAnalyticsArtifactFacts::new(
            time(20, 4),
            bond_ref.clone(),
            delivery_rule_ref.clone(),
            snapshot_ref.clone(),
            fixed("8", 2),
        );
        let delivery_facts = FuturesDeliveryArtifactFacts::new(
            time(20, 4),
            contract_ref.clone(),
            delivery_rule_ref,
            snapshot_ref,
            CgbFuturesProduct::TenYear,
            vec![FuturesDeliveryArtifactCandidateFacts::new(
                bond_ref.clone(),
                fixed("9", 1),
            )],
            0,
        );
        let artifacts = artifact_fixtures(
            &bond_ref,
            &contract_ref,
            &target_facts,
            &delivery_facts,
            &ctd_facts,
        );
        let ports = Arc::new(FixturePorts {
            definitions,
            subjects: vec![fixture_subject(drift == Drift::SubjectTaxProfile)],
            data,
            curve,
            points,
            nodes,
            source,
            artifacts,
            target_facts,
            delivery_facts,
            ctd_facts,
            corrupt_data: drift == Drift::CorruptDataContent,
            reverse_quotes: drift == Drift::ReverseQuoteOrder,
            integrity_events: AtomicUsize::new(0),
            tax_reads: AtomicUsize::new(0),
            formal_outputs: Mutex::new(Vec::new()),
        });
        let calls = Calls::default();
        let engines = Arc::new(RecordingEngines {
            calls: calls.clone(),
        });
        let futures_delivery: Arc<dyn FuturesDeliveryEngine> = if oracle_delivery {
            Arc::new(R5eOracleDeliveryEngine {
                calls: calls.delivery.clone(),
            })
        } else {
            engines.clone()
        };
        let identity = TrustedIdentity::implicit(
            "rates-r5d-test",
            id('A'),
            id('0'),
            vec![id('1')],
            PlatformRole::Researcher,
            ["rates:analyze"],
        )
        .expect("test identity is valid");
        let application: Arc<dyn PlatformPort> = Arc::new(
            PlatformApplication::try_new(
                Arc::new(SystemClock),
                SessionPolicy::new(900, 60).expect("test session policy is valid"),
                KEY,
                Vec::new(),
                Some(identity),
                Vec::new(),
            )
            .expect("test application is valid"),
        );
        let service = RatesGrpcService::new_with_formal_materialization(
            application,
            engines.clone(),
            engines.clone(),
            engines.clone(),
            futures_delivery,
            ports.clone(),
            ports.clone(),
            Arc::new(CgbFuturesDeliveryRulePackParser),
            ports.clone(),
            ports.clone(),
            ports.clone(),
            ports.clone(),
            Arc::new(FundingRulePackV1Parser),
            Arc::new(TaxRulePackV2Parser),
            engines,
            ports.clone(),
            ports.clone(),
            ports.clone(),
            ports.clone(),
            ports.clone(),
            ports.clone(),
            ports.clone(),
            formal_publisher(ports.clone()),
            KEY,
        )
        .expect("R5D Rates service is valid");
        Self {
            service,
            ports,
            calls,
        }
    }
}

#[tokio::test]
async fn authorization_fails_before_contract_parsing_or_any_engine() {
    for (role, scopes, case) in [
        (PlatformRole::Researcher, ["rates:read"], "missing scope"),
        (
            PlatformRole::PlatformAdmin,
            ["rates:analyze"],
            "wrong active role",
        ),
    ] {
        let fixture = Fixture::new(Drift::None);
        let (service, calls) = rates_service_with_identity(&fixture, role, scopes);
        let response = service
            .analyze_bond(Request::new(AnalyzeBondRequest::default()))
            .await
            .expect("business error is transported")
            .into_inner();
        let Some(analyze_bond_response::Result::Error(error)) = response.result else {
            panic!("{case} must be rejected");
        };
        assert_eq!(error.code, ErrorCode::Forbidden as i32, "{case}");
        assert_eq!(calls.total(), 0, "{case} reached a numerical engine");
    }
}

fn rates_service_with_identity<const N: usize>(
    fixture: &Fixture,
    role: PlatformRole,
    scopes: [&str; N],
) -> (RatesGrpcService, Calls) {
    let identity = TrustedIdentity::implicit(
        "rates-authorization-negative",
        id('A'),
        id('0'),
        vec![id('1')],
        role,
        scopes,
    )
    .expect("test identity is valid");
    let application: Arc<dyn PlatformPort> = Arc::new(
        PlatformApplication::try_new(
            Arc::new(SystemClock),
            SessionPolicy::new(900, 60).expect("test session policy is valid"),
            KEY,
            Vec::new(),
            Some(identity),
            Vec::new(),
        )
        .expect("test application is valid"),
    );
    let calls = Calls::default();
    let engines = Arc::new(RecordingEngines {
        calls: calls.clone(),
    });
    let service = RatesGrpcService::new_with_formal_materialization(
        application,
        engines.clone(),
        engines.clone(),
        engines.clone(),
        engines.clone(),
        fixture.ports.clone(),
        fixture.ports.clone(),
        Arc::new(CgbFuturesDeliveryRulePackParser),
        fixture.ports.clone(),
        fixture.ports.clone(),
        fixture.ports.clone(),
        fixture.ports.clone(),
        Arc::new(FundingRulePackV1Parser),
        Arc::new(TaxRulePackV2Parser),
        engines,
        fixture.ports.clone(),
        fixture.ports.clone(),
        fixture.ports.clone(),
        fixture.ports.clone(),
        fixture.ports.clone(),
        fixture.ports.clone(),
        fixture.ports.clone(),
        formal_publisher(fixture.ports.clone()),
        KEY,
    )
    .expect("R5D Rates service is valid");
    (service, calls)
}

fn formal_publisher(repository: Arc<FixturePorts>) -> FormalOutputPublisher {
    FormalOutputPublisher::new(
        repository,
        CodeBinding::new(
            "34402344c7d2c9238dc171af52ac4db77eb6b462",
            "f66e03c55703837d6f2aee9959eba482612272f1",
        )
        .expect("test code binding"),
        RuntimeBinding::new(
            ContentHash::digest(b"rates-test-image"),
            ContentHash::digest(b"rates-test-environment"),
        ),
    )
}

#[tokio::test]
async fn all_five_rpcs_require_one_resolved_exact_subject_before_engines() {
    let fixture = Fixture::new(Drift::None);
    let missing_subject = ProtoVersionRef {
        id: Some(proto_ulid('Z')),
        version: 1,
    };

    let mut bond = bond_request(&fixture.ports);
    bond.context.as_mut().expect("context").subject_ref = Some(missing_subject.clone());
    assert_bond_error(
        fixture
            .service
            .analyze_bond(Request::new(bond))
            .await
            .expect("business error is transported")
            .into_inner(),
    );

    let mut curve = curve_request();
    curve.context.as_mut().expect("context").subject_ref = Some(missing_subject.clone());
    assert_curve_error(
        fixture
            .service
            .interpolate_yield_curve(Request::new(curve))
            .await
            .expect("business error is transported")
            .into_inner(),
    );

    let mut carry = carry_request(&fixture.ports);
    carry.context.as_mut().expect("context").subject_ref = Some(missing_subject.clone());
    assert_carry_error(
        fixture
            .service
            .analyze_carry_roll(Request::new(carry))
            .await
            .expect("business error is transported")
            .into_inner(),
    );

    let mut delivery = delivery_request(&fixture.ports);
    delivery.context.as_mut().expect("context").subject_ref = Some(missing_subject.clone());
    assert_delivery_error(
        fixture
            .service
            .analyze_futures_delivery(Request::new(delivery))
            .await
            .expect("business error is transported")
            .into_inner(),
    );

    let mut hedge = hedge_request(&fixture.ports);
    hedge.context.as_mut().expect("context").subject_ref = Some(missing_subject);
    assert_hedge_error(
        fixture
            .service
            .analyze_futures_hedge(Request::new(hedge))
            .await
            .expect("business error is transported")
            .into_inner(),
    );

    assert_eq!(fixture.calls.total(), 0);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn all_five_rpcs_return_stable_complete_consumed_input_evidence() {
    let fixture = Fixture::new(Drift::None);

    let bond_request = bond_request(&fixture.ports);
    let first_bond = fixture
        .service
        .analyze_bond(Request::new(bond_request.clone()))
        .await
        .expect("Bond response is transported")
        .into_inner();
    let second_bond = fixture
        .service
        .analyze_bond(Request::new(bond_request.clone()))
        .await
        .expect("Bond response is transported")
        .into_inner();
    assert_eq!(
        first_bond.encode_to_vec(),
        second_bond.encode_to_vec(),
        "identical verified Bond inputs must produce byte-identical responses",
    );
    let first_bond = bond_analysis(first_bond);
    let second_bond = bond_analysis(second_bond);
    let after_tax = first_bond.after_tax.as_ref().expect("R5E after-tax view");
    let oracle_expected = R5E_ORACLE_EXPECTED;
    let oracle_bond = oracle_bond_case(oracle_expected, "cutoff-day-taxable");
    assert_eq!(after_tax.claim_scope, 1);
    assert_eq!(after_tax.cashflows.len(), first_bond.cashflows.len());
    for (market, subject) in first_bond.cashflows.iter().zip(&after_tax.cashflows) {
        assert_eq!(market.sequence, subject.sequence);
        assert_eq!(market.nominal_date, subject.nominal_date);
        assert_eq!(market.payment_date, subject.payment_date);
        assert_eq!(market.principal, subject.principal);
        assert_eq!(
            proto_decimal_value(subject.coupon.as_ref().expect("subject coupon")),
            ExactDecimal::from_i128_with_scale(1_179_245_283_019, 12),
            "each gross 1.25 coupon is independently divided by 1.06 with ties-to-even",
        );
    }
    assert_eq!(
        proto_decimal_value(
            first_bond
                .measures
                .as_ref()
                .expect("market Bond measures")
                .yield_to_maturity
                .as_ref()
                .expect("market yield to maturity"),
        ),
        oracle_decimal(oracle_string(
            oracle_bond,
            "market_pre_tax_yield_to_maturity",
        )),
        "production pre-tax YTM must match the independent R5E Decimal Oracle",
    );
    assert_eq!(
        proto_decimal_value(
            after_tax
                .yield_to_maturity
                .as_ref()
                .expect("subject yield to maturity"),
        ),
        oracle_decimal(oracle_string(
            oracle_bond,
            "subject_tax_adjusted_yield_to_maturity",
        )),
        "production subject YTM must match the independent R5E Decimal Oracle",
    );
    assert_eq!(first_bond.metadata, second_bond.metadata);
    assert_metadata(
        first_bond.metadata.as_ref().expect("Bond metadata"),
        &roles(&[
            (AnalysisInputRole::Subject, 1),
            (AnalysisInputRole::Unit, 9),
            (AnalysisInputRole::Bond, 1),
            (AnalysisInputRole::Calendar, 1),
            (AnalysisInputRole::DataSnapshot, 1),
            (AnalysisInputRole::TaxRulePack, 1),
        ]),
        ALGORITHM_ID,
    );
    assert_eq!(fixture.calls.bond.load(Ordering::SeqCst), 4);

    let curve_request = curve_request();
    let first_curve = fixture
        .service
        .interpolate_yield_curve(Request::new(curve_request.clone()))
        .await
        .expect("curve response is transported")
        .into_inner();
    let second_curve = fixture
        .service
        .interpolate_yield_curve(Request::new(curve_request))
        .await
        .expect("curve response is transported")
        .into_inner();
    assert_eq!(
        first_curve.encode_to_vec(),
        second_curve.encode_to_vec(),
        "identical verified Curve inputs must produce byte-identical responses",
    );
    let first_curve = curve_point(first_curve);
    let second_curve = curve_point(second_curve);
    assert_eq!(first_curve.metadata, second_curve.metadata);
    assert_metadata(
        first_curve.metadata.as_ref().expect("curve metadata"),
        &roles(&[
            (AnalysisInputRole::Subject, 1),
            (AnalysisInputRole::Unit, 9),
            (AnalysisInputRole::Calendar, 1),
            (AnalysisInputRole::CurveSnapshot, 1),
            (AnalysisInputRole::DataSnapshot, 1),
            (AnalysisInputRole::DataSource, 1),
            (AnalysisInputRole::CurveRulePack, 1),
            (AnalysisInputRole::CurveNodeDefinition, 2),
        ]),
        CURVE_ALGORITHM_ID,
    );
    assert_eq!(fixture.calls.curve.load(Ordering::SeqCst), 2);

    let carry_request = carry_request(&fixture.ports);
    let first_carry = fixture
        .service
        .analyze_carry_roll(Request::new(carry_request.clone()))
        .await
        .expect("carry response is transported")
        .into_inner();
    let second_carry = fixture
        .service
        .analyze_carry_roll(Request::new(carry_request))
        .await
        .expect("carry response is transported")
        .into_inner();
    assert_eq!(
        first_carry.encode_to_vec(),
        second_carry.encode_to_vec(),
        "identical verified Carry inputs must produce byte-identical responses",
    );
    let first_carry = carry_analysis(first_carry);
    let second_carry = carry_analysis(second_carry);
    assert_eq!(first_carry.metadata, second_carry.metadata);
    assert_metadata(
        first_carry.metadata.as_ref().expect("carry metadata"),
        &roles(&[
            (AnalysisInputRole::Subject, 1),
            (AnalysisInputRole::Unit, 9),
            (AnalysisInputRole::Bond, 1),
            (AnalysisInputRole::Calendar, 1),
            (AnalysisInputRole::CurveSnapshot, 1),
            (AnalysisInputRole::DataSnapshot, 1),
            (AnalysisInputRole::DataSource, 1),
            (AnalysisInputRole::CurveRulePack, 1),
            (AnalysisInputRole::CurveNodeDefinition, 2),
        ]),
        CARRY_ROLL_ALGORITHM_ID,
    );
    assert_eq!(fixture.calls.carry.load(Ordering::SeqCst), 2);

    let delivery_request = delivery_request(&fixture.ports);
    let first_delivery = fixture
        .service
        .analyze_futures_delivery(Request::new(delivery_request.clone()))
        .await
        .expect("delivery response is transported")
        .into_inner();
    let second_delivery = fixture
        .service
        .analyze_futures_delivery(Request::new(delivery_request))
        .await
        .expect("delivery response is transported")
        .into_inner();
    assert_eq!(
        first_delivery.encode_to_vec(),
        second_delivery.encode_to_vec(),
        "identical verified Delivery inputs must produce byte-identical responses",
    );
    let first_delivery = delivery_analysis(first_delivery);
    let second_delivery = delivery_analysis(second_delivery);
    assert_eq!(first_delivery.candidates.len(), 2);
    assert_eq!(
        first_delivery.ctd_index, 1,
        "market CTD is the taxable Bond"
    );
    assert_eq!(
        first_delivery.subject_ctd_index, 0,
        "coupon output VAT reverses the subject CTD to the exempt Bond"
    );
    for candidate in &first_delivery.candidates {
        assert_eq!(
            candidate.claim_scope, 1,
            "the production v2 fixture returns the authority-approved claim scope"
        );
        let measures = candidate.measures.as_ref().expect("delivery measures");
        assert!(measures.tax_adjusted_interim_coupons.is_some());
        assert!(measures.subject_tax_adjusted_irr.is_some());
    }
    assert_eq!(
        selected_bond_id(&first_delivery, first_delivery.ctd_index),
        id('D').as_str(),
    );
    assert_eq!(
        selected_bond_id(&first_delivery, first_delivery.subject_ctd_index),
        id('B').as_str(),
    );
    assert_eq!(first_delivery.metadata, second_delivery.metadata);
    assert_metadata(
        first_delivery.metadata.as_ref().expect("delivery metadata"),
        &roles(&[
            (AnalysisInputRole::Subject, 1),
            (AnalysisInputRole::Unit, 9),
            (AnalysisInputRole::Bond, 2),
            (AnalysisInputRole::DataSnapshot, 1),
            (AnalysisInputRole::DataSource, 1),
            (AnalysisInputRole::TaxRulePack, 1),
            (AnalysisInputRole::FundingRulePack, 1),
            (AnalysisInputRole::DeliveryRulePack, 1),
            (AnalysisInputRole::FuturesContract, 1),
        ]),
        FUTURES_DELIVERY_ALGORITHM_ID,
    );
    assert_eq!(fixture.calls.delivery.load(Ordering::SeqCst), 4);

    let hedge_request = hedge_request(&fixture.ports);
    let first_hedge = fixture
        .service
        .analyze_futures_hedge(Request::new(hedge_request.clone()))
        .await
        .expect("hedge response is transported")
        .into_inner();
    let second_hedge = fixture
        .service
        .analyze_futures_hedge(Request::new(hedge_request))
        .await
        .expect("hedge response is transported")
        .into_inner();
    assert_eq!(
        first_hedge.encode_to_vec(),
        second_hedge.encode_to_vec(),
        "identical verified Hedge inputs must produce byte-identical responses",
    );
    let first_hedge = hedge_analysis(first_hedge);
    let second_hedge = hedge_analysis(second_hedge);
    let hedge_measures = first_hedge.measures.as_ref().expect("hedge measures");
    // Exact Decimal hand witness:
    // 0.08 * (1_000_000 / 100) / 0.9 = 888.888888888889;
    // The kernel reports the absolute hand count:
    // 1000 / 888.888888888889 = 1.125, rounded to the nearest contract: 1.
    assert_eq!(
        proto_decimal_value(
            hedge_measures
                .futures_contract_dv01
                .as_ref()
                .expect("contract DV01"),
        ),
        ExactDecimal::from_i128_with_scale(888_888_888_888_889, 12)
    );
    assert_eq!(
        proto_decimal_value(
            hedge_measures
                .raw_contracts
                .as_ref()
                .expect("raw contracts"),
        ),
        ExactDecimal::from_i128_with_scale(1_125_000_000_000, 12)
    );
    assert_eq!(hedge_measures.recommended_contracts, 1);
    assert_eq!(first_hedge.metadata, second_hedge.metadata);
    assert_metadata(
        first_hedge.metadata.as_ref().expect("hedge metadata"),
        &roles(&[
            (AnalysisInputRole::Subject, 1),
            (AnalysisInputRole::Unit, 9),
            (AnalysisInputRole::Bond, 1),
            (AnalysisInputRole::DeliveryRulePack, 1),
            (AnalysisInputRole::FuturesContract, 1),
            (AnalysisInputRole::TargetRiskArtifact, 1),
            (AnalysisInputRole::DeliveryArtifact, 1),
            (AnalysisInputRole::CtdAnalyticsArtifact, 1),
        ]),
        FUTURES_HEDGE_ALGORITHM_ID,
    );
    assert_eq!(fixture.calls.hedge.load(Ordering::SeqCst), 2);

    let baseline_metadata = first_bond.metadata.as_ref().expect("Bond metadata");
    let mut later_knowledge = bond_request;
    later_knowledge
        .context
        .as_mut()
        .expect("context")
        .knowledge_at = Some(proto_time(20, 9));
    let later = bond_analysis(
        fixture
            .service
            .analyze_bond(Request::new(later_knowledge))
            .await
            .expect("Bond response is transported")
            .into_inner(),
    );
    let later_metadata = later.metadata.as_ref().expect("Bond metadata");
    assert_eq!(
        baseline_metadata.consumed_inputs,
        later_metadata.consumed_inputs
    );
    assert_ne!(
        baseline_metadata.request_fingerprint,
        later_metadata.request_fingerprint
    );
    assert_ne!(
        baseline_metadata
            .parameter_digest
            .as_ref()
            .expect("parameter digest")
            .canonical_parameters_sha256,
        later_metadata
            .parameter_digest
            .as_ref()
            .expect("parameter digest")
            .canonical_parameters_sha256
    );
    assert_eq!(fixture.calls.bond.load(Ordering::SeqCst), 6);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn private_bond_seam_rejects_stale_or_extra_proof_before_engine() {
    let fixture = Fixture::new(Drift::None);
    let request = bond_request(&fixture.ports);
    let response = fixture
        .service
        .analyze_bond(Request::new(request.clone()))
        .await
        .expect("Bond response is transported")
        .into_inner();
    let public_metadata = bond_analysis(response)
        .metadata
        .expect("public materialization supplies metadata");
    let materialized = materialize_bond(&fixture.ports).await;
    assert_eq!(
        materialized.coupon_tax_treatment().coupon_tax_rate(),
        fixed("6", 2)
    );
    let metadata = RatesGrpcService::canonical_materialized_bond_metadata(
        &request,
        materialized.input(),
        materialized.coupon_tax_treatment(),
        &public_metadata.consumed_inputs,
    )
    .expect("canonical private-port metadata is valid");
    assert!(
        public_metadata.formal_evidence.is_some(),
        "public Rates success must carry persisted formal evidence"
    );
    let mut public_private_port_view = public_metadata.clone();
    public_private_port_view.formal_evidence = None;
    assert_eq!(metadata, public_private_port_view);

    let mut wrong_schema = metadata.clone();
    wrong_schema.schema_id.push_str(".stale");

    let mut stale_parameter_digest = metadata.clone();
    stale_parameter_digest
        .parameter_digest
        .as_mut()
        .expect("parameter digest")
        .canonical_parameters_sha256
        .as_mut()
        .expect("parameter hash")
        .value[0] ^= 1;

    let mut stale_fingerprint = metadata.clone();
    stale_fingerprint
        .request_fingerprint
        .as_mut()
        .expect("request fingerprint")
        .value[0] ^= 1;

    let mut extra_evidence = metadata.clone();
    extra_evidence.consumed_inputs.push(
        extra_evidence
            .consumed_inputs
            .first()
            .expect("at least one evidence item")
            .clone(),
    );
    extra_evidence
        .consumed_inputs
        .sort_by_key(Message::encode_to_vec);

    for (case, supplied_metadata, coupon_tax_treatment) in [
        (
            "wrong schema",
            wrong_schema,
            CouponTaxTreatment::legacy_retained_rate(fixed("1", 1), unit_ref('Z')),
        ),
        (
            "stale parameter digest",
            stale_parameter_digest,
            CouponTaxTreatment::legacy_retained_rate(fixed("1", 1), unit_ref('Z')),
        ),
        (
            "stale fingerprint",
            stale_fingerprint,
            CouponTaxTreatment::legacy_retained_rate(fixed("1", 1), unit_ref('Z')),
        ),
        (
            "extra evidence",
            extra_evidence,
            CouponTaxTreatment::legacy_retained_rate(fixed("1", 1), unit_ref('Z')),
        ),
        (
            "coupon tax scalar drift",
            metadata,
            CouponTaxTreatment::legacy_retained_rate(fixed("2", 1), unit_ref('Z')),
        ),
    ] {
        let calls = Calls::default();
        let engine = RecordingEngines {
            calls: calls.clone(),
        };
        assert!(
            RatesGrpcService::execute_materialized_bond_request(
                &engine,
                &request,
                materialized.input(),
                &coupon_tax_treatment,
                supplied_metadata,
            )
            .is_err(),
            "{case} must fail closed"
        );
        assert_eq!(calls.total(), 0, "{case} reached the numerical engine");
    }

    let changed_terms = BondTerms::with_issuance(
        materialized.input().terms().first_issue_date(),
        materialized.input().terms().current_issue_date(),
        materialized.input().terms().maturity_date(),
        CouponFrequency::Semiannual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        fixed("3", 2),
        materialized.input().terms().face_amount(),
        materialized.input().terms().cumulative_issued_amount(),
        materialized
            .input()
            .terms()
            .tax_attributes()
            .expect("exact Bond has tax attributes"),
    )
    .expect("drifted but individually valid terms");
    let changed_input = BondAnalyticsInput::new(
        materialized.input().owner().clone(),
        materialized.input().bond().clone(),
        materialized.input().rule_pack().clone(),
        materialized.input().snapshot().clone(),
        materialized.input().valuation_at().clone(),
        materialized.input().settlement_date(),
        materialized.input().calendar_requirement(),
        materialized.input().calendar().clone(),
        changed_terms,
        materialized.input().mode(),
        materialized.input().input_value(),
    )
    .expect("drifted private input is individually valid");
    let calls = Calls::default();
    let engine = RecordingEngines {
        calls: calls.clone(),
    };
    assert!(
        RatesGrpcService::execute_materialized_bond_request(
            &engine,
            &request,
            &changed_input,
            materialized.coupon_tax_treatment(),
            public_metadata,
        )
        .is_err(),
        "materialized Bond terms drift must fail closed"
    );
    assert_eq!(calls.total(), 0, "terms drift reached the numerical engine");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn identity_version_hash_owner_knowledge_valuation_visible_effective_and_content_drift_close_before_engines()
 {
    let fixture = Fixture::new(Drift::None);

    let mut identity = bond_request(&fixture.ports);
    identity.bond.as_mut().expect("Bond binding").object = Some(ProtoVersionRef {
        id: Some(proto_ulid('Z')),
        version: 1,
    });
    assert_bond_error(
        fixture
            .service
            .analyze_bond(Request::new(identity))
            .await
            .expect("business error is transported")
            .into_inner(),
    );
    assert_eq!(fixture.calls.bond.load(Ordering::SeqCst), 0);

    let mut version_drift = bond_request(&fixture.ports);
    version_drift
        .bond
        .as_mut()
        .expect("Bond binding")
        .object
        .as_mut()
        .expect("Bond reference")
        .version = 2;
    assert_bond_error(
        fixture
            .service
            .analyze_bond(Request::new(version_drift))
            .await
            .expect("business error is transported")
            .into_inner(),
    );
    assert_eq!(fixture.calls.bond.load(Ordering::SeqCst), 0);

    let mut hash_drift = bond_request(&fixture.ports);
    hash_drift
        .bond
        .as_mut()
        .expect("Bond binding")
        .content_hash
        .as_mut()
        .expect("Bond hash")
        .value[0] ^= 1;
    assert_bond_error(
        fixture
            .service
            .analyze_bond(Request::new(hash_drift))
            .await
            .expect("business error is transported")
            .into_inner(),
    );
    assert_eq!(fixture.calls.bond.load(Ordering::SeqCst), 0);

    let owner_drift = Fixture::new(Drift::DataOwner);
    assert_bond_error(
        owner_drift
            .service
            .analyze_bond(Request::new(bond_request(&owner_drift.ports)))
            .await
            .expect("business error is transported")
            .into_inner(),
    );
    assert_eq!(owner_drift.calls.bond.load(Ordering::SeqCst), 0);

    let mut knowledge_drift = bond_request(&fixture.ports);
    knowledge_drift
        .context
        .as_mut()
        .expect("context")
        .knowledge_at = Some(proto_time(20, 5));
    assert_bond_error(
        fixture
            .service
            .analyze_bond(Request::new(knowledge_drift))
            .await
            .expect("business error is transported")
            .into_inner(),
    );
    assert_eq!(fixture.calls.bond.load(Ordering::SeqCst), 0);

    let mut valuation_drift = bond_request(&fixture.ports);
    valuation_drift.valuation_at = Some(proto_time(20, 5));
    assert_bond_error(
        fixture
            .service
            .analyze_bond(Request::new(valuation_drift))
            .await
            .expect("business error is transported")
            .into_inner(),
    );
    assert_eq!(fixture.calls.bond.load(Ordering::SeqCst), 0);

    let mut unit_role_drift = bond_request(&fixture.ports);
    let units = unit_role_drift
        .context
        .as_mut()
        .expect("context")
        .units
        .as_mut()
        .expect("units");
    std::mem::swap(&mut units.rate, &mut units.dimensionless);
    assert_bond_error(
        fixture
            .service
            .analyze_bond(Request::new(unit_role_drift))
            .await
            .expect("business error is transported")
            .into_inner(),
    );
    assert_eq!(fixture.calls.bond.load(Ordering::SeqCst), 0);

    let visible_drift = Fixture::new(Drift::CurveVisibleAfterKnowledge);
    assert_curve_error(
        visible_drift
            .service
            .interpolate_yield_curve(Request::new(curve_request()))
            .await
            .expect("business error is transported")
            .into_inner(),
    );
    assert_eq!(visible_drift.calls.curve.load(Ordering::SeqCst), 0);

    let effective_drift = Fixture::new(Drift::TaxEffectiveAfterValuation);
    assert_bond_error(
        effective_drift
            .service
            .analyze_bond(Request::new(bond_request(&effective_drift.ports)))
            .await
            .expect("business error is transported")
            .into_inner(),
    );
    assert_eq!(effective_drift.calls.bond.load(Ordering::SeqCst), 0);

    let content_drift = Fixture::new(Drift::CorruptDataContent);
    assert_bond_error(
        content_drift
            .service
            .analyze_bond(Request::new(bond_request(&content_drift.ports)))
            .await
            .expect("business error is transported")
            .into_inner(),
    );
    assert_eq!(content_drift.calls.bond.load(Ordering::SeqCst), 0);
    assert_eq!(
        content_drift.ports.integrity_events.load(Ordering::SeqCst),
        1
    );
}

fn bond_request(ports: &FixturePorts) -> AnalyzeBondRequest {
    AnalyzeBondRequest {
        context: Some(context(ALGORITHM_ID, ALGORITHM_VERSION, CONVENTION_PROFILE)),
        bond: Some(ports.binding('D')),
        valuation_at: Some(proto_time(20, 4)),
        settlement_date: "2026-07-21".to_owned(),
        calendar_requirement: ProtoCalendarRequirement::ReferenceReplay as i32,
        calendar: Some(ports.binding('K')),
        input: Some(analyze_bond_request::Input::CleanPrice(proto_decimal(
            "100", 0, 'P',
        ))),
        data_snapshot: Some(snapshot_binding('E', &ContentHash::digest(DATA_BYTES))),
        tax_rule_pack: Some(ports.binding('T')),
    }
}

async fn materialize_bond(ports: &FixturePorts) -> BondRatesMaterialization {
    let parser = TaxRulePackV2Parser;
    MaterializeBondRatesInput::new(ports, ports, ports, ports, ports, &parser)
        .execute(
            &AccessScope::new(id('0'), id('2'), vec![id('1')])
                .expect("fixture access scope is valid"),
            BondRatesCommand {
                owner: owner(),
                subject_ref: VersionRef::new(id('S'), version(1)),
                units: unit_requirements(),
                currency_unit: unit_ref('M'),
                rate_unit: unit_ref('Z'),
                knowledge_at: time(20, 8),
                bond: analytics_ref(ports.definition('D')),
                calendar: analytics_ref(ports.definition('K')),
                data_snapshot: ImmutableSnapshotBinding::new(
                    id('E'),
                    ContentHash::digest(DATA_BYTES),
                ),
                tax_rule_pack: analytics_ref(ports.definition('T')),
                valuation_at: time(20, 4),
                settlement_date: date(2026, 7, 21),
                calendar_requirement: CalendarRequirement::ReferenceReplay,
                mode: AnalyticsMode::PriceIn,
                input_value: fixed("100", 0),
            },
            SafeTraceContext::new("0123456789abcdef0123456789abcdef")
                .expect("fixture trace is valid"),
        )
        .await
        .expect("fixture Bond materialization is valid")
}

async fn materialize_delivery(ports: &FixturePorts) -> DeliveryRatesMaterialization {
    let delivery = CgbFuturesDeliveryRulePackParser;
    let funding = FundingRulePackV1Parser;
    let tax = TaxRulePackV2Parser;
    MaterializeDeliveryRatesInput::new(
        ports, ports, ports, ports, ports, ports, ports, &delivery, &funding, &tax,
    )
    .execute(
        &AccessScope::new(id('0'), id('2'), vec![id('1')]).expect("fixture access scope is valid"),
        DeliveryRatesCommand {
            owner: owner(),
            subject_ref: VersionRef::new(id('S'), version(1)),
            units: unit_requirements(),
            currency_unit: unit_ref('M'),
            price_unit: unit_ref('P'),
            rate_unit: unit_ref('Z'),
            knowledge_at: time(20, 8),
            futures_contract: analytics_ref(ports.definition('C')),
            data_snapshot: ImmutableSnapshotBinding::new(id('E'), ContentHash::digest(DATA_BYTES)),
            funding_rule_pack: analytics_ref(ports.definition('F')),
            tax_rule_pack: analytics_ref(ports.definition('T')),
            valuation_at: time(20, 4),
            purchase_date: date(2026, 7, 20),
        },
        SafeTraceContext::new("0123456789abcdef0123456789abcdef").expect("fixture trace is valid"),
    )
    .await
    .expect("fixture Delivery materialization is valid")
}

#[tokio::test]
async fn production_v2_delivery_materializes_exact_tax_treatment() {
    let fixture = Fixture::new(Drift::None);
    let materialized = materialize_delivery(&fixture.ports).await;
    assert_eq!(
        fixture.ports.tax_reads.load(Ordering::SeqCst),
        1,
        "one exact TaxRulePack read serves every candidate"
    );
    assert_eq!(materialized.inputs().len(), 2);
    assert_eq!(
        materialized
            .coupon_tax_treatments()
            .iter()
            .map(CouponTaxTreatment::value_added_tax_rate)
            .collect::<Vec<_>>(),
        vec![FixedDecimal::ZERO, fixed("6", 2)]
    );
    let market = NativeFuturesDeliveryEngine
        .calculate(&materialized.inputs()[1])
        .expect("market Delivery engine succeeds");
    let measures = market.measures();
    let adjusted = materialized.coupon_tax_treatments()[1]
        .adjust_coupon(measures.interim_coupons())
        .expect("authority coupon adjustment succeeds");
    assert!(adjusted < measures.interim_coupons());
    let ratio = measures
        .invoice_price()
        .checked_add(adjusted)
        .and_then(|value| value.checked_div_round_ties_even(measures.purchase_dirty_price()))
        .expect("subject return ratio is representable");
    let _annualized = ratio
        .checked_sub(FixedDecimal::ONE)
        .and_then(|value| value.checked_mul_integer(365))
        .and_then(|value| value.checked_div_round_ties_even(fixed("60", 0)))
        .expect("subject annualized IRR is representable");
}

#[tokio::test]
async fn production_v2_delivery_matches_oracle_and_is_order_invariant() {
    let fixture = Fixture::with_oracle_delivery(Drift::None);
    let result = delivery_analysis(
        fixture
            .service
            .analyze_futures_delivery(Request::new(delivery_request(&fixture.ports)))
            .await
            .expect("Oracle Delivery response is transported")
            .into_inner(),
    );
    assert_oracle_delivery_result(&result);
    assert_eq!(fixture.calls.delivery.load(Ordering::SeqCst), 2);

    let reversed = Fixture::with_oracle_delivery(Drift::ReverseQuoteOrder);
    let reversed_result = delivery_analysis(
        reversed
            .service
            .analyze_futures_delivery(Request::new(delivery_request(&reversed.ports)))
            .await
            .expect("reordered Oracle Delivery response is transported")
            .into_inner(),
    );
    assert_oracle_delivery_result(&reversed_result);
    assert_eq!(reversed.calls.delivery.load(Ordering::SeqCst), 2);
    assert_eq!(
        result.candidates, reversed_result.candidates,
        "verified quote order must not alter the stably materialized candidate results",
    );
}

fn assert_oracle_delivery_result(
    result: &ficant_contracts::ficant::rates::v1::AnalyzeFuturesDeliveryResult,
) {
    assert_eq!(result.candidates.len(), 2);
    assert_eq!(selected_bond_id(result, result.ctd_index), id('D').as_str());
    assert_eq!(
        selected_bond_id(result, result.subject_ctd_index),
        id('B').as_str(),
    );
    let expected = R5E_ORACLE_EXPECTED;
    let expected_basket = oracle_basket(expected, "market-subject-ctd-reversal");
    let market_ctd = oracle_fixture_bond_id(oracle_string(expected_basket, "market_ctd_bond_id"));
    let subject_ctd = oracle_fixture_bond_id(oracle_string(expected_basket, "subject_ctd_bond_id"));
    assert_eq!(
        selected_bond_id(result, result.ctd_index),
        market_ctd.as_str()
    );
    assert_eq!(
        selected_bond_id(result, result.subject_ctd_index),
        subject_ctd.as_str(),
    );
    for candidate in &result.candidates {
        let bond = selected_candidate_bond_id(candidate);
        let oracle_bond = if bond == id('B').as_str() {
            "CGB-EXEMPT"
        } else if bond == id('D').as_str() {
            "CGB-TAXABLE"
        } else {
            panic!("unexpected candidate Bond {bond}");
        };
        let expected = oracle_candidate(expected, "market-subject-ctd-reversal", oracle_bond);
        let measures = candidate.measures.as_ref().expect("candidate measures");
        assert_eq!(
            proto_decimal_value(
                measures
                    .tax_adjusted_interim_coupons
                    .as_ref()
                    .expect("tax-adjusted interim coupons"),
            ),
            oracle_decimal(oracle_string(expected, "tax_adjusted_interim_coupons")),
        );
        assert_eq!(
            proto_decimal_value(
                measures
                    .implied_repo_rate
                    .as_ref()
                    .expect("market pre-tax IRR"),
            ),
            oracle_decimal(oracle_string(expected, "market_pre_tax_irr")),
        );
        assert_eq!(
            proto_decimal_value(
                measures
                    .subject_tax_adjusted_irr
                    .as_ref()
                    .expect("subject tax-adjusted IRR"),
            ),
            oracle_decimal(oracle_string(expected, "subject_tax_adjusted_irr")),
        );
    }
}

#[tokio::test]
async fn production_v2_delivery_tax_drift_matrix_never_reaches_the_engine() {
    for drift in [
        Drift::TaxEffectiveAfterValuation,
        Drift::TaxSource,
        Drift::TaxVerification,
        Drift::TaxPayload,
        Drift::RateUnitDefinition,
        Drift::SubjectTaxProfile,
        Drift::CandidateTaxAttributes,
    ] {
        let fixture = Fixture::new(drift);
        assert_delivery_error(
            fixture
                .service
                .analyze_futures_delivery(Request::new(delivery_request(&fixture.ports)))
                .await
                .expect("business error is transported")
                .into_inner(),
        );
        assert_eq!(
            fixture.calls.delivery.load(Ordering::SeqCst),
            0,
            "{drift:?} reached the Delivery engine"
        );
    }

    let fixture = Fixture::new(Drift::None);
    let mut hash_drift = delivery_request(&fixture.ports);
    hash_drift
        .tax_rule_pack
        .as_mut()
        .expect("TaxRulePack binding")
        .content_hash
        .as_mut()
        .expect("TaxRulePack hash")
        .value[0] ^= 1;
    assert_delivery_error(
        fixture
            .service
            .analyze_futures_delivery(Request::new(hash_drift))
            .await
            .expect("business error is transported")
            .into_inner(),
    );
    assert_eq!(fixture.calls.delivery.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn production_v2_no_tax_difference_keeps_market_and_subject_ctd_identical() {
    let fixture = Fixture::new(Drift::AllExempt);
    let result = delivery_analysis(
        fixture
            .service
            .analyze_futures_delivery(Request::new(delivery_request(&fixture.ports)))
            .await
            .expect("Delivery response is transported")
            .into_inner(),
    );
    assert_eq!(result.candidates.len(), 2);
    assert_eq!(result.ctd_index, result.subject_ctd_index);
    for candidate in result.candidates {
        let measures = candidate.measures.expect("Delivery measures");
        assert_eq!(
            measures.interim_coupons, measures.tax_adjusted_interim_coupons,
            "an exempt Bond has no coupon tax difference",
        );
        assert!(measures.subject_tax_adjusted_irr.is_some());
    }
}

fn curve_request() -> InterpolateYieldCurveRequest {
    InterpolateYieldCurveRequest {
        context: Some(context(
            CURVE_ALGORITHM_ID,
            CURVE_ALGORITHM_VERSION,
            CURVE_CONVENTION_PROFILE,
        )),
        curve: Some(snapshot_binding('L', &ContentHash::digest(CURVE_BYTES))),
        query_date: "2034-01-01".to_owned(),
    }
}

fn carry_request(ports: &FixturePorts) -> AnalyzeCarryRollRequest {
    AnalyzeCarryRollRequest {
        context: Some(context(
            CARRY_ROLL_ALGORITHM_ID,
            CARRY_ROLL_ALGORITHM_VERSION,
            CARRY_ROLL_CONVENTION_PROFILE,
        )),
        bond: Some(ports.binding('D')),
        valuation_at: Some(proto_time(20, 4)),
        initial_settlement: "2026-07-21".to_owned(),
        horizon_settlement: "2027-07-21".to_owned(),
        calendar_requirement: ProtoCalendarRequirement::ReferenceReplay as i32,
        curve: Some(snapshot_binding('L', &ContentHash::digest(CURVE_BYTES))),
    }
}

fn delivery_request(ports: &FixturePorts) -> AnalyzeFuturesDeliveryRequest {
    AnalyzeFuturesDeliveryRequest {
        context: Some(context(
            FUTURES_DELIVERY_ALGORITHM_ID,
            FUTURES_DELIVERY_ALGORITHM_VERSION,
            FUTURES_DELIVERY_CONVENTION_PROFILE,
        )),
        futures_contract: Some(ports.binding('C')),
        valuation_at: Some(proto_time(20, 4)),
        purchase_date: "2026-07-20".to_owned(),
        data_snapshot: Some(snapshot_binding('E', &ContentHash::digest(DATA_BYTES))),
        funding_rule_pack: Some(ports.binding('F')),
        tax_rule_pack: Some(ports.binding('T')),
    }
}

fn hedge_request(ports: &FixturePorts) -> AnalyzeFuturesHedgeRequest {
    AnalyzeFuturesHedgeRequest {
        context: Some(context(
            FUTURES_HEDGE_ALGORITHM_ID,
            FUTURES_HEDGE_ALGORITHM_VERSION,
            FUTURES_HEDGE_CONVENTION_PROFILE,
        )),
        target_risk_artifact: Some(ports.artifact_binding('A')),
        delivery_artifact: Some(ports.artifact_binding('G')),
        ctd_analytics_artifact: Some(ports.artifact_binding('J')),
        futures_contract: Some(ports.binding('C')),
        valuation_at: Some(proto_time(20, 4)),
    }
}

fn context(
    algorithm_id: &str,
    algorithm_version: u32,
    convention_profile: &str,
) -> AnalysisContext {
    AnalysisContext {
        owner: Some(proto_owner()),
        algorithm: Some(AlgorithmBinding {
            algorithm_id: algorithm_id.to_owned(),
            algorithm_version,
            convention_profile: convention_profile.to_owned(),
            abi_version: ABI_VERSION,
        }),
        units: Some(AnalysisUnits {
            currency_amount: Some(proto_unit('M')),
            price_per_100: Some(proto_unit('P')),
            rate: Some(proto_unit('Z')),
            years: Some(proto_unit('Y')),
            years_squared: Some(proto_unit('X')),
            dv01_per_100: Some(proto_unit('V')),
            dv01: Some(proto_unit('W')),
            dimensionless: Some(proto_unit('I')),
            contract_count: Some(proto_unit('H')),
        }),
        subject_ref: Some(ProtoVersionRef {
            id: Some(proto_ulid('S')),
            version: 1,
        }),
        knowledge_at: Some(proto_time(20, 8)),
    }
}

fn roles(specification: &[(AnalysisInputRole, usize)]) -> Vec<AnalysisInputRole> {
    let mut result = specification
        .iter()
        .flat_map(|(role, count)| std::iter::repeat_n(*role, *count))
        .collect::<Vec<_>>();
    result.sort_by_key(|value| *value as i32);
    result
}

fn assert_metadata(
    metadata: &ResultMetadata,
    expected_roles: &[AnalysisInputRole],
    algorithm_id: &str,
) {
    let actual_roles = metadata
        .consumed_inputs
        .iter()
        .map(|value| AnalysisInputRole::try_from(value.role).expect("closed input role"))
        .collect::<Vec<_>>();
    assert_eq!(actual_roles, expected_roles);
    assert!(
        metadata
            .consumed_inputs
            .windows(2)
            .all(|pair| pair[0].encode_to_vec() <= pair[1].encode_to_vec()),
        "consumed input evidence must be stably sorted"
    );
    for input in &metadata.consumed_inputs {
        assert_eq!(input.owner.as_ref(), Some(&proto_owner()));
        assert!(input.binding.is_some());
        let role = AnalysisInputRole::try_from(input.role).expect("closed input role");
        match role {
            AnalysisInputRole::CurveSnapshot | AnalysisInputRole::DataSnapshot => {
                assert_eq!(input.observed_at.as_ref(), Some(&proto_time(20, 4)));
                assert_eq!(input.visible_at.as_ref(), Some(&proto_time(20, 6)));
            }
            AnalysisInputRole::Calendar
            | AnalysisInputRole::CurveRulePack
            | AnalysisInputRole::TaxRulePack
            | AnalysisInputRole::FundingRulePack
            | AnalysisInputRole::DeliveryRulePack => {
                assert!(input.effective_from.is_some());
                assert!(input.effective_to.is_some());
            }
            AnalysisInputRole::TargetRiskArtifact
            | AnalysisInputRole::DeliveryArtifact
            | AnalysisInputRole::CtdAnalyticsArtifact => {
                assert_eq!(input.observed_at.as_ref(), Some(&proto_time(20, 4)));
            }
            AnalysisInputRole::CurveNodeDefinition => {
                let Some(
                    ficant_contracts::ficant::rates::v1::analysis_input_binding::Binding::CurveNode(
                        curve_node,
                    ),
                ) = input.binding.as_ref()
                else {
                    panic!("CurveNodeDefinition evidence must use CurveNodeBinding");
                };
                assert!(curve_node.curve_node_id.starts_with("cn.gov.yield-curve."));
                assert_eq!(
                    curve_node
                        .content_hash
                        .as_ref()
                        .expect("factor content hash")
                        .value
                        .len(),
                    32
                );
            }
            _ => {}
        }
    }
    let algorithm = metadata.algorithm.as_ref().expect("metadata algorithm");
    assert_eq!(algorithm.algorithm_id, algorithm_id);
    let digest = metadata
        .parameter_digest
        .as_ref()
        .expect("parameter digest");
    assert_eq!(digest.algorithm.as_ref(), Some(algorithm));
    assert_eq!(
        digest
            .canonical_parameters_sha256
            .as_ref()
            .expect("parameter hash")
            .value
            .len(),
        32
    );
    assert_eq!(
        metadata
            .request_fingerprint
            .as_ref()
            .expect("request fingerprint")
            .value
            .len(),
        32
    );
}

fn bond_analysis(
    response: ficant_contracts::ficant::rates::v1::AnalyzeBondResponse,
) -> ficant_contracts::ficant::rates::v1::AnalyzeBondResult {
    match response.result {
        Some(analyze_bond_response::Result::Analysis(value)) => value,
        other => panic!("Bond exact materialization must succeed: {other:?}"),
    }
}

fn curve_point(
    response: ficant_contracts::ficant::rates::v1::InterpolateYieldCurveResponse,
) -> ficant_contracts::ficant::rates::v1::InterpolateYieldCurveResult {
    match response.result {
        Some(interpolate_yield_curve_response::Result::Point(value)) => value,
        other => panic!("curve exact materialization must succeed: {other:?}"),
    }
}

fn carry_analysis(
    response: ficant_contracts::ficant::rates::v1::AnalyzeCarryRollResponse,
) -> ficant_contracts::ficant::rates::v1::AnalyzeCarryRollResult {
    match response.result {
        Some(analyze_carry_roll_response::Result::Analysis(value)) => value,
        other => panic!("carry exact materialization must succeed: {other:?}"),
    }
}

fn delivery_analysis(
    response: ficant_contracts::ficant::rates::v1::AnalyzeFuturesDeliveryResponse,
) -> ficant_contracts::ficant::rates::v1::AnalyzeFuturesDeliveryResult {
    match response.result {
        Some(analyze_futures_delivery_response::Result::Analysis(value)) => value,
        other => panic!("delivery exact materialization must succeed: {other:?}"),
    }
}

fn hedge_analysis(
    response: ficant_contracts::ficant::rates::v1::AnalyzeFuturesHedgeResponse,
) -> ficant_contracts::ficant::rates::v1::AnalyzeFuturesHedgeResult {
    match response.result {
        Some(analyze_futures_hedge_response::Result::Analysis(value)) => value,
        other => panic!("hedge exact materialization must succeed: {other:?}"),
    }
}

fn assert_bond_error(response: ficant_contracts::ficant::rates::v1::AnalyzeBondResponse) {
    let result = response.result;
    assert!(matches!(
        result,
        Some(analyze_bond_response::Result::Error(_))
    ));
}

fn assert_curve_error(
    response: ficant_contracts::ficant::rates::v1::InterpolateYieldCurveResponse,
) {
    let result = response.result;
    assert!(matches!(
        result,
        Some(interpolate_yield_curve_response::Result::Error(_))
    ));
}

fn assert_carry_error(response: ficant_contracts::ficant::rates::v1::AnalyzeCarryRollResponse) {
    let result = response.result;
    assert!(matches!(
        result,
        Some(analyze_carry_roll_response::Result::Error(_))
    ));
}

fn assert_delivery_error(
    response: ficant_contracts::ficant::rates::v1::AnalyzeFuturesDeliveryResponse,
) {
    let result = response.result;
    assert!(matches!(
        result,
        Some(analyze_futures_delivery_response::Result::Error(_))
    ));
}

fn assert_hedge_error(response: ficant_contracts::ficant::rates::v1::AnalyzeFuturesHedgeResponse) {
    let result = response.result;
    assert!(matches!(
        result,
        Some(analyze_futures_hedge_response::Result::Error(_))
    ));
}

fn unit_definitions(drift_rate_definition: bool) -> Vec<DefinitionValue> {
    [
        ('M', "CNY", "currency_amount", 2, 28),
        ('P', "CNY100", "price_per_100", 12, 28),
        ('Z', "RATE", "rate", 12, 18),
        ('Y', "YEAR", "years", 12, 28),
        ('X', "YEAR2", "years_squared", 12, 28),
        ('V', "DV01_100", "dv01_per_100", 12, 28),
        ('W', "DV01", "dv01", 12, 28),
        ('I', "ONE", "dimensionless", 12, 28),
        ('H', "CONTRACT", "contract_count", 0, 28),
    ]
    .into_iter()
    .map(|(suffix, code, dimension, scale, mut precision)| {
        if suffix == 'Z' && drift_rate_definition {
            precision = 17;
        }
        DefinitionValue::Unit(
            Unit::new(UnitInput {
                unit_id: id(suffix),
                version: version(1),
                owner: owner(),
                code: code.to_owned(),
                dimension: dimension.to_owned(),
                scale,
                precision,
            })
            .expect("fixture Unit is valid"),
        )
    })
    .collect()
}

fn calendar() -> Calendar {
    Calendar::new(CalendarInput {
        calendar_id: id('K'),
        version: version(1),
        owner: owner(),
        market: "CN".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: EffectivePeriod::new(market_time(2020, 1, 1, 4), market_time(2040, 1, 1, 4))
            .expect("fixture Calendar period is valid"),
        sessions: vec![
            CalendarSession::open(
                date(2026, 7, 20),
                NaiveTime::from_hms_opt(9, 0, 0).expect("valid time"),
                NaiveTime::from_hms_opt(17, 0, 0).expect("valid time"),
            )
            .expect("fixture Calendar session is valid"),
        ],
    })
    .expect("fixture Calendar is valid")
}

fn curve_rule_pack() -> MarketRulePack {
    let bytes = b"r5d-yield-curve-rule";
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('U'),
            version: version(1),
            owner: owner(),
            market: "CN".to_owned(),
            rule_type: "yield-curve".to_owned(),
            source: "R5D fixture".to_owned(),
            effective: EffectivePeriod::new(market_time(2020, 1, 1, 4), market_time(2040, 1, 1, 4))
                .expect("fixture RulePack period is valid"),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(bytes),
        },
        RulePackContent::new(
            "type.googleapis.com/ficant.market.v1.CurveRulePack",
            bytes.to_vec(),
        )
        .expect("fixture curve RulePack content is valid"),
    )
    .expect("fixture curve RulePack is valid")
}

fn delivery_rule_pack() -> MarketRulePack {
    let payload = CgbFuturesDeliveryRulePack {
        products: vec![CgbFuturesProductRule {
            product_code: Some("T".to_owned()),
            original_term_max_months: Some(120),
            residual_min_months: Some(78),
            residual_upper_bound: Some(ResidualUpperBound::ResidualMaxMonthsUnbounded(true)),
            contract_size_in_quote_units: Some(10_000),
        }],
        delivery_months: vec![3, 6, 9, 12],
        nominal_coupon: Some(proto_decimal("3", 2, 'Z')),
        face_quote_basis: Some(proto_decimal("100", 0, 'P')),
        accrued_interest_day_count: Some(1),
        conversion_factor_rounding_places: Some(4),
        accrued_interest_rounding_places: Some(7),
        annual_day_basis: Some(365),
    };
    let content = RulePackContent::new(
        "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack",
        payload.encode_to_vec(),
    )
    .expect("fixture delivery payload is valid");
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('R'),
            version: version(1),
            owner: owner(),
            market: "CFFEX".to_owned(),
            rule_type: "cgb-futures".to_owned(),
            source: "R5D fixture".to_owned(),
            effective: EffectivePeriod::new(market_time(2020, 1, 1, 4), market_time(2040, 1, 1, 4))
                .expect("fixture RulePack period is valid"),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(content.value()),
        },
        content,
    )
    .expect("fixture delivery RulePack is valid")
}

fn funding_rule_pack() -> MarketRulePack {
    let payload = FundingRulePack {
        rates: vec![FundingTierRate {
            funding_tier: ProtoFundingTier::DrAvailable as i32,
            annual_financing_rate: Some(proto_decimal("18", 3, 'Z')),
        }],
    };
    let content = RulePackContent::new(FUNDING_TYPE_URL, payload.encode_to_vec())
        .expect("fixture funding payload is valid");
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('F'),
            version: version(1),
            owner: owner(),
            market: FUNDING_MARKET.to_owned(),
            rule_type: FUNDING_RULE_TYPE.to_owned(),
            source: "R5D fixture".to_owned(),
            effective: EffectivePeriod::new(market_time(2020, 1, 1, 4), market_time(2040, 1, 1, 4))
                .expect("fixture RulePack period is valid"),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(content.value()),
        },
        content,
    )
    .expect("fixture funding RulePack is valid")
}

fn tax_rule_pack(drift: Drift) -> MarketRulePack {
    let payload = if drift == Drift::TaxPayload {
        b"not-the-authority-payload".to_vec()
    } else {
        include_bytes!("../../../domain-packs/cgb-interest-tax/cgb-interest-tax-v1.bin").to_vec()
    };
    let content =
        RulePackContent::new(TAX_TYPE_URL, payload).expect("fixture tax payload is valid");
    let effective_from = if drift == Drift::TaxEffectiveAfterValuation {
        market_time(2026, 7, 21, 4)
    } else {
        authority_year_start(2026)
    };
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('T'),
            version: version(1),
            owner: owner(),
            market: TAX_MARKET.to_owned(),
            rule_type: TAX_RULE_TYPE.to_owned(),
            source: if drift == Drift::TaxSource {
                "unapproved-source".to_owned()
            } else {
                TAX_SOURCE.to_owned()
            },
            effective: EffectivePeriod::new(effective_from, authority_year_start(2028))
                .expect("fixture RulePack period is valid"),
            verification_status: if drift == Drift::TaxVerification {
                VerificationStatus::Unverified
            } else {
                VerificationStatus::Verified
            },
            content_hash: ContentHash::digest(content.value()),
        },
        content,
    )
    .expect("fixture tax RulePack is valid")
}

fn bond_definition(exempt: bool, attribute_drift: bool) -> DefinitionValue {
    let instrument = Instrument::new(InstrumentInput {
        instrument_id: id('D'),
        version: version(1),
        owner: owner(),
        kind: InstrumentKind::Bond,
        market: "CFFEX".to_owned(),
        symbol: "260011.IB".to_owned(),
        currency: unit_ref('M'),
        calendar: VersionRef::new(id('K'), version(1)),
    })
    .expect("fixture Bond instrument is valid");
    let (first_issue_date, mut tax_attributes) = if exempt {
        (
            date(2025, 2, 8),
            BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt),
        )
    } else {
        (
            date(2025, 8, 8),
            BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Exempt),
        )
    };
    if attribute_drift {
        tax_attributes =
            BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt);
    }
    let bond = Bond::with_issuance(
        &instrument,
        first_issue_date,
        date(2025, 8, 8),
        date(2034, 8, 8),
        domain_decimal("100000000", 0, 'M'),
        tax_attributes,
        domain_decimal("100", 0, 'M'),
    )
    .expect("fixture Bond issuance is valid")
    .with_pricing_terms(
        BondPricingTerms::new(
            domain_decimal("25", 3, 'Z'),
            BondCouponFrequency::Semiannual,
            BondDayCountConvention::ActActBondIsma,
            BondBusinessDayConvention::Following,
        )
        .expect("fixture Bond pricing terms are valid"),
    )
    .expect("fixture Bond is priced");
    DefinitionValue::Instrument(
        InstrumentDefinition::new(instrument, Some(InstrumentSubtype::Bond(bond)))
            .expect("fixture Bond definition is valid"),
    )
}

fn exempt_bond_definition() -> DefinitionValue {
    let instrument = Instrument::new(InstrumentInput {
        instrument_id: id('B'),
        version: version(1),
        owner: owner(),
        kind: InstrumentKind::Bond,
        market: "CFFEX".to_owned(),
        symbol: "250001.IB".to_owned(),
        currency: unit_ref('M'),
        calendar: VersionRef::new(id('K'), version(1)),
    })
    .expect("fixture exempt Bond instrument is valid");
    let bond = Bond::with_issuance(
        &instrument,
        date(2025, 2, 8),
        date(2025, 8, 8),
        date(2034, 8, 8),
        domain_decimal("100000000", 0, 'M'),
        BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt),
        domain_decimal("100", 0, 'M'),
    )
    .expect("fixture exempt Bond issuance is valid")
    .with_pricing_terms(
        BondPricingTerms::new(
            domain_decimal("25", 3, 'Z'),
            BondCouponFrequency::Semiannual,
            BondDayCountConvention::ActActBondIsma,
            BondBusinessDayConvention::Following,
        )
        .expect("fixture exempt Bond pricing terms are valid"),
    )
    .expect("fixture exempt Bond is priced");
    DefinitionValue::Instrument(
        InstrumentDefinition::new(instrument, Some(InstrumentSubtype::Bond(bond)))
            .expect("fixture exempt Bond definition is valid"),
    )
}

fn futures_contract_definition() -> DefinitionValue {
    let instrument = Instrument::new(InstrumentInput {
        instrument_id: id('C'),
        version: version(1),
        owner: owner(),
        kind: InstrumentKind::Futures,
        market: "CFFEX".to_owned(),
        symbol: "T2609".to_owned(),
        currency: unit_ref('M'),
        calendar: VersionRef::new(id('K'), version(1)),
    })
    .expect("fixture Futures instrument is valid");
    let contract = FuturesContract::new(
        &instrument,
        market_time(2026, 9, 11, 11),
        market_time(2026, 9, 11, 15),
        market_time(2026, 9, 18, 8),
        domain_decimal("10000", 0, 'M'),
        VersionRef::new(id('R'), version(1)),
    )
    .expect("fixture FuturesContract is valid")
    .with_risk_terms("T", unit_ref('P'))
    .expect("fixture FuturesContract exact risk terms are valid");
    DefinitionValue::Instrument(
        InstrumentDefinition::new(
            instrument,
            Some(InstrumentSubtype::FuturesContract(contract)),
        )
        .expect("fixture Futures definition is valid"),
    )
}

fn curve_node(curve_node_id: &str, tenor: &str) -> CurveNodeDefinition {
    let mut input = CurveNodeDefinitionInput {
        curve_node_id: curve_node_id.to_owned(),
        curve_family_id: CURVE_FAMILY.to_owned(),
        tenor: tenor.to_owned(),
        factor_unit: unit_ref('Z'),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = CurveNodeDefinition::content_hash_for(&input);
    CurveNodeDefinition::new(input).expect("fixture CurveNode definition is valid")
}

fn fixture_subject(drift_tax_profile: bool) -> SubjectRecord {
    let subject =
        Subject::new(id('S'), "R5D exact Rates subject").expect("fixture Subject is valid");
    let reference = VersionRef::new(subject.id().clone(), version(1));
    let subject_version = SubjectVersion::new(
        reference,
        AccessSet::new(
            ["CFFEX", "CN"],
            [
                "bond-analytics",
                "carry-roll",
                "futures-delivery",
                "futures-hedge",
                "yield-curve",
            ],
        )
        .expect("fixture access set is valid"),
        FundingTier::DrAvailable,
        TaxTreatment::new(
            if drift_tax_profile {
                "small-scale-taxpayer"
            } else {
                "cn-vat-general-taxpayer"
            },
            "cn-cgb-interest-cit-exempt",
        )
        .expect("fixture tax treatment is valid"),
        "direct",
        "principal",
        None,
    )
    .expect("fixture Subject version is valid");
    SubjectRecord::new(subject, subject_version).expect("fixture Subject record is valid")
}

fn artifact_fixtures(
    bond_ref: &AnalyticsObjectRef,
    contract_ref: &AnalyticsObjectRef,
    target_facts: &BondAnalyticsArtifactFacts,
    delivery_facts: &FuturesDeliveryArtifactFacts,
    ctd_facts: &BondAnalyticsArtifactFacts,
) -> Vec<(Artifact, Vec<u8>)> {
    let target = Artifact::new(
        id('A'),
        owner(),
        ArtifactKind::Generic,
        BOND_ANALYTICS_MEDIA_TYPE,
        ContentHash::digest(TARGET_BYTES),
        TARGET_BYTES.len() as u64,
        vec![
            LineageRef::versioned(id('D'), version(1)),
            LineageRef::new(
                id('T'),
                Some(version(1)),
                Some(target_facts.rule_pack().content_hash().clone()),
            )
            .expect("target RulePack lineage is valid"),
            LineageRef::content_addressed(id('E'), target_facts.snapshot().content_hash().clone()),
        ],
    )
    .expect("target Artifact is valid");
    let delivery = Artifact::new(
        id('G'),
        owner(),
        ArtifactKind::Generic,
        FUTURES_DELIVERY_MEDIA_TYPE,
        ContentHash::digest(DELIVERY_BYTES),
        DELIVERY_BYTES.len() as u64,
        vec![
            LineageRef::versioned(
                contract_ref.version_ref().id().clone(),
                contract_ref.version_ref().version(),
            ),
            LineageRef::versioned(
                bond_ref.version_ref().id().clone(),
                bond_ref.version_ref().version(),
            ),
            LineageRef::new(
                id('R'),
                Some(version(1)),
                Some(delivery_facts.rule_pack().content_hash().clone()),
            )
            .expect("delivery RulePack lineage is valid"),
            LineageRef::content_addressed(
                id('E'),
                delivery_facts.snapshot().content_hash().clone(),
            ),
        ],
    )
    .expect("delivery Artifact is valid");
    let ctd = Artifact::new(
        id('J'),
        owner(),
        ArtifactKind::Generic,
        BOND_ANALYTICS_MEDIA_TYPE,
        ContentHash::digest(CTD_BYTES),
        CTD_BYTES.len() as u64,
        vec![
            LineageRef::versioned(id('D'), version(1)),
            LineageRef::new(
                id('R'),
                Some(version(1)),
                Some(ctd_facts.rule_pack().content_hash().clone()),
            )
            .expect("CTD RulePack lineage is valid"),
            LineageRef::content_addressed(id('E'), ctd_facts.snapshot().content_hash().clone()),
        ],
    )
    .expect("CTD Artifact is valid");
    vec![
        (target, TARGET_BYTES.to_vec()),
        (delivery, DELIVERY_BYTES.to_vec()),
        (ctd, CTD_BYTES.to_vec()),
    ]
}

fn quote(instrument: char, coefficient: &str, scale: u32) -> CanonicalQuote {
    let price = fixed(coefficient, scale);
    CanonicalQuote::new(
        VersionRef::new(id(instrument), version(1)),
        time(20, 4),
        time(20, 6),
        date(2026, 7, 20),
        Some(price),
        Some(price),
        unit_ref('P'),
    )
}

fn analytics_ref(value: &DefinitionValue) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(
            Ulid::new(value.identity()).expect("definition identity is a ULID"),
            Version::new(value.version()).expect("definition version is nonzero"),
        ),
        definition_content_hash(value),
    )
}

fn unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, false)
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('0'), id('1'))
}

fn proto_owner() -> ProtoOwnerRef {
    ProtoOwnerRef {
        tenant_id: Some(proto_ulid('0')),
        owner_id: Some(proto_ulid('1')),
    }
}

fn id(suffix: char) -> Ulid {
    if suffix == 'Z' {
        return Ulid::new("01K2CGBVAT0000000000000000").expect("authority Unit ULID is valid");
    }
    Ulid::new(format!("0000000000000000000000000{}", ulid_char(suffix)))
        .expect("fixture ULID is valid")
}

fn proto_ulid(suffix: char) -> ProtoUlid {
    ProtoUlid {
        value: id(suffix).as_str().to_owned(),
    }
}

const fn ulid_char(suffix: char) -> char {
    match suffix {
        'I' => 'N',
        'L' => '2',
        'O' => '4',
        'U' => '3',
        value => value,
    }
}

fn version(value: u64) -> Version {
    Version::new(value).expect("fixture version is nonzero")
}

fn unit_ref(suffix: char) -> UnitRef {
    UnitRef::new(id(suffix), version(1))
}

fn unit_requirements() -> Vec<RatesUnitRequirement> {
    [
        ('M', "currency_amount"),
        ('P', "price_per_100"),
        ('Z', "rate"),
        ('Y', "years"),
        ('X', "years_squared"),
        ('V', "dv01_per_100"),
        ('W', "dv01"),
        ('I', "dimensionless"),
        ('H', "contract_count"),
    ]
    .into_iter()
    .map(|(suffix, dimension)| RatesUnitRequirement::new(unit_ref(suffix), dimension))
    .collect()
}

fn proto_unit(suffix: char) -> ProtoUnitRef {
    ProtoUnitRef {
        unit_id: Some(proto_ulid(suffix)),
        version: 1,
    }
}

fn domain_decimal(coefficient: &str, scale: u32, unit: char) -> DomainDecimalValue {
    DomainDecimalValue::new(coefficient, scale, unit_ref(unit))
        .expect("fixture DecimalValue is valid")
}

fn proto_decimal(coefficient: &str, scale: u32, unit: char) -> DecimalValue {
    DecimalValue {
        coefficient: coefficient.to_owned(),
        scale,
        unit: Some(proto_unit(unit)),
    }
}

fn proto_decimal_value(value: &DecimalValue) -> ExactDecimal {
    ExactDecimal::from_i128_with_scale(
        value
            .coefficient
            .parse()
            .expect("response Decimal coefficient is valid"),
        value.scale,
    )
}

fn oracle_decimal(value: &str) -> ExactDecimal {
    value
        .parse()
        .expect("independent R5E Oracle Decimal is valid")
}

fn oracle_bond_case<'a>(document: &'a str, case_id: &str) -> &'a str {
    oracle_object(document, "case_id", case_id)
}

fn oracle_basket<'a>(document: &'a str, basket_id: &str) -> &'a str {
    oracle_object(document, "basket_id", basket_id)
}

fn oracle_candidate<'a>(document: &'a str, basket_id: &str, bond_id: &str) -> &'a str {
    oracle_object(oracle_basket(document, basket_id), "bond_id", bond_id)
}

fn oracle_object<'a>(document: &'a str, identity_field: &str, identity: &str) -> &'a str {
    let marker = format!("\"{identity_field}\": \"{identity}\"");
    let identity_index = document
        .find(&marker)
        .unwrap_or_else(|| panic!("R5E Oracle object {identity_field}={identity} exists"));
    let start = document[..identity_index]
        .rfind('{')
        .expect("R5E Oracle object has an opening brace");
    let mut depth = 0_u32;
    let mut quoted = false;
    let mut escaped = false;
    for (offset, byte) in document.as_bytes()[start..].iter().copied().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
            continue;
        }
        match byte {
            b'"' => quoted = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth
                    .checked_sub(1)
                    .expect("R5E Oracle braces are balanced");
                if depth == 0 {
                    return &document[start..=start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("R5E Oracle object {identity_field}={identity} has a closing brace")
}

fn oracle_string<'a>(object: &'a str, field: &str) -> &'a str {
    let marker = format!("\"{field}\": \"");
    let start = object
        .find(&marker)
        .unwrap_or_else(|| panic!("R5E Oracle field {field} exists"))
        + marker.len();
    let tail = &object[start..];
    let end = tail
        .find('"')
        .unwrap_or_else(|| panic!("R5E Oracle field {field} is a string"));
    &tail[..end]
}

fn oracle_fixed(value: &str, field: &str) -> FixedDecimal {
    let decimal = oracle_decimal(oracle_string(value, field));
    assert!(
        decimal.scale() <= 12,
        "R5E Oracle FixedDecimal scale must not exceed 12",
    );
    let scaled = decimal
        .mantissa()
        .checked_mul(
            10_i128
                .checked_pow(12 - decimal.scale())
                .expect("R5E Oracle scale factor is representable"),
        )
        .expect("R5E Oracle Decimal is representable");
    FixedDecimal::from_scaled(scaled)
}

fn oracle_fixture_bond_id(value: &str) -> String {
    match value {
        "CGB-EXEMPT" => id('B').as_str().to_owned(),
        "CGB-TAXABLE" => id('D').as_str().to_owned(),
        other => panic!("unexpected R5E Oracle Bond id {other}"),
    }
}

fn selected_candidate_bond_id(
    candidate: &ficant_contracts::ficant::rates::v1::FuturesDeliveryCandidateResult,
) -> &str {
    candidate
        .bond
        .as_ref()
        .expect("candidate Bond")
        .object
        .as_ref()
        .expect("candidate Bond version")
        .id
        .as_ref()
        .expect("candidate Bond id")
        .value
        .as_str()
}

fn selected_bond_id(
    result: &ficant_contracts::ficant::rates::v1::AnalyzeFuturesDeliveryResult,
    index: u32,
) -> &str {
    selected_candidate_bond_id(
        &result.candidates[usize::try_from(index).expect("CTD index fits usize")],
    )
}

fn fixed(coefficient: &str, scale: u32) -> FixedDecimal {
    let coefficient = coefficient
        .parse::<i128>()
        .expect("fixture coefficient is valid");
    let factor = 10_i128
        .checked_pow(12 - scale)
        .expect("fixture scale is supported");
    FixedDecimal::from_scaled(
        coefficient
            .checked_mul(factor)
            .expect("fixture Decimal does not overflow"),
    )
}

fn time(day: u32, hour: u32) -> MarketTime {
    market_time(2026, 7, day, hour)
}

fn market_time(year: i32, month: u32, day: u32, hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
            .single()
            .expect("fixture UTC instant is valid"),
        "Asia/Shanghai",
        date(year, month, day),
    )
    .expect("fixture MarketTime is valid")
}

fn authority_year_start(year: i32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year - 1, 12, 31, 16, 0, 0)
            .single()
            .expect("authority effective instant is valid"),
        "Asia/Shanghai",
        date(year, 1, 1),
    )
    .expect("authority effective MarketTime is valid")
}

fn proto_time(day: u32, hour: u32) -> ProtoMarketTime {
    let value = time(day, hour);
    ProtoMarketTime {
        instant: Some(prost_types::Timestamp {
            seconds: value.instant().timestamp(),
            nanos: i32::try_from(value.instant().timestamp_subsec_nanos())
                .expect("nanoseconds fit i32"),
        }),
        market_timezone: value.market_timezone().to_owned(),
        local_trading_date: value.local_trading_date().to_string(),
    }
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("fixture date is valid")
}

fn object_binding(suffix: char, version: u64, hash: &ContentHash) -> ObjectBinding {
    ObjectBinding {
        object: Some(ProtoVersionRef {
            id: Some(proto_ulid(suffix)),
            version,
        }),
        content_hash: Some(proto_hash(hash)),
    }
}

fn snapshot_binding(suffix: char, hash: &ContentHash) -> SnapshotBinding {
    SnapshotBinding {
        snapshot_id: Some(proto_ulid(suffix)),
        content_hash: Some(proto_hash(hash)),
    }
}

fn proto_hash(hash: &ContentHash) -> Sha256 {
    Sha256 {
        value: hash.as_bytes().to_vec(),
    }
}

struct CanonicalBytes {
    bytes: Vec<u8>,
}

impl CanonicalBytes {
    fn new(schema: &str) -> Self {
        let mut value = Self {
            bytes: Vec::with_capacity(256),
        };
        value.bytes.extend_from_slice(b"FCMD");
        value.bytes.extend_from_slice(&1_u16.to_be_bytes());
        value.field(1, schema.as_bytes());
        value
    }

    fn field(&mut self, tag: u8, bytes: &[u8]) -> &mut Self {
        self.bytes.push(tag);
        self.bytes.extend_from_slice(
            &u64::try_from(bytes.len())
                .expect("canonical field length fits u64")
                .to_be_bytes(),
        );
        self.bytes.extend_from_slice(bytes);
        self
    }

    fn u64(&mut self, tag: u8, value: u64) -> &mut Self {
        self.field(tag, &value.to_be_bytes())
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

fn definition_content_hash(value: &DefinitionValue) -> ContentHash {
    let bytes = match value {
        DefinitionValue::Unit(value) => unit_bytes(value),
        DefinitionValue::Calendar(value) => calendar_bytes(value),
        DefinitionValue::MarketRulePack(value) => rule_pack_bytes(value),
        DefinitionValue::Instrument(value) => instrument_definition_bytes(value),
    };
    ContentHash::digest(&bytes)
}

fn owner_bytes(value: &OwnerRef) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("owner-ref/v1");
    bytes.field(2, value.tenant_id().as_str().as_bytes());
    bytes.field(3, value.owner_id().as_str().as_bytes());
    bytes.finish()
}

fn version_ref_bytes(value: &VersionRef) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("version-ref/v1");
    bytes.field(2, value.id().as_str().as_bytes());
    bytes.u64(3, value.version().get());
    bytes.finish()
}

fn unit_ref_bytes(value: &UnitRef) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("unit-ref/v1");
    bytes.field(2, value.unit_id().as_str().as_bytes());
    bytes.u64(3, value.version().get());
    bytes.finish()
}

fn decimal_bytes(value: &DomainDecimalValue) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("decimal/v1");
    bytes.field(2, value.coefficient().as_bytes());
    bytes.u64(3, u64::from(value.scale()));
    bytes.field(4, &unit_ref_bytes(value.unit()));
    bytes.finish()
}

fn market_time_bytes(value: &MarketTime) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("market-time/v1");
    bytes.field(2, &value.instant().timestamp().to_be_bytes());
    bytes.field(3, &value.instant().timestamp_subsec_nanos().to_be_bytes());
    bytes.field(4, value.market_timezone().as_bytes());
    bytes.field(5, value.local_trading_date().to_string().as_bytes());
    bytes.finish()
}

fn effective_period_bytes(value: &EffectivePeriod) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("effective-period/v1");
    bytes.field(2, &market_time_bytes(value.from()));
    bytes.field(3, &market_time_bytes(value.to()));
    bytes.finish()
}

fn unit_bytes(value: &Unit) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("definition/unit/v1");
    bytes.field(2, value.identity().as_bytes());
    bytes.u64(3, value.version());
    bytes.field(4, &owner_bytes(value.owner()));
    bytes.field(5, value.code().as_bytes());
    bytes.field(6, value.dimension().as_bytes());
    bytes.u64(7, u64::from(value.scale()));
    bytes.u64(8, u64::from(value.precision()));
    bytes.finish()
}

fn calendar_bytes(value: &Calendar) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("definition/calendar/v1");
    bytes.field(2, value.identity().as_bytes());
    bytes.u64(3, value.version());
    bytes.field(4, &owner_bytes(value.owner()));
    bytes.field(5, value.market().as_bytes());
    bytes.field(6, value.market_timezone().as_bytes());
    bytes.field(7, &effective_period_bytes(value.effective()));
    bytes.u64(
        8,
        u64::try_from(value.sessions().len()).expect("session count fits u64"),
    );
    for session in value.sessions() {
        let mut encoded = CanonicalBytes::new("calendar-session/v1");
        encoded.field(2, session.local_date().to_string().as_bytes());
        encoded.field(
            3,
            session
                .open_local_time()
                .map_or_else(|| "-".to_owned(), |time| time.to_string())
                .as_bytes(),
        );
        encoded.field(
            4,
            session
                .close_local_time()
                .map_or_else(|| "-".to_owned(), |time| time.to_string())
                .as_bytes(),
        );
        bytes.field(9, &encoded.finish());
    }
    bytes.finish()
}

fn rule_pack_bytes(value: &MarketRulePack) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("definition/rule-pack/v1");
    bytes.field(2, value.identity().as_bytes());
    bytes.u64(3, value.version());
    bytes.field(4, &owner_bytes(value.owner()));
    bytes.field(5, value.market().as_bytes());
    bytes.field(6, value.rule_type().as_bytes());
    bytes.field(7, value.source().as_bytes());
    bytes.field(8, &effective_period_bytes(value.effective()));
    bytes.field(
        9,
        &[match value.verification_status() {
            VerificationStatus::Unverified => 1,
            VerificationStatus::Verified => 2,
            VerificationStatus::Rejected => 3,
        }],
    );
    bytes.field(10, value.content_hash().as_bytes());
    bytes.finish()
}

fn instrument_definition_bytes(value: &InstrumentDefinition) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("definition/instrument-aggregate/v1");
    bytes.field(2, &instrument_bytes(value.instrument()));
    match value.subtype() {
        Some(InstrumentSubtype::Bond(bond)) => {
            bytes.field(3, &[1]);
            bytes.field(4, &bond_bytes(bond));
        }
        Some(InstrumentSubtype::FuturesContract(contract)) => {
            bytes.field(3, &[2]);
            bytes.field(4, &futures_contract_bytes(contract));
        }
        None => {
            bytes.field(3, &[0]);
        }
    }
    bytes.finish()
}

fn instrument_bytes(value: &Instrument) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("definition/instrument/v1");
    bytes.field(2, value.id().as_str().as_bytes());
    bytes.u64(3, value.version());
    bytes.field(4, &owner_bytes(value.owner()));
    bytes.field(
        5,
        &[match value.kind() {
            InstrumentKind::Bond => 1,
            InstrumentKind::Futures => 2,
            InstrumentKind::Other => 3,
        }],
    );
    bytes.field(6, value.market().as_bytes());
    bytes.field(7, value.symbol().as_bytes());
    bytes.field(8, &unit_ref_bytes(value.currency()));
    bytes.field(9, &version_ref_bytes(value.calendar()));
    bytes.finish()
}

fn bond_bytes(value: &Bond) -> Vec<u8> {
    let pricing = value
        .pricing_terms()
        .expect("R5D Bond definition has exact pricing terms");
    let mut bytes = CanonicalBytes::new("definition/bond/v3");
    bytes.field(2, &version_ref_bytes(value.instrument()));
    bytes.field(3, value.first_issue_date().to_string().as_bytes());
    bytes.field(4, value.current_issue_date().to_string().as_bytes());
    bytes.field(5, value.maturity_date().to_string().as_bytes());
    bytes.field(6, &decimal_bytes(value.cumulative_issued_amount()));
    let tax = value
        .tax_attributes()
        .expect("R5D Bond definition has exact tax attributes");
    bytes.field(
        7,
        &[
            match tax.value_added_tax_status() {
                ValueAddedTaxStatus::Exempt => 1,
                ValueAddedTaxStatus::Taxable => 2,
            },
            match tax.income_tax_status() {
                IncomeTaxStatus::Exempt => 1,
                IncomeTaxStatus::Taxable => 2,
            },
        ],
    );
    bytes.field(8, &decimal_bytes(value.face_value()));
    bytes.field(9, &decimal_bytes(pricing.coupon_rate()));
    bytes.field(
        10,
        &[match pricing.frequency() {
            BondCouponFrequency::Annual => 1,
            BondCouponFrequency::Semiannual => 2,
        }],
    );
    bytes.field(
        11,
        &[match pricing.day_count() {
            BondDayCountConvention::ActActBondIsma => 1,
        }],
    );
    bytes.field(
        12,
        &[match pricing.business_day() {
            BondBusinessDayConvention::Following => 1,
        }],
    );
    bytes.finish()
}

fn futures_contract_bytes(value: &FuturesContract) -> Vec<u8> {
    let mut bytes = CanonicalBytes::new("definition/futures/v1");
    bytes.field(2, &version_ref_bytes(value.instrument()));
    bytes.field(3, &market_time_bytes(value.last_trade_time()));
    bytes.field(4, &market_time_bytes(value.expiry_time()));
    bytes.field(5, &market_time_bytes(value.settlement_time()));
    bytes.field(6, &decimal_bytes(value.multiplier()));
    bytes.field(7, &version_ref_bytes(value.rule_pack()));
    if let (Some(product_code), Some(price_unit)) = (value.product_code(), value.price_unit()) {
        bytes.field(8, product_code.as_bytes());
        bytes.field(9, &unit_ref_bytes(price_unit));
    }
    bytes.finish()
}
