mod support;

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ApplicationErrorCategory;
use ficant_application::ports::{
    AccessScope, PortfolioPerformanceReadQuery, PortfolioPerformanceRepository,
};
use ficant_domain::ContentAddressed;
use ficant_domain::portfolio::{
    BenchmarkLevelSnapshot, BenchmarkLevelSnapshotInput, PortfolioDecimalRounding,
    PortfolioExternalFlowTiming, PortfolioPerformanceConvention,
    PortfolioPerformanceConventionInput, PortfolioPerformanceConventionRef,
    PortfolioPerformanceReturnMethod, PortfolioSnapshotBinding, PortfolioValuationFrequency,
    PortfolioValuationSnapshot, PortfolioValuationSnapshotInput,
};
use ficant_domain::primitives::{
    ContentHash, FixedDecimal, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use sqlx::PgPool;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn performance_inputs_are_bitemporal_immutable_and_fail_closed_on_tamper() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    seed_catalog_dependencies(&pool).await;
    let fixture = fixture();
    insert_performance_convention(&pool, &fixture.convention, &time(19, 3)).await;

    let day_20_first = valuation(
        14,
        20,
        time(21, 9),
        fixture.currency.clone(),
        110,
        10,
        100,
        0,
        &fixture,
    );
    let day_20_revision = valuation(
        15,
        20,
        time(22, 9),
        fixture.currency.clone(),
        111,
        10,
        101,
        0,
        &fixture,
    );
    let day_21 = valuation(
        16,
        21,
        time(21, 10),
        fixture.currency.clone(),
        115,
        5,
        110,
        5,
        &fixture,
    );
    for value in [&day_20_first, &day_20_revision, &day_21] {
        insert_valuation(&pool, value, None)
            .await
            .expect("valid valuation revision must be appendable");
    }
    let level_20 = benchmark_level(17, 20, time(21, 9), 100, &fixture);
    let level_21 = benchmark_level(18, 21, time(21, 10), 101, &fixture);
    for value in [&level_20, &level_21] {
        insert_benchmark_level(&pool, value, value.content_hash())
            .await
            .expect("valid benchmark level must be appendable");
    }

    let early_query = read_query(20, 21, time(21, 12), &fixture);
    let repository = support::repository(pool.clone());
    let early = repository
        .read_valuation_snapshots(&support::access_scope(&owner()), &early_query)
        .await
        .expect("knowledge-at read must succeed");
    assert_eq!(early.len(), 2);
    assert_eq!(early[0].snapshot_id(), day_20_first.snapshot_id());
    assert_eq!(early[0].net_asset_value(), amount(100));
    assert_eq!(early[1].snapshot_id(), day_21.snapshot_id());
    let early_levels = repository
        .read_benchmark_level_snapshots(&support::access_scope(&owner()), &early_query)
        .await
        .expect("benchmark series must round-trip");
    assert_eq!(early_levels, vec![level_20.clone(), level_21.clone()]);
    let convention = repository
        .read_performance_convention_exact(
            &support::access_scope(&owner()),
            &owner(),
            fixture.convention.reference(),
            fixture.convention.content_hash(),
            &time(21, 12),
        )
        .await
        .expect("exact convention read must succeed")
        .expect("exact convention must exist");
    assert_eq!(convention.value(), &fixture.convention);
    assert_eq!(convention.visible_at(), &time(19, 3));

    let late_query = read_query(20, 21, time(22, 12), &fixture);
    let late = repository
        .read_valuation_snapshots(&support::access_scope(&owner()), &late_query)
        .await
        .expect("later knowledge-at read must succeed");
    assert_eq!(late.len(), 2);
    assert_eq!(late[0].snapshot_id(), day_20_revision.snapshot_id());
    assert_eq!(late[0].net_asset_value(), amount(101));

    drop(repository);
    pool.close().await;
    let reopened_pool = support::postgres_pool().await;
    let reopened = support::repository(reopened_pool.clone());
    let after_restart = reopened
        .read_valuation_snapshots(&support::access_scope(&owner()), &late_query)
        .await
        .expect("fresh repository and connection must reproduce the same read");
    assert_eq!(after_restart, late);

    let update_error = sqlx::query(
        "UPDATE portfolio.valuation_snapshots
         SET net_asset_value_scaled='999'
         WHERE tenant_id=$1 AND snapshot_id=$2",
    )
    .bind(owner().tenant_id().as_str())
    .bind(day_21.snapshot_id().as_str())
    .execute(&reopened_pool)
    .await
    .expect_err("immutable valuation snapshot must reject UPDATE");
    assert!(update_error.to_string().contains("immutable"));
    let delete_error = sqlx::query(
        "DELETE FROM portfolio.benchmark_level_snapshots
         WHERE tenant_id=$1 AND snapshot_id=$2",
    )
    .bind(owner().tenant_id().as_str())
    .bind(level_21.snapshot_id().as_str())
    .execute(&reopened_pool)
    .await
    .expect_err("immutable benchmark level must reject DELETE");
    assert!(delete_error.to_string().contains("immutable"));
    let convention_update = sqlx::query(
        "UPDATE portfolio.performance_conventions
         SET rounding='TIES_TO_EVEN'
         WHERE tenant_id=$1 AND convention_id=$2 AND version=1",
    )
    .bind(owner().tenant_id().as_str())
    .bind(fixture.convention.reference().id().as_str())
    .execute(&reopened_pool)
    .await
    .expect_err("immutable performance convention must reject UPDATE");
    assert!(convention_update.to_string().contains("immutable"));

    let duplicate = valuation(
        22,
        21,
        day_21.visible_at().clone(),
        fixture.currency.clone(),
        115,
        5,
        110,
        5,
        &fixture,
    );
    assert!(
        insert_valuation(&reopened_pool, &duplicate, None)
            .await
            .is_err(),
        "same exact portfolio/session/visible revision must be unique"
    );

    let wrong_unit = UnitRef::new(id(30), Version::new(1).unwrap());
    let unit_tamper = valuation(23, 22, time(22, 10), wrong_unit, 120, 5, 115, 0, &fixture);
    assert!(
        insert_valuation(&reopened_pool, &unit_tamper, None)
            .await
            .is_err(),
        "unknown exact Unit must be rejected by the database"
    );

    let foreign_scope = AccessScope::new(id(20), id(21), vec![owner().owner_id().clone()]).unwrap();
    let forbidden = reopened
        .read_valuation_snapshots(&foreign_scope, &late_query)
        .await
        .expect_err("cross-tenant read must fail before SQL materialization");
    assert_eq!(forbidden.category(), ApplicationErrorCategory::Forbidden);

    let level_22 = benchmark_level(19, 22, time(22, 10), 102, &fixture);
    insert_benchmark_level(
        &reopened_pool,
        &level_22,
        &ContentHash::digest(b"tampered-level-content"),
    )
    .await
    .expect("the storage boundary must detect, not silently repair, a bad payload hash");
    let hash_error = reopened
        .read_benchmark_level_snapshots(
            &support::access_scope(&owner()),
            &read_query(22, 23, time(23, 12), &fixture),
        )
        .await
        .expect_err("content hash drift must fail closed on read");
    assert_eq!(
        hash_error.category(),
        ApplicationErrorCategory::HashMismatch
    );

    let time_tamper = valuation(
        24,
        23,
        time(23, 10),
        fixture.currency.clone(),
        125,
        5,
        120,
        0,
        &fixture,
    );
    insert_valuation(&reopened_pool, &time_tamper, Some("Pacific/Honolulu"))
        .await
        .expect("database stores explicit time evidence for verified-read validation");
    let time_error = reopened
        .read_valuation_snapshots(
            &support::access_scope(&owner()),
            &read_query(23, 24, time(24, 12), &fixture),
        )
        .await
        .expect_err("timezone/local-date drift must fail closed on read");
    assert_eq!(
        time_error.category(),
        ApplicationErrorCategory::ValidationFailed
    );

    drop(reopened);
    support::reset_postgres(&reopened_pool).await;
    support::migrate(&reopened_pool).await;
}

struct Fixture {
    portfolio: LineageRef,
    benchmark: LineageRef,
    convention: PortfolioPerformanceConvention,
    position: PortfolioSnapshotBinding,
    currency: UnitRef,
    dimensionless: UnitRef,
}

fn fixture() -> Fixture {
    let calendar = exact_ref(5, 0x77);
    let mut convention_input = PortfolioPerformanceConventionInput {
        convention: version_ref(13),
        owner: owner(),
        schema_id: "ficant.portfolio-performance-convention.v1".to_owned(),
        calendar,
        return_method: PortfolioPerformanceReturnMethod::DailyTimeWeighted,
        flow_timing: PortfolioExternalFlowTiming::EndOfDay,
        valuation_frequency: PortfolioValuationFrequency::CalendarSessionClose,
        rounding: PortfolioDecimalRounding::TiesToEven,
        effective_from: time(18, 0),
        effective_to: time(30, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    convention_input.content_hash =
        PortfolioPerformanceConvention::content_hash_for(&convention_input);
    Fixture {
        portfolio: exact_ref(12, 0x55),
        benchmark: exact_ref(10, 0x33),
        convention: PortfolioPerformanceConvention::new(convention_input).unwrap(),
        position: PortfolioSnapshotBinding::new(id(8), hash_byte(0x66), time(19, 1), time(19, 2))
            .unwrap(),
        currency: UnitRef::new(id(3), Version::new(1).unwrap()),
        dimensionless: UnitRef::new(id(4), Version::new(1).unwrap()),
    }
}

#[allow(clippy::too_many_arguments)]
fn valuation(
    identity: usize,
    day: u32,
    visible_at: MarketTime,
    currency: UnitRef,
    gross_assets: i128,
    liabilities: i128,
    nav: i128,
    flow: i128,
    fixture: &Fixture,
) -> PortfolioValuationSnapshot {
    let mut input = PortfolioValuationSnapshotInput {
        snapshot_id: id(identity),
        owner: owner(),
        subject_ref: subject(),
        portfolio: fixture.portfolio.clone(),
        position_snapshot: fixture.position.clone(),
        performance_convention: PortfolioPerformanceConventionRef::new(
            fixture.convention.reference().clone(),
            fixture.convention.content_hash().clone(),
        ),
        valuation_at: time(day, 8),
        visible_at,
        currency_unit: currency,
        gross_assets: amount(gross_assets),
        liabilities: amount(liabilities),
        net_asset_value: amount(nav),
        net_external_flow: amount(flow),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = PortfolioValuationSnapshot::content_hash_for(&input);
    PortfolioValuationSnapshot::new(input).unwrap()
}

fn benchmark_level(
    identity: usize,
    day: u32,
    visible_at: MarketTime,
    level: i128,
    fixture: &Fixture,
) -> BenchmarkLevelSnapshot {
    let mut input = BenchmarkLevelSnapshotInput {
        snapshot_id: id(identity),
        owner: owner(),
        subject_ref: subject(),
        benchmark: fixture.benchmark.clone(),
        valuation_at: time(day, 8),
        visible_at,
        level_unit: fixture.dimensionless.clone(),
        level: amount(level),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = BenchmarkLevelSnapshot::content_hash_for(&input);
    BenchmarkLevelSnapshot::new(input).unwrap()
}

fn read_query(
    from_day: u32,
    to_day: u32,
    knowledge_at: MarketTime,
    fixture: &Fixture,
) -> PortfolioPerformanceReadQuery {
    PortfolioPerformanceReadQuery {
        owner: owner(),
        subject_ref: subject(),
        member_portfolios: vec![fixture.portfolio.clone()],
        benchmark: fixture.benchmark.clone(),
        period_from: time(from_day, 8),
        period_to: time(to_day, 8),
        knowledge_at,
    }
}

#[allow(clippy::too_many_lines)]
async fn seed_catalog_dependencies(pool: &PgPool) {
    sqlx::raw_sql(
        "INSERT INTO core.subject_identities
             (tenant_id,subject_id,owner_id,latest_version)
         VALUES
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1','01ARZ3NDEKTSV4RRFFQ69G5FA0',
              '01ARZ3NDEKTSV4RRFFQ69G5FA2',1);
         INSERT INTO core.subject_versions
             (subject_id,version,display_name,funding_tier,value_added_tax_profile,
              income_tax_profile,assessment_mechanism,liability_profile,tenant_id,owner_id)
         VALUES
             ('01ARZ3NDEKTSV4RRFFQ69G5FA0',1,'R8B Subject','DR_AVAILABLE','vat',
              'income','daily','general','01ARZ3NDEKTSV4RRFFQ69G5FA1',
              '01ARZ3NDEKTSV4RRFFQ69G5FA2');
         INSERT INTO market.units
             (tenant_id,unit_id,version,owner_id,code,dimension,scale,precision,payload)
         VALUES
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1','01ARZ3NDEKTSV4RRFFQ69G5FA3',1,
              '01ARZ3NDEKTSV4RRFFQ69G5FA2','CNY','currency',2,28,decode('01','hex')),
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1','01ARZ3NDEKTSV4RRFFQ69G5FA4',1,
              '01ARZ3NDEKTSV4RRFFQ69G5FA2','ONE','dimensionless',12,28,decode('01','hex'));
         INSERT INTO market.calendars
             (tenant_id,calendar_id,version,owner_id,market,market_timezone,
              effective_from,effective_to,payload)
         VALUES
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1','01ARZ3NDEKTSV4RRFFQ69G5FA5',1,
              '01ARZ3NDEKTSV4RRFFQ69G5FA2','CIBM','Asia/Shanghai',
              '2026-08-18T00:00:00Z','2026-08-30T00:00:00Z',decode('01','hex'));
         INSERT INTO storage.blobs(tenant_id,content_hash,object_key,blob_size)
         VALUES
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1',repeat('66',32),
              'immutable/' || repeat('66',32),1),
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1',repeat('67',32),
              'immutable/' || repeat('67',32),1);
         INSERT INTO research.position_snapshots
             (tenant_id,snapshot_id,owner_id,subject_id,subject_version,observed_at,visible_at,
              content_hash,idempotency_key,fingerprint,payload)
         VALUES
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1','01ARZ3NDEKTSV4RRFFQ69G5FA8',
              '01ARZ3NDEKTSV4RRFFQ69G5FA2','01ARZ3NDEKTSV4RRFFQ69G5FA0',1,
              '2026-08-19T01:00:00Z','2026-08-19T02:00:00Z',repeat('66',32),
              'r8b-position',decode(repeat('11',32),'hex'),decode('01','hex')),
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1','01ARZ3NDEKTSV4RRFFQ69G5FA9',
              '01ARZ3NDEKTSV4RRFFQ69G5FA2','01ARZ3NDEKTSV4RRFFQ69G5FA0',1,
              '2026-08-19T01:00:00Z','2026-08-19T02:00:00Z',repeat('67',32),
              'r8b-benchmark-position',decode(repeat('12',32),'hex'),decode('01','hex'));
         INSERT INTO portfolio.books
             (tenant_id,book_id,version,owner_id,subject_id,subject_version,code,display_name,
              status,effective_from,effective_from_nanos,effective_from_timezone,
              effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
              effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
              visible_at_local_date,content_hash)
         VALUES
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1','01ARZ3NDEKTSV4RRFFQ69G5FA6',1,
              '01ARZ3NDEKTSV4RRFFQ69G5FA2','01ARZ3NDEKTSV4RRFFQ69G5FA0',1,'R8B-BOOK',
              'R8B Book','ACTIVE','2026-08-18T00:00:00Z',0,'Asia/Shanghai','2026-08-18',
              '2026-08-30T00:00:00Z',0,'Asia/Shanghai','2026-08-30',
              '2026-08-19T03:00:00Z',0,'Asia/Shanghai','2026-08-19',repeat('11',32));
         INSERT INTO portfolio.groups
             (tenant_id,group_id,version,owner_id,subject_id,subject_version,book_id,book_version,
              book_hash,parent_group_id,parent_group_version,parent_group_hash,code,display_name,
              status,effective_from,effective_from_nanos,effective_from_timezone,
              effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
              effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
              visible_at_local_date,content_hash)
         VALUES
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1','01ARZ3NDEKTSV4RRFFQ69G5FA7',1,
              '01ARZ3NDEKTSV4RRFFQ69G5FA2','01ARZ3NDEKTSV4RRFFQ69G5FA0',1,
              '01ARZ3NDEKTSV4RRFFQ69G5FA6',1,repeat('11',32),NULL,NULL,NULL,'R8B-GROUP',
              'R8B Group','ACTIVE','2026-08-18T00:00:00Z',0,'Asia/Shanghai','2026-08-18',
              '2026-08-30T00:00:00Z',0,'Asia/Shanghai','2026-08-30',
              '2026-08-19T03:00:00Z',0,'Asia/Shanghai','2026-08-19',repeat('22',32));
         INSERT INTO portfolio.benchmarks
             (tenant_id,benchmark_id,version,owner_id,subject_id,subject_version,code,display_name,
              snapshot_id,snapshot_hash,snapshot_observed_at,snapshot_observed_at_nanos,
              snapshot_observed_at_timezone,snapshot_observed_at_local_date,snapshot_visible_at,
              snapshot_visible_at_nanos,snapshot_visible_at_timezone,snapshot_visible_at_local_date,
              effective_from,effective_from_nanos,effective_from_timezone,effective_from_local_date,
              effective_to,effective_to_nanos,effective_to_timezone,effective_to_local_date,
              visible_at,visible_at_nanos,visible_at_timezone,visible_at_local_date,content_hash)
         VALUES
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1','01ARZ3NDEKTSV4RRFFQ69G5FAA',1,
              '01ARZ3NDEKTSV4RRFFQ69G5FA2','01ARZ3NDEKTSV4RRFFQ69G5FA0',1,'R8B-BENCH',
              'R8B Benchmark','01ARZ3NDEKTSV4RRFFQ69G5FA9',repeat('67',32),
              '2026-08-19T01:00:00Z',0,'Asia/Shanghai','2026-08-19',
              '2026-08-19T02:00:00Z',0,'Asia/Shanghai','2026-08-19',
              '2026-08-18T00:00:00Z',0,'Asia/Shanghai','2026-08-18',
              '2026-08-30T00:00:00Z',0,'Asia/Shanghai','2026-08-30',
              '2026-08-19T03:00:00Z',0,'Asia/Shanghai','2026-08-19',repeat('33',32));
         INSERT INTO portfolio.metric_conventions
             (tenant_id,convention_id,version,owner_id,schema_id,ytm_weighting,duration_weighting,
              convexity_weighting,coupon_weighting,remaining_life_weighting,rounding,
              freshness_limit_seconds,effective_from,effective_from_nanos,effective_from_timezone,
              effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
              effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
              visible_at_local_date,content_hash)
         VALUES
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1','01ARZ3NDEKTSV4RRFFQ69G5FAB',1,
              '01ARZ3NDEKTSV4RRFFQ69G5FA2','ficant.portfolio-metric-convention.v1',
              'MARKET_VALUE_TIMES_MODIFIED_DURATION','MARKET_VALUE','MARKET_VALUE','NOTIONAL',
              'NOTIONAL','TIES_TO_EVEN',86400,'2026-08-18T00:00:00Z',0,'Asia/Shanghai',
              '2026-08-18','2026-08-30T00:00:00Z',0,'Asia/Shanghai','2026-08-30',
              '2026-08-19T03:00:00Z',0,'Asia/Shanghai','2026-08-19',repeat('44',32));
         INSERT INTO portfolio.portfolios
             (tenant_id,portfolio_id,version,owner_id,subject_id,subject_version,book_id,
              book_version,book_hash,group_id,group_version,group_hash,code,display_name,status,
              snapshot_id,snapshot_hash,snapshot_observed_at,snapshot_observed_at_nanos,
              snapshot_observed_at_timezone,snapshot_observed_at_local_date,snapshot_visible_at,
              snapshot_visible_at_nanos,snapshot_visible_at_timezone,snapshot_visible_at_local_date,
              benchmark_id,benchmark_version,benchmark_hash,convention_id,convention_version,
              convention_hash,effective_from,effective_from_nanos,effective_from_timezone,
              effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
              effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
              visible_at_local_date,content_hash)
         VALUES
             ('01ARZ3NDEKTSV4RRFFQ69G5FA1','01ARZ3NDEKTSV4RRFFQ69G5FAC',1,
              '01ARZ3NDEKTSV4RRFFQ69G5FA2','01ARZ3NDEKTSV4RRFFQ69G5FA0',1,
              '01ARZ3NDEKTSV4RRFFQ69G5FA6',1,repeat('11',32),
              '01ARZ3NDEKTSV4RRFFQ69G5FA7',1,repeat('22',32),'R8B-PORTFOLIO','R8B Portfolio',
              'ACTIVE','01ARZ3NDEKTSV4RRFFQ69G5FA8',repeat('66',32),
              '2026-08-19T01:00:00Z',0,'Asia/Shanghai','2026-08-19',
              '2026-08-19T02:00:00Z',0,'Asia/Shanghai','2026-08-19',
              '01ARZ3NDEKTSV4RRFFQ69G5FAA',1,repeat('33',32),
              '01ARZ3NDEKTSV4RRFFQ69G5FAB',1,repeat('44',32),
              '2026-08-18T00:00:00Z',0,'Asia/Shanghai','2026-08-18',
              '2026-08-30T00:00:00Z',0,'Asia/Shanghai','2026-08-30',
              '2026-08-19T03:00:00Z',0,'Asia/Shanghai','2026-08-19',repeat('55',32));",
    )
    .execute(pool)
    .await
    .expect("R8B catalog dependencies must seed");
}

async fn insert_performance_convention(
    pool: &PgPool,
    value: &PortfolioPerformanceConvention,
    visible_at: &MarketTime,
) {
    let mut query = sqlx::query(
        "INSERT INTO portfolio.performance_conventions
         (tenant_id,convention_id,version,owner_id,schema_id,calendar_id,calendar_version,
          calendar_hash,return_method,flow_timing,valuation_frequency,rounding,
          effective_from,effective_from_nanos,effective_from_timezone,effective_from_local_date,
          effective_to,effective_to_nanos,effective_to_timezone,effective_to_local_date,
          visible_at,visible_at_nanos,visible_at_timezone,visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                 $21,$22,$23,$24,$25)",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get()).unwrap())
    .bind(value.owner().owner_id().as_str())
    .bind(value.schema_id())
    .bind(value.calendar().object_id().as_str())
    .bind(i64::try_from(value.calendar().version().unwrap().get()).unwrap())
    .bind(hash_hex(value.calendar().content_hash().unwrap()))
    .bind("DAILY_TIME_WEIGHTED")
    .bind("END_OF_DAY")
    .bind("CALENDAR_SESSION_CLOSE")
    .bind("TIES_TO_EVEN");
    query = bind_time(query, value.effective_from(), None);
    query = bind_time(query, value.effective_to(), None);
    query = bind_time(query, visible_at, None);
    query
        .bind(hash_hex(value.content_hash()))
        .execute(pool)
        .await
        .expect("performance convention must seed");
}

async fn insert_valuation(
    pool: &PgPool,
    value: &PortfolioValuationSnapshot,
    valuation_timezone_override: Option<&str>,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    let position = value.position_snapshot();
    let convention = value.performance_convention();
    let mut query = sqlx::query(
        "INSERT INTO portfolio.valuation_snapshots
         (tenant_id,snapshot_id,owner_id,subject_id,subject_version,portfolio_id,
          portfolio_version,portfolio_hash,position_snapshot_id,position_snapshot_hash,
          position_observed_at,position_observed_at_nanos,position_observed_at_timezone,
          position_observed_at_local_date,position_visible_at,position_visible_at_nanos,
          position_visible_at_timezone,position_visible_at_local_date,convention_id,
          convention_version,convention_hash,valuation_at,valuation_at_nanos,
          valuation_at_timezone,valuation_at_local_date,visible_at,visible_at_nanos,
          visible_at_timezone,visible_at_local_date,currency_unit_id,currency_unit_version,
          gross_assets_scaled,liabilities_scaled,net_asset_value_scaled,
          net_external_flow_scaled,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,
                 $19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34,$35,$36)",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.snapshot_id().as_str())
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get()).unwrap())
    .bind(value.portfolio().object_id().as_str())
    .bind(i64::try_from(value.portfolio().version().unwrap().get()).unwrap())
    .bind(hash_hex(value.portfolio().content_hash().unwrap()))
    .bind(position.snapshot_id().as_str())
    .bind(hash_hex(position.content_hash()));
    query = bind_time(query, position.observed_at(), None);
    query = bind_time(query, position.visible_at(), None);
    query = query
        .bind(convention.reference().id().as_str())
        .bind(i64::try_from(convention.reference().version().get()).unwrap())
        .bind(hash_hex(convention.content_hash()));
    query = bind_time(query, value.valuation_at(), valuation_timezone_override);
    query = bind_time(query, value.visible_at(), None);
    query
        .bind(value.currency_unit().unit_id().as_str())
        .bind(i64::try_from(value.currency_unit().version().get()).unwrap())
        .bind(value.gross_assets().scaled().to_string())
        .bind(value.liabilities().scaled().to_string())
        .bind(value.net_asset_value().scaled().to_string())
        .bind(value.net_external_flow().scaled().to_string())
        .bind(hash_hex(value.content_hash()))
        .execute(pool)
        .await
}

async fn insert_benchmark_level(
    pool: &PgPool,
    value: &BenchmarkLevelSnapshot,
    stored_hash: &ContentHash,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    let mut query = sqlx::query(
        "INSERT INTO portfolio.benchmark_level_snapshots
         (tenant_id,snapshot_id,owner_id,subject_id,subject_version,benchmark_id,
          benchmark_version,benchmark_hash,valuation_at,valuation_at_nanos,
          valuation_at_timezone,valuation_at_local_date,visible_at,visible_at_nanos,
          visible_at_timezone,visible_at_local_date,level_unit_id,level_unit_version,
          level_scaled,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.snapshot_id().as_str())
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get()).unwrap())
    .bind(value.benchmark().object_id().as_str())
    .bind(i64::try_from(value.benchmark().version().unwrap().get()).unwrap())
    .bind(hash_hex(value.benchmark().content_hash().unwrap()));
    query = bind_time(query, value.valuation_at(), None);
    query = bind_time(query, value.visible_at(), None);
    query
        .bind(value.level_unit().unit_id().as_str())
        .bind(i64::try_from(value.level_unit().version().get()).unwrap())
        .bind(value.level().scaled().to_string())
        .bind(hash_hex(stored_hash))
        .execute(pool)
        .await
}

fn bind_time<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    value: &MarketTime,
    timezone_override: Option<&'q str>,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    query
        .bind(value.instant())
        .bind(i32::try_from(value.instant().timestamp_subsec_nanos()).unwrap())
        .bind(timezone_override.map_or_else(|| value.market_timezone().to_owned(), str::to_owned))
        .bind(value.local_trading_date())
}

fn amount(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value * 1_000_000_000_000)
}

fn exact_ref(index: usize, hash: u8) -> LineageRef {
    LineageRef::new(
        id(index),
        Some(Version::new(1).unwrap()),
        Some(hash_byte(hash)),
    )
    .unwrap()
}

fn hash_byte(value: u8) -> ContentHash {
    ContentHash::from_bytes(&[value; 32]).unwrap()
}

fn hash_hex(value: &ContentHash) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            use std::fmt::Write as _;
            write!(text, "{byte:02x}").unwrap();
            text
        })
}

fn time(day: u32, hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0)
            .single()
            .unwrap(),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 8, day).unwrap(),
    )
    .unwrap()
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
