#![allow(clippy::too_many_lines, clippy::similar_names)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::Duration;

use chrono::{NaiveDate, TimeZone};
use chrono_tz::Asia::Shanghai;
use ficant_application::ports::{
    ApplicationResult, BlotterCommitPlan, BlotterEventJournalPort, BlotterPersistencePort,
    FuturesDeliveryRuleParser, IdempotencyScopeKey, ManualBlotterClock, MarketQuoteStreamPort,
    NlpIntentParserPort, PollEventsOutcome, PositionBaseProviderPort, RegulatoryRulePackParser,
    StoredIdempotencyReceipt, TradeFeedReconcilerPort,
};
use ficant_application::use_cases::blotter::{
    AccountDeskPolicy, AnalysisUnits, BlotterEvaluationEngines, ConfirmDraftEarmarkCommand,
    DeskBlotterOrchestrator, DraftTargetState, FullOpeningPositionBase,
    ImportFullOpeningBaseCommand, IngestMarketQuotesInput, InstrumentCatalogEntry, MarketQuoteTick,
    NlpEarmarkProposal, QuoteCursor, RateCoverageEligibility, SourceTextAnchor,
    SubmitDraftEarmarkCommand, SwitchActiveQuoteSourceCommand, pending_missing_trade_revisions,
    validate_source_text_anchors,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_blotter_store::{
    BLOTTER_SCHEMA_VERSION, CsvMarketQuoteStream, CsvPositionBaseProvider, CsvTable,
    CsvTemplateNlpIntentParser, CsvTradeFeedReconciler, MockMarketQuoteStream, MockNlpIntentParser,
    MockPositionBaseProvider, MockTradeFeedReconciler, REQUIRED_TABLES, SqliteBlotterOpenOptions,
    SqliteBlotterStore, StoreFaultInjection, WriteExecutionOutcome,
    default_reference_analysis_units, format_fixed_decimal, parse_fixed_decimal,
};
use ficant_domain::analytics::{
    CalendarBinding, CalendarRequirement, FixedDecimal, MARKET_TIMEZONE,
};
use ficant_domain::blotter::{
    BlotterConfig, BlotterIdGenerator, ConfirmHoldCommand, CreateCommittedCommand,
    CreateHoldCommand, DeterministicIdGenerator, EarmarkState, IngestTradeFeedInput,
    OpeningInstrumentBalance, OpeningPositionBase, PendingMatchReason,
    PendingMatchResolutionAction, PendingMatchState, QuoteBasis, QuoteTerms,
    ResolvePendingMatchInput, SetFeedControlCommand, SettlementBucket, TradeFeedRecordAction,
    TradeFillRecord, TradeSide,
};
use ficant_domain::constraint::{ConstraintDefinition, ConstraintMeasure, ConstraintSet};
use ficant_domain::futures_delivery::{CgbFuturesProduct, FuturesDeliveryRule};
use ficant_domain::market::{
    Bond, BondBusinessDayConvention, BondCouponFrequency, BondDayCountConvention, BondPricingTerms,
    BondTaxAttributes, IncomeTaxStatus, Instrument, InstrumentInput, InstrumentKind,
    MarketRulePack, MarketRulePackInput, PriceSourceType, RulePackContent, ValueAddedTaxStatus,
    VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_fixed_income_native::{NativeBondAnalyticsEngine, NativeFuturesHedgeEngine};
use rusqlite::{Connection, OpenFlags, params};
use tempfile::TempDir;

const TEST_REG_MARKET: &str = "TEST-REG-MKT";
const TEST_REG_TYPE: &str = "regulatory";
const TEST_REG_TYPE_URL: &str = "type.googleapis.com/ficant.test.RegulatoryRulePack";

struct StaticRegulatoryParser;
impl RegulatoryRulePackParser for StaticRegulatoryParser {
    fn market(&self) -> &'static str {
        TEST_REG_MARKET
    }

    fn rule_type(&self) -> &'static str {
        TEST_REG_TYPE
    }

    fn type_url(&self) -> &'static str {
        TEST_REG_TYPE_URL
    }

    fn parse(
        &self,
        content: &RulePackContent,
        rule_pack: VersionRef,
    ) -> ApplicationResult<ConstraintSet> {
        if content.type_url() != TEST_REG_TYPE_URL {
            return Err(ApplicationError::blotter_rule_pack_invalid());
        }
        let def = ConstraintDefinition::new("MAX_INST_FACE", ConstraintMeasure::InstrumentFace)
            .map_err(|_| ApplicationError::blotter_rule_pack_invalid())?;
        ConstraintSet::new(rule_pack, vec![def])
            .map_err(|_| ApplicationError::blotter_rule_pack_invalid())
    }
}

struct UnusedDeliveryParser;
impl FuturesDeliveryRuleParser for UnusedDeliveryParser {
    fn market(&self) -> &'static str {
        "TEST-FUT-MKT"
    }

    fn rule_type(&self) -> &'static str {
        "delivery"
    }

    fn type_url(&self) -> &'static str {
        "type.googleapis.com/ficant.test.DeliveryRulePack"
    }

    fn parse(
        &self,
        _content: &RulePackContent,
        _product: CgbFuturesProduct,
    ) -> ApplicationResult<FuturesDeliveryRule> {
        Err(ApplicationError::new(
            ApplicationErrorCategory::ValidationFailed,
            false,
        ))
    }
}

fn ulid_pair(a: char, b: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F{a}{b}")).unwrap()
}

fn fixed(units: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(units * 1_000_000_000_000)
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn shanghai_time(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> MarketTime {
    let dt = date(year, month, day)
        .and_hms_opt(hour, minute, second)
        .unwrap();
    let utc = Shanghai.from_local_datetime(&dt).single().unwrap().to_utc();
    MarketTime::new(utc, MARKET_TIMEZONE, date(year, month, day)).unwrap()
}

fn test_units() -> AnalysisUnits {
    default_reference_analysis_units().unwrap()
}

fn test_owner() -> OwnerRef {
    OwnerRef::new(ulid_pair('B', '1'), ulid_pair('B', '2'))
}

fn test_bond(code: &str, units: &AnalysisUnits) -> Bond {
    let inst = Instrument::new(InstrumentInput {
        instrument_id: ulid_pair('C', '1'),
        version: Version::new(1).unwrap(),
        owner: test_owner(),
        symbol: code.to_owned(),
        kind: InstrumentKind::Bond,
        market: "TEST-BOND-MKT".to_owned(),
        currency: units.currency_amount.clone(),
        calendar: VersionRef::new(ulid_pair('D', '1'), Version::new(1).unwrap()),
    })
    .unwrap();
    let face_val = DecimalValue::new("100", 0, units.currency_amount.clone()).unwrap();
    let issued = DecimalValue::new("50000000000", 0, units.currency_amount.clone()).unwrap();
    let attrs = BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt);
    let coupon = DecimalValue::new("250", 4, units.rate.clone()).unwrap();
    let pricing = BondPricingTerms::new(
        coupon,
        BondCouponFrequency::Semiannual,
        BondDayCountConvention::ActActBondIsma,
        BondBusinessDayConvention::Following,
    )
    .unwrap();
    Bond::with_issuance(
        &inst,
        date(2024, 1, 15),
        date(2024, 1, 15),
        date(2034, 1, 15),
        issued,
        attrs,
        face_val,
    )
    .unwrap()
    .with_pricing_terms(pricing)
    .unwrap()
}

fn test_calendar() -> CalendarBinding {
    CalendarBinding::new(
        ulid_pair('D', '1').as_str(),
        Version::new(1).unwrap(),
        ContentHash::digest(b"cal:2025"),
        date(2025, 1, 1),
        date(2035, 12, 31),
        Vec::new(),
        Vec::new(),
    )
    .unwrap()
}

fn test_regulatory_pack() -> MarketRulePack {
    let payload = b"regulatory-pack-v1".to_vec();
    let hash = ContentHash::digest(&payload);
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: ulid_pair('R', '1'),
            version: Version::new(1).unwrap(),
            owner: test_owner(),
            market: TEST_REG_MARKET.to_owned(),
            rule_type: TEST_REG_TYPE.to_owned(),
            source: "desk-policy".to_owned(),
            effective: EffectivePeriod::new(
                shanghai_time(2025, 1, 1, 0, 0, 0),
                shanghai_time(2027, 1, 1, 0, 0, 0),
            )
            .unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: hash,
        },
        RulePackContent::new(TEST_REG_TYPE_URL, payload).unwrap(),
    )
    .unwrap()
}

fn test_policy(default_quote_source_id: &str) -> AccountDeskPolicy {
    let mut limit_ceilings = BTreeMap::new();
    limit_ceilings.insert("MAX_INST_FACE".to_owned(), fixed(20_000_000_000));
    AccountDeskPolicy {
        policy_version: 1,
        default_quote_source_id: default_quote_source_id.to_owned(),
        instrument_quote_sources: BTreeMap::new(),
        quote_staleness_seconds: 600,
        default_draft_hold_ttl_seconds: 1_800,
        allowed_hedge_product_codes: vec!["PROD_A".to_owned(), "PROD_B".to_owned()],
        regulatory_rule_pack: Some(test_regulatory_pack()),
        limit_ceilings,
    }
}

fn make_opening_base(
    account_id: &str,
    base_ulid: Ulid,
    source_id: &str,
    instrument_code: &str,
    opening_avail: FixedDecimal,
    source_start_after_feed_seq: u64,
) -> FullOpeningPositionBase {
    let units = test_units();
    FullOpeningPositionBase {
        core: OpeningPositionBase {
            account_id: account_id.to_owned(),
            base_id: base_ulid,
            trading_date: date(2025, 6, 16),
            next_settlement_date: date(2025, 6, 17),
            effective_from: shanghai_time(2025, 6, 16, 8, 30, 0),
            watermark_time: shanghai_time(2025, 6, 16, 9, 0, 0),
            watermark_source_seq: Some(source_start_after_feed_seq),
            source_id: source_id.to_owned(),
            opening_cash_t0: fixed(500_000_000),
            opening_net_cash_delta_t1: fixed(100_000_000),
            balances: vec![OpeningInstrumentBalance {
                instrument_code: instrument_code.to_owned(),
                opening_economic_notional: opening_avail,
                opening_available_notional: opening_avail,
                opening_net_t0_notional: FixedDecimal::ZERO,
                opening_net_t1_notional: FixedDecimal::ZERO,
            }],
            instrument_codes: vec![instrument_code.to_owned()],
            baseline_evidence: Vec::new(),
            source_start_after_feed_seq,
        },
        instruments: vec![InstrumentCatalogEntry {
            instrument_code: instrument_code.to_owned(),
            bond: Some(test_bond(instrument_code, &units)),
            eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
            calendar_requirement: Some(CalendarRequirement::ExactMarket),
        }],
        calendar: Some(test_calendar()),
        default_calendar_requirement: Some(CalendarRequirement::ReferenceReplay),
        units,
    }
}

fn orchestrator(clock: &ManualBlotterClock) -> DeskBlotterOrchestrator<'_> {
    DeskBlotterOrchestrator::new(
        clock,
        BlotterEvaluationEngines {
            bond_engine: &NativeBondAnalyticsEngine,
            hedge_engine: &NativeFuturesHedgeEngine,
            delivery_rule_parser: &UnusedDeliveryParser,
            regulatory_rule_parser: &StaticRegulatoryParser,
        },
    )
}

fn import_account_into_store(
    store: &SqliteBlotterStore,
    orch: &DeskBlotterOrchestrator<'_>,
    actor_id: &str,
    idem_key: &str,
    base: FullOpeningPositionBase,
    policy: AccountDeskPolicy,
) -> ApplicationResult<WriteExecutionOutcome<u64>> {
    let scope_key = IdempotencyScopeKey {
        actor_id: actor_id.to_owned(),
        rpc_name: "ImportOpeningBase".to_owned(),
        scope_key: base.core.account_id.clone(),
        idempotency_key: idem_key.to_owned(),
    };
    let req_hash = base.core.canonical_hash();
    let cmd = ImportFullOpeningBaseCommand {
        actor_id: actor_id.to_owned(),
        idempotency_key: idem_key.to_owned(),
        opening_base: base,
        expected_account_version: 0,
        policy,
    };
    store.execute_write(Some((&scope_key, &req_hash)), |ws| {
        let (next, out, event) = orch.import_opening_base(ws, &cmd)?;
        let receipt = StoredIdempotencyReceipt {
            key: scope_key.clone(),
            request_hash: req_hash.clone(),
            committed_seq: out.committed_seq,
            response_payload: out.committed_seq.to_be_bytes().to_vec(),
        };
        Ok((
            out.committed_seq,
            BlotterCommitPlan {
                next_state: next,
                event,
                idempotency_receipt: Some(receipt),
            },
        ))
    })
}

// ============================================================================
// 1. Production path anti-spoofing, real WAL PRAGMAs, business schema version
// ============================================================================

#[test]
fn production_open_rejects_all_memory_uris_empty_dirs_and_contradictory_flags() {
    let dir = TempDir::new().unwrap();
    let config = BlotterConfig::default();

    let forbidden_paths = [
        ":memory:",
        ":MEMORY:",
        ":temp:",
        "file::memory:",
        "file::memory:?cache=shared",
        "file:blotter.db?mode=memory",
        "FILE:blotter.db?mode=memory&cache=shared",
        "file:///tmp/blotter.db",
        "blotter.db?mode=memory",
        "",
        "   ",
        " blotter.db ",
    ];
    for bad in forbidden_paths {
        let err = SqliteBlotterStore::open_file(bad, config.clone()).unwrap_err();
        assert_eq!(
            err.category(),
            ApplicationErrorCategory::ValidationFailed,
            "path '{bad}' must be rejected"
        );
    }

    // Existing directory must be rejected.
    let dir_err = SqliteBlotterStore::open_file(dir.path(), config.clone()).unwrap_err();
    assert_eq!(
        dir_err.category(),
        ApplicationErrorCategory::ValidationFailed
    );

    // Contradictory open options must be rejected.
    let both_set = SqliteBlotterOpenOptions {
        path: Some(dir.path().join("contradictory.db")),
        ephemeral_in_memory: true,
        config: config.clone(),
        event_retention_window: 100,
    };
    assert_eq!(
        SqliteBlotterStore::open(&both_set).unwrap_err().category(),
        ApplicationErrorCategory::ValidationFailed
    );

    let neither_set = SqliteBlotterOpenOptions {
        path: None,
        ephemeral_in_memory: false,
        config,
        event_retention_window: 100,
    };
    assert_eq!(
        SqliteBlotterStore::open(&neither_set)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ValidationFailed
    );
}

#[test]
fn real_wal_pragmas_and_explicit_business_schema_version_fail_closed() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("desk_blotter.sqlite");
    let config = BlotterConfig::default();

    {
        let store = SqliteBlotterStore::open_file(&db_path, config.clone()).unwrap();
        let pragmas = store.inspect_pragmas().unwrap();
        assert_eq!(pragmas.journal_mode, "wal");
        assert_eq!(pragmas.locking_mode, "exclusive");
        assert_eq!(pragmas.synchronous, 2);
        assert_eq!(pragmas.foreign_keys, 1);
        assert_eq!(pragmas.business_schema_version, BLOTTER_SCHEMA_VERSION);
    }

    // Verify every required business table exists in sqlite_master.
    {
        let raw = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
        let mut stmt = raw
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap();
        let tables: BTreeSet<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for &req in REQUIRED_TABLES {
            assert!(tables.contains(req), "missing required table {req}");
        }
    }

    // Reopening a valid DB succeeds.
    {
        let store = SqliteBlotterStore::open_file(&db_path, config.clone()).unwrap();
        assert_eq!(
            store.inspect_pragmas().unwrap().business_schema_version,
            BLOTTER_SCHEMA_VERSION
        );
    }

    // 1. Tamper blotter_meta.schema_version = 99 (SQLite PRAGMA schema_version untouched).
    {
        let raw = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
        raw.execute(
            "UPDATE blotter_meta SET schema_version = 99 WHERE singleton_id = 1",
            [],
        )
        .unwrap();
    }
    assert_eq!(
        SqliteBlotterStore::open_file(&db_path, config.clone())
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::StorageUnavailable
    );

    // 2. Delete blotter_meta singleton row -> fail closed.
    {
        let raw = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
        raw.execute("DELETE FROM blotter_meta", []).unwrap();
    }
    assert_eq!(
        SqliteBlotterStore::open_file(&db_path, config.clone())
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::StorageUnavailable
    );

    // 3. Restore valid meta row, then drop one required business table -> fail closed.
    {
        let raw = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
        let cfg_json = ficant_blotter_store::codec::config_to_json(&config).unwrap();
        raw.execute(
            "INSERT INTO blotter_meta (singleton_id, schema_version, head_seq, oldest_retained_seq, config_json)
             VALUES (1, 1, 0, 1, ?1)",
            [cfg_json],
        )
        .unwrap();
        raw.execute("DROP TABLE blotter_event_accounts", [])
            .unwrap();
    }
    assert_eq!(
        SqliteBlotterStore::open_file(&db_path, config.clone())
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::StorageUnavailable
    );

    // 4. Explicit ephemeral in-memory test entrypoint executes identical DDL and works end-to-end.
    let mem_store = SqliteBlotterStore::open_ephemeral_in_memory(config).unwrap();
    assert_eq!(
        mem_store.inspect_pragmas().unwrap().business_schema_version,
        BLOTTER_SCHEMA_VERSION
    );
    assert_eq!(mem_store.head_seq().unwrap(), 0);
}

// ============================================================================
// 2. Dual-instance and real second OS process exclusive WAL lock
// ============================================================================

#[test]
fn dual_instance_and_dual_os_process_enforce_exclusive_wal_lock() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("exclusive.sqlite");
    let config = BlotterConfig::default();

    let first = SqliteBlotterStore::open_file(&db_path, config.clone()).unwrap();

    // Second in-process instance must fail immediately (busy_timeout = 0).
    let second_err = SqliteBlotterStore::open_file(&db_path, config.clone()).unwrap_err();
    assert_eq!(
        second_err.category(),
        ApplicationErrorCategory::StorageUnavailable
    );

    // Second OS process probing the exact same WAL file must also fail immediately.
    let exe = std::env::current_exe().unwrap();
    let status = Command::new(&exe)
        .args(["--exact", "exclusive_lock_child_probe", "--nocapture"])
        .env("FICANT_BLOTTER_LOCK_PROBE_PATH", &db_path)
        .env("FICANT_BLOTTER_LOCK_PROBE_EXPECT", "locked")
        .status()
        .unwrap();
    assert!(
        status.success(),
        "second OS process probe did not observe exclusive lock"
    );

    // After releasing the first instance, another OS process can open the WAL cleanly.
    drop(first);
    let status_unlocked = Command::new(&exe)
        .args(["--exact", "exclusive_lock_child_probe", "--nocapture"])
        .env("FICANT_BLOTTER_LOCK_PROBE_PATH", &db_path)
        .env("FICANT_BLOTTER_LOCK_PROBE_EXPECT", "unlocked")
        .status()
        .unwrap();
    assert!(
        status_unlocked.success(),
        "second OS process could not open WAL after parent released lock"
    );
}

#[test]
fn exclusive_lock_child_probe() {
    let Ok(path_str) = std::env::var("FICANT_BLOTTER_LOCK_PROBE_PATH") else {
        return;
    };
    let expect = std::env::var("FICANT_BLOTTER_LOCK_PROBE_EXPECT").unwrap();
    let path = PathBuf::from(path_str);
    let res = SqliteBlotterStore::open_file(&path, BlotterConfig::default());
    match expect.as_str() {
        "locked" => {
            let err = res.expect_err("second OS process must fail on exclusive WAL lock");
            assert_eq!(err.category(), ApplicationErrorCategory::StorageUnavailable);
        }
        "unlocked" => {
            let store = res.expect("second OS process should open released WAL");
            assert_eq!(
                store.inspect_pragmas().unwrap().business_schema_version,
                BLOTTER_SCHEMA_VERSION
            );
        }
        other => panic!("unexpected probe expectation: {other}"),
    }
}

// ============================================================================
// 3. One-time opening base import & 6-state ImmutableViolation
// ============================================================================

#[test]
fn opening_base_import_once_and_immutable_violation_across_six_account_states() {
    let dir = TempDir::new().unwrap();
    let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 16, 9, 30, 0));
    let orch = orchestrator(&clock);
    let mut id_gen = DeterministicIdGenerator::default();

    // Helper: create fresh DB, import acct once, verify same-key replay + different-payload HashMismatch,
    // advance account to the desired state, reopen WAL, and assert new-key re-import -> ImmutableViolation.
    let mut assert_reimport_blocked = |db_name: &str,
                                       advance: &mut dyn FnMut(
        &SqliteBlotterStore,
        &DeskBlotterOrchestrator<'_>,
        &mut DeterministicIdGenerator,
    )| {
        let db_path = dir.path().join(db_name);
        let base = make_opening_base(
            "ACCT-IMMUT",
            ulid_pair('M', '1'),
            "FEED-A",
            "BOND_001",
            fixed(100_000_000),
            10,
        );
        let policy = test_policy("QSRC-A");

        {
            let store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
            let first = import_account_into_store(
                &store,
                &orch,
                "trader-1",
                "import-k1",
                base.clone(),
                policy.clone(),
            )
            .unwrap();
            let seq = match first {
                WriteExecutionOutcome::Committed { committed_seq, .. } => committed_seq,
                WriteExecutionOutcome::Replayed(_) => panic!("expected first commit"),
            };

            // Same actor + same key + same payload -> replay original receipt.
            let replayed = import_account_into_store(
                &store,
                &orch,
                "trader-1",
                "import-k1",
                base.clone(),
                policy.clone(),
            )
            .unwrap();
            match replayed {
                WriteExecutionOutcome::Replayed(r) => assert_eq!(r.committed_seq, seq),
                WriteExecutionOutcome::Committed { .. } => panic!("expected idempotent replay"),
            }

            // Same actor + same key + mutated payload -> HashMismatch.
            let mut mutated = base.clone();
            mutated.core.opening_cash_t0 = fixed(999_000_000);
            let hash_err = import_account_into_store(
                &store,
                &orch,
                "trader-1",
                "import-k1",
                mutated,
                policy.clone(),
            )
            .unwrap_err();
            assert_eq!(hash_err.category(), ApplicationErrorCategory::HashMismatch);

            advance(&store, &orch, &mut id_gen);
        }

        // Reopen WAL and attempt re-import with a NEW idempotency key -> ImmutableViolation.
        let reopened = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
        let mut new_base = base;
        new_base.core.base_id = ulid_pair('M', '9');
        let err = import_account_into_store(
            &reopened,
            &orch,
            "trader-1",
            "import-k2-new",
            new_base,
            policy,
        )
        .unwrap_err();
        assert_eq!(
            err.category(),
            ApplicationErrorCategory::ImmutableViolation,
            "scenario {db_name} must reject re-import with ImmutableViolation"
        );
    };

    // State 1: only original import receipt exists.
    assert_reimport_blocked("state1_receipt_only.sqlite", &mut |_, _, _| {});

    // State 2: only a pending DraftEarmarkRecord exists.
    assert_reimport_blocked("state2_draft_only.sqlite", &mut |store, orch, id_gen| {
        let raw_text = "BUY T+0 INST=BOND_001 NOTIONAL=10000000 CLEAN=100.00";
        let draft_id = id_gen.next_ulid();
        let cmd = SubmitDraftEarmarkCommand {
            account_id: "ACCT-IMMUT".to_owned(),
            draft_id,
            proposal: NlpEarmarkProposal {
                parser_source_id: "parser-1".to_owned(),
                model_version: "1.0".to_owned(),
                raw_source_text: raw_text.to_owned(),
                anchors: vec![SourceTextAnchor {
                    field_name: "side".to_owned(),
                    byte_start: 0,
                    byte_end: 3,
                    span_text: "BUY".to_owned(),
                }],
                confidence_score: FixedDecimal::from_scaled(900_000_000_000),
                account_id: "ACCT-IMMUT".to_owned(),
                instrument_code: "BOND_001".to_owned(),
                side: TradeSide::Buy,
                settlement_bucket: SettlementBucket::TPlus0,
                notional: Some(fixed(10_000_000)),
                quote_terms: Some(QuoteTerms {
                    basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                    settlement_amount: None,
                }),
                suggested_target_state: DraftTargetState::Hold,
            },
        };
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.submit_draft_earmark(ws, &cmd)?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: Some(ev),
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();
    });

    // State 3: active HOLD exists.
    assert_reimport_blocked("state3_hold.sqlite", &mut |store, orch, id_gen| {
        let cmd = CreateHoldCommand {
            account_id: "ACCT-IMMUT".to_owned(),
            earmark_id: id_gen.next_ulid(),
            expected_position_version: 1,
            instrument_code: "BOND_001".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: fixed(10_000_000),
            quote_terms: None,
            ttl_seconds: 1_800,
            note: "hold".to_owned(),
            trader_id: "trader-1".to_owned(),
        };
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.create_hold(ws, &cmd)?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: Some(ev),
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();
    });

    // State 4: active COMMITTED exists.
    assert_reimport_blocked("state4_committed.sqlite", &mut |store, orch, id_gen| {
        let cmd = CreateCommittedCommand {
            account_id: "ACCT-IMMUT".to_owned(),
            earmark_id: id_gen.next_ulid(),
            expected_position_version: 1,
            instrument_code: "BOND_001".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: fixed(20_000_000),
            quote_terms: QuoteTerms {
                basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                settlement_amount: Some(fixed(20_100_000)),
            },
            exact_dirty_price_input: None,
            note: "committed".to_owned(),
            trader_id: "trader-1".to_owned(),
        };
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.create_committed(ws, &cmd)?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: Some(ev),
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();
    });

    // State 5: OPEN PendingMatchRecord exists.
    assert_reimport_blocked("state5_open_pending.sqlite", &mut |store, orch, id_gen| {
        let fill = TradeFillRecord {
            source_id: "FEED-A".to_owned(),
            external_trade_id: "TRD-ORPHAN-1".to_owned(),
            source_revision: 1,
            feed_seq: 11,
            action: TradeFeedRecordAction::NewFill,
            account_id: "ACCT-IMMUT".to_owned(),
            instrument_code: "BOND_001".to_owned(),
            side: TradeSide::Sell,
            settlement_date: date(2025, 6, 16),
            notional: fixed(15_000_000),
            quote_terms: Some(QuoteTerms {
                basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                settlement_amount: Some(fixed(15_000_000)),
            }),
            executed_at: shanghai_time(2025, 6, 16, 9, 25, 0),
            batch_first_feed_seq: 11,
            batch_last_feed_seq: 11,
            exact_dirty_price_input: None,
        };
        let cmd = IngestTradeFeedInput {
            source_id: "FEED-A".to_owned(),
            fills: vec![fill],
            heartbeat_at: None,
            actor_id: "feed-adapter".to_owned(),
        };
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.ingest_trade_feed(ws, &cmd, id_gen)?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: ev,
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();
    });

    // State 6: terminal object + late trade revision exists.
    assert_reimport_blocked(
        "state6_terminal_and_late_rev.sqlite",
        &mut |store, orch, id_gen| {
            let em_id = id_gen.next_ulid();
            let create = CreateCommittedCommand {
                account_id: "ACCT-IMMUT".to_owned(),
                earmark_id: em_id,
                expected_position_version: 1,
                instrument_code: "BOND_001".to_owned(),
                side: TradeSide::Sell,
                settlement_bucket: SettlementBucket::TPlus0,
                notional: fixed(10_000_000),
                quote_terms: QuoteTerms {
                    basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                    settlement_amount: Some(fixed(10_000_000)),
                },
                exact_dirty_price_input: None,
                note: "to-reconcile".to_owned(),
                trader_id: "trader-1".to_owned(),
            };
            store
                .execute_write(None, |ws| {
                    let (next, out, ev) = orch.create_committed(ws, &create)?;
                    Ok((
                        out,
                        BlotterCommitPlan {
                            next_state: next,
                            event: Some(ev),
                            idempotency_receipt: None,
                        },
                    ))
                })
                .unwrap();

            // Reconcile via rev1 then cancel via rev2 (late revision leaving terminal objects).
            let rev1 = TradeFillRecord {
                source_id: "FEED-A".to_owned(),
                external_trade_id: "TRD-LATE-1".to_owned(),
                source_revision: 1,
                feed_seq: 11,
                action: TradeFeedRecordAction::NewFill,
                account_id: "ACCT-IMMUT".to_owned(),
                instrument_code: "BOND_001".to_owned(),
                side: TradeSide::Sell,
                settlement_date: date(2025, 6, 16),
                notional: fixed(10_000_000),
                quote_terms: Some(QuoteTerms {
                    basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                    settlement_amount: Some(fixed(10_000_000)),
                }),
                executed_at: shanghai_time(2025, 6, 16, 9, 25, 0),
                batch_first_feed_seq: 11,
                batch_last_feed_seq: 11,
                exact_dirty_price_input: None,
            };
            let rev2 = TradeFillRecord {
                source_revision: 2,
                feed_seq: 12,
                action: TradeFeedRecordAction::CancelReversal,
                notional: FixedDecimal::ZERO,
                quote_terms: None,
                batch_first_feed_seq: 12,
                batch_last_feed_seq: 12,
                ..rev1.clone()
            };
            for fill in [rev1, rev2] {
                let cmd = IngestTradeFeedInput {
                    source_id: "FEED-A".to_owned(),
                    fills: vec![fill],
                    heartbeat_at: None,
                    actor_id: "feed-adapter".to_owned(),
                };
                store
                    .execute_write(None, |ws| {
                        let (next, out, ev) = orch.ingest_trade_feed(ws, &cmd, id_gen)?;
                        Ok((
                            out,
                            BlotterCommitPlan {
                                next_state: next,
                                event: ev,
                                idempotency_receipt: None,
                            },
                        ))
                    })
                    .unwrap();
            }
        },
    );
}

// ============================================================================
// 4. 8-thread Barrier CAS race on last 10M
// ============================================================================

#[test]
fn eight_thread_barrier_cas_race_allows_exactly_one_winner() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("cas_race.sqlite");
    let store =
        Arc::new(SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap());
    let clock = Arc::new(ManualBlotterClock::new(shanghai_time(
        2025, 6, 16, 9, 30, 0,
    )));

    {
        let orch = orchestrator(&clock);
        import_account_into_store(
            &store,
            &orch,
            "trader-1",
            "imp-1",
            make_opening_base(
                "ACCT-RACE",
                ulid_pair('R', '2'),
                "FEED-R",
                "BOND_001",
                fixed(10_000_000),
                0,
            ),
            test_policy("QSRC-A"),
        )
        .unwrap();
    }
    let head_before = store.head_seq().unwrap();
    assert_eq!(head_before, 1);

    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();

    for idx in 0..8_u8 {
        let store_c = Arc::clone(&store);
        let clock_c = Arc::clone(&clock);
        let barrier_c = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let orch = orchestrator(&clock_c);
            let earmark_id = Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F9{idx}")).unwrap();
            let cmd = CreateHoldCommand {
                account_id: "ACCT-RACE".to_owned(),
                earmark_id,
                expected_position_version: 1,
                instrument_code: "BOND_001".to_owned(),
                side: TradeSide::Sell,
                settlement_bucket: SettlementBucket::TPlus0,
                notional: fixed(10_000_000),
                quote_terms: None,
                ttl_seconds: 1_800,
                note: format!("race-{idx}"),
                trader_id: format!("trader-{idx}"),
            };
            barrier_c.wait();
            store_c.execute_write(None, |ws| {
                let (next, out, ev) = orch.create_hold(ws, &cmd)?;
                Ok((
                    out.committed_seq,
                    BlotterCommitPlan {
                        next_state: next,
                        event: Some(ev),
                        idempotency_receipt: None,
                    },
                ))
            })
        }));
    }

    let mut winners = 0_usize;
    let mut conflicts = 0_usize;
    for h in handles {
        match h.join().unwrap() {
            Ok(WriteExecutionOutcome::Committed { .. }) => winners += 1,
            Ok(WriteExecutionOutcome::Replayed(_)) => panic!("unexpected replay"),
            Err(err) => {
                assert_eq!(
                    err.category(),
                    ApplicationErrorCategory::ConcurrencyConflict
                );
                conflicts += 1;
            }
        }
    }

    assert_eq!(winners, 1);
    assert_eq!(conflicts, 7);
    assert_eq!(store.head_seq().unwrap(), head_before + 1);

    // Reopen WAL and verify exact persisted state.
    let expected_ws = store.load_workspace_state().unwrap();
    drop(store);
    let reopened = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
    assert_eq!(reopened.head_seq().unwrap(), 2);
    assert_eq!(reopened.load_workspace_state().unwrap(), expected_ws);
}

// ============================================================================
// 5. SQLite pre-commit failure rollback, AfterSqliteCommitBeforeMemoryApply
//    crash recovery, idempotency replay/HASH_MISMATCH, and ConfirmHold EXPIRED
// ============================================================================

#[test]
fn fault_injection_rollback_post_commit_crash_recovery_and_expired_confirm_receipt() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("fault_recovery.sqlite");
    let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 16, 9, 30, 0));
    let orch = orchestrator(&clock);
    let mut id_gen = DeterministicIdGenerator::default();

    let store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
    import_account_into_store(
        &store,
        &orch,
        "trader-1",
        "imp-1",
        make_opening_base(
            "ACCT-FAULT",
            ulid_pair('F', '1'),
            "FEED-F",
            "BOND_001",
            fixed(100_000_000),
            0,
        ),
        test_policy("QSRC-A"),
    )
    .unwrap();
    let state_before_fail = store.load_workspace_state().unwrap();

    // 1. FailBeforeSqliteCommit -> clean rollback, sanitized StorageUnavailable, zero memory drift.
    let fail_key = IdempotencyScopeKey {
        actor_id: "trader-1".to_owned(),
        rpc_name: "CreateHold".to_owned(),
        scope_key: "ACCT-FAULT".to_owned(),
        idempotency_key: "hold-rollback-1".to_owned(),
    };
    let fail_hash = ContentHash::digest(b"create-hold-rollback-1");
    let hold_cmd_1 = CreateHoldCommand {
        account_id: "ACCT-FAULT".to_owned(),
        earmark_id: id_gen.next_ulid(),
        expected_position_version: 1,
        instrument_code: "BOND_001".to_owned(),
        side: TradeSide::Sell,
        settlement_bucket: SettlementBucket::TPlus0,
        notional: fixed(25_000_000),
        quote_terms: None,
        ttl_seconds: 1_800,
        note: "will-rollback".to_owned(),
        trader_id: "trader-1".to_owned(),
    };
    store.inject_next_fault(StoreFaultInjection::FailBeforeSqliteCommit);
    let fail_err = store
        .execute_write(Some((&fail_key, &fail_hash)), |ws| {
            let (next, out, ev) = orch.create_hold(ws, &hold_cmd_1)?;
            Ok((
                out.committed_seq,
                BlotterCommitPlan {
                    next_state: next,
                    event: Some(ev),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: fail_key.clone(),
                        request_hash: fail_hash.clone(),
                        committed_seq: out.committed_seq,
                        response_payload: b"rolled-back".to_vec(),
                    }),
                },
            ))
        })
        .unwrap_err();
    assert_eq!(
        fail_err.category(),
        ApplicationErrorCategory::StorageUnavailable
    );
    assert!(!store.is_halted());
    assert_eq!(store.head_seq().unwrap(), 1);
    assert_eq!(store.load_workspace_state().unwrap(), state_before_fail);
    assert!(store.lookup_idempotency(&fail_key).unwrap().is_none());

    // 2. AfterSqliteCommitBeforeMemoryApply -> WAL commits, store halts immediately,
    //    reopen recovers full state and replays same key (even with stale CAS expected_position_version=1).
    let crash_key = IdempotencyScopeKey {
        actor_id: "trader-1".to_owned(),
        rpc_name: "CreateHold".to_owned(),
        scope_key: "ACCT-FAULT".to_owned(),
        idempotency_key: "hold-crash-1".to_owned(),
    };
    let crash_hash = ContentHash::digest(b"create-hold-crash-1");
    let hold_id = id_gen.next_ulid();
    let hold_cmd_crash = CreateHoldCommand {
        account_id: "ACCT-FAULT".to_owned(),
        earmark_id: hold_id.clone(),
        expected_position_version: 1,
        instrument_code: "BOND_001".to_owned(),
        side: TradeSide::Sell,
        settlement_bucket: SettlementBucket::TPlus0,
        notional: fixed(30_000_000),
        quote_terms: None,
        ttl_seconds: 1_800,
        note: "committed-before-crash".to_owned(),
        trader_id: "trader-1".to_owned(),
    };

    store.inject_next_fault(StoreFaultInjection::AfterSqliteCommitBeforeMemoryApply);
    let crash_err = store
        .execute_write(Some((&crash_key, &crash_hash)), |ws| {
            let (next, out, ev) = orch.create_hold(ws, &hold_cmd_crash)?;
            Ok((
                out.committed_seq,
                BlotterCommitPlan {
                    next_state: next,
                    event: Some(ev),
                    idempotency_receipt: Some(StoredIdempotencyReceipt {
                        key: crash_key.clone(),
                        request_hash: crash_hash.clone(),
                        committed_seq: out.committed_seq,
                        response_payload: hold_id.as_str().as_bytes().to_vec(),
                    }),
                },
            ))
        })
        .unwrap_err();
    assert_eq!(
        crash_err.category(),
        ApplicationErrorCategory::StorageUnavailable
    );
    assert!(store.is_halted());
    // Halted instance refuses all requests until reopened from WAL.
    assert_eq!(
        store.load_workspace_state().unwrap_err().category(),
        ApplicationErrorCategory::StorageUnavailable
    );
    drop(store);

    // Reopen WAL -> recovered at seq=2 with hold persisted.
    let recovered = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
    assert_eq!(recovered.head_seq().unwrap(), 2);
    assert!(
        recovered
            .load_workspace_state()
            .unwrap()
            .domain
            .accounts
            .get("ACCT-FAULT")
            .unwrap()
            .earmarks
            .contains_key(&hold_id)
    );

    // Same actor retries same key + same hash (carrying old CAS expected_position_version=1!)
    // -> returns Replayed(receipt) with original committed_seq=2 without incrementing head_seq.
    let replay = recovered
        .execute_write(Some((&crash_key, &crash_hash)), |ws| {
            let (next, out, ev) = orch.create_hold(ws, &hold_cmd_crash)?;
            Ok((
                out.committed_seq,
                BlotterCommitPlan {
                    next_state: next,
                    event: Some(ev),
                    idempotency_receipt: None,
                },
            ))
        })
        .unwrap();
    match replay {
        WriteExecutionOutcome::Replayed(r) => {
            assert_eq!(r.committed_seq, 2);
            assert_eq!(r.response_payload, hold_id.as_str().as_bytes());
        }
        WriteExecutionOutcome::Committed { .. } => panic!("expected replayed receipt"),
    }
    assert_eq!(recovered.head_seq().unwrap(), 2);

    // Same key with mutated payload hash -> HASH_MISMATCH.
    let wrong_hash = ContentHash::digest(b"mutated-payload");
    let mismatch_err = recovered
        .execute_write::<(), _>(Some((&crash_key, &wrong_hash)), |_| {
            panic!("closure must not run on idempotency HASH_MISMATCH")
        })
        .unwrap_err();
    assert_eq!(
        mismatch_err.category(),
        ApplicationErrorCategory::HashMismatch
    );

    // 3. Committed lazy-expiration rejection receipt on ConfirmHold survives WAL restart.
    // Pause feed so expired hold re-validation fails and commits EXPIRED + reconfirmation_error.
    let pause_cmd = SetFeedControlCommand {
        source_id: "FEED-F".to_owned(),
        expected_feed_status_revision: 1,
        paused_by_operator: true,
        force_clear_gap_pause: false,
        reason: "maintenance".to_owned(),
    };
    recovered
        .execute_write(None, |ws| {
            let (next, out, ev) = orch.set_feed_control(ws, &pause_cmd)?;
            Ok((
                out,
                BlotterCommitPlan {
                    next_state: next,
                    event: Some(ev),
                    idempotency_receipt: None,
                },
            ))
        })
        .unwrap();

    // Advance clock to exact expires_at (1800s -> expired).
    clock.advance_seconds(1_800).unwrap();

    let confirm_key = IdempotencyScopeKey {
        actor_id: "trader-1".to_owned(),
        rpc_name: "ConfirmHold".to_owned(),
        scope_key: "ACCT-FAULT".to_owned(),
        idempotency_key: "confirm-expired-1".to_owned(),
    };
    let confirm_hash = ContentHash::digest(b"confirm-expired-1");
    let confirm_cmd = ConfirmHoldCommand {
        account_id: "ACCT-FAULT".to_owned(),
        earmark_id: hold_id.clone(),
        expected_earmark_revision: 1,
        expected_position_version: 2,
        quote_terms: QuoteTerms {
            basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
            settlement_amount: Some(fixed(30_000_000)),
        },
        exact_dirty_price_input: None,
        trader_id: "trader-1".to_owned(),
    };
    let confirm_outcome = recovered
        .execute_write(Some((&confirm_key, &confirm_hash)), |ws| {
            let (next, out, ev) = orch.confirm_hold(ws, &confirm_cmd)?;
            assert_eq!(out.earmark.state, EarmarkState::Expired);
            assert!(out.reconfirmation_error.is_some());
            let receipt = StoredIdempotencyReceipt {
                key: confirm_key.clone(),
                request_hash: confirm_hash.clone(),
                committed_seq: out.committed_seq,
                response_payload: b"EXPIRED:FEED_PAUSED".to_vec(),
            };
            Ok((
                out,
                BlotterCommitPlan {
                    next_state: next,
                    event: Some(ev),
                    idempotency_receipt: Some(receipt),
                },
            ))
        })
        .unwrap();
    let expired_seq = match confirm_outcome {
        WriteExecutionOutcome::Committed { committed_seq, .. } => committed_seq,
        WriteExecutionOutcome::Replayed(_) => panic!("expected commit"),
    };
    drop(recovered);

    let reopened2 = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
    let persisted_em = reopened2
        .load_workspace_state()
        .unwrap()
        .domain
        .accounts
        .get("ACCT-FAULT")
        .unwrap()
        .earmarks
        .get(&hold_id)
        .unwrap()
        .clone();
    assert_eq!(persisted_em.state, EarmarkState::Expired);
    assert_eq!(persisted_em.earmark_revision, 2);

    let replayed_confirm = reopened2
        .execute_write::<(), _>(Some((&confirm_key, &confirm_hash)), |_| {
            panic!("should replay committed expiration rejection")
        })
        .unwrap();
    match replayed_confirm {
        WriteExecutionOutcome::Replayed(r) => {
            assert_eq!(r.committed_seq, expired_seq);
            assert_eq!(r.response_payload, b"EXPIRED:FEED_PAUSED");
        }
        WriteExecutionOutcome::Committed { .. } => panic!("expected replay"),
    }
}

// ============================================================================
// 6. Mid-flight WAL restart equivalence: 10->12->11, rev2->rev1,
//    10B 4-slice allocation & reversal, quote source switch + retention fallback
// ============================================================================

#[test]
fn restart_equivalence_gap_10_12_11_and_rev2_rev1_with_backfill() {
    let dir = TempDir::new().unwrap();
    let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 16, 10, 0, 0));
    let orch = orchestrator(&clock);

    let make_fill = |seq: u64,
                     trade_id: &str,
                     rev: u64,
                     action: TradeFeedRecordAction,
                     notional: FixedDecimal,
                     cash: FixedDecimal| TradeFillRecord {
        source_id: "FEED-SEQ".to_owned(),
        external_trade_id: trade_id.to_owned(),
        source_revision: rev,
        feed_seq: seq,
        action,
        account_id: "ACCT-GAP".to_owned(),
        instrument_code: "BOND_001".to_owned(),
        side: TradeSide::Sell,
        settlement_date: date(2025, 6, 16),
        notional,
        quote_terms: Some(QuoteTerms {
            basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
            settlement_amount: Some(cash),
        }),
        executed_at: shanghai_time(2025, 6, 16, 9, 45, 0),
        batch_first_feed_seq: seq,
        batch_last_feed_seq: seq,
        exact_dirty_price_input: None,
    };

    let f10 = make_fill(
        10,
        "TRD-10",
        1,
        TradeFeedRecordAction::NewFill,
        fixed(10_000_000),
        fixed(10_050_000),
    );
    let f11 = make_fill(
        11,
        "TRD-11",
        1,
        TradeFeedRecordAction::NewFill,
        fixed(20_000_000),
        fixed(20_100_000),
    );
    let f12 = make_fill(
        12,
        "TRD-11",
        2,
        TradeFeedRecordAction::Amend,
        fixed(15_000_000),
        fixed(15_075_000),
    );

    // Reference: sequential 10 -> 11 -> 12 without restart.
    let ref_store = SqliteBlotterStore::open_ephemeral_in_memory(BlotterConfig::default()).unwrap();
    import_account_into_store(
        &ref_store,
        &orch,
        "trader-1",
        "imp",
        make_opening_base(
            "ACCT-GAP",
            ulid_pair('G', '1'),
            "FEED-SEQ",
            "BOND_001",
            fixed(100_000_000),
            9,
        ),
        test_policy("QSRC-A"),
    )
    .unwrap();
    let mut ref_gen = DeterministicIdGenerator::default();
    for fill in [f10.clone(), f11.clone(), f12.clone()] {
        let cmd = IngestTradeFeedInput {
            source_id: "FEED-SEQ".to_owned(),
            fills: vec![fill],
            heartbeat_at: None,
            actor_id: "adapter".to_owned(),
        };
        ref_store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.ingest_trade_feed(ws, &cmd, &mut ref_gen)?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: ev,
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();
    }

    // Test WAL: 10 -> 12 -> CLOSE & REOPEN WAL -> 11.
    let gap_db = dir.path().join("gap_10_12_11.sqlite");
    let mut test_gen = DeterministicIdGenerator::default();
    {
        let store = SqliteBlotterStore::open_file(&gap_db, BlotterConfig::default()).unwrap();
        import_account_into_store(
            &store,
            &orch,
            "trader-1",
            "imp",
            make_opening_base(
                "ACCT-GAP",
                ulid_pair('G', '1'),
                "FEED-SEQ",
                "BOND_001",
                fixed(100_000_000),
                9,
            ),
            test_policy("QSRC-A"),
        )
        .unwrap();
        for fill in [f10, f12] {
            let cmd = IngestTradeFeedInput {
                source_id: "FEED-SEQ".to_owned(),
                fills: vec![fill],
                heartbeat_at: None,
                actor_id: "adapter".to_owned(),
            };
            store
                .execute_write(None, |ws| {
                    let (next, out, ev) = orch.ingest_trade_feed(ws, &cmd, &mut test_gen)?;
                    Ok((
                        out,
                        BlotterCommitPlan {
                            next_state: next,
                            event: ev,
                            idempotency_receipt: None,
                        },
                    ))
                })
                .unwrap();
        }
    }

    // Reopen WAL: contiguous cursor MUST still be 10 with gap [11, 11] and paused_by_gap = true.
    let reopened = SqliteBlotterStore::open_file(&gap_db, BlotterConfig::default()).unwrap();
    let mid_ws = reopened.load_workspace_state().unwrap();
    let mid_feed = mid_ws.domain.feeds.get("FEED-SEQ").unwrap();
    assert_eq!(mid_feed.contiguous_feed_seq, 10);
    assert_eq!(mid_feed.max_seen_feed_seq, 12);
    assert!(mid_feed.paused_by_gap);
    assert_eq!(mid_feed.missing_feed_ranges.len(), 1);
    assert_eq!(mid_feed.missing_feed_ranges[0].first, 11);
    assert_eq!(mid_feed.missing_feed_ranges[0].last, 11);

    // Now deliver 11 -> releases 11 and 12 in source batch order.
    let cmd11 = IngestTradeFeedInput {
        source_id: "FEED-SEQ".to_owned(),
        fills: vec![f11],
        heartbeat_at: None,
        actor_id: "adapter".to_owned(),
    };
    reopened
        .execute_write(None, |ws| {
            let (next, out, ev) = orch.ingest_trade_feed(ws, &cmd11, &mut test_gen)?;
            Ok((
                out,
                BlotterCommitPlan {
                    next_state: next,
                    event: ev,
                    idempotency_receipt: None,
                },
            ))
        })
        .unwrap();

    let ref_snap = orch
        .get_account_snapshot(&ref_store.load_workspace_state().unwrap(), "ACCT-GAP")
        .unwrap();
    let gap_snap = orch
        .get_account_snapshot(&reopened.load_workspace_state().unwrap(), "ACCT-GAP")
        .unwrap();
    assert_eq!(
        gap_snap.positions[0].position.economic_net_notional,
        ref_snap.positions[0].position.economic_net_notional
    );
    assert_eq!(
        gap_snap.positions[0].position.available_t0_notional,
        ref_snap.positions[0].position.available_t0_notional
    );
    assert_eq!(
        gap_snap.funding_summary.pending_match_cash_t0,
        ref_snap.funding_summary.pending_match_cash_t0
    );
    assert_eq!(
        gap_snap.funding_summary.pending_match_count,
        ref_snap.funding_summary.pending_match_count
    );
    assert_eq!(gap_snap.feed_statuses[0].contiguous_feed_seq, 12);
    assert!(!gap_snap.feed_statuses[0].paused_by_gap);

    // Part B: rev2 -> restart -> FetchTradeRevision NotFound keeps REVISION_HOLD -> populate rev1 -> backfill.
    let rev_db = dir.path().join("rev2_rev1.sqlite");
    let rev1 = TradeFillRecord {
        source_id: "FEED-REV".to_owned(),
        external_trade_id: "TRD-REV-1".to_owned(),
        source_revision: 1,
        feed_seq: 1,
        action: TradeFeedRecordAction::NewFill,
        account_id: "ACCT-REV".to_owned(),
        instrument_code: "BOND_001".to_owned(),
        side: TradeSide::Sell,
        settlement_date: date(2025, 6, 16),
        notional: fixed(10_000_000),
        quote_terms: Some(QuoteTerms {
            basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
            settlement_amount: Some(fixed(10_000_000)),
        }),
        executed_at: shanghai_time(2025, 6, 16, 9, 40, 0),
        batch_first_feed_seq: 1,
        batch_last_feed_seq: 1,
        exact_dirty_price_input: None,
    };
    let rev2 = TradeFillRecord {
        source_revision: 2,
        feed_seq: 11,
        action: TradeFeedRecordAction::Amend,
        notional: fixed(8_000_000),
        quote_terms: Some(QuoteTerms {
            basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
            settlement_amount: Some(fixed(8_000_000)),
        }),
        batch_first_feed_seq: 11,
        batch_last_feed_seq: 11,
        ..rev1.clone()
    };

    {
        let store = SqliteBlotterStore::open_file(&rev_db, BlotterConfig::default()).unwrap();
        import_account_into_store(
            &store,
            &orch,
            "trader-1",
            "imp",
            make_opening_base(
                "ACCT-REV",
                ulid_pair('V', '1'),
                "FEED-REV",
                "BOND_001",
                fixed(50_000_000),
                10,
            ),
            test_policy("QSRC-A"),
        )
        .unwrap();
        let cmd = IngestTradeFeedInput {
            source_id: "FEED-REV".to_owned(),
            fills: vec![rev2],
            heartbeat_at: None,
            actor_id: "adapter".to_owned(),
        };
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.ingest_trade_feed(ws, &cmd, &mut test_gen)?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: ev,
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();
    }

    // Reopen WAL: missing predecessor rev1 MUST persist across restart.
    let rev_reopened = SqliteBlotterStore::open_file(&rev_db, BlotterConfig::default()).unwrap();
    let missing =
        pending_missing_trade_revisions(&rev_reopened.load_workspace_state().unwrap(), "FEED-REV");
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].source_revision, 1);

    // Backfill with empty reconciler -> NotFound keeps REVISION_HOLD frozen.
    let reconciler = MockTradeFeedReconciler::new();
    let (_, bf_empty, _) = orch
        .backfill_missing_trade_revisions(
            &rev_reopened.load_workspace_state().unwrap(),
            "FEED-REV",
            "adapter",
            &mut test_gen,
            &reconciler,
        )
        .unwrap();
    assert_eq!(bf_empty.fetched_count, 0);
    assert_eq!(bf_empty.not_found_revisions.len(), 1);

    // Now populate historical rev1 in MockTradeFeedReconciler and backfill.
    reconciler.insert_historical_revision(rev1).unwrap();
    rev_reopened
        .execute_write(None, |ws| {
            let (next, out, ev) = orch.backfill_missing_trade_revisions(
                ws,
                "FEED-REV",
                "adapter",
                &mut test_gen,
                &reconciler,
            )?;
            assert_eq!(out.fetched_count, 1);
            assert!(out.not_found_revisions.is_empty());
            Ok((
                out,
                BlotterCommitPlan {
                    next_state: next,
                    event: ev,
                    idempotency_receipt: None,
                },
            ))
        })
        .unwrap();
    assert!(
        pending_missing_trade_revisions(&rev_reopened.load_workspace_state().unwrap(), "FEED-REV")
            .is_empty()
    );
}

#[test]
fn ten_billion_four_slice_allocation_and_reversal_survives_wal_restarts() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("ten_billion_4slice.sqlite");
    let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 16, 10, 0, 0));
    let orch = orchestrator(&clock);
    let mut id_gen = DeterministicIdGenerator::default();

    // 10 billion notional split into 4 earmarks: 1B + 2B + 3B + 4B = 10B.
    let slice_notionals = [
        fixed(1_000_000_000),
        fixed(2_000_000_000),
        fixed(3_000_000_000),
        fixed(4_000_000_000),
    ];
    let total_fill_notional = fixed(10_000_000_000);
    // Non-integral settlement cash: 9_999_999_997.123456789013
    let total_fill_cash = parse_fixed_decimal("9999999997.123456789013").unwrap();

    let mut earmark_ids = Vec::new();
    let pending_id: Ulid;

    {
        let store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
        import_account_into_store(
            &store,
            &orch,
            "trader-1",
            "imp",
            make_opening_base(
                "ACCT-10B",
                ulid_pair('T', '1'),
                "FEED-10B",
                "BOND_001",
                fixed(15_000_000_000),
                0,
            ),
            test_policy("QSRC-A"),
        )
        .unwrap();

        for (i, &n) in slice_notionals.iter().enumerate() {
            let em_id = id_gen.next_ulid();
            let expected_pos_ver = (i as u64) + 1;
            // Distinct non-integral cash on each earmark.
            let em_cash = FixedDecimal::from_scaled(n.scaled() + 333_333_333_333);
            let cmd = CreateCommittedCommand {
                account_id: "ACCT-10B".to_owned(),
                earmark_id: em_id.clone(),
                expected_position_version: expected_pos_ver,
                instrument_code: "BOND_001".to_owned(),
                side: TradeSide::Sell,
                settlement_bucket: SettlementBucket::TPlus0,
                notional: n,
                quote_terms: QuoteTerms {
                    basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                    settlement_amount: Some(em_cash),
                },
                exact_dirty_price_input: None,
                note: format!("slice-{i}"),
                trader_id: "trader-1".to_owned(),
            };
            store
                .execute_write(None, |ws| {
                    let (next, out, ev) = orch.create_committed(ws, &cmd)?;
                    Ok((
                        out,
                        BlotterCommitPlan {
                            next_state: next,
                            event: Some(ev),
                            idempotency_receipt: None,
                        },
                    ))
                })
                .unwrap();
            earmark_ids.push(em_id);
        }

        // Ingest 10B fill -> lands in OPEN pending match (4 candidate earmarks).
        let fill10b = TradeFillRecord {
            source_id: "FEED-10B".to_owned(),
            external_trade_id: "TRD-10B-1".to_owned(),
            source_revision: 1,
            feed_seq: 1,
            action: TradeFeedRecordAction::NewFill,
            account_id: "ACCT-10B".to_owned(),
            instrument_code: "BOND_001".to_owned(),
            side: TradeSide::Sell,
            settlement_date: date(2025, 6, 16),
            notional: total_fill_notional,
            quote_terms: Some(QuoteTerms {
                basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                settlement_amount: Some(total_fill_cash),
            }),
            executed_at: shanghai_time(2025, 6, 16, 9, 50, 0),
            batch_first_feed_seq: 1,
            batch_last_feed_seq: 1,
            exact_dirty_price_input: None,
        };
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.ingest_trade_feed(
                    ws,
                    &IngestTradeFeedInput {
                        source_id: "FEED-10B".to_owned(),
                        fills: vec![fill10b],
                        heartbeat_at: None,
                        actor_id: "adapter".to_owned(),
                    },
                    &mut id_gen,
                )?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: ev,
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();

        let ws = store.load_workspace_state().unwrap();
        let pm = ws
            .domain
            .accounts
            .get("ACCT-10B")
            .unwrap()
            .pending_matches
            .values()
            .next()
            .unwrap();
        assert_eq!(pm.state, PendingMatchState::Open);
        assert_eq!(pm.reason, PendingMatchReason::AmbiguousCandidates);
        pending_id = pm.pending_match_id.clone();

        // Apply first 2 slices before WAL restart.
        for i in 0..2 {
            let cur_ws = store.load_workspace_state().unwrap();
            let acct = cur_ws.domain.accounts.get("ACCT-10B").unwrap();
            let pm_rev = acct
                .pending_matches
                .get(&pending_id)
                .unwrap()
                .pending_revision;
            let pos_ver = acct.positions.get("BOND_001").unwrap().position_version;
            let em_rev = acct.earmarks.get(&earmark_ids[i]).unwrap().earmark_revision;
            let resolve = ResolvePendingMatchInput {
                account_id: "ACCT-10B".to_owned(),
                pending_match_id: pending_id.clone(),
                expected_pending_revision: pm_rev,
                expected_position_version: pos_ver,
                action: PendingMatchResolutionAction::SplitOffset,
                expected_earmark_revision: Some(em_rev),
                target_earmark_id: Some(earmark_ids[i].clone()),
                offset_notional: Some(slice_notionals[i]),
                reason: format!("split-{i}"),
                baseline_delta: None,
                actor_id: "trader-1".to_owned(),
            };
            store
                .execute_write(None, |ws| {
                    let (next, out, ev) = orch.resolve_pending_match(ws, &resolve, &mut id_gen)?;
                    Ok((
                        out,
                        BlotterCommitPlan {
                            next_state: next,
                            event: Some(ev),
                            idempotency_receipt: None,
                        },
                    ))
                })
                .unwrap();
        }
    }

    // Restart WAL mid-split and apply remaining 2 slices (slices 2 & 3).
    let mut sum_alloc_notional = FixedDecimal::ZERO;
    let mut sum_alloc_fill_cash = FixedDecimal::ZERO;
    {
        let store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
        for i in 2..4 {
            let cur_ws = store.load_workspace_state().unwrap();
            let acct = cur_ws.domain.accounts.get("ACCT-10B").unwrap();
            let pm_rev = acct
                .pending_matches
                .get(&pending_id)
                .unwrap()
                .pending_revision;
            let pos_ver = acct.positions.get("BOND_001").unwrap().position_version;
            let em_rev = acct.earmarks.get(&earmark_ids[i]).unwrap().earmark_revision;
            let resolve = ResolvePendingMatchInput {
                account_id: "ACCT-10B".to_owned(),
                pending_match_id: pending_id.clone(),
                expected_pending_revision: pm_rev,
                expected_position_version: pos_ver,
                action: PendingMatchResolutionAction::SplitOffset,
                expected_earmark_revision: Some(em_rev),
                target_earmark_id: Some(earmark_ids[i].clone()),
                offset_notional: Some(slice_notionals[i]),
                reason: format!("split-{i}"),
                baseline_delta: None,
                actor_id: "trader-1".to_owned(),
            };
            store
                .execute_write(None, |ws| {
                    let (next, out, ev) = orch.resolve_pending_match(ws, &resolve, &mut id_gen)?;
                    Ok((
                        out,
                        BlotterCommitPlan {
                            next_state: next,
                            event: Some(ev),
                            idempotency_receipt: None,
                        },
                    ))
                })
                .unwrap();
        }

        let ws = store.load_workspace_state().unwrap();
        let acct = ws.domain.accounts.get("ACCT-10B").unwrap();
        assert_eq!(acct.allocations.len(), 4);
        for alloc in acct.allocations.values() {
            sum_alloc_notional = sum_alloc_notional
                .checked_add(alloc.allocated_notional)
                .unwrap();
            let fc = alloc.allocated_fill_cash.unwrap();
            assert!(fc.is_positive(), "sell cash slice must be positive");
            sum_alloc_fill_cash = sum_alloc_fill_cash.checked_add(fc).unwrap();
        }
        assert_eq!(sum_alloc_notional, total_fill_notional);
        assert_eq!(sum_alloc_fill_cash, total_fill_cash);
        for em_id in &earmark_ids {
            assert_eq!(
                acct.earmarks.get(em_id).unwrap().state,
                EarmarkState::Reconciled
            );
        }
    }

    // Restart WAL again and ingest CANCEL_REVERSAL (rev 2) -> reverses all 4 allocations exactly.
    {
        let store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
        let cancel_rev2 = TradeFillRecord {
            source_id: "FEED-10B".to_owned(),
            external_trade_id: "TRD-10B-1".to_owned(),
            source_revision: 2,
            feed_seq: 2,
            action: TradeFeedRecordAction::CancelReversal,
            account_id: "ACCT-10B".to_owned(),
            instrument_code: "BOND_001".to_owned(),
            side: TradeSide::Sell,
            settlement_date: date(2025, 6, 16),
            notional: FixedDecimal::ZERO,
            quote_terms: None,
            executed_at: shanghai_time(2025, 6, 16, 10, 5, 0),
            batch_first_feed_seq: 2,
            batch_last_feed_seq: 2,
            exact_dirty_price_input: None,
        };
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.ingest_trade_feed(
                    ws,
                    &IngestTradeFeedInput {
                        source_id: "FEED-10B".to_owned(),
                        fills: vec![cancel_rev2],
                        heartbeat_at: None,
                        actor_id: "adapter".to_owned(),
                    },
                    &mut id_gen,
                )?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: ev,
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();
    }

    // Final WAL reopen: verify all 4 earmarks restored to exact original notional & cash.
    let final_store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
    let final_ws = final_store.load_workspace_state().unwrap();
    let acct = final_ws.domain.accounts.get("ACCT-10B").unwrap();
    for (i, em_id) in earmark_ids.iter().enumerate() {
        let em = acct.earmarks.get(em_id).unwrap();
        assert_eq!(em.state, EarmarkState::Committed);
        assert!(em.reopened_by_reversal);
        assert_eq!(em.unreconciled_notional, slice_notionals[i]);
        assert_eq!(em.unreconciled_settlement_cash, em.locked_settlement_cash);
    }
    for alloc in acct.allocations.values() {
        assert_eq!(alloc.reversed_by_revision, Some(2));
    }
}

#[test]
fn quote_source_switch_restart_reconciliation_and_event_journal_retention_fallback() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("quotes_and_retention.sqlite");
    let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 16, 10, 0, 0));
    let orch = orchestrator(&clock);

    // Open with a small retention window of 2 events to test journal pruning & fallback.
    {
        let opts = SqliteBlotterOpenOptions::file(&db_path, BlotterConfig::default())
            .with_event_retention_window(2);
        let store = SqliteBlotterStore::open(&opts).unwrap();

        // seq 1: import account A
        import_account_into_store(
            &store,
            &orch,
            "trader-1",
            "imp-a",
            make_opening_base(
                "ACCT-A",
                ulid_pair('Q', '1'),
                "FEED-Q",
                "BOND_001",
                fixed(50_000_000),
                0,
            ),
            test_policy("QSRC-A"),
        )
        .unwrap();

        // seq 2: ingest quote from QSRC-A at quote_seq = 100
        let tick_a = MarketQuoteTick {
            source_id: "QSRC-A".to_owned(),
            quote_seq: 100,
            instrument_code: "BOND_001".to_owned(),
            source_type: PriceSourceType::ActiveQuote,
            bid_clean_price: None,
            ask_clean_price: None,
            mid_clean_price: Some(fixed(100)),
            mid_yield_to_maturity: None,
            observed_at: shanghai_time(2025, 6, 16, 9, 59, 0),
            received_at: shanghai_time(2025, 6, 16, 9, 59, 1),
        };
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.ingest_market_quotes(
                    ws,
                    &IngestMarketQuotesInput {
                        account_id: "ACCT-A".to_owned(),
                        source_id: "QSRC-A".to_owned(),
                        ticks: vec![tick_a],
                    },
                )?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: ev,
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();

        // seq 3: ingest quote from QSRC-B at quote_seq = 1 (coexists independently with QSRC-A 100!)
        let tick_b = MarketQuoteTick {
            source_id: "QSRC-B".to_owned(),
            quote_seq: 1,
            instrument_code: "BOND_001".to_owned(),
            source_type: PriceSourceType::ActiveQuote,
            bid_clean_price: None,
            ask_clean_price: None,
            mid_clean_price: Some(fixed(101)),
            mid_yield_to_maturity: None,
            observed_at: shanghai_time(2025, 6, 16, 9, 59, 30),
            received_at: shanghai_time(2025, 6, 16, 9, 59, 31),
        };
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.ingest_market_quotes(
                    ws,
                    &IngestMarketQuotesInput {
                        account_id: "ACCT-A".to_owned(),
                        source_id: "QSRC-B".to_owned(),
                        ticks: vec![tick_b],
                    },
                )?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: ev,
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();

        // seq 4: switch active quote source to QSRC-B (retention window=2 -> oldest_retained_seq=3)
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.switch_active_quote_source(
                    ws,
                    &SwitchActiveQuoteSourceCommand {
                        account_id: "ACCT-A".to_owned(),
                        instrument_code: Some("BOND_001".to_owned()),
                        new_source_id: "QSRC-B".to_owned(),
                    },
                )?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: ev,
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();
    }

    // Reopen WAL and verify quotes coexistence, active source B, retention fallback, and
    // preservation of original import idempotency receipt despite event pruning!
    let opts = SqliteBlotterOpenOptions::file(&db_path, BlotterConfig::default())
        .with_event_retention_window(2);
    let reopened = SqliteBlotterStore::open(&opts).unwrap();
    assert_eq!(reopened.head_seq().unwrap(), 4);
    assert_eq!(reopened.oldest_retained_seq().unwrap(), 3);

    let ws = reopened.load_workspace_state().unwrap();
    assert_eq!(
        ws.quotes
            .latest_for("QSRC-A", "BOND_001")
            .unwrap()
            .tick
            .quote_seq,
        100
    );
    assert_eq!(
        ws.quotes
            .latest_for("QSRC-B", "BOND_001")
            .unwrap()
            .tick
            .quote_seq,
        1
    );
    let snap = orch.get_account_snapshot(&ws, "ACCT-A").unwrap();
    assert_eq!(snap.positions[0].active_quote_source_id, "QSRC-B");
    assert_eq!(snap.positions[0].active_quote_seq, Some(1));

    // Poll with after_seq = 1 (< oldest_retained_seq - 1 = 2) -> SnapshotFallbackRequired.
    match reopened.poll_events(1, 10, None, None).unwrap() {
        PollEventsOutcome::Fallback(fb) => {
            assert_eq!(fb.requested_after_seq, 1);
            assert_eq!(fb.oldest_retained_seq, 3);
            assert_eq!(fb.current_head_seq, 4);
        }
        PollEventsOutcome::Batch(_) => panic!("expected SnapshotFallbackRequired"),
    }

    // Poll with future cursor after_seq = 5 -> ValidationFailed.
    assert_eq!(
        reopened
            .poll_events(5, 10, None, None)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ValidationFailed
    );

    // Poll with after_seq = 2 -> returns seq 4 (seq 3 was QSRC-B ingest while ACCT-A was still on QSRC-A
    // so seq 3 had empty changes for ACCT-A; scanned_through_seq advances to 4!).
    match reopened.poll_events(2, 10, Some("ACCT-A"), None).unwrap() {
        PollEventsOutcome::Batch(batch) => {
            assert_eq!(batch.current_head_seq, 4);
            assert_eq!(batch.scanned_through_seq, 4);
            assert!(!batch.has_more);
            assert_eq!(batch.events.len(), 1);
            assert_eq!(batch.events[0].seq, 4);
        }
        PollEventsOutcome::Fallback(_) => panic!("expected Batch"),
    }

    // Original import receipt from seq 1 MUST still be preserved (retention trims ONLY journal).
    let imp_key = IdempotencyScopeKey {
        actor_id: "trader-1".to_owned(),
        rpc_name: "ImportOpeningBase".to_owned(),
        scope_key: "ACCT-A".to_owned(),
        idempotency_key: "imp-a".to_owned(),
    };
    assert!(reopened.lookup_idempotency(&imp_key).unwrap().is_some());

    // Startup reconciliation detects policy switch back to QSRC-A and commits seq 5.
    let mut startup_policies = BTreeMap::new();
    startup_policies.insert("ACCT-A".to_owned(), test_policy("QSRC-A"));
    reopened
        .execute_write(None, |ws| {
            let (next, out, ev) = orch.reconcile_startup_quote_sources(ws, &startup_policies)?;
            assert!(out.mutated);
            Ok((
                out,
                BlotterCommitPlan {
                    next_state: next,
                    event: ev,
                    idempotency_receipt: None,
                },
            ))
        })
        .unwrap();
    let after_reconcile = orch
        .get_account_snapshot(&reopened.load_workspace_state().unwrap(), "ACCT-A")
        .unwrap();
    assert_eq!(
        after_reconcile.positions[0].active_quote_source_id,
        "QSRC-A"
    );
    assert_eq!(after_reconcile.positions[0].active_quote_seq, Some(100));
}

// ============================================================================
// 7. Four Open-Source Reference SPI Adapters (CSV RFC-4180 + Mock)
// ============================================================================

#[test]
fn reference_spi_adapters_csv_and_mock_pagination_not_found_and_fail_closed() {
    let units = test_units();

    // --- RFC-4180 CSV parser edge & failure cases ---
    let quoted_csv = "a,b\r\n\"hello, \"\"world\"\"\",\"line1\nline2\"\r\n";
    let parsed = CsvTable::parse(quoted_csv).unwrap();
    assert_eq!(parsed.rows().len(), 1);
    assert_eq!(
        parsed.field(&parsed.rows()[0], "a").unwrap(),
        "hello, \"world\""
    );
    assert_eq!(
        parsed.field(&parsed.rows()[0], "b").unwrap(),
        "line1\nline2"
    );

    // Malformed CSVs fail closed.
    for bad_csv in [
        "",
        "a,a\n1,2\n",             // duplicate header
        "a,b\n1\n",               // ragged row
        "a,b\n\"unclosed,2\n",    // unclosed quote
        "a,b\nfoo\"bar,2\n",      // bare quote in unquoted field
        "a,b\n\"ok\"garbage,2\n", // trailing chars after quote
    ] {
        assert_eq!(
            CsvTable::parse(bad_csv).unwrap_err().category(),
            ApplicationErrorCategory::ValidationFailed
        );
    }

    // --- SPI #1: PositionBaseProviderPort (CSV + Mock) ---
    let accounts_csv = concat!(
        "account_id,base_id,trading_date,next_settlement_date,effective_from_rfc3339,",
        "watermark_time_rfc3339,market_timezone,watermark_source_seq,source_id,",
        "opening_cash_t0,opening_net_cash_delta_t1,source_start_after_feed_seq\n",
        "ACCT-CSV,01ARZ3NDEKTSV4RRFFQ69G5FA9,2025-06-16,2025-06-17,",
        "2025-06-16T00:30:00Z,2025-06-16T01:00:00Z,Asia/Shanghai,10,FEED-CSV,",
        "100000000,20000000,10\n"
    );
    let balances_csv = concat!(
        "account_id,trading_date,instrument_code,opening_economic_notional,",
        "opening_available_notional,opening_net_t0_notional,opening_net_t1_notional,",
        "eligibility_class,calendar_requirement\n",
        "ACCT-CSV,2025-06-16,BOND_001,50000000,50000000,0,0,SOVEREIGN_OR_POLICY_FIXED,EXACT_MARKET\n"
    );
    let csv_pos =
        CsvPositionBaseProvider::from_csv_str(accounts_csv, balances_csv, &units).unwrap();
    let fetched_base = csv_pos
        .fetch_opening_position_base("req-1", "ACCT-CSV", date(2025, 6, 16))
        .unwrap();
    assert_eq!(fetched_base.core.opening_cash_t0, fixed(100_000_000));
    assert_eq!(fetched_base.core.balances.len(), 1);
    assert_eq!(
        csv_pos
            .fetch_opening_position_base("req-2", "UNKNOWN", date(2025, 6, 16))
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::NotFound
    );
    // Bad balance CSV (duplicate instrument) fails closed.
    let dup_balances = format!(
        "{balances_csv}ACCT-CSV,2025-06-16,BOND_001,10,10,0,0,SOVEREIGN_OR_POLICY_FIXED,EXACT_MARKET\n"
    );
    assert_eq!(
        CsvPositionBaseProvider::from_csv_str(accounts_csv, &dup_balances, &units)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ValidationFailed
    );

    let mock_pos = MockPositionBaseProvider::new();
    mock_pos.insert(fetched_base.clone());
    assert_eq!(
        mock_pos
            .fetch_opening_position_base("req-3", "ACCT-CSV", date(2025, 6, 16))
            .unwrap(),
        fetched_base
    );

    // --- SPI #2: TradeFeedReconcilerPort (CSV + Mock, 1..=1024 pagination, gap stop, NotFound) ---
    let trades_csv = concat!(
        "source_id,external_trade_id,source_revision,feed_seq,action,account_id,instrument_code,",
        "side,settlement_date,notional,clean_price_per_100,yield_to_maturity,settlement_amount,",
        "executed_at_rfc3339,market_timezone,batch_first_feed_seq,batch_last_feed_seq\n",
        "FEED-1,T1,1,1,NEW_FILL,ACCT-1,BOND_001,SELL,2025-06-16,10000000,100.25,,10050000,",
        "2025-06-16T01:30:00Z,Asia/Shanghai,1,1\n",
        "FEED-1,T2,1,2,NEW_FILL,ACCT-1,BOND_001,BUY,2025-06-16,5000000,100.10,,5010000,",
        "2025-06-16T01:31:00Z,Asia/Shanghai,2,2\n",
        "FEED-1,T4,1,4,NEW_FILL,ACCT-1,BOND_001,SELL,2025-06-16,8000000,99.90,,8000000,",
        "2025-06-16T01:33:00Z,Asia/Shanghai,4,4\n"
    );
    let csv_feed = CsvTradeFeedReconciler::from_csv_str(trades_csv).unwrap();

    // Reject out-of-range page sizes 0 and 1025.
    assert_eq!(
        csv_feed
            .pull_trade_feed_batch("FEED-1", 0, 0)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ValidationFailed
    );
    assert_eq!(
        csv_feed
            .pull_trade_feed_batch("FEED-1", 0, 1_025)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ValidationFailed
    );

    // Pull page size 1 -> returns seq 1, has_more = true.
    let p1 = csv_feed.pull_trade_feed_batch("FEED-1", 0, 1).unwrap();
    assert_eq!(p1.fills.len(), 1);
    assert_eq!(p1.scanned_through_feed_seq, 1);
    assert!(p1.has_more);

    // Pull from after_feed_seq=1 with max=10 -> returns seq 2 ONLY (stops at missing seq 3 gap!).
    let p2 = csv_feed.pull_trade_feed_batch("FEED-1", 1, 10).unwrap();
    assert_eq!(p2.fills.len(), 1);
    assert_eq!(p2.fills[0].feed_seq, 2);
    assert_eq!(p2.scanned_through_feed_seq, 2);
    assert!(!p2.has_more);

    // Empty page keeps scanned_through_feed_seq unchanged.
    let p3 = csv_feed.pull_trade_feed_batch("FEED-1", 2, 10).unwrap();
    assert!(p3.fills.is_empty());
    assert_eq!(p3.scanned_through_feed_seq, 2);

    // FetchTradeRevision returns authoritative packet or NotFound.
    let rev = csv_feed.fetch_trade_revision("FEED-1", "T4", 1).unwrap();
    assert_eq!(rev.feed_seq, 4);
    assert_eq!(
        csv_feed
            .fetch_trade_revision("FEED-1", "T4", 99)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::NotFound
    );

    // Bad trade CSV (both clean price and yield populated) fails closed.
    let bad_trades_csv = concat!(
        "source_id,external_trade_id,source_revision,feed_seq,action,account_id,instrument_code,",
        "side,settlement_date,notional,clean_price_per_100,yield_to_maturity,settlement_amount,",
        "executed_at_rfc3339,market_timezone,batch_first_feed_seq,batch_last_feed_seq\n",
        "FEED-1,T1,1,1,NEW_FILL,ACCT-1,BOND_001,SELL,2025-06-16,10000000,100.25,0.025,10050000,",
        "2025-06-16T01:30:00Z,Asia/Shanghai,1,1\n"
    );
    assert_eq!(
        CsvTradeFeedReconciler::from_csv_str(bad_trades_csv)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ValidationFailed
    );

    // --- SPI #3: MarketQuoteStreamPort (CSV + Mock, 1..=1024 pagination, pair isolation) ---
    let quotes_csv = concat!(
        "source_id,quote_seq,instrument_code,source_type,bid_clean_price,ask_clean_price,",
        "mid_clean_price,mid_yield_to_maturity,observed_at_rfc3339,received_at_rfc3339,market_timezone\n",
        "QSRC-A,100,BOND_001,ACTIVE_QUOTE,,,100.25,,2025-06-16T01:00:00Z,2025-06-16T01:00:01Z,Asia/Shanghai\n",
        "QSRC-A,101,BOND_001,ACTIVE_QUOTE,,,100.30,,2025-06-16T01:01:00Z,2025-06-16T01:01:01Z,Asia/Shanghai\n",
        "QSRC-B,1,BOND_001,REAL_TRADE,,,101.00,,2025-06-16T01:02:00Z,2025-06-16T01:02:01Z,Asia/Shanghai\n"
    );
    let csv_quotes = CsvMarketQuoteStream::from_csv_str(quotes_csv).unwrap();

    let cur_a = QuoteCursor {
        source_id: "QSRC-A".to_owned(),
        instrument_code: "BOND_001".to_owned(),
        after_quote_seq: 0,
    };
    assert_eq!(
        csv_quotes
            .pull_market_quotes(&cur_a, 0)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ValidationFailed
    );
    assert_eq!(
        csv_quotes
            .pull_market_quotes(&cur_a, 1_025)
            .unwrap_err()
            .category(),
        ApplicationErrorCategory::ValidationFailed
    );

    let qp1 = csv_quotes.pull_market_quotes(&cur_a, 1).unwrap();
    assert_eq!(qp1.ticks.len(), 1);
    assert_eq!(qp1.ticks[0].quote_seq, 100);
    assert_eq!(qp1.next_cursor.source_id, "QSRC-A");
    assert_eq!(qp1.next_cursor.after_quote_seq, 100);
    assert!(qp1.has_more);

    let qp2 = csv_quotes.pull_market_quotes(&qp1.next_cursor, 10).unwrap();
    assert_eq!(qp2.ticks.len(), 1);
    assert_eq!(qp2.ticks[0].quote_seq, 101);
    assert!(!qp2.has_more);

    // Empty page leaves cursor unchanged.
    let qp3 = csv_quotes.pull_market_quotes(&qp2.next_cursor, 10).unwrap();
    assert!(qp3.ticks.is_empty());
    assert_eq!(qp3.next_cursor, qp2.next_cursor);

    // Source B stays strictly isolated from Source A.
    let mock_quotes = MockMarketQuoteStream::new();
    for t in qp1.ticks.into_iter().chain(qp2.ticks) {
        mock_quotes.push_tick(t).unwrap();
    }
    let mq = mock_quotes
        .pull_market_quotes(
            &QuoteCursor {
                source_id: "QSRC-B".to_owned(),
                instrument_code: "BOND_001".to_owned(),
                after_quote_seq: 0,
            },
            10,
        )
        .unwrap();
    assert!(mq.ticks.is_empty());

    // --- SPI #4: NlpIntentParserPort (Mock + CSV Template, UTF-8 half-open anchors) ---
    let nlp_mock = MockNlpIntentParser::default();
    let utf8_text = "备注：BUY T+1 INST=BOND_001 NOTIONAL=25000000 CLEAN=100.125 TARGET=COMMITTED";
    let proposal = nlp_mock
        .parse_nlp_intent("req-nlp-1", "ACCT-1", utf8_text)
        .unwrap();
    assert_eq!(proposal.side, TradeSide::Buy);
    assert_eq!(proposal.settlement_bucket, SettlementBucket::TPlus1);
    assert_eq!(proposal.instrument_code, "BOND_001");
    assert_eq!(proposal.suggested_target_state, DraftTargetState::Committed);
    validate_source_text_anchors(&proposal.raw_source_text, &proposal.anchors).unwrap();
    // Verify multi-byte UTF-8 prefix offset ("备注：" = 9 UTF-8 bytes)
    assert_eq!(proposal.anchors[0].byte_start, 9);
    assert_eq!(proposal.anchors[0].byte_end, 12);

    let nlp_csv = concat!(
        "account_id,raw_source_text,parser_source_id,model_version,confidence_score,",
        "instrument_code,side,settlement_bucket,notional,clean_price_per_100,yield_to_maturity,",
        "settlement_amount,suggested_target_state,anchor_field,anchor_span\n",
        "ACCT-1,\"买入 BOND_001 1000万\",csv-nlp,1.0,0.90,BOND_001,BUY,T_PLUS_0,10000000,100.00,,,HOLD,instrument_code,BOND_001\n"
    );
    let csv_nlp = CsvTemplateNlpIntentParser::from_csv_str(nlp_csv).unwrap();
    let csv_prop = csv_nlp
        .parse_nlp_intent("req-nlp-2", "ACCT-1", "买入 BOND_001 1000万")
        .unwrap();
    validate_source_text_anchors(&csv_prop.raw_source_text, &csv_prop.anchors).unwrap();

    // Canonical fixed decimal formatting round-trip sanity check.
    let edge = parse_fixed_decimal("-10000000000.000000000001").unwrap();
    assert_eq!(format_fixed_decimal(edge), "-10000000000.000000000001");
}

// ============================================================================
// 8. M2 Review Regression Tests ([M-1], [M-2], [B-2] WAL Restart)
// ============================================================================

#[test]
fn execute_write_authorized_runs_before_idempotency_replay_and_serializes_under_writer_lock() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("authorized_write.sqlite");
    let store =
        Arc::new(SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap());
    let clock = Arc::new(ManualBlotterClock::new(shanghai_time(
        2025, 6, 16, 10, 0, 0,
    )));
    let orch = orchestrator(&clock);

    // Import ACCT-A (on QSRC-A) and ACCT-B (on QSRC-B).
    import_account_into_store(
        &store,
        &orch,
        "trader-1",
        "imp-a",
        make_opening_base(
            "ACCT-A",
            ulid_pair('A', '1'),
            "FEED-AUTH",
            "BOND_001",
            fixed(50_000_000),
            0,
        ),
        test_policy("QSRC-A"),
    )
    .unwrap();
    import_account_into_store(
        &store,
        &orch,
        "trader-1",
        "imp-b",
        make_opening_base(
            "ACCT-B",
            ulid_pair('A', '2'),
            "FEED-AUTH",
            "BOND_001",
            fixed(50_000_000),
            0,
        ),
        test_policy("QSRC-B"),
    )
    .unwrap();

    // 1. Commit an idempotent IngestMarketQuotes on QSRC-A via execute_write_authorized.
    let scope_key = IdempotencyScopeKey {
        actor_id: "quote-bot".to_owned(),
        rpc_name: "IngestMarketQuotes".to_owned(),
        scope_key: "QSRC-A".to_owned(),
        idempotency_key: "q-auth-1".to_owned(),
    };
    let req_hash = ContentHash::digest(b"ingest-qsrc-a-seq-1");
    let tick_1 = MarketQuoteTick {
        source_id: "QSRC-A".to_owned(),
        quote_seq: 1,
        instrument_code: "BOND_001".to_owned(),
        source_type: PriceSourceType::ActiveQuote,
        bid_clean_price: None,
        ask_clean_price: None,
        mid_clean_price: Some(fixed(100)),
        mid_yield_to_maturity: None,
        observed_at: shanghai_time(2025, 6, 16, 9, 59, 0),
        received_at: shanghai_time(2025, 6, 16, 9, 59, 1),
    };
    let first_outcome = store
        .execute_write_authorized(
            Some((&scope_key, &req_hash)),
            |ws| {
                // Only ACCT-A uses QSRC-A on BOND_001 -> authorized.
                for (acct_id, meta) in &ws.accounts_meta {
                    if meta.policy.active_quote_source_for("BOND_001") == "QSRC-A"
                        && acct_id != "ACCT-A"
                    {
                        return Err(ApplicationError::new(
                            ApplicationErrorCategory::Forbidden,
                            false,
                        ));
                    }
                }
                Ok(())
            },
            |ws| {
                let (next, out, ev) = orch.ingest_market_quotes(
                    ws,
                    &IngestMarketQuotesInput {
                        account_id: "ACCT-A".to_owned(),
                        source_id: "QSRC-A".to_owned(),
                        ticks: vec![tick_1.clone()],
                    },
                )?;
                let receipt = StoredIdempotencyReceipt {
                    key: scope_key.clone(),
                    request_hash: req_hash.clone(),
                    committed_seq: out.committed_seq,
                    response_payload: b"quote-receipt-1".to_vec(),
                };
                Ok((
                    out.committed_seq,
                    BlotterCommitPlan {
                        next_state: next,
                        event: ev,
                        idempotency_receipt: Some(receipt),
                    },
                ))
            },
        )
        .unwrap();
    let committed_seq = match first_outcome {
        WriteExecutionOutcome::Committed { committed_seq, .. } => committed_seq,
        WriteExecutionOutcome::Replayed(_) => panic!("expected first commit"),
    };
    assert_eq!(committed_seq, 3);

    // 2. Retry the EXACT SAME existing idempotency key + MATCHING hash, but fail authorize.
    //    Authorization MUST run before idempotency lookup/replay and return PermissionDenied.
    let transition_called = AtomicBool::new(false);
    let denied_same_hash = store
        .execute_write_authorized::<(), _, _>(
            Some((&scope_key, &req_hash)),
            |_| {
                Err(ApplicationError::new(
                    ApplicationErrorCategory::Forbidden,
                    false,
                ))
            },
            |_| {
                transition_called.store(true, Ordering::SeqCst);
                unreachable!("transition closure must not run when authorize denies")
            },
        )
        .unwrap_err();
    assert_eq!(
        denied_same_hash.category(),
        ApplicationErrorCategory::Forbidden
    );
    assert!(!transition_called.load(Ordering::SeqCst));

    // 3. Retry the SAME existing idempotency key with a MISMATCHING hash, failing authorize.
    //    Must STILL return PermissionDenied (never HashMismatch), preventing receipt oracle leaks.
    let wrong_hash = ContentHash::digest(b"different-payload");
    let denied_diff_hash = store
        .execute_write_authorized::<(), _, _>(
            Some((&scope_key, &wrong_hash)),
            |_| {
                Err(ApplicationError::new(
                    ApplicationErrorCategory::Forbidden,
                    false,
                ))
            },
            |_| unreachable!("transition closure must not run when authorize denies"),
        )
        .unwrap_err();
    assert_eq!(
        denied_diff_hash.category(),
        ApplicationErrorCategory::Forbidden
    );

    // 4. When authorize succeeds on the cached key + matching hash, cached receipt replays.
    let replayed = store
        .execute_write_authorized::<(), _, _>(
            Some((&scope_key, &req_hash)),
            |_| Ok(()),
            |_| unreachable!("transition closure must not run on idempotency replay"),
        )
        .unwrap();
    match replayed {
        WriteExecutionOutcome::Replayed(r) => {
            assert_eq!(r.committed_seq, committed_seq);
            assert_eq!(r.response_payload, b"quote-receipt-1");
        }
        WriteExecutionOutcome::Committed { .. } => panic!("expected idempotency replay"),
    }

    // 5. Lock serialization & zero TOCTOU race:
    //    Thread 1 acquires the single-writer mutex and switches ACCT-B from QSRC-B -> QSRC-A.
    //    While Thread 1 holds the mutex, Thread 2 enters execute_write_authorized for QSRC-A
    //    (authorized only for ACCT-A). Thread 2's authorize hook MUST block on the writer mutex
    //    and observe the post-switch state (seeing ACCT-B on QSRC-A) with mutual exclusion.
    let active_in_lock = Arc::new(AtomicUsize::new(0));
    let (tx_entered, rx_entered) = mpsc::channel::<()>();

    let store_t1 = Arc::clone(&store);
    let clock_t1 = Arc::clone(&clock);
    let active_t1 = Arc::clone(&active_in_lock);
    let t1 = thread::spawn(move || {
        let orch_t1 = orchestrator(&clock_t1);
        store_t1
            .execute_write_authorized(
                None,
                |_| {
                    assert_eq!(active_t1.fetch_add(1, Ordering::SeqCst), 0);
                    Ok(())
                },
                |ws| {
                    tx_entered.send(()).unwrap();
                    thread::sleep(Duration::from_millis(40));
                    let (next, out, ev) = orch_t1.switch_active_quote_source(
                        ws,
                        &SwitchActiveQuoteSourceCommand {
                            account_id: "ACCT-B".to_owned(),
                            instrument_code: Some("BOND_001".to_owned()),
                            new_source_id: "QSRC-A".to_owned(),
                        },
                    )?;
                    assert_eq!(active_t1.fetch_sub(1, Ordering::SeqCst), 1);
                    Ok((
                        out.committed_seq,
                        BlotterCommitPlan {
                            next_state: next,
                            event: ev,
                            idempotency_receipt: None,
                        },
                    ))
                },
            )
            .unwrap()
    });

    let store_t2 = Arc::clone(&store);
    let active_t2 = Arc::clone(&active_in_lock);
    let t2 = thread::spawn(move || {
        rx_entered.recv().unwrap();
        let observed_acct_b_on_qsrc_a = AtomicBool::new(false);
        let res = store_t2.execute_write_authorized::<(), _, _>(
            Some((&scope_key, &req_hash)),
            |ws| {
                // Mutual exclusion: Thread 1 cannot be inside the lock simultaneously.
                assert_eq!(active_t2.fetch_add(1, Ordering::SeqCst), 0);
                let mut denied = false;
                for (acct_id, meta) in &ws.accounts_meta {
                    if meta.policy.active_quote_source_for("BOND_001") == "QSRC-A"
                        && acct_id != "ACCT-A"
                    {
                        observed_acct_b_on_qsrc_a.store(true, Ordering::SeqCst);
                        denied = true;
                    }
                }
                assert_eq!(active_t2.fetch_sub(1, Ordering::SeqCst), 1);
                if denied {
                    Err(ApplicationError::new(
                        ApplicationErrorCategory::Forbidden,
                        false,
                    ))
                } else {
                    Ok(())
                }
            },
            |_| unreachable!("must not execute or replay after ACCT-B joined QSRC-A"),
        );
        (res, observed_acct_b_on_qsrc_a.load(Ordering::SeqCst))
    });

    match t1.join().unwrap() {
        WriteExecutionOutcome::Committed { committed_seq, .. } => assert_eq!(committed_seq, 4),
        WriteExecutionOutcome::Replayed(_) => panic!("expected switch commit"),
    }
    let (t2_res, saw_b) = t2.join().unwrap();
    assert!(
        saw_b,
        "authorize under writer mutex must see Thread 1's committed switch of ACCT-B to QSRC-A"
    );
    assert_eq!(
        t2_res.unwrap_err().category(),
        ApplicationErrorCategory::Forbidden
    );
    assert_eq!(store.head_seq().unwrap(), 4);
}

#[test]
fn revision_hold_evolution_syncs_pending_match_columns_and_no_event_head_advance_rejected() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("revision_hold_columns.sqlite");
    let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 16, 10, 0, 0));
    let orch = orchestrator(&clock);
    let mut id_gen = DeterministicIdGenerator::default();

    let mut base = make_opening_base(
        "ACCT-RH",
        ulid_pair('H', '1'),
        "FEED-RH",
        "BOND_001",
        fixed(50_000_000),
        0,
    );
    let units = test_units();
    base.core.balances.push(OpeningInstrumentBalance {
        instrument_code: "BOND_002".to_owned(),
        opening_economic_notional: fixed(50_000_000),
        opening_available_notional: fixed(50_000_000),
        opening_net_t0_notional: FixedDecimal::ZERO,
        opening_net_t1_notional: FixedDecimal::ZERO,
    });
    base.core.instrument_codes.push("BOND_002".to_owned());
    base.instruments.push(InstrumentCatalogEntry {
        instrument_code: "BOND_002".to_owned(),
        bond: Some(test_bond("BOND_002", &units)),
        eligibility_class: Some(RateCoverageEligibility::SovereignOrPolicyFixed),
        calendar_requirement: Some(CalendarRequirement::ExactMarket),
    });

    let pending_match_id: Ulid;
    let state_before_reopen;

    {
        let store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
        import_account_into_store(
            &store,
            &orch,
            "trader-1",
            "imp-rh",
            base,
            test_policy("QSRC-A"),
        )
        .unwrap();

        // Create a COMMITTED earmark for 10M BOND_001 so rev 1 reconciles cleanly.
        let em_id = id_gen.next_ulid();
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.create_committed(
                    ws,
                    &CreateCommittedCommand {
                        account_id: "ACCT-RH".to_owned(),
                        earmark_id: em_id,
                        expected_position_version: 1,
                        instrument_code: "BOND_001".to_owned(),
                        side: TradeSide::Sell,
                        settlement_bucket: SettlementBucket::TPlus0,
                        notional: fixed(10_000_000),
                        quote_terms: QuoteTerms {
                            basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                            settlement_amount: Some(fixed(10_000_000)),
                        },
                        exact_dirty_price_input: None,
                        note: "match-rev1".to_owned(),
                        trader_id: "trader-1".to_owned(),
                    },
                )?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: Some(ev),
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();

        let make_fill = |seq: u64,
                         rev: u64,
                         action: TradeFeedRecordAction,
                         inst: &str,
                         notional: FixedDecimal| TradeFillRecord {
            source_id: "FEED-RH".to_owned(),
            external_trade_id: "TRD-RH-1".to_owned(),
            source_revision: rev,
            feed_seq: seq,
            action,
            account_id: "ACCT-RH".to_owned(),
            instrument_code: inst.to_owned(),
            side: TradeSide::Sell,
            settlement_date: date(2025, 6, 16),
            notional,
            quote_terms: Some(QuoteTerms {
                basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                settlement_amount: Some(notional),
            }),
            executed_at: shanghai_time(2025, 6, 16, 9, 50, 0),
            batch_first_feed_seq: seq,
            batch_last_feed_seq: seq,
            exact_dirty_price_input: None,
        };

        // 1. Ingest rev 1 (reconciles cleanly).
        let rev1 = make_fill(
            1,
            1,
            TradeFeedRecordAction::NewFill,
            "BOND_001",
            fixed(10_000_000),
        );
        // 2. Skip rev 2; ingest rev 3 on BOND_001 (12M) -> creates REVISION_HOLD pending match.
        let rev3 = make_fill(
            2,
            3,
            TradeFeedRecordAction::Amend,
            "BOND_001",
            fixed(12_000_000),
        );
        // 3. Still missing rev 2; ingest rev 4 on BOND_002 (18M) -> updates the SAME REVISION_HOLD
        //    pending match in-place (instrument_code, source_revision, original_notional change!).
        let rev4 = make_fill(
            3,
            4,
            TradeFeedRecordAction::Amend,
            "BOND_002",
            fixed(18_000_000),
        );

        for fill in [rev1, rev3, rev4] {
            store
                .execute_write(None, |ws| {
                    let (next, out, ev) = orch.ingest_trade_feed(
                        ws,
                        &IngestTradeFeedInput {
                            source_id: "FEED-RH".to_owned(),
                            fills: vec![fill.clone()],
                            heartbeat_at: None,
                            actor_id: "adapter".to_owned(),
                        },
                        &mut id_gen,
                    )?;
                    Ok((
                        out,
                        BlotterCommitPlan {
                            next_state: next,
                            event: ev,
                            idempotency_receipt: None,
                        },
                    ))
                })
                .unwrap();
        }

        let ws = store.load_workspace_state().unwrap();
        let acct = ws.domain.accounts.get("ACCT-RH").unwrap();
        let open_holds: Vec<_> = acct
            .pending_matches
            .values()
            .filter(|pm| {
                pm.state == PendingMatchState::Open && pm.reason == PendingMatchReason::RevisionHold
            })
            .collect();
        assert_eq!(open_holds.len(), 1);
        let pm = open_holds[0];
        assert_eq!(pm.pending_revision, 2);
        assert_eq!(pm.instrument_code, "BOND_002");
        assert_eq!(pm.source_revision, 4);
        assert_eq!(pm.original_notional, fixed(18_000_000));
        pending_match_id = pm.pending_match_id.clone();

        // Also test [M-2] no-event head_seq invariant: advancing head_seq with event = None
        // must fail closed with ConcurrencyConflict and leave state untouched.
        let head_before = store.head_seq().unwrap();
        let mut bogus_state = ws.clone();
        bogus_state.domain.head_seq = head_before + 1;
        let err = store
            .commit_transaction(&BlotterCommitPlan {
                next_state: bogus_state,
                event: None,
                idempotency_receipt: None,
            })
            .unwrap_err();
        assert_eq!(
            err.category(),
            ApplicationErrorCategory::ConcurrencyConflict
        );
        assert_eq!(store.head_seq().unwrap(), head_before);
        assert_eq!(store.load_workspace_state().unwrap(), ws);

        // No-op commit keeping head_seq unchanged with event = None remains valid.
        let same_head = store
            .commit_transaction(&BlotterCommitPlan {
                next_state: ws.clone(),
                event: None,
                idempotency_receipt: None,
            })
            .unwrap();
        assert_eq!(same_head, head_before);

        state_before_reopen = ws;
    }

    // Inspect raw SQLite WAL normalized columns & foreign keys directly.
    {
        let raw = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let (inst_code, pending_rev, src_rev, orig_notional, state_str, reason_str): (
            String,
            i64,
            i64,
            String,
            String,
            String,
        ) = raw
            .query_row(
                "SELECT instrument_code, pending_revision, source_revision,
                        original_notional, state, reason
                 FROM blotter_pending_matches
                 WHERE pending_match_id = ?1",
                params![pending_match_id.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(inst_code, "BOND_002");
        assert_eq!(pending_rev, 2);
        assert_eq!(src_rev, 4);
        assert_eq!(
            parse_fixed_decimal(&orig_notional).unwrap(),
            fixed(18_000_000)
        );
        assert_eq!(state_str, "OPEN");
        assert_eq!(reason_str, "REVISION_HOLD");

        let mut fk_stmt = raw.prepare("PRAGMA foreign_key_check").unwrap();
        let fk_violations: usize = fk_stmt.query_map([], |_| Ok(())).unwrap().count();
        assert_eq!(fk_violations, 0, "foreign key check must pass cleanly");
    }

    // Reopen WAL and assert exact hydrated state equivalence.
    let reopened = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
    let hydrated = reopened.load_workspace_state().unwrap();
    assert_eq!(hydrated, state_before_reopen);
    let pm_reopened = hydrated
        .domain
        .accounts
        .get("ACCT-RH")
        .unwrap()
        .pending_matches
        .get(&pending_match_id)
        .unwrap();
    assert_eq!(pm_reopened.instrument_code, "BOND_002");
    assert_eq!(pm_reopened.source_revision, 4);
    assert_eq!(pm_reopened.original_notional, fixed(18_000_000));
    assert_eq!(pm_reopened.pending_revision, 2);
}

#[test]
fn wal_restart_reconciles_full_desk_policy_and_invalidates_stale_drafts() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("policy_restart_reconcile.sqlite");
    let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 16, 10, 0, 0));
    let orch = orchestrator(&clock);
    let mut id_gen = DeterministicIdGenerator::default();

    let draft_id = id_gen.next_ulid();
    let confirm_token: ContentHash;
    let draft_rev: u64;

    // Phase 1: Import account with initial policy, ingest quote, submit a pending draft.
    {
        let store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
        import_account_into_store(
            &store,
            &orch,
            "trader-1",
            "imp-pol",
            make_opening_base(
                "ACCT-POL",
                ulid_pair('P', '1'),
                "FEED-POL",
                "BOND_001",
                fixed(50_000_000),
                0,
            ),
            test_policy("QSRC-A"),
        )
        .unwrap();

        let tick = MarketQuoteTick {
            source_id: "QSRC-A".to_owned(),
            quote_seq: 1,
            instrument_code: "BOND_001".to_owned(),
            source_type: PriceSourceType::ActiveQuote,
            bid_clean_price: None,
            ask_clean_price: None,
            mid_clean_price: Some(fixed(100)),
            mid_yield_to_maturity: None,
            observed_at: shanghai_time(2025, 6, 16, 9, 59, 0),
            received_at: shanghai_time(2025, 6, 16, 9, 59, 1),
        };
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.ingest_market_quotes(
                    ws,
                    &IngestMarketQuotesInput {
                        account_id: "ACCT-POL".to_owned(),
                        source_id: "QSRC-A".to_owned(),
                        ticks: vec![tick],
                    },
                )?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: ev,
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();

        let raw_text = "BUY T+0 INST=BOND_001 NOTIONAL=10000000 CLEAN=100.00";
        let draft_out = store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.submit_draft_earmark(
                    ws,
                    &SubmitDraftEarmarkCommand {
                        account_id: "ACCT-POL".to_owned(),
                        draft_id: draft_id.clone(),
                        proposal: NlpEarmarkProposal {
                            parser_source_id: "parser-1".to_owned(),
                            model_version: "1.0".to_owned(),
                            raw_source_text: raw_text.to_owned(),
                            anchors: vec![SourceTextAnchor {
                                field_name: "side".to_owned(),
                                byte_start: 0,
                                byte_end: 3,
                                span_text: "BUY".to_owned(),
                            }],
                            confidence_score: FixedDecimal::from_scaled(900_000_000_000),
                            account_id: "ACCT-POL".to_owned(),
                            instrument_code: "BOND_001".to_owned(),
                            side: TradeSide::Buy,
                            settlement_bucket: SettlementBucket::TPlus0,
                            notional: Some(fixed(10_000_000)),
                            quote_terms: Some(QuoteTerms {
                                basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                                settlement_amount: None,
                            }),
                            suggested_target_state: DraftTargetState::Hold,
                        },
                    },
                )?;
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: Some(ev),
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();
        let submitted = match draft_out {
            WriteExecutionOutcome::Committed { value, .. } => value,
            WriteExecutionOutcome::Replayed(_) => panic!("expected commit"),
        };
        confirm_token = submitted.draft.confirmation_token;
        draft_rev = submitted.draft.draft_revision;
    }

    // Build updated startup policy keeping quote source identical ("QSRC-A") while updating
    // regulatory_rule_pack, limit_ceilings, allowed_hedge_product_codes, staleness, and draft TTL.
    let updated_pack_payload = b"regulatory-pack-v2".to_vec();
    let updated_pack_hash = ContentHash::digest(&updated_pack_payload);
    let updated_pack = MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: ulid_pair('R', '2'),
            version: Version::new(2).unwrap(),
            owner: test_owner(),
            market: TEST_REG_MARKET.to_owned(),
            rule_type: TEST_REG_TYPE.to_owned(),
            source: "desk-policy-v2".to_owned(),
            effective: EffectivePeriod::new(
                shanghai_time(2025, 1, 1, 0, 0, 0),
                shanghai_time(2027, 1, 1, 0, 0, 0),
            )
            .unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: updated_pack_hash,
        },
        RulePackContent::new(TEST_REG_TYPE_URL, updated_pack_payload).unwrap(),
    )
    .unwrap();
    let mut updated_ceilings = BTreeMap::new();
    updated_ceilings.insert("MAX_INST_FACE".to_owned(), fixed(15_000_000_000));
    let updated_policy = AccountDeskPolicy {
        policy_version: 1,
        default_quote_source_id: "QSRC-A".to_owned(),
        instrument_quote_sources: BTreeMap::new(),
        quote_staleness_seconds: 300,
        default_draft_hold_ttl_seconds: 2_400,
        allowed_hedge_product_codes: vec!["PROD_C".to_owned()],
        regulatory_rule_pack: Some(updated_pack.clone()),
        limit_ceilings: updated_ceilings.clone(),
    };
    let mut startup_policies = BTreeMap::new();
    startup_policies.insert("ACCT-POL".to_owned(), updated_policy);

    // Phase 2: Reopen WAL, run startup policy reconciliation, commit, and shut down again.
    let reconciled_policy_version: u64;
    let reconciled_head_seq: u64;
    {
        let store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
        let outcome = store
            .execute_write(None, |ws| {
                let (next, out, ev) =
                    orch.reconcile_startup_quote_sources(ws, &startup_policies)?;
                assert!(out.mutated, "full desk policy delta must trigger mutation");
                assert!(ev.is_some());
                Ok((
                    out,
                    BlotterCommitPlan {
                        next_state: next,
                        event: ev,
                        idempotency_receipt: None,
                    },
                ))
            })
            .unwrap();
        let out = match outcome {
            WriteExecutionOutcome::Committed { value, .. } => value,
            WriteExecutionOutcome::Replayed(_) => panic!("expected commit"),
        };
        reconciled_policy_version = out.policy_version;
        reconciled_head_seq = out.committed_seq;
        assert_eq!(reconciled_policy_version, 2);
    }

    // Verify blotter_accounts.policy_version & policy_json persisted on disk before second reopen.
    {
        let raw = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let (persisted_pol_ver, policy_json): (i64, String) = raw
            .query_row(
                "SELECT policy_version, policy_json FROM blotter_accounts WHERE account_id = 'ACCT-POL'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(persisted_pol_ver, 2);
        assert!(policy_json.contains("PROD_C"));
        assert!(policy_json.contains("desk-policy-v2"));
    }

    // Phase 3: Reopen WAL again, assert hydrated policy_json, idempotent re-reconcile, and stale
    // draft invalidation on ConfirmDraftEarmark.
    let reopened = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
    assert_eq!(reopened.head_seq().unwrap(), reconciled_head_seq);
    let ws = reopened.load_workspace_state().unwrap();
    let hydrated_policy = &ws.accounts_meta.get("ACCT-POL").unwrap().policy;
    assert_eq!(hydrated_policy.policy_version, reconciled_policy_version);
    assert_eq!(hydrated_policy.quote_staleness_seconds, 300);
    assert_eq!(hydrated_policy.default_draft_hold_ttl_seconds, 2_400);
    assert_eq!(
        hydrated_policy.allowed_hedge_product_codes,
        vec!["PROD_C".to_owned()]
    );
    assert_eq!(hydrated_policy.regulatory_rule_pack, Some(updated_pack));
    assert_eq!(hydrated_policy.limit_ceilings, updated_ceilings);

    let snap = orch.get_account_snapshot(&ws, "ACCT-POL").unwrap();
    assert_eq!(snap.allowed_hedge_product_codes, vec!["PROD_C".to_owned()]);

    // Re-running startup reconciliation with the same config must now be a no-op.
    reopened
        .execute_write(None, |cur| {
            let (next, out, ev) = orch.reconcile_startup_quote_sources(cur, &startup_policies)?;
            assert!(!out.mutated);
            assert!(ev.is_none());
            Ok((
                out,
                BlotterCommitPlan {
                    next_state: next,
                    event: ev,
                    idempotency_receipt: None,
                },
            ))
        })
        .unwrap();
    assert_eq!(reopened.head_seq().unwrap(), reconciled_head_seq);

    // Pre-restart draft MUST fail confirmation with ConcurrencyConflict due to stale context_hash.
    let pos_ver = ws
        .domain
        .accounts
        .get("ACCT-POL")
        .unwrap()
        .positions
        .get("BOND_001")
        .unwrap()
        .position_version;
    let confirm_err = reopened
        .execute_write(None, |cur| {
            let (next, out, ev) = orch.confirm_draft_earmark(
                cur,
                &ConfirmDraftEarmarkCommand {
                    account_id: "ACCT-POL".to_owned(),
                    draft_id: draft_id.clone(),
                    earmark_id: id_gen.next_ulid(),
                    expected_draft_revision: draft_rev,
                    expected_position_version: pos_ver,
                    confirmation_token: confirm_token.clone(),
                    acknowledge_uncovered_dv01: false,
                    trader_id: "trader-1".to_owned(),
                },
            )?;
            Ok((
                out,
                BlotterCommitPlan {
                    next_state: next,
                    event: Some(ev),
                    idempotency_receipt: None,
                },
            ))
        })
        .unwrap_err();
    assert_eq!(
        confirm_err.category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );
}
