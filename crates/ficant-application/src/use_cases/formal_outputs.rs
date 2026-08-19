use ficant_domain::primitives::ContentHash;

use crate::ports::{AccessScope, ApplicationResult, FormalOutputRecord, FormalOutputRepository};

pub struct FormalOutputUseCase<'a> {
    repository: &'a dyn FormalOutputRepository,
}

impl<'a> FormalOutputUseCase<'a> {
    #[must_use]
    pub const fn new(repository: &'a dyn FormalOutputRepository) -> Self {
        Self { repository }
    }

    /// Publishes one verified formal output before its service response is allowed to succeed.
    ///
    /// # Errors
    ///
    /// Returns authorization, integrity, idempotency, or storage errors without converting them to
    /// a successful response.
    pub async fn publish(
        &self,
        scope: &AccessScope,
        record: FormalOutputRecord,
    ) -> ApplicationResult<FormalOutputRecord> {
        scope.authorize(record.owner())?;
        record.verify()?;
        let stored = self.repository.publish(scope, record).await?;
        scope.authorize(stored.owner())?;
        stored.verify()?;
        Ok(stored)
    }

    /// Performs a required read and verifies the returned payload/evidence before exposing it.
    ///
    /// # Errors
    ///
    /// Returns authorization, storage, or integrity errors. Missing records remain `None`.
    pub async fn get(
        &self,
        scope: &AccessScope,
        output_identity: &ContentHash,
    ) -> ApplicationResult<Option<FormalOutputRecord>> {
        let Some(record) = self.repository.get(scope, output_identity).await? else {
            return Ok(None);
        };
        scope.authorize(record.owner())?;
        record.verify()?;
        Ok(Some(record))
    }
}
