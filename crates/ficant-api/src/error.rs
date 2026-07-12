use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::app::v1::{ErrorCode, SafeError};
use ring::hmac;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformFailureCode {
    Unauthenticated,
    Forbidden,
    NotFound,
    InvalidRequest,
    Expired,
    Unavailable,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformFailure {
    code: PlatformFailureCode,
    retryable: bool,
    trace_context: &'static str,
}

impl PlatformFailure {
    #[must_use]
    pub const fn new(
        code: PlatformFailureCode,
        retryable: bool,
        trace_context: &'static str,
    ) -> Self {
        Self {
            code,
            retryable,
            trace_context,
        }
    }

    #[must_use]
    pub const fn code(&self) -> PlatformFailureCode {
        self.code
    }
}

#[derive(Clone)]
pub struct SafeErrorMapper {
    key: hmac::Key,
}

impl SafeErrorMapper {
    /// Creates a mapper backed by a trace-signing key.
    ///
    /// # Errors
    ///
    /// Returns an error when the key contains fewer than 32 bytes.
    pub fn new(key: &[u8]) -> Result<Self, &'static str> {
        if key.len() < 32 {
            return Err("trace key must contain at least 32 bytes");
        }
        Ok(Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, key),
        })
    }

    #[must_use]
    pub fn map(&self, operation: &'static str, failure: &PlatformFailure) -> SafeError {
        let external = external_code(failure.code);
        let input = format!("{operation}:{}:{}", external as i32, failure.trace_context);
        let tag = hmac::sign(&self.key, input.as_bytes());
        SafeError {
            code: external as i32,
            safe_message: safe_message(failure.code).to_owned(),
            trace_id: format_trace_id(tag.as_ref()),
            retryable: failure.retryable,
        }
    }

    /// Maps the frozen application failure taxonomy without exposing internal details.
    #[must_use]
    pub fn map_application(&self, operation: &'static str, error: &ApplicationError) -> SafeError {
        let code = match error.category() {
            ApplicationErrorCategory::Unauthenticated => PlatformFailureCode::Unauthenticated,
            ApplicationErrorCategory::Forbidden => PlatformFailureCode::Forbidden,
            ApplicationErrorCategory::NotFound => PlatformFailureCode::NotFound,
            ApplicationErrorCategory::StorageUnavailable => PlatformFailureCode::Unavailable,
            ApplicationErrorCategory::ValidationFailed
            | ApplicationErrorCategory::AlreadyExists
            | ApplicationErrorCategory::VersionConflict
            | ApplicationErrorCategory::ConcurrencyConflict
            | ApplicationErrorCategory::ImmutableViolation
            | ApplicationErrorCategory::HashMismatch
            | ApplicationErrorCategory::LineageIncomplete
            | ApplicationErrorCategory::StateConflict => PlatformFailureCode::InvalidRequest,
        };
        self.map(
            operation,
            &PlatformFailure::new(code, error.retryable(), "application-port"),
        )
    }
}

const fn external_code(code: PlatformFailureCode) -> ErrorCode {
    match code {
        PlatformFailureCode::Unauthenticated => ErrorCode::Unauthenticated,
        PlatformFailureCode::Forbidden => ErrorCode::Forbidden,
        PlatformFailureCode::NotFound => ErrorCode::NotFound,
        PlatformFailureCode::InvalidRequest => ErrorCode::InvalidRequest,
        PlatformFailureCode::Expired => ErrorCode::Expired,
        PlatformFailureCode::Unavailable => ErrorCode::Unavailable,
        PlatformFailureCode::Internal => ErrorCode::Internal,
    }
}

const fn safe_message(code: PlatformFailureCode) -> &'static str {
    match code {
        PlatformFailureCode::Unauthenticated => "当前身份未通过认证",
        PlatformFailureCode::Forbidden => "当前身份无权执行此操作",
        PlatformFailureCode::NotFound => "请求的资源不存在",
        PlatformFailureCode::InvalidRequest => "请求内容无效",
        PlatformFailureCode::Expired => "当前会话或授权已过期",
        PlatformFailureCode::Unavailable => "平台服务暂时不可用",
        PlatformFailureCode::Internal => "平台无法完成当前请求",
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
