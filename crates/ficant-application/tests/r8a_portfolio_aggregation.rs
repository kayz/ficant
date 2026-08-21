use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, AuthorizedPrincipal, ExactPortfolioScope,
    ExactPortfolioScopeKind, NormalizedPortfolioContext, NormalizedPortfolioContextResolution,
    PortfolioAnalyticsEvidenceBinding, PortfolioAnalyticsEvidenceKind, PortfolioBondRatesAuthority,
    PortfolioBondRatesAuthorityResolution, PortfolioCatalogEvidenceBinding,
    PortfolioCatalogEvidenceRole, PortfolioCurrencyMode, PortfolioImmutableSnapshotAuthority,
    PortfolioLookThroughMode, PortfolioPeriodPreset, PortfolioRatesUnitAuthority,
    PortfolioRatesUnitRole, PortfolioRiskAuthority, PortfolioValuationAuthorityBinding,
    PositionSnapshotRepository, ResolvedPortfolioAggregationInputs,
    ResolvedPortfolioAnalyticsAuthority, SubjectRepository, VisibleCatalogRecord,
};
use ficant_application::use_cases::portfolio_aggregation::{
    OwnedPortfolioOverviewRecordFactory, PortfolioAggregationAuthority,
    PortfolioAggregationUseCase, PortfolioAnalyticsAuthorityHandoff, PortfolioBondAnalysis,
    PortfolioBondAnalysisHandoff, PortfolioBondAnalysisResult, PortfolioBondMetricFacts,
    PortfolioCoverageReason, PortfolioFormalExecutionBinding, PortfolioMetricDataMode,
    PortfolioMetricOutputScales, PortfolioMetricPosition, PortfolioOverview,
    PortfolioOverviewDraft, PortfolioOverviewPublisher, PortfolioOverviewRecordFactory,
    PortfolioPositionViewsHandoff, PortfolioRatesUnitBindings,
    PortfolioRatesUnitRole as ResultUnitRole, PortfolioRiskAnalysis, PortfolioRiskHandoff,
    PortfolioRiskNamedEvidenceBinding, PortfolioRiskNamedEvidenceKind,
    PortfolioWeightedMetricEligibility, PortfolioWeightedMetricUnits, aggregate_portfolio_metrics,
};
use ficant_application::use_cases::position_views::{
    PositionViews, project_verified_position_views,
};
use ficant_application::use_cases::rates_materialization::{
    RatesEvidenceBinding, RatesInputEvidence, RatesInputRole, RatesRequestEvidence,
    RatesUnitRequirement,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::analytics::{
    AnalyticsMeasures, AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondAnalyticsResult,
    BondTerms, BusinessDayConvention, CalendarBinding, CalendarRequirement, CalendarResolution,
    CouponFrequency, DayCountConvention, DerivedCashflow,
};
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::PriceSourceType;
use ficant_domain::portfolio::{
    Benchmark, BenchmarkInput, BenchmarkRef, Portfolio, PortfolioDecimalRounding, PortfolioInput,
    PortfolioMetricConvention, PortfolioMetricConventionInput, PortfolioMetricConventionRef,
    PortfolioMetricWeighting, PortfolioSnapshotBinding, PortfolioStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, FixedDecimal, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{
    AccountingClassification, AccountingClassificationState, CoverageDeclaration, FactorDv01,
    PortfolioKeyRateExposure, Position, PositionHoldingForm, PositionInput,
    PositionKeyRateExposure, PositionSnapshot, PositionSnapshotInput, PriceSourceCount,
    PriceSourceSummary, RiskAlgorithmBinding,
};
use ficant_domain::subject::{
    AccessSet, FundingTier, Subject, SubjectRecord, SubjectStateSnapshot, SubjectVersion,
    TaxTreatment,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};
use ficant_runtime::{
    CodeBinding, FormalImplementationBinding, FormalInputBinding, FormalInputBindingInput,
    FormalInputKind, FormalInputReference, FormalOutputEvidence, FormalOutputEvidenceInput,
    RuntimeBinding,
};
use serde_json::Value;

const INPUTS: &str =
    include_str!("../../../tests/oracle/portfolio360/r8a_portfolio_metric_inputs.json");
const EXPECTED: &str =
    include_str!("../../../tests/oracle/portfolio360/r8a_portfolio_metric_expected.json");

#[test]
fn production_aggregation_matches_the_frozen_independent_decimal_expected() {
    let inputs: Value = serde_json::from_str(INPUTS).expect("oracle inputs JSON");
    let expected: Value = serde_json::from_str(EXPECTED).expect("oracle expected JSON");
    let output_scale =
        u32::try_from(required_u64(&inputs, "output_scale")).expect("fixture output scale");

    for (index, portfolio) in required_array(&inputs, "portfolios").iter().enumerate() {
        let positions = positions_for(&inputs, required_array(portfolio, "positions"), index * 4);
        let aggregate_id = required_str(portfolio, "portfolio_id");
        let expected_aggregate = required_array(&expected, "portfolios")
            .iter()
            .find(|candidate| required_str(candidate, "aggregate_id") == aggregate_id)
            .expect("portfolio expected");
        assert_aggregate(
            &positions,
            expected_aggregate,
            output_scale,
            PortfolioMetricDataMode::Real,
        );
    }

    let mut scope_positions = required_array(&inputs, "portfolios")
        .iter()
        .enumerate()
        .flat_map(|(index, portfolio)| {
            positions_for(&inputs, required_array(portfolio, "positions"), index * 4)
        })
        .collect::<Vec<_>>();
    scope_positions.sort_by(|left, right| left.position_id().cmp(right.position_id()));
    assert_aggregate(
        &scope_positions,
        required_object(&expected, "scope"),
        output_scale,
        PortfolioMetricDataMode::Real,
    );

    let benchmark = required_object(&inputs, "benchmark");
    let benchmark_positions = positions_for(&inputs, required_array(benchmark, "positions"), 12);
    assert_aggregate(
        &benchmark_positions,
        required_object(&expected, "benchmark"),
        output_scale,
        PortfolioMetricDataMode::Real,
    );
}

#[test]
fn inverse_positions_preserve_signed_facts_and_omit_every_weighted_average() {
    let inputs: Value = serde_json::from_str(INPUTS).expect("oracle inputs JSON");
    let expected: Value = serde_json::from_str(EXPECTED).expect("oracle expected JSON");
    let mut positions = required_array(&inputs, "portfolios")
        .iter()
        .enumerate()
        .flat_map(|(index, portfolio)| {
            inverse_positions_for(&inputs, required_array(portfolio, "positions"), index * 4)
        })
        .collect::<Vec<_>>();
    positions.sort_by(|left, right| left.position_id().cmp(right.position_id()));
    let inverse = required_object(
        required_object(&expected, "metamorphic_witness"),
        "inverse_scope",
    );
    let result = aggregate_portfolio_metrics(
        &positions,
        &krd_from_expected(inverse),
        scales(u32::try_from(required_u64(&inputs, "output_scale")).expect("scale")),
    )
    .expect("inverse aggregate");

    assert_eq!(result.data_mode(), PortfolioMetricDataMode::Partial);
    assert_eq!(
        result.coverage().missing_reasons(),
        &[PortfolioCoverageReason::ShortPositionExcludedFromWeightedAverages]
    );
    let metrics = result.basic_metrics();
    assert!(metrics.weighted_ytm().is_none());
    assert!(metrics.modified_duration().is_none());
    assert!(metrics.convexity().is_none());
    assert!(metrics.weighted_coupon_rate().is_none());
    assert!(metrics.weighted_remaining_years().is_none());
    assert_eq!(
        render(metrics.market_value(), 6),
        required_str(required_object(inverse, "basic_metrics"), "market_value")
    );
    assert_eq!(
        render(metrics.economic_pnl(), 6),
        required_str(required_object(inverse, "basic_metrics"), "economic_pnl")
    );
    assert_eq!(
        render(metrics.dv01(), 6),
        required_str(required_object(inverse, "basic_metrics"), "dv01")
    );
}

#[test]
fn final_rounding_is_ties_to_even_and_never_uses_epsilon() {
    let metrics = PortfolioBondMetricFacts::new(
        fixed("2.355"),
        fixed("1"),
        fixed("1"),
        fixed("0.01"),
        fixed("1"),
        weighted_units(),
    )
    .expect("metrics");
    let position = PortfolioMetricPosition::from_totals(
        id(2),
        decimal("2.345", money_unit()),
        decimal("-2.345", money_unit()),
        fixed("1"),
        PortfolioWeightedMetricEligibility::Bond(Box::new(metrics)),
    )
    .expect("position");
    let result = aggregate_portfolio_metrics(&[position], &krd("0.005"), scales(2))
        .expect("ties-even aggregate");

    assert_eq!(render(result.basic_metrics().market_value(), 2), "2.34");
    assert_eq!(render(result.basic_metrics().economic_pnl(), 2), "-2.34");
    assert_eq!(
        render(result.basic_metrics().weighted_ytm().expect("ytm"), 2),
        "2.36"
    );
}

#[test]
fn production_non_divisible_weights_are_accumulated_until_the_final_unit_scale() {
    let inputs = [
        (
            50,
            101_230_000_000_000_i128,
            100_000_000_000_000_i128,
            18_242_283_105_i128,
            3_271_416_794_250_i128,
            14_234_453_531_166_i128,
            26_800_000_000_i128,
            3_482_000_000_000_i128,
            500_000_000_000_i128,
        ),
        (
            51,
            79_640_000_000_000_i128,
            80_000_000_000_000_i128,
            19_167_980_296_i128,
            5_263_324_351_546_i128,
            34_075_105_431_199_i128,
            27_600_000_000_i128,
            5_739_000_000_000_i128,
            440_000_000_000_i128,
        ),
        (
            52,
            119_040_000_000_000_i128,
            120_000_000_000_000_i128,
            20_182_758_621_i128,
            7_752_665_535_280_i128,
            70_906_554_273_366_i128,
            19_300_000_000_i128,
            8_564_000_000_000_i128,
            200_000_000_000_i128,
        ),
    ];
    let positions = inputs
        .into_iter()
        .map(
            |(
                position_id,
                market_value,
                notional,
                ytm,
                duration,
                convexity,
                coupon,
                remaining,
                expected_remainder,
            )| {
                assert_eq!(
                    market_value.checked_mul(duration).expect("raw product") % 1_000_000_000_000,
                    expected_remainder,
                    "the production coefficient must exercise a non-divisible Decimal product"
                );
                let metrics = PortfolioBondMetricFacts::new(
                    FixedDecimal::from_scaled(ytm),
                    FixedDecimal::from_scaled(duration),
                    FixedDecimal::from_scaled(convexity),
                    FixedDecimal::from_scaled(coupon),
                    FixedDecimal::from_scaled(remaining),
                    weighted_units(),
                )
                .expect("production metrics");
                PortfolioMetricPosition::from_totals(
                    id(position_id),
                    DecimalValue::new(market_value.to_string(), 12, money_unit())
                        .expect("production market value"),
                    decimal("0", money_unit()),
                    FixedDecimal::from_scaled(notional),
                    PortfolioWeightedMetricEligibility::Bond(Box::new(metrics)),
                )
                .expect("production position")
            },
        )
        .collect::<Vec<_>>();

    let result = aggregate_portfolio_metrics(&positions, &krd("0.01"), scales(6))
        .expect("non-divisible products remain exact until final rounding");
    let metrics = result.basic_metrics();
    assert_eq!(render(metrics.weighted_ytm().expect("ytm"), 6), "0.019544");
    assert_eq!(
        render(metrics.modified_duration().expect("duration"), 6),
        "5.579054"
    );
    assert_eq!(
        render(metrics.convexity().expect("convexity"), 6),
        "41.997304"
    );
    assert_eq!(
        render(metrics.weighted_coupon_rate().expect("coupon"), 6),
        "0.024013"
    );
    assert_eq!(
        render(metrics.weighted_remaining_years().expect("remaining"), 6),
        "6.116667"
    );
}

#[test]
fn weighted_average_rounds_once_at_the_final_unit_scale() {
    let first_metrics = PortfolioBondMetricFacts::new(
        fixed("1"),
        fixed("0.000001499998"),
        fixed("1"),
        fixed("1"),
        fixed("1"),
        weighted_units(),
    )
    .expect("first metrics");
    let second_metrics = PortfolioBondMetricFacts::new(
        fixed("1"),
        fixed("0.000001500000"),
        fixed("1"),
        fixed("1"),
        fixed("1"),
        weighted_units(),
    )
    .expect("second metrics");
    let positions = vec![
        PortfolioMetricPosition::from_totals(
            id(53),
            decimal("1", money_unit()),
            decimal("0", money_unit()),
            fixed("1"),
            PortfolioWeightedMetricEligibility::Bond(Box::new(first_metrics)),
        )
        .expect("first position"),
        PortfolioMetricPosition::from_totals(
            id(54),
            decimal("4", money_unit()),
            decimal("0", money_unit()),
            fixed("4"),
            PortfolioWeightedMetricEligibility::Bond(Box::new(second_metrics)),
        )
        .expect("second position"),
    ];

    // The exact weighted duration is 0.0000014999996. Direct scale-6 rounding is 0.000001;
    // rounding first to FixedDecimal's scale 12 produces 0.000001500000 and would incorrectly
    // round the second time to 0.000002.
    let result = aggregate_portfolio_metrics(&positions, &krd("0.01"), scales(6))
        .expect("single final rounding");
    assert_eq!(
        render(
            result
                .basic_metrics()
                .modified_duration()
                .expect("duration"),
            6,
        ),
        "0.000001"
    );
}

#[test]
fn every_metric_uses_its_own_exact_unit_scale() {
    let metrics = PortfolioBondMetricFacts::new(
        fixed("0.0123"),
        fixed("2.3456"),
        fixed("7.25"),
        fixed("0.04555"),
        fixed("5.5555"),
        weighted_units(),
    )
    .expect("weighted facts");
    let position = PortfolioMetricPosition::from_totals(
        id(43),
        decimal("2.345", money_unit()),
        decimal("-2.355", money_unit()),
        fixed("100"),
        PortfolioWeightedMetricEligibility::Bond(Box::new(metrics)),
    )
    .expect("position");
    let result = aggregate_portfolio_metrics(
        &[position],
        &krd("0.123456"),
        PortfolioMetricOutputScales::new(2, 4, 3, 1, 5).expect("different scales"),
    )
    .expect("scaled aggregate");
    let values = result.basic_metrics();
    assert_eq!(render(values.market_value(), 2), "2.34");
    assert_eq!(render(values.economic_pnl(), 2), "-2.36");
    assert_eq!(render(values.weighted_ytm().expect("ytm"), 4), "0.0123");
    assert_eq!(
        render(values.modified_duration().expect("duration"), 3),
        "2.346"
    );
    assert_eq!(render(values.convexity().expect("convexity"), 1), "7.2");
    assert_eq!(
        render(values.weighted_coupon_rate().expect("coupon"), 4),
        "0.0456"
    );
    assert_eq!(
        render(values.weighted_remaining_years().expect("remaining"), 3),
        "5.556"
    );
    assert_eq!(render(values.dv01(), 5), "0.12346");
}

#[test]
fn mixed_units_and_missing_or_zero_weight_authority_fail_closed() {
    let metrics = PortfolioBondMetricFacts::new(
        fixed("0.02"),
        fixed("2"),
        fixed("5"),
        fixed("0.02"),
        fixed("2"),
        weighted_units(),
    )
    .expect("metrics");
    let first = PortfolioMetricPosition::from_totals(
        id(2),
        decimal("100", money_unit()),
        decimal("1", money_unit()),
        fixed("100"),
        PortfolioWeightedMetricEligibility::Bond(Box::new(metrics.clone())),
    )
    .expect("first");
    let mixed = PortfolioMetricPosition::from_totals(
        id(3),
        decimal("100", alternate_money_unit()),
        decimal("1", alternate_money_unit()),
        fixed("100"),
        PortfolioWeightedMetricEligibility::Bond(Box::new(metrics)),
    )
    .expect("mixed position");
    assert!(aggregate_portfolio_metrics(&[first, mixed], &krd("0.01"), scales(6)).is_err());

    assert!(
        PortfolioBondMetricFacts::new(
            fixed("0.02"),
            FixedDecimal::ZERO,
            fixed("5"),
            fixed("0.02"),
            fixed("2"),
            weighted_units(),
        )
        .is_err()
    );
    assert!(aggregate_portfolio_metrics(&[], &krd("0.01"), scales(6)).is_err());
}

#[test]
fn risk_actual_input_set_rejects_missing_extra_duplicate_and_hash_drift() {
    let snapshot = aggregation_snapshot(id(20), id(22), "100");
    let exposure = risk_for(&snapshot);
    let factor = PortfolioRiskNamedEvidenceBinding::immutable_definition(
        PortfolioRiskNamedEvidenceKind::FactorDefinition,
        "cn.gov.yield.02y",
        ContentHash::digest(b"cn.gov.yield.02y"),
    )
    .expect("factor evidence");
    let node = PortfolioRiskNamedEvidenceBinding::immutable_definition(
        PortfolioRiskNamedEvidenceKind::CurveNodeDefinition,
        "cn.gov.curve.02y",
        ContentHash::digest(b"cn.gov.curve.02y"),
    )
    .expect("node evidence");
    assert!(PortfolioRiskAnalysis::new(exposure.clone(), vec![factor.clone()]).is_err());
    assert!(
        PortfolioRiskAnalysis::new(
            exposure.clone(),
            vec![factor.clone(), node.clone(), node.clone()]
        )
        .is_err()
    );
    let extra = PortfolioRiskNamedEvidenceBinding::immutable_definition(
        PortfolioRiskNamedEvidenceKind::CurveNodeDefinition,
        "cn.gov.curve.05y",
        ContentHash::digest(b"cn.gov.curve.05y"),
    )
    .expect("extra node");
    assert!(
        PortfolioRiskAnalysis::new(exposure.clone(), vec![factor.clone(), node.clone(), extra])
            .is_err()
    );
    let drifted_factor = PortfolioRiskNamedEvidenceBinding::immutable_definition(
        PortfolioRiskNamedEvidenceKind::FactorDefinition,
        "cn.gov.yield.02y",
        ContentHash::digest(b"drifted-factor"),
    )
    .expect("drifted factor");
    assert!(
        PortfolioRiskAnalysis::new(exposure.clone(), vec![drifted_factor, node.clone()]).is_err()
    );
    let drifted_node = PortfolioRiskNamedEvidenceBinding::immutable_definition(
        PortfolioRiskNamedEvidenceKind::CurveNodeDefinition,
        "cn.gov.curve.02y",
        ContentHash::digest(b"drifted-node"),
    )
    .expect("drifted node");
    assert!(PortfolioRiskAnalysis::new(exposure, vec![factor, drifted_node]).is_err());
}

#[tokio::test]
async fn authority_drift_matrix_stops_every_numerical_handoff_and_publication() {
    for category in [
        ApplicationErrorCategory::HashMismatch,
        ApplicationErrorCategory::LineageIncomplete,
        ApplicationErrorCategory::ValidationFailed,
        ApplicationErrorCategory::NotFound,
        ApplicationErrorCategory::Forbidden,
    ] {
        let authority = RejectingAuthority::new(category);
        let analytics = FixtureAnalytics::default();
        let positions = SpyPositions::default();
        let views = SpyViews::default();
        let risk = SpyRisk::default();
        let bonds = SpyBonds::default();
        let publisher = SpyPublisher::default();
        let use_case = PortfolioAggregationUseCase::new(
            &authority, &analytics, &positions, &views, &risk, &bonds, &publisher,
        );

        let error = use_case
            .execute(&aggregation_principal(), &aggregation_context())
            .await
            .expect_err("authority drift must fail");
        assert_eq!(error.category(), category);
        assert_eq!(authority.calls.load(Ordering::SeqCst), 1);
        assert_eq!(analytics.calls.load(Ordering::SeqCst), 0);
        assert_eq!(positions.calls.load(Ordering::SeqCst), 0);
        assert_eq!(views.calls.load(Ordering::SeqCst), 0);
        assert_eq!(risk.calls.load(Ordering::SeqCst), 0);
        assert_eq!(bonds.calls.load(Ordering::SeqCst), 0);
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn analytics_authority_time_drift_stops_all_numerical_handoffs() {
    let baseline = aggregation_snapshot(id(20), id(22), "100");
    for observed_at in drifted_market_times(baseline.observed_at()) {
        let fixture = happy_aggregation_fixture();
        let authority = ResolvedAuthority {
            resolved: fixture.resolved,
        };
        let positions = SnapshotRepo {
            member: fixture.member_snapshot,
            benchmark: fixture.benchmark_snapshot,
            calls: AtomicUsize::new(0),
        };
        let analytics = TimeDriftAnalytics {
            observed_at,
            calls: AtomicUsize::new(0),
        };
        let views = SpyViews::default();
        let risk = SpyRisk::default();
        let bonds = SpyBonds::default();
        let publisher = SpyPublisher::default();
        let use_case = PortfolioAggregationUseCase::new(
            &authority, &analytics, &positions, &views, &risk, &bonds, &publisher,
        );

        let error = use_case
            .execute(&aggregation_principal(), &fixture.context)
            .await
            .expect_err("authority time drift");
        assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);
        assert_eq!(analytics.calls.load(Ordering::SeqCst), 1);
        assert_eq!(views.calls.load(Ordering::SeqCst), 0);
        assert_eq!(risk.calls.load(Ordering::SeqCst), 0);
        assert_eq!(bonds.calls.load(Ordering::SeqCst), 0);
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn normalized_catalog_missing_extra_hash_or_time_drift_stops_before_any_handoff() {
    let baseline = happy_aggregation_fixture();
    for catalog_evidence in drifted_catalog_evidence_sets(&baseline.resolved.catalog_evidence) {
        let resolution =
            NormalizedPortfolioContextResolution::new(baseline.context.clone(), catalog_evidence)
                .expect("shape-valid drifted resolution");
        let authority = ResolvedAuthority {
            resolved: baseline.resolved.clone(),
        };
        let positions = SpyPositions::default();
        let analytics = FixtureAnalytics::default();
        let views = SpyViews::default();
        let risk = SpyRisk::default();
        let bonds = SpyBonds::default();
        let publisher = SpyPublisher::default();
        let use_case = PortfolioAggregationUseCase::new(
            &authority, &analytics, &positions, &views, &risk, &bonds, &publisher,
        );

        let error = use_case
            .execute_resolution(&aggregation_principal(), &resolution)
            .await
            .expect_err("catalog evidence drift");
        assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);
        assert_eq!(positions.calls.load(Ordering::SeqCst), 0);
        assert_eq!(analytics.calls.load(Ordering::SeqCst), 0);
        assert_eq!(views.calls.load(Ordering::SeqCst), 0);
        assert_eq!(risk.calls.load(Ordering::SeqCst), 0);
        assert_eq!(bonds.calls.load(Ordering::SeqCst), 0);
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 0);
    }
}

fn drifted_catalog_evidence_sets(
    baseline: &[PortfolioCatalogEvidenceBinding],
) -> Vec<Vec<PortfolioCatalogEvidenceBinding>> {
    let mut missing = baseline.to_vec();
    missing.pop();

    let mut extra = baseline.to_vec();
    extra.push(
        PortfolioCatalogEvidenceBinding::new(
            PortfolioCatalogEvidenceRole::MemberPortfolio,
            VersionRef::new(id(66), Version::new(1).expect("extra version")),
            ContentHash::digest(b"extra catalog member"),
            aggregation_time(21, 8),
            aggregation_time(19, 0),
            aggregation_time(22, 0),
        )
        .expect("extra catalog evidence"),
    );

    let first = baseline.first().expect("catalog evidence");
    let mut hash_drift = baseline.to_vec();
    hash_drift[0] = PortfolioCatalogEvidenceBinding::new(
        first.role(),
        first.reference().clone(),
        ContentHash::digest(b"drifted catalog hash"),
        first.visible_at().clone(),
        first.effective_from().clone(),
        first.effective_to().clone(),
    )
    .expect("hash drift evidence");

    let visible_at = market_time_from_instant(
        first.visible_at().instant() + chrono::Duration::nanoseconds(1),
        first.visible_at().market_timezone(),
    );
    let mut time_drift = baseline.to_vec();
    time_drift[0] = PortfolioCatalogEvidenceBinding::new(
        first.role(),
        first.reference().clone(),
        first.content_hash().clone(),
        visible_at,
        first.effective_from().clone(),
        first.effective_to().clone(),
    )
    .expect("time drift evidence");

    vec![missing, extra, hash_drift, time_drift]
}

#[tokio::test]
async fn aggregation_reuses_position_views_krd_and_bond_handoffs_without_reimplementing_them() {
    let fixture = happy_aggregation_fixture();
    let resolution = NormalizedPortfolioContextResolution::new(
        fixture.context.clone(),
        fixture.resolved.catalog_evidence.clone(),
    )
    .expect("normalized resolution");
    let expected_participation = risk_for(&fixture.member_snapshot).coverage().clone();
    let authority = ResolvedAuthority {
        resolved: fixture.resolved,
    };
    let positions = SnapshotRepo {
        member: fixture.member_snapshot,
        benchmark: fixture.benchmark_snapshot,
        calls: AtomicUsize::new(0),
    };
    let analytics = FixtureAnalytics::default();
    let views = ReuseViews::default();
    let risk = ReuseRisk::default();
    let bonds = ReuseBonds::default();
    let publisher = EvidencePublisher::default();
    let use_case = PortfolioAggregationUseCase::new(
        &authority, &analytics, &positions, &views, &risk, &bonds, &publisher,
    );

    let overview = use_case
        .execute_resolution(&aggregation_principal(), &resolution)
        .await
        .expect("reused aggregation seams");

    assert_eq!(overview.draft().data_mode(), PortfolioMetricDataMode::Real);
    assert_eq!(overview.draft().members().len(), 1);
    let actual_participation = overview.draft().coverage().participation();
    assert_eq!(
        actual_participation.imported_position_count(),
        expected_participation.imported_position_count()
    );
    assert_eq!(
        actual_participation.participating_position_count(),
        expected_participation.participating_position_count()
    );
    assert_eq!(
        actual_participation.imported_gross_economic_value_by_unit(),
        expected_participation.imported_gross_economic_value_by_unit()
    );
    assert_eq!(
        actual_participation.participating_gross_economic_value_by_unit(),
        expected_participation.participating_gross_economic_value_by_unit()
    );
    assert_eq!(
        actual_participation.missing_critical_field_record_count(),
        expected_participation.missing_critical_field_record_count()
    );
    assert_eq!(
        actual_participation.source_confidence(),
        expected_participation.source_confidence()
    );
    assert_eq!(
        actual_participation.distinct_external_data_source_version_count(),
        expected_participation.distinct_external_data_source_version_count()
    );
    assert_eq!(positions.calls.load(Ordering::SeqCst), 2);
    assert_eq!(analytics.calls.load(Ordering::SeqCst), 2);
    assert_eq!(views.calls.load(Ordering::SeqCst), 2);
    assert_eq!(risk.calls.load(Ordering::SeqCst), 2);
    assert_eq!(bonds.calls.load(Ordering::SeqCst), 2);
    assert_eq!(publisher.calls.load(Ordering::SeqCst), 1);

    let implementations = overview
        .draft()
        .implementation_bindings()
        .expect("verified implementation bindings");
    let roles = implementations
        .iter()
        .map(FormalImplementationBinding::role)
        .collect::<Vec<_>>();
    assert_eq!(
        roles,
        vec![
            "analyze-bond.benchmark.0000",
            "analyze-bond.member.0000.0000",
            "factor-topology.benchmark",
            "factor-topology.member.0000",
            "krd-algorithm.benchmark",
            "krd-algorithm.member.0000",
            "portfolio-aggregation",
            "position-views",
        ]
    );
    assert_eq!(
        overview.formal_evidence().implementations(),
        implementations
    );
    assert_implementation_drift_changes_identity(&overview);
}

#[tokio::test]
async fn empty_missing_or_drifted_implementation_evidence_fails_closed() {
    for mode in [
        EvidenceImplementationMode::Empty,
        EvidenceImplementationMode::Missing,
        EvidenceImplementationMode::Drifted,
    ] {
        let error = execute_happy_with_evidence_mode(mode)
            .await
            .expect_err("forged implementation evidence must fail");
        assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);
    }
}

#[tokio::test]
async fn owned_overview_factory_binds_complete_member_and_benchmark_risk_inputs() {
    let overview = execute_happy_with_evidence_mode(EvidenceImplementationMode::Exact)
        .await
        .expect("verified overview draft");
    let factory = OwnedPortfolioOverviewRecordFactory::new(
        Arc::new(SubjectFixture),
        PortfolioFormalExecutionBinding::new(
            CodeBinding::new(
                "34402344c7d2c9238dc171af52ac4db77eb6b462",
                "f66e03c55703837d6f2aee9959eba482612272f1",
            )
            .expect("code"),
            RuntimeBinding::new(
                ContentHash::digest(b"image"),
                ContentHash::digest(b"environment"),
            ),
        ),
    );
    let principal = aggregation_principal();
    let record = factory
        .build(
            principal.access_scope(),
            &aggregation_owner(),
            overview.draft(),
        )
        .await
        .expect("owned record factory");
    let inputs = record.evidence().consumed_inputs();
    for (role, kind, identity) in [
        (
            "member.0000.risk.0000",
            FormalInputKind::FactorDefinition,
            "cn.gov.yield.02y",
        ),
        (
            "member.0000.risk.0001",
            FormalInputKind::CurveNodeDefinition,
            "cn.gov.curve.02y",
        ),
        (
            "benchmark.risk.0000",
            FormalInputKind::FactorDefinition,
            "cn.gov.yield.02y",
        ),
        (
            "benchmark.risk.0001",
            FormalInputKind::CurveNodeDefinition,
            "cn.gov.curve.02y",
        ),
    ] {
        let input = inputs
            .iter()
            .find(|input| input.role() == role)
            .expect("risk formal input");
        assert_eq!(input.kind(), kind);
        let FormalInputReference::Named(reference) = input.reference() else {
            panic!("named risk input");
        };
        assert_eq!(reference.identity(), identity);
        assert!(input.observed_at().is_none());
        assert!(input.visible_at().is_none());
        assert!(input.effective_from().is_none());
        assert!(input.effective_to().is_none());
    }
    for (role, evidence) in [
        (
            "member.0000.authority.0000",
            &overview.draft().members()[0].analytics_authority_evidence()[0],
        ),
        (
            "benchmark.authority.0000",
            &overview.draft().benchmark_analytics_authority_evidence()[0],
        ),
    ] {
        let input = inputs
            .iter()
            .find(|input| input.role() == role)
            .expect("snapshot authority formal input");
        assert_eq!(input.observed_at(), evidence.observed_at.as_ref());
        assert_eq!(input.visible_at(), evidence.visible_at.as_ref());
    }
    assert_valuation_fact_inputs(inputs);
    assert_eq!(
        record.evidence().implementations(),
        overview
            .draft()
            .implementation_bindings()
            .expect("implementations")
    );
}

fn assert_valuation_fact_inputs(inputs: &[FormalInputBinding]) {
    for role in ["member.0000.authority.0001", "benchmark.authority.0001"] {
        let input = inputs
            .iter()
            .find(|input| input.role() == role)
            .expect("valuation authority formal input");
        assert_eq!(input.kind(), FormalInputKind::Fact);
        assert!(matches!(
            input.reference(),
            FormalInputReference::Object(reference)
                if reference.object_id() == &id(65)
                    && reference.version() == Some(Version::new(1).expect("valuation version"))
                    && reference.content_hash() == Some(&ContentHash::digest(b"bond-valuation"))
        ));
        assert_eq!(input.observed_at(), Some(&aggregation_time(20, 9)));
    }
}

#[tokio::test]
async fn authority_time_precision_timezone_and_local_date_bind_fingerprint_and_identity() {
    let overview = execute_happy_with_evidence_mode(EvidenceImplementationMode::Exact)
        .await
        .expect("verified overview");
    let draft = overview.draft();
    let member_snapshot = aggregation_snapshot(id(20), id(22), "100");
    let baseline_authority = resolved_analytics_authority(&member_snapshot).expect("authority");
    let baseline_identity = overview.formal_evidence().output_identity();
    let implementations = draft
        .implementation_bindings()
        .expect("implementation bindings");

    for drifted_time in drifted_market_times(member_snapshot.observed_at()) {
        let drifted_authority =
            resolved_analytics_authority_at(&member_snapshot, drifted_time.clone())
                .expect("time-drifted authority");
        assert_ne!(
            drifted_authority.request_fingerprint,
            baseline_authority.request_fingerprint
        );

        let mut inputs = draft
            .formal_input_bindings(&aggregation_owner(), &aggregation_subject_hash())
            .expect("formal inputs");
        let input = inputs
            .iter_mut()
            .find(|input| input.role() == "member.0000.authority.0000")
            .expect("member authority input");
        *input = FormalInputBinding::new(FormalInputBindingInput {
            role: input.role().to_owned(),
            kind: input.kind(),
            owner: input.owner().clone(),
            reference: input.reference().clone(),
            observed_at: Some(drifted_time),
            visible_at: input.visible_at().cloned(),
            effective_from: input.effective_from().cloned(),
            effective_to: input.effective_to().cloned(),
        })
        .expect("drifted formal input");
        let drifted = fixture_formal_evidence_with_inputs(
            &aggregation_owner(),
            draft,
            implementations.clone(),
            inputs,
        )
        .expect("drifted evidence");
        assert_ne!(drifted.output_identity(), baseline_identity);
    }
}

#[tokio::test]
async fn catalog_visible_subsecond_timezone_and_local_date_bind_request_and_output_identity() {
    let baseline = execute_happy_with_evidence_mode(EvidenceImplementationMode::Exact)
        .await
        .expect("baseline overview");
    let visible_at = aggregation_time(21, 8);
    let variants = [
        market_time_from_instant(
            visible_at.instant() + chrono::Duration::nanoseconds(1),
            visible_at.market_timezone(),
        ),
        market_time_from_instant(visible_at.instant(), "UTC"),
        market_time_from_instant(visible_at.instant(), "Pacific/Honolulu"),
    ];
    assert_ne!(
        variants[2].local_trading_date(),
        visible_at.local_trading_date()
    );
    for drifted_visible_at in variants {
        let fixture = aggregation_fixture_with_portfolio_visibility(&drifted_visible_at);
        let drifted = execute_aggregation_fixture(fixture, EvidenceImplementationMode::Exact)
            .await
            .expect("time-bound overview");
        assert_ne!(
            drifted.draft().request_fingerprint(),
            baseline.draft().request_fingerprint()
        );
        assert_ne!(
            drifted.formal_evidence().output_identity(),
            baseline.formal_evidence().output_identity()
        );
    }
}

async fn execute_happy_with_evidence_mode(
    mode: EvidenceImplementationMode,
) -> ApplicationResult<PortfolioOverview> {
    execute_aggregation_fixture(happy_aggregation_fixture(), mode).await
}

async fn execute_aggregation_fixture(
    fixture: HappyAggregationFixture,
    mode: EvidenceImplementationMode,
) -> ApplicationResult<PortfolioOverview> {
    let resolution = NormalizedPortfolioContextResolution::new(
        fixture.context.clone(),
        fixture.resolved.catalog_evidence.clone(),
    )?;
    let authority = ResolvedAuthority {
        resolved: fixture.resolved,
    };
    let positions = SnapshotRepo {
        member: fixture.member_snapshot,
        benchmark: fixture.benchmark_snapshot,
        calls: AtomicUsize::new(0),
    };
    let analytics = FixtureAnalytics::default();
    let views = ReuseViews::default();
    let risk = ReuseRisk::default();
    let bonds = ReuseBonds::default();
    let publisher = EvidencePublisher::new(mode);
    PortfolioAggregationUseCase::new(
        &authority, &analytics, &positions, &views, &risk, &bonds, &publisher,
    )
    .execute_resolution(&aggregation_principal(), &resolution)
    .await
}

struct HappyAggregationFixture {
    context: NormalizedPortfolioContext,
    resolved: ResolvedPortfolioAggregationInputs,
    member_snapshot: PositionSnapshot,
    benchmark_snapshot: PositionSnapshot,
}

fn happy_aggregation_fixture() -> HappyAggregationFixture {
    let member_snapshot = aggregation_snapshot(id(20), id(22), "100");
    let benchmark_snapshot = aggregation_snapshot(id(21), id(23), "90");
    let member_binding = snapshot_binding(&member_snapshot);
    let benchmark_binding = snapshot_binding(&benchmark_snapshot);
    let convention = aggregation_convention();
    let benchmark = aggregation_benchmark(benchmark_binding.clone());
    let portfolio = aggregation_portfolio(&member_binding, &benchmark, &convention);
    let selected = definition_lineage(&portfolio);
    let context = NormalizedPortfolioContext {
        owner: aggregation_owner(),
        subject_ref: aggregation_subject(),
        scope: ExactPortfolioScope::new(
            ExactPortfolioScopeKind::Portfolio(selected.clone()),
            vec![selected],
        ),
        valuation_at: aggregation_time(20, 9),
        knowledge_at: aggregation_time(21, 9),
        currency: PortfolioCurrencyMode::Original,
        currency_unit: money_unit(),
        look_through: PortfolioLookThroughMode::None,
        benchmark: BenchmarkRef::new(
            benchmark.reference().clone(),
            benchmark.content_hash().clone(),
        ),
        period: PortfolioPeriodPreset::OneDay,
        period_from: aggregation_time(19, 9),
        period_to: aggregation_time(20, 9),
        metric_convention: PortfolioMetricConventionRef::new(
            convention.reference().clone(),
            convention.content_hash().clone(),
        ),
    };
    let visible_at = aggregation_time(21, 8);
    let catalog_evidence = vec![
        PortfolioCatalogEvidenceBinding::new(
            PortfolioCatalogEvidenceRole::SelectedPortfolio,
            portfolio.reference().clone(),
            portfolio.content_hash().clone(),
            visible_at.clone(),
            portfolio.effective_from().clone(),
            portfolio.effective_to().clone(),
        )
        .expect("selected evidence"),
        PortfolioCatalogEvidenceBinding::new(
            PortfolioCatalogEvidenceRole::MemberPortfolio,
            portfolio.reference().clone(),
            portfolio.content_hash().clone(),
            visible_at.clone(),
            portfolio.effective_from().clone(),
            portfolio.effective_to().clone(),
        )
        .expect("member evidence"),
        PortfolioCatalogEvidenceBinding::new(
            PortfolioCatalogEvidenceRole::Benchmark,
            benchmark.reference().clone(),
            benchmark.content_hash().clone(),
            visible_at.clone(),
            benchmark.effective_from().clone(),
            benchmark.effective_to().clone(),
        )
        .expect("benchmark evidence"),
        PortfolioCatalogEvidenceBinding::new(
            PortfolioCatalogEvidenceRole::MetricConvention,
            convention.reference().clone(),
            convention.content_hash().clone(),
            visible_at.clone(),
            convention.effective_from().clone(),
            convention.effective_to().clone(),
        )
        .expect("convention evidence"),
    ];
    let resolved = ResolvedPortfolioAggregationInputs {
        exact_scope: context.scope.clone(),
        portfolios: vec![VisibleCatalogRecord::new(portfolio, visible_at.clone())],
        convention: VisibleCatalogRecord::new(convention, visible_at.clone()),
        benchmark: VisibleCatalogRecord::new(benchmark, visible_at),
        benchmark_snapshot: benchmark_binding,
        catalog_evidence,
    };
    HappyAggregationFixture {
        context,
        resolved,
        member_snapshot,
        benchmark_snapshot,
    }
}

fn aggregation_fixture_with_portfolio_visibility(
    visible_at: &MarketTime,
) -> HappyAggregationFixture {
    let mut fixture = happy_aggregation_fixture();
    let portfolio = fixture.resolved.portfolios[0].value().clone();
    fixture.resolved.portfolios[0] = VisibleCatalogRecord::new(portfolio, visible_at.clone());
    fixture.resolved.catalog_evidence = fixture
        .resolved
        .catalog_evidence
        .iter()
        .map(|binding| {
            let binding_visible_at = if matches!(
                binding.role(),
                PortfolioCatalogEvidenceRole::SelectedPortfolio
                    | PortfolioCatalogEvidenceRole::MemberPortfolio
            ) {
                visible_at.clone()
            } else {
                binding.visible_at().clone()
            };
            PortfolioCatalogEvidenceBinding::new(
                binding.role(),
                binding.reference().clone(),
                binding.content_hash().clone(),
                binding_visible_at,
                binding.effective_from().clone(),
                binding.effective_to().clone(),
            )
            .expect("time-bound catalog evidence")
        })
        .collect();
    fixture
}

fn aggregation_snapshot(snapshot_id: Ulid, position_id: Ulid, value: &str) -> PositionSnapshot {
    let position = Position::new(PositionInput {
        position_id,
        instrument_ref: VersionRef::new(id(24), Version::new(1).expect("instrument version")),
        quantity: decimal("1", money_unit()),
        economic_value: decimal(value, money_unit()),
        economic_pnl: decimal("1", money_unit()),
        accounting_pnl: decimal("1", money_unit()),
        capital_requirement: decimal("1", money_unit()),
        accounting_classification: AccountingClassification::new(
            AccountingClassificationState::NotApplicable,
            None,
        )
        .expect("accounting classification"),
        holding_form: PositionHoldingForm::Owned,
    })
    .expect("position");
    let mut input = PositionSnapshotInput {
        snapshot_id,
        owner: aggregation_owner(),
        subject_ref: aggregation_subject(),
        observed_at: aggregation_time(20, 9),
        visible_at: aggregation_time(21, 8),
        content_hash: ContentHash::digest(b"placeholder"),
        lineage: vec![exact_lineage(id(25), b"snapshot-lineage")],
        positions: vec![position],
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    PositionSnapshot::new(input).expect("position snapshot")
}

fn snapshot_binding(snapshot: &PositionSnapshot) -> PortfolioSnapshotBinding {
    PortfolioSnapshotBinding::new(
        snapshot.id().clone(),
        snapshot.content_hash().clone(),
        snapshot.observed_at().clone(),
        snapshot.visible_at().clone(),
    )
    .expect("snapshot binding")
}

fn aggregation_convention() -> PortfolioMetricConvention {
    let mut input = PortfolioMetricConventionInput {
        convention: VersionRef::new(id(26), Version::new(1).expect("convention version")),
        owner: aggregation_owner(),
        schema_id: "ficant.portfolio-metric-convention.v1".to_owned(),
        ytm_weighting: PortfolioMetricWeighting::MarketValueTimesModifiedDuration,
        duration_weighting: PortfolioMetricWeighting::MarketValue,
        convexity_weighting: PortfolioMetricWeighting::MarketValue,
        coupon_weighting: PortfolioMetricWeighting::Notional,
        remaining_life_weighting: PortfolioMetricWeighting::Notional,
        rounding: PortfolioDecimalRounding::TiesToEven,
        freshness_limit_seconds: 86_400,
        effective_from: aggregation_time(19, 0),
        effective_to: aggregation_time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = PortfolioMetricConvention::content_hash_for(&input);
    PortfolioMetricConvention::new(input).expect("convention")
}

fn aggregation_benchmark(binding: PortfolioSnapshotBinding) -> Benchmark {
    let mut input = BenchmarkInput {
        benchmark: VersionRef::new(id(27), Version::new(1).expect("benchmark version")),
        owner: aggregation_owner(),
        subject_ref: aggregation_subject(),
        code: "CGB-BENCH".to_owned(),
        display_name: "CGB Benchmark".to_owned(),
        position_snapshot: binding,
        effective_from: aggregation_time(19, 0),
        effective_to: aggregation_time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = Benchmark::content_hash_for(&input);
    Benchmark::new(input).expect("benchmark")
}

fn aggregation_portfolio(
    binding: &PortfolioSnapshotBinding,
    benchmark: &Benchmark,
    convention: &PortfolioMetricConvention,
) -> Portfolio {
    let mut input = PortfolioInput {
        portfolio: VersionRef::new(id(28), Version::new(1).expect("portfolio version")),
        owner: aggregation_owner(),
        subject_ref: aggregation_subject(),
        book: exact_lineage(id(29), b"book"),
        group: exact_lineage(id(30), b"group"),
        code: "CGB-PORT".to_owned(),
        display_name: "CGB Portfolio".to_owned(),
        status: PortfolioStatus::Active,
        position_snapshot: binding.clone(),
        benchmark: BenchmarkRef::new(
            benchmark.reference().clone(),
            benchmark.content_hash().clone(),
        ),
        metric_convention: PortfolioMetricConventionRef::new(
            convention.reference().clone(),
            convention.content_hash().clone(),
        ),
        effective_from: aggregation_time(19, 0),
        effective_to: aggregation_time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = Portfolio::content_hash_for(&input);
    Portfolio::new(input).expect("portfolio")
}

fn definition_lineage<T>(value: &T) -> LineageRef
where
    T: ContentAddressed + VersionedDefinition,
{
    LineageRef::new(
        Ulid::new(value.identity().to_owned()).expect("definition id"),
        Some(Version::new(value.version()).expect("definition version")),
        Some(value.content_hash().clone()),
    )
    .expect("definition lineage")
}

struct ResolvedAuthority {
    resolved: ResolvedPortfolioAggregationInputs,
}

#[async_trait]
impl PortfolioAggregationAuthority for ResolvedAuthority {
    async fn resolve_aggregation_inputs(
        &self,
        _principal: &AuthorizedPrincipal,
        _context: &NormalizedPortfolioContext,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs> {
        Ok(self.resolved.clone())
    }
}

struct SnapshotRepo {
    member: PositionSnapshot,
    benchmark: PositionSnapshot,
    calls: AtomicUsize,
}

#[async_trait]
impl PositionSnapshotRepository for SnapshotRepo {
    async fn get_position_snapshot(
        &self,
        _scope: &AccessScope,
        snapshot_id: Ulid,
        _knowledge_at: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(if snapshot_id == *self.member.id() {
            Some(self.member.clone())
        } else if snapshot_id == *self.benchmark.id() {
            Some(self.benchmark.clone())
        } else {
            None
        })
    }

    async fn resolve_position_snapshot(
        &self,
        _scope: &AccessScope,
        _subject_ref: VersionRef,
        _observed_at: MarketTime,
        _knowledge_at: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>> {
        Ok(None)
    }
}

#[derive(Default)]
struct ReuseViews {
    calls: AtomicUsize,
}

#[derive(Default)]
struct FixtureAnalytics {
    calls: AtomicUsize,
}

struct TimeDriftAnalytics {
    observed_at: MarketTime,
    calls: AtomicUsize,
}

#[async_trait]
impl PortfolioAnalyticsAuthorityHandoff for TimeDriftAnalytics {
    async fn resolve(
        &self,
        _principal: &AuthorizedPrincipal,
        _context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
    ) -> ApplicationResult<ResolvedPortfolioAnalyticsAuthority> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        resolved_analytics_authority_at(snapshot, self.observed_at.clone())
    }
}

#[async_trait]
impl PortfolioAnalyticsAuthorityHandoff for FixtureAnalytics {
    async fn resolve(
        &self,
        _principal: &AuthorizedPrincipal,
        _context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
    ) -> ApplicationResult<ResolvedPortfolioAnalyticsAuthority> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        resolved_analytics_authority(snapshot)
    }
}

fn resolved_analytics_authority(
    snapshot: &PositionSnapshot,
) -> ApplicationResult<ResolvedPortfolioAnalyticsAuthority> {
    resolved_analytics_authority_at(snapshot, snapshot.observed_at().clone())
}

fn resolved_analytics_authority_at(
    snapshot: &PositionSnapshot,
    observed_at: MarketTime,
) -> ApplicationResult<ResolvedPortfolioAnalyticsAuthority> {
    let units = analytics_units();
    let bonds = snapshot
        .positions()
        .iter()
        .map(|position| bond_authority(position, units.clone()))
        .collect();
    ResolvedPortfolioAnalyticsAuthority::new(
        id(60),
        &ContentHash::digest(b"analytics-authority"),
        PortfolioRiskAuthority {
            curve_snapshot_id: id(32),
            dv01_unit: dv01_unit(),
            futures_data_snapshot_id: None,
        },
        units,
        bonds,
        vec![
            PortfolioAnalyticsEvidenceBinding {
                kind: PortfolioAnalyticsEvidenceKind::PositionSnapshot,
                object_id: snapshot.id().clone(),
                version: None,
                content_hash: snapshot.content_hash().clone(),
                observed_at: Some(observed_at.clone()),
                visible_at: Some(snapshot.visible_at().clone()),
                effective_from: None,
                effective_to: None,
            },
            PortfolioAnalyticsEvidenceBinding {
                kind: PortfolioAnalyticsEvidenceKind::Valuation,
                object_id: id(65),
                version: Some(Version::new(1).expect("valuation version")),
                content_hash: ContentHash::digest(b"bond-valuation"),
                observed_at: Some(observed_at),
                visible_at: None,
                effective_from: None,
                effective_to: None,
            },
        ],
    )
}

fn bond_authority(
    position: &Position,
    result_units: Vec<PortfolioRatesUnitAuthority>,
) -> PortfolioBondRatesAuthorityResolution {
    PortfolioBondRatesAuthorityResolution::Bond(Box::new(PortfolioBondRatesAuthority {
        position_id: position.id().clone(),
        instrument_ref: position.instrument_ref().clone(),
        bond: AnalyticsObjectRef::new(
            position.instrument_ref().clone(),
            ContentHash::digest(b"bond-definition"),
        ),
        calendar: AnalyticsObjectRef::new(
            VersionRef::new(id(62), Version::new(1).expect("calendar version")),
            ContentHash::digest(b"calendar-definition"),
        ),
        data_snapshot: PortfolioImmutableSnapshotAuthority {
            id: id(63),
            content_hash: ContentHash::digest(b"market-data-snapshot"),
        },
        tax_rule_pack: AnalyticsObjectRef::new(
            VersionRef::new(id(64), Version::new(1).expect("tax version")),
            ContentHash::digest(b"tax-rule-pack"),
        ),
        currency_unit: money_unit(),
        rate_unit: rate_unit(),
        result_units,
        settlement_date: date(2026, 8, 21),
        calendar_requirement: CalendarRequirement::ExactMarket,
        mode: AnalyticsMode::YieldIn,
        input_value: fixed("0.03"),
        remaining_years: fixed("3.5"),
        valuation: PortfolioValuationAuthorityBinding {
            valuation_id: id(65),
            source_revision: 1,
            content_hash: ContentHash::digest(b"bond-valuation"),
            value_index: 0,
        },
    }))
}

fn analytics_units() -> Vec<PortfolioRatesUnitAuthority> {
    [
        (PortfolioRatesUnitRole::CurrencyAmount, money_unit()),
        (PortfolioRatesUnitRole::PricePer100, alternate_money_unit()),
        (PortfolioRatesUnitRole::Rate, rate_unit()),
        (PortfolioRatesUnitRole::Years, duration_unit()),
        (PortfolioRatesUnitRole::YearsSquared, convexity_unit()),
        (
            PortfolioRatesUnitRole::Dv01Per100,
            unit("01ARZ3NDEKTSV4RRFFQ69G5FB1"),
        ),
        (PortfolioRatesUnitRole::Dv01, dv01_unit()),
        (
            PortfolioRatesUnitRole::Dimensionless,
            unit("01ARZ3NDEKTSV4RRFFQ69G5FB2"),
        ),
        (
            PortfolioRatesUnitRole::ContractCount,
            unit("01ARZ3NDEKTSV4RRFFQ69G5FB3"),
        ),
    ]
    .into_iter()
    .map(|(role, reference)| PortfolioRatesUnitAuthority {
        role,
        reference,
        content_hash: ContentHash::digest(role.expected_dimension().as_bytes()),
        dimension: role.expected_dimension().to_owned(),
        scale: 6,
    })
    .collect()
}

impl PortfolioPositionViewsHandoff for ReuseViews {
    fn project(&self, snapshot: PositionSnapshot) -> ApplicationResult<PositionViews> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        project_verified_position_views(snapshot)
    }
}

#[derive(Default)]
struct ReuseRisk {
    calls: AtomicUsize,
}

#[async_trait]
impl PortfolioRiskHandoff for ReuseRisk {
    async fn calculate(
        &self,
        _scope: &AccessScope,
        _context: &NormalizedPortfolioContext,
        snapshot: &PositionSnapshot,
        _authority: &PortfolioRiskAuthority,
    ) -> ApplicationResult<PortfolioRiskAnalysis> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        risk_analysis_for(snapshot)
    }
}

fn risk_analysis_for(snapshot: &PositionSnapshot) -> ApplicationResult<PortfolioRiskAnalysis> {
    let exposure = risk_for(snapshot);
    let factor = exposure.totals().first().expect("risk factor");
    let factor_id = factor.factor_id().to_owned();
    let factor_hash = factor.factor_definition_hash().clone();
    PortfolioRiskAnalysis::new(
        exposure,
        vec![
            PortfolioRiskNamedEvidenceBinding::immutable_definition(
                PortfolioRiskNamedEvidenceKind::FactorDefinition,
                factor_id,
                factor_hash,
            )?,
            PortfolioRiskNamedEvidenceBinding::immutable_definition(
                PortfolioRiskNamedEvidenceKind::CurveNodeDefinition,
                "cn.gov.curve.02y",
                ContentHash::digest(b"cn.gov.curve.02y"),
            )?,
        ],
    )
}

fn risk_for(snapshot: &PositionSnapshot) -> PortfolioKeyRateExposure {
    let position = snapshot.positions().first().expect("risk position");
    let factor = FactorDv01::new(
        "cn.gov.yield.02y",
        ContentHash::digest(b"cn.gov.yield.02y"),
        fixed("0.01"),
        dv01_unit(),
    )
    .expect("factor");
    let mut risk_input_hashes = vec![
        ContentHash::digest(b"cn.gov.curve.02y"),
        ContentHash::digest(b"risk-input"),
    ];
    risk_input_hashes.sort_unstable();
    let exposure = PositionKeyRateExposure::new(
        position.id().clone(),
        position.instrument_ref().clone(),
        vec![factor],
        risk_input_hashes,
        vec![exact_lineage(id(31), b"risk-lineage")],
    )
    .expect("position exposure");
    let source = PriceSourceSummary::new(vec![
        PriceSourceCount::new(PriceSourceType::CurveInterpolation, 1).expect("source count"),
    ])
    .expect("source summary");
    let coverage = CoverageDeclaration::for_complete_positions(
        snapshot.positions(),
        &[position.id().clone()],
        Some(source.clone()),
        0,
    )
    .expect("risk coverage");
    PortfolioKeyRateExposure::new(
        snapshot.id().clone(),
        id(32),
        vec![exposure],
        RiskAlgorithmBinding::new("ficant.test.krd", 1, "test-v1").expect("risk algorithm"),
        (source, coverage),
        vec![exact_lineage(id(33), b"portfolio-risk")],
    )
    .expect("portfolio risk")
}

#[derive(Default)]
struct ReuseBonds {
    calls: AtomicUsize,
}

#[async_trait]
impl PortfolioBondAnalysisHandoff for ReuseBonds {
    async fn analyze(
        &self,
        _scope: &AccessScope,
        context: &NormalizedPortfolioContext,
        _snapshot: &PositionSnapshot,
        position: &Position,
        authority: &PortfolioBondRatesAuthorityResolution,
        _authority_fingerprint: &ficant_application::ports::OperationFingerprint,
    ) -> ApplicationResult<PortfolioBondAnalysis> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        verified_bond_analysis(context, position, authority)
    }
}

// The fixture deliberately materializes every transport-neutral R5D field in one place so a
// partial fake cannot accidentally satisfy the Portfolio Overview factory.
#[allow(clippy::too_many_lines)]
fn verified_bond_analysis(
    context: &NormalizedPortfolioContext,
    position: &Position,
    authority: &PortfolioBondRatesAuthorityResolution,
) -> ApplicationResult<PortfolioBondAnalysis> {
    let PortfolioBondRatesAuthorityResolution::Bond(authority) = authority else {
        panic!("bond fixture authority");
    };
    let terms = BondTerms::new(
        date(2020, 1, 1),
        date(2030, 1, 1),
        CouponFrequency::Annual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        fixed("0.03"),
        fixed("100"),
    )
    .expect("bond terms");
    let calendar = CalendarBinding::new(
        authority.calendar.version_ref().id().as_str(),
        authority.calendar.version_ref().version(),
        authority.calendar.content_hash().clone(),
        date(2020, 1, 1),
        date(2040, 1, 1),
        Vec::new(),
        Vec::new(),
    )
    .expect("calendar binding");
    let input = BondAnalyticsInput::new(
        context.owner.clone(),
        authority.bond.clone(),
        authority.tax_rule_pack.clone(),
        AnalyticsObjectRef::new(
            VersionRef::new(
                authority.data_snapshot.id.clone(),
                Version::new(1).expect("snapshot version"),
            ),
            authority.data_snapshot.content_hash.clone(),
        ),
        context.valuation_at.clone(),
        authority.settlement_date,
        authority.calendar_requirement,
        calendar,
        terms,
        authority.mode,
        authority.input_value,
    )
    .expect("analytics input");
    let cashflow = DerivedCashflow::new(
        1,
        date(2030, 1, 1),
        date(2030, 1, 1),
        fixed("3"),
        fixed("100"),
        fixed("103"),
    )
    .expect("cashflow");
    let measures = AnalyticsMeasures::new(
        fixed("0.1"),
        fixed("99.9"),
        fixed("0.03"),
        fixed("4.1"),
        fixed("4"),
        fixed("18"),
        fixed("0.04"),
    )
    .expect("analytics measures");
    let analytics =
        BondAnalyticsResult::new(input, CalendarResolution::Exact, vec![cashflow], measures)
            .expect("bond result");
    let request_evidence = RatesRequestEvidence::new(
        vec![RatesInputEvidence::new(
            RatesInputRole::Subject,
            context.owner.clone(),
            RatesEvidenceBinding::Object(AnalyticsObjectRef::new(
                context.subject_ref.clone(),
                ContentHash::digest(b"r5d-rates-subject-hash"),
            )),
            None,
            None,
            None,
            None,
        )],
        b"verified-bond-analysis",
    )?;
    let result_units = result_unit_bindings(&authority.result_units)?;
    let analysis_result = PortfolioBondAnalysisResult::from_verified(
        analytics,
        result_units,
        context.subject_ref.clone(),
        request_evidence,
    )?;
    let metrics = PortfolioBondMetricFacts::new(
        fixed("0.03"),
        fixed("4"),
        fixed("18"),
        fixed("0.03"),
        authority.remaining_years,
        weighted_units(),
    )?;
    assert_eq!(position.id(), &authority.position_id);
    Ok(PortfolioBondAnalysis::bond(
        fixed("1"),
        metrics,
        analysis_result,
    ))
}

fn result_unit_bindings(
    units: &[PortfolioRatesUnitAuthority],
) -> ApplicationResult<PortfolioRatesUnitBindings> {
    PortfolioRatesUnitBindings::new(
        units
            .iter()
            .map(|unit| {
                let role = match unit.role {
                    PortfolioRatesUnitRole::CurrencyAmount => ResultUnitRole::CurrencyAmount,
                    PortfolioRatesUnitRole::PricePer100 => ResultUnitRole::PricePer100,
                    PortfolioRatesUnitRole::Rate => ResultUnitRole::Rate,
                    PortfolioRatesUnitRole::Years => ResultUnitRole::Years,
                    PortfolioRatesUnitRole::YearsSquared => ResultUnitRole::YearsSquared,
                    PortfolioRatesUnitRole::Dv01Per100 => ResultUnitRole::Dv01Per100,
                    PortfolioRatesUnitRole::Dv01 => ResultUnitRole::Dv01,
                    PortfolioRatesUnitRole::Dimensionless => ResultUnitRole::Dimensionless,
                    PortfolioRatesUnitRole::ContractCount => ResultUnitRole::ContractCount,
                };
                (
                    role,
                    RatesUnitRequirement::new(unit.reference.clone(), role_dimension(role)),
                )
            })
            .collect(),
    )
}

const fn role_dimension(role: ResultUnitRole) -> &'static str {
    match role {
        ResultUnitRole::CurrencyAmount => "currency_amount",
        ResultUnitRole::PricePer100 => "price_per_100",
        ResultUnitRole::Rate => "rate",
        ResultUnitRole::Years => "years",
        ResultUnitRole::YearsSquared => "years_squared",
        ResultUnitRole::Dv01Per100 => "dv01_per_100",
        ResultUnitRole::Dv01 => "dv01",
        ResultUnitRole::Dimensionless => "dimensionless",
        ResultUnitRole::ContractCount => "contract_count",
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum EvidenceImplementationMode {
    #[default]
    Exact,
    Empty,
    Missing,
    Drifted,
}

struct EvidencePublisher {
    calls: AtomicUsize,
    mode: EvidenceImplementationMode,
}

impl Default for EvidencePublisher {
    fn default() -> Self {
        Self::new(EvidenceImplementationMode::Exact)
    }
}

impl EvidencePublisher {
    const fn new(mode: EvidenceImplementationMode) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            mode,
        }
    }
}

#[async_trait]
impl PortfolioOverviewPublisher for EvidencePublisher {
    async fn publish(
        &self,
        _scope: &AccessScope,
        owner: &OwnerRef,
        draft: &PortfolioOverviewDraft,
    ) -> ApplicationResult<FormalOutputEvidence> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut implementations = draft.implementation_bindings()?;
        match self.mode {
            EvidenceImplementationMode::Exact => {}
            EvidenceImplementationMode::Empty => implementations.clear(),
            EvidenceImplementationMode::Missing => {
                implementations.pop();
            }
            EvidenceImplementationMode::Drifted => {
                let first = implementations
                    .first_mut()
                    .expect("non-empty implementations");
                *first = FormalImplementationBinding::new(
                    first.role(),
                    ContentHash::digest(b"drifted-implementation"),
                )
                .expect("drifted implementation");
            }
        }
        fixture_formal_evidence(owner, draft, implementations)
    }
}

fn fixture_formal_evidence(
    owner: &OwnerRef,
    draft: &PortfolioOverviewDraft,
    implementations: Vec<FormalImplementationBinding>,
) -> ApplicationResult<FormalOutputEvidence> {
    let subject_hash = aggregation_subject_hash();
    let inputs = draft.formal_input_bindings(owner, &subject_hash)?;
    fixture_formal_evidence_with_inputs(owner, draft, implementations, inputs)
}

fn fixture_formal_evidence_with_inputs(
    owner: &OwnerRef,
    draft: &PortfolioOverviewDraft,
    implementations: Vec<FormalImplementationBinding>,
    consumed_inputs: Vec<FormalInputBinding>,
) -> ApplicationResult<FormalOutputEvidence> {
    let subject_hash = aggregation_subject_hash();
    let subject = FormalInputBinding::new(FormalInputBindingInput {
        role: "subject".to_owned(),
        kind: FormalInputKind::Subject,
        owner: owner.clone(),
        reference: FormalInputReference::Object(
            LineageRef::new(
                id(4),
                Some(Version::new(1).expect("subject version")),
                Some(subject_hash.clone()),
            )
            .expect("formal subject lineage"),
        ),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .expect("formal subject");
    FormalOutputEvidence::new(FormalOutputEvidenceInput {
        schema_id: "ficant.portfolio.v1.PortfolioOverview".to_owned(),
        subject,
        consumed_inputs,
        code: CodeBinding::new(
            "34402344c7d2c9238dc171af52ac4db77eb6b462",
            "f66e03c55703837d6f2aee9959eba482612272f1",
        )
        .expect("code"),
        runtime: RuntimeBinding::new(
            ContentHash::digest(b"image"),
            ContentHash::digest(b"environment"),
        ),
        implementations,
        parameters_hash: draft.request_fingerprint().clone(),
        seed: None,
        result_hash: ContentHash::digest(&draft.canonical_payload()),
    })
    .map_err(ficant_application::map_domain_error)
}

fn assert_implementation_drift_changes_identity(overview: &PortfolioOverview) {
    let draft = overview.draft();
    let implementations = draft
        .implementation_bindings()
        .expect("implementation bindings");
    let baseline_identity = overview.formal_evidence().output_identity();
    assert_rates_implementation_drift(draft, &implementations, baseline_identity);
    assert_krd_implementation_drift(draft, &implementations, baseline_identity);
    let empty = fixture_formal_evidence(&aggregation_owner(), draft, Vec::new())
        .expect("empty implementation evidence remains structurally representable");
    assert_ne!(empty.output_identity(), baseline_identity);
}

fn assert_rates_implementation_drift(
    draft: &PortfolioOverviewDraft,
    implementations: &[FormalImplementationBinding],
    baseline_identity: &ContentHash,
) {
    let member = draft.members().first().expect("member overview");
    let analysis = member
        .bond_analyses()
        .first()
        .expect("member bond analysis")
        .analysis();
    let metadata = analysis.metadata();
    let rates_role = "analyze-bond.member.0000.0000";
    let rates_digest = rates_implementation_digest(
        metadata.engine_id(),
        metadata.engine_version(),
        metadata.algorithm_id(),
        metadata.algorithm_version(),
        metadata.convention_profile(),
        metadata.abi_version(),
    );
    assert_eq!(
        implementation_digest(implementations, rates_role),
        &rates_digest
    );
    for drifted in [
        rates_implementation_digest(
            "drifted-engine",
            metadata.engine_version(),
            metadata.algorithm_id(),
            metadata.algorithm_version(),
            metadata.convention_profile(),
            metadata.abi_version(),
        ),
        rates_implementation_digest(
            metadata.engine_id(),
            metadata.engine_version(),
            "drifted-algorithm",
            metadata.algorithm_version(),
            metadata.convention_profile(),
            metadata.abi_version(),
        ),
        rates_implementation_digest(
            metadata.engine_id(),
            metadata.engine_version(),
            metadata.algorithm_id(),
            metadata.algorithm_version(),
            metadata.convention_profile(),
            metadata.abi_version() + 1,
        ),
    ] {
        assert_ne!(drifted, rates_digest);
        assert_drifted_identity(
            draft,
            implementations,
            rates_role,
            drifted,
            baseline_identity,
        );
    }
}

fn assert_krd_implementation_drift(
    draft: &PortfolioOverviewDraft,
    implementations: &[FormalImplementationBinding],
    baseline_identity: &ContentHash,
) {
    let member = draft.members().first().expect("member overview");
    let exposure = member.key_rate_exposure();
    let algorithm = exposure.algorithm();
    let algorithm_role = "krd-algorithm.member.0000";
    let algorithm_digest = krd_algorithm_digest(
        algorithm.algorithm_id(),
        algorithm.algorithm_version(),
        algorithm.convention_profile(),
    );
    assert_eq!(
        implementation_digest(implementations, algorithm_role),
        &algorithm_digest
    );
    let drifted_algorithm = krd_algorithm_digest(
        "drifted-krd",
        algorithm.algorithm_version(),
        algorithm.convention_profile(),
    );
    assert_ne!(drifted_algorithm, algorithm_digest);
    assert_drifted_identity(
        draft,
        implementations,
        algorithm_role,
        drifted_algorithm,
        baseline_identity,
    );

    let topology_role = "factor-topology.member.0000";
    let topology_digest = factor_topology_digest(exposure, None);
    assert_eq!(
        implementation_digest(implementations, topology_role),
        &topology_digest
    );
    let drifted_topology = factor_topology_digest(
        exposure,
        Some(&ContentHash::digest(b"drifted-factor-definition")),
    );
    assert_ne!(drifted_topology, topology_digest);
    assert_drifted_identity(
        draft,
        implementations,
        topology_role,
        drifted_topology,
        baseline_identity,
    );
}

fn assert_drifted_identity(
    draft: &PortfolioOverviewDraft,
    implementations: &[FormalImplementationBinding],
    role: &str,
    drifted_digest: ContentHash,
    baseline_identity: &ContentHash,
) {
    let mut drifted = implementations.to_vec();
    let binding = drifted
        .iter_mut()
        .find(|binding| binding.role() == role)
        .expect("implementation role");
    *binding = FormalImplementationBinding::new(role, drifted_digest)
        .expect("drifted implementation binding");
    let evidence = fixture_formal_evidence(&aggregation_owner(), draft, drifted)
        .expect("drifted formal evidence");
    assert_ne!(evidence.output_identity(), baseline_identity);
}

fn implementation_digest<'a>(
    implementations: &'a [FormalImplementationBinding],
    role: &str,
) -> &'a ContentHash {
    implementations
        .iter()
        .find(|binding| binding.role() == role)
        .expect("implementation role")
        .digest()
}

fn rates_implementation_digest(
    engine_id: &str,
    engine_version: &str,
    algorithm_id: &str,
    algorithm_version: u32,
    convention_profile: &str,
    abi_version: u32,
) -> ContentHash {
    let mut bytes = Vec::new();
    append_test_field(
        &mut bytes,
        b"ficant.portfolio.analyze-bond-implementation.v1",
    );
    append_test_field(&mut bytes, engine_id.as_bytes());
    append_test_field(&mut bytes, engine_version.as_bytes());
    append_test_field(&mut bytes, algorithm_id.as_bytes());
    append_test_field(&mut bytes, &algorithm_version.to_be_bytes());
    append_test_field(&mut bytes, convention_profile.as_bytes());
    append_test_field(&mut bytes, &abi_version.to_be_bytes());
    ContentHash::digest(&bytes)
}

fn krd_algorithm_digest(
    algorithm_id: &str,
    algorithm_version: u32,
    convention_profile: &str,
) -> ContentHash {
    let mut bytes = Vec::new();
    append_test_field(&mut bytes, b"ficant.portfolio.krd-algorithm.v1");
    append_test_field(&mut bytes, algorithm_id.as_bytes());
    append_test_field(&mut bytes, &algorithm_version.to_be_bytes());
    append_test_field(&mut bytes, convention_profile.as_bytes());
    ContentHash::digest(&bytes)
}

fn factor_topology_digest(
    exposure: &PortfolioKeyRateExposure,
    drifted_hash: Option<&ContentHash>,
) -> ContentHash {
    let mut bytes = Vec::new();
    append_test_field(&mut bytes, b"ficant.portfolio.factor-topology.v1");
    for factor in exposure.totals() {
        append_test_field(&mut bytes, factor.factor_id().as_bytes());
        append_test_field(
            &mut bytes,
            drifted_hash
                .unwrap_or_else(|| factor.factor_definition_hash())
                .as_bytes(),
        );
        append_test_field(&mut bytes, factor.unit().unit_id().as_str().as_bytes());
        append_test_field(&mut bytes, &factor.unit().version().get().to_be_bytes());
    }
    ContentHash::digest(&bytes)
}

fn append_test_field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

struct SubjectFixture;

#[async_trait]
impl SubjectRepository for SubjectFixture {
    async fn register_subject(&self, _value: SubjectRecord) -> ApplicationResult<SubjectRecord> {
        panic!("record factory performs exact reads only")
    }

    async fn get_subject(&self, reference: VersionRef) -> ApplicationResult<Option<SubjectRecord>> {
        Ok((reference == aggregation_subject()).then(aggregation_subject_record))
    }

    async fn register_subject_state(
        &self,
        _value: SubjectStateSnapshot,
    ) -> ApplicationResult<SubjectStateSnapshot> {
        panic!("record factory performs exact reads only")
    }

    async fn get_subject_state(
        &self,
        _snapshot_id: Ulid,
        _knowledge_at: chrono::DateTime<Utc>,
    ) -> ApplicationResult<Option<SubjectStateSnapshot>> {
        panic!("record factory performs exact reads only")
    }
}

struct RejectingAuthority {
    category: ApplicationErrorCategory,
    calls: AtomicUsize,
}

impl RejectingAuthority {
    fn new(category: ApplicationErrorCategory) -> Self {
        Self {
            category,
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl PortfolioAggregationAuthority for RejectingAuthority {
    async fn resolve_aggregation_inputs(
        &self,
        _principal: &AuthorizedPrincipal,
        _context: &NormalizedPortfolioContext,
    ) -> ApplicationResult<ResolvedPortfolioAggregationInputs> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ApplicationError::new(self.category, false))
    }
}

#[derive(Default)]
struct SpyPositions {
    calls: AtomicUsize,
}

#[async_trait]
impl PositionSnapshotRepository for SpyPositions {
    async fn get_position_snapshot(
        &self,
        _scope: &AccessScope,
        _snapshot_id: Ulid,
        _knowledge_at: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        panic!("position read after authority drift")
    }

    async fn resolve_position_snapshot(
        &self,
        _scope: &AccessScope,
        _subject_ref: VersionRef,
        _observed_at: MarketTime,
        _knowledge_at: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        panic!("position resolution after authority drift")
    }
}

#[derive(Default)]
struct SpyViews {
    calls: AtomicUsize,
}

impl PortfolioPositionViewsHandoff for SpyViews {
    fn project(&self, _snapshot: PositionSnapshot) -> ApplicationResult<PositionViews> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        panic!("PositionViews after authority drift")
    }
}

#[derive(Default)]
struct SpyRisk {
    calls: AtomicUsize,
}

#[async_trait]
impl PortfolioRiskHandoff for SpyRisk {
    async fn calculate(
        &self,
        _scope: &AccessScope,
        _context: &NormalizedPortfolioContext,
        _snapshot: &PositionSnapshot,
        _authority: &PortfolioRiskAuthority,
    ) -> ApplicationResult<PortfolioRiskAnalysis> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        panic!("PortfolioRisk after authority drift")
    }
}

#[derive(Default)]
struct SpyBonds {
    calls: AtomicUsize,
}

#[async_trait]
impl PortfolioBondAnalysisHandoff for SpyBonds {
    async fn analyze(
        &self,
        _scope: &AccessScope,
        _context: &NormalizedPortfolioContext,
        _snapshot: &PositionSnapshot,
        _position: &Position,
        _authority: &PortfolioBondRatesAuthorityResolution,
        _authority_fingerprint: &ficant_application::ports::OperationFingerprint,
    ) -> ApplicationResult<PortfolioBondAnalysis> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        panic!("AnalyzeBond after authority drift")
    }
}

#[derive(Default)]
struct SpyPublisher {
    calls: AtomicUsize,
}

#[async_trait]
impl PortfolioOverviewPublisher for SpyPublisher {
    async fn publish(
        &self,
        _scope: &AccessScope,
        _owner: &OwnerRef,
        _draft: &PortfolioOverviewDraft,
    ) -> ApplicationResult<FormalOutputEvidence> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        panic!("formal publication after authority drift")
    }
}

fn aggregation_context() -> NormalizedPortfolioContext {
    let portfolio = exact_lineage(id(6), b"portfolio");
    NormalizedPortfolioContext {
        owner: aggregation_owner(),
        subject_ref: VersionRef::new(id(4), Version::new(1).expect("subject version")),
        scope: ExactPortfolioScope::new(
            ExactPortfolioScopeKind::Portfolio(portfolio.clone()),
            vec![portfolio],
        ),
        valuation_at: aggregation_time(20, 9),
        knowledge_at: aggregation_time(21, 9),
        currency: PortfolioCurrencyMode::Original,
        currency_unit: money_unit(),
        look_through: PortfolioLookThroughMode::None,
        benchmark: BenchmarkRef::new(
            VersionRef::new(id(7), Version::new(1).expect("benchmark version")),
            ContentHash::digest(b"benchmark"),
        ),
        period: PortfolioPeriodPreset::OneDay,
        period_from: aggregation_time(19, 9),
        period_to: aggregation_time(20, 9),
        metric_convention: PortfolioMetricConventionRef::new(
            VersionRef::new(id(8), Version::new(1).expect("convention version")),
            ContentHash::digest(b"convention"),
        ),
    }
}

fn aggregation_principal() -> AuthorizedPrincipal {
    AuthorizedPrincipal::new(
        "researcher@example.test".to_owned(),
        id(3),
        aggregation_owner().tenant_id().clone(),
        vec![aggregation_owner().owner_id().clone()],
        PlatformRole::Researcher,
        vec!["portfolio:read".to_owned()],
        ContentHash::digest(b"credential"),
    )
    .expect("aggregation principal")
}

fn aggregation_owner() -> OwnerRef {
    OwnerRef::new(id(1), id(2))
}

fn aggregation_subject() -> VersionRef {
    VersionRef::new(id(4), Version::new(1).expect("subject version"))
}

fn aggregation_subject_record() -> SubjectRecord {
    let subject = Subject::new_owned(id(4), aggregation_owner(), "Portfolio360 Subject")
        .expect("owned subject");
    let version = SubjectVersion::new(
        aggregation_subject(),
        AccessSet::new(["CN"], ["portfolio360"]).expect("subject access"),
        FundingTier::ROnly,
        TaxTreatment::new("fixture-vat", "fixture-income").expect("tax treatment"),
        "fixture-assessment",
        "fixture-liability",
        None,
    )
    .expect("subject version");
    SubjectRecord::new(subject, version).expect("subject record")
}

fn aggregation_subject_hash() -> ContentHash {
    ficant_application::ports::subject_record_content_hash(&aggregation_subject_record())
        .expect("subject content hash")
}

fn exact_lineage(value: Ulid, content: &[u8]) -> LineageRef {
    LineageRef::new(
        value,
        Some(Version::new(1).expect("lineage version")),
        Some(ContentHash::digest(content)),
    )
    .expect("exact lineage")
}

fn aggregation_time(day: u32, hour: u32) -> MarketTime {
    let instant = Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0).unwrap();
    let local_date = instant
        .with_timezone(&chrono_tz::Asia::Shanghai)
        .date_naive();
    MarketTime::new(instant, "Asia/Shanghai", local_date).expect("aggregation time")
}

fn drifted_market_times(value: &MarketTime) -> Vec<MarketTime> {
    let subsecond = value.instant() + chrono::Duration::nanoseconds(1);
    let prior_day = value.instant() - chrono::Duration::days(1);
    vec![
        MarketTime::new(
            subsecond,
            value.market_timezone(),
            subsecond
                .with_timezone(&chrono_tz::Asia::Shanghai)
                .date_naive(),
        )
        .expect("subsecond drift"),
        MarketTime::new(
            value.instant(),
            "UTC",
            value.instant().with_timezone(&Utc).date_naive(),
        )
        .expect("timezone drift"),
        MarketTime::new(
            prior_day,
            value.market_timezone(),
            prior_day
                .with_timezone(&chrono_tz::Asia::Shanghai)
                .date_naive(),
        )
        .expect("local-date drift"),
    ]
}

fn market_time_from_instant(instant: chrono::DateTime<Utc>, timezone: &str) -> MarketTime {
    let zone = timezone.parse::<chrono_tz::Tz>().expect("fixture timezone");
    MarketTime::new(instant, timezone, instant.with_timezone(&zone).date_naive())
        .expect("fixture market time")
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("fixture date")
}

fn assert_aggregate(
    positions: &[PortfolioMetricPosition],
    expected: &Value,
    output_scale: u32,
    expected_mode: PortfolioMetricDataMode,
) {
    let result = aggregate_portfolio_metrics(
        positions,
        &krd_from_expected(expected),
        scales(output_scale),
    )
    .expect("production aggregate");
    assert_eq!(result.data_mode(), expected_mode);
    assert!(result.coverage().missing_reasons().is_empty());
    assert_eq!(
        result
            .coverage()
            .weighted_average_participating_position_count(),
        required_u64(
            required_object(expected, "coverage"),
            "weighted_average_participating_position_count"
        )
    );
    let expected_metrics = required_object(expected, "basic_metrics");
    let actual = result.basic_metrics();
    for (name, value) in [
        ("market_value", actual.market_value()),
        ("economic_pnl", actual.economic_pnl()),
        ("weighted_ytm", actual.weighted_ytm().expect("ytm")),
        (
            "modified_duration",
            actual.modified_duration().expect("duration"),
        ),
        ("convexity", actual.convexity().expect("convexity")),
        (
            "weighted_coupon_rate",
            actual.weighted_coupon_rate().expect("coupon"),
        ),
        (
            "weighted_remaining_years",
            actual.weighted_remaining_years().expect("remaining years"),
        ),
        ("dv01", actual.dv01()),
    ] {
        assert_eq!(
            render(value, output_scale),
            required_str(expected_metrics, name),
            "metric {name}"
        );
    }
    let expected_krd = required_object(expected, "krd_summary");
    assert_eq!(
        render(result.krd_summary().parallel_dv01(), output_scale),
        required_str(expected_krd, "parallel_dv01")
    );
    for (actual, expected) in result
        .krd_summary()
        .totals()
        .iter()
        .zip(required_array(expected_krd, "factor_totals"))
    {
        assert_eq!(actual.factor_id(), required_str(expected, "factor_id"));
        assert_eq!(
            render_fixed(actual.value(), output_scale),
            required_str(expected, "dv01")
        );
    }
}

fn positions_for(
    inputs: &Value,
    positions: &[Value],
    id_offset: usize,
) -> Vec<PortfolioMetricPosition> {
    positions
        .iter()
        .enumerate()
        .map(|(index, position)| position_from(inputs, position, id_offset + index, false))
        .collect()
}

fn inverse_positions_for(
    inputs: &Value,
    positions: &[Value],
    id_offset: usize,
) -> Vec<PortfolioMetricPosition> {
    positions
        .iter()
        .enumerate()
        .map(|(index, position)| position_from(inputs, position, id_offset + index, true))
        .collect()
}

fn position_from(
    inputs: &Value,
    position: &Value,
    index: usize,
    inverse: bool,
) -> PortfolioMetricPosition {
    let instrument_id = required_str(position, "instrument_id");
    let bond = required_array(inputs, "bonds")
        .iter()
        .find(|candidate| required_str(candidate, "instrument_id") == instrument_id)
        .expect("bond fixture");
    let quantity = fixed(required_str(position, "quantity"));
    let quantity = if inverse {
        FixedDecimal::ZERO
            .checked_sub(quantity)
            .expect("inverse quantity")
    } else {
        quantity
    };
    let metrics = PortfolioBondMetricFacts::new(
        fixed(required_str(bond, "ytm")),
        fixed(required_str(bond, "modified_duration")),
        fixed(required_str(bond, "convexity")),
        fixed(required_str(bond, "coupon_rate")),
        fixed(required_str(bond, "remaining_years")),
        weighted_units(),
    )
    .expect("bond metrics");
    PortfolioMetricPosition::from_per_quantity(
        id(u8::try_from(index + 2).expect("fixture id suffix")),
        quantity,
        fixed(required_str(bond, "notional_per_quantity")),
        fixed(required_str(bond, "market_value_per_quantity")),
        fixed(required_str(bond, "economic_pnl_per_quantity")),
        money_unit(),
        PortfolioWeightedMetricEligibility::Bond(Box::new(metrics)),
    )
    .expect("metric position")
}

fn krd_from_expected(expected: &Value) -> Vec<FactorDv01> {
    required_array(required_object(expected, "krd_summary"), "factor_totals")
        .iter()
        .map(|factor| {
            FactorDv01::new(
                required_str(factor, "factor_id"),
                ContentHash::digest(required_str(factor, "factor_id").as_bytes()),
                fixed(required_str(factor, "dv01")),
                dv01_unit(),
            )
            .expect("factor")
        })
        .collect()
}

fn krd(value: &str) -> Vec<FactorDv01> {
    vec![
        FactorDv01::new(
            "CGB-2Y",
            ContentHash::digest(b"CGB-2Y"),
            fixed(value),
            dv01_unit(),
        )
        .expect("factor"),
    ]
}

fn fixed(value: &str) -> FixedDecimal {
    let negative = value.starts_with('-');
    let unsigned = value.trim_start_matches(['-', '+']);
    let (whole, fractional) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    assert!(fractional.len() <= 12, "fixture fixed scale");
    let mut digits = format!("{whole}{fractional}");
    digits.extend(std::iter::repeat_n('0', 12 - fractional.len()));
    let magnitude = digits.parse::<i128>().expect("fixed coefficient");
    FixedDecimal::from_scaled(if negative { -magnitude } else { magnitude })
}

fn decimal(value: &str, unit: UnitRef) -> DecimalValue {
    let negative = value.starts_with('-');
    let unsigned = value.trim_start_matches(['-', '+']);
    let (whole, fractional) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let coefficient = format!("{}{}{}", if negative { "-" } else { "" }, whole, fractional);
    DecimalValue::new(
        coefficient,
        u32::try_from(fractional.len()).expect("decimal scale"),
        unit,
    )
    .expect("decimal")
}

fn render(value: &DecimalValue, output_scale: u32) -> String {
    let coefficient = value.coefficient();
    let negative = coefficient.starts_with('-');
    let mut digits = coefficient.trim_start_matches('-').to_owned();
    let scale = usize::try_from(value.scale()).expect("scale");
    let output_scale = usize::try_from(output_scale).expect("output scale");
    assert!(scale <= output_scale);
    if digits.len() <= scale {
        digits = format!("{}{}", "0".repeat(scale + 1 - digits.len()), digits);
    }
    let split = digits.len() - scale;
    let mut rendered = if scale == 0 {
        digits
    } else {
        format!("{}.{}", &digits[..split], &digits[split..])
    };
    if output_scale > scale {
        if scale == 0 {
            rendered.push('.');
        }
        rendered.push_str(&"0".repeat(output_scale - scale));
    }
    if negative
        && rendered
            .chars()
            .any(|character| character.is_ascii_digit() && character != '0')
    {
        rendered.insert(0, '-');
    }
    rendered
}

fn render_fixed(value: FixedDecimal, output_scale: u32) -> String {
    let decimal =
        DecimalValue::new(value.scaled().to_string(), 12, dv01_unit()).expect("fixed decimal");
    render(&decimal, output_scale)
}

fn weighted_units() -> PortfolioWeightedMetricUnits {
    PortfolioWeightedMetricUnits::new(
        rate_unit(),
        duration_unit(),
        convexity_unit(),
        rate_unit(),
        duration_unit(),
    )
}

fn money_unit() -> UnitRef {
    unit("01ARZ3NDEKTSV4RRFFQ69G5FAV")
}

fn alternate_money_unit() -> UnitRef {
    unit("01ARZ3NDEKTSV4RRFFQ69G5FAW")
}

fn dv01_unit() -> UnitRef {
    unit("01ARZ3NDEKTSV4RRFFQ69G5FAX")
}

fn rate_unit() -> UnitRef {
    unit("01ARZ3NDEKTSV4RRFFQ69G5FAY")
}

fn duration_unit() -> UnitRef {
    unit("01ARZ3NDEKTSV4RRFFQ69G5FAZ")
}

fn convexity_unit() -> UnitRef {
    unit("01ARZ3NDEKTSV4RRFFQ69G5FB0")
}

fn unit(value: &str) -> UnitRef {
    UnitRef::new(
        Ulid::new(value).expect("unit ULID"),
        Version::new(1).expect("unit version"),
    )
}

fn scales(scale: u32) -> PortfolioMetricOutputScales {
    PortfolioMetricOutputScales::new(scale, scale, scale, scale, scale)
        .expect("fixture output scales")
}

fn id(suffix: u8) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F{suffix:02X}")).expect("position ULID")
}

fn required_object<'a>(value: &'a Value, field: &str) -> &'a Value {
    value
        .get(field)
        .unwrap_or_else(|| panic!("missing {field}"))
}

fn required_array<'a>(value: &'a Value, field: &str) -> &'a [Value] {
    required_object(value, field)
        .as_array()
        .unwrap_or_else(|| panic!("{field} array"))
}

fn required_str<'a>(value: &'a Value, field: &str) -> &'a str {
    required_object(value, field)
        .as_str()
        .unwrap_or_else(|| panic!("{field} string"))
}

fn required_u64(value: &Value, field: &str) -> u64 {
    required_object(value, field)
        .as_u64()
        .unwrap_or_else(|| panic!("{field} u64"))
}
