//! Production native research nodes backed by the frozen fixed-income engine.

use ficant_api::RatesGrpcService;
use ficant_contracts::ficant::core::v1::{
    DecimalValue as ProtoDecimalValue, MarketTime as ProtoMarketTime, UnitRef,
};
use ficant_contracts::ficant::market::v1::SubjectCouponTaxTreatment;
use ficant_contracts::ficant::rates::v1::{
    AnalysisContext, AnalyzeBondRequest, AnalyzeBondResult, ObjectBinding, ResultMetadata,
    RiskSummary, SnapshotBinding, analyze_bond_request,
};
use ficant_domain::DomainErrorCode;
use ficant_domain::analytics::{
    ABI_VERSION, ALGORITHM_ID, ALGORITHM_VERSION, AnalyticsMode, AnalyticsObjectRef,
    BondAnalyticsInput, BondTerms, BusinessDayConvention, CONVENTION_PROFILE, CalendarBinding,
    CalendarRequirement as DomainCalendarRequirement, CouponFrequency, DayCountConvention,
    ENGINE_ID, ENGINE_VERSION, FixedDecimal,
};
use ficant_domain::market::{BondTaxAttributes, IncomeTaxStatus, ValueAddedTaxStatus};
use ficant_domain::primitives::{
    ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef as DomainVersionRef,
};
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
pub const MATERIALIZED_INPUT_PORT: &str = "materialized";
pub const RESULT_PORT: &str = "result";
pub const REQUEST_TYPE_ID: &str = "ficant.rates.v1.analyze-bond-request";
pub const MATERIALIZED_BOND_INPUT_TYPE_ID: &str = "ficant.domain.bond-analytics-input.materialized";
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
const MATERIALIZED_BOND_INPUT_SCHEMA: &[u8] = b"ficant/materialized-bond-analytics-input-schema/v2";
const MATERIALIZED_BOND_INPUT_MAGIC: &[u8] = b"FMBI\0\x02";
const ANALYTICS_IMPLEMENTATION_DOMAIN: &[u8] =
    b"ficant/cgb-bond-analytics-native-node-implementation/v3";
const RISK_SUMMARY_IMPLEMENTATION_DOMAIN: &[u8] =
    b"ficant/cgb-bond-risk-summary-native-node-implementation/v1";

#[must_use]
pub fn analyze_bond_request_type() -> TypedValue {
    message_type(REQUEST_TYPE_ID, REQUEST_PROTOBUF_NAME)
}

/// Returns the private, deterministic Application-to-native materialization type.
///
/// # Panics
///
/// Panics only if the frozen in-binary type identifier or version becomes invalid.
#[must_use]
pub fn materialized_bond_input_type() -> TypedValue {
    TypedValue::new(
        MATERIALIZED_BOND_INPUT_TYPE_ID,
        Version::new(1).expect("frozen materialized input version is positive"),
        ContentHash::digest(MATERIALIZED_BOND_INPUT_SCHEMA),
    )
    .expect("frozen materialized input type identifier is valid")
}

#[must_use]
pub fn analyze_bond_result_type() -> TypedValue {
    message_type(RESULT_TYPE_ID, RESULT_PROTOBUF_NAME)
}

#[must_use]
pub fn risk_summary_type() -> TypedValue {
    message_type(RISK_SUMMARY_TYPE_ID, RISK_SUMMARY_PROTOBUF_NAME)
}

/// Encodes an already verified Application-layer input for the private native port.
///
/// The codec is deliberately local and versioned. It is not a public transport DTO,
/// and it preserves every fact consumed by the numerical engine.
#[must_use]
pub fn encode_materialized_bond_input(
    input: &BondAnalyticsInput,
    coupon_tax_treatment: &SubjectCouponTaxTreatment,
    authority_semantic_hash: &ContentHash,
    metadata: &ResultMetadata,
) -> Vec<u8> {
    let mut bytes = MATERIALIZED_BOND_INPUT_MAGIC.to_vec();
    encode_owner(&mut bytes, input.owner());
    encode_object_ref(&mut bytes, input.bond());
    encode_object_ref(&mut bytes, input.rule_pack());
    encode_object_ref(&mut bytes, input.snapshot());
    push_str(&mut bytes, &input.valuation_at().instant().to_rfc3339());
    push_str(&mut bytes, input.valuation_at().market_timezone());
    push_str(
        &mut bytes,
        &input.valuation_at().local_trading_date().to_string(),
    );
    push_str(&mut bytes, &input.settlement_date().to_string());
    bytes.push(input.calendar_requirement() as u8);
    encode_calendar(&mut bytes, input.calendar());
    encode_terms(&mut bytes, input.terms());
    bytes.push(input.mode() as u8);
    bytes.extend_from_slice(&input.input_value().scaled().to_be_bytes());
    let treatment = deterministic_encode(coupon_tax_treatment);
    bytes.extend_from_slice(&(treatment.len() as u64).to_be_bytes());
    bytes.extend_from_slice(&treatment);
    bytes.extend_from_slice(authority_semantic_hash.as_bytes());
    let metadata = deterministic_encode(metadata);
    bytes.extend_from_slice(&(metadata.len() as u64).to_be_bytes());
    bytes.extend_from_slice(&metadata);
    bytes
}

fn decode_materialized_bond_input(
    bytes: &[u8],
) -> Result<
    (
        BondAnalyticsInput,
        SubjectCouponTaxTreatment,
        ContentHash,
        ResultMetadata,
    ),
    RuntimeError,
> {
    let mut reader = MaterializedReader::new(bytes);
    if reader.take(MATERIALIZED_BOND_INPUT_MAGIC.len())? != MATERIALIZED_BOND_INPUT_MAGIC {
        return Err(invalid());
    }
    let owner = decode_owner(&mut reader)?;
    let bond = decode_object_ref(&mut reader)?;
    let rule_pack = decode_object_ref(&mut reader)?;
    let snapshot = decode_object_ref(&mut reader)?;
    let valuation_at = MarketTime::new(
        reader.string()?.parse().map_err(|_| invalid())?,
        reader.string()?,
        reader.string()?.parse().map_err(|_| invalid())?,
    )
    .map_err(RuntimeError::Domain)?;
    let settlement_date = reader.string()?.parse().map_err(|_| invalid())?;
    let calendar_requirement = match reader.u8()? {
        1 => DomainCalendarRequirement::ReferenceReplay,
        2 => DomainCalendarRequirement::ExactMarket,
        _ => return Err(invalid()),
    };
    let calendar = decode_calendar(&mut reader)?;
    let terms = decode_terms(&mut reader)?;
    let mode = match reader.u8()? {
        1 => AnalyticsMode::YieldIn,
        2 => AnalyticsMode::PriceIn,
        _ => return Err(invalid()),
    };
    let input_value = FixedDecimal::from_scaled(reader.i128()?);
    let treatment_length = usize::try_from(reader.u64()?).map_err(|_| invalid())?;
    let coupon_tax_treatment =
        SubjectCouponTaxTreatment::decode(reader.take(treatment_length)?).map_err(|_| invalid())?;
    let authority_semantic_hash =
        ContentHash::from_bytes(reader.take(32)?).map_err(RuntimeError::Domain)?;
    let metadata_length = usize::try_from(reader.u64()?).map_err(|_| invalid())?;
    let metadata = ResultMetadata::decode(reader.take(metadata_length)?).map_err(|_| invalid())?;
    if !reader.is_empty() {
        return Err(invalid());
    }
    let input = BondAnalyticsInput::new(
        owner,
        bond,
        rule_pack,
        snapshot,
        valuation_at,
        settlement_date,
        calendar_requirement,
        calendar,
        terms,
        mode,
        input_value,
    )
    .map_err(RuntimeError::Domain)?;
    Ok((
        input,
        coupon_tax_treatment,
        authority_semantic_hash,
        metadata,
    ))
}

fn encode_owner(bytes: &mut Vec<u8>, owner: &OwnerRef) {
    push_str(bytes, owner.tenant_id().as_str());
    push_str(bytes, owner.owner_id().as_str());
}

fn decode_owner(reader: &mut MaterializedReader<'_>) -> Result<OwnerRef, RuntimeError> {
    Ok(OwnerRef::new(
        Ulid::new(reader.string()?).map_err(RuntimeError::Domain)?,
        Ulid::new(reader.string()?).map_err(RuntimeError::Domain)?,
    ))
}

fn encode_object_ref(bytes: &mut Vec<u8>, value: &AnalyticsObjectRef) {
    push_str(bytes, value.version_ref().id().as_str());
    bytes.extend_from_slice(&value.version_ref().version().get().to_be_bytes());
    bytes.extend_from_slice(value.content_hash().as_bytes());
}

fn decode_object_ref(
    reader: &mut MaterializedReader<'_>,
) -> Result<AnalyticsObjectRef, RuntimeError> {
    let id = Ulid::new(reader.string()?).map_err(RuntimeError::Domain)?;
    let version = Version::new(reader.u64()?).map_err(RuntimeError::Domain)?;
    let content_hash = ContentHash::from_bytes(reader.take(32)?).map_err(RuntimeError::Domain)?;
    Ok(AnalyticsObjectRef::new(
        DomainVersionRef::new(id, version),
        content_hash,
    ))
}

fn encode_calendar(bytes: &mut Vec<u8>, value: &CalendarBinding) {
    push_str(bytes, value.id());
    bytes.extend_from_slice(&value.version().get().to_be_bytes());
    bytes.extend_from_slice(value.content_hash().as_bytes());
    push_str(bytes, &value.coverage_start().to_string());
    push_str(bytes, &value.coverage_end().to_string());
    encode_dates(bytes, value.non_business_days());
    encode_dates(bytes, value.work_weekends());
}

fn decode_calendar(reader: &mut MaterializedReader<'_>) -> Result<CalendarBinding, RuntimeError> {
    let id = reader.string()?;
    let version = Version::new(reader.u64()?).map_err(RuntimeError::Domain)?;
    let content_hash = ContentHash::from_bytes(reader.take(32)?).map_err(RuntimeError::Domain)?;
    let coverage_start = reader.string()?.parse().map_err(|_| invalid())?;
    let coverage_end = reader.string()?.parse().map_err(|_| invalid())?;
    let non_business_days = reader.dates()?;
    let work_weekends = reader.dates()?;
    CalendarBinding::new(
        id,
        version,
        content_hash,
        coverage_start,
        coverage_end,
        non_business_days,
        work_weekends,
    )
    .map_err(RuntimeError::Domain)
}

fn encode_dates(bytes: &mut Vec<u8>, dates: &[impl ToString]) {
    bytes.extend_from_slice(&(dates.len() as u64).to_be_bytes());
    for date in dates {
        push_str(bytes, &date.to_string());
    }
}

fn encode_terms(bytes: &mut Vec<u8>, value: &BondTerms) {
    push_str(bytes, &value.first_issue_date().to_string());
    push_str(bytes, &value.current_issue_date().to_string());
    push_str(bytes, &value.maturity_date().to_string());
    bytes.extend_from_slice(&(value.frequency() as u32).to_be_bytes());
    bytes.extend_from_slice(&(value.day_count() as u32).to_be_bytes());
    bytes.extend_from_slice(&(value.business_day() as u32).to_be_bytes());
    for decimal in [
        value.coupon_rate(),
        value.face_amount(),
        value.cumulative_issued_amount(),
    ] {
        bytes.extend_from_slice(&decimal.scaled().to_be_bytes());
    }
    match value.tax_attributes() {
        Some(attributes) => {
            bytes.push(1);
            bytes.push(match attributes.value_added_tax_status() {
                ValueAddedTaxStatus::Exempt => 1,
                ValueAddedTaxStatus::Taxable => 2,
            });
            bytes.push(match attributes.income_tax_status() {
                IncomeTaxStatus::Exempt => 1,
                IncomeTaxStatus::Taxable => 2,
            });
        }
        None => bytes.push(0),
    }
}

fn decode_terms(reader: &mut MaterializedReader<'_>) -> Result<BondTerms, RuntimeError> {
    let first_issue_date = reader.string()?.parse().map_err(|_| invalid())?;
    let current_issue_date = reader.string()?.parse().map_err(|_| invalid())?;
    let maturity_date = reader.string()?.parse().map_err(|_| invalid())?;
    let frequency = match reader.u32()? {
        1 => CouponFrequency::Annual,
        2 => CouponFrequency::Semiannual,
        _ => return Err(invalid()),
    };
    let day_count = match reader.u32()? {
        1 => DayCountConvention::ActActBondIsma,
        _ => return Err(invalid()),
    };
    let business_day = match reader.u32()? {
        1 => BusinessDayConvention::Following,
        _ => return Err(invalid()),
    };
    let coupon_rate = FixedDecimal::from_scaled(reader.i128()?);
    let face_amount = FixedDecimal::from_scaled(reader.i128()?);
    let cumulative_issued_amount = FixedDecimal::from_scaled(reader.i128()?);
    let tax_attributes = match reader.u8()? {
        0 => None,
        1 => Some(BondTaxAttributes::new(
            decode_vat_status(reader.u8()?)?,
            decode_income_tax_status(reader.u8()?)?,
        )),
        _ => return Err(invalid()),
    };
    match tax_attributes {
        Some(attributes) => BondTerms::with_issuance(
            first_issue_date,
            current_issue_date,
            maturity_date,
            frequency,
            day_count,
            business_day,
            coupon_rate,
            face_amount,
            cumulative_issued_amount,
            attributes,
        ),
        None if current_issue_date == first_issue_date
            && cumulative_issued_amount == face_amount =>
        {
            BondTerms::new(
                first_issue_date,
                maturity_date,
                frequency,
                day_count,
                business_day,
                coupon_rate,
                face_amount,
            )
        }
        None => Err(DomainErrorCode::InvalidValue),
    }
    .map_err(RuntimeError::Domain)
}

fn decode_vat_status(value: u8) -> Result<ValueAddedTaxStatus, RuntimeError> {
    match value {
        1 => Ok(ValueAddedTaxStatus::Exempt),
        2 => Ok(ValueAddedTaxStatus::Taxable),
        _ => Err(invalid()),
    }
}

fn decode_income_tax_status(value: u8) -> Result<IncomeTaxStatus, RuntimeError> {
    match value {
        1 => Ok(IncomeTaxStatus::Exempt),
        2 => Ok(IncomeTaxStatus::Taxable),
        _ => Err(invalid()),
    }
}

struct MaterializedReader<'a> {
    remaining: &'a [u8],
}

impl<'a> MaterializedReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], RuntimeError> {
        if self.remaining.len() < length {
            return Err(invalid());
        }
        let (value, remaining) = self.remaining.split_at(length);
        self.remaining = remaining;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, RuntimeError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, RuntimeError> {
        let bytes: [u8; 4] = self.take(4)?.try_into().map_err(|_| invalid())?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, RuntimeError> {
        let bytes: [u8; 8] = self.take(8)?.try_into().map_err(|_| invalid())?;
        Ok(u64::from_be_bytes(bytes))
    }

    fn i128(&mut self) -> Result<i128, RuntimeError> {
        let bytes: [u8; 16] = self.take(16)?.try_into().map_err(|_| invalid())?;
        Ok(i128::from_be_bytes(bytes))
    }

    fn string(&mut self) -> Result<String, RuntimeError> {
        let length = usize::try_from(self.u64()?).map_err(|_| invalid())?;
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| invalid())
    }

    fn dates<T>(&mut self) -> Result<Vec<T>, RuntimeError>
    where
        T: std::str::FromStr,
    {
        let count = usize::try_from(self.u64()?).map_err(|_| invalid())?;
        if count > 100_000 || count > self.remaining.len() / 8 {
            return Err(invalid());
        }
        (0..count)
            .map(|_| self.string()?.parse().map_err(|_| invalid()))
            .collect()
    }

    const fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }
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
        contract_version: Version::new(2)?,
        input_types: vec![
            PortType::new(REQUEST_PORT, analyze_bond_request_type())?,
            PortType::new(MATERIALIZED_INPUT_PORT, materialized_bond_input_type())?,
        ],
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
            "materialized_business_input_exact".to_owned(),
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
            || request.inputs().len() != 2
        {
            return Err(invalid());
        }
        let public_input = request
            .inputs()
            .iter()
            .find(|input| input.port_name() == REQUEST_PORT)
            .ok_or_else(invalid)?;
        let materialized_input = request
            .inputs()
            .iter()
            .find(|input| input.port_name() == MATERIALIZED_INPUT_PORT)
            .ok_or_else(invalid)?;
        if public_input.value_type() != &analyze_bond_request_type()
            || materialized_input.value_type() != &materialized_bond_input_type()
        {
            return Err(invalid());
        }
        validate_external_lineage(request.identity(), public_input)?;
        validate_external_lineage(request.identity(), materialized_input)?;
        let protobuf = AnalyzeBondRequest::decode(public_input.payload()).map_err(|_| invalid())?;
        let (materialized, coupon_tax_treatment, authority_semantic_hash, metadata) =
            decode_materialized_bond_input(materialized_input.payload())?;
        validate_request_against_materialized(request.identity(), &protobuf, &materialized)?;
        let result = RatesGrpcService::execute_materialized_v2_bond_request(
            &NativeBondAnalyticsEngine,
            &protobuf,
            &materialized,
            &coupon_tax_treatment,
            authority_semantic_hash.as_bytes(),
            metadata,
        )
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
    request: &AnalyzeBondRequest,
) -> Result<(), RuntimeError> {
    let snapshot_hash = request
        .data_snapshot
        .as_ref()
        .and_then(|binding| binding.content_hash.as_ref())
        .and_then(|value| ContentHash::from_bytes(&value.value).ok())
        .ok_or_else(invalid)?;
    if &snapshot_hash != identity.data_snapshot_hash() {
        return Err(hash_mismatch());
    }
    let rule = request.tax_rule_pack.as_ref().ok_or_else(invalid)?;
    let rule_ref = rule.object.as_ref().ok_or_else(invalid)?;
    let rule_id = rule_ref
        .id
        .as_ref()
        .map(|value| value.value.as_str())
        .ok_or_else(invalid)?;
    let version = Version::new(rule_ref.version).map_err(RuntimeError::Domain)?;
    let content_hash = rule
        .content_hash
        .as_ref()
        .and_then(|value| ContentHash::from_bytes(&value.value).ok())
        .ok_or_else(invalid)?;
    let exact_rule = identity.rule_pack_bindings().iter().any(|binding| {
        binding.rule_pack_id == rule_id
            && binding.version == version
            && binding.content_hash == content_hash
    });
    if !exact_rule {
        return Err(hash_mismatch());
    }
    Ok(())
}

fn validate_request_against_materialized(
    identity: &ReproducibilityIdentity,
    request: &AnalyzeBondRequest,
    materialized: &BondAnalyticsInput,
) -> Result<(), RuntimeError> {
    validate_business_lineage(identity, request)?;
    let context = request.context.as_ref().ok_or_else(invalid)?;
    validate_context(context, request.valuation_at.as_ref().ok_or_else(invalid)?)?;
    if !owner_matches(context, materialized.owner())
        || !object_matches(
            request.bond.as_ref().ok_or_else(invalid)?,
            materialized.bond(),
        )
        || !calendar_matches(
            request.calendar.as_ref().ok_or_else(invalid)?,
            materialized.calendar(),
        )
        || !snapshot_matches(
            request.data_snapshot.as_ref().ok_or_else(invalid)?,
            materialized.snapshot(),
        )
        || !object_matches(
            request.tax_rule_pack.as_ref().ok_or_else(invalid)?,
            materialized.rule_pack(),
        )
        || !market_time_matches(
            request.valuation_at.as_ref().ok_or_else(invalid)?,
            materialized.valuation_at(),
        )
        || request.settlement_date != materialized.settlement_date().to_string()
        || request.calendar_requirement != materialized.calendar_requirement() as i32
    {
        return Err(hash_mismatch());
    }
    let units = context.units.as_ref().ok_or_else(invalid)?;
    let (mode, value, unit) = match request.input.as_ref().ok_or_else(invalid)? {
        analyze_bond_request::Input::YieldToMaturity(value) => (
            AnalyticsMode::YieldIn,
            value,
            units.rate.as_ref().ok_or_else(invalid)?,
        ),
        analyze_bond_request::Input::CleanPrice(value) => (
            AnalyticsMode::PriceIn,
            value,
            units.price_per_100.as_ref().ok_or_else(invalid)?,
        ),
    };
    if materialized.mode() != mode
        || decimal_scaled(value, unit)? != materialized.input_value().scaled()
    {
        return Err(hash_mismatch());
    }
    Ok(())
}

fn validate_context(
    context: &AnalysisContext,
    valuation_at: &ProtoMarketTime,
) -> Result<(), RuntimeError> {
    let algorithm = context.algorithm.as_ref().ok_or_else(invalid)?;
    if algorithm.algorithm_id != ALGORITHM_ID
        || algorithm.algorithm_version != ALGORITHM_VERSION
        || algorithm.convention_profile != CONVENTION_PROFILE
        || algorithm.abi_version != ABI_VERSION
    {
        return Err(invalid());
    }
    let subject = context.subject_ref.as_ref().ok_or_else(invalid)?;
    validate_proto_version_ref(subject)?;
    let units = context.units.as_ref().ok_or_else(invalid)?;
    for unit in [
        units.currency_amount.as_ref(),
        units.price_per_100.as_ref(),
        units.rate.as_ref(),
        units.years.as_ref(),
        units.years_squared.as_ref(),
        units.dv01_per_100.as_ref(),
        units.dv01.as_ref(),
        units.dimensionless.as_ref(),
        units.contract_count.as_ref(),
    ] {
        validate_unit(unit.ok_or_else(invalid)?)?;
    }
    let knowledge = context.knowledge_at.as_ref().ok_or_else(invalid)?;
    validate_proto_time(valuation_at)?;
    validate_proto_time(knowledge)?;
    if knowledge.market_timezone != valuation_at.market_timezone
        || timestamp_key(knowledge)? < timestamp_key(valuation_at)?
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_unit(unit: &UnitRef) -> Result<(), RuntimeError> {
    let id = unit.unit_id.as_ref().ok_or_else(invalid)?;
    Ulid::new(id.value.clone()).map_err(RuntimeError::Domain)?;
    Version::new(unit.version).map_err(RuntimeError::Domain)?;
    Ok(())
}

fn validate_proto_version_ref(
    value: &ficant_contracts::ficant::core::v1::VersionRef,
) -> Result<(), RuntimeError> {
    let id = value.id.as_ref().ok_or_else(invalid)?;
    Ulid::new(id.value.clone()).map_err(RuntimeError::Domain)?;
    Version::new(value.version).map_err(RuntimeError::Domain)?;
    Ok(())
}

fn validate_proto_time(value: &ProtoMarketTime) -> Result<(), RuntimeError> {
    let instant = value.instant.as_ref().ok_or_else(invalid)?;
    if !(0..1_000_000_000).contains(&instant.nanos)
        || value.market_timezone.trim().is_empty()
        || value.market_timezone != value.market_timezone.trim()
        || value.local_trading_date.len() != 10
    {
        return Err(invalid());
    }
    Ok(())
}

fn timestamp_key(value: &ProtoMarketTime) -> Result<(i64, i32), RuntimeError> {
    let instant = value.instant.as_ref().ok_or_else(invalid)?;
    Ok((instant.seconds, instant.nanos))
}

fn owner_matches(context: &AnalysisContext, expected: &OwnerRef) -> bool {
    context.owner.as_ref().is_some_and(|owner| {
        owner
            .tenant_id
            .as_ref()
            .is_some_and(|id| id.value == expected.tenant_id().as_str())
            && owner
                .owner_id
                .as_ref()
                .is_some_and(|id| id.value == expected.owner_id().as_str())
    })
}

fn object_matches(binding: &ObjectBinding, expected: &AnalyticsObjectRef) -> bool {
    binding.object.as_ref().is_some_and(|object| {
        object
            .id
            .as_ref()
            .is_some_and(|id| id.value == expected.version_ref().id().as_str())
            && object.version == expected.version_ref().version().get()
    }) && proto_hash_matches(binding.content_hash.as_ref(), expected.content_hash())
}

fn calendar_matches(binding: &ObjectBinding, expected: &CalendarBinding) -> bool {
    binding.object.as_ref().is_some_and(|object| {
        object
            .id
            .as_ref()
            .is_some_and(|id| id.value == expected.id())
            && object.version == expected.version().get()
    }) && proto_hash_matches(binding.content_hash.as_ref(), expected.content_hash())
}

fn snapshot_matches(binding: &SnapshotBinding, expected: &AnalyticsObjectRef) -> bool {
    binding
        .snapshot_id
        .as_ref()
        .is_some_and(|id| id.value == expected.version_ref().id().as_str())
        && proto_hash_matches(binding.content_hash.as_ref(), expected.content_hash())
}

fn proto_hash_matches(
    actual: Option<&ficant_contracts::ficant::core::v1::Sha256>,
    expected: &ContentHash,
) -> bool {
    actual.is_some_and(|value| value.value.as_slice() == expected.as_bytes())
}

fn market_time_matches(actual: &ProtoMarketTime, expected: &MarketTime) -> bool {
    actual.instant.as_ref().is_some_and(|instant| {
        instant.seconds == expected.instant().timestamp()
            && instant.nanos == expected.instant().timestamp_subsec_nanos().cast_signed()
    }) && actual.market_timezone == expected.market_timezone()
        && actual.local_trading_date == expected.local_trading_date().to_string()
}

fn decimal_scaled(
    value: &ProtoDecimalValue,
    expected_unit: &UnitRef,
) -> Result<i128, RuntimeError> {
    const SCALE: u32 = ficant_domain::analytics::DECIMAL_SCALE;

    if value.unit.as_ref() != Some(expected_unit) || !normalized_coefficient(&value.coefficient) {
        return Err(invalid());
    }
    let coefficient = value.coefficient.parse::<i128>().map_err(|_| invalid())?;
    if value.scale <= SCALE {
        coefficient
            .checked_mul(pow10(SCALE - value.scale)?)
            .ok_or_else(invalid)
    } else {
        let divisor = pow10(value.scale - SCALE)?;
        if coefficient % divisor != 0 {
            return Err(invalid());
        }
        Ok(coefficient / divisor)
    }
}

fn normalized_coefficient(value: &str) -> bool {
    if value == "0" {
        return true;
    }
    let digits = value.strip_prefix('-').unwrap_or(value);
    !digits.is_empty()
        && !digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn pow10(exponent: u32) -> Result<i128, RuntimeError> {
    if exponent > 38 {
        return Err(invalid());
    }
    (0..exponent).try_fold(1_i128, |value, _| value.checked_mul(10).ok_or_else(invalid))
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
        MATERIALIZED_BOND_INPUT_TYPE_ID,
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
    bytes.extend_from_slice(materialized_bond_input_type().schema_hash().as_bytes());
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
