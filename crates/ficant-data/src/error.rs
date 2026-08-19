use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceRowViolationReason {
    ObservedAfterVisible,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DataError {
    #[error("invalid data configuration")]
    InvalidConfiguration,
    #[error("source data is invalid")]
    InvalidSourceData,
    #[error("point-in-time boundary was violated")]
    PointInTimeViolation,
    #[error("data quality rule failed")]
    QualityRuleFailed,
    #[error("source row violates a canonical bitemporal rule")]
    SourceRowViolation {
        source_record_id: String,
        reason: SourceRowViolationReason,
    },
    #[error("data source is unavailable")]
    SourceUnavailable,
    #[error("canonical schema mismatch")]
    SchemaMismatch,
    #[error("snapshot integrity validation failed")]
    SnapshotIntegrityFailed,
}

impl DataError {
    #[must_use]
    pub fn observed_after_visible_source_record_id(&self) -> Option<&str> {
        match self {
            Self::SourceRowViolation {
                source_record_id,
                reason: SourceRowViolationReason::ObservedAfterVisible,
            } => Some(source_record_id),
            _ => None,
        }
    }
}

pub type DataResult<T> = Result<T, DataError>;
