use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use ficant_api::{
    FormalOutputPublisher, GrpcWebServerConfig, PlatformApplication, PlatformGrpcService,
    PlatformPort, RatesGrpcService, SessionPolicy, SystemClock, TrustedIdentity,
    serve_grpc_web_routes,
};
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, ApplicationResult, ArtifactRepository,
    BondAnalyticsArtifactCodec, BondAnalyticsArtifactFacts, CanonicalQuote,
    CanonicalSnapshotDecoder, CurvePointSetDecoder, CurveSnapshotMetadata,
    CurveSnapshotMetadataRepository, DataSourceRepository, DecodedCanonicalQuotes,
    DecodedCurvePoint, DecodedCurvePointSet, DefinitionIdentity, DefinitionRepository,
    DefinitionValue, EncodedBondAnalyticsArtifact, EncodedFuturesDeliveryArtifact,
    FactorTopologyRepository, FormalOutputRecord, FormalOutputRepository,
    FuturesDeliveryArtifactCandidateFacts, FuturesDeliveryArtifactCodec,
    FuturesDeliveryArtifactFacts, IdempotencyKey, InstrumentDefinition, InstrumentSubtype,
    IntegrityEvent, IntegrityEventSink, PublishArtifact, RegisterDataSource,
    RequiredVerifiedBlobRead, SnapshotVerifiedReadMetadata, SnapshotVerifiedReadMetadataRepository,
    SubjectRepository, VerifiedBlobPayload, VerifiedBlobReader, VerifiedBlobRole,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, BOND_ANALYTICS_MEDIA_TYPE,
    FUTURES_DELIVERY_MEDIA_TYPE, rates_data_source_content_hash,
};
use ficant_cgb_futures_pack::{CgbFuturesDeliveryRulePackParser, MARKET, RULE_TYPE, TYPE_URL};
use ficant_contracts::ficant::app::v1::platform_service_server::PlatformServiceServer;
use ficant_contracts::ficant::core::v1::{
    DecimalValue, FundingTier as ProtoFundingTier, Ulid as ProtoUlid, UnitRef as ProtoUnitRef,
};
use ficant_contracts::ficant::market::v1::{
    BondCouponTaxRule, BondTaxAttributes, FundingRulePack, FundingTierRate,
    IncomeTaxStatus as ProtoIncomeTaxStatus, SubjectCouponTaxRate, TaxRulePack,
    ValueAddedTaxStatus as ProtoValueAddedTaxStatus,
};
use ficant_contracts::ficant::rates::v1::rates_analytics_service_server::RatesAnalyticsServiceServer;
use ficant_domain::analytics::{
    AnalyticsError, AnalyticsObjectRef, BondAnalyticsInput, BondAnalyticsResult, FixedDecimal,
};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliverableInput, FuturesDeliveryBasketResult,
};
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::{
    ArtifactInputKind, Bond, BondBusinessDayConvention, BondCouponFrequency,
    BondDayCountConvention, BondPricingTerms, BondTaxAttributes as DomainBondTaxAttributes,
    Calendar, CalendarInput, CalendarSession, CurveSnapshot, CurveSnapshotInput, DataSource,
    DataSourceInput, DataSourceKind, FuturesContract, IncomeTaxStatus, Instrument, InstrumentInput,
    InstrumentKind, MarketRulePack, MarketRulePackInput, PriceSourceType, RulePackContent, Unit,
    UnitInput, ValueAddedTaxStatus, VerificationStatus,
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
    MARKET as TAX_MARKET, RULE_TYPE as TAX_RULE_TYPE, TYPE_URL as TAX_TYPE_URL, TaxRulePackV1Parser,
};
use prost::Message;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use tonic::service::RoutesBuilder;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";
const TOKEN: &str = "phase2e-python-sdk-test-token";
const CGB_FUTURES_PACK: &[u8] =
    include_bytes!("../../../domain-packs/cgb-futures/cgb-futures-v2.bin");
const BOND_BYTES: &[u8] = b"phase2e-bond-snapshot";
const CURVE_DATA_BYTES: &[u8] = b"phase2e-curve-data-snapshot";
const DELIVERY_BYTES: &[u8] = b"phase2e-delivery-snapshot";
const MANIFEST_BYTES: &[u8] = b"phase2e-manifest";
const CURVE_BYTES: &[u8] = b"phase2e-curve-points";
const TARGET_BYTES: &[u8] = b"phase2e-target-risk";
const DELIVERY_ARTIFACT_BYTES: &[u8] = b"phase2e-delivery-artifact";
const CTD_BYTES: &[u8] = b"phase2e-ctd-risk";
const CURVE_FAMILY: &str = "phase2e.cn.gov.yield-curve";

#[derive(Clone)]
struct FixtureDefinitions {
    values: Vec<DefinitionValue>,
}

#[tonic::async_trait]
impl DefinitionRepository for FixtureDefinitions {
    async fn create_identity(&self, _: DefinitionIdentity) -> Result<(), ApplicationError> {
        Err(storage_unavailable())
    }

    async fn append_version(
        &self,
        _: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        Err(storage_unavailable())
    }

    async fn get_version(
        &self,
        _: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        let found = self
            .values
            .iter()
            .find(|value| {
                value.identity() == definition_id.as_str() && value.version() == version.get()
            })
            .cloned();
        Ok(found)
    }

    async fn resolve_as_of(
        &self,
        _: &AccessScope,
        _: Ulid,
        _: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Err(storage_unavailable())
    }
}

#[derive(Clone)]
struct FixtureSnapshots {
    values: Vec<DataSnapshot>,
}

#[tonic::async_trait]
impl SnapshotVerifiedReadMetadataRepository for FixtureSnapshots {
    async fn get_verified_read_metadata(
        &self,
        _: &AccessScope,
        snapshot_id: Ulid,
    ) -> Result<Option<SnapshotVerifiedReadMetadata>, ApplicationError> {
        let Some(snapshot) = self.values.iter().find(|value| value.id() == &snapshot_id) else {
            return Ok(None);
        };
        let blob_size = match snapshot_id.as_str().chars().last() {
            Some('M') => BOND_BYTES.len(),
            Some('5') => CURVE_DATA_BYTES.len(),
            Some('Y') => DELIVERY_BYTES.len(),
            _ => unreachable!("fixture exposes only R5D DataSnapshots M, 5 and Y"),
        };
        SnapshotVerifiedReadMetadata::data(
            snapshot.clone(),
            u64::try_from(blob_size).expect("fixture size fits u64"),
            u64::try_from(MANIFEST_BYTES.len()).expect("fixture size fits u64"),
        )
        .map(Some)
    }
}

#[derive(Clone)]
struct FixtureBlobs {
    artifacts: Vec<(Artifact, Vec<u8>)>,
}

#[tonic::async_trait]
impl VerifiedBlobReader for FixtureBlobs {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> Result<VerifiedBlobPayload, ApplicationError> {
        let bytes = match request.blob_role() {
            VerifiedBlobRole::DataParquet => match request.resource_id().as_str().chars().last() {
                Some('M') => BOND_BYTES,
                Some('5') => CURVE_DATA_BYTES,
                Some('Y') => DELIVERY_BYTES,
                _ => unreachable!("fixture exposes only R5D DataSnapshots M, 5 and Y"),
            },
            VerifiedBlobRole::DataManifest => MANIFEST_BYTES,
            VerifiedBlobRole::CurvePoints => CURVE_BYTES,
            VerifiedBlobRole::ArtifactPayload => self
                .artifacts
                .iter()
                .find(|(artifact, _)| artifact.id() == request.resource_id())
                .map(|(_, bytes)| bytes.as_slice())
                .expect("fixture Artifact payload exists"),
            _ => unreachable!("Rates reads only DataSnapshot, CurveSnapshot and Artifact blobs"),
        };
        request.verify_bytes(sink, bytes.to_vec()).await
    }
}

#[derive(Clone, Copy)]
struct FixtureIntegrityEvents;

#[tonic::async_trait]
impl IntegrityEventSink for FixtureIntegrityEvents {
    async fn emit(&self, _: IntegrityEvent) -> Result<(), ApplicationError> {
        panic!("fixture payload hashes and sizes are exact")
    }
}

#[derive(Clone, Copy)]
struct FixtureFormalOutputs;

#[tonic::async_trait]
impl FormalOutputRepository for FixtureFormalOutputs {
    async fn publish(
        &self,
        scope: &AccessScope,
        record: FormalOutputRecord,
    ) -> ApplicationResult<FormalOutputRecord> {
        scope.authorize(record.owner())?;
        record.verify()?;
        Ok(record)
    }

    async fn get(
        &self,
        _: &AccessScope,
        _: &ContentHash,
    ) -> ApplicationResult<Option<FormalOutputRecord>> {
        Ok(None)
    }
}

fn formal_output_publisher() -> FormalOutputPublisher {
    FormalOutputPublisher::new(
        Arc::new(FixtureFormalOutputs),
        CodeBinding::new(
            "34402344c7d2c9238dc171af52ac4db77eb6b462",
            "f66e03c55703837d6f2aee9959eba482612272f1",
        )
        .expect("fixture Code binding is valid"),
        RuntimeBinding::new(
            ContentHash::digest(b"phase2e-server-image"),
            ContentHash::digest(b"phase2e-server-environment"),
        ),
    )
}

#[derive(Clone)]
struct FixtureCanonicalSnapshotDecoder {
    source: VersionRef,
}

#[tonic::async_trait]
impl CanonicalSnapshotDecoder for FixtureCanonicalSnapshotDecoder {
    async fn decode_quotes(
        &self,
        snapshot: &DataSnapshot,
        parquet: &[u8],
        manifest: &[u8],
    ) -> Result<DecodedCanonicalQuotes, ApplicationError> {
        assert_eq!(snapshot.id(), &id('Y'));
        assert_eq!(parquet, DELIVERY_BYTES);
        assert_eq!(manifest, MANIFEST_BYTES);
        DecodedCanonicalQuotes::new(
            self.source.clone(),
            vec![
                canonical_quote('Z', "995", 1),
                canonical_quote('2', "102", 0),
                canonical_quote('3', "100", 0),
                canonical_quote('4', "100", 0),
            ],
        )
    }
}

#[derive(Clone)]
struct FixtureSubjects {
    value: SubjectRecord,
}

#[tonic::async_trait]
impl SubjectRepository for FixtureSubjects {
    async fn register_subject(&self, _: SubjectRecord) -> Result<SubjectRecord, ApplicationError> {
        Err(storage_unavailable())
    }

    async fn get_subject(
        &self,
        reference: ficant_domain::primitives::VersionRef,
    ) -> Result<Option<SubjectRecord>, ApplicationError> {
        Ok((self.value.version().reference() == &reference).then(|| self.value.clone()))
    }

    async fn register_subject_state(
        &self,
        _: SubjectStateSnapshot,
    ) -> Result<SubjectStateSnapshot, ApplicationError> {
        Err(storage_unavailable())
    }

    async fn get_subject_state(
        &self,
        _: Ulid,
        _: chrono::DateTime<Utc>,
    ) -> Result<Option<SubjectStateSnapshot>, ApplicationError> {
        Err(storage_unavailable())
    }
}

#[derive(Clone)]
struct FixtureDataSource {
    value: DataSource,
}

#[tonic::async_trait]
impl DataSourceRepository for FixtureDataSource {
    async fn register(&self, _: RegisterDataSource) -> ApplicationResult<DataSource> {
        Err(storage_unavailable())
    }

    async fn get_exact(
        &self,
        _: &AccessScope,
        reference: VersionRef,
    ) -> ApplicationResult<Option<DataSource>> {
        Ok(
            (self.value.id() == reference.id()
                && self.value.version() == reference.version().get())
            .then(|| self.value.clone()),
        )
    }
}

#[derive(Clone)]
struct FixtureCurve {
    snapshot: CurveSnapshot,
    points: DecodedCurvePointSet,
    nodes: Vec<CurveNodeDefinition>,
}

#[tonic::async_trait]
impl CurveSnapshotMetadataRepository for FixtureCurve {
    async fn get_curve_snapshot_metadata(
        &self,
        _: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> ApplicationResult<Option<CurveSnapshotMetadata>> {
        Ok((self.snapshot.id() == &curve_snapshot_id).then(|| {
            CurveSnapshotMetadata::new(
                self.snapshot.clone(),
                u64::try_from(CURVE_BYTES.len()).expect("fixture size fits u64"),
            )
            .expect("fixture CurveSnapshot metadata is valid")
        }))
    }
}

impl CurvePointSetDecoder for FixtureCurve {
    fn decode_canonical(&self, bytes: &[u8]) -> ApplicationResult<DecodedCurvePointSet> {
        assert_eq!(bytes, CURVE_BYTES);
        Ok(self.points.clone())
    }
}

#[tonic::async_trait]
impl FactorTopologyRepository for FixtureCurve {
    async fn register_factor_definition(
        &self,
        _: &AccessScope,
        _: FactorDefinition,
        _: IdempotencyKey,
    ) -> ApplicationResult<FactorDefinition> {
        Err(storage_unavailable())
    }

    async fn register_curve_node_definition(
        &self,
        _: &AccessScope,
        _: CurveNodeDefinition,
        _: IdempotencyKey,
    ) -> ApplicationResult<CurveNodeDefinition> {
        Err(storage_unavailable())
    }

    async fn bind_factor_target(
        &self,
        _: &AccessScope,
        _: FactorTargetBinding,
        _: IdempotencyKey,
    ) -> ApplicationResult<FactorTargetBinding> {
        Err(storage_unavailable())
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
        Err(storage_unavailable())
    }

    async fn get_target_factors(
        &self,
        _: &AccessScope,
        _: &FactorTarget,
    ) -> ApplicationResult<Vec<FactorDefinition>> {
        Err(storage_unavailable())
    }

    async fn exact_target_exists(&self, _: &FactorTarget) -> ApplicationResult<bool> {
        Ok(false)
    }
}

#[derive(Clone)]
struct FixtureArtifacts {
    values: Vec<(Artifact, Vec<u8>)>,
    target: BondAnalyticsArtifactFacts,
    delivery: FuturesDeliveryArtifactFacts,
    ctd: BondAnalyticsArtifactFacts,
}

#[tonic::async_trait]
impl ArtifactRepository for FixtureArtifacts {
    async fn publish_verified_blob(&self, _: PublishArtifact) -> ApplicationResult<Artifact> {
        Err(storage_unavailable())
    }

    async fn get_metadata(
        &self,
        _: &AccessScope,
        artifact_id: Ulid,
    ) -> ApplicationResult<Option<Artifact>> {
        Ok(self
            .values
            .iter()
            .find(|(artifact, _)| artifact.id() == &artifact_id)
            .map(|(artifact, _)| artifact.clone()))
    }
}

impl BondAnalyticsArtifactCodec for FixtureArtifacts {
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
            TARGET_BYTES => Ok(self.target.clone()),
            CTD_BYTES => Ok(self.ctd.clone()),
            _ => Err(AnalyticsError::InvalidInput),
        }
    }
}

impl FuturesDeliveryArtifactCodec for FixtureArtifacts {
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
        if bytes == DELIVERY_ARTIFACT_BYTES {
            Ok(self.delivery.clone())
        } else {
            Err(AnalyticsError::InvalidInput)
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "invoked only by scripts/check-phase2e-sdk.ps1"]
#[allow(clippy::too_many_lines)]
async fn python_sdk_matches_phase2_reference_slices_through_live_rule_pack_composition() {
    let address = free_loopback_address();
    let application = application();
    let platform = PlatformGrpcService::new(Arc::clone(&application), KEY)
        .expect("fixture platform service is valid");
    let fixture = Fixture::new();
    let definitions = Arc::new(FixtureDefinitions {
        values: fixture.definitions.clone(),
    });
    let subjects = Arc::new(FixtureSubjects {
        value: fixture_subject(),
    });
    let snapshots = Arc::new(FixtureSnapshots {
        values: fixture.snapshots.clone(),
    });
    let blobs = Arc::new(FixtureBlobs {
        artifacts: fixture.artifacts.values.clone(),
    });
    let data_source = Arc::new(FixtureDataSource {
        value: fixture.source.clone(),
    });
    let curve = Arc::new(fixture.curve.clone());
    let artifacts = Arc::new(fixture.artifacts.clone());
    let decoder = Arc::new(FixtureCanonicalSnapshotDecoder {
        source: VersionRef::new(id('6'), fixture_version()),
    });
    let rates = RatesGrpcService::new_with_formal_materialization(
        application,
        Arc::new(NativeBondAnalyticsEngine),
        Arc::new(NativeYieldCurveEngine),
        Arc::new(NativeCarryRollEngine),
        Arc::new(NativeFuturesDeliveryEngine),
        definitions,
        subjects,
        Arc::new(CgbFuturesDeliveryRulePackParser),
        snapshots,
        blobs,
        Arc::new(FixtureIntegrityEvents),
        decoder,
        Arc::new(FundingRulePackV1Parser),
        Arc::new(TaxRulePackV1Parser),
        Arc::new(NativeFuturesHedgeEngine),
        data_source,
        curve.clone(),
        curve.clone(),
        artifacts.clone(),
        curve,
        artifacts.clone(),
        artifacts,
        formal_output_publisher(),
        KEY,
    )
    .expect("fixture rates service is valid");
    let server = tokio::spawn(async move {
        let mut routes = RoutesBuilder::default();
        routes.add_service(PlatformServiceServer::new(platform));
        routes.add_service(RatesAnalyticsServiceServer::new(rates));
        serve_grpc_web_routes(
            GrpcWebServerConfig {
                bind: address,
                allowed_origins: vec!["http://127.0.0.1:4174".to_owned()],
            },
            routes.routes(),
        )
        .await
    });

    let endpoint = address.to_string();
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root exists");
    let binding_environment = fixture.binding_environment();
    let outcome = tokio::task::spawn_blocking(move || {
        let mut command = Command::new("uv");
        for (key, value) in binding_environment {
            command.env(key, value);
        }
        command
            .args([
                "run",
                "--offline",
                "--locked",
                "--project",
                "python",
                "python",
                "-m",
                "pytest",
                "python/tests/test_rates_sdk_live.py",
                "-q",
            ])
            .current_dir(repository_root)
            .env("FICANT_PHASE2E_ENDPOINT", endpoint)
            .env_remove("FICANT_PHASE2E_SERVER_BIN")
            .output()
            .expect("uv must be available to run the Phase 2E SDK check")
    })
    .await
    .expect("Phase 2E SDK process must join");
    server.abort();
    let _ = server.await;

    assert!(
        outcome.status.success(),
        "Phase 2E Python SDK parity failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&outcome.stdout),
        String::from_utf8_lossy(&outcome.stderr),
    );
}

fn application() -> Arc<dyn PlatformPort> {
    let identity = TrustedIdentity::bearer(
        "phase2e-sdk-test",
        TOKEN.as_bytes(),
        id('A'),
        id('0'),
        vec![id('1')],
        PlatformRole::Researcher,
        ["rates:analyze"],
    )
    .expect("fixture bearer identity is valid");
    Arc::new(
        PlatformApplication::try_new(
            Arc::new(SystemClock),
            SessionPolicy::new(900, 60).expect("fixture session policy is valid"),
            KEY,
            vec![identity],
            None,
            Vec::new(),
        )
        .expect("fixture platform application is valid"),
    )
}

#[derive(Clone)]
struct Fixture {
    definitions: Vec<DefinitionValue>,
    snapshots: Vec<DataSnapshot>,
    source: DataSource,
    curve: FixtureCurve,
    artifacts: FixtureArtifacts,
}

impl Fixture {
    #[allow(clippy::too_many_lines)]
    fn new() -> Self {
        let source = DataSource::new(DataSourceInput {
            data_source_id: id('6'),
            version: fixture_version(),
            owner: fixture_owner(),
            kind: DataSourceKind::FileNdjson,
            name: "Phase 2E exact quote source".to_owned(),
            connection_binding: "phase2e-fixture".to_owned(),
            dataset: "rates_quotes".to_owned(),
            canonical_schema_id: "ficant.market.quote.canonical.v1".to_owned(),
            canonical_schema_hash: ContentHash::digest(b"phase2e-canonical-schema"),
        })
        .expect("fixture DataSource is valid")
        .with_price_source_type(PriceSourceType::ActiveQuote)
        .expect("fixture DataSource price role is valid");
        let source_lineage = LineageRef::new(
            source.id().clone(),
            Some(fixture_version()),
            Some(rates_data_source_content_hash(&source)),
        )
        .expect("fixture source lineage is valid");
        let bond_snapshot = data_snapshot('M', bond_valuation_time(), BOND_BYTES, &source_lineage);
        let curve_data_snapshot = data_snapshot(
            '5',
            curve_valuation_time(),
            CURVE_DATA_BYTES,
            &source_lineage,
        );
        let delivery_snapshot =
            data_snapshot('Y', valuation_time(), DELIVERY_BYTES, &source_lineage);

        let nodes = [
            ("phase2e.cn.gov.node.0166d", "P166D", "125", 4),
            ("phase2e.cn.gov.node.0366d", "P366D", "175", 4),
            ("phase2e.cn.gov.node.0531d", "P531D", "19", 3),
            ("phase2e.cn.gov.node.0897d", "P897D", "225", 4),
            ("phase2e.cn.gov.node.1461d", "P1461D", "3", 2),
        ]
        .into_iter()
        .map(|(node_id, tenor, coefficient, scale)| {
            let mut input = CurveNodeDefinitionInput {
                curve_node_id: node_id.to_owned(),
                curve_family_id: CURVE_FAMILY.to_owned(),
                tenor: tenor.to_owned(),
                factor_unit: unit_ref('C'),
                content_hash: ContentHash::digest(b"placeholder"),
            };
            input.content_hash = CurveNodeDefinition::content_hash_for(&input);
            (
                CurveNodeDefinition::new(input).expect("fixture CurveNode is valid"),
                coefficient,
                scale,
            )
        })
        .collect::<Vec<_>>();
        let points = DecodedCurvePointSet::new(
            CURVE_FAMILY,
            nodes
                .iter()
                .map(|(node, coefficient, scale)| {
                    DecodedCurvePoint::new(
                        node.curve_node_id(),
                        node.content_hash().clone(),
                        domain_decimal(coefficient, *scale, 'C'),
                    )
                    .expect("fixture curve point is valid")
                })
                .collect(),
        )
        .expect("fixture curve set is valid");
        let curve_snapshot = CurveSnapshot::new(CurveSnapshotInput {
            curve_snapshot_id: id('Q'),
            owner: fixture_owner(),
            as_of: curve_valuation_time(),
            currency: unit_ref('A'),
            curve_kind: "YTM".to_owned(),
            calendar: VersionRef::new(id('K'), fixture_version()),
            rule_pack: VersionRef::new(id('R'), fixture_version()),
            point_schema: ficant_application::ports::CURVE_POINT_SCHEMA.to_owned(),
            content_hash: ContentHash::digest(CURVE_BYTES),
            lineage: vec![
                source_lineage.clone(),
                LineageRef::content_addressed(
                    curve_data_snapshot.id().clone(),
                    curve_data_snapshot.content_hash().clone(),
                ),
            ],
            input_kind: ArtifactInputKind::ExternalFixture,
        })
        .expect("fixture CurveSnapshot is valid")
        .with_knowledge_time(curve_valuation_time(), CURVE_FAMILY)
        .expect("fixture CurveSnapshot knowledge time is valid");
        let curve = FixtureCurve {
            snapshot: curve_snapshot,
            points,
            nodes: nodes.into_iter().map(|(node, _, _)| node).collect(),
        };

        let units = unit_definitions();
        let calendar = calendar();
        let curve_rule = curve_rule_pack();
        let delivery_rule = frozen_cgb_futures_pack();
        let funding = synthetic_funding_pack();
        let tax = synthetic_tax_pack();
        let bond_n = bond_definition(
            'N',
            "260008.IB",
            date(2026, 4, 15),
            date(2031, 4, 15),
            "15",
            3,
            BondCouponFrequency::Annual,
        );
        let bond_w = bond_definition(
            'W',
            "phase2b-carry-bond",
            date(2026, 1, 1),
            date(2029, 1, 1),
            "2",
            2,
            BondCouponFrequency::Annual,
        );
        let bond_2 = delivery_bond_definition('2', "T-bond-expensive");
        let bond_3 = delivery_bond_definition('3', "T-bond-ctd");
        let bond_4 = delivery_bond_definition('4', "T-bond-tied-later");
        let contract_z = futures_contract_definition('Z', "T2609", "T");
        let contract_p = futures_contract_definition('P', "TS2609", "TS");
        let ctd_bond = analytics_ref(&bond_3);
        let contract_ref = analytics_ref(&contract_p);
        let delivery_rule_ref = AnalyticsObjectRef::new(
            VersionRef::new(id('X'), fixture_version()),
            delivery_rule.content_hash().clone(),
        );
        let snapshot_ref = AnalyticsObjectRef::new(
            VersionRef::new(id('Y'), fixture_version()),
            delivery_snapshot.content_hash().clone(),
        );
        let target = BondAnalyticsArtifactFacts::new(
            valuation_time(),
            ctd_bond.clone(),
            delivery_rule_ref.clone(),
            snapshot_ref.clone(),
            fixed_decimal("12345", 1),
        );
        let delivery = FuturesDeliveryArtifactFacts::new(
            valuation_time(),
            contract_ref.clone(),
            delivery_rule_ref.clone(),
            snapshot_ref.clone(),
            CgbFuturesProduct::TwoYear,
            vec![FuturesDeliveryArtifactCandidateFacts::new(
                ctd_bond.clone(),
                fixed_decimal("9987", 4),
            )],
            0,
        );
        let ctd = BondAnalyticsArtifactFacts::new(
            valuation_time(),
            ctd_bond.clone(),
            delivery_rule_ref.clone(),
            snapshot_ref.clone(),
            fixed_decimal("145", 4),
        );
        let artifact_values = artifact_fixtures(&ctd_bond, &contract_ref, &target, &delivery, &ctd);
        let artifacts = FixtureArtifacts {
            values: artifact_values,
            target,
            delivery,
            ctd,
        };
        let definitions = units
            .into_iter()
            .chain([
                DefinitionValue::Calendar(calendar),
                DefinitionValue::MarketRulePack(curve_rule),
                DefinitionValue::MarketRulePack(delivery_rule),
                DefinitionValue::MarketRulePack(funding),
                DefinitionValue::MarketRulePack(tax),
                bond_n,
                bond_w,
                bond_2,
                bond_3,
                bond_4,
                contract_z,
                contract_p,
            ])
            .collect();
        Self {
            definitions,
            snapshots: vec![bond_snapshot, curve_data_snapshot, delivery_snapshot],
            source,
            curve,
            artifacts,
        }
    }

    fn binding_environment(&self) -> Vec<(String, String)> {
        let mut values = Vec::new();
        for suffix in ['N', 'T', 'W', 'Z', 'V', 'P', 'K'] {
            let definition = self
                .definitions
                .iter()
                .find(|value| value.identity() == id(suffix).as_str())
                .expect("public fixture definition exists");
            values.push((
                format!("FICANT_PHASE2E_OBJECT_{suffix}_SHA256"),
                hex(definition_content_hash(definition).as_bytes()),
            ));
        }
        for suffix in ['M', 'Y'] {
            let snapshot = self
                .snapshots
                .iter()
                .find(|value| value.id() == &id(suffix))
                .expect("fixture DataSnapshot exists");
            values.push((
                format!("FICANT_PHASE2E_SNAPSHOT_{suffix}_SHA256"),
                hex(snapshot.content_hash().as_bytes()),
            ));
        }
        values.push((
            "FICANT_PHASE2E_SNAPSHOT_Q_SHA256".to_owned(),
            hex(self.curve.snapshot.content_hash().as_bytes()),
        ));
        for suffix in ['7', '8', '9'] {
            let artifact = self
                .artifacts
                .values
                .iter()
                .find(|(artifact, _)| artifact.id() == &id(suffix))
                .map(|(artifact, _)| artifact)
                .expect("fixture Artifact exists");
            values.push((
                format!("FICANT_PHASE2E_ARTIFACT_{suffix}_SHA256"),
                hex(artifact.content_hash().as_bytes()),
            ));
        }
        values
    }
}

fn data_snapshot(
    suffix: char,
    as_of: MarketTime,
    bytes: &[u8],
    source: &LineageRef,
) -> DataSnapshot {
    DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: id(suffix),
        owner: fixture_owner(),
        visible_at: as_of.clone(),
        as_of,
        schema_hash: ContentHash::digest(b"phase2e-canonical-schema"),
        manifest_hash: ContentHash::digest(MANIFEST_BYTES),
        blob_content_hash: ContentHash::digest(bytes),
        lineage: vec![source.clone()],
    })
    .expect("fixture DataSnapshot is valid")
}

fn frozen_cgb_futures_pack() -> MarketRulePack {
    let content = RulePackContent::new(TYPE_URL, CGB_FUTURES_PACK.to_vec())
        .expect("frozen CGB futures payload is valid");
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('X'),
            version: Version::new(1).expect("fixture version is valid"),
            owner: OwnerRef::new(id('0'), id('1')),
            market: MARKET.to_owned(),
            rule_type: RULE_TYPE.to_owned(),
            source: "phase2e-fixture".to_owned(),
            effective: EffectivePeriod::new(domain_time(2026, 1, 1), domain_time(2027, 1, 1))
                .expect("fixture effective period is valid"),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(content.value()),
        },
        content,
    )
    .expect("frozen CGB futures RulePack is valid")
}

fn synthetic_funding_pack() -> MarketRulePack {
    let content = RulePackContent::new(
        FUNDING_TYPE_URL,
        FundingRulePack {
            rates: vec![
                FundingTierRate {
                    funding_tier: ProtoFundingTier::DrAvailable as i32,
                    annual_financing_rate: Some(decimal("18", 3)),
                },
                FundingTierRate {
                    funding_tier: ProtoFundingTier::ROnly as i32,
                    annual_financing_rate: Some(decimal("25", 3)),
                },
            ],
        }
        .encode_to_vec(),
    )
    .expect("synthetic funding payload is valid");
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('V'),
            version: Version::new(1).expect("fixture version is valid"),
            owner: OwnerRef::new(id('0'), id('1')),
            market: FUNDING_MARKET.to_owned(),
            rule_type: FUNDING_RULE_TYPE.to_owned(),
            source: "synthetic-r3a-fixture".to_owned(),
            effective: EffectivePeriod::new(domain_time(2026, 1, 1), domain_time(2027, 1, 1))
                .expect("fixture effective period is valid"),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(content.value()),
        },
        content,
    )
    .expect("synthetic funding RulePack is valid")
}

fn synthetic_tax_pack() -> MarketRulePack {
    let content = RulePackContent::new(
        TAX_TYPE_URL,
        TaxRulePack {
            coupon_rules: vec![BondCouponTaxRule {
                first_issue_from: "2000-01-01".to_owned(),
                first_issue_to: String::new(),
                tax_attributes: Some(BondTaxAttributes {
                    value_added_tax_status: ProtoValueAddedTaxStatus::Taxable as i32,
                    income_tax_status: ProtoIncomeTaxStatus::Taxable as i32,
                }),
                rates: vec![SubjectCouponTaxRate {
                    value_added_tax_profile: "synthetic-vat".to_owned(),
                    income_tax_profile: "synthetic-income".to_owned(),
                    coupon_tax_rate: Some(decimal("0", 0)),
                }],
            }],
        }
        .encode_to_vec(),
    )
    .expect("synthetic tax payload is valid");
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('T'),
            version: Version::new(1).expect("fixture version is valid"),
            owner: OwnerRef::new(id('0'), id('1')),
            market: TAX_MARKET.to_owned(),
            rule_type: TAX_RULE_TYPE.to_owned(),
            source: "synthetic-r3b-tax-fixture-not-authoritative".to_owned(),
            effective: EffectivePeriod::new(domain_time(2026, 1, 1), domain_time(2027, 1, 1))
                .expect("fixture effective period is valid"),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(content.value()),
        },
        content,
    )
    .expect("synthetic tax RulePack is valid")
}

fn futures_contract_definition(suffix: char, symbol: &str, product: &str) -> DefinitionValue {
    let instrument = instrument(suffix, InstrumentKind::Futures, symbol);
    let contract = FuturesContract::new(
        &instrument,
        market_time(2026, 9, 17, 7),
        market_time(2026, 9, 18, 7),
        market_time(2026, 9, 18, 8),
        domain_decimal("100", 0, 'A'),
        VersionRef::new(id('X'), Version::new(1).expect("fixture version is valid")),
    )
    .expect("fixture concrete futures contract is valid")
    .with_risk_terms(product, unit_ref('B'))
    .expect("fixture FuturesContract risk terms are valid");
    DefinitionValue::Instrument(
        InstrumentDefinition::new(
            instrument,
            Some(InstrumentSubtype::FuturesContract(contract)),
        )
        .expect("fixture futures definition is valid"),
    )
}

fn delivery_bond_definition(suffix: char, symbol: &str) -> DefinitionValue {
    bond_definition(
        suffix,
        symbol,
        date(2024, 8, 15),
        date(2034, 8, 15),
        "25",
        3,
        BondCouponFrequency::Semiannual,
    )
}

#[allow(clippy::too_many_arguments)]
fn bond_definition(
    suffix: char,
    symbol: &str,
    issue_date: NaiveDate,
    maturity_date: NaiveDate,
    coupon_coefficient: &str,
    coupon_scale: u32,
    frequency: BondCouponFrequency,
) -> DefinitionValue {
    let instrument = instrument(suffix, InstrumentKind::Bond, symbol);
    let bond = Bond::with_issuance(
        &instrument,
        issue_date,
        issue_date,
        maturity_date,
        domain_decimal("100", 0, 'A'),
        DomainBondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Taxable),
        domain_decimal("100", 0, 'A'),
    )
    .expect("fixture registered Bond is valid")
    .with_pricing_terms(
        BondPricingTerms::new(
            domain_decimal(coupon_coefficient, coupon_scale, 'C'),
            frequency,
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

fn instrument(suffix: char, kind: InstrumentKind, symbol: &str) -> Instrument {
    Instrument::new(InstrumentInput {
        instrument_id: id(suffix),
        version: Version::new(1).expect("fixture version is valid"),
        owner: fixture_owner(),
        kind,
        market: if kind == InstrumentKind::Bond {
            "CN".to_owned()
        } else {
            "CFFEX".to_owned()
        },
        symbol: symbol.to_owned(),
        currency: UnitRef::new(id('A'), Version::new(1).expect("fixture version is valid")),
        calendar: VersionRef::new(id('K'), Version::new(1).expect("fixture version is valid")),
    })
    .expect("fixture Instrument is valid")
}

fn unit_definitions() -> Vec<DefinitionValue> {
    [
        ('A', "CNY", "currency_amount", 2),
        ('B', "CNY100", "price_per_100", 12),
        ('C', "RATE", "rate", 12),
        ('D', "YEAR", "years", 12),
        ('E', "YEAR2", "years_squared", 12),
        ('F', "DV01_100", "dv01_per_100", 12),
        ('G', "DV01", "dv01", 12),
        ('H', "ONE", "dimensionless", 12),
        ('J', "CONTRACT", "contract_count", 0),
    ]
    .into_iter()
    .map(|(suffix, code, dimension, scale)| {
        DefinitionValue::Unit(
            Unit::new(UnitInput {
                unit_id: id(suffix),
                version: fixture_version(),
                owner: fixture_owner(),
                code: code.to_owned(),
                dimension: dimension.to_owned(),
                scale,
                precision: 28,
            })
            .expect("fixture Unit is valid"),
        )
    })
    .collect()
}

fn calendar() -> Calendar {
    Calendar::new(CalendarInput {
        calendar_id: id('K'),
        version: fixture_version(),
        owner: fixture_owner(),
        market: "CN".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: EffectivePeriod::new(market_time(2005, 1, 1, 0), market_time(2031, 1, 10, 0))
            .expect("fixture Calendar period is valid"),
        sessions: vec![
            CalendarSession::open(
                date(2026, 7, 20),
                NaiveTime::from_hms_opt(9, 0, 0).expect("fixture time is valid"),
                NaiveTime::from_hms_opt(17, 0, 0).expect("fixture time is valid"),
            )
            .expect("fixture Calendar session is valid"),
        ],
    })
    .expect("fixture Calendar is valid")
}

fn curve_rule_pack() -> MarketRulePack {
    let bytes = b"phase2e-yield-curve-rule";
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('R'),
            version: fixture_version(),
            owner: fixture_owner(),
            market: "CN".to_owned(),
            rule_type: "yield-curve".to_owned(),
            source: "phase2e-fixture".to_owned(),
            effective: EffectivePeriod::new(
                market_time(2005, 1, 1, 0),
                market_time(2031, 1, 10, 0),
            )
            .expect("fixture Curve RulePack period is valid"),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(bytes),
        },
        RulePackContent::new(
            "type.googleapis.com/ficant.market.v1.CurveRulePack",
            bytes.to_vec(),
        )
        .expect("fixture Curve RulePack content is valid"),
    )
    .expect("fixture Curve RulePack is valid")
}

fn artifact_fixtures(
    bond: &AnalyticsObjectRef,
    contract: &AnalyticsObjectRef,
    target: &BondAnalyticsArtifactFacts,
    delivery: &FuturesDeliveryArtifactFacts,
    ctd: &BondAnalyticsArtifactFacts,
) -> Vec<(Artifact, Vec<u8>)> {
    let target_artifact = Artifact::new(
        id('7'),
        fixture_owner(),
        ArtifactKind::Generic,
        BOND_ANALYTICS_MEDIA_TYPE,
        ContentHash::digest(TARGET_BYTES),
        u64::try_from(TARGET_BYTES.len()).expect("fixture size fits u64"),
        vec![
            LineageRef::versioned(
                bond.version_ref().id().clone(),
                bond.version_ref().version(),
            ),
            LineageRef::new(
                target.rule_pack().version_ref().id().clone(),
                Some(target.rule_pack().version_ref().version()),
                Some(target.rule_pack().content_hash().clone()),
            )
            .expect("fixture target RulePack lineage is valid"),
            LineageRef::content_addressed(
                target.snapshot().version_ref().id().clone(),
                target.snapshot().content_hash().clone(),
            ),
        ],
    )
    .expect("fixture target Artifact is valid");
    let delivery_artifact = Artifact::new(
        id('8'),
        fixture_owner(),
        ArtifactKind::Generic,
        FUTURES_DELIVERY_MEDIA_TYPE,
        ContentHash::digest(DELIVERY_ARTIFACT_BYTES),
        u64::try_from(DELIVERY_ARTIFACT_BYTES.len()).expect("fixture size fits u64"),
        vec![
            LineageRef::versioned(
                contract.version_ref().id().clone(),
                contract.version_ref().version(),
            ),
            LineageRef::versioned(
                bond.version_ref().id().clone(),
                bond.version_ref().version(),
            ),
            LineageRef::new(
                delivery.rule_pack().version_ref().id().clone(),
                Some(delivery.rule_pack().version_ref().version()),
                Some(delivery.rule_pack().content_hash().clone()),
            )
            .expect("fixture delivery RulePack lineage is valid"),
            LineageRef::content_addressed(
                delivery.snapshot().version_ref().id().clone(),
                delivery.snapshot().content_hash().clone(),
            ),
        ],
    )
    .expect("fixture delivery Artifact is valid");
    let ctd_artifact = Artifact::new(
        id('9'),
        fixture_owner(),
        ArtifactKind::Generic,
        BOND_ANALYTICS_MEDIA_TYPE,
        ContentHash::digest(CTD_BYTES),
        u64::try_from(CTD_BYTES.len()).expect("fixture size fits u64"),
        vec![
            LineageRef::versioned(
                bond.version_ref().id().clone(),
                bond.version_ref().version(),
            ),
            LineageRef::new(
                ctd.rule_pack().version_ref().id().clone(),
                Some(ctd.rule_pack().version_ref().version()),
                Some(ctd.rule_pack().content_hash().clone()),
            )
            .expect("fixture CTD RulePack lineage is valid"),
            LineageRef::content_addressed(
                ctd.snapshot().version_ref().id().clone(),
                ctd.snapshot().content_hash().clone(),
            ),
        ],
    )
    .expect("fixture CTD Artifact is valid");
    vec![
        (target_artifact, TARGET_BYTES.to_vec()),
        (delivery_artifact, DELIVERY_ARTIFACT_BYTES.to_vec()),
        (ctd_artifact, CTD_BYTES.to_vec()),
    ]
}

fn canonical_quote(suffix: char, coefficient: &str, scale: u32) -> CanonicalQuote {
    let price = fixed_decimal(coefficient, scale);
    CanonicalQuote::new(
        VersionRef::new(
            id(suffix),
            Version::new(1).expect("fixture version is valid"),
        ),
        valuation_time(),
        valuation_time(),
        NaiveDate::from_ymd_opt(2026, 7, 20).expect("fixture quote date is valid"),
        Some(price),
        Some(price),
        unit_ref('B'),
    )
}

fn fixed_decimal(coefficient: &str, scale: u32) -> FixedDecimal {
    let scaled = coefficient
        .parse::<i128>()
        .expect("fixture Decimal coefficient is valid")
        .checked_mul(
            10_i128
                .checked_pow(12 - scale)
                .expect("fixture Decimal scale is valid"),
        )
        .expect("fixture Decimal fits the fixed representation");
    FixedDecimal::from_scaled(scaled)
}

fn domain_decimal(coefficient: &str, scale: u32, unit_suffix: char) -> DomainDecimalValue {
    DomainDecimalValue::new(
        coefficient,
        scale,
        UnitRef::new(
            id(unit_suffix),
            Version::new(1).expect("fixture version is valid"),
        ),
    )
    .expect("fixture Decimal is valid")
}

fn analytics_ref(value: &DefinitionValue) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(
        VersionRef::new(
            Ulid::new(value.identity()).expect("fixture definition identity is a ULID"),
            Version::new(value.version()).expect("fixture definition version is valid"),
        ),
        definition_content_hash(value),
    )
}

fn unit_ref(suffix: char) -> UnitRef {
    UnitRef::new(id(suffix), fixture_version())
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("fixture date is valid")
}

fn curve_valuation_time() -> MarketTime {
    market_time(2026, 7, 19, 7)
}

fn bond_valuation_time() -> MarketTime {
    market_time(2026, 7, 13, 7)
}

fn fixture_version() -> Version {
    Version::new(1).expect("fixture version is valid")
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut value, "{byte:02x}").expect("writing to String is infallible");
    }
    value
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
        .expect("R5D fixture Bond has exact pricing terms");
    let mut bytes = CanonicalBytes::new("definition/bond/v3");
    bytes.field(2, &version_ref_bytes(value.instrument()));
    bytes.field(3, value.first_issue_date().to_string().as_bytes());
    bytes.field(4, value.current_issue_date().to_string().as_bytes());
    bytes.field(5, value.maturity_date().to_string().as_bytes());
    bytes.field(6, &decimal_bytes(value.cumulative_issued_amount()));
    let tax = value
        .tax_attributes()
        .expect("R5D fixture Bond has exact tax attributes");
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
    bytes.field(11, &[1]);
    bytes.field(12, &[1]);
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

fn fixture_subject() -> SubjectRecord {
    let subject = Subject::new(id('S'), "Phase 2E fixture Subject").expect("fixture Subject");
    let version = SubjectVersion::new(
        ficant_domain::primitives::VersionRef::new(
            subject.id().clone(),
            Version::new(1).expect("fixture version is valid"),
        ),
        AccessSet::new(
            ["CN", "CFFEX"],
            [
                "bond-analytics",
                "yield-curve",
                "carry-roll",
                "futures-delivery",
                "futures-hedge",
            ],
        )
        .expect("fixture access is valid"),
        FundingTier::DrAvailable,
        TaxTreatment::new("synthetic-vat", "synthetic-income").expect("fixture tax"),
        "synthetic-assessment",
        "synthetic-liability",
        None,
    )
    .expect("fixture Subject version is valid");
    SubjectRecord::new(subject, version).expect("fixture Subject record is valid")
}

fn decimal(coefficient: &str, scale: u32) -> DecimalValue {
    DecimalValue {
        coefficient: coefficient.to_owned(),
        scale,
        unit: Some(ProtoUnitRef {
            unit_id: Some(ProtoUlid {
                value: id('C').as_str().to_owned(),
            }),
            version: 1,
        }),
    }
}

fn free_loopback_address() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("an ephemeral loopback port is available");
    let address = listener.local_addr().expect("listener has an address");
    drop(listener);
    address
}

fn domain_time(year: i32, month: u32, day: u32) -> MarketTime {
    market_time(year, month, day, 0)
}

fn valuation_time() -> MarketTime {
    market_time(2026, 7, 20, 7)
}

fn market_time(year: i32, month: u32, day: u32, utc_hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, month, day, utc_hour, 0, 0)
            .single()
            .expect("fixture instant is valid"),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(year, month, day).expect("fixture local date is valid"),
    )
    .expect("fixture market time is valid")
}

fn fixture_owner() -> OwnerRef {
    OwnerRef::new(id('0'), id('1'))
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => 'N',
        'L' => '2',
        'O' => '4',
        'U' => '3',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).expect("fixture ULID is valid")
}

fn storage_unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, false)
}
