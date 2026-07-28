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
pub enum ApplicationErrorDetail {
    RulePackItemMissing { path: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationError {
    category: ApplicationErrorCategory,
    retryable: bool,
    detail: Option<ApplicationErrorDetail>,
}

impl ApplicationError {
    #[must_use]
    pub fn new(category: ApplicationErrorCategory, retryable: bool) -> Self {
        Self {
            category,
            retryable,
            detail: None,
        }
    }

    /// Builds the one client-safe detail used for a missing parsed `RulePack` item.
    #[must_use]
    pub fn rule_pack_item_missing(path: impl Into<String>) -> Self {
        let path = path.into();
        let detail = is_safe_rule_pack_path(&path)
            .then_some(ApplicationErrorDetail::RulePackItemMissing { path });
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail,
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

    #[must_use]
    pub fn detail(&self) -> Option<&ApplicationErrorDetail> {
        self.detail.as_ref()
    }
}

fn is_safe_rule_pack_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'[' | b']' | b'=' | b'-')
        })
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
