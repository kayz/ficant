use ficant_domain::analytics::AnalyticsError;
use ficant_domain::analytics::AnalyticsObjectRef;
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliverableInput, FuturesDeliveryBasketResult, FuturesDeliveryResult,
};
use ficant_domain::primitives::{ContentHash, FixedDecimal, MarketTime};
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
pub struct FuturesDeliveryArtifactCandidateFacts {
    bond: AnalyticsObjectRef,
    conversion_factor: FixedDecimal,
}

impl FuturesDeliveryArtifactCandidateFacts {
    #[must_use]
    pub fn new(bond: AnalyticsObjectRef, conversion_factor: FixedDecimal) -> Self {
        Self {
            bond,
            conversion_factor,
        }
    }

    #[must_use]
    pub fn bond(&self) -> &AnalyticsObjectRef {
        &self.bond
    }

    #[must_use]
    pub const fn conversion_factor(&self) -> FixedDecimal {
        self.conversion_factor
    }
}

/// Self-describing facts read from one verified delivery-basket Artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesDeliveryArtifactFacts {
    valuation_at: MarketTime,
    futures_contract: AnalyticsObjectRef,
    rule_pack: AnalyticsObjectRef,
    snapshot: AnalyticsObjectRef,
    product: CgbFuturesProduct,
    candidates: Vec<FuturesDeliveryArtifactCandidateFacts>,
    ctd_index: usize,
}

impl FuturesDeliveryArtifactFacts {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        valuation_at: MarketTime,
        futures_contract: AnalyticsObjectRef,
        rule_pack: AnalyticsObjectRef,
        snapshot: AnalyticsObjectRef,
        product: CgbFuturesProduct,
        candidates: Vec<FuturesDeliveryArtifactCandidateFacts>,
        ctd_index: usize,
    ) -> Self {
        Self {
            valuation_at,
            futures_contract,
            rule_pack,
            snapshot,
            product,
            candidates,
            ctd_index,
        }
    }

    #[must_use]
    pub fn valuation_at(&self) -> &MarketTime {
        &self.valuation_at
    }

    #[must_use]
    pub fn futures_contract(&self) -> &AnalyticsObjectRef {
        &self.futures_contract
    }

    #[must_use]
    pub fn rule_pack(&self) -> &AnalyticsObjectRef {
        &self.rule_pack
    }

    #[must_use]
    pub fn snapshot(&self) -> &AnalyticsObjectRef {
        &self.snapshot
    }

    #[must_use]
    pub const fn product(&self) -> CgbFuturesProduct {
        self.product
    }

    #[must_use]
    pub fn candidates(&self) -> &[FuturesDeliveryArtifactCandidateFacts] {
        &self.candidates
    }

    #[must_use]
    pub const fn ctd_index(&self) -> usize {
        self.ctd_index
    }

    #[must_use]
    pub fn ctd(&self) -> Option<&FuturesDeliveryArtifactCandidateFacts> {
        self.candidates.get(self.ctd_index)
    }
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
    /// Encodes one validated basket into the frozen Phase 2C v1 representation.
    ///
    /// This entry point remains only as the byte-for-byte compatibility witness for the
    /// independently frozen Phase 2C artifact. New publication must use
    /// [`Self::encode_self_describing`].
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error when any row cannot be represented exactly.
    fn encode(
        &self,
        result: &FuturesDeliveryBasketResult,
    ) -> Result<EncodedFuturesDeliveryArtifact, AnalyticsError>;

    /// Encodes one validated basket with every fact required by downstream R5D materialization.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error when any row or authoritative input fact cannot be
    /// represented exactly.
    fn encode_self_describing(
        &self,
        result: &FuturesDeliveryBasketResult,
    ) -> Result<EncodedFuturesDeliveryArtifact, AnalyticsError>;

    /// Decodes bytes only when every row binds to the exact expected inputs.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for malformed bytes, schema drift, or input drift. Both
    /// the frozen Phase 2C representation and the R5D self-describing representation are accepted
    /// only when the caller supplies the exact original inputs.
    fn decode(
        &self,
        bytes: &[u8],
        expected_inputs: &[FuturesDeliverableInput],
    ) -> Result<FuturesDeliveryBasketResult, AnalyticsError>;

    /// Decodes every downstream-consumed fact without caller-supplied replacements.
    ///
    /// # Errors
    ///
    /// Returns a stable analytics error for malformed bytes, schema drift or inconsistent rows.
    fn decode_facts(&self, bytes: &[u8]) -> Result<FuturesDeliveryArtifactFacts, AnalyticsError>;
}
