use ficant_api::CoreBusinessErrorMapper;
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_contracts::ficant::core::v1::{ErrorCode, ErrorDetail};
use ficant_domain::DomainErrorCode;
use prost::Message;
use tonic::Code;

const TRACE_KEY: &[u8; 32] = b"iteration-2-core-error-key-00001";

#[test]
fn every_application_category_has_an_exact_core_and_transport_mapping() {
    let mapper = CoreBusinessErrorMapper::new(TRACE_KEY).expect("valid trace key");
    let cases = [
        (
            ApplicationErrorCategory::ValidationFailed,
            false,
            ErrorCode::ValidationFailed,
            Code::InvalidArgument,
        ),
        (
            ApplicationErrorCategory::NotFound,
            false,
            ErrorCode::NotFound,
            Code::NotFound,
        ),
        (
            ApplicationErrorCategory::AlreadyExists,
            false,
            ErrorCode::AlreadyExists,
            Code::AlreadyExists,
        ),
        (
            ApplicationErrorCategory::VersionConflict,
            true,
            ErrorCode::VersionConflict,
            Code::Aborted,
        ),
        (
            ApplicationErrorCategory::ConcurrencyConflict,
            true,
            ErrorCode::ConcurrencyConflict,
            Code::Aborted,
        ),
        (
            ApplicationErrorCategory::ImmutableViolation,
            false,
            ErrorCode::ImmutableViolation,
            Code::FailedPrecondition,
        ),
        (
            ApplicationErrorCategory::HashMismatch,
            false,
            ErrorCode::HashMismatch,
            Code::DataLoss,
        ),
        (
            ApplicationErrorCategory::LineageIncomplete,
            false,
            ErrorCode::LineageIncomplete,
            Code::FailedPrecondition,
        ),
        (
            ApplicationErrorCategory::StateConflict,
            true,
            ErrorCode::ImmutableViolation,
            Code::FailedPrecondition,
        ),
        (
            ApplicationErrorCategory::Unauthenticated,
            false,
            ErrorCode::Unauthenticated,
            Code::Unauthenticated,
        ),
        (
            ApplicationErrorCategory::Forbidden,
            false,
            ErrorCode::Forbidden,
            Code::PermissionDenied,
        ),
        (
            ApplicationErrorCategory::StorageUnavailable,
            true,
            ErrorCode::StorageUnavailable,
            Code::Unavailable,
        ),
    ];

    for (category, application_retryable, expected_core, expected_transport) in cases {
        let error = ApplicationError::new(category, application_retryable);
        let status = mapper.status("publish-signal", "application-port", &error);
        let detail = ErrorDetail::decode(status.details()).expect("protobuf ErrorDetail");

        assert_eq!(status.code(), expected_transport, "{category:?}");
        assert_eq!(detail.code, expected_core as i32, "{category:?}");
        assert_eq!(
            detail.retryable,
            application_retryable && category != ApplicationErrorCategory::StateConflict,
            "{category:?}"
        );
        assert_eq!(status.message(), detail.message, "{category:?}");
        assert!(detail.resource_ref.is_empty(), "{category:?}");
        assert!(detail.field_violations.is_empty(), "{category:?}");
    }
}

#[test]
fn invalid_value_chain_maps_to_validation_without_inventing_a_field() {
    let mapper = CoreBusinessErrorMapper::new(TRACE_KEY).expect("valid trace key");
    let error = map_domain_error(DomainErrorCode::InvalidValue);

    let status = mapper.status("record-value", "domain-error", &error);
    let detail = ErrorDetail::decode(status.details()).expect("protobuf ErrorDetail");

    assert_eq!(status.code(), Code::InvalidArgument);
    assert_eq!(detail.code, ErrorCode::ValidationFailed as i32);
    assert!(!detail.retryable);
    assert!(detail.field_violations.is_empty());
}

#[test]
fn trace_is_stable_nonempty_operation_scoped_and_does_not_leak_sources() {
    let mapper = CoreBusinessErrorMapper::new(TRACE_KEY).expect("valid trace key");
    let error = ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true);
    let unsafe_source = "postgres://admin:credential@db/raw_bucket sql stack raw-input";

    let first = mapper.status("read-signal", unsafe_source, &error);
    let repeated = mapper.status("read-signal", unsafe_source, &error);
    let other_operation = mapper.status("write-signal", unsafe_source, &error);
    let first_detail = ErrorDetail::decode(first.details()).expect("protobuf ErrorDetail");
    let repeated_detail = ErrorDetail::decode(repeated.details()).expect("protobuf ErrorDetail");
    let other_detail =
        ErrorDetail::decode(other_operation.details()).expect("protobuf ErrorDetail");

    assert!(!first_detail.trace_id.is_empty());
    assert_eq!(first_detail.trace_id, repeated_detail.trace_id);
    assert_ne!(first_detail.trace_id, other_detail.trace_id);

    let visible = format!(
        "{} {} {} {:?}",
        first.message(),
        first_detail.message,
        first_detail.resource_ref,
        first_detail.field_violations
    )
    .to_ascii_lowercase();
    for secret in [
        "postgres",
        "admin",
        "credential",
        "raw_bucket",
        "sql",
        "stack",
        "raw-input",
    ] {
        assert!(!visible.contains(secret), "leaked {secret}: {visible}");
    }
}

#[test]
fn empty_trace_key_is_rejected() {
    assert!(CoreBusinessErrorMapper::new(&[]).is_err());
    assert!(CoreBusinessErrorMapper::new(&[0; 31]).is_err());
}
