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

pub mod cli;
pub mod clock;
pub mod policy;
pub mod store_adapter;
pub mod web_ui;

use std::env;
use std::fs;
use std::io::{BufRead, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use tokio::sync::{mpsc, oneshot, watch};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

pub use cli::{
    CliCommand, parse_cli_args, parse_cli_args_os, run_sqlite_rollover_json,
    run_verify_sqlite_archive_json,
};
pub use clock::SystemBlotterClock;
pub use policy::{
    AccountPolicyJson, AccountPolicyOverrideJson, BlotterConfigJson, DeskBlotterPolicyFile,
    DeskBlotterPolicyRuntime, EMBEDDED_DEFAULT_POLICY_JSON, RulePackBindingJson,
};
pub use store_adapter::{DEFAULT_EVENT_BUS_CAPACITY, SqliteBlotterRuntime};
pub use web_ui::{
    AssetLoadHookStage, HttpEdgeLayer, HttpEdgeState, MAX_WEB_ASSETS_FILE_COUNT,
    MAX_WEB_ASSETS_TOTAL_BYTES, PINNED_RULEPACK_ASSET_PATH, PINNED_RULEPACK_PAYLOAD_BYTES,
    PINNED_RULEPACK_PAYLOAD_SHA256, WebAsset, WebAssetCatalog, WebUiConfig, WebUiMode,
    embedded_assets_count, load_web_asset_catalog_from_dir,
    load_web_asset_catalog_from_dir_with_hook, parse_optional_bool_env, parse_web_ui_env_values,
    resolve_web_ui, resolve_web_ui_with_embedded, sanitize_static_request_path,
    web_ui_config_from_env,
};

pub const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:50051";
pub const DEFAULT_WATCHDOG_INTERVAL: Duration = Duration::from_secs(5);
pub const DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

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

impl BlotterStorageMode {
    #[must_use]
    pub const fn as_healthz_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::PersistentWal { .. } => "persistent_wal",
            Self::EphemeralInMemory => "ephemeral_in_memory",
        }
    }
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
    web_ui_mode: WebUiMode,
    runtime: Option<Arc<SqliteBlotterRuntime>>,
    watchdog: Option<WatchdogController>,
    stream_shutdown_tx: Option<watch::Sender<bool>>,
    stream_drain_rx: Option<mpsc::Receiver<()>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<tokio::task::JoinHandle<Result<(), String>>>,
}

impl ServerHandle {
    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    #[must_use]
    pub const fn web_ui_mode(&self) -> WebUiMode {
        self.web_ui_mode
    }

    #[must_use]
    pub const fn runtime(&self) -> Option<&Arc<SqliteBlotterRuntime>> {
        self.runtime.as_ref()
    }

    #[must_use]
    pub const fn watchdog(&self) -> Option<&WatchdogController> {
        self.watchdog.as_ref()
    }

    /// Gracefully shuts down active streams, background watchdog, and the gRPC/HTTP server within
    /// a bounded 5-second drain budget, deterministically releasing any exclusive WAL and
    /// companion rollover locks (`F-09`).
    pub async fn shutdown(mut self) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT;

        if let Some(tx) = self.stream_shutdown_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(wd) = self.watchdog.take()
            && tokio::time::timeout_at(deadline, wd.shutdown_and_wait())
                .await
                .is_err()
        {
            return Err("watchdog shutdown timed out".to_owned());
        }
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(mut jh) = self.join_handle.take() {
            if let Ok(joined) = tokio::time::timeout_at(deadline, &mut jh).await {
                joined.map_err(|err| format!("server task join failed: {err}"))??;
            } else {
                jh.abort();
                return Err("server graceful shutdown drain timed out".to_owned());
            }
        }
        if let Some(mut rx) = self.stream_drain_rx.take()
            && tokio::time::timeout_at(deadline, rx.recv()).await.is_err()
        {
            return Err("active stream task drain timed out".to_owned());
        }
        drop(self.runtime.take());
        Ok(())
    }

    async fn wait_until_shutdown_or_exit(
        mut self,
        shutdown_signal: oneshot::Receiver<()>,
    ) -> Result<(), String> {
        if let Some(mut jh) = self.join_handle.take() {
            tokio::select! {
                res = &mut jh => {
                    if let Some(tx) = self.stream_shutdown_tx.take() {
                        let _ = tx.send(true);
                    }
                    if let Some(wd) = self.watchdog.take() {
                        wd.shutdown_and_wait().await;
                    }
                    drop(self.runtime.take());
                    res.map_err(|err| format!("server task join failed: {err}"))??;
                    Ok(())
                }
                _ = shutdown_signal => {
                    self.join_handle = Some(jh);
                    self.shutdown().await
                }
            }
        } else {
            self.shutdown().await
        }
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
/// accepting any incoming requests. Defaults static Web UI to [`WebUiConfig::Disabled`] for
/// backward compatibility.
pub async fn spawn_server(config: FicantServerConfig) -> Result<ServerHandle, String> {
    spawn_server_with_web_ui(config, WebUiConfig::Disabled).await
}

/// Starts a single-port `ficant-server` with explicit [`WebUiConfig`] (`F-10`, `F-15`, `F-09`).
#[allow(clippy::too_many_lines)]
pub async fn spawn_server_with_web_ui(
    config: FicantServerConfig,
    web_ui: WebUiConfig,
) -> Result<ServerHandle, String> {
    config.service_config.validate()?;
    let (web_ui_mode, catalog) = resolve_web_ui(&web_ui)?;
    let reference_units = match config.spi_backends.spi_units.clone() {
        Some(u) => u,
        None => default_reference_analysis_units()
            .map_err(|_| "failed to construct default analysis units".to_owned())?,
    };

    let (stream_shutdown_tx, stream_shutdown_rx) = watch::channel(false);
    let (stream_drain_tx, stream_drain_rx) = mpsc::channel::<()>(1);
    let storage_mode_str = config.storage_mode.as_healthz_str();
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
            TradeFeedReconcilerServiceServer::new(
                TradeFeedReconcilerGrpcService::disabled(
                    config.clock.clone(),
                    reference_units.clone(),
                )
                .with_shutdown_signal(stream_shutdown_rx.clone())
                .with_stream_drain_guard(stream_drain_tx.clone()),
            ),
            MarketQuoteStreamServiceServer::new(
                MarketQuoteStreamGrpcService::disabled(
                    config.clock.clone(),
                    reference_units.clone(),
                )
                .with_shutdown_signal(stream_shutdown_rx.clone())
                .with_stream_drain_guard(stream_drain_tx.clone()),
            ),
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
            build_enabled_blotter_stack(
                store,
                credentials_port,
                &config,
                reference_units,
                &stream_shutdown_rx,
                &stream_drain_tx,
            )?
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
            build_enabled_blotter_stack(
                store,
                credentials_port,
                &config,
                reference_units,
                &stream_shutdown_rx,
                &stream_drain_tx,
            )?
        }
    };
    drop(stream_drain_tx);

    let listener = TcpListener::bind(config.listen_addr)
        .await
        .map_err(|err| format!("failed to bind {}: {err}", config.listen_addr))?;
    let bound_addr = listener
        .local_addr()
        .map_err(|err| format!("failed to read bound addr: {err}"))?;
    let incoming = TcpListenerStream::new(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let edge_state = HttpEdgeState {
        storage_mode_str,
        runtime: runtime_opt.clone(),
        web_ui_mode,
        catalog,
        embedded_assets_count: embedded_assets_count(),
    };

    let mut router = Server::builder()
        .accept_http1(true)
        .layer(HttpEdgeLayer::new(edge_state))
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
        web_ui_mode,
        runtime: runtime_opt,
        watchdog: watchdog_opt,
        stream_shutdown_tx: Some(stream_shutdown_tx),
        stream_drain_rx: Some(stream_drain_rx),
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
    stream_shutdown_rx: &watch::Receiver<bool>,
    stream_drain_tx: &mpsc::Sender<()>,
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
    )?
    .with_shutdown_signal(stream_shutdown_rx.clone())
    .with_stream_drain_guard(stream_drain_tx.clone());

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
        TradeFeedReconcilerServiceServer::new(
            TradeFeedReconcilerGrpcService::new(
                feed_port,
                credentials.clone(),
                config.clock.clone(),
                reference_units.clone(),
            )
            .with_shutdown_signal(stream_shutdown_rx.clone())
            .with_stream_drain_guard(stream_drain_tx.clone()),
        ),
        MarketQuoteStreamServiceServer::new(
            MarketQuoteStreamGrpcService::new(
                quote_port,
                credentials.clone(),
                config.clock.clone(),
                reference_units.clone(),
            )
            .with_shutdown_signal(stream_shutdown_rx.clone())
            .with_stream_drain_guard(stream_drain_tx.clone()),
        ),
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

/// Maps an unspecified listen address (`0.0.0.0:p` or `[::]:p`) to its corresponding loopback
/// address (`127.0.0.1:p` or `[::1]:p`) for `--health-check` probing (`AC-004`).
#[must_use]
pub const fn map_health_check_addr(addr: SocketAddr) -> SocketAddr {
    match addr {
        SocketAddr::V4(v4) if v4.ip().is_unspecified() => {
            SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), v4.port())
        }
        SocketAddr::V6(v6) if v6.ip().is_unspecified() => {
            SocketAddr::new(std::net::IpAddr::V6(Ipv6Addr::LOCALHOST), v6.port())
        }
        other => other,
    }
}

/// Sends a bounded HTTP/1.1 `GET /healthz` probe to `addr` and validates HTTP `200`,
/// `status == "ok"`, `blotter_healthy == true`, and `service == "ficant-server"`.
pub fn probe_healthz(addr: SocketAddr, timeout: Duration) -> Result<(), String> {
    let probe = map_health_check_addr(addr);
    let deadline = Instant::now() + timeout;
    let remaining = || -> Result<Duration, String> {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| format!("health check timed out for {probe}"))
    };

    let mut stream = TcpStream::connect_timeout(&probe, remaining()?)
        .map_err(|err| format!("health check failed for {probe}: {err}"))?;
    stream
        .set_write_timeout(Some(remaining()?))
        .map_err(|err| format!("health check failed to set write timeout: {err}"))?;
    stream
        .set_read_timeout(Some(remaining()?))
        .map_err(|err| format!("health check failed to set read timeout: {err}"))?;

    let req = format!("GET /healthz HTTP/1.1\r\nHost: {probe}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|err| format!("health check write failed for {probe}: {err}"))?;
    stream
        .flush()
        .map_err(|err| format!("health check flush failed for {probe}: {err}"))?;

    let mut raw = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        stream
            .set_read_timeout(Some(remaining()?))
            .map_err(|err| format!("health check failed to update read timeout: {err}"))?;
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if raw.len().saturating_add(n) > 64 * 1024 {
                    return Err("health check response exceeded 64 KiB limit".to_owned());
                }
                raw.extend_from_slice(&buf[..n]);
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => return Err(format!("health check read failed for {probe}: {err}")),
        }
    }

    let Some(sep) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Err(format!("malformed HTTP response from {probe}/healthz"));
    };
    let head = std::str::from_utf8(&raw[..sep])
        .map_err(|_| format!("non-UTF-8 HTTP headers from {probe}/healthz"))?;
    let status_line = head.lines().next().unwrap_or("");
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("invalid HTTP status line '{status_line}'"))?;
    if status_code != 200 {
        return Err(format!("healthz returned HTTP {status_code}"));
    }

    let body_raw = &raw[sep + 4..];
    let body_bytes = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked_body(body_raw)?
    } else {
        body_raw.to_vec()
    };

    let json: serde_json::Value = serde_json::from_slice(&body_bytes)
        .map_err(|err| format!("invalid JSON from /healthz: {err}"))?;
    let status = json.get("status").and_then(serde_json::Value::as_str);
    let service = json.get("service").and_then(serde_json::Value::as_str);
    let blotter_healthy = json
        .get("blotter_healthy")
        .and_then(serde_json::Value::as_bool);

    if status != Some("ok") || service != Some("ficant-server") || blotter_healthy != Some(true) {
        return Err(format!(
            "unhealthy /healthz response: status={status:?}, service={service:?}, blotter_healthy={blotter_healthy:?}"
        ));
    }
    Ok(())
}

fn decode_chunked_body(mut slice: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    while let Some(pos) = slice.windows(2).position(|w| w == b"\r\n") {
        let len_str = std::str::from_utf8(&slice[..pos])
            .map_err(|_| "invalid chunk size encoding".to_owned())?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let sz =
            usize::from_str_radix(len_str, 16).map_err(|_| "invalid hex chunk size".to_owned())?;
        slice = &slice[pos + 2..];
        if sz == 0 {
            return Ok(out);
        }
        if slice.len() < sz + 2 {
            return Err("truncated chunked body".to_owned());
        }
        out.extend_from_slice(&slice[..sz]);
        slice = &slice[sz + 2..];
    }
    Err("unterminated chunked body".to_owned())
}

pub fn health_check_from_env() -> Result<(), String> {
    let addr = resolve_listen_addr()?;
    probe_healthz(addr, DEFAULT_HEALTH_CHECK_TIMEOUT)
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
    let web_ui = web_ui_config_from_env()?;
    let stdin_shutdown_env = env::var("FICANT_ENABLE_STDIN_SHUTDOWN").ok();
    let stdin_shutdown_enabled = parse_bool_env(
        "FICANT_ENABLE_STDIN_SHUTDOWN",
        stdin_shutdown_env.as_deref(),
    )?;

    let handle = spawn_server_with_web_ui(config, web_ui).await?;
    if stdin_shutdown_enabled {
        let (trigger_tx, trigger_rx) = oneshot::channel::<()>();
        std::thread::Builder::new()
            .name("ficant-stdin-shutdown".to_owned())
            .spawn(move || {
                let stdin = std::io::stdin();
                let mut locked = stdin.lock();
                let mut line = String::new();
                loop {
                    line.clear();
                    match locked.read_line(&mut line) {
                        Ok(0) | Err(_) => {
                            let _ = trigger_tx.send(());
                            break;
                        }
                        Ok(_) => {
                            if line.trim_end_matches(['\r', '\n']) == "SHUTDOWN" {
                                let _ = trigger_tx.send(());
                                break;
                            }
                        }
                    }
                }
            })
            .map_err(|err| format!("failed to spawn stdin shutdown watcher: {err}"))?;
        return handle.wait_until_shutdown_or_exit(trigger_rx).await;
    }

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
    Ok(parse_optional_bool_env(name, raw)?.unwrap_or(false))
}

fn resolve_listen_addr() -> Result<SocketAddr, String> {
    let raw = env::var("FICANT_SERVER_ADDR").unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.to_owned());
    raw.parse()
        .map_err(|err| format!("invalid FICANT_SERVER_ADDR '{raw}': {err}"))
}
