use ficant_domain::DomainErrorCode;
use ficant_domain::market::ImportInterface;
use ficant_domain::primitives::VersionRef;
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
    RulePackItemMissing {
        path: String,
    },
    SubjectBindingInvalid,
    UnknownAccountingPositions {
        position_ids: Vec<String>,
    },
    DataSourceNotAuthorized {
        authorization_ref: VersionRef,
        data_source_ref: Option<VersionRef>,
        import_interface: ImportInterface,
    },
    SourceRowViolation {
        source_record_id: String,
        reason: SourceRowViolationReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceRowViolationReason {
    ObservedAfterVisible,
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

    /// Builds the one client-safe detail used when a required exact Subject binding is absent,
    /// cannot be read, drifts from the requested reference, or lacks the requested access.
    #[must_use]
    pub const fn subject_binding_invalid() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::SubjectBindingInvalid),
        }
    }

    #[must_use]
    pub fn unknown_accounting_positions(position_ids: Vec<String>) -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::UnknownAccountingPositions { position_ids }),
        }
    }

    #[must_use]
    pub fn data_source_not_authorized(
        authorization_ref: VersionRef,
        data_source_ref: Option<VersionRef>,
        import_interface: ImportInterface,
    ) -> Self {
        Self {
            category: ApplicationErrorCategory::Forbidden,
            retryable: false,
            detail: Some(ApplicationErrorDetail::DataSourceNotAuthorized {
                authorization_ref,
                data_source_ref,
                import_interface,
            }),
        }
    }

    /// Builds a typed, client-safe canonical source-row violation.
    #[must_use]
    pub fn observed_after_visible_source_row(source_record_id: String) -> Self {
        let detail = is_safe_source_record_id(&source_record_id).then_some(
            ApplicationErrorDetail::SourceRowViolation {
                source_record_id,
                reason: SourceRowViolationReason::ObservedAfterVisible,
            },
        );
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

fn is_safe_source_record_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
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
