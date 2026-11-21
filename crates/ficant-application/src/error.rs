use ficant_domain::DomainErrorCode;
use ficant_domain::blotter::{BlotterError, BlotterErrorKind};

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
    SubjectBindingInvalid,
    UnknownAccountingPositions { position_ids: Vec<String> },
    BlotterFeedPaused,
    BlotterInsufficientAvailable,
    BlotterSellAmbiguityLocked,
    BlotterInvalidStateTransition,
    BlotterRulePackInvalid,
    BlotterConstraintLimitMissing { limit_code: String },
    BlotterConstraintLimitExceeded { limit_code: String },
    BlotterUncoveredRiskAcknowledgementRequired,
    BlotterWholeAccountHedgeBlocked,
    BlotterEmptyCoveredBook,
    BlotterHedgeProductNotAllowed,
    BlotterCalendarCoverageMissing,
    BlotterPayloadTooLarge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationError {
    category: ApplicationErrorCategory,
    retryable: bool,
    detail: Option<ApplicationErrorDetail>,
}

impl ApplicationError {
    #[must_use]
    pub const fn new(category: ApplicationErrorCategory, retryable: bool) -> Self {
        Self {
            category,
            retryable,
            detail: None,
        }
    }

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
    pub const fn subject_binding_invalid() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::SubjectBindingInvalid),
        }
    }

    #[must_use]
    pub const fn unknown_accounting_positions(position_ids: Vec<String>) -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::UnknownAccountingPositions { position_ids }),
        }
    }

    #[must_use]
    pub const fn blotter_feed_paused() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: true,
            detail: Some(ApplicationErrorDetail::BlotterFeedPaused),
        }
    }

    #[must_use]
    pub const fn blotter_insufficient_available() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::BlotterInsufficientAvailable),
        }
    }

    #[must_use]
    pub const fn blotter_sell_ambiguity_locked() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::BlotterSellAmbiguityLocked),
        }
    }

    #[must_use]
    pub const fn blotter_invalid_state_transition() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::BlotterInvalidStateTransition),
        }
    }

    #[must_use]
    pub const fn blotter_rule_pack_invalid() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::BlotterRulePackInvalid),
        }
    }

    #[must_use]
    pub fn blotter_constraint_limit_missing(limit_code: impl Into<String>) -> Self {
        let limit_code = limit_code.into();
        let detail = is_safe_rule_pack_path(&limit_code)
            .then_some(ApplicationErrorDetail::BlotterConstraintLimitMissing { limit_code });
        Self {
            category: ApplicationErrorCategory::LineageIncomplete,
            retryable: false,
            detail,
        }
    }

    #[must_use]
    pub fn blotter_constraint_limit_exceeded(limit_code: impl Into<String>) -> Self {
        let limit_code = limit_code.into();
        let detail = is_safe_rule_pack_path(&limit_code)
            .then_some(ApplicationErrorDetail::BlotterConstraintLimitExceeded { limit_code });
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail,
        }
    }

    #[must_use]
    pub const fn blotter_uncovered_risk_acknowledgement_required() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::BlotterUncoveredRiskAcknowledgementRequired),
        }
    }

    #[must_use]
    pub const fn blotter_whole_account_hedge_blocked() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::BlotterWholeAccountHedgeBlocked),
        }
    }

    #[must_use]
    pub const fn blotter_empty_covered_book() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::BlotterEmptyCoveredBook),
        }
    }

    #[must_use]
    pub const fn blotter_hedge_product_not_allowed() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::BlotterHedgeProductNotAllowed),
        }
    }

    #[must_use]
    pub const fn blotter_calendar_coverage_missing() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::BlotterCalendarCoverageMissing),
        }
    }

    #[must_use]
    pub const fn blotter_payload_too_large() -> Self {
        Self {
            category: ApplicationErrorCategory::ValidationFailed,
            retryable: false,
            detail: Some(ApplicationErrorDetail::BlotterPayloadTooLarge),
        }
    }

    #[must_use]
    pub const fn category(&self) -> ApplicationErrorCategory {
        self.category
    }

    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    #[must_use]
    pub const fn detail(&self) -> Option<&ApplicationErrorDetail> {
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
pub const fn map_domain_error(error: DomainErrorCode) -> ApplicationError {
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
pub fn map_blotter_error(error: &BlotterError) -> ApplicationError {
    match error.kind() {
        BlotterErrorKind::ValidationFailed => {
            ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
        }
        BlotterErrorKind::NotFound => {
            ApplicationError::new(ApplicationErrorCategory::NotFound, false)
        }
        BlotterErrorKind::ConcurrencyConflict => {
            ApplicationError::new(ApplicationErrorCategory::ConcurrencyConflict, true)
        }
        BlotterErrorKind::VersionConflict => {
            ApplicationError::new(ApplicationErrorCategory::VersionConflict, true)
        }
        BlotterErrorKind::ImmutableViolation => {
            ApplicationError::new(ApplicationErrorCategory::ImmutableViolation, false)
        }
        BlotterErrorKind::HashMismatch => {
            ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
        }
        BlotterErrorKind::LineageIncomplete => {
            ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
        }
        BlotterErrorKind::InvalidStateTransition => {
            ApplicationError::blotter_invalid_state_transition()
        }
        BlotterErrorKind::FeedPaused => ApplicationError::blotter_feed_paused(),
        BlotterErrorKind::InsufficientAvailable => {
            ApplicationError::blotter_insufficient_available()
        }
        BlotterErrorKind::SellAmbiguityLocked => ApplicationError::blotter_sell_ambiguity_locked(),
    }
}
