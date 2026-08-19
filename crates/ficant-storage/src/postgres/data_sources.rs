use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, Cursor, CursorPage, DataSourceAuthorizationRepository,
    DataSourceAuthorizationResolution, DataSourceRepository, PageRequest,
    PublishDataSourceAuthorization, RegisterDataSource, data_source_content_hash,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::VersionedDefinition;
use ficant_domain::governance::FoundationChangeOperation;
use ficant_domain::market::{
    DataSource, DataSourceAuthorization, DataSourceAuthorizationInput,
    DataSourceAuthorizationState, DataSourceInput, DataSourceKind, ImportInterface,
    PriceSourceType,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};
use sqlx::{Postgres, Row, Transaction};

use super::PostgresRepository;
use super::common::{IdempotencyOutcome, application_error, lock_idempotency, map_sqlx_error};

#[async_trait]
impl DataSourceRepository for PostgresRepository {
    async fn register(&self, command: RegisterDataSource) -> Result<DataSource, ApplicationError> {
        command.scope().authorize(command.value().owner())?;
        let source = command.value();
        let tenant = source.owner().tenant_id().as_str();
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let outcome = lock_idempotency(
            &mut transaction,
            tenant,
            "data-source:register:v1",
            command.idempotency_key().as_str(),
            command.fingerprint().content_hash().as_bytes(),
            source.id().as_str(),
        )
        .await?;

        if outcome == IdempotencyOutcome::Replay {
            return complete_source_replay(
                transaction,
                tenant,
                source,
                command.fingerprint().content_hash(),
            )
            .await;
        }

        let expected_latest = command.expected_latest_version().map_or(0, Version::get);
        let identity: Option<(String, i64)> = sqlx::query_as(
            "SELECT owner_id::text, latest_version
             FROM data.source_identities
             WHERE tenant_id = $1 AND data_source_id = $2
             FOR UPDATE",
        )
        .bind(tenant)
        .bind(source.id().as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;

        match identity {
            None if expected_latest == 0 => {
                sqlx::query(
                    "INSERT INTO data.source_identities
                     (tenant_id, data_source_id, owner_id, latest_version)
                     VALUES ($1, $2, $3, 0)",
                )
                .bind(tenant)
                .bind(source.id().as_str())
                .bind(source.owner().owner_id().as_str())
                .execute(&mut *transaction)
                .await
                .map_err(map_sqlx_error)?;
            }
            Some((owner_id, latest))
                if owner_id == source.owner().owner_id().as_str()
                    && latest == version_i64(expected_latest)? => {}
            _ => {
                return Err(application_error(
                    ApplicationErrorCategory::VersionConflict,
                    true,
                ));
            }
        }

        let before_hash = if expected_latest == 0 {
            None
        } else {
            Some(data_source_content_hash(
                &read_source_in_transaction(
                    &mut transaction,
                    tenant,
                    source.id().as_str(),
                    expected_latest,
                )
                .await?
                .ok_or_else(|| {
                    application_error(ApplicationErrorCategory::VersionConflict, true)
                })?,
            ))
        };
        insert_source(&mut transaction, source).await?;
        let updated = sqlx::query(
            "UPDATE data.source_identities
             SET latest_version = $3
             WHERE tenant_id = $1 AND data_source_id = $2 AND latest_version = $4",
        )
        .bind(tenant)
        .bind(source.id().as_str())
        .bind(version_i64(source.version())?)
        .bind(version_i64(expected_latest)?)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if updated.rows_affected() != 1 {
            return Err(application_error(
                ApplicationErrorCategory::ConcurrencyConflict,
                true,
            ));
        }
        let change = command.change_record(before_hash)?;
        super::governance::insert_change(&mut transaction, tenant, &change).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(source.clone())
    }

    async fn get_exact(
        &self,
        scope: &AccessScope,
        reference: VersionRef,
    ) -> Result<Option<DataSource>, ApplicationError> {
        let row = read_source(
            self,
            scope.tenant_id().as_str(),
            reference.id().as_str(),
            reference.version().get(),
        )
        .await?;
        if let Some(source) = row.as_ref() {
            scope.authorize(source.owner())?;
        }
        Ok(row)
    }
}

#[async_trait]
impl DataSourceAuthorizationRepository for PostgresRepository {
    async fn publish_authorization(
        &self,
        command: PublishDataSourceAuthorization,
    ) -> Result<DataSourceAuthorization, ApplicationError> {
        command.scope().authorize(command.value().owner())?;
        let value = command.value();
        let tenant = value.owner().tenant_id().as_str();
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let outcome = lock_idempotency(
            &mut transaction,
            tenant,
            "data-source-authorization:publish:v1",
            command.idempotency_key().as_str(),
            command.fingerprint().content_hash().as_bytes(),
            value.id().as_str(),
        )
        .await?;
        if outcome == IdempotencyOutcome::Replay {
            return complete_authorization_replay(
                transaction,
                tenant,
                value,
                command.fingerprint().content_hash(),
            )
            .await;
        }

        validate_authorization_source(&mut transaction, tenant, value).await?;

        let expected_latest = command.expected_latest_version().map_or(0, Version::get);
        let identity: Option<(String, i64)> = sqlx::query_as(
            "SELECT owner_id::text, latest_version FROM data.source_authorization_identities
             WHERE tenant_id=$1 AND authorization_id=$2 FOR UPDATE",
        )
        .bind(tenant)
        .bind(value.id().as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        match identity {
            None if expected_latest == 0 => {
                sqlx::query(
                    "INSERT INTO data.source_authorization_identities
                     (tenant_id, authorization_id, owner_id, latest_version) VALUES ($1,$2,$3,0)",
                )
                .bind(tenant)
                .bind(value.id().as_str())
                .bind(value.owner().owner_id().as_str())
                .execute(&mut *transaction)
                .await
                .map_err(map_sqlx_error)?;
            }
            Some((owner, latest))
                if owner == value.owner().owner_id().as_str()
                    && latest == version_i64(expected_latest)? => {}
            _ => {
                return Err(application_error(
                    ApplicationErrorCategory::VersionConflict,
                    true,
                ));
            }
        }
        let before_hash = if expected_latest == 0 {
            None
        } else {
            Some(
                read_authorization_in_transaction(
                    &mut transaction,
                    tenant,
                    value.id().as_str(),
                    expected_latest,
                )
                .await?
                .ok_or_else(|| application_error(ApplicationErrorCategory::VersionConflict, true))?
                .content_hash()
                .clone(),
            )
        };
        insert_authorization(&mut transaction, value).await?;
        let updated = sqlx::query(
            "UPDATE data.source_authorization_identities SET latest_version=$3
             WHERE tenant_id=$1 AND authorization_id=$2 AND latest_version=$4",
        )
        .bind(tenant)
        .bind(value.id().as_str())
        .bind(version_i64(value.version())?)
        .bind(version_i64(expected_latest)?)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;
        if updated.rows_affected() != 1 {
            return Err(application_error(
                ApplicationErrorCategory::ConcurrencyConflict,
                true,
            ));
        }
        let change = command.change_record(before_hash)?;
        super::governance::insert_change(&mut transaction, tenant, &change).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(value.clone())
    }

    async fn get_authorization_exact(
        &self,
        scope: &AccessScope,
        reference: VersionRef,
    ) -> Result<Option<DataSourceAuthorization>, ApplicationError> {
        let value = read_authorization(
            self,
            scope.tenant_id().as_str(),
            reference.id().as_str(),
            reference.version().get(),
        )
        .await?;
        if let Some(value) = value.as_ref() {
            scope.authorize(value.owner())?;
        }
        Ok(value)
    }

    async fn resolve_authorization_exact(
        &self,
        scope: &AccessScope,
        reference: VersionRef,
    ) -> Result<Option<DataSourceAuthorizationResolution>, ApplicationError> {
        let value = read_authorization(
            self,
            scope.tenant_id().as_str(),
            reference.id().as_str(),
            reference.version().get(),
        )
        .await?;
        Ok(value.map(|authorization| {
            if scope.authorize(authorization.owner()).is_ok() {
                DataSourceAuthorizationResolution::Authorized(Box::new(authorization))
            } else {
                DataSourceAuthorizationResolution::OwnerMismatch {
                    data_source: authorization.data_source().clone(),
                }
            }
        }))
    }

    async fn list_authorizations_for_source(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        source: &VersionRef,
        import_interface: Option<ImportInterface>,
        page: PageRequest,
    ) -> Result<CursorPage<DataSourceAuthorization>, ApplicationError> {
        scope.authorize(owner)?;
        page.authorize_scope(scope)?;
        let cursor = page
            .cursor()
            .map(|value| parse_authorization_cursor(value.opaque_value()))
            .transpose()?;
        let (has_cursor, cursor_id, cursor_version) = cursor
            .map_or((false, String::new(), 0_i64), |(id, version)| {
                (true, id, version)
            });
        let limit = i64::from(page.limit()) + 1;
        let rows = sqlx::query(
            "SELECT authorization_id::text, version FROM data.source_authorizations
             WHERE tenant_id=$1 AND owner_id=$2 AND data_source_id=$3 AND data_source_version=$4
               AND ($5::text IS NULL OR import_interface=$5)
               AND (NOT $6 OR (authorization_id, version) > ($7, $8))
             ORDER BY authorization_id, version LIMIT $9",
        )
        .bind(scope.tenant_id().as_str())
        .bind(owner.owner_id().as_str())
        .bind(source.id().as_str())
        .bind(version_i64(source.version().get())?)
        .bind(import_interface.map(interface_text))
        .bind(has_cursor)
        .bind(&cursor_id)
        .bind(cursor_version)
        .bind(limit)
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let page_len = usize::try_from(page.limit()).map_err(|_| validation())?;
        let has_more = rows.len() > page_len;
        let page_rows = rows.into_iter().take(page_len).collect::<Vec<_>>();
        let next_cursor = if has_more {
            let row = page_rows.last().ok_or_else(validation)?;
            let id: String = row.try_get("authorization_id").map_err(map_sqlx_error)?;
            let version: i64 = row.try_get("version").map_err(map_sqlx_error)?;
            Some(Cursor::issue(
                self.cursor_codec(),
                scope,
                format!("{id}.{version}"),
            )?)
        } else {
            None
        };
        let mut result = Vec::with_capacity(page_rows.len());
        for row in page_rows {
            let id: String = row.try_get("authorization_id").map_err(map_sqlx_error)?;
            let version: i64 = row.try_get("version").map_err(map_sqlx_error)?;
            result.push(
                read_authorization(
                    self,
                    scope.tenant_id().as_str(),
                    &id,
                    u64::try_from(version).map_err(|_| validation())?,
                )
                .await?
                .ok_or_else(validation)?,
            );
        }
        Ok(CursorPage::new(result, next_cursor))
    }
}

fn parse_authorization_cursor(value: &str) -> Result<(String, i64), ApplicationError> {
    let (id, version) = value.split_once('.').ok_or_else(validation)?;
    if version.contains('.') {
        return Err(validation());
    }
    let id = Ulid::new(id).map_err(map_domain_error)?;
    let version = version
        .parse::<u64>()
        .ok()
        .and_then(|value| Version::new(value).ok())
        .ok_or_else(validation)?;
    Ok((id.as_str().to_owned(), version_i64(version.get())?))
}

async fn complete_source_replay(
    mut transaction: Transaction<'_, Postgres>,
    tenant: &str,
    source: &DataSource,
    request_fingerprint: &ContentHash,
) -> Result<DataSource, ApplicationError> {
    let persisted = read_source_in_transaction(
        &mut transaction,
        tenant,
        source.id().as_str(),
        source.version(),
    )
    .await?;
    if persisted.as_ref() != Some(source) {
        return Err(application_error(
            ApplicationErrorCategory::ImmutableViolation,
            false,
        ));
    }
    super::governance::verify_change_replay(
        &mut transaction,
        tenant,
        FoundationChangeOperation::RegisterDataSource,
        &format!("data-source:{}@{}", source.id(), source.version()),
        request_fingerprint,
    )
    .await?;
    transaction.commit().await.map_err(map_sqlx_error)?;
    Ok(source.clone())
}

async fn validate_authorization_source(
    transaction: &mut Transaction<'_, Postgres>,
    tenant: &str,
    authorization: &DataSourceAuthorization,
) -> Result<(), ApplicationError> {
    let source = read_source_in_transaction(
        transaction,
        tenant,
        authorization.data_source().id().as_str(),
        authorization.data_source().version().get(),
    )
    .await?
    .ok_or_else(|| application_error(ApplicationErrorCategory::ValidationFailed, false))?;
    if source.owner() != authorization.owner()
        || data_source_content_hash(&source) != *authorization.data_source_hash()
        || source.canonical_schema_id() != authorization.canonical_schema_id()
        || source.canonical_schema_hash() != authorization.canonical_schema_hash()
    {
        return Err(application_error(
            ApplicationErrorCategory::ValidationFailed,
            false,
        ));
    }
    Ok(())
}

async fn complete_authorization_replay(
    mut transaction: Transaction<'_, Postgres>,
    tenant: &str,
    authorization: &DataSourceAuthorization,
    request_fingerprint: &ContentHash,
) -> Result<DataSourceAuthorization, ApplicationError> {
    let persisted = read_authorization_in_transaction(
        &mut transaction,
        tenant,
        authorization.id().as_str(),
        authorization.version(),
    )
    .await?;
    if persisted.as_ref() != Some(authorization) {
        return Err(application_error(
            ApplicationErrorCategory::ImmutableViolation,
            false,
        ));
    }
    super::governance::verify_change_replay(
        &mut transaction,
        tenant,
        FoundationChangeOperation::PublishDataSourceAuthorization,
        &format!(
            "data-source-authorization:{}@{}",
            authorization.id(),
            authorization.version()
        ),
        request_fingerprint,
    )
    .await?;
    transaction.commit().await.map_err(map_sqlx_error)?;
    Ok(authorization.clone())
}

async fn insert_authorization(
    transaction: &mut Transaction<'_, Postgres>,
    value: &DataSourceAuthorization,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO data.source_authorizations
         (tenant_id, authorization_id, version, owner_id, data_source_id, data_source_version,
          data_source_hash, import_interface, canonical_schema_id, canonical_schema_hash,
          effective_from, effective_from_timezone, effective_from_local_date,
          effective_to, effective_to_timezone, effective_to_local_date, state,
          supersedes_id, supersedes_version, mapping_id, mapping_hash, content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.id().as_str())
    .bind(version_i64(value.version())?)
    .bind(value.owner().owner_id().as_str())
    .bind(value.data_source().id().as_str())
    .bind(version_i64(value.data_source().version().get())?)
    .bind(crate::s3::content_addressed::hash_hex(
        value.data_source_hash(),
    ))
    .bind(interface_text(value.import_interface()))
    .bind(value.canonical_schema_id())
    .bind(crate::s3::content_addressed::hash_hex(
        value.canonical_schema_hash(),
    ))
    .bind(value.effective().from().instant())
    .bind(value.effective().from().market_timezone())
    .bind(value.effective().from().local_trading_date())
    .bind(value.effective().to().instant())
    .bind(value.effective().to().market_timezone())
    .bind(value.effective().to().local_trading_date())
    .bind(state_text(value.state()))
    .bind(value.supersedes().map(|reference| reference.id().as_str()))
    .bind(
        value
            .supersedes()
            .map(|reference| version_i64(reference.version().get()))
            .transpose()?,
    )
    .bind(value.mapping_id().as_str())
    .bind(crate::s3::content_addressed::hash_hex(value.mapping_hash()))
    .bind(crate::s3::content_addressed::hash_hex(value.content_hash()))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

async fn insert_source(
    transaction: &mut Transaction<'_, Postgres>,
    source: &DataSource,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO data.sources
         (tenant_id, data_source_id, version, owner_id, kind, name, connection_binding,
          dataset, canonical_schema_id, canonical_schema_hash, price_source_type)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(source.owner().tenant_id().as_str())
    .bind(source.id().as_str())
    .bind(version_i64(source.version())?)
    .bind(source.owner().owner_id().as_str())
    .bind(kind_text(source.kind()))
    .bind(source.name())
    .bind(source.connection_binding())
    .bind(source.dataset())
    .bind(source.canonical_schema_id())
    .bind(crate::s3::content_addressed::hash_hex(
        source.canonical_schema_hash(),
    ))
    .bind(source.price_source_type().map(price_source_type_text))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    Ok(())
}

async fn read_source(
    repository: &PostgresRepository,
    tenant: &str,
    source_id: &str,
    version: u64,
) -> Result<Option<DataSource>, ApplicationError> {
    let row: Option<SourceRow> = sqlx::query_as(
        "SELECT owner_id::text, kind, name, connection_binding, dataset,
                canonical_schema_id, decode(canonical_schema_hash::text, 'hex'),
                price_source_type
         FROM data.sources
         WHERE tenant_id = $1 AND data_source_id = $2 AND version = $3",
    )
    .bind(tenant)
    .bind(source_id)
    .bind(version_i64(version)?)
    .fetch_optional(repository.pool())
    .await
    .map_err(map_sqlx_error)?;
    row.map(|row| decode_source(tenant, source_id, version, row))
        .transpose()
}

async fn read_source_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    tenant: &str,
    source_id: &str,
    version: u64,
) -> Result<Option<DataSource>, ApplicationError> {
    let row: Option<SourceRow> = sqlx::query_as(
        "SELECT owner_id::text, kind, name, connection_binding, dataset,
                canonical_schema_id, decode(canonical_schema_hash::text, 'hex'),
                price_source_type
         FROM data.sources
         WHERE tenant_id = $1 AND data_source_id = $2 AND version = $3
         FOR SHARE",
    )
    .bind(tenant)
    .bind(source_id)
    .bind(version_i64(version)?)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    row.map(|row| decode_source(tenant, source_id, version, row))
        .transpose()
}

type SourceRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    Vec<u8>,
    Option<String>,
);

fn decode_source(
    tenant: &str,
    source_id: &str,
    version: u64,
    row: SourceRow,
) -> Result<DataSource, ApplicationError> {
    let (owner_id, kind, name, connection_binding, dataset, schema_id, schema_hash, source_type) =
        row;
    let source = DataSource::new(DataSourceInput {
        data_source_id: Ulid::new(source_id).map_err(map_domain_error)?,
        version: Version::new(version).map_err(map_domain_error)?,
        owner: OwnerRef::new(
            Ulid::new(tenant).map_err(map_domain_error)?,
            Ulid::new(owner_id).map_err(map_domain_error)?,
        ),
        kind: parse_kind(&kind)?,
        name,
        connection_binding,
        dataset,
        canonical_schema_id: schema_id,
        canonical_schema_hash: ContentHash::from_bytes(&schema_hash).map_err(map_domain_error)?,
    })
    .map_err(map_domain_error)?;
    source_type.map_or(Ok(source.clone()), |value| {
        source
            .with_price_source_type(parse_price_source_type(&value)?)
            .map_err(map_domain_error)
    })
}

const fn kind_text(kind: DataSourceKind) -> &'static str {
    match kind {
        DataSourceKind::FileNdjson => "FILE_NDJSON",
        DataSourceKind::Postgres => "POSTGRES",
    }
}

fn parse_kind(value: &str) -> Result<DataSourceKind, ApplicationError> {
    match value {
        "FILE_NDJSON" => Ok(DataSourceKind::FileNdjson),
        "POSTGRES" => Ok(DataSourceKind::Postgres),
        _ => Err(application_error(
            ApplicationErrorCategory::ValidationFailed,
            false,
        )),
    }
}

fn price_source_type_text(value: PriceSourceType) -> &'static str {
    match value {
        PriceSourceType::RealTrade => "REAL_TRADE",
        PriceSourceType::ActiveQuote => "ACTIVE_QUOTE",
        PriceSourceType::ModelValuation => "MODEL_VALUATION",
        PriceSourceType::CurveInterpolation => {
            unreachable!("internal curve interpolation cannot be persisted as a DataSource")
        }
    }
}

fn parse_price_source_type(value: &str) -> Result<PriceSourceType, ApplicationError> {
    match value {
        "REAL_TRADE" => Ok(PriceSourceType::RealTrade),
        "ACTIVE_QUOTE" => Ok(PriceSourceType::ActiveQuote),
        "MODEL_VALUATION" => Ok(PriceSourceType::ModelValuation),
        _ => Err(application_error(
            ApplicationErrorCategory::ValidationFailed,
            false,
        )),
    }
}

async fn read_authorization(
    repository: &PostgresRepository,
    tenant: &str,
    authorization_id: &str,
    version: u64,
) -> Result<Option<DataSourceAuthorization>, ApplicationError> {
    let row = sqlx::query(
        "SELECT owner_id::text, data_source_id::text, data_source_version,
                decode(data_source_hash::text,'hex') AS data_source_hash, import_interface,
                canonical_schema_id, decode(canonical_schema_hash::text,'hex') AS schema_hash,
                effective_from, effective_from_timezone, effective_from_local_date,
                effective_to, effective_to_timezone, effective_to_local_date, state,
                supersedes_id::text, supersedes_version, mapping_id::text,
                decode(mapping_hash::text,'hex') AS mapping_hash,
                decode(content_hash::text,'hex') AS content_hash
         FROM data.source_authorizations
         WHERE tenant_id=$1 AND authorization_id=$2 AND version=$3",
    )
    .bind(tenant)
    .bind(authorization_id)
    .bind(version_i64(version)?)
    .fetch_optional(repository.pool())
    .await
    .map_err(map_sqlx_error)?;
    row.map(|row| decode_authorization(tenant, authorization_id, version, &row))
        .transpose()
}

async fn read_authorization_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    tenant: &str,
    authorization_id: &str,
    version: u64,
) -> Result<Option<DataSourceAuthorization>, ApplicationError> {
    let row = sqlx::query(
        "SELECT owner_id::text, data_source_id::text, data_source_version,
                decode(data_source_hash::text,'hex') AS data_source_hash, import_interface,
                canonical_schema_id, decode(canonical_schema_hash::text,'hex') AS schema_hash,
                effective_from, effective_from_timezone, effective_from_local_date,
                effective_to, effective_to_timezone, effective_to_local_date, state,
                supersedes_id::text, supersedes_version, mapping_id::text,
                decode(mapping_hash::text,'hex') AS mapping_hash,
                decode(content_hash::text,'hex') AS content_hash
         FROM data.source_authorizations
         WHERE tenant_id=$1 AND authorization_id=$2 AND version=$3 FOR SHARE",
    )
    .bind(tenant)
    .bind(authorization_id)
    .bind(version_i64(version)?)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    row.map(|row| decode_authorization(tenant, authorization_id, version, &row))
        .transpose()
}

fn decode_authorization(
    tenant: &str,
    authorization_id: &str,
    version: u64,
    row: &sqlx::postgres::PgRow,
) -> Result<DataSourceAuthorization, ApplicationError> {
    let supersedes_id: Option<String> = row.try_get("supersedes_id").map_err(map_sqlx_error)?;
    let supersedes_version: Option<i64> =
        row.try_get("supersedes_version").map_err(map_sqlx_error)?;
    let input = DataSourceAuthorizationInput {
        authorization_id: Ulid::new(authorization_id).map_err(map_domain_error)?,
        version: Version::new(version).map_err(map_domain_error)?,
        owner: OwnerRef::new(
            Ulid::new(tenant).map_err(map_domain_error)?,
            Ulid::new(
                row.try_get::<String, _>("owner_id")
                    .map_err(map_sqlx_error)?,
            )
            .map_err(map_domain_error)?,
        ),
        data_source: VersionRef::new(
            Ulid::new(
                row.try_get::<String, _>("data_source_id")
                    .map_err(map_sqlx_error)?,
            )
            .map_err(map_domain_error)?,
            Version::new(
                u64::try_from(
                    row.try_get::<i64, _>("data_source_version")
                        .map_err(map_sqlx_error)?,
                )
                .map_err(|_| validation())?,
            )
            .map_err(map_domain_error)?,
        ),
        data_source_hash: ContentHash::from_bytes(
            &row.try_get::<Vec<u8>, _>("data_source_hash")
                .map_err(map_sqlx_error)?,
        )
        .map_err(map_domain_error)?,
        import_interface: parse_interface(
            &row.try_get::<String, _>("import_interface")
                .map_err(map_sqlx_error)?,
        )?,
        canonical_schema_id: row.try_get("canonical_schema_id").map_err(map_sqlx_error)?,
        canonical_schema_hash: ContentHash::from_bytes(
            &row.try_get::<Vec<u8>, _>("schema_hash")
                .map_err(map_sqlx_error)?,
        )
        .map_err(map_domain_error)?,
        effective: EffectivePeriod::new(
            decode_time(
                row,
                "effective_from",
                "effective_from_timezone",
                "effective_from_local_date",
            )?,
            decode_time(
                row,
                "effective_to",
                "effective_to_timezone",
                "effective_to_local_date",
            )?,
        )
        .map_err(map_domain_error)?,
        state: parse_state(&row.try_get::<String, _>("state").map_err(map_sqlx_error)?)?,
        supersedes: match (supersedes_id, supersedes_version) {
            (Some(id), Some(version)) => Some(VersionRef::new(
                Ulid::new(id).map_err(map_domain_error)?,
                Version::new(u64::try_from(version).map_err(|_| validation())?)
                    .map_err(map_domain_error)?,
            )),
            (None, None) => None,
            _ => return Err(validation()),
        },
        mapping_id: Ulid::new(
            row.try_get::<String, _>("mapping_id")
                .map_err(map_sqlx_error)?,
        )
        .map_err(map_domain_error)?,
        mapping_hash: ContentHash::from_bytes(
            &row.try_get::<Vec<u8>, _>("mapping_hash")
                .map_err(map_sqlx_error)?,
        )
        .map_err(map_domain_error)?,
    };
    let claimed = ContentHash::from_bytes(
        &row.try_get::<Vec<u8>, _>("content_hash")
            .map_err(map_sqlx_error)?,
    )
    .map_err(map_domain_error)?;
    DataSourceAuthorization::from_claimed_hash(input, claimed).map_err(map_domain_error)
}

fn decode_time(
    row: &sqlx::postgres::PgRow,
    instant: &str,
    timezone: &str,
    local_date: &str,
) -> Result<MarketTime, ApplicationError> {
    MarketTime::new(
        row.try_get(instant).map_err(map_sqlx_error)?,
        row.try_get::<String, _>(timezone).map_err(map_sqlx_error)?,
        row.try_get(local_date).map_err(map_sqlx_error)?,
    )
    .map_err(map_domain_error)
}

const fn interface_text(value: ImportInterface) -> &'static str {
    match value {
        ImportInterface::CanonicalQuoteSnapshot => "CANONICAL_QUOTE_SNAPSHOT",
    }
}
fn parse_interface(value: &str) -> Result<ImportInterface, ApplicationError> {
    match value {
        "CANONICAL_QUOTE_SNAPSHOT" => Ok(ImportInterface::CanonicalQuoteSnapshot),
        _ => Err(validation()),
    }
}
const fn state_text(value: DataSourceAuthorizationState) -> &'static str {
    match value {
        DataSourceAuthorizationState::Active => "ACTIVE",
        DataSourceAuthorizationState::Revoked => "REVOKED",
    }
}
fn parse_state(value: &str) -> Result<DataSourceAuthorizationState, ApplicationError> {
    match value {
        "ACTIVE" => Ok(DataSourceAuthorizationState::Active),
        "REVOKED" => Ok(DataSourceAuthorizationState::Revoked),
        _ => Err(validation()),
    }
}

fn validation() -> ApplicationError {
    application_error(ApplicationErrorCategory::ValidationFailed, false)
}

fn version_i64(value: u64) -> Result<i64, ApplicationError> {
    i64::try_from(value)
        .map_err(|_| application_error(ApplicationErrorCategory::ValidationFailed, false))
}
