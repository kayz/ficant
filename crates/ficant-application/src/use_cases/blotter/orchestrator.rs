use std::collections::{BTreeMap, BTreeSet};

use ficant_domain::blotter::{
    BlotterIdGenerator, CancelCommittedCommand, ConfirmHoldCommand, CreateCommittedCommand,
    CreateHoldCommand, EarmarkRecord, EarmarkState, ExtendHoldCommand, FeedStatusView,
    FundingGapSummary, ImportOpeningBaseCommand, IngestTradeFeedInput, PendingMatchRecord,
    PendingMatchResolutionAction, PendingMatchState, ReconcileAllocationRecord, ReleaseHoldCommand,
    ResolvePendingMatchInput, SetFeedControlCommand, SweepLifecycleCommand, TradeFillIngestOutcome,
    TradeFillRecord, apply_cancel_committed, apply_confirm_hold, apply_create_committed,
    apply_create_hold, apply_extend_hold, apply_import_opening_base, apply_ingest_trade_feed,
    apply_release_hold, apply_resolve_pending_match, apply_set_feed_control, apply_sweep_lifecycle,
};
use ficant_domain::primitives::MarketTime;

use crate::ports::{
    ApplicationResult, BlotterClock, BondAnalyticsEngine, FuturesDeliveryRuleParser,
    FuturesHedgeEngine, RegulatoryRulePackParser, TradeFeedReconcilerPort,
};
use crate::use_cases::blotter::drafts::{
    ConfirmDraftEarmarkCommand, ConfirmDraftEarmarkOutput, SubmitDraftEarmarkCommand,
    SubmitDraftEarmarkOutput, apply_confirm_draft_earmark_state, apply_submit_draft_earmark_state,
};
use crate::use_cases::blotter::model::{
    AccountChange, AccountDeskPolicy, AccountMetadataState, BlotterEventEnvelope,
    DeskBlotterWorkspaceState, DraftEarmarkStatus, EnrichedAccountBlotterSnapshot,
    FullOpeningPositionBase, InstrumentCatalogEntry, PreviewCoveredHedgeInput,
    PreviewCoveredHedgeOutput, RiskAndCoverageSummary, ValuedInstrumentPositionView, invalid,
    require_non_empty_text,
};
use crate::use_cases::blotter::quotes::{
    IngestMarketQuotesInput, SwitchActiveQuoteSourceCommand, apply_ingest_market_quotes_state,
    apply_startup_quote_source_reconciliation, apply_switch_active_quote_source_state,
};
use crate::use_cases::blotter::valuation::{
    compute_trade_exact_dirty_price_input, evaluate_account_positions_and_risk,
    evaluate_single_instrument_position, execute_preview_covered_hedge,
};
use crate::{ApplicationError, ApplicationErrorCategory, map_blotter_error};

/// Pure synchronous pricing and rule-parsing engines injected into desk-blotter orchestration.
pub struct BlotterEvaluationEngines<'a> {
    pub bond_engine: &'a dyn BondAnalyticsEngine,
    pub hedge_engine: &'a dyn FuturesHedgeEngine,
    pub delivery_rule_parser: &'a dyn FuturesDeliveryRuleParser,
    pub regulatory_rule_parser: &'a dyn RegulatoryRulePackParser,
}

/// Command to import an account's immutable full opening position base once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportFullOpeningBaseCommand {
    pub actor_id: String,
    pub idempotency_key: String,
    pub opening_base: FullOpeningPositionBase,
    pub expected_account_version: u64,
    pub policy: AccountDeskPolicy,
}

/// Output of `ImportOpeningBase`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportFullOpeningBaseOutput {
    pub committed_seq: u64,
    pub replayed: bool,
    pub snapshot: EnrichedAccountBlotterSnapshot,
}

/// Output of `CreateHold`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedCreateHoldOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
    pub position: ValuedInstrumentPositionView,
}

/// Output of `ExtendHold`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedExtendHoldOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
}

/// Output of `ConfirmHold` (including committed lazy-expiration rejection detail).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedConfirmHoldOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
    pub position: ValuedInstrumentPositionView,
    pub risk_summary: RiskAndCoverageSummary,
    pub funding_summary: FundingGapSummary,
    pub reconfirmation_error: Option<ApplicationError>,
}

/// Output of `ReleaseHold`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedReleaseHoldOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
    pub position: ValuedInstrumentPositionView,
}

/// Output of `CreateCommitted`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedCreateCommittedOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
    pub position: ValuedInstrumentPositionView,
    pub risk_summary: RiskAndCoverageSummary,
    pub funding_summary: FundingGapSummary,
}

/// Output of `CancelCommitted`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedCancelCommittedOutput {
    pub committed_seq: u64,
    pub earmark: EarmarkRecord,
    pub position: ValuedInstrumentPositionView,
    pub risk_summary: RiskAndCoverageSummary,
    pub funding_summary: FundingGapSummary,
}

/// Missing predecessor revision identified for `TradeFeedReconcilerPort::fetch_trade_revision`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MissingTradeRevisionRequest {
    pub source_id: String,
    pub external_trade_id: String,
    pub source_revision: u64,
}

/// Output of `IngestTradeFeed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedIngestTradeFeedOutput {
    pub committed_seq: u64,
    pub mutated: bool,
    pub item_outcomes: Vec<TradeFillIngestOutcome>,
    pub feed_status: FeedStatusView,
    pub affected_accounts: Vec<String>,
    pub changed_allocations: Vec<ReconcileAllocationRecord>,
    pub missing_revisions: Vec<MissingTradeRevisionRequest>,
}

/// Output of synchronous `FetchTradeRevision` historical backfill orchestration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackfillTradeRevisionsOutput {
    pub fetched_count: u32,
    pub not_found_revisions: Vec<MissingTradeRevisionRequest>,
    pub ingest_output: Option<OrchestratedIngestTradeFeedOutput>,
}

/// Output of `ResolvePendingMatch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedResolvePendingMatchOutput {
    pub committed_seq: u64,
    pub pending_match: PendingMatchRecord,
    pub updated_earmark: Option<EarmarkRecord>,
    pub position: ValuedInstrumentPositionView,
    pub risk_summary: RiskAndCoverageSummary,
    pub funding_summary: FundingGapSummary,
    pub allocation: Option<ReconcileAllocationRecord>,
}

/// Output of `SweepLifecycle`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedSweepLifecycleOutput {
    pub head_seq: u64,
    pub mutated: bool,
    pub expired_hold_count: u32,
    pub newly_alerted_committed_count: u32,
    pub newly_stale_feed_count: u32,
}

/// Output of `SetFeedControl`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedSetFeedControlOutput {
    pub committed_seq: u64,
    pub feed_status: FeedStatusView,
}

/// Output of `IngestMarketQuotes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedIngestMarketQuotesOutput {
    pub committed_seq: u64,
    pub mutated: bool,
    pub applied_tick_count: u32,
    pub skipped_stale_tick_count: u32,
    pub backfilled_tick_count: u32,
    pub risk_summary: RiskAndCoverageSummary,
    pub affected_accounts: Vec<String>,
}

/// Output of `SwitchActiveQuoteSource` or startup quote-source reconciliation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratedSwitchQuoteSourceOutput {
    pub committed_seq: u64,
    pub mutated: bool,
    pub policy_version: u64,
    pub affected_accounts: Vec<String>,
    pub risk_summary: Option<RiskAndCoverageSummary>,
}

/// Returns the deterministic sorted list of missing predecessor revisions on `source_id`
/// requiring `FetchTradeRevision` historical backfill.
#[must_use]
pub fn pending_missing_trade_revisions(
    workspace: &DeskBlotterWorkspaceState,
    source_id: &str,
) -> Vec<MissingTradeRevisionRequest> {
    let Some(feed) = workspace.domain.feeds.get(source_id) else {
        return Vec::new();
    };
    let mut requests = Vec::new();
    for (trade_id, chain) in &feed.trade_chains {
        for &rev in &chain.missing_predecessor_revisions {
            requests.push(MissingTradeRevisionRequest {
                source_id: source_id.to_owned(),
                external_trade_id: trade_id.clone(),
                source_revision: rev,
            });
        }
    }
    requests.sort();
    requests
}

/// Pure synchronous L2 application orchestrator for all Desk Blotter use cases.
pub struct DeskBlotterOrchestrator<'a> {
    clock: &'a dyn BlotterClock,
    engines: BlotterEvaluationEngines<'a>,
}

impl<'a> DeskBlotterOrchestrator<'a> {
    #[must_use]
    pub const fn new(clock: &'a dyn BlotterClock, engines: BlotterEvaluationEngines<'a>) -> Self {
        Self { clock, engines }
    }

    /// Projects a complete enriched snapshot for one account at `workspace.domain.head_seq`.
    pub fn get_account_snapshot(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        account_id: &str,
    ) -> ApplicationResult<EnrichedAccountBlotterSnapshot> {
        let now = self.clock.now();
        build_enriched_snapshot(workspace, account_id, &now, self.engines.bond_engine)
    }

    /// Imports an account's immutable opening position base (allowed at most once).
    ///
    /// Includes L2 cross-account `baseline_evidence` conflict defense on shared feed sources.
    pub fn import_opening_base(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &ImportFullOpeningBaseCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        ImportFullOpeningBaseOutput,
        Option<BlotterEventEnvelope>,
    )> {
        cmd.policy.validate()?;
        let instruments_map = validate_and_index_catalog(&cmd.opening_base)?;

        // Cross-account baseline_evidence conflict defense on shared sources (only for new accounts;
        // existing accounts proceed to L1 idempotent replay or ImmutableViolation handling).
        if !workspace
            .domain
            .accounts
            .contains_key(&cmd.opening_base.core.account_id)
        {
            validate_no_cross_account_baseline_evidence_conflict(workspace, &cmd.opening_base)?;
        }

        let (next_domain, out) = apply_import_opening_base(
            &workspace.domain,
            &ImportOpeningBaseCommand {
                actor_id: cmd.actor_id.clone(),
                idempotency_key: cmd.idempotency_key.clone(),
                opening_base: cmd.opening_base.core.clone(),
                expected_account_version: cmd.expected_account_version,
            },
        )
        .map_err(|err| map_blotter_error(&err))?;

        let now = self.clock.now();
        if out.replayed {
            let snapshot = build_enriched_snapshot(
                workspace,
                &cmd.opening_base.core.account_id,
                &now,
                self.engines.bond_engine,
            )?;
            return Ok((
                workspace.clone(),
                ImportFullOpeningBaseOutput {
                    committed_seq: out.committed_seq,
                    replayed: true,
                    snapshot,
                },
                None,
            ));
        }

        let mut next = workspace.clone();
        next.domain = next_domain;
        next.accounts_meta.insert(
            cmd.opening_base.core.account_id.clone(),
            AccountMetadataState {
                instruments: instruments_map,
                calendar: cmd.opening_base.calendar.clone(),
                default_calendar_requirement: cmd.opening_base.default_calendar_requirement,
                units: cmd.opening_base.units.clone(),
                policy: cmd.policy.clone(),
                drafts: BTreeMap::new(),
            },
        );

        let snapshot = build_enriched_snapshot(
            &next,
            &cmd.opening_base.core.account_id,
            &now,
            self.engines.bond_engine,
        )?;
        let event = BlotterEventEnvelope {
            seq: out.committed_seq,
            occurred_at: now,
            changes: vec![AccountChange {
                account_id: cmd.opening_base.core.account_id.clone(),
                replace_snapshot: snapshot.clone(),
                changed_allocations: Vec::new(),
            }],
            event_type: "IMPORT_OPENING_BASE".to_owned(),
        };

        Ok((
            next,
            ImportFullOpeningBaseOutput {
                committed_seq: out.committed_seq,
                replayed: false,
                snapshot,
            },
            Some(event),
        ))
    }

    /// Creates an in-term `HOLD` earmark.
    pub fn create_hold(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &CreateHoldCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedCreateHoldOutput,
        BlotterEventEnvelope,
    )> {
        let now = self.clock.now();
        let (next_domain, out) = apply_create_hold(&workspace.domain, cmd, &now)
            .map_err(|err| map_blotter_error(&err))?;
        let mut next = workspace.clone();
        next.domain = next_domain;

        let position = evaluate_single_instrument_position(
            &next,
            &cmd.account_id,
            &cmd.instrument_code,
            &now,
            self.engines.bond_engine,
        )?;
        let event = build_single_account_event(
            &next,
            &cmd.account_id,
            out.committed_seq,
            &now,
            "CREATE_HOLD",
            Vec::new(),
            self.engines.bond_engine,
        )?;

        Ok((
            next,
            OrchestratedCreateHoldOutput {
                committed_seq: out.committed_seq,
                earmark: out.earmark,
                position,
            },
            event,
        ))
    }

    /// Extends an in-term `HOLD` earmark from `clock.now()`.
    pub fn extend_hold(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &ExtendHoldCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedExtendHoldOutput,
        BlotterEventEnvelope,
    )> {
        let now = self.clock.now();
        let (next_domain, out) = apply_extend_hold(&workspace.domain, cmd, &now)
            .map_err(|err| map_blotter_error(&err))?;
        let mut next = workspace.clone();
        next.domain = next_domain;

        let event = build_single_account_event(
            &next,
            &cmd.account_id,
            out.committed_seq,
            &now,
            "EXTEND_HOLD",
            Vec::new(),
            self.engines.bond_engine,
        )?;

        Ok((
            next,
            OrchestratedExtendHoldOutput {
                committed_seq: out.committed_seq,
                earmark: out.earmark,
            },
            event,
        ))
    }

    /// Confirms a `HOLD` (or already-swept `EXPIRED`) earmark into `COMMITTED`.
    ///
    /// Automatically computes exact trade settlement-date dirty price when eligible and no
    /// explicit `settlement_amount` was supplied.
    pub fn confirm_hold(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &ConfirmHoldCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedConfirmHoldOutput,
        BlotterEventEnvelope,
    )> {
        let now = self.clock.now();
        let mut enriched_cmd = cmd.clone();
        if enriched_cmd.exact_dirty_price_input.is_none()
            && let Some(acct) = workspace.domain.accounts.get(&cmd.account_id)
            && let Some(meta) = workspace.accounts_meta.get(&cmd.account_id)
            && let Some(earmark) = acct.earmarks.get(&cmd.earmark_id)
        {
            enriched_cmd.exact_dirty_price_input = compute_trade_exact_dirty_price_input(
                meta,
                &earmark.instrument_code,
                earmark.settlement_date,
                Some(&cmd.quote_terms),
                self.engines.bond_engine,
            );
        }

        let (next_domain, out) = apply_confirm_hold(&workspace.domain, &enriched_cmd, &now)
            .map_err(|err| map_blotter_error(&err))?;
        let mut next = workspace.clone();
        next.domain = next_domain;

        let position = evaluate_single_instrument_position(
            &next,
            &cmd.account_id,
            &out.earmark.instrument_code,
            &now,
            self.engines.bond_engine,
        )?;
        let (_positions, risk_summary) = evaluate_account_positions_and_risk(
            &next,
            &cmd.account_id,
            &now,
            self.engines.bond_engine,
        )?;
        let reconfirmation_error = out.reconfirmation_error.as_ref().map(map_blotter_error);
        let event_type = if reconfirmation_error.is_some() {
            "CONFIRM_HOLD_EXPIRED_REJECTED"
        } else {
            "CONFIRM_HOLD"
        };
        let event = build_single_account_event(
            &next,
            &cmd.account_id,
            out.committed_seq,
            &now,
            event_type,
            Vec::new(),
            self.engines.bond_engine,
        )?;

        Ok((
            next,
            OrchestratedConfirmHoldOutput {
                committed_seq: out.committed_seq,
                earmark: out.earmark,
                position,
                risk_summary,
                funding_summary: out.funding_summary,
                reconfirmation_error,
            },
            event,
        ))
    }

    /// Manually releases a `HOLD` earmark.
    pub fn release_hold(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &ReleaseHoldCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedReleaseHoldOutput,
        BlotterEventEnvelope,
    )> {
        let now = self.clock.now();
        let (next_domain, out) =
            apply_release_hold(&workspace.domain, cmd).map_err(|err| map_blotter_error(&err))?;
        let mut next = workspace.clone();
        next.domain = next_domain;

        let position = evaluate_single_instrument_position(
            &next,
            &cmd.account_id,
            &out.earmark.instrument_code,
            &now,
            self.engines.bond_engine,
        )?;
        let event = build_single_account_event(
            &next,
            &cmd.account_id,
            out.committed_seq,
            &now,
            "RELEASE_HOLD",
            Vec::new(),
            self.engines.bond_engine,
        )?;

        Ok((
            next,
            OrchestratedReleaseHoldOutput {
                committed_seq: out.committed_seq,
                earmark: out.earmark,
                position,
            },
            event,
        ))
    }

    /// Directly creates a `COMMITTED` earmark with trade settlement-date exact dirty-price cash.
    pub fn create_committed(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &CreateCommittedCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedCreateCommittedOutput,
        BlotterEventEnvelope,
    )> {
        let now = self.clock.now();
        let mut enriched_cmd = cmd.clone();
        if enriched_cmd.exact_dirty_price_input.is_none()
            && let Some(acct) = workspace.domain.accounts.get(&cmd.account_id)
            && let Some(meta) = workspace.accounts_meta.get(&cmd.account_id)
        {
            let settlement_date = acct
                .opening_base
                .settlement_date_for_bucket(cmd.settlement_bucket);
            enriched_cmd.exact_dirty_price_input = compute_trade_exact_dirty_price_input(
                meta,
                &cmd.instrument_code,
                settlement_date,
                Some(&cmd.quote_terms),
                self.engines.bond_engine,
            );
        }

        let (next_domain, out) = apply_create_committed(&workspace.domain, &enriched_cmd, &now)
            .map_err(|err| map_blotter_error(&err))?;
        let mut next = workspace.clone();
        next.domain = next_domain;

        let position = evaluate_single_instrument_position(
            &next,
            &cmd.account_id,
            &cmd.instrument_code,
            &now,
            self.engines.bond_engine,
        )?;
        let (_positions, risk_summary) = evaluate_account_positions_and_risk(
            &next,
            &cmd.account_id,
            &now,
            self.engines.bond_engine,
        )?;
        let event = build_single_account_event(
            &next,
            &cmd.account_id,
            out.committed_seq,
            &now,
            "CREATE_COMMITTED",
            Vec::new(),
            self.engines.bond_engine,
        )?;

        Ok((
            next,
            OrchestratedCreateCommittedOutput {
                committed_seq: out.committed_seq,
                earmark: out.earmark,
                position,
                risk_summary,
                funding_summary: out.funding_summary,
            },
            event,
        ))
    }

    /// Manually cancels ONLY the unreconciled portion of a `COMMITTED` earmark.
    pub fn cancel_committed(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &CancelCommittedCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedCancelCommittedOutput,
        BlotterEventEnvelope,
    )> {
        let now = self.clock.now();
        let (next_domain, out) = apply_cancel_committed(&workspace.domain, cmd)
            .map_err(|err| map_blotter_error(&err))?;
        let mut next = workspace.clone();
        next.domain = next_domain;

        let position = evaluate_single_instrument_position(
            &next,
            &cmd.account_id,
            &out.earmark.instrument_code,
            &now,
            self.engines.bond_engine,
        )?;
        let (_positions, risk_summary) = evaluate_account_positions_and_risk(
            &next,
            &cmd.account_id,
            &now,
            self.engines.bond_engine,
        )?;
        let event = build_single_account_event(
            &next,
            &cmd.account_id,
            out.committed_seq,
            &now,
            "CANCEL_COMMITTED",
            Vec::new(),
            self.engines.bond_engine,
        )?;

        Ok((
            next,
            OrchestratedCancelCommittedOutput {
                committed_seq: out.committed_seq,
                earmark: out.earmark,
                position,
                risk_summary,
                funding_summary: out.funding_summary,
            },
            event,
        ))
    }

    /// Validates and submits a draft earmark using the shared deterministic `RulePack` validator.
    pub fn submit_draft_earmark(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &SubmitDraftEarmarkCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        SubmitDraftEarmarkOutput,
        BlotterEventEnvelope,
    )> {
        let now = self.clock.now();
        let (next, out) = apply_submit_draft_earmark_state(
            workspace,
            cmd,
            &now,
            self.engines.bond_engine,
            self.engines.regulatory_rule_parser,
        )?;
        let event = build_single_account_event(
            &next,
            &cmd.account_id,
            out.committed_seq,
            &now,
            "SUBMIT_DRAFT_EARMARK",
            Vec::new(),
            self.engines.bond_engine,
        )?;
        Ok((next, out, event))
    }

    /// Re-validates and confirms a pending draft earmark using the shared deterministic `RulePack`
    /// validator, consuming the draft once and recording the verified trader ID.
    pub fn confirm_draft_earmark(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &ConfirmDraftEarmarkCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        ConfirmDraftEarmarkOutput,
        BlotterEventEnvelope,
    )> {
        let now = self.clock.now();
        let (next, out) = apply_confirm_draft_earmark_state(
            workspace,
            cmd,
            &now,
            self.engines.bond_engine,
            self.engines.regulatory_rule_parser,
        )?;
        let event = build_single_account_event(
            &next,
            &cmd.account_id,
            out.committed_seq,
            &now,
            "CONFIRM_DRAFT_EARMARK",
            Vec::new(),
            self.engines.bond_engine,
        )?;
        Ok((next, out, event))
    }

    /// Ingests a batch or fragment of trade fills on a single source, deterministically pricing
    /// exact settlement-date dirty price when eligible and assembling one atomic cross-account
    /// event envelope.
    pub fn ingest_trade_feed(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &IngestTradeFeedInput,
        id_gen: &mut dyn BlotterIdGenerator,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedIngestTradeFeedOutput,
        Option<BlotterEventEnvelope>,
    )> {
        let now = self.clock.now();
        let mut enriched_cmd = cmd.clone();
        for fill in &mut enriched_cmd.fills {
            if fill.exact_dirty_price_input.is_none()
                && let Some(meta) = workspace.accounts_meta.get(&fill.account_id)
            {
                fill.exact_dirty_price_input = compute_trade_exact_dirty_price_input(
                    meta,
                    &fill.instrument_code,
                    fill.settlement_date,
                    fill.quote_terms.as_ref(),
                    self.engines.bond_engine,
                );
            }
        }

        let (next_domain, out) =
            apply_ingest_trade_feed(&workspace.domain, &enriched_cmd, &now, id_gen)
                .map_err(|err| map_blotter_error(&err))?;
        let mut next = workspace.clone();
        next.domain = next_domain;

        let missing_revisions = pending_missing_trade_revisions(&next, &cmd.source_id);

        let event = if out.mutated {
            let mut allocs_by_acct: BTreeMap<String, Vec<ReconcileAllocationRecord>> =
                BTreeMap::new();
            for alloc in &out.changed_allocations {
                allocs_by_acct
                    .entry(alloc.account_id.clone())
                    .or_default()
                    .push(alloc.clone());
            }
            for allocs in allocs_by_acct.values_mut() {
                allocs.sort_by(|a, b| a.allocation_id.cmp(&b.allocation_id));
            }

            let mut changes = Vec::with_capacity(out.affected_accounts.len());
            for acct_id in &out.affected_accounts {
                let snapshot =
                    build_enriched_snapshot(&next, acct_id, &now, self.engines.bond_engine)?;
                let changed_allocations = allocs_by_acct.remove(acct_id).unwrap_or_default();
                changes.push(AccountChange {
                    account_id: acct_id.clone(),
                    replace_snapshot: snapshot,
                    changed_allocations,
                });
            }
            changes.sort_by(|a, b| a.account_id.cmp(&b.account_id));
            Some(BlotterEventEnvelope {
                seq: out.committed_seq,
                occurred_at: now,
                changes,
                event_type: "INGEST_TRADE_FEED".to_owned(),
            })
        } else {
            None
        };

        Ok((
            next,
            OrchestratedIngestTradeFeedOutput {
                committed_seq: out.committed_seq,
                mutated: out.mutated,
                item_outcomes: out.item_outcomes,
                feed_status: out.feed_status,
                affected_accounts: out.affected_accounts,
                changed_allocations: out.changed_allocations,
                missing_revisions,
            },
            event,
        ))
    }

    /// Synchronously backfills missing predecessor trade revisions for `REVISION_HOLD` trades on
    /// `source_id` via `TradeFeedReconcilerPort::fetch_trade_revision`.
    ///
    /// Missing revisions returning `NotFound` remain frozen in `REVISION_HOLD`; fetched revisions
    /// are fed through `ingest_trade_feed` without rewinding `contiguous_feed_seq`.
    pub fn backfill_missing_trade_revisions(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        source_id: &str,
        actor_id: &str,
        id_gen: &mut dyn BlotterIdGenerator,
        reconciler: &dyn TradeFeedReconcilerPort,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        BackfillTradeRevisionsOutput,
        Option<BlotterEventEnvelope>,
    )> {
        require_non_empty_text(source_id)?;
        require_non_empty_text(actor_id)?;

        let missing = pending_missing_trade_revisions(workspace, source_id);
        if missing.is_empty() {
            return Ok((
                workspace.clone(),
                BackfillTradeRevisionsOutput {
                    fetched_count: 0,
                    not_found_revisions: Vec::new(),
                    ingest_output: None,
                },
                None,
            ));
        }

        let mut fetched: Vec<TradeFillRecord> = Vec::new();
        let mut not_found: Vec<MissingTradeRevisionRequest> = Vec::new();

        for req in missing {
            match reconciler.fetch_trade_revision(
                &req.source_id,
                &req.external_trade_id,
                req.source_revision,
            ) {
                Ok(record) => {
                    if record.source_id != req.source_id
                        || record.external_trade_id != req.external_trade_id
                        || record.source_revision != req.source_revision
                    {
                        return Err(invalid());
                    }
                    fetched.push(record);
                }
                Err(err) if err.category() == ApplicationErrorCategory::NotFound => {
                    not_found.push(req);
                }
                Err(err) => return Err(err),
            }
        }

        if fetched.is_empty() {
            return Ok((
                workspace.clone(),
                BackfillTradeRevisionsOutput {
                    fetched_count: 0,
                    not_found_revisions: not_found,
                    ingest_output: None,
                },
                None,
            ));
        }

        let fetched_count = u32::try_from(fetched.len()).unwrap_or(u32::MAX);
        let (next, ingest_out, event) = self.ingest_trade_feed(
            workspace,
            &IngestTradeFeedInput {
                source_id: source_id.to_owned(),
                fills: fetched,
                heartbeat_at: None,
                actor_id: actor_id.to_owned(),
            },
            id_gen,
        )?;

        Ok((
            next,
            BackfillTradeRevisionsOutput {
                fetched_count,
                not_found_revisions: not_found,
                ingest_output: Some(ingest_out),
            },
            event,
        ))
    }

    /// Manually resolves an open pending match with L2 cross-key in-flight held-trade defense.
    pub fn resolve_pending_match(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &ResolvePendingMatchInput,
        id_gen: &mut dyn BlotterIdGenerator,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedResolvePendingMatchOutput,
        BlotterEventEnvelope,
    )> {
        validate_no_inflight_cross_key_hold(workspace, cmd)?;

        let now = self.clock.now();
        let (next_domain, out) = apply_resolve_pending_match(&workspace.domain, cmd, id_gen)
            .map_err(|err| map_blotter_error(&err))?;
        let mut next = workspace.clone();
        next.domain = next_domain;

        let position = evaluate_single_instrument_position(
            &next,
            &cmd.account_id,
            &out.pending_match.instrument_code,
            &now,
            self.engines.bond_engine,
        )?;
        let (_positions, risk_summary) = evaluate_account_positions_and_risk(
            &next,
            &cmd.account_id,
            &now,
            self.engines.bond_engine,
        )?;
        let mut changed_allocations: Vec<ReconcileAllocationRecord> =
            out.allocation.iter().cloned().collect();
        changed_allocations.sort_by(|a, b| a.allocation_id.cmp(&b.allocation_id));

        let source_id = next
            .domain
            .accounts
            .get(&cmd.account_id)
            .map(|acct| acct.opening_base.source_id.as_str());
        let prev_open_count = source_id
            .and_then(|src| workspace.domain.feed_status(src).ok())
            .map(|view| view.open_pending_match_count);
        let next_open_count = source_id
            .and_then(|src| next.domain.feed_status(src).ok())
            .map(|view| view.open_pending_match_count);
        let open_count_changed = out.pending_match.state != PendingMatchState::Open
            || prev_open_count != next_open_count;

        let mut affected_accounts: BTreeSet<String> = BTreeSet::new();
        affected_accounts.insert(cmd.account_id.clone());
        if open_count_changed && let Some(src) = source_id {
            for (acct_id, acct) in &next.domain.accounts {
                if acct.opening_base.source_id == src {
                    affected_accounts.insert(acct_id.clone());
                }
            }
        }

        let mut changes = Vec::with_capacity(affected_accounts.len());
        for acct_id in affected_accounts {
            let snapshot =
                build_enriched_snapshot(&next, &acct_id, &now, self.engines.bond_engine)?;
            let acct_allocations = if acct_id == cmd.account_id {
                std::mem::take(&mut changed_allocations)
            } else {
                Vec::new()
            };
            changes.push(AccountChange {
                account_id: acct_id,
                replace_snapshot: snapshot,
                changed_allocations: acct_allocations,
            });
        }
        let event = BlotterEventEnvelope {
            seq: out.committed_seq,
            occurred_at: now,
            changes,
            event_type: "RESOLVE_PENDING_MATCH".to_owned(),
        };

        Ok((
            next,
            OrchestratedResolvePendingMatchOutput {
                committed_seq: out.committed_seq,
                pending_match: out.pending_match,
                updated_earmark: out.updated_earmark,
                position,
                risk_summary,
                funding_summary: out.funding_summary,
                allocation: out.allocation,
            },
            event,
        ))
    }

    /// Sweeps expired `HOLD`s, overdue/pre-close `COMMITTED` alerts, and stale trade feeds.
    pub fn sweep_lifecycle(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &SweepLifecycleCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedSweepLifecycleOutput,
        Option<BlotterEventEnvelope>,
    )> {
        let now = self.clock.now();
        let (next_domain, out) = apply_sweep_lifecycle(&workspace.domain, cmd, &now)
            .map_err(|err| map_blotter_error(&err))?;
        let mut next = workspace.clone();
        next.domain = next_domain;

        let event =
            if out.mutated {
                let mut affected: BTreeSet<String> = BTreeSet::new();
                for (acct_id, next_acct) in &next.domain.accounts {
                    if workspace
                        .domain
                        .accounts
                        .get(acct_id)
                        .is_some_and(|prev| prev.account_version != next_acct.account_version)
                    {
                        affected.insert(acct_id.clone());
                    }
                }
                for (src_id, next_feed) in &next.domain.feeds {
                    if workspace.domain.feeds.get(src_id).is_some_and(|prev| {
                        prev.feed_status_revision != next_feed.feed_status_revision
                    }) {
                        for (acct_id, acct) in &next.domain.accounts {
                            if &acct.opening_base.source_id == src_id {
                                affected.insert(acct_id.clone());
                            }
                        }
                    }
                }
                let mut changes = Vec::with_capacity(affected.len());
                for acct_id in affected {
                    let snapshot =
                        build_enriched_snapshot(&next, &acct_id, &now, self.engines.bond_engine)?;
                    changes.push(AccountChange {
                        account_id: acct_id,
                        replace_snapshot: snapshot,
                        changed_allocations: Vec::new(),
                    });
                }
                Some(BlotterEventEnvelope {
                    seq: out.head_seq,
                    occurred_at: now,
                    changes,
                    event_type: "SWEEP_LIFECYCLE".to_owned(),
                })
            } else {
                None
            };

        Ok((
            next,
            OrchestratedSweepLifecycleOutput {
                head_seq: out.head_seq,
                mutated: out.mutated,
                expired_hold_count: out.expired_hold_count,
                newly_alerted_committed_count: out.newly_alerted_committed_count,
                newly_stale_feed_count: out.newly_stale_feed_count,
            },
            event,
        ))
    }

    /// Toggles operator pause or force-clears gap pause on a trade feed source.
    pub fn set_feed_control(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &SetFeedControlCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedSetFeedControlOutput,
        BlotterEventEnvelope,
    )> {
        let now = self.clock.now();
        let (next_domain, out) = apply_set_feed_control(&workspace.domain, cmd)
            .map_err(|err| map_blotter_error(&err))?;
        let mut next = workspace.clone();
        next.domain = next_domain;

        let mut affected: Vec<String> = next
            .domain
            .accounts
            .iter()
            .filter(|(_, acct)| acct.opening_base.source_id == cmd.source_id)
            .map(|(acct_id, _)| acct_id.clone())
            .collect();
        affected.sort();

        let mut changes = Vec::with_capacity(affected.len());
        for acct_id in affected {
            let snapshot =
                build_enriched_snapshot(&next, &acct_id, &now, self.engines.bond_engine)?;
            changes.push(AccountChange {
                account_id: acct_id,
                replace_snapshot: snapshot,
                changed_allocations: Vec::new(),
            });
        }
        let event = BlotterEventEnvelope {
            seq: out.committed_seq,
            occurred_at: now,
            changes,
            event_type: "SET_FEED_CONTROL".to_owned(),
        };

        Ok((
            next,
            OrchestratedSetFeedControlOutput {
                committed_seq: out.committed_seq,
                feed_status: out.feed_status,
            },
            event,
        ))
    }

    /// Ingests per-`(source_id, instrument_code)` market quotes and propagates valuation changes
    /// across all affected accounts in one atomic transaction.
    pub fn ingest_market_quotes(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &IngestMarketQuotesInput,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedIngestMarketQuotesOutput,
        Option<BlotterEventEnvelope>,
    )> {
        let now = self.clock.now();
        let (next, effect) = apply_ingest_market_quotes_state(workspace, cmd, &now)?;
        let (_positions, risk_summary) = evaluate_account_positions_and_risk(
            &next,
            &cmd.account_id,
            &now,
            self.engines.bond_engine,
        )?;

        let event = if effect.mutated {
            let mut changes = Vec::with_capacity(effect.affected_accounts.len());
            for acct_id in &effect.affected_accounts {
                let snapshot =
                    build_enriched_snapshot(&next, acct_id, &now, self.engines.bond_engine)?;
                changes.push(AccountChange {
                    account_id: acct_id.clone(),
                    replace_snapshot: snapshot,
                    changed_allocations: Vec::new(),
                });
            }
            Some(BlotterEventEnvelope {
                seq: effect.committed_seq,
                occurred_at: now,
                changes,
                event_type: "INGEST_MARKET_QUOTES".to_owned(),
            })
        } else {
            None
        };

        Ok((
            next,
            OrchestratedIngestMarketQuotesOutput {
                committed_seq: effect.committed_seq,
                mutated: effect.mutated,
                applied_tick_count: effect.applied_tick_count,
                skipped_stale_tick_count: effect.skipped_stale_tick_count,
                backfilled_tick_count: effect.backfilled_tick_count,
                risk_summary,
                affected_accounts: effect.affected_accounts,
            },
            event,
        ))
    }

    /// Switches an account's or instrument's active valuation quote source as a visible
    /// policy-versioned transaction.
    pub fn switch_active_quote_source(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        cmd: &SwitchActiveQuoteSourceCommand,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedSwitchQuoteSourceOutput,
        Option<BlotterEventEnvelope>,
    )> {
        let now = self.clock.now();
        let (next, effect) = apply_switch_active_quote_source_state(workspace, cmd)?;
        let (_positions, risk_summary) = evaluate_account_positions_and_risk(
            &next,
            &cmd.account_id,
            &now,
            self.engines.bond_engine,
        )?;

        let event = if effect.mutated {
            Some(build_single_account_event(
                &next,
                &cmd.account_id,
                effect.committed_seq,
                &now,
                "SWITCH_ACTIVE_QUOTE_SOURCE",
                Vec::new(),
                self.engines.bond_engine,
            )?)
        } else {
            None
        };

        Ok((
            next,
            OrchestratedSwitchQuoteSourceOutput {
                committed_seq: effect.committed_seq,
                mutated: effect.mutated,
                policy_version: effect.policy_version,
                affected_accounts: effect.affected_accounts,
                risk_summary: Some(risk_summary),
            },
            event,
        ))
    }

    /// Reconciles persisted desk policy configuration (quote sources, staleness, draft hold TTL,
    /// hedge allowlist, regulatory rule pack, and limit ceilings) on restart before serving requests.
    pub fn reconcile_startup_quote_sources(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        configured_policies: &BTreeMap<String, AccountDeskPolicy>,
    ) -> ApplicationResult<(
        DeskBlotterWorkspaceState,
        OrchestratedSwitchQuoteSourceOutput,
        Option<BlotterEventEnvelope>,
    )> {
        let now = self.clock.now();
        let (next, effect) =
            apply_startup_quote_source_reconciliation(workspace, configured_policies)?;

        let event = if effect.mutated {
            let mut changes = Vec::with_capacity(effect.affected_accounts.len());
            for acct_id in &effect.affected_accounts {
                let snapshot =
                    build_enriched_snapshot(&next, acct_id, &now, self.engines.bond_engine)?;
                changes.push(AccountChange {
                    account_id: acct_id.clone(),
                    replace_snapshot: snapshot,
                    changed_allocations: Vec::new(),
                });
            }
            Some(BlotterEventEnvelope {
                seq: effect.committed_seq,
                occurred_at: now,
                changes,
                event_type: "RECONCILE_STARTUP_QUOTE_SOURCES".to_owned(),
            })
        } else {
            None
        };

        Ok((
            next,
            OrchestratedSwitchQuoteSourceOutput {
                committed_seq: effect.committed_seq,
                mutated: effect.mutated,
                policy_version: effect.policy_version,
                affected_accounts: effect.affected_accounts,
                risk_summary: None,
            },
            event,
        ))
    }

    /// Computes a covered futures hedge preview for `WHOLE_ACCOUNT` or `COVERED_SUBSET_ONLY`.
    pub fn preview_covered_hedge(
        &self,
        workspace: &DeskBlotterWorkspaceState,
        input: &PreviewCoveredHedgeInput,
    ) -> ApplicationResult<PreviewCoveredHedgeOutput> {
        let now = self.clock.now();
        execute_preview_covered_hedge(
            workspace,
            input,
            &now,
            self.engines.bond_engine,
            self.engines.hedge_engine,
            self.engines.delivery_rule_parser,
        )
    }
}

fn validate_and_index_catalog(
    opening_base: &FullOpeningPositionBase,
) -> ApplicationResult<BTreeMap<String, InstrumentCatalogEntry>> {
    let mut map = BTreeMap::new();
    for entry in &opening_base.instruments {
        require_non_empty_text(&entry.instrument_code)?;
        if map
            .insert(entry.instrument_code.clone(), entry.clone())
            .is_some()
        {
            return Err(invalid());
        }
    }
    Ok(map)
}

/// Rejects cross-account `baseline_evidence` conflicts on shared feed sources (`[m-3]`).
fn validate_no_cross_account_baseline_evidence_conflict(
    workspace: &DeskBlotterWorkspaceState,
    opening_base: &FullOpeningPositionBase,
) -> ApplicationResult<()> {
    let source_id = &opening_base.core.source_id;
    let target_account_id = &opening_base.core.account_id;

    for ev in &opening_base.core.baseline_evidence {
        for (other_acct_id, other_acct) in &workspace.domain.accounts {
            if other_acct_id == target_account_id || other_acct.opening_base.source_id != *source_id
            {
                continue;
            }
            if other_acct
                .opening_base
                .baseline_evidence
                .iter()
                .any(|other_ev| {
                    other_ev.source_id == ev.source_id
                        && other_ev.external_trade_id == ev.external_trade_id
                })
            {
                return Err(invalid());
            }
        }

        if let Some(feed) = workspace.domain.feeds.get(source_id) {
            for (_, stored) in feed
                .trade_revisions
                .range((ev.external_trade_id.clone(), 0)..=(ev.external_trade_id.clone(), u64::MAX))
            {
                if &stored.record.account_id != target_account_id {
                    return Err(invalid());
                }
            }
            if let Some(chain) = feed.trade_chains.get(&ev.external_trade_id)
                && let Some(orig) = &chain.original_new_fill
                && &orig.account_id != target_account_id
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

/// Prevents premature manual pending-match resolution while a trade has in-flight held revisions
/// across four-keys or while the source feed still holds an incomplete unreleased batch (`[m-2]`).
fn validate_no_inflight_cross_key_hold(
    workspace: &DeskBlotterWorkspaceState,
    cmd: &ResolvePendingMatchInput,
) -> ApplicationResult<()> {
    let Some(account) = workspace.domain.accounts.get(&cmd.account_id) else {
        return Ok(());
    };
    let Some(pm) = account.pending_matches.get(&cmd.pending_match_id) else {
        return Ok(());
    };
    let Some(feed) = workspace.domain.feeds.get(&account.opening_base.source_id) else {
        return Ok(());
    };

    // 1. The pending match's own trade must not have any in-flight held revision on ANY key.
    if feed
        .trade_chains
        .get(&pm.external_trade_id)
        .is_some_and(|chain| !chain.held_revisions.is_empty())
    {
        return Err(ApplicationError::blotter_invalid_state_transition());
    }

    if cmd.action == PendingMatchResolutionAction::ApplyBaselineDelta {
        return Ok(());
    }

    // 2. Any unreleased source batch on this feed blocks manual non-baseline resolution.
    if feed.source_batches.values().any(|batch| !batch.released) {
        return Err(ApplicationError::blotter_invalid_state_transition());
    }

    // 3. Any other trade in this account with in-flight held revisions that touches pm's four-key
    //    OR is a cross-key amendment touching pm's instrument blocks premature manual resolution.
    for (other_trade_id, chain) in &feed.trade_chains {
        if chain.held_revisions.is_empty() {
            continue;
        }
        let mut touches_same_four_key = false;
        let mut touches_same_instrument = false;
        let mut distinct_keys_in_account: BTreeSet<(
            &str,
            ficant_domain::blotter::TradeSide,
            chrono::NaiveDate,
        )> = BTreeSet::new();

        for (_, stored) in feed
            .trade_revisions
            .range((other_trade_id.clone(), 0)..=(other_trade_id.clone(), u64::MAX))
        {
            if stored.record.account_id != pm.account_id {
                continue;
            }
            distinct_keys_in_account.insert((
                stored.record.instrument_code.as_str(),
                stored.record.side,
                stored.record.settlement_date,
            ));
            if stored.record.instrument_code == pm.instrument_code {
                touches_same_instrument = true;
                if stored.record.side == pm.side
                    && stored.record.settlement_date == pm.settlement_date
                {
                    touches_same_four_key = true;
                }
            }
        }

        if touches_same_four_key || (touches_same_instrument && distinct_keys_in_account.len() > 1)
        {
            return Err(ApplicationError::blotter_invalid_state_transition());
        }
    }

    Ok(())
}

fn build_enriched_snapshot(
    workspace: &DeskBlotterWorkspaceState,
    account_id: &str,
    now: &MarketTime,
    bond_engine: &dyn BondAnalyticsEngine,
) -> ApplicationResult<EnrichedAccountBlotterSnapshot> {
    let account = workspace
        .domain
        .accounts
        .get(account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    let meta = workspace
        .accounts_meta
        .get(account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    let domain_snap = workspace
        .domain
        .snapshot_account(account_id)
        .map_err(|err| map_blotter_error(&err))?;
    let (positions, risk_summary) =
        evaluate_account_positions_and_risk(workspace, account_id, now, bond_engine)?;

    let mut active_earmarks: Vec<EarmarkRecord> = account
        .earmarks
        .values()
        .filter(|e| matches!(e.state, EarmarkState::Hold | EarmarkState::Committed))
        .cloned()
        .collect();
    active_earmarks.sort_by(|a, b| a.earmark_id.cmp(&b.earmark_id));

    let mut open_pending_matches: Vec<PendingMatchRecord> = account
        .pending_matches
        .values()
        .filter(|pm| pm.state == PendingMatchState::Open)
        .cloned()
        .collect();
    open_pending_matches.sort_by(|a, b| a.pending_match_id.cmp(&b.pending_match_id));

    let mut pending_drafts = meta
        .drafts
        .values()
        .filter(|d| d.status == DraftEarmarkStatus::PendingConfirmation)
        .cloned()
        .collect::<Vec<_>>();
    pending_drafts.sort_by(|a, b| a.draft_id.cmp(&b.draft_id));

    let opening_base = FullOpeningPositionBase {
        core: account.opening_base.clone(),
        instruments: meta.instruments.values().cloned().collect(),
        calendar: meta.calendar.clone(),
        default_calendar_requirement: meta.default_calendar_requirement,
        units: meta.units.clone(),
    };

    Ok(EnrichedAccountBlotterSnapshot {
        snapshot_seq: workspace.domain.head_seq,
        account_id: account.account_id.clone(),
        account_version: account.account_version,
        opening_base,
        risk_summary,
        funding_summary: domain_snap.funding_summary,
        feed_statuses: vec![domain_snap.feed_status],
        positions,
        active_earmarks,
        open_pending_matches,
        pending_drafts,
        allowed_hedge_product_codes: meta.policy.allowed_hedge_product_codes.clone(),
    })
}

fn build_single_account_event(
    workspace: &DeskBlotterWorkspaceState,
    account_id: &str,
    seq: u64,
    occurred_at: &MarketTime,
    event_type: &str,
    mut changed_allocations: Vec<ReconcileAllocationRecord>,
    bond_engine: &dyn BondAnalyticsEngine,
) -> ApplicationResult<BlotterEventEnvelope> {
    changed_allocations.sort_by(|a, b| a.allocation_id.cmp(&b.allocation_id));
    let snapshot = build_enriched_snapshot(workspace, account_id, occurred_at, bond_engine)?;
    Ok(BlotterEventEnvelope {
        seq,
        occurred_at: occurred_at.clone(),
        changes: vec![AccountChange {
            account_id: account_id.to_owned(),
            replace_snapshot: snapshot,
            changed_allocations,
        }],
        event_type: event_type.to_owned(),
    })
}
