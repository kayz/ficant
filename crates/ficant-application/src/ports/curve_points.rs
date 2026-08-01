use async_trait::async_trait;
use ficant_domain::DomainErrorCode;
use ficant_domain::market::CurveSnapshot;
use ficant_domain::primitives::{ContentHash, DecimalValue, Ulid};

use super::{AccessScope, ApplicationResult};
use crate::map_domain_error;

pub const CURVE_POINT_SCHEMA: &str = "ficant.yield-curve-points.protobuf.v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedCurvePoint {
    curve_node_id: String,
    curve_node_content_hash: ContentHash,
    yield_to_maturity: DecimalValue,
}

impl DecodedCurvePoint {
    /// Creates one transport-neutral verified curve point.
    ///
    /// # Errors
    ///
    /// Returns a validation error for a blank or non-canonical node identity.
    pub fn new(
        curve_node_id: impl Into<String>,
        curve_node_content_hash: ContentHash,
        yield_to_maturity: DecimalValue,
    ) -> ApplicationResult<Self> {
        let curve_node_id = curve_node_id.into();
        if curve_node_id.trim().is_empty() || curve_node_id != curve_node_id.trim() {
            return Err(map_domain_error(DomainErrorCode::InvalidId));
        }
        Ok(Self {
            curve_node_id,
            curve_node_content_hash,
            yield_to_maturity,
        })
    }

    #[must_use]
    pub fn curve_node_id(&self) -> &str {
        &self.curve_node_id
    }

    #[must_use]
    pub fn curve_node_content_hash(&self) -> &ContentHash {
        &self.curve_node_content_hash
    }

    #[must_use]
    pub fn yield_to_maturity(&self) -> &DecimalValue {
        &self.yield_to_maturity
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedCurvePointSet {
    curve_family_id: String,
    points: Vec<DecodedCurvePoint>,
}

impl DecodedCurvePointSet {
    /// Creates one canonical, strictly ordered curve-point set.
    ///
    /// # Errors
    ///
    /// Returns a validation error unless the family is canonical and at least two unique points
    /// are strictly ordered by stable node identity.
    pub fn new(
        curve_family_id: impl Into<String>,
        points: Vec<DecodedCurvePoint>,
    ) -> ApplicationResult<Self> {
        let curve_family_id = curve_family_id.into();
        if curve_family_id.trim().is_empty()
            || curve_family_id != curve_family_id.trim()
            || points.len() < 2
            || points
                .windows(2)
                .any(|pair| pair[0].curve_node_id() >= pair[1].curve_node_id())
        {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        Ok(Self {
            curve_family_id,
            points,
        })
    }

    #[must_use]
    pub fn curve_family_id(&self) -> &str {
        &self.curve_family_id
    }

    #[must_use]
    pub fn points(&self) -> &[DecodedCurvePoint] {
        &self.points
    }
}

pub trait CurvePointSetDecoder: Send + Sync {
    /// Decodes bytes only when the wire payload is canonical and complete.
    ///
    /// # Errors
    ///
    /// Returns an integrity or validation error for malformed, non-canonical, or incomplete bytes.
    fn decode_canonical(&self, bytes: &[u8]) -> ApplicationResult<DecodedCurvePointSet>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurveSnapshotMetadata {
    snapshot: CurveSnapshot,
    blob_size: u64,
}

impl CurveSnapshotMetadata {
    /// Binds one immutable curve snapshot to its declared durable blob size.
    ///
    /// # Errors
    ///
    /// Returns a validation error for a zero blob size.
    pub fn new(snapshot: CurveSnapshot, blob_size: u64) -> ApplicationResult<Self> {
        if blob_size == 0 {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        }
        Ok(Self {
            snapshot,
            blob_size,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> &CurveSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub const fn blob_size(&self) -> u64 {
        self.blob_size
    }
}

#[async_trait]
pub trait CurveSnapshotMetadataRepository: Send + Sync {
    async fn get_curve_snapshot_metadata(
        &self,
        scope: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> ApplicationResult<Option<CurveSnapshotMetadata>>;
}
