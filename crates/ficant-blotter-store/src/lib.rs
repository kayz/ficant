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
pub mod schema;
pub mod spi;
pub mod store;

pub use codec::{format_fixed_decimal, parse_fixed_decimal};
pub use schema::{
    BLOTTER_SCHEMA_DDL, BLOTTER_SCHEMA_VERSION, REQUIRED_TABLES, SqlitePragmaSnapshot,
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
