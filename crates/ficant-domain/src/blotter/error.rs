use thiserror::Error;

use crate::DomainErrorCode;

/// Semantic category for desk-blotter domain errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlotterErrorKind {
    ValidationFailed,
    NotFound,
    ConcurrencyConflict,
    VersionConflict,
    ImmutableViolation,
    HashMismatch,
    InvalidStateTransition,
    FeedPaused,
    InsufficientAvailable,
    SellAmbiguityLocked,
    LineageIncomplete,
}

/// Pure domain error returned by the synchronous desk-blotter state machine.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{kind:?}: {message}")]
pub struct BlotterError {
    kind: BlotterErrorKind,
    message: String,
    retryable: bool,
}

impl BlotterError {
    pub fn new(kind: BlotterErrorKind, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable,
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(BlotterErrorKind::ValidationFailed, message, false)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(BlotterErrorKind::NotFound, message, false)
    }

    pub fn concurrency_conflict(message: impl Into<String>) -> Self {
        Self::new(BlotterErrorKind::ConcurrencyConflict, message, true)
    }

    pub fn version_conflict(message: impl Into<String>) -> Self {
        Self::new(BlotterErrorKind::VersionConflict, message, true)
    }

    pub fn immutable_violation(message: impl Into<String>) -> Self {
        Self::new(BlotterErrorKind::ImmutableViolation, message, false)
    }

    pub fn hash_mismatch(message: impl Into<String>) -> Self {
        Self::new(BlotterErrorKind::HashMismatch, message, false)
    }

    pub fn invalid_state_transition(message: impl Into<String>) -> Self {
        Self::new(BlotterErrorKind::InvalidStateTransition, message, false)
    }

    pub fn feed_paused(message: impl Into<String>) -> Self {
        Self::new(BlotterErrorKind::FeedPaused, message, true)
    }

    pub fn insufficient_available(message: impl Into<String>) -> Self {
        Self::new(BlotterErrorKind::InsufficientAvailable, message, false)
    }

    pub fn sell_ambiguity_locked(message: impl Into<String>) -> Self {
        Self::new(BlotterErrorKind::SellAmbiguityLocked, message, false)
    }

    pub fn lineage_incomplete(message: impl Into<String>) -> Self {
        Self::new(BlotterErrorKind::LineageIncomplete, message, false)
    }

    pub const fn kind(&self) -> BlotterErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    /// Maps this blotter error onto the crate-level [`DomainErrorCode`] taxonomy.
    pub const fn domain_error_code(&self) -> DomainErrorCode {
        match self.kind {
            BlotterErrorKind::ValidationFailed | BlotterErrorKind::NotFound => {
                DomainErrorCode::InvalidValue
            }
            BlotterErrorKind::ConcurrencyConflict => DomainErrorCode::JournalSequenceConflict,
            BlotterErrorKind::VersionConflict => DomainErrorCode::VersionConflict,
            BlotterErrorKind::HashMismatch => DomainErrorCode::ContentHashMismatch,
            BlotterErrorKind::LineageIncomplete => DomainErrorCode::BrokenLineage,
            BlotterErrorKind::ImmutableViolation
            | BlotterErrorKind::InvalidStateTransition
            | BlotterErrorKind::FeedPaused
            | BlotterErrorKind::InsufficientAvailable
            | BlotterErrorKind::SellAmbiguityLocked => DomainErrorCode::InvalidStateTransition,
        }
    }
}

impl From<DomainErrorCode> for BlotterError {
    fn from(code: DomainErrorCode) -> Self {
        match code {
            DomainErrorCode::InvalidId
            | DomainErrorCode::InvalidUnit
            | DomainErrorCode::InvalidEffectiveTime
            | DomainErrorCode::InvalidValue => Self::validation(code.to_string()),
            DomainErrorCode::VersionConflict => Self::version_conflict(code.to_string()),
            DomainErrorCode::ContentHashMismatch => Self::hash_mismatch(code.to_string()),
            DomainErrorCode::BrokenLineage => Self::lineage_incomplete(code.to_string()),
            DomainErrorCode::InvalidStateTransition => {
                Self::invalid_state_transition(code.to_string())
            }
            DomainErrorCode::JournalSequenceConflict => {
                Self::concurrency_conflict(code.to_string())
            }
        }
    }
}

pub type BlotterResult<T> = Result<T, BlotterError>;
