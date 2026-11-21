//! Bridges `ficant_blotter_store::SqliteBlotterStore` to `ficant_api::BlotterStorePort` with a
//! single-writer post-commit broadcast hook for race-free `SubscribeEvents` handoff.

use std::collections::BTreeSet;
use std::sync::Arc;

use ficant_api::{
    BlotterStorePort, BlotterWriteOutcome, SubscribeBarrierSnapshot, WriteAuthorizeFn,
    WriteTransitionFn,
};
use ficant_application::ports::{
    ApplicationResult, BlotterEventJournalPort, IdempotencyScopeKey, PollEventsOutcome,
};
use ficant_application::use_cases::blotter::{BlotterEventEnvelope, DeskBlotterWorkspaceState};
use ficant_blotter_store::{SqliteBlotterStore, WriteExecutionOutcome};
use ficant_domain::primitives::ContentHash;
use tokio::sync::broadcast;

/// Default capacity of the in-process post-commit event broadcast bus.
pub const DEFAULT_EVENT_BUS_CAPACITY: usize = 256;

/// Runtime adapter combining an exclusive WAL [`SqliteBlotterStore`] with a post-commit
/// broadcast bus wired strictly under the store's single-writer lock.
pub struct SqliteBlotterRuntime {
    store: Arc<SqliteBlotterStore>,
    event_tx: broadcast::Sender<BlotterEventEnvelope>,
}

impl std::fmt::Debug for SqliteBlotterRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteBlotterRuntime")
            .finish_non_exhaustive()
    }
}

impl SqliteBlotterRuntime {
    /// Attaches a post-commit broadcast hook to `store` and returns the shared runtime adapter.
    #[must_use]
    pub fn new(store: Arc<SqliteBlotterStore>, event_bus_capacity: usize) -> Self {
        let cap = event_bus_capacity.max(1);
        let (event_tx, _rx) = broadcast::channel(cap);
        let hook_tx = event_tx.clone();
        store.set_event_commit_hook(Some(Arc::new(move |event: &BlotterEventEnvelope| {
            let _ = hook_tx.send(event.clone());
        })));
        Self { store, event_tx }
    }

    #[must_use]
    pub const fn store(&self) -> &Arc<SqliteBlotterStore> {
        &self.store
    }
}

impl BlotterStorePort for SqliteBlotterRuntime {
    fn execute_write_authorized_bytes(
        &self,
        idempotency: Option<(&IdempotencyScopeKey, &ContentHash)>,
        authorize: WriteAuthorizeFn,
        transition: WriteTransitionFn,
    ) -> ApplicationResult<BlotterWriteOutcome> {
        let outcome = self
            .store
            .execute_write_authorized(idempotency, authorize, transition)?;
        Ok(match outcome {
            WriteExecutionOutcome::Committed {
                value,
                committed_seq,
            } => BlotterWriteOutcome::Committed {
                response_bytes: value,
                committed_seq,
            },
            WriteExecutionOutcome::Replayed(receipt) => BlotterWriteOutcome::Replayed(receipt),
        })
    }

    fn read_workspace_snapshot(&self) -> ApplicationResult<(DeskBlotterWorkspaceState, u64, u64)> {
        self.store
            .with_read_barrier(|state, head, oldest| (state.clone(), head, oldest))
    }

    fn subscribe_with_barrier(&self) -> ApplicationResult<SubscribeBarrierSnapshot> {
        // Subscribing inside `with_read_barrier` guarantees the broadcast receiver is registered
        // under the EXACT same mutex that commits transactions and invokes `event_hook`.
        self.store.with_read_barrier(|_state, head, oldest| {
            let receiver = self.event_tx.subscribe();
            SubscribeBarrierSnapshot {
                head_seq: head,
                oldest_retained_seq: oldest,
                receiver,
            }
        })
    }

    fn poll_events(
        &self,
        after_seq: u64,
        max_events: u32,
        account_filter: Option<&str>,
        allowed_accounts: Option<&BTreeSet<String>>,
    ) -> ApplicationResult<PollEventsOutcome> {
        self.store
            .poll_events(after_seq, max_events, account_filter, allowed_accounts)
    }
}
