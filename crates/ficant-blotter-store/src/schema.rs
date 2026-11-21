use std::collections::BTreeSet;
use std::path::Path;

use ficant_application::{ApplicationError, ApplicationErrorCategory, ports::ApplicationResult};
use ficant_domain::blotter::BlotterConfig;
use rusqlite::{Connection, OptionalExtension, params};

use crate::codec::config_to_json;

/// Current business schema version stored explicitly in `blotter_meta.schema_version`.
///
/// Never confused with `SQLite`'s internal DDL cookie `PRAGMA schema_version`.
pub const BLOTTER_SCHEMA_VERSION: u32 = 1;

/// Complete list of required business tables in schema v1.
pub const REQUIRED_TABLES: &[&str] = &[
    "blotter_meta",
    "blotter_accounts",
    "blotter_instruments",
    "blotter_positions",
    "blotter_earmarks",
    "blotter_draft_earmarks",
    "blotter_feed_status",
    "blotter_feed_raw_packets",
    "blotter_ingested_trades",
    "blotter_trade_effective_revisions",
    "blotter_pending_matches",
    "blotter_reconcile_allocations",
    "blotter_quote_history",
    "blotter_quote_heads",
    "blotter_idempotency_keys",
    "blotter_event_journal",
    "blotter_event_accounts",
];

/// Complete business DDL shared identically by production WAL databases and explicit ephemeral
/// in-memory test databases.
pub const BLOTTER_SCHEMA_DDL: &str = r"
CREATE TABLE blotter_meta (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    head_seq INTEGER NOT NULL CHECK (head_seq >= 0),
    oldest_retained_seq INTEGER NOT NULL CHECK (oldest_retained_seq >= 1),
    config_json TEXT NOT NULL
);

CREATE TABLE blotter_accounts (
    account_id TEXT PRIMARY KEY NOT NULL,
    base_id TEXT NOT NULL UNIQUE,
    trading_date TEXT NOT NULL,
    next_settlement_date TEXT NOT NULL,
    effective_from_json TEXT NOT NULL,
    watermark_time_json TEXT NOT NULL,
    watermark_source_seq INTEGER CHECK (watermark_source_seq IS NULL OR watermark_source_seq >= 0),
    source_id TEXT NOT NULL,
    source_start_after_feed_seq INTEGER NOT NULL CHECK (source_start_after_feed_seq >= 0),
    opening_cash_t0 TEXT NOT NULL,
    opening_net_cash_delta_t1 TEXT NOT NULL,
    account_version INTEGER NOT NULL CHECK (account_version >= 1),
    policy_version INTEGER NOT NULL CHECK (policy_version >= 1),
    default_quote_source_id TEXT NOT NULL,
    import_actor_id TEXT NOT NULL,
    import_idempotency_key TEXT NOT NULL,
    import_request_hash BLOB NOT NULL CHECK (length(import_request_hash) = 32),
    import_committed_seq INTEGER NOT NULL CHECK (import_committed_seq >= 1),
    opening_base_json TEXT NOT NULL,
    policy_json TEXT NOT NULL
);

CREATE TABLE blotter_instruments (
    account_id TEXT NOT NULL REFERENCES blotter_accounts(account_id) ON DELETE RESTRICT,
    instrument_code TEXT NOT NULL,
    active_quote_source_id TEXT NOT NULL,
    policy_version INTEGER NOT NULL CHECK (policy_version >= 1),
    has_catalog_entry INTEGER NOT NULL CHECK (has_catalog_entry IN (0, 1)),
    catalog_entry_json TEXT,
    PRIMARY KEY (account_id, instrument_code)
);

CREATE TABLE blotter_positions (
    account_id TEXT NOT NULL REFERENCES blotter_accounts(account_id) ON DELETE RESTRICT,
    instrument_code TEXT NOT NULL,
    position_version INTEGER NOT NULL CHECK (position_version >= 1),
    opening_economic_notional TEXT NOT NULL,
    opening_available_notional TEXT NOT NULL,
    opening_net_t0_notional TEXT NOT NULL,
    opening_net_t1_notional TEXT NOT NULL,
    reconciled_buy_t0 TEXT NOT NULL,
    reconciled_buy_t1 TEXT NOT NULL,
    reconciled_sell_t0 TEXT NOT NULL,
    reconciled_sell_t1 TEXT NOT NULL,
    PRIMARY KEY (account_id, instrument_code)
);

CREATE TABLE blotter_feed_status (
    source_id TEXT PRIMARY KEY NOT NULL,
    feed_status_revision INTEGER NOT NULL CHECK (feed_status_revision >= 1),
    initial_after_feed_seq INTEGER NOT NULL CHECK (initial_after_feed_seq >= 0),
    contiguous_feed_seq INTEGER NOT NULL CHECK (contiguous_feed_seq >= 0),
    max_seen_feed_seq INTEGER NOT NULL CHECK (max_seen_feed_seq >= 0),
    missing_feed_ranges_json TEXT NOT NULL,
    paused_by_gap INTEGER NOT NULL CHECK (paused_by_gap IN (0, 1)),
    paused_by_staleness INTEGER NOT NULL CHECK (paused_by_staleness IN (0, 1)),
    paused_by_operator INTEGER NOT NULL CHECK (paused_by_operator IN (0, 1)),
    last_heartbeat_at_json TEXT,
    status_reason TEXT NOT NULL,
    source_batches_json TEXT NOT NULL
);

CREATE TABLE blotter_feed_raw_packets (
    source_id TEXT NOT NULL REFERENCES blotter_feed_status(source_id) ON DELETE RESTRICT,
    feed_seq INTEGER NOT NULL CHECK (feed_seq >= 1),
    external_trade_id TEXT NOT NULL,
    source_revision INTEGER NOT NULL CHECK (source_revision >= 1),
    payload_hash BLOB NOT NULL CHECK (length(payload_hash) = 32),
    business_hash BLOB NOT NULL CHECK (length(business_hash) = 32),
    batch_first_feed_seq INTEGER NOT NULL CHECK (batch_first_feed_seq >= 1),
    batch_last_feed_seq INTEGER NOT NULL CHECK (batch_last_feed_seq >= batch_first_feed_seq),
    record_json TEXT NOT NULL,
    PRIMARY KEY (source_id, feed_seq)
);

CREATE TABLE blotter_ingested_trades (
    source_id TEXT NOT NULL REFERENCES blotter_feed_status(source_id) ON DELETE RESTRICT,
    external_trade_id TEXT NOT NULL,
    source_revision INTEGER NOT NULL CHECK (source_revision >= 1),
    business_hash BLOB NOT NULL CHECK (length(business_hash) = 32),
    canonical_feed_seq INTEGER NOT NULL CHECK (canonical_feed_seq >= 1),
    alias_feed_seqs_json TEXT NOT NULL,
    lifecycle TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY (source_id, external_trade_id, source_revision)
);

CREATE TABLE blotter_trade_effective_revisions (
    source_id TEXT NOT NULL REFERENCES blotter_feed_status(source_id) ON DELETE RESTRICT,
    external_trade_id TEXT NOT NULL,
    latest_effective_revision INTEGER CHECK (latest_effective_revision IS NULL OR latest_effective_revision >= 1),
    original_new_fill_json TEXT,
    baseline_original_evidence_json TEXT,
    held_revisions_json TEXT NOT NULL,
    missing_predecessor_revisions_json TEXT NOT NULL,
    PRIMARY KEY (source_id, external_trade_id)
);

CREATE TABLE blotter_earmarks (
    earmark_id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL,
    instrument_code TEXT NOT NULL,
    earmark_revision INTEGER NOT NULL CHECK (earmark_revision >= 1),
    side TEXT NOT NULL,
    settlement_bucket TEXT NOT NULL,
    settlement_date TEXT NOT NULL,
    state TEXT NOT NULL,
    notional TEXT NOT NULL,
    unreconciled_notional TEXT NOT NULL,
    locked_settlement_cash TEXT,
    unreconciled_settlement_cash TEXT,
    funding_precision TEXT NOT NULL,
    created_by TEXT NOT NULL,
    confirmed_by_trader_id TEXT,
    created_seq INTEGER NOT NULL CHECK (created_seq >= 0),
    updated_seq INTEGER NOT NULL CHECK (updated_seq >= 0),
    record_json TEXT NOT NULL,
    FOREIGN KEY (account_id, instrument_code)
        REFERENCES blotter_positions(account_id, instrument_code) ON DELETE RESTRICT
);

CREATE TABLE blotter_draft_earmarks (
    draft_id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL,
    instrument_code TEXT NOT NULL,
    draft_revision INTEGER NOT NULL CHECK (draft_revision >= 1),
    status TEXT NOT NULL,
    created_seq INTEGER NOT NULL CHECK (created_seq >= 0),
    confirmation_token BLOB NOT NULL CHECK (length(confirmation_token) = 32),
    context_hash BLOB NOT NULL CHECK (length(context_hash) = 32),
    confirmed_by_trader_id TEXT,
    record_json TEXT,
    FOREIGN KEY (account_id, instrument_code)
        REFERENCES blotter_positions(account_id, instrument_code) ON DELETE RESTRICT
);

CREATE TABLE blotter_pending_matches (
    pending_match_id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL,
    instrument_code TEXT NOT NULL,
    pending_revision INTEGER NOT NULL CHECK (pending_revision >= 1),
    source_id TEXT NOT NULL,
    external_trade_id TEXT NOT NULL,
    source_revision INTEGER NOT NULL CHECK (source_revision >= 1),
    state TEXT NOT NULL,
    reason TEXT NOT NULL,
    original_notional TEXT NOT NULL,
    unresolved_notional TEXT NOT NULL,
    unresolved_signed_cash TEXT,
    resolution_actor_id TEXT,
    resolution_reason TEXT,
    created_seq INTEGER NOT NULL CHECK (created_seq >= 0),
    updated_seq INTEGER NOT NULL CHECK (updated_seq >= 0),
    record_json TEXT NOT NULL,
    FOREIGN KEY (account_id, instrument_code)
        REFERENCES blotter_positions(account_id, instrument_code) ON DELETE RESTRICT,
    FOREIGN KEY (source_id, external_trade_id, source_revision)
        REFERENCES blotter_ingested_trades(source_id, external_trade_id, source_revision) ON DELETE RESTRICT
);

CREATE TABLE blotter_reconcile_allocations (
    allocation_id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL REFERENCES blotter_accounts(account_id) ON DELETE RESTRICT,
    instrument_code TEXT NOT NULL,
    source_id TEXT NOT NULL,
    external_trade_id TEXT NOT NULL,
    source_revision INTEGER NOT NULL CHECK (source_revision >= 1),
    earmark_id TEXT REFERENCES blotter_earmarks(earmark_id) ON DELETE RESTRICT,
    pending_match_id TEXT REFERENCES blotter_pending_matches(pending_match_id) ON DELETE RESTRICT,
    allocated_notional TEXT NOT NULL,
    allocated_fill_cash TEXT,
    released_earmark_cash TEXT,
    fill_segment_index INTEGER CHECK (fill_segment_index IS NULL OR fill_segment_index >= 1),
    earmark_segment_index INTEGER CHECK (earmark_segment_index IS NULL OR earmark_segment_index >= 1),
    reversed_by_revision INTEGER CHECK (reversed_by_revision IS NULL OR reversed_by_revision >= 1),
    actor_id TEXT NOT NULL,
    baseline_delta_json TEXT,
    record_json TEXT NOT NULL,
    FOREIGN KEY (source_id, external_trade_id, source_revision)
        REFERENCES blotter_ingested_trades(source_id, external_trade_id, source_revision) ON DELETE RESTRICT
);

CREATE TABLE blotter_quote_history (
    source_id TEXT NOT NULL,
    instrument_code TEXT NOT NULL,
    quote_seq INTEGER NOT NULL CHECK (quote_seq >= 1),
    payload_hash BLOB NOT NULL CHECK (length(payload_hash) = 32),
    first_received_at_json TEXT NOT NULL,
    first_ingested_at_json TEXT NOT NULL,
    tick_json TEXT NOT NULL,
    PRIMARY KEY (source_id, instrument_code, quote_seq)
);

CREATE TABLE blotter_quote_heads (
    source_id TEXT NOT NULL,
    instrument_code TEXT NOT NULL,
    head_quote_seq INTEGER NOT NULL CHECK (head_quote_seq >= 1),
    PRIMARY KEY (source_id, instrument_code),
    FOREIGN KEY (source_id, instrument_code, head_quote_seq)
        REFERENCES blotter_quote_history(source_id, instrument_code, quote_seq) ON DELETE RESTRICT
);

CREATE TABLE blotter_idempotency_keys (
    actor_id TEXT NOT NULL,
    rpc_name TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash BLOB NOT NULL CHECK (length(request_hash) = 32),
    committed_seq INTEGER NOT NULL CHECK (committed_seq >= 0),
    response_payload BLOB NOT NULL,
    PRIMARY KEY (actor_id, rpc_name, scope_key, idempotency_key)
);

CREATE TABLE blotter_event_journal (
    seq INTEGER PRIMARY KEY NOT NULL CHECK (seq >= 1),
    occurred_at_json TEXT NOT NULL,
    event_type TEXT NOT NULL,
    envelope_json TEXT NOT NULL
);

CREATE TABLE blotter_event_accounts (
    seq INTEGER NOT NULL REFERENCES blotter_event_journal(seq) ON DELETE CASCADE,
    account_id TEXT NOT NULL,
    PRIMARY KEY (seq, account_id)
);

CREATE INDEX idx_blotter_earmarks_acct_state
    ON blotter_earmarks(account_id, state);
CREATE INDEX idx_blotter_pending_acct_state
    ON blotter_pending_matches(account_id, state);
CREATE INDEX idx_blotter_allocations_acct_trade
    ON blotter_reconcile_allocations(account_id, source_id, external_trade_id, source_revision);
CREATE INDEX idx_blotter_event_accounts_acct_seq
    ON blotter_event_accounts(account_id, seq);
";

/// Observed `SQLite` connection PRAGMAs and explicit business schema version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlitePragmaSnapshot {
    pub journal_mode: String,
    pub locking_mode: String,
    pub synchronous: i64,
    pub foreign_keys: i64,
    pub business_schema_version: u32,
}

/// Validates that a production database path is a genuine local file path and rejects all
/// in-memory pseudo-paths, `file:` URIs, empty strings, and directories.
pub fn validate_production_sqlite_path(path: &Path) -> ApplicationResult<()> {
    let Some(raw) = path.to_str() else {
        return Err(validation_failed());
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed != raw {
        return Err(validation_failed());
    }
    if raw.contains('\0') {
        return Err(validation_failed());
    }

    let lower = raw.to_ascii_lowercase();
    if lower == ":memory:"
        || lower.starts_with(':')
        || lower.starts_with("file:")
        || lower.contains(":memory:")
        || lower.contains("mode=memory")
        || lower.contains("vfs=memdb")
    {
        return Err(validation_failed());
    }

    if path.is_dir() || path.file_name().is_none() {
        return Err(validation_failed());
    }

    // Reject a path ending in a directory separator (e.g. "dir/" or "dir\").
    if raw.ends_with('/') || raw.ends_with('\\') {
        return Err(validation_failed());
    }

    Ok(())
}

/// Configures `SQLite` PRAGMAs (zero-wait exclusive locking, WAL for file-backed DBs, FULL sync,
/// foreign keys) and verifies every PRAGMA readback before touching schema.
pub fn configure_and_verify_pragmas(
    conn: &Connection,
    is_ephemeral_in_memory: bool,
) -> ApplicationResult<()> {
    conn.execute_batch(
        "PRAGMA busy_timeout = 0;
         PRAGMA foreign_keys = ON;
         PRAGMA synchronous = FULL;",
    )
    .map_err(|_| storage_unavailable())?;

    if !is_ephemeral_in_memory {
        let locking_mode: String = conn
            .query_row("PRAGMA locking_mode = EXCLUSIVE", [], |row| row.get(0))
            .map_err(|_| storage_unavailable())?;
        if !locking_mode.eq_ignore_ascii_case("exclusive") {
            return Err(storage_unavailable());
        }

        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(|_| storage_unavailable())?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            return Err(storage_unavailable());
        }
    }

    let synchronous: i64 = conn
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .map_err(|_| storage_unavailable())?;
    if synchronous != 2 {
        return Err(storage_unavailable());
    }

    let foreign_keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(|_| storage_unavailable())?;
    if foreign_keys != 1 {
        return Err(storage_unavailable());
    }

    if !is_ephemeral_in_memory {
        let verified_locking: String = conn
            .query_row("PRAGMA locking_mode", [], |row| row.get(0))
            .map_err(|_| storage_unavailable())?;
        if !verified_locking.eq_ignore_ascii_case("exclusive") {
            return Err(storage_unavailable());
        }

        let verified_journal: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|_| storage_unavailable())?;
        if !verified_journal.eq_ignore_ascii_case("wal") {
            return Err(storage_unavailable());
        }
    }

    Ok(())
}

/// Initializes a verified empty database or validates an existing database's required tables and
/// explicit `blotter_meta.schema_version` while acquiring the exclusive write lock.
pub fn initialize_or_verify_schema(
    conn: &Connection,
    default_config: &BlotterConfig,
) -> ApplicationResult<()> {
    let config_json = config_to_json(default_config)?;

    // Acquire an immediate write transaction so exclusive locking is actively held from open.
    conn.execute_batch("BEGIN IMMEDIATE;")
        .map_err(|_| storage_unavailable())?;

    let result = (|| -> ApplicationResult<()> {
        let existing_object_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type IN ('table', 'index', 'trigger', 'view')
                   AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| storage_unavailable())?;

        if existing_object_count == 0 {
            conn.execute_batch(BLOTTER_SCHEMA_DDL)
                .map_err(|_| storage_unavailable())?;
            conn.execute(
                "INSERT INTO blotter_meta
                 (singleton_id, schema_version, head_seq, oldest_retained_seq, config_json)
                 VALUES (1, ?1, 0, 1, ?2)",
                params![i64::from(BLOTTER_SCHEMA_VERSION), config_json],
            )
            .map_err(|_| storage_unavailable())?;
        } else {
            verify_existing_tables(conn)?;
            verify_business_meta_version(conn)?;
            // Touch singleton row in-place to ensure an actual write occurs inside this
            // exclusive-mode transaction so SQLite retains the exclusive lock for connection life.
            let touched = conn
                .execute(
                    "UPDATE blotter_meta
                     SET schema_version = schema_version
                     WHERE singleton_id = 1",
                    [],
                )
                .map_err(|_| storage_unavailable())?;
            if touched != 1 {
                return Err(storage_unavailable());
            }
        }
        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT;")
                .map_err(|_| storage_unavailable())?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(err)
        }
    }
}

/// Reads back actual PRAGMAs and explicit `blotter_meta.schema_version`.
pub fn read_pragma_snapshot(conn: &Connection) -> ApplicationResult<SqlitePragmaSnapshot> {
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(|_| storage_unavailable())?;
    let locking_mode: String = conn
        .query_row("PRAGMA locking_mode", [], |row| row.get(0))
        .map_err(|_| storage_unavailable())?;
    let synchronous: i64 = conn
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .map_err(|_| storage_unavailable())?;
    let foreign_keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(|_| storage_unavailable())?;
    let version_raw: i64 = conn
        .query_row(
            "SELECT schema_version FROM blotter_meta WHERE singleton_id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| storage_unavailable())?;
    let business_schema_version = u32::try_from(version_raw).map_err(|_| storage_unavailable())?;

    Ok(SqlitePragmaSnapshot {
        journal_mode: journal_mode.to_ascii_lowercase(),
        locking_mode: locking_mode.to_ascii_lowercase(),
        synchronous,
        foreign_keys,
        business_schema_version,
    })
}

fn verify_existing_tables(conn: &Connection) -> ApplicationResult<()> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )
        .map_err(|_| storage_unavailable())?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| storage_unavailable())?;
    let mut present = BTreeSet::new();
    for name in names {
        present.insert(name.map_err(|_| storage_unavailable())?);
    }

    for &required in REQUIRED_TABLES {
        if !present.contains(required) {
            return Err(storage_unavailable());
        }
    }
    Ok(())
}

fn verify_business_meta_version(conn: &Connection) -> ApplicationResult<()> {
    let row_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM blotter_meta", [], |row| row.get(0))
        .map_err(|_| storage_unavailable())?;
    if row_count != 1 {
        return Err(storage_unavailable());
    }

    let version_opt: Option<i64> = conn
        .query_row(
            "SELECT schema_version FROM blotter_meta WHERE singleton_id = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| storage_unavailable())?;
    let Some(version) = version_opt else {
        return Err(storage_unavailable());
    };
    if version != i64::from(BLOTTER_SCHEMA_VERSION) {
        return Err(storage_unavailable());
    }
    Ok(())
}

const fn validation_failed() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

const fn storage_unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
}
