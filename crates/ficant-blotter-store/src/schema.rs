use std::collections::BTreeSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use ficant_application::{ApplicationError, ApplicationErrorCategory, ports::ApplicationResult};
use ficant_domain::blotter::BlotterConfig;
use rusqlite::{Connection, OptionalExtension, params};

use crate::codec::config_to_json;
use crate::rollover::{PhysicalFileId, derive_legacy_v1_identity_on_conn};

/// Current business schema version stored explicitly in `blotter_meta.schema_version`.
///
/// Never confused with `SQLite`'s internal DDL cookie `PRAGMA schema_version`.
pub const BLOTTER_SCHEMA_VERSION: u32 = 1;

/// Exact column list of `blotter_meta` in legacy BP-2/BP-3 v1 databases.
pub const BLOTTER_META_LEGACY_COLUMNS: &[&str] = &[
    "singleton_id",
    "schema_version",
    "head_seq",
    "oldest_retained_seq",
    "config_json",
];

/// Exact column list of `blotter_meta` with BP-4 ledger identity extension columns.
pub const BLOTTER_META_EXTENDED_COLUMNS: &[&str] = &[
    "singleton_id",
    "schema_version",
    "head_seq",
    "oldest_retained_seq",
    "config_json",
    "ledger_epoch",
    "ledger_instance_id",
    "last_reset_operation_id",
];

/// Complete list of required business tables in schema v1 (fixed canonical order).
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

/// Authoritative column specification for each non-meta business table in schema v1.
pub const REQUIRED_NON_META_TABLE_COLUMNS: &[(&str, &[&str])] = &[
    (
        "blotter_accounts",
        &[
            "account_id",
            "base_id",
            "trading_date",
            "next_settlement_date",
            "effective_from_json",
            "watermark_time_json",
            "watermark_source_seq",
            "source_id",
            "source_start_after_feed_seq",
            "opening_cash_t0",
            "opening_net_cash_delta_t1",
            "account_version",
            "policy_version",
            "default_quote_source_id",
            "import_actor_id",
            "import_idempotency_key",
            "import_request_hash",
            "import_committed_seq",
            "opening_base_json",
            "policy_json",
        ],
    ),
    (
        "blotter_instruments",
        &[
            "account_id",
            "instrument_code",
            "active_quote_source_id",
            "policy_version",
            "has_catalog_entry",
            "catalog_entry_json",
        ],
    ),
    (
        "blotter_positions",
        &[
            "account_id",
            "instrument_code",
            "position_version",
            "opening_economic_notional",
            "opening_available_notional",
            "opening_net_t0_notional",
            "opening_net_t1_notional",
            "reconciled_buy_t0",
            "reconciled_buy_t1",
            "reconciled_sell_t0",
            "reconciled_sell_t1",
        ],
    ),
    (
        "blotter_earmarks",
        &[
            "earmark_id",
            "account_id",
            "instrument_code",
            "earmark_revision",
            "side",
            "settlement_bucket",
            "settlement_date",
            "state",
            "notional",
            "unreconciled_notional",
            "locked_settlement_cash",
            "unreconciled_settlement_cash",
            "funding_precision",
            "created_by",
            "confirmed_by_trader_id",
            "created_seq",
            "updated_seq",
            "record_json",
        ],
    ),
    (
        "blotter_draft_earmarks",
        &[
            "draft_id",
            "account_id",
            "instrument_code",
            "draft_revision",
            "status",
            "created_seq",
            "confirmation_token",
            "context_hash",
            "confirmed_by_trader_id",
            "record_json",
        ],
    ),
    (
        "blotter_feed_status",
        &[
            "source_id",
            "feed_status_revision",
            "initial_after_feed_seq",
            "contiguous_feed_seq",
            "max_seen_feed_seq",
            "missing_feed_ranges_json",
            "paused_by_gap",
            "paused_by_staleness",
            "paused_by_operator",
            "last_heartbeat_at_json",
            "status_reason",
            "source_batches_json",
        ],
    ),
    (
        "blotter_feed_raw_packets",
        &[
            "source_id",
            "feed_seq",
            "external_trade_id",
            "source_revision",
            "payload_hash",
            "business_hash",
            "batch_first_feed_seq",
            "batch_last_feed_seq",
            "record_json",
        ],
    ),
    (
        "blotter_ingested_trades",
        &[
            "source_id",
            "external_trade_id",
            "source_revision",
            "business_hash",
            "canonical_feed_seq",
            "alias_feed_seqs_json",
            "lifecycle",
            "record_json",
        ],
    ),
    (
        "blotter_trade_effective_revisions",
        &[
            "source_id",
            "external_trade_id",
            "latest_effective_revision",
            "original_new_fill_json",
            "baseline_original_evidence_json",
            "held_revisions_json",
            "missing_predecessor_revisions_json",
        ],
    ),
    (
        "blotter_pending_matches",
        &[
            "pending_match_id",
            "account_id",
            "instrument_code",
            "pending_revision",
            "source_id",
            "external_trade_id",
            "source_revision",
            "state",
            "reason",
            "original_notional",
            "unresolved_notional",
            "unresolved_signed_cash",
            "resolution_actor_id",
            "resolution_reason",
            "created_seq",
            "updated_seq",
            "record_json",
        ],
    ),
    (
        "blotter_reconcile_allocations",
        &[
            "allocation_id",
            "account_id",
            "instrument_code",
            "source_id",
            "external_trade_id",
            "source_revision",
            "earmark_id",
            "pending_match_id",
            "allocated_notional",
            "allocated_fill_cash",
            "released_earmark_cash",
            "fill_segment_index",
            "earmark_segment_index",
            "reversed_by_revision",
            "actor_id",
            "baseline_delta_json",
            "record_json",
        ],
    ),
    (
        "blotter_quote_history",
        &[
            "source_id",
            "instrument_code",
            "quote_seq",
            "payload_hash",
            "first_received_at_json",
            "first_ingested_at_json",
            "tick_json",
        ],
    ),
    (
        "blotter_quote_heads",
        &["source_id", "instrument_code", "head_quote_seq"],
    ),
    (
        "blotter_idempotency_keys",
        &[
            "actor_id",
            "rpc_name",
            "scope_key",
            "idempotency_key",
            "request_hash",
            "committed_seq",
            "response_payload",
        ],
    ),
    (
        "blotter_event_journal",
        &["seq", "occurred_at_json", "event_type", "envelope_json"],
    ),
    ("blotter_event_accounts", &["seq", "account_id"]),
];

/// Complete business DDL shared identically by production WAL databases and explicit ephemeral
/// in-memory test databases.
///
/// The three ledger identity extension columns on `blotter_meta` remain SQL-nullable so legacy
/// 5-column `INSERT INTO blotter_meta` fixtures continue to execute while Rust validation fails
/// closed with `StorageUnavailable` whenever extension columns exist and are NULL, half-null,
/// or malformed.
pub const BLOTTER_SCHEMA_DDL: &str = r"
CREATE TABLE blotter_meta (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    head_seq INTEGER NOT NULL CHECK (head_seq >= 0),
    oldest_retained_seq INTEGER NOT NULL CHECK (oldest_retained_seq >= 1),
    config_json TEXT NOT NULL,
    ledger_epoch INTEGER CHECK (ledger_epoch IS NULL OR ledger_epoch >= 1),
    ledger_instance_id TEXT,
    last_reset_operation_id TEXT
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

/// Committed identity of a Desk Blotter ledger instance persisted in `blotter_meta`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerIdentity {
    pub ledger_epoch: u64,
    pub ledger_instance_id: String,
    pub last_reset_operation_id: Option<String>,
}

/// Detected column layout of `blotter_meta`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaSchemaLayout {
    /// Legacy BP-2/BP-3 5-column layout without identity extension columns.
    V1Legacy,
    /// BP-4 8-column layout with `ledger_epoch`, `ledger_instance_id`, `last_reset_operation_id`.
    V1WithIdentity,
}

/// Deterministic fault injection for testing startup schema/identity migration rollback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartupSchemaFault {
    #[default]
    None,
    /// Rolls back the startup `BEGIN IMMEDIATE` transaction right before `COMMIT`.
    FailBeforeSchemaCommit,
}

/// Observed `SQLite` connection PRAGMAs and explicit business schema version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlitePragmaSnapshot {
    pub journal_mode: String,
    pub locking_mode: String,
    pub synchronous: i64,
    pub foreign_keys: i64,
    pub business_schema_version: u32,
}

/// Returns `true` iff `value` is a canonical 26-character uppercase Crockford Base32 ULID.
#[must_use]
pub fn is_canonical_ulid(value: &str) -> bool {
    if value.len() != 26 || !value.is_ascii() {
        return false;
    }
    ulid::Ulid::from_string(value).is_ok_and(|parsed| parsed.to_string() == value)
}

/// Validates that `value` is a canonical 26-character uppercase ULID.
pub fn validate_canonical_ulid(value: &str) -> ApplicationResult<String> {
    if is_canonical_ulid(value) {
        Ok(value.to_owned())
    } else {
        Err(storage_unavailable())
    }
}

/// Generates a new canonical 26-character uppercase ULID using `getrandom::fill`, checked
/// millisecond timestamp, and `ulid::Ulid::from_parts` (workspace `ulid` has default features off).
pub fn generate_random_ulid() -> ApplicationResult<String> {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| storage_unavailable())?;
    let timestamp_ms = u64::try_from(dur.as_millis()).map_err(|_| storage_unavailable())?;
    if timestamp_ms >= (1_u64 << 48) {
        return Err(storage_unavailable());
    }
    let mut entropy = [0_u8; 16];
    getrandom::fill(&mut entropy).map_err(|_| storage_unavailable())?;
    let rand_u128 = u128::from_be_bytes(entropy);
    let generated = ulid::Ulid::from_parts(timestamp_ms, rand_u128).to_string();
    validate_canonical_ulid(&generated)
}

/// Generates a new random ULID guaranteed to differ from `avoid`.
pub fn generate_distinct_random_ulid(avoid: &str) -> ApplicationResult<String> {
    for _ in 0..16 {
        let candidate = generate_random_ulid()?;
        if candidate != avoid {
            return Ok(candidate);
        }
    }
    Err(storage_unavailable())
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

    let busy_timeout: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .map_err(|_| storage_unavailable())?;
    if busy_timeout != 0 {
        return Err(storage_unavailable());
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

/// Inspects `PRAGMA table_info(blotter_meta)` and accepts ONLY the exact legacy 5-column layout
/// or the exact 8-column layout with ledger identity extension columns.
pub fn inspect_meta_layout(conn: &Connection) -> ApplicationResult<MetaSchemaLayout> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(blotter_meta)")
        .map_err(|_| storage_unavailable())?;
    let cols = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| storage_unavailable())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| storage_unavailable())?;
    let col_refs: Vec<&str> = cols.iter().map(String::as_str).collect();
    if col_refs.as_slice() == BLOTTER_META_LEGACY_COLUMNS {
        Ok(MetaSchemaLayout::V1Legacy)
    } else if col_refs.as_slice() == BLOTTER_META_EXTENDED_COLUMNS {
        Ok(MetaSchemaLayout::V1WithIdentity)
    } else {
        Err(storage_unavailable())
    }
}

/// Installs the three ledger identity extension columns onto a legacy v1 `blotter_meta` table
/// and persists `identity` inside the caller's active transaction.
pub fn install_meta_identity_columns(
    conn: &Connection,
    identity: &LedgerIdentity,
) -> ApplicationResult<()> {
    if identity.ledger_epoch < 1 || !is_canonical_ulid(&identity.ledger_instance_id) {
        return Err(storage_unavailable());
    }
    if let Some(op_id) = &identity.last_reset_operation_id
        && !is_canonical_ulid(op_id)
    {
        return Err(storage_unavailable());
    }
    let epoch_i64 = i64::try_from(identity.ledger_epoch).map_err(|_| storage_unavailable())?;
    conn.execute_batch(
        "ALTER TABLE blotter_meta ADD COLUMN ledger_epoch INTEGER CHECK (ledger_epoch IS NULL OR ledger_epoch >= 1);
         ALTER TABLE blotter_meta ADD COLUMN ledger_instance_id TEXT;
         ALTER TABLE blotter_meta ADD COLUMN last_reset_operation_id TEXT;",
    )
    .map_err(|_| storage_unavailable())?;
    let updated = conn
        .execute(
            "UPDATE blotter_meta
             SET ledger_epoch = ?1,
                 ledger_instance_id = ?2,
                 last_reset_operation_id = ?3
             WHERE singleton_id = 1",
            params![
                epoch_i64,
                identity.ledger_instance_id,
                identity.last_reset_operation_id
            ],
        )
        .map_err(|_| storage_unavailable())?;
    if updated != 1 {
        return Err(storage_unavailable());
    }
    Ok(())
}

/// Reads and strictly validates persisted `(ledger_epoch, ledger_instance_id,
/// last_reset_operation_id)` from an extended `blotter_meta` table.
///
/// Fails closed with `StorageUnavailable` if `ledger_epoch` or `ledger_instance_id` is NULL,
/// half-null, `< 1`, or not a canonical 26-character uppercase ULID.
pub fn read_persisted_ledger_identity(conn: &Connection) -> ApplicationResult<LedgerIdentity> {
    let row: Option<(Option<i64>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT ledger_epoch, ledger_instance_id, last_reset_operation_id
             FROM blotter_meta
             WHERE singleton_id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|_| storage_unavailable())?;
    let Some((epoch_opt, instance_opt, reset_op_opt)) = row else {
        return Err(storage_unavailable());
    };
    let Some(epoch_raw) = epoch_opt else {
        return Err(storage_unavailable());
    };
    if epoch_raw < 1 {
        return Err(storage_unavailable());
    }
    let ledger_epoch = u64::try_from(epoch_raw).map_err(|_| storage_unavailable())?;
    let Some(instance_raw) = instance_opt else {
        return Err(storage_unavailable());
    };
    let ledger_instance_id = validate_canonical_ulid(&instance_raw)?;
    let last_reset_operation_id = reset_op_opt
        .map(|op| validate_canonical_ulid(&op))
        .transpose()?;
    Ok(LedgerIdentity {
        ledger_epoch,
        ledger_instance_id,
        last_reset_operation_id,
    })
}

/// Initializes a verified empty database or validates an existing database's required tables and
/// explicit `blotter_meta.schema_version` while acquiring the exclusive write lock.
pub fn initialize_or_verify_schema(
    conn: &Connection,
    default_config: &BlotterConfig,
) -> ApplicationResult<()> {
    initialize_or_verify_schema_with_identity(conn, default_config, None, StartupSchemaFault::None)
        .map(|_| ())
}

/// Initializes a new database with `(epoch=1, random ULID)`, migrates a legacy 5-column v1
/// database in one transaction using deterministic `(epoch=1, derived ULID)`, or verifies an
/// existing v1+identity database and returns its committed [`LedgerIdentity`].
pub fn initialize_or_verify_schema_with_identity(
    conn: &Connection,
    default_config: &BlotterConfig,
    source_physical_id: Option<&PhysicalFileId>,
    fault: StartupSchemaFault,
) -> ApplicationResult<LedgerIdentity> {
    let config_json = config_to_json(default_config)?;

    // Acquire an immediate write transaction so exclusive locking is actively held from open.
    conn.execute_batch("BEGIN IMMEDIATE;")
        .map_err(|_| storage_unavailable())?;

    let result = (|| -> ApplicationResult<LedgerIdentity> {
        let existing_object_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type IN ('table', 'index', 'trigger', 'view')
                   AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| storage_unavailable())?;

        let identity = if existing_object_count == 0 {
            conn.execute_batch(BLOTTER_SCHEMA_DDL)
                .map_err(|_| storage_unavailable())?;
            let instance_id = generate_random_ulid()?;
            conn.execute(
                "INSERT INTO blotter_meta
                 (singleton_id, schema_version, head_seq, oldest_retained_seq, config_json,
                  ledger_epoch, ledger_instance_id, last_reset_operation_id)
                 VALUES (1, ?1, 0, 1, ?2, 1, ?3, NULL)",
                params![i64::from(BLOTTER_SCHEMA_VERSION), config_json, instance_id],
            )
            .map_err(|_| storage_unavailable())?;
            LedgerIdentity {
                ledger_epoch: 1,
                ledger_instance_id: instance_id,
                last_reset_operation_id: None,
            }
        } else {
            verify_existing_tables(conn)?;
            verify_business_meta_version(conn)?;
            match inspect_meta_layout(conn)? {
                MetaSchemaLayout::V1Legacy => {
                    let Some(phys_id) = source_physical_id else {
                        return Err(storage_unavailable());
                    };
                    let derived = derive_legacy_v1_identity_on_conn(conn, phys_id)
                        .map_err(|_| storage_unavailable())?;
                    install_meta_identity_columns(conn, &derived)?;
                    derived
                }
                MetaSchemaLayout::V1WithIdentity => {
                    let persisted = read_persisted_ledger_identity(conn)?;
                    // Touch singleton row in-place to ensure an actual write occurs inside this
                    // exclusive-mode transaction so SQLite retains the exclusive lock for life.
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
                    persisted
                }
            }
        };

        if fault == StartupSchemaFault::FailBeforeSchemaCommit {
            return Err(storage_unavailable());
        }

        Ok(identity)
    })();

    match result {
        Ok(identity) => {
            conn.execute_batch("COMMIT;")
                .map_err(|_| storage_unavailable())?;
            Ok(identity)
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

pub(crate) fn verify_existing_tables(conn: &Connection) -> ApplicationResult<()> {
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

    if present.len() != REQUIRED_TABLES.len() {
        return Err(storage_unavailable());
    }
    for &required in REQUIRED_TABLES {
        if !present.contains(required) {
            return Err(storage_unavailable());
        }
    }
    for &(table_name, expected_cols) in REQUIRED_NON_META_TABLE_COLUMNS {
        let mut col_stmt = conn
            .prepare(&format!("PRAGMA table_info({table_name})"))
            .map_err(|_| storage_unavailable())?;
        let actual_cols = col_stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|_| storage_unavailable())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| storage_unavailable())?;
        let actual_refs: Vec<&str> = actual_cols.iter().map(String::as_str).collect();
        if actual_refs.as_slice() != expected_cols {
            return Err(storage_unavailable());
        }
    }
    Ok(())
}

pub(crate) fn verify_business_meta_version(conn: &Connection) -> ApplicationResult<()> {
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
