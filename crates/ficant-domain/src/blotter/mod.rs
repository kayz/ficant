//! Pure synchronous in-memory domain state machine for intraday Desk Blotter
//! position reservation, source-batch trade feed reconciliation, baseline revision evidence,
//! funding gap projection, and overflow-safe wide-integer cumulative allocation.
//!
//! This module contains zero database, filesystem, network, clock I/O, async runtime, or
//! `unsafe` dependencies.

pub mod engine;
pub mod error;
pub mod funding;
pub mod model;
pub mod position;
pub mod reconcile;
pub mod wide_math;

pub use engine::{
    BlotterState, CancelCommittedCommand, CancelCommittedOutput, ConfirmHoldCommand,
    ConfirmHoldOutput, CreateCommittedCommand, CreateCommittedOutput, CreateHoldCommand,
    CreateHoldOutput, ExtendHoldCommand, ExtendHoldOutput, ImportOpeningBaseCommand,
    ImportOpeningBaseOutput, IngestTradeFeedOutput, RegisterDraftCommand, RegisterDraftOutput,
    ReleaseHoldCommand, ReleaseHoldOutput, ResolvePendingMatchOutput, SetFeedControlCommand,
    SetFeedControlOutput, SweepLifecycleCommand, SweepLifecycleOutput, apply_cancel_committed,
    apply_confirm_hold, apply_create_committed, apply_create_hold, apply_extend_hold,
    apply_import_opening_base, apply_ingest_trade_feed, apply_register_draft, apply_release_hold,
    apply_resolve_pending_match, apply_set_feed_control, apply_sweep_lifecycle,
};
pub use error::{BlotterError, BlotterErrorKind, BlotterResult};
pub use funding::{
    ResolvedSettlementCash, apply_cash_sign, compute_funding_summary, resolve_settlement_cash,
};
pub use model::{
    AccountBlotterSnapshot, AccountState, BaseEvidence, BaselineDelta, BlotterConfig,
    BlotterIdGenerator, BucketBalanceView, DeterministicIdGenerator, DraftPresenceRecord,
    EarmarkRecord, EarmarkState, ExactDirtyPriceCashInput, FeedHealthState, FeedStatusView,
    FundingGapSummary, FundingPrecision, InstrumentPositionState, InstrumentPositionView,
    MAX_COMMITTED_OVERDUE_SECONDS, MAX_HOLD_TTL_SECONDS, MIN_COMMITTED_OVERDUE_SECONDS,
    MIN_HOLD_TTL_SECONDS, MatchKey, OpeningImportReceipt, OpeningInstrumentBalance,
    OpeningPositionBase, PendingMatchReason, PendingMatchRecord, PendingMatchResolutionAction,
    PendingMatchState, QuoteBasis, QuoteTerms, ReconcileAllocationRecord, SeqRange,
    SettlementBucket, SourceBatchRecord, SourceFeedState, StoredRawPacket, StoredTradeRevision,
    TradeChainState, TradeFeedRecordAction, TradeFillIngestOutcome, TradeFillOutcomeKind,
    TradeFillRecord, TradeRevisionLifecycle, TradeSide,
};
pub use position::{
    project_instrument_position, validate_in_term_confirm_capacity, validate_new_earmark_capacity,
};
pub use reconcile::{
    IngestTradeFeedEffect, IngestTradeFeedInput, ResolvePendingMatchEffect,
    ResolvePendingMatchInput,
};
pub use wide_math::{
    AllocationPool, AllocationSegment, U256, cumulative_cash_magnitude, div_rem_u256_by_u128,
    mul_u128_wide, wide_mul_div_fixed_round_half_even, wide_mul_div_u128_round_half_even,
};
