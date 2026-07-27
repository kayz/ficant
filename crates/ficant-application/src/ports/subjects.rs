use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ficant_domain::primitives::{Ulid, VersionRef};
use ficant_domain::subject::{SubjectRecord, SubjectStateSnapshot};

use super::ApplicationResult;

#[async_trait]
pub trait SubjectRepository: Send + Sync {
    async fn register_subject(&self, value: SubjectRecord) -> ApplicationResult<SubjectRecord>;

    async fn get_subject(&self, reference: VersionRef) -> ApplicationResult<Option<SubjectRecord>>;

    async fn register_subject_state(
        &self,
        value: SubjectStateSnapshot,
    ) -> ApplicationResult<SubjectStateSnapshot>;

    async fn get_subject_state(
        &self,
        snapshot_id: Ulid,
        knowledge_at: DateTime<Utc>,
    ) -> ApplicationResult<Option<SubjectStateSnapshot>>;
}
