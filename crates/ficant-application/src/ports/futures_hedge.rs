use ficant_domain::analytics::AnalyticsError;
use ficant_domain::futures_hedge::{FuturesHedgeInput, FuturesHedgeResult};
use ficant_domain::primitives::ContentHash;
use ficant_domain::{DomainErrorCode, DomainResult};

pub trait FuturesHedgeEngine: Send + Sync {
    /// Calculates one validated CTD-based DV01 hedge.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for invalid ABI, input, or numerical failure.
    fn calculate(&self, input: &FuturesHedgeInput) -> Result<FuturesHedgeResult, AnalyticsError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedFuturesHedgeArtifact {
    bytes: Vec<u8>,
    content_hash: ContentHash,
}

impl EncodedFuturesHedgeArtifact {
    /// Binds non-empty encoded bytes to their exact content hash.
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

pub trait FuturesHedgeArtifactCodec: Send + Sync {
    /// Encodes one validated hedge result deterministically.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error when validation or encoding fails.
    fn encode(
        &self,
        result: &FuturesHedgeResult,
    ) -> Result<EncodedFuturesHedgeArtifact, AnalyticsError>;

    /// Decodes bytes only when they bind to the exact expected input.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for malformed bytes or an input mismatch.
    fn decode(
        &self,
        bytes: &[u8],
        expected_input: &FuturesHedgeInput,
    ) -> Result<FuturesHedgeResult, AnalyticsError>;
}
