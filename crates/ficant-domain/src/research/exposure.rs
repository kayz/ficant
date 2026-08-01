use crate::analytics::FixedDecimal;
use crate::primitives::{ContentHash, LineageRef, Ulid, UnitRef, Version, VersionRef};
use crate::research::SensitivityDirection;
use crate::{ContentAddressed, DomainErrorCode, DomainResult, Lineaged};

const FIXED_SCALE: i128 = 1_000_000_000_000;

/// Calculates price sensitivity per one basis point using the frozen R4d-a direction formula.
pub fn key_rate_dv01(
    base: FixedDecimal,
    up: FixedDecimal,
    down: FixedDecimal,
    bump_bp: FixedDecimal,
    direction: SensitivityDirection,
) -> DomainResult<FixedDecimal> {
    if !bump_bp.is_positive() {
        return Err(DomainErrorCode::InvalidValue);
    }
    let numerator = match direction {
        SensitivityDirection::Central => down.checked_sub(up)?,
        SensitivityDirection::Up => base.checked_sub(up)?,
        SensitivityDirection::Down => down.checked_sub(base)?,
    };
    let divisor = match direction {
        SensitivityDirection::Central => bump_bp
            .scaled()
            .checked_mul(2)
            .ok_or(DomainErrorCode::InvalidValue)?,
        SensitivityDirection::Up | SensitivityDirection::Down => bump_bp.scaled(),
    };
    let scaled_numerator = numerator
        .scaled()
        .checked_mul(FIXED_SCALE)
        .ok_or(DomainErrorCode::InvalidValue)?;
    if divisor == 0 || scaled_numerator % divisor != 0 {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(FixedDecimal::from_scaled(scaled_numerator / divisor))
}

/// Scales one fixed-CTD registered-face KRD into a signed futures-position KRD.
///
/// Every intermediate operation must be exactly representable at the canonical fixed scale.
pub fn scale_futures_key_rate_dv01(
    ctd_registered_face_krd: FixedDecimal,
    registered_face: FixedDecimal,
    face_quote_basis: FixedDecimal,
    contract_size_in_quote_units: u32,
    conversion_factor: FixedDecimal,
    signed_contract_count: i64,
) -> DomainResult<FixedDecimal> {
    if !registered_face.is_positive()
        || !face_quote_basis.is_positive()
        || contract_size_in_quote_units == 0
        || !conversion_factor.is_positive()
        || signed_contract_count == 0
    {
        return Err(DomainErrorCode::InvalidValue);
    }
    let quote_krd = exact_div(
        ctd_registered_face_krd.checked_mul(face_quote_basis)?,
        registered_face,
    )?;
    let contract_size = FixedDecimal::from_scaled(
        i128::from(contract_size_in_quote_units)
            .checked_mul(FIXED_SCALE)
            .ok_or(DomainErrorCode::InvalidValue)?,
    );
    let one_contract = exact_div(quote_krd.checked_mul(contract_size)?, conversion_factor)?;
    one_contract.checked_mul(FixedDecimal::from_scaled(
        i128::from(signed_contract_count)
            .checked_mul(FIXED_SCALE)
            .ok_or(DomainErrorCode::InvalidValue)?,
    ))
}

fn exact_div(numerator: FixedDecimal, denominator: FixedDecimal) -> DomainResult<FixedDecimal> {
    if denominator.scaled() == 0 {
        return Err(DomainErrorCode::InvalidValue);
    }
    let scaled = numerator
        .scaled()
        .checked_mul(FIXED_SCALE)
        .ok_or(DomainErrorCode::InvalidValue)?;
    if scaled % denominator.scaled() != 0 {
        return Err(DomainErrorCode::InvalidValue);
    }
    Ok(FixedDecimal::from_scaled(scaled / denominator.scaled()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactorDv01 {
    factor_id: String,
    factor_definition_hash: ContentHash,
    value: FixedDecimal,
    unit: UnitRef,
}

impl FactorDv01 {
    pub fn new(
        factor_id: impl Into<String>,
        factor_definition_hash: ContentHash,
        value: FixedDecimal,
        unit: UnitRef,
    ) -> DomainResult<Self> {
        let factor_id = factor_id.into();
        if factor_id.trim().is_empty() || factor_id != factor_id.trim() {
            return Err(DomainErrorCode::InvalidId);
        }
        Ok(Self {
            factor_id,
            factor_definition_hash,
            value,
            unit,
        })
    }

    pub fn factor_id(&self) -> &str {
        &self.factor_id
    }

    pub fn factor_definition_hash(&self) -> &ContentHash {
        &self.factor_definition_hash
    }

    pub const fn value(&self) -> FixedDecimal {
        self.value
    }

    pub fn unit(&self) -> &UnitRef {
        &self.unit
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionKeyRateExposure {
    position_id: Ulid,
    instrument: VersionRef,
    exposures: Vec<FactorDv01>,
    input_evidence_hashes: Vec<ContentHash>,
    content_hash: ContentHash,
    lineage: Vec<LineageRef>,
}

impl PositionKeyRateExposure {
    pub fn new(
        position_id: Ulid,
        instrument: VersionRef,
        exposures: Vec<FactorDv01>,
        input_evidence_hashes: Vec<ContentHash>,
        lineage: Vec<LineageRef>,
    ) -> DomainResult<Self> {
        if exposures.is_empty()
            || input_evidence_hashes.is_empty()
            || lineage.is_empty()
            || exposures
                .windows(2)
                .any(|pair| pair[0].factor_id() >= pair[1].factor_id())
            || input_evidence_hashes
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || exposures
                .iter()
                .any(|value| value.unit() != exposures[0].unit())
        {
            return Err(DomainErrorCode::BrokenLineage);
        }
        let content_hash = hash_position(
            &position_id,
            &instrument,
            &exposures,
            &input_evidence_hashes,
            &lineage,
        );
        Ok(Self {
            position_id,
            instrument,
            exposures,
            input_evidence_hashes,
            content_hash,
            lineage,
        })
    }

    pub fn position_id(&self) -> &Ulid {
        &self.position_id
    }

    pub fn instrument(&self) -> &VersionRef {
        &self.instrument
    }

    pub fn exposures(&self) -> &[FactorDv01] {
        &self.exposures
    }

    pub fn input_evidence_hashes(&self) -> &[ContentHash] {
        &self.input_evidence_hashes
    }
}

impl ContentAddressed for PositionKeyRateExposure {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl Lineaged for PositionKeyRateExposure {
    fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}

pub fn aggregate_bond_key_rate_exposures(
    positions: &[PositionKeyRateExposure],
) -> DomainResult<Vec<FactorDv01>> {
    let first = positions.first().ok_or(DomainErrorCode::InvalidValue)?;
    let mut totals = first.exposures.clone();
    for position in &positions[1..] {
        if position.exposures.len() != totals.len() {
            return Err(DomainErrorCode::BrokenLineage);
        }
        for (total, value) in totals.iter_mut().zip(&position.exposures) {
            if total.factor_id != value.factor_id
                || total.factor_definition_hash != value.factor_definition_hash
                || total.unit != value.unit
            {
                return Err(DomainErrorCode::BrokenLineage);
            }
            total.value = total.value.checked_add(value.value)?;
        }
    }
    Ok(totals)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskAlgorithmBinding {
    algorithm_id: String,
    algorithm_version: u32,
    convention_profile: String,
}

impl RiskAlgorithmBinding {
    pub fn new(
        algorithm_id: impl Into<String>,
        algorithm_version: u32,
        convention_profile: impl Into<String>,
    ) -> DomainResult<Self> {
        let algorithm_id = algorithm_id.into();
        let convention_profile = convention_profile.into();
        if algorithm_version == 0
            || algorithm_id.trim().is_empty()
            || algorithm_id != algorithm_id.trim()
            || convention_profile.trim().is_empty()
            || convention_profile != convention_profile.trim()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            algorithm_id,
            algorithm_version,
            convention_profile,
        })
    }

    pub fn algorithm_id(&self) -> &str {
        &self.algorithm_id
    }

    pub const fn algorithm_version(&self) -> u32 {
        self.algorithm_version
    }

    pub fn convention_profile(&self) -> &str {
        &self.convention_profile
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioKeyRateExposure {
    position_snapshot_id: Ulid,
    curve_snapshot_id: Ulid,
    futures_data_snapshot_id: Option<Ulid>,
    positions: Vec<PositionKeyRateExposure>,
    totals: Vec<FactorDv01>,
    algorithm: RiskAlgorithmBinding,
    content_hash: ContentHash,
    lineage: Vec<LineageRef>,
}

impl PortfolioKeyRateExposure {
    pub fn new(
        position_snapshot_id: Ulid,
        curve_snapshot_id: Ulid,
        positions: Vec<PositionKeyRateExposure>,
        algorithm: RiskAlgorithmBinding,
        lineage: Vec<LineageRef>,
    ) -> DomainResult<Self> {
        Self::new_inner(
            position_snapshot_id,
            curve_snapshot_id,
            None,
            positions,
            algorithm,
            lineage,
        )
    }

    pub fn new_with_futures_data_snapshot(
        position_snapshot_id: Ulid,
        curve_snapshot_id: Ulid,
        futures_data_snapshot_id: Ulid,
        positions: Vec<PositionKeyRateExposure>,
        algorithm: RiskAlgorithmBinding,
        lineage: Vec<LineageRef>,
    ) -> DomainResult<Self> {
        Self::new_inner(
            position_snapshot_id,
            curve_snapshot_id,
            Some(futures_data_snapshot_id),
            positions,
            algorithm,
            lineage,
        )
    }

    fn new_inner(
        position_snapshot_id: Ulid,
        curve_snapshot_id: Ulid,
        futures_data_snapshot_id: Option<Ulid>,
        positions: Vec<PositionKeyRateExposure>,
        algorithm: RiskAlgorithmBinding,
        lineage: Vec<LineageRef>,
    ) -> DomainResult<Self> {
        if positions.is_empty()
            || lineage.is_empty()
            || positions
                .windows(2)
                .any(|pair| pair[0].position_id() >= pair[1].position_id())
        {
            return Err(DomainErrorCode::BrokenLineage);
        }
        let totals = aggregate_bond_key_rate_exposures(&positions)?;
        let mut bytes = Vec::new();
        append(&mut bytes, position_snapshot_id.as_str().as_bytes());
        append(&mut bytes, curve_snapshot_id.as_str().as_bytes());
        if let Some(value) = &futures_data_snapshot_id {
            append(&mut bytes, b"futures_data_snapshot_id");
            append(&mut bytes, value.as_str().as_bytes());
        }
        for position in &positions {
            append(&mut bytes, position.content_hash().as_bytes());
        }
        for total in &totals {
            append(&mut bytes, total.factor_id().as_bytes());
            append(&mut bytes, total.factor_definition_hash().as_bytes());
            append(&mut bytes, &total.value().scaled().to_be_bytes());
            append(&mut bytes, total.unit().unit_id().as_str().as_bytes());
            append(&mut bytes, &total.unit().version().get().to_be_bytes());
        }
        append(&mut bytes, algorithm.algorithm_id().as_bytes());
        append(&mut bytes, &algorithm.algorithm_version().to_be_bytes());
        append(&mut bytes, algorithm.convention_profile().as_bytes());
        for reference in &lineage {
            append_lineage(&mut bytes, reference);
        }
        let content_hash = ContentHash::digest(&bytes);
        Ok(Self {
            position_snapshot_id,
            curve_snapshot_id,
            futures_data_snapshot_id,
            positions,
            totals,
            algorithm,
            content_hash,
            lineage,
        })
    }

    pub fn position_snapshot_id(&self) -> &Ulid {
        &self.position_snapshot_id
    }

    pub fn curve_snapshot_id(&self) -> &Ulid {
        &self.curve_snapshot_id
    }

    pub fn futures_data_snapshot_id(&self) -> Option<&Ulid> {
        self.futures_data_snapshot_id.as_ref()
    }

    pub fn positions(&self) -> &[PositionKeyRateExposure] {
        &self.positions
    }

    pub fn totals(&self) -> &[FactorDv01] {
        &self.totals
    }

    pub fn algorithm(&self) -> &RiskAlgorithmBinding {
        &self.algorithm
    }
}

impl ContentAddressed for PortfolioKeyRateExposure {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl Lineaged for PortfolioKeyRateExposure {
    fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}

fn hash_position(
    position_id: &Ulid,
    instrument: &VersionRef,
    exposures: &[FactorDv01],
    input_evidence_hashes: &[ContentHash],
    lineage: &[LineageRef],
) -> ContentHash {
    let mut bytes = Vec::new();
    append(&mut bytes, position_id.as_str().as_bytes());
    append(&mut bytes, instrument.id().as_str().as_bytes());
    append(&mut bytes, &instrument.version().get().to_be_bytes());
    for exposure in exposures {
        append(&mut bytes, exposure.factor_id.as_bytes());
        append(&mut bytes, exposure.factor_definition_hash.as_bytes());
        append(&mut bytes, &exposure.value.scaled().to_be_bytes());
        append(&mut bytes, exposure.unit.unit_id().as_str().as_bytes());
        append(&mut bytes, &exposure.unit.version().get().to_be_bytes());
    }
    for hash in input_evidence_hashes {
        append(&mut bytes, hash.as_bytes());
    }
    for reference in lineage {
        append_lineage(&mut bytes, reference);
    }
    ContentHash::digest(&bytes)
}

fn append_lineage(bytes: &mut Vec<u8>, reference: &LineageRef) {
    append(bytes, reference.object_id().as_str().as_bytes());
    append(
        bytes,
        &reference.version().map_or(0, Version::get).to_be_bytes(),
    );
    match reference.content_hash() {
        Some(hash) => append(bytes, hash.as_bytes()),
        None => append(bytes, &[]),
    }
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}
