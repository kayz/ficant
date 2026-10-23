//! External market-data adapters and canonicalization boundary.
//!
//! This crate owns source-specific parsing and Arrow representation. It consumes validated domain
//! definitions but never exposes database connections, file paths, or credentials to Domain or
//! Application.

#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

mod bar_canonical;
mod bar_source;
mod canonical;
mod catalog;
mod error;
mod file_ndjson;
mod governed_import;
mod mapping;
mod postgres;
mod snapshot;
mod source;

pub use bar_canonical::{
    BAR_QUALITY_RULE_SET_ID, BarIngestReport, BarRejection, BarRejectionReason,
    CANONICAL_BAR_SCHEMA_ID, CanonicalBarBatch, CanonicalBarIngestRequest, CanonicalBarIngestor,
    canonical_bar_schema, canonical_bar_schema_hash,
};
pub use bar_source::{
    BarSourceAdapter, BarVisibilityRule, CodeField, FileNdjsonBarSource, NdjsonBarMapping,
    RawBarRow, SourceTimeFormat, parse_decimal_text,
};
pub use canonical::{
    CANONICAL_QUOTE_SCHEMA_ID, CanonicalIngestRequest, CanonicalQuoteBatch, CanonicalQuoteIngestor,
    QualityReport, canonical_quote_schema, canonical_quote_schema_hash,
};
pub use catalog::{QuoteSourceCatalog, RegisteredQuoteSource};
pub use error::{DataError, DataResult};
pub use file_ndjson::FileNdjsonQuoteSource;
pub use governed_import::{
    GovernedCanonicalImportRequest, GovernedCanonicalQuoteImporter, PreparedCanonicalImport,
    canonical_data_source_content_hash,
};
pub use mapping::{InstrumentMapping, InstrumentMappingEntry};
pub use postgres::PostgresQuoteSource;
pub use snapshot::{
    CanonicalImportEvidence, CanonicalQuoteProjection, CanonicalSnapshotCodec,
    CanonicalSnapshotPackage, PARQUET_CREATED_BY, SNAPSHOT_MANIFEST_SCHEMA_ID, SnapshotManifest,
    VerifiedCanonicalSnapshot,
};
pub use source::{PointInTimeWindow, RawDecimal, RawQuoteRow, RawQuoteSource};
