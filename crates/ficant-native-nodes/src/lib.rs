//! Production native research nodes backed by the frozen fixed-income engine.

use ficant_api::{execute_parsed_bond_request, parse_analyze_bond_request};
use ficant_contracts::ficant::rates::v1::{AnalyzeBondRequest, AnalyzeBondResult};
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{
    ABI_VERSION, ALGORITHM_ID, ALGORITHM_VERSION, CONVENTION_PROFILE, ENGINE_ID, ENGINE_VERSION,
};
use ficant_domain::primitives::{ContentHash, Ulid, Version};
use ficant_domain::research::{
    DeterminismClass, FilesystemPermission, NodePermissions, PortType, ResearchNodeContract,
    ResearchNodeContractInput, ResourceLimits, TypedValue,
};
use ficant_fixed_income_native::NativeBondAnalyticsEngine;
use ficant_runtime::{
    NativeNode, NativeNodeRequest, NativePortValue, ReproducibilityIdentity, RuntimeError,
};
use prost::Message;

pub const REQUEST_PORT: &str = "request";
pub const RESULT_PORT: &str = "result";
pub const REQUEST_TYPE_ID: &str = "ficant.rates.v1.analyze-bond-request";
pub const RESULT_TYPE_ID: &str = "ficant.rates.v1.analyze-bond-result";
pub const NODE_CONTRACT_ID: &str = "ficant.native.cgb-bond-analytics";
pub const NODE_SOURCE_VERSION: &str = "ficant-native-nodes/cgb-bond-analytics/v1";

const ANALYTICS_PROTO: &[u8] =
    include_bytes!("../../../interface/proto/ficant/rates/v1/analytics.proto");
const REQUEST_PROTOBUF_NAME: &str = "ficant.rates.v1.AnalyzeBondRequest";
const RESULT_PROTOBUF_NAME: &str = "ficant.rates.v1.AnalyzeBondResult";

#[must_use]
pub fn analyze_bond_request_type() -> TypedValue {
    message_type(REQUEST_TYPE_ID, REQUEST_PROTOBUF_NAME)
}

#[must_use]
pub fn analyze_bond_result_type() -> TypedValue {
    message_type(RESULT_TYPE_ID, RESULT_PROTOBUF_NAME)
}

/// Returns the exact production contract accepted by `CgbBondAnalyticsNativeNode`.
///
/// # Errors
///
/// Returns a domain validation error only if frozen contract constants are internally invalid.
pub fn cgb_bond_analytics_contract() -> Result<ResearchNodeContract, DomainErrorCode> {
    ResearchNodeContract::new(ResearchNodeContractInput {
        contract_id: NODE_CONTRACT_ID.to_owned(),
        contract_version: Version::new(1)?,
        input_types: vec![PortType::new(REQUEST_PORT, analyze_bond_request_type())?],
        output_types: vec![PortType::new(RESULT_PORT, analyze_bond_result_type())?],
        state_schema: ContentHash::digest(b"ficant.native.stateless.v1"),
        parameter_schema: ContentHash::digest(b"ficant.native.no-parameters.v1"),
        determinism_class: DeterminismClass::Deterministic,
        permissions: NodePermissions {
            network: false,
            database: false,
            filesystem: FilesystemPermission::None,
        },
        resource_limits: ResourceLimits::new(1, 256, 30)?,
        required_invariants: vec![
            "external_input_hash_exact".to_owned(),
            "no_external_side_effects".to_owned(),
            "protobuf_contract_exact".to_owned(),
            "reproducibility_lineage_exact".to_owned(),
        ],
    })
}

#[derive(Clone, Debug)]
pub struct CgbBondAnalyticsNativeNode {
    node_id: Ulid,
    contract_digest: ContentHash,
    implementation_digest: ContentHash,
}

impl CgbBondAnalyticsNativeNode {
    /// Binds the immutable production implementation to one graph node identity.
    ///
    /// # Errors
    ///
    /// Returns a stable runtime error if the frozen production contract is invalid.
    pub fn new(node_id: Ulid) -> Result<Self, RuntimeError> {
        let contract = cgb_bond_analytics_contract().map_err(RuntimeError::Domain)?;
        let implementation_digest = implementation_digest(&contract);
        Ok(Self {
            node_id,
            contract_digest: contract.digest().clone(),
            implementation_digest,
        })
    }
}

impl NativeNode for CgbBondAnalyticsNativeNode {
    fn node_id(&self) -> &Ulid {
        &self.node_id
    }

    fn implementation_digest(&self) -> &ContentHash {
        &self.implementation_digest
    }

    fn execute(
        &self,
        request: &NativeNodeRequest<'_>,
    ) -> Result<Vec<NativePortValue>, RuntimeError> {
        if request.node().contract().digest() != &self.contract_digest
            || request.inputs().len() != 1
        {
            return Err(invalid());
        }
        let input = &request.inputs()[0];
        if input.port_name() != REQUEST_PORT || input.value_type() != &analyze_bond_request_type() {
            return Err(invalid());
        }
        validate_external_lineage(request.identity(), input)?;
        let protobuf = AnalyzeBondRequest::decode(input.payload()).map_err(|_| invalid())?;
        let parsed = parse_analyze_bond_request(&protobuf).map_err(|_| invalid())?;
        validate_business_lineage(request.identity(), &parsed)?;
        let result = execute_parsed_bond_request(&NativeBondAnalyticsEngine, &parsed)
            .map_err(|_| invalid())?;
        Ok(vec![NativePortValue::new(
            RESULT_PORT,
            analyze_bond_result_type(),
            deterministic_encode(&result),
        )?])
    }
}

fn validate_external_lineage(
    identity: &ReproducibilityIdentity,
    input: &NativePortValue,
) -> Result<(), RuntimeError> {
    let matches = identity
        .external_inputs()
        .iter()
        .filter(|binding| {
            binding.value_type() == input.value_type()
                && binding.content_hash() == input.content_hash()
        })
        .count();
    if matches != 1 {
        return Err(hash_mismatch());
    }
    Ok(())
}

fn validate_business_lineage(
    identity: &ReproducibilityIdentity,
    parsed: &ficant_api::ParsedBondAnalyticsRequest,
) -> Result<(), RuntimeError> {
    let input = parsed.input();
    if input.snapshot().content_hash() != identity.data_snapshot_hash() {
        return Err(hash_mismatch());
    }
    let rule = input.rule_pack();
    let exact_rule = identity.rule_pack_bindings().iter().any(|binding| {
        binding.rule_pack_id == rule.version_ref().id().as_str()
            && binding.version == rule.version_ref().version()
            && binding.content_hash == *rule.content_hash()
    });
    if !exact_rule {
        return Err(hash_mismatch());
    }
    Ok(())
}

fn message_type(type_id: &str, protobuf_name: &str) -> TypedValue {
    let mut schema = b"ficant/protobuf-message-schema/v1".to_vec();
    push_str(&mut schema, protobuf_name);
    schema.extend_from_slice(ANALYTICS_PROTO);
    TypedValue::new(
        type_id,
        Version::new(1).expect("frozen protobuf type version is positive"),
        ContentHash::digest(&schema),
    )
    .expect("frozen protobuf type identifier is valid")
}

fn implementation_digest(contract: &ResearchNodeContract) -> ContentHash {
    let mut bytes = b"ficant/cgb-bond-analytics-native-node-implementation/v1".to_vec();
    bytes.extend_from_slice(contract.digest().as_bytes());
    for value in [
        NODE_SOURCE_VERSION,
        env!("CARGO_PKG_VERSION"),
        ENGINE_ID,
        ENGINE_VERSION,
        ALGORITHM_ID,
        CONVENTION_PROFILE,
        REQUEST_TYPE_ID,
        RESULT_TYPE_ID,
        REQUEST_PROTOBUF_NAME,
        RESULT_PROTOBUF_NAME,
    ] {
        push_str(&mut bytes, value);
    }
    bytes.extend_from_slice(&ABI_VERSION.to_be_bytes());
    bytes.extend_from_slice(&ALGORITHM_VERSION.to_be_bytes());
    bytes.extend_from_slice(analyze_bond_request_type().schema_hash().as_bytes());
    bytes.extend_from_slice(analyze_bond_result_type().schema_hash().as_bytes());
    ContentHash::digest(&bytes)
}

fn deterministic_encode(message: &AnalyzeBondResult) -> Vec<u8> {
    message.encode_to_vec()
}

fn push_str(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

fn invalid() -> RuntimeError {
    RuntimeError::Domain(DomainErrorCode::InvalidValue)
}

fn hash_mismatch() -> RuntimeError {
    RuntimeError::Domain(DomainErrorCode::ContentHashMismatch)
}
