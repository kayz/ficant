use chrono::NaiveDate;
use ficant_domain::analytics::{CalendarRequirement, FixedDecimal};
use ficant_domain::blotter::{
    CreateCommittedCommand, CreateHoldCommand, EarmarkRecord, ExactDirtyPriceCashInput,
    FundingGapSummary, QuoteTerms, RegisterDraftCommand, SettlementBucket, TradeSide,
    apply_create_committed, apply_create_hold, apply_register_draft, compute_funding_summary,
    project_instrument_position, resolve_settlement_cash,
};
use ficant_domain::constraint::ConstraintMeasure;
use ficant_domain::primitives::{ContentHash, MarketTime, Ulid};
use ficant_domain::{ContentAddressed, VersionedDefinition};

use crate::ports::{ApplicationResult, BondAnalyticsEngine, RegulatoryRulePackParser};
use crate::use_cases::blotter::model::{
    AccountMetadataState, CoverageStatus, DeskBlotterWorkspaceState, DraftEarmarkRecord,
    DraftEarmarkStatus, DraftTargetState, NlpEarmarkProposal, RiskAndCoverageSummary, RuleLineage,
    SourceTextAnchor, UncoveredReason, ValuedInstrumentPositionView, append_bytes, append_fixed,
    append_opt_fixed, append_quote_terms, append_str, append_u8, append_u32, append_u64,
    append_unit_ref, invalid, require_non_empty_text,
};
use crate::use_cases::blotter::valuation::{
    InstrumentPricingOutcome, compute_trade_exact_dirty_price_input,
    evaluate_account_positions_and_risk, evaluate_instrument_pricing,
    evaluate_single_instrument_position, position_dv01_from_d100,
};
use crate::use_cases::parse_verified_regulatory_rule;
use crate::{ApplicationError, ApplicationErrorCategory, map_blotter_error, map_domain_error};

/// Normalized proposal terms shared between `SubmitDraftEarmark` and `ConfirmDraftEarmark`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftValidationTerms<'a> {
    pub account_id: &'a str,
    pub instrument_code: &'a str,
    pub side: TradeSide,
    pub settlement_bucket: SettlementBucket,
    pub notional: FixedDecimal,
    pub quote_terms: &'a QuoteTerms,
    pub target_state: DraftTargetState,
    pub raw_source_text: &'a str,
    pub anchors: &'a [SourceTextAnchor],
    pub confidence_score: FixedDecimal,
    pub parser_source_id: &'a str,
}

/// Pure deterministic output of the shared draft validator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedDraftPreview {
    pub rule_lineage: RuleLineage,
    pub preview_coverage_status: CoverageStatus,
    pub preview_delta_pre_tax_dv01: Option<FixedDecimal>,
    pub preview_uncovered_reason: Option<UncoveredReason>,
    pub preview_delta_uncovered_notional: Option<FixedDecimal>,
    pub preview_delta_cash_t0: Option<FixedDecimal>,
    pub preview_delta_cash_t1: Option<FixedDecimal>,
    pub preview_quote_source_id: String,
    pub preview_quote_seq: u64,
    pub valuation_settlement_date: NaiveDate,
    pub context_hash: ContentHash,
    pub exact_dirty_price_input: Option<ExactDirtyPriceCashInput>,
}

/// Command to submit a draft earmark for preview and trader confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitDraftEarmarkCommand {
    pub account_id: String,
    pub draft_id: Ulid,
    pub proposal: NlpEarmarkProposal,
}

/// Output of `SubmitDraftEarmark`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitDraftEarmarkOutput {
    pub committed_seq: u64,
    pub draft: DraftEarmarkRecord,
}

/// Command for a verified trader to confirm a pending draft earmark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmDraftEarmarkCommand {
    pub account_id: String,
    pub draft_id: Ulid,
    pub earmark_id: Ulid,
    pub expected_draft_revision: u64,
    pub expected_position_version: u64,
    pub confirmation_token: ContentHash,
    pub acknowledge_uncovered_dv01: bool,
    pub trader_id: String,
}

/// Output of `ConfirmDraftEarmark`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmDraftEarmarkOutput {
    pub committed_seq: u64,
    pub draft: DraftEarmarkRecord,
    pub earmark: EarmarkRecord,
    pub position: ValuedInstrumentPositionView,
    pub risk_summary: RiskAndCoverageSummary,
    pub funding_summary: FundingGapSummary,
}

/// Validates UTF-8 half-open `SourceTextAnchor` spans against `raw_source_text`.
pub fn validate_source_text_anchors(
    raw_source_text: &str,
    anchors: &[SourceTextAnchor],
) -> ApplicationResult<()> {
    if raw_source_text.trim().is_empty() {
        return Err(invalid());
    }
    let total_len = raw_source_text.len();
    for anchor in anchors {
        require_non_empty_text(&anchor.field_name)?;
        let start = usize::try_from(anchor.byte_start).map_err(|_| invalid())?;
        let end = usize::try_from(anchor.byte_end).map_err(|_| invalid())?;
        if start >= end
            || end > total_len
            || !raw_source_text.is_char_boundary(start)
            || !raw_source_text.is_char_boundary(end)
        {
            return Err(invalid());
        }
        if raw_source_text[start..end] != anchor.span_text {
            return Err(invalid());
        }
    }
    Ok(())
}

/// Computes the deterministic `confirmation_token` binding `draft_id` and `context_hash`.
#[must_use]
pub fn compute_draft_confirmation_token(
    draft_id: &Ulid,
    context_hash: &ContentHash,
) -> ContentHash {
    let mut bytes = Vec::new();
    append_str(&mut bytes, "ficant.blotter.draft.confirm.v1");
    append_str(&mut bytes, draft_id.as_str());
    append_bytes(&mut bytes, context_hash.as_bytes());
    ContentHash::digest(&bytes)
}

/// Shared pure deterministic validator invoked identically by BOTH `SubmitDraftEarmark` and
/// `ConfirmDraftEarmark`.
///
/// Validates:
/// 1. Proposal fields and UTF-8 `SourceTextAnchor` byte spans;
/// 2. Server-selected trusted regulatory `MarketRulePack` via `parse_verified_regulatory_rule`
///    at server time `now`;
/// 3. Binding every parsed `ConstraintDefinition` to account policy ceilings (`InstrumentFace`,
///    `PortfolioFace`, `BorrowedCash`) and evaluating proposed post-trade exposure;
/// 4. Standardized `PRE_TAX` DV01 preview and canonical `context_hash`.
///
/// Missing, tampered, unverified, expired, wrong-type rule packs, missing limit ceilings, or
/// breached constraints ALWAYS return a rule error regardless of `acknowledge_uncovered_dv01`.
#[allow(clippy::too_many_lines)]
pub fn validate_and_preview_draft(
    workspace: &DeskBlotterWorkspaceState,
    terms: &DraftValidationTerms<'_>,
    now: &MarketTime,
    bond_engine: &dyn BondAnalyticsEngine,
    regulatory_rule_parser: &dyn RegulatoryRulePackParser,
) -> ApplicationResult<ValidatedDraftPreview> {
    require_non_empty_text(terms.account_id)?;
    require_non_empty_text(terms.instrument_code)?;
    require_non_empty_text(terms.parser_source_id)?;
    if !terms.notional.is_positive() {
        return Err(invalid());
    }
    terms
        .quote_terms
        .validate()
        .map_err(|err| map_blotter_error(&err))?;
    if !terms.confidence_score.is_non_negative() || terms.confidence_score > FixedDecimal::ONE {
        return Err(invalid());
    }
    validate_source_text_anchors(terms.raw_source_text, terms.anchors)?;

    let account = workspace
        .domain
        .accounts
        .get(terms.account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    let meta = workspace
        .accounts_meta
        .get(terms.account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    let feed = workspace
        .domain
        .feeds
        .get(&account.opening_base.source_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    let pos_state = account
        .positions
        .get(terms.instrument_code)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;

    // Step 1: Parse and verify the server-selected trusted regulatory RulePack.
    let rule_pack = meta
        .policy
        .regulatory_rule_pack
        .as_ref()
        .ok_or_else(ApplicationError::blotter_rule_pack_invalid)?;
    let constraint_set =
        parse_verified_regulatory_rule(rule_pack, Some(now), regulatory_rule_parser)?;

    // Step 2: Bind every parsed constraint definition to account policy ceilings.
    let mut bound_ceilings: Vec<(&str, ConstraintMeasure, FixedDecimal)> =
        Vec::with_capacity(constraint_set.definitions().len());
    for def in constraint_set.definitions() {
        let Some(&ceiling) = meta.policy.limit_ceilings.get(def.limit_code()) else {
            return Err(ApplicationError::blotter_constraint_limit_missing(
                def.limit_code(),
            ));
        };
        if !ceiling.is_non_negative() {
            return Err(ApplicationError::blotter_rule_pack_invalid());
        }
        bound_ceilings.push((def.limit_code(), def.measure(), ceiling));
    }

    let parsed_constraints_hash =
        compute_parsed_constraints_hash(rule_pack.identity(), rule_pack.version(), &bound_ceilings);
    let rule_lineage = RuleLineage {
        pack_id: Ulid::new(rule_pack.identity()).map_err(map_domain_error)?,
        version: rule_pack.version(),
        source: rule_pack.source().to_owned(),
        content_hash: rule_pack.content_hash().clone(),
        parsed_constraints_hash,
    };

    // Step 3: Compute proposed post-trade exposure and settlement cash for constraint evaluation.
    let signed_notional_delta = match terms.side {
        TradeSide::Buy => terms.notional,
        TradeSide::Sell => FixedDecimal::ZERO
            .checked_sub(terms.notional)
            .map_err(map_domain_error)?,
    };

    let target_pos_view = project_instrument_position(account, terms.instrument_code)
        .map_err(|err| map_blotter_error(&err))?;
    let proposed_target_econ = target_pos_view
        .economic_net_notional
        .checked_add(signed_notional_delta)
        .map_err(map_domain_error)?;
    let proposed_target_abs = FixedDecimal::from_scaled(proposed_target_econ.scaled().abs());

    let mut proposed_portfolio_face = FixedDecimal::ZERO;
    for inst_code in account.positions.keys() {
        if inst_code == terms.instrument_code {
            proposed_portfolio_face = proposed_portfolio_face
                .checked_add(proposed_target_abs)
                .map_err(map_domain_error)?;
        } else {
            let other_view = project_instrument_position(account, inst_code)
                .map_err(|err| map_blotter_error(&err))?;
            let abs_other =
                FixedDecimal::from_scaled(other_view.economic_net_notional.scaled().abs());
            proposed_portfolio_face = proposed_portfolio_face
                .checked_add(abs_other)
                .map_err(map_domain_error)?;
        }
    }

    let trade_settlement_date = account
        .opening_base
        .settlement_date_for_bucket(terms.settlement_bucket);
    let exact_dirty_price_input = compute_trade_exact_dirty_price_input(
        meta,
        terms.instrument_code,
        trade_settlement_date,
        Some(terms.quote_terms),
        bond_engine,
    );
    let resolved_trade_cash = resolve_settlement_cash(
        terms.side,
        terms.notional,
        Some(terms.quote_terms),
        exact_dirty_price_input,
    )
    .map_err(|err| map_blotter_error(&err))?;

    let current_funding =
        compute_funding_summary(account, feed).map_err(|err| map_blotter_error(&err))?;

    for (limit_code, measure, ceiling) in &bound_ceilings {
        match measure {
            ConstraintMeasure::InstrumentFace => {
                if proposed_target_abs > *ceiling {
                    return Err(ApplicationError::blotter_constraint_limit_exceeded(
                        *limit_code,
                    ));
                }
            }
            ConstraintMeasure::PortfolioFace => {
                if proposed_portfolio_face > *ceiling {
                    return Err(ApplicationError::blotter_constraint_limit_exceeded(
                        *limit_code,
                    ));
                }
            }
            ConstraintMeasure::BorrowedCash => {
                let Some(trade_cash) = resolved_trade_cash.signed_cash else {
                    // Constraint cannot be evaluated without deterministic settlement cash.
                    return Err(ApplicationError::blotter_rule_pack_invalid());
                };
                let (delta_t0, delta_t1) = match terms.settlement_bucket {
                    SettlementBucket::TPlus0 => (trade_cash, trade_cash),
                    SettlementBucket::TPlus1 => (FixedDecimal::ZERO, trade_cash),
                };
                let proposed_p0 = current_funding
                    .projected_cash_t0
                    .checked_add(delta_t0)
                    .map_err(map_domain_error)?;
                let proposed_p1 = current_funding
                    .projected_cash_t1
                    .checked_add(delta_t1)
                    .map_err(map_domain_error)?;
                let borrowed_t0 = positive_deficit(proposed_p0)?;
                let borrowed_t1 = positive_deficit(proposed_p1)?;
                let peak_borrowed = borrowed_t0.max(borrowed_t1);
                if peak_borrowed > *ceiling {
                    return Err(ApplicationError::blotter_constraint_limit_exceeded(
                        *limit_code,
                    ));
                }
            }
        }
    }

    // Step 4: Evaluate standardized PRE_TAX valuation preview using shared valuation engine.
    let valuation_settlement_date = account.opening_base.trading_date;
    let pricing_outcome = evaluate_instrument_pricing(
        workspace,
        account,
        meta,
        terms.instrument_code,
        now,
        bond_engine,
    );

    let (
        preview_coverage_status,
        preview_delta_pre_tax_dv01,
        preview_uncovered_reason,
        preview_delta_uncovered_notional,
        preview_quote_source_id,
        preview_quote_seq,
        quote_payload_hash,
        quote_is_stale,
    ) = match pricing_outcome {
        InstrumentPricingOutcome::Covered {
            pricing,
            active_quote_source_id,
            active_quote_seq,
            quote_payload_hash,
        } => {
            let delta_dv01 = match terms.target_state {
                DraftTargetState::Hold => FixedDecimal::ZERO,
                DraftTargetState::Committed => {
                    position_dv01_from_d100(signed_notional_delta, pricing.d100)?
                }
            };
            (
                CoverageStatus::CoveredPreTax,
                Some(delta_dv01),
                None,
                None,
                active_quote_source_id,
                active_quote_seq,
                Some(quote_payload_hash),
                false,
            )
        }
        InstrumentPricingOutcome::Uncovered {
            reason,
            active_quote_source_id,
            active_quote_seq,
            quote_payload_hash,
            quote_is_stale,
            ..
        } => {
            let delta_uncovered = match terms.target_state {
                DraftTargetState::Hold => FixedDecimal::ZERO,
                DraftTargetState::Committed => signed_notional_delta,
            };
            (
                CoverageStatus::Uncovered,
                None,
                Some(reason),
                Some(delta_uncovered),
                active_quote_source_id,
                active_quote_seq.unwrap_or(0),
                quote_payload_hash,
                quote_is_stale,
            )
        }
    };

    let (preview_delta_cash_t0, preview_delta_cash_t1) = match terms.target_state {
        DraftTargetState::Hold => (None, None),
        DraftTargetState::Committed => match terms.settlement_bucket {
            SettlementBucket::TPlus0 => (resolved_trade_cash.signed_cash, None),
            SettlementBucket::TPlus1 => (None, resolved_trade_cash.signed_cash),
        },
    };

    let context_hash = compute_draft_context_hash(
        terms,
        meta,
        &rule_lineage,
        valuation_settlement_date,
        &preview_quote_source_id,
        preview_quote_seq,
        quote_payload_hash.as_ref(),
        quote_is_stale,
        preview_coverage_status,
        preview_uncovered_reason,
        preview_delta_pre_tax_dv01,
        preview_delta_uncovered_notional,
        preview_delta_cash_t0,
        preview_delta_cash_t1,
        pos_state.position_version,
        target_pos_view.economic_net_notional,
    );

    Ok(ValidatedDraftPreview {
        rule_lineage,
        preview_coverage_status,
        preview_delta_pre_tax_dv01,
        preview_uncovered_reason,
        preview_delta_uncovered_notional,
        preview_delta_cash_t0,
        preview_delta_cash_t1,
        preview_quote_source_id,
        preview_quote_seq,
        valuation_settlement_date,
        context_hash,
        exact_dirty_price_input,
    })
}

/// Pure transition: validate and persist a pending draft earmark.
pub fn apply_submit_draft_earmark_state(
    workspace: &DeskBlotterWorkspaceState,
    cmd: &SubmitDraftEarmarkCommand,
    now: &MarketTime,
    bond_engine: &dyn BondAnalyticsEngine,
    regulatory_rule_parser: &dyn RegulatoryRulePackParser,
) -> ApplicationResult<(DeskBlotterWorkspaceState, SubmitDraftEarmarkOutput)> {
    require_non_empty_text(&cmd.account_id)?;
    if cmd.proposal.account_id != cmd.account_id {
        return Err(invalid());
    }
    let notional = cmd.proposal.notional.ok_or_else(invalid)?;
    let quote_terms = cmd.proposal.quote_terms.as_ref().ok_or_else(invalid)?;

    let validation_terms = DraftValidationTerms {
        account_id: &cmd.account_id,
        instrument_code: &cmd.proposal.instrument_code,
        side: cmd.proposal.side,
        settlement_bucket: cmd.proposal.settlement_bucket,
        notional,
        quote_terms,
        target_state: cmd.proposal.suggested_target_state,
        raw_source_text: &cmd.proposal.raw_source_text,
        anchors: &cmd.proposal.anchors,
        confidence_score: cmd.proposal.confidence_score,
        parser_source_id: &cmd.proposal.parser_source_id,
    };

    let preview = validate_and_preview_draft(
        workspace,
        &validation_terms,
        now,
        bond_engine,
        regulatory_rule_parser,
    )?;

    let (next_domain, reg_out) = apply_register_draft(
        &workspace.domain,
        &RegisterDraftCommand {
            account_id: cmd.account_id.clone(),
            draft_id: cmd.draft_id.clone(),
            instrument_code: cmd.proposal.instrument_code.clone(),
        },
    )
    .map_err(|err| map_blotter_error(&err))?;

    let confirmation_token = compute_draft_confirmation_token(&cmd.draft_id, &preview.context_hash);

    let draft_record = DraftEarmarkRecord {
        draft_id: cmd.draft_id.clone(),
        draft_revision: 1,
        account_id: cmd.account_id.clone(),
        instrument_code: cmd.proposal.instrument_code.clone(),
        side: cmd.proposal.side,
        settlement_bucket: cmd.proposal.settlement_bucket,
        notional,
        quote_terms: quote_terms.clone(),
        target_state: cmd.proposal.suggested_target_state,
        status: DraftEarmarkStatus::PendingConfirmation,
        raw_source_text: cmd.proposal.raw_source_text.clone(),
        anchors: cmd.proposal.anchors.clone(),
        confidence_score: cmd.proposal.confidence_score,
        parser_source_id: cmd.proposal.parser_source_id.clone(),
        preview_coverage_status: preview.preview_coverage_status,
        preview_delta_pre_tax_dv01: preview.preview_delta_pre_tax_dv01,
        preview_uncovered_reason: preview.preview_uncovered_reason,
        preview_delta_uncovered_notional: preview.preview_delta_uncovered_notional,
        preview_delta_cash_t0: preview.preview_delta_cash_t0,
        preview_delta_cash_t1: preview.preview_delta_cash_t1,
        preview_at_seq: reg_out.committed_seq,
        preview_quote_seq: preview.preview_quote_seq,
        confirmation_token,
        rule_lineage: preview.rule_lineage,
        preview_quote_source_id: preview.preview_quote_source_id,
        context_hash: preview.context_hash,
        confirmed_by_trader_id: None,
        valuation_settlement_date: preview.valuation_settlement_date,
    };

    let mut next = workspace.clone();
    next.domain = next_domain;
    let meta = next
        .accounts_meta
        .get_mut(&cmd.account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    if meta.drafts.contains_key(&cmd.draft_id) {
        return Err(invalid());
    }
    meta.drafts
        .insert(cmd.draft_id.clone(), draft_record.clone());

    Ok((
        next,
        SubmitDraftEarmarkOutput {
            committed_seq: reg_out.committed_seq,
            draft: draft_record,
        },
    ))
}

/// Pure transition: re-validate a pending draft with the EXACT same validator as submission,
/// verify context/token immutability and uncovered-risk acknowledgement, consume the draft once,
/// and create the target `HOLD` or `COMMITTED` earmark in one atomic transaction.
#[allow(clippy::too_many_lines)]
pub fn apply_confirm_draft_earmark_state(
    workspace: &DeskBlotterWorkspaceState,
    cmd: &ConfirmDraftEarmarkCommand,
    now: &MarketTime,
    bond_engine: &dyn BondAnalyticsEngine,
    regulatory_rule_parser: &dyn RegulatoryRulePackParser,
) -> ApplicationResult<(DeskBlotterWorkspaceState, ConfirmDraftEarmarkOutput)> {
    require_non_empty_text(&cmd.account_id)?;
    require_non_empty_text(&cmd.trader_id)?;

    let account = workspace
        .domain
        .accounts
        .get(&cmd.account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    let meta = workspace
        .accounts_meta
        .get(&cmd.account_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    let draft = meta
        .drafts
        .get(&cmd.draft_id)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;

    if draft.status != DraftEarmarkStatus::PendingConfirmation {
        return Err(ApplicationError::blotter_invalid_state_transition());
    }
    if draft.draft_revision != cmd.expected_draft_revision {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::ConcurrencyConflict,
            true,
        ));
    }
    let pos_state = account
        .positions
        .get(&draft.instrument_code)
        .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
    if pos_state.position_version != cmd.expected_position_version {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::ConcurrencyConflict,
            true,
        ));
    }
    if cmd.confirmation_token != draft.confirmation_token {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::ConcurrencyConflict,
            true,
        ));
    }

    // Re-run the EXACT same shared deterministic validator at current server time `now`.
    // Any missing/tampered/unverified/expired/wrong-type rule pack, missing ceiling, or
    // breached limit fails HERE with a rule error (even when acknowledge_uncovered_dv01 == true).
    let validation_terms = DraftValidationTerms {
        account_id: &draft.account_id,
        instrument_code: &draft.instrument_code,
        side: draft.side,
        settlement_bucket: draft.settlement_bucket,
        notional: draft.notional,
        quote_terms: &draft.quote_terms,
        target_state: draft.target_state,
        raw_source_text: &draft.raw_source_text,
        anchors: &draft.anchors,
        confidence_score: draft.confidence_score,
        parser_source_id: &draft.parser_source_id,
    };

    let recomputed = validate_and_preview_draft(
        workspace,
        &validation_terms,
        now,
        bond_engine,
        regulatory_rule_parser,
    )?;

    // Any upstream context change (rule version, ceiling adjustment, policy, quote, source switch,
    // staleness transition, or position change) invalidates the preview -> ConcurrencyConflict.
    if recomputed.context_hash != draft.context_hash {
        return Err(ApplicationError::new(
            ApplicationErrorCategory::ConcurrencyConflict,
            true,
        ));
    }

    // Valid rule pack but uncovered DV01 requires explicit trader acknowledgement.
    if recomputed.preview_coverage_status == CoverageStatus::Uncovered
        && !cmd.acknowledge_uncovered_dv01
    {
        return Err(ApplicationError::blotter_uncovered_risk_acknowledgement_required());
    }

    let note = format!("confirmed_from_draft:{}", draft.draft_id.as_str());
    let (next_domain, committed_seq, earmark, funding_summary) = match draft.target_state {
        DraftTargetState::Hold => {
            let (d, out) = apply_create_hold(
                &workspace.domain,
                &CreateHoldCommand {
                    account_id: draft.account_id.clone(),
                    earmark_id: cmd.earmark_id.clone(),
                    expected_position_version: cmd.expected_position_version,
                    instrument_code: draft.instrument_code.clone(),
                    side: draft.side,
                    settlement_bucket: draft.settlement_bucket,
                    notional: draft.notional,
                    quote_terms: Some(draft.quote_terms.clone()),
                    ttl_seconds: meta.policy.default_draft_hold_ttl_seconds,
                    note,
                    trader_id: cmd.trader_id.clone(),
                },
                now,
            )
            .map_err(|err| map_blotter_error(&err))?;
            let funding = d
                .funding_summary(&cmd.account_id)
                .map_err(|err| map_blotter_error(&err))?;
            (d, out.committed_seq, out.earmark, funding)
        }
        DraftTargetState::Committed => {
            let (d, out) = apply_create_committed(
                &workspace.domain,
                &CreateCommittedCommand {
                    account_id: draft.account_id.clone(),
                    earmark_id: cmd.earmark_id.clone(),
                    expected_position_version: cmd.expected_position_version,
                    instrument_code: draft.instrument_code.clone(),
                    side: draft.side,
                    settlement_bucket: draft.settlement_bucket,
                    notional: draft.notional,
                    quote_terms: draft.quote_terms.clone(),
                    exact_dirty_price_input: recomputed.exact_dirty_price_input,
                    note,
                    trader_id: cmd.trader_id.clone(),
                },
                now,
            )
            .map_err(|err| map_blotter_error(&err))?;
            (d, out.committed_seq, out.earmark, out.funding_summary)
        }
    };

    let mut next = workspace.clone();
    next.domain = next_domain;
    let updated_draft = {
        let next_meta = next
            .accounts_meta
            .get_mut(&cmd.account_id)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
        let d = next_meta
            .drafts
            .get_mut(&cmd.draft_id)
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
        d.status = DraftEarmarkStatus::Confirmed;
        d.draft_revision += 1;
        d.confirmed_by_trader_id = Some(cmd.trader_id.clone());
        d.clone()
    };

    let position = evaluate_single_instrument_position(
        &next,
        &cmd.account_id,
        &updated_draft.instrument_code,
        now,
        bond_engine,
    )?;
    let (_positions, risk_summary) =
        evaluate_account_positions_and_risk(&next, &cmd.account_id, now, bond_engine)?;

    Ok((
        next,
        ConfirmDraftEarmarkOutput {
            committed_seq,
            draft: updated_draft,
            earmark,
            position,
            risk_summary,
            funding_summary,
        },
    ))
}

fn positive_deficit(projected_cash: FixedDecimal) -> ApplicationResult<FixedDecimal> {
    if projected_cash.is_non_negative() {
        Ok(FixedDecimal::ZERO)
    } else {
        FixedDecimal::ZERO
            .checked_sub(projected_cash)
            .map_err(map_domain_error)
    }
}

fn compute_parsed_constraints_hash(
    pack_id: &str,
    pack_version: u64,
    bound: &[(&str, ConstraintMeasure, FixedDecimal)],
) -> ContentHash {
    let mut bytes = Vec::new();
    append_str(&mut bytes, "ficant.blotter.constraints.v1");
    append_str(&mut bytes, pack_id);
    append_u64(&mut bytes, pack_version);
    append_u64(&mut bytes, u64::try_from(bound.len()).unwrap_or(u64::MAX));
    for (limit_code, measure, ceiling) in bound {
        append_str(&mut bytes, limit_code);
        append_str(&mut bytes, measure.code());
        append_fixed(&mut bytes, *ceiling);
    }
    ContentHash::digest(&bytes)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn compute_draft_context_hash(
    terms: &DraftValidationTerms<'_>,
    meta: &AccountMetadataState,
    rule_lineage: &RuleLineage,
    valuation_settlement_date: NaiveDate,
    active_quote_source_id: &str,
    active_quote_seq: u64,
    quote_payload_hash: Option<&ContentHash>,
    quote_is_stale: bool,
    preview_coverage_status: CoverageStatus,
    preview_uncovered_reason: Option<UncoveredReason>,
    preview_delta_pre_tax_dv01: Option<FixedDecimal>,
    preview_delta_uncovered_notional: Option<FixedDecimal>,
    preview_delta_cash_t0: Option<FixedDecimal>,
    preview_delta_cash_t1: Option<FixedDecimal>,
    position_version: u64,
    economic_net_notional: FixedDecimal,
) -> ContentHash {
    let mut bytes = Vec::new();
    append_str(&mut bytes, "ficant.blotter.draft.context.v1");
    append_str(&mut bytes, terms.account_id);
    append_str(&mut bytes, terms.instrument_code);
    append_u8(
        &mut bytes,
        match terms.side {
            TradeSide::Buy => 1,
            TradeSide::Sell => 2,
        },
    );
    append_u8(
        &mut bytes,
        match terms.settlement_bucket {
            SettlementBucket::TPlus0 => 1,
            SettlementBucket::TPlus1 => 2,
        },
    );
    append_fixed(&mut bytes, terms.notional);
    append_quote_terms(&mut bytes, terms.quote_terms);
    append_u8(
        &mut bytes,
        match terms.target_state {
            DraftTargetState::Hold => 1,
            DraftTargetState::Committed => 2,
        },
    );

    // Rule lineage (identity, version, source, pack hash, bound constraints & ceilings hash)
    append_str(&mut bytes, rule_lineage.pack_id.as_str());
    append_u64(&mut bytes, rule_lineage.version);
    append_str(&mut bytes, &rule_lineage.source);
    append_bytes(&mut bytes, rule_lineage.content_hash.as_bytes());
    append_bytes(&mut bytes, rule_lineage.parsed_constraints_hash.as_bytes());

    // Account desk policy & staleness settings
    append_u64(&mut bytes, meta.policy.policy_version);
    append_u32(&mut bytes, meta.policy.quote_staleness_seconds);
    append_u32(&mut bytes, meta.policy.default_draft_hold_ttl_seconds);
    append_u64(
        &mut bytes,
        u64::try_from(meta.policy.allowed_hedge_product_codes.len()).unwrap_or(u64::MAX),
    );
    for code in &meta.policy.allowed_hedge_product_codes {
        append_str(&mut bytes, code);
    }

    // Calendar & valuation settlement date
    append_str(&mut bytes, &valuation_settlement_date.to_string());
    match &meta.calendar {
        Some(cal) => {
            append_u8(&mut bytes, 1);
            append_str(&mut bytes, cal.id());
            append_u64(&mut bytes, cal.version().get());
            append_bytes(&mut bytes, cal.content_hash().as_bytes());
        }
        None => append_u8(&mut bytes, 0),
    }
    append_u8(&mut bytes, cal_req_tag(meta.default_calendar_requirement));
    append_u8(
        &mut bytes,
        cal_req_tag(
            meta.instruments
                .get(terms.instrument_code)
                .and_then(|e| e.calendar_requirement),
        ),
    );

    // Units
    append_unit_ref(&mut bytes, &meta.units.currency_amount);
    append_unit_ref(&mut bytes, &meta.units.price_per_100);
    append_unit_ref(&mut bytes, &meta.units.rate);
    append_unit_ref(&mut bytes, &meta.units.dv01);
    append_unit_ref(&mut bytes, &meta.units.dv01_per_100);
    append_unit_ref(&mut bytes, &meta.units.dimensionless);
    append_unit_ref(&mut bytes, &meta.units.contract_count);

    // Active quote state (source, seq, payload hash, staleness)
    append_str(&mut bytes, active_quote_source_id);
    append_u64(&mut bytes, active_quote_seq);
    match quote_payload_hash {
        Some(hash) => {
            append_u8(&mut bytes, 1);
            append_bytes(&mut bytes, hash.as_bytes());
        }
        None => append_u8(&mut bytes, 0),
    }
    append_u8(&mut bytes, u8::from(quote_is_stale));

    // Preview outputs
    append_u8(
        &mut bytes,
        match preview_coverage_status {
            CoverageStatus::CoveredPreTax => 1,
            CoverageStatus::Uncovered => 2,
            CoverageStatus::InactiveZeroPosition => 3,
        },
    );
    append_u8(
        &mut bytes,
        match preview_uncovered_reason {
            None => 0,
            Some(UncoveredReason::InstrumentSpecMissing) => 1,
            Some(UncoveredReason::UnsupportedCouponStructure) => 2,
            Some(UncoveredReason::EmbeddedOptionOrCallable) => 3,
            Some(UncoveredReason::UnsupportedConvention) => 4,
            Some(UncoveredReason::MissingMarketQuote) => 5,
            Some(UncoveredReason::CalendarCoverageMissing) => 6,
            Some(UncoveredReason::KernelPricingFailed) => 7,
            Some(UncoveredReason::EligibilityUnverified) => 8,
            Some(UncoveredReason::CreditOrNonRatesIssuer) => 9,
            Some(UncoveredReason::StaleQuote) => 10,
        },
    );
    append_opt_fixed(&mut bytes, preview_delta_pre_tax_dv01);
    append_opt_fixed(&mut bytes, preview_delta_uncovered_notional);
    append_opt_fixed(&mut bytes, preview_delta_cash_t0);
    append_opt_fixed(&mut bytes, preview_delta_cash_t1);

    // Ledger position binding
    append_u64(&mut bytes, position_version);
    append_fixed(&mut bytes, economic_net_notional);

    ContentHash::digest(&bytes)
}

const fn cal_req_tag(req: Option<CalendarRequirement>) -> u8 {
    match req {
        None => 0,
        Some(CalendarRequirement::ReferenceReplay) => 1,
        Some(CalendarRequirement::ExactMarket) => 2,
    }
}
