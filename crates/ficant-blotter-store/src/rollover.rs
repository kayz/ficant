//! Offline Desk Blotter `SQLite` rollover, companion locking, physical file identity verification,
//! legacy v1 deterministic identity derivation, 17-table dual-source audit, and 6-window
//! resumable state machine (`BP-4` `F-01`, `F-02`, `F-03`, `F-17`, `F-18`, `F-19`, `F-22`).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, SecondsFormat, Utc};
use ficant_application::use_cases::blotter::{
    AccountDeskPolicy, DraftEarmarkStatus, InstrumentCatalogEntry,
};
use ficant_domain::blotter::{
    EarmarkState, OpeningInstrumentBalance, PendingMatchState, TradeFillRecord,
};
use ficant_domain::primitives::FixedDecimal;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, ErrorCode, OpenFlags, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::codec::{
    AccountDeskPolicyDto, BaseEvidenceDto, BaselineDeltaDto, BlotterEventEnvelopeDto,
    DraftEarmarkRecordDto, EarmarkRecordDto, InstrumentCatalogEntryDto, MarketQuoteTickDto,
    MarketTimeDto, OpeningBaseFullImageDto, PendingMatchRecordDto, ReconcileAllocationRecordDto,
    SeqRangeDto, SourceBatchRecordDto, TradeFillRecordDto, bucket_to_str, config_from_json,
    content_hash_from_blob, deserialize_json, draft_status_to_str, earmark_state_to_str,
    encode_hex, format_fixed_decimal, funding_precision_to_str, parse_fixed_decimal_storage,
    parse_naive_date, parse_u64_set, parse_ulid, pending_reason_to_str, pending_state_to_str,
    side_to_str, sqlite_i64_to_u64, trade_lifecycle_from_str,
};
use crate::schema::{
    BLOTTER_META_EXTENDED_COLUMNS, BLOTTER_META_LEGACY_COLUMNS, BLOTTER_SCHEMA_VERSION,
    LedgerIdentity, MetaSchemaLayout, REQUIRED_NON_META_TABLE_COLUMNS,
    generate_distinct_random_ulid, generate_random_ulid, inspect_meta_layout,
    install_meta_identity_columns, is_canonical_ulid, read_persisted_ledger_identity,
    validate_production_sqlite_path, verify_business_meta_version, verify_existing_tables,
};

/// Fixed manifest schema identifier (`interfaces.md`).
pub const ARCHIVE_MANIFEST_SCHEMA: &str = "ficant.blotter.archive-manifest.v1";

/// Fixed rollover state file schema identifier.
pub const ROLLOVER_STATE_SCHEMA: &str = "ficant.blotter.rollover-state.v1";

const LEGACY_V1_IDENTITY_PREFIX: &[u8] = b"ficant.blotter.legacy-v1-identity.v1";
const LEGACY_V1_CONTENT_DIGEST_PREFIX: &[u8] = b"ficant.blotter.legacy-v1-content.v1\0";
const CANONICAL_CONTENT_DIGEST_PREFIX: &[u8] = b"ficant.blotter.content-digest.v1\0";

/// Windows `FILE_ATTRIBUTE_REPARSE_POINT` bit (`0x0400`).
#[cfg(windows)]
const WIN_FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
/// Windows `FILE_SHARE_READ | FILE_SHARE_WRITE` (deliberately omitting `FILE_SHARE_DELETE`).
#[cfg(windows)]
const WIN_SHARE_READ_WRITE_NO_DELETE: u32 = 1 | 2;
/// Windows `FILE_FLAG_BACKUP_SEMANTICS` for opening a directory handle safely via `OpenOptionsExt`.
#[cfg(windows)]
const WIN_FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

/// Structured error returned by rollover, verification, locking, and file-identity checks.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RolloverError {
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("companion or database lock busy: {0}")]
    LockBusy(String),
    #[error(
        "rollover reset blocked by unresolved items (unresolved_committed={unresolved_committed}, open_pending_match={open_pending_match})"
    )]
    UnresolvedItemsBlocked {
        unresolved_committed: u64,
        open_pending_match: u64,
    },
    #[error("rollover mode conflict: {0}")]
    ModeConflict(String),
    #[error("archive artifact conflict: {0}")]
    ArchiveConflict(String),
    #[error("rollover state conflict: {0}")]
    StateConflict(String),
    #[error("database or manifest audit failed: {0}")]
    AuditFailed(String),
    #[error("sqlite error: {0}")]
    Sqlite(String),
    #[error("filesystem io error: {0}")]
    Io(String),
    #[error("injected rollover fault: {0:?}")]
    FaultInjected(RolloverFaultInjection),
}

/// Platform-independent physical file identity (`volume_id` + `file_id`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalFileId {
    pub volume_id: String,
    pub file_id: String,
}

/// Observed physical identity and hardlink count of an existing filesystem object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIdentityInfo {
    pub id: PhysicalFileId,
    pub nlink: u64,
    pub is_dir: bool,
}

/// Rollover execution mode (`interfaces.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RolloverMode {
    #[serde(rename = "rollover_reset")]
    RolloverReset,
    #[serde(rename = "archive_only")]
    ArchiveOnly,
}

/// Manifest status enum (`interfaces.md` 5-state matrix).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManifestStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "archive_only")]
    ArchiveOnly,
    #[serde(rename = "completed_reset")]
    CompletedReset,
}

/// Persistent phase of the resumable rollover state machine (`design.md` §1 `F-02`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RolloverPhase {
    #[serde(rename = "PREPARED")]
    Prepared,
    #[serde(rename = "ARCHIVE_PUBLISHED")]
    ArchivePublished,
    #[serde(rename = "RESET_COMMITTED_MAINTENANCE_PENDING")]
    ResetCommittedMaintenancePending,
    #[serde(rename = "COMPLETED_RESET")]
    CompletedReset,
    #[serde(rename = "COMPLETED_ARCHIVE_ONLY")]
    CompletedArchiveOnly,
}

/// Per-account summary recorded in `ficant.blotter.archive-manifest.v1` (sorted by `account_id`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestAccountSummary {
    pub account_id: String,
    pub trading_date: String,
    pub next_settlement_date: String,
}

/// Exact 10 non-negative count fields required in `ficant.blotter.archive-manifest.v1`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveManifestCounts {
    pub active_hold: u64,
    pub unresolved_committed: u64,
    pub reconciled_committed: u64,
    pub expired_hold: u64,
    pub released_hold: u64,
    pub cancelled_committed: u64,
    pub open_pending_match: u64,
    pub resolved_pending_match: u64,
    pub pending_draft: u64,
    pub retained_events: u64,
}

impl ArchiveManifestCounts {
    #[must_use]
    pub const fn has_unresolved(&self) -> bool {
        self.unresolved_committed > 0 || self.open_pending_match > 0
    }
}

/// Complete `ficant.blotter.archive-manifest.v1` JSON contract (`interfaces.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveManifest {
    pub schema: String,
    pub operation_id: String,
    pub archived_at_rfc3339: String,
    pub mode: RolloverMode,
    pub status: ManifestStatus,
    pub source_reset_committed: bool,
    pub source_ledger_epoch: u64,
    pub source_ledger_instance_id: String,
    pub next_ledger_epoch: Option<u64>,
    pub next_ledger_instance_id: Option<String>,
    pub allow_unresolved_used: bool,
    pub wal_truncated: bool,
    pub business_schema_version: u32,
    pub archive_filename: String,
    pub archive_size_bytes: u64,
    pub archive_sha256_hex: String,
    pub content_sha256_hex: String,
    pub head_seq: u64,
    pub oldest_retained_seq: u64,
    pub account_count: u64,
    pub accounts: Vec<ManifestAccountSummary>,
    pub counts: ArchiveManifestCounts,
}

/// Summary produced by auditing all 17 Desk Blotter tables on a `SQLite` connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseAuditSummary {
    pub meta_layout: MetaSchemaLayout,
    pub identity: LedgerIdentity,
    pub head_seq: u64,
    pub oldest_retained_seq: u64,
    pub content_sha256_hex: String,
    pub accounts: Vec<ManifestAccountSummary>,
    pub counts: ArchiveManifestCounts,
    pub business_tables_empty: bool,
}

/// Request parameters for [`sqlite_rollover`] / [`sqlite_rollover_with_fault`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqliteRolloverRequest {
    pub db_path: PathBuf,
    pub archive_dir: PathBuf,
    pub archive_name: Option<String>,
    pub archive_only: bool,
    pub allow_unresolved: bool,
}

/// Deterministic fault injection windows for testing 6-window crash recovery (`F-02`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RolloverFaultInjection {
    #[default]
    None,
    /// Window 1: writes a half-written state temp file and aborts before atomic state replace.
    HalfWrittenStateTempFile,
    /// Window 2: aborts immediately after formal archive publish (`ARCHIVE_PUBLISHED`).
    AfterArchivePublished,
    /// Window 3: aborts immediately after pending manifest publish.
    AfterPendingManifestPublished,
    /// Window 4: aborts right after source reset transaction `COMMIT`, before state pointer update.
    AfterSourceResetCommittedBeforeStateUpdate,
    /// Window 5: aborts during `VACUUM` / final `wal_checkpoint(TRUNCATE)` maintenance.
    DuringMaintenanceVacuumOrCheckpoint,
    /// Window 6: aborts right after final manifest publish, before retiring active state pointer.
    AfterFinalManifestPublishedBeforeStateRetire,
}

/// Structured output returned by [`sqlite_rollover`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqliteRolloverReport {
    pub operation_id: String,
    pub stem: String,
    pub mode: RolloverMode,
    pub phase: RolloverPhase,
    pub reused_completed_receipt: bool,
    pub archive_path: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: ArchiveManifest,
}

/// Structured output returned by [`verify_sqlite_archive`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifySqliteArchiveReport {
    pub manifest_path: PathBuf,
    pub archive_path: PathBuf,
    pub manifest: ArchiveManifest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActiveRolloverOperation {
    operation_id: String,
    archived_at_rfc3339: String,
    source_physical_id: PhysicalFileId,
    source_ledger_epoch: u64,
    source_ledger_instance_id: String,
    archive_dir_physical_id: PhysicalFileId,
    stem: String,
    mode: RolloverMode,
    allow_unresolved_requested: bool,
    allow_unresolved_used: bool,
    archive_only_checkpoint_source: bool,
    source_content_sha256_hex: String,
    next_ledger_epoch: Option<u64>,
    next_ledger_instance_id: Option<String>,
    staging_archive_size_bytes: Option<u64>,
    staging_archive_sha256_hex: Option<String>,
    phase: RolloverPhase,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletedRolloverReceipt {
    operation_id: String,
    stem: String,
    mode: RolloverMode,
    phase: RolloverPhase,
    archive_dir_physical_id: PhysicalFileId,
    source_ledger_epoch: u64,
    source_ledger_instance_id: String,
    next_ledger_epoch: Option<u64>,
    next_ledger_instance_id: Option<String>,
    source_content_sha256_hex: String,
    archive_sha256_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RolloverStateFile {
    schema: String,
    active: Option<ActiveRolloverOperation>,
    last_completed: Option<CompletedRolloverReceipt>,
}

// ============================================================================
// Companion Lock & Path Helpers (F-01, F-22(1))
// ============================================================================

/// Returns `<db>.rollover.lock` by appending `.rollover.lock` to `db_path`.
#[must_use]
pub fn companion_lock_path(db_path: &Path) -> PathBuf {
    append_suffix(db_path, ".rollover.lock")
}

/// Returns `<db>.rollover.state.json` by appending `.rollover.state.json` to `db_path`.
#[must_use]
pub fn rollover_state_path(db_path: &Path) -> PathBuf {
    append_suffix(db_path, ".rollover.state.json")
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os = OsString::from(path.as_os_str());
    os.push(suffix);
    PathBuf::from(os)
}

fn sqlite_companions(db_path: &Path) -> [PathBuf; 3] {
    [
        append_suffix(db_path, "-wal"),
        append_suffix(db_path, "-shm"),
        append_suffix(db_path, "-journal"),
    ]
}

fn staging_archive_path(archive_dir: &Path, stem: &str, op_id: &str) -> PathBuf {
    archive_dir.join(format!(".{stem}.{op_id}.tmp.sqlite"))
}

fn final_archive_path(archive_dir: &Path, stem: &str) -> PathBuf {
    archive_dir.join(format!("{stem}.sqlite"))
}

fn pending_manifest_path(archive_dir: &Path, stem: &str) -> PathBuf {
    archive_dir.join(format!("{stem}.manifest.pending.json"))
}

fn final_manifest_path(archive_dir: &Path, stem: &str) -> PathBuf {
    archive_dir.join(format!("{stem}.manifest.json"))
}

/// Exclusive companion lock held on `<db>.rollover.lock` via `SQLite` `BEGIN EXCLUSIVE` +
/// `PRAGMA busy_timeout = 0` for the entire lifetime of an open `SqliteBlotterStore` or rollover.
pub(crate) struct CompanionLock {
    conn: Connection,
    lock_identity: FileIdentityInfo,
}

impl CompanionLock {
    pub(crate) fn acquire(db_path: &Path) -> Result<Self, RolloverError> {
        let abs_db = normalize_and_verify_no_reparse(db_path)?;
        let parent = abs_db.parent().ok_or_else(|| {
            RolloverError::Validation("database path has no parent directory".to_owned())
        })?;
        if !parent.exists() {
            return Err(RolloverError::Io(
                "database parent directory does not exist".to_owned(),
            ));
        }
        let _parent_id = inspect_physical_file_identity(parent, true)?;

        let lock_path = companion_lock_path(&abs_db);
        let state_path = rollover_state_path(&abs_db);
        let db_companions = sqlite_companions(&abs_db);
        let lock_companions = sqlite_companions(&lock_path);

        // Verify no symlinks/reparse points on any companion path before opening anything.
        let mut all_paths: Vec<PathBuf> = vec![abs_db.clone(), lock_path.clone(), state_path];
        all_paths.extend(db_companions);
        all_paths.extend(lock_companions);
        for p in &all_paths {
            verify_existing_entry_no_reparse(p)?;
        }

        // Check path-level distinctness and pairwise physical distinctness before SQLite open.
        assert_distinct_file_roles(&all_paths)?;
        if abs_db.exists() {
            let db_id = inspect_physical_file_identity(&abs_db, false)?;
            if db_id.nlink != 1 {
                return Err(RolloverError::Validation(format!(
                    "source database must have exactly 1 hardlink (observed {})",
                    db_id.nlink
                )));
            }
        }
        if lock_path.exists() {
            let pre_lock_id = inspect_physical_file_identity(&lock_path, false)?;
            if pre_lock_id.nlink != 1 {
                return Err(RolloverError::Validation(format!(
                    "companion lock file must have exactly 1 hardlink (observed {})",
                    pre_lock_id.nlink
                )));
            }
        }
        assert_pairwise_physical_distinct(&all_paths)?;

        // Open companion lock DB without SQLITE_OPEN_URI and immediately BEGIN EXCLUSIVE with
        // busy_timeout = 0 (zero retry sleep).
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&lock_path, flags).map_err(map_sqlite_lock_err)?;
        conn.execute_batch("PRAGMA busy_timeout = 0;")
            .map_err(map_sqlite_lock_err)?;
        let observed_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .map_err(map_sqlite_lock_err)?;
        if observed_timeout != 0 {
            return Err(RolloverError::Sqlite(
                "companion lock PRAGMA busy_timeout readback != 0".to_owned(),
            ));
        }
        conn.execute_batch("BEGIN EXCLUSIVE;")
            .map_err(map_sqlite_lock_err)?;

        // Re-verify lock file physical identity and isolation after lock acquisition.
        verify_existing_entry_no_reparse(&lock_path)?;
        let lock_identity = inspect_physical_file_identity(&lock_path, false)?;
        if lock_identity.nlink != 1 {
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(RolloverError::Validation(format!(
                "companion lock file must have exactly 1 hardlink after lock (observed {})",
                lock_identity.nlink
            )));
        }
        if abs_db.exists() {
            let db_id = inspect_physical_file_identity(&abs_db, false)?;
            if db_id.nlink != 1 {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(RolloverError::Validation(format!(
                    "source database must have exactly 1 hardlink after lock (observed {})",
                    db_id.nlink
                )));
            }
            if db_id.id == lock_identity.id {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(RolloverError::Validation(
                    "source database and companion lock resolve to the same physical file"
                        .to_owned(),
                ));
            }
        }

        Ok(Self {
            conn,
            lock_identity,
        })
    }

    pub(crate) const fn lock_identity(&self) -> &FileIdentityInfo {
        &self.lock_identity
    }
}

impl Drop for CompanionLock {
    fn drop(&mut self) {
        let _ = self.conn.execute_batch("ROLLBACK;");
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_sqlite_lock_err(err: rusqlite::Error) -> RolloverError {
    if let rusqlite::Error::SqliteFailure(ref f, _) = err
        && matches!(f.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    {
        return RolloverError::LockBusy(err.to_string());
    }
    RolloverError::Sqlite(err.to_string())
}

/// Checks `<db>.rollover.state.json` under an acquired companion lock and fails closed if an
/// unfinished or corrupt rollover operation exists.
pub(crate) fn ensure_no_unfinished_rollover_state(db_path: &Path) -> Result<(), RolloverError> {
    let abs_db = normalize_and_verify_no_reparse(db_path)?;
    let state_path = rollover_state_path(&abs_db);
    if !state_path.exists() {
        return Ok(());
    }
    verify_existing_entry_no_reparse(&state_path)?;
    let state = read_rollover_state_file(&state_path)?;
    if state.active.is_some() {
        return Err(RolloverError::StateConflict(
            "active unfinished rollover operation prevents opening blotter store".to_owned(),
        ));
    }
    Ok(())
}

/// Re-verifies source database physical identity (`nlink == 1`, non-reparse, distinct from lock)
/// after `SQLite` has created or opened the file in `SqliteBlotterStore::open`.
pub(crate) fn verify_opened_source_identity(
    db_path: &Path,
    lock: &CompanionLock,
) -> Result<PhysicalFileId, RolloverError> {
    let abs_db = normalize_and_verify_no_reparse(db_path)?;
    verify_existing_entry_no_reparse(&abs_db)?;
    let db_info = inspect_physical_file_identity(&abs_db, false)?;
    if db_info.nlink != 1 {
        return Err(RolloverError::Validation(format!(
            "source database must have exactly 1 hardlink (observed {})",
            db_info.nlink
        )));
    }
    if db_info.id == lock.lock_identity().id {
        return Err(RolloverError::Validation(
            "source database aliases companion lock file".to_owned(),
        ));
    }
    Ok(db_info.id)
}

// ============================================================================
// Physical File Identity & Reparse/Symlink Verification (F-01, F-22(1))
// ============================================================================

/// Validates that `path` and every existing ancestor component along both the raw path and the
/// lexically normalized absolute path contain no symlinks or Windows reparse points.
pub(crate) fn normalize_and_verify_no_reparse(path: &Path) -> Result<PathBuf, RolloverError> {
    let Some(raw) = path.to_str() else {
        return Err(RolloverError::Validation(
            "path is not valid UTF-8".to_owned(),
        ));
    };
    if raw.is_empty() || raw.contains('\0') {
        return Err(RolloverError::Validation(
            "path is empty or contains NUL".to_owned(),
        ));
    }

    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| RolloverError::Io(e.to_string()))?
            .join(path)
    };

    // Walk every prefix BEFORE collapsing `..` so symlink/junction traversal via `a/../b` is caught.
    check_prefixes_no_reparse(&joined)?;

    // Lexically normalize `.` and `..` without following symlinks.
    let normalized = lexically_normalize_path(&joined)?;

    // Walk every prefix AFTER lexical normalization as well.
    check_prefixes_no_reparse(&normalized)?;

    Ok(normalized)
}

fn lexically_normalize_path(path: &Path) -> Result<PathBuf, RolloverError> {
    let mut out = PathBuf::new();
    let mut normal_depth = 0_usize;
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => {
                #[cfg(windows)]
                {
                    use std::path::Prefix;
                    match p.kind() {
                        Prefix::Disk(_) | Prefix::VerbatimDisk(_) => {}
                        _ => {
                            return Err(RolloverError::Validation(
                                "unsupported Windows non-local path prefix".to_owned(),
                            ));
                        }
                    }
                }
                out.push(comp.as_os_str());
            }
            Component::RootDir => {
                out.push(comp.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if normal_depth == 0 {
                    return Err(RolloverError::Validation(
                        "path escapes filesystem root via '..'".to_owned(),
                    ));
                }
                out.pop();
                normal_depth -= 1;
            }
            Component::Normal(seg) => {
                out.push(seg);
                normal_depth += 1;
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(RolloverError::Validation(
            "normalized path is empty".to_owned(),
        ));
    }
    Ok(out)
}

fn check_prefixes_no_reparse(path: &Path) -> Result<(), RolloverError> {
    let mut cur = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {
                cur.push(comp.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                cur.push("..");
                verify_existing_entry_no_reparse(&cur)?;
            }
            Component::Normal(seg) => {
                cur.push(seg);
                verify_existing_entry_no_reparse(&cur)?;
            }
        }
    }
    Ok(())
}

fn verify_existing_entry_no_reparse(path: &Path) -> Result<(), RolloverError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(RolloverError::Validation(format!(
                    "symlink rejected at '{}'",
                    path.display()
                )));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if (meta.file_attributes() & WIN_FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
                    return Err(RolloverError::Validation(format!(
                        "Windows reparse point / junction rejected at '{}'",
                        path.display()
                    )));
                }
            }
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(RolloverError::Io(format!(
            "failed to inspect metadata for '{}': {err}",
            path.display()
        ))),
    }
}

/// Inspects the physical identity (`volume_id`, `file_id`) and hardlink count (`nlink`) of an
/// existing file or directory without `unsafe` code and without administrator privileges.
pub fn inspect_physical_file_identity(
    path: &Path,
    expect_dir: bool,
) -> Result<FileIdentityInfo, RolloverError> {
    let abs = normalize_and_verify_no_reparse(path)?;
    let pre_meta = fs::symlink_metadata(&abs).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            RolloverError::Io(format!("path does not exist: '{}'", abs.display()))
        } else {
            RolloverError::Io(format!("metadata failed for '{}': {e}", abs.display()))
        }
    })?;
    if pre_meta.file_type().is_symlink() {
        return Err(RolloverError::Validation(format!(
            "symlink rejected at '{}'",
            abs.display()
        )));
    }
    if expect_dir && !pre_meta.is_dir() {
        return Err(RolloverError::Validation(format!(
            "expected directory at '{}'",
            abs.display()
        )));
    }
    if !expect_dir && !pre_meta.is_file() {
        return Err(RolloverError::Validation(format!(
            "expected regular file at '{}'",
            abs.display()
        )));
    }

    #[cfg(unix)]
    {
        inspect_physical_identity_unix(&abs, expect_dir)
    }
    #[cfg(windows)]
    {
        inspect_physical_identity_windows(&abs, expect_dir)
    }
}

#[cfg(unix)]
fn inspect_physical_identity_unix(
    abs: &Path,
    expect_dir: bool,
) -> Result<FileIdentityInfo, RolloverError> {
    use std::os::unix::fs::MetadataExt;

    let file = File::open(abs).map_err(|e| RolloverError::Io(e.to_string()))?;
    let meta = file
        .metadata()
        .map_err(|e| RolloverError::Io(e.to_string()))?;
    if meta.is_dir() != expect_dir || (!expect_dir && !meta.is_file()) {
        return Err(RolloverError::Validation(format!(
            "file type changed during identity check at '{}'",
            abs.display()
        )));
    }
    let post_meta = fs::symlink_metadata(abs).map_err(|e| RolloverError::Io(e.to_string()))?;
    if post_meta.file_type().is_symlink()
        || post_meta.dev() != meta.dev()
        || post_meta.ino() != meta.ino()
    {
        return Err(RolloverError::Validation(format!(
            "file identity changed during inspection at '{}'",
            abs.display()
        )));
    }
    Ok(FileIdentityInfo {
        id: PhysicalFileId {
            volume_id: format!("unix-dev:{:016x}", meta.dev()),
            file_id: format!("unix-ino:{:016x}", meta.ino()),
        },
        nlink: meta.nlink(),
        is_dir: expect_dir,
    })
}

#[cfg(windows)]
#[allow(clippy::too_many_lines)]
fn inspect_physical_identity_windows(
    abs: &Path,
    expect_dir: bool,
) -> Result<FileIdentityInfo, RolloverError> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::process::Command;

    // Hold an open handle with FILE_SHARE_READ | FILE_SHARE_WRITE (no FILE_SHARE_DELETE) so the
    // object cannot be renamed or deleted while we query volume ID, file ID, and hardlink list.
    let mut opts = OpenOptions::new();
    opts.read(true).share_mode(WIN_SHARE_READ_WRITE_NO_DELETE);
    if expect_dir {
        opts.custom_flags(WIN_FILE_FLAG_BACKUP_SEMANTICS);
    }
    let handle = opts.open(abs).map_err(|e| {
        RolloverError::Io(format!(
            "failed to open identity handle for '{}': {e}",
            abs.display()
        ))
    })?;
    let handle_meta = handle
        .metadata()
        .map_err(|e| RolloverError::Io(e.to_string()))?;
    if (handle_meta.file_attributes() & WIN_FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(RolloverError::Validation(format!(
            "reparse point detected on open handle for '{}'",
            abs.display()
        )));
    }
    if handle_meta.is_dir() != expect_dir || (!expect_dir && !handle_meta.is_file()) {
        return Err(RolloverError::Validation(format!(
            "unexpected filesystem object type at '{}'",
            abs.display()
        )));
    }

    // Re-check zero reparse points and resolve canonical long path (handles 8.3 short paths).
    verify_existing_entry_no_reparse(abs)?;
    let canon = fs::canonicalize(abs).map_err(|e| RolloverError::Io(e.to_string()))?;
    let canon_str = canon
        .to_str()
        .ok_or_else(|| RolloverError::Validation("canonical path is not valid UTF-8".to_owned()))?;
    let stripped = canon_str.strip_prefix(r"\\?\").unwrap_or(canon_str);
    if stripped.len() < 3 || &stripped[1..3] != ":\\" {
        return Err(RolloverError::Validation(format!(
            "unsupported Windows path root for identity check: '{stripped}'"
        )));
    }
    let drive_letter = stripped.as_bytes()[0].to_ascii_uppercase() as char;
    let drive_root = format!("{drive_letter}:\\");
    let drive_colon = format!("{drive_letter}:");
    let vol_relative = &stripped[2..]; // starts with '\'

    // 1. Volume identity via `mountvol <root> /L` (non-admin), fallback `cmd /c vol <drive>:`.
    let volume_id = query_windows_volume_id(&drive_root, &drive_colon)?;

    // 2. File ID via `fsutil file queryfileid <path>` (extract 0x[0-9a-fA-F]+ hex token only).
    let qid_out = Command::new("fsutil")
        .args(["file", "queryfileid", stripped])
        .output()
        .map_err(|e| RolloverError::Io(format!("failed to run fsutil queryfileid: {e}")))?;
    if !qid_out.status.success() {
        return Err(RolloverError::Io(format!(
            "fsutil file queryfileid failed with status {}",
            qid_out.status
        )));
    }
    let qid_text = String::from_utf8_lossy(&qid_out.stdout);
    let file_id = parse_single_hex_file_id(&qid_text)?;

    // 3. Hardlink enumeration via `fsutil hardlink list <path>`.
    let hl_out = Command::new("fsutil")
        .args(["hardlink", "list", stripped])
        .output()
        .map_err(|e| RolloverError::Io(format!("failed to run fsutil hardlink list: {e}")))?;
    if !hl_out.status.success() {
        return Err(RolloverError::Io(format!(
            "fsutil hardlink list failed with status {}",
            hl_out.status
        )));
    }
    let hl_text = String::from_utf8_lossy(&hl_out.stdout);
    let hl_lines: Vec<&str> = hl_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if hl_lines.is_empty() {
        return Err(RolloverError::AuditFailed(
            "fsutil hardlink list returned no entries".to_owned(),
        ));
    }
    // Bind fsutil result back to the held object's canonical volume-relative path (ASCII paths).
    if vol_relative.is_ascii()
        && !hl_lines
            .iter()
            .any(|line| line.eq_ignore_ascii_case(vol_relative))
    {
        return Err(RolloverError::AuditFailed(format!(
            "fsutil hardlink list could not be bound to held path '{vol_relative}'"
        )));
    }
    let nlink = u64::try_from(hl_lines.len())
        .map_err(|_| RolloverError::AuditFailed("hardlink count overflow".to_owned()))?;

    // Re-check symlink_metadata while still holding the no-delete handle.
    let post_meta = fs::symlink_metadata(abs).map_err(|e| RolloverError::Io(e.to_string()))?;
    if post_meta.file_type().is_symlink()
        || (post_meta.file_attributes() & WIN_FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || post_meta.is_dir() != expect_dir
    {
        return Err(RolloverError::Validation(format!(
            "path mutated during Windows identity inspection at '{}'",
            abs.display()
        )));
    }
    drop(handle);

    Ok(FileIdentityInfo {
        id: PhysicalFileId { volume_id, file_id },
        nlink,
        is_dir: expect_dir,
    })
}

#[cfg(windows)]
fn query_windows_volume_id(drive_root: &str, drive_colon: &str) -> Result<String, RolloverError> {
    use std::process::Command;

    if let Ok(out) = Command::new("mountvol").args([drive_root, "/L"]).output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with(r"\\?\Volume{") && trimmed.ends_with(r"}\") {
                return Ok(format!("win-vol:{}", trimmed.to_ascii_lowercase()));
            }
        }
    }

    let out = Command::new("cmd")
        .args(["/c", "vol", drive_colon])
        .output()
        .map_err(|e| RolloverError::Io(format!("failed to run cmd /c vol: {e}")))?;
    if !out.status.success() {
        return Err(RolloverError::Io(format!(
            "failed to query Windows volume identity for '{drive_root}'"
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut serials = Vec::new();
    for raw_tok in text.split_whitespace() {
        let tok = raw_tok.trim_matches(|c: char| !c.is_ascii_hexdigit() && c != '-');
        if tok.len() == 9
            && tok.as_bytes()[4] == b'-'
            && tok[..4].bytes().all(|b| b.is_ascii_hexdigit())
            && tok[5..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            serials.push(tok.to_ascii_lowercase());
        }
    }
    if serials.len() == 1 {
        Ok(format!("win-vol-serial:{}", serials[0]))
    } else {
        Err(RolloverError::AuditFailed(format!(
            "ambiguous or unreadable Windows volume serial for '{drive_root}'"
        )))
    }
}

/// Extracts exactly one `0x[0-9a-fA-F]+` hex file-ID token from `fsutil file queryfileid` output
/// without depending on English prose (works identically across English and Chinese locales).
pub fn parse_single_hex_file_id(output: &str) -> Result<String, RolloverError> {
    let mut matches: Vec<String> = Vec::new();
    for raw in output.split(|c: char| {
        c.is_whitespace() || matches!(c, ',' | ';' | ':' | '(' | ')' | '[' | ']' | '.')
    }) {
        if let Some(hex_part) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X"))
            && !hex_part.is_empty()
            && hex_part.bytes().all(|b| b.is_ascii_hexdigit())
        {
            let parsed = u128::from_str_radix(hex_part, 16).map_err(|_| {
                RolloverError::AuditFailed(format!("invalid 128-bit hex file ID token '{raw}'"))
            })?;
            matches.push(format!("0x{parsed:032x}"));
        }
    }
    if matches.len() == 1 {
        Ok(matches.remove(0))
    } else {
        Err(RolloverError::AuditFailed(format!(
            "expected exactly 1 hex file ID token from fsutil output, found {}",
            matches.len()
        )))
    }
}

fn normalized_path_key(path: &Path) -> String {
    let s = path.to_string_lossy();
    #[cfg(windows)]
    {
        s.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        s.into_owned()
    }
}

fn assert_distinct_file_roles(paths: &[PathBuf]) -> Result<(), RolloverError> {
    let mut seen = BTreeSet::new();
    for p in paths {
        let key = normalized_path_key(p);
        if !seen.insert(key) {
            return Err(RolloverError::Validation(format!(
                "path collision or alias detected for '{}'",
                p.display()
            )));
        }
    }
    Ok(())
}

fn assert_pairwise_physical_distinct(paths: &[PathBuf]) -> Result<(), RolloverError> {
    let mut seen_ids: BTreeMap<PhysicalFileId, PathBuf> = BTreeMap::new();
    for p in paths {
        if !p.exists() {
            continue;
        }
        verify_existing_entry_no_reparse(p)?;
        let info = inspect_physical_file_identity(p, false)?;
        if let Some(other) = seen_ids.insert(info.id, p.clone()) {
            return Err(RolloverError::Validation(format!(
                "physical file alias detected between '{}' and '{}'",
                other.display(),
                p.display()
            )));
        }
    }
    Ok(())
}

// ============================================================================
// Dual Validators: Stem vs Manifest Archive Filename & 5-State Matrix (F-19)
// ============================================================================

/// Validates a bare archive stem (`--archive-name <STEM>` or manifest `<stem>.manifest.json`).
///
/// Rules (`interfaces.md`, `F-19`):
/// - Non-empty ASCII single component (<= 128 chars)
/// - Starts with ASCII alphanumeric
/// - Remaining bytes are ASCII alphanumeric, `_`, or `-` (forbids `.`, `/`, `\`, `:`, `.sqlite`)
/// - Excludes Windows reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`..`COM9`, `LPT1`..`LPT9`)
pub fn validate_archive_stem(stem: &str) -> Result<(), RolloverError> {
    if stem.is_empty() || stem.len() > 128 {
        return Err(RolloverError::Validation(format!(
            "archive stem length must be 1..=128, got {}",
            stem.len()
        )));
    }
    let bytes = stem.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() {
        return Err(RolloverError::Validation(format!(
            "archive stem '{stem}' must start with ASCII alphanumeric"
        )));
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
    {
        return Err(RolloverError::Validation(format!(
            "archive stem '{stem}' contains forbidden characters"
        )));
    }
    if is_windows_reserved_device_name(stem) {
        return Err(RolloverError::Validation(format!(
            "archive stem '{stem}' is a reserved Windows device name"
        )));
    }
    Ok(())
}

/// Validates `manifest.archive_filename` by stripping `.sqlite` **once**, validating the remaining
/// stem via [`validate_archive_stem`], and requiring exact equality with `<expected_stem>.sqlite`.
pub fn validate_archive_filename(filename: &str, expected_stem: &str) -> Result<(), RolloverError> {
    validate_archive_stem(expected_stem)?;
    let Some(stripped) = filename.strip_suffix(".sqlite") else {
        return Err(RolloverError::Validation(format!(
            "archive_filename '{filename}' must end with '.sqlite'"
        )));
    };
    validate_archive_stem(stripped)?;
    if stripped != expected_stem {
        return Err(RolloverError::Validation(format!(
            "archive_filename '{filename}' does not match expected stem '{expected_stem}.sqlite'"
        )));
    }
    Ok(())
}

fn is_windows_reserved_device_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn is_lowercase_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Validates all structural fields and the 5-state matrix of an [`ArchiveManifest`] (`F-19`).
#[allow(clippy::too_many_lines)]
pub fn validate_archive_manifest(
    manifest: &ArchiveManifest,
    expected_stem: &str,
    final_only: bool,
) -> Result<(), RolloverError> {
    if manifest.schema != ARCHIVE_MANIFEST_SCHEMA {
        return Err(RolloverError::AuditFailed(format!(
            "invalid manifest schema '{}'",
            manifest.schema
        )));
    }
    if !is_canonical_ulid(&manifest.operation_id) {
        return Err(RolloverError::AuditFailed(
            "manifest operation_id is not a canonical ULID".to_owned(),
        ));
    }
    DateTime::parse_from_rfc3339(&manifest.archived_at_rfc3339).map_err(|_| {
        RolloverError::AuditFailed("manifest archived_at_rfc3339 is invalid RFC3339".to_owned())
    })?;
    if manifest.business_schema_version != BLOTTER_SCHEMA_VERSION {
        return Err(RolloverError::AuditFailed(format!(
            "manifest business_schema_version {} != {}",
            manifest.business_schema_version, BLOTTER_SCHEMA_VERSION
        )));
    }
    if manifest.source_ledger_epoch < 1 {
        return Err(RolloverError::AuditFailed(
            "manifest source_ledger_epoch must be >= 1".to_owned(),
        ));
    }
    if !is_canonical_ulid(&manifest.source_ledger_instance_id) {
        return Err(RolloverError::AuditFailed(
            "manifest source_ledger_instance_id is not a canonical ULID".to_owned(),
        ));
    }
    if manifest.next_ledger_epoch.is_some() != manifest.next_ledger_instance_id.is_some() {
        return Err(RolloverError::AuditFailed(
            "manifest next_ledger_epoch and next_ledger_instance_id must not be half-null"
                .to_owned(),
        ));
    }
    if let (Some(next_epoch), Some(next_id)) = (
        manifest.next_ledger_epoch,
        manifest.next_ledger_instance_id.as_deref(),
    ) {
        if manifest.source_ledger_epoch.checked_add(1) != Some(next_epoch) {
            return Err(RolloverError::AuditFailed(
                "manifest next_ledger_epoch must equal source_ledger_epoch + 1".to_owned(),
            ));
        }
        if !is_canonical_ulid(next_id) || next_id == manifest.source_ledger_instance_id {
            return Err(RolloverError::AuditFailed(
                "manifest next_ledger_instance_id must be a distinct canonical ULID".to_owned(),
            ));
        }
    }
    validate_archive_filename(&manifest.archive_filename, expected_stem)
        .map_err(|e| RolloverError::AuditFailed(e.to_string()))?;
    if !is_lowercase_sha256_hex(&manifest.archive_sha256_hex) {
        return Err(RolloverError::AuditFailed(
            "manifest archive_sha256_hex must be 64 lowercase hex chars".to_owned(),
        ));
    }
    if !is_lowercase_sha256_hex(&manifest.content_sha256_hex) {
        return Err(RolloverError::AuditFailed(
            "manifest content_sha256_hex must be 64 lowercase hex chars".to_owned(),
        ));
    }
    if manifest.oldest_retained_seq < 1 {
        return Err(RolloverError::AuditFailed(
            "manifest oldest_retained_seq must be >= 1".to_owned(),
        ));
    }
    if manifest.account_count != u64::try_from(manifest.accounts.len()).unwrap_or(u64::MAX) {
        return Err(RolloverError::AuditFailed(
            "manifest account_count does not match accounts.len()".to_owned(),
        ));
    }
    let mut prev_acct: Option<&str> = None;
    for acct in &manifest.accounts {
        if acct.account_id.is_empty() {
            return Err(RolloverError::AuditFailed(
                "manifest account_id is empty".to_owned(),
            ));
        }
        if let Some(prev) = prev_acct
            && acct.account_id.as_str() <= prev
        {
            return Err(RolloverError::AuditFailed(
                "manifest accounts must be strictly sorted and unique by account_id".to_owned(),
            ));
        }
        prev_acct = Some(&acct.account_id);
        parse_naive_date(&acct.trading_date)
            .map_err(|_| RolloverError::AuditFailed("invalid manifest trading_date".to_owned()))?;
        parse_naive_date(&acct.next_settlement_date).map_err(|_| {
            RolloverError::AuditFailed("invalid manifest next_settlement_date".to_owned())
        })?;
    }

    let has_unresolved = manifest.counts.has_unresolved();

    // Enforce the 5-state matrix from `interfaces.md`.
    match (manifest.status, manifest.mode) {
        // States ① and ②: pending rollover_reset
        (ManifestStatus::Pending, RolloverMode::RolloverReset) => {
            if final_only {
                return Err(RolloverError::AuditFailed(
                    "final manifest cannot have status='pending'".to_owned(),
                ));
            }
            if manifest.next_ledger_epoch.is_none()
                || manifest.wal_truncated
                || manifest.allow_unresolved_used != has_unresolved
            {
                return Err(RolloverError::AuditFailed(
                    "pending rollover_reset violates 5-state manifest matrix".to_owned(),
                ));
            }
        }
        // State ③: pending archive_only
        (ManifestStatus::Pending, RolloverMode::ArchiveOnly) => {
            if final_only {
                return Err(RolloverError::AuditFailed(
                    "final manifest cannot have status='pending'".to_owned(),
                ));
            }
            if manifest.next_ledger_epoch.is_some()
                || manifest.source_reset_committed
                || manifest.allow_unresolved_used
                || manifest.wal_truncated
            {
                return Err(RolloverError::AuditFailed(
                    "pending archive_only violates 5-state manifest matrix".to_owned(),
                ));
            }
        }
        // State ④: final archive_only
        (ManifestStatus::ArchiveOnly, RolloverMode::ArchiveOnly) => {
            if manifest.next_ledger_epoch.is_some()
                || manifest.source_reset_committed
                || manifest.allow_unresolved_used
            {
                return Err(RolloverError::AuditFailed(
                    "archive_only manifest violates 5-state manifest matrix".to_owned(),
                ));
            }
        }
        // State ⑤: final completed_reset
        (ManifestStatus::CompletedReset, RolloverMode::RolloverReset) => {
            if manifest.next_ledger_epoch.is_none()
                || !manifest.source_reset_committed
                || !manifest.wal_truncated
                || manifest.allow_unresolved_used != has_unresolved
            {
                return Err(RolloverError::AuditFailed(
                    "completed_reset manifest violates 5-state manifest matrix".to_owned(),
                ));
            }
        }
        _ => {
            return Err(RolloverError::AuditFailed(format!(
                "illegal manifest (status={:?}, mode={:?}) combination",
                manifest.status, manifest.mode
            )));
        }
    }

    Ok(())
}

// ============================================================================
// Legacy v1 Deterministic Identity Derivation & 17-Table Dual-Source Audit (F-03, F-18)
// ============================================================================

/// Deterministically derives `(epoch=1, Ulid::from_bytes(sha256[0..16]))` for a legacy 5-column
/// v1 database from `(fixed prefix, source physical identity, v1 17-table canonical digest)`.
pub(crate) fn derive_legacy_v1_identity_on_conn(
    conn: &Connection,
    source_physical_id: &PhysicalFileId,
) -> Result<LedgerIdentity, RolloverError> {
    let layout = inspect_meta_layout(conn)
        .map_err(|_| RolloverError::AuditFailed("invalid blotter_meta column layout".to_owned()))?;
    if layout != MetaSchemaLayout::V1Legacy {
        return Err(RolloverError::AuditFailed(
            "derive_legacy_v1_identity_on_conn requires legacy 5-column v1 layout".to_owned(),
        ));
    }
    let v1_digest = compute_legacy_v1_content_digest(conn)?;
    Ok(derive_legacy_v1_identity_from_digest(
        source_physical_id,
        &v1_digest,
    ))
}

fn derive_legacy_v1_identity_from_digest(
    source_physical_id: &PhysicalFileId,
    v1_digest: &[u8; 32],
) -> LedgerIdentity {
    let mut hasher = Sha256::new();
    hash_len_prefixed(&mut hasher, LEGACY_V1_IDENTITY_PREFIX);
    hash_len_prefixed(&mut hasher, source_physical_id.volume_id.as_bytes());
    hash_len_prefixed(&mut hasher, source_physical_id.file_id.as_bytes());
    hash_len_prefixed(&mut hasher, v1_digest);
    let digest = hasher.finalize();
    let mut first16 = [0_u8; 16];
    first16.copy_from_slice(&digest[..16]);
    let derived_ulid = ulid::Ulid::from_bytes(first16).to_string();
    LedgerIdentity {
        ledger_epoch: 1,
        ledger_instance_id: derived_ulid,
        last_reset_operation_id: None,
    }
}

/// Runs complete integrity check, foreign-key check, schema layout check, all 17 tables'
/// dual-source SQL-vs-DTO projection verification, and deterministic full-content SHA-256 (`F-03`).
#[allow(clippy::too_many_lines)]
pub fn audit_sqlite_database(
    conn: &Connection,
    virtual_legacy_identity: Option<&LedgerIdentity>,
) -> Result<DatabaseAuditSummary, RolloverError> {
    // 1. PRAGMA integrity_check
    let integrity_rows: Vec<String> = {
        let mut stmt = conn
            .prepare("PRAGMA integrity_check")
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        stmt.query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
    };
    if integrity_rows.len() != 1 || !integrity_rows[0].eq_ignore_ascii_case("ok") {
        return Err(RolloverError::AuditFailed(format!(
            "PRAGMA integrity_check failed: {integrity_rows:?}"
        )));
    }

    // 2. PRAGMA foreign_key_check
    let fk_violations: i64 = {
        let mut stmt = conn
            .prepare("PRAGMA foreign_key_check")
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut count = 0_i64;
        while rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
            .is_some()
        {
            count += 1;
        }
        count
    };
    if fk_violations != 0 {
        return Err(RolloverError::AuditFailed(format!(
            "PRAGMA foreign_key_check reported {fk_violations} violation(s)"
        )));
    }

    // 3. Exact 17-table v1 schema & singleton meta check
    verify_existing_tables(conn).map_err(|_| {
        RolloverError::AuditFailed("schema table/column layout mismatch".to_owned())
    })?;
    verify_business_meta_version(conn).map_err(|_| {
        RolloverError::AuditFailed("blotter_meta singleton or schema_version invalid".to_owned())
    })?;
    let meta_layout = inspect_meta_layout(conn)
        .map_err(|_| RolloverError::AuditFailed("invalid blotter_meta column layout".to_owned()))?;

    let (head_seq_i64, oldest_retained_i64, config_json): (i64, i64, String) = conn
        .query_row(
            "SELECT head_seq, oldest_retained_seq, config_json
             FROM blotter_meta WHERE singleton_id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
    let head_seq = sqlite_i64_to_u64(head_seq_i64)
        .map_err(|_| RolloverError::AuditFailed("negative head_seq".to_owned()))?;
    let oldest_retained_seq = sqlite_i64_to_u64(oldest_retained_i64)
        .map_err(|_| RolloverError::AuditFailed("negative oldest_retained_seq".to_owned()))?;
    if oldest_retained_seq < 1 || oldest_retained_seq > head_seq.saturating_add(1) {
        return Err(RolloverError::AuditFailed(format!(
            "invalid oldest_retained_seq={oldest_retained_seq} vs head_seq={head_seq}"
        )));
    }
    config_from_json(&config_json)
        .map_err(|_| RolloverError::AuditFailed("invalid blotter_meta.config_json".to_owned()))?;

    let identity = match meta_layout {
        MetaSchemaLayout::V1WithIdentity => read_persisted_ledger_identity(conn).map_err(|_| {
            RolloverError::AuditFailed("invalid or NULL blotter_meta identity columns".to_owned())
        })?,
        MetaSchemaLayout::V1Legacy => {
            let Some(virt) = virtual_legacy_identity else {
                return Err(RolloverError::AuditFailed(
                    "legacy v1 database missing materialized ledger identity columns".to_owned(),
                ));
            };
            if virt.ledger_epoch != 1
                || !is_canonical_ulid(&virt.ledger_instance_id)
                || virt.last_reset_operation_id.is_some()
            {
                return Err(RolloverError::AuditFailed(
                    "invalid virtual legacy v1 identity".to_owned(),
                ));
            }
            virt.clone()
        }
    };

    // 4. Table-by-table dual-source SQL vs DTO verification across all 16 business tables.
    let mut accounts: Vec<ManifestAccountSummary> = Vec::new();
    let mut account_policies: BTreeMap<String, AccountDeskPolicy> = BTreeMap::new();
    let mut account_opening_balances: BTreeMap<String, BTreeMap<String, OpeningInstrumentBalance>> =
        BTreeMap::new();
    let mut counts = ArchiveManifestCounts::default();
    let mut total_business_rows: u64 = 0;

    // Table 2: blotter_accounts
    {
        let mut stmt = conn
            .prepare(
                "SELECT account_id, base_id, trading_date, next_settlement_date,
                        effective_from_json, watermark_time_json, watermark_source_seq,
                        source_id, source_start_after_feed_seq,
                        opening_cash_t0, opening_net_cash_delta_t1,
                        account_version, policy_version, default_quote_source_id,
                        import_actor_id, import_idempotency_key, import_request_hash,
                        import_committed_seq, opening_base_json, policy_json
                 FROM blotter_accounts ORDER BY account_id ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let account_id: String = row_get(row, 0)?;
            let base_id: String = row_get(row, 1)?;
            let trading_date: String = row_get(row, 2)?;
            let next_settlement_date: String = row_get(row, 3)?;
            let effective_from_json: String = row_get(row, 4)?;
            let watermark_time_json: String = row_get(row, 5)?;
            let watermark_source_seq = row_get::<Option<i64>>(row, 6)?
                .map(sqlite_i64_to_u64)
                .transpose()
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid watermark_source_seq".to_owned())
                })?;
            let source_id: String = row_get(row, 7)?;
            let source_start_after_feed_seq =
                sqlite_i64_to_u64(row_get::<i64>(row, 8)?).map_err(|_| {
                    RolloverError::AuditFailed("invalid source_start_after_feed_seq".to_owned())
                })?;
            let opening_cash_t0: String = row_get(row, 9)?;
            let opening_net_cash_delta_t1: String = row_get(row, 10)?;
            let account_version = sqlite_i64_to_u64(row_get::<i64>(row, 11)?)
                .map_err(|_| RolloverError::AuditFailed("invalid account_version".to_owned()))?;
            let policy_version = sqlite_i64_to_u64(row_get::<i64>(row, 12)?)
                .map_err(|_| RolloverError::AuditFailed("invalid policy_version".to_owned()))?;
            let default_quote_source_id: String = row_get(row, 13)?;
            let import_actor_id: String = row_get(row, 14)?;
            let import_idempotency_key: String = row_get(row, 15)?;
            let import_request_hash: Vec<u8> = row_get(row, 16)?;
            let import_committed_seq =
                sqlite_i64_to_u64(row_get::<i64>(row, 17)?).map_err(|_| {
                    RolloverError::AuditFailed("invalid import_committed_seq".to_owned())
                })?;
            let opening_base_json: String = row_get(row, 18)?;
            let policy_json: String = row_get(row, 19)?;

            if account_version < 1
                || policy_version < 1
                || import_actor_id.is_empty()
                || import_idempotency_key.is_empty()
                || import_committed_seq < 1
                || import_committed_seq > head_seq
            {
                return Err(RolloverError::AuditFailed(format!(
                    "blotter_accounts independent columns invalid for '{account_id}'"
                )));
            }
            content_hash_from_blob(&import_request_hash).map_err(|_| {
                RolloverError::AuditFailed("invalid import_request_hash blob".to_owned())
            })?;

            let (core, _cal, _req, _units) =
                deserialize_json::<OpeningBaseFullImageDto>(&opening_base_json)
                    .and_then(OpeningBaseFullImageDto::into_parts)
                    .map_err(|_| {
                        RolloverError::AuditFailed(format!(
                            "invalid opening_base_json for '{account_id}'"
                        ))
                    })?;
            let policy = deserialize_json::<AccountDeskPolicyDto>(&policy_json)
                .and_then(AccountDeskPolicyDto::into_app)
                .map_err(|_| {
                    RolloverError::AuditFailed(format!("invalid policy_json for '{account_id}'"))
                })?;

            let sql_eff = deserialize_json::<MarketTimeDto>(&effective_from_json)
                .and_then(MarketTimeDto::into_domain)
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid effective_from_json".to_owned())
                })?;
            let sql_wm = deserialize_json::<MarketTimeDto>(&watermark_time_json)
                .and_then(MarketTimeDto::into_domain)
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid watermark_time_json".to_owned())
                })?;

            audit_eq("blotter_accounts.account_id", &account_id, &core.account_id)?;
            audit_eq(
                "blotter_accounts.base_id",
                &base_id.as_str(),
                &core.base_id.as_str(),
            )?;
            audit_eq(
                "blotter_accounts.trading_date",
                &trading_date,
                &core.trading_date.to_string(),
            )?;
            audit_eq(
                "blotter_accounts.next_settlement_date",
                &next_settlement_date,
                &core.next_settlement_date.to_string(),
            )?;
            audit_eq(
                "blotter_accounts.effective_from",
                &sql_eff,
                &core.effective_from,
            )?;
            audit_eq(
                "blotter_accounts.watermark_time",
                &sql_wm,
                &core.watermark_time,
            )?;
            audit_eq(
                "blotter_accounts.watermark_source_seq",
                &watermark_source_seq,
                &core.watermark_source_seq,
            )?;
            audit_eq("blotter_accounts.source_id", &source_id, &core.source_id)?;
            audit_eq(
                "blotter_accounts.source_start_after_feed_seq",
                &source_start_after_feed_seq,
                &core.source_start_after_feed_seq,
            )?;
            audit_eq(
                "blotter_accounts.opening_cash_t0",
                &opening_cash_t0,
                &format_fixed_decimal(core.opening_cash_t0),
            )?;
            audit_eq(
                "blotter_accounts.opening_net_cash_delta_t1",
                &opening_net_cash_delta_t1,
                &format_fixed_decimal(core.opening_net_cash_delta_t1),
            )?;
            audit_eq(
                "blotter_accounts.policy_version",
                &policy_version,
                &policy.policy_version,
            )?;
            audit_eq(
                "blotter_accounts.default_quote_source_id",
                &default_quote_source_id,
                &policy.default_quote_source_id,
            )?;

            let mut bal_map = BTreeMap::new();
            for b in &core.balances {
                bal_map.insert(b.instrument_code.clone(), b.clone());
            }
            accounts.push(ManifestAccountSummary {
                account_id: account_id.clone(),
                trading_date,
                next_settlement_date,
            });
            account_policies.insert(account_id.clone(), policy);
            account_opening_balances.insert(account_id, bal_map);
        }
    }

    // Table 3: blotter_instruments
    {
        let mut stmt = conn
            .prepare(
                "SELECT account_id, instrument_code, active_quote_source_id,
                        policy_version, has_catalog_entry, catalog_entry_json
                 FROM blotter_instruments ORDER BY account_id ASC, instrument_code ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let account_id: String = row_get(row, 0)?;
            let instrument_code: String = row_get(row, 1)?;
            let active_quote_source_id: String = row_get(row, 2)?;
            let policy_version = sqlite_i64_to_u64(row_get::<i64>(row, 3)?).map_err(|_| {
                RolloverError::AuditFailed("invalid instrument policy_version".to_owned())
            })?;
            let has_catalog_entry: i64 = row_get(row, 4)?;
            let catalog_entry_json: Option<String> = row_get(row, 5)?;

            let policy = account_policies.get(&account_id).ok_or_else(|| {
                RolloverError::AuditFailed(format!(
                    "blotter_instruments references unknown account '{account_id}'"
                ))
            })?;
            audit_eq(
                "blotter_instruments.policy_version",
                &policy_version,
                &policy.policy_version,
            )?;
            audit_eq(
                "blotter_instruments.active_quote_source_id",
                &active_quote_source_id.as_str(),
                &policy.active_quote_source_for(&instrument_code),
            )?;
            match (has_catalog_entry, catalog_entry_json) {
                (1, Some(json)) => {
                    let entry: InstrumentCatalogEntry =
                        deserialize_json::<InstrumentCatalogEntryDto>(&json)
                            .and_then(InstrumentCatalogEntryDto::into_app)
                            .map_err(|_| {
                                RolloverError::AuditFailed(format!(
                                    "invalid catalog_entry_json for '{instrument_code}'"
                                ))
                            })?;
                    audit_eq(
                        "blotter_instruments.catalog_entry.instrument_code",
                        &instrument_code,
                        &entry.instrument_code,
                    )?;
                }
                (0, None) => {}
                _ => {
                    return Err(RolloverError::AuditFailed(format!(
                        "blotter_instruments has_catalog_entry/catalog_entry_json mismatch for '{instrument_code}'"
                    )));
                }
            }
        }
    }

    // Table 4: blotter_positions
    {
        let mut stmt = conn
            .prepare(
                "SELECT account_id, instrument_code, position_version,
                        opening_economic_notional, opening_available_notional,
                        opening_net_t0_notional, opening_net_t1_notional,
                        reconciled_buy_t0, reconciled_buy_t1,
                        reconciled_sell_t0, reconciled_sell_t1
                 FROM blotter_positions ORDER BY account_id ASC, instrument_code ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let account_id: String = row_get(row, 0)?;
            let instrument_code: String = row_get(row, 1)?;
            let position_version = sqlite_i64_to_u64(row_get::<i64>(row, 2)?)
                .map_err(|_| RolloverError::AuditFailed("invalid position_version".to_owned()))?;
            if position_version < 1 {
                return Err(RolloverError::AuditFailed(
                    "position_version must be >= 1".to_owned(),
                ));
            }
            let open_econ = parse_canonical_fixed_col(&row_get::<String>(row, 3)?)?;
            let open_avail = parse_canonical_fixed_col(&row_get::<String>(row, 4)?)?;
            let open_t0 = parse_canonical_fixed_col(&row_get::<String>(row, 5)?)?;
            let open_t1 = parse_canonical_fixed_col(&row_get::<String>(row, 6)?)?;
            let _rec_buy_t0 = parse_canonical_fixed_col(&row_get::<String>(row, 7)?)?;
            let _rec_buy_t1 = parse_canonical_fixed_col(&row_get::<String>(row, 8)?)?;
            let _rec_sell_t0 = parse_canonical_fixed_col(&row_get::<String>(row, 9)?)?;
            let _rec_sell_t1 = parse_canonical_fixed_col(&row_get::<String>(row, 10)?)?;

            let bals = account_opening_balances.get(&account_id).ok_or_else(|| {
                RolloverError::AuditFailed(format!(
                    "blotter_positions references unknown account '{account_id}'"
                ))
            })?;
            let (exp_econ, exp_avail, exp_t0, exp_t1) = match bals.get(&instrument_code) {
                Some(b) => (
                    b.opening_economic_notional,
                    b.opening_available_notional,
                    b.opening_net_t0_notional,
                    b.opening_net_t1_notional,
                ),
                None => (
                    FixedDecimal::ZERO,
                    FixedDecimal::ZERO,
                    FixedDecimal::ZERO,
                    FixedDecimal::ZERO,
                ),
            };
            audit_eq(
                "blotter_positions.opening_economic_notional",
                &open_econ,
                &exp_econ,
            )?;
            audit_eq(
                "blotter_positions.opening_available_notional",
                &open_avail,
                &exp_avail,
            )?;
            audit_eq(
                "blotter_positions.opening_net_t0_notional",
                &open_t0,
                &exp_t0,
            )?;
            audit_eq(
                "blotter_positions.opening_net_t1_notional",
                &open_t1,
                &exp_t1,
            )?;
        }
    }

    // Table 5: blotter_earmarks
    {
        let mut stmt = conn
            .prepare(
                "SELECT earmark_id, account_id, instrument_code, earmark_revision,
                        side, settlement_bucket, settlement_date, state,
                        notional, unreconciled_notional, locked_settlement_cash,
                        unreconciled_settlement_cash, funding_precision,
                        created_by, confirmed_by_trader_id,
                        created_seq, updated_seq, record_json
                 FROM blotter_earmarks ORDER BY earmark_id ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let earmark_id: String = row_get(row, 0)?;
            let account_id: String = row_get(row, 1)?;
            let instrument_code: String = row_get(row, 2)?;
            let earmark_revision = sqlite_i64_to_u64(row_get::<i64>(row, 3)?)
                .map_err(|_| RolloverError::AuditFailed("invalid earmark_revision".to_owned()))?;
            let side: String = row_get(row, 4)?;
            let settlement_bucket: String = row_get(row, 5)?;
            let settlement_date: String = row_get(row, 6)?;
            let state: String = row_get(row, 7)?;
            let notional: String = row_get(row, 8)?;
            let unreconciled_notional: String = row_get(row, 9)?;
            let locked_settlement_cash: Option<String> = row_get(row, 10)?;
            let unreconciled_settlement_cash: Option<String> = row_get(row, 11)?;
            let funding_precision: String = row_get(row, 12)?;
            let created_by: String = row_get(row, 13)?;
            let confirmed_by_trader_id: Option<String> = row_get(row, 14)?;
            let created_seq = sqlite_i64_to_u64(row_get::<i64>(row, 15)?).map_err(|_| {
                RolloverError::AuditFailed("invalid earmark created_seq".to_owned())
            })?;
            let updated_seq = sqlite_i64_to_u64(row_get::<i64>(row, 16)?).map_err(|_| {
                RolloverError::AuditFailed("invalid earmark updated_seq".to_owned())
            })?;
            let record_json: String = row_get(row, 17)?;

            let em = deserialize_json::<EarmarkRecordDto>(&record_json)
                .and_then(EarmarkRecordDto::into_domain)
                .map_err(|_| {
                    RolloverError::AuditFailed(format!(
                        "invalid earmark record_json '{earmark_id}'"
                    ))
                })?;

            audit_eq(
                "blotter_earmarks.earmark_id",
                &earmark_id.as_str(),
                &em.earmark_id.as_str(),
            )?;
            audit_eq("blotter_earmarks.account_id", &account_id, &em.account_id)?;
            audit_eq(
                "blotter_earmarks.instrument_code",
                &instrument_code,
                &em.instrument_code,
            )?;
            audit_eq(
                "blotter_earmarks.earmark_revision",
                &earmark_revision,
                &em.earmark_revision,
            )?;
            audit_eq(
                "blotter_earmarks.side",
                &side.as_str(),
                &side_to_str(em.side),
            )?;
            audit_eq(
                "blotter_earmarks.settlement_bucket",
                &settlement_bucket.as_str(),
                &bucket_to_str(em.settlement_bucket),
            )?;
            audit_eq(
                "blotter_earmarks.settlement_date",
                &settlement_date,
                &em.settlement_date.to_string(),
            )?;
            audit_eq(
                "blotter_earmarks.state",
                &state.as_str(),
                &earmark_state_to_str(em.state),
            )?;
            audit_eq(
                "blotter_earmarks.notional",
                &notional,
                &format_fixed_decimal(em.notional),
            )?;
            audit_eq(
                "blotter_earmarks.unreconciled_notional",
                &unreconciled_notional,
                &format_fixed_decimal(em.unreconciled_notional),
            )?;
            audit_eq(
                "blotter_earmarks.locked_settlement_cash",
                &locked_settlement_cash,
                &em.locked_settlement_cash.map(format_fixed_decimal),
            )?;
            audit_eq(
                "blotter_earmarks.unreconciled_settlement_cash",
                &unreconciled_settlement_cash,
                &em.unreconciled_settlement_cash.map(format_fixed_decimal),
            )?;
            audit_eq(
                "blotter_earmarks.funding_precision",
                &funding_precision.as_str(),
                &funding_precision_to_str(em.funding_precision),
            )?;
            audit_eq("blotter_earmarks.created_by", &created_by, &em.created_by)?;
            audit_eq(
                "blotter_earmarks.confirmed_by_trader_id",
                &confirmed_by_trader_id,
                &em.confirmed_by_trader_id,
            )?;
            audit_eq(
                "blotter_earmarks.created_seq",
                &created_seq,
                &em.created_seq,
            )?;
            audit_eq(
                "blotter_earmarks.updated_seq",
                &updated_seq,
                &em.updated_seq,
            )?;

            match em.state {
                EarmarkState::Hold => counts.active_hold += 1,
                EarmarkState::Committed => counts.unresolved_committed += 1,
                EarmarkState::Reconciled => counts.reconciled_committed += 1,
                EarmarkState::Expired => counts.expired_hold += 1,
                EarmarkState::Released => counts.released_hold += 1,
                EarmarkState::Cancelled => counts.cancelled_committed += 1,
            }
        }
    }

    // Table 6: blotter_draft_earmarks (full drafts AND legal record_json IS NULL presence drafts)
    {
        let mut stmt = conn
            .prepare(
                "SELECT draft_id, account_id, instrument_code, draft_revision,
                        status, created_seq, confirmation_token, context_hash,
                        confirmed_by_trader_id, record_json
                 FROM blotter_draft_earmarks ORDER BY draft_id ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let draft_id: String = row_get(row, 0)?;
            let account_id: String = row_get(row, 1)?;
            let instrument_code: String = row_get(row, 2)?;
            let draft_revision = sqlite_i64_to_u64(row_get::<i64>(row, 3)?)
                .map_err(|_| RolloverError::AuditFailed("invalid draft_revision".to_owned()))?;
            let status: String = row_get(row, 4)?;
            let created_seq = sqlite_i64_to_u64(row_get::<i64>(row, 5)?)
                .map_err(|_| RolloverError::AuditFailed("invalid draft created_seq".to_owned()))?;
            let confirmation_token: Vec<u8> = row_get(row, 6)?;
            let context_hash: Vec<u8> = row_get(row, 7)?;
            let confirmed_by_trader_id: Option<String> = row_get(row, 8)?;
            let record_json: Option<String> = row_get(row, 9)?;

            parse_ulid(&draft_id).map_err(|_| {
                RolloverError::AuditFailed(format!("invalid draft_id '{draft_id}'"))
            })?;
            if created_seq > head_seq {
                return Err(RolloverError::AuditFailed(format!(
                    "draft '{draft_id}' created_seq {created_seq} > head_seq {head_seq}"
                )));
            }
            content_hash_from_blob(&confirmation_token).map_err(|_| {
                RolloverError::AuditFailed("invalid draft confirmation_token blob".to_owned())
            })?;
            content_hash_from_blob(&context_hash).map_err(|_| {
                RolloverError::AuditFailed("invalid draft context_hash blob".to_owned())
            })?;

            if let Some(json) = record_json {
                let draft = deserialize_json::<DraftEarmarkRecordDto>(&json)
                    .and_then(DraftEarmarkRecordDto::into_app)
                    .map_err(|_| {
                        RolloverError::AuditFailed(format!(
                            "invalid draft record_json for '{draft_id}'"
                        ))
                    })?;
                audit_eq(
                    "blotter_draft_earmarks.draft_id",
                    &draft_id.as_str(),
                    &draft.draft_id.as_str(),
                )?;
                audit_eq(
                    "blotter_draft_earmarks.account_id",
                    &account_id,
                    &draft.account_id,
                )?;
                audit_eq(
                    "blotter_draft_earmarks.instrument_code",
                    &instrument_code,
                    &draft.instrument_code,
                )?;
                audit_eq(
                    "blotter_draft_earmarks.draft_revision",
                    &draft_revision,
                    &draft.draft_revision,
                )?;
                audit_eq(
                    "blotter_draft_earmarks.status",
                    &status.as_str(),
                    &draft_status_to_str(draft.status),
                )?;
                audit_eq(
                    "blotter_draft_earmarks.confirmation_token",
                    &confirmation_token.as_slice(),
                    &draft.confirmation_token.as_bytes().as_slice(),
                )?;
                audit_eq(
                    "blotter_draft_earmarks.context_hash",
                    &context_hash.as_slice(),
                    &draft.context_hash.as_bytes().as_slice(),
                )?;
                audit_eq(
                    "blotter_draft_earmarks.confirmed_by_trader_id",
                    &confirmed_by_trader_id,
                    &draft.confirmed_by_trader_id,
                )?;
                if draft.status == DraftEarmarkStatus::PendingConfirmation {
                    counts.pending_draft += 1;
                }
            } else {
                // Legal presence-only draft written by `store.rs:1174-1182`.
                audit_eq(
                    "blotter_draft_earmarks.null.draft_revision",
                    &draft_revision,
                    &1_u64,
                )?;
                audit_eq(
                    "blotter_draft_earmarks.null.status",
                    &status.as_str(),
                    &"PENDING_CONFIRMATION",
                )?;
                audit_eq(
                    "blotter_draft_earmarks.null.confirmation_token",
                    &confirmation_token.as_slice(),
                    &[0_u8; 32].as_slice(),
                )?;
                audit_eq(
                    "blotter_draft_earmarks.null.context_hash",
                    &context_hash.as_slice(),
                    &[0_u8; 32].as_slice(),
                )?;
                if confirmed_by_trader_id.is_some() {
                    return Err(RolloverError::AuditFailed(format!(
                        "presence-only draft '{draft_id}' must have NULL confirmed_by_trader_id"
                    )));
                }
                counts.pending_draft += 1;
            }
        }
    }

    // Table 7: blotter_feed_status
    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, feed_status_revision, initial_after_feed_seq,
                        contiguous_feed_seq, max_seen_feed_seq, missing_feed_ranges_json,
                        paused_by_gap, paused_by_staleness, paused_by_operator,
                        last_heartbeat_at_json, status_reason, source_batches_json
                 FROM blotter_feed_status ORDER BY source_id ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let source_id: String = row_get(row, 0)?;
            let feed_status_revision =
                sqlite_i64_to_u64(row_get::<i64>(row, 1)?).map_err(|_| {
                    RolloverError::AuditFailed("invalid feed_status_revision".to_owned())
                })?;
            let initial_after_feed_seq =
                sqlite_i64_to_u64(row_get::<i64>(row, 2)?).map_err(|_| {
                    RolloverError::AuditFailed("invalid initial_after_feed_seq".to_owned())
                })?;
            let contiguous_feed_seq = sqlite_i64_to_u64(row_get::<i64>(row, 3)?).map_err(|_| {
                RolloverError::AuditFailed("invalid contiguous_feed_seq".to_owned())
            })?;
            let max_seen_feed_seq = sqlite_i64_to_u64(row_get::<i64>(row, 4)?)
                .map_err(|_| RolloverError::AuditFailed("invalid max_seen_feed_seq".to_owned()))?;
            let missing_json: String = row_get(row, 5)?;
            let paused_gap: i64 = row_get(row, 6)?;
            let paused_stale: i64 = row_get(row, 7)?;
            let paused_op: i64 = row_get(row, 8)?;
            let hb_json: Option<String> = row_get(row, 9)?;
            let _status_reason: String = row_get(row, 10)?;
            let batches_json: String = row_get(row, 11)?;

            if source_id.is_empty()
                || feed_status_revision < 1
                || contiguous_feed_seq < initial_after_feed_seq
                || max_seen_feed_seq < contiguous_feed_seq
                || !(0..=1).contains(&paused_gap)
                || !(0..=1).contains(&paused_stale)
                || !(0..=1).contains(&paused_op)
            {
                return Err(RolloverError::AuditFailed(format!(
                    "blotter_feed_status invariants failed for '{source_id}'"
                )));
            }
            let missing: Vec<SeqRangeDto> = deserialize_json(&missing_json).map_err(|_| {
                RolloverError::AuditFailed("invalid missing_feed_ranges_json".to_owned())
            })?;
            let mut prev_end = 0_u64;
            for r in &missing {
                if r.first < 1 || r.first > r.last || r.first <= prev_end {
                    return Err(RolloverError::AuditFailed(
                        "invalid missing_feed_ranges ordering/bounds".to_owned(),
                    ));
                }
                prev_end = r.last;
            }
            if let Some(j) = hb_json {
                deserialize_json::<MarketTimeDto>(&j)
                    .and_then(MarketTimeDto::into_domain)
                    .map_err(|_| {
                        RolloverError::AuditFailed("invalid last_heartbeat_at_json".to_owned())
                    })?;
            }
            let batches: Vec<SourceBatchRecordDto> =
                deserialize_json(&batches_json).map_err(|_| {
                    RolloverError::AuditFailed("invalid source_batches_json".to_owned())
                })?;
            for b in &batches {
                if b.first_feed_seq < 1 || b.first_feed_seq > b.last_feed_seq {
                    return Err(RolloverError::AuditFailed(
                        "invalid source_batches bounds".to_owned(),
                    ));
                }
            }
        }
    }

    // Table 8: blotter_feed_raw_packets
    let mut raw_packet_index: BTreeMap<(String, u64), (String, u64, [u8; 32])> = BTreeMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, feed_seq, external_trade_id, source_revision,
                        payload_hash, business_hash, batch_first_feed_seq,
                        batch_last_feed_seq, record_json
                 FROM blotter_feed_raw_packets ORDER BY source_id ASC, feed_seq ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let source_id: String = row_get(row, 0)?;
            let feed_seq = sqlite_i64_to_u64(row_get::<i64>(row, 1)?).map_err(|_| {
                RolloverError::AuditFailed("invalid raw packet feed_seq".to_owned())
            })?;
            let external_trade_id: String = row_get(row, 2)?;
            let source_revision = sqlite_i64_to_u64(row_get::<i64>(row, 3)?).map_err(|_| {
                RolloverError::AuditFailed("invalid raw packet revision".to_owned())
            })?;
            let payload_hash_blob: Vec<u8> = row_get(row, 4)?;
            let business_hash_blob: Vec<u8> = row_get(row, 5)?;
            let batch_first = sqlite_i64_to_u64(row_get::<i64>(row, 6)?).map_err(|_| {
                RolloverError::AuditFailed("invalid batch_first_feed_seq".to_owned())
            })?;
            let batch_last = sqlite_i64_to_u64(row_get::<i64>(row, 7)?).map_err(|_| {
                RolloverError::AuditFailed("invalid batch_last_feed_seq".to_owned())
            })?;
            let record_json: String = row_get(row, 8)?;

            let rec: TradeFillRecord = deserialize_json::<TradeFillRecordDto>(&record_json)
                .and_then(TradeFillRecordDto::into_domain)
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid raw packet record_json".to_owned())
                })?;

            audit_eq(
                "blotter_feed_raw_packets.source_id",
                &source_id,
                &rec.source_id,
            )?;
            audit_eq(
                "blotter_feed_raw_packets.feed_seq",
                &feed_seq,
                &rec.feed_seq,
            )?;
            audit_eq(
                "blotter_feed_raw_packets.external_trade_id",
                &external_trade_id,
                &rec.external_trade_id,
            )?;
            audit_eq(
                "blotter_feed_raw_packets.source_revision",
                &source_revision,
                &rec.source_revision,
            )?;
            audit_eq(
                "blotter_feed_raw_packets.batch_first_feed_seq",
                &batch_first,
                &rec.batch_first_feed_seq,
            )?;
            audit_eq(
                "blotter_feed_raw_packets.batch_last_feed_seq",
                &batch_last,
                &rec.batch_last_feed_seq,
            )?;
            if batch_first > feed_seq || feed_seq > batch_last {
                return Err(RolloverError::AuditFailed(
                    "raw packet feed_seq outside batch bounds".to_owned(),
                ));
            }
            let expected_payload = rec.payload_hash();
            let expected_business = rec.business_hash();
            audit_eq(
                "blotter_feed_raw_packets.payload_hash",
                &payload_hash_blob.as_slice(),
                &expected_payload.as_bytes().as_slice(),
            )?;
            audit_eq(
                "blotter_feed_raw_packets.business_hash",
                &business_hash_blob.as_slice(),
                &expected_business.as_bytes().as_slice(),
            )?;
            raw_packet_index.insert(
                (source_id, feed_seq),
                (
                    external_trade_id,
                    source_revision,
                    *expected_business.as_bytes(),
                ),
            );
        }
    }

    // Table 9: blotter_ingested_trades
    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, external_trade_id, source_revision,
                        business_hash, canonical_feed_seq, alias_feed_seqs_json,
                        lifecycle, record_json
                 FROM blotter_ingested_trades
                 ORDER BY source_id ASC, external_trade_id ASC, source_revision ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let source_id: String = row_get(row, 0)?;
            let external_trade_id: String = row_get(row, 1)?;
            let source_revision = sqlite_i64_to_u64(row_get::<i64>(row, 2)?).map_err(|_| {
                RolloverError::AuditFailed("invalid ingested source_revision".to_owned())
            })?;
            let business_hash_blob: Vec<u8> = row_get(row, 3)?;
            let canonical_feed_seq = sqlite_i64_to_u64(row_get::<i64>(row, 4)?)
                .map_err(|_| RolloverError::AuditFailed("invalid canonical_feed_seq".to_owned()))?;
            let alias_json: String = row_get(row, 5)?;
            let lifecycle: String = row_get(row, 6)?;
            let record_json: String = row_get(row, 7)?;

            let rec = deserialize_json::<TradeFillRecordDto>(&record_json)
                .and_then(TradeFillRecordDto::into_domain)
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid ingested trade record_json".to_owned())
                })?;
            trade_lifecycle_from_str(&lifecycle).map_err(|_| {
                RolloverError::AuditFailed(format!("invalid trade lifecycle '{lifecycle}'"))
            })?;
            let aliases: BTreeSet<u64> = deserialize_json(&alias_json).map_err(|_| {
                RolloverError::AuditFailed("invalid alias_feed_seqs_json".to_owned())
            })?;

            audit_eq(
                "blotter_ingested_trades.source_id",
                &source_id,
                &rec.source_id,
            )?;
            audit_eq(
                "blotter_ingested_trades.external_trade_id",
                &external_trade_id,
                &rec.external_trade_id,
            )?;
            audit_eq(
                "blotter_ingested_trades.source_revision",
                &source_revision,
                &rec.source_revision,
            )?;
            audit_eq(
                "blotter_ingested_trades.canonical_feed_seq",
                &canonical_feed_seq,
                &rec.feed_seq,
            )?;
            let expected_biz = rec.business_hash();
            audit_eq(
                "blotter_ingested_trades.business_hash",
                &business_hash_blob.as_slice(),
                &expected_biz.as_bytes().as_slice(),
            )?;

            for seq in std::iter::once(canonical_feed_seq).chain(aliases) {
                let Some((pkt_tid, pkt_rev, pkt_biz)) =
                    raw_packet_index.get(&(source_id.clone(), seq))
                else {
                    return Err(RolloverError::AuditFailed(format!(
                        "ingested trade references missing raw packet ({source_id}, {seq})"
                    )));
                };
                if pkt_tid != &external_trade_id
                    || *pkt_rev != source_revision
                    || pkt_biz != expected_biz.as_bytes()
                {
                    return Err(RolloverError::AuditFailed(format!(
                        "ingested trade disagrees with raw packet ({source_id}, {seq})"
                    )));
                }
            }
        }
    }

    // Table 10: blotter_trade_effective_revisions
    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, external_trade_id, latest_effective_revision,
                        original_new_fill_json, baseline_original_evidence_json,
                        held_revisions_json, missing_predecessor_revisions_json
                 FROM blotter_trade_effective_revisions
                 ORDER BY source_id ASC, external_trade_id ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let source_id: String = row_get(row, 0)?;
            let external_trade_id: String = row_get(row, 1)?;
            let latest_rev = row_get::<Option<i64>>(row, 2)?
                .map(sqlite_i64_to_u64)
                .transpose()
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid latest_effective_revision".to_owned())
                })?;
            if let Some(r) = latest_rev
                && r < 1
            {
                return Err(RolloverError::AuditFailed(
                    "latest_effective_revision must be >= 1".to_owned(),
                ));
            }
            if let Some(j) = row_get::<Option<String>>(row, 3)? {
                let orig = deserialize_json::<TradeFillRecordDto>(&j)
                    .and_then(TradeFillRecordDto::into_domain)
                    .map_err(|_| {
                        RolloverError::AuditFailed("invalid original_new_fill_json".to_owned())
                    })?;
                audit_eq(
                    "blotter_trade_effective_revisions.original.source_id",
                    &source_id,
                    &orig.source_id,
                )?;
                audit_eq(
                    "blotter_trade_effective_revisions.original.external_trade_id",
                    &external_trade_id,
                    &orig.external_trade_id,
                )?;
            }
            if let Some(j) = row_get::<Option<String>>(row, 4)? {
                let ev = deserialize_json::<BaseEvidenceDto>(&j)
                    .and_then(BaseEvidenceDto::into_domain)
                    .map_err(|_| {
                        RolloverError::AuditFailed(
                            "invalid baseline_original_evidence_json".to_owned(),
                        )
                    })?;
                audit_eq(
                    "blotter_trade_effective_revisions.evidence.source_id",
                    &source_id,
                    &ev.source_id,
                )?;
                audit_eq(
                    "blotter_trade_effective_revisions.evidence.external_trade_id",
                    &external_trade_id,
                    &ev.external_trade_id,
                )?;
            }
            let held = parse_u64_set(&row_get::<String>(row, 5)?).map_err(|_| {
                RolloverError::AuditFailed("invalid held_revisions_json".to_owned())
            })?;
            if held.contains(&0) {
                return Err(RolloverError::AuditFailed(
                    "held_revisions cannot contain 0".to_owned(),
                ));
            }
            let missing_pred = parse_u64_set(&row_get::<String>(row, 6)?).map_err(|_| {
                RolloverError::AuditFailed("invalid missing_predecessor_revisions_json".to_owned())
            })?;
            if missing_pred.contains(&0) {
                return Err(RolloverError::AuditFailed(
                    "missing_predecessor_revisions cannot contain 0".to_owned(),
                ));
            }
        }
    }

    // Table 11: blotter_pending_matches
    {
        let mut stmt = conn
            .prepare(
                "SELECT pending_match_id, account_id, instrument_code, pending_revision,
                        source_id, external_trade_id, source_revision,
                        state, reason, original_notional, unresolved_notional,
                        unresolved_signed_cash, resolution_actor_id, resolution_reason,
                        created_seq, updated_seq, record_json
                 FROM blotter_pending_matches ORDER BY pending_match_id ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let pm_id: String = row_get(row, 0)?;
            let account_id: String = row_get(row, 1)?;
            let instrument_code: String = row_get(row, 2)?;
            let pending_revision = sqlite_i64_to_u64(row_get::<i64>(row, 3)?)
                .map_err(|_| RolloverError::AuditFailed("invalid pending_revision".to_owned()))?;
            let source_id: String = row_get(row, 4)?;
            let external_trade_id: String = row_get(row, 5)?;
            let source_revision = sqlite_i64_to_u64(row_get::<i64>(row, 6)?)
                .map_err(|_| RolloverError::AuditFailed("invalid pm source_revision".to_owned()))?;
            let state: String = row_get(row, 7)?;
            let reason: String = row_get(row, 8)?;
            let original_notional: String = row_get(row, 9)?;
            let unresolved_notional: String = row_get(row, 10)?;
            let unresolved_signed_cash: Option<String> = row_get(row, 11)?;
            let resolution_actor_id: Option<String> = row_get(row, 12)?;
            let resolution_reason: Option<String> = row_get(row, 13)?;
            let created_seq = sqlite_i64_to_u64(row_get::<i64>(row, 14)?)
                .map_err(|_| RolloverError::AuditFailed("invalid pm created_seq".to_owned()))?;
            let updated_seq = sqlite_i64_to_u64(row_get::<i64>(row, 15)?)
                .map_err(|_| RolloverError::AuditFailed("invalid pm updated_seq".to_owned()))?;
            let record_json: String = row_get(row, 16)?;

            let pm = deserialize_json::<PendingMatchRecordDto>(&record_json)
                .and_then(PendingMatchRecordDto::into_domain)
                .map_err(|_| {
                    RolloverError::AuditFailed(format!(
                        "invalid pending_match record_json '{pm_id}'"
                    ))
                })?;

            audit_eq(
                "blotter_pending_matches.pending_match_id",
                &pm_id.as_str(),
                &pm.pending_match_id.as_str(),
            )?;
            audit_eq(
                "blotter_pending_matches.account_id",
                &account_id,
                &pm.account_id,
            )?;
            audit_eq(
                "blotter_pending_matches.instrument_code",
                &instrument_code,
                &pm.instrument_code,
            )?;
            audit_eq(
                "blotter_pending_matches.pending_revision",
                &pending_revision,
                &pm.pending_revision,
            )?;
            audit_eq(
                "blotter_pending_matches.source_id",
                &source_id,
                &pm.source_id,
            )?;
            audit_eq(
                "blotter_pending_matches.external_trade_id",
                &external_trade_id,
                &pm.external_trade_id,
            )?;
            audit_eq(
                "blotter_pending_matches.source_revision",
                &source_revision,
                &pm.source_revision,
            )?;
            audit_eq(
                "blotter_pending_matches.state",
                &state.as_str(),
                &pending_state_to_str(pm.state),
            )?;
            audit_eq(
                "blotter_pending_matches.reason",
                &reason.as_str(),
                &pending_reason_to_str(pm.reason),
            )?;
            audit_eq(
                "blotter_pending_matches.original_notional",
                &original_notional,
                &format_fixed_decimal(pm.original_notional),
            )?;
            audit_eq(
                "blotter_pending_matches.unresolved_notional",
                &unresolved_notional,
                &format_fixed_decimal(pm.unresolved_notional),
            )?;
            audit_eq(
                "blotter_pending_matches.unresolved_signed_cash",
                &unresolved_signed_cash,
                &pm.unresolved_signed_cash.map(format_fixed_decimal),
            )?;
            audit_eq(
                "blotter_pending_matches.resolution_actor_id",
                &resolution_actor_id,
                &pm.resolution_actor_id,
            )?;
            audit_eq(
                "blotter_pending_matches.resolution_reason",
                &resolution_reason,
                &pm.resolution_reason,
            )?;
            audit_eq(
                "blotter_pending_matches.created_seq",
                &created_seq,
                &pm.created_seq,
            )?;
            audit_eq(
                "blotter_pending_matches.updated_seq",
                &updated_seq,
                &pm.updated_seq,
            )?;

            match pm.state {
                PendingMatchState::Open => counts.open_pending_match += 1,
                PendingMatchState::ResolvedMatched
                | PendingMatchState::ResolvedOrphanAccepted
                | PendingMatchState::Released
                | PendingMatchState::ReversedByFeed
                | PendingMatchState::ResolvedBaselineDelta => counts.resolved_pending_match += 1,
            }
        }
    }

    // Table 12: blotter_reconcile_allocations
    {
        let mut stmt = conn
            .prepare(
                "SELECT allocation_id, account_id, instrument_code,
                        source_id, external_trade_id, source_revision,
                        earmark_id, pending_match_id,
                        allocated_notional, allocated_fill_cash, released_earmark_cash,
                        fill_segment_index, earmark_segment_index, reversed_by_revision,
                        actor_id, baseline_delta_json, record_json
                 FROM blotter_reconcile_allocations ORDER BY allocation_id ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let allocation_id: String = row_get(row, 0)?;
            let account_id: String = row_get(row, 1)?;
            let instrument_code: String = row_get(row, 2)?;
            let source_id: String = row_get(row, 3)?;
            let external_trade_id: String = row_get(row, 4)?;
            let source_revision = sqlite_i64_to_u64(row_get::<i64>(row, 5)?).map_err(|_| {
                RolloverError::AuditFailed("invalid alloc source_revision".to_owned())
            })?;
            let earmark_id: Option<String> = row_get(row, 6)?;
            let pending_match_id: Option<String> = row_get(row, 7)?;
            let allocated_notional: String = row_get(row, 8)?;
            let allocated_fill_cash: Option<String> = row_get(row, 9)?;
            let released_earmark_cash: Option<String> = row_get(row, 10)?;
            let fill_seg = row_get::<Option<i64>>(row, 11)?
                .map(|v| {
                    u32::try_from(v).map_err(|_| {
                        RolloverError::AuditFailed("invalid fill_segment_index".to_owned())
                    })
                })
                .transpose()?;
            let earmark_seg = row_get::<Option<i64>>(row, 12)?
                .map(|v| {
                    u32::try_from(v).map_err(|_| {
                        RolloverError::AuditFailed("invalid earmark_segment_index".to_owned())
                    })
                })
                .transpose()?;
            let reversed_by = row_get::<Option<i64>>(row, 13)?
                .map(sqlite_i64_to_u64)
                .transpose()
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid reversed_by_revision".to_owned())
                })?;
            let actor_id: String = row_get(row, 14)?;
            let baseline_delta_json: Option<String> = row_get(row, 15)?;
            let record_json: String = row_get(row, 16)?;

            let alloc = deserialize_json::<ReconcileAllocationRecordDto>(&record_json)
                .and_then(ReconcileAllocationRecordDto::into_domain)
                .map_err(|_| {
                    RolloverError::AuditFailed(format!(
                        "invalid reconcile allocation record_json '{allocation_id}'"
                    ))
                })?;

            audit_eq(
                "blotter_reconcile_allocations.allocation_id",
                &allocation_id.as_str(),
                &alloc.allocation_id.as_str(),
            )?;
            audit_eq(
                "blotter_reconcile_allocations.account_id",
                &account_id,
                &alloc.account_id,
            )?;
            audit_eq(
                "blotter_reconcile_allocations.instrument_code",
                &instrument_code,
                &alloc.instrument_code,
            )?;
            audit_eq(
                "blotter_reconcile_allocations.source_id",
                &source_id,
                &alloc.source_id,
            )?;
            audit_eq(
                "blotter_reconcile_allocations.external_trade_id",
                &external_trade_id,
                &alloc.external_trade_id,
            )?;
            audit_eq(
                "blotter_reconcile_allocations.source_revision",
                &source_revision,
                &alloc.source_revision,
            )?;
            audit_eq(
                "blotter_reconcile_allocations.earmark_id",
                &earmark_id,
                &alloc.earmark_id.as_ref().map(|u| u.as_str().to_owned()),
            )?;
            audit_eq(
                "blotter_reconcile_allocations.pending_match_id",
                &pending_match_id,
                &alloc
                    .pending_match_id
                    .as_ref()
                    .map(|u| u.as_str().to_owned()),
            )?;
            audit_eq(
                "blotter_reconcile_allocations.allocated_notional",
                &allocated_notional,
                &format_fixed_decimal(alloc.allocated_notional),
            )?;
            audit_eq(
                "blotter_reconcile_allocations.allocated_fill_cash",
                &allocated_fill_cash,
                &alloc.allocated_fill_cash.map(format_fixed_decimal),
            )?;
            audit_eq(
                "blotter_reconcile_allocations.released_earmark_cash",
                &released_earmark_cash,
                &alloc.released_earmark_cash.map(format_fixed_decimal),
            )?;
            audit_eq(
                "blotter_reconcile_allocations.fill_segment_index",
                &fill_seg,
                &alloc.fill_segment_index,
            )?;
            audit_eq(
                "blotter_reconcile_allocations.earmark_segment_index",
                &earmark_seg,
                &alloc.earmark_segment_index,
            )?;
            audit_eq(
                "blotter_reconcile_allocations.reversed_by_revision",
                &reversed_by,
                &alloc.reversed_by_revision,
            )?;
            audit_eq(
                "blotter_reconcile_allocations.actor_id",
                &actor_id,
                &alloc.actor_id,
            )?;

            let sql_bd = baseline_delta_json
                .as_deref()
                .map(|j| {
                    deserialize_json::<BaselineDeltaDto>(j).and_then(BaselineDeltaDto::into_domain)
                })
                .transpose()
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid baseline_delta_json".to_owned())
                })?;
            audit_eq(
                "blotter_reconcile_allocations.baseline_delta",
                &sql_bd,
                &alloc.baseline_delta,
            )?;
        }
    }

    // Table 13: blotter_quote_history
    let mut quote_max_seq: BTreeMap<(String, String), u64> = BTreeMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, instrument_code, quote_seq, payload_hash,
                        first_received_at_json, first_ingested_at_json, tick_json
                 FROM blotter_quote_history
                 ORDER BY source_id ASC, instrument_code ASC, quote_seq ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let source_id: String = row_get(row, 0)?;
            let instrument_code: String = row_get(row, 1)?;
            let quote_seq = sqlite_i64_to_u64(row_get::<i64>(row, 2)?)
                .map_err(|_| RolloverError::AuditFailed("invalid quote_seq".to_owned()))?;
            let payload_hash_blob: Vec<u8> = row_get(row, 3)?;
            let first_rec_json: String = row_get(row, 4)?;
            let first_ing_json: String = row_get(row, 5)?;
            let tick_json: String = row_get(row, 6)?;

            let tick = deserialize_json::<MarketQuoteTickDto>(&tick_json)
                .and_then(MarketQuoteTickDto::into_app)
                .map_err(|_| RolloverError::AuditFailed("invalid quote tick_json".to_owned()))?;
            let first_rec = deserialize_json::<MarketTimeDto>(&first_rec_json)
                .and_then(MarketTimeDto::into_domain)
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid quote first_received_at_json".to_owned())
                })?;
            let _first_ing = deserialize_json::<MarketTimeDto>(&first_ing_json)
                .and_then(MarketTimeDto::into_domain)
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid quote first_ingested_at_json".to_owned())
                })?;

            audit_eq(
                "blotter_quote_history.source_id",
                &source_id,
                &tick.source_id,
            )?;
            audit_eq(
                "blotter_quote_history.instrument_code",
                &instrument_code,
                &tick.instrument_code,
            )?;
            audit_eq(
                "blotter_quote_history.quote_seq",
                &quote_seq,
                &tick.quote_seq,
            )?;
            audit_eq(
                "blotter_quote_history.payload_hash",
                &payload_hash_blob.as_slice(),
                &tick.canonical_payload_hash().as_bytes().as_slice(),
            )?;
            audit_eq(
                "blotter_quote_history.first_received_at",
                &first_rec,
                &tick.received_at,
            )?;

            let entry = quote_max_seq
                .entry((source_id, instrument_code))
                .or_insert(0);
            *entry = (*entry).max(quote_seq);
        }
    }

    // Table 14: blotter_quote_heads
    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, instrument_code, head_quote_seq
                 FROM blotter_quote_heads ORDER BY source_id ASC, instrument_code ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut observed_heads: BTreeMap<(String, String), u64> = BTreeMap::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let source_id: String = row_get(row, 0)?;
            let instrument_code: String = row_get(row, 1)?;
            let head_quote_seq = sqlite_i64_to_u64(row_get::<i64>(row, 2)?)
                .map_err(|_| RolloverError::AuditFailed("invalid head_quote_seq".to_owned()))?;
            observed_heads.insert((source_id, instrument_code), head_quote_seq);
        }
        audit_eq("blotter_quote_heads", &observed_heads, &quote_max_seq)?;
    }

    // Table 15: blotter_idempotency_keys
    {
        let mut stmt = conn
            .prepare(
                "SELECT actor_id, rpc_name, scope_key, idempotency_key,
                        request_hash, committed_seq, response_payload
                 FROM blotter_idempotency_keys
                 ORDER BY actor_id ASC, rpc_name ASC, scope_key ASC, idempotency_key ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let actor_id: String = row_get(row, 0)?;
            let rpc_name: String = row_get(row, 1)?;
            let scope_key: String = row_get(row, 2)?;
            let idempotency_key: String = row_get(row, 3)?;
            let request_hash: Vec<u8> = row_get(row, 4)?;
            let committed_seq = sqlite_i64_to_u64(row_get::<i64>(row, 5)?).map_err(|_| {
                RolloverError::AuditFailed("invalid receipt committed_seq".to_owned())
            })?;
            let _response_payload: Vec<u8> = row_get(row, 6)?;

            if actor_id.is_empty()
                || rpc_name.is_empty()
                || scope_key.is_empty()
                || idempotency_key.is_empty()
                || committed_seq > head_seq
            {
                return Err(RolloverError::AuditFailed(
                    "invalid blotter_idempotency_keys row".to_owned(),
                ));
            }
            content_hash_from_blob(&request_hash).map_err(|_| {
                RolloverError::AuditFailed("invalid receipt request_hash".to_owned())
            })?;
        }
    }

    // Table 16: blotter_event_journal
    let mut expected_event_accounts: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT seq, occurred_at_json, event_type, envelope_json
                 FROM blotter_event_journal ORDER BY seq ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let seq = sqlite_i64_to_u64(row_get::<i64>(row, 0)?)
                .map_err(|_| RolloverError::AuditFailed("invalid event seq".to_owned()))?;
            let occurred_at_json: String = row_get(row, 1)?;
            let event_type: String = row_get(row, 2)?;
            let envelope_json: String = row_get(row, 3)?;

            if seq < 1 || seq > head_seq || seq < oldest_retained_seq {
                return Err(RolloverError::AuditFailed(format!(
                    "event seq {seq} out of retained range [{oldest_retained_seq}, {head_seq}]"
                )));
            }
            let occurred_at = deserialize_json::<MarketTimeDto>(&occurred_at_json)
                .and_then(MarketTimeDto::into_domain)
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid event occurred_at_json".to_owned())
                })?;
            let env = deserialize_json::<BlotterEventEnvelopeDto>(&envelope_json)
                .and_then(BlotterEventEnvelopeDto::into_app)
                .map_err(|_| {
                    RolloverError::AuditFailed("invalid event envelope_json".to_owned())
                })?;

            audit_eq("blotter_event_journal.seq", &seq, &env.seq)?;
            audit_eq(
                "blotter_event_journal.occurred_at",
                &occurred_at,
                &env.occurred_at,
            )?;
            audit_eq(
                "blotter_event_journal.event_type",
                &event_type,
                &env.event_type,
            )?;

            let accts: BTreeSet<String> =
                env.changes.iter().map(|c| c.account_id.clone()).collect();
            if !accts.is_empty() {
                expected_event_accounts.insert(seq, accts);
            }
            counts.retained_events += 1;
        }
    }

    // Table 17: blotter_event_accounts
    {
        let mut stmt = conn
            .prepare(
                "SELECT seq, account_id
                 FROM blotter_event_accounts ORDER BY seq ASC, account_id ASC",
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut actual_event_accounts: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
        {
            total_business_rows += 1;
            let seq = sqlite_i64_to_u64(row_get::<i64>(row, 0)?)
                .map_err(|_| RolloverError::AuditFailed("invalid event_accounts seq".to_owned()))?;
            let account_id: String = row_get(row, 1)?;
            actual_event_accounts
                .entry(seq)
                .or_default()
                .insert(account_id);
        }
        audit_eq(
            "blotter_event_accounts",
            &actual_event_accounts,
            &expected_event_accounts,
        )?;
    }

    // 5. Compute deterministic full-content 17-table SHA-256 (with virtual meta projection for v1).
    let content_sha256_hex = compute_canonical_content_sha256_hex(conn, meta_layout, &identity)?;

    Ok(DatabaseAuditSummary {
        meta_layout,
        identity,
        head_seq,
        oldest_retained_seq,
        content_sha256_hex,
        accounts,
        counts,
        business_tables_empty: total_business_rows == 0,
    })
}

fn parse_canonical_fixed_col(raw: &str) -> Result<FixedDecimal, RolloverError> {
    let val = parse_fixed_decimal_storage(raw)
        .map_err(|_| RolloverError::AuditFailed(format!("invalid FixedDecimal '{raw}'")))?;
    let formatted = format_fixed_decimal(val);
    if formatted != raw {
        return Err(RolloverError::AuditFailed(format!(
            "non-canonical FixedDecimal text '{raw}' (expected '{formatted}')"
        )));
    }
    Ok(val)
}

fn row_get<T: rusqlite::types::FromSql>(
    row: &rusqlite::Row<'_>,
    idx: usize,
) -> Result<T, RolloverError> {
    row.get(idx)
        .map_err(|e| RolloverError::AuditFailed(format!("column {idx} read failed: {e}")))
}

fn audit_eq<T: PartialEq + std::fmt::Debug>(
    field: &str,
    sql_val: &T,
    dto_val: &T,
) -> Result<(), RolloverError> {
    if sql_val == dto_val {
        Ok(())
    } else {
        Err(RolloverError::AuditFailed(format!(
            "dual-source mismatch on {field}: sql={sql_val:?}, dto={dto_val:?}"
        )))
    }
}

fn compute_legacy_v1_content_digest(conn: &Connection) -> Result<[u8; 32], RolloverError> {
    let mut hasher = Sha256::new();
    hasher.update(LEGACY_V1_CONTENT_DIGEST_PREFIX);
    hash_table_rows(
        conn,
        &mut hasher,
        "blotter_meta",
        BLOTTER_META_LEGACY_COLUMNS,
        "singleton_id ASC",
        None,
    )?;
    for &(table_name, cols) in REQUIRED_NON_META_TABLE_COLUMNS {
        hash_table_rows(
            conn,
            &mut hasher,
            table_name,
            cols,
            table_primary_key_order_by(table_name),
            None,
        )?;
    }
    let out = hasher.finalize();
    let mut arr = [0_u8; 32];
    arr.copy_from_slice(&out);
    Ok(arr)
}

fn compute_canonical_content_sha256_hex(
    conn: &Connection,
    meta_layout: MetaSchemaLayout,
    identity: &LedgerIdentity,
) -> Result<String, RolloverError> {
    let mut hasher = Sha256::new();
    hasher.update(CANONICAL_CONTENT_DIGEST_PREFIX);
    let virtual_ext = match meta_layout {
        MetaSchemaLayout::V1Legacy => Some(identity),
        MetaSchemaLayout::V1WithIdentity => None,
    };
    hash_table_rows(
        conn,
        &mut hasher,
        "blotter_meta",
        BLOTTER_META_EXTENDED_COLUMNS,
        "singleton_id ASC",
        virtual_ext,
    )?;
    for &(table_name, cols) in REQUIRED_NON_META_TABLE_COLUMNS {
        hash_table_rows(
            conn,
            &mut hasher,
            table_name,
            cols,
            table_primary_key_order_by(table_name),
            None,
        )?;
    }
    Ok(encode_hex(&hasher.finalize()))
}

fn hash_table_rows(
    conn: &Connection,
    hasher: &mut Sha256,
    table_name: &str,
    expected_cols: &[&str],
    order_by: &str,
    virtual_meta_identity: Option<&LedgerIdentity>,
) -> Result<(), RolloverError> {
    hasher.update([b'T']);
    hash_len_prefixed(hasher, table_name.as_bytes());
    hasher.update(
        u64::try_from(expected_cols.len())
            .unwrap_or(0)
            .to_be_bytes(),
    );
    for &col in expected_cols {
        hash_len_prefixed(hasher, col.as_bytes());
    }

    let select_cols = if virtual_meta_identity.is_some() {
        BLOTTER_META_LEGACY_COLUMNS.join(", ")
    } else {
        expected_cols.join(", ")
    };
    let sql = format!("SELECT {select_cols} FROM {table_name} ORDER BY {order_by}");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| RolloverError::Sqlite(e.to_string()))?;

    let mut row_count = 0_u64;
    let query_col_count = if virtual_meta_identity.is_some() {
        BLOTTER_META_LEGACY_COLUMNS.len()
    } else {
        expected_cols.len()
    };

    while let Some(row) = rows
        .next()
        .map_err(|e| RolloverError::Sqlite(e.to_string()))?
    {
        row_count += 1;
        hasher.update([b'R']);
        for i in 0..query_col_count {
            let val = row
                .get_ref(i)
                .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
            hash_sqlite_value(hasher, val);
        }
        if let Some(virt) = virtual_meta_identity {
            let epoch_i64 = i64::try_from(virt.ledger_epoch)
                .map_err(|_| RolloverError::AuditFailed("virtual epoch overflow".to_owned()))?;
            hash_sqlite_value(hasher, ValueRef::Integer(epoch_i64));
            hash_sqlite_value(hasher, ValueRef::Text(virt.ledger_instance_id.as_bytes()));
            match &virt.last_reset_operation_id {
                Some(op) => hash_sqlite_value(hasher, ValueRef::Text(op.as_bytes())),
                None => hash_sqlite_value(hasher, ValueRef::Null),
            }
        }
    }

    hasher.update([b'E']);
    hasher.update(row_count.to_be_bytes());
    Ok(())
}

fn hash_sqlite_value(hasher: &mut Sha256, val: ValueRef<'_>) {
    match val {
        ValueRef::Null => hasher.update([0_u8]),
        ValueRef::Integer(i) => {
            hasher.update([1_u8]);
            hasher.update(i.to_be_bytes());
        }
        ValueRef::Real(f) => {
            hasher.update([2_u8]);
            hasher.update(f.to_bits().to_be_bytes());
        }
        ValueRef::Text(bytes) => {
            hasher.update([3_u8]);
            hash_len_prefixed(hasher, bytes);
        }
        ValueRef::Blob(bytes) => {
            hasher.update([4_u8]);
            hash_len_prefixed(hasher, bytes);
        }
    }
}

fn hash_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(bytes);
}

fn table_primary_key_order_by(table_name: &str) -> &'static str {
    match table_name {
        "blotter_accounts" => "account_id ASC",
        "blotter_instruments" | "blotter_positions" => "account_id ASC, instrument_code ASC",
        "blotter_earmarks" => "earmark_id ASC",
        "blotter_draft_earmarks" => "draft_id ASC",
        "blotter_feed_status" => "source_id ASC",
        "blotter_feed_raw_packets" => "source_id ASC, feed_seq ASC",
        "blotter_ingested_trades" => "source_id ASC, external_trade_id ASC, source_revision ASC",
        "blotter_trade_effective_revisions" => "source_id ASC, external_trade_id ASC",
        "blotter_pending_matches" => "pending_match_id ASC",
        "blotter_reconcile_allocations" => "allocation_id ASC",
        "blotter_quote_history" => "source_id ASC, instrument_code ASC, quote_seq ASC",
        "blotter_quote_heads" => "source_id ASC, instrument_code ASC",
        "blotter_idempotency_keys" => {
            "actor_id ASC, rpc_name ASC, scope_key ASC, idempotency_key ASC"
        }
        "blotter_event_journal" => "seq ASC",
        "blotter_event_accounts" => "seq ASC, account_id ASC",
        _ => "rowid ASC",
    }
}

// ============================================================================
// Resumable 6-Window Rollover State Machine (F-02, F-17, F-18, F-19, F-22)
// ============================================================================

/// Executes offline Desk Blotter `SQLite` archival and optional reset (`--sqlite-rollover`).
pub fn sqlite_rollover(
    request: &SqliteRolloverRequest,
) -> Result<SqliteRolloverReport, RolloverError> {
    sqlite_rollover_with_fault(request, RolloverFaultInjection::None)
}

/// Executes offline Desk Blotter `SQLite` rollover with an optional deterministic crash window.
#[allow(clippy::too_many_lines)]
pub fn sqlite_rollover_with_fault(
    request: &SqliteRolloverRequest,
    fault: RolloverFaultInjection,
) -> Result<SqliteRolloverReport, RolloverError> {
    // 1. Validate request flags & stem before touching any file or lock.
    if request.archive_only && request.allow_unresolved {
        return Err(RolloverError::Validation(
            "--archive-only and --allow-unresolved cannot be combined".to_owned(),
        ));
    }
    validate_production_sqlite_path(&request.db_path)
        .map_err(|_| RolloverError::Validation("invalid source database path".to_owned()))?;
    if let Some(stem) = &request.archive_name {
        validate_archive_stem(stem)?;
    }

    let abs_db = normalize_and_verify_no_reparse(&request.db_path)?;
    let abs_archive_dir = normalize_and_verify_no_reparse(&request.archive_dir)?;

    if !abs_db.exists() {
        return Err(RolloverError::Validation(format!(
            "source database does not exist: '{}'",
            abs_db.display()
        )));
    }
    let source_pre_info = inspect_physical_file_identity(&abs_db, false)?;
    if source_pre_info.nlink != 1 {
        return Err(RolloverError::Validation(format!(
            "source database must have exactly 1 hardlink (observed {})",
            source_pre_info.nlink
        )));
    }

    if !abs_archive_dir.exists() {
        return Err(RolloverError::Validation(format!(
            "archive directory does not exist: '{}'",
            abs_archive_dir.display()
        )));
    }
    let archive_dir_info = inspect_physical_file_identity(&abs_archive_dir, true)?;

    // Pre-lock path/physical alias check when explicit stem is given (zero writes on alias).
    let lock_path = companion_lock_path(&abs_db);
    let state_path = rollover_state_path(&abs_db);
    if let Some(stem) = &request.archive_name {
        verify_rollover_path_isolation(&abs_db, &lock_path, &state_path, &abs_archive_dir, stem)?;
    }

    // 2. Acquire exclusive companion lock `<db>.rollover.lock` (busy_timeout=0, zero retry sleep).
    let lock = CompanionLock::acquire(&abs_db)?;
    let source_info = inspect_physical_file_identity(&abs_db, false)?;
    if source_info.nlink != 1 || source_info.id != source_pre_info.id {
        return Err(RolloverError::Validation(
            "source database identity or hardlink count changed under lock".to_owned(),
        ));
    }
    if source_info.id == lock.lock_identity().id {
        return Err(RolloverError::Validation(
            "source database aliases companion lock".to_owned(),
        ));
    }

    let req_mode = if request.archive_only {
        RolloverMode::ArchiveOnly
    } else {
        RolloverMode::RolloverReset
    };

    // 3. Read `<db>.rollover.state.json` BEFORE choosing default timestamp stem (`F-02`).
    let mut state_file = if state_path.exists() {
        verify_existing_entry_no_reparse(&state_path)?;
        read_rollover_state_file(&state_path)?
    } else {
        RolloverStateFile {
            schema: ROLLOVER_STATE_SCHEMA.to_owned(),
            active: None,
            last_completed: None,
        }
    };

    // 4. If an active unfinished operation exists, resume it (or reject mode/stem conflicts).
    if let Some(active) = state_file.active.clone() {
        if active.mode != req_mode {
            return Err(RolloverError::ModeConflict(format!(
                "active rollover operation '{}' has mode {:?}, cannot run {:?}",
                active.operation_id, active.mode, req_mode
            )));
        }
        if active.source_physical_id != source_info.id {
            return Err(RolloverError::StateConflict(
                "active rollover operation binds a different source physical file".to_owned(),
            ));
        }
        if active.archive_dir_physical_id != archive_dir_info.id {
            return Err(RolloverError::StateConflict(
                "active rollover operation binds a different archive directory".to_owned(),
            ));
        }
        if let Some(requested_stem) = &request.archive_name
            && requested_stem != &active.stem
        {
            return Err(RolloverError::ArchiveConflict(format!(
                "active rollover operation already bound to stem '{}', cannot use '{requested_stem}'",
                active.stem
            )));
        }
        verify_rollover_path_isolation(
            &abs_db,
            &lock_path,
            &state_path,
            &abs_archive_dir,
            &active.stem,
        )?;
        return drive_active_operation(
            &abs_db,
            &lock,
            &state_path,
            &abs_archive_dir,
            &mut state_file,
            active,
            request.allow_unresolved,
            fault,
        );
    }

    // 5. Inspect source DB via private copy (ZERO mutation of source DB and `-wal`!).
    let (private_dir, private_db_path, private_audit) =
        inspect_source_in_private_copy(&abs_db, &source_info.id)?;

    // 6. Check if retrying an immediately completed operation (default naming or matching stem).
    if let Some(last) = state_file.last_completed.clone()
        && last.mode == req_mode
        && last.archive_dir_physical_id == archive_dir_info.id
        && request
            .archive_name
            .as_deref()
            .is_none_or(|s| s == last.stem)
    {
        let matches_completed = match last.mode {
            RolloverMode::RolloverReset => {
                private_audit.meta_layout == MetaSchemaLayout::V1WithIdentity
                    && Some(private_audit.identity.ledger_epoch) == last.next_ledger_epoch
                    && Some(private_audit.identity.ledger_instance_id.as_str())
                        == last.next_ledger_instance_id.as_deref()
                    && private_audit.identity.last_reset_operation_id.as_deref()
                        == Some(last.operation_id.as_str())
                    && private_audit.head_seq == 0
                    && private_audit.oldest_retained_seq == 1
                    && private_audit.business_tables_empty
            }
            RolloverMode::ArchiveOnly => {
                private_audit.identity.ledger_epoch == last.source_ledger_epoch
                    && private_audit.identity.ledger_instance_id == last.source_ledger_instance_id
                    && private_audit.content_sha256_hex == last.source_content_sha256_hex
            }
        };

        if matches_completed {
            let fin_manifest = final_manifest_path(&abs_archive_dir, &last.stem);
            let verified = verify_sqlite_archive(&fin_manifest)?;
            if verified.manifest.operation_id != last.operation_id
                || verified.manifest.archive_sha256_hex != last.archive_sha256_hex
                || verified.manifest.content_sha256_hex != last.source_content_sha256_hex
            {
                return Err(RolloverError::StateConflict(
                    "completed rollover artifact does not match recorded receipt".to_owned(),
                ));
            }
            return Ok(SqliteRolloverReport {
                operation_id: last.operation_id,
                stem: last.stem,
                mode: last.mode,
                phase: last.phase,
                reused_completed_receipt: true,
                archive_path: verified.archive_path,
                manifest_path: verified.manifest_path,
                manifest: verified.manifest,
            });
        }
    }

    // 7. Block unresolved reset BEFORE creating any state, archive, checkpoint, or DB write!
    let has_unresolved = private_audit.counts.has_unresolved();
    if req_mode == RolloverMode::RolloverReset && has_unresolved && !request.allow_unresolved {
        return Err(RolloverError::UnresolvedItemsBlocked {
            unresolved_committed: private_audit.counts.unresolved_committed,
            open_pending_match: private_audit.counts.open_pending_match,
        });
    }

    // 8. Determine stem and verify zero collisions / orphan files / physical aliases.
    let now_utc: DateTime<Utc> = SystemTime::now().into();
    let stem = match &request.archive_name {
        Some(explicit) => explicit.clone(),
        None => format!(
            "blotter-{}-e{}",
            now_utc.format("%Y%m%dT%H%M%SZ"),
            private_audit.identity.ledger_epoch
        ),
    };
    validate_archive_stem(&stem)?;
    verify_rollover_path_isolation(&abs_db, &lock_path, &state_path, &abs_archive_dir, &stem)?;

    let fin_archive = final_archive_path(&abs_archive_dir, &stem);
    let fin_manifest = final_manifest_path(&abs_archive_dir, &stem);
    let pend_manifest = pending_manifest_path(&abs_archive_dir, &stem);
    if fin_archive.exists() || fin_manifest.exists() || pend_manifest.exists() {
        return Err(RolloverError::ArchiveConflict(format!(
            "target archive or manifest for stem '{stem}' already exists without an active operation"
        )));
    }

    // 9. Decide archive-only source checkpoint branch upfront (`interfaces.md` state ④).
    let source_readonly = fs::metadata(&abs_db)
        .map_err(|e| RolloverError::Io(e.to_string()))?
        .permissions()
        .readonly();
    let archive_only_checkpoint_source = req_mode == RolloverMode::ArchiveOnly
        && private_audit.meta_layout == MetaSchemaLayout::V1WithIdentity
        && !source_readonly
        && !has_unresolved;

    // 10. Generate operation_id and (for reset) next_ledger_instance_id via `getrandom`.
    let operation_id = generate_random_ulid().map_err(|_| {
        RolloverError::Io("entropy source failure generating operation_id".to_owned())
    })?;
    let (next_ledger_epoch, next_ledger_instance_id) = match req_mode {
        RolloverMode::RolloverReset => {
            let next_epoch = private_audit
                .identity
                .ledger_epoch
                .checked_add(1)
                .ok_or_else(|| RolloverError::StateConflict("ledger_epoch overflow".to_owned()))?;
            let next_id = generate_distinct_random_ulid(&private_audit.identity.ledger_instance_id)
                .map_err(|_| {
                    RolloverError::Io(
                        "entropy source failure generating next_ledger_instance_id".to_owned(),
                    )
                })?;
            (Some(next_epoch), Some(next_id))
        }
        RolloverMode::ArchiveOnly => (None, None),
    };

    let active = ActiveRolloverOperation {
        operation_id,
        archived_at_rfc3339: now_utc.to_rfc3339_opts(SecondsFormat::Secs, true),
        source_physical_id: source_info.id,
        source_ledger_epoch: private_audit.identity.ledger_epoch,
        source_ledger_instance_id: private_audit.identity.ledger_instance_id.clone(),
        archive_dir_physical_id: archive_dir_info.id,
        stem,
        mode: req_mode,
        allow_unresolved_requested: request.allow_unresolved,
        allow_unresolved_used: req_mode == RolloverMode::RolloverReset && has_unresolved,
        archive_only_checkpoint_source,
        source_content_sha256_hex: private_audit.content_sha256_hex.clone(),
        next_ledger_epoch,
        next_ledger_instance_id,
        staging_archive_size_bytes: None,
        staging_archive_sha256_hex: None,
        phase: RolloverPhase::Prepared,
    };

    // Persist PREPARED state atomically (Window 1 fault injects half-written temp file here).
    state_file.active = Some(active.clone());
    if fault == RolloverFaultInjection::HalfWrittenStateTempFile {
        write_half_written_temp_file(&state_path, &active.operation_id)?;
        return Err(RolloverError::FaultInjected(fault));
    }
    write_rollover_state_file_atomic(&state_path, &state_file, &active.operation_id)?;

    // Drop the inspection private copy before driving the state machine (PREPARED builds staging).
    drop((private_dir, private_db_path));

    drive_active_operation(
        &abs_db,
        &lock,
        &state_path,
        &abs_archive_dir,
        &mut state_file,
        active,
        request.allow_unresolved,
        fault,
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn drive_active_operation(
    abs_db: &Path,
    lock: &CompanionLock,
    state_path: &Path,
    abs_archive_dir: &Path,
    state_file: &mut RolloverStateFile,
    mut active: ActiveRolloverOperation,
    current_allow_unresolved: bool,
    fault: RolloverFaultInjection,
) -> Result<SqliteRolloverReport, RolloverError> {
    let fin_archive = final_archive_path(abs_archive_dir, &active.stem);
    let fin_manifest = final_manifest_path(abs_archive_dir, &active.stem);
    let pend_manifest = pending_manifest_path(abs_archive_dir, &active.stem);
    let stg_archive = staging_archive_path(abs_archive_dir, &active.stem, &active.operation_id);

    // Phase 1: PREPARED -> build staging archive, record digest, atomically publish formal archive.
    if active.phase == RolloverPhase::Prepared {
        if fin_archive.exists() {
            // Crash may have occurred right after hard_link(staging, final) before unlinking staging
            // or before updating state_file to ARCHIVE_PUBLISHED.
            verify_existing_entry_no_reparse(&fin_archive)?;
            if stg_archive.exists() {
                verify_existing_entry_no_reparse(&stg_archive)?;
                let stg_id = inspect_physical_file_identity(&stg_archive, false)?;
                let fin_id = inspect_physical_file_identity(&fin_archive, false)?;
                if stg_id.id == fin_id.id {
                    fs::remove_file(&stg_archive).map_err(|e| RolloverError::Io(e.to_string()))?;
                }
            }
            let fin_info = inspect_physical_file_identity(&fin_archive, false)?;
            if fin_info.nlink != 1
                || fin_info.id == active.source_physical_id
                || fin_info.id == lock.lock_identity().id
            {
                return Err(RolloverError::ArchiveConflict(
                    "existing formal archive fails physical isolation or nlink==1 check".to_owned(),
                ));
            }
            let (Some(exp_size), Some(exp_sha)) = (
                active.staging_archive_size_bytes,
                active.staging_archive_sha256_hex.as_deref(),
            ) else {
                return Err(RolloverError::ArchiveConflict(
                    "orphan formal archive exists before staging digest was recorded".to_owned(),
                ));
            };
            let (actual_size, actual_sha) = compute_file_size_and_sha256(&fin_archive)?;
            if actual_size != exp_size || actual_sha != exp_sha {
                return Err(RolloverError::ArchiveConflict(
                    "existing formal archive size/digest mismatches prepared operation".to_owned(),
                ));
            }
            let archive_audit = audit_readonly_archive_file(&fin_archive)?;
            verify_archive_audit_matches_active(&archive_audit, &active)?;
            active.phase = RolloverPhase::ArchivePublished;
            state_file.active = Some(active.clone());
            write_rollover_state_file_atomic(state_path, state_file, &active.operation_id)?;
        } else {
            // If archive-only selected live source checkpoint branch, execute checkpoint & verify
            // 0-byte WAL BEFORE building the staging archive (never downgrade on failure).
            if active.archive_only_checkpoint_source {
                checkpoint_and_verify_empty_wal(abs_db, false)?;
            }

            // Build self-contained staging archive from a private copy (0-byte source mutation for
            // legacy v1 / readonly / unresolved sources).
            let (stg_size, stg_sha) = build_staging_archive(abs_db, &stg_archive, &active)?;

            active.staging_archive_size_bytes = Some(stg_size);
            active.staging_archive_sha256_hex = Some(stg_sha);
            state_file.active = Some(active.clone());
            write_rollover_state_file_atomic(state_path, state_file, &active.operation_id)?;

            // Atomic non-overwriting publish via hard_link, then remove staging link -> nlink == 1.
            publish_file_no_overwrite(&stg_archive, &fin_archive)?;
            let fin_info = inspect_physical_file_identity(&fin_archive, false)?;
            if fin_info.nlink != 1
                || fin_info.id == active.source_physical_id
                || fin_info.id == lock.lock_identity().id
            {
                return Err(RolloverError::ArchiveConflict(
                    "published formal archive failed post-publish isolation check".to_owned(),
                ));
            }

            active.phase = RolloverPhase::ArchivePublished;
            state_file.active = Some(active.clone());
            write_rollover_state_file_atomic(state_path, state_file, &active.operation_id)?;
        }

        if fault == RolloverFaultInjection::AfterArchivePublished {
            return Err(RolloverError::FaultInjected(fault));
        }
    }

    // Check if final manifest was ALREADY published before a crash (Window 6 recovery).
    if fin_manifest.exists() {
        return finalize_already_published_manifest(
            abs_db,
            state_path,
            state_file,
            &active,
            &fin_manifest,
            &pend_manifest,
        );
    }

    // Phase 2: ARCHIVE_PUBLISHED -> verify formal archive, publish pending manifest, and either
    // finish archive-only or execute the single reset delete transaction.
    if active.phase == RolloverPhase::ArchivePublished {
        let (archive_size, archive_sha) = compute_file_size_and_sha256(&fin_archive)?;
        if active.staging_archive_size_bytes != Some(archive_size)
            || active.staging_archive_sha256_hex.as_deref() != Some(archive_sha.as_str())
        {
            return Err(RolloverError::AuditFailed(
                "published archive size/sha256 does not match active operation".to_owned(),
            ));
        }
        let archive_audit = audit_readonly_archive_file(&fin_archive)?;
        verify_archive_audit_matches_active(&archive_audit, &active)?;

        // Write intermediate pending manifest at separate `<stem>.manifest.pending.json` (`F-22(2)`).
        // If a pending manifest was already published before a crash, verify it matches `active`
        // rather than blindly overwriting it (catches tampered `state.json` next_ledger_instance_id!).
        if pend_manifest.exists() {
            verify_existing_entry_no_reparse(&pend_manifest)?;
            let existing_raw =
                fs::read_to_string(&pend_manifest).map_err(|e| RolloverError::Io(e.to_string()))?;
            let existing_pending: ArchiveManifest =
                serde_json::from_str(&existing_raw).map_err(|e| {
                    RolloverError::AuditFailed(format!("corrupt pending manifest: {e}"))
                })?;
            validate_archive_manifest(&existing_pending, &active.stem, false)?;
            if existing_pending.operation_id != active.operation_id
                || existing_pending.mode != active.mode
                || existing_pending.source_ledger_epoch != active.source_ledger_epoch
                || existing_pending.source_ledger_instance_id != active.source_ledger_instance_id
                || existing_pending.next_ledger_epoch != active.next_ledger_epoch
                || existing_pending.next_ledger_instance_id != active.next_ledger_instance_id
                || existing_pending.archive_size_bytes != archive_size
                || existing_pending.archive_sha256_hex != archive_sha
                || existing_pending.content_sha256_hex != active.source_content_sha256_hex
            {
                return Err(RolloverError::StateConflict(
                    "existing pending manifest disagrees with active rollover state".to_owned(),
                ));
            }
        } else {
            let pending_manifest = build_manifest(
                &active,
                &archive_audit,
                archive_size,
                archive_sha.clone(),
                ManifestStatus::Pending,
                false,
                false,
            );
            validate_archive_manifest(&pending_manifest, &active.stem, false)?;
            write_json_file_atomic(&pend_manifest, &pending_manifest, &active.operation_id)?;
        }

        if fault == RolloverFaultInjection::AfterPendingManifestPublished {
            return Err(RolloverError::FaultInjected(fault));
        }

        match active.mode {
            RolloverMode::ArchiveOnly => {
                if active.archive_only_checkpoint_source {
                    // Re-verify post-close WAL is absent or 0 bytes before claiming wal_truncated=true.
                    verify_wal_absent_or_empty(abs_db)?;
                }
                let final_manifest = build_manifest(
                    &active,
                    &archive_audit,
                    archive_size,
                    archive_sha.clone(),
                    ManifestStatus::ArchiveOnly,
                    false,
                    active.archive_only_checkpoint_source,
                );
                validate_archive_manifest(&final_manifest, &active.stem, true)?;
                publish_json_no_overwrite(&fin_manifest, &final_manifest, &active.operation_id)?;

                if fault == RolloverFaultInjection::AfterFinalManifestPublishedBeforeStateRetire {
                    return Err(RolloverError::FaultInjected(fault));
                }

                let _ = fs::remove_file(&pend_manifest);
                retire_completed_operation(
                    state_path,
                    state_file,
                    &active,
                    RolloverPhase::CompletedArchiveOnly,
                    &archive_sha,
                )?;

                return Ok(SqliteRolloverReport {
                    operation_id: active.operation_id,
                    stem: active.stem,
                    mode: active.mode,
                    phase: RolloverPhase::CompletedArchiveOnly,
                    reused_completed_receipt: false,
                    archive_path: fin_archive,
                    manifest_path: fin_manifest,
                    manifest: final_manifest,
                });
            }
            RolloverMode::RolloverReset => {
                // Single delete transaction entrypoint (`F-17`).
                execute_reset_delete_transaction(
                    abs_db,
                    &fin_archive,
                    &pend_manifest,
                    &active,
                    current_allow_unresolved,
                )?;

                if fault == RolloverFaultInjection::AfterSourceResetCommittedBeforeStateUpdate {
                    return Err(RolloverError::FaultInjected(fault));
                }

                // Update pending manifest to State ② (source_reset_committed=true, wal_truncated=false).
                let committed_pending = build_manifest(
                    &active,
                    &archive_audit,
                    archive_size,
                    archive_sha,
                    ManifestStatus::Pending,
                    true,
                    false,
                );
                validate_archive_manifest(&committed_pending, &active.stem, false)?;
                write_json_file_atomic(&pend_manifest, &committed_pending, &active.operation_id)?;

                active.phase = RolloverPhase::ResetCommittedMaintenancePending;
                state_file.active = Some(active.clone());
                write_rollover_state_file_atomic(state_path, state_file, &active.operation_id)?;
            }
        }
    }

    // Phase 3: RESET_COMMITTED_MAINTENANCE_PENDING -> verify reset source, VACUUM, final
    // wal_checkpoint(TRUNCATE), close & verify empty WAL, then publish COMPLETED_RESET.
    if active.phase == RolloverPhase::ResetCommittedMaintenancePending {
        if active.mode != RolloverMode::RolloverReset {
            return Err(RolloverError::StateConflict(
                "non-reset operation in RESET_COMMITTED_MAINTENANCE_PENDING".to_owned(),
            ));
        }

        // Re-verify published formal archive before maintenance & final manifest.
        let (archive_size, archive_sha) = compute_file_size_and_sha256(&fin_archive)?;
        if active.staging_archive_size_bytes != Some(archive_size)
            || active.staging_archive_sha256_hex.as_deref() != Some(archive_sha.as_str())
        {
            return Err(RolloverError::AuditFailed(
                "published archive mismatch during maintenance phase".to_owned(),
            ));
        }
        let archive_audit = audit_readonly_archive_file(&fin_archive)?;
        verify_archive_audit_matches_active(&archive_audit, &active)?;

        // Verify source DB genuinely carries (E+1, N, operation_id) and clean empty book.
        verify_source_is_clean_reset_target(abs_db, &active)?;

        if fault == RolloverFaultInjection::DuringMaintenanceVacuumOrCheckpoint {
            return Err(RolloverError::FaultInjected(fault));
        }

        // Run VACUUM + PRAGMA wal_checkpoint(TRUNCATE) + close + 0-byte WAL verification.
        checkpoint_and_verify_empty_wal(abs_db, true)?;

        // Publish final manifest (State ⑤: completed_reset, wal_truncated=true) atomically.
        let final_manifest = build_manifest(
            &active,
            &archive_audit,
            archive_size,
            archive_sha.clone(),
            ManifestStatus::CompletedReset,
            true,
            true,
        );
        validate_archive_manifest(&final_manifest, &active.stem, true)?;
        publish_json_no_overwrite(&fin_manifest, &final_manifest, &active.operation_id)?;

        if fault == RolloverFaultInjection::AfterFinalManifestPublishedBeforeStateRetire {
            return Err(RolloverError::FaultInjected(fault));
        }

        let _ = fs::remove_file(&pend_manifest);
        retire_completed_operation(
            state_path,
            state_file,
            &active,
            RolloverPhase::CompletedReset,
            &archive_sha,
        )?;

        return Ok(SqliteRolloverReport {
            operation_id: active.operation_id,
            stem: active.stem,
            mode: active.mode,
            phase: RolloverPhase::CompletedReset,
            reused_completed_receipt: false,
            archive_path: fin_archive,
            manifest_path: fin_manifest,
            manifest: final_manifest,
        });
    }

    Err(RolloverError::StateConflict(format!(
        "unexpected active rollover phase {:?}",
        active.phase
    )))
}

/// Single delete transaction entrypoint (`design.md` §1 `F-17`).
///
/// Re-verifies mode, `--allow-unresolved` vs live `COMMITTED`/`OPEN` counts, `(E, I)`, `(E+1, N)`,
/// `operation_id`, pending manifest, and full 17-table equivalence with the published read-only
/// archive inside `BEGIN IMMEDIATE` before deleting a single row.
#[allow(clippy::too_many_lines)]
fn execute_reset_delete_transaction(
    abs_db: &Path,
    fin_archive: &Path,
    pend_manifest_path: &Path,
    active: &ActiveRolloverOperation,
    current_allow_unresolved: bool,
) -> Result<(), RolloverError> {
    // 1. Re-verify mode is RolloverReset and next identity pair is valid.
    if active.mode != RolloverMode::RolloverReset {
        return Err(RolloverError::ModeConflict(
            "delete transaction refused for non-reset mode".to_owned(),
        ));
    }
    let (Some(next_epoch), Some(next_instance_id)) = (
        active.next_ledger_epoch,
        active.next_ledger_instance_id.as_deref(),
    ) else {
        return Err(RolloverError::StateConflict(
            "missing scheduled next ledger identity in active operation".to_owned(),
        ));
    };
    if active.source_ledger_epoch.checked_add(1) != Some(next_epoch)
        || !is_canonical_ulid(next_instance_id)
        || next_instance_id == active.source_ledger_instance_id
    {
        return Err(RolloverError::StateConflict(
            "invalid scheduled next ledger identity pair".to_owned(),
        ));
    }

    // 2. Re-verify pending manifest exists and matches active operation.
    verify_existing_entry_no_reparse(pend_manifest_path)?;
    let pending_bytes =
        fs::read_to_string(pend_manifest_path).map_err(|e| RolloverError::Io(e.to_string()))?;
    let pending_manifest: ArchiveManifest = serde_json::from_str(&pending_bytes)
        .map_err(|e| RolloverError::AuditFailed(format!("invalid pending manifest JSON: {e}")))?;
    validate_archive_manifest(&pending_manifest, &active.stem, false)?;
    if pending_manifest.operation_id != active.operation_id
        || pending_manifest.source_ledger_epoch != active.source_ledger_epoch
        || pending_manifest.source_ledger_instance_id != active.source_ledger_instance_id
        || pending_manifest.next_ledger_epoch != Some(next_epoch)
        || pending_manifest.next_ledger_instance_id.as_deref() != Some(next_instance_id)
        || pending_manifest.content_sha256_hex != active.source_content_sha256_hex
    {
        return Err(RolloverError::StateConflict(
            "pending manifest disagrees with active rollover operation".to_owned(),
        ));
    }

    // 3. Re-verify published formal archive (read-only) and audit all 17 tables.
    let (archive_size, archive_sha) = compute_file_size_and_sha256(fin_archive)?;
    if pending_manifest.archive_size_bytes != archive_size
        || pending_manifest.archive_sha256_hex != archive_sha
    {
        return Err(RolloverError::AuditFailed(
            "published archive bytes disagree with pending manifest".to_owned(),
        ));
    }
    let archive_audit = audit_readonly_archive_file(fin_archive)?;
    verify_archive_audit_matches_active(&archive_audit, active)?;

    // 4. Verify live source physical identity before opening maintenance connection.
    let live_info = inspect_physical_file_identity(abs_db, false)?;
    if live_info.nlink != 1 || live_info.id != active.source_physical_id {
        return Err(RolloverError::StateConflict(
            "source database physical identity changed before delete transaction".to_owned(),
        ));
    }

    // 5. Open maintenance connection (NORMAL, wal, FK=ON, FULL, busy_timeout=0) and BEGIN IMMEDIATE.
    let conn = open_maintenance_connection(abs_db)?;
    conn.execute_batch("BEGIN IMMEDIATE;")
        .map_err(map_sqlite_lock_err)?;

    let tx_res = (|| -> Result<bool, RolloverError> {
        let layout = inspect_meta_layout(&conn).map_err(|_| {
            RolloverError::AuditFailed("invalid source blotter_meta layout".to_owned())
        })?;

        // Window 4 recovery: detect if THIS exact operation already committed the reset tx!
        if layout == MetaSchemaLayout::V1WithIdentity
            && let Ok(persisted) = read_persisted_ledger_identity(&conn)
            && persisted.ledger_epoch == next_epoch
            && persisted.ledger_instance_id == next_instance_id
            && persisted.last_reset_operation_id.as_deref() == Some(active.operation_id.as_str())
        {
            let post_audit = audit_sqlite_database(&conn, None)?;
            if post_audit.head_seq == 0
                && post_audit.oldest_retained_seq == 1
                && post_audit.business_tables_empty
            {
                return Ok(true); // Already committed cleanly; repair external phase only.
            }
            return Err(RolloverError::StateConflict(
                "source carries next identity/operation_id but business tables are not clean"
                    .to_owned(),
            ));
        }

        // Compute live 17-table audit (with legacy virtual identity if source is still v1 legacy).
        let virtual_id = match layout {
            MetaSchemaLayout::V1Legacy => Some(derive_legacy_v1_identity_on_conn(
                &conn,
                &active.source_physical_id,
            )?),
            MetaSchemaLayout::V1WithIdentity => None,
        };
        let live_audit = audit_sqlite_database(&conn, virtual_id.as_ref())?;

        // Verify source logical identity matches (E, I).
        if live_audit.identity.ledger_epoch != active.source_ledger_epoch
            || live_audit.identity.ledger_instance_id != active.source_ledger_instance_id
        {
            return Err(RolloverError::StateConflict(format!(
                "source ledger identity mismatch: live=({}, {}), expected=({}, {})",
                live_audit.identity.ledger_epoch,
                live_audit.identity.ledger_instance_id,
                active.source_ledger_epoch,
                active.source_ledger_instance_id,
            )));
        }

        // Verify live 17-table content, counts, accounts, and seqs match published archive 100%.
        // This catches ANY mutation after archival, including a new idempotency key at same head_seq!
        if live_audit.content_sha256_hex != archive_audit.content_sha256_hex
            || live_audit.content_sha256_hex != active.source_content_sha256_hex
            || live_audit.counts != archive_audit.counts
            || live_audit.accounts != archive_audit.accounts
            || live_audit.head_seq != archive_audit.head_seq
            || live_audit.oldest_retained_seq != archive_audit.oldest_retained_seq
        {
            return Err(RolloverError::StateConflict(
                "live source database diverged from published archive; aborting with zero deletions"
                    .to_owned(),
            ));
        }

        // Re-verify unresolved counts against BOTH current explicit flag and original intent.
        if live_audit.counts.has_unresolved()
            && (!current_allow_unresolved || !active.allow_unresolved_requested)
        {
            return Err(RolloverError::UnresolvedItemsBlocked {
                unresolved_committed: live_audit.counts.unresolved_committed,
                open_pending_match: live_audit.counts.open_pending_match,
            });
        }

        // Migrate legacy v1 source columns inside this exact reset transaction if needed.
        let next_identity = LedgerIdentity {
            ledger_epoch: next_epoch,
            ledger_instance_id: next_instance_id.to_owned(),
            last_reset_operation_id: Some(active.operation_id.clone()),
        };
        if layout == MetaSchemaLayout::V1Legacy {
            install_meta_identity_columns(&conn, &next_identity).map_err(|_| {
                RolloverError::Sqlite("failed to install v1 identity cols".to_owned())
            })?;
        }

        // Delete all 16 business tables in strict reverse foreign-key dependency order.
        let reverse_fk_delete_tables: &[&str] = &[
            "blotter_event_accounts",
            "blotter_event_journal",
            "blotter_idempotency_keys",
            "blotter_quote_heads",
            "blotter_quote_history",
            "blotter_reconcile_allocations",
            "blotter_pending_matches",
            "blotter_draft_earmarks",
            "blotter_earmarks",
            "blotter_trade_effective_revisions",
            "blotter_ingested_trades",
            "blotter_feed_raw_packets",
            "blotter_feed_status",
            "blotter_positions",
            "blotter_instruments",
            "blotter_accounts",
        ];
        for &tbl in reverse_fk_delete_tables {
            conn.execute(&format!("DELETE FROM {tbl}"), [])
                .map_err(|e| RolloverError::Sqlite(format!("DELETE FROM {tbl} failed: {e}")))?;
        }

        let next_epoch_i64 = i64::try_from(next_epoch)
            .map_err(|_| RolloverError::StateConflict("next_epoch i64 overflow".to_owned()))?;
        let updated = conn
            .execute(
                "UPDATE blotter_meta
                 SET head_seq = 0,
                     oldest_retained_seq = 1,
                     ledger_epoch = ?1,
                     ledger_instance_id = ?2,
                     last_reset_operation_id = ?3
                 WHERE singleton_id = 1",
                params![next_epoch_i64, next_instance_id, active.operation_id],
            )
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        if updated != 1 {
            return Err(RolloverError::Sqlite(
                "failed to update blotter_meta during reset".to_owned(),
            ));
        }

        // Verify zero FK violations inside the transaction before COMMIT.
        let mut fk_stmt = conn
            .prepare("PRAGMA foreign_key_check")
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        let mut fk_rows = fk_stmt
            .query([])
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        if fk_rows
            .next()
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?
            .is_some()
        {
            return Err(RolloverError::AuditFailed(
                "foreign_key_check failed after reset delete".to_owned(),
            ));
        }

        Ok(false)
    })();

    match tx_res {
        Ok(already_committed) => {
            if already_committed {
                let _ = conn.execute_batch("ROLLBACK;");
            } else {
                conn.execute_batch("COMMIT;")
                    .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
            }
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(err)
        }
    }
}

/// Independent archive & manifest verifier (`--verify-sqlite-archive --manifest <FILE>`).
pub fn verify_sqlite_archive(
    manifest_path: &Path,
) -> Result<VerifySqliteArchiveReport, RolloverError> {
    let abs_manifest = normalize_and_verify_no_reparse(manifest_path)?;
    if !abs_manifest.exists() {
        return Err(RolloverError::Validation(format!(
            "manifest file does not exist: '{}'",
            abs_manifest.display()
        )));
    }
    let manifest_info = inspect_physical_file_identity(&abs_manifest, false)?;

    let file_name = abs_manifest
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| RolloverError::Validation("invalid manifest filename".to_owned()))?;
    let Some(expected_stem) = file_name.strip_suffix(".manifest.json") else {
        return Err(RolloverError::Validation(format!(
            "manifest filename '{file_name}' must end with '.manifest.json'"
        )));
    };
    validate_archive_stem(expected_stem)?;

    let raw_json =
        fs::read_to_string(&abs_manifest).map_err(|e| RolloverError::Io(e.to_string()))?;
    let manifest: ArchiveManifest = serde_json::from_str(&raw_json)
        .map_err(|e| RolloverError::AuditFailed(format!("invalid manifest JSON: {e}")))?;

    // Final manifest only allows states ④ (`archive_only`) and ⑤ (`completed_reset`).
    validate_archive_manifest(&manifest, expected_stem, true)?;

    let archive_dir = abs_manifest.parent().ok_or_else(|| {
        RolloverError::Validation("manifest path has no parent directory".to_owned())
    })?;
    let archive_path = archive_dir.join(&manifest.archive_filename);
    verify_existing_entry_no_reparse(&archive_path)?;
    if !archive_path.exists() {
        return Err(RolloverError::AuditFailed(format!(
            "archive file '{}' referenced by manifest does not exist",
            archive_path.display()
        )));
    }
    let archive_info = inspect_physical_file_identity(&archive_path, false)?;
    if archive_info.id == manifest_info.id {
        return Err(RolloverError::AuditFailed(
            "manifest and archive resolve to the same physical file".to_owned(),
        ));
    }

    let (actual_size, actual_sha) = compute_file_size_and_sha256(&archive_path)?;
    if actual_size != manifest.archive_size_bytes {
        return Err(RolloverError::AuditFailed(format!(
            "archive size mismatch: file={actual_size}, manifest={}",
            manifest.archive_size_bytes
        )));
    }
    if actual_sha != manifest.archive_sha256_hex {
        return Err(RolloverError::AuditFailed(format!(
            "archive_sha256_hex mismatch: file={actual_sha}, manifest={}",
            manifest.archive_sha256_hex
        )));
    }

    // Open archive read-only (immutable standalone single-file SQLite DB) and run full 17-table audit.
    let audit = audit_readonly_archive_file(&archive_path)?;
    if audit.meta_layout != MetaSchemaLayout::V1WithIdentity {
        return Err(RolloverError::AuditFailed(
            "archive must carry materialized ledger identity columns".to_owned(),
        ));
    }
    if audit.identity.ledger_epoch != manifest.source_ledger_epoch
        || audit.identity.ledger_instance_id != manifest.source_ledger_instance_id
    {
        return Err(RolloverError::AuditFailed(
            "archive ledger identity mismatches manifest".to_owned(),
        ));
    }
    if audit.content_sha256_hex != manifest.content_sha256_hex {
        return Err(RolloverError::AuditFailed(format!(
            "content_sha256_hex mismatch: archive={}, manifest={}",
            audit.content_sha256_hex, manifest.content_sha256_hex
        )));
    }
    if audit.head_seq != manifest.head_seq
        || audit.oldest_retained_seq != manifest.oldest_retained_seq
        || audit.accounts != manifest.accounts
        || audit.counts != manifest.counts
    {
        return Err(RolloverError::AuditFailed(
            "archive audit metadata/accounts/counts disagree with manifest".to_owned(),
        ));
    }

    Ok(VerifySqliteArchiveReport {
        manifest_path: abs_manifest,
        archive_path,
        manifest,
    })
}

// ============================================================================
// Private Rollover Helpers
// ============================================================================

fn verify_rollover_path_isolation(
    abs_db: &Path,
    lock_path: &Path,
    state_path: &Path,
    abs_archive_dir: &Path,
    stem: &str,
) -> Result<(), RolloverError> {
    let fin_archive = final_archive_path(abs_archive_dir, stem);
    let fin_manifest = final_manifest_path(abs_archive_dir, stem);
    let pend_manifest = pending_manifest_path(abs_archive_dir, stem);

    let mut roles = vec![
        abs_db.to_path_buf(),
        lock_path.to_path_buf(),
        state_path.to_path_buf(),
        fin_archive.clone(),
        fin_manifest,
        pend_manifest,
    ];
    roles.extend(sqlite_companions(abs_db));
    roles.extend(sqlite_companions(lock_path));
    roles.extend(sqlite_companions(&fin_archive));

    for p in &roles {
        verify_existing_entry_no_reparse(p)?;
    }
    assert_distinct_file_roles(&roles)?;
    assert_pairwise_physical_distinct(&roles)?;
    Ok(())
}

/// Copies the main DB file and any `-wal`/`-journal` companion into a private temp directory so we
/// can recover and audit a WAL database with **0 bytes** mutated on the real source DB or `-wal`.
fn inspect_source_in_private_copy(
    abs_db: &Path,
    source_physical_id: &PhysicalFileId,
) -> Result<(PrivateTempDir, PathBuf, DatabaseAuditSummary), RolloverError> {
    let temp = PrivateTempDir::new(abs_db.parent())?;
    let copy_db = temp.path().join("inspect.sqlite");
    copy_sqlite_with_companions(abs_db, &copy_db)?;

    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(&copy_db, flags)
        .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
    conn.execute_batch("PRAGMA busy_timeout = 0; PRAGMA foreign_keys = ON;")
        .map_err(|e| RolloverError::Sqlite(e.to_string()))?;

    let layout = inspect_meta_layout(&conn)
        .map_err(|_| RolloverError::AuditFailed("invalid blotter_meta layout".to_owned()))?;
    let virtual_id = match layout {
        MetaSchemaLayout::V1Legacy => Some(derive_legacy_v1_identity_on_conn(
            &conn,
            source_physical_id,
        )?),
        MetaSchemaLayout::V1WithIdentity => None,
    };
    let audit = audit_sqlite_database(&conn, virtual_id.as_ref())?;
    drop(conn);
    Ok((temp, copy_db, audit))
}

fn copy_sqlite_with_companions(src_db: &Path, dst_db: &Path) -> Result<(), RolloverError> {
    fs::copy(src_db, dst_db).map_err(|e| RolloverError::Io(format!("copy main DB failed: {e}")))?;
    // Ensure copied file is writable in our private temp directory even if source is read-only.
    let mut perms = fs::metadata(dst_db)
        .map_err(|e| RolloverError::Io(e.to_string()))?
        .permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    if perms.readonly() {
        perms.set_readonly(false);
        fs::set_permissions(dst_db, perms).map_err(|e| RolloverError::Io(e.to_string()))?;
    }

    for suffix in ["-wal", "-journal"] {
        let src_comp = append_suffix(src_db, suffix);
        if src_comp.exists() {
            verify_existing_entry_no_reparse(&src_comp)?;
            let dst_comp = append_suffix(dst_db, suffix);
            fs::copy(&src_comp, &dst_comp)
                .map_err(|e| RolloverError::Io(format!("copy {suffix} failed: {e}")))?;
            let mut cperms = fs::metadata(&dst_comp)
                .map_err(|e| RolloverError::Io(e.to_string()))?
                .permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            if cperms.readonly() {
                cperms.set_readonly(false);
                fs::set_permissions(&dst_comp, cperms)
                    .map_err(|e| RolloverError::Io(e.to_string()))?;
            }
        }
    }
    Ok(())
}

/// Builds a clean, self-contained single-file staging archive at `stg_archive` (materializing
/// legacy v1 identity columns inside the private archive copy without mutating the source).
fn build_staging_archive(
    abs_db: &Path,
    stg_archive: &Path,
    active: &ActiveRolloverOperation,
) -> Result<(u64, String), RolloverError> {
    // Clean any leftover staging file from an interrupted attempt of the same operation_id.
    for path in std::iter::once(stg_archive.to_path_buf()).chain(sqlite_companions(stg_archive)) {
        if path.exists() {
            verify_existing_entry_no_reparse(&path)?;
            fs::remove_file(&path).map_err(|e| RolloverError::Io(e.to_string()))?;
        }
    }

    let temp = PrivateTempDir::new(stg_archive.parent())?;
    let work_db = temp.path().join("work.sqlite");
    copy_sqlite_with_companions(abs_db, &work_db)?;

    {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&work_db, flags)
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        conn.execute_batch(
            "PRAGMA busy_timeout = 0;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = FULL;
             PRAGMA locking_mode = NORMAL;",
        )
        .map_err(|e| RolloverError::Sqlite(e.to_string()))?;

        // Materialize legacy v1 identity columns inside the private archive copy only.
        let layout = inspect_meta_layout(&conn)
            .map_err(|_| RolloverError::AuditFailed("invalid work DB meta layout".to_owned()))?;
        if layout == MetaSchemaLayout::V1Legacy {
            let derived = derive_legacy_v1_identity_on_conn(&conn, &active.source_physical_id)?;
            if derived.ledger_epoch != active.source_ledger_epoch
                || derived.ledger_instance_id != active.source_ledger_instance_id
            {
                return Err(RolloverError::StateConflict(
                    "legacy v1 identity changed while building staging archive".to_owned(),
                ));
            }
            conn.execute_batch("BEGIN IMMEDIATE;")
                .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
            let mig = install_meta_identity_columns(&conn, &derived);
            if mig.is_err() {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(RolloverError::Sqlite(
                    "failed to materialize identity columns in archive copy".to_owned(),
                ));
            }
            conn.execute_batch("COMMIT;")
                .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        }

        // Switch archive copy to DELETE journal mode and VACUUM into a clean single-file DB.
        let jm: String = conn
            .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
        if !jm.eq_ignore_ascii_case("delete") {
            return Err(RolloverError::Sqlite(format!(
                "archive staging journal_mode={jm}, expected delete"
            )));
        }
        conn.execute_batch("VACUUM;")
            .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
    }

    // Verify work_db has no stray -wal or -journal and copy into stg_archive with fsync.
    copy_file_and_sync(&work_db, stg_archive)?;
    let archive_audit = audit_readonly_archive_file(stg_archive)?;
    verify_archive_audit_matches_active(&archive_audit, active)?;
    compute_file_size_and_sha256(stg_archive)
}

fn open_maintenance_connection(abs_db: &Path) -> Result<Connection, RolloverError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(abs_db, flags).map_err(map_sqlite_lock_err)?;
    conn.execute_batch(
        "PRAGMA busy_timeout = 0;
         PRAGMA foreign_keys = ON;
         PRAGMA synchronous = FULL;",
    )
    .map_err(map_sqlite_lock_err)?;
    let locking: String = conn
        .query_row("PRAGMA locking_mode = NORMAL", [], |r| r.get(0))
        .map_err(map_sqlite_lock_err)?;
    let journal: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
        .map_err(map_sqlite_lock_err)?;

    let bt: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .map_err(map_sqlite_lock_err)?;
    let fk: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .map_err(map_sqlite_lock_err)?;
    let sync_mode: i64 = conn
        .query_row("PRAGMA synchronous", [], |r| r.get(0))
        .map_err(map_sqlite_lock_err)?;
    let ver_locking: String = conn
        .query_row("PRAGMA locking_mode", [], |r| r.get(0))
        .map_err(map_sqlite_lock_err)?;
    let ver_journal: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .map_err(map_sqlite_lock_err)?;

    if bt != 0
        || fk != 1
        || sync_mode != 2
        || !locking.eq_ignore_ascii_case("normal")
        || !ver_locking.eq_ignore_ascii_case("normal")
        || !journal.eq_ignore_ascii_case("wal")
        || !ver_journal.eq_ignore_ascii_case("wal")
    {
        return Err(RolloverError::Sqlite(format!(
            "maintenance PRAGMA readback failed (bt={bt}, fk={fk}, sync={sync_mode}, locking={ver_locking}, journal={ver_journal})"
        )));
    }
    Ok(conn)
}

fn checkpoint_and_verify_empty_wal(abs_db: &Path, run_vacuum: bool) -> Result<(), RolloverError> {
    {
        let conn = open_maintenance_connection(abs_db)?;
        if run_vacuum {
            conn.execute_batch("VACUUM;").map_err(map_sqlite_lock_err)?;
        }
        let (busy, log, checkpointed): (i64, i64, i64) = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(map_sqlite_lock_err)?;
        if busy != 0 || log < 0 || checkpointed != log {
            return Err(RolloverError::Sqlite(format!(
                "wal_checkpoint(TRUNCATE) incomplete: busy={busy}, log={log}, checkpointed={checkpointed}"
            )));
        }
        // Close connection explicitly before post-close WAL length check.
        conn.close()
            .map_err(|(_, e)| RolloverError::Sqlite(e.to_string()))?;
    }
    verify_wal_absent_or_empty(abs_db)
}

fn verify_wal_absent_or_empty(abs_db: &Path) -> Result<(), RolloverError> {
    let wal_path = append_suffix(abs_db, "-wal");
    verify_existing_entry_no_reparse(&wal_path)?;
    if wal_path.exists() {
        let len = fs::metadata(&wal_path)
            .map_err(|e| RolloverError::Io(e.to_string()))?
            .len();
        if len != 0 {
            return Err(RolloverError::AuditFailed(format!(
                "post-close WAL file is non-empty ({len} bytes)"
            )));
        }
    }
    Ok(())
}

fn verify_source_is_clean_reset_target(
    abs_db: &Path,
    active: &ActiveRolloverOperation,
) -> Result<(), RolloverError> {
    let (Some(next_epoch), Some(next_id)) = (
        active.next_ledger_epoch,
        active.next_ledger_instance_id.as_deref(),
    ) else {
        return Err(RolloverError::StateConflict(
            "missing next identity during maintenance verification".to_owned(),
        ));
    };
    let conn = open_maintenance_connection(abs_db)?;
    let audit = audit_sqlite_database(&conn, None)?;
    if audit.meta_layout != MetaSchemaLayout::V1WithIdentity
        || audit.identity.ledger_epoch != next_epoch
        || audit.identity.ledger_instance_id != next_id
        || audit.identity.last_reset_operation_id.as_deref() != Some(active.operation_id.as_str())
        || audit.head_seq != 0
        || audit.oldest_retained_seq != 1
        || !audit.business_tables_empty
    {
        return Err(RolloverError::StateConflict(
            "source database does not match expected committed reset state".to_owned(),
        ));
    }
    Ok(())
}

fn audit_readonly_archive_file(archive_path: &Path) -> Result<DatabaseAuditSummary, RolloverError> {
    verify_existing_entry_no_reparse(archive_path)?;
    for comp in sqlite_companions(archive_path) {
        if comp.exists() {
            return Err(RolloverError::AuditFailed(format!(
                "standalone archive must not have companion file '{}'",
                comp.display()
            )));
        }
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(archive_path, flags)
        .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
    conn.execute_batch("PRAGMA busy_timeout = 0; PRAGMA query_only = ON;")
        .map_err(|e| RolloverError::Sqlite(e.to_string()))?;
    audit_sqlite_database(&conn, None)
}

fn verify_archive_audit_matches_active(
    archive_audit: &DatabaseAuditSummary,
    active: &ActiveRolloverOperation,
) -> Result<(), RolloverError> {
    if archive_audit.meta_layout != MetaSchemaLayout::V1WithIdentity {
        return Err(RolloverError::AuditFailed(
            "archive missing materialized identity columns".to_owned(),
        ));
    }
    if archive_audit.identity.ledger_epoch != active.source_ledger_epoch
        || archive_audit.identity.ledger_instance_id != active.source_ledger_instance_id
    {
        return Err(RolloverError::AuditFailed(
            "archive ledger identity mismatches active operation".to_owned(),
        ));
    }
    if archive_audit.content_sha256_hex != active.source_content_sha256_hex {
        return Err(RolloverError::AuditFailed(
            "archive content_sha256_hex mismatches active operation".to_owned(),
        ));
    }
    Ok(())
}

fn finalize_already_published_manifest(
    abs_db: &Path,
    state_path: &Path,
    state_file: &mut RolloverStateFile,
    active: &ActiveRolloverOperation,
    fin_manifest: &Path,
    pend_manifest: &Path,
) -> Result<SqliteRolloverReport, RolloverError> {
    let verified = verify_sqlite_archive(fin_manifest)?;
    let expected_status = match active.mode {
        RolloverMode::ArchiveOnly => ManifestStatus::ArchiveOnly,
        RolloverMode::RolloverReset => ManifestStatus::CompletedReset,
    };
    if verified.manifest.operation_id != active.operation_id
        || verified.manifest.mode != active.mode
        || verified.manifest.status != expected_status
        || verified.manifest.source_ledger_epoch != active.source_ledger_epoch
        || verified.manifest.source_ledger_instance_id != active.source_ledger_instance_id
        || verified.manifest.next_ledger_epoch != active.next_ledger_epoch
        || verified.manifest.next_ledger_instance_id != active.next_ledger_instance_id
        || verified.manifest.content_sha256_hex != active.source_content_sha256_hex
        || active.staging_archive_sha256_hex.as_deref()
            != Some(verified.manifest.archive_sha256_hex.as_str())
    {
        return Err(RolloverError::ArchiveConflict(
            "existing final manifest does not match active rollover operation".to_owned(),
        ));
    }
    if active.mode == RolloverMode::RolloverReset {
        verify_source_is_clean_reset_target(abs_db, active)?;
        verify_wal_absent_or_empty(abs_db)?;
    } else if active.archive_only_checkpoint_source {
        verify_wal_absent_or_empty(abs_db)?;
    }

    let final_phase = match active.mode {
        RolloverMode::ArchiveOnly => RolloverPhase::CompletedArchiveOnly,
        RolloverMode::RolloverReset => RolloverPhase::CompletedReset,
    };
    let _ = fs::remove_file(pend_manifest);
    retire_completed_operation(
        state_path,
        state_file,
        active,
        final_phase,
        &verified.manifest.archive_sha256_hex,
    )?;

    Ok(SqliteRolloverReport {
        operation_id: active.operation_id.clone(),
        stem: active.stem.clone(),
        mode: active.mode,
        phase: final_phase,
        reused_completed_receipt: false,
        archive_path: verified.archive_path,
        manifest_path: verified.manifest_path,
        manifest: verified.manifest,
    })
}

fn build_manifest(
    active: &ActiveRolloverOperation,
    audit: &DatabaseAuditSummary,
    archive_size_bytes: u64,
    archive_sha256_hex: String,
    status: ManifestStatus,
    source_reset_committed: bool,
    wal_truncated: bool,
) -> ArchiveManifest {
    ArchiveManifest {
        schema: ARCHIVE_MANIFEST_SCHEMA.to_owned(),
        operation_id: active.operation_id.clone(),
        archived_at_rfc3339: active.archived_at_rfc3339.clone(),
        mode: active.mode,
        status,
        source_reset_committed,
        source_ledger_epoch: active.source_ledger_epoch,
        source_ledger_instance_id: active.source_ledger_instance_id.clone(),
        next_ledger_epoch: active.next_ledger_epoch,
        next_ledger_instance_id: active.next_ledger_instance_id.clone(),
        allow_unresolved_used: active.allow_unresolved_used,
        wal_truncated,
        business_schema_version: BLOTTER_SCHEMA_VERSION,
        archive_filename: format!("{}.sqlite", active.stem),
        archive_size_bytes,
        archive_sha256_hex,
        content_sha256_hex: audit.content_sha256_hex.clone(),
        head_seq: audit.head_seq,
        oldest_retained_seq: audit.oldest_retained_seq,
        account_count: u64::try_from(audit.accounts.len()).unwrap_or(u64::MAX),
        accounts: audit.accounts.clone(),
        counts: audit.counts.clone(),
    }
}

fn retire_completed_operation(
    state_path: &Path,
    state_file: &mut RolloverStateFile,
    active: &ActiveRolloverOperation,
    final_phase: RolloverPhase,
    archive_sha256_hex: &str,
) -> Result<(), RolloverError> {
    state_file.active = None;
    state_file.last_completed = Some(CompletedRolloverReceipt {
        operation_id: active.operation_id.clone(),
        stem: active.stem.clone(),
        mode: active.mode,
        phase: final_phase,
        archive_dir_physical_id: active.archive_dir_physical_id.clone(),
        source_ledger_epoch: active.source_ledger_epoch,
        source_ledger_instance_id: active.source_ledger_instance_id.clone(),
        next_ledger_epoch: active.next_ledger_epoch,
        next_ledger_instance_id: active.next_ledger_instance_id.clone(),
        source_content_sha256_hex: active.source_content_sha256_hex.clone(),
        archive_sha256_hex: archive_sha256_hex.to_owned(),
    });
    write_rollover_state_file_atomic(state_path, state_file, &active.operation_id)
}

// ============================================================================
// Atomic Durable File I/O Helpers
// ============================================================================

fn read_rollover_state_file(state_path: &Path) -> Result<RolloverStateFile, RolloverError> {
    let raw = fs::read_to_string(state_path).map_err(|e| RolloverError::Io(e.to_string()))?;
    let parsed: RolloverStateFile = serde_json::from_str(&raw)
        .map_err(|e| RolloverError::StateConflict(format!("corrupt rollover state JSON: {e}")))?;
    if parsed.schema != ROLLOVER_STATE_SCHEMA {
        return Err(RolloverError::StateConflict(format!(
            "unexpected rollover state schema '{}'",
            parsed.schema
        )));
    }
    Ok(parsed)
}

fn write_rollover_state_file_atomic(
    state_path: &Path,
    state: &RolloverStateFile,
    op_id: &str,
) -> Result<(), RolloverError> {
    write_json_file_atomic(state_path, state, op_id)
}

fn write_half_written_temp_file(target_path: &Path, op_id: &str) -> Result<(), RolloverError> {
    let tmp = same_dir_temp_path(target_path, op_id, "tmp")?;
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp)
        .map_err(|e| RolloverError::Io(e.to_string()))?;
    f.write_all(b"{\"schema\":\"ficant.blotter.rollover-state.v1\",\"active\":{\"operation_id\":\"TRUNCATED")
        .map_err(|e| RolloverError::Io(e.to_string()))?;
    f.sync_all().map_err(|e| RolloverError::Io(e.to_string()))?;
    Ok(())
}

fn write_json_file_atomic<T: Serialize>(
    target_path: &Path,
    value: &T,
    op_id: &str,
) -> Result<(), RolloverError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| RolloverError::Io(format!("JSON serialization failed: {e}")))?;
    let tmp = same_dir_temp_path(target_path, op_id, "tmp")?;
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| RolloverError::Io(e.to_string()))?;
        f.write_all(&bytes)
            .map_err(|e| RolloverError::Io(e.to_string()))?;
        f.sync_all().map_err(|e| RolloverError::Io(e.to_string()))?;
    }
    fs::rename(&tmp, target_path).map_err(|e| RolloverError::Io(e.to_string()))?;
    sync_parent_dir(target_path)?;
    Ok(())
}

fn publish_json_no_overwrite<T: Serialize>(
    target_path: &Path,
    value: &T,
    op_id: &str,
) -> Result<(), RolloverError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| RolloverError::Io(format!("JSON serialization failed: {e}")))?;
    let tmp = same_dir_temp_path(target_path, op_id, "publish.tmp")?;
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| RolloverError::Io(e.to_string()))?;
        f.write_all(&bytes)
            .map_err(|e| RolloverError::Io(e.to_string()))?;
        f.sync_all().map_err(|e| RolloverError::Io(e.to_string()))?;
    }
    let publish_res = publish_file_no_overwrite(&tmp, target_path);
    if publish_res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    publish_res
}

/// Atomically publishes `staging_path` to `final_path` WITHOUT overwriting any existing file
/// (`fs::hard_link` fails atomically with `AlreadyExists` if `final_path` exists), then removes
/// `staging_path` so `final_path` retains `nlink == 1`.
fn publish_file_no_overwrite(staging_path: &Path, final_path: &Path) -> Result<(), RolloverError> {
    fs::hard_link(staging_path, final_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            RolloverError::ArchiveConflict(format!(
                "refusing to overwrite existing immutable artifact '{}'",
                final_path.display()
            ))
        } else {
            RolloverError::Io(format!(
                "atomic hard_link publish to '{}' failed: {e}",
                final_path.display()
            ))
        }
    })?;
    sync_parent_dir(final_path)?;
    fs::remove_file(staging_path).map_err(|e| RolloverError::Io(e.to_string()))?;
    sync_parent_dir(final_path)?;
    Ok(())
}

fn copy_file_and_sync(src: &Path, dst: &Path) -> Result<(), RolloverError> {
    let mut reader = File::open(src).map_err(|e| RolloverError::Io(e.to_string()))?;
    let mut writer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .map_err(|e| RolloverError::Io(e.to_string()))?;
    let mut buf = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| RolloverError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        writer
            .write_all(&buf[..n])
            .map_err(|e| RolloverError::Io(e.to_string()))?;
    }
    writer
        .sync_all()
        .map_err(|e| RolloverError::Io(e.to_string()))?;
    sync_parent_dir(dst)?;
    Ok(())
}

fn same_dir_temp_path(
    target_path: &Path,
    op_id: &str,
    tag: &str,
) -> Result<PathBuf, RolloverError> {
    let parent = target_path
        .parent()
        .ok_or_else(|| RolloverError::Io("target path has no parent directory".to_owned()))?;
    let name = target_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| RolloverError::Io("target path has invalid filename".to_owned()))?;
    Ok(parent.join(format!(".{name}.{op_id}.{tag}")))
}

#[allow(clippy::unnecessary_wraps)]
fn sync_parent_dir(path: &Path) -> Result<(), RolloverError> {
    #[cfg(unix)]
    {
        if let Some(parent) = path.parent() {
            let dir = File::open(parent).map_err(|e| RolloverError::Io(e.to_string()))?;
            dir.sync_all()
                .map_err(|e| RolloverError::Io(e.to_string()))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn compute_file_size_and_sha256(path: &Path) -> Result<(u64, String), RolloverError> {
    let mut f = File::open(path).map_err(|e| RolloverError::Io(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buf = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| RolloverError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(n).unwrap_or(u64::MAX))
            .ok_or_else(|| RolloverError::Io("file size overflow".to_owned()))?;
        hasher.update(&buf[..n]);
    }
    Ok((total, encode_hex(&hasher.finalize())))
}

/// Minimal RAII private temporary directory created in `base_dir` (or OS temp dir) using
/// `getrandom`-backed unique suffix so we never depend on `tempfile` in normal `[dependencies]`.
struct PrivateTempDir {
    path: PathBuf,
}

impl PrivateTempDir {
    fn new(preferred_parent: Option<&Path>) -> Result<Self, RolloverError> {
        let base = preferred_parent
            .filter(|p| p.exists())
            .map_or_else(std::env::temp_dir, Path::to_path_buf);
        for _ in 0..16 {
            let id = generate_random_ulid()
                .map_err(|_| RolloverError::Io("failed to generate temp dir ULID".to_owned()))?;
            let candidate = base.join(format!(".ficant-rollover-{id}"));
            match fs::create_dir(&candidate) {
                Ok(()) => return Ok(Self { path: candidate }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => {
                    // Fallback to OS temp dir if preferred parent is read-only.
                    let fallback = std::env::temp_dir().join(format!(".ficant-rollover-{id}"));
                    fs::create_dir(&fallback).map_err(|fe| {
                        RolloverError::Io(format!("failed to create private temp dir: {fe}"))
                    })?;
                    return Ok(Self { path: fallback });
                }
            }
        }
        Err(RolloverError::Io(
            "failed to allocate unique private temp directory".to_owned(),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PrivateTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

// ============================================================================
// Unit tests for pure validators & parsers (`cargo test --lib`)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_and_archive_filename_dual_validators_cover_edge_cases() {
        assert!(validate_archive_stem("blotter-20261125T163000Z-e1").is_ok());
        assert!(validate_archive_stem("A1_b2-c3").is_ok());

        for bad in [
            "",
            "-starts-with-dash",
            "_starts-with-underscore",
            "has.dot",
            "already.sqlite",
            "a/b",
            "a\\b",
            "a:b",
            "..",
            "CON",
            "con",
            "NUL",
            "PRN",
            "AUX",
            "COM1",
            "LPT9",
            "bad space",
            "中文stem",
        ] {
            assert!(
                validate_archive_stem(bad).is_err(),
                "stem '{bad}' must be rejected"
            );
        }

        assert!(validate_archive_filename("day1_e1.sqlite", "day1_e1").is_ok());
        assert!(validate_archive_filename("day1_e1.sqlite.sqlite", "day1_e1").is_err());
        assert!(validate_archive_filename("day1_e1.sqlite.sqlite", "day1_e1.sqlite").is_err());
        assert!(validate_archive_filename("other.sqlite", "day1_e1").is_err());
        assert!(validate_archive_filename("day1_e1", "day1_e1").is_err());
        assert!(validate_archive_filename("sub/day1_e1.sqlite", "day1_e1").is_err());
    }

    #[test]
    fn parse_single_hex_file_id_works_for_english_and_chinese_windows_locales() {
        assert_eq!(
            parse_single_hex_file_id("File ID is 0x00000000000000000017000000443027\r\n").unwrap(),
            "0x00000000000000000017000000443027"
        );
        assert_eq!(
            parse_single_hex_file_id("文件 ID 为 0x00000000000000000017000000443027\r\n").unwrap(),
            "0x00000000000000000017000000443027"
        );
        assert_eq!(
            parse_single_hex_file_id("ID: 0X1A2B").unwrap(),
            "0x00000000000000000000000000001a2b"
        );
        assert!(parse_single_hex_file_id("no hex token here").is_err());
        assert!(parse_single_hex_file_id("two tokens 0x11 and 0x22").is_err());
    }

    #[test]
    fn manifest_five_state_matrix_accepts_legal_and_rejects_illegal_states() {
        let op_id = generate_random_ulid().unwrap();
        let src_id = generate_random_ulid().unwrap();
        let next_id = generate_distinct_random_ulid(&src_id).unwrap();

        let base = ArchiveManifest {
            schema: ARCHIVE_MANIFEST_SCHEMA.to_owned(),
            operation_id: op_id,
            archived_at_rfc3339: "2026-11-25T16:30:00Z".to_owned(),
            mode: RolloverMode::RolloverReset,
            status: ManifestStatus::Pending,
            source_reset_committed: false,
            source_ledger_epoch: 1,
            source_ledger_instance_id: src_id,
            next_ledger_epoch: Some(2),
            next_ledger_instance_id: Some(next_id),
            allow_unresolved_used: false,
            wal_truncated: false,
            business_schema_version: 1,
            archive_filename: "snap_e1.sqlite".to_owned(),
            archive_size_bytes: 4096,
            archive_sha256_hex: "a".repeat(64),
            content_sha256_hex: "b".repeat(64),
            head_seq: 0,
            oldest_retained_seq: 1,
            account_count: 0,
            accounts: Vec::new(),
            counts: ArchiveManifestCounts::default(),
        };

        // ① pending rollover_reset (committed=false)
        assert!(validate_archive_manifest(&base, "snap_e1", false).is_ok());
        assert!(validate_archive_manifest(&base, "snap_e1", true).is_err());

        // ② pending rollover_reset (committed=true, wal_truncated=false)
        let mut s2 = base.clone();
        s2.source_reset_committed = true;
        assert!(validate_archive_manifest(&s2, "snap_e1", false).is_ok());
        assert!(validate_archive_manifest(&s2, "snap_e1", true).is_err());

        // ③ pending archive_only
        let mut s3 = base.clone();
        s3.mode = RolloverMode::ArchiveOnly;
        s3.next_ledger_epoch = None;
        s3.next_ledger_instance_id = None;
        assert!(validate_archive_manifest(&s3, "snap_e1", false).is_ok());
        assert!(validate_archive_manifest(&s3, "snap_e1", true).is_err());

        // ④ final archive_only (wal_truncated=false and wal_truncated=true both valid)
        let mut s4 = s3.clone();
        s4.status = ManifestStatus::ArchiveOnly;
        assert!(validate_archive_manifest(&s4, "snap_e1", true).is_ok());
        s4.wal_truncated = true;
        assert!(validate_archive_manifest(&s4, "snap_e1", true).is_ok());
        // archive_only with allow_unresolved_used=true is illegal
        let mut bad_s4 = s4.clone();
        bad_s4.allow_unresolved_used = true;
        assert!(validate_archive_manifest(&bad_s4, "snap_e1", true).is_err());

        // ⑤ final completed_reset
        let mut s5 = base.clone();
        s5.status = ManifestStatus::CompletedReset;
        s5.source_reset_committed = true;
        s5.wal_truncated = true;
        assert!(validate_archive_manifest(&s5, "snap_e1", true).is_ok());

        // Illegal: half-null next_*
        let mut half_null = s5.clone();
        half_null.next_ledger_instance_id = None;
        assert!(validate_archive_manifest(&half_null, "snap_e1", true).is_err());

        // Illegal: completed_reset with wal_truncated=false
        let mut bad_wal = s5.clone();
        bad_wal.wal_truncated = false;
        assert!(validate_archive_manifest(&bad_wal, "snap_e1", true).is_err());

        // Illegal: completed_reset with unresolved_committed > 0 but allow_unresolved_used=false
        let mut bad_unres = s5;
        bad_unres.counts.unresolved_committed = 1;
        assert!(validate_archive_manifest(&bad_unres, "snap_e1", true).is_err());
        bad_unres.allow_unresolved_used = true;
        assert!(validate_archive_manifest(&bad_unres, "snap_e1", true).is_ok());
    }
}
