#![allow(
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::missing_panics_doc
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{NaiveDate, TimeZone};
use chrono_tz::Asia::Shanghai;
use ficant_api::{
    AUTHORIZATION_HEADER, BlotterLedgerIdentity, BlotterStorePort, BlotterWriteOutcome,
    CredentialEntry, DeskBlotterGrpcService, DeskBlotterServiceConfig,
    EXPECTED_LEDGER_EPOCH_HEADER, EXPECTED_LEDGER_INSTANCE_HEADER, FICANT_CREDENTIAL_HEADER,
    IntranetNlpIntentParserGrpcService, MarketQuoteStreamGrpcService, OwnedBlotterEngines,
    PositionBaseProviderGrpcService, StaticCredentialVerifier, SubscribeBarrierSnapshot,
    TradeFeedReconcilerGrpcService, WriteAuthorizeFn, WriteAuthorizeWithIdentityFn,
    WriteTransitionFn, extract_expected_ledger_fence, is_canonical_ulid,
};
use ficant_application::ports::{
    ApplicationResult, BlotterEventJournalPort, DeskBlotterPolicyProvider, IdempotencyScopeKey,
    ManualBlotterClock, PollEventsOutcome, PrincipalRole,
};
use ficant_application::use_cases::blotter::{
    AccountDeskPolicy, BlotterEventEnvelope, DeskBlotterWorkspaceState,
};
use ficant_blotter_store::{
    CsvMarketQuoteStream, CsvPositionBaseProvider, CsvTemplateNlpIntentParser,
    CsvTradeFeedReconciler, MockMarketQuoteStream, MockNlpIntentParser, MockPositionBaseProvider,
    MockTradeFeedReconciler, SqliteBlotterOpenOptions, SqliteBlotterStore, WriteExecutionOutcome,
    default_reference_analysis_units,
};
use ficant_cgb_futures_pack::{
    CgbFuturesDeliveryRulePackParser, MARKET as FUTURES_MARKET, RULE_TYPE as FUTURES_RULE_TYPE,
    TYPE_URL as FUTURES_TYPE_URL,
};
use ficant_contracts::ficant::blotter::v1 as pb;
use ficant_contracts::ficant::blotter::v1::desk_blotter_service_client::DeskBlotterServiceClient;
use ficant_contracts::ficant::blotter::v1::desk_blotter_service_server::{
    DeskBlotterService, DeskBlotterServiceServer,
};
use ficant_contracts::ficant::blotter::v1::intranet_nlp_intent_parser_service_client::IntranetNlpIntentParserServiceClient;
use ficant_contracts::ficant::blotter::v1::intranet_nlp_intent_parser_service_server::IntranetNlpIntentParserServiceServer;
use ficant_contracts::ficant::blotter::v1::market_quote_stream_service_client::MarketQuoteStreamServiceClient;
use ficant_contracts::ficant::blotter::v1::market_quote_stream_service_server::MarketQuoteStreamServiceServer;
use ficant_contracts::ficant::blotter::v1::position_base_provider_service_client::PositionBaseProviderServiceClient;
use ficant_contracts::ficant::blotter::v1::position_base_provider_service_server::PositionBaseProviderServiceServer;
use ficant_contracts::ficant::blotter::v1::trade_feed_reconciler_service_client::TradeFeedReconcilerServiceClient;
use ficant_contracts::ficant::blotter::v1::trade_feed_reconciler_service_server::TradeFeedReconcilerServiceServer;
use ficant_contracts::ficant::core::v1 as core_pb;
use ficant_contracts::ficant::market::v1 as market_pb;
use ficant_contracts::ficant::rates::v1 as rates_pb;
use ficant_domain::analytics::{FixedDecimal, MARKET_TIMEZONE};
use ficant_domain::blotter::BlotterConfig;
use ficant_domain::market::{
    MarketRulePack, MarketRulePackInput, RulePackContent, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version,
};
use ficant_fixed_income_native::{NativeBondAnalyticsEngine, NativeFuturesHedgeEngine};
use ficant_regulatory_pack::{
    MARKET as REG_MARKET, RULE_TYPE as REG_RULE_TYPE, RegulatoryRulePackV1Parser,
    TYPE_URL as REG_TYPE_URL,
};
use prost::Message;
use rust_decimal::Decimal;
use std::str::FromStr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::transport::Server;

const TRADER_A_TOKEN: &str = "tok-trader-a-high-entropy-0001";
const TRADER_AB_TOKEN: &str = "tok-trader-ab-high-entropy-0002";
const PARSER_TOKEN: &str = "tok-nlp-parser-high-entropy-0003";
const FEED_A_ONLY_TOKEN: &str = "tok-feed-a-only-high-entropy-0004";
const FEED_AB_TOKEN: &str = "tok-feed-ab-high-entropy-0005";
const OPERATOR_TOKEN: &str = "tok-operator-high-entropy-0006";

// ============================================================================
// Test Store Adapter + Policy Provider + Dual-Protocol Client Harness
// ============================================================================

struct TestStoreAdapter {
    store: Arc<SqliteBlotterStore>,
    event_tx: broadcast::Sender<BlotterEventEnvelope>,
}

impl TestStoreAdapter {
    fn new(store: Arc<SqliteBlotterStore>, bus_cap: usize) -> Self {
        let (event_tx, _rx) = broadcast::channel(bus_cap.max(1));
        let tx_hook = event_tx.clone();
        store.set_event_commit_hook(Some(Arc::new(move |ev: &BlotterEventEnvelope| {
            let _ = tx_hook.send(ev.clone());
        })));
        Self { store, event_tx }
    }
}

impl BlotterStorePort for TestStoreAdapter {
    fn execute_write_authorized_bytes(
        &self,
        idempotency: Option<(&IdempotencyScopeKey, &ContentHash)>,
        authorize: WriteAuthorizeFn,
        transition: WriteTransitionFn,
    ) -> ApplicationResult<BlotterWriteOutcome> {
        let outcome = self
            .store
            .execute_write_authorized(idempotency, authorize, transition)?;
        Ok(match outcome {
            WriteExecutionOutcome::Committed {
                value,
                committed_seq,
            } => BlotterWriteOutcome::Committed {
                response_bytes: value,
                committed_seq,
            },
            WriteExecutionOutcome::Replayed(r) => BlotterWriteOutcome::Replayed(r),
        })
    }

    fn execute_write_authorized_with_identity_bytes(
        &self,
        idempotency: Option<(&IdempotencyScopeKey, &ContentHash)>,
        authorize: WriteAuthorizeWithIdentityFn,
        transition: WriteTransitionFn,
    ) -> ApplicationResult<BlotterWriteOutcome> {
        let outcome = self.store.execute_write_authorized_with_identity(
            idempotency,
            |state, id| {
                let api_id = BlotterLedgerIdentity {
                    ledger_epoch: id.ledger_epoch,
                    ledger_instance_id: id.ledger_instance_id.clone(),
                };
                authorize(state, &api_id)
            },
            transition,
        )?;
        Ok(match outcome {
            WriteExecutionOutcome::Committed {
                value,
                committed_seq,
            } => BlotterWriteOutcome::Committed {
                response_bytes: value,
                committed_seq,
            },
            WriteExecutionOutcome::Replayed(r) => BlotterWriteOutcome::Replayed(r),
        })
    }

    fn read_workspace_snapshot(&self) -> ApplicationResult<(DeskBlotterWorkspaceState, u64, u64)> {
        self.store.with_read_barrier(|s, h, o| (s.clone(), h, o))
    }

    fn subscribe_with_barrier(&self) -> ApplicationResult<SubscribeBarrierSnapshot> {
        self.store.with_read_barrier(|_, head, oldest| {
            let receiver = self.event_tx.subscribe();
            SubscribeBarrierSnapshot {
                head_seq: head,
                oldest_retained_seq: oldest,
                receiver,
            }
        })
    }

    fn poll_events(
        &self,
        after_seq: u64,
        max_events: u32,
        account_filter: Option<&str>,
        allowed_accounts: Option<&BTreeSet<String>>,
    ) -> ApplicationResult<PollEventsOutcome> {
        self.store
            .poll_events(after_seq, max_events, account_filter, allowed_accounts)
    }
}

struct TestPolicyProvider {
    blotter_config: BlotterConfig,
    policies: Mutex<BTreeMap<String, AccountDeskPolicy>>,
    default_policy: AccountDeskPolicy,
}

impl TestPolicyProvider {
    fn new(default_policy: AccountDeskPolicy) -> Self {
        Self {
            blotter_config: BlotterConfig::default(),
            policies: Mutex::new(BTreeMap::new()),
            default_policy,
        }
    }

    fn set_account_policy(&self, account_id: &str, policy: AccountDeskPolicy) {
        self.policies
            .lock()
            .unwrap()
            .insert(account_id.to_owned(), policy);
    }
}

impl DeskBlotterPolicyProvider for TestPolicyProvider {
    fn blotter_config(&self) -> BlotterConfig {
        self.blotter_config.clone()
    }

    fn account_policy(&self, account_id: &str) -> ApplicationResult<AccountDeskPolicy> {
        Ok(self
            .policies
            .lock()
            .unwrap()
            .get(account_id)
            .cloned()
            .unwrap_or_else(|| self.default_policy.clone()))
    }
}

struct LiveTestServer {
    addr: SocketAddr,
    clock: Arc<ManualBlotterClock>,
    verifier: Arc<StaticCredentialVerifier>,
    store: Arc<SqliteBlotterStore>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl LiveTestServer {
    async fn start(
        policy_provider: Arc<TestPolicyProvider>,
        service_config: DeskBlotterServiceConfig,
        retention_window: u64,
        bus_capacity: usize,
    ) -> Self {
        Self::start_with_spis(
            policy_provider,
            service_config,
            retention_window,
            bus_capacity,
            Arc::new(MockPositionBaseProvider::new()),
            Arc::new(MockTradeFeedReconciler::new()),
            Arc::new(MockMarketQuoteStream::new()),
            Arc::new(MockNlpIntentParser::default()),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_with_spis(
        policy_provider: Arc<TestPolicyProvider>,
        service_config: DeskBlotterServiceConfig,
        retention_window: u64,
        bus_capacity: usize,
        pos_spi: Arc<dyn ficant_application::ports::PositionBaseProviderPort>,
        feed_spi: Arc<dyn ficant_application::ports::TradeFeedReconcilerPort>,
        quote_spi: Arc<dyn ficant_application::ports::MarketQuoteStreamPort>,
        nlp_spi: Arc<dyn ficant_application::ports::NlpIntentParserPort>,
    ) -> Self {
        let clock = Arc::new(ManualBlotterClock::new(shanghai_time(
            2026, 6, 15, 10, 0, 0,
        )));
        let verifier = Arc::new(build_test_verifier());
        let opts = SqliteBlotterOpenOptions::ephemeral_in_memory(policy_provider.blotter_config())
            .with_event_retention_window(retention_window);
        let store = Arc::new(SqliteBlotterStore::open(&opts).expect("open ephemeral store"));
        let adapter = Arc::new(TestStoreAdapter::new(store.clone(), bus_capacity));
        let engines = OwnedBlotterEngines {
            bond_engine: Arc::new(NativeBondAnalyticsEngine),
            hedge_engine: Arc::new(NativeFuturesHedgeEngine),
            delivery_rule_parser: Arc::new(CgbFuturesDeliveryRulePackParser),
            regulatory_rule_parser: Arc::new(RegulatoryRulePackV1Parser),
        };
        let blotter_svc = DeskBlotterGrpcService::new(
            adapter,
            policy_provider,
            verifier.clone(),
            clock.clone(),
            engines,
            service_config,
        )
        .expect("valid blotter config");

        let units = default_reference_analysis_units().unwrap();
        let pos_svc =
            PositionBaseProviderGrpcService::new(pos_spi, verifier.clone(), clock.clone());
        let feed_svc = TradeFeedReconcilerGrpcService::new(
            feed_spi,
            verifier.clone(),
            clock.clone(),
            units.clone(),
        );
        let quote_svc = MarketQuoteStreamGrpcService::new(
            quote_spi,
            verifier.clone(),
            clock.clone(),
            units.clone(),
        );
        let nlp_svc = IntranetNlpIntentParserGrpcService::new(
            nlp_spi,
            verifier.clone(),
            clock.clone(),
            units,
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().unwrap();
        let incoming = TcpListenerStream::new(listener);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            let _ = Server::builder()
                .accept_http1(true)
                .layer(tonic_web::GrpcWebLayer::new())
                .add_service(DeskBlotterServiceServer::new(blotter_svc))
                .add_service(PositionBaseProviderServiceServer::new(pos_svc))
                .add_service(TradeFeedReconcilerServiceServer::new(feed_svc))
                .add_service(MarketQuoteStreamServiceServer::new(quote_svc))
                .add_service(IntranetNlpIntentParserServiceServer::new(nlp_svc))
                .serve_with_incoming_shutdown(incoming, async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        Self {
            addr,
            clock,
            verifier,
            store,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    async fn client(&self) -> DeskBlotterServiceClient<tonic::transport::Channel> {
        DeskBlotterServiceClient::connect(format!("http://{}", self.addr))
            .await
            .expect("connect tonic client")
    }
}

impl Drop for LiveTestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

fn build_test_verifier() -> StaticCredentialVerifier {
    StaticCredentialVerifier::new(vec![
        CredentialEntry::from_plaintext_token(
            "trader-a",
            PrincipalRole::Trader,
            TRADER_A_TOKEN,
            ["acct-A".to_owned()],
            [
                "SRC-SHARED".to_owned(),
                "Q-SHARED".to_owned(),
                "Q-B".to_owned(),
            ],
        ),
        CredentialEntry::from_plaintext_token(
            "trader-ab",
            PrincipalRole::Trader,
            TRADER_AB_TOKEN,
            ["acct-A".to_owned(), "acct-B".to_owned()],
            [
                "SRC-SHARED".to_owned(),
                "Q-SHARED".to_owned(),
                "Q-B".to_owned(),
            ],
        ),
        CredentialEntry::from_plaintext_token(
            "nlp-bot-1",
            PrincipalRole::NlpParser,
            PARSER_TOKEN,
            ["acct-A".to_owned(), "acct-B".to_owned()],
            Vec::<String>::new(),
        ),
        CredentialEntry::from_plaintext_token(
            "feed-a-only",
            PrincipalRole::FeedAdapter,
            FEED_A_ONLY_TOKEN,
            ["acct-A".to_owned()],
            ["SRC-SHARED".to_owned(), "Q-SHARED".to_owned()],
        ),
        CredentialEntry::from_plaintext_token(
            "feed-ab",
            PrincipalRole::FeedAdapter,
            FEED_AB_TOKEN,
            ["acct-A".to_owned(), "acct-B".to_owned()],
            [
                "SRC-SHARED".to_owned(),
                "Q-SHARED".to_owned(),
                "Q-B".to_owned(),
            ],
        ),
        CredentialEntry::from_plaintext_token(
            "operator-1",
            PrincipalRole::Operator,
            OPERATOR_TOKEN,
            ["acct-A".to_owned(), "acct-B".to_owned()],
            [
                "SRC-SHARED".to_owned(),
                "Q-SHARED".to_owned(),
                "Q-B".to_owned(),
            ],
        ),
    ])
    .expect("valid test verifier")
}

// ============================================================================
// Raw HTTP/1.1 gRPC-Web Client Framing (application/grpc-web+proto)
// ============================================================================

async fn grpc_web_unary<Req: Message, Resp: Message + Default>(
    addr: SocketAddr,
    path: &str,
    req: &Req,
    auth_header: Option<(&str, &str)>,
) -> Resp {
    let frames = grpc_web_call_raw(addr, path, req, auth_header).await;
    assert_eq!(frames.len(), 1, "expected 1 data frame for unary gRPC-Web");
    Resp::decode(frames[0].as_slice()).expect("decode gRPC-Web unary response")
}

async fn grpc_web_call_raw<Req: Message>(
    addr: SocketAddr,
    path: &str,
    req: &Req,
    auth_header: Option<(&str, &str)>,
) -> Vec<Vec<u8>> {
    let payload = req.encode_to_vec();
    let mut framed = Vec::with_capacity(5 + payload.len());
    framed.push(0x00);
    framed.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
    framed.extend_from_slice(&payload);

    let mut headers = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/grpc-web+proto\r\nAccept: application/grpc-web+proto\r\nX-Grpc-Web: 1\r\nContent-Length: {}\r\nConnection: close\r\n",
        framed.len()
    );
    if let Some((k, v)) = auth_header {
        let _ = write!(headers, "{k}: {v}\r\n");
    }
    headers.push_str("\r\n");

    let mut stream = TcpStream::connect(addr).await.expect("tcp connect");
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&framed).await.unwrap();
    stream.flush().await.unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("HTTP header separator");
    let header_text = String::from_utf8_lossy(&raw[..sep]);
    assert!(
        header_text.starts_with("HTTP/1.1 200"),
        "expected HTTP 200, got: {header_text}"
    );
    let body_raw = &raw[sep + 4..];
    let body = if header_text
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked_body(body_raw)
    } else {
        body_raw.to_vec()
    };

    let mut pos = 0_usize;
    let mut data_frames = Vec::new();
    let mut saw_trailer = false;
    while pos + 5 <= body.len() {
        let flag = body[pos];
        let len = u32::from_be_bytes([body[pos + 1], body[pos + 2], body[pos + 3], body[pos + 4]])
            as usize;
        pos += 5;
        assert!(pos + len <= body.len(), "truncated gRPC-Web frame");
        let slice = &body[pos..pos + len];
        pos += len;
        if flag == 0x00 {
            data_frames.push(slice.to_vec());
        } else if flag == 0x80 {
            saw_trailer = true;
            let trailer_text = String::from_utf8_lossy(slice);
            assert!(
                trailer_text.contains("grpc-status:0") || trailer_text.contains("grpc-status: 0"),
                "unexpected gRPC-Web trailer: {trailer_text}"
            );
        }
    }
    assert!(saw_trailer, "expected 0x80 gRPC-Web trailer frame");
    data_frames
}

fn decode_chunked_body(mut raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(crlf) = raw.windows(2).position(|w| w == b"\r\n") {
        let size_str = String::from_utf8_lossy(&raw[..crlf]);
        let size = usize::from_str_radix(size_str.trim(), 16).unwrap_or(0);
        raw = &raw[crlf + 2..];
        if size == 0 {
            break;
        }
        out.extend_from_slice(&raw[..size]);
        raw = &raw[size + 2..];
    }
    out
}

// ============================================================================
// Fixture Builders
// ============================================================================

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn with_bearer<T>(msg: T, token: &str) -> Request<T> {
    let mut req = Request::new(msg);
    req.metadata_mut().insert(
        AUTHORIZATION_HEADER,
        MetadataValue::try_from(format!("Bearer {token}")).unwrap(),
    );
    req
}

fn with_x_cred<T>(msg: T, token: &str) -> Request<T> {
    let mut req = Request::new(msg);
    req.metadata_mut().insert(
        FICANT_CREDENTIAL_HEADER,
        MetadataValue::try_from(token).unwrap(),
    );
    req
}

fn ulid(suffix: &str) -> core_pb::Ulid {
    core_pb::Ulid {
        value: format!("01ARZ3NDEKTSV4RRFFQ69G5F{suffix}"),
    }
}

fn shanghai_time(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> MarketTime {
    let naive = NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(hh, mm, ss)
        .unwrap();
    let utc = Shanghai
        .from_local_datetime(&naive)
        .single()
        .unwrap()
        .to_utc();
    MarketTime::new(
        utc,
        MARKET_TIMEZONE,
        NaiveDate::from_ymd_opt(y, m, d).unwrap(),
    )
    .unwrap()
}

fn pb_time(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> core_pb::MarketTime {
    ficant_api::blotter_codec::to_pb_market_time(&shanghai_time(y, m, d, hh, mm, ss))
}

fn pb_units() -> rates_pb::AnalysisUnits {
    let u = default_reference_analysis_units().unwrap();
    ficant_api::blotter_codec::to_pb_analysis_units(&u)
}

fn dec(val: &str, unit: &core_pb::UnitRef) -> core_pb::DecimalValue {
    let d = Decimal::from_str(val).unwrap().normalize();
    core_pb::DecimalValue {
        coefficient: d.mantissa().to_string(),
        scale: d.scale(),
        unit: Some(unit.clone()),
    }
}

fn fixed(val: &str) -> FixedDecimal {
    ficant_blotter_store::parse_fixed_decimal(val).unwrap()
}

fn verified_regulatory_rule_pack(
    version: u64,
    effective_to_year: i32,
    tamper_hash: bool,
    status: VerificationStatus,
) -> MarketRulePack {
    let payload = market_pb::RegulatoryRulePack {
        constraints: vec![market_pb::ConstraintDefinition {
            limit_code: "MAX_INST_FACE".to_owned(),
            measure: market_pb::ConstraintMeasure::InstrumentFace as i32,
        }],
    };
    let raw = payload.encode_to_vec();
    let hash = if tamper_hash {
        ContentHash::from_bytes(&[0xEE; 32]).unwrap()
    } else {
        ContentHash::digest(&raw)
    };
    let owner_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
            version: Version::new(version).unwrap(),
            owner: OwnerRef::new(owner_id.clone(), owner_id),
            market: REG_MARKET.to_owned(),
            rule_type: REG_RULE_TYPE.to_owned(),
            source: "desk-risk-policy".to_owned(),
            effective: EffectivePeriod::new(
                shanghai_time(2026, 1, 1, 0, 0, 0),
                shanghai_time(effective_to_year, 1, 1, 0, 0, 0),
            )
            .unwrap(),
            verification_status: status,
            content_hash: hash,
        },
        RulePackContent::new(REG_TYPE_URL.to_owned(), raw).unwrap(),
    )
    .unwrap()
}

fn default_test_policy(with_reg_pack: bool) -> AccountDeskPolicy {
    let mut ceilings = BTreeMap::new();
    ceilings.insert("MAX_INST_FACE".to_owned(), fixed("500000000"));
    AccountDeskPolicy {
        policy_version: 1,
        default_quote_source_id: "Q-SHARED".to_owned(),
        instrument_quote_sources: BTreeMap::new(),
        quote_staleness_seconds: 300,
        default_draft_hold_ttl_seconds: 1800,
        allowed_hedge_product_codes: vec!["T".to_owned(), "TF".to_owned(), "TL".to_owned()],
        regulatory_rule_pack: with_reg_pack
            .then(|| verified_regulatory_rule_pack(1, 2028, false, VerificationStatus::Verified)),
        limit_ceilings: ceilings,
    }
}

fn pb_bond(
    id_suffix: &str,
    face_value: &str,
    first_issue: &str,
    maturity: &str,
    coupon: &str,
) -> market_pb::Bond {
    let u = pb_units();
    market_pb::Bond {
        instrument: Some(core_pb::VersionRef {
            id: Some(ulid(id_suffix)),
            version: 1,
        }),
        first_issue_date: first_issue.to_owned(),
        current_issue_date: first_issue.to_owned(),
        maturity_date: maturity.to_owned(),
        face_value: Some(dec(face_value, u.currency_amount.as_ref().unwrap())),
        cumulative_issued_amount: Some(dec("50000000000", u.currency_amount.as_ref().unwrap())),
        tax_attributes: Some(market_pb::BondTaxAttributes {
            value_added_tax_status: market_pb::ValueAddedTaxStatus::Exempt as i32,
            income_tax_status: market_pb::IncomeTaxStatus::Exempt as i32,
        }),
        coupon_rate: Some(dec(coupon, u.rate.as_ref().unwrap())),
        coupon_frequency: market_pb::BondCouponFrequency::Semiannual as i32,
        day_count: market_pb::BondDayCountConvention::ActActBondIsma as i32,
        business_day: market_pb::BondBusinessDayConvention::Following as i32,
    }
}

fn pb_calendar(start: &str, end: &str) -> market_pb::Calendar {
    market_pb::Calendar {
        calendar_id: Some(ulid("CA")),
        version: 1,
        owner: None,
        market: "INTERBANK".to_owned(),
        market_timezone: MARKET_TIMEZONE.to_owned(),
        effective_from: Some(pb_time(2024, 1, 1, 0, 0, 0)),
        effective_to: Some(pb_time(2036, 1, 1, 0, 0, 0)),
        sessions: vec![
            market_pb::CalendarSession {
                local_date: start.to_owned(),
                open_local_time: "09:00:00".to_owned(),
                close_local_time: "17:00:00".to_owned(),
                closed: false,
            },
            market_pb::CalendarSession {
                local_date: end.to_owned(),
                open_local_time: "09:00:00".to_owned(),
                close_local_time: "17:00:00".to_owned(),
                closed: false,
            },
        ],
    }
}

fn make_opening_base(
    account_id: &str,
    base_suffix: &str,
    opening_notional: &str,
    face_value: &str,
    maturity: &str,
    default_cal_req: rates_pb::CalendarRequirement,
    cal_end: &str,
) -> pb::OpeningPositionBase {
    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();
    pb::OpeningPositionBase {
        account_id: account_id.to_owned(),
        base_id: Some(ulid(base_suffix)),
        trading_date: "2026-06-15".to_owned(),
        next_settlement_date: "2026-06-16".to_owned(),
        effective_from: Some(pb_time(2026, 6, 15, 9, 0, 0)),
        watermark_time: Some(pb_time(2026, 6, 15, 9, 0, 0)),
        watermark_source_seq: Some(0),
        source_id: "SRC-SHARED".to_owned(),
        opening_cash_t0: Some(dec("100000000", cur)),
        opening_net_cash_delta_t1: Some(dec("20000000", cur)),
        balances: vec![pb::OpeningInstrumentBalance {
            instrument_code: "BOND_10Y".to_owned(),
            opening_economic_notional: Some(dec(opening_notional, cur)),
            opening_available_notional: Some(dec(opening_notional, cur)),
            opening_net_t0_notional: Some(dec("0", cur)),
            opening_net_t1_notional: Some(dec("0", cur)),
        }],
        instruments: vec![pb::InstrumentCatalogEntry {
            instrument_code: "BOND_10Y".to_owned(),
            bond: Some(pb_bond("B1", face_value, "2025-01-15", maturity, "0.025")),
            eligibility_class: pb::RateCoverageEligibility::SovereignOrPolicyFixed as i32,
            calendar_requirement: rates_pb::CalendarRequirement::Unspecified as i32,
        }],
        calendar: Some(pb_calendar("2025-01-01", cal_end)),
        default_calendar_requirement: default_cal_req as i32,
        units: Some(u),
        baseline_evidence: Vec::new(),
        source_start_after_feed_seq: 0,
    }
}

fn futures_v2_pack() -> market_pb::MarketRulePack {
    let raw = fs::read(repo_root().join("domain-packs/cgb-futures/cgb-futures-v2.bin"))
        .expect("cgb-futures-v2.bin");
    let digest = ContentHash::digest(&raw).as_bytes().to_vec();
    market_pb::MarketRulePack {
        rule_pack_id: Some(ulid("FP")),
        version: 1,
        owner: None,
        market: FUTURES_MARKET.to_owned(),
        rule_type: FUTURES_RULE_TYPE.to_owned(),
        source: "cffex".to_owned(),
        effective_from: Some(pb_time(2026, 1, 1, 0, 0, 0)),
        effective_to: Some(pb_time(2028, 1, 1, 0, 0, 0)),
        verification_status: market_pb::VerificationStatus::Verified as i32,
        content_hash: Some(core_pb::Sha256 { value: digest }),
        content: Some(prost_types::Any {
            type_url: FUTURES_TYPE_URL.to_owned(),
            value: raw,
        }),
    }
}

fn quote_tick(source_id: &str, quote_seq: u64, inst: &str, ytm: &str) -> pb::MarketQuoteTick {
    let u = pb_units();
    pb::MarketQuoteTick {
        source_id: source_id.to_owned(),
        quote_seq,
        instrument_code: inst.to_owned(),
        source_type: market_pb::PriceSourceType::ActiveQuote as i32,
        bid_clean_price: None,
        ask_clean_price: None,
        mid_clean_price: None,
        mid_yield_to_maturity: Some(dec(ytm, u.rate.as_ref().unwrap())),
        observed_at: Some(pb_time(2026, 6, 15, 9, 59, 0)),
        received_at: Some(pb_time(2026, 6, 15, 9, 59, 5)),
    }
}

fn extract_err_code(detail: Option<&core_pb::ErrorDetail>) -> core_pb::ErrorCode {
    core_pb::ErrorCode::try_from(detail.expect("error detail").code).unwrap()
}

// ============================================================================
// TEST 1: Full Auth & Pre-Idempotency RBAC Matrix over Native gRPC & gRPC-Web
// ============================================================================

#[tokio::test]
async fn dual_protocol_auth_and_pre_idempotency_rbac_matrix() {
    let policy_provider = Arc::new(TestPolicyProvider::new(default_test_policy(true)));
    let srv = LiveTestServer::start(
        policy_provider,
        DeskBlotterServiceConfig::default(),
        10_000,
        128,
    )
    .await;
    let mut client = srv.client().await;

    // Import acct-A and acct-B using operator via native gRPC (authorization: Bearer) and
    // gRPC-Web (x-ficant-credential) to prove BOTH headers work across BOTH transports!
    let base_a = make_opening_base(
        "acct-A",
        "A1",
        "100000000",
        "100",
        "2030-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2031-12-31",
    );
    let imp_a = client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "imp-a".to_owned(),
                opening_base: Some(base_a),
                expected_account_version: 0,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        imp_a.result,
        Some(pb::import_opening_base_response::Result::Value(_))
    ));

    let base_b = make_opening_base(
        "acct-B",
        "B1",
        "100000000",
        "100",
        "2030-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2031-12-31",
    );
    let imp_b: pb::ImportOpeningBaseResponse = grpc_web_unary(
        srv.addr,
        "/ficant.blotter.v1.DeskBlotterService/ImportOpeningBase",
        &pb::ImportOpeningBaseRequest {
            idempotency_key: "imp-b".to_owned(),
            opening_base: Some(base_b),
            expected_account_version: 0,
        },
        Some((FICANT_CREDENTIAL_HEADER, OPERATOR_TOKEN)),
    )
    .await;
    assert!(matches!(
        imp_b.result,
        Some(pb::import_opening_base_response::Result::Value(_))
    ));

    // 1. Missing / unknown / duplicate credential headers -> UNAUTHENTICATED on BOTH protocols
    let snap_req = pb::GetAccountSnapshotRequest {
        account_id: "acct-A".to_owned(),
    };
    let no_cred_grpc = client
        .get_account_snapshot(Request::new(snap_req.clone()))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        no_cred_grpc.result,
        Some(pb::get_account_snapshot_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Unauthenticated
    ));

    let no_cred_web: pb::GetAccountSnapshotResponse = grpc_web_unary(
        srv.addr,
        "/ficant.blotter.v1.DeskBlotterService/GetAccountSnapshot",
        &snap_req,
        None,
    )
    .await;
    assert!(matches!(
        no_cred_web.result,
        Some(pb::get_account_snapshot_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Unauthenticated
    ));

    let bad_cred_web: pb::GetAccountSnapshotResponse = grpc_web_unary(
        srv.addr,
        "/ficant.blotter.v1.DeskBlotterService/GetAccountSnapshot",
        &snap_req,
        Some((AUTHORIZATION_HEADER, "Bearer totally-unknown-secret")),
    )
    .await;
    match bad_cred_web.result {
        Some(pb::get_account_snapshot_response::Result::Error(e)) => {
            assert_eq!(
                extract_err_code(Some(&e)),
                core_pb::ErrorCode::Unauthenticated
            );
            assert!(!e.message.contains("totally-unknown-secret"));
        }
        other => panic!("expected Unauthenticated, got {other:?}"),
    }

    // 2. TRADER creates HOLD with key "hold-k1", then NLP_PARSER replays exact same key -> FORBIDDEN
    let u = pb_units();
    let create_hold_req = pb::CreateHoldRequest {
        idempotency_key: "hold-k1".to_owned(),
        account_id: "acct-A".to_owned(),
        expected_position_version: 1,
        instrument_code: "BOND_10Y".to_owned(),
        side: pb::TradeSide::Sell as i32,
        settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
        notional: Some(dec("10000000", u.currency_amount.as_ref().unwrap())),
        quote_terms: Some(pb::QuoteTerms {
            price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                "100.5",
                u.price_per_100.as_ref().unwrap(),
            ))),
            settlement_amount: None,
        }),
        ttl_seconds: 1800,
        note: "trader-a-hold".to_owned(),
    };
    let hold_ok = client
        .create_hold(with_x_cred(create_hold_req.clone(), TRADER_A_TOKEN))
        .await
        .unwrap()
        .into_inner();
    let created_earmark = match hold_ok.result {
        Some(pb::create_hold_response::Result::Value(v)) => {
            assert_eq!(v.earmark.as_ref().unwrap().created_by, "trader-a");
            v.earmark.unwrap()
        }
        other => panic!("expected hold created, got {other:?}"),
    };

    // NLP_PARSER calls CreateHold with TRADER's exact idempotency_key over gRPC-Web -> FORBIDDEN!
    let parser_replay: pb::CreateHoldResponse = grpc_web_unary(
        srv.addr,
        "/ficant.blotter.v1.DeskBlotterService/CreateHold",
        &create_hold_req,
        Some((AUTHORIZATION_HEADER, &format!("Bearer {PARSER_TOKEN}"))),
    )
    .await;
    assert!(matches!(
        parser_replay.result,
        Some(pb::create_hold_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Forbidden
    ));

    // Confirm HOLD as TRADER_A -> confirmed_by_trader_id == "trader-a" (never bearer token!)
    let confirm_ok = client
        .confirm_hold(with_bearer(
            pb::ConfirmHoldRequest {
                idempotency_key: "confirm-k1".to_owned(),
                account_id: "acct-A".to_owned(),
                earmark_id: created_earmark.earmark_id.clone(),
                expected_earmark_revision: created_earmark.earmark_revision,
                expected_position_version: 2,
                quote_terms: pb::QuoteTerms {
                    price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                        "100.5",
                        u.price_per_100.as_ref().unwrap(),
                    ))),
                    settlement_amount: Some(dec("10050000", u.currency_amount.as_ref().unwrap())),
                }
                .into(),
            },
            TRADER_A_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    match confirm_ok.result {
        Some(pb::confirm_hold_response::Result::Value(v)) => {
            let em = v.earmark.unwrap();
            assert_eq!(em.confirmed_by_trader_id.as_deref(), Some("trader-a"));
            assert!(!format!("{em:?}").contains(TRADER_A_TOKEN));
        }
        other => panic!("expected confirm ok, got {other:?}"),
    }

    // 3. State-dependent batch auth (IngestMarketQuotes on shared Q-SHARED affecting acct-A & acct-B):
    // trader-a ONLY owns acct-A -> MUST be rejected FORBIDDEN with ZERO writes and ZERO
    // HashMismatch/replay oracle leak even when reusing a previously committed idempotency_key!
    let head_before_quote = srv.store.head_seq().unwrap();
    let unauth_quote = client
        .ingest_market_quotes(with_bearer(
            pb::IngestMarketQuotesRequest {
                idempotency_key: "q-unauth-1".to_owned(),
                account_id: "acct-A".to_owned(),
                ticks: vec![quote_tick("Q-SHARED", 1, "BOND_10Y", "0.025")],
                source_id: "Q-SHARED".to_owned(),
            },
            TRADER_A_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        unauth_quote.result,
        Some(pb::ingest_market_quotes_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Forbidden
    ));
    assert_eq!(srv.store.head_seq().unwrap(), head_before_quote);

    // Pre-seed a receipt under trader-a's own actor_id for "q-oracle-key" on an account-local source
    // ("Q-B" is not active on acct-B, so trader-a succeeds on Q-B), then try reusing "q-oracle-key"
    // with a DIFFERENT payload on shared "Q-SHARED" (which affects acct-B). Must return FORBIDDEN
    // (never HashMismatch!), proving state-aware auth runs strictly BEFORE idempotency lookup!
    let seed_q = client
        .ingest_market_quotes(with_bearer(
            pb::IngestMarketQuotesRequest {
                idempotency_key: "q-oracle-key".to_owned(),
                account_id: "acct-A".to_owned(),
                ticks: vec![quote_tick("Q-B", 1, "BOND_10Y", "0.024")],
                source_id: "Q-B".to_owned(),
            },
            TRADER_A_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        seed_q.result,
        Some(pb::ingest_market_quotes_response::Result::Value(_))
    ));
    let oracle_probe = client
        .ingest_market_quotes(with_bearer(
            pb::IngestMarketQuotesRequest {
                idempotency_key: "q-oracle-key".to_owned(),
                account_id: "acct-A".to_owned(),
                ticks: vec![quote_tick("Q-SHARED", 2, "BOND_10Y", "0.029")],
                source_id: "Q-SHARED".to_owned(),
            },
            TRADER_A_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        oracle_probe.result,
        Some(pb::ingest_market_quotes_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Forbidden
    ));
    let head_before_quote = srv.store.head_seq().unwrap();

    // 4. Multi-account IngestTradeFeed containing acct-B called by feed-a-only -> FORBIDDEN, zero writes!
    let fill_a = pb::TradeFillRecord {
        source_id: "SRC-SHARED".to_owned(),
        external_trade_id: "TRD-AUTH-1".to_owned(),
        source_revision: 1,
        feed_seq: 1,
        action: pb::TradeFeedRecordAction::NewFill as i32,
        account_id: "acct-A".to_owned(),
        instrument_code: "BOND_10Y".to_owned(),
        side: pb::TradeSide::Buy as i32,
        settlement_date: "2026-06-15".to_owned(),
        notional: Some(dec("1000000", u.currency_amount.as_ref().unwrap())),
        quote_terms: Some(pb::QuoteTerms {
            price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                "100.0",
                u.price_per_100.as_ref().unwrap(),
            ))),
            settlement_amount: Some(dec("1000000", u.currency_amount.as_ref().unwrap())),
        }),
        executed_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
        batch_first_feed_seq: 1,
        batch_last_feed_seq: 2,
    };
    let mut fill_b = fill_a.clone();
    fill_b.external_trade_id = "TRD-AUTH-2".to_owned();
    fill_b.feed_seq = 2;
    fill_b.account_id = "acct-B".to_owned();

    let unauth_feed = client
        .ingest_trade_feed(with_bearer(
            pb::IngestTradeFeedRequest {
                idempotency_key: "feed-unauth-1".to_owned(),
                source_id: "SRC-SHARED".to_owned(),
                fills: vec![fill_a, fill_b],
                heartbeat_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
            },
            FEED_A_ONLY_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        unauth_feed.result,
        Some(pb::ingest_trade_feed_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Forbidden
    ));
    assert_eq!(srv.store.head_seq().unwrap(), head_before_quote);

    // 5. Force-clear gap pause: TRADER -> FORBIDDEN, OPERATOR -> allowed
    let trader_force = client
        .set_feed_control(with_bearer(
            pb::SetFeedControlRequest {
                idempotency_key: "fc-1".to_owned(),
                source_id: "SRC-SHARED".to_owned(),
                expected_feed_status_revision: 1,
                paused_by_operator: false,
                force_clear_gap_pause: true,
                reason: "override attempt".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        trader_force.result,
        Some(pb::set_feed_control_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Forbidden
    ));

    // Verify no bearer tokens leaked into any journal event
    let polled = client
        .poll_events(with_bearer(
            pb::PollEventsRequest {
                after_seq: 0,
                max_events: 100,
                account_id: None,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    let debug_dump = format!("{polled:?}");
    for secret in [
        TRADER_A_TOKEN,
        TRADER_AB_TOKEN,
        PARSER_TOKEN,
        FEED_A_ONLY_TOKEN,
        FEED_AB_TOKEN,
        OPERATOR_TOKEN,
    ] {
        assert!(!debug_dump.contains(secret), "secret leaked into events!");
    }
}

// ============================================================================
// TEST 2: Dual-Account PollEvents vs SubscribeEvents Exact-Seq Snapshot Parity
//         + Barrier Concurrency Injection + Mid-Stream Revocation + Lag/Fallback
// ============================================================================

#[tokio::test]
async fn dual_account_poll_and_subscribe_exact_seq_field_parity_and_barrier() {
    let policy_provider = Arc::new(TestPolicyProvider::new(default_test_policy(true)));
    let barrier_fired = Arc::new(AtomicBool::new(false));
    let barrier_flag = barrier_fired.clone();

    let svc_cfg = DeskBlotterServiceConfig {
        stream_heartbeat_interval: Duration::from_secs(45),
        on_subscribe_barrier_acquired: Some(Arc::new(move |_head| {
            barrier_flag.store(true, Ordering::SeqCst);
        })),
        ..DeskBlotterServiceConfig::default()
    };

    let srv = LiveTestServer::start(policy_provider, svc_cfg, 10_000, 128).await;
    let mut client = srv.client().await;
    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();
    let px = u.price_per_100.as_ref().unwrap();

    // 1. Import Account A & Account B
    for (acct, suf) in [("acct-A", "A1"), ("acct-B", "B1")] {
        let base = make_opening_base(
            acct,
            suf,
            "100000000",
            "100",
            "2030-01-15",
            rates_pb::CalendarRequirement::ReferenceReplay,
            "2031-12-31",
        );
        let resp = client
            .import_opening_base(with_bearer(
                pb::ImportOpeningBaseRequest {
                    idempotency_key: format!("imp-{acct}"),
                    opening_base: Some(base),
                    expected_account_version: 0,
                },
                OPERATOR_TOKEN,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            resp.result,
            Some(pb::import_opening_base_response::Result::Value(_))
        ));
    }

    let base_seq = srv.store.head_seq().unwrap();
    let old_snap_a = match client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-A".to_owned(),
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::get_account_snapshot_response::Result::Value(v)) => v.snapshot.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };
    let old_snap_b = match client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-B".to_owned(),
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::get_account_snapshot_response::Result::Value(v)) => v.snapshot.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };

    // 2. Create two COMMITTED earmarks on acct-A (30M + 20M) so we can later split-offset both
    //    and reverse both simultaneously, plus one COMMITTED earmark on acct-B.
    let em_a1 = match client
        .create_committed(with_bearer(
            pb::CreateCommittedRequest {
                idempotency_key: "cc-a1".to_owned(),
                account_id: "acct-A".to_owned(),
                expected_position_version: 1,
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Sell as i32,
                settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                notional: Some(dec("30000000", cur)),
                quote_terms: Some(pb::QuoteTerms {
                    price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                        "100.0", px,
                    ))),
                    settlement_amount: Some(dec("30000000", cur)),
                }),
                note: "a1".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::create_committed_response::Result::Value(v)) => v.earmark.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };

    let em_a2 = match client
        .create_committed(with_bearer(
            pb::CreateCommittedRequest {
                idempotency_key: "cc-a2".to_owned(),
                account_id: "acct-A".to_owned(),
                expected_position_version: 2,
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Sell as i32,
                settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                notional: Some(dec("20000000", cur)),
                quote_terms: Some(pb::QuoteTerms {
                    price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                        "100.0", px,
                    ))),
                    settlement_amount: Some(dec("20000000", cur)),
                }),
                note: "a2".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::create_committed_response::Result::Value(v)) => v.earmark.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };

    // Start SubscribeEvents from `base_seq` AFTER two writes already occurred (so it exercises
    // BOTH catch-up replay AND live streaming across the barrier!).
    let mut sub_stream = client
        .subscribe_events(with_bearer(
            pb::SubscribeEventsRequest {
                after_seq: base_seq,
                account_id: None,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();

    // 3. Multi-account trade feed batch: 50M sell on acct-A (pending split against a1+a2) + 15M orphan buy on acct-B
    let ingest_resp = client
        .ingest_trade_feed(with_bearer(
            pb::IngestTradeFeedRequest {
                idempotency_key: "feed-multi-1".to_owned(),
                source_id: "SRC-SHARED".to_owned(),
                fills: vec![
                    pb::TradeFillRecord {
                        source_id: "SRC-SHARED".to_owned(),
                        external_trade_id: "TRD-A-50M".to_owned(),
                        source_revision: 1,
                        feed_seq: 1,
                        action: pb::TradeFeedRecordAction::NewFill as i32,
                        account_id: "acct-A".to_owned(),
                        instrument_code: "BOND_10Y".to_owned(),
                        side: pb::TradeSide::Sell as i32,
                        settlement_date: "2026-06-15".to_owned(),
                        notional: Some(dec("50000000", cur)),
                        quote_terms: Some(pb::QuoteTerms {
                            price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(
                                dec("100.0", px),
                            )),
                            settlement_amount: Some(dec("50000000", cur)),
                        }),
                        executed_at: Some(pb_time(2026, 6, 15, 10, 1, 0)),
                        batch_first_feed_seq: 1,
                        batch_last_feed_seq: 2,
                    },
                    pb::TradeFillRecord {
                        source_id: "SRC-SHARED".to_owned(),
                        external_trade_id: "TRD-B-15M".to_owned(),
                        source_revision: 1,
                        feed_seq: 2,
                        action: pb::TradeFeedRecordAction::NewFill as i32,
                        account_id: "acct-B".to_owned(),
                        instrument_code: "BOND_10Y".to_owned(),
                        side: pb::TradeSide::Buy as i32,
                        settlement_date: "2026-06-15".to_owned(),
                        notional: Some(dec("15000000", cur)),
                        quote_terms: Some(pb::QuoteTerms {
                            price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(
                                dec("100.2", px),
                            )),
                            settlement_amount: Some(dec("15030000", cur)),
                        }),
                        executed_at: Some(pb_time(2026, 6, 15, 10, 1, 0)),
                        batch_first_feed_seq: 1,
                        batch_last_feed_seq: 2,
                    },
                ],
                heartbeat_at: Some(pb_time(2026, 6, 15, 10, 1, 0)),
            },
            FEED_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    let (pm_a_id, pm_b_id) = match ingest_resp.result {
        Some(pb::ingest_trade_feed_response::Result::Value(v)) => (
            v.item_outcomes[0].pending_match_id.clone().unwrap(),
            v.item_outcomes[1].pending_match_id.clone().unwrap(),
        ),
        other => panic!("unexpected: {other:?}"),
    };

    // 4. Shared-source market quote update affecting BOTH acct-A & acct-B in one atomic commit
    let q_resp = client
        .ingest_market_quotes(with_bearer(
            pb::IngestMarketQuotesRequest {
                idempotency_key: "q-shared-1".to_owned(),
                account_id: "acct-A".to_owned(),
                ticks: vec![quote_tick("Q-SHARED", 1, "BOND_10Y", "0.025")],
                source_id: "Q-SHARED".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        q_resp.result,
        Some(pb::ingest_market_quotes_response::Result::Value(_))
    ));

    // 5. Split-offset TRD-A-50M across BOTH earmark A1 (30M) and earmark A2 (20M)
    let split1 = client
        .resolve_pending_match(with_bearer(
            pb::ResolvePendingMatchRequest {
                idempotency_key: "split-a1".to_owned(),
                account_id: "acct-A".to_owned(),
                pending_match_id: Some(pm_a_id.clone()),
                expected_pending_revision: 1,
                expected_position_version: 4,
                action: pb::PendingMatchResolutionAction::SplitOffset as i32,
                expected_earmark_revision: Some(em_a1.earmark_revision),
                target_earmark_id: em_a1.earmark_id.clone(),
                offset_notional: Some(dec("30000000", cur)),
                reason: "offset A1".to_owned(),
                baseline_delta: None,
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        split1.result,
        Some(pb::resolve_pending_match_response::Result::Value(_))
    ));

    let split2 = client
        .resolve_pending_match(with_bearer(
            pb::ResolvePendingMatchRequest {
                idempotency_key: "split-a2".to_owned(),
                account_id: "acct-A".to_owned(),
                pending_match_id: Some(pm_a_id),
                expected_pending_revision: 2,
                expected_position_version: 5,
                action: pb::PendingMatchResolutionAction::SplitOffset as i32,
                expected_earmark_revision: Some(em_a2.earmark_revision),
                target_earmark_id: em_a2.earmark_id.clone(),
                offset_notional: Some(dec("20000000", cur)),
                reason: "offset A2".to_owned(),
                baseline_delta: None,
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        split2.result,
        Some(pb::resolve_pending_match_response::Result::Value(_))
    ));

    // 6. CANCEL_REVERSAL on TRD-A-50M -> reverses BOTH earmarks A1 & A2 simultaneously!
    let rev_resp = client
        .ingest_trade_feed(with_bearer(
            pb::IngestTradeFeedRequest {
                idempotency_key: "feed-rev-a".to_owned(),
                source_id: "SRC-SHARED".to_owned(),
                fills: vec![pb::TradeFillRecord {
                    source_id: "SRC-SHARED".to_owned(),
                    external_trade_id: "TRD-A-50M".to_owned(),
                    source_revision: 2,
                    feed_seq: 3,
                    action: pb::TradeFeedRecordAction::CancelReversal as i32,
                    account_id: "acct-A".to_owned(),
                    instrument_code: "BOND_10Y".to_owned(),
                    side: pb::TradeSide::Sell as i32,
                    settlement_date: "2026-06-15".to_owned(),
                    notional: Some(dec("0", cur)),
                    quote_terms: None,
                    executed_at: Some(pb_time(2026, 6, 15, 10, 5, 0)),
                    batch_first_feed_seq: 3,
                    batch_last_feed_seq: 3,
                }],
                heartbeat_at: Some(pb_time(2026, 6, 15, 10, 5, 0)),
            },
            FEED_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        rev_resp.result,
        Some(pb::ingest_trade_feed_response::Result::Value(_))
    ));

    // 7. Terminal removals: CancelCommitted on A2 (leaves active_earmarks) + ACCEPT_ORPHAN on acct-B (leaves open_pending_matches)
    let snap_a_mid = match client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-A".to_owned(),
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::get_account_snapshot_response::Result::Value(v)) => v.snapshot.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };
    let a2_rev = snap_a_mid
        .active_earmarks
        .iter()
        .find(|e| e.earmark_id == em_a2.earmark_id)
        .expect("a2 reopened")
        .earmark_revision;
    let cancel_a2 = client
        .cancel_committed(with_bearer(
            pb::CancelCommittedRequest {
                idempotency_key: "cancel-a2".to_owned(),
                account_id: "acct-A".to_owned(),
                earmark_id: em_a2.earmark_id.clone(),
                expected_earmark_revision: a2_rev,
                reason: "terminal removal test".to_owned(),
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        cancel_a2.result,
        Some(pb::cancel_committed_response::Result::Value(_))
    ));

    let accept_b = client
        .resolve_pending_match(with_bearer(
            pb::ResolvePendingMatchRequest {
                idempotency_key: "accept-b".to_owned(),
                account_id: "acct-B".to_owned(),
                pending_match_id: Some(pm_b_id),
                expected_pending_revision: 1,
                expected_position_version: 2,
                action: pb::PendingMatchResolutionAction::AcceptOrphan as i32,
                expected_earmark_revision: None,
                target_earmark_id: None,
                offset_notional: None,
                reason: "accept official fill".to_owned(),
                baseline_delta: None,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        accept_b.result,
        Some(pb::resolve_pending_match_response::Result::Value(_))
    ));

    let target_seq = srv.store.head_seq().unwrap();

    // Capture authoritative server snapshots for BOTH accounts at exact `target_seq`
    let server_snap_a = match client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-A".to_owned(),
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::get_account_snapshot_response::Result::Value(v)) => v.snapshot.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };
    let server_snap_b = match client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-B".to_owned(),
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::get_account_snapshot_response::Result::Value(v)) => v.snapshot.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };
    assert_eq!(server_snap_a.snapshot_seq, target_seq);
    assert_eq!(server_snap_b.snapshot_seq, target_seq);
    assert_eq!(server_snap_a.active_earmarks.len(), 1); // A2 removed!
    assert!(server_snap_b.open_pending_matches.is_empty()); // B pending removed!

    // 8. Replay via PollEvents (page size = 1 to prove whole-tx pagination never splits)
    let mut poll_a = old_snap_a.clone();
    let mut poll_b = old_snap_b.clone();
    let mut poll_cursor = base_seq;
    let mut saw_double_reversal = false;
    loop {
        let resp = client
            .poll_events(with_bearer(
                pb::PollEventsRequest {
                    after_seq: poll_cursor,
                    max_events: 1,
                    account_id: None,
                },
                OPERATOR_TOKEN,
            ))
            .await
            .unwrap()
            .into_inner();
        let batch = match resp.result {
            Some(pb::poll_events_response::Result::Value(pb::PollEventsResult {
                outcome: Some(pb::poll_events_result::Outcome::Batch(b)),
            })) => b,
            other => panic!("expected batch, got {other:?}"),
        };
        for ev in &batch.events {
            assert!(ev.seq > poll_cursor);
            for ch in &ev.changes {
                if ch
                    .changed_allocations
                    .iter()
                    .filter(|a| a.reversed_by_revision == Some(2))
                    .count()
                    == 2
                {
                    saw_double_reversal = true;
                }
                if ch.account_id == "acct-A" {
                    poll_a = ch.replace_snapshot.clone().unwrap();
                } else if ch.account_id == "acct-B" {
                    poll_b = ch.replace_snapshot.clone().unwrap();
                }
            }
        }
        poll_cursor = batch.scanned_through_seq;
        if !batch.has_more {
            break;
        }
    }
    assert!(
        saw_double_reversal,
        "must observe both earmark allocations reversed in one event"
    );
    assert_eq!(poll_cursor, target_seq);
    // Normalize snapshot_seq to target_seq only for untouched account if final tx touched only one account
    poll_a.snapshot_seq = target_seq;
    poll_b.snapshot_seq = target_seq;
    assert_eq!(
        poll_a, server_snap_a,
        "PollEvents acct-A field-for-field mismatch"
    );
    assert_eq!(
        poll_b, server_snap_b,
        "PollEvents acct-B field-for-field mismatch"
    );

    // 9. Consume from live SubscribeEvents stream up to `target_seq`
    let mut sub_a = old_snap_a;
    let mut sub_b = old_snap_b;
    let mut sub_cursor = base_seq;
    while sub_cursor < target_seq {
        let item = tokio::time::timeout(Duration::from_secs(5), sub_stream.next())
            .await
            .expect("subscribe item timeout")
            .expect("stream open")
            .expect("grpc ok");
        match item.payload {
            Some(pb::subscribe_events_response::Payload::Event(ev)) => {
                assert!(ev.seq > sub_cursor, "duplicate or regressed seq {}", ev.seq);
                sub_cursor = ev.seq;
                for ch in ev.changes {
                    if ch.account_id == "acct-A" {
                        sub_a = ch.replace_snapshot.unwrap();
                    } else if ch.account_id == "acct-B" {
                        sub_b = ch.replace_snapshot.unwrap();
                    }
                }
            }
            Some(pb::subscribe_events_response::Payload::Heartbeat(hb)) => {
                assert!(
                    hb.scanned_through_seq >= sub_cursor,
                    "heartbeat scanned_through_seq must never regress"
                );
            }
            other => panic!("unexpected subscribe payload: {other:?}"),
        }
    }
    assert!(barrier_fired.load(Ordering::SeqCst));
    assert_eq!(sub_cursor, target_seq);
    sub_a.snapshot_seq = target_seq;
    sub_b.snapshot_seq = target_seq;
    assert_eq!(
        sub_a, server_snap_a,
        "SubscribeEvents acct-A field-for-field mismatch"
    );
    assert_eq!(
        sub_b, server_snap_b,
        "SubscribeEvents acct-B field-for-field mismatch"
    );

    // 10. Account-filtered PollEvents advances `scanned_through_seq` past invisible commits!
    let filtered = client
        .poll_events(with_bearer(
            pb::PollEventsRequest {
                after_seq: target_seq - 1,
                max_events: 10,
                account_id: Some("acct-A".to_owned()),
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    match filtered.result {
        Some(pb::poll_events_response::Result::Value(pb::PollEventsResult {
            outcome: Some(pb::poll_events_result::Outcome::Batch(b)),
        })) => {
            assert_eq!(b.scanned_through_seq, target_seq);
            assert!(!b.has_more);
        }
        other => panic!("unexpected: {other:?}"),
    }

    // 11. Mid-subscription credential revocation stops pushing and terminates stream!
    srv.verifier.revoke_actor("operator-1");
    // Trigger one more event so the live subscriber hits the auth re-check
    let _ = client
        .create_hold(with_bearer(
            pb::CreateHoldRequest {
                idempotency_key: "post-revoke-hold".to_owned(),
                account_id: "acct-A".to_owned(),
                expected_position_version: server_snap_a.positions[0].position_version,
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Buy as i32,
                settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                notional: Some(dec("1000000", cur)),
                quote_terms: None,
                ttl_seconds: 1800,
                note: "trigger-revoke-check".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap();

    let revoked_item = tokio::time::timeout(Duration::from_secs(5), sub_stream.next())
        .await
        .expect("revoke event timeout")
        .expect("revoke error frame")
        .expect("grpc ok");
    assert!(matches!(
        revoked_item.payload,
        Some(pb::subscribe_events_response::Payload::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Unauthenticated
    ));
    // Stream terminates cleanly immediately after the error item
    assert!(
        tokio::time::timeout(Duration::from_secs(2), sub_stream.next())
            .await
            .unwrap()
            .is_none()
    );
}

// ============================================================================
// TEST 3: Lag Recovery, Retention SnapshotFallbackRequired, Future Cursor,
//         Payload Size Admission & Reversal Headroom
// ============================================================================

#[tokio::test]
async fn subscribe_lag_retention_fallback_future_cursor_and_payload_headroom() {
    let policy_provider = Arc::new(TestPolicyProvider::new(default_test_policy(true)));
    // Small retention window = 3, small broadcast bus capacity = 2, and stream buffer = 1 so
    // pausing the client deterministically triggers broadcast::RecvError::Lagged + retention fallback!
    let lag_cfg = DeskBlotterServiceConfig {
        subscribe_stream_channel_capacity: 1,
        ..DeskBlotterServiceConfig::default()
    };
    let srv = LiveTestServer::start(policy_provider, lag_cfg, 3, 2).await;
    let mut client = srv.client().await;
    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();

    let base_a = make_opening_base(
        "acct-A",
        "A1",
        "100000000",
        "100",
        "2030-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2031-12-31",
    );
    let _ = client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "imp-lag".to_owned(),
                opening_base: Some(base_a),
                expected_account_version: 0,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap();

    // Open a subscriber at seq=1, read its initial heartbeat, then pause reading while 6 writes
    // overflow BOTH the broadcast channel (cap=2) AND the retention window (window=3)!
    let mut sub = client
        .subscribe_events(with_bearer(
            pb::SubscribeEventsRequest {
                after_seq: 1,
                account_id: None,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    let first_hb = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        first_hb.payload,
        Some(pb::subscribe_events_response::Payload::Heartbeat(_))
    ));

    // 1. Direct in-process SubscribeEvents (no HTTP/2 64 KiB send buffer) with channel_cap=1,
    //    bus_cap=2, retention=3 deterministically overflows live_rx -> RecvError::Lagged -> SnapshotFallbackRequired!
    let direct_store = Arc::new(
        SqliteBlotterStore::open(
            &SqliteBlotterOpenOptions::ephemeral_in_memory(BlotterConfig::default())
                .with_event_retention_window(3),
        )
        .unwrap(),
    );
    let direct_adapter = Arc::new(TestStoreAdapter::new(direct_store.clone(), 2));
    let direct_svc = DeskBlotterGrpcService::new(
        direct_adapter,
        Arc::new(TestPolicyProvider::new(default_test_policy(true))),
        Arc::new(build_test_verifier()),
        Arc::new(ManualBlotterClock::new(shanghai_time(
            2026, 6, 15, 10, 0, 0,
        ))),
        OwnedBlotterEngines {
            bond_engine: Arc::new(NativeBondAnalyticsEngine),
            hedge_engine: Arc::new(NativeFuturesHedgeEngine),
            delivery_rule_parser: Arc::new(CgbFuturesDeliveryRulePackParser),
            regulatory_rule_parser: Arc::new(RegulatoryRulePackV1Parser),
        },
        DeskBlotterServiceConfig {
            subscribe_stream_channel_capacity: 1,
            ..DeskBlotterServiceConfig::default()
        },
    )
    .unwrap();
    let _ = DeskBlotterService::import_opening_base(
        &direct_svc,
        with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "dir-imp".to_owned(),
                opening_base: Some(make_opening_base(
                    "acct-A",
                    "D1",
                    "100000000",
                    "100",
                    "2030-01-15",
                    rates_pb::CalendarRequirement::ReferenceReplay,
                    "2031-12-31",
                )),
                expected_account_version: 0,
            },
            OPERATOR_TOKEN,
        ),
    )
    .await
    .unwrap();
    let mut direct_sub = direct_svc
        .subscribe_events(with_bearer(
            pb::SubscribeEventsRequest {
                after_seq: 1,
                account_id: None,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    let direct_hb = tokio::time::timeout(Duration::from_secs(3), direct_sub.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        direct_hb.payload,
        Some(pb::subscribe_events_response::Payload::Heartbeat(_))
    ));
    let mut d_pos_ver = 1_u64;
    for i in 1..=8_u32 {
        let r = direct_svc
            .create_hold(with_bearer(
                pb::CreateHoldRequest {
                    idempotency_key: format!("dh-{i}"),
                    account_id: "acct-A".to_owned(),
                    expected_position_version: d_pos_ver,
                    instrument_code: "BOND_10Y".to_owned(),
                    side: pb::TradeSide::Buy as i32,
                    settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                    notional: Some(dec("1000000", cur)),
                    quote_terms: None,
                    ttl_seconds: 1800,
                    note: format!("dh{i}"),
                },
                TRADER_AB_TOKEN,
            ))
            .await
            .unwrap()
            .into_inner();
        if let Some(pb::create_hold_response::Result::Value(v)) = r.result {
            d_pos_ver = v.position.unwrap().position_version;
        }
    }
    let mut saw_live_lag_fallback = false;
    for _ in 0..10 {
        let Some(Ok(item)) = tokio::time::timeout(Duration::from_secs(3), direct_sub.next())
            .await
            .unwrap()
        else {
            break;
        };
        if let Some(pb::subscribe_events_response::Payload::SnapshotFallback(fb)) = item.payload {
            assert!(fb.oldest_retained_seq > fb.requested_after_seq + 1);
            assert_eq!(fb.current_head_seq, 9);
            saw_live_lag_fallback = true;
            break;
        }
    }
    assert!(
        saw_live_lag_fallback,
        "expected SnapshotFallbackRequired on lagged live subscriber when pruned"
    );

    // 2. Over live HTTP/2 network: commit 6 holds to prune seqs 1..=4 (retention=3 keeps 5,6,7),
    //    then SubscribeEvents(after_seq = 0) immediately returns SnapshotFallbackRequired!
    let mut pos_ver = 1_u64;
    for i in 1..=6_u32 {
        let resp = client
            .create_hold(with_bearer(
                pb::CreateHoldRequest {
                    idempotency_key: format!("h-{i}"),
                    account_id: "acct-A".to_owned(),
                    expected_position_version: pos_ver,
                    instrument_code: "BOND_10Y".to_owned(),
                    side: pb::TradeSide::Buy as i32,
                    settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                    notional: Some(dec("1000000", cur)),
                    quote_terms: None,
                    ttl_seconds: 1800,
                    note: format!("h{i}"),
                },
                TRADER_AB_TOKEN,
            ))
            .await
            .unwrap()
            .into_inner();
        match resp.result {
            Some(pb::create_hold_response::Result::Value(v)) => {
                pos_ver = v.position.unwrap().position_version;
            }
            other => panic!("expected hold ok, got {other:?}"),
        }
    }

    let mut pruned_sub = client
        .subscribe_events(with_bearer(
            pb::SubscribeEventsRequest {
                after_seq: 0,
                account_id: None,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    let fb_item = tokio::time::timeout(Duration::from_secs(3), pruned_sub.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match fb_item.payload {
        Some(pb::subscribe_events_response::Payload::SnapshotFallback(fb)) => {
            assert_eq!(fb.requested_after_seq, 0);
            assert!(fb.oldest_retained_seq > 1);
            assert_eq!(fb.current_head_seq, 7);
        }
        other => panic!("expected SnapshotFallbackRequired, got {other:?}"),
    }
    drop(sub);

    // PollEvents(after_seq = 0) ALSO returns SnapshotFallbackRequired
    let poll_fb = client
        .poll_events(with_bearer(
            pb::PollEventsRequest {
                after_seq: 0,
                max_events: 10,
                account_id: None,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        poll_fb.result,
        Some(pb::poll_events_response::Result::Value(
            pb::PollEventsResult {
                outcome: Some(pb::poll_events_result::Outcome::SnapshotFallback(_))
            }
        ))
    ));

    // Future cursor (after_seq = 999) -> ValidationFailed on BOTH PollEvents and SubscribeEvents
    let future_poll = client
        .poll_events(with_bearer(
            pb::PollEventsRequest {
                after_seq: 999,
                max_events: 10,
                account_id: None,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        future_poll.result,
        Some(pb::poll_events_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed
    ));

    let mut future_sub = client
        .subscribe_events(with_bearer(
            pb::SubscribeEventsRequest {
                after_seq: 999,
                account_id: None,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    let future_err = future_sub.next().await.unwrap().unwrap();
    assert!(matches!(
        future_err.payload,
        Some(pb::subscribe_events_response::Payload::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed
    ));

    // Payload size admission & reversal headroom test:
    let tiny_cfg = DeskBlotterServiceConfig {
        max_event_envelope_bytes: 1_800,
        reversal_headroom_bytes: 600,
        ..DeskBlotterServiceConfig::default()
    };
    let srv_cap = LiveTestServer::start(
        Arc::new(TestPolicyProvider::new(default_test_policy(true))),
        tiny_cfg,
        100,
        16,
    )
    .await;
    let mut cap_client = srv_cap.client().await;
    let huge_base = make_opening_base(
        "acct-A",
        "A9",
        "100000000",
        "100",
        "2030-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2031-12-31",
    );
    // Even with a single account, envelope exceeds 1200-byte normal admission budget -> rejected!
    let oversized = cap_client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "oversized-1".to_owned(),
                opening_base: Some(huge_base),
                expected_account_version: 0,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    match oversized.result {
        Some(pb::import_opening_base_response::Result::Error(e)) => {
            assert_eq!(
                extract_err_code(Some(&e)),
                core_pb::ErrorCode::ValidationFailed
            );
            assert!(
                e.field_violations.iter().any(|fv| fv.field == "request"),
                "expected split-batch payload diagnostic, got {e:?}"
            );
        }
        other => panic!("expected payload too large error, got {other:?}"),
    }
    assert_eq!(
        srv_cap.store.head_seq().unwrap(),
        0,
        "head_seq must remain 0"
    );

    // Reversal headroom guarantee: admit an account + 1 COMMITTED earmark just under normal budget
    // (max - headroom), reject a new capacity-expanding HOLD that crosses normal budget, yet STILL
    // allow CancelCommitted using the reserved reversal headroom without truncating the event!
    let headroom_srv = LiveTestServer::start(
        Arc::new(TestPolicyProvider::new(default_test_policy(true))),
        DeskBlotterServiceConfig {
            max_event_envelope_bytes: 8_000,
            reversal_headroom_bytes: 2_500,
            ..DeskBlotterServiceConfig::default()
        },
        100,
        16,
    )
    .await;
    let mut hr_client = headroom_srv.client().await;
    let _ = hr_client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "hr-imp".to_owned(),
                opening_base: Some(make_opening_base(
                    "acct-A",
                    "H1",
                    "100000000",
                    "100",
                    "2030-01-15",
                    rates_pb::CalendarRequirement::ReferenceReplay,
                    "2031-12-31",
                )),
                expected_account_version: 0,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap();
    let committed = match hr_client
        .create_committed(with_bearer(
            pb::CreateCommittedRequest {
                idempotency_key: "hr-cc".to_owned(),
                account_id: "acct-A".to_owned(),
                expected_position_version: 1,
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Buy as i32,
                settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                notional: Some(dec("10000000", cur)),
                quote_terms: Some(pb::QuoteTerms {
                    price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                        "100.0",
                        u.price_per_100.as_ref().unwrap(),
                    ))),
                    settlement_amount: Some(dec("10000000", cur)),
                }),
                note: "x".repeat(400),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::create_committed_response::Result::Value(v)) => v.earmark.unwrap(),
        other => panic!("expected committed under normal budget, got {other:?}"),
    };
    // Oversized new HOLD crosses normal budget (5500) -> rejected cleanly before commit
    let blocked_hold = hr_client
        .create_hold(with_bearer(
            pb::CreateHoldRequest {
                idempotency_key: "hr-big-hold".to_owned(),
                account_id: "acct-A".to_owned(),
                expected_position_version: 2,
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Buy as i32,
                settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                notional: Some(dec("1000000", cur)),
                quote_terms: None,
                ttl_seconds: 1800,
                note: "y".repeat(3_500),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        blocked_hold.result,
        Some(pb::create_hold_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed
    ));
    // Reversal/CancelCommitted still succeeds using reserved reversal headroom!
    let cancel_ok = hr_client
        .cancel_committed(with_bearer(
            pb::CancelCommittedRequest {
                idempotency_key: "hr-cancel".to_owned(),
                account_id: "acct-A".to_owned(),
                earmark_id: committed.earmark_id,
                expected_earmark_revision: committed.earmark_revision,
                reason: "r".repeat(1_000),
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        cancel_ok.result,
        Some(pb::cancel_committed_response::Result::Value(_))
    ));
}

// ============================================================================
// TEST 4: 100 vs 1000 Face-Value DV01/Draft/Hedge Parity, Calendar Priority,
//         Expired ConfirmHold Lazy-Expiry Receipt, and RulePack Validation Matrix
// ============================================================================

#[tokio::test]
async fn valuation_face_parity_draft_rulepack_matrix_and_expired_confirm_hold() {
    let policy_provider = Arc::new(TestPolicyProvider::new(default_test_policy(true)));
    let srv = LiveTestServer::start(
        policy_provider.clone(),
        DeskBlotterServiceConfig::default(),
        10_000,
        64,
    )
    .await;
    let mut client = srv.client().await;
    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();
    let px = u.price_per_100.as_ref().unwrap();
    let dv100 = u.dv01_per_100.as_ref().unwrap();
    let dim = u.dimensionless.as_ref().unwrap();

    // Import acct-A with face_value=100 and acct-B with face_value=1000, identical economic 100M
    // notional, identical 2035 long maturity (tests PROVISIONAL_WEEKEND_BEYOND_COVERAGE!).
    let base_100 = make_opening_base(
        "acct-A",
        "F1",
        "100000000",
        "100",
        "2035-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2027-12-31",
    );
    let base_1000 = make_opening_base(
        "acct-B",
        "F2",
        "100000000",
        "1000",
        "2035-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2027-12-31",
    );
    for (k, base) in [("i-100", base_100), ("i-1000", base_1000)] {
        let r = client
            .import_opening_base(with_bearer(
                pb::ImportOpeningBaseRequest {
                    idempotency_key: k.to_owned(),
                    opening_base: Some(base),
                    expected_account_version: 0,
                },
                OPERATOR_TOKEN,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            r.result,
            Some(pb::import_opening_base_response::Result::Value(_))
        ));
    }

    // Ingest same yield quote on shared source for both accounts
    let _ = client
        .ingest_market_quotes(with_bearer(
            pb::IngestMarketQuotesRequest {
                idempotency_key: "q-parity".to_owned(),
                account_id: "acct-A".to_owned(),
                ticks: vec![quote_tick("Q-SHARED", 1, "BOND_10Y", "0.025")],
                source_id: "Q-SHARED".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap();

    let snap_100 = match client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-A".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::get_account_snapshot_response::Result::Value(v)) => v.snapshot.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };
    let snap_1000 = match client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-B".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::get_account_snapshot_response::Result::Value(v)) => v.snapshot.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };

    // Exact fixed-point equality of PRE_TAX DV01 between face_value=100 and face_value=1000!
    assert_eq!(
        snap_100.positions[0].pre_tax_dv01,
        snap_1000.positions[0].pre_tax_dv01
    );
    assert_eq!(
        snap_100.positions[0].calendar_resolution,
        pb::CalendarResolutionMode::ProvisionalWeekendBeyondCoverage as i32
    );

    // PreviewCoveredHedge: exact contract parity for "T" on both accounts!
    let hedge_a = match client
        .preview_covered_hedge(with_bearer(
            pb::PreviewCoveredHedgeRequest {
                account_id: "acct-A".to_owned(),
                scope: pb::PreviewHedgeScope::WholeAccount as i32,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: Some(dec("0.075", dv100)),
                conversion_factor: Some(dec("0.985", dim)),
                delivery_rule_pack: Some(futures_v2_pack()),
                valuation_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
                expected_snapshot_seq: None,
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::preview_covered_hedge_response::Result::Value(v)) => v,
        other => panic!("unexpected hedge A: {other:?}"),
    };
    let hedge_b = match client
        .preview_covered_hedge(with_bearer(
            pb::PreviewCoveredHedgeRequest {
                account_id: "acct-B".to_owned(),
                scope: pb::PreviewHedgeScope::WholeAccount as i32,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: Some(dec("0.075", dv100)),
                conversion_factor: Some(dec("0.985", dim)),
                delivery_rule_pack: Some(futures_v2_pack()),
                valuation_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
                expected_snapshot_seq: None,
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::preview_covered_hedge_response::Result::Value(v)) => v,
        other => panic!("unexpected hedge B: {other:?}"),
    };
    assert_eq!(
        hedge_a.covered_net_pre_tax_dv01,
        hedge_b.covered_net_pre_tax_dv01
    );
    assert_eq!(
        hedge_a.measures.as_ref().unwrap().recommended_contracts,
        hedge_b.measures.as_ref().unwrap().recommended_contracts
    );

    // Draft submission & confirmation via NLP_PARSER -> TRADER:
    let raw_text = "BUY T+0 INST=BOND_10Y NOTIONAL=10000000 CLEAN=100.0 TARGET=COMMITTED";
    let draft_resp = client
        .submit_draft_earmark(with_bearer(
            pb::SubmitDraftEarmarkRequest {
                idempotency_key: "draft-1".to_owned(),
                account_id: "acct-A".to_owned(),
                proposal: Some(pb::NlpEarmarkProposal {
                    parser_source_id: "nlp-bot-1".to_owned(),
                    model_version: "1.0".to_owned(),
                    raw_source_text: raw_text.to_owned(),
                    anchors: vec![pb::SourceTextAnchor {
                        field_name: "side".to_owned(),
                        byte_start: 0,
                        byte_end: 3,
                        span_text: "BUY".to_owned(),
                    }],
                    confidence_score: Some(dec("0.95", dim)),
                    account_id: "acct-A".to_owned(),
                    instrument_code: "BOND_10Y".to_owned(),
                    side: pb::TradeSide::Buy as i32,
                    settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                    notional: Some(dec("10000000", cur)),
                    quote_terms: Some(pb::QuoteTerms {
                        price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                            "100.0", px,
                        ))),
                        settlement_amount: Some(dec("10000000", cur)),
                    }),
                    suggested_target_state: pb::DraftTargetState::Committed as i32,
                }),
            },
            PARSER_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    let draft = match draft_resp.result {
        Some(pb::submit_draft_earmark_response::Result::Value(v)) => v.draft.unwrap(),
        other => panic!("expected draft ok, got {other:?}"),
    };
    assert!(draft.preview_delta_pre_tax_dv01.is_some());

    // NLP_PARSER cannot confirm its own draft -> FORBIDDEN
    let parser_confirm = client
        .confirm_draft_earmark(with_bearer(
            pb::ConfirmDraftEarmarkRequest {
                idempotency_key: "confirm-draft-1".to_owned(),
                account_id: "acct-A".to_owned(),
                draft_id: draft.draft_id.clone(),
                expected_draft_revision: draft.draft_revision,
                expected_position_version: 1,
                confirmation_token: draft.confirmation_token.clone(),
                acknowledge_uncovered_dv01: false,
            },
            PARSER_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        parser_confirm.result,
        Some(pb::confirm_draft_earmark_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Forbidden
    ));

    // TRADER confirms draft -> succeeds with confirmed_by_trader_id = "trader-ab"
    let trader_confirm = client
        .confirm_draft_earmark(with_bearer(
            pb::ConfirmDraftEarmarkRequest {
                idempotency_key: "confirm-draft-1".to_owned(),
                account_id: "acct-A".to_owned(),
                draft_id: draft.draft_id.clone(),
                expected_draft_revision: draft.draft_revision,
                expected_position_version: 1,
                confirmation_token: draft.confirmation_token.clone(),
                acknowledge_uncovered_dv01: false,
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    match trader_confirm.result {
        Some(pb::confirm_draft_earmark_response::Result::Value(v)) => {
            assert_eq!(
                v.draft.unwrap().confirmed_by_trader_id.as_deref(),
                Some("trader-ab")
            );
        }
        other => panic!("expected draft confirm ok, got {other:?}"),
    }

    // Expired ConfirmHold lazy-expiration rejection + committed_seq + idempotent replay:
    // 1. Create SELL HOLD for 100M on acct-B (consumes all 100M available).
    let hold_b = match client
        .create_hold(with_bearer(
            pb::CreateHoldRequest {
                idempotency_key: "hold-exp-1".to_owned(),
                account_id: "acct-B".to_owned(),
                expected_position_version: 1,
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Sell as i32,
                settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                notional: Some(dec("100000000", cur)),
                quote_terms: None,
                ttl_seconds: 1800,
                note: "expiring".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::create_hold_response::Result::Value(v)) => v.earmark.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };

    // 2. Advance clock to exact expires_at (+1800s) and pause operator feed so re-validation FAILS!
    srv.clock.advance_seconds(1800).unwrap();
    let _ = client
        .set_feed_control(with_bearer(
            pb::SetFeedControlRequest {
                idempotency_key: "pause-op".to_owned(),
                source_id: "SRC-SHARED".to_owned(),
                expected_feed_status_revision: 1,
                paused_by_operator: true,
                force_clear_gap_pause: false,
                reason: "pause before expired confirm".to_owned(),
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap();

    // 3. Call ConfirmHold on expired HOLD -> commits EXPIRED + reconfirmation_error!
    let exp_req = pb::ConfirmHoldRequest {
        idempotency_key: "confirm-expired-1".to_owned(),
        account_id: "acct-B".to_owned(),
        earmark_id: hold_b.earmark_id.clone(),
        expected_earmark_revision: hold_b.earmark_revision,
        expected_position_version: 2,
        quote_terms: Some(pb::QuoteTerms {
            price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                "100.0", px,
            ))),
            settlement_amount: Some(dec("100000000", cur)),
        }),
    };
    let exp_res_1 = match client
        .confirm_hold(with_bearer(exp_req.clone(), TRADER_AB_TOKEN))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::confirm_hold_response::Result::Value(v)) => v,
        other => panic!("expected committed lazy-expiry value, got {other:?}"),
    };
    assert!(!exp_res_1.replayed);
    assert_eq!(
        exp_res_1.earmark.as_ref().unwrap().state,
        pb::EarmarkState::Expired as i32
    );
    assert!(exp_res_1.reconfirmation_error.is_some());

    // 4. Retry exact same ConfirmHold idempotency key -> replays committed EXPIRED receipt!
    let exp_res_2 = match client
        .confirm_hold(with_bearer(exp_req, TRADER_AB_TOKEN))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::confirm_hold_response::Result::Value(v)) => v,
        other => panic!("expected replayed lazy-expiry value, got {other:?}"),
    };
    assert!(exp_res_2.replayed);
    assert_eq!(exp_res_2.committed_seq, exp_res_1.committed_seq);
    assert_eq!(exp_res_2.earmark, exp_res_1.earmark);
    assert_eq!(
        exp_res_2.reconfirmation_error,
        exp_res_1.reconfirmation_error
    );

    let _ = policy_provider;
}

// ============================================================================
// TEST 5: All 4 CSV & Mock SPI Services (Unary + Streaming + Auth + Bounds)
// ============================================================================

#[tokio::test]
async fn four_csv_and_mock_spi_services_unary_stream_and_auth() {
    let units = default_reference_analysis_units().unwrap();
    let accts_csv = "\
account_id,base_id,trading_date,next_settlement_date,effective_from_rfc3339,watermark_time_rfc3339,market_timezone,watermark_source_seq,source_id,opening_cash_t0,opening_net_cash_delta_t1,source_start_after_feed_seq
acct-A,01ARZ3NDEKTSV4RRFFQ69G5FA1,2026-06-15,2026-06-16,2026-06-15T09:00:00+08:00,2026-06-15T09:00:00+08:00,Asia/Shanghai,0,SRC-SHARED,100000000,10000000,0
";
    let bals_csv = "\
account_id,trading_date,instrument_code,opening_economic_notional,opening_available_notional,opening_net_t0_notional,opening_net_t1_notional,eligibility_class,calendar_requirement
acct-A,2026-06-15,BOND_10Y,50000000,50000000,0,0,SOVEREIGN_OR_POLICY_FIXED,REFERENCE_REPLAY
";
    let feed_csv = "\
source_id,external_trade_id,source_revision,feed_seq,action,account_id,instrument_code,side,settlement_date,notional,clean_price_per_100,yield_to_maturity,settlement_amount,executed_at_rfc3339,market_timezone,batch_first_feed_seq,batch_last_feed_seq
SRC-SHARED,T-CSV-1,1,1,NEW_FILL,acct-A,BOND_10Y,BUY,2026-06-15,10000000,100.25,,10025000,2026-06-15T10:00:00+08:00,Asia/Shanghai,1,1
";
    let quote_csv = "\
source_id,quote_seq,instrument_code,source_type,bid_clean_price,ask_clean_price,mid_clean_price,mid_yield_to_maturity,observed_at_rfc3339,received_at_rfc3339,market_timezone
Q-SHARED,1,BOND_10Y,ACTIVE_QUOTE,100.10,100.20,100.15,0.025,2026-06-15T09:59:00+08:00,2026-06-15T09:59:01+08:00,Asia/Shanghai
";
    let nlp_csv = "\
account_id,raw_source_text,parser_source_id,model_version,confidence_score,instrument_code,side,settlement_bucket,notional,clean_price_per_100,yield_to_maturity,settlement_amount,suggested_target_state,anchor_field,anchor_span
acct-A,BUY 10M BOND_10Y,csv-nlp,1.0,0.92,BOND_10Y,BUY,T_PLUS_0,10000000,100.15,,10015000,HOLD,side,BUY
";

    let pos_spi =
        Arc::new(CsvPositionBaseProvider::from_csv_str(accts_csv, bals_csv, &units).unwrap());
    let feed_spi = Arc::new(CsvTradeFeedReconciler::from_csv_str(feed_csv).unwrap());
    let quote_spi = Arc::new(CsvMarketQuoteStream::from_csv_str(quote_csv).unwrap());
    let nlp_spi = Arc::new(CsvTemplateNlpIntentParser::from_csv_str(nlp_csv).unwrap());

    let srv = LiveTestServer::start_with_spis(
        Arc::new(TestPolicyProvider::new(default_test_policy(true))),
        DeskBlotterServiceConfig::default(),
        100,
        32,
        pos_spi,
        feed_spi,
        quote_spi,
        nlp_spi,
    )
    .await;

    let endpoint = format!("http://{}", srv.addr);
    let mut pos_client = PositionBaseProviderServiceClient::connect(endpoint.clone())
        .await
        .unwrap();
    let mut feed_client = TradeFeedReconcilerServiceClient::connect(endpoint.clone())
        .await
        .unwrap();
    let mut quote_client = MarketQuoteStreamServiceClient::connect(endpoint.clone())
        .await
        .unwrap();
    let mut nlp_client = IntranetNlpIntentParserServiceClient::connect(endpoint)
        .await
        .unwrap();

    // 1. PositionBaseProviderService
    let pos_ok = pos_client
        .fetch_opening_position_base(with_bearer(
            pb::FetchOpeningPositionBaseRequest {
                request_id: "r1".to_owned(),
                account_id: "acct-A".to_owned(),
                trading_date: "2026-06-15".to_owned(),
            },
            FEED_A_ONLY_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        pos_ok.result,
        Some(pb::fetch_opening_position_base_response::Result::Value(ref b))
        if b.account_id == "acct-A"
    ));

    // Trader calling PositionBaseProviderService -> FORBIDDEN
    let pos_forbidden = pos_client
        .fetch_opening_position_base(with_bearer(
            pb::FetchOpeningPositionBaseRequest {
                request_id: "r2".to_owned(),
                account_id: "acct-A".to_owned(),
                trading_date: "2026-06-15".to_owned(),
            },
            TRADER_A_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        pos_forbidden.result,
        Some(pb::fetch_opening_position_base_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Forbidden
    ));

    // 2. TradeFeedReconcilerService: Pull + Stream + FetchTradeRevision + page bounds (0 & 1025)
    let bad_page = feed_client
        .pull_trade_feed_batch(with_bearer(
            pb::PullTradeFeedBatchRequest {
                source_id: "SRC-SHARED".to_owned(),
                after_feed_seq: 0,
                max_records: 1025,
            },
            FEED_A_ONLY_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        bad_page.result,
        Some(pb::pull_trade_feed_batch_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed
    ));

    let pull_ok = feed_client
        .pull_trade_feed_batch(with_bearer(
            pb::PullTradeFeedBatchRequest {
                source_id: "SRC-SHARED".to_owned(),
                after_feed_seq: 0,
                max_records: 10,
            },
            FEED_A_ONLY_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    match pull_ok.result {
        Some(pb::pull_trade_feed_batch_response::Result::Value(v)) => {
            assert_eq!(v.fills.len(), 1);
            assert_eq!(v.scanned_through_feed_seq, 1);
        }
        other => panic!("unexpected: {other:?}"),
    }

    let missing_rev = feed_client
        .fetch_trade_revision(with_bearer(
            pb::FetchTradeRevisionRequest {
                source_id: "SRC-SHARED".to_owned(),
                external_trade_id: "T-CSV-1".to_owned(),
                source_revision: 99,
            },
            FEED_A_ONLY_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        missing_rev.result,
        Some(pb::fetch_trade_revision_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::NotFound
    ));

    let mut feed_stream = feed_client
        .stream_trade_feed(with_bearer(
            pb::StreamTradeFeedRequest {
                source_id: "SRC-SHARED".to_owned(),
                after_feed_seq: 0,
            },
            FEED_A_ONLY_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    let f1 = feed_stream.next().await.unwrap().unwrap();
    assert!(matches!(
        f1.payload,
        Some(pb::stream_trade_feed_response::Payload::Fill(_))
    ));
    let f2 = feed_stream.next().await.unwrap().unwrap();
    assert!(matches!(
        f2.payload,
        Some(pb::stream_trade_feed_response::Payload::Heartbeat(_))
    ));

    // 3. MarketQuoteStreamService: Pull + Stream
    let q_pull = quote_client
        .pull_market_quotes(with_bearer(
            pb::PullMarketQuotesRequest {
                cursor: Some(pb::QuoteCursor {
                    source_id: "Q-SHARED".to_owned(),
                    instrument_code: "BOND_10Y".to_owned(),
                    after_quote_seq: 0,
                }),
                max_ticks: 10,
            },
            FEED_A_ONLY_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    match q_pull.result {
        Some(pb::pull_market_quotes_response::Result::Value(v)) => {
            assert_eq!(v.ticks.len(), 1);
            assert_eq!(v.next_cursor.unwrap().after_quote_seq, 1);
        }
        other => panic!("unexpected: {other:?}"),
    }

    let mut q_stream = quote_client
        .stream_market_quotes(with_bearer(
            pb::StreamMarketQuotesRequest {
                cursor: Some(pb::QuoteCursor {
                    source_id: "Q-SHARED".to_owned(),
                    instrument_code: "BOND_10Y".to_owned(),
                    after_quote_seq: 0,
                }),
            },
            FEED_A_ONLY_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        q_stream.next().await.unwrap().unwrap().payload,
        Some(pb::stream_market_quotes_response::Payload::Tick(_))
    ));
    assert!(matches!(
        q_stream.next().await.unwrap().unwrap().payload,
        Some(pb::stream_market_quotes_response::Payload::Heartbeat(_))
    ));

    // 4. IntranetNlpIntentParserService (CSV backend via gRPC-Web!)
    let nlp_web: pb::ParseNlpIntentResponse = grpc_web_unary(
        srv.addr,
        "/ficant.blotter.v1.IntranetNlpIntentParserService/ParseNlpIntent",
        &pb::ParseNlpIntentRequest {
            request_id: "nlp-1".to_owned(),
            account_id: "acct-A".to_owned(),
            raw_source_text: "BUY 10M BOND_10Y".to_owned(),
        },
        Some((AUTHORIZATION_HEADER, &format!("Bearer {PARSER_TOKEN}"))),
    )
    .await;
    match nlp_web.result {
        Some(pb::parse_nlp_intent_response::Result::Value(p)) => {
            assert_eq!(p.instrument_code, "BOND_10Y");
            assert_eq!(p.anchors.len(), 1);
            assert_eq!(p.anchors[0].span_text, "BUY");
        }
        other => panic!("unexpected nlp response: {other:?}"),
    }

    // FeedAdapter calling NLP SPI -> FORBIDDEN
    let nlp_forbidden = nlp_client
        .parse_nlp_intent(with_bearer(
            pb::ParseNlpIntentRequest {
                request_id: "nlp-2".to_owned(),
                account_id: "acct-A".to_owned(),
                raw_source_text: "BUY 10M BOND_10Y".to_owned(),
            },
            FEED_A_ONLY_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        nlp_forbidden.result,
        Some(pb::parse_nlp_intent_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Forbidden
    ));
}

// ============================================================================
// TEST 6: Seamless Lag Catch-Up (Small Bus, Large Retention) & gRPC-Web Stream
// ============================================================================

#[tokio::test]
async fn subscribe_lag_seamless_catchup_and_grpc_web_subscribe_stream() {
    let policy_provider = Arc::new(TestPolicyProvider::new(default_test_policy(true)));
    // Large retention window (1000), tiny broadcast bus (2), stream channel = 1 -> lag triggers seamless poll_events catch-up!
    let lag_cfg = DeskBlotterServiceConfig {
        subscribe_stream_channel_capacity: 1,
        ..DeskBlotterServiceConfig::default()
    };
    let srv = LiveTestServer::start(policy_provider, lag_cfg, 1_000, 2).await;
    let mut client = srv.client().await;
    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();

    let base_a = make_opening_base(
        "acct-A",
        "K1",
        "100000000",
        "100",
        "2030-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2031-12-31",
    );
    let imp = client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "imp-lag-ok".to_owned(),
                opening_base: Some(base_a),
                expected_account_version: 0,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        imp.result,
        Some(pb::import_opening_base_response::Result::Value(_))
    ));

    let mut sub = client
        .subscribe_events(with_bearer(
            pb::SubscribeEventsRequest {
                after_seq: 1,
                account_id: None,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    let _hb = tokio::time::timeout(Duration::from_secs(3), sub.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Commit 6 holds rapidly (exceeding bus cap = 2, well within retention = 1000)
    let mut pos_ver = 1_u64;
    for i in 1..=6_u32 {
        let resp = client
            .create_hold(with_bearer(
                pb::CreateHoldRequest {
                    idempotency_key: format!("lag-h-{i}"),
                    account_id: "acct-A".to_owned(),
                    expected_position_version: pos_ver,
                    instrument_code: "BOND_10Y".to_owned(),
                    side: pb::TradeSide::Buy as i32,
                    settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                    notional: Some(dec("1000000", cur)),
                    quote_terms: None,
                    ttl_seconds: 1800,
                    note: format!("lag-{i}"),
                },
                TRADER_AB_TOKEN,
            ))
            .await
            .unwrap()
            .into_inner();
        match resp.result {
            Some(pb::create_hold_response::Result::Value(v)) => {
                pos_ver = v.position.unwrap().position_version;
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // Drain subscriber -> MUST receive all 6 seqs (2..=7) in strict monotonic order without fallback!
    let mut seen_seqs = Vec::new();
    while seen_seqs.len() < 6 {
        let item = tokio::time::timeout(Duration::from_secs(3), sub.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match item.payload {
            Some(pb::subscribe_events_response::Payload::Event(ev)) => {
                seen_seqs.push(ev.seq);
            }
            Some(pb::subscribe_events_response::Payload::Heartbeat(_)) => {}
            other => panic!("unexpected payload during lag catch-up: {other:?}"),
        }
    }
    assert_eq!(seen_seqs, vec![2, 3, 4, 5, 6, 7]);

    // Raw HTTP/1.1 gRPC-Web server-streaming SubscribeEvents test (on future cursor -> 1 error frame + trailer)
    let web_err_frames = grpc_web_call_raw(
        srv.addr,
        "/ficant.blotter.v1.DeskBlotterService/SubscribeEvents",
        &pb::SubscribeEventsRequest {
            after_seq: 999,
            account_id: None,
        },
        Some((AUTHORIZATION_HEADER, &format!("Bearer {OPERATOR_TOKEN}"))),
    )
    .await;
    assert_eq!(web_err_frames.len(), 1);
    let web_sub_resp = pb::SubscribeEventsResponse::decode(web_err_frames[0].as_slice()).unwrap();
    assert!(matches!(
        web_sub_resp.payload,
        Some(pb::subscribe_events_response::Payload::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed
    ));
}

// ============================================================================
// TEST 7: RulePack Error Matrix (Missing/Corrupted/Expired vs Valid+Uncovered)
//         + Zero-Exposure Hedge + 100/60/40 Oversell & Zero-Base BUY HOLD
// ============================================================================

#[tokio::test]
async fn rulepack_error_matrix_zero_exposure_hedge_and_100_60_40_ambiguity() {
    // 1. Account A: missing regulatory rule pack -> draft rejected even with ack=true
    let policy_provider = Arc::new(TestPolicyProvider::new(default_test_policy(false)));
    // Account B: valid regulatory rule pack + credit bond (uncovered DV01) + zero-position bond
    policy_provider.set_account_policy("acct-B", default_test_policy(true));

    let srv = LiveTestServer::start(
        policy_provider,
        DeskBlotterServiceConfig::default(),
        1_000,
        32,
    )
    .await;
    let mut client = srv.client().await;
    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();
    let px = u.price_per_100.as_ref().unwrap();
    let dv100 = u.dv01_per_100.as_ref().unwrap();
    let dim = u.dimensionless.as_ref().unwrap();

    // acct-A: 100M position for 100/60/40 + missing rule pack test
    let base_a = make_opening_base(
        "acct-A",
        "R1",
        "100000000",
        "100",
        "2030-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2031-12-31",
    );
    let _ = client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "imp-r1".to_owned(),
                opening_base: Some(base_a),
                expected_account_version: 0,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap();

    // Draft on acct-A (missing regulatory rule pack) -> ValidationFailed with regulatory_rule_pack violation
    let raw_text = "BUY T+0 INST=BOND_10Y NOTIONAL=10000000 CLEAN=100.0 TARGET=COMMITTED";
    let missing_pack_draft = client
        .submit_draft_earmark(with_bearer(
            pb::SubmitDraftEarmarkRequest {
                idempotency_key: "d-missing".to_owned(),
                account_id: "acct-A".to_owned(),
                proposal: Some(pb::NlpEarmarkProposal {
                    parser_source_id: "nlp-bot-1".to_owned(),
                    model_version: "1.0".to_owned(),
                    raw_source_text: raw_text.to_owned(),
                    anchors: vec![pb::SourceTextAnchor {
                        field_name: "side".to_owned(),
                        byte_start: 0,
                        byte_end: 3,
                        span_text: "BUY".to_owned(),
                    }],
                    confidence_score: Some(dec("0.9", dim)),
                    account_id: "acct-A".to_owned(),
                    instrument_code: "BOND_10Y".to_owned(),
                    side: pb::TradeSide::Buy as i32,
                    settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                    notional: Some(dec("10000000", cur)),
                    quote_terms: Some(pb::QuoteTerms {
                        price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                            "100.0", px,
                        ))),
                        settlement_amount: Some(dec("10000000", cur)),
                    }),
                    suggested_target_state: pb::DraftTargetState::Committed as i32,
                }),
            },
            PARSER_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        missing_pack_draft.result,
        Some(pb::submit_draft_earmark_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed
    ));

    // acct-B: zero-base bond (for zero-base BUY HOLD + zero net exposure hedge!) + credit bond (uncovered!)
    let mut base_b = make_opening_base(
        "acct-B",
        "R2",
        "0",
        "100",
        "2030-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2031-12-31",
    );
    // Add a covered long + equal short bond so net covered DV01 == 0!
    base_b.balances.push(pb::OpeningInstrumentBalance {
        instrument_code: "BOND_LONG".to_owned(),
        opening_economic_notional: Some(dec("10000000", cur)),
        opening_available_notional: Some(dec("10000000", cur)),
        opening_net_t0_notional: Some(dec("0", cur)),
        opening_net_t1_notional: Some(dec("0", cur)),
    });
    base_b.balances.push(pb::OpeningInstrumentBalance {
        instrument_code: "BOND_SHORT".to_owned(),
        opening_economic_notional: Some(dec("-10000000", cur)),
        opening_available_notional: Some(dec("0", cur)),
        opening_net_t0_notional: Some(dec("0", cur)),
        opening_net_t1_notional: Some(dec("0", cur)),
    });
    base_b.balances.push(pb::OpeningInstrumentBalance {
        instrument_code: "CREDIT_01".to_owned(),
        opening_economic_notional: Some(dec("5000000", cur)),
        opening_available_notional: Some(dec("5000000", cur)),
        opening_net_t0_notional: Some(dec("0", cur)),
        opening_net_t1_notional: Some(dec("0", cur)),
    });
    base_b.instruments.push(pb::InstrumentCatalogEntry {
        instrument_code: "BOND_LONG".to_owned(),
        bond: Some(pb_bond("K2", "100", "2025-01-15", "2030-01-15", "0.025")),
        eligibility_class: pb::RateCoverageEligibility::SovereignOrPolicyFixed as i32,
        calendar_requirement: rates_pb::CalendarRequirement::Unspecified as i32,
    });
    base_b.instruments.push(pb::InstrumentCatalogEntry {
        instrument_code: "BOND_SHORT".to_owned(),
        bond: Some(pb_bond("S1", "100", "2025-01-15", "2030-01-15", "0.025")),
        eligibility_class: pb::RateCoverageEligibility::SovereignOrPolicyFixed as i32,
        calendar_requirement: rates_pb::CalendarRequirement::Unspecified as i32,
    });
    base_b.instruments.push(pb::InstrumentCatalogEntry {
        instrument_code: "CREDIT_01".to_owned(),
        bond: Some(pb_bond("C1", "100", "2025-01-15", "2030-01-15", "0.035")),
        eligibility_class: pb::RateCoverageEligibility::CreditOrSpreadProduct as i32,
        calendar_requirement: rates_pb::CalendarRequirement::Unspecified as i32,
    });

    let _ = client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "imp-r2".to_owned(),
                opening_base: Some(base_b),
                expected_account_version: 0,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap();

    // Zero-base BUY HOLD on BOND_10Y (opening=0) succeeds!
    let zero_buy_hold = client
        .create_hold(with_bearer(
            pb::CreateHoldRequest {
                idempotency_key: "zero-buy".to_owned(),
                account_id: "acct-B".to_owned(),
                expected_position_version: 1,
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Buy as i32,
                settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                notional: Some(dec("5000000", cur)),
                quote_terms: None,
                ttl_seconds: 1800,
                note: "zero-base buy".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        zero_buy_hold.result,
        Some(pb::create_hold_response::Result::Value(_))
    ));

    // Ingest identical quotes on BOND_LONG & BOND_SHORT -> net covered DV01 == 0!
    let _ = client
        .ingest_market_quotes(with_bearer(
            pb::IngestMarketQuotesRequest {
                idempotency_key: "q-zero-net".to_owned(),
                account_id: "acct-B".to_owned(),
                ticks: vec![
                    quote_tick("Q-SHARED", 1, "BOND_LONG", "0.025"),
                    quote_tick("Q-SHARED", 1, "BOND_SHORT", "0.025"),
                ],
                source_id: "Q-SHARED".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap();

    // WHOLE_ACCOUNT hedge blocked because CREDIT_01 is uncovered!
    let whole_blocked = client
        .preview_covered_hedge(with_bearer(
            pb::PreviewCoveredHedgeRequest {
                account_id: "acct-B".to_owned(),
                scope: pb::PreviewHedgeScope::WholeAccount as i32,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: Some(dec("0.075", dv100)),
                conversion_factor: Some(dec("0.985", dim)),
                delivery_rule_pack: Some(futures_v2_pack()),
                valuation_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
                expected_snapshot_seq: None,
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        whole_blocked.result,
        Some(pb::preview_covered_hedge_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed
    ));

    // COVERED_SUBSET_ONLY with net DV01 == 0 -> zero_net_exposure=true, recommended=0, effectiveness=1!
    let zero_hedge = match client
        .preview_covered_hedge(with_bearer(
            pb::PreviewCoveredHedgeRequest {
                account_id: "acct-B".to_owned(),
                scope: pb::PreviewHedgeScope::CoveredSubsetOnly as i32,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: Some(dec("0.075", dv100)),
                conversion_factor: Some(dec("0.985", dim)),
                delivery_rule_pack: Some(futures_v2_pack()),
                valuation_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
                expected_snapshot_seq: None,
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::preview_covered_hedge_response::Result::Value(v)) => v,
        other => panic!("expected zero-exposure hedge ok, got {other:?}"),
    };
    assert!(zero_hedge.zero_net_exposure);
    assert_eq!(
        zero_hedge.measures.as_ref().unwrap().recommended_contracts,
        0
    );

    // Valid rule pack + uncovered CREDIT_01 draft: submit succeeds, confirm without ack fails, with ack succeeds!
    let cred_text = "BUY T+0 INST=CREDIT_01 NOTIONAL=1000000 CLEAN=100.0 TARGET=COMMITTED";
    let cred_draft = match client
        .submit_draft_earmark(with_bearer(
            pb::SubmitDraftEarmarkRequest {
                idempotency_key: "d-cred".to_owned(),
                account_id: "acct-B".to_owned(),
                proposal: Some(pb::NlpEarmarkProposal {
                    parser_source_id: "nlp-bot-1".to_owned(),
                    model_version: "1.0".to_owned(),
                    raw_source_text: cred_text.to_owned(),
                    anchors: vec![pb::SourceTextAnchor {
                        field_name: "side".to_owned(),
                        byte_start: 0,
                        byte_end: 3,
                        span_text: "BUY".to_owned(),
                    }],
                    confidence_score: Some(dec("0.9", dim)),
                    account_id: "acct-B".to_owned(),
                    instrument_code: "CREDIT_01".to_owned(),
                    side: pb::TradeSide::Buy as i32,
                    settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                    notional: Some(dec("1000000", cur)),
                    quote_terms: Some(pb::QuoteTerms {
                        price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                            "100.0", px,
                        ))),
                        settlement_amount: Some(dec("1000000", cur)),
                    }),
                    suggested_target_state: pb::DraftTargetState::Committed as i32,
                }),
            },
            PARSER_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::submit_draft_earmark_response::Result::Value(v)) => v.draft.unwrap(),
        other => panic!("expected credit draft ok, got {other:?}"),
    };
    assert_eq!(
        cred_draft.preview_coverage_status,
        pb::CoverageStatus::Uncovered as i32
    );

    // Without ack -> rejected!
    let no_ack = client
        .confirm_draft_earmark(with_bearer(
            pb::ConfirmDraftEarmarkRequest {
                idempotency_key: "c-cred-noack".to_owned(),
                account_id: "acct-B".to_owned(),
                draft_id: cred_draft.draft_id.clone(),
                expected_draft_revision: cred_draft.draft_revision,
                expected_position_version: 1,
                confirmation_token: cred_draft.confirmation_token.clone(),
                acknowledge_uncovered_dv01: false,
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        no_ack.result,
        Some(pb::confirm_draft_earmark_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed
    ));

    // With explicit ack -> succeeds!
    let with_ack = client
        .confirm_draft_earmark(with_bearer(
            pb::ConfirmDraftEarmarkRequest {
                idempotency_key: "c-cred-ack".to_owned(),
                account_id: "acct-B".to_owned(),
                draft_id: cred_draft.draft_id.clone(),
                expected_draft_revision: cred_draft.draft_revision,
                expected_position_version: 1,
                confirmation_token: cred_draft.confirmation_token.clone(),
                acknowledge_uncovered_dv01: true,
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        with_ack.result,
        Some(pb::confirm_draft_earmark_response::Result::Value(_))
    ));

    // 100/60/40 oversell & ambiguity lock on acct-A:
    // Opening 100M, COMMITTED SELL 60M, independent SELL fill 40M -> available=0, ambiguity_locked=true
    let em_60 = match client
        .create_committed(with_bearer(
            pb::CreateCommittedRequest {
                idempotency_key: "sell-60".to_owned(),
                account_id: "acct-A".to_owned(),
                expected_position_version: 1,
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Sell as i32,
                settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                notional: Some(dec("60000000", cur)),
                quote_terms: Some(pb::QuoteTerms {
                    price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                        "100.0", px,
                    ))),
                    settlement_amount: Some(dec("60000000", cur)),
                }),
                note: "60m".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::create_committed_response::Result::Value(v)) => v.earmark.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };

    let pm_40 = match client
        .ingest_trade_feed(with_bearer(
            pb::IngestTradeFeedRequest {
                idempotency_key: "fill-40".to_owned(),
                source_id: "SRC-SHARED".to_owned(),
                fills: vec![pb::TradeFillRecord {
                    source_id: "SRC-SHARED".to_owned(),
                    external_trade_id: "TRD-40M".to_owned(),
                    source_revision: 1,
                    feed_seq: 1,
                    action: pb::TradeFeedRecordAction::NewFill as i32,
                    account_id: "acct-A".to_owned(),
                    instrument_code: "BOND_10Y".to_owned(),
                    side: pb::TradeSide::Sell as i32,
                    settlement_date: "2026-06-15".to_owned(),
                    notional: Some(dec("40000000", cur)),
                    quote_terms: Some(pb::QuoteTerms {
                        price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                            "100.0", px,
                        ))),
                        settlement_amount: Some(dec("40000000", cur)),
                    }),
                    executed_at: Some(pb_time(2026, 6, 15, 10, 1, 0)),
                    batch_first_feed_seq: 1,
                    batch_last_feed_seq: 1,
                }],
                heartbeat_at: Some(pb_time(2026, 6, 15, 10, 1, 0)),
            },
            FEED_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::ingest_trade_feed_response::Result::Value(v)) => {
            v.item_outcomes[0].pending_match_id.clone().unwrap()
        }
        other => panic!("unexpected: {other:?}"),
    };

    let snap_locked = match client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-A".to_owned(),
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::get_account_snapshot_response::Result::Value(v)) => v.snapshot.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };
    assert!(
        snap_locked.positions[0]
            .bucket_t0
            .as_ref()
            .unwrap()
            .sell_ambiguity_locked
    );
    assert!(
        !snap_locked
            .funding_summary
            .as_ref()
            .unwrap()
            .funding_is_complete_exact
    );

    // SPLIT_OFFSET 40M against 60M -> unlocks sell ambiguity and restores 40M available!
    let split_ok = match client
        .resolve_pending_match(with_bearer(
            pb::ResolvePendingMatchRequest {
                idempotency_key: "split-40".to_owned(),
                account_id: "acct-A".to_owned(),
                pending_match_id: Some(pm_40),
                expected_pending_revision: 1,
                expected_position_version: snap_locked.positions[0].position_version,
                action: pb::PendingMatchResolutionAction::SplitOffset as i32,
                expected_earmark_revision: Some(em_60.earmark_revision),
                target_earmark_id: em_60.earmark_id.clone(),
                offset_notional: Some(dec("40000000", cur)),
                reason: "confirm 40M slice of 60M".to_owned(),
                baseline_delta: None,
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::resolve_pending_match_response::Result::Value(v)) => v,
        other => panic!("unexpected: {other:?}"),
    };
    assert!(
        !split_ok
            .position
            .as_ref()
            .unwrap()
            .bucket_t0
            .as_ref()
            .unwrap()
            .sell_ambiguity_locked
    );
    assert_eq!(
        split_ok
            .position
            .as_ref()
            .unwrap()
            .available_t0_notional
            .as_ref()
            .unwrap()
            .coefficient,
        "40000000"
    );
}

// ============================================================================
// TEST 8: M3-F1 Regression — Canonical Request Hashing Preserves UnitRef & Full Bond Terms
// ============================================================================

#[tokio::test]
async fn canonical_request_hash_rejects_unit_ref_and_bond_term_mutations_with_hash_mismatch() {
    let policy_provider = Arc::new(TestPolicyProvider::new(default_test_policy(true)));
    let srv = LiveTestServer::start(
        policy_provider,
        DeskBlotterServiceConfig::default(),
        10_000,
        128,
    )
    .await;
    let mut client = srv.client().await;
    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();
    let px = u.price_per_100.as_ref().unwrap();
    let rate = u.rate.as_ref().unwrap();
    let dim = u.dimensionless.as_ref().unwrap();

    let mut cur_v2 = cur.clone();
    cur_v2.version += 1;
    let mut px_v2 = px.clone();
    px_v2.version += 1;
    let mut rate_v2 = rate.clone();
    rate_v2.version += 1;
    let mut dim_v2 = dim.clone();
    dim_v2.version += 1;

    // 1. ImportOpeningBase: changing coupon_rate, coupon_frequency, tax_attributes, or
    //    cumulative_issued_amount with same idempotency_key MUST return HASH_MISMATCH!
    let mut base_a = make_opening_base(
        "acct-A",
        "H1",
        "100000000",
        "100",
        "2030-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2031-12-31",
    );
    // Attach baseline evidence for later ApplyBaselineDelta testing
    let ev_hash = core_pb::Sha256 {
        value: ContentHash::digest(b"baseline-fill-ev-1")
            .as_bytes()
            .to_vec(),
    };
    let base_ev = pb::BaseEvidence {
        source_id: "SRC-SHARED".to_owned(),
        external_trade_id: "TRD-BASE-1".to_owned(),
        original_feed_seq: 1,
        original_executed_at: Some(pb_time(2026, 6, 15, 8, 55, 0)),
        old_notional: Some(dec("10000000", cur)),
        old_signed_cash: Some(dec("-10000000", cur)),
        settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
        evidence_ref: "ev-ref-001".to_owned(),
        evidence_hash: Some(ev_hash),
        instrument_code: "BOND_10Y".to_owned(),
        side: pb::TradeSide::Buy as i32,
        original_revision: 1,
    };
    base_a.watermark_source_seq = Some(1);
    base_a.source_start_after_feed_seq = 1;
    base_a.baseline_evidence = vec![base_ev.clone()];

    let imp_req = pb::ImportOpeningBaseRequest {
        idempotency_key: "imp-hash-a".to_owned(),
        opening_base: Some(base_a.clone()),
        expected_account_version: 0,
    };
    let imp_ok = client
        .import_opening_base(with_bearer(imp_req.clone(), OPERATOR_TOKEN))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        imp_ok.result,
        Some(pb::import_opening_base_response::Result::Value(ref v)) if !v.replayed
    ));

    // Exact retry -> replayed=true
    let imp_replay = client
        .import_opening_base(with_bearer(imp_req.clone(), OPERATOR_TOKEN))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        imp_replay.result,
        Some(pb::import_opening_base_response::Result::Value(ref v)) if v.replayed
    ));

    // Mutate coupon_rate -> HASH_MISMATCH
    let mut imp_coupon = imp_req.clone();
    imp_coupon.opening_base.as_mut().unwrap().instruments[0]
        .bond
        .as_mut()
        .unwrap()
        .coupon_rate = Some(dec("0.031", rate));
    let r_coupon = client
        .import_opening_base(with_bearer(imp_coupon, OPERATOR_TOKEN))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        r_coupon.result,
        Some(pb::import_opening_base_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));

    // Mutate coupon_frequency -> HASH_MISMATCH
    let mut imp_freq = imp_req.clone();
    imp_freq.opening_base.as_mut().unwrap().instruments[0]
        .bond
        .as_mut()
        .unwrap()
        .coupon_frequency = market_pb::BondCouponFrequency::Annual as i32;
    let r_freq = client
        .import_opening_base(with_bearer(imp_freq, OPERATOR_TOKEN))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        r_freq.result,
        Some(pb::import_opening_base_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));

    // Mutate tax_attributes -> HASH_MISMATCH
    let mut imp_tax = imp_req.clone();
    imp_tax.opening_base.as_mut().unwrap().instruments[0]
        .bond
        .as_mut()
        .unwrap()
        .tax_attributes
        .as_mut()
        .unwrap()
        .value_added_tax_status = market_pb::ValueAddedTaxStatus::Taxable as i32;
    let r_tax = client
        .import_opening_base(with_bearer(imp_tax, OPERATOR_TOKEN))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        r_tax.result,
        Some(pb::import_opening_base_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));

    // Mutate cumulative_issued_amount -> HASH_MISMATCH
    let mut imp_cum = imp_req;
    imp_cum.opening_base.as_mut().unwrap().instruments[0]
        .bond
        .as_mut()
        .unwrap()
        .cumulative_issued_amount = Some(dec("60000000000", cur));
    let r_cum = client
        .import_opening_base(with_bearer(imp_cum, OPERATOR_TOKEN))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        r_cum.result,
        Some(pb::import_opening_base_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));

    // 2. IngestMarketQuotes: changed UnitRef on mid_yield_to_maturity -> HASH_MISMATCH
    let q_req = pb::IngestMarketQuotesRequest {
        idempotency_key: "q-unit-k1".to_owned(),
        account_id: "acct-A".to_owned(),
        ticks: vec![quote_tick("Q-SHARED", 1, "BOND_10Y", "0.025")],
        source_id: "Q-SHARED".to_owned(),
    };
    let q_ok = client
        .ingest_market_quotes(with_bearer(q_req.clone(), TRADER_AB_TOKEN))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        q_ok.result,
        Some(pb::ingest_market_quotes_response::Result::Value(ref v)) if !v.replayed
    ));
    let mut q_bad_unit = q_req.clone();
    q_bad_unit.ticks[0].mid_yield_to_maturity = Some(dec("0.025", &rate_v2));
    let q_mm = client
        .ingest_market_quotes(with_bearer(q_bad_unit, TRADER_AB_TOKEN))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        q_mm.result,
        Some(pb::ingest_market_quotes_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    let q_replay = client
        .ingest_market_quotes(with_bearer(q_req, TRADER_AB_TOKEN))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        q_replay.result,
        Some(pb::ingest_market_quotes_response::Result::Value(ref v)) if v.replayed
    ));

    // 3. CreateHold: changed UnitRef on notional or quote_terms -> HASH_MISMATCH
    let hold_req = pb::CreateHoldRequest {
        idempotency_key: "hold-unit-k1".to_owned(),
        account_id: "acct-A".to_owned(),
        expected_position_version: 1,
        instrument_code: "BOND_10Y".to_owned(),
        side: pb::TradeSide::Sell as i32,
        settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
        notional: Some(dec("10000000", cur)),
        quote_terms: Some(pb::QuoteTerms {
            price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                "100.0", px,
            ))),
            settlement_amount: None,
        }),
        ttl_seconds: 1800,
        note: "unit-check".to_owned(),
    };
    let hold_em = match client
        .create_hold(with_bearer(hold_req.clone(), TRADER_AB_TOKEN))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::create_hold_response::Result::Value(v)) => {
            assert!(!v.replayed);
            v.earmark.unwrap()
        }
        other => panic!("expected hold ok, got {other:?}"),
    };
    let mut hold_bad_notional_unit = hold_req.clone();
    hold_bad_notional_unit.notional = Some(dec("10000000", &cur_v2));
    assert!(matches!(
        client
            .create_hold(with_bearer(hold_bad_notional_unit, TRADER_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::create_hold_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    let mut hold_bad_px_unit = hold_req.clone();
    hold_bad_px_unit.quote_terms = Some(pb::QuoteTerms {
        price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
            "100.0", &px_v2,
        ))),
        settlement_amount: None,
    });
    assert!(matches!(
        client
            .create_hold(with_bearer(hold_bad_px_unit, TRADER_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::create_hold_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    assert!(matches!(
        client
            .create_hold(with_bearer(hold_req, TRADER_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::create_hold_response::Result::Value(ref v)) if v.replayed
    ));

    // 4. ConfirmHold: changed UnitRef on quote_terms -> HASH_MISMATCH
    let confirm_req = pb::ConfirmHoldRequest {
        idempotency_key: "confirm-unit-k1".to_owned(),
        account_id: "acct-A".to_owned(),
        earmark_id: hold_em.earmark_id.clone(),
        expected_earmark_revision: hold_em.earmark_revision,
        expected_position_version: 2,
        quote_terms: Some(pb::QuoteTerms {
            price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                "100.0", px,
            ))),
            settlement_amount: Some(dec("10000000", cur)),
        }),
    };
    assert!(matches!(
        client
            .confirm_hold(with_bearer(confirm_req.clone(), TRADER_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::confirm_hold_response::Result::Value(ref v)) if !v.replayed
    ));
    let mut confirm_bad_unit = confirm_req.clone();
    confirm_bad_unit
        .quote_terms
        .as_mut()
        .unwrap()
        .settlement_amount = Some(dec("10000000", &cur_v2));
    assert!(matches!(
        client
            .confirm_hold(with_bearer(confirm_bad_unit, TRADER_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::confirm_hold_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    assert!(matches!(
        client
            .confirm_hold(with_bearer(confirm_req, TRADER_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::confirm_hold_response::Result::Value(ref v)) if v.replayed
    ));

    // 5. CreateCommitted: changed UnitRef on notional or quote_terms -> HASH_MISMATCH
    let comm_req = pb::CreateCommittedRequest {
        idempotency_key: "comm-unit-k1".to_owned(),
        account_id: "acct-A".to_owned(),
        expected_position_version: 3,
        instrument_code: "BOND_10Y".to_owned(),
        side: pb::TradeSide::Sell as i32,
        settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
        notional: Some(dec("20000000", cur)),
        quote_terms: Some(pb::QuoteTerms {
            price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                "100.0", px,
            ))),
            settlement_amount: Some(dec("20000000", cur)),
        }),
        note: "comm-unit".to_owned(),
    };
    let comm_em = match client
        .create_committed(with_bearer(comm_req.clone(), TRADER_AB_TOKEN))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::create_committed_response::Result::Value(v)) => {
            assert!(!v.replayed);
            v.earmark.unwrap()
        }
        other => panic!("expected committed ok, got {other:?}"),
    };
    let mut comm_bad_unit = comm_req.clone();
    comm_bad_unit.notional = Some(dec("20000000", px)); // swapped unit_id!
    assert!(matches!(
        client
            .create_committed(with_bearer(comm_bad_unit, TRADER_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::create_committed_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    assert!(matches!(
        client
            .create_committed(with_bearer(comm_req, TRADER_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::create_committed_response::Result::Value(ref v)) if v.replayed
    ));

    // 6. SubmitDraftEarmark: changed UnitRef on confidence_score / notional / quote_terms -> HASH_MISMATCH
    let raw_text = "BUY T+0 INST=BOND_10Y NOTIONAL=5000000 CLEAN=100.0 TARGET=COMMITTED";
    let draft_req = pb::SubmitDraftEarmarkRequest {
        idempotency_key: "draft-unit-k1".to_owned(),
        account_id: "acct-A".to_owned(),
        proposal: Some(pb::NlpEarmarkProposal {
            parser_source_id: "nlp-bot-1".to_owned(),
            model_version: "1.0".to_owned(),
            raw_source_text: raw_text.to_owned(),
            anchors: vec![pb::SourceTextAnchor {
                field_name: "side".to_owned(),
                byte_start: 0,
                byte_end: 3,
                span_text: "BUY".to_owned(),
            }],
            confidence_score: Some(dec("0.95", dim)),
            account_id: "acct-A".to_owned(),
            instrument_code: "BOND_10Y".to_owned(),
            side: pb::TradeSide::Buy as i32,
            settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
            notional: Some(dec("5000000", cur)),
            quote_terms: Some(pb::QuoteTerms {
                price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                    "100.0", px,
                ))),
                settlement_amount: Some(dec("5000000", cur)),
            }),
            suggested_target_state: pb::DraftTargetState::Committed as i32,
        }),
    };
    assert!(matches!(
        client
            .submit_draft_earmark(with_bearer(draft_req.clone(), PARSER_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::submit_draft_earmark_response::Result::Value(ref v)) if !v.replayed
    ));
    let mut draft_bad_conf = draft_req.clone();
    draft_bad_conf.proposal.as_mut().unwrap().confidence_score = Some(dec("0.95", &dim_v2));
    assert!(matches!(
        client
            .submit_draft_earmark(with_bearer(draft_bad_conf, PARSER_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::submit_draft_earmark_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    let mut draft_bad_notional = draft_req.clone();
    draft_bad_notional.proposal.as_mut().unwrap().notional = Some(dec("5000000", &cur_v2));
    assert!(matches!(
        client
            .submit_draft_earmark(with_bearer(draft_bad_notional, PARSER_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::submit_draft_earmark_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    assert!(matches!(
        client
            .submit_draft_earmark(with_bearer(draft_req, PARSER_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::submit_draft_earmark_response::Result::Value(ref v)) if v.replayed
    ));

    // 7. IngestTradeFeed: changed UnitRef on fill.notional or fill.quote_terms -> HASH_MISMATCH
    let feed_req = pb::IngestTradeFeedRequest {
        idempotency_key: "feed-unit-k1".to_owned(),
        source_id: "SRC-SHARED".to_owned(),
        fills: vec![
            // 15M SELL -> creates pending match against 20M committed earmark (for SplitOffset test)
            pb::TradeFillRecord {
                source_id: "SRC-SHARED".to_owned(),
                external_trade_id: "TRD-UNIT-1".to_owned(),
                source_revision: 1,
                feed_seq: 2,
                action: pb::TradeFeedRecordAction::NewFill as i32,
                account_id: "acct-A".to_owned(),
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Sell as i32,
                settlement_date: "2026-06-15".to_owned(),
                notional: Some(dec("15000000", cur)),
                quote_terms: Some(pb::QuoteTerms {
                    price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                        "100.0", px,
                    ))),
                    settlement_amount: Some(dec("15000000", cur)),
                }),
                executed_at: Some(pb_time(2026, 6, 15, 10, 1, 0)),
                batch_first_feed_seq: 2,
                batch_last_feed_seq: 3,
            },
            // Amend pre-watermark trade TRD-BASE-1 rev 2 -> creates BaselineRevisionManual pending match
            pb::TradeFillRecord {
                source_id: "SRC-SHARED".to_owned(),
                external_trade_id: "TRD-BASE-1".to_owned(),
                source_revision: 2,
                feed_seq: 3,
                action: pb::TradeFeedRecordAction::Amend as i32,
                account_id: "acct-A".to_owned(),
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Buy as i32,
                settlement_date: "2026-06-15".to_owned(),
                notional: Some(dec("12000000", cur)),
                quote_terms: Some(pb::QuoteTerms {
                    price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                        "100.0", px,
                    ))),
                    settlement_amount: Some(dec("12000000", cur)),
                }),
                executed_at: Some(pb_time(2026, 6, 15, 10, 1, 0)),
                batch_first_feed_seq: 2,
                batch_last_feed_seq: 3,
            },
        ],
        heartbeat_at: Some(pb_time(2026, 6, 15, 10, 1, 0)),
    };
    let (pm_split_id, pm_base_id) = match client
        .ingest_trade_feed(with_bearer(feed_req.clone(), FEED_AB_TOKEN))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::ingest_trade_feed_response::Result::Value(v)) => {
            assert!(!v.replayed);
            (
                v.item_outcomes[0].pending_match_id.clone().unwrap(),
                v.item_outcomes[1].pending_match_id.clone().unwrap(),
            )
        }
        other => panic!("expected feed ingest ok, got {other:?}"),
    };
    let mut feed_bad_notional = feed_req.clone();
    feed_bad_notional.fills[0].notional = Some(dec("15000000", &cur_v2));
    assert!(matches!(
        client
            .ingest_trade_feed(with_bearer(feed_bad_notional, FEED_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::ingest_trade_feed_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    let mut feed_bad_qt = feed_req.clone();
    feed_bad_qt.fills[0].quote_terms = Some(pb::QuoteTerms {
        price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
            "100.0", &px_v2,
        ))),
        settlement_amount: Some(dec("15000000", cur)),
    });
    assert!(matches!(
        client
            .ingest_trade_feed(with_bearer(feed_bad_qt, FEED_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::ingest_trade_feed_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    assert!(matches!(
        client
            .ingest_trade_feed(with_bearer(feed_req, FEED_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::ingest_trade_feed_response::Result::Value(ref v)) if v.replayed
    ));

    // 8. ResolvePendingMatch: changed UnitRef on offset_notional & baseline_delta -> HASH_MISMATCH
    let snap = match client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-A".to_owned(),
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::get_account_snapshot_response::Result::Value(v)) => v.snapshot.unwrap(),
        other => panic!("unexpected snap: {other:?}"),
    };
    let pos_ver = snap.positions[0].position_version;

    let split_req = pb::ResolvePendingMatchRequest {
        idempotency_key: "rpm-offset-unit-k1".to_owned(),
        account_id: "acct-A".to_owned(),
        pending_match_id: Some(pm_split_id),
        expected_pending_revision: 1,
        expected_position_version: pos_ver,
        action: pb::PendingMatchResolutionAction::SplitOffset as i32,
        expected_earmark_revision: Some(comm_em.earmark_revision),
        target_earmark_id: comm_em.earmark_id.clone(),
        offset_notional: Some(dec("15000000", cur)),
        reason: "offset-unit-test".to_owned(),
        baseline_delta: None,
    };
    let split_pos_ver = match client
        .resolve_pending_match(with_bearer(split_req.clone(), TRADER_AB_TOKEN))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::resolve_pending_match_response::Result::Value(v)) => {
            assert!(!v.replayed);
            v.position.unwrap().position_version
        }
        other => panic!("expected split ok, got {other:?}"),
    };
    let mut split_bad_unit = split_req.clone();
    split_bad_unit.offset_notional = Some(dec("15000000", &cur_v2));
    assert!(matches!(
        client
            .resolve_pending_match(with_bearer(split_bad_unit, TRADER_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::resolve_pending_match_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    assert!(matches!(
        client
            .resolve_pending_match(with_bearer(split_req, TRADER_AB_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::resolve_pending_match_response::Result::Value(ref v)) if v.replayed
    ));

    let base_delta_req = pb::ResolvePendingMatchRequest {
        idempotency_key: "rpm-base-unit-k1".to_owned(),
        account_id: "acct-A".to_owned(),
        pending_match_id: Some(pm_base_id),
        expected_pending_revision: 1,
        expected_position_version: split_pos_ver,
        action: pb::PendingMatchResolutionAction::ApplyBaselineDelta as i32,
        expected_earmark_revision: None,
        target_earmark_id: None,
        offset_notional: None,
        reason: "apply baseline amend delta".to_owned(),
        baseline_delta: Some(pb::BaselineDelta {
            delta_notional: Some(dec("2000000", cur)),
            delta_cash: Some(dec("-2000000", cur)),
            settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
            evidence: Some(base_ev),
            reason: "pre-watermark amend +2M".to_owned(),
        }),
    };
    assert!(matches!(
        client
            .resolve_pending_match(with_bearer(base_delta_req.clone(), OPERATOR_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::resolve_pending_match_response::Result::Value(ref v)) if !v.replayed
    ));
    let mut delta_bad_notional = base_delta_req.clone();
    delta_bad_notional
        .baseline_delta
        .as_mut()
        .unwrap()
        .delta_notional = Some(dec("2000000", &cur_v2));
    assert!(matches!(
        client
            .resolve_pending_match(with_bearer(delta_bad_notional, OPERATOR_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::resolve_pending_match_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    let mut delta_bad_ev = base_delta_req.clone();
    delta_bad_ev
        .baseline_delta
        .as_mut()
        .unwrap()
        .evidence
        .as_mut()
        .unwrap()
        .old_notional = Some(dec("10000000", &cur_v2));
    assert!(matches!(
        client
            .resolve_pending_match(with_bearer(delta_bad_ev, OPERATOR_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::resolve_pending_match_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::HashMismatch
    ));
    assert!(matches!(
        client
            .resolve_pending_match(with_bearer(base_delta_req, OPERATOR_TOKEN))
            .await
            .unwrap()
            .into_inner()
            .result,
        Some(pb::resolve_pending_match_response::Result::Value(ref v)) if v.replayed
    ));
}

// ============================================================================
// TEST 9: M3-F2 Regression — Pre-Idempotency Affected-Account RBAC in SetFeedControl
// ============================================================================

#[tokio::test]
async fn set_feed_control_enforces_pre_idempotency_affected_account_rbac_on_shared_source() {
    let policy_provider = Arc::new(TestPolicyProvider::new(default_test_policy(true)));
    let srv = LiveTestServer::start(
        policy_provider,
        DeskBlotterServiceConfig::default(),
        10_000,
        128,
    )
    .await;
    let mut client = srv.client().await;

    // 1. Import ONLY acct-A bound to SRC-SHARED.
    let base_a = make_opening_base(
        "acct-A",
        "S1",
        "100000000",
        "100",
        "2030-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2031-12-31",
    );
    let imp_a = client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "imp-fc-a".to_owned(),
                opening_base: Some(base_a),
                expected_account_version: 0,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        imp_a.result,
        Some(pb::import_opening_base_response::Result::Value(_))
    ));

    // 2. While ONLY acct-A is bound to SRC-SHARED, TRADER_A (acct-A + SRC-SHARED) CAN call
    //    SetFeedControl and seeds an idempotency receipt under `(trader-a, SetFeedControl, SRC-SHARED, fc-seed-key)`.
    let seeded_req = pb::SetFeedControlRequest {
        idempotency_key: "fc-seed-key".to_owned(),
        source_id: "SRC-SHARED".to_owned(),
        expected_feed_status_revision: 1,
        paused_by_operator: true,
        force_clear_gap_pause: false,
        reason: "single-desk maintenance pause".to_owned(),
    };
    let seeded_ok = client
        .set_feed_control(with_bearer(seeded_req.clone(), TRADER_A_TOKEN))
        .await
        .unwrap()
        .into_inner();
    let rev_after_seed = match seeded_ok.result {
        Some(pb::set_feed_control_response::Result::Value(v)) => {
            assert!(!v.replayed);
            v.feed_status.unwrap().feed_status_revision
        }
        other => panic!("expected SetFeedControl ok on single-account source, got {other:?}"),
    };

    // 3. Now import acct-B also bound to SRC-SHARED -> SRC-SHARED affects BOTH acct-A and acct-B!
    let base_b = make_opening_base(
        "acct-B",
        "S2",
        "100000000",
        "100",
        "2030-01-15",
        rates_pb::CalendarRequirement::ReferenceReplay,
        "2031-12-31",
    );
    let imp_b = client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "imp-fc-b".to_owned(),
                opening_base: Some(base_b),
                expected_account_version: 0,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        imp_b.result,
        Some(pb::import_opening_base_response::Result::Value(_))
    ));

    let head_before = srv.store.head_seq().unwrap();

    // 4a. TRADER_A attempting SetFeedControl with a new key on shared SRC-SHARED -> FORBIDDEN, zero writes!
    let unauth_new = client
        .set_feed_control(with_bearer(
            pb::SetFeedControlRequest {
                idempotency_key: "fc-unauth-shared".to_owned(),
                source_id: "SRC-SHARED".to_owned(),
                expected_feed_status_revision: rev_after_seed,
                paused_by_operator: false,
                force_clear_gap_pause: false,
                reason: "unauthorized unpause".to_owned(),
            },
            TRADER_A_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        unauth_new.result,
        Some(pb::set_feed_control_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Forbidden
    ));
    assert_eq!(srv.store.head_seq().unwrap(), head_before);

    // 4b. TRADER_A probing the exact previously seeded idempotency_key (identical payload) -> FORBIDDEN (never replayed)!
    let probe_same = client
        .set_feed_control(with_bearer(seeded_req.clone(), TRADER_A_TOKEN))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        probe_same.result,
        Some(pb::set_feed_control_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Forbidden
    ));

    // 4c. TRADER_A probing the previously seeded idempotency_key with mutated payload -> FORBIDDEN (never HASH_MISMATCH)!
    let mut probe_mutated = seeded_req;
    probe_mutated.paused_by_operator = false;
    let probe_diff = client
        .set_feed_control(with_bearer(probe_mutated, TRADER_A_TOKEN))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        probe_diff.result,
        Some(pb::set_feed_control_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::Forbidden
    ));
    assert_eq!(srv.store.head_seq().unwrap(), head_before);

    // 5. TRADER_AB (authorized for BOTH acct-A and acct-B + SRC-SHARED) succeeds!
    let ab_ok = client
        .set_feed_control(with_bearer(
            pb::SetFeedControlRequest {
                idempotency_key: "fc-ab-unpause".to_owned(),
                source_id: "SRC-SHARED".to_owned(),
                expected_feed_status_revision: rev_after_seed,
                paused_by_operator: false,
                force_clear_gap_pause: false,
                reason: "authorized multi-account unpause".to_owned(),
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        ab_ok.result,
        Some(pb::set_feed_control_response::Result::Value(ref v))
        if !v.feed_status.as_ref().unwrap().paused_by_operator
    ));
}

// ============================================================================
// BP-4 F-08 / F-22(3) / AC-008: Server-Enforced Ledger Epoch/Instance Fence
// ============================================================================

fn with_bearer_and_fence<T>(
    msg: T,
    token: &str,
    epoch: Option<&str>,
    instance: Option<&str>,
) -> Request<T> {
    let mut req = with_bearer(msg, token);
    if let Some(e) = epoch {
        req.metadata_mut().insert(
            EXPECTED_LEDGER_EPOCH_HEADER,
            MetadataValue::try_from(e).unwrap(),
        );
    }
    if let Some(i) = instance {
        req.metadata_mut().insert(
            EXPECTED_LEDGER_INSTANCE_HEADER,
            MetadataValue::try_from(i).unwrap(),
        );
    }
    req
}

#[tokio::test]
async fn bp4_f08_ledger_fence_enforced_across_all_three_sidecar_write_rpcs_with_zero_writes() {
    // 1. Unit assertions on metadata parser
    assert!(is_canonical_ulid("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
    assert!(!is_canonical_ulid("01arz3ndektsv4rrffq69g5fav")); // lowercase
    assert!(!is_canonical_ulid("01ARZ3NDEKTSV4RRFFQ69G5FA")); // 25 chars
    assert!(!is_canonical_ulid("01ARZ3NDEKTSV4RRFFQ69G5FAI")); // 'I' invalid Crockford
    assert!(!is_canonical_ulid("81ARZ3NDEKTSV4RRFFQ69G5FAV")); // overflow first char > '7'

    let empty_meta = tonic::metadata::MetadataMap::new();
    assert_eq!(extract_expected_ledger_fence(&empty_meta).unwrap(), None);

    let mut dup_epoch = tonic::metadata::MetadataMap::new();
    dup_epoch.append(
        EXPECTED_LEDGER_EPOCH_HEADER,
        MetadataValue::try_from("1").unwrap(),
    );
    dup_epoch.append(
        EXPECTED_LEDGER_EPOCH_HEADER,
        MetadataValue::try_from("1").unwrap(),
    );
    dup_epoch.insert(
        EXPECTED_LEDGER_INSTANCE_HEADER,
        MetadataValue::try_from("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap(),
    );
    assert_eq!(
        extract_expected_ledger_fence(&dup_epoch)
            .unwrap_err()
            .category(),
        ficant_application::ApplicationErrorCategory::ValidationFailed
    );

    // 2. Live server tests across IngestMarketQuotes, IngestTradeFeed, SubmitDraftEarmark
    let policy = Arc::new(TestPolicyProvider::new(default_test_policy(true)));
    let srv = LiveTestServer::start(policy, DeskBlotterServiceConfig::default(), 1000, 64).await;
    let mut client = srv.client().await;

    let actual_id = srv.store.ledger_identity().unwrap();
    let actual_epoch_str = actual_id.ledger_epoch.to_string();
    let wrong_epoch_str = (actual_id.ledger_epoch + 1).to_string();
    let wrong_instance = if actual_id.ledger_instance_id == "01ARZ3NDEKTSV4RRFFQ69G5FAV" {
        "01ARZ3NDEKTSV4RRFFQ69G5F00"
    } else {
        "01ARZ3NDEKTSV4RRFFQ69G5FAV"
    };

    // Import opening base for acct-A so all 3 RPCs can execute cleanly on valid requests.
    let imp = client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "f08-imp-a".to_owned(),
                opening_base: Some(make_opening_base(
                    "acct-A",
                    "F8",
                    "100000000",
                    "100",
                    "2030-01-15",
                    rates_pb::CalendarRequirement::ReferenceReplay,
                    "2032-12-31",
                )),
                expected_account_version: 0,
            },
            OPERATOR_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        imp.result,
        Some(pb::import_opening_base_response::Result::Value(_))
    ));

    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();
    let rate = u.rate.as_ref().unwrap();

    // --- RPC 1: IngestMarketQuotes ---
    let make_quote_req = |idem: &str, seq: u64| pb::IngestMarketQuotesRequest {
        idempotency_key: idem.to_owned(),
        account_id: "acct-A".to_owned(),
        source_id: "Q-SHARED".to_owned(),
        ticks: vec![pb::MarketQuoteTick {
            source_id: "Q-SHARED".to_owned(),
            quote_seq: seq,
            instrument_code: "BOND_10Y".to_owned(),
            source_type: market_pb::PriceSourceType::ActiveQuote as i32,
            bid_clean_price: None,
            ask_clean_price: None,
            mid_clean_price: None,
            mid_yield_to_maturity: Some(dec("0.025", rate)),
            observed_at: Some(pb_time(2026, 6, 15, 9, 59, 0)),
            received_at: Some(pb_time(2026, 6, 15, 9, 59, 5)),
        }],
    };

    let head_before_q = srv.store.head_seq().unwrap();
    // Malformed / half-present headers -> VALIDATION_FAILED, zero writes
    for (ep, inst) in [
        (Some(actual_epoch_str.as_str()), None),
        (None, Some(actual_id.ledger_instance_id.as_str())),
        (Some("0"), Some(actual_id.ledger_instance_id.as_str())),
        (Some("01"), Some(actual_id.ledger_instance_id.as_str())),
        (Some("+1"), Some(actual_id.ledger_instance_id.as_str())),
        (
            Some(actual_epoch_str.as_str()),
            Some("01arz3ndektsv4rrffq69g5fav"),
        ),
    ] {
        let resp = client
            .ingest_market_quotes(with_bearer_and_fence(
                make_quote_req("f08-q-bad", 1),
                FEED_A_ONLY_TOKEN,
                ep,
                inst,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            resp.result,
            Some(pb::ingest_market_quotes_response::Result::Error(ref e))
            if extract_err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed && !e.retryable
        ));
    }
    // Wrong epoch & wrong instance -> non-retryable CONCURRENCY_CONFLICT, zero writes
    for (ep, inst) in [
        (
            wrong_epoch_str.as_str(),
            actual_id.ledger_instance_id.as_str(),
        ),
        (actual_epoch_str.as_str(), wrong_instance),
    ] {
        let resp = client
            .ingest_market_quotes(with_bearer_and_fence(
                make_quote_req("f08-q-conflict", 1),
                FEED_A_ONLY_TOKEN,
                Some(ep),
                Some(inst),
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            resp.result,
            Some(pb::ingest_market_quotes_response::Result::Error(ref e))
            if extract_err_code(Some(e)) == core_pb::ErrorCode::ConcurrencyConflict && !e.retryable
        ));
    }
    assert_eq!(srv.store.head_seq().unwrap(), head_before_q);
    assert!(
        srv.store
            .with_read_barrier(|s, _, _| s.quotes.history.is_empty())
            .unwrap()
    );

    // Same idempotency key with MATCHING fence succeeds with replayed=false (proving zero receipt saved on rejection!)
    let q_ok = client
        .ingest_market_quotes(with_bearer_and_fence(
            make_quote_req("f08-q-conflict", 1),
            FEED_A_ONLY_TOKEN,
            Some(&actual_epoch_str),
            Some(&actual_id.ledger_instance_id),
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        q_ok.result,
        Some(pb::ingest_market_quotes_response::Result::Value(ref v)) if !v.replayed && v.applied_tick_count == 1
    ));

    // Replaying an EXISTING committed idempotency key with STALE fence must STILL fail with
    // non-retryable CONCURRENCY_CONFLICT (fence runs strictly BEFORE query_idempotency_row!).
    let q_stale_replay = client
        .ingest_market_quotes(with_bearer_and_fence(
            make_quote_req("f08-q-conflict", 1),
            FEED_A_ONLY_TOKEN,
            Some(&wrong_epoch_str),
            Some(&actual_id.ledger_instance_id),
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        q_stale_replay.result,
        Some(pb::ingest_market_quotes_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ConcurrencyConflict && !e.retryable
    ));

    // Both headers absent -> legacy compatible
    let q_legacy = client
        .ingest_market_quotes(with_bearer(
            make_quote_req("f08-q-legacy", 2),
            FEED_A_ONLY_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        q_legacy.result,
        Some(pb::ingest_market_quotes_response::Result::Value(ref v)) if !v.replayed && v.applied_tick_count == 1
    ));

    // --- RPC 2: IngestTradeFeed ---
    let px = u.price_per_100.as_ref().unwrap();
    let dim = u.dimensionless.as_ref().unwrap();
    let make_trade_req = |idem: &str, feed_seq: u64, ext_id: &str| pb::IngestTradeFeedRequest {
        idempotency_key: idem.to_owned(),
        source_id: "SRC-SHARED".to_owned(),
        fills: vec![pb::TradeFillRecord {
            source_id: "SRC-SHARED".to_owned(),
            external_trade_id: ext_id.to_owned(),
            source_revision: 1,
            feed_seq,
            action: pb::TradeFeedRecordAction::NewFill as i32,
            account_id: "acct-A".to_owned(),
            instrument_code: "BOND_10Y".to_owned(),
            side: pb::TradeSide::Buy as i32,
            settlement_date: "2026-06-15".to_owned(),
            notional: Some(dec("1000000", cur)),
            quote_terms: Some(pb::QuoteTerms {
                price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                    "100.0", px,
                ))),
                settlement_amount: Some(dec("1000000", cur)),
            }),
            executed_at: Some(pb_time(2026, 6, 15, 9, 58, 0)),
            batch_first_feed_seq: feed_seq,
            batch_last_feed_seq: feed_seq,
        }],
        heartbeat_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
    };

    let head_before_t = srv.store.head_seq().unwrap();
    // Duplicate instance header -> VALIDATION_FAILED
    let mut dup_t = with_bearer(
        make_trade_req("f08-t-bad", 1, "EXT-F08-1"),
        FEED_A_ONLY_TOKEN,
    );
    dup_t.metadata_mut().insert(
        EXPECTED_LEDGER_EPOCH_HEADER,
        MetadataValue::try_from(actual_epoch_str.as_str()).unwrap(),
    );
    dup_t.metadata_mut().append(
        EXPECTED_LEDGER_INSTANCE_HEADER,
        MetadataValue::try_from(actual_id.ledger_instance_id.as_str()).unwrap(),
    );
    dup_t.metadata_mut().append(
        EXPECTED_LEDGER_INSTANCE_HEADER,
        MetadataValue::try_from(actual_id.ledger_instance_id.as_str()).unwrap(),
    );
    let t_dup_resp = client.ingest_trade_feed(dup_t).await.unwrap().into_inner();
    assert!(matches!(
        t_dup_resp.result,
        Some(pb::ingest_trade_feed_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed && !e.retryable
    ));

    // Wrong instance -> non-retryable CONCURRENCY_CONFLICT, zero rows / events / head_seq
    let t_conflict = client
        .ingest_trade_feed(with_bearer_and_fence(
            make_trade_req("f08-t-conflict", 1, "EXT-F08-1"),
            FEED_A_ONLY_TOKEN,
            Some(&actual_epoch_str),
            Some(wrong_instance),
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        t_conflict.result,
        Some(pb::ingest_trade_feed_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ConcurrencyConflict && !e.retryable
    ));
    assert_eq!(srv.store.head_seq().unwrap(), head_before_t);
    assert!(
        srv.store
            .with_read_barrier(|s, _, _| {
                s.domain
                    .feeds
                    .get("SRC-SHARED")
                    .is_none_or(|f| f.raw_packets.is_empty())
            })
            .unwrap()
    );

    // Matching fence with same idempotency_key commits cleanly with replayed=false
    let t_ok = client
        .ingest_trade_feed(with_bearer_and_fence(
            make_trade_req("f08-t-conflict", 1, "EXT-F08-1"),
            FEED_A_ONLY_TOKEN,
            Some(&actual_epoch_str),
            Some(&actual_id.ledger_instance_id),
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        t_ok.result,
        Some(pb::ingest_trade_feed_response::Result::Value(ref v)) if !v.replayed
    ));

    // --- RPC 3: SubmitDraftEarmark ---
    let make_draft_req = |idem: &str| pb::SubmitDraftEarmarkRequest {
        idempotency_key: idem.to_owned(),
        account_id: "acct-A".to_owned(),
        proposal: Some(pb::NlpEarmarkProposal {
            parser_source_id: "test-nlp".to_owned(),
            model_version: "1.0".to_owned(),
            raw_source_text: "BUY BOND_10Y 10000000".to_owned(),
            anchors: vec![pb::SourceTextAnchor {
                field_name: "instrument_code".to_owned(),
                byte_start: 4,
                byte_end: 12,
                span_text: "BOND_10Y".to_owned(),
            }],
            confidence_score: Some(dec("0.95", dim)),
            account_id: "acct-A".to_owned(),
            instrument_code: "BOND_10Y".to_owned(),
            side: pb::TradeSide::Buy as i32,
            settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
            notional: Some(dec("10000000", cur)),
            quote_terms: Some(pb::QuoteTerms {
                price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                    "100.0", px,
                ))),
                settlement_amount: Some(dec("10000000", cur)),
            }),
            suggested_target_state: pb::DraftTargetState::Committed as i32,
        }),
    };

    let head_before_d = srv.store.head_seq().unwrap();
    // Half-present header -> VALIDATION_FAILED
    let d_half = client
        .submit_draft_earmark(with_bearer_and_fence(
            make_draft_req("f08-d-bad"),
            PARSER_TOKEN,
            Some(&actual_epoch_str),
            None,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        d_half.result,
        Some(pb::submit_draft_earmark_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed && !e.retryable
    ));
    // Stale epoch -> non-retryable CONCURRENCY_CONFLICT, zero drafts / receipts / events / head_seq
    let d_conflict = client
        .submit_draft_earmark(with_bearer_and_fence(
            make_draft_req("f08-d-conflict"),
            PARSER_TOKEN,
            Some(&wrong_epoch_str),
            Some(&actual_id.ledger_instance_id),
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        d_conflict.result,
        Some(pb::submit_draft_earmark_response::Result::Error(ref e))
        if extract_err_code(Some(e)) == core_pb::ErrorCode::ConcurrencyConflict && !e.retryable
    ));
    assert_eq!(srv.store.head_seq().unwrap(), head_before_d);
    assert!(
        srv.store
            .with_read_barrier(|s, _, _| {
                s.accounts_meta
                    .get("acct-A")
                    .is_none_or(|m| m.drafts.is_empty())
                    && s.domain
                        .accounts
                        .get("acct-A")
                        .is_none_or(|a| a.drafts.is_empty())
            })
            .unwrap()
    );

    // Matching fence with same idempotency_key succeeds with replayed=false
    let d_ok = client
        .submit_draft_earmark(with_bearer_and_fence(
            make_draft_req("f08-d-conflict"),
            PARSER_TOKEN,
            Some(&actual_epoch_str),
            Some(&actual_id.ledger_instance_id),
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        d_ok.result,
        Some(pb::submit_draft_earmark_response::Result::Value(ref v)) if !v.replayed && v.draft.is_some()
    ));
}

#[tokio::test]
async fn bp4_f09_shutdown_watch_terminates_slow_subscriber_with_full_queue() {
    let policy = Arc::new(TestPolicyProvider::new(default_test_policy(false)));
    let clock = Arc::new(ManualBlotterClock::new(shanghai_time(
        2026, 6, 15, 10, 0, 0,
    )));
    let verifier = Arc::new(build_test_verifier());
    let opts = SqliteBlotterOpenOptions::ephemeral_in_memory(policy.blotter_config());
    let store = Arc::new(SqliteBlotterStore::open(&opts).unwrap());
    let adapter = Arc::new(TestStoreAdapter::new(store.clone(), 16));
    let engines = OwnedBlotterEngines {
        bond_engine: Arc::new(NativeBondAnalyticsEngine),
        hedge_engine: Arc::new(NativeFuturesHedgeEngine),
        delivery_rule_parser: Arc::new(CgbFuturesDeliveryRulePackParser),
        regulatory_rule_parser: Arc::new(RegulatoryRulePackV1Parser),
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let (drain_tx, mut drain_rx) = tokio::sync::mpsc::channel::<()>(1);
    let svc_cfg = DeskBlotterServiceConfig {
        subscribe_stream_channel_capacity: 1,
        ..DeskBlotterServiceConfig::default()
    };
    let svc = DeskBlotterGrpcService::new(adapter, policy, verifier, clock, engines, svc_cfg)
        .unwrap()
        .with_shutdown_signal(shutdown_rx)
        .with_stream_drain_guard(drain_tx);

    // Open subscription with capacity=1 and intentionally DO NOT read from stream so mpsc fills.
    let mut stream = svc
        .subscribe_events(with_bearer(
            pb::SubscribeEventsRequest {
                account_id: None,
                after_seq: 0,
            },
            TRADER_AB_TOKEN,
        ))
        .await
        .unwrap()
        .into_inner();

    // Give spawned producer a moment to push initial heartbeat into capacity=1 channel.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Signal shutdown while client has NOT drained the queued heartbeat.
    let _ = shutdown_tx.send(true);
    drop(svc);

    // Response stream immediately ends cleanly with None (not hanging!) and drain_rx closes!
    let next_item = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("stream must terminate promptly on shutdown");
    assert!(next_item.is_none());
    let drained = tokio::time::timeout(Duration::from_secs(2), drain_rx.recv())
        .await
        .expect("producer task must exit promptly on shutdown");
    assert!(drained.is_none());
}
