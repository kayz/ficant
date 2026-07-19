use thiserror::Error;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum DataError {
    #[error("invalid data configuration")]
    InvalidConfiguration,
    #[error("source data is invalid")]
    InvalidSourceData,
    #[error("point-in-time boundary was violated")]
    PointInTimeViolation,
    #[error("data quality rule failed")]
    QualityRuleFailed,
    #[error("data source is unavailable")]
    SourceUnavailable,
    #[error("canonical schema mismatch")]
    SchemaMismatch,
    #[error("snapshot integrity validation failed")]
    SnapshotIntegrityFailed,
}

pub type DataResult<T> = Result<T, DataError>;
