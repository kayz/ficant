use async_trait::async_trait;
use ficant_application::ports::{AccessScope, DataHealthThresholdProfileRepository, SnapshotValue};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::primitives::{MarketTime, OwnerRef, VersionRef};
use ficant_domain::research::DataHealthThresholdProfile;
use sqlx::types::chrono::{DateTime, Utc};

use super::PostgresRepository;
use super::codec::decode_snapshot;
use super::common::{application_error, map_sqlx_error};

#[async_trait]
impl DataHealthThresholdProfileRepository for PostgresRepository {
    async fn get_exact(
        &self,
        scope: &AccessScope,
        profile_ref: VersionRef,
        knowledge_at: MarketTime,
    ) -> Result<Option<DataHealthThresholdProfile>, ApplicationError> {
        let payload: Option<(Vec<u8>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT payload, visible_at FROM research.data_health_threshold_profiles
             WHERE tenant_id = $1 AND profile_id = $2 AND profile_version = $3
               AND owner_id::text = ANY($4::text[]) AND visible_at <= $5",
        )
        .bind(scope.tenant_id().as_str())
        .bind(profile_ref.id().as_str())
        .bind(i64::try_from(profile_ref.version().get()).map_err(|_| invalid())?)
        .bind(owner_strings(scope))
        .bind(knowledge_at.instant())
        .fetch_optional(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        payload
            .map(|(payload, visible_at)| {
                decode_profile(
                    &payload,
                    visible_at,
                    Some(&profile_ref),
                    None,
                    &knowledge_at,
                )
            })
            .transpose()
    }

    async fn resolve_active(
        &self,
        scope: &AccessScope,
        owner: OwnerRef,
        evaluated_at: MarketTime,
    ) -> Result<Option<DataHealthThresholdProfile>, ApplicationError> {
        scope.authorize(&owner)?;
        let rows: Vec<(Vec<u8>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT payload, visible_at FROM research.data_health_threshold_profiles
             WHERE tenant_id = $1 AND owner_id = $2
               AND visible_at <= $3 AND effective_from <= $3 AND $3 < effective_to
             ORDER BY profile_id, profile_version",
        )
        .bind(scope.tenant_id().as_str())
        .bind(owner.owner_id().as_str())
        .bind(evaluated_at.instant())
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        match rows.as_slice() {
            [] => Ok(None),
            [(payload, visible_at)] => {
                decode_profile(payload, *visible_at, None, Some(&owner), &evaluated_at).map(Some)
            }
            _ => Err(application_error(
                ApplicationErrorCategory::ImmutableViolation,
                false,
            )),
        }
    }
}

fn decode_profile(
    payload: &[u8],
    stored_visible_at: DateTime<Utc>,
    expected_ref: Option<&VersionRef>,
    expected_owner: Option<&OwnerRef>,
    knowledge_at: &MarketTime,
) -> Result<DataHealthThresholdProfile, ApplicationError> {
    let SnapshotValue::DataHealthThresholdProfile(profile) = decode_snapshot(payload)? else {
        return Err(invalid());
    };
    if expected_ref.is_some_and(|reference| profile.profile_ref() != reference)
        || expected_owner.is_some_and(|owner| profile.owner() != owner)
        || !database_time_matches(profile.visible_at().instant(), stored_visible_at)
        || profile.visible_at().instant() > knowledge_at.instant()
    {
        return Err(invalid());
    }
    Ok(profile)
}

fn database_time_matches(decoded: DateTime<Utc>, stored: DateTime<Utc>) -> bool {
    decoded.timestamp() == stored.timestamp()
        && decoded.timestamp_subsec_micros() == stored.timestamp_subsec_micros()
}

fn owner_strings(scope: &AccessScope) -> Vec<String> {
    scope
        .allowed_owner_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
