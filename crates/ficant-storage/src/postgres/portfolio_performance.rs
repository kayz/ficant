use std::collections::BTreeSet;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, PortfolioPerformanceReadQuery, PortfolioPerformanceRepository,
    VisiblePortfolioPerformanceConvention,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
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
use sqlx::Row;
use sqlx::postgres::PgRow;

use super::PostgresRepository;
use super::common::map_sqlx_error;

#[async_trait]
impl PortfolioPerformanceRepository for PostgresRepository {
    async fn read_valuation_snapshots(
        &self,
        scope: &AccessScope,
        query: &PortfolioPerformanceReadQuery,
    ) -> ApplicationResult<Vec<PortfolioValuationSnapshot>> {
        authorize(scope, &query.owner)?;
        validate_query(query)?;
        let ids = query
            .member_portfolios
            .iter()
            .map(|reference| reference.object_id().as_str().to_owned())
            .collect::<Vec<_>>();
        let rows = sqlx::query(
            "SELECT * FROM portfolio.valuation_snapshots
             WHERE tenant_id=$1 AND owner_id=$2 AND subject_id=$3 AND subject_version=$4
               AND portfolio_id::text = ANY($5)
               AND valuation_at_local_date BETWEEN $6 AND $7
               AND visible_at <= $8
             ORDER BY portfolio_id, portfolio_version, valuation_at, valuation_at_nanos,
                      visible_at DESC, visible_at_nanos DESC, snapshot_id",
        )
        .bind(query.owner.tenant_id().as_str())
        .bind(query.owner.owner_id().as_str())
        .bind(query.subject_ref.id().as_str())
        .bind(checked_i64(query.subject_ref.version().get())?)
        .bind(ids)
        .bind(query.period_from.local_trading_date())
        .bind(query.period_to.local_trading_date())
        .bind(query.knowledge_at.instant())
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;

        let mut selected = BTreeSet::new();
        let mut result = Vec::new();
        for row in rows {
            let snapshot = decode_valuation_snapshot(&row)?;
            if snapshot.visible_at().instant() > query.knowledge_at.instant()
                || !query.member_portfolios.contains(snapshot.portfolio())
            {
                continue;
            }
            let key = format!(
                "{}@{}#{}:{}",
                snapshot.portfolio().object_id().as_str(),
                snapshot.portfolio().version().ok_or_else(integrity)?.get(),
                encode_hash(snapshot.portfolio().content_hash().ok_or_else(integrity)?),
                snapshot.valuation_at().local_trading_date()
            );
            if selected.insert(key) {
                result.push(snapshot);
            }
        }
        result.sort_by(|left, right| {
            left.portfolio()
                .object_id()
                .cmp(right.portfolio().object_id())
                .then_with(|| {
                    left.valuation_at()
                        .instant()
                        .cmp(&right.valuation_at().instant())
                })
        });
        Ok(result)
    }

    async fn read_benchmark_level_snapshots(
        &self,
        scope: &AccessScope,
        query: &PortfolioPerformanceReadQuery,
    ) -> ApplicationResult<Vec<BenchmarkLevelSnapshot>> {
        authorize(scope, &query.owner)?;
        validate_query(query)?;
        let rows = sqlx::query(
            "SELECT * FROM portfolio.benchmark_level_snapshots
             WHERE tenant_id=$1 AND owner_id=$2 AND subject_id=$3 AND subject_version=$4
               AND benchmark_id=$5 AND benchmark_version=$6 AND benchmark_hash=$7
               AND valuation_at_local_date BETWEEN $8 AND $9
               AND visible_at <= $10
             ORDER BY valuation_at, valuation_at_nanos,
                      visible_at DESC, visible_at_nanos DESC, snapshot_id",
        )
        .bind(query.owner.tenant_id().as_str())
        .bind(query.owner.owner_id().as_str())
        .bind(query.subject_ref.id().as_str())
        .bind(checked_i64(query.subject_ref.version().get())?)
        .bind(query.benchmark.object_id().as_str())
        .bind(checked_i64(
            query.benchmark.version().ok_or_else(validation)?.get(),
        )?)
        .bind(encode_hash(
            query.benchmark.content_hash().ok_or_else(validation)?,
        ))
        .bind(query.period_from.local_trading_date())
        .bind(query.period_to.local_trading_date())
        .bind(query.knowledge_at.instant())
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;

        let mut selected = BTreeSet::new();
        let mut result = Vec::new();
        for row in rows {
            let snapshot = decode_benchmark_level(&row)?;
            if snapshot.visible_at().instant() > query.knowledge_at.instant()
                || snapshot.benchmark() != &query.benchmark
            {
                continue;
            }
            if selected.insert(snapshot.valuation_at().local_trading_date()) {
                result.push(snapshot);
            }
        }
        result.sort_by(|left, right| {
            left.valuation_at()
                .instant()
                .cmp(&right.valuation_at().instant())
        });
        Ok(result)
    }

    async fn read_performance_convention_exact(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        reference: &VersionRef,
        content_hash: &ContentHash,
        knowledge_at: &MarketTime,
    ) -> ApplicationResult<Option<VisiblePortfolioPerformanceConvention>> {
        authorize(scope, owner)?;
        let rows = sqlx::query(
            "SELECT * FROM portfolio.performance_conventions
             WHERE tenant_id=$1 AND owner_id=$2 AND convention_id=$3 AND version=$4
               AND content_hash=$5 AND visible_at <= $6
             ORDER BY visible_at DESC, visible_at_nanos DESC",
        )
        .bind(owner.tenant_id().as_str())
        .bind(owner.owner_id().as_str())
        .bind(reference.id().as_str())
        .bind(checked_i64(reference.version().get())?)
        .bind(encode_hash(content_hash))
        .bind(knowledge_at.instant())
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let mut values = rows
            .iter()
            .map(decode_convention)
            .filter(|value| {
                value.as_ref().map_or(true, |record| {
                    record.visible_at().instant() <= knowledge_at.instant()
                })
            })
            .collect::<ApplicationResult<Vec<_>>>()?;
        if values.len() > 1 && values[0].visible_at() == values[1].visible_at() {
            return Err(integrity());
        }
        Ok(values.drain(..).next())
    }
}

fn validate_query(query: &PortfolioPerformanceReadQuery) -> ApplicationResult<()> {
    if query.member_portfolios.is_empty()
        || query.period_from.instant() >= query.period_to.instant()
        || query.knowledge_at.instant() < query.period_to.instant()
        || query
            .member_portfolios
            .iter()
            .any(|value| value.version().is_none() || value.content_hash().is_none())
        || query.benchmark.version().is_none()
        || query.benchmark.content_hash().is_none()
    {
        return Err(validation());
    }
    Ok(())
}

fn decode_convention(row: &PgRow) -> ApplicationResult<VisiblePortfolioPerformanceConvention> {
    expect(
        row,
        "schema_id",
        "ficant.portfolio-performance-convention.v1",
    )?;
    expect(row, "return_method", "DAILY_TIME_WEIGHTED")?;
    expect(row, "flow_timing", "END_OF_DAY")?;
    expect(row, "valuation_frequency", "CALENDAR_SESSION_CLOSE")?;
    expect(row, "rounding", "TIES_TO_EVEN")?;
    let input = PortfolioPerformanceConventionInput {
        convention: version_ref(row, "convention_id", "version")?,
        owner: owner(row)?,
        schema_id: row.try_get("schema_id").map_err(map_sqlx_error)?,
        calendar: lineage(row, "calendar")?,
        return_method: PortfolioPerformanceReturnMethod::DailyTimeWeighted,
        flow_timing: PortfolioExternalFlowTiming::EndOfDay,
        valuation_frequency: PortfolioValuationFrequency::CalendarSessionClose,
        rounding: PortfolioDecimalRounding::TiesToEven,
        effective_from: market_time(row, "effective_from")?,
        effective_to: market_time(row, "effective_to")?,
        content_hash: content_hash(row, "content_hash")?,
    };
    Ok(VisiblePortfolioPerformanceConvention::new(
        PortfolioPerformanceConvention::new(input).map_err(map_domain_error)?,
        market_time(row, "visible_at")?,
    ))
}

fn decode_valuation_snapshot(row: &PgRow) -> ApplicationResult<PortfolioValuationSnapshot> {
    let input = PortfolioValuationSnapshotInput {
        snapshot_id: ulid(row, "snapshot_id")?,
        owner: owner(row)?,
        subject_ref: version_ref(row, "subject_id", "subject_version")?,
        portfolio: lineage(row, "portfolio")?,
        position_snapshot: PortfolioSnapshotBinding::new(
            ulid(row, "position_snapshot_id")?,
            content_hash(row, "position_snapshot_hash")?,
            market_time(row, "position_observed_at")?,
            market_time(row, "position_visible_at")?,
        )
        .map_err(map_domain_error)?,
        performance_convention: PortfolioPerformanceConventionRef::new(
            version_ref(row, "convention_id", "convention_version")?,
            content_hash(row, "convention_hash")?,
        ),
        valuation_at: market_time(row, "valuation_at")?,
        visible_at: market_time(row, "visible_at")?,
        currency_unit: UnitRef::new(
            ulid(row, "currency_unit_id")?,
            checked_version(
                row.try_get("currency_unit_version")
                    .map_err(map_sqlx_error)?,
            )?,
        ),
        gross_assets: scaled(row, "gross_assets_scaled")?,
        liabilities: scaled(row, "liabilities_scaled")?,
        net_asset_value: scaled(row, "net_asset_value_scaled")?,
        net_external_flow: scaled(row, "net_external_flow_scaled")?,
        content_hash: content_hash(row, "content_hash")?,
    };
    PortfolioValuationSnapshot::new(input).map_err(map_domain_error)
}

fn decode_benchmark_level(row: &PgRow) -> ApplicationResult<BenchmarkLevelSnapshot> {
    let input = BenchmarkLevelSnapshotInput {
        snapshot_id: ulid(row, "snapshot_id")?,
        owner: owner(row)?,
        subject_ref: version_ref(row, "subject_id", "subject_version")?,
        benchmark: lineage(row, "benchmark")?,
        valuation_at: market_time(row, "valuation_at")?,
        visible_at: market_time(row, "visible_at")?,
        level_unit: UnitRef::new(
            ulid(row, "level_unit_id")?,
            checked_version(row.try_get("level_unit_version").map_err(map_sqlx_error)?)?,
        ),
        level: scaled(row, "level_scaled")?,
        content_hash: content_hash(row, "content_hash")?,
    };
    BenchmarkLevelSnapshot::new(input).map_err(map_domain_error)
}

fn authorize(scope: &AccessScope, owner: &OwnerRef) -> ApplicationResult<()> {
    scope.authorize(owner)
}

fn owner(row: &PgRow) -> ApplicationResult<OwnerRef> {
    Ok(OwnerRef::new(
        ulid(row, "tenant_id")?,
        ulid(row, "owner_id")?,
    ))
}

fn version_ref(row: &PgRow, id: &str, version: &str) -> ApplicationResult<VersionRef> {
    Ok(VersionRef::new(
        ulid(row, id)?,
        checked_version(row.try_get(version).map_err(map_sqlx_error)?)?,
    ))
}

fn lineage(row: &PgRow, prefix: &str) -> ApplicationResult<LineageRef> {
    LineageRef::new(
        ulid(row, &format!("{prefix}_id"))?,
        Some(checked_version(
            row.try_get(format!("{prefix}_version").as_str())
                .map_err(map_sqlx_error)?,
        )?),
        Some(content_hash(row, &format!("{prefix}_hash"))?),
    )
    .map_err(map_domain_error)
}

fn market_time(row: &PgRow, prefix: &str) -> ApplicationResult<MarketTime> {
    let second: DateTime<Utc> = row.try_get(prefix).map_err(map_sqlx_error)?;
    let nanos: i32 = row
        .try_get(format!("{prefix}_nanos").as_str())
        .map_err(map_sqlx_error)?;
    let timezone: String = row
        .try_get(format!("{prefix}_timezone").as_str())
        .map_err(map_sqlx_error)?;
    let local_date: NaiveDate = row
        .try_get(format!("{prefix}_local_date").as_str())
        .map_err(map_sqlx_error)?;
    let instant = DateTime::from_timestamp(
        second.timestamp(),
        u32::try_from(nanos).map_err(|_| integrity())?,
    )
    .ok_or_else(integrity)?;
    MarketTime::new(instant, timezone, local_date).map_err(map_domain_error)
}

fn scaled(row: &PgRow, column: &str) -> ApplicationResult<FixedDecimal> {
    let value: String = row.try_get(column).map_err(map_sqlx_error)?;
    Ok(FixedDecimal::from_scaled(
        value.parse::<i128>().map_err(|_| integrity())?,
    ))
}

fn content_hash(row: &PgRow, column: &str) -> ApplicationResult<ContentHash> {
    let value: String = row.try_get(column).map_err(map_sqlx_error)?;
    let bytes = decode_hex(&value).ok_or_else(integrity)?;
    ContentHash::from_bytes(&bytes).map_err(map_domain_error)
}

fn ulid(row: &PgRow, column: &str) -> ApplicationResult<Ulid> {
    let value: String = row.try_get(column).map_err(map_sqlx_error)?;
    Ulid::new(value).map_err(map_domain_error)
}

fn expect(row: &PgRow, column: &str, expected: &str) -> ApplicationResult<()> {
    let actual: String = row.try_get(column).map_err(map_sqlx_error)?;
    (actual == expected).then_some(()).ok_or_else(integrity)
}

fn checked_version(value: i64) -> ApplicationResult<Version> {
    Version::new(u64::try_from(value).map_err(|_| integrity())?).map_err(map_domain_error)
}

fn checked_i64(value: u64) -> ApplicationResult<i64> {
    i64::try_from(value).map_err(|_| validation())
}

fn encode_hash(value: &ContentHash) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut encoded, byte| {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
            encoded
        })
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn integrity() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
}
