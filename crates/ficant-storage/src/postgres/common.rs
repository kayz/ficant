use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::primitives::{ContentHash, LineageRef};
use sqlx::{Postgres, Transaction};

pub(crate) type StorageResult<T> = Result<T, ApplicationError>;

// `Result::map_err` consumes its error, so this mapper intentionally matches that signature.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn map_sqlx_error(error: sqlx::Error) -> ApplicationError {
    if let Some(database) = error.as_database_error() {
        return match database.code().as_deref() {
            Some("23505") => application_error(ApplicationErrorCategory::AlreadyExists, false),
            Some("23503") => application_error(ApplicationErrorCategory::LineageIncomplete, false),
            Some("23514" | "23502" | "22P02" | "22003") => {
                application_error(ApplicationErrorCategory::ValidationFailed, false)
            }
            Some("40001" | "40P01") => {
                application_error(ApplicationErrorCategory::ConcurrencyConflict, true)
            }
            _ => application_error(ApplicationErrorCategory::StorageUnavailable, true),
        };
    }
    application_error(ApplicationErrorCategory::StorageUnavailable, true)
}

pub(crate) fn application_error(
    category: ApplicationErrorCategory,
    retryable: bool,
) -> ApplicationError {
    ApplicationError::new(category, retryable)
}

pub(crate) async fn lock_idempotency(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    scope: &str,
    idempotency_key: &str,
    fingerprint: &[u8],
    result_id: &str,
) -> StorageResult<IdempotencyOutcome> {
    let inserted = sqlx::query(
        "INSERT INTO core.idempotency_records
         (tenant_id, scope, idempotency_key, fingerprint, result_id)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (tenant_id, scope, idempotency_key) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(scope)
    .bind(idempotency_key)
    .bind(fingerprint)
    .bind(result_id)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;

    if inserted.rows_affected() == 1 {
        return Ok(IdempotencyOutcome::Fresh);
    }

    let existing: Option<(Vec<u8>, String)> = sqlx::query_as(
        "SELECT fingerprint, result_id::text
         FROM core.idempotency_records
         WHERE tenant_id = $1 AND scope = $2 AND idempotency_key = $3
         FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(scope)
    .bind(idempotency_key)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;

    match existing {
        Some((stored_fingerprint, stored_result_id))
            if stored_fingerprint == fingerprint && stored_result_id == result_id =>
        {
            Ok(IdempotencyOutcome::Replay)
        }
        _ => Err(application_error(
            ApplicationErrorCategory::AlreadyExists,
            false,
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdempotencyOutcome {
    Fresh,
    Replay,
}

pub(crate) async fn publish_blob_reference(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    content_hash: &ContentHash,
    blob_size: u64,
) -> StorageResult<()> {
    let size = i64::try_from(blob_size)
        .map_err(|_| application_error(ApplicationErrorCategory::ValidationFailed, false))?;
    let hash = crate::minio::content_addressed::hash_hex(content_hash);
    let object_key = format!("immutable/{hash}");
    let candidate: Option<(String, i64)> = sqlx::query_as(
        "SELECT object_key, blob_size
         FROM storage.orphan_candidates
         WHERE content_hash = $1
         FOR UPDATE",
    )
    .bind(&hash)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    let durable_reference: Option<(String, i64)> = if candidate.is_none() {
        sqlx::query_as(
            "SELECT object_key, blob_size
             FROM storage.blobs
             WHERE content_hash = $1
             ORDER BY tenant_id
             LIMIT 1
             FOR SHARE",
        )
        .bind(&hash)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?
    } else {
        None
    };
    match candidate.or(durable_reference) {
        None => {
            return Err(application_error(ApplicationErrorCategory::NotFound, false));
        }
        Some(value) if value != (object_key.clone(), size) => {
            return Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            ));
        }
        Some(_) => {}
    }
    sqlx::query(
        "INSERT INTO storage.blobs
         (tenant_id, content_hash, object_key, blob_size)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (tenant_id, content_hash) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(&hash)
    .bind(&object_key)
    .bind(size)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;

    let persisted: Option<(String, i64)> = sqlx::query_as(
        "SELECT object_key, blob_size
         FROM storage.blobs
         WHERE tenant_id = $1 AND content_hash = $2",
    )
    .bind(tenant_id)
    .bind(&hash)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if persisted != Some((object_key, size)) {
        return Err(application_error(
            ApplicationErrorCategory::ImmutableViolation,
            false,
        ));
    }

    sqlx::query("DELETE FROM storage.orphan_candidates WHERE content_hash = $1")
        .bind(hash)
        .execute(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
    Ok(())
}

pub(crate) async fn insert_lineage(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    source_object_id: &str,
    lineage: &[LineageRef],
) -> StorageResult<()> {
    for (ordinal, reference) in lineage.iter().enumerate() {
        let ordinal = i32::try_from(ordinal)
            .map_err(|_| application_error(ApplicationErrorCategory::ValidationFailed, false))?;
        let version = reference
            .version()
            .map(|value| i64::try_from(value.get()))
            .transpose()
            .map_err(|_| application_error(ApplicationErrorCategory::ValidationFailed, false))?;
        let content_hash = reference
            .content_hash()
            .map(crate::minio::content_addressed::hash_hex);
        let target_exists: bool = sqlx::query_scalar(
            "WITH candidates(target_version, target_content_hash) AS (
                 SELECT version, NULL::text FROM market.units
                  WHERE tenant_id = $1 AND unit_id = $2
                 UNION ALL
                 SELECT version, NULL::text FROM market.calendars
                  WHERE tenant_id = $1 AND calendar_id = $2
                 UNION ALL
                 SELECT version, content_hash::text FROM market.market_rule_packs
                  WHERE tenant_id = $1 AND rule_pack_id = $2
                 UNION ALL
                 SELECT version, NULL::text FROM market.instruments
                  WHERE tenant_id = $1 AND instrument_id = $2
                 UNION ALL
                 SELECT source_revision, NULL::text FROM market.cashflows
                  WHERE tenant_id = $1 AND cashflow_id = $2
                 UNION ALL
                 SELECT source_revision, NULL::text FROM market.quotes
                  WHERE tenant_id = $1 AND quote_id = $2
                 UNION ALL
                 SELECT source_revision, NULL::text FROM market.trades
                  WHERE tenant_id = $1 AND trade_id = $2
                 UNION ALL
                 SELECT source_revision, NULL::text FROM market.valuations
                  WHERE tenant_id = $1 AND valuation_id = $2
                 UNION ALL
                 SELECT NULL::bigint, content_hash::text FROM market.curve_snapshots
                  WHERE tenant_id = $1 AND curve_snapshot_id = $2
                 UNION ALL
                 SELECT NULL::bigint, content_hash::text FROM research.data_snapshots
                  WHERE tenant_id = $1 AND data_snapshot_id = $2
                 UNION ALL
                 SELECT NULL::bigint, content_hash::text FROM research.universe_snapshots
                  WHERE tenant_id = $1 AND universe_snapshot_id = $2
                 UNION ALL
                 SELECT NULL::bigint, content_hash::text FROM research.artifacts
                  WHERE tenant_id = $1 AND artifact_id = $2
                 UNION ALL
                 SELECT NULL::bigint, content_hash::text FROM research.signal_sets
                  WHERE tenant_id = $1 AND signal_set_id = $2
                 UNION ALL
                 SELECT revision, NULL::text FROM research.experiment_run_revisions
                  WHERE tenant_id = $1 AND experiment_run_id = $2
                 UNION ALL
                 SELECT sequence, event_hash::text FROM research.run_journal
                  WHERE tenant_id = $1 AND journal_event_id = $2
             )
             SELECT EXISTS(
                 SELECT 1 FROM candidates
                 WHERE ($3::bigint IS NULL OR target_version = $3)
                   AND ($4::text IS NULL OR target_content_hash = $4)
                   AND ($3::bigint IS NULL OR target_version IS NOT NULL)
                   AND ($4::text IS NULL OR target_content_hash IS NOT NULL)
             )",
        )
        .bind(tenant_id)
        .bind(reference.object_id().as_str())
        .bind(version)
        .bind(content_hash.as_deref())
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
        if !target_exists {
            return Err(application_error(
                ApplicationErrorCategory::LineageIncomplete,
                false,
            ));
        }
        sqlx::query(
            "INSERT INTO research.lineage_edges
             (tenant_id, source_object_id, lineage_ordinal, target_object_id,
              target_version, target_content_hash)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(tenant_id)
        .bind(source_object_id)
        .bind(ordinal)
        .bind(reference.object_id().as_str())
        .bind(version)
        .bind(content_hash)
        .execute(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
    }
    Ok(())
}
