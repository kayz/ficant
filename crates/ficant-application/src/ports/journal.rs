use async_trait::async_trait;
use ficant_domain::DomainErrorCode;
use ficant_domain::primitives::{OwnerRef, Ulid};
use ficant_domain::research::RunJournal;

use super::fingerprint::{FingerprintBuilder, journal_bytes, owner_bytes};
use super::{
    AccessScope, ApplicationResult, CursorPage, IdempotencyKey, OperationFingerprint, PageRequest,
};
use crate::map_domain_error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendJournalEvent {
    scope: AccessScope,
    target_owner: OwnerRef,
    run_id: Ulid,
    expected_next_sequence: u64,
    event: RunJournal,
    idempotency_key: IdempotencyKey,
    fingerprint: OperationFingerprint,
}

impl AppendJournalEvent {
    /// Creates a run-owned, expected-sequence journal append command.
    ///
    /// # Errors
    ///
    /// Returns lineage or sequence conflict when command and canonical event disagree.
    pub fn new(
        scope: AccessScope,
        target_owner: OwnerRef,
        run_id: Ulid,
        expected_next_sequence: u64,
        event: RunJournal,
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Self> {
        scope.authorize(&target_owner)?;
        if expected_next_sequence == 0 || event.sequence() != expected_next_sequence {
            return Err(map_domain_error(DomainErrorCode::JournalSequenceConflict));
        }
        if event.run_id() != &run_id {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let idempotency_key = idempotency_key.scoped_to(&scope)?;
        let mut canonical = FingerprintBuilder::new("append-journal-event/v2");
        canonical.field(2, scope.fingerprint().content_hash().as_bytes());
        canonical.field(3, &owner_bytes(&target_owner));
        canonical.field(4, run_id.as_str().as_bytes());
        canonical.u64(5, expected_next_sequence);
        canonical.field(6, &journal_bytes(&event));
        let fingerprint = canonical.finish();
        Ok(Self {
            scope,
            target_owner,
            run_id,
            expected_next_sequence,
            event,
            idempotency_key,
            fingerprint,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &AccessScope {
        &self.scope
    }

    #[must_use]
    pub fn target_owner(&self) -> &OwnerRef {
        &self.target_owner
    }

    #[must_use]
    pub fn run_id(&self) -> &Ulid {
        &self.run_id
    }

    #[must_use]
    pub fn expected_next_sequence(&self) -> u64 {
        self.expected_next_sequence
    }

    #[must_use]
    pub fn event(&self) -> &RunJournal {
        &self.event
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    #[must_use]
    pub fn fingerprint(&self) -> &OperationFingerprint {
        &self.fingerprint
    }
}

#[async_trait]
pub trait RunJournalRepository: Send + Sync {
    /// Appends one canonical journal event under expected-next-sequence control.
    ///
    /// # Errors
    ///
    /// Returns an application error on sequence, hash, run, or idempotency conflict.
    async fn append(&self, command: AppendJournalEvent) -> ApplicationResult<RunJournal>;

    /// Reads an ordered stable cursor page after tenant and allowed-owner filtering.
    ///
    /// # Errors
    ///
    /// Implementations must reject scope drift and apply tenant plus allowed-owner predicates;
    /// events remain in adapter order without sorting or repair.
    async fn read(
        &self,
        scope: &AccessScope,
        run_id: Ulid,
        page: PageRequest,
    ) -> ApplicationResult<CursorPage<RunJournal>>;
}
