use sha2::{Digest, Sha256};

use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    pub fn from_bytes(bytes: &[u8]) -> DomainResult<Self> {
        let value: [u8; 32] = bytes
            .try_into()
            .map_err(|_| DomainErrorCode::ContentHashMismatch)?;
        Ok(Self(value))
    }

    pub fn digest(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn verify(&self, bytes: &[u8]) -> DomainResult<()> {
        if &Self::digest(bytes) != self {
            return Err(DomainErrorCode::ContentHashMismatch);
        }
        Ok(())
    }
}
