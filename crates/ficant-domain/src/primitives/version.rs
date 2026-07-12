use crate::primitives::Ulid;
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(u64);

impl Version {
    pub fn new(value: u64) -> DomainResult<Self> {
        if value == 0 {
            return Err(DomainErrorCode::VersionConflict);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VersionRef {
    id: Ulid,
    version: Version,
}

impl VersionRef {
    pub fn new(id: Ulid, version: Version) -> Self {
        Self { id, version }
    }

    pub fn id(&self) -> &Ulid {
        &self.id
    }

    pub fn version(&self) -> Version {
        self.version
    }
}

pub fn ensure_next_version(
    current_identity: &Ulid,
    current_version: Version,
    candidate_identity: &Ulid,
    candidate_version: Version,
) -> Result<(), DomainErrorCode> {
    let expected_version = current_version
        .get()
        .checked_add(1)
        .ok_or(DomainErrorCode::VersionConflict)?;

    if current_identity != candidate_identity || candidate_version.get() != expected_version {
        return Err(DomainErrorCode::VersionConflict);
    }

    Ok(())
}
