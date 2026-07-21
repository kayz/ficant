use std::collections::{BTreeMap, BTreeSet};

use crate::primitives::{ContentHash, OwnerRef, Ulid, Version};
use crate::{DomainErrorCode, DomainResult, VersionedDefinition};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeterminismClass {
    Deterministic,
    Seeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilesystemPermission {
    None,
    TemporaryOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodePermissions {
    pub network: bool,
    pub database: bool,
    pub filesystem: FilesystemPermission,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceLimits {
    cpu_cores: u16,
    memory_mb: u32,
    timeout_seconds: u32,
}

impl ResourceLimits {
    pub fn new(cpu_cores: u16, memory_mb: u32, timeout_seconds: u32) -> DomainResult<Self> {
        if cpu_cores == 0 || memory_mb == 0 || timeout_seconds == 0 {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            cpu_cores,
            memory_mb,
            timeout_seconds,
        })
    }

    pub fn cpu_cores(self) -> u16 {
        self.cpu_cores
    }

    pub fn memory_mb(self) -> u32 {
        self.memory_mb
    }

    pub fn timeout_seconds(self) -> u32 {
        self.timeout_seconds
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedValue {
    type_id: String,
    type_version: Version,
    schema_hash: ContentHash,
}

impl TypedValue {
    pub fn new(
        type_id: impl Into<String>,
        type_version: Version,
        schema_hash: ContentHash,
    ) -> DomainResult<Self> {
        let type_id = type_id.into();
        ensure_symbol(&type_id)?;
        Ok(Self {
            type_id,
            type_version,
            schema_hash,
        })
    }

    pub fn type_id(&self) -> &str {
        &self.type_id
    }

    pub fn type_version(&self) -> Version {
        self.type_version
    }

    pub fn schema_hash(&self) -> &ContentHash {
        &self.schema_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortType {
    port_name: String,
    value_type: TypedValue,
}

impl PortType {
    pub fn new(port_name: impl Into<String>, value_type: TypedValue) -> DomainResult<Self> {
        let port_name = port_name.into();
        ensure_symbol(&port_name)?;
        Ok(Self {
            port_name,
            value_type,
        })
    }

    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    pub fn value_type(&self) -> &TypedValue {
        &self.value_type
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchNodeContract {
    contract_id: String,
    contract_version: Version,
    input_types: Vec<PortType>,
    output_types: Vec<PortType>,
    state_schema: ContentHash,
    parameter_schema: ContentHash,
    determinism_class: DeterminismClass,
    permissions: NodePermissions,
    resource_limits: ResourceLimits,
    required_invariants: Vec<String>,
    digest: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchNodeContractInput {
    pub contract_id: String,
    pub contract_version: Version,
    pub input_types: Vec<PortType>,
    pub output_types: Vec<PortType>,
    pub state_schema: ContentHash,
    pub parameter_schema: ContentHash,
    pub determinism_class: DeterminismClass,
    pub permissions: NodePermissions,
    pub resource_limits: ResourceLimits,
    pub required_invariants: Vec<String>,
}

impl ResearchNodeContract {
    pub fn new(input: ResearchNodeContractInput) -> DomainResult<Self> {
        ensure_symbol(&input.contract_id)?;
        if input.output_types.is_empty() || input.required_invariants.is_empty() {
            return Err(DomainErrorCode::InvalidValue);
        }
        let input_types = canonical_ports(input.input_types)?;
        let output_types = canonical_ports(input.output_types)?;
        let required_invariants = canonical_symbols(input.required_invariants)?;
        let mut result = Self {
            contract_id: input.contract_id,
            contract_version: input.contract_version,
            input_types,
            output_types,
            state_schema: input.state_schema,
            parameter_schema: input.parameter_schema,
            determinism_class: input.determinism_class,
            permissions: input.permissions,
            resource_limits: input.resource_limits,
            required_invariants,
            digest: ContentHash::digest(b"uninitialized"),
        };
        result.digest = ContentHash::digest(&result.canonical_bytes());
        Ok(result)
    }

    pub fn contract_id(&self) -> &str {
        &self.contract_id
    }

    pub fn contract_version(&self) -> Version {
        self.contract_version
    }

    pub fn input_types(&self) -> &[PortType] {
        &self.input_types
    }

    pub fn output_types(&self) -> &[PortType] {
        &self.output_types
    }

    pub fn state_schema(&self) -> &ContentHash {
        &self.state_schema
    }

    pub fn parameter_schema(&self) -> &ContentHash {
        &self.parameter_schema
    }

    pub fn determinism_class(&self) -> DeterminismClass {
        self.determinism_class
    }

    pub fn permissions(&self) -> NodePermissions {
        self.permissions
    }

    pub fn resource_limits(&self) -> ResourceLimits {
        self.resource_limits
    }

    pub fn required_invariants(&self) -> &[String] {
        &self.required_invariants
    }

    pub fn digest(&self) -> &ContentHash {
        &self.digest
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = b"ficant/research-node-contract/v1".to_vec();
        push_str(&mut bytes, &self.contract_id);
        push_u64(&mut bytes, self.contract_version.get());
        push_ports(&mut bytes, &self.input_types);
        push_ports(&mut bytes, &self.output_types);
        bytes.extend_from_slice(self.state_schema.as_bytes());
        bytes.extend_from_slice(self.parameter_schema.as_bytes());
        bytes.push(match self.determinism_class {
            DeterminismClass::Deterministic => 1,
            DeterminismClass::Seeded => 2,
        });
        bytes.push(u8::from(self.permissions.network));
        bytes.push(u8::from(self.permissions.database));
        bytes.push(match self.permissions.filesystem {
            FilesystemPermission::None => 1,
            FilesystemPermission::TemporaryOnly => 2,
        });
        bytes.extend_from_slice(&self.resource_limits.cpu_cores.to_be_bytes());
        bytes.extend_from_slice(&self.resource_limits.memory_mb.to_be_bytes());
        bytes.extend_from_slice(&self.resource_limits.timeout_seconds.to_be_bytes());
        push_u64(&mut bytes, self.required_invariants.len() as u64);
        for invariant in &self.required_invariants {
            push_str(&mut bytes, invariant);
        }
        bytes
    }
}

impl VersionedDefinition for ResearchNodeContract {
    fn identity(&self) -> &str {
        &self.contract_id
    }

    fn version(&self) -> u64 {
        self.contract_version.get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchNode {
    node_id: Ulid,
    contract: ResearchNodeContract,
    parameters_hash: ContentHash,
}

impl ResearchNode {
    pub fn new(
        node_id: Ulid,
        contract: ResearchNodeContract,
        parameters_hash: ContentHash,
    ) -> Self {
        Self {
            node_id,
            contract,
            parameters_hash,
        }
    }

    pub fn node_id(&self) -> &Ulid {
        &self.node_id
    }

    pub fn contract(&self) -> &ResearchNodeContract {
        &self.contract
    }

    pub fn parameters_hash(&self) -> &ContentHash {
        &self.parameters_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResearchEdge {
    from_node: Ulid,
    from_port: String,
    to_node: Ulid,
    to_port: String,
}

impl ResearchEdge {
    pub fn new(
        from_node: Ulid,
        from_port: impl Into<String>,
        to_node: Ulid,
        to_port: impl Into<String>,
    ) -> DomainResult<Self> {
        let from_port = from_port.into();
        let to_port = to_port.into();
        ensure_symbol(&from_port)?;
        ensure_symbol(&to_port)?;
        if from_node == to_node {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            from_node,
            from_port,
            to_node,
            to_port,
        })
    }

    pub fn from_node(&self) -> &Ulid {
        &self.from_node
    }

    pub fn from_port(&self) -> &str {
        &self.from_port
    }

    pub fn to_node(&self) -> &Ulid {
        &self.to_node
    }

    pub fn to_port(&self) -> &str {
        &self.to_port
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchGraph {
    graph_id: Ulid,
    version: Version,
    owner: OwnerRef,
    nodes: Vec<ResearchNode>,
    edges: Vec<ResearchEdge>,
    topological_order: Vec<Ulid>,
    digest: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchGraphInput {
    pub graph_id: Ulid,
    pub version: Version,
    pub owner: OwnerRef,
    pub nodes: Vec<ResearchNode>,
    pub edges: Vec<ResearchEdge>,
}

impl ResearchGraph {
    pub fn new(input: ResearchGraphInput) -> DomainResult<Self> {
        if input.nodes.is_empty() {
            return Err(DomainErrorCode::InvalidValue);
        }
        let mut nodes = input.nodes;
        nodes.sort_by(|left, right| left.node_id.cmp(&right.node_id));
        if nodes
            .windows(2)
            .any(|pair| pair[0].node_id == pair[1].node_id)
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        let node_map: BTreeMap<Ulid, &ResearchNode> = nodes
            .iter()
            .map(|node| (node.node_id.clone(), node))
            .collect();
        let mut edges = input.edges;
        edges.sort();
        if edges.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(DomainErrorCode::InvalidValue);
        }
        validate_edges(&node_map, &edges)?;
        let topological_order = topological_order(&node_map, &edges)?;
        let mut result = Self {
            graph_id: input.graph_id,
            version: input.version,
            owner: input.owner,
            nodes,
            edges,
            topological_order,
            digest: ContentHash::digest(b"uninitialized"),
        };
        result.digest = ContentHash::digest(&result.canonical_bytes());
        Ok(result)
    }

    pub fn graph_id(&self) -> &Ulid {
        &self.graph_id
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn nodes(&self) -> &[ResearchNode] {
        &self.nodes
    }

    pub fn edges(&self) -> &[ResearchEdge] {
        &self.edges
    }

    pub fn topological_order(&self) -> &[Ulid] {
        &self.topological_order
    }

    pub fn digest(&self) -> &ContentHash {
        &self.digest
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = b"ficant/research-graph/v1".to_vec();
        push_str(&mut bytes, self.graph_id.as_str());
        push_u64(&mut bytes, self.version.get());
        push_str(&mut bytes, self.owner.tenant_id().as_str());
        push_str(&mut bytes, self.owner.owner_id().as_str());
        push_u64(&mut bytes, self.nodes.len() as u64);
        for node in &self.nodes {
            push_str(&mut bytes, node.node_id.as_str());
            bytes.extend_from_slice(node.contract.digest.as_bytes());
            bytes.extend_from_slice(node.parameters_hash.as_bytes());
        }
        push_u64(&mut bytes, self.edges.len() as u64);
        for edge in &self.edges {
            push_str(&mut bytes, edge.from_node.as_str());
            push_str(&mut bytes, &edge.from_port);
            push_str(&mut bytes, edge.to_node.as_str());
            push_str(&mut bytes, &edge.to_port);
        }
        bytes
    }
}

impl VersionedDefinition for ResearchGraph {
    fn identity(&self) -> &str {
        self.graph_id.as_str()
    }

    fn version(&self) -> u64 {
        self.version.get()
    }
}

fn validate_edges(
    nodes: &BTreeMap<Ulid, &ResearchNode>,
    edges: &[ResearchEdge],
) -> DomainResult<()> {
    let mut bindings = BTreeSet::new();
    for edge in edges {
        let source = nodes
            .get(&edge.from_node)
            .ok_or(DomainErrorCode::BrokenLineage)?;
        let target = nodes
            .get(&edge.to_node)
            .ok_or(DomainErrorCode::BrokenLineage)?;
        let output = port(&source.contract.output_types, &edge.from_port)?;
        let input = port(&target.contract.input_types, &edge.to_port)?;
        if output.value_type != input.value_type {
            return Err(DomainErrorCode::InvalidValue);
        }
        if !bindings.insert((edge.to_node.clone(), edge.to_port.clone())) {
            return Err(DomainErrorCode::InvalidValue);
        }
    }
    for node in nodes.values() {
        for input in &node.contract.input_types {
            if !bindings.contains(&(node.node_id.clone(), input.port_name.clone())) {
                return Err(DomainErrorCode::BrokenLineage);
            }
        }
    }
    Ok(())
}

fn topological_order(
    nodes: &BTreeMap<Ulid, &ResearchNode>,
    edges: &[ResearchEdge],
) -> DomainResult<Vec<Ulid>> {
    let mut indegree: BTreeMap<Ulid, usize> =
        nodes.keys().cloned().map(|node_id| (node_id, 0)).collect();
    let mut outgoing: BTreeMap<Ulid, Vec<Ulid>> = BTreeMap::new();
    for edge in edges {
        let value = indegree
            .get_mut(&edge.to_node)
            .ok_or(DomainErrorCode::BrokenLineage)?;
        *value = value.checked_add(1).ok_or(DomainErrorCode::InvalidValue)?;
        outgoing
            .entry(edge.from_node.clone())
            .or_default()
            .push(edge.to_node.clone());
    }
    let mut ready: BTreeSet<Ulid> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(node_id, _)| node_id.clone())
        .collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(node_id) = ready.pop_first() {
        order.push(node_id.clone());
        if let Some(targets) = outgoing.get(&node_id) {
            for target in targets {
                let degree = indegree
                    .get_mut(target)
                    .ok_or(DomainErrorCode::BrokenLineage)?;
                *degree = degree.checked_sub(1).ok_or(DomainErrorCode::InvalidValue)?;
                if *degree == 0 {
                    ready.insert(target.clone());
                }
            }
        }
    }
    if order.len() != nodes.len() {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(order)
}

fn port<'a>(ports: &'a [PortType], name: &str) -> DomainResult<&'a PortType> {
    ports
        .binary_search_by(|candidate| candidate.port_name.as_str().cmp(name))
        .map(|index| &ports[index])
        .map_err(|_| DomainErrorCode::BrokenLineage)
}

fn canonical_ports(mut ports: Vec<PortType>) -> DomainResult<Vec<PortType>> {
    ports.sort_by(|left, right| left.port_name.cmp(&right.port_name));
    if ports
        .windows(2)
        .any(|pair| pair[0].port_name == pair[1].port_name)
    {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(ports)
}

fn canonical_symbols(mut values: Vec<String>) -> DomainResult<Vec<String>> {
    for value in &values {
        ensure_symbol(value)?;
    }
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(values)
}

fn ensure_symbol(value: &str) -> DomainResult<()> {
    let is_alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    if value.is_empty()
        || value.len() > 128
        || !value
            .as_bytes()
            .first()
            .is_some_and(|byte| is_alphanumeric(*byte))
        || !value
            .as_bytes()
            .last()
            .is_some_and(|byte| is_alphanumeric(*byte))
        || !value
            .bytes()
            .all(|byte| is_alphanumeric(byte) || b"._-".contains(&byte))
    {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}

fn push_ports(bytes: &mut Vec<u8>, ports: &[PortType]) {
    push_u64(bytes, ports.len() as u64);
    for port in ports {
        push_str(bytes, &port.port_name);
        push_str(bytes, &port.value_type.type_id);
        push_u64(bytes, port.value_type.type_version.get());
        bytes.extend_from_slice(port.value_type.schema_hash.as_bytes());
    }
}

fn push_str(bytes: &mut Vec<u8>, value: &str) {
    push_u64(bytes, value.len() as u64);
    bytes.extend_from_slice(value.as_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_be_bytes());
}
