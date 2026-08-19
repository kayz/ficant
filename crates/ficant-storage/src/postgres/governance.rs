use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use ficant_application::ports::{
    AccessScope, ApplicationResult, Cursor, CursorPage, FoundationChangeFilter,
    FoundationChangeRepository, PageRequest,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::governance::{
    ChangeJustification, FoundationChangeOperation, FoundationChangeRecord,
    FoundationChangeRecordInput, FoundationResourceKind, FoundationResourceRef, PlatformRole,
    SourceDocumentRef,
};
use ficant_domain::primitives::{ContentHash, MarketTime, Ulid, Version, VersionRef};
use sqlx::{Postgres, Row, Transaction};

use super::PostgresRepository;
use super::common::{application_error, map_sqlx_error};

pub(crate) async fn insert_change(
    transaction: &mut Transaction<'_, Postgres>,
    tenant: &str,
    record: &FoundationChangeRecord,
) -> ApplicationResult<()> {
    sqlx::query(
        "INSERT INTO core.foundation_change_records
         (tenant_id, record_id, actor_id, owner_id, active_role, operation, resource_kind,
          resource_id, resource_version, resource_ref, before_hash, after_hash, reason,
          request_fingerprint, occurred_at, occurred_timezone, occurred_local_date,
          authorization_id, authorization_version)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)",
    )
    .bind(tenant)
    .bind(record.record_id().as_str())
    .bind(record.actor_id().as_str())
    .bind(record.owner().owner_id().as_str())
    .bind(role_text(record.active_role()))
    .bind(record.operation().as_str())
    .bind(record.resource().kind().as_str())
    .bind(record.resource().id().as_str())
    .bind(
        record
            .resource()
            .version()
            .map(|value| version_i64(value.get()))
            .transpose()?,
    )
    .bind(record.resource().canonical_ref())
    .bind(
        record
            .before_hash()
            .map(crate::s3::content_addressed::hash_hex),
    )
    .bind(crate::s3::content_addressed::hash_hex(record.after_hash()))
    .bind(record.change().reason())
    .bind(crate::s3::content_addressed::hash_hex(
        record.request_fingerprint(),
    ))
    .bind(record.occurred_at().instant())
    .bind(record.occurred_at().market_timezone())
    .bind(record.occurred_at().local_trading_date())
    .bind(record.authorization_ref().map(|value| value.id().as_str()))
    .bind(
        record
            .authorization_ref()
            .map(|value| version_i64(value.version().get()))
            .transpose()?,
    )
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    for (ordinal, source) in record.change().sources().iter().enumerate() {
        sqlx::query(
            "INSERT INTO core.foundation_change_sources
             (tenant_id, record_id, source_ordinal, uri, sha256) VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(tenant)
        .bind(record.record_id().as_str())
        .bind(i32::try_from(ordinal).map_err(|_| validation())?)
        .bind(source.uri())
        .bind(crate::s3::content_addressed::hash_hex(source.sha256()))
        .execute(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
    }
    Ok(())
}

pub(crate) async fn verify_change_replay(
    transaction: &mut Transaction<'_, Postgres>,
    tenant: &str,
    operation: FoundationChangeOperation,
    resource_ref: &str,
    fingerprint: &ContentHash,
) -> ApplicationResult<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM core.foundation_change_records
           WHERE tenant_id=$1 AND operation=$2 AND resource_ref=$3 AND request_fingerprint=$4
         )",
    )
    .bind(tenant)
    .bind(operation.as_str())
    .bind(resource_ref)
    .bind(crate::s3::content_addressed::hash_hex(fingerprint))
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if !exists {
        return Err(application_error(
            ApplicationErrorCategory::ImmutableViolation,
            false,
        ));
    }
    Ok(())
}

#[async_trait]
impl FoundationChangeRepository for PostgresRepository {
    async fn get_change(
        &self,
        scope: &AccessScope,
        record_id: &Ulid,
    ) -> ApplicationResult<Option<FoundationChangeRecord>> {
        let row = sqlx::query(
            "SELECT * FROM core.foundation_change_records WHERE tenant_id=$1 AND record_id=$2",
        )
        .bind(scope.tenant_id().as_str())
        .bind(record_id.as_str())
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        match row {
            Some(row) => {
                let change = decode_change(self, scope.tenant_id().as_str(), row).await?;
                scope.authorize(change.owner())?;
                Ok(Some(change))
            }
            None => Ok(None),
        }
    }

    async fn list_changes(
        &self,
        scope: &AccessScope,
        filter: &FoundationChangeFilter,
        page: PageRequest,
    ) -> ApplicationResult<CursorPage<FoundationChangeRecord>> {
        page.authorize_scope(scope)?;
        let cursor = page
            .cursor()
            .map(|value| parse_change_cursor(value.opaque_value()))
            .transpose()?;
        let (has_cursor, cursor_time, cursor_id) = cursor.map_or(
            (false, DateTime::<Utc>::UNIX_EPOCH, String::new()),
            |(time, id)| (true, time, id),
        );
        let limit = i64::from(page.limit()) + 1;
        let rows = sqlx::query(
            "SELECT * FROM core.foundation_change_records
             WHERE tenant_id=$1
               AND ($2::text IS NULL OR resource_ref=$2)
               AND ($3::text IS NULL OR actor_id=$3)
               AND ($4::timestamptz IS NULL OR occurred_at >= $4)
               AND ($5::timestamptz IS NULL OR occurred_at < $5)
               AND owner_id::text = ANY($6::text[])
               AND (NOT $7 OR (occurred_at, record_id) > ($8, $9))
             ORDER BY occurred_at, record_id LIMIT $10",
        )
        .bind(scope.tenant_id().as_str())
        .bind(filter.resource_ref())
        .bind(filter.actor_id().map(Ulid::as_str))
        .bind(filter.occurred_from().map(MarketTime::instant))
        .bind(filter.occurred_to().map(MarketTime::instant))
        .bind(
            scope
                .allowed_owner_ids()
                .iter()
                .map(|value| value.as_str().to_owned())
                .collect::<Vec<_>>(),
        )
        .bind(has_cursor)
        .bind(cursor_time)
        .bind(&cursor_id)
        .bind(limit)
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let page_len = usize::try_from(page.limit()).map_err(|_| validation())?;
        let has_more = rows.len() > page_len;
        let page_rows = rows.into_iter().take(page_len).collect::<Vec<_>>();
        let next_cursor = if has_more {
            let row = page_rows.last().ok_or_else(validation)?;
            let occurred_at: DateTime<Utc> = row.try_get("occurred_at").map_err(map_sqlx_error)?;
            let record_id: String = row.try_get("record_id").map_err(map_sqlx_error)?;
            Some(Cursor::issue(
                self.cursor_codec(),
                scope,
                format!(
                    "{}.{}.{}",
                    occurred_at.timestamp(),
                    occurred_at.timestamp_subsec_nanos(),
                    record_id
                ),
            )?)
        } else {
            None
        };
        let mut result = Vec::with_capacity(page_rows.len());
        for row in page_rows {
            result.push(decode_change(self, scope.tenant_id().as_str(), row).await?);
        }
        Ok(CursorPage::new(result, next_cursor))
    }
}

fn parse_change_cursor(value: &str) -> ApplicationResult<(DateTime<Utc>, String)> {
    let mut fields = value.split('.');
    let seconds = fields
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(validation)?;
    let nanos = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value < 1_000_000_000)
        .ok_or_else(validation)?;
    let record_id = fields
        .next()
        .and_then(|value| Ulid::new(value).ok())
        .ok_or_else(validation)?;
    if fields.next().is_some() {
        return Err(validation());
    }
    let instant = DateTime::<Utc>::from_timestamp(seconds, nanos).ok_or_else(validation)?;
    Ok((instant, record_id.as_str().to_owned()))
}

async fn decode_change(
    repository: &PostgresRepository,
    tenant: &str,
    row: sqlx::postgres::PgRow,
) -> ApplicationResult<FoundationChangeRecord> {
    let record_id: String = row.try_get("record_id").map_err(map_sqlx_error)?;
    let source_rows = sqlx::query(
        "SELECT uri, decode(sha256::text,'hex') AS sha256
         FROM core.foundation_change_sources
         WHERE tenant_id=$1 AND record_id=$2 ORDER BY source_ordinal",
    )
    .bind(tenant)
    .bind(&record_id)
    .fetch_all(repository.pool())
    .await
    .map_err(map_sqlx_error)?;
    let mut sources = Vec::with_capacity(source_rows.len());
    for source in source_rows {
        sources.push(
            SourceDocumentRef::new(
                source.try_get::<String, _>("uri").map_err(map_sqlx_error)?,
                ContentHash::from_bytes(
                    &source
                        .try_get::<Vec<u8>, _>("sha256")
                        .map_err(map_sqlx_error)?,
                )
                .map_err(map_domain_error)?,
            )
            .map_err(map_domain_error)?,
        );
    }
    let authorization_id: Option<String> =
        row.try_get("authorization_id").map_err(map_sqlx_error)?;
    let authorization_version: Option<i64> = row
        .try_get("authorization_version")
        .map_err(map_sqlx_error)?;
    let change = if authorization_id.is_some() {
        ChangeJustification::for_authorized_import(
            row.try_get::<String, _>("reason").map_err(map_sqlx_error)?,
        )
    } else {
        ChangeJustification::new(
            row.try_get::<String, _>("reason").map_err(map_sqlx_error)?,
            sources,
        )
    }
    .map_err(map_domain_error)?;
    let before_hash: Option<String> = row.try_get("before_hash").map_err(map_sqlx_error)?;
    let after_hash: String = row.try_get("after_hash").map_err(map_sqlx_error)?;
    let request_fingerprint: String = row.try_get("request_fingerprint").map_err(map_sqlx_error)?;
    FoundationChangeRecord::new(FoundationChangeRecordInput {
        record_id: Ulid::new(record_id).map_err(map_domain_error)?,
        actor_id: Ulid::new(
            row.try_get::<String, _>("actor_id")
                .map_err(map_sqlx_error)?,
        )
        .map_err(map_domain_error)?,
        owner: ficant_domain::primitives::OwnerRef::new(
            Ulid::new(tenant).map_err(map_domain_error)?,
            Ulid::new(
                row.try_get::<String, _>("owner_id")
                    .map_err(map_sqlx_error)?,
            )
            .map_err(map_domain_error)?,
        ),
        active_role: parse_role(
            &row.try_get::<String, _>("active_role")
                .map_err(map_sqlx_error)?,
        )?,
        operation: parse_operation(
            &row.try_get::<String, _>("operation")
                .map_err(map_sqlx_error)?,
        )?,
        resource: decode_resource(&row)?,
        before_hash: decode_optional_hash(before_hash.as_deref())?,
        after_hash: decode_hash(&after_hash)?,
        change,
        request_fingerprint: decode_hash(&request_fingerprint)?,
        occurred_at: decode_market_time(
            row.try_get("occurred_at").map_err(map_sqlx_error)?,
            row.try_get("occurred_timezone").map_err(map_sqlx_error)?,
            row.try_get("occurred_local_date").map_err(map_sqlx_error)?,
        )?,
        authorization_ref: match (authorization_id, authorization_version) {
            (Some(id), Some(version)) => Some(VersionRef::new(
                Ulid::new(id).map_err(map_domain_error)?,
                Version::new(u64::try_from(version).map_err(|_| validation())?)
                    .map_err(map_domain_error)?,
            )),
            (None, None) => None,
            _ => return Err(validation()),
        },
    })
    .map_err(map_domain_error)
}

fn decode_resource(row: &sqlx::postgres::PgRow) -> ApplicationResult<FoundationResourceRef> {
    let kind = parse_resource_kind(
        &row.try_get::<String, _>("resource_kind")
            .map_err(map_sqlx_error)?,
    )?;
    let id = Ulid::new(
        row.try_get::<String, _>("resource_id")
            .map_err(map_sqlx_error)?,
    )
    .map_err(map_domain_error)?;
    let version: Option<i64> = row.try_get("resource_version").map_err(map_sqlx_error)?;
    match version {
        Some(version) => Ok(FoundationResourceRef::versioned(
            kind,
            VersionRef::new(
                id,
                Version::new(u64::try_from(version).map_err(|_| validation())?)
                    .map_err(map_domain_error)?,
            ),
        )),
        None => Ok(FoundationResourceRef::unversioned(kind, id)),
    }
}

fn decode_market_time(
    instant: DateTime<Utc>,
    timezone: String,
    local_date: NaiveDate,
) -> ApplicationResult<MarketTime> {
    MarketTime::new(instant, timezone, local_date).map_err(map_domain_error)
}
fn decode_hash(value: &str) -> ApplicationResult<ContentHash> {
    let bytes = (0..64)
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| validation())?;
    ContentHash::from_bytes(&bytes).map_err(map_domain_error)
}
fn decode_optional_hash(value: Option<&str>) -> ApplicationResult<Option<ContentHash>> {
    value.map(decode_hash).transpose()
}

fn role_text(value: PlatformRole) -> &'static str {
    match value {
        PlatformRole::PlatformAdmin => "PLATFORM_ADMIN",
        PlatformRole::Researcher => "RESEARCHER",
    }
}
fn parse_role(value: &str) -> ApplicationResult<PlatformRole> {
    match value {
        "PLATFORM_ADMIN" => Ok(PlatformRole::PlatformAdmin),
        "RESEARCHER" => Ok(PlatformRole::Researcher),
        _ => Err(validation()),
    }
}
fn parse_operation(value: &str) -> ApplicationResult<FoundationChangeOperation> {
    use FoundationChangeOperation as O;
    match value {
        "data-source.register" => Ok(O::RegisterDataSource),
        "data-source-authorization.publish" => Ok(O::PublishDataSourceAuthorization),
        "market-definition.append" => Ok(O::AppendMarketDefinition),
        "market-fact.append" => Ok(O::AppendMarketFact),
        "market-fact.correct" => Ok(O::CorrectMarketFact),
        "curve-snapshot.publish" => Ok(O::PublishCurveSnapshot),
        "data-snapshot.import-canonical-quotes" => Ok(O::ImportCanonicalQuoteSnapshot),
        "universe-snapshot.publish" => Ok(O::PublishUniverseSnapshot),
        "subject.register" => Ok(O::RegisterSubject),
        "subject-state.publish" => Ok(O::PublishSubjectState),
        "position-snapshot.publish" => Ok(O::PublishPositionSnapshot),
        "data-health-threshold.configure" => Ok(O::ConfigureDataHealthThreshold),
        _ => Err(validation()),
    }
}
fn parse_resource_kind(value: &str) -> ApplicationResult<FoundationResourceKind> {
    use FoundationResourceKind as K;
    match value {
        "data-source" => Ok(K::DataSource),
        "data-source-authorization" => Ok(K::DataSourceAuthorization),
        "market-definition" => Ok(K::MarketDefinition),
        "market-fact" => Ok(K::MarketFact),
        "curve-snapshot" => Ok(K::CurveSnapshot),
        "data-snapshot" => Ok(K::DataSnapshot),
        "universe-snapshot" => Ok(K::UniverseSnapshot),
        "subject" => Ok(K::Subject),
        "subject-state" => Ok(K::SubjectState),
        "position-snapshot" => Ok(K::PositionSnapshot),
        "data-health-threshold-profile" => Ok(K::DataHealthThresholdProfile),
        _ => Err(validation()),
    }
}
fn version_i64(value: u64) -> ApplicationResult<i64> {
    i64::try_from(value).map_err(|_| validation())
}
fn validation() -> ApplicationError {
    application_error(ApplicationErrorCategory::ValidationFailed, false)
}
