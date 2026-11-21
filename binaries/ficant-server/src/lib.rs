//! Single-process zero-dependency Ficant server entrypoint.
//!
//! Serves native HTTP/2 gRPC and HTTP/1.1 `gRPC-Web` on a single port across:
//! - `ficant.rates.v1.RatesAnalyticsService` (always enabled — BP-1 compatible)
//! - `ficant.blotter.v1.DeskBlotterService` (3 storage modes: disabled safe default, exclusive
//!   WAL file production mode, or explicit ephemeral in-memory test mode)
//! - Four versioned Desk Blotter SPIs (`PositionBaseProviderService`,
//!   `TradeFeedReconcilerService`, `MarketQuoteStreamService`, `IntranetNlpIntentParserService`).

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod clock;
pub mod policy;
pub mod store_adapter;

use std::env;
use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ficant_api::{
    BlotterStorePort, DeskBlotterGrpcService, DeskBlotterServiceConfig,
    DisabledDeskBlotterGrpcService, IntranetNlpIntentParserGrpcService,
    MarketQuoteStreamGrpcService, OwnedBlotterEngines, PositionBaseProviderGrpcService,
    RatesGrpcService, StaticCredentialVerifier, TradeFeedReconcilerGrpcService, WatchdogController,
    spawn_blotter_watchdog,
};
use ficant_application::ports::{
    BlotterClock, BlotterCommitPlan, CredentialVerifier, MarketQuoteStreamPort,
    NlpIntentParserPort, PositionBaseProviderPort, TradeFeedReconcilerPort,
};
use ficant_application::use_cases::blotter::{AnalysisUnits, DeskBlotterOrchestrator};
use ficant_blotter_store::{
    CsvMarketQuoteStream, CsvPositionBaseProvider, CsvTemplateNlpIntentParser,
    CsvTradeFeedReconciler, DEFAULT_EVENT_RETENTION_WINDOW, MockMarketQuoteStream,
    MockNlpIntentParser, MockPositionBaseProvider, MockTradeFeedReconciler,
    SqliteBlotterOpenOptions, SqliteBlotterStore, default_reference_analysis_units,
};
use ficant_cgb_futures_pack::CgbFuturesDeliveryRulePackParser;
use ficant_contracts::ficant::blotter::v1::desk_blotter_service_server::DeskBlotterServiceServer;
use ficant_contracts::ficant::blotter::v1::intranet_nlp_intent_parser_service_server::IntranetNlpIntentParserServiceServer;
use ficant_contracts::ficant::blotter::v1::market_quote_stream_service_server::MarketQuoteStreamServiceServer;
use ficant_contracts::ficant::blotter::v1::position_base_provider_service_server::PositionBaseProviderServiceServer;
use ficant_contracts::ficant::blotter::v1::trade_feed_reconciler_service_server::TradeFeedReconcilerServiceServer;
use ficant_contracts::ficant::rates::v1::rates_analytics_service_server::RatesAnalyticsServiceServer;
use ficant_fixed_income_native::{
    NativeBondAnalyticsEngine, NativeCarryRollEngine, NativeFuturesDeliveryEngine,
    NativeFuturesHedgeEngine, NativeYieldCurveEngine,
};
use ficant_funding_pack::FundingRulePackV1Parser;
use ficant_regulatory_pack::RegulatoryRulePackV1Parser;
use ficant_tax_pack::TaxRulePackV2Parser;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

pub use clock::SystemBlotterClock;
pub use policy::{
    AccountPolicyJson, AccountPolicyOverrideJson, BlotterConfigJson, DeskBlotterPolicyFile,
    DeskBlotterPolicyRuntime, EMBEDDED_DEFAULT_POLICY_JSON, RulePackBindingJson,
};
pub use store_adapter::{DEFAULT_EVENT_BUS_CAPACITY, SqliteBlotterRuntime};

pub const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:50051";
pub const DEFAULT_WATCHDOG_INTERVAL: Duration = Duration::from_secs(5);

/// Storage operating mode for Desk Blotter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BlotterStorageMode {
    /// BP-1 safe default: `RatesAnalyticsService` and `--health-check` work; Desk Blotter and
    /// SPIs return `STORAGE_UNAVAILABLE` and ZERO files are created.
    #[default]
    Disabled,
    /// Production persistent exclusive-WAL `SQLite` database at `db_path`.
    PersistentWal { db_path: PathBuf },
    /// Explicit isolated ephemeral in-memory `SQLite` database (reusing identical DDL).
    EphemeralInMemory,
}

/// Optional reference SPI backend implementations mounted alongside `DeskBlotterService`.
#[derive(Clone, Default)]
pub struct SpiBackends {
    pub position_base: Option<Arc<dyn PositionBaseProviderPort>>,
    pub trade_feed: Option<Arc<dyn TradeFeedReconcilerPort>>,
    pub market_quotes: Option<Arc<dyn MarketQuoteStreamPort>>,
    pub nlp_parser: Option<Arc<dyn NlpIntentParserPort>>,
    pub spi_units: Option<AnalysisUnits>,
}

/// Complete programmatic configuration for starting a `ficant-server` instance.
pub struct FicantServerConfig {
    pub listen_addr: SocketAddr,
    pub storage_mode: BlotterStorageMode,
    pub policy: DeskBlotterPolicyRuntime,
    pub credentials: Option<Arc<StaticCredentialVerifier>>,
    pub clock: Arc<dyn BlotterClock>,
    pub service_config: DeskBlotterServiceConfig,
    pub event_retention_window: u64,
    pub event_bus_capacity: usize,
    /// `Some(dur)` enables periodic background sweeps + manual trigger; `None` runs sweeps ONLY
    /// on explicit [`WatchdogController::trigger_sweep_and_wait`] calls (deterministic mode).
    pub watchdog_interval: Option<Duration>,
    pub spi_backends: SpiBackends,
}

impl FicantServerConfig {
    /// Builds a default safe BP-1 compatible configuration (Rates-only, blotter disabled).
    pub fn disabled_default(listen_addr: SocketAddr) -> Result<Self, String> {
        Ok(Self {
            listen_addr,
            storage_mode: BlotterStorageMode::Disabled,
            policy: DeskBlotterPolicyRuntime::from_embedded_default()?,
            credentials: None,
            clock: Arc::new(SystemBlotterClock),
            service_config: DeskBlotterServiceConfig::default(),
            event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
            event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
            watchdog_interval: Some(DEFAULT_WATCHDOG_INTERVAL),
            spi_backends: SpiBackends::default(),
        })
    }
}

/// Handle to a running single-port `ficant-server` instance.
pub struct ServerHandle {
    addr: SocketAddr,
    runtime: Option<Arc<SqliteBlotterRuntime>>,
    watchdog: Option<WatchdogController>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<tokio::task::JoinHandle<Result<(), String>>>,
}

impl ServerHandle {
    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    #[must_use]
    pub const fn runtime(&self) -> Option<&Arc<SqliteBlotterRuntime>> {
        self.runtime.as_ref()
    }

    #[must_use]
    pub const fn watchdog(&self) -> Option<&WatchdogController> {
        self.watchdog.as_ref()
    }

    /// Gracefully shuts down the gRPC server, background watchdog, and releases any exclusive WAL
    /// file lock cleanly.
    pub async fn shutdown(mut self) -> Result<(), String> {
        if let Some(wd) = self.watchdog.take() {
            wd.shutdown();
        }
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(jh) = self.join_handle.take() {
            jh.await
                .map_err(|err| format!("server task join failed: {err}"))??;
        }
        drop(self.runtime.take());
        Ok(())
    }
}

#[must_use]
pub fn build_rates_service() -> RatesGrpcService {
    RatesGrpcService::new(
        Arc::new(NativeBondAnalyticsEngine),
        Arc::new(NativeYieldCurveEngine),
        Arc::new(NativeCarryRollEngine),
        Arc::new(NativeFuturesDeliveryEngine),
        Arc::new(NativeFuturesHedgeEngine),
        Arc::new(CgbFuturesDeliveryRulePackParser),
        Arc::new(FundingRulePackV1Parser),
        Arc::new(TaxRulePackV2Parser),
    )
}

#[must_use]
pub fn build_blotter_engines() -> OwnedBlotterEngines {
    OwnedBlotterEngines {
        bond_engine: Arc::new(NativeBondAnalyticsEngine),
        hedge_engine: Arc::new(NativeFuturesHedgeEngine),
        delivery_rule_parser: Arc::new(CgbFuturesDeliveryRulePackParser),
        regulatory_rule_parser: Arc::new(RegulatoryRulePackV1Parser),
    }
}

/// Starts a server in background on `config.listen_addr` (supports `127.0.0.1:0` ephemeral port),
/// performing fail-closed validation, WAL opening, and startup policy reconciliation BEFORE
/// accepting any incoming requests.
#[allow(clippy::too_many_lines)]
pub async fn spawn_server(config: FicantServerConfig) -> Result<ServerHandle, String> {
    config.service_config.validate()?;
    let reference_units = match config.spi_backends.spi_units.clone() {
        Some(u) => u,
        None => default_reference_analysis_units()
            .map_err(|_| "failed to construct default analysis units".to_owned())?,
    };

    let rates_svc = RatesAnalyticsServiceServer::new(build_rates_service());

    let (
        runtime_opt,
        watchdog_opt,
        blotter_enabled_svc,
        blotter_disabled_svc,
        pos_svc,
        feed_svc,
        quote_svc,
        nlp_svc,
    ) = match &config.storage_mode {
        BlotterStorageMode::Disabled => (
            None,
            None,
            None,
            Some(DeskBlotterServiceServer::new(
                DisabledDeskBlotterGrpcService,
            )),
            PositionBaseProviderServiceServer::new(PositionBaseProviderGrpcService::disabled(
                config.clock.clone(),
            )),
            TradeFeedReconcilerServiceServer::new(TradeFeedReconcilerGrpcService::disabled(
                config.clock.clone(),
                reference_units.clone(),
            )),
            MarketQuoteStreamServiceServer::new(MarketQuoteStreamGrpcService::disabled(
                config.clock.clone(),
                reference_units.clone(),
            )),
            IntranetNlpIntentParserServiceServer::new(
                IntranetNlpIntentParserGrpcService::disabled(
                    config.clock.clone(),
                    reference_units.clone(),
                ),
            ),
        ),
        BlotterStorageMode::PersistentWal { db_path } => {
            let credentials = config.credentials.clone().ok_or_else(|| {
                "blotter storage is enabled but no credentials verifier was configured".to_owned()
            })?;
            credentials.validate_startup(&config.clock.now())?;
            let open_opts =
                SqliteBlotterOpenOptions::file(db_path, config.policy.blotter_config.clone())
                    .with_event_retention_window(config.event_retention_window);
            let store = Arc::new(SqliteBlotterStore::open(&open_opts).map_err(|err| {
                format!("failed to open blotter WAL store: {:?}", err.category())
            })?);
            let credentials_port: Arc<dyn CredentialVerifier> = credentials;
            build_enabled_blotter_stack(store, credentials_port, &config, reference_units)?
        }
        BlotterStorageMode::EphemeralInMemory => {
            let credentials = config.credentials.clone().ok_or_else(|| {
                "blotter storage is enabled but no credentials verifier was configured".to_owned()
            })?;
            credentials.validate_startup(&config.clock.now())?;
            let open_opts =
                SqliteBlotterOpenOptions::ephemeral_in_memory(config.policy.blotter_config.clone())
                    .with_event_retention_window(config.event_retention_window);
            let store = Arc::new(SqliteBlotterStore::open(&open_opts).map_err(|err| {
                format!(
                    "failed to open ephemeral blotter store: {:?}",
                    err.category()
                )
            })?);
            let credentials_port: Arc<dyn CredentialVerifier> = credentials;
            build_enabled_blotter_stack(store, credentials_port, &config, reference_units)?
        }
    };

    let listener = TcpListener::bind(config.listen_addr)
        .await
        .map_err(|err| format!("failed to bind {}: {err}", config.listen_addr))?;
    let bound_addr = listener
        .local_addr()
        .map_err(|err| format!("failed to read bound addr: {err}"))?;
    let incoming = TcpListenerStream::new(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let mut router = Server::builder()
        .accept_http1(true)
        .layer(tonic_web::GrpcWebLayer::new())
        .add_service(rates_svc)
        .add_service(pos_svc)
        .add_service(feed_svc)
        .add_service(quote_svc)
        .add_service(nlp_svc);

    if let Some(svc) = blotter_enabled_svc {
        router = router.add_service(svc);
    } else if let Some(svc) = blotter_disabled_svc {
        router = router.add_service(svc);
    }

    let join_handle = tokio::spawn(async move {
        router
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .map_err(|err| format!("server failed: {err}"))
    });

    Ok(ServerHandle {
        addr: bound_addr,
        runtime: runtime_opt,
        watchdog: watchdog_opt,
        shutdown_tx: Some(shutdown_tx),
        join_handle: Some(join_handle),
    })
}

type EnabledStackTuple = (
    Option<Arc<SqliteBlotterRuntime>>,
    Option<WatchdogController>,
    Option<DeskBlotterServiceServer<DeskBlotterGrpcService>>,
    Option<DeskBlotterServiceServer<DisabledDeskBlotterGrpcService>>,
    PositionBaseProviderServiceServer<PositionBaseProviderGrpcService>,
    TradeFeedReconcilerServiceServer<TradeFeedReconcilerGrpcService>,
    MarketQuoteStreamServiceServer<MarketQuoteStreamGrpcService>,
    IntranetNlpIntentParserServiceServer<IntranetNlpIntentParserGrpcService>,
);

fn build_enabled_blotter_stack(
    store: Arc<SqliteBlotterStore>,
    credentials: Arc<dyn CredentialVerifier>,
    config: &FicantServerConfig,
    reference_units: AnalysisUnits,
) -> Result<EnabledStackTuple, String> {
    let engines = build_blotter_engines();
    let runtime = Arc::new(SqliteBlotterRuntime::new(store, config.event_bus_capacity));

    // Startup policy & active quote-source reconciliation BEFORE serving any RPCs.
    reconcile_startup_policies(
        runtime.as_ref(),
        &config.policy,
        config.clock.as_ref(),
        &engines,
    )?;

    let policy_provider = Arc::new(config.policy.clone());
    let blotter_svc = DeskBlotterGrpcService::new(
        runtime.clone(),
        policy_provider,
        credentials.clone(),
        config.clock.clone(),
        engines.clone(),
        config.service_config.clone(),
    )?;

    let watchdog = spawn_blotter_watchdog(
        runtime.clone(),
        config.clock.clone(),
        engines,
        config.watchdog_interval,
    );

    let pos_port = config
        .spi_backends
        .position_base
        .clone()
        .unwrap_or_else(|| Arc::new(MockPositionBaseProvider::new()));
    let feed_port = config
        .spi_backends
        .trade_feed
        .clone()
        .unwrap_or_else(|| Arc::new(MockTradeFeedReconciler::new()));
    let quote_port = config
        .spi_backends
        .market_quotes
        .clone()
        .unwrap_or_else(|| Arc::new(MockMarketQuoteStream::new()));
    let nlp_port = config
        .spi_backends
        .nlp_parser
        .clone()
        .unwrap_or_else(|| Arc::new(MockNlpIntentParser::default()));

    Ok((
        Some(runtime),
        Some(watchdog),
        Some(DeskBlotterServiceServer::new(blotter_svc)),
        None,
        PositionBaseProviderServiceServer::new(PositionBaseProviderGrpcService::new(
            pos_port,
            credentials.clone(),
            config.clock.clone(),
        )),
        TradeFeedReconcilerServiceServer::new(TradeFeedReconcilerGrpcService::new(
            feed_port,
            credentials.clone(),
            config.clock.clone(),
            reference_units.clone(),
        )),
        MarketQuoteStreamServiceServer::new(MarketQuoteStreamGrpcService::new(
            quote_port,
            credentials.clone(),
            config.clock.clone(),
            reference_units.clone(),
        )),
        IntranetNlpIntentParserServiceServer::new(IntranetNlpIntentParserGrpcService::new(
            nlp_port,
            credentials,
            config.clock.clone(),
            reference_units,
        )),
    ))
}

fn reconcile_startup_policies(
    runtime: &SqliteBlotterRuntime,
    policy: &DeskBlotterPolicyRuntime,
    clock: &dyn BlotterClock,
    engines: &OwnedBlotterEngines,
) -> Result<(), String> {
    let policy_cloned = policy.clone();
    let engines_cloned = engines.clone();
    let clock_now = clock.now();

    // Use a pinned startup clock holding `clock_now` inside the synchronous startup transaction.
    let startup_clock = ficant_application::ports::ManualBlotterClock::new(clock_now);
    let _ = runtime
        .execute_write_authorized_bytes(
            None,
            Box::new(|_| Ok(())),
            Box::new(move |state| {
                let desired = policy_cloned.startup_policies_for_accounts(
                    state.domain.accounts.keys().map(String::as_str),
                );
                if desired.is_empty() {
                    return Ok((
                        Vec::new(),
                        BlotterCommitPlan {
                            next_state: state.clone(),
                            event: None,
                            idempotency_receipt: None,
                        },
                    ));
                }
                let orch =
                    DeskBlotterOrchestrator::new(&startup_clock, engines_cloned.as_borrowed());
                let (next_state, _out, event) =
                    orch.reconcile_startup_quote_sources(state, &desired)?;
                Ok((
                    Vec::new(),
                    BlotterCommitPlan {
                        next_state,
                        event,
                        idempotency_receipt: None,
                    },
                ))
            }),
        )
        .map_err(|err| format!("startup policy reconciliation failed: {:?}", err.category()))?;
    Ok(())
}

/// Serves BP-1 compatible Rates-only server on `addr` (preserves original API signature).
pub async fn serve_addr(addr: SocketAddr) -> Result<(), String> {
    let config = FicantServerConfig::disabled_default(addr)?;
    let handle = spawn_server(config).await?;
    if let Some(jh) = handle.join_handle {
        jh.await
            .map_err(|err| format!("server task join failed: {err}"))??;
    }
    Ok(())
}

pub fn health_check_from_env() -> Result<(), String> {
    let addr = resolve_listen_addr()?;
    let probe = if addr.ip().is_unspecified() {
        SocketAddr::from(([127, 0, 0, 1], addr.port()))
    } else {
        addr
    };
    TcpStream::connect_timeout(&probe, Duration::from_secs(2))
        .map(|_| ())
        .map_err(|err| format!("health check failed for {probe}: {err}"))
}

/// Parses server configuration from environment variables (fail-closed on any invalid or
/// contradictory storage, policy, or credential configuration).
pub fn config_from_env() -> Result<FicantServerConfig, String> {
    let listen_addr = resolve_listen_addr()?;

    let db_path_env = env::var("FICANT_BLOTTER_DB_PATH").ok();
    let ephemeral_env = env::var("FICANT_BLOTTER_EPHEMERAL_IN_MEMORY").ok();
    let ephemeral_enabled = parse_bool_env(
        "FICANT_BLOTTER_EPHEMERAL_IN_MEMORY",
        ephemeral_env.as_deref(),
    )?;

    if db_path_env.is_some() && ephemeral_enabled {
        return Err(
            "contradictory storage configuration: both FICANT_BLOTTER_DB_PATH and FICANT_BLOTTER_EPHEMERAL_IN_MEMORY are set"
                .to_owned(),
        );
    }

    let storage_mode = if let Some(raw_path) = db_path_env {
        BlotterStorageMode::PersistentWal {
            db_path: PathBuf::from(raw_path),
        }
    } else if ephemeral_enabled {
        BlotterStorageMode::EphemeralInMemory
    } else {
        BlotterStorageMode::Disabled
    };

    let policy = match env::var("FICANT_BLOTTER_POLICY_PATH").ok() {
        Some(path) => DeskBlotterPolicyRuntime::from_file(path)?,
        None => DeskBlotterPolicyRuntime::from_embedded_default()?,
    };

    let clock: Arc<dyn BlotterClock> = Arc::new(SystemBlotterClock);
    let credentials: Option<Arc<StaticCredentialVerifier>> =
        match env::var("FICANT_BLOTTER_CREDENTIALS_PATH").ok() {
            Some(path) => {
                let raw = fs::read_to_string(&path)
                    .map_err(|err| format!("failed to read credentials file '{path}': {err}"))?;
                let verifier = if storage_mode == BlotterStorageMode::Disabled {
                    StaticCredentialVerifier::from_json_str(&raw)?
                } else {
                    StaticCredentialVerifier::from_json_str_at(&raw, &clock.now())?
                };
                Some(Arc::new(verifier))
            }
            None => None,
        };

    let event_retention_window = match env::var("FICANT_BLOTTER_EVENT_RETENTION_WINDOW").ok() {
        Some(raw) => raw.parse::<u64>().map_err(|err| {
            format!("invalid FICANT_BLOTTER_EVENT_RETENTION_WINDOW '{raw}': {err}")
        })?,
        None => DEFAULT_EVENT_RETENTION_WINDOW,
    };

    let watchdog_interval = match env::var("FICANT_BLOTTER_WATCHDOG_INTERVAL_MS").ok() {
        Some(raw) => {
            let ms = raw.parse::<u64>().map_err(|err| {
                format!("invalid FICANT_BLOTTER_WATCHDOG_INTERVAL_MS '{raw}': {err}")
            })?;
            if ms == 0 {
                None
            } else {
                Some(Duration::from_millis(ms))
            }
        }
        None => Some(DEFAULT_WATCHDOG_INTERVAL),
    };

    let spi_backends = load_spi_backends_from_env()?;

    Ok(FicantServerConfig {
        listen_addr,
        storage_mode,
        policy,
        credentials,
        clock,
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval,
        spi_backends,
    })
}

pub async fn entry_from_env() -> Result<(), String> {
    let config = config_from_env()?;
    let handle = spawn_server(config).await?;
    if let Some(jh) = handle.join_handle {
        jh.await
            .map_err(|err| format!("server task join failed: {err}"))??;
    }
    Ok(())
}

fn load_spi_backends_from_env() -> Result<SpiBackends, String> {
    let units = default_reference_analysis_units()
        .map_err(|_| "failed to construct default analysis units".to_owned())?;

    let position_base: Option<Arc<dyn PositionBaseProviderPort>> = match (
        env::var("FICANT_SPI_POSITIONS_ACCOUNTS_CSV").ok(),
        env::var("FICANT_SPI_POSITIONS_BALANCES_CSV").ok(),
    ) {
        (Some(accts), Some(bals)) => {
            let provider = CsvPositionBaseProvider::from_csv_files(&accts, &bals, &units)
                .map_err(|_| format!("failed to load position base CSVs '{accts}', '{bals}'"))?;
            Some(Arc::new(provider))
        }
        (None, None) => None,
        _ => {
            return Err(
                "both FICANT_SPI_POSITIONS_ACCOUNTS_CSV and FICANT_SPI_POSITIONS_BALANCES_CSV must be set together"
                    .to_owned(),
            );
        }
    };

    let trade_feed: Option<Arc<dyn TradeFeedReconcilerPort>> =
        match env::var("FICANT_SPI_TRADE_FEED_CSV").ok() {
            Some(path) => {
                let reconciler = CsvTradeFeedReconciler::from_csv_file(&path)
                    .map_err(|_| format!("failed to load trade feed CSV '{path}'"))?;
                Some(Arc::new(reconciler))
            }
            None => None,
        };

    let market_quotes: Option<Arc<dyn MarketQuoteStreamPort>> =
        match env::var("FICANT_SPI_QUOTES_CSV").ok() {
            Some(path) => {
                let stream = CsvMarketQuoteStream::from_csv_file(&path)
                    .map_err(|_| format!("failed to load quotes CSV '{path}'"))?;
                Some(Arc::new(stream))
            }
            None => None,
        };

    let nlp_parser: Option<Arc<dyn NlpIntentParserPort>> = match env::var("FICANT_SPI_NLP_CSV").ok()
    {
        Some(path) => {
            let parser = CsvTemplateNlpIntentParser::from_csv_file(&path)
                .map_err(|_| format!("failed to load NLP CSV '{path}'"))?;
            Some(Arc::new(parser))
        }
        None => None,
    };

    Ok(SpiBackends {
        position_base,
        trade_feed,
        market_quotes,
        nlp_parser,
        spi_units: Some(units),
    })
}

fn parse_bool_env(name: &str, raw: Option<&str>) -> Result<bool, String> {
    match raw {
        Some("1" | "true" | "TRUE") => Ok(true),
        None | Some("0" | "false" | "FALSE" | "") => Ok(false),
        Some(other) => Err(format!("invalid boolean value '{other}' for {name}")),
    }
}

fn resolve_listen_addr() -> Result<SocketAddr, String> {
    let raw = env::var("FICANT_SERVER_ADDR").unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.to_owned());
    raw.parse()
        .map_err(|err| format!("invalid FICANT_SERVER_ADDR '{raw}': {err}"))
}
