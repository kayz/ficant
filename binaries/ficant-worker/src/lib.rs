mod production;

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use ficant_application::ports::StoredExecutionIdentity;
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid};
use ficant_domain::research::{Artifact, ResearchGraph};
use ficant_runtime::NativePortValue;
use tokio::sync::watch;
use tokio::time::{Instant, sleep, sleep_until};

pub use production::ProductionWorkerBackend;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerConfig {
    pub database_url: String,
    pub s3_endpoint: String,
    pub s3_bucket: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub worker_id: Ulid,
    pub lease_duration: Duration,
    pub renew_interval: Duration,
    pub idle_poll_interval: Duration,
    pub node_timeout: Duration,
}

impl WorkerConfig {
    /// Reads and validates the fail-closed production worker environment.
    ///
    /// # Errors
    ///
    /// Returns an invalid-configuration error without including secret values.
    pub fn from_env() -> Result<Self, WorkerError> {
        let value = Self {
            database_url: required_env("FICANT_WORKER_DATABASE_URL")?,
            s3_endpoint: required_env("FICANT_WORKER_S3_ENDPOINT")?,
            s3_bucket: required_env("FICANT_WORKER_S3_BUCKET")?,
            s3_access_key: required_env("FICANT_WORKER_S3_ACCESS_KEY")?,
            s3_secret_key: required_env("FICANT_WORKER_S3_SECRET_KEY")?,
            worker_id: Ulid::new(required_env("FICANT_WORKER_ID")?)
                .map_err(|_| WorkerError::InvalidConfiguration("FICANT_WORKER_ID"))?,
            lease_duration: seconds_env("FICANT_WORKER_LEASE_SECONDS", 60)?,
            renew_interval: seconds_env("FICANT_WORKER_RENEW_SECONDS", 20)?,
            idle_poll_interval: milliseconds_env("FICANT_WORKER_IDLE_POLL_MS", 500)?,
            node_timeout: seconds_env("FICANT_WORKER_NODE_TIMEOUT_SECONDS", 30)?,
        };
        value.validate()?;
        Ok(value)
    }

    /// Validates timing relationships used by the lease/fencing loop.
    ///
    /// # Errors
    ///
    /// Returns an invalid-configuration error for zero or unsafe durations.
    pub fn validate(&self) -> Result<(), WorkerError> {
        if self.database_url.trim().is_empty()
            || self.s3_endpoint.trim().is_empty()
            || self.s3_bucket.trim().is_empty()
            || self.s3_access_key.trim().is_empty()
            || self.s3_secret_key.trim().is_empty()
        {
            return Err(WorkerError::InvalidConfiguration("worker endpoint"));
        }
        if self.lease_duration.is_zero()
            || self.renew_interval.is_zero()
            || self.idle_poll_interval.is_zero()
            || self.node_timeout.is_zero()
            || self.renew_interval >= self.lease_duration
        {
            return Err(WorkerError::InvalidConfiguration("worker duration"));
        }
        Ok(())
    }

    fn lease_seconds(&self) -> Result<u32, WorkerError> {
        u32::try_from(self.lease_duration.as_secs())
            .map_err(|_| WorkerError::InvalidConfiguration("FICANT_WORKER_LEASE_SECONDS"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedTask {
    pub tenant_id: Ulid,
    pub task_id: Ulid,
    pub run_id: Ulid,
    pub node_id: Ulid,
    pub graph_digest: ContentHash,
    pub execution_identity_digest: ContentHash,
    pub planned_artifact_id: Ulid,
    pub lease_id: Ulid,
    pub attempt: u64,
}

#[derive(Clone, Debug)]
pub struct LoadedTask {
    pub owner: OwnerRef,
    pub graph: ResearchGraph,
    pub stored_identity: StoredExecutionIdentity,
}

#[derive(Clone, Debug)]
pub struct NodeCompletion {
    pub artifact: Artifact,
    pub output_manifest: Vec<u8>,
    pub next_task: Option<ficant_application::ports::EnqueueNode>,
}

#[derive(Clone, Debug)]
pub enum InputSource {
    External { input_id: String },
    Upstream { node_id: Ulid, port_name: String },
}

#[derive(Clone, Debug)]
pub struct InputEvidence {
    pub target_port: String,
    pub value_type: ficant_domain::research::TypedValue,
    pub artifact_id: Ulid,
    pub content_hash: ContentHash,
    pub source: InputSource,
}

#[derive(Clone, Debug)]
pub struct PreparedInputs {
    pub values: Vec<NativePortValue>,
    pub evidence: Vec<InputEvidence>,
}

#[derive(Clone, Debug)]
pub struct ExecutedNode {
    pub outputs: Vec<NativePortValue>,
    pub output_envelope: Vec<u8>,
    pub output_envelope_hash: ContentHash,
    pub input_evidence: Vec<InputEvidence>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerStep {
    Load,
    Begin,
    ReadInput,
    Execute,
    Promote,
    Renew,
    Complete,
    Fail,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerError {
    InvalidConfiguration(&'static str),
    Backend { step: WorkerStep, retryable: bool },
    InvalidTask(&'static str),
    TimedOut,
}

impl WorkerError {
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Backend {
                retryable: true,
                ..
            }
        )
    }
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(name) => {
                write!(formatter, "invalid worker configuration: {name}")
            }
            Self::Backend { step, retryable } => {
                write!(
                    formatter,
                    "worker backend failure at {step:?} (retryable={retryable})"
                )
            }
            Self::InvalidTask(reason) => write!(formatter, "invalid claimed task: {reason}"),
            Self::TimedOut => formatter.write_str("native node execution timed out"),
        }
    }
}

impl std::error::Error for WorkerError {}

#[async_trait]
pub trait WorkerBackend: Send + Sync {
    async fn claim(
        &self,
        worker_id: &Ulid,
        lease_id: &Ulid,
        lease_seconds: u32,
    ) -> Result<Option<ClaimedTask>, WorkerError>;

    async fn renew(
        &self,
        task: &ClaimedTask,
        worker_id: &Ulid,
        lease_seconds: u32,
    ) -> Result<(), WorkerError>;

    async fn load(&self, task: &ClaimedTask, worker_id: &Ulid) -> Result<LoadedTask, WorkerError>;

    async fn begin(&self, task: &ClaimedTask, worker_id: &Ulid) -> Result<(), WorkerError>;

    async fn read_inputs(
        &self,
        task: &ClaimedTask,
        loaded: &LoadedTask,
    ) -> Result<PreparedInputs, WorkerError>;

    async fn execute(
        &self,
        task: &ClaimedTask,
        loaded: &LoadedTask,
        inputs: PreparedInputs,
    ) -> Result<ExecutedNode, WorkerError>;

    async fn promote(
        &self,
        task: &ClaimedTask,
        loaded: &LoadedTask,
        execution: ExecutedNode,
    ) -> Result<NodeCompletion, WorkerError>;

    async fn complete(
        &self,
        task: &ClaimedTask,
        worker_id: &Ulid,
        completion: NodeCompletion,
    ) -> Result<(), WorkerError>;

    async fn fail(
        &self,
        task: &ClaimedTask,
        worker_id: &Ulid,
        failure_hash: ContentHash,
    ) -> Result<(), WorkerError>;
}

/// Claims and executes work until a graceful drain signal is observed.
///
/// An active task is allowed to finish or time out; drain only prevents a new claim.
///
/// # Errors
///
/// Returns only configuration or non-retryable backend failures.
pub async fn run_worker(
    backend: &dyn WorkerBackend,
    config: &WorkerConfig,
    drain: watch::Receiver<bool>,
) -> Result<(), WorkerError> {
    config.validate()?;
    while !*drain.borrow() {
        let lease_id = derived_id(
            b"ficant/worker-lease/v1",
            &[
                config.worker_id.as_str().as_bytes(),
                monotonic_nonce().as_slice(),
            ],
        );
        let claimed = backend
            .claim(&config.worker_id, &lease_id, config.lease_seconds()?)
            .await;
        match claimed {
            Ok(Some(task)) => {
                if let Err(error) = run_claimed(backend, config, &task).await
                    && !error.retryable()
                {
                    return Err(error);
                }
            }
            Ok(None) => sleep(config.idle_poll_interval).await,
            Err(error) if error.retryable() => sleep(config.idle_poll_interval).await,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Executes exactly one already-claimed task under lease renewal and timeout control.
///
/// # Errors
///
/// Returns the original execution error after recording a fenced failure when possible.
pub async fn run_claimed(
    backend: &dyn WorkerBackend,
    config: &WorkerConfig,
    task: &ClaimedTask,
) -> Result<(), WorkerError> {
    let loaded = backend.load(task, &config.worker_id).await?;
    backend.begin(task, &config.worker_id).await?;
    if let Err(error) = validate_loaded(task, &loaded) {
        backend
            .fail(task, &config.worker_id, failure_hash(&error))
            .await?;
        return Err(error);
    }

    let work = async {
        let inputs = backend.read_inputs(task, &loaded).await?;
        let execution = backend.execute(task, &loaded, inputs).await?;
        backend.promote(task, &loaded, execution).await
    };
    tokio::pin!(work);

    let deadline = Instant::now() + config.node_timeout;
    let mut next_renewal = Instant::now() + config.renew_interval;
    let result = loop {
        tokio::select! {
            value = &mut work => break value,
            () = sleep_until(next_renewal) => {
                if let Err(error) = backend
                    .renew(task, &config.worker_id, config.lease_seconds()?)
                    .await
                {
                    break Err(error);
                }
                next_renewal = Instant::now() + config.renew_interval;
            }
            () = sleep_until(deadline) => break Err(WorkerError::TimedOut),
        }
    };

    match result {
        Ok(completion) => backend.complete(task, &config.worker_id, completion).await,
        Err(error) => {
            if !error.retryable() {
                let failure_hash = failure_hash(&error);
                backend.fail(task, &config.worker_id, failure_hash).await?;
            }
            Err(error)
        }
    }
}

fn validate_loaded(task: &ClaimedTask, loaded: &LoadedTask) -> Result<(), WorkerError> {
    if loaded.owner.tenant_id() != &task.tenant_id
        || loaded.graph.digest() != &task.graph_digest
        || loaded.stored_identity.identity.run_id() != &task.run_id
        || loaded.stored_identity.identity.digest() != &task.execution_identity_digest
        || !loaded
            .graph
            .nodes()
            .iter()
            .any(|node| node.node_id() == &task.node_id)
    {
        return Err(WorkerError::InvalidTask("persisted identity mismatch"));
    }
    Ok(())
}

fn failure_hash(error: &WorkerError) -> ContentHash {
    ContentHash::digest(format!("ficant/worker-failure/v1:{error}").as_bytes())
}

fn required_env(name: &'static str) -> Result<String, WorkerError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty() && value.trim() == value)
        .ok_or(WorkerError::InvalidConfiguration(name))
}

fn seconds_env(name: &'static str, default: u64) -> Result<Duration, WorkerError> {
    duration_env(name, default, Duration::from_secs)
}

fn milliseconds_env(name: &'static str, default: u64) -> Result<Duration, WorkerError> {
    duration_env(name, default, Duration::from_millis)
}

fn duration_env(
    name: &'static str,
    default: u64,
    convert: fn(u64) -> Duration,
) -> Result<Duration, WorkerError> {
    let value = match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| WorkerError::InvalidConfiguration(name))?,
        Err(std::env::VarError::NotPresent) => default,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(WorkerError::InvalidConfiguration(name));
        }
    };
    Ok(convert(value))
}

fn monotonic_nonce() -> [u8; 16] {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let mixed = time ^ u128::from(COUNTER.fetch_add(1, Ordering::Relaxed));
    mixed.to_be_bytes()
}

pub(crate) fn derived_id(domain: &[u8], parts: &[&[u8]]) -> Ulid {
    let mut bytes = domain.to_vec();
    for part in parts {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    let digest = ContentHash::digest(&bytes);
    let mut first = [0_u8; 16];
    first.copy_from_slice(&digest.as_bytes()[..16]);
    let mut value = u128::from_be_bytes(first);
    let alphabet = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut encoded = [b'0'; 26];
    for position in (0..26).rev() {
        encoded[position] = alphabet[(value & 31) as usize];
        value >>= 5;
    }
    Ulid::new(String::from_utf8(encoded.to_vec()).expect("ULID alphabet is UTF-8"))
        .expect("128-bit Crockford encoding is canonical")
}

#[cfg(test)]
mod tests;
