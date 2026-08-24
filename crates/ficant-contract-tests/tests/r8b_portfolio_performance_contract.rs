use std::fs;
use std::path::PathBuf;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("contract-test crate must live under the repository crates directory")
        .to_path_buf()
}

fn source(relative: &str) -> String {
    fs::read_to_string(repository_root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

#[test]
fn r8b_declares_exact_daily_performance_contract() {
    let proto = source("interface/proto/ficant/portfolio/v1/portfolio.proto");
    for required in [
        "message PortfolioPerformanceConventionRef",
        "message PortfolioPerformanceConvention",
        "message PortfolioValuationSnapshot",
        "message BenchmarkLevelSnapshot",
        "message PortfolioDailyPerformancePoint",
        "message PortfolioPerformanceCoverage",
        "message PortfolioPerformanceSeries",
        "message GetPortfolioPerformanceRequest",
        "message GetPortfolioPerformanceResponse",
        "service PortfolioPerformanceService",
        "rpc GetPortfolioPerformance(GetPortfolioPerformanceRequest) returns (GetPortfolioPerformanceResponse);",
        "ficant.core.v1.FormalOutputEvidence formal_evidence = 10;",
    ] {
        assert!(
            proto.contains(required),
            "R8B contract is missing {required}"
        );
    }

    for forbidden in [
        "double ",
        "float ",
        "Annualized",
        "Sharpe",
        "Calmar",
        "Drawdown",
        "Campisi",
        "ValueAtRisk",
        "rpc PublishPortfolioValuation",
    ] {
        assert!(
            !proto.contains(forbidden),
            "R8B contract must not expose {forbidden}"
        );
    }
}

#[test]
fn r8b_formal_input_kinds_are_append_only() {
    let evidence = source("interface/proto/ficant/core/v1/evidence.proto");
    for required in [
        "FORMAL_INPUT_KIND_PORTFOLIO_VALUATION_SNAPSHOT = 22;",
        "FORMAL_INPUT_KIND_BENCHMARK_LEVEL_SNAPSHOT = 23;",
        "FORMAL_INPUT_KIND_PORTFOLIO_PERFORMANCE_CONVENTION = 24;",
    ] {
        assert!(
            evidence.contains(required),
            "R8B evidence is missing {required}"
        );
    }
}
