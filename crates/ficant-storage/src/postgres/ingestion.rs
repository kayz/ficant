use ficant_application::ApplicationErrorCategory;
use ficant_domain::primitives::LineageRef;
use sqlx::{Postgres, Transaction};

use super::common::{StorageResult, application_error, map_sqlx_error};

/// Persists snapshot lineage after validating every exact R6A input authority.
pub(crate) async fn insert_snapshot_lineage(
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
            .map(crate::s3::content_addressed::hash_hex);
        let target_exists = snapshot_lineage_target_exists(
            transaction,
            tenant_id,
            reference.object_id().as_str(),
            version,
            content_hash.as_deref(),
        )
        .await?;
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

async fn snapshot_lineage_target_exists(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    target_object_id: &str,
    version: Option<i64>,
    content_hash: Option<&str>,
) -> StorageResult<bool> {
    sqlx::query_scalar(
        "WITH candidates(target_version, target_content_hash) AS (
                 SELECT version, NULL::text FROM data.sources
                  WHERE tenant_id = $1 AND data_source_id = $2
                 UNION ALL
                 SELECT version, content_hash::text FROM data.source_authorizations
                  WHERE tenant_id = $1 AND authorization_id = $2
                 UNION ALL
                 SELECT NULL::bigint, mapping_hash::text FROM data.source_authorizations
                  WHERE tenant_id = $1 AND mapping_id = $2
                 UNION ALL
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
                 SELECT NULL::bigint, content_hash::text FROM research.position_snapshots
                  WHERE tenant_id = $1 AND snapshot_id = $2
                 UNION ALL
                 SELECT NULL::bigint, content_hash::text FROM research.data_health_threshold_profiles
                  WHERE tenant_id = $1 AND profile_snapshot_id = $2
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
    .bind(target_object_id)
    .bind(version)
    .bind(content_hash)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sqlx_error)
}
