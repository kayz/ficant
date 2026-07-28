use chrono::{NaiveDate, TimeZone, Utc};
use ficant_api::{
    GrpcWebServerConfig, PlatformApplication, PlatformGrpcService, PlatformPort, RatesGrpcService,
    SessionPolicy, SystemClock, TrustedIdentity, serve_grpc_web_with_rates,
};
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, DefinitionIdentity, DefinitionRepository, DefinitionValue,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_cgb_futures_pack::{CgbFuturesDeliveryRulePackParser, MARKET, RULE_TYPE, TYPE_URL};
use ficant_domain::VersionedDefinition;
use ficant_domain::market::{
    MarketRulePack, MarketRulePackInput, RulePackContent, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version,
};
use ficant_fixed_income_native::{
    NativeBondAnalyticsEngine, NativeCarryRollEngine, NativeFuturesDeliveryEngine,
    NativeFuturesHedgeEngine, NativeYieldCurveEngine,
};
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
    value: MarketRulePack,
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
        if self.value.identity() == definition_id.as_str() && self.value.version() == version.get()
        {
            Ok(Some(DefinitionValue::MarketRulePack(self.value.clone())))
        } else {
            Ok(None)
        }
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
            value: frozen_cgb_futures_pack(),
        }),
        Arc::new(CgbFuturesDeliveryRulePackParser),
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

fn free_loopback_address() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("an ephemeral loopback port is available");
    let address = listener.local_addr().expect("listener has an address");
    drop(listener);
    address
}

fn domain_time(year: i32, month: u32, day: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .expect("fixture instant is valid"),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(year, month, day).expect("fixture local date is valid"),
    )
    .expect("fixture market time is valid")
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).expect("fixture ULID is valid")
}

fn storage_unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, false)
}
