use chrono::{NaiveDate, TimeZone, Utc};
use ficant_domain::portfolio::{
    BenchmarkLevelSnapshot, BenchmarkLevelSnapshotInput, BenchmarkSessionLevel,
    PortfolioDailyPerformancePoint, PortfolioDecimalRounding, PortfolioExternalFlowTiming,
    PortfolioPerformanceConvention, PortfolioPerformanceConventionInput,
    PortfolioPerformanceConventionRef, PortfolioPerformanceReturnMethod, PortfolioSessionAggregate,
    PortfolioSnapshotBinding, PortfolioValuationFrequency, PortfolioValuationSnapshot,
    PortfolioValuationSnapshotInput, calculate_daily_performance,
};
use ficant_domain::primitives::{
    ContentHash, FixedDecimal, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::{ContentAddressed, DomainErrorCode};

#[test]
fn convention_and_snapshots_are_exact_content_addressed_inputs() {
    let mut convention_input = PortfolioPerformanceConventionInput {
        convention: version_ref('C'),
        owner: owner(),
        schema_id: "ficant.portfolio-performance-convention.v1".to_owned(),
        calendar: exact_ref('A'),
        return_method: PortfolioPerformanceReturnMethod::DailyTimeWeighted,
        flow_timing: PortfolioExternalFlowTiming::EndOfDay,
        valuation_frequency: PortfolioValuationFrequency::CalendarSessionClose,
        rounding: PortfolioDecimalRounding::TiesToEven,
        effective_from: market_time(20, 15),
        effective_to: market_time(23, 15),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    convention_input.content_hash =
        PortfolioPerformanceConvention::content_hash_for(&convention_input);
    let convention = PortfolioPerformanceConvention::new(convention_input.clone()).unwrap();
    assert_eq!(convention.calendar(), &exact_ref('A'));
    assert_eq!(convention.content_hash(), &convention_input.content_hash);

    let mut valuation_input = valuation_input();
    valuation_input.content_hash = PortfolioValuationSnapshot::content_hash_for(&valuation_input);
    let valuation = PortfolioValuationSnapshot::new(valuation_input.clone()).unwrap();
    assert_eq!(valuation.net_asset_value(), amount(100));
    assert_eq!(valuation.gross_assets(), amount(120));
    assert_eq!(valuation.liabilities(), amount(20));

    valuation_input.net_asset_value = amount(99);
    valuation_input.content_hash = PortfolioValuationSnapshot::content_hash_for(&valuation_input);
    assert_eq!(
        PortfolioValuationSnapshot::new(valuation_input).unwrap_err(),
        DomainErrorCode::InvalidValue
    );

    let mut level_input = BenchmarkLevelSnapshotInput {
        snapshot_id: id('L'),
        owner: owner(),
        subject_ref: version_ref('S'),
        benchmark: exact_ref('B'),
        valuation_at: market_time(20, 15),
        visible_at: market_time(20, 16),
        level_unit: unit('D'),
        level: amount(100),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    level_input.content_hash = BenchmarkLevelSnapshot::content_hash_for(&level_input);
    assert_eq!(
        BenchmarkLevelSnapshot::new(level_input).unwrap().level(),
        amount(100)
    );
}

#[test]
fn end_of_day_flow_and_geometric_compounding_are_exact() {
    let portfolio = vec![
        PortfolioSessionAggregate::new(market_time(20, 15), amount(100), FixedDecimal::ZERO)
            .unwrap(),
        PortfolioSessionAggregate::new(market_time(21, 15), amount(110), FixedDecimal::ZERO)
            .unwrap(),
        PortfolioSessionAggregate::new(market_time(22, 15), amount(110), amount(-11)).unwrap(),
    ];
    let benchmark = vec![
        BenchmarkSessionLevel::new(market_time(20, 15), amount(100)).unwrap(),
        BenchmarkSessionLevel::new(market_time(21, 15), scaled("105.000000000000")).unwrap(),
        BenchmarkSessionLevel::new(market_time(22, 15), scaled("110.250000000000")).unwrap(),
    ];

    let points = calculate_daily_performance(&portfolio, &benchmark).unwrap();
    assert_eq!(points.len(), 2);
    assert_point(
        &points[0],
        amount(10),
        scaled("0.100000000000"),
        scaled("0.050000000000"),
        scaled("0.100000000000"),
        scaled("0.050000000000"),
    );
    assert_point(
        &points[1],
        amount(11),
        scaled("0.100000000000"),
        scaled("0.050000000000"),
        scaled("0.210000000000"),
        scaled("0.102500000000"),
    );
    assert_eq!(
        points[1].active_cumulative_return(),
        scaled("0.107500000000")
    );
}

#[test]
fn ties_to_even_multiplication_is_explicit_and_does_not_change_exact_multiply() {
    let half_even_down = FixedDecimal::from_scaled(1)
        .checked_mul_round_ties_even(FixedDecimal::from_scaled(500_000_000_000))
        .unwrap();
    let half_even_up = FixedDecimal::from_scaled(3)
        .checked_mul_round_ties_even(FixedDecimal::from_scaled(500_000_000_000))
        .unwrap();
    assert_eq!(half_even_down.scaled(), 0);
    assert_eq!(half_even_up.scaled(), 2);
    assert_eq!(
        FixedDecimal::from_scaled(1).checked_mul(FixedDecimal::from_scaled(500_000_000_000)),
        Err(DomainErrorCode::InvalidValue)
    );
}

#[test]
fn production_formula_matches_independent_decimal_oracle_for_two_member_group() {
    const INPUTS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/oracle/portfolio/r8b_portfolio_performance_inputs.json"
    ));
    const EXPECTED: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/oracle/portfolio/r8b_portfolio_performance_expected.json"
    ));
    assert!(INPUTS.contains("\"flow_timing\": \"END_OF_DAY\""));
    assert!(INPUTS.contains("\"rounding\": \"TIES_TO_EVEN\""));
    assert!(EXPECTED.contains("ficant.portfolio.performance-oracle-expected.v1"));

    let dates = json_string_array(INPUTS, "session_local_dates")
        .iter()
        .map(|value| {
            value[8..10]
                .parse::<u32>()
                .map(|day| market_time(day, 15))
                .unwrap()
        })
        .collect::<Vec<_>>();
    let portfolio_a = input_series(INPUTS, "portfolio_a", &dates);
    let portfolio_b = input_series(INPUTS, "portfolio_b", &dates);
    let benchmark = json_string_array(INPUTS, "benchmark_levels")
        .into_iter()
        .zip(&dates)
        .map(|(level, date)| BenchmarkSessionLevel::new(date.clone(), scaled(&level)).unwrap())
        .collect::<Vec<_>>();
    let group = portfolio_a
        .iter()
        .zip(&portfolio_b)
        .map(|(left, right)| {
            PortfolioSessionAggregate::new(
                left.valuation_at().clone(),
                left.net_asset_value()
                    .checked_add(right.net_asset_value())
                    .unwrap(),
                left.net_external_flow()
                    .checked_add(right.net_external_flow())
                    .unwrap(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();

    for (prefix, series) in [
        ("portfolio_a", portfolio_a),
        ("portfolio_b", portfolio_b),
        ("group", group),
    ] {
        let points = calculate_daily_performance(&series, &benchmark).unwrap();
        assert_oracle_points(prefix, &points, EXPECTED);
    }
}

fn input_series(
    document: &str,
    prefix: &str,
    dates: &[MarketTime],
) -> Vec<PortfolioSessionAggregate> {
    let nav = json_string_array(document, &format!("{prefix}_nav"));
    let flow = json_string_array(document, &format!("{prefix}_flow"));
    assert_eq!(nav.len(), dates.len());
    assert_eq!(flow.len(), dates.len());
    dates
        .iter()
        .zip(nav)
        .zip(flow)
        .map(|((date, nav), flow)| {
            PortfolioSessionAggregate::new(date.clone(), scaled(&nav), scaled(&flow)).unwrap()
        })
        .collect()
}

fn assert_oracle_points(prefix: &str, points: &[PortfolioDailyPerformancePoint], expected: &str) {
    let actual = |field: fn(&PortfolioDailyPerformancePoint) -> FixedDecimal| {
        points
            .iter()
            .map(|point| render_scaled(field(point)))
            .collect::<Vec<_>>()
    };
    for (name, values) in [
        (
            "opening_nav",
            actual(PortfolioDailyPerformancePoint::opening_nav),
        ),
        (
            "ending_nav",
            actual(PortfolioDailyPerformancePoint::ending_nav),
        ),
        (
            "net_external_flow",
            actual(PortfolioDailyPerformancePoint::net_external_flow),
        ),
        (
            "economic_pnl",
            actual(PortfolioDailyPerformancePoint::economic_pnl),
        ),
        (
            "daily_return",
            actual(PortfolioDailyPerformancePoint::daily_return),
        ),
        (
            "benchmark_return",
            actual(PortfolioDailyPerformancePoint::benchmark_return),
        ),
        (
            "active_return",
            actual(PortfolioDailyPerformancePoint::active_return),
        ),
        (
            "cumulative_return",
            actual(PortfolioDailyPerformancePoint::cumulative_return),
        ),
        (
            "benchmark_cumulative_return",
            actual(PortfolioDailyPerformancePoint::benchmark_cumulative_return),
        ),
        (
            "active_cumulative_return",
            actual(PortfolioDailyPerformancePoint::active_cumulative_return),
        ),
    ] {
        assert_eq!(
            values,
            json_string_array(expected, &format!("{prefix}_{name}")),
            "{prefix}_{name} drifted from the independent Decimal witness"
        );
    }
}

fn json_string_array(document: &str, key: &str) -> Vec<String> {
    let marker = format!("\"{key}\"");
    let tail = document
        .split_once(&marker)
        .unwrap_or_else(|| panic!("missing frozen Oracle key {key}"))
        .1;
    let array = tail
        .split_once('[')
        .and_then(|(_, value)| value.split_once(']'))
        .map_or_else(
            || panic!("invalid frozen Oracle array {key}"),
            |(value, _)| value,
        );
    array
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_matches('"').to_owned())
        .collect()
}

fn render_scaled(value: FixedDecimal) -> String {
    let scaled = value.scaled();
    let sign = if scaled < 0 { "-" } else { "" };
    let magnitude = scaled.abs();
    format!(
        "{sign}{}.{:012}",
        magnitude / 1_000_000_000_000,
        magnitude % 1_000_000_000_000
    )
}

fn assert_point(
    point: &PortfolioDailyPerformancePoint,
    pnl: FixedDecimal,
    daily: FixedDecimal,
    benchmark: FixedDecimal,
    cumulative: FixedDecimal,
    benchmark_cumulative: FixedDecimal,
) {
    assert_eq!(point.economic_pnl(), pnl);
    assert_eq!(point.daily_return(), daily);
    assert_eq!(point.benchmark_return(), benchmark);
    assert_eq!(point.active_return(), daily.checked_sub(benchmark).unwrap());
    assert_eq!(point.cumulative_return(), cumulative);
    assert_eq!(point.benchmark_cumulative_return(), benchmark_cumulative);
}

fn valuation_input() -> PortfolioValuationSnapshotInput {
    PortfolioValuationSnapshotInput {
        snapshot_id: id('V'),
        owner: owner(),
        subject_ref: version_ref('S'),
        portfolio: exact_ref('P'),
        position_snapshot: PortfolioSnapshotBinding::new(
            id('N'),
            ContentHash::digest(b"positions"),
            market_time(20, 14),
            market_time(20, 14),
        )
        .unwrap(),
        performance_convention: PortfolioPerformanceConventionRef::new(
            version_ref('C'),
            ContentHash::digest(b"performance-convention"),
        ),
        valuation_at: market_time(20, 15),
        visible_at: market_time(20, 16),
        currency_unit: unit('U'),
        gross_assets: amount(120),
        liabilities: amount(20),
        net_asset_value: amount(100),
        net_external_flow: FixedDecimal::ZERO,
        content_hash: ContentHash::digest(b"placeholder"),
    }
}

fn amount(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value * 1_000_000_000_000)
}

fn scaled(value: &str) -> FixedDecimal {
    let (sign, unsigned) = value
        .strip_prefix('-')
        .map_or((1_i128, value), |unsigned| (-1_i128, unsigned));
    let (whole, fraction) = unsigned.split_once('.').unwrap();
    assert_eq!(fraction.len(), 12);
    let magnitude =
        whole.parse::<i128>().unwrap() * 1_000_000_000_000 + fraction.parse::<i128>().unwrap();
    FixedDecimal::from_scaled(sign * magnitude)
}

fn market_time(day: u32, hour: u32) -> MarketTime {
    let instant = Utc.with_ymd_and_hms(2026, 8, day, hour - 8, 0, 0).unwrap();
    MarketTime::new(
        instant,
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 8, day).unwrap(),
    )
    .unwrap()
}

fn exact_ref(suffix: char) -> LineageRef {
    LineageRef::new(
        id(suffix),
        Some(version()),
        Some(ContentHash::digest(format!("exact-{suffix}").as_bytes())),
    )
    .unwrap()
}

fn unit(suffix: char) -> UnitRef {
    UnitRef::new(id(suffix), version())
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('O'))
}

fn version_ref(suffix: char) -> VersionRef {
    VersionRef::new(id(suffix), version())
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'T' => '0',
        'O' => '1',
        'S' => '2',
        'A' => '3',
        'B' => '4',
        'C' => '5',
        'P' => '6',
        'N' => '7',
        'V' => '8',
        'L' => '9',
        'U' => 'A',
        'D' => 'B',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
