use chrono::{TimeZone, Utc};
use ficant_domain::portfolio::{
    Benchmark, BenchmarkInput, BenchmarkRef, Book, BookInput, Portfolio, PortfolioDecimalRounding,
    PortfolioGroup, PortfolioGroupInput, PortfolioInput, PortfolioMetricConvention,
    PortfolioMetricConventionInput, PortfolioMetricConventionRef, PortfolioMetricWeighting,
    PortfolioSnapshotBinding, PortfolioStatus,
};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use ficant_domain::{ContentAddressed, DomainErrorCode};

#[test]
fn directory_objects_are_double_time_scoped_and_content_addressed() {
    let mut input = book_input();
    input.content_hash = Book::content_hash_for(&input);
    let book = Book::new(input.clone()).unwrap();
    assert_eq!(book.owner(), &owner());
    assert_eq!(book.subject_ref(), &version_ref('S'));
    assert_eq!(book.status(), PortfolioStatus::Active);
    assert_eq!(book.content_hash(), &input.content_hash);

    let mut changed = input.clone();
    changed.display_name = "Changed".to_owned();
    assert_ne!(Book::content_hash_for(&changed), input.content_hash);

    input.content_hash = ContentHash::digest(b"wrong");
    assert_eq!(
        Book::new(input).unwrap_err(),
        DomainErrorCode::ContentHashMismatch
    );

    let mut inverted = book_input();
    inverted.effective_from = market_time(3);
    inverted.effective_to = market_time(2);
    inverted.content_hash = Book::content_hash_for(&inverted);
    assert_eq!(
        Book::new(inverted).unwrap_err(),
        DomainErrorCode::InvalidEffectiveTime
    );
}

#[test]
fn hierarchy_requires_exact_book_group_and_snapshot_bindings() {
    let exact_book = exact_ref('B');
    let mut group_input = PortfolioGroupInput {
        group: version_ref('G'),
        owner: owner(),
        subject_ref: version_ref('S'),
        book: exact_book.clone(),
        parent_group: None,
        code: "GOV".to_owned(),
        display_name: "Government".to_owned(),
        status: PortfolioStatus::Active,
        effective_from: market_time(1),
        effective_to: market_time(3),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    group_input.content_hash = PortfolioGroup::content_hash_for(&group_input);
    let group = PortfolioGroup::new(group_input.clone()).unwrap();
    assert_eq!(group.book(), &exact_book);

    group_input.book = LineageRef::versioned(id('B'), version());
    group_input.content_hash = PortfolioGroup::content_hash_for(&group_input);
    assert_eq!(
        PortfolioGroup::new(group_input).unwrap_err(),
        DomainErrorCode::BrokenLineage
    );

    let snapshot = PortfolioSnapshotBinding::new(
        id('P'),
        ContentHash::digest(b"positions"),
        market_time(1),
        market_time(2),
    )
    .unwrap();
    let mut portfolio_input = PortfolioInput {
        portfolio: version_ref('R'),
        owner: owner(),
        subject_ref: version_ref('S'),
        book: exact_ref('B'),
        group: exact_ref('G'),
        code: "CGB-CORE".to_owned(),
        display_name: "CGB Core".to_owned(),
        status: PortfolioStatus::Active,
        position_snapshot: snapshot,
        benchmark: BenchmarkRef::new(version_ref('E'), ContentHash::digest(b"benchmark")),
        metric_convention: PortfolioMetricConventionRef::new(
            version_ref('C'),
            ContentHash::digest(b"convention"),
        ),
        effective_from: market_time(1),
        effective_to: market_time(3),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    portfolio_input.content_hash = Portfolio::content_hash_for(&portfolio_input);
    let portfolio = Portfolio::new(portfolio_input.clone()).unwrap();
    assert_eq!(
        portfolio.position_snapshot().content_hash(),
        &ContentHash::digest(b"positions")
    );
    assert_eq!(portfolio.benchmark().reference(), &version_ref('E'));

    portfolio_input.position_snapshot = PortfolioSnapshotBinding::new(
        id('P'),
        ContentHash::digest(b"drift"),
        market_time(1),
        market_time(2),
    )
    .unwrap();
    assert_eq!(
        Portfolio::new(portfolio_input).unwrap_err(),
        DomainErrorCode::ContentHashMismatch
    );
}

#[test]
fn metric_convention_closes_p0_weighting_rounding_and_freshness() {
    let mut input = convention_input();
    input.content_hash = PortfolioMetricConvention::content_hash_for(&input);
    let convention = PortfolioMetricConvention::new(input.clone()).unwrap();
    assert_eq!(
        convention.schema_id(),
        "ficant.portfolio-metric-convention.v1"
    );
    assert_eq!(
        convention.ytm_weighting(),
        PortfolioMetricWeighting::MarketValueTimesModifiedDuration
    );
    assert_eq!(
        convention.duration_weighting(),
        PortfolioMetricWeighting::MarketValue
    );
    assert_eq!(
        convention.coupon_weighting(),
        PortfolioMetricWeighting::Notional
    );
    assert_eq!(convention.rounding(), PortfolioDecimalRounding::TiesToEven);
    assert_eq!(convention.freshness_limit_seconds(), 86_400);

    let mutations: [fn(&mut PortfolioMetricConventionInput); 4] = [
        |value: &mut PortfolioMetricConventionInput| value.schema_id = "ficant.other.v1".to_owned(),
        |value: &mut PortfolioMetricConventionInput| {
            value.ytm_weighting = PortfolioMetricWeighting::MarketValue;
        },
        |value: &mut PortfolioMetricConventionInput| {
            value.rounding = PortfolioDecimalRounding::Unspecified;
        },
        |value: &mut PortfolioMetricConventionInput| value.freshness_limit_seconds = 0,
    ];
    for mutation in mutations {
        let mut invalid = input.clone();
        mutation(&mut invalid);
        invalid.content_hash = PortfolioMetricConvention::content_hash_for(&invalid);
        assert_eq!(
            PortfolioMetricConvention::new(invalid).unwrap_err(),
            DomainErrorCode::InvalidValue
        );
    }
}

#[test]
fn snapshot_binding_rejects_visible_time_before_observed_time() {
    assert_eq!(
        PortfolioSnapshotBinding::new(
            id('P'),
            ContentHash::digest(b"positions"),
            market_time(2),
            market_time(1),
        )
        .unwrap_err(),
        DomainErrorCode::InvalidEffectiveTime
    );
}

#[test]
fn benchmark_catalog_record_binds_one_exact_position_snapshot() {
    let snapshot = PortfolioSnapshotBinding::new(
        id('P'),
        ContentHash::digest(b"benchmark-positions"),
        market_time(1),
        market_time(2),
    )
    .unwrap();
    let mut input = BenchmarkInput {
        benchmark: version_ref('E'),
        owner: owner(),
        subject_ref: version_ref('S'),
        code: "CGB-BENCHMARK".to_owned(),
        display_name: "CGB Benchmark".to_owned(),
        position_snapshot: snapshot,
        effective_from: market_time(1),
        effective_to: market_time(3),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = Benchmark::content_hash_for(&input);
    let benchmark = Benchmark::new(input.clone()).unwrap();
    assert_eq!(benchmark.owner(), &owner());
    assert_eq!(benchmark.subject_ref(), &version_ref('S'));
    assert_eq!(
        benchmark.position_snapshot().content_hash(),
        &ContentHash::digest(b"benchmark-positions")
    );

    input.position_snapshot = PortfolioSnapshotBinding::new(
        id('P'),
        ContentHash::digest(b"drift"),
        market_time(1),
        market_time(2),
    )
    .unwrap();
    assert_eq!(
        Benchmark::new(input).unwrap_err(),
        DomainErrorCode::ContentHashMismatch
    );
}

fn book_input() -> BookInput {
    BookInput {
        book: version_ref('B'),
        owner: owner(),
        subject_ref: version_ref('S'),
        code: "BOOK-CGB".to_owned(),
        display_name: "CGB Book".to_owned(),
        status: PortfolioStatus::Active,
        effective_from: market_time(1),
        effective_to: market_time(3),
        content_hash: ContentHash::digest(b"placeholder"),
    }
}

fn convention_input() -> PortfolioMetricConventionInput {
    PortfolioMetricConventionInput {
        convention: version_ref('C'),
        owner: owner(),
        schema_id: "ficant.portfolio-metric-convention.v1".to_owned(),
        ytm_weighting: PortfolioMetricWeighting::MarketValueTimesModifiedDuration,
        duration_weighting: PortfolioMetricWeighting::MarketValue,
        convexity_weighting: PortfolioMetricWeighting::MarketValue,
        coupon_weighting: PortfolioMetricWeighting::Notional,
        remaining_life_weighting: PortfolioMetricWeighting::Notional,
        rounding: PortfolioDecimalRounding::TiesToEven,
        freshness_limit_seconds: 86_400,
        effective_from: market_time(1),
        effective_to: market_time(3),
        content_hash: ContentHash::digest(b"placeholder"),
    }
}

fn exact_ref(suffix: char) -> LineageRef {
    LineageRef::new(
        id(suffix),
        Some(version()),
        Some(ContentHash::digest(format!("exact-{suffix}").as_bytes())),
    )
    .unwrap()
}

fn market_time(hour: u32) -> MarketTime {
    let instant = Utc.with_ymd_and_hms(2026, 8, 21, hour, 0, 0).unwrap();
    MarketTime::new(
        instant,
        "Asia/Shanghai",
        chrono::NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
    )
    .unwrap()
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
        'O' => '0',
        'T' => '1',
        'S' => '2',
        'B' => '3',
        'G' => '4',
        'P' => '5',
        'R' => '6',
        'E' => '7',
        'C' => '8',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
