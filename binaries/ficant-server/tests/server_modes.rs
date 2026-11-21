#![allow(
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::missing_panics_doc
)]

use std::collections::BTreeMap;
use std::fs;
use std::net::{SocketAddr, TcpStream as StdTcpStream};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{NaiveDate, TimeZone};
use chrono_tz::Asia::Shanghai;
use ficant_api::{
    AUTHORIZATION_HEADER, CredentialEntry, DeskBlotterServiceConfig, StaticCredentialVerifier,
    encode_digest_hex,
};
use ficant_application::ports::{ManualBlotterClock, PrincipalRole};
use ficant_blotter_store::{
    BLOTTER_SCHEMA_VERSION, DEFAULT_EVENT_RETENTION_WINDOW, default_reference_analysis_units,
    parse_fixed_decimal,
};
use ficant_cgb_futures_pack::{
    MARKET as FUTURES_MARKET, RULE_TYPE as FUTURES_RULE_TYPE, TYPE_URL as FUTURES_TYPE_URL,
};
use ficant_contracts::ficant::blotter::v1 as pb;
use ficant_contracts::ficant::blotter::v1::desk_blotter_service_client::DeskBlotterServiceClient;
use ficant_contracts::ficant::blotter::v1::intranet_nlp_intent_parser_service_client::IntranetNlpIntentParserServiceClient;
use ficant_contracts::ficant::blotter::v1::market_quote_stream_service_client::MarketQuoteStreamServiceClient;
use ficant_contracts::ficant::blotter::v1::position_base_provider_service_client::PositionBaseProviderServiceClient;
use ficant_contracts::ficant::blotter::v1::trade_feed_reconciler_service_client::TradeFeedReconcilerServiceClient;
use ficant_contracts::ficant::core::v1 as core_pb;
use ficant_contracts::ficant::market::v1 as market_pb;
use ficant_contracts::ficant::rates::v1 as rates_pb;
use ficant_contracts::ficant::rates::v1::rates_analytics_service_client::RatesAnalyticsServiceClient;
use ficant_domain::analytics::MARKET_TIMEZONE;
use ficant_domain::market::{
    MarketRulePack, MarketRulePackInput, RulePackContent, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version,
};
use ficant_regulatory_pack::{
    MARKET as REG_MARKET, RULE_TYPE as REG_RULE_TYPE, TYPE_URL as REG_TYPE_URL,
};
use ficant_server::{
    BlotterStorageMode, DEFAULT_EVENT_BUS_CAPACITY, DeskBlotterPolicyRuntime, FicantServerConfig,
    SpiBackends, spawn_server,
};
use prost::Message;
use rust_decimal::Decimal;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tonic::Request;
use tonic::metadata::MetadataValue;

const TRADER_TOK: &str = "srv-trader-secret-token-001";
const OPERATOR_TOK: &str = "srv-operator-secret-token-002";
const FEED_TOK: &str = "srv-feed-secret-token-003";
const PARSER_TOK: &str = "srv-parser-secret-token-004";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn ephemeral_addr() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn with_bearer<T>(msg: T, token: &str) -> Request<T> {
    let mut req = Request::new(msg);
    req.metadata_mut().insert(
        AUTHORIZATION_HEADER,
        MetadataValue::try_from(format!("Bearer {token}")).unwrap(),
    );
    req
}

fn server_verifier() -> Arc<StaticCredentialVerifier> {
    Arc::new(
        StaticCredentialVerifier::new(vec![
            CredentialEntry::from_plaintext_token(
                "trader-1",
                PrincipalRole::Trader,
                TRADER_TOK,
                ["acct-1".to_owned()],
                [
                    "FEED-1".to_owned(),
                    "SRC-A".to_owned(),
                    "SRC-B".to_owned(),
                    "primary-quote-feed".to_owned(),
                ],
            ),
            CredentialEntry::from_plaintext_token(
                "operator-1",
                PrincipalRole::Operator,
                OPERATOR_TOK,
                ["acct-1".to_owned()],
                [
                    "FEED-1".to_owned(),
                    "SRC-A".to_owned(),
                    "SRC-B".to_owned(),
                    "primary-quote-feed".to_owned(),
                ],
            ),
            CredentialEntry::from_plaintext_token(
                "feed-1",
                PrincipalRole::FeedAdapter,
                FEED_TOK,
                ["acct-1".to_owned()],
                [
                    "FEED-1".to_owned(),
                    "SRC-A".to_owned(),
                    "SRC-B".to_owned(),
                    "primary-quote-feed".to_owned(),
                ],
            ),
            CredentialEntry::from_plaintext_token(
                "parser-1",
                PrincipalRole::NlpParser,
                PARSER_TOK,
                ["acct-1".to_owned()],
                Vec::<String>::new(),
            ),
        ])
        .unwrap(),
    )
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

fn ulid(suffix: &str) -> core_pb::Ulid {
    core_pb::Ulid {
        value: format!("01ARZ3NDEKTSV4RRFFQ69G5F{suffix}"),
    }
}

fn pb_units() -> rates_pb::AnalysisUnits {
    let u = default_reference_analysis_units().unwrap();
    let mut pb_u = ficant_api::blotter_codec::to_pb_analysis_units(&u);
    pb_u.years = Some(core_pb::UnitRef {
        unit_id: Some(ulid("Y1")),
        version: 1,
    });
    pb_u.years_squared = Some(core_pb::UnitRef {
        unit_id: Some(ulid("Y2")),
        version: 1,
    });
    pb_u
}

fn dec(val: &str, unit: &core_pb::UnitRef) -> core_pb::DecimalValue {
    let d = Decimal::from_str(val).unwrap().normalize();
    core_pb::DecimalValue {
        coefficient: d.mantissa().to_string(),
        scale: d.scale(),
        unit: Some(unit.clone()),
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

fn verified_reg_pack() -> MarketRulePack {
    let payload = market_pb::RegulatoryRulePack {
        constraints: vec![market_pb::ConstraintDefinition {
            limit_code: "MAX_INST_FACE".to_owned(),
            measure: market_pb::ConstraintMeasure::InstrumentFace as i32,
        }],
    };
    let raw = payload.encode_to_vec();
    let hash = ContentHash::digest(&raw);
    let owner_id = Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F01").unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5F02").unwrap(),
            version: Version::new(1).unwrap(),
            owner: OwnerRef::new(owner_id.clone(), owner_id),
            market: REG_MARKET.to_owned(),
            rule_type: REG_RULE_TYPE.to_owned(),
            source: "desk-policy".to_owned(),
            effective: EffectivePeriod::new(
                shanghai_time(2026, 1, 1, 0, 0, 0),
                shanghai_time(2028, 1, 1, 0, 0, 0),
            )
            .unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: hash,
        },
        RulePackContent::new(REG_TYPE_URL.to_owned(), raw).unwrap(),
    )
    .unwrap()
}

fn make_opening_base(start_after_seq: u64) -> pb::OpeningPositionBase {
    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();
    let rate = u.rate.as_ref().unwrap();
    pb::OpeningPositionBase {
        account_id: "acct-1".to_owned(),
        base_id: Some(ulid("A1")),
        trading_date: "2026-06-15".to_owned(),
        next_settlement_date: "2026-06-16".to_owned(),
        effective_from: Some(pb_time(2026, 6, 15, 9, 0, 0)),
        watermark_time: Some(pb_time(2026, 6, 15, 9, 0, 0)),
        watermark_source_seq: Some(start_after_seq),
        source_id: "FEED-1".to_owned(),
        opening_cash_t0: Some(dec("100000000", cur)),
        opening_net_cash_delta_t1: Some(dec("10000000", cur)),
        balances: vec![pb::OpeningInstrumentBalance {
            instrument_code: "BOND_10Y".to_owned(),
            opening_economic_notional: Some(dec("100000000", cur)),
            opening_available_notional: Some(dec("100000000", cur)),
            opening_net_t0_notional: Some(dec("0", cur)),
            opening_net_t1_notional: Some(dec("0", cur)),
        }],
        instruments: vec![pb::InstrumentCatalogEntry {
            instrument_code: "BOND_10Y".to_owned(),
            bond: Some(market_pb::Bond {
                instrument: Some(core_pb::VersionRef {
                    id: Some(ulid("B1")),
                    version: 1,
                }),
                first_issue_date: "2025-01-15".to_owned(),
                current_issue_date: "2025-01-15".to_owned(),
                maturity_date: "2030-01-15".to_owned(),
                face_value: Some(dec("100", cur)),
                cumulative_issued_amount: Some(dec("50000000000", cur)),
                tax_attributes: Some(market_pb::BondTaxAttributes {
                    value_added_tax_status: market_pb::ValueAddedTaxStatus::Exempt as i32,
                    income_tax_status: market_pb::IncomeTaxStatus::Exempt as i32,
                }),
                coupon_rate: Some(dec("0.025", rate)),
                coupon_frequency: market_pb::BondCouponFrequency::Semiannual as i32,
                day_count: market_pb::BondDayCountConvention::ActActBondIsma as i32,
                business_day: market_pb::BondBusinessDayConvention::Following as i32,
            }),
            eligibility_class: pb::RateCoverageEligibility::SovereignOrPolicyFixed as i32,
            calendar_requirement: rates_pb::CalendarRequirement::Unspecified as i32,
        }],
        calendar: Some(market_pb::Calendar {
            calendar_id: Some(ulid("C1")),
            version: 1,
            owner: None,
            market: "INTERBANK".to_owned(),
            market_timezone: MARKET_TIMEZONE.to_owned(),
            effective_from: Some(pb_time(2025, 1, 1, 0, 0, 0)),
            effective_to: Some(pb_time(2032, 1, 1, 0, 0, 0)),
            sessions: vec![
                market_pb::CalendarSession {
                    local_date: "2025-01-01".to_owned(),
                    open_local_time: "09:00:00".to_owned(),
                    close_local_time: "17:00:00".to_owned(),
                    closed: false,
                },
                market_pb::CalendarSession {
                    local_date: "2031-12-31".to_owned(),
                    open_local_time: "09:00:00".to_owned(),
                    close_local_time: "17:00:00".to_owned(),
                    closed: false,
                },
            ],
        }),
        default_calendar_requirement: rates_pb::CalendarRequirement::ReferenceReplay as i32,
        units: Some(u),
        baseline_evidence: Vec::new(),
        source_start_after_feed_seq: start_after_seq,
    }
}

fn err_code(detail: Option<&core_pb::ErrorDetail>) -> core_pb::ErrorCode {
    core_pb::ErrorCode::try_from(detail.expect("error detail").code).unwrap()
}

async fn grpc_web_unary<Req: Message, Resp: Message + Default>(
    addr: SocketAddr,
    path: &str,
    req: &Req,
    bearer_token: &str,
) -> Resp {
    let payload = req.encode_to_vec();
    let mut framed = Vec::with_capacity(5 + payload.len());
    framed.push(0x00);
    framed.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
    framed.extend_from_slice(&payload);

    let headers = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/grpc-web+proto\r\nAccept: application/grpc-web+proto\r\nX-Grpc-Web: 1\r\nauthorization: Bearer {bearer_token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        framed.len()
    );

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&framed).await.unwrap();
    stream.flush().await.unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let sep = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let hdr_str = String::from_utf8_lossy(&raw[..sep]);
    assert!(hdr_str.starts_with("HTTP/1.1 200"));
    let body_raw = &raw[sep + 4..];
    let body = if hdr_str
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        let mut out = Vec::new();
        let mut slice = body_raw;
        while let Some(pos) = slice.windows(2).position(|w| w == b"\r\n") {
            let sz = usize::from_str_radix(String::from_utf8_lossy(&slice[..pos]).trim(), 16)
                .unwrap_or(0);
            slice = &slice[pos + 2..];
            if sz == 0 {
                break;
            }
            out.extend_from_slice(&slice[..sz]);
            slice = &slice[sz + 2..];
        }
        out
    } else {
        body_raw.to_vec()
    };

    assert_eq!(body[0], 0x00);
    let len = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize;
    Resp::decode(&body[5..5 + len]).unwrap()
}

// ============================================================================
// TEST 1: Mode 1 — Safe Default Disabled Mode (BP-1 Compatibility, Zero Files)
// ============================================================================

#[tokio::test]
async fn mode_1_disabled_default_serves_rates_and_returns_storage_unavailable_for_blotter() {
    let empty_dir = TempDir::new().unwrap();
    let cfg = FicantServerConfig::disabled_default(ephemeral_addr()).unwrap();
    let handle = spawn_server(cfg).await.unwrap();
    let endpoint = format!("http://{}", handle.addr());

    // 1. Health check probe succeeds
    StdTcpStream::connect_timeout(&handle.addr(), Duration::from_secs(2))
        .expect("TCP health check succeeds");

    // 2. RatesAnalyticsService (BP-1) works normally
    let mut rates_client = RatesAnalyticsServiceClient::connect(endpoint.clone())
        .await
        .unwrap();
    let u = pb_units();
    let hedge = rates_client
        .analyze_futures_hedge(rates_pb::AnalyzeFuturesHedgeRequest {
            units: Some(u.clone()),
            product_code: "T".to_owned(),
            target_dv01: Some(dec("1000", u.dv01.as_ref().unwrap())),
            ctd_dv01_per_100: Some(dec("0.075", u.dv01_per_100.as_ref().unwrap())),
            conversion_factor: Some(dec("0.985", u.dimensionless.as_ref().unwrap())),
            delivery_rule_pack: Some(futures_v2_pack()),
            valuation_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        hedge.result,
        Some(rates_pb::analyze_futures_hedge_response::Result::Analysis(
            _
        ))
    ));

    // 3. DeskBlotterService & all 4 SPIs return STORAGE_UNAVAILABLE
    let mut blotter = DeskBlotterServiceClient::connect(endpoint.clone())
        .await
        .unwrap();
    let snap = blotter
        .get_account_snapshot(pb::GetAccountSnapshotRequest {
            account_id: "acct-1".to_owned(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        snap.result,
        Some(pb::get_account_snapshot_response::Result::Error(ref e))
        if err_code(Some(e)) == core_pb::ErrorCode::StorageUnavailable
    ));

    let mut pos_spi = PositionBaseProviderServiceClient::connect(endpoint.clone())
        .await
        .unwrap();
    let pos_res = pos_spi
        .fetch_opening_position_base(pb::FetchOpeningPositionBaseRequest {
            request_id: "r1".to_owned(),
            account_id: "acct-1".to_owned(),
            trading_date: "2026-06-15".to_owned(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        pos_res.result,
        Some(pb::fetch_opening_position_base_response::Result::Error(ref e))
        if err_code(Some(e)) == core_pb::ErrorCode::StorageUnavailable
    ));

    let mut feed_spi = TradeFeedReconcilerServiceClient::connect(endpoint.clone())
        .await
        .unwrap();
    let feed_res = feed_spi
        .pull_trade_feed_batch(pb::PullTradeFeedBatchRequest {
            source_id: "FEED-1".to_owned(),
            after_feed_seq: 0,
            max_records: 10,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        feed_res.result,
        Some(pb::pull_trade_feed_batch_response::Result::Error(ref e))
        if err_code(Some(e)) == core_pb::ErrorCode::StorageUnavailable
    ));

    let mut quote_spi = MarketQuoteStreamServiceClient::connect(endpoint.clone())
        .await
        .unwrap();
    let q_res = quote_spi
        .pull_market_quotes(pb::PullMarketQuotesRequest {
            cursor: Some(pb::QuoteCursor {
                source_id: "SRC-A".to_owned(),
                instrument_code: "BOND_10Y".to_owned(),
                after_quote_seq: 0,
            }),
            max_ticks: 10,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        q_res.result,
        Some(pb::pull_market_quotes_response::Result::Error(ref e))
        if err_code(Some(e)) == core_pb::ErrorCode::StorageUnavailable
    ));

    let mut nlp_spi = IntranetNlpIntentParserServiceClient::connect(endpoint)
        .await
        .unwrap();
    let nlp_res = nlp_spi
        .parse_nlp_intent(pb::ParseNlpIntentRequest {
            request_id: "n1".to_owned(),
            account_id: "acct-1".to_owned(),
            raw_source_text: "BUY".to_owned(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        nlp_res.result,
        Some(pb::parse_nlp_intent_response::Result::Error(ref e))
        if err_code(Some(e)) == core_pb::ErrorCode::StorageUnavailable
    ));

    // 4. Zero files created in empty_dir
    assert_eq!(fs::read_dir(empty_dir.path()).unwrap().count(), 0);
    handle.shutdown().await.unwrap();
}

// ============================================================================
// TEST 2: Mode 2 — WAL Anti-Spoofing, PRAGMAs, Exclusive Lock & Bad Configs
// ============================================================================

#[tokio::test]
async fn mode_2_persistent_wal_anti_spoofing_pragmas_exclusive_lock_and_bad_configs() {
    let dir = TempDir::new().unwrap();

    // 1. Rejects :memory: and all file: URIs + empty path + directory path
    for bad_path in [
        ":memory:",
        "file::memory:",
        "file::memory:?cache=shared",
        "file:blotter.db?mode=memory",
        "file:///tmp/blotter.db",
        "",
        dir.path().to_str().unwrap(),
    ] {
        let cfg = FicantServerConfig {
            listen_addr: ephemeral_addr(),
            storage_mode: BlotterStorageMode::PersistentWal {
                db_path: PathBuf::from(bad_path),
            },
            policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
            credentials: Some(server_verifier()),
            clock: Arc::new(ManualBlotterClock::new(shanghai_time(
                2026, 6, 15, 10, 0, 0,
            ))),
            service_config: DeskBlotterServiceConfig::default(),
            event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
            event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
            watchdog_interval: None,
            spi_backends: SpiBackends::default(),
        };
        assert!(
            spawn_server(cfg).await.is_err(),
            "expected bad path '{bad_path}' to be rejected"
        );
    }

    // 2. Rejects missing credentials when storage is enabled
    let db_path = dir.path().join("desk-blotter.sqlite");
    let no_cred_cfg = FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::PersistentWal {
            db_path: db_path.clone(),
        },
        policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
        credentials: None,
        clock: Arc::new(ManualBlotterClock::new(shanghai_time(
            2026, 6, 15, 10, 0, 0,
        ))),
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    };
    assert!(spawn_server(no_cred_cfg).await.is_err());

    // 3. Rejects invalid policy JSON & duplicate/revoked/expired credentials JSON
    assert!(DeskBlotterPolicyRuntime::from_json_str(r#"{"unknown_field": 1}"#, None).is_err());
    let dup_digest = encode_digest_hex(&ficant_api::sha256_token_digest("same-secret"));
    let dup_cred_json = format!(
        r#"{{"schema_version":1,"credentials":[
            {{"actor_id":"a1","role":"TRADER","token_sha256_hex":"{dup_digest}"}},
            {{"actor_id":"a2","role":"TRADER","token_sha256_hex":"{dup_digest}"}}
        ]}}"#
    );
    assert!(StaticCredentialVerifier::from_json_str(&dup_cred_json).is_err());

    let rev_digest = encode_digest_hex(&ficant_api::sha256_token_digest("pre-revoked-secret"));
    let revoked_cred_json = format!(
        r#"{{"schema_version":1,"credentials":[
            {{"actor_id":"rev-1","role":"TRADER","token_sha256_hex":"{rev_digest}","revoked":true}}
        ]}}"#
    );
    assert!(StaticCredentialVerifier::from_json_str(&revoked_cred_json).is_err());

    let startup_now = shanghai_time(2026, 6, 15, 10, 0, 0);
    let exp_digest = encode_digest_hex(&ficant_api::sha256_token_digest("expired-secret"));
    let expired_cred_json = format!(
        r#"{{"schema_version":1,"credentials":[
            {{"actor_id":"exp-1","role":"TRADER","token_sha256_hex":"{exp_digest}","expires_at_rfc3339":"2026-06-15T02:00:00Z"}}
        ]}}"#
    );
    assert!(StaticCredentialVerifier::from_json_str_at(&expired_cred_json, &startup_now).is_err());

    // spawn_server rejects already-expired (exact boundary & past) and pre-revoked verifiers in
    // BOTH PersistentWal (without creating WAL file!) and EphemeralInMemory modes.
    let expired_verifier = Arc::new(
        StaticCredentialVerifier::new(vec![
            CredentialEntry::from_plaintext_token(
                "exp-trader",
                PrincipalRole::Trader,
                "exp-tok",
                ["acct-1".to_owned()],
                ["FEED-1".to_owned()],
            )
            .with_expires_at(startup_now.instant()),
        ])
        .unwrap(),
    );
    let bad_cred_db = dir.path().join("must-not-be-created.sqlite");
    assert!(
        spawn_server(FicantServerConfig {
            listen_addr: ephemeral_addr(),
            storage_mode: BlotterStorageMode::PersistentWal {
                db_path: bad_cred_db.clone(),
            },
            policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
            credentials: Some(expired_verifier.clone()),
            clock: Arc::new(ManualBlotterClock::new(startup_now.clone())),
            service_config: DeskBlotterServiceConfig::default(),
            event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
            event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
            watchdog_interval: None,
            spi_backends: SpiBackends::default(),
        })
        .await
        .is_err()
    );
    assert!(!bad_cred_db.exists());
    assert!(
        spawn_server(FicantServerConfig {
            listen_addr: ephemeral_addr(),
            storage_mode: BlotterStorageMode::EphemeralInMemory,
            policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
            credentials: Some(expired_verifier),
            clock: Arc::new(ManualBlotterClock::new(startup_now.clone())),
            service_config: DeskBlotterServiceConfig::default(),
            event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
            event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
            watchdog_interval: None,
            spi_backends: SpiBackends::default(),
        })
        .await
        .is_err()
    );

    let revoked_before_start = server_verifier();
    revoked_before_start.revoke_actor("trader-1");
    assert!(
        spawn_server(FicantServerConfig {
            listen_addr: ephemeral_addr(),
            storage_mode: BlotterStorageMode::EphemeralInMemory,
            policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
            credentials: Some(revoked_before_start),
            clock: Arc::new(ManualBlotterClock::new(startup_now)),
            service_config: DeskBlotterServiceConfig::default(),
            event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
            event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
            watchdog_interval: None,
            spi_backends: SpiBackends::default(),
        })
        .await
        .is_err()
    );

    // 4. Real WAL opens cleanly, satisfies all PRAGMAs, and blocks second concurrent server!
    let valid_cfg = FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::PersistentWal {
            db_path: db_path.clone(),
        },
        policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
        credentials: Some(server_verifier()),
        clock: Arc::new(ManualBlotterClock::new(shanghai_time(
            2026, 6, 15, 10, 0, 0,
        ))),
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    };
    let handle_1 = spawn_server(valid_cfg).await.unwrap();
    let pragmas = handle_1
        .runtime()
        .unwrap()
        .store()
        .inspect_pragmas()
        .unwrap();
    assert_eq!(pragmas.journal_mode.to_ascii_lowercase(), "wal");
    assert_eq!(pragmas.locking_mode.to_ascii_lowercase(), "exclusive");
    assert_eq!(pragmas.synchronous, 2);
    assert_eq!(pragmas.foreign_keys, 1);
    assert_eq!(pragmas.business_schema_version, BLOTTER_SCHEMA_VERSION);

    // Second server on same WAL path fails immediately due to exclusive lock
    let second_cfg = FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::PersistentWal { db_path },
        policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
        credentials: Some(server_verifier()),
        clock: Arc::new(ManualBlotterClock::new(shanghai_time(
            2026, 6, 15, 10, 0, 0,
        ))),
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    };
    assert!(spawn_server(second_cfg).await.is_err());

    handle_1.shutdown().await.unwrap();
}

// ============================================================================
// TEST 3: Mode 3 — Explicit Ephemeral In-Memory + Policy A vs Policy B
// ============================================================================

#[tokio::test]
async fn mode_3_ephemeral_and_policy_a_vs_policy_b_runtime_override() {
    let temp = TempDir::new().unwrap();

    // 1. Embedded Policy A (["T", "TF", "TL"] -> rejects "TS")
    let policy_a = DeskBlotterPolicyRuntime::from_embedded_default().unwrap();
    assert_eq!(policy_a.policy_profile, "A");
    assert_eq!(
        policy_a.default_account_policy.allowed_hedge_product_codes,
        vec!["T".to_owned(), "TF".to_owned(), "TL".to_owned()]
    );

    let srv_a = spawn_server(FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::EphemeralInMemory,
        policy: policy_a,
        credentials: Some(server_verifier()),
        clock: Arc::new(ManualBlotterClock::new(shanghai_time(
            2026, 6, 15, 10, 0, 0,
        ))),
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    })
    .await
    .unwrap();

    let mut client_a = DeskBlotterServiceClient::connect(format!("http://{}", srv_a.addr()))
        .await
        .unwrap();
    let _ = client_a
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "imp-pa".to_owned(),
                opening_base: Some(make_opening_base(0)),
                expected_account_version: 0,
            },
            OPERATOR_TOK,
        ))
        .await
        .unwrap();
    let u = pb_units();
    let _ = client_a
        .ingest_market_quotes(with_bearer(
            pb::IngestMarketQuotesRequest {
                idempotency_key: "q-pa".to_owned(),
                account_id: "acct-1".to_owned(),
                ticks: vec![pb::MarketQuoteTick {
                    source_id: "primary-quote-feed".to_owned(),
                    quote_seq: 1,
                    instrument_code: "BOND_10Y".to_owned(),
                    source_type: market_pb::PriceSourceType::ActiveQuote as i32,
                    bid_clean_price: None,
                    ask_clean_price: None,
                    mid_clean_price: None,
                    mid_yield_to_maturity: Some(dec("0.025", u.rate.as_ref().unwrap())),
                    observed_at: Some(pb_time(2026, 6, 15, 9, 59, 0)),
                    received_at: Some(pb_time(2026, 6, 15, 9, 59, 5)),
                }],
                source_id: "primary-quote-feed".to_owned(),
            },
            TRADER_TOK,
        ))
        .await
        .unwrap();

    // Policy A allows "T" and rejects "TS"
    let t_ok = client_a
        .preview_covered_hedge(with_bearer(
            pb::PreviewCoveredHedgeRequest {
                account_id: "acct-1".to_owned(),
                scope: pb::PreviewHedgeScope::WholeAccount as i32,
                product_code: "T".to_owned(),
                contract_code: "T2609".to_owned(),
                ctd_dv01_per_100: Some(dec("0.075", u.dv01_per_100.as_ref().unwrap())),
                conversion_factor: Some(dec("0.985", u.dimensionless.as_ref().unwrap())),
                delivery_rule_pack: Some(futures_v2_pack()),
                valuation_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
                expected_snapshot_seq: None,
            },
            TRADER_TOK,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        t_ok.result,
        Some(pb::preview_covered_hedge_response::Result::Value(_))
    ));

    let ts_rejected = client_a
        .preview_covered_hedge(with_bearer(
            pb::PreviewCoveredHedgeRequest {
                account_id: "acct-1".to_owned(),
                scope: pb::PreviewHedgeScope::WholeAccount as i32,
                product_code: "TS".to_owned(),
                contract_code: "TS2609".to_owned(),
                ctd_dv01_per_100: Some(dec("0.0145", u.dv01_per_100.as_ref().unwrap())),
                conversion_factor: Some(dec("0.9987", u.dimensionless.as_ref().unwrap())),
                delivery_rule_pack: Some(futures_v2_pack()),
                valuation_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
                expected_snapshot_seq: None,
            },
            TRADER_TOK,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        ts_rejected.result,
        Some(pb::preview_covered_hedge_response::Result::Error(ref e))
        if err_code(Some(e)) == core_pb::ErrorCode::ValidationFailed
            && e.field_violations.iter().any(|fv| fv.field == "product_code")
    ));
    srv_a.shutdown().await.unwrap();

    // 2. Runtime Policy B override from external JSON file (["TS", "T", "TF", "TL"] -> accepts "TS" with 2M notional!)
    let policy_b_path = temp.path().join("policy-b.json");
    fs::write(
        &policy_b_path,
        r#"{
  "schema_version": 1,
  "policy_version": 1,
  "policy_profile": "B",
  "allowed_hedge_product_codes": ["TS", "T", "TF", "TL"],
  "blotter_config": {
    "clean_price_tolerance": "0.05",
    "yield_tolerance": "0.0005",
    "committed_overdue_seconds": 3600,
    "pre_close_alert_time": "16:30:00",
    "feed_staleness_seconds": 300
  },
  "default_account_policy": {
    "default_quote_source_id": "primary-quote-feed",
    "instrument_quote_sources": {},
    "quote_staleness_seconds": 300,
    "default_draft_hold_ttl_seconds": 1800,
    "limit_ceilings": {},
    "regulatory_rule_pack": null
  },
  "accounts": {}
}"#,
    )
    .unwrap();
    let policy_b = DeskBlotterPolicyRuntime::from_file(&policy_b_path).unwrap();

    let srv_b = spawn_server(FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::EphemeralInMemory,
        policy: policy_b,
        credentials: Some(server_verifier()),
        clock: Arc::new(ManualBlotterClock::new(shanghai_time(
            2026, 6, 15, 10, 0, 0,
        ))),
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    })
    .await
    .unwrap();

    let mut client_b = DeskBlotterServiceClient::connect(format!("http://{}", srv_b.addr()))
        .await
        .unwrap();
    let _ = client_b
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "imp-pb".to_owned(),
                opening_base: Some(make_opening_base(0)),
                expected_account_version: 0,
            },
            OPERATOR_TOK,
        ))
        .await
        .unwrap();
    let _ = client_b
        .ingest_market_quotes(with_bearer(
            pb::IngestMarketQuotesRequest {
                idempotency_key: "q-pb".to_owned(),
                account_id: "acct-1".to_owned(),
                ticks: vec![pb::MarketQuoteTick {
                    source_id: "primary-quote-feed".to_owned(),
                    quote_seq: 1,
                    instrument_code: "BOND_10Y".to_owned(),
                    source_type: market_pb::PriceSourceType::ActiveQuote as i32,
                    bid_clean_price: None,
                    ask_clean_price: None,
                    mid_clean_price: None,
                    mid_yield_to_maturity: Some(dec("0.025", u.rate.as_ref().unwrap())),
                    observed_at: Some(pb_time(2026, 6, 15, 9, 59, 0)),
                    received_at: Some(pb_time(2026, 6, 15, 9, 59, 5)),
                }],
                source_id: "primary-quote-feed".to_owned(),
            },
            TRADER_TOK,
        ))
        .await
        .unwrap();

    let ts_ok = match client_b
        .preview_covered_hedge(with_bearer(
            pb::PreviewCoveredHedgeRequest {
                account_id: "acct-1".to_owned(),
                scope: pb::PreviewHedgeScope::WholeAccount as i32,
                product_code: "TS".to_owned(),
                contract_code: "TS2609".to_owned(),
                ctd_dv01_per_100: Some(dec("0.0145", u.dv01_per_100.as_ref().unwrap())),
                conversion_factor: Some(dec("0.9987", u.dimensionless.as_ref().unwrap())),
                delivery_rule_pack: Some(futures_v2_pack()),
                valuation_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
                expected_snapshot_seq: None,
            },
            TRADER_TOK,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::preview_covered_hedge_response::Result::Value(v)) => v,
        other => panic!("expected TS hedge ok under Policy B, got {other:?}"),
    };
    let cn = ts_ok.measures.unwrap().contract_notional.unwrap();
    let cn_val = Decimal::from_i128_with_scale(cn.coefficient.parse().unwrap(), cn.scale);
    assert_eq!(cn_val, Decimal::from(2_000_000));
    srv_b.shutdown().await.unwrap();
}

// ============================================================================
// TEST 4: Startup Quote-Source Reconciliation Across WAL Restart + Draft Invalidation
// ============================================================================

#[tokio::test]
async fn startup_quote_source_reconciliation_commits_on_restart_and_invalidates_draft() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("reconcile.sqlite");
    let clock = Arc::new(ManualBlotterClock::new(shanghai_time(
        2026, 6, 15, 10, 0, 0,
    )));

    let mut ceilings = BTreeMap::new();
    ceilings.insert(
        "MAX_INST_FACE".to_owned(),
        parse_fixed_decimal("500000000").unwrap(),
    );

    let mut policy_1 = DeskBlotterPolicyRuntime::from_embedded_default().unwrap();
    policy_1.default_account_policy.default_quote_source_id = "SRC-A".to_owned();
    policy_1.default_account_policy.regulatory_rule_pack = Some(verified_reg_pack());
    policy_1.default_account_policy.limit_ceilings = ceilings.clone();

    let srv_1 = spawn_server(FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::PersistentWal {
            db_path: db_path.clone(),
        },
        policy: policy_1,
        credentials: Some(server_verifier()),
        clock: clock.clone(),
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    })
    .await
    .unwrap();

    let mut c1 = DeskBlotterServiceClient::connect(format!("http://{}", srv_1.addr()))
        .await
        .unwrap();
    let u = pb_units();
    let _ = c1
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "imp-src".to_owned(),
                opening_base: Some(make_opening_base(0)),
                expected_account_version: 0,
            },
            OPERATOR_TOK,
        ))
        .await
        .unwrap();

    // Ingest SRC-A seq=100 and SRC-B seq=1 (distinct source sequence spaces coexist!)
    for (k, src, seq, ytm) in [
        ("qa", "SRC-A", 100_u64, "0.025"),
        ("qb", "SRC-B", 1_u64, "0.026"),
    ] {
        let _ = c1
            .ingest_market_quotes(with_bearer(
                pb::IngestMarketQuotesRequest {
                    idempotency_key: k.to_owned(),
                    account_id: "acct-1".to_owned(),
                    ticks: vec![pb::MarketQuoteTick {
                        source_id: src.to_owned(),
                        quote_seq: seq,
                        instrument_code: "BOND_10Y".to_owned(),
                        source_type: market_pb::PriceSourceType::ActiveQuote as i32,
                        bid_clean_price: None,
                        ask_clean_price: None,
                        mid_clean_price: None,
                        mid_yield_to_maturity: Some(dec(ytm, u.rate.as_ref().unwrap())),
                        observed_at: Some(pb_time(2026, 6, 15, 9, 59, 0)),
                        received_at: Some(pb_time(2026, 6, 15, 9, 59, 5)),
                    }],
                    source_id: src.to_owned(),
                },
                TRADER_TOK,
            ))
            .await
            .unwrap();
    }

    // Submit draft bound to SRC-A seq=100
    let raw_text = "BUY T+0 INST=BOND_10Y NOTIONAL=5000000 CLEAN=100.0 TARGET=COMMITTED";
    let draft = match c1
        .submit_draft_earmark(with_bearer(
            pb::SubmitDraftEarmarkRequest {
                idempotency_key: "draft-src-a".to_owned(),
                account_id: "acct-1".to_owned(),
                proposal: Some(pb::NlpEarmarkProposal {
                    parser_source_id: "parser-1".to_owned(),
                    model_version: "1.0".to_owned(),
                    raw_source_text: raw_text.to_owned(),
                    anchors: vec![pb::SourceTextAnchor {
                        field_name: "side".to_owned(),
                        byte_start: 0,
                        byte_end: 3,
                        span_text: "BUY".to_owned(),
                    }],
                    confidence_score: Some(dec("0.95", u.dimensionless.as_ref().unwrap())),
                    account_id: "acct-1".to_owned(),
                    instrument_code: "BOND_10Y".to_owned(),
                    side: pb::TradeSide::Buy as i32,
                    settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                    notional: Some(dec("5000000", u.currency_amount.as_ref().unwrap())),
                    quote_terms: Some(pb::QuoteTerms {
                        price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                            "100.0",
                            u.price_per_100.as_ref().unwrap(),
                        ))),
                        settlement_amount: Some(dec(
                            "5000000",
                            u.currency_amount.as_ref().unwrap(),
                        )),
                    }),
                    suggested_target_state: pb::DraftTargetState::Committed as i32,
                }),
            },
            PARSER_TOK,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::submit_draft_earmark_response::Result::Value(v)) => v.draft.unwrap(),
        other => panic!("unexpected draft: {other:?}"),
    };
    assert_eq!(draft.preview_quote_source_id, "SRC-A");
    assert_eq!(draft.preview_quote_seq, 100);
    let seq_before_restart = srv_1.runtime().unwrap().store().inspect_pragmas().unwrap();
    assert_eq!(
        seq_before_restart.business_schema_version,
        BLOTTER_SCHEMA_VERSION
    );
    srv_1.shutdown().await.unwrap();

    // Restart server #2 on SAME WAL with policy switched to SRC-B!
    let mut policy_2 = DeskBlotterPolicyRuntime::from_embedded_default().unwrap();
    policy_2.default_account_policy.default_quote_source_id = "SRC-B".to_owned();
    policy_2.default_account_policy.regulatory_rule_pack = Some(verified_reg_pack());
    policy_2.default_account_policy.limit_ceilings = ceilings;

    let srv_2 = spawn_server(FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::PersistentWal { db_path },
        policy: policy_2,
        credentials: Some(server_verifier()),
        clock,
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    })
    .await
    .unwrap();

    let mut c2 = DeskBlotterServiceClient::connect(format!("http://{}", srv_2.addr()))
        .await
        .unwrap();
    let snap = match c2
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-1".to_owned(),
            },
            TRADER_TOK,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::get_account_snapshot_response::Result::Value(v)) => v.snapshot.unwrap(),
        other => panic!("unexpected snap: {other:?}"),
    };
    assert_eq!(snap.positions[0].active_quote_source_id, "SRC-B");
    assert_eq!(snap.positions[0].active_quote_seq, Some(1));

    // Old draft token bound to SRC-A MUST be invalidated -> CONCURRENCY_CONFLICT!
    let stale_confirm = c2
        .confirm_draft_earmark(with_bearer(
            pb::ConfirmDraftEarmarkRequest {
                idempotency_key: "confirm-stale".to_owned(),
                account_id: "acct-1".to_owned(),
                draft_id: draft.draft_id,
                expected_draft_revision: draft.draft_revision,
                expected_position_version: snap.positions[0].position_version,
                confirmation_token: draft.confirmation_token,
                acknowledge_uncovered_dv01: false,
            },
            TRADER_TOK,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        stale_confirm.result,
        Some(pb::confirm_draft_earmark_response::Result::Error(ref e))
        if err_code(Some(e)) == core_pb::ErrorCode::ConcurrencyConflict
    ));

    srv_2.shutdown().await.unwrap();
}

// ============================================================================
// TEST 5: Deterministic ManualBlotterClock + WatchdogController (Zero Sleep!)
// ============================================================================

#[tokio::test]
async fn deterministic_watchdog_expires_holds_alerts_committed_and_idempotent_sweep() {
    let clock = Arc::new(ManualBlotterClock::new(shanghai_time(
        2026, 6, 15, 10, 0, 0,
    )));
    let srv = spawn_server(FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::EphemeralInMemory,
        policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
        credentials: Some(server_verifier()),
        clock: clock.clone(),
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None, // 100% deterministic trigger mode!
        spi_backends: SpiBackends::default(),
    })
    .await
    .unwrap();

    let mut client = DeskBlotterServiceClient::connect(format!("http://{}", srv.addr()))
        .await
        .unwrap();
    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();
    let px = u.price_per_100.as_ref().unwrap();

    let _ = client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "wd-imp".to_owned(),
                opening_base: Some(make_opening_base(0)),
                expected_account_version: 0,
            },
            OPERATOR_TOK,
        ))
        .await
        .unwrap();

    // Send a feed heartbeat at 10:00:00 so feed staleness clock starts
    let _ = client
        .ingest_trade_feed(with_bearer(
            pb::IngestTradeFeedRequest {
                idempotency_key: "wd-hb".to_owned(),
                source_id: "FEED-1".to_owned(),
                fills: Vec::new(),
                heartbeat_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
            },
            FEED_TOK,
        ))
        .await
        .unwrap();

    // Create 1 HOLD (TTL 1800s -> expires at 10:30:00) and 1 COMMITTED (overdue at +3600s -> 11:00:00)
    let _ = client
        .create_hold(with_bearer(
            pb::CreateHoldRequest {
                idempotency_key: "wd-hold".to_owned(),
                account_id: "acct-1".to_owned(),
                expected_position_version: 1,
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Sell as i32,
                settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                notional: Some(dec("10000000", cur)),
                quote_terms: None,
                ttl_seconds: 1800,
                note: "wd-hold".to_owned(),
            },
            TRADER_TOK,
        ))
        .await
        .unwrap();

    let _ = client
        .create_committed(with_bearer(
            pb::CreateCommittedRequest {
                idempotency_key: "wd-comm".to_owned(),
                account_id: "acct-1".to_owned(),
                expected_position_version: 2,
                instrument_code: "BOND_10Y".to_owned(),
                side: pb::TradeSide::Sell as i32,
                settlement_bucket: pb::SettlementBucket::TPlus0 as i32,
                notional: Some(dec("15000000", cur)),
                quote_terms: Some(pb::QuoteTerms {
                    price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                        "100.0", px,
                    ))),
                    settlement_amount: Some(dec("15000000", cur)),
                }),
                note: "wd-comm".to_owned(),
            },
            TRADER_TOK,
        ))
        .await
        .unwrap();

    let wd = srv.watchdog().unwrap().clone();

    // Advance +100s (still within 300s feed staleness and 1800s TTL) -> zero transitions, head_seq unchanged!
    clock.advance_seconds(100).unwrap();
    let s1 = wd.trigger_sweep_and_wait().await.unwrap();
    assert!(!s1.mutated);
    assert_eq!(s1.expired_hold_count, 0);
    assert_eq!(s1.newly_stale_feed_count, 0);
    let seq_before_expiry = s1.head_seq;

    // Advance +1700s (total +1800s -> now == expires_at AND feed stale >= 300s!)
    clock.advance_seconds(1700).unwrap();
    let s2 = wd.trigger_sweep_and_wait().await.unwrap();
    assert!(s2.mutated);
    assert_eq!(s2.expired_hold_count, 1);
    assert_eq!(s2.newly_stale_feed_count, 1);
    assert_eq!(s2.head_seq, seq_before_expiry + 1);

    // Repeat sweep immediately -> ZERO transitions, head_seq unchanged!
    let s3 = wd.trigger_sweep_and_wait().await.unwrap();
    assert!(!s3.mutated);
    assert_eq!(s3.head_seq, s2.head_seq);

    // Advance +1800s (total +3600s -> COMMITTED becomes overdue_alert=true, NEVER released!)
    clock.advance_seconds(1800).unwrap();
    let s4 = wd.trigger_sweep_and_wait().await.unwrap();
    assert!(s4.mutated);
    assert_eq!(s4.newly_alerted_committed_count, 1);
    assert_eq!(s4.head_seq, s3.head_seq + 1);

    // Verify COMMITTED is still active with overdue_alert=true, while HOLD is gone from active_earmarks
    let snap = match client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-1".to_owned(),
            },
            OPERATOR_TOK,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::get_account_snapshot_response::Result::Value(v)) => v.snapshot.unwrap(),
        other => panic!("unexpected: {other:?}"),
    };
    assert_eq!(snap.active_earmarks.len(), 1);
    assert_eq!(
        snap.active_earmarks[0].state,
        pb::EarmarkState::Committed as i32
    );
    assert!(snap.active_earmarks[0].overdue_alert);
    assert!(snap.feed_statuses[0].paused_by_staleness);

    // Repeat sweep again -> ZERO head_seq increment!
    let s5 = wd.trigger_sweep_and_wait().await.unwrap();
    assert!(!s5.mutated);
    assert_eq!(s5.head_seq, s4.head_seq);

    srv.shutdown().await.unwrap();
}

// ============================================================================
// TEST 6: WAL Restart Matrix (Import-Once, Idempotency Replay, Gap 10->12->11,
//         HTTP/1.1 gRPC-Web Recovery)
// ============================================================================

#[tokio::test]
async fn wal_restart_import_once_idempotency_gap_10_12_11_and_grpc_web() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("restart-matrix.sqlite");
    let clock = Arc::new(ManualBlotterClock::new(shanghai_time(
        2026, 6, 15, 10, 0, 0,
    )));
    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();
    let px = u.price_per_100.as_ref().unwrap();

    // Server #1: import base starting after seq 9, test re-import rejection, send seq 10 & seq 12 (gap [11,11])
    let srv_1 = spawn_server(FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::PersistentWal {
            db_path: db_path.clone(),
        },
        policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
        credentials: Some(server_verifier()),
        clock: clock.clone(),
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    })
    .await
    .unwrap();

    let mut c1 = DeskBlotterServiceClient::connect(format!("http://{}", srv_1.addr()))
        .await
        .unwrap();
    let imp_req = pb::ImportOpeningBaseRequest {
        idempotency_key: "imp-once-1".to_owned(),
        opening_base: Some(make_opening_base(9)),
        expected_account_version: 0,
    };
    let imp_1 = match c1
        .import_opening_base(with_bearer(imp_req.clone(), OPERATOR_TOK))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::import_opening_base_response::Result::Value(v)) => v,
        other => panic!("unexpected import: {other:?}"),
    };
    assert!(!imp_1.replayed);

    // Re-import with a NEW key -> IMMUTABLE_VIOLATION!
    let mut reimport_req = imp_req.clone();
    reimport_req.idempotency_key = "imp-once-new-key".to_owned();
    let reimport_err = c1
        .import_opening_base(with_bearer(reimport_req, OPERATOR_TOK))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        reimport_err.result,
        Some(pb::import_opening_base_response::Result::Error(ref e))
        if err_code(Some(e)) == core_pb::ErrorCode::ImmutableViolation
    ));

    // Ingest feed_seq=10, then feed_seq=12 (missing 11 -> gap [11,11]!)
    let make_fill = |seq: u64, trade_id: &str| pb::TradeFillRecord {
        source_id: "FEED-1".to_owned(),
        external_trade_id: trade_id.to_owned(),
        source_revision: 1,
        feed_seq: seq,
        action: pb::TradeFeedRecordAction::NewFill as i32,
        account_id: "acct-1".to_owned(),
        instrument_code: "BOND_10Y".to_owned(),
        side: pb::TradeSide::Buy as i32,
        settlement_date: "2026-06-15".to_owned(),
        notional: Some(dec("10000000", cur)),
        quote_terms: Some(pb::QuoteTerms {
            price_or_yield: Some(pb::quote_terms::PriceOrYield::CleanPricePer100(dec(
                "100.0", px,
            ))),
            settlement_amount: Some(dec("10000000", cur)),
        }),
        executed_at: Some(pb_time(2026, 6, 15, 10, 1, 0)),
        batch_first_feed_seq: seq,
        batch_last_feed_seq: seq,
    };

    let _ = c1
        .ingest_trade_feed(with_bearer(
            pb::IngestTradeFeedRequest {
                idempotency_key: "f-10".to_owned(),
                source_id: "FEED-1".to_owned(),
                fills: vec![make_fill(10, "T-10")],
                heartbeat_at: Some(pb_time(2026, 6, 15, 10, 1, 0)),
            },
            FEED_TOK,
        ))
        .await
        .unwrap();

    let gap_res = match c1
        .ingest_trade_feed(with_bearer(
            pb::IngestTradeFeedRequest {
                idempotency_key: "f-12".to_owned(),
                source_id: "FEED-1".to_owned(),
                fills: vec![make_fill(12, "T-12")],
                heartbeat_at: Some(pb_time(2026, 6, 15, 10, 2, 0)),
            },
            FEED_TOK,
        ))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::ingest_trade_feed_response::Result::Value(v)) => v,
        other => panic!("unexpected gap resp: {other:?}"),
    };
    assert_eq!(
        gap_res.item_outcomes[0].kind,
        pb::TradeFillOutcomeKind::BufferedGap as i32
    );
    assert_eq!(
        gap_res.feed_status.as_ref().unwrap().contiguous_feed_seq,
        10
    );
    assert!(gap_res.feed_status.as_ref().unwrap().paused_by_gap);

    // Shut down Server #1 and restart Server #2 on SAME WAL
    srv_1.shutdown().await.unwrap();

    let srv_2 = spawn_server(FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::PersistentWal { db_path },
        policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
        credentials: Some(server_verifier()),
        clock,
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    })
    .await
    .unwrap();

    let mut c2 = DeskBlotterServiceClient::connect(format!("http://{}", srv_2.addr()))
        .await
        .unwrap();

    // Replay original ImportOpeningBase idempotency key after WAL restart -> replayed=true!
    let imp_replay = match c2
        .import_opening_base(with_bearer(imp_req, OPERATOR_TOK))
        .await
        .unwrap()
        .into_inner()
        .result
    {
        Some(pb::import_opening_base_response::Result::Value(v)) => v,
        other => panic!("expected replay ok, got {other:?}"),
    };
    assert!(imp_replay.replayed);
    assert_eq!(imp_replay.committed_seq, imp_1.committed_seq);

    // Deliver missing seq 11 over raw HTTP/1.1 gRPC-Web -> heals gap [11,11], advances contiguous to 12!
    let heal_web: pb::IngestTradeFeedResponse = grpc_web_unary(
        srv_2.addr(),
        "/ficant.blotter.v1.DeskBlotterService/IngestTradeFeed",
        &pb::IngestTradeFeedRequest {
            idempotency_key: "f-11".to_owned(),
            source_id: "FEED-1".to_owned(),
            fills: vec![make_fill(11, "T-11")],
            heartbeat_at: Some(pb_time(2026, 6, 15, 10, 3, 0)),
        },
        FEED_TOK,
    )
    .await;
    match heal_web.result {
        Some(pb::ingest_trade_feed_response::Result::Value(v)) => {
            let fs = v.feed_status.unwrap();
            assert_eq!(fs.contiguous_feed_seq, 12);
            assert!(!fs.paused_by_gap);
            assert!(fs.missing_feed_ranges.is_empty());
        }
        other => panic!("expected gap heal ok, got {other:?}"),
    }

    srv_2.shutdown().await.unwrap();
}

// ============================================================================
// TEST 7: M3-F3 Regression — Future-Expiring Credential Starts Cleanly & Expires at Runtime
// ============================================================================

#[tokio::test]
async fn startup_allows_future_expiring_credential_and_enforces_runtime_expiry() {
    let start_time = shanghai_time(2026, 6, 15, 10, 0, 0);
    let expiry_time = shanghai_time(2026, 6, 15, 10, 5, 0);
    let clock = Arc::new(ManualBlotterClock::new(start_time));

    let verifier = Arc::new(
        StaticCredentialVerifier::new(vec![
            CredentialEntry::from_plaintext_token(
                "short-lived-op",
                PrincipalRole::Operator,
                OPERATOR_TOK,
                ["acct-1".to_owned()],
                ["FEED-1".to_owned()],
            )
            .with_expires_at(expiry_time.instant()),
        ])
        .unwrap(),
    );

    let srv = spawn_server(FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::EphemeralInMemory,
        policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
        credentials: Some(verifier),
        clock: clock.clone(),
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    })
    .await
    .unwrap();

    let mut client = DeskBlotterServiceClient::connect(format!("http://{}", srv.addr()))
        .await
        .unwrap();

    // Before expiry -> succeeds
    let imp = client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "imp-exp-ok".to_owned(),
                opening_base: Some(make_opening_base(0)),
                expected_account_version: 0,
            },
            OPERATOR_TOK,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        imp.result,
        Some(pb::import_opening_base_response::Result::Value(_))
    ));

    // Advance clock to exact expiry (+300s) -> UNAUTHENTICATED
    clock.advance_seconds(300).unwrap();
    let expired_snap = client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-1".to_owned(),
            },
            OPERATOR_TOK,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        expired_snap.result,
        Some(pb::get_account_snapshot_response::Result::Error(ref e))
        if err_code(Some(e)) == core_pb::ErrorCode::Unauthenticated
    ));

    srv.shutdown().await.unwrap();
}
