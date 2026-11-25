#![allow(clippy::too_many_lines, clippy::similar_names)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{NaiveDate, TimeZone};
use chrono_tz::Asia::Shanghai;
use ficant_application::ports::{
    ApplicationResult, BlotterCommitPlan, BlotterEventJournalPort, FuturesDeliveryRuleParser,
    IdempotencyScopeKey, ManualBlotterClock, RegulatoryRulePackParser, StoredIdempotencyReceipt,
};
use ficant_application::use_cases::blotter::{
    AccountDeskPolicy, AnalysisUnits, BlotterEvaluationEngines, DeskBlotterOrchestrator,
    DraftTargetState, FullOpeningPositionBase, ImportFullOpeningBaseCommand,
    IngestMarketQuotesInput, InstrumentCatalogEntry, MarketQuoteTick, NlpEarmarkProposal,
    RateCoverageEligibility, SourceTextAnchor, SubmitDraftEarmarkCommand,
    SwitchActiveQuoteSourceCommand,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_blotter_store::{
    ARCHIVE_MANIFEST_SCHEMA, ArchiveManifest, BLOTTER_SCHEMA_VERSION, ManifestStatus,
    RolloverError, RolloverFaultInjection, RolloverPhase, SqliteBlotterOpenOptions,
    SqliteBlotterStore, SqliteRolloverRequest, StartupSchemaFault, WriteExecutionOutcome,
    audit_sqlite_database, companion_lock_path, default_reference_analysis_units,
    generate_random_ulid, inspect_physical_file_identity, is_canonical_ulid, rollover_state_path,
    sqlite_rollover, sqlite_rollover_with_fault, validate_archive_filename,
    validate_archive_manifest, validate_archive_stem, verify_sqlite_archive,
};
use ficant_domain::analytics::{
    CalendarBinding, CalendarRequirement, FixedDecimal, MARKET_TIMEZONE,
};
use ficant_domain::blotter::{
    BlotterConfig, BlotterIdGenerator, CancelCommittedCommand, CreateCommittedCommand,
    CreateHoldCommand, DeterministicIdGenerator, IngestTradeFeedInput, OpeningInstrumentBalance,
    OpeningPositionBase, PendingMatchResolutionAction, QuoteBasis, QuoteTerms, ReleaseHoldCommand,
    ResolvePendingMatchInput, SettlementBucket, SweepLifecycleCommand, TradeFeedRecordAction,
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
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const TEST_REG_MARKET: &str = "TEST-REG-MKT";
const TEST_REG_TYPE: &str = "regulatory";
const TEST_REG_TYPE_URL: &str = "type.googleapis.com/ficant.test.RegulatoryRulePack";

type ManifestMutator = fn(&mut ArchiveManifest);

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
        allowed_hedge_product_codes: vec!["PROD_A".to_owned()],
        regulatory_rule_pack: Some(test_regulatory_pack()),
        limit_ceilings,
    }
}

fn make_opening_base(
    account_id: &str,
    base_ulid: Ulid,
    source_id: &str,
    instrument_code: &str,
    trading_date: NaiveDate,
    next_settlement_date: NaiveDate,
    source_start_after_feed_seq: u64,
) -> FullOpeningPositionBase {
    let units = test_units();
    FullOpeningPositionBase {
        core: OpeningPositionBase {
            account_id: account_id.to_owned(),
            base_id: base_ulid,
            trading_date,
            next_settlement_date,
            effective_from: shanghai_time(2025, 6, 16, 8, 30, 0),
            watermark_time: shanghai_time(2025, 6, 16, 9, 0, 0),
            watermark_source_seq: Some(source_start_after_feed_seq),
            source_id: source_id.to_owned(),
            opening_cash_t0: fixed(500_000_000),
            opening_net_cash_delta_t1: fixed(100_000_000),
            balances: vec![OpeningInstrumentBalance {
                instrument_code: instrument_code.to_owned(),
                opening_economic_notional: fixed(500_000_000),
                opening_available_notional: fixed(500_000_000),
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

fn import_account(
    store: &SqliteBlotterStore,
    orch: &DeskBlotterOrchestrator<'_>,
    actor_id: &str,
    idem_key: &str,
    base: FullOpeningPositionBase,
    policy: AccountDeskPolicy,
) -> u64 {
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
    match store
        .execute_write(Some((&scope_key, &req_hash)), |ws| {
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
        .unwrap()
    {
        WriteExecutionOutcome::Committed { committed_seq, .. } => committed_seq,
        WriteExecutionOutcome::Replayed(r) => r.committed_seq,
    }
}

fn file_sha256_opt(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let bytes = fs::read(path).unwrap();
    let digest = Sha256::digest(&bytes);
    Some(ficant_blotter_store::codec::encode_hex(&digest))
}

fn wal_path(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push("-wal");
    PathBuf::from(os)
}

/// Converts a freshly populated 8-column DB back into a genuine legacy 5-column BP-2/BP-3 v1 DB
/// (with unmerged frames left in `-wal`!) by recreating `blotter_meta` with only the 5 legacy cols.
fn convert_to_legacy_v1_with_live_wal(db_path: &Path) {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA journal_mode = WAL;
         PRAGMA wal_autocheckpoint = 0;
         BEGIN IMMEDIATE;
         CREATE TABLE blotter_meta_legacy (
             singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
             schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
             head_seq INTEGER NOT NULL CHECK (head_seq >= 0),
             oldest_retained_seq INTEGER NOT NULL CHECK (oldest_retained_seq >= 1),
             config_json TEXT NOT NULL
         );
         INSERT INTO blotter_meta_legacy
             (singleton_id, schema_version, head_seq, oldest_retained_seq, config_json)
         SELECT singleton_id, schema_version, head_seq, oldest_retained_seq, config_json
         FROM blotter_meta;
         DROP TABLE blotter_meta;
         ALTER TABLE blotter_meta_legacy RENAME TO blotter_meta;
         COMMIT;",
    )
    .unwrap();
    // Leave unmerged frames inside `<db>-wal` by NOT checkpointing before closing in EXCLUSIVE?
    // Even in NORMAL mode, wal_autocheckpoint=0 writes frames to WAL; confirm WAL exists when
    // kept open or by performing a write with locking_mode=EXCLUSIVE? Note: on NORMAL close,
    // SQLite auto-checkpoints WAL! To keep genuine unmerged WAL frames on disk after close
    // (exactly like a process crash / SIGKILL!), copy DB + WAL while the writer holds it open!
    drop(conn);
}

/// Creates a legacy 5-column v1 DB at `dst_db` with REAL unmerged frames sitting in `<dst_db>-wal`.
fn create_legacy_v1_with_unmerged_wal(src_populated_db: &Path, dst_db: &Path) {
    fs::copy(src_populated_db, dst_db).unwrap();
    convert_to_legacy_v1_with_live_wal(dst_db);

    // Open with wal_autocheckpoint=0, write a real committed idempotency receipt into `<tmp>-wal`
    // so committed data sits strictly in the unmerged WAL, then copy BOTH main DB and `-wal` to
    // `dst_db` while the connection is still open (simulating a process crash leaving live WAL).
    let stage_dir = TempDir::new().unwrap();
    let stage_db = stage_dir.path().join("stage.sqlite");
    fs::copy(dst_db, &stage_db).unwrap();

    let conn = Connection::open_with_flags(
        &stage_db,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;
         PRAGMA wal_autocheckpoint = 0;
         BEGIN IMMEDIATE;
         INSERT INTO blotter_idempotency_keys
             (actor_id, rpc_name, scope_key, idempotency_key, request_hash, committed_seq, response_payload)
         VALUES ('wal-fixture', 'SeedWal', 'ACCT-01', 'k-wal-1', zeroblob(32), 1, X'AABBCCDD');
         COMMIT;",
    )
    .unwrap();

    let stage_wal = wal_path(&stage_db);
    assert!(
        stage_wal.exists() && fs::metadata(&stage_wal).unwrap().len() > 0,
        "expected non-empty unmerged WAL frames"
    );

    fs::copy(&stage_db, dst_db).unwrap();
    fs::copy(&stage_wal, wal_path(dst_db)).unwrap();
    drop(conn);
}

fn populate_simple_db(db_path: &Path, with_unresolved_committed: bool) {
    let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 16, 9, 30, 0));
    let orch = orchestrator(&clock);
    let mut id_gen = DeterministicIdGenerator::default();
    let store = SqliteBlotterStore::open_file(db_path, BlotterConfig::default()).unwrap();
    let base = make_opening_base(
        "ACCT-01",
        ulid_pair('A', '1'),
        "FEED-A",
        "BOND_10Y",
        date(2025, 6, 16),
        date(2025, 6, 17),
        0,
    );
    import_account(&store, &orch, "admin", "imp-1", base, test_policy("QSRC-A"));

    if with_unresolved_committed {
        let cmd = CreateCommittedCommand {
            account_id: "ACCT-01".to_owned(),
            earmark_id: id_gen.next_ulid(),
            expected_position_version: 1,
            instrument_code: "BOND_10Y".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: fixed(10_000_000),
            quote_terms: QuoteTerms {
                basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                settlement_amount: Some(fixed(10_050_000)),
            },
            exact_dirty_price_input: None,
            note: "unresolved committed".to_owned(),
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
    }
}

// ============================================================================
// 1. Physical Identity, Hardlink/Alias/Junction Rejection & Lock Exclusion (F-01)
// ============================================================================

#[test]
fn physical_identity_aliases_hardlinks_junctions_and_lock_mutual_exclusion() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("source.sqlite");
    populate_simple_db(&db_path, false);

    let orig_db_sha = file_sha256_opt(&db_path).unwrap();
    let orig_wal_sha = file_sha256_opt(&wal_path(&db_path));

    // 1a. Same path: archive_dir == db parent && archive_name == "source" -> source.sqlite == archive!
    let same_err = sqlite_rollover(&SqliteRolloverRequest {
        db_path: db_path.clone(),
        archive_dir: dir.path().to_path_buf(),
        archive_name: Some("source".to_owned()),
        archive_only: false,
        allow_unresolved: false,
    })
    .unwrap_err();
    assert!(matches!(same_err, RolloverError::Validation(_)));
    assert_eq!(file_sha256_opt(&db_path).unwrap(), orig_db_sha);
    assert_eq!(file_sha256_opt(&wal_path(&db_path)), orig_wal_sha);

    // 1b. Relative path alias (`sub/../source.sqlite`)
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    let rel_archive_dir = sub.join("..");
    let rel_err = sqlite_rollover(&SqliteRolloverRequest {
        db_path: db_path.clone(),
        archive_dir: rel_archive_dir,
        archive_name: Some("source".to_owned()),
        archive_only: false,
        allow_unresolved: false,
    })
    .unwrap_err();
    assert!(matches!(rel_err, RolloverError::Validation(_)));
    assert_eq!(file_sha256_opt(&db_path).unwrap(), orig_db_sha);

    // 1c. Windows case alias (`SOURCE.sqlite` vs `source.sqlite`)
    #[cfg(windows)]
    {
        let case_err = sqlite_rollover(&SqliteRolloverRequest {
            db_path: db_path.clone(),
            archive_dir: dir.path().to_path_buf(),
            archive_name: Some("SOURCE".to_owned()),
            archive_only: false,
            allow_unresolved: false,
        })
        .unwrap_err();
        assert!(matches!(case_err, RolloverError::Validation(_)));
        assert_eq!(file_sha256_opt(&db_path).unwrap(), orig_db_sha);
    }

    // 1d. Hardlink alias of source DB blocks BOTH rollover (as archive alias & nlink>1) AND open_file!
    let archive_dir = dir.path().join("archives");
    fs::create_dir(&archive_dir).unwrap();
    let hardlink_archive = archive_dir.join("hl_alias.sqlite");
    fs::hard_link(&db_path, &hardlink_archive).unwrap();

    let hl_rollover_err = sqlite_rollover(&SqliteRolloverRequest {
        db_path: db_path.clone(),
        archive_dir: archive_dir.clone(),
        archive_name: Some("hl_alias".to_owned()),
        archive_only: false,
        allow_unresolved: false,
    })
    .unwrap_err();
    assert!(matches!(hl_rollover_err, RolloverError::Validation(_)));
    assert_eq!(file_sha256_opt(&db_path).unwrap(), orig_db_sha);

    // Opening via hardlink alias OR original while nlink > 1 fails with ValidationFailed!
    let alias_open_err =
        SqliteBlotterStore::open_file(&hardlink_archive, BlotterConfig::default()).unwrap_err();
    assert_eq!(
        alias_open_err.category(),
        ApplicationErrorCategory::ValidationFailed
    );
    let orig_open_err =
        SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap_err();
    assert_eq!(
        orig_open_err.category(),
        ApplicationErrorCategory::ValidationFailed
    );
    fs::remove_file(&hardlink_archive).unwrap();

    // 1e. source == lock hardlink and archive == lock hardlink rejected before write!
    let lock_file = companion_lock_path(&db_path);
    if lock_file.exists() {
        fs::remove_file(&lock_file).unwrap();
    }
    fs::hard_link(&db_path, &lock_file).unwrap();
    let src_lock_err = sqlite_rollover(&SqliteRolloverRequest {
        db_path: db_path.clone(),
        archive_dir: archive_dir.clone(),
        archive_name: Some("snap1".to_owned()),
        archive_only: false,
        allow_unresolved: false,
    })
    .unwrap_err();
    assert!(matches!(src_lock_err, RolloverError::Validation(_)));
    fs::remove_file(&lock_file).unwrap();

    // Archive == lock hardlink
    fs::write(&lock_file, b"").unwrap();
    let arch_lock_alias = archive_dir.join("lock_alias.sqlite");
    fs::hard_link(&lock_file, &arch_lock_alias).unwrap();
    let arch_lock_err = sqlite_rollover(&SqliteRolloverRequest {
        db_path: db_path.clone(),
        archive_dir: archive_dir.clone(),
        archive_name: Some("lock_alias".to_owned()),
        archive_only: false,
        allow_unresolved: false,
    })
    .unwrap_err();
    assert!(matches!(arch_lock_err, RolloverError::Validation(_)));
    fs::remove_file(&arch_lock_alias).unwrap();

    // 1f. Windows Junction / Unix Symlink ancestor rejection
    #[cfg(windows)]
    {
        let junction = dir.path().join("junc_dir");
        let status = std::process::Command::new("cmd")
            .args([
                "/c",
                "mklink",
                "/J",
                junction.to_str().unwrap(),
                dir.path().to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J should succeed without admin");
        let junc_db = junction.join("source.sqlite");
        let junc_open_err =
            SqliteBlotterStore::open_file(&junc_db, BlotterConfig::default()).unwrap_err();
        assert_eq!(
            junc_open_err.category(),
            ApplicationErrorCategory::ValidationFailed
        );
        let junc_roll_err = sqlite_rollover(&SqliteRolloverRequest {
            db_path: junc_db,
            archive_dir: archive_dir.clone(),
            archive_name: Some("junc_snap".to_owned()),
            archive_only: true,
            allow_unresolved: false,
        })
        .unwrap_err();
        assert!(matches!(junc_roll_err, RolloverError::Validation(_)));
        // Clean up junction cleanly via rmdir (not remove_dir_all!)
        let _ = std::process::Command::new("cmd")
            .args(["/c", "rmdir", junction.to_str().unwrap()])
            .status();
    }

    // 1g. Service vs Rollover mutual exclusion (busy_timeout=0, zero retry sleep)
    let store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
    let bound_id = store.ledger_identity().unwrap();
    assert_eq!(bound_id.ledger_epoch, 1);
    assert!(is_canonical_ulid(&bound_id.ledger_instance_id));

    let t0 = Instant::now();
    let busy_err = sqlite_rollover(&SqliteRolloverRequest {
        db_path: db_path.clone(),
        archive_dir: archive_dir.clone(),
        archive_name: Some("while_open".to_owned()),
        archive_only: true,
        allow_unresolved: false,
    })
    .unwrap_err();
    assert!(
        matches!(busy_err, RolloverError::LockBusy(_)),
        "expected LockBusy while store holds companion lock, got {busy_err:?}"
    );
    assert!(
        t0.elapsed().as_secs() < 5,
        "lock busy must return immediately without retry sleep"
    );

    // Identity-fenced write hook runs BEFORE idempotency replay and leaves zero writes on denial.
    let existing_scope = IdempotencyScopeKey {
        actor_id: "admin".to_owned(),
        rpc_name: "ImportOpeningBase".to_owned(),
        scope_key: "ACCT-01".to_owned(),
        idempotency_key: "imp-1".to_owned(),
    };
    let dummy_hash = ContentHash::digest(b"any");
    let head_before = store.head_seq().unwrap();
    let fence_err = store
        .execute_write_authorized_with_identity(
            Some((&existing_scope, &dummy_hash)),
            |_ws, id| {
                assert_eq!(id, &bound_id);
                Err(ApplicationError::new(
                    ApplicationErrorCategory::ConcurrencyConflict,
                    false,
                ))
            },
            |_ws| -> ApplicationResult<((), BlotterCommitPlan)> {
                panic!("transition must not run after identity fence denial")
            },
        )
        .unwrap_err();
    assert_eq!(
        fence_err.category(),
        ApplicationErrorCategory::ConcurrencyConflict
    );
    assert!(!fence_err.retryable());
    assert_eq!(store.head_seq().unwrap(), head_before);

    // Dropping store releases companion lock so rollover succeeds immediately.
    drop(store);
    let ok_rep = sqlite_rollover(&SqliteRolloverRequest {
        db_path: db_path.clone(),
        archive_dir,
        archive_name: Some("after_close".to_owned()),
        archive_only: true,
        allow_unresolved: false,
    })
    .unwrap();
    assert_eq!(ok_rep.phase, RolloverPhase::CompletedArchiveOnly);
    assert!(ok_rep.manifest.wal_truncated);
    assert_eq!(ok_rep.manifest.source_ledger_epoch, bound_id.ledger_epoch);
    assert_eq!(
        ok_rep.manifest.source_ledger_instance_id,
        bound_id.ledger_instance_id
    );
    verify_sqlite_archive(&ok_rep.manifest_path).unwrap();
}

// ============================================================================
// 2. Legacy v1 Identity Derivation, Zero-Mutation Block & Migration (F-18)
// ============================================================================

#[test]
fn legacy_v1_identity_derivation_zero_mutation_block_archive_only_and_startup_migration() {
    let dir = TempDir::new().unwrap();
    let seed_db = dir.path().join("seed.sqlite");
    populate_simple_db(&seed_db, true);

    let legacy_db = dir.path().join("legacy_v1.sqlite");
    create_legacy_v1_with_unmerged_wal(&seed_db, &legacy_db);

    let orig_db_sha = file_sha256_opt(&legacy_db).unwrap();
    let orig_wal_sha = file_sha256_opt(&wal_path(&legacy_db)).unwrap();
    assert!(
        fs::metadata(wal_path(&legacy_db)).unwrap().len() > 0,
        "legacy test fixture must have live unmerged WAL frames"
    );

    let archive_dir = dir.path().join("archives");
    fs::create_dir(&archive_dir).unwrap();

    // 2a. Blocked unresolved reset on legacy v1 DB: checks private WAL copy -> blocks with 0-byte source mutation!
    let blocked = sqlite_rollover(&SqliteRolloverRequest {
        db_path: legacy_db.clone(),
        archive_dir: archive_dir.clone(),
        archive_name: Some("blocked_v1".to_owned()),
        archive_only: false,
        allow_unresolved: false,
    })
    .unwrap_err();
    assert_eq!(
        blocked,
        RolloverError::UnresolvedItemsBlocked {
            unresolved_committed: 1,
            open_pending_match: 0,
        }
    );
    assert_eq!(file_sha256_opt(&legacy_db).unwrap(), orig_db_sha);
    assert_eq!(
        file_sha256_opt(&wal_path(&legacy_db)).unwrap(),
        orig_wal_sha
    );
    assert!(!rollover_state_path(&legacy_db).exists());

    // 2b. Direct `--archive-only` on legacy v1 DB: 0-byte source mutation, deterministic virtual ULID materialized in archive!
    let arch_rep = sqlite_rollover(&SqliteRolloverRequest {
        db_path: legacy_db.clone(),
        archive_dir: archive_dir.clone(),
        archive_name: Some("v1_arch_1".to_owned()),
        archive_only: true,
        allow_unresolved: false,
    })
    .unwrap();
    assert_eq!(arch_rep.manifest.status, ManifestStatus::ArchiveOnly);
    assert!(!arch_rep.manifest.wal_truncated);
    assert_eq!(arch_rep.manifest.source_ledger_epoch, 1);
    assert!(is_canonical_ulid(
        &arch_rep.manifest.source_ledger_instance_id
    ));
    assert_eq!(file_sha256_opt(&legacy_db).unwrap(), orig_db_sha);
    assert_eq!(
        file_sha256_opt(&wal_path(&legacy_db)).unwrap(),
        orig_wal_sha
    );
    verify_sqlite_archive(&arch_rep.manifest_path).unwrap();

    // Repeat `--archive-only` with a different stem -> derives the EXACT SAME deterministic ULID!
    let arch_rep_2 = sqlite_rollover(&SqliteRolloverRequest {
        db_path: legacy_db.clone(),
        archive_dir: archive_dir.clone(),
        archive_name: Some("v1_arch_2".to_owned()),
        archive_only: true,
        allow_unresolved: false,
    })
    .unwrap();
    assert_eq!(
        arch_rep_2.manifest.source_ledger_instance_id,
        arch_rep.manifest.source_ledger_instance_id
    );
    assert_eq!(
        arch_rep_2.manifest.content_sha256_hex,
        arch_rep.manifest.content_sha256_hex
    );

    // 2c. Independent physical copy with identical bytes produces a DIFFERENT derived ULID!
    let copy_db = dir.path().join("legacy_v1_physical_copy.sqlite");
    fs::copy(&legacy_db, &copy_db).unwrap();
    fs::copy(wal_path(&legacy_db), wal_path(&copy_db)).unwrap();
    assert_ne!(
        inspect_physical_file_identity(&legacy_db, false)
            .unwrap()
            .id,
        inspect_physical_file_identity(&copy_db, false).unwrap().id
    );
    let copy_rep = sqlite_rollover(&SqliteRolloverRequest {
        db_path: copy_db.clone(),
        archive_dir: archive_dir.clone(),
        archive_name: Some("v1_copy_arch".to_owned()),
        archive_only: true,
        allow_unresolved: false,
    })
    .unwrap();
    assert_ne!(
        copy_rep.manifest.source_ledger_instance_id,
        arch_rep.manifest.source_ledger_instance_id
    );

    // 2d. Startup transaction interrupted before COMMIT -> rolls back, retry yields identical ULID!
    let opts = SqliteBlotterOpenOptions::file(&legacy_db, BlotterConfig::default());
    let fault_err = SqliteBlotterStore::open_with_startup_fault(
        &opts,
        StartupSchemaFault::FailBeforeSchemaCommit,
    )
    .unwrap_err();
    assert_eq!(
        fault_err.category(),
        ApplicationErrorCategory::StorageUnavailable
    );

    // Normal startup migrates in one transaction to the EXACT SAME derived ULID!
    let migrated_id = {
        let store = SqliteBlotterStore::open_file(&legacy_db, BlotterConfig::default()).unwrap();
        let id = store.ledger_identity().unwrap();
        assert_eq!(id.ledger_epoch, 1);
        assert_eq!(
            id.ledger_instance_id,
            arch_rep.manifest.source_ledger_instance_id
        );
        assert!(id.last_reset_operation_id.is_none());

        // Perform a normal write (which rewrites `blotter_meta.config_json`) -> identity preserved!
        let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 16, 9, 35, 0));
        let orch = orchestrator(&clock);
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.switch_active_quote_source(
                    ws,
                    &SwitchActiveQuoteSourceCommand {
                        account_id: "ACCT-01".to_owned(),
                        instrument_code: None,
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
        id
    };

    // Reopen after normal write -> identity strictly preserved!
    {
        let store = SqliteBlotterStore::open_file(&legacy_db, BlotterConfig::default()).unwrap();
        assert_eq!(store.ledger_identity().unwrap(), migrated_id);
    }

    // Direct reset on legacy v1 copy (without prior normal open) migrates source & increments epoch 1 -> 2!
    let reset_rep = sqlite_rollover(&SqliteRolloverRequest {
        db_path: copy_db.clone(),
        archive_dir: archive_dir.clone(),
        archive_name: Some("v1_direct_reset".to_owned()),
        archive_only: false,
        allow_unresolved: true,
    })
    .unwrap();
    assert_eq!(reset_rep.manifest.status, ManifestStatus::CompletedReset);
    assert_eq!(reset_rep.manifest.source_ledger_epoch, 1);
    assert_eq!(
        reset_rep.manifest.source_ledger_instance_id,
        copy_rep.manifest.source_ledger_instance_id
    );
    assert_eq!(reset_rep.manifest.next_ledger_epoch, Some(2));
    assert!(reset_rep.manifest.allow_unresolved_used);
    assert!(reset_rep.manifest.wal_truncated);

    // Open post-reset DB, do a normal write, close & reopen -> (2, next_id, op_id) preserved!
    let next_expected_id = reset_rep.manifest.next_ledger_instance_id.clone().unwrap();
    {
        let store = SqliteBlotterStore::open_file(&copy_db, BlotterConfig::default()).unwrap();
        let id = store.ledger_identity().unwrap();
        assert_eq!(id.ledger_epoch, 2);
        assert_eq!(id.ledger_instance_id, next_expected_id);
        assert_eq!(
            id.last_reset_operation_id.as_deref(),
            Some(reset_rep.operation_id.as_str())
        );
        let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 17, 9, 0, 0));
        let orch = orchestrator(&clock);
        let day2_base = make_opening_base(
            "ACCT-01",
            ulid_pair('A', '2'),
            "FEED-A",
            "BOND_10Y",
            date(2025, 6, 17),
            date(2025, 6, 18),
            0,
        );
        import_account(
            &store,
            &orch,
            "admin",
            "imp-d2",
            day2_base,
            test_policy("QSRC-A"),
        );
    }
    {
        let store = SqliteBlotterStore::open_file(&copy_db, BlotterConfig::default()).unwrap();
        let id = store.ledger_identity().unwrap();
        assert_eq!(id.ledger_epoch, 2);
        assert_eq!(id.ledger_instance_id, next_expected_id);
        assert_eq!(
            id.last_reset_operation_id.as_deref(),
            Some(reset_rep.operation_id.as_str())
        );
    }

    // 2e. Partial migration / NULL / half-null / invalid ULID fail closed with StorageUnavailable!
    let bad_db = dir.path().join("bad_identity.sqlite");
    populate_simple_db(&bad_db, false);
    for sql in [
        "UPDATE blotter_meta SET ledger_epoch = NULL, ledger_instance_id = NULL WHERE singleton_id = 1",
        "UPDATE blotter_meta SET ledger_epoch = 1, ledger_instance_id = NULL WHERE singleton_id = 1",
        "UPDATE blotter_meta SET ledger_epoch = 1, ledger_instance_id = 'invalid-ulid' WHERE singleton_id = 1",
        "UPDATE blotter_meta SET ledger_epoch = 1, ledger_instance_id = '01arz3ndektsv4rrffq69g5fav' WHERE singleton_id = 1",
    ] {
        let conn = Connection::open(&bad_db).unwrap();
        conn.execute(sql, []).unwrap();
        drop(conn);
        let err = SqliteBlotterStore::open_file(&bad_db, BlotterConfig::default()).unwrap_err();
        assert_eq!(
            err.category(),
            ApplicationErrorCategory::StorageUnavailable,
            "SQL '{sql}' must fail closed with StorageUnavailable"
        );
    }

    // Partial migration (only 6 columns instead of 5 or 8) fails closed!
    let partial_db = dir.path().join("partial_migration.sqlite");
    fs::copy(&seed_db, &partial_db).unwrap();
    convert_to_legacy_v1_with_live_wal(&partial_db);
    {
        let conn = Connection::open(&partial_db).unwrap();
        conn.execute(
            "ALTER TABLE blotter_meta ADD COLUMN ledger_epoch INTEGER",
            [],
        )
        .unwrap();
    }
    let partial_err =
        SqliteBlotterStore::open_file(&partial_db, BlotterConfig::default()).unwrap_err();
    assert_eq!(
        partial_err.category(),
        ApplicationErrorCategory::StorageUnavailable
    );

    // 2f. Ephemeral in-memory stores generate distinct epoch=1 canonical ULIDs.
    let m1 = SqliteBlotterStore::open_ephemeral_in_memory(BlotterConfig::default()).unwrap();
    let m2 = SqliteBlotterStore::open_ephemeral_in_memory(BlotterConfig::default()).unwrap();
    assert_eq!(m1.ledger_identity().unwrap().ledger_epoch, 1);
    assert_eq!(m2.ledger_identity().unwrap().ledger_epoch, 1);
    assert_ne!(
        m1.ledger_identity().unwrap().ledger_instance_id,
        m2.ledger_identity().unwrap().ledger_instance_id
    );
}

// ============================================================================
// 3. 17-Table Dual-Source Audit Positive Cases & Single-Field Tampering (F-03)
// ============================================================================

#[test]
fn seventeen_table_dual_source_audit_positive_cases_and_single_field_tamper_detection() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("rich_17_tables.sqlite");
    let archive_dir = dir.path().join("archives");
    fs::create_dir(&archive_dir).unwrap();

    let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 16, 9, 30, 0));
    let orch = orchestrator(&clock);
    let mut id_gen = DeterministicIdGenerator::default();

    // Use a small event retention window (4) so event pruning occurs and retained_events != head_seq!
    let opts = SqliteBlotterOpenOptions::file(&db_path, BlotterConfig::default())
        .with_event_retention_window(4);
    {
        let store = SqliteBlotterStore::open(&opts).unwrap();
        let base = make_opening_base(
            "ACCT-RICH",
            ulid_pair('E', '1'),
            "FEED-R",
            "BOND_10Y",
            date(2025, 6, 16),
            date(2025, 6, 17),
            0,
        );
        import_account(
            &store,
            &orch,
            "admin",
            "imp-rich",
            base,
            test_policy("QSRC-A"),
        );

        // 1. Advance `account_version` independently of OpeningBaseFullImageDto!
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.switch_active_quote_source(
                    ws,
                    &SwitchActiveQuoteSourceCommand {
                        account_id: "ACCT-RICH".to_owned(),
                        instrument_code: None,
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

        // 2. Ingest market quotes (populates blotter_quote_history & blotter_quote_heads).
        let q_tick = MarketQuoteTick {
            source_id: "QSRC-B".to_owned(),
            quote_seq: 1,
            instrument_code: "BOND_10Y".to_owned(),
            source_type: PriceSourceType::ActiveQuote,
            bid_clean_price: Some(fixed(100)),
            ask_clean_price: Some(fixed(101)),
            mid_clean_price: Some(fixed(100)),
            mid_yield_to_maturity: Some(FixedDecimal::from_scaled(25_000_000_000)),
            observed_at: shanghai_time(2025, 6, 16, 9, 29, 0),
            received_at: shanghai_time(2025, 6, 16, 9, 29, 30),
        };
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.ingest_market_quotes(
                    ws,
                    &IngestMarketQuotesInput {
                        account_id: "ACCT-RICH".to_owned(),
                        source_id: "QSRC-B".to_owned(),
                        ticks: vec![q_tick.clone()],
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

        // 3. Submit full DraftEarmarkRecord (created_seq != preview_at_seq!).
        let full_draft_id = id_gen.next_ulid();
        let raw_text = "BUY T+0 INST=BOND_10Y NOTIONAL=10000000 CLEAN=100.00";
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.submit_draft_earmark(
                    ws,
                    &SubmitDraftEarmarkCommand {
                        account_id: "ACCT-RICH".to_owned(),
                        draft_id: full_draft_id.clone(),
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
                            confidence_score: FixedDecimal::from_scaled(950_000_000_000),
                            account_id: "ACCT-RICH".to_owned(),
                            instrument_code: "BOND_10Y".to_owned(),
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

        // 4. Create all 6 earmark states: HOLD, RELEASED, EXPIRED, COMMITTED, CANCELLED, RECONCILED.
        let hold_active_id = id_gen.next_ulid();
        let hold_release_id = id_gen.next_ulid();
        let hold_expire_id = id_gen.next_ulid();
        for (eid, ttl) in [
            (hold_active_id.clone(), 2_700_u32),
            (hold_release_id.clone(), 2_700_u32),
            (hold_expire_id.clone(), 1_800_u32),
        ] {
            store
                .execute_write(None, |ws| {
                    let pos_ver =
                        ws.domain.accounts["ACCT-RICH"].positions["BOND_10Y"].position_version;
                    let (next, out, ev) = orch.create_hold(
                        ws,
                        &CreateHoldCommand {
                            account_id: "ACCT-RICH".to_owned(),
                            earmark_id: eid,
                            expected_position_version: pos_ver,
                            instrument_code: "BOND_10Y".to_owned(),
                            side: TradeSide::Sell,
                            settlement_bucket: SettlementBucket::TPlus0,
                            notional: fixed(5_000_000),
                            quote_terms: None,
                            ttl_seconds: ttl,
                            note: "hold".to_owned(),
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
        }

        // Release one hold -> RELEASED.
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.release_hold(
                    ws,
                    &ReleaseHoldCommand {
                        account_id: "ACCT-RICH".to_owned(),
                        earmark_id: hold_release_id.clone(),
                        expected_earmark_revision: 1,
                        actor_id: "trader-1".to_owned(),
                        reason: "released".to_owned(),
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

        // Advance clock by 35 min (between 1800s and 2700s) and sweep -> expires `hold_expire_id` -> EXPIRED.
        clock.set(shanghai_time(2025, 6, 16, 10, 5, 0));
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.sweep_lifecycle(
                    ws,
                    &SweepLifecycleCommand {
                        account_id: Some("ACCT-RICH".to_owned()),
                        source_id: None,
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

        // Create 3 committed earmarks: one stays COMMITTED, one CANCELLED, one RECONCILED via trade fill.
        let comm_active_id = id_gen.next_ulid();
        let comm_cancel_id = id_gen.next_ulid();
        let comm_recon_id = id_gen.next_ulid();
        for (eid, bucket, notional) in [
            (
                comm_active_id.clone(),
                SettlementBucket::TPlus1,
                fixed(10_000_000),
            ),
            (
                comm_cancel_id.clone(),
                SettlementBucket::TPlus0,
                fixed(12_000_000),
            ),
            (
                comm_recon_id.clone(),
                SettlementBucket::TPlus0,
                fixed(15_000_000),
            ),
        ] {
            store
                .execute_write(None, |ws| {
                    let pos_ver =
                        ws.domain.accounts["ACCT-RICH"].positions["BOND_10Y"].position_version;
                    let (next, out, ev) = orch.create_committed(
                        ws,
                        &CreateCommittedCommand {
                            account_id: "ACCT-RICH".to_owned(),
                            earmark_id: eid,
                            expected_position_version: pos_ver,
                            instrument_code: "BOND_10Y".to_owned(),
                            side: TradeSide::Sell,
                            settlement_bucket: bucket,
                            notional,
                            quote_terms: QuoteTerms {
                                basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                                settlement_amount: Some(notional),
                            },
                            exact_dirty_price_input: None,
                            note: "comm".to_owned(),
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
        }

        // Cancel one committed -> CANCELLED.
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.cancel_committed(
                    ws,
                    &CancelCommittedCommand {
                        account_id: "ACCT-RICH".to_owned(),
                        earmark_id: comm_cancel_id.clone(),
                        expected_earmark_revision: 1,
                        actor_id: "trader-1".to_owned(),
                        reason: "cancelled".to_owned(),
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

        // Ingest 3 fills:
        // seq=1 matches `comm_recon_id` -> RECONCILED + reconcile_allocation
        // seq=2 orphan -> OPEN pending_match (we will resolve to RESOLVED_ORPHAN_ACCEPTED)
        // seq=3 orphan -> stays OPEN pending_match
        let fills = vec![
            TradeFillRecord {
                source_id: "FEED-R".to_owned(),
                external_trade_id: "TRD-MATCH".to_owned(),
                source_revision: 1,
                feed_seq: 1,
                action: TradeFeedRecordAction::NewFill,
                account_id: "ACCT-RICH".to_owned(),
                instrument_code: "BOND_10Y".to_owned(),
                side: TradeSide::Sell,
                settlement_date: date(2025, 6, 16),
                notional: fixed(15_000_000),
                quote_terms: Some(QuoteTerms {
                    basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                    settlement_amount: Some(fixed(15_000_000)),
                }),
                executed_at: shanghai_time(2025, 6, 16, 9, 31, 0),
                batch_first_feed_seq: 1,
                batch_last_feed_seq: 3,
                exact_dirty_price_input: None,
            },
            TradeFillRecord {
                source_id: "FEED-R".to_owned(),
                external_trade_id: "TRD-ORPHAN-1".to_owned(),
                source_revision: 1,
                feed_seq: 2,
                action: TradeFeedRecordAction::NewFill,
                account_id: "ACCT-RICH".to_owned(),
                instrument_code: "BOND_10Y".to_owned(),
                side: TradeSide::Buy,
                settlement_date: date(2025, 6, 16),
                notional: fixed(20_000_000),
                quote_terms: Some(QuoteTerms {
                    basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                    settlement_amount: Some(fixed(20_000_000)),
                }),
                executed_at: shanghai_time(2025, 6, 16, 9, 31, 10),
                batch_first_feed_seq: 1,
                batch_last_feed_seq: 3,
                exact_dirty_price_input: None,
            },
            TradeFillRecord {
                source_id: "FEED-R".to_owned(),
                external_trade_id: "TRD-ORPHAN-2".to_owned(),
                source_revision: 1,
                feed_seq: 3,
                action: TradeFeedRecordAction::NewFill,
                account_id: "ACCT-RICH".to_owned(),
                instrument_code: "BOND_10Y".to_owned(),
                side: TradeSide::Buy,
                settlement_date: date(2025, 6, 16),
                notional: fixed(25_000_000),
                quote_terms: Some(QuoteTerms {
                    basis: Some(QuoteBasis::CleanPricePer100(fixed(100))),
                    settlement_amount: Some(fixed(25_000_000)),
                }),
                executed_at: shanghai_time(2025, 6, 16, 9, 31, 20),
                batch_first_feed_seq: 1,
                batch_last_feed_seq: 3,
                exact_dirty_price_input: None,
            },
        ];
        let mut resolved_pm_id = None;
        store
            .execute_write(None, |ws| {
                let (next, out, ev) = orch.ingest_trade_feed(
                    ws,
                    &IngestTradeFeedInput {
                        source_id: "FEED-R".to_owned(),
                        fills,
                        heartbeat_at: Some(shanghai_time(2025, 6, 16, 10, 5, 0)),
                        actor_id: "feed-adapter".to_owned(),
                    },
                    &mut id_gen,
                )?;
                resolved_pm_id = out.item_outcomes[1].pending_match_id.clone();
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

        // Resolve one orphan pending match -> RESOLVED_ORPHAN_ACCEPTED.
        store
            .execute_write(None, |ws| {
                let pos_ver =
                    ws.domain.accounts["ACCT-RICH"].positions["BOND_10Y"].position_version;
                let (next, out, ev) = orch.resolve_pending_match(
                    ws,
                    &ResolvePendingMatchInput {
                        account_id: "ACCT-RICH".to_owned(),
                        pending_match_id: resolved_pm_id.clone().unwrap(),
                        expected_pending_revision: 1,
                        expected_position_version: pos_ver,
                        action: PendingMatchResolutionAction::AcceptOrphan,
                        expected_earmark_revision: None,
                        target_earmark_id: None,
                        offset_notional: None,
                        reason: "accepted".to_owned(),
                        baseline_delta: None,
                        actor_id: "trader-1".to_owned(),
                    },
                    &mut id_gen,
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

        // Verify event journal retention pruning actually happened!
        assert!(store.oldest_retained_seq().unwrap() > 1);
    }

    // Insert a legal `record_json IS NULL` presence-only draft matching `store.rs:1174-1182`!
    let null_draft_id = generate_random_ulid().unwrap();
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO blotter_draft_earmarks (
                draft_id, account_id, instrument_code, draft_revision,
                status, created_seq, confirmation_token, context_hash,
                confirmed_by_trader_id, record_json
             ) VALUES (?1, 'ACCT-RICH', 'BOND_10Y', 1, 'PENDING_CONFIRMATION', 1, ?2, ?3, NULL, NULL)",
            params![null_draft_id, [0_u8; 32].as_slice(), [0_u8; 32].as_slice()],
        )
        .unwrap();
    }

    // Archive-only and verify!
    let rep = sqlite_rollover(&SqliteRolloverRequest {
        db_path: db_path.clone(),
        archive_dir: archive_dir.clone(),
        archive_name: Some("rich_snap".to_owned()),
        archive_only: true,
        allow_unresolved: false,
    })
    .unwrap();
    verify_sqlite_archive(&rep.manifest_path).unwrap();

    // Check EVERY count and field!
    assert_eq!(rep.manifest.schema, ARCHIVE_MANIFEST_SCHEMA);
    assert_eq!(rep.manifest.business_schema_version, BLOTTER_SCHEMA_VERSION);
    assert_eq!(rep.manifest.account_count, 1);
    assert_eq!(rep.manifest.accounts[0].account_id, "ACCT-RICH");
    assert_eq!(rep.manifest.counts.active_hold, 1);
    assert_eq!(rep.manifest.counts.unresolved_committed, 1);
    assert_eq!(rep.manifest.counts.reconciled_committed, 1);
    assert_eq!(rep.manifest.counts.expired_hold, 1);
    assert_eq!(rep.manifest.counts.released_hold, 1);
    assert_eq!(rep.manifest.counts.cancelled_committed, 1);
    assert_eq!(rep.manifest.counts.open_pending_match, 1);
    assert_eq!(rep.manifest.counts.resolved_pending_match, 1);
    assert_eq!(rep.manifest.counts.pending_draft, 2); // 1 full draft + 1 legal NULL draft!
    assert_eq!(rep.manifest.counts.retained_events, 4); // retention window = 4!
    assert!(rep.manifest.oldest_retained_seq > 1);

    // Single-field tamper detection tests on isolated copies of the verified archive!
    let tamper_cases: &[(&str, &str)] = &[
        (
            "account_trading_date",
            "UPDATE blotter_accounts SET trading_date = '2099-01-01' WHERE account_id = 'ACCT-RICH'",
        ),
        (
            "earmark_state",
            "UPDATE blotter_earmarks SET state = 'CANCELLED' WHERE state = 'HOLD'",
        ),
        (
            "full_draft_token",
            "UPDATE blotter_draft_earmarks SET confirmation_token = zeroblob(32) WHERE record_json IS NOT NULL",
        ),
        (
            "null_draft_bad_revision",
            "UPDATE blotter_draft_earmarks SET draft_revision = 2 WHERE record_json IS NULL",
        ),
        (
            "null_draft_nonzero_token",
            "UPDATE blotter_draft_earmarks SET confirmation_token = X'0100000000000000000000000000000000000000000000000000000000000000' WHERE record_json IS NULL",
        ),
    ];
    for (label, sql) in tamper_cases {
        let tmp = dir.path().join(format!("tamper_{label}.sqlite"));
        fs::copy(&rep.archive_path, &tmp).unwrap();
        let conn = Connection::open(&tmp).unwrap();
        let changed = conn.execute(sql, []).unwrap();
        assert!(changed >= 1, "tamper SQL '{label}' must modify a row");
        let audit_err = audit_sqlite_database(&conn, None).unwrap_err();
        assert!(
            matches!(audit_err, RolloverError::AuditFailed(_)),
            "tamper '{label}' must be rejected by 17-table audit, got {audit_err:?}"
        );
    }

    // Tamper blotter_idempotency_keys.response_payload -> verify_sqlite_archive catches digest change!
    {
        let tdir = dir.path().join("tamper_receipt");
        fs::create_dir(&tdir).unwrap();
        let t_arch = tdir.join("rich_snap.sqlite");
        let t_man = tdir.join("rich_snap.manifest.json");
        fs::copy(&rep.archive_path, &t_arch).unwrap();
        fs::copy(&rep.manifest_path, &t_man).unwrap();
        {
            let conn = Connection::open(&t_arch).unwrap();
            conn.execute(
                "UPDATE blotter_idempotency_keys SET response_payload = X'DEADBEEF'",
                [],
            )
            .unwrap();
        }
        assert!(matches!(
            verify_sqlite_archive(&t_man).unwrap_err(),
            RolloverError::AuditFailed(_)
        ));
    }

    // Tamper individual manifest fields -> each rejected by verify_sqlite_archive!
    let base_manifest: ArchiveManifest =
        serde_json::from_str(&fs::read_to_string(&rep.manifest_path).unwrap()).unwrap();
    let manifest_mutations: &[(&str, ManifestMutator)] = &[
        ("archive_sha", |m| m.archive_sha256_hex = "0".repeat(64)),
        ("content_sha", |m| m.content_sha256_hex = "f".repeat(64)),
        ("counts_pending_draft", |m| m.counts.pending_draft += 1),
        ("account_trading_date", |m| {
            m.accounts[0].trading_date = "2099-12-31".to_owned();
        }),
        ("pending_status_in_final", |m| {
            m.status = ManifestStatus::Pending;
        }),
        ("half_null_next_epoch", |m| m.next_ledger_epoch = Some(2)),
    ];
    for &(label, mutate) in manifest_mutations {
        let mut mutated = base_manifest.clone();
        mutate(&mut mutated);
        let tdir = dir.path().join(format!("tamper_man_{label}"));
        fs::create_dir(&tdir).unwrap();
        fs::copy(&rep.archive_path, tdir.join("rich_snap.sqlite")).unwrap();
        let mpath = tdir.join("rich_snap.manifest.json");
        fs::write(&mpath, serde_json::to_vec_pretty(&mutated).unwrap()).unwrap();
        assert!(
            matches!(
                verify_sqlite_archive(&mpath).unwrap_err(),
                RolloverError::AuditFailed(_)
            ),
            "manifest mutation '{label}' must fail verification"
        );
    }

    // Self-produced manifest round-trip passes both dual validators!
    validate_archive_stem("rich_snap").unwrap();
    validate_archive_filename(&base_manifest.archive_filename, "rich_snap").unwrap();
    validate_archive_manifest(&base_manifest, "rich_snap", true).unwrap();
}

// ============================================================================
// 4. Single Delete Transaction Zero-Bypass Recovery Guards (F-17)
// ============================================================================

#[test]
fn single_delete_tx_zero_bypass_guards_prevent_all_five_misuse_paths() {
    // (a) Same-head_seq new idempotency receipt added after ARCHIVE_PUBLISHED -> 0 rows deleted!
    {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("same_head_seq.sqlite");
        let arch_dir = dir.path().join("arch");
        fs::create_dir(&arch_dir).unwrap();
        populate_simple_db(&db_path, false);

        let err = sqlite_rollover_with_fault(
            &SqliteRolloverRequest {
                db_path: db_path.clone(),
                archive_dir: arch_dir.clone(),
                archive_name: None,
                archive_only: false,
                allow_unresolved: false,
            },
            RolloverFaultInjection::AfterPendingManifestPublished,
        )
        .unwrap_err();
        assert_eq!(
            err,
            RolloverError::FaultInjected(RolloverFaultInjection::AfterPendingManifestPublished)
        );

        // Insert a new idempotency receipt WITHOUT changing head_seq!
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "INSERT INTO blotter_idempotency_keys
                 (actor_id, rpc_name, scope_key, idempotency_key, request_hash, committed_seq, response_payload)
                 VALUES ('sneaky', 'TestRpc', 'ACCT-01', 'k-late', zeroblob(32), 1, X'0102')",
                [],
            )
            .unwrap();
        }

        let retry_err = sqlite_rollover(&SqliteRolloverRequest {
            db_path: db_path.clone(),
            archive_dir: arch_dir.clone(),
            archive_name: None,
            archive_only: false,
            allow_unresolved: false,
        })
        .unwrap_err();
        assert!(matches!(retry_err, RolloverError::StateConflict(_)));

        // Verify ZERO rows deleted from blotter_accounts!
        let conn = Connection::open(&db_path).unwrap();
        let acct_cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM blotter_accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(acct_cnt, 1);
    }

    // (b) `--archive-only` encountering active `rollover_reset` -> ModeConflict, 0 deletions!
    {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("mode_conflict.sqlite");
        let arch_dir = dir.path().join("arch");
        fs::create_dir(&arch_dir).unwrap();
        populate_simple_db(&db_path, false);

        let _ = sqlite_rollover_with_fault(
            &SqliteRolloverRequest {
                db_path: db_path.clone(),
                archive_dir: arch_dir.clone(),
                archive_name: None,
                archive_only: false,
                allow_unresolved: false,
            },
            RolloverFaultInjection::AfterPendingManifestPublished,
        )
        .unwrap_err();

        let mode_err = sqlite_rollover(&SqliteRolloverRequest {
            db_path: db_path.clone(),
            archive_dir: arch_dir,
            archive_name: None,
            archive_only: true,
            allow_unresolved: false,
        })
        .unwrap_err();
        assert!(matches!(mode_err, RolloverError::ModeConflict(_)));

        let conn = Connection::open(&db_path).unwrap();
        let acct_cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM blotter_accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(acct_cnt, 1);
    }

    // (c) Dropping `--allow-unresolved` when resuming reset on DB with unresolved COMMITTED -> blocked, 0 deletions!
    {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("drop_override.sqlite");
        let arch_dir = dir.path().join("arch");
        fs::create_dir(&arch_dir).unwrap();
        populate_simple_db(&db_path, true);

        let _ = sqlite_rollover_with_fault(
            &SqliteRolloverRequest {
                db_path: db_path.clone(),
                archive_dir: arch_dir.clone(),
                archive_name: None,
                archive_only: false,
                allow_unresolved: true,
            },
            RolloverFaultInjection::AfterPendingManifestPublished,
        )
        .unwrap_err();

        let err = sqlite_rollover(&SqliteRolloverRequest {
            db_path: db_path.clone(),
            archive_dir: arch_dir,
            archive_name: None,
            archive_only: false,
            allow_unresolved: false,
        })
        .unwrap_err();
        assert_eq!(
            err,
            RolloverError::UnresolvedItemsBlocked {
                unresolved_committed: 1,
                open_pending_match: 0,
            }
        );

        let conn = Connection::open(&db_path).unwrap();
        let acct_cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM blotter_accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(acct_cnt, 1);
    }

    // (d) Tampered scheduled `next_ledger_instance_id` in state.json -> StateConflict, 0 deletions!
    {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("tamper_next_id.sqlite");
        let arch_dir = dir.path().join("arch");
        fs::create_dir(&arch_dir).unwrap();
        populate_simple_db(&db_path, false);

        let _ = sqlite_rollover_with_fault(
            &SqliteRolloverRequest {
                db_path: db_path.clone(),
                archive_dir: arch_dir.clone(),
                archive_name: None,
                archive_only: false,
                allow_unresolved: false,
            },
            RolloverFaultInjection::AfterPendingManifestPublished,
        )
        .unwrap_err();

        let sp = rollover_state_path(&db_path);
        let mut val: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&sp).unwrap()).unwrap();
        let bogus_next = generate_random_ulid().unwrap();
        val["active"]["next_ledger_instance_id"] = serde_json::Value::String(bogus_next);
        fs::write(&sp, serde_json::to_vec_pretty(&val).unwrap()).unwrap();

        let err = sqlite_rollover(&SqliteRolloverRequest {
            db_path: db_path.clone(),
            archive_dir: arch_dir,
            archive_name: None,
            archive_only: false,
            allow_unresolved: false,
        })
        .unwrap_err();
        assert!(matches!(err, RolloverError::StateConflict(_)));

        let conn = Connection::open(&db_path).unwrap();
        let acct_cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM blotter_accounts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(acct_cnt, 1);
    }

    // (e) Orphan `<stem>.sqlite` with no active state -> ArchiveConflict, orphan untouched!
    {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("orphan_test.sqlite");
        let arch_dir = dir.path().join("arch");
        fs::create_dir(&arch_dir).unwrap();
        populate_simple_db(&db_path, false);

        let orphan = arch_dir.join("orphan_stem.sqlite");
        fs::write(&orphan, b"foreign-orphan-bytes").unwrap();

        let err = sqlite_rollover(&SqliteRolloverRequest {
            db_path: db_path.clone(),
            archive_dir: arch_dir,
            archive_name: Some("orphan_stem".to_owned()),
            archive_only: false,
            allow_unresolved: false,
        })
        .unwrap_err();
        assert!(matches!(err, RolloverError::ArchiveConflict(_)));
        assert_eq!(fs::read(&orphan).unwrap(), b"foreign-orphan-bytes");
    }
}

// ============================================================================
// 5. All 6 Fault Injection Crash Windows + Default-Stem Recovery (F-02, F-22(2))
// ============================================================================

#[test]
fn six_window_crash_recovery_default_stem_and_completed_retry() {
    let windows = [
        RolloverFaultInjection::HalfWrittenStateTempFile,
        RolloverFaultInjection::AfterArchivePublished,
        RolloverFaultInjection::AfterPendingManifestPublished,
        RolloverFaultInjection::AfterSourceResetCommittedBeforeStateUpdate,
        RolloverFaultInjection::DuringMaintenanceVacuumOrCheckpoint,
        RolloverFaultInjection::AfterFinalManifestPublishedBeforeStateRetire,
    ];

    for window in windows {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join(format!("win_{window:?}.sqlite"));
        let arch_dir = dir.path().join("arch");
        fs::create_dir(&arch_dir).unwrap();
        populate_simple_db(&db_path, false);

        // Capture initial ledger identity (epoch 1).
        let initial_id = {
            let s = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
            s.ledger_identity().unwrap()
        };
        assert_eq!(initial_id.ledger_epoch, 1);

        // Trigger crash window using default naming (`archive_name: None`).
        let req = SqliteRolloverRequest {
            db_path: db_path.clone(),
            archive_dir: arch_dir.clone(),
            archive_name: None,
            archive_only: false,
            allow_unresolved: false,
        };
        let err = sqlite_rollover_with_fault(&req, window).unwrap_err();
        assert_eq!(err, RolloverError::FaultInjected(window));

        // For windows 2..=6, an active unfinished rollover exists in `.rollover.state.json`:
        // 1. `SqliteBlotterStore::open_file` MUST refuse with StorageUnavailable!
        // 2. Record the scheduled operation_id, stem, and next_ledger_instance_id to verify recovery reuses them!
        let mut expected_op_stem_next: Option<(String, String, String)> = None;
        if window != RolloverFaultInjection::HalfWrittenStateTempFile {
            let open_err =
                SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap_err();
            assert_eq!(
                open_err.category(),
                ApplicationErrorCategory::StorageUnavailable,
                "service must refuse to open DB during unfinished window {window:?}"
            );

            let state_json: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(rollover_state_path(&db_path)).unwrap())
                    .unwrap();
            let op_id = state_json["active"]["operation_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let stem = state_json["active"]["stem"].as_str().unwrap().to_owned();
            let next_id = state_json["active"]["next_ledger_instance_id"]
                .as_str()
                .unwrap()
                .to_owned();

            // Window 5 specifically MUST NOT have published final `<stem>.manifest.json` yet!
            if window == RolloverFaultInjection::DuringMaintenanceVacuumOrCheckpoint {
                assert_eq!(
                    state_json["active"]["phase"].as_str(),
                    Some("RESET_COMMITTED_MAINTENANCE_PENDING")
                );
                assert!(
                    !arch_dir.join(format!("{stem}.manifest.json")).exists(),
                    "COMPLETED_RESET manifest must NOT exist before maintenance completes"
                );
            }
            expected_op_stem_next = Some((op_id, stem, next_id));
        }

        // Retry with default naming (`archive_name: None`) -> recovers cleanly to COMPLETED_RESET!
        let rep = sqlite_rollover(&req).unwrap();
        assert_eq!(rep.phase, RolloverPhase::CompletedReset);
        assert_eq!(rep.manifest.status, ManifestStatus::CompletedReset);
        assert!(rep.manifest.source_reset_committed);
        assert!(rep.manifest.wal_truncated);
        assert_eq!(rep.manifest.source_ledger_epoch, 1);
        assert_eq!(
            rep.manifest.source_ledger_instance_id,
            initial_id.ledger_instance_id
        );
        assert_eq!(rep.manifest.next_ledger_epoch, Some(2));
        assert!(!rep.reused_completed_receipt);

        if let Some((exp_op, exp_stem, exp_next)) = expected_op_stem_next {
            assert_eq!(
                rep.operation_id, exp_op,
                "window {window:?} must resume same op"
            );
            assert_eq!(
                rep.stem, exp_stem,
                "window {window:?} must resume same stem"
            );
            assert_eq!(
                rep.manifest.next_ledger_instance_id.as_deref(),
                Some(exp_next.as_str()),
                "window {window:?} must preserve scheduled next_ledger_instance_id"
            );
        }

        // Pending manifest cleaned up; final manifest & archive pass independent verification!
        assert!(
            !arch_dir
                .join(format!("{}.manifest.pending.json", rep.stem))
                .exists()
        );
        verify_sqlite_archive(&rep.manifest_path).unwrap();

        // Post-close WAL must be absent or 0 bytes!
        let wal = wal_path(&db_path);
        assert!(
            !wal.exists() || fs::metadata(&wal).unwrap().len() == 0,
            "post-reset WAL must be absent or 0 bytes for window {window:?}"
        );

        // Immediate retry with default naming returns the exact same completed receipt (epoch NOT incremented again!).
        let repeat = sqlite_rollover(&req).unwrap();
        assert!(repeat.reused_completed_receipt);
        assert_eq!(repeat.operation_id, rep.operation_id);
        assert_eq!(repeat.stem, rep.stem);
        assert_eq!(repeat.manifest, rep.manifest);

        // Opening the reset store succeeds, sees epoch=2 (strictly incremented once!) and clean empty book!
        let store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
        let post_id = store.ledger_identity().unwrap();
        assert_eq!(post_id.ledger_epoch, 2);
        assert_eq!(
            Some(post_id.ledger_instance_id.as_str()),
            rep.manifest.next_ledger_instance_id.as_deref()
        );
        assert_eq!(
            post_id.last_reset_operation_id.as_deref(),
            Some(rep.operation_id.as_str())
        );
        assert_eq!(store.head_seq().unwrap(), 0);
        assert_eq!(store.oldest_retained_seq().unwrap(), 1);
        drop(store);
    }
}

#[test]
fn new_trading_day_content_starts_next_epoch_rollover_automatically() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("day1_to_day2.sqlite");
    let arch_dir = dir.path().join("arch");
    fs::create_dir(&arch_dir).unwrap();
    populate_simple_db(&db_path, false);

    let req = SqliteRolloverRequest {
        db_path: db_path.clone(),
        archive_dir: arch_dir,
        archive_name: None,
        archive_only: false,
        allow_unresolved: false,
    };
    let day1_rep = sqlite_rollover(&req).unwrap();
    assert_eq!(day1_rep.manifest.source_ledger_epoch, 1);
    assert_eq!(day1_rep.manifest.next_ledger_epoch, Some(2));

    // Import Day-2 opening base -> epoch 2 now has business content!
    {
        let store = SqliteBlotterStore::open_file(&db_path, BlotterConfig::default()).unwrap();
        let clock = ManualBlotterClock::new(shanghai_time(2025, 6, 17, 9, 0, 0));
        let orch = orchestrator(&clock);
        let day2_base = make_opening_base(
            "ACCT-01",
            ulid_pair('D', '2'),
            "FEED-A",
            "BOND_10Y",
            date(2025, 6, 17),
            date(2025, 6, 18),
            0,
        );
        import_account(
            &store,
            &orch,
            "admin",
            "imp-d2",
            day2_base,
            test_policy("QSRC-A"),
        );
    }

    // Default rollover now automatically opens a NEW operation for epoch 2 -> 3!
    let day2_rep = sqlite_rollover(&req).unwrap();
    assert!(!day2_rep.reused_completed_receipt);
    assert_ne!(day2_rep.operation_id, day1_rep.operation_id);
    assert_eq!(day2_rep.manifest.source_ledger_epoch, 2);
    assert_eq!(
        Some(day2_rep.manifest.source_ledger_instance_id.as_str()),
        day1_rep.manifest.next_ledger_instance_id.as_deref()
    );
    assert_eq!(day2_rep.manifest.next_ledger_epoch, Some(3));
    verify_sqlite_archive(&day2_rep.manifest_path).unwrap();
}
