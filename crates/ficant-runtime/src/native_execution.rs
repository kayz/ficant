use std::collections::{BTreeMap, BTreeSet};

use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::{ContentHash, Ulid};
use ficant_domain::research::{ResearchGraph, ResearchNode, TypedValue};

use crate::RuntimeError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeImplementation {
    pub node_id: Ulid,
    pub implementation_digest: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionIdentityInput {
    pub run_id: Ulid,
    pub data_snapshot_hash: ContentHash,
    pub universe_snapshot_hash: ContentHash,
    pub parameters_hash: ContentHash,
    pub runtime_image_digest: ContentHash,
    pub environment_digest: ContentHash,
    pub seed: u64,
    pub node_implementations: Vec<NodeImplementation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionIdentity {
    run_id: Ulid,
    data_snapshot_hash: ContentHash,
    universe_snapshot_hash: ContentHash,
    graph_digest: ContentHash,
    parameters_hash: ContentHash,
    runtime_image_digest: ContentHash,
    environment_digest: ContentHash,
    seed: u64,
    node_implementations: Vec<NodeImplementation>,
    digest: ContentHash,
}

impl ExecutionIdentity {
    /// Freezes every input that may change a native graph result.
    ///
    /// # Errors
    ///
    /// Returns a stable invalid-value error when implementation bindings do not exactly cover the graph.
    pub fn new(graph: &ResearchGraph, input: ExecutionIdentityInput) -> Result<Self, RuntimeError> {
        let mut implementations = input.node_implementations;
        implementations.sort_by(|left, right| left.node_id.cmp(&right.node_id));
        if implementations
            .windows(2)
            .any(|pair| pair[0].node_id == pair[1].node_id)
            || implementations.len() != graph.nodes().len()
            || !implementations
                .iter()
                .zip(graph.nodes())
                .all(|(binding, node)| binding.node_id == *node.node_id())
        {
            return Err(invalid());
        }
        let mut result = Self {
            run_id: input.run_id,
            data_snapshot_hash: input.data_snapshot_hash,
            universe_snapshot_hash: input.universe_snapshot_hash,
            graph_digest: graph.digest().clone(),
            parameters_hash: input.parameters_hash,
            runtime_image_digest: input.runtime_image_digest,
            environment_digest: input.environment_digest,
            seed: input.seed,
            node_implementations: implementations,
            digest: ContentHash::digest(b"uninitialized"),
        };
        result.digest = ContentHash::digest(&result.canonical_bytes());
        Ok(result)
    }

    #[must_use]
    pub fn digest(&self) -> &ContentHash {
        &self.digest
    }
    #[must_use]
    pub fn run_id(&self) -> &Ulid {
        &self.run_id
    }
    #[must_use]
    pub fn graph_digest(&self) -> &ContentHash {
        &self.graph_digest
    }
    #[must_use]
    pub fn data_snapshot_hash(&self) -> &ContentHash {
        &self.data_snapshot_hash
    }
    #[must_use]
    pub fn universe_snapshot_hash(&self) -> &ContentHash {
        &self.universe_snapshot_hash
    }
    #[must_use]
    pub fn parameters_hash(&self) -> &ContentHash {
        &self.parameters_hash
    }
    #[must_use]
    pub fn runtime_image_digest(&self) -> &ContentHash {
        &self.runtime_image_digest
    }
    #[must_use]
    pub fn environment_digest(&self) -> &ContentHash {
        &self.environment_digest
    }
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }
    #[must_use]
    pub fn node_implementations(&self) -> &[NodeImplementation] {
        &self.node_implementations
    }

    fn implementation(&self, node_id: &Ulid) -> Option<&ContentHash> {
        self.node_implementations
            .binary_search_by(|binding| binding.node_id.cmp(node_id))
            .ok()
            .map(|index| &self.node_implementations[index].implementation_digest)
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = b"ficant/execution-identity/v1".to_vec();
        push_str(&mut bytes, self.run_id.as_str());
        for hash in [
            &self.data_snapshot_hash,
            &self.universe_snapshot_hash,
            &self.graph_digest,
            &self.parameters_hash,
            &self.runtime_image_digest,
            &self.environment_digest,
        ] {
            bytes.extend_from_slice(hash.as_bytes());
        }
        bytes.extend_from_slice(&self.seed.to_be_bytes());
        push_u64(&mut bytes, self.node_implementations.len() as u64);
        for binding in &self.node_implementations {
            push_str(&mut bytes, binding.node_id.as_str());
            bytes.extend_from_slice(binding.implementation_digest.as_bytes());
        }
        bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativePortValue {
    port_name: String,
    value_type: TypedValue,
    payload: Vec<u8>,
    content_hash: ContentHash,
}

impl NativePortValue {
    /// Creates one immutable typed node output.
    ///
    /// # Errors
    ///
    /// Returns `InvalidValue` for an empty/padded port or empty payload.
    pub fn new(
        port_name: impl Into<String>,
        value_type: TypedValue,
        payload: Vec<u8>,
    ) -> Result<Self, RuntimeError> {
        let port_name = port_name.into();
        if port_name.is_empty() || port_name.trim() != port_name || payload.is_empty() {
            return Err(invalid());
        }
        let content_hash = ContentHash::digest(&payload);
        Ok(Self {
            port_name,
            value_type,
            payload,
            content_hash,
        })
    }
    #[must_use]
    pub fn port_name(&self) -> &str {
        &self.port_name
    }
    #[must_use]
    pub fn value_type(&self) -> &TypedValue {
        &self.value_type
    }
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

pub struct NativeNodeRequest<'a> {
    node: &'a ResearchNode,
    identity: &'a ExecutionIdentity,
    inputs: &'a [NativePortValue],
}

impl NativeNodeRequest<'_> {
    #[must_use]
    pub fn node(&self) -> &ResearchNode {
        self.node
    }
    #[must_use]
    pub fn identity(&self) -> &ExecutionIdentity {
        self.identity
    }
    #[must_use]
    pub fn inputs(&self) -> &[NativePortValue] {
        self.inputs
    }
}

pub trait NativeNode {
    fn node_id(&self) -> &Ulid;
    fn implementation_digest(&self) -> &ContentHash;
    /// Executes one validated request without external side effects.
    ///
    /// # Errors
    ///
    /// Returns a stable runtime/domain error when execution cannot produce the declared outputs.
    fn execute(
        &self,
        request: &NativeNodeRequest<'_>,
    ) -> Result<Vec<NativePortValue>, RuntimeError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeNodeArtifact {
    node_id: Ulid,
    contract_digest: ContentHash,
    implementation_digest: ContentHash,
    input_artifacts: Vec<ContentHash>,
    output_hashes: Vec<ContentHash>,
    artifact_digest: ContentHash,
}

impl NativeNodeArtifact {
    #[must_use]
    pub fn node_id(&self) -> &Ulid {
        &self.node_id
    }
    #[must_use]
    pub fn contract_digest(&self) -> &ContentHash {
        &self.contract_digest
    }
    #[must_use]
    pub fn implementation_digest(&self) -> &ContentHash {
        &self.implementation_digest
    }
    #[must_use]
    pub fn input_artifacts(&self) -> &[ContentHash] {
        &self.input_artifacts
    }
    #[must_use]
    pub fn output_hashes(&self) -> &[ContentHash] {
        &self.output_hashes
    }
    #[must_use]
    pub fn artifact_digest(&self) -> &ContentHash {
        &self.artifact_digest
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeExecutionResult {
    identity: ExecutionIdentity,
    artifacts: Vec<NativeNodeArtifact>,
    result_digest: ContentHash,
}

impl NativeExecutionResult {
    #[must_use]
    pub fn identity(&self) -> &ExecutionIdentity {
        &self.identity
    }
    #[must_use]
    pub fn artifacts(&self) -> &[NativeNodeArtifact] {
        &self.artifacts
    }
    #[must_use]
    pub fn result_digest(&self) -> &ContentHash {
        &self.result_digest
    }
}

/// Executes every native node exactly once in deterministic graph order.
///
/// # Errors
///
/// Returns a stable error for missing/duplicate implementations or any input/output contract drift.
pub fn execute_native_graph(
    graph: &ResearchGraph,
    identity: &ExecutionIdentity,
    executors: &[&dyn NativeNode],
) -> Result<NativeExecutionResult, RuntimeError> {
    if identity.graph_digest != *graph.digest() || executors.len() != graph.nodes().len() {
        return Err(invalid());
    }
    let registry = executor_registry(executors)?;
    let nodes = research_nodes(graph);
    let mut outputs: BTreeMap<(Ulid, String), NativePortValue> = BTreeMap::new();
    let mut artifacts: BTreeMap<Ulid, NativeNodeArtifact> = BTreeMap::new();
    for node_id in graph.topological_order() {
        let node = nodes.get(node_id).ok_or_else(broken)?;
        let executor = registry.get(node_id).ok_or_else(broken)?;
        if identity.implementation(node_id) != Some(executor.implementation_digest()) {
            return Err(invalid());
        }
        let mut inputs = Vec::new();
        let mut input_artifacts = Vec::new();
        for edge in graph
            .edges()
            .iter()
            .filter(|edge| edge.to_node() == node_id)
        {
            let source = outputs
                .get(&(edge.from_node().clone(), edge.from_port().to_owned()))
                .ok_or_else(broken)?;
            inputs.push(NativePortValue::new(
                edge.to_port(),
                source.value_type.clone(),
                source.payload.clone(),
            )?);
            let artifact = artifacts.get(edge.from_node()).ok_or_else(broken)?;
            input_artifacts.push(artifact.artifact_digest.clone());
        }
        inputs.sort_by(|left, right| left.port_name.cmp(&right.port_name));
        input_artifacts.sort();
        input_artifacts.dedup();
        if inputs.len() != node.contract().input_types().len()
            || !inputs
                .iter()
                .zip(node.contract().input_types())
                .all(|(value, port)| {
                    value.port_name == port.port_name() && value.value_type == *port.value_type()
                })
        {
            return Err(invalid());
        }
        let request = NativeNodeRequest {
            node,
            identity,
            inputs: &inputs,
        };
        let mut node_outputs = executor.execute(&request)?;
        node_outputs.sort_by(|left, right| left.port_name.cmp(&right.port_name));
        if node_outputs.len() != node.contract().output_types().len()
            || !node_outputs
                .iter()
                .zip(node.contract().output_types())
                .all(|(value, port)| {
                    value.port_name == port.port_name() && value.value_type == *port.value_type()
                })
        {
            return Err(invalid());
        }
        let output_hashes = node_outputs
            .iter()
            .map(|value| value.content_hash.clone())
            .collect::<Vec<_>>();
        let artifact_digest = artifact_digest(
            identity.digest(),
            node,
            executor.implementation_digest(),
            &input_artifacts,
            &output_hashes,
        );
        artifacts.insert(
            node_id.clone(),
            NativeNodeArtifact {
                node_id: node_id.clone(),
                contract_digest: node.contract().digest().clone(),
                implementation_digest: executor.implementation_digest().clone(),
                input_artifacts,
                output_hashes,
                artifact_digest,
            },
        );
        for output in node_outputs {
            outputs.insert((node_id.clone(), output.port_name.clone()), output);
        }
    }
    let ordered = graph
        .topological_order()
        .iter()
        .map(|id| artifacts.remove(id).ok_or_else(broken))
        .collect::<Result<Vec<_>, _>>()?;
    let result_digest = result_digest(identity.digest(), &ordered);
    Ok(NativeExecutionResult {
        identity: identity.clone(),
        artifacts: ordered,
        result_digest,
    })
}

fn result_digest(identity: &ContentHash, artifacts: &[NativeNodeArtifact]) -> ContentHash {
    let mut bytes = b"ficant/native-execution-result/v1".to_vec();
    bytes.extend_from_slice(identity.as_bytes());
    for artifact in artifacts {
        bytes.extend_from_slice(artifact.artifact_digest.as_bytes());
    }
    ContentHash::digest(&bytes)
}

fn executor_registry<'a>(
    executors: &[&'a dyn NativeNode],
) -> Result<BTreeMap<Ulid, &'a dyn NativeNode>, RuntimeError> {
    let mut registry = BTreeMap::new();
    for executor in executors {
        if registry
            .insert(executor.node_id().clone(), *executor)
            .is_some()
        {
            return Err(invalid());
        }
    }
    Ok(registry)
}

fn research_nodes(graph: &ResearchGraph) -> BTreeMap<Ulid, &ResearchNode> {
    graph
        .nodes()
        .iter()
        .map(|node| (node.node_id().clone(), node))
        .collect()
}

/// Confirms that a replay produced the exact same frozen identity and node lineage.
///
/// # Errors
///
/// Returns `ContentHashMismatch` for any identity, artifact, or final result drift.
pub fn verify_native_replay(
    expected: &NativeExecutionResult,
    replayed: &NativeExecutionResult,
) -> Result<(), RuntimeError> {
    if expected != replayed {
        return Err(RuntimeError::Domain(DomainErrorCode::ContentHashMismatch));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComparisonDimension {
    DataSnapshot,
    UniverseSnapshot,
    Graph,
    Parameters,
    RuntimeImage,
    Environment,
    Seed,
    Implementation,
    Result,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExperimentComparison {
    differences: Vec<ComparisonDimension>,
}
impl ExperimentComparison {
    #[must_use]
    pub fn differences(&self) -> &[ComparisonDimension] {
        &self.differences
    }
    #[must_use]
    pub fn identical(&self) -> bool {
        self.differences.is_empty()
    }
}

#[must_use]
pub fn compare_experiments(
    left: &NativeExecutionResult,
    right: &NativeExecutionResult,
) -> ExperimentComparison {
    let mut differences = BTreeSet::new();
    let l = &left.identity;
    let r = &right.identity;
    if l.data_snapshot_hash != r.data_snapshot_hash {
        differences.insert(ComparisonDimension::DataSnapshot);
    }
    if l.universe_snapshot_hash != r.universe_snapshot_hash {
        differences.insert(ComparisonDimension::UniverseSnapshot);
    }
    if l.graph_digest != r.graph_digest {
        differences.insert(ComparisonDimension::Graph);
    }
    if l.parameters_hash != r.parameters_hash {
        differences.insert(ComparisonDimension::Parameters);
    }
    if l.runtime_image_digest != r.runtime_image_digest {
        differences.insert(ComparisonDimension::RuntimeImage);
    }
    if l.environment_digest != r.environment_digest {
        differences.insert(ComparisonDimension::Environment);
    }
    if l.seed != r.seed {
        differences.insert(ComparisonDimension::Seed);
    }
    if l.node_implementations != r.node_implementations {
        differences.insert(ComparisonDimension::Implementation);
    }
    if left.result_digest != right.result_digest {
        differences.insert(ComparisonDimension::Result);
    }
    ExperimentComparison {
        differences: differences.into_iter().collect(),
    }
}

fn artifact_digest(
    identity: &ContentHash,
    node: &ResearchNode,
    implementation: &ContentHash,
    inputs: &[ContentHash],
    outputs: &[ContentHash],
) -> ContentHash {
    let mut bytes = b"ficant/native-node-artifact/v1".to_vec();
    bytes.extend_from_slice(identity.as_bytes());
    push_str(&mut bytes, node.node_id().as_str());
    bytes.extend_from_slice(node.contract().digest().as_bytes());
    bytes.extend_from_slice(implementation.as_bytes());
    push_u64(&mut bytes, inputs.len() as u64);
    for hash in inputs {
        bytes.extend_from_slice(hash.as_bytes());
    }
    push_u64(&mut bytes, outputs.len() as u64);
    for hash in outputs {
        bytes.extend_from_slice(hash.as_bytes());
    }
    ContentHash::digest(&bytes)
}
fn push_str(bytes: &mut Vec<u8>, value: &str) {
    push_u64(bytes, value.len() as u64);
    bytes.extend_from_slice(value.as_bytes());
}
fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_be_bytes());
}
fn invalid() -> RuntimeError {
    RuntimeError::Domain(DomainErrorCode::InvalidValue)
}
fn broken() -> RuntimeError {
    RuntimeError::Domain(DomainErrorCode::BrokenLineage)
}
