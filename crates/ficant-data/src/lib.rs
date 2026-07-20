//! External market-data adapters and canonicalization boundary.
//!
//! This crate owns source-specific parsing and Arrow representation. It consumes validated domain
//! definitions but never exposes database connections, file paths, or credentials to Domain or
//! Application.

#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

mod canonical;
mod error;
mod file_ndjson;
mod mapping;
mod postgres;
mod snapshot;
mod source;

pub use canonical::{
    CANONICAL_QUOTE_SCHEMA_ID, CanonicalIngestRequest, CanonicalQuoteBatch, CanonicalQuoteIngestor,
    QualityReport, canonical_quote_schema, canonical_quote_schema_hash,
};
pub use error::{DataError, DataResult};
pub use file_ndjson::FileNdjsonQuoteSource;
pub use mapping::{InstrumentMapping, InstrumentMappingEntry};
pub use postgres::PostgresQuoteSource;
pub use snapshot::{
    CanonicalSnapshotCodec, CanonicalSnapshotPackage, PARQUET_CREATED_BY,
    SNAPSHOT_MANIFEST_SCHEMA_ID, SnapshotManifest, VerifiedCanonicalSnapshot,
};
pub use source::{PointInTimeWindow, RawDecimal, RawQuoteRow, RawQuoteSource};
