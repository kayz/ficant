//! Frozen platform transport boundary for the Phase 1 application shell.

mod core_error;
mod error;
mod grpc_web;
mod rates;
mod registry;
mod session;

pub use core_error::CoreBusinessErrorMapper;
pub use error::{PlatformFailure, PlatformFailureCode, SafeErrorMapper};
pub use grpc_web::{
    GrpcWebServeError, GrpcWebServerConfig, PlatformGrpcService, serve_grpc_web,
    serve_grpc_web_with_rates,
};
pub use rates::RatesGrpcService;
pub use registry::{AppRegistration, CspPolicy, PlatformApplication, PlatformPort};
pub use session::{Clock, SessionPolicy, SystemClock, TrustedIdentity};
