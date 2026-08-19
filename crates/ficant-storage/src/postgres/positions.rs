use async_trait::async_trait;
use ficant_application::ports::{AccessScope, PositionSnapshotRepository, SnapshotValue};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::primitives::{MarketTime, Ulid, VersionRef};
use ficant_domain::research::PositionSnapshot;
use sqlx::types::chrono::{DateTime, Utc};

use super::PostgresRepository;
use super::codec::decode_snapshot;
use super::common::map_sqlx_error;

#[async_trait]
impl PositionSnapshotRepository for PostgresRepository {
    async fn get_position_snapshot(
        &self,
        scope: &AccessScope,
        snapshot_id: Ulid,
        knowledge_at: MarketTime,
    ) -> Result<Option<PositionSnapshot>, ApplicationError> {
        let payload: Option<(Vec<u8>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT payload, visible_at FROM research.position_snapshots
             WHERE tenant_id = $1 AND snapshot_id = $2
               AND owner_id::text = ANY($3::text[]) AND visible_at <= $4",
        )
        .bind(scope.tenant_id().as_str())
        .bind(snapshot_id.as_str())
        .bind(owners(scope))
        .bind(knowledge_at.instant())
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        payload
            .map(|(payload, visible_at)| decode_position(&payload, visible_at, &knowledge_at))
            .transpose()
    }

    async fn resolve_position_snapshot(
        &self,
        scope: &AccessScope,
        subject_ref: VersionRef,
        observed_at: MarketTime,
        knowledge_at: MarketTime,
    ) -> Result<Option<PositionSnapshot>, ApplicationError> {
        let rows: Vec<(Vec<u8>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT payload, visible_at FROM research.position_snapshots
             WHERE tenant_id = $1 AND subject_id = $2 AND subject_version = $3
               AND owner_id::text = ANY($4::text[]) AND observed_at = $5 AND visible_at <= $6
             ORDER BY visible_at DESC",
        )
        .bind(scope.tenant_id().as_str())
        .bind(subject_ref.id().as_str())
        .bind(i64::try_from(subject_ref.version().get()).map_err(|_| invalid())?)
        .bind(owners(scope))
        .bind(observed_at.instant())
        .bind(knowledge_at.instant())
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        for (payload, visible_at) in rows {
            let value = decode_position(&payload, visible_at, &knowledge_at)?;
            if value.subject_ref() == &subject_ref && value.observed_at() == &observed_at {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }
}

fn decode_position(
    payload: &[u8],
    stored_visible_at: DateTime<Utc>,
    knowledge_at: &MarketTime,
) -> Result<PositionSnapshot, ApplicationError> {
    match decode_snapshot(payload)? {
        SnapshotValue::Position(value)
            if database_time_matches(value.visible_at().instant(), stored_visible_at)
                && value.visible_at().instant() <= knowledge_at.instant() =>
        {
            Ok(value)
        }
        _ => Err(invalid()),
    }
}

fn database_time_matches(decoded: DateTime<Utc>, stored: DateTime<Utc>) -> bool {
    decoded.timestamp() == stored.timestamp()
        && decoded.timestamp_subsec_micros() == stored.timestamp_subsec_micros()
}

fn owners(scope: &AccessScope) -> Vec<String> {
    scope
        .allowed_owner_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
