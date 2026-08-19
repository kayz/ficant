//! Frozen platform transport boundary for the Phase 1 application shell.

mod artifact;
mod canonical_snapshot;
mod core_error;
mod curve_points;
mod data_health;
mod data_source_registry;
mod error;
mod experiment;
mod factor_registry;
mod formal_evidence;
mod governance;
mod grpc_web;
mod market_definition;
mod market_fact;
mod portfolio_risk;
mod position_snapshot;
mod rates;
mod registry;
mod session;
mod snapshot;
mod subject_registry;

pub use artifact::ArtifactGrpcService;
pub use canonical_snapshot::CanonicalSnapshotCodecAdapter;
pub use core_error::CoreBusinessErrorMapper;
pub use curve_points::CanonicalCurvePointSetDecoder;
pub use data_health::DataHealthGrpcService;
pub use data_source_registry::DataSourceRegistryGrpcService;
pub use error::{PlatformFailure, PlatformFailureCode, SafeErrorMapper};
pub use experiment::{ExperimentGrpcService, TrustedExperimentScope, TrustedNodeCatalog};
pub use factor_registry::FactorRegistryGrpcService;
pub use formal_evidence::FormalOutputPublisher;
pub use governance::FoundationChangeGrpcService;
pub use grpc_web::{
    GrpcWebServeError, GrpcWebServerConfig, PlatformGrpcService, ProductionGrpcServices,
    ProductionRouteSet, build_production_routes, serve_grpc_web_routes, serve_production_routes,
};
pub use market_definition::MarketDefinitionGrpcService;
pub use market_fact::MarketFactGrpcService;
pub use portfolio_risk::PortfolioRiskGrpcService;
pub use position_snapshot::PositionSnapshotGrpcService;
pub use rates::{
    ParsedBondAnalyticsRequest, RatesGrpcService, analyze_bond_request,
    execute_parsed_bond_request, parse_analyze_bond_request,
};
pub use registry::{AppRegistration, CspPolicy, PlatformApplication, PlatformPort};
pub use session::{Clock, SessionPolicy, SystemClock, TrustedIdentity};
pub use snapshot::SnapshotGrpcService;
pub use subject_registry::SubjectRegistryGrpcService;
