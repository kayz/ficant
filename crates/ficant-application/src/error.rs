use ficant_domain::DomainErrorCode;
use ficant_runtime::RuntimeError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationErrorCategory {
    ValidationFailed,
    NotFound,
    AlreadyExists,
    VersionConflict,
    ConcurrencyConflict,
    ImmutableViolation,
    HashMismatch,
    LineageIncomplete,
    StateConflict,
    Unauthenticated,
    Forbidden,
    StorageUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationError {
    category: ApplicationErrorCategory,
    retryable: bool,
}

impl ApplicationError {
    #[must_use]
    pub fn new(category: ApplicationErrorCategory, retryable: bool) -> Self {
        Self {
            category,
            retryable,
        }
    }

    #[must_use]
    pub fn category(&self) -> ApplicationErrorCategory {
        self.category
    }

    #[must_use]
    pub fn retryable(&self) -> bool {
        self.retryable
    }
}

#[must_use]
pub fn map_domain_error(error: DomainErrorCode) -> ApplicationError {
    let (category, retryable) = match error {
        DomainErrorCode::InvalidId
        | DomainErrorCode::InvalidUnit
        | DomainErrorCode::InvalidEffectiveTime
        | DomainErrorCode::InvalidValue => (ApplicationErrorCategory::ValidationFailed, false),
        DomainErrorCode::VersionConflict => (ApplicationErrorCategory::VersionConflict, true),
        DomainErrorCode::ContentHashMismatch => (ApplicationErrorCategory::HashMismatch, false),
        DomainErrorCode::BrokenLineage => (ApplicationErrorCategory::LineageIncomplete, false),
        DomainErrorCode::InvalidStateTransition => (ApplicationErrorCategory::StateConflict, false),
        DomainErrorCode::JournalSequenceConflict => {
            (ApplicationErrorCategory::ConcurrencyConflict, true)
        }
    };
    ApplicationError::new(category, retryable)
}

#[must_use]
pub fn map_runtime_error(error: &RuntimeError) -> ApplicationError {
    match error {
        RuntimeError::Domain(error) => map_domain_error(*error),
        RuntimeError::IdempotencyConflict => {
            ApplicationError::new(ApplicationErrorCategory::AlreadyExists, false)
        }
        RuntimeError::ConcurrencyConflict { .. } => {
            ApplicationError::new(ApplicationErrorCategory::ConcurrencyConflict, true)
        }
        RuntimeError::RunIdentityConflict => {
            ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
        }
    }
}
