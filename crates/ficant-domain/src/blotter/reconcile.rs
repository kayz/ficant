use std::collections::{BTreeMap, BTreeSet};

use crate::blotter::error::{BlotterError, BlotterResult};
use crate::blotter::funding::{ResolvedSettlementCash, resolve_settlement_cash};
use crate::blotter::model::{
    AccountState, BaseEvidence, BaselineDelta, BlotterConfig, BlotterIdGenerator, EarmarkState,
    FundingPrecision, InstrumentPositionState, MatchKey, PendingMatchReason, PendingMatchRecord,
    PendingMatchResolutionAction, PendingMatchState, QuoteBasis, ReconcileAllocationRecord,
    SeqRange, SettlementBucket, SourceBatchRecord, SourceFeedState, StoredRawPacket,
    StoredTradeRevision, TradeFeedRecordAction, TradeFillIngestOutcome, TradeFillOutcomeKind,
    TradeFillRecord, TradeRevisionLifecycle, require_non_empty,
};
use crate::blotter::position::{add_reconciled_notional, checked_add, checked_sub};
use crate::blotter::wide_math::AllocationPool;
use crate::primitives::{ContentHash, FixedDecimal, MarketTime, Ulid};

/// Parameters for ingesting a batch (or fragment) of trade fills on a single feed source.
#[derive(Clone, Debug)]
pub struct IngestTradeFeedInput {
    pub source_id: String,
    pub fills: Vec<TradeFillRecord>,
    pub heartbeat_at: Option<MarketTime>,
    pub actor_id: String,
}

/// Output produced by [`ingest_trade_feed_into`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestTradeFeedEffect {
    pub mutated: bool,
    pub item_outcomes: Vec<TradeFillIngestOutcome>,
    /// Accounts whose business state changed and whose `account_version` should be incremented.
    pub version_bumped_accounts: BTreeSet<String>,
    /// Accounts bound to `source_id` whose projected shared feed status changed.
    pub feed_status_affected_accounts: BTreeSet<String>,
    pub changed_allocations: Vec<ReconcileAllocationRecord>,
}

/// Parameters for manually resolving an open pending match.
#[derive(Clone, Debug)]
pub struct ResolvePendingMatchInput {
    pub account_id: String,
    pub pending_match_id: Ulid,
    pub expected_pending_revision: u64,
    pub expected_position_version: u64,
    pub action: PendingMatchResolutionAction,
    pub expected_earmark_revision: Option<u64>,
    pub target_earmark_id: Option<Ulid>,
    pub offset_notional: Option<FixedDecimal>,
    pub reason: String,
    pub baseline_delta: Option<BaselineDelta>,
    pub actor_id: String,
}

/// Output produced by [`resolve_pending_match_in_account`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvePendingMatchEffect {
    pub pending_match: PendingMatchRecord,
    pub updated_earmark_id: Option<Ulid>,
    pub allocation: Option<ReconcileAllocationRecord>,
}

enum MatchDecision {
    AutoMatch {
        fill: TradeFillRecord,
        earmark_id: Ulid,
    },
    Pending {
        fill: TradeFillRecord,
        reason: PendingMatchReason,
        candidate_earmark_ids: Vec<Ulid>,
    },
}

/// Pure synchronous trade-feed ingestion, gap capture, revision-chain evaluation, and
/// set-level 1:1 matching across complete source batches.
#[allow(clippy::too_many_lines)]
pub(crate) fn ingest_trade_feed_into(
    accounts: &mut BTreeMap<String, AccountState>,
    feeds: &mut BTreeMap<String, SourceFeedState>,
    config: &BlotterConfig,
    input: &IngestTradeFeedInput,
    now: &MarketTime,
    next_seq: u64,
    id_gen: &mut dyn BlotterIdGenerator,
) -> BlotterResult<IngestTradeFeedEffect> {
    require_non_empty(&input.source_id, "source_id")?;
    require_non_empty(&input.actor_id, "actor_id")?;

    let feed = feeds.get(&input.source_id).ok_or_else(|| {
        BlotterError::not_found(format!("unknown feed source {}", input.source_id))
    })?;

    validate_incoming_fills(accounts, feed, input)?;

    let feed = feeds.get_mut(&input.source_id).ok_or_else(|| {
        BlotterError::not_found(format!("unknown feed source {}", input.source_id))
    })?;

    let prev_missing_ranges = feed.missing_feed_ranges.clone();
    let mut any_new_packet = false;
    let mut occurrence_records: Vec<(u64, String, u64, bool)> =
        Vec::with_capacity(input.fills.len());
    let mut first_seen_in_call: BTreeSet<u64> = BTreeSet::new();
    let mut new_packets_by_seq: BTreeMap<u64, TradeFillRecord> = BTreeMap::new();

    for fill in &input.fills {
        let already_stored = feed.raw_packets.contains_key(&fill.feed_seq);
        let duplicate_in_call = !first_seen_in_call.insert(fill.feed_seq);
        let is_duplicate = already_stored || duplicate_in_call;
        occurrence_records.push((
            fill.feed_seq,
            fill.external_trade_id.clone(),
            fill.source_revision,
            is_duplicate,
        ));
        if !is_duplicate {
            new_packets_by_seq.insert(fill.feed_seq, fill.clone());
        }
    }

    let mut primary_outcomes: BTreeMap<u64, TradeFillIngestOutcome> = BTreeMap::new();
    let mut version_bumped_accounts: BTreeSet<String> = BTreeSet::new();
    let mut changed_allocations: Vec<ReconcileAllocationRecord> = Vec::new();
    let mut alias_seqs: BTreeSet<u64> = BTreeSet::new();
    let mut historical_backfill_batches: BTreeMap<(u64, u64), Vec<u64>> = BTreeMap::new();

    // Capture newly seen raw packets in ascending feed_seq order.
    for (seq, fill) in new_packets_by_seq {
        any_new_packet = true;
        version_bumped_accounts.insert(fill.account_id.clone());

        let payload_hash = fill.payload_hash();
        let business_hash = fill.business_hash();
        let is_pre_cursor_backfill = seq <= feed.initial_after_feed_seq;

        feed.raw_packets.insert(
            seq,
            StoredRawPacket {
                feed_seq: seq,
                external_trade_id: fill.external_trade_id.clone(),
                source_revision: fill.source_revision,
                payload_hash,
                business_hash: business_hash.clone(),
                batch_first_feed_seq: fill.batch_first_feed_seq,
                batch_last_feed_seq: fill.batch_last_feed_seq,
                record: fill.clone(),
            },
        );

        // Only incremental packets (> initial_after_feed_seq) enter completeness-gated source
        // batches. Pre-cursor historical packets fetched via FetchTradeRevision must NEVER wait
        // on never-fetched pre-cursor siblings in [batch_first, batch_last].
        if !is_pre_cursor_backfill {
            feed.source_batches
                .entry((fill.batch_first_feed_seq, fill.batch_last_feed_seq))
                .or_insert(SourceBatchRecord {
                    first_feed_seq: fill.batch_first_feed_seq,
                    last_feed_seq: fill.batch_last_feed_seq,
                    released: false,
                });
        }

        let rev_key = (fill.external_trade_id.clone(), fill.source_revision);
        if let Some(existing_rev) = feed.trade_revisions.get_mut(&rev_key) {
            // New feed_seq delivering the same verified business revision: record alias.
            // effective_revision is backfilled after batch processing completes so in-batch
            // aliases match same-seq deduplicated items.
            existing_rev.alias_feed_seqs.push(seq);
            alias_seqs.insert(seq);
            primary_outcomes.insert(
                seq,
                TradeFillIngestOutcome {
                    source_id: fill.source_id.clone(),
                    external_trade_id: fill.external_trade_id.clone(),
                    source_revision: fill.source_revision,
                    feed_seq: seq,
                    kind: TradeFillOutcomeKind::PerItemDeduplicated,
                    pending_match_id: None,
                    allocation_ids: Vec::new(),
                    effective_revision: None,
                },
            );
        } else {
            feed.trade_revisions.insert(
                rev_key,
                StoredTradeRevision {
                    business_hash,
                    canonical_feed_seq: seq,
                    alias_feed_seqs: Vec::new(),
                    record: fill.clone(),
                    lifecycle: TradeRevisionLifecycle::BufferedGap,
                },
            );
            primary_outcomes.insert(
                seq,
                TradeFillIngestOutcome {
                    source_id: fill.source_id.clone(),
                    external_trade_id: fill.external_trade_id.clone(),
                    source_revision: fill.source_revision,
                    feed_seq: seq,
                    kind: TradeFillOutcomeKind::BufferedGap,
                    pending_match_id: None,
                    allocation_ids: Vec::new(),
                    effective_revision: None,
                },
            );
            if is_pre_cursor_backfill {
                historical_backfill_batches
                    .entry((fill.batch_first_feed_seq, fill.batch_last_feed_seq))
                    .or_default()
                    .push(seq);
            }
        }

        if seq > feed.max_seen_feed_seq {
            feed.max_seen_feed_seq = seq;
        }
    }

    // Advance contiguous cursor and recompute missing feed ranges.
    while feed
        .raw_packets
        .contains_key(&(feed.contiguous_feed_seq + 1))
    {
        feed.contiguous_feed_seq += 1;
    }
    let new_missing_ranges = compute_missing_ranges(
        feed.contiguous_feed_seq,
        feed.max_seen_feed_seq,
        &feed.raw_packets,
    );
    let introduced_new_gap = has_newly_missing_seq(&prev_missing_ranges, &new_missing_ranges);
    feed.missing_feed_ranges = new_missing_ranges;
    if feed.missing_feed_ranges.is_empty() {
        feed.paused_by_gap = false;
    } else if introduced_new_gap {
        feed.paused_by_gap = true;
    }

    // Freshness update: duplicate batches MUST NOT refresh heartbeat or clear staleness.
    let pure_heartbeat = input.fills.is_empty() && input.heartbeat_at.is_some();
    let mut freshness_updated = false;
    if any_new_packet || pure_heartbeat {
        let candidate_hb = input.heartbeat_at.clone().unwrap_or_else(|| now.clone());
        let should_update_hb = feed
            .last_heartbeat_at
            .as_ref()
            .is_none_or(|existing| candidate_hb.instant() > existing.instant());
        if should_update_hb {
            feed.last_heartbeat_at = Some(candidate_hb);
            freshness_updated = true;
        }
        if feed.paused_by_staleness {
            feed.paused_by_staleness = false;
            freshness_updated = true;
        }
    }

    // Process pre-cursor historical backfills first (in ascending batch-bound order) without
    // requiring un-fetched pre-cursor siblings in [batch_first, batch_last].
    for (_bounds, seqs) in historical_backfill_batches {
        process_canonical_revisions(
            accounts,
            feed,
            config,
            &seqs,
            &input.actor_id,
            next_seq,
            id_gen,
            &mut primary_outcomes,
            &mut version_bumped_accounts,
            &mut changed_allocations,
        )?;
    }

    // Identify complete incremental source batches ready to release in strict boundary order.
    let ready_batches: Vec<(u64, u64)> = feed
        .source_batches
        .iter()
        .filter(|&(&(first, last), batch)| {
            !batch.released
                && last <= feed.contiguous_feed_seq
                && (first..=last).all(|seq| feed.raw_packets.contains_key(&seq))
        })
        .map(|(&bounds, _)| bounds)
        .collect();

    for bounds in ready_batches {
        process_complete_source_batch(
            accounts,
            feed,
            config,
            bounds,
            &input.actor_id,
            next_seq,
            id_gen,
            &mut primary_outcomes,
            &mut version_bumped_accounts,
            &mut changed_allocations,
        )?;
        if let Some(batch) = feed.source_batches.get_mut(&bounds) {
            batch.released = true;
        }
    }

    // Backfill effective_revision for newly seen alias feed_seqs now that batch processing is done.
    for alias_seq in alias_seqs {
        if let Some(outcome) = primary_outcomes.get_mut(&alias_seq) {
            outcome.effective_revision = feed
                .trade_chains
                .get(&outcome.external_trade_id)
                .and_then(|chain| chain.latest_effective_revision);
        }
    }

    let mutated = any_new_packet || freshness_updated;
    let feed_status_affected_accounts: BTreeSet<String> = if mutated {
        feed.feed_status_revision = feed
            .feed_status_revision
            .checked_add(1)
            .ok_or_else(|| BlotterError::validation("feed_status_revision overflow"))?;
        accounts
            .iter()
            .filter(|(_, acct)| acct.opening_base.source_id == input.source_id)
            .map(|(acct_id, _)| acct_id.clone())
            .collect()
    } else {
        BTreeSet::new()
    };

    // Build canonical sorted per-occurrence outcomes.
    let mut item_outcomes = Vec::with_capacity(occurrence_records.len());
    for (feed_seq, trade_id, rev, is_dup) in occurrence_records {
        if is_dup {
            let effective_revision = feed
                .trade_chains
                .get(&trade_id)
                .and_then(|chain| chain.latest_effective_revision);
            item_outcomes.push((
                feed_seq,
                trade_id.clone(),
                rev,
                1_u8,
                TradeFillIngestOutcome {
                    source_id: input.source_id.clone(),
                    external_trade_id: trade_id,
                    source_revision: rev,
                    feed_seq,
                    kind: TradeFillOutcomeKind::PerItemDeduplicated,
                    pending_match_id: None,
                    allocation_ids: Vec::new(),
                    effective_revision,
                },
            ));
        } else if let Some(primary) = primary_outcomes.get(&feed_seq) {
            item_outcomes.push((feed_seq, trade_id, rev, 0_u8, primary.clone()));
        }
    }

    item_outcomes.sort_by(|a, b| (a.0, &a.1, a.2, a.3).cmp(&(b.0, &b.1, b.2, b.3)));
    let item_outcomes = item_outcomes.into_iter().map(|entry| entry.4).collect();

    Ok(IngestTradeFeedEffect {
        mutated,
        item_outcomes,
        version_bumped_accounts,
        feed_status_affected_accounts,
        changed_allocations,
    })
}

#[allow(clippy::too_many_lines)]
fn validate_incoming_fills(
    accounts: &BTreeMap<String, AccountState>,
    feed: &SourceFeedState,
    input: &IngestTradeFeedInput,
) -> BlotterResult<()> {
    let mut seen_seq_hash: BTreeMap<u64, ContentHash> = BTreeMap::new();
    let mut seen_rev_hash: BTreeMap<(String, u64), ContentHash> = BTreeMap::new();
    let mut seen_trade_account: BTreeMap<String, String> = BTreeMap::new();
    let mut incoming_batches: BTreeSet<(u64, u64)> = BTreeSet::new();

    // Map external_trade_id -> account_id from all accounts' opening baseline evidence on this source.
    let mut baseline_ev_accounts: BTreeMap<&str, &str> = BTreeMap::new();
    for (acct_id, acct) in accounts {
        if acct.opening_base.source_id != input.source_id {
            continue;
        }
        for ev in &acct.opening_base.baseline_evidence {
            if ev.source_id == input.source_id {
                baseline_ev_accounts.insert(ev.external_trade_id.as_str(), acct_id.as_str());
            }
        }
    }

    for fill in &input.fills {
        if fill.source_id != input.source_id {
            return Err(BlotterError::validation(
                "all trade fills in a batch must match request source_id",
            ));
        }
        require_non_empty(&fill.external_trade_id, "external_trade_id")?;
        require_non_empty(&fill.account_id, "account_id")?;
        require_non_empty(&fill.instrument_code, "instrument_code")?;
        if fill.feed_seq == 0 || fill.source_revision == 0 {
            return Err(BlotterError::validation(
                "feed_seq and source_revision must be >= 1",
            ));
        }
        if fill.batch_first_feed_seq == 0
            || fill.batch_first_feed_seq > fill.feed_seq
            || fill.feed_seq > fill.batch_last_feed_seq
        {
            return Err(BlotterError::validation(
                "invalid source batch bounds for feed_seq",
            ));
        }
        match fill.action {
            TradeFeedRecordAction::NewFill | TradeFeedRecordAction::Amend => {
                if !fill.notional.is_positive() {
                    return Err(BlotterError::validation(
                        "NEW_FILL and AMEND notional must be positive",
                    ));
                }
            }
            TradeFeedRecordAction::CancelReversal => {
                if !fill.notional.is_non_negative() {
                    return Err(BlotterError::validation(
                        "CANCEL_REVERSAL notional must be non-negative",
                    ));
                }
            }
        }
        if let Some(terms) = &fill.quote_terms {
            terms.validate()?;
        }
        if let Some(exact) = fill.exact_dirty_price_input {
            exact.validate()?;
        }

        let account = accounts.get(&fill.account_id).ok_or_else(|| {
            BlotterError::not_found(format!("unknown account {}", fill.account_id))
        })?;
        if account.opening_base.source_id != input.source_id {
            return Err(BlotterError::validation(format!(
                "account {} belongs to source {}, not {}",
                fill.account_id, account.opening_base.source_id, input.source_id
            )));
        }
        if fill.action == TradeFeedRecordAction::NewFill
            && account
                .opening_base
                .bucket_for_settlement_date(fill.settlement_date)
                .is_none()
        {
            return Err(BlotterError::validation(format!(
                "settlement_date {} is outside account {} T+0/T+1 buckets",
                fill.settlement_date, fill.account_id
            )));
        }

        // Enforce account_id immutability across all revisions of (source_id, external_trade_id):
        // check against in-batch revisions, stored trade_revisions, original_new_fill, and
        // opening BaseEvidence.
        if let Some(prev_acct) = seen_trade_account.get(&fill.external_trade_id)
            && prev_acct != &fill.account_id
        {
            return Err(BlotterError::validation(format!(
                "account_id mismatch within batch for ({}, {}): expected {prev_acct}, got {}",
                fill.source_id, fill.external_trade_id, fill.account_id
            )));
        }
        seen_trade_account.insert(fill.external_trade_id.clone(), fill.account_id.clone());

        for (_, stored) in feed
            .trade_revisions
            .range((fill.external_trade_id.clone(), 0)..=(fill.external_trade_id.clone(), u64::MAX))
        {
            if stored.record.account_id != fill.account_id {
                return Err(BlotterError::validation(format!(
                    "immutable account_id violation for ({}, rev {}): trade belongs to {}, got {}",
                    fill.external_trade_id,
                    fill.source_revision,
                    stored.record.account_id,
                    fill.account_id
                )));
            }
        }
        if let Some(chain) = feed.trade_chains.get(&fill.external_trade_id)
            && let Some(orig) = &chain.original_new_fill
            && orig.account_id != fill.account_id
        {
            return Err(BlotterError::validation(format!(
                "immutable account_id violation for ({}, rev {}): original_new_fill belongs to {}, got {}",
                fill.external_trade_id, fill.source_revision, orig.account_id, fill.account_id
            )));
        }
        if let Some(&ev_acct) = baseline_ev_accounts.get(fill.external_trade_id.as_str())
            && ev_acct != fill.account_id
        {
            return Err(BlotterError::validation(format!(
                "immutable account_id violation for ({}, rev {}): baseline_evidence belongs to {ev_acct}, got {}",
                fill.external_trade_id, fill.source_revision, fill.account_id
            )));
        }

        let p_hash = fill.payload_hash();
        let b_hash = fill.business_hash();

        if let Some(existing) = feed.raw_packets.get(&fill.feed_seq)
            && existing.payload_hash != p_hash
        {
            return Err(BlotterError::hash_mismatch(format!(
                "conflicting payload for feed_seq {}",
                fill.feed_seq
            )));
        }
        if let Some(prev) = seen_seq_hash.get(&fill.feed_seq)
            && prev != &p_hash
        {
            return Err(BlotterError::hash_mismatch(format!(
                "conflicting duplicate feed_seq {} in batch",
                fill.feed_seq
            )));
        }
        seen_seq_hash.insert(fill.feed_seq, p_hash);

        let rev_key = (fill.external_trade_id.clone(), fill.source_revision);
        if let Some(existing) = feed.trade_revisions.get(&rev_key)
            && existing.business_hash != b_hash
        {
            return Err(BlotterError::hash_mismatch(format!(
                "conflicting business payload for ({}, rev {})",
                fill.external_trade_id, fill.source_revision
            )));
        }
        if let Some(prev) = seen_rev_hash.get(&rev_key)
            && prev != &b_hash
        {
            return Err(BlotterError::hash_mismatch(format!(
                "conflicting duplicate revision ({}, rev {}) in batch",
                fill.external_trade_id, fill.source_revision
            )));
        }
        seen_rev_hash.insert(rev_key, b_hash);
        incoming_batches.insert((fill.batch_first_feed_seq, fill.batch_last_feed_seq));
    }

    // Verify source batch intervals are mutually disjoint (or identical), including historical
    // packets stored in raw_packets.
    let mut all_batches: Vec<(u64, u64)> = feed
        .source_batches
        .keys()
        .copied()
        .chain(
            feed.raw_packets
                .values()
                .map(|pkt| (pkt.batch_first_feed_seq, pkt.batch_last_feed_seq)),
        )
        .chain(incoming_batches)
        .collect();
    all_batches.sort_unstable();
    all_batches.dedup();
    for pair in all_batches.windows(2) {
        if pair[0].1 >= pair[1].0 {
            return Err(BlotterError::validation(format!(
                "overlapping source batch ranges [{}, {}] and [{}, {}]",
                pair[0].0, pair[0].1, pair[1].0, pair[1].1
            )));
        }
    }

    Ok(())
}

enum BaselineOrigin {
    Proven(BaseEvidence),
    PreWatermarkUnpriced(TradeFillRecord),
}

impl BaselineOrigin {
    fn original_revision(&self) -> u64 {
        match self {
            Self::Proven(ev) => ev.original_revision,
            Self::PreWatermarkUnpriced(orig) => orig.source_revision,
        }
    }

    fn instrument_code(&self) -> &str {
        match self {
            Self::Proven(ev) => &ev.instrument_code,
            Self::PreWatermarkUnpriced(orig) => &orig.instrument_code,
        }
    }

    fn side(&self) -> crate::blotter::model::TradeSide {
        match self {
            Self::Proven(ev) => ev.side,
            Self::PreWatermarkUnpriced(orig) => orig.side,
        }
    }

    fn old_notional(&self) -> FixedDecimal {
        match self {
            Self::Proven(ev) => ev.old_notional,
            Self::PreWatermarkUnpriced(orig) => orig.notional,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_complete_source_batch(
    accounts: &mut BTreeMap<String, AccountState>,
    feed: &mut SourceFeedState,
    config: &BlotterConfig,
    bounds: (u64, u64),
    actor_id: &str,
    next_seq: u64,
    id_gen: &mut dyn BlotterIdGenerator,
    primary_outcomes: &mut BTreeMap<u64, TradeFillIngestOutcome>,
    version_bumped_accounts: &mut BTreeSet<String>,
    changed_allocations: &mut Vec<ReconcileAllocationRecord>,
) -> BlotterResult<()> {
    let (first, last) = bounds;
    let seqs: Vec<u64> = (first..=last).collect();
    process_canonical_revisions(
        accounts,
        feed,
        config,
        &seqs,
        actor_id,
        next_seq,
        id_gen,
        primary_outcomes,
        version_bumped_accounts,
        changed_allocations,
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn process_canonical_revisions(
    accounts: &mut BTreeMap<String, AccountState>,
    feed: &mut SourceFeedState,
    config: &BlotterConfig,
    seqs: &[u64],
    actor_id: &str,
    next_seq: u64,
    id_gen: &mut dyn BlotterIdGenerator,
    primary_outcomes: &mut BTreeMap<u64, TradeFillIngestOutcome>,
    version_bumped_accounts: &mut BTreeSet<String>,
    changed_allocations: &mut Vec<ReconcileAllocationRecord>,
) -> BlotterResult<()> {
    // Collect canonical (non-alias) trade revisions in this observation unit, grouped by trade_id.
    let mut trades_in_batch: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    for &seq in seqs {
        let Some(pkt) = feed.raw_packets.get(&seq) else {
            continue;
        };
        let rev_key = (pkt.external_trade_id.clone(), pkt.source_revision);
        let Some(stored_rev) = feed.trade_revisions.get(&rev_key) else {
            continue;
        };
        if stored_rev.canonical_feed_seq != seq {
            continue;
        }
        trades_in_batch
            .entry(pkt.external_trade_id.clone())
            .or_default()
            .insert(pkt.source_revision);
    }

    // Merge previously held revisions for any trade touched by this batch.
    for (trade_id, revs) in &mut trades_in_batch {
        if let Some(chain) = feed.trade_chains.get(trade_id) {
            for &held_rev in &chain.held_revisions {
                revs.insert(held_rev);
            }
        }
    }

    let mut batch_candidates: BTreeMap<MatchKey, Vec<TradeFillRecord>> = BTreeMap::new();

    // Phase 1: Evaluate revision continuity, baseline status, reversals, and build candidate set.
    for (trade_id, revs_set) in trades_in_batch {
        let sorted_revs: Vec<u64> = revs_set.into_iter().collect();
        let Some(&first_rev) = sorted_revs.first() else {
            continue;
        };
        let first_record = feed
            .trade_revisions
            .get(&(trade_id.clone(), first_rev))
            .ok_or_else(|| BlotterError::validation("missing stored trade revision"))?
            .record
            .clone();
        let account_id = first_record.account_id.clone();
        version_bumped_accounts.insert(account_id.clone());

        let base_ev_from_opening = accounts
            .get(&account_id)
            .and_then(|acct| find_opening_baseline_evidence(acct, &feed.source_id, &trade_id));

        let mut contiguous_ready: Vec<u64> = Vec::new();
        let mut still_held: Vec<u64> = Vec::new();
        let mut missing_preds: BTreeSet<u64> = BTreeSet::new();

        for &rev in &sorted_revs {
            let mut rev_missing = Vec::new();
            for pred in 1..rev {
                let present_in_feed = feed.trade_revisions.contains_key(&(trade_id.clone(), pred));
                let covered_by_opening_ev = base_ev_from_opening
                    .as_ref()
                    .is_some_and(|ev| pred <= ev.original_revision);
                if !present_in_feed && !covered_by_opening_ev {
                    rev_missing.push(pred);
                }
            }
            if rev_missing.is_empty() && still_held.is_empty() {
                contiguous_ready.push(rev);
            } else {
                for m in rev_missing {
                    missing_preds.insert(m);
                }
                still_held.push(rev);
            }
        }

        // Validate action legality & account_id immutability across contiguous_ready.
        validate_revision_chain_actions(feed, &trade_id, &contiguous_ready)?;

        let chain = feed.trade_chains.entry(trade_id.clone()).or_default();

        // Update held revisions tracking.
        for &r in &contiguous_ready {
            chain.held_revisions.remove(&r);
        }
        for &r in &still_held {
            chain.held_revisions.insert(r);
        }
        chain.missing_predecessor_revisions = missing_preds;

        // Record original NEW_FILL (rev 1) if present in contiguous_ready.
        if contiguous_ready.contains(&1)
            && let Some(rev1) = feed.trade_revisions.get(&(trade_id.clone(), 1))
        {
            chain.original_new_fill = Some(rev1.record.clone());
        }

        // Determine authoritative baseline evidence (if any) without aborting the entire
        // shared-source batch when a pre-watermark NEW_FILL is yield-only (no explicit cash).
        let mut baseline_origin: Option<BaselineOrigin> = None;
        if let Some(ev) = chain.baseline_original_evidence.clone() {
            baseline_origin = Some(BaselineOrigin::Proven(ev));
        } else if let Some(ev) = base_ev_from_opening {
            chain.baseline_original_evidence = Some(ev.clone());
            baseline_origin = Some(BaselineOrigin::Proven(ev));
        } else if let Some(orig) = chain.original_new_fill.clone()
            && let Some(acct) = accounts.get(&account_id)
            && is_record_pre_watermark(&orig, &acct.opening_base)
        {
            if let Some(ev) = try_build_evidence_from_original_new_fill(&orig, &acct.opening_base)?
            {
                chain.baseline_original_evidence = Some(ev.clone());
                baseline_origin = Some(BaselineOrigin::Proven(ev));
            } else {
                baseline_origin = Some(BaselineOrigin::PreWatermarkUnpriced(orig));
            }
        }

        let prev_effective_rev = chain.latest_effective_revision;

        // Close any stale RevisionHold pending match for this trade when revisions unblock.
        if !contiguous_ready.is_empty()
            && let Some(acct) = accounts.get_mut(&account_id)
        {
            close_revision_hold_pending_matches(acct, &feed.source_id, &trade_id, next_seq);
        }

        if let Some(&last_ready_rev) = contiguous_ready.last() {
            if let Some(origin) = &baseline_origin {
                apply_baseline_trade_revisions(
                    accounts,
                    feed,
                    &account_id,
                    &trade_id,
                    &contiguous_ready,
                    last_ready_rev,
                    origin,
                    next_seq,
                    id_gen,
                    primary_outcomes,
                    changed_allocations,
                )?;
            } else {
                apply_intraday_trade_revisions(
                    accounts,
                    feed,
                    &account_id,
                    &trade_id,
                    &contiguous_ready,
                    last_ready_rev,
                    prev_effective_rev,
                    next_seq,
                    primary_outcomes,
                    changed_allocations,
                    &mut batch_candidates,
                )?;
            }
        }

        // Maintain at most ONE active REVISION_HOLD pending match per (source_id, external_trade_id),
        // anchored at the latest held revision and never double-counting notional or cash.
        if let Some(&latest_held_rev) = still_held.last() {
            let latest_rec = feed
                .trade_revisions
                .get(&(trade_id.clone(), latest_held_rev))
                .ok_or_else(|| BlotterError::validation("missing held revision record"))?
                .record
                .clone();
            let acct = accounts
                .get_mut(&latest_rec.account_id)
                .ok_or_else(|| BlotterError::not_found("account missing for held revision"))?;
            ensure_position_exists(acct, &latest_rec.instrument_code);
            let pm_id = ensure_revision_hold_pending_match(acct, &latest_rec, next_seq, id_gen);

            let current_effective = feed
                .trade_chains
                .get(&trade_id)
                .and_then(|c| c.latest_effective_revision);

            for rev in still_held {
                let rec = feed
                    .trade_revisions
                    .get(&(trade_id.clone(), rev))
                    .ok_or_else(|| BlotterError::validation("missing held revision record"))?
                    .record
                    .clone();
                let canonical_seq = feed
                    .trade_revisions
                    .get(&(trade_id.clone(), rev))
                    .map_or(rec.feed_seq, |entry| entry.canonical_feed_seq);
                if let Some(stored) = feed.trade_revisions.get_mut(&(trade_id.clone(), rev)) {
                    stored.lifecycle = TradeRevisionLifecycle::RevisionHold;
                }
                primary_outcomes.insert(
                    canonical_seq,
                    TradeFillIngestOutcome {
                        source_id: rec.source_id.clone(),
                        external_trade_id: rec.external_trade_id.clone(),
                        source_revision: rec.source_revision,
                        feed_seq: canonical_seq,
                        kind: TradeFillOutcomeKind::BufferedRevision,
                        pending_match_id: Some(pm_id.clone()),
                        allocation_ids: Vec::new(),
                        effective_revision: current_effective,
                    },
                );
            }
        }
    }

    // Phase 2: Frozen read-only candidate snapshot & simultaneous set-level matching by K.
    let mut decisions: Vec<MatchDecision> = Vec::new();
    for (match_key, mut candidates) in batch_candidates {
        candidates.sort_by(|a, b| a.external_trade_id.cmp(&b.external_trade_id));

        let acct = accounts
            .get(&match_key.account_id)
            .ok_or_else(|| BlotterError::not_found("account missing during matching"))?;

        let active_earmarks: Vec<&crate::blotter::model::EarmarkRecord> = acct
            .earmarks
            .values()
            .filter(|e| {
                e.state == EarmarkState::Committed
                    && e.unreconciled_notional.is_positive()
                    && e.account_id == match_key.account_id
                    && e.instrument_code == match_key.instrument_code
                    && e.side == match_key.side
                    && e.settlement_date == match_key.settlement_date
            })
            .collect();
        let active_earmark_ids: Vec<Ulid> = active_earmarks
            .iter()
            .map(|e| e.earmark_id.clone())
            .collect();

        let existing_open_on_key: Vec<&PendingMatchRecord> = acct
            .pending_matches
            .values()
            .filter(|pm| {
                pm.state == PendingMatchState::Open
                    && pm.account_id == match_key.account_id
                    && pm.instrument_code == match_key.instrument_code
                    && pm.side == match_key.side
                    && pm.settlement_date == match_key.settlement_date
            })
            .collect();

        // Distinguish genuine live holds (trade itself still has un-unblocked held revisions, or
        // source feed currently has missing seq ranges) from complete, applied trades that were
        // previously deferred to OPEN.
        let has_active_revision_freeze = existing_open_on_key.iter().any(|pm| {
            pm.reason == PendingMatchReason::RevisionHold
                && feed
                    .trade_chains
                    .get(&pm.external_trade_id)
                    .is_some_and(|chain| !chain.held_revisions.is_empty())
        });
        let has_active_gap_freeze = !feed.missing_feed_ranges.is_empty();

        if has_active_revision_freeze {
            // Complete, applied fills blocked collaterally by another trade's active RevisionHold
            // on key K MUST NOT inherit the permanent RevisionHold tag; tag ExistingPendingOnKey
            // so they remain resolvable once the real RevisionHold clears.
            for fill in candidates {
                decisions.push(MatchDecision::Pending {
                    fill,
                    reason: PendingMatchReason::ExistingPendingOnKey,
                    candidate_earmark_ids: active_earmark_ids.clone(),
                });
            }
        } else if has_active_gap_freeze {
            for fill in candidates {
                decisions.push(MatchDecision::Pending {
                    fill,
                    reason: PendingMatchReason::GapHold,
                    candidate_earmark_ids: active_earmark_ids.clone(),
                });
            }
        } else if !existing_open_on_key.is_empty() {
            for fill in candidates {
                decisions.push(MatchDecision::Pending {
                    fill,
                    reason: PendingMatchReason::ExistingPendingOnKey,
                    candidate_earmark_ids: active_earmark_ids.clone(),
                });
            }
        } else if active_earmarks.is_empty() {
            for fill in candidates {
                decisions.push(MatchDecision::Pending {
                    fill,
                    reason: PendingMatchReason::OrphanNoCommitment,
                    candidate_earmark_ids: Vec::new(),
                });
            }
        } else if candidates.len() > 1 || active_earmarks.len() > 1 {
            for fill in candidates {
                decisions.push(MatchDecision::Pending {
                    fill,
                    reason: PendingMatchReason::AmbiguousCandidates,
                    candidate_earmark_ids: active_earmark_ids.clone(),
                });
            }
        } else {
            let fill = candidates
                .into_iter()
                .next()
                .expect("candidates has exactly one element");
            let earmark = active_earmarks[0];
            if fill.notional == earmark.unreconciled_notional {
                match compare_quote_terms(
                    fill.quote_terms.as_ref(),
                    earmark.quote_terms.as_ref(),
                    config,
                ) {
                    QuoteComparison::WithinTolerance => {
                        decisions.push(MatchDecision::AutoMatch {
                            fill,
                            earmark_id: earmark.earmark_id.clone(),
                        });
                    }
                    QuoteComparison::OutOfTolerance => {
                        decisions.push(MatchDecision::Pending {
                            fill,
                            reason: PendingMatchReason::PriceOutOfTolerance,
                            candidate_earmark_ids: vec![earmark.earmark_id.clone()],
                        });
                    }
                    QuoteComparison::Incomparable => {
                        decisions.push(MatchDecision::Pending {
                            fill,
                            reason: PendingMatchReason::IncomparableBasis,
                            candidate_earmark_ids: vec![earmark.earmark_id.clone()],
                        });
                    }
                }
            } else {
                decisions.push(MatchDecision::Pending {
                    fill,
                    reason: PendingMatchReason::NotionalMismatchOrSplit,
                    candidate_earmark_ids: vec![earmark.earmark_id.clone()],
                });
            }
        }
    }

    // Phase 3: Commit all decided outcomes in deterministic order.
    for decision in decisions {
        match decision {
            MatchDecision::AutoMatch { fill, earmark_id } => {
                let acct = accounts
                    .get_mut(&fill.account_id)
                    .ok_or_else(|| BlotterError::not_found("account missing on auto-match"))?;
                let alloc = apply_auto_match(acct, &fill, &earmark_id, actor_id, next_seq, id_gen)?;
                let alloc_id = alloc.allocation_id.clone();
                changed_allocations.push(alloc);

                let canonical_seq = feed
                    .trade_revisions
                    .get(&(fill.external_trade_id.clone(), fill.source_revision))
                    .map_or(fill.feed_seq, |entry| entry.canonical_feed_seq);
                if let Some(stored) = feed
                    .trade_revisions
                    .get_mut(&(fill.external_trade_id.clone(), fill.source_revision))
                {
                    stored.lifecycle = TradeRevisionLifecycle::Applied;
                }
                primary_outcomes.insert(
                    canonical_seq,
                    TradeFillIngestOutcome {
                        source_id: fill.source_id.clone(),
                        external_trade_id: fill.external_trade_id.clone(),
                        source_revision: fill.source_revision,
                        feed_seq: canonical_seq,
                        kind: TradeFillOutcomeKind::AutoMatched,
                        pending_match_id: None,
                        allocation_ids: vec![alloc_id],
                        effective_revision: Some(fill.source_revision),
                    },
                );
            }
            MatchDecision::Pending {
                fill,
                reason,
                candidate_earmark_ids,
            } => {
                let acct = accounts
                    .get_mut(&fill.account_id)
                    .ok_or_else(|| BlotterError::not_found("account missing on pending match"))?;
                let pm_id = create_pending_match(
                    acct,
                    &fill,
                    reason,
                    candidate_earmark_ids,
                    next_seq,
                    id_gen,
                )?;

                let canonical_seq = feed
                    .trade_revisions
                    .get(&(fill.external_trade_id.clone(), fill.source_revision))
                    .map_or(fill.feed_seq, |entry| entry.canonical_feed_seq);
                if let Some(stored) = feed
                    .trade_revisions
                    .get_mut(&(fill.external_trade_id.clone(), fill.source_revision))
                {
                    stored.lifecycle = TradeRevisionLifecycle::Applied;
                }
                primary_outcomes.insert(
                    canonical_seq,
                    TradeFillIngestOutcome {
                        source_id: fill.source_id.clone(),
                        external_trade_id: fill.external_trade_id.clone(),
                        source_revision: fill.source_revision,
                        feed_seq: canonical_seq,
                        kind: TradeFillOutcomeKind::PendingMatch,
                        pending_match_id: Some(pm_id),
                        allocation_ids: Vec::new(),
                        effective_revision: Some(fill.source_revision),
                    },
                );
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn apply_baseline_trade_revisions(
    accounts: &mut BTreeMap<String, AccountState>,
    feed: &mut SourceFeedState,
    account_id: &str,
    trade_id: &str,
    contiguous_ready: &[u64],
    last_ready_rev: u64,
    origin: &BaselineOrigin,
    next_seq: u64,
    id_gen: &mut dyn BlotterIdGenerator,
    primary_outcomes: &mut BTreeMap<u64, TradeFillIngestOutcome>,
    changed_allocations: &mut Vec<ReconcileAllocationRecord>,
) -> BlotterResult<()> {
    for &rev in contiguous_ready {
        let canonical_seq = feed
            .trade_revisions
            .get(&(trade_id.to_owned(), rev))
            .ok_or_else(|| BlotterError::validation("missing revision"))?
            .canonical_feed_seq;

        if rev <= origin.original_revision() {
            if let Some(stored) = feed.trade_revisions.get_mut(&(trade_id.to_owned(), rev)) {
                stored.lifecycle = TradeRevisionLifecycle::PreWatermarkIgnored;
            }
            primary_outcomes.insert(
                canonical_seq,
                TradeFillIngestOutcome {
                    source_id: feed.source_id.clone(),
                    external_trade_id: trade_id.to_owned(),
                    source_revision: rev,
                    feed_seq: canonical_seq,
                    kind: TradeFillOutcomeKind::PreWatermarkIgnored,
                    pending_match_id: None,
                    allocation_ids: Vec::new(),
                    effective_revision: None,
                },
            );
            continue;
        }

        if rev < last_ready_rev {
            if let Some(stored) = feed.trade_revisions.get_mut(&(trade_id.to_owned(), rev)) {
                stored.lifecycle = TradeRevisionLifecycle::SupersededInBatch;
            }
            primary_outcomes.insert(
                canonical_seq,
                TradeFillIngestOutcome {
                    source_id: feed.source_id.clone(),
                    external_trade_id: trade_id.to_owned(),
                    source_revision: rev,
                    feed_seq: canonical_seq,
                    kind: TradeFillOutcomeKind::SupersededInBatch,
                    pending_match_id: None,
                    allocation_ids: Vec::new(),
                    effective_revision: Some(last_ready_rev),
                },
            );
            continue;
        }

        // Final baseline revision (AMEND or CANCEL_REVERSAL):
        // 1. Reverse any currently active approved BaselineDelta allocation for this trade.
        // 2. Close any old OPEN pending match for this trade.
        // 3. Create new OPEN BaselineRevisionManual pending match referencing the SAME original
        //    BaseEvidence (or evidence=None when the pre-watermark NEW_FILL lacked explicit cash).
        let rec = feed
            .trade_revisions
            .get(&(trade_id.to_owned(), rev))
            .ok_or_else(|| BlotterError::validation("missing final baseline revision"))?
            .record
            .clone();

        let acct = accounts
            .get_mut(account_id)
            .ok_or_else(|| BlotterError::not_found("account missing for baseline revision"))?;

        reverse_active_allocations_for_trade(
            acct,
            &feed.source_id,
            trade_id,
            rev,
            next_seq,
            changed_allocations,
        )?;
        close_open_pending_matches_for_trade(acct, &feed.source_id, trade_id, next_seq);

        ensure_position_exists(acct, origin.instrument_code());
        ensure_position_exists(acct, &rec.instrument_code);

        let rec_bucket = acct
            .opening_base
            .bucket_for_settlement_date(rec.settlement_date);
        let origin_bucket = match origin {
            BaselineOrigin::Proven(ev) => Some(ev.settlement_bucket),
            BaselineOrigin::PreWatermarkUnpriced(orig) => acct
                .opening_base
                .bucket_for_settlement_date(orig.settlement_date),
        };
        let cross_key_or_bucket = rec.instrument_code != origin.instrument_code()
            || rec.side != origin.side()
            || rec_bucket != origin_bucket;

        let target_bucket = rec_bucket
            .or(origin_bucket)
            .unwrap_or(SettlementBucket::TPlus0);
        let old_notional = origin.old_notional();

        let (original_notional, unresolved_notional, unresolved_signed_cash, precision, evidence) =
            match origin {
                BaselineOrigin::Proven(ev) => {
                    let (rec_notional, unres_n, unres_cash, prec) =
                        if rec.action == TradeFeedRecordAction::CancelReversal {
                            let neg_old_cash = checked_sub(FixedDecimal::ZERO, ev.old_signed_cash)?;
                            (
                                FixedDecimal::ZERO,
                                FixedDecimal::ZERO,
                                Some(neg_old_cash),
                                FundingPrecision::ExactSettlementAmount,
                            )
                        } else {
                            let resolved = resolve_settlement_cash(
                                rec.side,
                                rec.notional,
                                rec.quote_terms.as_ref(),
                                rec.exact_dirty_price_input,
                            )?;
                            let extra_notional = if rec.notional > ev.old_notional {
                                checked_sub(rec.notional, ev.old_notional)?
                            } else {
                                FixedDecimal::ZERO
                            };
                            let delta_cash = match resolved.signed_cash {
                                Some(new_cash) => Some(checked_sub(new_cash, ev.old_signed_cash)?),
                                None => None,
                            };
                            (rec.notional, extra_notional, delta_cash, resolved.precision)
                        };
                    (rec_notional, unres_n, unres_cash, prec, Some(ev.clone()))
                }
                BaselineOrigin::PreWatermarkUnpriced(_) => {
                    // Original pre-watermark NEW_FILL was yield-only (lacked explicit cash);
                    // block APPLY_BASELINE_DELTA via evidence=None and surface unknown cash.
                    let (rec_notional, unres_n) =
                        if rec.action == TradeFeedRecordAction::CancelReversal {
                            (FixedDecimal::ZERO, FixedDecimal::ZERO)
                        } else {
                            let extra_notional = if rec.notional > old_notional {
                                checked_sub(rec.notional, old_notional)?
                            } else {
                                FixedDecimal::ZERO
                            };
                            (rec.notional, extra_notional)
                        };
                    (
                        rec_notional,
                        unres_n,
                        None,
                        FundingPrecision::UnpricedYieldOnly,
                        None,
                    )
                }
            };

        let pm_id = id_gen.next_ulid();
        let pm = PendingMatchRecord {
            pending_match_id: pm_id.clone(),
            pending_revision: 1,
            account_id: acct.account_id.clone(),
            instrument_code: rec.instrument_code.clone(),
            side: rec.side,
            settlement_bucket: target_bucket,
            settlement_date: rec.settlement_date,
            source_id: rec.source_id.clone(),
            external_trade_id: rec.external_trade_id.clone(),
            source_revision: rec.source_revision,
            original_notional,
            unresolved_notional,
            prior_baseline_notional: Some(old_notional),
            quote_terms: rec.quote_terms.clone(),
            executed_at: rec.executed_at.clone(),
            state: PendingMatchState::Open,
            reason: PendingMatchReason::BaselineRevisionManual,
            candidate_earmark_ids: Vec::new(),
            created_seq: next_seq,
            updated_seq: next_seq,
            unresolved_signed_cash,
            funding_precision: precision,
            evidence,
            applied_delta: None,
            resolution_actor_id: None,
            resolution_reason: None,
            cross_key_or_bucket_anomaly: cross_key_or_bucket,
            allocation_pool: AllocationPool::empty(),
        };
        acct.pending_matches.insert(pm_id.clone(), pm);
        if let Some(pos) = acct.positions.get_mut(&rec.instrument_code) {
            pos.position_version += 1;
        }

        if let Some(chain) = feed.trade_chains.get_mut(trade_id) {
            chain.latest_effective_revision = Some(rev);
        }
        if let Some(stored) = feed.trade_revisions.get_mut(&(trade_id.to_owned(), rev)) {
            stored.lifecycle = TradeRevisionLifecycle::BaselineReview;
        }

        primary_outcomes.insert(
            canonical_seq,
            TradeFillIngestOutcome {
                source_id: feed.source_id.clone(),
                external_trade_id: trade_id.to_owned(),
                source_revision: rev,
                feed_seq: canonical_seq,
                kind: TradeFillOutcomeKind::BaselineReview,
                pending_match_id: Some(pm_id),
                allocation_ids: Vec::new(),
                effective_revision: Some(rev),
            },
        );
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_intraday_trade_revisions(
    accounts: &mut BTreeMap<String, AccountState>,
    feed: &mut SourceFeedState,
    account_id: &str,
    trade_id: &str,
    contiguous_ready: &[u64],
    last_ready_rev: u64,
    prev_effective_rev: Option<u64>,
    next_seq: u64,
    primary_outcomes: &mut BTreeMap<u64, TradeFillIngestOutcome>,
    changed_allocations: &mut Vec<ReconcileAllocationRecord>,
    batch_candidates: &mut BTreeMap<MatchKey, Vec<TradeFillRecord>>,
) -> BlotterResult<()> {
    // If trade was previously effective, reverse its old allocations & close its old OPEN.
    if prev_effective_rev.is_some()
        && let Some(acct) = accounts.get_mut(account_id)
    {
        reverse_active_allocations_for_trade(
            acct,
            &feed.source_id,
            trade_id,
            last_ready_rev,
            next_seq,
            changed_allocations,
        )?;
        close_open_pending_matches_for_trade(acct, &feed.source_id, trade_id, next_seq);
    }

    for &rev in contiguous_ready {
        let canonical_seq = feed
            .trade_revisions
            .get(&(trade_id.to_owned(), rev))
            .ok_or_else(|| BlotterError::validation("missing revision"))?
            .canonical_feed_seq;

        if rev < last_ready_rev {
            if let Some(stored) = feed.trade_revisions.get_mut(&(trade_id.to_owned(), rev)) {
                stored.lifecycle = TradeRevisionLifecycle::SupersededInBatch;
            }
            primary_outcomes.insert(
                canonical_seq,
                TradeFillIngestOutcome {
                    source_id: feed.source_id.clone(),
                    external_trade_id: trade_id.to_owned(),
                    source_revision: rev,
                    feed_seq: canonical_seq,
                    kind: TradeFillOutcomeKind::SupersededInBatch,
                    pending_match_id: None,
                    allocation_ids: Vec::new(),
                    effective_revision: Some(last_ready_rev),
                },
            );
            continue;
        }

        let rec = feed
            .trade_revisions
            .get(&(trade_id.to_owned(), rev))
            .ok_or_else(|| BlotterError::validation("missing final intraday revision"))?
            .record
            .clone();

        if let Some(chain) = feed.trade_chains.get_mut(trade_id) {
            chain.latest_effective_revision = Some(rev);
        }

        if rec.action == TradeFeedRecordAction::CancelReversal {
            if let Some(stored) = feed.trade_revisions.get_mut(&(trade_id.to_owned(), rev)) {
                stored.lifecycle = TradeRevisionLifecycle::Reversed;
            }
            primary_outcomes.insert(
                canonical_seq,
                TradeFillIngestOutcome {
                    source_id: feed.source_id.clone(),
                    external_trade_id: trade_id.to_owned(),
                    source_revision: rev,
                    feed_seq: canonical_seq,
                    kind: TradeFillOutcomeKind::Reversed,
                    pending_match_id: None,
                    allocation_ids: Vec::new(),
                    effective_revision: Some(rev),
                },
            );
        } else {
            if let Some(acct) = accounts.get_mut(&rec.account_id) {
                ensure_position_exists(acct, &rec.instrument_code);
            }
            let key = MatchKey {
                account_id: rec.account_id.clone(),
                instrument_code: rec.instrument_code.clone(),
                side: rec.side,
                settlement_date: rec.settlement_date,
            };
            batch_candidates.entry(key).or_default().push(rec);
        }
    }

    Ok(())
}

/// Reverses all non-reversed allocations of `(source_id, external_trade_id)` directly from
/// stored original slice values (never recomputing), restoring earmarks and opening fresh
/// allocation segments on restored earmarks.
fn reverse_active_allocations_for_trade(
    account: &mut AccountState,
    source_id: &str,
    external_trade_id: &str,
    reversed_by_revision: u64,
    next_seq: u64,
    changed_allocations: &mut Vec<ReconcileAllocationRecord>,
) -> BlotterResult<()> {
    let alloc_ids: Vec<Ulid> = account
        .allocations
        .values()
        .filter(|a| {
            a.source_id == source_id
                && a.external_trade_id == external_trade_id
                && a.reversed_by_revision.is_none()
        })
        .map(|a| a.allocation_id.clone())
        .collect();

    let mut restored_earmarks: BTreeSet<Ulid> = BTreeSet::new();

    for alloc_id in alloc_ids {
        let alloc_snapshot = {
            let alloc = account
                .allocations
                .get_mut(&alloc_id)
                .ok_or_else(|| BlotterError::not_found("allocation disappeared"))?;
            alloc.reversed_by_revision = Some(reversed_by_revision);
            alloc.clone()
        };

        // Reverse position reconciled notional.
        let pos = account
            .positions
            .get_mut(&alloc_snapshot.instrument_code)
            .ok_or_else(|| BlotterError::not_found("position missing during reversal"))?;
        let neg_notional = checked_sub(FixedDecimal::ZERO, alloc_snapshot.allocated_notional)?;
        add_reconciled_notional(
            pos,
            alloc_snapshot.side,
            alloc_snapshot.settlement_bucket,
            neg_notional,
        )?;
        pos.position_version += 1;

        // Restore earmark directly from stored slice amounts.
        if let Some(earmark_id) = &alloc_snapshot.earmark_id
            && let Some(earmark) = account.earmarks.get_mut(earmark_id)
        {
            earmark.unreconciled_notional = checked_add(
                earmark.unreconciled_notional,
                alloc_snapshot.allocated_notional,
            )?;
            if let Some(released_cash) = alloc_snapshot.released_earmark_cash {
                let current_unrec = earmark
                    .unreconciled_settlement_cash
                    .unwrap_or(FixedDecimal::ZERO);
                earmark.unreconciled_settlement_cash =
                    Some(checked_add(current_unrec, released_cash)?);
            }
            earmark.state = EarmarkState::Committed;
            earmark.reopened_by_reversal = true;
            earmark.earmark_revision += 1;
            earmark.updated_seq = next_seq;
            restored_earmarks.insert(earmark_id.clone());
        }

        changed_allocations.push(alloc_snapshot);
    }

    // For every restored earmark, archive the prior allocation segment and start a fresh segment
    // anchored at the actual remaining `(unreconciled_notional, unreconciled_settlement_cash)`.
    for earmark_id in restored_earmarks {
        if let Some(earmark) = account.earmarks.get_mut(&earmark_id)
            && let Some(rem_cash) = earmark.unreconciled_settlement_cash
            && earmark.unreconciled_notional.is_positive()
        {
            earmark
                .allocation_pool
                .open_new_segment_from_remainder(earmark.unreconciled_notional, rem_cash)?;
        }
    }

    Ok(())
}

fn close_open_pending_matches_for_trade(
    account: &mut AccountState,
    source_id: &str,
    external_trade_id: &str,
    next_seq: u64,
) {
    let mut touched_instruments: BTreeSet<String> = BTreeSet::new();
    for pm in account.pending_matches.values_mut() {
        if pm.state == PendingMatchState::Open
            && pm.source_id == source_id
            && pm.external_trade_id == external_trade_id
        {
            pm.state = PendingMatchState::ReversedByFeed;
            pm.unresolved_notional = FixedDecimal::ZERO;
            if pm.unresolved_signed_cash.is_some() {
                pm.unresolved_signed_cash = Some(FixedDecimal::ZERO);
            }
            pm.pending_revision += 1;
            pm.updated_seq = next_seq;
            touched_instruments.insert(pm.instrument_code.clone());
        }
    }
    for inst in touched_instruments {
        if let Some(pos) = account.positions.get_mut(&inst) {
            pos.position_version += 1;
        }
    }
}

fn close_revision_hold_pending_matches(
    account: &mut AccountState,
    source_id: &str,
    external_trade_id: &str,
    next_seq: u64,
) {
    let mut touched_instruments: BTreeSet<String> = BTreeSet::new();
    for pm in account.pending_matches.values_mut() {
        if pm.state == PendingMatchState::Open
            && pm.reason == PendingMatchReason::RevisionHold
            && pm.source_id == source_id
            && pm.external_trade_id == external_trade_id
        {
            pm.state = PendingMatchState::ReversedByFeed;
            pm.unresolved_notional = FixedDecimal::ZERO;
            if pm.unresolved_signed_cash.is_some() {
                pm.unresolved_signed_cash = Some(FixedDecimal::ZERO);
            }
            pm.pending_revision += 1;
            pm.updated_seq = next_seq;
            touched_instruments.insert(pm.instrument_code.clone());
        }
    }
    for inst in touched_instruments {
        if let Some(pos) = account.positions.get_mut(&inst) {
            pos.position_version += 1;
        }
    }
}

fn ensure_revision_hold_pending_match(
    account: &mut AccountState,
    rec: &TradeFillRecord,
    next_seq: u64,
    id_gen: &mut dyn BlotterIdGenerator,
) -> Ulid {
    let bucket = account
        .opening_base
        .bucket_for_settlement_date(rec.settlement_date)
        .unwrap_or(SettlementBucket::TPlus0);

    // Maintain at most ONE active REVISION_HOLD pending match per (source_id, external_trade_id),
    // updating it in-place to the latest held revision without stacking notional or cash.
    let existing_id = account
        .pending_matches
        .values()
        .find(|pm| {
            pm.state == PendingMatchState::Open
                && pm.reason == PendingMatchReason::RevisionHold
                && pm.source_id == rec.source_id
                && pm.external_trade_id == rec.external_trade_id
        })
        .map(|pm| pm.pending_match_id.clone());

    if let Some(pm_id) = existing_id {
        let mut touched_instruments: BTreeSet<String> = BTreeSet::new();
        if let Some(existing) = account.pending_matches.get_mut(&pm_id)
            && existing.source_revision != rec.source_revision
        {
            touched_instruments.insert(existing.instrument_code.clone());
            touched_instruments.insert(rec.instrument_code.clone());
            existing.instrument_code.clone_from(&rec.instrument_code);
            existing.side = rec.side;
            existing.settlement_bucket = bucket;
            existing.settlement_date = rec.settlement_date;
            existing.source_revision = rec.source_revision;
            existing.original_notional = rec.notional;
            existing.unresolved_notional = FixedDecimal::ZERO;
            existing.quote_terms.clone_from(&rec.quote_terms);
            existing.executed_at.clone_from(&rec.executed_at);
            existing.unresolved_signed_cash = None;
            existing.funding_precision = FundingPrecision::UnpricedYieldOnly;
            existing.pending_revision += 1;
            existing.updated_seq = next_seq;
        }
        for inst in touched_instruments {
            if let Some(pos) = account.positions.get_mut(&inst) {
                pos.position_version += 1;
            }
        }
        return pm_id;
    }

    let pm_id = id_gen.next_ulid();
    let pm = PendingMatchRecord {
        pending_match_id: pm_id.clone(),
        pending_revision: 1,
        account_id: account.account_id.clone(),
        instrument_code: rec.instrument_code.clone(),
        side: rec.side,
        settlement_bucket: bucket,
        settlement_date: rec.settlement_date,
        source_id: rec.source_id.clone(),
        external_trade_id: rec.external_trade_id.clone(),
        source_revision: rec.source_revision,
        original_notional: rec.notional,
        // Unapplied revision hold does not double-count notional or cash on top of the active
        // prior revision or opening base; unknown cash forces funding_is_complete_exact = false.
        unresolved_notional: FixedDecimal::ZERO,
        prior_baseline_notional: None,
        quote_terms: rec.quote_terms.clone(),
        executed_at: rec.executed_at.clone(),
        state: PendingMatchState::Open,
        reason: PendingMatchReason::RevisionHold,
        candidate_earmark_ids: Vec::new(),
        created_seq: next_seq,
        updated_seq: next_seq,
        unresolved_signed_cash: None,
        funding_precision: FundingPrecision::UnpricedYieldOnly,
        evidence: None,
        applied_delta: None,
        resolution_actor_id: None,
        resolution_reason: None,
        cross_key_or_bucket_anomaly: false,
        allocation_pool: AllocationPool::empty(),
    };
    account.pending_matches.insert(pm_id.clone(), pm);
    if let Some(pos) = account.positions.get_mut(&rec.instrument_code) {
        pos.position_version += 1;
    }
    pm_id
}

fn apply_auto_match(
    account: &mut AccountState,
    fill: &TradeFillRecord,
    earmark_id: &Ulid,
    actor_id: &str,
    next_seq: u64,
    id_gen: &mut dyn BlotterIdGenerator,
) -> BlotterResult<ReconcileAllocationRecord> {
    let bucket = account
        .opening_base
        .bucket_for_settlement_date(fill.settlement_date)
        .ok_or_else(|| BlotterError::validation("fill settlement date outside account buckets"))?;
    let resolved = resolve_settlement_cash(
        fill.side,
        fill.notional,
        fill.quote_terms.as_ref(),
        fill.exact_dirty_price_input,
    )?;

    let mut fill_pool = match resolved.signed_cash {
        Some(cash) => AllocationPool::new(fill.notional, cash)?,
        None => AllocationPool::empty(),
    };
    let (fill_seg_idx, fill_cash_slice) = if fill_pool.active_segment.is_some() {
        let (idx, cash) = fill_pool.allocate_slice(fill.notional)?;
        (Some(idx), Some(cash))
    } else {
        (None, None)
    };

    let earmark = account
        .earmarks
        .get_mut(earmark_id)
        .ok_or_else(|| BlotterError::not_found("target earmark missing on auto-match"))?;
    let (earmark_seg_idx, released_earmark_cash) =
        if earmark.allocation_pool.active_segment.is_some() {
            let (idx, cash) = earmark.allocation_pool.allocate_slice(fill.notional)?;
            (Some(idx), Some(cash))
        } else {
            (None, earmark.unreconciled_settlement_cash)
        };

    earmark.unreconciled_notional = FixedDecimal::ZERO;
    if earmark.unreconciled_settlement_cash.is_some() {
        earmark.unreconciled_settlement_cash = Some(FixedDecimal::ZERO);
    }
    earmark.state = EarmarkState::Reconciled;
    earmark.earmark_revision += 1;
    earmark.updated_seq = next_seq;

    let pos = account
        .positions
        .get_mut(&fill.instrument_code)
        .ok_or_else(|| BlotterError::not_found("position missing on auto-match"))?;
    add_reconciled_notional(pos, fill.side, bucket, fill.notional)?;
    pos.position_version += 1;

    let alloc_id = id_gen.next_ulid();
    let alloc = ReconcileAllocationRecord {
        allocation_id: alloc_id.clone(),
        account_id: account.account_id.clone(),
        instrument_code: fill.instrument_code.clone(),
        side: fill.side,
        settlement_bucket: bucket,
        source_id: fill.source_id.clone(),
        external_trade_id: fill.external_trade_id.clone(),
        source_revision: fill.source_revision,
        earmark_id: Some(earmark_id.clone()),
        pending_match_id: None,
        allocated_notional: fill.notional,
        allocated_fill_cash: fill_cash_slice,
        fill_funding_precision: resolved.precision,
        fill_segment_index: fill_seg_idx,
        earmark_segment_index: earmark_seg_idx,
        reversed_by_revision: None,
        released_earmark_cash,
        baseline_delta: None,
        actor_id: actor_id.to_owned(),
    };
    account.allocations.insert(alloc_id, alloc.clone());
    Ok(alloc)
}

fn create_pending_match(
    account: &mut AccountState,
    fill: &TradeFillRecord,
    reason: PendingMatchReason,
    candidate_earmark_ids: Vec<Ulid>,
    next_seq: u64,
    id_gen: &mut dyn BlotterIdGenerator,
) -> BlotterResult<Ulid> {
    let bucket = account
        .opening_base
        .bucket_for_settlement_date(fill.settlement_date)
        .ok_or_else(|| BlotterError::validation("fill settlement date outside account buckets"))?;
    let resolved = resolve_settlement_cash(
        fill.side,
        fill.notional,
        fill.quote_terms.as_ref(),
        fill.exact_dirty_price_input,
    )?;

    let pool = match resolved.signed_cash {
        Some(cash) if fill.notional.is_positive() => AllocationPool::new(fill.notional, cash)?,
        _ => AllocationPool::empty(),
    };

    let pm_id = id_gen.next_ulid();
    let pm = PendingMatchRecord {
        pending_match_id: pm_id.clone(),
        pending_revision: 1,
        account_id: account.account_id.clone(),
        instrument_code: fill.instrument_code.clone(),
        side: fill.side,
        settlement_bucket: bucket,
        settlement_date: fill.settlement_date,
        source_id: fill.source_id.clone(),
        external_trade_id: fill.external_trade_id.clone(),
        source_revision: fill.source_revision,
        original_notional: fill.notional,
        unresolved_notional: fill.notional,
        prior_baseline_notional: None,
        quote_terms: fill.quote_terms.clone(),
        executed_at: fill.executed_at.clone(),
        state: PendingMatchState::Open,
        reason,
        candidate_earmark_ids,
        created_seq: next_seq,
        updated_seq: next_seq,
        unresolved_signed_cash: resolved.signed_cash,
        funding_precision: resolved.precision,
        evidence: None,
        applied_delta: None,
        resolution_actor_id: None,
        resolution_reason: None,
        cross_key_or_bucket_anomaly: false,
        allocation_pool: pool,
    };
    account.pending_matches.insert(pm_id.clone(), pm);
    if let Some(pos) = account.positions.get_mut(&fill.instrument_code) {
        pos.position_version += 1;
    }
    Ok(pm_id)
}

/// Applies manual pending match disambiguation (`MATCH_FULL`, `SPLIT_OFFSET`, `ACCEPT_ORPHAN`,
/// `RELEASE_REJECT`, `APPLY_BASELINE_DELTA`) with strict field-combination, four-key `K`,
/// bilateral CAS, and baseline-evidence checks.
#[allow(clippy::too_many_lines)]
pub(crate) fn resolve_pending_match_in_account(
    account: &mut AccountState,
    feed: &SourceFeedState,
    input: &ResolvePendingMatchInput,
    next_seq: u64,
    id_gen: &mut dyn BlotterIdGenerator,
) -> BlotterResult<ResolvePendingMatchEffect> {
    require_non_empty(&input.actor_id, "actor_id")?;

    let pm_snapshot = account
        .pending_matches
        .get(&input.pending_match_id)
        .ok_or_else(|| BlotterError::not_found("pending match not found"))?
        .clone();

    if pm_snapshot.state != PendingMatchState::Open {
        return Err(BlotterError::invalid_state_transition(
            "pending match is not OPEN",
        ));
    }
    if pm_snapshot.pending_revision != input.expected_pending_revision {
        return Err(BlotterError::concurrency_conflict(
            "stale expected_pending_revision",
        ));
    }
    let pos_version = account
        .positions
        .get(&pm_snapshot.instrument_code)
        .ok_or_else(|| BlotterError::not_found("position missing for pending match"))?
        .position_version;
    if pos_version != input.expected_position_version {
        return Err(BlotterError::concurrency_conflict(
            "stale expected_position_version",
        ));
    }

    // Strict input combination validation per interfaces.md.
    match input.action {
        PendingMatchResolutionAction::MatchFull => {
            if input.target_earmark_id.is_none()
                || input.expected_earmark_revision.is_none()
                || input.offset_notional.is_some()
                || input.baseline_delta.is_some()
            {
                return Err(BlotterError::validation(
                    "MATCH_FULL requires target_earmark_id and expected_earmark_revision, and forbids offset_notional/baseline_delta",
                ));
            }
            forbid_hold_or_baseline(account, feed, &pm_snapshot, "MATCH_FULL")?;
        }
        PendingMatchResolutionAction::SplitOffset => {
            if input.target_earmark_id.is_none()
                || input.expected_earmark_revision.is_none()
                || input.offset_notional.is_none()
                || input.baseline_delta.is_some()
            {
                return Err(BlotterError::validation(
                    "SPLIT_OFFSET requires target_earmark_id, expected_earmark_revision, and positive offset_notional, and forbids baseline_delta",
                ));
            }
            forbid_hold_or_baseline(account, feed, &pm_snapshot, "SPLIT_OFFSET")?;
        }
        PendingMatchResolutionAction::AcceptOrphan => {
            if input.target_earmark_id.is_some()
                || input.expected_earmark_revision.is_some()
                || input.offset_notional.is_some()
                || input.baseline_delta.is_some()
            {
                return Err(BlotterError::validation(
                    "ACCEPT_ORPHAN forbids target_earmark_id, expected_earmark_revision, offset_notional, and baseline_delta",
                ));
            }
            forbid_hold_or_baseline(account, feed, &pm_snapshot, "ACCEPT_ORPHAN")?;
        }
        PendingMatchResolutionAction::ReleaseReject => {
            if input.target_earmark_id.is_some()
                || input.expected_earmark_revision.is_some()
                || input.offset_notional.is_some()
                || input.baseline_delta.is_some()
            {
                return Err(BlotterError::validation(
                    "RELEASE_REJECT forbids target_earmark_id, expected_earmark_revision, offset_notional, and baseline_delta",
                ));
            }
            require_non_empty(&input.reason, "reason")?;
            forbid_hold_or_baseline(account, feed, &pm_snapshot, "RELEASE_REJECT")?;
        }
        PendingMatchResolutionAction::ApplyBaselineDelta => {
            if input.target_earmark_id.is_some()
                || input.expected_earmark_revision.is_some()
                || input.offset_notional.is_some()
                || input.baseline_delta.is_none()
            {
                return Err(BlotterError::validation(
                    "APPLY_BASELINE_DELTA requires baseline_delta and forbids target/offset fields",
                ));
            }
            if pm_snapshot.reason != PendingMatchReason::BaselineRevisionManual {
                return Err(BlotterError::invalid_state_transition(
                    "APPLY_BASELINE_DELTA is only valid for BASELINE_REVISION_MANUAL pending matches",
                ));
            }
        }
    }

    match input.action {
        PendingMatchResolutionAction::MatchFull => {
            let target_id = input.target_earmark_id.as_ref().expect("validated above");
            let expected_rev = input.expected_earmark_revision.expect("validated above");
            let offset = pm_snapshot.unresolved_notional;
            if !offset.is_positive() {
                return Err(BlotterError::invalid_state_transition(
                    "pending match has zero unresolved notional",
                ));
            }
            execute_earmark_offset(
                account,
                &input.pending_match_id,
                target_id,
                expected_rev,
                offset,
                &input.actor_id,
                &input.reason,
                next_seq,
                id_gen,
            )
        }
        PendingMatchResolutionAction::SplitOffset => {
            let target_id = input.target_earmark_id.as_ref().expect("validated above");
            let expected_rev = input.expected_earmark_revision.expect("validated above");
            let offset = input.offset_notional.expect("validated above");
            if !offset.is_positive() {
                return Err(BlotterError::validation(
                    "SPLIT_OFFSET offset_notional must be positive",
                ));
            }
            execute_earmark_offset(
                account,
                &input.pending_match_id,
                target_id,
                expected_rev,
                offset,
                &input.actor_id,
                &input.reason,
                next_seq,
                id_gen,
            )
        }
        PendingMatchResolutionAction::AcceptOrphan => {
            let pm = account
                .pending_matches
                .get_mut(&input.pending_match_id)
                .ok_or_else(|| BlotterError::not_found("pending match not found"))?;
            let rem_notional = pm.unresolved_notional;
            let (fill_seg_idx, rem_cash) =
                if pm.allocation_pool.active_segment.is_some() && rem_notional.is_positive() {
                    let (idx, cash) = pm.allocation_pool.allocate_slice(rem_notional)?;
                    (Some(idx), Some(cash))
                } else {
                    (None, pm.unresolved_signed_cash)
                };

            pm.unresolved_notional = FixedDecimal::ZERO;
            if pm.unresolved_signed_cash.is_some() {
                pm.unresolved_signed_cash = Some(FixedDecimal::ZERO);
            }
            pm.state = PendingMatchState::ResolvedOrphanAccepted;
            pm.pending_revision += 1;
            pm.updated_seq = next_seq;
            pm.resolution_actor_id = Some(input.actor_id.clone());
            pm.resolution_reason = if input.reason.trim().is_empty() {
                None
            } else {
                Some(input.reason.clone())
            };
            let updated_pm = pm.clone();

            let pos = account
                .positions
                .get_mut(&updated_pm.instrument_code)
                .ok_or_else(|| BlotterError::not_found("position missing on ACCEPT_ORPHAN"))?;
            add_reconciled_notional(
                pos,
                updated_pm.side,
                updated_pm.settlement_bucket,
                rem_notional,
            )?;
            pos.position_version += 1;

            let alloc_id = id_gen.next_ulid();
            let alloc = ReconcileAllocationRecord {
                allocation_id: alloc_id.clone(),
                account_id: account.account_id.clone(),
                instrument_code: updated_pm.instrument_code.clone(),
                side: updated_pm.side,
                settlement_bucket: updated_pm.settlement_bucket,
                source_id: updated_pm.source_id.clone(),
                external_trade_id: updated_pm.external_trade_id.clone(),
                source_revision: updated_pm.source_revision,
                earmark_id: None,
                pending_match_id: Some(updated_pm.pending_match_id.clone()),
                allocated_notional: rem_notional,
                allocated_fill_cash: rem_cash,
                fill_funding_precision: updated_pm.funding_precision,
                fill_segment_index: fill_seg_idx,
                earmark_segment_index: None,
                reversed_by_revision: None,
                released_earmark_cash: None,
                baseline_delta: None,
                actor_id: input.actor_id.clone(),
            };
            account.allocations.insert(alloc_id, alloc.clone());

            Ok(ResolvePendingMatchEffect {
                pending_match: updated_pm,
                updated_earmark_id: None,
                allocation: Some(alloc),
            })
        }
        PendingMatchResolutionAction::ReleaseReject => {
            let pm = account
                .pending_matches
                .get_mut(&input.pending_match_id)
                .ok_or_else(|| BlotterError::not_found("pending match not found"))?;
            pm.unresolved_notional = FixedDecimal::ZERO;
            if pm.unresolved_signed_cash.is_some() {
                pm.unresolved_signed_cash = Some(FixedDecimal::ZERO);
            }
            pm.state = PendingMatchState::Released;
            pm.pending_revision += 1;
            pm.updated_seq = next_seq;
            pm.resolution_actor_id = Some(input.actor_id.clone());
            pm.resolution_reason = Some(input.reason.clone());
            let updated_pm = pm.clone();

            let pos = account
                .positions
                .get_mut(&updated_pm.instrument_code)
                .ok_or_else(|| BlotterError::not_found("position missing on RELEASE_REJECT"))?;
            pos.position_version += 1;

            Ok(ResolvePendingMatchEffect {
                pending_match: updated_pm,
                updated_earmark_id: None,
                allocation: None,
            })
        }
        PendingMatchResolutionAction::ApplyBaselineDelta => {
            let delta = input.baseline_delta.as_ref().expect("validated above");
            delta.evidence.validate()?;
            require_non_empty(&delta.reason, "baseline_delta.reason")?;

            if pm_snapshot.cross_key_or_bucket_anomaly {
                return Err(BlotterError::invalid_state_transition(
                    "cross-key or cross-bucket baseline amendment cannot be confirmed with single-bucket APPLY_BASELINE_DELTA",
                ));
            }

            let expected_ev = pm_snapshot.evidence.as_ref().ok_or_else(|| {
                BlotterError::lineage_incomplete(
                    "pending baseline revision lacks proven BaseEvidence",
                )
            })?;
            if &delta.evidence != expected_ev {
                return Err(BlotterError::validation(
                    "baseline_delta.evidence must match proven original BaseEvidence",
                ));
            }
            if delta.settlement_bucket != expected_ev.settlement_bucket
                || delta.settlement_bucket != pm_snapshot.settlement_bucket
            {
                return Err(BlotterError::invalid_state_transition(
                    "cross-bucket baseline delta rejected",
                ));
            }

            let pm = account
                .pending_matches
                .get_mut(&input.pending_match_id)
                .ok_or_else(|| BlotterError::not_found("pending match not found"))?;
            pm.unresolved_notional = FixedDecimal::ZERO;
            pm.unresolved_signed_cash = Some(FixedDecimal::ZERO);
            pm.applied_delta = Some(delta.clone());
            pm.state = PendingMatchState::ResolvedBaselineDelta;
            pm.pending_revision += 1;
            pm.updated_seq = next_seq;
            pm.resolution_actor_id = Some(input.actor_id.clone());
            pm.resolution_reason = Some(delta.reason.clone());
            let updated_pm = pm.clone();

            let pos = account
                .positions
                .get_mut(&expected_ev.instrument_code)
                .ok_or_else(|| {
                    BlotterError::not_found("position missing on APPLY_BASELINE_DELTA")
                })?;
            add_reconciled_notional(
                pos,
                expected_ev.side,
                delta.settlement_bucket,
                delta.delta_notional,
            )?;
            pos.position_version += 1;

            let alloc_id = id_gen.next_ulid();
            let alloc = ReconcileAllocationRecord {
                allocation_id: alloc_id.clone(),
                account_id: account.account_id.clone(),
                instrument_code: expected_ev.instrument_code.clone(),
                side: expected_ev.side,
                settlement_bucket: delta.settlement_bucket,
                source_id: updated_pm.source_id.clone(),
                external_trade_id: updated_pm.external_trade_id.clone(),
                source_revision: updated_pm.source_revision,
                earmark_id: None,
                pending_match_id: Some(updated_pm.pending_match_id.clone()),
                allocated_notional: delta.delta_notional,
                allocated_fill_cash: Some(delta.delta_cash),
                fill_funding_precision: FundingPrecision::ExactSettlementAmount,
                fill_segment_index: None,
                earmark_segment_index: None,
                reversed_by_revision: None,
                released_earmark_cash: None,
                baseline_delta: Some(delta.clone()),
                actor_id: input.actor_id.clone(),
            };
            account.allocations.insert(alloc_id, alloc.clone());

            Ok(ResolvePendingMatchEffect {
                pending_match: updated_pm,
                updated_earmark_id: None,
                allocation: Some(alloc),
            })
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn execute_earmark_offset(
    account: &mut AccountState,
    pending_match_id: &Ulid,
    target_earmark_id: &Ulid,
    expected_earmark_revision: u64,
    offset_notional: FixedDecimal,
    actor_id: &str,
    reason: &str,
    next_seq: u64,
    id_gen: &mut dyn BlotterIdGenerator,
) -> BlotterResult<ResolvePendingMatchEffect> {
    // Validate bilateral CAS, four-key K equality, and remaining capacities BEFORE mutating.
    let pm_check = account
        .pending_matches
        .get(pending_match_id)
        .ok_or_else(|| BlotterError::not_found("pending match not found"))?;
    let earmark_check = account
        .earmarks
        .get(target_earmark_id)
        .ok_or_else(|| BlotterError::not_found("target earmark not found"))?;

    if earmark_check.state != EarmarkState::Committed {
        return Err(BlotterError::invalid_state_transition(
            "target earmark must be COMMITTED",
        ));
    }
    if earmark_check.earmark_revision != expected_earmark_revision {
        return Err(BlotterError::concurrency_conflict(
            "stale expected_earmark_revision",
        ));
    }
    if earmark_check.account_id != pm_check.account_id
        || earmark_check.instrument_code != pm_check.instrument_code
        || earmark_check.side != pm_check.side
        || earmark_check.settlement_date != pm_check.settlement_date
    {
        return Err(BlotterError::validation(
            "target earmark four-key K=(account, instrument, side, settlement_date) does not match pending match",
        ));
    }
    if offset_notional > pm_check.unresolved_notional
        || offset_notional > earmark_check.unreconciled_notional
    {
        return Err(BlotterError::validation(
            "offset_notional exceeds pending match or target earmark remaining notional",
        ));
    }

    // Allocate independently from pending match's fill pool and target earmark's pool.
    let (fill_seg_idx, fill_cash_slice, updated_pm) = {
        let pm = account
            .pending_matches
            .get_mut(pending_match_id)
            .ok_or_else(|| BlotterError::not_found("pending match disappeared"))?;
        let (f_idx, f_cash) = if pm.allocation_pool.active_segment.is_some() {
            let (idx, cash) = pm.allocation_pool.allocate_slice(offset_notional)?;
            if let Some(rem_cash) = pm.unresolved_signed_cash {
                pm.unresolved_signed_cash = Some(checked_sub(rem_cash, cash)?);
            }
            (Some(idx), Some(cash))
        } else {
            (None, None)
        };
        pm.unresolved_notional = checked_sub(pm.unresolved_notional, offset_notional)?;
        if pm.unresolved_notional == FixedDecimal::ZERO {
            pm.state = PendingMatchState::ResolvedMatched;
        }
        pm.pending_revision += 1;
        pm.updated_seq = next_seq;
        pm.resolution_actor_id = Some(actor_id.to_owned());
        if !reason.trim().is_empty() {
            pm.resolution_reason = Some(reason.to_owned());
        }
        (f_idx, f_cash, pm.clone())
    };

    let (earmark_seg_idx, released_earmark_cash) = {
        let earmark = account
            .earmarks
            .get_mut(target_earmark_id)
            .ok_or_else(|| BlotterError::not_found("target earmark disappeared"))?;
        let (e_idx, e_cash) = if earmark.allocation_pool.active_segment.is_some() {
            let (idx, cash) = earmark.allocation_pool.allocate_slice(offset_notional)?;
            if let Some(rem_cash) = earmark.unreconciled_settlement_cash {
                earmark.unreconciled_settlement_cash = Some(checked_sub(rem_cash, cash)?);
            }
            (Some(idx), Some(cash))
        } else {
            (None, None)
        };
        earmark.unreconciled_notional =
            checked_sub(earmark.unreconciled_notional, offset_notional)?;
        if earmark.unreconciled_notional == FixedDecimal::ZERO {
            earmark.state = EarmarkState::Reconciled;
        }
        earmark.earmark_revision += 1;
        earmark.updated_seq = next_seq;
        (e_idx, e_cash)
    };

    let pos = account
        .positions
        .get_mut(&updated_pm.instrument_code)
        .ok_or_else(|| BlotterError::not_found("position missing on earmark offset"))?;
    add_reconciled_notional(
        pos,
        updated_pm.side,
        updated_pm.settlement_bucket,
        offset_notional,
    )?;
    pos.position_version += 1;

    let alloc_id = id_gen.next_ulid();
    let alloc = ReconcileAllocationRecord {
        allocation_id: alloc_id.clone(),
        account_id: account.account_id.clone(),
        instrument_code: updated_pm.instrument_code.clone(),
        side: updated_pm.side,
        settlement_bucket: updated_pm.settlement_bucket,
        source_id: updated_pm.source_id.clone(),
        external_trade_id: updated_pm.external_trade_id.clone(),
        source_revision: updated_pm.source_revision,
        earmark_id: Some(target_earmark_id.clone()),
        pending_match_id: Some(updated_pm.pending_match_id.clone()),
        allocated_notional: offset_notional,
        allocated_fill_cash: fill_cash_slice,
        fill_funding_precision: updated_pm.funding_precision,
        fill_segment_index: fill_seg_idx,
        earmark_segment_index: earmark_seg_idx,
        reversed_by_revision: None,
        released_earmark_cash,
        baseline_delta: None,
        actor_id: actor_id.to_owned(),
    };
    account.allocations.insert(alloc_id, alloc.clone());

    Ok(ResolvePendingMatchEffect {
        pending_match: updated_pm,
        updated_earmark_id: Some(target_earmark_id.clone()),
        allocation: Some(alloc),
    })
}

fn forbid_hold_or_baseline(
    account: &AccountState,
    feed: &SourceFeedState,
    pm: &PendingMatchRecord,
    action_name: &str,
) -> BlotterResult<()> {
    match pm.reason {
        PendingMatchReason::RevisionHold => {
            return Err(BlotterError::invalid_state_transition(format!(
                "{action_name} is forbidden while pending match is in REVISION_HOLD"
            )));
        }
        PendingMatchReason::BaselineRevisionManual => {
            return Err(BlotterError::invalid_state_transition(format!(
                "{action_name} is forbidden for BASELINE_REVISION_MANUAL; use APPLY_BASELINE_DELTA"
            )));
        }
        _ => {}
    }

    // Block manual resolution while the same four-key K still has a genuine unresolved RevisionHold
    // or while the source feed still has an active missing-seq gap. Once the real hold/gap clears,
    // collaterally deferred complete trades (including GapHold / ExistingPendingOnKey) are unlocked.
    let key_has_active_revision_hold = account.pending_matches.values().any(|other| {
        other.state == PendingMatchState::Open
            && other.reason == PendingMatchReason::RevisionHold
            && other.account_id == pm.account_id
            && other.instrument_code == pm.instrument_code
            && other.side == pm.side
            && other.settlement_date == pm.settlement_date
            && feed
                .trade_chains
                .get(&other.external_trade_id)
                .is_some_and(|chain| !chain.held_revisions.is_empty())
    });
    if key_has_active_revision_hold {
        return Err(BlotterError::invalid_state_transition(format!(
            "{action_name} is forbidden while key has an active REVISION_HOLD"
        )));
    }

    if !feed.missing_feed_ranges.is_empty() {
        return Err(BlotterError::invalid_state_transition(format!(
            "{action_name} is forbidden while feed source {} has an active gap",
            feed.source_id
        )));
    }

    Ok(())
}

enum QuoteComparison {
    WithinTolerance,
    OutOfTolerance,
    Incomparable,
}

fn compare_quote_terms(
    fill_terms: Option<&crate::blotter::model::QuoteTerms>,
    earmark_terms: Option<&crate::blotter::model::QuoteTerms>,
    config: &BlotterConfig,
) -> QuoteComparison {
    let (Some(ft), Some(et)) = (fill_terms, earmark_terms) else {
        return QuoteComparison::Incomparable;
    };
    match (ft.basis, et.basis) {
        (Some(QuoteBasis::CleanPricePer100(fp)), Some(QuoteBasis::CleanPricePer100(ep))) => {
            let diff = fp.scaled().abs_diff(ep.scaled());
            if diff <= config.clean_price_tolerance.scaled().unsigned_abs() {
                QuoteComparison::WithinTolerance
            } else {
                QuoteComparison::OutOfTolerance
            }
        }
        (Some(QuoteBasis::YieldToMaturity(fy)), Some(QuoteBasis::YieldToMaturity(ey))) => {
            let diff = fy.scaled().abs_diff(ey.scaled());
            if diff <= config.yield_tolerance.scaled().unsigned_abs() {
                QuoteComparison::WithinTolerance
            } else {
                QuoteComparison::OutOfTolerance
            }
        }
        _ => QuoteComparison::Incomparable,
    }
}

fn validate_revision_chain_actions(
    feed: &SourceFeedState,
    trade_id: &str,
    contiguous_ready: &[u64],
) -> BlotterResult<()> {
    let mut prev_was_cancel = feed
        .trade_chains
        .get(trade_id)
        .and_then(|chain| chain.latest_effective_revision)
        .and_then(|rev| feed.trade_revisions.get(&(trade_id.to_owned(), rev)))
        .is_some_and(|stored| stored.record.action == TradeFeedRecordAction::CancelReversal);

    let mut expected_account: Option<&str> = feed
        .trade_chains
        .get(trade_id)
        .and_then(|chain| chain.original_new_fill.as_ref())
        .map(|orig| orig.account_id.as_str());

    for &rev in contiguous_ready {
        let rec = &feed
            .trade_revisions
            .get(&(trade_id.to_owned(), rev))
            .ok_or_else(|| BlotterError::validation("missing revision during chain check"))?
            .record;
        if let Some(exp) = expected_account {
            if rec.account_id != exp {
                return Err(BlotterError::validation(format!(
                    "immutable account_id violation in revision chain for ({trade_id}, rev {rev}): expected {exp}, got {}",
                    rec.account_id
                )));
            }
        } else {
            expected_account = Some(rec.account_id.as_str());
        }
        if rev == 1 {
            if rec.action != TradeFeedRecordAction::NewFill {
                return Err(BlotterError::invalid_state_transition(
                    "source_revision 1 must be NEW_FILL",
                ));
            }
            prev_was_cancel = false;
        } else {
            if rec.action == TradeFeedRecordAction::NewFill {
                return Err(BlotterError::invalid_state_transition(
                    "source_revision > 1 cannot be NEW_FILL",
                ));
            }
            if prev_was_cancel {
                return Err(BlotterError::invalid_state_transition(
                    "cannot amend or cancel a trade after CANCEL_REVERSAL",
                ));
            }
            prev_was_cancel = rec.action == TradeFeedRecordAction::CancelReversal;
        }
    }
    Ok(())
}

fn find_opening_baseline_evidence(
    account: &AccountState,
    source_id: &str,
    external_trade_id: &str,
) -> Option<BaseEvidence> {
    account
        .opening_base
        .baseline_evidence
        .iter()
        .find(|ev| {
            ev.source_id == source_id
                && ev.external_trade_id == external_trade_id
                && ev.satisfies_watermark(&account.opening_base)
        })
        .cloned()
}

fn is_record_pre_watermark(
    rec: &TradeFillRecord,
    opening_base: &crate::blotter::model::OpeningPositionBase,
) -> bool {
    if let Some(wm_seq) = opening_base.watermark_source_seq {
        rec.feed_seq <= wm_seq
    } else {
        rec.executed_at.instant() <= opening_base.watermark_time.instant()
    }
}

fn try_build_evidence_from_original_new_fill(
    orig: &TradeFillRecord,
    opening_base: &crate::blotter::model::OpeningPositionBase,
) -> BlotterResult<Option<BaseEvidence>> {
    let bucket = opening_base
        .bucket_for_settlement_date(orig.settlement_date)
        .ok_or_else(|| {
            BlotterError::validation("original NEW_FILL settlement_date outside buckets")
        })?;
    let ResolvedSettlementCash { signed_cash, .. } = resolve_settlement_cash(
        orig.side,
        orig.notional,
        orig.quote_terms.as_ref(),
        orig.exact_dirty_price_input,
    )?;
    let Some(old_signed_cash) = signed_cash else {
        return Ok(None);
    };
    Ok(Some(BaseEvidence {
        source_id: orig.source_id.clone(),
        external_trade_id: orig.external_trade_id.clone(),
        original_feed_seq: orig.feed_seq,
        original_executed_at: orig.executed_at.clone(),
        old_notional: orig.notional,
        old_signed_cash,
        settlement_bucket: bucket,
        evidence_ref: format!(
            "raw_new_fill:{}:{}:{}",
            orig.source_id, orig.external_trade_id, orig.source_revision
        ),
        evidence_hash: orig.business_hash(),
        instrument_code: orig.instrument_code.clone(),
        side: orig.side,
        original_revision: orig.source_revision,
    }))
}

fn ensure_position_exists(account: &mut AccountState, instrument_code: &str) {
    account
        .positions
        .entry(instrument_code.to_owned())
        .or_insert_with(|| InstrumentPositionState::zero(instrument_code.to_owned()));
}

fn compute_missing_ranges(
    contiguous_feed_seq: u64,
    max_seen_feed_seq: u64,
    raw_packets: &BTreeMap<u64, StoredRawPacket>,
) -> Vec<SeqRange> {
    if max_seen_feed_seq <= contiguous_feed_seq {
        return Vec::new();
    }

    let mut present_above: Vec<u64> = raw_packets
        .range((contiguous_feed_seq + 1)..=max_seen_feed_seq)
        .map(|(&seq, _)| seq)
        .collect();
    present_above.sort_unstable();

    let mut ranges = Vec::new();
    let mut cursor = contiguous_feed_seq + 1;
    for seq in present_above {
        if seq > cursor {
            ranges.push(SeqRange {
                first: cursor,
                last: seq - 1,
            });
        }
        cursor = seq.saturating_add(1);
    }
    if cursor <= max_seen_feed_seq {
        ranges.push(SeqRange {
            first: cursor,
            last: max_seen_feed_seq,
        });
    }
    ranges
}

fn has_newly_missing_seq(prev: &[SeqRange], next: &[SeqRange]) -> bool {
    for candidate in next {
        let mut cursor = candidate.first;
        for existing in prev {
            if existing.last < cursor {
                continue;
            }
            if existing.first > cursor {
                return true;
            }
            if existing.last >= candidate.last {
                cursor = candidate.last.saturating_add(1);
                break;
            }
            cursor = existing.last.saturating_add(1);
        }
        if cursor <= candidate.last {
            return true;
        }
    }
    false
}
