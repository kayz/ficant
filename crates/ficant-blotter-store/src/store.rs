use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ficant_application::ports::{
    ApplicationResult, BlotterEventJournalPort, BlotterPersistencePort, IdempotencyScopeKey,
    PollEventsOutcome, StoredIdempotencyReceipt, validate_spi_page_size,
};
use ficant_application::use_cases::blotter::{
    AccountMetadataState, BlotterEventEnvelope, DeskBlotterWorkspaceState, DraftEarmarkRecord,
    InstrumentCatalogEntry, PollEventsBatch, QuoteStoreState, SnapshotFallbackRequired,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, map_blotter_error};
use ficant_domain::blotter::{
    AccountState, BlotterConfig, BlotterState, DraftPresenceRecord, EarmarkRecord,
    InstrumentPositionState, OpeningImportReceipt, PendingMatchRecord, ReconcileAllocationRecord,
    SeqRange, SourceBatchRecord, SourceFeedState, StoredRawPacket, StoredTradeRevision,
    TradeChainState,
};
use ficant_domain::primitives::{ContentHash, Ulid};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::codec::{
    AccountDeskPolicyDto, BaseEvidenceDto, BaselineDeltaDto, BlotterEventEnvelopeDto,
    DraftEarmarkRecordDto, EarmarkRecordDto, InstrumentCatalogEntryDto, MarketQuoteTickDto,
    MarketTimeDto, OpeningBaseFullImageDto, PendingMatchRecordDto, ReconcileAllocationRecordDto,
    SeqRangeDto, SourceBatchRecordDto, TradeFillRecordDto, bucket_to_str, config_from_json,
    config_to_json, content_hash_from_blob, deserialize_json, draft_status_to_str,
    earmark_state_to_str, format_fixed_decimal, funding_precision_to_str,
    parse_fixed_decimal_storage, parse_u64_set, parse_ulid, pending_reason_to_str,
    pending_state_to_str, serialize_json, side_to_str, sqlite_i64_to_u64, stored_quote_from_row,
    trade_lifecycle_from_str, trade_lifecycle_to_str, u64_to_sqlite_i64,
};
use crate::schema::{
    SqlitePragmaSnapshot, configure_and_verify_pragmas, initialize_or_verify_schema,
    read_pragma_snapshot, validate_production_sqlite_path,
};

/// Default number of most-recent event envelopes retained in `blotter_event_journal`.
pub const DEFAULT_EVENT_RETENTION_WINDOW: u64 = 100_000;

/// Hook invoked under the single-writer lock immediately after an event is committed to `SQLite`
/// and installed in memory, enabling race-free live event subscription handoff.
pub type EventCommitHook = Arc<dyn Fn(&BlotterEventEnvelope) + Send + Sync>;

/// Deterministic fault injection modes for transactional rollback and post-commit crash recovery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StoreFaultInjection {
    #[default]
    None,
    /// Fails inside `SQLite` before `COMMIT`, rolling back `SQLite` and leaving memory untouched.
    FailBeforeSqliteCommit,
    /// Commits `SQLite` WAL transaction, then halts the store before installing the in-memory state.
    AfterSqliteCommitBeforeMemoryApply,
}

/// Result of `SqliteBlotterStore::execute_write` and `SqliteBlotterStore::execute_write_authorized`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteExecutionOutcome<T> {
    Committed { value: T, committed_seq: u64 },
    Replayed(StoredIdempotencyReceipt),
}

/// Configuration options for opening a [`SqliteBlotterStore`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqliteBlotterOpenOptions {
    pub path: Option<PathBuf>,
    pub ephemeral_in_memory: bool,
    pub config: BlotterConfig,
    pub event_retention_window: u64,
}

impl SqliteBlotterOpenOptions {
    /// Production file-backed exclusive WAL database options.
    #[must_use]
    pub fn file(path: impl Into<PathBuf>, config: BlotterConfig) -> Self {
        Self {
            path: Some(path.into()),
            ephemeral_in_memory: false,
            config,
            event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        }
    }

    /// Explicit ephemeral in-memory database options for isolated tests (reuses identical DDL).
    #[must_use]
    pub const fn ephemeral_in_memory(config: BlotterConfig) -> Self {
        Self {
            path: None,
            ephemeral_in_memory: true,
            config,
            event_retention_window: DEFAULT_EVENT_RETENTION_WINDOW,
        }
    }

    #[must_use]
    pub const fn with_event_retention_window(mut self, window: u64) -> Self {
        self.event_retention_window = window;
        self
    }
}

struct StoreInner {
    conn: Connection,
    state: DeskBlotterWorkspaceState,
    oldest_retained_seq: u64,
    event_retention_window: u64,
    next_fault: StoreFaultInjection,
    halted: bool,
    event_hook: Option<EventCommitHook>,
}

/// Single-writer exclusive WAL `SQLite` persistence engine for Desk Blotter.
pub struct SqliteBlotterStore {
    inner: Mutex<StoreInner>,
}

impl std::fmt::Debug for SqliteBlotterStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteBlotterStore").finish_non_exhaustive()
    }
}

impl SqliteBlotterStore {
    /// Opens a production file-backed exclusive WAL store at `path`.
    ///
    /// Rejects `:memory:`, all `file:` URIs, empty paths, directories, and concurrent instances.
    pub fn open_file(path: impl AsRef<Path>, config: BlotterConfig) -> ApplicationResult<Self> {
        Self::open(&SqliteBlotterOpenOptions::file(path.as_ref(), config))
    }

    /// Explicitly opens an ephemeral in-memory store for isolated tests, executing the exact same
    /// business DDL as production WAL databases.
    pub fn open_ephemeral_in_memory(config: BlotterConfig) -> ApplicationResult<Self> {
        Self::open(&SqliteBlotterOpenOptions::ephemeral_in_memory(config))
    }

    /// Opens a store with explicit options, rejecting contradictory production/in-memory switches.
    pub fn open(options: &SqliteBlotterOpenOptions) -> ApplicationResult<Self> {
        options
            .config
            .validate()
            .map_err(|e| map_blotter_error(&e))?;
        if options.event_retention_window == 0 {
            return Err(validation_failed());
        }

        // Reject contradictory mode switches.
        if options.ephemeral_in_memory && options.path.is_some() {
            return Err(validation_failed());
        }
        if !options.ephemeral_in_memory && options.path.is_none() {
            return Err(validation_failed());
        }

        // SQLITE_OPEN_URI is deliberately omitted so URI paths can never silently open in-memory.
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;

        let conn = if options.ephemeral_in_memory {
            Connection::open_in_memory_with_flags(flags).map_err(|_| storage_unavailable())?
        } else {
            let path = options.path.as_deref().ok_or_else(validation_failed)?;
            validate_production_sqlite_path(path)?;
            Connection::open_with_flags(path, flags).map_err(|_| storage_unavailable())?
        };

        configure_and_verify_pragmas(&conn, options.ephemeral_in_memory)?;
        initialize_or_verify_schema(&conn, &options.config)?;
        let (state, oldest_retained_seq) = hydrate_workspace_state(&conn)?;

        Ok(Self {
            inner: Mutex::new(StoreInner {
                conn,
                state,
                oldest_retained_seq,
                event_retention_window: options.event_retention_window,
                next_fault: StoreFaultInjection::None,
                halted: false,
                event_hook: None,
            }),
        })
    }

    /// Inspects live `SQLite` PRAGMAs and explicit `blotter_meta.schema_version`.
    pub fn inspect_pragmas(&self) -> ApplicationResult<SqlitePragmaSnapshot> {
        let guard = self.lock_healthy()?;
        read_pragma_snapshot(&guard.conn)
    }

    /// Arms a one-shot fault injection for the next commit attempt.
    pub fn inject_next_fault(&self, fault: StoreFaultInjection) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.next_fault = fault;
    }

    /// Returns `true` if the store halted after a post-commit memory installation failure.
    #[must_use]
    pub fn is_halted(&self) -> bool {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.halted
    }

    /// Registers a synchronous commit hook invoked under the writer lock upon each event commit.
    pub fn set_event_commit_hook(&self, hook: Option<EventCommitHook>) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.event_hook = hook;
    }

    /// Executes a read-only closure while holding the single-writer barrier lock, returning
    /// `(workspace_state, head_seq, oldest_retained_seq)` consistently.
    pub fn with_read_barrier<R>(
        &self,
        f: impl FnOnce(&DeskBlotterWorkspaceState, u64, u64) -> R,
    ) -> ApplicationResult<R> {
        let guard = self.lock_healthy()?;
        let head = guard.state.domain.head_seq;
        let oldest = guard.oldest_retained_seq;
        Ok(f(&guard.state, head, oldest))
    }

    /// Executes an atomic single-writer command with optional idempotency check/replay strictly
    /// before CAS, clock, and business checks.
    ///
    /// On idempotency hit with identical `request_hash`, returns `WriteExecutionOutcome::Replayed`.
    /// On idempotency hit with differing `request_hash`, returns `HashMismatch`.
    pub fn execute_write<T, F>(
        &self,
        idempotency: Option<(&IdempotencyScopeKey, &ContentHash)>,
        f: F,
    ) -> ApplicationResult<WriteExecutionOutcome<T>>
    where
        F: FnOnce(
            &DeskBlotterWorkspaceState,
        ) -> ApplicationResult<(T, ficant_application::ports::BlotterCommitPlan)>,
    {
        self.execute_write_authorized(idempotency, |_| Ok(()), f)
    }

    /// Executes an atomic single-writer command with a state-aware pre-idempotency authorization
    /// hook strictly under the exclusive single-writer mutex.
    ///
    /// Execution order under the mutex:
    /// 1. `authorize(&state)` — fails closed before any idempotency receipt lookup or replay,
    ///    eliminating TOCTOU races for state-dependent batch authorization (e.g.
    ///    `IngestMarketQuotes`) and never leaking or replaying a cached receipt on denial;
    /// 2. Optional idempotency check/replay (`WriteExecutionOutcome::Replayed` on matching
    ///    `request_hash`, `HashMismatch` on differing `request_hash`), strictly before CAS,
    ///    clock, and business checks;
    /// 3. State transition `f(&state)` and atomic `SQLite` WAL commit.
    pub fn execute_write_authorized<T, A, F>(
        &self,
        idempotency: Option<(&IdempotencyScopeKey, &ContentHash)>,
        authorize: A,
        f: F,
    ) -> ApplicationResult<WriteExecutionOutcome<T>>
    where
        A: FnOnce(&DeskBlotterWorkspaceState) -> ApplicationResult<()>,
        F: FnOnce(
            &DeskBlotterWorkspaceState,
        ) -> ApplicationResult<(T, ficant_application::ports::BlotterCommitPlan)>,
    {
        let mut guard = self.lock_healthy()?;

        authorize(&guard.state)?;

        if let Some((key, request_hash)) = idempotency
            && let Some(receipt) = query_idempotency_row(&guard.conn, key)?
        {
            if &receipt.request_hash != request_hash {
                return Err(ApplicationError::new(
                    ApplicationErrorCategory::HashMismatch,
                    false,
                ));
            }
            return Ok(WriteExecutionOutcome::Replayed(receipt));
        }

        let (value, plan) = f(&guard.state)?;
        let committed_seq = commit_plan_locked(&mut guard, &plan)?;
        Ok(WriteExecutionOutcome::Committed {
            value,
            committed_seq,
        })
    }

    fn lock_healthy(&self) -> ApplicationResult<std::sync::MutexGuard<'_, StoreInner>> {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.halted {
            return Err(storage_unavailable());
        }
        Ok(guard)
    }
}

impl BlotterPersistencePort for SqliteBlotterStore {
    fn load_workspace_state(&self) -> ApplicationResult<DeskBlotterWorkspaceState> {
        let guard = self.lock_healthy()?;
        Ok(guard.state.clone())
    }

    fn lookup_idempotency(
        &self,
        key: &IdempotencyScopeKey,
    ) -> ApplicationResult<Option<StoredIdempotencyReceipt>> {
        let guard = self.lock_healthy()?;
        query_idempotency_row(&guard.conn, key)
    }

    fn commit_transaction(
        &self,
        plan: &ficant_application::ports::BlotterCommitPlan,
    ) -> ApplicationResult<u64> {
        let mut guard = self.lock_healthy()?;
        commit_plan_locked(&mut guard, plan)
    }
}

impl BlotterEventJournalPort for SqliteBlotterStore {
    fn head_seq(&self) -> ApplicationResult<u64> {
        let guard = self.lock_healthy()?;
        Ok(guard.state.domain.head_seq)
    }

    fn oldest_retained_seq(&self) -> ApplicationResult<u64> {
        let guard = self.lock_healthy()?;
        Ok(guard.oldest_retained_seq)
    }

    fn poll_events(
        &self,
        after_seq: u64,
        max_events: u32,
        account_filter: Option<&str>,
        allowed_accounts: Option<&BTreeSet<String>>,
    ) -> ApplicationResult<PollEventsOutcome> {
        validate_spi_page_size(max_events)?;
        if let Some(acct) = account_filter
            && (acct.trim().is_empty() || acct != acct.trim())
        {
            return Err(validation_failed());
        }

        let guard = self.lock_healthy()?;
        let head_seq = guard.state.domain.head_seq;
        let oldest_retained_seq = guard.oldest_retained_seq;

        if after_seq > head_seq {
            return Err(validation_failed());
        }

        if head_seq > 0 && after_seq.saturating_add(1) < oldest_retained_seq {
            return Ok(PollEventsOutcome::Fallback(SnapshotFallbackRequired {
                requested_after_seq: after_seq,
                oldest_retained_seq,
                current_head_seq: head_seq,
                reason: "requested cursor precedes retained event journal window".to_owned(),
            }));
        }

        if after_seq == head_seq {
            return Ok(PollEventsOutcome::Batch(PollEventsBatch {
                events: Vec::new(),
                scanned_through_seq: head_seq,
                current_head_seq: head_seq,
                has_more: false,
            }));
        }

        let after_i64 = u64_to_sqlite_i64(after_seq)?;
        let mut stmt = guard
            .conn
            .prepare(
                "SELECT seq, envelope_json FROM blotter_event_journal
                 WHERE seq > ?1
                 ORDER BY seq ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt
            .query(params![after_i64])
            .map_err(|_| storage_unavailable())?;

        let max_count = usize::try_from(max_events).unwrap_or(usize::MAX);
        let mut events = Vec::new();
        let mut scanned_through_seq = after_seq;
        let mut has_more = false;

        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let seq = sqlite_i64_to_u64(row.get::<_, i64>(0).map_err(|_| storage_unavailable())?)?;
            let envelope_json: String = row.get(1).map_err(|_| storage_unavailable())?;
            let mut envelope =
                deserialize_json::<BlotterEventEnvelopeDto>(&envelope_json)?.into_app()?;

            envelope.changes.retain(|change| {
                let matches_filter =
                    account_filter.is_none_or(|filter| change.account_id == filter);
                let matches_allowed =
                    allowed_accounts.is_none_or(|allowed| allowed.contains(&change.account_id));
                matches_filter && matches_allowed
            });

            if envelope.changes.is_empty() {
                // Entire transaction invisible to caller: advance scanned_through_seq without
                // consuming page budget.
                scanned_through_seq = seq;
                continue;
            }

            if events.len() >= max_count {
                // Whole-transaction pagination: never split one seq across pages.
                has_more = true;
                break;
            }

            scanned_through_seq = seq;
            events.push(envelope);
        }

        if !has_more {
            scanned_through_seq = head_seq;
        }

        Ok(PollEventsOutcome::Batch(PollEventsBatch {
            events,
            scanned_through_seq,
            current_head_seq: head_seq,
            has_more,
        }))
    }
}

// ============================================================================
// Single-writer transactional commit
// ============================================================================

#[allow(clippy::too_many_lines)]
fn commit_plan_locked(
    inner: &mut StoreInner,
    plan: &ficant_application::ports::BlotterCommitPlan,
) -> ApplicationResult<u64> {
    let prev_head = inner.state.domain.head_seq;
    let next_head = plan.next_state.domain.head_seq;

    match &plan.event {
        Some(event) => {
            if event.seq != next_head || prev_head.checked_add(1) != Some(next_head) {
                return Err(ApplicationError::new(
                    ApplicationErrorCategory::ConcurrencyConflict,
                    true,
                ));
            }
        }
        None => {
            if next_head != prev_head {
                return Err(ApplicationError::new(
                    ApplicationErrorCategory::ConcurrencyConflict,
                    true,
                ));
            }
        }
    }

    let next_config_json = config_to_json(&plan.next_state.domain.config)?;

    let target_oldest = if next_head > inner.event_retention_window {
        next_head - inner.event_retention_window + 1
    } else {
        1
    };
    let new_oldest_retained = inner.oldest_retained_seq.max(target_oldest);

    inner
        .conn
        .execute_batch("BEGIN IMMEDIATE;")
        .map_err(|_| storage_unavailable())?;

    let write_res = (|| -> ApplicationResult<()> {
        if let Some(receipt) = &plan.idempotency_receipt
            && let Some(existing) = query_idempotency_row(&inner.conn, &receipt.key)?
        {
            if existing.request_hash != receipt.request_hash {
                return Err(ApplicationError::new(
                    ApplicationErrorCategory::HashMismatch,
                    false,
                ));
            }
            return Err(ApplicationError::new(
                ApplicationErrorCategory::AlreadyExists,
                false,
            ));
        }

        // Row-level CAS on blotter_meta singleton row.
        let meta_updated = inner
            .conn
            .execute(
                "UPDATE blotter_meta
                 SET head_seq = ?1, oldest_retained_seq = ?2, config_json = ?3
                 WHERE singleton_id = 1 AND head_seq = ?4",
                params![
                    u64_to_sqlite_i64(next_head)?,
                    u64_to_sqlite_i64(new_oldest_retained)?,
                    next_config_json,
                    u64_to_sqlite_i64(prev_head)?,
                ],
            )
            .map_err(|_| storage_unavailable())?;
        if meta_updated != 1 {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::ConcurrencyConflict,
                true,
            ));
        }

        // 1. Accounts, instruments, positions
        sync_accounts_and_positions(&inner.conn, &inner.state, &plan.next_state)?;

        // 2. Feeds, raw packets, ingested trades, effective revisions
        sync_feeds(&inner.conn, &inner.state, &plan.next_state)?;

        // 3. Earmarks, drafts, pending matches, reconcile allocations
        sync_account_objects(&inner.conn, &inner.state, &plan.next_state)?;

        // 4. Quote history & heads
        sync_quotes(&inner.conn, &inner.state.quotes, &plan.next_state.quotes)?;

        // 5. Idempotency receipt (immutable once committed)
        if let Some(receipt) = &plan.idempotency_receipt {
            inner
                .conn
                .execute(
                    "INSERT INTO blotter_idempotency_keys
                     (actor_id, rpc_name, scope_key, idempotency_key, request_hash, committed_seq, response_payload)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        receipt.key.actor_id,
                        receipt.key.rpc_name,
                        receipt.key.scope_key,
                        receipt.key.idempotency_key,
                        receipt.request_hash.as_bytes().as_slice(),
                        u64_to_sqlite_i64(receipt.committed_seq)?,
                        receipt.response_payload.as_slice(),
                    ],
                )
                .map_err(|_| storage_unavailable())?;
        }

        // 6. Event journal & affected accounts index
        if let Some(event) = &plan.event {
            let seq_i64 = u64_to_sqlite_i64(event.seq)?;
            let occurred_at_json = serialize_json(&MarketTimeDto::from_domain(&event.occurred_at))?;
            let envelope_json = serialize_json(&BlotterEventEnvelopeDto::from_app(event))?;
            inner
                .conn
                .execute(
                    "INSERT INTO blotter_event_journal
                     (seq, occurred_at_json, event_type, envelope_json)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![seq_i64, occurred_at_json, event.event_type, envelope_json],
                )
                .map_err(|_| storage_unavailable())?;

            for change in &event.changes {
                inner
                    .conn
                    .execute(
                        "INSERT INTO blotter_event_accounts (seq, account_id) VALUES (?1, ?2)",
                        params![seq_i64, change.account_id],
                    )
                    .map_err(|_| storage_unavailable())?;
            }
        }

        // 7. Retention pruning trims ONLY blotter_event_journal (never trades/receipts/allocations).
        if new_oldest_retained > inner.oldest_retained_seq {
            inner
                .conn
                .execute(
                    "DELETE FROM blotter_event_journal WHERE seq < ?1",
                    params![u64_to_sqlite_i64(new_oldest_retained)?],
                )
                .map_err(|_| storage_unavailable())?;
        }

        // Fault injection: simulate SQLite failure right before COMMIT.
        if inner.next_fault == StoreFaultInjection::FailBeforeSqliteCommit {
            inner.next_fault = StoreFaultInjection::None;
            return Err(storage_unavailable());
        }

        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = inner.conn.execute_batch("ROLLBACK;");
        return Err(err);
    }

    if inner.conn.execute_batch("COMMIT;").is_err() {
        let _ = inner.conn.execute_batch("ROLLBACK;");
        return Err(storage_unavailable());
    }

    // Fault injection: SQLite WAL committed, simulate crash before in-memory installation.
    // The store immediately halts and refuses all further operations until restarted from WAL.
    if inner.next_fault == StoreFaultInjection::AfterSqliteCommitBeforeMemoryApply {
        inner.next_fault = StoreFaultInjection::None;
        inner.halted = true;
        return Err(storage_unavailable());
    }

    inner.state = plan.next_state.clone();
    inner.oldest_retained_seq = new_oldest_retained;

    if let (Some(hook), Some(event)) = (&inner.event_hook, &plan.event) {
        hook(event);
    }

    Ok(next_head)
}

#[allow(clippy::too_many_lines)]
fn sync_accounts_and_positions(
    conn: &Connection,
    prev: &DeskBlotterWorkspaceState,
    next: &DeskBlotterWorkspaceState,
) -> ApplicationResult<()> {
    for (acct_id, next_acct) in &next.domain.accounts {
        let next_meta = next
            .accounts_meta
            .get(acct_id)
            .ok_or_else(storage_unavailable)?;
        let prev_acct = prev.domain.accounts.get(acct_id);
        let prev_meta = prev.accounts_meta.get(acct_id);

        match prev_acct {
            None => {
                let base = &next_acct.opening_base;
                let opening_image = OpeningBaseFullImageDto::from_parts(
                    base,
                    next_meta.calendar.as_ref(),
                    next_meta.default_calendar_requirement,
                    &next_meta.units,
                );
                let policy_dto = AccountDeskPolicyDto::from_app(&next_meta.policy);
                conn.execute(
                    "INSERT INTO blotter_accounts (
                        account_id, base_id, trading_date, next_settlement_date,
                        effective_from_json, watermark_time_json, watermark_source_seq,
                        source_id, source_start_after_feed_seq,
                        opening_cash_t0, opening_net_cash_delta_t1,
                        account_version, policy_version, default_quote_source_id,
                        import_actor_id, import_idempotency_key, import_request_hash,
                        import_committed_seq, opening_base_json, policy_json
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                        ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
                     )",
                    params![
                        next_acct.account_id,
                        base.base_id.as_str(),
                        base.trading_date.to_string(),
                        base.next_settlement_date.to_string(),
                        serialize_json(&MarketTimeDto::from_domain(&base.effective_from))?,
                        serialize_json(&MarketTimeDto::from_domain(&base.watermark_time))?,
                        base.watermark_source_seq
                            .map(u64_to_sqlite_i64)
                            .transpose()?,
                        base.source_id,
                        u64_to_sqlite_i64(base.source_start_after_feed_seq)?,
                        format_fixed_decimal(base.opening_cash_t0),
                        format_fixed_decimal(base.opening_net_cash_delta_t1),
                        u64_to_sqlite_i64(next_acct.account_version)?,
                        u64_to_sqlite_i64(next_meta.policy.policy_version)?,
                        next_meta.policy.default_quote_source_id,
                        next_acct.import_receipt.actor_id,
                        next_acct.import_receipt.idempotency_key,
                        next_acct.import_receipt.request_hash.as_bytes().as_slice(),
                        u64_to_sqlite_i64(next_acct.import_receipt.committed_seq)?,
                        serialize_json(&opening_image)?,
                        serialize_json(&policy_dto)?,
                    ],
                )
                .map_err(|_| storage_unavailable())?;
            }
            Some(old_acct) => {
                let policy_changed = prev_meta.is_none_or(|pm| pm.policy != next_meta.policy);
                if old_acct.account_version != next_acct.account_version || policy_changed {
                    let policy_dto = AccountDeskPolicyDto::from_app(&next_meta.policy);
                    let updated = conn
                        .execute(
                            "UPDATE blotter_accounts
                             SET account_version = ?1,
                                 policy_version = ?2,
                                 default_quote_source_id = ?3,
                                 policy_json = ?4
                             WHERE account_id = ?5 AND account_version = ?6",
                            params![
                                u64_to_sqlite_i64(next_acct.account_version)?,
                                u64_to_sqlite_i64(next_meta.policy.policy_version)?,
                                next_meta.policy.default_quote_source_id,
                                serialize_json(&policy_dto)?,
                                acct_id,
                                u64_to_sqlite_i64(old_acct.account_version)?,
                            ],
                        )
                        .map_err(|_| storage_unavailable())?;
                    if updated != 1 {
                        return Err(ApplicationError::new(
                            ApplicationErrorCategory::ConcurrencyConflict,
                            true,
                        ));
                    }
                }
            }
        }

        // Instruments catalog & active quote source per instrument.
        let mut all_insts: BTreeSet<&str> = BTreeSet::new();
        for code in next_acct.positions.keys() {
            all_insts.insert(code.as_str());
        }
        for code in next_meta.instruments.keys() {
            all_insts.insert(code.as_str());
        }

        for inst_code in all_insts {
            let entry = next_meta.instruments.get(inst_code);
            let active_src = next_meta.policy.active_quote_source_for(inst_code);
            let prev_entry = prev_meta.and_then(|pm| pm.instruments.get(inst_code));
            let prev_active_src = prev_meta.map(|pm| pm.policy.active_quote_source_for(inst_code));
            let prev_policy_ver = prev_meta.map(|pm| pm.policy.policy_version);

            if prev_acct.is_none()
                || prev_entry != entry
                || prev_active_src != Some(active_src)
                || prev_policy_ver != Some(next_meta.policy.policy_version)
            {
                let (has_catalog, catalog_json) = match entry {
                    Some(e) => (
                        1_i64,
                        Some(serialize_json(&InstrumentCatalogEntryDto::from_app(e))?),
                    ),
                    None => (0_i64, None),
                };
                conn.execute(
                    "INSERT INTO blotter_instruments (
                        account_id, instrument_code, active_quote_source_id,
                        policy_version, has_catalog_entry, catalog_entry_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(account_id, instrument_code) DO UPDATE SET
                        active_quote_source_id = excluded.active_quote_source_id,
                        policy_version = excluded.policy_version,
                        has_catalog_entry = excluded.has_catalog_entry,
                        catalog_entry_json = excluded.catalog_entry_json",
                    params![
                        acct_id,
                        inst_code,
                        active_src,
                        u64_to_sqlite_i64(next_meta.policy.policy_version)?,
                        has_catalog,
                        catalog_json,
                    ],
                )
                .map_err(|_| storage_unavailable())?;
            }
        }

        // Positions (row-level CAS on position_version when updating).
        for (inst_code, next_pos) in &next_acct.positions {
            let prev_pos = prev_acct.and_then(|a| a.positions.get(inst_code));
            match prev_pos {
                None => {
                    conn.execute(
                        "INSERT INTO blotter_positions (
                            account_id, instrument_code, position_version,
                            opening_economic_notional, opening_available_notional,
                            opening_net_t0_notional, opening_net_t1_notional,
                            reconciled_buy_t0, reconciled_buy_t1,
                            reconciled_sell_t0, reconciled_sell_t1
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        params![
                            acct_id,
                            inst_code,
                            u64_to_sqlite_i64(next_pos.position_version)?,
                            format_fixed_decimal(next_pos.opening_economic_notional),
                            format_fixed_decimal(next_pos.opening_available_notional),
                            format_fixed_decimal(next_pos.opening_net_t0_notional),
                            format_fixed_decimal(next_pos.opening_net_t1_notional),
                            format_fixed_decimal(next_pos.reconciled_buy_t0),
                            format_fixed_decimal(next_pos.reconciled_buy_t1),
                            format_fixed_decimal(next_pos.reconciled_sell_t0),
                            format_fixed_decimal(next_pos.reconciled_sell_t1),
                        ],
                    )
                    .map_err(|_| storage_unavailable())?;
                }
                Some(old_pos) if old_pos != next_pos => {
                    let updated = conn
                        .execute(
                            "UPDATE blotter_positions
                             SET position_version = ?1,
                                 reconciled_buy_t0 = ?2,
                                 reconciled_buy_t1 = ?3,
                                 reconciled_sell_t0 = ?4,
                                 reconciled_sell_t1 = ?5
                             WHERE account_id = ?6
                               AND instrument_code = ?7
                               AND position_version = ?8",
                            params![
                                u64_to_sqlite_i64(next_pos.position_version)?,
                                format_fixed_decimal(next_pos.reconciled_buy_t0),
                                format_fixed_decimal(next_pos.reconciled_buy_t1),
                                format_fixed_decimal(next_pos.reconciled_sell_t0),
                                format_fixed_decimal(next_pos.reconciled_sell_t1),
                                acct_id,
                                inst_code,
                                u64_to_sqlite_i64(old_pos.position_version)?,
                            ],
                        )
                        .map_err(|_| storage_unavailable())?;
                    if updated != 1 {
                        return Err(ApplicationError::new(
                            ApplicationErrorCategory::ConcurrencyConflict,
                            true,
                        ));
                    }
                }
                Some(_) => {}
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn sync_feeds(
    conn: &Connection,
    prev: &DeskBlotterWorkspaceState,
    next: &DeskBlotterWorkspaceState,
) -> ApplicationResult<()> {
    for (source_id, next_feed) in &next.domain.feeds {
        let prev_feed = prev.domain.feeds.get(source_id);

        let missing_json = serialize_json(
            &next_feed
                .missing_feed_ranges
                .iter()
                .copied()
                .map(SeqRangeDto::from_domain)
                .collect::<Vec<_>>(),
        )?;
        let batches_json = serialize_json(
            &next_feed
                .source_batches
                .values()
                .copied()
                .map(SourceBatchRecordDto::from_domain)
                .collect::<Vec<_>>(),
        )?;
        let heartbeat_json = next_feed
            .last_heartbeat_at
            .as_ref()
            .map(|t| serialize_json(&MarketTimeDto::from_domain(t)))
            .transpose()?;

        match prev_feed {
            None => {
                conn.execute(
                    "INSERT INTO blotter_feed_status (
                        source_id, feed_status_revision, initial_after_feed_seq,
                        contiguous_feed_seq, max_seen_feed_seq, missing_feed_ranges_json,
                        paused_by_gap, paused_by_staleness, paused_by_operator,
                        last_heartbeat_at_json, status_reason, source_batches_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        source_id,
                        u64_to_sqlite_i64(next_feed.feed_status_revision)?,
                        u64_to_sqlite_i64(next_feed.initial_after_feed_seq)?,
                        u64_to_sqlite_i64(next_feed.contiguous_feed_seq)?,
                        u64_to_sqlite_i64(next_feed.max_seen_feed_seq)?,
                        missing_json,
                        i64::from(next_feed.paused_by_gap),
                        i64::from(next_feed.paused_by_staleness),
                        i64::from(next_feed.paused_by_operator),
                        heartbeat_json,
                        next_feed.status_reason,
                        batches_json,
                    ],
                )
                .map_err(|_| storage_unavailable())?;
            }
            Some(old_feed) if feed_header_changed(old_feed, next_feed) => {
                let updated = conn
                    .execute(
                        "UPDATE blotter_feed_status
                         SET feed_status_revision = ?1,
                             contiguous_feed_seq = ?2,
                             max_seen_feed_seq = ?3,
                             missing_feed_ranges_json = ?4,
                             paused_by_gap = ?5,
                             paused_by_staleness = ?6,
                             paused_by_operator = ?7,
                             last_heartbeat_at_json = ?8,
                             status_reason = ?9,
                             source_batches_json = ?10
                         WHERE source_id = ?11 AND feed_status_revision = ?12",
                        params![
                            u64_to_sqlite_i64(next_feed.feed_status_revision)?,
                            u64_to_sqlite_i64(next_feed.contiguous_feed_seq)?,
                            u64_to_sqlite_i64(next_feed.max_seen_feed_seq)?,
                            missing_json,
                            i64::from(next_feed.paused_by_gap),
                            i64::from(next_feed.paused_by_staleness),
                            i64::from(next_feed.paused_by_operator),
                            heartbeat_json,
                            next_feed.status_reason,
                            batches_json,
                            source_id,
                            u64_to_sqlite_i64(old_feed.feed_status_revision)?,
                        ],
                    )
                    .map_err(|_| storage_unavailable())?;
                if updated != 1 {
                    return Err(ApplicationError::new(
                        ApplicationErrorCategory::ConcurrencyConflict,
                        true,
                    ));
                }
            }
            Some(_) => {}
        }

        // Raw packets (immutable append-only per (source_id, feed_seq)).
        for (&feed_seq, pkt) in &next_feed.raw_packets {
            let already = prev_feed.is_some_and(|pf| pf.raw_packets.contains_key(&feed_seq));
            if !already {
                conn.execute(
                    "INSERT INTO blotter_feed_raw_packets (
                        source_id, feed_seq, external_trade_id, source_revision,
                        payload_hash, business_hash, batch_first_feed_seq,
                        batch_last_feed_seq, record_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        source_id,
                        u64_to_sqlite_i64(feed_seq)?,
                        pkt.external_trade_id,
                        u64_to_sqlite_i64(pkt.source_revision)?,
                        pkt.payload_hash.as_bytes().as_slice(),
                        pkt.business_hash.as_bytes().as_slice(),
                        u64_to_sqlite_i64(pkt.batch_first_feed_seq)?,
                        u64_to_sqlite_i64(pkt.batch_last_feed_seq)?,
                        serialize_json(&TradeFillRecordDto::from_domain(&pkt.record))?,
                    ],
                )
                .map_err(|_| storage_unavailable())?;
            }
        }

        // Ingested trade revisions.
        for ((trade_id, rev), stored) in &next_feed.trade_revisions {
            let prev_stored =
                prev_feed.and_then(|pf| pf.trade_revisions.get(&(trade_id.clone(), *rev)));
            if prev_stored != Some(stored) {
                conn.execute(
                    "INSERT INTO blotter_ingested_trades (
                        source_id, external_trade_id, source_revision,
                        business_hash, canonical_feed_seq, alias_feed_seqs_json,
                        lifecycle, record_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(source_id, external_trade_id, source_revision) DO UPDATE SET
                        alias_feed_seqs_json = excluded.alias_feed_seqs_json,
                        lifecycle = excluded.lifecycle,
                        record_json = excluded.record_json",
                    params![
                        source_id,
                        trade_id,
                        u64_to_sqlite_i64(*rev)?,
                        stored.business_hash.as_bytes().as_slice(),
                        u64_to_sqlite_i64(stored.canonical_feed_seq)?,
                        serialize_json(&stored.alias_feed_seqs)?,
                        trade_lifecycle_to_str(stored.lifecycle),
                        serialize_json(&TradeFillRecordDto::from_domain(&stored.record))?,
                    ],
                )
                .map_err(|_| storage_unavailable())?;
            }
        }

        // Per-trade effective revision pointers & baseline evidence.
        for (trade_id, chain) in &next_feed.trade_chains {
            let prev_chain = prev_feed.and_then(|pf| pf.trade_chains.get(trade_id));
            if prev_chain != Some(chain) {
                let orig_new_json = chain
                    .original_new_fill
                    .as_ref()
                    .map(|r| serialize_json(&TradeFillRecordDto::from_domain(r)))
                    .transpose()?;
                let base_ev_json = chain
                    .baseline_original_evidence
                    .as_ref()
                    .map(|e| serialize_json(&BaseEvidenceDto::from_domain(e)))
                    .transpose()?;
                conn.execute(
                    "INSERT INTO blotter_trade_effective_revisions (
                        source_id, external_trade_id, latest_effective_revision,
                        original_new_fill_json, baseline_original_evidence_json,
                        held_revisions_json, missing_predecessor_revisions_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(source_id, external_trade_id) DO UPDATE SET
                        latest_effective_revision = excluded.latest_effective_revision,
                        original_new_fill_json = excluded.original_new_fill_json,
                        baseline_original_evidence_json = excluded.baseline_original_evidence_json,
                        held_revisions_json = excluded.held_revisions_json,
                        missing_predecessor_revisions_json = excluded.missing_predecessor_revisions_json",
                    params![
                        source_id,
                        trade_id,
                        chain
                            .latest_effective_revision
                            .map(u64_to_sqlite_i64)
                            .transpose()?,
                        orig_new_json,
                        base_ev_json,
                        serialize_json(&chain.held_revisions)?,
                        serialize_json(&chain.missing_predecessor_revisions)?,
                    ],
                )
                .map_err(|_| storage_unavailable())?;
            }
        }
    }

    Ok(())
}

fn feed_header_changed(old: &SourceFeedState, new: &SourceFeedState) -> bool {
    old.feed_status_revision != new.feed_status_revision
        || old.contiguous_feed_seq != new.contiguous_feed_seq
        || old.max_seen_feed_seq != new.max_seen_feed_seq
        || old.missing_feed_ranges != new.missing_feed_ranges
        || old.paused_by_gap != new.paused_by_gap
        || old.paused_by_staleness != new.paused_by_staleness
        || old.paused_by_operator != new.paused_by_operator
        || old.last_heartbeat_at != new.last_heartbeat_at
        || old.status_reason != new.status_reason
        || old.source_batches != new.source_batches
}

#[allow(clippy::too_many_lines)]
fn sync_account_objects(
    conn: &Connection,
    prev: &DeskBlotterWorkspaceState,
    next: &DeskBlotterWorkspaceState,
) -> ApplicationResult<()> {
    for (acct_id, next_acct) in &next.domain.accounts {
        let prev_acct = prev.domain.accounts.get(acct_id);
        let next_meta = next
            .accounts_meta
            .get(acct_id)
            .ok_or_else(storage_unavailable)?;
        let prev_meta = prev.accounts_meta.get(acct_id);

        // 1. Earmarks (row-level CAS on earmark_revision when updating).
        for (earmark_id, next_em) in &next_acct.earmarks {
            let prev_em = prev_acct.and_then(|a| a.earmarks.get(earmark_id));
            match prev_em {
                None => {
                    conn.execute(
                        "INSERT INTO blotter_earmarks (
                            earmark_id, account_id, instrument_code, earmark_revision,
                            side, settlement_bucket, settlement_date, state,
                            notional, unreconciled_notional, locked_settlement_cash,
                            unreconciled_settlement_cash, funding_precision,
                            created_by, confirmed_by_trader_id,
                            created_seq, updated_seq, record_json
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                        params![
                            earmark_id.as_str(),
                            acct_id,
                            next_em.instrument_code,
                            u64_to_sqlite_i64(next_em.earmark_revision)?,
                            side_to_str(next_em.side),
                            bucket_to_str(next_em.settlement_bucket),
                            next_em.settlement_date.to_string(),
                            earmark_state_to_str(next_em.state),
                            format_fixed_decimal(next_em.notional),
                            format_fixed_decimal(next_em.unreconciled_notional),
                            next_em.locked_settlement_cash.map(format_fixed_decimal),
                            next_em.unreconciled_settlement_cash.map(format_fixed_decimal),
                            funding_precision_to_str(next_em.funding_precision),
                            next_em.created_by,
                            next_em.confirmed_by_trader_id,
                            u64_to_sqlite_i64(next_em.created_seq)?,
                            u64_to_sqlite_i64(next_em.updated_seq)?,
                            serialize_json(&EarmarkRecordDto::from_domain(next_em))?,
                        ],
                    )
                    .map_err(|_| storage_unavailable())?;
                }
                Some(old_em) if old_em != next_em => {
                    let updated = conn
                        .execute(
                            "UPDATE blotter_earmarks
                             SET earmark_revision = ?1,
                                 state = ?2,
                                 unreconciled_notional = ?3,
                                 locked_settlement_cash = ?4,
                                 unreconciled_settlement_cash = ?5,
                                 funding_precision = ?6,
                                 confirmed_by_trader_id = ?7,
                                 updated_seq = ?8,
                                 record_json = ?9
                             WHERE earmark_id = ?10 AND earmark_revision = ?11",
                            params![
                                u64_to_sqlite_i64(next_em.earmark_revision)?,
                                earmark_state_to_str(next_em.state),
                                format_fixed_decimal(next_em.unreconciled_notional),
                                next_em.locked_settlement_cash.map(format_fixed_decimal),
                                next_em
                                    .unreconciled_settlement_cash
                                    .map(format_fixed_decimal),
                                funding_precision_to_str(next_em.funding_precision),
                                next_em.confirmed_by_trader_id,
                                u64_to_sqlite_i64(next_em.updated_seq)?,
                                serialize_json(&EarmarkRecordDto::from_domain(next_em))?,
                                earmark_id.as_str(),
                                u64_to_sqlite_i64(old_em.earmark_revision)?,
                            ],
                        )
                        .map_err(|_| storage_unavailable())?;
                    if updated != 1 {
                        return Err(ApplicationError::new(
                            ApplicationErrorCategory::ConcurrencyConflict,
                            true,
                        ));
                    }
                }
                Some(_) => {}
            }
        }

        // 2. Draft earmarks (combining L1 presence & L2 full DraftEarmarkRecord).
        for (draft_id, presence) in &next_acct.drafts {
            let full_draft = next_meta.drafts.get(draft_id);
            let prev_presence = prev_acct.and_then(|a| a.drafts.get(draft_id));
            let prev_full = prev_meta.and_then(|m| m.drafts.get(draft_id));

            if prev_presence != Some(presence) || prev_full != full_draft {
                let (rev, status_str, token_bytes, ctx_bytes, confirmed_by, record_json) =
                    if let Some(d) = full_draft {
                        (
                            d.draft_revision,
                            draft_status_to_str(d.status),
                            *d.confirmation_token.as_bytes(),
                            *d.context_hash.as_bytes(),
                            d.confirmed_by_trader_id.as_deref(),
                            Some(serialize_json(&DraftEarmarkRecordDto::from_app(d))?),
                        )
                    } else {
                        (
                            1_u64,
                            "PENDING_CONFIRMATION",
                            [0_u8; 32],
                            [0_u8; 32],
                            None,
                            None,
                        )
                    };

                conn.execute(
                    "INSERT INTO blotter_draft_earmarks (
                        draft_id, account_id, instrument_code, draft_revision,
                        status, created_seq, confirmation_token, context_hash,
                        confirmed_by_trader_id, record_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT(draft_id) DO UPDATE SET
                        draft_revision = excluded.draft_revision,
                        status = excluded.status,
                        confirmation_token = excluded.confirmation_token,
                        context_hash = excluded.context_hash,
                        confirmed_by_trader_id = excluded.confirmed_by_trader_id,
                        record_json = excluded.record_json",
                    params![
                        draft_id.as_str(),
                        acct_id,
                        presence.instrument_code,
                        u64_to_sqlite_i64(rev)?,
                        status_str,
                        u64_to_sqlite_i64(presence.created_seq)?,
                        token_bytes.as_slice(),
                        ctx_bytes.as_slice(),
                        confirmed_by,
                        record_json,
                    ],
                )
                .map_err(|_| storage_unavailable())?;
            }
        }

        // 3. Pending matches (row-level CAS on pending_revision when updating).
        for (pm_id, next_pm) in &next_acct.pending_matches {
            let prev_pm = prev_acct.and_then(|a| a.pending_matches.get(pm_id));
            match prev_pm {
                None => {
                    conn.execute(
                        "INSERT INTO blotter_pending_matches (
                            pending_match_id, account_id, instrument_code, pending_revision,
                            source_id, external_trade_id, source_revision,
                            state, reason, original_notional, unresolved_notional,
                            unresolved_signed_cash, resolution_actor_id, resolution_reason,
                            created_seq, updated_seq, record_json
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                        params![
                            pm_id.as_str(),
                            acct_id,
                            next_pm.instrument_code,
                            u64_to_sqlite_i64(next_pm.pending_revision)?,
                            next_pm.source_id,
                            next_pm.external_trade_id,
                            u64_to_sqlite_i64(next_pm.source_revision)?,
                            pending_state_to_str(next_pm.state),
                            pending_reason_to_str(next_pm.reason),
                            format_fixed_decimal(next_pm.original_notional),
                            format_fixed_decimal(next_pm.unresolved_notional),
                            next_pm.unresolved_signed_cash.map(format_fixed_decimal),
                            next_pm.resolution_actor_id,
                            next_pm.resolution_reason,
                            u64_to_sqlite_i64(next_pm.created_seq)?,
                            u64_to_sqlite_i64(next_pm.updated_seq)?,
                            serialize_json(&PendingMatchRecordDto::from_domain(next_pm))?,
                        ],
                    )
                    .map_err(|_| storage_unavailable())?;
                }
                Some(old_pm) if old_pm != next_pm => {
                    let updated = conn
                        .execute(
                            "UPDATE blotter_pending_matches
                             SET instrument_code = ?1,
                                 pending_revision = ?2,
                                 source_revision = ?3,
                                 state = ?4,
                                 reason = ?5,
                                 original_notional = ?6,
                                 unresolved_notional = ?7,
                                 unresolved_signed_cash = ?8,
                                 resolution_actor_id = ?9,
                                 resolution_reason = ?10,
                                 updated_seq = ?11,
                                 record_json = ?12
                             WHERE pending_match_id = ?13 AND pending_revision = ?14",
                            params![
                                next_pm.instrument_code,
                                u64_to_sqlite_i64(next_pm.pending_revision)?,
                                u64_to_sqlite_i64(next_pm.source_revision)?,
                                pending_state_to_str(next_pm.state),
                                pending_reason_to_str(next_pm.reason),
                                format_fixed_decimal(next_pm.original_notional),
                                format_fixed_decimal(next_pm.unresolved_notional),
                                next_pm.unresolved_signed_cash.map(format_fixed_decimal),
                                next_pm.resolution_actor_id,
                                next_pm.resolution_reason,
                                u64_to_sqlite_i64(next_pm.updated_seq)?,
                                serialize_json(&PendingMatchRecordDto::from_domain(next_pm))?,
                                pm_id.as_str(),
                                u64_to_sqlite_i64(old_pm.pending_revision)?,
                            ],
                        )
                        .map_err(|_| storage_unavailable())?;
                    if updated != 1 {
                        return Err(ApplicationError::new(
                            ApplicationErrorCategory::ConcurrencyConflict,
                            true,
                        ));
                    }
                }
                Some(_) => {}
            }
        }

        // 4. Reconcile allocations (referencing trade revision, optional earmark, optional pending).
        for (alloc_id, next_alloc) in &next_acct.allocations {
            let prev_alloc = prev_acct.and_then(|a| a.allocations.get(alloc_id));
            if prev_alloc != Some(next_alloc) {
                let baseline_delta_json = next_alloc
                    .baseline_delta
                    .as_ref()
                    .map(|bd| serialize_json(&BaselineDeltaDto::from_domain(bd)))
                    .transpose()?;
                conn.execute(
                    "INSERT INTO blotter_reconcile_allocations (
                        allocation_id, account_id, instrument_code,
                        source_id, external_trade_id, source_revision,
                        earmark_id, pending_match_id,
                        allocated_notional, allocated_fill_cash, released_earmark_cash,
                        fill_segment_index, earmark_segment_index, reversed_by_revision,
                        actor_id, baseline_delta_json, record_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                     ON CONFLICT(allocation_id) DO UPDATE SET
                        reversed_by_revision = excluded.reversed_by_revision,
                        record_json = excluded.record_json",
                    params![
                        alloc_id.as_str(),
                        acct_id,
                        next_alloc.instrument_code,
                        next_alloc.source_id,
                        next_alloc.external_trade_id,
                        u64_to_sqlite_i64(next_alloc.source_revision)?,
                        next_alloc.earmark_id.as_ref().map(Ulid::as_str),
                        next_alloc.pending_match_id.as_ref().map(Ulid::as_str),
                        format_fixed_decimal(next_alloc.allocated_notional),
                        next_alloc.allocated_fill_cash.map(format_fixed_decimal),
                        next_alloc.released_earmark_cash.map(format_fixed_decimal),
                        next_alloc.fill_segment_index.map(i64::from),
                        next_alloc.earmark_segment_index.map(i64::from),
                        next_alloc
                            .reversed_by_revision
                            .map(u64_to_sqlite_i64)
                            .transpose()?,
                        next_alloc.actor_id,
                        baseline_delta_json,
                        serialize_json(&ReconcileAllocationRecordDto::from_domain(next_alloc))?,
                    ],
                )
                .map_err(|_| storage_unavailable())?;
            }
        }
    }

    Ok(())
}

fn sync_quotes(
    conn: &Connection,
    prev: &QuoteStoreState,
    next: &QuoteStoreState,
) -> ApplicationResult<()> {
    for ((source_id, instrument_code, quote_seq), stored) in &next.history {
        if !prev
            .history
            .contains_key(&(source_id.clone(), instrument_code.clone(), *quote_seq))
        {
            conn.execute(
                "INSERT INTO blotter_quote_history (
                    source_id, instrument_code, quote_seq,
                    payload_hash, first_received_at_json, first_ingested_at_json, tick_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    source_id,
                    instrument_code,
                    u64_to_sqlite_i64(*quote_seq)?,
                    stored.payload_hash.as_bytes().as_slice(),
                    serialize_json(&MarketTimeDto::from_domain(&stored.first_received_at))?,
                    serialize_json(&MarketTimeDto::from_domain(&stored.first_ingested_at))?,
                    serialize_json(&MarketQuoteTickDto::from_app(&stored.tick))?,
                ],
            )
            .map_err(|_| storage_unavailable())?;
        }
    }

    for ((source_id, instrument_code), &head_seq) in &next.heads {
        if prev
            .heads
            .get(&(source_id.clone(), instrument_code.clone()))
            != Some(&head_seq)
        {
            conn.execute(
                "INSERT INTO blotter_quote_heads (source_id, instrument_code, head_quote_seq)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(source_id, instrument_code) DO UPDATE SET
                    head_quote_seq = excluded.head_quote_seq",
                params![source_id, instrument_code, u64_to_sqlite_i64(head_seq)?],
            )
            .map_err(|_| storage_unavailable())?;
        }
    }

    Ok(())
}

fn query_idempotency_row(
    conn: &Connection,
    key: &IdempotencyScopeKey,
) -> ApplicationResult<Option<StoredIdempotencyReceipt>> {
    let row_opt: Option<(Vec<u8>, i64, Vec<u8>)> = conn
        .query_row(
            "SELECT request_hash, committed_seq, response_payload
             FROM blotter_idempotency_keys
             WHERE actor_id = ?1 AND rpc_name = ?2 AND scope_key = ?3 AND idempotency_key = ?4",
            params![
                key.actor_id,
                key.rpc_name,
                key.scope_key,
                key.idempotency_key
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| storage_unavailable())?;

    let Some((hash_blob, seq_i64, payload)) = row_opt else {
        return Ok(None);
    };

    Ok(Some(StoredIdempotencyReceipt {
        key: key.clone(),
        request_hash: content_hash_from_blob(&hash_blob)?,
        committed_seq: sqlite_i64_to_u64(seq_i64)?,
        response_payload: payload,
    }))
}

// ============================================================================
// Full WAL Hydration on Open / Restart Recovery
// ============================================================================

#[allow(clippy::too_many_lines)]
fn hydrate_workspace_state(
    conn: &Connection,
) -> ApplicationResult<(DeskBlotterWorkspaceState, u64)> {
    let (head_seq_i64, oldest_retained_i64, config_json): (i64, i64, String) = conn
        .query_row(
            "SELECT head_seq, oldest_retained_seq, config_json
             FROM blotter_meta WHERE singleton_id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| storage_unavailable())?;

    let head_seq = sqlite_i64_to_u64(head_seq_i64)?;
    let oldest_retained_seq = sqlite_i64_to_u64(oldest_retained_i64)?;
    let config = config_from_json(&config_json)?;

    let mut accounts: BTreeMap<String, AccountState> = BTreeMap::new();
    let mut accounts_meta: BTreeMap<String, AccountMetadataState> = BTreeMap::new();

    // 1. Accounts
    {
        let mut stmt = conn
            .prepare(
                "SELECT account_id, account_version, import_actor_id, import_idempotency_key,
                        import_request_hash, import_committed_seq, opening_base_json, policy_json
                 FROM blotter_accounts
                 ORDER BY account_id ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let account_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let account_version =
                sqlite_i64_to_u64(row.get::<_, i64>(1).map_err(|_| storage_unavailable())?)?;
            let import_actor_id: String = row.get(2).map_err(|_| storage_unavailable())?;
            let import_idempotency_key: String = row.get(3).map_err(|_| storage_unavailable())?;
            let import_hash_blob: Vec<u8> = row.get(4).map_err(|_| storage_unavailable())?;
            let import_committed_seq =
                sqlite_i64_to_u64(row.get::<_, i64>(5).map_err(|_| storage_unavailable())?)?;
            let opening_base_json: String = row.get(6).map_err(|_| storage_unavailable())?;
            let policy_json: String = row.get(7).map_err(|_| storage_unavailable())?;

            let (core_base, calendar, default_calendar_requirement, units) =
                deserialize_json::<OpeningBaseFullImageDto>(&opening_base_json)?.into_parts()?;
            let policy = deserialize_json::<AccountDeskPolicyDto>(&policy_json)?.into_app()?;

            accounts.insert(
                account_id.clone(),
                AccountState {
                    account_id: account_id.clone(),
                    account_version,
                    opening_base: core_base,
                    import_receipt: OpeningImportReceipt {
                        actor_id: import_actor_id,
                        idempotency_key: import_idempotency_key,
                        request_hash: content_hash_from_blob(&import_hash_blob)?,
                        committed_seq: import_committed_seq,
                    },
                    positions: BTreeMap::new(),
                    earmarks: BTreeMap::new(),
                    pending_matches: BTreeMap::new(),
                    allocations: BTreeMap::new(),
                    drafts: BTreeMap::new(),
                },
            );
            accounts_meta.insert(
                account_id,
                AccountMetadataState {
                    instruments: BTreeMap::new(),
                    calendar,
                    default_calendar_requirement,
                    units,
                    policy,
                    drafts: BTreeMap::new(),
                },
            );
        }
    }

    // 2. Instruments catalog
    {
        let mut stmt = conn
            .prepare(
                "SELECT account_id, instrument_code, has_catalog_entry, catalog_entry_json
                 FROM blotter_instruments
                 ORDER BY account_id ASC, instrument_code ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let account_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let inst_code: String = row.get(1).map_err(|_| storage_unavailable())?;
            let has_entry: i64 = row.get(2).map_err(|_| storage_unavailable())?;
            let entry_json: Option<String> = row.get(3).map_err(|_| storage_unavailable())?;

            if has_entry == 1 {
                let json = entry_json.ok_or_else(storage_unavailable)?;
                let entry: InstrumentCatalogEntry =
                    deserialize_json::<InstrumentCatalogEntryDto>(&json)?.into_app()?;
                let meta = accounts_meta
                    .get_mut(&account_id)
                    .ok_or_else(storage_unavailable)?;
                meta.instruments.insert(inst_code, entry);
            }
        }
    }

    // 3. Positions
    {
        let mut stmt = conn
            .prepare(
                "SELECT account_id, instrument_code, position_version,
                        opening_economic_notional, opening_available_notional,
                        opening_net_t0_notional, opening_net_t1_notional,
                        reconciled_buy_t0, reconciled_buy_t1,
                        reconciled_sell_t0, reconciled_sell_t1
                 FROM blotter_positions
                 ORDER BY account_id ASC, instrument_code ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let account_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let instrument_code: String = row.get(1).map_err(|_| storage_unavailable())?;
            let position_version =
                sqlite_i64_to_u64(row.get::<_, i64>(2).map_err(|_| storage_unavailable())?)?;
            let pos = InstrumentPositionState {
                instrument_code: instrument_code.clone(),
                position_version,
                opening_economic_notional: parse_fixed_decimal_storage(
                    &row.get::<_, String>(3).map_err(|_| storage_unavailable())?,
                )?,
                opening_available_notional: parse_fixed_decimal_storage(
                    &row.get::<_, String>(4).map_err(|_| storage_unavailable())?,
                )?,
                opening_net_t0_notional: parse_fixed_decimal_storage(
                    &row.get::<_, String>(5).map_err(|_| storage_unavailable())?,
                )?,
                opening_net_t1_notional: parse_fixed_decimal_storage(
                    &row.get::<_, String>(6).map_err(|_| storage_unavailable())?,
                )?,
                reconciled_buy_t0: parse_fixed_decimal_storage(
                    &row.get::<_, String>(7).map_err(|_| storage_unavailable())?,
                )?,
                reconciled_buy_t1: parse_fixed_decimal_storage(
                    &row.get::<_, String>(8).map_err(|_| storage_unavailable())?,
                )?,
                reconciled_sell_t0: parse_fixed_decimal_storage(
                    &row.get::<_, String>(9).map_err(|_| storage_unavailable())?,
                )?,
                reconciled_sell_t1: parse_fixed_decimal_storage(
                    &row.get::<_, String>(10)
                        .map_err(|_| storage_unavailable())?,
                )?,
            };
            let acct = accounts
                .get_mut(&account_id)
                .ok_or_else(storage_unavailable)?;
            acct.positions.insert(instrument_code, pos);
        }
    }

    // 4. Earmarks
    {
        let mut stmt = conn
            .prepare("SELECT account_id, record_json FROM blotter_earmarks ORDER BY earmark_id ASC")
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let account_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let record_json: String = row.get(1).map_err(|_| storage_unavailable())?;
            let record: EarmarkRecord =
                deserialize_json::<EarmarkRecordDto>(&record_json)?.into_domain()?;
            let acct = accounts
                .get_mut(&account_id)
                .ok_or_else(storage_unavailable)?;
            acct.earmarks.insert(record.earmark_id.clone(), record);
        }
    }

    // 5. Draft earmarks
    {
        let mut stmt = conn
            .prepare(
                "SELECT draft_id, account_id, instrument_code, created_seq, record_json
                 FROM blotter_draft_earmarks ORDER BY draft_id ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let draft_id_str: String = row.get(0).map_err(|_| storage_unavailable())?;
            let account_id: String = row.get(1).map_err(|_| storage_unavailable())?;
            let instrument_code: String = row.get(2).map_err(|_| storage_unavailable())?;
            let created_seq =
                sqlite_i64_to_u64(row.get::<_, i64>(3).map_err(|_| storage_unavailable())?)?;
            let record_json: Option<String> = row.get(4).map_err(|_| storage_unavailable())?;

            let draft_id = parse_ulid(&draft_id_str)?;
            let acct = accounts
                .get_mut(&account_id)
                .ok_or_else(storage_unavailable)?;
            acct.drafts.insert(
                draft_id.clone(),
                DraftPresenceRecord {
                    draft_id: draft_id.clone(),
                    instrument_code,
                    created_seq,
                },
            );

            if let Some(json) = record_json {
                let draft: DraftEarmarkRecord =
                    deserialize_json::<DraftEarmarkRecordDto>(&json)?.into_app()?;
                let meta = accounts_meta
                    .get_mut(&account_id)
                    .ok_or_else(storage_unavailable)?;
                meta.drafts.insert(draft_id, draft);
            }
        }
    }

    // 6. Pending matches
    {
        let mut stmt = conn
            .prepare(
                "SELECT account_id, record_json FROM blotter_pending_matches
                 ORDER BY pending_match_id ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let account_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let record_json: String = row.get(1).map_err(|_| storage_unavailable())?;
            let record: PendingMatchRecord =
                deserialize_json::<PendingMatchRecordDto>(&record_json)?.into_domain()?;
            let acct = accounts
                .get_mut(&account_id)
                .ok_or_else(storage_unavailable)?;
            acct.pending_matches
                .insert(record.pending_match_id.clone(), record);
        }
    }

    // 7. Reconcile allocations
    {
        let mut stmt = conn
            .prepare(
                "SELECT account_id, record_json FROM blotter_reconcile_allocations
                 ORDER BY allocation_id ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let account_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let record_json: String = row.get(1).map_err(|_| storage_unavailable())?;
            let record: ReconcileAllocationRecord =
                deserialize_json::<ReconcileAllocationRecordDto>(&record_json)?.into_domain()?;
            let acct = accounts
                .get_mut(&account_id)
                .ok_or_else(storage_unavailable)?;
            acct.allocations
                .insert(record.allocation_id.clone(), record);
        }
    }

    // 8. Feeds, raw packets, ingested trades, effective revisions
    let mut feeds: BTreeMap<String, SourceFeedState> = BTreeMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, feed_status_revision, initial_after_feed_seq,
                        contiguous_feed_seq, max_seen_feed_seq, missing_feed_ranges_json,
                        paused_by_gap, paused_by_staleness, paused_by_operator,
                        last_heartbeat_at_json, status_reason, source_batches_json
                 FROM blotter_feed_status ORDER BY source_id ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let source_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let missing_dtos: Vec<SeqRangeDto> =
                deserialize_json(&row.get::<_, String>(5).map_err(|_| storage_unavailable())?)?;
            let missing_feed_ranges: Vec<SeqRange> = missing_dtos
                .into_iter()
                .map(SeqRangeDto::into_domain)
                .collect();

            let last_heartbeat_at = row
                .get::<_, Option<String>>(9)
                .map_err(|_| storage_unavailable())?
                .map(|j| deserialize_json::<MarketTimeDto>(&j)?.into_domain())
                .transpose()?;

            let batch_dtos: Vec<SourceBatchRecordDto> = deserialize_json(
                &row.get::<_, String>(11)
                    .map_err(|_| storage_unavailable())?,
            )?;
            let mut source_batches = BTreeMap::new();
            for b in batch_dtos {
                let rec: SourceBatchRecord = b.into_domain();
                source_batches.insert((rec.first_feed_seq, rec.last_feed_seq), rec);
            }

            feeds.insert(
                source_id.clone(),
                SourceFeedState {
                    source_id,
                    feed_status_revision: sqlite_i64_to_u64(
                        row.get::<_, i64>(1).map_err(|_| storage_unavailable())?,
                    )?,
                    initial_after_feed_seq: sqlite_i64_to_u64(
                        row.get::<_, i64>(2).map_err(|_| storage_unavailable())?,
                    )?,
                    contiguous_feed_seq: sqlite_i64_to_u64(
                        row.get::<_, i64>(3).map_err(|_| storage_unavailable())?,
                    )?,
                    max_seen_feed_seq: sqlite_i64_to_u64(
                        row.get::<_, i64>(4).map_err(|_| storage_unavailable())?,
                    )?,
                    missing_feed_ranges,
                    paused_by_gap: row.get::<_, i64>(6).map_err(|_| storage_unavailable())? != 0,
                    paused_by_staleness: row.get::<_, i64>(7).map_err(|_| storage_unavailable())?
                        != 0,
                    paused_by_operator: row.get::<_, i64>(8).map_err(|_| storage_unavailable())?
                        != 0,
                    last_heartbeat_at,
                    status_reason: row.get(10).map_err(|_| storage_unavailable())?,
                    raw_packets: BTreeMap::new(),
                    trade_revisions: BTreeMap::new(),
                    trade_chains: BTreeMap::new(),
                    source_batches,
                },
            );
        }
    }

    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, feed_seq, external_trade_id, source_revision,
                        payload_hash, business_hash, batch_first_feed_seq,
                        batch_last_feed_seq, record_json
                 FROM blotter_feed_raw_packets ORDER BY source_id ASC, feed_seq ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let source_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let feed_seq =
                sqlite_i64_to_u64(row.get::<_, i64>(1).map_err(|_| storage_unavailable())?)?;
            let pkt = StoredRawPacket {
                feed_seq,
                external_trade_id: row.get(2).map_err(|_| storage_unavailable())?,
                source_revision: sqlite_i64_to_u64(
                    row.get::<_, i64>(3).map_err(|_| storage_unavailable())?,
                )?,
                payload_hash: content_hash_from_blob(
                    &row.get::<_, Vec<u8>>(4)
                        .map_err(|_| storage_unavailable())?,
                )?,
                business_hash: content_hash_from_blob(
                    &row.get::<_, Vec<u8>>(5)
                        .map_err(|_| storage_unavailable())?,
                )?,
                batch_first_feed_seq: sqlite_i64_to_u64(
                    row.get::<_, i64>(6).map_err(|_| storage_unavailable())?,
                )?,
                batch_last_feed_seq: sqlite_i64_to_u64(
                    row.get::<_, i64>(7).map_err(|_| storage_unavailable())?,
                )?,
                record: deserialize_json::<TradeFillRecordDto>(
                    &row.get::<_, String>(8).map_err(|_| storage_unavailable())?,
                )?
                .into_domain()?,
            };
            let feed = feeds.get_mut(&source_id).ok_or_else(storage_unavailable)?;
            feed.raw_packets.insert(feed_seq, pkt);
        }
    }

    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, external_trade_id, source_revision,
                        business_hash, canonical_feed_seq, alias_feed_seqs_json,
                        lifecycle, record_json
                 FROM blotter_ingested_trades
                 ORDER BY source_id ASC, external_trade_id ASC, source_revision ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let source_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let trade_id: String = row.get(1).map_err(|_| storage_unavailable())?;
            let rev = sqlite_i64_to_u64(row.get::<_, i64>(2).map_err(|_| storage_unavailable())?)?;
            let stored = StoredTradeRevision {
                business_hash: content_hash_from_blob(
                    &row.get::<_, Vec<u8>>(3)
                        .map_err(|_| storage_unavailable())?,
                )?,
                canonical_feed_seq: sqlite_i64_to_u64(
                    row.get::<_, i64>(4).map_err(|_| storage_unavailable())?,
                )?,
                alias_feed_seqs: deserialize_json(
                    &row.get::<_, String>(5).map_err(|_| storage_unavailable())?,
                )?,
                lifecycle: trade_lifecycle_from_str(
                    &row.get::<_, String>(6).map_err(|_| storage_unavailable())?,
                )?,
                record: deserialize_json::<TradeFillRecordDto>(
                    &row.get::<_, String>(7).map_err(|_| storage_unavailable())?,
                )?
                .into_domain()?,
            };
            let feed = feeds.get_mut(&source_id).ok_or_else(storage_unavailable)?;
            feed.trade_revisions.insert((trade_id, rev), stored);
        }
    }

    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, external_trade_id, latest_effective_revision,
                        original_new_fill_json, baseline_original_evidence_json,
                        held_revisions_json, missing_predecessor_revisions_json
                 FROM blotter_trade_effective_revisions
                 ORDER BY source_id ASC, external_trade_id ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let source_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let trade_id: String = row.get(1).map_err(|_| storage_unavailable())?;
            let latest_effective_revision = row
                .get::<_, Option<i64>>(2)
                .map_err(|_| storage_unavailable())?
                .map(sqlite_i64_to_u64)
                .transpose()?;
            let original_new_fill = row
                .get::<_, Option<String>>(3)
                .map_err(|_| storage_unavailable())?
                .map(|j| deserialize_json::<TradeFillRecordDto>(&j)?.into_domain())
                .transpose()?;
            let baseline_original_evidence = row
                .get::<_, Option<String>>(4)
                .map_err(|_| storage_unavailable())?
                .map(|j| deserialize_json::<BaseEvidenceDto>(&j)?.into_domain())
                .transpose()?;
            let held_revisions =
                parse_u64_set(&row.get::<_, String>(5).map_err(|_| storage_unavailable())?)?;
            let missing_predecessor_revisions =
                parse_u64_set(&row.get::<_, String>(6).map_err(|_| storage_unavailable())?)?;

            let feed = feeds.get_mut(&source_id).ok_or_else(storage_unavailable)?;
            feed.trade_chains.insert(
                trade_id,
                TradeChainState {
                    latest_effective_revision,
                    original_new_fill,
                    baseline_original_evidence,
                    held_revisions,
                    missing_predecessor_revisions,
                },
            );
        }
    }

    // 9. Quote history & heads
    let mut quotes = QuoteStoreState::default();
    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, instrument_code, quote_seq, payload_hash,
                        first_received_at_json, first_ingested_at_json, tick_json
                 FROM blotter_quote_history
                 ORDER BY source_id ASC, instrument_code ASC, quote_seq ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let source_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let instrument_code: String = row.get(1).map_err(|_| storage_unavailable())?;
            let quote_seq =
                sqlite_i64_to_u64(row.get::<_, i64>(2).map_err(|_| storage_unavailable())?)?;
            let hash_blob: Vec<u8> = row.get(3).map_err(|_| storage_unavailable())?;
            let first_rec_json: String = row.get(4).map_err(|_| storage_unavailable())?;
            let first_ing_json: String = row.get(5).map_err(|_| storage_unavailable())?;
            let tick_json: String = row.get(6).map_err(|_| storage_unavailable())?;

            let stored =
                stored_quote_from_row(&hash_blob, &first_rec_json, &first_ing_json, &tick_json)?;
            quotes
                .history
                .insert((source_id, instrument_code, quote_seq), stored);
        }
    }
    {
        let mut stmt = conn
            .prepare(
                "SELECT source_id, instrument_code, head_quote_seq
                 FROM blotter_quote_heads ORDER BY source_id ASC, instrument_code ASC",
            )
            .map_err(|_| storage_unavailable())?;
        let mut rows = stmt.query([]).map_err(|_| storage_unavailable())?;
        while let Some(row) = rows.next().map_err(|_| storage_unavailable())? {
            let source_id: String = row.get(0).map_err(|_| storage_unavailable())?;
            let instrument_code: String = row.get(1).map_err(|_| storage_unavailable())?;
            let head_seq =
                sqlite_i64_to_u64(row.get::<_, i64>(2).map_err(|_| storage_unavailable())?)?;
            quotes.heads.insert((source_id, instrument_code), head_seq);
        }
    }

    let workspace = DeskBlotterWorkspaceState {
        domain: BlotterState {
            head_seq,
            config,
            accounts,
            feeds,
        },
        accounts_meta,
        quotes,
    };

    Ok((workspace, oldest_retained_seq))
}

const fn validation_failed() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}

const fn storage_unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, true)
}
