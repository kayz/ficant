use crate::primitives::{ContentHash, DecimalValue, OwnerRef, UnitRef, VersionRef};
use crate::{ContentAddressed, DomainErrorCode, DomainResult};

/// Immutable sensitivity direction belonging to a global Factor definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensitivityDirection {
    Central,
    Up,
    Down,
}

/// Whether a future calculation rebuilds its curve after applying the factor bump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveRebuildPolicy {
    Rebuild,
    Hold,
}

/// Whether a future calculation includes a second-order term.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecondOrderPolicy {
    Include,
    Exclude,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SensitivityConvention {
    bump: DecimalValue,
    direction: SensitivityDirection,
    curve_rebuild: CurveRebuildPolicy,
    second_order: SecondOrderPolicy,
}

impl SensitivityConvention {
    pub fn new(
        bump: DecimalValue,
        direction: SensitivityDirection,
        curve_rebuild: CurveRebuildPolicy,
        second_order: SecondOrderPolicy,
    ) -> DomainResult<Self> {
        if !bump.is_positive() {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            bump,
            direction,
            curve_rebuild,
            second_order,
        })
    }

    pub fn bump(&self) -> &DecimalValue {
        &self.bump
    }

    pub const fn direction(&self) -> SensitivityDirection {
        self.direction
    }

    pub const fn curve_rebuild(&self) -> CurveRebuildPolicy {
        self.curve_rebuild
    }

    pub const fn second_order(&self) -> SecondOrderPolicy {
        self.second_order
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactorDefinition {
    factor_id: String,
    factor_unit: UnitRef,
    convention: SensitivityConvention,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactorDefinitionInput {
    pub factor_id: String,
    pub factor_unit: UnitRef,
    pub convention: SensitivityConvention,
    pub content_hash: ContentHash,
}

impl FactorDefinition {
    pub fn new(input: FactorDefinitionInput) -> DomainResult<Self> {
        validate_factor_id(&input.factor_id)?;
        let actual = Self::content_hash_for(&input);
        if actual != input.content_hash {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        Ok(Self {
            factor_id: input.factor_id,
            factor_unit: input.factor_unit,
            convention: input.convention,
            content_hash: input.content_hash,
        })
    }

    pub fn content_hash_for(input: &FactorDefinitionInput) -> ContentHash {
        let mut bytes = Vec::new();
        append(&mut bytes, input.factor_id.as_bytes());
        append_unit(&mut bytes, &input.factor_unit);
        append_decimal(&mut bytes, input.convention.bump());
        append(&mut bytes, &[direction_code(input.convention.direction())]);
        append(
            &mut bytes,
            &[curve_rebuild_code(input.convention.curve_rebuild())],
        );
        append(
            &mut bytes,
            &[second_order_code(input.convention.second_order())],
        );
        ContentHash::digest(&bytes)
    }

    pub fn factor_id(&self) -> &str {
        &self.factor_id
    }

    pub fn factor_unit(&self) -> &UnitRef {
        &self.factor_unit
    }

    pub fn convention(&self) -> &SensitivityConvention {
        &self.convention
    }
}

impl ContentAddressed for FactorDefinition {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurveNodeDefinition {
    curve_node_id: String,
    curve_family_id: String,
    tenor: String,
    factor_unit: UnitRef,
    content_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurveNodeDefinitionInput {
    pub curve_node_id: String,
    pub curve_family_id: String,
    pub tenor: String,
    pub factor_unit: UnitRef,
    pub content_hash: ContentHash,
}

impl CurveNodeDefinition {
    pub fn new(input: CurveNodeDefinitionInput) -> DomainResult<Self> {
        validate_dotted_id(&input.curve_node_id, 3)?;
        validate_dotted_id(&input.curve_family_id, 3)?;
        validate_tenor(&input.tenor)?;
        if Self::content_hash_for(&input) != input.content_hash {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        Ok(Self {
            curve_node_id: input.curve_node_id,
            curve_family_id: input.curve_family_id,
            tenor: input.tenor,
            factor_unit: input.factor_unit,
            content_hash: input.content_hash,
        })
    }

    pub fn content_hash_for(input: &CurveNodeDefinitionInput) -> ContentHash {
        let mut bytes = Vec::new();
        append(&mut bytes, input.curve_node_id.as_bytes());
        append(&mut bytes, input.curve_family_id.as_bytes());
        append(&mut bytes, input.tenor.as_bytes());
        append_unit(&mut bytes, &input.factor_unit);
        ContentHash::digest(&bytes)
    }

    pub fn curve_node_id(&self) -> &str {
        &self.curve_node_id
    }

    pub fn curve_family_id(&self) -> &str {
        &self.curve_family_id
    }

    pub fn tenor(&self) -> &str {
        &self.tenor
    }

    pub fn factor_unit(&self) -> &UnitRef {
        &self.factor_unit
    }
}

impl ContentAddressed for CurveNodeDefinition {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CurveNodeRef {
    curve_node_id: String,
    content_hash: ContentHash,
}

impl CurveNodeRef {
    pub fn new(curve_node_id: impl Into<String>, content_hash: ContentHash) -> DomainResult<Self> {
        let curve_node_id = curve_node_id.into();
        validate_dotted_id(&curve_node_id, 3)?;
        Ok(Self {
            curve_node_id,
            content_hash,
        })
    }

    pub fn curve_node_id(&self) -> &str {
        &self.curve_node_id
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentFactorTarget {
    owner: OwnerRef,
    instrument: VersionRef,
}

impl InstrumentFactorTarget {
    pub fn new(owner: OwnerRef, instrument: VersionRef) -> Self {
        Self { owner, instrument }
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn instrument(&self) -> &VersionRef {
        &self.instrument
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FactorTarget {
    Instrument(InstrumentFactorTarget),
    CurveNode(CurveNodeRef),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactorTargetBinding {
    factor_id: String,
    target: FactorTarget,
    content_hash: ContentHash,
}

impl FactorTargetBinding {
    pub fn new(factor_id: impl Into<String>, target: FactorTarget) -> DomainResult<Self> {
        let factor_id = factor_id.into();
        validate_factor_id(&factor_id)?;
        let content_hash = binding_hash(&factor_id, &target);
        Ok(Self {
            factor_id,
            target,
            content_hash,
        })
    }

    pub fn factor_id(&self) -> &str {
        &self.factor_id
    }

    pub fn target(&self) -> &FactorTarget {
        &self.target
    }
}

impl ContentAddressed for FactorTargetBinding {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

fn binding_hash(factor_id: &str, target: &FactorTarget) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, factor_id.as_bytes());
    match target {
        FactorTarget::Instrument(value) => {
            append(&mut bytes, &[1]);
            append(&mut bytes, value.owner().tenant_id().as_str().as_bytes());
            append(&mut bytes, value.owner().owner_id().as_str().as_bytes());
            append(&mut bytes, value.instrument().id().as_str().as_bytes());
            append(
                &mut bytes,
                &value.instrument().version().get().to_be_bytes(),
            );
        }
        FactorTarget::CurveNode(value) => {
            append(&mut bytes, &[2]);
            append(&mut bytes, value.curve_node_id().as_bytes());
            append(&mut bytes, value.content_hash().as_bytes());
        }
    }
    ContentHash::digest(&bytes)
}

fn validate_factor_id(value: &str) -> DomainResult<()> {
    let segments = value.split('.').collect::<Vec<_>>();
    if segments.len() != 4 || segments.iter().any(|segment| !is_id_segment(segment)) {
        return Err(DomainErrorCode::InvalidId);
    }
    Ok(())
}

fn validate_dotted_id(value: &str, minimum_segments: usize) -> DomainResult<()> {
    let segments = value.split('.').collect::<Vec<_>>();
    if segments.len() < minimum_segments || segments.iter().any(|segment| !is_id_segment(segment)) {
        return Err(DomainErrorCode::InvalidId);
    }
    Ok(())
}

fn is_id_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn validate_tenor(value: &str) -> DomainResult<()> {
    let bytes = value.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'P' || !matches!(bytes[bytes.len() - 1], b'Y' | b'M' | b'D')
    {
        return Err(DomainErrorCode::InvalidValue);
    }
    let amount = &bytes[1..bytes.len() - 1];
    if amount[0] == b'0' || !amount.iter().all(u8::is_ascii_digit) {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(())
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn append_unit(bytes: &mut Vec<u8>, value: &UnitRef) {
    append(bytes, value.unit_id().as_str().as_bytes());
    append(bytes, &value.version().get().to_be_bytes());
}

fn append_decimal(bytes: &mut Vec<u8>, value: &DecimalValue) {
    append(bytes, value.coefficient().as_bytes());
    append(bytes, &value.scale().to_be_bytes());
    append_unit(bytes, value.unit());
}

const fn direction_code(value: SensitivityDirection) -> u8 {
    match value {
        SensitivityDirection::Central => 1,
        SensitivityDirection::Up => 2,
        SensitivityDirection::Down => 3,
    }
}

const fn curve_rebuild_code(value: CurveRebuildPolicy) -> u8 {
    match value {
        CurveRebuildPolicy::Rebuild => 1,
        CurveRebuildPolicy::Hold => 2,
    }
}

const fn second_order_code(value: SecondOrderPolicy) -> u8 {
    match value {
        SecondOrderPolicy::Include => 1,
        SecondOrderPolicy::Exclude => 2,
    }
}
