use crate::primitives::{ContentHash, LineageRef, MarketTime, OwnerRef, Ulid};
use crate::{ContentAddressed, DomainErrorCode, DomainResult, Lineaged};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataSnapshot {
    data_snapshot_id: Ulid,
    owner: OwnerRef,
    visible_at: MarketTime,
    as_of: MarketTime,
    schema_hash: ContentHash,
    manifest_hash: ContentHash,
    blob_content_hash: ContentHash,
    lineage: Vec<LineageRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataSnapshotInput {
    pub data_snapshot_id: Ulid,
    pub owner: OwnerRef,
    pub visible_at: MarketTime,
    pub as_of: MarketTime,
    pub schema_hash: ContentHash,
    pub manifest_hash: ContentHash,
    pub blob_content_hash: ContentHash,
    pub lineage: Vec<LineageRef>,
}

impl DataSnapshot {
    pub fn new(input: DataSnapshotInput) -> DomainResult<Self> {
        let DataSnapshotInput {
            data_snapshot_id,
            owner,
            visible_at,
            as_of,
            schema_hash,
            manifest_hash,
            blob_content_hash,
            lineage,
        } = input;
        if visible_at.instant() < as_of.instant() {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        if lineage.is_empty() {
            return Err(DomainErrorCode::BrokenLineage);
        }
        Ok(Self {
            data_snapshot_id,
            owner,
            visible_at,
            as_of,
            schema_hash,
            manifest_hash,
            blob_content_hash,
            lineage,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.data_snapshot_id
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn visible_at(&self) -> &MarketTime {
        &self.visible_at
    }

    pub fn as_of(&self) -> &MarketTime {
        &self.as_of
    }

    pub fn schema_hash(&self) -> &ContentHash {
        &self.schema_hash
    }

    pub fn manifest_hash(&self) -> &ContentHash {
        &self.manifest_hash
    }
}

impl ContentAddressed for DataSnapshot {
    fn content_hash(&self) -> &ContentHash {
        &self.blob_content_hash
    }
}

impl Lineaged for DataSnapshot {
    fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}
