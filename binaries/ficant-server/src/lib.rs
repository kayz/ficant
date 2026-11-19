//! Single-process zero-dependency Ficant server entrypoint.
//!
//! Serves both native gRPC and browser `gRPC-Web` on one port without requiring
//! `PostgreSQL` or Ceph object storage.

#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::env;
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use ficant_api::RatesGrpcService;
use ficant_cgb_futures_pack::CgbFuturesDeliveryRulePackParser;
use ficant_contracts::ficant::rates::v1::rates_analytics_service_server::RatesAnalyticsServiceServer;
use ficant_fixed_income_native::{
    NativeBondAnalyticsEngine, NativeCarryRollEngine, NativeFuturesDeliveryEngine,
    NativeFuturesHedgeEngine, NativeYieldCurveEngine,
};
use ficant_funding_pack::FundingRulePackV1Parser;
use ficant_tax_pack::TaxRulePackV2Parser;
use tonic::transport::Server;

pub const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:50051";

pub fn build_rates_service() -> RatesGrpcService {
    RatesGrpcService::new(
        Arc::new(NativeBondAnalyticsEngine),
        Arc::new(NativeYieldCurveEngine),
        Arc::new(NativeCarryRollEngine),
        Arc::new(NativeFuturesDeliveryEngine),
        Arc::new(NativeFuturesHedgeEngine),
        Arc::new(CgbFuturesDeliveryRulePackParser),
        Arc::new(FundingRulePackV1Parser),
        Arc::new(TaxRulePackV2Parser),
    )
}

pub async fn serve_addr(addr: SocketAddr) -> Result<(), String> {
    let service = RatesAnalyticsServiceServer::new(build_rates_service());
    Server::builder()
        .accept_http1(true)
        .layer(tonic_web::GrpcWebLayer::new())
        .add_service(service)
        .serve(addr)
        .await
        .map_err(|err| format!("server failed: {err}"))
}

pub fn health_check_from_env() -> Result<(), String> {
    let addr = resolve_listen_addr()?;
    let probe = if addr.ip().is_unspecified() {
        SocketAddr::from(([127, 0, 0, 1], addr.port()))
    } else {
        addr
    };
    TcpStream::connect_timeout(&probe, Duration::from_secs(2))
        .map(|_| ())
        .map_err(|err| format!("health check failed for {probe}: {err}"))
}

pub async fn entry_from_env() -> Result<(), String> {
    serve_addr(resolve_listen_addr()?).await
}

fn resolve_listen_addr() -> Result<SocketAddr, String> {
    let raw = env::var("FICANT_SERVER_ADDR").unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.to_owned());
    raw.parse()
        .map_err(|err| format!("invalid FICANT_SERVER_ADDR '{raw}': {err}"))
}
