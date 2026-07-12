use std::fmt;

use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ulid(String);

impl Ulid {
    pub fn new(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        let parsed = value
            .parse::<ulid::Ulid>()
            .map_err(|_| DomainErrorCode::InvalidId)?;

        if value.len() != 26 || !value.is_ascii() || parsed.to_string() != value {
            return Err(DomainErrorCode::InvalidId);
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Ulid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerRef {
    tenant_id: Ulid,
    owner_id: Ulid,
}

impl OwnerRef {
    pub fn new(tenant_id: Ulid, owner_id: Ulid) -> Self {
        Self {
            tenant_id,
            owner_id,
        }
    }

    pub fn tenant_id(&self) -> &Ulid {
        &self.tenant_id
    }

    pub fn owner_id(&self) -> &Ulid {
        &self.owner_id
    }
}
