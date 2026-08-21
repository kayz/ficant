use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, ExactCatalogRead, MarketFact,
    PortfolioAnalyticsAuthorityCandidate, PortfolioAnalyticsAuthorityQuery,
    PortfolioAnalyticsAuthorityRepository, PortfolioBondRatesAuthorityCandidate,
    PortfolioCatalogRepository, PortfolioCatalogSnapshot, PortfolioCatalogTemporalScope,
    PortfolioImmutableSnapshotAuthority, PortfolioRatesUnitRole, PortfolioScopeAuthority,
    PortfolioScopeSelector, PortfolioUnitAuthorityBinding, PortfolioValuationAuthorityBinding,
    VisibleCatalogRecord, market_fact_content_hash,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::ContentAddressed;
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, CalendarRequirement, FixedDecimal,
};
use ficant_domain::market::Valuation;
use ficant_domain::portfolio::{
    Benchmark, BenchmarkInput, BenchmarkRef, Book, BookInput, Portfolio, PortfolioDecimalRounding,
    PortfolioGroup, PortfolioGroupInput, PortfolioInput, PortfolioMetricConvention,
    PortfolioMetricConventionInput, PortfolioMetricConventionRef, PortfolioMetricWeighting,
    PortfolioSnapshotBinding, PortfolioStatus,
};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};

use super::PostgresRepository;
use super::codec::decode_fact;
use super::common::map_sqlx_error;

#[async_trait]
impl PortfolioAnalyticsAuthorityRepository for PostgresRepository {
    async fn read_candidates(
        &self,
        scope: &AccessScope,
        query: &PortfolioAnalyticsAuthorityQuery,
    ) -> ApplicationResult<Vec<PortfolioAnalyticsAuthorityCandidate>> {
        authorize(scope, &query.owner)?;
        if query.owner.tenant_id() != scope.tenant_id()
            || query.knowledge_at.instant() < query.valuation_at.instant()
            || query.position_snapshot.observed_at().instant() > query.valuation_at.instant()
            || query.position_snapshot.visible_at().instant() > query.knowledge_at.instant()
        {
            return Err(validation());
        }
        let rows = sqlx::query(
            "SELECT * FROM portfolio.analytics_authority_sets
             WHERE tenant_id=$1 AND owner_id=$2 AND subject_id=$3 AND subject_version=$4
               AND position_snapshot_id=$5 AND position_snapshot_hash=$6
               AND visible_at <= $7 AND effective_from <= $8 AND effective_to >= $8
             ORDER BY authority_set_id",
        )
        .bind(query.owner.tenant_id().as_str())
        .bind(query.owner.owner_id().as_str())
        .bind(query.subject_ref.id().as_str())
        .bind(checked_i64(query.subject_ref.version().get())?)
        .bind(query.position_snapshot.snapshot_id().as_str())
        .bind(encode_hash(query.position_snapshot.content_hash()))
        .bind(query.knowledge_at.instant())
        .bind(query.valuation_at.instant())
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let mut candidates = Vec::new();
        for row in rows {
            let effective_from = market_time(&row, "effective_from")?;
            let effective_to = market_time(&row, "effective_to")?;
            let visible_at = market_time(&row, "visible_at")?;
            if effective_from.instant() > query.valuation_at.instant()
                || effective_to.instant() <= query.valuation_at.instant()
                || visible_at.instant() > query.knowledge_at.instant()
            {
                continue;
            }
            let authority_set_id = ulid(&row, "authority_set_id")?;
            let units = read_authority_units(self.pool(), scope, &authority_set_id).await?;
            let bond_rates = read_bond_authorities(self.pool(), scope, &authority_set_id).await?;
            candidates.push(PortfolioAnalyticsAuthorityCandidate {
                authority_set_id,
                owner: owner(&row)?,
                subject_ref: version_ref(&row, "subject_id", "subject_version")?,
                position_snapshot: PortfolioImmutableSnapshotAuthority {
                    id: ulid(&row, "position_snapshot_id")?,
                    content_hash: content_hash(&row, "position_snapshot_hash")?,
                },
                curve_snapshot: PortfolioImmutableSnapshotAuthority {
                    id: ulid(&row, "curve_snapshot_id")?,
                    content_hash: content_hash(&row, "curve_snapshot_hash")?,
                },
                data_snapshot: PortfolioImmutableSnapshotAuthority {
                    id: ulid(&row, "data_snapshot_id")?,
                    content_hash: content_hash(&row, "data_snapshot_hash")?,
                },
                futures_data_snapshot: optional_snapshot_authority(
                    &row,
                    "futures_data_snapshot_id",
                    "futures_data_snapshot_hash",
                )?,
                tax_rule_pack: AnalyticsObjectRef::new(
                    version_ref(&row, "tax_rule_pack_id", "tax_rule_pack_version")?,
                    content_hash(&row, "tax_rule_pack_hash")?,
                ),
                effective_from,
                effective_to,
                visible_at,
                units,
                bond_rates,
                content_hash: content_hash(&row, "content_hash")?,
            });
        }
        Ok(candidates)
    }

    async fn read_valuation_exact(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        binding: &PortfolioValuationAuthorityBinding,
    ) -> ApplicationResult<Option<Valuation>> {
        authorize(scope, owner)?;
        let payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT payload FROM market.valuations
             WHERE tenant_id=$1 AND owner_id=$2 AND valuation_id=$3 AND source_revision=$4",
        )
        .bind(owner.tenant_id().as_str())
        .bind(owner.owner_id().as_str())
        .bind(binding.valuation_id.as_str())
        .bind(checked_i64(binding.source_revision)?)
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let Some(payload) = payload else {
            return Ok(None);
        };
        let MarketFact::Valuation(valuation) = decode_fact(&payload)? else {
            return Err(integrity());
        };
        if valuation.id() != &binding.valuation_id
            || valuation.owner() != owner
            || valuation.source().source_revision() != binding.source_revision
            || market_fact_content_hash(&MarketFact::Valuation(valuation.clone()))
                != binding.content_hash
        {
            return Err(integrity());
        }
        Ok(Some(valuation))
    }
}

#[async_trait]
impl PortfolioCatalogRepository for PostgresRepository {
    async fn find_scope_authorities(
        &self,
        scope: &AccessScope,
        selector: &PortfolioScopeSelector,
        valuation_at: &MarketTime,
        knowledge_at: &MarketTime,
    ) -> ApplicationResult<Vec<PortfolioScopeAuthority>> {
        if knowledge_at.instant() < valuation_at.instant() {
            return Err(validation());
        }
        match selector {
            PortfolioScopeSelector::Book(id) => {
                fetch_scope_authorities(
                    self.pool(),
                    scope,
                    ("books", "book_id"),
                    id,
                    (valuation_at, knowledge_at),
                    decode_book,
                    book_parts,
                )
                .await
            }
            PortfolioScopeSelector::Group(id) => {
                fetch_scope_authorities(
                    self.pool(),
                    scope,
                    ("groups", "group_id"),
                    id,
                    (valuation_at, knowledge_at),
                    decode_group,
                    group_parts,
                )
                .await
            }
            PortfolioScopeSelector::Portfolio(id) => {
                fetch_scope_authorities(
                    self.pool(),
                    scope,
                    ("portfolios", "portfolio_id"),
                    id,
                    (valuation_at, knowledge_at),
                    decode_portfolio,
                    portfolio_parts,
                )
                .await
            }
        }
    }

    async fn read_catalog_snapshot(
        &self,
        scope: &AccessScope,
        temporal: &PortfolioCatalogTemporalScope,
    ) -> ApplicationResult<PortfolioCatalogSnapshot> {
        authorize(scope, temporal.owner())?;
        let books = fetch_records(
            self.pool(),
            "SELECT * FROM portfolio.books
             WHERE tenant_id=$1 AND owner_id=$2 AND subject_id=$3 AND subject_version=$4
               AND visible_at <= $5 AND effective_from <= $6 AND effective_to >= $6
             ORDER BY book_id, version, visible_at",
            temporal,
            decode_book,
        )
        .await?;
        let groups = fetch_records(
            self.pool(),
            "SELECT * FROM portfolio.groups
             WHERE tenant_id=$1 AND owner_id=$2 AND subject_id=$3 AND subject_version=$4
               AND visible_at <= $5 AND effective_from <= $6 AND effective_to >= $6
             ORDER BY group_id, version, visible_at",
            temporal,
            decode_group,
        )
        .await?;
        let portfolios = fetch_records(
            self.pool(),
            "SELECT * FROM portfolio.portfolios
             WHERE tenant_id=$1 AND owner_id=$2 AND subject_id=$3 AND subject_version=$4
               AND visible_at <= $5 AND effective_from <= $6 AND effective_to >= $6
             ORDER BY portfolio_id, version, visible_at",
            temporal,
            decode_portfolio,
        )
        .await?;
        let benchmarks = fetch_records(
            self.pool(),
            "SELECT * FROM portfolio.benchmarks
             WHERE tenant_id=$1 AND owner_id=$2 AND subject_id=$3 AND subject_version=$4
               AND visible_at <= $5 AND effective_from <= $6 AND effective_to >= $6
             ORDER BY benchmark_id, version, visible_at",
            temporal,
            decode_benchmark,
        )
        .await?;
        let convention_rows = sqlx::query(
            "SELECT * FROM portfolio.metric_conventions
             WHERE tenant_id=$1 AND owner_id=$2
               AND visible_at <= $3 AND effective_from <= $4 AND effective_to >= $4
             ORDER BY convention_id, version, visible_at",
        )
        .bind(temporal.owner().tenant_id().as_str())
        .bind(temporal.owner().owner_id().as_str())
        .bind(temporal.knowledge_at().instant())
        .bind(temporal.as_of().instant())
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let mut metric_conventions = Vec::new();
        for row in convention_rows {
            let record = decode_metric_convention(&row)?;
            if eligible(
                record.value().effective_from(),
                record.value().effective_to(),
                record.visible_at(),
                temporal,
            ) {
                metric_conventions.push(record);
            }
        }
        Ok(PortfolioCatalogSnapshot::new(
            books,
            groups,
            portfolios,
            benchmarks,
            metric_conventions,
        ))
    }

    async fn read_book_exact(
        &self,
        scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<Book>>> {
        authorize(scope, read.temporal().owner())?;
        let row = exact_row(self.pool(), "books", "book_id", read, true).await?;
        exact_result(row.as_ref().map(decode_book).transpose()?, read, book_parts)
    }

    async fn read_group_exact(
        &self,
        scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<PortfolioGroup>>> {
        authorize(scope, read.temporal().owner())?;
        let row = exact_row(self.pool(), "groups", "group_id", read, true).await?;
        exact_result(
            row.as_ref().map(decode_group).transpose()?,
            read,
            group_parts,
        )
    }

    async fn read_portfolio_exact(
        &self,
        scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<Portfolio>>> {
        authorize(scope, read.temporal().owner())?;
        let row = exact_row(self.pool(), "portfolios", "portfolio_id", read, true).await?;
        exact_result(
            row.as_ref().map(decode_portfolio).transpose()?,
            read,
            portfolio_parts,
        )
    }

    async fn read_benchmark_exact(
        &self,
        scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<Benchmark>>> {
        authorize(scope, read.temporal().owner())?;
        let row = exact_row(self.pool(), "benchmarks", "benchmark_id", read, true).await?;
        exact_result(
            row.as_ref().map(decode_benchmark).transpose()?,
            read,
            benchmark_parts,
        )
    }

    async fn read_metric_convention_exact(
        &self,
        scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<PortfolioMetricConvention>>> {
        authorize(scope, read.temporal().owner())?;
        let row = exact_row(
            self.pool(),
            "metric_conventions",
            "convention_id",
            read,
            false,
        )
        .await?;
        exact_result(
            row.as_ref().map(decode_metric_convention).transpose()?,
            read,
            convention_parts,
        )
    }

    async fn resolve_currency_unit(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        currency_code: &str,
    ) -> ApplicationResult<Option<UnitRef>> {
        authorize(scope, owner)?;
        if currency_code.is_empty()
            || !currency_code
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return Err(validation());
        }
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT unit_id, version FROM market.units
             WHERE tenant_id=$1 AND owner_id=$2 AND code=$3
             ORDER BY version DESC, unit_id LIMIT 2",
        )
        .bind(owner.tenant_id().as_str())
        .bind(owner.owner_id().as_str())
        .bind(currency_code)
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        if rows.len() > 1 && rows[0].1 == rows[1].1 {
            return Err(state_conflict());
        }
        rows.first()
            .map(|(id, version)| {
                Ok(UnitRef::new(
                    Ulid::new(id.clone()).map_err(map_domain_error)?,
                    checked_version(*version)?,
                ))
            })
            .transpose()
    }
}

async fn read_authority_units(
    pool: &PgPool,
    scope: &AccessScope,
    authority_set_id: &Ulid,
) -> ApplicationResult<Vec<PortfolioUnitAuthorityBinding>> {
    let rows = sqlx::query(
        "SELECT role, unit_id, unit_version, unit_hash
         FROM portfolio.analytics_authority_units
         WHERE tenant_id=$1 AND authority_set_id=$2
         ORDER BY role, unit_id, unit_version",
    )
    .bind(scope.tenant_id().as_str())
    .bind(authority_set_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(map_sqlx_error)?;
    let mut bindings = rows
        .into_iter()
        .map(|row| {
            let role: String = row.try_get("role").map_err(map_sqlx_error)?;
            Ok(PortfolioUnitAuthorityBinding {
                role: parse_unit_role(&role)?,
                reference: UnitRef::new(
                    ulid(&row, "unit_id")?,
                    checked_version(row.try_get("unit_version").map_err(map_sqlx_error)?)?,
                ),
                content_hash: content_hash(&row, "unit_hash")?,
            })
        })
        .collect::<ApplicationResult<Vec<_>>>()?;
    bindings.sort_by_key(|binding| binding.role);
    Ok(bindings)
}

async fn read_bond_authorities(
    pool: &PgPool,
    scope: &AccessScope,
    authority_set_id: &Ulid,
) -> ApplicationResult<Vec<PortfolioBondRatesAuthorityCandidate>> {
    let rows = sqlx::query(
        "SELECT position_id, instrument_id, instrument_version, valuation_id,
                valuation_source_revision, valuation_hash, valuation_value_index,
                remaining_years_value_index, mode,
                input_coefficient::text AS input_coefficient_text, input_scale,
                remaining_years_coefficient::text AS remaining_years_coefficient_text,
                remaining_years_scale,
                settlement_date, calendar_requirement
         FROM portfolio.bond_rates_authorities
         WHERE tenant_id=$1 AND authority_set_id=$2
         ORDER BY position_id, instrument_id, instrument_version",
    )
    .bind(scope.tenant_id().as_str())
    .bind(authority_set_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(map_sqlx_error)?;
    rows.into_iter()
        .map(|row| {
            let revision: i64 = row
                .try_get("valuation_source_revision")
                .map_err(map_sqlx_error)?;
            let value_index: i32 = row
                .try_get("valuation_value_index")
                .map_err(map_sqlx_error)?;
            let remaining_years_value_index: i32 = row
                .try_get("remaining_years_value_index")
                .map_err(map_sqlx_error)?;
            let input_scale: i32 = row.try_get("input_scale").map_err(map_sqlx_error)?;
            let remaining_years_scale: i32 = row
                .try_get("remaining_years_scale")
                .map_err(map_sqlx_error)?;
            let input: String = row
                .try_get("input_coefficient_text")
                .map_err(map_sqlx_error)?;
            let remaining_years: String = row
                .try_get("remaining_years_coefficient_text")
                .map_err(map_sqlx_error)?;
            if input_scale
                != i32::try_from(ficant_domain::analytics::DECIMAL_SCALE)
                    .map_err(|_| validation())?
                || remaining_years_scale != input_scale
            {
                return Err(integrity());
            }
            Ok(PortfolioBondRatesAuthorityCandidate {
                position_id: ulid(&row, "position_id")?,
                instrument_ref: version_ref(&row, "instrument_id", "instrument_version")?,
                valuation: PortfolioValuationAuthorityBinding {
                    valuation_id: ulid(&row, "valuation_id")?,
                    source_revision: u64::try_from(revision).map_err(|_| integrity())?,
                    content_hash: content_hash(&row, "valuation_hash")?,
                    value_index: u32::try_from(value_index).map_err(|_| integrity())?,
                },
                remaining_years_value_index: u32::try_from(remaining_years_value_index)
                    .map_err(|_| integrity())?,
                mode: parse_analytics_mode(
                    &row.try_get::<String, _>("mode").map_err(map_sqlx_error)?,
                )?,
                input_value: FixedDecimal::from_scaled(
                    input.parse::<i128>().map_err(|_| integrity())?,
                ),
                remaining_years: FixedDecimal::from_scaled(
                    remaining_years.parse::<i128>().map_err(|_| integrity())?,
                ),
                settlement_date: row.try_get("settlement_date").map_err(map_sqlx_error)?,
                calendar_requirement: parse_calendar_requirement(
                    &row.try_get::<String, _>("calendar_requirement")
                        .map_err(map_sqlx_error)?,
                )?,
            })
        })
        .collect()
}

fn optional_snapshot_authority(
    row: &PgRow,
    id_column: &str,
    hash_column: &str,
) -> ApplicationResult<Option<PortfolioImmutableSnapshotAuthority>> {
    let id: Option<String> = row.try_get(id_column).map_err(map_sqlx_error)?;
    let hash: Option<String> = row.try_get(hash_column).map_err(map_sqlx_error)?;
    match (id, hash) {
        (None, None) => Ok(None),
        (Some(id), Some(hash)) => Ok(Some(PortfolioImmutableSnapshotAuthority {
            id: Ulid::new(id).map_err(map_domain_error)?,
            content_hash: parse_hash(&hash)?,
        })),
        _ => Err(integrity()),
    }
}

fn parse_unit_role(value: &str) -> ApplicationResult<PortfolioRatesUnitRole> {
    match value {
        "CURRENCY_AMOUNT" => Ok(PortfolioRatesUnitRole::CurrencyAmount),
        "PRICE_PER_100" => Ok(PortfolioRatesUnitRole::PricePer100),
        "RATE" => Ok(PortfolioRatesUnitRole::Rate),
        "YEARS" => Ok(PortfolioRatesUnitRole::Years),
        "YEARS_SQUARED" => Ok(PortfolioRatesUnitRole::YearsSquared),
        "DV01_PER_100" => Ok(PortfolioRatesUnitRole::Dv01Per100),
        "DV01" => Ok(PortfolioRatesUnitRole::Dv01),
        "DIMENSIONLESS" => Ok(PortfolioRatesUnitRole::Dimensionless),
        "CONTRACT_COUNT" => Ok(PortfolioRatesUnitRole::ContractCount),
        _ => Err(integrity()),
    }
}

fn parse_analytics_mode(value: &str) -> ApplicationResult<AnalyticsMode> {
    match value {
        "PRICE_IN" => Ok(AnalyticsMode::PriceIn),
        "YIELD_IN" => Ok(AnalyticsMode::YieldIn),
        _ => Err(integrity()),
    }
}

fn parse_calendar_requirement(value: &str) -> ApplicationResult<CalendarRequirement> {
    match value {
        "EXACT_MARKET" => Ok(CalendarRequirement::ExactMarket),
        "REFERENCE_REPLAY" => Ok(CalendarRequirement::ReferenceReplay),
        _ => Err(integrity()),
    }
}

async fn fetch_scope_authorities<T>(
    pool: &PgPool,
    scope: &AccessScope,
    relation: (&str, &str),
    identity: &Ulid,
    boundary: (&MarketTime, &MarketTime),
    decode: fn(&PgRow) -> ApplicationResult<VisibleCatalogRecord<T>>,
    parts: ExactCatalogPartsFn<T>,
) -> ApplicationResult<Vec<PortfolioScopeAuthority>> {
    let (table, id_column) = relation;
    let (valuation_at, knowledge_at) = boundary;
    let sql = format!(
        "SELECT * FROM portfolio.{table}
         WHERE tenant_id=$1 AND {id_column}=$2 AND owner_id=ANY($3::text[])
           AND visible_at <= $4 AND effective_from <= $5 AND effective_to > $5
         ORDER BY owner_id, subject_id, subject_version, version, visible_at"
    );
    let allowed_owner_ids = scope
        .allowed_owner_ids()
        .iter()
        .map(|owner_id| owner_id.as_str().to_owned())
        .collect::<Vec<_>>();
    let rows = sqlx::query(&sql)
        .bind(scope.tenant_id().as_str())
        .bind(identity.as_str())
        .bind(&allowed_owner_ids)
        .bind(knowledge_at.instant())
        .bind(valuation_at.instant())
        .fetch_all(pool)
        .await
        .map_err(map_sqlx_error)?;
    let mut authorities = Vec::new();
    for row in rows {
        let record = decode(&row)?;
        let (reference, owner, subject_ref, effective_from, effective_to, _) =
            parts(record.value());
        let subject_ref = subject_ref.ok_or_else(integrity)?;
        if reference.id() != identity
            || !scope.allows(owner)
            || effective_from.instant() > valuation_at.instant()
            || effective_to.instant() <= valuation_at.instant()
            || record.visible_at().instant() > knowledge_at.instant()
        {
            return Err(integrity());
        }
        authorities.push(PortfolioScopeAuthority::new(
            owner.clone(),
            subject_ref.clone(),
        ));
    }
    Ok(authorities)
}

async fn fetch_records<T>(
    pool: &PgPool,
    sql: &str,
    temporal: &PortfolioCatalogTemporalScope,
    decode: fn(&PgRow) -> ApplicationResult<VisibleCatalogRecord<T>>,
) -> ApplicationResult<Vec<VisibleCatalogRecord<T>>>
where
    T: EffectiveCatalogValue,
{
    let rows = sqlx::query(sql)
        .bind(temporal.owner().tenant_id().as_str())
        .bind(temporal.owner().owner_id().as_str())
        .bind(temporal.subject_ref().id().as_str())
        .bind(checked_i64(temporal.subject_ref().version().get())?)
        .bind(temporal.knowledge_at().instant())
        .bind(temporal.as_of().instant())
        .fetch_all(pool)
        .await
        .map_err(map_sqlx_error)?;
    let mut result = Vec::new();
    for row in rows {
        let record = decode(&row)?;
        if eligible(
            record.value().effective_from(),
            record.value().effective_to(),
            record.visible_at(),
            temporal,
        ) {
            result.push(record);
        }
    }
    Ok(result)
}

async fn exact_row(
    pool: &PgPool,
    table: &str,
    id_column: &str,
    read: &ExactCatalogRead,
    subject_scoped: bool,
) -> ApplicationResult<Option<PgRow>> {
    let subject = if subject_scoped {
        " AND subject_id=$6 AND subject_version=$7"
    } else {
        ""
    };
    let sql = format!(
        "SELECT * FROM portfolio.{table}
         WHERE tenant_id=$1 AND owner_id=$2 AND {id_column}=$3 AND version=$4
           AND content_hash=$5{subject}"
    );
    let query = sqlx::query(&sql)
        .bind(read.temporal().owner().tenant_id().as_str())
        .bind(read.temporal().owner().owner_id().as_str())
        .bind(read.reference().id().as_str())
        .bind(checked_i64(read.reference().version().get())?)
        .bind(encode_hash(read.content_hash()));
    let query = if subject_scoped {
        query
            .bind(read.temporal().subject_ref().id().as_str())
            .bind(checked_i64(read.temporal().subject_ref().version().get())?)
    } else {
        query
    };
    query.fetch_optional(pool).await.map_err(map_sqlx_error)
}

type ExactCatalogParts<'a> = (
    &'a VersionRef,
    &'a OwnerRef,
    Option<&'a VersionRef>,
    &'a MarketTime,
    &'a MarketTime,
    &'a ContentHash,
);

type ExactCatalogPartsFn<T> = for<'a> fn(&'a T) -> ExactCatalogParts<'a>;

fn exact_result<T>(
    record: Option<VisibleCatalogRecord<T>>,
    read: &ExactCatalogRead,
    parts: ExactCatalogPartsFn<T>,
) -> ApplicationResult<Option<VisibleCatalogRecord<T>>> {
    let Some(record) = record else {
        return Ok(None);
    };
    let (reference, owner, subject, effective_from, effective_to, hash) = parts(record.value());
    if reference != read.reference()
        || owner != read.temporal().owner()
        || subject.is_some_and(|value| value != read.temporal().subject_ref())
        || hash != read.content_hash()
        || !eligible(
            effective_from,
            effective_to,
            record.visible_at(),
            read.temporal(),
        )
    {
        return Err(integrity());
    }
    Ok(Some(record))
}

fn decode_book(row: &PgRow) -> ApplicationResult<VisibleCatalogRecord<Book>> {
    let input = BookInput {
        book: version_ref(row, "book_id", "version")?,
        owner: owner(row)?,
        subject_ref: version_ref(row, "subject_id", "subject_version")?,
        code: row.try_get("code").map_err(map_sqlx_error)?,
        display_name: row.try_get("display_name").map_err(map_sqlx_error)?,
        status: status(row)?,
        effective_from: market_time(row, "effective_from")?,
        effective_to: market_time(row, "effective_to")?,
        content_hash: content_hash(row, "content_hash")?,
    };
    Ok(VisibleCatalogRecord::new(
        Book::new(input).map_err(map_domain_error)?,
        market_time(row, "visible_at")?,
    ))
}

fn decode_group(row: &PgRow) -> ApplicationResult<VisibleCatalogRecord<PortfolioGroup>> {
    let parent_group = optional_lineage(row, "parent_group")?;
    let input = PortfolioGroupInput {
        group: version_ref(row, "group_id", "version")?,
        owner: owner(row)?,
        subject_ref: version_ref(row, "subject_id", "subject_version")?,
        book: lineage(row, "book")?,
        parent_group,
        code: row.try_get("code").map_err(map_sqlx_error)?,
        display_name: row.try_get("display_name").map_err(map_sqlx_error)?,
        status: status(row)?,
        effective_from: market_time(row, "effective_from")?,
        effective_to: market_time(row, "effective_to")?,
        content_hash: content_hash(row, "content_hash")?,
    };
    Ok(VisibleCatalogRecord::new(
        PortfolioGroup::new(input).map_err(map_domain_error)?,
        market_time(row, "visible_at")?,
    ))
}

fn decode_benchmark(row: &PgRow) -> ApplicationResult<VisibleCatalogRecord<Benchmark>> {
    let input = BenchmarkInput {
        benchmark: version_ref(row, "benchmark_id", "version")?,
        owner: owner(row)?,
        subject_ref: version_ref(row, "subject_id", "subject_version")?,
        code: row.try_get("code").map_err(map_sqlx_error)?,
        display_name: row.try_get("display_name").map_err(map_sqlx_error)?,
        position_snapshot: snapshot_binding(row)?,
        effective_from: market_time(row, "effective_from")?,
        effective_to: market_time(row, "effective_to")?,
        content_hash: content_hash(row, "content_hash")?,
    };
    Ok(VisibleCatalogRecord::new(
        Benchmark::new(input).map_err(map_domain_error)?,
        market_time(row, "visible_at")?,
    ))
}

fn decode_metric_convention(
    row: &PgRow,
) -> ApplicationResult<VisibleCatalogRecord<PortfolioMetricConvention>> {
    expect_text(row, "ytm_weighting", "MARKET_VALUE_TIMES_MODIFIED_DURATION")?;
    expect_text(row, "duration_weighting", "MARKET_VALUE")?;
    expect_text(row, "convexity_weighting", "MARKET_VALUE")?;
    expect_text(row, "coupon_weighting", "NOTIONAL")?;
    expect_text(row, "remaining_life_weighting", "NOTIONAL")?;
    expect_text(row, "rounding", "TIES_TO_EVEN")?;
    let freshness: i64 = row
        .try_get("freshness_limit_seconds")
        .map_err(map_sqlx_error)?;
    let input = PortfolioMetricConventionInput {
        convention: version_ref(row, "convention_id", "version")?,
        owner: owner(row)?,
        schema_id: row.try_get("schema_id").map_err(map_sqlx_error)?,
        ytm_weighting: PortfolioMetricWeighting::MarketValueTimesModifiedDuration,
        duration_weighting: PortfolioMetricWeighting::MarketValue,
        convexity_weighting: PortfolioMetricWeighting::MarketValue,
        coupon_weighting: PortfolioMetricWeighting::Notional,
        remaining_life_weighting: PortfolioMetricWeighting::Notional,
        rounding: PortfolioDecimalRounding::TiesToEven,
        freshness_limit_seconds: u64::try_from(freshness).map_err(|_| validation())?,
        effective_from: market_time(row, "effective_from")?,
        effective_to: market_time(row, "effective_to")?,
        content_hash: content_hash(row, "content_hash")?,
    };
    Ok(VisibleCatalogRecord::new(
        PortfolioMetricConvention::new(input).map_err(map_domain_error)?,
        market_time(row, "visible_at")?,
    ))
}

fn decode_portfolio(row: &PgRow) -> ApplicationResult<VisibleCatalogRecord<Portfolio>> {
    let input = PortfolioInput {
        portfolio: version_ref(row, "portfolio_id", "version")?,
        owner: owner(row)?,
        subject_ref: version_ref(row, "subject_id", "subject_version")?,
        book: lineage(row, "book")?,
        group: lineage(row, "group")?,
        code: row.try_get("code").map_err(map_sqlx_error)?,
        display_name: row.try_get("display_name").map_err(map_sqlx_error)?,
        status: status(row)?,
        position_snapshot: snapshot_binding(row)?,
        benchmark: BenchmarkRef::new(
            version_ref(row, "benchmark_id", "benchmark_version")?,
            content_hash(row, "benchmark_hash")?,
        ),
        metric_convention: PortfolioMetricConventionRef::new(
            version_ref(row, "convention_id", "convention_version")?,
            content_hash(row, "convention_hash")?,
        ),
        effective_from: market_time(row, "effective_from")?,
        effective_to: market_time(row, "effective_to")?,
        content_hash: content_hash(row, "content_hash")?,
    };
    Ok(VisibleCatalogRecord::new(
        Portfolio::new(input).map_err(map_domain_error)?,
        market_time(row, "visible_at")?,
    ))
}

fn snapshot_binding(row: &PgRow) -> ApplicationResult<PortfolioSnapshotBinding> {
    PortfolioSnapshotBinding::new(
        ulid(row, "snapshot_id")?,
        content_hash(row, "snapshot_hash")?,
        market_time(row, "snapshot_observed_at")?,
        market_time(row, "snapshot_visible_at")?,
    )
    .map_err(map_domain_error)
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

fn optional_lineage(row: &PgRow, prefix: &str) -> ApplicationResult<Option<LineageRef>> {
    let id: Option<String> = row
        .try_get(format!("{prefix}_id").as_str())
        .map_err(map_sqlx_error)?;
    let version: Option<i64> = row
        .try_get(format!("{prefix}_version").as_str())
        .map_err(map_sqlx_error)?;
    let hash: Option<String> = row
        .try_get(format!("{prefix}_hash").as_str())
        .map_err(map_sqlx_error)?;
    match (id, version, hash) {
        (None, None, None) => Ok(None),
        (Some(id), Some(version), Some(hash)) => Ok(Some(
            LineageRef::new(
                Ulid::new(id).map_err(map_domain_error)?,
                Some(checked_version(version)?),
                Some(parse_hash(&hash)?),
            )
            .map_err(map_domain_error)?,
        )),
        _ => Err(integrity()),
    }
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
    let nanos = u32::try_from(nanos).map_err(|_| integrity())?;
    let instant = DateTime::from_timestamp(second.timestamp(), nanos).ok_or_else(integrity)?;
    MarketTime::new(instant, timezone, local_date).map_err(map_domain_error)
}

fn content_hash(row: &PgRow, column: &str) -> ApplicationResult<ContentHash> {
    let value: String = row.try_get(column).map_err(map_sqlx_error)?;
    parse_hash(&value)
}

fn parse_hash(value: &str) -> ApplicationResult<ContentHash> {
    let bytes = decode_hex(value).ok_or_else(integrity)?;
    ContentHash::from_bytes(&bytes).map_err(map_domain_error)
}

fn ulid(row: &PgRow, column: &str) -> ApplicationResult<Ulid> {
    let value: String = row.try_get(column).map_err(map_sqlx_error)?;
    Ulid::new(value).map_err(map_domain_error)
}

fn status(row: &PgRow) -> ApplicationResult<PortfolioStatus> {
    let value: String = row.try_get("status").map_err(map_sqlx_error)?;
    match value.as_str() {
        "ACTIVE" => Ok(PortfolioStatus::Active),
        "SUSPENDED" => Ok(PortfolioStatus::Suspended),
        "CLOSED" => Ok(PortfolioStatus::Closed),
        _ => Err(integrity()),
    }
}

fn expect_text(row: &PgRow, column: &str, expected: &str) -> ApplicationResult<()> {
    let value: String = row.try_get(column).map_err(map_sqlx_error)?;
    if value != expected {
        return Err(integrity());
    }
    Ok(())
}

fn eligible(
    effective_from: &MarketTime,
    effective_to: &MarketTime,
    visible_at: &MarketTime,
    temporal: &PortfolioCatalogTemporalScope,
) -> bool {
    effective_from.instant() <= temporal.as_of().instant()
        && temporal.as_of().instant() < effective_to.instant()
        && visible_at.instant() <= temporal.knowledge_at().instant()
}

trait EffectiveCatalogValue {
    fn effective_from(&self) -> &MarketTime;
    fn effective_to(&self) -> &MarketTime;
}

macro_rules! effective_catalog_value {
    ($($value:ty),+ $(,)?) => {
        $(impl EffectiveCatalogValue for $value {
            fn effective_from(&self) -> &MarketTime { self.effective_from() }
            fn effective_to(&self) -> &MarketTime { self.effective_to() }
        })+
    };
}

effective_catalog_value!(Book, PortfolioGroup, Portfolio, Benchmark);

fn book_parts(
    value: &Book,
) -> (
    &VersionRef,
    &OwnerRef,
    Option<&VersionRef>,
    &MarketTime,
    &MarketTime,
    &ContentHash,
) {
    (
        value.reference(),
        value.owner(),
        Some(value.subject_ref()),
        value.effective_from(),
        value.effective_to(),
        value.content_hash(),
    )
}

fn group_parts(
    value: &PortfolioGroup,
) -> (
    &VersionRef,
    &OwnerRef,
    Option<&VersionRef>,
    &MarketTime,
    &MarketTime,
    &ContentHash,
) {
    (
        value.reference(),
        value.owner(),
        Some(value.subject_ref()),
        value.effective_from(),
        value.effective_to(),
        value.content_hash(),
    )
}

fn portfolio_parts(
    value: &Portfolio,
) -> (
    &VersionRef,
    &OwnerRef,
    Option<&VersionRef>,
    &MarketTime,
    &MarketTime,
    &ContentHash,
) {
    (
        value.reference(),
        value.owner(),
        Some(value.subject_ref()),
        value.effective_from(),
        value.effective_to(),
        value.content_hash(),
    )
}

fn benchmark_parts(
    value: &Benchmark,
) -> (
    &VersionRef,
    &OwnerRef,
    Option<&VersionRef>,
    &MarketTime,
    &MarketTime,
    &ContentHash,
) {
    (
        value.reference(),
        value.owner(),
        Some(value.subject_ref()),
        value.effective_from(),
        value.effective_to(),
        value.content_hash(),
    )
}

fn convention_parts(
    value: &PortfolioMetricConvention,
) -> (
    &VersionRef,
    &OwnerRef,
    Option<&VersionRef>,
    &MarketTime,
    &MarketTime,
    &ContentHash,
) {
    (
        value.reference(),
        value.owner(),
        None,
        value.effective_from(),
        value.effective_to(),
        value.content_hash(),
    )
}

fn authorize(scope: &AccessScope, owner: &OwnerRef) -> ApplicationResult<()> {
    scope.authorize(owner)
}

fn checked_version(value: i64) -> ApplicationResult<Version> {
    let value = u64::try_from(value).map_err(|_| validation())?;
    Version::new(value).map_err(map_domain_error)
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
    if value.len() != 64 {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = nibble(pair[0])?;
            let low = nibble(pair[1])?;
            Some((high << 4) | low)
        })
        .collect()
}

const fn nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

fn integrity() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::HashMismatch, false)
}

fn state_conflict() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}
