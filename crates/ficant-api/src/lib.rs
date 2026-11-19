//! Stateless transport layer for the core onshore rates pricing service.

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod error;
pub mod rates;

pub use error::to_error_detail;
pub use rates::RatesGrpcService;
