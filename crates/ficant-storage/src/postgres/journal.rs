use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, AppendJournalEvent, Cursor, CursorPage, PageRequest, RunJournalRepository,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::ContentAddressed;
use ficant_domain::research::{JournalEventType, RunJournal};

use super::PostgresRepository;
use super::codec::{decode_journal, encode_journal};
use super::common::{IdempotencyOutcome, application_error, lock_idempotency, map_sqlx_error};

#[async_trait]
impl RunJournalRepository for PostgresRepository {
    async fn append(&self, command: AppendJournalEvent) -> Result<RunJournal, ApplicationError> {
        let mut transaction = self.pool().begin().await.map_err(map_sqlx_error)?;
        let event = persist_journal(&mut transaction, &command).await?;
        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(event)
    }

    async fn read(
        &self,
        scope: &AccessScope,
        run_id: ficant_domain::primitives::Ulid,
        page: PageRequest,
    ) -> Result<CursorPage<RunJournal>, ApplicationError> {
        page.authorize_scope(scope)?;
        let after = page
            .cursor()
            .map(|cursor| cursor.opaque_value().parse::<u64>().map_err(|_| invalid()))
            .transpose()?
            .unwrap_or(0);
        let owners = scope
            .allowed_owner_ids()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>();
        let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT j.sequence, j.payload
             FROM research.run_journal j
             JOIN research.experiment_runs r
               ON r.tenant_id = j.tenant_id AND r.experiment_run_id = j.run_id
             WHERE j.tenant_id = $1 AND j.run_id = $2 AND j.sequence > $3
               AND r.owner_id::text = ANY($4::text[])
             ORDER BY j.sequence
             LIMIT $5",
        )
        .bind(scope.tenant_id().as_str())
        .bind(run_id.as_str())
        .bind(version_i64(after)?)
        .bind(&owners)
        .bind(i64::from(page.limit()) + 1)
        .fetch_all(self.pool())
        .await
        .map_err(map_sqlx_error)?;
        let page_len = usize::try_from(page.limit()).map_err(|_| invalid())?;
        let has_more = rows.len() > page_len;
        let rows = rows.into_iter().take(page_len).collect::<Vec<_>>();
        let next_cursor = if has_more {
            let sequence = rows.last().ok_or_else(storage_error)?.0;
            Some(Cursor::issue(
                self.cursor_codec(),
                scope,
                sequence.to_string(),
            )?)
        } else {
            None
        };
        let events = rows
            .into_iter()
            .map(|(_, payload)| decode_journal(&payload))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CursorPage::new(events, next_cursor))
    }
}

// Sequence CAS, hash-chain validation, and insertion remain adjacent to preserve atomic reviewability.
#[allow(clippy::too_many_lines)]
pub(crate) async fn persist_journal(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &AppendJournalEvent,
) -> Result<RunJournal, ApplicationError> {
    command.scope().authorize(command.target_owner())?;
    let tenant = command.target_owner().tenant_id().as_str();
    let owner = command.target_owner().owner_id().as_str();
    let run_id = command.run_id().as_str();
    let event = command.event();
    let outcome = lock_idempotency(
        transaction,
        tenant,
        "run-journal:append:v2",
        command.idempotency_key().as_str(),
        command.fingerprint().content_hash().as_bytes(),
        event.id().as_str(),
    )
    .await?;
    if outcome == IdempotencyOutcome::Replay {
        let payload: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT j.payload FROM research.run_journal j
             JOIN research.experiment_runs r
               ON r.tenant_id = j.tenant_id AND r.experiment_run_id = j.run_id
             WHERE j.tenant_id = $1 AND j.run_id = $2 AND j.sequence = $3 AND r.owner_id = $4",
        )
        .bind(tenant)
        .bind(run_id)
        .bind(version_i64(command.expected_next_sequence())?)
        .bind(owner)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?;
        return payload
            .map(|bytes| decode_journal(&bytes))
            .transpose()?
            .ok_or_else(storage_error);
    }
    let expected_previous = if command.expected_next_sequence() == 1 {
        None
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT j.event_hash::text FROM research.run_journal j
             JOIN research.experiment_runs r
               ON r.tenant_id = j.tenant_id AND r.experiment_run_id = j.run_id
             WHERE j.tenant_id = $1 AND j.run_id = $2 AND j.sequence = $3 AND r.owner_id = $4",
        )
        .bind(tenant)
        .bind(run_id)
        .bind(version_i64(command.expected_next_sequence() - 1)?)
        .bind(owner)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sqlx_error)?
    };
    let actual_previous = event
        .prev_hash()
        .map(crate::minio::content_addressed::hash_hex);
    if expected_previous != actual_previous {
        return Err(application_error(
            ApplicationErrorCategory::LineageIncomplete,
            false,
        ));
    }
    let advanced: Option<i64> = sqlx::query_scalar(
        "UPDATE research.run_journal_sequences s
         SET next_sequence = next_sequence + 1
         WHERE s.tenant_id = $1 AND s.run_id = $2 AND s.next_sequence = $3
           AND EXISTS (
             SELECT 1 FROM research.experiment_runs r
             WHERE r.tenant_id = s.tenant_id AND r.experiment_run_id = s.run_id
               AND r.owner_id = $4
           )
         RETURNING next_sequence",
    )
    .bind(tenant)
    .bind(run_id)
    .bind(version_i64(command.expected_next_sequence())?)
    .bind(owner)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    if advanced.is_none() {
        return Err(application_error(
            ApplicationErrorCategory::ConcurrencyConflict,
            true,
        ));
    }
    sqlx::query(
        "INSERT INTO research.run_journal
         (tenant_id, run_id, sequence, journal_event_id, event_type, occurred_at,
          prev_hash, event_hash, idempotency_key, fingerprint, payload)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(tenant)
    .bind(run_id)
    .bind(version_i64(event.sequence())?)
    .bind(event.id().as_str())
    .bind(event_type(event.event_type()))
    .bind(event.occurred_at().instant())
    .bind(
        event
            .prev_hash()
            .map(crate::minio::content_addressed::hash_hex),
    )
    .bind(crate::minio::content_addressed::hash_hex(
        event.content_hash(),
    ))
    .bind(command.idempotency_key().as_str())
    .bind(command.fingerprint().content_hash().as_bytes().as_slice())
    .bind(encode_journal(event))
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;
    Ok(event.clone())
}

const fn event_type(value: JournalEventType) -> &'static str {
    match value {
        JournalEventType::RunCreated => "RUN_CREATED",
        JournalEventType::RunStarted => "RUN_STARTED",
        JournalEventType::RunSucceeded => "RUN_SUCCEEDED",
        JournalEventType::RunFailed => "RUN_FAILED",
        JournalEventType::RunCancelled => "RUN_CANCELLED",
        JournalEventType::ArtifactPublished => "ARTIFACT_PUBLISHED",
        JournalEventType::SignalSetPublished => "SIGNAL_SET_PUBLISHED",
    }
}

fn version_i64(value: u64) -> Result<i64, ApplicationError> {
    i64::try_from(value).map_err(|_| invalid())
}

fn invalid() -> ApplicationError {
    application_error(ApplicationErrorCategory::ValidationFailed, false)
}

fn storage_error() -> ApplicationError {
    application_error(ApplicationErrorCategory::StorageUnavailable, false)
}
