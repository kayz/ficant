use ficant_domain::analytics::AnalyticsError;
use ficant_domain::futures_delivery::{
    FuturesDeliverableInput, FuturesDeliveryBasketResult, FuturesDeliveryResult,
};
use ficant_domain::primitives::ContentHash;
use ficant_domain::{DomainErrorCode, DomainResult};

pub trait FuturesDeliveryEngine: Send + Sync {
    /// Calculates one validated CFFEX deliverable-candidate result.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for invalid ABI, input, eligibility, or numerical failure.
    fn calculate(
        &self,
        input: &FuturesDeliverableInput,
    ) -> Result<FuturesDeliveryResult, AnalyticsError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedFuturesDeliveryArtifact {
    bytes: Vec<u8>,
    content_hash: ContentHash,
}

impl EncodedFuturesDeliveryArtifact {
    /// Binds non-empty encoded bytes to their exact content hash.
    ///
    /// # Errors
    ///
    /// Returns a domain error for an empty payload or a hash mismatch.
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

pub trait FuturesDeliveryArtifactCodec: Send + Sync {
    /// Encodes one validated basket into its deterministic physical representation.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error when any row cannot be represented exactly.
    fn encode(
        &self,
        result: &FuturesDeliveryBasketResult,
    ) -> Result<EncodedFuturesDeliveryArtifact, AnalyticsError>;

    /// Decodes bytes only when every row binds to the exact expected inputs.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for malformed bytes, schema drift, or input drift.
    fn decode(
        &self,
        bytes: &[u8],
        expected_inputs: &[FuturesDeliverableInput],
    ) -> Result<FuturesDeliveryBasketResult, AnalyticsError>;
}
