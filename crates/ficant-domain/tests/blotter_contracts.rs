#![allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::similar_names
)]

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use proptest::prelude::*;

use ficant_domain::DomainErrorCode;
use ficant_domain::blotter::{
    AllocationPool, BaseEvidence, BaselineDelta, BlotterConfig, BlotterError, BlotterErrorKind,
    BlotterIdGenerator, BlotterState, CancelCommittedCommand, ConfirmHoldCommand,
    CreateCommittedCommand, CreateHoldCommand, DeterministicIdGenerator, EarmarkState,
    ExactDirtyPriceCashInput, ExtendHoldCommand, FeedHealthState, FundingPrecision,
    ImportOpeningBaseCommand, IngestTradeFeedInput, OpeningInstrumentBalance, OpeningPositionBase,
    PendingMatchReason, PendingMatchResolutionAction, PendingMatchState, QuoteBasis, QuoteTerms,
    RegisterDraftCommand, ReleaseHoldCommand, ResolvePendingMatchInput, SeqRange,
    SetFeedControlCommand, SettlementBucket, SweepLifecycleCommand, TradeFeedRecordAction,
    TradeFillOutcomeKind, TradeFillRecord, TradeSide, U256, apply_cancel_committed,
    apply_confirm_hold, apply_create_committed, apply_create_hold, apply_extend_hold,
    apply_import_opening_base, apply_ingest_trade_feed, apply_register_draft, apply_release_hold,
    apply_resolve_pending_match, apply_set_feed_control, apply_sweep_lifecycle,
    cumulative_cash_magnitude, div_rem_u256_by_u128, mul_u128_wide, resolve_settlement_cash,
    wide_mul_div_u128_round_half_even,
};
use ficant_domain::primitives::{ContentHash, FixedDecimal, MarketTime};

const SCALE: i128 = 1_000_000_000_000;
const TZ: &str = "Asia/Shanghai";

fn d(whole: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(whole * SCALE)
}

fn d_million(millions: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(millions * 1_000_000 * SCALE)
}

fn d_scaled(scaled: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(scaled)
}

fn shanghai_time(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> MarketTime {
    let date = NaiveDate::from_ymd_opt(year, month, day).expect("valid date");
    let naive = date.and_hms_opt(hour, min, sec).expect("valid hms");
    // Asia/Shanghai is UTC+8
    let utc = DateTime::<Utc>::from_naive_utc_and_offset(naive - Duration::hours(8), Utc);
    MarketTime::new(utc, TZ, date).expect("valid market time")
}

fn t_day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 5, 18).expect("valid trading date")
}

fn t_next() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 5, 19).expect("valid next settlement date")
}

fn quote_clean(price_scaled: i128, settlement_amount: Option<FixedDecimal>) -> QuoteTerms {
    QuoteTerms {
        basis: Some(QuoteBasis::CleanPricePer100(d_scaled(price_scaled))),
        settlement_amount,
    }
}

fn quote_yield(ytm_scaled: i128, settlement_amount: Option<FixedDecimal>) -> QuoteTerms {
    QuoteTerms {
        basis: Some(QuoteBasis::YieldToMaturity(d_scaled(ytm_scaled))),
        settlement_amount,
    }
}

fn make_opening_base(
    account_id: &str,
    source_id: &str,
    id_gen: &mut DeterministicIdGenerator,
    balances: Vec<OpeningInstrumentBalance>,
    instrument_codes: Vec<String>,
    opening_cash_t0: FixedDecimal,
    opening_net_cash_delta_t1: FixedDecimal,
    watermark_source_seq: Option<u64>,
    source_start_after_feed_seq: u64,
    baseline_evidence: Vec<BaseEvidence>,
) -> OpeningPositionBase {
    OpeningPositionBase {
        account_id: account_id.to_owned(),
        base_id: id_gen.next_ulid(),
        trading_date: t_day(),
        next_settlement_date: t_next(),
        effective_from: shanghai_time(2026, 5, 18, 9, 0, 0),
        watermark_time: shanghai_time(2026, 5, 18, 9, 0, 0),
        watermark_source_seq,
        source_id: source_id.to_owned(),
        opening_cash_t0,
        opening_net_cash_delta_t1,
        balances,
        instrument_codes,
        baseline_evidence,
        source_start_after_feed_seq,
    }
}

fn init_single_account(
    opening_avail_m: i128,
    opening_econ_m: i128,
    opening_cash_t0: FixedDecimal,
    opening_delta_t1: FixedDecimal,
    start_after_seq: u64,
    id_gen: &mut DeterministicIdGenerator,
) -> BlotterState {
    let state = BlotterState::new(BlotterConfig::default()).expect("valid default config");
    let base = make_opening_base(
        "acct-1",
        "src-1",
        id_gen,
        vec![OpeningInstrumentBalance {
            instrument_code: "240011.IB".to_owned(),
            opening_economic_notional: d_million(opening_econ_m),
            opening_available_notional: d_million(opening_avail_m),
            opening_net_t0_notional: FixedDecimal::ZERO,
            opening_net_t1_notional: FixedDecimal::ZERO,
        }],
        vec!["240011.IB".to_owned()],
        opening_cash_t0,
        opening_delta_t1,
        Some(start_after_seq),
        start_after_seq,
        Vec::new(),
    );
    let (state, out) = apply_import_opening_base(
        &state,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "import-1".to_owned(),
            opening_base: base,
            expected_account_version: 0,
        },
    )
    .expect("initial import succeeds");
    assert!(!out.replayed);
    assert_eq!(out.committed_seq, 1);
    state
}

#[allow(clippy::too_many_arguments)]
fn make_fill(
    source_id: &str,
    trade_id: &str,
    rev: u64,
    feed_seq: u64,
    batch_first: u64,
    batch_last: u64,
    action: TradeFeedRecordAction,
    account_id: &str,
    instrument_code: &str,
    side: TradeSide,
    settlement_date: NaiveDate,
    notional: FixedDecimal,
    quote_terms: Option<QuoteTerms>,
    executed_at: MarketTime,
) -> TradeFillRecord {
    TradeFillRecord {
        source_id: source_id.to_owned(),
        external_trade_id: trade_id.to_owned(),
        source_revision: rev,
        feed_seq,
        action,
        account_id: account_id.to_owned(),
        instrument_code: instrument_code.to_owned(),
        side,
        settlement_date,
        notional,
        quote_terms,
        executed_at,
        batch_first_feed_seq: batch_first,
        batch_last_feed_seq: batch_last,
        exact_dirty_price_input: None,
    }
}

// ============================================================================
// 1. ImportOpeningBase single-import, replay, hash mismatch & 6-state rejection
// ============================================================================

#[test]
fn import_opening_base_single_import_replay_hash_mismatch_and_six_states() {
    let mut id_gen = DeterministicIdGenerator::default();
    let base = make_opening_base(
        "acct-1",
        "src-1",
        &mut id_gen,
        vec![OpeningInstrumentBalance {
            instrument_code: "240011.IB".to_owned(),
            opening_economic_notional: d_million(100),
            opening_available_notional: d_million(100),
            opening_net_t0_notional: FixedDecimal::ZERO,
            opening_net_t1_notional: FixedDecimal::ZERO,
        }],
        vec!["240011.IB".to_owned()],
        d_million(200),
        d_million(10),
        Some(10),
        10,
        Vec::new(),
    );

    let initial = BlotterState::new(BlotterConfig::default()).expect("config valid");

    // Non-zero expected_account_version fails initial import.
    let bad_ver = apply_import_opening_base(
        &initial,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-1".to_owned(),
            opening_base: base.clone(),
            expected_account_version: 1,
        },
    )
    .expect_err("non-zero version must fail initial import");
    assert_eq!(bad_ver.kind(), BlotterErrorKind::ConcurrencyConflict);

    // State 1: only import receipt.
    let (state1, out1) = apply_import_opening_base(
        &initial,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-1".to_owned(),
            opening_base: base.clone(),
            expected_account_version: 0,
        },
    )
    .expect("first import succeeds");
    assert_eq!(out1.committed_seq, 1);
    assert!(!out1.replayed);

    // Same actor + same key + same payload replays original receipt without bumping head_seq.
    let (replayed_state, replay_out) = apply_import_opening_base(
        &state1,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-1".to_owned(),
            opening_base: base.clone(),
            expected_account_version: 0,
        },
    )
    .expect("same key same payload replays");
    assert!(replay_out.replayed);
    assert_eq!(replay_out.committed_seq, 1);
    assert_eq!(replayed_state.head_seq, 1);

    // Same actor + same key + different payload => HashMismatch -> ContentHashMismatch.
    let mut mutated_base = base.clone();
    mutated_base.opening_cash_t0 = d_million(999);
    let mismatch = apply_import_opening_base(
        &state1,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-1".to_owned(),
            opening_base: mutated_base,
            expected_account_version: 0,
        },
    )
    .expect_err("same key different payload must fail HashMismatch");
    assert_eq!(mismatch.kind(), BlotterErrorKind::HashMismatch);
    assert_eq!(
        mismatch.domain_error_code(),
        DomainErrorCode::ContentHashMismatch
    );

    let assert_new_key_rejected = |st: &BlotterState, label: &str| {
        let err = apply_import_opening_base(
            st,
            &ImportOpeningBaseCommand {
                actor_id: "trader-1".to_owned(),
                idempotency_key: format!("imp-new-{label}"),
                opening_base: base.clone(),
                expected_account_version: 0,
            },
        )
        .expect_err("re-import with new key must fail");
        assert_eq!(
            err.kind(),
            BlotterErrorKind::ImmutableViolation,
            "state {label} must return ImmutableViolation"
        );
        assert_eq!(
            err.domain_error_code(),
            DomainErrorCode::InvalidStateTransition,
            "state {label} must map to InvalidStateTransition"
        );
    };

    // State 1: only import receipt.
    assert_new_key_rejected(&state1, "only-import-receipt");

    // State 2: only draft present.
    let (state2_draft, _) = apply_register_draft(
        &state1,
        &RegisterDraftCommand {
            account_id: "acct-1".to_owned(),
            draft_id: id_gen.next_ulid(),
            instrument_code: "240011.IB".to_owned(),
        },
    )
    .expect("register draft");
    assert_new_key_rejected(&state2_draft, "only-draft");

    // State 3: active HOLD.
    let now = shanghai_time(2026, 5, 18, 9, 30, 0);
    let hold_id = id_gen.next_ulid();
    let (state3_hold, _) = apply_create_hold(
        &state1,
        &CreateHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: hold_id.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(10),
            quote_terms: None,
            ttl_seconds: 1800,
            note: "hold".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create hold");
    assert_new_key_rejected(&state3_hold, "active-hold");

    // State 4: active COMMITTED.
    let (state4_committed, _) = apply_confirm_hold(
        &state3_hold,
        &ConfirmHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: hold_id.clone(),
            expected_earmark_revision: 1,
            expected_position_version: 2,
            quote_terms: quote_clean(100 * SCALE, Some(d_million(10))),
            exact_dirty_price_input: None,
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("confirm hold");
    assert_new_key_rejected(&state4_committed, "active-committed");

    // State 5: active OPEN pending match.
    let orphan_fill = make_fill(
        "src-1",
        "TRD-ORPHAN-1",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Buy,
        t_day(),
        d_million(5),
        Some(quote_clean(100 * SCALE, Some(d_million(5)))),
        now.clone(),
    );
    let (state5_open, _) = apply_ingest_trade_feed(
        &state1,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![orphan_fill],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest orphan");
    assert_new_key_rejected(&state5_open, "active-open");

    // State 6: terminal object (released HOLD) + late revision (cancel reversal).
    let (state6_terminal, _) = apply_release_hold(
        &state3_hold,
        &ReleaseHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: hold_id,
            expected_earmark_revision: 1,
            reason: "done".to_owned(),
            actor_id: "trader-1".to_owned(),
        },
    )
    .expect("release hold");
    assert_new_key_rejected(&state6_terminal, "terminal-released");
}

// ============================================================================
// 2. Anti-oversell 100/60/40, negative available official risk, zero-base BUY,
//    T+1 deficit rejecting in-term SELL T+0, TTL & lazy expiration, sweeps
// ============================================================================

#[test]
fn anti_oversell_100_60_40_and_official_negative_available_recovery() {
    let mut id_gen = DeterministicIdGenerator::default();
    let state = init_single_account(100, 100, d_million(50), FixedDecimal::ZERO, 10, &mut id_gen);
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);

    // 1. Create COMMITTED SELL T+0 for 60M.
    let e_sell_60 = id_gen.next_ulid();
    let (state, c_out) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e_sell_60.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(60),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(60))),
            exact_dirty_price_input: None,
            note: "sell 60".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create committed 60M");
    assert_eq!(c_out.position.available_t0_notional, d_million(40));
    assert!(!c_out.position.bucket_t0.sell_ambiguity_locked);

    // 2. Official SELL T+0 fill of 40M arrives -> NotionalMismatchOrSplit -> OPEN.
    let fill_40 = make_fill(
        "src-1",
        "TRD-40",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );
    let (state, ing_out) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![fill_40],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest 40M fill");
    assert_eq!(
        ing_out.item_outcomes[0].kind,
        TradeFillOutcomeKind::PendingMatch
    );
    let pm_40_id = ing_out.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm id");

    // Assert 100/60/40: S0=100, A0=0, sell_ambiguity_locked=true.
    let pos_100_60_40 = state.position_view("acct-1", "240011.IB").expect("pos");
    assert_eq!(pos_100_60_40.available_t0_notional, FixedDecimal::ZERO);
    assert_eq!(pos_100_60_40.available_t1_notional, FixedDecimal::ZERO);
    assert!(pos_100_60_40.bucket_t0.sell_ambiguity_locked);

    // New SELL blocked by sell ambiguity lock.
    let blocked = apply_create_hold(
        &state,
        &CreateHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: id_gen.next_ulid(),
            expected_position_version: pos_100_60_40.position_version,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(1),
            quote_terms: None,
            ttl_seconds: 1800,
            note: "blocked".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect_err("sell must be blocked under ambiguity lock");
    assert_eq!(blocked.kind(), BlotterErrorKind::SellAmbiguityLocked);

    // Official new risk pushing available negative must succeed without panic and raise alert.
    let fill_neg_20 = make_fill(
        "src-1",
        "TRD-EXTRA-20",
        1,
        12,
        12,
        12,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(20),
        Some(quote_clean(100 * SCALE, Some(d_million(20)))),
        now.clone(),
    );
    let (state_neg, _) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![fill_neg_20],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("official negative available must record without panic");
    let pos_neg = state_neg.position_view("acct-1", "240011.IB").expect("pos");
    assert_eq!(pos_neg.available_t0_notional, d_million(-20));
    assert!(pos_neg.bucket_t0.negative_available_alert);

    // 3. On the 100/60/40 state, manually SPLIT_OFFSET(40M) against the 60M earmark.
    let (state, split_out) = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_40_id,
            expected_pending_revision: 1,
            expected_position_version: pos_100_60_40.position_version,
            action: PendingMatchResolutionAction::SplitOffset,
            expected_earmark_revision: Some(1),
            target_earmark_id: Some(e_sell_60.clone()),
            offset_notional: Some(d_million(40)),
            reason: "partial fill belongs to 60M commitment".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("split offset 40M succeeds");
    assert_eq!(
        split_out.pending_match.state,
        PendingMatchState::ResolvedMatched
    );
    assert_eq!(
        split_out.position.bucket_t0.reconciled_sell_notional,
        d_million(40)
    );
    assert_eq!(
        split_out
            .position
            .bucket_t0
            .committed_sell_unreconciled_notional,
        d_million(20)
    );
    assert_eq!(
        split_out
            .position
            .bucket_t0
            .pending_match_sell_unresolved_notional,
        FixedDecimal::ZERO
    );
    assert_eq!(split_out.position.available_t0_notional, d_million(40));
    assert!(!split_out.position.bucket_t0.sell_ambiguity_locked);

    // 4. Next 20M fill automatically 1:1 matches the remaining 20M of e_sell_60.
    let fill_20 = make_fill(
        "src-1",
        "TRD-20",
        1,
        12,
        12,
        12,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(20),
        Some(quote_clean(100 * SCALE, Some(d_million(20)))),
        now.clone(),
    );
    let (state, match20_out) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![fill_20],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest 20M fill");
    assert_eq!(
        match20_out.item_outcomes[0].kind,
        TradeFillOutcomeKind::AutoMatched
    );
    let pos_final = state.position_view("acct-1", "240011.IB").expect("pos");
    assert_eq!(pos_final.bucket_t0.reconciled_sell_notional, d_million(60));
    assert_eq!(
        pos_final.bucket_t0.committed_sell_unreconciled_notional,
        FixedDecimal::ZERO
    );
    assert_eq!(pos_final.available_t0_notional, d_million(40));
    assert_eq!(pos_final.economic_net_notional, d_million(40));
    assert_eq!(
        state.accounts["acct-1"].earmarks[&e_sell_60].state,
        EarmarkState::Reconciled
    );
}

#[test]
fn zero_base_buy_holds_unreconciled_buy_not_in_a1_and_t1_deficit_blocks_in_term_sell_t0() {
    let mut id_gen = DeterministicIdGenerator::default();
    let now = shanghai_time(2026, 5, 18, 9, 30, 0);

    // Zero opening base: BUY T+0 and BUY T+1 CreateHold + ConfirmHold both succeed.
    let state = init_single_account(0, 0, d_million(500), FixedDecimal::ZERO, 10, &mut id_gen);
    let buy0 = id_gen.next_ulid();
    let (state, h0) = apply_create_hold(
        &state,
        &CreateHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: buy0.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Buy,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(30),
            quote_terms: None,
            ttl_seconds: 1800,
            note: "buy t0".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("zero-base BUY T+0 hold succeeds");
    assert_eq!(h0.position.available_t1_notional, FixedDecimal::ZERO);

    let (state, c0) = apply_confirm_hold(
        &state,
        &ConfirmHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: buy0,
            expected_earmark_revision: 1,
            expected_position_version: h0.position.position_version,
            quote_terms: quote_clean(100 * SCALE, Some(d_million(30))),
            exact_dirty_price_input: None,
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("zero-base BUY T+0 confirm succeeds");
    // Unreconciled BUY T+0 MUST NOT increase A1!
    assert_eq!(c0.position.available_t1_notional, FixedDecimal::ZERO);
    assert_eq!(c0.position.economic_net_notional, d_million(30));

    // BUY T+1 hold & confirm on zero base also succeed.
    let buy1 = id_gen.next_ulid();
    let (state, h1) = apply_create_hold(
        &state,
        &CreateHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: buy1.clone(),
            expected_position_version: c0.position.position_version,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Buy,
            settlement_bucket: SettlementBucket::TPlus1,
            notional: d_million(20),
            quote_terms: None,
            ttl_seconds: 2700,
            note: "buy t1".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("zero-base BUY T+1 hold succeeds");
    let (state, c1) = apply_confirm_hold(
        &state,
        &ConfirmHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: buy1,
            expected_earmark_revision: 1,
            expected_position_version: h1.position.position_version,
            quote_terms: quote_clean(100 * SCALE, Some(d_million(20))),
            exact_dirty_price_input: None,
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("zero-base BUY T+1 confirm succeeds");
    assert_eq!(c1.position.available_t1_notional, FixedDecimal::ZERO);

    // Once BUY T+0 is reconciled by an official fill, A1 increases by ReconciledBuy0 (30M).
    let fill_buy0 = make_fill(
        "src-1",
        "TRD-BUY0",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Buy,
        t_day(),
        d_million(30),
        Some(quote_clean(100 * SCALE, Some(d_million(30)))),
        now.clone(),
    );
    let (state, _) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![fill_buy0],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("reconcile buy t0");
    let pos_after_rec_buy0 = state.position_view("acct-1", "240011.IB").expect("pos");
    assert_eq!(pos_after_rec_buy0.available_t0_notional, FixedDecimal::ZERO);
    assert_eq!(pos_after_rec_buy0.available_t1_notional, d_million(30));

    // In-term SELL T+0 ConfirmHold rejected when T+1 is in deficit (min(A0, A1) < 0),
    // preserving original HOLD intact!
    let state2 = init_single_account(100, 100, d_million(50), FixedDecimal::ZERO, 10, &mut id_gen);
    let sell_hold = id_gen.next_ulid();
    let (state2, sh_out) = apply_create_hold(
        &state2,
        &CreateHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: sell_hold.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(50),
            quote_terms: None,
            ttl_seconds: 1800,
            note: "sell t0 hold".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create sell t0 hold");

    // Official SELL T+1 of 60M arrives and is accepted as orphan -> A0 = 50M, A1 = 100 - 50 - 60 = -10M.
    let fill_t1 = make_fill(
        "src-1",
        "TRD-T1-60",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_next(),
        d_million(60),
        Some(quote_clean(100 * SCALE, Some(d_million(60)))),
        now.clone(),
    );
    let (state2, ing_t1) = apply_ingest_trade_feed(
        &state2,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![fill_t1],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest t1 fill");
    let pm_t1_id = ing_t1.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm id");
    let pv = state2
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (state2, acc_out) = apply_resolve_pending_match(
        &state2,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_t1_id,
            expected_pending_revision: 1,
            expected_position_version: pv,
            action: PendingMatchResolutionAction::AcceptOrphan,
            expected_earmark_revision: None,
            target_earmark_id: None,
            offset_notional: None,
            reason: "accept t1 risk".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("accept t1 orphan");
    assert_eq!(acc_out.position.available_t0_notional, d_million(50));
    assert_eq!(acc_out.position.available_t1_notional, d_million(-10));

    // In-term ConfirmHold of SELL T+0 fails and preserves HOLD untouched!
    let in_term_time = shanghai_time(2026, 5, 18, 9, 45, 0);
    let err = apply_confirm_hold(
        &state2,
        &ConfirmHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: sell_hold.clone(),
            expected_earmark_revision: sh_out.earmark.earmark_revision,
            expected_position_version: acc_out.position.position_version,
            quote_terms: quote_clean(100 * SCALE, Some(d_million(50))),
            exact_dirty_price_input: None,
            trader_id: "trader-1".to_owned(),
        },
        &in_term_time,
    )
    .expect_err("in-term SELL T+0 confirm must fail under T+1 deficit");
    assert_eq!(err.kind(), BlotterErrorKind::InsufficientAvailable);
    // Original HOLD completely intact.
    assert_eq!(
        state2.accounts["acct-1"].earmarks[&sell_hold].state,
        EarmarkState::Hold
    );
    assert_eq!(
        state2.accounts["acct-1"].earmarks[&sell_hold].earmark_revision,
        1
    );
}

#[test]
fn ttl_boundaries_lazy_expiration_reconfirmation_sweeps_and_cancel_committed() {
    let mut id_gen = DeterministicIdGenerator::default();
    let t0 = shanghai_time(2026, 5, 18, 9, 30, 0);
    let state = init_single_account(
        100,
        100,
        d_million(200),
        FixedDecimal::ZERO,
        10,
        &mut id_gen,
    );

    // TTL bounds 1799 / 2701 rejected.
    for bad_ttl in [1799_u32, 2701_u32] {
        let err = apply_create_hold(
            &state,
            &CreateHoldCommand {
                account_id: "acct-1".to_owned(),
                earmark_id: id_gen.next_ulid(),
                expected_position_version: 1,
                instrument_code: "240011.IB".to_owned(),
                side: TradeSide::Sell,
                settlement_bucket: SettlementBucket::TPlus0,
                notional: d_million(50),
                quote_terms: None,
                ttl_seconds: bad_ttl,
                note: "bad ttl".to_owned(),
                trader_id: "trader-1".to_owned(),
            },
            &t0,
        )
        .expect_err("out-of-range ttl must fail");
        assert_eq!(err.kind(), BlotterErrorKind::ValidationFailed);
    }

    // Create HOLD with TTL = 1800s -> expires_at = 10:00:00.
    let h_id = id_gen.next_ulid();
    let (state, h_out) = apply_create_hold(
        &state,
        &CreateHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: h_id.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(60),
            quote_terms: None,
            ttl_seconds: 1800,
            note: "hold 60".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &t0,
    )
    .expect("create hold 60M");
    let exact_expiry = shanghai_time(2026, 5, 18, 10, 0, 0);
    assert_eq!(h_out.earmark.expires_at, Some(exact_expiry.clone()));

    // ExtendHold at now == expires_at rejected.
    let ext_expired = apply_extend_hold(
        &state,
        &ExtendHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: h_id.clone(),
            expected_earmark_revision: 1,
            ttl_seconds: 1800,
        },
        &exact_expiry,
    )
    .expect_err("cannot extend at now == expires_at");
    assert_eq!(ext_expired.kind(), BlotterErrorKind::InvalidStateTransition);

    // ExtendHold in-term resets expires_at from now.
    let t_mid = shanghai_time(2026, 5, 18, 9, 45, 0);
    let (state_ext, ext_out) = apply_extend_hold(
        &state,
        &ExtendHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: h_id.clone(),
            expected_earmark_revision: 1,
            ttl_seconds: 2700,
        },
        &t_mid,
    )
    .expect("in-term extend succeeds");
    assert_eq!(
        ext_out.earmark.expires_at,
        Some(shanghai_time(2026, 5, 18, 10, 30, 0))
    );
    assert_eq!(ext_out.earmark.earmark_revision, 2);

    // Expired ConfirmHold: operator pauses feed, then ConfirmHold at now == expires_at
    // releases old HOLD into EXPIRED, bumps revision & committed_seq, and returns reconfirmation_error!
    let (state_paused, _) = apply_set_feed_control(
        &state,
        &SetFeedControlCommand {
            source_id: "src-1".to_owned(),
            expected_feed_status_revision: 1,
            paused_by_operator: true,
            force_clear_gap_pause: false,
            reason: "maintenance".to_owned(),
        },
    )
    .expect("pause feed");
    let seq_before = state_paused.head_seq;
    let (state_lazy_exp, lazy_out) = apply_confirm_hold(
        &state_paused,
        &ConfirmHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: h_id.clone(),
            expected_earmark_revision: 1,
            expected_position_version: 2,
            quote_terms: quote_clean(100 * SCALE, Some(d_million(60))),
            exact_dirty_price_input: None,
            trader_id: "trader-1".to_owned(),
        },
        &exact_expiry,
    )
    .expect("expired ConfirmHold returns committed EXPIRED result with reconfirmation_error");
    assert_eq!(lazy_out.earmark.state, EarmarkState::Expired);
    assert_eq!(lazy_out.earmark.earmark_revision, 2);
    assert_eq!(lazy_out.committed_seq, seq_before + 1);
    assert_eq!(lazy_out.position.available_t0_notional, d_million(100));
    assert_eq!(
        lazy_out
            .reconfirmation_error
            .as_ref()
            .map(BlotterError::kind),
        Some(BlotterErrorKind::FeedPaused)
    );
    assert_eq!(state_lazy_exp.head_seq, seq_before + 1);

    // Expired ConfirmHold when capacity & feed OK -> single commit straight into COMMITTED!
    let (state_reconfirmed, reconf_out) = apply_confirm_hold(
        &state,
        &ConfirmHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: h_id.clone(),
            expected_earmark_revision: 1,
            expected_position_version: 2,
            quote_terms: quote_clean(100 * SCALE, Some(d_million(60))),
            exact_dirty_price_input: None,
            trader_id: "trader-1".to_owned(),
        },
        &exact_expiry,
    )
    .expect("expired ConfirmHold succeeds when re-validation passes");
    assert_eq!(reconf_out.earmark.state, EarmarkState::Committed);
    assert!(reconf_out.reconfirmation_error.is_none());
    assert_eq!(reconf_out.position.available_t0_notional, d_million(40));

    // Swept EXPIRED earmark: stale revision -> ConcurrencyConflict; latest revision -> re-validates into COMMITTED!
    let (state_swept, sweep_out) = apply_sweep_lifecycle(
        &state_ext,
        &SweepLifecycleCommand::default(),
        &shanghai_time(2026, 5, 18, 10, 30, 0),
    )
    .expect("sweep expires extended hold");
    assert_eq!(sweep_out.expired_hold_count, 1);
    assert!(sweep_out.mutated);

    // Repeated sweep with no transitions does NOT advance head_seq.
    let head_after_first_sweep = state_swept.head_seq;
    let (state_swept_again, sweep_noop) = apply_sweep_lifecycle(
        &state_swept,
        &SweepLifecycleCommand::default(),
        &shanghai_time(2026, 5, 18, 10, 31, 0),
    )
    .expect("noop sweep succeeds");
    assert!(!sweep_noop.mutated);
    assert_eq!(sweep_noop.head_seq, head_after_first_sweep);
    assert_eq!(state_swept_again.head_seq, head_after_first_sweep);

    // Old revision (2) conflicts; new revision (3) re-validates into COMMITTED.
    let pv_swept = state_swept
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let stale_err = apply_confirm_hold(
        &state_swept,
        &ConfirmHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: h_id.clone(),
            expected_earmark_revision: 2,
            expected_position_version: pv_swept,
            quote_terms: quote_clean(100 * SCALE, Some(d_million(60))),
            exact_dirty_price_input: None,
            trader_id: "trader-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 35, 0),
    )
    .expect_err("stale revision on swept EXPIRED must fail");
    assert_eq!(stale_err.kind(), BlotterErrorKind::ConcurrencyConflict);

    let (_state_resurrected, res_out) = apply_confirm_hold(
        &state_swept,
        &ConfirmHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: h_id.clone(),
            expected_earmark_revision: 3,
            expected_position_version: pv_swept,
            quote_terms: quote_clean(100 * SCALE, Some(d_million(60))),
            exact_dirty_price_input: None,
            trader_id: "trader-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 35, 0),
    )
    .expect("latest revision on swept EXPIRED re-applies cleanly");
    assert_eq!(res_out.earmark.state, EarmarkState::Committed);

    // COMMITTED never auto-releases across +60m, +90m, +8h, 16:30 — only alerts!
    let (state_60m, sw_60m) = apply_sweep_lifecycle(
        &state_reconfirmed,
        &SweepLifecycleCommand::default(),
        &shanghai_time(2026, 5, 18, 11, 0, 0),
    )
    .expect("sweep +60m");
    assert_eq!(sw_60m.newly_alerted_committed_count, 1);
    assert_eq!(
        state_60m.accounts["acct-1"].earmarks[&h_id].state,
        EarmarkState::Committed
    );
    assert!(state_60m.accounts["acct-1"].earmarks[&h_id].overdue_alert);
    assert!(!state_60m.accounts["acct-1"].earmarks[&h_id].pre_close_alert);

    // +90m: no new alert -> head_seq unchanged.
    let (state_90m, sw_90m) = apply_sweep_lifecycle(
        &state_60m,
        &SweepLifecycleCommand::default(),
        &shanghai_time(2026, 5, 18, 11, 30, 0),
    )
    .expect("sweep +90m");
    assert!(!sw_90m.mutated);
    assert_eq!(state_90m.head_seq, state_60m.head_seq);

    // 16:30 and +8h (18:00): raises pre_close_alert, NEVER releases COMMITTED.
    let (state_close, sw_close) = apply_sweep_lifecycle(
        &state_90m,
        &SweepLifecycleCommand::default(),
        &shanghai_time(2026, 5, 18, 16, 30, 0),
    )
    .expect("sweep 16:30");
    assert_eq!(sw_close.newly_alerted_committed_count, 1);
    assert!(state_close.accounts["acct-1"].earmarks[&h_id].pre_close_alert);
    assert_eq!(
        state_close.accounts["acct-1"].earmarks[&h_id].state,
        EarmarkState::Committed
    );

    let (state_8h, sw_8h) = apply_sweep_lifecycle(
        &state_close,
        &SweepLifecycleCommand::default(),
        &shanghai_time(2026, 5, 18, 18, 0, 0),
    )
    .expect("sweep +8h");
    assert!(!sw_8h.mutated);
    assert_eq!(
        state_8h.accounts["acct-1"].earmarks[&h_id].state,
        EarmarkState::Committed
    );

    // Partial offset 25M then CancelCommitted: empty reason rejected; valid reason cancels ONLY remaining 35M!
    let fill_25 = make_fill(
        "src-1",
        "TRD-25",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(25),
        Some(quote_clean(100 * SCALE, Some(d_million(25)))),
        t0,
    );
    let (state_c, ing25) = apply_ingest_trade_feed(
        &state_8h,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![fill_25],
            heartbeat_at: None,
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 18, 1, 0),
        &mut id_gen,
    )
    .expect("ingest 25M");
    let pm25 = ing25.item_outcomes[0].pending_match_id.clone().expect("pm");
    let erev = state_c.accounts["acct-1"].earmarks[&h_id].earmark_revision;
    let pv = state_c
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (state_c, _) = apply_resolve_pending_match(
        &state_c,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm25,
            expected_pending_revision: 1,
            expected_position_version: pv,
            action: PendingMatchResolutionAction::SplitOffset,
            expected_earmark_revision: Some(erev),
            target_earmark_id: Some(h_id.clone()),
            offset_notional: Some(d_million(25)),
            reason: "offset 25".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("split offset 25M");

    let empty_reason_err = apply_cancel_committed(
        &state_c,
        &CancelCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: h_id.clone(),
            expected_earmark_revision: erev + 1,
            reason: "   ".to_owned(),
            actor_id: "trader-1".to_owned(),
        },
    )
    .expect_err("empty cancel reason must fail");
    assert_eq!(empty_reason_err.kind(), BlotterErrorKind::ValidationFailed);

    let (_state_cancelled, cancel_out) = apply_cancel_committed(
        &state_c,
        &CancelCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: h_id,
            expected_earmark_revision: erev + 1,
            reason: "counterparty withdrew remainder".to_owned(),
            actor_id: "trader-1".to_owned(),
        },
    )
    .expect("cancel remaining 35M");
    assert_eq!(cancel_out.earmark.state, EarmarkState::Cancelled);
    assert_eq!(cancel_out.earmark.unreconciled_notional, FixedDecimal::ZERO);
    // Reconciled 25M stays intact; cancelled 35M is freed -> A0 = 75M.
    assert_eq!(
        cancel_out.position.bucket_t0.reconciled_sell_notional,
        d_million(25)
    );
    assert_eq!(
        cancel_out
            .position
            .bucket_t0
            .committed_sell_unreconciled_notional,
        FixedDecimal::ZERO
    );
    assert_eq!(cancel_out.position.available_t0_notional, d_million(75));
}

// ============================================================================
// 3. FundingGapSummary, pending official cash & 4 disambiguation paths (F-26)
// ============================================================================

#[test]
fn funding_gap_pending_cash_exact_flag_and_four_disambiguation_paths() {
    let mut id_gen = DeterministicIdGenerator::default();
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);

    // opening_cash_t0 = 100M, opening_net_cash_delta_t1 = -120M -> P0=100M, P1=-20M (no double add of opening_cash_t0!).
    let state = init_single_account(100, 100, d_million(100), d_million(-120), 10, &mut id_gen);
    let f0 = state.funding_summary("acct-1").expect("funding");
    assert_eq!(f0.projected_cash_t0, d_million(100));
    assert_eq!(f0.projected_cash_t1, d_million(-20));
    assert_eq!(f0.funding_gap_t0, FixedDecimal::ZERO);
    assert_eq!(f0.funding_gap_t1, d_million(20));
    assert!(f0.funding_is_complete_exact);

    // Cash precedence check: explicit > exact dirty price > clean estimate > yield-only.
    let explicit = resolve_settlement_cash(
        TradeSide::Buy,
        d_million(10),
        Some(&quote_clean(100 * SCALE, Some(d_million(11)))),
        Some(ExactDirtyPriceCashInput {
            dirty_price: d(102),
            face_value: d(100),
        }),
    )
    .expect("explicit");
    assert_eq!(explicit.precision, FundingPrecision::ExactSettlementAmount);
    assert_eq!(explicit.signed_cash, Some(d_million(-11)));

    let dirty = resolve_settlement_cash(
        TradeSide::Sell,
        d_million(10),
        Some(&quote_clean(100 * SCALE, None)),
        Some(ExactDirtyPriceCashInput {
            dirty_price: d_scaled(1025 * SCALE / 10), // 102.5
            face_value: d(100),
        }),
    )
    .expect("dirty");
    assert_eq!(dirty.precision, FundingPrecision::ExactDirtyPrice);
    assert_eq!(dirty.signed_cash, Some(d_scaled(10_250_000 * SCALE)));

    let clean_est = resolve_settlement_cash(
        TradeSide::Buy,
        d_million(10),
        Some(&quote_clean(99 * SCALE, None)),
        None,
    )
    .expect("clean est");
    assert_eq!(clean_est.precision, FundingPrecision::CleanPriceEstimate);
    assert_eq!(clean_est.signed_cash, Some(d_scaled(-9_900_000 * SCALE)));

    let yield_only = resolve_settlement_cash(
        TradeSide::Buy,
        d_million(10),
        Some(&quote_yield(25_000_000_000, None)),
        None,
    )
    .expect("yield only");
    assert_eq!(yield_only.precision, FundingPrecision::UnpricedYieldOnly);
    assert_eq!(yield_only.signed_cash, None);

    // Official orphan BUY 40M exact cash enters OPEN -> recognized_flow unchanged, pending_match_cash=-40M, exact=false.
    let orphan_buy = make_fill(
        "src-1",
        "OB-1",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Buy,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );
    let (state, out_ob) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![orphan_buy],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest orphan buy");
    let pm_buy = out_ob.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    let f1 = state.funding_summary("acct-1").expect("funding");
    assert_eq!(f1.net_settlement_flow_t0, FixedDecimal::ZERO);
    assert_eq!(f1.pending_match_cash_t0, d_million(-40));
    assert_eq!(f1.pending_match_count, 1);
    assert!(!f1.funding_is_complete_exact);

    // Add equal-amount official orphan SELL 40M -> pending_match_cash nets to 0, yet exact MUST remain false!
    let orphan_sell = make_fill(
        "src-1",
        "OS-1",
        1,
        12,
        12,
        12,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );
    let (state, out_orphan_sell) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![orphan_sell],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest orphan sell");
    let pm_sell = out_orphan_sell.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    let f2 = state.funding_summary("acct-1").expect("funding");
    assert_eq!(f2.pending_match_cash_t0, FixedDecimal::ZERO);
    assert_eq!(f2.pending_match_count, 2);
    assert!(
        !f2.funding_is_complete_exact,
        "OPEN pending matches netting to 0 MUST keep funding_is_complete_exact=false"
    );

    // Path A: ACCEPT_ORPHAN on orphan_buy -> moves -40M from pending to recognized (zero overlap).
    let pv = state
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (state, acc_out) = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_buy,
            expected_pending_revision: 1,
            expected_position_version: pv,
            action: PendingMatchResolutionAction::AcceptOrphan,
            expected_earmark_revision: None,
            target_earmark_id: None,
            offset_notional: None,
            reason: "confirmed".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("accept orphan buy");
    assert_eq!(
        acc_out.funding_summary.net_settlement_flow_t0,
        d_million(-40)
    );
    assert_eq!(acc_out.funding_summary.pending_match_cash_t0, d_million(40));
    assert_eq!(acc_out.funding_summary.pending_match_count, 1);

    // Path B: RELEASE_REJECT on orphan_sell -> drops +40M pending without altering recognized flow!
    let (state, rel_out) = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_sell,
            expected_pending_revision: 1,
            expected_position_version: acc_out.position.position_version,
            action: PendingMatchResolutionAction::ReleaseReject,
            expected_earmark_revision: None,
            target_earmark_id: None,
            offset_notional: None,
            reason: "false positive".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("release reject sell");
    assert_eq!(
        rel_out.funding_summary.net_settlement_flow_t0,
        d_million(-40)
    );
    assert_eq!(
        rel_out.funding_summary.pending_match_cash_t0,
        FixedDecimal::ZERO
    );
    assert_eq!(rel_out.funding_summary.pending_match_count, 0);
    assert!(rel_out.funding_summary.funding_is_complete_exact);

    // Path C: Partial SPLIT_OFFSET shrinks pending, releases committed earmark cash, adds official slice!
    let e_buy = id_gen.next_ulid();
    let (state, c_out) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e_buy.clone(),
            expected_position_version: rel_out.position.position_version,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Buy,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(50),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(50))),
            exact_dirty_price_input: None,
            note: "buy 50".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create committed buy 50M");
    // Recognized = -40 (OB-1) + -50 (e_buy) = -90M.
    assert_eq!(c_out.funding_summary.net_settlement_flow_t0, d_million(-90));

    let fill_buy_30 = make_fill(
        "src-1",
        "TRD-B30",
        1,
        13,
        13,
        13,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Buy,
        t_day(),
        d_million(30),
        Some(quote_clean(101 * SCALE, Some(d_scaled(30_300_000 * SCALE)))), // 30.3M cash
        now.clone(),
    );
    let (state, out_b30) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![fill_buy_30],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest buy 30M");
    let pm_b30 = out_b30.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    let pv = state
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (state, split_out) = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_b30,
            expected_pending_revision: 1,
            expected_position_version: pv,
            action: PendingMatchResolutionAction::SplitOffset,
            expected_earmark_revision: Some(1),
            target_earmark_id: Some(e_buy),
            offset_notional: Some(d_million(20)),
            reason: "partial 20M".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("split offset 20M");
    // 20M of 30M fill = 20.2M official cash; 20M of 50M earmark = 20.0M released -> remaining earmark cash = -30.0M;
    // remaining pending = -10.1M; recognized = -40 (OB-1) - 30 (e_buy rem) - 20.2 (official slice) = -90.2M!
    assert_eq!(
        split_out.funding_summary.net_settlement_flow_t0,
        d_scaled(-90_200_000 * SCALE)
    );
    assert_eq!(
        split_out.funding_summary.pending_match_cash_t0,
        d_scaled(-10_100_000 * SCALE)
    );

    // Path D: CANCEL_REVERSAL on TRD-B30 reverses exact original slices -> restores recognized=-90M, pending=0!
    let cancel_b30 = make_fill(
        "src-1",
        "TRD-B30",
        2,
        14,
        14,
        14,
        TradeFeedRecordAction::CancelReversal,
        "acct-1",
        "240011.IB",
        TradeSide::Buy,
        t_day(),
        FixedDecimal::ZERO,
        None,
        now.clone(),
    );
    let (state, _) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![cancel_b30],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 5, 0),
        &mut id_gen,
    )
    .expect("cancel b30");
    let f_final = state.funding_summary("acct-1").expect("funding");
    assert_eq!(f_final.net_settlement_flow_t0, d_million(-90));
    assert_eq!(f_final.pending_match_cash_t0, FixedDecimal::ZERO);
    assert_eq!(f_final.pending_match_count, 0);
    assert!(f_final.funding_is_complete_exact);
}

// ============================================================================
// 4. Source-batch bilateral 1:1 matching, dual-permutation invariance,
//    duplicate folding, aliases, cross-batch burst, tolerance ±1 ULP & basis
// ============================================================================

#[test]
fn same_batch_two_fills_both_permutations_identical_double_open() {
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);
    let build_base_state = || {
        let mut id_gen = DeterministicIdGenerator::new(1_700_000_000_000, 1);
        let state = init_single_account(
            100,
            100,
            d_million(100),
            FixedDecimal::ZERO,
            100,
            &mut id_gen,
        );
        let e_id = id_gen.next_ulid();
        let (state, _) = apply_create_committed(
            &state,
            &CreateCommittedCommand {
                account_id: "acct-1".to_owned(),
                earmark_id: e_id,
                expected_position_version: 1,
                instrument_code: "240011.IB".to_owned(),
                side: TradeSide::Sell,
                settlement_bucket: SettlementBucket::TPlus0,
                notional: d_million(50),
                quote_terms: quote_clean(100 * SCALE, Some(d_million(50))),
                exact_dirty_price_input: None,
                note: "sell 50".to_owned(),
                trader_id: "trader-1".to_owned(),
            },
            &now,
        )
        .expect("create committed");
        (state, id_gen)
    };

    let f1 = make_fill(
        "src-1",
        "TRD-A",
        1,
        101,
        101,
        102,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );
    let f2 = make_fill(
        "src-1",
        "TRD-B",
        1,
        102,
        101,
        102,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );

    let (state_ab, mut gen_ab) = build_base_state();
    let (res_ab, out_ab) = apply_ingest_trade_feed(
        &state_ab,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f1.clone(), f2.clone()],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut gen_ab,
    )
    .expect("[f1, f2] succeeds");

    let (state_ba, mut gen_ba) = build_base_state();
    let (res_ba, out_ba) = apply_ingest_trade_feed(
        &state_ba,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f2, f1],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut gen_ba,
    )
    .expect("[f2, f1] succeeds");

    assert_eq!(out_ab, out_ba, "outcomes must be permutation-invariant");
    assert_eq!(
        res_ab, res_ba,
        "entire BlotterState must be permutation-invariant"
    );
    assert_eq!(out_ab.item_outcomes.len(), 2);
    for item in &out_ab.item_outcomes {
        assert_eq!(item.kind, TradeFillOutcomeKind::PendingMatch);
    }
    let snap = res_ab.snapshot_account("acct-1").expect("snapshot");
    assert_eq!(snap.open_pending_matches.len(), 2);
    for pm in &snap.open_pending_matches {
        assert_eq!(pm.reason, PendingMatchReason::AmbiguousCandidates);
    }
}

#[test]
fn duplicate_folding_alias_cross_batch_burst_and_existing_open_no_steal() {
    let mut id_gen = DeterministicIdGenerator::default();
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);
    let state = init_single_account(
        200,
        200,
        d_million(100),
        FixedDecimal::ZERO,
        100,
        &mut id_gen,
    );

    let e1 = id_gen.next_ulid();
    let (state, _) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e1.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(50),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(50))),
            exact_dirty_price_input: None,
            note: "sell 50".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create committed e1");

    // Duplicate exact packet twice in one call -> 1 AutoMatched + 1 PerItemDeduplicated.
    let f1 = make_fill(
        "src-1",
        "TRD-1",
        1,
        101,
        101,
        101,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );
    let (state, out_dup) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f1.clone(), f1.clone()],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("duplicate packet folded");
    assert_eq!(out_dup.item_outcomes.len(), 2);
    assert_eq!(
        out_dup.item_outcomes[0].kind,
        TradeFillOutcomeKind::AutoMatched
    );
    assert_eq!(
        out_dup.item_outcomes[1].kind,
        TradeFillOutcomeKind::PerItemDeduplicated
    );
    assert_eq!(
        state.accounts["acct-1"].earmarks[&e1].state,
        EarmarkState::Reconciled
    );

    // Alias: new feed_seq 102 delivering identical (TRD-1, rev 1) -> advances cursor, applied once!
    let mut f1_alias = f1;
    f1_alias.feed_seq = 102;
    f1_alias.batch_first_feed_seq = 102;
    f1_alias.batch_last_feed_seq = 102;
    let (state, out_alias) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f1_alias],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("alias seq advances cursor");
    assert_eq!(out_alias.feed_status.contiguous_feed_seq, 102);
    assert_eq!(
        out_alias.item_outcomes[0].kind,
        TradeFillOutcomeKind::PerItemDeduplicated
    );
    assert_eq!(
        state
            .position_view("acct-1", "240011.IB")
            .expect("pos")
            .bucket_t0
            .reconciled_sell_notional,
        d_million(50)
    );

    // Cross-batch second independent trade with no remaining commitment -> OrphanNoCommitment.
    let f2 = make_fill(
        "src-1",
        "TRD-2",
        1,
        103,
        103,
        103,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );
    let (state, out_f2) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f2],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("cross-batch second trade orphan");
    assert_eq!(
        out_f2.item_outcomes[0].kind,
        TradeFillOutcomeKind::PendingMatch
    );
    let pm2 = out_f2.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    assert_eq!(
        state.accounts["acct-1"].pending_matches[&pm2].reason,
        PendingMatchReason::OrphanNoCommitment
    );

    // Create a new COMMITTED on the same key K while pm2 is OPEN -> next fill hits ExistingPendingOnKey (no stealing!).
    let pv = state
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let e2 = id_gen.next_ulid();
    let (state, _) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e2,
            expected_position_version: pv,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(50),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(50))),
            exact_dirty_price_input: None,
            note: "sell 50 second".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create second committed");

    let f3 = make_fill(
        "src-1",
        "TRD-3",
        1,
        104,
        104,
        104,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );
    let (state, out_f3) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f3],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 1, 0),
        &mut id_gen,
    )
    .expect("f3 hits existing open");
    let pm3 = out_f3.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm3");
    assert_eq!(
        state.accounts["acct-1"].pending_matches[&pm3].reason,
        PendingMatchReason::ExistingPendingOnKey
    );
}

#[test]
fn tolerance_exact_and_plus_one_ulp_and_incomparable_basis_and_manual_validation() {
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);
    let tol = BlotterConfig::default().clean_price_tolerance.scaled(); // 50_000_000_000 (0.05)

    // 1. diff == tolerance -> AutoMatched.
    let mut id_gen = DeterministicIdGenerator::default();
    let state = init_single_account(
        100,
        100,
        d_million(100),
        FixedDecimal::ZERO,
        10,
        &mut id_gen,
    );
    let e_eq = id_gen.next_ulid();
    let (state, _) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e_eq,
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(20),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(20))),
            exact_dirty_price_input: None,
            note: "eq tol".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("committed");
    let f_eq = make_fill(
        "src-1",
        "TRD-EQ",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(20),
        Some(quote_clean(100 * SCALE + tol, Some(d_million(20)))),
        now.clone(),
    );
    let (_, out_eq) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f_eq],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("eq tol");
    assert_eq!(
        out_eq.item_outcomes[0].kind,
        TradeFillOutcomeKind::AutoMatched
    );

    // 2. diff == tolerance + 1 ULP -> PriceOutOfTolerance.
    let f_ulp = make_fill(
        "src-1",
        "TRD-ULP",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(20),
        Some(quote_clean(100 * SCALE + tol + 1, Some(d_million(20)))),
        now.clone(),
    );
    let (state_ulp, out_ulp) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f_ulp],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("+1 ULP");
    let pm_ulp = out_ulp.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    assert_eq!(
        state_ulp.accounts["acct-1"].pending_matches[&pm_ulp].reason,
        PendingMatchReason::PriceOutOfTolerance
    );

    // 3. Clean price vs yield-to-maturity -> IncomparableBasis.
    let f_ytm = make_fill(
        "src-1",
        "TRD-YTM",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(20),
        Some(quote_yield(25_000_000_000, Some(d_million(20)))),
        now.clone(),
    );
    let (state_ytm, out_ytm) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f_ytm],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 1, 0),
        &mut id_gen,
    )
    .expect("incomparable basis");
    let pm_ytm = out_ytm.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    assert_eq!(
        state_ytm.accounts["acct-1"].pending_matches[&pm_ytm].reason,
        PendingMatchReason::IncomparableBasis
    );

    // Manual validation: ACCEPT_ORPHAN with stray target_earmark_id rejected.
    let pv = state_ytm
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let bad_combo = apply_resolve_pending_match(
        &state_ytm,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_ytm,
            expected_pending_revision: 1,
            expected_position_version: pv,
            action: PendingMatchResolutionAction::AcceptOrphan,
            expected_earmark_revision: None,
            target_earmark_id: Some(id_gen.next_ulid()),
            offset_notional: None,
            reason: "invalid".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect_err("ACCEPT_ORPHAN with target_earmark_id must be rejected");
    assert_eq!(bad_combo.kind(), BlotterErrorKind::ValidationFailed);
}

// ============================================================================
// 5. Capture vs Apply: 10->12->11, rev2->rev1, rev3->rev2, HASH_MISMATCH,
//    SetFeedControl, independent pause flags & duplicate freshness
// ============================================================================

#[test]
fn gap_10_12_11_capture_vs_apply_matches_sequential_without_merging_batches() {
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);
    let build_base = || {
        let mut ids = DeterministicIdGenerator::new(1_700_000_000_000, 1);
        let st = init_single_account(100, 100, d_million(100), FixedDecimal::ZERO, 10, &mut ids);
        let eid = ids.next_ulid();
        let (st, _) = apply_create_committed(
            &st,
            &CreateCommittedCommand {
                account_id: "acct-1".to_owned(),
                earmark_id: eid,
                expected_position_version: 1,
                instrument_code: "240011.IB".to_owned(),
                side: TradeSide::Sell,
                settlement_bucket: SettlementBucket::TPlus0,
                notional: d_million(30),
                quote_terms: quote_clean(100 * SCALE, Some(d_million(30))),
                exact_dirty_price_input: None,
                note: "sell 30".to_owned(),
                trader_id: "trader-1".to_owned(),
            },
            &now,
        )
        .expect("committed 30");
        (st, ids)
    };

    // Two SEPARATE source batches [11, 11] and [12, 12] on the same key K:
    // If merged erroneously on gap recovery, both would go AmbiguousCandidates!
    // Because batches are released in strict boundary order, [11, 11] AutoMatches and [12, 12] becomes OrphanNoCommitment.
    let f11 = make_fill(
        "src-1",
        "TRD-11",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(30),
        Some(quote_clean(100 * SCALE, Some(d_million(30)))),
        now.clone(),
    );
    let f12 = make_fill(
        "src-1",
        "TRD-12",
        1,
        12,
        12,
        12,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(30),
        Some(quote_clean(100 * SCALE, Some(d_million(30)))),
        now.clone(),
    );

    // Sequential 11 -> 12
    let (seq_st, mut seq_gen) = build_base();
    let (seq_st, _) = apply_ingest_trade_feed(
        &seq_st,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f11.clone()],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut seq_gen,
    )
    .expect("seq 11");
    let (seq_st, _) = apply_ingest_trade_feed(
        &seq_st,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f12.clone()],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut seq_gen,
    )
    .expect("seq 12");

    // Out-of-order 10 -> 12 -> 11
    let (ooo_st, mut ooo_gen) = build_base();
    let (ooo_st, out12) = apply_ingest_trade_feed(
        &ooo_st,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f12],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut ooo_gen,
    )
    .expect("capture 12");
    assert_eq!(
        out12.item_outcomes[0].kind,
        TradeFillOutcomeKind::BufferedGap
    );
    assert_eq!(out12.feed_status.contiguous_feed_seq, 10);
    assert_eq!(out12.feed_status.max_seen_feed_seq, 12);
    assert_eq!(
        out12.feed_status.missing_feed_ranges,
        vec![SeqRange {
            first: 11,
            last: 11
        }]
    );
    assert!(out12.feed_status.paused_by_gap);
    assert_eq!(out12.feed_status.health_state, FeedHealthState::GapDetected);
    // 12 was NOT applied!
    assert_eq!(
        ooo_st
            .position_view("acct-1", "240011.IB")
            .expect("pos")
            .bucket_t0
            .reconciled_sell_notional,
        FixedDecimal::ZERO
    );
    assert!(
        !ooo_st
            .funding_summary("acct-1")
            .expect("funding")
            .funding_is_complete_exact
    );
    assert!(
        ooo_st
            .funding_summary("acct-1")
            .expect("funding")
            .feed_incomplete
    );

    // Operator SetFeedControl force_clear_gap_pause: empty reason rejected; valid reason clears pause
    // WITHOUT advancing contiguous_feed_seq, clearing missing ranges, or applying buffered 12!
    let bad_ctrl = apply_set_feed_control(
        &ooo_st,
        &SetFeedControlCommand {
            source_id: "src-1".to_owned(),
            expected_feed_status_revision: out12.feed_status.feed_status_revision,
            paused_by_operator: false,
            force_clear_gap_pause: true,
            reason: String::new(),
        },
    )
    .expect_err("force_clear_gap_pause requires non-empty reason");
    assert_eq!(bad_ctrl.kind(), BlotterErrorKind::ValidationFailed);

    let (ooo_ctrl, ctrl_out) = apply_set_feed_control(
        &ooo_st,
        &SetFeedControlCommand {
            source_id: "src-1".to_owned(),
            expected_feed_status_revision: out12.feed_status.feed_status_revision,
            paused_by_operator: false,
            force_clear_gap_pause: true,
            reason: "ops override during feed repair".to_owned(),
        },
    )
    .expect("force_clear_gap_pause with reason succeeds");
    assert!(!ctrl_out.feed_status.paused_by_gap);
    assert!(!ctrl_out.feed_status.earmarks_paused);
    assert_eq!(ctrl_out.feed_status.contiguous_feed_seq, 10);
    assert_eq!(
        ctrl_out.feed_status.missing_feed_ranges,
        vec![SeqRange {
            first: 11,
            last: 11
        }]
    );
    assert_eq!(
        ooo_ctrl
            .position_view("acct-1", "240011.IB")
            .expect("pos")
            .bucket_t0
            .reconciled_sell_notional,
        FixedDecimal::ZERO
    );

    // Now deliver 11 -> releases [11, 11] THEN [12, 12] separately!
    let (ooo_done, out11) = apply_ingest_trade_feed(
        &ooo_ctrl,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f11],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 1, 0),
        &mut ooo_gen,
    )
    .expect("deliver 11");
    assert_eq!(out11.feed_status.contiguous_feed_seq, 12);
    assert!(out11.feed_status.missing_feed_ranges.is_empty());

    // Economic ledger, cash, reconciled & pending balances match sequential delivery identically!
    assert_eq!(
        ooo_done
            .position_view("acct-1", "240011.IB")
            .expect("pos")
            .economic_net_notional,
        seq_st
            .position_view("acct-1", "240011.IB")
            .expect("pos")
            .economic_net_notional
    );
    assert_eq!(
        ooo_done
            .position_view("acct-1", "240011.IB")
            .expect("pos")
            .bucket_t0,
        seq_st
            .position_view("acct-1", "240011.IB")
            .expect("pos")
            .bucket_t0
    );
    assert_eq!(
        ooo_done.funding_summary("acct-1").expect("funding"),
        seq_st.funding_summary("acct-1").expect("funding")
    );
    let ooo_snap = ooo_done.snapshot_account("acct-1").expect("snap");
    assert_eq!(ooo_snap.open_pending_matches.len(), 1);
    assert_eq!(
        ooo_snap.open_pending_matches[0].reason,
        PendingMatchReason::OrphanNoCommitment
    );
    assert_eq!(ooo_snap.open_pending_matches[0].external_trade_id, "TRD-12");
}

#[test]
fn revision_hold_rev2_to_rev1_and_rev3_to_rev2_and_hash_mismatch_and_duplicate_freshness() {
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);
    let mut id_gen = DeterministicIdGenerator::default();
    let state = init_single_account(
        100,
        100,
        d_million(100),
        FixedDecimal::ZERO,
        10,
        &mut id_gen,
    );

    // 1. rev2 arrives before rev1 (contiguous feed_seq=11, NO baseline evidence):
    // MUST enter REVISION_HOLD, never misclassify as baseline, and forbid ACCEPT_ORPHAN/RELEASE_REJECT.
    let trd_rev2 = make_fill(
        "src-1",
        "TRD-R",
        2,
        11,
        11,
        11,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );
    let (state, out_r2) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![trd_rev2.clone()],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest rev2");
    assert_eq!(
        out_r2.item_outcomes[0].kind,
        TradeFillOutcomeKind::BufferedRevision
    );
    let pm_hold = out_r2.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    assert_eq!(
        state.accounts["acct-1"].pending_matches[&pm_hold].reason,
        PendingMatchReason::RevisionHold
    );
    assert!(
        state
            .funding_summary("acct-1")
            .expect("funding")
            .feed_incomplete
    );

    // ACCEPT_ORPHAN and RELEASE_REJECT forbidden on REVISION_HOLD!
    let pv = state
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    for action in [
        PendingMatchResolutionAction::AcceptOrphan,
        PendingMatchResolutionAction::ReleaseReject,
    ] {
        let err = apply_resolve_pending_match(
            &state,
            &ResolvePendingMatchInput {
                account_id: "acct-1".to_owned(),
                pending_match_id: pm_hold.clone(),
                expected_pending_revision: 1,
                expected_position_version: pv,
                action,
                expected_earmark_revision: None,
                target_earmark_id: None,
                offset_notional: None,
                reason: "forbidden shortcut".to_owned(),
                baseline_delta: None,
                actor_id: "trader-1".to_owned(),
            },
            &mut id_gen,
        )
        .expect_err("shortcut on REVISION_HOLD must be rejected");
        assert_eq!(err.kind(), BlotterErrorKind::InvalidStateTransition);
    }

    // Same feed_seq with different payload => whole-batch HASH_MISMATCH, cursor unchanged.
    let mut bad_seq_payload = trd_rev2.clone();
    bad_seq_payload.notional = d_million(41);
    let hm1 = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![bad_seq_payload],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect_err("same feed_seq different payload must fail HashMismatch");
    assert_eq!(hm1.kind(), BlotterErrorKind::HashMismatch);

    // Same (source, trade, rev) on new feed_seq with different business payload => whole-batch HASH_MISMATCH.
    let mut bad_rev_payload = trd_rev2.clone();
    bad_rev_payload.feed_seq = 12;
    bad_rev_payload.batch_first_feed_seq = 12;
    bad_rev_payload.batch_last_feed_seq = 12;
    bad_rev_payload.notional = d_million(42);
    let hm2 = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![bad_rev_payload],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect_err("same (source, trade, rev) different payload must fail HashMismatch");
    assert_eq!(hm2.kind(), BlotterErrorKind::HashMismatch);

    // Duplicate packet MUST NOT refresh staleness / heartbeat!
    let stale_time = shanghai_time(2026, 5, 18, 10, 10, 0);
    let (state_stale, sw_stale) =
        apply_sweep_lifecycle(&state, &SweepLifecycleCommand::default(), &stale_time)
            .expect("sweep marks stale");
    assert_eq!(sw_stale.newly_stale_feed_count, 1);
    assert!(
        state_stale
            .feed_status("src-1")
            .expect("fs")
            .paused_by_staleness
    );

    let (state_after_dup, dup_out) = apply_ingest_trade_feed(
        &state_stale,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![trd_rev2],
            heartbeat_at: Some(stale_time.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &stale_time,
        &mut id_gen,
    )
    .expect("duplicate batch accepted as dedup");
    assert!(!dup_out.mutated);
    assert!(
        state_after_dup
            .feed_status("src-1")
            .expect("fs")
            .paused_by_staleness,
        "duplicate batch MUST NOT clear paused_by_staleness"
    );

    // Create COMMITTED SELL 40M on key K (on non-stale state) and then supply missing rev1 (intraday)!
    let e40 = id_gen.next_ulid();
    let (state, _) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e40.clone(),
            expected_position_version: pv,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(40),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(40))),
            exact_dirty_price_input: None,
            note: "sell 40".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create committed 40M");

    // Supply intraday rev1 (feed_seq=12 > watermark 10) -> unblocks [1, 2], supersedes rev1, rev2 AutoMatches e40!
    let trd_rev1 = make_fill(
        "src-1",
        "TRD-R",
        1,
        12,
        12,
        12,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        shanghai_time(2026, 5, 18, 9, 55, 0),
    );
    let (state, _) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![trd_rev1],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("deliver missing rev1");
    assert_eq!(
        state.accounts["acct-1"].earmarks[&e40].state,
        EarmarkState::Reconciled
    );
    assert_eq!(
        state.feeds["src-1"].trade_chains["TRD-R"].latest_effective_revision,
        Some(2)
    );
    assert!(!state.funding_summary("acct-1").expect("f").feed_incomplete);

    // 2. Now test rev4 -> rev3 (generalized rev3 -> rev2 after rev1..2 applied):
    // Deliver rev4 (missing rev3) -> REVISION_HOLD; then deliver rev3 -> unblocks [3, 4], reverses rev2, applies rev4!
    let trd_rev4 = make_fill(
        "src-1",
        "TRD-R",
        4,
        13,
        13,
        13,
        TradeFeedRecordAction::CancelReversal,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        FixedDecimal::ZERO,
        None,
        now.clone(),
    );
    let (state, out_r4) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![trd_rev4],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest rev4 missing rev3");
    assert_eq!(
        out_r4.item_outcomes[0].kind,
        TradeFillOutcomeKind::BufferedRevision
    );
    // e40 still Reconciled until rev3 arrives.
    assert_eq!(
        state.accounts["acct-1"].earmarks[&e40].state,
        EarmarkState::Reconciled
    );

    let trd_rev3 = make_fill(
        "src-1",
        "TRD-R",
        3,
        14,
        14,
        14,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(35),
        Some(quote_clean(100 * SCALE, Some(d_million(35)))),
        now.clone(),
    );
    let (state, _) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![trd_rev3],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 2, 0),
        &mut id_gen,
    )
    .expect("deliver missing rev3");
    assert_eq!(
        state.feeds["src-1"].trade_chains["TRD-R"].latest_effective_revision,
        Some(4)
    );
    // Reversal of rev2 reopened e40!
    assert_eq!(
        state.accounts["acct-1"].earmarks[&e40].state,
        EarmarkState::Committed
    );
    assert!(state.accounts["acct-1"].earmarks[&e40].reopened_by_reversal);
    assert_eq!(
        state.accounts["acct-1"].earmarks[&e40].unreconciled_notional,
        d_million(40)
    );
}

// ============================================================================
// 6. Baseline evidence & APPLY_BASELINE_DELTA (4 cases + cross-bucket reject)
// ============================================================================

#[test]
fn baseline_delta_50m_four_cases_and_cross_bucket_rejection() {
    let mut id_gen = DeterministicIdGenerator::default();
    let base_time = shanghai_time(2026, 5, 18, 8, 50, 0);
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);

    // Baseline sell 50M, cash C = +50M, already reflected in opening base:
    // opening_economic = -50M, opening_available = 50M, opening_cash_t0 = +50M.
    let cash_c = d_million(50);
    let evidence = BaseEvidence {
        source_id: "src-1".to_owned(),
        external_trade_id: "BASE-50M".to_owned(),
        original_feed_seq: 5,
        original_executed_at: base_time,
        old_notional: d_million(50),
        old_signed_cash: cash_c,
        settlement_bucket: SettlementBucket::TPlus0,
        evidence_ref: "opening-ledger:BASE-50M".to_owned(),
        evidence_hash: ContentHash::digest(b"BASE-50M-v1"),
        instrument_code: "240011.IB".to_owned(),
        side: TradeSide::Sell,
        original_revision: 1,
    };

    let build_baseline_state = |ids: &mut DeterministicIdGenerator| {
        let st = BlotterState::new(BlotterConfig::default()).expect("config");
        let base = make_opening_base(
            "acct-1",
            "src-1",
            ids,
            vec![OpeningInstrumentBalance {
                instrument_code: "240011.IB".to_owned(),
                opening_economic_notional: d_million(-50),
                opening_available_notional: d_million(50),
                opening_net_t0_notional: FixedDecimal::ZERO,
                opening_net_t1_notional: FixedDecimal::ZERO,
            }],
            vec!["240011.IB".to_owned()],
            cash_c,
            FixedDecimal::ZERO,
            Some(10),
            10,
            vec![evidence.clone()],
        );
        apply_import_opening_base(
            &st,
            &ImportOpeningBaseCommand {
                actor_id: "trader-1".to_owned(),
                idempotency_key: "imp-base".to_owned(),
                opening_base: base,
                expected_account_version: 0,
            },
        )
        .expect("import")
        .0
    };

    // Case 1: Quantity amend 50M -> 40M (pro-rata C' = 40M => delta = (-10M, -10M)).
    let st_case1 = build_baseline_state(&mut id_gen);
    let amend_40 = make_fill(
        "src-1",
        "BASE-50M",
        2,
        11,
        11,
        11,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );
    let (st_case1, out_a40) = apply_ingest_trade_feed(
        &st_case1,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![amend_40],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest baseline amend 40M");
    assert_eq!(
        out_a40.item_outcomes[0].kind,
        TradeFillOutcomeKind::BaselineReview
    );
    let pm_a40 = out_a40.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    let pv1 = st_case1
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (st_case1, res1) = apply_resolve_pending_match(
        &st_case1,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_a40,
            expected_pending_revision: 1,
            expected_position_version: pv1,
            action: PendingMatchResolutionAction::ApplyBaselineDelta,
            expected_earmark_revision: None,
            target_earmark_id: None,
            offset_notional: None,
            reason: "50->40".to_owned(),
            baseline_delta: Some(BaselineDelta {
                delta_notional: d_million(-10),
                delta_cash: d_million(-10),
                settlement_bucket: SettlementBucket::TPlus0,
                evidence: evidence.clone(),
                reason: "baseline qty reduced 50M->40M".to_owned(),
            }),
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("apply baseline delta (-10M, -10M)");
    assert_eq!(res1.position.economic_net_notional, d_million(-40));
    assert_eq!(res1.position.available_t0_notional, d_million(60));
    assert_eq!(res1.funding_summary.projected_cash_t0, d_million(40));
    assert!(res1.funding_summary.funding_is_complete_exact);

    // Case 4 (chained on Case 1): 50M -> 40M -> Cancel!
    // Must atomically reverse (-10M, -10M) and compute new pending against ORIGINAL 50M evidence -> cumulative (-50M, -50M)!
    let cancel_after_40 = make_fill(
        "src-1",
        "BASE-50M",
        3,
        12,
        12,
        12,
        TradeFeedRecordAction::CancelReversal,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        FixedDecimal::ZERO,
        None,
        now.clone(),
    );
    let (st_chain, out_cancel) = apply_ingest_trade_feed(
        &st_case1,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![cancel_after_40],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest baseline cancel rev 3");
    // Prior (-10M, -10M) was reversed back to (-50M, +50M) pending new confirmation.
    assert_eq!(
        st_chain
            .position_view("acct-1", "240011.IB")
            .expect("pos")
            .economic_net_notional,
        d_million(-50)
    );
    assert_eq!(
        st_chain
            .funding_summary("acct-1")
            .expect("f")
            .projected_cash_t0,
        d_million(50)
    );
    let pm_chain = out_cancel.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    let pv_chain = st_chain
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (_st_chain_done, res_chain) = apply_resolve_pending_match(
        &st_chain,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_chain,
            expected_pending_revision: 1,
            expected_position_version: pv_chain,
            action: PendingMatchResolutionAction::ApplyBaselineDelta,
            expected_earmark_revision: None,
            target_earmark_id: None,
            offset_notional: None,
            reason: "full cancel".to_owned(),
            baseline_delta: Some(BaselineDelta {
                delta_notional: d_million(-50),
                delta_cash: d_million(-50),
                settlement_bucket: SettlementBucket::TPlus0,
                evidence: evidence.clone(),
                reason: "baseline cancelled 50->40->cancel".to_owned(),
            }),
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("apply chained cancel (-50M, -50M)");
    assert_eq!(res_chain.position.economic_net_notional, FixedDecimal::ZERO);
    assert_eq!(res_chain.position.available_t0_notional, d_million(100));
    assert_eq!(
        res_chain.funding_summary.projected_cash_t0,
        FixedDecimal::ZERO
    );

    // Case 2: Price-only amend 50M -> 50M, cash C' = 51M => delta = (0, +1M).
    let st_case2 = build_baseline_state(&mut id_gen);
    let amend_price = make_fill(
        "src-1",
        "BASE-50M",
        2,
        11,
        11,
        11,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(102 * SCALE, Some(d_million(51)))),
        now.clone(),
    );
    let (st_case2, out_p) = apply_ingest_trade_feed(
        &st_case2,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![amend_price],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest price-only amend");
    let pm_p = out_p.item_outcomes[0].pending_match_id.clone().expect("pm");
    let pv2 = st_case2
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (_, res2) = apply_resolve_pending_match(
        &st_case2,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_p,
            expected_pending_revision: 1,
            expected_position_version: pv2,
            action: PendingMatchResolutionAction::ApplyBaselineDelta,
            expected_earmark_revision: None,
            target_earmark_id: None,
            offset_notional: None,
            reason: "price-only".to_owned(),
            baseline_delta: Some(BaselineDelta {
                delta_notional: FixedDecimal::ZERO,
                delta_cash: d_million(1),
                settlement_bucket: SettlementBucket::TPlus0,
                evidence: evidence.clone(),
                reason: "baseline price amended 50M->50M".to_owned(),
            }),
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("apply price-only delta");
    assert_eq!(res2.position.economic_net_notional, d_million(-50));
    assert_eq!(res2.position.available_t0_notional, d_million(50));
    assert_eq!(res2.funding_summary.projected_cash_t0, d_million(51));

    // Case 3: Direct full cancel 50M -> Cancel => delta = (-50M, -50M).
    let st_case3 = build_baseline_state(&mut id_gen);
    let cancel_direct = make_fill(
        "src-1",
        "BASE-50M",
        2,
        11,
        11,
        11,
        TradeFeedRecordAction::CancelReversal,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        FixedDecimal::ZERO,
        None,
        now.clone(),
    );
    let (st_case3, out_c3) = apply_ingest_trade_feed(
        &st_case3,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![cancel_direct],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest direct cancel");
    let pm_c3 = out_c3.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    let pv3 = st_case3
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (_, res3) = apply_resolve_pending_match(
        &st_case3,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_c3,
            expected_pending_revision: 1,
            expected_position_version: pv3,
            action: PendingMatchResolutionAction::ApplyBaselineDelta,
            expected_earmark_revision: None,
            target_earmark_id: None,
            offset_notional: None,
            reason: "direct cancel".to_owned(),
            baseline_delta: Some(BaselineDelta {
                delta_notional: d_million(-50),
                delta_cash: d_million(-50),
                settlement_bucket: SettlementBucket::TPlus0,
                evidence: evidence.clone(),
                reason: "direct baseline cancel".to_owned(),
            }),
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("apply direct cancel");
    assert_eq!(res3.position.economic_net_notional, FixedDecimal::ZERO);
    assert_eq!(res3.position.available_t0_notional, d_million(100));
    assert_eq!(res3.funding_summary.projected_cash_t0, FixedDecimal::ZERO);

    // Cross-bucket baseline amendment (T+0 -> T+1): single-bucket APPLY_BASELINE_DELTA rejected,
    // stays OPEN in exception queue and marks funding_is_complete_exact=false!
    let st_cross = build_baseline_state(&mut id_gen);
    let amend_cross_bucket = make_fill(
        "src-1",
        "BASE-50M",
        2,
        11,
        11,
        11,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_next(), // moved from T+0 to T+1!
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );
    let (st_cross, out_cb) = apply_ingest_trade_feed(
        &st_cross,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![amend_cross_bucket],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 1, 0),
        &mut id_gen,
    )
    .expect("ingest cross-bucket baseline amend");
    let pm_cb = out_cb.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    assert!(st_cross.accounts["acct-1"].pending_matches[&pm_cb].cross_key_or_bucket_anomaly);
    assert!(
        !st_cross
            .funding_summary("acct-1")
            .expect("f")
            .funding_is_complete_exact
    );

    let pv_cb = st_cross
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let cross_err = apply_resolve_pending_match(
        &st_cross,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_cb,
            expected_pending_revision: 1,
            expected_position_version: pv_cb,
            action: PendingMatchResolutionAction::ApplyBaselineDelta,
            expected_earmark_revision: None,
            target_earmark_id: None,
            offset_notional: None,
            reason: "attempt cross bucket".to_owned(),
            baseline_delta: Some(BaselineDelta {
                delta_notional: FixedDecimal::ZERO,
                delta_cash: FixedDecimal::ZERO,
                settlement_bucket: SettlementBucket::TPlus0,
                evidence,
                reason: "invalid single bucket".to_owned(),
            }),
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect_err("cross-bucket baseline delta must be rejected");
    assert_eq!(cross_err.kind(), BlotterErrorKind::InvalidStateTransition);
}

// ============================================================================
// 7. Safe Rust 4x u64 wide-integer 10-billion 4-slice & 7-slice allocation,
//    multi-earmark 50M -> A(30M)+B(20M) reversal & non-contiguous new segment
// ============================================================================

#[test]
fn ten_billion_four_slice_and_seven_slice_wide_integer_conservation_and_tie_even() {
    // N = 10_000_000_000 (10 billion), scaled by 10^12 => 10^22 (would overflow u128 if multiplied naively!).
    let n_ten_billion = FixedDecimal::from_scaled(10_000_000_000_i128 * SCALE);
    // Non-divisible cash C = 10_123_456_789.333333333337
    let c_cash = FixedDecimal::from_scaled(10_123_456_789_i128 * SCALE + 333_333_333_337);

    // 4-slice split summing to 10B: [1.3B, 2.7B, 3.1B, 2.9B]
    let slices_4 = [
        FixedDecimal::from_scaled(1_300_000_000_i128 * SCALE),
        FixedDecimal::from_scaled(2_700_000_000_i128 * SCALE),
        FixedDecimal::from_scaled(3_100_000_000_i128 * SCALE),
        FixedDecimal::from_scaled(2_900_000_000_i128 * SCALE),
    ];
    let mut pool4 = AllocationPool::new(n_ten_billion, c_cash).expect("pool4");
    let mut sum4 = FixedDecimal::ZERO;
    for s in slices_4 {
        let (_, cash_slice) = pool4.allocate_slice(s).expect("slice");
        assert!(cash_slice.is_non_negative());
        // Verify each slice is within <= 1 ULP of ideal exact share C * s / N.
        let ideal = wide_mul_div_u128_round_half_even(
            c_cash.scaled().unsigned_abs(),
            s.scaled().unsigned_abs(),
            n_ten_billion.scaled().unsigned_abs(),
        )
        .expect("ideal");
        assert!(cash_slice.scaled().unsigned_abs().abs_diff(ideal) <= 1);
        sum4 = sum4.checked_add(cash_slice).expect("sum");
    }
    assert_eq!(sum4, c_cash, "4-slice sum MUST equal locked cash exactly");

    // 7-slice split summing to 10B (including odd/prime weights).
    let slices_7 = [
        FixedDecimal::from_scaled(1_000_000_003_i128 * SCALE),
        FixedDecimal::from_scaled(1_428_571_429_i128 * SCALE),
        FixedDecimal::from_scaled(999_999_997_i128 * SCALE),
        FixedDecimal::from_scaled(2_111_111_111_i128 * SCALE),
        FixedDecimal::from_scaled(1_777_777_777_i128 * SCALE),
        FixedDecimal::from_scaled(1_333_333_333_i128 * SCALE),
        FixedDecimal::from_scaled(1_349_206_350_i128 * SCALE),
    ];
    let mut pool7 = AllocationPool::new(n_ten_billion, c_cash).expect("pool7");
    let mut sum7 = FixedDecimal::ZERO;
    let mut prev_b = 0_u128;
    let mut cum_x = 0_u128;
    for s in slices_7 {
        cum_x += s.scaled().unsigned_abs();
        let b_x = cumulative_cash_magnitude(
            c_cash.scaled().unsigned_abs(),
            cum_x,
            n_ten_billion.scaled().unsigned_abs(),
        )
        .expect("B(x)");
        assert!(b_x >= prev_b, "B(x) must be monotone non-decreasing");
        prev_b = b_x;

        let (_, cash_slice) = pool7.allocate_slice(s).expect("slice7");
        assert!(cash_slice.is_non_negative());
        let ideal = wide_mul_div_u128_round_half_even(
            c_cash.scaled().unsigned_abs(),
            s.scaled().unsigned_abs(),
            n_ten_billion.scaled().unsigned_abs(),
        )
        .expect("ideal7");
        assert!(cash_slice.scaled().unsigned_abs().abs_diff(ideal) <= 1);
        sum7 = sum7.checked_add(cash_slice).expect("sum7");
    }
    assert_eq!(sum7, c_cash, "7-slice sum MUST equal locked cash exactly");
    assert_eq!(
        cumulative_cash_magnitude(
            c_cash.scaled().unsigned_abs(),
            0,
            n_ten_billion.scaled().unsigned_abs()
        )
        .expect("B(0)"),
        0
    );
    assert_eq!(
        cumulative_cash_magnitude(
            c_cash.scaled().unsigned_abs(),
            n_ten_billion.scaled().unsigned_abs(),
            n_ten_billion.scaled().unsigned_abs()
        )
        .expect("B(N)"),
        c_cash.scaled().unsigned_abs()
    );

    // Direct tie-half-to-even verification: 1/2 -> 0 (even), 3/2 -> 2 (even), 5/2 -> 2 (even), 7/2 -> 4 (even).
    assert_eq!(wide_mul_div_u128_round_half_even(1, 1, 2).expect("0.5"), 0);
    assert_eq!(wide_mul_div_u128_round_half_even(3, 1, 2).expect("1.5"), 2);
    assert_eq!(wide_mul_div_u128_round_half_even(5, 1, 2).expect("2.5"), 2);
    assert_eq!(wide_mul_div_u128_round_half_even(7, 1, 2).expect("3.5"), 4);

    // High-bit divisor near u128::MAX (tests carry bit in 256/128 long division!).
    let big_den = u128::MAX - 17;
    let prod = mul_u128_wide(big_den, 12_345_678_901_234_567_890_u128);
    let (q, r) = div_rem_u256_by_u128(prod, big_den).expect("div");
    assert_eq!(q, U256::from_u128(12_345_678_901_234_567_890_u128));
    assert_eq!(r, 0);
}

#[test]
fn fifty_million_split_across_two_earmarks_and_exact_reversal_with_new_segment() {
    let mut id_gen = DeterministicIdGenerator::default();
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);
    let state = init_single_account(
        100,
        100,
        d_million(100),
        FixedDecimal::ZERO,
        10,
        &mut id_gen,
    );

    // Earmark A = 30M (locked cash +30_000_000.000000000001)
    // Earmark B = 20M (locked cash +20_000_000.000000000002)
    let cash_a = d_scaled(30_000_000 * SCALE + 1);
    let cash_b = d_scaled(20_000_000 * SCALE + 2);
    let ea = id_gen.next_ulid();
    let eb = id_gen.next_ulid();

    let (state, out_a) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: ea.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(30),
            quote_terms: quote_clean(100 * SCALE, Some(cash_a)),
            exact_dirty_price_input: None,
            note: "A 30M".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create A");
    let (state, out_b) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: eb.clone(),
            expected_position_version: out_a.position.position_version,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(20),
            quote_terms: quote_clean(100 * SCALE, Some(cash_b)),
            exact_dirty_price_input: None,
            note: "B 20M".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create B");

    // 50M official fill with non-divisible cash 50_111_111.111111111113 enters OPEN (AmbiguousCandidates).
    let fill_cash = d_scaled(50_111_111 * SCALE + 111_111_111_113);
    let fill_50 = make_fill(
        "src-1",
        "TRD-50M",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(fill_cash))),
        now.clone(),
    );
    let (state, ing_50) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![fill_50],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest 50M");
    let pm_50 = ing_50.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");

    // Split 50M across A(30M) and B(20M).
    let pv = state
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (state, sp_a) = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_50.clone(),
            expected_pending_revision: 1,
            expected_position_version: pv,
            action: PendingMatchResolutionAction::SplitOffset,
            expected_earmark_revision: Some(out_a.earmark.earmark_revision),
            target_earmark_id: Some(ea.clone()),
            offset_notional: Some(d_million(30)),
            reason: "offset A".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("split A");
    let (state, sp_b) = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_50,
            expected_pending_revision: sp_a.pending_match.pending_revision,
            expected_position_version: sp_a.position.position_version,
            action: PendingMatchResolutionAction::SplitOffset,
            expected_earmark_revision: Some(out_b.earmark.earmark_revision),
            target_earmark_id: Some(eb.clone()),
            offset_notional: Some(d_million(20)),
            reason: "offset B".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("split B");

    // Verify fill slices sum exactly to fill_cash!
    let slice_a = sp_a
        .allocation
        .expect("alloc A")
        .allocated_fill_cash
        .expect("cash A");
    let slice_b = sp_b
        .allocation
        .expect("alloc B")
        .allocated_fill_cash
        .expect("cash B");
    assert_eq!(slice_a.checked_add(slice_b).expect("sum"), fill_cash);
    assert_eq!(
        state.accounts["acct-1"].earmarks[&ea].state,
        EarmarkState::Reconciled
    );
    assert_eq!(
        state.accounts["acct-1"].earmarks[&eb].state,
        EarmarkState::Reconciled
    );

    // CANCEL_REVERSAL of TRD-50M directly reverses both allocations -> restores A and B to exact original values!
    let cancel_50 = make_fill(
        "src-1",
        "TRD-50M",
        2,
        12,
        12,
        12,
        TradeFeedRecordAction::CancelReversal,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        FixedDecimal::ZERO,
        None,
        now.clone(),
    );
    let (state_rev, _) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![cancel_50],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("cancel 50M");

    let earmark_a_rec = &state_rev.accounts["acct-1"].earmarks[&ea];
    let earmark_b_rec = &state_rev.accounts["acct-1"].earmarks[&eb];
    assert_eq!(earmark_a_rec.state, EarmarkState::Committed);
    assert_eq!(earmark_b_rec.state, EarmarkState::Committed);
    assert!(earmark_a_rec.reopened_by_reversal);
    assert!(earmark_b_rec.reopened_by_reversal);
    assert_eq!(earmark_a_rec.unreconciled_notional, d_million(30));
    assert_eq!(earmark_a_rec.unreconciled_settlement_cash, Some(cash_a));
    assert_eq!(earmark_b_rec.unreconciled_notional, d_million(20));
    assert_eq!(earmark_b_rec.unreconciled_settlement_cash, Some(cash_b));
    // Historical segment 1 preserved; fresh segment 2 opened!
    assert_eq!(earmark_a_rec.allocation_pool.segment_history.len(), 1);
    assert_eq!(
        earmark_a_rec.allocation_pool.active_segment_index(),
        Some(2)
    );
    assert_eq!(earmark_b_rec.allocation_pool.segment_history.len(), 1);
    assert_eq!(
        earmark_b_rec.allocation_pool.active_segment_index(),
        Some(2)
    );

    // Verify funding restored to exact pre-fill state (cash_a + cash_b).
    let expected_flow = cash_a.checked_add(cash_b).expect("sum");
    assert_eq!(
        state_rev
            .funding_summary("acct-1")
            .expect("f")
            .net_settlement_flow_t0,
        expected_flow
    );

    // Partial non-contiguous reversal test on a single 30M earmark (allocated 10M from fill1 + 10M from fill2,
    // then fill1 reversed -> remainder 20M starts segment 2 anchored at exact remaining (20M, C_rem) and sums to C!).
    let mut pool = AllocationPool::new(d_million(30), cash_a).expect("pool");
    let (_, c1) = pool.allocate_slice(d_million(10)).expect("s1");
    let (_, c2) = pool.allocate_slice(d_million(10)).expect("s2");
    // Reverse c1 while c2 stays active -> remaining pool = (20M, cash_a - c2).
    let rem_cash = cash_a.checked_sub(c2).expect("rem");
    let seg2 = pool
        .open_new_segment_from_remainder(d_million(20), rem_cash)
        .expect("new seg");
    assert_eq!(seg2, 2);
    assert_eq!(pool.segment_history.len(), 1);
    let (_, c3) = pool.allocate_slice(d_million(7)).expect("s3");
    let (_, c4) = pool.allocate_slice(d_million(13)).expect("s4");
    // Surviving c2 + new segment slices c3 + c4 MUST equal original cash_a with zero drift (ignoring reversed c1)!
    let _ = c1;
    let total_active = c2.checked_add(c3).expect("s").checked_add(c4).expect("s");
    assert_eq!(total_active, cash_a);
}

proptest! {
    #[test]
    fn proptest_wide_cumulative_allocation_conserves_exact_cash_and_bounds_error(
        // Total notional between 1 and 100 billion (scaled 10^12)
        n_units in 1_u128..=100_000_000_000_u128,
        // Cash between 1 and 100 billion with arbitrary sub-ULP fractional tail
        c_whole in 1_u128..=100_000_000_000_u128,
        c_frac in 0_u128..1_000_000_000_000_u128,
        w1 in 1_u64..1000,
        w2 in 1_u64..1000,
        w3 in 1_u64..1000,
        w4 in 1_u64..1000,
    ) {
        let total_n = n_units * 1_000_000_000_000_u128;
        let total_c = c_whole * 1_000_000_000_000_u128 + c_frac;
        let w_sum = u128::from(w1) + u128::from(w2) + u128::from(w3) + u128::from(w4);

        let s1 = (total_n * u128::from(w1) / w_sum).max(1);
        let s2 = (total_n * u128::from(w2) / w_sum).max(1);
        let s3 = (total_n * u128::from(w3) / w_sum).max(1);
        prop_assume!(s1 + s2 + s3 < total_n);
        let s4 = total_n - s1 - s2 - s3;

        let mut pool = AllocationPool::new(
            FixedDecimal::from_scaled(i128::try_from(total_n).expect("fits i128")),
            FixedDecimal::from_scaled(i128::try_from(total_c).expect("fits i128")),
        ).expect("pool");

        let mut sum = 0_u128;
        for slice_n in [s1, s2, s3, s4] {
            let (_, cash_slice) = pool
                .allocate_slice(FixedDecimal::from_scaled(i128::try_from(slice_n).expect("fits")))
                .expect("alloc");
            prop_assert!(cash_slice.is_non_negative());
            let ideal = wide_mul_div_u128_round_half_even(total_c, slice_n, total_n).expect("ideal");
            prop_assert!(cash_slice.scaled().unsigned_abs().abs_diff(ideal) <= 1);
            sum += cash_slice.scaled().unsigned_abs();
        }
        prop_assert_eq!(sum, total_c);
    }

    #[test]
    fn proptest_u256_mul_div_exact_roundtrip(
        a in 1_u128..=u128::MAX,
        b in 1_u128..=u128::MAX,
    ) {
        let prod = mul_u128_wide(a, b);
        let (q, r) = div_rem_u256_by_u128(prod, a).expect("div by a");
        prop_assert_eq!(q, U256::from_u128(b));
        prop_assert_eq!(r, 0);
    }
}

#[test]
fn config_validation_rejects_invalid_thresholds() {
    let bad = BlotterConfig {
        committed_overdue_seconds: 3599,
        ..Default::default()
    };
    assert_eq!(
        BlotterState::new(bad).expect_err("below 3600").kind(),
        BlotterErrorKind::ValidationFailed
    );

    let bad2 = BlotterConfig {
        committed_overdue_seconds: 5401,
        ..Default::default()
    };
    assert_eq!(
        BlotterState::new(bad2).expect_err("above 5400").kind(),
        BlotterErrorKind::ValidationFailed
    );

    let good = BlotterConfig {
        pre_close_alert_time: NaiveTime::from_hms_opt(16, 30, 0).expect("valid"),
        committed_overdue_seconds: 5400,
        ..Default::default()
    };
    assert!(BlotterState::new(good).is_ok());
}

#[test]
fn baseline_evidence_via_pre_watermark_new_fill_and_time_watermark_fallback() {
    let mut id_gen = DeterministicIdGenerator::default();
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);

    // Account with watermark_source_seq = None -> uses original executed_at <= watermark_time (09:00:00).
    let st = BlotterState::new(BlotterConfig::default()).expect("config");
    let base = make_opening_base(
        "acct-time",
        "src-time",
        &mut id_gen,
        vec![OpeningInstrumentBalance {
            instrument_code: "240011.IB".to_owned(),
            opening_economic_notional: d_million(-50),
            opening_available_notional: d_million(50),
            opening_net_t0_notional: FixedDecimal::ZERO,
            opening_net_t1_notional: FixedDecimal::ZERO,
        }],
        vec!["240011.IB".to_owned()],
        d_million(50),
        FixedDecimal::ZERO,
        None,
        10,
        Vec::new(), // No OpeningPositionBase.baseline_evidence!
    );
    let (st, _) = apply_import_opening_base(
        &st,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-time".to_owned(),
            opening_base: base,
            expected_account_version: 0,
        },
    )
    .expect("import");

    // rev 2 arrives first (no rev 1 yet) -> MUST go to REVISION_HOLD (never assumed baseline!).
    let rev2 = make_fill(
        "src-time",
        "T-WM",
        2,
        11,
        11,
        11,
        TradeFeedRecordAction::Amend,
        "acct-time",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );
    let (st, out2) = apply_ingest_trade_feed(
        &st,
        &IngestTradeFeedInput {
            source_id: "src-time".to_owned(),
            fills: vec![rev2],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest rev2");
    assert_eq!(
        out2.item_outcomes[0].kind,
        TradeFillOutcomeKind::BufferedRevision
    );

    // Fetch historical rev 1 with executed_at = 08:55:00 <= 09:00:00 (pre-watermark!).
    let rev1_pre = make_fill(
        "src-time",
        "T-WM",
        1,
        5,
        5,
        5,
        TradeFeedRecordAction::NewFill,
        "acct-time",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        shanghai_time(2026, 5, 18, 8, 55, 0),
    );
    let (st, out1) = apply_ingest_trade_feed(
        &st,
        &IngestTradeFeedInput {
            source_id: "src-time".to_owned(),
            fills: vec![rev1_pre],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 1, 0),
        &mut id_gen,
    )
    .expect("ingest historical rev1");
    assert_eq!(
        out1.item_outcomes[0].kind,
        TradeFillOutcomeKind::PreWatermarkIgnored
    );
    let snap = st.snapshot_account("acct-time").expect("snap");
    assert_eq!(snap.open_pending_matches.len(), 1);
    assert_eq!(
        snap.open_pending_matches[0].reason,
        PendingMatchReason::BaselineRevisionManual
    );
    assert_eq!(
        snap.open_pending_matches[0]
            .evidence
            .as_ref()
            .expect("proven evidence")
            .old_notional,
        d_million(50)
    );
}

#[test]
fn sell_t1_transitions_and_manual_match_full_four_key_and_bilateral_cas() {
    let mut id_gen = DeterministicIdGenerator::default();
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);
    let state = init_single_account(50, 50, d_million(100), FixedDecimal::ZERO, 10, &mut id_gen);

    // SELL T+1 HOLD for 50M succeeds; exceeding A1 rejected.
    let too_big = apply_create_hold(
        &state,
        &CreateHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: id_gen.next_ulid(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus1,
            notional: d_million(51),
            quote_terms: None,
            ttl_seconds: 1800,
            note: "too big".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect_err("SELL T+1 > A1 rejected");
    assert_eq!(too_big.kind(), BlotterErrorKind::InsufficientAvailable);

    let e_t1 = id_gen.next_ulid();
    let (state, h1) = apply_create_hold(
        &state,
        &CreateHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e_t1.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus1,
            notional: d_million(50),
            quote_terms: None,
            ttl_seconds: 1800,
            note: "sell t1 50M".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("SELL T+1 hold succeeds");
    // A0 stays 50M (T+1 sell does not consume A0), A1 drops to 0.
    assert_eq!(h1.position.available_t0_notional, d_million(50));
    assert_eq!(h1.position.available_t1_notional, FixedDecimal::ZERO);

    let (state, c1) = apply_confirm_hold(
        &state,
        &ConfirmHoldCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e_t1.clone(),
            expected_earmark_revision: 1,
            expected_position_version: h1.position.position_version,
            quote_terms: quote_clean(100 * SCALE, Some(d_million(50))),
            exact_dirty_price_input: None,
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("SELL T+1 confirm succeeds");
    assert_eq!(c1.earmark.state, EarmarkState::Committed);

    // Fill on T+0 (different settlement_date -> different K!) goes OPEN (OrphanNoCommitment).
    let fill_t0 = make_fill(
        "src-1",
        "TRD-T0-MISMATCH",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );
    let (state, ing0) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![fill_t0],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest T+0 fill");
    let pm_t0 = ing0.item_outcomes[0].pending_match_id.clone().expect("pm");
    let pv = state
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;

    // Manual MATCH_FULL against T+1 earmark fails four-key K check!
    let k_err = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_t0.clone(),
            expected_pending_revision: 1,
            expected_position_version: pv,
            action: PendingMatchResolutionAction::MatchFull,
            expected_earmark_revision: Some(c1.earmark.earmark_revision),
            target_earmark_id: Some(e_t1.clone()),
            offset_notional: None,
            reason: "wrong bucket".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect_err("four-key K mismatch must be rejected");
    assert_eq!(k_err.kind(), BlotterErrorKind::ValidationFailed);

    // Bilateral CAS check: stale expected_earmark_revision rejected with ConcurrencyConflict.
    let cas_err = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_t0,
            expected_pending_revision: 1,
            expected_position_version: pv,
            action: PendingMatchResolutionAction::MatchFull,
            expected_earmark_revision: Some(999),
            target_earmark_id: Some(e_t1),
            offset_notional: None,
            reason: "stale earmark rev".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect_err("stale expected_earmark_revision must fail ConcurrencyConflict");
    assert_eq!(cas_err.kind(), BlotterErrorKind::ConcurrencyConflict);
}

// ============================================================================
// 8. M1 Review Regression Tests (B-1, B-2, M-1, M-2, M-3, m-2, m-3)
// ============================================================================

#[test]
fn b1_collateral_revision_hold_unblocks_after_predecessor_without_key_contagion() {
    let mut id_gen = DeterministicIdGenerator::default();
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);
    let state = init_single_account(
        200,
        200,
        d_million(100),
        FixedDecimal::ZERO,
        10,
        &mut id_gen,
    );

    let e50 = id_gen.next_ulid();
    let (state, c50) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e50.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(50),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(50))),
            exact_dirty_price_input: None,
            note: "sell 50".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create committed 50M");

    // Batch 1: TRD-2 rev 2 (missing rev 1) arrives on key K -> enters RevisionHold.
    let trd2_rev2 = make_fill(
        "src-1",
        "TRD-2",
        2,
        11,
        11,
        11,
        TradeFeedRecordAction::CancelReversal,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        FixedDecimal::ZERO,
        None,
        now.clone(),
    );
    let (state, out_b1) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![trd2_rev2],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest TRD-2 rev 2");
    assert_eq!(
        out_b1.item_outcomes[0].kind,
        TradeFillOutcomeKind::BufferedRevision
    );
    let pm_trd2 = out_b1.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm_trd2");
    assert_eq!(
        state.accounts["acct-1"].pending_matches[&pm_trd2].reason,
        PendingMatchReason::RevisionHold
    );

    // Batch 2: Complete valid TRD-1 rev 1 arrives on same key K -> blocked from auto-match,
    // tagged ExistingPendingOnKey (NOT permanent RevisionHold!).
    let trd1_rev1 = make_fill(
        "src-1",
        "TRD-1",
        1,
        12,
        12,
        12,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );
    let (state, out_b2) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![trd1_rev1],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest TRD-1 rev 1");
    assert_eq!(
        out_b2.item_outcomes[0].kind,
        TradeFillOutcomeKind::PendingMatch
    );
    let pm_trd1 = out_b2.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm_trd1");
    assert_eq!(
        state.accounts["acct-1"].pending_matches[&pm_trd1].reason,
        PendingMatchReason::ExistingPendingOnKey
    );

    // While TRD-2 is STILL in active RevisionHold on key K, resolving pm_trd1 is blocked.
    let pv_while_held = state
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    for action in [
        PendingMatchResolutionAction::AcceptOrphan,
        PendingMatchResolutionAction::ReleaseReject,
    ] {
        let err = apply_resolve_pending_match(
            &state,
            &ResolvePendingMatchInput {
                account_id: "acct-1".to_owned(),
                pending_match_id: pm_trd1.clone(),
                expected_pending_revision: 1,
                expected_position_version: pv_while_held,
                action,
                expected_earmark_revision: None,
                target_earmark_id: None,
                offset_notional: None,
                reason: "blocked while peer trade held".to_owned(),
                baseline_delta: None,
                actor_id: "trader-1".to_owned(),
            },
            &mut id_gen,
        )
        .expect_err("must block while key has live RevisionHold");
        assert_eq!(err.kind(), BlotterErrorKind::InvalidStateTransition);
    }

    // Batch 3: Supply missing TRD-2 rev 1 -> unblocks & closes pm_trd2.
    let trd2_rev1 = make_fill(
        "src-1",
        "TRD-2",
        1,
        13,
        13,
        13,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(20),
        Some(quote_clean(100 * SCALE, Some(d_million(20)))),
        now.clone(),
    );
    let (state, _) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![trd2_rev1],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("deliver TRD-2 rev 1");
    assert_eq!(
        state.accounts["acct-1"].pending_matches[&pm_trd2].state,
        PendingMatchState::ReversedByFeed
    );
    assert!(!state.funding_summary("acct-1").expect("f").feed_incomplete);

    // Now that real RevisionHold is cleared, pm_trd1 MUST be resolvable via ALL 4 actions:
    // verify SPLIT_OFFSET, ACCEPT_ORPHAN, RELEASE_REJECT on cloned states, and commit MATCH_FULL!
    let pv_unblocked = state
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (_, split_res) = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_trd1.clone(),
            expected_pending_revision: 1,
            expected_position_version: pv_unblocked,
            action: PendingMatchResolutionAction::SplitOffset,
            expected_earmark_revision: Some(c50.earmark.earmark_revision),
            target_earmark_id: Some(e50.clone()),
            offset_notional: Some(d_million(20)),
            reason: "split after hold cleared".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("SPLIT_OFFSET must succeed after RevisionHold cleared");
    assert_eq!(split_res.pending_match.unresolved_notional, d_million(30));

    let (_, orphan_res) = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_trd1.clone(),
            expected_pending_revision: 1,
            expected_position_version: pv_unblocked,
            action: PendingMatchResolutionAction::AcceptOrphan,
            expected_earmark_revision: None,
            target_earmark_id: None,
            offset_notional: None,
            reason: "accept after hold cleared".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("ACCEPT_ORPHAN must succeed after RevisionHold cleared");
    assert_eq!(
        orphan_res.pending_match.state,
        PendingMatchState::ResolvedOrphanAccepted
    );

    let (_, release_res) = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_trd1.clone(),
            expected_pending_revision: 1,
            expected_position_version: pv_unblocked,
            action: PendingMatchResolutionAction::ReleaseReject,
            expected_earmark_revision: None,
            target_earmark_id: None,
            offset_notional: None,
            reason: "release after hold cleared".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("RELEASE_REJECT must succeed after RevisionHold cleared");
    assert_eq!(release_res.pending_match.state, PendingMatchState::Released);

    // Commit MATCH_FULL on main state and verify subsequent trade on key K auto-matches cleanly!
    let (state, match_res) = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_trd1,
            expected_pending_revision: 1,
            expected_position_version: pv_unblocked,
            action: PendingMatchResolutionAction::MatchFull,
            expected_earmark_revision: Some(c50.earmark.earmark_revision),
            target_earmark_id: Some(e50.clone()),
            offset_notional: None,
            reason: "match full after hold cleared".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("MATCH_FULL must succeed after RevisionHold cleared");
    assert_eq!(
        match_res.pending_match.state,
        PendingMatchState::ResolvedMatched
    );
    assert_eq!(
        state.accounts["acct-1"].earmarks[&e50].state,
        EarmarkState::Reconciled
    );

    // New COMMITTED 30M + new TRD-3 rev 1 on same key K -> MUST auto-match (no permanent contagion!).
    let e30 = id_gen.next_ulid();
    let pv_after = state
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (state, _) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e30.clone(),
            expected_position_version: pv_after,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(30),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(30))),
            exact_dirty_price_input: None,
            note: "sell 30".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create committed 30M");
    let trd3 = make_fill(
        "src-1",
        "TRD-3",
        1,
        14,
        14,
        14,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(30),
        Some(quote_clean(100 * SCALE, Some(d_million(30)))),
        now.clone(),
    );
    let (state, out_b4) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![trd3],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 1, 0),
        &mut id_gen,
    )
    .expect("ingest TRD-3");
    assert_eq!(
        out_b4.item_outcomes[0].kind,
        TradeFillOutcomeKind::AutoMatched
    );
    assert_eq!(
        state.accounts["acct-1"].earmarks[&e30].state,
        EarmarkState::Reconciled
    );
}

#[test]
fn b1_collateral_gap_hold_unblocks_after_gap_healed_without_key_contagion() {
    let mut id_gen = DeterministicIdGenerator::default();
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);
    let state = BlotterState::new(BlotterConfig::default()).expect("config");
    let base = make_opening_base(
        "acct-1",
        "src-1",
        &mut id_gen,
        vec![
            OpeningInstrumentBalance {
                instrument_code: "240011.IB".to_owned(),
                opening_economic_notional: d_million(200),
                opening_available_notional: d_million(200),
                opening_net_t0_notional: FixedDecimal::ZERO,
                opening_net_t1_notional: FixedDecimal::ZERO,
            },
            OpeningInstrumentBalance {
                instrument_code: "240012.IB".to_owned(),
                opening_economic_notional: d_million(200),
                opening_available_notional: d_million(200),
                opening_net_t0_notional: FixedDecimal::ZERO,
                opening_net_t1_notional: FixedDecimal::ZERO,
            },
        ],
        vec!["240011.IB".to_owned(), "240012.IB".to_owned()],
        d_million(100),
        FixedDecimal::ZERO,
        Some(8),
        8,
        Vec::new(),
    );
    let (state, _) = apply_import_opening_base(
        &state,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-gap".to_owned(),
            opening_base: base,
            expected_account_version: 0,
        },
    )
    .expect("import");

    let e50 = id_gen.next_ulid();
    let (state, c50) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e50.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(50),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(50))),
            exact_dirty_price_input: None,
            note: "sell 50".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create e50");

    // 1. High packet seq=12 creates gap [9..11].
    let f12 = make_fill(
        "src-1",
        "TRD-12",
        1,
        12,
        12,
        12,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240012.IB",
        TradeSide::Sell,
        t_day(),
        d_million(10),
        Some(quote_clean(100 * SCALE, Some(d_million(10)))),
        now.clone(),
    );
    let (state, _) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f12],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest 12");

    // 2. Low complete batch [9, 10] releases while higher gap [11..11] is still active!
    let f9 = make_fill(
        "src-1",
        "TRD-LOW9",
        1,
        9,
        9,
        10,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );
    let f10 = make_fill(
        "src-1",
        "TRD-LOW10",
        1,
        10,
        9,
        10,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240012.IB",
        TradeSide::Sell,
        t_day(),
        d_million(10),
        Some(quote_clean(100 * SCALE, Some(d_million(10)))),
        now.clone(),
    );
    let (state, out_low) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f9, f10],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest [9,10]");
    assert_eq!(
        out_low.feed_status.missing_feed_ranges,
        vec![SeqRange {
            first: 11,
            last: 11
        }]
    );
    let pm_low9 = out_low.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm_low9");
    assert_eq!(
        state.accounts["acct-1"].pending_matches[&pm_low9].reason,
        PendingMatchReason::GapHold
    );

    // Operator force_clear_gap_pause clears pause bit but NEVER heals missing_feed_ranges ->
    // resolving pm_low9 MUST STILL be rejected while [11..11] is missing!
    let (state_forced, _) = apply_set_feed_control(
        &state,
        &SetFeedControlCommand {
            source_id: "src-1".to_owned(),
            expected_feed_status_revision: state
                .feed_status("src-1")
                .expect("fs")
                .feed_status_revision,
            paused_by_operator: false,
            force_clear_gap_pause: true,
            reason: "operator test".to_owned(),
        },
    )
    .expect("force clear pause");
    let pv_gap = state_forced
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let err_while_gap = apply_resolve_pending_match(
        &state_forced,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_low9.clone(),
            expected_pending_revision: 1,
            expected_position_version: pv_gap,
            action: PendingMatchResolutionAction::MatchFull,
            expected_earmark_revision: Some(c50.earmark.earmark_revision),
            target_earmark_id: Some(e50.clone()),
            offset_notional: None,
            reason: "still missing seq 11".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect_err("must reject while gap [11..11] is active");
    assert_eq!(
        err_while_gap.kind(),
        BlotterErrorKind::InvalidStateTransition
    );

    // 3. Deliver missing seq=11 -> gap heals (missing_feed_ranges empty)!
    let f11 = make_fill(
        "src-1",
        "TRD-11",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240012.IB",
        TradeSide::Sell,
        t_day(),
        d_million(10),
        Some(quote_clean(100 * SCALE, Some(d_million(10)))),
        now.clone(),
    );
    let (state, out_11) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f11],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("deliver seq 11");
    assert!(out_11.feed_status.missing_feed_ranges.is_empty());

    // 4. Now pm_low9 MUST be manually resolvable via MATCH_FULL (and subsequent trades auto-match).
    let pv_healed = state
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (state, res_low9) = apply_resolve_pending_match(
        &state,
        &ResolvePendingMatchInput {
            account_id: "acct-1".to_owned(),
            pending_match_id: pm_low9,
            expected_pending_revision: 1,
            expected_position_version: pv_healed,
            action: PendingMatchResolutionAction::MatchFull,
            expected_earmark_revision: Some(c50.earmark.earmark_revision),
            target_earmark_id: Some(e50.clone()),
            offset_notional: None,
            reason: "resolve after gap healed".to_owned(),
            baseline_delta: None,
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect("MATCH_FULL on GapHold item must succeed once gap is healed");
    assert_eq!(
        res_low9.pending_match.state,
        PendingMatchState::ResolvedMatched
    );
    assert_eq!(
        state.accounts["acct-1"].earmarks[&e50].state,
        EarmarkState::Reconciled
    );

    let e20 = id_gen.next_ulid();
    let pv_next = state
        .position_view("acct-1", "240011.IB")
        .expect("pos")
        .position_version;
    let (state, _) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e20.clone(),
            expected_position_version: pv_next,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(20),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(20))),
            exact_dirty_price_input: None,
            note: "sell 20".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create e20");
    let f13 = make_fill(
        "src-1",
        "TRD-13",
        1,
        13,
        13,
        13,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(20),
        Some(quote_clean(100 * SCALE, Some(d_million(20)))),
        now.clone(),
    );
    let (state, out_13) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![f13],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 1, 0),
        &mut id_gen,
    )
    .expect("ingest 13");
    assert_eq!(
        out_13.item_outcomes[0].kind,
        TradeFillOutcomeKind::AutoMatched
    );
    assert_eq!(
        state.accounts["acct-1"].earmarks[&e20].state,
        EarmarkState::Reconciled
    );
}

#[test]
fn b2_revision_hold_no_double_counting_on_nonzero_amend_chain_and_baseline() {
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);
    let mut id_seq = DeterministicIdGenerator::new(1_700_000_000_000, 1);
    let mut id_ooo = DeterministicIdGenerator::new(1_700_000_000_000, 1);

    // Scenario 1: Intraday 50M sell reconciled -> missing rev2 -> rev3(AMEND 45M) -> rev4(AMEND 40M) -> rev2(AMEND 48M).
    let seq_state = init_single_account(
        100,
        100,
        d_million(100),
        FixedDecimal::ZERO,
        10,
        &mut id_seq,
    );
    let ooo_state = init_single_account(
        100,
        100,
        d_million(100),
        FixedDecimal::ZERO,
        10,
        &mut id_ooo,
    );

    let e50 = id_seq.next_ulid();
    let e50_ooo = id_ooo.next_ulid();
    assert_eq!(e50, e50_ooo);

    let create_cmd = CreateCommittedCommand {
        account_id: "acct-1".to_owned(),
        earmark_id: e50.clone(),
        expected_position_version: 1,
        instrument_code: "240011.IB".to_owned(),
        side: TradeSide::Sell,
        settlement_bucket: SettlementBucket::TPlus0,
        notional: d_million(50),
        quote_terms: quote_clean(100 * SCALE, Some(d_million(50))),
        exact_dirty_price_input: None,
        note: "sell 50".to_owned(),
        trader_id: "trader-1".to_owned(),
    };
    let (seq_state, _) = apply_create_committed(&seq_state, &create_cmd, &now).expect("seq e50");
    let (ooo_state, _) = apply_create_committed(&ooo_state, &create_cmd, &now).expect("ooo e50");

    let rev1 = make_fill(
        "src-1",
        "TRD-X",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );
    let rev2_seq = make_fill(
        "src-1",
        "TRD-X",
        2,
        12,
        12,
        12,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(48),
        Some(quote_clean(100 * SCALE, Some(d_million(48)))),
        now.clone(),
    );
    let rev3_seq = make_fill(
        "src-1",
        "TRD-X",
        3,
        13,
        13,
        13,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(45),
        Some(quote_clean(100 * SCALE, Some(d_million(45)))),
        now.clone(),
    );
    let rev4_seq = make_fill(
        "src-1",
        "TRD-X",
        4,
        14,
        14,
        14,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );

    // Run ordered rev1 -> rev2 -> rev3 -> rev4.
    let mut seq_state = seq_state;
    for fill in [rev1.clone(), rev2_seq, rev3_seq, rev4_seq] {
        let (next_s, _) = apply_ingest_trade_feed(
            &seq_state,
            &IngestTradeFeedInput {
                source_id: "src-1".to_owned(),
                fills: vec![fill],
                heartbeat_at: Some(now.clone()),
                actor_id: "feed-1".to_owned(),
            },
            &now,
            &mut id_seq,
        )
        .expect("seq ingest");
        seq_state = next_s;
    }

    // Run out-of-order: rev1 -> rev3 (missing rev2) -> rev4 (still missing rev2) -> rev2.
    let (ooo_state, _) = apply_ingest_trade_feed(
        &ooo_state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![rev1],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_ooo,
    )
    .expect("ooo rev1");
    assert_eq!(
        ooo_state.accounts["acct-1"].earmarks[&e50].state,
        EarmarkState::Reconciled
    );

    let rev3_ooo = make_fill(
        "src-1",
        "TRD-X",
        3,
        12,
        12,
        12,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(45),
        Some(quote_clean(100 * SCALE, Some(d_million(45)))),
        now.clone(),
    );
    let (ooo_state, out3) = apply_ingest_trade_feed(
        &ooo_state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![rev3_ooo],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_ooo,
    )
    .expect("ooo rev3");
    assert_eq!(
        out3.item_outcomes[0].kind,
        TradeFillOutcomeKind::BufferedRevision
    );
    let pos_after_r3 = ooo_state.position_view("acct-1", "240011.IB").expect("pos");
    let fund_after_r3 = ooo_state.funding_summary("acct-1").expect("fund");
    // Total sell occupancy MUST remain 50M (NOT 95M!), available_t0 remains 50M!
    assert_eq!(
        pos_after_r3.bucket_t0.reconciled_sell_notional,
        d_million(50)
    );
    assert_eq!(
        pos_after_r3
            .bucket_t0
            .pending_match_sell_unresolved_notional,
        FixedDecimal::ZERO
    );
    assert_eq!(pos_after_r3.available_t0_notional, d_million(50));
    assert_eq!(fund_after_r3.net_settlement_flow_t0, d_million(50));
    assert_eq!(fund_after_r3.pending_match_cash_t0, FixedDecimal::ZERO);
    assert_eq!(fund_after_r3.pending_match_count, 1);
    assert_eq!(fund_after_r3.pending_unknown_cash_count, 1);
    assert!(fund_after_r3.feed_incomplete);
    assert!(!fund_after_r3.funding_is_complete_exact);

    let rev4_ooo = make_fill(
        "src-1",
        "TRD-X",
        4,
        13,
        13,
        13,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );
    let (ooo_state, out4) = apply_ingest_trade_feed(
        &ooo_state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![rev4_ooo],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_ooo,
    )
    .expect("ooo rev4");
    assert_eq!(
        out4.item_outcomes[0].kind,
        TradeFillOutcomeKind::BufferedRevision
    );
    // Single active REVISION_HOLD per trade (same pending_match_id updated in-place), NEVER 135M!
    assert_eq!(
        out4.item_outcomes[0].pending_match_id,
        out3.item_outcomes[0].pending_match_id
    );
    let snap_r4 = ooo_state.snapshot_account("acct-1").expect("snap");
    assert_eq!(snap_r4.open_pending_matches.len(), 1);
    assert_eq!(snap_r4.open_pending_matches[0].source_revision, 4);
    assert_eq!(snap_r4.positions[0].available_t0_notional, d_million(50));
    assert_eq!(
        snap_r4.positions[0]
            .bucket_t0
            .pending_match_sell_unresolved_notional,
        FixedDecimal::ZERO
    );
    assert_eq!(
        snap_r4.funding_summary.pending_match_cash_t0,
        FixedDecimal::ZERO
    );
    assert_eq!(snap_r4.funding_summary.pending_match_count, 1);
    assert_eq!(snap_r4.funding_summary.pending_unknown_cash_count, 1);
    assert!(!snap_r4.funding_summary.funding_is_complete_exact);

    // Deliver missing rev2 -> unblocks [2, 3, 4], matches sequential execution field-for-field!
    let rev2_ooo = make_fill(
        "src-1",
        "TRD-X",
        2,
        14,
        14,
        14,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(48),
        Some(quote_clean(100 * SCALE, Some(d_million(48)))),
        now.clone(),
    );
    let (ooo_state, _) = apply_ingest_trade_feed(
        &ooo_state,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![rev2_ooo],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_ooo,
    )
    .expect("ooo rev2");

    let seq_pos = seq_state
        .position_view("acct-1", "240011.IB")
        .expect("seq pos");
    let ooo_pos = ooo_state
        .position_view("acct-1", "240011.IB")
        .expect("ooo pos");
    assert_eq!(ooo_pos.economic_net_notional, seq_pos.economic_net_notional);
    assert_eq!(ooo_pos.available_t0_notional, seq_pos.available_t0_notional);
    assert_eq!(ooo_pos.available_t1_notional, seq_pos.available_t1_notional);
    assert_eq!(ooo_pos.bucket_t0, seq_pos.bucket_t0);
    assert_eq!(ooo_pos.bucket_t1, seq_pos.bucket_t1);
    assert_eq!(
        ooo_state.funding_summary("acct-1").expect("ooo f"),
        seq_state.funding_summary("acct-1").expect("seq f")
    );
    assert_eq!(
        ooo_state.accounts["acct-1"].earmarks[&e50].state,
        seq_state.accounts["acct-1"].earmarks[&e50].state
    );
    assert_eq!(
        ooo_state.accounts["acct-1"].earmarks[&e50].unreconciled_notional,
        seq_state.accounts["acct-1"].earmarks[&e50].unreconciled_notional
    );
    assert_eq!(
        ooo_state.accounts["acct-1"].earmarks[&e50].unreconciled_settlement_cash,
        seq_state.accounts["acct-1"].earmarks[&e50].unreconciled_settlement_cash
    );
    assert_eq!(
        ooo_state.feeds["src-1"].trade_chains["TRD-X"].latest_effective_revision,
        seq_state.feeds["src-1"].trade_chains["TRD-X"].latest_effective_revision
    );
    let seq_open = seq_state
        .snapshot_account("acct-1")
        .expect("s")
        .open_pending_matches;
    let ooo_open = ooo_state
        .snapshot_account("acct-1")
        .expect("o")
        .open_pending_matches;
    assert_eq!(ooo_open.len(), seq_open.len());
    assert_eq!(ooo_open[0].reason, seq_open[0].reason);
    assert_eq!(ooo_open[0].source_revision, seq_open[0].source_revision);
    assert_eq!(ooo_open[0].original_notional, seq_open[0].original_notional);
    assert_eq!(
        ooo_open[0].unresolved_notional,
        seq_open[0].unresolved_notional
    );
    assert_eq!(
        ooo_open[0].unresolved_signed_cash,
        seq_open[0].unresolved_signed_cash
    );

    // Scenario 2: Pre-watermark baseline 50M sell -> rev2(40M) arrives before rev1(50M):
    // MUST NOT double-deduct 40M from opening_available=50M, and after rev1 matches ordered run!
    let mut id_b_seq = DeterministicIdGenerator::new(1_700_000_000_000, 100);
    let mut id_b_ooo = DeterministicIdGenerator::new(1_700_000_000_000, 100);
    let make_base_state = |ig: &mut DeterministicIdGenerator| {
        let st = BlotterState::new(BlotterConfig::default()).expect("cfg");
        let base = make_opening_base(
            "acct-base",
            "src-base",
            ig,
            vec![OpeningInstrumentBalance {
                instrument_code: "240011.IB".to_owned(),
                opening_economic_notional: d_million(-50),
                opening_available_notional: d_million(50),
                opening_net_t0_notional: FixedDecimal::ZERO,
                opening_net_t1_notional: FixedDecimal::ZERO,
            }],
            vec!["240011.IB".to_owned()],
            d_million(50),
            FixedDecimal::ZERO,
            None,
            10,
            Vec::new(),
        );
        apply_import_opening_base(
            &st,
            &ImportOpeningBaseCommand {
                actor_id: "trader-1".to_owned(),
                idempotency_key: "imp-b".to_owned(),
                opening_base: base,
                expected_account_version: 0,
            },
        )
        .expect("import")
        .0
    };
    let st_b_seq = make_base_state(&mut id_b_seq);
    let st_b_ooo = make_base_state(&mut id_b_ooo);

    let b_rev1 = make_fill(
        "src-base",
        "T-BASE",
        1,
        5,
        5,
        5,
        TradeFeedRecordAction::NewFill,
        "acct-base",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        shanghai_time(2026, 5, 18, 8, 55, 0),
    );
    let b_rev2 = make_fill(
        "src-base",
        "T-BASE",
        2,
        11,
        11,
        11,
        TradeFeedRecordAction::Amend,
        "acct-base",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );

    // Ordered: rev1 -> rev2
    let (st_b_seq, _) = apply_ingest_trade_feed(
        &st_b_seq,
        &IngestTradeFeedInput {
            source_id: "src-base".to_owned(),
            fills: vec![b_rev1.clone()],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_b_seq,
    )
    .expect("seq b_rev1");
    let (st_b_seq, _) = apply_ingest_trade_feed(
        &st_b_seq,
        &IngestTradeFeedInput {
            source_id: "src-base".to_owned(),
            fills: vec![b_rev2.clone()],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_b_seq,
    )
    .expect("seq b_rev2");

    // Out-of-order: rev2 -> check NO double-deduction -> rev1
    let (st_b_ooo, _) = apply_ingest_trade_feed(
        &st_b_ooo,
        &IngestTradeFeedInput {
            source_id: "src-base".to_owned(),
            fills: vec![b_rev2],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_b_ooo,
    )
    .expect("ooo b_rev2");
    let pos_b_held = st_b_ooo
        .position_view("acct-base", "240011.IB")
        .expect("pos");
    let fund_b_held = st_b_ooo.funding_summary("acct-base").expect("fund");
    assert_eq!(pos_b_held.available_t0_notional, d_million(50));
    assert_eq!(fund_b_held.pending_match_cash_t0, FixedDecimal::ZERO);
    assert_eq!(fund_b_held.pending_unknown_cash_count, 1);
    assert!(!fund_b_held.funding_is_complete_exact);

    let (st_b_ooo, _) = apply_ingest_trade_feed(
        &st_b_ooo,
        &IngestTradeFeedInput {
            source_id: "src-base".to_owned(),
            fills: vec![b_rev1],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 1, 0),
        &mut id_b_ooo,
    )
    .expect("ooo b_rev1");

    assert_eq!(
        st_b_ooo
            .position_view("acct-base", "240011.IB")
            .expect("p")
            .bucket_t0,
        st_b_seq
            .position_view("acct-base", "240011.IB")
            .expect("p")
            .bucket_t0
    );
    assert_eq!(
        st_b_ooo.funding_summary("acct-base").expect("f"),
        st_b_seq.funding_summary("acct-base").expect("f")
    );
}

#[test]
fn m1_fetch_trade_revision_multi_record_batch_orig_rev_gt_1_and_yield_only_shared_source() {
    let mut id_gen = DeterministicIdGenerator::default();
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);

    // Sub-case 1: FetchTradeRevision returns pre-cursor rev1 (seq=5 <= initial_after_feed_seq=10)
    // belonging to a multi-record historical source batch [4, 6] whose siblings 4 and 6 are never
    // pulled by the incremental stream. Must NOT stall in BufferedGap or keep feed_incomplete=true!
    let st = init_single_account(50, -50, d_million(50), FixedDecimal::ZERO, 10, &mut id_gen);
    let rev2 = make_fill(
        "src-1",
        "T-HIST",
        2,
        11,
        11,
        11,
        TradeFeedRecordAction::Amend,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );
    let (st, out2) = apply_ingest_trade_feed(
        &st,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![rev2],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest rev2");
    assert_eq!(
        out2.item_outcomes[0].kind,
        TradeFillOutcomeKind::BufferedRevision
    );

    let rev1_multi_batch = make_fill(
        "src-1",
        "T-HIST",
        1,
        5,
        4,
        6,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        shanghai_time(2026, 5, 18, 8, 50, 0),
    );
    let (st, out1) = apply_ingest_trade_feed(
        &st,
        &IngestTradeFeedInput {
            source_id: "src-1".to_owned(),
            fills: vec![rev1_multi_batch],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("backfill rev1 from multi-record historical batch [4, 6]");
    assert_eq!(
        out1.item_outcomes[0].kind,
        TradeFillOutcomeKind::PreWatermarkIgnored
    );
    let snap_hist = st.snapshot_account("acct-1").expect("snap");
    assert!(!snap_hist.funding_summary.feed_incomplete);
    assert!(snap_hist.feed_status.missing_feed_ranges.is_empty());
    assert_eq!(snap_hist.open_pending_matches.len(), 1);
    assert_eq!(
        snap_hist.open_pending_matches[0].reason,
        PendingMatchReason::BaselineRevisionManual
    );

    // Sub-case 2: BaseEvidence with original_revision = 2 -> replaying rev 2 is PreWatermarkIgnored
    // (NOT BaselineRevisionManual), while post-opening rev 3 creates BaselineRevisionManual.
    let ev_rev2 = BaseEvidence {
        source_id: "src-ev2".to_owned(),
        external_trade_id: "BASE-REV2".to_owned(),
        original_feed_seq: 8,
        original_executed_at: shanghai_time(2026, 5, 18, 8, 50, 0),
        old_notional: d_million(40),
        old_signed_cash: d_million(40),
        settlement_bucket: SettlementBucket::TPlus0,
        evidence_ref: "opening:BASE-REV2:2".to_owned(),
        evidence_hash: ContentHash::digest(b"BASE-REV2-v2"),
        instrument_code: "240011.IB".to_owned(),
        side: TradeSide::Sell,
        original_revision: 2,
    };
    let st2 = BlotterState::new(BlotterConfig::default()).expect("cfg");
    let base2 = make_opening_base(
        "acct-ev2",
        "src-ev2",
        &mut id_gen,
        vec![OpeningInstrumentBalance {
            instrument_code: "240011.IB".to_owned(),
            opening_economic_notional: d_million(-40),
            opening_available_notional: d_million(60),
            opening_net_t0_notional: FixedDecimal::ZERO,
            opening_net_t1_notional: FixedDecimal::ZERO,
        }],
        vec!["240011.IB".to_owned()],
        d_million(40),
        FixedDecimal::ZERO,
        Some(10),
        10,
        vec![ev_rev2],
    );
    let (st2, _) = apply_import_opening_base(
        &st2,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-ev2".to_owned(),
            opening_base: base2,
            expected_account_version: 0,
        },
    )
    .expect("import ev2");
    let replay_rev2 = make_fill(
        "src-ev2",
        "BASE-REV2",
        2,
        8,
        7,
        9,
        TradeFeedRecordAction::Amend,
        "acct-ev2",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        shanghai_time(2026, 5, 18, 8, 50, 0),
    );
    let (st2, out_ev2_r2) = apply_ingest_trade_feed(
        &st2,
        &IngestTradeFeedInput {
            source_id: "src-ev2".to_owned(),
            fills: vec![replay_rev2],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("replay pre-watermark rev2");
    assert_eq!(
        out_ev2_r2.item_outcomes[0].kind,
        TradeFillOutcomeKind::PreWatermarkIgnored
    );
    assert!(
        st2.snapshot_account("acct-ev2")
            .expect("s")
            .open_pending_matches
            .is_empty()
    );

    let post_rev3 = make_fill(
        "src-ev2",
        "BASE-REV2",
        3,
        11,
        11,
        11,
        TradeFeedRecordAction::Amend,
        "acct-ev2",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(35),
        Some(quote_clean(100 * SCALE, Some(d_million(35)))),
        now.clone(),
    );
    let (st2, out_ev2_r3) = apply_ingest_trade_feed(
        &st2,
        &IngestTradeFeedInput {
            source_id: "src-ev2".to_owned(),
            fills: vec![post_rev3],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("post-opening rev3");
    assert_eq!(
        out_ev2_r3.item_outcomes[0].kind,
        TradeFillOutcomeKind::BaselineReview
    );
    assert_eq!(
        st2.snapshot_account("acct-ev2")
            .expect("s")
            .open_pending_matches
            .len(),
        1
    );

    // Sub-case 3: Shared source where acct-a has watermark 15 and acct-b has watermark 10
    // (source_start_after_feed_seq = 10). Batch [11, 12] has acct-a yield-only pre-watermark
    // NEW_FILL at seq=11 and acct-b intraday fill at seq=12. Batch MUST NOT fail with
    // LineageIncomplete; seq=11 is PreWatermarkIgnored and seq=12 auto-matches acct-b!
    let st3 = BlotterState::new(BlotterConfig::default()).expect("cfg");
    let base_a = make_opening_base(
        "acct-a",
        "src-shared",
        &mut id_gen,
        vec![OpeningInstrumentBalance {
            instrument_code: "240011.IB".to_owned(),
            opening_economic_notional: d_million(100),
            opening_available_notional: d_million(100),
            opening_net_t0_notional: FixedDecimal::ZERO,
            opening_net_t1_notional: FixedDecimal::ZERO,
        }],
        vec!["240011.IB".to_owned()],
        d_million(100),
        FixedDecimal::ZERO,
        Some(15),
        10,
        Vec::new(),
    );
    let base_b = make_opening_base(
        "acct-b",
        "src-shared",
        &mut id_gen,
        vec![OpeningInstrumentBalance {
            instrument_code: "240011.IB".to_owned(),
            opening_economic_notional: d_million(100),
            opening_available_notional: d_million(100),
            opening_net_t0_notional: FixedDecimal::ZERO,
            opening_net_t1_notional: FixedDecimal::ZERO,
        }],
        vec!["240011.IB".to_owned()],
        d_million(100),
        FixedDecimal::ZERO,
        Some(10),
        10,
        Vec::new(),
    );
    let (st3, _) = apply_import_opening_base(
        &st3,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-a".to_owned(),
            opening_base: base_a,
            expected_account_version: 0,
        },
    )
    .expect("import a");
    let (st3, _) = apply_import_opening_base(
        &st3,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-b".to_owned(),
            opening_base: base_b,
            expected_account_version: 0,
        },
    )
    .expect("import b");
    let eb = id_gen.next_ulid();
    let (st3, _) = apply_create_committed(
        &st3,
        &CreateCommittedCommand {
            account_id: "acct-b".to_owned(),
            earmark_id: eb.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(50),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(50))),
            exact_dirty_price_input: None,
            note: "acct-b sell 50".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create eb");

    let a_yield_pre_wm = make_fill(
        "src-shared",
        "TRD-A-YTM",
        1,
        11,
        11,
        12,
        TradeFeedRecordAction::NewFill,
        "acct-a",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(30),
        Some(quote_yield(25_000_000_000, None)),
        shanghai_time(2026, 5, 18, 8, 59, 0),
    );
    let b_intraday = make_fill(
        "src-shared",
        "TRD-B-OK",
        1,
        12,
        11,
        12,
        TradeFeedRecordAction::NewFill,
        "acct-b",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );
    let (st3, out_shared) = apply_ingest_trade_feed(
        &st3,
        &IngestTradeFeedInput {
            source_id: "src-shared".to_owned(),
            fills: vec![a_yield_pre_wm, b_intraday],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("shared batch with pre-watermark yield-only NEW_FILL must succeed");
    assert_eq!(
        out_shared.item_outcomes[0].kind,
        TradeFillOutcomeKind::PreWatermarkIgnored
    );
    assert_eq!(
        out_shared.item_outcomes[1].kind,
        TradeFillOutcomeKind::AutoMatched
    );
    assert_eq!(
        st3.accounts["acct-b"].earmarks[&eb].state,
        EarmarkState::Reconciled
    );

    // If acct-a later receives a baseline AMEND on that unpriced pre-watermark trade, it enters
    // BaselineRevisionManual with evidence=None, unknown cash flagged, and APPLY_BASELINE_DELTA
    // fails with LineageIncomplete.
    let a_amend = make_fill(
        "src-shared",
        "TRD-A-YTM",
        2,
        13,
        13,
        13,
        TradeFeedRecordAction::Amend,
        "acct-a",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(25),
        Some(quote_clean(100 * SCALE, Some(d_million(25)))),
        now.clone(),
    );
    let (st3, out_amend) = apply_ingest_trade_feed(
        &st3,
        &IngestTradeFeedInput {
            source_id: "src-shared".to_owned(),
            fills: vec![a_amend],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 1, 0),
        &mut id_gen,
    )
    .expect("baseline amend on unpriced pre-watermark trade captured for manual review");
    assert_eq!(
        out_amend.item_outcomes[0].kind,
        TradeFillOutcomeKind::BaselineReview
    );
    let pm_unpriced = out_amend.item_outcomes[0]
        .pending_match_id
        .clone()
        .expect("pm");
    assert!(
        st3.accounts["acct-a"].pending_matches[&pm_unpriced]
            .evidence
            .is_none()
    );
    let fa = st3.funding_summary("acct-a").expect("fa");
    assert_eq!(fa.pending_unknown_cash_count, 1);
    assert!(!fa.funding_is_complete_exact);

    let pv_a = st3
        .position_view("acct-a", "240011.IB")
        .expect("pos")
        .position_version;
    let fake_ev = BaseEvidence {
        source_id: "src-shared".to_owned(),
        external_trade_id: "TRD-A-YTM".to_owned(),
        original_feed_seq: 11,
        original_executed_at: shanghai_time(2026, 5, 18, 8, 59, 0),
        old_notional: d_million(30),
        old_signed_cash: d_million(30),
        settlement_bucket: SettlementBucket::TPlus0,
        evidence_ref: "unproven".to_owned(),
        evidence_hash: ContentHash::digest(b"unproven"),
        instrument_code: "240011.IB".to_owned(),
        side: TradeSide::Sell,
        original_revision: 1,
    };
    let lineage_err = apply_resolve_pending_match(
        &st3,
        &ResolvePendingMatchInput {
            account_id: "acct-a".to_owned(),
            pending_match_id: pm_unpriced,
            expected_pending_revision: 1,
            expected_position_version: pv_a,
            action: PendingMatchResolutionAction::ApplyBaselineDelta,
            expected_earmark_revision: None,
            target_earmark_id: None,
            offset_notional: None,
            reason: "missing original cash".to_owned(),
            baseline_delta: Some(BaselineDelta {
                delta_notional: d_million(-5),
                delta_cash: d_million(-5),
                settlement_bucket: SettlementBucket::TPlus0,
                evidence: fake_ev,
                reason: "missing original cash".to_owned(),
            }),
            actor_id: "trader-1".to_owned(),
        },
        &mut id_gen,
    )
    .expect_err("APPLY_BASELINE_DELTA without proven BaseEvidence must fail LineageIncomplete");
    assert_eq!(lineage_err.kind(), BlotterErrorKind::LineageIncomplete);
}

#[test]
fn m2_revision_chain_account_id_immutability_rejects_cross_account_amend_and_cancel() {
    let mut id_gen = DeterministicIdGenerator::default();
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);

    let ev_acct1 = BaseEvidence {
        source_id: "src-shared".to_owned(),
        external_trade_id: "BASE-IMMUT".to_owned(),
        original_feed_seq: 5,
        original_executed_at: shanghai_time(2026, 5, 18, 8, 50, 0),
        old_notional: d_million(50),
        old_signed_cash: d_million(50),
        settlement_bucket: SettlementBucket::TPlus0,
        evidence_ref: "opening:BASE-IMMUT".to_owned(),
        evidence_hash: ContentHash::digest(b"BASE-IMMUT"),
        instrument_code: "240011.IB".to_owned(),
        side: TradeSide::Sell,
        original_revision: 1,
    };

    let state = BlotterState::new(BlotterConfig::default()).expect("cfg");
    let base1 = make_opening_base(
        "acct-1",
        "src-shared",
        &mut id_gen,
        vec![OpeningInstrumentBalance {
            instrument_code: "240011.IB".to_owned(),
            opening_economic_notional: d_million(100),
            opening_available_notional: d_million(100),
            opening_net_t0_notional: FixedDecimal::ZERO,
            opening_net_t1_notional: FixedDecimal::ZERO,
        }],
        vec!["240011.IB".to_owned()],
        d_million(100),
        FixedDecimal::ZERO,
        Some(10),
        10,
        vec![ev_acct1],
    );
    let base2 = make_opening_base(
        "acct-2",
        "src-shared",
        &mut id_gen,
        vec![OpeningInstrumentBalance {
            instrument_code: "240011.IB".to_owned(),
            opening_economic_notional: d_million(100),
            opening_available_notional: d_million(100),
            opening_net_t0_notional: FixedDecimal::ZERO,
            opening_net_t1_notional: FixedDecimal::ZERO,
        }],
        vec!["240011.IB".to_owned()],
        d_million(100),
        FixedDecimal::ZERO,
        Some(10),
        10,
        Vec::new(),
    );
    let (state, _) = apply_import_opening_base(
        &state,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-1".to_owned(),
            opening_base: base1,
            expected_account_version: 0,
        },
    )
    .expect("imp-1");
    let (state, _) = apply_import_opening_base(
        &state,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-2".to_owned(),
            opening_base: base2,
            expected_account_version: 0,
        },
    )
    .expect("imp-2");

    // Reconcile TRD-1 rev 1 on acct-1.
    let e1 = id_gen.next_ulid();
    let (state, _) = apply_create_committed(
        &state,
        &CreateCommittedCommand {
            account_id: "acct-1".to_owned(),
            earmark_id: e1.clone(),
            expected_position_version: 1,
            instrument_code: "240011.IB".to_owned(),
            side: TradeSide::Sell,
            settlement_bucket: SettlementBucket::TPlus0,
            notional: d_million(50),
            quote_terms: quote_clean(100 * SCALE, Some(d_million(50))),
            exact_dirty_price_input: None,
            note: "acct-1 sell 50".to_owned(),
            trader_id: "trader-1".to_owned(),
        },
        &now,
    )
    .expect("create e1");
    let trd1_r1 = make_fill(
        "src-shared",
        "TRD-1",
        1,
        11,
        11,
        11,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(50),
        Some(quote_clean(100 * SCALE, Some(d_million(50)))),
        now.clone(),
    );
    let (state, _) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-shared".to_owned(),
            fills: vec![trd1_r1],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("auto match TRD-1 on acct-1");
    assert_eq!(
        state.accounts["acct-1"].earmarks[&e1].state,
        EarmarkState::Reconciled
    );

    // 1. Cross-account AMEND on TRD-1 (acct-2 instead of acct-1) alongside valid acct-2 fill ->
    // whole batch rejected, zero state changes.
    let spoof_amend = make_fill(
        "src-shared",
        "TRD-1",
        2,
        12,
        12,
        13,
        TradeFeedRecordAction::Amend,
        "acct-2",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );
    let innocent_acct2 = make_fill(
        "src-shared",
        "TRD-OK2",
        1,
        13,
        12,
        13,
        TradeFeedRecordAction::NewFill,
        "acct-2",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(10),
        Some(quote_clean(100 * SCALE, Some(d_million(10)))),
        now.clone(),
    );
    let err1 = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-shared".to_owned(),
            fills: vec![spoof_amend, innocent_acct2],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect_err("cross-account AMEND must reject entire batch");
    assert_eq!(err1.kind(), BlotterErrorKind::ValidationFailed);

    // 2. Cross-account CANCEL_REVERSAL on TRD-1 -> rejected.
    let spoof_cancel = make_fill(
        "src-shared",
        "TRD-1",
        2,
        12,
        12,
        12,
        TradeFeedRecordAction::CancelReversal,
        "acct-2",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        FixedDecimal::ZERO,
        None,
        now.clone(),
    );
    let err2 = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-shared".to_owned(),
            fills: vec![spoof_cancel],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect_err("cross-account CANCEL_REVERSAL must reject entire batch");
    assert_eq!(err2.kind(), BlotterErrorKind::ValidationFailed);

    // 3. Cross-account tampering against BaseEvidence (BASE-IMMUT belongs to acct-1) -> rejected.
    let spoof_base = make_fill(
        "src-shared",
        "BASE-IMMUT",
        2,
        12,
        12,
        12,
        TradeFeedRecordAction::Amend,
        "acct-2",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(40),
        Some(quote_clean(100 * SCALE, Some(d_million(40)))),
        now.clone(),
    );
    let err3 = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-shared".to_owned(),
            fills: vec![spoof_base],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect_err("cross-account AMEND against BaseEvidence must reject entire batch");
    assert_eq!(err3.kind(), BlotterErrorKind::ValidationFailed);

    // 4. Cross-account mismatch within the SAME batch -> rejected.
    let same_batch_r1 = make_fill(
        "src-shared",
        "TRD-SAME",
        1,
        12,
        12,
        13,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(10),
        Some(quote_clean(100 * SCALE, Some(d_million(10)))),
        now.clone(),
    );
    let same_batch_r2 = make_fill(
        "src-shared",
        "TRD-SAME",
        2,
        13,
        12,
        13,
        TradeFeedRecordAction::Amend,
        "acct-2",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(12),
        Some(quote_clean(100 * SCALE, Some(d_million(12)))),
        now.clone(),
    );
    let err4 = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-shared".to_owned(),
            fills: vec![same_batch_r1, same_batch_r2],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 1, 0),
        &mut id_gen,
    )
    .expect_err("in-batch cross-account revision must reject entire batch");
    assert_eq!(err4.kind(), BlotterErrorKind::ValidationFailed);
}

#[test]
fn m3_u256_big_endian_limb_ordering_across_all_limbs() {
    assert!(U256::from_u128(1) < U256::from_u128(1_u128 << 64));
    assert!(U256([u64::MAX, 0, 0, 0]) < U256([0, 1, 0, 0]));
    assert!(U256([u64::MAX, u64::MAX, 0, 0]) < U256([0, 0, 1, 0]));
    assert!(U256([u64::MAX, u64::MAX, u64::MAX, 0]) < U256([0, 0, 0, 1]));
    assert!(U256([1, 2, 3, 4]) < U256([0, 0, 0, 5]));
    assert!(U256([0, 0, 0, 5]) > U256([u64::MAX, u64::MAX, u64::MAX, 4]));
    assert_eq!(
        U256([7, 8, 9, 10]).cmp(&U256([7, 8, 9, 10])),
        std::cmp::Ordering::Equal
    );

    let p_small = mul_u128_wide(1_u128 << 64, 1_u128 << 64);
    let p_large = mul_u128_wide(1_u128 << 64, (1_u128 << 64) + 1);
    assert!(p_small < p_large);
}

#[test]
fn m2_m3_in_batch_alias_effective_revision_and_shared_source_affected_accounts() {
    let mut id_gen = DeterministicIdGenerator::default();
    let now = shanghai_time(2026, 5, 18, 10, 0, 0);

    let state = BlotterState::new(BlotterConfig::default()).expect("cfg");
    let base1 = make_opening_base(
        "acct-1",
        "src-shared",
        &mut id_gen,
        vec![OpeningInstrumentBalance {
            instrument_code: "240011.IB".to_owned(),
            opening_economic_notional: d_million(100),
            opening_available_notional: d_million(100),
            opening_net_t0_notional: FixedDecimal::ZERO,
            opening_net_t1_notional: FixedDecimal::ZERO,
        }],
        vec!["240011.IB".to_owned()],
        d_million(100),
        FixedDecimal::ZERO,
        Some(10),
        10,
        Vec::new(),
    );
    let base2 = make_opening_base(
        "acct-2",
        "src-shared",
        &mut id_gen,
        vec![OpeningInstrumentBalance {
            instrument_code: "240011.IB".to_owned(),
            opening_economic_notional: d_million(100),
            opening_available_notional: d_million(100),
            opening_net_t0_notional: FixedDecimal::ZERO,
            opening_net_t1_notional: FixedDecimal::ZERO,
        }],
        vec!["240011.IB".to_owned()],
        d_million(100),
        FixedDecimal::ZERO,
        Some(10),
        10,
        Vec::new(),
    );
    let (state, _) = apply_import_opening_base(
        &state,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-1".to_owned(),
            opening_base: base1,
            expected_account_version: 0,
        },
    )
    .expect("imp-1");
    let (state, _) = apply_import_opening_base(
        &state,
        &ImportOpeningBaseCommand {
            actor_id: "trader-1".to_owned(),
            idempotency_key: "imp-2".to_owned(),
            opening_base: base2,
            expected_account_version: 0,
        },
    )
    .expect("imp-2");
    let acct1_v0 = state.accounts["acct-1"].account_version;
    let acct2_v0 = state.accounts["acct-2"].account_version;

    // 1. In-batch canonical (seq=11) + same-seq duplicate (seq=11) + new-seq alias (seq=12) on
    // acct-1: ALL outcomes report effective_revision == Some(1), ONLY acct-1 bumps account_version,
    // while BOTH acct-1 and acct-2 appear in feed_status_affected_accounts / affected_accounts!
    let canon = make_fill(
        "src-shared",
        "TRD-ALIAS",
        1,
        11,
        11,
        12,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(20),
        Some(quote_clean(100 * SCALE, Some(d_million(20)))),
        now.clone(),
    );
    let alias = make_fill(
        "src-shared",
        "TRD-ALIAS",
        1,
        12,
        11,
        12,
        TradeFeedRecordAction::NewFill,
        "acct-1",
        "240011.IB",
        TradeSide::Sell,
        t_day(),
        d_million(20),
        Some(quote_clean(100 * SCALE, Some(d_million(20)))),
        now.clone(),
    );
    let (state, out1) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-shared".to_owned(),
            fills: vec![canon.clone(), canon.clone(), alias.clone()],
            heartbeat_at: Some(now.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &now,
        &mut id_gen,
    )
    .expect("ingest canonical + dup + alias");
    assert_eq!(out1.item_outcomes.len(), 3);
    assert_eq!(
        out1.item_outcomes[0].kind,
        TradeFillOutcomeKind::PendingMatch
    );
    assert_eq!(out1.item_outcomes[0].effective_revision, Some(1));
    assert_eq!(
        out1.item_outcomes[1].kind,
        TradeFillOutcomeKind::PerItemDeduplicated
    );
    assert_eq!(out1.item_outcomes[1].effective_revision, Some(1));
    assert_eq!(
        out1.item_outcomes[2].kind,
        TradeFillOutcomeKind::PerItemDeduplicated
    );
    assert_eq!(
        out1.item_outcomes[2].effective_revision,
        Some(1),
        "in-batch alias feed_seq must backfill effective_revision after batch processing"
    );
    assert_eq!(out1.version_bumped_accounts, vec!["acct-1".to_owned()]);
    assert_eq!(
        out1.feed_status_affected_accounts,
        vec!["acct-1".to_owned(), "acct-2".to_owned()]
    );
    assert_eq!(
        out1.affected_accounts,
        vec!["acct-1".to_owned(), "acct-2".to_owned()]
    );
    assert_eq!(state.accounts["acct-1"].account_version, acct1_v0 + 1);
    assert_eq!(state.accounts["acct-2"].account_version, acct2_v0);

    // 2. Duplicate re-delivery -> mutated=false, all affected sets empty.
    let (state, dup_out) = apply_ingest_trade_feed(
        &state,
        &IngestTradeFeedInput {
            source_id: "src-shared".to_owned(),
            fills: vec![canon, alias],
            heartbeat_at: Some(now),
            actor_id: "feed-1".to_owned(),
        },
        &shanghai_time(2026, 5, 18, 10, 1, 0),
        &mut id_gen,
    )
    .expect("dup batch");
    assert!(!dup_out.mutated);
    assert!(dup_out.version_bumped_accounts.is_empty());
    assert!(dup_out.feed_status_affected_accounts.is_empty());
    assert!(dup_out.affected_accounts.is_empty());

    // 3. Stale sweep -> pure heartbeat clears paused_by_staleness:
    // version_bumped_accounts is EMPTY (neither account version increments), while BOTH accounts
    // appear in feed_status_affected_accounts and affected_accounts!
    let stale_t = shanghai_time(2026, 5, 18, 10, 10, 0);
    let (state_stale, _) =
        apply_sweep_lifecycle(&state, &SweepLifecycleCommand::default(), &stale_t)
            .expect("sweep stale");
    assert!(
        state_stale
            .feed_status("src-shared")
            .expect("fs")
            .paused_by_staleness
    );
    let acct1_pre_hb = state_stale.accounts["acct-1"].account_version;
    let acct2_pre_hb = state_stale.accounts["acct-2"].account_version;

    let hb_t = shanghai_time(2026, 5, 18, 10, 11, 0);
    let (state_hb, hb_out) = apply_ingest_trade_feed(
        &state_stale,
        &IngestTradeFeedInput {
            source_id: "src-shared".to_owned(),
            fills: Vec::new(),
            heartbeat_at: Some(hb_t.clone()),
            actor_id: "feed-1".to_owned(),
        },
        &hb_t,
        &mut id_gen,
    )
    .expect("pure heartbeat");
    assert!(hb_out.mutated);
    assert!(!hb_out.feed_status.paused_by_staleness);
    assert!(hb_out.version_bumped_accounts.is_empty());
    assert_eq!(
        hb_out.feed_status_affected_accounts,
        vec!["acct-1".to_owned(), "acct-2".to_owned()]
    );
    assert_eq!(
        hb_out.affected_accounts,
        vec!["acct-1".to_owned(), "acct-2".to_owned()]
    );
    assert_eq!(state_hb.accounts["acct-1"].account_version, acct1_pre_hb);
    assert_eq!(state_hb.accounts["acct-2"].account_version, acct2_pre_hb);
}
