use ficant_domain::analytics::AnalyticsError;
use ficant_domain::curves::{CarryRollInput, CarryRollResult, YieldCurvePoint, YieldCurveQuery};
use ficant_domain::primitives::ContentHash;
use ficant_domain::{DomainErrorCode, DomainResult};

pub trait YieldCurveEngine: Send + Sync {
    /// Interpolates one validated point without extrapolation.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for invalid ABI, input, or numerical failure.
    fn interpolate(&self, query: &YieldCurveQuery) -> Result<YieldCurvePoint, AnalyticsError>;
}

pub trait CarryRollEngine: Send + Sync {
    /// Calculates one unfunded holding-period carry and roll-down decomposition.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for invalid input, curve coverage, or numerical failure.
    fn calculate(&self, input: &CarryRollInput) -> Result<CarryRollResult, AnalyticsError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedCarryRollArtifact {
    bytes: Vec<u8>,
    content_hash: ContentHash,
}

impl EncodedCarryRollArtifact {
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

pub trait CarryRollArtifactCodec: Send + Sync {
    /// Encodes one validated result into the frozen deterministic representation.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error when the result cannot be represented exactly.
    fn encode(&self, result: &CarryRollResult) -> Result<EncodedCarryRollArtifact, AnalyticsError>;

    /// Decodes bytes only when every frozen input field matches the expected input.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for malformed bytes, schema drift, or input drift.
    fn decode(
        &self,
        bytes: &[u8],
        expected_input: &CarryRollInput,
    ) -> Result<CarryRollResult, AnalyticsError>;
}
