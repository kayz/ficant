use std::collections::{BTreeMap, BTreeSet};

use chrono::Duration;
use ficant_domain::primitives::{ContentHash, MarketTime};

use crate::ports::ApplicationResult;
use crate::use_cases::blotter::model::{
    AccountDeskPolicy, DeskBlotterWorkspaceState, MarketQuoteTick, StoredMarketQuoteTick, invalid,
    require_non_empty_text,
};
use crate::{ApplicationError, ApplicationErrorCategory};

/// Input for ingesting a batch of market quote ticks from a single source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestMarketQuotesInput {
    pub account_id: String,
    pub source_id: String,
    pub ticks: Vec<MarketQuoteTick>,
}

/// Pure output of quote store ingestion before snapshot/risk projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuoteIngestEffect {
    pub committed_seq: u64,
    pub mutated: bool,
    pub applied_tick_count: u32,
    pub skipped_stale_tick_count: u32,
    pub backfilled_tick_count: u32,
    pub affected_accounts: Vec<String>,
}

/// Command to switch the active valuation quote source for an account or one instrument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchActiveQuoteSourceCommand {
    pub account_id: String,
    /// `None` switches `default_quote_source_id`; `Some(code)` sets a per-instrument override.
    pub instrument_code: Option<String>,
    pub new_source_id: String,
}

/// Output of switching an active quote source (a visible policy-versioned transaction).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchActiveQuoteSourceEffect {
    pub committed_seq: u64,
    pub mutated: bool,
    pub policy_version: u64,
    pub affected_accounts: Vec<String>,
}

/// Evaluates whether a stored market quote is stale relative to the server clock `now`.
///
/// Uses validated `observed_at`, immutable `first_received_at`, and immutable
/// `first_ingested_at` so replaying an already-seen quote can NEVER extend its freshness.
#[must_use]
pub fn is_quote_stale(
    stored: &StoredMarketQuoteTick,
    now: &MarketTime,
    quote_staleness_seconds: u32,
) -> bool {
    let max_age = Duration::seconds(i64::from(quote_staleness_seconds));
    let observed_age = now
        .instant()
        .signed_duration_since(stored.tick.observed_at.instant());
    let received_age = now
        .instant()
        .signed_duration_since(stored.first_received_at.instant());
    let ingested_age = now
        .instant()
        .signed_duration_since(stored.first_ingested_at.instant());
    observed_age >= max_age || received_age >= max_age || ingested_age >= max_age
}

/// Atomically validates and ingests a batch of quotes keyed by `(source_id, instrument_code, quote_seq)`.
///
/// Guarantees:
/// - All ticks in the batch share `input.source_id`.
/// - Conflicting payload for any existing or in-batch `(source_id, instrument_code, quote_seq)`
///   rejects the entire batch with `HashMismatch` before mutating state.
/// - Distinct sources maintain independent sequence spaces (`source A seq=100` coexists with
///   `source B seq=1`).
/// - Older unseen sequences backfill `history` without rewinding `heads`.
/// - Exact duplicate replays are deduplicated without modifying `first_received_at` / `first_ingested_at`.
#[allow(clippy::too_many_lines)]
pub fn apply_ingest_market_quotes_state(
    workspace: &DeskBlotterWorkspaceState,
    input: &IngestMarketQuotesInput,
    now: &MarketTime,
) -> ApplicationResult<(DeskBlotterWorkspaceState, QuoteIngestEffect)> {
    require_non_empty_text(&input.account_id)?;
    require_non_empty_text(&input.source_id)?;
    if !workspace.domain.accounts.contains_key(&input.account_id)
        || !workspace.accounts_meta.contains_key(&input.account_id)
    {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::NotFound,
            false,
        ));
    }

    // Phase 1: full batch validation & atomic conflict detection before any mutation.
    let mut batch_hashes: BTreeMap<(String, String, u64), ContentHash> = BTreeMap::new();
    for tick in &input.ticks {
        tick.validate()?;
        if tick.source_id != input.source_id {
            return Err(invalid());
        }
        let key = (
            tick.source_id.clone(),
            tick.instrument_code.clone(),
            tick.quote_seq,
        );
        let payload_hash = tick.canonical_payload_hash();

        if let Some(existing) = workspace.quotes.history.get(&key)
            && existing.payload_hash != payload_hash
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::HashMismatch,
                false,
            ));
        }
        if let Some(prev_hash) = batch_hashes.get(&key)
            && prev_hash != &payload_hash
        {
            return Err(ApplicationError::new(
                ApplicationErrorCategory::HashMismatch,
                false,
            ));
        }
        batch_hashes.insert(key, payload_hash);
    }

    // Phase 2: apply ticks atomically on a cloned workspace state.
    let mut next = workspace.clone();
    let mut applied_tick_count = 0_u32;
    let mut skipped_stale_tick_count = 0_u32;
    let mut backfilled_tick_count = 0_u32;
    let mut mutated = false;
    let mut head_advanced_instruments: BTreeSet<String> = BTreeSet::new();

    for tick in &input.ticks {
        let key = (
            tick.source_id.clone(),
            tick.instrument_code.clone(),
            tick.quote_seq,
        );
        if next.quotes.history.contains_key(&key) {
            // Identical replay: never overwrite first_received_at / first_ingested_at.
            skipped_stale_tick_count = skipped_stale_tick_count.saturating_add(1);
            continue;
        }

        let payload_hash = tick.canonical_payload_hash();
        next.quotes.history.insert(
            key,
            StoredMarketQuoteTick {
                tick: tick.clone(),
                payload_hash,
                first_received_at: tick.received_at.clone(),
                first_ingested_at: now.clone(),
            },
        );
        mutated = true;

        let pair_key = (tick.source_id.clone(), tick.instrument_code.clone());
        let current_head = next.quotes.heads.get(&pair_key).copied();
        if current_head.is_none_or(|head| tick.quote_seq > head) {
            next.quotes.heads.insert(pair_key, tick.quote_seq);
            applied_tick_count = applied_tick_count.saturating_add(1);
            head_advanced_instruments.insert(tick.instrument_code.clone());
        } else {
            // Historical backfill: stored in history, head NOT rewound.
            backfilled_tick_count = backfilled_tick_count.saturating_add(1);
            skipped_stale_tick_count = skipped_stale_tick_count.saturating_add(1);
        }
    }

    let mut affected_set: BTreeSet<String> = BTreeSet::new();
    if mutated {
        next.domain.head_seq = next.domain.head_seq.checked_add(1).ok_or_else(invalid)?;

        for (acct_id, acct) in &next.domain.accounts {
            let Some(meta) = next.accounts_meta.get(acct_id) else {
                continue;
            };
            let uses_source_for_updated_inst = head_advanced_instruments.iter().any(|inst| {
                acct.positions.contains_key(inst)
                    && meta.policy.active_quote_source_for(inst) == input.source_id
            });
            if uses_source_for_updated_inst {
                affected_set.insert(acct_id.clone());
            }
        }
    }

    let committed_seq = next.domain.head_seq;
    Ok((
        next,
        QuoteIngestEffect {
            committed_seq,
            mutated,
            applied_tick_count,
            skipped_stale_tick_count,
            backfilled_tick_count,
            affected_accounts: affected_set.into_iter().collect(),
        },
    ))
}

/// Switches the active valuation quote source for an account or one instrument as a visible
/// policy-versioned transaction.
pub fn apply_switch_active_quote_source_state(
    workspace: &DeskBlotterWorkspaceState,
    cmd: &SwitchActiveQuoteSourceCommand,
) -> ApplicationResult<(DeskBlotterWorkspaceState, SwitchActiveQuoteSourceEffect)> {
    require_non_empty_text(&cmd.account_id)?;
    require_non_empty_text(&cmd.new_source_id)?;
    if let Some(inst) = &cmd.instrument_code {
        require_non_empty_text(inst)?;
    }

    let acct = workspace
        .domain
        .accounts
        .get(&cmd.account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    let meta = workspace
        .accounts_meta
        .get(&cmd.account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;

    if let Some(inst) = &cmd.instrument_code
        && !acct.positions.contains_key(inst)
    {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::NotFound,
            false,
        ));
    }

    let changed = match &cmd.instrument_code {
        Some(inst) => meta.policy.active_quote_source_for(inst) != cmd.new_source_id,
        None => meta.policy.default_quote_source_id != cmd.new_source_id,
    };

    if !changed {
        return Ok((
            workspace.clone(),
            SwitchActiveQuoteSourceEffect {
                committed_seq: workspace.domain.head_seq,
                mutated: false,
                policy_version: meta.policy.policy_version,
                affected_accounts: Vec::new(),
            },
        ));
    }

    let mut next = workspace.clone();
    next.domain.head_seq = next.domain.head_seq.checked_add(1).ok_or_else(invalid)?;
    let next_acct = next
        .domain
        .accounts
        .get_mut(&cmd.account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    next_acct.account_version += 1;

    let next_meta = next
        .accounts_meta
        .get_mut(&cmd.account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    match &cmd.instrument_code {
        Some(inst) => {
            next_meta
                .policy
                .instrument_quote_sources
                .insert(inst.clone(), cmd.new_source_id.clone());
        }
        None => {
            next_meta
                .policy
                .default_quote_source_id
                .clone_from(&cmd.new_source_id);
        }
    }
    next_meta.policy.policy_version += 1;
    let policy_version = next_meta.policy.policy_version;
    let committed_seq = next.domain.head_seq;

    Ok((
        next,
        SwitchActiveQuoteSourceEffect {
            committed_seq,
            mutated: true,
            policy_version,
            affected_accounts: vec![cmd.account_id.clone()],
        },
    ))
}

/// Reconciles persisted account desk policies against startup configuration.
///
/// Per BP-2 design §5, if a restart detects changed business policy fields for any account
/// (quote sources, staleness threshold, draft hold TTL, hedge allowlist, regulatory rule pack,
/// or limit ceilings), a visible policy-versioned transaction MUST be committed before serving
/// requests so stale draft previews are invalidated.
pub fn apply_startup_quote_source_reconciliation(
    workspace: &DeskBlotterWorkspaceState,
    configured_policies: &BTreeMap<String, AccountDeskPolicy>,
) -> ApplicationResult<(DeskBlotterWorkspaceState, SwitchActiveQuoteSourceEffect)> {
    let mut next = workspace.clone();
    let mut changed_accounts = Vec::new();
    let mut last_policy_version = 0_u64;

    for (acct_id, desired) in configured_policies {
        desired.validate()?;
        let Some(meta) = next.accounts_meta.get_mut(acct_id) else {
            continue;
        };
        if !business_policy_eq(&meta.policy, desired) {
            meta.policy
                .default_quote_source_id
                .clone_from(&desired.default_quote_source_id);
            meta.policy
                .instrument_quote_sources
                .clone_from(&desired.instrument_quote_sources);
            meta.policy.quote_staleness_seconds = desired.quote_staleness_seconds;
            meta.policy.default_draft_hold_ttl_seconds = desired.default_draft_hold_ttl_seconds;
            meta.policy
                .allowed_hedge_product_codes
                .clone_from(&desired.allowed_hedge_product_codes);
            meta.policy
                .regulatory_rule_pack
                .clone_from(&desired.regulatory_rule_pack);
            meta.policy
                .limit_ceilings
                .clone_from(&desired.limit_ceilings);
            meta.policy.policy_version = meta
                .policy
                .policy_version
                .max(desired.policy_version)
                .saturating_add(1);
            last_policy_version = meta.policy.policy_version;
            if let Some(acct) = next.domain.accounts.get_mut(acct_id) {
                acct.account_version += 1;
            }
            changed_accounts.push(acct_id.clone());
        }
    }

    if changed_accounts.is_empty() {
        return Ok((
            workspace.clone(),
            SwitchActiveQuoteSourceEffect {
                committed_seq: workspace.domain.head_seq,
                mutated: false,
                policy_version: 0,
                affected_accounts: Vec::new(),
            },
        ));
    }

    next.domain.head_seq = next.domain.head_seq.checked_add(1).ok_or_else(invalid)?;
    let committed_seq = next.domain.head_seq;

    Ok((
        next,
        SwitchActiveQuoteSourceEffect {
            committed_seq,
            mutated: true,
            policy_version: last_policy_version,
            affected_accounts: changed_accounts,
        },
    ))
}

fn business_policy_eq(current: &AccountDeskPolicy, desired: &AccountDeskPolicy) -> bool {
    let AccountDeskPolicy {
        policy_version: _,
        default_quote_source_id,
        instrument_quote_sources,
        quote_staleness_seconds,
        default_draft_hold_ttl_seconds,
        allowed_hedge_product_codes,
        regulatory_rule_pack,
        limit_ceilings,
    } = current;
    default_quote_source_id == &desired.default_quote_source_id
        && instrument_quote_sources == &desired.instrument_quote_sources
        && *quote_staleness_seconds == desired.quote_staleness_seconds
        && *default_draft_hold_ttl_seconds == desired.default_draft_hold_ttl_seconds
        && allowed_hedge_product_codes == &desired.allowed_hedge_product_codes
        && regulatory_rule_pack == &desired.regulatory_rule_pack
        && limit_ceilings == &desired.limit_ceilings
}
