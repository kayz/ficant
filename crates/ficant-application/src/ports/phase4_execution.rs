use async_trait::async_trait;
use ficant_domain::primitives::{ContentHash, OwnerRef, Ulid, Version};
use ficant_domain::research::{Artifact, ExperimentRun, ResearchGraph};
pub use ficant_runtime::{
    ExecutionExternalInput, ExecutionInstanceIdentity, GraphNodeEvent, GraphReplayResult,
    NodeImplementation, ReproducibilityIdentity, ReproducibilityIdentityInput, RulePackBinding,
    replay_graph_execution,
};

use super::{AccessScope, ApplicationResult, IdempotencyKey, VerifiedBlobRef};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalInputArtifactBinding {
    pub input_id: String,
    pub artifact_id: Ulid,
    pub content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredExecutionIdentity {
    pub owner: OwnerRef,
    pub graph_id: Ulid,
    pub graph_version: Version,
    pub identity: ExecutionInstanceIdentity,
    pub external_input_artifacts: Vec<ExternalInputArtifactBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnqueueNode {
    pub tenant_id: Ulid,
    pub task_id: Ulid,
    pub run_id: Ulid,
    pub node_id: Ulid,
    pub graph_digest: ContentHash,
    pub execution_identity_digest: ContentHash,
    pub planned_artifact_id: Ulid,
    pub task_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeLeaseFence {
    pub tenant_id: Ulid,
    pub task_id: Ulid,
    pub run_id: Ulid,
    pub node_id: Ulid,
    pub worker_id: Ulid,
    pub lease_id: Ulid,
    /// The queue claim count is the attempt and fencing token.
    pub attempt: u64,
    pub execution_identity_digest: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BeginNode {
    pub fence: NodeLeaseFence,
    pub started_event_id: Ulid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompleteNode {
    pub fence: NodeLeaseFence,
    pub artifact: Artifact,
    /// Capability returned by the object store after hashing the promoted immutable object.
    ///
    /// The repository, rather than the worker, binds this proof to the planned Artifact.
    pub verified_blob: VerifiedBlobRef,
    /// Canonical output envelope bytes whose size and hash are bound to `verified_blob`.
    pub verified_payload: Vec<u8>,
    pub output_manifest: Vec<u8>,
    pub succeeded_event_id: Ulid,
    pub checkpoint_event_id: Ulid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailNode {
    pub fence: NodeLeaseFence,
    pub failure_hash: ContentHash,
    pub failed_event_id: Ulid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeJournalEvidence {
    pub sequence: u64,
    pub event_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeBeginResult {
    pub evidence: NodeJournalEvidence,
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeSuccessResult {
    pub artifact: Artifact,
    pub succeeded: NodeJournalEvidence,
    pub checkpointed: NodeJournalEvidence,
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeFailureResult {
    pub failed: NodeJournalEvidence,
    pub replayed: bool,
}

/// All frozen material required to start a graph run atomically.
///
/// Authentication, tenant/owner scope and trusted runtime attestation are constructed by the
/// application/API boundary. The repository never accepts a partially-created run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitGraphRun {
    pub scope: AccessScope,
    pub idempotency_key: IdempotencyKey,
    pub run: ExperimentRun,
    pub graph: ResearchGraph,
    pub identity: StoredExecutionIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRunRecord {
    pub run: ExperimentRun,
    pub graph: ResearchGraph,
    pub identity: StoredExecutionIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredNodeManifest {
    pub run_id: Ulid,
    pub node_id: Ulid,
    pub attempt: u64,
    pub artifact: Artifact,
    pub manifest_hash: ContentHash,
    pub manifest: Vec<u8>,
    pub checkpoint: NodeJournalEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputTrace {
    pub run: GraphRunRecord,
    /// Upstream-first manifests needed to reproduce and explain the selected output.
    pub manifests: Vec<StoredNodeManifest>,
    pub external_inputs: Vec<ExternalInputArtifactBinding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComparisonDimension {
    Data,
    Universe,
    Graph,
    Parameters,
    Runtime,
    Environment,
    Seed,
    RulePack,
    Implementation,
    ExternalInput,
    Result,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRunComparison {
    pub left_run_id: Ulid,
    pub right_run_id: Ulid,
    pub differing_dimensions: Vec<ComparisonDimension>,
}

/// Derives the immutable Artifact identity from result-affecting identity plus logical node.
///
/// The first 128 bits of the domain-separated SHA-256 digest are encoded as canonical Crockford
/// base32. The run id is deliberately absent, so identical reproducibility identities can reuse
/// the same Artifact across runs.
///
/// # Panics
///
/// Panics only if the fixed Crockford alphabet cannot be represented as UTF-8 or the canonical
/// 128-bit Crockford encoding is rejected by the domain `Ulid` parser.
#[must_use]
pub fn stable_node_artifact_id(reproducibility_digest: &ContentHash, node_id: &Ulid) -> Ulid {
    let mut bytes = b"ficant/native-node-artifact-id/v1".to_vec();
    bytes.extend_from_slice(reproducibility_digest.as_bytes());
    bytes.extend_from_slice(node_id.as_str().as_bytes());
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
    // Every Crockford digit above is ASCII and a 128-bit value always has a canonical first digit.
    Ulid::new(String::from_utf8(encoded.to_vec()).expect("Crockford alphabet is UTF-8"))
        .expect("128-bit Crockford encoding is a canonical ULID")
}

#[async_trait]
pub trait Phase4ExecutionRepository: Send + Sync {
    /// Publishes graph, run, journal, identity, bindings and the first task in one transaction.
    async fn submit_graph_run(&self, command: SubmitGraphRun) -> ApplicationResult<GraphRunRecord>;

    async fn publish_graph(
        &self,
        scope: &AccessScope,
        graph: ResearchGraph,
    ) -> ApplicationResult<ResearchGraph>;

    async fn load_graph(
        &self,
        scope: &AccessScope,
        graph_id: &Ulid,
        version: Version,
    ) -> ApplicationResult<Option<ResearchGraph>>;

    async fn publish_execution_identity(
        &self,
        scope: &AccessScope,
        value: StoredExecutionIdentity,
    ) -> ApplicationResult<StoredExecutionIdentity>;

    async fn load_execution_identity(
        &self,
        scope: &AccessScope,
        run_id: &Ulid,
    ) -> ApplicationResult<Option<StoredExecutionIdentity>>;

    async fn get_graph_run(
        &self,
        scope: &AccessScope,
        run_id: &Ulid,
    ) -> ApplicationResult<Option<GraphRunRecord>>;

    async fn list_node_manifests(
        &self,
        scope: &AccessScope,
        run_id: &Ulid,
    ) -> ApplicationResult<Vec<StoredNodeManifest>>;

    async fn trace_output(
        &self,
        scope: &AccessScope,
        run_id: &Ulid,
        node_id: &Ulid,
    ) -> ApplicationResult<Option<OutputTrace>>;

    async fn compare_graph_runs(
        &self,
        scope: &AccessScope,
        left_run_id: &Ulid,
        right_run_id: &Ulid,
    ) -> ApplicationResult<Option<GraphRunComparison>>;

    async fn enqueue_node(&self, command: EnqueueNode) -> ApplicationResult<()>;

    async fn begin_node(&self, command: BeginNode) -> ApplicationResult<NodeBeginResult>;

    /// Atomically publishes/reuses the verified Artifact, records the output manifest, appends
    /// `NodeSucceeded` and `NodeCheckpointed`, completes the lease, and advances the graph/run.
    async fn complete_node(&self, command: CompleteNode) -> ApplicationResult<NodeSuccessResult>;

    /// Atomically appends `NodeFailed`, fails the logical task and transitions the run to FAILED.
    async fn fail_node(&self, command: FailNode) -> ApplicationResult<NodeFailureResult>;
}
