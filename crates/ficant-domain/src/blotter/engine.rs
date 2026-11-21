use std::collections::{BTreeMap, BTreeSet};

use chrono::Duration;
use chrono_tz::Tz;

use crate::blotter::error::{BlotterError, BlotterResult};
use crate::blotter::funding::{compute_funding_summary, resolve_settlement_cash};
use crate::blotter::model::{
    AccountBlotterSnapshot, AccountState, BlotterConfig, BlotterIdGenerator, DraftPresenceRecord,
    EarmarkRecord, EarmarkState, ExactDirtyPriceCashInput, FeedStatusView, FundingGapSummary,
    FundingPrecision, InstrumentPositionState, InstrumentPositionView, MAX_HOLD_TTL_SECONDS,
    MIN_HOLD_TTL_SECONDS, OpeningImportReceipt, OpeningPositionBase, PendingMatchRecord,
    PendingMatchState, QuoteTerms, ReconcileAllocationRecord, SettlementBucket, SourceFeedState,
    TradeFillIngestOutcome, TradeSide, require_non_empty,
};
use crate::blotter::position::{
    project_instrument_position, validate_in_term_confirm_capacity, validate_new_earmark_capacity,
};
use crate::blotter::reconcile::{
    IngestTradeFeedInput, ResolvePendingMatchInput, ingest_trade_feed_into,
    resolve_pending_match_in_account,
};
use crate::blotter::wide_math::AllocationPool;
use crate::primitives::{FixedDecimal, MarketTime, Ulid};

/// Pure in-memory immutable state of the desk-blotter domain engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlotterState {
    pub head_seq: u64,
    pub config: BlotterConfig,
    pub accounts: BTreeMap<String, AccountState>,
    pub feeds: BTreeMap<String, SourceFeedState>,
}

impl BlotterState {
    pub fn new(config: BlotterConfig) -> BlotterResult<Self> {
        config.validate()?;
        Ok(Self {
            head_seq: 0,
            config,
            accounts: BTreeMap::new(),
            feeds: BTreeMap::new(),
        })
    }

    /// Projects an immutable full snapshot of one account at `self.head_seq`.
    pub fn snapshot_account(&self, account_id: &str) -> BlotterResult<AccountBlotterSnapshot> {
        let account = self
            .accounts
            .get(account_id)
            .ok_or_else(|| BlotterError::not_found(format!("unknown account {account_id}")))?;
        let feed = self
            .feeds
            .get(&account.opening_base.source_id)
            .ok_or_else(|| BlotterError::not_found("feed source missing for account"))?;

        let funding_summary = compute_funding_summary(account, feed)?;
        let open_pending_count = count_open_pending_for_source(&self.accounts, &feed.source_id);
        let feed_status = feed.view(open_pending_count);

        let mut positions = Vec::with_capacity(account.positions.len());
        for instrument_code in account.positions.keys() {
            positions.push(project_instrument_position(account, instrument_code)?);
        }

        let active_earmarks: Vec<EarmarkRecord> = account
            .earmarks
            .values()
            .filter(|e| matches!(e.state, EarmarkState::Hold | EarmarkState::Committed))
            .cloned()
            .collect();

        let open_pending_matches: Vec<PendingMatchRecord> = account
            .pending_matches
            .values()
            .filter(|pm| pm.state == PendingMatchState::Open)
            .cloned()
            .collect();

        Ok(AccountBlotterSnapshot {
            snapshot_seq: self.head_seq,
            account_id: account.account_id.clone(),
            account_version: account.account_version,
            opening_base: account.opening_base.clone(),
            funding_summary,
            feed_status,
            positions,
            active_earmarks,
            open_pending_matches,
        })
    }

    /// Projects the feed status view for one source.
    pub fn feed_status(&self, source_id: &str) -> BlotterResult<FeedStatusView> {
        let feed = self
            .feeds
            .get(source_id)
            .ok_or_else(|| BlotterError::not_found(format!("unknown feed source {source_id}")))?;
        let open_count = count_open_pending_for_source(&self.accounts, source_id);
        Ok(feed.view(open_count))
    }

    /// Projects one instrument position view.
    pub fn position_view(
        &self,
        account_id: &str,
        instrument_code: &str,
    ) -> BlotterResult<InstrumentPositionView> {
        let account = self
            .accounts
            .get(account_id)
            .ok_or_else(|| BlotterError::not_found(format!("unknown account {account_id}")))?;
        project_instrument_position(account, instrument_code)
    }

    /// Projects the funding summary for one account.
    pub fn funding_summary(&self, account_id: &str) -> BlotterResult<FundingGapSummary> {
        let account = self
            .accounts
            .get(account_id)
            .ok_or_else(|| BlotterError::not_found(format!("unknown account {account_id}")))?;
        let feed = self
            .feeds
            .get(&account.opening_base.source_id)
            .ok_or_else(|| BlotterError::not_found("feed source missing for account"))?;
        compute_funding_summary(account, feed)
    }
}

/// Command to import an account's immutable opening position base (allowed at most once).
#[derive(Clone, Debug)]
pub struct ImportOpeningBaseCommand {
    pub actor_id: String,
    pub idempotency_key: String,
    pub opening_base: OpeningPositionBase,
    pub expected_account_version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportOpeningBaseOutput {
    pub committed_seq: u64,
    pub replayed: bool,
    pub snapshot: AccountBlotterSnapshot,
}

/// Command to create an in-term `HOLD` earmark.
#[derive(Clone, Debug)]
pub struct CreateHoldCommand {
    pub account_id: String,
    pub earmark_id: Ulid,
    pub expected_position_version: u64,
    pub instrument_code: String,
    pub side: TradeSide,
    pub settlement_bucket: SettlementBucket,
    pub notional: FixedDecimal,
    pub quote_terms: Option<QuoteTerms>,
    pub ttl_seconds: u32,
    pub note: String,
    pub trader_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateHoldOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
    pub position: InstrumentPositionView,
}

/// Command to extend an in-term `HOLD` earmark from `now`.
#[derive(Clone, Debug)]
pub struct ExtendHoldCommand {
    pub account_id: String,
    pub earmark_id: Ulid,
    pub expected_earmark_revision: u64,
    pub ttl_seconds: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendHoldOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
}

/// Command to confirm a `HOLD` (or re-confirm an `EXPIRED` earmark with latest revision) into
/// `COMMITTED`.
#[derive(Clone, Debug)]
pub struct ConfirmHoldCommand {
    pub account_id: String,
    pub earmark_id: Ulid,
    pub expected_earmark_revision: u64,
    pub expected_position_version: u64,
    pub quote_terms: QuoteTerms,
    pub exact_dirty_price_input: Option<ExactDirtyPriceCashInput>,
    pub trader_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmHoldOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
    pub position: InstrumentPositionView,
    pub funding_summary: FundingGapSummary,
    /// Present when an expired `HOLD` was lazily released into `EXPIRED` and business
    /// re-validation failed; in that case `earmark.state == EarmarkState::Expired`.
    pub reconfirmation_error: Option<BlotterError>,
}

/// Command to manually release a `HOLD` earmark.
#[derive(Clone, Debug)]
pub struct ReleaseHoldCommand {
    pub account_id: String,
    pub earmark_id: Ulid,
    pub expected_earmark_revision: u64,
    pub reason: String,
    pub actor_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseHoldOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
    pub position: InstrumentPositionView,
}

/// Command to directly create a `COMMITTED` earmark.
#[derive(Clone, Debug)]
pub struct CreateCommittedCommand {
    pub account_id: String,
    pub earmark_id: Ulid,
    pub expected_position_version: u64,
    pub instrument_code: String,
    pub side: TradeSide,
    pub settlement_bucket: SettlementBucket,
    pub notional: FixedDecimal,
    pub quote_terms: QuoteTerms,
    pub exact_dirty_price_input: Option<ExactDirtyPriceCashInput>,
    pub note: String,
    pub trader_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateCommittedOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
    pub position: InstrumentPositionView,
    pub funding_summary: FundingGapSummary,
}

/// Command to manually cancel the remaining unreconciled portion of a `COMMITTED` earmark.
#[derive(Clone, Debug)]
pub struct CancelCommittedCommand {
    pub account_id: String,
    pub earmark_id: Ulid,
    pub expected_earmark_revision: u64,
    pub reason: String,
    pub actor_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelCommittedOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
    pub position: InstrumentPositionView,
    pub funding_summary: FundingGapSummary,
}

/// Command to record a pending draft marker on an account (used by L2 draft flow & L1 tests).
#[derive(Clone, Debug)]
pub struct RegisterDraftCommand {
    pub account_id: String,
    pub draft_id: Ulid,
    pub instrument_code: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterDraftOutput {
    pub committed_seq: u64,
    pub draft: DraftPresenceRecord,
}

/// Output of `IngestTradeFeed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestTradeFeedOutput {
    pub committed_seq: u64,
    pub mutated: bool,
    pub item_outcomes: Vec<TradeFillIngestOutcome>,
    pub feed_status: FeedStatusView,
    /// Accounts whose business state changed and whose `account_version` was incremented.
    pub version_bumped_accounts: Vec<String>,
    /// Accounts bound to `source_id` whose projected shared feed status / health changed
    /// (for assembling cross-account `AccountChange[]` without bumping unrelated accounts'
    /// `account_version`).
    pub feed_status_affected_accounts: Vec<String>,
    /// Sorted unique union of `version_bumped_accounts` and `feed_status_affected_accounts`
    /// identifying all accounts whose snapshot changed in this transaction.
    pub affected_accounts: Vec<String>,
    pub changed_allocations: Vec<ReconcileAllocationRecord>,
}

/// Output of `ResolvePendingMatch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvePendingMatchOutput {
    pub committed_seq: u64,
    pub pending_match: PendingMatchRecord,
    pub updated_earmark: Option<EarmarkRecord>,
    pub position: InstrumentPositionView,
    pub funding_summary: FundingGapSummary,
    pub allocation: Option<ReconcileAllocationRecord>,
}

/// Command to sweep expired holds, overdue/pre-close committed alerts, and stale feeds.
#[derive(Clone, Debug, Default)]
pub struct SweepLifecycleCommand {
    pub account_id: Option<String>,
    pub source_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SweepLifecycleOutput {
    pub head_seq: u64,
    pub mutated: bool,
    pub expired_hold_count: u32,
    pub newly_alerted_committed_count: u32,
    pub newly_stale_feed_count: u32,
}

/// Command to toggle operator pause or force-clear gap pause on a feed source.
///
/// Force-clearing gap pause requires a non-empty audit `reason` and NEVER advances cursors,
/// clears missing ranges, or releases buffered gap packets.
#[derive(Clone, Debug)]
pub struct SetFeedControlCommand {
    pub source_id: String,
    pub expected_feed_status_revision: u64,
    pub paused_by_operator: bool,
    pub force_clear_gap_pause: bool,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetFeedControlOutput {
    pub committed_seq: u64,
    pub feed_status: FeedStatusView,
}

/// Pure transition: import an account's opening position base once.
pub fn apply_import_opening_base(
    state: &BlotterState,
    cmd: &ImportOpeningBaseCommand,
) -> BlotterResult<(BlotterState, ImportOpeningBaseOutput)> {
    require_non_empty(&cmd.actor_id, "actor_id")?;
    require_non_empty(&cmd.idempotency_key, "idempotency_key")?;
    validate_opening_base(&cmd.opening_base)?;

    let request_hash = cmd.opening_base.canonical_hash();

    if let Some(existing) = state.accounts.get(&cmd.opening_base.account_id) {
        if existing.import_receipt.actor_id == cmd.actor_id
            && existing.import_receipt.idempotency_key == cmd.idempotency_key
        {
            if existing.import_receipt.request_hash == request_hash {
                let snapshot = state.snapshot_account(&existing.account_id)?;
                return Ok((
                    state.clone(),
                    ImportOpeningBaseOutput {
                        committed_seq: existing.import_receipt.committed_seq,
                        replayed: true,
                        snapshot,
                    },
                ));
            }
            return Err(BlotterError::hash_mismatch(
                "ImportOpeningBase retry with same idempotency_key has mismatched payload",
            ));
        }
        return Err(BlotterError::immutable_violation(format!(
            "account {} already imported an opening base; re-import is forbidden",
            cmd.opening_base.account_id
        )));
    }

    if cmd.expected_account_version != 0 {
        return Err(BlotterError::concurrency_conflict(
            "initial ImportOpeningBase requires expected_account_version == 0",
        ));
    }

    if let Some(existing_feed) = state.feeds.get(&cmd.opening_base.source_id)
        && existing_feed.initial_after_feed_seq != cmd.opening_base.source_start_after_feed_seq
    {
        return Err(BlotterError::validation(format!(
            "shared feed source {} already initialized with start cursor {}, got {}",
            cmd.opening_base.source_id,
            existing_feed.initial_after_feed_seq,
            cmd.opening_base.source_start_after_feed_seq
        )));
    }

    let mut next = state.clone();
    let next_seq = next_head_seq(next.head_seq)?;

    next.feeds
        .entry(cmd.opening_base.source_id.clone())
        .or_insert_with(|| {
            SourceFeedState::new(
                cmd.opening_base.source_id.clone(),
                cmd.opening_base.source_start_after_feed_seq,
            )
        });

    let mut positions = BTreeMap::new();
    for code in &cmd.opening_base.instrument_codes {
        positions.insert(code.clone(), InstrumentPositionState::zero(code.clone()));
    }
    for bal in &cmd.opening_base.balances {
        positions.insert(
            bal.instrument_code.clone(),
            InstrumentPositionState::from_opening(bal),
        );
    }

    let account = AccountState {
        account_id: cmd.opening_base.account_id.clone(),
        account_version: 1,
        opening_base: cmd.opening_base.clone(),
        import_receipt: OpeningImportReceipt {
            actor_id: cmd.actor_id.clone(),
            idempotency_key: cmd.idempotency_key.clone(),
            request_hash,
            committed_seq: next_seq,
        },
        positions,
        earmarks: BTreeMap::new(),
        pending_matches: BTreeMap::new(),
        allocations: BTreeMap::new(),
        drafts: BTreeMap::new(),
    };

    next.accounts.insert(account.account_id.clone(), account);
    next.head_seq = next_seq;
    let snapshot = next.snapshot_account(&cmd.opening_base.account_id)?;

    Ok((
        next,
        ImportOpeningBaseOutput {
            committed_seq: next_seq,
            replayed: false,
            snapshot,
        },
    ))
}

/// Pure transition: create an in-term `HOLD` earmark.
pub fn apply_create_hold(
    state: &BlotterState,
    cmd: &CreateHoldCommand,
    now: &MarketTime,
) -> BlotterResult<(BlotterState, CreateHoldOutput)> {
    require_non_empty(&cmd.account_id, "account_id")?;
    require_non_empty(&cmd.instrument_code, "instrument_code")?;
    require_non_empty(&cmd.trader_id, "trader_id")?;
    validate_ttl(cmd.ttl_seconds)?;
    if !cmd.notional.is_positive() {
        return Err(BlotterError::validation("notional must be positive"));
    }
    if let Some(terms) = &cmd.quote_terms {
        terms.validate()?;
    }

    let account = state
        .accounts
        .get(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found(format!("unknown account {}", cmd.account_id)))?;
    if account.earmarks.contains_key(&cmd.earmark_id) {
        return Err(BlotterError::validation("duplicate earmark_id"));
    }
    let feed = state
        .feeds
        .get(&account.opening_base.source_id)
        .ok_or_else(|| BlotterError::not_found("feed source missing"))?;
    if feed.earmarks_paused() {
        return Err(BlotterError::feed_paused(format!(
            "feed source {} is paused",
            feed.source_id
        )));
    }

    let pos = account.positions.get(&cmd.instrument_code).ok_or_else(|| {
        BlotterError::not_found(format!(
            "instrument {} not found in account {}",
            cmd.instrument_code, cmd.account_id
        ))
    })?;
    if pos.position_version != cmd.expected_position_version {
        return Err(BlotterError::concurrency_conflict(
            "stale expected_position_version",
        ));
    }

    let pre_view = project_instrument_position(account, &cmd.instrument_code)?;
    validate_new_earmark_capacity(&pre_view, cmd.side, cmd.settlement_bucket, cmd.notional)?;

    let expires_at = add_seconds_market_time(now, cmd.ttl_seconds)?;
    let settlement_date = account
        .opening_base
        .settlement_date_for_bucket(cmd.settlement_bucket);

    let mut next = state.clone();
    let next_seq = next_head_seq(next.head_seq)?;
    let next_acct = next
        .accounts
        .get_mut(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found("account not found"))?;

    let earmark = EarmarkRecord {
        earmark_id: cmd.earmark_id.clone(),
        earmark_revision: 1,
        account_id: cmd.account_id.clone(),
        instrument_code: cmd.instrument_code.clone(),
        side: cmd.side,
        settlement_bucket: cmd.settlement_bucket,
        settlement_date,
        notional: cmd.notional,
        unreconciled_notional: cmd.notional,
        quote_terms: cmd.quote_terms.clone(),
        locked_settlement_cash: None,
        unreconciled_settlement_cash: None,
        funding_precision: FundingPrecision::UnpricedYieldOnly,
        state: EarmarkState::Hold,
        created_by: cmd.trader_id.clone(),
        created_at: now.clone(),
        expires_at: Some(expires_at),
        committed_at: None,
        overdue_alert: false,
        pre_close_alert: false,
        reopened_by_reversal: false,
        note: cmd.note.clone(),
        created_seq: next_seq,
        updated_seq: next_seq,
        confirmed_by_trader_id: None,
        cancel_actor_id: None,
        cancel_reason: None,
        allocation_pool: AllocationPool::empty(),
    };

    next_acct
        .earmarks
        .insert(cmd.earmark_id.clone(), earmark.clone());
    next_acct
        .positions
        .get_mut(&cmd.instrument_code)
        .ok_or_else(|| BlotterError::not_found("position not found"))?
        .position_version += 1;
    next_acct.account_version += 1;
    next.head_seq = next_seq;

    let position = next.position_view(&cmd.account_id, &cmd.instrument_code)?;
    Ok((
        next,
        CreateHoldOutput {
            committed_seq: next_seq,
            earmark,
            position,
        },
    ))
}

/// Pure transition: extend an in-term `HOLD` earmark from `now`.
pub fn apply_extend_hold(
    state: &BlotterState,
    cmd: &ExtendHoldCommand,
    now: &MarketTime,
) -> BlotterResult<(BlotterState, ExtendHoldOutput)> {
    require_non_empty(&cmd.account_id, "account_id")?;
    validate_ttl(cmd.ttl_seconds)?;

    let account = state
        .accounts
        .get(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found(format!("unknown account {}", cmd.account_id)))?;
    let earmark = account
        .earmarks
        .get(&cmd.earmark_id)
        .ok_or_else(|| BlotterError::not_found("earmark not found"))?;
    if earmark.earmark_revision != cmd.expected_earmark_revision {
        return Err(BlotterError::concurrency_conflict(
            "stale expected_earmark_revision",
        ));
    }
    if earmark.state != EarmarkState::Hold {
        return Err(BlotterError::invalid_state_transition(
            "ExtendHold requires HOLD state",
        ));
    }
    let expires_at = earmark
        .expires_at
        .as_ref()
        .ok_or_else(|| BlotterError::invalid_state_transition("HOLD missing expires_at"))?;
    if now.instant() >= expires_at.instant() {
        return Err(BlotterError::invalid_state_transition(
            "cannot extend an expired HOLD (now >= expires_at)",
        ));
    }

    let new_expires = add_seconds_market_time(now, cmd.ttl_seconds)?;

    let mut next = state.clone();
    let next_seq = next_head_seq(next.head_seq)?;
    let next_acct = next
        .accounts
        .get_mut(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found("account not found"))?;
    let next_earmark = next_acct
        .earmarks
        .get_mut(&cmd.earmark_id)
        .ok_or_else(|| BlotterError::not_found("earmark not found"))?;
    next_earmark.expires_at = Some(new_expires);
    next_earmark.earmark_revision += 1;
    next_earmark.updated_seq = next_seq;
    let updated = next_earmark.clone();

    next_acct.account_version += 1;
    next.head_seq = next_seq;

    Ok((
        next,
        ExtendHoldOutput {
            committed_seq: next_seq,
            earmark: updated,
        },
    ))
}

/// Pure transition: confirm a `HOLD` (in-term or expired lazy-release) or already-swept `EXPIRED`
/// earmark into `COMMITTED`.
///
/// When an un-swept `HOLD` has `now >= expires_at`:
/// - First releases the old hold (`HOLD -> EXPIRED`, bumping `earmark_revision` and `position_version`).
/// - Re-validates as a fresh application.
/// - If re-validation succeeds, commits `COMMITTED` in the same transaction (`reconfirmation_error: None`).
/// - If re-validation fails, commits `EXPIRED` in this transaction and returns
///   `Ok((next_state, ConfirmHoldOutput { reconfirmation_error: Some(err), .. }))`.
#[allow(clippy::too_many_lines)]
pub fn apply_confirm_hold(
    state: &BlotterState,
    cmd: &ConfirmHoldCommand,
    now: &MarketTime,
) -> BlotterResult<(BlotterState, ConfirmHoldOutput)> {
    require_non_empty(&cmd.account_id, "account_id")?;
    require_non_empty(&cmd.trader_id, "trader_id")?;
    cmd.quote_terms.validate()?;
    if let Some(exact) = cmd.exact_dirty_price_input {
        exact.validate()?;
    }

    let account = state
        .accounts
        .get(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found(format!("unknown account {}", cmd.account_id)))?;
    let feed = state
        .feeds
        .get(&account.opening_base.source_id)
        .ok_or_else(|| BlotterError::not_found("feed source missing"))?;
    let earmark = account
        .earmarks
        .get(&cmd.earmark_id)
        .ok_or_else(|| BlotterError::not_found("earmark not found"))?;

    if earmark.earmark_revision != cmd.expected_earmark_revision {
        return Err(BlotterError::concurrency_conflict(
            "stale expected_earmark_revision",
        ));
    }
    let pos = account
        .positions
        .get(&earmark.instrument_code)
        .ok_or_else(|| BlotterError::not_found("position not found"))?;
    if pos.position_version != cmd.expected_position_version {
        return Err(BlotterError::concurrency_conflict(
            "stale expected_position_version",
        ));
    }

    match earmark.state {
        EarmarkState::Hold => {
            let expires_at = earmark.expires_at.as_ref().ok_or_else(|| {
                BlotterError::invalid_state_transition("HOLD earmark missing expires_at")
            })?;
            let is_expired = now.instant() >= expires_at.instant();

            if is_expired {
                // Lazy expiration release is an independent committed state change.
                let mut next = state.clone();
                let next_seq = next_head_seq(next.head_seq)?;
                let instrument_code = earmark.instrument_code.clone();
                let side = earmark.side;
                let bucket = earmark.settlement_bucket;
                let notional = earmark.notional;

                {
                    let next_acct = next
                        .accounts
                        .get_mut(&cmd.account_id)
                        .ok_or_else(|| BlotterError::not_found("account not found"))?;
                    let next_earmark = next_acct
                        .earmarks
                        .get_mut(&cmd.earmark_id)
                        .ok_or_else(|| BlotterError::not_found("earmark not found"))?;
                    next_earmark.state = EarmarkState::Expired;
                    next_earmark.unreconciled_notional = FixedDecimal::ZERO;
                    next_earmark.earmark_revision += 1;
                    next_earmark.updated_seq = next_seq;
                    next_acct
                        .positions
                        .get_mut(&instrument_code)
                        .ok_or_else(|| BlotterError::not_found("position not found"))?
                        .position_version += 1;
                    next_acct.account_version += 1;
                }
                next.head_seq = next_seq;

                // Re-validate as a fresh application after releasing the old hold.
                let revalidation_result = if feed.earmarks_paused() {
                    Err(BlotterError::feed_paused(format!(
                        "feed source {} is paused",
                        feed.source_id
                    )))
                } else {
                    let released_view = next.position_view(&cmd.account_id, &instrument_code)?;
                    validate_new_earmark_capacity(&released_view, side, bucket, notional)
                };

                match revalidation_result {
                    Ok(()) => {
                        let resolved = resolve_settlement_cash(
                            side,
                            notional,
                            Some(&cmd.quote_terms),
                            cmd.exact_dirty_price_input,
                        )?;
                        let pool = match resolved.signed_cash {
                            Some(cash) => AllocationPool::new(notional, cash)?,
                            None => AllocationPool::empty(),
                        };
                        let next_acct = next
                            .accounts
                            .get_mut(&cmd.account_id)
                            .ok_or_else(|| BlotterError::not_found("account not found"))?;
                        let next_earmark = next_acct
                            .earmarks
                            .get_mut(&cmd.earmark_id)
                            .ok_or_else(|| BlotterError::not_found("earmark not found"))?;
                        next_earmark.state = EarmarkState::Committed;
                        next_earmark.unreconciled_notional = notional;
                        next_earmark.quote_terms = Some(cmd.quote_terms.clone());
                        next_earmark.locked_settlement_cash = resolved.signed_cash;
                        next_earmark.unreconciled_settlement_cash = resolved.signed_cash;
                        next_earmark.funding_precision = resolved.precision;
                        next_earmark.committed_at = Some(now.clone());
                        next_earmark.confirmed_by_trader_id = Some(cmd.trader_id.clone());
                        next_earmark.allocation_pool = pool;
                        let final_earmark = next_earmark.clone();

                        let position = next.position_view(&cmd.account_id, &instrument_code)?;
                        let funding_summary = next.funding_summary(&cmd.account_id)?;
                        Ok((
                            next,
                            ConfirmHoldOutput {
                                committed_seq: next_seq,
                                earmark: final_earmark,
                                position,
                                funding_summary,
                                reconfirmation_error: None,
                            },
                        ))
                    }
                    Err(reconfirm_err) => {
                        let expired_earmark = next
                            .accounts
                            .get(&cmd.account_id)
                            .and_then(|a| a.earmarks.get(&cmd.earmark_id))
                            .ok_or_else(|| BlotterError::not_found("expired earmark not found"))?
                            .clone();
                        let position = next.position_view(&cmd.account_id, &instrument_code)?;
                        let funding_summary = next.funding_summary(&cmd.account_id)?;
                        Ok((
                            next,
                            ConfirmHoldOutput {
                                committed_seq: next_seq,
                                earmark: expired_earmark,
                                position,
                                funding_summary,
                                reconfirmation_error: Some(reconfirm_err),
                            },
                        ))
                    }
                }
            } else {
                // In-term confirmation: failure preserves original HOLD intact.
                if feed.earmarks_paused() {
                    return Err(BlotterError::feed_paused(format!(
                        "feed source {} is paused",
                        feed.source_id
                    )));
                }
                let instrument_code = earmark.instrument_code.clone();
                let side = earmark.side;
                let bucket = earmark.settlement_bucket;
                let notional = earmark.notional;

                let pre_view = project_instrument_position(account, &instrument_code)?;
                let resolved = resolve_settlement_cash(
                    side,
                    notional,
                    Some(&cmd.quote_terms),
                    cmd.exact_dirty_price_input,
                )?;
                let pool = match resolved.signed_cash {
                    Some(cash) => AllocationPool::new(notional, cash)?,
                    None => AllocationPool::empty(),
                };

                let mut next = state.clone();
                let next_seq = next_head_seq(next.head_seq)?;
                {
                    let next_acct = next
                        .accounts
                        .get_mut(&cmd.account_id)
                        .ok_or_else(|| BlotterError::not_found("account not found"))?;
                    let next_earmark = next_acct
                        .earmarks
                        .get_mut(&cmd.earmark_id)
                        .ok_or_else(|| BlotterError::not_found("earmark not found"))?;
                    next_earmark.state = EarmarkState::Committed;
                    next_earmark.unreconciled_notional = notional;
                    next_earmark.quote_terms = Some(cmd.quote_terms.clone());
                    next_earmark.locked_settlement_cash = resolved.signed_cash;
                    next_earmark.unreconciled_settlement_cash = resolved.signed_cash;
                    next_earmark.funding_precision = resolved.precision;
                    next_earmark.committed_at = Some(now.clone());
                    next_earmark.confirmed_by_trader_id = Some(cmd.trader_id.clone());
                    next_earmark.earmark_revision += 1;
                    next_earmark.updated_seq = next_seq;
                    next_earmark.allocation_pool = pool;
                    next_acct
                        .positions
                        .get_mut(&instrument_code)
                        .ok_or_else(|| BlotterError::not_found("position not found"))?
                        .position_version += 1;
                    next_acct.account_version += 1;
                }
                next.head_seq = next_seq;

                let post_view = next.position_view(&cmd.account_id, &instrument_code)?;
                validate_in_term_confirm_capacity(&pre_view, &post_view, side, bucket)?;

                let final_earmark = next
                    .accounts
                    .get(&cmd.account_id)
                    .and_then(|a| a.earmarks.get(&cmd.earmark_id))
                    .ok_or_else(|| BlotterError::not_found("committed earmark not found"))?
                    .clone();
                let funding_summary = next.funding_summary(&cmd.account_id)?;
                Ok((
                    next,
                    ConfirmHoldOutput {
                        committed_seq: next_seq,
                        earmark: final_earmark,
                        position: post_view,
                        funding_summary,
                        reconfirmation_error: None,
                    },
                ))
            }
        }
        EarmarkState::Expired => {
            // Already-swept EXPIRED earmark confirmed with latest revision: re-validate as new.
            if feed.earmarks_paused() {
                return Err(BlotterError::feed_paused(format!(
                    "feed source {} is paused",
                    feed.source_id
                )));
            }
            let instrument_code = earmark.instrument_code.clone();
            let side = earmark.side;
            let bucket = earmark.settlement_bucket;
            let notional = earmark.notional;

            let view = project_instrument_position(account, &instrument_code)?;
            validate_new_earmark_capacity(&view, side, bucket, notional)?;

            let resolved = resolve_settlement_cash(
                side,
                notional,
                Some(&cmd.quote_terms),
                cmd.exact_dirty_price_input,
            )?;
            let pool = match resolved.signed_cash {
                Some(cash) => AllocationPool::new(notional, cash)?,
                None => AllocationPool::empty(),
            };

            let mut next = state.clone();
            let next_seq = next_head_seq(next.head_seq)?;
            {
                let next_acct = next
                    .accounts
                    .get_mut(&cmd.account_id)
                    .ok_or_else(|| BlotterError::not_found("account not found"))?;
                let next_earmark = next_acct
                    .earmarks
                    .get_mut(&cmd.earmark_id)
                    .ok_or_else(|| BlotterError::not_found("earmark not found"))?;
                next_earmark.state = EarmarkState::Committed;
                next_earmark.unreconciled_notional = notional;
                next_earmark.quote_terms = Some(cmd.quote_terms.clone());
                next_earmark.locked_settlement_cash = resolved.signed_cash;
                next_earmark.unreconciled_settlement_cash = resolved.signed_cash;
                next_earmark.funding_precision = resolved.precision;
                next_earmark.committed_at = Some(now.clone());
                next_earmark.confirmed_by_trader_id = Some(cmd.trader_id.clone());
                next_earmark.earmark_revision += 1;
                next_earmark.updated_seq = next_seq;
                next_earmark.allocation_pool = pool;
                next_acct
                    .positions
                    .get_mut(&instrument_code)
                    .ok_or_else(|| BlotterError::not_found("position not found"))?
                    .position_version += 1;
                next_acct.account_version += 1;
            }
            next.head_seq = next_seq;

            let final_earmark = next
                .accounts
                .get(&cmd.account_id)
                .and_then(|a| a.earmarks.get(&cmd.earmark_id))
                .ok_or_else(|| BlotterError::not_found("committed earmark not found"))?
                .clone();
            let position = next.position_view(&cmd.account_id, &instrument_code)?;
            let funding_summary = next.funding_summary(&cmd.account_id)?;
            Ok((
                next,
                ConfirmHoldOutput {
                    committed_seq: next_seq,
                    earmark: final_earmark,
                    position,
                    funding_summary,
                    reconfirmation_error: None,
                },
            ))
        }
        EarmarkState::Committed
        | EarmarkState::Released
        | EarmarkState::Cancelled
        | EarmarkState::Reconciled => Err(BlotterError::invalid_state_transition(format!(
            "ConfirmHold is invalid for earmark state {:?}",
            earmark.state
        ))),
    }
}

/// Pure transition: manually release a `HOLD` earmark.
pub fn apply_release_hold(
    state: &BlotterState,
    cmd: &ReleaseHoldCommand,
) -> BlotterResult<(BlotterState, ReleaseHoldOutput)> {
    require_non_empty(&cmd.account_id, "account_id")?;
    require_non_empty(&cmd.reason, "reason")?;
    require_non_empty(&cmd.actor_id, "actor_id")?;

    let account = state
        .accounts
        .get(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found(format!("unknown account {}", cmd.account_id)))?;
    let earmark = account
        .earmarks
        .get(&cmd.earmark_id)
        .ok_or_else(|| BlotterError::not_found("earmark not found"))?;
    if earmark.earmark_revision != cmd.expected_earmark_revision {
        return Err(BlotterError::concurrency_conflict(
            "stale expected_earmark_revision",
        ));
    }
    if earmark.state != EarmarkState::Hold {
        return Err(BlotterError::invalid_state_transition(
            "ReleaseHold requires HOLD state",
        ));
    }

    let instrument_code = earmark.instrument_code.clone();
    let mut next = state.clone();
    let next_seq = next_head_seq(next.head_seq)?;
    let next_acct = next
        .accounts
        .get_mut(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found("account not found"))?;
    let next_earmark = next_acct
        .earmarks
        .get_mut(&cmd.earmark_id)
        .ok_or_else(|| BlotterError::not_found("earmark not found"))?;
    next_earmark.state = EarmarkState::Released;
    next_earmark.unreconciled_notional = FixedDecimal::ZERO;
    next_earmark.cancel_actor_id = Some(cmd.actor_id.clone());
    next_earmark.cancel_reason = Some(cmd.reason.clone());
    next_earmark.earmark_revision += 1;
    next_earmark.updated_seq = next_seq;
    let updated = next_earmark.clone();

    next_acct
        .positions
        .get_mut(&instrument_code)
        .ok_or_else(|| BlotterError::not_found("position not found"))?
        .position_version += 1;
    next_acct.account_version += 1;
    next.head_seq = next_seq;

    let position = next.position_view(&cmd.account_id, &instrument_code)?;
    Ok((
        next,
        ReleaseHoldOutput {
            committed_seq: next_seq,
            earmark: updated,
            position,
        },
    ))
}

/// Pure transition: directly create a `COMMITTED` earmark.
#[allow(clippy::too_many_lines)]
pub fn apply_create_committed(
    state: &BlotterState,
    cmd: &CreateCommittedCommand,
    now: &MarketTime,
) -> BlotterResult<(BlotterState, CreateCommittedOutput)> {
    require_non_empty(&cmd.account_id, "account_id")?;
    require_non_empty(&cmd.instrument_code, "instrument_code")?;
    require_non_empty(&cmd.trader_id, "trader_id")?;
    if !cmd.notional.is_positive() {
        return Err(BlotterError::validation("notional must be positive"));
    }
    cmd.quote_terms.validate()?;
    if let Some(exact) = cmd.exact_dirty_price_input {
        exact.validate()?;
    }

    let account = state
        .accounts
        .get(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found(format!("unknown account {}", cmd.account_id)))?;
    if account.earmarks.contains_key(&cmd.earmark_id) {
        return Err(BlotterError::validation("duplicate earmark_id"));
    }
    let feed = state
        .feeds
        .get(&account.opening_base.source_id)
        .ok_or_else(|| BlotterError::not_found("feed source missing"))?;
    if feed.earmarks_paused() {
        return Err(BlotterError::feed_paused(format!(
            "feed source {} is paused",
            feed.source_id
        )));
    }
    let pos = account.positions.get(&cmd.instrument_code).ok_or_else(|| {
        BlotterError::not_found(format!(
            "instrument {} not found in account {}",
            cmd.instrument_code, cmd.account_id
        ))
    })?;
    if pos.position_version != cmd.expected_position_version {
        return Err(BlotterError::concurrency_conflict(
            "stale expected_position_version",
        ));
    }

    let pre_view = project_instrument_position(account, &cmd.instrument_code)?;
    validate_new_earmark_capacity(&pre_view, cmd.side, cmd.settlement_bucket, cmd.notional)?;

    let resolved = resolve_settlement_cash(
        cmd.side,
        cmd.notional,
        Some(&cmd.quote_terms),
        cmd.exact_dirty_price_input,
    )?;
    let pool = match resolved.signed_cash {
        Some(cash) => AllocationPool::new(cmd.notional, cash)?,
        None => AllocationPool::empty(),
    };
    let settlement_date = account
        .opening_base
        .settlement_date_for_bucket(cmd.settlement_bucket);

    let mut next = state.clone();
    let next_seq = next_head_seq(next.head_seq)?;
    let next_acct = next
        .accounts
        .get_mut(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found("account not found"))?;

    let earmark = EarmarkRecord {
        earmark_id: cmd.earmark_id.clone(),
        earmark_revision: 1,
        account_id: cmd.account_id.clone(),
        instrument_code: cmd.instrument_code.clone(),
        side: cmd.side,
        settlement_bucket: cmd.settlement_bucket,
        settlement_date,
        notional: cmd.notional,
        unreconciled_notional: cmd.notional,
        quote_terms: Some(cmd.quote_terms.clone()),
        locked_settlement_cash: resolved.signed_cash,
        unreconciled_settlement_cash: resolved.signed_cash,
        funding_precision: resolved.precision,
        state: EarmarkState::Committed,
        created_by: cmd.trader_id.clone(),
        created_at: now.clone(),
        expires_at: None,
        committed_at: Some(now.clone()),
        overdue_alert: false,
        pre_close_alert: false,
        reopened_by_reversal: false,
        note: cmd.note.clone(),
        created_seq: next_seq,
        updated_seq: next_seq,
        confirmed_by_trader_id: Some(cmd.trader_id.clone()),
        cancel_actor_id: None,
        cancel_reason: None,
        allocation_pool: pool,
    };

    next_acct
        .earmarks
        .insert(cmd.earmark_id.clone(), earmark.clone());
    next_acct
        .positions
        .get_mut(&cmd.instrument_code)
        .ok_or_else(|| BlotterError::not_found("position not found"))?
        .position_version += 1;
    next_acct.account_version += 1;
    next.head_seq = next_seq;

    let position = next.position_view(&cmd.account_id, &cmd.instrument_code)?;
    let funding_summary = next.funding_summary(&cmd.account_id)?;
    Ok((
        next,
        CreateCommittedOutput {
            committed_seq: next_seq,
            earmark,
            position,
            funding_summary,
        },
    ))
}

/// Pure transition: cancel ONLY the unreconciled remaining portion of a `COMMITTED` earmark.
pub fn apply_cancel_committed(
    state: &BlotterState,
    cmd: &CancelCommittedCommand,
) -> BlotterResult<(BlotterState, CancelCommittedOutput)> {
    require_non_empty(&cmd.account_id, "account_id")?;
    require_non_empty(&cmd.reason, "reason")?;
    require_non_empty(&cmd.actor_id, "actor_id")?;

    let account = state
        .accounts
        .get(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found(format!("unknown account {}", cmd.account_id)))?;
    let earmark = account
        .earmarks
        .get(&cmd.earmark_id)
        .ok_or_else(|| BlotterError::not_found("earmark not found"))?;
    if earmark.earmark_revision != cmd.expected_earmark_revision {
        return Err(BlotterError::concurrency_conflict(
            "stale expected_earmark_revision",
        ));
    }
    if earmark.state != EarmarkState::Committed || !earmark.unreconciled_notional.is_positive() {
        return Err(BlotterError::invalid_state_transition(
            "CancelCommitted requires COMMITTED earmark with positive unreconciled_notional",
        ));
    }

    let instrument_code = earmark.instrument_code.clone();
    let mut next = state.clone();
    let next_seq = next_head_seq(next.head_seq)?;
    let next_acct = next
        .accounts
        .get_mut(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found("account not found"))?;
    let next_earmark = next_acct
        .earmarks
        .get_mut(&cmd.earmark_id)
        .ok_or_else(|| BlotterError::not_found("earmark not found"))?;
    next_earmark.state = EarmarkState::Cancelled;
    next_earmark.unreconciled_notional = FixedDecimal::ZERO;
    if next_earmark.unreconciled_settlement_cash.is_some() {
        next_earmark.unreconciled_settlement_cash = Some(FixedDecimal::ZERO);
    }
    next_earmark.cancel_actor_id = Some(cmd.actor_id.clone());
    next_earmark.cancel_reason = Some(cmd.reason.clone());
    next_earmark.earmark_revision += 1;
    next_earmark.updated_seq = next_seq;
    let updated = next_earmark.clone();

    next_acct
        .positions
        .get_mut(&instrument_code)
        .ok_or_else(|| BlotterError::not_found("position not found"))?
        .position_version += 1;
    next_acct.account_version += 1;
    next.head_seq = next_seq;

    let position = next.position_view(&cmd.account_id, &instrument_code)?;
    let funding_summary = next.funding_summary(&cmd.account_id)?;
    Ok((
        next,
        CancelCommittedOutput {
            committed_seq: next_seq,
            earmark: updated,
            position,
            funding_summary,
        },
    ))
}

/// Pure transition: register a draft presence record on an account.
pub fn apply_register_draft(
    state: &BlotterState,
    cmd: &RegisterDraftCommand,
) -> BlotterResult<(BlotterState, RegisterDraftOutput)> {
    require_non_empty(&cmd.account_id, "account_id")?;
    require_non_empty(&cmd.instrument_code, "instrument_code")?;
    let account = state
        .accounts
        .get(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found(format!("unknown account {}", cmd.account_id)))?;
    if account.drafts.contains_key(&cmd.draft_id) {
        return Err(BlotterError::validation("duplicate draft_id"));
    }

    let mut next = state.clone();
    let next_seq = next_head_seq(next.head_seq)?;
    let draft = DraftPresenceRecord {
        draft_id: cmd.draft_id.clone(),
        instrument_code: cmd.instrument_code.clone(),
        created_seq: next_seq,
    };
    let next_acct = next
        .accounts
        .get_mut(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found("account not found"))?;
    next_acct.drafts.insert(cmd.draft_id.clone(), draft.clone());
    next_acct.account_version += 1;
    next.head_seq = next_seq;

    Ok((
        next,
        RegisterDraftOutput {
            committed_seq: next_seq,
            draft,
        },
    ))
}

/// Pure transition: ingest trade feed records/fragments with gap capture, deduplication,
/// revision-chain evaluation, and set-level 1:1 matching across complete source batches.
pub fn apply_ingest_trade_feed(
    state: &BlotterState,
    cmd: &IngestTradeFeedInput,
    now: &MarketTime,
    id_gen: &mut dyn BlotterIdGenerator,
) -> BlotterResult<(BlotterState, IngestTradeFeedOutput)> {
    let mut next = state.clone();
    let candidate_seq = next_head_seq(next.head_seq)?;
    let effect = ingest_trade_feed_into(
        &mut next.accounts,
        &mut next.feeds,
        &next.config,
        cmd,
        now,
        candidate_seq,
        id_gen,
    )?;

    if effect.mutated {
        next.head_seq = candidate_seq;
        for acct_id in &effect.version_bumped_accounts {
            if let Some(acct) = next.accounts.get_mut(acct_id) {
                acct.account_version += 1;
            }
        }
    }

    let committed_seq = next.head_seq;
    let feed_status = next.feed_status(&cmd.source_id)?;
    let mut union_affected: BTreeSet<String> = effect.version_bumped_accounts.clone();
    union_affected.extend(effect.feed_status_affected_accounts.iter().cloned());
    let version_bumped_accounts: Vec<String> = effect.version_bumped_accounts.into_iter().collect();
    let feed_status_affected_accounts: Vec<String> =
        effect.feed_status_affected_accounts.into_iter().collect();
    let affected_accounts: Vec<String> = union_affected.into_iter().collect();

    Ok((
        next,
        IngestTradeFeedOutput {
            committed_seq,
            mutated: effect.mutated,
            item_outcomes: effect.item_outcomes,
            feed_status,
            version_bumped_accounts,
            feed_status_affected_accounts,
            affected_accounts,
            changed_allocations: effect.changed_allocations,
        },
    ))
}

/// Pure transition: manually resolve an open pending match.
pub fn apply_resolve_pending_match(
    state: &BlotterState,
    cmd: &ResolvePendingMatchInput,
    id_gen: &mut dyn BlotterIdGenerator,
) -> BlotterResult<(BlotterState, ResolvePendingMatchOutput)> {
    require_non_empty(&cmd.account_id, "account_id")?;
    let source_id = state
        .accounts
        .get(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found(format!("unknown account {}", cmd.account_id)))?
        .opening_base
        .source_id
        .clone();

    let mut next = state.clone();
    let next_seq = next_head_seq(next.head_seq)?;
    let feed = next
        .feeds
        .get(&source_id)
        .ok_or_else(|| BlotterError::not_found("feed source missing for account"))?;
    let acct = next
        .accounts
        .get_mut(&cmd.account_id)
        .ok_or_else(|| BlotterError::not_found("account not found"))?;
    let effect = resolve_pending_match_in_account(acct, feed, cmd, next_seq, id_gen)?;
    acct.account_version += 1;
    next.head_seq = next_seq;

    let updated_earmark = effect
        .updated_earmark_id
        .as_ref()
        .and_then(|eid| next.accounts.get(&cmd.account_id)?.earmarks.get(eid))
        .cloned();
    let instrument_code = effect.pending_match.instrument_code.clone();
    let position = next.position_view(&cmd.account_id, &instrument_code)?;
    let funding_summary = next.funding_summary(&cmd.account_id)?;

    Ok((
        next,
        ResolvePendingMatchOutput {
            committed_seq: next_seq,
            pending_match: effect.pending_match,
            updated_earmark,
            position,
            funding_summary,
            allocation: effect.allocation,
        },
    ))
}

/// Pure transition: sweep expired `HOLD`s, overdue/pre-close alerts on `COMMITTED` earmarks
/// (NEVER auto-releasing `COMMITTED`), and stale feed sources.
///
/// Repeated sweeps with zero transitions do NOT increment `head_seq`.
#[allow(clippy::too_many_lines)]
pub fn apply_sweep_lifecycle(
    state: &BlotterState,
    cmd: &SweepLifecycleCommand,
    now: &MarketTime,
) -> BlotterResult<(BlotterState, SweepLifecycleOutput)> {
    if let Some(acct_id) = &cmd.account_id
        && !state.accounts.contains_key(acct_id)
    {
        return Err(BlotterError::not_found(format!(
            "unknown account {acct_id}"
        )));
    }
    if let Some(src_id) = &cmd.source_id
        && !state.feeds.contains_key(src_id)
    {
        return Err(BlotterError::not_found(format!(
            "unknown feed source {src_id}"
        )));
    }

    let mut next = state.clone();
    let candidate_seq = next_head_seq(next.head_seq)?;
    let overdue_duration = Duration::seconds(i64::from(next.config.committed_overdue_seconds));
    let local_time_now = local_time_of(now)?;

    let mut expired_hold_count = 0_u32;
    let mut newly_alerted_committed_count = 0_u32;
    let mut newly_stale_feed_count = 0_u32;

    let account_ids: Vec<String> = match &cmd.account_id {
        Some(id) => vec![id.clone()],
        None => next.accounts.keys().cloned().collect(),
    };

    for acct_id in account_ids {
        let acct = next
            .accounts
            .get_mut(&acct_id)
            .ok_or_else(|| BlotterError::not_found("account not found"))?;
        let mut account_changed = false;
        let mut touched_instruments: BTreeSet<String> = BTreeSet::new();

        for earmark in acct.earmarks.values_mut() {
            match earmark.state {
                EarmarkState::Hold => {
                    if let Some(expires_at) = &earmark.expires_at
                        && now.instant() >= expires_at.instant()
                    {
                        earmark.state = EarmarkState::Expired;
                        earmark.unreconciled_notional = FixedDecimal::ZERO;
                        earmark.earmark_revision += 1;
                        earmark.updated_seq = candidate_seq;
                        expired_hold_count = expired_hold_count.saturating_add(1);
                        touched_instruments.insert(earmark.instrument_code.clone());
                        account_changed = true;
                    }
                }
                EarmarkState::Committed => {
                    // COMMITTED earmarks are NEVER auto-released; only alert flags are raised.
                    let mut newly_alerted = false;
                    if !earmark.overdue_alert
                        && let Some(committed_at) = &earmark.committed_at
                        && now.instant().signed_duration_since(committed_at.instant())
                            >= overdue_duration
                    {
                        earmark.overdue_alert = true;
                        newly_alerted = true;
                    }
                    if !earmark.pre_close_alert
                        && local_time_now >= next.config.pre_close_alert_time
                    {
                        earmark.pre_close_alert = true;
                        newly_alerted = true;
                    }
                    if newly_alerted {
                        earmark.earmark_revision += 1;
                        earmark.updated_seq = candidate_seq;
                        newly_alerted_committed_count =
                            newly_alerted_committed_count.saturating_add(1);
                        account_changed = true;
                    }
                }
                EarmarkState::Expired
                | EarmarkState::Released
                | EarmarkState::Cancelled
                | EarmarkState::Reconciled => {}
            }
        }

        for inst in touched_instruments {
            if let Some(pos) = acct.positions.get_mut(&inst) {
                pos.position_version += 1;
            }
        }
        if account_changed {
            acct.account_version += 1;
        }
    }

    if let Some(staleness_secs) = next.config.feed_staleness_seconds {
        let staleness_duration = Duration::seconds(i64::from(staleness_secs));
        let source_ids: Vec<String> = match &cmd.source_id {
            Some(id) => vec![id.clone()],
            None => next.feeds.keys().cloned().collect(),
        };
        for src_id in source_ids {
            let feed = next
                .feeds
                .get_mut(&src_id)
                .ok_or_else(|| BlotterError::not_found("feed not found"))?;
            if !feed.paused_by_staleness
                && let Some(last_hb) = &feed.last_heartbeat_at
                && now.instant().signed_duration_since(last_hb.instant()) >= staleness_duration
            {
                feed.paused_by_staleness = true;
                feed.feed_status_revision += 1;
                newly_stale_feed_count = newly_stale_feed_count.saturating_add(1);
            }
        }
    }

    let mutated =
        expired_hold_count > 0 || newly_alerted_committed_count > 0 || newly_stale_feed_count > 0;
    if mutated {
        next.head_seq = candidate_seq;
    }

    Ok((
        next.clone(),
        SweepLifecycleOutput {
            head_seq: next.head_seq,
            mutated,
            expired_hold_count,
            newly_alerted_committed_count,
            newly_stale_feed_count,
        },
    ))
}

/// Pure transition: operator feed control (`paused_by_operator` & `force_clear_gap_pause`).
///
/// Never advances `contiguous_feed_seq`, never mutates `missing_feed_ranges`, and never releases
/// buffered gap packets early.
pub fn apply_set_feed_control(
    state: &BlotterState,
    cmd: &SetFeedControlCommand,
) -> BlotterResult<(BlotterState, SetFeedControlOutput)> {
    require_non_empty(&cmd.source_id, "source_id")?;
    if cmd.force_clear_gap_pause {
        require_non_empty(&cmd.reason, "reason")?;
    }

    let feed = state
        .feeds
        .get(&cmd.source_id)
        .ok_or_else(|| BlotterError::not_found(format!("unknown feed source {}", cmd.source_id)))?;
    if feed.feed_status_revision != cmd.expected_feed_status_revision {
        return Err(BlotterError::concurrency_conflict(
            "stale expected_feed_status_revision",
        ));
    }

    let mut next = state.clone();
    let next_seq = next_head_seq(next.head_seq)?;
    let next_feed = next
        .feeds
        .get_mut(&cmd.source_id)
        .ok_or_else(|| BlotterError::not_found("feed not found"))?;

    next_feed.paused_by_operator = cmd.paused_by_operator;
    if cmd.force_clear_gap_pause {
        next_feed.paused_by_gap = false;
    }
    if !cmd.reason.trim().is_empty() {
        next_feed.status_reason.clone_from(&cmd.reason);
    }
    next_feed.feed_status_revision += 1;
    next.head_seq = next_seq;

    let feed_status = next.feed_status(&cmd.source_id)?;
    Ok((
        next,
        SetFeedControlOutput {
            committed_seq: next_seq,
            feed_status,
        },
    ))
}

fn validate_opening_base(base: &OpeningPositionBase) -> BlotterResult<()> {
    require_non_empty(&base.account_id, "account_id")?;
    require_non_empty(&base.source_id, "source_id")?;
    if base.trading_date >= base.next_settlement_date {
        return Err(BlotterError::validation(
            "trading_date must be strictly before next_settlement_date",
        ));
    }

    let mut seen_balances: BTreeSet<&str> = BTreeSet::new();
    for bal in &base.balances {
        require_non_empty(&bal.instrument_code, "instrument_code")?;
        if !seen_balances.insert(&bal.instrument_code) {
            return Err(BlotterError::validation(format!(
                "duplicate instrument_code {} in opening balances",
                bal.instrument_code
            )));
        }
        if !bal.opening_available_notional.is_non_negative() {
            return Err(BlotterError::validation(format!(
                "opening_available_notional for {} must be non-negative",
                bal.instrument_code
            )));
        }
    }

    let mut seen_instruments: BTreeSet<&str> = BTreeSet::new();
    for code in &base.instrument_codes {
        require_non_empty(code, "instrument_code")?;
        if !seen_instruments.insert(code) {
            return Err(BlotterError::validation(format!(
                "duplicate instrument_code {code} in instrument catalog"
            )));
        }
    }

    let mut seen_ev_keys: BTreeSet<(&str, &str)> = BTreeSet::new();
    for ev in &base.baseline_evidence {
        ev.validate()?;
        if ev.source_id != base.source_id {
            return Err(BlotterError::validation(
                "baseline_evidence source_id must match opening_base source_id",
            ));
        }
        if !ev.satisfies_watermark(base) {
            return Err(BlotterError::validation(
                "baseline_evidence does not satisfy account watermark",
            ));
        }
        if !seen_ev_keys.insert((&ev.source_id, &ev.external_trade_id)) {
            return Err(BlotterError::validation(
                "duplicate baseline_evidence for same (source_id, external_trade_id)",
            ));
        }
    }

    Ok(())
}

fn validate_ttl(ttl_seconds: u32) -> BlotterResult<()> {
    if !(MIN_HOLD_TTL_SECONDS..=MAX_HOLD_TTL_SECONDS).contains(&ttl_seconds) {
        return Err(BlotterError::validation(format!(
            "ttl_seconds must be within {MIN_HOLD_TTL_SECONDS}..={MAX_HOLD_TTL_SECONDS}, got {ttl_seconds}"
        )));
    }
    Ok(())
}

fn add_seconds_market_time(base: &MarketTime, seconds: u32) -> BlotterResult<MarketTime> {
    let instant = base.instant() + Duration::seconds(i64::from(seconds));
    let tz: Tz = base
        .market_timezone()
        .parse()
        .map_err(|_| BlotterError::validation("invalid market_timezone"))?;
    let local_date = instant.with_timezone(&tz).date_naive();
    MarketTime::new(instant, base.market_timezone(), local_date).map_err(BlotterError::from)
}

fn local_time_of(time: &MarketTime) -> BlotterResult<chrono::NaiveTime> {
    let tz: Tz = time
        .market_timezone()
        .parse()
        .map_err(|_| BlotterError::validation("invalid market_timezone"))?;
    Ok(time.instant().with_timezone(&tz).time())
}

fn next_head_seq(current: u64) -> BlotterResult<u64> {
    current
        .checked_add(1)
        .ok_or_else(|| BlotterError::validation("head_seq overflow"))
}

fn count_open_pending_for_source(
    accounts: &BTreeMap<String, AccountState>,
    source_id: &str,
) -> u32 {
    let count: usize = accounts
        .values()
        .filter(|acct| acct.opening_base.source_id == source_id)
        .flat_map(|acct| acct.pending_matches.values())
        .filter(|pm| pm.state == PendingMatchState::Open)
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}
