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

    let field_violations = map_field_violations(error.detail());

    ErrorDetail {
        code: code as i32,
        message,
        trace_id: String::new(),
        retryable: error.retryable(),
        resource_ref: String::new(),
        field_violations,
    }
}

fn map_field_violations(detail: Option<&ApplicationErrorDetail>) -> Vec<FieldViolation> {
    match detail {
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
        Some(ApplicationErrorDetail::BlotterFeedPaused) => vec![FieldViolation {
            field: "feed".to_owned(),
            description: "trade feed is paused".to_owned(),
        }],
        Some(ApplicationErrorDetail::BlotterInsufficientAvailable) => vec![FieldViolation {
            field: "notional".to_owned(),
            description: "insufficient available notional".to_owned(),
        }],
        Some(ApplicationErrorDetail::BlotterSellAmbiguityLocked) => vec![FieldViolation {
            field: "side".to_owned(),
            description: "sell ambiguity lock active".to_owned(),
        }],
        Some(ApplicationErrorDetail::BlotterInvalidStateTransition) => vec![FieldViolation {
            field: "state".to_owned(),
            description: "invalid blotter state transition".to_owned(),
        }],
        Some(ApplicationErrorDetail::BlotterRulePackInvalid) => vec![FieldViolation {
            field: "regulatory_rule_pack".to_owned(),
            description: "regulatory rule pack missing or invalid".to_owned(),
        }],
        Some(ApplicationErrorDetail::BlotterConstraintLimitMissing { limit_code }) => {
            vec![FieldViolation {
                field: format!("constraints[{limit_code}]"),
                description: "required constraint limit ceiling is missing".to_owned(),
            }]
        }
        Some(ApplicationErrorDetail::BlotterConstraintLimitExceeded { limit_code }) => {
            vec![FieldViolation {
                field: format!("constraints[{limit_code}]"),
                description: "proposed trade breaches bound constraint limit".to_owned(),
            }]
        }
        Some(ApplicationErrorDetail::BlotterUncoveredRiskAcknowledgementRequired) => {
            vec![FieldViolation {
                field: "acknowledge_uncovered_dv01".to_owned(),
                description: "explicit trader acknowledgement required for uncovered DV01 risk"
                    .to_owned(),
            }]
        }
        Some(ApplicationErrorDetail::BlotterWholeAccountHedgeBlocked) => vec![FieldViolation {
            field: "scope".to_owned(),
            description: "whole-account hedge blocked while uncovered instruments exist".to_owned(),
        }],
        Some(ApplicationErrorDetail::BlotterEmptyCoveredBook) => vec![FieldViolation {
            field: "account_id".to_owned(),
            description: "no covered instruments available for hedge preview".to_owned(),
        }],
        Some(ApplicationErrorDetail::BlotterHedgeProductNotAllowed) => vec![FieldViolation {
            field: "product_code".to_owned(),
            description: "hedge product code not allowed by active desk policy".to_owned(),
        }],
        Some(ApplicationErrorDetail::BlotterCalendarCoverageMissing) => vec![FieldViolation {
            field: "calendar".to_owned(),
            description: "required exact market calendar coverage is missing".to_owned(),
        }],
        Some(ApplicationErrorDetail::BlotterPayloadTooLarge) => vec![FieldViolation {
            field: "request".to_owned(),
            description: "encoded payload exceeds transport limit; split batch and retry"
                .to_owned(),
        }],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use ficant_application::{ApplicationError, ApplicationErrorCategory, map_blotter_error};
    use ficant_contracts::ficant::core::v1::ErrorCode;
    use ficant_domain::blotter::BlotterError;

    use super::to_error_detail;

    #[test]
    fn maps_blotter_domain_errors_with_sanitized_messages() {
        let secret_msg = "SELECT * FROM sqlite_master bearer=topsecret";
        let mapped = map_blotter_error(&BlotterError::feed_paused(secret_msg));
        let detail = to_error_detail(&mapped);
        assert_eq!(detail.code, ErrorCode::ValidationFailed as i32);
        assert!(detail.retryable);
        assert!(!detail.message.contains("SELECT"));
        assert!(!detail.message.contains("topsecret"));
        assert_eq!(detail.field_violations.len(), 1);
        assert_eq!(detail.field_violations[0].field, "feed");

        let immut = to_error_detail(&map_blotter_error(&BlotterError::immutable_violation(
            secret_msg,
        )));
        assert_eq!(immut.code, ErrorCode::ImmutableViolation as i32);
        assert!(!immut.retryable);
        assert!(!immut.message.contains("topsecret"));

        let conflict = to_error_detail(&map_blotter_error(&BlotterError::concurrency_conflict(
            secret_msg,
        )));
        assert_eq!(conflict.code, ErrorCode::ConcurrencyConflict as i32);
        assert!(conflict.retryable);

        let hash_err =
            to_error_detail(&map_blotter_error(&BlotterError::hash_mismatch(secret_msg)));
        assert_eq!(hash_err.code, ErrorCode::HashMismatch as i32);
        assert!(!hash_err.retryable);

        let limit_missing = to_error_detail(&ApplicationError::blotter_constraint_limit_missing(
            "MAX_INST_FACE",
        ));
        assert_eq!(limit_missing.code, ErrorCode::LineageIncomplete as i32);
        assert_eq!(
            limit_missing.field_violations[0].field,
            "constraints[MAX_INST_FACE]"
        );

        let unauth = to_error_detail(&ApplicationError::new(
            ApplicationErrorCategory::Unauthenticated,
            false,
        ));
        assert_eq!(unauth.code, ErrorCode::Unauthenticated as i32);

        let forbidden = to_error_detail(&ApplicationError::new(
            ApplicationErrorCategory::Forbidden,
            false,
        ));
        assert_eq!(forbidden.code, ErrorCode::Forbidden as i32);

        let storage = to_error_detail(&ApplicationError::new(
            ApplicationErrorCategory::StorageUnavailable,
            true,
        ));
        assert_eq!(storage.code, ErrorCode::StorageUnavailable as i32);
        assert!(storage.retryable);
    }
}
