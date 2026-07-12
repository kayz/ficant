use crate::primitives::{ContentHash, Ulid, Version};
use crate::{DomainErrorCode, DomainResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineageRef {
    object_id: Ulid,
    version: Option<Version>,
    content_hash: Option<ContentHash>,
}

impl LineageRef {
    pub fn new(
        object_id: Ulid,
        version: Option<Version>,
        content_hash: Option<ContentHash>,
    ) -> DomainResult<Self> {
        if version.is_none() && content_hash.is_none() {
            return Err(DomainErrorCode::BrokenLineage);
        }
        Ok(Self {
            object_id,
            version,
            content_hash,
        })
    }

    pub fn versioned(object_id: Ulid, version: Version) -> Self {
        Self {
            object_id,
            version: Some(version),
            content_hash: None,
        }
    }

    pub fn content_addressed(object_id: Ulid, content_hash: ContentHash) -> Self {
        Self {
            object_id,
            version: None,
            content_hash: Some(content_hash),
        }
    }

    pub fn object_id(&self) -> &Ulid {
        &self.object_id
    }

    pub fn version(&self) -> Option<Version> {
        self.version
    }

    pub fn content_hash(&self) -> Option<&ContentHash> {
        self.content_hash.as_ref()
    }
}
