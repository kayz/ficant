use chrono::{NaiveDate, TimeZone, Utc};
use ficant_api::{
    GrpcWebServerConfig, PlatformApplication, PlatformGrpcService, PlatformPort, RatesGrpcService,
    SessionPolicy, SystemClock, TrustedIdentity, serve_grpc_web_with_rates,
};
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, CanonicalQuote, CanonicalSnapshotDecoder,
    DecodedCanonicalQuotes, DefinitionIdentity, DefinitionRepository, DefinitionValue,
    InstrumentDefinition, InstrumentSubtype, IntegrityEvent, IntegrityEventSink,
    RequiredVerifiedBlobRead, SnapshotVerifiedReadMetadata, SnapshotVerifiedReadMetadataRepository,
    SubjectRepository, VerifiedBlobPayload, VerifiedBlobReader, VerifiedBlobRole,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_cgb_futures_pack::{CgbFuturesDeliveryRulePackParser, MARKET, RULE_TYPE, TYPE_URL};
use ficant_contracts::ficant::core::v1::{
    DecimalValue, FundingTier as ProtoFundingTier, Ulid as ProtoUlid, UnitRef as ProtoUnitRef,
};
use ficant_contracts::ficant::market::v1::{
    BondCouponTaxRule, BondTaxAttributes, FundingRulePack, FundingTierRate,
    IncomeTaxStatus as ProtoIncomeTaxStatus, SubjectCouponTaxRate, TaxRulePack,
    ValueAddedTaxStatus as ProtoValueAddedTaxStatus,
};
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::market::{
    Bond, BondTaxAttributes as DomainBondTaxAttributes, FuturesContract, IncomeTaxStatus,
    Instrument, InstrumentInput, InstrumentKind, MarketRulePack, MarketRulePackInput,
    RulePackContent, ValueAddedTaxStatus, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue as DomainDecimalValue, EffectivePeriod, LineageRef, MarketTime,
    OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{DataSnapshot, DataSnapshotInput};
use ficant_domain::subject::{
    AccessSet, FundingTier, Subject, SubjectRecord, SubjectStateSnapshot, SubjectVersion,
    TaxTreatment,
};
use ficant_fixed_income_native::{
    NativeBondAnalyticsEngine, NativeCarryRollEngine, NativeFuturesDeliveryEngine,
    NativeFuturesHedgeEngine, NativeYieldCurveEngine,
};
use ficant_funding_pack::{
    FundingRulePackV1Parser, MARKET as FUNDING_MARKET, RULE_TYPE as FUNDING_RULE_TYPE,
    TYPE_URL as FUNDING_TYPE_URL,
};
use ficant_tax_pack::{
    MARKET as TAX_MARKET, RULE_TYPE as TAX_RULE_TYPE, TYPE_URL as TAX_TYPE_URL, TaxRulePackV1Parser,
};
use prost::Message;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";
const TOKEN: &str = "phase2e-python-sdk-test-token";
const CGB_FUTURES_PACK: &[u8] =
    include_bytes!("../../../domain-packs/cgb-futures/cgb-futures-v1.bin");

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
        Ok(self
            .values
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
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Err(storage_unavailable())
    }
}

#[derive(Clone, Copy)]
struct FixtureSnapshotMetadata;

#[tonic::async_trait]
impl SnapshotVerifiedReadMetadataRepository for FixtureSnapshotMetadata {
    async fn get_verified_read_metadata(
        &self,
        _: &AccessScope,
        snapshot_id: Ulid,
    ) -> Result<Option<SnapshotVerifiedReadMetadata>, ApplicationError> {
        if snapshot_id != id('Y') {
            return Ok(None);
        }
        let snapshot = DataSnapshot::new(DataSnapshotInput {
            data_snapshot_id: id('Y'),
            owner: fixture_owner(),
            visible_at: valuation_time(),
            as_of: valuation_time(),
            schema_hash: ContentHash::digest(b"phase2e-canonical-schema"),
            manifest_hash: ContentHash::digest(b"phase2e-manifest"),
            blob_content_hash: ContentHash::digest(b"object-Y"),
            lineage: vec![LineageRef::content_addressed(
                id('Q'),
                ContentHash::digest(b"phase2e-source"),
            )],
        })
        .expect("fixture DataSnapshot is valid");
        SnapshotVerifiedReadMetadata::data(
            snapshot,
            b"object-Y".len() as u64,
            b"phase2e-manifest".len() as u64,
        )
        .map(Some)
    }
}

#[derive(Clone, Copy)]
struct FixtureBlobReader;

#[tonic::async_trait]
impl VerifiedBlobReader for FixtureBlobReader {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> Result<VerifiedBlobPayload, ApplicationError> {
        let bytes = match request.blob_role() {
            VerifiedBlobRole::DataParquet => b"object-Y".as_slice(),
            VerifiedBlobRole::DataManifest => b"phase2e-manifest".as_slice(),
            _ => unreachable!("delivery reads only DataSnapshot roles"),
        };
        request.verify_bytes(sink, bytes.to_vec()).await
    }
}

#[derive(Clone, Copy)]
struct FixtureIntegrityEvents;

#[tonic::async_trait]
impl IntegrityEventSink for FixtureIntegrityEvents {
    async fn emit(&self, _: IntegrityEvent) -> Result<(), ApplicationError> {
        unreachable!("fixture payload hashes and sizes are exact")
    }
}

#[derive(Clone, Copy)]
struct FixtureCanonicalSnapshotDecoder;

#[tonic::async_trait]
impl CanonicalSnapshotDecoder for FixtureCanonicalSnapshotDecoder {
    async fn decode_quotes(
        &self,
        snapshot: &DataSnapshot,
        parquet: &[u8],
        manifest: &[u8],
    ) -> Result<DecodedCanonicalQuotes, ApplicationError> {
        assert_eq!(snapshot.id(), &id('Y'));
        assert_eq!(parquet, b"object-Y");
        assert_eq!(manifest, b"phase2e-manifest");
        DecodedCanonicalQuotes::new(
            VersionRef::new(id('S'), Version::new(1).expect("fixture version is valid")),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "invoked only by scripts/check-phase2e-sdk.ps1"]
async fn python_sdk_matches_phase2_reference_slices_through_live_rule_pack_composition() {
    let address = free_loopback_address();
    let application = application();
    let platform = PlatformGrpcService::new(Arc::clone(&application), KEY)
        .expect("fixture platform service is valid");
    let rates = RatesGrpcService::new(
        application,
        Arc::new(NativeBondAnalyticsEngine),
        Arc::new(NativeYieldCurveEngine),
        Arc::new(NativeCarryRollEngine),
        Arc::new(NativeFuturesDeliveryEngine),
        Arc::new(FixtureDefinitions {
            values: vec![
                DefinitionValue::MarketRulePack(frozen_cgb_futures_pack()),
                DefinitionValue::MarketRulePack(synthetic_funding_pack()),
                DefinitionValue::MarketRulePack(synthetic_tax_pack()),
                futures_contract_definition(),
                bond_definition('2', "T-bond-expensive"),
                bond_definition('3', "T-bond-ctd"),
                bond_definition('4', "T-bond-tied-later"),
            ],
        }),
        Arc::new(FixtureSubjects {
            value: fixture_subject(),
        }),
        Arc::new(CgbFuturesDeliveryRulePackParser),
        Arc::new(FixtureSnapshotMetadata),
        Arc::new(FixtureBlobReader),
        Arc::new(FixtureIntegrityEvents),
        Arc::new(FixtureCanonicalSnapshotDecoder),
        Arc::new(FundingRulePackV1Parser),
        Arc::new(TaxRulePackV1Parser),
        Arc::new(NativeFuturesHedgeEngine),
        KEY,
    )
    .expect("fixture rates service is valid");
    let server = tokio::spawn(async move {
        serve_grpc_web_with_rates(
            GrpcWebServerConfig {
                bind: address,
                allowed_origins: vec!["http://127.0.0.1:4174".to_owned()],
            },
            platform,
            rates,
        )
        .await
    });

    let endpoint = address.to_string();
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root exists");
    let outcome = tokio::task::spawn_blocking(move || {
        Command::new("uv")
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
    let identity = TrustedIdentity::bearer("phase2e-sdk-test", TOKEN.as_bytes(), ["rates:analyze"])
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
            rule_pack_id: id('F'),
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

fn futures_contract_definition() -> DefinitionValue {
    let instrument = instrument('Z', InstrumentKind::Futures, "T2609");
    let contract = FuturesContract::new(
        &instrument,
        market_time(2026, 9, 17, 7),
        market_time(2026, 9, 18, 7),
        market_time(2026, 9, 18, 8),
        domain_decimal("100", 0, 'A'),
        VersionRef::new(id('X'), Version::new(1).expect("fixture version is valid")),
    )
    .expect("fixture concrete futures contract is valid");
    DefinitionValue::Instrument(
        InstrumentDefinition::new(
            instrument,
            Some(InstrumentSubtype::FuturesContract(contract)),
        )
        .expect("fixture futures definition is valid"),
    )
}

fn bond_definition(suffix: char, symbol: &str) -> DefinitionValue {
    let instrument = instrument(suffix, InstrumentKind::Bond, symbol);
    let bond = Bond::with_issuance(
        &instrument,
        NaiveDate::from_ymd_opt(2024, 8, 15).expect("fixture issue date is valid"),
        NaiveDate::from_ymd_opt(2024, 8, 15).expect("fixture current issue date is valid"),
        NaiveDate::from_ymd_opt(2034, 8, 15).expect("fixture maturity date is valid"),
        domain_decimal("100", 0, 'A'),
        DomainBondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Taxable),
        domain_decimal("100", 0, 'A'),
    )
    .expect("fixture registered Bond is valid");
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
        market: "CFFEX".to_owned(),
        symbol: symbol.to_owned(),
        currency: UnitRef::new(id('A'), Version::new(1).expect("fixture version is valid")),
        calendar: VersionRef::new(id('K'), Version::new(1).expect("fixture version is valid")),
    })
    .expect("fixture Instrument is valid")
}

fn canonical_quote(suffix: char, coefficient: &str, scale: u32) -> CanonicalQuote {
    CanonicalQuote::new(
        VersionRef::new(
            id(suffix),
            Version::new(1).expect("fixture version is valid"),
        ),
        valuation_time(),
        valuation_time(),
        NaiveDate::from_ymd_opt(2026, 7, 20).expect("fixture quote date is valid"),
        Some(fixed_decimal(coefficient, scale)),
        None,
        UnitRef::new(id('B'), Version::new(1).expect("fixture version is valid")),
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
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).expect("fixture ULID is valid")
}

fn storage_unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, false)
}
