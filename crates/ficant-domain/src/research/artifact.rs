use crate::primitives::{ContentHash, LineageRef, OwnerRef, Ulid};
use crate::{ContentAddressed, DomainErrorCode, DomainResult, Lineaged};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactKind {
    Generic,
    SignalSet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    artifact_id: Ulid,
    owner: OwnerRef,
    kind: ArtifactKind,
    media_type: String,
    content_hash: ContentHash,
    blob_size: u64,
    lineage: Vec<LineageRef>,
}

impl Artifact {
    pub fn new(
        artifact_id: Ulid,
        owner: OwnerRef,
        kind: ArtifactKind,
        media_type: impl Into<String>,
        content_hash: ContentHash,
        blob_size: u64,
        lineage: Vec<LineageRef>,
    ) -> DomainResult<Self> {
        let media_type = media_type.into();
        if media_type.trim().is_empty() || media_type != media_type.trim() || blob_size == 0 {
            return Err(DomainErrorCode::InvalidValue);
        }
        if lineage.is_empty() {
            return Err(DomainErrorCode::BrokenLineage);
        }
        Ok(Self {
            artifact_id,
            owner,
            kind,
            media_type,
            content_hash,
            blob_size,
            lineage,
        })
    }

    pub fn id(&self) -> &Ulid {
        &self.artifact_id
    }

    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    pub fn kind(&self) -> ArtifactKind {
        self.kind
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    pub fn blob_size(&self) -> u64 {
        self.blob_size
    }
}

impl ContentAddressed for Artifact {
    fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

impl Lineaged for Artifact {
    fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}
