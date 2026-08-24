#![allow(clippy::too_many_lines)]

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use ficant_application::ApplicationError;
use ficant_application::ports::{
    AeadCursorCodec, AuthorizedPrincipal, BlobStore, CursorKey, DefinitionRepository,
    DefinitionUseCase, DefinitionValue, FoundationChangeContext, GovernedAppendDefinitionVersion,
    IdempotencyKey, PortfolioCatalogRepository, PortfolioCatalogTemporalScope,
    PortfolioPerformanceReadQuery, PortfolioPerformanceRepository, PositionSnapshotRepository,
    SnapshotRepository, SnapshotValue, stored_definition_content_hash,
};
use ficant_application::use_cases::position_views::{
    PositionSnapshotPayload, PublishPositionSnapshot,
};
use ficant_domain::governance::{ChangeJustification, PlatformRole, SourceDocumentRef};
use ficant_domain::market::{Calendar, CalendarInput, CalendarSession};
use ficant_domain::portfolio::{
    BenchmarkLevelSnapshot, BenchmarkLevelSnapshotInput, PortfolioDecimalRounding,
    PortfolioExternalFlowTiming, PortfolioPerformanceConvention,
    PortfolioPerformanceConventionInput, PortfolioPerformanceConventionRef,
    PortfolioPerformanceReturnMethod, PortfolioSnapshotBinding, PortfolioValuationFrequency,
    PortfolioValuationSnapshot, PortfolioValuationSnapshotInput,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, FixedDecimal, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{PositionSnapshot, PositionSnapshotInput};
use ficant_domain::{ContentAddressed, Lineaged, VersionedDefinition};
use ficant_storage::postgres::PostgresRepository;
use ficant_storage::s3::S3BlobStore;
use serde::Deserialize;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

type AnyError = Box<dyn Error + Send + Sync>;

const FIXTURE_SCHEMA: &str = "ficant.portfolio-performance-fixture.v1";
const ADMIN_ACTOR_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const CALENDAR_CHANGE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC9";
const POSITION_CHANGE_IDS: [&str; 2] = ["01ARZ3NDEKTSV4RRFFQ69G5FCA", "01ARZ3NDEKTSV4RRFFQ69G5FCB"];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureSource {
    schema_id: String,
    tenant_id: String,
    owner_id: String,
    subject: VersionSource,
    as_of: String,
    knowledge_at: String,
    market_timezone: String,
    calendar: CalendarSource,
    performance_convention: ConventionSource,
    currency_unit: VersionSource,
    dimensionless_unit: VersionSource,
    benchmark_id: String,
    portfolios: Vec<PortfolioSource>,
    benchmark_levels: Vec<BenchmarkLevelSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionSource {
    id: String,
    version: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CalendarSource {
    id: String,
    version: u64,
    market: String,
    effective_from: String,
    effective_to: String,
    sessions: Vec<CalendarSessionSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CalendarSessionSource {
    local_date: String,
    open_local_time: String,
    close_local_time: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConventionSource {
    id: String,
    version: u64,
    visible_at: String,
    effective_from: String,
    effective_to: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortfolioSource {
    portfolio_id: String,
    current_position_snapshot_id: String,
    historical_position_snapshot_id: String,
    valuations: Vec<ValuationSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValuationSource {
    snapshot_id: String,
    session_index: usize,
    visible_at: String,
    gross_assets: String,
    liabilities: String,
    net_asset_value: String,
    net_external_flow: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BenchmarkLevelSource {
    snapshot_id: String,
    session_index: usize,
    visible_at: String,
    level: String,
}

#[tokio::main]
async fn main() -> Result<(), AnyError> {
    let fixture_path = fixture_argument()?;
    let source: FixtureSource = serde_json::from_slice(&fs::read(&fixture_path)?)?;
    validate_source(&source)?;
    let timezone = source.market_timezone.parse::<Tz>()?;
    let owner = OwnerRef::new(ulid(&source.tenant_id)?, ulid(&source.owner_id)?);
    let subject_ref = version_ref(&source.subject)?;
    let as_of = market_time(&source.as_of, timezone)?;
    let knowledge_at = market_time(&source.knowledge_at, timezone)?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&required_environment("FICANT_BOOTSTRAP_DATABASE_URL")?)
        .await?;
    let cursor = Arc::new(app(AeadCursorCodec::new(
        app(CursorKey::new("r8b-bootstrap", [0x42; 32]))?,
        Vec::new(),
    ))?);
    let repository = Arc::new(PostgresRepository::new(pool.clone(), cursor));
    let blob_store = Arc::new(app(S3BlobStore::new(
        &required_environment("FICANT_BOOTSTRAP_S3_ENDPOINT")?,
        required_environment("FICANT_BOOTSTRAP_S3_BUCKET")?,
        &required_environment("FICANT_BOOTSTRAP_S3_ACCESS_KEY")?,
        &required_environment("FICANT_BOOTSTRAP_S3_SECRET_KEY")?,
        pool.clone(),
    ))?);
    let principal = administrator(&owner)?;

    let (calendar, session_closes) = build_calendar(&source, &owner, timezone)?;
    publish_calendar(repository.as_ref(), &principal, &calendar, &knowledge_at).await?;
    let calendar_hash =
        stored_definition_content_hash(&DefinitionValue::Calendar(calendar.clone()));

    let temporal = app(PortfolioCatalogTemporalScope::new(
        owner.clone(),
        subject_ref.clone(),
        as_of.clone(),
        knowledge_at.clone(),
    ))?;
    let catalog = app(repository
        .read_catalog_snapshot(principal.access_scope(), &temporal)
        .await)?;
    let benchmark = catalog
        .benchmarks()
        .iter()
        .find(|record| record.value().reference().id().as_str() == source.benchmark_id)
        .ok_or("R8B exact Benchmark is absent from the R8A catalog")?
        .value()
        .clone();

    let convention = build_convention(&source, &owner, &calendar, calendar_hash, timezone)?;
    insert_convention(
        &pool,
        &convention,
        &market_time(&source.performance_convention.visible_at, timezone)?,
    )
    .await?;

    let currency = unit_ref(&source.currency_unit)?;
    let dimensionless = unit_ref(&source.dimensionless_unit)?;
    let snapshots: &dyn SnapshotRepository = repository.as_ref();
    let blobs: &dyn BlobStore = blob_store.as_ref();
    let publisher = PublishPositionSnapshot::new(blobs, snapshots);
    let historical_visible_at = market_time(&source.performance_convention.visible_at, timezone)?;
    let mut exact_portfolios = Vec::with_capacity(source.portfolios.len());
    let mut valuations = Vec::with_capacity(source.portfolios.len() * session_closes.len());

    for (portfolio_index, source_portfolio) in source.portfolios.iter().enumerate() {
        let portfolio = catalog
            .portfolios()
            .iter()
            .find(|record| {
                record.value().reference().id().as_str() == source_portfolio.portfolio_id
            })
            .ok_or("R8B exact Portfolio is absent from the R8A catalog")?
            .value()
            .clone();
        if portfolio.position_snapshot().snapshot_id().as_str()
            != source_portfolio.current_position_snapshot_id
        {
            return Err("R8B current PositionSnapshot binding drifted from R8A".into());
        }
        let current = app(repository
            .get_position_snapshot(
                principal.access_scope(),
                ulid(&source_portfolio.current_position_snapshot_id)?,
                knowledge_at.clone(),
            )
            .await)?
        .ok_or("R8B current PositionSnapshot payload is missing")?;
        let historical = historical_snapshot(
            &current,
            ulid(&source_portfolio.historical_position_snapshot_id)?,
            MarketTime::new(
                session_closes[0].instant() - chrono::Duration::hours(1),
                timezone.name(),
                session_closes[0].local_trading_date(),
            )?,
            historical_visible_at.clone(),
        )?;
        publish_historical_position(
            repository.as_ref(),
            &publisher,
            &principal,
            &historical,
            POSITION_CHANGE_IDS[portfolio_index],
            &knowledge_at,
        )
        .await?;

        let exact = exact_lineage(&portfolio)?;
        exact_portfolios.push(exact.clone());
        for source_value in &source_portfolio.valuations {
            let position = if source_value.session_index == 0 {
                &historical
            } else {
                &current
            };
            let value = build_valuation(
                source_value,
                &owner,
                &subject_ref,
                exact.clone(),
                position,
                &convention,
                currency.clone(),
                &session_closes,
                timezone,
            )?;
            insert_valuation(&pool, &value).await?;
            valuations.push(value);
        }
    }

    let exact_benchmark = exact_lineage(&benchmark)?;
    let mut levels = Vec::with_capacity(source.benchmark_levels.len());
    for source_level in &source.benchmark_levels {
        let value = build_benchmark_level(
            source_level,
            &owner,
            &subject_ref,
            exact_benchmark.clone(),
            dimensionless.clone(),
            &session_closes,
            timezone,
        )?;
        insert_benchmark_level(&pool, &value).await?;
        levels.push(value);
    }

    verify_inputs(
        repository.as_ref(),
        &principal,
        &owner,
        &subject_ref,
        exact_portfolios,
        exact_benchmark,
        &convention,
        &session_closes,
        &knowledge_at,
        &valuations,
        &levels,
    )
    .await?;
    println!(
        "R8B performance fixture ready: owner={} portfolios={} sessions={} valuations={} benchmark_levels={}",
        owner.owner_id(),
        source.portfolios.len(),
        session_closes.len(),
        valuations.len(),
        levels.len()
    );
    pool.close().await;
    Ok(())
}

fn validate_source(source: &FixtureSource) -> Result<(), AnyError> {
    if source.schema_id != FIXTURE_SCHEMA
        || source.subject.version == 0
        || source.calendar.version != 2
        || source.performance_convention.version != 1
        || source.calendar.sessions.len() != 2
        || source.portfolios.len() != 2
        || source.portfolios.iter().any(|value| {
            value.valuations.len() != 2
                || value
                    .valuations
                    .iter()
                    .enumerate()
                    .any(|(index, valuation)| valuation.session_index != index)
        })
        || source.benchmark_levels.len() != 2
        || source
            .benchmark_levels
            .iter()
            .enumerate()
            .any(|(index, value)| value.session_index != index)
    {
        return Err("R8B performance fixture schema/count/order invariant failed".into());
    }
    Ok(())
}

fn build_calendar(
    source: &FixtureSource,
    owner: &OwnerRef,
    timezone: Tz,
) -> Result<(Calendar, Vec<MarketTime>), AnyError> {
    let mut sessions = Vec::with_capacity(source.calendar.sessions.len());
    let mut closes = Vec::with_capacity(source.calendar.sessions.len());
    for source_session in &source.calendar.sessions {
        let date = NaiveDate::parse_from_str(&source_session.local_date, "%Y-%m-%d")?;
        let open = NaiveTime::parse_from_str(&source_session.open_local_time, "%H:%M:%S")?;
        let close = NaiveTime::parse_from_str(&source_session.close_local_time, "%H:%M:%S")?;
        sessions.push(CalendarSession::open(date, open, close)?);
        let instant = timezone
            .from_local_datetime(&date.and_time(close))
            .single()
            .ok_or("R8B Calendar close is ambiguous")?
            .with_timezone(&Utc);
        closes.push(MarketTime::new(instant, timezone.name(), date)?);
    }
    let calendar = Calendar::new(CalendarInput {
        calendar_id: ulid(&source.calendar.id)?,
        version: Version::new(source.calendar.version)?,
        owner: owner.clone(),
        market: source.calendar.market.clone(),
        market_timezone: timezone.name().to_owned(),
        effective: EffectivePeriod::new(
            market_time(&source.calendar.effective_from, timezone)?,
            market_time(&source.calendar.effective_to, timezone)?,
        )?,
        sessions,
    })?;
    Ok((calendar, closes))
}

fn build_convention(
    source: &FixtureSource,
    owner: &OwnerRef,
    calendar: &Calendar,
    calendar_hash: ContentHash,
    timezone: Tz,
) -> Result<PortfolioPerformanceConvention, AnyError> {
    let mut input = PortfolioPerformanceConventionInput {
        convention: version_ref(&VersionSource {
            id: source.performance_convention.id.clone(),
            version: source.performance_convention.version,
        })?,
        owner: owner.clone(),
        schema_id: "ficant.portfolio-performance-convention.v1".to_owned(),
        calendar: LineageRef::new(
            ulid(calendar.identity())?,
            Some(Version::new(calendar.version())?),
            Some(calendar_hash),
        )?,
        return_method: PortfolioPerformanceReturnMethod::DailyTimeWeighted,
        flow_timing: PortfolioExternalFlowTiming::EndOfDay,
        valuation_frequency: PortfolioValuationFrequency::CalendarSessionClose,
        rounding: PortfolioDecimalRounding::TiesToEven,
        effective_from: market_time(&source.performance_convention.effective_from, timezone)?,
        effective_to: market_time(&source.performance_convention.effective_to, timezone)?,
        content_hash: ContentHash::digest(b"pending"),
    };
    input.content_hash = PortfolioPerformanceConvention::content_hash_for(&input);
    Ok(PortfolioPerformanceConvention::new(input)?)
}

fn historical_snapshot(
    current: &PositionSnapshot,
    snapshot_id: Ulid,
    observed_at: MarketTime,
    visible_at: MarketTime,
) -> Result<PositionSnapshot, AnyError> {
    let mut input = PositionSnapshotInput {
        snapshot_id,
        owner: current.owner().clone(),
        subject_ref: current.subject_ref().clone(),
        observed_at,
        visible_at,
        content_hash: ContentHash::digest(b"pending"),
        lineage: current.lineage().to_vec(),
        positions: current.positions().to_vec(),
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    Ok(PositionSnapshot::new(input)?)
}

#[allow(clippy::too_many_arguments)]
fn build_valuation(
    source: &ValuationSource,
    owner: &OwnerRef,
    subject_ref: &VersionRef,
    portfolio: LineageRef,
    position: &PositionSnapshot,
    convention: &PortfolioPerformanceConvention,
    currency: UnitRef,
    sessions: &[MarketTime],
    timezone: Tz,
) -> Result<PortfolioValuationSnapshot, AnyError> {
    let mut input = PortfolioValuationSnapshotInput {
        snapshot_id: ulid(&source.snapshot_id)?,
        owner: owner.clone(),
        subject_ref: subject_ref.clone(),
        portfolio,
        position_snapshot: position_binding(position)?,
        performance_convention: PortfolioPerformanceConventionRef::new(
            convention.reference().clone(),
            convention.content_hash().clone(),
        ),
        valuation_at: sessions
            .get(source.session_index)
            .ok_or("R8B valuation session index is invalid")?
            .clone(),
        visible_at: market_time(&source.visible_at, timezone)?,
        currency_unit: currency,
        gross_assets: decimal(&source.gross_assets)?,
        liabilities: decimal(&source.liabilities)?,
        net_asset_value: decimal(&source.net_asset_value)?,
        net_external_flow: decimal(&source.net_external_flow)?,
        content_hash: ContentHash::digest(b"pending"),
    };
    input.content_hash = PortfolioValuationSnapshot::content_hash_for(&input);
    Ok(PortfolioValuationSnapshot::new(input)?)
}

#[allow(clippy::too_many_arguments)]
fn build_benchmark_level(
    source: &BenchmarkLevelSource,
    owner: &OwnerRef,
    subject_ref: &VersionRef,
    benchmark: LineageRef,
    unit: UnitRef,
    sessions: &[MarketTime],
    timezone: Tz,
) -> Result<BenchmarkLevelSnapshot, AnyError> {
    let mut input = BenchmarkLevelSnapshotInput {
        snapshot_id: ulid(&source.snapshot_id)?,
        owner: owner.clone(),
        subject_ref: subject_ref.clone(),
        benchmark,
        valuation_at: sessions
            .get(source.session_index)
            .ok_or("R8B Benchmark session index is invalid")?
            .clone(),
        visible_at: market_time(&source.visible_at, timezone)?,
        level_unit: unit,
        level: decimal(&source.level)?,
        content_hash: ContentHash::digest(b"pending"),
    };
    input.content_hash = BenchmarkLevelSnapshot::content_hash_for(&input);
    Ok(BenchmarkLevelSnapshot::new(input)?)
}

async fn publish_calendar(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    calendar: &Calendar,
    occurred_at: &MarketTime,
) -> Result<(), AnyError> {
    let value = DefinitionValue::Calendar(calendar.clone());
    if let Some(existing) = app(repository
        .get_version(
            principal.access_scope(),
            ulid(value.identity())?,
            Version::new(value.version())?,
        )
        .await)?
    {
        if existing != value {
            return Err("existing R8B Calendar v2 differs".into());
        }
        return Ok(());
    }
    let command = app(GovernedAppendDefinitionVersion::new(
        change_context(
            principal,
            CALENDAR_CHANGE_ID,
            "bootstrap R8B two-session Calendar",
            occurred_at,
        )?,
        Some(Version::new(1)?),
        value.clone(),
        app(IdempotencyKey::new("r8b-portfolio-calendar-v2"))?,
    ))?;
    let stored = app(DefinitionUseCase::new(repository).append(command).await)?;
    if stored != value {
        return Err("stored R8B Calendar v2 differs".into());
    }
    Ok(())
}

async fn publish_historical_position(
    repository: &PostgresRepository,
    publisher: &PublishPositionSnapshot<'_>,
    principal: &AuthorizedPrincipal,
    snapshot: &PositionSnapshot,
    change_id: &str,
    occurred_at: &MarketTime,
) -> Result<(), AnyError> {
    if let Some(existing) = app(repository
        .get_by_id(principal.access_scope(), snapshot.id().clone())
        .await)?
    {
        return match existing {
            SnapshotValue::Position(existing) if existing == *snapshot => Ok(()),
            SnapshotValue::Position(_)
            | SnapshotValue::Data(_)
            | SnapshotValue::DataHealthThresholdProfile(_)
            | SnapshotValue::Universe(_) => {
                Err("existing historical PositionSnapshot differs".into())
            }
        };
    }
    let payload = app(PositionSnapshotPayload::new(
        snapshot.clone(),
        app(IdempotencyKey::new(format!(
            "r8b-historical-position-{}",
            snapshot.id()
        )))?,
    ))?;
    let stored = app(publisher
        .execute(
            change_context(
                principal,
                change_id,
                "bootstrap R8B historical PositionSnapshot",
                occurred_at,
            )?,
            payload,
        )
        .await)?;
    if stored != *snapshot {
        return Err("stored historical PositionSnapshot differs".into());
    }
    Ok(())
}

async fn insert_convention(
    pool: &PgPool,
    value: &PortfolioPerformanceConvention,
    visible_at: &MarketTime,
) -> Result<(), AnyError> {
    let mut query = sqlx::query(
        "INSERT INTO portfolio.performance_conventions
         (tenant_id,convention_id,version,owner_id,schema_id,calendar_id,calendar_version,
          calendar_hash,return_method,flow_timing,valuation_frequency,rounding,
          effective_from,effective_from_nanos,effective_from_timezone,effective_from_local_date,
          effective_to,effective_to_nanos,effective_to_timezone,effective_to_local_date,
          visible_at,visible_at_nanos,visible_at_timezone,visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                 $21,$22,$23,$24,$25)
         ON CONFLICT (tenant_id,convention_id,version) DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get())?)
    .bind(value.owner().owner_id().as_str())
    .bind(value.schema_id())
    .bind(value.calendar().object_id().as_str())
    .bind(i64::try_from(
        value
            .calendar()
            .version()
            .ok_or("Calendar version missing")?
            .get(),
    )?)
    .bind(hash_hex(
        value
            .calendar()
            .content_hash()
            .ok_or("Calendar hash missing")?,
    ))
    .bind("DAILY_TIME_WEIGHTED")
    .bind("END_OF_DAY")
    .bind("CALENDAR_SESSION_CLOSE")
    .bind("TIES_TO_EVEN");
    query = bind_time(query, value.effective_from());
    query = bind_time(query, value.effective_to());
    query = bind_time(query, visible_at);
    query
        .bind(hash_hex(value.content_hash()))
        .execute(pool)
        .await?;
    assert_stored_hash(
        pool,
        "portfolio.performance_conventions",
        "convention_id",
        value.reference().id(),
        value.content_hash(),
    )
    .await
}

async fn insert_valuation(
    pool: &PgPool,
    value: &PortfolioValuationSnapshot,
) -> Result<(), AnyError> {
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
                 $19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34,$35,$36)
         ON CONFLICT (tenant_id,snapshot_id) DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.snapshot_id().as_str())
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get())?)
    .bind(value.portfolio().object_id().as_str())
    .bind(i64::try_from(
        value
            .portfolio()
            .version()
            .ok_or("Portfolio version missing")?
            .get(),
    )?)
    .bind(hash_hex(
        value
            .portfolio()
            .content_hash()
            .ok_or("Portfolio hash missing")?,
    ))
    .bind(position.snapshot_id().as_str())
    .bind(hash_hex(position.content_hash()));
    query = bind_time(query, position.observed_at());
    query = bind_time(query, position.visible_at());
    query = query
        .bind(convention.reference().id().as_str())
        .bind(i64::try_from(convention.reference().version().get())?)
        .bind(hash_hex(convention.content_hash()));
    query = bind_time(query, value.valuation_at());
    query = bind_time(query, value.visible_at());
    query
        .bind(value.currency_unit().unit_id().as_str())
        .bind(i64::try_from(value.currency_unit().version().get())?)
        .bind(value.gross_assets().scaled().to_string())
        .bind(value.liabilities().scaled().to_string())
        .bind(value.net_asset_value().scaled().to_string())
        .bind(value.net_external_flow().scaled().to_string())
        .bind(hash_hex(value.content_hash()))
        .execute(pool)
        .await?;
    assert_stored_hash(
        pool,
        "portfolio.valuation_snapshots",
        "snapshot_id",
        value.snapshot_id(),
        value.content_hash(),
    )
    .await
}

async fn insert_benchmark_level(
    pool: &PgPool,
    value: &BenchmarkLevelSnapshot,
) -> Result<(), AnyError> {
    let mut query = sqlx::query(
        "INSERT INTO portfolio.benchmark_level_snapshots
         (tenant_id,snapshot_id,owner_id,subject_id,subject_version,benchmark_id,
          benchmark_version,benchmark_hash,valuation_at,valuation_at_nanos,
          valuation_at_timezone,valuation_at_local_date,visible_at,visible_at_nanos,
          visible_at_timezone,visible_at_local_date,level_unit_id,level_unit_version,
          level_scaled,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
         ON CONFLICT (tenant_id,snapshot_id) DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.snapshot_id().as_str())
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get())?)
    .bind(value.benchmark().object_id().as_str())
    .bind(i64::try_from(
        value
            .benchmark()
            .version()
            .ok_or("Benchmark version missing")?
            .get(),
    )?)
    .bind(hash_hex(
        value
            .benchmark()
            .content_hash()
            .ok_or("Benchmark hash missing")?,
    ));
    query = bind_time(query, value.valuation_at());
    query = bind_time(query, value.visible_at());
    query
        .bind(value.level_unit().unit_id().as_str())
        .bind(i64::try_from(value.level_unit().version().get())?)
        .bind(value.level().scaled().to_string())
        .bind(hash_hex(value.content_hash()))
        .execute(pool)
        .await?;
    assert_stored_hash(
        pool,
        "portfolio.benchmark_level_snapshots",
        "snapshot_id",
        value.snapshot_id(),
        value.content_hash(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn verify_inputs(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    owner: &OwnerRef,
    subject_ref: &VersionRef,
    portfolios: Vec<LineageRef>,
    benchmark: LineageRef,
    convention: &PortfolioPerformanceConvention,
    sessions: &[MarketTime],
    knowledge_at: &MarketTime,
    expected_valuations: &[PortfolioValuationSnapshot],
    expected_levels: &[BenchmarkLevelSnapshot],
) -> Result<(), AnyError> {
    let query = PortfolioPerformanceReadQuery {
        owner: owner.clone(),
        subject_ref: subject_ref.clone(),
        member_portfolios: portfolios,
        benchmark,
        period_from: sessions[0].clone(),
        period_to: sessions[1].clone(),
        knowledge_at: knowledge_at.clone(),
    };
    let actual_valuations = app(repository
        .read_valuation_snapshots(principal.access_scope(), &query)
        .await)?;
    let actual_levels = app(repository
        .read_benchmark_level_snapshots(principal.access_scope(), &query)
        .await)?;
    if actual_valuations != expected_valuations || actual_levels != expected_levels {
        return Err("R8B performance input read-back differs".into());
    }
    let exact_convention = app(repository
        .read_performance_convention_exact(
            principal.access_scope(),
            owner,
            convention.reference(),
            convention.content_hash(),
            knowledge_at,
        )
        .await)?
    .ok_or("R8B performance convention read-back is missing")?;
    if exact_convention.value() != convention {
        return Err("R8B performance convention read-back differs".into());
    }
    Ok(())
}

async fn assert_stored_hash(
    pool: &PgPool,
    table: &str,
    id_column: &str,
    id: &Ulid,
    expected: &ContentHash,
) -> Result<(), AnyError> {
    let sql =
        format!("SELECT content_hash::text FROM {table} WHERE tenant_id=$1 AND {id_column}=$2");
    let stored: String = sqlx::query_scalar(&sql)
        .bind("01ARZ3NDEKTSV4RRFFQ69G5FA1")
        .bind(id.as_str())
        .fetch_one(pool)
        .await?;
    if stored != hash_hex(expected) {
        return Err(format!("existing immutable {table}/{id} differs").into());
    }
    Ok(())
}

fn administrator(owner: &OwnerRef) -> Result<AuthorizedPrincipal, AnyError> {
    app(AuthorizedPrincipal::new(
        "portfolio-performance-bootstrap".to_owned(),
        ulid(ADMIN_ACTOR_ID)?,
        owner.tenant_id().clone(),
        vec![owner.owner_id().clone()],
        PlatformRole::PlatformAdmin,
        vec![
            "definitions:write".to_owned(),
            "positions:write".to_owned(),
            "portfolio:read".to_owned(),
        ],
        ContentHash::digest(b"portfolio-performance-bootstrap-local-principal"),
    ))
}

fn change_context(
    principal: &AuthorizedPrincipal,
    record_id: &str,
    reason: &str,
    occurred_at: &MarketTime,
) -> Result<FoundationChangeContext, AnyError> {
    app(FoundationChangeContext::administrator(
        principal.clone(),
        ChangeJustification::new(
            reason,
            vec![SourceDocumentRef::new(
                "urn:ficant:r8b:portfolio-performance-fixture",
                ContentHash::digest(b"ficant.portfolio-performance-fixture.v1"),
            )?],
        )?,
        ulid(record_id)?,
        occurred_at.clone(),
    ))
}

fn position_binding(value: &PositionSnapshot) -> Result<PortfolioSnapshotBinding, AnyError> {
    Ok(PortfolioSnapshotBinding::new(
        value.id().clone(),
        value.content_hash().clone(),
        value.observed_at().clone(),
        value.visible_at().clone(),
    )?)
}

fn exact_lineage<T>(value: &T) -> Result<LineageRef, AnyError>
where
    T: ContentAddressed + VersionedDefinition,
{
    Ok(LineageRef::new(
        ulid(value.identity())?,
        Some(Version::new(value.version())?),
        Some(value.content_hash().clone()),
    )?)
}

fn bind_time<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    value: &MarketTime,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    query
        .bind(value.instant())
        .bind(i32::try_from(value.instant().timestamp_subsec_nanos()).unwrap())
        .bind(value.market_timezone().to_owned())
        .bind(value.local_trading_date())
}

fn decimal(value: &str) -> Result<FixedDecimal, AnyError> {
    let (sign, unsigned) = value
        .strip_prefix('-')
        .map_or((1_i128, value), |unsigned| (-1_i128, unsigned));
    let (whole, fraction) = unsigned
        .split_once('.')
        .ok_or("R8B Decimal must contain an explicit scale")?;
    if fraction.len() != 12
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("R8B Decimal must be a scale-12 finite string".into());
    }
    let magnitude = whole
        .parse::<i128>()?
        .checked_mul(1_000_000_000_000)
        .and_then(|value| value.checked_add(fraction.parse::<i128>().ok()?))
        .ok_or("R8B Decimal exceeds FixedDecimal")?;
    Ok(FixedDecimal::from_scaled(sign * magnitude))
}

fn market_time(value: &str, timezone: Tz) -> Result<MarketTime, AnyError> {
    let instant = DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc);
    Ok(MarketTime::new(
        instant,
        timezone.name(),
        instant.with_timezone(&timezone).date_naive(),
    )?)
}

fn version_ref(value: &VersionSource) -> Result<VersionRef, AnyError> {
    Ok(VersionRef::new(
        ulid(&value.id)?,
        Version::new(value.version)?,
    ))
}

fn unit_ref(value: &VersionSource) -> Result<UnitRef, AnyError> {
    Ok(UnitRef::new(ulid(&value.id)?, Version::new(value.version)?))
}

fn ulid(value: &str) -> Result<Ulid, AnyError> {
    Ok(Ulid::new(value.to_owned())?)
}

fn hash_hex(value: &ContentHash) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            use std::fmt::Write as _;
            write!(text, "{byte:02x}").expect("String writes cannot fail");
            text
        })
}

fn fixture_argument() -> Result<PathBuf, AnyError> {
    let mut arguments = env::args_os().skip(1);
    let flag = arguments.next().ok_or("--fixture is required")?;
    if flag != "--fixture" {
        return Err("only --fixture <path> is accepted".into());
    }
    let path = PathBuf::from(arguments.next().ok_or("--fixture path is required")?);
    if arguments.next().is_some() || !path.is_file() {
        return Err("--fixture must name one existing JSON file".into());
    }
    Ok(path)
}

fn required_environment(name: &str) -> Result<String, AnyError> {
    let value = env::var(name)?;
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty").into());
    }
    Ok(value)
}

fn app<T>(result: Result<T, ApplicationError>) -> Result<T, AnyError> {
    result.map_err(|error| {
        std::io::Error::other(format!(
            "bootstrap application failure: {:?} (retryable={})",
            error.category(),
            error.retryable()
        ))
        .into()
    })
}
