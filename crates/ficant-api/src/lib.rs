//! Frozen platform transport boundary for the Phase 1 application shell.

mod canonical_snapshot;
mod core_error;
mod curve_points;
mod error;
mod experiment;
mod factor_registry;
mod grpc_web;
mod portfolio_risk;
mod position_snapshot;
mod rates;
mod registry;
mod session;
mod subject_registry;

pub use canonical_snapshot::CanonicalSnapshotCodecAdapter;
pub use core_error::CoreBusinessErrorMapper;
pub use curve_points::CanonicalCurvePointSetDecoder;
pub use error::{PlatformFailure, PlatformFailureCode, SafeErrorMapper};
pub use experiment::{ExperimentGrpcService, TrustedExperimentScope, TrustedNodeCatalog};
pub use factor_registry::FactorRegistryGrpcService;
pub use grpc_web::{
    GrpcWebServeError, GrpcWebServerConfig, PlatformGrpcService, serve_grpc_web,
    serve_grpc_web_with_rates, serve_grpc_web_with_rates_and_experiment,
    serve_grpc_web_with_rates_and_experiment_and_registry,
    serve_grpc_web_with_rates_and_experiment_and_registry_and_positions,
    serve_grpc_web_with_rates_and_experiment_and_registry_and_positions_and_factors_and_portfolio_risk,
};
pub use portfolio_risk::PortfolioRiskGrpcService;
pub use position_snapshot::PositionSnapshotGrpcService;
pub use rates::{
    ParsedBondAnalyticsRequest, RatesGrpcService, analyze_bond_request,
    execute_parsed_bond_request, parse_analyze_bond_request,
};
pub use registry::{AppRegistration, CspPolicy, PlatformApplication, PlatformPort};
pub use session::{Clock, SessionPolicy, SystemClock, TrustedIdentity};
pub use subject_registry::SubjectRegistryGrpcService;
