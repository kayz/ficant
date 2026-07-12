use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::core::v1::{ErrorCode, ErrorDetail};
use prost::Message;
use ring::hmac;
use tonic::{Code, Status};

/// Maps application failures to the lossless core business error contract.
///
/// This mapper is deliberately independent from [`crate::SafeErrorMapper`]. The
/// platform error contract is coarser and cannot preserve the core taxonomy.
#[derive(Clone)]
pub struct CoreBusinessErrorMapper {
    trace_key: hmac::Key,
}

impl CoreBusinessErrorMapper {
    /// Creates a core business error mapper backed by a trace-signing key.
    ///
    /// # Errors
    ///
    /// Returns an error when the key contains fewer than 32 bytes.
    pub fn new(trace_key: &[u8]) -> Result<Self, &'static str> {
        if trace_key.len() < 32 {
            return Err("trace key must contain at least 32 bytes");
        }
        Ok(Self {
            trace_key: hmac::Key::new(hmac::HMAC_SHA256, trace_key),
        })
    }

    /// Builds the client-safe core detail for an application failure.
    ///
    /// `operation` and `trace_source` are used only as HMAC input. Neither value
    /// is copied into the client-visible message or protobuf fields.
    #[must_use]
    pub fn map(
        &self,
        operation: &str,
        trace_source: &str,
        error: &ApplicationError,
    ) -> ErrorDetail {
        let mapping = mapping(error.category());
        let trace_input = format!(
            "ficant.core.v1.ErrorDetail\0{}:{operation}\0{}:{trace_source}\0{}:{}:{}",
            operation.len(),
            trace_source.len(),
            mapping.core_code as i32,
            mapping.transport_code as i32,
            error.retryable()
        );
        let trace = hmac::sign(&self.trace_key, trace_input.as_bytes());

        ErrorDetail {
            code: mapping.core_code as i32,
            message: mapping.safe_message.to_owned(),
            trace_id: format_trace_id(trace.as_ref()),
            retryable: error.retryable() && !mapping.force_non_retryable,
            resource_ref: String::new(),
            field_violations: Vec::new(),
        }
    }

    /// Builds a tonic status whose details bytes contain one core `ErrorDetail`.
    #[must_use]
    pub fn status(&self, operation: &str, trace_source: &str, error: &ApplicationError) -> Status {
        let mapping = mapping(error.category());
        let detail = self.map(operation, trace_source, error);
        Status::with_details(
            mapping.transport_code,
            detail.message.clone(),
            detail.encode_to_vec().into(),
        )
    }
}

#[derive(Clone, Copy)]
struct BusinessErrorMapping {
    core_code: ErrorCode,
    transport_code: Code,
    safe_message: &'static str,
    force_non_retryable: bool,
}

const fn mapping(category: ApplicationErrorCategory) -> BusinessErrorMapping {
    match category {
        ApplicationErrorCategory::ValidationFailed => BusinessErrorMapping {
            core_code: ErrorCode::ValidationFailed,
            transport_code: Code::InvalidArgument,
            safe_message: "请求内容无效",
            force_non_retryable: false,
        },
        ApplicationErrorCategory::NotFound => BusinessErrorMapping {
            core_code: ErrorCode::NotFound,
            transport_code: Code::NotFound,
            safe_message: "请求的业务资源不存在",
            force_non_retryable: false,
        },
        ApplicationErrorCategory::AlreadyExists => BusinessErrorMapping {
            core_code: ErrorCode::AlreadyExists,
            transport_code: Code::AlreadyExists,
            safe_message: "业务资源已存在",
            force_non_retryable: false,
        },
        ApplicationErrorCategory::VersionConflict => BusinessErrorMapping {
            core_code: ErrorCode::VersionConflict,
            transport_code: Code::Aborted,
            safe_message: "业务资源版本已发生变化",
            force_non_retryable: false,
        },
        ApplicationErrorCategory::ConcurrencyConflict => BusinessErrorMapping {
            core_code: ErrorCode::ConcurrencyConflict,
            transport_code: Code::Aborted,
            safe_message: "并发操作发生冲突",
            force_non_retryable: false,
        },
        ApplicationErrorCategory::ImmutableViolation => BusinessErrorMapping {
            core_code: ErrorCode::ImmutableViolation,
            transport_code: Code::FailedPrecondition,
            safe_message: "不可变业务数据不能修改",
            force_non_retryable: false,
        },
        ApplicationErrorCategory::HashMismatch => BusinessErrorMapping {
            core_code: ErrorCode::HashMismatch,
            transport_code: Code::DataLoss,
            safe_message: "业务数据完整性校验失败",
            force_non_retryable: false,
        },
        ApplicationErrorCategory::LineageIncomplete => BusinessErrorMapping {
            core_code: ErrorCode::LineageIncomplete,
            transport_code: Code::FailedPrecondition,
            safe_message: "业务数据血缘不完整",
            force_non_retryable: false,
        },
        ApplicationErrorCategory::StateConflict => BusinessErrorMapping {
            core_code: ErrorCode::ImmutableViolation,
            transport_code: Code::FailedPrecondition,
            safe_message: "当前业务状态不允许执行此操作",
            force_non_retryable: true,
        },
        ApplicationErrorCategory::Unauthenticated => BusinessErrorMapping {
            core_code: ErrorCode::Unauthenticated,
            transport_code: Code::Unauthenticated,
            safe_message: "当前身份未通过认证",
            force_non_retryable: false,
        },
        ApplicationErrorCategory::Forbidden => BusinessErrorMapping {
            core_code: ErrorCode::Forbidden,
            transport_code: Code::PermissionDenied,
            safe_message: "当前身份无权执行此操作",
            force_non_retryable: false,
        },
        ApplicationErrorCategory::StorageUnavailable => BusinessErrorMapping {
            core_code: ErrorCode::StorageUnavailable,
            transport_code: Code::Unavailable,
            safe_message: "业务存储暂时不可用",
            force_non_retryable: false,
        },
    }
}

fn format_trace_id(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(32);
    for byte in &bytes[..16] {
        use std::fmt::Write;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}
