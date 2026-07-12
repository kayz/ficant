use crate::primitives::{ContentHash, LineageRef, OwnerRef, Ulid, VersionRef};
use crate::{ContentAddressed, DomainErrorCode, DomainResult, Lineaged};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniverseSnapshot {
    universe_snapshot_id: Ulid,
    owner: OwnerRef,
    instrument_versions: Vec<VersionRef>,
    filter_digest: ContentHash,
    content_hash: ContentHash,
    lineage: Vec<LineageRef>,
}

impl UniverseSnapshot {
    pub fn new(
        universe_snapshot_id: Ulid,
        owner: OwnerRef,
        instrument_versions: Vec<VersionRef>,
        filter_digest: ContentHash,
        content_hash: ContentHash,
        lineage: Vec<LineageRef>,
    ) -> DomainResult<Self> {
        if instrument_versions.is_empty()
            || instrument_versions
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        if lineage.is_empty() {
            return Err(DomainErrorCode::BrokenLineage);
        }
        Ok(Self {
            universe_snapshot_id,
            owner,
            instrument_versions,
            filter_digest,
            content_hash,
            lineage,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.universe_snapshot_id
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn instrument_versions(&self) -> &[VersionRef] {
        &self.instrument_versions
    }

    pub fn filter_digest(&self) -> &ContentHash {
        &self.filter_digest
    }
}

impl ContentAddressed for UniverseSnapshot {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl Lineaged for UniverseSnapshot {
    fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}
