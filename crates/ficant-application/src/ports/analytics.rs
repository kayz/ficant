use ficant_domain::analytics::{AnalyticsError, BondAnalyticsInput, BondAnalyticsResult};
use ficant_domain::primitives::ContentHash;
use ficant_domain::{DomainErrorCode, DomainResult};

pub trait BondAnalyticsEngine: Send + Sync {
    /// Calculates one provider-neutral result from an already validated exact input.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for invalid ABI, calendar, input, or numerical failure.
    fn calculate(&self, input: &BondAnalyticsInput) -> Result<BondAnalyticsResult, AnalyticsError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedBondAnalyticsArtifact {
    bytes: Vec<u8>,
    content_hash: ContentHash,
}

impl EncodedBondAnalyticsArtifact {
    /// Creates encoded bytes bound to their exact content hash.
    ///
    /// # Errors
    ///
    /// Returns a domain error for an empty payload or hash mismatch.
    pub fn new(bytes: Vec<u8>, content_hash: ContentHash) -> DomainResult<Self> {
        if bytes.is_empty() {
            return Err(DomainErrorCode::InvalidValue);
        }
        content_hash.verify(&bytes)?;
        Ok(Self {
            bytes,
            content_hash,
        })
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    #[must_use]
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        u64::try_from(self.bytes.len()).unwrap_or(u64::MAX)
    }
}

pub trait BondAnalyticsArtifactCodec: Send + Sync {
    /// Encodes one validated result into the frozen deterministic physical representation.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error when the value cannot be represented exactly.
    fn encode(
        &self,
        result: &BondAnalyticsResult,
    ) -> Result<EncodedBondAnalyticsArtifact, AnalyticsError>;

    /// Decodes bytes while binding omitted physical fields to the caller's exact old input.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for malformed bytes, schema drift, or input mismatch.
    fn decode(
        &self,
        bytes: &[u8],
        expected_input: &BondAnalyticsInput,
    ) -> Result<BondAnalyticsResult, AnalyticsError>;
}
