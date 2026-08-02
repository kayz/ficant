use async_trait::async_trait;
use ficant_application::ports::{AccessScope, DataSourceRepository, RegisterDataSource};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_domain_error};
use ficant_domain::VersionedDefinition;
use ficant_domain::market::{DataSource, DataSourceInput, DataSourceKind, PriceSourceType};
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, Version, VersionRef};
use sqlx::{Postgres, Transaction};

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
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(source.clone());
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

fn version_i64(value: u64) -> Result<i64, ApplicationError> {
    i64::try_from(value)
        .map_err(|_| application_error(ApplicationErrorCategory::ValidationFailed, false))
}
