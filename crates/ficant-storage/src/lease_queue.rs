use std::fmt::Write;

use chrono::{DateTime, Utc};
use ficant_domain::primitives::{ContentHash, Ulid};
use sqlx::{PgPool, Row};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseTaskState {
    Pending,
    Leased,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseTask {
    tenant_id: Ulid,
    task_id: Ulid,
    run_id: Ulid,
    node_id: Ulid,
    node_attempt: u32,
    graph_digest: ContentHash,
    task_key: String,
    state: LeaseTaskState,
    lease_owner: Option<Ulid>,
    lease_id: Option<Ulid>,
    lease_expires_at: Option<DateTime<Utc>>,
    claim_count: u64,
    completion_hash: Option<ContentHash>,
}

impl LeaseTask {
    #[must_use]
    pub fn tenant_id(&self) -> &Ulid {
        &self.tenant_id
    }
    #[must_use]
    pub fn task_id(&self) -> &Ulid {
        &self.task_id
    }
    #[must_use]
    pub fn run_id(&self) -> &Ulid {
        &self.run_id
    }
    #[must_use]
    pub fn node_id(&self) -> &Ulid {
        &self.node_id
    }
    #[must_use]
    pub fn node_attempt(&self) -> u32 {
        self.node_attempt
    }
    #[must_use]
    pub fn graph_digest(&self) -> &ContentHash {
        &self.graph_digest
    }
    #[must_use]
    pub fn task_key(&self) -> &str {
        &self.task_key
    }
    #[must_use]
    pub fn state(&self) -> LeaseTaskState {
        self.state
    }
    #[must_use]
    pub fn lease_owner(&self) -> Option<&Ulid> {
        self.lease_owner.as_ref()
    }
    #[must_use]
    pub fn lease_id(&self) -> Option<&Ulid> {
        self.lease_id.as_ref()
    }
    #[must_use]
    pub fn lease_expires_at(&self) -> Option<DateTime<Utc>> {
        self.lease_expires_at
    }
    #[must_use]
    pub fn claim_count(&self) -> u64 {
        self.claim_count
    }
    #[must_use]
    pub fn completion_hash(&self) -> Option<&ContentHash> {
        self.completion_hash.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnqueueTask {
    pub tenant_id: Ulid,
    pub task_id: Ulid,
    pub run_id: Ulid,
    pub node_id: Ulid,
    pub node_attempt: u32,
    pub graph_digest: ContentHash,
    pub task_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnqueueResult {
    task: LeaseTask,
    inserted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompleteResult {
    task: LeaseTask,
    completed: bool,
}

impl CompleteResult {
    #[must_use]
    pub fn task(&self) -> &LeaseTask {
        &self.task
    }

    #[must_use]
    pub fn completed(&self) -> bool {
        self.completed
    }
}

impl EnqueueResult {
    #[must_use]
    pub fn task(&self) -> &LeaseTask {
        &self.task
    }
    #[must_use]
    pub fn inserted(&self) -> bool {
        self.inserted
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseQueueError {
    InvalidValue,
    Conflict,
    NotFound,
    StorageUnavailable,
}

#[derive(Clone)]
pub struct PostgresLeaseQueue {
    pool: PgPool,
}

impl PostgresLeaseQueue {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts one immutable execution task or returns the identical prior task.
    ///
    /// # Errors
    ///
    /// Returns a stable validation, conflict, lineage, or storage error.
    pub async fn enqueue(&self, input: EnqueueTask) -> Result<EnqueueResult, LeaseQueueError> {
        validate_task_key(&input.task_key)?;
        if input.node_attempt == 0 {
            return Err(LeaseQueueError::InvalidValue);
        }
        let node_attempt = i64::from(input.node_attempt);
        let mut transaction = self.pool.begin().await.map_err(map_error)?;
        let inserted = sqlx::query(
            "INSERT INTO research.execution_tasks
             (tenant_id, task_id, run_id, node_id, node_attempt, graph_digest, task_key, state)
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'PENDING')
             ON CONFLICT (tenant_id, task_key) DO NOTHING",
        )
        .bind(input.tenant_id.as_str())
        .bind(input.task_id.as_str())
        .bind(input.run_id.as_str())
        .bind(input.node_id.as_str())
        .bind(node_attempt)
        .bind(hash_hex(&input.graph_digest))
        .bind(&input.task_key)
        .execute(&mut *transaction)
        .await
        .map_err(map_error)?
        .rows_affected()
            == 1;
        let task = fetch_by_key(&mut transaction, input.tenant_id.as_str(), &input.task_key)
            .await?
            .ok_or(LeaseQueueError::StorageUnavailable)?;
        if task.task_id != input.task_id
            || task.run_id != input.run_id
            || task.node_id != input.node_id
            || task.node_attempt != input.node_attempt
            || task.graph_digest != input.graph_digest
        {
            return Err(LeaseQueueError::Conflict);
        }
        transaction.commit().await.map_err(map_error)?;
        Ok(EnqueueResult { task, inserted })
    }

    /// Atomically claims the oldest pending or expired task within one tenant.
    ///
    /// # Errors
    ///
    /// Returns a stable validation or storage error.
    pub async fn claim(
        &self,
        tenant_id: &Ulid,
        worker_id: &Ulid,
        lease_id: &Ulid,
        lease_seconds: u32,
    ) -> Result<Option<LeaseTask>, LeaseQueueError> {
        validate_duration(lease_seconds)?;
        let seconds = i32::try_from(lease_seconds).map_err(|_| LeaseQueueError::InvalidValue)?;
        let row = sqlx::query(
            "WITH candidate AS (
                 SELECT tenant_id, task_id
                 FROM research.execution_tasks
                 WHERE tenant_id = $1
                   AND (state = 'PENDING'
                        OR (state = 'LEASED' AND lease_expires_at <= CURRENT_TIMESTAMP))
                 ORDER BY created_at, task_id
                 FOR UPDATE SKIP LOCKED
                 LIMIT 1
             )
             UPDATE research.execution_tasks task
             SET state = 'LEASED', lease_owner = $2, lease_id = $3,
                 lease_expires_at = CURRENT_TIMESTAMP + make_interval(secs => $4),
                 claim_count = claim_count + 1, updated_at = CURRENT_TIMESTAMP
             FROM candidate
             WHERE task.tenant_id = candidate.tenant_id AND task.task_id = candidate.task_id
             RETURNING task.*",
        )
        .bind(tenant_id.as_str())
        .bind(worker_id.as_str())
        .bind(lease_id.as_str())
        .bind(seconds)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_error)?;
        row.map(|row| decode_task(&row)).transpose()
    }

    /// Extends an active lease using the database clock.
    ///
    /// # Errors
    ///
    /// Returns `Conflict` for wrong ownership or expiration.
    pub async fn renew(
        &self,
        tenant_id: &Ulid,
        task_id: &Ulid,
        worker_id: &Ulid,
        lease_id: &Ulid,
        lease_seconds: u32,
    ) -> Result<LeaseTask, LeaseQueueError> {
        validate_duration(lease_seconds)?;
        let seconds = i32::try_from(lease_seconds).map_err(|_| LeaseQueueError::InvalidValue)?;
        let row = sqlx::query(
            "UPDATE research.execution_tasks
             SET lease_expires_at = CURRENT_TIMESTAMP + make_interval(secs => $5),
                 updated_at = CURRENT_TIMESTAMP
             WHERE tenant_id = $1 AND task_id = $2 AND state = 'LEASED'
               AND lease_owner = $3 AND lease_id = $4
               AND lease_expires_at > CURRENT_TIMESTAMP
             RETURNING *",
        )
        .bind(tenant_id.as_str())
        .bind(task_id.as_str())
        .bind(worker_id.as_str())
        .bind(lease_id.as_str())
        .bind(seconds)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_error)?;
        row.map(|row| decode_task(&row))
            .transpose()?
            .ok_or(LeaseQueueError::Conflict)
    }

    /// Completes an active lease once; an identical retry returns the stored completion.
    ///
    /// # Errors
    ///
    /// Returns `Conflict` for wrong ownership, expiration, or changed completion evidence.
    pub async fn complete(
        &self,
        tenant_id: &Ulid,
        task_id: &Ulid,
        worker_id: &Ulid,
        lease_id: &Ulid,
        completion_hash: &ContentHash,
    ) -> Result<CompleteResult, LeaseQueueError> {
        let mut transaction = self.pool.begin().await.map_err(map_error)?;
        let existing = fetch_by_id(&mut transaction, tenant_id.as_str(), task_id.as_str())
            .await?
            .ok_or(LeaseQueueError::NotFound)?;
        if existing.state == LeaseTaskState::Completed {
            if existing.lease_owner.as_ref() == Some(worker_id)
                && existing.lease_id.as_ref() == Some(lease_id)
                && existing.completion_hash.as_ref() == Some(completion_hash)
            {
                transaction.commit().await.map_err(map_error)?;
                return Ok(CompleteResult {
                    task: existing,
                    completed: false,
                });
            }
            return Err(LeaseQueueError::Conflict);
        }
        let row = sqlx::query(
            "UPDATE research.execution_tasks
             SET state = 'COMPLETED', completion_hash = $5, updated_at = CURRENT_TIMESTAMP
             WHERE tenant_id = $1 AND task_id = $2 AND state = 'LEASED'
               AND lease_owner = $3 AND lease_id = $4
               AND lease_expires_at > CURRENT_TIMESTAMP
             RETURNING *",
        )
        .bind(tenant_id.as_str())
        .bind(task_id.as_str())
        .bind(worker_id.as_str())
        .bind(lease_id.as_str())
        .bind(hash_hex(completion_hash))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_error)?;
        let task = row
            .map(|row| decode_task(&row))
            .transpose()?
            .ok_or(LeaseQueueError::Conflict)?;
        transaction.commit().await.map_err(map_error)?;
        Ok(CompleteResult {
            task,
            completed: true,
        })
    }
}

async fn fetch_by_key(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    task_key: &str,
) -> Result<Option<LeaseTask>, LeaseQueueError> {
    sqlx::query(
        "SELECT * FROM research.execution_tasks
         WHERE tenant_id = $1 AND task_key = $2 FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(task_key)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_error)?
    .map(|row| decode_task(&row))
    .transpose()
}

async fn fetch_by_id(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    task_id: &str,
) -> Result<Option<LeaseTask>, LeaseQueueError> {
    sqlx::query(
        "SELECT * FROM research.execution_tasks
         WHERE tenant_id = $1 AND task_id = $2 FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(task_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_error)?
    .map(|row| decode_task(&row))
    .transpose()
}

fn decode_task(row: &sqlx::postgres::PgRow) -> Result<LeaseTask, LeaseQueueError> {
    let state: String = row.try_get("state").map_err(map_error)?;
    let claim_count: i64 = row.try_get("claim_count").map_err(map_error)?;
    let node_attempt: i64 = row.try_get("node_attempt").map_err(map_error)?;
    Ok(LeaseTask {
        tenant_id: parse_id(row.try_get("tenant_id").map_err(map_error)?)?,
        task_id: parse_id(row.try_get("task_id").map_err(map_error)?)?,
        run_id: parse_id(row.try_get("run_id").map_err(map_error)?)?,
        node_id: parse_id(row.try_get("node_id").map_err(map_error)?)?,
        node_attempt: u32::try_from(node_attempt)
            .map_err(|_| LeaseQueueError::StorageUnavailable)?,
        graph_digest: parse_hash(
            &row.try_get::<String, _>("graph_digest")
                .map_err(map_error)?,
        )?,
        task_key: row.try_get("task_key").map_err(map_error)?,
        state: match state.as_str() {
            "PENDING" => LeaseTaskState::Pending,
            "LEASED" => LeaseTaskState::Leased,
            "COMPLETED" => LeaseTaskState::Completed,
            _ => return Err(LeaseQueueError::StorageUnavailable),
        },
        lease_owner: parse_optional_id(row.try_get("lease_owner").map_err(map_error)?)?,
        lease_id: parse_optional_id(row.try_get("lease_id").map_err(map_error)?)?,
        lease_expires_at: row.try_get("lease_expires_at").map_err(map_error)?,
        claim_count: u64::try_from(claim_count).map_err(|_| LeaseQueueError::StorageUnavailable)?,
        completion_hash: row
            .try_get::<Option<String>, _>("completion_hash")
            .map_err(map_error)?
            .map(|value| parse_hash(&value))
            .transpose()?,
    })
}

fn validate_task_key(value: &str) -> Result<(), LeaseQueueError> {
    if value.is_empty() || value.len() > 256 || value.trim() != value {
        return Err(LeaseQueueError::InvalidValue);
    }
    Ok(())
}

fn validate_duration(value: u32) -> Result<(), LeaseQueueError> {
    if !(1..=3600).contains(&value) {
        return Err(LeaseQueueError::InvalidValue);
    }
    Ok(())
}

fn parse_id(value: String) -> Result<Ulid, LeaseQueueError> {
    Ulid::new(value).map_err(|_| LeaseQueueError::StorageUnavailable)
}

fn parse_optional_id(value: Option<String>) -> Result<Option<Ulid>, LeaseQueueError> {
    value.map(parse_id).transpose()
}

fn parse_hash(value: &str) -> Result<ContentHash, LeaseQueueError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LeaseQueueError::StorageUnavailable);
    }
    let mut bytes = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).map_err(|_| LeaseQueueError::StorageUnavailable)?;
        bytes[index] =
            u8::from_str_radix(text, 16).map_err(|_| LeaseQueueError::StorageUnavailable)?;
    }
    ContentHash::from_bytes(&bytes).map_err(|_| LeaseQueueError::StorageUnavailable)
}

fn hash_hex(value: &ContentHash) -> String {
    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

// `Result::map_err` consumes its error, so this mapper intentionally matches that signature.
#[allow(clippy::needless_pass_by_value)]
fn map_error(error: sqlx::Error) -> LeaseQueueError {
    if let Some(database) = error.as_database_error() {
        return match database.code().as_deref() {
            Some("23503") => LeaseQueueError::NotFound,
            Some("23505") => LeaseQueueError::Conflict,
            Some("23514" | "23502" | "22P02" | "22003") => LeaseQueueError::InvalidValue,
            _ => LeaseQueueError::StorageUnavailable,
        };
    }
    LeaseQueueError::StorageUnavailable
}
