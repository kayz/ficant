#![allow(
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::missing_panics_doc
)]

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream as StdTcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

use chrono::{NaiveDate, TimeZone};
use chrono_tz::Asia::Shanghai;
use ficant_api::{AUTHORIZATION_HEADER, encode_digest_hex, sha256_token_digest};
use ficant_blotter_store::{
    ArchiveManifest, ManifestStatus, RolloverError, RolloverFaultInjection, RolloverMode,
    RolloverPhase, SqliteRolloverReport, SqliteRolloverRequest, VerifySqliteArchiveReport,
    default_reference_analysis_units, is_canonical_ulid, rollover_state_path,
    sqlite_rollover_with_fault,
};
use ficant_contracts::ficant::blotter::v1 as pb;
use ficant_contracts::ficant::blotter::v1::desk_blotter_service_client::DeskBlotterServiceClient;
use ficant_contracts::ficant::core::v1 as core_pb;
use ficant_contracts::ficant::market::v1 as market_pb;
use ficant_contracts::ficant::rates::v1 as rates_pb;
use ficant_domain::analytics::MARKET_TIMEZONE;
use ficant_domain::primitives::MarketTime;
use ficant_server::probe_healthz;
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tonic::Request;
use tonic::metadata::MetadataValue;

const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(10);
const TRADER_TOK: &str = "bp4-cli-trader-secret-001";
const OPERATOR_TOK: &str = "bp4-cli-operator-secret-002";
const FEED_TOK: &str = "bp4-cli-feed-secret-003";

// ============================================================================
// Subprocess & Test Fixtures Helpers (Strict 10s Outer Timeout on Every Wait)
// ============================================================================

struct CliOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    elapsed: Duration,
}

fn server_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ficant-server")
}

/// Runs `ficant-server` with `args` and optional `envs`, draining stdout & stderr concurrently so
/// pipe buffers can never deadlock, and enforcing a strict `SUBPROCESS_TIMEOUT` (10s) deadline.
fn run_cli_with_timeout(args: &[&str], envs: &[(&str, &str)]) -> CliOutput {
    let start = Instant::now();
    let mut cmd = Command::new(server_bin());
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for &(k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("failed to spawn ficant-server CLI");

    let mut child_stdout = child.stdout.take().expect("piped stdout");
    let mut child_stderr = child.stderr.take().expect("piped stderr");

    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = child_stdout.read_to_end(&mut buf);
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = child_stderr.read_to_end(&mut buf);
        buf
    });

    let deadline = start + SUBPROCESS_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait failed") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let out = stdout_thread.join().unwrap_or_default();
            let err = stderr_thread.join().unwrap_or_default();
            panic!(
                "ficant-server CLI {:?} timed out after {:?}\nstdout: {}\nstderr: {}",
                args,
                SUBPROCESS_TIMEOUT,
                String::from_utf8_lossy(&out),
                String::from_utf8_lossy(&err),
            );
        }
        std::thread::sleep(Duration::from_millis(15));
    };

    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&stdout_thread.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_thread.join().unwrap_or_default()).into_owned();
    CliOutput {
        status,
        stdout,
        stderr,
        elapsed,
    }
}

fn reserve_free_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("read local addr")
}

fn write_test_credentials(path: &Path) {
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
            },
            {
                "actor_id": "feed-1",
                "role": "FEED_ADAPTER",
                "token_sha256_hex": encode_digest_hex(&sha256_token_digest(FEED_TOK)),
                "allowed_accounts": ["acct-1"],
                "allowed_sources": ["FEED-1", "primary-quote-feed"]
            }
        ]
    });
    fs::write(path, serde_json::to_vec_pretty(&cred_json).unwrap()).unwrap();
}

/// RAII wrapper around a live `ficant-server` subprocess that always cleans up on drop and
/// enforces the 10-second outer timeout on shutdown/exit waits.
struct LiveServerChild {
    addr: SocketAddr,
    child: Option<Child>,
}

impl LiveServerChild {
    async fn start(db_path: &Path, cred_path: &Path) -> Self {
        let addr = reserve_free_addr();
        let child = Command::new(server_bin())
            .env("FICANT_SERVER_ADDR", addr.to_string())
            .env("FICANT_BLOTTER_DB_PATH", db_path)
            .env("FICANT_BLOTTER_CREDENTIALS_PATH", cred_path)
            .env("FICANT_ENABLE_STDIN_SHUTDOWN", "1")
            .env("FICANT_SERVE_WEB_UI", "false")
            .env("FICANT_BLOTTER_WATCHDOG_INTERVAL_MS", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn live ficant-server");

        let mut guard = Self {
            addr,
            child: Some(child),
        };

        let deadline = Instant::now() + SUBPROCESS_TIMEOUT;
        while probe_healthz(addr, Duration::from_millis(250)).is_err() {
            if let Some(status) = guard
                .child
                .as_mut()
                .expect("child present")
                .try_wait()
                .expect("try_wait failed")
            {
                let mut stderr_text = String::new();
                if let Some(mut err) = guard.child.as_mut().and_then(|c| c.stderr.take()) {
                    let _ = err.read_to_string(&mut stderr_text);
                }
                panic!(
                    "live ficant-server exited prematurely with {status} before /healthz ready: {stderr_text}"
                );
            }
            assert!(
                Instant::now() < deadline,
                "live ficant-server failed to become healthy within {SUBPROCESS_TIMEOUT:?}"
            );
            tokio::time::sleep(Duration::from_millis(40)).await;
        }

        guard
    }

    fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn fetch_healthz_json(&self) -> serde_json::Value {
        let mut stream = StdTcpStream::connect_timeout(&self.addr, Duration::from_secs(2))
            .expect("connect /healthz");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let req = format!(
            "GET /healthz HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.addr
        );
        stream.write_all(req.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let sep = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("HTTP response separator");
        let head = String::from_utf8_lossy(&raw[..sep]);
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "expected HTTP 200 from /healthz, got: {head}"
        );
        serde_json::from_slice(&raw[sep + 4..]).expect("valid /healthz JSON")
    }

    async fn graceful_shutdown(mut self) -> ExitStatus {
        let mut child = self.child.take().expect("child already consumed");
        {
            let mut stdin = child.stdin.take().expect("piped stdin");
            stdin.write_all(b"SHUTDOWN\n").unwrap();
            stdin.flush().unwrap();
        }
        tokio::task::spawn_blocking(move || wait_child_with_timeout(&mut child))
            .await
            .expect("wait task panicked")
    }

    async fn force_kill(mut self) -> ExitStatus {
        let mut child = self.child.take().expect("child already consumed");
        tokio::task::spawn_blocking(move || {
            let _ = child.kill();
            wait_child_with_timeout(&mut child)
        })
        .await
        .expect("kill wait task panicked")
    }
}

impl Drop for LiveServerChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn wait_child_with_timeout(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + SUBPROCESS_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait failed") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child process did not exit within {SUBPROCESS_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wal_path(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push("-wal");
    PathBuf::from(os)
}

fn file_sha256_opt(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let bytes = fs::read(path).unwrap();
    let digest = Sha256::digest(&bytes);
    Some(ficant_blotter_store::codec::encode_hex(&digest))
}

fn assert_wal_absent_or_empty(db_path: &Path) {
    let wal = wal_path(db_path);
    assert!(
        !wal.exists() || fs::metadata(&wal).unwrap().len() == 0,
        "expected WAL to be absent or 0 bytes at '{}', observed {:?} bytes",
        wal.display(),
        wal.exists().then(|| fs::metadata(&wal).unwrap().len())
    );
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

async fn seed_opening_base_and_quote(endpoint: &str, quote_seq: u64, import_base: bool) {
    let mut client = DeskBlotterServiceClient::connect(endpoint.to_owned())
        .await
        .expect("connect blotter gRPC");

    if import_base {
        let imp = client
            .import_opening_base(with_bearer(
                pb::ImportOpeningBaseRequest {
                    idempotency_key: "cli-imp-1".to_owned(),
                    opening_base: Some(make_opening_base()),
                    expected_account_version: 0,
                },
                OPERATOR_TOK,
            ))
            .await
            .expect("import_opening_base RPC")
            .into_inner();
        assert!(
            matches!(
                imp.result,
                Some(pb::import_opening_base_response::Result::Value(_))
            ),
            "expected ImportOpeningBase Value, got {imp:?}"
        );
    }

    let u = pb_units();
    let quote_resp = client
        .ingest_market_quotes(with_bearer(
            pb::IngestMarketQuotesRequest {
                idempotency_key: format!("cli-q-{quote_seq}"),
                account_id: "acct-1".to_owned(),
                source_id: "primary-quote-feed".to_owned(),
                ticks: vec![pb::MarketQuoteTick {
                    source_id: "primary-quote-feed".to_owned(),
                    quote_seq,
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
        .expect("ingest_market_quotes RPC")
        .into_inner();
    assert!(
        matches!(
            quote_resp.result,
            Some(pb::ingest_market_quotes_response::Result::Value(_))
        ),
        "expected IngestMarketQuotes Value, got {quote_resp:?}"
    );
}

// ============================================================================
// TEST 1: CLI Argument Validation, Mutual Exclusion & Invalid Stems (AC-005/006)
// ============================================================================

#[test]
fn cli_argument_validation_mutual_exclusion_and_invalid_stems() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("db.sqlite");
    let arch = tmp.path().join("arch");
    let man = tmp.path().join("s.manifest.json");
    let db_s = db.to_str().unwrap();
    let arch_s = arch.to_str().unwrap();
    let man_s = man.to_str().unwrap();

    let invalid_invocations: Vec<Vec<&str>> = vec![
        // Mode mutual exclusion
        vec![
            "--health-check",
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
        ],
        vec![
            "--health-check",
            "--verify-sqlite-archive",
            "--manifest",
            man_s,
        ],
        vec![
            "--sqlite-rollover",
            "--verify-sqlite-archive",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--manifest",
            man_s,
        ],
        // `--archive-only` + `--allow-unresolved` conflict
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-only",
            "--allow-unresolved",
        ],
        // Duplicate flags
        vec!["--health-check", "--health-check"],
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
        ],
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-only",
            "--archive-only",
        ],
        vec![
            "--verify-sqlite-archive",
            "--manifest",
            man_s,
            "--manifest",
            man_s,
        ],
        // Missing required arguments & empty values
        vec!["--sqlite-rollover"],
        vec!["--sqlite-rollover", "--db-path", db_s],
        vec!["--sqlite-rollover", "--archive-dir", arch_s],
        vec!["--sqlite-rollover", "--db-path=", "--archive-dir", arch_s],
        vec!["--sqlite-rollover", "--db-path", db_s, "--archive-dir="],
        vec!["--verify-sqlite-archive"],
        vec!["--verify-sqlite-archive", "--manifest="],
        // Cross-mode flag mixing & orphan options
        vec!["--health-check", "--db-path", db_s],
        vec![
            "--verify-sqlite-archive",
            "--manifest",
            man_s,
            "--db-path",
            db_s,
        ],
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--manifest",
            man_s,
        ],
        vec!["--db-path", db_s, "--archive-dir", arch_s],
        vec!["--manifest", man_s],
        // Unknown flags & positional arguments
        vec!["--unknown-flag"],
        vec!["--help"],
        vec!["positional-arg"],
        vec!["--"],
        // Invalid `--archive-name` stems
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-name",
            "CON",
        ],
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-name",
            "snap.sqlite",
        ],
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-name",
            "has.dot",
        ],
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-name",
            "a/b",
        ],
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-name",
            "-leading-dash",
        ],
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-name",
            "..",
        ],
    ];

    for args in &invalid_invocations {
        let out = run_cli_with_timeout(args, &[]);
        assert!(
            !out.status.success(),
            "expected non-zero exit for invalid CLI args {args:?}, got success with stdout={}",
            out.stdout
        );
        assert!(
            out.stdout.trim().is_empty(),
            "expected empty stdout on CLI rejection for {args:?}, got: {}",
            out.stdout
        );
        assert!(
            !out.stderr.trim().is_empty(),
            "expected diagnostic on stderr for invalid CLI args {args:?}"
        );
    }
}

// ============================================================================
// TEST 2: Online Contention Fails Fast & Live Server Keeps Serving (AC-005)
// ============================================================================

#[tokio::test]
async fn online_contention_fails_fast_and_live_server_continues_serving() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("blotter.sqlite");
    let cred_path = tmp.path().join("credentials.json");
    let arch_dir = tmp.path().join("archives");
    fs::create_dir_all(&arch_dir).unwrap();
    write_test_credentials(&cred_path);

    let server = LiveServerChild::start(&db_path, &cred_path).await;
    let hz_before = server.fetch_healthz_json();
    assert_eq!(hz_before["status"], "ok");
    assert_eq!(hz_before["storage_mode"], "persistent_wal");
    assert_eq!(hz_before["ledger_epoch"], 1);

    seed_opening_base_and_quote(&server.endpoint(), 1, true).await;

    let db_s = db_path.to_str().unwrap();
    let arch_s = arch_dir.to_str().unwrap();

    // Both full rollover and `--archive-only` MUST fail immediately on companion lock BUSY/LOCKED!
    for extra in [
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-name",
            "online_reset",
        ],
        vec![
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-name",
            "online_archive",
            "--archive-only",
        ],
    ] {
        let out = run_cli_with_timeout(&extra, &[]);
        assert!(
            !out.status.success(),
            "rollover while server is live MUST exit non-zero for {extra:?}"
        );
        assert!(
            out.elapsed < Duration::from_secs(5),
            "lock conflict must fail fast without retry sleep (elapsed={:?})",
            out.elapsed
        );
        assert!(out.stdout.trim().is_empty());
        let err_lower = out.stderr.to_ascii_lowercase();
        assert!(
            err_lower.contains("busy") || err_lower.contains("locked"),
            "expected lock busy diagnostic on stderr, got: {}",
            out.stderr
        );
    }

    assert!(
        fs::read_dir(&arch_dir).unwrap().next().is_none(),
        "archive dir must remain empty after failed online contention"
    );

    // Live server MUST continue serving `/healthz`, gRPC reads, and gRPC writes uninterrupted!
    let hz_after = server.fetch_healthz_json();
    assert_eq!(hz_after["status"], "ok");
    assert_eq!(hz_after["blotter_healthy"], true);

    let mut client = DeskBlotterServiceClient::connect(server.endpoint())
        .await
        .expect("reconnect to live server");
    let snap = client
        .get_account_snapshot(with_bearer(
            pb::GetAccountSnapshotRequest {
                account_id: "acct-1".to_owned(),
            },
            TRADER_TOK,
        ))
        .await
        .expect("GetAccountSnapshot after contention")
        .into_inner();
    assert!(matches!(
        snap.result,
        Some(pb::get_account_snapshot_response::Result::Value(_))
    ));

    seed_opening_base_and_quote(&server.endpoint(), 2, false).await;

    let exit_status = server.graceful_shutdown().await;
    assert!(
        exit_status.success(),
        "expected clean exit 0 on SHUTDOWN, got {exit_status}"
    );
}

// ============================================================================
// TEST 3: Graceful SHUTDOWN -> Rollover -> Verify -> Idempotent Repeat (AC-005/006)
// ============================================================================

#[tokio::test]
async fn graceful_shutdown_rollover_verify_idempotent_repeat_and_restart() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("blotter.sqlite");
    let cred_path = tmp.path().join("credentials.json");
    let arch_dir = tmp.path().join("archives");
    fs::create_dir_all(&arch_dir).unwrap();
    write_test_credentials(&cred_path);

    let server = LiveServerChild::start(&db_path, &cred_path).await;
    let hz_day1 = server.fetch_healthz_json();
    let orig_instance_id = hz_day1["ledger_instance_id"]
        .as_str()
        .expect("ledger_instance_id")
        .to_owned();
    assert_eq!(hz_day1["ledger_epoch"], 1);
    assert!(is_canonical_ulid(&orig_instance_id));

    seed_opening_base_and_quote(&server.endpoint(), 1, true).await;

    let shutdown_status = server.graceful_shutdown().await;
    assert!(
        shutdown_status.success(),
        "expected exit 0 on graceful SHUTDOWN, got {shutdown_status}"
    );

    let db_s = db_path.to_str().unwrap();
    let arch_s = arch_dir.to_str().unwrap();

    // 1. Default-stem rollover reset (no `--archive-name`)
    let roll_out = run_cli_with_timeout(
        &[
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
        ],
        &[],
    );
    assert!(
        roll_out.status.success(),
        "expected rollover exit 0, stderr: {}",
        roll_out.stderr
    );
    let report: SqliteRolloverReport =
        serde_json::from_str(&roll_out.stdout).expect("valid rollover JSON report");
    assert_eq!(report.mode, RolloverMode::RolloverReset);
    assert_eq!(report.phase, RolloverPhase::CompletedReset);
    assert!(!report.reused_completed_receipt);
    assert_eq!(report.manifest.status, ManifestStatus::CompletedReset);
    assert!(report.manifest.source_reset_committed);
    assert!(report.manifest.wal_truncated);
    assert_eq!(report.manifest.source_ledger_epoch, 1);
    assert_eq!(report.manifest.source_ledger_instance_id, orig_instance_id);
    assert_eq!(report.manifest.next_ledger_epoch, Some(2));
    let next_instance_id = report
        .manifest
        .next_ledger_instance_id
        .clone()
        .expect("next_ledger_instance_id");
    assert!(is_canonical_ulid(&next_instance_id));
    assert_ne!(next_instance_id, orig_instance_id);
    assert_eq!(report.manifest.account_count, 1);
    assert_wal_absent_or_empty(&db_path);

    // 2. Independent `--verify-sqlite-archive` succeeds
    let verify_out = run_cli_with_timeout(
        &[
            "--verify-sqlite-archive",
            "--manifest",
            report.manifest_path.to_str().unwrap(),
        ],
        &[],
    );
    assert!(
        verify_out.status.success(),
        "expected verify exit 0, stderr: {}",
        verify_out.stderr
    );
    let verified: VerifySqliteArchiveReport =
        serde_json::from_str(&verify_out.stdout).expect("valid verify JSON report");
    assert_eq!(verified.manifest, report.manifest);

    // 3. Immediate idempotent repeat with default stem returns the exact same completed receipt
    let repeat_out = run_cli_with_timeout(
        &[
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
        ],
        &[],
    );
    assert!(
        repeat_out.status.success(),
        "expected idempotent repeat exit 0, stderr: {}",
        repeat_out.stderr
    );
    let repeat_report: SqliteRolloverReport =
        serde_json::from_str(&repeat_out.stdout).expect("valid repeat rollover JSON");
    assert!(repeat_report.reused_completed_receipt);
    assert_eq!(repeat_report.operation_id, report.operation_id);
    assert_eq!(repeat_report.stem, report.stem);
    assert_eq!(repeat_report.manifest, report.manifest);

    // 4. Restart server on reset DB -> `/healthz` reports epoch=2 and next_instance_id
    let server_day2 = LiveServerChild::start(&db_path, &cred_path).await;
    let hz_day2 = server_day2.fetch_healthz_json();
    assert_eq!(hz_day2["status"], "ok");
    assert_eq!(hz_day2["blotter_healthy"], true);
    assert_eq!(hz_day2["ledger_epoch"], 2);
    assert_eq!(hz_day2["ledger_instance_id"], next_instance_id);

    let exit_day2 = server_day2.graceful_shutdown().await;
    assert!(exit_day2.success());
}

// ============================================================================
// TEST 4: Forced Kill with Live WAL -> Rollover WAL Recovery & Verify (AC-005/006)
// ============================================================================

#[tokio::test]
async fn forced_kill_with_live_wal_recovers_archives_resets_and_verifies() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("killed_blotter.sqlite");
    let cred_path = tmp.path().join("credentials.json");
    let arch_dir = tmp.path().join("archives");
    fs::create_dir_all(&arch_dir).unwrap();
    write_test_credentials(&cred_path);

    let server = LiveServerChild::start(&db_path, &cred_path).await;
    let hz_before = server.fetch_healthz_json();
    let orig_instance_id = hz_before["ledger_instance_id"].as_str().unwrap().to_owned();

    seed_opening_base_and_quote(&server.endpoint(), 1, true).await;

    // Confirm live WAL has committed frames before abrupt kill
    let wal = wal_path(&db_path);
    assert!(
        wal.exists() && fs::metadata(&wal).unwrap().len() > 0,
        "expected non-empty live WAL before forced kill"
    );

    // Hard kill (`Child::kill`) — OS releases `<db>.rollover.lock` while unmerged WAL remains
    let _kill_status = server.force_kill().await;

    // Rollover using `--flag=<VAL>` syntax recovers WAL, archives, resets epoch 1 -> 2, truncates WAL
    let db_flag = format!("--db-path={}", db_path.to_str().unwrap());
    let arch_flag = format!("--archive-dir={}", arch_dir.to_str().unwrap());
    let roll_out = run_cli_with_timeout(
        &[
            "--sqlite-rollover",
            &db_flag,
            &arch_flag,
            "--archive-name=after_forced_kill",
        ],
        &[],
    );
    assert!(
        roll_out.status.success(),
        "expected post-kill rollover exit 0, stderr: {}",
        roll_out.stderr
    );
    let report: SqliteRolloverReport =
        serde_json::from_str(&roll_out.stdout).expect("valid rollover JSON");
    assert_eq!(report.stem, "after_forced_kill");
    assert_eq!(report.phase, RolloverPhase::CompletedReset);
    assert_eq!(report.manifest.status, ManifestStatus::CompletedReset);
    assert_eq!(report.manifest.source_ledger_epoch, 1);
    assert_eq!(report.manifest.source_ledger_instance_id, orig_instance_id);
    assert_eq!(report.manifest.next_ledger_epoch, Some(2));
    assert_eq!(report.manifest.account_count, 1);
    assert!(report.manifest.head_seq >= 2);
    assert!(report.manifest.counts.retained_events >= 2);
    assert!(report.manifest.wal_truncated);
    assert_wal_absent_or_empty(&db_path);

    let manifest_flag = format!("--manifest={}", report.manifest_path.to_str().unwrap());
    let verify_out = run_cli_with_timeout(&["--verify-sqlite-archive", &manifest_flag], &[]);
    assert!(
        verify_out.status.success(),
        "expected verify exit 0 after forced kill recovery, stderr: {}",
        verify_out.stderr
    );

    // Server restarts cleanly on recovered epoch=2 ledger
    let restarted = LiveServerChild::start(&db_path, &cred_path).await;
    let hz_after = restarted.fetch_healthz_json();
    assert_eq!(hz_after["ledger_epoch"], 2);
    assert_eq!(
        hz_after["ledger_instance_id"].as_str(),
        report.manifest.next_ledger_instance_id.as_deref()
    );
    assert!(restarted.graceful_shutdown().await.success());
}

// ============================================================================
// TEST 5: Unresolved COMMITTED & OPEN Block (Zero Mutation), Override & Archive-Only
// ============================================================================

#[tokio::test]
async fn unresolved_block_zero_mutation_allow_unresolved_override_and_archive_only() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("unresolved.sqlite");
    let cred_path = tmp.path().join("credentials.json");
    let arch_dir = tmp.path().join("archives");
    fs::create_dir_all(&arch_dir).unwrap();
    write_test_credentials(&cred_path);

    let server = LiveServerChild::start(&db_path, &cred_path).await;
    seed_opening_base_and_quote(&server.endpoint(), 1, true).await;

    let mut client = DeskBlotterServiceClient::connect(server.endpoint())
        .await
        .unwrap();
    let u = pb_units();
    let cur = u.currency_amount.as_ref().unwrap();
    let px = u.price_per_100.as_ref().unwrap();

    // Create 1 unresolved COMMITTED earmark
    let comm = client
        .create_committed(with_bearer(
            pb::CreateCommittedRequest {
                idempotency_key: "cli-comm-1".to_owned(),
                account_id: "acct-1".to_owned(),
                expected_position_version: 1,
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
                note: "unresolved committed".to_owned(),
            },
            TRADER_TOK,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(
        matches!(
            comm.result,
            Some(pb::create_committed_response::Result::Value(_))
        ),
        "expected CreateCommitted Value, got {comm:?}"
    );

    // Ingest 1 unmatched BUY fill -> creates 1 OPEN pending_match!
    let feed = client
        .ingest_trade_feed(with_bearer(
            pb::IngestTradeFeedRequest {
                idempotency_key: "cli-orphan-1".to_owned(),
                source_id: "FEED-1".to_owned(),
                fills: vec![pb::TradeFillRecord {
                    source_id: "FEED-1".to_owned(),
                    external_trade_id: "TRD-ORPHAN-CLI-1".to_owned(),
                    source_revision: 1,
                    feed_seq: 1,
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
                    executed_at: Some(pb_time(2026, 6, 15, 10, 5, 0)),
                    batch_first_feed_seq: 1,
                    batch_last_feed_seq: 1,
                }],
                heartbeat_at: Some(pb_time(2026, 6, 15, 10, 5, 1)),
            },
            FEED_TOK,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(
        matches!(
            feed.result,
            Some(pb::ingest_trade_feed_response::Result::Value(_))
        ),
        "expected IngestTradeFeed Value, got {feed:?}"
    );

    // Force-kill so live unmerged WAL remains on disk when testing zero-mutation block
    let _ = server.force_kill().await;

    let wal = wal_path(&db_path);
    assert!(
        wal.exists() && fs::metadata(&wal).unwrap().len() > 0,
        "expected non-empty WAL for zero-mutation verification"
    );
    let db_sha_before = file_sha256_opt(&db_path).unwrap();
    let wal_sha_before = file_sha256_opt(&wal).unwrap();

    let db_s = db_path.to_str().unwrap();
    let arch_s = arch_dir.to_str().unwrap();

    // 1. Rollover WITHOUT `--allow-unresolved` MUST block with ZERO byte mutations on DB & WAL!
    let blocked = run_cli_with_timeout(
        &[
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
        ],
        &[],
    );
    assert!(
        !blocked.status.success(),
        "expected non-zero exit when unresolved COMMITTED & OPEN exist"
    );
    assert!(blocked.stdout.trim().is_empty());
    assert!(
        blocked.stderr.contains("unresolved_committed=1")
            && blocked.stderr.contains("open_pending_match=1"),
        "expected unresolved counts on stderr, got: {}",
        blocked.stderr
    );
    assert_eq!(
        file_sha256_opt(&db_path).unwrap(),
        db_sha_before,
        "blocked rollover MUST NOT mutate source DB bytes"
    );
    assert_eq!(
        file_sha256_opt(&wal).unwrap(),
        wal_sha_before,
        "blocked rollover MUST NOT mutate source WAL bytes"
    );
    assert!(!rollover_state_path(&db_path).exists());
    assert!(fs::read_dir(&arch_dir).unwrap().next().is_none());

    // 2. `--archive-only` succeeds and verifies cleanly even with unresolved items
    let arch_only = run_cli_with_timeout(
        &[
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-name",
            "snap_with_unresolved",
            "--archive-only",
        ],
        &[],
    );
    assert!(
        arch_only.status.success(),
        "expected --archive-only exit 0, stderr: {}",
        arch_only.stderr
    );
    let arch_rep: SqliteRolloverReport = serde_json::from_str(&arch_only.stdout).unwrap();
    assert_eq!(arch_rep.mode, RolloverMode::ArchiveOnly);
    assert_eq!(arch_rep.phase, RolloverPhase::CompletedArchiveOnly);
    assert_eq!(arch_rep.manifest.status, ManifestStatus::ArchiveOnly);
    assert!(arch_rep.manifest.next_ledger_epoch.is_none());
    assert!(arch_rep.manifest.next_ledger_instance_id.is_none());
    assert!(!arch_rep.manifest.allow_unresolved_used);
    assert_eq!(arch_rep.manifest.counts.unresolved_committed, 1);
    assert_eq!(arch_rep.manifest.counts.open_pending_match, 1);
    assert!(
        run_cli_with_timeout(
            &[
                "--verify-sqlite-archive",
                "--manifest",
                arch_rep.manifest_path.to_str().unwrap(),
            ],
            &[],
        )
        .status
        .success()
    );

    // 3. Explicit `--allow-unresolved` override completes reset (epoch 1 -> 2) and verifies
    let forced = run_cli_with_timeout(
        &[
            "--sqlite-rollover",
            "--db-path",
            db_s,
            "--archive-dir",
            arch_s,
            "--archive-name",
            "forced_override_reset",
            "--allow-unresolved",
        ],
        &[],
    );
    assert!(
        forced.status.success(),
        "expected --allow-unresolved reset exit 0, stderr: {}",
        forced.stderr
    );
    let forced_rep: SqliteRolloverReport = serde_json::from_str(&forced.stdout).unwrap();
    assert_eq!(forced_rep.mode, RolloverMode::RolloverReset);
    assert_eq!(forced_rep.phase, RolloverPhase::CompletedReset);
    assert_eq!(forced_rep.manifest.status, ManifestStatus::CompletedReset);
    assert!(forced_rep.manifest.allow_unresolved_used);
    assert!(forced_rep.manifest.wal_truncated);
    assert_eq!(forced_rep.manifest.source_ledger_epoch, 1);
    assert_eq!(forced_rep.manifest.next_ledger_epoch, Some(2));
    assert_wal_absent_or_empty(&db_path);
    assert!(
        run_cli_with_timeout(
            &[
                "--verify-sqlite-archive",
                "--manifest",
                forced_rep.manifest_path.to_str().unwrap(),
            ],
            &[],
        )
        .status
        .success()
    );
}

// ============================================================================
// TEST 6: Legacy v1 `--archive-only`, Maintenance-Pending Resume & Tamper Reject
// ============================================================================

#[tokio::test]
async fn legacy_v1_archive_only_maintenance_pending_resume_and_tamper_rejection() {
    let tmp = TempDir::new().unwrap();

    // ------------------------------------------------------------------------
    // 6a. Legacy 5-column v1 DB `--archive-only` via real CLI subprocess
    // ------------------------------------------------------------------------
    let legacy_db = tmp.path().join("legacy_v1.sqlite");
    let legacy_arch = tmp.path().join("legacy_arch");
    fs::create_dir_all(&legacy_arch).unwrap();
    write_legacy_v1_db_fixture(&legacy_db);

    let legacy_sha_before = file_sha256_opt(&legacy_db).unwrap();
    let v1_out_1 = run_cli_with_timeout(
        &[
            "--sqlite-rollover",
            "--db-path",
            legacy_db.to_str().unwrap(),
            "--archive-dir",
            legacy_arch.to_str().unwrap(),
            "--archive-name",
            "legacy_v1_snap_1",
            "--archive-only",
        ],
        &[],
    );
    assert!(
        v1_out_1.status.success(),
        "expected legacy v1 --archive-only exit 0, stderr: {}",
        v1_out_1.stderr
    );
    let v1_rep_1: SqliteRolloverReport = serde_json::from_str(&v1_out_1.stdout).unwrap();
    assert_eq!(v1_rep_1.mode, RolloverMode::ArchiveOnly);
    assert_eq!(v1_rep_1.manifest.status, ManifestStatus::ArchiveOnly);
    assert!(!v1_rep_1.manifest.wal_truncated);
    assert_eq!(v1_rep_1.manifest.source_ledger_epoch, 1);
    assert!(is_canonical_ulid(
        &v1_rep_1.manifest.source_ledger_instance_id
    ));
    assert_eq!(
        file_sha256_opt(&legacy_db).unwrap(),
        legacy_sha_before,
        "legacy v1 --archive-only MUST leave source DB bytes unchanged"
    );
    assert!(
        run_cli_with_timeout(
            &[
                "--verify-sqlite-archive",
                "--manifest",
                v1_rep_1.manifest_path.to_str().unwrap(),
            ],
            &[],
        )
        .status
        .success()
    );

    // Repeat with another stem -> deterministic derived ULID matches exactly
    let v1_out_2 = run_cli_with_timeout(
        &[
            "--sqlite-rollover",
            "--db-path",
            legacy_db.to_str().unwrap(),
            "--archive-dir",
            legacy_arch.to_str().unwrap(),
            "--archive-name",
            "legacy_v1_snap_2",
            "--archive-only",
        ],
        &[],
    );
    assert!(v1_out_2.status.success());
    let v1_rep_2: SqliteRolloverReport = serde_json::from_str(&v1_out_2.stdout).unwrap();
    assert_eq!(
        v1_rep_2.manifest.source_ledger_instance_id,
        v1_rep_1.manifest.source_ledger_instance_id
    );

    // ------------------------------------------------------------------------
    // 6b. Default-stem resume from RESET_COMMITTED_MAINTENANCE_PENDING via CLI
    // ------------------------------------------------------------------------
    let resume_db = tmp.path().join("resume.sqlite");
    let cred_path = tmp.path().join("credentials.json");
    let resume_arch = tmp.path().join("resume_arch");
    fs::create_dir_all(&resume_arch).unwrap();
    write_test_credentials(&cred_path);

    let srv = LiveServerChild::start(&resume_db, &cred_path).await;
    seed_opening_base_and_quote(&srv.endpoint(), 1, true).await;
    assert!(srv.graceful_shutdown().await.success());

    // Inject Window 5 fault right before VACUUM / final wal_checkpoint(TRUNCATE)
    let fault_err = sqlite_rollover_with_fault(
        &SqliteRolloverRequest {
            db_path: resume_db.clone(),
            archive_dir: resume_arch.clone(),
            archive_name: None,
            archive_only: false,
            allow_unresolved: false,
        },
        RolloverFaultInjection::DuringMaintenanceVacuumOrCheckpoint,
    )
    .unwrap_err();
    assert_eq!(
        fault_err,
        RolloverError::FaultInjected(RolloverFaultInjection::DuringMaintenanceVacuumOrCheckpoint)
    );

    let state_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(rollover_state_path(&resume_db)).unwrap())
            .unwrap();
    assert_eq!(
        state_json["active"]["phase"].as_str(),
        Some("RESET_COMMITTED_MAINTENANCE_PENDING")
    );
    let expected_op = state_json["active"]["operation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let expected_stem = state_json["active"]["stem"].as_str().unwrap().to_owned();
    let expected_next = state_json["active"]["next_ledger_instance_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        !resume_arch
            .join(format!("{expected_stem}.manifest.json"))
            .exists(),
        "final manifest must not exist while maintenance is pending"
    );

    // Server MUST refuse to start while rollover is in RESET_COMMITTED_MAINTENANCE_PENDING!
    let blocked_srv = run_cli_with_timeout(
        &[],
        &[
            ("FICANT_SERVER_ADDR", &reserve_free_addr().to_string()),
            ("FICANT_BLOTTER_DB_PATH", resume_db.to_str().unwrap()),
            (
                "FICANT_BLOTTER_CREDENTIALS_PATH",
                cred_path.to_str().unwrap(),
            ),
            ("FICANT_SERVE_WEB_UI", "false"),
        ],
    );
    assert!(
        !blocked_srv.status.success(),
        "ficant-server MUST refuse to open DB during RESET_COMMITTED_MAINTENANCE_PENDING"
    );

    // Real CLI default-stem retry (NO `--archive-name`) resumes and completes cleanly!
    let resumed_out = run_cli_with_timeout(
        &[
            "--sqlite-rollover",
            "--db-path",
            resume_db.to_str().unwrap(),
            "--archive-dir",
            resume_arch.to_str().unwrap(),
        ],
        &[],
    );
    assert!(
        resumed_out.status.success(),
        "expected default-stem resume exit 0, stderr: {}",
        resumed_out.stderr
    );
    let resumed: SqliteRolloverReport = serde_json::from_str(&resumed_out.stdout).unwrap();
    assert_eq!(resumed.operation_id, expected_op);
    assert_eq!(resumed.stem, expected_stem);
    assert_eq!(resumed.phase, RolloverPhase::CompletedReset);
    assert!(!resumed.reused_completed_receipt);
    assert_eq!(resumed.manifest.status, ManifestStatus::CompletedReset);
    assert_eq!(resumed.manifest.next_ledger_epoch, Some(2));
    assert_eq!(
        resumed.manifest.next_ledger_instance_id.as_deref(),
        Some(expected_next.as_str())
    );
    assert!(resumed.manifest.wal_truncated);
    assert_wal_absent_or_empty(&resume_db);

    let verified_resumed = run_cli_with_timeout(
        &[
            "--verify-sqlite-archive",
            "--manifest",
            resumed.manifest_path.to_str().unwrap(),
        ],
        &[],
    );
    assert!(
        verified_resumed.status.success(),
        "expected verify exit 0 after maintenance-pending resume, stderr: {}",
        verified_resumed.stderr
    );

    // ------------------------------------------------------------------------
    // 6c. Tampered archive & tampered manifest MUST fail `--verify-sqlite-archive`
    // ------------------------------------------------------------------------
    // Case 1: 1-byte flip in archive SQLite file
    let tamper_arch_dir = tmp.path().join("tamper_archive");
    fs::create_dir_all(&tamper_arch_dir).unwrap();
    let t_arch = tamper_arch_dir.join(format!("{}.sqlite", resumed.stem));
    let t_man = tamper_arch_dir.join(format!("{}.manifest.json", resumed.stem));
    let mut arch_bytes = fs::read(&resumed.archive_path).unwrap();
    let mid = arch_bytes.len() / 2;
    arch_bytes[mid] ^= 0xFF;
    fs::write(&t_arch, &arch_bytes).unwrap();
    fs::copy(&resumed.manifest_path, &t_man).unwrap();

    let bad_arch_verify = run_cli_with_timeout(
        &[
            "--verify-sqlite-archive",
            "--manifest",
            t_man.to_str().unwrap(),
        ],
        &[],
    );
    assert!(
        !bad_arch_verify.status.success(),
        "tampered archive MUST cause --verify-sqlite-archive to exit non-zero"
    );
    assert!(bad_arch_verify.stdout.trim().is_empty());
    assert!(!bad_arch_verify.stderr.trim().is_empty());

    // Case 2: Mutated manifest field (`counts.retained_events` and `content_sha256_hex`)
    let orig_manifest: ArchiveManifest =
        serde_json::from_str(&fs::read_to_string(&resumed.manifest_path).unwrap()).unwrap();
    for (tag, mutate) in [
        (
            "counts",
            (|m: &mut ArchiveManifest| m.counts.retained_events += 1) as fn(&mut ArchiveManifest),
        ),
        (
            "content_sha",
            (|m: &mut ArchiveManifest| m.content_sha256_hex = "0".repeat(64))
                as fn(&mut ArchiveManifest),
        ),
    ] {
        let tdir = tmp.path().join(format!("tamper_manifest_{tag}"));
        fs::create_dir_all(&tdir).unwrap();
        fs::copy(
            &resumed.archive_path,
            tdir.join(format!("{}.sqlite", resumed.stem)),
        )
        .unwrap();
        let mut mutated = orig_manifest.clone();
        mutate(&mut mutated);
        let mpath = tdir.join(format!("{}.manifest.json", resumed.stem));
        fs::write(&mpath, serde_json::to_vec_pretty(&mutated).unwrap()).unwrap();

        let bad_man_verify = run_cli_with_timeout(
            &[
                "--verify-sqlite-archive",
                "--manifest",
                mpath.to_str().unwrap(),
            ],
            &[],
        );
        assert!(
            !bad_man_verify.status.success(),
            "tampered manifest ({tag}) MUST cause --verify-sqlite-archive to exit non-zero"
        );
        assert!(bad_man_verify.stdout.trim().is_empty());
        assert!(!bad_man_verify.stderr.trim().is_empty());
    }

    // Case 3: Non-existent manifest file
    let missing_man = tmp.path().join("does_not_exist.manifest.json");
    let missing_verify = run_cli_with_timeout(
        &[
            "--verify-sqlite-archive",
            "--manifest",
            missing_man.to_str().unwrap(),
        ],
        &[],
    );
    assert!(!missing_verify.status.success());
    assert!(missing_verify.stdout.trim().is_empty());
}

// ============================================================================
// Self-Contained Legacy 5-Column v1 SQLite Fixture (Zero External Tools Needed)
// ============================================================================

fn write_legacy_v1_db_fixture(path: &Path) {
    let mut buf = vec![0_u8; LEGACY_V1_DB_LEN];
    for &(offset, chunk) in LEGACY_V1_DB_CHUNKS {
        buf[offset..offset + chunk.len()].copy_from_slice(chunk);
    }
    fs::write(path, &buf).expect("write legacy v1 fixture");
}

const LEGACY_V1_DB_LEN: usize = 35_328;
const LEGACY_V1_DB_CHUNKS: &[(usize, &[u8])] = &[
    (0, b"SQLite format 3\x00\x02\x00\x01\x01\x00@  \x00\x00\x00\x17\x00\x00\x00E"),
    (43, b"\x16\x00\x00\x00\x04"),
    (59, b"\x01"),
    (95, b"\x17\x00.\x86)\x05\x00\x00\x00\x11\x01\xab\x00\x00\x00\x00E\x01\xfb\x01\xf6\x01\xf1\x01\xec\x01\xe7\x01\xe2\x01\xdd\x01\xd8\x01\xd3\x01\xce\x01\xc9\x01\xc4\x01\xbf\x01\xba\x01\xb5\x01\xb0\x01\xab\x00\x00\x00\x06\x02\x06\x00\x00\x00\x00\x00\x82`\x01\x07\x17%%\x01\x85\x03tableblotter_metablotter_meta\x02CREATE TABLE blotter_meta (\x0a    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),\x0a    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),\x0a    head_seq INTEGER NOT NULL CHECK (head_seq >= 0),\x0a    oldest_retained_s\x00\x00\x00B$\x00\x00\x00?\x22\x00\x00\x00;\x1f\x00\x00\x00:\x1d\x00\x00\x007\x1c\x00\x00\x004\x1b\x00\x00\x003\x19\x00\x00\x00.\x18\x00\x00\x00&\x16\x00\x00\x00#\x12\x00\x00\x00\x1e\x10\x00\x00\x00\x18\x0d\x00\x00\x00\x14\x0a\x00\x00\x00\x0d\x08\x00\x00\x00\x0c\x05\x00\x00\x00\x09\x04\x00\x00\x00\x05\x01\x0d\x00\x00\x00\x01\x01I\x00\x01I"),
    (841, b"\x814\x01\x07\x00\x09\x08\x09\x82g{\x22clean_price_tolerance\x22:\x220.050000000000\x22,\x22yield_tolerance\x22:\x220.000500000000\x22,\x22committed_overdue_seconds\x22:3600,\x22pre_close_alert_time\x22:\x2216:30:00\x22,\x22feed_staleness_seconds\x22:300}\x0d\x00\x00\x00\x00\x02"),
    (1536, b"\x0a\x00\x00\x00\x00\x02"),
    (2048, b"\x0d\x00\x00\x00\x01\x00\x9d\x00\x00\x9d\x00T\x00\x13\x00\x13\x00\x00\x00?\x04\x06\x17S-\x01\x00indexsqlite_autoindex_blotter_accounts_2blotter_accounts\x06?\x03\x06\x17S-\x01\x00indexsqlite_autoindex_blotter_accounts_1blotter_accounts\x04\x00\x00\x00\x08\x00\x00\x00\x00\x82`\x01\x07\x17%%\x01\x85\x03tableblotter_metablotter_meta\x02CREATE TABLE blotter_meta (\x0a    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),\x0a    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),\x0a    head_seq INTEGER NOT NULL CHECK (head_seq >= 0),\x0a    oldest_retained_seq INTEGER NOT NULL CHECK (oldest_retained_seq >= 1),\x0a    config_json TEXT NOT NULL\x0a)\x0a\x00\x00\x00\x00\x02"),
    (3075, b"\x08r_accounts (\x0a    account_id TEXT PRIMARY KEY NOT NULL,\x0a    base_id TEXT NOT NULL UNIQUE,\x0a    trading_date TEXT NOT NULL,\x0a    next_settlement_date TEXT NOT NULL,\x0a    effective_from_json TEXT NOT NULL,\x0a    watermark_time_json TEXT NOT NULL,\x0a    watermark_source_seq INTEGER CHECK (watermark_source_seq IS NULL OR watermark_source_seq >= 0),\x0a    source_id TEXT NOT NULL,\x0a    source_start_after_feed_seq INTEGER NOT NULL CHECK (source_start_after_feed_seq >= 0),\x0a    opening_cash_t0 TEXT NOT NULL,\x0a    opening_ne\x00\x00\x00\x00t_cash_delta_t1 TEXT NOT NULL,\x0a    account_version INTEGER NOT NULL CHECK (account_version >= 1),\x0a    policy_version INTEGER NOT NULL CHECK (policy_version >= 1),\x0a    default_quote_source_id TEXT NOT NULL,\x0a    import_actor_id TEXT NOT NULL,\x0a    import_idempotency_key TEXT NOT NULL,\x0a    import_request_hash BLOB NOT NULL CHECK (length(import_request_hash) = 32),\x0a    import_committed_seq INTEGER NOT NULL CHECK (import_committed_seq >= 1),\x0a    opening_base_json TEXT NOT NULL,\x0a    policy_json TEXT NOT NULL\x0a)\x0d\x00\x00\x00\x03\x017\x00\x01\xb9\x01x\x017\x00\xe8\x00\xe8"),
    (4328, b"E\x06\x06\x17Y3\x01\x00indexsqlite_autoindex_blotter_instruments_1blotter_instruments\x0b\x00\x00\x00\x08\x00\x00\x00\x00?\x04\x06\x17S-\x01\x00indexsqlite_autoindex_blotter_accounts_2blotter_accounts\x06?\x03\x06\x17S-\x01\x00indexsqlite_autoindex_blotter_accounts_1blotter_accounts\x04\x888\x02\x07\x17--\x01\x90#tableblotter_accountsblotter_accounts\x03CREATE TABLE blotte\x00\x00\x00\x07\x0d\x00\x00\x00\x00\x02"),
    (5120, b"\x0a\x00\x00\x00\x00\x02"),
    (5632, b"\x0d\x00\x00\x00\x01\x00(\x00\x00("),
    (5672, b"\x83U\x05\x07\x1733\x01\x86Qtableblotter_instrumentsblotter_instruments\x0aCREATE TABLE blotter_instruments (\x0a    account_id TEXT NOT NULL REFERENCES blotter_accounts(account_id) ON DELETE RESTRICT,\x0a    instrument_code TEXT NOT NULL,\x0a    active_quote_source_id TEXT NOT NULL,\x0a    policy_version INTEGER NOT NULL CHECK (policy_version >= 1),\x0a    has_catalog_entry INTEGER NOT NULL CHECK (has_catalog_entry IN (0, 1)),\x0a    catalog_entry_json TEXT,\x0a    PRIMARY KEY (account_id, instrument_code)\x0a)\x0d\x01\xb1\x00\x03\x00\xdc\x00\x01\xb9\x00\xdc\x01n\x00\x95\x00\x95"),
    (6293, b"E\x0a\x06\x17Y3\x01\x00indexsqlite_autoindex_blotter_feed_status_1blotter_feed_status\x12\x85\x07\x07\x07\x17//\x01\x89=tableblotter_positionsblotter_positions\x0eCREATE TABLE blotter_positions (\x0a    account_id TEXT NOT NULL REFERENCES blotter_accounts(ac\x00\x00\x00\x10A\x08\x06\x17U/\x01\x00indexsqlite_autoindex_blotter_positions_1blotter_positions\x0f\x00\x00\x00\x08\x00\x00\x00\x00E\x06\x06\x17Y3\x01\x00indexsqlite_autoindex_blotter_instruments_1blotter_instruments\x0b\x0d\x00\x00\x00\x00\x02"),
    (7168, b"\x0a\x00\x00\x00\x00\x02"),
    (7684, b"count_id) ON DELETE RESTRICT,\x0a    instrument_code TEXT NOT NULL,\x0a    position_version INTEGER NOT NULL CHECK (position_version >= 1),\x0a    opening_economic_notional TEXT NOT NULL,\x0a    opening_available_notional TEXT NOT NULL,\x0a    opening_net_t0_notional TEXT NOT NULL,\x0a    opening_net_t1_notional TEXT NOT NULL,\x0a    reconciled_buy_t0 TEXT NOT NULL,\x0a    reconciled_buy_t1 TEXT NOT NULL,\x0a    reconciled_sell_t0 TEXT NOT NULL,\x0a    reconciled_sell_t1 TEXT NOT NULL,\x0a    PRIMARY KEY (account_id, instrument_code)\x0a)\x0d\x00\x00\x00\x00\x02"),
    (8704, b"\x0a\x00\x00\x00\x00\x02"),
    (9220, b"s_feed_seq INTEGER NOT NULL CHECK (contiguous_feed_seq >= 0),\x0a    max_seen_feed_seq INTEGER NOT NULL CHECK (max_seen_feed_seq >= 0),\x0a    missing_feed_ranges_json TEXT NOT NULL,\x0a    paused_by_gap INTEGER NOT NULL CHECK (paused_by_gap IN (0, 1)),\x0a    paused_by_staleness INTEGER NOT NULL CHECK (paused_by_staleness IN (0, 1)),\x0a    paused_by_operator INTEGER NOT NULL CHECK (paused_by_operator IN (0, 1)),\x0a    last_heartbeat_at_json TEXT,\x0a    status_reason TEXT NOT NULL,\x0a    source_batches_json TEXT NOT NULL\x0a)\x0d\x00\x00\x00\x02\x00\x88\x00\x00\xcf\x00\x88\x00/\x00/"),
    (9775, b"O\x0c\x06\x17c=\x01\x00indexsqlite_autoindex_blotter_feed_raw_packets_1blotter_feed_raw_packets\x16\x00\x00\x00\x08\x00\x00\x00\x00E\x0a\x06\x17Y3\x01\x00indexsqlite_autoindex_blotter_feed_status_1blotter_feed_status\x12\x86&\x09\x07\x1733\x01\x8bstableblotter_feed_statusblotter_feed_status\x11CREATE TABLE blotter_feed_status (\x0a    source_id TEXT PRIMARY KEY NOT NULL,\x0a    feed_status_revision INTEGER NOT NULL CHECK (feed_status_revision >= 1),\x0a    initial_after_feed_seq INTEGER NOT NULL CHECK (initial_after_feed_seq >= 0),\x0a    contiguou\x00\x00\x00\x13\x0d\x00\x00\x00\x00\x02"),
    (10_752, b"\x0a\x00\x00\x00\x00\x02"),
    (11_268, b" NULL CHECK (feed_seq >= 1),\x0a    external_trade_id TEXT NOT NULL,\x0a    source_revision INTEGER NOT NULL CHECK (source_revision >= 1),\x0a    payload_hash BLOB NOT NULL CHECK (length(payload_hash) = 32),\x0a    business_hash BLOB NOT NULL CHECK (length(business_hash) = 32),\x0a    batch_first_feed_seq INTEGER NOT NULL CHECK (batch_first_feed_seq >= 1),\x0a    batch_last_feed_seq INTEGER NOT NULL CHECK (batch_last_feed_seq >= batch_first_feed_seq),\x0a    record_json TEXT NOT NULL,\x0a    PRIMARY KEY (source_id, feed_seq)\x0a)\x0d\x00\x00\x00\x03\x00p\x00\x01\x22\x00\xd1\x00p\x00z\x00\xc9\x00\x00\x00\x00\x00\x00\x00\x84V\x0d\x07\x17;;\x01\x88Ctableblotter_ingested_tradesblotter_ingested_trades\x19CREATE TABLE blotter_inge\x84V\x0d\x07\x17;;\x01\x88Ctableblotter_ingested_tradesblotter_ingested_trades\x19CREATE TABLE blotter_ingested_t\x00\x00\x00\x1bO\x0c\x06\x17c=\x01\x00indexsqlite_autoindex_blotter_feed_raw_packets_1blotter_feed_raw_packets\x16\x85S\x0b\x07\x17==\x01\x8a9tableblotter_feed_raw_packetsblotter_feed_raw_packets\x15CREATE TABLE blotter_feed_raw_packets (\x0a    source_id TEXT NOT NULL REFERENCES blotter_feed_status(source_id) ON DELETE RESTRICT,\x0a    feed_seq INTEGER NOT\x00\x00\x00\x17\x0d\x00\x00\x00\x00\x02"),
    (12_800, b"\x0a\x00\x00\x00\x00\x02"),
    (13_316, b"rades (\x0a    source_id TEXT NOT NULL REFERENCES blotter_feed_status(source_id) ON DELETE RESTRICT,\x0a    external_trade_id TEXT NOT NULL,\x0a    source_revision INTEGER NOT NULL CHECK (source_revision >= 1),\x0a    business_hash BLOB NOT NULL CHECK (length(business_hash) = 32),\x0a    canonical_feed_seq INTEGER NOT NULL CHECK (canonical_feed_seq >= 1),\x0a    alias_feed_seqs_json TEXT NOT NULL,\x0a    lifecycle TEXT NOT NULL,\x0a    record_json TEXT NOT NULL,\x0a    PRIMARY KEY (source_id, external_trade_id, source_revision)\x0a)\x0d\x00\x00\x00\x00\x02"),
    (14_336, b"\x0a\x00\x00\x00\x00\x02"),
    (14_848, b"\x0d\x01H\x00\x03\x00\x90\x00\x01P\x00\x90\x00\xe5\x00O\x00O\x00O"),
    (14_927, b"?\x12\x06\x17S-\x01\x00indexsqlite_autoindex_blotter_earmarks_1blotter_earmarks!\x84J\x0f\x07\x17OO\x01\x88\x03tableblotter_trade_effective_revisionsblotter_trade_effective_revisions\x00\x00\x00\x1fa\x10\x06\x17uO\x01\x00indexsqlite_autoindex_blotter_trade_effective_revisions_1blotter_trade_effective_revisions\x1d\x01\x9f\x00\x08\x00\x00\x00\x00M\x0e\x06\x17a;\x01\x00indexsqlite_autoindex_blotter_ingested_trades_1blotter_ingested_trades\x1a\x00\x00\x00a\x17;;\x01\x88Ctableblotter_ingested_tradesblotter_ingested_trades\x19CREATE TABLE blotter_ingested_t\x00\x00\x00\x1b\x00\x00\x00\x00\x1cCREATE TABLE blotter_trade_effective_revisions (\x0a    source_id TEXT NOT NULL REFERENCES blotter_feed_status(source_id) ON DELETE RESTRICT,\x0a    external_trade_id TEXT NOT NULL,\x0a    latest_effective_revision INTEGER CHECK (latest_effective_revision IS NULL OR latest_effective_revision >= 1),\x0a    original_new_fill_json TEXT,\x0a    baseline_original_evidence_json TEXT,\x0a    held_revisions_json TEXT NOT NULL,\x0a    missing_predecessor_revisions_json TEXT NOT NULL,\x0a    PRIMARY KEY (source_id, external_trade_id)\x0a)\x0d\x00\x00\x00\x00\x02"),
    (16_384, b"\x0a\x00\x00\x00\x00\x02"),
    (16_900, b"EXT NOT NULL,\x0a    unreconciled_notional TEXT NOT NULL,\x0a    locked_settlement_cash TEXT,\x0a    unreconciled_settlement_cash TEXT,\x0a    funding_precision TEXT NOT NULL,\x0a    created_by TEXT NOT NULL,\x0a    confirmed_by_trader_id TEXT,\x0a    created_seq INTEGER NOT NULL CHECK (created_seq >= 0),\x0a    updated_seq INTEGER NOT NULL CHECK (updated_seq >= 0),\x0a    record_json TEXT NOT NULL,\x0a    FOREIGN KEY (account_id, instrument_code)\x0a        REFERENCES blotter_positions(account_id, instrument_code) ON DELETE RESTRICT\x0a)\x0d\x00\x00\x00\x02\x004\x00\x00u\x004\x00,"),
    (17_452, b"\x06\x13\x06\x00\x00\x00\x00\x00?\x12\x06\x17S-\x01\x00indexsqlite_autoindex_blotter_earmarks_1blotter_earmarks!\x87\x00\x11\x07\x17--\x01\x8d3tableblotter_earmarksblotter_earmarks CREATE TABLE blotter_earmarks (\x0a    earmark_id TEXT PRIMARY KEY NOT NULL,\x0a    account_id TEXT NOT NULL,\x0a    instrument_code TEXT NOT NULL,\x0a    earmark_revision INTEGER NOT NULL CHECK (earmark_revision >= 1),\x0a    side TEXT NOT NULL,\x0a    settlement_bucket TEXT NOT NULL,\x0a    settlement_date TEXT NOT NULL,\x0a    state TEXT NOT NULL,\x0a    notional T\x00\x00\x00\x22\x0d\x00\x00\x00\x00\x02"),
    (18_432, b"\x0a\x00\x00\x00\x00\x02"),
    (18_944, b"\x0d\x01\xb7\x00\x04\x00\x1a\x00\x00\xb6\x01j\x00i\x00\x1a\x00g\x01\xb7\x00\x00\x00\x00\x00\x00M\x16\x06\x17a;\x01\x00indexsqlite_autoindex_blotter_pending_matches_1blotter_pending_matches)\x88>\x15\x07\x17;;\x01\x90\x13tableblotter_pending_matchesblotter_pending_matches(CREATE TABL\x00\x00\x00*\x85)\x13\x07\x1799\x01\x89mtableblotter_draft_earmarksblotter_draft_earmarks$CREATE TABLE blotter_draft_earmarks (\x0a    draft_id TEXT PRIMARY KEY NOT NULL,\x0a    account_id TEXT NOT NULL,\x0a    inst\x00\x00\x00'K\x14\x06\x17_9\x01\x00indexsqlite_autoindex_blotter_draft_earmarks_1blotter_draft_earmarks%\x00\x00\x00I\x00\x00\x00\x00?\x12\x06\x17S-\x01\x00indexsqlite_autoindex_blotter_earmarks_1blotter_earmarks!\x00\x00\x00\x00rument_code TEXT NOT NULL,\x0a    draft_revision INTEGER NOT NULL CHECK (draft_revision >= 1),\x0a    status TEXT NOT NULL,\x0a    created_seq INTEGER NOT NULL CHECK (created_seq >= 0),\x0a    confirmation_token BLOB NOT NULL CHECK (length(confirmation_token) = 32),\x0a    context_hash BLOB NOT NULL CHECK (length(context_hash) = 32),\x0a    confirmed_by_trader_id TEXT,\x0a    record_json TEXT,\x0a    FOREIGN KEY (account_id, instrument_code)\x0a        REFERENCES blotter_positions(account_id, instrument_code) ON DELETE RESTRICT\x0a)\x0d\x00\x00\x00\x00\x02"),
    (20_480, b"\x0a\x00\x00\x00\x00\x02"),
    (20_995, b"+E blotter_pending_matches (\x0a    pending_match_id TEXT PRIMARY KEY NOT NULL,\x0a    account_id TEXT NOT NULL,\x0a    instrument_code TEXT NOT NULL,\x0a    pending_revision INTEGER NOT NULL CHECK (pending_revision >= 1),\x0a    source_id TEXT NOT NULL,\x0a    external_trade_id TEXT NOT NULL,\x0a    source_revision INTEGER NOT NULL CHECK (source_revision >= 1),\x0a    state TEXT NOT NULL,\x0a    reason TEXT NOT NULL,\x0a    original_notional TEXT NOT NULL,\x0a    unresolved_notional TEXT NOT NULL,\x0a    unresolved_signed_cash TEXT,\x0a    r\x00\x00\x00\x00esolution_actor_id TEXT,\x0a    resolution_reason TEXT,\x0a    created_seq INTEGER NOT NULL CHECK (created_seq >= 0),\x0a    updated_seq INTEGER NOT NULL CHECK (updated_seq >= 0),\x0a    record_json TEXT NOT NULL,\x0a    FOREIGN KEY (account_id, instrument_code)\x0a        REFERENCES blotter_positions(account_id, instrument_code) ON DELETE RESTRICT,\x0a    FOREIGN KEY (source_id, external_trade_id, source_revision)\x0a        REFERENCES blotter_ingested_trades(source_id, external_trade_id, source_revision) ON DELETE RESTRICT\x0a)\x0d\x00\x00\x00\x00\x02"),
    (22_528, b"\x0a\x00\x00\x00\x00\x02"),
    (23_040, b"\x0d\x01d\x00\x02\x00\x11\x00\x00\x11\x01\x09\x01\xad\x01\xad\x00\x89i\x17\x07\x17GG\x01\x92Qtableblotter_reconcile_allocationsblotter_reconcile_allocations,CREATE TABLE blotter_reconcile_allocations (\x0a    allocation_id TEXT PRIMARY KEY NOT NULL,\x0a    account_id TEXT NOT NULL REFERENCES blotter_accounts(account_id) ON DELETE R\x00\x00\x00/Y\x18\x06\x17mG\x01\x00indexsqlite_autoindex_blotter_reconcile_allocations_1blotter_reconcile_allocations-\x00\x00\x00\x9ca;\x01\x00indexsqlite_autoindex_blotter_pending_matches_1blotter_pending_maI\x1a\x06\x17]7\x01\x00indexsqlite_autoindex_blotter_quote_history_1blotter_quote_history2\x00\x00\x00\x08\x00\x00\x00\x00\x00\x00\x000ESTRICT,\x0a    instrument_code TEXT NOT NULL,\x0a    source_id TEXT NOT NULL,\x0a    external_trade_id TEXT NOT NULL,\x0a    source_revision INTEGER NOT NULL CHECK (source_revision >= 1),\x0a    earmark_id TEXT REFERENCES blotter_earmarks(earmark_id) ON DELETE RESTRICT,\x0a    pending_match_id TEXT REFERENCES blotter_pending_matches(pending_match_id) ON DELETE RESTRICT,\x0a    allocated_notional TEXT NOT NULL,\x0a    allocated_fill_cash TEXT,\x0a    released_earmark_cash TEXT,\x0a    fill_segment_index INTEGER CHECK (fill_segment_i\x00\x00\x00\x00ndex IS NULL OR fill_segment_index >= 1),\x0a    earmark_segment_index INTEGER CHECK (earmark_segment_index IS NULL OR earmark_segment_index >= 1),\x0a    reversed_by_revision INTEGER CHECK (reversed_by_revision IS NULL OR reversed_by_revision >= 1),\x0a    actor_id TEXT NOT NULL,\x0a    baseline_delta_json TEXT,\x0a    record_json TEXT NOT NULL,\x0a    FOREIGN KEY (source_id, external_trade_id, source_revision)\x0a        REFERENCES blotter_ingested_trades(source_id, external_trade_id, source_revision) ON DELETE RESTRICT\x0a)\x0d\x00\x00\x00\x00\x02"),
    (25_088, b"\x0a\x00\x00\x00\x00\x02"),
    (25_600, b"\x0d\x00\x00\x00\x01\x00>\x00\x00>"),
    (25_662, b"\x83?\x19\x07\x1777\x01\x86\x1dtableblotter_quote_historyblotter_quote_history1CREATE TABLE blotter_quote_history (\x0a    source_id TEXT NOT NULL,\x0a    instrument_code TEXT NOT NULL,\x0a    quote_seq INTEGER NOT NULL CHECK (quote_seq >= 1),\x0a    payload_hash BLOB NOT NULL CHECK (length(payload_hash) = 32),\x0a    first_received_at_json TEXT NOT NULL,\x0a    first_ingested_at_json TEXT NOT NULL,\x0a    tick_json TEXT NOT NULL,\x0a    PRIMARY KEY (source_id, instrument_code, quote_seq)\x0a)\x0d\x00\x00\x00\x02\x00\x0c\x00\x01\xb5\x00\x0c\x83&\x1b\x07\x1733\x01\x85stableblotter_quote_headsblotter_quote_heads5CREATE TABLE blotter_quote_heads (\x0a    source_id TEXT NOT NULL,\x0a    instrument_code TEXT NOT NULL,\x0a    head_quote_seq INTEGER NOT NULL CHECK (head_quote_seq >= 1),\x0a    PRIMARY KEY (source_id, instrument_code),\x0a    FOREIGN KEY (source_id, instrument_code, head_quote_seq)\x0a        REFERENCES blotter_quote_history(source_id, instrument_code, quote_seq) ON DELETE RESTRICT\x0a)I\x1a\x06\x17]7\x01\x00indexsqlite_autoindex_blotter_quote_history_1blotter_quote_history2\x0d\x00\x00\x00\x00\x02"),
    (27_136, b"\x0a\x00\x00\x00\x00\x02"),
    (27_648, b"\x0d\x00\x00\x00\x01\x01\xb9\x00\x01\xb9\x01`\x01`"),
    (28_000, b"O\x1e\x06\x17c=\x01\x00indexsqlite_autoindex_blotter_idempotency_keys_1blotter_idempotency_keys9\x00\x00\x00\x08\x00\x00\x00\x00E\x1c\x06\x17Y3\x01\x00indexsqlite_autoindex_blotter_quote_heads_1blotter_quote_heads6\x0d\x00\x00\x00\x00\x02"),
    (28_672, b"\x0a\x00\x00\x00\x00\x02"),
    (29_184, b"\x0d\x00\x00\x00\x01\x009\x00\x009"),
    (29_241, b"\x83D\x1d\x07\x17==\x01\x86\x1btableblotter_idempotency_keysblotter_idempotency_keys8CREATE TABLE blotter_idempotency_keys (\x0a    actor_id TEXT NOT NULL,\x0a    rpc_name TEXT NOT NULL,\x0a    scope_key TEXT NOT NULL,\x0a    idempotency_key TEXT NOT NULL,\x0a    request_hash BLOB NOT NULL CHECK (length(request_hash) = 32),\x0a    committed_seq INTEGER NOT NULL CHECK (committed_seq >= 0),\x0a    response_payload BLOB NOT NULL,\x0a    PRIMARY KEY (actor_id, rpc_name, scope_key, idempotency_key)\x0a)\x0d\x00\x00\x00\x02\x00\xb6\x00\x01\xaf\x00\xb6\x00a\x00a"),
    (29_793, b"K!\x06\x17_9\x01\x00indexsqlite_autoindex_blotter_event_accounts_1blotter_event_accounts>\x00\x00\x00\x08\x00\x00\x00\x00\x81v\x1f\x07\x1777\x01\x83\x0btableblotter_event_journalblotter_event_journal<CREATE TABLE blotter_event_journal (\x0a    seq INTEGER PRIMARY KEY NOT NULL CHECK (seq >= 1),\x0a    occurred_at_json TEXT NOT NULL,\x0a    event_type TEXT NOT NULL,\x0a    envelope_json TEXT NOT NULL\x0a)O\x1e\x06\x17c=\x01\x00indexsqlite_autoindex_blotter_idempotency_keys_1blotter_idempotency_keys9\x0d\x00\x00\x00\x00\x02"),
    (30_720, b"\x0d\x00\x00\x00\x00\x02"),
    (31_232, b"\x0a\x00\x00\x00\x00\x02"),
    (31_744, b"\x0d\x00\x00\x00\x03\x00(\x00\x01\x0b\x00\xbe\x00("),
    (31_784, b"\x81\x13\x22\x07\x17K-\x01\x81;indexidx_blotter_earmarks_acct_stateblotter_earmarks@CREATE INDEX idx_blotter_earmarks_acct_state\x0a    ON blotter_earmarks(account_id, state)K!\x06\x17_9\x01\x00indexsqlite_autoindex_blotter_event_accounts_1blotter_event_accounts>\x81r \x07\x1799\x01\x82\x7ftableblotter_event_accountsblotter_event_accounts=CREATE TABLE blotter_event_accounts (\x0a    seq INTEGER NOT NULL REFERENCES blotter_event_journal(seq) ON DELETE CASCADE,\x0a    account_id TEXT NOT NULL,\x0a    PRIMARY KEY (seq, account_id)\x0a)\x0a\x00\x00\x00\x00\x02"),
    (32_768, b"\x0a\x00\x00\x00\x00\x02"),
    (33_280, b"\x0d\x00\x00\x00\x02\x00\x80\x00\x01^\x00\x80"),
    (33_408, b"\x81[$\x07\x17QG\x01\x82+indexidx_blotter_allocations_acct_tradeblotter_reconcile_allocationsCCREATE INDEX idx_blotter_allocations_acct_trade\x0a    ON blotter_reconcile_allocations(account_id, source_id, external_trade_id, source_revision)\x81\x1f#\x07\x17I;\x01\x81Gindexidx_blotter_pending_acct_stateblotter_pending_matchesACREATE INDEX idx_blotter_pending_acct_state\x0a    ON blotter_pending_matches(account_id, state)\x0a\x00\x00\x00\x00\x02"),
    (34_304, b"\x0a\x00\x00\x00\x00\x02"),
    (34_816, b"\x0d\x00\x00\x00\x01\x01X\x00\x01X"),
    (35_160, b"\x81%%\x07\x17S9\x01\x81Kindexidx_blotter_event_accounts_acct_seqblotter_event_accountsDCREATE INDEX idx_blotter_event_accounts_acct_seq\x0a    ON blotter_event_accounts(account_id, seq)"),
];
