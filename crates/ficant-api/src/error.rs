use ficant_application::{ApplicationError, ApplicationErrorCategory, ApplicationErrorDetail};
use ficant_contracts::ficant::core::v1::{ErrorCode, ErrorDetail, FieldViolation};

pub fn to_error_detail(error: &ApplicationError) -> ErrorDetail {
    let (code, message) = match error.category() {
        ApplicationErrorCategory::ValidationFailed => (
            ErrorCode::ValidationFailed,
            "request validation failed".to_owned(),
        ),
        ApplicationErrorCategory::NotFound => {
            (ErrorCode::NotFound, "resource not found".to_owned())
        }
        ApplicationErrorCategory::AlreadyExists => (
            ErrorCode::AlreadyExists,
            "resource already exists".to_owned(),
        ),
        ApplicationErrorCategory::VersionConflict => {
            (ErrorCode::VersionConflict, "version conflict".to_owned())
        }
        ApplicationErrorCategory::ConcurrencyConflict => (
            ErrorCode::ConcurrencyConflict,
            "concurrency conflict".to_owned(),
        ),
        ApplicationErrorCategory::ImmutableViolation => (
            ErrorCode::ImmutableViolation,
            "immutable violation".to_owned(),
        ),
        ApplicationErrorCategory::HashMismatch => {
            (ErrorCode::HashMismatch, "content hash mismatch".to_owned())
        }
        ApplicationErrorCategory::LineageIncomplete => (
            ErrorCode::LineageIncomplete,
            "lineage incomplete".to_owned(),
        ),
        ApplicationErrorCategory::StateConflict => {
            (ErrorCode::Internal, "calculation state error".to_owned())
        }
        ApplicationErrorCategory::Unauthenticated => {
            (ErrorCode::Unauthenticated, "unauthenticated".to_owned())
        }
        ApplicationErrorCategory::Forbidden => (ErrorCode::Forbidden, "forbidden".to_owned()),
        ApplicationErrorCategory::StorageUnavailable => (
            ErrorCode::StorageUnavailable,
            "storage unavailable".to_owned(),
        ),
    };

    let field_violations = match error.detail() {
        Some(ApplicationErrorDetail::RulePackItemMissing { path }) => vec![FieldViolation {
            field: path.clone(),
            description: "missing required rule pack item".to_owned(),
        }],
        Some(ApplicationErrorDetail::SubjectBindingInvalid) => vec![FieldViolation {
            field: "subject".to_owned(),
            description: "invalid subject binding".to_owned(),
        }],
        Some(ApplicationErrorDetail::UnknownAccountingPositions { position_ids }) => position_ids
            .iter()
            .map(|id| FieldViolation {
                field: format!("positions[{id}].accounting_classification"),
                description: "accounting classification is UNKNOWN".to_owned(),
            })
            .collect(),
        None => Vec::new(),
    };

    ErrorDetail {
        code: code as i32,
        message,
        trace_id: String::new(),
        retryable: error.retryable(),
        resource_ref: String::new(),
        field_violations,
    }
}
