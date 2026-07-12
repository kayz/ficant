use crate::market::require_text;
use crate::primitives::{OwnerRef, Ulid, Version, ensure_next_version};
use crate::{DomainErrorCode, DomainResult, VersionedDefinition};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    unit_id: Ulid,
    version: Version,
    owner: OwnerRef,
    code: String,
    dimension: String,
    scale: u32,
    precision: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitInput {
    pub unit_id: Ulid,
    pub version: Version,
    pub owner: OwnerRef,
    pub code: String,
    pub dimension: String,
    pub scale: u32,
    pub precision: u32,
}

impl Unit {
    pub fn new(input: UnitInput) -> DomainResult<Self> {
        let UnitInput {
            unit_id,
            version,
            owner,
            code,
            dimension,
            scale,
            precision,
        } = input;
        let normalized_code = !code.is_empty()
            && code.bytes().all(|byte| {
                byte.is_ascii_uppercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            });
        if !normalized_code || precision == 0 || scale > precision {
            return Err(DomainErrorCode::InvalidUnit);
        }
        require_text(&dimension).map_err(|_| DomainErrorCode::InvalidUnit)?;
        Ok(Self {
            unit_id,
            version,
            owner,
            code,
            dimension,
            scale,
            precision,
        })
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    pub fn scale(&self) -> u32 {
        self.scale
    }

    pub fn precision(&self) -> u32 {
        self.precision
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn validate_successor(&self, candidate: &Self) -> DomainResult<()> {
        ensure_next_version(
            &self.unit_id,
            self.version,
            &candidate.unit_id,
            candidate.version,
        )?;
        if self.dimension != candidate.dimension {
            return Err(DomainErrorCode::VersionConflict);
        }
        Ok(())
    }
}

impl VersionedDefinition for Unit {
    fn identity(&self) -> &str {
        self.unit_id.as_str()
    }

    fn version(&self) -> u64 {
        self.version.get()
    }
}
