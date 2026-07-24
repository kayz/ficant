//! Production native research nodes backed by the frozen fixed-income engine.

use ficant_api::{execute_parsed_bond_request, parse_analyze_bond_request};
use ficant_contracts::ficant::rates::v1::{AnalyzeBondRequest, AnalyzeBondResult, RiskSummary};
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{
    ABI_VERSION, ALGORITHM_ID, ALGORITHM_VERSION, CONVENTION_PROFILE, ENGINE_ID, ENGINE_VERSION,
};
use ficant_domain::primitives::{ContentHash, Ulid, Version};
use ficant_domain::research::{
    DeterminismClass, FilesystemPermission, NodePermissions, PortType, ResearchNode,
    ResearchNodeContract, ResearchNodeContractInput, ResourceLimits, TypedValue,
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
pub const RISK_INPUT_PORT: &str = "analysis";
pub const RISK_OUTPUT_PORT: &str = "risk-summary";
pub const RISK_SUMMARY_TYPE_ID: &str = "ficant.rates.v1.risk-summary";
pub const RISK_SUMMARY_CONTRACT_ID: &str = "ficant.native.cgb-bond-risk-summary";

const ANALYTICS_PROTO: &[u8] =
    include_bytes!("../../../interface/proto/ficant/rates/v1/analytics.proto");
const REQUEST_PROTOBUF_NAME: &str = "ficant.rates.v1.AnalyzeBondRequest";
const RESULT_PROTOBUF_NAME: &str = "ficant.rates.v1.AnalyzeBondResult";
const RISK_SUMMARY_PROTOBUF_NAME: &str = "ficant.rates.v1.RiskSummary";
const ANALYTICS_IMPLEMENTATION_DOMAIN: &[u8] =
    b"ficant/cgb-bond-analytics-native-node-implementation/v2";
const RISK_SUMMARY_IMPLEMENTATION_DOMAIN: &[u8] =
    b"ficant/cgb-bond-risk-summary-native-node-implementation/v1";

#[must_use]
pub fn analyze_bond_request_type() -> TypedValue {
    message_type(REQUEST_TYPE_ID, REQUEST_PROTOBUF_NAME)
}

#[must_use]
pub fn analyze_bond_result_type() -> TypedValue {
    message_type(RESULT_TYPE_ID, RESULT_PROTOBUF_NAME)
}

#[must_use]
pub fn risk_summary_type() -> TypedValue {
    message_type(RISK_SUMMARY_TYPE_ID, RISK_SUMMARY_PROTOBUF_NAME)
}

/// Returns the build-generated digest of every source that can change a
/// trusted native node's contract or behavior.
#[must_use]
pub fn native_node_source_digest() -> ContentHash {
    decode_build_digest(env!("FICANT_NATIVE_NODES_SOURCE_DIGEST"))
}

/// Returns the compiled relevant-source digest in the deployment attestation format.
#[must_use]
pub fn native_node_source_digest_attestation() -> String {
    format!(
        "sha256:{}",
        encode_hex(native_node_source_digest().as_bytes())
    )
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

/// Returns the exact downstream risk projection contract.
///
/// # Errors
///
/// Returns a domain validation error only if frozen contract constants are internally invalid.
pub fn cgb_bond_risk_summary_contract() -> Result<ResearchNodeContract, DomainErrorCode> {
    ResearchNodeContract::new(ResearchNodeContractInput {
        contract_id: RISK_SUMMARY_CONTRACT_ID.to_owned(),
        contract_version: Version::new(1)?,
        input_types: vec![PortType::new(RISK_INPUT_PORT, analyze_bond_result_type())?],
        output_types: vec![PortType::new(RISK_OUTPUT_PORT, risk_summary_type())?],
        state_schema: ContentHash::digest(b"ficant.native.stateless.v1"),
        parameter_schema: ContentHash::digest(b"ficant.native.no-parameters.v1"),
        determinism_class: DeterminismClass::Deterministic,
        permissions: NodePermissions {
            network: false,
            database: false,
            filesystem: FilesystemPermission::None,
        },
        resource_limits: ResourceLimits::new(1, 128, 10)?,
        required_invariants: vec![
            "analyze_bond_result_contract_exact".to_owned(),
            "no_numerical_recomputation".to_owned(),
            "source_metadata_preserved".to_owned(),
            "upstream_artifact_lineage_exact".to_owned(),
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
        let implementation_digest = implementation_digest(
            ANALYTICS_IMPLEMENTATION_DOMAIN,
            &contract,
            &native_node_source_digest(),
        );
        Ok(Self {
            node_id,
            contract_digest: contract.digest().clone(),
            implementation_digest,
        })
    }
}

#[derive(Clone, Debug)]
pub struct CgbBondRiskSummaryNativeNode {
    node_id: Ulid,
    contract_digest: ContentHash,
    implementation_digest: ContentHash,
}

impl CgbBondRiskSummaryNativeNode {
    /// Binds the deterministic downstream risk projection to one graph node.
    ///
    /// # Errors
    ///
    /// Returns a stable runtime error if the frozen production contract is invalid.
    pub fn new(node_id: Ulid) -> Result<Self, RuntimeError> {
        let contract = cgb_bond_risk_summary_contract().map_err(RuntimeError::Domain)?;
        let implementation_digest = implementation_digest(
            RISK_SUMMARY_IMPLEMENTATION_DOMAIN,
            &contract,
            &native_node_source_digest(),
        );
        Ok(Self {
            node_id,
            contract_digest: contract.digest().clone(),
            implementation_digest,
        })
    }
}

impl NativeNode for CgbBondRiskSummaryNativeNode {
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
        if input.port_name() != RISK_INPUT_PORT || input.value_type() != &analyze_bond_result_type()
        {
            return Err(invalid());
        }
        let result = AnalyzeBondResult::decode(input.payload()).map_err(|_| invalid())?;
        let measures = result.measures.ok_or_else(invalid)?;
        let summary = RiskSummary {
            modified_duration: Some(measures.modified_duration.ok_or_else(invalid)?),
            convexity: Some(measures.convexity.ok_or_else(invalid)?),
            dv01: Some(measures.dv01.ok_or_else(invalid)?),
            source_metadata: Some(result.metadata.ok_or_else(invalid)?),
        };
        Ok(vec![NativePortValue::new(
            RISK_OUTPUT_PORT,
            risk_summary_type(),
            deterministic_encode(&summary),
        )?])
    }
}

#[derive(Clone, Debug)]
pub enum TrustedNativeNode {
    BondAnalytics(CgbBondAnalyticsNativeNode),
    BondRiskSummary(CgbBondRiskSummaryNativeNode),
}

impl NativeNode for TrustedNativeNode {
    fn node_id(&self) -> &Ulid {
        match self {
            Self::BondAnalytics(node) => node.node_id(),
            Self::BondRiskSummary(node) => node.node_id(),
        }
    }

    fn implementation_digest(&self) -> &ContentHash {
        match self {
            Self::BondAnalytics(node) => node.implementation_digest(),
            Self::BondRiskSummary(node) => node.implementation_digest(),
        }
    }

    fn execute(
        &self,
        request: &NativeNodeRequest<'_>,
    ) -> Result<Vec<NativePortValue>, RuntimeError> {
        match self {
            Self::BondAnalytics(node) => node.execute(request),
            Self::BondRiskSummary(node) => node.execute(request),
        }
    }
}

/// Resolves a persisted graph node through the fail-closed trusted registry.
///
/// # Errors
///
/// Rejects unknown identifiers and any contract bytes that differ from the
/// exact built-in contract registered under that identifier.
pub fn trusted_native_node(node: &ResearchNode) -> Result<TrustedNativeNode, RuntimeError> {
    let candidate = match node.contract().contract_id() {
        NODE_CONTRACT_ID => TrustedNativeNode::BondAnalytics(CgbBondAnalyticsNativeNode::new(
            node.node_id().clone(),
        )?),
        RISK_SUMMARY_CONTRACT_ID => TrustedNativeNode::BondRiskSummary(
            CgbBondRiskSummaryNativeNode::new(node.node_id().clone())?,
        ),
        _ => return Err(invalid()),
    };
    let expected_contract = match &candidate {
        TrustedNativeNode::BondAnalytics(_) => cgb_bond_analytics_contract(),
        TrustedNativeNode::BondRiskSummary(_) => cgb_bond_risk_summary_contract(),
    }
    .map_err(RuntimeError::Domain)?;
    if node.contract().digest() != expected_contract.digest() {
        return Err(invalid());
    }
    Ok(candidate)
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

fn implementation_digest(
    domain: &[u8],
    contract: &ResearchNodeContract,
    source_digest: &ContentHash,
) -> ContentHash {
    let mut bytes = domain.to_vec();
    bytes.extend_from_slice(contract.digest().as_bytes());
    bytes.extend_from_slice(source_digest.as_bytes());
    for value in [
        env!("CARGO_PKG_VERSION"),
        ENGINE_ID,
        ENGINE_VERSION,
        ALGORITHM_ID,
        CONVENTION_PROFILE,
        REQUEST_TYPE_ID,
        RESULT_TYPE_ID,
        RISK_SUMMARY_TYPE_ID,
        REQUEST_PROTOBUF_NAME,
        RESULT_PROTOBUF_NAME,
        RISK_SUMMARY_PROTOBUF_NAME,
    ] {
        push_str(&mut bytes, value);
    }
    bytes.extend_from_slice(&ABI_VERSION.to_be_bytes());
    bytes.extend_from_slice(&ALGORITHM_VERSION.to_be_bytes());
    bytes.extend_from_slice(analyze_bond_request_type().schema_hash().as_bytes());
    bytes.extend_from_slice(analyze_bond_result_type().schema_hash().as_bytes());
    ContentHash::digest(&bytes)
}

fn deterministic_encode(message: &impl Message) -> Vec<u8> {
    message.encode_to_vec()
}

fn decode_build_digest(value: &str) -> ContentHash {
    assert_eq!(value.len(), 64, "build-generated source digest is SHA-256");
    let mut bytes = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        bytes[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    ContentHash::from_bytes(&bytes).expect("build-generated SHA-256 digest is valid")
}

fn encode_hex(value: &[u8]) -> String {
    use std::fmt::Write as _;

    value.iter().fold(
        String::with_capacity(value.len() * 2),
        |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        },
    )
}

const fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => panic!("build-generated source digest is lowercase hexadecimal"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implementation_digest_binds_domain_contract_and_generated_source() {
        let contract = cgb_bond_analytics_contract().unwrap();
        let source = native_node_source_digest();
        let actual = implementation_digest(ANALYTICS_IMPLEMENTATION_DOMAIN, &contract, &source);
        assert_ne!(
            actual,
            implementation_digest(
                b"ficant/cgb-bond-analytics-native-node-implementation/drift",
                &contract,
                &source
            )
        );
        assert_ne!(
            actual,
            implementation_digest(
                ANALYTICS_IMPLEMENTATION_DOMAIN,
                &contract,
                &ContentHash::digest(b"changed-source")
            )
        );
    }
}
