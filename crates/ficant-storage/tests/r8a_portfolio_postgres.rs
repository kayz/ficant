mod support;

use chrono::{NaiveDate, TimeZone, Timelike, Utc};
use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    ExactCatalogRead, PortfolioAnalyticsAuthorityCandidate, PortfolioAnalyticsAuthorityQuery,
    PortfolioAnalyticsAuthorityRepository, PortfolioBondRatesAuthorityCandidate,
    PortfolioCatalogRepository, PortfolioCatalogTemporalScope, PortfolioImmutableSnapshotAuthority,
    PortfolioRatesUnitRole, PortfolioScopeSelector, PortfolioUnitAuthorityBinding,
    PortfolioValuationAuthorityBinding,
};
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, CalendarRequirement, FixedDecimal,
};
use ficant_domain::portfolio::{
    Benchmark, BenchmarkInput, BenchmarkRef, Book, BookInput, Portfolio, PortfolioDecimalRounding,
    PortfolioGroup, PortfolioGroupInput, PortfolioInput, PortfolioMetricConvention,
    PortfolioMetricConventionInput, PortfolioMetricConventionRef, PortfolioMetricWeighting,
    PortfolioSnapshotBinding, PortfolioStatus,
};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};
use sqlx::PgPool;

#[tokio::test]
async fn immutable_catalog_round_trips_and_repeat_seed_creates_no_duplicate_visible_objects() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let fixture = fixture();
    assert_fixture_source();
    seed_fixture(&pool, &fixture).await;
    seed_fixture(&pool, &fixture).await;

    let repository = support::repository(pool.clone());
    let temporal =
        PortfolioCatalogTemporalScope::new(owner(), subject(), time(21, 3), time(21, 5)).unwrap();
    let snapshot = repository
        .read_catalog_snapshot(&support::access_scope(&owner()), &temporal)
        .await
        .unwrap();
    assert_eq!(snapshot.books().len(), 1);
    assert_eq!(snapshot.groups().len(), 1);
    assert_eq!(snapshot.portfolios().len(), 2);
    assert_eq!(snapshot.benchmarks().len(), 1);
    assert_eq!(snapshot.metric_conventions().len(), 1);

    let visible_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM portfolio.portfolios")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(visible_rows, 2, "repeat bootstrap must be idempotent");

    let portfolio = &fixture.portfolios[0];
    let exact = ExactCatalogRead::new(
        temporal.clone(),
        portfolio.reference().clone(),
        portfolio.content_hash().clone(),
    );
    let round_trip = repository
        .read_portfolio_exact(&support::access_scope(&owner()), &exact)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(round_trip.value(), portfolio);
    assert_eq!(round_trip.visible_at(), &time(21, 3));

    let immutable = sqlx::query(
        "UPDATE portfolio.portfolios SET display_name='mutated'
         WHERE tenant_id=$1 AND portfolio_id=$2 AND version=1",
    )
    .bind(owner().tenant_id().as_str())
    .bind(portfolio.reference().id().as_str())
    .execute(&pool)
    .await
    .expect_err("immutable catalog trigger must reject updates");
    assert!(immutable.to_string().contains("immutable"));
}

#[tokio::test]
async fn exact_reads_fail_closed_on_owner_hash_version_and_time_tamper() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let fixture = fixture();
    seed_fixture(&pool, &fixture).await;
    let repository = support::repository(pool);
    let temporal =
        PortfolioCatalogTemporalScope::new(owner(), subject(), time(21, 3), time(21, 5)).unwrap();
    let portfolio = &fixture.portfolios[0];
    let scope = support::access_scope(&owner());

    let wrong_hash = ExactCatalogRead::new(
        temporal.clone(),
        portfolio.reference().clone(),
        ContentHash::digest(b"tampered"),
    );
    assert!(
        repository
            .read_portfolio_exact(&scope, &wrong_hash)
            .await
            .unwrap()
            .is_none()
    );

    let wrong_version = ExactCatalogRead::new(
        temporal.clone(),
        VersionRef::new(portfolio.reference().id().clone(), Version::new(2).unwrap()),
        portfolio.content_hash().clone(),
    );
    assert!(
        repository
            .read_portfolio_exact(&scope, &wrong_version)
            .await
            .unwrap()
            .is_none()
    );

    let early =
        PortfolioCatalogTemporalScope::new(owner(), subject(), time(21, 1), time(21, 1)).unwrap();
    let time_drift = ExactCatalogRead::new(
        early,
        portfolio.reference().clone(),
        portfolio.content_hash().clone(),
    );
    let error = repository
        .read_portfolio_exact(&scope, &time_drift)
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);

    let forbidden_owner = OwnerRef::new(owner().tenant_id().clone(), id(14));
    let forbidden_read = ExactCatalogRead::new(
        PortfolioCatalogTemporalScope::new(forbidden_owner, subject(), time(21, 3), time(21, 5))
            .unwrap(),
        portfolio.reference().clone(),
        portfolio.content_hash().clone(),
    );
    let error = repository
        .read_portfolio_exact(&scope, &forbidden_read)
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::Forbidden);
}

#[tokio::test]
async fn scope_authority_is_owner_scoped_bitemporal_and_surfaces_ambiguity() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let fixture = fixture();
    seed_fixture(&pool, &fixture).await;
    let repository = support::repository(pool.clone());
    let selected = &fixture.portfolios[0];
    let selector = PortfolioScopeSelector::Portfolio(selected.reference().id().clone());

    let resolved = repository
        .find_scope_authorities(
            &support::access_scope(&owner()),
            &selector,
            &time(21, 3),
            &time(21, 5),
        )
        .await
        .unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].owner(), &owner());
    assert_eq!(resolved[0].subject_ref(), &subject());

    let unlisted_owner = OwnerRef::new(owner().tenant_id().clone(), id(14));
    assert!(
        repository
            .find_scope_authorities(
                &support::access_scope(&unlisted_owner),
                &selector,
                &time(21, 3),
                &time(21, 5),
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .find_scope_authorities(
                &support::access_scope(&owner()),
                &selector,
                &time(21, 1),
                &time(21, 1),
            )
            .await
            .unwrap()
            .is_empty()
    );

    let second_visible_version = next_portfolio_version(selected);
    insert_portfolio(&pool, &second_visible_version).await;
    let ambiguous = repository
        .find_scope_authorities(
            &support::access_scope(&owner()),
            &selector,
            &time(21, 3),
            &time(21, 5),
        )
        .await
        .unwrap();
    assert_eq!(
        ambiguous.len(),
        2,
        "application must reject ambiguous authority"
    );
}

#[tokio::test]
async fn analytics_authority_round_trips_replays_and_rejects_mutation() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let fixture = fixture();
    seed_fixture(&pool, &fixture).await;
    let expected = seed_analytics_authority(
        &pool,
        fixture.portfolios[0].position_snapshot(),
        id(15),
        true,
    )
    .await;
    seed_analytics_authority(
        &pool,
        fixture.portfolios[0].position_snapshot(),
        id(15),
        true,
    )
    .await;

    let repository = support::repository(pool.clone());
    let query = PortfolioAnalyticsAuthorityQuery::new(
        owner(),
        subject(),
        fixture.portfolios[0].position_snapshot().clone(),
        time(21, 3),
        time(21, 5),
    )
    .unwrap();
    let candidates = repository
        .read_candidates(&support::access_scope(&owner()), &query)
        .await
        .unwrap();
    assert_eq!(candidates, vec![expected.clone()]);
    assert_eq!(
        candidates[0].canonical_content_hash(),
        expected.content_hash
    );
    assert_eq!(candidates[0].units.len(), 9);
    assert_eq!(candidates[0].bond_rates.len(), 1);
    assert_eq!(candidates[0].bond_rates[0].remaining_years_value_index, 1);

    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM portfolio.analytics_authority_sets),
           (SELECT COUNT(*) FROM portfolio.analytics_authority_units),
           (SELECT COUNT(*) FROM portfolio.bond_rates_authorities)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (1, 9, 1), "repeat fixture seed must be idempotent");

    let immutable = sqlx::query(
        "UPDATE portfolio.bond_rates_authorities SET remaining_years_value_index=2
         WHERE tenant_id=$1 AND authority_set_id=$2",
    )
    .bind(owner().tenant_id().as_str())
    .bind(expected.authority_set_id.as_str())
    .execute(&pool)
    .await
    .expect_err("remaining-years authority mutation must fail");
    assert!(immutable.to_string().contains("immutable"));

    let outside_effective = PortfolioAnalyticsAuthorityQuery::new(
        owner(),
        subject(),
        fixture.portfolios[0].position_snapshot().clone(),
        time(22, 1),
        time(22, 1),
    )
    .unwrap();
    assert!(
        repository
            .read_candidates(&support::access_scope(&owner()), &outside_effective)
            .await
            .unwrap()
            .is_empty()
    );

    assert_future_visible_authority_boundary(
        &pool,
        &query,
        expected,
        fixture.portfolios[0].position_snapshot(),
    )
    .await;
}

async fn assert_future_visible_authority_boundary(
    pool: &PgPool,
    query: &PortfolioAnalyticsAuthorityQuery,
    expected: PortfolioAnalyticsAuthorityCandidate,
    position_snapshot: &PortfolioSnapshotBinding,
) {
    let mut future_visible = expected.clone();
    future_visible.authority_set_id = id(31);
    future_visible.visible_at = time(21, 6);
    future_visible.content_hash = future_visible.canonical_content_hash();
    insert_analytics_authority(pool, &future_visible).await;
    let repository = support::repository(pool.clone());
    assert_eq!(
        repository
            .read_candidates(&support::access_scope(&owner()), query)
            .await
            .unwrap(),
        vec![expected],
        "future-visible authority must not leak before the frozen knowledge boundary"
    );
    let after_visible = PortfolioAnalyticsAuthorityQuery::new(
        owner(),
        subject(),
        position_snapshot.clone(),
        time(21, 3),
        time(21, 6),
    )
    .unwrap();
    assert_eq!(
        repository
            .read_candidates(&support::access_scope(&owner()), &after_visible)
            .await
            .unwrap()
            .len(),
        2,
        "application must reject two visible authority sets"
    );
}

#[tokio::test]
async fn analytics_authority_surfaces_aggregate_hash_tamper() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    let fixture = fixture();
    seed_fixture(&pool, &fixture).await;
    let expected = seed_analytics_authority(
        &pool,
        fixture.portfolios[0].position_snapshot(),
        id(15),
        false,
    )
    .await;
    let repository = support::repository(pool);
    let query = PortfolioAnalyticsAuthorityQuery::new(
        owner(),
        subject(),
        fixture.portfolios[0].position_snapshot().clone(),
        time(21, 3),
        time(21, 5),
    )
    .unwrap();
    let candidates = repository
        .read_candidates(&support::access_scope(&owner()), &query)
        .await
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0], expected);
    assert_ne!(
        candidates[0].canonical_content_hash(),
        candidates[0].content_hash,
        "application preflight must observe aggregate tamper before handoff"
    );
}

struct Fixture {
    book: Book,
    group: PortfolioGroup,
    benchmark: Benchmark,
    convention: PortfolioMetricConvention,
    portfolios: Vec<Portfolio>,
}

fn fixture() -> Fixture {
    let book = fixture_book();
    let book_ref = lineage(&book);
    let group = fixture_group(book_ref.clone());
    let group_ref = lineage(&group);
    let benchmark = fixture_benchmark();
    let convention = fixture_convention();
    let portfolios = vec![
        fixture_portfolio(
            9,
            "ALPHA",
            fixture_snapshot(5, b"positions-a"),
            &book_ref,
            &group_ref,
            &benchmark,
            &convention,
        ),
        fixture_portfolio(
            10,
            "ZETA",
            fixture_snapshot(11, b"positions-z"),
            &book_ref,
            &group_ref,
            &benchmark,
            &convention,
        ),
    ];
    Fixture {
        book,
        group,
        benchmark,
        convention,
        portfolios,
    }
}

fn fixture_book() -> Book {
    let mut book_input = BookInput {
        book: version_ref(3),
        owner: owner(),
        subject_ref: subject(),
        code: "BOOK-CGB".to_owned(),
        display_name: "CGB Book".to_owned(),
        status: PortfolioStatus::Active,
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    book_input.content_hash = Book::content_hash_for(&book_input);
    Book::new(book_input).unwrap()
}

fn fixture_group(book: LineageRef) -> PortfolioGroup {
    let mut group_input = PortfolioGroupInput {
        group: version_ref(4),
        owner: owner(),
        subject_ref: subject(),
        book,
        parent_group: None,
        code: "GOV".to_owned(),
        display_name: "Government".to_owned(),
        status: PortfolioStatus::Active,
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    group_input.content_hash = PortfolioGroup::content_hash_for(&group_input);
    PortfolioGroup::new(group_input).unwrap()
}

fn fixture_snapshot(identity: usize, payload: &[u8]) -> PortfolioSnapshotBinding {
    PortfolioSnapshotBinding::new(
        id(identity),
        ContentHash::digest(payload),
        time(21, 2),
        time(21, 3),
    )
    .unwrap()
}

fn fixture_benchmark() -> Benchmark {
    let mut benchmark_input = BenchmarkInput {
        benchmark: version_ref(7),
        owner: owner(),
        subject_ref: subject(),
        code: "CGB-BENCH".to_owned(),
        display_name: "CGB Benchmark".to_owned(),
        position_snapshot: fixture_snapshot(6, b"benchmark-positions"),
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    benchmark_input.content_hash = Benchmark::content_hash_for(&benchmark_input);
    Benchmark::new(benchmark_input).unwrap()
}

fn fixture_convention() -> PortfolioMetricConvention {
    let mut convention_input = PortfolioMetricConventionInput {
        convention: version_ref(8),
        owner: owner(),
        schema_id: "ficant.portfolio-metric-convention.v1".to_owned(),
        ytm_weighting: PortfolioMetricWeighting::MarketValueTimesModifiedDuration,
        duration_weighting: PortfolioMetricWeighting::MarketValue,
        convexity_weighting: PortfolioMetricWeighting::MarketValue,
        coupon_weighting: PortfolioMetricWeighting::Notional,
        remaining_life_weighting: PortfolioMetricWeighting::Notional,
        rounding: PortfolioDecimalRounding::TiesToEven,
        freshness_limit_seconds: 86_400,
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    convention_input.content_hash = PortfolioMetricConvention::content_hash_for(&convention_input);
    PortfolioMetricConvention::new(convention_input).unwrap()
}

fn fixture_portfolio(
    identity: usize,
    code: &str,
    snapshot: PortfolioSnapshotBinding,
    book: &LineageRef,
    group: &LineageRef,
    benchmark: &Benchmark,
    convention: &PortfolioMetricConvention,
) -> Portfolio {
    let mut input = PortfolioInput {
        portfolio: version_ref(identity),
        owner: owner(),
        subject_ref: subject(),
        book: book.clone(),
        group: group.clone(),
        code: code.to_owned(),
        display_name: format!("{code} Portfolio"),
        status: PortfolioStatus::Active,
        position_snapshot: snapshot,
        benchmark: BenchmarkRef::new(
            benchmark.reference().clone(),
            benchmark.content_hash().clone(),
        ),
        metric_convention: PortfolioMetricConventionRef::new(
            convention.reference().clone(),
            convention.content_hash().clone(),
        ),
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = Portfolio::content_hash_for(&input);
    Portfolio::new(input).unwrap()
}

fn next_portfolio_version(value: &Portfolio) -> Portfolio {
    let mut input = PortfolioInput {
        portfolio: VersionRef::new(
            value.reference().id().clone(),
            Version::new(value.reference().version().get() + 1).unwrap(),
        ),
        owner: value.owner().clone(),
        subject_ref: value.subject_ref().clone(),
        book: value.book().clone(),
        group: value.group().clone(),
        code: value.code().to_owned(),
        display_name: value.display_name().to_owned(),
        status: value.status(),
        position_snapshot: value.position_snapshot().clone(),
        benchmark: value.benchmark().clone(),
        metric_convention: value.metric_convention().clone(),
        effective_from: value.effective_from().clone(),
        effective_to: value.effective_to().clone(),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = Portfolio::content_hash_for(&input);
    Portfolio::new(input).unwrap()
}

async fn seed_analytics_authority(
    pool: &PgPool,
    position_snapshot: &PortfolioSnapshotBinding,
    authority_set_id: Ulid,
    valid_hash: bool,
) -> PortfolioAnalyticsAuthorityCandidate {
    let roles = [
        PortfolioRatesUnitRole::CurrencyAmount,
        PortfolioRatesUnitRole::PricePer100,
        PortfolioRatesUnitRole::Rate,
        PortfolioRatesUnitRole::Years,
        PortfolioRatesUnitRole::YearsSquared,
        PortfolioRatesUnitRole::Dv01Per100,
        PortfolioRatesUnitRole::Dv01,
        PortfolioRatesUnitRole::Dimensionless,
        PortfolioRatesUnitRole::ContractCount,
    ];
    let units = roles
        .into_iter()
        .enumerate()
        .map(|(offset, role)| PortfolioUnitAuthorityBinding {
            role,
            reference: UnitRef::new(id(22 + offset), Version::new(1).unwrap()),
            content_hash: ContentHash::digest(format!("unit-{role:?}").as_bytes()),
        })
        .collect::<Vec<_>>();
    seed_analytics_dependencies(pool, &units).await;

    let valuation = PortfolioValuationAuthorityBinding {
        valuation_id: id(21),
        source_revision: 1,
        content_hash: ContentHash::digest(b"valuation-payload"),
        value_index: 0,
    };
    let bond = PortfolioBondRatesAuthorityCandidate {
        position_id: id(12),
        instrument_ref: version_ref(20),
        valuation,
        remaining_years_value_index: 1,
        mode: AnalyticsMode::PriceIn,
        input_value: FixedDecimal::from_scaled(101_230_000_000_000),
        remaining_years: FixedDecimal::from_scaled(7_350_000_000_000),
        settlement_date: NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
        calendar_requirement: CalendarRequirement::ExactMarket,
    };
    let mut candidate = PortfolioAnalyticsAuthorityCandidate {
        authority_set_id,
        owner: owner(),
        subject_ref: subject(),
        position_snapshot: PortfolioImmutableSnapshotAuthority {
            id: position_snapshot.snapshot_id().clone(),
            content_hash: position_snapshot.content_hash().clone(),
        },
        curve_snapshot: PortfolioImmutableSnapshotAuthority {
            id: id(16),
            content_hash: ContentHash::digest(b"curve-points"),
        },
        data_snapshot: PortfolioImmutableSnapshotAuthority {
            id: id(17),
            content_hash: ContentHash::digest(b"data-parquet"),
        },
        futures_data_snapshot: None,
        tax_rule_pack: AnalyticsObjectRef::new(
            version_ref(18),
            ContentHash::digest(b"tax-rule-pack"),
        ),
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        visible_at: time(21, 3),
        units,
        bond_rates: vec![bond],
        content_hash: ContentHash::digest(b"placeholder"),
    };
    candidate.content_hash = if valid_hash {
        candidate.canonical_content_hash()
    } else {
        ContentHash::digest(b"tampered-authority-set")
    };
    insert_analytics_authority(pool, &candidate).await;
    candidate
}

async fn seed_analytics_dependencies(pool: &PgPool, units: &[PortfolioUnitAuthorityBinding]) {
    seed_authority_units(pool, units).await;
    seed_authority_definitions(pool).await;
    seed_authority_snapshots(pool).await;
}

async fn seed_authority_units(pool: &PgPool, units: &[PortfolioUnitAuthorityBinding]) {
    for unit in units {
        sqlx::query(
            "INSERT INTO market.units
             (tenant_id,unit_id,version,owner_id,code,dimension,scale,precision,payload)
             VALUES ($1,$2,1,$3,$4,$5,12,28,decode('01','hex')) ON CONFLICT DO NOTHING",
        )
        .bind(owner().tenant_id().as_str())
        .bind(unit.reference.unit_id().as_str())
        .bind(owner().owner_id().as_str())
        .bind(format!("AUTH_{:?}", unit.role).to_uppercase())
        .bind(unit.role.expected_dimension())
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn seed_authority_definitions(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO market.calendars
         (tenant_id,calendar_id,version,owner_id,market,market_timezone,effective_from,effective_to,payload)
         VALUES ($1,$2,1,$3,'CN','Asia/Shanghai',$4,$5,decode('01','hex'))
         ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(id(19).as_str())
    .bind(owner().owner_id().as_str())
    .bind(time(20, 0).instant())
    .bind(time(22, 0).instant())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.market_rule_packs
         (tenant_id,rule_pack_id,version,owner_id,market,rule_type,source,effective_from,
          effective_to,verification_status,content_hash,payload)
         VALUES ($1,$2,1,$3,'CN','tax','r8a-fixture',$4,$5,'VERIFIED',$6,decode('01','hex'))
         ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(id(18).as_str())
    .bind(owner().owner_id().as_str())
    .bind(time(20, 0).instant())
    .bind(time(22, 0).instant())
    .bind(hash_hex(&ContentHash::digest(b"tax-rule-pack")))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.instruments
         (tenant_id,instrument_id,version,owner_id,kind,market,symbol,currency_unit_id,
          currency_unit_version,calendar_id,calendar_version,payload)
         VALUES ($1,$2,1,$3,'BOND','CN','R8A.CGB',$4,1,$5,1,decode('01','hex'))
         ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(id(20).as_str())
    .bind(owner().owner_id().as_str())
    .bind(id(22).as_str())
    .bind(id(19).as_str())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.bonds
         (tenant_id,instrument_id,version,issue_date,maturity_date,face_coefficient,face_scale,
          face_unit_id,face_unit_version,payload)
         VALUES ($1,$2,1,DATE '2024-01-01',DATE '2034-01-01',100,0,$3,1,decode('01','hex'))
         ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(id(20).as_str())
    .bind(id(22).as_str())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.valuations
         (tenant_id,valuation_id,owner_id,instrument_id,instrument_version,fact_time,source_id,
          external_id,source_revision,rule_pack_id,rule_pack_version,payload)
         VALUES ($1,$2,$3,$4,1,$5,'r8a-fixture','valuation',1,$6,1,decode('01','hex'))
         ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(id(21).as_str())
    .bind(owner().owner_id().as_str())
    .bind(id(20).as_str())
    .bind(time(21, 3).instant())
    .bind(id(18).as_str())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_authority_snapshots(pool: &PgPool) {
    seed_blob(pool, &ContentHash::digest(b"curve-points")).await;
    seed_blob(pool, &ContentHash::digest(b"data-parquet")).await;
    seed_blob(pool, &ContentHash::digest(b"data-manifest")).await;
    sqlx::query(
        "INSERT INTO market.curve_snapshots
         (tenant_id,curve_snapshot_id,owner_id,as_of,currency_unit_id,currency_unit_version,
          curve_kind,calendar_id,calendar_version,rule_pack_id,rule_pack_version,point_schema,
          content_hash,blob_size,idempotency_key,fingerprint,payload,visible_at,curve_family_id)
         VALUES ($1,$2,$3,$4,$5,1,'YTM',$6,1,$7,1,'ficant.curve.points.v1',$8,1,
                 'r8a-curve',decode(repeat('22',32),'hex'),decode('01','hex'),$9,'cn.cgb.ytm')
         ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(id(16).as_str())
    .bind(owner().owner_id().as_str())
    .bind(time(21, 3).instant())
    .bind(id(22).as_str())
    .bind(id(19).as_str())
    .bind(id(18).as_str())
    .bind(hash_hex(&ContentHash::digest(b"curve-points")))
    .bind(time(21, 3).instant())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO research.data_snapshots
         (tenant_id,data_snapshot_id,owner_id,visible_at,as_of,schema_hash,manifest_hash,
          content_hash,idempotency_key,fingerprint,payload)
         VALUES ($1,$2,$3,$4,$4,$5,$6,$7,'r8a-data',decode(repeat('33',32),'hex'),decode('01','hex'))
         ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(id(17).as_str())
    .bind(owner().owner_id().as_str())
    .bind(time(21, 3).instant())
    .bind(hash_hex(&ContentHash::digest(b"data-schema")))
    .bind(hash_hex(&ContentHash::digest(b"data-manifest")))
    .bind(hash_hex(&ContentHash::digest(b"data-parquet")))
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_blob(pool: &PgPool, content_hash: &ContentHash) {
    let hash = hash_hex(content_hash);
    sqlx::query(
        "INSERT INTO storage.blobs(tenant_id,content_hash,object_key,blob_size)
         VALUES ($1,$2,'immutable/' || $2,1) ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(hash)
    .execute(pool)
    .await
    .unwrap();
}

#[allow(clippy::too_many_lines)]
async fn insert_analytics_authority(
    pool: &PgPool,
    candidate: &PortfolioAnalyticsAuthorityCandidate,
) {
    let mut query = sqlx::query(
        "INSERT INTO portfolio.analytics_authority_sets
         (tenant_id,authority_set_id,owner_id,subject_id,subject_version,
          position_snapshot_id,position_snapshot_hash,curve_snapshot_id,curve_snapshot_hash,
          data_snapshot_id,data_snapshot_hash,tax_rule_pack_id,tax_rule_pack_version,
          tax_rule_pack_hash,effective_from,effective_from_nanos,effective_from_timezone,
          effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
          effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
          visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                 $21,$22,$23,$24,$25,$26,$27) ON CONFLICT DO NOTHING",
    )
    .bind(candidate.owner.tenant_id().as_str())
    .bind(candidate.authority_set_id.as_str())
    .bind(candidate.owner.owner_id().as_str())
    .bind(candidate.subject_ref.id().as_str())
    .bind(i64::try_from(candidate.subject_ref.version().get()).unwrap())
    .bind(candidate.position_snapshot.id.as_str())
    .bind(hash_hex(&candidate.position_snapshot.content_hash))
    .bind(candidate.curve_snapshot.id.as_str())
    .bind(hash_hex(&candidate.curve_snapshot.content_hash))
    .bind(candidate.data_snapshot.id.as_str())
    .bind(hash_hex(&candidate.data_snapshot.content_hash))
    .bind(candidate.tax_rule_pack.version_ref().id().as_str())
    .bind(i64::try_from(candidate.tax_rule_pack.version_ref().version().get()).unwrap())
    .bind(hash_hex(candidate.tax_rule_pack.content_hash()));
    query = bind_time(query, &candidate.effective_from);
    query = bind_time(query, &candidate.effective_to);
    query = bind_time(query, &candidate.visible_at);
    query
        .bind(hash_hex(&candidate.content_hash))
        .execute(pool)
        .await
        .unwrap();
    for unit in &candidate.units {
        sqlx::query(
            "INSERT INTO portfolio.analytics_authority_units
             (tenant_id,authority_set_id,role,unit_id,unit_version,unit_hash)
             VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING",
        )
        .bind(candidate.owner.tenant_id().as_str())
        .bind(candidate.authority_set_id.as_str())
        .bind(unit_role(unit.role))
        .bind(unit.reference.unit_id().as_str())
        .bind(i64::try_from(unit.reference.version().get()).unwrap())
        .bind(hash_hex(&unit.content_hash))
        .execute(pool)
        .await
        .unwrap();
    }
    for bond in &candidate.bond_rates {
        sqlx::query(
            "INSERT INTO portfolio.bond_rates_authorities
             (tenant_id,authority_set_id,position_id,instrument_id,instrument_version,
              valuation_id,valuation_source_revision,valuation_hash,valuation_value_index,
              remaining_years_value_index,mode,input_coefficient,input_scale,
              remaining_years_coefficient,remaining_years_scale,settlement_date,
              calendar_requirement)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::numeric,12,$13::numeric,12,$14,$15)
             ON CONFLICT DO NOTHING",
        )
        .bind(candidate.owner.tenant_id().as_str())
        .bind(candidate.authority_set_id.as_str())
        .bind(bond.position_id.as_str())
        .bind(bond.instrument_ref.id().as_str())
        .bind(i64::try_from(bond.instrument_ref.version().get()).unwrap())
        .bind(bond.valuation.valuation_id.as_str())
        .bind(i64::try_from(bond.valuation.source_revision).unwrap())
        .bind(hash_hex(&bond.valuation.content_hash))
        .bind(i32::try_from(bond.valuation.value_index).unwrap())
        .bind(i32::try_from(bond.remaining_years_value_index).unwrap())
        .bind("PRICE_IN")
        .bind(bond.input_value.scaled().to_string())
        .bind(bond.remaining_years.scaled().to_string())
        .bind(bond.settlement_date)
        .bind("EXACT_MARKET")
        .execute(pool)
        .await
        .unwrap();
    }
}

const fn unit_role(role: PortfolioRatesUnitRole) -> &'static str {
    match role {
        PortfolioRatesUnitRole::CurrencyAmount => "CURRENCY_AMOUNT",
        PortfolioRatesUnitRole::PricePer100 => "PRICE_PER_100",
        PortfolioRatesUnitRole::Rate => "RATE",
        PortfolioRatesUnitRole::Years => "YEARS",
        PortfolioRatesUnitRole::YearsSquared => "YEARS_SQUARED",
        PortfolioRatesUnitRole::Dv01Per100 => "DV01_PER_100",
        PortfolioRatesUnitRole::Dv01 => "DV01",
        PortfolioRatesUnitRole::Dimensionless => "DIMENSIONLESS",
        PortfolioRatesUnitRole::ContractCount => "CONTRACT_COUNT",
    }
}

async fn seed_fixture(pool: &PgPool, fixture: &Fixture) {
    seed_subject_and_units(pool).await;
    let bindings = fixture
        .portfolios
        .iter()
        .map(Portfolio::position_snapshot)
        .chain(std::iter::once(fixture.benchmark.position_snapshot()));
    for binding in bindings {
        seed_position_snapshot(pool, binding).await;
    }
    insert_book(pool, &fixture.book).await;
    insert_group(pool, &fixture.group).await;
    insert_benchmark(pool, &fixture.benchmark).await;
    insert_convention(pool, &fixture.convention).await;
    for portfolio in &fixture.portfolios {
        insert_portfolio(pool, portfolio).await;
    }
}

async fn seed_subject_and_units(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO core.subject_identities
            (tenant_id, subject_id, owner_id, latest_version)
         VALUES ($1,$2,$3,1) ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(subject().id().as_str())
    .bind(owner().owner_id().as_str())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO core.subject_versions
            (subject_id, version, display_name, funding_tier, value_added_tax_profile,
             income_tax_profile, assessment_mechanism, liability_profile, tenant_id, owner_id)
         VALUES ($1,1,'R8A Subject','DR_AVAILABLE','vat','income','daily','general',$2,$3)
         ON CONFLICT DO NOTHING",
    )
    .bind(subject().id().as_str())
    .bind(owner().tenant_id().as_str())
    .bind(owner().owner_id().as_str())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO market.units
            (tenant_id, unit_id, version, owner_id, code, dimension, scale, precision, payload)
         VALUES ($1,$2,1,$3,'CNY','currency',2,28,decode('01','hex'))
         ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(id(13).as_str())
    .bind(owner().owner_id().as_str())
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_position_snapshot(pool: &PgPool, binding: &PortfolioSnapshotBinding) {
    let hash = hash_hex(binding.content_hash());
    sqlx::query(
        "INSERT INTO storage.blobs(tenant_id, content_hash, object_key, blob_size)
         VALUES ($1,$2,'immutable/' || $2,1) ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(&hash)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO research.position_snapshots
            (tenant_id, snapshot_id, owner_id, subject_id, subject_version, observed_at,
             visible_at, content_hash, idempotency_key, fingerprint, payload)
         VALUES ($1,$2,$3,$4,1,$5,$6,$7,$8,decode(repeat('11',32),'hex'),decode('01','hex'))
         ON CONFLICT DO NOTHING",
    )
    .bind(owner().tenant_id().as_str())
    .bind(binding.snapshot_id().as_str())
    .bind(owner().owner_id().as_str())
    .bind(subject().id().as_str())
    .bind(binding.observed_at().instant())
    .bind(binding.visible_at().instant())
    .bind(&hash)
    .bind(format!("r8a-{}", binding.snapshot_id()))
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_book(pool: &PgPool, value: &Book) {
    let mut query = sqlx::query(
        "INSERT INTO portfolio.books
         (tenant_id,book_id,version,owner_id,subject_id,subject_version,code,display_name,status,
          effective_from,effective_from_nanos,effective_from_timezone,effective_from_local_date,
          effective_to,effective_to_nanos,effective_to_timezone,effective_to_local_date,
          visible_at,visible_at_nanos,visible_at_timezone,visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)
         ON CONFLICT DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get()).unwrap())
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get()).unwrap())
    .bind(value.code())
    .bind(value.display_name())
    .bind(status(value.status()));
    query = bind_time(query, value.effective_from());
    query = bind_time(query, value.effective_to());
    query = bind_time(query, &time(21, 3));
    query
        .bind(hash_hex(value.content_hash()))
        .execute(pool)
        .await
        .unwrap();
}

async fn insert_group(pool: &PgPool, value: &PortfolioGroup) {
    let mut query = sqlx::query(
        "INSERT INTO portfolio.groups
         (tenant_id,group_id,version,owner_id,subject_id,subject_version,
          book_id,book_version,book_hash,parent_group_id,parent_group_version,parent_group_hash,
          code,display_name,status,effective_from,effective_from_nanos,effective_from_timezone,
          effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
          effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
          visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                 $21,$22,$23,$24,$25,$26,$27,$28) ON CONFLICT DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get()).unwrap())
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get()).unwrap())
    .bind(value.book().object_id().as_str())
    .bind(i64::try_from(value.book().version().unwrap().get()).unwrap())
    .bind(hash_hex(value.book().content_hash().unwrap()))
    .bind(
        value
            .parent_group()
            .map(|parent| parent.object_id().as_str()),
    )
    .bind(
        value
            .parent_group()
            .map(|parent| i64::try_from(parent.version().unwrap().get()).unwrap()),
    )
    .bind(
        value
            .parent_group()
            .map(|parent| hash_hex(parent.content_hash().unwrap())),
    )
    .bind(value.code())
    .bind(value.display_name())
    .bind(status(value.status()));
    query = bind_time(query, value.effective_from());
    query = bind_time(query, value.effective_to());
    query = bind_time(query, &time(21, 3));
    query
        .bind(hash_hex(value.content_hash()))
        .execute(pool)
        .await
        .unwrap();
}

async fn insert_benchmark(pool: &PgPool, value: &Benchmark) {
    let snapshot = value.position_snapshot();
    let mut query = sqlx::query(
        "INSERT INTO portfolio.benchmarks
         (tenant_id,benchmark_id,version,owner_id,subject_id,subject_version,code,display_name,
          snapshot_id,snapshot_hash,snapshot_observed_at,snapshot_observed_at_nanos,
          snapshot_observed_at_timezone,snapshot_observed_at_local_date,snapshot_visible_at,
          snapshot_visible_at_nanos,snapshot_visible_at_timezone,snapshot_visible_at_local_date,
          effective_from,effective_from_nanos,effective_from_timezone,effective_from_local_date,
          effective_to,effective_to_nanos,effective_to_timezone,effective_to_local_date,
          visible_at,visible_at_nanos,visible_at_timezone,visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                 $21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31) ON CONFLICT DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get()).unwrap())
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get()).unwrap())
    .bind(value.code())
    .bind(value.display_name())
    .bind(snapshot.snapshot_id().as_str())
    .bind(hash_hex(snapshot.content_hash()));
    query = bind_time(query, snapshot.observed_at());
    query = bind_time(query, snapshot.visible_at());
    query = bind_time(query, value.effective_from());
    query = bind_time(query, value.effective_to());
    query = bind_time(query, &time(21, 3));
    query
        .bind(hash_hex(value.content_hash()))
        .execute(pool)
        .await
        .unwrap();
}

async fn insert_convention(pool: &PgPool, value: &PortfolioMetricConvention) {
    let mut query = sqlx::query(
        "INSERT INTO portfolio.metric_conventions
         (tenant_id,convention_id,version,owner_id,schema_id,ytm_weighting,duration_weighting,
          convexity_weighting,coupon_weighting,remaining_life_weighting,rounding,
          freshness_limit_seconds,effective_from,effective_from_nanos,effective_from_timezone,
          effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
          effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
          visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,'MARKET_VALUE_TIMES_MODIFIED_DURATION','MARKET_VALUE',
                 'MARKET_VALUE','NOTIONAL','NOTIONAL','TIES_TO_EVEN',$6,$7,$8,$9,$10,$11,$12,
                 $13,$14,$15,$16,$17,$18,$19) ON CONFLICT DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get()).unwrap())
    .bind(value.owner().owner_id().as_str())
    .bind(value.schema_id())
    .bind(i64::try_from(value.freshness_limit_seconds()).unwrap());
    query = bind_time(query, value.effective_from());
    query = bind_time(query, value.effective_to());
    query = bind_time(query, &time(21, 3));
    query
        .bind(hash_hex(value.content_hash()))
        .execute(pool)
        .await
        .unwrap();
}

async fn insert_portfolio(pool: &PgPool, value: &Portfolio) {
    let snapshot = value.position_snapshot();
    let mut query = sqlx::query(
        "INSERT INTO portfolio.portfolios
         (tenant_id,portfolio_id,version,owner_id,subject_id,subject_version,
          book_id,book_version,book_hash,group_id,group_version,group_hash,code,display_name,status,
          snapshot_id,snapshot_hash,snapshot_observed_at,snapshot_observed_at_nanos,
          snapshot_observed_at_timezone,snapshot_observed_at_local_date,snapshot_visible_at,
          snapshot_visible_at_nanos,snapshot_visible_at_timezone,snapshot_visible_at_local_date,
          benchmark_id,benchmark_version,benchmark_hash,convention_id,convention_version,
          convention_hash,effective_from,effective_from_nanos,effective_from_timezone,
          effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
          effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
          visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                 $21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34,$35,$36,$37,$38,
                 $39,$40,$41,$42,$43,$44) ON CONFLICT DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get()).unwrap())
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get()).unwrap())
    .bind(value.book().object_id().as_str())
    .bind(i64::try_from(value.book().version().unwrap().get()).unwrap())
    .bind(hash_hex(value.book().content_hash().unwrap()))
    .bind(value.group().object_id().as_str())
    .bind(i64::try_from(value.group().version().unwrap().get()).unwrap())
    .bind(hash_hex(value.group().content_hash().unwrap()))
    .bind(value.code())
    .bind(value.display_name())
    .bind(status(value.status()))
    .bind(snapshot.snapshot_id().as_str())
    .bind(hash_hex(snapshot.content_hash()));
    query = bind_time(query, snapshot.observed_at());
    query = bind_time(query, snapshot.visible_at());
    query = query
        .bind(value.benchmark().reference().id().as_str())
        .bind(i64::try_from(value.benchmark().reference().version().get()).unwrap())
        .bind(hash_hex(value.benchmark().content_hash()))
        .bind(value.metric_convention().reference().id().as_str())
        .bind(i64::try_from(value.metric_convention().reference().version().get()).unwrap())
        .bind(hash_hex(value.metric_convention().content_hash()));
    query = bind_time(query, value.effective_from());
    query = bind_time(query, value.effective_to());
    query = bind_time(query, &time(21, 3));
    query
        .bind(hash_hex(value.content_hash()))
        .execute(pool)
        .await
        .unwrap();
}

fn bind_time<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    value: &MarketTime,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    query
        .bind(value.instant().with_nanosecond(0).unwrap())
        .bind(i32::try_from(value.instant().timestamp_subsec_nanos()).unwrap())
        .bind(value.market_timezone().to_owned())
        .bind(value.local_trading_date())
}

fn assert_fixture_source() {
    let value: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/portfolio/catalog-p0.json"
    )))
    .unwrap();
    assert_eq!(value["schema_id"], "ficant.portfolio-catalog-fixture.v1");
    assert_eq!(value["portfolios"].as_array().unwrap().len(), 2);
    assert_eq!(
        value["metric_convention"]["freshness_limit_seconds"],
        86_400
    );
    assert_eq!(
        value["analytics_authority"]["unit_roles"]
            .as_array()
            .unwrap()
            .len(),
        9
    );
    assert_eq!(
        value["analytics_authority"]["valuation"]["remaining_years_value_index"],
        1
    );
}

fn status(value: PortfolioStatus) -> &'static str {
    match value {
        PortfolioStatus::Active => "ACTIVE",
        PortfolioStatus::Suspended => "SUSPENDED",
        PortfolioStatus::Closed => "CLOSED",
    }
}

fn hash_hex(value: &ContentHash) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::new(), |mut text, byte| {
            use std::fmt::Write as _;
            write!(text, "{byte:02x}").unwrap();
            text
        })
}

fn lineage<T>(value: &T) -> LineageRef
where
    T: ContentAddressed + VersionedDefinition,
{
    LineageRef::new(
        Ulid::new(value.identity().to_owned()).unwrap(),
        Some(Version::new(value.version()).unwrap()),
        Some(value.content_hash().clone()),
    )
    .unwrap()
}

fn time(day: u32, hour: u32) -> MarketTime {
    let instant = Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0).unwrap();
    let local = NaiveDate::from_ymd_opt(2026, 8, day).unwrap();
    MarketTime::new(instant, "Asia/Shanghai", local).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id(1), id(2))
}

fn subject() -> VersionRef {
    version_ref(0)
}

fn version_ref(index: usize) -> VersionRef {
    VersionRef::new(id(index), Version::new(1).unwrap())
}

fn id(index: usize) -> Ulid {
    const SUFFIXES: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    Ulid::new(format!(
        "01ARZ3NDEKTSV4RRFFQ69G5FA{}",
        char::from(SUFFIXES[index])
    ))
    .unwrap()
}
