//! Single-writer exclusive WAL `SQLite` persistence and four open-source reference SPI adapters
//! for Ficant Desk Blotter.
//!
//! Enforces:
//! - Production path anti-spoofing (URI open disabled; rejects `:memory:`, all `file:` URIs,
//!   empty paths, directories, and contradictory mode flags);
//! - Zero-wait `locking_mode = EXCLUSIVE`, `journal_mode = WAL`, `synchronous = FULL`, and
//!   `foreign_keys = ON`;
//! - Explicit business schema version verification via `blotter_meta.schema_version` (never
//!   `SQLite`'s internal `PRAGMA schema_version`);
//! - Atomic single-writer commit, row-level CAS, deterministic fault injection, full WAL restart
//!   recovery, and retention pruning that trims only `blotter_event_journal`.

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod codec;
pub mod rollover;
pub mod schema;
pub mod spi;
pub mod store;

pub use codec::{format_fixed_decimal, parse_fixed_decimal};
pub use rollover::{
    ARCHIVE_MANIFEST_SCHEMA, ArchiveManifest, ArchiveManifestCounts, DatabaseAuditSummary,
    FileIdentityInfo, ManifestAccountSummary, ManifestStatus, PhysicalFileId,
    ROLLOVER_STATE_SCHEMA, RolloverError, RolloverFaultInjection, RolloverMode, RolloverPhase,
    SqliteRolloverReport, SqliteRolloverRequest, VerifySqliteArchiveReport, audit_sqlite_database,
    companion_lock_path, inspect_physical_file_identity, parse_single_hex_file_id,
    rollover_state_path, sqlite_rollover, sqlite_rollover_with_fault, validate_archive_filename,
    validate_archive_manifest, validate_archive_stem, verify_sqlite_archive,
};
pub use schema::{
    BLOTTER_META_EXTENDED_COLUMNS, BLOTTER_META_LEGACY_COLUMNS, BLOTTER_SCHEMA_DDL,
    BLOTTER_SCHEMA_VERSION, LedgerIdentity, MetaSchemaLayout, REQUIRED_TABLES,
    SqlitePragmaSnapshot, StartupSchemaFault, generate_random_ulid, is_canonical_ulid,
    validate_production_sqlite_path,
};
pub use spi::{
    CsvMarketQuoteStream, CsvPositionBaseProvider, CsvTable, CsvTemplateNlpIntentParser,
    CsvTradeFeedReconciler, MockMarketQuoteStream, MockNlpIntentParser, MockPositionBaseProvider,
    MockTradeFeedReconciler, default_reference_analysis_units,
};
pub use store::{
    DEFAULT_EVENT_RETENTION_WINDOW, EventCommitHook, SqliteBlotterOpenOptions, SqliteBlotterStore,
    StoreFaultInjection, WriteExecutionOutcome,
};
