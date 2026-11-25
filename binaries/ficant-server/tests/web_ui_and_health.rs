#![allow(
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::missing_panics_doc
)]

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{NaiveDate, TimeZone};
use chrono_tz::Asia::Shanghai;
use ficant_api::{
    AUTHORIZATION_HEADER, CredentialEntry, DeskBlotterServiceConfig, StaticCredentialVerifier,
    encode_digest_hex, sha256_token_digest,
};
use ficant_application::ports::{ManualBlotterClock, PrincipalRole};
use ficant_blotter_store::{
    DEFAULT_EVENT_RETENTION_WINDOW, SqliteBlotterStore, SqliteRolloverRequest, StoreFaultInjection,
    default_reference_analysis_units, is_canonical_ulid, sqlite_rollover,
};
use ficant_cgb_futures_pack::{
    MARKET as FUTURES_MARKET, RULE_TYPE as FUTURES_RULE_TYPE, TYPE_URL as FUTURES_TYPE_URL,
};
use ficant_contracts::ficant::blotter::v1 as pb;
use ficant_contracts::ficant::blotter::v1::desk_blotter_service_client::DeskBlotterServiceClient;
use ficant_contracts::ficant::core::v1 as core_pb;
use ficant_contracts::ficant::market::v1 as market_pb;
use ficant_contracts::ficant::rates::v1 as rates_pb;
use ficant_contracts::ficant::rates::v1::rates_analytics_service_client::RatesAnalyticsServiceClient;
use ficant_domain::analytics::MARKET_TIMEZONE;
use ficant_domain::primitives::{ContentHash, MarketTime};
use ficant_server::{
    AssetLoadHookStage, BlotterStorageMode, DEFAULT_EVENT_BUS_CAPACITY, DeskBlotterPolicyRuntime,
    FicantServerConfig, MAX_WEB_ASSETS_FILE_COUNT, PINNED_RULEPACK_ASSET_PATH,
    PINNED_RULEPACK_PAYLOAD_BYTES, PINNED_RULEPACK_PAYLOAD_SHA256, SpiBackends, WebUiConfig,
    WebUiMode, load_web_asset_catalog_from_dir, load_web_asset_catalog_from_dir_with_hook,
    map_health_check_addr, parse_web_ui_env_values, probe_healthz, resolve_web_ui_with_embedded,
    sanitize_static_request_path, spawn_server, spawn_server_with_web_ui,
};
use prost::Message;
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_stream::StreamExt;
use tonic::Request;
use tonic::metadata::MetadataValue;

const TRADER_TOK: &str = "bp4-trader-secret-token-001";
const OPERATOR_TOK: &str = "bp4-operator-secret-token-002";
const FEED_TOK: &str = "bp4-feed-secret-token-003";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn pinned_rulepack_bytes() -> Vec<u8> {
    let bytes = fs::read(repo_root().join("domain-packs/cgb-futures/cgb-futures-v2.bin")).unwrap();
    assert_eq!(bytes.len(), PINNED_RULEPACK_PAYLOAD_BYTES);
    bytes
}

fn create_valid_dist_dir(root: &Path, index_html: &str) {
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::create_dir_all(root.join("rulepacks")).unwrap();
    fs::write(root.join("index.html"), index_html.as_bytes()).unwrap();
    fs::write(root.join("assets/index-ABC123.js"), b"console.log('ok');").unwrap();
    fs::write(root.join("assets/index-ABC123.css"), b"body{margin:0}").unwrap();
    fs::write(
        root.join("rulepacks/cgb-futures-v2.bin"),
        pinned_rulepack_bytes(),
    )
    .unwrap();
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for &b in &digest {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0f)] as char);
    }
    out
}

fn ephemeral_addr() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
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

fn with_bearer<T>(msg: T, token: &str) -> Request<T> {
    let mut req = Request::new(msg);
    req.metadata_mut().insert(
        AUTHORIZATION_HEADER,
        MetadataValue::try_from(format!("Bearer {token}")).unwrap(),
    );
    req
}

fn test_verifier() -> Arc<StaticCredentialVerifier> {
    Arc::new(
        StaticCredentialVerifier::new(vec![
            CredentialEntry::from_plaintext_token(
                "trader-1",
                PrincipalRole::Trader,
                TRADER_TOK,
                ["acct-1".to_owned()],
                ["FEED-1".to_owned(), "primary-quote-feed".to_owned()],
            ),
            CredentialEntry::from_plaintext_token(
                "operator-1",
                PrincipalRole::Operator,
                OPERATOR_TOK,
                ["acct-1".to_owned()],
                ["FEED-1".to_owned(), "primary-quote-feed".to_owned()],
            ),
            CredentialEntry::from_plaintext_token(
                "feed-1",
                PrincipalRole::FeedAdapter,
                FEED_TOK,
                ["acct-1".to_owned()],
                ["FEED-1".to_owned(), "primary-quote-feed".to_owned()],
            ),
        ])
        .unwrap(),
    )
}

fn make_opening_base() -> pb::OpeningPositionBase {
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
        watermark_source_seq: Some(0),
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
        source_start_after_feed_seq: 0,
    }
}

struct RawHttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl RawHttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn assert_no_cors_headers(&self) {
        for (k, _) in &self.headers {
            assert!(
                !k.to_ascii_lowercase().starts_with("access-control-allow-"),
                "unexpected CORS header leaked: {k}"
            );
        }
    }
}

async fn raw_http_request(
    addr: SocketAddr,
    method: &str,
    target: &str,
    extra_headers: &[(&str, &str)],
    body: &[u8],
) -> RawHttpResponse {
    let mut req = format!("{method} {target} HTTP/1.1\r\nHost: {addr}\r\n");
    for (k, v) in extra_headers {
        let _ = write!(req, "{k}: {v}\r\n");
    }
    if !body.is_empty() {
        let _ = write!(req, "Content-Length: {}\r\n", body.len());
    }
    req.push_str("Connection: close\r\n\r\n");

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(req.as_bytes()).await.unwrap();
    if !body.is_empty() {
        stream.write_all(body).await.unwrap();
    }
    stream.flush().await.unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("valid HTTP response");
    let head_str = String::from_utf8_lossy(&raw[..sep]);
    let mut lines = head_str.lines();
    let status_line = lines.next().unwrap();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let body_raw = &raw[sep + 4..];
    let is_chunked = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked")
    });
    let parsed_body = if is_chunked {
        let mut out = Vec::new();
        let mut slice = body_raw;
        while let Some(pos) = slice.windows(2).position(|w| w == b"\r\n") {
            let sz_str = String::from_utf8_lossy(&slice[..pos]);
            let sz =
                usize::from_str_radix(sz_str.split(';').next().unwrap().trim(), 16).unwrap_or(0);
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

    RawHttpResponse {
        status,
        headers,
        body: parsed_body,
    }
}

fn create_dir_link_for_test(link_path: &Path, target_dir: &Path) -> bool {
    #[cfg(windows)]
    {
        // Windows directory junction via `mklink /J` works without administrator privileges!
        Command::new("cmd")
            .args([
                "/c",
                "mklink",
                "/J",
                link_path.to_str().unwrap(),
                target_dir.to_str().unwrap(),
            ])
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target_dir, link_path).is_ok()
    }
}

fn remove_dir_link_for_test(link_path: &Path) {
    #[cfg(windows)]
    {
        let _ = fs::remove_dir(link_path);
    }
    #[cfg(unix)]
    {
        let _ = fs::remove_file(link_path);
    }
}

// ============================================================================
// TEST 1: F-10 Asset Priority, Validation, Limits & Mid-Load TOCTOU Swap
// ============================================================================

#[test]
fn f10_web_ui_priority_validation_limits_and_toctou_swap_detection() {
    let tmp = TempDir::new().unwrap();
    let valid_dist = tmp.path().join("valid-dist");
    create_valid_dist_dir(&valid_dist, "<html><body>desk-v1</body></html>");

    let rp_bytes = pinned_rulepack_bytes();
    let embedded_sample: &[(&str, &[u8])] = &[
        ("/index.html", b"<html>embedded</html>"),
        (PINNED_RULEPACK_ASSET_PATH, rp_bytes.as_slice()),
    ];

    // 1. Env parsing fail-closed checks
    assert!(parse_web_ui_env_values(Some("yes"), None).is_err());
    assert!(parse_web_ui_env_values(Some(" true "), None).is_err());
    assert!(parse_web_ui_env_values(None, Some("   ")).is_err());
    assert!(parse_web_ui_env_values(Some("True"), None).is_ok());
    assert!(parse_web_ui_env_values(Some("FALSE"), None).is_ok());

    // 2. Four-level priority:
    // FICANT_SERVE_WEB_UI=false/0 forces disabled even when static_dir & embedded exist
    let cfg_force_off =
        parse_web_ui_env_values(Some("false"), Some(valid_dist.to_str().unwrap())).unwrap();
    let (mode, cat) = resolve_web_ui_with_embedded(&cfg_force_off, embedded_sample).unwrap();
    assert_eq!(mode, WebUiMode::Disabled);
    assert!(cat.is_none());

    let cfg_force_off_0 =
        parse_web_ui_env_values(Some("0"), Some(valid_dist.to_str().unwrap())).unwrap();
    let (mode, _) = resolve_web_ui_with_embedded(&cfg_force_off_0, embedded_sample).unwrap();
    assert_eq!(mode, WebUiMode::Disabled);

    // Non-empty FICANT_STATIC_WEB_DIR beats embedded
    let cfg_static = parse_web_ui_env_values(Some(""), Some(valid_dist.to_str().unwrap())).unwrap();
    let (mode, cat) = resolve_web_ui_with_embedded(&cfg_static, embedded_sample).unwrap();
    assert_eq!(mode, WebUiMode::StaticDir);
    assert_eq!(
        cat.unwrap().get("/index.html").unwrap().bytes.as_ref(),
        b"<html><body>desk-v1</body></html>"
    );

    // Embedded beats disabled
    let cfg_auto = parse_web_ui_env_values(None, None).unwrap();
    let (mode, cat) = resolve_web_ui_with_embedded(&cfg_auto, embedded_sample).unwrap();
    assert_eq!(mode, WebUiMode::Embedded);
    assert_eq!(
        cat.unwrap().get("/index.html").unwrap().bytes.as_ref(),
        b"<html>embedded</html>"
    );

    // Auto with no static & no embedded -> Disabled
    let (mode, cat) = resolve_web_ui_with_embedded(&cfg_auto, &[]).unwrap();
    assert_eq!(mode, WebUiMode::Disabled);
    assert!(cat.is_none());

    // Explicit FICANT_SERVE_WEB_UI=true/1 with no static & no embedded -> startup error!
    for true_val in ["true", "TRUE", "1"] {
        let cfg_req = parse_web_ui_env_values(Some(true_val), None).unwrap();
        assert!(resolve_web_ui_with_embedded(&cfg_req, &[]).is_err());
    }

    // 3. Missing / empty index.html & missing / 1-byte-tampered RulePack rejected
    let no_index = tmp.path().join("no-index");
    create_valid_dist_dir(&no_index, "<html>ok</html>");
    fs::remove_file(no_index.join("index.html")).unwrap();
    assert!(load_web_asset_catalog_from_dir(&no_index).is_err());

    let empty_index = tmp.path().join("empty-index");
    create_valid_dist_dir(&empty_index, "");
    assert!(load_web_asset_catalog_from_dir(&empty_index).is_err());

    let tampered_rp = tmp.path().join("tampered-rp");
    create_valid_dist_dir(&tampered_rp, "<html>ok</html>");
    let mut bad_bytes = pinned_rulepack_bytes();
    bad_bytes[0] ^= 0x01;
    fs::write(tampered_rp.join("rulepacks/cgb-futures-v2.bin"), &bad_bytes).unwrap();
    assert!(load_web_asset_catalog_from_dir(&tampered_rp).is_err());

    let missing_rp = tmp.path().join("missing-rp");
    create_valid_dist_dir(&missing_rp, "<html>ok</html>");
    fs::remove_file(missing_rp.join("rulepacks/cgb-futures-v2.bin")).unwrap();
    assert!(load_web_asset_catalog_from_dir(&missing_rp).is_err());

    // 4. Exceeding MAX_WEB_ASSETS_FILE_COUNT (2048) rejected
    let too_many_dir = tmp.path().join("too-many");
    create_valid_dist_dir(&too_many_dir, "<html>ok</html>");
    let extra_sub = too_many_dir.join("extra");
    fs::create_dir_all(&extra_sub).unwrap();
    for i in 0..=MAX_WEB_ASSETS_FILE_COUNT {
        fs::write(extra_sub.join(format!("f{i:04}.txt")), b"x").unwrap();
    }
    assert!(load_web_asset_catalog_from_dir(&too_many_dir).is_err());

    // 5. Symlink / Windows Junction as root OR inside tree rejected
    let link_root = tmp.path().join("junction-root");
    assert!(
        create_dir_link_for_test(&link_root, &valid_dist),
        "test requires non-admin directory junction / symlink creation"
    );
    assert!(load_web_asset_catalog_from_dir(&link_root).is_err());
    remove_dir_link_for_test(&link_root);

    let with_inner_link = tmp.path().join("inner-link-dist");
    create_valid_dist_dir(&with_inner_link, "<html>ok</html>");
    let target_outside = tmp.path().join("outside");
    fs::create_dir_all(&target_outside).unwrap();
    fs::write(target_outside.join("secret.txt"), b"secret").unwrap();
    let inner_link = with_inner_link.join("linked-sub");
    assert!(create_dir_link_for_test(&inner_link, &target_outside));
    assert!(load_web_asset_catalog_from_dir(&with_inner_link).is_err());
    remove_dir_link_for_test(&inner_link);

    // 6. Mid-load TOCTOU swap #1: directory swapped to another dir between pre-open check and open
    let swap_target_1 = tmp.path().join("swap1-active");
    let swap_alt_1 = tmp.path().join("swap1-evil");
    let swap_backup_1 = tmp.path().join("swap1-backup");
    create_valid_dist_dir(&swap_target_1, "<html>ORIGINAL-1</html>");
    create_valid_dist_dir(&swap_alt_1, "<html>SWAPPED--1</html>"); // exact same byte len!
    let mut swapped_1 = false;
    let res_1 = load_web_asset_catalog_from_dir_with_hook(&swap_target_1, |stage, rel_url| {
        if stage == AssetLoadHookStage::BeforeOpen && rel_url == "/index.html" && !swapped_1 {
            swapped_1 = true;
            fs::rename(&swap_target_1, &swap_backup_1).map_err(|e| e.to_string())?;
            fs::rename(&swap_alt_1, &swap_target_1).map_err(|e| e.to_string())?;
        }
        Ok(())
    });
    assert!(swapped_1);
    assert!(
        res_1.is_err(),
        "directory swap between pre-open check and open MUST abort batch load"
    );

    // 7. Mid-load TOCTOU swap #2: directory replaced with Junction between open/read and post-read verify
    let swap_target_2 = tmp.path().join("swap2-active");
    let swap_backup_2 = tmp.path().join("swap2-backup");
    create_valid_dist_dir(&swap_target_2, "<html>ORIGINAL-2</html>");
    let mut swapped_2 = false;
    let res_2 = load_web_asset_catalog_from_dir_with_hook(&swap_target_2, |stage, rel_url| {
        if stage == AssetLoadHookStage::AfterOpenBeforeVerify
            && rel_url == "/index.html"
            && !swapped_2
        {
            swapped_2 = true;
            fs::rename(&swap_target_2, &swap_backup_2).map_err(|e| e.to_string())?;
            assert!(create_dir_link_for_test(&swap_target_2, &swap_backup_2));
        }
        Ok(())
    });
    assert!(swapped_2);
    assert!(
        res_2.is_err(),
        "Junction swap between open and post-read verify MUST abort batch load"
    );
    remove_dir_link_for_test(&swap_target_2);
}

// ============================================================================
// TEST 2: Static Routing, Path Sanitization, Post-Load Immutability & Zero CORS
// ============================================================================

#[tokio::test]
async fn f10_f15_static_routing_path_sanitization_immutability_and_zero_cors() {
    // Unit check path sanitizer
    assert_eq!(sanitize_static_request_path("/").unwrap(), "/");
    assert_eq!(
        sanitize_static_request_path("/assets/app.js").unwrap(),
        "/assets/app.js"
    );
    for bad in [
        "no-leading-slash",
        "/%zz",
        "/%2",
        "/%",
        "/.",
        "/..",
        "/a/../b",
        "/a/./b",
        "/%2e%2e/etc/passwd",
        "/a%2fb",
        "/a%2Fb",
        "/a%5cb",
        "/a%5Cb",
        "/a\\b",
        "/a%00b",
        "/a%0ab",
        "/a%1fb",
        "//",
        "//index.html",
        "/assets//x.js",
    ] {
        assert!(
            sanitize_static_request_path(bad).is_err(),
            "expected bad path '{bad}' to be rejected"
        );
    }

    let tmp = TempDir::new().unwrap();
    let dist_dir = tmp.path().join("desk-dist");
    let orig_html = "<!doctype html><html><body>IMMUTABLE-RAM-DESK-UI</body></html>";
    create_valid_dist_dir(&dist_dir, orig_html);

    let cfg = FicantServerConfig::disabled_default(ephemeral_addr()).unwrap();
    let handle = spawn_server_with_web_ui(cfg, WebUiConfig::StaticDir(dist_dir.clone()))
        .await
        .unwrap();
    assert_eq!(handle.web_ui_mode(), WebUiMode::StaticDir);
    let addr = handle.addr();

    // Post-load disk tampering MUST NOT affect served RAM responses!
    fs::write(dist_dir.join("index.html"), b"MUTATED-ON-DISK").unwrap();
    fs::remove_file(dist_dir.join("rulepacks/cgb-futures-v2.bin")).unwrap();

    // 1. GET / serves original RAM index.html with nosniff & no-cache & zero CORS
    let root_get = raw_http_request(addr, "GET", "/", &[], &[]).await;
    assert_eq!(root_get.status, 200);
    assert_eq!(root_get.body, orig_html.as_bytes());
    assert_eq!(
        root_get.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(root_get.header("cache-control"), Some("no-cache"));
    assert_eq!(root_get.header("x-content-type-options"), Some("nosniff"));
    root_get.assert_no_cors_headers();

    // 2. HEAD / has empty body and identical entity headers
    let root_head = raw_http_request(addr, "HEAD", "/", &[], &[]).await;
    assert_eq!(root_head.status, 200);
    assert!(root_head.body.is_empty());
    assert_eq!(
        root_head.header("content-length"),
        Some(orig_html.len().to_string().as_str())
    );
    assert_eq!(
        root_head.header("content-type"),
        root_get.header("content-type")
    );
    assert_eq!(
        root_head.header("cache-control"),
        root_get.header("cache-control")
    );
    assert_eq!(root_head.header("x-content-type-options"), Some("nosniff"));
    root_head.assert_no_cors_headers();

    // 3. Hashed /assets/* has immutable cache-control & accurate MIME
    let js_get = raw_http_request(addr, "GET", "/assets/index-ABC123.js", &[], &[]).await;
    assert_eq!(js_get.status, 200);
    assert_eq!(js_get.body, b"console.log('ok');");
    assert_eq!(
        js_get.header("content-type"),
        Some("application/javascript; charset=utf-8")
    );
    assert_eq!(
        js_get.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(js_get.header("x-content-type-options"), Some("nosniff"));
    js_get.assert_no_cors_headers();

    // 4. RulePack served intact from RAM despite disk deletion
    let rp_get = raw_http_request(addr, "GET", PINNED_RULEPACK_ASSET_PATH, &[], &[]).await;
    assert_eq!(rp_get.status, 200);
    assert_eq!(rp_get.body.len(), PINNED_RULEPACK_PAYLOAD_BYTES);
    assert_eq!(sha256_hex(&rp_get.body), PINNED_RULEPACK_PAYLOAD_SHA256);
    rp_get.assert_no_cors_headers();

    // 5. Missing /assets/*, /rulepacks/*, or paths with file extension -> 404
    for missing in [
        "/assets/does-not-exist.js",
        "/rulepacks/other.bin",
        "/favicon.ico",
        "/unknown/file.json",
    ] {
        let resp = raw_http_request(addr, "GET", missing, &[], &[]).await;
        assert_eq!(resp.status, 404, "expected 404 for {missing}");
        resp.assert_no_cors_headers();
    }

    // 6. Valid extensionless SPA route falls back to index.html
    let spa_resp = raw_http_request(addr, "GET", "/blotter/acct-1/positions", &[], &[]).await;
    assert_eq!(spa_resp.status, 200);
    assert_eq!(spa_resp.body, orig_html.as_bytes());
    spa_resp.assert_no_cors_headers();

    // 7. Malformed paths -> 400 Bad Request, zero host path leakage, zero CORS
    let host_dir_str = tmp.path().to_str().unwrap();
    for bad in [
        "/%zz",
        "/a/../b",
        "/a%2fb",
        "/a%5cb",
        "/a%00b",
        "//index.html",
    ] {
        let resp = raw_http_request(addr, "GET", bad, &[], &[]).await;
        assert_eq!(resp.status, 400, "expected 400 for {bad}");
        resp.assert_no_cors_headers();
        assert!(!String::from_utf8_lossy(&resp.body).contains(host_dir_str));
    }

    // 8. Non-GET/HEAD on static paths -> 405 Method Not Allowed + Allow: GET, HEAD
    for method in ["POST", "PUT", "DELETE", "OPTIONS"] {
        let resp = raw_http_request(addr, method, "/", &[], &[]).await;
        assert_eq!(resp.status, 405, "expected 405 for {method} /");
        assert_eq!(resp.header("allow"), Some("GET, HEAD"));
        resp.assert_no_cors_headers();
    }

    handle.shutdown().await.unwrap();

    // 9. Disabled UI mode: valid static paths return 404 while malformed paths still return 400
    let disabled_handle =
        spawn_server(FicantServerConfig::disabled_default(ephemeral_addr()).unwrap())
            .await
            .unwrap();
    assert_eq!(disabled_handle.web_ui_mode(), WebUiMode::Disabled);
    let r_404 = raw_http_request(disabled_handle.addr(), "GET", "/", &[], &[]).await;
    assert_eq!(r_404.status, 404);
    r_404.assert_no_cors_headers();
    let r_400 = raw_http_request(disabled_handle.addr(), "GET", "/a%2fb", &[], &[]).await;
    assert_eq!(r_400.status, 400);
    r_400.assert_no_cors_headers();
    disabled_handle.shutdown().await.unwrap();
}

// ============================================================================
// TEST 3: F-15 Single-Port Unknown-RPC 3-Protocol Matrix & Info-Leak Sentinel
// ============================================================================

#[tokio::test]
async fn f15_single_port_protocol_matrix_and_zero_cors_or_secret_leakage() {
    let tmp = TempDir::new().unwrap();
    let dist_dir = tmp.path().join("dist");
    create_valid_dist_dir(&dist_dir, "<html>desk</html>");

    let verifier = test_verifier();
    let tok_digest_hex = encode_digest_hex(&sha256_token_digest(TRADER_TOK));

    let cfg = FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::EphemeralInMemory,
        policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
        credentials: Some(verifier),
        clock: Arc::new(ManualBlotterClock::new(shanghai_time(
            2026, 6, 15, 10, 0, 0,
        ))),
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    };
    let handle = spawn_server_with_web_ui(cfg, WebUiConfig::StaticDir(dist_dir))
        .await
        .unwrap();
    let addr = handle.addr();
    let endpoint = format!("http://{addr}");

    // 1. Valid native HTTP/2 RPC on RatesAnalyticsService works on the same port
    let mut rates_client = RatesAnalyticsServiceClient::connect(endpoint.clone())
        .await
        .unwrap();
    let u = pb_units();
    let raw_rp = pinned_rulepack_bytes();
    let hedge = rates_client
        .analyze_futures_hedge(rates_pb::AnalyzeFuturesHedgeRequest {
            units: Some(u.clone()),
            product_code: "T".to_owned(),
            target_dv01: Some(dec("1000", u.dv01.as_ref().unwrap())),
            ctd_dv01_per_100: Some(dec("0.075", u.dv01_per_100.as_ref().unwrap())),
            conversion_factor: Some(dec("0.985", u.dimensionless.as_ref().unwrap())),
            delivery_rule_pack: Some(market_pb::MarketRulePack {
                rule_pack_id: Some(ulid("FP")),
                version: 1,
                owner: None,
                market: FUTURES_MARKET.to_owned(),
                rule_type: FUTURES_RULE_TYPE.to_owned(),
                source: "cffex".to_owned(),
                effective_from: Some(pb_time(2026, 1, 1, 0, 0, 0)),
                effective_to: Some(pb_time(2028, 1, 1, 0, 0, 0)),
                verification_status: market_pb::VerificationStatus::Verified as i32,
                content_hash: Some(core_pb::Sha256 {
                    value: ContentHash::digest(&raw_rp).as_bytes().to_vec(),
                }),
                content: Some(prost_types::Any {
                    type_url: FUTURES_TYPE_URL.to_owned(),
                    value: raw_rp,
                }),
            }),
            valuation_at: Some(pb_time(2026, 6, 15, 10, 0, 0)),
        })
        .await
        .unwrap();
    for kv in hedge.metadata().iter() {
        let key_str = match kv {
            tonic::metadata::KeyAndValueRef::Ascii(k, _) => k.as_str(),
            tonic::metadata::KeyAndValueRef::Binary(k, _) => k.as_str(),
        };
        assert!(
            !key_str
                .to_ascii_lowercase()
                .starts_with("access-control-allow-"),
            "unexpected CORS header on native H2 RPC"
        );
    }

    // 2. Unknown RPC #1: Native HTTP/2 unknown service -> gRPC UNIMPLEMENTED (12)
    let channel = tonic::transport::Endpoint::from_shared(endpoint.clone())
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut grpc_client = tonic::client::Grpc::new(channel);
    grpc_client.ready().await.unwrap();
    let path = http::uri::PathAndQuery::from_static("/unknown.pkg.v1.NoSuchService/Call");
    let err = grpc_client
        .unary(Request::new(Vec::<u8>::new()), path, RawEmptyCodec)
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unimplemented);

    // 3. Unknown RPC #2: Valid gRPC-Web POST unknown service -> HTTP 200 with decodable gRPC-Web UNIMPLEMENTED (12)
    let empty_frame = [0x00, 0x00, 0x00, 0x00, 0x00];
    let gw_unknown = raw_http_request(
        addr,
        "POST",
        "/unknown.pkg.v1.NoSuchService/Call",
        &[
            ("Content-Type", "application/grpc-web+proto"),
            ("Accept", "application/grpc-web+proto"),
            ("X-Grpc-Web", "1"),
        ],
        &empty_frame,
    )
    .await;
    assert_eq!(gw_unknown.status, 200);
    gw_unknown.assert_no_cors_headers();
    // gRPC-Web trailer frame starts with 0x80 (or grpc-status header is 12)
    let header_status_12 = gw_unknown.header("grpc-status") == Some("12");
    let trailer_status_12 = gw_unknown.body.len() >= 5
        && gw_unknown.body[0] == 0x80
        && String::from_utf8_lossy(&gw_unknown.body[5..])
            .to_ascii_lowercase()
            .contains("grpc-status:12");
    assert!(
        header_status_12 || trailer_status_12,
        "expected decodable gRPC-Web UNIMPLEMENTED (12), headers={:?}, body={:?}",
        gw_unknown.headers,
        String::from_utf8_lossy(&gw_unknown.body)
    );

    // 4. Unknown RPC #3: Raw HTTP/1.1 with Content-Type: application/grpc -> HTTP 400
    let h1_raw_grpc = raw_http_request(
        addr,
        "POST",
        "/ficant.rates.v1.RatesAnalyticsService/AnalyzeFuturesHedge",
        &[("Content-Type", "application/grpc")],
        &empty_frame,
    )
    .await;
    assert_eq!(h1_raw_grpc.status, 400);
    h1_raw_grpc.assert_no_cors_headers();

    // 5. Info-leak sentinel: /healthz never leaks host paths or credential digests
    let hz = raw_http_request(addr, "GET", "/healthz", &[], &[]).await;
    assert_eq!(hz.status, 200);
    hz.assert_no_cors_headers();
    let hz_text = String::from_utf8_lossy(&hz.body);
    assert!(!hz_text.contains(tmp.path().to_str().unwrap()));
    assert!(!hz_text.contains(TRADER_TOK));
    assert!(!hz_text.contains(&tok_digest_hex));

    handle.shutdown().await.unwrap();
}

// ============================================================================
// TEST 4: AC-004 /healthz 8-Field Contract, Halted 503 & --health-check CLI
// ============================================================================

#[tokio::test]
async fn ac004_healthz_modes_halted_503_and_health_check_cli_mapping() {
    // 1. Address mapping checks for IPv4 & IPv6 unspecified addresses
    assert_eq!(
        map_health_check_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 50051)),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 50051)
    );
    assert_eq!(
        map_health_check_addr(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 50051)),
        SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 50051)
    );
    assert_eq!(
        map_health_check_addr(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
            8080
        )),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)), 8080)
    );

    let expected_keys: BTreeSet<&str> = [
        "status",
        "service",
        "storage_mode",
        "blotter_healthy",
        "ledger_epoch",
        "ledger_instance_id",
        "web_ui_mode",
        "embedded_assets_count",
    ]
    .into_iter()
    .collect();

    // 2. Mode: disabled
    let h_disabled = spawn_server(FicantServerConfig::disabled_default(ephemeral_addr()).unwrap())
        .await
        .unwrap();
    let hz_disabled = raw_http_request(h_disabled.addr(), "GET", "/healthz", &[], &[]).await;
    assert_eq!(hz_disabled.status, 200);
    hz_disabled.assert_no_cors_headers();
    let v_disabled: serde_json::Value = serde_json::from_slice(&hz_disabled.body).unwrap();
    let obj = v_disabled.as_object().unwrap();
    assert_eq!(
        obj.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        expected_keys
    );
    assert_eq!(obj["status"], "ok");
    assert_eq!(obj["service"], "ficant-server");
    assert_eq!(obj["storage_mode"], "disabled");
    assert_eq!(obj["blotter_healthy"], true);
    assert!(obj["ledger_epoch"].is_null());
    assert!(obj["ledger_instance_id"].is_null());
    assert_eq!(obj["web_ui_mode"], "disabled");
    assert_eq!(obj["embedded_assets_count"], 0);

    // HEAD /healthz has empty body and identical headers
    let hz_head = raw_http_request(h_disabled.addr(), "HEAD", "/healthz", &[], &[]).await;
    assert_eq!(hz_head.status, 200);
    assert!(hz_head.body.is_empty());
    assert_eq!(
        hz_head.header("content-length"),
        hz_disabled.header("content-length")
    );
    assert_eq!(
        hz_head.header("content-type"),
        hz_disabled.header("content-type")
    );
    hz_head.assert_no_cors_headers();

    // POST / OPTIONS /healthz -> 405 + Allow: GET, HEAD
    for method in ["POST", "PUT", "DELETE", "OPTIONS"] {
        let m_resp = raw_http_request(h_disabled.addr(), method, "/healthz", &[], &[]).await;
        assert_eq!(m_resp.status, 405);
        assert_eq!(m_resp.header("allow"), Some("GET, HEAD"));
        m_resp.assert_no_cors_headers();
    }

    // Real binary `--health-check` exits 0 when healthy (run on blocking pool so in-process Tokio server keeps running!)
    let server_bin = env!("CARGO_BIN_EXE_ficant-server");
    let disabled_addr_str = h_disabled.addr().to_string();
    let hc_ok = tokio::task::spawn_blocking(move || {
        Command::new(server_bin)
            .arg("--health-check")
            .env("FICANT_SERVER_ADDR", disabled_addr_str)
            .status()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(hc_ok.success());
    h_disabled.shutdown().await.unwrap();

    // 3. Mode: ephemeral_in_memory -> two instances produce distinct canonical ULIDs
    let make_eph_cfg = || FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::EphemeralInMemory,
        policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
        credentials: Some(test_verifier()),
        clock: Arc::new(ManualBlotterClock::new(shanghai_time(
            2026, 6, 15, 10, 0, 0,
        ))),
        service_config: DeskBlotterServiceConfig::default(),
        event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        event_bus_capacity: DEFAULT_EVENT_BUS_CAPACITY,
        watchdog_interval: None,
        spi_backends: SpiBackends::default(),
    };
    let h_eph1 = spawn_server(make_eph_cfg()).await.unwrap();
    let h_eph2 = spawn_server(make_eph_cfg()).await.unwrap();
    let j1: serde_json::Value = serde_json::from_slice(
        &raw_http_request(h_eph1.addr(), "GET", "/healthz", &[], &[])
            .await
            .body,
    )
    .unwrap();
    let j2: serde_json::Value = serde_json::from_slice(
        &raw_http_request(h_eph2.addr(), "GET", "/healthz", &[], &[])
            .await
            .body,
    )
    .unwrap();
    assert_eq!(j1["storage_mode"], "ephemeral_in_memory");
    assert_eq!(j1["ledger_epoch"], 1);
    assert_eq!(j2["ledger_epoch"], 1);
    let id1 = j1["ledger_instance_id"].as_str().unwrap();
    let id2 = j2["ledger_instance_id"].as_str().unwrap();
    assert!(is_canonical_ulid(id1));
    assert!(is_canonical_ulid(id2));
    assert_ne!(id1, id2);
    h_eph1.shutdown().await.unwrap();
    h_eph2.shutdown().await.unwrap();

    // 4. Mode: persistent_wal -> healthy 200 -> inject halt fault -> 503 degraded + --health-check exits non-zero!
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("healthz-wal.sqlite");
    let h_wal = spawn_server(FicantServerConfig {
        listen_addr: ephemeral_addr(),
        storage_mode: BlotterStorageMode::PersistentWal {
            db_path: db_path.clone(),
        },
        policy: DeskBlotterPolicyRuntime::from_embedded_default().unwrap(),
        credentials: Some(test_verifier()),
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
    let expected_id = h_wal.runtime().unwrap().store().ledger_identity().unwrap();
    let hz_wal = raw_http_request(h_wal.addr(), "GET", "/healthz", &[], &[]).await;
    assert_eq!(hz_wal.status, 200);
    let j_wal: serde_json::Value = serde_json::from_slice(&hz_wal.body).unwrap();
    assert_eq!(j_wal["storage_mode"], "persistent_wal");
    assert_eq!(j_wal["ledger_epoch"], expected_id.ledger_epoch);
    assert_eq!(j_wal["ledger_instance_id"], expected_id.ledger_instance_id);
    assert_eq!(j_wal["blotter_healthy"], true);

    // Halt the store via deterministic post-commit memory-apply fault injection
    h_wal
        .runtime()
        .unwrap()
        .store()
        .inject_next_fault(StoreFaultInjection::AfterSqliteCommitBeforeMemoryApply);
    let mut client = DeskBlotterServiceClient::connect(format!("http://{}", h_wal.addr()))
        .await
        .unwrap();
    let _ = client
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "halt-trigger".to_owned(),
                opening_base: Some(make_opening_base()),
                expected_account_version: 0,
            },
            OPERATOR_TOK,
        ))
        .await
        .unwrap();
    assert!(h_wal.runtime().unwrap().store().is_halted());

    let hz_halted = raw_http_request(h_wal.addr(), "GET", "/healthz", &[], &[]).await;
    assert_eq!(hz_halted.status, 503);
    hz_halted.assert_no_cors_headers();
    let j_halted: serde_json::Value = serde_json::from_slice(&hz_halted.body).unwrap();
    assert_eq!(j_halted["status"], "degraded");
    assert_eq!(j_halted["blotter_healthy"], false);

    // `--health-check` MUST exit non-zero on 503 degraded!
    let wal_addr_str = h_wal.addr().to_string();
    let hc_503 = tokio::task::spawn_blocking(move || {
        Command::new(server_bin)
            .arg("--health-check")
            .env("FICANT_SERVER_ADDR", wal_addr_str)
            .status()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(!hc_503.success());
    let wal_addr = h_wal.addr();
    h_wal.shutdown().await.unwrap();

    // Unreachable port fails promptly within bounded 2s probe timeout
    let t0 = Instant::now();
    assert!(probe_healthz(wal_addr, Duration::from_secs(2)).is_err());
    assert!(t0.elapsed() < Duration::from_secs(4));
}

// ============================================================================
// TEST 5: F-09 Bounded SHUTDOWN With Live Native H2 & gRPC-Web Streams + Rollover Lock
// ============================================================================

#[tokio::test]
async fn f09_bounded_stdin_shutdown_drains_live_h2_and_grpc_web_streams_and_releases_rollover_lock()
{
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("shutdown-live.sqlite");
    let archive_dir = tmp.path().join("archives");
    fs::create_dir_all(&archive_dir).unwrap();

    // Write credentials file for child process
    let cred_path = tmp.path().join("credentials.json");
    let cred_json = serde_json::json!({
        "schema_version": 1,
        "credentials": [
            {
                "actor_id": "trader-1",
                "role": "TRADER",
                "token_sha256_hex": encode_digest_hex(&sha256_token_digest(TRADER_TOK)),
                "allowed_accounts": ["acct-1"],
                "allowed_sources": ["FEED-1", "primary-quote-feed"]
            },
            {
                "actor_id": "operator-1",
                "role": "OPERATOR",
                "token_sha256_hex": encode_digest_hex(&sha256_token_digest(OPERATOR_TOK)),
                "allowed_accounts": ["acct-1"],
                "allowed_sources": ["FEED-1", "primary-quote-feed"]
            }
        ]
    });
    fs::write(&cred_path, serde_json::to_vec_pretty(&cred_json).unwrap()).unwrap();

    // Reserve an ephemeral free TCP port for the child server
    let free_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let listen_addr: SocketAddr = format!("127.0.0.1:{free_port}").parse().unwrap();
    let server_bin = env!("CARGO_BIN_EXE_ficant-server");

    let mut child = Command::new(server_bin)
        .env("FICANT_SERVER_ADDR", listen_addr.to_string())
        .env("FICANT_BLOTTER_DB_PATH", &db_path)
        .env("FICANT_BLOTTER_CREDENTIALS_PATH", &cred_path)
        .env("FICANT_ENABLE_STDIN_SHUTDOWN", "1")
        .env("FICANT_SERVE_WEB_UI", "false")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Wait until /healthz reports 200 ok
    let ready_deadline = Instant::now() + Duration::from_secs(10);
    while probe_healthz(listen_addr, Duration::from_millis(300)).is_err() {
        assert!(
            Instant::now() < ready_deadline,
            "child ficant-server failed to become healthy"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // 1. Open Native HTTP/2 SubscribeEvents stream (client will NOT cancel before SHUTDOWN)
    let mut h2_client = DeskBlotterServiceClient::connect(format!("http://{listen_addr}"))
        .await
        .unwrap();
    let mut h2_stream = h2_client
        .subscribe_events(with_bearer(
            pb::SubscribeEventsRequest {
                account_id: Some("acct-1".to_owned()),
                after_seq: 0,
            },
            TRADER_TOK,
        ))
        .await
        .unwrap()
        .into_inner();
    // Receive initial heartbeat frame
    let first_h2 = tokio::time::timeout(Duration::from_secs(3), h2_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        first_h2.payload,
        Some(pb::subscribe_events_response::Payload::Heartbeat(_))
    ));

    // 2. Open raw HTTP/1.1 gRPC-Web SubscribeEvents stream (client will NOT cancel before SHUTDOWN)
    let sub_payload = pb::SubscribeEventsRequest {
        account_id: Some("acct-1".to_owned()),
        after_seq: 0,
    }
    .encode_to_vec();
    let mut sub_framed = Vec::with_capacity(5 + sub_payload.len());
    sub_framed.push(0x00);
    sub_framed.extend_from_slice(&u32::try_from(sub_payload.len()).unwrap().to_be_bytes());
    sub_framed.extend_from_slice(&sub_payload);

    let gw_req = format!(
        "POST /ficant.blotter.v1.DeskBlotterService/SubscribeEvents HTTP/1.1\r\nHost: {listen_addr}\r\nContent-Type: application/grpc-web+proto\r\nAccept: application/grpc-web+proto\r\nX-Grpc-Web: 1\r\nauthorization: Bearer {TRADER_TOK}\r\nContent-Length: {}\r\n\r\n",
        sub_framed.len()
    );
    let mut gw_stream = TcpStream::connect(listen_addr).await.unwrap();
    gw_stream.write_all(gw_req.as_bytes()).await.unwrap();
    gw_stream.write_all(&sub_framed).await.unwrap();
    gw_stream.flush().await.unwrap();

    // Read HTTP headers + initial gRPC-Web heartbeat frame incrementally (proving unbuffered streaming!)
    let mut gw_buf = Vec::new();
    let mut tmp_buf = [0_u8; 4096];
    let gw_read_until_events =
        async |stream: &mut TcpStream, buf: &mut Vec<u8>, target_events: usize| -> usize {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut chunk_buf = [0_u8; 4096];
            loop {
                if let Some(sep) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let count = count_grpc_web_event_frames(&buf[sep + 4..]);
                    if count >= target_events {
                        return count;
                    }
                }
                let n = tokio::time::timeout_at(deadline, stream.read(&mut chunk_buf))
                    .await
                    .expect("timed out waiting for unbuffered gRPC-Web frames")
                    .unwrap();
                assert!(n > 0, "gRPC-Web stream closed prematurely");
                buf.extend_from_slice(&chunk_buf[..n]);
            }
        };

    // Wait for HTTP response headers + initial heartbeat chunk on gRPC-Web stream
    let init_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let n = tokio::time::timeout_at(init_deadline, gw_stream.read(&mut tmp_buf))
            .await
            .unwrap()
            .unwrap();
        gw_buf.extend_from_slice(&tmp_buf[..n]);
        if let Some(sep) = gw_buf.windows(4).position(|w| w == b"\r\n\r\n")
            && gw_buf.len() > sep + 4
        {
            let head_str = String::from_utf8_lossy(&gw_buf[..sep]);
            assert!(head_str.starts_with("HTTP/1.1 200"));
            assert!(
                !head_str
                    .to_ascii_lowercase()
                    .contains("access-control-allow-")
            );
            break;
        }
    }

    // 3. Commit 2 independent transactions and verify BOTH live streams receive BOTH frames immediately!
    let mut writer = DeskBlotterServiceClient::connect(format!("http://{listen_addr}"))
        .await
        .unwrap();
    let tx1 = writer
        .import_opening_base(with_bearer(
            pb::ImportOpeningBaseRequest {
                idempotency_key: "f09-tx-1".to_owned(),
                opening_base: Some(make_opening_base()),
                expected_account_version: 0,
            },
            OPERATOR_TOK,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        tx1.result,
        Some(pb::import_opening_base_response::Result::Value(_))
    ));

    let u = pb_units();
    let tx2 = writer
        .ingest_market_quotes(with_bearer(
            pb::IngestMarketQuotesRequest {
                idempotency_key: "f09-tx-2".to_owned(),
                account_id: "acct-1".to_owned(),
                source_id: "primary-quote-feed".to_owned(),
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
            },
            TRADER_TOK,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(matches!(
        tx2.result,
        Some(pb::ingest_market_quotes_response::Result::Value(_))
    ));

    // Native H2 stream receives 2 live Event frames
    let mut h2_events = 0_usize;
    while h2_events < 2 {
        let msg = tokio::time::timeout(Duration::from_secs(3), h2_stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if matches!(
            msg.payload,
            Some(pb::subscribe_events_response::Payload::Event(_))
        ) {
            h2_events += 1;
        }
    }
    assert_eq!(h2_events, 2);

    // HTTP/1.1 gRPC-Web stream receives 2 live Event frames (unbuffered!)
    let gw_events = gw_read_until_events(&mut gw_stream, &mut gw_buf, 2).await;
    assert!(gw_events >= 2);

    // 4. Do NOT cancel either client; send "SHUTDOWN\n" over child stdin and measure clean exit!
    let shutdown_start = Instant::now();
    {
        use std::io::Write as _;
        let mut child_stdin = child.stdin.take().unwrap();
        child_stdin.write_all(b"SHUTDOWN\n").unwrap();
        child_stdin.flush().unwrap();
    }

    // Both streams terminate cleanly from the server side
    let h2_eof = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(item) = h2_stream.next().await {
            let _ = item;
        }
    })
    .await;
    assert!(
        h2_eof.is_ok(),
        "native H2 stream must end cleanly on SHUTDOWN"
    );

    let gw_eof = tokio::time::timeout(Duration::from_secs(5), async {
        let mut drain = [0_u8; 1024];
        loop {
            match gw_stream.read(&mut drain).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(
        gw_eof.is_ok(),
        "HTTP/1.1 gRPC-Web stream must end cleanly on SHUTDOWN"
    );

    // Child process exits with code 0 well within 10s outer budget
    let exit_status = tokio::task::spawn_blocking(move || {
        let wait_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() >= wait_deadline {
                let _ = child.kill();
                panic!("child ficant-server failed to exit within 10s after SHUTDOWN");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })
    .await
    .unwrap();
    assert!(
        exit_status.success(),
        "expected exit code 0 after SHUTDOWN, got {exit_status}"
    );
    assert!(
        shutdown_start.elapsed() < Duration::from_secs(10),
        "shutdown exceeded 10s budget"
    );

    // 5. Immediately acquire companion rollover lock / execute archive-only rollover on FIRST attempt!
    let report = sqlite_rollover(&SqliteRolloverRequest {
        db_path: db_path.clone(),
        archive_dir,
        archive_name: Some("post-shutdown-check".to_owned()),
        archive_only: true,
        allow_unresolved: false,
    })
    .expect("rollover must acquire companion lock on very first attempt right after SHUTDOWN");
    assert!(report.manifest.wal_truncated);

    // And reopening SqliteBlotterStore succeeds cleanly
    let reopened =
        SqliteBlotterStore::open_file(&db_path, ficant_domain::blotter::BlotterConfig::default())
            .expect("reopen after clean shutdown");
    assert_eq!(reopened.ledger_identity().unwrap().ledger_epoch, 1);
}

#[derive(Clone, Copy, Debug, Default)]
struct RawEmptyCodec;

#[derive(Clone, Copy, Debug, Default)]
struct RawEmptyEncoder;

#[derive(Clone, Copy, Debug, Default)]
struct RawEmptyDecoder;

impl tonic::codec::Encoder for RawEmptyEncoder {
    type Item = Vec<u8>;
    type Error = tonic::Status;

    fn encode(
        &mut self,
        item: Self::Item,
        dst: &mut tonic::codec::EncodeBuf<'_>,
    ) -> Result<(), Self::Error> {
        use bytes::BufMut as _;
        dst.put_slice(&item);
        Ok(())
    }
}

impl tonic::codec::Decoder for RawEmptyDecoder {
    type Item = Vec<u8>;
    type Error = tonic::Status;

    fn decode(
        &mut self,
        src: &mut tonic::codec::DecodeBuf<'_>,
    ) -> Result<Option<Self::Item>, Self::Error> {
        use bytes::Buf as _;
        let mut out = vec![0_u8; src.remaining()];
        src.copy_to_slice(&mut out);
        Ok(Some(out))
    }
}

impl tonic::codec::Codec for RawEmptyCodec {
    type Encode = Vec<u8>;
    type Decode = Vec<u8>;
    type Encoder = RawEmptyEncoder;
    type Decoder = RawEmptyDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        RawEmptyEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawEmptyDecoder
    }
}

/// Counts decoded `SubscribeEventsResponse::Event` data frames inside an HTTP/1.1 chunked
/// gRPC-Web body buffer (tolerates trailing partial chunks while streaming live).
fn count_grpc_web_event_frames(mut raw: &[u8]) -> usize {
    let mut payload_bytes = Vec::new();
    while let Some(pos) = raw.windows(2).position(|w| w == b"\r\n") {
        let Ok(sz_str) = std::str::from_utf8(&raw[..pos]) else {
            break;
        };
        let Ok(sz) = usize::from_str_radix(sz_str.split(';').next().unwrap_or("").trim(), 16)
        else {
            break;
        };
        raw = &raw[pos + 2..];
        if sz == 0 {
            break;
        }
        if raw.len() < sz + 2 {
            break;
        }
        payload_bytes.extend_from_slice(&raw[..sz]);
        raw = &raw[sz + 2..];
    }

    let mut count = 0_usize;
    let mut slice = payload_bytes.as_slice();
    while slice.len() >= 5 {
        let flag = slice[0];
        let len = u32::from_be_bytes([slice[1], slice[2], slice[3], slice[4]]) as usize;
        if slice.len() < 5 + len {
            break;
        }
        if flag == 0x00
            && let Ok(msg) = pb::SubscribeEventsResponse::decode(&slice[5..5 + len])
            && matches!(
                msg.payload,
                Some(pb::subscribe_events_response::Payload::Event(_))
            )
        {
            count += 1;
        }
        slice = &slice[5 + len..];
    }
    count
}
