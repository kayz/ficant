use std::fs;
use std::path::PathBuf;

use ficant_contracts::ficant::portfolio::v1::{
    PortfolioCoverage, PortfolioPageDataMode, PortfolioPageEnvelope,
};

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
fn r8a_declares_the_single_additive_portfolio_package() {
    let proto = source("interface/proto/ficant/portfolio/v1/portfolio.proto");
    for required in [
        "package ficant.portfolio.v1;",
        "service PortfolioCatalogService",
        "rpc ListBooksAndPortfolios(ListBooksAndPortfoliosRequest) returns (ListBooksAndPortfoliosResponse);",
        "service PortfolioAggregationService",
        "rpc GetPortfolioOverview(GetPortfolioOverviewRequest) returns (GetPortfolioOverviewResponse);",
        "service PortfolioWorkbenchService",
        "rpc GetDefaultContext(GetDefaultContextRequest) returns (GetDefaultContextResponse);",
        "rpc GetPage(GetPortfolioPageRequest) returns (PortfolioPageEnvelope);",
        "string schema_version = 1;",
        "D01Projection d01 = 11;",
        "P04Projection p04 = 15;",
        "PortfolioWorkbenchTypedError typed_error = 16;",
        "ficant.core.v1.FormalOutputEvidence formal_evidence = 11;",
        "message PortfolioCoverage",
        "ficant.research.v1.CoverageDeclaration participation = 1;",
        "repeated string missing_reasons = 2;",
    ] {
        assert!(
            proto.contains(required),
            "Portfolio contract is missing {required}"
        );
    }

    assert_eq!(
        proto.matches("PortfolioCoverage coverage =").count(),
        3,
        "Overview, P03, and PageEnvelope must share the typed PortfolioCoverage"
    );

    for forbidden in [
        "PORTFOLIO_PAGE_DATA_MODE_DEMO",
        "double ",
        "float ",
        "Annualized",
        "Sharpe",
        "Calmar",
        "PageLayout",
        "ListWorkspaces",
        "ListPages",
        "rpc Execute",
    ] {
        assert!(
            !proto.contains(forbidden),
            "Portfolio P0 contract must not expose {forbidden}"
        );
    }
}

#[test]
fn r8a_public_enum_values_and_identity_kinds_are_append_only() {
    let evidence = source("interface/proto/ficant/core/v1/evidence.proto");
    for required in [
        "FORMAL_INPUT_KIND_PORTFOLIO = 16;",
        "FORMAL_INPUT_KIND_BOOK = 17;",
        "FORMAL_INPUT_KIND_PORTFOLIO_GROUP = 18;",
        "FORMAL_INPUT_KIND_BENCHMARK = 19;",
        "FORMAL_INPUT_KIND_PORTFOLIO_METRIC_CONVENTION = 20;",
        "FORMAL_INPUT_KIND_FACT = 21;",
    ] {
        assert!(
            evidence.contains(required),
            "formal input enum is missing {required}"
        );
    }

    let fact = source("interface/proto/ficant/market/v1/fact.proto");
    for required in [
        "enum ValuationValueRole",
        "VALUATION_VALUE_ROLE_UNSPECIFIED = 0;",
        "VALUATION_VALUE_ROLE_PRICE = 1;",
        "VALUATION_VALUE_ROLE_YIELD = 2;",
        "VALUATION_VALUE_ROLE_REMAINING_YEARS = 3;",
        "repeated ValuationValueRole value_roles = 10;",
    ] {
        assert!(
            fact.contains(required),
            "Valuation role contract is missing {required}"
        );
    }

    let proto = source("interface/proto/ficant/portfolio/v1/portfolio.proto");
    for required in [
        "PORTFOLIO_STATUS_ACTIVE = 1;",
        "PORTFOLIO_METRIC_WEIGHTING_MARKET_VALUE_TIMES_MODIFIED_DURATION = 2;",
        "PORTFOLIO_DECIMAL_ROUNDING_TIES_TO_EVEN = 1;",
        "PORTFOLIO_CURRENCY_MODE_CNY = 2;",
        "PORTFOLIO_LOOK_THROUGH_MODE_SEPARATE = 3;",
        "PORTFOLIO_PERIOD_PRESET_ONE_YEAR = 5;",
        "PORTFOLIO_WORKBENCH_PAGE_ID_P04 = 5;",
        "PORTFOLIO_PAGE_DATA_MODE_ERROR = 4;",
        "PORTFOLIO_WORKBENCH_ERROR_CODE_UNAVAILABLE = 7;",
    ] {
        assert!(
            proto.contains(required),
            "Portfolio enum is missing {required}"
        );
    }
}

#[test]
fn r8a_rust_transport_root_exposes_the_new_package() {
    let root = source("crates/ficant-contracts/src/lib.rs");
    assert!(
        root.contains("pub mod portfolio"),
        "generated Portfolio Rust contracts must be reachable as ficant::portfolio::v1"
    );
    assert!(
        root.contains("generated/ficant.portfolio.v1.rs"),
        "Portfolio prost output must be included"
    );
    assert!(
        root.contains("generated/ficant.portfolio.v1.tonic.rs"),
        "Portfolio tonic output must be included"
    );
}

#[test]
fn r8a_partial_and_real_modes_close_missing_reason_semantics() {
    let valid_partial = page_with_coverage(
        PortfolioPageDataMode::Partial,
        &["missing-duration", "short-position"],
    );
    assert!(validate_page_coverage(&valid_partial).is_ok());

    let empty_partial = page_with_coverage(PortfolioPageDataMode::Partial, &[]);
    assert_eq!(
        validate_page_coverage(&empty_partial),
        Err("PARTIAL requires at least one missing reason")
    );

    let invalid_real = page_with_coverage(PortfolioPageDataMode::Real, &["missing-duration"]);
    assert_eq!(
        validate_page_coverage(&invalid_real),
        Err("REAL forbids missing reasons")
    );

    let unsorted_partial = page_with_coverage(
        PortfolioPageDataMode::Partial,
        &["short-position", "missing-duration"],
    );
    assert_eq!(
        validate_page_coverage(&unsorted_partial),
        Err("missing reasons must be nonblank, sorted, and unique")
    );
}

fn page_with_coverage(
    mode: PortfolioPageDataMode,
    missing_reasons: &[&str],
) -> PortfolioPageEnvelope {
    PortfolioPageEnvelope {
        data_mode: mode as i32,
        coverage: Some(PortfolioCoverage {
            participation: None,
            missing_reasons: missing_reasons
                .iter()
                .map(|reason| (*reason).to_owned())
                .collect(),
        }),
        ..PortfolioPageEnvelope::default()
    }
}

fn validate_page_coverage(page: &PortfolioPageEnvelope) -> Result<(), &'static str> {
    let coverage = page.coverage.as_ref().ok_or("coverage is required")?;
    let reasons = &coverage.missing_reasons;
    if reasons
        .iter()
        .any(|reason| reason.is_empty() || reason != reason.trim())
        || reasons.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err("missing reasons must be nonblank, sorted, and unique");
    }
    match PortfolioPageDataMode::try_from(page.data_mode).ok() {
        Some(PortfolioPageDataMode::Partial) if reasons.is_empty() => {
            Err("PARTIAL requires at least one missing reason")
        }
        Some(PortfolioPageDataMode::Real) if !reasons.is_empty() => {
            Err("REAL forbids missing reasons")
        }
        _ => Ok(()),
    }
}
